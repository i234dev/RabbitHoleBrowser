//! Mouse and keyboard text selection through the content-thread protocol:
//! drags, double / triple clicks, shift+click, Ctrl+A / Ctrl+C,
//! `user-select`, the events scripts see and the painted highlight.
use super::*;

const PAGE: &str = r#"<!doctype html><style>body{margin:0;font:20px sans-serif}p{margin:0;height:40px}</style>
<p id=a>Hello brave world</p><p id=b>Second <b>bold</b> line</p><p id=n style="user-select:none">Not selectable</p>
<script>var starts=0,changes=0;document.addEventListener('selectstart',()=>starts++);document.addEventListener('selectionchange',()=>changes++);</script>"#;

fn mods(shift: bool, ctrl: bool) -> Modifiers {
    Modifiers { shift, ctrl, ..Default::default() }
}

fn press(h: &ContentHandle, x: f32, y: f32, m: Modifiers, clicks: u8) {
    h.send(ToContent::PointerDown { x, y, button: 0, mods: m, click_count: clicks });
}

fn release(h: &ContentHandle, x: f32, y: f32, m: Modifiers) {
    h.send(ToContent::PointerUp { x, y, button: 0, mods: m, inside: true });
}

fn key(h: &ContentHandle, value: &str, ctrl: bool) {
    h.send(ToContent::Key(KeyInput { key: value.into(), mods: mods(false, ctrl), pressed: true, ..Default::default() }));
}

/// Evaluate and collect the clipboard writes sent before the result.
fn eval(h: &ContentHandle, rx: &Received, source: &str) -> (String, Vec<String>) {
    h.send(ToContent::Eval(source.into()));
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut clipboard = Vec::new();
    loop {
        let (_, tab, message) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("content thread answers");
        if tab != 1 {
            continue;
        }
        match message {
            FromContent::Clipboard(text) => clipboard.push(text),
            FromContent::EvalResult(result) => return (result.trim().trim_matches('"').trim_matches('\'').to_string(), clipboard),
            FromContent::Crashed(reason) => panic!("content thread crashed: {reason}"),
            _ => {}
        }
    }
}

fn selected(h: &ContentHandle, rx: &Received) -> String {
    eval(h, rx, "getSelection().toString().replace(/\\n/g, '|')").0
}

fn ready(h: &ContentHandle, rx: &Received) {
    load(h, PAGE);
    wait_for(rx, 1, Duration::from_secs(20), is_frame).expect("first frame");
}

#[test]
fn dragging_selects_characters_and_fires_events() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    ready(&h, &rx);
    press(&h, 1.0, 20.0, mods(false, false), 1);
    h.send(ToContent::PointerMove { x: 30.0, y: 20.0, mods: mods(false, false) });
    h.send(ToContent::PointerMove { x: 120.0, y: 60.0, mods: mods(false, false) });
    release(&h, 120.0, 60.0, mods(false, false));
    let text = selected(&h, &rx);
    assert!(text.starts_with("Hello brave world") && text.len() > "Hello brave world".len(), "{text:?}");
    assert_eq!(eval(&h, &rx, "starts").0, "1");
    // selectionchange is coalesced per task by scripts, but every UA change
    // is reported.
    assert!(eval(&h, &rx, "changes").0.parse::<u32>().unwrap() >= 2);
    // Dragging backwards keeps the anchor.
    press(&h, 120.0, 20.0, mods(false, false), 1);
    h.send(ToContent::PointerMove { x: 1.0, y: 20.0, mods: mods(false, false) });
    release(&h, 1.0, 20.0, mods(false, false));
    assert_eq!(eval(&h, &rx, "getSelection().direction").0, "backward");
    // A click collapses the selection.
    press(&h, 50.0, 20.0, mods(false, false), 1);
    release(&h, 50.0, 20.0, mods(false, false));
    assert_eq!(eval(&h, &rx, "getSelection().type").0, "Caret");
}

#[test]
fn clicks_pick_words_paragraphs_and_shift_extends() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    ready(&h, &rx);
    press(&h, 70.0, 20.0, mods(false, false), 2);
    release(&h, 70.0, 20.0, mods(false, false));
    assert_eq!(selected(&h, &rx), "brave");
    press(&h, 70.0, 60.0, mods(false, false), 3);
    release(&h, 70.0, 60.0, mods(false, false));
    assert_eq!(selected(&h, &rx), "Second bold line");
    // Shift+click extends from the anchor.
    press(&h, 1.0, 20.0, mods(false, false), 1);
    release(&h, 1.0, 20.0, mods(false, false));
    press(&h, 399.0, 60.0, mods(true, false), 1);
    release(&h, 399.0, 60.0, mods(true, false));
    let text = selected(&h, &rx);
    assert_eq!(text, "Hello brave world|Second bold line");
}

#[test]
fn user_select_none_blocks_and_select_all_copies() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    ready(&h, &rx);
    // No selection starts on user-select: none text.
    press(&h, 10.0, 100.0, mods(false, false), 1);
    h.send(ToContent::PointerMove { x: 120.0, y: 100.0, mods: mods(false, false) });
    release(&h, 120.0, 100.0, mods(false, false));
    assert_eq!(eval(&h, &rx, "getSelection().isCollapsed + ',' + starts").0, "true,0");
    key(&h, "a", true);
    key(&h, "c", true);
    let (r, clipboard) = eval(&h, &rx, "getSelection().anchorNode === document.body");
    assert_eq!(r, "true");
    assert_eq!(clipboard, vec!["Hello brave world\nSecond bold line".to_string()]);
}

#[test]
fn selection_highlight_is_painted_in_frames() {
    let (sink, rx) = sink();
    let h = spawn(1, &sink);
    ready(&h, &rx);
    let highlight = |f: &Frame| {
        let mut n = 0;
        for y in 0..40u32 {
            for x in 0..400u32 {
                let p = f.canvas.data[(y * f.canvas.width + x) as usize];
                if ((p >> 16) & 0xff, (p >> 8) & 0xff, p & 0xff) == (181, 213, 255) {
                    n += 1;
                }
            }
        }
        n
    };
    press(&h, 70.0, 20.0, mods(false, false), 2);
    release(&h, 70.0, 20.0, mods(false, false));
    let (_, f) = wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::Frame(f) if highlight(f) > 100)).expect("a frame with the highlight");
    let FromContent::Frame(f) = f else { unreachable!() };
    assert!(highlight(&f) > 100);
    // Script selections are painted too, and removing them clears it.
    eval(&h, &rx, "getSelection().removeAllRanges()");
    wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::Frame(f) if highlight(f) == 0)).expect("a frame without the highlight");
    eval(&h, &rx, "getSelection().selectAllChildren(document.getElementById('a'))");
    wait_for(&rx, 1, Duration::from_secs(10), |m| matches!(m, FromContent::Frame(f) if highlight(f) > 500)).expect("script selection painted");
}
