//! Inline `<svg>` sizing regressions.

use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Viewport};

/// An inline `<svg viewBox>` without width/height fills its container
/// (google.com's 24px app-launcher icon inside a 40px button rendered at
/// its 2x raster size, ~600px).
#[test]
fn svg_with_only_viewbox_fills_container() {
    let html = r#"<html><body style="margin:0">
        <a id="btn" href="/apps" style="display:inline-block;width:40px;height:40px;padding:8px;box-sizing:border-box">
          <svg id="icon" viewBox="0 0 24 24"><path d="M6,8c1.1,0 2,-0.9 2,-2s-0.9,-2 -2,-2 -2,0.9 -2,2 0.9,2 2,2z"/></svg></a>
        <div style="width:100px"><svg id="wide" viewBox="0 0 20 10"><rect width="20" height="10"/></svg></div>
        <svg id="sized" width="16" height="16" viewBox="0 0 24 24"><rect width="24" height="24"/></svg>
        </body></html>"#;
    let mut engine = Engine::new_bundled_fonts_only();
    let page = engine.load_html(html, Url::parse("https://example.test/").unwrap(), Viewport { width: 800.0, height: 600.0 });
    let rect = |id: &str| page.rect_of(page.doc.element_by_id(id).unwrap()).unwrap();
    let icon = rect("icon");
    assert!((icon.w - 24.0).abs() < 0.5 && (icon.h - 24.0).abs() < 0.5, "{icon:?}");
    let wide = rect("wide");
    assert!((wide.w - 100.0).abs() < 0.5 && (wide.h - 50.0).abs() < 0.5, "{wide:?}");
    let sized = rect("sized");
    assert!((sized.w - 16.0).abs() < 0.5 && (sized.h - 16.0).abs() < 0.5, "{sized:?}");
}
