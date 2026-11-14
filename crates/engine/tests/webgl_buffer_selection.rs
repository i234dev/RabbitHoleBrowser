use rabbithole_engine::{net::Url, Engine, Viewport};

#[test]
fn draw_buffer_selection_reaches_page_pixels_without_presenting_disabled_writes() {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.loader.load_images = false;
    let mut page = engine.load_html(include_str!("../../../tests/pages/webgl-buffer-selection.html"), Url::parse("https://example.test/").unwrap(), Viewport { width: 200., height: 200. });
    engine.restyle(&mut page);
    let errors: Vec<_> = page.take_console().into_iter().filter(|(level, _)| level == "error").collect();
    assert!(errors.is_empty(), "{errors:?}");
    let pixels = page.images.get(&page.doc.element_by_id("visible").unwrap()).unwrap().pixels();
    for row in pixels.chunks_exact(8) {
        assert_eq!(&row[..4], [0,255,0,255]);
        assert_eq!(&row[4..], [0,0,255,255]);
    }
    let disabled = page.images.get(&page.doc.element_by_id("disabled").unwrap()).unwrap().pixels();
    assert!(disabled.chunks_exact(4).all(|p| p == [0,0,255,255]));
}
