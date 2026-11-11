//! Windows output: WASAPI in shared mode, event driven. The COM
//! interfaces are declared by hand (vtable layouts from mmdeviceapi.h /
//! audioclient.h); only the methods used are typed.

use super::Backend;
use std::ffi::c_void;
use windows_sys::core::{GUID, HRESULT};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED};
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

const CLSID_MM_DEVICE_ENUMERATOR: GUID = GUID::from_u128(0xbcde0395_e52f_467c_8e3d_c4579291692e);
const IID_IMM_DEVICE_ENUMERATOR: GUID = GUID::from_u128(0xa95664d2_9614_4f35_a746_de8db63617e6);
const IID_IAUDIO_CLIENT: GUID = GUID::from_u128(0x1cb9ad4c_dbfa_4c32_b178_c2f568a703b2);
const IID_IAUDIO_RENDER_CLIENT: GUID = GUID::from_u128(0xf294acfc_3146_4483_a7bf_addca7c260e2);
const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
const KSDATAFORMAT_SUBTYPE_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);

const E_RENDER: u32 = 0;
const E_CONSOLE: u32 = 0;
const AUDCLNT_SHAREMODE_SHARED: u32 = 0;
const AUDCLNT_STREAMFLAGS_EVENTCALLBACK: u32 = 0x0004_0000;
const AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM: u32 = 0x8000_0000;
const AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY: u32 = 0x0800_0000;
const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xfffe;

#[repr(C)]
struct IUnknownVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[repr(C)]
struct IMMDeviceEnumeratorVtbl {
    base: IUnknownVtbl,
    enum_audio_endpoints: usize,
    get_default_audio_endpoint: unsafe extern "system" fn(*mut c_void, u32, u32, *mut *mut c_void) -> HRESULT,
    get_device: usize,
    register_endpoint_notification_callback: usize,
    unregister_endpoint_notification_callback: usize,
}

#[repr(C)]
struct IMMDeviceVtbl {
    base: IUnknownVtbl,
    activate: unsafe extern "system" fn(*mut c_void, *const GUID, u32, *const c_void, *mut *mut c_void) -> HRESULT,
    open_property_store: usize,
    get_id: usize,
    get_state: usize,
}

#[repr(C)]
struct IAudioClientVtbl {
    base: IUnknownVtbl,
    initialize: unsafe extern "system" fn(*mut c_void, u32, u32, i64, i64, *const WaveFormatEx, *const GUID) -> HRESULT,
    get_buffer_size: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT,
    get_stream_latency: unsafe extern "system" fn(*mut c_void, *mut i64) -> HRESULT,
    get_current_padding: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT,
    is_format_supported: usize,
    get_mix_format: unsafe extern "system" fn(*mut c_void, *mut *mut WaveFormatEx) -> HRESULT,
    get_device_period: usize,
    start: unsafe extern "system" fn(*mut c_void) -> HRESULT,
    stop: unsafe extern "system" fn(*mut c_void) -> HRESULT,
    reset: usize,
    set_event_handle: unsafe extern "system" fn(*mut c_void, HANDLE) -> HRESULT,
    get_service: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
}

#[repr(C)]
struct IAudioRenderClientVtbl {
    base: IUnknownVtbl,
    get_buffer: unsafe extern "system" fn(*mut c_void, u32, *mut *mut u8) -> HRESULT,
    release_buffer: unsafe extern "system" fn(*mut c_void, u32, u32) -> HRESULT,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct WaveFormatEx {
    format_tag: u16,
    channels: u16,
    samples_per_sec: u32,
    avg_bytes_per_sec: u32,
    block_align: u16,
    bits_per_sample: u16,
    cb_size: u16,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct WaveFormatExtensible {
    format: WaveFormatEx,
    valid_bits_per_sample: u16,
    channel_mask: u32,
    sub_format: GUID,
}

/// A COM interface pointer, released on drop.
struct Com(*mut c_void);

impl Com {
    /// SAFETY: `self.0` must be a live interface whose vtable starts with `T`.
    unsafe fn vtbl<T>(&self) -> &T {
        &**(self.0 as *mut *const T)
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: every interface starts with IUnknown.
            unsafe { (self.vtbl::<IUnknownVtbl>().release)(self.0) };
        }
    }
}

fn guid_eq(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

fn check(hr: HRESULT, what: &str) -> Result<(), String> {
    if hr < 0 {
        Err(format!("{what} failed (0x{:08x})", hr as u32))
    } else {
        Ok(())
    }
}

pub(crate) struct WasapiBackend {
    client: Com,
    render: Com,
    event: HANDLE,
    buffer_frames: u32,
    rate: u32,
    channels: usize,
    float: bool,
    latency_secs: f64,
    pending: usize,
}

// COM pointers created on and used only from the mixer thread (MTA).
unsafe impl Send for WasapiBackend {}

impl WasapiBackend {
    pub(crate) fn open() -> Result<WasapiBackend, String> {
        // SAFETY: plain COM calls following the documented protocol; every
        // out pointer is checked before use and wrapped in `Com` to release.
        unsafe {
            let hr = CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED as u32);
            // S_FALSE / RPC_E_CHANGED_MODE are fine: COM is usable.
            let _ = hr;
            let mut p: *mut c_void = std::ptr::null_mut();
            check(CoCreateInstance(&CLSID_MM_DEVICE_ENUMERATOR, std::ptr::null_mut(), CLSCTX_ALL, &IID_IMM_DEVICE_ENUMERATOR, &mut p), "CoCreateInstance(MMDeviceEnumerator)")?;
            let enumerator = Com(p);
            let mut p: *mut c_void = std::ptr::null_mut();
            check((enumerator.vtbl::<IMMDeviceEnumeratorVtbl>().get_default_audio_endpoint)(enumerator.0, E_RENDER, E_CONSOLE, &mut p), "GetDefaultAudioEndpoint")?;
            let device = Com(p);
            let mut p: *mut c_void = std::ptr::null_mut();
            check((device.vtbl::<IMMDeviceVtbl>().activate)(device.0, &IID_IAUDIO_CLIENT, CLSCTX_ALL, std::ptr::null(), &mut p), "IMMDevice::Activate")?;
            let client = Com(p);
            let ac = client.vtbl::<IAudioClientVtbl>();
            let mut mix: *mut WaveFormatEx = std::ptr::null_mut();
            check((ac.get_mix_format)(client.0, &mut mix), "GetMixFormat")?;
            if mix.is_null() {
                return Err("GetMixFormat returned no format".into());
            }
            let wf = *mix;
            let rate = wf.samples_per_sec;
            let channels = (wf.channels as usize).max(1);
            let tag = wf.format_tag;
            let bits = wf.bits_per_sample;
            let float = match tag {
                WAVE_FORMAT_IEEE_FLOAT => true,
                WAVE_FORMAT_PCM => false,
                WAVE_FORMAT_EXTENSIBLE => {
                    let ext = *(mix as *const WaveFormatExtensible);
                    let sub = ext.sub_format;
                    guid_eq(&sub, &KSDATAFORMAT_SUBTYPE_IEEE_FLOAT) || (!guid_eq(&sub, &KSDATAFORMAT_SUBTYPE_PCM) && bits == 32)
                }
                _ => bits == 32,
            };
            // Ask for exactly the format we write: 32-bit float (or 16-bit
            // PCM) at the mix rate and channel count; the engine converts.
            let (fmt_bits, fmt_tag) = if float { (32u16, WAVE_FORMAT_IEEE_FLOAT) } else { (16u16, WAVE_FORMAT_PCM) };
            let block = (fmt_bits / 8) * channels as u16;
            let want = WaveFormatEx { format_tag: fmt_tag, channels: channels as u16, samples_per_sec: rate, avg_bytes_per_sec: rate * block as u32, block_align: block, bits_per_sample: fmt_bits, cb_size: 0 };
            CoTaskMemFree(mix as *const c_void);
            // 40 ms buffer, event driven.
            let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
            check((ac.initialize)(client.0, AUDCLNT_SHAREMODE_SHARED, flags, 400_000, 0, &want, std::ptr::null()), "IAudioClient::Initialize")?;
            let mut buffer_frames = 0u32;
            check((ac.get_buffer_size)(client.0, &mut buffer_frames), "GetBufferSize")?;
            let event = CreateEventW(std::ptr::null(), 0, 0, std::ptr::null());
            if event.is_null() {
                return Err("CreateEvent failed".into());
            }
            if let Err(e) = check((ac.set_event_handle)(client.0, event), "SetEventHandle") {
                CloseHandle(event);
                return Err(e);
            }
            let mut p: *mut c_void = std::ptr::null_mut();
            if let Err(e) = check((ac.get_service)(client.0, &IID_IAUDIO_RENDER_CLIENT, &mut p), "GetService(IAudioRenderClient)") {
                CloseHandle(event);
                return Err(e);
            }
            let render = Com(p);
            let mut lat: i64 = 0;
            let _ = (ac.get_stream_latency)(client.0, &mut lat);
            if let Err(e) = check((ac.start)(client.0), "IAudioClient::Start") {
                CloseHandle(event);
                return Err(e);
            }
            Ok(WasapiBackend { client, render, event, buffer_frames, rate, channels, float, latency_secs: lat as f64 / 1e7, pending: 0 })
        }
    }
}

impl Backend for WasapiBackend {
    fn rate(&self) -> u32 {
        self.rate
    }
    fn channels(&self) -> usize {
        self.channels
    }
    fn wait(&mut self) -> Result<usize, String> {
        loop {
            // SAFETY: live client; padding out pointer is valid.
            let mut padding = 0u32;
            let hr = unsafe { (self.client.vtbl::<IAudioClientVtbl>().get_current_padding)(self.client.0, &mut padding) };
            check(hr, "GetCurrentPadding")?;
            self.latency_secs = padding as f64 / self.rate as f64;
            let free = self.buffer_frames.saturating_sub(padding) as usize;
            if free >= 64.min(self.buffer_frames as usize) && free > 0 {
                self.pending = free;
                return Ok(free);
            }
            // SAFETY: valid event handle.
            unsafe { WaitForSingleObject(self.event, 200) };
        }
    }
    fn write(&mut self, data: &[f32]) -> Result<(), String> {
        let frames = (data.len() / self.channels).min(self.pending);
        if frames == 0 {
            return Ok(());
        }
        let rc = unsafe { self.render.vtbl::<IAudioRenderClientVtbl>() };
        let mut buf: *mut u8 = std::ptr::null_mut();
        // SAFETY: requesting at most the free space reported by `wait`.
        check(unsafe { (rc.get_buffer)(self.render.0, frames as u32, &mut buf) }, "GetBuffer")?;
        if buf.is_null() {
            return Err("GetBuffer returned null".into());
        }
        let n = frames * self.channels;
        // SAFETY: WASAPI gave us `frames` frames of the negotiated format.
        unsafe {
            if self.float {
                std::ptr::copy_nonoverlapping(data.as_ptr(), buf as *mut f32, n);
            } else {
                let out = std::slice::from_raw_parts_mut(buf as *mut i16, n);
                for (o, s) in out.iter_mut().zip(data) {
                    *o = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                }
            }
        }
        // SAFETY: releasing what we got.
        check(unsafe { (rc.release_buffer)(self.render.0, frames as u32, 0) }, "ReleaseBuffer")?;
        self.pending = 0;
        Ok(())
    }
    fn latency(&self) -> f64 {
        self.latency_secs
    }
    fn name(&self) -> &'static str {
        "wasapi"
    }
}

impl Drop for WasapiBackend {
    fn drop(&mut self) {
        // SAFETY: stop the live client, then close our event.
        unsafe {
            let _ = (self.client.vtbl::<IAudioClientVtbl>().stop)(self.client.0);
            CloseHandle(self.event);
        }
    }
}
