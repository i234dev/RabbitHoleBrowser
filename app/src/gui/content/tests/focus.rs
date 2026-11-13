//! The actual GUI content-thread keyboard path, with no window system.
use super::*;

fn eval(h: &ContentHandle, rx: &Received, source: &str) -> String {
    h.send(ToContent::Eval(source.into()));
    let (_, result) = wait_for(rx, 1, Duration::from_secs(20), |m| matches!(m, FromContent::EvalResult(_))).expect("keyboard path remains alive");
    let FromContent::EvalResult(result) = result else { unreachable!() };
    result
}

fn key(h: &ContentHandle, value: &str, shift: bool) {
    h.send(ToContent::Key(KeyInput {
        key: value.into(), code: if value == "Tab" { "Tab".into() } else { "".into() },
        text: if value == "Tab" { None } else { Some(value.into()) },
        mods: rabbithole_engine::Modifiers { shift, ..Default::default() },
        pressed: true, ..Default::default()
    }));
}

fn shortcut(h: &ContentHandle, value: &str, clipboard: Option<&str>) {
    h.send(ToContent::Key(KeyInput {
        key: value.into(),
        mods: rabbithole_engine::Modifiers { ctrl: true, ..Default::default() },
        clipboard: clipboard.map(str::to_string), pressed: true, ..Default::default()
    }));
}

/// Eval is queued after the input: retain every clipboard message before
/// its reply, including an unexpected copy that would otherwise be drained.
fn eval_with_clipboard(h: &ContentHandle, rx: &Received, source: &str) -> (String, Vec<String>) {
    h.send(ToContent::Eval(source.into()));
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut clipboard = Vec::new();
    loop {
        let (_, tab, message) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("keyboard path remains alive");
        if tab != 1 { continue; }
        match message {
            FromContent::Clipboard(text) => clipboard.push(text),
            FromContent::EvalResult(result) => return (result, clipboard),
            FromContent::Crashed(reason) => panic!("content thread crashed: {reason}"),
            _ => {}
        }
    }
}

#[test]
fn tab_skips_auxiliary_password_and_types_into_visible_password() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, include_str!("../../../../../tests/pages/signin-controls.html"));
    wait_for(&rx, 1, Duration::from_secs(20), is_frame).expect("first frame");
    key(&h, "Tab", false);
    assert_eq!(eval(&h, &rx, "document.activeElement.id").trim(), "'identity'");
    key(&h, "Tab", false);
    assert_eq!(eval(&h, &rx, "document.activeElement.id").trim(), "'secret'");
    for character in ["a", "é", "😀"] { key(&h, character, false); }
    assert_eq!(eval(&h, &rx, "document.getElementById('secret').value+'|'+document.getElementById('auxiliary').value").trim(), "'aé😀|'");
    if let Ok(path) = std::env::var("RH_FOCUS_SCREENSHOT") {
        h.send(ToContent::SetActive(true));
        let (_, frame) = wait_for(&rx, 1, Duration::from_secs(20), is_frame).expect("paint after typing");
        let FromContent::Frame(frame) = frame else { unreachable!() };
        let pixels = &frame.canvas;
        let rgba: Vec<u8> = pixels.data.iter().flat_map(|&pixel| [(pixel >> 16) as u8, (pixel >> 8) as u8, pixel as u8, 255]).collect();
        image::save_buffer(path, &rgba, pixels.width, pixels.height, image::ColorType::Rgba8).unwrap();
    }
    key(&h, "Tab", false);
    assert_eq!(eval(&h, &rx, "document.activeElement.id+'|'+seen.join(',')").trim(), "'next|identity,secret,next'");
    key(&h, "Tab", true);
    assert_eq!(eval(&h, &rx, "document.activeElement.id").trim(), "'secret'");
}

#[test]
fn tab_focus_handler_can_redirect_the_keyboard_to_another_control() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, r#"<!doctype html><input id=start><button id=redirect>Continue</button><input id=destination tabindex=-1>
      <script>document.getElementById('redirect').addEventListener('focus',()=>document.getElementById('destination').focus());</script>"#);
    wait_for(&rx, 1, Duration::from_secs(20), is_frame).expect("first frame");
    key(&h, "Tab", false);
    key(&h, "Tab", false);
    assert_eq!(eval(&h, &rx, "document.activeElement.id").trim(), "'destination'");
    key(&h, "x", false);
    assert_eq!(eval(&h, &rx, "document.getElementById('destination').value+'|'+document.getElementById('start').value").trim(), "'x|'");
}

#[test]
fn readonly_controls_select_and_copy_without_user_mutation() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, r#"<!doctype html><input id=locked readonly value="café😀"><textarea id=area readonly>sample</textarea>
      <script>var edits=0;document.addEventListener('input',()=>edits++);</script>"#);
    wait_for(&rx, 1, Duration::from_secs(20), is_frame).expect("first frame");
    key(&h, "Tab", false);
    assert_eq!(eval(&h, &rx, "document.activeElement.id").trim(), "'locked'");
    shortcut(&h, "a", None);
    shortcut(&h, "c", None);
    let (_, clipboard) = eval_with_clipboard(&h, &rx, "0");
    assert_eq!(clipboard, vec!["café😀"]);
    for value in ["x", " ", "Backspace", "Delete"] {
        key(&h, value, false);
        let (state, clipboard) = eval_with_clipboard(&h, &rx, "document.getElementById('locked').value+'|'+edits");
        assert_eq!(state.trim(), "'café😀|0'", "readonly {value}");
        assert!(clipboard.is_empty(), "readonly {value} changed clipboard");
    }
    h.send(ToContent::Text("composed".into()));
    shortcut(&h, "v", Some("pasted"));
    shortcut(&h, "x", None);
    let (state, clipboard) = eval_with_clipboard(&h, &rx, "document.getElementById('locked').value+'|'+edits");
    assert_eq!(state.trim(), "'café😀|0'");
    assert!(clipboard.is_empty(), "readonly cut changed clipboard");

    // Script focus must retain a selection buffer for read-only textarea.
    eval(&h, &rx, "document.getElementById('area').focus()");
    shortcut(&h, "a", None);
    shortcut(&h, "c", None);
    let (_, clipboard) = eval_with_clipboard(&h, &rx, "0");
    assert_eq!(clipboard, vec!["sample"]);
    key(&h, "Enter", false);
    h.send(ToContent::Text("composed".into()));
    let (state, clipboard) = eval_with_clipboard(&h, &rx, "document.activeElement.id+'|'+document.getElementById('area').value+'|'+edits");
    assert_eq!(state.trim(), "'area|sample|0'");
    assert!(clipboard.is_empty());
}

#[test]
fn keydown_readonly_changes_block_default_text_mutation() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, r#"<!doctype html><input id=field value=sample><script>
      var field=document.getElementById('field'),edits=0;
      field.addEventListener('input',()=>edits++);
      field.addEventListener('keydown',()=>field.readOnly=true);
      </script>"#);
    wait_for(&rx, 1, Duration::from_secs(20), is_frame).expect("first frame");
    key(&h, "Tab", false);
    key(&h, "x", false);
    assert_eq!(eval(&h, &rx, "field.value+'|'+field.readOnly+'|'+edits").trim(), "'sample|true|0'");
    shortcut(&h, "a", None);
    shortcut(&h, "c", None);
    let (_, clipboard) = eval_with_clipboard(&h, &rx, "0");
    assert_eq!(clipboard, vec!["sample"]);
}

#[test]
fn password_clipboard_uses_type_after_keydown() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    load(&h, r#"<!doctype html><input id=field type=password value=sample><script>
      var field=document.getElementById('field'),edits=0,maskOnKey=false;
      field.addEventListener('input',()=>edits++);
      field.addEventListener('keydown',e=>{if(maskOnKey&&(e.key==='c'||e.key==='x'))field.type='password';});
      </script>"#);
    wait_for(&rx, 1, Duration::from_secs(20), is_frame).expect("first frame");
    key(&h, "Tab", false);
    for command in ["c", "x"] {
        shortcut(&h, "a", None);
        shortcut(&h, command, None);
        let (state, clipboard) = eval_with_clipboard(&h, &rx, "field.value+'|'+edits");
        assert!(clipboard.is_empty(), "masked password Ctrl+{command} leaked clipboard data");
        assert_eq!(state.trim(), "'sample|0'");
    }

    // The buffer was opened with a different type: use the DOM after
    // keydown, rather than trusting a cached ControlEdit kind.
    for command in ["c", "x"] {
        eval(&h, &rx, "field.blur()");
        eval(&h, &rx, "field.type='text';maskOnKey=true;field.focus()");
        shortcut(&h, "a", None);
        shortcut(&h, command, None);
        let (state, clipboard) = eval_with_clipboard(&h, &rx, "field.type+'|'+field.value+'|'+edits");
        assert!(clipboard.is_empty(), "keydown password Ctrl+{command} leaked clipboard data");
        assert_eq!(state.trim(), "'password|sample|0'");
    }

    // Revealed text retains ordinary copy, cut and paste behavior.
    eval(&h, &rx, "maskOnKey=false;field.type='text'");
    shortcut(&h, "a", None);
    shortcut(&h, "c", None);
    shortcut(&h, "x", None);
    let (state, clipboard) = eval_with_clipboard(&h, &rx, "field.value+'|'+edits");
    assert_eq!(clipboard, vec!["sample", "sample"]);
    assert_eq!(state.trim(), "'|1'");
    shortcut(&h, "v", Some("restored"));
    assert_eq!(eval(&h, &rx, "field.value+'|'+edits").trim(), "'restored|2'");
}
