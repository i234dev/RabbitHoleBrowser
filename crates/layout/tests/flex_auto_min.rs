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

#[test]
fn preferred_row_width_only_caps_the_content_based_minimum() {
    let (doc, tree) = fixture(r#"
      <section><div id=large style="width:100px"><div></div></div></section>
      <section><div id=small style="width:50px"><div></div></div></section>
      <section><div id=capped style="width:100px;max-width:60px"><div></div></div></section>
      <section><div id=basis style="width:100px;flex-basis:10px"><div></div></div></section>
    "#, "section{display:flex;width:1px}section>div>div{width:80px;height:20px}");
    near(by_id(&doc, &tree, "large").content_rect.w, 80.);
    near(by_id(&doc, &tree, "small").content_rect.w, 50.);
    near(by_id(&doc, &tree, "capped").content_rect.w, 60.);
    near(by_id(&doc, &tree, "basis").content_rect.w, 80.);
}

#[test]
fn preferred_column_height_only_caps_the_content_based_minimum() {
    let (doc, tree) = fixture(r#"
      <section><div id=large style="height:100px"><div></div></div></section>
      <section><div id=small style="height:50px"><div></div></div></section>
      <section><div id=capped style="height:100px;max-height:60px"><div></div></div></section>
      <section><div id=basis style="height:100px;flex-basis:10px"><div></div></div></section>
    "#, "section{display:flex;flex-direction:column;width:20px;height:1px}section>div>div{width:20px;height:80px}");
    near(by_id(&doc, &tree, "large").content_rect.h, 80.);
    near(by_id(&doc, &tree, "small").content_rect.h, 50.);
    near(by_id(&doc, &tree, "capped").content_rect.h, 60.);
    near(by_id(&doc, &tree, "basis").content_rect.h, 80.);
}

#[test]
fn content_flex_bases_ignore_own_maximum_even_after_shrink_to_fit_measurement() {
    let (doc, tree) = fixture(r#"
      <section><div class=capped id=first><div></div></div><div id=second><div></div></div></section>
      <aside><section><div class=capped id=cached-first><div></div></div><div id=cached-second><div></div></div></section></aside>
    "#, "section{display:flex;width:300px}section>div{min-width:0;height:20px}section>div>div{width:300px;height:20px}.capped{max-width:100px}aside{float:left;max-width:300px}aside section{width:auto}");
    for id in ["first", "cached-first"] { near(by_id(&doc, &tree, id).content_rect.w, 100.); }
    for id in ["second", "cached-second"] { near(by_id(&doc, &tree, id).content_rect.w, 200.); }
}

#[test]
fn column_content_flex_bases_ignore_own_maximum_until_flexing() {
    let (doc, tree) = fixture(r#"
      <section><div id=capped style="max-height:100px"><div></div></div><div id=other><div></div></div></section>
    "#, "section{display:flex;flex-direction:column;width:20px;height:300px}section>div{min-height:0}section>div>div{width:20px;height:300px}");
    near(by_id(&doc, &tree, "capped").content_rect.h, 100.);
    near(by_id(&doc, &tree, "other").content_rect.h, 200.);
}

#[test]
fn content_flex_bases_ignore_own_minimum_until_hypothetical_sizing() {
    let (doc, tree) = fixture(r#"
      <section><div id=minimum style="min-width:200px"><div style="width:80px"></div></div><div id=other><div style="width:40px"></div></div></section>
    "#, "section{display:flex;width:400px}section>div{flex:1 1 content;min-width:0}section>div>div{height:20px}");
    near(by_id(&doc, &tree, "minimum").content_rect.w, 220.);
    near(by_id(&doc, &tree, "other").content_rect.w, 180.);
}

#[test]
fn an_empty_zero_basis_item_does_not_inherit_its_preferred_main_size() {
    let (doc, tree) = fixture(r#"
      <section><div id=row style="width:100px;height:20px;flex-basis:0%"></div></section>
      <section class=column><div id=column style="width:20px;height:100px;flex-basis:0"></div></section>
    "#, "section{display:flex;width:100px;height:100px}.column{flex-direction:column}");
    near(by_id(&doc, &tree, "row").content_rect.w, 0.);
    near(by_id(&doc, &tree, "column").content_rect.h, 0.);
}

#[test]
fn percentage_descendant_width_uses_the_final_flexed_main_size() {
    let (doc, tree) = fixture(r#"
      <section><div id=item><div id=percent></div></div></section>
    "#, "section{display:flex;width:50px}#item{width:100px}#percent{width:100%;height:20px}");
    near(by_id(&doc, &tree, "item").content_rect.w, 50.);
    near(by_id(&doc, &tree, "percent").content_rect.w, 50.);
}

#[test]
fn percentage_descendant_height_is_relaid_out_after_indefinite_content_measurement() {
    let (doc, tree) = fixture(r#"
      <section><div id=item><div id=percent></div></div></section>
    "#, "section{display:flex;flex-direction:column;width:20px;height:50px}#item{height:100px}#percent{height:100%;width:20px}");
    near(by_id(&doc, &tree, "item").content_rect.h, 50.);
    near(by_id(&doc, &tree, "percent").content_rect.h, 50.);
}
