//! `rh-wpt`: run web-platform-tests testharness.js tests in RabbitHole.
//!
//! ```text
//! rh-wpt run [PATH...] [--root DIR] [-j N] [--timeout MS] [--depth N]
//!            [--baseline FILE] [--update-baseline] [--compare]
//!            [--out FILE.jsonl] [--filter SUBSTR] [--memory-kb N] [--verbose] [--list]
//! rh-wpt summary [--baseline FILE] [--depth N]
//! rh-wpt serve [--root DIR]
//! rh-wpt child URL TIMEOUT_MS [--verbose]
//! ```

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use rabbithole_wpt::discover::{discover, TestId};
use rabbithole_wpt::report::{self, BaselineEntry};
use rabbithole_wpt::runner::{self, RunOptions, TestResult};
use rabbithole_wpt::server::Server;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(|s| s.as_str()) {
        Some("child") => child(&args[1..]),
        Some("run") => run(&args[1..]),
        Some("serve") => serve(&args[1..]),
        Some("summary") => summary(&args[1..]),
        _ => {
            eprintln!("usage: rh-wpt run|summary|serve|child ... (see crates/wpt/src/main.rs)");
            2
        }
    };
    std::process::exit(code);
}

struct Flags {
    positional: Vec<String>,
    values: Vec<(String, String)>,
    switches: Vec<String>,
}

impl Flags {
    fn parse(args: &[String], with_value: &[&str]) -> Flags {
        let mut f = Flags { positional: Vec::new(), values: Vec::new(), switches: Vec::new() };
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if let Some(name) = a.strip_prefix("--").or_else(|| a.strip_prefix('-').filter(|n| n.len() == 1)) {
                if with_value.contains(&name) && i + 1 < args.len() {
                    f.values.push((name.to_string(), args[i + 1].clone()));
                    i += 2;
                    continue;
                }
                f.switches.push(name.to_string());
            } else {
                f.positional.push(a.clone());
            }
            i += 1;
        }
        f
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.values.iter().rev().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    fn has(&self, name: &str) -> bool {
        self.switches.iter().any(|s| s == name)
    }

    fn num<T: std::str::FromStr>(&self, name: &str, default: T) -> T {
        self.value(name).and_then(|v| v.parse().ok()).unwrap_or(default)
    }
}

fn child(args: &[String]) -> i32 {
    let f = Flags::parse(args, &[]);
    let (Some(url), Some(ms)) = (f.positional.first(), f.positional.get(1)) else {
        eprintln!("usage: rh-wpt child URL TIMEOUT_MS");
        return 2;
    };
    let ms: u64 = ms.parse().unwrap_or(10_000);
    let code = runner::run_child(url, Duration::from_millis(ms), f.has("verbose"));
    let _ = std::io::stdout().flush();
    code
}

fn serve(args: &[String]) -> i32 {
    let f = Flags::parse(args, &["root"]);
    let root = f.value("root").map(PathBuf::from).unwrap_or_else(rabbithole_wpt::default_root);
    let server = match Server::start(&root) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot start server: {e}");
            return 1;
        }
    };
    println!("serving {} at {} (second port {})", root.display(), server.base(), server.config.ports[1]);
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn summary(args: &[String]) -> i32 {
    let f = Flags::parse(args, &["baseline", "depth"]);
    let path = f.value("baseline").map(PathBuf::from).unwrap_or_else(rabbithole_wpt::default_baseline);
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!("cannot read {}", path.display());
        return 1;
    };
    print!("{}", report::summary_table(&report::parse_baseline(&text), f.num("depth", 2)));
    0
}

fn run(args: &[String]) -> i32 {
    let f = Flags::parse(args, &["root", "j", "jobs", "timeout", "long-timeout", "depth", "baseline", "out", "filter", "memory-kb"]);
    let root = f.value("root").map(PathBuf::from).unwrap_or_else(rabbithole_wpt::default_root);
    if !root.join("resources/testharness.js").is_file() {
        eprintln!("no WPT checkout at {} (set --root or RH_WPT_ROOT)", root.display());
        return 1;
    }
    let dirs: Vec<String> = if f.positional.is_empty() {
        rabbithole_wpt::DEFAULT_DIRS.iter().map(|s| s.to_string()).collect()
    } else {
        f.positional.clone()
    };
    let mut tests: Vec<TestId> = dirs.iter().flat_map(|d| discover(&root, d)).collect();
    if let Some(filter) = f.value("filter") {
        tests.retain(|t| t.url.contains(filter));
    }
    tests.sort();
    tests.dedup();
    if f.has("list") {
        for t in &tests {
            println!("{}{}", t.url, if t.long { "\t(long)" } else { "" });
        }
        return 0;
    }
    let server = match Server::start(&root) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot start server: {e}");
            return 1;
        }
    };
    let timeout = f.num("timeout", 6000u64);
    let opts = RunOptions {
        jobs: f.value("j").or(f.value("jobs")).and_then(|v| v.parse().ok()).unwrap_or(1),
        timeout: Duration::from_millis(timeout),
        long_timeout: Duration::from_millis(f.num("long-timeout", timeout * 3)),
        verbose: f.has("verbose"),
        memory_kb: f.num("memory-kb", 2_500_000u64),
    };
    let base = server.base();
    eprintln!("running {} tests from {} at {} with {} jobs", tests.len(), root.display(), base, opts.jobs);
    let quiet = f.has("quiet");
    let started = std::time::Instant::now();
    let results: Vec<TestResult> = runner::run_all(&base, &tests, &opts, &|n, total, r| {
        if quiet {
            return;
        }
        let (p, t) = r.counts();
        eprintln!("[{n}/{total}] {:<9} {:>4}/{:<4} {:>6}ms {}", r.harness, p, t, r.ms, r.url);
    });
    eprintln!("finished in {:.0}s", started.elapsed().as_secs_f64());
    if let Some(out) = f.value("out") {
        let mut s = String::new();
        for r in &results {
            let v = serde_json::json!({
                "url": r.url, "harness": r.harness, "message": r.message, "ms": r.ms,
                "subtests": r.subtests.iter().map(|s| serde_json::json!({"name": s.name, "status": s.status, "message": s.message})).collect::<Vec<_>>(),
            });
            s.push_str(&v.to_string());
            s.push('\n');
        }
        if let Err(e) = std::fs::write(out, s) {
            eprintln!("cannot write {out}: {e}");
        }
    }
    let entries: Vec<BaselineEntry> = results.iter().map(BaselineEntry::from_result).collect();
    let depth = f.num("depth", 2);
    print!("{}", report::summary_table(&entries, depth));
    let baseline_path = f.value("baseline").map(PathBuf::from).unwrap_or_else(rabbithole_wpt::default_baseline);
    let mut code = 0;
    if f.has("compare") {
        let old = std::fs::read_to_string(&baseline_path).map(|t| report::parse_baseline(&t)).unwrap_or_default();
        let c = report::compare(&old, &entries);
        println!(
            "\ncompared with {}: {}/{} ({:.2}%) -> {}/{} ({:.2}%) on the {} pages in both",
            baseline_path.display(),
            c.before.passed,
            c.before.total,
            c.before.pct(),
            c.after.passed,
            c.after.total,
            c.after.pct(),
            c.after.files
        );
        println!("fixed subtests: {}", c.fixes.len());
        if !c.regressions.is_empty() || !c.count_drops.is_empty() {
            println!("REGRESSIONS ({} subtests, {} pages with fewer passes):", c.regressions.len(), c.count_drops.len());
            for (url, name) in c.regressions.iter().take(200) {
                println!("  {url}  ::  {name}");
            }
            for (url, a, b) in &c.count_drops {
                println!("  {url}: {a} -> {b} passing");
            }
            code = 1;
        } else {
            println!("no regressions");
        }
    }
    if f.has("update-baseline") {
        // Merge: pages run now replace their old entries, others stay.
        let old = std::fs::read_to_string(&baseline_path).map(|t| report::parse_baseline(&t)).unwrap_or_default();
        let ran: std::collections::HashSet<&str> = entries.iter().map(|e| e.url.as_str()).collect();
        let mut merged: Vec<BaselineEntry> = old.into_iter().filter(|e| !ran.contains(e.url.as_str())).collect();
        merged.extend(entries.iter().cloned());
        if let Some(parent) = baseline_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&baseline_path, report::write_baseline(&merged)) {
            eprintln!("cannot write {}: {e}", baseline_path.display());
            return 1;
        }
        let summary_path = baseline_path.with_file_name("summary.txt");
        let _ = std::fs::write(&summary_path, report::summary_table(&merged, depth));
        eprintln!("baseline updated: {} ({} pages)", baseline_path.display(), merged.len());
    }
    code
}
