//! Regression tests for rendering bugs found on real sites, each reduced
//! to a minimal local fixture.

use rabbithole_engine::dom::NodeId;
use rabbithole_engine::gfx::Color;
use rabbithole_engine::layout::DisplayItem;
use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Modifiers, Page, Viewport};

const CSS_RESET: &str = "<style>html,body{margin:0;padding:0;font-size:16px;line-height:20px}</style>";

fn load(engine: &mut Engine, body: &str) -> Page {
    let html = format!("<!doctype html><html><head>{CSS_RESET}</head><body>{body}</body></html>");
    let url = Url::parse("https://example.test/dir/page.html").unwrap();
    engine.load_html(&html, url, Viewport { width: 800.0, height: 600.0 })
}

fn engine(scripting: bool) -> Engine {
    let mut e = Engine::new_bundled_fonts_only();
    e.scripting = scripting;
    e.loader.load_scripts = scripting;
    e
}

fn by_id(page: &Page, id: &str) -> NodeId {
    page.doc.element_by_id(id).unwrap_or_else(|| panic!("no #{id}"))
}

fn background(page: &Page, id: &str) -> Color {
    page.styles.get(by_id(page, id)).expect("styled").background_color
}

fn painted_text(page: &Page) -> String {
    let mut out = String::new();
    for it in &page.display_list.items {
        if let DisplayItem::Text { text, .. } = it {
            out.push_str(text);
            out.push(' ');
        }
    }
    out
}

fn center(page: &Page, id: &str) -> (f32, f32) {
    let r = page.rect_of(by_id(page, id)).expect("laid out");
    (r.x + r.w / 2.0, r.y + r.h / 2.0)
}

const BLUE: Color = Color { r: 0, g: 0, b: 255, a: 255 };
const GREEN: Color = Color { r: 0, g: 128, b: 0, a: 255 };

/// Clicking a text field must not paint every ancestor with the `:focus`
/// style: `:focus` and `:focus-visible` match the focused element only,
/// `:focus-within` its ancestors.
#[test]
fn focus_matches_only_the_focused_element() {
    let mut e = engine(false);
    let mut page = load(
        &mut e,
        r#"<style>div:focus, textarea:focus { background: blue } div:focus-visible { background: blue }
           #outer:focus-within { background: green } button:focus-visible { background: blue }</style>
           <div id="outer"><div id="inner" style="padding:10px"><textarea id="q" style="width:200px;height:40px"></textarea></div></div>
           <button id="b" style="width:80px;height:30px">go</button>"#,
    );
    let (x, y) = center(&page, "q");
    e.click_at(&mut page, x, y, Modifiers::default());
    assert_eq!(page.state.focused, Some(by_id(&page, "q")));
    assert_eq!(background(&page, "q"), BLUE, "focused element matches :focus");
    assert_ne!(background(&page, "inner"), BLUE, "ancestors do not match :focus / :focus-visible");
    assert_eq!(background(&page, "outer"), GREEN, ":focus-within matches ancestors");
    // A button focused by a mouse press does not show :focus-visible.
    let (x, y) = center(&page, "b");
    e.click_at(&mut page, x, y, Modifiers::default());
    assert_eq!(page.state.focused, Some(by_id(&page, "b")));
    assert_ne!(background(&page, "b"), BLUE);
    assert_ne!(background(&page, "outer"), GREEN);
}

/// Text typed into a `<textarea>` (kept as its dirty value) is painted.
#[test]
fn textarea_paints_its_edited_value() {
    let mut e = engine(false);
    let mut page = load(&mut e, r#"<form><textarea id="q" name="q" style="width:300px;height:40px">default</textarea></form>"#);
    assert!(painted_text(&page).contains("default"));
    let q = by_id(&page, "q");
    page.set_control_text(q, "rabbit holes");
    e.restyle(&mut page);
    let text = painted_text(&page);
    assert!(text.contains("rabbit holes"), "painted: {text}");
    assert!(!text.contains("default"), "painted: {text}");
}

/// A closed `<details>` shows only its summary, even when the author gives
/// the content a `display` (a language menu built as `details > div`).
#[test]
fn closed_details_hides_content_with_author_display() {
    let mut e = engine(false);
    let mut page = load(
        &mut e,
        r#"<style>.menu { position: relative } .menu summary { list-style: none; width: 60px }
           .options { position: absolute; top: 100%; right: 0; display: grid; min-width: 190px }</style>
           <details class="menu" id="d"><summary id="s">EN</summary><div class="options"><a href="/en/">English</a><a href="/es/">Espanol</a></div></details>"#,
    );
    let text = painted_text(&page);
    assert!(text.contains("EN"), "painted: {text}");
    assert!(!text.contains("English"), "closed details content painted: {text}");
    // Clicking the summary opens it.
    let (x, y) = center(&page, "s");
    e.click_at(&mut page, x, y, Modifiers::default());
    assert!(page.doc.attr(by_id(&page, "d"), "open").is_some());
    assert!(painted_text(&page).contains("English"));
}

/// A one-row `<textarea>` styled like a search box (padding-top, border,
/// flex item stretched to its row): `rows` gives an intrinsic height, not
/// a `height` value, so the box stretches and the typed text is visible.
#[test]
fn textarea_rows_is_an_intrinsic_height() {
    let mut e = engine(false);
    let mut page = load(
        &mut e,
        r#"<style>.row { display: flex; height: 50px } .q { display: flex; flex: 100%; line-height: 22px; padding: 14px 0 0; border: none; border-bottom: 8px solid transparent; box-sizing: border-box; overflow: hidden }</style>
           <div class="row"><textarea id="q" class="q" rows="1"></textarea></div>
           <textarea id="plain" rows="3" style="line-height:20px;padding:0;border:0;box-sizing:content-box"></textarea>"#,
    );
    let q = by_id(&page, "q");
    page.set_control_text(q, "typed");
    e.restyle(&mut page);
    let r = page.rect_of(q).unwrap();
    assert!((r.h - 50.0).abs() < 0.5, "stretched textarea height {}", r.h);
    let mut found = false;
    for it in &page.display_list.items {
        if let DisplayItem::Text { text, y, .. } = it {
            if text.contains("typed") {
                found = true;
                assert!(*y > r.y + 14.0 && *y < r.y + r.h - 8.0, "baseline {y} outside the textarea content box {r:?}");
            }
        }
    }
    assert!(found);
    let plain = page.rect_of(by_id(&page, "plain")).unwrap();
    assert!((plain.h - 60.0).abs() < 0.5, "rows=3 height {}", plain.h);
}
