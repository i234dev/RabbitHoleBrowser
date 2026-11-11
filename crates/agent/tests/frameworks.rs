//! Framework conformance suite: small apps written with real framework
//! builds (vendored under `tests/fixtures/frameworks/vendor`, license
//! headers kept) served by a loopback HTTP server, driven through the
//! engine's pointer and keyboard entry points (hit testing at viewport
//! coordinates, `click_at`, typed keys), asserting the DOM after each step
//! and that no script error was reported.
//!
//! Every app follows the same contract (element ids), so one scenario
//! function drives all frameworks:
//! - counter: `#count`, `#inc`, `#dec`
//! - todo: `#todo-input`, `#todo-add`, `li.todo` (`.text`, `input.done`,
//!   `button.del`), `#todo-left`, `#todo-reverse` (keyed reordering: the
//!   same `<li>` nodes must move)
//! - form: `#name` → `#greeting`, `#upper` (controlled, forced to upper
//!   case), `#color` select, `#agree` enables `#submit`, `#result`
//! - router: `#link-home` / `#link-about` (pushState), `#view`, popstate
//! - fetch: `#load` → `#status` loading → `li.item` from `/api/items`
//! - transition: `#toggle` hides `#box` through a transition/animation
//! - ssr: `ssr.html` holds server-rendered markup that the app hydrates
//!   (nodes kept, events wired)
//!
//! Run one framework with e.g. `cargo test --release -p rabbithole-agent
//! --test frameworks react18`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use rabbithole_agent::actions;
use rabbithole_engine::pointer::ClickAction;
use rabbithole_engine::{Engine, Modifiers, Page, Viewport};
use serde_json::Value;

// ---------------------------------------------------------------- server

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/frameworks")
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| url_decode(v))
    })
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let h = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                match h {
                    Some(v) => {
                        out.push(v);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Dynamic endpoints: the JSON API used by the fetch scenario and the
/// HTML fragments htmx swaps in.
fn api(method: &str, path: &str, query: &str, body: &str) -> Option<(u16, &'static str, String)> {
    let form = |k: &str| query_param(body, k);
    let all = |k: &str| -> Vec<String> { body.split('&').filter_map(|kv| kv.split_once('=')).filter(|(n, _)| *n == k).map(|(_, v)| url_decode(v)).collect() };
    Some(match (method, path) {
        ("GET", "/api/items") => {
            std::thread::sleep(Duration::from_millis(250));
            (200, "application/json", r#"[{"id":1,"name":"Apple"},{"id":2,"name":"Banana"},{"id":3,"name":"Cherry"}]"#.to_string())
        }
        ("POST", "/api/echo") => (200, "application/json", format!("{{\"got\":{}}}", serde_json::to_string(body).unwrap())),
        ("GET", "/api/htmx/items") => {
            std::thread::sleep(Duration::from_millis(250));
            (200, "text/html", "<li class=\"item\">Apple</li><li class=\"item\">Banana</li><li class=\"item\">Cherry</li>".to_string())
        }
        ("POST", "/api/htmx/counter") => {
            let n: i64 = form("n").and_then(|n| n.parse().ok()).unwrap_or(0) + form("d").and_then(|d| d.parse::<i64>().ok()).unwrap_or(0);
            (200, "text/html", format!("<span id=\"count\">{n}</span><input type=\"hidden\" name=\"n\" value=\"{n}\">"))
        }
        ("POST", "/api/htmx/todos") => {
            // The list state travels with the request: items[] (+ done[]),
            // the new text, and an operation.
            let mut items: Vec<(String, bool)> = all("item").into_iter().map(|t| (t, false)).collect();
            for d in all("done") {
                if let Some(i) = d.parse::<usize>().ok() {
                    if let Some(it) = items.get_mut(i) {
                        it.1 = true;
                    }
                }
            }
            match form("op").as_deref() {
                Some("add") => {
                    let t = form("text").unwrap_or_default();
                    if !t.trim().is_empty() {
                        items.push((t.trim().to_string(), false));
                    }
                }
                Some("reverse") => items.reverse(),
                Some(op) if op.starts_with("del") => {
                    if let Ok(i) = op[3..].parse::<usize>() {
                        if i < items.len() {
                            items.remove(i);
                        }
                    }
                }
                // "toggle<i>": the checkbox state already travels in done[].
                _ => {}
            }
            let mut html = String::from("<ul id=\"todo-list\">");
            for (i, (t, done)) in items.iter().enumerate() {
                html.push_str(&format!(
                    "<li class=\"todo\" id=\"todo-{key}\"><input type=\"hidden\" name=\"item\" value=\"{v}\"><input class=\"done\" type=\"checkbox\" name=\"done\" value=\"{i}\"{c} hx-post=\"/api/htmx/todos\" hx-vals='{{\"op\":\"toggle{i}\"}}' hx-include=\"#todos\" hx-target=\"#todos\"> <span class=\"text\">{v}</span> <button type=\"button\" class=\"del\" hx-post=\"/api/htmx/todos\" hx-vals='{{\"op\":\"del{i}\"}}' hx-include=\"#todos\" hx-target=\"#todos\">x</button></li>",
                    key = esc(&t.to_lowercase()),
                    v = esc(t),
                    c = if *done { " checked" } else { "" }
                ));
            }
            html.push_str("</ul>");
            let left = items.iter().filter(|i| !i.1).count();
            html.push_str(&format!("<p id=\"todo-left\">{left} left</p>"));
            (200, "text/html", html)
        }
        ("GET", "/api/htmx/view") => {
            let v = query_param(query, "v").unwrap_or_default();
            let t = if v == "about" { "About" } else { "Home" };
            (200, "text/html", format!("<div id=\"view\">{t}</div>"))
        }
        ("POST", "/api/htmx/form") => {
            let r = format!("{}|{}|{}|{}", form("name").unwrap_or_default(), form("upper").unwrap_or_default().to_uppercase(), form("color").unwrap_or_default(), if form("agree").is_some() { "yes" } else { "no" });
            (200, "text/html", format!("<p id=\"result\">{}</p>", esc(&r)))
        }
        ("GET", "/api/htmx/box") => {
            let show = query_param(query, "show").as_deref() == Some("1");
            (200, "text/html", if show { "<div id=\"box\">box</div>".to_string() } else { String::new() })
        }
        ("GET", "/api/htmx/greeting") => {
            let n = query_param(query, "name").unwrap_or_default();
            (200, "text/html", format!("Hello, {}!", esc(&n)))
        }
        _ => return None,
    })
}

fn static_file(path: &str) -> Option<(u16, &'static str, Vec<u8>)> {
    if path.contains("..") {
        return None;
    }
    let rel = path.trim_start_matches('/');
    let mut p = fixtures().join(rel);
    if p.is_dir() {
        p = p.join("index.html");
    }
    if let Ok(b) = std::fs::read(&p) {
        return Some((200, content_type(&p.to_string_lossy()), b));
    }
    // History-API routes of an app: /fw/<name>/<route> serves its index.
    let mut parts = rel.split('/');
    if parts.next() == Some("fw") {
        if let Some(name) = parts.next() {
            let last = rel.rsplit('/').next().unwrap_or("");
            if !last.contains('.') {
                let idx = fixtures().join("fw").join(name).join("index.html");
                if let Ok(b) = std::fs::read(&idx) {
                    return Some((200, "text/html; charset=utf-8", b));
                }
            }
        }
    }
    None
}

fn handle(stream: std::net::TcpStream) {
    let mut w = match stream.try_clone() {
        Ok(w) => w,
        Err(_) => return,
    };
    let mut r = BufReader::new(stream);
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let mut it = line.split_whitespace();
        let method = it.next().unwrap_or("GET").to_string();
        let target = it.next().unwrap_or("/").to_string();
        let mut len = 0usize;
        loop {
            let mut h = String::new();
            if r.read_line(&mut h).unwrap_or(0) == 0 {
                return;
            }
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                if k.trim().eq_ignore_ascii_case("content-length") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
        }
        let mut body = vec![0; len];
        if r.read_exact(&mut body).is_err() {
            return;
        }
        let body = String::from_utf8_lossy(&body).into_owned();
        let (path, query) = target.split_once('?').unwrap_or((&target, ""));
        let (status, ct, payload) = match api(&method, path, query, &body) {
            Some((s, ct, b)) => (s, ct, b.into_bytes()),
            None => static_file(path).unwrap_or((404, "text/plain", b"not found".to_vec())),
        };
        let head = format!("HTTP/1.1 {status} OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\r\n", payload.len());
        if w.write_all(head.as_bytes()).and_then(|_| w.write_all(&payload)).is_err() {
            return;
        }
    }
}

fn server() -> u16 {
    static PORT: OnceLock<u16> = OnceLock::new();
    *PORT.get_or_init(|| {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                std::thread::spawn(move || handle(s));
            }
        });
        port
    })
}

// ---------------------------------------------------------------- driver

/// `$(sel)` / `$$(sel)`: elements by CSS selector across open shadow
/// roots (document order, shadow trees after their host's light tree).
const DEEP_QUERY: &str = r#"var $$ = function (sel) {
  var out = [];
  (function walk(root) {
    Array.prototype.push.apply(out, root.querySelectorAll(sel));
    var all = root.querySelectorAll('*');
    for (var i = 0; i < all.length; i++) if (all[i].shadowRoot) walk(all[i].shadowRoot);
  })(document);
  return out; };
var $ = function (sel) { return $$(sel)[0] || null; };"#;

struct App {
    engine: Engine,
    page: Page,
    name: String,
}

impl App {
    fn open(fw: &str, file: &str) -> App {
        let mut engine = Engine::new_bundled_fonts_only();
        engine.scripting = true;
        engine.loader.load_scripts = true;
        let url = format!("http://127.0.0.1:{}/fw/{fw}/{file}", server());
        let page = engine.load(&url, Viewport { width: 1000.0, height: 1400.0 }).unwrap_or_else(|e| panic!("{fw}: load {url}: {e:?}"));
        let mut app = App { engine, page, name: format!("{fw}/{file}") };
        if !app.wait("window.__ready === true", 15000) {
            let body = app.js("document.body.innerHTML.slice(0, 2000)");
            panic!("{}: app never became ready; errors: {:?}; body: {body}", app.name, app.errors());
        }
        app
    }

    /// Evaluate `expr` in the page; the result as JSON.
    fn js(&mut self, expr: &str) -> Value {
        let src = format!("(function(){{ {DEEP_QUERY} try {{ return JSON.stringify({{ v: ({expr}) }}); }} catch (e) {{ return JSON.stringify({{ err: String(e) }}); }} }})()");
        let s = self.engine.eval_in_page(&mut self.page, &src);
        let s = s.trim();
        let s = s.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')).or_else(|| s.strip_prefix('"').and_then(|s| s.strip_suffix('"'))).unwrap_or(s);
        let v: Value = serde_json::from_str(s).unwrap_or_else(|_| serde_json::from_str(&s.replace("\\'", "'")).unwrap_or(Value::String(s.to_string())));
        match v.get("v") {
            Some(v) => v.clone(),
            None => panic!("{}: `{expr}` threw: {v}", self.name),
        }
    }

    fn texts(&mut self, sel: &str) -> Vec<String> {
        let v = self.js(&format!("$$({sel:?}).map(function(e){{ return e.textContent.trim(); }})"));
        v.as_array().map(|a| a.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default()
    }

    /// Run timers, network completions and rendering for `ms`.
    fn pump(&mut self, ms: u64) {
        let end = Instant::now() + Duration::from_millis(ms);
        loop {
            let o = self.engine.tick(&mut self.page);
            if let Some(n) = o.navigation {
                panic!("{}: unexpected navigation to {}", self.name, n.request.url);
            }
            if Instant::now() >= end {
                break;
            }
            std::thread::sleep(Duration::from_millis(4));
        }
    }

    /// Pump until the JS condition holds (or `ms` elapses).
    fn wait(&mut self, cond: &str, ms: u64) -> bool {
        let end = Instant::now() + Duration::from_millis(ms);
        loop {
            if self.js(&format!("!!({cond})")) == Value::Bool(true) {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
            self.pump(8);
        }
    }

    #[track_caller]
    fn expect(&mut self, cond: &str, ms: u64) {
        if !self.wait(cond, ms) {
            let body = self.js("document.body.innerHTML.slice(0, 3000)");
            panic!("{}: timed out waiting for `{cond}`\nerrors: {:?}\nbody: {body}", self.name, self.errors());
        }
    }

    #[track_caller]
    fn expect_text(&mut self, sel: &str, want: &str) {
        let cond = format!("(function(){{ var e = $({sel:?}); return e && e.textContent.trim() === {want:?}; }})()");
        if !self.wait(&cond, 3000) {
            let got = self.js(&format!("(function(){{ var e = $({sel:?}); return e ? e.textContent.trim() : null; }})()"));
            panic!("{}: {sel} should read {want:?}, got {got}\nerrors: {:?}", self.name, self.errors());
        }
    }

    /// Viewport center of the element (scrolled into view first).
    fn center(&mut self, sel: &str) -> (f32, f32) {
        // Scroll it into view as the embedder would (apply the requested
        // scroll), then measure.
        self.js(&format!("(function(){{ var e = $({sel:?}); if (e) e.scrollIntoView({{block:'nearest'}}); return 1; }})()"));
        self.sync_scroll();
        let v = self.js(&format!(
            "(function(){{ var e = $({sel:?}); if (!e) return null; var r = e.getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2, r.width, r.height]; }})()"
        ));
        if !v.is_array() {
            let body = self.js("document.body.innerHTML.slice(0, 2000)");
            panic!("{}: no element {sel}; body: {body}", self.name);
        }
        let a = v.as_array().unwrap();
        let f = |i: usize| a[i].as_f64().unwrap() as f32;
        assert!(f(2) > 0.0 && f(3) > 0.0, "{}: {sel} has no box ({}x{})", self.name, f(2), f(3));
        // The element under that point must be `sel` (or inside it).
        let hit = self.js(&format!("(function(){{ var e = $({sel:?}), h = document.elementFromPoint({}, {}); while (h && h.shadowRoot) {{ var inner = h.shadowRoot.elementFromPoint({}, {}); if (!inner || inner === h) break; h = inner; }} return !!h && (h === e || e.contains(h)); }})()", f(0), f(1), f(0), f(1)));
        assert_eq!(hit, Value::Bool(true), "{}: {sel} is covered at its center ({}, {})", self.name, f(0), f(1));
        (f(0), f(1))
    }

    /// Apply a scroll position requested by scripts, as the embedder does.
    fn sync_scroll(&mut self) {
        if let Some((x, y)) = self.page.requested_scroll.take() {
            self.engine.set_page_scroll(&mut self.page, x, y);
        }
        self.engine.rebuild_display_list(&mut self.page);
    }

    /// A real click: hit test at the element's center, then the pointer
    /// down/up sequence and the default action (the GUI's path).
    fn click(&mut self, sel: &str) {
        let (x, y) = self.center(sel);
        let up = self.engine.click_at(&mut self.page, x, y, Modifiers::default());
        let mut out = up.outcome;
        match up.action {
            ClickAction::Navigate { url, .. } => panic!("{}: click on {sel} navigated to {url}", self.name),
            ClickAction::Submit { form, submitter } => {
                if let Some(req) = actions::submit_request(&self.engine, &mut self.page, form, submitter, &mut out) {
                    panic!("{}: form submission was not intercepted ({})", self.name, req.url);
                }
            }
            _ => {}
        }
        if let Some(n) = out.navigation {
            panic!("{}: click on {sel} navigated to {}", self.name, n.request.url);
        }
        self.pump(10);
    }

    /// Click into a text field and type `text` key by key.
    fn type_into(&mut self, sel: &str, text: &str) {
        self.click(sel);
        let node = self.page.state.focused.unwrap_or_else(|| panic!("{}: clicking {sel} did not focus it", self.name));
        let out = actions::type_text(&self.engine, &mut self.page, node, text, true).unwrap_or_else(|e| panic!("{}: typing into {sel}: {e}", self.name));
        if let Some(n) = out.navigation {
            panic!("{}: typing navigated to {}", self.name, n.request.url);
        }
        self.pump(10);
    }

    /// Choose an option of a `<select>` (focus by click, then pick).
    fn choose(&mut self, sel: &str, value: &str) {
        self.click(sel);
        let node = self.page.state.focused.unwrap_or_else(|| panic!("{}: clicking {sel} did not focus it", self.name));
        actions::fill_control(&self.engine, &mut self.page, node, value).unwrap_or_else(|e| panic!("{}: choose {value}: {e}", self.name));
        self.pump(10);
    }

    fn press(&mut self, key: &str) {
        let r = actions::press_key(&self.engine, &mut self.page, None, key);
        if let Some(req) = r.request {
            panic!("{}: key {key} navigated to {}", self.name, req.url);
        }
        self.pump(10);
    }

    fn errors(&self) -> Vec<String> {
        self.page
            .script
            .console
            .iter()
            .filter(|(level, msg)| level == "error" || msg.starts_with("Uncaught") || msg.contains("[Vue warn]") || msg.contains("hydration_mismatch"))
            .map(|(l, m)| format!("{l}: {m}"))
            .collect()
    }

    #[track_caller]
    fn no_errors(&self) {
        let e = self.errors();
        assert!(e.is_empty(), "{}: script errors: {e:#?}", self.name);
    }
}

// ---------------------------------------------------------------- scenarios

fn counter(fw: &str) {
    let mut a = App::open(fw, "");
    a.expect_text("#count", "0");
    for _ in 0..3 {
        a.click("#inc");
    }
    a.expect_text("#count", "3");
    a.click("#dec");
    a.expect_text("#count", "2");
    a.no_errors();
}

fn todo(fw: &str, keyed: bool) {
    let mut a = App::open(fw, "");
    for (i, t) in ["Alpha", "Beta", "Gamma"].iter().enumerate() {
        a.type_into("#todo-input", t);
        a.click("#todo-add");
        a.expect(&format!("$$('li.todo').length === {}", i + 1), 3000);
        a.expect("$('#todo-input').value === ''", 3000);
        // Let the framework settle the new row (htmx wires swapped content
        // after its settle delay) before using it.
        a.pump(60);
    }
    assert_eq!(a.texts("li.todo .text"), ["Alpha", "Beta", "Gamma"], "{fw}");
    a.expect_text("#todo-left", "3 left");
    a.click("li.todo:nth-of-type(2) input.done");
    a.expect_text("#todo-left", "2 left");
    assert_eq!(a.js("$('li.todo:nth-of-type(2) input.done').checked"), Value::Bool(true), "{fw}: checkbox state");
    a.js("$$('li.todo').forEach(function(li){ li.__mark = li.querySelector('.text').textContent.trim(); }), 1");
    a.click("#todo-reverse");
    a.expect("$('li.todo .text').textContent.trim() === 'Gamma'", 3000);
    a.pump(60);
    assert_eq!(a.texts("li.todo .text"), ["Gamma", "Beta", "Alpha"], "{fw}");
    if keyed {
        let marks = a.js("$$('li.todo').map(function(li){ return li.__mark || null; })");
        assert_eq!(marks, serde_json::json!(["Gamma", "Beta", "Alpha"]), "{fw}: keyed reorder must move the existing <li> nodes");
    }
    a.expect("$('li.todo:nth-of-type(2) input.done').checked === true", 3000);
    a.click("li.todo:nth-of-type(2) button.del");
    a.expect("$$('li.todo').length === 2", 3000);
    assert_eq!(a.texts("li.todo .text"), ["Gamma", "Alpha"], "{fw}");
    a.expect_text("#todo-left", "2 left");
    if keyed {
        let marks = a.js("$$('li.todo').map(function(li){ return li.__mark || null; })");
        assert_eq!(marks, serde_json::json!(["Gamma", "Alpha"]), "{fw}: removal keeps the other nodes");
    }
    // Enter in the field adds too (keyboard path).
    a.type_into("#todo-input", "Delta");
    a.press("Enter");
    a.expect("$$('li.todo').length === 3", 3000);
    assert_eq!(a.texts("li.todo .text"), ["Gamma", "Alpha", "Delta"], "{fw}");
    a.no_errors();
}

fn form(fw: &str) {
    let mut a = App::open(fw, "");
    a.type_into("#name", "Ada");
    a.expect_text("#greeting", "Hello, Ada!");
    a.type_into("#upper", "shout");
    a.expect("$('#upper').value === 'SHOUT'", 3000);
    a.choose("#color", "blue");
    a.expect_text("#color-out", "blue");
    assert_eq!(a.js("$('#submit').disabled"), Value::Bool(true), "{fw}: submit disabled until agree");
    a.click("#agree");
    a.expect("$('#submit').disabled === false", 3000);
    a.click("#submit");
    a.expect_text("#result", "Ada|SHOUT|blue|yes");
    a.no_errors();
}

fn router(fw: &str) {
    let mut a = App::open(fw, "");
    a.expect_text("#view", "Home");
    let base = a.js("location.pathname");
    a.click("#link-about");
    a.expect_text("#view", "About");
    assert!(a.js("location.pathname").as_str().unwrap().ends_with("/about"), "{fw}: pushState URL {:?}", a.js("location.href"));
    a.click("#link-home");
    a.expect_text("#view", "Home");
    assert_eq!(a.js("location.pathname"), base, "{fw}");
    a.js("history.back(), 1");
    a.expect_text("#view", "About");
    assert!(a.js("location.pathname").as_str().unwrap().ends_with("/about"), "{fw}");
    a.js("history.back(), 1");
    a.expect_text("#view", "Home");
    a.no_errors();
}

fn fetch(fw: &str) {
    let mut a = App::open(fw, "");
    a.click("#load");
    a.expect("$('#status') && $('#status').textContent.trim() === 'loading'", 3000);
    a.expect("$$('li.item').length === 3", 5000);
    assert_eq!(a.texts("li.item"), ["Apple", "Banana", "Cherry"], "{fw}");
    a.expect_text("#status", "loaded");
    a.no_errors();
}

fn transition(fw: &str) {
    let mut a = App::open(fw, "");
    let shown = "(function(){ var e = $('#box'); return !!e && getComputedStyle(e).display !== 'none'; })()";
    a.expect(shown, 2000);
    a.click("#toggle");
    // Still on screen while leaving (the transition runs)...
    assert_eq!(a.js(shown), Value::Bool(true), "{fw}: #box vanished without a transition");
    // ...and gone once it ends.
    a.expect(&format!("!{shown}"), 4000);
    a.click("#toggle");
    a.expect(shown, 3000);
    a.expect("(function(){ var e = $('#box'); return e && parseFloat(getComputedStyle(e).opacity) > 0.99; })()", 4000);
    a.no_errors();
}

fn ssr(fw: &str) {
    let mut a = App::open(fw, "ssr.html");
    // The server-rendered nodes are adopted, not replaced.
    assert_eq!(a.js("$('#count') === window.__ssr.count"), Value::Bool(true), "{fw}: #count was re-created instead of hydrated");
    assert_eq!(a.js("$('#inc') === window.__ssr.inc"), Value::Bool(true), "{fw}: #inc was re-created");
    a.expect_text("#count", "0");
    a.click("#inc");
    a.click("#inc");
    a.expect_text("#count", "2");
    assert_eq!(a.js("$('#count') === window.__ssr.count"), Value::Bool(true), "{fw}");
    // A conditional block that was rendered on the server and one that
    // was not: toggling both after hydration.
    a.expect_text("#cond", "shown");
    a.click("#cond-toggle");
    a.expect("!$('#cond')", 3000);
    a.click("#cond-toggle");
    a.expect_text("#cond", "shown");
    a.expect_text("#positive", "positive");
    if a.js("!!window.__expectClientOnly") == Value::Bool(true) {
        a.expect_text("#client-only", "client");
    }
    let items = a.texts("li.ssr-item");
    assert_eq!(items, ["one", "two", "three"], "{fw}");
    a.no_errors();
}

macro_rules! suite {
    ($fw:ident: $($scenario:ident $( ( $($arg:expr),* ) )?),* $(,)?) => {
        mod $fw {
            $(
                #[test]
                fn $scenario() {
                    super::$scenario(stringify!($fw) $( $(, $arg)* )?);
                }
            )*
        }
    };
}

suite!(react18: counter, todo(true), form, router, fetch, transition, ssr);
suite!(react19: counter, todo(true), form, router, fetch, transition, ssr);
suite!(vue: counter, todo(true), form, router, fetch, transition, ssr);
suite!(svelte: counter, todo(true), form, router, fetch, transition, ssr);
suite!(lit: counter, todo(true), form, router, fetch, transition, ssr);
suite!(preact: counter, todo(true), form, router, fetch, transition, ssr);
suite!(solid: counter, todo(true), form, router, fetch, transition);
suite!(alpine: counter, todo(true), form, router, fetch, transition);
suite!(htmx: counter, todo(false), form, router, fetch, transition);
suite!(jquery: counter, todo(true), form, router, fetch, transition);

