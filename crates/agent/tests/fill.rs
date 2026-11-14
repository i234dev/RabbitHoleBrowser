//! Fill outcomes and reference lifetime controls against loopback fixtures.
//! All content is synthetic; no authenticated sites or user data.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rabbithole_agent::{call, Agent, Config};
use serde_json::{json, Value};

type Route = Box<dyn Fn(&str) -> Option<(u16, &'static str, Vec<u8>, Vec<(String, String)>)> + Send + Sync>;

struct Server {
    port: u16,
    log: Arc<Mutex<Vec<(String, Vec<(String, String)>)>>>,
}

impl Server {
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }
    fn requests(&self) -> Vec<(String, Vec<(String, String)>)> {
        self.log.lock().unwrap().clone()
    }
}

fn serve(routes: Route) -> Server {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let log = Arc::new(Mutex::new(Vec::new()));
    let log2 = log.clone();
    let routes = Arc::new(routes);
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { continue };
            let (routes, log) = (routes.clone(), log2.clone());
            std::thread::spawn(move || {
                let mut w = s.try_clone().unwrap();
                let mut r = BufReader::new(s);
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let mut headers = Vec::new();
                    loop {
                        let mut h = String::new();
                        if r.read_line(&mut h).unwrap_or(0) == 0 {
                            return;
                        }
                        let h = h.trim_end().to_string();
                        if h.is_empty() {
                            break;
                        }
                        if let Some((k, v)) = h.split_once(':') {
                            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
                        }
                    }
                    let len = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse::<usize>().ok()).unwrap_or(0);
                    let mut body = vec![0; len];
                    let _ = r.read_exact(&mut body);
                    log.lock().unwrap().push((path.clone(), headers));
                    let (status, ct, body, extra) = routes(&path).unwrap_or((404, "text/plain", b"not found".to_vec(), vec![]));
                    let mut head = format!("HTTP/1.1 {status} X\r\nContent-Type: {ct}\r\nContent-Length: {}\r\n", body.len());
                    for (k, v) in extra {
                        head.push_str(&format!("{k}: {v}\r\n"));
                    }
                    head.push_str("\r\n");
                    if w.write_all(head.as_bytes()).and_then(|_| w.write_all(&body)).is_err() {
                        return;
                    }
                }
            });
        }
    });
    Server { port, log }
}

fn html(body: &'static str) -> Option<(u16, &'static str, Vec<u8>, Vec<(String, String)>)> {
    Some((200, "text/html; charset=utf-8", body.as_bytes().to_vec(), vec![]))
}

fn agent() -> Agent {
    Agent::new(Config { profile_dir: None, settle_cap: Duration::from_secs(3), ..Config::default() })
}

fn ok(a: &mut Agent, tool: &str, args: Value) -> String {
    match call(a, tool, &args) {
        Ok(o) => o.text,
        Err(e) => panic!("{tool} {args} failed: {e}"),
    }
}

fn err(a: &mut Agent, tool: &str, args: Value) -> String {
    match call(a, tool, &args) {
        Ok(o) => panic!("{tool} {args} should fail, got: {}", o.text),
        Err(e) => e,
    }
}

/// The ref printed right after `needle` (e.g. `button "Go"` → `@e3`).
fn ref_of(text: &str, needle: &str) -> String {
    let i = text.find(needle).unwrap_or_else(|| panic!("`{needle}` not in:\n{text}"));
    let rest = &text[i + needle.len()..];
    let j = rest.find('@').unwrap_or_else(|| panic!("no ref after `{needle}` in:\n{text}"));
    rest[j..].chars().take_while(|c| *c == '@' || c.is_ascii_alphanumeric()).collect()
}

fn control_value(a: &Agent, id: &str) -> String {
    let page = &a.tabs[a.active.unwrap()].frames[0].page;
    page.control_text(page.doc.element_by_id(id).unwrap())
}

#[test]
fn fill_select_change_follows_form_submit_and_returns_settled_summary() {
    let srv = serve(Box::new(|path| {
        if path == "/" {
            return html(r#"<html><body><form action="/picked" method="get"><label>Choice<select id="choice" name="pick" onchange="this.form.submit()"><option value="old">Old</option><option value="new">New</option></select></label></form></body></html>"#);
        }
        if path.starts_with("/picked?") {
            return html(r#"<html><body><h1 id="ready">Waiting</h1><script>setTimeout(function(){document.getElementById('ready').textContent='Pick ready'},20)</script></body></html>"#);
        }
        None
    }));
    let mut a = agent();
    ok(&mut a, "open", json!({"url":srv.url("/"),"snapshot":true}));
    let ti = a.active.unwrap();
    let node = a.tabs[ti].frames[0].page.doc.element_by_id("choice").unwrap();
    let field = format!("@e{}", a.tabs[ti].refs.lookup(0, node).unwrap());
    let output = ok(&mut a, "fill", json!({"ref":field,"value":"New"}));
    assert!(output.contains("→ navigated to") && output.contains("pick=new"), "{output}");
    assert!(output.contains("h1 \"Pick ready\"") && output.contains("Summary only"), "{output}");
    assert!(err(&mut a, "fill", json!({"ref":field,"value":"Old"})).contains("previous page"));
    assert_eq!(srv.requests().iter().filter(|(path, _)| path.starts_with("/picked?")).count(), 1);
}

#[test]
fn fill_input_navigation_stops_change_pending_fields_and_explicit_submit() {
    let srv = serve(Box::new(|path| {
        if path == "/" {
            return html(r#"<html><body><form action="/wrong-submit"><label>First<input name="a" oninput="location.href='/arrived?tail='+document.getElementById('tail').value" onchange="location.href='/wrong-change'"></label><label>Tail<input id="tail" name="z" value="untouched" oninput="location.href='/wrong-tail'"></label><button type="submit">Send</button></form></body></html>"#);
        }
        if path.starts_with("/arrived?") {
            return html("<html><body><h1>Input arrived</h1></body></html>");
        }
        html("<html><body><h1>Wrong navigation</h1></body></html>")
    }));
    let mut a = agent();
    let snapshot = ok(&mut a, "open", json!({"url":srv.url("/"),"snapshot":true}));
    let stale = ref_of(&snapshot, "textbox \"Tail\"");
    // Full snapshots publish element refs; list forms before using @f1.
    assert!(ok(&mut a, "fill", json!({})).contains("@f1 form"));
    let output = ok(&mut a, "fill", json!({"ref":"@f1","values":{"a":"go","z":"must-not-write"},"submit":true}));
    assert!(output.contains("→ navigated to") && output.contains("tail=untouched") && output.contains("Input arrived"), "{output}");
    assert!(output.contains("Not attempted after navigation: \"z\"") && output.contains("submit=true was not attempted"), "{output}");
    assert!(srv.requests().iter().all(|(path, _)| !path.starts_with("/wrong-")));
    assert!(err(&mut a, "fill", json!({"ref":stale,"value":"late"})).contains("previous page"));
}

#[test]
fn fill_waits_for_input_timer_navigation_before_the_next_field() {
    let srv = serve(Box::new(|path| {
        if path == "/" {
            return html(r#"<html><body><form><label>First<input name="a" oninput="setTimeout(function(){location.href='/delayed?tail='+document.getElementById('tail').value},20)"></label><label>Tail<input id="tail" name="z" value="untouched"></label></form></body></html>"#);
        }
        html("<html><body><h1>Delayed arrived</h1></body></html>")
    }));
    let mut a = agent();
    ok(&mut a, "open", json!({"url":srv.url("/")}));
    let output = ok(&mut a, "fill", json!({"ref":"@f1","values":{"a":"go","z":"must-not-write"}}));
    assert!(output.contains("→ navigated to") && output.contains("tail=untouched") && output.contains("Delayed arrived"), "{output}");
    assert!(output.contains("Not attempted after navigation: \"z\""), "{output}");
}

#[test]
fn fill_values_without_ref_resolves_each_field_ref_and_reports_ambiguous_forms() {
    let srv = serve(Box::new(|_| html(r#"<html><body><form><label>First<input id="first" name="first" value="one"></label></form><form><label>Second<input id="second" name="second" value="two"></label></form><label>Free<input id="free" value="three"></label></body></html>"#)));
    let mut a = agent();
    let snapshot = ok(&mut a, "open", json!({"url":srv.url("/"),"snapshot":true}));
    let first = ref_of(&snapshot, "textbox \"First\"");
    let second = ref_of(&snapshot, "textbox \"Second\"");
    let free = ref_of(&snapshot, "textbox \"Free\"");
    let mut values = serde_json::Map::new();
    values.insert(first, json!("café 🐇"));
    values.insert(second, json!("東京"));
    values.insert(free, json!("outside forms"));
    let output = ok(&mut a, "fill", json!({"values":Value::Object(values)}));
    assert!(output.contains("Filled:"), "{output}");
    assert_eq!(control_value(&a, "first"), "café 🐇");
    assert_eq!(control_value(&a, "second"), "東京");
    assert_eq!(control_value(&a, "free"), "outside forms");
    let error = err(&mut a, "fill", json!({"values":{"First":"must-not-guess"}}));
    assert!(error.contains("exactly one form") && error.contains("found 2") && error.contains("@f1 form") && error.contains("@f2 form"), "{error}");
    assert_eq!(control_value(&a, "first"), "café 🐇");
}

#[test]
fn fill_values_without_ref_uses_the_only_form_and_existing_control_conversions() {
    let srv = serve(Box::new(|path| {
        if path == "/" {
            return html(r#"<html><body><form id="form" action="/sole"><label>Name<input name="q" required></label><label><input type="checkbox" name="remember">Remember</label><label><input type="radio" name="choice" value="es">Spanish</label><label><input type="radio" name="choice" value="en">English</label><select name="lang" aria-label="Language"><option value="es">Spanish</option><option value="en">English</option></select><button type="submit">Send</button></form></body></html>"#);
        }
        html("<html><body><h1>Sole form arrived</h1></body></html>")
    }));
    let mut a = agent();
    ok(&mut a, "open", json!({"url":srv.url("/")}));
    let output = ok(&mut a, "fill", json!({"values":{"Name":"café","Remember":true,"choice":"English","Language":"English"},"submit":true}));
    assert!(output.contains("→ navigated to") && output.contains("Sole form arrived"), "{output}");
    let path = srv.requests().into_iter().find(|(path, _)| path.starts_with("/sole?")).unwrap().0;
    for expected in ["q=caf%C3%A9", "remember=on", "choice=en", "lang=en"] {
        assert!(path.contains(expected), "{expected} absent in {path}");
    }
}

#[test]
fn fill_submit_cancellation_stays_on_page_and_uncancelled_submit_uses_click_path() {
    let srv = serve(Box::new(|path| {
        if path == "/" {
            return html(r#"<html><body><form action="/sent" onsubmit="if(document.getElementById('cancel').checked){event.preventDefault();document.getElementById('state').textContent='cancelled'}"><label>Name<input id="name" name="q"></label><label><input id="cancel" name="cancel" type="checkbox" checked>Cancel</label><button type="submit">Send</button></form><p id="state">initial</p></body></html>"#);
        }
        html("<html><body><h1>Sent</h1></body></html>")
    }));
    let mut a = agent();
    ok(&mut a, "open", json!({"url":srv.url("/")}));
    let output = ok(&mut a, "fill", json!({"ref":"@f1","values":{"Name":"first"},"submit":true}));
    assert!(output.contains("Not submitted") && output.contains("cancelled"), "{output}");
    assert_eq!(control_value(&a, "name"), "first");
    assert!(srv.requests().iter().all(|(path, _)| !path.starts_with("/sent")));
    let output = ok(&mut a, "fill", json!({"ref":"@f1","values":{"Cancel":false,"Name":"second"},"submit":true}));
    assert!(output.contains("→ navigated to") && output.contains("q=second") && output.contains("Sent"), "{output}");
}

#[test]
fn fill_focus_replacement_does_not_write_through_a_detached_ref() {
    let srv = serve(Box::new(|_| html(r#"<html><body><form id="form"><label>Original<input id="old" name="a" onfocus="this.remove();document.getElementById('form').insertAdjacentHTML('afterbegin','<input id=replacement name=a value=replacement>')"></label></form></body></html>"#)));
    let mut a = agent();
    let snapshot = ok(&mut a, "open", json!({"url":srv.url("/"),"snapshot":true}));
    let old = ref_of(&snapshot, "textbox \"Original\"");
    let output = err(&mut a, "fill", json!({"ref":old,"value":"must-not-write"}));
    assert!(output.contains("no longer exists") && output.contains("Not filled"), "{output}");
    assert_eq!(control_value(&a, "replacement"), "replacement");
    let ti = a.active.unwrap();
    assert!(a.resolve(ti, &old).is_err());
    let snapshot = ok(&mut a, "snapshot", json!({}));
    let node = a.tabs[ti].frames[0].page.doc.element_by_id("replacement").unwrap();
    let new = format!("@e{}", a.tabs[ti].refs.lookup(0, node).unwrap_or_else(|| panic!("replacement has no ref in {snapshot}")));
    ok(&mut a, "fill", json!({"ref":new,"value":"fresh value"}));
    assert_eq!(control_value(&a, "replacement"), "fresh value");
}

#[test]
fn fill_rechecks_later_fields_after_input_callbacks_change_them() {
    let srv = serve(Box::new(|path| {
        let change = match path {
            "/disabled" => "document.getElementById('tail').setAttribute('disabled','')",
            "/readonly" => "document.getElementById('tail').setAttribute('readonly','')",
            "/removed" => "document.getElementById('tail').remove()",
            _ => "document.getElementById('tail').setAttribute('type','button')",
        };
        let page = format!("<html><body><form><label>First<input name=\"a\" oninput=\"{change}\"></label><label>Tail<input id=\"tail\" name=\"z\" value=\"keep\"></label></form></body></html>");
        Some((200, "text/html; charset=utf-8", page.into_bytes(), vec![]))
    }));
    let mut a = agent();
    for path in ["/disabled", "/readonly", "/removed", "/type"] {
        ok(&mut a, "open", json!({"url":srv.url(path)}));
        let ti = a.active.unwrap();
        let form = a.tabs[ti].frames[0].page.doc.descendants(rabbithole_engine::dom::Document::ROOT).find(|&node| a.tabs[ti].frames[0].page.doc.is_element(node, "form")).unwrap();
        let form = format!("@f{}", a.tabs[ti].refs.assign_form(0, form));
        let tail = a.tabs[ti].frames[0].page.doc.element_by_id("tail").unwrap();
        let output = ok(&mut a, "fill", json!({"ref":form,"values":{"a":"first applied","z":"must-not-write"}}));
        assert!(output.contains("Not filled"), "{path}: {output}");
        if path == "/removed" {
            assert!(!a.tabs[ti].frames[0].page.doc.is_connected(tail));
        } else {
            assert_eq!(a.tabs[ti].frames[0].page.control_text(tail), "keep", "{path}: {output}");
        }
    }
}

#[test]
fn fill_frame_navigation_rejects_old_element_and_form_refs_with_reused_node_ids() {
    let srv = serve(Box::new(|path| match path.split('?').next().unwrap() {
        "/" => html(r#"<html><body><form id="parent"><label>Parent<input id="parent-field" name="p"></label></form><iframe src="/a"></iframe><iframe src="/c"></iframe></body></html>"#),
        "/a" => html(r#"<html><body><form id="child-form" action="/b"><label>Choice<select id="choice" name="pick" onchange="this.form.submit()"><option value="one">One</option><option value="two">Two</option></select></label></form></body></html>"#),
        "/b" => html(r#"<html><body><form id="child-form" action="/wrong-submit"><label>Choice<select id="choice" name="pick"><option value="one">One</option><option value="two">Two</option></select></label></form></body></html>"#),
        "/c" => html(r#"<html><body><form id="neighbor"><label>Neighbor<input id="neighbor-field" name="c"></label></form></body></html>"#),
        _ => html("<html><body>wrong request</body></html>"),
    }));
    let mut a = agent();
    ok(&mut a, "open", json!({"url":srv.url("/"),"snapshot":true}));
    ok(&mut a, "fill", json!({}));
    let ti = a.active.unwrap();
    let child = a.tabs[ti].frames.iter().position(|frame| frame.page.url.path == "/a").unwrap();
    let neighbor = a.tabs[ti].frames.iter().position(|frame| frame.page.url.path == "/c").unwrap();
    let select = a.tabs[ti].frames[child].page.doc.element_by_id("choice").unwrap();
    let form = a.tabs[ti].frames[child].page.doc.element_by_id("child-form").unwrap();
    let parent = a.tabs[ti].frames[0].page.doc.element_by_id("parent-field").unwrap();
    let other = a.tabs[ti].frames[neighbor].page.doc.element_by_id("neighbor-field").unwrap();
    let old = format!("@e{}", a.tabs[ti].refs.lookup(child, select).unwrap());
    let old_form = format!("@f{}", a.tabs[ti].refs.assign_form(child, form));
    let parent_ref = format!("@e{}", a.tabs[ti].refs.lookup(0, parent).unwrap());
    let neighbor_ref = format!("@e{}", a.tabs[ti].refs.lookup(neighbor, other).unwrap());
    let output = ok(&mut a, "fill", json!({"ref":old,"value":"Two"}));
    assert!(output.contains("frame navigated to") && output.contains("/b?pick=two"), "{output}");
    assert_eq!(a.tabs[ti].frames[child].page.doc.element_by_id("choice"), Some(select), "control must exercise a reused NodeId");
    assert_eq!(a.tabs[ti].frames[child].page.doc.element_by_id("child-form"), Some(form), "form must exercise a reused NodeId");
    assert!(err(&mut a, "fill", json!({"ref":old,"value":"One"})).contains("unknown"));
    assert!(err(&mut a, "fill", json!({"ref":old_form,"values":{"pick":"One"},"submit":true})).contains("unknown"));
    assert_eq!(a.resolve(ti, &parent_ref).unwrap(), (0, parent));
    assert_eq!(a.resolve(ti, &neighbor_ref).unwrap(), (neighbor, other));
    assert!(srv.requests().iter().all(|(path, _)| !path.starts_with("/wrong-submit")));
    ok(&mut a, "fill", json!({}));
    let new = format!("@e{}", a.tabs[ti].refs.lookup(child, select).unwrap());
    let new_form = format!("@f{}", a.tabs[ti].refs.assign_form(child, form));
    assert_ne!(new, old);
    assert_ne!(new_form, old_form);
    ok(&mut a, "fill", json!({"ref":new,"value":"One"}));
    assert!(ok(&mut a, "fill", json!({"ref":new_form})).contains("combobox"));
}

#[test]
fn fill_focus_navigation_stops_before_value_input_change_and_remaining_fields() {
    let srv = serve(Box::new(|path| {
        if path == "/" {
            return html(r#"<html><body><form action="/wrong-submit"><label>First<input name="a" value="untouched" onfocus="location.href='/focused?before='+this.value" oninput="location.href='/wrong-input'" onchange="location.href='/wrong-change'"></label><label>Tail<input name="z" value="keep" onfocus="location.href='/wrong-tail'"></label><button type="submit">Send</button></form></body></html>"#);
        }
        if path.starts_with("/focused?") {
            return html("<html><body><h1>Focus arrived</h1></body></html>");
        }
        html("<html><body><h1>Wrong navigation</h1></body></html>")
    }));
    let mut a = agent();
    ok(&mut a, "open", json!({"url":srv.url("/")}));
    let output = ok(&mut a, "fill", json!({"ref":"@f1","values":{"a":"must-not-write","z":"must-not-write"},"submit":true}));
    assert!(output.contains("→ navigated to") && output.contains("before=untouched") && output.contains("Focus arrived"), "{output}");
    assert!(output.contains("a was not changed: navigation was requested while focusing"), "{output}");
    assert!(output.contains("Not attempted after navigation: \"z\"") && output.contains("submit=true was not attempted"), "{output}");
    assert!(srv.requests().iter().all(|(path, _)| !path.starts_with("/wrong-")));
}

#[test]
fn fill_focus_changes_to_readonly_disabled_or_type_prevent_value_writes() {
    let srv = serve(Box::new(|path| {
        let change = match path {
            "/readonly" => "this.setAttribute('readonly','')",
            "/disabled" => "this.setAttribute('disabled','')",
            _ => "this.setAttribute('type','checkbox')",
        };
        let page = format!("<html><body><form><label>Field<input id=\"field\" name=\"q\" value=\"keep\" onfocus=\"{change}\"></label></form></body></html>");
        Some((200, "text/html; charset=utf-8", page.into_bytes(), vec![]))
    }));
    let mut a = agent();
    for (path, reason) in [("/readonly", "read-only"), ("/disabled", "disabled"), ("/type", "changed type")] {
        let snapshot = ok(&mut a, "open", json!({"url":srv.url(path),"snapshot":true}));
        let field = ref_of(&snapshot, "textbox \"Field\"");
        let output = err(&mut a, "fill", json!({"ref":field,"value":"must-not-write"}));
        assert!(output.contains("Not filled") && output.contains(reason), "{path}: {output}");
        assert_eq!(control_value(&a, "field"), "keep", "{path}: {output}");
    }
}

#[test]
fn fill_focus_document_open_keeps_new_controls_untouched() {
    let srv = serve(Box::new(|_| html(r#"<html><body><label>Original<input id="original" value="original-keep" onfocus="document.open();document.write('&lt;html>&lt;body>&lt;label>Replacement&lt;input id=&quot;replacement&quot; value=&quot;replacement-keep&quot;>&lt;/label>&lt;/body>&lt;/html>');document.close()"></label></body></html>"#)));
    let mut a = agent();
    let snapshot = ok(&mut a, "open", json!({"url":srv.url("/"),"snapshot":true}));
    let old_ref = ref_of(&snapshot, "textbox \"Original\"");
    let ti = a.active.unwrap();
    let old_node = a.tabs[ti].frames[0].page.doc.element_by_id("original").unwrap();
    let document = a.tabs[ti].frames[0].page.doc.weak_identity();
    let result = call(&mut a, "fill", &json!({"ref":old_ref,"value":"must-not-write"}));
    let page = &a.tabs[ti].frames[0].page;
    assert!(!document.is_for(&page.doc), "focus must really replace the document incarnation");
    let replacement = page.doc.element_by_id("replacement").unwrap();
    assert_ne!(replacement, old_node, "ordinary document.write appends arena nodes");
    assert!(!page.doc.is_connected(old_node));
    assert_eq!(page.control_text(replacement), "replacement-keep");
    assert!(result.is_err(), "a disconnected old target must not be filled");
}

#[test]
fn fill_focus_document_open_with_reattached_node_stops_values_and_submit() {
    let srv = serve(Box::new(|path| {
        if path == "/" {
            return html(r#"<html><body><form id="original-form" action="/wrong-submit"><label>First<input id="first" name="a" value="original-keep" onfocus="var saved=this.form;document.open();document.write('&lt;html>&lt;body>&lt;h1>Replacement document&lt;/h1>&lt;/body>&lt;/html>');document.close();document.body.appendChild(saved);this.removeAttribute('onfocus');this.value='replacement-keep'" oninput="document.body.setAttribute('data-wrong-input','yes')" onchange="document.body.setAttribute('data-wrong-change','yes')"></label><label>Tail<input id="tail" name="z" value="tail-keep"></label><button type="submit">Send</button></form></body></html>"#);
        }
        html("<html><body><h1>Wrong submission</h1></body></html>")
    }));
    let mut a = agent();
    let snapshot = ok(&mut a, "open", json!({"url":srv.url("/"),"snapshot":true}));
    let old_ref = ref_of(&snapshot, "textbox \"First\"");
    assert!(ok(&mut a, "fill", json!({})).contains("@f1 form"));
    let ti = a.active.unwrap();
    let old_node = a.tabs[ti].frames[0].page.doc.element_by_id("first").unwrap();
    let document = a.tabs[ti].frames[0].page.doc.weak_identity();
    let result = call(&mut a, "fill", &json!({"ref":"@f1","values":{"a":"must-not-write","z":"must-not-write"},"submit":true}));
    let page = &a.tabs[ti].frames[0].page;
    assert!(!document.is_for(&page.doc), "focus must really replace the document incarnation");
    let replacement = page.doc.element_by_id("first").unwrap();
    assert_eq!(replacement, old_node, "the callback reattaches the same arena node in the new incarnation");
    assert!(page.doc.is_connected(replacement));
    assert_eq!(page.control_text(replacement), "replacement-keep", "fill must not overwrite a target from a replaced document");
    assert_eq!(page.control_text(page.doc.element_by_id("tail").unwrap()), "tail-keep");
    let body = page.doc.body().unwrap();
    assert!(page.doc.attr(body, "data-wrong-input").is_none());
    assert!(page.doc.attr(body, "data-wrong-change").is_none());
    assert!(srv.requests().iter().all(|(path, _)| path != "/wrong-submit"));
    let output = match result { Err(error) => error, Ok(out) => panic!("replaced document must stop fill, got {}", out.text) };
    assert!(output.contains("document was replaced") && output.contains("Not attempted after document replacement: \"z\"") && output.contains("submit=true was not attempted"), "{output}");
    assert!(err(&mut a, "fill", json!({"ref":old_ref,"value":"late"})).contains("unknown"));
    assert!(err(&mut a, "fill", json!({"ref":"@f1"})).contains("unknown"));
    let snapshot = ok(&mut a, "snapshot", json!({}));
    let fresh_ref = ref_of(&snapshot, "textbox \"First\"");
    assert_ne!(fresh_ref, old_ref);
    ok(&mut a, "fill", json!({"ref":fresh_ref,"value":"fresh"}));
    assert_eq!(control_value(&a, "first"), "fresh");
}

#[test]
fn fill_input_change_and_timer_document_replacement_stop_pending_fields() {
    for phase in ["input", "change", "timer"] {
        let srv = serve(Box::new(move |path| {
            if path != "/" {
                return html("<html><body><h1>Wrong submission</h1></body></html>");
            }
            let replace = "var saved=document.getElementById('original-form');document.open();document.write('&lt;html>&lt;body>&lt;h1>Replacement document&lt;/h1>&lt;/body>&lt;/html>');document.close();document.body.appendChild(saved);document.getElementById('first').value='replacement-keep'";
            let (input, change) = match phase {
                "input" => (replace.to_string(), "this.value='wrong-change'".to_string()),
                "change" => (String::new(), replace.to_string()),
                _ => (format!("setTimeout(function(){{{replace}}},20)"), String::new()),
            };
            let body = format!(r#"<html><body><form id="original-form" action="/wrong-submit"><label>First<input id="first" name="a" value="original-keep" oninput="{input}" onchange="{change}"></label><label>Tail<input id="tail" name="z" value="tail-keep"></label><button type="submit">Send</button></form></body></html>"#);
            Some((200, "text/html; charset=utf-8", body.into_bytes(), vec![]))
        }));
        let mut a = agent();
        let snapshot = ok(&mut a, "open", json!({"url":srv.url("/"),"snapshot":true}));
        let old_ref = ref_of(&snapshot, "textbox \"First\"");
        assert!(ok(&mut a, "fill", json!({})).contains("@f1 form"));
        let ti = a.active.unwrap();
        let old_node = a.tabs[ti].frames[0].page.doc.element_by_id("first").unwrap();
        let document = a.tabs[ti].frames[0].page.doc.weak_identity();
        let result = call(&mut a, "fill", &json!({"ref":"@f1","values":{"a":"written-before-callback","z":"must-not-write"},"submit":true}));
        let page = &a.tabs[ti].frames[0].page;
        assert!(!document.is_for(&page.doc), "{phase}: replacement callback must run");
        assert_eq!(page.doc.element_by_id("first"), Some(old_node), "{phase}: preserve the arena ID across replacement");
        assert!(page.doc.is_connected(old_node));
        assert_eq!(page.control_text(old_node), "replacement-keep", "{phase}: no later callback may write through the old identity");
        assert_eq!(page.control_text(page.doc.element_by_id("tail").unwrap()), "tail-keep", "{phase}: stop pending fields");
        assert!(srv.requests().iter().all(|(path, _)| path != "/wrong-submit"), "{phase}");
        let output = match result { Ok(out) => out.text, Err(error) => error };
        assert!(output.contains("document was replaced") && output.contains("Not attempted after document replacement: \"z\"") && output.contains("submit=true was not attempted"), "{phase}: {output}");
        assert!(err(&mut a, "fill", json!({"ref":old_ref,"value":"late"})).contains("unknown"), "{phase}");
        assert!(err(&mut a, "fill", json!({"ref":"@f1"})).contains("unknown"), "{phase}");
    }
}
