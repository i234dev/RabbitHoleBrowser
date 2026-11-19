//! PDF painting must retain the document's private fallback context.
use rabbithole_engine::gfx::{font_ext::sfnt, Family, FontContext};
use rabbithole_engine::net::Url;
use rabbithole_engine::style::FontFace;
use rabbithole_engine::{Engine, LoadedFont, Viewport};
use std::sync::Arc;

#[test]
fn retained_document_pdf_keeps_private_fallback_after_navigation() {
    let sans = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../assets/fonts/DejaVuSans.ttf"));
    let fonts = FontContext::new();
    let private_char = '\u{10fffd}';
    let glyph = fonts.shape(fonts.select(Family::Sans, 400, false), 20.0, "W", 0.0)[0].glyph as u32;
    let mut cmap = vec![0, 0, 0, 1, 0, 3, 0, 10, 0, 0, 0, 12];
    cmap.extend_from_slice(&12u16.to_be_bytes());
    cmap.extend_from_slice(&0u16.to_be_bytes());
    for n in [28u32, 0, 1, private_char as u32, private_char as u32, glyph] {
        cmap.extend_from_slice(&n.to_be_bytes());
    }
    let dir = sfnt::table_directory(sans, 0).unwrap();
    let tables: Vec<_> = dir.iter().map(|t| {
        let bytes = if t.tag == *b"cmap" { cmap.as_slice() }
            else { &sans[t.offset as usize..(t.offset + t.length) as usize] };
        (t.tag, bytes)
    }).collect();
    let private_font = sfnt::build_sfnt(0x00010000, &tables);
    let mut engine = Engine::new_bundled_fonts_only();
    engine.scripting = false;
    engine.loader.load_fonts = false;
    let viewport = Viewport { width: 300.0, height: 120.0 };
    let html = format!("<!doctype html><style>body{{font:20px sans-serif}}</style><p>{private_char}</p>");
    let mut loaded = engine.loader.load_html(&html, Url::parse("https://example.test/pdf-font-a").unwrap(), viewport);
    loaded.fonts.push(LoadedFont {
        face: FontFace { family: "PDF private fallback".into(), sources: vec![], weight: (400, 400), italic: false, stretch: (100, 100), display: "auto".into(), unicode_range: None, feature_settings: vec![] },
        data: Arc::new(private_font),
    });
    let mut a = engine.build_page_unscripted(loaded, viewport);
    let before = crate::gui::pdf::paginate(&a.display_list, &a.fonts, viewport.width, a.content_height(), a.background, 1.0);
    assert!(!before.is_empty());
    let _b = engine.load_html("<p>another document</p>", Url::parse("https://example.test/pdf-font-b").unwrap(), viewport);
    engine.restyle(&mut a);
    let after = crate::gui::pdf::paginate(&a.display_list, &a.fonts, viewport.width, a.content_height(), a.background, 1.0);
    assert_eq!(before.len(), after.len());
    for (before, after) in before.iter().zip(&after) {
        assert_eq!((before.width, before.height), (after.width, after.height));
        assert_eq!(before.rgb, after.rgb);
    }
    // Control: the base context cannot render this document-private glyph.
    let wrong = crate::gui::pdf::paginate(&a.display_list, &engine.fonts, viewport.width, a.content_height(), a.background, 1.0);
    assert_ne!(after[0].rgb, wrong[0].rgb);
}
