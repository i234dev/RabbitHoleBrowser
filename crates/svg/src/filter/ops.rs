//! Pixel operations of the filter primitives on premultiplied RGBA
//! buffers (components in 0..=1).

/// A premultiplied RGBA image.
#[derive(Debug, Clone, PartialEq)]
pub struct Pixels {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[f32; 4]>,
}

impl Pixels {
    pub fn new(w: usize, h: usize) -> Pixels {
        Pixels { w, h, px: vec![[0.0; 4]; w * h] }
    }
}

/// Integer pixel rectangle [x0, x1) × [y0, y1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IRect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl IRect {
    pub fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> IRect {
        IRect { x0, y0, x1: x1.max(x0), y1: y1.max(y0) }
    }
    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }
    pub fn intersect(&self, o: &IRect) -> IRect {
        IRect::new(self.x0.max(o.x0), self.y0.max(o.y0), self.x1.min(o.x1), self.y1.min(o.y1))
    }
    pub fn union(&self, o: &IRect) -> IRect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        IRect::new(self.x0.min(o.x0), self.y0.min(o.y0), self.x1.max(o.x1), self.y1.max(o.y1))
    }
    pub fn width(&self) -> i32 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> i32 {
        self.y1 - self.y0
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }
}

/// Clear everything outside `keep`.
pub fn clip_to(p: &mut Pixels, keep: IRect) {
    let w = p.w;
    for y in 0..p.h {
        let row = &mut p.px[y * w..(y + 1) * w];
        let yi = y as i32;
        if yi < keep.y0 || yi >= keep.y1 {
            row.fill([0.0; 4]);
            continue;
        }
        let a = (keep.x0.max(0) as usize).min(w);
        let b = (keep.x1.max(0) as usize).min(w);
        row[..a].fill([0.0; 4]);
        if b > a {
            row[b..].fill([0.0; 4]);
        } else {
            row[a..].fill([0.0; 4]);
        }
    }
}

fn lut_to_linear() -> &'static [f32; 4096] {
    static T: std::sync::OnceLock<[f32; 4096]> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut t = [0.0; 4096];
        for (i, v) in t.iter_mut().enumerate() {
            *v = srgb_to_linear(i as f32 / 4095.0);
        }
        t[4095] = 1.0;
        t
    })
}

fn lut_to_srgb() -> &'static [f32; 4096] {
    static T: std::sync::OnceLock<[f32; 4096]> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut t = [0.0; 4096];
        for (i, v) in t.iter_mut().enumerate() {
            *v = linear_to_srgb(i as f32 / 4095.0);
        }
        t[4095] = 1.0;
        t
    })
}

/// sRGB transfer function, inverse (exact).
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// sRGB transfer function (exact).
pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Convert the colours of `p` within `r` between sRGB and linearRGB
/// (`to_linear`), through unpremultiplied values.
pub fn convert_space(p: &mut Pixels, r: IRect, to_linear: bool) {
    let lut = if to_linear { lut_to_linear() } else { lut_to_srgb() };
    let w = p.w;
    for y in r.y0.max(0)..r.y1.min(p.h as i32) {
        for x in r.x0.max(0)..r.x1.min(w as i32) {
            let c = &mut p.px[y as usize * w + x as usize];
            let a = c[3];
            if a <= 0.0 {
                continue;
            }
            for k in 0..3 {
                let v = (c[k] / a).clamp(0.0, 1.0);
                c[k] = lut[(v * 4095.0 + 0.5) as usize] * a;
            }
        }
    }
}

/// Apply `f` to the unpremultiplied colour of every pixel in `r`.
pub fn map_unpremultiplied(p: &mut Pixels, r: IRect, f: impl Fn([f32; 4]) -> [f32; 4]) {
    let w = p.w;
    for y in r.y0.max(0)..r.y1.min(p.h as i32) {
        for x in r.x0.max(0)..r.x1.min(w as i32) {
            let c = &mut p.px[y as usize * w + x as usize];
            let a = c[3];
            let un = if a > 0.0 { [c[0] / a, c[1] / a, c[2] / a, a] } else { [0.0, 0.0, 0.0, 0.0] };
            let o = f(un);
            let na = o[3].clamp(0.0, 1.0);
            *c = [o[0].clamp(0.0, 1.0) * na, o[1].clamp(0.0, 1.0) * na, o[2].clamp(0.0, 1.0) * na, na];
        }
    }
}

/// feColorMatrix: 4×5 matrix on unpremultiplied RGBA.
pub fn color_matrix(p: &mut Pixels, r: IRect, m: &[f32; 20]) {
    map_unpremultiplied(p, r, |c| {
        let mut o = [0.0; 4];
        for (i, v) in o.iter_mut().enumerate() {
            let row = &m[i * 5..i * 5 + 5];
            *v = row[0] * c[0] + row[1] * c[1] + row[2] * c[2] + row[3] * c[3] + row[4];
        }
        o
    });
}

/// A box blur pass along rows (`horizontal`) or columns over `r`: each
/// output is the mean of the inputs from `i - left` to `i + right`
/// (transparent outside `r`).
fn box_pass(src: &Pixels, dst: &mut Pixels, r: IRect, left: usize, right: usize, horizontal: bool, line: &mut Vec<[f32; 4]>) {
    let w = src.w;
    let (n, lines, start, lstart) = if horizontal {
        (r.width().max(0) as usize, r.height().max(0) as usize, r.x0 as usize, r.y0 as usize)
    } else {
        (r.height().max(0) as usize, r.width().max(0) as usize, r.y0 as usize, r.x0 as usize)
    };
    let norm = 1.0 / (left + right + 1) as f32;
    let idx = |l: usize, i: usize| if horizontal { (lstart + l) * w + start + i } else { (start + i) * w + lstart + l };
    for l in 0..lines {
        line.clear();
        line.extend((0..n).map(|i| src.px[idx(l, i)]));
        let mut acc = [0.0f32; 4];
        // Window for output 0: inputs [-left, right].
        for v in line.iter().take(right.min(n.saturating_sub(1)) + 1) {
            for k in 0..4 {
                acc[k] += v[k];
            }
        }
        for i in 0..n {
            let o = &mut dst.px[idx(l, i)];
            for k in 0..4 {
                o[k] = acc[k] * norm;
            }
            let add = i + right + 1;
            if add < n {
                for k in 0..4 {
                    acc[k] += line[add][k];
                }
            }
            if i >= left {
                let s = line[i - left];
                for k in 0..4 {
                    acc[k] -= s[k];
                }
            }
        }
    }
}

/// Exact Gaussian along one axis (small deviations).
fn gauss_pass(src: &Pixels, dst: &mut Pixels, r: IRect, sigma: f32, horizontal: bool, line: &mut Vec<[f32; 4]>) {
    let rad = (sigma * 3.0).ceil() as i32;
    let mut kern: Vec<f32> = (-rad..=rad).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let s: f32 = kern.iter().sum();
    kern.iter_mut().for_each(|k| *k /= s);
    let w = src.w;
    let (n, lines, start, lstart) = if horizontal {
        (r.width().max(0) as usize, r.height().max(0) as usize, r.x0 as usize, r.y0 as usize)
    } else {
        (r.height().max(0) as usize, r.width().max(0) as usize, r.y0 as usize, r.x0 as usize)
    };
    let idx = |l: usize, i: usize| if horizontal { (lstart + l) * w + start + i } else { (start + i) * w + lstart + l };
    for l in 0..lines {
        line.clear();
        line.extend((0..n).map(|i| src.px[idx(l, i)]));
        for i in 0..n as i32 {
            let mut acc = [0.0f32; 4];
            let lo = (i - rad).max(0);
            let hi = (i + rad).min(n as i32 - 1);
            for j in lo..=hi {
                let kv = kern[(j - i + rad) as usize];
                let v = line[j as usize];
                for k in 0..4 {
                    acc[k] += v[k] * kv;
                }
            }
            dst.px[idx(l, i as usize)] = acc;
        }
    }
}

fn blur_axis(p: &mut Pixels, tmp: &mut Pixels, r: IRect, sigma: f32, horizontal: bool, line: &mut Vec<[f32; 4]>) {
    if sigma <= 0.0 {
        return;
    }
    if sigma < 2.0 {
        gauss_pass(p, tmp, r, sigma, horizontal, line);
        std::mem::swap(p, tmp);
        return;
    }
    // Three box blurs (Filter Effects 1 §feGaussianBlur).
    let d = (sigma * 3.0 * (2.0 * std::f32::consts::PI).sqrt() / 4.0 + 0.5).floor().max(1.0) as usize;
    if d % 2 == 1 {
        let h = d / 2;
        for _ in 0..3 {
            box_pass(p, tmp, r, h, h, horizontal, line);
            std::mem::swap(p, tmp);
        }
    } else {
        let h = d / 2;
        box_pass(p, tmp, r, h, h - 1, horizontal, line);
        std::mem::swap(p, tmp);
        box_pass(p, tmp, r, h - 1, h, horizontal, line);
        std::mem::swap(p, tmp);
        box_pass(p, tmp, r, h, h, horizontal, line);
        std::mem::swap(p, tmp);
    }
}

/// Gaussian blur of the pixels within `r` with deviations (sx, sy) px.
pub fn blur(p: &mut Pixels, r: IRect, sx: f32, sy: f32) {
    let r = r.intersect(&IRect::new(0, 0, p.w as i32, p.h as i32));
    if r.is_empty() || (sx <= 0.0 && sy <= 0.0) {
        return;
    }
    let mut tmp = Pixels::new(p.w, p.h);
    let mut line = Vec::with_capacity(p.w.max(p.h));
    blur_axis(p, &mut tmp, r, sx, true, &mut line);
    blur_axis(p, &mut tmp, r, sy, false, &mut line);
}

/// Translate by whole device pixels (dx, dy) (bilinear for fractions).
pub fn offset(src: &Pixels, dx: f32, dy: f32) -> Pixels {
    let mut out = Pixels::new(src.w, src.h);
    let (w, h) = (src.w as i32, src.h as i32);
    let fx = dx - dx.floor();
    let fy = dy - dy.floor();
    let (ix, iy) = (dx.floor() as i32, dy.floor() as i32);
    let exact = (dx - dx.round()).abs() < 1e-3 && (dy - dy.round()).abs() < 1e-3;
    let (ix, iy) = if exact { (dx.round() as i32, dy.round() as i32) } else { (ix, iy) };
    let get = |x: i32, y: i32| if x >= 0 && y >= 0 && x < w && y < h { src.px[(y * w + x) as usize] } else { [0.0; 4] };
    for y in 0..h {
        for x in 0..w {
            let o = &mut out.px[(y * w + x) as usize];
            if exact {
                *o = get(x - ix, y - iy);
                continue;
            }
            // Output (x, y) samples input at (x - dx, y - dy).
            let (sx, sy) = (x - ix - 1, y - iy - 1);
            let a = get(sx, sy);
            let b = get(sx + 1, sy);
            let c = get(sx, sy + 1);
            let d = get(sx + 1, sy + 1);
            for k in 0..4 {
                let top = a[k] * fx + b[k] * (1.0 - fx);
                let bot = c[k] * fx + d[k] * (1.0 - fx);
                o[k] = top * fy + bot * (1.0 - fy);
            }
        }
    }
    out
}

/// Separable blend functions B(cb, cs) (Compositing 1 §10).
fn blend_channel(mode: u8, cb: f32, cs: f32) -> f32 {
    match mode {
        1 => cb * cs,
        2 => cb + cs - cb * cs,
        3 => blend_channel(8, cs, cb),
        4 => cb.min(cs),
        5 => cb.max(cs),
        6 => {
            if cb == 0.0 {
                0.0
            } else if cs >= 1.0 {
                1.0
            } else {
                (cb / (1.0 - cs)).min(1.0)
            }
        }
        7 => {
            if cb >= 1.0 {
                1.0
            } else if cs <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - cb) / cs).min(1.0)
            }
        }
        8 => {
            if cs <= 0.5 {
                cb * 2.0 * cs
            } else {
                let s = 2.0 * cs - 1.0;
                cb + s - cb * s
            }
        }
        9 => {
            if cs <= 0.5 {
                cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb)
            } else {
                let d = if cb <= 0.25 { ((16.0 * cb - 12.0) * cb + 4.0) * cb } else { cb.sqrt() };
                cb + (2.0 * cs - 1.0) * (d - cb)
            }
        }
        10 => (cb - cs).abs(),
        11 => cb + cs - 2.0 * cb * cs,
        _ => cs,
    }
}

fn lum(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

fn clip_color(c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut o = c;
    if n < 0.0 {
        for v in o.iter_mut() {
            *v = l + (*v - l) * l / (l - n);
        }
    }
    if x > 1.0 {
        for v in o.iter_mut() {
            *v = l + (*v - l) * (1.0 - l) / (x - l);
        }
    }
    o
}

fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    clip_color([c[0] + d, c[1] + d, c[2] + d])
}

fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let mut idx = [0usize, 1, 2];
    idx.sort_by(|&a, &b| c[a].partial_cmp(&c[b]).unwrap_or(std::cmp::Ordering::Equal));
    let (mn, md, mx) = (idx[0], idx[1], idx[2]);
    let mut o = [0.0; 3];
    if c[mx] > c[mn] {
        o[md] = (c[md] - c[mn]) * s / (c[mx] - c[mn]);
        o[mx] = s;
    }
    o
}

/// The blended colour B(cb, cs) of unpremultiplied colours.
pub fn blend_color(mode: u8, cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    match mode {
        12 => set_lum(set_sat(cs, sat(cb)), lum(cb)),
        13 => set_lum(set_sat(cb, sat(cs)), lum(cb)),
        14 => set_lum(cs, lum(cb)),
        15 => set_lum(cb, lum(cs)),
        _ => [blend_channel(mode, cb[0], cs[0]), blend_channel(mode, cb[1], cs[1]), blend_channel(mode, cb[2], cs[2])],
    }
}

/// feBlend: `top` (in) blended over `bottom` (in2), premultiplied.
pub fn blend(top: &Pixels, bottom: &Pixels, mode: u8, r: IRect) -> Pixels {
    let mut out = Pixels::new(top.w, top.h);
    let w = top.w;
    for y in r.y0.max(0)..r.y1.min(top.h as i32) {
        for x in r.x0.max(0)..r.x1.min(w as i32) {
            let i = y as usize * w + x as usize;
            let s = top.px[i];
            let b = bottom.px[i];
            let (as_, ab) = (s[3], b[3]);
            let o = &mut out.px[i];
            if mode == 0 || as_ <= 0.0 || ab <= 0.0 {
                for k in 0..4 {
                    o[k] = s[k] + b[k] * (1.0 - as_);
                }
                continue;
            }
            let cs = [s[0] / as_, s[1] / as_, s[2] / as_];
            let cb = [b[0] / ab, b[1] / ab, b[2] / ab];
            let bl = blend_color(mode, cb, cs);
            for k in 0..3 {
                o[k] = s[k] * (1.0 - ab) + b[k] * (1.0 - as_) + as_ * ab * bl[k].clamp(0.0, 1.0);
            }
            o[3] = as_ + ab - as_ * ab;
        }
    }
    out
}

/// feComposite operators.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CompositeOp {
    Over,
    In,
    Out,
    Atop,
    Xor,
    Lighter,
    Arithmetic(f32, f32, f32, f32),
}

/// feComposite of `a` (in) with `b` (in2).
pub fn composite(a: &Pixels, b: &Pixels, op: CompositeOp, r: IRect) -> Pixels {
    let mut out = Pixels::new(a.w, a.h);
    let w = a.w;
    for y in r.y0.max(0)..r.y1.min(a.h as i32) {
        for x in r.x0.max(0)..r.x1.min(w as i32) {
            let i = y as usize * w + x as usize;
            let (s, d) = (a.px[i], b.px[i]);
            let (fa, fb) = match op {
                CompositeOp::Over => (1.0, 1.0 - s[3]),
                CompositeOp::In => (d[3], 0.0),
                CompositeOp::Out => (1.0 - d[3], 0.0),
                CompositeOp::Atop => (d[3], 1.0 - s[3]),
                CompositeOp::Xor => (1.0 - d[3], 1.0 - s[3]),
                CompositeOp::Lighter => (1.0, 1.0),
                CompositeOp::Arithmetic(k1, k2, k3, k4) => {
                    let mut o = [0.0f32; 4];
                    for k in 0..4 {
                        o[k] = (k1 * s[k] * d[k] + k2 * s[k] + k3 * d[k] + k4).clamp(0.0, 1.0);
                    }
                    for k in 0..3 {
                        o[k] = o[k].min(o[3]);
                    }
                    out.px[i] = o;
                    continue;
                }
            };
            let o = &mut out.px[i];
            for k in 0..4 {
                o[k] = (s[k] * fa + d[k] * fb).min(1.0);
            }
        }
    }
    out
}

/// feMorphology: erode (min) or dilate (max) over a (2rx+1)×(2ry+1)
/// rectangle (separable), within `r`.
pub fn morphology(src: &Pixels, r: IRect, rx: usize, ry: usize, dilate: bool) -> Pixels {
    let w = src.w;
    let mut tmp = Pixels::new(src.w, src.h);
    let pick = |a: f32, b: f32| if dilate { a.max(b) } else { a.min(b) };
    let init = if dilate { 0.0 } else { 1.0 };
    let r = r.intersect(&IRect::new(0, 0, src.w as i32, src.h as i32));
    // Outside the input there is transparent black.
    for y in r.y0..r.y1 {
        for x in r.x0..r.x1 {
            let mut acc = [init; 4];
            for xx in x - rx as i32..=x + rx as i32 {
                let v = if xx >= 0 && xx < w as i32 { src.px[y as usize * w + xx as usize] } else { [0.0; 4] };
                for k in 0..4 {
                    acc[k] = pick(acc[k], v[k]);
                }
            }
            tmp.px[y as usize * w + x as usize] = acc;
        }
    }
    let mut out = Pixels::new(src.w, src.h);
    for y in r.y0..r.y1 {
        for x in r.x0..r.x1 {
            let mut acc = [init; 4];
            for yy in y - ry as i32..=y + ry as i32 {
                let v = if yy >= 0 && yy < src.h as i32 { tmp.px[yy as usize * w + x as usize] } else { [0.0; 4] };
                for k in 0..4 {
                    acc[k] = pick(acc[k], v[k]);
                }
            }
            out.px[y as usize * w + x as usize] = acc;
        }
    }
    out
}

/// feTile: replicate the `tile` area of `src` over `r`.
pub fn tile(src: &Pixels, tile: IRect, r: IRect) -> Pixels {
    let mut out = Pixels::new(src.w, src.h);
    let tile = tile.intersect(&IRect::new(0, 0, src.w as i32, src.h as i32));
    if tile.is_empty() {
        return out;
    }
    let (tw, th) = (tile.width(), tile.height());
    let w = src.w;
    for y in r.y0.max(0)..r.y1.min(src.h as i32) {
        let sy = tile.y0 + (y - tile.y0).rem_euclid(th);
        for x in r.x0.max(0)..r.x1.min(w as i32) {
            let sx = tile.x0 + (x - tile.x0).rem_euclid(tw);
            out.px[y as usize * w + x as usize] = src.px[sy as usize * w + sx as usize];
        }
    }
    out
}

/// Edge handling of feConvolveMatrix / feGaussianBlur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeMode {
    Duplicate,
    Wrap,
    None,
}

/// feConvolveMatrix over `r`; `edges` is the input image's extent.
#[allow(clippy::too_many_arguments)]
pub fn convolve(src: &Pixels, r: IRect, edges: IRect, order: (usize, usize), kernel: &[f32], divisor: f32, bias: f32, target: (usize, usize), edge: EdgeMode, preserve_alpha: bool) -> Pixels {
    let mut out = Pixels::new(src.w, src.h);
    let w = src.w;
    let edges = edges.intersect(&IRect::new(0, 0, src.w as i32, src.h as i32));
    if edges.is_empty() {
        return out;
    }
    let (ox, oy) = (order.0 as i32, order.1 as i32);
    let (tx, ty) = (target.0 as i32, target.1 as i32);
    let get = |x: i32, y: i32| -> [f32; 4] {
        let (x, y) = match edge {
            EdgeMode::Duplicate => (x.clamp(edges.x0, edges.x1 - 1), y.clamp(edges.y0, edges.y1 - 1)),
            EdgeMode::Wrap => (edges.x0 + (x - edges.x0).rem_euclid(edges.width()), edges.y0 + (y - edges.y0).rem_euclid(edges.height())),
            EdgeMode::None => {
                if !edges.contains(x, y) {
                    return [0.0; 4];
                }
                (x, y)
            }
        };
        src.px[y as usize * w + x as usize]
    };
    for y in r.y0.max(0)..r.y1.min(src.h as i32) {
        for x in r.x0.max(0)..r.x1.min(w as i32) {
            let mut acc = [0.0f32; 4];
            for j in 0..oy {
                for i in 0..ox {
                    let mut v = get(x - tx + i, y - ty + j);
                    if preserve_alpha && v[3] > 0.0 {
                        let a = v[3];
                        v = [v[0] / a, v[1] / a, v[2] / a, a];
                    }
                    let kv = kernel[((oy - 1 - j) * ox + (ox - 1 - i)) as usize];
                    for k in 0..4 {
                        acc[k] += v[k] * kv;
                    }
                }
            }
            let i = y as usize * w + x as usize;
            let o = &mut out.px[i];
            if preserve_alpha {
                let a = src.px[i][3];
                for k in 0..3 {
                    o[k] = ((acc[k] / divisor + bias).clamp(0.0, 1.0)) * a;
                }
                o[3] = a;
            } else {
                let a = (acc[3] / divisor + bias).clamp(0.0, 1.0);
                for k in 0..3 {
                    o[k] = (acc[k] / divisor + bias).clamp(0.0, a);
                }
                o[3] = a;
            }
        }
    }
    out
}

/// feDisplacementMap: `map` gives the displacement (unpremultiplied
/// channels `xc` / `yc`, 0 = R … 3 = A) of `src`; `scale` in px.
pub fn displace(src: &Pixels, map: &Pixels, r: IRect, sx: f32, sy: f32, xc: usize, yc: usize) -> Pixels {
    let mut out = Pixels::new(src.w, src.h);
    let w = src.w as i32;
    let h = src.h as i32;
    for y in r.y0.max(0)..r.y1.min(h) {
        for x in r.x0.max(0)..r.x1.min(w) {
            let i = (y * w + x) as usize;
            let m = map.px[i];
            let a = m[3];
            let ch = |c: usize| if c == 3 { a } else if a > 0.0 { m[c] / a } else { 0.0 };
            let px = x as f32 + sx * (ch(xc) - 0.5);
            let py = y as f32 + sy * (ch(yc) - 0.5);
            let (ix, iy) = ((px + 0.5).floor() as i32, (py + 0.5).floor() as i32);
            if ix >= 0 && iy >= 0 && ix < w && iy < h {
                out.px[i] = src.px[(iy * w + ix) as usize];
            }
        }
    }
    out
}

/// Source-over of `top` onto `acc`.
pub fn over(acc: &mut Pixels, top: &Pixels, r: IRect) {
    let w = acc.w;
    for y in r.y0.max(0)..r.y1.min(acc.h as i32) {
        for x in r.x0.max(0)..r.x1.min(w as i32) {
            let i = y as usize * w + x as usize;
            let s = top.px[i];
            let d = &mut acc.px[i];
            let inv = 1.0 - s[3];
            for k in 0..4 {
                d[k] = s[k] + d[k] * inv;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, c: [f32; 4]) -> Pixels {
        Pixels { w, h, px: vec![c; w * h] }
    }

    #[test]
    fn colour_space_round_trip() {
        for v in [0.0f32, 0.02, 0.2, 0.5, 0.8, 1.0] {
            assert!((linear_to_srgb(srgb_to_linear(v)) - v).abs() < 1e-4);
        }
        // 50% grey is about 21.4% in linear light.
        assert!((srgb_to_linear(0.5) - 0.214).abs() < 0.001);
        let mut p = solid(1, 1, [0.25, 0.25, 0.25, 0.5]);
        let all = IRect::new(0, 0, 1, 1);
        convert_space(&mut p, all, true);
        assert!((p.px[0][0] / 0.5 - 0.214).abs() < 0.002);
        convert_space(&mut p, all, false);
        assert!((p.px[0][0] - 0.25).abs() < 0.002 && p.px[0][3] == 0.5);
    }

    #[test]
    fn blur_preserves_mass_and_spreads() {
        let mut p = Pixels::new(41, 41);
        p.px[20 * 41 + 20] = [1.0; 4];
        blur(&mut p, IRect::new(0, 0, 41, 41), 3.0, 3.0);
        let total: f32 = p.px.iter().map(|c| c[3]).sum();
        assert!((total - 1.0).abs() < 0.01, "{total}");
        assert!(p.px[20 * 41 + 23][3] > 0.0 && p.px[20 * 41 + 20][3] < 0.1);
        // Symmetric.
        assert!((p.px[20 * 41 + 17][3] - p.px[20 * 41 + 23][3]).abs() < 0.01);
        // One axis only.
        let mut q = Pixels::new(21, 21);
        q.px[10 * 21 + 10] = [1.0; 4];
        blur(&mut q, IRect::new(0, 0, 21, 21), 2.5, 0.0);
        assert_eq!(q.px[9 * 21 + 10][3], 0.0);
        assert!(q.px[10 * 21 + 12][3] > 0.0);
    }

    #[test]
    fn offset_moves_pixels() {
        let mut p = Pixels::new(4, 4);
        p.px[0] = [1.0; 4];
        let o = offset(&p, 2.0, 1.0);
        assert_eq!(o.px[4 + 2], [1.0; 4]);
        assert_eq!(o.px[0], [0.0; 4]);
    }

    #[test]
    fn composite_operators() {
        let a = solid(1, 1, [1.0, 0.0, 0.0, 1.0]);
        let b = solid(1, 1, [0.0, 0.0, 0.5, 0.5]);
        let r = IRect::new(0, 0, 1, 1);
        assert_eq!(composite(&a, &b, CompositeOp::In, r).px[0], [0.5, 0.0, 0.0, 0.5]);
        assert_eq!(composite(&a, &b, CompositeOp::Out, r).px[0], [0.5, 0.0, 0.0, 0.5]);
        assert_eq!(composite(&b, &a, CompositeOp::Over, r).px[0], [0.5, 0.0, 0.5, 1.0]);
        assert_eq!(composite(&a, &b, CompositeOp::Xor, r).px[0], [0.5, 0.0, 0.0, 0.5]);
        let ar = composite(&a, &b, CompositeOp::Arithmetic(0.0, 0.5, 0.5, 0.0), r).px[0];
        assert_eq!(ar, [0.5, 0.0, 0.25, 0.75]);
    }

    #[test]
    fn blend_modes() {
        let r = IRect::new(0, 0, 1, 1);
        let top = solid(1, 1, [0.5, 0.5, 0.5, 1.0]);
        let bottom = solid(1, 1, [1.0, 0.5, 0.0, 1.0]);
        let m = blend(&top, &bottom, 1, r).px[0];
        assert!((m[0] - 0.5).abs() < 1e-5 && (m[1] - 0.25).abs() < 1e-5 && m[2] == 0.0);
        let s = blend(&top, &bottom, 2, r).px[0];
        assert!((s[0] - 1.0).abs() < 1e-5 && (s[1] - 0.75).abs() < 1e-5);
        let d = blend(&top, &bottom, 10, r).px[0];
        assert!((d[0] - 0.5).abs() < 1e-5 && d[1].abs() < 1e-5);
        let l = blend(&top, &bottom, 15, r).px[0];
        assert!((lum([l[0], l[1], l[2]]) - 0.5).abs() < 1e-3);
    }

    #[test]
    fn morphology_and_tile() {
        let mut p = Pixels::new(5, 5);
        p.px[12] = [1.0; 4];
        let all = IRect::new(0, 0, 5, 5);
        let d = morphology(&p, all, 1, 1, true);
        assert_eq!(d.px.iter().filter(|c| c[3] > 0.0).count(), 9);
        let e = morphology(&d, all, 1, 1, false);
        assert_eq!(e.px.iter().filter(|c| c[3] > 0.0).count(), 1);
        let t = tile(&p, IRect::new(2, 2, 3, 3), all);
        assert!(t.px.iter().all(|c| c[3] == 1.0));
    }

    #[test]
    fn convolution_identity_and_edges() {
        let mut p = Pixels::new(3, 3);
        p.px[4] = [0.5, 0.5, 0.5, 0.5];
        let all = IRect::new(0, 0, 3, 3);
        let k = [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
        let o = convolve(&p, all, all, (3, 3), &k, 1.0, 0.0, (1, 1), EdgeMode::Duplicate, false);
        assert_eq!(o.px, p.px);
        // Shift right: kernel is rotated 180°.
        let k = [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
        let o = convolve(&p, all, all, (3, 3), &k, 1.0, 0.0, (1, 1), EdgeMode::None, false);
        assert_eq!(o.px[5][3], 0.5);
    }

    #[test]
    fn displacement_reads_map_channels() {
        let mut src = Pixels::new(4, 1);
        src.px[3] = [1.0; 4];
        // Red 1.0 with scale 2 moves the sample one pixel right.
        let map = solid(4, 1, [1.0, 0.0, 0.0, 1.0]);
        let o = displace(&src, &map, IRect::new(0, 0, 4, 1), 2.0, 0.0, 0, 1);
        assert_eq!(o.px[2], [1.0; 4]);
        assert_eq!(o.px[3], [0.0; 4]);
    }
}
