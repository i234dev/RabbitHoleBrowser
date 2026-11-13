//! Bilinear texture filtering and blending must compose in an actual page.
use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Viewport};

#[test]
fn bilinear_texture_and_alpha_blending_reach_page_pixels() {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.loader.load_images = false;
    let mut page = engine.load_html(
        include_str!("../../../tests/pages/webgl-compositing.html"),
        Url::parse("https://example.test/").unwrap(),
        Viewport { width: 640., height: 540. },
    );
    engine.restyle(&mut page);
    let errors: Vec<_> = page.take_console().into_iter()
        .filter(|(level, _)| level == "error").collect();
    assert!(errors.is_empty(), "{errors:?}");
    let canvas = page.doc.element_by_id("composited").unwrap();
    let pixels = page.images.get(&canvas).expect("GL pixels must be presented").pixels();
    assert_eq!(pixels.len(), 64 * 64 * 4);
    let pixel = |x: usize, y: usize| &pixels[(y * 64 + x) * 4..(y * 64 + x + 1) * 4];
    // The top-center pixel lies above the triangle. A two-column texture
    // has a smooth midpoint here; nearest sampling would be black.
    let background = pixel(32, 3);
    assert!((120..=128).contains(&background[0]), "{background:?}");
    assert_eq!(&background[0..3], &[background[0]; 3]);
    assert_eq!(background[3], 255);
    // At the center, source-alpha blending mixes the red triangle with
    // that gray background while the separate alpha factors keep it opaque.
    let center = pixel(32, 32);
    assert!((185..=195).contains(&center[0]), "{center:?}");
    assert!((58..=66).contains(&center[1]), "{center:?}");
    assert_eq!(center[1], center[2]);
    assert_eq!(center[3], 255);
    assert_eq!(pixel(3, 3), &[255, 255, 255, 255]);
    assert_eq!(pixel(60, 3), &[0, 0, 0, 255]);
    if let Ok(path) = std::env::var("RH_WEBGL_COMPOSITING_SCREENSHOT") {
        engine.snapshot(&page).save_png(std::path::Path::new(&path)).unwrap();
    }
}
