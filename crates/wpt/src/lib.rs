//! Web-platform-tests runner for RabbitHole: a loopback wptserve subset
//! ([`server`]), test discovery ([`discover`]), per-test child processes
//! that load pages in the engine ([`runner`]) and pass-rate reports with a
//! committed baseline ([`report`]).

pub mod discover;
pub mod handlers;
pub mod report;
pub mod runner;
pub mod server;

/// Directories run by default (relative to the checkout root).
pub const DEFAULT_DIRS: &[&str] = &[
    "dom",
    "domparsing",
    "url",
    "encoding",
    "xhr",
    "fetch/api",
    "FileAPI",
    "streams",
    "webidl",
    "uievents",
    "pointerevents",
    "shadow-dom",
    "custom-elements",
    "selection",
    "IndexedDB",
    "webstorage",
    "webmessaging",
    "workers",
    "hr-time",
    "performance-timeline",
    "user-timing",
    "intersection-observer",
    "resize-observer",
    "css/cssom",
    "css/cssom-view",
    "html/syntax",
    "html/dom",
    "html/semantics/forms",
    "html/semantics/scripting-1",
    "html/browsers/history",
    "html/webappapis/timers",
    "html/webappapis/microtask-queuing",
    "html/webappapis/structured-clone",
];

/// Default checkout location (override with `--root` or `RH_WPT_ROOT`).
pub fn default_root() -> std::path::PathBuf {
    std::env::var_os("RH_WPT_ROOT").map(Into::into).unwrap_or_else(|| "/mnt/hdd5tb/rabbithole/suites/wpt-dom".into())
}

/// The committed baseline (`tools/wpt/baseline.txt` in the repository).
pub fn default_baseline() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/wpt/baseline.txt")
}
