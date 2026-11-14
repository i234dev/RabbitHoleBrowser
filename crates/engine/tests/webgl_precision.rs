use rabbithole_engine::{net::Url, Engine, Viewport};

#[test]
fn versioned_precision_headers_and_validation_present_real_canvas_pixels() {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.loader.load_images = false;
    let mut page = engine.load_html(include_str!("../../../tests/pages/webgl-precision-validation.html"), Url::parse("https://example.test/").unwrap(), Viewport { width: 16., height: 12. });
    engine.restyle(&mut page);
    let errors: Vec<_> = page.take_console().into_iter().filter(|(level, _)| level == "error").collect();
    assert!(errors.is_empty(), "{errors:?}");
    let frame = engine.snapshot(&page);
    for id in ["gl1", "gl2"] {
        let node = page.doc.element_by_id(id).unwrap();
        let image = page.images.get(&node).unwrap();
        assert!(image.pixels().chunks_exact(4).all(|pixel| pixel == [0,255,0,255]), "{id}");
        let rect = page.rect_of(node).unwrap();
        let offset = ((rect.y as usize + 2) * frame.width as usize + rect.x as usize + 2) * 4;
        assert_eq!(&frame.rgba[offset..offset+4], [0,255,0,255], "{id}");
    }
}
