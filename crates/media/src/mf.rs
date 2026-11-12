//! H.264 decoding through Windows Media Foundation's decoder MFT
//! (`CLSID_CMSH264DecoderMFT`, hardware accelerated where the driver
//! offers it). `mfplat.dll` is loaded at run time (absent on Windows "N"
//! editions without the Media Feature Pack: then H.264 is reported as
//! unsupported); the COM interfaces are declared by hand.

use crate::video::{h264_to_annexb, avcc_parameter_sets, nv12_to_rgba, Matrix, VideoDecoder, VideoFrame};
use std::ffi::c_void;
use std::sync::{Arc, OnceLock};
use windows_sys::core::{GUID, HRESULT};
use windows_sys::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};

const CLSID_CMSH264_DECODER_MFT: GUID = GUID::from_u128(0x62ce7e72_4c71_4d20_b15d_452831a87d9d);
const IID_IMF_TRANSFORM: GUID = GUID::from_u128(0xbf94c121_5b05_4e6f_8000_ba29961414d3);
const MF_MT_MAJOR_TYPE: GUID = GUID::from_u128(0x48eba18e_f8c9_4687_bf11_0a74c9f96a8f);
const MF_MT_SUBTYPE: GUID = GUID::from_u128(0xf7e34c9a_42e8_4714_b74b_cb29d72c35e5);
const MF_MT_FRAME_SIZE: GUID = GUID::from_u128(0x1652c33d_d6b2_4012_b834_72030849a37d);
const MF_MT_DEFAULT_STRIDE: GUID = GUID::from_u128(0x644b4e48_1e02_4516_b0eb_c01ca9d49ac6);
const MF_MT_YUV_MATRIX: GUID = GUID::from_u128(0x3e23d450_2c75_4d25_a00e_b91670d12327);
const MF_MEDIA_TYPE_VIDEO: GUID = GUID::from_u128(0x73646976_0000_0010_8000_00aa00389b71);
const MF_VIDEO_FORMAT_H264: GUID = GUID::from_u128(0x34363248_0000_0010_8000_00aa00389b71);
const MF_VIDEO_FORMAT_NV12: GUID = GUID::from_u128(0x3231564e_0000_0010_8000_00aa00389b71);

const MF_VERSION: u32 = 0x0002_0070;
const MFT_MESSAGE_COMMAND_FLUSH: u32 = 0;
const MFT_MESSAGE_NOTIFY_BEGIN_STREAMING: u32 = 0x1000_0000;
const MFT_MESSAGE_NOTIFY_START_OF_STREAM: u32 = 0x1000_0003;
const MFT_OUTPUT_STREAM_PROVIDES_SAMPLES: u32 = 0x100;
const MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES: u32 = 0x200;
const MF_E_NOTACCEPTING: HRESULT = 0xC00D_36B5u32 as i32;
const MF_E_TRANSFORM_NEED_MORE_INPUT: HRESULT = 0xC00D_6D72u32 as i32;
const MF_E_TRANSFORM_STREAM_CHANGE: HRESULT = 0xC00D_6D61u32 as i32;
const MF_E_NO_MORE_TYPES: HRESULT = 0xC00D_36B9u32 as i32;

type StartupFn = unsafe extern "system" fn(u32, u32) -> HRESULT;
type CreateFn = unsafe extern "system" fn(*mut *mut c_void) -> HRESULT;
type CreateBufferFn = unsafe extern "system" fn(u32, *mut *mut c_void) -> HRESULT;

struct Api {
    create_media_type: CreateFn,
    create_sample: CreateFn,
    create_memory_buffer: CreateBufferFn,
}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        // SAFETY: loading a system DLL and resolving documented exports
        // with their documented signatures; the library is never unloaded.
        unsafe {
            let lib = libloading::Library::new("mfplat.dll").ok()?;
            let startup: StartupFn = *lib.get::<StartupFn>(b"MFStartup\0").ok()?;
            let create_media_type: CreateFn = *lib.get::<CreateFn>(b"MFCreateMediaType\0").ok()?;
            let create_sample: CreateFn = *lib.get::<CreateFn>(b"MFCreateSample\0").ok()?;
            let create_memory_buffer: CreateBufferFn = *lib.get::<CreateBufferFn>(b"MFCreateMemoryBuffer\0").ok()?;
            com_init();
            if startup(MF_VERSION, 0) < 0 {
                return None;
            }
            std::mem::forget(lib);
            Some(Api { create_media_type, create_sample, create_memory_buffer })
        }
    })
    .as_ref()
}

fn com_init() {
    // SAFETY: plain COM initialization; S_FALSE / RPC_E_CHANGED_MODE are
    // fine (COM is usable on this thread either way).
    unsafe {
        let _ = CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED as u32);
    }
}

/// Is the H.264 decoder MFT available? (Checked once, on a helper thread
/// so the caller's COM apartment is untouched.)
pub fn h264_available() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        std::thread::spawn(|| {
            com_init();
            api().is_some() && create_transform().is_some()
        })
        .join()
        .unwrap_or(false)
    })
}

// ---------------------------------------------------------------- vtables
// Slot numbers count from 0 at IUnknown::QueryInterface.

/// A COM interface pointer, released on drop.
struct Com(*mut c_void);

unsafe impl Send for Com {}

impl Com {
    /// Function pointer in vtable slot `i`.
    /// SAFETY: `self.0` must be a live interface with at least `i + 1` slots.
    unsafe fn slot(&self, i: usize) -> *const c_void {
        let vtbl = *(self.0 as *const *const *const c_void);
        *vtbl.add(i)
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: IUnknown::Release is slot 2 of every interface.
            unsafe {
                let release: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(self.slot(2));
                release(self.0);
            }
        }
    }
}

macro_rules! call {
    ($obj:expr, $slot:expr, fn($($t:ty),*) $(, $a:expr)*) => {{
        let f: unsafe extern "system" fn(*mut c_void $(, $t)*) -> HRESULT = std::mem::transmute($obj.slot($slot));
        f($obj.0 $(, $a)*)
    }};
}

mod aac;
pub use aac::MfAacDecoder;

// IMFAttributes
const ATTR_GET_UINT32: usize = 7;
const ATTR_GET_UINT64: usize = 8;
const ATTR_GET_GUID: usize = 10;
const ATTR_SET_GUID: usize = 24;
// IMFSample (after IMFAttributes' 33 slots)
const SAMPLE_GET_TIME: usize = 35;
const SAMPLE_SET_TIME: usize = 36;
const SAMPLE_CONTIGUOUS: usize = 41;
const SAMPLE_ADD_BUFFER: usize = 42;
// IMFMediaBuffer
const BUF_LOCK: usize = 3;
const BUF_UNLOCK: usize = 4;
const BUF_SET_LEN: usize = 6;
// IMFTransform
const T_GET_OUTPUT_STREAM_INFO: usize = 7;
const T_GET_OUTPUT_AVAILABLE_TYPE: usize = 14;
const T_SET_INPUT_TYPE: usize = 15;
const T_SET_OUTPUT_TYPE: usize = 16;
const T_PROCESS_MESSAGE: usize = 23;
const T_PROCESS_INPUT: usize = 24;
const T_PROCESS_OUTPUT: usize = 25;

#[repr(C)]
#[derive(Default)]
struct OutputStreamInfo {
    flags: u32,
    size: u32,
    alignment: u32,
}

#[repr(C)]
struct OutputDataBuffer {
    stream_id: u32,
    sample: *mut c_void,
    status: u32,
    events: *mut c_void,
}

fn create_transform() -> Option<Com> {
    let mut p: *mut c_void = std::ptr::null_mut();
    // SAFETY: documented CoCreateInstance call; checked result.
    let hr = unsafe { CoCreateInstance(&CLSID_CMSH264_DECODER_MFT, std::ptr::null_mut(), CLSCTX_INPROC_SERVER, &IID_IMF_TRANSFORM, &mut p) };
    if hr < 0 || p.is_null() {
        None
    } else {
        Some(Com(p))
    }
}

/// An H.264 decoder backed by the Media Foundation MFT.
pub struct MfH264Decoder {
    t: Com,
    param_sets: Vec<Vec<u8>>,
    width: usize,
    height: usize,
    stride: usize,
    matrix: Matrix,
    out_info: OutputStreamInfo,
}

impl MfH264Decoder {
    pub fn new(avcc: &[u8]) -> Option<MfH264Decoder> {
        com_init();
        let api = api()?;
        let t = create_transform()?;
        // SAFETY: COM calls on live interfaces with valid out pointers.
        unsafe {
            let mut mt: *mut c_void = std::ptr::null_mut();
            if (api.create_media_type)(&mut mt) < 0 || mt.is_null() {
                return None;
            }
            let mt = Com(mt);
            call!(mt, ATTR_SET_GUID, fn(*const GUID, *const GUID), &MF_MT_MAJOR_TYPE, &MF_MEDIA_TYPE_VIDEO);
            call!(mt, ATTR_SET_GUID, fn(*const GUID, *const GUID), &MF_MT_SUBTYPE, &MF_VIDEO_FORMAT_H264);
            if call!(t, T_SET_INPUT_TYPE, fn(u32, *mut c_void, u32), 0, mt.0, 0) < 0 {
                return None;
            }
            let mut d = MfH264Decoder { t, param_sets: avcc_parameter_sets(avcc), width: 0, height: 0, stride: 0, matrix: Matrix::Bt709, out_info: OutputStreamInfo::default() };
            if !d.negotiate_output() {
                return None;
            }
            call!(d.t, T_PROCESS_MESSAGE, fn(u32, usize), MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0);
            call!(d.t, T_PROCESS_MESSAGE, fn(u32, usize), MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0);
            Some(d)
        }
    }

    /// Pick the NV12 output type and read the picture geometry.
    fn negotiate_output(&mut self) -> bool {
        // SAFETY: as in `new`.
        unsafe {
            for i in 0..64u32 {
                let mut ty: *mut c_void = std::ptr::null_mut();
                let hr = call!(self.t, T_GET_OUTPUT_AVAILABLE_TYPE, fn(u32, u32, *mut *mut c_void), 0, i, &mut ty);
                if hr == MF_E_NO_MORE_TYPES || hr < 0 || ty.is_null() {
                    return false;
                }
                let ty = Com(ty);
                let mut sub = GUID::from_u128(0);
                call!(ty, ATTR_GET_GUID, fn(*const GUID, *mut GUID), &MF_MT_SUBTYPE, &mut sub);
                if !guid_eq(&sub, &MF_VIDEO_FORMAT_NV12) {
                    continue;
                }
                if call!(self.t, T_SET_OUTPUT_TYPE, fn(u32, *mut c_void, u32), 0, ty.0, 0) < 0 {
                    return false;
                }
                let mut size: u64 = 0;
                call!(ty, ATTR_GET_UINT64, fn(*const GUID, *mut u64), &MF_MT_FRAME_SIZE, &mut size);
                self.width = (size >> 32) as usize;
                self.height = (size & 0xffff_ffff) as usize;
                let mut stride: u32 = 0;
                if call!(ty, ATTR_GET_UINT32, fn(*const GUID, *mut u32), &MF_MT_DEFAULT_STRIDE, &mut stride) < 0 || (stride as i32) <= 0 {
                    stride = self.width as u32;
                }
                self.stride = stride as usize;
                let mut mtx: u32 = 0;
                if call!(ty, ATTR_GET_UINT32, fn(*const GUID, *mut u32), &MF_MT_YUV_MATRIX, &mut mtx) >= 0 {
                    // MFVideoTransferMatrix: 1 = BT.709, 2 = BT.601, 4 = BT.2020.
                    self.matrix = match mtx {
                        2 => Matrix::Bt601,
                        4 => Matrix::Bt2020,
                        _ => Matrix::Bt709,
                    };
                }
                let mut info = OutputStreamInfo::default();
                call!(self.t, T_GET_OUTPUT_STREAM_INFO, fn(u32, *mut OutputStreamInfo), 0, &mut info);
                self.out_info = info;
                return true;
            }
        }
        false
    }

    /// Pull every picture the MFT has ready.
    fn drain(&mut self, out: &mut Vec<VideoFrame>) {
        let Some(api) = api() else { return };
        for _ in 0..32 {
            // SAFETY: as in `new`; the sample we allocate (when the MFT does
            // not provide its own) outlives ProcessOutput.
            unsafe {
                let provides = self.out_info.flags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES) != 0;
                let mut own: Option<Com> = None;
                if !provides {
                    let mut s: *mut c_void = std::ptr::null_mut();
                    let mut b: *mut c_void = std::ptr::null_mut();
                    let size = self.out_info.size.max((self.stride * self.height * 3 / 2) as u32).max(1);
                    if (api.create_sample)(&mut s) < 0 || (api.create_memory_buffer)(size, &mut b) < 0 {
                        return;
                    }
                    let s = Com(s);
                    let b = Com(b);
                    call!(s, SAMPLE_ADD_BUFFER, fn(*mut c_void), b.0);
                    own = Some(s);
                }
                let mut db = OutputDataBuffer { stream_id: 0, sample: own.as_ref().map(|s| s.0).unwrap_or(std::ptr::null_mut()), status: 0, events: std::ptr::null_mut() };
                let mut status: u32 = 0;
                let hr = call!(self.t, T_PROCESS_OUTPUT, fn(u32, u32, *mut OutputDataBuffer, *mut u32), 0, 1, &mut db, &mut status);
                if !db.events.is_null() {
                    drop(Com(db.events));
                }
                if hr == MF_E_TRANSFORM_NEED_MORE_INPUT {
                    return;
                }
                if hr == MF_E_TRANSFORM_STREAM_CHANGE {
                    if !self.negotiate_output() {
                        return;
                    }
                    continue;
                }
                if hr < 0 || db.sample.is_null() {
                    return;
                }
                // The sample: ours, or one the MFT provided (we release it).
                let sample = match own {
                    Some(s) => s,
                    None => Com(db.sample),
                };
                let mut t100: i64 = 0;
                call!(sample, SAMPLE_GET_TIME, fn(*mut i64), &mut t100);
                let mut buf: *mut c_void = std::ptr::null_mut();
                if call!(sample, SAMPLE_CONTIGUOUS, fn(*mut *mut c_void), &mut buf) < 0 || buf.is_null() {
                    continue;
                }
                let buf = Com(buf);
                let mut ptr: *mut u8 = std::ptr::null_mut();
                let mut max: u32 = 0;
                let mut cur: u32 = 0;
                if call!(buf, BUF_LOCK, fn(*mut *mut u8, *mut u32, *mut u32), &mut ptr, &mut max, &mut cur) < 0 || ptr.is_null() {
                    continue;
                }
                let data = std::slice::from_raw_parts(ptr, cur as usize);
                let (w, h, stride) = (self.width, self.height, self.stride.max(self.width));
                // NV12: the UV plane follows the (16-aligned) Y plane.
                let y_rows = if data.len() >= stride * h.next_multiple_of(16) * 3 / 2 { h.next_multiple_of(16) } else { h };
                let rgba = if w > 0 && h > 0 && data.len() >= stride * y_rows + stride * h.div_ceil(2) { nv12_to_rgba(data, stride, w, h, stride * y_rows, self.matrix) } else { Vec::new() };
                call!(buf, BUF_UNLOCK, fn());
                if !rgba.is_empty() {
                    out.push(VideoFrame { pts: t100 as f64 / 1e7, width: w as u32, height: h as u32, rgba: Arc::new(rgba) });
                }
            }
        }
    }
}

fn guid_eq(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

impl VideoDecoder for MfH264Decoder {
    fn decode(&mut self, data: &[u8], pts: f64) -> Vec<VideoFrame> {
        let mut out = Vec::new();
        let Some(api) = api() else { return out };
        let annexb = h264_to_annexb(data, &self.param_sets);
        // SAFETY: as in `new`; the buffer is locked only while copying.
        unsafe {
            let mut b: *mut c_void = std::ptr::null_mut();
            if (api.create_memory_buffer)(annexb.len() as u32, &mut b) < 0 || b.is_null() {
                return out;
            }
            let b = Com(b);
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut max: u32 = 0;
            let mut cur: u32 = 0;
            if call!(b, BUF_LOCK, fn(*mut *mut u8, *mut u32, *mut u32), &mut ptr, &mut max, &mut cur) < 0 || ptr.is_null() {
                return out;
            }
            std::ptr::copy_nonoverlapping(annexb.as_ptr(), ptr, annexb.len().min(max as usize));
            call!(b, BUF_UNLOCK, fn());
            call!(b, BUF_SET_LEN, fn(u32), annexb.len() as u32);
            let mut s: *mut c_void = std::ptr::null_mut();
            if (api.create_sample)(&mut s) < 0 || s.is_null() {
                return out;
            }
            let s = Com(s);
            call!(s, SAMPLE_ADD_BUFFER, fn(*mut c_void), b.0);
            call!(s, SAMPLE_SET_TIME, fn(i64), (pts * 1e7).round() as i64);
            for _ in 0..4 {
                let hr = call!(self.t, T_PROCESS_INPUT, fn(u32, *mut c_void, u32), 0, s.0, 0);
                if hr == MF_E_NOTACCEPTING {
                    self.drain(&mut out);
                    continue;
                }
                break;
            }
        }
        self.drain(&mut out);
        out
    }

    fn flush(&mut self) {
        // SAFETY: live transform.
        unsafe {
            call!(self.t, T_PROCESS_MESSAGE, fn(u32, usize), MFT_MESSAGE_COMMAND_FLUSH, 0);
        }
    }
}
