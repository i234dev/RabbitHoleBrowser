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

/// Script scrolling (`scrollTo`, `scrollBy`, `scrollTop =`) fires one
/// `scroll` event at the document on the next task turn, as the viewport
/// does; infinite lists that load on `scroll` depend on it. Several moves
/// in one turn are coalesced and a call that does not move fires nothing.
#[test]
fn script_scrolling_queues_one_scroll_event_per_turn() {
    let mut e = engine(true);
    let mut page = load(
        &mut e,
        "<div style='height:3000px'></div><script>var n = 0, w = 0;\
         document.addEventListener('scroll', function () { n++; });\
         window.addEventListener('scroll', function () { w++; });</script>",
    );
    assert_eq!(e.eval_in_page(&mut page, "scrollTo(0, 800); scrollBy(0, 100); document.documentElement.scrollTop = 500; [n, w, scrollY].join()"), "'0,0,500'");
    assert!(page.next_script_due().is_some(), "a pending scroll event asks for a turn");
    let _ = e.tick(&mut page);
    assert_eq!(e.eval_in_page(&mut page, "[n, w].join()"), "'1,1'");
    // No movement, no event.
    assert_eq!(e.eval_in_page(&mut page, "scrollTo(0, 500); 0"), "0");
    let _ = e.tick(&mut page);
    assert_eq!(e.eval_in_page(&mut page, "[n, w].join()"), "'1,1'");
    // Standards mode: the document scrolls through <html>, not <body>.
    assert_eq!(e.eval_in_page(&mut page, "[document.compatMode, document.documentElement.scrollTop, document.body.scrollTop].join()"), "'CSS1Compat,500,0'");
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

/// Parser-inserted external scripts fire `load` after they run (`error`
/// when the fetch fails), as "execute the script element" says. Module
/// loaders that find their bundles already in the document wait for these
/// events before resolving the modules defined in them.
#[test]
fn parser_inserted_external_scripts_fire_load_and_error() {
    let mut e = engine(true);
    let mut page = load(
        &mut e,
        r#"<script>var log = []; document.addEventListener('load', function (ev) { if (ev.target.id) log.push('capture:' + ev.target.id); }, true);</script>
           <script id=ok src="data:text/javascript,log.push('ran')" onload="log.push('load')"></script>
           <script id=bad src="file:///nonexistent-rabbithole-dir/missing.js" onerror="log.push('error')" onload="log.push('bad-load')"></script>"#,
    );
    for _ in 0..3 {
        let _ = e.tick(&mut page);
    }
    assert_eq!(e.eval_in_page(&mut page, "log.join(',')"), "'ran,capture:ok,load,error'");
}

/// A stylesheet `<link>` inserted by a script that then forces a layout
/// (the "media swap" loader: clone a `media=print` link, set `media=all`,
/// replace the original, await the clone's `load`) still fires `load`: the
/// forced flush that collected the new link ran while the script host was
/// busy, and later flushes never looked at the link again.
#[test]
fn link_load_fires_after_forced_layout_in_inserting_script() {
    let mut e = engine(true);
    let mut page = load(
        &mut e,
        r#"<link rel="stylesheet" href="data:text/css,p{margin:0}" media="print" data-swap>
           <p id=p>x</p>
           <script>var log = [];
             var o = document.querySelector('[data-swap]'), l = o.cloneNode();
             l.media = 'all';
             l.addEventListener('load', function () { log.push('load'); });
             o.replaceWith(l);
             log.push('h' + (document.getElementById('p').offsetHeight > 0));
           </script>"#,
    );
    for _ in 0..5 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        let _ = e.tick(&mut page);
    }
    assert_eq!(e.eval_in_page(&mut page, "log.join(',')"), "'htrue,load'");
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

/// With text before and after the block, the split inline's rect is the
/// union of all its fragments (CSSOM View, as in Chrome): the "before" line,
/// the anonymous block wrapper holding the block and the "after" line; and
/// `getClientRects()` lists those three fragments, whose union is the
/// bounding rect.
#[test]
fn split_inline_rect_is_the_union_of_its_fragments() {
    let mut e = engine(true);
    let mut page = load(
        &mut e,
        r#"<style>body{margin:0;font:16px/20px sans-serif}</style><span id=s>before<div id=d>Block</div>after</span>"#,
    );
    let r = eval(&e, &mut page, "(function(){ var s = document.getElementById('s').getBoundingClientRect(), d = document.getElementById('d').getBoundingClientRect(); return [s.left, s.width, s.top >= 0 && s.top < 2, s.bottom > 58 && s.bottom <= 60, d.top].join(); })()");
    // Inline fragments are their font's content area (ascent + descent) inside
    // the 20px lines, so the union starts just below 0 and ends just above 60.
    assert_eq!(r, "0,800,true,true,20");
    let r = eval(&e, &mut page, "(function(){ var rs = document.getElementById('s').getClientRects(), b = document.getElementById('s').getBoundingClientRect(), l = 1e9, t = 1e9, rr = -1e9, bb = -1e9; for (var i = 0; i < rs.length; i++) { l = Math.min(l, rs[i].left); t = Math.min(t, rs[i].top); rr = Math.max(rr, rs[i].right); bb = Math.max(bb, rs[i].bottom); } return [rs.length === 3, l === b.left, t === b.top, rr === b.right, bb === b.bottom].join(); })()");
    assert_eq!(r, "true,true,true,true,true");
}

/// `calc(% ± length)` and min()/max()/clamp() mixing percentages and
/// lengths resolve against the containing block at layout (a header at
/// `calc(40% - 140px)` used to get 40%), for widths, heights, margins,
/// padding, insets and background positions; percentages of an indefinite
/// height behave as `auto`. getComputedStyle returns used px for box
/// properties (against the containing block) and keeps calc() for
/// background-position; the inline style keeps the specified value.
#[test]
fn calc_and_math_functions_mix_percentages_and_lengths() {
    let mut e = engine(true);
    let mut page = load(
        &mut e,
        r#"<div id=cb style="width:600px;height:400px;position:relative">
             <div id=w style="width:calc(100% - 20px);height:calc(50% + 10px);margin-left:calc(10% - 5px);padding-top:calc(5% + 2px)"></div>
             <div id=m style="width:min(100%, 500px);height:max(10%, 30px)"></div>
             <div id=c style="width:clamp(100px, 50%, 250px);height:10px"></div>
             <div id=n style="width:calc(100% - max(10%, 100px));height:min(calc(100% - 380px), 50px)"></div>
             <div id=a style="position:absolute;left:calc(50% - 50px);top:calc(25% + 5px);width:100px;height:10px"></div>
             <div id=bg style="width:100px;height:10px;background-position:calc(100% - 20px) 50%"></div>
           </div>
           <div id=narrow style="width:300px"><div id=m2 style="width:min(100%, 500px)">x</div></div>
           <div id=auto><div id=ih style="height:calc(50% + 10px)">line</div></div>"#,
    );
    let r = |id: &str| page.rect_of(by_id(&page, id)).expect("laid out");
    // 600 - 20 wide; 200 + 10 tall plus padding-top 5% of 600 + 2 = 32.
    let w = r("w");
    assert_eq!((w.w, w.h, w.x), (580.0, 242.0, 55.0));
    assert_eq!((r("m").w, r("m").h), (500.0, 40.0));
    assert_eq!(r("m2").w, 300.0);
    assert_eq!(r("c").w, 250.0);
    assert_eq!((r("n").w, r("n").h), (500.0, 20.0));
    let (a, cb) = (r("a"), r("cb"));
    assert_eq!((a.x - cb.x, a.y - cb.y), (250.0, 105.0));
    // Percentage of an indefinite height: auto (one 20px line).
    assert_eq!(r("ih").h, 20.0);
    let cs = eval(&e, &mut page, "(function(){ var g = function (id, p) { return getComputedStyle(document.getElementById(id)).getPropertyValue(p); }; return [g('w', 'width'), g('w', 'height'), g('w', 'margin-left'), g('w', 'padding-top'), g('m', 'width'), g('a', 'left'), g('a', 'top'), g('bg', 'background-position'), document.getElementById('w').style.width, document.getElementById('m').style.width].join('|'); })()");
    assert_eq!(cs, "580px|210px|55px|32px|500px|250px|105px|calc(100% - 20px) 50%|calc(100% - 20px)|min(100%, 500px)");
}

/// A page split into stacked bands sized with `calc(% - px)` of a 100%
/// tall body: the bands fill the viewport exactly (the percentage part used
/// to be taken alone, 40% instead of 40% - 140px, and the page scrolled).
#[test]
fn calc_percent_heights_fill_the_viewport_exactly() {
    let mut e = engine(true);
    let html = "<!doctype html><html><head><style>html,body{height:100%} body{margin:0;min-height:600px} .top{height:100px;height:calc(40% - 140px)} .bottom{height:calc(60% - 210px)} .center{height:350px}</style></head>\
                <body><div class=top id=top></div><div class=center id=center></div><div class=bottom id=bottom></div></body></html>";
    let mut page = e.load_html(html, Url::parse("https://example.test/").unwrap(), Viewport { width: 800.0, height: 900.0 });
    let r = |id: &str| page.rect_of(by_id(&page, id)).expect("laid out");
    assert_eq!((r("top").h, r("center").h, r("bottom").h), (220.0, 350.0, 330.0));
    assert_eq!(r("bottom").y + r("bottom").h, 900.0);
    assert_eq!(eval(&e, &mut page, "String(document.documentElement.scrollHeight)"), "900");
}

/// A media element's `textTracks` lists its `<track>` children's tracks
/// and is indexable (svelte.dev sets `video.textTracks[0].mode` on startup;
/// the list was always empty, so it threw and showed its error screen).
#[test]
fn media_text_tracks_include_track_children() {
    let mut e = engine(true);
    let mut page = load_doc(
        &mut e,
        "<video id=v><track kind=captions srclang=en default><track id=t2 kind=bogus></video><script>
           var v = document.getElementById('v');
           var tt = v.textTracks;
           var before = [tt.length, tt[0].mode, tt[0].kind, tt[1].kind, tt[1].mode, tt[1].id].join();
           tt[0].mode = 'hidden';
           var extra = v.addTextTrack('subtitles', 'x', 'fr');
           window.r = before + '|' + [v.textTracks.length, v.textTracks === tt, tt[0] === v.querySelector('track').track,
             tt[0].mode, tt[2] === extra, tt.getTrackById('t2') === tt[1]].join();
         </script>",
    );
    assert_eq!(eval(&e, &mut page, "r"), "2,showing,captions,metadata,disabled,t2|3,true,true,hidden,true,true");
}

#[test]
fn resolved_insets_follow_the_positioning_scheme() {
    let mut e = engine(true);
    let mut page = load(&mut e, r#"<div style="transform:scale(1);width:240px;height:120px;padding:10px">
        <div id=f style="position:fixed;top:25%;left:0;width:40px;height:10px"></div>
        <section style="width:160px"><div id=r style="position:relative;right:25%"></div></section>
        <div id=s style="top:30%"></div><div id=n style="display:none;position:absolute;left:10%"></div></div>"#);
    let out = eval(&e, &mut page, "[getComputedStyle(f).top,getComputedStyle(f).right,getComputedStyle(r).left,getComputedStyle(s).top,getComputedStyle(n).left].join('|')");
    assert_eq!(out, "35px|220px|-40px|30%|10%");
}

#[test]
fn css_style_property_detection_and_color_serialization() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<div id=box style='color:rgba(1,2,3,0.5);background-color:rgba(6,5,4,0.00392)'></div>");
    let out = eval(&e, &mut page, "var b=document.getElementById('box'),s=getComputedStyle(b); [s.color,s.backgroundColor,'color' in s,'background-color' in s,'fontSize' in b.style,'notAProperty' in s,'notAProperty' in b.style,'toString' in s].join('|')");
    assert_eq!(out, "rgba(1, 2, 3, 0.5)|rgba(6, 5, 4, 0.004)|true|true|true|false|false|true");
}

#[test]
fn inline_editing_preserves_unselected_text_and_caret_format() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<div id=ed contenteditable>abcdef</div>");
    let out = eval(&e, &mut page, r#"(()=>{
        const d=document,h=d.getElementById('ed'),s=getSelection();
        s.setBaseAndExtent(h.firstChild,1,h.firstChild,5);
        d.execCommand('bold');
        const before=h.innerHTML, on=d.queryCommandState('bold');
        const text=h.querySelector('b').firstChild;
        s.setBaseAndExtent(text,1,text,3);
        d.execCommand('bold');
        const after=h.innerHTML;
        s.collapse(h.lastChild,1);
        d.execCommand('italic');
        d.execCommand('insertText',false,'!');
        return [before,on,after,h.lastElementChild.outerHTML].join('|');
    })()"#);
    assert_eq!(out, "a<b>bcde</b>f|true|a<b>b</b>cd<b>e</b>f|<i>!</i>");
}

#[test]
fn keyboard_edits_the_selection_only_after_uncanceled_beforeinput() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<section contenteditable id=ed>xyz</section>");
    eval(&e,&mut page,r#"
        var ed=document.getElementById('ed'), events=[];
        ed.focus();getSelection().setBaseAndExtent(ed.firstChild,1,ed.firstChild,2);
        ed.addEventListener('beforeinput',e=>{events.push(e.type+' '+e.inputType);if(e.data==='!')e.preventDefault()});
        ed.addEventListener('input',e=>events.push(e.type+' '+e.isTrusted));
    "#);
    e.key_input(&mut page,"!","Digit1",true,Modifiers::default());
    e.key_input(&mut page,"0","Digit0",true,Modifiers::default());
    assert_eq!(eval(&e,&mut page,"ed.innerHTML+'|'+events.join('|')"),"x0z|beforeinput insertText|beforeinput insertText|input true");
}

#[test]
fn keyboard_target_ranges_snapshot_the_edit_and_clear_after_dispatch() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<section contenteditable>xyz</section>");
    eval(&e,&mut page,r#"
        var ed = document.querySelector('section'), node=ed.firstChild, seen=[], saved;
        ed.focus();
        getSelection().collapse(node,2);
        ed.onbeforeinput = e => {
            saved = e;
            const r = e.getTargetRanges()[0];
            seen.push(r instanceof StaticRange, r.startContainer === node, r.startOffset, r.endOffset);
            getSelection().collapse(node,3);
            seen.push(e.getTargetRanges()[0].endOffset);
        };
    "#);
    e.key_input(&mut page,"Backspace","Backspace",true,Modifiers::default());
    assert_eq!(eval(&e,&mut page,"[ed.textContent,...seen,saved.getTargetRanges().length].join('|')"), "xy|true|true|1|2|2|0");
    eval(&e,&mut page,"getSelection().removeAllRanges();seen.length=0");
    e.key_input(&mut page,"a","KeyA",true,Modifiers::default());
    assert_eq!(eval(&e,&mut page,"[ed.textContent,seen.length].join('|')"), "xy|0");
}

#[test]
fn computed_colors_preserve_coordinates_through_inheritance_and_shorthands() {
    let mut engine = engine(true);
    let mut page = load(&mut engine, "<section style='color:color(display-p3 120% 0 40% / 50%)'><i style='background:currentcolor'></i></section>");
    let result = eval(&engine, &mut page, r#"
        const el = document.querySelector('i');
        const colors = [getComputedStyle(el).color, getComputedStyle(el).backgroundColor];
        el.style.background = 'none';
        el.style.color = 'oklch(120% -2 1turn / none)';
        colors.push(getComputedStyle(el).color, getComputedStyle(el).backgroundColor);
        el.style.color = 'rgb(from rgb(51 102 153 / 0.8) r g b)';
        colors.push(getComputedStyle(el).color);
        colors.join('|')
    "#);
    assert_eq!(result, "color(display-p3 1.2 0 0.4 / 0.5)|color(display-p3 1.2 0 0.4 / 0.5)|oklch(1 0 0 / none)|rgba(0, 0, 0, 0)|color(srgb 0.2 0.4 0.6 / 0.8)");
}

#[test]
fn editing_applies_color_and_font_values_to_ranges_and_typed_text() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<p id=ed contenteditable>abc</p>");
    let out = eval(&e, &mut page, r#"(()=>{
        const d=document, sel=getSelection(), text=ed.firstChild;
        sel.setBaseAndExtent(text,2,text,1);
        d.execCommand('foreColor', false, 'blue');
        const color=d.queryCommandValue('foreColor'), backward=sel.anchorOffset>sel.focusOffset;
        sel.collapse(ed.lastChild,1);
        d.execCommand('fontSize', false, '+1');
        const size=d.queryCommandValue('fontSize');
        d.execCommand('insertText',false,'!');
        return [ed.innerHTML,color,backward,size,d.execCommand('foreColor',false,'invalid')].join('|');
    })()"#);
    assert_eq!(out, "a<font color=\"#0000ff\">b</font>c<font size=\"4\">!</font>|rgb(0, 0, 255)|true|4|false");
}

#[test]
fn peer_connections_deliver_over_an_authenticated_data_channel() {
    use std::time::{Duration, Instant};
    let mut e = engine(true);
    let mut page = load(&mut e, "<main></main>");
    eval(&e, &mut page, r#"
        var rtcResult = '', first, second = new RTCPeerConnection();
        (async () => {
            const algorithm = {name:'ECDSA',namedCurve:'P-256'};
            const expired = await RTCPeerConnection.generateCertificate({...algorithm,expires:0});
            let rejected = false;
            try { new RTCPeerConnection({certificates:[expired]}); } catch (e) { rejected = e.name === 'InvalidAccessError'; }
            if (!rejected) throw Error('Expired DTLS identity accepted');
            const certificate = await RTCPeerConnection.generateCertificate({...algorithm,expires:60000});
            first = new RTCPeerConnection({certificates:[certificate]});
            if (!(certificate instanceof RTCCertificate) || first.getConfiguration().certificates[0] !== certificate)
                throw Error('Configuration lost the supplied certificate');
            second.ondatachannel = e => {
                e.channel.onmessage = msg => e.channel.send(msg.data + '!');
            };
            const channel = first.createDataChannel('messages');
            channel.onopen = () => channel.send('hello');
            channel.onmessage = async e => {
                const stats = await first.getStats();
                const dc = [...stats.values()].find(r => r.type === 'data-channel');
                const pc = [...stats.values()].find(r => r.type === 'peer-connection');
                rtcResult = [channel.label, first.connectionState, e.data, stats instanceof RTCStatsReport,
                    stats.get(dc.id) === dc, pc.dataChannelsOpened, dc.messagesSent, dc.bytesSent,
                    dc.messagesReceived, dc.bytesReceived].join('|');
                first.close(); second.close();
            };
            await first.setLocalDescription(await first.createOffer());
            const fingerprint = certificate.getFingerprints()[0];
            if (!first.localDescription.sdp.toLowerCase().includes('a=fingerprint:' + fingerprint.algorithm + ' ' + fingerprint.value))
                throw Error('Offer fingerprint differs from supplied identity');
            await second.setRemoteDescription(first.localDescription);
            await second.setLocalDescription(await second.createAnswer());
            await first.setRemoteDescription(second.localDescription);
        })().catch(e => {rtcResult = String(e); first?.close(); second.close();});
    "#);
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        e.tick(&mut page);
        let result = eval(&e, &mut page, "rtcResult");
        if !result.is_empty() {
            assert_eq!(result, "messages|connected|hello!|true|true|1|1|5|1|6");
            break;
        }
        assert!(Instant::now() < deadline, "{}", eval(&e, &mut page,
            "[first?.signalingState,first?.iceConnectionState,first?.connectionState,second.connectionState].join('|')"));
        std::thread::sleep(Duration::from_millis(4));
    }
}

#[test]
fn css_color_layers_composite_in_paint_order_and_alpha_preserves_the_origin_space() {
    let mut e=engine(true);
    let mut page=load(&mut e,"<div id=box style='background-color:color-layers(multiply, yellow, cyan)'></div>");
    let result=eval(&e,&mut page,r#"(()=>{
        const bg=getComputedStyle(box).backgroundColor;
        box.style.color='alpha(from color(display-p3 1 0.2 0 / 0.5) / calc(alpha * 0.25))';
        const color=getComputedStyle(box).color;
        box.style.backgroundColor='color-layers(normal, red, blue)';
        return [bg,color,box.style.backgroundColor,getComputedStyle(box).backgroundColor].join('|');
    })()"#);
    assert_eq!(result,"rgb(0, 255, 0)|color(display-p3 1 0.2 0 / 0.125)|color-layers(red, blue)|rgb(255, 0, 0)");
}

#[test]
fn editing_aligns_only_selected_paragraphs_and_preserves_the_selection() {
    let mut e=engine(true);
    let mut page=load(&mut e,"<section contenteditable id=ed>plain<p>second</p><p>third</p></section>");
    let out=eval(&e,&mut page,r#"(()=>{
        const sel=getSelection(), a=ed.firstChild, b=ed.children[0].firstChild;
        sel.setBaseAndExtent(b,3,a,2);
        document.execCommand('justifyCenter');
        const aligned=[document.queryCommandValue('justifyCenter'),document.queryCommandState('justifyCenter'),
            getComputedStyle(ed.lastChild).textAlign,sel.anchorNode===b,sel.focusNode===a];
        sel.collapse(b,2);
        document.execCommand('formatBlock',false,'<h2>');
        return [...aligned,document.queryCommandValue('formatBlock'),sel.anchorNode===b,sel.anchorOffset].join('|');
    })()"#);
    assert_eq!(out,"center|true|start|true|true|h2|true|2");
}

#[test]
fn editing_rechecks_text_control_visibility_when_commands_run() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<section><textarea id=field>abc</textarea></section>");
    let result = eval(&e, &mut page, r#"(() => {
        field.focus();
        field.setSelectionRange(1, 2);
        const parent = field.parentElement;
        parent.style.display = 'none';
        const hidden = document.execCommand('delete');
        const unchanged = field.value;
        parent.style.display = 'block';
        field.focus();
        field.setSelectionRange(1, 2);
        const shown = document.execCommand('insertText', false, '!');
        return [hidden, unchanged, shown, field.value].join('|');
    })()"#);
    assert_eq!(result, "false|abc|true|a!c");
}

#[test]
fn alignment_cssom_preserves_modifiers_and_resolves_inherited_legacy_alignment() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<style>section { justify-items: right legacy } p { justify-items: legacy }</style><section><p id=box>x</p></section>");
    let result = eval(&e, &mut page, r#"(() => {
        const before = getComputedStyle(box).justifyItems;
        box.style.placeContent = 'first baseline';
        box.style.placeContent = 'baseline unsafe stretch';
        box.style.alignSelf = 'unsafe flex-end';
        const resolved = getComputedStyle(box);
        return [before, box.style.placeContent, resolved.placeContent, box.style.alignSelf,
            resolved.alignSelf, CSS.supports('align-content', 'safe self-end')].join('|');
    })()"#);
    assert_eq!(result, "legacy right|baseline start|baseline start|unsafe flex-end|unsafe flow-end|false");
}

#[test]
fn font_size_commands_validate_the_entire_numeric_argument() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<p id=ed contenteditable>type</p>");
    let out = eval(&e, &mut page, r#"(() => {
        getSelection().collapse(ed.firstChild, 2);
        const set = v => document.execCommand('fontSize', false, v);
        return [set('+2.5'), document.queryCommandValue('fontSize'), set('2em'),
                set('3e+'), document.queryCommandValue('fontSize')].join('|');
    })()"#);
    assert_eq!(out, "true|5|false|false|5");
}

#[test]
fn paragraph_deletion_moves_one_line_and_preserves_its_whitespace() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<section contenteditable id=ed><p>left</p>\n<div style='white-space:pre-wrap'><b>two  spaces<br>next</b></div></section>");
    let result = eval(&e, &mut page, r#"(() => {
        const sel = getSelection(), left = ed.firstChild.firstChild;
        const right = ed.lastChild.firstChild.firstChild;
        sel.collapse(right, 0);
        document.execCommand('delete');
        const moved = ed.firstChild.querySelector('b');
        return [ed.firstChild.textContent, getComputedStyle(moved).whiteSpace,
                ed.lastChild.textContent, sel.anchorNode === left, sel.anchorOffset].join('|');
    })()"#);
    assert_eq!(result, "lefttwo  spaces|pre-wrap|next|true|4");
}

#[test]
fn selectors_distinguish_empty_and_wildcard_namespaces() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<section id=host></section>");
    let result = eval(&e, &mut page, r#"(() => {
        const a = document.createElement('entry'), b = document.createElementNS(null,'entry');
        a.setAttributeNS('urn:fixture','a:level','one');
        a.setAttributeNS('urn:second','b:level','two');
        b.setAttribute('level','plain');
        host.append(a,b);
        return [a.matches('|entry'), b.matches('|entry'), a.matches('*|entry'),
            a.matches('[*|level="two"]'), a.matches('[|level]'), b.matches('[|level]')].join('|');
    })()"#);
    assert_eq!(result, "false|true|true|true|false|true");
}

#[test]
fn css_style_rule_selector_changes_are_atomic() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<style id=sheet>.selected { color: red }</style><article class=selected id=item>x</article>");
    let result = eval(&e, &mut page, r#"(() => {
        const rule = document.querySelector('#sheet').sheet.cssRules[0];
        rule.selectorText = 'article, .broken:unknown-pseudo';
        const original = [rule.selectorText, getComputedStyle(item).color];
        rule.selectorText = 'section, :is(.other, :unknown-pseudo)';
        return [...original, rule.selectorText, getComputedStyle(item).color].join('|');
    })()"#);
    assert_eq!(result, ".selected|rgb(255, 0, 0)|section, :is(.other, :unknown-pseudo)|rgb(0, 0, 0)");
}

#[test]
fn a_preserved_newline_ends_the_line_moved_by_delete() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<section id=ed contenteditable><p>old</p><div style='white-space:pre-wrap'><i>first\nsecond</i></div></section>");
    let result = eval(&e, &mut page, r#"(() => {
        const start = ed.firstChild.firstChild;
        getSelection().collapse(start, start.length);
        document.execCommand('forwardDelete');
        return [ed.firstChild.textContent, ed.lastChild.innerHTML,
                ed.firstChild.querySelector('i').textContent].join('|');
    })()"#);
    assert_eq!(result, "oldfirst|<i>second</i>|first");
}

#[test]
fn inline_and_rule_declarations_convert_null_and_undefined_per_cssom() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<style id=sheet>p {color:blue}</style><p id=box></p>");
    let result = eval(&e, &mut page, r#"(() => {
        return [box.style, sheet.sheet.cssRules[0].style].map(s => {
            s.setProperty('color','red',null);
            s.setProperty('color',undefined);
            s.setProperty('--example',undefined);
            const result = [s.color, s.getPropertyValue('--example')];
            s.setProperty('color',null);
            return [...result,s.color].join('/');
        }).join('|');
    })()"#);
    assert_eq!(result, "red/undefined/|red/undefined/");
}

#[test]
fn selector_queries_follow_focus_and_preserve_scope_on_indexed_subjects() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<section id=group><input id=field></section><a id=link href='#local'>x</a>");
    let result = eval(&e, &mut page, r#"(() => {
        field.focus();
        const focused = [field.matches(':focus-visible'), field.closest('section:focus-within') === group,
            document.querySelector('#field:focus') === field, group.querySelector(':scope > :focus') === field,
            link.matches(':link'), link.matches(':visited')];
        field.blur();
        return [...focused, document.querySelectorAll(':focus').length].join('|');
    })()"#);
    assert_eq!(result, "true|true|true|true|true|false|0");
}

#[test]
fn editable_viewport_restyles_and_keeps_author_focus_indicators() {
    use rabbithole_style::BorderStyle;
    let mut e = engine(true);
    let mut page = load(&mut e, "<style>article:read-write {color:green}</style><article id=bodytext>x</article><p contenteditable=false id=locked>y</p>");
    let root = page.doc.html_element().unwrap();
    let state = eval(&e, &mut page, r#"(() => {
        document.designMode = 'on';
        return [document.activeElement === document.documentElement,
            document.documentElement.matches(':focus-visible'),
            getComputedStyle(bodytext).color, locked.matches(':read-write')].join('|');
    })()"#);
    assert_eq!(state, "true|true|rgb(0, 128, 0)|false");
    e.flush_style(&mut page);
    assert_eq!(page.styles.get(root).unwrap().outline_style, BorderStyle::None);
    eval(&e, &mut page, "document.documentElement.style.outline='3px dashed red'");
    e.flush_style(&mut page);
    assert_eq!(page.styles.get(root).unwrap().outline_style, BorderStyle::Dashed);
    let color = eval(&e, &mut page, "document.designMode='off'; getComputedStyle(bodytext).color");
    assert_eq!(color, "rgb(0, 0, 0)");
}

#[test]
fn aria_dom_strings_reflect_absence_and_keep_their_content_attributes() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<section id=el></section><svg id=icon></svg>");
    let result = eval(&e, &mut page, r#"(() => {
        const initial = el.ariaLabel === null;
        el.ariaColIndexText = 4;
        icon.ariaRoleDescription = 'marker';
        el.role = 'button';
        el.ariaLabel = '';
        const values = [initial, el.hasAttribute('aria-label'), el.getAttribute('aria-colindextext'),
            icon.getAttribute('aria-roledescription'), el.getAttribute('role')];
        el.ariaLabel = undefined;
        el.role = null;
        return [...values, el.ariaLabel === null, el.hasAttribute('role')].join('|');
    })()"#);
    assert_eq!(result, "true|true|4|marker|button|true|false");
}

#[test]
fn data_channel_transports_report_negotiated_limits_and_the_selected_ice_pair() {
    use std::time::{Duration, Instant};
    let mut e = engine(true);
    let mut page = load(&mut e, "<main></main>");
    eval(&e, &mut page, r#"
        var result = '', step = 'offer';
        var sender = new RTCPeerConnection(), receiver = new RTCPeerConnection();
        (async () => {
            const states = [];
            receiver.oniceconnectionstatechange = () => {
                if (receiver.iceConnectionState === 'connected')
                    states.push(!!receiver.sctp.transport.iceTransport.getSelectedCandidatePair());
            };
            const channel = sender.createDataChannel('transport');
            const message = new Promise(resolve => channel.onmessage = event => resolve(event.data));
            receiver.ondatachannel = ({channel}) => {
                const reply = () => {
                    try {
                        let oversized = false;
                        try { channel.send('four'); } catch (error) { oversized = error instanceof TypeError; }
                        if (!oversized) throw Error('negotiated limit ignored');
                        channel.send('abc');
                    } catch (error) { result = String(error); }
                };
                if (channel.readyState === 'open') reply(); else channel.onopen = reply;
            };
            await sender.setLocalDescription(await sender.createOffer());
            const offer = sender.localDescription.sdp.replace(/a=max-message-size:[0-9]+/, 'a=max-message-size:3');
            await receiver.setRemoteDescription({type:'offer',sdp:offer});
            await receiver.setLocalDescription(await receiver.createAnswer());
            await sender.setRemoteDescription(receiver.localDescription);
            step = 'message';
            const data = await message;
            const sctp = receiver.sctp, dtls = sctp.transport, ice = dtls.iceTransport;
            step = 'pair';
            while (!ice.getSelectedCandidatePair()) await new Promise(resolve => setTimeout(resolve, 10));
            await receiver.addIceCandidate(null);
            while (ice.state !== 'completed') await new Promise(resolve => setTimeout(resolve, 5));
            if (states.join() !== 'true') throw Error('ICE selection was unavailable on connection');
            const pair = ice.getSelectedCandidatePair();
            const properties = [data, sctp instanceof RTCSctpTransport, dtls instanceof RTCDtlsTransport,
                ice instanceof RTCIceTransport, sctp.state, dtls.state, ice.role, sctp.maxMessageSize,
                ice.getLocalCandidates().includes(pair.local), ice.getRemoteCandidates().includes(pair.remote),
                ice.getLocalParameters().usernameFragment === sender.sctp.transport.iceTransport.getRemoteParameters().usernameFragment];
            const closed = new Promise(resolve => dtls.onstatechange = () => {
                if (dtls.state === 'closed') resolve();
            });
            step = 'close';
            sender.close();
            await closed;
            result = properties.concat([sctp.state, sender.sctp.state]).join('|');
            receiver.close();
        })().catch(error => { result = String(error); });
    "#);
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        e.tick(&mut page);
        let result = eval(&e, &mut page, "result");
        if !result.is_empty() {
            assert_eq!(result, "abc|true|true|true|connected|connected|controlled|3|true|true|true|closed|closed");
            break;
        }
        assert!(Instant::now() < deadline, "{}", eval(&e, &mut page, "step"));
        std::thread::sleep(Duration::from_millis(4));
    }
}

#[test]
fn rich_editing_preserves_paragraph_styles_typing_formats_and_preformatted_newlines() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<style>.code{white-space:pre-wrap}</style><main id=ed contenteditable><p class=code><b>ab</b></p></main>");
    let result = eval(&e, &mut page, r#"(() => {
        const sel = getSelection(), doc = document;
        ed.focus();
        sel.collapse(ed.querySelector('b').firstChild, 1);
        doc.execCommand('insertLineBreak');
        const soft = ed.querySelector('p').textContent;
        doc.execCommand('insertParagraph');
        doc.execCommand('italic');
        doc.execCommand('insertText', false, 'Y');
        const p = ed.lastChild;
        sel.collapse(p, p.childNodes.length);
        doc.execCommand('insertParagraph');
        return [soft,ed.firstChild.className,p.className,p.querySelector('b').textContent,
            p.querySelector('i').textContent,ed.lastChild.className,!!ed.lastChild.querySelector('br')].join('|');
    })()"#);
    assert_eq!(result, "a\nb|code|code|Yb|Y|code|true");
}

#[test]
fn stylesheet_namespaces_limit_type_and_attribute_selectors_in_nested_rules() {
    let mut e = engine(true);
    let mut page = load(&mut e, r#"<style id=sheet>@namespace H url(http://www.w3.org/1999/xhtml);
      @namespace S "http://www.w3.org/2000/svg"; @namespace a "urn:flag";
      @media all { H|*.item { opacity: 0.2 } S|*.item[a|flag] { opacity: 0.6 } }
      .absent { opacity: 0.8 }</style><div id=h class=item></div><svg><g id=s class=item></g></svg>"#);
    let result = eval(&e, &mut page, r#"(() => {
        s.setAttributeNS('urn:flag','flag','set');
        const rules = sheet.sheet.cssRules, rule = rules[rules.length - 1];
        const before = [getComputedStyle(h).opacity,getComputedStyle(s).opacity];
        rule.selectorText = 'unknown|g';
        const rejected = rule.selectorText;
        rule.selectorText = 'S|g.item[a|flag]';
        return [...before, rules[0].prefix, rules[0].namespaceURI, rejected,
            rule.selectorText, getComputedStyle(s).opacity].join('|');
    })()"#);
    assert_eq!(result, "0.2|0.6|H|http://www.w3.org/1999/xhtml|.absent|S|g.item[a|flag]|0.8");
}

#[test]
fn script_formatting_switches_baseline_semantics_on_only_the_selected_text() {
    let mut e=engine(true);
    let mut page=load(&mut e,"<main id=ed contenteditable><sub>abc</sub><span style='vertical-align:super'>tail</span></main>");
    let value=eval(&e,&mut page,r#"(() => {
        const sel=getSelection(), text=ed.firstChild.firstChild;
        sel.setBaseAndExtent(text,1,text,2);
        document.execCommand('styleWithCSS',false,'true');
        document.execCommand('superscript');
        const changed=ed.querySelector('sup');
        const state=document.queryCommandState('superscript');
        sel.selectAllChildren(ed.lastChild);
        return [ed.textContent,changed?.textContent,[...ed.querySelectorAll('sub')].map(e=>e.textContent).join(','),
          state,document.queryCommandState('superscript'),ed.lastChild.style.verticalAlign].join('|');
    })()"#);
    assert_eq!(value,"abctail|b|a,c|true|false|super");
}

#[test]
fn css_supports_preserves_selector_combinators_and_checks_anchor_grammar() {
    let mut e = engine(true);
    let mut page = load(&mut e, r#"<style>
        @supports selector(div span) and (left: anchor(--origin right, 2px)) {
            #result { width: 73px; }
        }
    </style><div id=result></div>"#);
    let value = eval(&e,&mut page,r#"
        (() => {
          result.style.shapeOutside = 'initial';
          return [getComputedStyle(result).width,result.style.shapeOutside,
            CSS.supports('shape-outside','circle(20px at 30% 50%)'),
            CSS.supports('top','anchor(--origin bottom)'),
            CSS.supports('width','anchor(--origin bottom)'),
            CSS.supports('position-try-fallbacks','--try flip-inline'),
            CSS.supports('anchor-scope','--first, --second')].join('|');
        })()
    "#);
    assert_eq!(value,"73px|initial|true|true|false|true|true");
}

#[test]
fn animated_float_exclusion_margin_clamps_after_extrapolation() {
    let mut e = engine(true);
    let mut page = load(&mut e,"<div id=box></div>");
    let value = eval(&e,&mut page,r#"
        (() => {
            const animation=box.animate([{shapeMargin:'0px'},{shapeMargin:'20px'}],
                {duration:1000,easing:'cubic-bezier(0.5,-1,0.5,-1)'});
            animation.pause(); animation.currentTime=500;
            return getComputedStyle(box).shapeMargin;
        })()
    "#);
    assert_eq!(value,"0px");
}

#[test]
fn empty_text_replaces_a_selection_and_preserves_a_collapsed_caret() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<div contenteditable id=ed>abcde</div>");
    let result = eval(&e, &mut page, r#"(() => {
        ed.focus();
        const events = [];
        ed.addEventListener('input', e => events.push([e.inputType, e.data]));
        const sel = getSelection(), text = ed.firstChild;
        sel.setBaseAndExtent(text, 4, text, 1);
        document.execCommand('insertText', false, '');
        document.execCommand('insertText', false, '');
        return JSON.stringify([ed.textContent, sel.isCollapsed, sel.focusOffset, events]);
    })()"#);
    assert_eq!(result, r#"["ae",true,1,[["insertText",""]]]"#);
}

#[test]
fn editing_keeps_neighboring_inline_spaces_and_releases_trailing_space_when_typing() {
    let mut e = engine(true);
    let mut page = load(&mut e,"<div id=ed contenteditable>a&nbsp; <b>&nbsp;b</b></div><div id=plain contenteditable=plaintext-only style=white-space:normal><br></div>");
    let result = eval(&e,&mut page,r#"(() => {
        const sel = getSelection();
        ed.focus(); sel.collapse(ed.firstChild,3);
        document.execCommand('delete');
        const untouched = ed.innerHTML;
        plain.focus(); sel.collapse(plain,0);
        document.execCommand('insertText',false,' ');
        const first = plain.innerHTML;
        ed.textContent = 'x'; ed.focus(); sel.collapse(ed.firstChild,1);
        document.execCommand('insertText',false,' ');
        document.execCommand('insertText',false,'y');
        const typed = [ed.textContent,sel.focusOffset];
        ed.innerHTML = 'a&nbsp;&nbsp;<i>&nbsp;b</i>';
        sel.collapse(ed.firstChild,1);
        document.execCommand('forwardDelete');
        const joined = ed.innerHTML;
        ed.innerHTML = 'a&nbsp;<i style="white-space:pre">b</i>';
        sel.collapse(ed.firstChild,2);
        document.execCommand('insertText',false,'\u00a0');
        return [untouched,first,...typed,joined,ed.firstChild.data].join('|');
    })()"#);
    assert_eq!(result,"a&nbsp;<b>&nbsp;b</b>|&nbsp;|x y|3|a&nbsp;<i>&nbsp;b</i>|a\u{a0}\u{a0}");
}

#[test]
fn editing_preserves_visible_spaces_across_inline_markup_and_leaves_preformatted_text_literal() {
    let mut e=engine(true);
    let mut page=load(&mut e,"<div id=ed contenteditable>a<b> </b>z</div><pre id=literal contenteditable>x</pre>");
    let value=eval(&e,&mut page,r#"(() => {
        const sel=getSelection(), mark=s=>s.replaceAll('\u00a0','_');
        ed.focus();
        sel.collapse(ed.firstChild,1);
        document.execCommand('insertText',false,' ');
        const inserted=ed.innerHTML;
        ed.textContent='x \u00a0';
        sel.collapse(ed.firstChild,3);
        document.execCommand('delete');
        const deleted=mark(ed.textContent);
        literal.focus(); sel.collapse(literal.firstChild,1);
        document.execCommand('insertText',false,'  ');
        return [inserted,deleted,literal.textContent,sel.focusOffset].join('|');
    })()"#);
    assert_eq!(value,"a&nbsp;<b> </b>z|x_|x  |3");
}

#[test]
fn float_exclusion_shapes_wrap_lines_without_moving_the_float() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<style>*{margin:0}main{width:300px;line-height:20px}#f{float:left;width:100px;height:100px;shape-outside:inset(0 70px 0 0);shape-margin:5px}i{display:inline-block;width:8px;height:10px}</style><main><div id=f></div><i id=mark></i><br><i></i></main>");
    let result = eval(&e, &mut page, r#"(() => {
        const left=()=>mark.getBoundingClientRect().left-f.getBoundingClientRect().left;
        const inset=left(), value=getComputedStyle(f).shapeOutside;
        f.style.shapeOutside='polygon(0px 0px, 60px 0px, 60px 80px, 0px 80px)';
        const polygon=left();
        const animation=f.animate([{shapeMargin:'0px'},{shapeMargin:'20px'}],1000);
        animation.pause(); animation.currentTime=500;
        const animated=left();
        animation.cancel();
        f.style.shapeOutside='none';
        return [inset,polygon,animated,left(),f.getBoundingClientRect().width,value].join('|');
    })()"#);
    assert_eq!(result, "35|65|70|100|100|inset(0px 70px 0px 0px)");
}

#[test]
fn link_editing_preserves_existing_attributes_and_customizes_typed_text() {
    let mut e = engine(true);
    let mut page=load(&mut e,"<main contenteditable id=ed><p><a href='/old' title='title'>old</a> plain</p><p>next</p></main>");
    let result=eval(&e,&mut page,r#"(() => {
        const sel=getSelection(), old=ed.querySelector('a'), text=ed.lastChild.firstChild;
        sel.setBaseAndExtent(text,2,old.firstChild,1);
        const made=document.execCommand('createLink',false,'/new');
        const linked=[...ed.querySelectorAll('a')].map(a=>a.getAttribute('href')).join(',');
        const backwards=sel.direction;
        document.execCommand('unlink');
        sel.collapse(text,0);
        document.execCommand('createLink',false,'/typed');
        document.execCommand('insertText',false,'x');
        return [made,linked,backwards,old.title,ed.querySelector('a[href]').textContent,ed.querySelector('a[href]').getAttribute('href')].join('|');
    })()"#);
    assert_eq!(result,"true|/new,/new,/new|backward|title|x|/typed");
}

#[test]
fn typing_a_new_link_inside_a_link_keeps_the_surrounding_destination() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<div id=ed contenteditable><a href='/old'><em>ab</em></a></div>");
    let result = eval(&e, &mut page, r#"(() => {
        getSelection().collapse(ed.querySelector('em').firstChild, 1);
        document.execCommand('createLink', false, '/new');
        document.execCommand('insertText', false, 'X');
        return Array.from(ed.querySelectorAll('a'), a => a.getAttribute('href') + '=' + a.textContent).join(',');
    })()"#);
    assert_eq!(result, "/old=a,/new=X,/old=b");
}

#[test]
fn removing_rich_formatting_preserves_links_unselected_text_and_selection_direction() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<div contenteditable id=ed><b>abc</b><a href='#n'><i>de</i></a><span contenteditable=false>locked</span></div>");
    let result = eval(&e, &mut page, r#"(() => {
        ed.focus();
        const sel = getSelection();
        sel.setBaseAndExtent(ed.querySelector('i').firstChild,1,ed.querySelector('b').firstChild,1);
        const events = [];
        ed.oninput = event => events.push(event.inputType);
        const ok = document.execCommand('removeFormat');
        return [ok,ed.innerHTML,sel.toString(),sel.direction,events.join(',')].join('|');
    })()"#);
    assert_eq!(result, "true|<b>a</b>bc<a href=\"#n\">d<i>e</i></a><span contenteditable=\"false\">locked</span>|bcd|backward|formatRemove");
    let nested = eval(&e, &mut page, r##"(() => {
        ed.innerHTML = '<b><a href="#n">abc</a></b>';
        const sel = getSelection(), text = ed.querySelector('a').firstChild;
        sel.setBaseAndExtent(text,1,text,2);
        document.execCommand('removeFormat');
        const middle = sel.anchorNode, html = ed.innerHTML;
        ed.style.color = 'red';
        document.execCommand('removeFormat');
        return [html, sel.anchorNode === middle, sel.toString()].join('|');
    })()"##);
    assert_eq!(nested, r##"<b><a href="#n">a</a></b><a href="#n">b</a><b><a href="#n">c</a></b>|true|b"##);

}

#[test]
fn rich_list_commands_group_paragraphs_and_split_only_selected_items() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<section id=ed contenteditable><p>one</p><p>two</p><p>three</p></section>");
    let result = eval(&e, &mut page, r#"(() => {
        const sel=getSelection(), first=ed.firstChild.firstChild, middle=ed.children[1].firstChild;
        ed.focus();
        sel.setBaseAndExtent(middle,2,first,1);
        document.execCommand('insertOrderedList');
        const initial = [ed.innerHTML,sel.direction,sel.anchorNode===middle,document.queryCommandState('insertorderedlist')];
        sel.collapse(middle,1);
        document.execCommand('insertUnorderedList');
        const converted = ed.innerHTML;
        document.execCommand('insertUnorderedList');
        return [...initial,converted,ed.innerHTML,sel.anchorNode===middle,sel.anchorOffset].join('|');
    })()"#);
    assert_eq!(result,"<ol><li>one</li><li>two</li></ol><p>three</p>|backward|true|true|<ol><li>one</li></ol><ul><li>two</li></ul><p>three</p>|<ol><li>one</li></ol><div>two</div><p>three</p>|true|1");
}

#[test]
fn physical_cssom_geometry_survives_nested_orthogonal_writing_modes() {
    let mut e = engine(true);
    let mut page = load(&mut e, r#"<style>
      #v { writing-mode:vertical-rl; position:relative; width:164px; height:88px; padding:6px 18px; border:3px solid }
      #i { writing-mode:horizontal-tb; position:absolute; left:10%; top:25%; width:26px; height:14px }
      </style><div id=v><div id=i>x</div></div>"#);
    let result = eval(&e, &mut page, r#"(() => {
      const rect=v.getBoundingClientRect(), inner=i.getBoundingClientRect(), css=getComputedStyle(i);
      return [v.clientWidth,v.clientHeight,rect.width,rect.height,inner.width,inner.height,css.left,css.top].join('|');
    })()"#);
    assert_eq!(result,"200|100|206|106|26|14|20px|25px");
}

#[test]
fn retained_computed_declaration_observes_rule_mutations_within_a_script() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<style id=sheet>.inactive{opacity:.2}</style><span id=target></span>");
    let result = eval(&e,&mut page,r#"(() => {
        const css=getComputedStyle(target), rule=sheet.sheet.cssRules[0];
        const before=css.opacity;
        rule.selectorText='#target';
        const matched=css.opacity;
        rule.style.setProperty('opacity','0.7');
        return [before,matched,css.getPropertyValue('opacity')].join('|');
    })()"#);
    assert_eq!(result,"1|0.2|0.7");
}

#[test]
fn computed_gradient_layers_retain_geometry_and_color_interpolation() {
    let mut e = engine(true);
    let mut page = load(&mut e,"<style>#sample {font-size:10px;background-image:radial-gradient(ellipse 50% 4em at right center, red, blue),linear-gradient(in oklch longer hue to right, color(srgb 1 0 0), blue);mask-image:conic-gradient(from 30deg,black,white)}</style><div id=sample></div>");
    let result = eval(&e,&mut page,"[getComputedStyle(sample).backgroundImage,getComputedStyle(sample).maskImage].join('|')");
    assert_eq!(result,"radial-gradient(50% 40px at 100% 50%, rgb(255, 0, 0), rgb(0, 0, 255)), linear-gradient(to right in oklch longer hue, color(srgb 1 0 0), rgb(0, 0, 255))|conic-gradient(from 30deg, rgb(0, 0, 0), rgb(255, 255, 255))");
}

#[test]
fn rich_list_commands_respect_sublists_and_table_cell_boundaries() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<section id=ed contenteditable><ul><li>outer<ul><li>inner</li></ul></li></ul><table><tr><td><b>cell</b></td></tr></table></section>");
    let result = eval(&e,&mut page,r#"(() => {
        ed.focus();
        const sel=getSelection(), outer=ed.querySelector('li').firstChild, cell=ed.querySelector('td');
        sel.collapse(outer,2);
        document.execCommand('insertOrderedList');
        const mixed=[document.queryCommandState('insertOrderedList'),document.queryCommandIndeterm('insertOrderedList')];
        sel.collapse(ed.querySelector('ul li').firstChild,2);
        document.execCommand('insertUnorderedList');
        const lifted=ed.firstChild.lastChild.textContent;
        const text=cell.firstChild.firstChild;
        sel.setBaseAndExtent(text,1,text,3);
        document.execCommand('insertOrderedList');
        document.execCommand('insertOrderedList');
        return [...mixed,lifted,cell.innerHTML,sel.anchorNode===text,sel.anchorOffset].join('|');
    })()"#);
    assert_eq!(result,"false|true|inner|<b>cell</b>|true|1");
}

#[test]
fn indent_and_outdent_preserve_paragraphs_formatting_and_selection() {
    let mut e = engine(true);
    let mut page = load(&mut e,"<div id=ed contenteditable><p>one</p><p><b>two</b></p><p>three</p></div>");
    let result=eval(&e,&mut page,r#"(() => {
        ed.focus();
        const sel=getSelection(), a=ed.firstChild.firstChild, b=ed.querySelector('b').firstChild;
        sel.setBaseAndExtent(b,2,a,1);
        document.execCommand('indent');
        const grouped=ed.firstChild.outerHTML, backwards=sel.direction;
        sel.collapse(b,1);
        document.execCommand('outdent');
        return [grouped,backwards,ed.innerHTML,sel.anchorNode===b,sel.anchorOffset].join('|');
    })()"#);
    assert_eq!(result,"<blockquote><p>one</p><p><b>two</b></p></blockquote>|backward|<blockquote><p>one</p></blockquote><p><b>two</b></p><p>three</p>|true|1");
}

#[test]
fn data_channels_added_during_an_empty_offer_trigger_one_followup_negotiation() {
    use std::time::{Duration, Instant};
    let mut e = engine(true);
    let mut page = load(&mut e, "<p>RTC negotiation</p>");
    eval(&e, &mut page, r#"
      var negotiationResult = '';
      (async () => {
        const a = new RTCPeerConnection(), b = new RTCPeerConnection();
        let calls = 0;
        await a.setLocalDescription();
        a.onnegotiationneeded = async () => {
          try {
            ++calls;
            await a.setLocalDescription();
            if (!a.localDescription.sdp.includes('m=application')) throw Error('missing SCTP');
            await b.setRemoteDescription(a.localDescription);
            await b.setLocalDescription();
            await a.setRemoteDescription(b.localDescription);
          } catch (e) { negotiationResult = String(e); }
        };
        const first = a.createDataChannel('one');
        a.createDataChannel('two');
        first.onopen = () => {
          a.createDataChannel('three');
          setTimeout(() => { negotiationResult = calls + '|' + a.signalingState; a.close(); b.close(); }, 10);
        };
        await b.setRemoteDescription(a.localDescription);
        await b.setLocalDescription();
        await a.setRemoteDescription(b.localDescription);
      })().catch(e => negotiationResult = String(e));
    "#);
    let until = Instant::now() + Duration::from_secs(8);
    loop {
        e.tick(&mut page);
        let result = eval(&e, &mut page, "negotiationResult");
        if !result.is_empty() { assert_eq!(result, "1|stable"); break; }
        assert!(Instant::now() < until, "data channel negotiation did not finish");
        std::thread::sleep(Duration::from_millis(4));
    }
}

#[test]
fn closing_a_data_channel_delivers_queued_blob_and_text_in_order() {
    use std::time::{Duration, Instant};
    let mut e = engine(true);
    let mut page = load(&mut e,"<p>data channel drain</p>");
    eval(&e,&mut page,r#"
      var drainResult='';
      (async () => {
        const a=new RTCPeerConnection(), b=new RTCPeerConnection();
        b.ondatachannel=e=>{
          const incoming=e.channel, messages=[];
          incoming.onmessage=event=>messages.push(typeof event.data==='string'?event.data:event.data.byteLength);
          incoming.onclose=()=>{drainResult=[...messages,c.bufferedAmount].join('|');a.close();b.close();};
        };
        const c=a.createDataChannel('drain');
        c.onopen=()=>{c.send(new Blob([new Uint8Array(4096)]));c.send('done');c.close();};
        await a.setLocalDescription(await a.createOffer());
        await b.setRemoteDescription(a.localDescription);
        await b.setLocalDescription(await b.createAnswer());
        await a.setRemoteDescription(b.localDescription);
      })().catch(e=>drainResult=String(e));
    "#);
    let until=Instant::now()+Duration::from_secs(8);
    loop {
        e.tick(&mut page);
        let value=eval(&e,&mut page,"drainResult");
        if !value.is_empty() { assert_eq!(value,"4096|done|0"); break; }
        assert!(Instant::now()<until,"remote channel did not receive and close");
        std::thread::sleep(Duration::from_millis(4));
    }
}

#[test]
fn indent_at_a_boundary_between_blocks_preserves_both_paragraphs() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<div id=ed contenteditable><p>first</p><div>second</div></div>");
    let result = eval(&e, &mut page, r#"(() => {
        ed.focus();
        const selection = getSelection();
        selection.collapse(ed, 1);
        document.execCommand('indent');
        document.execCommand('outdent');
        return [ed.innerHTML, selection.anchorNode === ed, selection.anchorOffset].join('|');
    })()"#);
    assert_eq!(result, "<p>first</p><div>second</div>|true|1");
}

#[test]
fn a_rejected_range_insertion_keeps_the_text_and_its_live_endpoints_intact() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<p>test</p>");
    let result = eval(&e, &mut page, r#"(() => {
        const doc = document.implementation.createHTMLDocument('other');
        const text = doc.body.appendChild(doc.createTextNode('abcdef'));
        const range = doc.createRange();
        range.setStart(text, 2);
        range.setEnd(text, 5);
        const errors = [];
        for (const bad of [doc, doc.body]) {
            try { range.insertNode(bad); } catch (e) { errors.push(e.name); }
        }
        const copy = range.cloneContents();
        return [...errors, copy.ownerDocument === doc, copy.textContent,
                text.data, doc.body.childNodes.length, range.endContainer === text, range.endOffset].join('|');
    })()"#);
    assert_eq!(result, "HierarchyRequestError|HierarchyRequestError|true|cde|abcdef|1|true|5");
}

#[test]
fn ordered_position_fallbacks_include_the_original_inset_space_and_keep_ties_stable() {
    let mut e=engine(true);
    let mut page=load(&mut e,"<style>#cb{position:relative;width:300px;height:100px}#item{position:absolute;left:180px;width:20px;height:10px;position-try:most-width --narrow,--wide,--equal}@position-try --narrow{left:110px}@position-try --wide{left:30px}@position-try --equal{right:30px;left:auto}</style><div id=cb><div id=item></div></div>");
    let value=eval(&e,&mut page,r#"(() => {
        const wide=item.offsetLeft;
        item.style.positionTry='normal --narrow, --wide';
        const first=item.offsetLeft;
        const fields=[item.style.positionTryOrder,item.style.positionTryFallbacks,item.style.positionTry];
        item.style.left='10px'; item.style.positionTryOrder='most-width';
        return [wide,first,item.offsetLeft,...fields].join('|');
    })()"#);
    assert_eq!(value,"30|180|10|normal|--narrow, --wide|--narrow, --wide");
}

#[test]
fn fallback_alignment_reflects_logical_edges_and_exchanges_axes() {
    let mut e = engine(true);
    let mut page = load(&mut e,"<style>#cb{position:relative;width:180px;height:120px}#target{position:absolute;left:250px;width:30px;height:20px;position-try-fallbacks:--fit flip-inline}@position-try --fit {inset:0;justify-self:start;align-self:end}</style><div id=cb><div id=target></div></div>");
    let value = eval(&e,&mut page,r#"(() => {
        const pos=()=>[target.offsetLeft,target.offsetTop].join(',');
        const inline=pos();
        target.style.positionTryFallbacks='--fit flip-start';
        const swap=[pos(),target.offsetWidth,target.offsetHeight].join(',');
        cb.style.direction='rtl';
        target.style.positionTryFallbacks='--fit flip-inline';
        return [inline,swap,pos()].join('|');
    })()"#);
    assert_eq!(value,"150,100|160,0,20,30|0,100");
}

#[test]
fn fallback_tactics_reflect_anchor_sides_and_preserve_try_order() {
    let mut e = engine(true);
    let mut page=load(&mut e,"<style>#cb{position:relative;width:180px;height:140px}#a{position:absolute;left:120px;top:100px;width:20px;height:20px;anchor-name:--a}#t{position:absolute;position-anchor:--a;width:55px;height:35px;left:anchor(right);top:anchor(bottom);position-try-fallbacks:flip-block,flip-inline,flip-block flip-inline}</style><main id=cb><i id=a></i><div id=t></div></main>");
    let result=eval(&e,&mut page,r#"(() => {
        const pos=()=>[t.offsetLeft,t.offsetTop].join(',');
        const first=pos();
        cb.style.width='240px';
        const second=pos();
        cb.style.height='200px';
        return [first,second,pos()].join('|');
    })()"#);
    assert_eq!(result,"65,65|140,65|140,120");
}

#[test]
fn position_try_descriptors_are_live_and_fallbacks_follow_layout() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<style id=rules>@position-try --fit { left: 12px; top: 8px; color:red; height:1px!important } #cb{width:80px;height:60px;position:relative}#target{position:absolute;left:65px;top:0;width:25px;height:12px;position-try-fallbacks:--missing,--fit}</style><div id=cb><i id=target></i></div>");
    let result = eval(&e,&mut page,r#"(() => {
        const rule = rules.sheet.cssRules[0], s = rule.style;
        const x = () => target.getBoundingClientRect().left - cb.getBoundingClientRect().left;
        const fields = [rule instanceof CSSPositionTryRule, s instanceof CSSPositionTryDescriptors, rule.name,
            s.length, s.getPropertyValue('height'), s.color, x()];
        s.left = '20px';
        fields.push(x(), getComputedStyle(target).top);
        s.setProperty('background-color','blue');
        s.setProperty('left','30px','important');
        fields.push(s.left, s.length);
        cb.style.width='120px';
        fields.push(x());
        cb.style.width='25px';
        fields.push(x());
        return fields.join('|');
    })()"#);
    assert_eq!(result,"true|true|--fit|2|||12|20|8px|20px|2|65|65");
}

#[test]
fn anchor_math_serialization_preserves_grouping_and_sorts_numeric_terms() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<i id=box></i>");
    let result = eval(&e, &mut page, r#"(() => {
        const s=box.style;
        s.left='calc(anchor(--p right) + 3%)';
        s.top='anchor(--p top, calc(0.5 * anchor(--q bottom)))';
        s.width='calc(2 * (anchor-size(--p width) + 5px))';
        return [s.left,s.top,s.width].join('|');
    })()"#);
    assert_eq!(result, "calc(3% + anchor(--p right))|anchor(--p top, calc(0.5 * anchor(--q bottom)))|calc(2 * (5px + anchor-size(--p width)))");
}

#[test]
fn anchor_lookup_observes_containing_blocks_and_out_of_flow_order() {
    let mut e=engine(true);
    let mut page=load(&mut e,"<style>section{position:relative}aside{position:absolute}i{display:block;width:35px;anchor-name:--x}.target{position:absolute;width:anchor-size(--x width, 7px);height:5px}</style><section><div class=target id=early></div><aside><i></i></aside><div class=target id=late></div></section><section><i style='width:90px'></i></section>");
    let result=eval(&e,&mut page,"[early.offsetWidth,late.offsetWidth].join(',')");
    assert_eq!(result,"7,35");
}

#[test]
fn anchor_functions_resolve_distinct_boxes_and_nested_fallbacks() {
    let mut e = engine(true);
    let mut page=load(&mut e,"<style>body{margin:0}section{position:relative;width:300px;height:240px}.a{width:40px;height:20px;margin-left:30px;anchor-name:--a}.b{width:80px;height:32px;margin-left:120px;anchor-name:--b}#tip{position:absolute;left:anchor(right --a);top:anchor(bottom --b);width:anchor-size(--b width);height:anchor-size(--missing height, anchor-size(--a height))}</style><section><div class=a id=a></div><div class=b id=b></div><div id=tip></div></section>");
    let result=eval(&e,&mut page,r#"(() => {
        const sample=()=>{const r=tip.getBoundingClientRect();return [r.x,r.y,r.width,r.height].join(',')};
        const first=sample();
        tip.style.left='anchor(top --a, 9px)';
        const specified=tip.style.left, second=sample();
        tip.style.left='anchor(right --b)';
        b.style.width='90px';
        const third=sample(), left=getComputedStyle(tip).left;
        tip.style.left='calc((anchor(--a right) + anchor(--b left)) / 2)';
        tip.style.maxWidth='max(anchor-size(--a width), 15px)';
        tip.style.left='anchor(bogus right)';
        return [first,specified,second,third,left,sample()].join('|');
    })()"#);
    assert_eq!(result,"70,52,80,20|anchor(--a top, 9px)|9,52,80,20|210,52,90,20|210px|95,52,40,20");
}

#[test]
fn local_anchor_names_follow_the_nearest_matching_inclusive_scope() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<style>body{margin:0}.anchor{anchor-name:--box;width:20px;height:20px}.target{position:absolute;position-anchor:--box;position-area:bottom right;width:10px;height:10px}section{anchor-scope:--box}#a{margin-left:30px}#b{margin-left:120px}</style><div id=a class=anchor></div><section id=scope><div id=b class=anchor></div><div id=inside class=target></div></section><div id=outside class=target></div>");
    let result = eval(&e,&mut page,r#"(() => {
        const left = node => node.getBoundingClientRect().x;
        const before = [getComputedStyle(scope).anchorScope,left(inside),left(outside)];
        scope.style.anchorScope='--other';
        return [...before,left(inside),left(outside)].join('|');
    })()"#);
    assert_eq!(result,"--box|140|50|140|140");
}

#[test]
fn reopening_a_popover_uses_its_current_invoker_as_the_implicit_anchor() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<style>button{position:absolute;top:20px;width:40px;height:30px}#first{left:30px}#next{left:120px}#panel{position:fixed;position-area:bottom left;inset:auto;margin:0;width:10px;height:10px;padding:0;border:0}</style><button id=first></button><div id=panel popover></div><button id=next></button>");
    let result = eval(&e,&mut page,r#"(() => {
        const sample = source => {
            panel.showPopover({source});
            const rect = panel.getBoundingClientRect();
            panel.hidePopover();
            return [rect.x,rect.y].join(',');
        };
        return [sample(first),sample(next)].join('|');
    })()"#);
    assert_eq!(result,"20,50|110,50");
}

#[test]
fn named_anchor_areas_reposition_and_resize_the_target_after_style_changes() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<style>*{box-sizing:border-box}body{margin:0}section{position:relative;width:300px;height:200px}#a{position:absolute;left:40px;top:60px;width:50px;height:30px;anchor-name:--card, --other}#t{position:absolute;position-anchor:--card;position-area:bottom left;width:20px;height:10px}#b{position:absolute;left:150px;top:20px;width:10px;height:10px;anchor-name:--second}</style><section><div id=a></div><div id=b></div><div id=t></div></section>");
    let result = eval(&e, &mut page, r#"(() => {
        const geometry = () => { const r=t.getBoundingClientRect(); return [r.x,r.y,r.width,r.height].join(','); };
        const initial=geometry();
        a.style.top='80px';
        const moved=geometry();
        t.style.positionAnchor='--second';
        t.style.positionArea='right center';
        t.style.width='50%';
        const changed=geometry();
        return [getComputedStyle(a).anchorName,getComputedStyle(t).positionArea,initial,moved,changed].join('|');
    })()"#);
    assert_eq!(result,"--card, --other|right center|20,90,20,10|20,110,20,10|160,20,70,10");
}

#[test]
fn joining_an_inline_line_preserves_contextual_whitespace_on_the_moved_fragment() {
    let mut e = engine(true);
    let mut page = load(&mut e, "<style>#ed > .code { white-space:pre-wrap }</style><main contenteditable id=ed><p>left</p><span class=code><b>two  words<br>right</b></span></main>");
    let result = eval(&e, &mut page, r#"(() => {
        const first=ed.firstChild.firstChild;
        getSelection().collapse(first,first.length);
        document.execCommand('forwardDelete');
        const moved=ed.firstChild.querySelector('.code');
        return [moved.textContent,getComputedStyle(moved).whiteSpace,ed.lastChild.textContent].join('|');
    })()"#);
    assert_eq!(result,"two  words|pre-wrap|right");
}

#[test]
fn alignment_shorthands_expand_and_follow_their_longhand_priorities() {
    let mut e = engine(true);
    let mut page=load(&mut e,"<style id=sheet>#target { place-items: safe center end !important }</style><p id=target style='place-items:safe center end !important'></p>");
    let result = eval(&e,&mut page,r#"(() => {
        const inspect = style => {
            const first=[style.alignItems,style.justifyItems,style.getPropertyPriority('place-items')];
            style.justifyItems='start';
            const mixed=style.placeItems;
            style.setProperty('align-items','end');
            const combined=style.placeItems;
            const old=style.removeProperty('place-items');
            return [...first,mixed,combined,old,style.alignItems,style.justifyItems].join(',');
        };
        return [inspect(target.style),inspect(sheet.sheet.cssRules[0].style)].join('|');
    })()"#);
    let expected = "safe center,end,important,,end start,end start,,";
    assert_eq!(result,format!("{expected}|{expected}"));
}
