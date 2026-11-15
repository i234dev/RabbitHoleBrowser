//! Fragment navigation keeps the live document; request counts alone cannot prove it.
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use rabbithole_agent::{call, Agent, Config};
use rabbithole_engine::net::{Request, Url};
use serde_json::{json, Value};

const APP: &str = r##"<!doctype html><title>Fragment fixture</title>
<a id=l href="#/active">Active</a><a id=c href="#/completed">Completed</a>
<a id=deep-link href="#deep">Deep</a><button id=kept>Kept</button><output id=result>initial</output>
<div style="height:1600px"></div><h2 id=deep>Deep target</h2><div style="height:1000px"></div>
<script>window.__hc=0;window.__events=[];
addEventListener('hashchange',function(e){__hc++;__events.push('hash');window.__last=[e.oldURL,e.newURL,e.isTrusted,e instanceof HashChangeEvent];document.getElementById('result').textContent=location.hash;});
addEventListener('popstate',function(e){__events.push('pop');window.__popTrusted=e.isTrusted&&e instanceof PopStateEvent;});
</script>"##;
const CHILD: &str = r##"<!doctype html><a href="#part">Child active</a><output id=result>child initial</output><div style="height:900px"></div><p id=part>Child target</p><script>window.__mark=321;window.__hc=0;addEventListener('hashchange',function(){__hc++;document.getElementById('result').textContent=location.hash;});</script>"##;

struct Server { port: u16, log: Arc<Mutex<Vec<String>>> }
impl Server {
    fn url(&self, path: &str) -> String { format!("http://127.0.0.1:{}{path}", self.port) }
    fn count(&self, method: &str, path: &str) -> usize {
        self.log.lock().unwrap().iter().filter(|line| line.starts_with(&format!("{method} {path} "))).count()
    }
}
fn server() -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let log = Arc::new(Mutex::new(Vec::new()));
    let requests = log.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let requests = requests.clone();
            std::thread::spawn(move || {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            if reader.read_line(&mut request).unwrap_or(0) == 0 { continue; }
            requests.lock().unwrap().push(request.trim_end().into());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" { break; }
            }
            let path = request.split_whitespace().nth(1).unwrap_or("/");
            let body = match path {
                "/child" => CHILD,
                "/frames" => "<!doctype html><title>Parent</title><iframe src='/child'></iframe><button>Parent kept</button>",
                "/other" => "<!doctype html><title>Different document</title><button>Other</button>",
                _ => APP,
            };
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len());
            let _ = stream.write_all(response.as_bytes());
            });
        }
    });
    Server { port, log }
}
fn agent() -> Agent {
    Agent::new(Config { profile_dir: None, settle_cap: Duration::from_secs(2), ..Config::default() })
}
fn tool(a: &mut Agent, name: &str, args: Value) -> String {
    call(a, name, &args).unwrap_or_else(|e| panic!("{name}: {e}")).text
}
fn eval(a: &mut Agent, code: &str) -> String {
    let ti = a.active.unwrap();
    a.engine.eval_in_page(&mut a.tabs[ti].frames[0], code)
}
fn reference(text: &str, label: &str) -> String {
    let tail = &text[text.find(label).unwrap_or_else(|| panic!("{label} missing: {text}")) + label.len()..];
    let tail = &tail[tail.find('@').unwrap()..];
    tail.chars().take_while(|c| *c == '@' || c.is_ascii_alphanumeric()).collect()
}

#[test]
fn click_fragment_keeps_realm_refs_hash_event_and_returns_only_action_diff() {
    let s = server();
    let mut a = agent();
    let snapshot = tool(&mut a, "open", json!({"url":s.url("/app"),"snapshot":true}));
    let kept = reference(&snapshot, "button \"Kept\"");
    let ti = a.active.unwrap();
    let document = a.tabs[ti].frames[0].doc.weak_identity();
    assert_eq!(eval(&mut a, "window.__mark=123"), "123");
    let text = tool(&mut a, "click", json!({"selector":"#l"}));
    assert!(text.contains("→ same-document navigation to"), "{text}");
    assert!(!text.contains("reloaded") && !text.contains("[page]") && !text.contains("no navigation"), "{text}");
    assert!(text.contains("#/active"), "{text}");
    assert_eq!(eval(&mut a, "JSON.stringify([__mark,location.hash,__hc,__last])"), format!("'[123,\"#/active\",1,[\"{}\",\"{}\",true,true]]'", s.url("/app"), s.url("/app#/active")));
    assert!(document.is_for(&a.tabs[ti].frames[0].doc));
    assert_eq!(a.tabs[ti].frames[0].url.to_string(), s.url("/app#/active"));
    assert!(a.resolve(ti, &kept).is_ok(), "pre-navigation ref remains usable");
    assert_eq!(s.count("GET", "/app"), 1);
}

#[test]
fn keyboard_open_and_history_traversal_keep_document_and_order_events() {
    let s = server();
    let mut a = agent();
    tool(&mut a, "open", json!({"url":s.url("/app")}));
    eval(&mut a, "window.__mark=123;document.getElementById('l').focus()");
    let text = tool(&mut a, "type", json!({"key":"Enter"}));
    assert!(text.contains("same-document navigation"), "{text}");
    tool(&mut a, "open", json!({"url":s.url("/app#/completed")}));
    assert_eq!(eval(&mut a, "__mark+'|'+__hc+'|'+history.length"), "'123|2|3'");
    tool(&mut a, "open", json!({"url":"back"}));
    assert_eq!(eval(&mut a, "location.hash+'|'+__events.join(',')+'|'+__popTrusted"), "'#/active|hash,hash,pop,hash|true'");
    tool(&mut a, "open", json!({"url":"forward"}));
    assert_eq!(eval(&mut a, "__mark+'|'+location.hash+'|'+__hc+'|'+history.length"), "'123|#/completed|4|3'");
    tool(&mut a, "click", json!({"selector":"#c"}));
    assert_eq!(eval(&mut a, "__hc+'|'+history.length"), "'4|3'");
    assert_eq!(s.count("GET", "/app"), 1);
}

#[test]
fn anchor_scroll_and_fragment_removal_keep_document_but_reload_and_post_replace_it() {
    let s = server();
    let mut a = agent();
    tool(&mut a, "open", json!({"url":s.url("/app")}));
    eval(&mut a, "window.__mark=123");
    tool(&mut a, "click", json!({"selector":"#deep-link"}));
    let ti = a.active.unwrap();
    assert!(a.tabs[ti].frames[0].page_scroll.1 > 500.0);
    tool(&mut a, "open", json!({"url":s.url("/app")}));
    assert_eq!(a.tabs[ti].frames[0].page_scroll.1, 0.0);
    assert_eq!(eval(&mut a, "__mark+'|'+location.hash+'|'+__hc"), "'123||2'");
    let old = a.tabs[ti].frames[0].doc.weak_identity();
    tool(&mut a, "open", json!({"url":"reload"}));
    assert!(!old.is_for(&a.tabs[ti].frames[0].doc));
    assert_eq!(eval(&mut a, "typeof __mark+'|'+__hc"), "'undefined|0'");
    assert_eq!(s.count("GET", "/app"), 2);
    let old = a.tabs[ti].frames[0].doc.weak_identity();
    let request = Request::new("POST", Url::parse(&s.url("/app#posted")).unwrap());
    a.navigate(ti, request, true).unwrap();
    assert!(!old.is_for(&a.tabs[ti].frames[0].doc));
    assert_eq!(s.count("POST", "/app"), 1);
    eval(&mut a, "window.__mark=999");
    let posted = a.tabs[ti].frames[0].doc.weak_identity();
    tool(&mut a, "open", json!({"url":"back"}));
    assert!(!posted.is_for(&a.tabs[ti].frames[0].doc), "same URL from an older document is not a same-document traversal");
    assert_eq!(eval(&mut a, "typeof __mark"), "'undefined'");
}

#[test]
fn another_resource_or_origin_is_never_treated_as_a_fragment_navigation() {
    let s = server();
    let other = server();
    let mut a = agent();
    tool(&mut a, "open", json!({"url":s.url("/app")}));
    eval(&mut a, "window.__mark=123");
    let ti = a.active.unwrap();
    let old = a.tabs[ti].frames[0].doc.weak_identity();
    tool(&mut a, "open", json!({"url":s.url("/other#/active")}));
    assert!(!old.is_for(&a.tabs[ti].frames[0].doc));
    let old = a.tabs[ti].frames[0].doc.weak_identity();
    tool(&mut a, "open", json!({"url":other.url("/other#/active")}));
    assert!(!old.is_for(&a.tabs[ti].frames[0].doc));
    assert_eq!(s.count("GET", "/other"), 1);
    assert_eq!(other.count("GET", "/other"), 1);
}

#[test]
fn child_fragment_updates_only_that_frame_and_preserves_parent_and_child_refs() {
    let s = server();
    let mut a = agent();
    let snapshot = tool(&mut a, "open", json!({"url":s.url("/frames"),"snapshot":true}));
    let child_link = reference(&snapshot, "link \"Child active\"");
    let ti = a.active.unwrap();
    let (frame, _) = a.resolve(ti, &child_link).unwrap();
    assert_ne!(frame, 0);
    let parent = a.tabs[ti].frames[0].doc.weak_identity();
    let child = a.tabs[ti].frames.get(frame).unwrap().page.doc.weak_identity();
    let text = tool(&mut a, "click", json!({"ref":child_link}));
    assert!(text.contains("same-document navigation"), "{text}");
    assert!(parent.is_for(&a.tabs[ti].frames[0].doc));
    assert!(child.is_for(&a.tabs[ti].frames.get(frame).unwrap().page.doc));
    assert_eq!(a.tabs[ti].frames.get(frame).unwrap().page.url.to_string(), s.url("/child#part"));
    assert_eq!(a.tabs[ti].frames[0].url.to_string(), s.url("/frames"));
    let result = a.tabs[ti].frames.with_page(&a.engine, frame, |page| a.engine.eval_in_page(page, "__mark+'|'+__hc+'|'+location.hash")).unwrap();
    assert_eq!(result, "'321|1|#part'");
    assert!(a.resolve(ti, &child_link).is_ok());
    assert_eq!(s.count("GET", "/child"), 1);
}

#[test]
fn fragment_in_a_new_tab_loads_its_own_document() {
    let s = server();
    let mut a = agent();
    tool(&mut a, "open", json!({"url":s.url("/app")}));
    eval(&mut a, "window.__mark=123");
    let original = a.active.unwrap();
    let document = a.tabs[original].frames[0].doc.weak_identity();
    tool(&mut a, "open", json!({"url":s.url("/app#/active"),"new_tab":true}));
    assert_ne!(a.active.unwrap(), original);
    assert_eq!(eval(&mut a, "typeof __mark+'|'+__hc"), "'undefined|0'");
    assert!(document.is_for(&a.tabs[original].frames[0].doc));
    assert_eq!(a.tabs[original].frames[0].url.to_string(), s.url("/app"));
    assert_eq!(s.count("GET", "/app"), 2);
}
