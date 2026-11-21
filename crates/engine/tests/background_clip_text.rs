//! `background-clip: text` on inline boxes: the background shows only
//! through the element's glyphs, not as a filled rectangle (gradient
//! headings written as `<h1>The <span class=accent>word</span></h1>`).

use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Viewport};

/// Number of (nearly) pure green pixels of the rendered page.
fn green_pixels(css: &str) -> usize {
    let mut e = Engine::new_bundled_fonts_only();
    let html = format!("<!doctype html><style>body {{ margin: 0; font: 80px/1 sans-serif }} .a {{ {css} }}</style><div>Ab <span class=a>MWMW</span> cd</div>");
    let page = e.load_html(&html, Url::parse("https://example.test/").unwrap(), Viewport { width: 600.0, height: 200.0 });
    let f = e.snapshot(&page);
    f.rgba.chunks_exact(4).filter(|p| p[0] < 40 && p[1] > 100 && p[2] < 40).count()
}

#[test]
fn inline_background_clip_text_paints_only_glyphs() {
    // Unclipped, the span's box is filled.
    let filled = green_pixels("background: green; color: transparent");
    assert!(filled > 5000, "unclipped background fills the box: {filled}");
    for css in [
        "background: green; background-clip: text; color: transparent",
        "-webkit-text-fill-color: transparent; background: linear-gradient(green, green); -webkit-background-clip: text; background-clip: text",
    ] {
        let clipped = green_pixels(css);
        assert!(clipped * 20 > filled && clipped * 10 < filled * 8, "{css}: only the glyphs are green ({clipped} of {filled})");
    }
}
