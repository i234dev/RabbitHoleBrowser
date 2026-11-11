//! PulseAudio (and PipeWire through pipewire-pulse) output via the
//! "simple" API of `libpulse-simple.so.0`, loaded at run time so the
//! binary has no link-time dependency on it.

use super::Backend;
use libloading::Library;
use std::ffi::{c_char, c_int, c_void, CStr};

#[repr(C)]
struct SampleSpec {
    format: u32,
    rate: u32,
    channels: u8,
}

#[repr(C)]
struct BufferAttr {
    maxlength: u32,
    tlength: u32,
    prebuf: u32,
    minreq: u32,
    fragsize: u32,
}

const PA_SAMPLE_FLOAT32LE: u32 = 5;
const PA_STREAM_PLAYBACK: c_int = 1;

type NewFn = unsafe extern "C" fn(*const c_char, *const c_char, c_int, *const c_char, *const c_char, *const SampleSpec, *const c_void, *const BufferAttr, *mut c_int) -> *mut c_void;
type WriteFn = unsafe extern "C" fn(*mut c_void, *const c_void, usize, *mut c_int) -> c_int;
type LatencyFn = unsafe extern "C" fn(*mut c_void, *mut c_int) -> u64;
type FreeFn = unsafe extern "C" fn(*mut c_void);
type StrErrorFn = unsafe extern "C" fn(c_int) -> *const c_char;

pub(crate) struct PulseBackend {
    _lib: Library,
    strerror: Option<StrErrorFn>,
    write: WriteFn,
    latency: LatencyFn,
    free: FreeFn,
    conn: *mut c_void,
    rate: u32,
    latency_secs: f64,
    bytes: Vec<u8>,
}

// The connection is only used from the mixer thread that owns the backend.
unsafe impl Send for PulseBackend {}

const RATE: u32 = 48_000;
/// Frames per write (10 ms).
const CHUNK: usize = 480;

impl PulseBackend {
    pub(crate) fn open() -> Result<PulseBackend, String> {
        // SAFETY: loading a well-known system library; its initialisers are
        // trusted like any system library linked at build time.
        let lib = unsafe { Library::new("libpulse-simple.so.0") }.map_err(|e| e.to_string())?;
        // SAFETY: the symbol types match the libpulse-simple C API.
        unsafe {
            let new: NewFn = *lib.get::<NewFn>(b"pa_simple_new\0").map_err(|e| e.to_string())?;
            let write: WriteFn = *lib.get::<WriteFn>(b"pa_simple_write\0").map_err(|e| e.to_string())?;
            let latency: LatencyFn = *lib.get::<LatencyFn>(b"pa_simple_get_latency\0").map_err(|e| e.to_string())?;
            let free: FreeFn = *lib.get::<FreeFn>(b"pa_simple_free\0").map_err(|e| e.to_string())?;
            // pa_strerror lives in libpulse, which libpulse-simple links.
            let strerror = Library::new("libpulse.so.0").ok().and_then(|l| {
                let f = l.get::<StrErrorFn>(b"pa_strerror\0").ok().map(|s| *s);
                std::mem::forget(l);
                f
            });
            let spec = SampleSpec { format: PA_SAMPLE_FLOAT32LE, rate: RATE, channels: 2 };
            let bytes_per_ms = RATE / 1000 * 8;
            let attr = BufferAttr { maxlength: u32::MAX, tlength: bytes_per_ms * 60, prebuf: u32::MAX, minreq: bytes_per_ms * 10, fragsize: u32::MAX };
            let mut err: c_int = 0;
            let conn = new(std::ptr::null(), c"RabbitHole".as_ptr(), PA_STREAM_PLAYBACK, std::ptr::null(), c"Media playback".as_ptr(), &spec, std::ptr::null(), &attr, &mut err);
            if conn.is_null() {
                let msg = strerror.map(|f| CStr::from_ptr(f(err)).to_string_lossy().into_owned()).unwrap_or_else(|| format!("error {err}"));
                return Err(msg);
            }
            Ok(PulseBackend { _lib: lib, strerror, write, latency, free, conn, rate: RATE, latency_secs: 0.06, bytes: Vec::new() })
        }
    }

    fn error(&self, err: c_int) -> String {
        match self.strerror {
            // SAFETY: pa_strerror returns a static string.
            Some(f) => unsafe { CStr::from_ptr(f(err)).to_string_lossy().into_owned() },
            None => format!("error {err}"),
        }
    }
}

impl Backend for PulseBackend {
    fn rate(&self) -> u32 {
        self.rate
    }
    fn channels(&self) -> usize {
        2
    }
    fn wait(&mut self) -> Result<usize, String> {
        // pa_simple_write blocks while the server's buffer is full, which
        // paces the mixer.
        Ok(CHUNK)
    }
    fn write(&mut self, data: &[f32]) -> Result<(), String> {
        self.bytes.clear();
        for s in data {
            self.bytes.extend_from_slice(&s.to_le_bytes());
        }
        let mut err: c_int = 0;
        // SAFETY: `conn` is a live connection; the buffer outlives the call.
        let r = unsafe { (self.write)(self.conn, self.bytes.as_ptr() as *const c_void, self.bytes.len(), &mut err) };
        if r < 0 {
            return Err(self.error(err));
        }
        // SAFETY: as above.
        let us = unsafe { (self.latency)(self.conn, &mut err) };
        if us != u64::MAX && us < 2_000_000 {
            self.latency_secs = us as f64 / 1e6;
        }
        Ok(())
    }
    fn latency(&self) -> f64 {
        self.latency_secs
    }
    fn name(&self) -> &'static str {
        "pulse"
    }
}

impl Drop for PulseBackend {
    fn drop(&mut self) {
        // SAFETY: freed once; the library is still loaded (`_lib` drops after).
        unsafe { (self.free)(self.conn) };
    }
}
