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

/// Can this build decode the picture of `codec` ("av1", "vp9", ...)?
pub fn can_decode_video(codec: &str) -> bool {
    codec == "av1"
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
