//! Final-layout Grid data, read through the public CSSOM APIs.
use rabbithole_engine::{Engine, Page, Viewport};
use rabbithole_engine::net::Url;

fn engine() -> Engine {
    let mut e = Engine::new_bundled_fonts_only(); e.scripting=true; e.loader.load_scripts=true; e
}
fn load(e:&mut Engine, html:&str, width:f32)->Page {
    e.load_html(&format!("<!doctype html><style>html,body{{margin:0;padding:0;font-size:0;line-height:0}}</style>{html}"), Url::parse("https://example.test/").unwrap(), Viewport{width,height:300.})
}
fn eval(e:&Engine, p:&mut Page, js:&str)->String { e.eval_in_page(p,js).trim_matches(|c| c=='\''||c=='"').to_string() }

#[test]
fn both_grid_axes_and_the_lanes_grid_axis_expose_final_tracks() {
    let mut e=engine();
    let mut p=load(&mut e, r#"<div id=g style="display:grid;width:50px;height:50px;grid-template-columns:[c]20px 30px;grid-template-rows:[r]10px 40px"></div>
        <div id=cols style="display:grid-lanes;width:50px;grid-template-columns:[c]20px 30px;grid-template-rows:repeat(2,7px)"></div>
        <div id=rows style="display:grid-lanes;grid-lanes-direction:row;height:50px;grid-template-rows:[r]10px 40px;grid-template-columns:repeat(2,9px)"></div>"#,800.);
    assert_eq!(eval(&e,&mut p,"['g','cols','rows'].map(function(id){var s=getComputedStyle(document.getElementById(id));return s.gridTemplateColumns+'|'+s.gridTemplateRows;}).join(';')"),
        "[c] 20px 30px|[r] 10px 40px;[c] 20px 30px|repeat(2, 7px);repeat(2, 9px)|[r] 10px 40px");
}

#[test]
fn collapsed_tracks_and_resize_are_read_before_the_script_returns() {
    let mut e=engine();
    let mut p=load(&mut e,r#"<div id=g style="display:grid;width:80px;grid-template-columns:repeat(auto-fit,[y]20px);grid-template-rows:10px"><i style="grid-column:2"></i></div>"#,800.);
    assert_eq!(eval(&e,&mut p,"var g=document.getElementById('g');var s=getComputedStyle(g);var a=s.gridTemplateColumns;g.style.width='40px';var b=s.gridTemplateColumns;g.firstChild.remove();[a,b,s.gridTemplateColumns].join('|')"),
        "[y] 0px [y] 20px [y] 0px [y] 0px|[y] 0px [y] 20px|[y] 0px [y] 0px");
    assert_eq!(eval(&e,&mut p,"g.style.gridTemplateColumns='[fresh] 10px 1fr';s.gridTemplateColumns"),"[fresh] 10px 30px");
}

#[test]
fn viewport_resize_replaces_the_used_repetition_count() {
    let mut e=engine();
    let mut p=load(&mut e,r#"<div id=g style="display:grid;grid-template-columns:repeat(auto-fill,20px)"></div>"#,80.);
    assert_eq!(eval(&e,&mut p,"getComputedStyle(document.getElementById('g')).gridTemplateColumns"),"20px 20px 20px 20px");
    e.relayout(&mut p,Viewport{width:40.,height:300.});
    assert_eq!(eval(&e,&mut p,"getComputedStyle(document.getElementById('g')).gridTemplateColumns"),"20px 20px");
}

#[test]
fn subgrid_serializes_only_its_local_lines_on_each_axis() {
    let mut e=engine();
    let mut p=load(&mut e,r#"<div id=p style="display:grid;width:100px;height:50px;grid-template-columns:[parent]50px [parent]50px [pend];grid-template-rows:20px 30px"><div id=s style="display:grid;grid-column:1/3;grid-row:1/3;grid-template-columns:subgrid [local] repeat(auto-fill,[b]);grid-template-rows:subgrid [] [r] [] [cut]"></div></div>"#,800.);
    assert_eq!(eval(&e,&mut p,"var s=getComputedStyle(document.getElementById('s'));[s.gridTemplateColumns,s.gridTemplateRows].join('|')"),"subgrid [local] [b] [b]|subgrid [] [r] []");
    assert_eq!(eval(&e,&mut p,"getComputedStyle(document.getElementById('p')).gridTemplateColumns"),"[parent] 50px [parent] 50px [pend]");
}

#[test]
fn no_box_and_non_grid_values_keep_computed_syntax_and_never_reuse_old_tracks() {
    let mut e=engine();
    let mut p=load(&mut e,r#"<div id=g style="display:grid;width:100px;grid-template-columns:[a]repeat(2,minmax(20%,1fr))"></div><div id=block style="grid-template-columns:repeat(auto-fit,20%)"></div>"#,800.);
    assert_eq!(eval(&e,&mut p,"var g=document.getElementById('g');var s=getComputedStyle(g);var a=s.gridTemplateColumns;g.style.display='none';[a,s.gridTemplateColumns,getComputedStyle(document.getElementById('block')).gridTemplateColumns].join('|')"),"[a] 50px 50px|[a] repeat(2, minmax(20%, 1fr))|repeat(auto-fit, 20%)");
    assert_eq!(eval(&e,&mut p,"g.style.display='grid';s.gridTemplateColumns"),"[a] 50px 50px");
}

#[test]
fn implicit_tracks_are_exposed_but_area_generated_names_are_not() {
    let mut e=engine();
    let mut p=load(&mut e,r#"<div id=areas style="display:grid;width:50px;grid-template-areas:'a b';grid-template-columns:[explicit]20px 30px;grid-template-rows:10px"><i style="grid-area:b"></i></div><div id=implicit style="display:grid;width:60px;grid-template-columns:[explicit]20px;grid-auto-columns:20px;grid-template-rows:10px"><i style="grid-column:3"></i></div>"#,800.);
    assert_eq!(eval(&e,&mut p,"['areas','implicit'].map(function(id){return getComputedStyle(document.getElementById(id)).gridTemplateColumns;}).join('|')"),"[explicit] 20px 30px|[explicit] 20px 20px 20px");
}

#[test]
fn writing_mode_wrappers_leave_template_queries_in_the_elements_logical_axes() {
    for mode in ["vertical-rl","vertical-lr","sideways-lr"] {
        let mut e=engine();
        let mut p=load(&mut e,&format!("<div id=g style='display:grid;writing-mode:{mode};width:70px;height:50px;grid-template-columns:[c]20px 30px;grid-template-rows:[r]10px 60px'></div>"),800.);
        assert_eq!(eval(&e,&mut p,"var s=getComputedStyle(document.getElementById('g'));s.gridTemplateColumns+'|'+s.gridTemplateRows"),"[c] 20px 30px|[r] 10px 60px","{mode}");
    }
}

#[test]
fn same_origin_frames_read_their_own_current_grid_and_their_ancestors_grid() {
    let inner=r#"<style>body{margin:0}#g{display:grid;grid-template-columns:repeat(auto-fill,20px)}</style><div id=g></div><script>function own(){return getComputedStyle(document.getElementById('g')).gridTemplateColumns}function ancestor(){return parent.getComputedStyle(parent.document.getElementById('outer')).gridTemplateColumns}</script>"#;
    let escaped=inner.replace('&',"&amp;").replace('"',"&quot;");
    let mut e=engine();
    let mut p=load(&mut e,&format!("<div id=outer style='display:grid;grid-template-columns:70px 30px'></div><iframe id=f style='width:80px;height:100px;border:0' srcdoc=\"{escaped}\"></iframe><iframe id=other style='width:40px;height:100px;border:0' srcdoc=\"{escaped}\"></iframe>"),800.);
    let deadline=std::time::Instant::now()+std::time::Duration::from_secs(5);
    let mut quiet=0;
    while std::time::Instant::now()<deadline {
        e.tick(&mut p);
        if !p.frames_loading() && p.frames.children.len()==2 && p.frames.children.iter().all(|frame|frame.page.is_some()) { quiet+=1; } else { quiet=0; }
        if quiet>=5 { break; }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(p.frames.children.len()==2 && p.frames.children.iter().all(|frame|frame.page.is_some()),"frames did not load");
    assert_eq!(eval(&e,&mut p,"var f=document.getElementById('f');var o=document.getElementById('other');[getComputedStyle(f.contentDocument.getElementById('g')).gridTemplateColumns,f.contentWindow.own(),getComputedStyle(o.contentDocument.getElementById('g')).gridTemplateColumns,f.contentWindow.ancestor()].join('|')"),"20px 20px 20px 20px|20px 20px 20px 20px|20px 20px|70px 30px");
    assert_eq!(eval(&e,&mut p,"f.contentDocument.getElementById('g').style.width='20px';getComputedStyle(f.contentDocument.getElementById('g')).gridTemplateColumns+'|'+f.contentWindow.own()"),"20px|20px");
}
