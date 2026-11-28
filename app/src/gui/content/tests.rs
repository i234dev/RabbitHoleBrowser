//! Headless tests of the content-thread protocol: the test plays the UI
//! thread, sending messages and checking what comes back (and that it
//! never has to wait for a busy page).

use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rabbithole_engine::gfx::Color;
use rabbithole_engine::net::{Request, Url};

mod consent;
mod editing;
mod feeds;
mod focus;
mod font_scopes;
mod popups;
mod selection;
mod service_workers;
mod sso;
mod scroll_events;
mod spa;

/// Messages received by the fake UI: (when, tab id, message).
type Received = Receiver<(Instant, u64, FromContent)>;

fn sink() -> (Sink, Received) {
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let sink: Sink = Arc::new(move |tab, _epoch, msg| {
        let _ = tx.lock().unwrap().send((Instant::now(), tab, msg));
    });
    (sink, rx)
}

fn config(view: ViewSize) -> ContentConfig {
    let dir = std::env::temp_dir().join(format!("rh-content-test-{}", std::process::id()));
    ContentConfig {
        window_context: None,
        loader: rabbithole_engine::Loader::new(),
        settings: ContentSettings { download_dir: dir, ..ContentSettings::new(true, true, false) },
        local_storage: Arc::new(Mutex::new(rabbithole_engine::LocalStorage::default())),
        next_download_id: Arc::new(AtomicU64::new(1)),
        view,
        system_fonts: false,
        console_to_stderr: false,
    }
}

fn spawn(tab: u64, sink: &Sink) -> ContentHandle {
    let view = ViewSize::new(800, 600, 1.0);
    let h = ContentHandle::spawn(tab, 1, config(view), sink.clone());
    h.send(ToContent::SetActive(true));
    h
}

fn url(s: &str) -> Url {
    Url::parse(s).unwrap()
}

fn load(h: &ContentHandle, html: &str) {
    h.send(ToContent::LoadHtml { html: html.to_string(), url: url("https://example.test/"), push_history: true });
}

/// Wait for a message of `tab` matching `pred`, returning it.
fn wait_for(rx: &Received, tab: u64, timeout: Duration, mut pred: impl FnMut(&FromContent) -> bool) -> Option<(Instant, FromContent)> {
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok((t, id, msg)) if id == tab && pred(&msg) => return Some((t, msg)),
            Ok((_, id, msg)) if std::env::var_os("RH_TEST_TRACE").is_some() => eprintln!("[skip] tab {id}: {msg:?}"),
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

fn is_frame(m: &FromContent) -> bool {
    matches!(m, FromContent::Frame(_))
}

const TALL: &str = "<html><body style='margin:0'><div style='height:5000px;background:linear-gradient(red,blue)'>tall</div></body></html>";

/// A Rust panic on a content thread is reported as `Crashed` (instead of
/// the tab silently freezing or the process going down), and other tabs
/// keep working.
#[test]
fn content_thread_panic_reports_crash() {
    let (sink, rx) = sink();
    let h = spawn(41, &sink);
    let other = spawn(42, &sink);
    load(&h, "<p>hello</p>");
    assert!(wait_for(&rx, 41, Duration::from_secs(20), is_frame).is_some(), "first frame");
    h.send(ToContent::Crash);
    let crashed = wait_for(&rx, 41, Duration::from_secs(20), |m| matches!(m, FromContent::Crashed(_)));
    match crashed {
        Some((_, FromContent::Crashed(reason))) => assert!(reason.contains("deliberate content thread crash") && reason.contains("worker.rs"), "{reason}"),
        _ => panic!("no Crashed message"),
    }
    load(&other, "<p>still alive</p>");
    assert!(wait_for(&rx, 42, Duration::from_secs(20), is_frame).is_some(), "other tab keeps painting");
}

#[test]
fn first_frame_and_commit_for_html() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, "<html><head><title>Hi</title></head><body><p>hello</p></body></html>");
    let (_, c) = wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::Committed { .. })).expect("commit");
    match c {
        FromContent::Committed { url, title, push_history } => {
            assert_eq!(url.host, "example.test");
            assert_eq!(title, "Hi");
            assert!(push_history);
        }
        _ => unreachable!(),
    }
    let (_, f) = wait_for(&rx, 1, Duration::from_secs(10), is_frame).expect("frame");
    let FromContent::Frame(f) = f else { unreachable!() };
    assert_eq!(f.canvas.width, 800);
    assert!(f.canvas.height >= 600);
    assert_eq!(f.canvas.data.len(), (f.canvas.width * f.canvas.height) as usize);
}

#[test]
fn frames_are_overscanned_for_ui_scrolling() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, TALL);
    let (_, f) = wait_for(&rx, 1, Duration::from_secs(10), is_frame).expect("frame");
    let FromContent::Frame(f) = f else { unreachable!() };
    // Painted below the viewport so the UI can scroll before a repaint.
    assert!(f.covered().bottom() >= 600.0 + 250.0, "{:?}", f.covered());
    assert!((f.content.1 - 5000.0).abs() < 1.0);
    // The UI scrolls a little: the frame still covers it (no repaint
    // needed); scrolling far away gets a new frame around the new spot.
    h.send(ToContent::Scroll { x: 0.0, y: 3000.0 });
    let (_, f2) = wait_for(&rx, 1, Duration::from_secs(10), is_frame).expect("repaint after a long scroll");
    let FromContent::Frame(f2) = f2 else { unreachable!() };
    let c = f2.covered();
    assert!(c.y <= 3000.0 && c.bottom() >= 3600.0, "{c:?}");
    assert_eq!(f2.scroll, (0.0, 3000.0));
}

/// The acceptance test for the threading: while one tab runs a script
/// that blocks its thread (`while (Date.now() < t) {}`), the UI never
/// waits, another tab keeps answering with frames, and the busy tab is
/// reported as busy (for the "not responding" bar).
#[test]
fn frames_keep_arriving_while_a_slow_script_runs() {
    let (sink, rx) = sink();
    let slow = spawn(1, &sink);
    let other = spawn(2, &sink);
    load(&other, TALL);
    wait_for(&rx, 2, Duration::from_secs(10), is_frame).expect("other tab first frame");
    load(&slow, "<html><body><p>slow</p><script>var t = Date.now() + 1500; while (Date.now() < t) {}</script></body></html>");

    // Wait until the slow tab is inside its script.
    let start = Instant::now();
    while slow.busy_for().map(|d| d < Duration::from_millis(200)).unwrap_or(true) {
        assert!(start.elapsed() < Duration::from_secs(10), "slow tab never got busy");
        std::thread::sleep(Duration::from_millis(5));
    }
    let busy_since = slow.busy_since();
    // Meanwhile: scroll the other tab several times; every scroll far
    // enough away is answered with a frame quickly.
    let mut answered = 0;
    let mut worst = Duration::ZERO;
    for i in 1..=4 {
        let y = (i * 1100) as f32;
        let sent = Instant::now();
        other.send(ToContent::Scroll { x: 0.0, y });
        let (t, _) = wait_for(&rx, 2, Duration::from_secs(2), |m| matches!(m, FromContent::Frame(f) if f.scroll.1 == y)).expect("frame from the other tab while the slow script runs");
        worst = worst.max(t.duration_since(sent));
        answered += 1;
        // Sending never blocked the "UI" thread.
        assert!(sent.elapsed() < Duration::from_secs(2));
    }
    assert_eq!(answered, 4);
    assert!(worst < Duration::from_millis(1000), "other tab answered in {worst:?}");
    // The slow tab is still in the same task: hang detection sees it.
    assert_eq!(slow.busy_since(), busy_since, "slow tab finished too early for the test to mean anything");
    assert!(slow.busy_for().unwrap() >= Duration::from_millis(200));

    // Once the script ends, the slow tab paints and answers again.
    wait_for(&rx, 1, Duration::from_secs(10), is_frame).expect("slow tab frame after its script");
    slow.send(ToContent::Eval("1 + 1".into()));
    let (_, r) = wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::EvalResult(_))).expect("eval result");
    let FromContent::EvalResult(r) = r else { unreachable!() };
    assert_eq!(r.trim(), "2");
    assert!(slow.busy_for().map(|d| d < Duration::from_secs(1)).unwrap_or(true));
}

#[test]
fn clicking_a_link_asks_the_ui_to_navigate() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, "<html><body style='margin:0'><a href='/next' style='display:block;height:50px'>next</a></body></html>");
    wait_for(&rx, 1, Duration::from_secs(10), is_frame).expect("frame");
    let mods = rabbithole_engine::Modifiers::default();
    h.send(ToContent::PointerMove { x: 10.0, y: 10.0, mods });
    let (_, hover) = wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::Hover { .. })).expect("hover");
    match hover {
        FromContent::Hover { cursor, link } => {
            assert_eq!(cursor, "pointer");
            assert_eq!(link.map(|u| u.path), Some("/next".to_string()));
        }
        _ => unreachable!(),
    }
    h.send(ToContent::PointerDown { x: 10.0, y: 10.0, button: 0, mods, click_count: 1 });
    h.send(ToContent::PointerUp { x: 10.0, y: 10.0, button: 0, mods, inside: true });
    let (_, nav) = wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::Navigate { .. })).expect("navigation request");
    match nav {
        FromContent::Navigate { request, push_history } => {
            assert_eq!(request.url.to_string(), "https://example.test/next");
            assert!(push_history);
        }
        _ => unreachable!(),
    }
}

#[test]
fn typing_into_a_text_field() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, "<html><body style='margin:0'><input id=q style='width:200px;height:30px'><script>var n = 0; document.getElementById('q').addEventListener('input', function () { n++; });</script></body></html>");
    wait_for(&rx, 1, Duration::from_secs(10), is_frame).expect("frame");
    let mods = rabbithole_engine::Modifiers::default();
    h.send(ToContent::PointerDown { x: 20.0, y: 10.0, button: 0, mods, click_count: 1 });
    h.send(ToContent::PointerUp { x: 20.0, y: 10.0, button: 0, mods, inside: true });
    for c in ["r", "h"] {
        h.send(ToContent::Key(KeyInput { key: c.into(), code: format!("Key{}", c.to_uppercase()), text: Some(c.into()), pressed: true, ..Default::default() }));
    }
    let (_, st) = wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::State(s) if s.editing)).expect("editing state");
    let _ = st;
    h.send(ToContent::Eval("document.getElementById('q').value + ':' + n".into()));
    let (_, r) = wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::EvalResult(_))).expect("eval");
    let FromContent::EvalResult(r) = r else { unreachable!() };
    assert_eq!(r.trim(), "'rh:2'");
}

/// Dragging over text starts a selection, and the page still gets the
/// pointer moves (drag-to-pan widgets): they used to stop at the drag
/// threshold. A `wheel` the page cancels doesn't scroll it, and the page
/// state tells the UI to route the wheel through the page.
#[test]
fn drags_report_moves_and_wheels_reach_the_page() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(
        &h,
        "<html><body style='margin:0;height:3000px'><p id=t style='margin:0;font-size:40px'>drag over this text</p><script>\
         var moves = 0, wheels = []; var t = document.getElementById('t');\
         t.addEventListener('pointermove', function (e) { if (e.buttons) moves++; });\
         t.addEventListener('wheel', function (e) { wheels.push(e.deltaY); e.preventDefault(); }, { passive: false });\
         </script></body></html>",
    );
    // (The state goes out with the first frame, before it.)
    let (_, st) = wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::State(s) if s.wheel_blocking)).expect("wheel state");
    let FromContent::State(st) = st else { unreachable!() };
    assert!(st.wheel_listeners && st.wheel_blocking);
    let mods = rabbithole_engine::Modifiers::default();
    h.send(ToContent::PointerDown { x: 10.0, y: 20.0, button: 0, mods, click_count: 1 });
    for i in 1..=5 {
        h.send(ToContent::PointerMove { x: 10.0 + 30.0 * i as f32, y: 20.0, mods });
        // One move at a time (queued moves are coalesced).
        h.send(ToContent::Eval("0".into()));
        wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::EvalResult(_))).expect("eval");
    }
    h.send(ToContent::PointerUp { x: 160.0, y: 20.0, button: 0, mods, inside: true });
    h.send(ToContent::Wheel { x: 20.0, y: 20.0, dx: 0.0, dy: 120.0, mods, scrolled: false });
    h.send(ToContent::Eval("moves + ':' + wheels.join(',')".into()));
    let (_, r) = wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::EvalResult(_))).expect("eval");
    let FromContent::EvalResult(r) = r else { unreachable!() };
    assert_eq!(r.trim(), "'5:120'");
}

#[test]
fn page_scrolls_are_reported_to_the_ui() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, TALL);
    wait_for(&rx, 1, Duration::from_secs(10), is_frame).expect("frame");
    h.send(ToContent::Eval("window.scrollTo(0, 1234)".into()));
    let (_, m) = wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::ScrollTo { .. })).expect("scroll request");
    match m {
        FromContent::ScrollTo { y, .. } => assert_eq!(y, 1234.0),
        _ => unreachable!(),
    }
    // Find in page scrolls to and highlights the match.
    load(&h, "<html><body style='margin:0'><div style='height:3000px'></div><p>needle</p></body></html>");
    wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::Committed { .. })).expect("commit");
    h.send(ToContent::Find("needle".into()));
    let (_, m) = wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::FindResult { .. })).expect("find result");
    assert!(matches!(m, FromContent::FindResult { count: 1, current: 0 }), "{m:?}");
    let (_, m) = wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::ScrollTo { .. })).expect("scroll to match");
    assert!(matches!(m, FromContent::ScrollTo { y, .. } if y > 2000.0), "{m:?}");
}

/// A tiny HTTP server: `/` answers at once and references `/slow.js`,
/// which takes `delay` to arrive; `/hang` never answers in time.
fn serve(delay: Duration) -> String {
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
                let (ct, body) = if path.starts_with("/slow.js") {
                    std::thread::sleep(delay);
                    ("application/javascript", "document.title = 'scripted';".to_string())
                } else if path.starts_with("/hang") {
                    std::thread::sleep(Duration::from_secs(5));
                    ("text/html", "<p>late</p>".to_string())
                } else {
                    ("text/html", "<html><head><title>plain</title><script src='/slow.js'></script></head><body><p>content before scripts</p></body></html>".to_string())
                };
                let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = s.write_all(resp.as_bytes());
            });
        }
    });
    format!("http://{addr}")
}

#[test]
fn page_is_painted_before_slow_scripts_arrive() {
    let base = serve(Duration::from_millis(3000));
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    let start = Instant::now();
    h.send(ToContent::Navigate { request: Request::get(url(&format!("{base}/"))), push_history: true });
    let (committed, _) = wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::Committed { .. })).expect("commit");
    let (painted, _) = wait_for(&rx, 1, Duration::from_secs(10), is_frame).expect("first paint");
    let (titled, t) = wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::Title(_))).expect("title set by the script");
    assert!(matches!(t, FromContent::Title(ref s) if s == "scripted"), "{t:?}");
    assert!(committed.duration_since(start) < Duration::from_millis(2500), "commit took {:?}", committed.duration_since(start));
    assert!(painted.duration_since(start) < Duration::from_millis(2800), "first paint took {:?}", painted.duration_since(start));
    assert!(titled.duration_since(start) >= Duration::from_millis(2900), "script ran before it arrived?");
    assert!(painted < titled);
    wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::Loading(None))).expect("load finished");
}

#[test]
fn stop_cancels_a_pending_load() {
    let base = serve(Duration::from_millis(10));
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, "<p>old</p>");
    wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::Committed { .. })).expect("commit");
    h.send(ToContent::Navigate { request: Request::get(url(&format!("{base}/hang"))), push_history: true });
    std::thread::sleep(Duration::from_millis(100));
    h.send(ToContent::Stop);
    wait_for(&rx, 1, Duration::from_secs(5), |m| matches!(m, FromContent::Loading(None))).expect("stopped");
    // The late response is ignored.
    assert!(wait_for(&rx, 1, Duration::from_secs(6), |m| matches!(m, FromContent::Committed { .. })).is_none());
}

#[test]
fn coalescing_keeps_order_of_different_messages() {
    let m = |y: f32| Inbox::Ui(ToContent::Scroll { x: 0.0, y });
    let mods = rabbithole_engine::Modifiers::default();
    let batch = vec![m(1.0), m(2.0), Inbox::Ui(ToContent::PointerDown { x: 0.0, y: 0.0, button: 0, mods, click_count: 1 }), m(3.0), m(4.0)];
    let out = worker_coalesce(batch);
    let ys: Vec<String> = out
        .iter()
        .map(|i| match i {
            Inbox::Ui(ToContent::Scroll { y, .. }) => format!("s{y}"),
            Inbox::Ui(ToContent::PointerDown { .. }) => "down".into(),
            _ => "?".into(),
        })
        .collect();
    assert_eq!(ys, vec!["s2", "down", "s4"]);
}

fn worker_coalesce(batch: Vec<Inbox>) -> Vec<Inbox> {
    super::worker::coalesce_for_tests(batch)
}

#[test]
fn handle_drop_stops_the_thread() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, "<p>x</p>");
    wait_for(&rx, 1, Duration::from_secs(10), is_frame).expect("frame");
    drop(h);
    drop(sink);
    // The thread drops its sink clone when it exits: the channel closes.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Err(RecvTimeoutError::Disconnected) => break,
            _ => assert!(Instant::now() < deadline, "content thread did not exit"),
        }
    }
}

// ------------------------------------------------------- incremental paint

/// A tall page of distinct lines (no fixed content), so scrolls and
/// style changes take the incremental paths.
fn lines_page(color: &str) -> String {
    let mut body = String::from("<html><body style='margin:0;font:16px sans-serif'>");
    body.push_str(&format!("<div id='a' style='color:{color};background:#eef;height:40px'>changing block</div>"));
    for i in 0..200 {
        body.push_str(&format!("<p style='margin:0;height:30px;background:rgb({},{},200)'>line {i} of the incremental paint test</p>", (i * 7) % 255, (i * 13) % 255));
    }
    body.push_str("</body></html>");
    body
}

/// A full, from-scratch paint of `html` with the frame's geometry.
fn reference_paint(html: &str, f: &Frame) -> Vec<u32> {
    let mut engine = rabbithole_engine::Engine::new_bundled_fonts_only();
    let mut page = engine.load_html(html, url("https://example.test/"), rabbithole_engine::Viewport { width: 800.0, height: 600.0 });
    engine.flush_display_list(&mut page);
    let mut c = rabbithole_engine::gfx::Canvas::new(f.canvas.width, f.canvas.height);
    c.clear(rabbithole_engine::gfx::Color::WHITE);
    rabbithole_engine::layout::paint_with_fixed_origin(&page.display_list, &mut c, &page.fonts, f.origin.0, f.origin.1, (f.scroll.0 - f.origin.0, f.scroll.1 - f.origin.1));
    c.data
}

fn mismatches(a: &[u32], b: &[u32]) -> usize {
    a.iter().zip(b).filter(|(x, y)| x != y).count()
}

#[test]
fn incremental_scroll_and_damage_match_full_paints() {
    use std::sync::atomic::Ordering;
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    let html = lines_page("red");
    load(&h, &html);
    wait_for(&rx, 1, Duration::from_secs(10), is_frame).expect("first frame");
    let before = super::worker::PARTIAL_PAINTS.load(Ordering::Relaxed);
    // Scroll past the overscan: the old pixels shift, only the strip is painted.
    for y in [700.0, 1400.0, 1000.0] {
        h.send(ToContent::Scroll { x: 0.0, y });
        let (_, f) = wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::Frame(f) if f.scroll.1 == y)).expect("frame after scroll");
        let FromContent::Frame(f) = f else { unreachable!() };
        let reference = reference_paint(&html, &f);
        let bad = mismatches(&f.canvas.data, &reference);
        assert!(bad * 1000 < reference.len(), "scroll to {y}: {bad} of {} pixels differ", reference.len());
    }
    assert!(super::worker::PARTIAL_PAINTS.load(Ordering::Relaxed) > before, "scrolling used partial paints");
    // A style change repaints only its damage.
    h.send(ToContent::Scroll { x: 0.0, y: 0.0 });
    wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::Frame(f) if f.scroll.1 == 0.0)).expect("back at the top");
    let before = super::worker::PARTIAL_PAINTS.load(Ordering::Relaxed);
    h.send(ToContent::Eval("document.getElementById('a').style.color = 'blue'".into()));
    let (_, f) = wait_for(&rx, 1, Duration::from_secs(10), is_frame).expect("frame after the style change");
    let FromContent::Frame(f) = f else { unreachable!() };
    let reference = reference_paint(&lines_page("blue"), &f);
    let bad = mismatches(&f.canvas.data, &reference);
    assert!(bad * 1000 < reference.len(), "after style change: {bad} pixels differ");
    assert!(super::worker::PARTIAL_PAINTS.load(Ordering::Relaxed) > before, "the style change used a partial paint");
}

#[test]
fn shift_rows_moves_content() {
    let mut c = rabbithole_engine::gfx::Canvas::new(2, 4);
    for (i, p) in c.data.iter_mut().enumerate() {
        *p = (i / 2) as u32;
    }
    super::worker::shift_rows_for_tests(&mut c, 1);
    assert_eq!(c.data, vec![1, 1, 2, 2, 3, 3, 3, 3]);
    super::worker::shift_rows_for_tests(&mut c, -2);
    assert_eq!(&c.data[4..], &[1, 1, 2, 2]);
}

#[test]
fn context_menu_reports_images() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, "<html><body style='margin:0'><a href='/x'><img src='/pic.png' width=100 height=100 style='display:block'></a></body></html>");
    wait_for(&rx, 1, Duration::from_secs(10), is_frame).expect("frame");
    h.send(ToContent::PointerDown { x: 50.0, y: 50.0, button: 2, mods: Default::default(), click_count: 1 });
    let (_, m) = wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::ContextMenu { .. })).expect("context menu");
    let FromContent::ContextMenu { link, image, .. } = m else { unreachable!() };
    assert_eq!(link.map(|u| u.path), Some("/x".to_string()));
    assert_eq!(image.map(|u| u.to_string()), Some("https://example.test/pic.png".to_string()));
}

#[test]
fn inspector_tree_and_styles() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, "<html><body style='margin:0'><div id='box' class='big' style='color:rgb(1, 2, 3);width:120px;height:50px'>hello</div></body></html>");
    wait_for(&rx, 1, Duration::from_secs(10), is_frame).expect("frame");
    h.send(ToContent::InspectAt { x: 20.0, y: 12.0 });
    let (_, t) = wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::DomTree(_))).expect("tree");
    let FromContent::DomTree(rows) = t else { unreachable!() };
    assert!(rows.iter().any(|r| r.label.starts_with("<div id=\"box\"")), "{rows:?}");
    assert!(rows.iter().any(|r| r.is_text && r.label.contains("hello")));
    let (_, d) = wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::NodeDetails(_))).expect("details");
    let FromContent::NodeDetails(d) = d else { unreachable!() };
    let get = |k: &str| d.styles.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    assert_eq!(get("color").as_deref(), Some("rgb(1, 2, 3)"), "{:?}", d.styles);
    assert_eq!(get("display").as_deref(), Some("block"));
    assert!(d.attrs.iter().any(|(k, v)| k == "class" && v == "big"));
}

#[test]
fn print_to_pdf_writes_pages() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, TALL);
    wait_for(&rx, 1, Duration::from_secs(10), is_frame).expect("frame");
    let path = std::env::temp_dir().join(format!("rh-print-{}.pdf", std::process::id()));
    h.send(ToContent::PrintPdf(path.clone()));
    let (_, m) = wait_for(&rx, 1, Duration::from_secs(60), |m| matches!(m, FromContent::Status(s) if s.contains("PDF"))).expect("status");
    let FromContent::Status(msg) = m else { unreachable!() };
    assert!(msg.starts_with("Saved PDF"), "{msg}");
    let data = std::fs::read(&path).unwrap();
    assert!(data.starts_with(b"%PDF-1.4"));
    // 5000 px tall at 800 px wide → 5 A4 pages (800 × 1131).
    assert!(String::from_utf8_lossy(&data).contains("/Count 5"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn fixed_header_stays_in_place_before_the_scroll_repaint() {
    use rabbithole_engine::gfx::Canvas;
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    let html = "<style>*{margin:0}header{position:fixed;top:0;left:0;width:100%;height:32px;background:rgba(255,0,0,.5)}footer{position:fixed;bottom:0;width:100%;height:15px;background:green}</style><div style='height:75px;background:white'></div><div style='height:2000px;background:black'></div><header></header><footer></footer>";
    load(&h, html);
    let (_, FromContent::Frame(first)) = wait_for(&rx, 1, Duration::from_secs(10), is_frame).unwrap() else { unreachable!() };
    assert_eq!(motions(&first), ["fixed"], "fixed header and footer paint last");

    let area = Rect::new(2.0, 3.0, 800.0, 600.0);
    let present = |f: &Frame, y| {
        let mut dst = Canvas::new(804, 606);
        f.present(&mut dst, area, (0.0, y), 1.0);
        dst.data
    };
    let reference = reference_paint(html, &first);
    let normal = present(&first, 0.0);
    for y in 0..600 {
        assert_eq!(&normal[(3 + y) * 804 + 2..(3 + y) * 804 + 802], &reference[y * 800..(y + 1) * 800]);
    }
    let speculative = present(&first, 90.0);
    h.send(ToContent::Scroll { x: 0.0, y: 90.0 });
    let (_, FromContent::Frame(next)) = wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::Frame(f) if f.scroll.1 == 90.0)).unwrap() else { unreachable!() };

    assert_eq!(mismatches(&speculative, &present(&next, 90.0)), 0);
    // Scroll up on the same frame to expose pixels previously covered by
    // the header and move a white background under its translucent fill.
    assert_eq!(mismatches(&present(&next, 0.0), &present(&first, 0.0)), 0);
}

/// The layers of a frame, bottom to top.
fn motions(f: &Frame) -> Vec<&'static str> {
    use rabbithole_engine::layout::paint::LayerMotion;
    f.layers.iter().map(|l| match l.motion {
        LayerMotion::Fixed => "fixed",
        LayerMotion::Scroll => "scroll",
        LayerMotion::Sticky { .. } => "sticky",
    }).collect()
}

/// What the UI shows for `f` at page scroll `y` in a `w`×`h` page area.
fn shown(f: &Frame, y: f32, w: u32, h: u32, scale: f32) -> Vec<u32> {
    let mut dst = rabbithole_engine::gfx::Canvas::new(w, h);
    f.present(&mut dst, Rect::new(0.0, 0.0, w as f32, h as f32), (0.0, y), scale);
    dst.data
}

fn frame_at(rx: &Received, y: f32) -> Frame {
    match wait_for(rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::Frame(f) if f.scroll.1 == y)) {
        Some((_, FromContent::Frame(f))) => f,
        _ => panic!("no frame at scroll {y}"),
    }
}

#[test]
fn fixed_header_below_a_scrolling_box_stays_in_paint_order() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    let html = "<body style='margin:0;height:2000px'><header style='position:fixed;top:0;width:100%;height:60px;background:red'></header><div style='position:relative;z-index:2;width:40px;height:60px;background:blue'></div>";
    load(&h, html);
    let (_, FromContent::Frame(f)) = wait_for(&rx, 1, Duration::from_secs(10), is_frame).unwrap() else { unreachable!() };
    // The box painted above the header is a scrolling layer above it.
    assert_eq!(motions(&f), ["fixed", "scroll"]);
    assert_eq!(shown(&f, 0.0, 800, 600, 1.0), reference_paint(html, &f)[..800 * 600]);
    let speculative = shown(&f, 30.0, 800, 600, 1.0);
    h.send(ToContent::Scroll { x: 0.0, y: 30.0 });
    let next = frame_at(&rx, 30.0);
    assert_eq!(mismatches(&speculative, &shown(&next, 30.0, 800, 600, 1.0)), 0);
    let (blue, red) = (Color::rgb(0, 0, 255).to_u32(), Color::rgb(255, 0, 0).to_u32());
    assert_eq!((speculative[10 * 800 + 10], speculative[10 * 800 + 100], speculative[40 * 800 + 10]), (blue, red, red));
}

#[test]
fn fixed_groups_around_scrolling_content_stay_in_place() {
    let (sink, rx) = sink();
    let h = ContentHandle::spawn(1, 1, config(ViewSize::new(400, 300, 1.25)), sink.clone());
    h.send(ToContent::SetActive(true));
    // Header, a box above it, footer, and a tall box above the footer.
    let html = "<style>*{margin:0}main{height:2500px;background:linear-gradient(black,white)}\
        header{position:fixed;top:0;left:0;width:100%;height:20px;background:#781753;color:white;z-index:1}\
        #mid{position:relative;z-index:2;top:40px;width:50px;height:30px;background:#36c}\
        footer{position:fixed;bottom:0;left:0;width:100%;height:20px;background:#285539;z-index:3}\
        #top{position:relative;z-index:4;top:40px;width:30px;height:900px;background:#c63}</style>\
        <header>Top</header><main><div id=mid></div><div id=top></div></main><footer>Bottom</footer>";
    load(&h, html);
    let (_, FromContent::Frame(first)) = wait_for(&rx, 1, Duration::from_secs(10), is_frame).unwrap() else { unreachable!() };
    assert_eq!(motions(&first), ["fixed", "scroll", "fixed", "scroll"]);
    // Header and footer rows to the right of the scrolling boxes.
    let bars = |px: &[u32]| -> Vec<u32> {
        (0..25).chain(275..300).flat_map(|y| px[y * 400 + 70..y * 400 + 400].to_vec()).collect()
    };
    let expected = bars(&shown(&first, 0.0, 400, 300, 1.25));
    let mut prev = first;
    for y in [0.25, 3.75, 37.25, 50.5, 81.75, 160.5, 120.25, 30.75, 0.0] {
        let speculative = shown(&prev, y, 400, 300, 1.25);
        assert!(bars(&speculative) == expected, "anticipating scroll {y}");
        h.send(ToContent::Scroll { x: 0.0, y });
        let next = frame_at(&rx, y);
        let repainted = shown(&next, y, 400, 300, 1.25);
        assert_eq!(mismatches(&speculative, &repainted), 0, "scroll {y}");
        // The tall box stays above the footer.
        assert_eq!(repainted[290 * 400 + 10], Color::rgb(0xcc, 0x66, 0x33).to_u32(), "scroll {y}");
        prev = next;
    }
}

#[test]
fn sticky_header_stays_in_place_until_its_container_ends() {
    let (sink, rx) = sink();
    let h = ContentHandle::spawn(1, 1, config(ViewSize::new(400, 300, 1.25)), sink.clone());
    h.send(ToContent::SetActive(true));
    // The heading sticks from scroll 50 to 630, then leaves with its section.
    let html = "<style>*{margin:0}#before{height:50px;background:#ddd}section{height:600px;background:linear-gradient(#123,#9ab)}\
        h2{position:sticky;top:0;height:20px;background:rgb(200,30,30);color:white;font-size:14px}#after{height:2000px;background:#eee}</style>\
        <div id=before></div><section><h2>Sticky</h2></section><div id=after></div>";
    load(&h, html);
    let (_, FromContent::Frame(first)) = wait_for(&rx, 1, Duration::from_secs(10), is_frame).unwrap() else { unreachable!() };
    assert_eq!(motions(&first), ["sticky"]);
    let stuck = |y: f32| (50.0..=630.0).contains(&y);
    let heading = Color::rgb(200, 30, 30).to_u32();
    // First fully covered row of the heading (right of its text).
    let top = |px: &[u32]| (0..300).find(|&r| px[r * 400 + 390] == heading);
    let mut stuck_band: Option<Vec<u32>> = None;
    let mut prev = first;
    for y in [10.4, 49.6, 50.0, 70.25, 160.5, 250.75, 340.25, 430.5, 520.75, 610.25, 629.6, 640.25, 700.5, 640.25, 610.25, 520.75, 430.5, 340.25, 250.75, 160.5, 70.25, 30.25, 0.0] {
        let speculative = shown(&prev, y, 400, 300, 1.25);
        h.send(ToContent::Scroll { x: 0.0, y });
        let next = frame_at(&rx, y);
        assert_eq!(motions(&next), ["sticky"]);
        let repainted = shown(&next, y, 400, 300, 1.25);
        if stuck(y) {
            assert_eq!(top(&repainted), Some(0), "scroll {y}");
            let band = repainted[..25 * 400].to_vec();
            let expected = stuck_band.get_or_insert_with(|| band.clone());
            assert!(band == *expected, "stuck heading moved at scroll {y}");
            if stuck(prev.scroll.1) {
                assert!(speculative[..25 * 400] == expected[..], "stuck heading moved before the repaint at {y}");
            }
        }
        if stuck(y) == stuck(prev.scroll.1) {
            assert_eq!(mismatches(&speculative, &repainted), 0, "scroll {prev:?} to {y}");
        } else {
            // Sticking or unsticking between two frames: within a device pixel.
            let (a, b) = (top(&speculative).unwrap() as i32, top(&repainted).unwrap() as i32);
            assert!((a - b).abs() <= 1, "scroll {prev:?} to {y}: {a} {b}");
        }
        prev = next;
    }
    // Past the end of its section the heading scrolled away with it.
    let end = shown(&prev, 700.5, 400, 300, 1.25);
    assert_eq!(top(&end), None);
}

#[test]
fn focus_ring_is_painted_above_the_fixed_layer() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    let html = "<style>*{margin:0}header{position:fixed;top:0;width:100%;height:20px;background:#781753}main{height:3000px}</style><header></header><main><button id=b style='margin-top:100px'>Go</button></main>";
    load(&h, html);
    let (_, FromContent::Frame(first)) = wait_for(&rx, 1, Duration::from_secs(10), is_frame).unwrap() else { unreachable!() };
    assert_eq!(motions(&first), ["fixed"]);
    let band = |px: &[u32]| px[..20 * 800].to_vec();
    let expected = band(&shown(&first, 0.0, 800, 600, 1.0));
    h.send(ToContent::Eval("document.getElementById('b').focus()".into()));
    let focused = match wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::Frame(f) if f.layers.len() == 2)) {
        Some((_, FromContent::Frame(f))) => f,
        _ => panic!("no frame with the focus ring"),
    };
    // The ring is a scrolling overlay: the header keeps its own layer.
    assert_eq!(motions(&focused), ["fixed", "scroll"]);
    let ring = shown(&focused, 0.0, 800, 600, 1.0);
    assert_ne!(ring, shown(&first, 0.0, 800, 600, 1.0));
    assert!(band(&shown(&focused, 37.5, 800, 600, 1.0)) == expected);
    // A removed element loses focus, and its ring.
    h.send(ToContent::Eval("document.getElementById('b').remove()".into()));
    assert!(wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::Frame(f) if motions(f) == ["fixed"])).is_some());
}

/// A page with a fixed header under a fixed consent overlay that blurs
/// the page behind it.
const BACKDROP_PAGE: &str = "<style>*{margin:0}main{height:3000px;background:repeating-linear-gradient(#fff 0 13px,#36c 13px 17px)}\
    header{position:fixed;top:0;left:0;width:100%;height:40px;background:#e35614;color:white;z-index:1001}\
    #cmp{position:fixed;inset:0;background:rgba(33,41,52,.75);backdrop-filter:blur(4px);z-index:2147483647;display:flex;align-items:center;justify-content:center}\
    #ui{width:200px;height:120px;background:white;color:black}</style>\
    <header>Header</header><main>Text</main><div id=cmp><div id=ui>We value your privacy</div></div>";

#[test]
fn fixed_backdrop_overlay_stays_in_place_before_the_scroll_repaint() {
    let (sink, rx) = sink();
    let h = ContentHandle::spawn(1, 1, config(ViewSize::new(400, 300, 1.25)), sink.clone());
    h.send(ToContent::SetActive(true));
    load(&h, BACKDROP_PAGE);
    let (_, FromContent::Frame(first)) = wait_for(&rx, 1, Duration::from_secs(10), is_frame).unwrap() else { unreachable!() };
    assert_eq!(motions(&first), ["fixed"]);
    // The dialog box, and the header rows the blur cannot reach from the
    // scrolling page below the header.
    let dialog = |px: &[u32]| -> Vec<u32> { (113..187).flat_map(|y| px[y * 400 + 75..y * 400 + 325].to_vec()).collect() };
    let header = |px: &[u32]| px[..32 * 400].to_vec();
    let reference = shown(&first, 0.0, 400, 300, 1.25);
    let mut prev = first;
    for y in [0.25, 3.75, 37.25, 50.5, 81.75, 160.5, 120.25, 30.75, 0.0] {
        let speculative = shown(&prev, y, 400, 300, 1.25);
        // The overlay covers the viewport: the snapshot does not move.
        assert!(speculative == shown(&prev, prev.scroll.1, 400, 300, 1.25), "anticipating scroll {y}");
        h.send(ToContent::Scroll { x: 0.0, y });
        let next = frame_at(&rx, y);
        let repainted = shown(&next, y, 400, 300, 1.25);
        assert!(dialog(&repainted) == dialog(&reference) && header(&repainted) == header(&reference), "scroll {y}");
        prev = next;
    }
}

#[test]
fn fixed_backdrop_overlay_matches_a_single_canvas_paint() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, BACKDROP_PAGE);
    let (_, FromContent::Frame(f)) = wait_for(&rx, 1, Duration::from_secs(10), is_frame).unwrap() else { unreachable!() };
    assert_eq!(motions(&f), ["fixed"]);
    // (Away from the bottom edge, where the single canvas has more page
    // below the viewport for the blur to read.)
    let rows = 560 * 800;
    assert_eq!(mismatches(&shown(&f, 0.0, 800, 600, 1.0)[..rows], &reference_paint(BACKDROP_PAGE, &f)[..rows]), 0);
}

#[test]
fn focus_ring_of_a_fixed_element_stays_with_it() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    let html = "<style>*{margin:0}header{position:fixed;top:0;width:100%;height:40px;background:#781753}main{height:3000px}</style><header><button id=b style='margin:10px 300px'>Go</button></header><main></main>";
    load(&h, html);
    wait_for(&rx, 1, Duration::from_secs(10), is_frame).unwrap();
    h.send(ToContent::Scroll { x: 0.0, y: 200.0 });
    let before = frame_at(&rx, 200.0);
    h.send(ToContent::Eval("document.getElementById('b').focus()".into()));
    let focused = match wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::Frame(f) if f.layers.len() == 2)) {
        Some((_, FromContent::Frame(f))) => f,
        _ => panic!("no frame with the focus ring"),
    };
    assert_eq!(motions(&focused), ["fixed", "fixed"]);
    let header = |px: &[u32]| px[..45 * 800].to_vec();
    let ring = header(&shown(&focused, 200.0, 800, 600, 1.0));
    // Drawn around the button in the header, and kept there while scrolling.
    assert!(mismatches(&ring, &header(&shown(&before, 200.0, 800, 600, 1.0))) > 0);
    assert!(header(&shown(&focused, 262.5, 800, 600, 1.0)) == ring);
}

#[test]
fn partial_fixed_backdrop_stays_in_place_and_lets_the_page_scroll_around_it() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    let page = BACKDROP_PAGE.replace("#cmp{position:fixed;inset:0;", "#cmp{position:fixed;left:200px;top:150px;width:400px;height:300px;");
    load(&h, &page);
    let (_, FromContent::Frame(first)) = wait_for(&rx, 1, Duration::from_secs(10), is_frame).unwrap() else { unreachable!() };
    assert_eq!(motions(&first), ["fixed"]);
    let rows = 560 * 800;
    assert_eq!(mismatches(&shown(&first, 0.0, 800, 600, 1.0)[..rows], &reference_paint(&page, &first)[..rows]), 0);
    let overlay = |px: &[u32]| -> Vec<u32> { (150..450).flat_map(|y| px[y * 800 + 200..y * 800 + 600].to_vec()).collect() };
    let speculative = shown(&first, 33.0, 800, 600, 1.0);
    // The overlay keeps its snapshot while the page beside it scrolls.
    assert!(overlay(&speculative) == overlay(&shown(&first, 0.0, 800, 600, 1.0)));
    assert!(speculative[..40 * 800] == shown(&first, 0.0, 800, 600, 1.0)[..40 * 800]);
    assert_eq!(speculative[100 * 800 + 10], shown(&first, 0.0, 800, 600, 1.0)[133 * 800 + 10]);
    h.send(ToContent::Scroll { x: 0.0, y: 33.0 });
    let next = frame_at(&rx, 33.0);
    let repainted = shown(&next, 33.0, 800, 600, 1.0);
    // Outside the overlay and the header, the page moved exactly.
    let outside = |px: &[u32]| -> Vec<u32> { (40..600).flat_map(|y| px[y * 800..y * 800 + 190].to_vec()).collect() };
    assert!(outside(&speculative) == outside(&repainted));
}

#[test]
fn fixed_bars_keep_their_pixels_through_every_scroll_frame() {
    use rabbithole_engine::gfx::Canvas;
    let (sink, rx) = sink();
    let view = ViewSize::new(400, 300, 1.25);
    let h = ContentHandle::spawn(1, 1, config(view), sink.clone());
    h.send(ToContent::SetActive(true));
    let html = "<style>*{margin:0}header,footer{position:fixed;left:0;width:100%;height:20px;color:white;background:#781753}header{top:0}footer{bottom:0;background:#285539}main{height:2500px;background:linear-gradient(black,white)}</style><main></main><header>Top</header><footer>Bottom</footer>";
    load(&h, html);
    let (_, FromContent::Frame(first)) = wait_for(&rx, 1, Duration::from_secs(10), is_frame).unwrap() else { unreachable!() };
    let area = Rect::new(0.0, 0.0, 400.0, 300.0);
    let bars = |f: &Frame, y| {
        assert_eq!(motions(f), ["fixed"]);
        let mut dst = Canvas::new(400, 300);
        f.present(&mut dst, area, (0.0, y), 1.25);
        [dst.data[..25 * 400].to_vec(), dst.data[275 * 400..].to_vec()]
    };
    let expected = bars(&first, 0.0);
    assert_ne!(expected[0][399], expected[1][399]);
    for y in [0.25, 0.5, 3.75, 20.0, 37.25, 50.5, 81.75] {
        assert!(bars(&first, y) == expected, "anticipating scroll {y}");
        h.send(ToContent::Scroll { x: 0.0, y });
        let (_, FromContent::Frame(f)) = wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::Frame(f) if f.scroll.1 == y)).unwrap() else { unreachable!() };
        assert!(bars(&f, y) == expected, "repainted scroll {y}");
    }
}

/// Development probe: `RH_PROBE_URL=<url> cargo test -p rabbithole
/// probe_fixed_layer_on_url -- --ignored --nocapture` (with
/// `RH_FIXED_LAYER_TRACE=1`) reports which frames carry a fixed layer while
/// the page loads and scrolls.
#[test]
#[ignore]
fn probe_fixed_layer_on_url() {
    let Ok(target) = std::env::var("RH_PROBE_URL") else { return };
    let scale = std::env::var("RH_PROBE_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let (sink, rx) = sink();
    let h = ContentHandle::spawn(1, 1, config(ViewSize::new(1280, 800, scale)), sink.clone());
    h.send(ToContent::SetActive(true));
    h.send(ToContent::Navigate { request: Request::get(url(&target)), push_history: true });
    let report = |label: &str, f: &Frame| eprintln!("[probe] {label} seq={} scroll={:?} origin={:?} layers={:?}", f.seq, f.scroll, f.origin, motions(f));
    let deadline = Instant::now() + Duration::from_secs(std::env::var("RH_PROBE_WAIT").ok().and_then(|s| s.parse().ok()).unwrap_or(20));
    let mut last = None;
    while let Some((_, m)) = wait_for(&rx, 1, deadline.saturating_duration_since(Instant::now()), is_frame) {
        if let FromContent::Frame(f) = m { report("load", &f); last = Some(f) }
    }
    let Some(mut last) = last else { return };
    // Rows of the window above `band` CSS px, as the UI shows a frame.
    let band: f32 = std::env::var("RH_PROBE_BAND").ok().and_then(|s| s.parse().ok()).unwrap_or(60.0);
    let rows = |f: &Frame, y: f32| {
        let mut dst = rabbithole_engine::gfx::Canvas::new(1280, 800);
        f.present(&mut dst, Rect::new(0.0, 0.0, 1280.0, 800.0), (0.0, y), scale);
        dst.data[..(band * scale) as usize * 1280].to_vec()
    };
    // RH_PROBE_EVAL, then RH_PROBE_EVAL2 three seconds later: the
    // reference is the frame shown once they settled.
    for (n, var) in ["RH_PROBE_EVAL", "RH_PROBE_EVAL2"].into_iter().enumerate() {
        let Ok(js) = std::env::var(var) else { continue };
        if n > 0 { std::thread::sleep(Duration::from_secs(3)); }
        h.send(ToContent::Eval(js));
        if let Some((_, FromContent::EvalResult(r))) = wait_for(&rx, 1, Duration::from_secs(60), |m| matches!(m, FromContent::EvalResult(_))) {
            eprintln!("[probe] eval {r}");
        }
    }
    let settle = Instant::now() + Duration::from_secs(std::env::var("RH_PROBE_SETTLE").ok().and_then(|s| s.parse().ok()).unwrap_or(3));
    while let Some((_, m)) = wait_for(&rx, 1, settle.saturating_duration_since(Instant::now()), is_frame) {
        if let FromContent::Frame(f) = m { report("settle", &f); last = f }
    }
    let reference = rows(&last, 0.0);
    // RH_PROBE_RECT=x,y,w,h (CSS px of the viewport): compared between
    // the anticipated and the repainted frame of every step.
    let rect: Option<Vec<f32>> = std::env::var("RH_PROBE_RECT").ok().map(|r| r.split(',').filter_map(|v| v.parse().ok()).collect());
    let region = |f: &Frame, y: f32| -> Vec<u32> {
        let Some(r) = rect.as_ref().filter(|r| r.len() == 4) else { return Vec::new() };
        let mut dst = rabbithole_engine::gfx::Canvas::new(1280, 800);
        f.present(&mut dst, Rect::new(0.0, 0.0, 1280.0, 800.0), (0.0, y), scale);
        let (x0, y0, x1, y1) = ((r[0] * scale) as usize, (r[1] * scale) as usize, ((r[0] + r[2]) * scale).min(1280.0) as usize, ((r[1] + r[3]) * scale).min(800.0) as usize);
        (y0..y1).flat_map(|row| dst.data[row * 1280 + x0..row * 1280 + x1].to_vec()).collect()
    };
    for i in 1..=20 {
        let y = i as f32 * 37.5;
        h.send(ToContent::Scroll { x: 0.0, y });
        if let Some((_, FromContent::Frame(f))) = wait_for(&rx, 1, Duration::from_secs(5), |m| matches!(m, FromContent::Frame(f) if f.scroll.1 == y)) {
            report("scroll", &f);
            let (a, b) = (region(&last, y), region(&f, y));
            let width = rect.as_ref().map(|r| ((r[0] + r[2]) * scale).min(1280.0) as usize - (r[0] * scale) as usize).unwrap_or(1);
            let diff: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
            let rows_hit = (diff.first().map(|i| i / width), diff.last().map(|i| i / width));
            let cols = (diff.iter().map(|i| i % width).min(), diff.iter().map(|i| i % width).max());
            eprintln!("[probe]   speculative={} repainted={} (pixels differing from the unscrolled band) band-moved={} region={} rows={rows_hit:?} cols={cols:?}",
                mismatches(&rows(&last, y), &reference), mismatches(&rows(&f, y), &reference), mismatches(&rows(&last, y), &rows(&f, y)), diff.len());
            last = f;
        }
    }
}
