//! Multi-column layout: column geometry, balancing, breaks, spanners and
//! fragments.

use crate::tree::{BoxKind, LayoutBox, LayoutTree};
use crate::{layout, LayoutContext};
use rabbithole_css::{parse_stylesheet, MediaEnv};
use rabbithole_dom::Document;
use rabbithole_gfx::{FontContext, Rect};
use rabbithole_style::{style_tree, StyledTree};
use std::collections::HashMap;

struct Fx {
    doc: Document,
    styles: StyledTree,
    fonts: FontContext,
}

impl Fx {
    fn new(html: &str) -> Fx {
        let doc = rabbithole_html::parse(html);
        let env = MediaEnv { width: 800.0, height: 600.0, screen: true, scripting: true };
        let styles = style_tree(&doc, &[parse_stylesheet("body { margin: 0 }")], &env);
        Fx { doc, styles, fonts: FontContext::new() }
    }

    fn layout(&self) -> LayoutTree {
        let (images, bgs, scrolls) = (HashMap::new(), HashMap::new(), HashMap::new());
        let ctx = LayoutContext {
            doc: &self.doc,
            styles: &self.styles,
            fonts: &self.fonts,
            images: &images,
            background_images: &bgs,
            viewport_width: 800.0,
            viewport_height: 600.0,
            scroll_offsets: &scrolls,
            page_scroll: (0.0, 0.0),
        };
        layout(&ctx)
    }

    fn find<'t>(&self, tree: &'t LayoutTree, id: &str) -> &'t LayoutBox {
        let node = self.doc.element_by_id(id).unwrap_or_else(|| panic!("no #{id}"));
        fn find(b: &LayoutBox, n: usize) -> Option<&LayoutBox> {
            if b.node == Some(n) && b.kind != BoxKind::Text && b.pseudo.is_none() {
                return Some(b);
            }
            b.children.iter().find_map(|c| find(c, n))
        }
        find(&tree.root, node).unwrap_or_else(|| panic!("no box for #{id}\n{}", tree.dump()))
    }

    fn rect(&self, tree: &LayoutTree, id: &str) -> Rect {
        self.find(tree, id).rect
    }

    fn pieces(&self, tree: &LayoutTree, id: &str) -> Vec<Rect> {
        self.find(tree, id).col_pieces.as_ref().map(|p| p.pieces.iter().map(|p| p.rect).collect()).unwrap_or_default()
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

#[test]
fn blocks_are_balanced_into_columns() {
    let fx = Fx::new(r#"<div id=mc style="columns:3; column-gap:10px; width:320px">
        <div id=a style="height:50px"></div><div id=b style="height:50px"></div><div id=c style="height:50px"></div></div>"#);
    let tree = fx.layout();
    assert!(close(fx.rect(&tree, "mc").h, 50.0), "{}", tree.dump());
    let (a, b, c) = (fx.rect(&tree, "a"), fx.rect(&tree, "b"), fx.rect(&tree, "c"));
    assert!(close(a.x, 0.0) && close(a.y, 0.0) && close(a.w, 100.0));
    assert!(close(b.x, 110.0) && close(b.y, 0.0), "{b:?}");
    assert!(close(c.x, 220.0) && close(c.y, 0.0), "{c:?}");
}

#[test]
fn a_tall_block_is_broken_into_fragments() {
    let fx = Fx::new(r#"<div id=mc style="columns:2; column-gap:0; width:200px; height:100px; column-fill:auto">
        <div id=a style="height:150px; background:green"></div></div>"#);
    let tree = fx.layout();
    let pieces = fx.pieces(&tree, "a");
    assert_eq!(pieces.len(), 2, "{}", tree.dump());
    assert!(close(pieces[0].x, 0.0) && close(pieces[0].y, 0.0) && close(pieces[0].h, 100.0), "{pieces:?}");
    assert!(close(pieces[1].x, 100.0) && close(pieces[1].y, 0.0) && close(pieces[1].h, 50.0), "{pieces:?}");
}

#[test]
fn forced_breaks_and_rtl_columns() {
    let fx = Fx::new(r#"<div id=mc style="columns:2; column-gap:0; width:200px; direction:rtl">
        <div id=a style="height:10px"></div><div id=b style="height:10px; break-before:column"></div></div>"#);
    let tree = fx.layout();
    let (a, b) = (fx.rect(&tree, "a"), fx.rect(&tree, "b"));
    assert!(close(a.x, 100.0) && close(b.x, 0.0) && close(b.y, 0.0), "{a:?} {b:?}");
    assert!(close(fx.rect(&tree, "mc").h, 10.0));
}

#[test]
fn spanners_separate_column_sets() {
    let fx = Fx::new(r#"<div id=mc style="columns:2; column-gap:0; width:200px">
        <div id=a style="height:40px"></div><div id=s style="column-span:all; height:10px"></div>
        <div id=b style="height:20px"></div></div>"#);
    let tree = fx.layout();
    let s = fx.rect(&tree, "s");
    assert!(close(s.w, 200.0) && close(s.y, 20.0), "{s:?}\n{}", tree.dump());
    let b = fx.rect(&tree, "b");
    assert!(close(b.y, 30.0), "{b:?}");
    // 20 (balanced 40px) + 10 + 10 (balanced 20px).
    assert!(close(fx.rect(&tree, "mc").h, 40.0));
}

#[test]
fn column_rules_go_between_used_columns() {
    let fx = Fx::new(r#"<div id=mc style="columns:3; column-gap:10px; width:320px; column-rule:4px solid">
        <div style="height:90px"></div></div>"#);
    let tree = fx.layout();
    let rules = fx.find(&tree, "mc").col_rules.as_ref().map(|r| r.to_vec()).unwrap_or_default();
    assert_eq!(rules.len(), 2, "{rules:?}");
    assert!(close(rules[0].x, 103.0) && close(rules[0].w, 4.0) && close(rules[0].h, 30.0), "{rules:?}");
}
