//! WPT runs against the committed baseline (`tools/wpt/baseline.txt`).
//!
//! The fast subset runs by default when a checkout exists (see
//! `rabbithole_wpt::default_root`, override with `RH_WPT_ROOT`); the full
//! run is `#[ignore]`d: `cargo test --release -p rabbithole-wpt -- --ignored`.

use std::process::Command;

/// Small, quick pages spanning the main areas (DOM tree, events, parsing,
/// URL, encoding, timers).
const FAST_SUBSET: &[&str] = &[
    "dom/nodes/Node-appendChild.html",
    "dom/nodes/Node-insertBefore.html",
    "dom/nodes/Node-removeChild.html",
    "dom/nodes/Element-classlist.html",
    "dom/events/Event-dispatch-bubbles-false.html",
    "dom/events/EventTarget-dispatchEvent.html",
    "domparsing/DOMParser-parseFromString-html.html",
    "url/urlsearchparams-append.any.js",
    "encoding/textdecoder-labels.any.js",
    "html/webappapis/timers/negative-settimeout.any.js",
];

fn run(paths: &[&str]) {
    let root = rabbithole_wpt::default_root();
    if !root.join("resources/testharness.js").is_file() {
        eprintln!("skipping: no WPT checkout at {}", root.display());
        return;
    }
    let out = Command::new(env!("CARGO_BIN_EXE_rh-wpt"))
        .arg("run")
        .args(paths)
        .args(["--compare", "--quiet", "-j", "1"])
        .output()
        .expect("run rh-wpt");
    let stdout = String::from_utf8_lossy(&out.stdout);
    eprintln!("{stdout}");
    assert!(out.status.success(), "WPT regressions against the baseline:\n{stdout}");
}

#[test]
fn fast_subset_has_no_regressions() {
    run(FAST_SUBSET);
}

#[test]
#[ignore]
fn full_run_has_no_regressions() {
    run(&[]);
}
