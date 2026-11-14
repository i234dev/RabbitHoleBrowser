//! A static document can finish before a script realm is requested.
use rabbithole_engine::{Engine, Viewport};
use rabbithole_engine::net::Url;

const VIEW: Viewport = Viewport { width: 800.0, height: 600.0 };

#[test]
fn late_script_realm_observes_a_completed_static_document() {
    let mut engine = Engine::new_bundled_fonts_only();
    let mut page = engine.load_html("<!doctype html><title>Static</title>", Url::parse("about:blank").unwrap(), VIEW);
    assert!(page.script.host.is_none(), "static loading stays lazy");
    assert_eq!(engine.eval_in_page(&mut page, "document.readyState"), "'complete'");
}

#[test]
fn early_script_realm_stays_loading_until_completion_and_fires_load_once() {
    let mut engine = Engine::new_bundled_fonts_only();
    let loaded = engine.loader.load_html("<!doctype html><title>Progressive</title>", Url::parse("https://example.test/").unwrap(), VIEW);
    let mut page = engine.build_page_unscripted(loaded, VIEW);
    assert_eq!(engine.eval_in_page(&mut page, "var seen=[];document.addEventListener('DOMContentLoaded',()=>seen.push('dom'));addEventListener('load',()=>seen.push('load'));document.readyState"), "'loading'");
    engine.finish_page_load(&mut page);
    assert_eq!(engine.eval_in_page(&mut page, "document.readyState+'|'+seen.join(',')"), "'complete|dom,load'");
    assert!(engine.continue_page_load(&mut page, None));
    assert_eq!(engine.eval_in_page(&mut page, "seen.join(',')"), "'dom,load'");
}
