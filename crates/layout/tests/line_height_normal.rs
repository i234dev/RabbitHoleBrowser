//! `line-height: normal` comes from the first available font's metrics
//! (CSS Inline 3 §5.1): ascent, descent and line gap, with the
//! `@font-face` metric overrides, plus the fonts that fallback uses.
use std::collections::HashMap;

use rabbithole_css::{parse_stylesheet, MediaEnv};
use rabbithole_dom::Document;
use rabbithole_gfx::{FontContext, MetricOverrides};
use rabbithole_layout::{layout, BoxKind, LayoutBox, LayoutContext, LayoutTree};
use rabbithole_style::style_tree;

const SANS: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSans.ttf");

fn fixture(html: &str, css: &str, fonts: &FontContext) -> (Document, LayoutTree) {
    let doc = rabbithole_html::parse(html);
    let styles = style_tree(&doc, &[parse_stylesheet(css)], &MediaEnv { width: 800., height: 600., screen: true, scripting: true });
    let images = HashMap::new();
    let background_images = HashMap::new();
    let scroll_offsets = HashMap::new();
    let tree = layout(&LayoutContext {
        doc: &doc,
        styles: &styles,
        fonts,
        images: &images,
        background_images: &background_images,
        scroll_offsets: &scroll_offsets,
        viewport_width: 800.,
        viewport_height: 600.,
        page_scroll: (0., 0.),
    });
    (doc, tree)
}

fn height(doc: &Document, tree: &LayoutTree, id: &str) -> f32 {
    fn find(b: &LayoutBox, node: usize) -> Option<&LayoutBox> {
        if b.node == Some(node) && b.kind != BoxKind::Text {
            return Some(b);
        }
        b.children.iter().find_map(|child| find(child, node))
    }
    find(&tree.root, doc.element_by_id(id).unwrap()).unwrap().content_rect.h
}

fn near(actual: f32, expected: f32) {
    assert!((actual - expected).abs() < 0.01, "actual={actual}, expected={expected}");
}

/// The bundled DejaVu faces: hhea ascender 1901, descender -483, no line
/// gap (2048 units per em), each rounded as Blink's line spacing.
#[test]
fn normal_line_height_uses_font_metrics() {
    let fonts = FontContext::new();
    let (doc, tree) = fixture(
        r#"<div id=a>Text</div><div id=b style="font-size:100px">Text</div><div id=c style="font-size:13px">Text<br>Text</div><div id=d style="line-height:1.2">Text</div>"#,
        "body{margin:0;font-family:sans-serif;font-size:16px}",
        &fonts,
    );
    near(height(&doc, &tree, "a"), 19.0);
    near(height(&doc, &tree, "b"), 93.0 + 24.0);
    near(height(&doc, &tree, "c"), 2.0 * 15.0);
    near(height(&doc, &tree, "d"), 19.2);
}

/// The metric overrides of a web font set its line height; an inline box
/// with that font raises the line by its own half-leading.
#[test]
fn metric_overrides_set_the_line_height() {
    let fonts = FontContext::new();
    let overrides = MetricOverrides { ascent: Some(1.0), descent: Some(0.5), line_gap: Some(0.25) };
    fonts.add_font_face_with_metrics("Overridden", (400, 400), false, (100, 100), None, overrides, SANS);
    let small = MetricOverrides { ascent: Some(0.25), descent: Some(0.125), line_gap: Some(0.125) };
    fonts.add_font_face_with_metrics("Small", (400, 400), false, (100, 100), None, small, SANS);
    let (doc, tree) = fixture(
        r#"<div id=a class=o>Text</div><div id=b>Text <span class=o>big</span></div><div id=c class=s>Text</div><div id=d class=o style="line-height:10px">Text</div>"#,
        "body{margin:0;font:16px sans-serif}.o{font:20px Overridden}.s{font:80px Small}",
        &fonts,
    );
    near(height(&doc, &tree, "a"), 20.0 + 10.0 + 5.0);
    // The strut (DejaVu, 16px) lies within the span's box: 22.5 above and
    // 12.5 below the baseline.
    near(height(&doc, &tree, "b"), 35.0);
    // Smaller than the font size.
    near(height(&doc, &tree, "c"), 20.0 + 10.0 + 10.0);
    near(height(&doc, &tree, "d"), 10.0);
}

/// Characters drawn by a fallback font add its metrics to the line, but
/// only with `line-height: normal`; a family whose `unicode-range` leaves
/// out the space is not the first available font.
#[test]
fn fallback_fonts_and_first_available_font() {
    let fonts = FontContext::new();
    let small = MetricOverrides { ascent: Some(0.4), descent: Some(0.1), line_gap: None };
    fonts.add_font_face_with_metrics("Latin", (400, 400), false, (100, 100), Some(&[(0x20, 0x7F)]), small, SANS);
    fonts.add_font_face_with_metrics("Letters", (400, 400), false, (100, 100), Some(&[(0x61, 0x7A)]), small, SANS);
    let (doc, tree) = fixture(
        r#"<div id=a>abc</div><div id=b>ab ж</div><div id=c style="line-height:10px">ab ж</div><div id=d class=l>abc</div>"#,
        "body{margin:0;font:20px Latin, sans-serif}.l{font-family:Letters, sans-serif}",
        &fonts,
    );
    near(height(&doc, &tree, "a"), 10.0);
    // DejaVu Sans at 20px for the Cyrillic letter: 19 + 5 with 0.36px
    // half-leading around its 18.56 + 4.72.
    near(height(&doc, &tree, "b"), 24.0);
    near(height(&doc, &tree, "c"), 10.0);
    // "Letters" has no space: the strut is DejaVu Sans (24px), and the
    // letters drawn with "Letters" count as fallback (they fit inside it).
    near(height(&doc, &tree, "d"), 24.0);
}
