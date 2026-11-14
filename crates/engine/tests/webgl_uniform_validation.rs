use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Viewport};

#[test]
fn uniform_validation_keeps_both_canvas_images_red_after_rejected_updates() {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.loader.load_images = false;
    let mut page = engine.load_html(
        include_str!("../../../tests/pages/webgl-uniform-validation.html"),
        Url::parse("https://example.test/").unwrap(),
        Viewport { width: 640., height: 480. },
    );
    engine.restyle(&mut page);
    let console = page.take_console();
    assert!(!console.iter().any(|(level, _)| level == "error"), "{console:?}");
    assert_eq!(console.iter().filter(|(_, text)| text.ends_with("uniform validation ok")).count(), 2, "{console:?}");
    for id in ["uniform1", "uniform2"] {
        let image = page.images.get(&page.doc.element_by_id(id).unwrap()).unwrap();
        assert_eq!(image.pixels(), &[255, 0, 0, 255], "{id}");
    }
}
