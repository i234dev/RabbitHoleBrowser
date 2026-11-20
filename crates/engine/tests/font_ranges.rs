//! `@font-face` rules that differ only in `unicode-range` (the subsets a
//! font service serves for one family and weight) are all registered.
use rabbithole_engine::net::Url;
use rabbithole_engine::style::FontFace;
use rabbithole_engine::{Engine, LoadedFont, Viewport};
use std::sync::Arc;

const SANS_BOLD: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSans-Bold.ttf");
const MONO: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSansMono.ttf");
const VIEWPORT: Viewport = Viewport { width: 500.0, height: 200.0 };
const HTML: &str = "<!doctype html><style>body{margin:0}span{font:700 20px 'Subset Web';text-transform:uppercase}</style><span id=s>italia</span>";

fn face(range: Option<Vec<(u32, u32)>>) -> FontFace {
    FontFace { family: "Subset Web".into(), sources: vec![], weight: (700, 700), italic: false, stretch: (100, 100), display: "swap".into(), unicode_range: range, feature_settings: vec![] }
}

fn span_width(fonts: Vec<LoadedFont>) -> f32 {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.loader.load_fonts = false;
    engine.loader.load_images = false;
    engine.scripting = false;
    let mut loaded = engine.loader.load_html(HTML, Url::parse("https://example.test/subsets").unwrap(), VIEWPORT);
    loaded.fonts = fonts;
    let page = engine.build_page_unscripted(loaded, VIEWPORT);
    page.rect_of(page.doc.element_by_id("s").unwrap()).unwrap().w
}

#[test]
fn every_unicode_range_subset_of_a_weight_is_used() {
    // Subsets in the order font services list them: Cyrillic first, Latin
    // last. The Cyrillic file (Mono) maps Latin letters too.
    let subsets = vec![
        LoadedFont { face: face(Some(vec![(0x0400, 0x045F)])), data: Arc::new(MONO.to_vec()) },
        LoadedFont { face: face(Some(vec![(0x0000, 0x00FF), (0x2000, 0x206F)])), data: Arc::new(SANS_BOLD.to_vec()) },
    ];
    let whole = vec![LoadedFont { face: face(None), data: Arc::new(SANS_BOLD.to_vec()) }];
    let mono = vec![LoadedFont { face: face(None), data: Arc::new(MONO.to_vec()) }];
    let w = span_width(subsets);
    assert_eq!(w, span_width(whole));
    assert_ne!(w, span_width(mono));
}
