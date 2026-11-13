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
