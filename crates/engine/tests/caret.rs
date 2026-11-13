//! Caret geometry uses input-value byte offsets, not masked-text byte offsets.
use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Viewport};

fn setup() -> (Engine, rabbithole_engine::Page) {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.scripting = false;
    let page = engine.load_html(
        "<!doctype html><style>input{font:20px sans-serif;letter-spacing:3px}</style><input id=secret type=password placeholder=Password><input id=plain>",
        Url::parse("https://example.test/signin").unwrap(),
        Viewport { width: 640., height: 360. },
    );
    (engine, page)
}

#[test]
fn password_caret_after_typing_uses_mask_character_boundaries() {
    let (engine, mut page) = setup();
    let secret = page.doc.element_by_id("secret").unwrap();
    for value in ["a", "abc", "é😀a", "e\u{301}"] {
        page.set_control_text(secret, value);
        engine.restyle(&mut page);
        let run = page.text_runs().into_iter().find(|r| r.node == Some(secret)).unwrap();
        assert_eq!(&*run.text, "•".repeat(value.chars().count()));
        let start = page.caret_x(&engine.fonts, secret, 0).unwrap();
        for (count, (offset, _)) in value.char_indices().chain(std::iter::once((value.len(), ' '))).enumerate() {
            let x = page.caret_x(&engine.fonts, secret, offset).unwrap();
            let expected = engine.fonts.measure(run.font, run.size, &"•".repeat(count))
                + run.letter_spacing * count as f32;
            assert!((x - start - expected).abs() < 0.01, "value={value:?}, offset={offset}, x={x}");
        }
        // The layout and its normal painting path must remain usable after editing.
        let frame = engine.snapshot(&page);
        assert!(!frame.rgba.is_empty());
    }
    page.set_control_text(secret, "");
    engine.restyle(&mut page);
    assert!(page.caret_x(&engine.fonts, secret, 0).unwrap().is_finite());
}

#[test]
fn caret_clamps_stale_and_interior_unicode_offsets() {
    let (engine, mut page) = setup();
    let plain = page.doc.element_by_id("plain").unwrap();
    page.set_control_text(plain, "•");
    engine.restyle(&mut page);
    let start = page.caret_x(&engine.fonts, plain, 0).unwrap();
    assert_eq!(page.caret_x(&engine.fonts, plain, 1), Some(start));
    assert_eq!(page.caret_x(&engine.fonts, plain, 2), Some(start));
    let end = page.caret_x(&engine.fonts, plain, "•".len()).unwrap();
    assert!(end > start);
    assert_eq!(page.caret_x(&engine.fonts, plain, usize::MAX), Some(end));
    let secret = page.doc.element_by_id("secret").unwrap();
    page.set_control_text(secret, "é😀");
    engine.restyle(&mut page);
    let start = page.caret_x(&engine.fonts, secret, 0).unwrap();
    let after_first = page.caret_x(&engine.fonts, secret, 2).unwrap();
    assert_eq!(page.caret_x(&engine.fonts, secret, 1), Some(start));
    for offset in 3..6 {
        assert_eq!(page.caret_x(&engine.fonts, secret, offset), Some(after_first));
    }
    assert!(page.caret_x(&engine.fonts, secret, usize::MAX).unwrap() > after_first);
}
