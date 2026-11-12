//! test262 conformance.
//!
//! * `smoke` (default): runs the ~500 tests listed in `test262_smoke.txt`
//!   (all passing at the baseline) and fails on any that no longer pass.
//! * `full` (`#[ignore]`d): runs the whole suite in worker subprocesses and
//!   compares with `test262_baseline.txt`, failing on regressions:
//!   `cargo test --release -p rabbithole-js --test test262 -- --ignored full --nocapture`.
//!
//! Both need a test262 checkout at `$TEST262_DIR` (default
//! `/mnt/hdd5tb/rabbithole/suites/test262`); without one they only print a
//! note. The everyday runner with reports is `examples/test262.rs`.
#[global_allocator]
static ALLOC: rabbithole_js::alloc::RhAlloc = rabbithole_js::alloc::RhAlloc;

mod test262_support;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test262_support::*;

fn suite() -> Option<PathBuf> {
    let p = default_suite();
    if p.join("test").is_dir() && p.join("harness").is_dir() {
        Some(p)
    } else {
        eprintln!("test262 suite not found at {} (set TEST262_DIR); skipping", p.display());
        None
    }
}

fn manifest_file(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join(name)
}

#[test]
fn smoke() {
    let Some(suite) = suite() else { return };
    let list = std::fs::read_to_string(manifest_file("test262_smoke.txt")).expect("test262_smoke.txt");
    let tests: Vec<String> = list.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from).collect();
    let queue = Arc::new(Mutex::new(tests.clone()));
    let failures = Arc::new(Mutex::new(Vec::new()));
    quiet_panics();
    let mut handles = Vec::new();
    for _ in 0..3 {
        let (queue, failures, suite) = (queue.clone(), failures.clone(), suite.clone());
        handles.push(
            std::thread::Builder::new()
                .stack_size(TEST_STACK)
                .spawn(move || loop {
                    let Some(rel) = queue.lock().unwrap().pop() else { break };
                    if !suite.join("test").join(&rel).exists() {
                        continue;
                    }
                    let o = run_test_guarded(&suite, &rel, Duration::from_secs(20));
                    if !matches!(o.status, Status::Pass | Status::Skip) {
                        failures.lock().unwrap().push(format!("{} {} {}", o.status.as_str(), o.path, o.message));
                    }
                })
                .unwrap(),
        );
    }
    for h in handles {
        h.join().unwrap();
    }
    let failures = failures.lock().unwrap();
    assert!(failures.is_empty(), "{} of {} smoke tests failed:\n{}", failures.len(), tests.len(), failures.join("\n"));
}

/// Worker subprocess of `full` (a no-op unless spawned by it).
#[test]
fn worker() {
    if std::env::var_os("RH_T262_WORKER").is_none() {
        return;
    }
    let suite = default_suite();
    worker_main(suite, Duration::from_secs(10));
}

#[test]
#[ignore]
fn full() {
    let Some(suite) = suite() else { return };
    let tests = discover(&suite, &[]);
    let exe = std::env::current_exe().unwrap();
    let suite_env = suite.clone();
    let cmd: WorkerCommand = Arc::new(move || {
        let mut c = std::process::Command::new(&exe);
        c.args(["--exact", "worker", "--nocapture", "--test-threads=1"]).env("RH_T262_WORKER", "1").env("TEST262_DIR", &suite_env);
        c
    });
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
    let results = run_parallel(tests, threads, Duration::from_secs(120), cmd, &|_, _| {});
    let commit = suite_commit(&suite);
    println!("{}", summary_text(&results, &commit));
    let base = read_baseline(&std::fs::read_to_string(manifest_file("test262_baseline.txt")).expect("test262_baseline.txt"));
    let (regressions, fixes) = compare(&results, &base);
    println!("{} newly passing, {} regressions", fixes.len(), regressions.len());
    let lines: Vec<String> = regressions.iter().map(|r| format!("{} {} {}", r.status.as_str(), r.path, r.message)).collect();
    assert!(regressions.is_empty(), "test262 regressions:\n{}", lines.join("\n"));
}
