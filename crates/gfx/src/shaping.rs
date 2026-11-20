//! OpenType shaping of text runs with rustybuzz (a pure-Rust HarfBuzz
//! port): GSUB substitutions (ligatures — including the icon ligatures of
//! Material Icons / Symbols —, contextual forms of Arabic, conjuncts of
//! Indic scripts) and GPOS positioning (kerning, mark attachment).
//!
//! Results are cached per (face, text, features) in font units, so they
//! serve every font size.

use crate::font::{FontContext, FontId};
use std::cell::RefCell;
use crate::fxhash::FxHashMap as HashMap;
use std::rc::Rc;

/// A font's decoration line metrics in px: (position upwards from the
/// baseline, thickness).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DecorationMetrics {
    pub underline: Option<(f32, f32)>,
    pub strikeout: Option<(f32, f32)>,
}

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
    /// Shaped runs by (face, features and direction key), then text.
    cache: RefCell<HashMap<(usize, u8), HashMap<Box<str>, Rc<UnitRun>>>>,
    /// Runs in `cache`.
    cache_len: std::cell::Cell<usize>,
    /// Faces that are another face shaped with OpenType features (CSS
    /// `font-feature-settings` / `font-variant-*`): face → (base, features).
    pub(crate) variants: RefCell<HashMap<usize, (usize, Rc<[FontFeature]>)>>,
    /// (base face, features, fallback chain) → variant face.
    pub(crate) variant_ids: RefCell<HashMap<(usize, Rc<[FontFeature]>, Option<Rc<[usize]>>), usize>>,
    /// Variant face → the faces of the later families of its `font-family`
    /// list, for characters outside its family's unicode-ranges.
    pub(crate) chains: RefCell<HashMap<usize, Rc<[usize]>>>,
    /// Default features of a face (`@font-face { font-feature-settings }`).
    pub(crate) face_features: RefCell<HashMap<usize, Rc<[FontFeature]>>>,
    /// Face → the printable ASCII characters it renders itself (bit per
    /// code point), for [`FontContext::fallback_faces`].
    pub(crate) ascii_cover: RefCell<HashMap<usize, u128>>,
    /// (face, tag) → does the face's GSUB have that feature?
    gsub_features: RefCell<HashMap<(usize, [u8; 4]), bool>>,
}

/// The `ascent-override`, `descent-override` and `line-gap-override`
/// descriptors of an `@font-face` rule (CSS Fonts 4 §4.11), as fractions
/// of the used font size; `None` keeps the font's own metric.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MetricOverrides {
    pub ascent: Option<f32>,
    pub descent: Option<f32>,
    pub line_gap: Option<f32>,
}

impl MetricOverrides {
    pub fn is_empty(&self) -> bool {
        self.ascent.is_none() && self.descent.is_none() && self.line_gap.is_none()
    }
}

/// An OpenType feature setting: tag and value (0 = off, 1 = on, or an
/// alternate index).
pub type FontFeature = ([u8; 4], u32);

/// A pseudo feature carrying an OpenType language system tag (the value,
/// big-endian) for the shaper.
pub const LANGUAGE_TAG: &[u8; 4] = b"\0lng";

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
    /// The face a feature variant shapes with (the face itself otherwise).
    pub(crate) fn variant_base(&self, idx: usize) -> usize {
        let variants = self.shaping.variants.borrow();
        if variants.is_empty() {
            return idx;
        }
        variants.get(&idx).map(|v| v.0).unwrap_or(idx)
    }

    /// Does `font` substitute glyphs for the OpenType feature `tag` (e.g.
    /// `smcp`: real small capitals instead of synthesized ones)?
    pub fn has_feature(&self, font: FontId, tag: [u8; 4]) -> bool {
        let base = self.variant_base(font.0);
        if let Some(&v) = self.shaping.gsub_features.borrow().get(&(base, tag)) {
            return v;
        }
        let v = self
            .shaping_data(base)
            .and_then(|(b, _, _)| {
                let face = rustybuzz::ttf_parser::Face::parse(&b, 0).ok()?;
                let gsub = face.tables().gsub?;
                let want = rustybuzz::ttf_parser::Tag::from_bytes(&tag);
                Some(gsub.features.into_iter().any(|f| f.tag == want))
            })
            .unwrap_or(false);
        self.shaping.gsub_features.borrow_mut().insert((base, tag), v);
        v
    }

    /// Default OpenType features of a face (the `font-feature-settings`
    /// descriptor of its `@font-face` rule).
    pub fn set_face_features(&self, font: FontId, features: &[FontFeature]) {
        self.shaping.face_features.borrow_mut().insert(font.0, Rc::from(features));
    }

    /// The `@font-face` metric overrides of a face (of its base face for
    /// a feature variant).
    pub(crate) fn metric_overrides(&self, idx: usize) -> Option<MetricOverrides> {
        let base = self.variant_base(idx);
        let faces = self.faces.borrow();
        faces.get(base).map(|s| s.metric_overrides).filter(|o| !o.is_empty())
    }

    /// `font` shaped with OpenType `features` (an unbound copy of the
    /// face; the same id for the same request). Features apply to the runs
    /// this face renders, not to fallback faces.
    pub fn with_features(&self, font: FontId, features: &[FontFeature]) -> FontId {
        if features.is_empty() {
            return font;
        }
        let base = self.variant_base(font.0);
        // The face's own defaults come first; the element's settings win.
        let mut merged: Vec<FontFeature> = self.shaping.face_features.borrow().get(&base).map(|f| f.to_vec()).unwrap_or_default();
        for f in features {
            merged.retain(|g| g.0 != f.0);
            merged.push(*f);
        }
        let chain = self.shaping.chains.borrow().get(&font.0).cloned();
        self.variant(base, Rc::from(merged), chain).unwrap_or(font)
    }

    /// `font` falling back to `chain` (the faces chosen for the later
    /// families of a `font-family` list) for characters outside the
    /// unicode-ranges of its family (CSS Fonts 4 §5.2: such characters
    /// use the next family).
    pub(crate) fn with_chain(&self, font: FontId, chain: Vec<usize>) -> FontId {
        let base = self.variant_base(font.0);
        let feats = self.shaping.variants.borrow().get(&font.0).map(|v| v.1.clone());
        let feats = feats.or_else(|| self.shaping.face_features.borrow().get(&base).cloned()).unwrap_or_else(|| Rc::from(Vec::new()));
        self.variant(base, feats, Some(Rc::from(chain))).unwrap_or(font)
    }

    /// An unbound copy of face `base` shaped with `feats` and falling back
    /// to `chain` (the same id for the same request).
    fn variant(&self, base: usize, feats: Rc<[FontFeature]>, chain: Option<Rc<[usize]>>) -> Option<FontId> {
        let key = (base, feats.clone(), chain.clone());
        if let Some(&id) = self.shaping.variant_ids.borrow().get(&key) {
            return Some(FontId(id));
        }
        // Copies share the parsed font.
        let _ = self.try_face(base);
        let idx = {
            let mut faces = self.faces.borrow_mut();
            let slot = faces.get(base)?;
            let copy = crate::font::FaceSlot {
                name: slot.name.clone(),
                family: slot.family.clone(),
                family_lower: slot.family_lower.clone(),
                weight: slot.weight,
                italic: slot.italic,
                stretch: slot.stretch,
                generic: slot.generic,
                unicode_range: slot.unicode_range.clone(),
                metric_overrides: slot.metric_overrides,
                source: match &slot.source {
                    crate::font::FaceSource::Bytes(b) => crate::font::FaceSource::Bytes(Rc::clone(b)),
                    crate::font::FaceSource::Path(p, i) => crate::font::FaceSource::Path(p.clone(), *i),
                },
                font: RefCell::new(slot.font.borrow().clone()),
                failed: std::cell::Cell::new(slot.failed.get()),
            };
            faces.push(copy);
            faces.len() - 1
        };
        self.shaping.variants.borrow_mut().insert(idx, (base, feats));
        if let Some(chain) = chain {
            self.shaping.chains.borrow_mut().insert(idx, chain);
        }
        self.shaping.variant_ids.borrow_mut().insert(key, idx);
        Some(FontId(idx))
    }

    /// SFNT bytes of a face and whether it has OpenType layout tables.
    fn shaping_data(&self, idx: usize) -> Option<(Rc<Vec<u8>>, bool, bool)> {
        let idx = self.variant_base(idx);
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

    /// The font's underline and strikeout metrics at `size`, in px:
    /// `(underline position, underline thickness, strikeout position,
    /// strikeout thickness)`, positions measured upwards from the baseline
    /// (an underline below the baseline has a negative position).
    pub fn decoration_metrics(&self, font: crate::FontId, size: f32, variations: &[([u8; 4], f32)]) -> DecorationMetrics {
        let mut m = DecorationMetrics::default();
        let Some(b) = self.face_bytes(font.0) else { return m };
        let Ok(mut face) = rustybuzz::ttf_parser::Face::parse(&b, 0) else { return m };
        for (tag, v) in variations {
            let _ = face.set_variation(rustybuzz::ttf_parser::Tag::from_bytes(tag), *v);
        }
        let upem = face.units_per_em() as f32;
        if upem <= 0.0 {
            return m;
        }
        let k = size / upem;
        if let Some(u) = face.underline_metrics() {
            m.underline = Some((u.position as f32 * k, u.thickness as f32 * k));
        }
        if let Some(s) = face.strikeout_metrics() {
            m.strikeout = Some((s.position as f32 * k, s.thickness as f32 * k));
        }
        m
    }

    /// `ch` (advance of "0") and `ex` (x-height) of the font chosen for a
    /// family list, in em (CSS Values 4 §6.1.1; 0.5em when unknown).
    /// Also `ic` (advance of U+6C34), 1em when the font lacks it.
    pub fn unit_metrics(&self, families: &[String], generic: crate::Family, weight: u16, italic: bool) -> (f32, f32, f32) {
        let font = self.first_available(families, generic, weight, italic);
        let Some(b) = self.face_bytes(font.0) else { return (0.5, 0.5, 1.0) };
        let Ok(face) = rustybuzz::ttf_parser::Face::parse(&b, 0) else { return (0.5, 0.5, 1.0) };
        let upem = face.units_per_em() as f32;
        if upem <= 0.0 {
            return (0.5, 0.5, 1.0);
        }
        // A mapped glyph counts even with a zero advance.
        let adv = |c: char| face.glyph_index(c).map(|g| face.glyph_hor_advance(g).unwrap_or(0) as f32 / upem);
        let ch = adv('0').unwrap_or(0.5);
        let ic = adv('\u{6C34}').unwrap_or(1.0);
        let ex = face
            .x_height()
            .map(|v| v as f32)
            .filter(|v| *v > 0.0)
            .or_else(|| {
                let g = face.glyph_index('x')?;
                face.glyph_bounding_box(g).map(|r| r.y_max as f32)
            })
            .map(|xh| xh / upem)
            .unwrap_or(0.5);
        (ch, ex, ic)
    }

    /// The first available font of a `font-family` list: the first one
    /// that has a space glyph.
    pub fn first_available(&self, families: &[String], generic: crate::Family, weight: u16, italic: bool) -> FontId {
        for i in 0..families.len() {
            let f = self.select_family(&families[i..], generic, weight, italic);
            let has_space = self
                .face_bytes(f.0)
                .and_then(|b| rustybuzz::ttf_parser::Face::parse(&b, 0).ok().map(|face| face.glyph_index(' ').is_some()))
                .unwrap_or(true);
            if has_space {
                return f;
            }
        }
        self.select(generic, weight, italic)
    }

    /// The ascent, descent and line gap (in em) of the first available
    /// font of a `font-family` list (for `line-height: normal` in style
    /// computations: the `lh` unit, percentages of the line height).
    pub fn unit_line_metrics(&self, families: &[String], generic: crate::Family, weight: u16, italic: bool) -> crate::LineMetrics {
        let font = if families.is_empty() { self.select(generic, weight, italic) } else { self.select_family(families, generic, weight, italic) };
        self.line_metrics(font, 1.0)
    }

    /// Cap height of the primary face in em (OS/2 sCapHeight, else the
    /// height of `H`).
    pub fn cap_height_em(&self, families: &[String], generic: crate::Family, weight: u16, italic: bool) -> Option<f32> {
        let font = if families.is_empty() { self.select(generic, weight, italic) } else { self.select_family(families, generic, weight, italic) };
        let b = self.face_bytes(font.0)?;
        let face = rustybuzz::ttf_parser::Face::parse(&b, 0).ok()?;
        let upem = face.units_per_em() as f32;
        let cap = face.capital_height().map(|v| v as f32).filter(|v| *v > 0.0).or_else(|| {
            let g = face.glyph_index('H')?;
            face.glyph_bounding_box(g).map(|r| r.y_max as f32)
        })?;
        (upem > 0.0).then(|| cap / upem)
    }

    /// The face's SFNT bytes.
    pub(crate) fn face_bytes(&self, idx: usize) -> Option<Rc<Vec<u8>>> {
        self.shaping_data(idx).map(|(b, _, _)| b)
    }

    /// Shape `text` with face `idx` (cached). None when the face cannot be
    /// parsed by the shaper.
    pub(crate) fn shape_units(&self, idx: usize, text: &str, features: ShapeFeatures, rtl: Option<bool>) -> Option<Rc<UnitRun>> {
        let dir_key = match rtl {
            None => 0u8,
            Some(false) => 4,
            Some(true) => 8,
        };
        let key = (idx, features.key() | dir_key);
        if let Some(r) = self.shaping.cache.borrow().get(&key).and_then(|m| m.get(text)) {
            return Some(Rc::clone(r));
        }
        let (data, _, _) = self.shaping_data(idx)?;
        let face = rustybuzz::Face::from_slice(&data, 0)?;
        let mut buf = rustybuzz::UnicodeBuffer::new();
        buf.push_str(text);
        buf.guess_segment_properties();
        if let Some(r) = rtl {
            buf.set_direction(if r { rustybuzz::Direction::RightToLeft } else { rustybuzz::Direction::LeftToRight });
        }
        let mut feats = Vec::new();
        let extra: Option<Rc<[FontFeature]>> = self.shaping.variants.borrow().get(&idx).map(|v| v.1.clone()).or_else(|| self.shaping.face_features.borrow().get(&idx).cloned());
        if let Some(extra) = extra {
            for (tag, value) in extra.iter() {
                if tag == LANGUAGE_TAG {
                    // An OpenType language system (`font-language-override`);
                    // registered tags are upper case, others match nothing.
                    if value.to_be_bytes().iter().any(|b| b.is_ascii_lowercase()) {
                        continue;
                    }
                    let ot: String = value.to_be_bytes().iter().filter(|b| **b != b' ' && **b != 0).map(|b| b.to_ascii_lowercase() as char).collect();
                    if let Ok(lang) = format!("x-hbot{ot}").parse::<rustybuzz::Language>() {
                        buf.set_language(lang);
                    }
                    continue;
                }
                feats.push(rustybuzz::Feature::new(rustybuzz::ttf_parser::Tag::from_bytes(tag), *value, ..));
            }
        }
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
        if self.shaping.cache_len.get() > 60_000 {
            cache.clear();
            self.shaping.cache_len.set(0);
        }
        cache.entry(key).or_default().insert(Box::from(text), Rc::clone(&run));
        self.shaping.cache_len.set(self.shaping.cache_len.get() + 1);
        Some(run)
    }
}
