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
    FontFace { family: "Subset Web".into(), sources: vec![], weight: (700, 700), italic: false, stretch: (100, 100), display: "swap".into(), unicode_range: range, feature_settings: vec![], metric_overrides: Default::default() }
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

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len() * 4 / 3 + 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// `document.fonts` lists the document's @font-face rules (CSS-connected
/// faces) with their load status; they cannot be deleted from the set.
#[test]
fn document_fonts_lists_css_connected_faces() {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.loader.load_images = false;
    let url = format!("data:font/ttf;base64,{}", base64(SANS_BOLD));
    let html = format!(
        "<!doctype html><style>@font-face{{font-family:'Subset Web';src:url({url}) format('truetype');font-weight:700;unicode-range:U+0-FF,U+131}}\
         @font-face{{font-family:'Subset Web';src:url({url}) format('truetype');font-weight:700;unicode-range:U+400-45F}}</style>\
         <span style=\"font:700 20px 'Subset Web'\">italia</span>"
    );
    let loaded = engine.loader.load_html(&html, Url::parse("https://example.test/fonts").unwrap(), VIEWPORT);
    let mut page = engine.build_page_unscripted(loaded, VIEWPORT);
    let faces = engine.eval_in_page(&mut page, "[...document.fonts].map(f=>[f.family,f.weight,f.style,f.unicodeRange,f.status].join(' ')).join('|')");
    assert_eq!(faces, "'Subset Web 700 normal U+0-FF, U+131 loaded|Subset Web 700 normal U+400-45F unloaded'");
    let rest = engine.eval_in_page(&mut page, "[document.fonts.size, document.fonts.check(\"700 20px 'Subset Web'\"), document.fonts.check(\"700 20px 'Subset Web'\", 'Ж'), document.fonts.delete([...document.fonts][0]), (document.fonts.clear(), document.fonts.size)].join(' ')");
    assert_eq!(rest, "'2 true false false 2'");
}
