use rabbithole_engine::{Engine, Page, Viewport, net::Url};
#[path="../../../tests/support/nested_frames.rs"] mod fixture;

fn engine()->Engine {let mut engine=Engine::new_bundled_fonts_only();engine.scripting=true;engine.loader.load_scripts=true;engine}
fn settle(engine:&Engine,page:&mut Page) {
    let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);let mut quiet=0;
    while std::time::Instant::now()<deadline&&quiet<5 {
        let busy=page.frames_loading();engine.tick(page);std::thread::sleep(std::time::Duration::from_millis(10));
        if busy {quiet=0;}else{quiet+=1;}
    }
    assert!(!page.frames_loading(),"nested frames must finish loading");
}
fn load(engine:&mut Engine,source:&str,url:&str)->Page {let mut page=engine.load_html(source,Url::parse(url).unwrap(),Viewport{width:800.,height:600.});settle(engine,&mut page);page}
fn eval(engine:&Engine,page:&mut Page,js:&str)->String {engine.eval_in_page(page,js).trim_matches(|c|c=='\''||c=='"').to_string()}

#[test]
fn nested_frameset_documents_have_real_windows_documents_and_named_contexts() {
    let server=fixture::serve();let mut engine=engine();let mut page=load(&mut engine,fixture::ROOT,&server.url("/nested_frames"));
    assert_eq!(page.frames.children.len(),2);
    assert_eq!(page.frames.children[0].page.as_ref().unwrap().frames.children.len(),3);
    assert_eq!(eval(&engine,&mut page,"[frames.length,frames[0].frames.length].join('|')"),"2|3");
    assert_eq!(eval(&engine,&mut page,"[frames[0][0].document.body.textContent.trim(),frames[0][1].document.body.textContent.trim(),frames[0][2].document.body.textContent.trim(),frames[1].document.body.textContent.trim()].join('|')"),"LEFT|MIDDLE|RIGHT|BOTTOM");
    assert_eq!(eval(&engine,&mut page,"document.body.tagName"),"FRAMESET");
    assert_eq!(eval(&engine,&mut page,"document.getElementById('topFrame') instanceof HTMLFrameElement && document.getElementById('topFrame').contentWindow===frames[0] && document.getElementById('topFrame').contentDocument===frames[0].document"),"true");
    assert_eq!(eval(&engine,&mut page,"frames[0].document.getElementById('middleFrame').contentWindow===frames[0][1] && frames[0].document.getElementById('middleFrame').contentDocument.body.textContent.trim()==='MIDDLE'"),"true");
    assert_eq!(eval(&engine,&mut page,"frames===window && frames[0].parent===window && frames[0][0].parent===frames[0] && frames[0][0].top===window && window.topPane===frames[0] && frames[0].leftPane===frames[0][0]"),"true");
    assert!(page.take_console().iter().all(|(level,_)|level!="error"));
}

#[test]
fn frame_context_limits_and_cross_origin_document_access_match_iframes() {
    let local=fixture::serve();let remote=fixture::serve();let mut engine=engine();
    let html=format!("<!doctype html><frameset rows='*'><frame id='remote' src='{}'></frameset>",remote.url("/left.html"));
    let mut page=load(&mut engine,&html,&local.url("/"));
    assert_eq!(eval(&engine,&mut page,"frames.length===1 && document.getElementById('remote').contentWindow!==null && document.getElementById('remote').contentDocument===null && frames[0].document===null"),"true");
    let mut page=load(&mut engine,"<!doctype html><frameset><frame src='loop.html'></frameset>",&local.url("/"));
    let mut current=&page;let mut depth=0;
    while let Some(child)=current.frames.children.first() {depth+=1;assert!(depth<=4);current=child.page.as_ref().expect("loaded recursive frame");}
    assert_eq!(depth,4,"the shared iframe nesting limit must remain enforced");
    let _=eval(&engine,&mut page,"document.querySelector('frame').remove()");settle(&engine,&mut page);
    assert_eq!(eval(&engine,&mut page,"frames.length"),"0");
}

#[test]
fn frame_initial_blank_is_live_and_nonstandard_srcdoc_does_not_replace_src() {
    let server=fixture::serve();let mut engine=engine();let mut page=load(&mut engine,"<!doctype html><frameset id='layout'></frameset>",&server.url("/"));
    assert_eq!(eval(&engine,&mut page,"var inserted=document.createElement('frame');inserted.name='livePane';document.body.appendChild(inserted);inserted.contentDocument.body.textContent='live';frames.length===1 && inserted.contentWindow===frames[0] && inserted.contentDocument.body.textContent==='live'"),"true");
    let source=format!("<!doctype html><frameset><frame id='real' src='{}' srcdoc='<p>WRONG</p>'></frameset>",server.url("/left.html"));
    let mut page=load(&mut engine,&source,&server.url("/"));
    assert_eq!(eval(&engine,&mut page,"document.getElementById('real').contentDocument.body.textContent.trim()"),"LEFT");
}
