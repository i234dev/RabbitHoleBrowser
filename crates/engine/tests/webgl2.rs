//! The experimental WebGL2 path must paint real GLSL300/VAO/uint32 geometry.
use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Viewport};
#[test]
fn webgl2_vao_uint32_glsl300_pixels_reach_page_frame() {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.loader.load_images = false;
    let mut page = engine.load_html(
        include_str!("../../../tests/pages/webgl2.html"),
        Url::parse("https://example.test/").unwrap(),
        Viewport {
            width: 200.,
            height: 160.,
        },
    );
    engine.restyle(&mut page);
    let frame = engine.snapshot(&page);
    for (x, expected) in [(40, [255, 0, 0, 255]), (120, [0, 255, 0, 255])] {
        let i = (48 * frame.width as usize + x) * 4;
        assert_eq!(&frame.rgba[i..i + 4], &expected);
    }
    let errors: Vec<_> = page
        .take_console()
        .into_iter()
        .filter(|(level, _)| level == "error")
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    if let Ok(path) = std::env::var("RH_WEBGL2_SCREENSHOT") {
        frame.save_png(std::path::Path::new(&path)).unwrap();
    }
}
