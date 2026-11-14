//! Top-level browsing contexts. The embedder owns their real lifetime;
//! script bindings never manufacture a permanently-open window handle.

use std::sync::Arc;
use rabbithole_net::Url;
use crate::env::with_env;
use crate::host::{arg, root_value, state, unroot_value, TargetKey};
use rabbithole_js::{JsResult, Realm, Value, V, ALL, CONFIGURABLE, ENUMERABLE};

#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub url: Url,
    pub base_url: Url,
    /// Serialized origin ("null" for an opaque origin).
    pub origin: String,
    /// Security identity. Opaque origins must have unique keys; an initial
    /// about:blank inherits the creator's key as well as its serialized origin.
    pub origin_key: String,
    pub opener: Option<u64>,
    pub closed: bool,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct WindowOpenRequest {
    pub source: u64,
    pub source_origin: String,
    pub source_url: Url,
    pub source_base_url: Url,
    /// None leaves the initial blank document in place.
    pub url: Option<Url>,
    pub name: String,
    pub noopener: bool,
    pub noreferrer: bool,
}

#[derive(Debug, Clone)]
pub struct WindowMessage {
    pub source: u64,
    /// Structured-clone serialization, never a reference into another heap.
    pub data: String,
    /// Sender's serialized origin captured when postMessage was called.
    pub origin: String,
    /// None means '*'; otherwise the destination's identity must still match.
    pub target_origin_key: Option<String>,
}

/// Thread-safe transport to the browser's context owner. Commands must
/// preserve source order, including open followed immediately by navigate
/// or close before the new content thread has started.
pub trait WindowHost: Send + Sync {
    /// Reserve a real browsing context and queue its creation. None means
    /// blocked or unsupported. Reuse a named context only when permitted.
    fn open(&self, request: WindowOpenRequest) -> Option<u64>;
    fn info(&self, id: u64) -> Option<WindowInfo>;
    /// Revalidate navigation permission at the owner, including related
    /// context relationships. Returns false for closed/forbidden targets.
    fn navigate(&self, source: u64, target: u64, url: Url, replace: bool) -> bool;
    fn focus(&self, source: u64, target: u64) -> bool;
    /// Only contexts allowed to be script-closed may be closed this way.
    fn close(&self, source: u64, target: u64) -> bool;
    fn set_name(&self, source: u64, target: u64, name: String) -> bool;
    fn post_message(&self, source: u64, target: u64, data: String, target_origin_key: Option<String>) -> bool;
    fn disown(&self, source: u64);
    /// Called before the new top-level document executes scripts. Preserve
    /// inherited blank origin; assign fresh identities to new opaque origins.
    fn document_changed(&self, id: u64, url: Url, origin: String);
}

#[derive(Clone)]
pub struct WindowContext {
    pub id: u64,
    pub host: Arc<dyn WindowHost>,
}

impl std::fmt::Debug for WindowContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowContext").field("id", &self.id).finish_non_exhaustive()
    }
}

fn api_call(realm: &mut Realm, method: &str, args: &[Value]) -> JsResult<Value> {
    let Some(api) = state().borrow().protos.get("__windows_api").copied() else { return Ok(Value::Null) };
    let function = realm.get_str(Value::Object(api), method)?;
    realm.call(function, Value::Object(api), args)
}

fn window_for(realm: &mut Realm, id: u64) -> JsResult<Value> {
    api_call(realm, "windowFor", &[Value::Number(id as f64)])
}

pub(crate) fn install(realm: &mut Realm) {
    let native = realm.new_native_function("windowContext", 4, native_info);
    let native = Value::Object(native);
    root_value(realm, &native);
    let clone = state().borrow().protos.get("__frames_api").copied().map(Value::Object).unwrap_or(Value::Null);
    let result = realm.eval(include_str!("windows.js"), "rabbithole:windows.js").and_then(|factory| realm.call(factory, Value::Undefined, &[native.clone(), clone]));
    unroot_value(realm, &native);
    match result {
        Ok(api) if api.is_object() => {
            root_value(realm, &api);
            state().borrow_mut().protos.insert("__windows_api", api.as_object().unwrap());
        }
        Ok(_) => return,
        Err(error) => { let message = realm.error_to_string(&error); crate::host::log("error", &format!("WindowProxy setup failed: {message}")); return; }
    }
    // Headless hosts and iframe realms retain their existing global name,
    // focus and opener semantics; this transport owns top-level GUI contexts.
    if with_env(|env| env.window_context()).is_none() { return; }
    let global = realm.global();
    let attributes = CONFIGURABLE | ENUMERABLE;
    realm.define_accessor(global, "closed", Some(own_closed), None, attributes);
    realm.define_accessor(global, "opener", Some(own_opener), Some(set_own_opener), attributes);
    realm.define_accessor(global, "name", Some(own_name), Some(set_own_name), attributes);
    realm.define_method(global, "focus", 0, own_focus);
    realm.define_method(global, "close", 0, own_close);
}

fn optional_string(realm: &mut Realm, args: &[Value], index: usize, default: &str) -> JsResult<String> {
    match args.get(index).map(Value::v) {
        None | Some(V::Undefined) => Ok(default.to_string()),
        _ => realm.to_rust_string(arg(args, index)),
    }
}

fn feature(features: &str, name: &str) -> bool {
    let mut enabled = false;
    let bytes = features.as_bytes();
    let mut index = 0;
    let separator = |byte: u8| byte == b',' || byte == b'=' || byte.is_ascii_whitespace();
    while index < bytes.len() {
        while index < bytes.len() && separator(bytes[index]) { index += 1; }
        let start = index;
        while index < bytes.len() && !separator(bytes[index]) { index += 1; }
        let key = &features[start..index];
        while index < bytes.len() && bytes[index].is_ascii_whitespace() { index += 1; }
        let mut value = "";
        if index < bytes.len() && bytes[index] == b'=' {
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_whitespace() { index += 1; }
            let start = index;
            while index < bytes.len() && !separator(bytes[index]) { index += 1; }
            value = &features[start..index];
        }
        if key.eq_ignore_ascii_case(name) {
            let integer = value.strip_prefix('+').or_else(|| value.strip_prefix('-')).unwrap_or(value);
            let nonzero = integer.bytes().take_while(u8::is_ascii_digit).any(|byte| byte != b'0');
            enabled = value.is_empty() || value.eq_ignore_ascii_case("yes") || value.eq_ignore_ascii_case("true") || nonzero;
        }
    }
    enabled
}

pub(crate) fn open(realm: &mut Realm, args: &[Value]) -> JsResult<Value> {
    let raw = optional_string(realm, args, 0, "")?;
    let mut name = optional_string(realm, args, 1, "_blank")?;
    if name.is_empty() { name = "_blank".to_string(); }
    let features = optional_string(realm, args, 2, "")?;
    let url = if raw.is_empty() { None } else {
        Some(crate::host::resolve_url(&raw).ok_or_else(|| crate::misc::dom_exception(realm, "SyntaxError", "Invalid URL passed to window.open."))?)
    };
    if matches!(name.to_ascii_lowercase().as_str(), "_self" | "_parent" | "_top") {
        let own = with_env(|env| env.frame_id());
        let mut target = own;
        if !name.eq_ignore_ascii_case("_self") {
            for _ in 0..32 {
                let Some(parent) = with_env(|env| env.frame_parent(target)) else { break };
                target = parent;
                if name.eq_ignore_ascii_case("_parent") { break; }
            }
        }
        if let Some(url) = url {
            with_env(|env| if target == own { env.navigate(url, false); } else { env.navigate_frame(target, &url.to_string()); });
        }
        return crate::frames::window_for(realm, Some(target));
    }
    let Some(context) = with_env(|env| env.window_context()) else {
        if let Some(url) = url { with_env(|env| env.open_window(url)); }
        return Ok(Value::Null);
    };
    let noreferrer = feature(&features, "noreferrer");
    let noopener = noreferrer || feature(&features, "noopener");
    let request = with_env(|env| WindowOpenRequest {
        source: context.id, source_origin: env.security_origin(), source_url: env.document_url(),
        source_base_url: env.base_url(), url, name, noopener, noreferrer,
    });
    match context.host.open(request) {
        Some(id) if !noopener => window_for(realm, id),
        _ => Ok(Value::Null),
    }
}

fn own_closed(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(with_env(|env| env.window_context()).map(|c| c.host.info(c.id).map(|i| i.closed).unwrap_or(true)).unwrap_or(false)))
}

fn own_opener(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let opener = with_env(|env| env.window_context()).and_then(|c| c.host.info(c.id)).and_then(|info| info.opener);
    match opener { Some(id) => window_for(realm, id), None => Ok(Value::Null) }
}

fn set_own_opener(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let value = arg(args, 0);
    if value.is_null() {
        if let Some(context) = with_env(|env| env.window_context()) { context.host.disown(context.id); }
    } else {
        let global = realm.global();
        realm.define_property(global, "opener", value, ALL);
    }
    Ok(Value::Undefined)
}

fn own_name(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::str(&with_env(|env| env.window_context()).and_then(|c| c.host.info(c.id)).map(|i| i.name).unwrap_or_default()))
}

fn set_own_name(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let name = realm.to_rust_string(arg(args, 0))?;
    if let Some(context) = with_env(|env| env.window_context()) { context.host.set_name(context.id, context.id, name); }
    Ok(Value::Undefined)
}

fn own_focus(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    if let Some(context) = with_env(|env| env.window_context()) { context.host.focus(context.id, context.id); }
    Ok(Value::Undefined)
}

fn own_close(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    if let Some(context) = with_env(|env| env.window_context()) { context.host.close(context.id, context.id); }
    Ok(Value::Undefined)
}

fn denied(realm: &mut Realm) -> Value { crate::misc::dom_exception(realm, "SecurityError", "Cross-origin WindowProxy access is denied.") }

fn native_info(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(context) = with_env(|env| env.window_context()) else { return Ok(Value::Null) };
    let raw = realm.to_number(arg(args, 0))?;
    let id = if raw < 0.0 { context.id } else if raw.is_finite() && raw.fract() == 0.0 && raw <= 9007199254740991.0 { raw as u64 } else { return Err(realm.throw_type_error("Invalid window context")); };
    let operation = realm.to_rust_string(arg(args, 1))?;
    if operation == "self" { return Ok(Value::Number(context.id as f64)); }
    let target = context.host.info(id);
    if operation == "closed" { return Ok(Value::Bool(target.as_ref().map(|i| i.closed).unwrap_or(true))); }
    let same = match (context.host.info(context.id), target.as_ref()) {
        (Some(source), Some(target)) => !source.closed && !target.closed && !source.origin_key.is_empty() && source.origin_key == target.origin_key,
        _ => false,
    };
    if operation == "same" { return Ok(Value::Bool(same)); }
    match operation.as_str() {
        "href" | "name" | "setName" | "document" | "unknown" | "assign" if !same => return Err(denied(realm)),
        _ => {}
    }
    match operation.as_str() {
        "href" => Ok(Value::str(&target.map(|i| i.url.to_string()).unwrap_or_default())),
        "name" => Ok(Value::str(&target.map(|i| i.name).unwrap_or_default())),
        "opener" => Ok(target.and_then(|i| i.opener).map(|i| Value::Number(i as f64)).unwrap_or(Value::Null)),
        "document" => Err(crate::misc::dom_exception(realm, "NotSupportedError", "Synchronous DOM access to another top-level context is not implemented.")),
        "setName" => { let value = realm.to_rust_string(arg(args, 2))?; context.host.set_name(context.id, id, value); Ok(Value::Undefined) }
        "focus" => { context.host.focus(context.id, id); Ok(Value::Undefined) }
        "close" => { context.host.close(context.id, id); Ok(Value::Undefined) }
        "navigate" | "assign" | "replace" => {
            let value = realm.to_rust_string(arg(args, 2))?;
            let url = crate::host::resolve_url(&value).ok_or_else(|| crate::misc::dom_exception(realm, "SyntaxError", "Invalid window location."))?;
            if !context.host.navigate(context.id, id, url, operation == "replace") { return Err(denied(realm)); }
            Ok(Value::Undefined)
        }
        "post" => {
            let data = realm.to_rust_string(arg(args, 2))?;
            let target_origin = realm.to_rust_string(arg(args, 3))?;
            let key = match target_origin.as_str() {
                "*" => None,
                "/" => context.host.info(context.id).map(|i| i.origin_key),
                _ => {
                    let url = Url::parse(&target_origin).ok_or_else(|| crate::misc::dom_exception(realm, "SyntaxError", "Invalid target origin."))?;
                    Some(url.origin())
                }
            };
            // A missing sender must never turn default '/' into wildcard.
            if target_origin != "/" || key.is_some() { context.host.post_message(context.id, id, data, key); }
            Ok(Value::Undefined)
        }
        _ => Ok(Value::Undefined),
    }
}

pub(crate) fn deliver(realm: &mut Realm, message: &WindowMessage) -> JsResult<()> {
    let data = api_call(realm, "deserialize", &[Value::str(&message.data)])?;
    root_value(realm, &data);
    let source = match window_for(realm, message.source) { Ok(source) => source, Err(error) => { unroot_value(realm, &data); return Err(error); } };
    root_value(realm, &source);
    let ports = realm.new_array_value(Vec::new());
    root_value(realm, &ports);
    let id = crate::events::create_event(realm, "MessageEvent", "message", &crate::EventInit::simple(false, false), true);
    for (name, value) in [("data", data.clone()), ("source", source.clone()), ("origin", Value::str(&message.origin)), ("ports", ports.clone())] {
        realm.define_property(id, name, value, CONFIGURABLE | ENUMERABLE);
    }
    crate::events::dispatch_object(realm, TargetKey::Window, id, None);
    unroot_value(realm, &ports);
    unroot_value(realm, &source);
    unroot_value(realm, &data);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::feature;

    #[test]
    fn popup_features_preserve_boolean_values_and_separators() {
        for features in ["noopener", "NOOPENER=yes", "noopener=true", "noopener = -2", "noopener=1suffix"] {
            assert!(feature(features, "noopener"), "{features}");
        }
        for features in ["", "width=500", "noopener=0", "noopener = no", "noopener=false", "noopener=1,noopener=0"] {
            assert!(!feature(features, "noopener"), "{features}");
        }
        assert!(feature("width=500, noreferrer = yes, noopener=0", "noreferrer"));
        assert!(!feature("width=500, noreferrer = yes, noopener=0", "noopener"));
    }
}
