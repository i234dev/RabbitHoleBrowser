//! Nested browsing contexts (iframes) from the point of view of one realm.
//!
//! Every frame has its own realm; the engine keeps the frame tree and
//! routes messages. Here: `window.parent` / `top` / `length` / `frames[i]`,
//! `iframe.contentWindow`, and cross-realm `postMessage` (structured clone
//! through a serialized form), written mostly in `frames.js`.

use crate::env::with_env;
use crate::host::{arg, root_value, state};
use rabbithole_js::{JsResult, Realm, Value};

/// Install the frame accessors on the global. Call after `window::install`.
pub(crate) fn install(realm: &mut Realm) {
    let post = realm.new_native_function("post", 3, native_post);
    let info = realm.new_native_function("info", 3, native_info);
    let factory = match realm.eval(include_str!("frames.js"), "rabbithole:frames.js") {
        Ok(f) => f,
        Err(e) => {
            let msg = realm.error_to_string(&e);
            eprintln!("[rabbithole] frames.js failed: {msg}");
            return;
        }
    };
    match realm.call(factory, Value::Undefined, &[Value::Object(post), Value::Object(info)]) {
        Ok(api @ Value::Object(id)) => {
            root_value(realm, &api);
            state().borrow_mut().protos.insert("__frames_api", id);
        }
        Ok(_) => {}
        Err(e) => {
            let msg = realm.error_to_string(&e);
            eprintln!("[rabbithole] frames.js failed: {msg}");
        }
    }
    let name = with_env(|env| env.frame_name());
    if let Some(name) = name {
        let g = realm.global();
        let _ = realm.set_str(Value::Object(g), "name", Value::str(&name));
    }
}

/// The document of frame `id` as a node of this realm (same-origin
/// frames), or null.
pub(crate) fn frame_document_value(realm: &mut Realm, id: u64) -> Value {
    if id >= crate::host::FRAME_DOC as u64 || with_env(|env| env.frame_document(id).is_none()) {
        return Value::Null;
    }
    crate::host::wrap(realm, crate::host::NodeRef { doc: crate::host::FRAME_DOC | id as u32, node: rabbithole_dom::Document::ROOT })
}

fn api_call(realm: &mut Realm, method: &str, args: &[Value]) -> JsResult<Value> {
    let Some(api) = state().borrow().protos.get("__frames_api").copied() else {
        return Ok(Value::Null);
    };
    let f = realm.get_str(Value::Object(api), method)?;
    realm.call(f, Value::Object(api), args)
}

/// WindowProxy of the frame with engine id `id` (the global for this
/// realm's own frame), or null.
pub(crate) fn window_for(realm: &mut Realm, id: Option<u64>) -> JsResult<Value> {
    match id {
        Some(id) => api_call(realm, "windowFor", &[Value::Number(id as f64)]),
        None => Ok(Value::Null),
    }
}

/// Deliver a message posted by another frame: a `MessageEvent` on window.
pub(crate) fn deliver(realm: &mut Realm, data: &str, origin: &str, source: u64) -> JsResult<Value> {
    api_call(realm, "deliver", &[Value::str(data), Value::str(origin), Value::Number(source as f64)])
}

fn to_id(realm: &mut Realm, v: Value) -> JsResult<Option<u64>> {
    let n = realm.to_number(v)?;
    Ok(if n.is_finite() && n >= 0.0 { Some(n as u64) } else { None })
}

fn id_value(id: Option<u64>) -> Value {
    Value::Number(id.map(|i| i as f64).unwrap_or(-1.0))
}

/// `post(targetId, serializedData, targetOrigin)`.
fn native_post(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(target) = to_id(realm, arg(args, 0))? else { return Ok(Value::Undefined) };
    let data = realm.to_rust_string(arg(args, 1))?;
    let origin = realm.to_rust_string(arg(args, 2))?;
    with_env(|env| env.post_message(target, data, origin));
    Ok(Value::Undefined)
}

/// `info(id, what, index)`: frame tree queries (`id` < 0 = this frame).
fn native_info(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = match to_id(realm, arg(args, 0))? {
        Some(i) => i,
        None => with_env(|env| env.frame_id()),
    };
    let what = realm.to_rust_string(arg(args, 1))?;
    Ok(match what.as_str() {
        "self" => Value::Number(id as f64),
        "parent" => id_value(with_env(|env| env.frame_parent(id))),
        "top" => {
            let mut cur = id;
            let mut guard = 0;
            while let Some(p) = with_env(|env| env.frame_parent(cur)) {
                cur = p;
                guard += 1;
                if guard > 32 {
                    break;
                }
            }
            Value::Number(cur as f64)
        }
        "length" => Value::Number(with_env(|env| env.frame_children(id)).len() as f64),
        "child" => {
            let i = realm.to_number(arg(args, 2))?;
            let kids = with_env(|env| env.frame_children(id));
            id_value(if i >= 0.0 { kids.get(i as usize).copied() } else { None })
        }
        "closed" => Value::Bool(!with_env(|env| env.frame_exists(id))),
        "href" => Value::str(&with_env(|env| env.frame_url(id)).unwrap_or_default()),
        "document" => frame_document_value(realm, id),
        "name" => match with_env(|env| env.frame_name_of(id)) {
            Some(n) => Value::str(&n),
            None => Value::Null,
        },
        "named" => {
            let name = realm.to_rust_string(arg(args, 2))?;
            id_value(with_env(|env| env.frame_named_child(id, &name)))
        }
        "navigate" => {
            let url = realm.to_rust_string(arg(args, 2))?;
            with_env(|env| env.navigate_frame(id, &url));
            Value::Undefined
        }
        _ => Value::Undefined,
    })
}
