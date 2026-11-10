//! Acid2 reftest: the test page scrolled to `#top` must match the
//! reference rendering pixel for pixel.
//!
//! `fixtures/acid2/` holds unmodified copies of the Web Standards Project's
//! Acid2 test (<http://acid2.acidtests.org/>: `test.html`, `reference.html`,
//! `reference.png`), freely redistributable for testing. The one external
//! resource of the test (`http://www.damowmow.com/404/`, an `<object>` of
//! type `text/html`) is never fetched: non-image objects render their
//! fallback content.

use rabbithole_engine::diff::{diff_frames, diff_frames_image, DiffOptions, Frame};
use rabbithole_engine::{Engine, Viewport};
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/acid2").join(name)
}

fn file_url(name: &str, fragment: &str) -> String {
    format!("file://{}{fragment}", fixture(name).to_string_lossy().replace('\\', "/"))
}

fn render(engine: &mut Engine, url: &str) -> Frame {
    engine.scripting = false;
    engine.loader.load_scripts = false;
    engine.render_url(url, Viewport { width: 800.0, height: 600.0 }).expect("load")
}

/// On failure, write the frames and a visual diff next to the test binary
/// output for inspection.
fn dump_failure(name: &str, a: &Frame, b: &Frame) -> String {
    let dir = std::env::temp_dir().join("rabbithole-acid2");
    let _ = std::fs::create_dir_all(&dir);
    let _ = a.save_png(&dir.join(format!("{name}-test.png")));
    let _ = b.save_png(&dir.join(format!("{name}-reference.png")));
    let (_, img) = diff_frames_image(a, b, &DiffOptions::default());
    let _ = img.save_png(&dir.join(format!("{name}-diff.png")));
    dir.display().to_string()
}

#[test]
fn acid2_matches_reference_page() {
    let mut engine = Engine::new_bundled_fonts_only();
    let test = render(&mut engine, &file_url("test.html", "#top"));
    let reference = render(&mut engine, &file_url("reference.html", ""));
    let r = diff_frames(&test, &reference, &DiffOptions::default());
    if !r.identical() {
        let dir = dump_failure("page", &test, &reference);
        panic!("Acid2 differs from the reference page: {} px ({:.3}%), regions {:?} (images in {dir})", r.mismatched, r.percent(), r.regions);
    }
}

#[test]
fn acid2_face_matches_reference_png() {
    let mut engine = Engine::new_bundled_fonts_only();
    let test = render(&mut engine, &file_url("test.html", "#top"));
    let reference = Frame::load_png(&fixture("reference.png")).expect("reference.png");
    // The reference page shows reference.png at (72, 108).
    let face = test.crop(72, 108, reference.width, reference.height);
    let r = diff_frames(&face, &reference, &DiffOptions::default());
    if !r.identical() {
        let dir = dump_failure("face", &face, &reference);
        panic!("Acid2 face differs from reference.png: {} px ({:.3}%), regions {:?} (images in {dir})", r.mismatched, r.percent(), r.regions);
    }
}
