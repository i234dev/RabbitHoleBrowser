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
