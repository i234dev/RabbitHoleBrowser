//! test262 runner core, shared by `examples/test262.rs` (the full run)
//! and `tests/test262.rs` (the default smoke subset and the `#[ignore]`d
//! full run).
//!
//! * [`run_test`] runs one test file in-process: it parses the YAML
//!   front-matter, runs the file in the modes it asks for (non-strict and
//!   strict, raw, module), with the harness includes, `$262`, `print`,
//!   `$DONE` (async tests) and a module loader rooted at the test's
//!   directory, and classifies the outcome.
//! * [`run_parallel`] runs many tests in worker *subprocesses* (one test in
//!   flight per worker), so a crash or hang of the engine costs exactly one
//!   test: the worker is restarted and the test recorded as CRASH/TIMEOUT.
//! * Reports: per-directory table, failures file, baseline file and
//!   `--compare` against a baseline.
#![allow(dead_code)]

use rabbithole_js::heap::ObjectKind;
use rabbithole_js::{ModuleId, PropertyKey, Realm, Value, V};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

/// Default location of the test262 checkout.
pub const DEFAULT_SUITE: &str = "/mnt/hdd5tb/rabbithole/suites/test262";

/// The suite: `$TEST262_DIR`, else `~/suites/test262`, else [`DEFAULT_SUITE`].
pub fn default_suite() -> PathBuf {
    if let Ok(d) = std::env::var("TEST262_DIR") {
        return PathBuf::from(d);
    }
    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home).join("suites/test262");
        if p.join("test").is_dir() {
            return p;
        }
    }
    PathBuf::from(DEFAULT_SUITE)
}

/// Cap this process's address space (`RLIMIT_AS`, bytes) so a runaway
/// test fails its own worker instead of exhausting the machine.
pub fn limit_memory(bytes: u64) {
    #[cfg(target_os = "linux")]
    {
        #[repr(C)]
        struct Rlimit {
            cur: u64,
            max: u64,
        }
        extern "C" {
            fn setrlimit(resource: i32, rlim: *const Rlimit) -> i32;
        }
        // RLIMIT_AS = 9.
        // SAFETY: plain libc call with a valid struct.
        unsafe { setrlimit(9, &Rlimit { cur: bytes, max: bytes }) };
    }
    #[cfg(not(target_os = "linux"))]
    let _ = bytes;
}

/// Features this engine does not implement (stage-3 proposals and a few
/// large standard additions); tests that need them are skipped.
pub const UNSUPPORTED_FEATURES: &[&str] = &[
    // Stage 3 proposals.
    "Intl.Era-monthcode",
    "Intl.Locale-info",
    "Intl.NumberFormat-v3",
    "legacy-regexp",
    "ShadowRealm",
    "decorators",
    "source-phase-imports",
    "source-phase-imports-module-source",
    "import-defer",
    "export-defer",
    "import-text",
    "import-bytes",
    "immutable-arraybuffer",
    "nonextensible-applies-to-private",
    "await-dictionary",
    "error-stack-accessor",
    "Iterator.prototype.join",
    // Standard but not implemented.
    "Temporal",
    // No browser implements proper tail calls.
    "tail-call-optimization",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Status {
    Pass,
    Fail,
    Timeout,
    Crash,
    Skip,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Timeout => "TIMEOUT",
            Status::Crash => "CRASH",
            Status::Skip => "SKIP",
        }
    }

    pub fn parse(s: &str) -> Option<Status> {
        Some(match s {
            "PASS" => Status::Pass,
            "FAIL" => Status::Fail,
            "TIMEOUT" => Status::Timeout,
            "CRASH" => Status::Crash,
            "SKIP" => Status::Skip,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Outcome {
    /// Path relative to the suite's `test/` directory.
    pub path: String,
    pub status: Status,
    /// First line of the error (or the skip reason).
    pub message: String,
}

// ------------------------------------------------------------ metadata

#[derive(Default, Debug, Clone)]
pub struct Meta {
    pub includes: Vec<String>,
    pub flags: Vec<String>,
    pub features: Vec<String>,
    /// (phase, type)
    pub negative: Option<(String, String)>,
}

impl Meta {
    pub fn has_flag(&self, f: &str) -> bool {
        self.flags.iter().any(|x| x == f)
    }
}

fn parse_list(s: &str) -> Vec<String> {
    let s = s.trim();
    let s = s.strip_prefix('[').unwrap_or(s);
    let s = s.strip_suffix(']').unwrap_or(s);
    s.split(',').map(|x| x.trim().trim_matches(|c| c == '\'' || c == '"').to_string()).filter(|x| !x.is_empty()).collect()
}

/// Parse the `/*--- ... ---*/` YAML front-matter (the subset test262 uses).
pub fn parse_meta(src: &str) -> Meta {
    let mut meta = Meta::default();
    let Some(start) = src.find("/*---") else { return meta };
    let Some(end) = src[start..].find("---*/") else { return meta };
    let yaml = &src[start + 5..start + end];
    let mut key = String::new();
    for line in yaml.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if !indented {
            if let Some((k, v)) = line.split_once(':') {
                key = k.trim().to_string();
                let v = v.trim();
                match key.as_str() {
                    "includes" => meta.includes.extend(parse_list(v)),
                    "flags" => meta.flags.extend(parse_list(v)),
                    "features" => meta.features.extend(parse_list(v)),
                    _ => {}
                }
            } else {
                key.clear();
            }
            continue;
        }
        match key.as_str() {
            "includes" | "flags" | "features" => {
                if let Some(item) = trimmed.strip_prefix("- ") {
                    let item = item.split(" #").next().unwrap_or(item).trim().to_string();
                    match key.as_str() {
                        "includes" => meta.includes.push(item),
                        "flags" => meta.flags.push(item),
                        _ => meta.features.push(item),
                    }
                }
            }
            "negative" => {
                if let Some((k, v)) = trimmed.split_once(':') {
                    let v = v.trim().to_string();
                    let neg = meta.negative.get_or_insert((String::new(), String::new()));
                    match k.trim() {
                        "phase" => neg.0 = v,
                        "type" => neg.1 = v,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    meta
}

/// Why a test is skipped, if it is.
pub fn skip_reason(meta: &Meta, src: &str) -> Option<String> {
    for f in &meta.features {
        if UNSUPPORTED_FEATURES.contains(&f.as_str()) {
            return Some(format!("feature {f}"));
        }
    }
    if meta.has_flag("CanBlockIsTrue") {
        return Some("flag CanBlockIsTrue".into());
    }
    if src.contains("$262.agent") {
        return Some("feature $262.agent".into());
    }
    None
}

// ------------------------------------------------------------- harness

/// Shared per-thread state of the running test (natives are plain fn
/// pointers, so they reach it through a thread-local).
#[derive(Default)]
struct TestState {
    printed: Vec<String>,
    suite: PathBuf,
}

thread_local! {
    static STATE: RefCell<TestState> = RefCell::new(TestState::default());
    static HARNESS_CACHE: RefCell<HashMap<String, Rc<String>>> = RefCell::new(HashMap::new());
}

fn harness_source(suite: &Path, name: &str) -> Result<Rc<String>, String> {
    HARNESS_CACHE.with(|c| {
        if let Some(s) = c.borrow().get(name) {
            return Ok(s.clone());
        }
        let p = suite.join("harness").join(name);
        let s = Rc::new(std::fs::read_to_string(&p).map_err(|e| format!("cannot read harness {}: {e}", p.display()))?);
        c.borrow_mut().insert(name.to_string(), s.clone());
        Ok(s)
    })
}

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Undefined)
}

fn native_print(realm: &mut Realm, _this: Value, args: &[Value]) -> Result<Value, Value> {
    let mut parts = Vec::new();
    for a in args {
        parts.push(realm.to_string(a.clone())?.to_rust_string());
    }
    STATE.with(|s| s.borrow_mut().printed.push(parts.join(" ")));
    Ok(Value::Undefined)
}

fn native_eval_script(realm: &mut Realm, _this: Value, args: &[Value]) -> Result<Value, Value> {
    let src = realm.to_string(arg(args, 0))?.to_rust_string();
    realm.eval(&src, "evalScript")
}

fn native_gc(_realm: &mut Realm, _this: Value, _args: &[Value]) -> Result<Value, Value> {
    Ok(Value::Undefined)
}

fn native_detach(realm: &mut Realm, _this: Value, args: &[Value]) -> Result<Value, Value> {
    rabbithole_js::builtins::typed_array::detach_array_buffer(realm, arg(args, 0))?;
    Ok(Value::Null)
}

fn native_create_realm(realm: &mut Realm, _this: Value, _args: &[Value]) -> Result<Value, Value> {
    Err(realm.throw_type_error("$262.createRealm is not supported"))
}

fn native_htmldda(_realm: &mut Realm, _this: Value, _args: &[Value]) -> Result<Value, Value> {
    Ok(Value::Null)
}

/// Install `print` and `$262` on the realm's global object.
pub fn install_host(realm: &mut Realm) {
    let global = realm.global();
    let print = realm.new_native_function("print", 1, native_print);
    realm.define_property(global, "print", Value::Object(print), rabbithole_js::WRITABLE | rabbithole_js::CONFIGURABLE);
    let d = realm.new_object();
    realm.define_property(d, "global", Value::Object(global), rabbithole_js::ALL);
    let f = realm.new_native_function("evalScript", 1, native_eval_script);
    realm.define_property(d, "evalScript", Value::Object(f), rabbithole_js::ALL);
    let f = realm.new_native_function("gc", 0, native_gc);
    realm.define_property(d, "gc", Value::Object(f), rabbithole_js::ALL);
    let f = realm.new_native_function("detachArrayBuffer", 1, native_detach);
    realm.define_property(d, "detachArrayBuffer", Value::Object(f), rabbithole_js::ALL);
    let f = realm.new_native_function("createRealm", 0, native_create_realm);
    realm.define_property(d, "createRealm", Value::Object(f), rabbithole_js::ALL);
    let dda = realm.new_native_function("IsHTMLDDA", 0, native_htmldda);
    realm.make_htmldda(dda);
    realm.define_property(d, "IsHTMLDDA", Value::Object(dda), rabbithole_js::ALL);
    realm.define_property(global, "$262", Value::Object(d), rabbithole_js::WRITABLE | rabbithole_js::CONFIGURABLE);
}

/// Resolve a module specifier against the referrer's directory.
fn resolve_specifier(referrer: &str, specifier: &str) -> PathBuf {
    let base = Path::new(referrer).parent().map(Path::to_path_buf).unwrap_or_default();
    let mut out = PathBuf::new();
    for c in base.join(specifier).components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn install_loader(realm: &mut Realm) {
    let cache: Rc<RefCell<HashMap<PathBuf, ModuleId>>> = Rc::new(RefCell::new(HashMap::new()));
    realm.set_module_loader(Box::new(move |realm: &mut Realm, referrer: &str, specifier: &str| {
        let path = if referrer.is_empty() { PathBuf::from(specifier) } else { resolve_specifier(referrer, specifier) };
        if let Some(id) = cache.borrow().get(&path) {
            return Ok(*id);
        }
        let src = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => return Err(realm.new_error("TypeError", &format!("cannot load module {}: {e}", path.display()))),
        };
        let url = path.to_string_lossy().to_string();
        let id = if url.ends_with(".json") {
            let json = realm.get(Value::Object(realm.global()), PropertyKey::str("JSON"))?;
            let parse = realm.get(json.clone(), PropertyKey::str("parse"))?;
            let s = realm.new_string(&src);
            let v = realm.call(parse, json, &[s])?;
            realm.create_synthetic_module(&url, vec![("default".to_string(), v)])
        } else {
            realm.compile_module(&src, &url)?
        };
        cache.borrow_mut().insert(path, id);
        Ok(id)
    }));
}

/// Name of the constructor of a thrown value (`e.constructor.name`).
fn error_type(realm: &mut Realm, e: &Value) -> Option<String> {
    if !e.is_object() {
        return None;
    }
    let ctor = realm.get(e.clone(), PropertyKey::str("constructor")).ok()?;
    if !ctor.is_object() {
        return None;
    }
    let name = realm.get(ctor, PropertyKey::str("name")).ok()?;
    match name.v() {
        V::String(s) => Some(s.to_rust_string()),
        _ => None,
    }
}

fn first_line(s: &str) -> String {
    let l = s.lines().next().unwrap_or("").trim();
    let mut l: String = l.chars().take(240).collect();
    l = l.replace('\t', " ");
    l
}

fn describe(realm: &mut Realm, e: &Value) -> String {
    let s = realm.error_to_string(e);
    first_line(&s)
}

/// The kind of run a test needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Sloppy,
    Strict,
    Raw,
    Module,
}

pub fn modes(meta: &Meta) -> Vec<Mode> {
    if meta.has_flag("module") {
        vec![Mode::Module]
    } else if meta.has_flag("raw") {
        vec![Mode::Raw]
    } else if meta.has_flag("onlyStrict") {
        vec![Mode::Strict]
    } else if meta.has_flag("noStrict") {
        vec![Mode::Sloppy]
    } else {
        vec![Mode::Sloppy, Mode::Strict]
    }
}

/// Promise state of an object (None when not a promise).
fn promise_state(realm: &Realm, v: &Value) -> Option<(rabbithole_js::heap::PromiseState, Value)> {
    let o = v.as_object()?;
    match &realm.obj(o).kind {
        ObjectKind::Promise(p) => Some((p.state, p.value.clone())),
        _ => None,
    }
}

fn drain_jobs(realm: &mut Realm) {
    for _ in 0..1000 {
        if !realm.has_pending_jobs() || realm.timed_out() {
            break;
        }
        realm.run_jobs();
    }
}

/// Run one mode of a test. Ok(()) on pass, Err(message) otherwise.
fn run_mode(suite: &Path, abs: &Path, src: &str, meta: &Meta, mode: Mode, deadline: Duration) -> Result<(), String> {
    let mut realm = Realm::new();
    realm.heap_limit = 4_000_000;
    let console: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = console.clone();
    realm.set_console_sink(Box::new(move |level, text| sink.borrow_mut().push(format!("{level}: {text}"))));
    install_host(&mut realm);
    install_loader(&mut realm);
    STATE.with(|s| s.borrow_mut().printed.clear());
    realm.set_deadline(Some(Instant::now() + deadline));
    let r = run_mode_in(&mut realm, suite, abs, src, meta, mode);
    let timed_out = realm.timed_out();
    // Dropping a realm full of cycles is the realm's business; keep it
    // from hiding the result if it misbehaves.
    drop(realm);
    match r {
        Err(m) if timed_out => Err(format!("TIMEOUT {m}")),
        r => r,
    }
}

fn run_mode_in(realm: &mut Realm, suite: &Path, abs: &Path, src: &str, meta: &Meta, mode: Mode) -> Result<(), String> {
    let is_async = meta.has_flag("async");
    if mode != Mode::Raw {
        let mut includes: Vec<String> = vec!["assert.js".into(), "sta.js".into()];
        if is_async {
            includes.push("doneprintHandle.js".into());
        }
        includes.extend(meta.includes.iter().cloned());
        for inc in &includes {
            let hs = harness_source(suite, inc)?;
            if let Err(e) = realm.exec(&hs, inc) {
                return Err(format!("harness {inc} threw: {}", describe(realm, &e)));
            }
        }
    }
    let file = abs.to_string_lossy().to_string();
    let negative = meta.negative.clone();
    let expect_type = |realm: &mut Realm, e: &Value, phase: &str| -> Result<(), String> {
        match &negative {
            Some((p, t)) if p == phase => {
                let got = error_type(realm, e);
                if got.as_deref() == Some(t.as_str()) {
                    Ok(())
                } else {
                    Err(format!("expected {phase} {t}, got {}", describe(realm, e)))
                }
            }
            _ => Err(format!("{}{}", if phase == "parse" { "parse error: " } else { "" }, describe(realm, e))),
        }
    };
    let result: Result<Value, Value>;
    if mode == Mode::Module {
        // Through the loader, so the entry module is in its cache
        // (self-imports); a load error here is the entry's parse error.
        let _ = src;
        let id = match realm.load_imported_module("", &file) {
            Ok(id) => id,
            Err(e) => return expect_type(realm, &e, "parse"),
        };
        if let Some((p, t)) = &negative {
            if p == "parse" {
                return Err(format!("expected parse {t}, parsed fine"));
            }
        }
        if let Err(e) = realm.link_module(id) {
            return expect_type(realm, &e, "resolution");
        }
        if let Some((p, t)) = &negative {
            if p == "resolution" {
                return Err(format!("expected resolution {t}, linked fine"));
            }
        }
        result = match realm.evaluate_module(id) {
            Ok(p) => {
                drain_jobs(realm);
                match promise_state(realm, &p) {
                    Some((rabbithole_js::heap::PromiseState::Rejected, v)) => Err(v),
                    _ => Ok(Value::Undefined),
                }
            }
            Err(e) => Err(e),
        };
    } else {
        let code_src: std::borrow::Cow<str> = if mode == Mode::Strict { format!("\"use strict\";\n{src}").into() } else { src.into() };
        let code = match realm.compile(&code_src, &file, false) {
            Ok(c) => c,
            Err(e) => return expect_type(realm, &e, "parse"),
        };
        if let Some((p, t)) = &negative {
            if p == "parse" {
                return Err(format!("expected parse {t}, parsed fine"));
            }
        }
        result = realm.run_script(code);
        drain_jobs(realm);
    }
    if realm.timed_out() {
        return Err("timed out".into());
    }
    match result {
        Err(e) => return expect_type(realm, &e, "runtime"),
        Ok(_) => {
            if let Some((p, t)) = &negative {
                return Err(format!("expected {p} {t}, completed normally"));
            }
        }
    }
    if is_async {
        let printed = STATE.with(|s| s.borrow().printed.clone());
        for line in &printed {
            if line.starts_with("Test262:AsyncTestComplete") {
                return Ok(());
            }
            if let Some(rest) = line.strip_prefix("Test262:AsyncTestFailure:") {
                return Err(format!("async failure: {}", first_line(rest)));
            }
        }
        return Err("async test did not call $DONE".into());
    }
    Ok(())
}

/// Run one test file (path relative to `<suite>/test`).
pub fn run_test(suite: &Path, rel: &str, deadline: Duration) -> Outcome {
    let abs = suite.join("test").join(rel);
    let outcome = |status, message: String| Outcome { path: rel.to_string(), status, message };
    let src = match std::fs::read_to_string(&abs) {
        Ok(s) => s,
        Err(e) => return outcome(Status::Fail, format!("cannot read: {e}")),
    };
    let meta = parse_meta(&src);
    if let Some(r) = skip_reason(&meta, &src) {
        return outcome(Status::Skip, r);
    }
    STATE.with(|s| s.borrow_mut().suite = suite.to_path_buf());
    for mode in modes(&meta) {
        if let Err(m) = run_mode(suite, &abs, &src, &meta, mode, deadline) {
            let status = if m.starts_with("TIMEOUT") || m == "timed out" { Status::Timeout } else { Status::Fail };
            let tag = match mode {
                Mode::Strict => "[strict] ",
                Mode::Sloppy => "[sloppy] ",
                _ => "",
            };
            return outcome(status, format!("{tag}{m}"));
        }
    }
    outcome(Status::Pass, String::new())
}

thread_local! {
    static PANIC_MSG: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Install a panic hook that records the message instead of printing it.
pub fn quiet_panics() {
    std::panic::set_hook(Box::new(|info| {
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "panic".to_string()
        };
        let loc = info.location().map(|l| format!(" at {}:{}", l.file(), l.line())).unwrap_or_default();
        PANIC_MSG.with(|p| *p.borrow_mut() = Some(format!("{msg}{loc}")));
    }));
}

/// [`run_test`] with panics turned into CRASH outcomes.
pub fn run_test_guarded(suite: &Path, rel: &str, deadline: Duration) -> Outcome {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_test(suite, rel, deadline))) {
        Ok(o) => o,
        Err(_) => {
            let msg = PANIC_MSG.with(|p| p.borrow_mut().take()).unwrap_or_else(|| "panic".into());
            Outcome { path: rel.to_string(), status: Status::Crash, message: format!("panic: {}", first_line(&msg)) }
        }
    }
}

/// Stack size of the threads that run tests.
pub const TEST_STACK: usize = 256 << 20;

// -------------------------------------------------------------- workers

/// Worker loop: read test paths (relative) from stdin, write one
/// `R\t<path>\t<STATUS>\t<message>` line per test to stdout.
pub fn worker_main(suite: PathBuf, deadline: Duration) {
    quiet_panics();
    let mb: u64 = std::env::var("RH_T262_MEM_MB").ok().and_then(|v| v.parse().ok()).unwrap_or(2000);
    limit_memory(mb << 20);
    let h = std::thread::Builder::new()
        .stack_size(TEST_STACK)
        .spawn(move || {
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            for line in stdin.lock().lines() {
                let Ok(rel) = line else { break };
                let rel = rel.trim().to_string();
                if rel.is_empty() {
                    continue;
                }
                let o = run_test_guarded(&suite, &rel, deadline);
                let mut out = stdout.lock();
                let _ = writeln!(out, "R\t{}\t{}\t{}", o.path, o.status.as_str(), o.message.replace(['\n', '\r', '\t'], " "));
                let _ = out.flush();
            }
        })
        .expect("spawn worker thread");
    let _ = h.join();
}

/// How to start a worker subprocess.
pub type WorkerCommand = Arc<dyn Fn() -> Command + Send + Sync>;

struct Worker {
    child: Child,
    stdin: std::process::ChildStdin,
    lines: mpsc::Receiver<String>,
}

fn start_worker(cmd: &WorkerCommand) -> Worker {
    let mut c = cmd();
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = c.spawn().expect("spawn worker");
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if line.starts_with("R\t") && tx.send(line).is_err() {
                break;
            }
        }
    });
    Worker { child, stdin, lines: rx }
}

/// Run `tests` on `threads` worker subprocesses. `wall_limit` bounds one
/// test's wall-clock time (the worker is killed past it). `progress` is
/// called after each result.
pub fn run_parallel(tests: Vec<String>, threads: usize, wall_limit: Duration, cmd: WorkerCommand, progress: &(dyn Fn(usize, &Outcome) + Sync)) -> Vec<Outcome> {
    let queue = Arc::new(Mutex::new(tests.into_iter().collect::<VecDeque<_>>()));
    let results = Mutex::new(Vec::new());
    let done = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            let queue = queue.clone();
            let cmd = cmd.clone();
            let results = &results;
            let done = &done;
            s.spawn(move || {
                let mut worker: Option<Worker> = None;
                loop {
                    let Some(rel) = queue.lock().unwrap().pop_front() else { break };
                    let w = worker.get_or_insert_with(|| start_worker(&cmd));
                    let outcome = if writeln!(w.stdin, "{rel}").and_then(|_| w.stdin.flush()).is_err() {
                        None
                    } else {
                        match w.lines.recv_timeout(wall_limit) {
                            Ok(line) => {
                                let mut parts = line.splitn(4, '\t');
                                parts.next();
                                let path = parts.next().unwrap_or("").to_string();
                                let status = parts.next().and_then(Status::parse).unwrap_or(Status::Crash);
                                let message = parts.next().unwrap_or("").to_string();
                                Some(Outcome { path, status, message })
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                let mut w = worker.take().unwrap();
                                let _ = w.child.kill();
                                let _ = w.child.wait();
                                Some(Outcome { path: rel.clone(), status: Status::Timeout, message: format!("wall-clock limit {:?} (worker killed)", wall_limit) })
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => None,
                        }
                    };
                    let outcome = outcome.unwrap_or_else(|| {
                        // The worker died: the in-flight test crashed it.
                        let mut w = worker.take().unwrap();
                        let _ = w.child.kill();
                        let st = w.child.wait().ok();
                        Outcome { path: rel.clone(), status: Status::Crash, message: format!("worker died ({})", st.map(|s| s.to_string()).unwrap_or_default()) }
                    });
                    let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    progress(n, &outcome);
                    results.lock().unwrap().push(outcome);
                }
                if let Some(mut w) = worker {
                    drop(w.stdin);
                    let _ = w.child.wait();
                }
            });
        }
    });
    let mut r = results.into_inner().unwrap();
    r.sort_by(|a, b| a.path.cmp(&b.path));
    r
}

// ------------------------------------------------------------ discovery

/// All test files under `<suite>/test/<sub>` (relative to `test/`),
/// excluding fixtures, sorted.
pub fn discover(suite: &Path, subs: &[String]) -> Vec<String> {
    let root = suite.join("test");
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = if subs.is_empty() { vec![root.clone()] } else { subs.iter().map(|s| root.join(s)).collect() };
    while let Some(p) = stack.pop() {
        if p.is_file() {
            if let Ok(r) = p.strip_prefix(&root) {
                out.push(r.to_string_lossy().to_string());
            }
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&p) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                stack.push(path);
            } else if name.ends_with(".js") && !name.contains("_FIXTURE") {
                if let Ok(r) = path.strip_prefix(&root) {
                    out.push(r.to_string_lossy().to_string());
                }
            }
        }
    }
    out.retain(|r| !r.starts_with("harness/") && !r.contains("_FIXTURE"));
    out.sort();
    out.dedup();
    out
}

// -------------------------------------------------------------- reports

/// Directory bucket used in tables: `built-ins/Array`, `language/expressions`, ...
pub fn bucket(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    parts.iter().take(2.min(parts.len().saturating_sub(1)).max(1)).cloned().collect::<Vec<_>>().join("/")
}

#[derive(Default, Clone, Copy, Debug)]
pub struct Counts {
    pub pass: usize,
    pub fail: usize,
    pub skip: usize,
}

impl Counts {
    pub fn executed(&self) -> usize {
        self.pass + self.fail
    }
    pub fn pct(&self) -> f64 {
        if self.executed() == 0 {
            0.0
        } else {
            100.0 * self.pass as f64 / self.executed() as f64
        }
    }
    fn add(&mut self, s: Status) {
        match s {
            Status::Pass => self.pass += 1,
            Status::Skip => self.skip += 1,
            _ => self.fail += 1,
        }
    }
}

pub fn tally(results: &[Outcome]) -> (Counts, BTreeMap<String, Counts>) {
    let mut total = Counts::default();
    let mut dirs: BTreeMap<String, Counts> = BTreeMap::new();
    for r in results {
        total.add(r.status);
        dirs.entry(bucket(&r.path)).or_default().add(r.status);
    }
    (total, dirs)
}

pub fn summary_text(results: &[Outcome], commit: &str) -> String {
    let (total, dirs) = tally(results);
    let mut s = String::new();
    let mut by_status: BTreeMap<&str, usize> = BTreeMap::new();
    for r in results {
        *by_status.entry(r.status.as_str()).or_default() += 1;
    }
    s.push_str(&format!("test262 {commit}\n"));
    s.push_str(&format!("total {} executed {} pass {} fail {} skip {} => {:.2}%\n", results.len(), total.executed(), total.pass, total.fail, total.skip, total.pct()));
    let mut no_intl = Counts::default();
    for r in results.iter().filter(|r| !r.path.starts_with("intl402/")) {
        no_intl.add(r.status);
    }
    s.push_str(&format!("without intl402: executed {} pass {} fail {} skip {} => {:.2}%\n", no_intl.executed(), no_intl.pass, no_intl.fail, no_intl.skip, no_intl.pct()));
    s.push_str(&format!("statuses: {by_status:?}\n"));
    let mut tops: BTreeMap<String, Counts> = BTreeMap::new();
    for (d, c) in &dirs {
        let t = tops.entry(d.split('/').next().unwrap_or("").to_string()).or_default();
        t.pass += c.pass;
        t.fail += c.fail;
        t.skip += c.skip;
    }
    s.push_str("\ntop-level\n");
    for (d, c) in &tops {
        s.push_str(&format!("{:<44} {:>6} {:>6} {:>6} {:>7.2}%\n", d, c.pass, c.fail, c.skip, c.pct()));
    }
    s.push_str(&format!("\n{:<44} {:>6} {:>6} {:>6} {:>8}\n", "directory", "pass", "fail", "skip", "pass%"));
    for (d, c) in &dirs {
        s.push_str(&format!("{:<44} {:>6} {:>6} {:>6} {:>7.2}%\n", d, c.pass, c.fail, c.skip, c.pct()));
    }
    s
}

/// Baseline file: commit, per-directory counts, failing test paths.
pub fn baseline_text(results: &[Outcome], commit: &str) -> String {
    let (total, dirs) = tally(results);
    let mut s = String::new();
    s.push_str("# RabbitHole test262 baseline (generated by `cargo run --release -p rabbithole-js --example test262 -- --write-baseline`)\n");
    s.push_str(&format!("commit {commit}\n"));
    s.push_str(&format!("total {} {} {} {:.2}\n", total.pass, total.fail, total.skip, total.pct()));
    for (d, c) in &dirs {
        s.push_str(&format!("dir {} {} {} {}\n", d, c.pass, c.fail, c.skip));
    }
    for r in results {
        if matches!(r.status, Status::Fail | Status::Timeout | Status::Crash) {
            s.push_str(&format!("fail {}\n", r.path));
        }
    }
    s
}

pub struct Baseline {
    pub commit: String,
    pub failing: std::collections::HashSet<String>,
    pub dirs: BTreeMap<String, Counts>,
}

pub fn read_baseline(text: &str) -> Baseline {
    let mut b = Baseline { commit: String::new(), failing: Default::default(), dirs: Default::default() };
    for line in text.lines() {
        if let Some(c) = line.strip_prefix("commit ") {
            b.commit = c.trim().to_string();
        } else if let Some(p) = line.strip_prefix("fail ") {
            b.failing.insert(p.trim().to_string());
        } else if let Some(d) = line.strip_prefix("dir ") {
            let f: Vec<&str> = d.split_whitespace().collect();
            if f.len() == 4 {
                b.dirs.insert(f[0].to_string(), Counts { pass: f[1].parse().unwrap_or(0), fail: f[2].parse().unwrap_or(0), skip: f[3].parse().unwrap_or(0) });
            }
        }
    }
    b
}

/// (regressions, fixes) relative to a baseline.
pub fn compare(results: &[Outcome], base: &Baseline) -> (Vec<Outcome>, Vec<String>) {
    let mut regressions = Vec::new();
    let mut fixes = Vec::new();
    for r in results {
        let failing = matches!(r.status, Status::Fail | Status::Timeout | Status::Crash);
        let was_failing = base.failing.contains(&r.path);
        if failing && !was_failing && base.dirs.contains_key(&bucket(&r.path)) {
            regressions.push(r.clone());
        } else if r.status == Status::Pass && was_failing {
            fixes.push(r.path.clone());
        }
    }
    (regressions, fixes)
}

/// The suite's git commit, if it is a checkout.
pub fn suite_commit(suite: &Path) -> String {
    Command::new("git")
        .arg("-C")
        .arg(suite)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}
