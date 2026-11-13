//! The GL feature branches must compose through actual page rendering.
//! Requires both framebuffer and instancing implementations to be integrated.
use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Viewport};

#[test]
fn framebuffer_instancing_vao_uint32_texture_sampling_reach_page_frame() {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.loader.load_images = false;
    let mut page = engine.load_html(
        include_str!("../../../tests/pages/webgl-pipeline.html"),
        Url::parse("https://example.test/pipeline").unwrap(),
        Viewport {
            width: 640.,
            height: 420.,
        },
    );
    engine.restyle(&mut page);
    let console = page.take_console();
    let errors: Vec<_> = console
        .iter()
        .filter(|(level, _)| level == "error")
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(
        console
            .iter()
            .any(|(_, line)| line
                == "webgl-pipeline: framebuffer, instancing and presentation passed"),
        "{console:?}"
    );
    let pipeline = page.doc.element_by_id("pipeline").unwrap();
    let before = page.doc.element_by_id("default-before").unwrap();
    let pixels = page
        .images
        .get(&pipeline)
        .expect("Sampled GL texture must be presented")
        .pixels();
    assert_eq!(pixels.len(), 4 * 2 * 4);
    for y in 0..2 {
        for x in 0..4 {
            let expected = if x < 2 {
                [255, 0, 0, 255]
            } else {
                [0, 255, 0, 255]
            };
            assert_eq!(&pixels[(y * 4 + x) * 4..(y * 4 + x + 1) * 4], &expected);
        }
    }
    let blue = page
        .images
        .get(&before)
        .expect("Default canvas snapshot must exist")
        .pixels();
    assert_eq!(blue.len(), 4 * 2 * 4);
    assert!(blue.chunks_exact(4).all(|pixel| pixel == [0, 0, 255, 255]));

    // Check the painted frame as well as the native canvas pixel store.
    let frame = engine.snapshot(&page);
    for (node, fraction, expected) in [
        (pipeline, 0.25, [255, 0, 0, 255]),
        (pipeline, 0.75, [0, 255, 0, 255]),
        (before, 0.5, [0, 0, 255, 255]),
    ] {
        let rect = page.rect_of(node).unwrap();
        let x = (rect.x + rect.w * fraction) as usize;
        let y = (rect.y + rect.h * 0.5) as usize;
        assert!(x < frame.width as usize && y < frame.height as usize);
        let offset = (y * frame.width as usize + x) * 4;
        assert_eq!(&frame.rgba[offset..offset + 4], &expected);
    }
    if let Ok(path) = std::env::var("RH_WEBGL_PIPELINE_SCREENSHOT") {
        frame.save_png(std::path::Path::new(&path)).unwrap();
    }
}
