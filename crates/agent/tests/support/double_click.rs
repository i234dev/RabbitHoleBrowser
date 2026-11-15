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
    let result = ok(&mut a, "click", json!({"selector":"#go","double":true}));
    assert!(result.starts_with("Clicked @e") && result.contains("double-click stopped: navigation was requested") && result.contains("navigated to"), "{result}");
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
        let result = ok(&mut a, "click", json!({"selector":"#t","double":true,"dialog":"accept"}));
        assert!(result.starts_with("Clicked @e") && result.contains("double-click stopped:"), "{result}");
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
        let result = ok(&mut a, "click", json!({"selector":"#t","double":true}));
        assert!(result.contains("double-click stopped: the document was replaced"), "{phase}: {result}");
        assert_eq!(text(&a, "out"), "Fresh", "{phase}");
        assert!(a.tabs[a.active.unwrap()].frames[0].pointer.down.is_none());
    }
}
