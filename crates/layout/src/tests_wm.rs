//! Writing-mode tests (CSS Writing Modes 3/4): vertical subtrees,
//! orthogonal flows and their painting, checked in physical coordinates.

use crate::tree::{BoxKind, LayoutBox, LayoutTree};
use crate::{build_display_list, layout, DisplayItem, DisplayList, LayoutContext};
use rabbithole_css::{parse_stylesheet, MediaEnv};
use rabbithole_dom::Document;
use rabbithole_gfx::{FontContext, Image, Matrix, Rect};
use rabbithole_style::{style_tree, StyledTree};
use std::collections::HashMap;
use std::sync::Arc;

type Bgs = HashMap<String, Arc<Image>>;

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

    fn run<R>(&self, bgs: &Bgs, f: impl FnOnce(LayoutTree, &LayoutContext) -> R) -> R {
        let images = HashMap::new();
        let scrolls = HashMap::new();
        let ctx = LayoutContext {
            doc: &self.doc,
            styles: &self.styles,
            fonts: &self.fonts,
            images: &images,
            background_images: bgs,
            viewport_width: 800.0,
            viewport_height: 600.0,
            scroll_offsets: &scrolls,
            page_scroll: (0.0, 0.0),
        };
        let t = layout(&ctx);
        f(t, &ctx)
    }

    fn layout(&self) -> LayoutTree {
        self.run(&HashMap::new(), |t, _| t)
    }

    fn display_list(&self, bgs: &Bgs) -> DisplayList {
        self.run(bgs, |t, ctx| build_display_list(&t, ctx))
    }

    /// Physical border box of the element with `id` (the writing-mode
    /// transforms of its ancestors applied).
    fn rect(&self, tree: &LayoutTree, id: &str) -> Rect {
        let node = self.doc.element_by_id(id).unwrap_or_else(|| panic!("no #{id}"));
        fn find(b: &LayoutBox, n: usize, m: Matrix) -> Option<Rect> {
            if b.node == Some(n) && b.kind != BoxKind::Text && b.pseudo.is_none() {
                return Some(rabbithole_gfx::path::transform_rect_bounds(&m, b.rect));
            }
            let m = match b.wm_transform {
                Some(w) => rabbithole_style::multiply_matrix(m, w),
                None => m,
            };
            b.children.iter().find_map(|c| find(c, n, m))
        }
        find(&tree.root, node, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]).unwrap_or_else(|| panic!("no box for #{id}\n{}", tree.dump()))
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

macro_rules! assert_rect {
    ($r:expr, $x:expr, $y:expr, $w:expr, $h:expr, $tree:expr) => {{
        let r = $r;
        assert!(close(r.x, $x) && close(r.y, $y) && close(r.w, $w) && close(r.h, $h), "{} = {:?} (expected {} {} {} {})\n{}", stringify!($r), r, $x, $y, $w, $h, $tree.dump())
    }};
}

const RESET: &str = "html, body { margin: 0; padding: 0; } div { display: block; }";

fn fake_image(w: u32, h: u32) -> Arc<Image> {
    Arc::new(Image::new(w, h, vec![200u8; (w * h * 4) as usize]))
}

#[test]
fn positioned_vertical_box_paints_its_background_image() {
    // The wrapper paints the image unturned; a positioned inner box is a
    // stacking context of its own and must still receive it.
    let fx = Fx::new("<div style='writing-mode:vertical-rl;position:relative;width:100px;height:50px;background:red url(x.png)'></div>", RESET);
    let mut bgs: Bgs = HashMap::new();
    bgs.insert("x.png".into(), fake_image(10, 10));
    let list = fx.display_list(&bgs);
    assert!(list.items.iter().any(|i| matches!(i, DisplayItem::Image { .. } | DisplayItem::ImageTiled { .. })), "{:#?}", list.items);
}

#[test]
fn user_agent_paragraph_margins_are_flow_relative() {
    // HTML's `p { margin-block: 1em }`: in vertical-rl the block axis is
    // horizontal, so the margins are on the right and left.
    let fx = Fx::new("<div style='writing-mode:vertical-rl;font-size:10px;height:100px'><p id=p style='width:20px'></p></div>", RESET);
    let p = fx.doc.element_by_id("p").unwrap();
    let s = fx.styles.get(p).expect("style");
    assert_eq!((s.margin.top, s.margin.bottom), (rabbithole_style::Length::ZERO, rabbithole_style::Length::ZERO));
    assert_eq!((s.margin.right, s.margin.left), (rabbithole_style::Length::Px(10.0), rabbithole_style::Length::Px(10.0)));
    // And in a right-to-left list the marker side gets the padding.
    let fx = Fx::new("<ul id=u dir=rtl><li>x</ul>", RESET);
    let s = fx.styles.get(fx.doc.element_by_id("u").unwrap()).expect("style");
    assert_eq!((s.padding.left, s.padding.right), (rabbithole_style::Length::ZERO, rabbithole_style::Length::Px(40.0)));
}

#[test]
fn nested_vertical_box_paints_background_images_unturned() {
    // A box inside a vertical subtree: its image is painted under the
    // inverse of the subtree's mapping, onto the box's physical rect.
    let fx = Fx::new("<div style='writing-mode:vertical-rl;width:300px;height:200px'><div id=b style='width:100px;height:50px;background:url(x.png)'></div></div>", RESET);
    let mut bgs: Bgs = HashMap::new();
    bgs.insert("x.png".into(), fake_image(10, 10));
    let list = fx.display_list(&bgs);
    let img = list.items.iter().position(|i| matches!(i, DisplayItem::Image { .. } | DisplayItem::ImageTiled { .. })).expect("image");
    // The innermost open transform is the inverse quarter turn.
    let mut open = Vec::new();
    for it in &list.items[..img] {
        match it {
            DisplayItem::PushTransform(m) => open.push(*m),
            DisplayItem::PopTransform => {
                open.pop();
            }
            _ => {}
        }
    }
    let m = open.last().copied().expect("transform");
    assert!(m[0] == 0.0 && m[1] == -1.0 && m[2] == 1.0, "{m:?}");
    let t = fx.layout();
    assert_rect!(fx.rect(&t, "b"), 200.0, 0.0, 100.0, 50.0, t);
}

#[test]
fn vertical_lines_centre_atomic_inlines_on_the_central_baseline() {
    // A 40px line (physical width): the strut is centred on the central
    // baseline, and so is the 10px inline-block.
    let fx = Fx::new("<div style='writing-mode:vertical-rl;font-size:20px;line-height:40px;height:200px'><span id=i style='display:inline-block;width:10px;height:10px'></span></div>", RESET);
    let t = fx.layout();
    assert_rect!(fx.rect(&t, "i"), 15.0, 0.0, 10.0, 10.0, t);
    // Set sideways, the line keeps the alphabetic baseline: the box sits
    // on it (above it, i.e. to its right), not centred.
    let fx = Fx::new("<div style='writing-mode:vertical-rl;text-orientation:sideways;font-size:20px;line-height:40px;height:200px'><span id=i style='display:inline-block;width:10px;height:10px'></span></div>", RESET);
    let t = fx.layout();
    assert!(!close(fx.rect(&t, "i").x, 15.0), "{}", t.dump());
}

#[test]
fn orthogonal_children_do_not_give_an_inline_block_its_baseline() {
    let fx = Fx::new("<div style='font-size:20px;line-height:20px'>aaa<div id=ib style='display:inline-block'>bbb<div style='writing-mode:vertical-rl'>vvv</div></div></div>", RESET);
    let t = fx.layout();
    // Aligned by its "bbb" line: its top is the line's top.
    assert!(close(fx.rect(&t, "ib").y, 0.0), "{}", t.dump());
}

#[test]
fn right_to_left_inline_absolutes_end_at_their_static_position() {
    // After a 50px inline-block in a right-to-left line, the hypothetical
    // box would start (on the right) where the inline-block ends.
    let fx = Fx::new("<div style='direction:rtl;width:200px;position:relative'><span style='display:inline-block;width:50px;height:10px'></span><span id=a style='position:absolute;width:10px;height:10px'></span></div>", RESET);
    let t = fx.layout();
    assert_rect!(fx.rect(&t, "a"), 140.0, 0.0, 10.0, 10.0, t);
    // The same in a vertical line (inline axis downwards).
    let fx = Fx::new("<div style='writing-mode:vertical-rl;direction:rtl;height:200px;width:20px;position:relative'><span style='display:inline-block;height:50px;width:10px'></span><span id=a style='position:absolute;width:10px;height:10px'></span></div>", RESET);
    let t = fx.layout();
    let a = fx.rect(&t, "a");
    assert!(close(a.y, 140.0) && close(a.h, 10.0), "{a:?}\n{}", t.dump());
}

#[test]
fn orthogonal_flow_in_an_auto_sized_container_fills_the_icb_inline_size() {
    // The vertical container's width (the horizontal box's inline axis)
    // is indefinite: the available size is the viewport width (800px).
    let fx = Fx::new("<div style='writing-mode:vertical-rl'><div id=h style='writing-mode:horizontal-tb'><span style='display:inline-block;width:500px'></span> <span style='display:inline-block;width:500px'></span></div></div>", RESET);
    let t = fx.layout();
    let h = fx.rect(&t, "h");
    assert!(close(h.w, 800.0), "{h:?}\n{}", t.dump());
}

fn line_texts(t: &LayoutTree) -> Vec<String> {
    fn walk(b: &LayoutBox, out: &mut Vec<String>) {
        for l in &b.lines {
            let mut s = String::new();
            for f in &l.fragments {
                if let crate::tree::FragmentKind::Text { text, .. } = &f.kind {
                    s.push_str(text);
                }
            }
            out.push(s);
        }
        for c in &b.children {
            walk(c, out);
        }
    }
    let mut out = Vec::new();
    walk(&t.root, &mut out);
    out
}

#[test]
fn no_break_spaces_neither_hang_nor_trim() {
    // A run of NBSPs after a space is a word: it moves to the next line
    // rather than hanging at the end of this one.
    let nb = "\u{a0}".repeat(20);
    let fx = Fx::new(&format!("<div style='width:40px'>x {nb}</div>"), RESET);
    let t = fx.layout();
    assert_eq!(line_texts(&t).len(), 2, "{:?}", line_texts(&t));
    // NBSPs ending a line are kept (they paint as spaces).
    let fx = Fx::new("<div style='width:1000px'>a\u{a0}\u{a0} b</div>", RESET);
    let t = fx.layout();
    assert_eq!(line_texts(&t), vec!["a   b".to_string()]);
}

fn find_box<'t>(b: &'t LayoutBox, f: &dyn Fn(&LayoutBox) -> bool) -> Option<&'t LayoutBox> {
    if f(b) {
        return Some(b);
    }
    b.children.iter().find_map(|c| find_box(c, f))
}

#[test]
fn combined_upright_text_is_one_squeezed_em_square() {
    let fx = Fx::new("<div style='writing-mode:vertical-rl;font-size:20px;line-height:1;height:300px'>a<span style='text-combine-upright:all'>12345</span>b</div>", RESET);
    let t = fx.layout();
    let w = find_box(&t.root, &|b| b.wm_tcy).unwrap_or_else(|| panic!("no composition\n{}", t.dump()));
    // 1em along the line (logical width) and across it.
    assert!(close(w.rect.w, 20.0) && close(w.rect.h, 20.0), "{:?}\n{}", w.rect, t.dump());
    // Five digits are wider than 1em: the turn is combined with a squeeze.
    let m = w.wm_transform.expect("mapping");
    assert!(m[1].abs() < 1.0 && m[1] != 0.0 && m[2].abs() == 1.0, "{m:?}");
    // `digits 2` combines only runs of at most two digits.
    assert_eq!(crate::wm::combined_runs_for_test("a12b345c6", 3), vec![(0, 1, false), (1, 3, true), (3, 8, false), (8, 9, true)]);
}
