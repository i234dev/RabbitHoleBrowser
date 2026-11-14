//! Load-time document.write controls through the real snapshot loader.
use rabbithole_engine::{net::Url, Engine, Viewport};

fn load(source:&str)->(Engine,rabbithole_engine::Page) {
    let mut engine=Engine::new_bundled_fonts_only();
    engine.loader.load_images=false;
    let page=engine.load_html(source,Url::parse("https://example.test/").unwrap(),Viewport{width:600.,height:400.});
    (engine,page)
}

#[test]
fn loading_page_writes_three_lines_in_order_before_its_original_tail() {
    let (engine,mut page)=load(r#"<!doctype html><body><div id=a>antes</div><script id=s>
        for(var i=1;i<=3;i++)document.write('<p class=w>linea '+i+'</p>');
    </script><div id=b>despues</div>"#);
    assert_eq!(engine.eval_in_page(&mut page,"Array.from(document.querySelectorAll('.w')).map(n=>n.textContent).join('|')"),"'linea 1|linea 2|linea 3'");
    let tags:Vec<_>=page.doc.child_elements(page.doc.body().unwrap()).map(|n|page.doc.element(n).unwrap().tag.as_str()).collect();
    assert_eq!(tags,["div","script","p","p","p","div"]);
    assert_eq!(page.doc.attr(page.doc.last_child(page.doc.body().unwrap()).unwrap(),"id"),Some("b"));
    assert!(page.take_console().iter().all(|(level,_)|level!="error"));
}

#[test]
fn loading_div_keeps_unicode_chunks_nested_scripts_microtasks_and_single_load() {
    let (engine,mut page)=load(r#"<!doctype html><body><div id=container><b id=a>antes</b><script id=s>
        window.seen=[];
        document.addEventListener('DOMContentLoaded',()=>seen.push('dom'));
        addEventListener('load',()=>seen.push('load'));
        document.write('<p id="uni');document.write('code">á•😀中 &am');document.write('p;</p>');
        document.write('<script id=nested>seen.push("inner");document.write("<i id=one>uno</i>");document.write("<i id=two>dos</i>");<\/script>');
        seen.push('outer');
        Promise.resolve().then(()=>document.write('<p id=micro>micro</p>'));
    </script><b id=b>despues</b></div><p id=outside>fuera</p>"#);
    let container=page.doc.element_by_id("container").unwrap();
    let ids:Vec<_>=page.doc.child_elements(container).map(|n|page.doc.attr(n,"id").unwrap().to_string()).collect();
    assert_eq!(ids,["a","s","unicode","nested","one","two","micro","b"]);
    assert_eq!(page.doc.text_content(page.doc.element_by_id("unicode").unwrap()),"á•😀中 &");
    assert_eq!(engine.eval_in_page(&mut page,"seen.join(',')+'|'+document.readyState"),"'inner,outer,dom,load|complete'");
    assert_eq!(page.doc.parent(page.doc.element_by_id("outside").unwrap()),page.doc.body());
    assert!(page.take_console().iter().all(|(level,_)|level!="error"));
}
