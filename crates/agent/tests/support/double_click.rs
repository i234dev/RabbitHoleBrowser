//! Real MCP actions using the parent integration suite's loopback server.
use super::*;

const OWNER: &str = include_str!("../fixtures/double-click-owner.html");
const SEQUENCE: &str = "mousedown:1 mouseup:1 click:1 mousedown:2 mouseup:2 click:2 dblclick:2";

fn text(a: &Agent, id: &str) -> String {
    let page = &a.tabs[a.active.unwrap()].frames[0];
    page.doc.text_content(page.doc.element_by_id(id).unwrap())
}

#[test]
fn double_click_owner_repro_has_the_real_seven_mouse_events() {
    let srv = serve(Box::new(|_| html(OWNER)));
    let mut a = agent();
    ok(&mut a, "open", json!({"url":srv.url("/")}));
    let result = ok(&mut a, "click", json!({"selector":"#t","double":true}));
    assert!(result.starts_with("Double-clicked @e"), "{result}");
    assert_eq!(text(&a, "out"), SEQUENCE);
}

#[test]
fn two_normal_clicks_stay_single_and_hover_combination_has_no_click_side_effect() {
    let srv = serve(Box::new(|_| html(OWNER)));
    let mut a = agent();
    ok(&mut a, "open", json!({"url":srv.url("/")}));
    let error = err(&mut a, "click", json!({"selector":"#t","double":true,"hover":true}));
    assert!(error.contains("double and hover"), "{error}");
    assert_eq!(text(&a, "out"), "sin eventos");
    let hovered = ok(&mut a, "click", json!({"selector":"#t","hover":true}));
    assert!(hovered.starts_with("Hovered @e"), "{hovered}");
    for _ in 0..2 {
        let result = ok(&mut a, "click", json!({"selector":"#t"}));
        assert!(result.starts_with("Clicked @e"), "{result}");
    }
    assert_eq!(text(&a, "out"), "mousedown:1 mouseup:1 click:1 mousedown:1 mouseup:1 click:1");
}

#[test]
fn double_click_preserves_mouse_coordinates_trust_and_default_checkbox_activation() {
    let srv = serve(Box::new(|_| html(r#"<!doctype html><style>input{width:24px;height:24px;margin:20px}</style>
        <input id=normal type=checkbox><input id=cancel type=checkbox><p id=out>Before</p>
        <script>
        var records={normal:[],cancel:[]}, coordinates=[], trusted=true, mouse=true;
        ['normal','cancel'].forEach(function(id){
          var box=document.getElementById(id);
          ['mousedown','mouseup','click','dblclick','input','change'].forEach(function(type){
            box.addEventListener(type,function(e){
              if(type==='click'||type==='dblclick') records[id].push(type+':'+e.detail+':'+box.checked);
              else records[id].push(type);
              if(type==='mousedown'||type==='mouseup'||type==='click'||type==='dblclick'){
                coordinates.push(e.clientX+','+e.clientY);trusted=trusted&&e.isTrusted;mouse=mouse&&(e instanceof MouseEvent);
              }
              if(id==='cancel'&&type==='click')e.preventDefault();
            });
          });
        });
        </script>"#)));
    let mut a = agent();
    ok(&mut a, "open", json!({"url":srv.url("/")}));
    ok(&mut a, "click", json!({"selector":"#normal","double":true}));
    let observed = ok(&mut a, "debug", json!({"eval":"records.normal.join('|')+';'+trusted+';'+mouse+';'+(coordinates.length===7&&coordinates.every(function(v){return v===coordinates[0]}))+';'+document.getElementById('normal').checked"}));
    assert!(observed.contains("mousedown|mouseup|click:1:true|input|change|mousedown|mouseup|click:2:false|input|change|dblclick:2:false;true;true;true;false"), "{observed}");
    ok(&mut a, "click", json!({"selector":"#cancel","double":true}));
    let canceled = ok(&mut a, "debug", json!({"eval":"records.cancel.join('|')+';'+document.getElementById('cancel').checked"}));
    assert!(canceled.contains("mousedown|mouseup|click:1:true|mousedown|mouseup|click:2:true|dblclick:2:false;false"), "{canceled}");
}

#[test]
fn double_click_enters_editor_and_real_typing_with_enter_renames_task() {
    let srv = serve(Box::new(|_| html(r#"<!doctype html><style>
        #edit{display:none}.editing #edit{display:block}.editing #task{display:none}
        </style><form id=form onsubmit="event.preventDefault();document.getElementById('task').textContent=document.getElementById('edit').value;this.className=''">
        <label id=task ondblclick="document.getElementById('form').className='editing';document.getElementById('edit').focus()">Original task</label>
        <input id=edit aria-label='Edit task' value='Original task'></form>"#)));
    let mut a = agent();
    ok(&mut a, "open", json!({"url":srv.url("/")}));
    let clicked = ok(&mut a, "click", json!({"selector":"#task","double":true}));
    assert!(clicked.starts_with("Double-clicked @e"), "{clicked}");
    let snapshot = ok(&mut a, "snapshot", json!({}));
    assert!(snapshot.contains("textbox \"Edit task\"") && snapshot.contains("Original task"), "{snapshot}");
    ok(&mut a, "type", json!({"selector":"#edit","text":"Renamed task","clear":true,"submit":true}));
    assert_eq!(text(&a, "task"), "Renamed task");
}

#[test]
fn first_click_navigation_is_followed_once_without_clicking_the_new_document() {
    let srv = serve(Box::new(|path| match path {
        "/" => html("<a id=go href='/arrived'>Go</a>"),
        "/arrived" => html("<title>Arrived</title><button onclick=\"document.getElementById('out').textContent='WRONG'\">Replacement</button><p id=out>Untouched</p>"),
        _ => None,
    }));
    let mut a = agent();
    ok(&mut a, "open", json!({"url":srv.url("/")}));
    let result = err(&mut a, "click", json!({"selector":"#go","double":true}));
    assert!(result.starts_with("Double-click incomplete: only one click completed.") && result.contains("Double-click stopped @e") && result.contains("double-click stopped: navigation was requested") && result.contains("navigated to"), "{result}");
    assert_eq!(text(&a, "out"), "Untouched");
    assert_eq!(srv.requests().iter().filter(|(path,_)|path=="/arrived").count(), 1);
}

#[test]
fn first_click_dialog_and_target_replacement_stop_the_second_cycle() {
    for replace in [false,true] {
        let body = if replace {
            r#"<div id=host><button id=t onclick="document.getElementById('out').textContent='first';document.getElementById('host').innerHTML='&lt;button onclick=&quot;document.getElementById(\'out\').textContent=\'WRONG\'&quot;&gt;Replacement&lt;/button&gt;'">Replace</button></div><p id=out>Before</p>"#
        } else {
            r#"<button id=t onclick="document.getElementById('out').textContent+='x';alert('first')" ondblclick="document.getElementById('out').textContent='WRONG'">Dialog</button><p id=out>Before</p>"#
        };
        let srv = serve(Box::new(move |_| html(body)));
        let mut a = agent();
        ok(&mut a, "open", json!({"url":srv.url("/")}));
        let result = err(&mut a, "click", json!({"selector":"#t","double":true,"dialog":"accept"}));
        assert!(result.starts_with("Double-click incomplete: only one click completed.") && result.contains("Double-click stopped @e") && result.contains("double-click stopped:"), "{result}");
        assert_eq!(text(&a, "out"), if replace {"first"} else {"Beforex"});
        assert!(a.tabs[a.active.unwrap()].frames[0].pointer.down.is_none());
    }
}

#[test]
fn document_replacement_at_each_pointer_phase_never_dispatches_into_recycled_nodes() {
    for phase in ["pointerdown","mousedown","pointerup","mouseup","click"] {
        let body = format!(r#"<!doctype html><button id=t on{phase}="document.open();document.write('&lt;button onclick=&quot;document.getElementById(\'out\').textContent=\'WRONG\'&quot;&gt;New&lt;/button&gt;&lt;p id=out&gt;Fresh&lt;/p&gt;');document.close()">Replace document</button>"#);
        let body: &'static str = Box::leak(body.into_boxed_str());
        let srv = serve(Box::new(move |_| html(body)));
        let mut a = agent();
        ok(&mut a, "open", json!({"url":srv.url("/")}));
        let result = err(&mut a, "click", json!({"selector":"#t","double":true}));
        assert!(result.contains("double-click stopped: the document was replaced"), "{phase}: {result}");
        assert!(result.contains(if phase == "click" { "only one click completed" } else { "no clicks completed" }), "{phase}: {result}");
        assert_eq!(text(&a, "out"), "Fresh", "{phase}");
        assert!(a.tabs[a.active.unwrap()].frames[0].pointer.down.is_none());
    }
}

#[test]
fn interrupted_before_first_click_reports_zero_and_stops_the_batch() {
    let srv = serve(Box::new(|_| html("<button id=t onmouseover='this.remove()'>Removed on hover</button><button id=after onclick=\"document.getElementById('out').textContent='WRONG'\">After</button><p id=out>Before</p>")));
    let mut a = agent();
    ok(&mut a, "open", json!({"url":srv.url("/")}));
    let result = err(&mut a, "batch", json!({"steps":[
        {"tool":"click","args":{"selector":"#t","double":true}},
        {"tool":"click","args":{"selector":"#after"}}
    ]}));
    assert!(result.contains("attempted 1/2, succeeded 0, failed 1, pending 1"), "{result}");
    assert!(result.contains("Double-click incomplete: no clicks completed.") && result.contains("Double-click stopped @e"), "{result}");
    assert!(!result.contains("Double-clicked") && !result.contains("only one click completed"), "{result}");
    assert_eq!(text(&a, "out"), "Before");
}

#[test]
fn interruption_after_second_click_does_not_claim_that_dblclick_was_delivered() {
    let srv = serve(Box::new(|_| html("<button id=t onclick=\"if(event.detail===2){document.getElementById('out').textContent='second';this.remove()}\">Remove on second click</button><p id=out>Before</p>")));
    let mut a = agent();
    ok(&mut a, "open", json!({"url":srv.url("/")}));
    let result = err(&mut a, "click", json!({"selector":"#t","double":true}));
    assert!(result.starts_with("Double-click incomplete: two clicks completed, but dblclick was not dispatched."), "{result}");
    assert!(!result.contains("Double-clicked"), "{result}");
    assert_eq!(text(&a, "out"), "second");
}

#[test]
fn default_action_callbacks_cannot_apply_old_ids_to_a_replacement_document() {
    for kind in ["input", "submit", "reset"] {
        let fresh = if kind == "input" {
            "<!doctype html><form id=form action=/wrong><input id=t type=checkbox onchange=\"document.getElementById('out').textContent='WRONG'\"><button type=button>Other</button><p id=out>Fresh</p></form>".to_string()
        } else {
            format!("<!doctype html><form id=form action=/wrong><input id=field value=baseline><button id=t type={kind}>Action</button><p id=out>Fresh</p></form><script>document.getElementById('field').value='kept'</script>")
        };
        let handler = format!("document.open();document.write({});document.close()", serde_json::to_string(&fresh).unwrap())
            .replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
        let body = if kind == "input" {
            format!("<!doctype html><form id=form action=/wrong><input id=t type=checkbox oninput=\"{handler}\"><button type=button>Other</button><p id=out>Before</p></form>")
        } else {
            format!("<!doctype html><form id=form action=/wrong on{kind}=\"{handler}\"><input id=field value=baseline><button id=t type={kind}>Action</button><p id=out>Before</p></form>")
        };
        let body: &'static str = Box::leak(body.into_boxed_str());
        let srv = serve(Box::new(move |path| if path == "/wrong" { html("<p id=out>WRONG navigation</p>") } else { html(body) }));
        let mut a = agent();
        ok(&mut a, "open", json!({"url":srv.url("/")}));
        let old = a.tabs[a.active.unwrap()].frames[0].doc.element_by_id("t").unwrap();
        let result = err(&mut a, "click", json!({"selector":"#t","double":true}));
        assert!(result.contains("only one click completed") && result.contains("the document was replaced"), "{kind}: {result}");
        let page = &a.tabs[a.active.unwrap()].frames[0];
        assert_eq!(page.doc.element_by_id("t"), Some(old), "fixture must actually recycle the target ID: {kind}");
        assert_eq!(text(&a, "out"), "Fresh", "{kind}: the old input/change callback must not reach new nodes");
        if kind == "reset" {
            assert_eq!(page.control_text(page.doc.element_by_id("field").unwrap()), "kept", "the old reset must not reset a new form");
        }
        assert!(!srv.requests().iter().any(|(path,_)| path=="/wrong"), "the old submit must not submit a new form");
    }
}
