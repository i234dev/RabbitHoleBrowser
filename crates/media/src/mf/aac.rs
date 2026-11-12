//! AAC (incl. HE-AAC v1/v2, which Symphonia does not decode) through the
//! Media Foundation AAC decoder MFT: raw AAC frames in, PCM out.

use super::*;

const CLSID_CMSAAC_DEC_MFT: GUID = GUID::from_u128(0x32d186a7_218f_4c75_8876_dd77273a8999);
const MF_MEDIA_TYPE_AUDIO: GUID = GUID::from_u128(0x73647561_0000_0010_8000_00aa00389b71);
const MF_AUDIO_FORMAT_AAC: GUID = GUID::from_u128(0x00001610_0000_0010_8000_00aa00389b71);
const MF_AUDIO_FORMAT_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
const MF_AUDIO_FORMAT_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
const MF_MT_AUDIO_SAMPLES_PER_SECOND: GUID = GUID::from_u128(0x5faeeae7_0290_4c31_9e8a_c534f68d9dba);
const MF_MT_AUDIO_NUM_CHANNELS: GUID = GUID::from_u128(0x37e48bf5_645e_4c5b_89de_ada9e29b696a);
const MF_MT_AUDIO_BITS_PER_SAMPLE: GUID = GUID::from_u128(0xf2deb57f_40fa_4764_aa33_ed4f2d1ff669);
const MF_MT_AAC_PAYLOAD_TYPE: GUID = GUID::from_u128(0xbfbabe79_7434_4d1c_94f0_72a3b9e17188);
const MF_MT_USER_DATA: GUID = GUID::from_u128(0xb6bc765f_4c3b_40a4_bd51_2535b66fe09d);
const ATTR_SET_UINT32: usize = 21;
const ATTR_SET_BLOB: usize = 26;

pub struct MfAacDecoder {
    t: Com,
    rate: u32,
    channels: usize,
    float: bool,
    out_info: OutputStreamInfo,
}

impl MfAacDecoder {
    /// `asc`: the AudioSpecificConfig; `rate` / `channels` from the container.
    pub fn new(asc: &[u8], rate: u32, channels: u32) -> Option<MfAacDecoder> {
        com_init();
        let api = api()?;
        let mut p: *mut c_void = std::ptr::null_mut();
        // SAFETY: COM calls on live interfaces with valid out pointers.
        unsafe {
            let hr = CoCreateInstance(&CLSID_CMSAAC_DEC_MFT, std::ptr::null_mut(), CLSCTX_INPROC_SERVER, &IID_IMF_TRANSFORM, &mut p);
            if hr < 0 || p.is_null() {
                return None;
            }
            let t = Com(p);
            let mut mt: *mut c_void = std::ptr::null_mut();
            if (api.create_media_type)(&mut mt) < 0 || mt.is_null() {
                return None;
            }
            let mt = Com(mt);
            call!(mt, ATTR_SET_GUID, fn(*const GUID, *const GUID), &MF_MT_MAJOR_TYPE, &MF_MEDIA_TYPE_AUDIO);
            call!(mt, ATTR_SET_GUID, fn(*const GUID, *const GUID), &MF_MT_SUBTYPE, &MF_AUDIO_FORMAT_AAC);
            call!(mt, ATTR_SET_UINT32, fn(*const GUID, u32), &MF_MT_AUDIO_SAMPLES_PER_SECOND, rate.max(8000));
            call!(mt, ATTR_SET_UINT32, fn(*const GUID, u32), &MF_MT_AUDIO_NUM_CHANNELS, channels.max(1));
            call!(mt, ATTR_SET_UINT32, fn(*const GUID, u32), &MF_MT_AAC_PAYLOAD_TYPE, 0);
            // HEAACWAVEINFO after WAVEFORMATEX: payload type 0 (raw),
            // profile/level 0xFE (unspecified), then the ASC.
            let mut user = vec![0u8; 12];
            user[2..4].copy_from_slice(&0xFEu16.to_le_bytes());
            user.extend_from_slice(asc);
            call!(mt, ATTR_SET_BLOB, fn(*const GUID, *const u8, u32), &MF_MT_USER_DATA, user.as_ptr(), user.len() as u32);
            if call!(t, T_SET_INPUT_TYPE, fn(u32, *mut c_void, u32), 0, mt.0, 0) < 0 {
                return None;
            }
            let mut d = MfAacDecoder { t, rate, channels: channels.max(1) as usize, float: false, out_info: OutputStreamInfo::default() };
            if !d.negotiate_output() {
                return None;
            }
            call!(d.t, T_PROCESS_MESSAGE, fn(u32, usize), MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0);
            call!(d.t, T_PROCESS_MESSAGE, fn(u32, usize), MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0);
            Some(d)
        }
    }

    fn negotiate_output(&mut self) -> bool {
        // SAFETY: as in `new`.
        unsafe {
            let mut fallback: Option<Com> = None;
            for i in 0..64u32 {
                let mut ty: *mut c_void = std::ptr::null_mut();
                let hr = call!(self.t, T_GET_OUTPUT_AVAILABLE_TYPE, fn(u32, u32, *mut *mut c_void), 0, i, &mut ty);
                if hr < 0 || ty.is_null() {
                    break;
                }
                let ty = Com(ty);
                let mut sub = GUID::from_u128(0);
                call!(ty, ATTR_GET_GUID, fn(*const GUID, *mut GUID), &MF_MT_SUBTYPE, &mut sub);
                if guid_eq(&sub, &MF_AUDIO_FORMAT_FLOAT) {
                    return self.use_output(ty, true);
                }
                if guid_eq(&sub, &MF_AUDIO_FORMAT_PCM) && fallback.is_none() {
                    let mut bits: u32 = 0;
                    call!(ty, ATTR_GET_UINT32, fn(*const GUID, *mut u32), &MF_MT_AUDIO_BITS_PER_SAMPLE, &mut bits);
                    if bits == 16 {
                        fallback = Some(ty);
                    }
                }
            }
            match fallback {
                Some(ty) => self.use_output(ty, false),
                None => false,
            }
        }
    }

    /// SAFETY: `ty` is a media type offered by this transform.
    unsafe fn use_output(&mut self, ty: Com, float: bool) -> bool {
        if call!(self.t, T_SET_OUTPUT_TYPE, fn(u32, *mut c_void, u32), 0, ty.0, 0) < 0 {
            return false;
        }
        let mut rate: u32 = 0;
        let mut ch: u32 = 0;
        call!(ty, ATTR_GET_UINT32, fn(*const GUID, *mut u32), &MF_MT_AUDIO_SAMPLES_PER_SECOND, &mut rate);
        call!(ty, ATTR_GET_UINT32, fn(*const GUID, *mut u32), &MF_MT_AUDIO_NUM_CHANNELS, &mut ch);
        if rate > 0 {
            self.rate = rate;
        }
        if ch > 0 {
            self.channels = ch as usize;
        }
        self.float = float;
        let mut info = OutputStreamInfo::default();
        call!(self.t, T_GET_OUTPUT_STREAM_INFO, fn(u32, *mut OutputStreamInfo), 0, &mut info);
        self.out_info = info;
        true
    }

    /// Decode one raw AAC frame: (rate, channels, interleaved samples).
    pub fn decode(&mut self, data: &[u8]) -> Result<(u32, usize, Vec<f32>), String> {
        let api = api().ok_or("Media Foundation unavailable")?;
        let mut samples: Vec<f32> = Vec::new();
        // SAFETY: as in `new`.
        unsafe {
            let mut b: *mut c_void = std::ptr::null_mut();
            if (api.create_memory_buffer)(data.len().max(1) as u32, &mut b) < 0 || b.is_null() {
                return Err("MFCreateMemoryBuffer failed".into());
            }
            let b = Com(b);
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut max: u32 = 0;
            let mut cur: u32 = 0;
            if call!(b, BUF_LOCK, fn(*mut *mut u8, *mut u32, *mut u32), &mut ptr, &mut max, &mut cur) < 0 || ptr.is_null() {
                return Err("buffer lock failed".into());
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len().min(max as usize));
            call!(b, BUF_UNLOCK, fn());
            call!(b, BUF_SET_LEN, fn(u32), data.len() as u32);
            let mut s: *mut c_void = std::ptr::null_mut();
            if (api.create_sample)(&mut s) < 0 || s.is_null() {
                return Err("MFCreateSample failed".into());
            }
            let s = Com(s);
            call!(s, SAMPLE_ADD_BUFFER, fn(*mut c_void), b.0);
            for _ in 0..4 {
                let hr = call!(self.t, T_PROCESS_INPUT, fn(u32, *mut c_void, u32), 0, s.0, 0);
                if hr == MF_E_NOTACCEPTING {
                    self.drain(&mut samples);
                    continue;
                }
                if hr < 0 {
                    return Err(format!("ProcessInput failed (0x{:08x})", hr as u32));
                }
                break;
            }
        }
        self.drain(&mut samples);
        Ok((self.rate, self.channels, samples))
    }

    fn drain(&mut self, out: &mut Vec<f32>) {
        let Some(api) = api() else { return };
        for _ in 0..16 {
            // SAFETY: as in `MfH264Decoder::drain`.
            unsafe {
                let provides = self.out_info.flags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES) != 0;
                let mut own: Option<Com> = None;
                if !provides {
                    let mut s: *mut c_void = std::ptr::null_mut();
                    let mut b: *mut c_void = std::ptr::null_mut();
                    let size = self.out_info.size.max(2048 * 8 * 4);
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
                let sample = match own {
                    Some(s) => s,
                    None => Com(db.sample),
                };
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
                let bytes = std::slice::from_raw_parts(ptr, cur as usize);
                if self.float {
                    out.extend(bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])));
                } else {
                    out.extend(bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0));
                }
                call!(buf, BUF_UNLOCK, fn());
            }
        }
    }

    pub fn reset(&mut self) {
        // SAFETY: live transform.
        unsafe {
            call!(self.t, T_PROCESS_MESSAGE, fn(u32, usize), MFT_MESSAGE_COMMAND_FLUSH, 0);
        }
    }
}
