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
        Ok(api) if api.is_object() => {
            let id = api.as_object().unwrap();
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

thread_local! {
    /// Some realm called into a same-origin frame's realm since the last
    /// microtask checkpoint (see [`checkpoint`]).
    static FOREIGN_CALLS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Microtask checkpoint of the realm running now: frames it called into
/// run theirs too (same-origin frames share one event loop).
pub(crate) fn checkpoint() {
    if FOREIGN_CALLS.with(|f| f.replace(false)) && crate::env::env_active() {
        with_env(|env| env.frame_checkpoint());
    }
}

/// Run `call` in the realm of same-origin frame `id` (None when the frame
/// cannot be scripted from this realm).
pub(crate) fn call_frame(id: u64, call: crate::FrameCall) -> Option<crate::FrameReply> {
    let r = with_env(|env| env.frame_call(id, call));
    if r.is_some() {
        FOREIGN_CALLS.with(|f| f.set(true));
    }
    r
}

/// A parent realm's request, in this (the frame's) realm.
pub(crate) fn foreign(realm: &mut Realm, call: crate::FrameCall) -> crate::FrameReply {
    use crate::{FrameCall, FrameReply};
    let r = match call {
        FrameCall::Dispatch { node, iface, ty, init } => {
            let n = crate::host::wrap(realm, crate::host::NodeRef::main(node));
            api_call(realm, "foreignDispatch", &[n, Value::str(&iface), Value::str(&ty), Value::str(&init)])
        }
        FrameCall::Method { node, name } => {
            let n = crate::host::wrap(realm, crate::host::NodeRef::main(node));
            realm.invoke(n, &name, &[]).map(|_| Value::Undefined)
        }
        FrameCall::GetGlobal { name } => api_call(realm, "foreignGet", &[Value::str(&name)]),
        FrameCall::CallGlobal { name, args } => api_call(realm, "foreignCall", &[Value::str(&name), Value::str(&args)]),
    };
    match r {
        Err(e) => FrameReply::Error(realm.error_to_string(&e)),
        Ok(v) => match v.v() {
            rabbithole_js::V::Bool(b) => FrameReply::Bool(b),
            rabbithole_js::V::Undefined => FrameReply::Undefined,
            rabbithole_js::V::String(s) => FrameReply::Value(s.to_rust_string()),
            rabbithole_js::V::Object(_) => match crate::host::node_of(realm, &v) {
                Some(n) if n.doc == 0 => FrameReply::Node(n.node),
                // The API returns a function marker as an empty array.
                _ => FrameReply::Function,
            },
            _ => FrameReply::Undefined,
        },
    }
}

/// A reply from another realm as a value of this one.
fn reply_value(realm: &mut Realm, id: u64, r: Option<crate::FrameReply>) -> JsResult<Value> {
    use crate::FrameReply;
    match r {
        None | Some(FrameReply::Undefined) => Ok(Value::Undefined),
        Some(FrameReply::Bool(b)) => Ok(Value::Bool(b)),
        Some(FrameReply::Node(node)) if id < crate::host::FRAME_DOC as u64 => Ok(crate::host::wrap(realm, crate::host::NodeRef { doc: crate::host::FRAME_DOC | id as u32, node })),
        Some(FrameReply::Node(_)) => Ok(Value::Null),
        Some(FrameReply::Function) => Ok(Value::str("\u{0}function")),
        Some(FrameReply::Value(s)) => Ok(Value::str(&s)),
        Some(FrameReply::Error(m)) => Err(realm.throw_type_error(&m)),
    }
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
        // Same-origin frames: `contentWindow[key]` and calls of its
        // functions run in the frame's realm.
        "global" => {
            let name = realm.to_rust_string(arg(args, 2))?;
            let r = call_frame(id, crate::FrameCall::GetGlobal { name });
            reply_value(realm, id, r)?
        }
        "call" => {
            let name = realm.to_rust_string(arg(args, 2))?;
            let a = realm.to_rust_string(arg(args, 3))?;
            let r = call_frame(id, crate::FrameCall::CallGlobal { name, args: a });
            reply_value(realm, id, r)?
        }
        "navigate" => {
            let url = realm.to_rust_string(arg(args, 2))?;
            with_env(|env| env.navigate_frame(id, &url));
            Value::Undefined
        }
        _ => Value::Undefined,
    })
}
