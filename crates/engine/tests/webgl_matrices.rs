use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Viewport};

#[test]
fn matrix_uniform_updates_reach_both_page_canvases_without_cross_context_changes() {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.loader.load_images = false;
    let mut page = engine.load_html(
        include_str!("../../../tests/pages/webgl-matrices.html"),
        Url::parse("https://example.test/").unwrap(),
        Viewport { width: 640., height: 480. },
    );
    engine.restyle(&mut page);
    let errors: Vec<_> = page.take_console().into_iter().filter(|(level, _)| level == "error").collect();
    assert!(errors.is_empty(), "{errors:?}");
    for id in ["matrix1", "matrix2"] {
        let pixels = page.images.get(&page.doc.element_by_id(id).unwrap()).unwrap().pixels();
        let left = (2 * 8 + 1) * 4;
        let right = (2 * 8 + 6) * 4;
        assert_eq!(&pixels[left..left+4], &[0,0,255,255], "{id} left");
        assert_eq!(&pixels[right..right+4], &[128,64,0,255], "{id} right");
    }
}
