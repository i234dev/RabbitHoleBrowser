//! Fonts: bundled DejaVu faces (so rendering is identical on every OS),
//! web fonts (`@font-face`) and system fonts registered by family name,
//! glyph shaping (advances + kerning, per-character font fallback) and a
//! glyph bitmap cache.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
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
    /// Horizontal offset from the start of the run (pen position).
    pub x: f32,
    pub advance: f32,
    /// Offset of the glyph from its pen position (mark attachment,
    /// GPOS adjustments); `dy` is upwards.
    pub dx: f32,
    pub dy: f32,
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

pub(crate) const SANS: usize = 0;
pub(crate) const SANS_BOLD: usize = 1;
pub(crate) const SANS_ITALIC: usize = 2;
pub(crate) const SANS_BOLD_ITALIC: usize = 3;
pub(crate) const MONO: usize = 4;
pub(crate) const SERIF: usize = 5;
pub(crate) const SERIF_BOLD: usize = 6;
pub(crate) const BUNDLED_COUNT: usize = 7;

/// Fallback search order among the bundled faces when a face lacks a glyph.
const FALLBACK_ORDER: [usize; 7] = [SANS, SERIF, MONO, SANS_BOLD, SANS_ITALIC, SANS_BOLD_ITALIC, SERIF_BOLD];

/// Where a face's bytes come from.
pub(crate) enum FaceSource {
    /// In-memory SFNT data (bundled or web font, already decoded).
    Bytes(Rc<Vec<u8>>),
    /// A system font file (parsed on first use); `index` selects a face in
    /// a collection.
    Path(PathBuf, u32),
}

/// One registered face.
pub(crate) struct FaceSlot {
    pub name: String,
    pub family: String,
    pub family_lower: String,
    pub weight: u16,
    pub italic: bool,
    /// `font-stretch` range in percent (min, max); 100 for host faces.
    pub stretch: (u16, u16),
    /// Generic family for bundled faces (used for fallback preference).
    pub generic: Option<Family>,
    pub source: FaceSource,
    pub font: RefCell<Option<Rc<fontdue::Font>>>,
    pub failed: Cell<bool>,
}

/// Retained web-font resources per shared store, independent of live document
/// count. This does not bound decoder scratch, parsed fonts or glyph caches.
#[derive(Debug, Clone, Copy)]
pub struct FontLimits {
    pub max_web_bindings: usize,
    pub max_sfnt_bytes: usize,
}

impl Default for FontLimits {
    fn default() -> Self {
        Self { max_web_bindings: 4096, max_sfnt_bytes: 128 * 1024 * 1024 }
    }
}

pub struct FontContext {
    pub(crate) faces: Rc<RefCell<Vec<FaceSlot>>>,
    /// Lower-cased family name → face indices.
    pub(crate) by_family: Rc<RefCell<HashMap<String, Vec<usize>>>>,
    base_families: Rc<RefCell<HashMap<String, Vec<usize>>>>,
    base_generation: Rc<Cell<u64>>,
    pub(crate) byte_faces: Rc<RefCell<HashMap<u64, Vec<usize>>>>,
    pub(crate) limits: FontLimits,
    pub(crate) web_bindings: Rc<Cell<usize>>,
    pub(crate) web_sfnt_bytes: Rc<Cell<usize>>,
    seen_base_generation: Cell<u64>,
    local_generation: Cell<u64>,
    /// (face, glyph, size×4, subpixel quarter) → bitmap
    cache: Rc<RefCell<HashMap<(usize, u16, u32, u8), Rc<GlyphBitmap>>>>,
    /// (primary face, char) → face that has the char (None = nowhere)
    fallback_cache: RefCell<HashMap<(usize, char), Option<usize>>>,
    pub(crate) system_loaded: Rc<Cell<bool>>,
    /// OpenType shaping state (face data, shaped-run cache).
    pub(crate) shaping: Rc<crate::shaping::ShapingState>,
    /// (face, glyph, size×4) → color rendering (None: not a color glyph).
    pub(crate) color_cache: Rc<RefCell<HashMap<(usize, u16, u32), Option<Rc<crate::color_glyph::ColorGlyph>>>>>,
}

impl Default for FontContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Per character: does it belong to a right-to-left run? Strong Hebrew /
/// Arabic / Syriac / Thaana / NKo... letters are RTL; neutrals (spaces,
/// punctuation) between two RTL letters too. `None` when the text has no
/// RTL letter (the common case, checked on the UTF-8 lead bytes).
fn rtl_chars(text: &str) -> Option<Vec<bool>> {
    if !text.bytes().any(|b| b >= 0xD6) {
        return None;
    }
    // 0 neutral, 1 LTR, 2 RTL.
    let class = |ch: char| -> u8 {
        let c = ch as u32;
        if matches!(c, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF) {
            if ch.is_alphabetic() {
                2
            } else {
                0
            }
        } else if ch.is_alphanumeric() {
            1
        } else {
            0
        }
    };
    let classes: Vec<u8> = text.chars().map(class).collect();
    if !classes.contains(&2) {
        return None;
    }
    let n = classes.len();
    let mut out = vec![false; n];
    let mut prev = 0u8;
    for i in 0..n {
        out[i] = match classes[i] {
            2 => true,
            1 => false,
            _ => {
                let next = classes[i + 1..].iter().copied().find(|c| *c != 0).unwrap_or(0);
                prev == 2 && next == 2
            }
        };
        if classes[i] != 0 {
            prev = classes[i];
        }
    }
    Some(out)
}

/// Combining marks, variation selectors and joiners: they belong to the
/// preceding character's cluster.
fn is_mark_like(ch: char) -> bool {
    matches!(ch as u32,
        0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05C7 | 0x0610..=0x061A | 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06ED
        | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x200C | 0x200D | 0x20D0..=0x20FF | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F | 0xE0100..=0xE01EF)
}

/// Bidi_Mirroring_Glyph of common paired punctuation.
fn mirror_bracket(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '\u{AB}' => '\u{BB}',
        '\u{BB}' => '\u{AB}',
        '\u{2039}' => '\u{203A}',
        '\u{203A}' => '\u{2039}',
        c => c,
    }
}

fn is_zero_width(ch: char) -> bool {
    matches!(
        ch,
        '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' | '\u{00AD}' | '\u{2060}' | '\u{200E}' | '\u{200F}'
            | '\u{061C}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    ) // Other control characters are visible (CSS Text 3 §4.1.1).
        || matches!(ch, '\n' | '\r' | '\u{0C}' | '\0')
}

pub(crate) fn fontdue_settings() -> fontdue::FontSettings {
    fontdue::FontSettings {
        scale: 40.0,
        ..Default::default()
    }
}

impl FontContext {
    /// Load the bundled fonts (no system font scan; see
    /// [`FontContext::load_system_fonts`] / [`FontContext::with_system_fonts`]).
    pub fn new() -> FontContext {
        Self::with_limits(FontLimits::default())
    }

    /// Bundled fonts with explicit limits for subsequent web-font registration.
    /// System font paths and bundled data do not consume these quotas.
    pub fn with_limits(limits: FontLimits) -> FontContext {
        let sources: [(&str, &str, u16, bool, Family, &[u8]); BUNDLED_COUNT] = [
            ("DejaVu Sans", "DejaVu Sans", 400, false, Family::Sans, include_bytes!("../../../assets/fonts/DejaVuSans.ttf")),
            ("DejaVu Sans Bold", "DejaVu Sans", 700, false, Family::Sans, include_bytes!("../../../assets/fonts/DejaVuSans-Bold.ttf")),
            ("DejaVu Sans Oblique", "DejaVu Sans", 400, true, Family::Sans, include_bytes!("../../../assets/fonts/DejaVuSans-Oblique.ttf")),
            ("DejaVu Sans Bold Oblique", "DejaVu Sans", 700, true, Family::Sans, include_bytes!("../../../assets/fonts/DejaVuSans-BoldOblique.ttf")),
            ("DejaVu Sans Mono", "DejaVu Sans Mono", 400, false, Family::Mono, include_bytes!("../../../assets/fonts/DejaVuSansMono.ttf")),
            ("DejaVu Serif", "DejaVu Serif", 400, false, Family::Serif, include_bytes!("../../../assets/fonts/DejaVuSerif.ttf")),
            ("DejaVu Serif Bold", "DejaVu Serif", 700, false, Family::Serif, include_bytes!("../../../assets/fonts/DejaVuSerif-Bold.ttf")),
        ];
        let base_families = Rc::new(RefCell::new(HashMap::new()));
        let ctx = FontContext {
            faces: Rc::new(RefCell::new(Vec::with_capacity(BUNDLED_COUNT))),
            by_family: base_families.clone(),
            base_families,
            base_generation: Rc::new(Cell::new(0)),
            byte_faces: Rc::new(RefCell::new(HashMap::new())),
            limits,
            web_bindings: Rc::new(Cell::new(0)),
            web_sfnt_bytes: Rc::new(Cell::new(0)),
            seen_base_generation: Cell::new(0),
            local_generation: Cell::new(0),
            cache: Rc::new(RefCell::new(HashMap::new())),
            fallback_cache: RefCell::new(HashMap::new()),
            system_loaded: Rc::new(Cell::new(false)),
            shaping: Default::default(),
            color_cache: Rc::new(RefCell::new(HashMap::new())),
        };
        // Bundled faces are parsed on first use (`try_face`): fontdue builds
        // every glyph outline when parsing, and a page usually needs one or
        // two of the seven.
        for (name, family, weight, italic, generic, bytes) in sources {
            ctx.register(FaceSlot {
                name: name.to_string(),
                family: family.to_string(),
                family_lower: family.to_ascii_lowercase(),
                weight,
                italic,
                stretch: (100, 100),
                generic: Some(generic),
                source: FaceSource::Bytes(Rc::new(bytes.to_vec())),
                font: RefCell::new(None),
                failed: Cell::new(false),
            });
        }
        ctx
    }

    /// Bundled fonts plus every font installed on the system.
    pub fn with_system_fonts() -> FontContext {
        let ctx = FontContext::new();
        ctx.load_system_fonts();
        ctx
    }

    /// A fresh document view. Host/bundled registrations remain visible, but
    /// aliases belonging to this view (or its parent document) do not transfer.
    /// Font IDs and caches stay valid while any view retains the shared store.
    pub fn document_scope(&self) -> Self {
        Self {
            faces: self.faces.clone(),
            by_family: Rc::new(RefCell::new(HashMap::new())),
            base_families: self.base_families.clone(),
            base_generation: self.base_generation.clone(),
            byte_faces: self.byte_faces.clone(),
            limits: self.limits,
            web_bindings: self.web_bindings.clone(),
            web_sfnt_bytes: self.web_sfnt_bytes.clone(),
            seen_base_generation: Cell::new(self.base_generation.get()),
            local_generation: Cell::new(0),
            cache: self.cache.clone(),
            fallback_cache: RefCell::new(HashMap::new()),
            system_loaded: self.system_loaded.clone(),
            shaping: self.shaping.clone(),
            color_cache: self.color_cache.clone(),
        }
    }

    /// O(1) revision for caches owned by this view. Scope identity still
    /// belongs to the cache owner: equal revisions do not identify two views.
    pub fn selection_generation(&self) -> (u64, u64) {
        (self.base_generation.get(), self.local_generation.get())
    }

    pub(crate) fn host_scope(&self) -> Self {
        let mut host = self.document_scope();
        host.by_family = host.base_families.clone();
        host
    }

    pub(crate) fn bytes_key(bytes: &[u8]) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hasher);
        hasher.finish()
    }

    pub(crate) fn family_faces(&self, family: &str) -> Option<Vec<usize>> {
        self.by_family
            .borrow()
            .get(family)
            .cloned()
            .or_else(|| self.base_families.borrow().get(family).cloned())
    }

    /// Eligible faces only: historical documents must never enter fallback.
    pub(crate) fn visible_faces(&self) -> Vec<usize> {
        let mut ids: Vec<_> = self.base_families.borrow().values().flatten().copied().collect();
        ids.extend(self.by_family.borrow().values().flatten().copied());
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    pub(crate) fn bind_face(&self, idx: usize) {
        let key = self.faces.borrow()[idx].family_lower.clone();
        let mut families = self.by_family.borrow_mut();
        let ids = families.entry(key).or_default();
        if !ids.contains(&idx) {
            ids.push(idx);
            self.local_generation.set(self.local_generation.get().wrapping_add(1));
            self.fallback_cache.borrow_mut().clear();
            if Rc::ptr_eq(&self.by_family, &self.base_families) {
                self.base_generation.set(self.base_generation.get().wrapping_add(1));
            }
        }
    }

    /// Add a face to the store and bind it only in this view.
    pub(crate) fn register(&self, slot: FaceSlot) -> FontId {
        let bytes_key = match &slot.source {
            FaceSource::Bytes(bytes) => Some(Self::bytes_key(bytes)),
            FaceSource::Path(..) => None,
        };
        let idx = {
            let mut faces = self.faces.borrow_mut();
            let idx = faces.len();
            faces.push(slot);
            idx
        };
        if let Some(key) = bytes_key {
            self.byte_faces.borrow_mut().entry(key).or_default().push(idx);
        }
        self.bind_face(idx);
        FontId(idx)
    }

    /// Choose a bundled face for a generic family, CSS weight (100-900)
    /// and style.
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

    /// The parsed font for a face index, loading it on first use. Faces
    /// that fail to load (or out-of-range ids) resolve to the bundled sans.
    pub(crate) fn face(&self, idx: usize) -> Rc<fontdue::Font> {
        match self.try_face(idx) {
            Some(f) => f,
            None => self.try_face(SANS).expect("bundled sans"),
        }
    }

    /// Resolve a face index to one that can be loaded.
    pub(crate) fn resolve_index(&self, idx: usize) -> usize {
        if self.try_face(idx).is_some() {
            idx
        } else {
            SANS
        }
    }

    pub(crate) fn try_face(&self, idx: usize) -> Option<Rc<fontdue::Font>> {
        let faces = self.faces.borrow();
        let slot = faces.get(idx)?;
        if let Some(f) = slot.font.borrow().as_ref() {
            return Some(Rc::clone(f));
        }
        if slot.failed.get() {
            return None;
        }
        let parsed = match &slot.source {
            FaceSource::Bytes(b) => fontdue::Font::from_bytes(b.as_slice(), fontdue_settings()).ok(),
            FaceSource::Path(p, index) => std::fs::read(p).ok().and_then(|data| {
                let sfnt = crate::font_ext::sfnt::extract_face(&data, *index)?;
                fontdue::Font::from_bytes(sfnt.as_slice(), fontdue_settings()).ok()
            }),
        };
        match parsed {
            Some(f) => {
                let rc = Rc::new(f);
                *slot.font.borrow_mut() = Some(Rc::clone(&rc));
                Some(rc)
            }
            None => {
                slot.failed.set(true);
                None
            }
        }
    }

    pub fn metrics(&self, font: FontId, size: f32) -> LineMetrics {
        match self.face(font.0).horizontal_line_metrics(size) {
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
        let face = self.face(primary);
        let g = face.lookup_glyph_index(ch);
        if g != 0 {
            return (primary, g);
        }
        match self.fallback_for_char(ch, FontId(primary)) {
            Some(FontId(f)) => {
                let g = self.face(f).lookup_glyph_index(ch);
                (f, g)
            }
            None => (primary, 0),
        }
    }

    /// Unicode-aware fallback used by `shape`: find a face containing `ch`,
    /// preferring other styles of the same family, bundled faces of the
    /// same generic family, then script-specific well-known system fonts,
    /// then any registered face. Results are cached per (face, char).
    pub fn fallback_for_char(&self, ch: char, prefer: FontId) -> Option<FontId> {
        let primary = self.resolve_index(prefer.0);
        let generation = self.base_generation.get();
        if self.seen_base_generation.replace(generation) != generation {
            self.fallback_cache.borrow_mut().clear();
        }
        if let Some(r) = self.fallback_cache.borrow().get(&(primary, ch)) {
            return r.map(FontId);
        }
        let result = self.find_fallback(ch, primary);
        self.fallback_cache.borrow_mut().insert((primary, ch), result);
        result.map(FontId)
    }

    fn find_fallback(&self, ch: char, primary: usize) -> Option<usize> {
        // A face not loaded yet is first checked in its character map:
        // loading it (fontdue builds every glyph outline up front) costs far
        // more, and most candidates of a fallback search lack the character.
        // Only a face that maps it is loaded, which also confirms it is usable.
        let has = |idx: usize| -> bool {
            let loaded = self.faces.borrow().get(idx).and_then(|s| s.font.borrow().clone());
            if let Some(f) = loaded {
                return f.lookup_glyph_index(ch) != 0;
            }
            if self.faces.borrow().get(idx).is_none_or(|s| s.failed.get()) {
                return false;
            }
            let mapped = self.face_bytes(idx).is_some_and(|b| {
                rustybuzz::ttf_parser::Face::parse(&b, 0).ok().and_then(|f| f.glyph_index(ch)).is_some_and(|g| g.0 != 0)
            });
            mapped && self.try_face(idx).is_some_and(|f| f.lookup_glyph_index(ch) != 0)
        };
        if has(primary) {
            return Some(primary);
        }
        let (family_lower, generic, prim_italic, prim_weight) = {
            let faces = self.faces.borrow();
            let s = &faces[primary];
            (s.family_lower.clone(), s.generic, s.italic, s.weight)
        };
        // 1. Same family, other styles.
        let same_family: Vec<usize> = self.family_faces(&family_lower).unwrap_or_default();
        for idx in same_family {
            if idx != primary && has(idx) {
                return Some(idx);
            }
        }
        // Emoji-presentation characters prefer the system's color emoji
        // font over the monochrome symbols of the bundled faces.
        if matches!(ch as u32, 0x1F000..=0x1FAFF) {
            for fam in crate::font_ext::script_fallback_families(ch) {
                if let Some(ids) = self.family_faces(&fam.to_ascii_lowercase()) {
                    if let Some(idx) = self.best_of(&ids, prim_weight, prim_italic) {
                        if has(idx) {
                            return Some(idx);
                        }
                    }
                }
            }
        }
        // 2. Bundled faces of the same generic, then all bundled.
        let generic = generic.unwrap_or(Family::Sans);
        let bundled_pref: Vec<usize> = match generic {
            Family::Serif => vec![SERIF, SERIF_BOLD],
            Family::Mono => vec![MONO],
            Family::Sans => vec![SANS, SANS_BOLD, SANS_ITALIC, SANS_BOLD_ITALIC],
        };
        for idx in bundled_pref.into_iter().chain(FALLBACK_ORDER) {
            if idx != primary && has(idx) {
                return Some(idx);
            }
        }
        // 3. Script-specific system fonts.
        for fam in crate::font_ext::script_fallback_families(ch) {
            if let Some(ids) = self.family_faces(&fam.to_ascii_lowercase()) {
                if let Some(idx) = self.best_of(&ids, prim_weight, prim_italic) {
                    if has(idx) {
                        return Some(idx);
                    }
                }
            }
        }
        // 4. Any registered face (bounded scan: loading hundreds of system
        //    fonts for one glyph is not worth it).
        let mut scanned = 0;
        for idx in self.visible_faces().into_iter().filter(|&idx| idx >= BUNDLED_COUNT) {
            if scanned >= 48 {
                break;
            }
            let already_loaded = self.faces.borrow()[idx].font.borrow().is_some();
            if !already_loaded {
                scanned += 1;
            }
            if has(idx) {
                return Some(idx);
            }
        }
        None
    }

    /// Shape a run: glyphs with their pen positions, with per-character
    /// font fallback when the primary face lacks a glyph. Runs of a face
    /// with OpenType layout tables are shaped with its GSUB/GPOS
    /// (ligatures, kerning, marks, joining and Indic forms).
    /// `letter_spacing` is added after every glyph (and turns off
    /// ligatures).
    pub fn shape(&self, font: FontId, size: f32, text: &str, letter_spacing: f32) -> Vec<ShapedGlyph> {
        let features = crate::ShapeFeatures { no_ligatures: letter_spacing.abs() > 0.001, no_kerning: false };
        self.shape_with(font, size, text, letter_spacing, features)
    }

    /// [`FontContext::shape`] with explicit font features.
    pub fn shape_with(&self, font: FontId, size: f32, text: &str, letter_spacing: f32, features: crate::ShapeFeatures) -> Vec<ShapedGlyph> {
        // A leading RLO / LRO (put there by bidi layout) gives the whole run
        // its direction: right-to-left runs come out in visual order.
        let (forced, skip) = match text.chars().next() {
            Some('\u{202E}') => (Some(true), 3),
            Some('\u{202D}') => (Some(false), 3),
            _ => (None, 0),
        };
        let text = &text[skip..];
        let mut out = self.shape_dir(font, size, text, letter_spacing, features, forced);
        if skip > 0 {
            for g in &mut out {
                g.byte_index += skip;
            }
        }
        out
    }

    fn shape_dir(&self, font: FontId, size: f32, text: &str, letter_spacing: f32, features: crate::ShapeFeatures, forced: Option<bool>) -> Vec<ShapedGlyph> {
        let primary = self.resolve_index(font.0);
        let primary_face = self.face(primary);
        let mut out = Vec::with_capacity(text.len());
        let mut x = 0.0f32;
        // Segment the text into runs of one face; tabs and zero-width
        // characters are runs of their own (`face == usize::MAX`).
        const SPECIAL: usize = usize::MAX;
        let mut segs: Vec<(usize, usize, usize, bool)> = Vec::new(); // (start, end, face, rtl)
        let dirs = match forced {
            Some(true) => Some(vec![true; text.chars().count()]),
            Some(false) => None,
            None => rtl_chars(text),
        };
        let mut ci = 0usize;
        for (i, ch) in text.char_indices() {
            let end = i + ch.len_utf8();
            let joiner = matches!(ch, '\u{200C}' | '\u{200D}');
            let face = if ch == '\t' || (is_zero_width(ch) && !joiner) {
                SPECIAL
            } else {
                // Combining marks, variation selectors and joiners stay
                // with the preceding character's face when it can shape
                // them.
                let prev = segs.last().map(|s| s.2).filter(|&f| f != SPECIAL);
                match prev {
                    Some(pf) if is_mark_like(ch) && (joiner || self.face(pf).lookup_glyph_index(ch) != 0) => pf,
                    _ if joiner => prev.unwrap_or(SPECIAL),
                    _ => self.lookup(primary, ch).0,
                }
            };
            // Right-to-left runs are shaped on their own (the shaper lays
            // them out in visual order).
            let rtl = dirs.as_ref().map(|d| d[ci]).unwrap_or(false);
            ci += 1;
            match segs.last_mut() {
                Some(s) if s.2 == face && face != SPECIAL && s.3 == rtl => s.1 = end,
                _ => segs.push((i, end, face, rtl)),
            }
        }
        for (start, end, face_idx, seg_rtl) in segs {
            let run = &text[start..end];
            if face_idx == SPECIAL {
                let ch = run.chars().next().unwrap_or(' ');
                let adv = if ch == '\t' {
                    let space = primary_face.lookup_glyph_index(' ');
                    primary_face.metrics_indexed(space, size).advance_width * 4.0 + letter_spacing
                } else {
                    0.0
                };
                out.push(ShapedGlyph { font: FontId(primary), glyph: ZERO_WIDTH_GLYPH, x, advance: adv, dx: 0.0, dy: 0.0, byte_index: start });
                x += adv;
                continue;
            }
            if self.wants_shaping(face_idx) {
                let dir = if forced.is_some() || seg_rtl { Some(seg_rtl) } else { None };
                if let Some(units) = self.shape_units(face_idx, run, features, dir) {
                    let scale = size / units.upem.max(1.0);
                    for g in &units.glyphs {
                        let mut adv = g.x_advance as f32 * scale;
                        if adv != 0.0 {
                            adv += letter_spacing;
                        }
                        out.push(ShapedGlyph {
                            font: FontId(face_idx),
                            glyph: g.glyph,
                            x,
                            advance: adv,
                            dx: g.x_offset as f32 * scale,
                            dy: g.y_offset as f32 * scale,
                            byte_index: start + g.cluster as usize,
                        });
                        x += adv;
                    }
                    continue;
                }
            }
            // No layout tables: advances plus `kern` pairs (right-to-left
            // runs in reverse, with mirrored brackets).
            let face = self.face(face_idx);
            let mut prev: Option<u16> = None;
            let chars: Vec<(usize, char)> = if seg_rtl {
                run.char_indices().rev().map(|(i, c)| (i, mirror_bracket(c))).collect()
            } else {
                run.char_indices().collect()
            };
            for (bi, ch) in chars {
                let glyph = face.lookup_glyph_index(ch);
                if let Some(pg) = prev {
                    if let Some(k) = face.horizontal_kern_indexed(pg, glyph, size) {
                        x += k;
                    }
                }
                let adv = face.metrics_indexed(glyph, size).advance_width + letter_spacing;
                out.push(ShapedGlyph { font: FontId(face_idx), glyph, x, advance: adv, dx: 0.0, dy: 0.0, byte_index: start + bi });
                x += adv;
                prev = Some(glyph);
            }
        }
        out
    }

    /// Rasterize (cached) a glyph at a given size.
    pub fn rasterize(&self, font: FontId, glyph: u16, size: f32) -> Rc<GlyphBitmap> {
        self.rasterize_subpixel(font, glyph, size, 0)
    }

    /// Rasterize a glyph shifted right by `quarter`/4 px (subpixel
    /// positioning; the coverage is resampled horizontally).
    pub fn rasterize_subpixel(&self, font: FontId, glyph: u16, size: f32, quarter: u8) -> Rc<GlyphBitmap> {
        let face_idx = self.resolve_index(font.0);
        let quarter = quarter % 4;
        let key = (face_idx, glyph, (size * 4.0).round().max(0.0) as u32, quarter);
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
            let (m, coverage) = self.face(face_idx).rasterize_indexed(glyph, size);
            let base = GlyphBitmap {
                width: m.width as u32,
                height: m.height as u32,
                left: m.xmin,
                top: m.ymin + m.height as i32,
                coverage,
            };
            if quarter == 0 || base.is_empty() {
                base
            } else {
                shift_coverage(&base, quarter as f32 / 4.0)
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
        let primary = self.resolve_index(font.0);
        let face = self.face(primary);
        if ch == '\t' {
            let space = face.lookup_glyph_index(' ');
            return face.metrics_indexed(space, size).advance_width * 4.0;
        }
        let (face_idx, glyph) = self.lookup(primary, ch);
        self.face(face_idx).metrics_indexed(glyph, size).advance_width
    }

    pub fn face_name(&self, font: FontId) -> String {
        let faces = self.faces.borrow();
        faces.get(font.0).or(faces.first()).map(|s| s.name.clone()).unwrap_or_default()
    }

    /// Number of faces eligible in this view. IDs are shared, not contiguous.
    pub fn face_count(&self) -> usize {
        self.visible_faces().len()
    }

    /// Whether `font` has a real glyph for `ch` (no fallback considered).
    pub fn has_glyph(&self, font: FontId, ch: char) -> bool {
        self.face(font.0).lookup_glyph_index(ch) != 0
    }

    /// Among candidate faces, the one best matching weight/style (CSS font
    /// matching).
    pub(crate) fn best_of(&self, ids: &[usize], weight: u16, italic: bool) -> Option<usize> {
        self.best_of_stretch(ids, weight, italic, 100)
    }

    /// CSS Fonts 4 §5.2 matching: width first (narrower faces first for a
    /// desired width at or below 100%, wider ones above), then style, then
    /// weight.
    pub(crate) fn best_of_stretch(&self, ids: &[usize], weight: u16, italic: bool, stretch: u16) -> Option<usize> {
        let faces = self.faces.borrow();
        let mut best: Option<(i32, usize)> = None;
        for &idx in ids {
            let Some(s) = faces.get(idx) else { continue };
            if s.failed.get() {
                continue;
            }
            let style_penalty = if s.italic == italic { 0 } else { 10_000 };
            let w = s.weight as i32;
            let d = weight as i32;
            let weight_penalty = if d <= 500 {
                // Prefer weights at or below, closest first; then above.
                if w <= d {
                    d - w
                } else {
                    1000 + (w - d)
                }
            } else {
                if w >= d {
                    w - d
                } else {
                    1000 + (d - w)
                }
            };
            let (lo, hi) = (s.stretch.0 as i32, s.stretch.1 as i32);
            let want = stretch as i32;
            let stretch_penalty = if want >= lo && want <= hi {
                0
            } else if want <= 100 {
                if hi < want { want - hi } else { 1000 + (lo - want) }
            } else if lo > want {
                lo - want
            } else {
                1000 + (want - hi)
            };
            let score = stretch_penalty * 100_000 + style_penalty + weight_penalty;
            if best.map(|(b, _)| score < b).unwrap_or(true) {
                best = Some((score, idx));
            }
        }
        best.map(|(_, i)| i)
    }
}

/// Shift a coverage bitmap right by `frac` (0..1) pixels with linear
/// resampling (adds one column).
fn shift_coverage(b: &GlyphBitmap, frac: f32) -> GlyphBitmap {
    let w = b.width as usize;
    let h = b.height as usize;
    let nw = w + 1;
    let mut out = vec![0u8; nw * h];
    let f = frac;
    for y in 0..h {
        let src = &b.coverage[y * w..(y + 1) * w];
        let dst = &mut out[y * nw..(y + 1) * nw];
        for x in 0..nw {
            let left = if x < w { src[x] as f32 } else { 0.0 };
            let prev = if x > 0 { src[x - 1] as f32 } else { 0.0 };
            dst[x] = (left * (1.0 - f) + prev * f + 0.5) as u8;
        }
    }
    GlyphBitmap { width: nw as u32, height: b.height, left: b.left, top: b.top, coverage: out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kerning_can_be_turned_off_with_a_feature_variant() {
        let fc = FontContext::new();
        let f = fc.select(Family::Sans, 400, false);
        let nk = fc.with_features(f, &[(*b"kern", 0)]);
        assert_ne!(nk, f);
        assert_eq!(fc.with_features(f, &[(*b"kern", 0)]), nk, "variants are reused");
        // DejaVu Sans kerns "AV": without kerning the pair is the sum of
        // its advances.
        let sum = fc.measure(f, 32.0, "A") + fc.measure(f, 32.0, "V");
        let kerned = fc.measure(f, 32.0, "AV");
        let plain = fc.measure(nk, 32.0, "AV");
        assert!(kerned < sum - 0.5, "{kerned} vs {sum}");
        assert!((plain - sum).abs() < 0.01, "{plain} vs {sum}");
        assert_eq!(fc.metrics(nk, 16.0), fc.metrics(f, 16.0));
    }

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

    /// Bundled faces load on first use, and every one of them is valid.
    #[test]
    fn bundled_faces_load_lazily_and_parse() {
        let fc = FontContext::new();
        assert!(fc.faces.borrow().iter().all(|s| s.font.borrow().is_none()), "nothing parsed up front");
        for i in 0..BUNDLED_COUNT {
            assert!(fc.try_face(i).is_some(), "bundled face {i} parses");
        }
    }

    /// Fallback candidates are screened by their character map before being
    /// loaded; the chosen face must still map the character once loaded,
    /// and the result must not depend on which faces were loaded before.
    #[test]
    fn fallback_faces_map_their_characters() {
        let chars = ['A', 'é', 'Ж', 'ש', 'ب', 'क', '中', 'あ', '한', '€', '→', '∑', '★', '♪', '😀', '\u{10FFFF}'];
        let fresh = || FontContext::with_system_fonts();
        let warm = fresh();
        // Load every face of the warm context first.
        for i in 0..warm.faces.borrow().len() {
            let _ = warm.try_face(i);
        }
        for &ch in &chars {
            let cold = fresh();
            let a = cold.fallback_for_char(ch, FontId(0));
            let b = warm.fallback_for_char(ch, FontId(0));
            assert_eq!(a, b, "fallback for {ch:?} depends on loaded faces");
            if let Some(f) = a {
                let face = cold.try_face(cold.resolve_index(f.0)).expect("chosen face loads");
                assert_ne!(face.lookup_glyph_index(ch), 0, "chosen face lacks {ch:?}");
            }
        }
    }

    /// Faces of one family differing only in width are kept apart and
    /// ranked by `font-stretch` (narrower first at or below 100%).
    #[test]
    fn web_faces_match_font_stretch() {
        let fc = FontContext::new();
        let sans = include_bytes!("../../../assets/fonts/DejaVuSans.ttf");
        let serif = include_bytes!("../../../assets/fonts/DejaVuSerif.ttf");
        let mono = include_bytes!("../../../assets/fonts/DejaVuSansMono.ttf");
        let normal = fc.add_font_data_stretch("W", 400, false, (100, 100), sans).unwrap();
        let condensed = fc.add_font_data_stretch("W", 400, false, (75, 75), serif).unwrap();
        let expanded = fc.add_font_data_stretch("W", 400, false, (125, 150), mono).unwrap();
        let family = ["W".to_string()];
        let pick = |stretch| fc.select_family_stretch(&family, Family::Sans, 400, false, stretch);
        assert_eq!(pick(100), normal);
        assert_eq!(pick(75), condensed);
        assert_eq!(pick(50), condensed, "narrower first, then wider");
        assert_eq!(pick(87), condensed);
        assert_eq!(pick(112), expanded, "wider first above 100%");
        assert_eq!(pick(140), expanded, "inside a range");
        assert_eq!(pick(200), expanded);
        assert_eq!(fc.select_family(&family, Family::Sans, 400, false), normal);
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
    fn opentype_shaping() {
        let fc = FontContext::new();
        let f = fc.select(Family::Sans, 400, false);
        // GPOS kerning pulls "AV" together.
        let a = fc.measure(f, 40.0, "A");
        let v = fc.measure(f, 40.0, "V");
        assert!(fc.measure(f, 40.0, "AV") < a + v - 1.0, "AV is kerned");
        // Arabic letters take their joining forms (other glyphs than the
        // isolated ones of the cmap).
        let word = fc.shape(f, 20.0, "\u{0633}\u{0644}\u{0627}\u{0645}", 0.0);
        let face = fc.face(f.0);
        let isolated: Vec<u16> = "\u{0633}\u{0644}\u{0627}\u{0645}".chars().map(|c| face.lookup_glyph_index(c)).collect();
        assert!(isolated.iter().all(|g| *g != 0), "DejaVu Sans has Arabic");
        let shaped: Vec<u16> = word.iter().map(|g| g.glyph).collect();
        assert!(shaped.iter().any(|g| !isolated.contains(g)), "joining forms: {shaped:?} vs {isolated:?}");
        // Right-to-left: glyphs come out in visual order (last char first).
        assert!(word.first().unwrap().byte_index > word.last().unwrap().byte_index);
        // Letter spacing turns ligatures off and is added per glyph.
        let spaced = fc.shape(f, 16.0, "ff", 3.0);
        assert_eq!(spaced.len(), 2);
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


/// A font face registered by name (web font or system font).
#[derive(Debug, Clone, PartialEq)]
pub struct FaceInfo {
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    pub id: FontId,
}

