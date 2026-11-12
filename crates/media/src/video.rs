//! Video decoding: AV1 through `rav1d` (the pure-Rust dav1d port the
//! image decoder already uses for AVIF), YUV → RGBA conversion, and the
//! decoded frame type the element paints.

use rav1d::include::dav1d::data::Dav1dData;
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
use rav1d::include::dav1d::headers::{DAV1D_MC_BT2020_CL, DAV1D_MC_BT2020_NCL, DAV1D_MC_BT709, DAV1D_PIXEL_LAYOUT_I400, DAV1D_PIXEL_LAYOUT_I420, DAV1D_PIXEL_LAYOUT_I422};
use rav1d::include::dav1d::picture::Dav1dPicture;
use rav1d::src::lib::{dav1d_close, dav1d_data_create, dav1d_data_unref, dav1d_default_settings, dav1d_flush, dav1d_get_picture, dav1d_open, dav1d_picture_unref, dav1d_send_data};
use std::mem::MaybeUninit;
use std::ptr::NonNull;
use std::sync::Arc;

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd"))]
const EAGAIN: i32 = 35;
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd")))]
const EAGAIN: i32 = 11;

/// A decoded picture, straight RGBA.
#[derive(Clone)]
pub struct VideoFrame {
    pub pts: f64,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

impl std::fmt::Debug for VideoFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VideoFrame({}x{} @ {:.3})", self.width, self.height, self.pts)
    }
}

/// Can this build decode the picture of `codec` ("av1", "vp9", "h264")?
/// AV1 (rav1d) and VP9 (rusty_vp9) everywhere; H.264 through the
/// operating system's decoder (Media Foundation on Windows).
pub fn can_decode_video(codec: &str) -> bool {
    match codec {
        "av1" | "vp9" => true,
        "h264" => h264_available(),
        _ => false,
    }
}

#[cfg(windows)]
fn h264_available() -> bool {
    crate::mf::h264_available()
}

#[cfg(not(windows))]
fn h264_available() -> bool {
    false
}

/// A streaming video decoder: one coded frame (access unit) in, the
/// pictures that came out of it (display order) back.
pub trait VideoDecoder: Send {
    fn decode(&mut self, data: &[u8], pts: f64) -> Vec<VideoFrame>;
    /// Drop decoder state (seek).
    fn flush(&mut self);
}

impl VideoDecoder for Av1Decoder {
    fn decode(&mut self, data: &[u8], pts: f64) -> Vec<VideoFrame> {
        Av1Decoder::decode(self, data, pts)
    }
    fn flush(&mut self) {
        Av1Decoder::flush(self)
    }
}

/// A decoder for `codec`; `config` is the container's codec configuration
/// (avcC for H.264; unused by AV1 / VP9, whose headers are in-band).
pub fn new_decoder(codec: &str, config: &[u8]) -> Option<Box<dyn VideoDecoder>> {
    match codec {
        "av1" => Av1Decoder::new().map(|d| Box::new(d) as Box<dyn VideoDecoder>),
        "vp9" => Some(Box::new(Vp9Decoder { dec: rusty_vp9::Vp9Decoder::new() })),
        #[cfg(windows)]
        "h264" => crate::mf::MfH264Decoder::new(config).map(|d| Box::new(d) as Box<dyn VideoDecoder>),
        _ => {
            let _ = config;
            None
        }
    }
}

/// Is this coded frame a random access point?
pub fn is_key(codec: &str, data: &[u8]) -> bool {
    match codec {
        "av1" => av1_is_key(data),
        "vp9" => vp9_is_key(data),
        "h264" => h264_is_key(data),
        _ => true,
    }
}

/// Does this AV1 temporal unit hold a key frame (a sequence header or a
/// KEY_FRAME frame header)?
pub fn av1_is_key(data: &[u8]) -> bool {
    let mut p = 0usize;
    while p < data.len() {
        let h = data[p];
        let ty = (h >> 3) & 0x0f;
        if ty == 1 {
            return true;
        }
        let ext = h & 0x04 != 0;
        let has_size = h & 0x02 != 0;
        p += 1 + ext as usize;
        if !has_size {
            break;
        }
        let mut size = 0usize;
        let mut shift = 0;
        while p < data.len() {
            let b = data[p];
            p += 1;
            size |= ((b & 0x7f) as usize) << shift;
            shift += 7;
            if b & 0x80 == 0 || shift > 56 {
                break;
            }
        }
        if ty == 6 || ty == 3 {
            if let Some(&b) = data.get(p) {
                return b & 0x80 == 0 && (b >> 5) & 0x03 == 0;
            }
        }
        p += size;
    }
    false
}

/// VP9 uncompressed header: frame_marker, profile, show_existing_frame,
/// frame_type (0 = KEY_FRAME).
pub fn vp9_is_key(data: &[u8]) -> bool {
    let Some(&b) = data.first() else { return false };
    if b >> 6 != 2 {
        return false;
    }
    let profile = ((b >> 5) & 1) | (((b >> 4) & 1) << 1);
    let mut bit = 3; // next bit position (from the MSB side: 7 - consumed)
    if profile == 3 {
        bit -= 1; // reserved zero
    }
    let show_existing = (b >> bit) & 1;
    if show_existing == 1 {
        return false;
    }
    (b >> (bit - 1)) & 1 == 0
}

/// H.264 access unit (length-prefixed AVCC or Annex B) with an IDR slice.
pub fn h264_is_key(data: &[u8]) -> bool {
    h264_nals(data).any(|n| n.first().is_some_and(|h| h & 0x1f == 5))
}

/// NAL units of an access unit: Annex B (start codes) or 4-byte
/// length-prefixed (MP4 / Matroska).
pub fn h264_nals(data: &[u8]) -> Box<dyn Iterator<Item = &[u8]> + '_> {
    let annexb = data.starts_with(&[0, 0, 1]) || data.starts_with(&[0, 0, 0, 1]);
    if annexb {
        let mut starts = Vec::new();
        let mut i = 0;
        while i + 3 <= data.len() {
            if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
                starts.push(i + 3);
                i += 3;
            } else {
                i += 1;
            }
        }
        let mut out: Vec<&[u8]> = Vec::new();
        for (k, &s) in starts.iter().enumerate() {
            let mut e = starts.get(k + 1).map(|&n| n - 3).unwrap_or(data.len());
            while e > s && data[e - 1] == 0 {
                e -= 1;
            }
            out.push(&data[s..e]);
        }
        Box::new(out.into_iter())
    } else {
        let mut out: Vec<&[u8]> = Vec::new();
        let mut p = 0;
        while p + 4 <= data.len() {
            let n = u32::from_be_bytes([data[p], data[p + 1], data[p + 2], data[p + 3]]) as usize;
            p += 4;
            if n == 0 || p + n > data.len() {
                break;
            }
            out.push(&data[p..p + n]);
            p += n;
        }
        Box::new(out.into_iter())
    }
}

/// SPS / PPS NAL units of an avcC record.
pub fn avcc_parameter_sets(avcc: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if avcc.len() < 7 || avcc[0] != 1 {
        return out;
    }
    let mut p = 5;
    let nsps = (avcc[p] & 0x1f) as usize;
    p += 1;
    for _ in 0..nsps {
        if p + 2 > avcc.len() {
            return out;
        }
        let n = u16::from_be_bytes([avcc[p], avcc[p + 1]]) as usize;
        p += 2;
        if p + n > avcc.len() {
            return out;
        }
        out.push(avcc[p..p + n].to_vec());
        p += n;
    }
    if p >= avcc.len() {
        return out;
    }
    let npps = avcc[p] as usize;
    p += 1;
    for _ in 0..npps {
        if p + 2 > avcc.len() {
            return out;
        }
        let n = u16::from_be_bytes([avcc[p], avcc[p + 1]]) as usize;
        p += 2;
        if p + n > avcc.len() {
            return out;
        }
        out.push(avcc[p..p + n].to_vec());
        p += n;
    }
    out
}

/// An access unit as an Annex B byte stream, with the avcC parameter sets
/// in front of key frames (what OS decoders expect).
pub fn h264_to_annexb(data: &[u8], param_sets: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 64);
    if h264_is_key(data) {
        for ps in param_sets {
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(ps);
        }
    }
    for n in h264_nals(data) {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(n);
    }
    out
}

// ------------------------------------------------------------------- VP9

struct Vp9Decoder {
    dec: rusty_vp9::Vp9Decoder,
}

impl VideoDecoder for Vp9Decoder {
    fn decode(&mut self, data: &[u8], pts: f64) -> Vec<VideoFrame> {
        let mut out = Vec::new();
        if self.dec.push(data, Some((pts * 1e6).round() as i64)).is_err() {
            return out;
        }
        while let Ok(f) = self.dec.next_frame() {
            let p = f.pts.map(|t| t as f64 / 1e6).unwrap_or(pts);
            if f.planes.len() < 3 {
                continue;
            }
            let shift = f.bit_depth.saturating_sub(8);
            let rgba = yuv_to_rgba(
                &Planes { y: &f.planes[0], u: &f.planes[1], v: &f.planes[2], strides: [f.strides[0], f.strides[1], f.strides[2]], hbd: f.bit_depth > 8, shift },
                f.width as usize,
                f.height as usize,
                f.subsampling_x as usize,
                f.subsampling_y as usize,
                if f.height >= 720 { Matrix::Bt709 } else { Matrix::Bt601 },
                false,
            );
            out.push(VideoFrame { pts: p, width: f.width, height: f.height, rgba: Arc::new(rgba) });
        }
        out
    }
    fn flush(&mut self) {
        self.dec = rusty_vp9::Vp9Decoder::new();
    }
}

#[derive(Clone, Copy)]
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) enum Matrix {
    Bt601,
    Bt709,
    Bt2020,
}

pub(crate) struct Planes<'a> {
    pub y: &'a [u8],
    pub u: &'a [u8],
    pub v: &'a [u8],
    /// Bytes per row.
    pub strides: [usize; 3],
    /// Samples are little-endian u16.
    pub hbd: bool,
    /// Right shift to 8 bits.
    pub shift: u32,
}

/// Planar YUV → RGBA (integer arithmetic).
pub(crate) fn yuv_to_rgba(p: &Planes, w: usize, h: usize, ssx: usize, ssy: usize, m: Matrix, full: bool) -> Vec<u8> {
    let (kr, kb) = match m {
        Matrix::Bt709 => (0.2126f32, 0.0722f32),
        Matrix::Bt2020 => (0.2627, 0.0593),
        Matrix::Bt601 => (0.299, 0.114),
    };
    let kg = 1.0 - kr - kb;
    let (ys, cs, yo) = if full { (1.0f32, 1.0f32, 0i32) } else { (255.0 / 219.0, 255.0 / 224.0, 16) };
    let crr = (2.0 * (1.0 - kr) * cs * 1024.0) as i32;
    let cbb = (2.0 * (1.0 - kb) * cs * 1024.0) as i32;
    let cgr = (2.0 * (1.0 - kr) * kr / kg * cs * 1024.0) as i32;
    let cgb = (2.0 * (1.0 - kb) * kb / kg * cs * 1024.0) as i32;
    let yk = (ys * 1024.0) as i32;
    let get = |plane: &[u8], stride: usize, x: usize, y: usize| -> i32 {
        if p.hbd {
            let i = y * stride + x * 2;
            match plane.get(i..i + 2) {
                Some(b) => (u16::from_le_bytes([b[0], b[1]]) >> p.shift) as i32,
                None => 128,
            }
        } else {
            plane.get(y * stride + x).copied().unwrap_or(128) as i32
        }
    };
    let mut rgba = vec![255u8; w * h * 4];
    for y in 0..h {
        let cy = y >> ssy;
        for x in 0..w {
            let cx = x >> ssx;
            let yy = (get(p.y, p.strides[0], x, y) - yo) * yk;
            let u = get(p.u, p.strides[1], cx, cy) - 128;
            let v = get(p.v, p.strides[2], cx, cy) - 128;
            let o = (y * w + x) * 4;
            rgba[o] = ((yy + crr * v) >> 10).clamp(0, 255) as u8;
            rgba[o + 1] = ((yy - cgr * v - cgb * u) >> 10).clamp(0, 255) as u8;
            rgba[o + 2] = ((yy + cbb * u) >> 10).clamp(0, 255) as u8;
        }
    }
    rgba
}

/// NV12 (Y plane + interleaved UV) → RGBA.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn nv12_to_rgba(data: &[u8], stride: usize, w: usize, h: usize, uv_offset: usize, m: Matrix) -> Vec<u8> {
    // De-interleave the chroma into U and V planes, then reuse the planar path.
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    let mut u = vec![128u8; cw * ch];
    let mut v = vec![128u8; cw * ch];
    for y in 0..ch {
        for x in 0..cw {
            let i = uv_offset + y * stride + x * 2;
            if let Some(b) = data.get(i..i + 2) {
                u[y * cw + x] = b[0];
                v[y * cw + x] = b[1];
            }
        }
    }
    yuv_to_rgba(&Planes { y: data, u: &u, v: &v, strides: [stride, cw, cw], hbd: false, shift: 0 }, w, h, 1, 1, m, false)
}

/// A streaming AV1 decoder.
pub struct Av1Decoder {
    ctx: Option<Dav1dContext>,
}

// The context is only used from the thread that owns the decoder.
unsafe impl Send for Av1Decoder {}

impl Av1Decoder {
    pub fn new() -> Option<Av1Decoder> {
        // SAFETY: dav1d C-ABI calls with pointers to live locals.
        unsafe {
            let mut s = MaybeUninit::<Dav1dSettings>::uninit();
            dav1d_default_settings(NonNull::new(s.as_mut_ptr())?);
            let mut s = s.assume_init();
            let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).min(4);
            s.n_threads = threads as i32;
            s.max_frame_delay = 1;
            let mut ctx: Option<Dav1dContext> = None;
            if dav1d_open(Some(NonNull::from(&mut ctx)), Some(NonNull::from(&mut s))).0 < 0 {
                return None;
            }
            ctx?;
            Some(Av1Decoder { ctx })
        }
    }

    /// Decode one temporal unit (a container frame); returns the pictures
    /// that came out (usually one).
    pub fn decode(&mut self, data: &[u8], pts: f64) -> Vec<VideoFrame> {
        let mut out = Vec::new();
        let Some(c) = self.ctx else { return out };
        if data.is_empty() {
            return out;
        }
        // SAFETY: as in `new`; the data buffer is created by dav1d and
        // every picture is unreferenced after conversion.
        unsafe {
            let mut d = Dav1dData::default();
            let ptr = dav1d_data_create(Some(NonNull::from(&mut d)), data.len());
            if ptr.is_null() {
                return out;
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
            d.m.timestamp = (pts * 1e6).round() as i64;
            for _ in 0..64 {
                if d.sz > 0 {
                    let r = dav1d_send_data(Some(c), Some(NonNull::from(&mut d))).0;
                    if r < 0 && r != -EAGAIN {
                        break;
                    }
                }
                let mut pic = Dav1dPicture::default();
                let r = dav1d_get_picture(Some(c), Some(NonNull::from(&mut pic))).0;
                if r == 0 {
                    if let Some(f) = convert(&pic) {
                        out.push(f);
                    }
                    dav1d_picture_unref(Some(NonNull::from(&mut pic)));
                    continue;
                }
                if r != -EAGAIN || d.sz == 0 {
                    break;
                }
            }
            if d.sz > 0 {
                dav1d_data_unref(Some(NonNull::from(&mut d)));
            }
        }
        out
    }

    /// Drop decoder state (seek).
    pub fn flush(&mut self) {
        if let Some(c) = self.ctx {
            // SAFETY: live context.
            unsafe { dav1d_flush(c) };
        }
    }
}

impl Drop for Av1Decoder {
    fn drop(&mut self) {
        if self.ctx.is_some() {
            // SAFETY: closed once.
            unsafe { dav1d_close(Some(NonNull::from(&mut self.ctx))) };
        }
    }
}

/// YUV picture → RGBA (integer BT.601 / BT.709 / BT.2020, limited or full
/// range; high bit depth is reduced to 8 bits).
unsafe fn convert(pic: &Dav1dPicture) -> Option<VideoFrame> {
    let w = pic.p.w as usize;
    let h = pic.p.h as usize;
    if w == 0 || h == 0 || w * h > 8192 * 8192 {
        return None;
    }
    let hbd = pic.p.bpc > 8;
    let shift = pic.p.bpc.saturating_sub(8) as u32;
    let layout = pic.p.layout;
    let (full, mtrx) = match pic.seq_hdr {
        // SAFETY: the sequence header lives as long as the picture.
        Some(sh) => unsafe {
            let sh = sh.as_ref();
            (sh.color_range != 0, sh.mtrx)
        },
        None => (false, DAV1D_MC_BT709),
    };
    // Fixed-point coefficients (×1024).
    let (kr, kb) = match mtrx {
        DAV1D_MC_BT709 => (0.2126f32, 0.0722f32),
        DAV1D_MC_BT2020_NCL | DAV1D_MC_BT2020_CL => (0.2627, 0.0593),
        _ => (0.299, 0.114),
    };
    let kg = 1.0 - kr - kb;
    let (ys, cs, yo) = if full { (1.0f32, 1.0f32, 0i32) } else { (255.0 / 219.0, 255.0 / 224.0, 16) };
    let crr = (2.0 * (1.0 - kr) * cs * 1024.0) as i32;
    let cbb = (2.0 * (1.0 - kb) * cs * 1024.0) as i32;
    let cgr = (2.0 * (1.0 - kr) * kr / kg * cs * 1024.0) as i32;
    let cgb = (2.0 * (1.0 - kb) * kb / kg * cs * 1024.0) as i32;
    let yk = (ys * 1024.0) as i32;
    let py = pic.data[0]?.as_ptr() as *const u8;
    let (pu, pv) = if layout == DAV1D_PIXEL_LAYOUT_I400 { (py, py) } else { (pic.data[1]?.as_ptr() as *const u8, pic.data[2]?.as_ptr() as *const u8) };
    let ystride = pic.stride[0] as isize;
    let cstride = pic.stride[1] as isize;
    let (sx, sy) = match layout {
        DAV1D_PIXEL_LAYOUT_I420 => (1, 1),
        DAV1D_PIXEL_LAYOUT_I422 => (1, 0),
        _ => (0, 0),
    };
    let mut rgba = vec![255u8; w * h * 4];
    let sample = |base: *const u8, stride: isize, x: usize, y: usize| -> i32 {
        // SAFETY: (x, y) is inside the plane dav1d allocated.
        unsafe {
            let row = base.offset(y as isize * stride);
            if hbd {
                ((row as *const u16).add(x).read_unaligned() >> shift) as i32
            } else {
                *row.add(x) as i32
            }
        }
    };
    for y in 0..h {
        let cy = y >> sy;
        let o_row = y * w * 4;
        for x in 0..w {
            let yy = (sample(py, ystride, x, y) - yo) * yk;
            let (u, v) = if layout == DAV1D_PIXEL_LAYOUT_I400 {
                (0, 0)
            } else {
                let cx = x >> sx;
                (sample(pu, cstride, cx, cy) - 128, sample(pv, cstride, cx, cy) - 128)
            };
            let r = (yy + crr * v) >> 10;
            let g = (yy - cgr * v - cgb * u) >> 10;
            let b = (yy + cbb * u) >> 10;
            let o = o_row + x * 4;
            rgba[o] = r.clamp(0, 255) as u8;
            rgba[o + 1] = g.clamp(0, 255) as u8;
            rgba[o + 2] = b.clamp(0, 255) as u8;
        }
    }
    Some(VideoFrame { pts: pic.m.timestamp as f64 / 1e6, width: w as u32, height: h as u32, rgba: Arc::new(rgba) })
}
