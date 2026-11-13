//! `filter` and `backdrop-filter` functions (Filter Effects 1 §5) applied
//! to premultiplied RGBA pixel buffers (components in 0..=1).

use rabbithole_style::FilterOp;

/// A premultiplied RGBA image.
pub(crate) struct Pixels {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[f32; 4]>,
}

fn color_matrix(p: &mut Pixels, m: &[f32; 9], offset: f32) {
    for c in p.px.iter_mut() {
        let a = c[3];
        if a <= 0.0 {
            continue;
        }
        let (r, g, b) = (c[0] / a, c[1] / a, c[2] / a);
        let nr = (m[0] * r + m[1] * g + m[2] * b + offset).clamp(0.0, 1.0);
        let ng = (m[3] * r + m[4] * g + m[5] * b + offset).clamp(0.0, 1.0);
        let nb = (m[6] * r + m[7] * g + m[8] * b + offset).clamp(0.0, 1.0);
        c[0] = nr * a;
        c[1] = ng * a;
        c[2] = nb * a;
    }
}

fn per_channel(p: &mut Pixels, f: impl Fn(f32) -> f32) {
    for c in p.px.iter_mut() {
        let a = c[3];
        if a <= 0.0 {
            continue;
        }
        for k in 0..3 {
            c[k] = f(c[k] / a).clamp(0.0, 1.0) * a;
        }
    }
}

/// Box blur of one channel (`radius` px) along rows or columns.
fn box_pass(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize, horizontal: bool) {
    let (n, lines) = if horizontal { (w, h) } else { (h, w) };
    let idx = |line: usize, i: usize| if horizontal { line * w + i } else { i * w + line };
    let norm = 1.0 / (2 * r + 1) as f32;
    for line in 0..lines {
        let mut acc = 0.0f32;
        // Transparent outside the buffer.
        for i in 0..=r.min(n.saturating_sub(1)) {
            acc += src[idx(line, i)];
        }
        for i in 0..n {
            dst[idx(line, i)] = acc * norm;
            let add = i + r + 1;
            if add < n {
                acc += src[idx(line, add)];
            }
            if i >= r {
                acc -= src[idx(line, i - r)];
            }
        }
    }
}

/// Gaussian blur (three box blurs) with standard deviation `sigma` px.
pub(crate) fn blur(p: &mut Pixels, sigma: f32) {
    if sigma <= 0.0 || p.w == 0 || p.h == 0 {
        return;
    }
    // Box size for three passes (SVG 1.1 feGaussianBlur).
    let d = (sigma * 3.0 * (2.0 * std::f32::consts::PI).sqrt() / 4.0 + 0.5).floor().max(1.0) as usize;
    let r = d / 2;
    let (w, h) = (p.w, p.h);
    let mut ch: Vec<f32> = vec![0.0; w * h];
    let mut tmp: Vec<f32> = vec![0.0; w * h];
    for k in 0..4 {
        for (i, c) in p.px.iter().enumerate() {
            ch[i] = c[k];
        }
        for _ in 0..3 {
            box_pass(&ch, &mut tmp, w, h, r, true);
            box_pass(&tmp, &mut ch, w, h, r, false);
        }
        for (i, c) in p.px.iter_mut().enumerate() {
            c[k] = ch[i];
        }
    }
}

/// How far (device px) a filter chain can spread content.
pub(crate) fn spread(ops: &[FilterOp]) -> f32 {
    let mut e = 0.0f32;
    for op in ops {
        match op {
            FilterOp::Blur(s) => e += s * 3.0,
            FilterOp::DropShadow { x, y, blur, .. } => e += x.abs().max(y.abs()) + blur * 1.5,
            _ => {}
        }
    }
    e
}

/// Apply a filter chain in order; `scale` maps CSS px to device px.
pub(crate) fn apply(p: &mut Pixels, ops: &[FilterOp], scale: f32) {
    for op in ops {
        match *op {
            FilterOp::Blur(s) => blur(p, s * scale),
            FilterOp::Brightness(v) => per_channel(p, |c| c * v),
            FilterOp::Contrast(v) => per_channel(p, |c| (c - 0.5) * v + 0.5),
            FilterOp::Invert(v) => per_channel(p, |c| c * (1.0 - v) + (1.0 - c) * v),
            FilterOp::Opacity(v) => {
                for c in p.px.iter_mut() {
                    for k in 0..4 {
                        c[k] *= v;
                    }
                }
            }
            FilterOp::Grayscale(a) => {
                let a = 1.0 - a;
                let m = [
                    0.2126 + 0.7874 * a, 0.7152 - 0.7152 * a, 0.0722 - 0.0722 * a,
                    0.2126 - 0.2126 * a, 0.7152 + 0.2848 * a, 0.0722 - 0.0722 * a,
                    0.2126 - 0.2126 * a, 0.7152 - 0.7152 * a, 0.0722 + 0.9278 * a,
                ];
                color_matrix(p, &m, 0.0);
            }
            FilterOp::Sepia(a) => {
                let a = 1.0 - a;
                let m = [
                    0.393 + 0.607 * a, 0.769 - 0.769 * a, 0.189 - 0.189 * a,
                    0.349 - 0.349 * a, 0.686 + 0.314 * a, 0.168 - 0.168 * a,
                    0.272 - 0.272 * a, 0.534 - 0.534 * a, 0.131 + 0.869 * a,
                ];
                color_matrix(p, &m, 0.0);
            }
            FilterOp::Saturate(s) => {
                let m = [
                    0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s,
                    0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s,
                    0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s,
                ];
                color_matrix(p, &m, 0.0);
            }
            FilterOp::HueRotate(t) => {
                let (c, s) = (t.cos(), t.sin());
                let m = [
                    0.213 + c * 0.787 - s * 0.213, 0.715 - c * 0.715 - s * 0.715, 0.072 - c * 0.072 + s * 0.928,
                    0.213 - c * 0.213 + s * 0.143, 0.715 + c * 0.285 + s * 0.140, 0.072 - c * 0.072 - s * 0.283,
                    0.213 - c * 0.213 - s * 0.787, 0.715 - c * 0.715 + s * 0.715, 0.072 + c * 0.928 + s * 0.072,
                ];
                color_matrix(p, &m, 0.0);
            }
            FilterOp::DropShadow { x, y, blur: b, color } => {
                let (w, h) = (p.w, p.h);
                let (ox, oy) = ((x * scale).round() as i64, (y * scale).round() as i64);
                let ca = color.a as f32 / 255.0;
                let (cr, cg, cb) = (color.r as f32 / 255.0, color.g as f32 / 255.0, color.b as f32 / 255.0);
                let mut sh = Pixels { w, h, px: vec![[0.0; 4]; w * h] };
                for yy in 0..h as i64 {
                    for xx in 0..w as i64 {
                        let (sx, sy) = (xx - ox, yy - oy);
                        if sx < 0 || sy < 0 || sx >= w as i64 || sy >= h as i64 {
                            continue;
                        }
                        let a = p.px[(sy as usize) * w + sx as usize][3] * ca;
                        sh.px[(yy as usize) * w + xx as usize] = [cr * a, cg * a, cb * a, a];
                    }
                }
                blur(&mut sh, b * scale / 2.0);
                for (c, s) in p.px.iter_mut().zip(sh.px.iter()) {
                    let inv = 1.0 - c[3];
                    for k in 0..4 {
                        c[k] += s[k] * inv;
                    }
                }
            }
        }
    }
}

/// Recover premultiplied RGBA from the same content rendered over black
/// and over white (0x00RRGGBB pixels).
pub(crate) fn from_black_white(black: &[u32], white: &[u32], w: usize, h: usize) -> Pixels {
    let ch = |v: u32, s: u32| ((v >> s) & 0xff) as f32 / 255.0;
    let px = black
        .iter()
        .zip(white.iter())
        .map(|(&b, &wh)| {
            let a = 1.0 - ((ch(wh, 16) - ch(b, 16)) + (ch(wh, 8) - ch(b, 8)) + (ch(wh, 0) - ch(b, 0))) / 3.0;
            let a = a.clamp(0.0, 1.0);
            [ch(b, 16).min(a), ch(b, 8).min(a), ch(b, 0).min(a), a]
        })
        .collect();
    Pixels { w, h, px }
}

/// Opaque pixels (0x00RRGGBB) as premultiplied RGBA.
pub(crate) fn from_opaque(data: &[u32], w: usize, h: usize) -> Pixels {
    let ch = |v: u32, s: u32| ((v >> s) & 0xff) as f32 / 255.0;
    Pixels { w, h, px: data.iter().map(|&v| [ch(v, 16), ch(v, 8), ch(v, 0), 1.0]).collect() }
}

/// Straight RGBA bytes of premultiplied pixels.
pub(crate) fn to_rgba(p: &Pixels) -> Vec<u8> {
    let mut out = Vec::with_capacity(p.px.len() * 4);
    for c in &p.px {
        let a = c[3].clamp(0.0, 1.0);
        let un = |v: f32| if a > 0.0 { (v / a).clamp(0.0, 1.0) } else { 0.0 };
        out.push((un(c[0]) * 255.0 + 0.5) as u8);
        out.push((un(c[1]) * 255.0 + 0.5) as u8);
        out.push((un(c[2]) * 255.0 + 0.5) as u8);
        out.push((a * 255.0 + 0.5) as u8);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invert_and_grayscale() {
        let mut p = Pixels { w: 1, h: 1, px: vec![[1.0, 0.0, 0.0, 1.0]] };
        apply(&mut p, &[FilterOp::Invert(1.0)], 1.0);
        assert!((p.px[0][0] - 0.0).abs() < 1e-4 && (p.px[0][1] - 1.0).abs() < 1e-4);
        let mut g = Pixels { w: 1, h: 1, px: vec![[0.0, 1.0, 0.0, 1.0]] };
        apply(&mut g, &[FilterOp::Grayscale(1.0)], 1.0);
        assert!((g.px[0][0] - g.px[0][2]).abs() < 1e-4);
    }

    #[test]
    fn black_white_recovers_alpha() {
        // 50% red over black and over white.
        let p = from_black_white(&[0x7f0000], &[0xff7f7f], 1, 1);
        assert!((p.px[0][3] - 0.5).abs() < 0.01);
        assert!((p.px[0][0] - 0.5).abs() < 0.01);
    }
}
