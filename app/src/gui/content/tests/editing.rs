//! Native form editing through the real content-thread protocol.
use super::*;

fn key(h: &ContentHandle, value: &str, ctrl: bool, clipboard: Option<&str>) {
    h.send(ToContent::Key(KeyInput {
        key: value.into(), text: if value.chars().count() == 1 && !ctrl { Some(value.into()) } else { None },
        mods: Modifiers { ctrl, ..Default::default() }, clipboard: clipboard.map(str::to_string),
        pressed: true, ..Default::default()
    }));
}

fn eval(h: &ContentHandle, rx: &Received, source: &str) -> (String, Vec<String>) {
    h.send(ToContent::Eval(source.into()));
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut clipboard = Vec::new();
    loop {
        let (_, tab, message) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("content thread answers");
        if tab != 1 { continue; }
        match message {
            FromContent::Clipboard(text) => clipboard.push(text),
            FromContent::EvalResult(result) => return (result, clipboard),
            FromContent::Crashed(reason) => panic!("content thread crashed: {reason}"),
            _ => {}
        }
    }
}

fn check(h: &ContentHandle, rx: &Received, expression: &str) {
    let (result, clipboard) = eval(h, rx, expression);
    assert_eq!(result.trim(), "true", "{expression}: {result}");
    assert!(clipboard.is_empty(), "unexpected clipboard write");
}

#[test]
fn beforeinput_cancels_keys_paste_deletion_and_committed_text() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, r#"<!doctype html><textarea id=field>seed</textarea><script>
      var field=document.getElementById('field'),seen=[],inputs=0;
      field.addEventListener('beforeinput',e=>{
        seen.push([e.inputType,e.data,e.cancelable,e.isTrusted,e instanceof InputEvent]);e.preventDefault();
      });
      field.addEventListener('input',()=>inputs++);
      </script>"#);
    wait_for(&rx, 1, Duration::from_secs(20), is_frame).expect("first frame");
    key(&h, "Tab", false, None);
    key(&h, "x", false, None);
    key(&h, "v", true, Some("pasted"));
    h.send(ToContent::Text("漢😀".into()));
    key(&h, "Backspace", false, None);
    key(&h, "Home", false, None);
    key(&h, "Delete", false, None);
    key(&h, "Enter", false, None);
    check(&h, &rx, r#"field.value==='seed'&&inputs===0&&JSON.stringify(seen)===JSON.stringify([
      ['insertText','x',true,true,true],['insertFromPaste','pasted',true,true,true],
      ['insertText','漢😀',true,true,true],['deleteContentBackward',null,true,true,true],
      ['deleteContentForward',null,true,true,true],['insertLineBreak',null,true,true,true]])"#);
}

#[test]
fn input_reports_the_edit_delta_and_ignores_boundary_noops() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, r#"<!doctype html><input id=field value=base><textarea id=area>a</textarea><script>
      var field=document.getElementById('field'),area=document.getElementById('area'),seen=[];
      document.addEventListener('input',e=>seen.push([e.inputType,e.data,e.target.value,e.cancelable,e.isComposing]));
      </script>"#);
    wait_for(&rx, 1, Duration::from_secs(20), is_frame).expect("first frame");
    key(&h, "Tab", false, None);
    key(&h, "é", false, None);
    key(&h, "Backspace", true, None);
    key(&h, "Backspace", false, None);
    key(&h, "Delete", false, None);
    key(&h, "v", true, Some("ab"));
    key(&h, "Home", false, None);
    key(&h, "Delete", false, None);
    key(&h, "a", true, None);
    key(&h, "x", true, None);
    let (_, clipboard) = eval(&h, &rx, "area.focus()");
    assert_eq!(clipboard, vec!["b"]);
    key(&h, "Enter", false, None);
    check(&h, &rx, r#"JSON.stringify(seen)===JSON.stringify([
      ['insertText','é','baseé',false,false],['deleteWordBackward',null,'',false,false],
      ['insertFromPaste','ab','ab',false,false],['deleteContentForward',null,'b',false,false],
      ['deleteByCut',null,'',false,false],['insertLineBreak',null,'a\n',false,false]])"#);
}

#[test]
fn input_handlers_preserve_script_values_and_beforeinput_focus_changes() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, r#"<!doctype html><input id=field value=seed><input id=other value=other><script>
      var field=document.getElementById('field'),other=document.getElementById('other'),inputs=0;
      field.addEventListener('beforeinput',e=>{
        if(e.data==='x')field.value='replacement';
        if(e.data==='!'){field.value='cancelled';e.preventDefault();}
        if(e.data==='q')other.focus();
      });
      field.addEventListener('input',e=>{inputs++;if(e.data==='x')field.value=field.value.toUpperCase();});
      </script>"#);
    wait_for(&rx, 1, Duration::from_secs(20), is_frame).expect("first frame");
    key(&h, "Tab", false, None);
    key(&h, "x", false, None);
    key(&h, "y", false, None);
    check(&h, &rx, "field.value==='REPLACEMENTXy'&&inputs===2");
    key(&h, "!", false, None);
    check(&h, &rx, "field.value==='cancelled'&&inputs===2");
    key(&h, "q", false, None);
    check(&h, &rx, "document.activeElement===other&&other.value==='other'&&field.value==='cancelled'&&inputs===2");
}

#[test]
fn beforeinput_rechecks_readonly_and_password_before_default_mutation() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, r#"<!doctype html><input id=field value=sample><script>
      var field=document.getElementById('field'),inputs=0;
      field.addEventListener('beforeinput',e=>{
        if(e.inputType==='insertText')field.readOnly=true;
        if(e.inputType==='deleteByCut')field.type='password';
      });
      field.addEventListener('input',()=>inputs++);
      </script>"#);
    wait_for(&rx, 1, Duration::from_secs(20), is_frame).expect("first frame");
    key(&h, "Tab", false, None);
    key(&h, "x", false, None);
    check(&h, &rx, "field.value==='sample'&&field.readOnly&&inputs===0");
    eval(&h, &rx, "field.readOnly=false");
    key(&h, "a", true, None);
    key(&h, "x", true, None);
    check(&h, &rx, "field.type==='password'&&field.value==='sample'&&inputs===0");
}

#[test]
fn control_selection_uses_utf16_and_native_keys_follow_script_ranges() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, r#"<!doctype html><input id=field value="ab😀cd"><input id=number type=number value=123><script>
      var field=document.getElementById('field'),number=document.getElementById('number');
      </script>"#);
    wait_for(&rx, 1, Duration::from_secs(20), is_frame).expect("first frame");
    check(&h, &rx, "field.selectionStart===0&&field.selectionEnd===0&&field.selectionDirection==='none'");
    key(&h, "Tab", false, None);
    eval(&h, &rx, "field.setSelectionRange(2,4,'backward')");
    check(&h, &rx, "field.selectionStart===2&&field.selectionEnd===4&&field.selectionDirection==='backward'");
    key(&h, "c", true, None);
    let (_, clipboard) = eval(&h, &rx, "0");
    assert_eq!(clipboard, vec!["😀"]);
    key(&h, "X", false, None);
    check(&h, &rx, "field.value==='abXcd'&&field.selectionStart===3&&field.selectionEnd===3");
    key(&h, "ArrowLeft", false, None);
    check(&h, &rx, "field.selectionStart===2&&field.selectionEnd===2");
    eval(&h, &rx, "field.selectionStart=1;field.selectionEnd=2;field.selectionDirection='backward'");
    key(&h, "Delete", false, None);
    check(&h, &rx, "field.value==='aXcd'&&field.selectionStart===1&&field.selectionEnd===1");
    check(&h, &rx, "number.selectionStart===null&&number.selectionEnd===null&&number.selectionDirection===null");
    check(&h, &rx, "(()=>{try{number.setSelectionRange(0,1);return false;}catch(e){return e.name==='InvalidStateError';}})()");
}

#[test]
fn handler_selection_changes_survive_value_sync_and_next_input() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, r#"<!doctype html><input id=field value=abcd><textarea id=area readonly>sample</textarea><script>
      var field=document.getElementById('field'),area=document.getElementById('area'),seen=[];
      field.addEventListener('beforeinput',e=>{
        seen.push([e.inputType,e.target.selectionStart,e.target.selectionEnd]);
        if(e.data==='X')field.setSelectionRange(1,3);
      });
      field.addEventListener('input',e=>{if(e.data==='X'){
        field.value=field.value.toUpperCase();field.setSelectionRange(0,1,'backward');
      }});
      </script>"#);
    wait_for(&rx, 1, Duration::from_secs(20), is_frame).expect("first frame");
    key(&h, "Tab", false, None);
    key(&h, "X", false, None);
    check(&h, &rx, "field.value==='AXD'&&field.selectionStart===0&&field.selectionEnd===1&&field.selectionDirection==='backward'");
    key(&h, "y", false, None);
    check(&h, &rx, "field.value==='yXD'&&field.selectionStart===1&&JSON.stringify(seen)===JSON.stringify([['insertText',4,4],['insertText',0,1]])");
    // Assigning the same value preserves selection; changing it moves the cursor.
    eval(&h, &rx, "field.setSelectionRange(0,2);field.value=field.value");
    check(&h, &rx, "field.selectionStart===0&&field.selectionEnd===2");
    eval(&h, &rx, "field.value='new'");
    check(&h, &rx, "field.selectionStart===3&&field.selectionEnd===3");
    // Read-only script-selected text remains copyable and cannot be cut.
    eval(&h, &rx, "area.focus();area.select()");
    check(&h, &rx, "area.selectionStart===0&&area.selectionEnd===6");
    key(&h, "c", true, None);
    let (_, clipboard) = eval(&h, &rx, "0");
    assert_eq!(clipboard, vec!["sample"]);
    key(&h, "x", true, None);
    check(&h, &rx, "area.value==='sample'&&area.selectionStart===0&&area.selectionEnd===6");
}
