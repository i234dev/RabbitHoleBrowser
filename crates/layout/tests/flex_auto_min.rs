//! Flex automatic minima depend on scrollable overflow values, not clipping.
use std::collections::HashMap;

use rabbithole_css::{parse_stylesheet, MediaEnv};
use rabbithole_dom::Document;
use rabbithole_gfx::FontContext;
use rabbithole_layout::{layout, BoxKind, LayoutBox, LayoutContext, LayoutTree};
use rabbithole_style::style_tree;

fn fixture(html: &str, css: &str) -> (Document, LayoutTree) {
    let doc = rabbithole_html::parse(html);
    let styles = style_tree(&doc, &[parse_stylesheet(css)], &MediaEnv {
        width: 800., height: 600., screen: true, scripting: true,
    });
    let fonts = FontContext::new();
    let images = HashMap::new();
    let background_images = HashMap::new();
    let scroll_offsets = HashMap::new();
    let tree = layout(&LayoutContext {
        doc: &doc, styles: &styles, fonts: &fonts, images: &images,
        background_images: &background_images, scroll_offsets: &scroll_offsets,
        viewport_width: 800., viewport_height: 600., page_scroll: (0., 0.),
    });
    (doc, tree)
}

fn by_id<'a>(doc: &Document, tree: &'a LayoutTree, id: &str) -> &'a LayoutBox {
    fn find(b: &LayoutBox, node: usize) -> Option<&LayoutBox> {
        if b.node == Some(node) && b.kind != BoxKind::Text { return Some(b); }
        b.children.iter().find_map(|child| find(child, node))
    }
    find(&tree.root, doc.element_by_id(id).unwrap()).unwrap()
}

fn near(actual: f32, expected: f32) {
    assert!((actual - expected).abs() < 0.01, "actual={actual}, expected={expected}");
}

#[test]
fn non_scrollable_overflow_keeps_the_flex_row_content_minimum() {
    let (doc, tree) = fixture(r#"
      <section><div id=visible><div></div></div></section>
      <section><div id=clip style="overflow:clip"><div></div></div></section>
      <section><div id=clipx style="overflow-x:clip;overflow-y:visible"><div></div></div></section>
      <section><div id=clipy style="overflow-x:visible;overflow-y:clip"><div></div></div></section>
      <section><div id=hidden style="overflow:hidden"><div></div></div></section>
      <section><div id=explicit style="overflow:clip;min-width:0"><div></div></div></section>
    "#, "section{display:flex;width:100px}section>div>div{width:150px;height:20px}");
    for id in ["visible", "clip", "clipx", "clipy"] { near(by_id(&doc, &tree, id).content_rect.w, 150.); }
    for id in ["hidden", "explicit"] { near(by_id(&doc, &tree, id).content_rect.w, 100.); }
}

#[test]
fn non_scrollable_overflow_keeps_the_flex_column_content_minimum() {
    let (doc, tree) = fixture(r#"
      <section><div id=visible><div></div></div></section>
      <section><div id=clip style="overflow:clip"><div></div></div></section>
      <section><div id=hidden style="overflow:hidden"><div></div></div></section>
      <section><div id=explicit style="overflow:clip;min-height:0"><div></div></div></section>
    "#, "section{display:flex;flex-direction:column;width:100px;height:100px}section>div>div{width:20px;height:150px}");
    for id in ["visible", "clip"] { near(by_id(&doc, &tree, id).content_rect.h, 150.); }
    for id in ["hidden", "explicit"] { near(by_id(&doc, &tree, id).content_rect.h, 100.); }
}

#[test]
fn a_scrollable_value_on_the_other_axis_still_disables_the_automatic_minimum() {
    let (doc, tree) = fixture(r#"
      <section><div id=x style="overflow-x:hidden;overflow-y:clip"><div></div></div></section>
      <section><div id=y style="overflow-x:clip;overflow-y:hidden"><div></div></div></section>
    "#, "section{display:flex;width:100px}section>div>div{width:150px;height:20px}");
    near(by_id(&doc, &tree, "x").content_rect.w, 100.);
    near(by_id(&doc, &tree, "y").content_rect.w, 100.);
}
