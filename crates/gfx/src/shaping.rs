//! OpenType shaping of text runs with rustybuzz (a pure-Rust HarfBuzz
//! port): GSUB substitutions (ligatures — including the icon ligatures of
//! Material Icons / Symbols —, contextual forms of Arabic, conjuncts of
//! Indic scripts) and GPOS positioning (kerning, mark attachment).
//!
//! Results are cached per (face, text, features) in font units, so they
//! serve every font size.

use crate::font::FontContext;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// One shaped glyph in font units.
#[derive(Debug, Clone, Copy)]
pub(crate) struct UnitGlyph {
    pub glyph: u16,
    /// Byte offset of the cluster's first char in the run.
    pub cluster: u32,
    pub x_advance: i32,
    pub x_offset: i32,
    pub y_offset: i32,
}

/// A shaped run (glyphs in visual order) and the face's units per em.
#[derive(Debug)]
pub(crate) struct UnitRun {
    pub glyphs: Vec<UnitGlyph>,
    pub upem: f32,
}

/// Per-face data needed by the shaper.
#[derive(Default)]
pub(crate) struct ShapingState {
    /// Face index → SFNT bytes (None: unavailable) and whether the face
    /// has layout tables worth shaping with (GSUB / GPOS / kern).
    data: RefCell<HashMap<usize, Option<(Rc<Vec<u8>>, bool, bool)>>>,
    cache: RefCell<HashMap<(usize, Box<str>, u8), Rc<UnitRun>>>,
}

/// Font features requested for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShapeFeatures {
    /// Disable common/contextual ligatures (`letter-spacing` other than 0,
    /// `font-variant-ligatures: none`).
    pub no_ligatures: bool,
    /// Disable kerning (`font-kerning: none`).
    pub no_kerning: bool,
}

impl ShapeFeatures {
    fn key(self) -> u8 {
        (self.no_ligatures as u8) | ((self.no_kerning as u8) << 1)
    }
}

impl FontContext {
    /// SFNT bytes of a face and whether it has OpenType layout tables.
    fn shaping_data(&self, idx: usize) -> Option<(Rc<Vec<u8>>, bool, bool)> {
        if let Some(d) = self.shaping.data.borrow().get(&idx) {
            return d.clone();
        }
        let bytes = {
            let faces = self.faces.borrow();
            faces.get(idx).and_then(|slot| match &slot.source {
                crate::font::FaceSource::Bytes(b) => Some(Rc::clone(b)),
                crate::font::FaceSource::Path(p, index) => {
                    std::fs::read(p).ok().and_then(|data| crate::font_ext::sfnt::extract_face(&data, *index)).map(Rc::new)
                }
            })
        };
        let entry = bytes.and_then(|b| {
            let face = rustybuzz::ttf_parser::Face::parse(&b, 0).ok()?;
            let t = face.tables();
            let layout = t.gsub.is_some() || t.gpos.is_some() || t.kern.is_some();
            let color = t.colr.is_some() || t.cbdt.is_some() || t.sbix.is_some();
            Some((Rc::clone(&b), layout, color))
        });
        self.shaping.data.borrow_mut().insert(idx, entry.clone());
        entry
    }

    /// Does this face have OpenType layout tables?
    pub(crate) fn wants_shaping(&self, idx: usize) -> bool {
        self.shaping_data(idx).map(|(_, l, _)| l).unwrap_or(false)
    }

    /// Does this face have color glyphs (COLR / CBDT / sbix)?
    pub(crate) fn has_color_tables(&self, idx: usize) -> bool {
        self.shaping_data(idx).map(|(_, _, c)| c).unwrap_or(false)
    }

    /// The face's SFNT bytes.
    pub(crate) fn face_bytes(&self, idx: usize) -> Option<Rc<Vec<u8>>> {
        self.shaping_data(idx).map(|(b, _, _)| b)
    }

    /// Shape `text` with face `idx` (cached). None when the face cannot be
    /// parsed by the shaper.
    pub(crate) fn shape_units(&self, idx: usize, text: &str, features: ShapeFeatures) -> Option<Rc<UnitRun>> {
        let key = (idx, Box::<str>::from(text), features.key());
        if let Some(r) = self.shaping.cache.borrow().get(&key) {
            return Some(Rc::clone(r));
        }
        let (data, _, _) = self.shaping_data(idx)?;
        let face = rustybuzz::Face::from_slice(&data, 0)?;
        let mut buf = rustybuzz::UnicodeBuffer::new();
        buf.push_str(text);
        buf.guess_segment_properties();
        let mut feats = Vec::new();
        if features.no_ligatures {
            for tag in [b"liga", b"clig", b"dlig", b"hlig", b"calt"] {
                feats.push(rustybuzz::Feature::new(rustybuzz::ttf_parser::Tag::from_bytes(tag), 0, ..));
            }
        }
        if features.no_kerning {
            feats.push(rustybuzz::Feature::new(rustybuzz::ttf_parser::Tag::from_bytes(b"kern"), 0, ..));
        }
        let out = rustybuzz::shape(&face, &feats, buf);
        let glyphs = out
            .glyph_infos()
            .iter()
            .zip(out.glyph_positions())
            .map(|(i, p)| UnitGlyph { glyph: i.glyph_id.min(u16::MAX as u32 - 1) as u16, cluster: i.cluster, x_advance: p.x_advance, x_offset: p.x_offset, y_offset: p.y_offset })
            .collect();
        let run = Rc::new(UnitRun { glyphs, upem: face.units_per_em() as f32 });
        let mut cache = self.shaping.cache.borrow_mut();
        if cache.len() > 60_000 {
            cache.clear();
        }
        cache.insert(key, Rc::clone(&run));
        Some(run)
    }
}
