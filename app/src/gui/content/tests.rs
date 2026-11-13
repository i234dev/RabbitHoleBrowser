//! Headless tests of the content-thread protocol: the test plays the UI
//! thread, sending messages and checking what comes back (and that it
//! never has to wait for a busy page).

use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rabbithole_engine::net::{Request, Url};

mod consent;
mod feeds;
mod focus;

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
    rabbithole_engine::layout::paint_with_fixed_origin(&page.display_list, &mut c, &engine.fonts, f.origin.0, f.origin.1, (f.scroll.0 - f.origin.0, f.scroll.1 - f.origin.1));
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
