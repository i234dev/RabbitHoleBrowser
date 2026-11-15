//! Independent used-geometry controls; the context/security fixtures remain
//! in framesets.rs. No border dragging or complete legacy rendering claim.
use rabbithole_engine::{Engine,Page,Viewport,net::Url,gfx::Rect};
use rabbithole_engine::style::Display;
#[path="../../../tests/support/nested_frames.rs"] mod fixture;

fn engine()->Engine {let mut engine=Engine::new_bundled_fonts_only();engine.scripting=true;engine.loader.load_scripts=true;engine}
fn settle(engine:&Engine,page:&mut Page) {
    let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);
    let mut quiet=0;
    while std::time::Instant::now()<deadline && quiet<5 {
        let busy=page.frames_loading();engine.tick(page);
        if busy {quiet=0;} else {quiet+=1;}
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!page.frames_loading(),"frames must finish loading");
}
fn load(engine:&mut Engine,html:&str,url:&str)->Page {
    let mut page=engine.load_html(html,Url::parse(url).unwrap(),Viewport{width:800.,height:600.});
    settle(engine,&mut page);page
}
fn rect(page:&Page,id:&str)->Rect {
    let node=page.doc.element_by_id(id).expect("fixture element");
    // Do not climb to an ancestor if the frame failed to generate a box.
    page.layout.box_of(&page.doc,node).expect("frame needs its own box").rect
}
fn near(actual:Rect,expected:(f32,f32,f32,f32)) {
    for (actual,expected) in [actual.x,actual.y,actual.w,actual.h].into_iter()
        .zip([expected.0,expected.1,expected.2,expected.3]) {
        assert!((actual-expected).abs()<0.1,"{actual} != {expected}");
    }
}

#[test]
fn nested_documents_fill_disjoint_rows_and_columns_of_the_viewport() {
    let server=fixture::serve();let mut engine=engine();
    let page=load(&mut engine,fixture::ROOT,&server.url("/"));
    near(rect(&page,"topFrame"),(0.,0.,800.,390.));
    near(rect(&page,"bottomFrame"),(0.,390.,800.,210.));
    let top=page.frames.children[0].page.as_deref().expect("top document");
    assert_eq!(top.viewport,Viewport{width:800.,height:390.});
    near(rect(top,"leftFrame"),(0.,0.,240.,390.));
    near(rect(top,"middleFrame"),(240.,0.,320.,390.));
    near(rect(top,"rightFrame"),(560.,0.,240.,390.));
    let bottom=page.frames.children[1].page.as_deref().expect("bottom document");
    assert_eq!(bottom.viewport,Viewport{width:800.,height:210.});
    for child in &top.frames.children {
        let child=child.page.as_deref().expect("column document");
        assert!(child.viewport.width>=240. && child.viewport.height==390.);
    }
}

#[test]
fn same_document_nested_framesets_allocate_pixels_percent_and_relative_space() {
    let mut engine=engine();
    let page=load(&mut engine,"<!doctype html><frameset id='root' rows='120,*'><frameset id='upper' cols='200,25%,*'><frame id='left'><frame id='middle'><frame id='right'></frameset><frame id='bottom'></frameset>","about:blank");
    near(rect(&page,"root"),(0.,0.,800.,600.));
    near(rect(&page,"upper"),(0.,0.,800.,120.));
    near(rect(&page,"left"),(0.,0.,200.,120.));
    near(rect(&page,"middle"),(200.,0.,200.,120.));
    near(rect(&page,"right"),(400.,0.,400.,120.));
    near(rect(&page,"bottom"),(0.,120.,800.,480.));
    assert_eq!(page.frames.children.len(),4,"framesets group cells but create no browsing context");
}

#[test]
fn reflected_dimensions_and_viewport_resize_rebuild_used_geometry() {
    let mut engine=engine();
    let mut page=load(&mut engine,"<!doctype html><frameset id='root' cols='25%,*'><frame id='left'><frame id='right'></frameset>","about:blank");
    near(rect(&page,"left"),(0.,0.,200.,600.));
    let _=engine.eval_in_page(&mut page,"document.body.cols='50%,50%'");
    settle(&engine,&mut page);
    near(rect(&page,"left"),(0.,0.,400.,600.));
    near(rect(&page,"right"),(400.,0.,400.,600.));
    engine.relayout(&mut page,Viewport{width:400.,height:200.});
    settle(&engine,&mut page);
    near(rect(&page,"left"),(0.,0.,200.,200.));
    near(rect(&page,"right"),(200.,0.,200.,200.));
    assert_eq!(page.frames.children[0].page.as_ref().unwrap().viewport,Viewport{width:200.,height:200.});
}

#[test]
fn bounded_tracks_keep_css_computed_values_and_context_limits_independent() {
    let mut engine=engine();
    let cols=std::iter::repeat_n("*",17).collect::<Vec<_>>().join(",");
    let mut html=format!("<!doctype html><frameset id='root' cols='{cols}'>");
    for index in 0..17 {html.push_str(&format!("<frame id='f{index}'>"));}
    html.push_str("</frameset>");
    let mut page=load(&mut engine,&html,"about:blank");
    for index in 0..16 {near(rect(&page,&format!("f{index}")),(index as f32*50.,0.,50.,600.));}
    let extra=page.doc.element_by_id("f16").unwrap();
    assert!(page.layout.box_of(&page.doc,extra).is_none(),"extra tracks must not grow an implicit grid");
    assert_eq!(page.frames.children.len(),16,"the shared engine context cap remains16");
    let root=page.doc.body().unwrap();let computed=page.styles.get(root).unwrap();
    assert_eq!(computed.display,Display::Block);
    assert!(computed.grid_template_columns.is_empty() && computed.grid_template_rows.is_empty(),"legacy used tracks must not become computed author CSS");
    assert_eq!(engine.eval_in_page(&mut page,"getComputedStyle(document.body).display").trim_matches(|c|c=='\''||c=='"'),"block");
}
