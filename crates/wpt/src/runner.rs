//! Running tests: a child process per test page (crash and hang
//! isolation) loads the page in the engine and prints the harness results;
//! the parent schedules children over a few jobs and collects results.

use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rabbithole_engine::{Engine, Viewport};
use serde_json::Value;

use crate::discover::TestId;

/// Subtest status names (testharness.js numbering).
pub const SUBTEST_STATUS: [&str; 5] = ["PASS", "FAIL", "TIMEOUT", "NOTRUN", "PRECONDITION_FAILED"];
/// Harness status names (testharness.js numbering).
pub const HARNESS_STATUS: [&str; 4] = ["OK", "ERROR", "TIMEOUT", "PRECONDITION_FAILED"];

#[derive(Debug, Clone, PartialEq)]
pub struct Subtest {
    pub name: String,
    pub status: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TestResult {
    pub url: String,
    /// OK, ERROR, TIMEOUT, PRECONDITION_FAILED (harness), or CRASH /
    /// NORESULT (runner).
    pub harness: String,
    pub message: Option<String>,
    pub subtests: Vec<Subtest>,
    pub ms: u64,
}

impl TestResult {
    /// (passed, total) as counted for pass rates: each subtest counts; a
    /// page without subtests counts as one test that passes when the
    /// harness completed OK.
    pub fn counts(&self) -> (usize, usize) {
        if self.subtests.is_empty() {
            return (usize::from(self.harness == "OK"), 1);
        }
        (self.subtests.iter().filter(|s| s.status == "PASS").count(), self.subtests.len())
    }
}

fn status_name(table: &[&str], v: &Value) -> String {
    match v.as_u64() {
        Some(i) => table.get(i as usize).copied().unwrap_or("UNKNOWN").to_string(),
        None => v.as_str().unwrap_or("UNKNOWN").to_string(),
    }
}

/// Parse the JSON line the report script prints.
pub fn parse_report(url: &str, json: &str, ms: u64) -> Option<TestResult> {
    let v: Value = serde_json::from_str(json).ok()?;
    let subtests = v["tests"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|t| Subtest {
                    name: t["name"].as_str().unwrap_or("").to_string(),
                    status: status_name(&SUBTEST_STATUS, &t["status"]),
                    message: t["message"].as_str().map(|s| s.to_string()),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(TestResult {
        url: url.to_string(),
        harness: status_name(&HARNESS_STATUS, &v["status"]),
        message: v["message"].as_str().map(|s| s.to_string()),
        subtests,
        ms,
    })
}

/// A script can stop while registering synchronous tests and leave an OK
/// report for only the prefix it reached. Preserve that prefix, but mark
/// execution incomplete using the engine's timeout state, never log text.
fn report_after_watchdog_abort(json: &str, watchdog_aborts: u64) -> String {
    if watchdog_aborts == 0 {
        return json.to_string();
    }
    let Ok(mut report) = serde_json::from_str::<Value>(json) else { return json.to_string() };
    if report["status"].as_u64() != Some(0) && report["status"].as_str() != Some("OK") {
        return json.to_string();
    }
    report["status"] = Value::from(2);
    let reason = format!("Execution watchdog aborted {watchdog_aborts} page task(s); registered subtests may be incomplete.");
    let message = match report["message"].as_str() {
        Some(message) if !message.is_empty() => format!("{reason} {message}"),
        _ => reason,
    };
    report["message"] = Value::from(message);
    report.to_string()
}

const MARKER: &str = "@@WPT@@";
const TD_MARKER: &str = "@@TD@@";

/// WebDriver key → (DOM key, DOM code).
fn webdriver_key(c: char) -> (String, String) {
    let named = match c {
        '\u{E003}' => Some(("Backspace", "Backspace")),
        '\u{E004}' => Some(("Tab", "Tab")),
        '\u{E006}' | '\u{E007}' => Some(("Enter", "Enter")),
        '\u{E008}' | '\u{E050}' => Some(("Shift", "ShiftLeft")),
        '\u{E009}' | '\u{E051}' => Some(("Control", "ControlLeft")),
        '\u{E00A}' | '\u{E052}' => Some(("Alt", "AltLeft")),
        '\u{E00C}' => Some(("Escape", "Escape")),
        '\u{E00D}' => Some((" ", "Space")),
        '\u{E00E}' => Some(("PageUp", "PageUp")),
        '\u{E00F}' => Some(("PageDown", "PageDown")),
        '\u{E010}' => Some(("End", "End")),
        '\u{E011}' => Some(("Home", "Home")),
        '\u{E012}' => Some(("ArrowLeft", "ArrowLeft")),
        '\u{E013}' => Some(("ArrowUp", "ArrowUp")),
        '\u{E014}' => Some(("ArrowRight", "ArrowRight")),
        '\u{E015}' => Some(("ArrowDown", "ArrowDown")),
        '\u{E017}' => Some(("Delete", "Delete")),
        '\u{E03D}' | '\u{E053}' => Some(("Meta", "MetaLeft")),
        _ => None,
    };
    if let Some((k, code)) = named {
        return (k.to_string(), code.to_string());
    }
    let code = if c.is_ascii_alphabetic() {
        format!("Key{}", c.to_ascii_uppercase())
    } else if c.is_ascii_digit() {
        format!("Digit{c}")
    } else if c == ' ' {
        "Space".to_string()
    } else {
        String::new()
    };
    (c.to_string(), code)
}

fn set_modifier(m: &mut rabbithole_engine::Modifiers, key: &str, down: bool) {
    match key {
        "Shift" => m.shift = down,
        "Control" => m.ctrl = down,
        "Alt" => m.alt = down,
        "Meta" => m.meta = down,
        _ => {}
    }
}

/// Perform one test_driver request with the engine's input pipeline.
fn perform_test_driver(engine: &Engine, page: &mut rabbithole_engine::Page, req: &Value) -> Result<(), String> {
    let num = |v: &Value| v.as_f64().unwrap_or(0.0) as f32;
    let mut m = rabbithole_engine::Modifiers::default();
    match req["action"].as_str().unwrap_or("") {
        "click" => {
            let _ = engine.click_at(page, num(&req["x"]), num(&req["y"]), m);
        }
        "keys" => {
            for c in req["keys"].as_str().unwrap_or("").chars() {
                let (key, code) = webdriver_key(c);
                let modifier = matches!(key.as_str(), "Shift" | "Control" | "Alt" | "Meta");
                if modifier {
                    // Modifier keys stay pressed until the end (WebDriver).
                    set_modifier(&mut m, &key, true);
                    let _ = engine.key_input(page, &key, &code, true, m);
                    continue;
                }
                let _ = engine.key_input(page, &key, &code, true, m);
                let _ = engine.key_input(page, &key, &code, false, m);
            }
        }
        "actions" => {
            let sources = req["sources"].as_array().cloned().unwrap_or_default();
            let ticks = sources.iter().map(|s| s["actions"].as_array().map(|a| a.len()).unwrap_or(0)).max().unwrap_or(0);
            let (mut px, mut py) = (0.0f32, 0.0f32);
            for i in 0..ticks {
                for src in &sources {
                    let Some(a) = src["actions"].as_array().and_then(|a| a.get(i)) else { continue };
                    let button = a["button"].as_u64().unwrap_or(0) as u8;
                    match (src["type"].as_str().unwrap_or(""), a["type"].as_str().unwrap_or("")) {
                        ("pointer", "pointerMove") => {
                            let (x, y) = if a["origin"].as_str() == Some("pointer") { (px + num(&a["x"]), py + num(&a["y"])) } else { (num(&a["x"]), num(&a["y"])) };
                            px = x;
                            py = y;
                            let _ = engine.pointer_move(page, x, y, m);
                        }
                        ("pointer", "pointerDown") => {
                            let _ = engine.pointer_down(page, px, py, button, m);
                        }
                        ("wheel", "scroll") => {
                            let _ = engine.scroll_at(page, num(&a["x"]), num(&a["y"]), num(&a["deltaX"]), num(&a["deltaY"]));
                        }
                        ("pointer", "pointerUp") => {
                            let _ = engine.pointer_up(page, px, py, button, m);
                        }
                        ("key", t @ ("keyDown" | "keyUp")) => {
                            let c = a["value"].as_str().and_then(|v| v.chars().next()).unwrap_or('\0');
                            let (key, code) = webdriver_key(c);
                            set_modifier(&mut m, &key, t == "keyDown");
                            let _ = engine.key_input(page, &key, &code, t == "keyDown", m);
                        }
                        _ => {}
                    }
                }
                let _ = engine.tick(page);
            }
        }
        other => return Err(format!("test_driver action '{other}' is not supported")),
    }
    Ok(())
}

/// Child side: load `url`, run its event loop until the harness reports
/// (or `timeout` passes, then ask the harness to time out), and print the
/// report JSON line on stdout.
pub fn run_child(url: &str, timeout: Duration, verbose: bool) -> i32 {
    let start = Instant::now();
    let mut engine = Engine::new_bundled_fonts_only();
    engine.loader.load_fonts = false;
    let viewport = Viewport { width: 800.0, height: 600.0 };
    let mut page = match engine.load(url, viewport) {
        Ok(p) => p,
        Err(e) => {
            println!("{{\"status\":\"NORESULT\",\"message\":{},\"tests\":[]}}", Value::from(format!("load failed: {e}")));
            return 0;
        }
    };
    let mut driver_requests: Vec<String> = Vec::new();
    let check = |page: &mut rabbithole_engine::Page, driver: &mut Vec<String>| -> Option<String> {
        let mut found = None;
        for (level, msg) in page.take_console() {
            if let Some(j) = msg.strip_prefix(MARKER) {
                found = Some(j.to_string());
            } else if let Some(j) = msg.strip_prefix(TD_MARKER) {
                driver.push(j.to_string());
            } else if verbose {
                eprintln!("[console.{level}] {msg}");
            }
        }
        found
    };
    let deadline = start + timeout;
    let mut timed_out = false;
    let mut grace_end = None;
    loop {
        if let Some(j) = check(&mut page, &mut driver_requests) {
            println!("{}", report_after_watchdog_abort(&j, page.script.watchdog_abort_count()));
            return 0;
        }
        for r in std::mem::take(&mut driver_requests) {
            let Ok(req) = serde_json::from_str::<Value>(&r) else { continue };
            let id = req["id"].as_u64().unwrap_or(0);
            let res = perform_test_driver(&engine, &mut page, &req);
            let err = match res {
                Ok(()) => "null".to_string(),
                Err(e) => Value::from(e).to_string(),
            };
            let _ = engine.eval_in_page(&mut page, &format!("self.__rhTestDriverDone && self.__rhTestDriverDone({id}, {err}); 0"));
        }
        let now = Instant::now();
        if !timed_out && now >= deadline {
            timed_out = true;
            grace_end = Some(now + Duration::from_millis(1500));
            // explicit_timeout: the harness waits for the runner to call
            // `timeout()`, which reports the unfinished tests as TIMEOUT.
            let _ = engine.eval_in_page(&mut page, "typeof timeout === 'function' && timeout(); 0");
            continue;
        }
        if let Some(g) = grace_end {
            if now >= g {
                break;
            }
        }
        let now_ms = engine.start.elapsed().as_secs_f64() * 1000.0;
        let wait = match page.next_script_due() {
            Some(due) if due > now_ms => Duration::from_micros(((due - now_ms) * 1000.0) as u64).min(Duration::from_millis(5)),
            Some(_) => Duration::ZERO,
            None => Duration::from_millis(5),
        };
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
        let _ = engine.tick(&mut page);
    }
    println!("{{\"status\":\"TIMEOUT\",\"message\":\"no harness report\",\"tests\":[]}}");
    0
}

/// Parent-side options.
#[derive(Debug, Clone)]
pub struct RunOptions {
    pub jobs: usize,
    pub timeout: Duration,
    pub long_timeout: Duration,
    pub verbose: bool,
    /// Address-space limit for each child (KiB, 0 = none).
    pub memory_kb: u64,
}

/// Run one test in a child process with a hard kill deadline.
pub fn run_one(exe: &std::path::Path, base: &str, test: &TestId, opts: &RunOptions) -> TestResult {
    let start = Instant::now();
    let timeout = if test.long { opts.long_timeout } else { opts.timeout };
    let url = format!("{base}{}", test.url);
    // Each page runs under an address-space limit (the machines running
    // this are small and shared): `sh -c 'ulimit -v N; exec rh-wpt child ...'`.
    let mut cmd = if cfg!(unix) && opts.memory_kb > 0 {
        let mut c = Command::new("sh");
        c.arg("-c").arg(format!("ulimit -v {} 2>/dev/null; exec \"$0\" \"$@\"", opts.memory_kb)).arg(exe);
        c
    } else {
        Command::new(exe)
    };
    cmd.arg("child").arg(&url).arg(timeout.as_millis().to_string());
    if opts.verbose {
        cmd.arg("--verbose");
    }
    cmd.stdout(Stdio::piped()).stderr(if opts.verbose { Stdio::inherit() } else { Stdio::null() }).stdin(Stdio::null());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return crash(test, &format!("spawn failed: {e}"), 0),
    };
    let mut stdout = child.stdout.take().expect("piped stdout");
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    // Hard deadline: the child's own timeout plus time to report.
    let kill_at = start + timeout + Duration::from_secs(8);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if Instant::now() >= kill_at => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => break None,
        }
    };
    let out = reader.join().unwrap_or_default();
    let ms = start.elapsed().as_millis() as u64;
    let line = BufReader::new(out.as_bytes()).lines().map_while(Result::ok).filter(|l| l.starts_with('{')).last();
    match (status, line) {
        (_, Some(l)) => parse_report(&test.url, &l, ms).unwrap_or_else(|| crash(test, "unparsable report", ms)),
        (None, None) => TestResult { url: test.url.clone(), harness: "TIMEOUT".into(), message: Some("killed".into()), subtests: Vec::new(), ms },
        (Some(s), None) => crash(test, &format!("exited with {s}"), ms),
    }
}

fn crash(test: &TestId, msg: &str, ms: u64) -> TestResult {
    TestResult { url: test.url.clone(), harness: "CRASH".into(), message: Some(msg.to_string()), subtests: Vec::new(), ms }
}

/// Run all `tests` over `opts.jobs` worker threads; `progress` is called
/// after each result.
pub fn run_all(base: &str, tests: &[TestId], opts: &RunOptions, progress: &(dyn Fn(usize, usize, &TestResult) + Sync)) -> Vec<TestResult> {
    let exe = std::env::current_exe().expect("current exe");
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let results: Arc<Mutex<Vec<Option<TestResult>>>> = Arc::new(Mutex::new(vec![None; tests.len()]));
    std::thread::scope(|s| {
        for _ in 0..opts.jobs.max(1) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= tests.len() {
                    break;
                }
                let r = run_one(&exe, base, &tests[i], opts);
                let n = done.fetch_add(1, Ordering::SeqCst) + 1;
                progress(n, tests.len(), &r);
                results.lock().unwrap()[i] = Some(r);
            });
        }
    });
    let v = std::mem::take(&mut *results.lock().unwrap());
    v.into_iter().flatten().collect()
}
