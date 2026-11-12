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

fn load_doc(engine: &mut Engine, html: &str) -> Page {
    let url = Url::parse("https://example.test/").unwrap();
    engine.load_html(html, url, Viewport { width: 800.0, height: 600.0 })
}

fn eval(engine: &Engine, page: &mut Page, js: &str) -> String {
    engine.eval_in_page(page, js).trim_matches(|c| c == '\'' || c == '"').to_string()
}

/// Server-rendered head content is hydrated by walking siblings from a
/// comment marker by count (Svelte 5 `<svelte:head>`): the whitespace text
/// nodes between head elements must exist, as in browsers.
#[test]
fn head_hydration_walk_finds_the_ssr_nodes() {
    let mut e = engine(true);
    let mut page = load_doc(
        &mut e,
        "<!doctype html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<!--h1--><!--[0--><link rel=\"canonical\" href=\"/\"><!--]--> <meta name=\"a\" content=\"one\"> <meta name=\"b\" content=\"two\"><!----><title>T</title>\n</head>\n<body><div style=\"display:contents\"><!--[--><p>ssr</p><!--]--></div>
         <script>
           var first = Object.getOwnPropertyDescriptor(Node.prototype, 'firstChild').get;
           var next = Object.getOwnPropertyDescriptor(Node.prototype, 'nextSibling').get;
           function sib(n, k) { while (k--) n = next.call(n); return n; }
           var a = first.call(document.head);
           while (a !== null && (a.nodeType !== 8 || a.data !== 'h1')) a = next.call(a);
           var r = next.call(a);
           var meta = sib(r, 4);
           var meta2 = sib(r, 6);
           window.result = [meta.getAttribute('content'), meta2.getAttribute('content'), sib(r, 7).nodeType].join(',');
         </script></body></html>",
    );
    assert_eq!(eval(&e, &mut page, "result"), "one,two,8");
}

/// `MessageChannel` (schedulers of large apps post to themselves through
/// one) delivers asynchronously with a structured clone; `BroadcastChannel`
/// reaches the other instances of its name; `structuredClone` keeps types.
#[test]
fn message_channel_and_structured_clone() {
    let mut e = engine(true);
    let mut page = load_doc(
        &mut e,
        "<script>window.log = [];
           var ch = new MessageChannel();
           ch.port1.onmessage = function (ev) { log.push('p1:' + ev.data.n + ':' + (ev.data.d instanceof Date)); };
           ch.port2.postMessage({ n: 1, d: new Date(0) });
           log.push('sync');
           var a = new BroadcastChannel('x'), b = new BroadcastChannel('x');
           b.addEventListener('message', function (ev) { log.push('bc:' + ev.data); });
           a.postMessage('hi');
           var m = structuredClone(new Map([[1, { s: new Set([2]) }]]));
           log.push('clone:' + (m.get(1).s.has(2)));
         </script>",
    );
    for _ in 0..5 {
        e.tick(&mut page);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(eval(&e, &mut page, "log.join(',')"), "sync,clone:true,p1:1:true,bc:hi");
}

/// Internal prototypes looked up by name (NamedNodeMap, NodeList, ...)
/// survive garbage collection while no JS object uses them.
#[test]
fn internal_prototypes_survive_gc() {
    let mut e = engine(true);
    let mut page = load_doc(
        &mut e,
        "<p id=x class=a>x</p><script>
           var junk = [];
           for (var i = 0; i < 400000; i++) { junk.push({ i: i }); if (junk.length > 1000) junk = []; }
           window.r = [typeof document.getElementById('x').attributes.getNamedItem, document.getElementById('x').attributes.getNamedItem('class').value, document.querySelectorAll('p').length].join();
         </script>",
    );
    assert_eq!(eval(&e, &mut page, "r"), "function,a,1");
}

/// `getBoundingClientRect()` of an inline element is the box of its own
/// text, not its block container's (links in a nav bar were reported as
/// the full-width line, so a click at their "center" hit the wrong link).
#[test]
fn inline_element_rect_is_its_fragments() {
    let mut e = engine(true);
    let mut page = load(
        &mut e,
        r#"<nav id=nav><a id=a href="/x">Home</a> <a id=b href="/y"><b>About</b> us</a></nav><p><span id=hidden style="display:none">x</span></p>"#,
    );
    let r = eval(&e, &mut page, "(function(){ var n = document.getElementById('nav').getBoundingClientRect(), a = document.getElementById('a').getBoundingClientRect(), b = document.getElementById('b').getBoundingClientRect(); return [n.width === 800, a.left === 0, a.width > 10 && a.width < 100, b.left > a.right, b.width > 10 && b.width < 200, a.height > 10 && a.height < 30].join(); })()");
    assert_eq!(r, "true,true,true,true,true,true");
    let (x, y) = {
        let b = page.rect_of(by_id(&page, "b")).unwrap();
        (b.x + b.w / 2.0, b.y + b.h / 2.0)
    };
    let hit = page.node_at_viewport(x, y).unwrap();
    let mut cur = Some(hit);
    let mut found = false;
    while let Some(c) = cur {
        if c == by_id(&page, "b") {
            found = true;
        }
        cur = page.doc.parent(c);
    }
    assert!(found, "center of #b hits #b");
}

/// Viewport scrolling fires `scroll` at the document (bubbling to the
/// window), so `document.addEventListener('scroll', ...)` feeds work.
#[test]
fn page_scroll_event_reaches_document_and_window_listeners() {
    let mut e = engine(true);
    let mut page = load(
        &mut e,
        "<div style='height:3000px'></div><script>var log = [];\
         document.addEventListener('scroll', function (ev) { log.push('d:' + (ev.target === document) + ':' + ev.bubbles + ':' + scrollY); });\
         window.addEventListener('scroll', function (ev) { log.push('w:' + (ev.target === document) + ':' + scrollY); });\
         window.onscroll = function () { log.push('on'); };</script>",
    );
    e.set_page_scroll(&mut page, 0.0, 700.0);
    let _ = e.dispatch_window_event(&mut page, "scroll");
    assert_eq!(e.eval_in_page(&mut page, "log.join(',')"), "'d:true:true:700,w:true:700,on'");
}

/// Scrolling an inner scroll container re-runs IntersectionObservers
/// rooted in it (the `scroll` event invalidates the rendering).
#[test]
fn scroll_container_scroll_updates_intersection_observers() {
    let mut e = engine(true);
    let mut page = load(
        &mut e,
        "<div id=box style='height:200px;overflow:auto'><div style='height:1000px'></div><div id=s style='height:10px'></div></div>\
         <script>var log = []; new IntersectionObserver(function (es) { es.forEach(function (x) { log.push(x.isIntersecting); }); }, { root: document.getElementById('box') }).observe(document.getElementById('s'));</script>",
    );
    let tick = |e: &Engine, page: &mut Page| {
        for _ in 0..5 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            let _ = e.tick(page);
        }
    };
    tick(&e, &mut page);
    assert_eq!(e.eval_in_page(&mut page, "log.join(',')"), "'false'");
    assert!(e.scroll_at(&mut page, 50.0, 50.0, 0.0, 900.0));
    let bx = by_id(&page, "box");
    let _ = e.dispatch_event(&mut page, bx, "scroll", rabbithole_engine::script::EventInit::simple(false, false));
    tick(&e, &mut page);
    assert_eq!(e.eval_in_page(&mut page, "log.join(',')"), "'false,true'");
}

/// `<link>` elements fire `load` once their stylesheet is fetched (`error`
/// when the fetch fails), parser-inserted or added by script, and
/// `modulepreload` links fire `load` too. Vite's preload helper (SvelteKit
/// apps) awaits a stylesheet link's `load` before importing the module
/// that needs it: without the event the app never starts.
#[test]
fn link_elements_fire_load_and_error_events() {
    let mut e = engine(true);
    let mut page = load(
        &mut e,
        r#"<link rel="stylesheet" href="data:text/css,%23p{color:red}" onload="log.push('parsed')">
           <script>var log = [];
             function add(rel, href) {
               var l = document.createElement('link'); l.rel = rel; l.href = href;
               l.onload = function () { log.push(rel + ':load'); };
               l.onerror = function () { log.push(rel + ':error'); };
               document.head.appendChild(l);
             }
             add('stylesheet', 'data:text/css,p{margin:0}');
             add('modulepreload', 'data:text/javascript,export%20default%201');
             add('stylesheet', 'file:///nonexistent-rabbithole-dir/missing.css');
           </script><p id=p>x</p>"#,
    );
    for _ in 0..5 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        let _ = e.tick(&mut page);
    }
    let log = e.eval_in_page(&mut page, "log.slice().sort().join(',')");
    assert_eq!(log, "'modulepreload:load,parsed,stylesheet:error,stylesheet:load'");
    // Each link fires once.
    for _ in 0..3 {
        let _ = e.tick(&mut page);
    }
    assert_eq!(e.eval_in_page(&mut page, "log.length"), "4");
}

/// Grid tracks sized with math functions mixing percentages and lengths
/// resolve against the grid container: svelte-split-pane's
/// `grid-template-columns: clamp(var(--min), var(--pos), var(--max)) 1fr`
/// (the Svelte playground's editor | preview split) was dropped as invalid,
/// stacking both panes in one column.
#[test]
fn grid_tracks_with_mixed_math_functions() {
    let mut e = engine(false);
    let page = load(
        &mut e,
        r#"<style>x-pane{--c:clamp(var(--min), var(--pos), var(--max));width:800px;height:100px;display:grid;grid-template-columns:var(--c) 1fr}
           #g{display:grid;width:800px;height:50px;grid-template-columns:calc(25% + 10px) minmax(calc(10% - 5px), 1fr) min(50%, 100px)}</style>
           <x-pane style="--pos: 50%; --min: 100px; --max: calc(100% - 4rem)"><div id=a></div><div id=b></div></x-pane>
           <x-pane style="--pos: 99%; --min: 100px; --max: calc(100% - 4rem)"><div id=c></div><div id=d></div></x-pane>
           <div id=g><div id=g1></div><div id=g2></div><div id=g3></div></div>"#,
    );
    let rect = |id: &str| page.rect_of(by_id(&page, id)).expect("laid out");
    assert_eq!((rect("a").x, rect("a").w, rect("b").x, rect("b").y), (0.0, 400.0, 400.0, rect("a").y));
    // Clamped to the maximum: 800 - 4rem.
    assert_eq!(rect("c").w, 736.0);
    assert_eq!((rect("g1").w, rect("g3").w), (210.0, 100.0));
    assert_eq!(rect("g2").x, 210.0);
}

/// An inline element wrapping a block (a lazy "partial" custom element with
/// a loading placeholder, slotted into a feed's shadow tree) is split around
/// the block; it has no box and no line fragments of its own, and its rect
/// used to fall back to the nearest ancestor box (the whole feed), so its
/// IntersectionObserver always reported it intersecting and infinite scroll
/// fetched page after page.
#[test]
fn inline_element_split_around_a_block_has_the_blocks_rect() {
    let mut e = engine(true);
    let mut page = load(
        &mut e,
        "<div id=feed><template shadowrootmode=open><div class=list><slot></slot></div></template>\
         <div style='height:2000px'>posts</div><x-partial id=xp><div id=ph style='height:30px'>Loading...</div></x-partial></div>\
         <span id=sp><div id=inner style='height:40px'>block in span</div></span>\
         <script>var log = []; new IntersectionObserver(function (es) { es.forEach(function (x) { log.push(x.isIntersecting); }); }).observe(document.getElementById('xp'));</script>",
    );
    for _ in 0..5 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        let _ = e.tick(&mut page);
    }
    let rect = |page: &Page, id: &str| page.rect_of(by_id(page, id)).expect("laid out");
    let (xp, ph) = (rect(&page, "xp"), rect(&page, "ph"));
    assert_eq!((xp.y, xp.h), (ph.y, ph.h), "partial {xp:?} placeholder {ph:?}");
    assert_eq!(ph.y, 2000.0);
    let (sp, inner) = (rect(&page, "sp"), rect(&page, "inner"));
    assert_eq!((sp.y, sp.h), (inner.y, inner.h), "span {sp:?} block {inner:?}");
    assert_eq!(eval(&e, &mut page, "(() => { const r = document.getElementById('xp').getBoundingClientRect(); return r.top + ':' + r.height; })()"), "2000:30");
    // Far below the 600px viewport: not intersecting.
    assert_eq!(eval(&e, &mut page, "log.join(',')"), "false");
}
