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
