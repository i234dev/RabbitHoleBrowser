//! Parse / compile throughput on real-world scripts:
//! `cargo run --release -p rabbithole-js --example parsebench [files...]`
//!
//! Without arguments it uses the framework bundles of the agent test
//! fixtures. Reports the best of several rounds of `parse_program` and of
//! `Realm::compile` (parse + early errors + top-level compile) over all files.
#[global_allocator]
static ALLOC: rabbithole_js::alloc::RhAlloc = rabbithole_js::alloc::RhAlloc;

use std::time::Instant;

const DEFAULT: &[&str] = &[
    "crates/agent/tests/fixtures/frameworks/vendor/react19/react19.production.js",
    "crates/agent/tests/fixtures/frameworks/vendor/vue/vue.global.prod.js",
    "crates/agent/tests/fixtures/frameworks/vendor/react18/react-dom.production.min.js",
    "crates/agent/tests/fixtures/frameworks/vendor/jquery/jquery.min.js",
    "crates/agent/tests/fixtures/frameworks/vendor/alpine/alpine.min.js",
    "crates/agent/tests/fixtures/frameworks/vendor/htmx/htmx.min.js",
    "crates/script/src/js/dom.js",
    "crates/script/src/js/html.js",
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let files: Vec<std::path::PathBuf> = if args.is_empty() { DEFAULT.iter().map(|f| root.join(f)).collect() } else { args.iter().map(Into::into).collect() };
    let sources: Vec<String> = files.iter().map(|f| std::fs::read_to_string(f).unwrap_or_else(|e| panic!("{}: {e}", f.display()))).collect();
    let bytes: usize = sources.iter().map(|s| s.len()).sum();
    let rounds = 15;
    let mut best_parse = f64::MAX;
    let mut best_compile = f64::MAX;
    for _ in 0..rounds {
        let t = Instant::now();
        for s in &sources {
            rabbithole_js::parse_program(s, false).expect("parse");
        }
        best_parse = best_parse.min(t.elapsed().as_secs_f64() * 1000.0);
        let mut realm = rabbithole_js::Realm::new();
        let t = Instant::now();
        for s in &sources {
            realm.compile(s, "bench", false).map_err(|_| "compile error").unwrap();
        }
        best_compile = best_compile.min(t.elapsed().as_secs_f64() * 1000.0);
    }
    println!("{} files, {} KB", sources.len(), bytes / 1024);
    println!("parse          {:>8.2} ms  ({:.1} MB/s)", best_parse, bytes as f64 / 1e6 / (best_parse / 1000.0));
    println!("parse+compile  {:>8.2} ms  ({:.1} MB/s)", best_compile, bytes as f64 / 1e6 / (best_compile / 1000.0));
}
