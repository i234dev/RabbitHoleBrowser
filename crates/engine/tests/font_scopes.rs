//! Retained documents and frames keep their own font aliases and fallback.
use rabbithole_engine::gfx::{font_ext::sfnt, Family, FontContext};
use rabbithole_engine::net::Url;
use rabbithole_engine::style::FontFace;
use rabbithole_engine::{Engine, LoadedFont, Page, Viewport};
use std::sync::Arc;

const SANS: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSans.ttf");
const MONO: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSansMono.ttf");
const PRIVATE_CHAR: char = '\u{10fffd}';
const VIEWPORT: Viewport = Viewport { width: 500.0, height: 250.0 };

fn make_engine() -> Engine {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.loader.load_fonts = false;
    engine.loader.load_images = false;
    engine
}

fn page(engine: &Engine, html: &str, family: &str, data: &[u8]) -> Page {
    let mut loaded = engine.loader.load_html(html, Url::parse("https://example.test/font-scope").unwrap(), VIEWPORT);
    loaded.fonts.push(LoadedFont {
        face: FontFace { family: family.into(), sources: vec![], weight: (400, 400), italic: false, display: "auto".into(), unicode_range: None },
        data: Arc::new(data.to_vec()),
    });
    engine.build_page_unscripted(loaded, VIEWPORT)
}

fn canvas_width(engine: &Engine, page: &mut Page, family: &str, text: &str) -> f32 {
    engine.eval_in_page(page, &format!(
        "(()=>{{let c=document.getElementById('canvas'),x=c.getContext('2d');x.font='20px {family}';x.clearRect(0,0,200,40);x.fillText('{text}',0,25);return x.measureText('{text}').width}})()"
    )).parse().expect("numeric Canvas text width")
}

fn caret(page: &Page, text: &str) -> f32 {
    let input = page.doc.element_by_id("input").unwrap();
    page.caret_x(&page.fonts, input, text.len()).unwrap()
}

#[test]
fn navigation_keeps_retained_alias_layout_canvas_and_caret() {
    let engine = make_engine();
    let html = "<!doctype html><style>body{margin:0;font:20px Scoped}input{font:20px Scoped}#units{width:10ch;height:1ex}</style><div id=units>WWWiii</div><input id=input value=WWWiii><canvas id=canvas width=200 height=40></canvas>";
    let mut a = page(&engine, html, "Scoped", SANS);
    let a_width = canvas_width(&engine, &mut a, "Scoped", "WWWiii");
    let a_caret = caret(&a, "WWWiii");
    let a_units = a.rect_of(a.doc.element_by_id("units").unwrap()).unwrap();
    let frame = engine.snapshot(&a);
    let mut b = page(&engine, html, "Scoped", MONO);
    assert_ne!(canvas_width(&engine, &mut b, "Scoped", "WWWiii"), a_width);
    assert_ne!(caret(&b, "WWWiii"), a_caret);
    assert!(!engine.fonts.has_family("Scoped"));
    engine.restyle(&mut a);
    assert_eq!(canvas_width(&engine, &mut a, "Scoped", "WWWiii"), a_width);
    assert_eq!(caret(&a, "WWWiii"), a_caret);
    assert_eq!(a.rect_of(a.doc.element_by_id("units").unwrap()).unwrap(), a_units);
    assert_eq!(engine.snapshot(&a), frame);
    engine.check_incremental(&mut a).unwrap();
    let mut fresh_engine = make_engine();
    fresh_engine.scripting = false;
    let mut fresh = page(&fresh_engine, html, "Scoped", SANS);
    canvas_width(&fresh_engine, &mut fresh, "Scoped", "WWWiii");
    assert_eq!(engine.snapshot(&a), fresh_engine.snapshot(&fresh));
}

/// A document-private fallback glyph whose advance distinguishes Sans/Mono.
fn private_font(data: &[u8], family: Family) -> Vec<u8> {
    let bundled = FontContext::new();
    let glyph = bundled.shape(bundled.select(family, 400, false), 20.0, "W", 0.0)[0].glyph as u32;
    let mut cmap = vec![0, 0, 0, 1, 0, 3, 0, 10, 0, 0, 0, 12];
    cmap.extend_from_slice(&12u16.to_be_bytes());
    cmap.extend_from_slice(&0u16.to_be_bytes());
    for n in [28u32, 0, 1, PRIVATE_CHAR as u32, PRIVATE_CHAR as u32, glyph] {
        cmap.extend_from_slice(&n.to_be_bytes());
    }
    let dir = sfnt::table_directory(data, 0).unwrap();
    let tables: Vec<_> = dir.iter().map(|t| {
        let bytes = if t.tag == *b"cmap" { cmap.as_slice() }
            else { &data[t.offset as usize..(t.offset + t.length) as usize] };
        (t.tag, bytes)
    }).collect();
    sfnt::build_sfnt(0x00010000, &tables)
}

#[test]
fn retained_paint_and_measurement_use_document_private_fallback() {
    let engine = make_engine();
    let html = format!("<!doctype html><style>body{{margin:0;font:20px sans-serif}}input{{font:20px sans-serif}}</style><p>{PRIVATE_CHAR}</p><input id=input value='{PRIVATE_CHAR}'><canvas id=canvas width=200 height=40></canvas>");
    let a_data = private_font(SANS, Family::Sans);
    let b_data = private_font(MONO, Family::Mono);
    let mut a = page(&engine, &html, "Private fallback", &a_data);
    let width = canvas_width(&engine, &mut a, "sans-serif", &PRIVATE_CHAR.to_string());
    let caret_before = caret(&a, &PRIVATE_CHAR.to_string());
    let before = engine.snapshot(&a);
    let mut b = page(&engine, &html, "Private fallback", &b_data);
    assert_ne!(canvas_width(&engine, &mut b, "sans-serif", &PRIVATE_CHAR.to_string()), width);
    assert_ne!(caret(&b, &PRIVATE_CHAR.to_string()), caret_before);
    assert_eq!(engine.fonts.fallback_for_char(PRIVATE_CHAR, engine.fonts.select(Family::Sans, 400, false)), None);
    engine.restyle(&mut a);
    assert_eq!(canvas_width(&engine, &mut a, "sans-serif", &PRIVATE_CHAR.to_string()), width);
    assert_eq!(caret(&a, &PRIVATE_CHAR.to_string()), caret_before);
    assert_eq!(engine.snapshot(&a), before);
    engine.check_incremental(&mut a).unwrap();
}

#[test]
fn iframe_font_view_is_independent_of_retained_parent_and_new_pages() {
    let engine = make_engine();
    let mut parent = page(&engine, "<!doctype html><style>body{font:20px Scoped}</style><p>WWWiii</p><iframe srcdoc='<!doctype html><style>body{font:20px Scoped}</style><p>WWWiii</p>'></iframe>", "Scoped", SANS);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while parent.frames.children.first().and_then(|f| f.page.as_ref()).is_none() && std::time::Instant::now() < deadline {
        engine.tick(&mut parent);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let parent_id = parent.fonts.select_family(&["Scoped".into()], Family::Sans, 400, false);
    let child = parent.frames.children.first_mut().and_then(|f| f.page.as_mut()).expect("srcdoc child");
    assert!(!child.fonts.has_family("Scoped"), "document scope must not copy the parent's aliases");
    let child_id = child.fonts.add_font_data("Scoped", 400, false, MONO).unwrap();
    engine.restyle(child);
    let child_before = engine.snapshot(child);
    assert_ne!(parent_id, child_id);
    let mut other = page(&engine, "<p>new document</p>", "Scoped", SANS);
    engine.restyle(&mut other);
    let child = parent.frames.children.first_mut().unwrap().page.as_mut().unwrap();
    engine.restyle(child);
    assert_eq!(engine.snapshot(child), child_before);
    assert_eq!(child.fonts.select_family(&["Scoped".into()], Family::Sans, 400, false), child_id);
    assert_eq!(parent.fonts.select_family(&["Scoped".into()], Family::Sans, 400, false), parent_id);
}

#[test]
fn late_host_registration_recomputes_retained_unit_geometry() {
    let mut engine = make_engine();
    engine.scripting = false;
    let mut a = engine.load_html("<!doctype html><style>#units{font:20px Late;width:10ch;height:1ex}</style><div id=units>WWWiii</div>", Url::parse("https://example.test/late-font").unwrap(), VIEWPORT);
    let node = a.doc.element_by_id("units").unwrap();
    let before = a.rect_of(node).unwrap();
    engine.fonts.add_font_data("Late", 400, false, MONO).unwrap();
    engine.restyle(&mut a);
    let after = a.rect_of(node).unwrap();
    assert_ne!(before.w, after.w);
    let expected = a.fonts.unit_metrics(&["Late".into()], Family::Sans, 400, false).0 * 20.0 * 10.0;
    assert!((after.w - expected).abs() < 0.02);
    engine.check_incremental(&mut a).unwrap();
}
