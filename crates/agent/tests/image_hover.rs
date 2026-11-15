//! Actual pointer actions on local source-only pages; images remain unfetched.
use std::time::Duration;
use rabbithole_agent::{call, Agent, Config};
use rabbithole_agent::browser::Frame;
use rabbithole_agent::ax::GeoMap;
use rabbithole_engine::dom::Document;
use rabbithole_engine::net::Url;
use serde_json::json;

fn fixture(html: &str) -> Agent {
    let mut agent = Agent::new(Config { images: false, settle_cap: Duration::from_millis(100), ..Config::default() });
    let tab = agent.new_tab(0);
    agent.active = Some(tab);
    let page = agent.engine.load_html(html, Url::parse_absolute("https://pointer.example.test/").unwrap(), agent.cfg.viewport);
    agent.tabs[tab].frames.push(Frame { page, parent: None });
    agent
}

fn reference(agent: &mut Agent, id: &str) -> String {
    let tab = agent.active.unwrap();
    let doc = &agent.tabs[tab].frames[0].page.doc;
    let node = doc.descendants(Document::ROOT).find(|&n| doc.attr(n, "id") == Some(id)).unwrap();
    format!("@e{}", agent.tabs[tab].refs.assign(0, node, "fixture", id))
}

fn snapshot(agent: &mut Agent) -> String {
    call(agent, "snapshot", &json!({})).unwrap().text
}

fn assert_ancestor_note(agent: &Agent, id: &str, result: &str) {
    let doc = &agent.tabs[0].frames[0].page.doc;
    let node = doc.descendants(Document::ROOT).find(|&n| doc.attr(n, "id") == Some(id)).unwrap();
    let reference = agent.tabs[0].refs.lookup(0, node).expect("fallback ancestor gets a stable ref");
    assert!(result.contains(&format!("nearest visible ancestor @e{reference}")), "{result}");
}

fn assert_untouched(agent: &Agent) {
    let doc = &agent.tabs[0].frames[0].page.doc;
    let result = doc.descendants(Document::ROOT).find(|&n| doc.attr(n, "id") == Some("result")).unwrap();
    assert_eq!(doc.text_content(result), "Untouched", "an inaccessible target must not activate its parent or another control");
}

#[test]
fn unloaded_image_hover_uses_its_alt_box_and_reveals_heading_and_link() {
    let mut agent = fixture("<!doctype html><style>img{font-size:16px;line-height:20px}.tip{display:none}img:hover+.tip{display:block}</style><main><img id=trigger src=unfetched.svg alt='Navigation preview'><section class=tip><h2>Preview details</h2><a href=about:blank>Open preview</a></section></main>");
    let target = reference(&mut agent, "trigger");
    let page = &agent.tabs[0].frames[0].page;
    let node = page.doc.descendants(Document::ROOT).find(|&n| page.doc.attr(n, "id") == Some("trigger")).unwrap();
    assert!(!page.images.contains_key(&node), "the fixture must exercise the unfetched image path");
    let geometry = GeoMap::build(page);
    let rect = geometry.get(node).unwrap().rect;
    assert!(rect.w > 30.0 && rect.h >= 20.0, "the image needs a real alt box before hovering: {rect:?}");
    assert!(!snapshot(&mut agent).contains("Preview details"));
    let result = call(&mut agent, "click", &json!({"ref":target,"hover":true})).unwrap().text;
    let after = snapshot(&mut agent);
    assert!(after.contains("heading") && after.contains("Preview details"), "{result}\n{after}");
    assert!(after.contains("link") && after.contains("Open preview"), "{result}\n{after}");
    assert!(!result.contains("nearest visible ancestor"), "the real alt box should be used: {result}");
    assert!(!agent.tabs[0].frames[0].page.images.contains_key(&node));
}

#[test]
fn boxless_image_hover_uses_the_nearest_visible_ancestor_and_explains_it() {
    let mut agent = fixture("<!doctype html><style>#menu{position:relative;width:180px;height:40px;background:#ddd}.tip{display:none;position:absolute;top:40px;left:0;width:200px}#menu:hover>.tip{display:block}</style><main><div id=menu><img id=trigger width=0 height=0 alt=''><section class=tip><h2>Menu details</h2><a href=about:blank>Menu destination</a></section></div></main>");
    let target = reference(&mut agent, "trigger");
    assert!(!snapshot(&mut agent).contains("Menu details"));
    let result = call(&mut agent, "click", &json!({"ref":target,"hover":true})).unwrap().text;
    assert_ancestor_note(&agent, "menu", &result);
    let after = snapshot(&mut agent);
    assert!(after.contains("heading") && after.contains("Menu details"), "{result}\n{after}");
    assert!(after.contains("link") && after.contains("Menu destination"), "{result}\n{after}");
}

#[test]
fn boxless_image_click_uses_the_visible_link_ancestor() {
    let mut agent = fixture("<!doctype html><main><h1 id=result>Before click</h1><a id=link href=about:blank style='display:block;width:180px;height:40px' onclick=\"event.preventDefault();document.getElementById('result').textContent='Activated destination'\"><img id=trigger width=0 height=0 alt=''></a></main>");
    let target = reference(&mut agent, "trigger");
    let result = call(&mut agent, "click", &json!({"ref":target})).unwrap().text;
    assert_ancestor_note(&agent, "link", &result);
    let after = snapshot(&mut agent);
    assert!(after.contains("heading") && after.contains("Activated destination"), "{result}\n{after}");
}

#[test]
fn boxless_fallback_does_not_activate_hidden_or_disabled_controls() {
    for element in ["<button id=trigger style='display:none'>Hidden</button>",
                    "<input id=trigger type=hidden>",
                    "<button id=trigger disabled style='width:0;height:0;padding:0;border:0'>Disabled</button>",
                    "<img id=trigger width=0 height=0 alt='' style='visibility:hidden'>",
                    "<span style='opacity:0'><img id=trigger width=0 height=0 alt=''></span>",
                    "<span inert><img id=trigger width=0 height=0 alt=''></span>"] {
        let html = format!("<!doctype html><main><h1 id=result>Untouched</h1><div style='width:180px;height:40px' onclick=\"document.getElementById('result').textContent='Wrong action'\">{element}</div></main>");
        let mut agent = fixture(&html);
        let target = reference(&mut agent, "trigger");
        assert!(call(&mut agent, "click", &json!({"ref":target})).is_err(), "must reject {element}");
        assert_untouched(&agent);
    }
}

#[test]
fn boxless_fallback_does_not_escape_clipping_or_click_an_unrelated_control() {
    for body in ["<div style='overflow:hidden;width:0;height:0'><img id=trigger width=40 height=20 alt='Clipped'></div>",
                 "<div style='position:relative;width:180px;height:40px'><img id=trigger width=0 height=0 alt=''><button style='position:absolute;inset:0' onclick=\"document.getElementById('result').textContent='Wrong action'\">Other button</button></div>",
                 "<div style='width:180px;height:40px;pointer-events:none' onclick=\"document.getElementById('result').textContent='Wrong action'\"><img id=trigger width=0 height=0 alt=''></div>",
                 "<div style='width:180px;height:40px' onclick=\"document.getElementById('result').textContent='Wrong action'\"><img id=trigger width=0 height=0 alt=''></div><div style='position:fixed;inset:0;background:#ccc;z-index:999'>Covering overlay</div>"] {
        let mut agent = fixture(&format!("<!doctype html><main><h1 id=result>Untouched</h1>{body}</main>"));
        let target = reference(&mut agent, "trigger");
        assert!(call(&mut agent, "click", &json!({"ref":target})).is_err(), "must not bypass the target's hit testing: {body}");
        assert_untouched(&agent);
    }
}
