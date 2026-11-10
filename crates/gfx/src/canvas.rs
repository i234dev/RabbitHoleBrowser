//! Pixel canvas and rasterization primitives.
//!
//! Everything here is plain CPU code over a `Vec<u32>` framebuffer: alpha
//! blended rectangles with anti-aliased fractional edges, rounded corners,
//! borders, lines, glyph masks and scaled images.

use crate::font::{FontContext, FontId, GlyphBitmap};
use crate::image::Image;
use crate::paint::Shader;
use crate::path::{is_axis_aligned, is_identity, rect_path, transform_rect_aligned};
use crate::raster::Mask;
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const TRANSPARENT: Color = Color { r: 0, g: 0, b: 0, a: 0 };
    pub const BLACK: Color = Color { r: 0, g: 0, b: 0, a: 255 };
    pub const WHITE: Color = Color { r: 255, g: 255, b: 255, a: 255 };

    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color {
        Color { r, g, b, a }
    }
    pub fn is_transparent(&self) -> bool {
        self.a == 0
    }
    pub fn with_alpha(self, a: u8) -> Color {
        Color { a, ..self }
    }
    /// Multiply alpha by `f` (0..=1).
    pub fn scale_alpha(self, f: f32) -> Color {
        Color {
            a: (self.a as f32 * f.clamp(0.0, 1.0)).round() as u8,
            ..self
        }
    }
    /// 0x00RRGGBB as used by the window surface.
    pub fn to_u32(self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
    }
    /// From 0x00RRGGBB (opaque).
    pub fn from_u32(v: u32) -> Color {
        Color::rgb(((v >> 16) & 0xff) as u8, ((v >> 8) & 0xff) as u8, (v & 0xff) as u8)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && py >= self.y && px < self.right() && py < self.bottom()
    }
    pub fn intersect(&self, o: &Rect) -> Rect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        Rect { x, y, w: (r - x).max(0.0), h: (b - y).max(0.0) }
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }
    pub fn translate(&self, dx: f32, dy: f32) -> Rect {
        Rect { x: self.x + dx, y: self.y + dy, ..*self }
    }
    pub fn union(&self, o: &Rect) -> Rect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect { x, y, w: self.right().max(o.right()) - x, h: self.bottom().max(o.bottom()) - y }
    }
    /// Shrink by per-side amounts (top, right, bottom, left).
    pub fn inset(&self, t: f32, r: f32, b: f32, l: f32) -> Rect {
        Rect { x: self.x + l, y: self.y + t, w: (self.w - l - r).max(0.0), h: (self.h - t - b).max(0.0) }
    }
}


/// Blend `src` (with 8-bit `alpha`) over an opaque 0x00RRGGBB pixel.
#[inline(always)]
pub(crate) fn blend(dst: u32, src: Color, alpha: u32) -> u32 {
    if alpha >= 255 {
        return src.to_u32();
    }
    if alpha == 0 {
        return dst;
    }
    let inv = 255 - alpha;
    let dr = (dst >> 16) & 0xff;
    let dg = (dst >> 8) & 0xff;
    let db = dst & 0xff;
    let r = (src.r as u32 * alpha + dr * inv + 127) / 255;
    let g = (src.g as u32 * alpha + dg * inv + 127) / 255;
    let b = (src.b as u32 * alpha + db * inv + 127) / 255;
    (r << 16) | (g << 8) | b
}

/// Source-over for image pixels (`src` channels in 0..=255): truncating
/// like other engines' image compositing, so semi-transparent image pixels
/// match reference renderings exactly.
#[inline]
pub(crate) fn blend_image(dst: u32, src: [f32; 3], alpha: u32) -> u32 {
    let s = src.map(|c| c.round().clamp(0.0, 255.0) as u32);
    if alpha >= 255 {
        return (s[0] << 16) | (s[1] << 8) | s[2];
    }
    if alpha == 0 {
        return dst;
    }
    let inv = 255 - alpha;
    let d = [(dst >> 16) & 0xff, (dst >> 8) & 0xff, dst & 0xff];
    let c = |i: usize| (s[i] * alpha + d[i] * inv) / 255;
    (c(0) << 16) | (c(1) << 8) | c(2)
}

/// Coverage of the pixel span [p, p+1) by the interval [a, b), in 0..=1.
#[inline(always)]
pub(crate) fn span_coverage(p: f32, a: f32, b: f32) -> f32 {
    (b.min(p + 1.0) - a.max(p)).clamp(0.0, 1.0)
}

/// One clip stack entry: a device-space rectangle and, for non-rectangular
/// clips, a coverage mask covering (at least) that rectangle.
#[derive(Clone)]
pub(crate) struct ClipEntry {
    pub rect: Rect,
    pub mask: Option<Rc<Mask>>,
}

/// An offscreen layer: the saved pixels of a region, restored and blended
/// with the layer's drawing on `end_layer`.
pub(crate) struct Layer {
    pub x0: i32,
    pub y0: i32,
    pub w: usize,
    pub h: usize,
    pub saved: Vec<u32>,
}

/// An RGB framebuffer. Pixels are `0x00RRGGBB` so the buffer can be handed
/// straight to the window surface.
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u32>,
    pub(crate) clips: Vec<ClipEntry>,
    pub(crate) transforms: Vec<Matrix>,
    pub(crate) layers: Vec<Layer>,
    /// Smooth (bilinear/box) image scaling; false = nearest neighbour
    /// (`image-rendering: pixelated`, canvas `imageSmoothingEnabled`).
    pub image_smoothing: bool,
}

/// Integer pixel bounds [x0, x1) × [y0, y1).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Bounds {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Bounds {
    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }
    pub fn tuple(&self) -> (i32, i32, i32, i32) {
        (self.x0, self.y0, self.x1, self.y1)
    }
}

/// Gamma lookup applied to glyph coverage for dark text (mild boost that
/// brings thin stems closer to what GPU/Skia text looks like).
fn text_gamma_lut() -> &'static [u8; 256] {
    static LUT: std::sync::OnceLock<[u8; 256]> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        let mut t = [0u8; 256];
        for (i, v) in t.iter_mut().enumerate() {
            let c = i as f32 / 255.0;
            *v = (c.powf(1.0 / 1.43) * 255.0 + 0.5) as u8;
        }
        t
    })
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Canvas {
        Canvas {
            width,
            height,
            data: vec![0xffffff; (width * height) as usize],
            clips: Vec::new(),
            transforms: Vec::new(),
            layers: Vec::new(),
            image_smoothing: true,
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.data.clear();
        self.data.resize((width * height) as usize, 0xffffff);
        self.clips.clear();
        self.transforms.clear();
        self.layers.clear();
    }

    pub fn clear(&mut self, color: Color) {
        let v = color.to_u32();
        self.data.fill(v);
    }

    pub(crate) fn full_rect(&self) -> Rect {
        Rect::new(0.0, 0.0, self.width as f32, self.height as f32)
    }

    /// Current clip rectangle in device space (whole canvas if none pushed).
    pub fn clip(&self) -> Rect {
        match self.clips.last() {
            Some(c) => c.rect,
            None => self.full_rect(),
        }
    }

    pub(crate) fn current_mask(&self) -> Option<Rc<Mask>> {
        self.clips.last().and_then(|c| c.mask.clone())
    }

    /// Push a clip rectangle (in current user space) intersected with the
    /// current clip. Under a rotating/skewing transform the rectangle
    /// becomes a path clip.
    pub fn push_clip(&mut self, rect: Rect) {
        let m = self.transform();
        if is_identity(&m) || is_axis_aligned(&m) {
            let dr = if is_identity(&m) { rect } else { transform_rect_aligned(&m, rect) };
            let c = self.clip().intersect(&dr);
            let mask = self.current_mask();
            self.clips.push(ClipEntry { rect: c, mask });
        } else {
            self.push_clip_path(&rect_path(rect), FillRule::NonZero);
        }
    }

    pub fn pop_clip(&mut self) {
        self.clips.pop();
    }

    /// Push a clip described by a device-space coverage mask (intersected
    /// with the current clip rect and mask).
    pub(crate) fn push_mask_clip(&mut self, mask: Mask) {
        let rect = self.clip().intersect(&mask.rect());
        let combined = match self.current_mask() {
            None => mask,
            Some(prev) => {
                let mut m = mask;
                for y in 0..m.height as i32 {
                    for x in 0..m.width as i32 {
                        let i = y as usize * m.width as usize + x as usize;
                        let p = prev.get(m.x0 + x, m.y0 + y) as u32;
                        m.data[i] = ((m.data[i] as u32 * p + 127) / 255) as u8;
                    }
                }
                m
            }
        };
        self.clips.push(ClipEntry { rect, mask: Some(Rc::new(combined)) });
    }

    /// Integer pixel bounds of the device-space `rect` intersected with the
    /// clip and canvas.
    pub(crate) fn bounds(&self, rect: &Rect) -> Bounds {
        let c = self.clip().intersect(&self.full_rect());
        let r = rect.intersect(&c);
        if r.is_empty() {
            return Bounds { x0: 0, y0: 0, x1: 0, y1: 0 };
        }
        Bounds {
            x0: r.x.floor() as i32,
            y0: r.y.floor() as i32,
            x1: r.right().ceil() as i32,
            y1: r.bottom().ceil() as i32,
        }
    }

    /// Clip ∩ canvas as integer bounds.
    pub(crate) fn clip_bounds(&self) -> Bounds {
        let full = self.full_rect();
        self.bounds(&full)
    }

    /// Fill an axis-aligned rect with alpha blending and anti-aliased
    /// fractional edges.
    pub fn fill_rect(&mut self, rect: Rect, color: Color) {
        if color.a == 0 || rect.is_empty() {
            return;
        }
        let m = self.transform();
        let rect = if is_identity(&m) {
            rect
        } else if is_axis_aligned(&m) {
            transform_rect_aligned(&m, rect)
        } else {
            let path = rect_path(rect);
            return self.fill_path(&path, FillRule::NonZero, &Paint::Solid(color));
        };
        self.fill_device_rect(rect, color);
    }

    /// Fill a device-space rect (no transform), honoring clip and mask.
    pub(crate) fn fill_device_rect(&mut self, rect: Rect, color: Color) {
        let clip = self.clip().intersect(&self.full_rect());
        let r = rect.intersect(&clip);
        if r.is_empty() {
            return;
        }
        let b = self.bounds(&rect);
        if b.is_empty() {
            return;
        }
        let mask = self.current_mask();
        let integer_aligned = r.x.fract() == 0.0 && r.y.fract() == 0.0 && r.right().fract() == 0.0 && r.bottom().fract() == 0.0;
        if integer_aligned && color.a == 255 && mask.is_none() {
            let v = color.to_u32();
            let w = self.width as usize;
            for y in b.y0..b.y1 {
                let row = y as usize * w;
                self.data[row + b.x0 as usize..row + b.x1 as usize].fill(v);
            }
            return;
        }
        let a = color.a as f32;
        let w = self.width as usize;
        for y in b.y0..b.y1 {
            let cy = span_coverage(y as f32, r.y, r.bottom());
            if cy <= 0.0 {
                continue;
            }
            let row = y as usize * w;
            for x in b.x0..b.x1 {
                let cx = span_coverage(x as f32, r.x, r.right());
                let mut alpha = (a * cx * cy + 0.5) as u32;
                if let Some(mk) = &mask {
                    alpha = (alpha * mk.get(x, y) as u32 + 127) / 255;
                }
                if alpha > 0 {
                    let idx = row + x as usize;
                    self.data[idx] = blend(self.data[idx], color, alpha);
                }
            }
        }
    }

    /// Composite a coverage mask with a shader, honoring clip and clip mask.
    pub(crate) fn fill_mask(&mut self, cov: &Mask, shader: &Shader) {
        if cov.is_empty() {
            return;
        }
        let b = self.bounds(&cov.rect());
        if b.is_empty() {
            return;
        }
        let clipmask = self.current_mask();
        let w = self.width as usize;
        let solid = shader.is_solid();
        let cw = cov.width as usize;
        for y in b.y0..b.y1 {
            let row = y as usize * w;
            let mrow = (y - cov.y0) as usize * cw;
            let covrow = &cov.data[mrow + (b.x0 - cov.x0) as usize..mrow + (b.x1 - cov.x0) as usize];
            let dst = &mut self.data[row + b.x0 as usize..row + b.x1 as usize];
            match (solid, &clipmask) {
                (Some(color), None) => {
                    let ca = color.a as u32;
                    let v = color.to_u32();
                    for (d, &c) in dst.iter_mut().zip(covrow) {
                        if c == 0 {
                            continue;
                        }
                        if c == 255 && ca == 255 {
                            *d = v;
                        } else {
                            *d = blend(*d, color, (c as u32 * ca + 127) / 255);
                        }
                    }
                }
                (None, None) => {
                    let linear = shader.row_params(b.x0, y);
                    for (i, (d, &c)) in dst.iter_mut().zip(covrow).enumerate() {
                        if c == 0 {
                            continue;
                        }
                        let color = match linear {
                            Some((t0, dt, repeat)) => shader.lut_color(t0 + dt * i as f32, repeat),
                            None => shader.color_at(b.x0 + i as i32, y),
                        };
                        if color.a == 0 {
                            continue;
                        }
                        if c == 255 && color.a == 255 {
                            *d = color.to_u32();
                        } else {
                            *d = blend(*d, color, (c as u32 * color.a as u32 + 127) / 255);
                        }
                    }
                }
                (_, Some(mk)) => {
                    let linear = shader.row_params(b.x0, y);
                    for (i, (d, &c)) in dst.iter_mut().zip(covrow).enumerate() {
                        if c == 0 {
                            continue;
                        }
                        let x = b.x0 + i as i32;
                        let color = match (solid, linear) {
                            (Some(col), _) => col,
                            (None, Some((t0, dt, repeat))) => shader.lut_color(t0 + dt * i as f32, repeat),
                            (None, None) => shader.color_at(x, y),
                        };
                        if color.a == 0 {
                            continue;
                        }
                        let alpha = (c as u32 * color.a as u32 + 127) / 255;
                        let alpha = (alpha * mk.get(x, y) as u32 + 127) / 255;
                        if alpha > 0 {
                            *d = blend(*d, color, alpha);
                        }
                    }
                }
            }
        }
    }

    /// Fill a rect with rounded corners (radius clamped to half the size).
    pub fn fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
        if radius <= 0.5 {
            return self.fill_rect(rect, color);
        }
        let r = (radius, radius);
        self.fill_rounded_rect_radii(rect, [r; 4], &Paint::Solid(color));
    }

    /// Stroke a rect border given per-side widths (top, right, bottom, left)
    /// and colors, optionally with rounded corners. `styles` per side:
    /// 0 = solid, 1 = dashed, 2 = dotted, 3 = double (4-7: 3D styles).
    pub fn stroke_rect(&mut self, rect: Rect, widths: [f32; 4], colors: [Color; 4], styles: [u8; 4], radius: f32) {
        let r = (radius.max(0.0), radius.max(0.0));
        self.stroke_rect_radii(rect, widths, colors, styles, [r; 4]);
    }

    /// Anti-aliased line of given width.
    pub fn draw_line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, color: Color) {
        if color.a == 0 || width <= 0.0 {
            return;
        }
        let half = width / 2.0;
        let aligned = is_identity(&self.transform()) || is_axis_aligned(&self.transform());
        // Axis-aligned lines are snapped to the pixel grid so 1px rules
        // (underlines, <hr>) stay crisp instead of smearing over two rows.
        if aligned && (y0 - y1).abs() < 0.01 {
            let (a, b) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
            let top = (y0 - half).round();
            return self.fill_rect(Rect::new(a, top, b - a, width.round().max(1.0)), color);
        }
        if aligned && (x0 - x1).abs() < 0.01 {
            let (a, b) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
            let left = (x0 - half).round();
            return self.fill_rect(Rect::new(left, a, width.round().max(1.0), b - a), color);
        }
        let path = [PathSeg::MoveTo(Point::new(x0, y0)), PathSeg::LineTo(Point::new(x1, y1))];
        let style = StrokeStyle { width: width.max(0.75), ..Default::default() };
        self.stroke_path(&path, &style, &Paint::Solid(color));
    }

    /// Blit a glyph coverage mask with its top-left at device (x, y).
    pub fn draw_glyph(&mut self, bitmap: &GlyphBitmap, x: i32, y: i32, color: Color) {
        if color.a == 0 || bitmap.width == 0 || bitmap.height == 0 {
            return;
        }
        let b = self.bounds(&Rect::new(x as f32, y as f32, bitmap.width as f32, bitmap.height as f32));
        if b.is_empty() {
            return;
        }
        let mask = self.current_mask();
        let w = self.width as usize;
        let bw = bitmap.width as usize;
        let a = color.a as u32;
        let luma = (color.r as u32 * 299 + color.g as u32 * 587 + color.b as u32 * 114) / 1000;
        let lut = if luma < 140 { Some(text_gamma_lut()) } else { None };
        for py in b.y0..b.y1 {
            let srow = (py - y) as usize * bw;
            let drow = py as usize * w;
            for px in b.x0..b.x1 {
                let mut cov = bitmap.coverage[srow + (px - x) as usize] as u32;
                if cov == 0 {
                    continue;
                }
                if let Some(l) = lut {
                    cov = l[cov as usize] as u32;
                }
                let mut alpha = (cov * a + 127) / 255;
                if let Some(mk) = &mask {
                    alpha = (alpha * mk.get(px, py) as u32 + 127) / 255;
                }
                let idx = drow + px as usize;
                self.data[idx] = blend(self.data[idx], color, alpha);
            }
        }
    }

    /// Draw `text` with its baseline at `baseline_y`, returns the advance.
    pub fn draw_text(&mut self, fonts: &FontContext, font: FontId, size: f32, text: &str, x: f32, baseline_y: f32, color: Color) -> f32 {
        self.draw_text_ext(fonts, font, size, text, x, baseline_y, color, 0.0, false, false)
    }

    /// Draw an RGBA image scaled into `dest` (box filtered when downscaling,
    /// bilinear when upscaling), with alpha blending and `opacity`.
    pub fn draw_image(&mut self, image: &Image, dest: Rect, opacity: f32) {
        let src = Rect::new(0.0, 0.0, image.width as f32, image.height as f32);
        self.draw_image_region(image, src, dest, opacity);
    }

    /// Copy of the pixels as tightly packed RGBA8 (for PNG export).
    pub fn to_rgba8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.data.len() * 4);
        for &p in &self.data {
            out.push(((p >> 16) & 0xff) as u8);
            out.push(((p >> 8) & 0xff) as u8);
            out.push((p & 0xff) as u8);
            out.push(255);
        }
        out
    }

    /// Blit another canvas at (x, y) (used to composite the browser chrome).
    pub fn blit(&mut self, src: &Canvas, x: i32, y: i32) {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + src.width as i32).min(self.width as i32);
        let y1 = (y + src.height as i32).min(self.height as i32);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let sw = src.width as usize;
        let dw = self.width as usize;
        for py in y0..y1 {
            let srow = (py - y) as usize * sw + (x0 - x) as usize;
            let drow = py as usize * dw + x0 as usize;
            let n = (x1 - x0) as usize;
            self.data[drow..drow + n].copy_from_slice(&src.data[srow..srow + n]);
        }
    }
}

/// Average of the source pixels covering [u0,u1)×[v0,v1) (premultiplied
/// average so transparent pixels don't bleed colour). Returns straight RGBA
/// as f32 0..255.
pub(crate) fn sample_box(img: &Image, u0: f32, v0: f32, u1: f32, v1: f32) -> [f32; 4] {
    let x0 = u0.floor().max(0.0) as u32;
    let y0 = v0.floor().max(0.0) as u32;
    let x1 = (u1.ceil() as u32).min(img.width).max(x0 + 1);
    let y1 = (v1.ceil() as u32).min(img.height).max(y0 + 1);
    let mut r = 0.0f32;
    let mut g = 0.0f32;
    let mut b = 0.0f32;
    let mut a = 0.0f32;
    let mut wsum = 0.0f32;
    let px = img.pixels();
    for y in y0..y1 {
        let wy = span_coverage(y as f32, v0, v1);
        if wy <= 0.0 {
            continue;
        }
        let row = (y * img.width * 4) as usize;
        for x in x0..x1 {
            let wx = span_coverage(x as f32, u0, u1);
            if wx <= 0.0 {
                continue;
            }
            let wgt = wx * wy;
            let i = row + (x * 4) as usize;
            let pa = px[i + 3] as f32 * wgt;
            r += px[i] as f32 * pa;
            g += px[i + 1] as f32 * pa;
            b += px[i + 2] as f32 * pa;
            a += pa;
            wsum += wgt;
        }
    }
    if wsum <= 0.0 || a <= 0.0 {
        return [0.0, 0.0, 0.0, 0.0];
    }
    [r / a, g / a, b / a, a / wsum]
}

/// Nearest-neighbour sample at source pixel coordinates (straight RGBA).
pub(crate) fn sample_nearest(img: &Image, u: f32, v: f32) -> [f32; 4] {
    let x = (u.floor().max(0.0) as u32).min(img.width - 1);
    let y = (v.floor().max(0.0) as u32).min(img.height - 1);
    let i = ((y * img.width + x) * 4) as usize;
    let px = img.pixels();
    [px[i] as f32, px[i + 1] as f32, px[i + 2] as f32, px[i + 3] as f32]
}

/// Bilinear sample at source coordinates (u, v) in pixel units (centre at .5
/// already subtracted by the caller). Returns straight RGBA 0..255.
pub(crate) fn sample_bilinear(img: &Image, u: f32, v: f32) -> [f32; 4] {
    let maxx = (img.width - 1) as f32;
    let maxy = (img.height - 1) as f32;
    let u = u.clamp(0.0, maxx);
    let v = v.clamp(0.0, maxy);
    let x0 = u.floor() as u32;
    let y0 = v.floor() as u32;
    let x1 = (x0 + 1).min(img.width - 1);
    let y1 = (y0 + 1).min(img.height - 1);
    let fx = u - x0 as f32;
    let fy = v - y0 as f32;
    let px = img.pixels();
    let p = |x: u32, y: u32| {
        let i = ((y * img.width + x) * 4) as usize;
        let a = px[i + 3] as f32;
        [px[i] as f32 * a, px[i + 1] as f32 * a, px[i + 2] as f32 * a, a]
    };
    let p00 = p(x0, y0);
    let p10 = p(x1, y0);
    let p01 = p(x0, y1);
    let p11 = p(x1, y1);
    let mut out = [0.0f32; 4];
    for i in 0..4 {
        let top = p00[i] * (1.0 - fx) + p10[i] * fx;
        let bot = p01[i] * (1.0 - fx) + p11[i] * fx;
        out[i] = top * (1.0 - fy) + bot * fy;
    }
    if out[3] <= 0.0 {
        return [0.0, 0.0, 0.0, 0.0];
    }
    [out[0] / out[3], out[1] / out[3], out[2] / out[3], out[3]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::Family;

    fn px(c: &Canvas, x: u32, y: u32) -> Color {
        Color::from_u32(c.data[(y * c.width + x) as usize])
    }

    #[test]
    fn fill_rect_opaque_and_blended() {
        let mut c = Canvas::new(10, 10);
        c.fill_rect(Rect::new(2.0, 2.0, 3.0, 3.0), Color::rgb(255, 0, 0));
        assert_eq!(px(&c, 2, 2), Color::rgb(255, 0, 0));
        assert_eq!(px(&c, 4, 4), Color::rgb(255, 0, 0));
        assert_eq!(px(&c, 5, 5), Color::WHITE);
        assert_eq!(px(&c, 1, 2), Color::WHITE);
        // 50% black over white -> mid grey
        c.fill_rect(Rect::new(0.0, 0.0, 1.0, 1.0), Color::rgba(0, 0, 0, 128));
        let p = px(&c, 0, 0);
        assert!((p.r as i32 - 127).abs() <= 1, "{p:?}");
        // Fractional edge: half-covered column
        let mut c = Canvas::new(4, 1);
        c.fill_rect(Rect::new(0.5, 0.0, 1.0, 1.0), Color::BLACK);
        let p0 = px(&c, 0, 0);
        let p1 = px(&c, 1, 0);
        assert!((p0.r as i32 - 127).abs() <= 1 && (p1.r as i32 - 127).abs() <= 1, "{p0:?} {p1:?}");
        assert_eq!(px(&c, 2, 0), Color::WHITE);
        // Transparent is a no-op
        c.fill_rect(Rect::new(0.0, 0.0, 4.0, 1.0), Color::TRANSPARENT);
        assert_eq!(px(&c, 2, 0), Color::WHITE);
    }

    #[test]
    fn clipping() {
        let mut c = Canvas::new(10, 10);
        c.push_clip(Rect::new(2.0, 2.0, 4.0, 4.0));
        c.push_clip(Rect::new(0.0, 0.0, 4.0, 10.0));
        assert_eq!(c.clip(), Rect::new(2.0, 2.0, 2.0, 4.0));
        c.fill_rect(Rect::new(0.0, 0.0, 10.0, 10.0), Color::BLACK);
        assert_eq!(px(&c, 2, 2), Color::BLACK);
        assert_eq!(px(&c, 3, 5), Color::BLACK);
        assert_eq!(px(&c, 4, 2), Color::WHITE);
        assert_eq!(px(&c, 1, 1), Color::WHITE);
        c.pop_clip();
        c.pop_clip();
        assert_eq!(c.clip(), Rect::new(0.0, 0.0, 10.0, 10.0));
        // Rect entirely outside the canvas is fine
        c.fill_rect(Rect::new(-50.0, -50.0, 10.0, 10.0), Color::BLACK);
        c.fill_rect(Rect::new(50.0, 50.0, 10.0, 10.0), Color::BLACK);
        c.draw_glyph(
            &GlyphBitmap { width: 2, height: 2, left: 0, top: 0, coverage: vec![255; 4] },
            -1,
            -1,
            Color::BLACK,
        );
        assert_eq!(px(&c, 0, 0), Color::BLACK);
        assert_eq!(px(&c, 1, 1), Color::WHITE);
    }

    #[test]
    fn rounded_rect() {
        let mut c = Canvas::new(20, 20);
        c.fill_rounded_rect(Rect::new(0.0, 0.0, 20.0, 20.0), 8.0, Color::BLACK);
        assert_eq!(px(&c, 10, 10), Color::BLACK);
        assert_eq!(px(&c, 10, 0), Color::BLACK);
        assert_eq!(px(&c, 0, 10), Color::BLACK);
        // Corner pixel is outside the circle -> stays (almost) white
        assert!(px(&c, 0, 0).r > 200);
        assert!(px(&c, 19, 19).r > 200);
        // Somewhere along the arc we should see a partial pixel
        let partial = (0..8).any(|x| {
            (0..8).any(|y| {
                let p = px(&c, x, y);
                p.r > 20 && p.r < 235
            })
        });
        assert!(partial);
        // radius 0 falls back to a plain rect
        let mut c = Canvas::new(4, 4);
        c.fill_rounded_rect(Rect::new(0.0, 0.0, 4.0, 4.0), 0.0, Color::BLACK);
        assert_eq!(px(&c, 0, 0), Color::BLACK);
    }

    #[test]
    fn borders() {
        let mut c = Canvas::new(10, 10);
        let red = Color::rgb(255, 0, 0);
        let blue = Color::rgb(0, 0, 255);
        c.stroke_rect(Rect::new(0.0, 0.0, 10.0, 10.0), [2.0, 1.0, 2.0, 1.0], [red, blue, red, blue], [0; 4], 0.0);
        assert_eq!(px(&c, 5, 0), red);
        assert_eq!(px(&c, 5, 1), red);
        assert_eq!(px(&c, 5, 2), Color::WHITE);
        assert_eq!(px(&c, 0, 5), blue);
        assert_eq!(px(&c, 9, 5), blue);
        assert_eq!(px(&c, 5, 5), Color::WHITE);
        // Rounded: middle of the top edge is painted, centre is not, corner outside is not
        let mut c = Canvas::new(20, 20);
        c.stroke_rect(Rect::new(0.0, 0.0, 20.0, 20.0), [2.0; 4], [red; 4], [0; 4], 6.0);
        assert_eq!(px(&c, 10, 0), red);
        assert_eq!(px(&c, 10, 10), Color::WHITE);
        assert!(px(&c, 0, 0).g > 200);
        // Dashed draws some but not all pixels along the side
        let mut c = Canvas::new(40, 10);
        c.stroke_rect(Rect::new(0.0, 0.0, 40.0, 10.0), [2.0, 0.0, 0.0, 0.0], [red; 4], [1; 4], 0.0);
        let painted = (0..40).filter(|&x| px(&c, x, 0) == red).count();
        assert!(painted > 10 && painted < 40, "{painted}");
    }

    #[test]
    fn lines_and_text() {
        let mut c = Canvas::new(20, 20);
        c.draw_line(2.0, 5.0, 18.0, 5.0, 1.0, Color::BLACK);
        assert_eq!(px(&c, 10, 5), Color::BLACK);
        assert_eq!(px(&c, 10, 6), Color::WHITE);
        c.draw_line(0.0, 0.0, 19.0, 19.0, 1.0, Color::BLACK);
        assert!(px(&c, 10, 10).r < 128);
        let fonts = FontContext::new();
        let f = fonts.select(Family::Sans, 700, false);
        let mut c = Canvas::new(100, 30);
        let adv = c.draw_text(&fonts, f, 16.0, "Hello", 2.0, 20.0, Color::BLACK);
        assert!(adv > 20.0);
        let dark = c.data.iter().filter(|&&p| (p & 0xff) < 100).count();
        assert!(dark > 20, "dark pixels: {dark}");
        // Nothing painted below the baseline+descent area far right
        assert_eq!(px(&c, 99, 29), Color::WHITE);
    }

    #[test]
    fn images_and_export() {
        // 2x2 image: red, green / blue, transparent
        let img = Image::new(2, 2, vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 0, 0, 0, 0]);
        let mut c = Canvas::new(8, 8);
        c.draw_image(&img, Rect::new(0.0, 0.0, 8.0, 8.0), 1.0);
        assert_eq!(px(&c, 0, 0), Color::rgb(255, 0, 0));
        assert_eq!(px(&c, 7, 0), Color::rgb(0, 255, 0));
        assert_eq!(px(&c, 0, 7), Color::rgb(0, 0, 255));
        assert_eq!(px(&c, 7, 7), Color::WHITE); // transparent
        // Downscale 8x8 solid grey to 2x2
        let big = Image::new(8, 8, vec![100; 8 * 8 * 4]);
        let mut c = Canvas::new(2, 2);
        c.draw_image(&big, Rect::new(0.0, 0.0, 2.0, 2.0), 1.0);
        let p = px(&c, 1, 1);
        // alpha 100/255 of grey 100 over white
        assert!((p.r as i32 - 194).abs() <= 2, "{p:?}");
        // Partially off-canvas
        let mut c = Canvas::new(4, 4);
        c.draw_image(&img, Rect::new(-2.0, -2.0, 4.0, 4.0), 0.5);
        // canvas (0,0) samples around the image's green pixel (1,0), 50% over white
        let p = px(&c, 0, 0);
        assert!(p != Color::WHITE && p.g > p.r, "{p:?}");
        assert_eq!(px(&c, 2, 2), Color::WHITE);
        let rgba = c.to_rgba8();
        assert_eq!(rgba.len(), 4 * 4 * 4);
        assert_eq!(rgba[3], 255);
        // blit
        let mut dst = Canvas::new(6, 6);
        let mut src = Canvas::new(3, 3);
        src.clear(Color::BLACK);
        dst.blit(&src, 4, 4);
        assert_eq!(px(&dst, 4, 4), Color::BLACK);
        assert_eq!(px(&dst, 3, 3), Color::WHITE);
        dst.resize(2, 2);
        assert_eq!(dst.data.len(), 4);
        assert_eq!(px(&dst, 1, 1), Color::WHITE);
    }
}

// ------------------------------------------------------------------ v0.2 API
// Gradients, shadows, per-corner radii, transforms and paths. Implemented
// in `canvas_ext.rs`.

/// Corner radii as [top-left, top-right, bottom-right, bottom-left], each
/// (horizontal, vertical) in px.
pub type Radii = [(f32, f32); 4];

/// One gradient color stop at `offset` ∈ 0..=1 along the gradient line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorStop {
    pub offset: f32,
    pub color: Color,
}

/// Affine transform [a, b, c, d, e, f]: x' = a·x + c·y + e, y' = b·x + d·y + f.
pub type Matrix = [f32; 6];

pub const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// A 2-D point.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// Path segments for `fill_path` / `stroke_path` (used by borders with
/// radii, SVG and shapes). Curves are flattened by the rasterizer.
#[derive(Debug, Clone, PartialEq)]
pub enum PathSeg {
    MoveTo(Point),
    LineTo(Point),
    QuadTo(Point, Point),
    CubicTo(Point, Point, Point),
    /// Elliptical arc (SVG semantics): radii, x-axis rotation (rad),
    /// large-arc, sweep, end point.
    ArcTo {
        rx: f32,
        ry: f32,
        rotation: f32,
        large_arc: bool,
        sweep: bool,
        to: Point,
    },
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillRule {
    NonZero,
    EvenOdd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineCap {
    Butt,
    Round,
    Square,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineJoin {
    Miter,
    Round,
    Bevel,
}

/// Stroke parameters for `stroke_path`.
#[derive(Debug, Clone, PartialEq)]
pub struct StrokeStyle {
    pub width: f32,
    pub cap: LineCap,
    pub join: LineJoin,
    pub miter_limit: f32,
    /// Dash lengths (on, off, ...); empty = solid.
    pub dashes: Vec<f32>,
    pub dash_offset: f32,
}

impl Default for StrokeStyle {
    fn default() -> Self {
        StrokeStyle { width: 1.0, cap: LineCap::Butt, join: LineJoin::Miter, miter_limit: 4.0, dashes: Vec::new(), dash_offset: 0.0 }
    }
}

/// What to paint a shape with.
#[derive(Debug, Clone, PartialEq)]
pub enum Paint {
    Solid(Color),
    /// Linear gradient from `start` to `end` (canvas coordinates after the
    /// current transform), with `repeat` for repeating gradients.
    Linear {
        start: Point,
        end: Point,
        stops: Vec<ColorStop>,
        repeat: bool,
    },
    /// Elliptical radial gradient centred at `center` with radii (rx, ry).
    Radial {
        center: Point,
        rx: f32,
        ry: f32,
        stops: Vec<ColorStop>,
        repeat: bool,
    },
}
