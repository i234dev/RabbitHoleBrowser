//! Document bindings must not contaminate other live documents or the host.
use crate::font::{Family, FontContext};
use crate::font_ext::sfnt;
use std::rc::Rc;

const SANS: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSans.ttf");
const MONO: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSansMono.ttf");
const PRIVATE_CHAR: char = '\u{10fffd}';

/// Give a real font one private-use mapping absent from all bundled faces.
fn private_font() -> Vec<u8> {
    let parsed = fontdue::Font::from_bytes(SANS, crate::font::fontdue_settings()).unwrap();
    let glyph = parsed.lookup_glyph_index('W') as u32;
    let mut cmap = vec![0, 0, 0, 1, 0, 3, 0, 10, 0, 0, 0, 12];
    cmap.extend_from_slice(&12u16.to_be_bytes());
    cmap.extend_from_slice(&0u16.to_be_bytes());
    for n in [28u32, 0, 1, PRIVATE_CHAR as u32, PRIVATE_CHAR as u32, glyph] {
        cmap.extend_from_slice(&n.to_be_bytes());
    }
    let dir = sfnt::table_directory(SANS, 0).unwrap();
    let tables: Vec<_> = dir.iter().map(|t| {
        let bytes = if t.tag == *b"cmap" { cmap.as_slice() }
            else { &SANS[t.offset as usize..(t.offset + t.length) as usize] };
        (t.tag, bytes)
    }).collect();
    sfnt::build_sfnt(0x00010000, &tables)
}

#[test]
fn aliases_are_document_local_and_retained_ids_stay_valid() {
    let host = FontContext::new();
    host.add_font_data("Host test font", 400, false, SANS).unwrap();
    let a = host.document_scope();
    let aid = a.add_font_data("Shared alias", 400, false, SANS).unwrap();
    let before = a.measure(aid, 20.0, "WWWiii");
    let b = host.document_scope();
    let bid = b.add_font_data("Shared alias", 400, false, MONO).unwrap();
    assert_ne!(aid, bid);
    assert_eq!(a.select_family(&["Shared alias".into()], Family::Sans, 400, false), aid);
    assert_eq!(b.select_family(&["Shared alias".into()], Family::Sans, 400, false), bid);
    assert_eq!(a.measure(aid, 20.0, "WWWiii"), before);
    assert_ne!(b.measure(bid, 20.0, "WWWiii"), before);
    assert!(!host.has_family("Shared alias"));
    let child = a.document_scope();
    assert!(child.has_family("Host test font"));
    assert!(!child.has_family("Shared alias"));
}

#[test]
fn repeated_navigation_interns_data_and_full_binding_metadata() {
    let host = FontContext::new();
    let a = host.document_scope();
    let first = a.add_font_data("Document alias", 400, false, SANS).unwrap();
    let slots = host.faces.borrow().len();
    for _ in 0..20 {
        let next = host.document_scope();
        assert_eq!(next.add_font_data("document ALIAS", 400, false, SANS), Some(first));
    }
    assert_eq!(host.faces.borrow().len(), slots);
    let italic = a.add_font_data("Document alias", 400, true, SANS).unwrap();
    let bold = a.add_font_data("Document alias", 700, false, SANS).unwrap();
    assert_ne!(first, italic);
    assert_ne!(first, bold);
    assert!(!a.needs_synthetic_italic(italic, true));
    assert!(!a.needs_synthetic_bold(bold, 700));
    assert!(Rc::ptr_eq(&a.face(first.0), &a.face(italic.0)));
    let faces = host.faces.borrow();
    let crate::font::FaceSource::Bytes(original) = &faces[first.0].source else { panic!() };
    let crate::font::FaceSource::Bytes(other) = &faces[bold.0].source else { panic!() };
    assert!(Rc::ptr_eq(original, other));
}

#[test]
fn private_fallback_does_not_escape_and_late_host_fonts_invalidate_misses() {
    let host = FontContext::new();
    let a = host.document_scope();
    let b = host.document_scope();
    let primary = host.select(Family::Sans, 400, false);
    assert_eq!(b.fallback_for_char(PRIVATE_CHAR, primary), None);
    let private = private_font();
    let fallback = a.add_font_data("Private fallback", 400, false, &private).unwrap();
    assert!(a.has_glyph(fallback, PRIVATE_CHAR));
    assert_eq!(a.fallback_for_char(PRIVATE_CHAR, primary), Some(fallback));
    assert_eq!(b.fallback_for_char(PRIVATE_CHAR, primary), None);
    assert!(!b.faces().iter().any(|face| face.id == fallback));
    // Host changes are visible to already-created pages, including cached misses.
    let shared = host.add_font_data("Late host fallback", 400, false, &private).unwrap();
    assert_eq!(b.fallback_for_char(PRIVATE_CHAR, primary), Some(shared));
}

#[test]
fn limits_are_shared_atomic_and_allow_interning_at_capacity() {
    use crate::font::FontLimits;
    let host = FontContext::with_limits(FontLimits { max_web_bindings: 1, max_sfnt_bytes: 0 });
    let a = host.document_scope();
    // Reusing bundled data needs a binding but no additional SFNT storage.
    let id = a.add_font_data("Reusable", 400, false, SANS).unwrap();
    let b = host.document_scope();
    assert_eq!(b.add_font_data("Reusable", 400, false, SANS), Some(id));
    assert!(b.add_font_data("Denied", 400, false, SANS).is_none());
    assert!(!b.has_family("Denied"));
    assert_eq!(host.web_bindings.get(), 1);
    assert_eq!(host.web_sfnt_bytes.get(), 0);

    let data = private_font();
    let exact = FontContext::with_limits(FontLimits { max_web_bindings: 2, max_sfnt_bytes: data.len() });
    assert!(exact.add_font_data("Private", 400, false, &data).is_some());
    assert!(exact.add_font_data("Private alias", 400, true, &data).is_some());
    assert_eq!(exact.web_sfnt_bytes.get(), data.len());
    let low = FontContext::with_limits(FontLimits { max_web_bindings: 2, max_sfnt_bytes: data.len() - 1 });
    assert!(low.add_font_data("Rejected", 400, false, &data).is_none());
    assert!(low.add_font_data("Invalid", 400, false, b"not a font").is_none());
    assert_eq!(low.web_sfnt_bytes.get(), 0);
    assert_eq!(low.web_bindings.get(), 0);
    assert!(!low.has_family("Rejected"));
    assert!(low.add_font_data("Still available", 400, false, SANS).is_some());
}
