//! Single-page applications change the URL of their document in place:
//! fragment navigations (`location.hash`, `location.href = '#…'`,
//! `location.assign/replace` of a fragment, fragment links) and
//! `history.pushState/replaceState`. None of them may reload the page;
//! back/forward between those entries stays in the document too.
use super::*;
use crate::gui::tab::Tab;
use std::sync::atomic::AtomicUsize;

const APP: &str = r##"<!doctype html><html><head><title>spa</title></head><body style="margin:0">
<a id="link" href="#section" style="display:block;height:40px">section</a>
<div style="height:3000px"></div><p id="section">section</p>
<script>
window.marker = 'first-document';
window.events = [];
addEventListener('hashchange', function (e) { events.push('hashchange ' + new URL(e.oldURL).hash + ' > ' + new URL(e.newURL).hash); });
addEventListener('popstate', function (e) { events.push('popstate ' + location.pathname + location.hash + ' ' + (e.state && e.state.n)); });
</script></body></html>"##;

/// Serve `APP` at `/app` (and every path below), counting its fetches.
fn serve_app(hits: Arc<AtomicUsize>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let hits = hits.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                let resp = if path.starts_with("/app") {
                    hits.fetch_add(1, Ordering::SeqCst);
                    format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{APP}", APP.len())
                } else {
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
                };
                let _ = s.write_all(resp.as_bytes());
            });
        }
    });
    format!("http://{addr}")
}

/// The UI side: the tab's back/forward list and address bar, updated
/// from the content thread like `App` does, and the document loads seen.
struct Ui {
    h: ContentHandle,
    rx: Received,
    tab: Tab,
    commits: usize,
}

impl Ui {
    /// Handle UI messages until `pred` matches one (returned).
    fn until(&mut self, what: &str, mut pred: impl FnMut(&FromContent) -> bool) -> FromContent {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let (_, _, msg) = self.rx.recv_timeout(left).unwrap_or_else(|_| panic!("timed out waiting for {what}"));
            if std::env::var_os("RH_TEST_TRACE").is_some() && !matches!(msg, FromContent::Frame(_)) {
                eprintln!("[ui] {msg:?}");
            }
            match &msg {
                FromContent::Committed { url, push_history, .. } => {
                    self.commits += 1;
                    self.tab.current_url = Some(url.clone());
                    self.tab.loading = None;
                    self.tab.new_document();
                    if *push_history || self.tab.history.is_empty() { self.tab.push_history(url.clone()) } else { self.tab.replace_history(url.clone()) }
                }
                FromContent::UrlChanged { url, push_history } => {
                    self.tab.current_url = Some(url.clone());
                    if *push_history { self.tab.push_history(url.clone()) } else { self.tab.replace_history(url.clone()) }
                }
                FromContent::Navigate { request, push_history } => {
                    self.tab.loading = Some(request.url.clone());
                    self.h.send(ToContent::Navigate { request: request.clone(), push_history: *push_history });
                }
                _ => {}
            }
            if pred(&msg) {
                return msg;
            }
        }
    }

    fn eval(&mut self, js: &str) -> String {
        self.h.send(ToContent::Eval(js.to_string()));
        match self.until("eval result", |m| matches!(m, FromContent::EvalResult(_))) {
            FromContent::EvalResult(r) => r.trim_matches(|c| c == '\'' || c == '"').to_string(),
            _ => unreachable!(),
        }
    }

    /// Back/forward like `App::go_history_tab`.
    fn go(&mut self, delta: isize) {
        let target = (self.tab.index as isize + delta) as usize;
        let same = self.tab.same_document_entry(target);
        self.tab.index = target;
        let url = self.tab.history[target].clone();
        self.tab.current_url = Some(url.clone());
        if same {
            self.h.send(ToContent::TraverseSameDocument(url));
        } else {
            self.tab.loading = Some(url.clone());
            self.h.send(ToContent::Navigate { request: Request::get(url), push_history: false });
        }
    }

    fn address(&self) -> String {
        self.tab.current_url.as_ref().map(|u| u.to_string()).unwrap_or_default()
    }

    fn history(&self) -> Vec<String> {
        self.tab.history.iter().map(|u| format!("{}{}", u.path, u.fragment.as_ref().map(|f| format!("#{f}")).unwrap_or_default())).collect()
    }
}

#[test]
fn spa_url_changes_never_reload_the_document() {
    let hits = Arc::new(AtomicUsize::new(0));
    let base = serve_app(hits.clone());
    let (sink, rx) = sink();
    let mut ui = Ui { h: spawn(1, &sink), rx, tab: Tab::new(1), commits: 0 };
    ui.h.send(ToContent::Navigate { request: Request::get(url(&format!("{base}/app"))), push_history: true });
    ui.until("commit", |m| matches!(m, FromContent::Committed { .. }));
    // Scripts ran.
    let deadline = Instant::now() + Duration::from_secs(20);
    while ui.eval("String(window.marker)") != "first-document" {
        assert!(Instant::now() < deadline, "page scripts never ran");
        std::thread::sleep(Duration::from_millis(50));
    }

    // Every way a script changes the fragment or the URL in place, and a
    // native click on a fragment link.
    let steps: &[(&str, &str, bool)] = &[
        ("location.href = '#inbox'", "/app#inbox", true),
        ("location.assign('/app#inbox/42')", "/app#inbox/42", true),
        ("location.replace('#inbox/43')", "/app#inbox/43", false),
        ("location.hash = 'inbox/44'", "/app#inbox/44", true),
        ("location = '#inbox'", "/app#inbox", true),
        ("document.getElementById('link').click()", "/app#section", true),
        ("location.hash = 'x'", "/app#x", true),
        ("native click", "/app#section", true),
        ("history.pushState({n: 1}, '', '/app/view/7')", "/app/view/7", true),
        ("history.replaceState({n: 2}, '', '/app/view/8')", "/app/view/8", false),
        ("history.pushState({n: 3}, '', '#detail')", "/app/view/8#detail", true),
    ];
    let mods = rabbithole_engine::Modifiers::default();
    for (js, expect, push) in steps {
        if *js == "native click" {
            ui.h.send(ToContent::Scroll { x: 0.0, y: 0.0 });
            ui.h.send(ToContent::PointerDown { x: 10.0, y: 10.0, button: 0, mods, click_count: 1 });
            ui.h.send(ToContent::PointerUp { x: 10.0, y: 10.0, button: 0, mods, inside: true });
        } else {
            ui.h.send(ToContent::Eval(js.to_string()));
        }
        match ui.until(js, |m| matches!(m, FromContent::UrlChanged { .. })) {
            FromContent::UrlChanged { url: u, push_history } => {
                assert_eq!(format!("{}{}", u.path, u.fragment.as_ref().map(|f| format!("#{f}")).unwrap_or_default()), *expect, "{js}");
                assert_eq!(push_history, *push, "{js}");
            }
            _ => unreachable!(),
        }
        assert_eq!(ui.address(), format!("{base}{expect}"), "address bar after {js}");
    }
    assert_eq!(ui.eval("location.pathname + location.hash + ' ' + history.state.n + ' ' + history.length"), "/app/view/8#detail 3 10");
    assert_eq!(ui.history(), ["/app", "/app#inbox", "/app#inbox/43", "/app#inbox/44", "/app#inbox", "/app#section", "/app#x", "/app#section", "/app/view/8", "/app/view/8#detail"]);
    assert_eq!(
        ui.eval("events.join('|')"),
        "hashchange  > #inbox|hashchange #inbox > #inbox/42|hashchange #inbox/42 > #inbox/43|hashchange #inbox/43 > #inbox/44|hashchange #inbox/44 > #inbox|hashchange #inbox > #section|hashchange #section > #x|hashchange #x > #section"
    );

    // Back/forward through the document's entries: popstate (with the
    // entry's state) and hashchange, never a load.
    ui.eval("events.length = 0");
    // The page goes back itself; the UI follows.
    ui.h.send(ToContent::Eval("history.back()".into()));
    match ui.until("history.back()", |m| matches!(m, FromContent::HistoryTraversed { .. })) {
        FromContent::HistoryTraversed { delta, url: u } => {
            assert_eq!((delta, u.path.as_str(), u.fragment.as_deref()), (-1, "/app/view/8", None));
            ui.tab.follow_traversal(delta as isize, u);
        }
        _ => unreachable!(),
    }
    assert_eq!(ui.tab.index, 8);
    assert_eq!(ui.address(), format!("{base}/app/view/8"));
    // The back button.
    let wait_events = |ui: &mut Ui, n: usize| {
        let deadline = Instant::now() + Duration::from_secs(20);
        while ui.eval("String(events.length)") != n.to_string() {
            assert!(Instant::now() < deadline, "traversal events: {}", ui.eval("events.join('|')"));
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    wait_events(&mut ui, 2);
    for (i, delta) in [-1, -3, 2].into_iter().enumerate() {
        ui.go(delta);
        wait_events(&mut ui, 4 + 2 * i);
    }
    assert_eq!(
        ui.eval("events.join('|')"),
        "popstate /app/view/8 2|hashchange #detail > |popstate /app#section null|hashchange  > #section|popstate /app#inbox null|hashchange #section > #inbox|popstate /app#x null|hashchange #inbox > #x"
    );
    assert_eq!(ui.address(), format!("{base}/app#x"));
    assert_eq!(ui.eval("location.pathname + location.hash + ' ' + window.marker + ' ' + navigation.currentEntry.url.split('/').pop()"), "/app#x first-document app#x");
    // Going back past the document's first entry is the UI's business.
    ui.h.send(ToContent::Eval("history.go(-50)".into()));
    match ui.until("history.go(-50)", |m| matches!(m, FromContent::HistoryGo(_))) {
        FromContent::HistoryGo(d) => assert_eq!(d, -50),
        _ => unreachable!(),
    }

    assert_eq!(ui.commits, 1, "no document was loaded again");
    assert_eq!(hits.load(Ordering::SeqCst), 1, "the page was fetched once");

    // A real reload still reloads.
    ui.h.send(ToContent::Eval("location.reload()".into()));
    ui.until("reload", |m| matches!(m, FromContent::Committed { .. }));
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}
