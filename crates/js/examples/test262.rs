//! test262 conformance runner.
//!
//! ```text
//! cargo run --release -p rabbithole-js --example test262 -- [options] [subdirs or files...]
//!   --suite DIR            test262 checkout (default $TEST262_DIR or /mnt/hdd5tb/rabbithole/suites/test262)
//!   -j N                   worker processes (default: available cores)
//!   --timeout SECS         per-mode CPU budget inside the engine (default 10)
//!   --wall SECS            per-test wall-clock limit; the worker is killed past it (default 120)
//!   --out DIR              write summary.txt, results.txt, failures.txt there (default target dir)
//!   --compare FILE         list regressions against a baseline; exit 1 if any
//!   --write-baseline FILE  write a baseline (commit, per-directory counts, failing paths)
//!   --single PATH          run one test in-process and print the outcome
//! ```
//! Subdirectories/files are relative to the suite's `test/` directory
//! (e.g. `built-ins/Array language/statements/class`); none means all.
#[global_allocator]
static ALLOC: rabbithole_js::alloc::RhAlloc = rabbithole_js::alloc::RhAlloc;

#[path = "../tests/test262_support/mod.rs"]
mod support;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::*;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut suite = default_suite();
    let mut threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
    let mut timeout = 10.0f64;
    let mut wall = 120.0f64;
    let mut out: Option<PathBuf> = None;
    let mut compare_file: Option<PathBuf> = None;
    let mut write_baseline: Option<PathBuf> = None;
    let mut single: Option<String> = None;
    let mut worker = false;
    let mut subs = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let mut val = || {
            i += 1;
            args.get(i).cloned().unwrap_or_else(|| panic!("{a} needs a value"))
        };
        match a {
            "--suite" => suite = PathBuf::from(val()),
            "-j" => threads = val().parse().expect("-j N"),
            "--timeout" => timeout = val().parse().expect("--timeout SECS"),
            "--wall" => wall = val().parse().expect("--wall SECS"),
            "--out" => out = Some(PathBuf::from(val())),
            "--compare" => compare_file = Some(PathBuf::from(val())),
            "--write-baseline" => write_baseline = Some(PathBuf::from(val())),
            "--single" => single = Some(val()),
            "--worker" => worker = true,
            _ => subs.push(a.trim_start_matches("test/").to_string()),
        }
        i += 1;
    }
    let deadline = Duration::from_secs_f64(timeout);
    if worker {
        worker_main(suite, deadline);
        return;
    }
    if let Some(rel) = single {
        let rel = rel.trim_start_matches("test/").to_string();
        let h = std::thread::Builder::new().stack_size(TEST_STACK).spawn(move || run_test(&suite, &rel, deadline)).unwrap();
        let o = h.join().expect("test panicked");
        println!("{} {} {}", o.status.as_str(), o.path, o.message);
        return;
    }
    let tests = discover(&suite, &subs);
    let commit = suite_commit(&suite);
    eprintln!("test262 {} ({}): {} files, {} workers", suite.display(), commit, tests.len(), threads);
    let exe = std::env::current_exe().expect("current exe");
    let suite_arg = suite.clone();
    let cmd: WorkerCommand = Arc::new(move || {
        let mut c = std::process::Command::new(&exe);
        c.arg("--worker").arg("--suite").arg(&suite_arg).arg("--timeout").arg(timeout.to_string());
        c
    });
    let total = tests.len();
    let start = Instant::now();
    let results = run_parallel(tests, threads, Duration::from_secs_f64(wall), cmd, &|n, o| {
        if matches!(o.status, Status::Crash | Status::Timeout) {
            eprintln!("{} {} {}", o.status.as_str(), o.path, o.message);
        }
        if n % 1000 == 0 || n == total {
            eprintln!("[{n}/{total}] {:.0}s", start.elapsed().as_secs_f64());
        }
    });
    let summary = summary_text(&results, &commit);
    println!("{summary}");
    let out = out.unwrap_or_else(|| {
        let t = std::env::var("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("target"));
        t.join("test262")
    });
    let _ = std::fs::create_dir_all(&out);
    let _ = std::fs::write(out.join("summary.txt"), &summary);
    let mut all = String::new();
    let mut failures = String::new();
    for r in &results {
        all.push_str(&format!("{}\t{}\t{}\n", r.status.as_str(), r.path, r.message));
        if matches!(r.status, Status::Fail | Status::Timeout | Status::Crash) {
            failures.push_str(&format!("{}\t{}\t{}\n", r.status.as_str(), r.path, r.message));
        }
    }
    let _ = std::fs::write(out.join("results.txt"), all);
    let _ = std::fs::write(out.join("failures.txt"), failures);
    eprintln!("wrote {}/{{summary,results,failures}}.txt in {:.0}s", out.display(), start.elapsed().as_secs_f64());
    if let Some(b) = write_baseline {
        std::fs::write(&b, baseline_text(&results, &commit)).expect("write baseline");
        eprintln!("wrote baseline {}", b.display());
    }
    if let Some(c) = compare_file {
        let text = std::fs::read_to_string(&c).expect("read baseline");
        let base = read_baseline(&text);
        if base.commit != commit {
            eprintln!("warning: baseline is for test262 {}, suite is {}", base.commit, commit);
        }
        let (regressions, fixes) = compare(&results, &base);
        println!("{} newly passing, {} regressions vs {}", fixes.len(), regressions.len(), c.display());
        for r in &regressions {
            println!("REGRESSION {} {} {}", r.status.as_str(), r.path, r.message);
        }
        if !regressions.is_empty() {
            std::process::exit(1);
        }
    }
}
