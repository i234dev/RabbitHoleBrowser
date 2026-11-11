//! ALSA output through `libasound.so.2`, loaded at run time (no link-time
//! dependency). Uses the "default" PCM, which goes through dmix /
//! pipewire-alsa / pulse plugins as configured.

use super::Backend;
use libloading::Library;
use std::ffi::{c_char, c_int, c_long, c_uint, c_void, CStr};

type OpenFn = unsafe extern "C" fn(*mut *mut c_void, *const c_char, c_int, c_int) -> c_int;
type SetParamsFn = unsafe extern "C" fn(*mut c_void, c_int, c_int, c_uint, c_uint, c_int, c_uint) -> c_int;
type WriteiFn = unsafe extern "C" fn(*mut c_void, *const c_void, c_ulong) -> c_long;
type RecoverFn = unsafe extern "C" fn(*mut c_void, c_int, c_int) -> c_int;
type CloseFn = unsafe extern "C" fn(*mut c_void) -> c_int;
type DelayFn = unsafe extern "C" fn(*mut c_void, *mut c_long) -> c_int;
type StrErrorFn = unsafe extern "C" fn(c_int) -> *const c_char;
#[allow(non_camel_case_types)]
type c_ulong = std::ffi::c_ulong;

const SND_PCM_STREAM_PLAYBACK: c_int = 0;
const SND_PCM_FORMAT_S16_LE: c_int = 2;
const SND_PCM_FORMAT_FLOAT_LE: c_int = 14;
const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;

pub(crate) struct AlsaBackend {
    _lib: Library,
    writei: WriteiFn,
    recover: RecoverFn,
    close: CloseFn,
    delay: DelayFn,
    strerror: StrErrorFn,
    pcm: *mut c_void,
    float: bool,
    rate: u32,
    latency_secs: f64,
    s16: Vec<i16>,
}

// The PCM handle is only used from the mixer thread that owns the backend.
unsafe impl Send for AlsaBackend {}

const RATE: u32 = 48_000;
const CHUNK: usize = 480;

impl AlsaBackend {
    pub(crate) fn open() -> Result<AlsaBackend, String> {
        // SAFETY: loading the system ALSA library.
        let lib = unsafe { Library::new("libasound.so.2") }.map_err(|e| e.to_string())?;
        // SAFETY: the symbol types match the alsa-lib C API.
        unsafe {
            let open: OpenFn = *lib.get::<OpenFn>(b"snd_pcm_open\0").map_err(|e| e.to_string())?;
            let set_params: SetParamsFn = *lib.get::<SetParamsFn>(b"snd_pcm_set_params\0").map_err(|e| e.to_string())?;
            let writei: WriteiFn = *lib.get::<WriteiFn>(b"snd_pcm_writei\0").map_err(|e| e.to_string())?;
            let recover: RecoverFn = *lib.get::<RecoverFn>(b"snd_pcm_recover\0").map_err(|e| e.to_string())?;
            let close: CloseFn = *lib.get::<CloseFn>(b"snd_pcm_close\0").map_err(|e| e.to_string())?;
            let delay: DelayFn = *lib.get::<DelayFn>(b"snd_pcm_delay\0").map_err(|e| e.to_string())?;
            let strerror: StrErrorFn = *lib.get::<StrErrorFn>(b"snd_strerror\0").map_err(|e| e.to_string())?;
            let err = |e: c_int| CStr::from_ptr(strerror(e)).to_string_lossy().into_owned();
            let mut pcm: *mut c_void = std::ptr::null_mut();
            let r = open(&mut pcm, c"default".as_ptr(), SND_PCM_STREAM_PLAYBACK, 0);
            if r < 0 || pcm.is_null() {
                return Err(err(r));
            }
            let mut float = true;
            let mut r = set_params(pcm, SND_PCM_FORMAT_FLOAT_LE, SND_PCM_ACCESS_RW_INTERLEAVED, 2, RATE, 1, 60_000);
            if r < 0 {
                float = false;
                r = set_params(pcm, SND_PCM_FORMAT_S16_LE, SND_PCM_ACCESS_RW_INTERLEAVED, 2, RATE, 1, 60_000);
            }
            if r < 0 {
                close(pcm);
                return Err(err(r));
            }
            Ok(AlsaBackend { _lib: lib, writei, recover, close, delay, strerror, pcm, float, rate: RATE, latency_secs: 0.06, s16: Vec::new() })
        }
    }
}

impl Backend for AlsaBackend {
    fn rate(&self) -> u32 {
        self.rate
    }
    fn channels(&self) -> usize {
        2
    }
    fn wait(&mut self) -> Result<usize, String> {
        // snd_pcm_writei blocks while the ring buffer is full.
        Ok(CHUNK)
    }
    fn write(&mut self, data: &[f32]) -> Result<(), String> {
        let frames = data.len() / 2;
        let ptr: *const c_void = if self.float {
            data.as_ptr() as *const c_void
        } else {
            self.s16.clear();
            self.s16.extend(data.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16));
            self.s16.as_ptr() as *const c_void
        };
        let mut done = 0usize;
        let mut tries = 0;
        while done < frames {
            let bytes_per_frame = if self.float { 8 } else { 4 };
            // SAFETY: `pcm` is open; the pointer covers `frames - done` frames.
            let r = unsafe { (self.writei)(self.pcm, (ptr as *const u8).add(done * bytes_per_frame) as *const c_void, (frames - done) as c_ulong) };
            if r < 0 {
                tries += 1;
                // SAFETY: as above.
                let rr = unsafe { (self.recover)(self.pcm, r as c_int, 1) };
                if rr < 0 || tries > 3 {
                    // SAFETY: snd_strerror returns a static string.
                    return Err(unsafe { CStr::from_ptr((self.strerror)(rr)).to_string_lossy().into_owned() });
                }
                continue;
            }
            done += r as usize;
        }
        let mut d: c_long = 0;
        // SAFETY: as above.
        if unsafe { (self.delay)(self.pcm, &mut d) } >= 0 && d >= 0 {
            self.latency_secs = d as f64 / self.rate as f64;
        }
        Ok(())
    }
    fn latency(&self) -> f64 {
        self.latency_secs
    }
    fn name(&self) -> &'static str {
        "alsa"
    }
}

impl Drop for AlsaBackend {
    fn drop(&mut self) {
        // SAFETY: closed once, library still loaded.
        unsafe { (self.close)(self.pcm) };
    }
}
