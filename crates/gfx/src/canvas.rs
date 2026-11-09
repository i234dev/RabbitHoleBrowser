//! Pixel canvas and rasterization primitives.
//!
//! Everything here is plain CPU code over a `Vec<u32>` framebuffer: alpha
//! blended rectangles with anti-aliased fractional edges, rounded corners,
//! borders, lines, glyph masks and scaled images.

use crate::font::{FontContext, FontId, GlyphBitmap};
use crate::image::Image;

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
fn blend(dst: u32, src: Color, alpha: u32) -> u32 {
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

/// Coverage of the pixel span [p, p+1) by the interval [a, b), in 0..=1.
#[inline(always)]
fn span_coverage(p: f32, a: f32, b: f32) -> f32 {
    (b.min(p + 1.0) - a.max(p)).clamp(0.0, 1.0)
}

/// An RGB framebuffer. Pixels are `0x00RRGGBB` so the buffer can be handed
/// straight to the window surface.
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u32>,
    clip_stack: Vec<Rect>,
}

/// Integer pixel bounds [x0, x1) × [y0, y1).
#[derive(Clone, Copy)]
struct Bounds {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

impl Bounds {
    fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Canvas {
        Canvas {
            width,
            height,
            data: vec![0xffffff; (width * height) as usize],
            clip_stack: Vec::new(),
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.data.clear();
        self.data.resize((width * height) as usize, 0xffffff);
        self.clip_stack.clear();
    }

    pub fn clear(&mut self, color: Color) {
        let v = color.to_u32();
        self.data.fill(v);
    }

    fn full_rect(&self) -> Rect {
        Rect::new(0.0, 0.0, self.width as f32, self.height as f32)
    }

    /// Current clip rectangle (whole canvas if none pushed).
    pub fn clip(&self) -> Rect {
        match self.clip_stack.last() {
            Some(r) => *r,
            None => self.full_rect(),
        }
    }

    /// Push a clip that is intersected with the current one.
    pub fn push_clip(&mut self, rect: Rect) {
        let c = self.clip().intersect(&rect);
        self.clip_stack.push(c);
    }

    pub fn pop_clip(&mut self) {
        self.clip_stack.pop();
    }

    /// Integer pixel bounds of `rect` intersected with the clip and canvas.
    fn bounds(&self, rect: &Rect) -> Bounds {
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

    #[inline(always)]
    fn put(&mut self, x: i32, y: i32, color: Color, alpha: u32) {
        let idx = y as usize * self.width as usize + x as usize;
        self.data[idx] = blend(self.data[idx], color, alpha);
    }

    /// Fill an axis-aligned rect with alpha blending and anti-aliased
    /// fractional edges.
    pub fn fill_rect(&mut self, rect: Rect, color: Color) {
        if color.a == 0 || rect.is_empty() {
            return;
        }
        let clip = self.clip().intersect(&self.full_rect());
        let r = rect.intersect(&clip);
        if r.is_empty() {
            return;
        }
        let b = self.bounds(&rect);
        if b.is_empty() {
            return;
        }
        let integer_aligned = r.x.fract() == 0.0 && r.y.fract() == 0.0 && r.right().fract() == 0.0 && r.bottom().fract() == 0.0;
        if integer_aligned && color.a == 255 {
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
                let alpha = (a * cx * cy + 0.5) as u32;
                if alpha > 0 {
                    let idx = row + x as usize;
                    self.data[idx] = blend(self.data[idx], color, alpha);
                }
            }
        }
    }

    /// Fill a rect with rounded corners (radius clamped to half the size).
    pub fn fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
        let radius = radius.min(rect.w / 2.0).min(rect.h / 2.0);
        if radius <= 0.5 {
            return self.fill_rect(rect, color);
        }
        if color.a == 0 || rect.is_empty() {
            return;
        }
        let b = self.bounds(&rect);
        if b.is_empty() {
            return;
        }
        let a = color.a as f32;
        let w = self.width as usize;
        let shape = RoundedRect::new(rect, radius);
        for y in b.y0..b.y1 {
            let row = y as usize * w;
            for x in b.x0..b.x1 {
                let cov = shape.coverage(x, y);
                let alpha = (a * cov + 0.5) as u32;
                if alpha > 0 {
                    let idx = row + x as usize;
                    self.data[idx] = blend(self.data[idx], color, alpha);
                }
            }
        }
    }

    /// Stroke a rect border given per-side widths (top, right, bottom, left)
    /// and colors, optionally with rounded corners. `styles` per side:
    /// 0 = solid, 1 = dashed, 2 = dotted, 3 = double.
    pub fn stroke_rect(&mut self, rect: Rect, widths: [f32; 4], colors: [Color; 4], styles: [u8; 4], radius: f32) {
        if rect.is_empty() || widths.iter().all(|w| *w <= 0.0) {
            return;
        }
        let [wt, wr, wb, wl] = widths;
        let radius = radius.min(rect.w / 2.0).min(rect.h / 2.0);
        if radius > 0.5 {
            self.stroke_rounded(rect, widths, colors, radius);
            return;
        }
        // Top
        if wt > 0.0 {
            self.stroke_side(Rect::new(rect.x, rect.y, rect.w, wt), colors[0], styles[0], wt, true, wl, wr);
        }
        if wb > 0.0 {
            self.stroke_side(Rect::new(rect.x, rect.bottom() - wb, rect.w, wb), colors[2], styles[2], wb, true, wl, wr);
        }
        if wl > 0.0 {
            self.stroke_side(Rect::new(rect.x, rect.y + wt, wl, rect.h - wt - wb), colors[3], styles[3], wl, false, 0.0, 0.0);
        }
        if wr > 0.0 {
            self.stroke_side(Rect::new(rect.right() - wr, rect.y + wt, wr, rect.h - wt - wb), colors[1], styles[1], wr, false, 0.0, 0.0);
        }
    }

    /// Draw one side of a border. `band` is the full strip; for dashed and
    /// dotted styles the corner squares (where adjacent sides meet) are
    /// always filled so corners look closed.
    fn stroke_side(&mut self, band: Rect, color: Color, style: u8, width: f32, horizontal: bool, corner_a: f32, corner_b: f32) {
        if color.a == 0 || band.is_empty() {
            return;
        }
        match style {
            1 | 2 => {
                let (dash, gap) = if style == 1 { (width * 3.0, width * 1.5) } else { (width, width) };
                let period = dash + gap;
                if horizontal {
                    let start = band.x + corner_a;
                    let end = band.right() - corner_b;
                    if corner_a > 0.0 {
                        self.fill_rect(Rect::new(band.x, band.y, corner_a, band.h), color);
                    }
                    if corner_b > 0.0 {
                        self.fill_rect(Rect::new(end, band.y, corner_b, band.h), color);
                    }
                    let mut x = start;
                    while x < end {
                        let w = dash.min(end - x);
                        if style == 2 {
                            self.fill_rounded_rect(Rect::new(x, band.y, w, band.h), width / 2.0, color);
                        } else {
                            self.fill_rect(Rect::new(x, band.y, w, band.h), color);
                        }
                        x += period;
                    }
                } else {
                    let mut y = band.y;
                    while y < band.bottom() {
                        let h = dash.min(band.bottom() - y);
                        if style == 2 {
                            self.fill_rounded_rect(Rect::new(band.x, y, band.w, h), width / 2.0, color);
                        } else {
                            self.fill_rect(Rect::new(band.x, y, band.w, h), color);
                        }
                        y += period;
                    }
                }
            }
            3 => {
                let t = (width / 3.0).max(1.0);
                if horizontal {
                    self.fill_rect(Rect::new(band.x, band.y, band.w, t), color);
                    self.fill_rect(Rect::new(band.x, band.bottom() - t, band.w, t), color);
                } else {
                    self.fill_rect(Rect::new(band.x, band.y, t, band.h), color);
                    self.fill_rect(Rect::new(band.right() - t, band.y, t, band.h), color);
                }
            }
            _ => self.fill_rect(band, color),
        }
    }

    fn stroke_rounded(&mut self, rect: Rect, widths: [f32; 4], colors: [Color; 4], radius: f32) {
        let [wt, wr, wb, wl] = widths;
        let outer = RoundedRect::new(rect, radius);
        let inner_rect = rect.inset(wt, wr, wb, wl);
        let inner_radius = (radius - widths.iter().cloned().fold(0.0, f32::max)).max(0.0);
        let inner = if inner_rect.is_empty() { None } else { Some(RoundedRect::new(inner_rect, inner_radius)) };
        let b = self.bounds(&rect);
        if b.is_empty() {
            return;
        }
        let w = self.width as usize;
        for y in b.y0..b.y1 {
            let row = y as usize * w;
            let py = y as f32 + 0.5;
            for x in b.x0..b.x1 {
                let cov_outer = outer.coverage(x, y);
                if cov_outer <= 0.0 {
                    continue;
                }
                let cov_inner = inner.map(|i| i.coverage(x, y)).unwrap_or(0.0);
                let cov = (cov_outer - cov_inner).clamp(0.0, 1.0);
                if cov <= 0.0 {
                    continue;
                }
                let px = x as f32 + 0.5;
                // Pick the side color by the band the pixel centre falls in.
                let side = if py < rect.y + wt && wt > 0.0 {
                    // top band, unless clearly in the left/right band near corners
                    if px < rect.x + wl && (px - rect.x) / wl.max(0.01) < (py - rect.y) / wt {
                        3
                    } else if px >= rect.right() - wr && (rect.right() - px) / wr.max(0.01) < (py - rect.y) / wt {
                        1
                    } else {
                        0
                    }
                } else if py >= rect.bottom() - wb && wb > 0.0 {
                    if px < rect.x + wl && (px - rect.x) / wl.max(0.01) < (rect.bottom() - py) / wb {
                        3
                    } else if px >= rect.right() - wr && (rect.right() - px) / wr.max(0.01) < (rect.bottom() - py) / wb {
                        1
                    } else {
                        2
                    }
                } else if px < rect.x + wl {
                    3
                } else if px >= rect.right() - wr {
                    1
                } else if px - rect.x < rect.right() - px {
                    3
                } else {
                    1
                };
                let color = colors[side];
                let alpha = (color.a as f32 * cov + 0.5) as u32;
                if alpha > 0 {
                    let idx = row + x as usize;
                    self.data[idx] = blend(self.data[idx], color, alpha);
                }
            }
        }
    }

    /// Anti-aliased line of given width.
    pub fn draw_line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, color: Color) {
        if color.a == 0 || width <= 0.0 {
            return;
        }
        let half = width / 2.0;
        if (y0 - y1).abs() < 0.01 {
            let (a, b) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
            return self.fill_rect(Rect::new(a, y0 - half, b - a, width), color);
        }
        if (x0 - x1).abs() < 0.01 {
            let (a, b) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
            return self.fill_rect(Rect::new(x0 - half, a, width, b - a), color);
        }
        // General case: walk along the major axis stamping small squares.
        let dx = x1 - x0;
        let dy = y1 - y0;
        let len = (dx * dx + dy * dy).sqrt();
        let steps = (len / (width.min(1.0) * 0.5)).ceil().max(1.0) as usize;
        let stamp = width.max(1.0);
        // Stamping overlaps; use full colour but avoid re-blending the same
        // pixel many times by tracking the last stamped integer position.
        let mut last = (i32::MIN, i32::MIN);
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let px = x0 + dx * t;
            let py = y0 + dy * t;
            let key = ((px * 2.0) as i32, (py * 2.0) as i32);
            if key == last {
                continue;
            }
            last = key;
            self.fill_rect(Rect::new(px - stamp / 2.0, py - stamp / 2.0, stamp, stamp), color);
        }
    }

    /// Blit a glyph coverage mask with its top-left at (x, y).
    pub fn draw_glyph(&mut self, bitmap: &GlyphBitmap, x: i32, y: i32, color: Color) {
        if color.a == 0 || bitmap.width == 0 || bitmap.height == 0 {
            return;
        }
        let b = self.bounds(&Rect::new(x as f32, y as f32, bitmap.width as f32, bitmap.height as f32));
        if b.is_empty() {
            return;
        }
        let w = self.width as usize;
        let bw = bitmap.width as usize;
        let a = color.a as u32;
        for py in b.y0..b.y1 {
            let srow = (py - y) as usize * bw;
            let drow = py as usize * w;
            for px in b.x0..b.x1 {
                let cov = bitmap.coverage[srow + (px - x) as usize] as u32;
                if cov == 0 {
                    continue;
                }
                let alpha = (cov * a + 127) / 255;
                let idx = drow + px as usize;
                self.data[idx] = blend(self.data[idx], color, alpha);
            }
        }
    }

    /// Draw `text` with its baseline at `baseline_y`, returns the advance.
    pub fn draw_text(&mut self, fonts: &FontContext, font: FontId, size: f32, text: &str, x: f32, baseline_y: f32, color: Color) -> f32 {
        let glyphs = fonts.shape(font, size, text, 0.0);
        let baseline = baseline_y.round() as i32;
        let mut advance = 0.0;
        for g in &glyphs {
            advance = g.x + g.advance;
            if g.glyph == crate::font::ZERO_WIDTH_GLYPH {
                continue;
            }
            let bitmap = fonts.rasterize(g.font, g.glyph, size);
            if bitmap.is_empty() {
                continue;
            }
            let gx = (x + g.x).round() as i32 + bitmap.left;
            let gy = baseline - bitmap.top;
            self.draw_glyph(&bitmap, gx, gy, color);
        }
        advance
    }

    /// Draw an RGBA image scaled into `dest` (box filtered when downscaling,
    /// bilinear when upscaling), with alpha blending and `opacity`.
    pub fn draw_image(&mut self, image: &Image, dest: Rect, opacity: f32) {
        if dest.is_empty() || image.width == 0 || image.height == 0 || opacity <= 0.0 {
            return;
        }
        let b = self.bounds(&dest);
        if b.is_empty() {
            return;
        }
        let sx = image.width as f32 / dest.w;
        let sy = image.height as f32 / dest.h;
        let op = opacity.clamp(0.0, 1.0);
        let w = self.width as usize;
        let downscale = sx > 1.0 || sy > 1.0;
        for py in b.y0..b.y1 {
            let drow = py as usize * w;
            // Source span covered by this destination row.
            let v0 = (py as f32 - dest.y) * sy;
            let v1 = (py as f32 + 1.0 - dest.y) * sy;
            for px in b.x0..b.x1 {
                let u0 = (px as f32 - dest.x) * sx;
                let u1 = (px as f32 + 1.0 - dest.x) * sx;
                let [r, g, bl, a] = if downscale {
                    sample_box(image, u0, v0, u1, v1)
                } else {
                    sample_bilinear(image, (u0 + u1) * 0.5 - 0.5, (v0 + v1) * 0.5 - 0.5)
                };
                let alpha = (a * op + 0.5) as u32;
                if alpha == 0 {
                    continue;
                }
                let idx = drow + px as usize;
                self.data[idx] = blend(self.data[idx], Color::rgb(r as u8, g as u8, bl as u8), alpha);
            }
        }
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

/// Analytic-ish coverage for a rounded rectangle: straight edges use exact
/// span coverage; pixels inside the corner squares are 4×4 supersampled.
#[derive(Clone, Copy)]
struct RoundedRect {
    r: Rect,
    radius: f32,
}

impl RoundedRect {
    fn new(r: Rect, radius: f32) -> Self {
        RoundedRect { r, radius }
    }

    #[inline]
    fn inside(&self, px: f32, py: f32) -> bool {
        let r = &self.r;
        if px < r.x || py < r.y || px >= r.right() || py >= r.bottom() {
            return false;
        }
        let rad = self.radius;
        // Corner centres
        let cx = if px < r.x + rad {
            r.x + rad
        } else if px > r.right() - rad {
            r.right() - rad
        } else {
            return true;
        };
        let cy = if py < r.y + rad {
            r.y + rad
        } else if py > r.bottom() - rad {
            r.bottom() - rad
        } else {
            return true;
        };
        let dx = px - cx;
        let dy = py - cy;
        dx * dx + dy * dy <= rad * rad
    }

    fn coverage(&self, x: i32, y: i32) -> f32 {
        let r = &self.r;
        let fx = x as f32;
        let fy = y as f32;
        let rad = self.radius;
        let in_corner_x = fx + 1.0 > r.x && fx < r.x + rad || fx + 1.0 > r.right() - rad && fx < r.right();
        let in_corner_y = fy + 1.0 > r.y && fy < r.y + rad || fy + 1.0 > r.bottom() - rad && fy < r.bottom();
        if in_corner_x && in_corner_y {
            // Supersample 4x4
            let mut hits = 0;
            for sy in 0..4 {
                let py = fy + (sy as f32 + 0.5) / 4.0;
                for sx in 0..4 {
                    let px = fx + (sx as f32 + 0.5) / 4.0;
                    if self.inside(px, py) {
                        hits += 1;
                    }
                }
            }
            hits as f32 / 16.0
        } else {
            span_coverage(fx, r.x, r.right()) * span_coverage(fy, r.y, r.bottom())
        }
    }
}

/// Average of the source pixels covering [u0,u1)×[v0,v1) (premultiplied
/// average so transparent pixels don't bleed colour). Returns straight RGBA
/// as f32 0..255.
fn sample_box(img: &Image, u0: f32, v0: f32, u1: f32, v1: f32) -> [f32; 4] {
    let x0 = u0.floor().max(0.0) as u32;
    let y0 = v0.floor().max(0.0) as u32;
    let x1 = (u1.ceil() as u32).min(img.width).max(x0 + 1);
    let y1 = (v1.ceil() as u32).min(img.height).max(y0 + 1);
    let mut r = 0.0f32;
    let mut g = 0.0f32;
    let mut b = 0.0f32;
    let mut a = 0.0f32;
    let mut wsum = 0.0f32;
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
            let pa = img.rgba[i + 3] as f32 * wgt;
            r += img.rgba[i] as f32 * pa;
            g += img.rgba[i + 1] as f32 * pa;
            b += img.rgba[i + 2] as f32 * pa;
            a += pa;
            wsum += wgt;
        }
    }
    if wsum <= 0.0 || a <= 0.0 {
        return [0.0, 0.0, 0.0, 0.0];
    }
    [r / a, g / a, b / a, a / wsum]
}

/// Bilinear sample at source coordinates (u, v) in pixel units (centre at .5
/// already subtracted by the caller). Returns straight RGBA 0..255.
fn sample_bilinear(img: &Image, u: f32, v: f32) -> [f32; 4] {
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
    let p = |x: u32, y: u32| {
        let i = ((y * img.width + x) * 4) as usize;
        let a = img.rgba[i + 3] as f32;
        [img.rgba[i] as f32 * a, img.rgba[i + 1] as f32 * a, img.rgba[i + 2] as f32 * a, a]
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
        let partial = (0..8).any(|i| {
            let p = px(&c, i, 7 - i);
            p.r > 20 && p.r < 235
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
        assert!(px(&c, 0, 0).r > 200 && px(&c, 0, 0).g < 200);
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
