//! Same-document navigations through the production MCP Browser.
use super::*;
use rabbithole_engine::net::{Request, Url};

const APP: &str = r##"<!doctype html><title>Fragment fixture</title>
<a id=l href="#/active">Active</a><a id=c href="#/completed">Completed</a>
<a id=deep-link href="#deep">Deep</a><button id=kept>Kept</button><output id=result>initial</output>
<div style="height:1600px"></div><h2 id=deep>Deep target</h2><div style="height:1000px"></div>
<script>window.__hc=0;window.__events=[];
addEventListener('hashchange',function(e){__hc++;__events.push('hash');window.__last=[e.oldURL,e.newURL,e.isTrusted,e instanceof HashChangeEvent];document.getElementById('result').textContent=location.hash;});
addEventListener('popstate',function(e){__events.push('pop');window.__popTrusted=e.isTrusted&&e instanceof PopStateEvent;window.__popURL=location.href;});
</script>"##;
const CHILD: &str = r##"<!doctype html><a href="#part">Child active</a><output id=result>child initial</output><div style="height:900px"></div><p id=part>Child target</p><script>window.__mark=321;window.__hc=0;addEventListener('hashchange',function(){__hc++;document.getElementById('result').textContent=location.hash;});</script>"##;

fn fragment_server() -> Server {
    serve(Box::new(|path| {
        let body = match path {
            "/child" => CHILD,
            "/frames" => "<!doctype html><title>Parent</title><iframe src='/child'></iframe><button>Parent kept</button>",
            "/other" => "<!doctype html><title>Different document</title><button>Other</button>",
            _ => APP,
        };
        let (status, content_type, body, mut headers) = html(body).unwrap();
        headers.push(("Cache-Control".into(), "no-store".into()));
        Some((status, content_type, body, headers))
    }))
}
fn count(server: &Server, path: &str) -> usize {
    server.requests().iter().filter(|(actual, _)| actual == path).count()
}

fn eval(a: &mut Agent, code: &str) -> String {
    let ti = a.active.unwrap();
    a.engine.eval_in_page(&mut a.tabs[ti].frames[0], code)
}

#[test]
fn click_fragment_keeps_realm_refs_hash_event_and_returns_only_action_diff() {
    let s = fragment_server();
    let mut a = agent();
    let snapshot = ok(&mut a, "open", json!({"url":s.url("/app"),"snapshot":true}));
    let kept = ref_of(&snapshot, "button \"Kept\"");
    let ti = a.active.unwrap();
    let document = a.tabs[ti].frames[0].doc.weak_identity();
    assert_eq!(eval(&mut a, "window.__mark=123"), "123");
    let text = ok(&mut a, "click", json!({"selector":"#l"}));
    assert!(text.contains("→ same-document navigation to"), "{text}");
    assert!(!text.contains("reloaded") && !text.contains("[page]") && !text.contains("no navigation"), "{text}");
    assert!(text.contains("#/active"), "{text}");
    assert_eq!(eval(&mut a, "JSON.stringify([__mark,location.hash,__hc,__last])"), format!("'[123,\"#/active\",1,[\"{}\",\"{}\",true,true]]'", s.url("/app"), s.url("/app#/active")));
    assert!(document.is_for(&a.tabs[ti].frames[0].doc));
    assert_eq!(a.tabs[ti].frames[0].url.to_string(), s.url("/app#/active"));
    assert!(a.resolve(ti, &kept).is_ok(), "pre-navigation ref remains usable");
    assert_eq!(count(&s, "/app"), 1);
}

#[test]
fn keyboard_open_and_history_traversal_keep_document_and_order_events() {
    let s = fragment_server();
    let mut a = agent();
    ok(&mut a, "open", json!({"url":s.url("/app")}));
    eval(&mut a, "window.__mark=123;document.getElementById('l').focus()");
    let text = ok(&mut a, "type", json!({"key":"Enter"}));
    assert!(text.contains("same-document navigation"), "{text}");
    ok(&mut a, "open", json!({"url":s.url("/app#/completed")}));
    assert_eq!(eval(&mut a, "__mark+'|'+__hc+'|'+history.length"), "'123|2|3'");
    ok(&mut a, "open", json!({"url":"back"}));
    assert_eq!(eval(&mut a, "location.hash+'|'+__events.join(',')+'|'+__popTrusted"), "'#/active|hash,hash,pop,hash|true'");
    assert_eq!(eval(&mut a, "JSON.stringify([__popURL,__last[0],__last[1]])"), format!("'[\"{}\",\"{}\",\"{}\"]'", s.url("/app#/active"), s.url("/app#/completed"), s.url("/app#/active")));
    ok(&mut a, "open", json!({"url":"forward"}));
    assert_eq!(eval(&mut a, "__mark+'|'+location.hash+'|'+__hc+'|'+history.length"), "'123|#/completed|4|3'");
    ok(&mut a, "click", json!({"selector":"#c"}));
    assert_eq!(eval(&mut a, "__hc+'|'+history.length"), "'4|3'");
    assert_eq!(count(&s, "/app"), 1);
}

#[test]
fn anchor_scroll_and_fragment_removal_keep_document_but_reload_and_post_replace_it() {
    let s = fragment_server();
    let mut a = agent();
    ok(&mut a, "open", json!({"url":s.url("/app")}));
    eval(&mut a, "window.__mark=123");
    ok(&mut a, "click", json!({"selector":"#deep-link"}));
    let ti = a.active.unwrap();
    assert!(a.tabs[ti].frames[0].page_scroll.1 > 500.0);
    ok(&mut a, "open", json!({"url":s.url("/app")}));
    assert_eq!(a.tabs[ti].frames[0].page_scroll.1, 0.0);
    assert_eq!(eval(&mut a, "__mark+'|'+location.hash+'|'+__hc"), "'123||2'");
    let old = a.tabs[ti].frames[0].doc.weak_identity();
    ok(&mut a, "open", json!({"url":"reload"}));
    assert!(!old.is_for(&a.tabs[ti].frames[0].doc));
    assert_eq!(eval(&mut a, "typeof __mark+'|'+__hc"), "'undefined|0'");
    assert_eq!(count(&s, "/app"), 2);
    let old = a.tabs[ti].frames[0].doc.weak_identity();
    let request = Request::new("POST", Url::parse(&s.url("/app#posted")).unwrap());
    a.navigate(ti, request, true).unwrap();
    assert!(!old.is_for(&a.tabs[ti].frames[0].doc));
    assert_eq!(count(&s, "/app"), 3, "POST loads the existing path again");
    eval(&mut a, "window.__mark=999");
    let posted = a.tabs[ti].frames[0].doc.weak_identity();
    ok(&mut a, "open", json!({"url":"back"}));
    assert!(!posted.is_for(&a.tabs[ti].frames[0].doc), "same URL from an older document is not a same-document traversal");
    assert_eq!(eval(&mut a, "typeof __mark"), "'undefined'");
}

#[test]
fn another_resource_or_origin_is_never_treated_as_a_fragment_navigation() {
    let s = fragment_server();
    let other = fragment_server();
    let mut a = agent();
    ok(&mut a, "open", json!({"url":s.url("/app")}));
    eval(&mut a, "window.__mark=123");
    let ti = a.active.unwrap();
    let old = a.tabs[ti].frames[0].doc.weak_identity();
    ok(&mut a, "open", json!({"url":s.url("/other#/active")}));
    assert!(!old.is_for(&a.tabs[ti].frames[0].doc));
    let old = a.tabs[ti].frames[0].doc.weak_identity();
    ok(&mut a, "open", json!({"url":other.url("/other#/active")}));
    assert!(!old.is_for(&a.tabs[ti].frames[0].doc));
    assert_eq!(count(&s, "/other"), 1);
    assert_eq!(count(&other, "/other"), 1);
}

#[test]
fn child_fragment_updates_only_that_frame_and_preserves_parent_and_child_refs() {
    let s = fragment_server();
    let mut a = agent();
    let snapshot = ok(&mut a, "open", json!({"url":s.url("/frames"),"snapshot":true}));
    let child_link = ref_of(&snapshot, "link \"Child active\"");
    let ti = a.active.unwrap();
    let (frame, _) = a.resolve(ti, &child_link).unwrap();
    assert_ne!(frame, 0);
    let parent = a.tabs[ti].frames[0].doc.weak_identity();
    let child = a.tabs[ti].frames.get(frame).unwrap().page.doc.weak_identity();
    let text = ok(&mut a, "click", json!({"ref":child_link}));
    assert!(text.contains("same-document navigation"), "{text}");
    assert!(parent.is_for(&a.tabs[ti].frames[0].doc));
    assert!(child.is_for(&a.tabs[ti].frames.get(frame).unwrap().page.doc));
    assert_eq!(a.tabs[ti].frames.get(frame).unwrap().page.url.to_string(), s.url("/child#part"));
    assert_eq!(a.tabs[ti].frames[0].url.to_string(), s.url("/frames"));
    let result = a.tabs[ti].frames.with_page(&a.engine, frame, |page| a.engine.eval_in_page(page, "__mark+'|'+__hc+'|'+location.hash")).unwrap();
    assert_eq!(result, "'321|1|#part'");
    assert!(a.resolve(ti, &child_link).is_ok());
    assert_eq!(count(&s, "/child"), 1);
}

#[test]
fn fragment_in_a_new_tab_loads_its_own_document() {
    let s = fragment_server();
    let mut a = agent();
    ok(&mut a, "open", json!({"url":s.url("/app")}));
    eval(&mut a, "window.__mark=123");
    let original = a.active.unwrap();
    let document = a.tabs[original].frames[0].doc.weak_identity();
    ok(&mut a, "open", json!({"url":s.url("/app#/active"),"new_tab":true}));
    assert_ne!(a.active.unwrap(), original);
    assert_eq!(eval(&mut a, "typeof __mark+'|'+__hc"), "'undefined|0'");
    assert!(document.is_for(&a.tabs[original].frames[0].doc));
    assert_eq!(a.tabs[original].frames[0].url.to_string(), s.url("/app"));
    assert_eq!(count(&s, "/app"), 2);
}

#[test]
fn hashchange_requested_navigation_is_followed_for_fragments_and_another_document() {
    let s = fragment_server();
    let mut a = agent();
    ok(&mut a, "open", json!({"url":s.url("/app")}));
    eval(&mut a, "window.__mark=123;addEventListener('hashchange',function(){if(location.hash==='#/active'){location.assign('#/completed');}else if(location.hash==='#deep'){localStorage.setItem('fragment-before-leave',__mark+'|'+__hc+'|'+location.hash);location.assign('/other');}})");
    let ti = a.active.unwrap();
    let document = a.tabs[ti].frames[0].doc.weak_identity();
    let text = ok(&mut a, "click", json!({"selector":"#l"}));
    assert!(text.contains("same-document navigation") && text.contains("#/completed"), "{text}");
    assert!(document.is_for(&a.tabs[ti].frames[0].doc));
    assert_eq!(eval(&mut a, "__mark+'|'+__hc+'|'+location.hash"), "'123|2|#/completed'");
    assert_eq!(count(&s, "/app"), 1);
    let text = ok(&mut a, "click", json!({"selector":"#deep-link"}));
    assert!(text.contains("navigated to") && text.contains("/other"), "{text}");
    assert!(!document.is_for(&a.tabs[ti].frames[0].doc));
    assert_eq!(a.tabs[ti].frames[0].url.to_string(), s.url("/other"));
    assert_eq!(eval(&mut a, "localStorage.getItem('fragment-before-leave')"), "'123|3|#deep'");
    assert_eq!(count(&s, "/other"), 1);
}
