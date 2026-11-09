//! Fonts: bundled DejaVu faces (so rendering is identical on every OS),
//! glyph shaping (advances + kerning, per-character font fallback) and a
//! glyph bitmap cache.

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

/// Glyph index used for characters that take no space and draw nothing
/// (zero-width spaces, joiners, soft hyphens, control characters).
pub const ZERO_WIDTH_GLYPH: u16 = u16::MAX;

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

impl GlyphBitmap {
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

const SANS: usize = 0;
const SANS_BOLD: usize = 1;
const SANS_ITALIC: usize = 2;
const SANS_BOLD_ITALIC: usize = 3;
const MONO: usize = 4;
const SERIF: usize = 5;
const SERIF_BOLD: usize = 6;

/// Fallback search order when a face lacks a glyph.
const FALLBACK_ORDER: [usize; 7] = [SANS, SERIF, MONO, SANS_BOLD, SANS_ITALIC, SANS_BOLD_ITALIC, SERIF_BOLD];

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

fn is_zero_width(ch: char) -> bool {
    matches!(
        ch,
        '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' | '\u{00AD}' | '\u{2060}' | '\u{200E}' | '\u{200F}'
    ) || (ch.is_control() && ch != '\t')
}

impl FontContext {
    /// Load the bundled fonts.
    pub fn new() -> FontContext {
        let sources: [(&str, &[u8]); 7] = [
            ("DejaVu Sans", include_bytes!("../../../assets/fonts/DejaVuSans.ttf")),
            ("DejaVu Sans Bold", include_bytes!("../../../assets/fonts/DejaVuSans-Bold.ttf")),
            ("DejaVu Sans Oblique", include_bytes!("../../../assets/fonts/DejaVuSans-Oblique.ttf")),
            ("DejaVu Sans Bold Oblique", include_bytes!("../../../assets/fonts/DejaVuSans-BoldOblique.ttf")),
            ("DejaVu Sans Mono", include_bytes!("../../../assets/fonts/DejaVuSansMono.ttf")),
            ("DejaVu Serif", include_bytes!("../../../assets/fonts/DejaVuSerif.ttf")),
            ("DejaVu Serif Bold", include_bytes!("../../../assets/fonts/DejaVuSerif-Bold.ttf")),
        ];
        let settings = fontdue::FontSettings {
            scale: 40.0,
            ..Default::default()
        };
        let mut faces = Vec::with_capacity(sources.len());
        let mut names = Vec::with_capacity(sources.len());
        for (name, bytes) in sources {
            let font = fontdue::Font::from_bytes(bytes, settings.clone()).expect("bundled font is valid");
            faces.push(font);
            names.push(name.to_string());
        }
        FontContext {
            faces,
            names,
            cache: RefCell::new(HashMap::new()),
        }
    }

    /// Choose a face for a family, CSS weight (100-900) and style.
    pub fn select(&self, family: Family, weight: u16, italic: bool) -> FontId {
        let bold = weight >= 600;
        let idx = match family {
            Family::Sans => match (bold, italic) {
                (false, false) => SANS,
                (true, false) => SANS_BOLD,
                (false, true) => SANS_ITALIC,
                (true, true) => SANS_BOLD_ITALIC,
            },
            Family::Serif => {
                if bold {
                    SERIF_BOLD
                } else {
                    SERIF
                }
            }
            Family::Mono => MONO,
        };
        FontId(idx)
    }

    fn face(&self, font: FontId) -> &fontdue::Font {
        &self.faces[font.0.min(self.faces.len() - 1)]
    }

    pub fn metrics(&self, font: FontId, size: f32) -> LineMetrics {
        match self.face(font).horizontal_line_metrics(size) {
            Some(m) => LineMetrics {
                ascent: m.ascent.max(0.0),
                descent: m.descent.abs(),
                line_gap: m.line_gap.max(0.0),
            },
            None => LineMetrics {
                ascent: size * 0.9,
                descent: size * 0.25,
                line_gap: 0.0,
            },
        }
    }

    /// Width of `text` at `size`, including kerning and fallback.
    pub fn measure(&self, font: FontId, size: f32, text: &str) -> f32 {
        self.shape(font, size, text, 0.0)
            .last()
            .map(|g| g.x + g.advance)
            .unwrap_or(0.0)
    }

    /// Find a face that has `ch`: the primary face first, then fallbacks.
    fn lookup(&self, primary: usize, ch: char) -> (usize, u16) {
        let g = self.faces[primary].lookup_glyph_index(ch);
        if g != 0 {
            return (primary, g);
        }
        for &f in &FALLBACK_ORDER {
            if f == primary {
                continue;
            }
            let g = self.faces[f].lookup_glyph_index(ch);
            if g != 0 {
                return (f, g);
            }
        }
        (primary, 0)
    }

    /// Shape a run: positions for each char, with per-char font fallback
    /// when the primary face lacks the glyph. `letter_spacing` is added
    /// after every glyph.
    pub fn shape(&self, font: FontId, size: f32, text: &str, letter_spacing: f32) -> Vec<ShapedGlyph> {
        let primary = font.0.min(self.faces.len() - 1);
        let mut out = Vec::with_capacity(text.len());
        let mut x = 0.0f32;
        let mut prev: Option<(usize, u16)> = None;
        for (byte_index, ch) in text.char_indices() {
            if is_zero_width(ch) {
                out.push(ShapedGlyph {
                    font: FontId(primary),
                    glyph: ZERO_WIDTH_GLYPH,
                    x,
                    advance: 0.0,
                    byte_index,
                });
                prev = None;
                continue;
            }
            if ch == '\t' {
                let space = self.faces[primary].lookup_glyph_index(' ');
                let adv = self.faces[primary].metrics_indexed(space, size).advance_width * 4.0;
                out.push(ShapedGlyph {
                    font: FontId(primary),
                    glyph: ZERO_WIDTH_GLYPH,
                    x,
                    advance: adv + letter_spacing,
                    byte_index,
                });
                x += adv + letter_spacing;
                prev = None;
                continue;
            }
            let (face_idx, glyph) = self.lookup(primary, ch);
            let face = &self.faces[face_idx];
            if let Some((pf, pg)) = prev {
                if pf == face_idx {
                    if let Some(k) = face.horizontal_kern_indexed(pg, glyph, size) {
                        x += k;
                    }
                }
            }
            let adv = face.metrics_indexed(glyph, size).advance_width + letter_spacing;
            out.push(ShapedGlyph {
                font: FontId(face_idx),
                glyph,
                x,
                advance: adv,
                byte_index,
            });
            x += adv;
            prev = Some((face_idx, glyph));
        }
        out
    }

    /// Rasterize (cached) a glyph at a given size.
    pub fn rasterize(&self, font: FontId, glyph: u16, size: f32) -> Rc<GlyphBitmap> {
        let face_idx = font.0.min(self.faces.len() - 1);
        let key = (face_idx, glyph, (size * 4.0).round().max(0.0) as u32);
        if let Some(b) = self.cache.borrow().get(&key) {
            return Rc::clone(b);
        }
        let bitmap = if glyph == ZERO_WIDTH_GLYPH || size <= 0.0 {
            GlyphBitmap {
                width: 0,
                height: 0,
                left: 0,
                top: 0,
                coverage: Vec::new(),
            }
        } else {
            let (m, coverage) = self.faces[face_idx].rasterize_indexed(glyph, size);
            GlyphBitmap {
                width: m.width as u32,
                height: m.height as u32,
                left: m.xmin,
                top: m.ymin + m.height as i32,
                coverage,
            }
        };
        let rc = Rc::new(bitmap);
        self.cache.borrow_mut().insert(key, Rc::clone(&rc));
        rc
    }

    /// Advance width of a single char (used for tab stops / list markers).
    pub fn char_advance(&self, font: FontId, size: f32, ch: char) -> f32 {
        if is_zero_width(ch) {
            return 0.0;
        }
        let primary = font.0.min(self.faces.len() - 1);
        if ch == '\t' {
            let space = self.faces[primary].lookup_glyph_index(' ');
            return self.faces[primary].metrics_indexed(space, size).advance_width * 4.0;
        }
        let (face_idx, glyph) = self.lookup(primary, ch);
        self.faces[face_idx].metrics_indexed(glyph, size).advance_width
    }

    pub fn face_name(&self, font: FontId) -> &str {
        &self.names[font.0.min(self.names.len() - 1)]
    }

    /// Number of loaded faces.
    pub fn face_count(&self) -> usize {
        self.faces.len()
    }

    /// Whether `font` has a real glyph for `ch` (no fallback considered).
    pub fn has_glyph(&self, font: FontId, ch: char) -> bool {
        self.face(font).lookup_glyph_index(ch) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_and_measures() {
        let fc = FontContext::new();
        assert_eq!(fc.face_count(), 7);
        let f = fc.select(Family::Sans, 400, false);
        assert_eq!(f, FontId(SANS));
        assert_eq!(fc.select(Family::Sans, 700, true), FontId(SANS_BOLD_ITALIC));
        assert_eq!(fc.select(Family::Serif, 700, false), FontId(SERIF_BOLD));
        assert_eq!(fc.select(Family::Mono, 700, true), FontId(MONO));
        let w = fc.measure(f, 16.0, "Hello");
        assert!(w > 20.0 && w < 80.0, "width {w}");
        assert!(fc.measure(f, 16.0, "") == 0.0);
        let m = fc.metrics(f, 16.0);
        assert!(m.ascent > 10.0 && m.descent > 2.0);
        assert_eq!(fc.face_name(f), "DejaVu Sans");
        // Bold should be wider.
        assert!(fc.measure(fc.select(Family::Sans, 700, false), 16.0, "Hello") > w);
    }

    #[test]
    fn shaping_and_fallback() {
        let fc = FontContext::new();
        let f = fc.select(Family::Sans, 400, false);
        let glyphs = fc.shape(f, 16.0, "a\u{200B}b\t漢", 0.0);
        assert_eq!(glyphs.len(), 5);
        assert_eq!(glyphs[1].glyph, ZERO_WIDTH_GLYPH);
        assert_eq!(glyphs[1].advance, 0.0);
        assert_eq!(glyphs[2].byte_index, 1 + '\u{200B}'.len_utf8());
        assert!(glyphs[3].advance > 0.0); // tab
        // CJK is missing from every bundled face -> glyph 0 of primary
        assert_eq!(glyphs[4].glyph, 0);
        assert_eq!(glyphs[4].font, f);
        // Letter spacing adds up
        let w0 = fc.measure(f, 16.0, "abc");
        let spaced = fc.shape(f, 16.0, "abc", 2.0);
        let w1 = spaced.last().map(|g| g.x + g.advance).unwrap();
        assert!((w1 - (w0 + 6.0)).abs() < 0.01);
        // A char only in Serif? DejaVu Sans is very complete; just make sure
        // fallback picks a face that has the glyph when primary is Mono.
        let mono = fc.select(Family::Mono, 400, false);
        let g = fc.shape(mono, 16.0, "☃", 0.0);
        if !fc.has_glyph(mono, '☃') {
            assert_ne!(g[0].font, mono);
        }
    }

    #[test]
    fn rasterize_cached() {
        let fc = FontContext::new();
        let f = fc.select(Family::Sans, 400, false);
        let g = fc.shape(f, 20.0, "A", 0.0)[0];
        let a = fc.rasterize(g.font, g.glyph, 20.0);
        let b = fc.rasterize(g.font, g.glyph, 20.0);
        assert!(Rc::ptr_eq(&a, &b));
        assert!(a.width > 5 && a.height > 10);
        assert!(a.top > 0);
        assert!(a.coverage.iter().any(|&c| c == 255));
        let z = fc.rasterize(f, ZERO_WIDTH_GLYPH, 20.0);
        assert!(z.is_empty());
    }
}

// ------------------------------------------------------------------ v0.2 API
// Web fonts (@font-face), system fonts and family matching. Implemented in
// `font_ext.rs`.

/// A font face registered by name (web font or system font).
#[derive(Debug, Clone, PartialEq)]
pub struct FaceInfo {
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    pub id: FontId,
}

impl FontContext {
    /// Register a font from raw bytes (TTF/OTF, WOFF, WOFF2) under
    /// `family` with the given weight/style (from the `@font-face`
    /// descriptors). Returns None when the data cannot be decoded.
    pub fn add_font_data(&self, family: &str, weight: u16, italic: bool, data: &[u8]) -> Option<FontId> {
        let _ = (family, weight, italic, data);
        unimplemented!()
    }

    /// Scan the platform font directories (Windows: %WINDIR%\\Fonts and the
    /// per-user fonts folder; macOS: /System/Library/Fonts, /Library/Fonts,
    /// ~/Library/Fonts; Linux: /usr/share/fonts, /usr/local/share/fonts,
    /// ~/.fonts, ~/.local/share/fonts) and register TTF/OTF/TTC faces by
    /// their family name (parsed from the `name` table) lazily: only the
    /// name tables are read up-front; glyph data is loaded on first use.
    /// Returns the number of faces found.
    pub fn load_system_fonts(&self) -> usize {
        unimplemented!()
    }

    /// Pick the best face for a CSS family list: the first family (case
    /// insensitive; generic names `sans-serif`, `serif`, `monospace`,
    /// `system-ui`, `cursive`, `fantasy`, `emoji` map to platform defaults
    /// or the bundled faces) that has a registered face, choosing the
    /// closest weight (CSS font matching algorithm) and style (synthetic
    /// oblique is left to the caller via `needs_synthetic_italic`). Falls
    /// back to `select(generic, weight, italic)`.
    pub fn select_family(&self, families: &[String], generic: Family, weight: u16, italic: bool) -> FontId {
        let _ = (families, generic, weight, italic);
        unimplemented!()
    }

    /// True when `font` is an upright face chosen for an italic request.
    pub fn needs_synthetic_italic(&self, font: FontId, italic: bool) -> bool {
        let _ = (font, italic);
        unimplemented!()
    }

    /// True when `font` is a non-bold face chosen for a bold request
    /// (caller may embolden synthetically).
    pub fn needs_synthetic_bold(&self, font: FontId, weight: u16) -> bool {
        let _ = (font, weight);
        unimplemented!()
    }

    /// Registered faces (web + system + bundled).
    pub fn faces(&self) -> Vec<FaceInfo> {
        unimplemented!()
    }

    /// Does any registered face provide `family`?
    pub fn has_family(&self, family: &str) -> bool {
        let _ = family;
        unimplemented!()
    }

    /// Unicode-aware fallback used by `shape`: find a face containing `ch`,
    /// preferring faces of the same generic family, then any registered
    /// face (so system CJK/emoji/symbol fonts are used when available).
    pub fn fallback_for_char(&self, ch: char, prefer: FontId) -> Option<FontId> {
        let _ = (ch, prefer);
        unimplemented!()
    }
}
