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

/// `::first-letter` wraps the first letter and its punctuation, inside a
/// nested block, in an inline box (or a float when floated).
#[test]
fn first_letter_wraps_letter_in_nested_block() {
    use crate::tree::{Flow, InlineItem};
    let fx = Fx::new("<div id=d><p id=p>  \"Txy</p></div>", "div::first-letter { font-size: 40px } p { margin: 0 }");
    let tree = fx.layout();
    let node = fx.doc.element_by_id("p").unwrap();
    fn find(b: &LayoutBox, n: usize) -> Option<&LayoutBox> {
        if b.node == Some(n) && b.kind == BoxKind::Block {
            return Some(b);
        }
        b.children.iter().find_map(|c| find(c, n))
    }
    let p = find(&tree.root, node).unwrap();
    let texts: Vec<(String, f32)> = p.items.iter().filter_map(|i| match i {
        InlineItem::Text { text, style, .. } => Some((text.to_string(), style.font_size)),
        _ => None,
    }).collect();
    assert!(texts.iter().any(|(t, s)| t == "\"T" && *s == 40.0), "{texts:?}");
    assert!(texts.iter().any(|(t, s)| t == "xy" && *s == 16.0), "{texts:?}");

    let fx = Fx::new("<div id=d>Drop cap</div>", "div::first-letter { float: left; font-size: 50px }");
    let tree = fx.layout();
    let node = fx.doc.element_by_id("d").unwrap();
    let d = find(&tree.root, node).unwrap();
    assert!(d.children.iter().any(|c| c.flow == Flow::FloatLeft && c.rect.h > 40.0), "{}", tree.dump());
}

/// Percentage `top` against an indefinite containing block height is
/// `auto`; against a definite one it resolves.
#[test]
fn relative_percent_top_needs_definite_height() {
    let fx = Fx::new(
        "<div style='min-height:100px'><div id=a style='height:10px;position:relative;top:50%'></div></div>\
         <div style='height:100px'><div id=b style='height:10px;position:relative;top:50%'></div></div>",
        "body { margin: 0 }",
    );
    let tree = fx.layout();
    assert!(close(fx.rect(&tree, "a").y, 0.0));
    assert!(close(fx.rect(&tree, "b").y, 150.0));
}

/// A block inside a relatively positioned inline moves with it.
#[test]
fn block_in_relative_inline_is_offset() {
    let fx = Fx::new("<div style='height:100px'><span style='position:relative;left:30px;top:20px'><div id=b style='height:10px'></div></span></div>", "body { margin: 0 }");
    let tree = fx.layout();
    let r = fx.rect(&tree, "b");
    assert!(close(r.x, 30.0) && close(r.y, 20.0), "{r:?}");
}

fn box_of<'t>(fx: &Fx, tree: &'t LayoutTree, id: &str) -> &'t LayoutBox {
    let node = fx.doc.element_by_id(id).unwrap();
    fn find(b: &LayoutBox, n: usize) -> Option<&LayoutBox> {
        if b.node == Some(n) && b.kind != BoxKind::Text && b.pseudo.is_none() {
            return Some(b);
        }
        b.children.iter().find_map(|c| find(c, n))
    }
    find(&tree.root, node).unwrap()
}

/// Column-group borders take part in collapsed border resolution.
#[test]
fn collapsed_borders_include_column_groups() {
    let fx = Fx::new(
        "<table style='border-collapse:collapse'><colgroup style='border:4px solid green'><col><col></colgroup><tr><td id=a>x</td><td id=b>y</td></tr></table>",
        "",
    );
    let tree = fx.layout();
    let a = box_of(&fx, &tree, "a");
    assert!(a.style.border_width.left > 0.0 && a.style.border_width.top > 0.0, "{:?}", a.style.border_width);
    // No border between the two columns of the group.
    assert_eq!(a.style.border_width.right, 0.0);
}

/// A table with an auto width goes beside a float, narrowed to the space
/// left; an atomic inline starting a line clears floats over its height.
#[test]
fn tables_and_atomics_beside_floats() {
    let fx = Fx::new(
        "<div style='width:300px'><div style='float:left;width:100px;height:50px'></div>\
         <table id=t style='border-spacing:0'><tr><td style='padding:0'><span style='display:inline-block;width:150px;height:10px'></span> <span style='display:inline-block;width:150px;height:10px'></span></td></tr></table></div>",
        "body { margin: 0 }",
    );
    let tree = fx.layout();
    let t = fx.rect(&tree, "t");
    assert!(close(t.x, 100.0) && t.w <= 200.01, "{t:?}");

    let fx = Fx::new(
        "<div style='width:400px'><div style='float:left;clear:left;width:50px;height:75px'></div><div style='float:left;clear:left;width:100px;height:75px'></div>\
         <span style='display:inline-block;vertical-align:top;width:200px;height:50px'></span><span id=s style='display:inline-block;vertical-align:top;width:200px;height:50px'></span></div>",
        "body { margin: 0 }",
    );
    let tree = fx.layout();
    let s = fx.rect(&tree, "s");
    assert!(close(s.x, 100.0) && close(s.y, 50.0), "{s:?}");
}

/// An absolutely positioned table between two insets keeps its
/// shrink-to-fit width.
#[test]
fn absolute_table_between_insets_shrinks_to_fit() {
    let fx = Fx::new(
        "<div style='position:relative;width:400px;height:100px'><table id=t style='position:absolute;left:1px;right:1px;border-spacing:0'><tr><td style='padding:0'><div style='width:50px;height:10px'></div></td></tr></table></div>",
        "body { margin: 0 }",
    );
    let tree = fx.layout();
    let t = fx.rect(&tree, "t");
    assert!(close(t.x, 1.0) && close(t.w, 50.0), "{t:?}");
}

/// `::first-line` colors the text of the first formatted line only.
#[test]
fn first_line_color_applies_to_first_line() {
    use crate::tree::FragmentKind;
    let fx = Fx::new(
        "<div id=d style='width:60px'><p id=p>aaa bbb ccc ddd eee fff</p></div>",
        "body { margin: 0 } p { margin: 0; color: rgb(255, 0, 0) } div::first-line { color: rgb(0, 128, 0) }",
    );
    let tree = fx.layout();
    let p = box_of(&fx, &tree, "p");
    assert!(p.lines.len() > 1);
    let colors = |i: usize| -> Vec<rabbithole_gfx::Color> {
        p.lines[i].fragments.iter().filter_map(|f| match &f.kind { FragmentKind::Text { color, .. } => Some(*color), _ => None }).collect()
    };
    // The paragraph sets its own color: the div's first line does not
    // override it.
    assert!(colors(0).iter().all(|c| *c == rabbithole_gfx::Color::rgb(255, 0, 0)));

    let fx = Fx::new(
        "<p id=p style='width:60px'>aaa bbb ccc ddd eee fff</p>",
        "body { margin: 0 } p { margin: 0; color: rgb(255, 0, 0) } p::first-line { color: rgb(0, 128, 0) }",
    );
    let tree = fx.layout();
    let p = box_of(&fx, &tree, "p");
    let first: Vec<_> = p.lines[0].fragments.iter().filter_map(|f| match &f.kind { FragmentKind::Text { color, .. } => Some(*color), _ => None }).collect();
    let second: Vec<_> = p.lines[1].fragments.iter().filter_map(|f| match &f.kind { FragmentKind::Text { color, .. } => Some(*color), _ => None }).collect();
    assert!(!first.is_empty() && first.iter().all(|c| *c == rabbithole_gfx::Color::rgb(0, 128, 0)));
    assert!(second.iter().all(|c| *c == rabbithole_gfx::Color::rgb(255, 0, 0)));
}

/// An auto width between two insets limited by max-width is recomputed
/// as specified: auto margins then center the box (CSS 2.1 §10.4).
#[test]
fn absolute_max_width_between_insets_centers_with_auto_margins() {
    let fx = Fx::new(
        "<div style='position:relative;width:400px;height:100px'><div id=a style='position:absolute;left:0;right:0;max-width:100px;margin:0 auto;height:10px'></div></div>",
        "body { margin: 0 }",
    );
    let tree = fx.layout();
    let a = fx.rect(&tree, "a");
    assert!(close(a.x, 150.0) && close(a.w, 100.0), "{a:?}");
}

#[test]
fn inline_box_fragments_exclude_margins_and_preceding_space() {
    let fx = Fx::new(r#"<body><p><span id=a style="margin-left:50px;margin-right:30px;background:red">x</span> <span id=b style="background:black">y</span></p></body>"#, "");
    let t = fx.layout();
    let boxes: Vec<Rect> = t.root.children[0].children[0].lines[0]
        .fragments
        .iter()
        .filter(|f| matches!(f.kind, crate::tree::FragmentKind::InlineBox { .. }))
        .map(|f| f.rect)
        .collect();
    let texts: Vec<Rect> = t.root.children[0].children[0].lines[0]
        .fragments
        .iter()
        .filter(|f| matches!(f.kind, crate::tree::FragmentKind::Text { .. }))
        .map(|f| f.rect)
        .collect();
    assert_eq!(boxes.len(), 2, "{}", t.dump());
    // The first box starts after its left margin, at its text.
    assert!(close(boxes[0].x, 58.0) && close(boxes[0].right(), texts[0].right()), "{}", t.dump());
    // The second starts at its own text, after the right margin and the space.
    let y = texts.last().unwrap();
    assert!(close(boxes[1].x, y.x) && close(boxes[1].right(), y.right()), "{}", t.dump());
    assert!(boxes[1].x > boxes[0].right() + 30.0, "{}", t.dump());
}

#[test]
fn table_height_goes_to_rows_without_specified_height() {
    let fx = Fx::new(
        "<table id=t><tr><td id=a>x</td><td>x</td></tr><tr><td id=b>x</td><td>x</td></tr></table>",
        "body { margin: 0 } table { height: 100px; border-collapse: collapse } td { padding: 0 } tr:first-child td { height: 30px }",
    );
    let t = fx.layout();
    let (a, b) = (fx.rect(&t, "a"), fx.rect(&t, "b"));
    assert!(close(a.h, 30.0), "{a:?}");
    assert!(close(b.h, 70.0), "{b:?}");
}

#[test]
fn empty_list_item_is_one_marker_line_tall() {
    let fx = Fx::new("<ul><li id=a></li><li id=b style='list-style:none'></li></ul>", "body { margin: 0 } li { line-height: 20px }");
    let t = fx.layout();
    assert!(close(fx.rect(&t, "a").h, 20.0), "{}", t.dump());
    assert!(close(fx.rect(&t, "b").h, 0.0), "{}", t.dump());
}

#[test]
fn calc_margins_resolve_against_zero_in_content_sizes() {
    let fx = Fx::new("<div id=a><div style='margin-left: calc(10% + 100px)'></div></div>", "body { margin: 0 } #a { width: min-content; height: 10px }");
    let t = fx.layout();
    assert!(close(fx.rect(&t, "a").w, 100.0), "{}", t.dump());
}

#[test]
fn percentage_height_replaced_in_anonymous_block_sizes_max_content() {
    let fx = Fx::new(
        "<div id=f><div id=i><div></div><canvas width=400 height=400 style='height:100%'></canvas></div></div>",
        "body { margin: 0 } #f { display: flex; flex-direction: column } #i { width: max-content; height: 200px }",
    );
    let t = fx.layout();
    assert!(close(fx.rect(&t, "i").w, 200.0), "{}", t.dump());
}

#[test]
fn border_box_replaced_heights_keep_the_ratio() {
    let fx = Fx::new(
        "<img id=a width=100 height=100><img id=b width=100 height=100><img id=c width=100 height=100>",
        "body { margin: 0 } img { box-sizing: border-box; padding-bottom: 30px; width: auto; height: auto } #a { max-height: 100px } #b { height: 100px } #c { min-height: 160px }",
    );
    let t = fx.layout();
    let (a, b, c) = (fx.rect(&t, "a"), fx.rect(&t, "b"), fx.rect(&t, "c"));
    assert!(close(a.w, 70.0) && close(a.h, 100.0), "{a:?}");
    assert!(close(b.w, 70.0) && close(b.h, 100.0), "{b:?}");
    assert!(close(c.w, 130.0) && close(c.h, 160.0), "{c:?}");
}

#[test]
fn layout_and_paint_containment_contain_absolute_boxes() {
    for c in ["layout", "paint", "content"] {
        let fx = Fx::new("<div id=c><div id=a></div></div>", &format!("body {{ margin: 0 }} #c {{ contain: {c}; width: 100px; height: 100px; margin: 50px }} #a {{ position: absolute; right: 0; bottom: 0; width: 10px; height: 10px }}"));
        let t = fx.layout();
        let a = fx.rect(&t, "a");
        assert!(close(a.x, 140.0) && close(a.y, 140.0), "{c}: {a:?}");
    }
}

#[test]
fn empty_inline_with_padding_makes_a_line() {
    let fx = Fx::new("<div id=a><span style='padding: 10px'></span></div><div id=b><span></span></div>", "body { margin: 0 } div { line-height: 20px }");
    let t = fx.layout();
    assert!(close(fx.rect(&t, "a").h, 20.0), "{}", t.dump());
    assert!(close(fx.rect(&t, "b").h, 0.0), "{}", t.dump());
}

#[test]
fn inline_box_start_edge_wraps_with_its_content() {
    let fx = Fx::new("<div><span style='font-size: 16px'>aa <span id=s style='border-left: 20px solid'>b</span></span></div>", "body { margin: 0 } div { width: 40px; font: 16px monospace }");
    let t = fx.layout();
    let lines = &t.root.children[0].children[0].lines;
    assert_eq!(lines.len(), 2, "{}", t.dump());
    let boxes = |l: usize| lines[l].fragments.iter().filter(|f| matches!(&f.kind, crate::tree::FragmentKind::InlineBox { style, .. } if style.border_width.left > 0.0)).map(|f| f.rect).collect::<Vec<_>>();
    // The bordered box starts on the second line, at its start.
    assert!(boxes(0).is_empty(), "{}", t.dump());
    assert!(boxes(1).iter().any(|r| close(r.x, 0.0)), "{}", t.dump());
}
