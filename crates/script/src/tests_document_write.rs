//! Causal document.write ordering/continuation controls, without DOM repair.
use crate::tests::{find,run,setup};
use rabbithole_dom::{Document,NodeId};

fn assert_ids(doc:&Document,parent:NodeId,expected:&[&str]) {
    let actual:Vec<_>=doc.child_elements(parent).filter_map(|node|doc.attr(node,"id").map(str::to_owned)).collect();
    assert_eq!(actual.join("|"),expected.join("|"),"{}",doc.dump());
}

#[test]
fn loading_script_writes_stay_in_call_order_before_the_original_tail() {
    let (mut env,mut host)=setup("<div id=a>antes</div><script id=s></script><div id=b>despues</div>");
    host.set_current_script(Some(find(&env,"script")));
    run(&mut env,&mut host,r#"
        for(var i=1;i<=3;i++) document.write('<p class=w id=w'+i+'>linea '+i+'</p>');
    "#);
    host.set_current_script(None);
    assert!(env.errors().is_empty(),"{:?}",env.errors());
    assert_ids(&env.doc,env.doc.body().unwrap(),&["a","s","w1","w2","w3","b"]);
    let text:Vec<_>=env.doc.descendants(Document::ROOT).filter(|&node|env.doc.attr(node,"class")==Some("w")).map(|node|env.doc.text_content(node)).collect();
    assert_eq!(text.join("|"),"linea 1|linea 2|linea 3");
}

#[test]
fn loading_script_inside_a_div_keeps_its_parent_and_tail() {
    let (mut env,mut host)=setup("<section id=outer><div id=container><b id=a>antes</b><script id=s></script><b id=b>despues</b></div><i id=outside>fuera</i></section>");
    host.set_current_script(Some(find(&env,"script")));
    run(&mut env,&mut host,r#"
        for(var i=1;i<=3;i++) document.write('<p class=w id=w'+i+'>linea '+i+'</p>');
    "#);
    host.set_current_script(None);
    let container=env.doc.element_by_id("container").unwrap();
    assert_ids(&env.doc,container,&["a","s","w1","w2","w3","b"]);
    for id in ["w1","w2","w3"] {assert_eq!(env.doc.parent(env.doc.element_by_id(id).unwrap()),Some(container));}
    assert_ids(&env.doc,env.doc.element_by_id("outer").unwrap(),&["container","outside"]);
    assert!(env.errors().is_empty(),"{:?}",env.errors());
}

#[test]
fn loading_writes_continue_fragmented_tags_unicode_entities_and_crlf() {
    let (mut env,mut host)=setup("<div id=a>antes</div><script id=s></script><div id=b>despues</div>");
    host.set_current_script(Some(find(&env,"script")));
    run(&mut env,&mut host,r#"
        document.write('<p id="uni');
        document.write('code">á•😀中 &am');
        document.write('p;\r');
        document.write('');
        document.write('\nfin</p><p id=two>');
        document.write('segunda</p>');
    "#);
    host.set_current_script(None);
    assert!(env.errors().is_empty(),"{:?}",env.errors());
    assert_ids(&env.doc,env.doc.body().unwrap(),&["a","s","unicode","two","b"]);
    assert_eq!(env.doc.text_content(env.doc.element_by_id("unicode").unwrap()),"á•😀中 &\nfin");
    assert_eq!(env.doc.text_content(env.doc.element_by_id("two").unwrap()),"segunda");
}

#[test]
fn loading_nested_parser_scripts_write_before_outer_continuation() {
    let (mut env,mut host)=setup("<div id=a>antes</div><script id=s></script><div id=b>despues</div>");
    host.set_current_script(Some(find(&env,"script")));
    let logs=run(&mut env,&mut host,r#"
        window.sequence=[];
        document.write('<p id=w1>uno</p>');
        document.write('<script id=nested>sequence.push("inner");document.write("<i id=n1>interior 1</i>");document.write("<i id=n2>interior 2</i>");<\/script>');
        sequence.push('outer');
        document.write('<p id=w2>dos</p>');
        console.log(sequence.join(','));
    "#);
    host.set_current_script(None);
    assert!(env.errors().is_empty(),"{:?}",env.errors());
    assert_eq!(logs,["inner,outer"]);
    assert_ids(&env.doc,env.doc.body().unwrap(),&["a","s","w1","nested","n1","n2","w2","b"]);
}

#[test]
fn separate_loading_scripts_keep_separate_write_positions() {
    let (mut env,mut host)=setup("<div id=a></div><script id=s1></script><b id=b1></b><script id=s2></script><b id=b2></b>");
    for (script,prefix) in [("s1","x"),("s2","y")] {
        host.set_current_script(Some(env.doc.element_by_id(script).unwrap()));
        run(&mut env,&mut host,&format!("document.write('<i id={prefix}1></i>');document.write('<i id={prefix}2></i>');"));
        host.set_current_script(None);
    }
    assert!(env.errors().is_empty(),"{:?}",env.errors());
    assert_ids(&env.doc,env.doc.body().unwrap(),&["a","s1","x1","x2","b1","s2","y1","y2","b2"]);
}

#[test]
fn script_created_parser_keeps_nested_writes_and_chunk_continuation() {
    let (mut env,mut host)=setup("<p id=old>old</p>");host.mark_loaded();
    run(&mut env,&mut host,r#"
        document.open();
        document.write('<main id=target><p id="one');
        document.write('">á•</p><script id=nested>document.write("<i id=inside>中</i>");<\/script>');
        document.write('<p id=two>segunda</p></main>');
        document.close();
    "#);
    assert!(env.errors().is_empty(),"{:?}",env.errors());
    assert_ids(&env.doc,env.doc.element_by_id("target").unwrap(),&["one","nested","inside","two"]);
    assert_eq!(env.doc.text_content(env.doc.element_by_id("one").unwrap()),"á•");
}
