//! CSS 2.1 conformance tests with minimal fixtures: margin collapsing,
//! percentage heights, clearance and tables.

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
    fn new(html: &str, css: &str) -> Fx {
        let doc = rabbithole_html::parse(html);
        let env = MediaEnv { width: 800.0, height: 600.0, screen: true, scripting: true };
        let styles = style_tree(&doc, &[parse_stylesheet(css)], &env);
        Fx { doc, styles, fonts: FontContext::new() }
    }

    fn layout(&self) -> LayoutTree {
        let images = HashMap::new();
        let bgs = HashMap::new();
        let scrolls = HashMap::new();
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

    /// Border box of the element with `id`.
    fn rect(&self, tree: &LayoutTree, id: &str) -> Rect {
        let node = self.doc.element_by_id(id).unwrap_or_else(|| panic!("no #{id}"));
        fn find(b: &LayoutBox, n: usize) -> Option<&LayoutBox> {
            if b.node == Some(n) && b.kind != BoxKind::Text && b.pseudo.is_none() {
                return Some(b);
            }
            b.children.iter().find_map(|c| find(c, n))
        }
        find(&tree.root, node).unwrap_or_else(|| panic!("no box for #{id}\n{}", tree.dump())).rect
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

macro_rules! assert_px {
    ($a:expr, $b:expr) => {
        assert!(close($a, $b), "{} = {} (expected {})", stringify!($a), $a, $b)
    };
    ($a:expr, $b:expr, $tree:expr) => {
        assert!(close($a, $b), "{} = {} (expected {})\n{}", stringify!($a), $a, $b, $tree.dump())
    };
}

const RESET: &str = "html, body { margin: 0; padding: 0; } div { display: block; }";

#[test]
fn sibling_margins_collapse_with_mixed_signs() {
    // Max of the positives plus min of the negatives, not pairwise.
    let fx = Fx::new(
        "<div id=a style='height:10px;margin-bottom:10px'></div><div id=e style='margin:-5px 0 8px'></div><div id=b style='height:10px'></div>",
        RESET,
    );
    let t = fx.layout();
    // Adjoining: 10, -5, 8 (empty #e collapses through) → 10 - 5 = 5.
    assert_px!(fx.rect(&t, "b").y, 15.0, t);
}

#[test]
fn empty_block_collapses_through_with_children() {
    let fx = Fx::new(
        "<div id=a style='height:10px;margin-bottom:48px'></div><div class=empty><div></div></div><div id=b style='height:10px;margin-top:20px'></div>",
        &format!("{RESET} .empty {{ margin: 75px; height: 10%; }} .empty div {{ margin: 0 24px -72px 48px; }}"),
    );
    let t = fx.layout();
    // 48, 75, 75, 0, -72, 20 all adjoin → 75 - 72 = 3.
    assert_px!(fx.rect(&t, "b").y, 13.0, t);
}

#[test]
fn percentage_height_in_auto_parent_is_auto() {
    let fx = Fx::new(
        "<div id=p><div id=c style='height:50%'><div style='height:30px'></div></div></div>",
        RESET,
    );
    let t = fx.layout();
    assert_px!(fx.rect(&t, "c").h, 30.0, t);
    // ...but resolves against a definite parent height.
    let fx = Fx::new("<div style='height:200px'><div id=c style='height:50%'></div></div>", RESET);
    let t = fx.layout();
    assert_px!(fx.rect(&t, "c").h, 100.0, t);
}

#[test]
fn parent_and_first_child_margins_collapse_unless_separated() {
    let fx = Fx::new(
        "<div id=p style='margin-top:10px'><div id=c style='margin-top:30px;height:5px'></div></div>\
         <div id=q style='margin-top:10px;padding-top:1px'><div id=d style='margin-top:30px;height:5px'></div></div>",
        RESET,
    );
    let t = fx.layout();
    assert_px!(fx.rect(&t, "p").y, 30.0, t);
    assert_px!(fx.rect(&t, "c").y, 30.0, t);
    // Parent bottom margin (0) collapses with the child's (0): p ends at 35.
    let q = fx.rect(&t, "q");
    assert_px!(q.y, 45.0, t);
    assert_px!(fx.rect(&t, "d").y, 45.0 + 1.0 + 30.0, t);
}

#[test]
fn last_child_bottom_margin_collapses_through_parent() {
    let fx = Fx::new(
        "<div id=p><div style='height:5px;margin-bottom:20px'></div></div><div id=n style='margin-top:-5px'></div><div id=m style='height:1px'></div>",
        RESET,
    );
    let t = fx.layout();
    assert_px!(fx.rect(&t, "p").h, 5.0, t);
    // 20 and -5 collapse → 15.
    assert_px!(fx.rect(&t, "m").y, 20.0, t);
}

#[test]
fn bfc_root_does_not_collapse_with_children() {
    let fx = Fx::new(
        "<div id=p style='overflow:hidden'><div id=c style='margin-top:20px;height:5px'></div></div>",
        RESET,
    );
    let t = fx.layout();
    assert_px!(fx.rect(&t, "p").y, 0.0, t);
    assert_px!(fx.rect(&t, "c").y, 20.0, t);
    assert_px!(fx.rect(&t, "p").h, 25.0, t);
}

#[test]
fn negative_clearance_acid2_smile() {
    // The Acid2 "nose / empty / smile" sequence: the float is placed after
    // the previous sibling's margin, the empty block's margins collapse
    // through, and the cleared block's border edge sits on the float's
    // bottom margin edge (clearance is negative).
    let css = format!(
        "{RESET} body {{ font-size: 12px }} .forehead {{ margin: 4em; height: 1em }} \
         .nose {{ float: left; margin: -2em 2em -1em; height: 3em; width: 2em; border-bottom: 1em solid }} \
         .empty {{ margin: 6.25em; height: 10%; }} .empty div {{ margin: 0 2em -6em 4em; }} \
         .smile {{ margin: 5em 3em; clear: both; }} .smile div {{ margin-top: 0.25em; height: 2em }}"
    );
    let fx = Fx::new(
        "<div style='border-top:12px solid'><div class=forehead id=f></div><div class=nose id=n></div><div class=empty><div></div></div><div class=smile id=s><div id=sd></div></div></div>",
        &css,
    );
    let t = fx.layout();
    let f = fx.rect(&t, "f");
    assert_px!(f.y, 12.0 + 48.0, t);
    let n = fx.rect(&t, "n");
    // Float margin-box top = forehead bottom + 4em; border box 2em higher.
    assert_px!(n.y, f.bottom() + 48.0 - 24.0, t);
    let s = fx.rect(&t, "s");
    // Float margin-box bottom = border bottom - 1em.
    assert_px!(s.y, n.bottom() - 12.0, t);
    assert_px!(fx.rect(&t, "sd").y, s.y, t);
}
