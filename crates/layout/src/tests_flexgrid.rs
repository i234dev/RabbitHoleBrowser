//! Flexbox and grid layout tests. Properties the cascade may not parse yet
//! are set directly on the computed styles via `patch`.

use crate::tree::{BoxKind, LayoutBox};
use crate::{layout, LayoutContext, LayoutTree};
use rabbithole_css::{parse_stylesheet, MediaEnv};
use rabbithole_dom::Document;
use rabbithole_gfx::FontContext;
use rabbithole_style::{
    style_tree, ComputedStyle, ContentDistribution, FlexDirection, FlexWrap, GridAutoFlow, GridLine, ItemAlign, Length, StyledTree,
    TrackListItem, TrackSize,
};
use std::collections::HashMap;
use std::rc::Rc;

struct Fx {
    doc: Document,
    styles: StyledTree,
    fonts: FontContext,
}

impl Fx {
    fn new(html: &str, css: &str) -> Fx {
        let doc = rabbithole_html::parse(html);
        let sheets = vec![parse_stylesheet(css)];
        let env = MediaEnv {
            width: 800.0,
            height: 600.0,
            screen: true,
            scripting: true,
        };
        let styles = style_tree(&doc, &sheets, &env);
        Fx {
            doc,
            styles,
            fonts: FontContext::new(),
        }
    }

    /// Mutate the computed style of the element with `id`.
    fn patch(&mut self, id: &str, f: impl FnOnce(&mut ComputedStyle)) {
        let node = self.doc.element_by_id(id).unwrap_or_else(|| panic!("no element #{id}"));
        let slot = self.styles.styles[node].as_mut().expect("styled");
        f(Rc::make_mut(slot));
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
            scroll_offsets: &scrolls,
            page_scroll: (0.0, 0.0),
            viewport_width: 800.0,
            viewport_height: 600.0,
        };
        layout(&ctx)
    }

    fn by_id<'t>(&self, tree: &'t LayoutTree, id: &str) -> &'t LayoutBox {
        let node = self.doc.element_by_id(id).unwrap_or_else(|| panic!("no element #{id}"));
        fn walk<'t>(b: &'t LayoutBox, node: usize) -> Option<&'t LayoutBox> {
            if b.node == Some(node) && b.kind != BoxKind::Text {
                return Some(b);
            }
            b.children.iter().find_map(|c| walk(c, node))
        }
        walk(&tree.root, node).unwrap_or_else(|| panic!("no box for #{id}"))
    }
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.6
}

macro_rules! assert_near {
    ($a:expr, $b:expr) => {
        let (a, b) = ($a, $b);
        assert!(near(a, b), "expected {} ≈ {} ({})", a, b, stringify!($a));
    };
}

fn px(v: f32) -> TrackListItem {
    TrackListItem::Track {
        names: vec![],
        size: TrackSize::Length(Length::Px(v)),
    }
}
fn fr(v: f32) -> TrackListItem {
    TrackListItem::Track {
        names: vec![],
        size: TrackSize::Fr(v),
    }
}
fn tr(size: TrackSize) -> TrackListItem {
    TrackListItem::Track { names: vec![], size }
}

const FLEX3: &str = "<div id=c><div id=a>A</div><div id=b>B</div><div id=d>D</div></div>";
const FLEX_CSS: &str = "#c { display: flex; width: 600px; } #c > div { height: 20px; width: 0 }";

fn grow_all(fx: &mut Fx, ids: &[&str], g: f32) {
    for id in ids {
        fx.patch(id, |s| s.flex_grow = g);
    }
}

// ------------------------------------------------------------------ flexbox

#[test]
fn flex_equal_share() {
    let mut fx = Fx::new(FLEX3, FLEX_CSS);
    grow_all(&mut fx, &["a", "b", "d"], 1.0);
    let t = fx.layout();
    let (a, b, d) = (fx.by_id(&t, "a"), fx.by_id(&t, "b"), fx.by_id(&t, "d"));
    assert_near!(a.rect.w, 200.0);
    assert_near!(b.rect.w, 200.0);
    assert_near!(d.rect.w, 200.0);
    assert_near!(b.rect.x, a.rect.x + 200.0);
    assert_near!(d.rect.x, a.rect.x + 400.0);
    assert_near!(a.rect.y, b.rect.y);
}

#[test]
fn flex_grow_1_vs_2() {
    let mut fx = Fx::new("<div id=c><div id=a>A</div><div id=b>B</div></div>", "#c { display: flex; width: 600px } #c > div { width: 0 }");
    fx.patch("a", |s| s.flex_grow = 1.0);
    fx.patch("b", |s| s.flex_grow = 2.0);
    let t = fx.layout();
    assert_near!(fx.by_id(&t, "a").rect.w, 200.0);
    assert_near!(fx.by_id(&t, "b").rect.w, 400.0);
}

#[test]
fn flex_shrink_proportional() {
    let mut fx = Fx::new("<div id=c><div id=a></div><div id=b></div></div>", "#c { display: flex; width: 600px } #a { width: 400px; height: 10px } #b { width: 400px; height: 10px }");
    fx.patch("a", |s| s.min_width = Length::Px(0.0));
    fx.patch("b", |s| s.min_width = Length::Px(0.0));
    let t = fx.layout();
    assert_near!(fx.by_id(&t, "a").rect.w, 300.0);
    assert_near!(fx.by_id(&t, "b").rect.w, 300.0);
    // shrink factor 3 vs 1: deficit 200 split 150 / 50
    fx.patch("a", |s| s.flex_shrink = 3.0);
    let t = fx.layout();
    assert_near!(fx.by_id(&t, "a").rect.w, 250.0);
    assert_near!(fx.by_id(&t, "b").rect.w, 350.0);
}

#[test]
fn flex_wrap_two_lines() {
    let mut fx = Fx::new(FLEX3, "#c { display: flex; width: 300px } #c > div { width: 150px; height: 20px }");
    fx.patch("c", |s| s.flex_wrap = FlexWrap::Wrap);
    let t = fx.layout();
    let (a, b, d) = (fx.by_id(&t, "a"), fx.by_id(&t, "b"), fx.by_id(&t, "d"));
    assert_near!(a.rect.y, b.rect.y);
    assert_near!(d.rect.y, a.rect.y + 20.0);
    assert_near!(d.rect.x, a.rect.x);
    assert_near!(fx.by_id(&t, "c").rect.h, 40.0);
}

#[test]
fn flex_justify_content() {
    let html = "<div id=c><div id=a>A</div><div id=b>B</div></div>";
    let css = "#c { display: flex; width: 600px } #c > div { width: 100px; height: 10px }";
    let mut fx = Fx::new(html, css);
    fx.patch("c", |s| s.justify_content = ContentDistribution::SpaceBetween);
    let t = fx.layout();
    let cx = fx.by_id(&t, "c").content_rect.x;
    assert_near!(fx.by_id(&t, "a").rect.x, cx);
    assert_near!(fx.by_id(&t, "b").rect.x, cx + 500.0);

    fx.patch("c", |s| s.justify_content = ContentDistribution::Center);
    let t = fx.layout();
    assert_near!(fx.by_id(&t, "a").rect.x, cx + 200.0);
    assert_near!(fx.by_id(&t, "b").rect.x, cx + 300.0);

    fx.patch("c", |s| s.justify_content = ContentDistribution::SpaceEvenly);
    let t = fx.layout();
    assert_near!(fx.by_id(&t, "a").rect.x, cx + 400.0 / 3.0);
    assert_near!(fx.by_id(&t, "b").rect.x, cx + 800.0 / 3.0 + 100.0);

    fx.patch("c", |s| s.justify_content = ContentDistribution::FlexEnd);
    let t = fx.layout();
    assert_near!(fx.by_id(&t, "b").rect.x, cx + 500.0);
}

#[test]
fn flex_align_items() {
    let html = "<div id=c><div id=a>A</div></div>";
    let css = "#c { display: flex; width: 600px; height: 200px } #a { width: 100px; height: 50px }";
    let mut fx = Fx::new(html, css);
    fx.patch("c", |s| s.align_items = ItemAlign::Center);
    let t = fx.layout();
    let cy = fx.by_id(&t, "c").content_rect.y;
    assert_near!(fx.by_id(&t, "a").rect.y, cy + 75.0);
    fx.patch("c", |s| s.align_items = ItemAlign::FlexEnd);
    let t = fx.layout();
    assert_near!(fx.by_id(&t, "a").rect.y, cy + 150.0);
    // stretch with auto height
    let mut fx = Fx::new(html, "#c { display: flex; width: 600px; height: 200px } #a { width: 100px }");
    fx.patch("c", |s| s.align_items = ItemAlign::Stretch);
    let t = fx.layout();
    assert_near!(fx.by_id(&t, "a").rect.h, 200.0);
}

#[test]
fn flex_column_splits_height() {
    let mut fx = Fx::new(FLEX3, "#c { display: flex; width: 600px; height: 300px }");
    fx.patch("c", |s| s.flex_direction = FlexDirection::Column);
    grow_all(&mut fx, &["a", "b", "d"], 1.0);
    let t = fx.layout();
    let (a, b, d) = (fx.by_id(&t, "a"), fx.by_id(&t, "b"), fx.by_id(&t, "d"));
    assert_near!(a.rect.h, 100.0);
    assert_near!(b.rect.h, 100.0);
    assert_near!(d.rect.h, 100.0);
    assert_near!(b.rect.y, a.rect.y + 100.0);
    assert_near!(d.rect.y, a.rect.y + 200.0);
    // Column items stretch to the container width.
    assert_near!(a.rect.w, 600.0);
}

#[test]
fn flex_column_reverse() {
    let mut fx = Fx::new(FLEX3, "#c { display: flex; width: 600px; height: 300px } #c > div { height: 50px }");
    fx.patch("c", |s| s.flex_direction = FlexDirection::ColumnReverse);
    let t = fx.layout();
    let (a, d) = (fx.by_id(&t, "a"), fx.by_id(&t, "d"));
    let cb = fx.by_id(&t, "c").content_rect.bottom();
    assert_near!(a.rect.bottom(), cb);
    assert!(d.rect.y < a.rect.y);
}

#[test]
fn flex_order() {
    let mut fx = Fx::new(FLEX3, "#c { display: flex; width: 600px } #c > div { width: 100px; height: 20px }");
    fx.patch("b", |s| s.order = -1);
    let t = fx.layout();
    assert!(fx.by_id(&t, "b").rect.x < fx.by_id(&t, "a").rect.x);
    assert!(fx.by_id(&t, "a").rect.x < fx.by_id(&t, "d").rect.x);
}

#[test]
fn flex_auto_margin() {
    let html = "<div id=c><div id=a>A</div><div id=b>B</div></div>";
    let css = "#c { display: flex; width: 600px } #c > div { width: 100px; height: 10px } #b { margin-left: auto }";
    let fx = Fx::new(html, css);
    let t = fx.layout();
    let c = fx.by_id(&t, "c");
    assert_near!(fx.by_id(&t, "b").rect.right(), c.content_rect.right());
    assert_near!(fx.by_id(&t, "a").rect.x, c.content_rect.x);
}

#[test]
fn flex_gap() {
    let html = "<div id=c><div id=a>A</div><div id=b>B</div></div>";
    let css = "#c { display: flex; width: 600px } #c > div { width: 100px; height: 10px }";
    let mut fx = Fx::new(html, css);
    fx.patch("c", |s| s.column_gap = Length::Px(20.0));
    let t = fx.layout();
    assert_near!(fx.by_id(&t, "b").rect.x, fx.by_id(&t, "a").rect.x + 120.0);
}

#[test]
fn flex_nested() {
    let html = "<div id=outer><div id=left>L</div><div id=inner><div id=i1>1</div><div id=i2>2</div></div></div>";
    let css = "#outer { display: flex; width: 600px } #left { width: 200px; height: 10px } #inner { display: flex } #inner > div { height: 10px; width: 0 }";
    let mut fx = Fx::new(html, css);
    fx.patch("inner", |s| s.flex_grow = 1.0);
    fx.patch("i1", |s| s.flex_grow = 1.0);
    fx.patch("i2", |s| s.flex_grow = 1.0);
    let t = fx.layout();
    assert_near!(fx.by_id(&t, "inner").rect.w, 400.0);
    assert_near!(fx.by_id(&t, "i1").rect.w, 200.0);
    assert_near!(fx.by_id(&t, "i2").rect.w, 200.0);
    assert_near!(fx.by_id(&t, "i2").rect.x, fx.by_id(&t, "left").rect.x + 400.0);
}

#[test]
fn flex_min_width_auto() {
    let html = "<div id=c><div id=a>Supercalifragilisticexpialidocious</div><div id=b>B</div></div>";
    let css = "#c { display: flex; width: 120px } #c > div { height: 10px }";
    let mut fx = Fx::new(html, css);
    fx.patch("a", |s| s.min_width = Length::Auto);
    fx.patch("b", |s| s.min_width = Length::Px(0.0));
    let t = fx.layout();
    // The long word cannot shrink the item below its min-content width.
    let a = fx.by_id(&t, "a");
    assert!(a.rect.w > 120.0, "a.w = {}", a.rect.w);
    let mut fx2 = Fx::new(html, css);
    fx2.patch("a", |s| s.min_width = Length::Px(0.0));
    fx2.patch("b", |s| s.min_width = Length::Px(0.0));
    let t2 = fx2.layout();
    let (a2, b2) = (fx2.by_id(&t2, "a"), fx2.by_id(&t2, "b"));
    assert!(a2.rect.w < 120.0, "a.w = {}", a2.rect.w);
    assert_near!(a2.rect.w + b2.rect.w, 120.0);
}

#[test]
fn flex_align_self() {
    let html = "<div id=c><div id=a>A</div><div id=b>B</div></div>";
    let css = "#c { display: flex; width: 600px; height: 100px } #c > div { width: 50px; height: 20px }";
    let mut fx = Fx::new(html, css);
    fx.patch("b", |s| s.align_self = ItemAlign::FlexEnd);
    let t = fx.layout();
    let cy = fx.by_id(&t, "c").content_rect.y;
    assert_near!(fx.by_id(&t, "a").rect.y, cy);
    assert_near!(fx.by_id(&t, "b").rect.y, cy + 80.0);
}

#[test]
fn flex_percentage_width() {
    let html = "<div id=c><div id=a>A</div><div id=b>B</div></div>";
    let css = "#c { display: flex; width: 600px } #a { width: 50%; height: 10px } #b { width: 25%; height: 10px }";
    let fx = Fx::new(html, css);
    let t = fx.layout();
    assert_near!(fx.by_id(&t, "a").rect.w, 300.0);
    assert_near!(fx.by_id(&t, "b").rect.w, 150.0);
}

#[test]
fn flex_baseline_alignment() {
    let html = "<div id=c><div id=a>A</div><div id=b>B</div></div>";
    let css = "#c { display: flex; width: 600px } #a { font-size: 40px } #b { font-size: 12px }";
    let mut fx = Fx::new(html, css);
    fx.patch("c", |s| s.align_items = ItemAlign::Baseline);
    let t = fx.layout();
    let ba = fx.by_id(&t, "a").first_baseline().unwrap();
    let bb = fx.by_id(&t, "b").first_baseline().unwrap();
    assert_near!(ba, bb);
    assert!(fx.by_id(&t, "b").rect.y > fx.by_id(&t, "a").rect.y);
}

#[test]
fn flex_inline_flex_shrinks_to_fit() {
    let html = "<div><span id=c><span id=a>A</span><span id=b>B</span></span></div>";
    let css = "#c { display: inline-flex } #c > span { width: 40px; height: 10px; display: block }";
    let fx = Fx::new(html, css);
    let t = fx.layout();
    assert_near!(fx.by_id(&t, "c").rect.w, 80.0);
}

// --------------------------------------------------------------------- grid

fn grid3(html: &str, css: &str) -> Fx {
    let mut fx = Fx::new(html, css);
    fx.patch("g", |s| s.grid_template_columns = vec![px(100.0), fr(1.0), fr(2.0)]);
    fx
}

#[test]
fn grid_fixed_and_fr_columns() {
    let fx = grid3("<div id=g><div id=a>A</div><div id=b>B</div><div id=c>C</div></div>", "#g { display: grid; width: 700px } #g > div { height: 10px }");
    let t = fx.layout();
    let (a, b, c) = (fx.by_id(&t, "a"), fx.by_id(&t, "b"), fx.by_id(&t, "c"));
    assert_near!(a.rect.w, 100.0);
    assert_near!(b.rect.w, 200.0);
    assert_near!(c.rect.w, 400.0);
    assert_near!(b.rect.x, a.rect.x + 100.0);
    assert_near!(c.rect.x, a.rect.x + 300.0);
    assert_near!(a.rect.y, c.rect.y);
}

#[test]
fn grid_repeat_auto_placement() {
    let html = "<div id=g><div id=i1>1</div><div>2</div><div>3</div><div id=i4>4</div><div>5</div><div>6</div><div id=i7>7</div></div>";
    let mut fx = Fx::new(html, "#g { display: grid; width: 600px } #g > div { height: 20px }");
    fx.patch("g", |s| {
        s.grid_template_columns = vec![TrackListItem::Repeat {
            count: Some(3),
            auto_fit: false,
            tracks: vec![fr(1.0)],
        }]
    });
    let t = fx.layout();
    let (i1, i4, i7) = (fx.by_id(&t, "i1"), fx.by_id(&t, "i4"), fx.by_id(&t, "i7"));
    assert_near!(i1.rect.w, 200.0);
    assert_near!(i4.rect.y, i1.rect.y + 20.0);
    assert_near!(i7.rect.y, i1.rect.y + 40.0);
    assert_near!(i4.rect.x, i1.rect.x);
    assert_near!(fx.by_id(&t, "g").rect.h, 60.0);
}

#[test]
fn grid_template_areas() {
    let html = "<div id=g><div id=main>M</div><div id=head>H</div><div id=nav>N</div></div>";
    let css = "#g { display: grid; width: 600px } #g > div { height: 30px }";
    let mut fx = Fx::new(html, css);
    fx.patch("g", |s| {
        s.grid_template_columns = vec![px(100.0), fr(1.0)];
        s.grid_template_areas = vec![vec!["head".into(), "head".into()], vec!["nav".into(), "main".into()]];
    });
    for id in ["head", "nav", "main"] {
        fx.patch(id, |s| {
            s.grid_row_start = GridLine::Named(id.into(), false);
            s.grid_row_end = GridLine::Named(id.into(), false);
            s.grid_column_start = GridLine::Named(id.into(), false);
            s.grid_column_end = GridLine::Named(id.into(), false);
        });
    }
    let t = fx.layout();
    let (g, head, nav, main) = (fx.by_id(&t, "g"), fx.by_id(&t, "head"), fx.by_id(&t, "nav"), fx.by_id(&t, "main"));
    assert_near!(head.rect.w, 600.0);
    assert_near!(head.rect.y, g.content_rect.y);
    assert_near!(nav.rect.w, 100.0);
    assert_near!(nav.rect.y, g.content_rect.y + 30.0);
    assert_near!(main.rect.x, g.content_rect.x + 100.0);
    assert_near!(main.rect.w, 500.0);
    assert_near!(main.rect.y, nav.rect.y);
}

#[test]
fn grid_explicit_span_and_negative_lines() {
    let html = "<div id=g><div id=a>A</div><div id=b>B</div><div id=c>C</div></div>";
    let mut fx = grid3(html, "#g { display: grid; width: 700px } #g > div { height: 10px }");
    fx.patch("a", |s| {
        s.grid_column_start = GridLine::Line(1);
        s.grid_column_end = GridLine::Line(3);
    });
    fx.patch("c", |s| {
        s.grid_column_start = GridLine::Line(1);
        s.grid_column_end = GridLine::Line(-1);
    });
    let t = fx.layout();
    let (a, b, c) = (fx.by_id(&t, "a"), fx.by_id(&t, "b"), fx.by_id(&t, "c"));
    assert_near!(a.rect.w, 300.0);
    assert_near!(b.rect.x, a.rect.x + 300.0);
    assert_near!(b.rect.w, 400.0);
    assert_near!(c.rect.w, 700.0);
    assert_near!(c.rect.y, a.rect.y + 10.0);
}

#[test]
fn grid_span_auto_placement() {
    let html = "<div id=g><div id=a>A</div><div id=b>B</div><div id=c>C</div></div>";
    let mut fx = Fx::new(html, "#g { display: grid; width: 600px } #g > div { height: 10px }");
    fx.patch("g", |s| s.grid_template_columns = vec![fr(1.0), fr(1.0), fr(1.0)]);
    fx.patch("b", |s| {
        s.grid_column_start = GridLine::Span(2);
    });
    let t = fx.layout();
    let (a, b, c) = (fx.by_id(&t, "a"), fx.by_id(&t, "b"), fx.by_id(&t, "c"));
    assert_near!(a.rect.w, 200.0);
    assert_near!(b.rect.w, 400.0);
    assert_near!(b.rect.x, a.rect.x + 200.0);
    assert_near!(c.rect.y, a.rect.y + 10.0);
    assert_near!(c.rect.x, a.rect.x);
}

#[test]
fn grid_auto_fill_minmax() {
    let html = "<div id=g><div id=a>A</div><div>B</div><div>C</div><div>D</div><div id=e>E</div></div>";
    let mut fx = Fx::new(html, "#g { display: grid; width: 450px } #g > div { height: 10px }");
    fx.patch("g", |s| {
        s.grid_template_columns = vec![TrackListItem::Repeat {
            count: None,
            auto_fit: false,
            tracks: vec![tr(TrackSize::MinMax(
                Box::new(TrackSize::Length(Length::Px(100.0))),
                Box::new(TrackSize::Fr(1.0)),
            ))],
        }]
    });
    let t = fx.layout();
    let (a, e) = (fx.by_id(&t, "a"), fx.by_id(&t, "e"));
    assert_near!(a.rect.w, 112.5);
    assert_near!(e.rect.y, a.rect.y + 10.0);
    assert_near!(e.rect.x, a.rect.x);
}

#[test]
fn grid_auto_fit_collapses_empty_tracks() {
    let html = "<div id=g><div id=a>A</div><div id=b>B</div></div>";
    let mut fx = Fx::new(html, "#g { display: grid; width: 450px } #g > div { height: 10px }");
    fx.patch("g", |s| {
        s.grid_template_columns = vec![TrackListItem::Repeat {
            count: None,
            auto_fit: true,
            tracks: vec![tr(TrackSize::MinMax(
                Box::new(TrackSize::Length(Length::Px(100.0))),
                Box::new(TrackSize::Fr(1.0)),
            ))],
        }]
    });
    let t = fx.layout();
    assert_near!(fx.by_id(&t, "a").rect.w, 225.0);
    assert_near!(fx.by_id(&t, "b").rect.w, 225.0);
}

#[test]
fn grid_gaps() {
    let html = "<div id=g><div id=a>A</div><div id=b>B</div><div id=c>C</div></div>";
    let mut fx = Fx::new(html, "#g { display: grid; width: 500px } #g > div { height: 10px }");
    fx.patch("g", |s| {
        s.grid_template_columns = vec![fr(1.0), fr(1.0)];
        s.column_gap = Length::Px(20.0);
        s.row_gap = Length::Px(30.0);
    });
    let t = fx.layout();
    let (a, b, c) = (fx.by_id(&t, "a"), fx.by_id(&t, "b"), fx.by_id(&t, "c"));
    assert_near!(a.rect.w, 240.0);
    assert_near!(b.rect.x, a.rect.x + 260.0);
    assert_near!(c.rect.y, a.rect.y + 40.0);
    assert_near!(fx.by_id(&t, "g").rect.h, 50.0);
}

#[test]
fn grid_auto_rows_implicit() {
    let html = "<div id=g><div id=a>A</div><div>B</div><div id=c>C</div><div>D</div></div>";
    let mut fx = Fx::new(html, "#g { display: grid; width: 400px }");
    fx.patch("g", |s| {
        s.grid_template_columns = vec![fr(1.0), fr(1.0)];
        s.grid_auto_rows = vec![TrackSize::Length(Length::Px(50.0))];
    });
    let t = fx.layout();
    let (a, c) = (fx.by_id(&t, "a"), fx.by_id(&t, "c"));
    assert_near!(a.rect.h, 50.0);
    assert_near!(c.rect.y, a.rect.y + 50.0);
    assert_near!(fx.by_id(&t, "g").rect.h, 100.0);
}

#[test]
fn grid_align_items_center() {
    let html = "<div id=g><div id=a>A</div><div id=b>B</div></div>";
    let mut fx = Fx::new(html, "#g { display: grid; width: 400px } #a { height: 100px } #b { height: 20px }");
    fx.patch("g", |s| {
        s.grid_template_columns = vec![fr(1.0), fr(1.0)];
        s.align_items = ItemAlign::Center;
    });
    let t = fx.layout();
    let (a, b) = (fx.by_id(&t, "a"), fx.by_id(&t, "b"));
    assert_near!(b.rect.y, a.rect.y + 40.0);
    // justify-self end
    fx.patch("b", |s| {
        s.justify_self = ItemAlign::FlexEnd;
        s.width = Length::Px(50.0);
    });
    let t = fx.layout();
    assert_near!(fx.by_id(&t, "b").rect.right(), fx.by_id(&t, "g").content_rect.right());
}

#[test]
fn grid_dense_packing() {
    let html = "<div id=g><div id=a>A</div><div id=b>B</div><div id=c>C</div></div>";
    let css = "#g { display: grid; width: 600px } #g > div { height: 10px }";
    let mk = |dense: bool| {
        let mut fx = Fx::new(html, css);
        fx.patch("g", |s| {
            s.grid_template_columns = vec![fr(1.0), fr(1.0), fr(1.0)];
            s.grid_auto_flow = if dense { GridAutoFlow::RowDense } else { GridAutoFlow::Row };
        });
        fx.patch("a", |s| s.grid_column_start = GridLine::Span(2));
        fx.patch("b", |s| s.grid_column_start = GridLine::Span(2));
        fx
    };
    let fx = mk(false);
    let t = fx.layout();
    assert_near!(fx.by_id(&t, "c").rect.y, fx.by_id(&t, "b").rect.y);
    let fx = mk(true);
    let t = fx.layout();
    assert_near!(fx.by_id(&t, "c").rect.y, fx.by_id(&t, "a").rect.y);
    assert_near!(fx.by_id(&t, "c").rect.x, fx.by_id(&t, "a").rect.x + 400.0);
}

#[test]
fn grid_justify_content_center() {
    let html = "<div id=g><div id=a>A</div><div id=b>B</div></div>";
    let mut fx = Fx::new(html, "#g { display: grid; width: 500px } #g > div { height: 10px }");
    fx.patch("g", |s| {
        s.grid_template_columns = vec![px(100.0), px(100.0)];
        s.justify_content = ContentDistribution::Center;
    });
    let t = fx.layout();
    let g = fx.by_id(&t, "g");
    assert_near!(fx.by_id(&t, "a").rect.x, g.content_rect.x + 150.0);
    assert_near!(fx.by_id(&t, "b").rect.x, g.content_rect.x + 250.0);
}

#[test]
fn grid_intrinsic_tracks() {
    let html = "<div id=g><div id=a>Hello</div><div id=b>B</div></div>";
    let mut fx = Fx::new(html, "#g { display: grid; width: 600px } #g > div { height: 10px }");
    fx.patch("g", |s| s.grid_template_columns = vec![tr(TrackSize::Length(Length::MaxContent)), fr(1.0)]);
    let t = fx.layout();
    let (a, b) = (fx.by_id(&t, "a"), fx.by_id(&t, "b"));
    assert!(a.rect.w > 20.0 && a.rect.w < 100.0, "a.w = {}", a.rect.w);
    assert_near!(b.rect.w, 600.0 - a.rect.w);
    // auto track without fr: sized to content, remaining space stretches it.
    fx.patch("g", |s| s.grid_template_columns = vec![tr(TrackSize::Length(Length::Auto)), px(100.0)]);
    let t = fx.layout();
    assert_near!(fx.by_id(&t, "a").rect.w, 500.0);
}

#[test]
fn grid_in_flex() {
    let html = "<div id=f><div id=left>L</div><div id=g><div id=a>A</div><div id=b>B</div></div></div>";
    let css = "#f { display: flex; width: 600px } #left { width: 100px; height: 10px } #g { display: grid } #g > div { height: 10px }";
    let mut fx = Fx::new(html, css);
    fx.patch("g", |s| {
        s.flex_grow = 1.0;
        s.grid_template_columns = vec![fr(1.0), fr(3.0)];
    });
    let t = fx.layout();
    assert_near!(fx.by_id(&t, "g").rect.w, 500.0);
    assert_near!(fx.by_id(&t, "a").rect.w, 125.0);
    assert_near!(fx.by_id(&t, "b").rect.w, 375.0);
}

#[test]
fn grid_inline_grid_shrinks_to_fit() {
    let html = "<div><span id=g><span id=a>A</span><span id=b>B</span></span></div>";
    let css = "#g { display: inline-grid } #g > span { display: block; width: 40px; height: 10px }";
    let mut fx = Fx::new(html, css);
    fx.patch("g", |s| s.grid_template_columns = vec![px(50.0), px(60.0)]);
    let t = fx.layout();
    assert_near!(fx.by_id(&t, "g").rect.w, 110.0);
}

#[test]
fn grid_perf_1000_items() {
    let mut html = String::from("<div id=g>");
    for i in 0..1000 {
        html.push_str(&format!("<div>item {i}</div>"));
    }
    html.push_str("</div>");
    let mut fx = Fx::new(&html, "#g { display: grid; width: 800px } #g > div { height: 20px }");
    fx.patch("g", |s| {
        s.grid_template_columns = vec![TrackListItem::Repeat {
            count: None,
            auto_fit: false,
            tracks: vec![tr(TrackSize::MinMax(
                Box::new(TrackSize::Length(Length::Px(80.0))),
                Box::new(TrackSize::Fr(1.0)),
            ))],
        }]
    });
    let start = std::time::Instant::now();
    let t = fx.layout();
    let elapsed = start.elapsed();
    assert!(elapsed.as_secs_f32() < 1.0, "took {elapsed:?}");
    // 10 columns of 80px → 100 rows.
    assert_near!(fx.by_id(&t, "g").rect.h, 2000.0);
}

// --------------------------------------------------------------- grid lanes

#[test]
fn grid_lanes_columns_fill_shortest_lane() {
    let html = "<div id=g><div id=a style='height:50px'></div><div id=b style='height:20px'></div><div id=c style='height:30px'></div><div id=d style='height:10px'></div></div>";
    let css = "#g { display: grid-lanes; grid-template-columns: 100px 100px; gap: 10px; width: 210px; fit-tolerance: 0 }";
    let fx = Fx::new(html, css);
    let t = fx.layout();
    let (a, b, c, d) = (fx.by_id(&t, "a"), fx.by_id(&t, "b"), fx.by_id(&t, "c"), fx.by_id(&t, "d"));
    assert_near!(b.rect.x, a.rect.x + 110.0);
    // c goes under b (the shorter lane), d under a.
    assert_near!(c.rect.x, b.rect.x);
    assert_near!(c.rect.y, b.rect.y + 30.0);
    assert_near!(d.rect.x, a.rect.x);
    assert_near!(d.rect.y, a.rect.y + 60.0);
    assert_near!(fx.by_id(&t, "g").rect.h, 70.0);
}

#[test]
fn grid_lanes_auto_tracks_share_space_with_gaps() {
    let html = "<div id=g><div id=a>Some text here</div><div id=b>More words in this one</div><div id=c>x</div><div id=d style='grid-column: span 2'>A spanning item with some long text in it</div></div>";
    let css = "#g { display: grid-lanes; grid-template-columns: auto auto auto; gap: 20px; width: 340px; padding: 20px } #g > div { padding: 10px }";
    let fx = Fx::new(html, css);
    let t = fx.layout();
    let (a, b, c) = (fx.by_id(&t, "a"), fx.by_id(&t, "b"), fx.by_id(&t, "c"));
    assert_near!(c.rect.x + c.rect.w - a.rect.x, 340.0);
    assert_near!(b.rect.x - (a.rect.x + a.rect.w), 20.0);
}


// ------------------------------------------------------------ writing modes

fn find_wrapper(b: &LayoutBox) -> Option<&LayoutBox> {
    if b.wm_wrapper {
        return Some(b);
    }
    b.children.iter().find_map(find_wrapper)
}

#[test]
fn vertical_rl_block_is_sized_from_its_columns() {
    let html = "<body style='margin:0'><div id=v style='writing-mode: vertical-rl; height: 100px'><div style='width: 20px'></div><div style='width: 30px'></div></div><div id=after style='height:10px'></div></body>";
    let fx = Fx::new(html, "");
    let t = fx.layout();
    let w = find_wrapper(&t.root).expect("wrapper");
    assert_near!(w.rect.w, 50.0);
    assert_near!(w.rect.h, 100.0);
    assert_near!(w.rect.x, 0.0);
    // The next block follows below.
    assert_near!(fx.by_id(&t, "after").rect.y, 100.0);
    // The first child (block-start) maps to the right: logical y 0..20.
    let m = w.wm_transform.expect("transform");
    let first = &w.children[0].children[0];
    let (x0, _) = crate::tree::map_point(m, first.rect.x, first.rect.y);
    assert_near!(x0, 50.0);
}

#[test]
fn vertical_lr_root_flows_left_to_right() {
    let html = "<html style='writing-mode: vertical-lr'><body style='margin:0'><div style='width: 20px'></div><div id=b style='width: 30px'></div></body></html>";
    let fx = Fx::new(html, "");
    let t = fx.layout();
    assert!(t.root.wm_wrapper);
    let m = t.root.wm_transform.expect("transform");
    let b = fx.by_id(&t, "b");
    let (x0, y0) = crate::tree::map_point(m, b.rect.x, b.rect.y);
    assert_near!(x0, 20.0);
    assert_near!(y0, 0.0);
    // Inline size: the viewport height.
    assert_near!(b.rect.w, 600.0);
}

// -------------------------------------------------------------------- bidi

fn line_texts(b: &LayoutBox) -> Vec<(f32, String)> {
    let mut out: Vec<(f32, String)> = b
        .lines
        .iter()
        .flat_map(|l| l.fragments.iter())
        .filter_map(|f| match &f.kind {
            crate::tree::FragmentKind::Text { text, .. } => Some((f.rect.x, text.to_string())),
            _ => None,
        })
        .collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

#[test]
fn bidi_ltr_words_keep_their_order_in_rtl_paragraphs() {
    // Hebrew, a number and English in a right-to-left paragraph: the
    // Hebrew word is rightmost, the English run reads left to right.
    let fx = Fx::new("<p id=p dir=rtl>\u{5E9}\u{5DC}\u{5D5}\u{5DD} 12 abc def</p>", "p { width: 600px; margin: 0 }");
    let t = fx.layout();
    let texts = line_texts(fx.by_id(&t, "p"));
    let words: Vec<String> = texts.iter().map(|(_, s)| s.trim_start_matches(['\u{202E}', '\u{202D}']).trim().to_string()).filter(|s| !s.is_empty()).collect();
    assert_eq!(words, vec!["abc def", "12", "\u{5E9}\u{5DC}\u{5D5}\u{5DD}"], "{texts:?}");
}

#[test]
fn bidi_override_reverses_latin() {
    let fx = Fx::new("<p id=p><bdo dir=rtl>abc</bdo></p>", "p { margin: 0 }");
    let t = fx.layout();
    let texts = line_texts(fx.by_id(&t, "p"));
    // The run is shaped right to left (RLO prefix for the shaper).
    assert!(texts.iter().any(|(_, s)| s == "\u{202E}abc"), "{texts:?}");
}

#[test]
fn horizontal_block_inside_vertical_rl_keeps_its_physical_size() {
    let html = "<body style='margin:0'><div style='writing-mode: vertical-rl; height: 200px'><div id=h style='writing-mode: horizontal-tb; width: 50px; height: 30px'></div></div></body>";
    let fx = Fx::new(html, "");
    let t = fx.layout();
    let w1 = find_wrapper(&t.root).expect("outer wrapper");
    let m1 = w1.wm_transform.expect("outer transform");
    let w2 = find_wrapper(&w1.children[0]).expect("inner wrapper");
    let m2 = w2.wm_transform.expect("inner transform");
    let h = fx.by_id(&t, "h");
    let map = |x: f32, y: f32| {
        let (a, b) = crate::tree::map_point(m2, x, y);
        crate::tree::map_point(m1, a, b)
    };
    let (ax, ay) = map(h.rect.x, h.rect.y);
    let (bx, by) = map(h.rect.x + h.rect.w, h.rect.y + h.rect.h);
    let (x0, x1, y0, y1) = (ax.min(bx), ax.max(bx), ay.min(by), ay.max(by));
    assert_near!(x1 - x0, 50.0);
    assert_near!(y1 - y0, 30.0);
    assert_near!(y0, 0.0);
    assert_near!(w1.rect.w, 50.0);
    // Upright: the box's own top-left maps to the physical top-left.
    assert_near!(ax, x0);
    assert_near!(ay, y0);
}

#[test]
fn grid_items_share_a_baseline() {
    let html = "<div id=g><div id=a style='font-size: 10px; padding-top: 15px'>a</div><div id=b style='font-size: 30px'>b</div></div>";
    let css = "#g { display: grid; grid-template-columns: 100px 100px; align-items: baseline }";
    let fx = Fx::new(html, css);
    let t = fx.layout();
    let (a, b) = (fx.by_id(&t, "a"), fx.by_id(&t, "b"));
    let (ba, bb) = (a.first_baseline().unwrap(), b.first_baseline().unwrap());
    assert_near!(ba, bb);
    // The row grows to hold the shifted item.
    let g = fx.by_id(&t, "g");
    assert!(g.rect.bottom() + 0.5 >= a.rect.bottom().max(b.rect.bottom()));
}
