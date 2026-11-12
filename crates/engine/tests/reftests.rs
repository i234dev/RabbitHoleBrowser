//! Reftests in the WPT format: our own fixtures (`fixtures/reftests/`,
//! always run) and a small subset of web-platform-tests known to pass
//! (`wpt-subset.txt`), run when a WPT checkout is present
//! (`$RH_WPT_DIR`, default `/mnt/hdd5tb/rabbithole/suites/wpt`). The full
//! WPT run is `examples/wpt_reftest.rs`.

use rabbithole_engine::reftest::{self, Outcome};
use rabbithole_engine::Engine;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/reftests")
}

fn file_url(p: &Path) -> String {
    format!("file://{}", p.to_string_lossy().replace('\\', "/"))
}

fn run_dir_tests(engine: &mut Engine, tests: &[(String, String)], dump: &Path) -> Vec<String> {
    let mut failures = Vec::new();
    for (name, url) in tests {
        let path = url.strip_prefix("file://").map(PathBuf::from);
        let src = match &path {
            Some(p) => std::fs::read_to_string(p).unwrap_or_default(),
            None => String::new(),
        };
        let outcome = reftest::run_test(engine, url, &src, Duration::from_secs(5), Some(&dump.join(name)));
        if outcome != Outcome::Pass {
            failures.push(format!("{name}: {outcome:?}"));
        }
    }
    failures
}

#[test]
fn own_reftests_pass() {
    let mut tests: Vec<(String, String)> = std::fs::read_dir(fixtures())
        .expect("fixtures")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "html") && !p.file_name().unwrap().to_string_lossy().ends_with("-ref.html"))
        .map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), file_url(&p)))
        .collect();
    tests.sort();
    assert!(!tests.is_empty());
    let mut engine = Engine::new_bundled_fonts_only();
    engine.scripting = false;
    engine.loader.load_scripts = false;
    let dump = std::env::temp_dir().join("rabbithole-reftests");
    let failures = run_dir_tests(&mut engine, &tests, &dump);
    assert!(failures.is_empty(), "reftest failures (images in {}):\n{}", dump.display(), failures.join("\n"));
}

#[test]
fn wpt_subset_passes() {
    let root = PathBuf::from(std::env::var("RH_WPT_DIR").unwrap_or_else(|_| "/mnt/hdd5tb/rabbithole/suites/wpt".into()));
    if !root.join("css").is_dir() {
        eprintln!("no WPT checkout at {}: skipped", root.display());
        return;
    }
    let list = include_str!("wpt-subset.txt");
    let names: Vec<&str> = list.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
    let base = reftest::serve_dir(root.clone()).expect("serve");
    let mut engine = reftest::test_engine(&root);
    let dump = std::env::temp_dir().join("rabbithole-wpt-subset");
    let mut failures = Vec::new();
    for name in names {
        let Ok(src) = std::fs::read_to_string(root.join(name)) else { continue };
        let outcome = reftest::run_test(&mut engine, &format!("{base}{name}"), &src, Duration::from_secs(5), Some(&dump.join(name)));
        if outcome != Outcome::Pass {
            failures.push(format!("{name}: {outcome:?}"));
        }
    }
    assert!(failures.is_empty(), "WPT subset regressions (images in {}):\n{}", dump.display(), failures.join("\n"));
}
