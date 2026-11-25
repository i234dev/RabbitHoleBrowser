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
        face: FontFace { family: family.into(), sources: vec![], weight: (400, 400), italic: false, stretch: (100, 100), display: "auto".into(), unicode_range: None, feature_settings: vec![], metric_overrides: Default::default() },
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
    let mut wrong = rabbithole_engine::gfx::Canvas::new(VIEWPORT.width as u32, VIEWPORT.height as u32);
    rabbithole_engine::layout::paint(&a.display_list, &mut wrong, &engine.fonts, 0.0, 0.0);
    assert_ne!(before.rgba, wrong.to_rgba8(), "control must detect rendering private fallback with the host view");
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
    let html = format!("<!doctype html><style>body{{font:20px Scoped}}</style><p>WWWiii</p><iframe srcdoc='<!doctype html><style>body{{font:20px Scoped}}</style><p>WWWiii {PRIVATE_CHAR}</p>'></iframe>");
    let mut parent = page(&engine, &html, "Scoped", SANS);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while parent.frames.children.first().and_then(|f| f.page.as_ref()).is_none() && std::time::Instant::now() < deadline {
        engine.tick(&mut parent);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let parent_id = parent.fonts.select_family(&["Scoped".into()], Family::Sans, 400, false);
    let child = parent.frames.children.first_mut().and_then(|f| f.page.as_mut()).expect("srcdoc child");
    assert!(!child.fonts.has_family("Scoped"), "document scope must not copy the parent's aliases");
    let child_id = child.fonts.add_font_data("Scoped", 400, false, MONO).unwrap();
    child.fonts.add_font_data("Child private fallback", 400, false, &private_font(MONO, Family::Mono)).unwrap();
    assert_ne!(parent_id, child_id);
    // Restyling the parent must notice its child's changed view, regenerate
    // the child layout, then repaint the cached iframe raster in the parent.
    engine.restyle(&mut parent);
    engine.present(&mut parent);
    let iframe = parent.frames.children[0].node;
    let child = parent.frames.children[0].page.as_ref().unwrap();
    let child_before = engine.snapshot(child);
    let composed = parent.images.get(&iframe).expect("composed child raster");
    assert_eq!(composed.pixels(), child_before.rgba.as_slice());
    let mut wrong = rabbithole_engine::gfx::Canvas::new(child_before.width, child_before.height);
    rabbithole_engine::layout::paint(&child.display_list, &mut wrong, &engine.fonts, 0.0, 0.0);
    assert_ne!(composed.pixels(), wrong.to_rgba8().as_slice(), "control must detect a base-context render_frame");
    let parent_before = engine.snapshot(&parent);
    let mut other = page(&engine, "<p>new document</p>", "Scoped", SANS);
    engine.restyle(&mut other);
    engine.restyle(&mut parent);
    let child = parent.frames.children[0].page.as_ref().unwrap();
    assert_eq!(engine.snapshot(child), child_before);
    assert_eq!(parent.images[&iframe].pixels(), child_before.rgba.as_slice());
    assert_eq!(engine.snapshot(&parent), parent_before);
    assert_eq!(child.fonts.select_family(&["Scoped".into()], Family::Sans, 400, false), child_id);
    assert_eq!(parent.fonts.select_family(&["Scoped".into()], Family::Sans, 400, false), parent_id);
}

#[test]
fn late_host_registration_recomputes_retained_unit_geometry() {
    let mut engine = make_engine();
    engine.scripting = false;
    let html = "<!doctype html><style>#units{font:20px Late;width:10ch;height:1ex}</style><div id=units>WWWiii</div><iframe srcdoc='<!doctype html><style>#units{font:20px Late;width:10ch;height:1ex}</style><div id=units>WWWiii</div>'></iframe>";
    let mut a = engine.load_html(html, Url::parse("https://example.test/late-font").unwrap(), VIEWPORT);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while a.frames.children.first().and_then(|f| f.page.as_ref()).is_none() && std::time::Instant::now() < deadline {
        engine.tick(&mut a);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let node = a.doc.element_by_id("units").unwrap();
    let before = a.rect_of(node).unwrap();
    let child = a.frames.children[0].page.as_ref().expect("late-font child");
    let child_node = child.doc.element_by_id("units").unwrap();
    let child_before = child.rect_of(child_node).unwrap();
    engine.fonts.add_font_data("Late", 400, false, MONO).unwrap();
    engine.restyle(&mut a);
    let after = a.rect_of(node).unwrap();
    assert_ne!(before.w, after.w);
    let expected = a.fonts.unit_metrics(&["Late".into()], Family::Sans, 400, false).0 * 20.0 * 10.0;
    assert!((after.w - expected).abs() < 0.02);
    let child = a.frames.children[0].page.as_ref().unwrap();
    let child_after = child.rect_of(child_node).unwrap();
    assert_ne!(child_before.w, child_after.w);
    assert!((child_after.w - expected).abs() < 0.02);
    engine.present(&mut a);
    let child = a.frames.children[0].page.as_ref().unwrap();
    let raster = a.images.get(&a.frames.children[0].node).unwrap();
    assert_eq!(raster.pixels(), engine.snapshot(child).rgba.as_slice());
    engine.check_incremental(&mut a).unwrap();
}

fn autospace_font() -> Vec<u8> {
    let fonts = FontContext::new();
    let face = fonts.select(Family::Mono, 400, false);
    let mappings = [(' ', ' '), ('0', '0'), ('A', 'A'), ('x', 'x'), ('水', 'W')];
    let mut cmap = vec![0, 0, 0, 1, 0, 3, 0, 10, 0, 0, 0, 12];
    cmap.extend_from_slice(&12u16.to_be_bytes());
    cmap.extend_from_slice(&0u16.to_be_bytes());
    for n in [16u32 + 12 * mappings.len() as u32, 0, mappings.len() as u32] {
        cmap.extend_from_slice(&n.to_be_bytes());
    }
    for (character, source) in mappings {
        let glyph = fonts.shape(face, 20.0, &source.to_string(), 0.0)[0].glyph as u32;
        for n in [character as u32, character as u32, glyph] {
            cmap.extend_from_slice(&n.to_be_bytes());
        }
    }
    let dir = sfnt::table_directory(MONO, 0).unwrap();
    let tables: Vec<_> = dir.iter().map(|t| {
        let bytes = if t.tag == *b"cmap" { cmap.as_slice() }
            else { &MONO[t.offset as usize..(t.offset + t.length) as usize] };
        (t.tag, bytes)
    }).collect();
    sfnt::build_sfnt(0x00010000, &tables)
}

#[test]
fn layout_autospace_uses_document_ic_after_style_scope_has_ended() {
    let engine = make_engine();
    let data = autospace_font();
    let html = "<!doctype html><style>span{display:inline-block;font:20px Scoped;white-space:nowrap}#spaced{text-autospace:normal}#plain{text-autospace:no-autospace}</style><span id=plain>A水A</span><span id=spaced>A水A</span>";
    let mut a = page(&engine, html, "Scoped", &data);
    let plain = a.doc.element_by_id("plain").unwrap();
    let spaced = a.doc.element_by_id("spaced").unwrap();
    let ic = a.fonts.unit_metrics(&["Scoped".into()], Family::Sans, 400, false).2;
    assert!((ic - 1.0).abs() > 0.1, "control must differ from default ic");
    let expected = 2.0 * 20.0 * ic / 8.0;
    let gap = |page: &Page| page.rect_of(spaced).unwrap().w - page.rect_of(plain).unwrap().w;
    assert!((gap(&a) - expected).abs() < 0.02, "gap={}, expected={expected}", gap(&a));
    let _b = page(&engine, "<p>another document</p>", "Scoped", SANS);
    a.viewport.width -= 1.0;
    engine.restyle(&mut a);
    assert!((gap(&a) - expected).abs() < 0.02);
    engine.check_incremental(&mut a).unwrap();
}

fn canvas_font_pixels(engine: &Engine, page: &mut Page, font: &str, stroke: bool) -> Vec<u8> {
    let operation = if stroke { "strokeText" } else { "fillText" };
    let code = format!(
        "(()=>{{let c=document.getElementById('canvas'),x=c.getContext('2d');x.clearRect(0,0,200,40);x.font=`{font}`;x.{operation}('WWWiii',0,25);return true}})()"
    );
    assert_eq!(engine.eval_in_page(page, &code), "true");
    let canvas = page.doc.element_by_id("canvas").unwrap();
    engine.present(page);
    page.images.get(&canvas).expect("Canvas text pixels").pixels().to_vec()
}

#[test]
fn canvas_named_font_fill_and_stroke_keep_order_quotes_and_retained_context() {
    let engine = make_engine();
    let html = "<!doctype html><canvas id=canvas width=200 height=40></canvas>";
    let mut a = page(&engine, html, "Scoped", MONO);
    let named_fill = canvas_font_pixels(&engine, &mut a, "20px Scoped", false);
    let generic_fill = canvas_font_pixels(&engine, &mut a, "20px sans-serif", false);
    assert_ne!(named_fill, generic_fill, "fillText must select the named font");
    let named_stroke = canvas_font_pixels(&engine, &mut a, "20px Scoped", true);
    assert_ne!(named_stroke, canvas_font_pixels(&engine, &mut a, "20px sans-serif", true), "strokeText must select the named font");
    assert_eq!(named_fill, canvas_font_pixels(&engine, &mut a, "20px Missing, Scoped, serif", false));
    assert_eq!(canvas_font_pixels(&engine, &mut a, "20px serif", false), canvas_font_pixels(&engine, &mut a, "20px Missing, serif, Scoped", false));
    a.fonts.add_font_data("serif", 400, false, MONO).unwrap();
    assert_eq!(named_fill, canvas_font_pixels(&engine, &mut a, "20px 'serif'", false));
    assert_ne!(named_fill, canvas_font_pixels(&engine, &mut a, "20px serif", false));
    // Keep the actual Canvas context and its font state in A's live realm.
    assert_eq!(engine.eval_in_page(&mut a, "globalThis.kept=document.getElementById('canvas').getContext('2d');kept.font='20px Scoped';globalThis.oldWidth=kept.measureText('WWWiii').width;true"), "true");
    let mut b = page(&engine, html, "Scoped", SANS);
    assert_ne!(named_fill, canvas_font_pixels(&engine, &mut b, "20px Scoped", false));
    assert_eq!(engine.eval_in_page(&mut a, "kept.clearRect(0,0,200,40);kept.fillText('WWWiii',0,25);kept.measureText('WWWiii').width===oldWidth"), "true");
    let canvas = a.doc.element_by_id("canvas").unwrap();
    assert_eq!(a.images[&canvas].pixels(), named_fill.as_slice());
}
