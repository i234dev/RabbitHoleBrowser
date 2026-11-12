//! Text fields of decorated (Material-style) forms and infinite-scroll
//! feeds, driven through the content thread exactly like the UI does
//! (pointer, key, scroll and wheel messages).

use super::*;

/// Evaluate `js` in the tab and return the result (string quotes removed).
fn eval(h: &ContentHandle, rx: &Received, js: &str) -> String {
    h.send(ToContent::Eval(js.to_string()));
    let (_, r) = wait_for(rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::EvalResult(_))).expect("eval result");
    let FromContent::EvalResult(r) = r else { unreachable!() };
    r.trim().trim_matches(|c| c == '\'' || c == '"').to_string()
}

/// Viewport rect (x, y, w, h) of the first element matching `sel`.
fn rect_of(h: &ContentHandle, rx: &Received, sel: &str) -> (f32, f32, f32, f32) {
    let r = eval(h, rx, &format!("(() => {{ const r = document.querySelector({sel:?}).getBoundingClientRect(); return [r.x, r.y, r.width, r.height].join(','); }})()"));
    let v: Vec<f32> = r.split(',').map(|s| s.parse().unwrap_or_else(|_| panic!("rect {r:?}"))).collect();
    (v[0], v[1], v[2], v[3])
}

fn click(h: &ContentHandle, x: f32, y: f32) {
    let mods = rabbithole_engine::Modifiers::default();
    h.send(ToContent::PointerDown { x, y, button: 0, mods, click_count: 1 });
    h.send(ToContent::PointerUp { x, y, button: 0, mods, inside: true });
}

fn type_text(h: &ContentHandle, text: &str) {
    for c in text.chars() {
        let s = c.to_string();
        h.send(ToContent::Key(KeyInput { key: s.clone(), code: format!("Key{}", s.to_uppercase()), text: Some(s), pressed: true, ..Default::default() }));
    }
}

/// The newest frame received within `window`.
fn last_frame(rx: &Received, window: Duration) -> Option<Frame> {
    let mut last = None;
    while let Some((_, FromContent::Frame(f))) = wait_for(rx, 1, window, is_frame) {
        last = Some(f);
    }
    last
}

/// Pixels of `f` inside the viewport rect `r` darker than `max` in every
/// channel (the frame is painted at scroll 0, scale 1).
fn dark_pixels(f: &Frame, r: (f32, f32, f32, f32), max: u32) -> usize {
    let mut n = 0;
    for y in r.1.max(0.0) as u32..((r.1 + r.3) as u32).min(f.canvas.height) {
        for x in r.0.max(0.0) as u32..((r.0 + r.2) as u32).min(f.canvas.width) {
            let p = f.canvas.data[(y * f.canvas.width + x) as usize];
            if (p >> 16) & 0xff < max && (p >> 8) & 0xff < max && p & 0xff < max {
                n += 1;
            }
        }
    }
    n
}

/// A Google-style Material outlined text field: the `<input>` is a flex
/// item with `z-index: 1` inside a `position: relative` wrapper, followed
/// by an absolutely positioned outline that covers the whole field, and a
/// floating label with a white background that moves up on `:focus`.
const MATERIAL_FIELD: &str = r#"<!doctype html><html><head><style>
body{margin:20px;font-family:sans-serif}
.rFrNMe{display:inline-block;outline:none;padding-bottom:8px;width:200px}
.aCsJod{height:40px;position:relative;vertical-align:top}
.aXBtI{display:flex;position:relative;top:14px}
.Xb9hP{display:flex;flex-grow:1;flex-shrink:1;min-width:0%;position:relative}
.whsOnd{flex-grow:1;flex-shrink:1;background-color:transparent;border:none;display:block;font:400 16px sans-serif;height:24px;line-height:24px;margin:0;min-width:0%;outline:none;padding:0;z-index:0}
.AxOyFc{transform-origin:bottom left;color:rgba(0,0,0,0.38);font:400 16px sans-serif;pointer-events:none;position:absolute;bottom:3px;left:0;width:100%}
.rFrNMe.CDELXb .AxOyFc,.whsOnd:not([disabled]):focus~.AxOyFc{transform:scale(.75) translateY(-39px)}
.X3mtXb{box-sizing:content-box;display:block;padding:16px 0 0;width:100%}
.X3mtXb .oJeWuf{height:56px;padding-top:0}
.X3mtXb .Wic03c{align-items:center;position:static;top:0}
.X3mtXb .snByac{background:#fff;color:#444746;bottom:17px;box-sizing:border-box;font-size:1rem;left:8px;max-width:calc(100% - 16px);overflow:hidden;padding:0 8px;white-space:nowrap;width:auto;z-index:1}
.X3mtXb .zHQkBf{border-radius:4px;color:#1f1f1f;font-size:1rem;height:28px;margin:1px 1px 0 1px;padding:13px 15px;text-align:left;width:100%;z-index:1}
.X3mtXb.u3bW4e .zHQkBf{margin:2px 2px 0 2px;padding:12px 14px}
.i9lrp{background-color:rgba(0,0,0,0.12);bottom:-2px;height:1px;left:0;position:absolute;width:100%}
.X3mtXb .mIZh1c{background-color:transparent;height:auto;top:0;border:1px solid #747775;border-radius:4px;box-sizing:border-box;bottom:0}
.wrap{width:360px}
</style></head><body><div class="wrap">
<div class="rFrNMe X3mtXb" id="field"><div class="aCsJod oJeWuf"><div class="aXBtI Wic03c"><div class="Xb9hP"><input type="email" class="whsOnd zHQkBf" id="identifierId" value=""><div class="AxOyFc snByac" aria-hidden="true">Email or phone</div></div><div class="i9lrp mIZh1c"></div></div></div></div>
</div><script>
var f = document.getElementById('field'), i = document.getElementById('identifierId');
i.addEventListener('focus', function () { f.classList.add('u3bW4e'); });
i.addEventListener('input', function () { f.classList.toggle('CDELXb', i.value !== ''); });
</script></body></html>"#;

#[test]
fn typing_into_a_material_outlined_field_is_visible() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, MATERIAL_FIELD);
    wait_for(&rx, 1, Duration::from_secs(10), is_frame).expect("frame");
    let r = rect_of(&h, &rx, "#identifierId");
    let (cx, cy) = (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0);
    // Content-box sizing: 28px tall plus 13px padding above and below.
    assert!((r.3 - 54.0).abs() < 0.5, "input rect {r:?}");
    // The input (a z-index:1 flex item) is above the outline drawn over it.
    assert_eq!(eval(&h, &rx, &format!("document.elementFromPoint({cx}, {cy}).id")), "identifierId");
    click(&h, cx, cy);
    type_text(&h, "hello");
    assert_eq!(eval(&h, &rx, "document.activeElement.id + ':' + document.getElementById('identifierId').value"), "identifierId:hello");
    let f = last_frame(&rx, Duration::from_millis(400)).expect("frame after typing");
    // The typed text is painted inside the field (the floating label moved
    // away; nothing covers the input): dark glyph pixels, color #1f1f1f.
    let inner = (r.0 + 10.0, r.1 + 8.0, r.2 - 20.0, r.3 - 16.0);
    let dark = dark_pixels(&f, inner, 0x40);
    assert!(dark > 40, "typed text not visible: {dark} dark pixels in {inner:?}");
}

#[test]
fn script_focus_moves_typing_into_the_field() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    // A decorated field whose click handler focuses the real input under
    // a full-size overlay, plus a field focused by a script on load.
    load(
        &h,
        "<html><body style='margin:0'>\
         <div style='position:relative;width:300px;height:40px'><input id=q style='width:280px;height:30px'><div id=cover style='position:absolute;inset:0'></div></div>\
         <input id=auto style='width:200px;height:30px'>\
         <script>var q = document.getElementById('q'), auto = document.getElementById('auto');\
         document.getElementById('cover').onclick = function () { q.focus(); }; auto.focus();</script></body></html>",
    );
    wait_for(&rx, 1, Duration::from_secs(10), is_frame).expect("frame");
    // Focused by the page's script: typing goes there right away.
    type_text(&h, "xy");
    assert_eq!(eval(&h, &rx, "auto.value"), "xy");
    click(&h, 20.0, 20.0);
    type_text(&h, "ab");
    assert_eq!(eval(&h, &rx, "document.activeElement.id + ':' + q.value + ':' + auto.value"), "q:ab:xy");
    // A script blur ends the edit: keys stop typing into the field.
    eval(&h, &rx, "q.blur()");
    wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::State(s) if !s.editing)).expect("edit ended");
    type_text(&h, "z");
    assert_eq!(eval(&h, &rx, "q.value"), "ab");
}

// ------------------------------------------------------------- feeds

const POST_CSS: &str = "body{margin:0;font:16px sans-serif} .post{height:300px;border-bottom:1px solid #ccc;background:#eef}";

/// The feed pages: `/io`, `/scroll`, `/partial`, `/nested` and the
/// fragments they fetch (`/page?n=K`: five posts; `/partial?n=K`: five
/// posts and the next lazy partial).
fn feed_route(path: &str) -> (&'static str, String) {
    let n: u32 = path.split("n=").nth(1).and_then(|v| v.parse().ok()).unwrap_or(0);
    let posts = |label: &str| (0..5).map(|i| format!("<div class='post'>{label} {n} item {i}</div>")).collect::<String>();
    let page = |body: &str| format!("<!doctype html><html><head><style>{POST_CSS}</style></head><body>{body}</body></html>");
    if path.starts_with("/page") {
        ("text/html", posts("page"))
    } else if path.starts_with("/partial?") {
        let next = if n < 3 { format!("<faceplate-partial loading='lazy' src='/partial?n={}'><div>Loading</div></faceplate-partial>", n + 1) } else { String::new() };
        ("text/html", format!("{}{next}", posts("partial")))
    } else if path.starts_with("/io") {
        // (a) IntersectionObserver on a sentinel below the feed; every new
        // post is observed too (elements added later).
        ("text/html", page(r#"<div id="feed"></div><div id="sentinel" style="height:1px"></div><script>
var page = 0, loading = false, log = [], seen = 0;
var feed = document.getElementById('feed');
var watch = new IntersectionObserver(function (es) { es.forEach(function (e) { if (e.isIntersecting) seen++; }); }, { threshold: 0.5 });
function add(html) { var t = document.createElement('div'); t.innerHTML = html; while (t.firstChild) { watch.observe(t.firstChild); feed.appendChild(t.firstChild); } }
add('<div class="post">initial 0</div><div class="post">initial 1</div><div class="post">initial 2</div><div class="post">initial 3</div><div class="post">initial 4</div>');
var io = new IntersectionObserver(function (entries) {
  entries.forEach(function (e) {
    log.push('io ' + e.isIntersecting + ' ' + Math.round(e.boundingClientRect.top) + ' ' + Math.round(e.rootBounds.height));
    if (e.isIntersecting && !loading) {
      loading = true;
      fetch('/page?n=' + (++page)).then(function (r) { return r.text(); }).then(function (t) { add(t); loading = false; log.push('loaded ' + page); });
    }
  });
}, { rootMargin: '0px 0px 200px 0px' });
io.observe(document.getElementById('sentinel'));
</script>"#))
    } else if path.starts_with("/scroll") {
        // (b) scroll listeners on window and document, XHR; `html` is
        // viewport-tall (the document overflows it).
        ("text/html", page(r#"<style>html,body{height:100%}</style><div id="feed"></div><script>
var page = 0, loading = false, log = [];
var feed = document.getElementById('feed');
function add(html) { feed.insertAdjacentHTML('beforeend', html); }
for (var i = 0; i < 5; i++) add('<div class="post">initial ' + i + '</div>');
function check(src) {
  var de = document.documentElement;
  log.push(src + ' ' + window.scrollY + ' ' + window.pageYOffset + ' ' + de.scrollTop + ' ' + document.scrollingElement.scrollTop + ' ' + de.scrollHeight + ' ' + window.innerHeight);
  if (!loading && window.scrollY + window.innerHeight >= de.scrollHeight - 300) {
    loading = true;
    var x = new XMLHttpRequest();
    x.open('GET', '/page?n=' + (++page));
    x.onload = function () { add(x.responseText); loading = false; log.push('loaded ' + page); };
    x.send();
  }
}
window.addEventListener('scroll', function () { check('w'); });
document.addEventListener('scroll', function (e) { check(e.target === document ? 'd' : 'x'); });
</script>"#))
    } else if path.starts_with("/partial-feed") {
        // (c) a custom element that loads its `src` when it gets near the
        // viewport and replaces itself (like Reddit's faceplate-partial).
        ("text/html", page(r#"<style>faceplate-partial{display:block}</style><div id="feed">
<div class="post">initial 0</div><div class="post">initial 1</div><div class="post">initial 2</div><div class="post">initial 3</div>
<faceplate-partial loading="lazy" src="/partial?n=1"><div>Loading</div></faceplate-partial></div><script>
var log = [];
class FaceplatePartial extends HTMLElement {
  connectedCallback() {
    if (this.getAttribute('loading') !== 'lazy') return this.load();
    this._io = new IntersectionObserver((es) => {
      for (const e of es) if (e.isIntersecting) { this._io.disconnect(); this.load(); }
    }, { rootMargin: '0px 0px 50% 0px' });
    this._io.observe(this);
  }
  disconnectedCallback() { if (this._io) this._io.disconnect(); }
  async load() {
    log.push('load ' + this.getAttribute('src'));
    const r = await fetch(this.getAttribute('src'));
    const html = await r.text();
    const t = document.createElement('template');
    t.innerHTML = html;
    this.replaceWith(t.content);
  }
}
customElements.define('faceplate-partial', FaceplatePartial);
</script>"#))
    } else {
        // (d) a feed inside a scroll container, observed with `root` = the
        // container.
        ("text/html", page(r#"<div id="box" style="margin:50px;height:400px;overflow:auto"><div class="post">initial 0</div><div class="post">initial 1</div><div class="post">initial 2</div><div id="sentinel" style="height:1px"></div></div><script>
var page = 0, loading = false, log = [];
var box = document.getElementById('box'), sentinel = document.getElementById('sentinel');
box.addEventListener('scroll', function () {
  log.push('s ' + box.scrollTop);
  // The classic check, on the container.
  if (box.scrollTop + box.clientHeight >= box.scrollHeight - 50) log.push('bottom');
});
new IntersectionObserver(function (entries) {
  entries.forEach(function (e) {
    log.push('io ' + e.isIntersecting + ' ' + Math.round(e.rootBounds.top) + ' ' + Math.round(e.rootBounds.height));
    if (e.isIntersecting && !loading) {
      loading = true;
      fetch('/page?n=' + (++page)).then(function (r) { return r.text(); }).then(function (t) { sentinel.insertAdjacentHTML('beforebegin', t); loading = false; log.push('loaded ' + page); });
    }
  });
}, { root: box, rootMargin: '0px 0px 100px 0px' }).observe(sentinel);
</script>"#))
    }
}

/// A loopback HTTP server answering with [`feed_route`].
fn serve_feed() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                let (ct, body) = feed_route(&path);
                let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = s.write_all(resp.as_bytes());
            });
        }
    });
    format!("http://{addr}")
}

fn navigate(h: &ContentHandle, url_str: &str) {
    h.send(ToContent::Navigate { request: Request::get(url(url_str)), push_history: true });
}

/// Wait for a frame whose document is taller than `min` (CSS px).
fn frame_taller_than(rx: &Received, min: f32, timeout: Duration) -> Frame {
    match wait_for(rx, 1, timeout, |m| matches!(m, FromContent::Frame(f) if f.content.1 > min)) {
        Some((_, FromContent::Frame(f))) => f,
        _ => panic!("no frame with a document taller than {min}"),
    }
}

/// Scroll the page from `from` to `to` like the UI's smooth scrolling: a
/// stream of `Scroll` messages a few ms apart (faster than one scroll
/// event per frame, so the last position needs the trailing event).
fn smooth_scroll(h: &ContentHandle, from: f32, to: f32) {
    let steps = 12;
    for i in 1..=steps {
        let y = from + (to - from) * i as f32 / steps as f32;
        h.send(ToContent::Scroll { x: 0.0, y });
        std::thread::sleep(Duration::from_millis(6));
    }
}

#[test]
fn infinite_scroll_with_an_intersection_observer_sentinel() {
    let base = serve_feed();
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    navigate(&h, &format!("{base}/io"));
    let f = frame_taller_than(&rx, 1400.0, Duration::from_secs(10));
    let h0 = f.content.1;
    assert!((h0 - 1506.0).abs() < 2.0, "{h0}");
    // At the top: the sentinel is reported, not intersecting.
    assert!(eval(&h, &rx, "log.join('|')").starts_with("io false 1505 800"), "{}", eval(&h, &rx, "log.join('|')"));
    // Scroll to the bottom: the sentinel enters the (extended) viewport,
    // the next page is fetched, appended, laid out and painted, and the UI
    // learns the taller document from the frame.
    smooth_scroll(&h, 0.0, h0 - 600.0);
    let f = frame_taller_than(&rx, h0 + 1000.0, Duration::from_secs(10));
    let h1 = f.content.1;
    assert!((h1 - (h0 + 1505.0)).abs() < 2.0, "{h1}");
    // Keep scrolling with the new height: one more page.
    smooth_scroll(&h, h0 - 600.0, h1 - 600.0);
    let h2 = frame_taller_than(&rx, h1 + 1000.0, Duration::from_secs(10)).content.1;
    assert!((h2 - (h1 + 1505.0)).abs() < 2.0, "{h2}");
    let log = eval(&h, &rx, "log.join('|')");
    assert!(log.contains("loaded 1") && log.contains("loaded 2"), "{log}");
    assert!(log.contains("io true"), "{log}");
    // Posts appended later were observed too, and seen when scrolled by.
    let seen: u32 = eval(&h, &rx, "seen").parse().unwrap();
    assert!(seen >= 4, "seen {seen}");
    let painted = eval(&h, &rx, "document.querySelectorAll('.post').length");
    assert_eq!(painted, "15");
}

#[test]
fn infinite_scroll_with_window_and_document_scroll_listeners() {
    let base = serve_feed();
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    navigate(&h, &format!("{base}/scroll"));
    let h0 = frame_taller_than(&rx, 1400.0, Duration::from_secs(10)).content.1;
    smooth_scroll(&h, 0.0, h0 - 600.0);
    let h1 = frame_taller_than(&rx, h0 + 1000.0, Duration::from_secs(10)).content.1;
    // Give the trailing (throttled) scroll event its frame.
    std::thread::sleep(Duration::from_millis(100));
    let log = eval(&h, &rx, "log.join('|')");
    let entries: Vec<&str> = log.split('|').collect();
    // Both the window and the document heard the scrolls, with consistent
    // positions (scrollY = pageYOffset = scrollTop = scrollingElement).
    assert!(entries.iter().any(|e| e.starts_with("d ")), "{log}");
    assert!(!entries.iter().any(|e| e.starts_with("x ")), "{log}");
    for e in entries.iter().filter(|e| e.starts_with("w ") || e.starts_with("d ")) {
        let v: Vec<&str> = e.split(' ').collect();
        assert!(v[1] == v[2] && v[2] == v[3] && v[3] == v[4], "{e}");
        assert_eq!(v[6], "600", "{e}");
    }
    // The last event saw the final position (the trailing scroll event).
    let last = entries.iter().rev().find(|e| e.starts_with("w ")).unwrap();
    assert_eq!(last.split(' ').nth(1).unwrap().parse::<f32>().unwrap(), h0 - 600.0, "{log}");
    assert!(log.contains("loaded 1"), "{log}");
    smooth_scroll(&h, h0 - 600.0, h1 - 600.0);
    frame_taller_than(&rx, h1 + 1000.0, Duration::from_secs(10));
}

#[test]
fn infinite_scroll_with_lazy_partial_custom_elements() {
    let base = serve_feed();
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    navigate(&h, &format!("{base}/partial-feed"));
    let mut height = frame_taller_than(&rx, 1100.0, Duration::from_secs(10)).content.1;
    let mut y = 0.0;
    // Each partial is fetched when it nears the viewport and replaced by
    // five posts plus the next partial (three in all).
    for n in 1..=3 {
        smooth_scroll(&h, y, height - 600.0);
        y = height - 600.0;
        height = frame_taller_than(&rx, height + 1000.0, Duration::from_secs(10)).content.1;
        let log = eval(&h, &rx, "log.join('|')");
        assert!(log.contains(&format!("load /partial?n={n}")), "{log}");
    }
    assert_eq!(eval(&h, &rx, "document.querySelectorAll('.post').length + ':' + document.querySelectorAll('faceplate-partial').length"), "19:0");
}

#[test]
fn infinite_scroll_inside_a_scroll_container() {
    let base = serve_feed();
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    navigate(&h, &format!("{base}/nested"));
    wait_for(&rx, 1, Duration::from_secs(10), is_frame).expect("frame");
    assert!(eval(&h, &rx, "log.join('|')").starts_with("io false 50 500"), "{}", eval(&h, &rx, "log.join('|')"));
    // Wheel over the container (the UI forwards it to the page).
    for _ in 0..12 {
        h.send(ToContent::Wheel { x: 200.0, y: 200.0, dx: 0.0, dy: 100.0, mods: Modifiers::default(), scrolled: false });
        std::thread::sleep(Duration::from_millis(20));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut log = String::new();
    while Instant::now() < deadline {
        log = eval(&h, &rx, "log.join('|')");
        if log.contains("loaded 1") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(log.contains("s "), "container scroll events: {log}");
    assert!(log.contains("io true"), "{log}");
    assert!(log.contains("loaded 1"), "{log}");
    assert!(log.contains("bottom"), "{log}");
    assert_eq!(eval(&h, &rx, "box.scrollHeight"), "2409");
    // Scripts read and set the container's real scroll position.
    let top: f32 = eval(&h, &rx, "box.scrollTop").parse().unwrap();
    assert!(top > 400.0, "{top}");
    assert_eq!(eval(&h, &rx, "box.scrollTop = 1500; box.scrollTop"), "1500");
    assert_eq!(eval(&h, &rx, "document.documentElement.scrollHeight"), "600");
}
