//! web-platform-tests reftest runner.
//!
//! ```text
//! wpt_reftest [--wpt DIR] [--jobs N] [--timeout SECS] [--mem-kb KB] [--out FILE]
//!             [--baseline FILE [--update-baseline | --compare]] [--filter SUBSTR]
//!             [--list FILE] [DIR ...]
//! ```
//!
//! Serves the WPT checkout (default `$RH_WPT_DIR`, else
//! `/mnt/hdd5tb/rabbithole/suites/wpt`) over HTTP on localhost, finds the
//! reftests under each `DIR` (relative to the checkout, default a list of
//! `css/` areas), runs them in worker processes (each test with a timeout;
//! a crashed or hung worker is replaced) and prints the pass rate per
//! directory (`--list` runs the test paths listed in a file instead,
//! one per line; a baseline's `fail` lines work). `--out` writes every result as TSV. `--baseline` with
//! `--update-baseline` stores the counts and failing tests; with
//! `--compare` it reports tests that passed in the baseline and fail now
//! (exit status 1 on regressions).
//!
//! `RH_REFTEST_DUMP=DIR` makes workers save test/reference/diff PNGs of
//! failures under `DIR/<test path>/`.

use rabbithole_engine::reftest;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

const DEFAULT_DIRS: &[&str] = &[
    "css/CSS2", "css/css-align", "css/css-backgrounds", "css/css-borders", "css/css-box", "css/css-cascade", "css/css-color", "css/css-conditional",
    "css/css-contain", "css/css-display", "css/css-flexbox", "css/css-fonts", "css/css-grid", "css/css-images", "css/css-masking", "css/css-nesting",
    "css/css-overflow", "css/css-position", "css/css-sizing", "css/css-text", "css/css-text-decor", "css/css-transforms", "css/css-ui", "css/css-values",
    "css/css-writing-modes", "css/compositing", "css/filter-effects", "css/selectors",
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--worker") {
        worker(&args[2], Path::new(&args[3]));
        return;
    }
    let mut wpt = std::env::var("RH_WPT_DIR").unwrap_or_else(|_| "/mnt/hdd5tb/rabbithole/suites/wpt".into());
    let mut jobs = 1usize;
    let mut timeout = 20u64;
    let mut mem_kb = 2_500_000u64;
    let mut out: Option<String> = None;
    let mut baseline: Option<String> = None;
    let mut update = false;
    let mut compare = false;
    let mut filter: Option<String> = None;
    let mut list: Option<String> = None;
    let mut dirs: Vec<String> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        let a = args[i].as_str();
        let mut val = || {
            i += 1;
            args.get(i).cloned().unwrap_or_default()
        };
        match a {
            "--wpt" => wpt = val(),
            "--jobs" => jobs = val().parse().unwrap_or(1).max(1),
            "--timeout" => timeout = val().parse().unwrap_or(20),
            "--mem-kb" => mem_kb = val().parse().unwrap_or(mem_kb),
            "--out" => out = Some(val()),
            "--baseline" => baseline = Some(val()),
            "--filter" => filter = Some(val()),
            "--list" => list = Some(val()),
            "--update-baseline" => update = true,
            "--compare" => compare = true,
            _ => dirs.push(a.trim_end_matches('/').to_string()),
        }
        i += 1;
    }
    if dirs.is_empty() {
        dirs = DEFAULT_DIRS.iter().map(|s| s.to_string()).collect();
    }
    let root = PathBuf::from(&wpt);
    let base = reftest::serve_dir(root.clone()).expect("bind localhost");

    // Collect tests.
    let mut tests: Vec<(String, String)> = Vec::new(); // (group, path)
    if let Some(l) = &list {
        // Explicit test paths (a baseline's `fail …` lines work too).
        let text = std::fs::read_to_string(l).expect("read --list file");
        for line in text.lines() {
            let t = line.trim().trim_start_matches("fail ").trim();
            if t.is_empty() || t.starts_with('#') || t.starts_with("dir ") || filter.as_ref().is_some_and(|f| !t.contains(f.as_str())) {
                continue;
            }
            let group = t.splitn(3, '/').take(2).collect::<Vec<_>>().join("/");
            tests.push((group, t.to_string()));
        }
        dirs = tests.iter().map(|(g, _)| g.clone()).collect::<BTreeSet<_>>().into_iter().collect();
    }
    for d in dirs.iter().filter(|_| list.is_none()) {
        for t in reftest::find_tests(&root, d) {
            if filter.as_ref().is_some_and(|f| !t.contains(f.as_str())) {
                continue;
            }
            tests.push((d.clone(), t));
        }
    }
    eprintln!("{} reftests in {} directories; {jobs} worker(s), {timeout}s timeout", tests.len(), dirs.len());

    // Run them.
    let (res_tx, res_rx) = mpsc::channel::<(usize, String, String)>();
    let queue = std::sync::Arc::new(std::sync::Mutex::new((0..tests.len()).collect::<std::collections::VecDeque<usize>>()));
    let paths: std::sync::Arc<Vec<String>> = std::sync::Arc::new(tests.iter().map(|(_, p)| p.clone()).collect());
    let exe = std::env::current_exe().expect("own path");
    let mut handles = Vec::new();
    for _ in 0..jobs {
        let queue = queue.clone();
        let paths = paths.clone();
        let res_tx = res_tx.clone();
        let exe = exe.clone();
        let base = base.clone();
        let root = root.clone();
        handles.push(std::thread::spawn(move || {
            let mut w: Option<Worker> = None;
            loop {
                let Some(idx) = queue.lock().unwrap().pop_front() else { break };
                let path = &paths[idx];
                if w.is_none() {
                    w = Worker::spawn(&exe, &base, &root, mem_kb);
                }
                let Some(wk) = w.as_mut() else {
                    let _ = res_tx.send((idx, "CRASH".into(), "cannot start worker".into()));
                    continue;
                };
                if writeln!(wk.stdin, "{path}").and_then(|_| wk.stdin.flush()).is_err() {
                    let _ = res_tx.send((idx, "CRASH".into(), "worker gone".into()));
                    w = None;
                    continue;
                }
                match wk.lines.recv_timeout(Duration::from_secs(timeout)) {
                    Ok(line) => {
                        let mut parts = line.splitn(3, '\t');
                        let _ = parts.next();
                        let status = parts.next().unwrap_or("ERROR").to_string();
                        let detail = parts.next().unwrap_or("").to_string();
                        let _ = res_tx.send((idx, status, detail));
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        let _ = res_tx.send((idx, "TIMEOUT".into(), String::new()));
                        if let Some(mut wk) = w.take() {
                            let _ = wk.child.kill();
                            let _ = wk.child.wait();
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        let _ = res_tx.send((idx, "CRASH".into(), String::new()));
                        if let Some(mut wk) = w.take() {
                            let _ = wk.child.wait();
                        }
                    }
                }
            }
            if let Some(mut wk) = w {
                drop(wk.stdin);
                let _ = wk.child.wait();
            }
        }));
    }
    drop(res_tx);
    let started = Instant::now();
    let mut results: Vec<Option<(String, String)>> = vec![None; tests.len()];
    let mut done = 0;
    // Partial results as they arrive (`<out>.partial`).
    let mut partial = out.as_ref().and_then(|o| std::fs::File::create(format!("{o}.partial")).ok());
    for (idx, status, detail) in res_rx {
        done += 1;
        if let Some(f) = partial.as_mut() {
            let _ = writeln!(f, "{}\t{status}\t{detail}", tests[idx].1);
        }
        if done % 200 == 0 {
            eprintln!("  {done}/{} ({:.0}s)", tests.len(), started.elapsed().as_secs_f32());
        }
        results[idx] = Some((status, detail));
    }
    for h in handles {
        let _ = h.join();
    }

    // Report.
    let mut per_dir: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut failing: BTreeSet<String> = BTreeSet::new();
    let mut lines = String::new();
    for ((group, path), r) in tests.iter().zip(results.iter()) {
        let (status, detail) = r.clone().unwrap_or(("CRASH".into(), String::new()));
        lines.push_str(&format!("{path}\t{status}\t{detail}\n"));
        if status == "SKIP" {
            continue;
        }
        let e = per_dir.entry(group.clone()).or_default();
        e.1 += 1;
        if status == "PASS" {
            e.0 += 1;
        } else {
            failing.insert(path.clone());
        }
    }
    let (mut tp, mut tt) = (0, 0);
    println!("{:<28} {:>6} {:>6} {:>7}", "directory", "pass", "total", "%");
    for (d, (p, t)) in &per_dir {
        tp += p;
        tt += t;
        println!("{:<28} {:>6} {:>6} {:>6.1}%", d, p, t, pct(*p, *t));
    }
    println!("{:<28} {:>6} {:>6} {:>6.1}%", "TOTAL", tp, tt, pct(tp, tt));
    if let Some(o) = &out {
        let _ = std::fs::write(o, lines);
    }
    if let Some(b) = &baseline {
        if update {
            let mut s = String::from("# WPT reftest baseline: per-directory pass/total, then failing tests.\n");
            for (d, (p, t)) in &per_dir {
                s.push_str(&format!("dir {d} {p} {t}\n"));
            }
            for f in &failing {
                s.push_str(&format!("fail {f}\n"));
            }
            std::fs::write(b, s).expect("write baseline");
            eprintln!("baseline written to {b}");
        } else if compare {
            let text = std::fs::read_to_string(b).expect("read baseline");
            let mut base_dirs: BTreeMap<String, (usize, usize)> = BTreeMap::new();
            let mut base_fail = BTreeSet::new();
            for l in text.lines() {
                let mut it = l.split_whitespace();
                match it.next() {
                    Some("dir") => {
                        let d = it.next().unwrap_or("").to_string();
                        let p = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
                        let t = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
                        base_dirs.insert(d, (p, t));
                    }
                    Some("fail") => {
                        base_fail.insert(it.next().unwrap_or("").to_string());
                    }
                    _ => {}
                }
            }
            println!("\n{:<28} {:>8} {:>8}", "directory", "before", "after");
            for (d, (p, t)) in &per_dir {
                if let Some((bp, bt)) = base_dirs.get(d) {
                    println!("{:<28} {:>7.1}% {:>7.1}%", d, pct(*bp, *bt), pct(*p, *t));
                }
            }
            let regressions: Vec<&String> = tests
                .iter()
                .filter(|(g, p)| base_dirs.contains_key(g) && failing.contains(p) && !base_fail.contains(p))
                .map(|(_, p)| p)
                .collect();
            let fixed = base_fail.iter().filter(|p| paths.contains(p) && !failing.contains(*p)).count();
            println!("\n{fixed} newly passing, {} regressions", regressions.len());
            for r in &regressions {
                println!("REGRESSION {r}");
            }
            if !regressions.is_empty() {
                std::process::exit(1);
            }
        }
    }
}

fn pct(p: usize, t: usize) -> f64 {
    if t == 0 {
        0.0
    } else {
        p as f64 * 100.0 / t as f64
    }
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

impl Worker {
    fn spawn(exe: &Path, base: &str, root: &Path, mem_kb: u64) -> Option<Worker> {
        let mut cmd = if cfg!(unix) && mem_kb > 0 {
            // Bound the worker's address space (`ulimit -v`).
            let mut c = Command::new("sh");
            c.arg("-c").arg(format!("ulimit -v {mem_kb}; exec \"$0\" --worker \"$1\" \"$2\"")).arg(exe).arg(base).arg(root);
            c
        } else {
            let mut c = Command::new(exe);
            c.arg("--worker").arg(base).arg(root);
            c
        };
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).env("RH_HTTP_CACHE", "0");
        let mut child = cmd.spawn().ok()?;
        let stdin = child.stdin.take()?;
        let stdout = child.stdout.take()?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for l in BufReader::new(stdout).lines() {
                let Ok(l) = l else { break };
                if l.starts_with("RESULT\t") && tx.send(l).is_err() {
                    break;
                }
            }
        });
        Some(Worker { child, stdin, lines: rx })
    }
}

/// Worker process: read test paths from stdin, print one `RESULT` line per
/// test.
fn worker(base: &str, root: &Path) {
    let dump = std::env::var("RH_REFTEST_DUMP").ok().map(PathBuf::from);
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    let mut engine = reftest::test_engine(root);
    let mut count = 0;
    for line in stdin.lock().lines() {
        let Ok(path) = line else { break };
        let path = path.trim().to_string();
        if path.is_empty() {
            continue;
        }
        count += 1;
        if count % 50 == 0 {
            // A fresh engine now and then (caches, registered web fonts).
            engine = reftest::test_engine(root);
        }
        engine.scripting = true;
        let src = std::fs::read_to_string(root.join(&path)).unwrap_or_default();
        let url = format!("{base}{path}");
        let dump_dir = dump.as_ref().map(|d| d.join(&path));
        let outcome = reftest::run_test(&mut engine, &url, &src, Duration::from_secs(5), dump_dir.as_deref());
        let (status, detail) = match outcome {
            reftest::Outcome::Pass => ("PASS", String::new()),
            reftest::Outcome::Fail { pixels, max_delta } => ("FAIL", format!("{pixels}px maxdiff {max_delta}")),
            reftest::Outcome::Skip(why) => ("SKIP", why),
            reftest::Outcome::Error(e) => ("ERROR", e.replace(['\t', '\n'], " ")),
        };
        let _ = writeln!(out, "RESULT\t{status}\t{detail}");
        let _ = out.flush();
    }
}
