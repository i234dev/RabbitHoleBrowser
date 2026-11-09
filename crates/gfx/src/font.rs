//! Fonts: bundled DejaVu faces (so rendering is identical on every OS),
//! optional system font discovery, glyph shaping (advances + kerning,
//! per-character font fallback) and a glyph bitmap cache.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    Sans,
    Serif,
    Mono,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FontId(pub usize);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineMetrics {
    /// Distance from baseline to top of the line box (positive).
    pub ascent: f32,
    /// Distance from baseline to bottom (positive).
    pub descent: f32,
    pub line_gap: f32,
}

impl LineMetrics {
    pub fn height(&self) -> f32 {
        self.ascent + self.descent + self.line_gap
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapedGlyph {
    /// Font actually used (after fallback).
    pub font: FontId,
    pub glyph: u16,
    /// Horizontal offset from the start of the run.
    pub x: f32,
    pub advance: f32,
    /// Byte offset of the source char in the shaped string.
    pub byte_index: usize,
}

/// 8-bit coverage mask of a rasterized glyph.
#[derive(Debug, Clone)]
pub struct GlyphBitmap {
    pub width: u32,
    pub height: u32,
    /// Offset from the glyph origin (on the baseline) to the bitmap's left.
    pub left: i32,
    /// Offset from the baseline up to the bitmap's top (positive = above).
    pub top: i32,
    pub coverage: Vec<u8>,
}

pub struct FontContext {
    faces: Vec<fontdue::Font>,
    names: Vec<String>,
    cache: RefCell<HashMap<(usize, u16, u32), Rc<GlyphBitmap>>>,
}

impl Default for FontContext {
    fn default() -> Self {
        Self::new()
    }
}

impl FontContext {
    /// Load the bundled fonts.
    pub fn new() -> FontContext {
        unimplemented!()
    }

    /// Choose a face for a family, CSS weight (100-900) and style.
    pub fn select(&self, family: Family, weight: u16, italic: bool) -> FontId {
        unimplemented!()
    }

    pub fn metrics(&self, font: FontId, size: f32) -> LineMetrics {
        unimplemented!()
    }

    /// Width of `text` at `size`, including kerning and fallback.
    pub fn measure(&self, font: FontId, size: f32, text: &str) -> f32 {
        unimplemented!()
    }

    /// Shape a run: positions for each char, with per-char font fallback
    /// when the primary face lacks the glyph. `letter_spacing` is added
    /// after every glyph.
    pub fn shape(&self, font: FontId, size: f32, text: &str, letter_spacing: f32) -> Vec<ShapedGlyph> {
        unimplemented!()
    }

    /// Rasterize (cached) a glyph at a given size.
    pub fn rasterize(&self, font: FontId, glyph: u16, size: f32) -> Rc<GlyphBitmap> {
        unimplemented!()
    }

    /// Advance width of a single char (used for tab stops / list markers).
    pub fn char_advance(&self, font: FontId, size: f32, ch: char) -> f32 {
        unimplemented!()
    }

    pub fn face_name(&self, font: FontId) -> &str {
        unimplemented!()
    }
}
