//! Real Engine gates for the isolated Grid axis consumer.
//! These fixtures stay outside the shipped own-reftest directory while the
//! consumer is incomplete. A failure is visible, not skipped or rebaselined.
use rabbithole_engine::{reftest::{self, Outcome}, Engine};
use std::path::PathBuf;
use std::time::Duration;

fn check(name:&str) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/grid-axis-consumer");
    let path = root.join(format!("{name}.html"));
    let source = std::fs::read_to_string(&path).expect("fixture");
    let url = format!("file://{}", path.to_string_lossy().replace('\\', "/"));
    let mut engine = Engine::new_bundled_fonts_only();
    engine.scripting = false;
    engine.loader.load_scripts = false;
    let dump = std::env::temp_dir().join("rabbithole-grid-axis-consumer").join(name);
    let outcome = reftest::run_test(&mut engine, &url, &source, Duration::from_secs(20), Some(&dump));
    assert_eq!(outcome, Outcome::Pass, "images in {}", dump.display());
}

#[test]
fn grid_orthogonal_items_use_their_real_perpendicular_track_span() { check("wrapper-definite-perpendicular"); }

#[test]
fn own_intrinsic_column_geometry_does_not_resolve_cyclic_percentages() { check("cyclic-inline-percentage"); }
