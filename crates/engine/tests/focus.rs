//! Keyboard focus candidates must use rendering and tabindex, not style presence.
use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Page, Viewport};

fn load(engine: &mut Engine, body: &str) -> Page {
    engine.load_html(body, Url::parse("https://example.test/signin").unwrap(), Viewport { width: 800., height: 600. })
}

fn order(page: &Page) -> Vec<String> {
    page.sequential_focus_order().into_iter().map(|n| page.doc.attr(n, "id").unwrap_or("").to_string()).collect()
}

#[test]
fn sequential_focus_skips_hidden_disabled_and_negative_controls() {
    let mut engine = Engine::new_bundled_fonts_only();
    let page = load(&mut engine, r#"<!doctype html>
      <input id=identity>
      <input id=auxiliary type=password tabindex=-1 style="display:none">
      <input id=programmatic tabindex=-1>
      <input id=secret type=password>
      <button id=next>Next</button>
      <input id=disabled disabled>
      <input id=positive2 tabindex=2>
      <input id=positive1a tabindex=1><a id=positive1b href=/ tabindex=1>earlier</a>
      <div style="display:none"><input id=hiddenAncestor style="display:block"></div>
      <div style="visibility:hidden"><input id=invisible><input id=visibleOverride style="visibility:visible"></div>
      <div inert><input id=inertChild style="visibility:visible"></div>
      <input id=readonly readonly>
      <div id=editable contenteditable=true>Editable</div>
      <div id=notEditable contenteditable=FALSE>Not editable</div>
      <details><summary id=summary>More</summary><a id=closedContent href=/>Hidden</a></details>
    "#);
    assert_eq!(order(&page), ["positive1a", "positive1b", "positive2", "identity", "secret", "next", "visibleOverride", "readonly", "editable", "summary"]);
}

#[test]
fn disabled_fieldset_keeps_only_controls_in_its_first_legend() {
    let mut engine = Engine::new_bundled_fonts_only();
    let page = load(&mut engine, r#"<!doctype html>
      <fieldset disabled>
        <legend><input id=allowed><fieldset disabled><input id=nestedDisabled></fieldset></legend>
        <legend><input id=secondLegend></legend><input id=disabledByFieldset>
        <fieldset><input id=disabledNested></fieldset>
      </fieldset>
      <input id=after>
    "#);
    assert_eq!(order(&page), ["allowed", "after"]);
}

#[test]
fn focus_order_reflects_visibility_and_tabindex_changes() {
    let mut engine = Engine::new_bundled_fonts_only();
    let mut page = load(&mut engine, "<!doctype html><input id=a><input id=b><input id=c>");
    assert_eq!(order(&page), ["a", "b", "c"]);
    let result = engine.eval_in_page(&mut page, "document.getElementById('a').style.display='none';document.getElementById('b').setAttribute('tabindex','-1');document.getElementById('c').setAttribute('tabindex','2');'changed'");
    assert!(result.contains("changed"), "{result}");
    engine.restyle(&mut page);
    assert_eq!(order(&page), ["c"]);
}
