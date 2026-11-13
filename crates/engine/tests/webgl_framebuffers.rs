//! Render-to-texture must reach page pixels without presenting offscreen data.
use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Viewport};

#[test]
fn render_to_texture_is_visible_and_offscreen_targets_do_not_replace_canvas() {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.loader.load_images = false;
    let mut page = engine.load_html(
        include_str!("../../../tests/pages/webgl-framebuffer.html"),
        Url::parse("https://example.test/").unwrap(),
        Viewport {
            width: 640.,
            height: 480.,
        },
    );
    engine.restyle(&mut page);
    let errors: Vec<_> = page
        .take_console()
        .into_iter()
        .filter(|(level, _)| level == "error")
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    let scene = page
        .images
        .get(&page.doc.element_by_id("scene").unwrap())
        .unwrap()
        .pixels();
    let offset = (32 * 64 + 32) * 4;
    assert_eq!(&scene[offset..offset + 4], &[255, 0, 0, 255]);
    assert_eq!(&scene[..4], &[0, 0, 0, 0]);
    let untouched = page
        .images
        .get(&page.doc.element_by_id("untouched").unwrap())
        .unwrap()
        .pixels();
    assert!(untouched.chunks_exact(4).all(|p| p == [0, 0, 255, 255]));
    if let Ok(path) = std::env::var("RH_WEBGL_FRAMEBUFFER_SCREENSHOT") {
        engine
            .snapshot(&page)
            .save_png(std::path::Path::new(&path))
            .unwrap();
    }
}
