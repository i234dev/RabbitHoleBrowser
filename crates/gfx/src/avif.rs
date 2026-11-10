//! AVIF still images: the HEIF container is parsed by `avif-parse`, the
//! AV1 bitstream decoded by `rav1d` (a pure-Rust port of dav1d, built
//! without its assembly) and the YUV planes converted to RGBA here.

use crate::image::Image;
use rav1d::include::dav1d::data::Dav1dData;
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
use rav1d::include::dav1d::headers::{
    DAV1D_MC_BT2020_CL, DAV1D_MC_BT2020_NCL, DAV1D_MC_BT709, DAV1D_MC_IDENTITY, DAV1D_PIXEL_LAYOUT_I400, DAV1D_PIXEL_LAYOUT_I420, DAV1D_PIXEL_LAYOUT_I422,
};
use rav1d::include::dav1d::picture::Dav1dPicture;
use rav1d::src::lib::{dav1d_close, dav1d_data_create, dav1d_data_unref, dav1d_default_settings, dav1d_get_picture, dav1d_open, dav1d_picture_unref, dav1d_send_data};
use std::mem::MaybeUninit;
use std::ptr::NonNull;

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd"))]
const EAGAIN: i32 = 35;
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd")))]
const EAGAIN: i32 = 11;

/// One decoded plane set.
struct Planes {
    w: usize,
    h: usize,
    bpc: u32,
    /// 0 = 4:0:0, 1 = 4:2:0, 2 = 4:2:2, 3 = 4:4:4
    layout: u32,
    full_range: bool,
    mtrx: u32,
    y: Vec<u16>,
    u: Vec<u16>,
    v: Vec<u16>,
    cw: usize,
}

/// Decode an AVIF file to RGBA.
pub fn decode(bytes: &[u8]) -> Option<Image> {
    let data = avif_parse::read_avif(&mut std::io::Cursor::new(bytes)).ok()?;
    let color = decode_av1(&data.primary_item)?;
    let alpha = data.alpha_item.as_ref().and_then(|a| decode_av1(a));
    let (w, h) = (color.w, color.h);
    let mut rgba = vec![0u8; w * h * 4];
    to_rgb(&color, &mut rgba);
    match alpha {
        Some(a) if a.w == w && a.h == h => {
            let max = ((1u32 << a.bpc) - 1) as f32;
            for i in 0..w * h {
                let raw = a.y[i] as f32;
                let v = if a.full_range { raw / max } else { (raw - 16.0 * (max + 1.0) / 256.0) / (219.0 * (max + 1.0) / 256.0) };
                let al = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                rgba[i * 4 + 3] = al;
                if data.premultiplied_alpha && al > 0 && al < 255 {
                    for c in 0..3 {
                        rgba[i * 4 + c] = ((rgba[i * 4 + c] as u32 * 255 / al as u32).min(255)) as u8;
                    }
                }
            }
        }
        _ => {
            for i in 0..w * h {
                rgba[i * 4 + 3] = 255;
            }
        }
    }
    Some(Image::new(w as u32, h as u32, rgba))
}

fn decode_av1(obu: &[u8]) -> Option<Planes> {
    if obu.is_empty() {
        return None;
    }
    // SAFETY: the dav1d C-ABI functions are called with valid pointers to
    // locals that outlive each call; the context is closed and every
    // picture/data reference released before returning.
    unsafe {
        let mut s = MaybeUninit::<Dav1dSettings>::uninit();
        dav1d_default_settings(NonNull::new(s.as_mut_ptr())?);
        let mut s = s.assume_init();
        s.n_threads = 1;
        s.max_frame_delay = 1;
        let mut ctx: Option<Dav1dContext> = None;
        if dav1d_open(Some(NonNull::from(&mut ctx)), Some(NonNull::from(&mut s))).0 < 0 {
            return None;
        }
        let c = ctx?;
        let mut data = Dav1dData::default();
        let ptr = dav1d_data_create(Some(NonNull::from(&mut data)), obu.len());
        if ptr.is_null() {
            dav1d_close(Some(NonNull::from(&mut ctx)));
            return None;
        }
        std::ptr::copy_nonoverlapping(obu.as_ptr(), ptr, obu.len());
        let mut pic = Dav1dPicture::default();
        let mut ok = false;
        for _ in 0..256 {
            if data.sz > 0 {
                let r = dav1d_send_data(Some(c), Some(NonNull::from(&mut data))).0;
                if r < 0 && r != -EAGAIN {
                    break;
                }
            }
            let r = dav1d_get_picture(Some(c), Some(NonNull::from(&mut pic))).0;
            if r == 0 {
                ok = true;
                break;
            }
            if r != -EAGAIN {
                break;
            }
        }
        if data.sz > 0 {
            dav1d_data_unref(Some(NonNull::from(&mut data)));
        }
        let planes = if ok { extract(&pic) } else { None };
        if ok {
            dav1d_picture_unref(Some(NonNull::from(&mut pic)));
        }
        dav1d_close(Some(NonNull::from(&mut ctx)));
        planes
    }
}

/// Copy the planes of a decoded picture out of dav1d's buffers.
unsafe fn extract(pic: &Dav1dPicture) -> Option<Planes> {
    let w = pic.p.w as usize;
    let h = pic.p.h as usize;
    let bpc = pic.p.bpc as u32;
    if w == 0 || h == 0 || w * h > 16384 * 16384 {
        return None;
    }
    let layout = match pic.p.layout {
        DAV1D_PIXEL_LAYOUT_I400 => 0,
        DAV1D_PIXEL_LAYOUT_I420 => 1,
        DAV1D_PIXEL_LAYOUT_I422 => 2,
        _ => 3,
    };
    let (full_range, mtrx) = match pic.seq_hdr {
        // SAFETY: the sequence header lives as long as the picture.
        Some(sh) => unsafe {
            let sh = sh.as_ref();
            (sh.color_range != 0, sh.mtrx)
        },
        None => (false, 2),
    };
    let (cw, ch) = match layout {
        0 => (0, 0),
        1 => ((w + 1) / 2, (h + 1) / 2),
        2 => ((w + 1) / 2, h),
        _ => (w, h),
    };
    let hbd = bpc > 8;
    let read = |plane: usize, pw: usize, ph: usize, stride: isize| -> Option<Vec<u16>> {
        let base = pic.data[plane]?.as_ptr() as *const u8;
        let mut out = Vec::with_capacity(pw * ph);
        for y in 0..ph {
            // SAFETY: dav1d guarantees `ph` rows of `stride` bytes holding
            // at least `pw` samples each.
            unsafe {
                let row = base.offset(y as isize * stride);
                if hbd {
                    let row = row as *const u16;
                    for x in 0..pw {
                        out.push(row.add(x).read_unaligned());
                    }
                } else {
                    for x in 0..pw {
                        out.push(*row.add(x) as u16);
                    }
                }
            }
        }
        Some(out)
    };
    let y = read(0, w, h, pic.stride[0] as isize)?;
    let (u, v) = if layout == 0 { (Vec::new(), Vec::new()) } else { (read(1, cw, ch, pic.stride[1] as isize)?, read(2, cw, ch, pic.stride[1] as isize)?) };
    Some(Planes { w, h, bpc, layout, full_range, mtrx, y, u, v, cw })
}

fn to_rgb(p: &Planes, out: &mut [u8]) {
    let scale = (1u32 << (p.bpc - 8)) as f32;
    let max = ((1u32 << p.bpc) - 1) as f32;
    let (kr, kb) = match p.mtrx {
        DAV1D_MC_BT709 => (0.2126, 0.0722),
        DAV1D_MC_BT2020_NCL | DAV1D_MC_BT2020_CL => (0.2627, 0.0593),
        _ => (0.299, 0.114),
    };
    let kg = 1.0 - kr - kb;
    let identity = p.mtrx == DAV1D_MC_IDENTITY;
    let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    for y in 0..p.h {
        for x in 0..p.w {
            let yy = p.y[y * p.w + x] as f32;
            let (cb, cr) = if p.layout == 0 {
                (128.0 * scale, 128.0 * scale)
            } else {
                let (cx, cy) = match p.layout {
                    1 => (x / 2, y / 2),
                    2 => (x / 2, y),
                    _ => (x, y),
                };
                let i = cy * p.cw + cx;
                (p.u[i] as f32, p.v[i] as f32)
            };
            let o = (y * p.w + x) * 4;
            if identity {
                // G = Y, B = U, R = V.
                let n = |v: f32| if p.full_range { v / max } else { (v - 16.0 * scale) / (219.0 * scale) };
                out[o] = to8(n(cr));
                out[o + 1] = to8(n(yy));
                out[o + 2] = to8(n(cb));
                continue;
            }
            let (yn, cbn, crn) = if p.full_range {
                (yy / max, (cb - 128.0 * scale) / max, (cr - 128.0 * scale) / max)
            } else {
                ((yy - 16.0 * scale) / (219.0 * scale), (cb - 128.0 * scale) / (224.0 * scale), (cr - 128.0 * scale) / (224.0 * scale))
            };
            let r = yn + 2.0 * (1.0 - kr) * crn;
            let b = yn + 2.0 * (1.0 - kb) * cbn;
            let g = (yn - kr * r - kb * b) / kg;
            out[o] = to8(r);
            out[o + 1] = to8(g);
            out[o + 2] = to8(b);
        }
    }
}
