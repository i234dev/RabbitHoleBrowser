//! Geometry controls for named lines in auto-fit grids and their subgrids.
use std::collections::HashMap;
use rabbithole_css::{parse_stylesheet, MediaEnv};
use rabbithole_dom::Document;
use rabbithole_gfx::FontContext;
use rabbithole_layout::{layout, BoxKind, LayoutBox, LayoutContext, LayoutTree};
use rabbithole_style::style_tree;

fn render(html: &str, css: &str) -> (Document, LayoutTree) {
    let doc = rabbithole_html::parse(html);
    let sheets = [parse_stylesheet(&format!("*{{margin:0;padding:0;font-size:0;line-height:0}}{css}"))];
    let styles = style_tree(&doc, &sheets, &MediaEnv { width:800., height:600., screen:true, scripting:true });
    let fonts = FontContext::new();
    let images = HashMap::new(); let backgrounds = HashMap::new(); let scrolls = HashMap::new();
    let tree = layout(&LayoutContext { doc:&doc, styles:&styles, fonts:&fonts, images:&images, background_images:&backgrounds, scroll_offsets:&scrolls, viewport_width:800., viewport_height:600., page_scroll:(0.,0.) });
    (doc, tree)
}

fn by_id<'a>(doc: &Document, tree: &'a LayoutTree, id: &str) -> &'a LayoutBox {
    fn find(b: &LayoutBox, node: usize) -> Option<&LayoutBox> {
        if b.node == Some(node) && b.kind != BoxKind::Text { return Some(b); }
        b.children.iter().find_map(|child| find(child, node))
    }
    find(&tree.root, doc.element_by_id(id).unwrap()).unwrap()
}

fn close(actual: f32, expected: f32) { assert!((actual-expected).abs()<0.02, "{actual} != {expected}"); }

#[test]
fn auto_fit_occupancy_uses_named_occurrences_for_both_grid_axes_and_lanes() {
    for lanes in [false, true] {
        for rows in [false, true] {
            let axis = if rows { "row" } else { "column" };
            let display = if lanes { "grid-lanes" } else { "grid" };
            let css = format!("#p{{display:{display};width:{}px;height:{}px;grid-lanes-direction:{axis};grid-template-{}s:repeat(auto-fit,[y] 20px) [tail] 20px;grid-template-{}s:20px}}#a{{grid-{axis}:y 2}}#b{{grid-{axis}:y -1}}#tail{{grid-{axis}:tail}}i{{width:20px;height:20px}}", if rows {20} else {100}, if rows {100} else {20}, axis, if rows {"column"} else {"row"});
            let (d,t) = render("<section id=p><i id=a></i><i id=b></i><i id=tail></i></section>", &css);
            let p = by_id(&d,&t,"p");
            for (id,expected) in [("a",0.),("b",20.),("tail",40.)] {
                let item = by_id(&d,&t,id);
                close(if rows { item.rect.y-p.content_rect.y } else { item.rect.x-p.content_rect.x }, expected);
                close(item.content_rect.w,20.); close(item.content_rect.h,20.);
            }
        }
    }
}

#[test]
fn named_occurrences_use_the_subgrid_local_names_after_auto_fit_collapse() {
    for lanes in [false, true] {
        for rows in [false, true] {
            let axis = if rows {"row"} else {"column"};
            let other = if rows {"column"} else {"row"};
            let display = if lanes {"grid-lanes"} else {"grid"};
            let css = format!("#p{{display:{display};width:{}px;height:{}px;grid-lanes-direction:{axis};grid-template-{axis}s:repeat(auto-fit,[y] 20px) [tail] 20px;grid-template-{other}s:10px}}#s{{display:grid;grid-{axis}:2/5;grid-template-{axis}s:subgrid;grid-template-{other}s:10px}}#a{{grid-{axis}:y 2;grid-{other}:1}}#b{{grid-{axis}:y -1;grid-{other}:1}}", if rows {10} else {100}, if rows {100} else {10});
            let (d,t) = render("<section id=p><div id=s><i id=a></i><i id=b></i></div></section>", &css);
            let s = by_id(&d,&t,"s");
            close(if rows {s.content_rect.h} else {s.content_rect.w},60.);
            for (id,expected) in [("a",20.),("b",40.)] {
                let item = by_id(&d,&t,id);
                close(if rows {item.rect.y-s.content_rect.y} else {item.rect.x-s.content_rect.x}, expected);
                close(if rows {item.content_rect.h} else {item.content_rect.w},20.);
            }
        }
    }
}

#[test]
fn named_spans_count_names_instead_of_physical_tracks() {
    let (d,t) = render("<section id=p><i id=forward></i><i id=backward></i></section>",
        "#p{display:grid;width:150px;grid-template-columns:[y] 10px 20px [y] 30px 40px [y] 50px [y];grid-template-rows:10px 10px}#forward{grid-column:y 1/span y 2;grid-row:1}#backward{grid-column:span 2 y/y -1;grid-row:2}");
    let p = by_id(&d,&t,"p"); let a = by_id(&d,&t,"forward"); let b = by_id(&d,&t,"backward");
    close(a.rect.x-p.content_rect.x,0.); close(a.content_rect.w,100.);
    close(b.rect.x-p.content_rect.x,30.); close(b.content_rect.w,120.);
}
