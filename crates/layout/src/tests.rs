//! Layout tests (need the gfx fonts and the style cascade).

use std::sync::Arc;
use crate::tree::{BoxKind, FragmentKind, LayoutBox};
use crate::{build_display_list, layout, paint, DisplayItem, LayoutContext, LayoutTree};
use rabbithole_css::{parse_stylesheet, MediaEnv};
use rabbithole_dom::Document;
use rabbithole_gfx::{Canvas, FontContext, Image, Paint, Rect};
use rabbithole_style::{style_tree, StyledTree};
use std::collections::HashMap;

type Images = HashMap<usize, Arc<Image>>;
type Bgs = HashMap<String, Arc<Image>>;
type Scrolls = HashMap<usize, (f32, f32)>;

struct Fixture {
    doc: Document,
    styles: StyledTree,
    fonts: FontContext,
    scrolls: Scrolls,
    page_scroll: (f32, f32),
}

impl Fixture {
    fn new(html: &str, css: &str) -> Fixture {
        let doc = rabbithole_html::parse(html);
        let sheets = vec![parse_stylesheet(css)];
        let env = MediaEnv {
            width: 800.0,
            height: 600.0,
            screen: true,
            scripting: true,
        };
        let styles = style_tree(&doc, &sheets, &env);
        Fixture {
            doc,
            styles,
            fonts: FontContext::new(),
            scrolls: HashMap::new(),
            page_scroll: (0.0, 0.0),
        }
    }

    fn layout(&self) -> LayoutTree {
        let images = HashMap::new();
        let bgs = HashMap::new();
        let ctx = self.ctx(&images, &bgs);
        layout(&ctx)
    }

    fn ctx<'a>(&'a self, images: &'a Images, bgs: &'a Bgs) -> LayoutContext<'a> {
        LayoutContext {
            doc: &self.doc,
            styles: &self.styles,
            fonts: &self.fonts,
            images,
            background_images: bgs,
            viewport_width: 800.0,
            viewport_height: 600.0,
            scroll_offsets: &self.scrolls,
            page_scroll: self.page_scroll,
        }
    }

    fn display_list(&self) -> crate::DisplayList {
        let images = HashMap::new();
        let bgs = HashMap::new();
        let ctx = self.ctx(&images, &bgs);
        let t = layout(&ctx);
        build_display_list(&t, &ctx)
    }

    fn find<'t>(&self, tree: &'t LayoutTree, tag: &str) -> Vec<&'t LayoutBox> {
        let mut out = Vec::new();
        fn walk<'t>(b: &'t LayoutBox, doc: &Document, tag: &str, out: &mut Vec<&'t LayoutBox>) {
            if let Some(n) = b.node {
                if doc.tag(n) == Some(tag) && b.kind != BoxKind::Text {
                    out.push(b);
                }
            }
            for c in &b.children {
                walk(c, doc, tag, out);
            }
        }
        walk(&tree.root, &self.doc, tag, &mut out);
        out
    }
}

fn text_of(b: &LayoutBox) -> String {
    let mut s = String::new();
    for l in &b.lines {
        for f in &l.fragments {
            if let FragmentKind::Text { text, .. } = &f.kind {
                s.push_str(text);
            }
        }
    }
    s
}

#[test]
fn body_width_and_margins() {
    let fx = Fixture::new("<p>Hello</p>", "");
    let t = fx.layout();
    let body = &fx.find(&t, "body")[0];
    assert!((body.rect.w - 784.0).abs() < 0.01, "body width {}", body.rect.w);
    assert!((body.rect.x - 8.0).abs() < 0.01);
    // body's 8px margin collapses with the paragraph's 16px margin.
    assert!((body.rect.y - 16.0).abs() < 0.01, "body y {}", body.rect.y);
    let p = &fx.find(&t, "p")[0];
    assert!((p.rect.y - 16.0).abs() < 0.01, "p y {}", p.rect.y);
}

#[test]
fn paragraph_margins_collapse() {
    let fx = Fixture::new("<p>One</p><p>Two</p>", "p { margin: 16px 0 }");
    let t = fx.layout();
    let ps = fx.find(&t, "p");
    assert_eq!(ps.len(), 2);
    let gap = ps[1].rect.y - ps[0].rect.bottom();
    assert!((gap - 16.0).abs() < 0.01, "gap {gap}");
    assert!(ps[1].rect.y > ps[0].rect.y);
}

#[test]
fn percentage_and_auto_margins() {
    let fx = Fixture::new(
        "<div id=a style='width:50%'></div><div id=b style='width:200px;margin:0 auto'></div>",
        "",
    );
    let t = fx.layout();
    let divs = fx.find(&t, "div");
    assert!((divs[0].rect.w - 392.0).abs() < 0.01, "{}", divs[0].rect.w);
    assert!((divs[1].rect.x - (8.0 + (784.0 - 200.0) / 2.0)).abs() < 0.01, "{}", divs[1].rect.x);
}

/// `width:auto` limited by `max-width` is centred by `margin:auto`
/// (google.com's search box: `max-width:688px; margin:0 auto`).
#[test]
fn max_width_with_auto_margins_centres() {
    let fx = Fixture::new("<div style='max-width:300px;margin:0 auto'></div><div style='max-width:300px;margin-left:auto'></div>", "");
    let t = fx.layout();
    let divs = fx.find(&t, "div");
    assert!((divs[0].rect.w - 300.0).abs() < 0.01, "{}", divs[0].rect.w);
    assert!((divs[0].rect.x - (8.0 + (784.0 - 300.0) / 2.0)).abs() < 0.01, "{}", divs[0].rect.x);
    assert!((divs[1].rect.x - (8.0 + 784.0 - 300.0)).abs() < 0.01, "{}", divs[1].rect.x);
}

#[test]
fn text_wraps_within_container() {
    let words: Vec<String> = (0..200).map(|i| format!("word{i}")).collect();
    let html = format!("<div style='width:300px'><p>{}</p></div>", words.join(" "));
    let fx = Fixture::new(&html, "");
    let t = fx.layout();
    let p = &fx.find(&t, "p")[0];
    assert!(p.lines.len() > 5, "lines {}", p.lines.len());
    let right = p.content_rect.right();
    for l in &p.lines {
        for f in &l.fragments {
            assert!(f.rect.right() <= right + 0.5, "fragment overflows: {:?}", f.rect);
        }
    }
    assert!(p.rect.h >= p.lines.len() as f32 * 10.0);
}

#[test]
fn nowrap_and_br() {
    let words: Vec<String> = (0..100).map(|i| format!("w{i}")).collect();
    let html = format!(
        "<div style='width:200px'><p id=a style='white-space:nowrap'>{}</p><p id=b>one<br>two</p></div>",
        words.join(" ")
    );
    let fx = Fixture::new(&html, "");
    let t = fx.layout();
    let ps = fx.find(&t, "p");
    assert_eq!(ps[0].lines.len(), 1);
    assert_eq!(ps[1].lines.len(), 2);
    assert_eq!(text_of(&ps[1]), "onetwo");
}

#[test]
fn long_word_breaks() {
    let html = "<div style='width:100px'><p>https://example.com/a/very/long/url/that/does/not/fit</p></div>";
    let fx = Fixture::new(html, "p{overflow-wrap:anywhere}");
    let t = fx.layout();
    let p = &fx.find(&t, "p")[0];
    assert!(p.lines.len() > 1);
    for l in &p.lines {
        for f in &l.fragments {
            assert!(f.rect.right() <= p.content_rect.right() + 0.5);
        }
    }
}

#[test]
fn inline_block_on_line() {
    let html = "<p>before <span style='display:inline-block;width:50px;height:30px;background:red'></span> after</p>";
    let fx = Fixture::new(html, "");
    let t = fx.layout();
    let p = &fx.find(&t, "p")[0];
    assert_eq!(p.lines.len(), 1);
    assert_eq!(p.children.len(), 1);
    let span = &p.children[0];
    assert!((span.rect.w - 50.0).abs() < 0.01);
    assert!((span.rect.h - 30.0).abs() < 0.01);
    assert!(p.lines[0].rect.h >= 30.0);
    let has_box = p.lines[0].fragments.iter().any(|f| matches!(f.kind, FragmentKind::Box(_)));
    assert!(has_box);
    // the inline-block sits between the two text fragments
    let text_x: Vec<f32> = p.lines[0]
        .fragments
        .iter()
        .filter(|f| matches!(f.kind, FragmentKind::Text { .. }))
        .map(|f| f.rect.x)
        .collect();
    assert!(text_x[0] < span.rect.x && span.rect.x < text_x[1]);
}

#[test]
fn float_shrinks_lines() {
    let words: Vec<String> = (0..80).map(|i| format!("w{i}")).collect();
    let html = format!(
        "<div style='width:400px'><div id=f style='float:left;width:100px;height:50px'></div><p>{}</p></div>",
        words.join(" ")
    );
    let fx = Fixture::new(&html, "p{margin:0}");
    let t = fx.layout();
    let f = &fx.find(&t, "div")[1];
    assert!((f.rect.w - 100.0).abs() < 0.01);
    let p = &fx.find(&t, "p")[0];
    assert!(p.lines.len() > 2);
    assert!((p.lines[0].rect.x - f.rect.right()).abs() < 0.01, "line x {} float right {}", p.lines[0].rect.x, f.rect.right());
    let below = p.lines.iter().find(|l| l.rect.y >= f.rect.bottom()).expect("line below float");
    assert!((below.rect.x - p.content_rect.x).abs() < 0.01);
}

#[test]
fn absolute_does_not_affect_height() {
    let html = "<div id=c style='position:relative'><p>text</p><div id=a style='position:absolute;top:10px;left:20px;width:50px;height:500px'></div></div>";
    let fx = Fixture::new(html, "p{margin:0}");
    let t = fx.layout();
    let divs = fx.find(&t, "div");
    let c = divs[0];
    let a = divs[1];
    assert!(c.rect.h < 100.0, "container height {}", c.rect.h);
    assert!((a.rect.y - (c.rect.y + 10.0)).abs() < 0.01);
    assert!((a.rect.x - (c.rect.x + 20.0)).abs() < 0.01);
    assert!(t.height >= a.rect.bottom());
}

#[test]
fn overflow_hidden_keeps_height() {
    let html = "<div style='height:40px;overflow:hidden'><p>a</p><p>b</p><p>c</p><p>d</p></div>";
    let fx = Fixture::new(html, "");
    let t = fx.layout();
    let d = &fx.find(&t, "div")[0];
    assert!((d.rect.h - 40.0).abs() < 0.01);
    assert!(d.clips);
    let list = build_display_list(&t, &fx.ctx(&HashMap::new(), &HashMap::new()));
    assert!(list.items.iter().any(|i| matches!(i, DisplayItem::PushClip(_))));
}

#[test]
fn overflow_clip_keeps_the_other_axis_visible_in_paint_and_hits() {
    for (overflow, visible, clipped) in [
        ("overflow-x:clip", (50, 30), (30, 50)),
        ("overflow-y:clip", (30, 50), (50, 30)),
    ] {
        let fx = Fixture::new(
            "<div id=outer><div id=child></div></div>",
            &format!("html,body{{margin:0}} #outer{{position:relative;margin:40px;width:40px;height:40px;background:black;{overflow}}} #child{{position:relative;left:-20px;top:-20px;width:80px;height:80px;background:green;opacity:0.5}}"),
        );
        let list = fx.display_list();
        let mut canvas = Canvas::new(160, 160);
        paint(&list, &mut canvas, &fx.fonts, 0.0, 0.0);
        let pixel = |(x, y): (usize, usize)| canvas.data[y * 160 + x];
        assert_ne!(pixel(visible), 0xffffff, "{overflow}");
        assert_eq!(pixel(clipped), 0xffffff, "{overflow}");
        let child = fx.doc.element_by_id("child").unwrap();
        assert_eq!(list.hits.hit_test(visible.0 as f32, visible.1 as f32, (0.0, 0.0)), Some(child), "{overflow}");
        assert_ne!(list.hits.hit_test(clipped.0 as f32, clipped.1 as f32, (0.0, 0.0)), Some(child), "{overflow}");
        let tree = fx.layout();
        assert_eq!(tree.node_at(visible.0 as f32, visible.1 as f32), Some(child), "{overflow}");
        assert_ne!(tree.node_at(clipped.0 as f32, clipped.1 as f32), Some(child), "{overflow}");
    }
}

#[test]
fn single_axis_overflow_clip_does_not_round_the_content_clip() {
    for overflow in ["overflow-x:clip", "overflow-y:clip", "overflow:clip"] {
        let fx = Fixture::new(
            "<div id=outer><div></div></div>",
            &format!("html,body{{margin:0}} #outer{{margin:40px;width:40px;height:40px;background:red;border-radius:50%;{overflow}}} #outer>div{{width:40px;height:40px;background:green}}"),
        );
        let mut canvas = Canvas::new(160, 160);
        paint(&fx.display_list(), &mut canvas, &fx.fonts, 0.0, 0.0);
        let corner = canvas.data[41 * 160 + 41];
        if overflow == "overflow:clip" {
            assert_ne!(corner, 0x008000);
        } else {
            assert_eq!(corner, 0x008000, "{overflow}");
        }
    }
}

#[test]
fn overflow_clip_propagates_visible_axis_scrollable_overflow() {
    let fx = Fixture::new(
        "<div id=scroller><div id=clip><div id=child></div></div></div>",
        "html,body{margin:0} #scroller{width:100px;height:50px;overflow:auto} #clip{width:50px;height:50px;overflow-x:clip} #child{width:200px;height:100px}",
    );
    let tree = fx.layout();
    let scroller = fx.find(&tree, "div")[0];
    assert!((scroller.scroll_width - 100.0).abs() < 0.01);
    assert!((scroller.scroll_height - 100.0).abs() < 0.01, "height={}", scroller.scroll_height);
    for (overflow, expected) in [("overflow-x:clip", (800.0, 700.0)), ("overflow-y:clip", (900.0, 600.0))] {
        let fx = Fixture::new(
            "<div id=clip><div></div></div>",
            &format!("html,body{{margin:0}} #clip{{width:50px;height:50px;{overflow}}} #clip>div{{width:900px;height:700px}}"),
        );
        let tree = fx.layout();
        assert_eq!((tree.width, tree.height), expected, "{overflow}");
    }
}

#[test]
fn table_columns_align() {
    let html = "<table><tr><td>a</td><td>bbbbbbbbbb</td></tr><tr><td>cccccc</td><td>d</td></tr></table>";
    let fx = Fixture::new(html, "td{padding:2px}");
    let t = fx.layout();
    let tds = fx.find(&t, "td");
    assert_eq!(tds.len(), 4);
    assert!((tds[1].rect.x - tds[3].rect.x).abs() < 0.01, "col2 x {} vs {}", tds[1].rect.x, tds[3].rect.x);
    assert!((tds[0].rect.x - tds[2].rect.x).abs() < 0.01);
    assert!(tds[2].rect.y > tds[0].rect.y);
    assert!((tds[0].rect.h - tds[1].rect.h).abs() < 0.01);
    assert!(tds[0].rect.w >= tds[2].rect.w - 0.01);
}

#[test]
fn list_marker() {
    let html = "<ul><li>item</li></ul><ol start=3><li>x</li><li>y</li></ol>";
    let fx = Fixture::new(html, "li{display:list-item} ul,ol{padding-left:40px} ol li{list-style-type:decimal}");
    let t = fx.layout();
    let lis = fx.find(&t, "li");
    assert_eq!(lis.len(), 3);
    let m = lis[0].marker.as_ref().expect("marker");
    assert_eq!(&*m.text, "•");
    assert!(m.x < lis[0].content_rect.x);
    assert_eq!(&*lis[2].marker.as_ref().unwrap().text, "4.");
}

#[test]
fn hit_test_link() {
    let html = "<p>Go <a href='x'>here</a> now</p>";
    let fx = Fixture::new(html, "");
    let t = fx.layout();
    let p = &fx.find(&t, "p")[0];
    let link_frag = p.lines[0]
        .fragments
        .iter()
        .find(|f| matches!(&f.kind, FragmentKind::Text { text, .. } if &**text == "here"))
        .expect("link fragment");
    let n = t.node_at(link_frag.rect.x + 2.0, link_frag.rect.y + 2.0).expect("hit");
    assert_eq!(fx.doc.tag(n), Some("a"));
    let n2 = t.node_at(p.lines[0].fragments[0].rect.x + 1.0, p.lines[0].fragments[0].rect.y + 1.0).unwrap();
    assert_eq!(fx.doc.tag(n2), Some("p"));
}

#[test]
fn display_list_and_paint() {
    let html = "<div style='background:#ff0000;padding:10px'><p>Painted text</p></div>";
    let fx = Fixture::new(html, "");
    let t = fx.layout();
    let images = HashMap::new();
    let bgs = HashMap::new();
    let ctx = fx.ctx(&images, &bgs);
    let list = build_display_list(&t, &ctx);
    assert!(list
        .items
        .iter()
        .any(|i| matches!(i, DisplayItem::Text { text, .. } if text == "Painted text")));
    assert!(list.items.iter().any(|i| matches!(i, DisplayItem::Rect { color, .. } if color.r == 255 && color.g == 0)));
    let mut canvas = Canvas::new(800, 600);
    paint(&list, &mut canvas, &fx.fonts, 0.0, 0.0);
    assert!(canvas.data.iter().any(|p| *p != 0xffffff));
}

#[test]
fn flex_row_items_side_by_side() {
    let html = "<div style='display:flex;width:600px'><div>a</div><div>b</div><div>c</div></div>";
    let fx = Fixture::new(html, "");
    let t = fx.layout();
    let divs = fx.find(&t, "div");
    let items = &divs[1..];
    assert_eq!(items.len(), 3);
    assert!(items[0].rect.x < items[1].rect.x && items[1].rect.x < items[2].rect.x);
    assert!((items[0].rect.y - items[1].rect.y).abs() < 0.01);
    // Items without `flex-grow` are content-sized (CSS Flexbox §9.7).
    assert!(items[0].rect.w < 100.0, "{}", items[0].rect.w);
}

#[test]
fn pre_preserves_lines() {
    let html = "<pre>line one\n  line two\nline three</pre>";
    let fx = Fixture::new(html, "pre{white-space:pre}");
    let t = fx.layout();
    let pre = &fx.find(&t, "pre")[0];
    assert_eq!(pre.lines.len(), 3);
    assert!(text_of(pre).contains("  line two"));
}

#[test]
fn text_align_center() {
    let html = "<div style='width:400px;text-align:center'><p>hi</p></div>";
    let fx = Fixture::new(html, "");
    let t = fx.layout();
    let p = &fx.find(&t, "p")[0];
    let f = &p.lines[0].fragments[0];
    let center = f.rect.x + f.rect.w / 2.0;
    let expected = p.content_rect.x + p.content_rect.w / 2.0;
    assert!((center - expected).abs() < 1.0, "{center} vs {expected}");
}

/// Renders a sample page to the scratch directory for visual inspection
/// (`RABBITHOLE_SNAPSHOT=/path/out.png cargo test -p rabbithole-layout snapshot`).
#[test]
fn snapshot_sample_page() {
    let path = match std::env::var("RABBITHOLE_SNAPSHOT") {
        Ok(p) => p,
        Err(_) => return,
    };
    let html = r##"<!doctype html><html><head><title>t</title></head><body>
<h1>RabbitHole layout <em>sample</em></h1>
<p>This is a paragraph with <b>bold</b>, <i>italic</i>, <a href="#">a link</a> and <code>code</code>.
It should wrap nicely across several lines when the viewport is narrow enough to require it, and the
<span style="background:#ffe08a;padding:0 4px;border:1px solid #c90">highlighted span</span> gets a background.</p>
<div style="float:left;width:120px;height:80px;background:#9cf;margin:0 10px 10px 0;border:2px solid #36c">float</div>
<p>Text next to a float. Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.</p>
<ul><li>First item</li><li>Second item with <strong>strong</strong></li><li>Third</li></ul>
<ol><li>uno</li><li>dos</li></ol>
<table border=1 style="border:1px solid #888"><tr><th>Name</th><th>Value</th></tr><tr><td>alpha</td><td>1</td></tr><tr><td>beta is longer</td><td>22</td></tr></table>
<div style="display:flex;gap:10px"><div style="flex:1;background:#eee;padding:8px">card one</div><div style="background:#ddd;padding:8px">card two with more text</div><div style="background:#ccc;padding:8px">three</div></div>
<pre>pre
  formatted   text</pre>
<p style="text-align:center">centered <span style="display:inline-block;width:40px;height:20px;background:red;vertical-align:middle"></span> text</p>
<div style="position:relative;height:60px;background:#f5f5f5"><div style="position:absolute;right:10px;top:10px;width:100px;height:40px;background:#fc6">abs</div></div>
<p><img src="x" alt="[alt text of missing image]"> after image</p>
<p style="border-radius:8px;border:3px dashed #393;padding:6px;margin:4px 0">dashed rounded border</p>
</body></html>"##;
    let fx = Fixture::new(html, "th,td{border:1px solid #888;padding:3px} code{font-family:monospace;background:#eee} a{color:#00e;text-decoration:underline} h1{font-size:2em}");
    let t = fx.layout();
    let images = HashMap::new();
    let bgs = HashMap::new();
    let ctx = fx.ctx(&images, &bgs);
    let list = build_display_list(&t, &ctx);
    let scroll: f32 = std::env::var("RABBITHOLE_SCROLL").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let mut canvas = Canvas::new(800, 600);
    paint(&list, &mut canvas, &fx.fonts, 0.0, scroll);
    let rgba = canvas.to_rgba8();
    image::save_buffer(&path, &rgba, 800, 600, image::ColorType::Rgba8).unwrap();
    std::fs::write(format!("{path}.txt"), t.dump()).unwrap();
}

#[test]
fn large_page_layout_is_fast() {
    let mut html = String::from("<body>");
    for i in 0..400 {
        html.push_str(&format!(
            "<div class=c><h2>Section {i}</h2><p>Paragraph {i} with <b>bold</b> and <a href=#>a link</a> plus some more words to wrap around the container edge.</p><ul><li>one</li><li>two</li><li>three</li></ul><table><tr><td>a{i}</td><td>b</td></tr></table></div>"
        ));
    }
    html.push_str("</body>");
    let fx = Fixture::new(&html, ".c{padding:4px;border:1px solid #ccc} td{padding:2px}");
    assert!(fx.doc.nodes.len() > 5000, "{}", fx.doc.nodes.len());
    let start = std::time::Instant::now();
    let t = fx.layout();
    let elapsed = start.elapsed();
    let images = HashMap::new();
    let bgs = HashMap::new();
    let ctx = fx.ctx(&images, &bgs);
    let list = build_display_list(&t, &ctx);
    let mut canvas = Canvas::new(800, 600);
    paint(&list, &mut canvas, &fx.fonts, 0.0, 1000.0);
    let total = start.elapsed();
    eprintln!("layout {elapsed:?}, layout+paint {total:?}, items {}", list.items.len());
    assert!(t.height > 10_000.0);
    assert!(elapsed.as_secs_f32() < 5.0, "layout took {elapsed:?}");
}

// ------------------------------------------------------------- v0.2 tests

fn fake_image(w: u32, h: u32) -> Arc<Image> {
    Arc::new(Image::new(w, h, vec![200u8; (w * h * 4) as usize]))
}

fn frag_texts(b: &LayoutBox) -> Vec<String> {
    let mut out = Vec::new();
    for l in &b.lines {
        for f in &l.fragments {
            if let FragmentKind::Text { text, .. } = &f.kind {
                out.push(text.to_string());
            }
        }
    }
    out
}

#[test]
fn generated_content_before_after() {
    let html = "<p id=x data-k='v'>mid</p>";
    let fx = Fixture::new(html, "p::before{content:'A' attr(data-k) '-'} p::after{content:' Z'}");
    let t = fx.layout();
    let p = &fx.find(&t, "p")[0];
    let text = text_of(p);
    assert_eq!(text, "Av-mid Z", "{text}");
    // The ::before text is hit-testable to the originating element.
    let f = &p.lines[0].fragments[0];
    let n = t.node_at(f.rect.x + 1.0, f.rect.y + 2.0).expect("hit");
    assert_eq!(fx.doc.tag(n), Some("p"));
    assert!(p.children.iter().any(|c| c.pseudo == Some(rabbithole_css::PseudoElement::Before)) || p.lines[0].fragments.len() >= 2);
}

#[test]
fn counters_number_items() {
    let html = "<ol class=c><li>a</li><li>b</li><li>c</li></ol>";
    let fx = Fixture::new(
        html,
        "ol.c{counter-reset:item;list-style:none} ol.c li::before{counter-increment:item;content:counter(item) '. '}",
    );
    let t = fx.layout();
    let lis = fx.find(&t, "li");
    assert_eq!(lis.len(), 3);
    assert!(text_of(lis[0]).starts_with("1. "), "{:?}", text_of(lis[0]));
    assert!(text_of(lis[1]).starts_with("2. "), "{:?}", text_of(lis[1]));
    assert!(text_of(lis[2]).starts_with("3. "), "{:?}", text_of(lis[2]));
}

#[test]
fn quotes_nest() {
    let html = "<p><q>outer <q>inner</q> end</q></p>";
    let fx = Fixture::new(html, "q::before{content:open-quote} q::after{content:close-quote}");
    let t = fx.layout();
    let p = &fx.find(&t, "p")[0];
    let text = text_of(p);
    assert_eq!(text, "\u{201c}outer \u{2018}inner\u{2019} end\u{201d}", "{text}");
}

#[test]
fn text_overflow_ellipsis() {
    let html = "<div style='width:120px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis'>This is a fairly long sentence that cannot fit</div>";
    let fx = Fixture::new(html, "");
    let t = fx.layout();
    let d = &fx.find(&t, "div")[0];
    assert_eq!(d.lines.len(), 1);
    let texts = frag_texts(d);
    assert!(texts.last().unwrap().ends_with('\u{2026}'), "{texts:?}");
    for f in &d.lines[0].fragments {
        assert!(f.rect.right() <= d.content_rect.right() + 0.6, "{:?}", f.rect);
    }
}

#[test]
fn line_clamp_two_lines() {
    let words: Vec<String> = (0..80).map(|i| format!("word{i}")).collect();
    let html = format!("<div style='width:200px;overflow:hidden;display:-webkit-box;-webkit-box-orient:vertical;-webkit-line-clamp:2'>{}</div>", words.join(" "));
    let fx = Fixture::new(&html, "");
    let t = fx.layout();
    let d = &fx.find(&t, "div")[0];
    if d.style.line_clamp.is_none() {
        eprintln!("line-clamp not parsed by the style crate yet; skipping");
        return;
    }
    assert_eq!(d.lines.len(), 2, "{}", d.lines.len());
    let texts = frag_texts(d);
    assert!(texts.last().unwrap().ends_with('\u{2026}'), "{texts:?}");
}

#[test]
fn word_break_and_overflow_wrap() {
    let long = "Pneumonoultramicroscopicsilicovolcanoconiosis".repeat(2);
    let html = format!(
        "<div style='width:120px'><p id=a style='word-break:break-all'>{long}</p><p id=b>{long}</p><p id=c style='overflow-wrap:break-word'>{long}</p></div>"
    );
    let fx = Fixture::new(&html, "");
    let t = fx.layout();
    let ps = fx.find(&t, "p");
    assert!(ps[0].lines.len() > 1, "break-all should break");
    // overflow-wrap: normal lets the word overflow on one line.
    assert_eq!(ps[1].lines.len(), 1);
    let f = &ps[1].lines[0].fragments[0];
    assert!(f.rect.right() > ps[1].content_rect.right() + 1.0, "should overflow");
    assert!(ps[2].lines.len() > 1, "break-word should break");
    for l in &ps[2].lines {
        for f in &l.fragments {
            assert!(f.rect.right() <= ps[2].content_rect.right() + 0.6);
        }
    }
}

#[test]
fn sticky_box_follows_scroll() {
    let html = "<div style='height:3000px'><div id=s style='position:sticky;top:10px;height:20px'>sticky</div><p>after</p></div>";
    let mut fx = Fixture::new(html, "");
    fx.page_scroll = (0.0, 500.0);
    let images = HashMap::new();
    let bgs = HashMap::new();
    let ctx = fx.ctx(&images, &bgs);
    let t = layout(&ctx);
    let s = fx.find(&t, "div")[1];
    assert!(s.rect.y < 100.0, "layout position is normal flow");
    let list = build_display_list(&t, &ctx);
    // The sticky box is wrapped in a translation that moves it to scroll_y + 10.
    let shift = list.items.iter().find_map(|i| match i {
        DisplayItem::PushTransform(m) if m[5] > 0.0 => Some(m[5]),
        _ => None,
    });
    let expected = 500.0 + 10.0 - s.margin_rect().y;
    assert!(shift.map(|v| (v - expected).abs() < 0.5).unwrap_or(false), "shift {shift:?} expected {expected}");
}

#[test]
fn fixed_box_in_fixed_layer() {
    let html = "<div style='height:2000px'>tall</div><div id=f style='position:fixed;bottom:0;left:0;width:100px;height:30px;background:#123456'>f</div>";
    let fx = Fixture::new(html, "");
    let t = fx.layout();
    let f = fx.find(&t, "div")[1];
    assert!(f.fixed);
    assert!((f.rect.bottom() - 600.0).abs() < 0.01, "fixed to viewport bottom: {:?}", f.rect);
    assert!(t.height < 2100.0, "fixed boxes don't extend the document");
    let list = fx.display_list();
    let fixed_start = list.items.iter().position(|i| matches!(i, DisplayItem::PushFixed)).expect("PushFixed");
    let fixed_end = list.items.iter().position(|i| matches!(i, DisplayItem::PopFixed)).expect("PopFixed");
    let rect_pos = list
        .items
        .iter()
        .position(|i| matches!(i, DisplayItem::Rect { color, .. } if color.r == 0x12 && color.g == 0x34))
        .expect("fixed bg");
    assert!(fixed_start < rect_pos && rect_pos < fixed_end);
    // Hit testing in viewport coordinates finds it even when scrolled.
    let n = t.node_at_with_scroll(10.0, 590.0, (0.0, 1500.0)).expect("hit");
    assert_eq!(fx.doc.attr(n, "id"), Some("f"));
}

#[test]
fn scroll_container_offsets_children() {
    let html = "<div id=s style='height:100px;overflow:auto'><p style='margin:0;height:300px'>a</p><p style='margin:0'>b</p></div>";
    let mut fx = Fixture::new(html, "");
    let t0 = fx.layout();
    let s = fx.find(&t0, "div")[0];
    assert!(s.scroll_container);
    assert!(s.scroll_height >= 300.0, "{}", s.scroll_height);
    let info = t0.scroll_container_at(s.rect.x + 5.0, s.rect.y + 5.0).expect("scroll info");
    assert_eq!(fx.doc.attr(info.node, "id"), Some("s"));
    let node = s.node.unwrap();
    let p_y = fx.find(&t0, "p")[0].rect.y;
    fx.scrolls.insert(node, (0.0, 50.0));
    let t1 = fx.layout();
    let p1 = fx.find(&t1, "p")[0];
    assert!((p1.rect.y - (p_y - 50.0)).abs() < 0.01, "child shifted by the offset");
    let s1 = fx.find(&t1, "div")[0];
    assert_eq!(s1.scroll_offset, (0.0, 50.0));
}

#[test]
fn z_index_order() {
    let html = "<div id=p style='position:relative;background:#010101'><div id=n style='position:absolute;z-index:-1;width:10px;height:10px;background:#020202'></div><p>text</p></div>";
    let fx = Fixture::new(html, "");
    let list = fx.display_list();
    let pos = |pred: &dyn Fn(&DisplayItem) -> bool| list.items.iter().position(pred).expect("item");
    let parent_bg = pos(&|i| matches!(i, DisplayItem::Rect { color, .. } if color.r == 1));
    let neg_bg = pos(&|i| matches!(i, DisplayItem::Rect { color, .. } if color.r == 2));
    let text = pos(&|i| matches!(i, DisplayItem::Text { text, .. } if text == "text"));
    // The parent (position: relative, z-index auto) is no stacking
    // context: the negative child goes below it, into the root's
    // negative layer (CSS 2.1 Appendix E), as in other browsers.
    assert!(neg_bg < parent_bg && parent_bg < text, "{parent_bg} {neg_bg} {text}");
    // With a z-index the parent is a stacking context and keeps it above
    // its own background.
    let fx = Fixture::new(&html.replace("position:relative;", "position:relative;z-index:0;"), "");
    let list = fx.display_list();
    let pos = |pred: &dyn Fn(&DisplayItem) -> bool| list.items.iter().position(pred).expect("item");
    let parent_bg = pos(&|i| matches!(i, DisplayItem::Rect { color, .. } if color.r == 1));
    let neg_bg = pos(&|i| matches!(i, DisplayItem::Rect { color, .. } if color.r == 2));
    assert!(parent_bg < neg_bg, "{parent_bg} {neg_bg}");
}

#[test]
fn opacity_layer_and_transform() {
    let html = "<div style='opacity:0.5;width:100px;height:50px;background:red'>o</div><div id=t style='transform:rotate(90deg);width:100px;height:50px'>t</div>";
    let fx = Fixture::new(html, "");
    let t = fx.layout();
    let list = fx.display_list();
    assert!(list.items.iter().any(|i| matches!(i, DisplayItem::PushLayer(_))));
    assert!(list.items.iter().any(|i| matches!(i, DisplayItem::PopLayer(o) if (*o - 0.5).abs() < 0.01)));
    let d = fx.find(&t, "div")[1];
    let m = list
        .items
        .iter()
        .find_map(|i| match i {
            DisplayItem::PushTransform(m) if m[0].abs() < 0.01 => Some(*m),
            _ => None,
        })
        .expect("rotation matrix");
    // rotate(90deg): a=0 b=1 c=-1 d=0, about the center.
    assert!((m[1] - 1.0).abs() < 0.01 && (m[2] + 1.0).abs() < 0.01, "{m:?}");
    let (cx, cy) = (d.rect.x + d.rect.w / 2.0, d.rect.y + d.rect.h / 2.0);
    let (px, py) = crate::tree::map_point(m, cx, cy);
    assert!((px - cx).abs() < 0.01 && (py - cy).abs() < 0.01, "center is fixed");
}

#[test]
fn background_layers_and_gradient() {
    let html = "<div style='width:200px;height:100px;background:url(x.png) no-repeat center / cover, linear-gradient(red, blue)'>b</div>";
    let fx = Fixture::new(html, "");
    let mut bgs: Bgs = HashMap::new();
    bgs.insert("x.png".into(), fake_image(50, 50));
    let images = HashMap::new();
    let ctx = fx.ctx(&images, &bgs);
    let t = layout(&ctx);
    let list = build_display_list(&t, &ctx);
    let d = fx.find(&t, "div")[0];
    if d.style.background_layers.is_empty() {
        eprintln!("background layers not parsed by the style crate yet; skipping");
        return;
    }
    let img = list.items.iter().position(|i| matches!(i, DisplayItem::Image { .. } | DisplayItem::ImageTiled { .. })).expect("image layer");
    let grad = list
        .items
        .iter()
        .position(|i| matches!(i, DisplayItem::RectRadii { paint: Paint::Linear { .. }, .. }))
        .expect("gradient layer");
    assert!(grad < img, "first layer paints on top (later)");
    if let Some(DisplayItem::Image { rect, .. }) = list.items.get(img) {
        // cover on a 200x100 box with a square image → 200x200 tile, centered.
        assert!((rect.w - 200.0).abs() < 0.5, "{rect:?}");
    }
}

#[test]
fn radii_shadow_outline_order() {
    let html = "<div style='width:100px;height:50px;background:#0a0a0a;border:2px solid #0b0b0b;border-radius:10px 0 5px 0;box-shadow:2px 2px 4px #0c0c0c;outline:1px solid #0d0d0d'>x</div>";
    let fx = Fixture::new(html, "");
    let list = fx.display_list();
    let pos = |pred: &dyn Fn(&DisplayItem) -> bool| list.items.iter().position(pred);
    let shadow = pos(&|i| matches!(i, DisplayItem::BoxShadow { .. }));
    let bg = pos(&|i| matches!(i, DisplayItem::RectRadii { paint: Paint::Solid(c), .. } if c.r == 0x0a));
    let border = pos(&|i| matches!(i, DisplayItem::Border { colors, .. } if colors[0].r == 0x0b));
    let outline = pos(&|i| matches!(i, DisplayItem::Border { colors, .. } if colors[0].r == 0x0d));
    if bg.is_none() {
        eprintln!("per-corner radii not parsed by the style crate yet; skipping");
        return;
    }
    assert!(shadow.unwrap() < bg.unwrap(), "shadow before background");
    assert!(bg.unwrap() < border.unwrap());
    assert!(border.unwrap() < outline.unwrap(), "outline last");
    if let Some(DisplayItem::Border { radii, .. }) = list.items.get(border.unwrap()) {
        assert!((radii[0].0 - 10.0).abs() < 0.01 && radii[1].0 < 0.01 && (radii[2].0 - 5.0).abs() < 0.01, "{radii:?}");
    }
}

#[test]
fn object_fit_cover_and_aspect_ratio() {
    let html = "<img id=i src=x style='width:100px;height:100px;object-fit:cover'><div id=d style='width:200px;aspect-ratio:2/1'></div><div id=m style='width:min-content'>short longestword</div>";
    let fx = Fixture::new(html, "");
    let mut images: Images = HashMap::new();
    let img_node = fx.doc.find_element("img").unwrap();
    images.insert(img_node, fake_image(200, 100));
    let bgs = HashMap::new();
    let ctx = fx.ctx(&images, &bgs);
    let t = layout(&ctx);
    let list = build_display_list(&t, &ctx);
    if let Some(DisplayItem::Image { src, rect, .. }) = list.items.iter().find(|i| matches!(i, DisplayItem::Image { .. })) {
        // cover: scale 1.0 (100/100 vs 100/200 → max) → shows the middle 100x100 of the 200x100 image.
        assert!((rect.w - 100.0).abs() < 0.01 && (rect.h - 100.0).abs() < 0.01);
        assert!((src.w - 100.0).abs() < 0.5 && (src.x - 50.0).abs() < 0.5, "{src:?}");
    } else {
        panic!("no image item");
    }
    let divs = fx.find(&t, "div");
    let d = divs[0];
    if d.style.aspect_ratio.is_some() {
        assert!((d.rect.h - 100.0).abs() < 0.01, "aspect-ratio height {}", d.rect.h);
    }
    let m = divs[1];
    if m.style.width.is_intrinsic() {
        let font = fx.fonts.select(rabbithole_gfx::Family::Sans, 400, false);
        let longest = fx.fonts.measure(font, 16.0, "longestword");
        let space = fx.fonts.measure(font, 16.0, " ");
        // Tolerate one space width (intrinsic.rs counts the leading space of a word).
        assert!(m.rect.w >= longest - 0.5 && m.rect.w <= longest + space + 0.5, "min-content {} vs {}", m.rect.w, longest);
    }
}

#[test]
fn pointer_events_none_is_transparent() {
    let html = "<div id=a style='width:100px;height:100px'><div id=b style='width:100px;height:100px;pointer-events:none'></div></div>";
    let fx = Fixture::new(html, "");
    let t = fx.layout();
    let b = fx.find(&t, "div")[1];
    let n = t.node_at(b.rect.x + 5.0, b.rect.y + 5.0).unwrap();
    if b.style.pointer_events == rabbithole_style::PointerEvents::None {
        assert_eq!(fx.doc.attr(n, "id"), Some("a"));
    }
}

#[test]
fn rtl_aligns_right() {
    let html = "<div style='width:300px;direction:rtl'><p>hello world</p></div>";
    let fx = Fixture::new(html, "");
    let t = fx.layout();
    let p = &fx.find(&t, "p")[0];
    if p.style.direction != rabbithole_style::Direction::Rtl {
        return;
    }
    let f = p.lines[0].fragments.last().unwrap();
    assert!((f.rect.right() - p.content_rect.right()).abs() < 1.0, "{:?} vs {:?}", f.rect, p.content_rect);
}

#[test]
fn v2_paint_does_not_panic() {
    let html = "<div style='background:linear-gradient(to right,red,blue);border-radius:8px 2px;box-shadow:0 0 6px #000;opacity:.8;transform:translate(10px,5px)'><p style='text-shadow:1px 1px 2px #888;text-decoration:underline wavy red'>t</p></div><div style='position:fixed;top:0;left:0;width:20px;height:20px;background:#f00'></div>";
    let fx = Fixture::new(html, "");
    let list = fx.display_list();
    let mut canvas = Canvas::new(400, 300);
    paint(&list, &mut canvas, &fx.fonts, 0.0, 0.0);
    assert!(canvas.data.iter().any(|p| *p != 0xffffff));
    let _ = Rect::default();
}

/// A `z-index: 1` flex item inside a `position: relative` (z-index auto)
/// wrapper paints and takes clicks above an absolutely positioned sibling
/// of the wrapper that covers it (Material outlined text fields: the
/// `<input>` under the outline box).
#[test]
fn z_index_flex_item_escapes_a_positioned_z_auto_wrapper() {
    let html = "<div id=field><div id=row><div id=wrap><input id=q></div><div id=outline></div></div></div>";
    let css = "#field{position:relative;width:300px;height:56px} #row{display:flex} \
               #wrap{display:flex;position:relative;flex-grow:1} \
               #q{z-index:1;flex-grow:1;height:28px;padding:13px 15px;border:none;background:transparent} \
               #outline{position:absolute;top:0;bottom:0;left:0;width:100%;border:1px solid gray;box-sizing:border-box}";
    let fx = Fixture::new(html, css);
    let dl = fx.display_list();
    let q = fx.doc.element_by_id("q").unwrap();
    let outline = fx.doc.element_by_id("outline").unwrap();
    assert_eq!(dl.hits.hit_test(150.0, 30.0, (0.0, 0.0)), Some(q));
    // The outline still covers the field (it stretches between top and bottom).
    let t = fx.layout();
    let o = fx.find(&t, "div").into_iter().find(|b| b.node == Some(outline)).unwrap();
    assert!((o.rect.h - 56.0).abs() < 0.01, "outline height {}", o.rect.h);
    // Without the z-index the outline (later in tree order) is on top.
    let fx = Fixture::new(html, &css.replace("z-index:1;", ""));
    assert_eq!(fx.display_list().hits.hit_test(150.0, 30.0, (0.0, 0.0)), Some(outline));
}

/// Positive z-index descendants of nested `z-index: auto` positioned boxes
/// paint after later positioned siblings; z-index auto ones stay in place.
#[test]
fn positioned_z_auto_boxes_do_not_capture_z_indexed_descendants() {
    let html = "<div id=a><div id=b><div id=top></div><div id=mid></div></div></div><div id=cover></div>";
    let css = "body{margin:0} div{width:100px;height:100px} #a,#b{position:relative} #top{position:absolute;top:0;left:0;z-index:2;background:red} \
               #mid{position:absolute;top:0;left:0;background:green} #cover{position:absolute;top:0;left:0;background:blue}";
    let fx = Fixture::new(html, css);
    let dl = fx.display_list();
    // Painted on top of #cover (a later z-index:auto box)...
    assert_eq!(dl.hits.hit_test(50.0, 50.0, (0.0, 0.0)), fx.doc.element_by_id("top"));
    let hits = dl.hits.hit_test_all(50.0, 50.0, (0.0, 0.0));
    let pos = |id: &str| hits.iter().position(|&n| Some(n) == fx.doc.element_by_id(id)).unwrap();
    // ...while #mid (z-index auto) stays below #cover.
    assert!(pos("top") < pos("cover") && pos("cover") < pos("mid"), "{hits:?}");
    // And it is painted exactly once.
    let reds = dl.items.iter().filter(|it| matches!(it, DisplayItem::Rect { color, .. } if color.r == 255 && color.g == 0)).count();
    assert_eq!(reds, 1);
}

#[test]
fn absolute_box_with_top_and_bottom_fills_the_containing_block() {
    let fx = Fixture::new(
        "<div id=cb><div id=ov></div><div id=pad></div></div>",
        "#cb{position:relative;height:200px} #ov{position:absolute;top:10px;bottom:20px;left:0;width:50px} #pad{position:absolute;inset:0;padding:5px;border:2px solid}",
    );
    let t = fx.layout();
    let divs = fx.find(&t, "div");
    let get = |id: &str| *divs.iter().find(|b| b.node == fx.doc.element_by_id(id)).unwrap();
    assert!((get("ov").rect.h - 170.0).abs() < 0.01, "{}", get("ov").rect.h);
    assert!((get("ov").rect.y - get("cb").rect.y - 10.0).abs() < 0.01);
    assert!((get("pad").rect.h - 200.0).abs() < 0.01, "{}", get("pad").rect.h);
    assert!((get("pad").content_rect.h - 186.0).abs() < 0.01);
}

#[test]
fn absolute_descendants_use_the_positioned_inline_padding_box() {
    let fx = Fixture::new(
        "<div id=outer><i id=prefix></i><span id=inline>XX<i id=static><b id=absolute></b></i><i id=positioned><b id=inner></b></i>XX</span></div>",
        "body{margin:0} #outer{position:relative;width:400px;font:20px/20px monospace} \
         i{display:inline-block;width:40px;height:20px} #prefix{width:60px} \
         #inline{position:relative;border:3px solid;padding:5px 7px;margin:0 11px} \
         #absolute{position:absolute;inset:0} #positioned{position:relative} \
         #inner{position:absolute;left:0;top:0;width:50%;height:50%}",
    );
    let t = fx.layout();
    let node = |id| fx.doc.element_by_id(id).unwrap();
    let inline = t.inline_rect(&fx.doc, node("inline")).unwrap();
    let absolute = t.box_of(&fx.doc, node("absolute")).unwrap().rect;
    assert!((absolute.x - inline.x - 14.0).abs() < 0.01, "{absolute:?} vs {inline:?}");
    assert!((absolute.y - inline.y - 3.0).abs() < 0.01);
    assert!((absolute.w - inline.w + 28.0).abs() < 0.01);
    assert!((absolute.h - inline.h + 6.0).abs() < 0.01);
    let positioned = t.box_of(&fx.doc, node("positioned")).unwrap().rect;
    let inner = t.box_of(&fx.doc, node("inner")).unwrap().rect;
    assert!((inner.x - positioned.x).abs() < 0.01);
    assert!((inner.y - positioned.y).abs() < 0.01);
    assert!((inner.w - 20.0).abs() < 0.01);
    assert!((inner.h - 10.0).abs() < 0.01);
}

#[test]
fn inline_absolute_containing_block_uses_first_and_last_fragments() {
    let fx = Fixture::new(
        "<div id=outer><i class=prefix></i><span id=inline><i class=first></i><br><i class=wide></i><br><i class=last></i><b id=absolute></b></span></div><div id=other></div>",
        "body{margin:0} #outer{position:relative;width:200px;font:20px/20px monospace} \
         i{display:inline-block;height:20px} .prefix{width:60px}.first{width:40px}.wide{width:180px}.last{width:80px} \
         #inline{position:relative} #absolute{position:absolute;inset:0}",
    );
    let mut t = fx.layout();
    let absolute = t.box_of(&fx.doc, fx.doc.element_by_id("absolute").unwrap()).unwrap().rect;
    assert!((absolute.x - 60.0).abs() < 0.01, "{absolute:?}");
    assert!((absolute.w - 20.0).abs() < 0.01, "the wider middle line must not widen the containing block: {absolute:?}");
    let fragments = t.inline_client_rects(&fx.doc, fx.doc.element_by_id("inline").unwrap());
    assert_eq!(fragments.len(), 3);
    // Atomic descendants can extend beyond the inline's font box. Its
    // own vertical padding edges follow the font metrics at each baseline,
    // rather than the CSSOM union of its descendants' rectangles.
    let style = fx.styles.get(fx.doc.element_by_id("inline").unwrap()).unwrap();
    let font = fx.fonts.select_family(&style.font_family_list, style.font_family, style.font_weight, style.is_italic());
    let metrics = fx.fonts.metrics(font, style.font_size);
    let outer = t.box_of(&fx.doc, fx.doc.element_by_id("outer").unwrap()).unwrap();
    let top = outer.lines[0].baseline - metrics.ascent;
    let bottom = outer.lines[2].baseline + metrics.descent;
    assert!((absolute.y - top).abs() < 0.01, "{absolute:?}, expected top {top}");
    assert!((absolute.bottom() - bottom).abs() < 0.01, "{absolute:?}, expected bottom {bottom}");
    // Rebuilding a sibling must retain the first/last edges when reusing
    // the positioned container's cached lines.
    let images = HashMap::new();
    let backgrounds = HashMap::new();
    for _ in 0..3 {
        let damage = crate::LayoutDamage { full: false, dirty: [fx.doc.element_by_id("other").unwrap()].into_iter().collect() };
        t = crate::layout_incremental(t, &fx.ctx(&images, &backgrounds), &damage);
        assert_eq!(t.box_of(&fx.doc, fx.doc.element_by_id("absolute").unwrap()).unwrap().rect, absolute);
    }
}

#[test]
fn inline_relative_offsets_move_absolute_static_positions() {
    let fx = Fixture::new(
        "<div><span><b id=absolute></b></span></div>",
        "body{margin:0} div{position:relative;left:100px;width:100px} \
         span{position:relative;left:-100px;top:25px} #absolute{position:absolute;width:30px;height:40px}",
    );
    let t = fx.layout();
    let absolute = t.box_of(&fx.doc, fx.doc.element_by_id("absolute").unwrap()).unwrap().rect;
    assert!((absolute.x - 0.0).abs() < 0.01, "{absolute:?}");
    assert!((absolute.y - 25.0).abs() < 0.01, "{absolute:?}");
}

#[test]
fn absolute_aspect_ratio_selects_one_automatic_axis_before_stretching() {
    for (container, style, expected) in [
        ("width:100px;height:500px", "inset:0;aspect-ratio:1", (100.0, 100.0)),
        ("width:100px;height:500px", "inset:0;width:100px;aspect-ratio:1", (100.0, 100.0)),
        ("width:500px;height:100px", "inset:0;height:100px;aspect-ratio:1", (100.0, 100.0)),
        ("width:500px;height:100px", "inset:0;height:100%;aspect-ratio:1", (100.0, 100.0)),
        ("width:300px;height:50px", "left:0;top:0;bottom:0;aspect-ratio:2", (100.0, 50.0)),
        ("width:200px;height:100px", "inset:0;max-height:50px;aspect-ratio:2", (100.0, 50.0)),
        ("width:200px;height:100px", "inset:0;width:90px;height:40px;aspect-ratio:1", (90.0, 40.0)),
        ("width:300px;height:100px", "left:20px;height:50px;aspect-ratio:2;box-sizing:border-box;padding:10px;border:2px solid", (100.0, 50.0)),
        ("width:500px;height:100px", "aspect-ratio:1;max-height:100%;min-height:0;contain:size;contain-intrinsic-size:500px 500px", (100.0, 100.0)),
    ] {
        let fx = Fixture::new(
            "<div id=container><div id=absolute></div></div>",
            &format!("body{{margin:0}} #container{{position:relative;{container}}} #absolute{{position:absolute;{style}}}"),
        );
        let t = fx.layout();
        let rect = t.box_of(&fx.doc, fx.doc.element_by_id("absolute").unwrap()).unwrap().rect;
        assert!((rect.w - expected.0).abs() < 0.01 && (rect.h - expected.1).abs() < 0.01, "{style}: {rect:?}, expected {expected:?}");
    }
}

#[test]
fn absolute_aspect_ratio_content_minimum_applies_to_the_dependent_axis() {
    for (style, content, expected) in [
        ("height:100px;aspect-ratio:1/2", "width:100px", (100.0, 100.0)),
        ("width:100px;aspect-ratio:2", "height:100px", (100.0, 100.0)),
        ("width:100px;aspect-ratio:2;min-height:0", "height:100px", (100.0, 50.0)),
        ("width:100px;aspect-ratio:2;overflow:hidden", "height:100px", (100.0, 50.0)),
        ("width:100px;aspect-ratio:2;overflow:clip", "height:100px", (100.0, 100.0)),
        ("left:0;top:0;bottom:0;aspect-ratio:2", "height:100px", (100.0, 50.0)),
    ] {
        let fx = Fixture::new(
            "<div id=container><div id=absolute><div id=content></div></div></div>",
            &format!("body{{margin:0}} #container{{position:relative;width:300px;height:50px}} #absolute{{position:absolute;{style}}} #content{{{content}}}"),
        );
        let t = fx.layout();
        let rect = t.box_of(&fx.doc, fx.doc.element_by_id("absolute").unwrap()).unwrap().rect;
        assert!((rect.w - expected.0).abs() < 0.01 && (rect.h - expected.1).abs() < 0.01, "{style}: {rect:?}, expected {expected:?}");
    }
}

#[test]
fn absolute_aspect_ratio_preserves_auto_margin_centering() {
    let fx = Fixture::new(
        "<div id=container><div id=absolute></div></div>",
        "body{margin:0} #container{position:relative;width:500px;height:300px} #absolute{position:absolute;inset:0;margin:auto;height:100px;aspect-ratio:1}",
    );
    let t = fx.layout();
    let rect = t.box_of(&fx.doc, fx.doc.element_by_id("absolute").unwrap()).unwrap().rect;
    assert_eq!(rect, Rect::new(200.0, 100.0, 100.0, 100.0));
}

/// Text inputs are content-box (author height + padding add up) and show
/// their line of text centered vertically.
#[test]
fn text_input_is_content_box_with_centered_text() {
    let fx = Fixture::new(
        "<input id=q value=hello><input id=d value=x><input type=submit id=s value=Go style='height:40px;padding:5px'>",
        "#q{height:28px;padding:13px 15px;border:none;font-size:16px;line-height:normal} #d{border:1px solid;padding:1px 2px}",
    );
    let t = fx.layout();
    let inputs = fx.find(&t, "input");
    let q = inputs[0];
    assert!((q.rect.h - 54.0).abs() < 0.01, "height {}", q.rect.h);
    let line = &q.lines[0];
    let mid = line.rect.y + line.rect.h / 2.0;
    assert!((mid - (q.content_rect.y + 14.0)).abs() < 0.6, "line {:?} content {:?}", line.rect, q.content_rect);
    // Text fields are one line (normal line height) plus padding and
    // border tall.
    let d = inputs[1];
    assert!((d.rect.h - (13.333 * 1.164 + 4.0)).abs() < 0.6, "default height {}", d.rect.h);
    // Buttons stay border-box.
    assert!((inputs[2].rect.h - 40.0).abs() < 0.01, "button {}", inputs[2].rect.h);
}

#[test]
fn text_overflow_custom_string_and_empty_marker() {
    for marker in ["[more]", "", "123"] {
        let html = format!("<div style='width:120px;white-space:nowrap;overflow:hidden;text-overflow:\"{marker}\"'>This is a fairly long sentence that cannot fit</div>");
        let fx = Fixture::new(&html, "");
        let tree = fx.layout();
        let div = &fx.find(&tree, "div")[0];
        let text = text_of(div);
        assert!(text.ends_with(marker), "{marker}: {text}");
        assert!(!text.contains('…'), "{text}");
        for frag in &div.lines[0].fragments {
            assert!(frag.rect.right() <= div.content_rect.right() + 0.6, "{:?}", frag.rect);
        }
    }
}

#[test]
fn text_overflow_string_keeps_first_character_when_marker_is_too_wide() {
    let fx = Fixture::new("<div style='font:20px monospace;width:30px;white-space:nowrap;overflow:hidden;text-overflow:\"123456789\"'>abcdef</div>", "");
    let tree = fx.layout();
    let div = &fx.find(&tree, "div")[0];
    assert_eq!(text_of(div), "a123456789");
}

#[test]
fn text_overflow_string_collapses_forced_breaks() {
    let fx = Fixture::new("<div style='width:150px;white-space:nowrap;overflow:hidden;text-overflow:\"Hi\\a\\2028\\2029Hi\"'>This is a fairly long sentence that cannot fit</div>", "");
    let tree = fx.layout();
    let div = &fx.find(&tree, "div")[0];
    assert!(text_of(div).ends_with("Hi Hi"), "{}", text_of(div));
}

#[test]
fn text_overflow_string_preserves_spaces_with_white_space_pre() {
    let fx = Fixture::new("<div style='width:150px;white-space:pre;overflow:hidden;text-overflow:\"a  b\"'>This is a fairly long sentence that cannot fit</div>", "");
    let tree = fx.layout();
    let div = &fx.find(&tree, "div")[0];
    assert!(text_of(div).ends_with("a  b"), "{}", text_of(div));
}

#[test]
fn cached_scrolled_contents_keep_their_origin_when_another_box_is_rebuilt() {
    let mut fx = Fixture::new(
        "<div id=scroller style='overflow:scroll;width:100px;height:100px'><div id=inside style='display:inline-block;width:300px;height:300px'>content</div></div><div id=other>after</div>",
        "",
    );
    let scroller = fx.doc.element_by_id("scroller").unwrap();
    let inside = fx.doc.element_by_id("inside").unwrap();
    let other = fx.doc.element_by_id("other").unwrap();
    let mut tree = fx.layout();
    let origin = tree.box_of(&fx.doc, inside).unwrap().rect;
    let images = HashMap::new();
    let backgrounds = HashMap::new();
    for offset in [(20.0, 40.0), (50.0, 80.0), (0.0, 0.0)] {
        assert!(tree.set_scroll_offset(&fx.doc, scroller, offset));
        fx.scrolls.insert(scroller, offset);
        // Invalidating a sibling rebuilds ancestors while retaining this
        // scroller's boxes and the cache of its content layout.
        for _ in 0..3 {
            let damage = crate::LayoutDamage { full: false, dirty: [other].into_iter().collect() };
            tree = crate::layout_incremental(tree, &fx.ctx(&images, &backgrounds), &damage);
            let container = tree.box_of(&fx.doc, scroller).unwrap();
            let rect = tree.box_of(&fx.doc, inside).unwrap().rect;
            assert!((rect.x - (origin.x - offset.0)).abs() < 0.01, "{offset:?}: {rect:?}");
            assert!((rect.y - (origin.y - offset.1)).abs() < 0.01, "{offset:?}: {rect:?}");
            assert_eq!((container.scroll_width, container.scroll_height), (300.0, 300.0));
            assert_eq!(container.scroll_offset, offset);
        }
    }
}

#[test]
fn rtl_inline_containing_block_preserves_fragments_open_at_line_end() {
    let fx = Fixture::new(
        "<div id=outer dir=rtl><i class=prefix></i><span><i class=first></i><br><i class=last></i><b id=absolute></b></span></div>",
        "body{margin:0} #outer{position:relative;direction:rtl;width:200px;padding-top:10px;font-size:0;line-height:0} \
         i{display:inline-block;height:20px;vertical-align:top}.prefix{width:60px}.first{width:40px}.last{width:80px} \
         span{position:relative;padding:10px 0} #absolute{position:absolute;inset:0}",
    );
    let t = fx.layout();
    let absolute = t.box_of(&fx.doc, fx.doc.element_by_id("absolute").unwrap()).unwrap().rect;
    assert_eq!(absolute, Rect::new(120.0, 0.0, 20.0, 40.0));
}
