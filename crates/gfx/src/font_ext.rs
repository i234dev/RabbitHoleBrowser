//! Web fonts (`@font-face`: TTF/OTF/WOFF/WOFF2), system font discovery and
//! CSS family matching.

pub mod sfnt;
pub mod sysfonts;
pub mod woff;
pub mod woff2;

use crate::font::{FaceInfo, FaceSlot, FaceSource, Family, FontContext, FontId, BUNDLED_COUNT};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Well-known system families to try for a character's script before
/// scanning everything.
pub fn script_fallback_families(ch: char) -> &'static [&'static str] {
    let c = ch as u32;
    match c {
        0x0590..=0x05FF => &["Noto Sans Hebrew", "Segoe UI", "Arial", "Arial Hebrew", "DejaVu Sans"],
        0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => {
            &["Noto Sans Arabic", "Segoe UI", "Arial", "Geeza Pro", "Noto Naskh Arabic", "DejaVu Sans"]
        }
        0x0900..=0x097F => &["Noto Sans Devanagari", "Nirmala UI", "Kohinoor Devanagari", "Mangal", "Lohit Devanagari"],
        0x0980..=0x0DFF => &["Nirmala UI", "Noto Sans Bengali", "Noto Sans Tamil", "Noto Sans Telugu", "Noto Sans Kannada", "Noto Sans Malayalam"],
        0x0E00..=0x0E7F => &["Noto Sans Thai", "Leelawadee UI", "Thonburi", "Tahoma"],
        0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF => &["Noto Sans CJK KR", "Malgun Gothic", "Apple SD Gothic Neo", "NanumGothic", "Noto Sans KR"],
        0x3040..=0x30FF | 0x31F0..=0x31FF => &["Noto Sans CJK JP", "Yu Gothic", "Meiryo", "Hiragino Sans", "Hiragino Kaku Gothic ProN", "MS Gothic", "Noto Sans JP"],
        0x2E80..=0x2FDF | 0x3000..=0x303F | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FA1F => &[
            "Noto Sans CJK SC",
            "Microsoft YaHei",
            "PingFang SC",
            "Hiragino Sans GB",
            "SimSun",
            "Noto Sans CJK JP",
            "Noto Sans CJK TC",
            "WenQuanYi Zen Hei",
            "WenQuanYi Micro Hei",
            "Yu Gothic",
            "Meiryo",
            "Microsoft JhengHei",
            "Noto Sans SC",
        ],
        0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2300..=0x23FF | 0x2B00..=0x2BFF | 0xFE00..=0xFE0F => {
            &["Segoe UI Emoji", "Apple Color Emoji", "Noto Color Emoji", "Noto Emoji", "Segoe UI Symbol", "Symbola", "DejaVu Sans"]
        }
        0x2000..=0x2BFF | 0x1D400..=0x1D7FF => &["Segoe UI Symbol", "Noto Sans Symbols", "Noto Sans Symbols2", "Apple Symbols", "Symbola", "DejaVu Sans"],
        0x0370..=0x03FF | 0x0400..=0x052F => &["Segoe UI", "Arial", "Helvetica", "Noto Sans", "Liberation Sans", "DejaVu Sans"],
        _ => &["Noto Sans", "Segoe UI", "Arial", "Helvetica", "Liberation Sans", "Arial Unicode MS", "DejaVu Sans"],
    }
}

/// Platform default candidates for the generic families.
fn generic_candidates(name: &str) -> Option<&'static [&'static str]> {
    Some(match name {
        "sans-serif" | "system-ui" | "ui-sans-serif" | "-apple-system" | "blinkmacsystemfont" | "cursive" | "fantasy" => {
            &["Segoe UI", "Helvetica Neue", "Helvetica", "Arial", "Noto Sans", "Liberation Sans", "DejaVu Sans", "Cantarell", "Ubuntu", "Roboto"]
        }
        "serif" | "ui-serif" => &["Times New Roman", "Georgia", "Times", "Noto Serif", "Liberation Serif", "DejaVu Serif"],
        "monospace" | "ui-monospace" => &["Consolas", "Menlo", "Cascadia Mono", "SF Mono", "Courier New", "Noto Sans Mono", "Liberation Mono", "DejaVu Sans Mono"],
        "emoji" => &["Segoe UI Emoji", "Apple Color Emoji", "Noto Color Emoji", "Noto Emoji"],
        "math" => &["Cambria Math", "STIX Two Math", "Latin Modern Math", "DejaVu Math TeX Gyre", "DejaVu Serif"],
        "fangsong" => &["FangSong", "Noto Serif CJK SC"],
        _ => return None,
    })
}

impl FontContext {
    /// Register a font from raw bytes (TTF/OTF, TTC, WOFF, WOFF2) under
    /// `family` with the given weight/style (from the `@font-face`
    /// descriptors). Returns None when the data cannot be decoded.
    pub fn add_font_data(&self, family: &str, weight: u16, italic: bool, data: &[u8]) -> Option<FontId> {
        // Raw SFNT can be compared against the store without first copying it.
        let sfnt = match data.get(..4) {
            Some([0x00, 0x01, 0x00, 0x00] | b"OTTO" | b"true") => std::borrow::Cow::Borrowed(data),
            _ => std::borrow::Cow::Owned(decode_font_container(data)?),
        };
        let family = family.trim().trim_matches('"').trim_matches('\'').to_string();
        if family.is_empty() {
            return None;
        }
        let family_lower = family.to_ascii_lowercase();
        let weight = weight.clamp(1, 1000);
        let (existing_binding, existing_data) = {
            let faces = self.faces.borrow();
            let mut binding = None;
            let mut data = None;
            let candidates = self.byte_faces.borrow();
            for &idx in candidates.get(&Self::bytes_key(sfnt.as_ref())).into_iter().flatten() {
                let slot = &faces[idx];
                let FaceSource::Bytes(bytes) = &slot.source else { continue };
                if bytes.as_slice() != sfnt.as_ref() {
                    continue;
                }
                data = Some((bytes.clone(), slot.font.borrow().clone()));
                if slot.generic.is_none() && slot.family_lower == family_lower && slot.weight == weight && slot.italic == italic {
                    binding = Some(idx);
                    break;
                }
            }
            (binding, data)
        };
        if let Some(idx) = existing_binding {
            self.bind_face(idx);
            return Some(FontId(idx));
        }
        // Interning hits above remain usable even at capacity. Nothing is
        // published or charged until decoding and parsing have both succeeded.
        let additional_bytes = if existing_data.is_some() { 0 } else { sfnt.len() };
        let retained_bytes = self.web_sfnt_bytes.get().checked_add(additional_bytes)?;
        if self.web_bindings.get() >= self.limits.max_web_bindings || retained_bytes > self.limits.max_sfnt_bytes {
            return None;
        }
        let (bytes, font) = if let Some((bytes, Some(font))) = existing_data {
            (bytes, font)
        } else {
            let font = fontdue::Font::from_bytes(sfnt.as_ref(), crate::font::fontdue_settings()).ok()?;
            (Rc::new(sfnt.into_owned()), Rc::new(font))
        };
        let name = format!("{family} {weight}{}", if italic { " Italic" } else { "" });
        let id = self.register(FaceSlot {
            name,
            family_lower,
            family,
            weight,
            italic,
            generic: None,
            source: FaceSource::Bytes(bytes),
            font: RefCell::new(Some(font)),
            failed: Cell::new(false),
        });
        self.web_bindings.set(self.web_bindings.get() + 1);
        self.web_sfnt_bytes.set(retained_bytes);
        Some(id)
    }

    /// Scan the platform font directories and register every TTF/OTF/TTC
    /// face by family name. Only the font headers are read now; glyph data
    /// is parsed on first use. Returns the number of faces found.
    pub fn load_system_fonts(&self) -> usize {
        if self.system_loaded.get() {
            return 0;
        }
        self.system_loaded.set(true);
        // A system scan belongs to the host catalog even when requested
        // through a document view. Other views share the loaded flag.
        let host = self.host_scope();
        let found = sysfonts::scan_system_fonts();
        let n = found.len();
        for f in found {
            if host.has_exact_face(&f.family, f.weight, f.italic) {
                continue;
            }
            host.register(FaceSlot {
                name: f.full_name.clone(),
                family_lower: f.family.to_ascii_lowercase(),
                family: f.family,
                weight: f.weight,
                italic: f.italic,
                generic: None,
                source: FaceSource::Path(f.path, f.index),
                font: RefCell::new(None),
                failed: Cell::new(false),
            });
        }
        n
    }

    fn has_exact_face(&self, family: &str, weight: u16, italic: bool) -> bool {
        let key = family.to_ascii_lowercase();
        let faces = self.faces.borrow();
        self.family_faces(&key)
            .map(|ids| ids.iter().any(|&i| faces[i].weight == weight && faces[i].italic == italic && i >= BUNDLED_COUNT))
            .unwrap_or(false)
    }

    /// Pick the best face for a CSS family list, falling back to the
    /// bundled face for `generic`.
    pub fn select_family(&self, families: &[String], generic: Family, weight: u16, italic: bool) -> FontId {
        for fam in families {
            let name = fam.trim().trim_matches('"').trim_matches('\'').to_ascii_lowercase();
            if name.is_empty() {
                continue;
            }
            if let Some(candidates) = generic_candidates(&name) {
                for c in candidates {
                    if let Some(id) = self.select_exact_family(c, weight, italic) {
                        return id;
                    }
                }
                // A generic keyword with no system match: use the bundled face.
                let g = match name.as_str() {
                    "serif" | "ui-serif" => Family::Serif,
                    "monospace" | "ui-monospace" => Family::Mono,
                    _ => generic,
                };
                return self.select(g, weight, italic);
            }
            if let Some(id) = self.select_exact_family(&name, weight, italic) {
                return id;
            }
        }
        self.select(generic, weight, italic)
    }

    /// Resolve a generic through host fonts, without treating a document's
    /// named aliases as platform generic candidates. Unknown generic names
    /// use Sans; the rounded generic currently shares the Sans fallback.
    pub fn select_generic_family(&self, family: &str, weight: u16, italic: bool) -> FontId {
        let family = family.to_ascii_lowercase();
        let family = if family == "ui-rounded" { "sans-serif".to_string() } else { family };
        if generic_candidates(&family).is_none() {
            return self.select(Family::Sans, weight, italic);
        }
        self.host_scope().select_family(&[family], Family::Sans, weight, italic)
    }

    /// Select a literal family name without interpreting generic keywords.
    /// A quoted CSS family such as `"serif"` uses this path.
    pub fn select_exact_family(&self, family_lower: &str, weight: u16, italic: bool) -> Option<FontId> {
        let ids = self.family_faces(&family_lower.to_ascii_lowercase())?;
        let idx = self.best_of(&ids, weight, italic)?;
        // Make sure the face actually loads.
        self.try_face(idx)?;
        Some(FontId(idx))
    }

    /// True when `font` is an upright face chosen for an italic request.
    pub fn needs_synthetic_italic(&self, font: FontId, italic: bool) -> bool {
        if !italic {
            return false;
        }
        let faces = self.faces.borrow();
        faces.get(font.0).map(|s| !s.italic).unwrap_or(false)
    }

    /// True when `font` is a non-bold face chosen for a bold request.
    pub fn needs_synthetic_bold(&self, font: FontId, weight: u16) -> bool {
        if weight < 600 {
            return false;
        }
        let faces = self.faces.borrow();
        faces.get(font.0).map(|s| s.weight < 600).unwrap_or(false)
    }

    /// Registered faces (bundled + web + system).
    pub fn faces(&self) -> Vec<FaceInfo> {
        let faces = self.faces.borrow();
        self.visible_faces()
            .into_iter()
            .map(|i| {
                let s = &faces[i];
                FaceInfo { family: s.family.clone(), weight: s.weight, italic: s.italic, id: FontId(i) }
            })
            .collect()
    }

    /// Does any registered face provide `family`?
    pub fn has_family(&self, family: &str) -> bool {
        self.family_faces(&family.trim().to_ascii_lowercase()).is_some()
    }

    /// Family name of a face.
    pub fn family_of(&self, font: FontId) -> String {
        self.faces.borrow().get(font.0).map(|s| s.family.clone()).unwrap_or_default()
    }
}

/// Decode any supported container (TTF/OTF/TTC/WOFF/WOFF2) into plain SFNT
/// bytes that fontdue accepts.
pub fn decode_font_container(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 4 {
        return None;
    }
    match &data[0..4] {
        b"wOFF" => woff::decode(data),
        b"wOF2" => woff2::decode(data),
        b"ttcf" => sfnt::extract_face(data, 0),
        [0x00, 0x01, 0x00, 0x00] | b"OTTO" | b"true" => Some(data.to_vec()),
        _ => None,
    }
}
