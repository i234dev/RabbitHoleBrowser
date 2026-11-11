//! `<object>` fallback: an object whose resource cannot be displayed
//! (unknown type, non-image document, failed load) renders its children;
//! one showing an image is a replaced element and hides its children.

use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Viewport};

const PNG_1X1: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR42mP4%2F58BAAT%2FAf9jgNErAAAAAElFTkSuQmCC";

#[test]
fn nested_object_fallback_chain() {
    let html = format!(
        r#"<!DOCTYPE html><html><body style="margin:0">
        <p><object id="a" data="data:application/x-unknown,ERROR"><object id="b" data="http://unreachable.invalid/404/" type="text/html"><object id="c" data="{PNG_1X1}">ERROR-C</object></object></object></p>
        <p><object id="d" data="data:application/x-shockwave-flash,xx" type="application/x-shockwave-flash"><span id="fb">fallback text</span></object></p>
        </body></html>"#
    );
    let mut engine = Engine::new_bundled_fonts_only();
    engine.scripting = false;
    let page = engine.load_html(&html, Url::parse("https://example.test/").unwrap(), Viewport { width: 800.0, height: 600.0 });
    let rect = |id: &str| page.rect_of(page.doc.element_by_id(id).unwrap()).unwrap();
    // The innermost object shows its image at its natural size.
    let c = rect("c");
    assert!((c.w - 1.0).abs() < 0.01 && (c.h - 1.0).abs() < 0.01, "{c:?}\n{}", page.layout_dump());
    // Its own fallback content is not rendered; the outer objects' is.
    let dump = page.layout_dump();
    assert!(!dump.contains("ERROR-C"), "{dump}");
    // The outer objects are plain inline boxes, not 300×150 placeholders.
    assert!(!dump.contains("300.0x150.0"), "{dump}");
    // Plugin content falls back to the children.
    assert!(dump.contains("fallback text"), "{dump}");
}
