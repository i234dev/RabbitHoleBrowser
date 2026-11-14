//! Native dispatch must observe inline event attributes without script tags.

use rabbithole_engine::dom::Document;
use rabbithole_engine::net::Url;
use rabbithole_engine::scripting::{input_init, plain_init};
use rabbithole_engine::{Engine, Page, Viewport};

fn load(body: &str, scripting: bool) -> (Engine, Page) {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.scripting = scripting;
    engine.loader.load_scripts = scripting;
    let html = format!("<!doctype html><html><body>{body}</body></html>");
    let page = engine.load_html(&html, Url::parse("https://example.test/dir/page").unwrap(), Viewport { width: 800.0, height: 600.0 });
    assert!(!page.doc.descendants(Document::ROOT).any(|node| page.doc.is_element(node, "script")));
    (engine, page)
}

#[test]
fn inline_focus_input_and_change_execute_without_script_elements() {
    let (engine, mut page) = load(r#"<input id="field" onfocus="this.setAttribute('data-focus','yes')" oninput="this.setAttribute('data-input','yes')" onchange="this.setAttribute('data-change','yes')">"#, true);
    assert!(page.script.host.is_some(), "inline attributes already initialize the realm at load");
    let field = page.doc.element_by_id("field").unwrap();
    engine.focus_element(&mut page, Some(field));
    assert_eq!(page.doc.attr(field, "data-focus"), Some("yes"));
    engine.dispatch_event(&mut page, field, "input", input_init(Some("value")));
    assert_eq!(page.doc.attr(field, "data-input"), Some("yes"));
    engine.dispatch_event(&mut page, field, "change", plain_init(false));
    assert_eq!(page.doc.attr(field, "data-change"), Some("yes"));
}

#[test]
fn inline_change_returns_the_form_submission_request_without_script_elements() {
    let (engine, mut page) = load(r#"<form action="/chosen"><select id="choice" name="pick" onchange="this.form.submit()"><option value="old">Old</option><option value="new">New</option></select></form>"#, true);
    assert!(page.script.host.is_some());
    let field = page.doc.element_by_id("choice").unwrap();
    page.select_option(field, 1);
    let outcome = engine.dispatch_event(&mut page, field, "change", plain_init(false));
    let navigation = outcome.navigation.expect("the inline change callback must retain its navigation outcome");
    assert_eq!(navigation.request.url.path, "/chosen");
    assert_eq!(navigation.request.url.query.as_deref(), Some("pick=new"));
}

#[test]
fn inline_handlers_added_and_removed_on_the_ancestor_path_are_observed() {
    let (engine, mut page) = load(r#"<form id="parent" onchange="this.setAttribute('data-change','yes')"><input id="child"></form><input id="other" oninput="this.setAttribute('data-other','yes')">"#, true);
    assert!(page.script.host.is_some());
    let parent = page.doc.element_by_id("parent").unwrap();
    let child = page.doc.element_by_id("child").unwrap();
    let other = page.doc.element_by_id("other").unwrap();
    page.doc.set_attr(parent, "oninput", Some("this.setAttribute('data-target',event.target.id)"));
    engine.dispatch_event(&mut page, child, "input", input_init(Some("value")));
    assert_eq!(page.doc.attr(parent, "data-target"), Some("child"));
    assert_eq!(page.doc.attr(other, "data-other"), None, "an unrelated handler is not on this path");
    page.doc.set_attr(parent, "oninput", None);
    page.doc.set_attr(parent, "data-target", None);
    engine.dispatch_event(&mut page, child, "input", input_init(Some("again")));
    assert_eq!(page.doc.attr(parent, "data-target"), None, "removing an inline attribute must disable its cached handler");
    engine.dispatch_event(&mut page, other, "input", input_init(Some("other")));
    assert_eq!(page.doc.attr(other, "data-other"), Some("yes"));
}

#[test]
fn registered_and_idl_event_handlers_still_execute() {
    let (engine, mut page) = load("<input id=field>", true);
    engine.eval_in_page(&mut page, "document.getElementById('field').onchange=function(){this.setAttribute('data-idl','yes')};document.getElementById('field').addEventListener('input',function(){this.setAttribute('data-added','yes')});");
    let field = page.doc.element_by_id("field").unwrap();
    engine.dispatch_event(&mut page, field, "change", plain_init(false));
    engine.dispatch_event(&mut page, field, "input", input_init(Some("value")));
    assert_eq!(page.doc.attr(field, "data-idl"), Some("yes"));
    assert_eq!(page.doc.attr(field, "data-added"), Some("yes"));
}

#[test]
fn scripting_disabled_does_not_initialize_or_run_inline_handlers() {
    let (engine, mut page) = load(r#"<input id="field" oninput="this.setAttribute('data-ran','yes')">"#, false);
    assert!(page.script.host.is_none());
    let field = page.doc.element_by_id("field").unwrap();
    let outcome = engine.dispatch_event(&mut page, field, "input", input_init(Some("value")));
    assert!(outcome.navigation.is_none());
    assert_eq!(page.doc.attr(field, "data-ran"), None);
    assert!(page.script.host.is_none());
}
