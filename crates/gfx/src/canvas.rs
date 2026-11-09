//! Pixel canvas and rasterization primitives.

use crate::font::{FontContext, FontId};
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
}

/// An RGB framebuffer. Pixels are `0x00RRGGBB` so the buffer can be handed
/// straight to the window surface.
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u32>,
    clip_stack: Vec<Rect>,
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
        unimplemented!()
    }

    pub fn clear(&mut self, color: Color) {
        unimplemented!()
    }

    /// Current clip rectangle (whole canvas if none pushed).
    pub fn clip(&self) -> Rect {
        unimplemented!()
    }

    /// Push a clip that is intersected with the current one.
    pub fn push_clip(&mut self, rect: Rect) {
        unimplemented!()
    }

    pub fn pop_clip(&mut self) {
        unimplemented!()
    }

    /// Fill an axis-aligned rect with alpha blending and anti-aliased
    /// fractional edges.
    pub fn fill_rect(&mut self, rect: Rect, color: Color) {
        unimplemented!()
    }

    /// Fill a rect with rounded corners (radius clamped to half the size).
    pub fn fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
        unimplemented!()
    }

    /// Stroke a rect border given per-side widths (top, right, bottom, left)
    /// and colors, optionally with rounded corners. `dashed`/`dotted`
    /// are honoured per side via `styles` (0=solid,1=dashed,2=dotted,3=double).
    pub fn stroke_rect(&mut self, rect: Rect, widths: [f32; 4], colors: [Color; 4], styles: [u8; 4], radius: f32) {
        unimplemented!()
    }

    /// Anti-aliased line of given width.
    pub fn draw_line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, color: Color) {
        unimplemented!()
    }

    /// Blit a glyph coverage mask with its top-left at (x, y).
    pub fn draw_glyph(&mut self, bitmap: &crate::font::GlyphBitmap, x: i32, y: i32, color: Color) {
        unimplemented!()
    }

    /// Draw `text` with its baseline at `baseline_y`, returns the advance.
    pub fn draw_text(&mut self, fonts: &FontContext, font: FontId, size: f32, text: &str, x: f32, baseline_y: f32, color: Color) -> f32 {
        unimplemented!()
    }

    /// Draw an RGBA image scaled into `dest` (bilinear when downscaling,
    /// nearest-ish otherwise), with alpha blending and `opacity`.
    pub fn draw_image(&mut self, image: &Image, dest: Rect, opacity: f32) {
        unimplemented!()
    }

    /// Copy of the pixels as tightly packed RGBA8 (for PNG export).
    pub fn to_rgba8(&self) -> Vec<u8> {
        unimplemented!()
    }

    /// Blit another canvas at (x, y) (used to composite the browser chrome).
    pub fn blit(&mut self, src: &Canvas, x: i32, y: i32) {
        unimplemented!()
    }
}
