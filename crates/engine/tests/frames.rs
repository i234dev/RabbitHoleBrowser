//! `<iframe>`: nested browsing contexts with their own document, layout
//! and realm, painted into the parent, receiving input, and talking to
//! other frames with `postMessage`.

use rabbithole_engine::layout::DisplayItem;
use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Modifiers, Page, Viewport};

fn engine() -> Engine {
    let mut e = Engine::new_bundled_fonts_only();
    e.scripting = true;
    e.loader.load_scripts = true;
    e
}

fn load(engine: &mut Engine, html: &str) -> Page {
    let url = Url::parse("https://example.test/").unwrap();
    let mut page = engine.load_html(html, url, Viewport { width: 800.0, height: 600.0 });
    settle(engine, &mut page);
    page
}

/// Run ticks until frames are loaded and messages delivered.
fn settle(engine: &Engine, page: &mut Page) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut quiet = 0;
    while std::time::Instant::now() < deadline && quiet < 5 {
        let busy = page.frames_loading();
        engine.tick(page);
        std::thread::sleep(std::time::Duration::from_millis(10));
        if busy {
            quiet = 0;
        } else {
            quiet += 1;
        }
    }
}

fn eval(engine: &Engine, page: &mut Page, js: &str) -> String {
    engine.eval_in_page(page, js).trim_matches(|c| c == '\'' || c == '"').to_string()
}

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;")
}

#[test]
fn initial_blank_frames_fire_load_once_without_replacing_the_document() {
    let mut e = engine();
    let mut page = load(&mut e, r#"<script>
        var loads = [];
        for (var source of ['', 'about:blank']) {
            var f = document.createElement('iframe');
            if (source) f.src = source;
            f.onload = function () { loads.push(this.contentDocument.body.textContent); };
            document.body.appendChild(f);
            f.contentDocument.body.textContent = source || 'no src';
        }
        loads.push('sync');
    </script>"#);
    // No src: initial insertion fires synchronously. Explicit about:blank
    // navigates asynchronously, while keeping the same initial document.
    assert_eq!(eval(&e, &mut page, "loads.join('|')"), "|sync|about:blank");
    assert_eq!(eval(&e, &mut page, "document.querySelector('iframe').contentDocument.body.textContent"), "no src");
    settle(&e, &mut page);
    assert_eq!(eval(&e, &mut page, "loads.length"), "3");
}

#[test]
fn document_open_replaces_the_initial_frame_load_with_its_own() {
    let mut e = engine();
    let mut page = load(&mut e, r#"<script>
        var loads = [], f = document.createElement('iframe');
        f.src = 'about:blank';
        f.onload = function () { loads.push(this.contentDocument.body.textContent); };
        document.body.appendChild(f);
        var d = f.contentDocument;
        d.open();
        d.write('<body><p>written</p>');
    </script>"#);
    assert_eq!(eval(&e, &mut page, "loads.length"), "0");
    eval(&e, &mut page, "d.close()");
    settle(&e, &mut page);
    assert_eq!(eval(&e, &mut page, "loads.join('|')"), "written");
    settle(&e, &mut page);
    assert_eq!(eval(&e, &mut page, "loads.length"), "1");
}

fn child(page: &mut Page, i: usize) -> &mut Page {
    page.frames.children[i].page.as_mut().expect("frame loaded")
}

#[test]
fn srcdoc_frame_is_laid_out_and_painted_into_the_parent() {
    let mut e = engine();
    let inner = r#"<style>html,body{margin:0;background:rgb(255,0,0)}</style><p>inside</p>"#;
    let mut page = load(
        &mut e,
        &format!(r#"<body style="margin:0"><iframe id="f" style="border:0;width:200px;height:100px" srcdoc="{}"></iframe><noscript><iframe srcdoc="fallback"></iframe></noscript></body>"#, escape_attr(inner)),
    );
    // The `<noscript>` fallback frame is inert with scripting on.
    assert_eq!(page.frames.children.len(), 1);
    let c = child(&mut page, 0);
    assert_eq!(c.viewport.width, 200.0);
    assert_eq!(c.viewport.height, 100.0);
    assert!(c.text().contains("inside"));
    let img = page
        .display_list
        .items
        .iter()
        .find_map(|it| match it {
            DisplayItem::Image { rect, image, .. } if rect.w == 200.0 && rect.h == 100.0 => Some(image.clone()),
            _ => None,
        })
        .expect("frame painted as an image item");
    assert_eq!(img.pixel(img.width / 2, img.height - 2), [255, 0, 0, 255]);
    // The iframe keeps its CSS size (no aspect ratio from the raster).
    let r = page.rect_of(page.doc.element_by_id("f").unwrap()).unwrap();
    assert_eq!((r.w, r.h), (200.0, 100.0));
}

#[test]
fn frames_see_their_parent_and_exchange_messages() {
    let mut e = engine();
    let inner = r#"<script>
        window.addEventListener('message', function (ev) {
            if (ev.data && ev.data.ping) ev.source.postMessage({ pong: ev.data.ping * 2, sawParent: window.parent !== window, top: window.top === window.parent, name: window.name }, '*');
        });
        parent.postMessage({ hello: 'from child', list: [1, 2, { x: new Date(5) }], m: new Map([[1, 'a']]) }, '*');
    </script>"#;
    let mut page = load(
        &mut e,
        &format!(
            r#"<script>
                window.got = [];
                window.addEventListener('message', function (ev) {{
                    var f = document.getElementById('f');
                    got.push(ev.source === f.contentWindow);
                    got.push(JSON.stringify(ev.data.pong !== undefined ? ev.data : {{ hello: ev.data.hello, x: ev.data.list[2].x.getTime(), m: ev.data.m.get(1) }}));
                    got.push(ev.origin);
                    if (ev.data.hello) f.contentWindow.postMessage({{ ping: 21 }}, '*');
                }});
            </script>
            <iframe id="f" name="kid" srcdoc="{}"></iframe>
            <iframe id="g" srcdoc="<p>second</p>"></iframe>"#,
            escape_attr(inner)
        ),
    );
    assert_eq!(eval(&e, &mut page, "frames.length"), "2");
    assert_eq!(eval(&e, &mut page, "window[0] === document.getElementById('f').contentWindow"), "true");
    assert_eq!(eval(&e, &mut page, "window.parent === window && window.top === window"), "true");
    // srcdoc documents share the parent's origin: their document is scriptable.
    assert_eq!(eval(&e, &mut page, "document.getElementById('f').contentDocument.nodeName"), "#document");
    let got = eval(&e, &mut page, "got.join(' | ')");
    assert!(got.contains(r#"{"hello":"from child","x":5,"m":"a"}"#), "{got}");
    assert!(got.contains(r#""pong":42"#), "{got}");
    assert!(got.contains(r#""sawParent":true"#), "{got}");
    assert!(got.contains(r#""name":"kid""#), "{got}");
    assert!(got.starts_with("true"), "{got}");
    // The child sees its own realm.
    let c = child(&mut page, 0);
    assert_eq!(eval(&e, c, "typeof got"), "undefined");
    assert_eq!(eval(&e, c, "parent.frames.length"), "2");
}

#[test]
fn target_origin_mismatch_drops_the_message() {
    let mut e = engine();
    let inner = r#"<script>parent.postMessage('wrong', 'https://other.test'); parent.postMessage('right', 'https://example.test');</script>"#;
    let mut page = load(
        &mut e,
        &format!(r#"<script>window.got = []; addEventListener('message', function (e) {{ got.push(e.data); }});</script><iframe srcdoc="{}"></iframe>"#, escape_attr(inner)),
    );
    assert_eq!(eval(&e, &mut page, "got.join(',')"), "right");
}

#[test]
fn clicks_and_keys_are_routed_into_the_frame() {
    let mut e = engine();
    let inner = r#"<style>body{margin:0}</style><button id="b" style="width:100px;height:40px" onclick="document.body.dataset.clicked = 'yes'">go</button><input id="i" style="display:block;width:100px;height:20px">"#;
    let mut page = load(
        &mut e,
        &format!(
            r#"<body style="margin:0"><script>window.parentClicks = 0; addEventListener('click', function () {{ parentClicks++; }});</script><div style="height:50px"></div><iframe id="f" style="border:0;margin-left:20px;width:300px;height:200px" srcdoc="{}"></iframe>"#,
            escape_attr(inner)
        ),
    );
    // The button is at (0..100, 0..40) inside the frame at (20, 50).
    e.click_at(&mut page, 70.0, 70.0, Modifiers::default());
    settle(&e, &mut page);
    assert_eq!(eval(&e, child(&mut page, 0), "document.body.dataset.clicked"), "yes");
    assert_eq!(eval(&e, &mut page, "parentClicks"), "0", "clicks inside a frame stay in the frame");
    assert_eq!(eval(&e, &mut page, "document.activeElement.id"), "f");
    // Focus the input inside the frame and type.
    e.click_at(&mut page, 70.0, 105.0, Modifiers::default());
    for k in ["h", "i"] {
        e.frame_key_event(&mut page, k, "", true, Modifiers::default()).expect("routed");
        e.frame_key_event(&mut page, k, "", false, Modifiers::default());
    }
    assert_eq!(eval(&e, child(&mut page, 0), "document.getElementById('i').value"), "hi");
}

#[test]
fn nested_frames_and_src_changes() {
    let mut e = engine();
    let grandchild = "<p>deep</p>";
    let inner = format!(r#"<iframe srcdoc="{}"></iframe>"#, escape_attr(grandchild));
    let mut page = load(&mut e, &format!(r#"<iframe id="f" srcdoc="{}"></iframe>"#, escape_attr(&inner)));
    let c = child(&mut page, 0);
    assert_eq!(c.frames.children.len(), 1);
    let g = c.frames.children[0].page.as_mut().expect("grandchild");
    assert!(g.text().contains("deep"));
    assert_eq!(eval(&e, g, "window.top === window.parent.parent && window.top !== window.parent"), "true");
    // Removing the iframe drops its browsing context.
    eval(&e, &mut page, "document.getElementById('f').remove()");
    settle(&e, &mut page);
    assert!(page.frames.children.is_empty());
}

#[test]
fn message_ports_can_be_transferred_between_frames() {
    let mut e = engine();
    let inner = r#"<script>
        addEventListener('message', function (ev) {
            var port = ev.ports[0];
            port.onmessage = function (m) { port.postMessage('echo:' + m.data); };
            port.postMessage('ready:' + ev.data);
        });
    </script>"#;
    let mut page = load(
        &mut e,
        &format!(
            r#"<script>
                window.log = [];
                function go() {{
                    var ch = new MessageChannel();
                    ch.port1.onmessage = function (m) {{ log.push(m.data); if (m.data.indexOf('ready') === 0) ch.port1.postMessage('ping'); }};
                    document.getElementById('f').contentWindow.postMessage('hello', '*', [ch.port2]);
                }}
            </script><iframe id="f" onload="go()" srcdoc="{}"></iframe>"#,
            escape_attr(inner)
        ),
    );
    settle(&e, &mut page);
    assert_eq!(eval(&e, &mut page, "log.join(',')"), "ready:hello,echo:ping");
}

#[test]
fn same_origin_content_document_is_scriptable() {
    let mut e = engine();
    let mut page = load(&mut e, r#"<iframe id="f" srcdoc="<p id=p>child</p>"></iframe>"#);
    assert_eq!(eval(&e, &mut page, "document.getElementById('f').contentDocument.getElementById('p').textContent"), "child");
    assert_eq!(eval(&e, &mut page, "var f = document.getElementById('f'); f.contentWindow.document === f.contentDocument"), "true");
    // Listener registration on another frame's WindowProxy must not throw.
    assert_eq!(eval(&e, &mut page, "var w = document.getElementById('f').contentWindow; w.addEventListener('message', function () {}); w.removeEventListener('message', function () {}); typeof w.addEventListener"), "function");
    eval(&e, &mut page, "document.getElementById('f').contentDocument.body.innerHTML = '<b id=b>changed</b>'");
    settle(&e, &mut page);
    let html = eval(&e, &mut page, "document.getElementById('f').contentDocument.body.innerHTML");
    let text = child(&mut page, 0).text();
    assert!(text.contains("changed"), "child text {text:?}, innerHTML seen from the parent {html:?}, dom {}", child(&mut page, 0).dom_dump());
    // An inserted iframe has its about:blank document right away.
    let r = eval(
        &e,
        &mut page,
        "var i = document.createElement('iframe'); document.body.appendChild(i); var d = i.contentDocument; var s = d.createElement('span'); s.textContent = 'dyn'; d.body.appendChild(s); [d.body.textContent, d.URL, i.contentWindow.parent === window].join()",
    );
    assert_eq!(r, "dyn,about:blank,true");
}

#[test]
fn window_proxies_expose_builtins_and_named_frames() {
    let mut e = engine();
    let inner = r#"<script>window.fromChild = parent.frames['other'] ? 'found' : 'missing';</script>"#;
    let mut page = load(
        &mut e,
        &format!(r#"<iframe id="a" name="other" srcdoc="<p>x</p>"></iframe><iframe id="b" srcdoc="{}"></iframe>"#, escape_attr(inner)),
    );
    // A same-origin frame's WindowProxy serves the ECMAScript globals
    // (scripts take pristine JSON from a fresh iframe).
    let r = eval(&e, &mut page, "(() => { const w = document.getElementById('a').contentWindow; return [typeof w.JSON, w.JSON.parse('[1,2]').length, 'Array' in w, typeof w.nonexistent].join(); })()");
    assert_eq!(r, "object,2,true,undefined");
    // Named access to child frames, from the parent and from a sibling.
    let r = eval(&e, &mut page, "[window.frames[0] === document.getElementById('a').contentWindow, typeof document.getElementById('b').contentWindow.parent.frames['other'].postMessage].join()");
    assert_eq!(r, "true,function");
    let c = child(&mut page, 1);
    assert_eq!(e.eval_in_page(c, "window.fromChild").trim_matches('\''), "found");
}

#[test]
fn resized_frame_is_painted_whole() {
    // A frame whose box grows after it was painted (reCAPTCHA's challenge
    // frame) must not keep the old raster's source rectangle.
    let mut e = engine();
    let inner = r#"<style>html,body{margin:0;background:rgb(0,0,255)}</style><p>inside</p>"#;
    let mut page = load(&mut e, &format!(r#"<body style="margin:0"><iframe id="f" style="border:0;width:200px;height:100px" srcdoc="{}"></iframe></body>"#, escape_attr(inner)));
    eval(&e, &mut page, "document.getElementById('f').style.cssText = 'border:0;width:300px;height:250px'");
    settle(&e, &mut page);
    let (src, image) = page
        .display_list
        .items
        .iter()
        .find_map(|it| match it {
            DisplayItem::Image { rect, src, image, .. } if rect.w == 300.0 && rect.h == 250.0 => Some((*src, image.clone())),
            _ => None,
        })
        .expect("resized frame painted");
    assert_eq!((src.w.round(), src.h.round()), (image.width as f32, image.height as f32), "source covers the whole raster");
    assert_eq!(image.pixel(image.width / 2, image.height - 2), [0, 0, 255, 255]);
}

#[test]
fn same_origin_child_scripts_its_parent() {
    let mut e = engine();
    let child_src = "<script>var d = parent.document; d.getElementById('out').textContent = 'from child:' + frameElement.id + ':' + (top.document === d) + ':' + d.title + ':' + (frameElement.ownerDocument === d); var s = d.createElement('span'); s.id = 'made'; d.body.appendChild(s);</script>";
    let mut page = load(&mut e, &format!(r#"<title>T</title><p id="out">none</p><iframe id="f" srcdoc="{}"></iframe>"#, escape_attr(child_src)));
    assert_eq!(eval(&e, &mut page, "document.getElementById('out').textContent"), "from child:f:true:T:true");
    assert_eq!(eval(&e, &mut page, "!!document.getElementById('made')"), "true");
    assert!(page.text().contains("from child"), "the parent was restyled: {}", page.text());
    // Later, from a timer in the child.
    let mut page2 = load(&mut e, &format!(r#"<p id="out">none</p><iframe srcdoc="{}"></iframe>"#, escape_attr("<script>setTimeout(function(){ parent.document.getElementById('out').textContent = 'timer'; }, 30);</script>")));
    settle(&e, &mut page2);
    assert_eq!(eval(&e, &mut page2, "document.getElementById('out').textContent"), "timer");
    assert!(page2.text().contains("timer"));
    // A top-level document has no frameElement.
    assert_eq!(eval(&e, &mut page2, "String(window.frameElement)"), "null");
}

#[test]
fn hover_feedback_comes_from_inside_frames() {
    let mut e = engine();
    let inner = r#"<style>html,body{margin:0}</style><a id="l" href="https://example.test/inner" style="display:block;height:40px">link</a><input id="t" style="display:block;height:30px;width:100px">"#;
    let page = load(
        &mut e,
        &format!(r#"<body style="margin:0"><iframe style="border:0;width:300px;height:200px;display:block" srcdoc="{}"></iframe><p style="margin:0;height:50px">outside</p></body>"#, escape_attr(inner)),
    );
    let over_link = page.hover_info(20.0, 20.0);
    assert_eq!(over_link.cursor, "pointer");
    assert_eq!(over_link.link.map(|u| u.to_string()).as_deref(), Some("https://example.test/inner"));
    let over_input = page.hover_info(20.0, 55.0);
    assert_eq!(over_input.cursor, "text");
    assert!(over_input.link.is_none());
    let outside = page.hover_info(20.0, 220.0);
    assert!(outside.link.is_none());
}

/// A same-origin parent drives its frame the way Speedometer does: events
/// it dispatches at the frame's nodes run the frame's listeners (in the
/// frame's realm, as that realm's event interfaces), `click()` runs the
/// frame's handlers and default actions, `contentWindow.f()` calls the
/// frame's functions (nodes come back as the parent's wrappers), and the
/// frame's microtasks run at the parent's microtask checkpoint.
#[test]
fn parent_scripts_drive_a_same_origin_frame() {
    let mut e = engine();
    let inner = r#"<input id=i><button id=b>b</button><input id=c type=checkbox>
      <script>var log = [];
        var i = document.getElementById('i');
        i.addEventListener('keydown', function (e) { log.push('key:' + e.key + ':' + e.keyCode + ':' + (e instanceof KeyboardEvent)); e.preventDefault(); });
        i.addEventListener('input', function (e) { log.push('input:' + e.target.value); });
        document.getElementById('b').onclick = function () { log.push('click'); Promise.resolve().then(function () { log.push('micro'); }); };
        function getLog() { return log.join(','); }
        function button() { return document.getElementById('b'); }
        var answer = { n: 42 };
      </script>"#;
    let mut page = load(&mut e, &format!(r#"<iframe id="f" srcdoc="{}"></iframe>"#, escape_attr(inner)));
    let r = eval(
        &e,
        &mut page,
        "(() => { const f = document.getElementById('f'), d = f.contentDocument, i = d.getElementById('i');
           i.value = 'hi'; i.dispatchEvent(new Event('input', { bubbles: true }));
           const ok = i.dispatchEvent(new f.contentWindow.KeyboardEvent('keydown', { key: 'Enter', keyCode: 13, bubbles: true, cancelable: true }));
           d.getElementById('b').click(); d.getElementById('c').click();
           return [ok, f.contentWindow.getLog(), f.contentWindow.button() === d.getElementById('b'), d.getElementById('c').checked, f.contentWindow.answer.n, typeof f.contentWindow.nothing].join('|'); })()",
    );
    assert_eq!(r, "false|input:hi,key:Enter:13:true,click|true|true|42|undefined");
    settle(&e, &mut page);
    assert_eq!(eval(&e, &mut page, "document.getElementById('f').contentWindow.getLog()"), "input:hi,key:Enter:13:true,click,micro");
}

/// A frame whose realm used ranges in `DOMParser` / `createHTMLDocument`
/// documents is removed: its realm goes away, and the parent's ranges
/// (live slots in the parent's own documents) must be untouched by it. A
/// thread-wide list of released slots used to make the parent's next
/// `createRange` index documents it doesn't have (the tab crashed) or free
/// its own ranges' slots. Nodes of the removed frame's document held by
/// the parent fail with a JS error, never a crash.
#[test]
fn removed_frame_realm_leaves_the_parent_ranges_alone() {
    let mut e = engine();
    let inner = r#"<p>child</p><script>
        var d = new DOMParser().parseFromString('<p>a</p><p>b</p>', 'text/html');
        var h = document.implementation.createHTMLDocument('x');
        for (var i = 0; i < 6; i++) { var r = d.createRange(); r.selectNodeContents(d.body); var s = h.createRange(); s.selectNodeContents(h.body); var t = document.createRange(); t.selectNodeContents(document.body); }
        </script>"#;
    let mut page = load(&mut e, &format!(r#"<p id="x">hello</p><iframe id="f" srcdoc="{}"></iframe>"#, escape_attr(inner)));
    assert_eq!(eval(&e, &mut page, "window.r1 = document.createRange(); var t = document.getElementById('x').firstChild; r1.setStart(t, 1); r1.setEnd(t, 3); window.kept = document.getElementById('f').contentDocument.body; r1.toString()"), "el");
    eval(&e, &mut page, "document.getElementById('f').remove()");
    settle(&e, &mut page);
    assert_eq!(page.frames.children.len(), 0);
    let r = eval(&e, &mut page, "var r2 = document.createRange(); r2.selectNodeContents(document.body); var p = new DOMParser().parseFromString('<i>z</i>', 'text/html'); var r3 = p.createRange(); r3.selectNodeContents(p.body); [r1.startContainer.nodeName, r1.startOffset, r1.endOffset, r1.toString(), r3.toString()].join()");
    assert_eq!(r, "#text,1,3,el,z");
    let r = eval(&e, &mut page, "var out; try { out = String(kept.firstChild && kept.firstChild.nodeName); } catch (err) { out = 'threw'; } out + ',' + document.getElementById('x').textContent");
    assert!(r.ends_with(",hello"), "{r}");
}
