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
    let (green_count, green_bounds) = frame.rgba.chunks_exact(4).enumerate()
        .filter(|(_, pixel)| *pixel == [0,255,0,255])
        .fold((0usize, None::<(u32,u32,u32,u32)>), |(count, bounds), (index, _)| {
            let (x, y) = (index as u32 % frame.width, index as u32 / frame.width);
            (count + 1, Some(match bounds {
                Some((left, top, right, bottom)) => (left.min(x), top.min(y), right.max(x + 1), bottom.max(y + 1)),
                None => (x, y, x + 1, y + 1),
            }))
        });
    for (id, y) in [("gl1", 2), ("gl2", 6)] {
        let node = page.doc.element_by_id(id).unwrap();
        let start = (y * frame.width as usize + 2) * 4;
        let screenshot = if frame.rgba[start..start + 4] != [0,255,0,255] {
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir().join(format!("rh-webgl-idl-offsets-{}-{stamp}-{id}.png", std::process::id()));
            match frame.save_png(&path) {
                Ok(()) => format!("{}", path.display()),
                Err(error) => format!("{} (save failed: {error})", path.display()),
            }
        } else { "not saved: pixel passed".to_string() };
        assert_eq!(&frame.rgba[start..start + 4], [0,255,0,255],
            "{id}: expected pixel (2,{y}), rect {:?}, layout rect {:?}; viewport {:?}, frame {}x{}, scale {:?}, scroll origin {:?}; green pixels {green_count}, bounds {green_bounds:?}; failure PNG {screenshot}; display-list images {:?}",
            page.rect_of(node), page.layout_rect_of(node),
            page.viewport, frame.width, frame.height,
            (frame.width as f32 / page.viewport.width, frame.height as f32 / page.viewport.height), page.page_scroll,
            page.display_list.items.iter().filter_map(|item| match item {
                rabbithole_engine::layout::DisplayItem::Image { rect, src, image, .. } => Some((*rect, *src, image.width, image.height)),
                _ => None,
            }).collect::<Vec<_>>());
    }
}
