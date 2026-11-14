use rabbithole_engine::{net::Url, Engine, Viewport};

#[test]
fn converted_webgl_offsets_are_visible_in_both_canvas_versions() {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.loader.load_images = false;
    let mut page = engine.load_html(
        include_str!("../../../tests/pages/webgl-idl-offsets.html"),
        Url::parse("https://example.test/").unwrap(),
        Viewport { width: 16., height: 12. },
    );
    engine.restyle(&mut page);
    let errors: Vec<_> = page.take_console().into_iter().filter(|(level, _)| level == "error").collect();
    assert!(errors.is_empty(), "{errors:?}");
    for id in ["gl1", "gl2"] {
        let image = page.images.get(&page.doc.element_by_id(id).unwrap()).unwrap();
        assert!(image.pixels().chunks_exact(4).all(|pixel| pixel == [0,255,0,255]), "{id}");
    }
    let frame = engine.snapshot(&page);
    for y in [2, 6] {
        let start = (y * frame.width as usize + 2) * 4;
        assert_eq!(&frame.rgba[start..start + 4], [0,255,0,255]);
    }
    if let Ok(path) = std::env::var("RH_WEBGL_OFFSETS_SCREENSHOT") {
        frame.save_png(std::path::Path::new(&path)).unwrap();
    }
}
