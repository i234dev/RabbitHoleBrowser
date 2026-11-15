//! Real rendering control for named-line ordinals, with a Block-only oracle.
use rabbithole_engine::{reftest::{self, Outcome}, Engine};
use std::path::PathBuf;
use std::time::Duration;

#[test]
fn auto_fit_named_line_occurrences_match_independent_block_geometry() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/grid-named-lines/auto-fit-named-ordinals.html");
    let source = std::fs::read_to_string(&path).expect("fixture");
    let url = format!("file://{}", path.to_string_lossy().replace('\\', "/"));
    let mut engine = Engine::new_bundled_fonts_only();
    engine.scripting = false;
    engine.loader.load_scripts = false;
    let dump = std::env::temp_dir().join("rabbithole-grid-named-line-ordinals");
    let outcome = reftest::run_test(&mut engine, &url, &source, Duration::from_secs(20), Some(&dump));
    assert_eq!(outcome, Outcome::Pass, "images in {}", dump.display());
}
