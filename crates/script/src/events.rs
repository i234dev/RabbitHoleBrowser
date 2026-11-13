//! `EventTarget`, `Event` and its subclasses, listener registration and
//! the dispatch algorithm (capture → target → bubble).

use crate::env::with_env;
use rabbithole_js::V;
use crate::host::*;
use crate::{DefaultAction, DispatchResult, EventDetail, EventInit};
use rabbithole_dom::{Document, NodeId};
use rabbithole_js::{JsResult, ObjectId, PropertyKey, Realm, Value, ALL, CONFIGURABLE, ENUMERABLE, HIDDEN};
use std::rc::Rc;

const ACC: u8 = CONFIGURABLE | ENUMERABLE;

thread_local! {
    /// DOM "legacy-output-did-listeners-throw flag" of the last dispatches
    /// (read and reset by `__rhNative.listenersThrew()`; used by IndexedDB).
    static LISTENER_THREW: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `__rhNative.listenersThrew()`: whether a listener threw since the last
/// call (and reset the flag).
pub(crate) fn listeners_threw(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(LISTENER_THREW.with(|c| c.replace(false))))
}

/// Internal state of an Event object (host data).
pub struct EventData {
    pub ty: String,
    pub bubbles: bool,
    pub cancelable: bool,
    pub composed: bool,
    pub default_prevented: bool,
    pub propagation_stopped: bool,
    pub immediate_stopped: bool,
    pub phase: u8,
    pub target: Value,
    pub current_target: Value,
    pub is_trusted: bool,
    pub time_stamp: f64,
    pub dispatching: bool,
    pub path: Vec<Value>,
    /// DOM "initialized flag": false for `document.createEvent()` events
    /// until `initEvent()`.
    pub initialized: bool,
    /// DOM "in passive listener flag": preventDefault() is ignored.
    pub in_passive: bool,
}

fn event_data<'a>(realm: &'a mut Realm, this: &Value) -> JsResult<&'a mut EventData> {
    match this.v() {
        V::Object(o) if realm.host_data::<EventData>(o).is_some() => Ok(realm.host_data_mut::<EventData>(o).unwrap()),
        _ => Err(realm.throw_type_error("Illegal invocation: not an Event")),
    }
}

/// Create an Event object of class `class` (prototype name).
pub(crate) fn create_event(realm: &mut Realm, class: &str, ty: &str, init: &EventInit, trusted: bool) -> ObjectId {
    let p = proto(class);
    let id = realm.new_object_with_proto(Some(p));
    let now = with_env(|env| env.now_ms());
    realm.set_host_data(
        id,
        Box::new(EventData {
            ty: ty.to_string(),
            bubbles: init.bubbles,
            cancelable: init.cancelable,
            composed: false,
            default_prevented: false,
            propagation_stopped: false,
            immediate_stopped: false,
            phase: 0,
            target: Value::Null,
            current_target: Value::Null,
            is_trusted: trusted,
            time_stamp: now,
            dispatching: false,
            path: Vec::new(),
            initialized: true,
            in_passive: false,
        }),
    );
    realm.set_class_name(id, class);
    // Detail fields as own data properties.
    let wheel = match &init.detail {
        Some(EventDetail::Wheel { x, y, dx, dy, ctrl, shift, alt, meta }) => Some((EventDetail::Mouse { x: *x, y: *y, button: 0, ctrl: *ctrl, shift: *shift, alt: *alt, meta: *meta }, *dx, *dy)),
        _ => None,
    };
    if let Some((_, dx, dy)) = &wheel {
        for (k, v) in [
            ("deltaX", Value::Number(*dx as f64)),
            ("deltaY", Value::Number(*dy as f64)),
            ("deltaZ", Value::Number(0.0)),
            ("deltaMode", Value::Number(0.0)),
        ] {
            realm.define_property(id, k, v, ACC);
        }
    }
    let detail = match &wheel {
        Some((m, _, _)) => Some(m),
        None => init.detail.as_ref(),
    };
    match detail {
        Some(EventDetail::Mouse { x, y, button, ctrl, shift, alt, meta }) => {
            let (sx, sy) = with_env(|env| env.scroll());
            for (k, v) in [
                ("clientX", Value::Number(*x as f64)),
                ("clientY", Value::Number(*y as f64)),
                ("pageX", Value::Number((*x + sx) as f64)),
                ("pageY", Value::Number((*y + sy) as f64)),
                ("screenX", Value::Number(*x as f64)),
                ("screenY", Value::Number(*y as f64)),
                ("x", Value::Number(*x as f64)),
                ("y", Value::Number(*y as f64)),
                ("offsetX", Value::Number(0.0)),
                ("offsetY", Value::Number(0.0)),
                ("movementX", Value::Number(0.0)),
                ("movementY", Value::Number(0.0)),
                ("button", Value::Number(*button as f64)),
                ("buttons", Value::Number(if wheel.is_some() { 0.0 } else if *button == 0 { 1.0 } else if *button == 1 { 4.0 } else { 2.0 })),
                ("ctrlKey", Value::Bool(*ctrl)),
                ("shiftKey", Value::Bool(*shift)),
                ("altKey", Value::Bool(*alt)),
                ("metaKey", Value::Bool(*meta)),
                ("relatedTarget", Value::Null),
                ("detail", Value::Number(if wheel.is_some() { 0.0 } else { 1.0 })),
                ("pointerId", Value::Number(1.0)),
                ("pointerType", Value::str("mouse")),
                ("isPrimary", Value::Bool(true)),
                ("width", Value::Number(1.0)),
                ("height", Value::Number(1.0)),
                ("pressure", Value::Number(0.5)),
            ] {
                realm.define_property(id, k, v, ACC);
            }
        }
        Some(EventDetail::Key { key, code, ctrl, shift, alt, meta, repeat }) => {
            let key_code = key_code_for(key);
            for (k, v) in [
                ("key", Value::str(key)),
                ("code", Value::str(code)),
                ("keyCode", Value::Number(key_code as f64)),
                ("which", Value::Number(key_code as f64)),
                ("charCode", Value::Number(if key.chars().count() == 1 { key.chars().next().unwrap() as u32 as f64 } else { 0.0 })),
                ("ctrlKey", Value::Bool(*ctrl)),
                ("shiftKey", Value::Bool(*shift)),
                ("altKey", Value::Bool(*alt)),
                ("metaKey", Value::Bool(*meta)),
                ("repeat", Value::Bool(*repeat)),
                ("isComposing", Value::Bool(false)),
                ("location", Value::Number(0.0)),
                ("detail", Value::Number(0.0)),
            ] {
                realm.define_property(id, k, v, ACC);
            }
            realm.define_method(id, "getModifierState", 1, key_get_modifier_state);
        }
        Some(EventDetail::Input { data }) => {
            realm.define_property(id, "data", data.as_deref().map(Value::str).unwrap_or(Value::Null), ACC);
            realm.define_property(id, "inputType", Value::str("insertText"), ACC);
            realm.define_property(id, "isComposing", Value::Bool(false), ACC);
            realm.define_property(id, "dataTransfer", Value::Null, ACC);
        }
        Some(EventDetail::Animation { name, elapsed, pseudo, transition }) => {
            realm.define_property(id, if *transition { "propertyName" } else { "animationName" }, Value::str(name), ACC);
            realm.define_property(id, "elapsedTime", Value::Number(*elapsed as f64), ACC);
            realm.define_property(id, "pseudoElement", Value::str(pseudo), ACC);
        }
        Some(EventDetail::Wheel { .. }) | Some(EventDetail::None) | None => {}
    }
    id
}

fn key_get_modifier_state(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let name = arg_str(realm, args, 0)?;
    let prop = match name.as_str() {
        "Control" => "ctrlKey",
        "Shift" => "shiftKey",
        "Alt" => "altKey",
        "Meta" => "metaKey",
        _ => return Ok(Value::Bool(false)),
    };
    realm.get_str(this, prop)
}

/// Legacy `keyCode` for a `key` value.
pub fn key_code_for(key: &str) -> u32 {
    match key {
        "Backspace" => 8,
        "Tab" => 9,
        "Enter" => 13,
        "Shift" => 16,
        "Control" => 17,
        "Alt" => 18,
        "Pause" => 19,
        "CapsLock" => 20,
        "Escape" => 27,
        " " => 32,
        "PageUp" => 33,
        "PageDown" => 34,
        "End" => 35,
        "Home" => 36,
        "ArrowLeft" => 37,
        "ArrowUp" => 38,
        "ArrowRight" => 39,
        "ArrowDown" => 40,
        "Insert" => 45,
        "Delete" => 46,
        "Meta" => 91,
        "ContextMenu" => 93,
        _ => {
            let mut chars = key.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => {
                    if c.is_ascii_alphabetic() {
                        c.to_ascii_uppercase() as u32
                    } else if c.is_ascii_digit() {
                        c as u32
                    } else {
                        match c {
                            ';' => 186,
                            '=' => 187,
                            ',' => 188,
                            '-' => 189,
                            '.' => 190,
                            '/' => 191,
                            '`' => 192,
                            '[' => 219,
                            '\\' => 220,
                            ']' => 221,
                            '\'' => 222,
                            _ => 0,
                        }
                    }
                }
                _ => {
                    if let Some(n) = key.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
                        111 + n
                    } else {
                        0
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------ Event natives

fn ev_init_from_dict(realm: &mut Realm, dict: &Value) -> JsResult<EventInit> {
    Ok(EventInit {
        bubbles: dict_bool(realm, dict, "bubbles")?.unwrap_or(false),
        cancelable: dict_bool(realm, dict, "cancelable")?.unwrap_or(false),
        detail: None,
    })
}

fn copy_dict_props(realm: &mut Realm, id: ObjectId, dict: &Value, props: &[(&str, Value)]) -> JsResult<()> {
    for (k, default) in props {
        let v = dict_value(realm, dict, k)?;
        let v = if v.is_undefined() { default.clone() } else { v };
        realm.define_property(id, *k, v, ACC);
    }
    Ok(())
}

fn event_ctor_generic(realm: &mut Realm, class: &str, args: &[Value]) -> JsResult<ObjectId> {
    let ty = arg_str(realm, args, 0)?;
    let dict = arg(args, 1);
    let init = ev_init_from_dict(realm, &dict)?;
    let id = create_event(realm, class, &ty, &init, false);
    if dict.is_object() {
        if let Some(true) = dict_bool(realm, &dict, "composed")? {
            if let Some(d) = realm.host_data_mut::<EventData>(id) {
                d.composed = true;
            }
        }
    }
    // Honour subclassing (`class MyEvent extends Event`).
    let p = realm.proto_from_new_target(proto(class));
    realm.obj_mut(id).set_proto(Some(p));
    Ok(id)
}

fn event_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Object(event_ctor_generic(realm, "Event", args)?))
}

fn custom_event_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = event_ctor_generic(realm, "CustomEvent", args)?;
    let dict = arg(args, 1);
    let detail = dict_value(realm, &dict, "detail")?;
    realm.define_property(id, "detail", if detail.is_undefined() { Value::Null } else { detail }, ACC);
    Ok(Value::Object(id))
}

fn ui_event_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = event_ctor_generic(realm, "UIEvent", args)?;
    let dict = arg(args, 1);
    copy_dict_props(realm, id, &dict, &[("detail", Value::Number(0.0)), ("view", Value::Null)])?;
    Ok(Value::Object(id))
}

fn mouse_event_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = event_ctor_generic(realm, "MouseEvent", args)?;
    let dict = arg(args, 1);
    let zero = Value::Number(0.0);
    copy_dict_props(
        realm,
        id,
        &dict,
        &[
            ("clientX", zero.clone()),
            ("clientY", zero.clone()),
            ("screenX", zero.clone()),
            ("screenY", zero.clone()),
            ("pageX", zero.clone()),
            ("pageY", zero.clone()),
            ("offsetX", zero.clone()),
            ("offsetY", zero.clone()),
            ("movementX", zero.clone()),
            ("movementY", zero.clone()),
            ("button", zero.clone()),
            ("buttons", zero.clone()),
            ("detail", zero.clone()),
            ("ctrlKey", Value::Bool(false)),
            ("shiftKey", Value::Bool(false)),
            ("altKey", Value::Bool(false)),
            ("metaKey", Value::Bool(false)),
            ("relatedTarget", Value::Null),
            ("view", Value::Null),
            ("pointerId", Value::Number(1.0)),
            ("pointerType", Value::str("mouse")),
            ("isPrimary", Value::Bool(true)),
            ("deltaX", zero.clone()),
            ("deltaY", zero.clone()),
            ("deltaZ", zero.clone()),
            ("deltaMode", zero),
        ],
    )?;
    // x/y aliases
    let cx = realm.get_str(Value::Object(id), "clientX")?;
    let cy = realm.get_str(Value::Object(id), "clientY")?;
    realm.define_property(id, "x", cx, ACC);
    realm.define_property(id, "y", cy, ACC);
    Ok(Value::Object(id))
}

fn keyboard_event_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = event_ctor_generic(realm, "KeyboardEvent", args)?;
    let dict = arg(args, 1);
    copy_dict_props(
        realm,
        id,
        &dict,
        &[
            ("key", Value::str("")),
            ("code", Value::str("")),
            ("location", Value::Number(0.0)),
            ("ctrlKey", Value::Bool(false)),
            ("shiftKey", Value::Bool(false)),
            ("altKey", Value::Bool(false)),
            ("metaKey", Value::Bool(false)),
            ("repeat", Value::Bool(false)),
            ("isComposing", Value::Bool(false)),
            ("charCode", Value::Number(0.0)),
        ],
    )?;
    let key = realm.get_str(Value::Object(id), "key")?;
    let key = realm.to_rust_string(key)?;
    let kc = dict_value(realm, &dict, "keyCode")?;
    let kc = if kc.is_undefined() { Value::Number(key_code_for(&key) as f64) } else { kc };
    realm.define_property(id, "keyCode", kc.clone(), ACC);
    let which = dict_value(realm, &dict, "which")?;
    realm.define_property(id, "which", if which.is_undefined() { kc } else { which }, ACC);
    realm.define_method(id, "getModifierState", 1, key_get_modifier_state);
    Ok(Value::Object(id))
}

fn input_event_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = event_ctor_generic(realm, "InputEvent", args)?;
    let dict = arg(args, 1);
    copy_dict_props(realm, id, &dict, &[("data", Value::Null), ("inputType", Value::str("")), ("isComposing", Value::Bool(false)), ("dataTransfer", Value::Null)])?;
    Ok(Value::Object(id))
}

fn focus_event_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = event_ctor_generic(realm, "FocusEvent", args)?;
    let dict = arg(args, 1);
    copy_dict_props(realm, id, &dict, &[("relatedTarget", Value::Null)])?;
    Ok(Value::Object(id))
}

fn error_event_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = event_ctor_generic(realm, "ErrorEvent", args)?;
    let dict = arg(args, 1);
    copy_dict_props(realm, id, &dict, &[("message", Value::str("")), ("filename", Value::str("")), ("lineno", Value::Number(0.0)), ("colno", Value::Number(0.0)), ("error", Value::Null)])?;
    Ok(Value::Object(id))
}

fn message_event_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = event_ctor_generic(realm, "MessageEvent", args)?;
    let dict = arg(args, 1);
    let ports = realm.new_array_value(vec![]);
    copy_dict_props(realm, id, &dict, &[("data", Value::Null), ("origin", Value::str("")), ("lastEventId", Value::str("")), ("source", Value::Null), ("ports", ports)])?;
    Ok(Value::Object(id))
}

fn popstate_event_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = event_ctor_generic(realm, "PopStateEvent", args)?;
    let dict = arg(args, 1);
    copy_dict_props(realm, id, &dict, &[("state", Value::Null)])?;
    Ok(Value::Object(id))
}

fn hashchange_event_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = event_ctor_generic(realm, "HashChangeEvent", args)?;
    let dict = arg(args, 1);
    copy_dict_props(realm, id, &dict, &[("oldURL", Value::str("")), ("newURL", Value::str(""))])?;
    Ok(Value::Object(id))
}

fn progress_event_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = event_ctor_generic(realm, "ProgressEvent", args)?;
    let dict = arg(args, 1);
    copy_dict_props(realm, id, &dict, &[("lengthComputable", Value::Bool(false)), ("loaded", Value::Number(0.0)), ("total", Value::Number(0.0))])?;
    Ok(Value::Object(id))
}

fn generic_sub_event_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    // Used for classes whose dictionary members are copied verbatim.
    let id = event_ctor_generic(realm, "Event", args)?;
    let dict = arg(args, 1);
    if dict.is_object() {
        if let V::Object(d) = dict.v() {
            let keys = realm.own_enumerable_string_keys(d)?;
            for k in keys {
                let ks = k.as_str().map(|s| s.to_rust_string()).unwrap_or_default();
                if matches!(ks.as_str(), "bubbles" | "cancelable" | "composed") {
                    continue;
                }
                let v = realm.get(dict.clone(), k.clone())?;
                realm.define_property(id, k, v, ACC);
            }
        }
    }
    let p = realm.proto_from_new_target(proto("Event"));
    realm.obj_mut(id).set_proto(Some(p));
    Ok(Value::Object(id))
}

macro_rules! ev_getter {
    ($name:ident, $field:ident, $conv:expr) => {
        fn $name(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
            let d = event_data(realm, &this)?;
            #[allow(clippy::redundant_closure_call)]
            Ok(($conv)(&d.$field))
        }
    };
}
ev_getter!(ev_type, ty, |t: &String| Value::str(t));
ev_getter!(ev_bubbles, bubbles, |b: &bool| Value::Bool(*b));
ev_getter!(ev_cancelable, cancelable, |b: &bool| Value::Bool(*b));
ev_getter!(ev_composed, composed, |b: &bool| Value::Bool(*b));
ev_getter!(ev_default_prevented, default_prevented, |b: &bool| Value::Bool(*b));
ev_getter!(ev_target, target, |v: &Value| v.clone());
ev_getter!(ev_current_target, current_target, |v: &Value| v.clone());
ev_getter!(ev_is_trusted, is_trusted, |b: &bool| Value::Bool(*b));
ev_getter!(ev_time_stamp, time_stamp, |t: &f64| Value::Number(*t));
ev_getter!(ev_phase, phase, |p: &u8| Value::Number(*p as f64));
ev_getter!(ev_cancel_bubble, propagation_stopped, |b: &bool| Value::Bool(*b));

fn ev_return_value(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let d = event_data(realm, &this)?;
    Ok(Value::Bool(!d.default_prevented))
}

fn ev_return_value_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let v = realm.to_boolean(&arg(args, 0));
    let d = event_data(realm, &this)?;
    if !v && d.cancelable && !d.in_passive {
        d.default_prevented = true;
    }
    Ok(Value::Undefined)
}

fn ev_cancel_bubble_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let v = realm.to_boolean(&arg(args, 0));
    let d = event_data(realm, &this)?;
    if v {
        d.propagation_stopped = true;
    }
    Ok(Value::Undefined)
}

fn ev_prevent_default(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let d = event_data(realm, &this)?;
    if d.cancelable && !d.in_passive {
        d.default_prevented = true;
    }
    Ok(Value::Undefined)
}

fn ev_stop_propagation(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    event_data(realm, &this)?.propagation_stopped = true;
    Ok(Value::Undefined)
}

fn ev_stop_immediate(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let d = event_data(realm, &this)?;
    d.propagation_stopped = true;
    d.immediate_stopped = true;
    Ok(Value::Undefined)
}

fn ev_composed_path(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let path = event_data(realm, &this)?.path.clone();
    Ok(realm.new_array_value(path))
}

fn ev_init_event(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let ty = arg_str(realm, args, 0)?;
    let bubbles = realm.to_boolean(&arg(args, 1));
    let cancelable = realm.to_boolean(&arg(args, 2));
    let d = event_data(realm, &this)?;
    // DOM "initialize an event" (ignored while dispatching).
    if !d.dispatching {
        d.initialized = true;
        d.propagation_stopped = false;
        d.immediate_stopped = false;
        d.default_prevented = false;
        d.is_trusted = false;
        d.target = Value::Null;
        d.ty = ty;
        d.bubbles = bubbles;
        d.cancelable = cancelable;
    }
    Ok(Value::Undefined)
}

fn ev_init_custom_event(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    ev_init_event(realm, this.clone(), args)?;
    if let V::Object(id) = this.v() {
        realm.define_property(id, "detail", arg(args, 3), ACC);
    }
    Ok(Value::Undefined)
}

fn ev_init_mouse_event(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    ev_init_event(realm, this.clone(), args)?;
    if let V::Object(id) = this.v() {
        let names = ["view", "detail", "screenX", "screenY", "clientX", "clientY", "ctrlKey", "altKey", "shiftKey", "metaKey", "button", "relatedTarget"];
        for (i, n) in names.iter().enumerate() {
            realm.define_property(id, *n, arg(args, 3 + i), ACC);
        }
    }
    Ok(Value::Undefined)
}

// -------------------------------------------------------------- EventTarget

/// Resolve `this` of an EventTarget method to a key.
pub(crate) fn target_key(realm: &Realm, this: &Value) -> Option<TargetKey> {
    match this.v() {
        V::Object(o) => {
            if let Some(n) = realm.host_data::<NodeRef>(o) {
                return Some(TargetKey::Node(*n));
            }
            if o == realm.global() || Some(o) == state().borrow().protos.get("__window").copied() {
                return Some(TargetKey::Window);
            }
            Some(TargetKey::Object(o))
        }
        V::Undefined | V::Null => Some(TargetKey::Window),
        _ => None,
    }
}

fn listener_options(realm: &mut Realm, v: &Value) -> JsResult<(bool, bool, bool, Option<ObjectId>)> {
    let (c, o, p, s) = listener_options_raw(realm, v)?;
    Ok((c, o, p.unwrap_or(false), s))
}

/// (capture, once, passive if given, signal) of an options argument.
fn listener_options_raw(realm: &mut Realm, v: &Value) -> JsResult<(bool, bool, Option<bool>, Option<ObjectId>)> {
    match v.v() {
        V::Object(_) => {
            let capture = dict_bool(realm, v, "capture")?.unwrap_or(false);
            let once = dict_bool(realm, v, "once")?.unwrap_or(false);
            let passive = dict_bool(realm, v, "passive")?;
            let signal = dict_value(realm, v, "signal")?;
            let signal = match signal.v() {
                V::Undefined => None,
                V::Object(o) => Some(o),
                _ => return Err(realm.throw_type_error("Failed to read the 'signal' property from 'AddEventListenerOptions': Failed to convert value to 'AbortSignal'.")),
            };
            Ok((capture, once, passive, signal))
        }
        V::Undefined | V::Null => Ok((false, false, None, None)),
        other => Ok((realm.to_boolean(&other.to_value()), false, None, None)),
    }
}

/// DOM "default passive value": touch and wheel listeners on the window,
/// the document, the document element and the body are passive.
fn default_passive(key: TargetKey, ty: &str) -> bool {
    if !matches!(ty, "touchstart" | "touchmove" | "wheel" | "mousewheel") {
        return false;
    }
    match key {
        TargetKey::Window => true,
        TargetKey::Node(n) => with_doc(n.doc, |doc| n.node == Document::ROOT || doc.html_element() == Some(n.node) || doc.body() == Some(n.node)),
        _ => false,
    }
}

fn et_add_event_listener(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(key) = target_key(realm, &this) else {
        return Err(realm.throw_type_error("Illegal invocation"));
    };
    let ty = arg_str(realm, args, 0)?;
    let cb = arg(args, 1);
    if !cb.is_object() {
        return Ok(Value::Undefined);
    }
    let (capture, once, passive, signal) = listener_options_raw(realm, &arg(args, 2))?;
    let passive = passive.unwrap_or_else(|| default_passive(key, &ty));
    if let Some(sig) = signal {
        if get_hidden(realm, sig, "__aborted").truthy() {
            return Ok(Value::Undefined);
        }
    }
    add_listener(realm, key, &ty, cb, capture, once, passive, signal);
    Ok(Value::Undefined)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn add_listener(realm: &mut Realm, key: TargetKey, ty: &str, cb: Value, capture: bool, once: bool, passive: bool, signal: Option<ObjectId>) {
    {
        let st = state();
        let mut st = st.borrow_mut();
        let list = st.listeners.entry(key).or_default();
        if list.iter().any(|l| l.ty == ty && l.capture == capture && realm.strict_equals(&l.callback, &cb)) {
            return;
        }
        list.push(Listener { ty: ty.to_string(), callback: cb.clone(), capture, once, passive, signal, removed: Rc::new(std::cell::Cell::new(false)), is_handler: false });
        *st.listener_counts.entry((key, ty.to_string())).or_insert(0) += 1;
    }
    root_value(realm, &cb);
    if let Some(sig) = signal {
        // Register for removal on abort.
        let entry = realm.new_array_value(vec![]);
        let existing = get_hidden(realm, sig, "__listeners");
        let arr = if existing.is_object() { existing } else { set_hidden(realm, sig, "__listeners", entry.clone()); entry };
        let rec = object_from(realm, vec![("type", Value::str(ty)), ("callback", cb), ("capture", Value::Bool(capture))]);
        if let (V::Object(a), V::Object(_)) = (arr.v(), rec.v()) {
            if let rabbithole_js::ObjectKind::Array { elements, length } = &mut realm.obj_mut(a).kind {
                elements.push(rec);
                *length = elements.len() as u32;
            }
        }
    }
}

pub(crate) fn remove_listener(realm: &mut Realm, key: TargetKey, ty: &str, cb: &Value, capture: bool) {
    let removed_cb = {
        let st = state();
        let mut st = st.borrow_mut();
        let Some(list) = st.listeners.get_mut(&key) else { return };
        let pos = list.iter().position(|l| l.ty == ty && l.capture == capture && realm.strict_equals(&l.callback, cb));
        match pos {
            Some(p) => {
                let l = list.remove(p);
                l.removed.set(true);
                if let Some(c) = st.listener_counts.get_mut(&(key, ty.to_string())) {
                    *c = c.saturating_sub(1);
                }
                Some(l.callback)
            }
            None => None,
        }
    };
    if let Some(cb) = removed_cb {
        unroot_value(realm, &cb);
    }
}

fn et_remove_event_listener(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(key) = target_key(realm, &this) else {
        return Err(realm.throw_type_error("Illegal invocation"));
    };
    let ty = arg_str(realm, args, 0)?;
    let cb = arg(args, 1);
    let (capture, _, _, _) = listener_options(realm, &arg(args, 2))?;
    remove_listener(realm, key, &ty, &cb, capture);
    Ok(Value::Undefined)
}

fn et_dispatch_event(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(key) = target_key(realm, &this) else {
        return Err(realm.throw_type_error("Illegal invocation"));
    };
    let ev = arg(args, 0);
    let V::Object(eid) = ev.v() else {
        return Err(realm.throw_type_error("parameter 1 is not of type 'Event'."));
    };
    if realm.host_data::<EventData>(eid).is_none() {
        return Err(realm.throw_type_error("parameter 1 is not of type 'Event'."));
    }
    if realm.host_data::<EventData>(eid).map(|d| d.dispatching || !d.initialized).unwrap_or(false) {
        return Err(crate::misc::dom_exception(realm, "InvalidStateError", "Failed to execute 'dispatchEvent' on 'EventTarget': The event is already being dispatched or was not initialized."));
    }
    if let Some(d) = realm.host_data_mut::<EventData>(eid) {
        d.is_trusted = false;
    }
    if let TargetKey::Node(n) = key {
        if n.doc & crate::host::FRAME_DOC != 0 {
            // A node of a same-origin frame: its listeners live in the
            // frame's realm, where the event is re-created and dispatched.
            return dispatch_into_frame(realm, n, eid, &ev);
        }
    }
    let r = dispatch_object(realm, key, eid, Some(this));
    Ok(Value::Bool(!r.default_prevented))
}

/// `dispatchEvent` at a node of a same-origin child frame's document.
fn dispatch_into_frame(realm: &mut Realm, n: NodeRef, eid: ObjectId, ev: &Value) -> JsResult<Value> {
    let frame = (n.doc & !crate::host::FRAME_DOC) as u64;
    let api = state().borrow().protos.get("__frames_api").copied();
    let (iface, init) = match api {
        Some(api) => {
            let f = realm.get_str(Value::Object(api), "eventInit")?;
            let pair = realm.call(f, Value::Object(api), &[ev.clone()])?;
            let _pair_root = realm.root(pair.clone());
            let iface = realm.get_str(pair.clone(), "0")?;
            let init = realm.get_str(pair, "1")?;
            (realm.to_rust_string(iface)?, realm.to_rust_string(init)?)
        }
        None => ("Event".to_string(), "{}".to_string()),
    };
    let ty = realm.host_data::<EventData>(eid).map(|d| d.ty.clone()).unwrap_or_default();
    match crate::frames::call_frame(frame, crate::FrameCall::Dispatch { node: n.node, iface, ty, init }) {
        Some(crate::FrameReply::Bool(not_cancelled)) => {
            if !not_cancelled {
                if let Some(d) = realm.host_data_mut::<EventData>(eid) {
                    d.default_prevented = true;
                }
            }
            Ok(Value::Bool(not_cancelled))
        }
        Some(crate::FrameReply::Error(m)) => {
            crate::host::log("error", &format!("Uncaught {m}"));
            Ok(Value::Bool(true))
        }
        // The frame is gone (or not scriptable): nothing listens.
        _ => {
            let r = dispatch_object(realm, TargetKey::Node(n), eid, None);
            Ok(Value::Bool(!r.default_prevented))
        }
    }
}

fn et_ctor(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    let p = realm.proto_from_new_target(proto("EventTarget"));
    let o = realm.new_object_with_proto(Some(p));
    realm.set_class_name(o, "EventTarget");
    Ok(Value::Object(o))
}

// --------------------------------------------------------------- dispatch

/// DOM "get the parent" for event paths: a slotted node's parent is its
/// assigned slot, a shadow root's parent is its host (unless the event is
/// not composed and the shadow root is the root of the original target).
pub(crate) fn event_parent(doc: &Document, node: NodeId, composed: bool, target_root: NodeId) -> Option<NodeId> {
    if let Some(p) = doc.parent(node) {
        if doc.shadow_root(p).is_some() {
            // Slottables of a shadow host propagate through their slot.
            if let Some(s) = doc.assigned_slot(node) {
                return Some(s);
            }
        }
        return Some(p);
    }
    let host = doc.shadow_host(node)?;
    if !composed && node == target_root {
        return None;
    }
    Some(host)
}

/// The propagation path for a target: [(key, invocation target,
/// shadow-adjusted target)] from the target to the document and window.
fn build_path(realm: &mut Realm, key: TargetKey, composed: bool) -> Vec<(TargetKey, Value, Value)> {
    let mut path = Vec::new();
    match key {
        TargetKey::Window => {
            let w = window_value(realm);
            path.push((TargetKey::Window, w.clone(), w));
        }
        TargetKey::Object(o) => path.push((key, Value::Object(o), Value::Object(o))),
        TargetKey::Node(n) => {
            let mut chain: Vec<(NodeId, NodeId)> = Vec::new();
            let mut reaches_document = false;
            with_doc(n.doc, |doc| {
                let target_root = doc.tree_root(n.node);
                let mut cur = Some(n.node);
                while let Some(c) = cur {
                    let adjusted = if doc.shadow_hosts.is_empty() { n.node } else { crate::shadow::retarget(doc, n.node, c) };
                    chain.push((c, adjusted));
                    if c == Document::ROOT {
                        reaches_document = true;
                    }
                    cur = event_parent(doc, c, composed, target_root);
                    if chain.len() > 100_000 {
                        break;
                    }
                }
            });
            for (c, adjusted) in chain {
                let r = NodeRef { doc: n.doc, node: c };
                let v = wrap(realm, r);
                let t = if adjusted == c { v.clone() } else { wrap(realm, NodeRef { doc: n.doc, node: adjusted }) };
                path.push((TargetKey::Node(r), v, t));
            }
            // Only main-document paths that reach the document reach the window.
            if n.doc == 0 && reaches_document {
                let w = window_value(realm);
                let doc_target = path.last().map(|(_, _, t)| t.clone()).unwrap_or(Value::Null);
                path.push((TargetKey::Window, w, doc_target));
            }
        }
    }
    path
}

/// Trusted UI events are composed (they cross shadow boundaries).
fn is_composed_ui_event(ty: &str) -> bool {
    matches!(
        ty,
        "click" | "auxclick" | "contextmenu" | "dblclick" | "mousedown" | "mouseup" | "mousemove" | "mouseover" | "mouseout" | "wheel"
            | "pointerdown" | "pointerup" | "pointermove" | "pointerover" | "pointerout" | "pointercancel" | "gotpointercapture" | "lostpointercapture"
            | "keydown" | "keyup" | "keypress" | "input" | "beforeinput" | "focus" | "blur" | "focusin" | "focusout"
            | "touchstart" | "touchend" | "touchmove" | "touchcancel" | "compositionstart" | "compositionupdate" | "compositionend" | "select" | "selectionchange"
            | "dragstart" | "drag" | "dragend" | "dragenter" | "dragleave" | "dragover" | "drop" | "copy" | "cut" | "paste"
    )
}

pub(crate) fn window_value(realm: &mut Realm) -> Value {
    let _ = realm;
    Value::Object(state().borrow().window)
}

/// Dispatch a freshly created event of `event_type` to `key`.
pub(crate) fn dispatch(realm: &mut Realm, key: TargetKey, event_type: &str, init: EventInit, trusted: bool) -> DispatchResult {
    // Skip entirely when nobody listens (cheap path for hover/move events).
    if !any_listener_on_path(realm, key, event_type) {
        return DispatchResult { default_prevented: false, handled: false, default_action: default_action_for(realm, key, event_type) };
    }
    let class = event_class_for(event_type, &init);
    let eid = create_event(realm, class, event_type, &init, trusted);
    set_mouse_offset(realm, key, eid, &init);
    if trusted && is_composed_ui_event(event_type) {
        if let Some(d) = realm.host_data_mut::<EventData>(eid) {
            d.composed = true;
        }
    }
    dispatch_object(realm, key, eid, None)
}

/// `offsetX`/`offsetY` of a mouse event: the pointer relative to the
/// target's padding edge.
fn set_mouse_offset(realm: &mut Realm, key: TargetKey, eid: ObjectId, init: &EventInit) {
    let (x, y) = match &init.detail {
        Some(EventDetail::Mouse { x, y, .. }) | Some(EventDetail::Wheel { x, y, .. }) => (x, y),
        _ => return,
    };
    let TargetKey::Node(n) = key else { return };
    if n.doc != 0 {
        return;
    }
    let geo = with_env(|env| {
        let r = env.node_rect(n.node)?;
        let b = env.computed_style(n.node).map(|s| (s.border_width.left, s.border_width.top)).unwrap_or((0.0, 0.0));
        let (sx, sy) = env.scroll();
        Some((x + sx - r.x - b.0, y + sy - r.y - b.1))
    });
    if let Some((ox, oy)) = geo {
        realm.define_property(eid, "offsetX", Value::Number(ox as f64), ACC);
        realm.define_property(eid, "offsetY", Value::Number(oy as f64), ACC);
    }
}

fn event_class_for(ty: &str, init: &EventInit) -> &'static str {
    match &init.detail {
        Some(EventDetail::Mouse { .. }) => {
            if ty.starts_with("pointer") {
                "PointerEvent"
            } else if ty == "wheel" {
                "WheelEvent"
            } else {
                "MouseEvent"
            }
        }
        Some(EventDetail::Wheel { .. }) => "WheelEvent",
        Some(EventDetail::Key { .. }) => "KeyboardEvent",
        Some(EventDetail::Input { .. }) => "InputEvent",
        Some(EventDetail::Animation { transition: true, .. }) => "TransitionEvent",
        Some(EventDetail::Animation { transition: false, .. }) => "AnimationEvent",
        _ => match ty {
            "focus" | "blur" | "focusin" | "focusout" => "FocusEvent",
            "submit" => "SubmitEvent",
            "input" | "beforeinput" => "InputEvent",
            "scroll" | "resize" | "load" | "unload" | "abort" | "error" | "select" => "UIEvent",
            "popstate" => "PopStateEvent",
            "hashchange" => "HashChangeEvent",
            "message" => "MessageEvent",
            _ => "Event",
        },
    }
}

/// Does any target on the path (or an inline handler) listen for `ty`?
fn any_listener_on_path(realm: &mut Realm, key: TargetKey, ty: &str) -> bool {
    let _ = realm;
    let st = state();
    let st = st.borrow();
    let has = |k: TargetKey| st.listener_counts.get(&(k, ty.to_string())).map(|c| *c > 0).unwrap_or(false);
    let handler_key = format!("__on_{ty}");
    match key {
        TargetKey::Window => {
            has(TargetKey::Window)
                || st.protos.get("__window").map(|w| realm.obj(*w).props.get(&PropertyKey::str(&handler_key)).map(|p| p.value.is_object()).unwrap_or(false)).unwrap_or(false)
                || with_doc_in_state(&st, 0, |doc| doc.body().map(|b| doc.attr(b, &format!("on{ty}")).is_some()).unwrap_or(false))
        }
        TargetKey::Object(o) => has(key) || realm.obj(o).props.get(&PropertyKey::str(&handler_key)).map(|p| p.value.is_object()).unwrap_or(false),
        TargetKey::Node(n) => {
            if has(TargetKey::Window) {
                return true;
            }
            let attr = format!("on{ty}");
            // Every node on the (composed, worst case) event path.
            let chain: Vec<NodeId> = with_doc_in_state(&st, n.doc, |doc| {
                let root = doc.tree_root(n.node);
                let mut v = Vec::new();
                let mut cur = Some(n.node);
                while let Some(c) = cur {
                    v.push(c);
                    cur = event_parent(doc, c, true, root);
                }
                v
            });
            let found = with_doc_in_state(&st, n.doc, |doc| chain.iter().any(|&c| has(TargetKey::Node(NodeRef { doc: n.doc, node: c })) || doc.attr(c, &attr).is_some()));
            if found {
                return true;
            }
            // IDL `onX` handlers stored on wrappers.
            for &c in &chain {
                if let Some(w) = st.wrappers.get(&NodeRef { doc: n.doc, node: c }) {
                    if realm.obj(*w).props.get(&PropertyKey::str(&handler_key)).map(|p| p.value.is_object()).unwrap_or(false) {
                        return true;
                    }
                }
            }
            if let Some(w) = st.protos.get("__window") {
                if realm.obj(*w).props.get(&PropertyKey::str(&handler_key)).map(|p| p.value.is_object()).unwrap_or(false) {
                    return true;
                }
            }
            false
        }
    }
}

fn with_doc_in_state<R>(st: &HostState, doc: u32, f: impl FnOnce(&Document) -> R) -> R {
    if doc == 0 {
        with_env(|env| f(env.doc()))
    } else if doc & crate::host::FRAME_DOC != 0 {
        with_env(|env| match env.frame_document((doc & !crate::host::FRAME_DOC) as u64) {
            Some(d) => f(d),
            None => f(&Document::new()),
        })
    } else {
        match st.extra_docs.get((doc - 1) as usize) {
            Some(d) => f(d),
            None => f(&Document::new()),
        }
    }
}

/// Default action the embedder should perform after an uncancelled event.
fn default_action_for(realm: &mut Realm, key: TargetKey, ty: &str) -> DefaultAction {
    let _ = realm;
    if ty != "click" {
        return DefaultAction::None;
    }
    let TargetKey::Node(n) = key else { return DefaultAction::None };
    if n.doc != 0 {
        return DefaultAction::None;
    }
    with_doc(0, |doc| {
        let root = doc.tree_root(n.node);
        let mut cur = Some(n.node);
        while let Some(c) = cur {
            if let Some(el) = doc.element(c) {
                match el.tag.as_str() {
                    "a" | "area" => {
                        if let Some(h) = el.attr("href") {
                            if let Some(u) = resolve_url(h) {
                                return DefaultAction::Navigate(u);
                            }
                        }
                        return DefaultAction::None;
                    }
                    "button" | "input" => {
                        let ty = el.attr("type").map(|t| t.to_ascii_lowercase()).unwrap_or_else(|| if el.tag == "button" { "submit".into() } else { "text".into() });
                        if ty == "submit" || ty == "image" {
                            if let Some(f) = doc.ancestor_or_self(c, |a| doc.is_element(a, "form")) {
                                return DefaultAction::SubmitForm(f, Some(c));
                            }
                        }
                        return DefaultAction::None;
                    }
                    "label" => {
                        return DefaultAction::None;
                    }
                    _ => {}
                }
            }
            cur = event_parent(doc, c, true, root);
        }
        DefaultAction::None
    })
}

/// Perform the default action of a click on `n` through the environment.
pub(crate) fn run_default_action(realm: &mut Realm, n: NodeRef) {
    match default_action_for(realm, TargetKey::Node(n), "click") {
        DefaultAction::Navigate(u) => with_env(|env| env.navigate(u, false)),
        DefaultAction::SubmitForm(f, s) => {
            let r = dispatch(realm, TargetKey::Node(NodeRef::main(f)), "submit", EventInit::simple(true, true), true);
            if !r.default_prevented {
                with_env(|env| env.submit_form(f, s));
            }
        }
        DefaultAction::None => {
            // Checkbox / radio activation.
            {
                let kind = with_doc(n.doc, |doc| {
                    if doc.is_element(n.node, "input") {
                        doc.attr(n.node, "type").map(|t| t.to_ascii_lowercase())
                    } else {
                        None
                    }
                });
                match kind.as_deref() {
                    Some("checkbox") => {
                        let checked = with_doc(n.doc, |doc| doc.is_checked(n.node));
                        crate::element::set_checkedness(n, Some(!checked));
                        dispatch(realm, TargetKey::Node(n), "input", EventInit::simple(true, false), true);
                        dispatch(realm, TargetKey::Node(n), "change", EventInit::simple(true, false), true);
                    }
                    Some("radio") => {
                        let w = wrap(realm, n);
                        let _ = realm.set_str(w, "checked", Value::Bool(true));
                        dispatch(realm, TargetKey::Node(n), "input", EventInit::simple(true, false), true);
                        dispatch(realm, TargetKey::Node(n), "change", EventInit::simple(true, false), true);
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Dispatch an existing Event object.
pub(crate) fn dispatch_object(realm: &mut Realm, key: TargetKey, eid: ObjectId, target_override: Option<Value>) -> DispatchResult {
    let composed = realm.host_data::<EventData>(eid).map(|d| d.composed).unwrap_or(false);
    let mut path = build_path(realm, key, composed);
    if let Some(t) = target_override {
        // dispatchEvent on a non-node target object (keep its identity).
        if !matches!(key, TargetKey::Node(_)) {
            for p in path.iter_mut() {
                p.2 = t.clone();
            }
        }
    }
    let target_value = path.first().map(|(_, _, t)| t.clone()).unwrap_or(Value::Null);
    let (ty, bubbles) = {
        let d = realm.host_data_mut::<EventData>(eid).expect("event data");
        d.dispatching = true;
        d.target = target_value.clone();
        d.path = path.iter().map(|(_, v, _)| v.clone()).collect();
        (d.ty.clone(), d.bubbles)
    };
    let default_action = default_action_for(realm, key, &ty);
    state().borrow_mut().event_stack.push(eid);
    // The event object and the path's targets are only held by this Rust
    // frame between listener calls: keep them alive across GCs that
    // listeners trigger.
    let _event_root = realm.root_object(eid);
    let _path_roots: Vec<rabbithole_js::Root> = path.iter().map(|(_, v, t)| (realm.root(v.clone()), realm.root(t.clone()))).flat_map(|(a, b)| [a, b]).collect();
    let mut handled = false;

    // An entry whose shadow-adjusted target is itself is "at target"
    // (the target, or a shadow host the event was retargeted to).
    let at_target = |p: &(TargetKey, Value, Value)| match (p.1.v(), p.2.v()) {
        (V::Object(a), V::Object(b)) => a == b,
        _ => false,
    };
    // Capture phase: window → … → target.
    for (i, entry) in path.iter().enumerate().rev() {
        if stopped(realm, eid) {
            break;
        }
        let (k, v, t) = entry;
        let phase = if at_target(entry) { 2 } else { 1 };
        if i != 0 && phase == 1 {
            set_phase_target(realm, eid, 1, v.clone(), t.clone());
            handled |= invoke_listeners(realm, *k, v, eid, &ty, true, false);
        } else if phase == 2 {
            set_phase_target(realm, eid, 2, v.clone(), t.clone());
            handled |= invoke_listeners(realm, *k, v, eid, &ty, true, true);
        }
    }
    // Target / bubble phase: target → … → window.
    for (i, entry) in path.iter().enumerate() {
        if stopped(realm, eid) {
            break;
        }
        let (k, v, t) = entry;
        let is_target = i == 0 || at_target(entry);
        if is_target {
            set_phase_target(realm, eid, 2, v.clone(), t.clone());
            handled |= invoke_listeners(realm, *k, v, eid, &ty, false, true);
        } else if bubbles {
            set_phase_target(realm, eid, 3, v.clone(), t.clone());
            handled |= invoke_listeners(realm, *k, v, eid, &ty, false, false);
        }
    }
    state().borrow_mut().event_stack.pop();
    let d = realm.host_data_mut::<EventData>(eid).expect("event data");
    d.dispatching = false;
    d.phase = 0;
    d.current_target = Value::Null;
    // The stop propagation flags are per dispatch.
    d.propagation_stopped = false;
    d.immediate_stopped = false;
    // The target stays the one seen from the outermost tree.
    if let Some((_, _, t)) = path.last() {
        d.target = t.clone();
    }
    d.path.clear();
    let default_prevented = d.default_prevented;
    DispatchResult { default_prevented, handled, default_action: if default_prevented { DefaultAction::None } else { default_action } }
}

fn stopped(realm: &Realm, eid: ObjectId) -> bool {
    realm.host_data::<EventData>(eid).map(|d| d.propagation_stopped).unwrap_or(true)
}

fn immediate_stopped(realm: &Realm, eid: ObjectId) -> bool {
    realm.host_data::<EventData>(eid).map(|d| d.immediate_stopped).unwrap_or(true)
}

fn set_phase_target(realm: &mut Realm, eid: ObjectId, phase: u8, current: Value, target: Value) {
    if let Some(d) = realm.host_data_mut::<EventData>(eid) {
        d.phase = phase;
        d.current_target = current;
        d.target = target;
    }
}

/// Run the listeners of one target for `ty` in the given phase. Returns
/// whether any ran.
fn run_inline_handler(realm: &mut Realm, key: TargetKey, current: &Value, eid: ObjectId, ty: &str) -> bool {
    let ev = Value::Object(eid);
    let Some(h) = inline_handler(realm, key, current, ty) else { return false };
    // Each listener of a top-level dispatch is a watchdog task of its own.
    realm.begin_task();
    let r = realm.call(h, current.clone(), &[ev]);
    match r {
        Ok(v) => {
            // Returning false from an inline handler cancels.
            if matches!(v.v(), V::Bool(false)) && ty != "mouseover" {
                if let Some(d) = realm.host_data_mut::<EventData>(eid) {
                    if d.cancelable {
                        d.default_prevented = true;
                    }
                }
            }
        }
        Err(e) => {
            LISTENER_THREW.with(|c| c.set(true));
            report_error(realm, &e, &format!("on{ty} handler"))
        }
    }
    true
}

fn invoke_listeners(realm: &mut Realm, key: TargetKey, current: &Value, eid: ObjectId, ty: &str, capture: bool, at_target: bool) -> bool {
    let _ = at_target;
    let mut ran = false;
    let ev = Value::Object(eid);
    let snapshot: Vec<Listener> = {
        let st = state();
        let st = st.borrow();
        match st.listeners.get(&key) {
            Some(list) => list.iter().filter(|l| l.ty == ty && l.capture == capture).cloned().collect(),
            None => Vec::new(),
        }
    };
    // Content-attribute handlers (`onclick="..."`) come first when no IDL
    // handler marker has been registered yet.
    if !capture && !snapshot.iter().any(|l| l.is_handler) {
        ran |= run_inline_handler(realm, key, current, eid, ty);
    }
    for l in snapshot {
        if l.removed.get() {
            continue;
        }
        if immediate_stopped(realm, eid) {
            break;
        }
        if l.is_handler {
            ran |= run_inline_handler(realm, key, current, eid, ty);
            continue;
        }
        if l.once {
            remove_listener(realm, key, ty, &l.callback, l.capture);
        }
        ran = true;
        let cb = l.callback.clone();
        if l.passive {
            if let Some(d) = realm.host_data_mut::<EventData>(eid) {
                d.in_passive = true;
            }
        }
        realm.begin_task();
        let r = if realm.is_callable(&cb) {
            realm.call(cb, current.clone(), &[ev.clone()])
        } else {
            match realm.get_str(cb.clone(), "handleEvent") {
                Ok(h) if realm.is_callable(&h) => realm.call(h, cb, &[ev.clone()]),
                Ok(_) => Err(realm.throw_type_error("The listener's handleEvent is not callable.")),
                Err(e) => Err(e),
            }
        };
        if let Some(d) = realm.host_data_mut::<EventData>(eid) {
            d.in_passive = false;
        }
        if let Err(e) = r {
            LISTENER_THREW.with(|c| c.set(true));
            report_error(realm, &e, &format!("{ty} listener"));
        }
    }
    ran
}

/// The `onX` handler of a target: IDL property, else compiled attribute.
fn inline_handler(realm: &mut Realm, key: TargetKey, current: &Value, ty: &str) -> Option<Value> {
    let hidden = format!("__on_{ty}");
    if let V::Object(o) = current.v() {
        let v = get_hidden(realm, o, &hidden);
        if realm.is_callable(&v) {
            return Some(v);
        }
        if !v.is_undefined() {
            return None; // explicitly set to null
        }
    }
    // `<body onload=...>` & co. are the window's handlers (HTML: "Window-
    // reflecting body element event handler set").
    let key = match key {
        TargetKey::Window
            if matches!(
                ty,
                "load" | "unload" | "beforeunload" | "resize" | "scroll" | "hashchange" | "popstate" | "message" | "pageshow" | "pagehide" | "storage" | "online" | "offline" | "focus" | "blur"
            ) =>
        {
            let body = with_doc(0, |doc| doc.body())?;
            TargetKey::Node(NodeRef { doc: 0, node: body })
        }
        k => k,
    };
    let TargetKey::Node(n) = key else { return None };
    let attr = format!("on{ty}");
    let body = with_doc(n.doc, |doc| doc.attr(n.node, &attr).map(|s| s.to_string()))?;
    let cached = state().borrow().inline_handlers.get(&(n, ty.to_string())).cloned();
    if let Some(c) = cached {
        return Some(c);
    }
    let src = format!("(function(event) {{\n{body}\n}})");
    match realm.eval(&src, "inline-handler") {
        Ok(f) => {
            root_value(realm, &f);
            state().borrow_mut().inline_handlers.insert((n, ty.to_string()), f.clone());
            Some(f)
        }
        Err(e) => {
            report_error(realm, &e, &attr);
            None
        }
    }
}

// ---------------------------------------------------- onX IDL attributes

struct HandlerSpec {
    ty: &'static str,
}

fn handler_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let spec = native_data::<HandlerSpec>(realm).expect("handler spec");
    let V::Object(o) = this.v() else { return Ok(Value::Null) };
    let v = get_hidden(realm, o, &format!("__on_{}", spec.ty));
    if !v.is_undefined() {
        return Ok(v);
    }
    // Fall back to the compiled content attribute.
    if let Some(n) = node_of(realm, &this) {
        if let Some(f) = inline_handler(realm, TargetKey::Node(n), &this, spec.ty) {
            return Ok(f);
        }
    }
    Ok(Value::Null)
}

fn handler_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let spec = native_data::<HandlerSpec>(realm).expect("handler spec");
    let V::Object(o) = this.v() else { return Ok(Value::Undefined) };
    let v = arg(args, 0);
    let v = if realm.is_callable(&v) { v } else { Value::Null };
    set_hidden(realm, o, &format!("__on_{}", spec.ty), v.clone());
    if v.is_object() {
        if let Some(key) = target_key(realm, &this) {
            let st = state();
            let mut st = st.borrow_mut();
            let list = st.listeners.entry(key).or_default();
            if !list.iter().any(|l| l.is_handler && l.ty == spec.ty) {
                list.push(Listener { ty: spec.ty.to_string(), callback: Value::Null, capture: false, once: false, passive: false, signal: None, removed: Rc::new(std::cell::Cell::new(false)), is_handler: true });
                *st.listener_counts.entry((key, spec.ty.to_string())).or_insert(0) += 1;
            }
        }
    }
    // Remove a content attribute so it doesn't run twice.
    if let Some(n) = node_of(realm, &this) {
        let attr = format!("on{}", spec.ty);
        if crate::element::get_attribute(n, &attr).is_some() {
            with_doc(n.doc, |doc| doc.set_attr(n.node, &attr, None));
            state().borrow_mut().inline_handlers.remove(&(n, spec.ty.to_string()));
        }
    }
    Ok(Value::Undefined)
}

/// Define `onclick`-style accessors on `obj`.
pub(crate) fn install_handler_props(realm: &mut Realm, obj: ObjectId, names: &[&'static str]) {
    for ty in names {
        let data: Rc<dyn std::any::Any> = Rc::new(HandlerSpec { ty });
        let g = realm.new_native_function_full(&format!("get on{ty}"), 0, handler_get, Some(data.clone()), vec![], false);
        let s = realm.new_native_function_full(&format!("set on{ty}"), 1, handler_set, Some(data), vec![], false);
        realm.define_accessor_values(obj, format!("on{ty}").as_str(), Some(Value::Object(g)), Some(Value::Object(s)), ACC);
    }
}

// ------------------------------------------------------------ installation

pub(crate) fn install(realm: &mut Realm) {
    let et = define_class(realm, "EventTarget", None, et_ctor);
    realm.define_method(et, "addEventListener", 2, et_add_event_listener);
    realm.define_method(et, "removeEventListener", 2, et_remove_event_listener);
    realm.define_method(et, "dispatchEvent", 1, et_dispatch_event);
    // Listeners may run whole applications (a framework's scheduler posts
    // its work loop through a MessageChannel): the dispatch roots the
    // event and its path (as for events the host dispatches), so the
    // collector may run inside it.
    realm.mark_gc_safe(et, "dispatchEvent");

    let ev = define_class(realm, "Event", None, event_ctor);
    let acc = |realm: &mut Realm, name: &str, g: rabbithole_js::NativeFn| realm.define_accessor(ev, name, Some(g), None, ACC);
    acc(realm, "type", ev_type);
    acc(realm, "bubbles", ev_bubbles);
    acc(realm, "cancelable", ev_cancelable);
    acc(realm, "composed", ev_composed);
    acc(realm, "defaultPrevented", ev_default_prevented);
    acc(realm, "target", ev_target);
    acc(realm, "srcElement", ev_target);
    acc(realm, "currentTarget", ev_current_target);
    acc(realm, "isTrusted", ev_is_trusted);
    acc(realm, "timeStamp", ev_time_stamp);
    acc(realm, "eventPhase", ev_phase);
    realm.define_accessor(ev, "returnValue", Some(ev_return_value), Some(ev_return_value_set), ACC);
    realm.define_accessor(ev, "cancelBubble", Some(ev_cancel_bubble), Some(ev_cancel_bubble_set), ACC);
    realm.define_method(ev, "preventDefault", 0, ev_prevent_default);
    realm.define_method(ev, "stopPropagation", 0, ev_stop_propagation);
    realm.define_method(ev, "stopImmediatePropagation", 0, ev_stop_immediate);
    realm.define_method(ev, "composedPath", 0, ev_composed_path);
    realm.define_method(ev, "initEvent", 3, ev_init_event);
    for (k, v) in [("NONE", 0.0), ("CAPTURING_PHASE", 1.0), ("AT_TARGET", 2.0), ("BUBBLING_PHASE", 3.0)] {
        realm.define_property(ev, k, Value::Number(v), 0);
    }
    let ce = define_class(realm, "CustomEvent", Some("Event"), custom_event_ctor);
    realm.define_method(ce, "initCustomEvent", 4, ev_init_custom_event);
    realm.define_property(ce, "detail", Value::Null, HIDDEN);
    define_class(realm, "UIEvent", Some("Event"), ui_event_ctor);
    let me = define_class(realm, "MouseEvent", Some("UIEvent"), mouse_event_ctor);
    realm.define_method(me, "initMouseEvent", 15, ev_init_mouse_event);
    define_class(realm, "PointerEvent", Some("MouseEvent"), mouse_event_ctor);
    define_class(realm, "WheelEvent", Some("MouseEvent"), mouse_event_ctor);
    define_class(realm, "DragEvent", Some("MouseEvent"), mouse_event_ctor);
    define_class(realm, "KeyboardEvent", Some("UIEvent"), keyboard_event_ctor);
    define_class(realm, "InputEvent", Some("UIEvent"), input_event_ctor);
    define_class(realm, "FocusEvent", Some("UIEvent"), focus_event_ctor);
    define_class(realm, "CompositionEvent", Some("UIEvent"), input_event_ctor);
    define_class(realm, "TouchEvent", Some("UIEvent"), generic_sub_event_ctor);
    define_class(realm, "ErrorEvent", Some("Event"), error_event_ctor);
    define_class(realm, "MessageEvent", Some("Event"), message_event_ctor);
    define_class(realm, "PopStateEvent", Some("Event"), popstate_event_ctor);
    define_class(realm, "HashChangeEvent", Some("Event"), hashchange_event_ctor);
    define_class(realm, "ProgressEvent", Some("Event"), progress_event_ctor);
    define_class(realm, "SubmitEvent", Some("Event"), generic_sub_event_ctor);
    define_class(realm, "TransitionEvent", Some("Event"), generic_sub_event_ctor);
    define_class(realm, "AnimationEvent", Some("Event"), generic_sub_event_ctor);
    define_class(realm, "BeforeUnloadEvent", Some("Event"), generic_sub_event_ctor);
    define_class(realm, "PageTransitionEvent", Some("Event"), generic_sub_event_ctor);
    define_class(realm, "StorageEvent", Some("Event"), generic_sub_event_ctor);
    define_class(realm, "ClipboardEvent", Some("Event"), generic_sub_event_ctor);
    define_class(realm, "PromiseRejectionEvent", Some("Event"), generic_sub_event_ctor);
    define_class(realm, "SecurityPolicyViolationEvent", Some("Event"), generic_sub_event_ctor);
    define_class(realm, "ToggleEvent", Some("Event"), generic_sub_event_ctor);
    let _ = ALL;
}
