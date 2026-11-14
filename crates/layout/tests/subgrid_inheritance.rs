//! Independent used-geometry controls; these tests have not yet been executed.
use std::collections::HashMap;
use rabbithole_css::{parse_stylesheet, MediaEnv};
use rabbithole_dom::Document;
use rabbithole_gfx::FontContext;
use rabbithole_layout::{layout, BoxKind, LayoutBox, LayoutContext, LayoutTree};
use rabbithole_style::style_tree;

fn render(html:&str,css:&str)->(Document,LayoutTree) {
    let doc=rabbithole_html::parse(html);
    let sheets=[parse_stylesheet(css)];
    let styles=style_tree(&doc,&sheets,&MediaEnv{width:800.,height:600.,screen:true,scripting:true});
    let fonts=FontContext::new();
    let images=HashMap::new();let backgrounds=HashMap::new();let scrolls=HashMap::new();
    let tree=layout(&LayoutContext{doc:&doc,styles:&styles,fonts:&fonts,images:&images,background_images:&backgrounds,scroll_offsets:&scrolls,viewport_width:800.,viewport_height:600.,page_scroll:(0.,0.)});
    (doc,tree)
}
fn by_id<'a>(doc:&Document,tree:&'a LayoutTree,id:&str)->&'a LayoutBox {
    fn find(b:&LayoutBox,node:usize)->Option<&LayoutBox> {
        if b.node==Some(node)&&b.kind!=BoxKind::Text {return Some(b);}
        b.children.iter().find_map(|child|find(child,node))
    }
    find(&tree.root,doc.element_by_id(id).unwrap()).unwrap()
}
fn close(actual:f32,expected:f32) {assert!((actual-expected).abs()<0.02,"{actual} != {expected}");}

#[test]
fn shared_axes_ignore_preferred_min_max_alignment_and_auto_margins() {
    let(d,t)=render("<section id=p><div id=s><i id=a></i><i id=b></i></div></section>",
        "section{display:grid;width:300px;grid-template-columns:60px 140px;grid-template-rows:40px 60px;gap:10px}#s{display:grid;grid-column:1/span 2;grid-row:1/span 2;grid-template-columns:subgrid;grid-template-rows:subgrid;width:999px;height:999px;min-width:999px;max-height:1px;justify-self:center;align-self:end;margin:auto}#a{grid-column:1;grid-row:1}#b{grid-column:2;grid-row:2}");
    let p=by_id(&d,&t,"p");let s=by_id(&d,&t,"s");let a=by_id(&d,&t,"a");let b=by_id(&d,&t,"b");
    close(s.content_rect.w,210.);close(s.content_rect.h,110.);
    close(a.content_rect.w,60.);close(a.content_rect.h,40.);
    close(b.content_rect.w,140.);close(b.content_rect.h,60.);
    close(a.rect.x-p.content_rect.x,0.);close(b.rect.x-p.content_rect.x,70.);close(b.rect.y-p.content_rect.y,50.);
}

#[test]
fn an_independent_axis_keeps_its_own_tracks_and_preferred_size() {
    let(d,t)=render("<section><div id=s><i id=x></i></div></section>",
        "section{display:grid;width:200px;grid-template-columns:60px 140px;grid-template-rows:100px}#s{display:grid;grid-column:span 2;grid-template-columns:subgrid;grid-template-rows:10px 20px;height:30px;align-self:start}#x{grid-column:2;grid-row:2}");
    let s=by_id(&d,&t,"s");let x=by_id(&d,&t,"x");close(s.content_rect.h,30.);close(x.content_rect.w,140.);close(x.content_rect.h,20.);close(x.rect.y-s.content_rect.y,10.);
}

#[test]
fn normal_gap_inherits_and_edge_padding_does_not_move_parent_tracks() {
    let(d,t)=render("<section id=p><div id=s><i id=a></i><i id=b></i></div></section>",
        "section{display:grid;width:300px;grid-template-columns:100px 150px;column-gap:50px}#s{display:grid;grid-column:span 2;grid-template-columns:subgrid;padding:10px;border:5px solid}i{height:20px}#a{grid-column:1}#b{grid-column:2}");
    let p=by_id(&d,&t,"p");let a=by_id(&d,&t,"a");let b=by_id(&d,&t,"b");
    close(a.rect.x-p.content_rect.x,15.);close(a.content_rect.w,85.);close(b.rect.x-p.content_rect.x,150.);close(b.content_rect.w,135.);
}

#[test]
fn nested_subgrid_edge_layers_accumulate() {
    let(d,t)=render("<section id=p><div id=s><div id=n><i id=a></i><i id=b></i></div></div></section>",
        "section{display:grid;width:300px;grid-template-columns:100px 150px;gap:50px}#s,#n{display:grid;grid-column:span 2;grid-template-columns:subgrid}#s{padding:10px}#n{padding:20px}i{height:20px}#a{grid-column:1}#b{grid-column:2}");
    let p=by_id(&d,&t,"p");let a=by_id(&d,&t,"a");let b=by_id(&d,&t,"b");
    close(a.rect.x-p.content_rect.x,30.);close(a.content_rect.w,70.);close(b.rect.x-p.content_rect.x,150.);close(b.content_rect.w,120.);
}

#[test]
fn descendants_size_auto_parent_columns_at_their_actual_shared_lines() {
    let(d,t)=render("<section id=p><div id=s><i id=a></i><i id=b></i></div></section>",
        "section{display:grid;width:max-content;grid-template-columns:auto auto;column-gap:10px}#s{display:grid;grid-column:span 2;grid-template-columns:subgrid;padding:0 5px;width:999px}#a{grid-column:1;width:30px;height:10px}#b{grid-column:2;width:70px;height:10px}");
    let p=by_id(&d,&t,"p");let a=by_id(&d,&t,"a");let b=by_id(&d,&t,"b");
    close(p.content_rect.w,120.);close(a.rect.x-p.content_rect.x,5.);close(b.rect.x-p.content_rect.x,45.);
}

#[test]
fn descendants_size_auto_parent_rows_without_the_subgrids_own_height() {
    let(d,t)=render("<section id=p><div id=s><i id=a></i><i id=b></i></div></section>",
        "section{display:grid;width:80px;grid-template-columns:80px;grid-template-rows:auto auto;row-gap:5px}#s{display:grid;grid-row:span 2;grid-template-rows:subgrid;height:999px;padding:3px 0 7px}#a{grid-row:1;height:20px}#b{grid-row:2;height:40px}");
    let p=by_id(&d,&t,"p");let a=by_id(&d,&t,"a");let b=by_id(&d,&t,"b");
    close(p.content_rect.h,75.);close(a.rect.y-p.content_rect.y,3.);close(b.rect.y-p.content_rect.y,28.);
}

#[test]
fn shared_axes_clamp_out_of_range_placements_without_implicit_tracks() {
    let(d,t)=render("<section id=p><div id=s><i id=a></i><i id=b></i><i id=c></i></div></section>",
        "section{display:grid;width:100px;grid-template-columns:30px 70px;grid-template-rows:20px}#s{display:grid;grid-column:span 2;grid-template-columns:subgrid;grid-template-rows:subgrid}#a{grid-column:99/100}#b{grid-column:-99/-98}#c{grid-column:1/span 100}");
    let p=by_id(&d,&t,"p");let a=by_id(&d,&t,"a");let b=by_id(&d,&t,"b");let c=by_id(&d,&t,"c");
    close(a.rect.x-p.content_rect.x,30.);close(a.content_rect.w,70.);close(b.rect.x-p.content_rect.x,0.);close(b.content_rect.w,30.);close(c.content_rect.w,100.);
}

#[test]
fn orthogonal_shared_rows_follow_parent_columns_in_the_correct_orientation() {
    for (mode,first,last) in [("vertical-lr",100.,150.),("vertical-rl",150.,100.)] {
        let(d,t)=render("<section><div id=s><i id=a></i><i id=b></i></div></section>",
            &format!("section{{display:grid;width:300px;height:80px;grid-template-columns:100px 150px;grid-template-rows:80px;column-gap:50px}}#s{{display:grid;writing-mode:{mode};grid-column:span 2;grid-template-columns:80px;grid-template-rows:subgrid}}#a{{grid-row:1}}#b{{grid-row:2}}"));
        let a=by_id(&d,&t,"a");let b=by_id(&d,&t,"b");close(a.content_rect.w,80.);close(a.content_rect.h,first);close(b.content_rect.h,last);
    }
}

#[test]
fn inherited_row_lanes_measure_automatic_inline_items_without_zero_width() {
    let(d,t)=render("<section id=min><div class=s><i>XXXX XXXX</i></div></section><section id=max><div class=s><i>XXXX XXXX</i></div></section><i id=word>XXXX XXXX</i>",
        "section{display:grid-lanes;grid-template-rows:100px;font:10px/1 monospace}#min{width:min-content}#max{width:max-content}.s{display:grid-lanes;grid-template-rows:subgrid;grid-row:1}#word{display:block;width:min-content;font:10px/1 monospace}");
    let min=by_id(&d,&t,"min");let max=by_id(&d,&t,"max");let word=by_id(&d,&t,"word");close(min.content_rect.w,max.content_rect.w);assert!(min.content_rect.w>word.content_rect.w+1.);
}

#[test]
fn automatic_column_lanes_subgrid_items_contribute_to_every_parent_track() {
    for (placement,expected) in [("span 2",120.),("2/span 2",40.)] {
        let(d,t)=render("<section id=p><div id=s><i></i></div></section>",
            &format!("section{{display:grid-lanes;width:max-content;grid-template-columns:auto auto auto}}#s{{display:grid;grid-column:{placement};grid-template-columns:subgrid}}i{{grid-column:1;width:40px;height:10px}}"));
        close(by_id(&d,&t,"p").content_rect.w,expected);
    }
}

#[test]
fn automatic_row_lanes_subgrid_items_contribute_to_every_parent_track() {
    for (placement,expected) in [("span 2",60.),("2/span 2",20.)] {
        let(d,t)=render("<section id=p><div id=s><i></i></div></section>",
            &format!("section{{display:grid-lanes;width:100px;grid-template-rows:auto auto auto}}#s{{display:grid;grid-row:{placement};grid-template-rows:subgrid;width:10px}}i{{grid-row:1;width:10px;height:20px}}"));
        close(by_id(&d,&t,"p").content_rect.h,expected);
    }
}
