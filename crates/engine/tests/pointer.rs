//! Hit testing at viewport coordinates and the mouse event sequence /
//! default actions, through the same engine entry points the GUI uses
//! (`Page::node_at_viewport`, `Engine::pointer_move/down/up`, `click_at`).

use rabbithole_engine::net::Url;
use rabbithole_engine::{ClickAction, Engine, Modifiers, Page, Viewport};

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

fn id_at(page: &Page, x: f32, y: f32) -> Option<String> {
    let n = page.node_at_viewport(x, y)?;
    // Climb to the nearest element with an id.
    let mut cur = Some(n);
    while let Some(c) = cur {
        if let Some(id) = page.doc.attr(c, "id") {
            return Some(id.to_string());
        }
        cur = page.doc.parent(c);
    }
    None
}

fn eval(engine: &Engine, page: &mut Page, js: &str) -> String {
    engine.eval_in_page(page, js).trim_matches(|c| c == '\'' || c == '"').to_string()
}

#[test]
fn z_index_wins_over_tree_order() {
    let mut e = engine(false);
    let page = load(
        &mut e,
        r#"<div id="hi" style="position:absolute;left:0;top:0;width:200px;height:200px;z-index:2"></div>
           <div id="lo" style="position:absolute;left:0;top:0;width:200px;height:200px;z-index:1"></div>"#,
    );
    assert_eq!(id_at(&page, 50.0, 50.0).as_deref(), Some("hi"));
}

#[test]
fn later_absolute_sibling_is_on_top() {
    let mut e = engine(false);
    let page = load(
        &mut e,
        r#"<div id="a" style="position:absolute;left:0;top:0;width:200px;height:200px"></div>
           <div id="b" style="position:absolute;left:100px;top:0;width:200px;height:200px"></div>"#,
    );
    assert_eq!(id_at(&page, 150.0, 50.0).as_deref(), Some("b"));
    assert_eq!(id_at(&page, 50.0, 50.0).as_deref(), Some("a"));
}

#[test]
fn positioned_element_covers_later_static_content() {
    // A relatively positioned menu overlapping following in-flow content
    // is painted (and clicked) on top of it.
    let mut e = engine(false);
    let page = load(
        &mut e,
        r#"<div id="menu" style="position:relative;height:40px;margin-bottom:-40px"><a id="link" href="/x" style="display:block;height:40px">menu</a></div>
           <div id="content" style="height:100px;background:#eee"></div>"#,
    );
    assert_eq!(id_at(&page, 20.0, 10.0).as_deref(), Some("link"));
}

#[test]
fn float_is_above_later_block_backgrounds() {
    // A floated toolbar of links followed by an in-flow block that spans
    // the same line: the block's background paints below the float
    // (CSS 2.1 Appendix E), so the links stay clickable.
    let mut e = engine(false);
    let page = load(
        &mut e,
        r#"<div><div style="float:left;width:200px;height:30px"><a id="comments" href="/c" style="display:inline-block;width:150px;height:30px">7 comments</a></div>
           <div id="after" style="height:30px;background:#eee"><span style="margin-left:300px">tags</span></div></div>"#,
    );
    assert_eq!(id_at(&page, 20.0, 10.0).as_deref(), Some("comments"));
    assert_eq!(id_at(&page, 400.0, 10.0).as_deref(), Some("after"));
}

#[test]
fn negative_z_index_is_below_in_flow_content() {
    let mut e = engine(false);
    let page = load(
        &mut e,
        r#"<div id="bg" style="position:absolute;left:0;top:0;width:300px;height:300px;z-index:-1"></div>
           <div id="text" style="height:100px">hello</div>"#,
    );
    assert_eq!(id_at(&page, 20.0, 50.0).as_deref(), Some("text"));
    assert_eq!(id_at(&page, 20.0, 200.0).as_deref(), Some("bg"));
}

#[test]
fn overflow_hidden_clips_hits() {
    let mut e = engine(false);
    let page = load(
        &mut e,
        r#"<div id="clip" style="overflow:hidden;width:100px;height:100px">
             <div id="big" style="position:relative;width:400px;height:400px"></div>
           </div>
           <div id="below" style="height:200px"></div>"#,
    );
    assert_eq!(id_at(&page, 50.0, 50.0).as_deref(), Some("big"));
    // Outside the clip the overflowing child is neither painted nor hit.
    assert_eq!(id_at(&page, 50.0, 150.0).as_deref(), Some("below"));
    assert_ne!(id_at(&page, 300.0, 50.0).as_deref(), Some("big"));
}

#[test]
fn pointer_events_none_and_visibility_hidden_pass_through() {
    let mut e = engine(false);
    let page = load(
        &mut e,
        r#"<a id="under" href="/u" style="display:block;width:300px;height:300px">under</a>
           <div id="overlay" style="position:absolute;left:0;top:0;width:300px;height:300px;pointer-events:none"></div>
           <div id="ghost" style="position:absolute;left:0;top:0;width:100px;height:100px;visibility:hidden"></div>"#,
    );
    assert_eq!(id_at(&page, 50.0, 50.0).as_deref(), Some("under"));
    assert_eq!(id_at(&page, 200.0, 200.0).as_deref(), Some("under"));
}

#[test]
fn opacity_zero_overlay_is_still_hit() {
    let mut e = engine(false);
    let page = load(
        &mut e,
        r#"<div id="under" style="width:300px;height:300px"></div>
           <input id="file" type="checkbox" style="position:absolute;left:0;top:0;width:300px;height:300px;opacity:0;margin:0">"#,
    );
    assert_eq!(id_at(&page, 50.0, 50.0).as_deref(), Some("file"));
}

#[test]
fn transformed_box_is_hit_where_painted() {
    let mut e = engine(false);
    let page = load(&mut e, r#"<div id="t" style="width:100px;height:100px;transform:translate(300px,200px)"></div>"#);
    assert_eq!(id_at(&page, 350.0, 250.0).as_deref(), Some("t"));
    assert_ne!(id_at(&page, 50.0, 50.0).as_deref(), Some("t"));
}

#[test]
fn fixed_header_hit_at_viewport_coordinates_when_scrolled() {
    let mut e = engine(false);
    let mut page = load(
        &mut e,
        r#"<div id="hdr" style="position:fixed;top:0;left:0;right:0;height:50px;z-index:10"></div>
           <div id="tall" style="height:3000px"><div id="spacer" style="height:1000px"></div><div id="target" style="height:100px"></div></div>"#,
    );
    e.set_page_scroll(&mut page, 0.0, 1000.0);
    assert_eq!(id_at(&page, 100.0, 10.0).as_deref(), Some("hdr"));
    // Below the header: document y = 1000 + 60.
    assert_eq!(id_at(&page, 100.0, 60.0).as_deref(), Some("target"));
    // Document-coordinate API agrees.
    let n = page.node_at(100.0, 1060.0).unwrap();
    assert_eq!(page.doc.attr(n, "id"), Some("target"));
}

#[test]
fn link_spanning_lines_and_block_inside_link() {
    let mut e = engine(false);
    let page = load(
        &mut e,
        r#"<div style="width:120px"><a id="wrap" href="/w">one two three four five six seven</a></div>
           <a href="/b"><div id="blk" style="height:40px">block in link</div></a>"#,
    );
    // Second line of the wrapped link.
    let n = page.node_at_viewport(10.0, 30.0).unwrap();
    assert_eq!(page.link_of(n).unwrap().path, "/w");
    // Anywhere in the block inside the link (even past the text).
    let r = page.rect_of(page.doc.element_by_id("blk").unwrap()).unwrap();
    let n = page.node_at_viewport(700.0, r.y + 20.0).unwrap();
    assert_eq!(page.link_of(n).unwrap().path, "/b");
}

#[test]
fn click_follows_link_with_modifiers_and_target() {
    let mut e = engine(false);
    let mut page = load(
        &mut e,
        r#"<a id="a" href="next.html" style="display:block;height:30px">go</a><a id="b" href="/blank" target="_blank" style="display:block;height:30px">new</a>"#,
    );
    let up = e.click_at(&mut page, 5.0, 10.0, Modifiers::default());
    match up.action {
        ClickAction::Navigate { url, new_tab } => {
            assert_eq!(url.to_string(), "https://example.test/dir/next.html");
            assert!(!new_tab);
        }
        other => panic!("unexpected {other:?}"),
    }
    let up = e.click_at(&mut page, 5.0, 10.0, Modifiers { ctrl: true, ..Default::default() });
    assert!(matches!(up.action, ClickAction::Navigate { new_tab: true, .. }));
    let up = e.click_at(&mut page, 5.0, 40.0, Modifiers::default());
    assert!(matches!(up.action, ClickAction::Navigate { new_tab: true, .. }));
}

#[test]
fn mouse_event_sequence_and_coordinates() {
    let mut e = engine(true);
    let mut page = load(
        &mut e,
        r#"<div style="height:100px"></div>
           <button id="btn" style="margin-left:50px;width:100px;height:40px;border:2px solid black;padding:0">x</button>
           <div style="height:2000px"></div>
           <script>
             window.log = [];
             var b = document.getElementById('btn');
             ['pointerover','pointerenter','mouseover','mouseenter','pointermove','mousemove','pointerdown','mousedown','focus','pointerup','mouseup','click'].forEach(function (t) {
               b.addEventListener(t, function (ev) {
                 if (t === 'click') window.coords = [ev.clientX, ev.clientY, ev.pageX, ev.pageY, ev.offsetX, ev.offsetY, ev.button].join(',');
                 window.log.push(t);
               });
             });
           </script>"#,
    );
    // Scroll the page by 50px: the button's top is at document y ~100.
    e.set_page_scroll(&mut page, 0.0, 50.0);
    let r = page.rect_of(page.doc.element_by_id("btn").unwrap()).unwrap();
    let (vx, vy) = (r.x + 10.0, r.y - 50.0 + 10.0);
    let up = e.click_at(&mut page, vx, vy, Modifiers::default());
    assert_eq!(up.clicked, page.doc.element_by_id("btn"));
    let log = eval(&e, &mut page, "window.log.join(',')");
    assert_eq!(log, "pointerover,mouseover,pointerenter,mouseenter,pointermove,mousemove,pointerdown,mousedown,focus,pointerup,mouseup,click");
    let coords = eval(&e, &mut page, "window.coords");
    let expect = format!("{},{},{},{},8,8,0", vx, vy, vx, vy + 50.0);
    assert_eq!(coords, expect);
    assert_eq!(page.state.focused, page.doc.element_by_id("btn"));
}

#[test]
fn hover_enter_leave_bubbling_rules() {
    let mut e = engine(true);
    let mut page = load(
        &mut e,
        r#"<div id="outer" style="height:200px"><div id="inner" style="height:100px"></div></div><div id="other" style="height:100px"></div>
           <script>
             window.log = [];
             ['outer','inner','other'].forEach(function (id) {
               var el = document.getElementById(id);
               ['mouseenter','mouseleave','mouseover','mouseout'].forEach(function (t) {
                 el.addEventListener(t, function (ev) { if (ev.currentTarget === ev.target || t.indexOf('enter') > 0 || t.indexOf('leave') > 0) window.log.push(id + ':' + t); });
               });
             });
           </script>"#,
    );
    e.pointer_move(&mut page, 10.0, 50.0, Modifiers::default());
    e.pointer_move(&mut page, 10.0, 150.0, Modifiers::default());
    e.pointer_move(&mut page, 10.0, 250.0, Modifiers::default());
    let log = eval(&e, &mut page, "window.log.join(' ')");
    assert_eq!(
        log,
        "inner:mouseover outer:mouseenter inner:mouseenter inner:mouseout inner:mouseleave outer:mouseover outer:mouseout outer:mouseleave other:mouseover other:mouseenter"
    );
}

#[test]
fn onclick_attribute_and_prevent_default() {
    let mut e = engine(true);
    let mut page = load(
        &mut e,
        r#"<a id="a" href="/nav" onclick="window.hit = (window.hit||0) + 1; return false;" style="display:block;height:30px">a</a>
           <div id="d" onclick="window.divHit = true" style="height:30px;cursor:pointer">div</div>"#,
    );
    let up = e.click_at(&mut page, 5.0, 10.0, Modifiers::default());
    assert_eq!(up.action, ClickAction::None, "onclick returning false cancels navigation");
    assert_eq!(eval(&e, &mut page, "String(window.hit)"), "1");
    e.click_at(&mut page, 5.0, 40.0, Modifiers::default());
    assert_eq!(eval(&e, &mut page, "String(window.divHit)"), "true");
}

#[test]
fn label_toggles_checkbox_and_cancel_reverts() {
    let mut e = engine(true);
    let mut page = load(
        &mut e,
        r#"<label id="l" style="display:block;height:30px"><input id="c" type="checkbox"> remember me</label>
           <input id="c2" type="checkbox" style="display:block;width:20px;height:20px;margin:0">
           <script>
             window.changes = 0;
             document.getElementById('c').addEventListener('change', function () { window.changes++; });
             document.getElementById('c2').addEventListener('click', function (ev) { window.during = this.checked; ev.preventDefault(); });
           </script>"#,
    );
    let c = page.doc.element_by_id("c").unwrap();
    // Click on the label text (not the box).
    let up = e.click_at(&mut page, 150.0, 10.0, Modifiers::default());
    assert_eq!(up.action, ClickAction::Changed);
    assert!(page.doc.attr(c, "checked").is_some());
    assert_eq!(eval(&e, &mut page, "String(window.changes)"), "1");
    e.click_at(&mut page, 150.0, 10.0, Modifiers::default());
    assert!(page.doc.attr(c, "checked").is_none());
    // A cancelled click sees the new state during dispatch, then reverts.
    let c2 = page.doc.element_by_id("c2").unwrap();
    let r = page.rect_of(c2).unwrap();
    e.click_at(&mut page, r.x + 5.0, r.y + 5.0, Modifiers::default());
    assert_eq!(eval(&e, &mut page, "String(window.during)"), "true");
    assert!(page.doc.attr(c2, "checked").is_none());
}

#[test]
fn label_for_text_input_focuses_it() {
    let mut e = engine(false);
    let mut page = load(&mut e, r#"<label for="q" style="display:block;height:30px">Search</label><input id="q">"#);
    let up = e.click_at(&mut page, 5.0, 10.0, Modifiers::default());
    let q = page.doc.element_by_id("q").unwrap();
    assert_eq!(up.action, ClickAction::FocusControl(q));
    assert_eq!(page.state.focused, Some(q));
}

#[test]
fn summary_toggles_details() {
    let mut e = engine(false);
    let mut page = load(&mut e, r#"<details id="d"><summary style="display:block;height:30px">More</summary><p>hidden text</p></details>"#);
    let d = page.doc.element_by_id("d").unwrap();
    let up = e.click_at(&mut page, 5.0, 10.0, Modifiers::default());
    assert_eq!(up.action, ClickAction::Changed);
    assert!(page.doc.attr(d, "open").is_some());
    e.click_at(&mut page, 5.0, 10.0, Modifiers::default());
    assert!(page.doc.attr(d, "open").is_none());
}

#[test]
fn submit_button_and_disabled_button() {
    let mut e = engine(false);
    let mut page = load(
        &mut e,
        r#"<form id="f" action="/s"><input name="q" value="x"><button id="go" style="display:block;height:30px">Go<span> now</span></button><button id="off" disabled style="display:block;height:30px">Off</button></form>"#,
    );
    let go = page.doc.element_by_id("go").unwrap();
    let r = page.rect_of(go).unwrap();
    let up = e.click_at(&mut page, r.x + 5.0, r.y + 10.0, Modifiers::default());
    assert_eq!(up.action, ClickAction::Submit { form: page.doc.element_by_id("f").unwrap(), submitter: Some(go) });
    let off = page.rect_of(page.doc.element_by_id("off").unwrap()).unwrap();
    let up = e.click_at(&mut page, off.x + 5.0, off.y + 10.0, Modifiers::default());
    assert_eq!(up.action, ClickAction::None);
}

#[test]
fn element_from_point_uses_paint_order() {
    let mut e = engine(true);
    let mut page = load(
        &mut e,
        r#"<div id="top" style="position:absolute;left:0;top:0;width:100px;height:100px;z-index:5"></div><div id="under" style="height:300px"></div>"#,
    );
    assert_eq!(eval(&e, &mut page, "document.elementFromPoint(50, 50).id"), "top");
    assert_eq!(eval(&e, &mut page, "document.elementFromPoint(50, 200).id"), "under");
}
