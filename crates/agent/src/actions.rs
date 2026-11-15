//! Low-level input helpers on a page: filling controls, typing with key
//! events, pressing keys with their default actions.

use rabbithole_engine::dom::{Document, NodeId};
use rabbithole_engine::net::Request;
use rabbithole_engine::script::{EventDetail, EventInit};
use rabbithole_engine::scripting::{input_init, key_init, plain_init, ScriptOutcome};
use rabbithole_engine::{ClickAction, ControlKind, Engine, Modifiers, Page};

/// Merge script side effects.
pub fn merge(into: &mut ScriptOutcome, o: ScriptOutcome) {
    into.relayout |= o.relayout;
    into.default_prevented |= o.default_prevented;
    if into.navigation.is_none() {
        into.navigation = o.navigation;
    }
    into.new_windows.extend(o.new_windows);
    into.dialogs.extend(o.dialogs);
    if o.scroll_to.is_some() {
        into.scroll_to = o.scroll_to;
    }
}

/// A value action and all callbacks it ran. A focus callback can navigate or
/// remove/change the control before its value is applied.
#[derive(Debug, Default)]
pub struct FillOutcome {
    pub outcome: ScriptOutcome,
    pub value_applied: bool,
    pub error: Option<String>,
}

#[derive(PartialEq)]
enum FilledValue {
    Checked(bool),
    Selected(Vec<String>),
    Text(String),
}

fn filled_value(page: &Page, control: NodeId, kind: ControlKind) -> FilledValue {
    match kind {
        ControlKind::Checkbox | ControlKind::Radio => FilledValue::Checked(page.doc.is_checked(control)),
        ControlKind::Select => FilledValue::Selected(page.select_options(control).into_iter().filter(|o| o.2).map(|o| o.1).collect()),
        _ => FilledValue::Text(page.control_text(control)),
    }
}

impl FilledValue {
    fn describe(&self, password: bool) -> String {
        match self {
            Self::Checked(value) => value.to_string(),
            Self::Selected(values) => serde_json::to_string(values).unwrap(),
            Self::Text(value) if password => format!("({} chars hidden)", value.chars().count()),
            Self::Text(value) => format!("\"{}\"", crate::ax::esc(&crate::ax::clip_text(value, 80))),
        }
    }
}

fn check_fill_control(page: &Page, control: NodeId) -> Result<(), String> {
    if !page.doc.is_connected(control) {
        return Err("the field no longer exists; take a new snapshot".into());
    }
    if crate::ax::states_of(page, control, "", false).iter().any(|state| state == "disabled") {
        return Err("the field is disabled".into());
    }
    if page.doc.attr(control, "readonly").is_some() && matches!(page.control_kind(control), ControlKind::Text | ControlKind::Password | ControlKind::TextArea | ControlKind::Other) {
        return Err("the field is read-only".into());
    }
    Ok(())
}

/// Set a value and fire input/change, retaining the same script outcomes used
/// by click. Navigation from focus/input stops this sequence immediately.
pub fn fill_control(engine: &Engine, page: &mut Page, control: NodeId, value: &str) -> Result<FillOutcome, String> {
    check_fill_control(page, control)?;
    let document = page.doc.weak_identity();
    let mut result = FillOutcome::default();
    let kind = page.control_kind(control);
    match kind {
        ControlKind::Checkbox | ControlKind::Radio => {
            let want = matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes" | "checked");
            let is = page.doc.is_checked(control);
            if want != is {
                page.toggle_control(control);
            } else {
                result.value_applied = true;
                return Ok(result);
            }
        }
        ControlKind::Select => {
            let options = page.select_options(control);
            let idx = options
                .iter()
                .position(|(label, v, _, _)| v == value || label == value)
                .or_else(|| options.iter().position(|(label, v, _, _)| v.eq_ignore_ascii_case(value) || label.eq_ignore_ascii_case(value)))
                .or_else(|| {
                    let lv = value.to_lowercase();
                    options.iter().position(|(label, _, _, _)| !lv.is_empty() && label.to_lowercase().contains(&lv))
                })
                .ok_or_else(|| {
                    let names: Vec<String> = options.iter().take(20).map(|o| o.0.clone()).collect();
                    format!("no option \"{value}\" (options: {})", names.join(" | "))
                })?;
            if options[idx].3 {
                return Err(format!("option \"{}\" is disabled", options[idx].0));
            }
            page.select_option(control, idx);
        }
        ControlKind::Text | ControlKind::Password | ControlKind::TextArea | ControlKind::Other => {
            merge(&mut result.outcome, engine.focus_element(page, Some(control)));
            if result.outcome.navigation.is_some() {
                return Ok(result);
            }
            if !document.is_for(&page.doc) {
                result.error = Some("the document was replaced while receiving focus; take a new snapshot".into());
                return Ok(result);
            }
            if let Err(error) = check_fill_control(page, control) {
                result.error = Some(error);
                return Ok(result);
            }
            if page.control_kind(control) != kind {
                result.error = Some("the field changed type while receiving focus; take a new snapshot".into());
                return Ok(result);
            }
            page.set_control_text(control, value);
        }
        other => return Err(format!("cannot fill a {other:?} control")),
    }
    result.value_applied = true;
    // Record the value actually applied (an option label may resolve to a
    // different value). Event handlers can synchronously replace it again.
    let expected = filled_value(page, control, kind);
    engine.restyle(page);
    merge(&mut result.outcome, engine.dispatch_event(page, control, "input", input_init(Some(value))));
    if !document.is_for(&page.doc) {
        result.error = Some("the document was replaced by the input event; take a new snapshot".into());
        return Ok(result);
    }
    if result.outcome.navigation.is_some() || !page.doc.is_connected(control) {
        return Ok(result);
    }
    merge(&mut result.outcome, engine.dispatch_event(page, control, "change", plain_init(false)));
    if result.outcome.navigation.is_none() && document.is_for(&page.doc) && page.doc.is_connected(control) {
        let actual = filled_value(page, control, kind);
        if actual != expected {
            result.error = Some(format!("value is still {} after input/change", actual.describe(kind == ControlKind::Password)));
        }
    }
    Ok(result)
}

/// Map a key name ("Enter", "a", "ArrowDown") to (key, code).
pub fn key_code(key: &str) -> (String, String) {
    let k = match key.to_ascii_lowercase().as_str() {
        "enter" | "return" => "Enter",
        "tab" => "Tab",
        "escape" | "esc" => "Escape",
        "backspace" => "Backspace",
        "delete" | "del" => "Delete",
        "space" | " " => " ",
        "arrowdown" | "down" => "ArrowDown",
        "arrowup" | "up" => "ArrowUp",
        "arrowleft" | "left" => "ArrowLeft",
        "arrowright" | "right" => "ArrowRight",
        "pagedown" => "PageDown",
        "pageup" => "PageUp",
        "home" => "Home",
        "end" => "End",
        _ => key,
    }
    .to_string();
    let code = match k.as_str() {
        " " => "Space".to_string(),
        s if s.chars().count() == 1 => {
            let c = s.chars().next().unwrap();
            if c.is_ascii_alphabetic() {
                format!("Key{}", c.to_ascii_uppercase())
            } else if c.is_ascii_digit() {
                format!("Digit{c}")
            } else {
                String::new()
            }
        }
        s => s.to_string(),
    };
    (k, code)
}

fn key_event(engine: &Engine, page: &mut Page, target: NodeId, ty: &str, key: &str, code: &str, out: &mut ScriptOutcome) -> bool {
    let o = engine.dispatch_event(page, target, ty, key_init(key, code, false, false, false, false, false));
    let prevented = o.default_prevented;
    merge(out, o);
    prevented
}

/// Type text into a control like a user: focus, then per character
/// keydown/keypress/input/keyup; `change` at the end.
pub fn type_text(engine: &Engine, page: &mut Page, node: NodeId, text: &str, clear: bool) -> Result<ScriptOutcome, String> {
    let mut out = ScriptOutcome::default();
    let kind = page.control_kind(node);
    let editable = page.doc.attr(node, "contenteditable").map(|v| v != "false").unwrap_or(false);
    if !matches!(kind, ControlKind::Text | ControlKind::Password | ControlKind::TextArea | ControlKind::Other) && !editable {
        return Err(format!("cannot type into a {kind:?} control"));
    }
    if page.doc.attr(node, "readonly").is_some() {
        return Err("the field is read-only".into());
    }
    merge(&mut out, engine.focus_element(page, Some(node)));
    let mut value = if clear { String::new() } else if editable { page.doc.text_content(node) } else { page.control_text(node) };
    let max = page.max_length(node);
    for ch in text.chars() {
        let s = ch.to_string();
        let (key, code) = if ch == '\n' { ("Enter".to_string(), "Enter".to_string()) } else { key_code(&s) };
        if key_event(engine, page, node, "keydown", &key, &code, &mut out) {
            key_event(engine, page, node, "keyup", &key, &code, &mut out);
            continue;
        }
        key_event(engine, page, node, "keypress", &key, &code, &mut out);
        if ch == '\n' && kind != ControlKind::TextArea {
            key_event(engine, page, node, "keyup", &key, &code, &mut out);
            continue;
        }
        if max.map(|m| value.chars().count() >= m).unwrap_or(false) {
            key_event(engine, page, node, "keyup", &key, &code, &mut out);
            continue;
        }
        value.push(ch);
        if editable {
            set_editable_text(page, node, &value);
        } else {
            page.set_control_text(node, &value);
        }
        let o = engine.dispatch_event(page, node, "input", input_init(Some(&s)));
        merge(&mut out, o);
        key_event(engine, page, node, "keyup", &key, &code, &mut out);
        // A handler may have rewritten the value (masks, formatting).
        if !editable {
            value = page.control_text(node);
        }
    }
    if text.is_empty() && clear {
        page.set_control_text(node, "");
        let o = engine.dispatch_event(page, node, "input", input_init(None));
        merge(&mut out, o);
    }
    engine.restyle(page);
    out.relayout = true;
    let o = engine.dispatch_event(page, node, "change", plain_init(false));
    merge(&mut out, o);
    Ok(out)
}

fn set_editable_text(page: &mut Page, node: NodeId, text: &str) {
    page.doc.clear_children(node);
    let t = page.doc.create_text(text);
    page.doc.append_child(node, t);
}

/// What pressing a key did.
#[derive(Debug, Default)]
pub struct KeyResult {
    pub outcome: ScriptOutcome,
    /// A navigation request (implicit submission, activated link).
    pub request: Option<Request>,
    /// A description of the default action.
    pub action: Option<String>,
}

fn focusable(page: &Page, n: NodeId) -> bool {
    rabbithole_engine::pointer::is_focusable(&page.doc, n)
        && crate::ax::displayed(page, n)
        && page.doc.attr(n, "tabindex").and_then(|t| t.trim().parse::<i32>().ok()).map(|t| t >= 0).unwrap_or(true)
}

/// Press a key on `target` (the focused element when None).
pub fn press_key(engine: &Engine, page: &mut Page, target: Option<NodeId>, key: &str) -> KeyResult {
    let mut r = KeyResult::default();
    let target = target.or(page.state.focused).or_else(|| page.doc.body()).unwrap_or(Document::ROOT);
    let (key, code) = key_code(key);
    let prevented = key_event(engine, page, target, "keydown", &key, &code, &mut r.outcome);
    if !prevented && key.chars().count() == 1 {
        key_event(engine, page, target, "keypress", &key, &code, &mut r.outcome);
    }
    if !prevented {
        let kind = page.control_kind(target);
        let tag = page.doc.tag(target).unwrap_or("").to_string();
        match key.as_str() {
            "Enter" => {
                if matches!(kind, ControlKind::Text | ControlKind::Password | ControlKind::Other) && tag == "input" {
                    if let Some(form) = page.form_of(target) {
                        r.action = Some("implicit form submission".into());
                        match page.default_submitter(form) {
                            Some(sub) => {
                                let (action, o) = engine.click_element(page, sub, None, Modifiers::default());
                                merge(&mut r.outcome, o);
                                if let ClickAction::Submit { form, submitter } = action {
                                    r.request = submit_request(engine, page, form, submitter, &mut r.outcome);
                                }
                            }
                            None => {
                                r.request = submit_request(engine, page, form, None, &mut r.outcome);
                            }
                        }
                    }
                } else if tag == "a" || tag == "button" || page.doc.attr(target, "role") == Some("button") {
                    let (action, o) = engine.click_element(page, target, None, Modifiers::default());
                    merge(&mut r.outcome, o);
                    r.action = Some("activated".into());
                    r.request = click_request(engine, page, action, &mut r.outcome);
                }
            }
            " " if tag == "button" || matches!(kind, ControlKind::Checkbox | ControlKind::Radio) => {
                let (action, o) = engine.click_element(page, target, None, Modifiers::default());
                merge(&mut r.outcome, o);
                r.action = Some("activated".into());
                r.request = click_request(engine, page, action, &mut r.outcome);
            }
            "Tab" => {
                let all: Vec<NodeId> = page.doc.descendants(Document::ROOT).filter(|&n| focusable(page, n)).collect();
                let next = match all.iter().position(|&n| n == target) {
                    Some(i) => all.get(i + 1).copied(),
                    None => all.first().copied(),
                };
                merge(&mut r.outcome, engine.focus_element(page, next));
                r.action = Some("focus moved".into());
            }
            "Backspace" if matches!(kind, ControlKind::Text | ControlKind::Password | ControlKind::TextArea) => {
                let mut v = page.control_text(target);
                v.pop();
                page.set_control_text(target, &v);
                let o = engine.dispatch_event(page, target, "input", input_init(None));
                merge(&mut r.outcome, o);
            }
            "ArrowDown" | "ArrowUp" if kind == ControlKind::Select => {
                let opts = page.select_options(target);
                let cur = opts.iter().position(|o| o.2).unwrap_or(0);
                let next = if key == "ArrowDown" { (cur + 1).min(opts.len().saturating_sub(1)) } else { cur.saturating_sub(1) };
                if next != cur {
                    page.select_option(target, next);
                    engine.restyle(page);
                    let o = engine.dispatch_event(page, target, "change", plain_init(false));
                    merge(&mut r.outcome, o);
                }
            }
            _ => {}
        }
    }
    key_event(engine, page, target, "keyup", &key, &code, &mut r.outcome);
    r
}

/// Fire `submit` (cancelable) and build the submission request.
pub fn submit_request(engine: &Engine, page: &mut Page, form: NodeId, submitter: Option<NodeId>, out: &mut ScriptOutcome) -> Option<Request> {
    let document = page.doc.weak_identity();
    let o = engine.dispatch_event(page, form, "submit", plain_init(true));
    let prevented = o.default_prevented;
    let nav = o.navigation.clone();
    merge(out, ScriptOutcome { navigation: None, ..o });
    if let Some(n) = nav {
        return Some(n.request);
    }
    // Preserve explicit script navigation, but never build the old form's
    // default request from recycled IDs in a replacement document.
    if prevented || !document.is_for(&page.doc) {
        return None;
    }
    page.submit_request(form, submitter, &|_| Vec::new())
}

/// The request a click's default action asks for.
pub fn click_request(engine: &Engine, page: &mut Page, action: ClickAction, out: &mut ScriptOutcome) -> Option<Request> {
    if let Some(n) = out.navigation.take() {
        return Some(n.request);
    }
    match action {
        ClickAction::Navigate { url, new_tab } => {
            if new_tab {
                out.new_windows.push(url);
                None
            } else {
                Some(Request::get(url).with_referer(page.url.clone()))
            }
        }
        ClickAction::Submit { form, submitter } => submit_request(engine, page, form, submitter, out),
        _ => None,
    }
}

/// Two real pointer cycles, keeping the first click's side effects if the
/// document or hit target changes before the second click can be delivered.
#[derive(Default)]
pub struct DoubleClickOutcome {
    pub outcome: ScriptOutcome,
    pub request: Option<Request>,
    pub completed: bool,
    pub clicks: u8,
    pub stopped: Option<&'static str>,
}

pub fn double_click_at(engine: &Engine, page: &mut Page, requested: NodeId, x: f32, y: f32) -> DoubleClickOutcome {
    let mut result = DoubleClickOutcome::default();
    let document = page.doc.weak_identity();
    let mods = Modifiers::default();
    merge(&mut result.outcome, engine.pointer_move(page, x, y, mods));
    let interrupted = |page: &Page, result: &DoubleClickOutcome| {
        if !document.is_for(&page.doc) { Some("the document was replaced") }
        else if result.request.is_some() || result.outcome.navigation.is_some() { Some("navigation was requested") }
        else if !result.outcome.new_windows.is_empty() { Some("a new window was opened") }
        else if !result.outcome.dialogs.is_empty() { Some("a dialog was opened") }
        else if !page.doc.is_connected(requested) { Some("the target was removed") }
        else { None }
    };
    let mut clicked = None;
    for count in [1, 2] {
        if let Some(reason) = interrupted(page, &result) { result.stopped = Some(reason); break; }
        let hit = page.node_at_viewport(x, y);
        let valid_hit = match clicked {
            Some(first) => hit == Some(first) && page.doc.is_connected(first),
            None => hit.is_some_and(|n| n == requested || page.doc.is_ancestor(requested, n)),
        };
        if !valid_hit { result.stopped = Some("the pointer point no longer hits the same target"); break; }
        if crate::ax::states_of(page, hit.unwrap(), "", false).iter().any(|s| s == "disabled") {
            result.stopped = Some("the target is disabled"); break;
        }
        let down = engine.pointer_down_counted(page, x, y, 0, mods, count);
        merge(&mut result.outcome, down.outcome);
        if let Some(reason) = interrupted(page, &result) { result.stopped = Some(reason); break; }
        if down.target != hit || page.node_at_viewport(x, y) != hit {
            result.stopped = Some("the target changed during the press"); break;
        }
        if crate::ax::states_of(page, hit.unwrap(), "", false).iter().any(|s| s == "disabled") {
            result.stopped = Some("the target became disabled during the press"); break;
        }
        let up = engine.pointer_up_counted(page, x, y, 0, mods, count);
        if up.clicked == hit { result.clicks += 1; }
        merge(&mut result.outcome, up.outcome);
        result.request = click_request(engine, page, up.action, &mut result.outcome);
        if let Some(reason) = interrupted(page, &result) { result.stopped = Some(reason); break; }
        if up.clicked != hit || !page.doc.is_connected(hit.unwrap()) {
            result.stopped = Some("the click was not delivered to the same target"); break;
        }
        clicked = up.clicked;
        if count == 2 {
            if crate::ax::states_of(page, clicked.unwrap(), "", false).iter().any(|s| s == "disabled") {
                result.stopped = Some("the target became disabled after click"); break;
            }
            let init = EventInit { bubbles: true, cancelable: true, detail: Some(EventDetail::MouseClick {
                x, y, button: 0, ctrl: false, shift: false, alt: false, meta: false, count: 2,
            }) };
            merge(&mut result.outcome, engine.dispatch_event(page, clicked.unwrap(), "dblclick", init));
            result.request = click_request(engine, page, ClickAction::None, &mut result.outcome);
            result.completed = true;
        }
    }
    if !result.completed {
        // An interrupted sequence must not leave a press/active state behind.
        page.pointer.down = None;
        page.pointer.frame_press = None;
        page.pointer.suppress_mouse = false;
        if page.state.active.take().is_some() && page.has_dynamic_rules {
            engine.restyle(page);
            result.outcome.relayout = true;
        }
    }
    result
}

/// Plain focus/blur events helper.
pub fn simple(bubbles: bool) -> EventInit {
    EventInit::simple(bubbles, false)
}
