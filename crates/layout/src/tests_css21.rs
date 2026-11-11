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

// ------------------------------------------------------------------ tables

#[test]
fn acid2_list_as_table_lays_cells_out_horizontally() {
    let css = format!(
        "{RESET} body {{ font-size: 12px }} ul {{ display: table; padding: 0; margin: 0; background: red; list-style: disc }} \
         ul li {{ padding: 0; margin: 0; }} \
         ul li.first-part {{ display: table-cell; height: 1em; width: 1em; background: black; }} \
         ul li.second-part {{ display: table; height: 1em; width: 1em; background: black; }} \
         ul li.third-part {{ display: table-cell; height: 0.5em; width: 1em; background: black; }} \
         ul li.fourth-part {{ list-style: none; height: 1em; width: 1em; background: black; }}"
    );
    let fx = Fx::new(
        "<ul id=ul>\n <li id=a class=first-part></li>\n <li id=b class=second-part></li>\n <li id=c class=third-part></li>\n <li id=d class=fourth-part></li>\n</ul>",
        &css,
    );
    let t = fx.layout();
    let ul = fx.rect(&t, "ul");
    assert_px!(ul.w, 48.0, t);
    assert_px!(ul.h, 12.0, t);
    for (id, x) in [("a", 0.0), ("b", 12.0), ("c", 24.0), ("d", 36.0)] {
        let r = fx.rect(&t, id);
        assert_px!(r.x, x, t);
        assert_px!(r.y, 0.0, t);
        assert_px!(r.h, if id == "c" { 12.0 } else { 12.0 }, t);
    }
}

#[test]
fn colspan_rowspan_and_border_spacing() {
    let fx = Fx::new(
        "<table style='border-spacing:4px 2px'><tr><td id=a colspan=2><div style='width:50px;height:10px'></div></td><td id=b rowspan=2><div style='width:20px;height:40px'></div></td></tr>\
         <tr><td id=c><div style='width:10px;height:10px'></div></td><td id=d><div style='width:30px;height:10px'></div></td></tr></table>",
        &format!("{RESET} td {{ padding: 0 }} table {{ box-sizing: content-box }}"),
    );
    let t = fx.layout();
    let (a, b, c, d) = (fx.rect(&t, "a"), fx.rect(&t, "b"), fx.rect(&t, "c"), fx.rect(&t, "d"));
    assert_px!(c.x, 4.0, t);
    assert_px!(d.x, c.right() + 4.0, t);
    assert_px!(a.x, 4.0, t);
    assert_px!(a.right(), d.right(), t);
    assert_px!(a.y, 2.0, t);
    assert_px!(c.y, a.bottom() + 2.0, t);
    assert_px!(b.x, a.right() + 4.0, t);
    // The row-spanning cell covers both rows (and makes them tall enough).
    assert_px!(b.y, 2.0, t);
    assert_px!(b.bottom(), c.bottom(), t);
    assert!(b.h >= 40.0);
    assert!(a.w >= 50.0 - 0.01, "{}", t.dump());
}

#[test]
fn collapsed_borders_share_grid_lines() {
    let fx = Fx::new(
        "<table id=t><tr><td id=a>x</td><td id=b>y</td></tr></table>",
        &format!("{RESET} table {{ border-collapse: collapse; border: 4px solid black; padding: 10px }} td {{ border: 2px solid red; padding: 0; width: 20px }}"),
    );
    let t = fx.layout();
    let (tb, a, b) = (fx.rect(&t, "t"), fx.rect(&t, "a"), fx.rect(&t, "b"));
    // Table border 4px wins at the outer edge: 2px outside the cells.
    assert_px!(a.x - tb.x, 2.0, t);
    // Cells touch: no spacing, the line between them is shared.
    assert_px!(b.x, a.right(), t);
    // Table padding is ignored in the collapsing model.
    assert_px!(a.y - tb.y, 2.0, t);
}

#[test]
fn fixed_table_layout_uses_first_row() {
    let fx = Fx::new(
        "<table style='table-layout:fixed;width:300px'><tr><td id=a style='width:100px'>x</td><td id=b>a-very-long-unbreakable-word-that-would-widen-an-auto-column-a-lot-more-than-two-hundred-pixels</td></tr></table>",
        &format!("{RESET} table {{ border-spacing: 0 }} td {{ padding: 0 }}"),
    );
    let t = fx.layout();
    assert_px!(fx.rect(&t, "a").w, 100.0, t);
    assert_px!(fx.rect(&t, "b").w, 200.0, t);
}

#[test]
fn captions_sit_outside_the_table_box() {
    let fx = Fx::new(
        "<table id=t><caption id=c>Cap</caption><tr><td id=a style='height:20px'>x</td></tr></table><table id=u><caption id=d style='caption-side:bottom'>Bottom</caption><tr><td id=e>x</td></tr></table>",
        &format!("{RESET} table {{ border: 3px solid; margin: 5px }} td {{ padding: 0 }}"),
    );
    let t = fx.layout();
    let (c, a) = (fx.rect(&t, "c"), fx.rect(&t, "a"));
    assert!(c.h > 0.0);
    // The wrapper carries the margins; the caption is above the table box.
    assert_px!(c.y, 5.0, t);
    fn find(b: &LayoutBox) -> Option<&LayoutBox> {
        if b.formatting == crate::tree::Formatting::Table {
            return Some(b);
        }
        b.children.iter().find_map(find)
    }
    let table_box = find(&t.root).unwrap().rect;
    assert_px!(table_box.y, c.bottom(), t);
    assert!(a.y >= table_box.y + 3.0);
    let (d, e) = (fx.rect(&t, "d"), fx.rect(&t, "e"));
    assert!(d.y > e.bottom(), "bottom caption below the cells\n{}", t.dump());
}

#[test]
fn vertical_align_in_cells() {
    let fx = Fx::new(
        "<table><tr><td id=tall style='height:100px'></td><td id=top style='vertical-align:top'><div id=t1 style='height:10px'></div></td>\
         <td style='vertical-align:middle'><div id=m1 style='height:10px'></div></td><td style='vertical-align:bottom'><div id=b1 style='height:10px'></div></td></tr></table>",
        &format!("{RESET} table {{ border-spacing: 0 }} td {{ padding: 0; width: 10px }}"),
    );
    let t = fx.layout();
    let tall = fx.rect(&t, "tall");
    assert_px!(tall.h, 100.0, t);
    assert_px!(fx.rect(&t, "t1").y, tall.y, t);
    assert_px!(fx.rect(&t, "m1").y, tall.y + 45.0, t);
    assert_px!(fx.rect(&t, "b1").y, tall.y + 90.0, t);
}

#[test]
fn misparented_cells_get_anonymous_table() {
    let fx = Fx::new(
        "<div id=p><div id=a style='display:table-cell;width:30px'>A</div>  <div id=b style='display:table-cell;width:40px'>B</div></div>",
        RESET,
    );
    let t = fx.layout();
    let (a, b) = (fx.rect(&t, "a"), fx.rect(&t, "b"));
    assert_px!(a.y, b.y, t);
    assert_px!(b.x, a.right(), t);
}

#[test]
fn table_height_is_shared_by_rows_and_is_a_minimum() {
    let fx = Fx::new(
        "<table style='height:100px'><tr><td id=a>x</td></tr><tr><td id=b>y</td></tr></table>",
        &format!("{RESET} table {{ border-spacing: 0; box-sizing: content-box }} td {{ padding: 0 }}"),
    );
    let t = fx.layout();
    let (a, b) = (fx.rect(&t, "a"), fx.rect(&t, "b"));
    assert_px!(a.h + b.h, 100.0, t);
    assert_px!(b.y, a.bottom(), t);
}

#[test]
fn inline_table_flows_inline() {
    let fx = Fx::new(
        "<div>before <span id=it style='display:inline-table'><span style='display:table-cell'>c1</span><span style='display:table-cell'>c2</span></span> after</div>",
        RESET,
    );
    let t = fx.layout();
    let it = fx.rect(&t, "it");
    assert!(it.x > 10.0, "{}", t.dump());
    assert!(it.w > 10.0);
}

#[test]
fn clearfix_div_contains_floats() {
    let fx = Fx::new(
        "<div id=p><div style='float:left;width:50px;height:80px'></div><div id=c style='clear:both'></div></div><div id=n style='height:5px'></div>",
        RESET,
    );
    let t = fx.layout();
    assert_px!(fx.rect(&t, "p").h, 80.0, t);
    assert_px!(fx.rect(&t, "n").y, 80.0, t);
}

#[test]
fn min_height_parent_keeps_child_bottom_margin_inside() {
    let fx = Fx::new(
        "<div id=p style='min-height:50px'><div style='height:10px;margin-bottom:30px'></div></div><div id=n style='height:5px'></div>",
        RESET,
    );
    let t = fx.layout();
    assert_px!(fx.rect(&t, "p").h, 50.0, t);
    assert_px!(fx.rect(&t, "n").y, 50.0, t);
}

#[test]
fn whitespace_only_block_collapses_through() {
    let fx = Fx::new(
        "<div style='height:10px;margin-bottom:20px'></div><div id=e style='margin:15px 0'>   </div><div id=n style='height:5px;margin-top:10px'></div>",
        RESET,
    );
    let t = fx.layout();
    assert_px!(fx.rect(&t, "n").y, 30.0, t);
}

#[test]
fn fixed_box_paints_in_tree_order_with_positioned_siblings() {
    // Acid2's `p.bad`: a fixed box earlier in the tree is covered by a
    // later absolutely positioned one (same stacking level).
    let fx = Fx::new(
        "<div style='position:fixed;top:0;left:0;width:50px;height:50px;background:#f00'></div><div style='position:absolute;top:0;left:0;width:50px;height:50px;background:#0f0'></div>",
        RESET,
    );
    let images = HashMap::new();
    let bgs = HashMap::new();
    let scrolls = HashMap::new();
    let ctx = LayoutContext {
        doc: &fx.doc,
        styles: &fx.styles,
        fonts: &fx.fonts,
        images: &images,
        background_images: &bgs,
        viewport_width: 800.0,
        viewport_height: 600.0,
        scroll_offsets: &scrolls,
        page_scroll: (0.0, 0.0),
    };
    let t = layout(&ctx);
    let list = crate::build_display_list(&t, &ctx);
    let pos = |c: rabbithole_gfx::Color| {
        list.items
            .iter()
            .position(|i| matches!(i, crate::DisplayItem::Rect { color, .. } if *color == c))
            .expect("rect")
    };
    let red = pos(rabbithole_gfx::Color::rgb(255, 0, 0));
    let green = pos(rabbithole_gfx::Color::rgb(0, 255, 0));
    assert!(red < green, "fixed box must paint below the later positioned box");
    let fixed = list.items.iter().position(|i| matches!(i, crate::DisplayItem::PushFixed)).expect("fixed scope");
    assert!(fixed < red);
}
