//! Source-bound MCP async evaluation over a private loopback origin.
use rabbithole_agent::{call, Agent, Config};
use serde_json::json;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, Condvar};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

struct Server {
    address: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<(String, String, String)>>>,
    release: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}
impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let held = release.clone();
        let stopped = stop.clone();
        let observed = requests.clone();
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stopped.load(Ordering::Acquire) { break; }
                let Ok(mut stream) = stream else { break };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut reader = BufReader::new(&mut stream);
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() { continue; }
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" { break; }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") { length = value.trim().parse::<usize>().unwrap_or(0).min(4096); }
                    }
                }
                let mut body = vec![0; length];
                if reader.read_exact(&mut body).is_err() { continue; }
                let mut words = first.split_whitespace();
                let method = words.next().unwrap_or("").to_string();
                let path = words.next().unwrap_or("/").to_string();
                observed.lock().unwrap().push((method, path.clone(), String::from_utf8_lossy(&body).into_owned()));
                if path == "/held" {
                    let (lock, changed) = &*held;
                    drop(changed.wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(30), |released| !*released).unwrap());
                }
                if path == "/pixel.svg" {
                    let image = "<svg xmlns='http://www.w3.org/2000/svg' width='1' height='1'><rect width='1' height='1' fill='green'/></svg>";
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: image/svg+xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", image.len(), image);
                    continue;
                }
                if path == "/slow" { std::thread::sleep(Duration::from_millis(100)); }
                let html = if path == "/child-open" {
                    "<!doctype html><p>Child</p><script>setTimeout(()=>{parent.document.open();parent.document.write('<p>Parent replacement</p>');parent.document.close()},10)</script>"
                } else if path == "/next" || path == "/submit" { "<!doctype html><title>Arrived</title><p>Destination</p>" } else {
                    "<!doctype html><title>Async fixture</title><p id='result'>Original</p><form id='form' method='post' action='/submit'><input name='message' value='causal'></form><script>globalThis.fixtureReady=true;</script>"
                };
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", html.len(), html);
            }
        });
        Self { address, stop, requests, release, thread: Some(thread) }
    }
    fn release(&self) { let (lock, changed) = &*self.release; *lock.lock().unwrap() = true; changed.notify_all(); }
    fn url(&self, path: &str) -> String { format!("http://{}{path}", self.address) }
    fn agent(&self) -> Agent {
        let mut agent = Agent::new(Config { settle_cap: Duration::from_millis(500), ..Config::default() });
        call(&mut agent, "open", &json!({"url":self.url("/")})).unwrap();
        agent
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.release();
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_secs(1));
        if let Some(thread) = self.thread.take() { thread.join().unwrap(); }
    }
}
fn eval(agent: &mut Agent, source: &str, timeout: u64) -> String {
    call(agent, "debug", &json!({"eval":source,"timeout":timeout})).unwrap().text
}

#[test]
fn debug_waits_for_loopback_fetch_timers_rejection_and_top_level_await() {
    let server = Server::new();
    let mut agent = server.agent();
    assert!(eval(&mut agent, "fetch(location.href).then(r=>r.status)", 500).contains("\n200\n"));
    assert!(eval(&mut agent, "await new Promise(r=>setTimeout(()=>r(7),50))", 500).contains("\n7\n"));
    assert!(eval(&mut agent, "await(Promise.resolve(42))", 500).contains("\n42\n"));
    assert!(eval(&mut agent, "await 0; 9; const after=1", 500).contains("\n9\n"));
    assert!(eval(&mut agent, "Promise.reject(new Error('rejected-marker'))", 500).contains("Uncaught (in promise) Error: rejected-marker"));
    assert!(eval(&mut agent, "await ({get then(){throw new Error('getter-marker')}})", 500).contains("getter-marker"));
    for source in ["'await'; /* await */ var normalDebugBinding=17; normalDebugBinding", "async function nestedDebug(){await 1}; normalDebugBinding"] {
        assert!(eval(&mut agent, source, 500).contains("\n17\n"));
    }
    let started = Instant::now();
    let pending = eval(&mut agent, "new Promise(()=>{})", 200);
    assert!(pending.contains("Promise still pending after 200 ms"), "{pending}");
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}: {pending}", started.elapsed());
}

#[test]
fn debug_eval_lexical_scope_is_fresh_and_explicit_globals_persist() {
    let server = Server::new();
    let mut agent = server.agent();
    for (source, expected) in [
        ("const s = 1; s", "1"),
        ("const s = 2; s", "2"),
        ("let local = 3; local += 1; local", "4"),
        ("let local = 5; local", "5"),
        ("class Local { value(){return 6} }; new Local().value()", "6"),
        ("class Local { value(){return 7} }; new Local().value()", "7"),
        ("[typeof s, typeof local, typeof Local].join('|')", "'undefined|undefined|undefined'"),
        ("var sharedDebugVar = 10; globalThis.sharedDebugValue = 20; sharedDebugVar + sharedDebugValue", "30"),
        ("sharedDebugVar += 1; sharedDebugValue += 2; sharedDebugVar + sharedDebugValue", "33"),
        ("const s = 40; globalThis.savedDebugScope = () => s; savedDebugScope()", "40"),
        ("const s = 2; savedDebugScope() + s", "42"),
        ("'use strict'; var strictDebugVar = 12; let local = 1; strictDebugVar + local", "13"),
        ("strictDebugVar", "12"),
        ("{ const nested = 8; nested + 1; }; const after = 1", "9"),
        ("let asyncLocal = 9; Promise.resolve().then(() => asyncLocal)", "9"),
    ] {
        let result = eval(&mut agent, source, 500);
        assert!(result.contains(&format!("\n{expected}\n")), "{source}: {result}");
    }
    assert!(eval(&mut agent, "const failedScope = 1; throw new Error('scope-marker')", 500).contains("scope-marker"));
    assert!(eval(&mut agent, "const failedScope = 2; failedScope", 500).contains("\n2\n"));
}

#[test]
fn debug_eval_await_keeps_scope_completion_and_global_vars() {
    let server = Server::new();
    let mut agent = server.agent();
    for (source, expected) in [
        ("const s = await Promise.resolve(1); s", "1"),
        ("const s = await Promise.resolve(2); s; const after = 0", "2"),
        ("let local = await Promise.resolve(3); local", "3"),
        ("let local = await Promise.resolve(4); local", "4"),
        ("class Local { [await Promise.resolve('value')](){return 5} }; new Local().value()", "5"),
        ("class Local { [await Promise.resolve('value')](){return 6} }; new Local().value()", "6"),
        ("[typeof s, typeof local, typeof Local].join('|')", "'undefined|undefined|undefined'"),
        ("var asyncDebugVar = await Promise.resolve(7); globalThis.asyncDebugValue = 8; asyncDebugVar + asyncDebugValue", "15"),
        ("asyncDebugVar + asyncDebugValue", "15"),
        ("'use strict'; var strictAsyncDebugVar = await Promise.resolve(9); strictAsyncDebugVar", "9"),
        ("strictAsyncDebugVar", "9"),
        ("const s = await Promise.resolve(40); globalThis.savedAsyncScope = () => s; s", "40"),
        ("const s = 2; savedAsyncScope() + s", "42"),
        ("try { await 0; 11; } finally { 12; }", "11"),
        ("await 0; this === globalThis", "true"),
    ] {
        let result = eval(&mut agent, source, 500);
        assert!(result.contains(&format!("\n{expected}\n")), "{source}: {result}");
    }
}

#[test]
fn debug_old_fetch_advances_on_later_tools_after_observation_timeout() {
    let server = Server::new();
    let mut agent = server.agent();
    assert!(eval(&mut agent, "globalThis.oldFetch=fetch('/slow').then(r=>{document.getElementById('result').textContent='Old fetch completed';return r.status}); oldFetch", 20).contains("Promise still pending after 20 ms"));
    let deadline = Instant::now() + Duration::from_secs(2);
    let text = loop {
        let text = call(&mut agent, "read", &json!({})).unwrap().text;
        if text.contains("Old fetch completed") { break text; }
        assert!(Instant::now() < deadline, "old fetch never delivered: {text}");
        std::thread::yield_now();
    };
    assert!(!text.contains("Original"));
    assert!(eval(&mut agent, "oldFetch", 500).contains("\n200\n"));
}

#[test]
fn debug_follow_is_explicit_and_preserves_form_post_navigation() {
    let server = Server::new();
    let mut agent = server.agent();
    let result = eval(&mut agent, "Promise.resolve().then(()=>location.href='/next')", 500);
    assert!(result.contains("not followed"), "{result}");
    assert_eq!(agent.tabs[agent.active.unwrap()].frames[0].url.to_string(), server.url("/"));
    let result = call(&mut agent, "debug", &json!({"eval":"new Promise(r=>setTimeout(()=>{document.getElementById('form').submit();r(1)},10))","timeout":500,"follow":true})).unwrap().text;
    assert!(result.contains("navigated to") && result.contains("/submit"), "{result}");
    assert_eq!(agent.tabs[agent.active.unwrap()].frames[0].url.to_string(), server.url("/submit"));
    assert!(server.requests.lock().unwrap().iter().any(|(method,path,body)|method=="POST" && path=="/submit" && body=="message=causal"));
}

#[test]
fn debug_cancels_replaced_document_and_bounds_runaway_microtasks() {
    let server = Server::new();
    let mut agent = server.agent();
    let result = eval(&mut agent, "new Promise(r=>setTimeout(()=>{document.open();document.write('<p>Replacement</p>');document.close();r(8)},10))", 500);
    assert!(result.contains("document was replaced"), "{result}");
    let started = Instant::now();
    let result = eval(&mut agent, "globalThis.haltDebugLoop=false; Promise.resolve().then(function recur(){if(!haltDebugLoop) Promise.resolve().then(recur)}); new Promise(()=>{})", 200);
    // The timeout is an upper bound: the CPU watchdog may abort the runaway
    // work before the wall cap, without claiming that all 200 ms elapsed.
    let message = result.lines().nth(2).unwrap_or("");
    assert!(matches!(message, "Promise still pending after 200 ms" | "Error: Console evaluation deadline expired"), "{result}");
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}: {result}", started.elapsed());
    assert!(eval(&mut agent, "haltDebugLoop=true; 23", 500).contains("\n23\n"));
}


#[test]
fn debug_async_http_timeout_does_not_wait_for_a_held_response_or_cancel_it() {
    let server = Server::new();
    let mut agent = server.agent();
    let started = Instant::now();
    let result = eval(&mut agent, "globalThis.heldResponse=fetch('/held').then(r=>r.status); heldResponse", 200);
    assert!(result.contains("Promise still pending after 200 ms"), "{result}");
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}: {result}", started.elapsed());
    assert!(server.requests.lock().unwrap().iter().any(|(_, path, _)| path == "/held"));
    server.release();
    assert!(eval(&mut agent, "heldResponse", 500).contains("\n200\n"));
}

#[test]
fn debug_pumps_iframe_and_image_ready_events_and_cancels_foreign_open() {
    let server = Server::new();
    let mut agent = Agent::new(Config { images: true, settle_cap: Duration::from_millis(500), ..Config::default() });
    call(&mut agent, "open", &json!({"url":server.url("/")})).unwrap();
    let result = eval(&mut agent, "new Promise((r,j)=>{const f=document.createElement('iframe');f.onload=()=>r(7);f.onerror=()=>j(new Error('frame failed'));f.src='/next';document.body.appendChild(f)})", 1000);
    assert!(result.contains("\n7\n"), "{result}");
    let result = eval(&mut agent, "new Promise((r,j)=>{const image=new Image();image.onload=()=>r(image.naturalWidth);image.onerror=()=>j(new Error('image failed'));image.src='/pixel.svg';document.body.appendChild(image)})", 1000);
    assert!(result.contains("\n1\n"), "{result}");
    let result = eval(&mut agent, "new Promise(r=>{const f=document.createElement('iframe');f.src='/child-open';document.body.appendChild(f)})", 1000);
    assert!(result.contains("document was replaced"), "{result}");
}

#[test]
fn debug_follow_propagates_navigation_failure_after_observation_closes() {
    let server = Server::new();
    let mut agent = server.agent();
    let unused = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = unused.local_addr().unwrap();
    drop(unused);
    let source = format!("location.href='http://{address}/unavailable'; 1");
    let result = call(&mut agent, "debug", &json!({"eval":source,"timeout":500,"follow":true}));
    assert!(result.is_err(), "follow navigation failure was discarded");
}
