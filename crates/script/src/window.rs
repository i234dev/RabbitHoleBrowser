//! The `window` global: location, history, navigator, screen, timers,
//! storage, console routing, scrolling, dialogs, and the remaining globals.

use crate::env::with_env;
use rabbithole_js::V;
use crate::host::*;
use rabbithole_js::{JsResult, ObjectId, PropertyKey, Realm, Value, ALL, CONFIGURABLE, ENUMERABLE, HIDDEN};
use rabbithole_net::Url;

const ACC: u8 = CONFIGURABLE | ENUMERABLE;

/// The current location href (pushState override or the document URL).
pub(crate) fn current_href() -> String {
    if let Some(o) = state().borrow().location_override.clone() {
        return o;
    }
    with_env(|env| env.document_url().to_string())
}

fn current_url() -> Url {
    Url::parse_absolute(&current_href()).unwrap_or_else(|| with_env(|env| env.document_url()))
}

// ------------------------------------------------------------- Location

fn loc_get(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let which = native_data::<String>(realm).expect("component");
    let u = current_url();
    Ok(Value::str(&crate::fetch::url_component(&u, &which)))
}

fn loc_set(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let which = native_data::<String>(realm).expect("component");
    let v = arg_str(realm, args, 0)?;
    let mut u = current_url();
    match which.as_str() {
        "href" => {
            match base_url().join(&v) {
                Some(nu) => navigate_to(nu, false),
                None => return Err(crate::misc::dom_exception(realm, "SyntaxError", &format!("Failed to set the 'href' property on 'Location': '{v}' is not a valid URL."))),
            }
            return Ok(Value::Undefined);
        }
        "hash" => {
            // Location's hash setter: an empty fragment, then the parser's
            // fragment state (percent-encoding) on the input.
            let input = v.strip_prefix('#').unwrap_or(&v);
            let mut nu = u.clone();
            if input.is_empty() {
                nu.fragment = Some(String::new());
            } else {
                nu.set_hash(&format!("#{input}"));
            }
            // Setting the current fragment again does nothing (no
            // navigation, no `hashchange`).
            if nu.fragment.as_deref().unwrap_or("") == u.fragment.as_deref().unwrap_or("") {
                return Ok(Value::Undefined);
            }
            u = nu;
            // Same-document navigation: update the override and fire events.
            let old = current_href();
            state().borrow_mut().location_override = Some(u.to_string());
            if let Some(frag) = &u.fragment {
                scroll_to_fragment(frag);
            }
            let new = u.to_string();
            fire_hashchange(realm, &old, &new);
            return Ok(Value::Undefined);
        }
        // The URL setters (URL Standard state overrides).
        "search" => u.set_search(&v),
        "pathname" => u.set_pathname(&v),
        "protocol" => u.set_protocol(&v),
        "host" => u.set_host(&v),
        "hostname" => u.set_hostname(&v),
        "port" => u.set_port(&v),
        _ => {}
    }
    navigate_to(u, false);
    Ok(Value::Undefined)
}

fn scroll_to_fragment(frag: &str) {
    with_env(|env| {
        let doc = env.doc();
        let target = doc.element_by_id(frag);
        if let Some(t) = target {
            env.ensure_layout();
            if let Some(r) = env.node_rect(t) {
                env.set_scroll(0.0, r.y.max(0.0));
            }
        }
    });
}

/// Queue `hashchange` (a task: listeners run after the script that changed
/// the fragment, like in browsers; a handler setting the hash again must
/// not recurse).
fn fire_hashchange(realm: &mut Realm, old: &str, new: &str) {
    let init = crate::EventInit::simple(false, false);
    let id = crate::events::create_event(realm, "HashChangeEvent", "hashchange", &init, true);
    realm.define_property(id, "oldURL", Value::str(old), ACC);
    realm.define_property(id, "newURL", Value::str(new), ACC);
    let f = realm.new_native_function("hashchange", 1, deliver_hashchange);
    add_timer(realm, Value::Object(f), 0.0, vec![Value::Object(id)], false);
}

fn deliver_hashchange(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if let V::Object(id) = arg(args, 0).v() {
        crate::events::dispatch_object(realm, TargetKey::Window, id, None);
    }
    Ok(Value::Undefined)
}

pub(crate) fn navigate_to(u: Url, replace: bool) {
    with_env(|env| env.navigate(u, replace));
}

fn loc_assign(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let v = arg_str(realm, args, 0)?;
    if let Some(u) = current_url().join(&v) {
        navigate_to(u, false);
    }
    Ok(Value::Undefined)
}

fn loc_replace(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let v = arg_str(realm, args, 0)?;
    if let Some(u) = current_url().join(&v) {
        navigate_to(u, true);
    }
    Ok(Value::Undefined)
}

fn loc_reload(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let u = current_url();
    navigate_to(u, true);
    Ok(Value::Undefined)
}

fn loc_to_string(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::str(&current_href()))
}

fn loc_ancestor_origins(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(realm.new_array_value(vec![]))
}

pub(crate) fn location_getter(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let l = state().borrow().protos.get("__location").copied();
    Ok(l.map(Value::Object).unwrap_or(Value::Undefined))
}

fn location_setter(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let v = arg_str(realm, args, 0)?;
    if let Some(u) = current_url().join(&v) {
        navigate_to(u, false);
    }
    Ok(Value::Undefined)
}

fn make_location(realm: &mut Realm) -> ObjectId {
    let p = define_class(realm, "Location", None, illegal_ctor);
    let o = realm.new_object_with_proto(Some(p));
    realm.set_class_name(o, "Location");
    for comp in ["href", "protocol", "host", "hostname", "port", "pathname", "search", "hash", "origin"] {
        let data: std::rc::Rc<dyn std::any::Any> = std::rc::Rc::new(comp.to_string());
        let g = realm.new_native_function_full(&format!("get {comp}"), 0, loc_get, Some(data.clone()), vec![], false);
        let s = if comp == "origin" { None } else { Some(Value::Object(realm.new_native_function_full(&format!("set {comp}"), 1, loc_set, Some(data), vec![], false))) };
        realm.define_accessor_values(o, comp, Some(Value::Object(g)), s, ACC);
    }
    realm.define_method(o, "assign", 1, loc_assign);
    realm.define_method(o, "replace", 1, loc_replace);
    realm.define_method(o, "reload", 0, loc_reload);
    realm.define_method(o, "toString", 0, loc_to_string);
    realm.define_accessor(o, "ancestorOrigins", Some(loc_ancestor_origins), None, ACC);
    realm.extra_roots.push(Value::Object(o));
    state().borrow_mut().protos.insert("__location", o);
    o
}

// -------------------------------------------------------------- History

fn hist_length(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Number(state().borrow().history_len as f64))
}

fn hist_state(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(state().borrow().history_state.clone())
}

fn hist_push(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    hist_update(realm, args, true)
}

fn hist_replace(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    hist_update(realm, args, false)
}

fn hist_update(realm: &mut Realm, args: &[Value], push: bool) -> JsResult<Value> {
    let st_val = arg(args, 0);
    let url = arg_str_opt(realm, args, 2)?;
    root_value(realm, &st_val);
    {
        let st = state();
        let mut st = st.borrow_mut();
        st.history_state = st_val;
        if push {
            st.history_len += 1;
        }
    }
    if let Some(u) = url {
        if let Some(nu) = current_url().join(&u) {
            state().borrow_mut().location_override = Some(nu.to_string());
        }
    }
    Ok(Value::Undefined)
}

fn hist_go(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    // Traversal is the embedder's job; logged for visibility.
    log("info", "history navigation requested (not supported from script)");
    Ok(Value::Undefined)
}

fn make_history(realm: &mut Realm) -> ObjectId {
    let p = define_class(realm, "History", None, illegal_ctor);
    let o = realm.new_object_with_proto(Some(p));
    realm.define_accessor(o, "length", Some(hist_length), None, ACC);
    realm.define_accessor(o, "state", Some(hist_state), None, ACC);
    realm.define_property(o, "scrollRestoration", Value::str("auto"), ALL);
    realm.define_method(o, "pushState", 3, hist_push);
    realm.define_method(o, "replaceState", 3, hist_replace);
    realm.define_method(o, "back", 0, hist_go);
    realm.define_method(o, "forward", 0, hist_go);
    realm.define_method(o, "go", 1, hist_go);
    realm.extra_roots.push(Value::Object(o));
    o
}

// ------------------------------------------------------------- Navigator

fn nav_user_agent(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::str(&with_env(|env| env.user_agent())))
}

/// `navigator.appVersion`: the User-Agent without its `Mozilla/` prefix.
fn nav_app_version(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let ua = with_env(|env| env.user_agent());
    Ok(Value::str(ua.strip_prefix("Mozilla/").unwrap_or(&ua)))
}

/// `navigator.vendor`: what Chromium-based engines report with a
/// Chromium-compatible User-Agent, empty otherwise.
fn nav_vendor(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let ua = with_env(|env| env.user_agent());
    Ok(Value::str(if chromium_major(&ua).is_some() { "Google Inc." } else { "" }))
}

fn nav_send_beacon(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    // Fire and forget.
    let url = arg_str(realm, args, 0)?;
    let Some(u) = resolve_url(&url) else {
        return Err(realm.throw_type_error(&format!("Failed to execute 'sendBeacon' on 'Navigator': Invalid URL '{url}'")));
    };
    {
        let _ = with_env(|env| env.fetch(rabbithole_net::Request::post(u, rabbithole_net::Body::text("text/plain", "")).with_dest(rabbithole_net::Destination::Empty).with_referer(env.document_url())));
    }
    Ok(Value::Bool(true))
}

fn resolved_undefined(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let (p, resolve, _) = realm.new_promise();
    realm.call(Value::Object(resolve), Value::Undefined, &[Value::Undefined])?;
    Ok(Value::Object(p))
}

fn resolved_empty_string(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let (p, resolve, _) = realm.new_promise();
    realm.call(Value::Object(resolve), Value::Undefined, &[Value::str("")])?;
    Ok(Value::Object(p))
}

fn nav_languages(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(realm.new_array_value(vec![Value::str("en-US"), Value::str("en")]))
}

fn nav_get_battery(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let b = object_from(realm, vec![("charging", Value::Bool(true)), ("level", Value::Number(1.0)), ("chargingTime", Value::Number(0.0)), ("dischargingTime", Value::Number(f64::INFINITY))]);
    let (p, resolve, _) = realm.new_promise();
    realm.call(Value::Object(resolve), Value::Undefined, &[b])?;
    Ok(Value::Object(p))
}

/// Major version of a Chromium-based User-Agent (`Chrome/142.0…`), if the
/// User-Agent is one.
fn chromium_major(ua: &str) -> Option<String> {
    let rest = &ua[ua.find("Chrome/")? + 7..];
    let major: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    (!major.is_empty()).then_some(major)
}

fn ua_platform() -> &'static str {
    if cfg!(windows) {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else {
        "Linux"
    }
}

/// `[{brand, version}]` in the order of the `Sec-CH-UA` request header.
fn ua_brands(realm: &mut Realm, major: &str, full: bool) -> Value {
    let v = if full { format!("{major}.0.0.0") } else { major.to_string() };
    let nb = if full { "99.0.0.0" } else { "99" };
    let mut list = Vec::new();
    for (b, ver) in [("Chromium", v.as_str()), ("Google Chrome", v.as_str()), ("Not_A Brand", nb)] {
        let o = object_from(realm, vec![("brand", Value::str(b)), ("version", Value::str(ver))]);
        list.push(o);
    }
    realm.new_array_value(list)
}

fn current_ua_major() -> String {
    chromium_major(&with_env(|env| env.user_agent())).unwrap_or_default()
}

/// `navigator.userAgentData` (User-Agent Client Hints): present when the
/// User-Agent is the Chromium-compatible one (it then matches the
/// `Sec-CH-UA*` headers sent with requests), absent for the bot identity.
fn nav_user_agent_data(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    if chromium_major(&with_env(|env| env.user_agent())).is_none() {
        return Ok(Value::Undefined);
    }
    let data = realm.current_native().and_then(|n| realm.native_captures(n).first().cloned()).unwrap_or(Value::Undefined);
    Ok(data)
}

fn uad_brands(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let major = current_ua_major();
    Ok(ua_brands(realm, &major, false))
}

fn uad_mobile(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(false))
}

fn uad_platform(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::str(ua_platform()))
}

fn uad_low_entropy(realm: &mut Realm) -> Vec<(&'static str, Value)> {
    let major = current_ua_major();
    let brands = ua_brands(realm, &major, false);
    vec![("brands", brands), ("mobile", Value::Bool(false)), ("platform", Value::str(ua_platform()))]
}

fn uad_to_json(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let pairs = uad_low_entropy(realm);
    Ok(object_from(realm, pairs))
}

/// `getHighEntropyValues(hints)`: the low-entropy values plus each
/// requested hint we know.
fn uad_high_entropy(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let mut hints: Vec<String> = Vec::new();
    if let Some(V::Object(arr)) = args.first().map(Value::v) {
        let len = realm.get_str(Value::Object(arr), "length")?;
        let n = realm.to_number(len)? as usize;
        for i in 0..n.min(64) {
            let v = realm.get(Value::Object(arr), PropertyKey::index(i as u32))?;
            hints.push(realm.to_rust_string(v)?);
        }
    }
    let major = current_ua_major();
    let mut pairs = uad_low_entropy(realm);
    for h in &hints {
        let v = match h.as_str() {
            "architecture" => Value::str(if cfg!(target_arch = "aarch64") { "arm" } else { "x86" }),
            "bitness" => Value::str(if cfg!(target_pointer_width = "64") { "64" } else { "32" }),
            "model" => Value::str(""),
            "platformVersion" => Value::str(if cfg!(windows) { "10.0.0" } else if cfg!(target_os = "macos") { "10.15.7" } else { "" }),
            "uaFullVersion" => Value::str(&format!("{major}.0.0.0")),
            "fullVersionList" => ua_brands(realm, &major, true),
            "wow64" => Value::Bool(false),
            "formFactors" => {
                let d = Value::str("Desktop");
                realm.new_array_value(vec![d])
            }
            _ => continue,
        };
        let key: &'static str = match h.as_str() {
            "architecture" => "architecture",
            "bitness" => "bitness",
            "model" => "model",
            "platformVersion" => "platformVersion",
            "uaFullVersion" => "uaFullVersion",
            "fullVersionList" => "fullVersionList",
            "wow64" => "wow64",
            _ => "formFactors",
        };
        pairs.push((key, v));
    }
    let o = object_from(realm, pairs);
    let (p, resolve, _) = realm.new_promise();
    realm.call(Value::Object(resolve), Value::Undefined, &[o])?;
    Ok(Value::Object(p))
}

fn make_navigator(realm: &mut Realm) -> ObjectId {
    let p = define_class(realm, "Navigator", None, illegal_ctor);
    let o = realm.new_object_with_proto(Some(p));
    realm.define_accessor(o, "userAgent", Some(nav_user_agent), None, ACC);
    realm.define_accessor(o, "appVersion", Some(nav_app_version), None, ACC);
    realm.define_accessor(o, "vendor", Some(nav_vendor), None, ACC);
    realm.define_accessor(o, "languages", Some(nav_languages), None, ACC);
    for (k, v) in [
        ("language", Value::str("en-US")),
        ("platform", Value::str(if cfg!(windows) { "Win32" } else if cfg!(target_os = "macos") { "MacIntel" } else { "Linux x86_64" })),
        ("appName", Value::str("Netscape")),
        ("appCodeName", Value::str("Mozilla")),
        ("product", Value::str("Gecko")),
        ("productSub", Value::str("20030107")),
        ("vendorSub", Value::str("")),
        ("onLine", Value::Bool(true)),
        ("cookieEnabled", Value::Bool(true)),
        ("hardwareConcurrency", Value::Number(4.0)),
        ("maxTouchPoints", Value::Number(0.0)),
        ("doNotTrack", Value::Null),
        ("pdfViewerEnabled", Value::Bool(false)),
        ("webdriver", Value::Bool(false)),
        ("deviceMemory", Value::Number(8.0)),
    ] {
        realm.define_property(o, k, v, ACC);
    }
    let uad_proto = define_class(realm, "NavigatorUAData", None, illegal_ctor);
    realm.define_accessor(uad_proto, "brands", Some(uad_brands), None, ACC);
    realm.define_accessor(uad_proto, "mobile", Some(uad_mobile), None, ACC);
    realm.define_accessor(uad_proto, "platform", Some(uad_platform), None, ACC);
    realm.define_method(uad_proto, "getHighEntropyValues", 1, uad_high_entropy);
    realm.define_method(uad_proto, "toJSON", 0, uad_to_json);
    let uad = realm.new_object_with_proto(Some(uad_proto));
    let getter = realm.new_native_function_full("get userAgentData", 0, nav_user_agent_data, None, vec![Value::Object(uad)], false);
    realm.define_accessor_values(o, "userAgentData", Some(Value::Object(getter)), None, ACC);
    let plugins = realm.new_array_value(vec![]);
    realm.define_property(o, "plugins", plugins.clone(), ACC);
    realm.define_property(o, "mimeTypes", plugins, ACC);
    let clipboard = realm.new_object();
    realm.define_method(clipboard, "writeText", 1, resolved_undefined);
    realm.define_method(clipboard, "readText", 0, resolved_empty_string);
    realm.define_property(o, "clipboard", Value::Object(clipboard), ACC);
    let perms = realm.new_object();
    realm.define_method(perms, "query", 1, permissions_query);
    realm.define_property(o, "permissions", Value::Object(perms), ACC);
    let conn = object_from(realm, vec![("effectiveType", Value::str("4g")), ("downlink", Value::Number(10.0)), ("rtt", Value::Number(50.0)), ("saveData", Value::Bool(false))]);
    realm.define_property(o, "connection", conn, ACC);
    let sw = realm.new_object();
    realm.define_method(sw, "register", 1, rejected_not_supported);
    realm.define_method(sw, "getRegistration", 0, resolved_undefined);
    realm.define_method(sw, "getRegistrations", 0, resolved_empty_array);
    realm.define_method(sw, "addEventListener", 2, noop);
    realm.define_property(sw, "controller", Value::Null, ACC);
    let (ready, _, _) = realm.new_promise();
    realm.define_property(sw, "ready", Value::Object(ready), ACC);
    realm.define_property(o, "serviceWorker", Value::Object(sw), ACC);
    let md = realm.new_object();
    realm.define_method(md, "enumerateDevices", 0, resolved_empty_array);
    realm.define_method(md, "getUserMedia", 1, rejected_not_supported);
    realm.define_property(o, "mediaDevices", Value::Object(md), ACC);
    let geo = realm.new_object();
    realm.define_method(geo, "getCurrentPosition", 1, noop);
    realm.define_method(geo, "watchPosition", 1, noop);
    realm.define_method(geo, "clearWatch", 1, noop);
    realm.define_property(o, "geolocation", Value::Object(geo), ACC);
    let storage = realm.new_object();
    realm.define_method(storage, "estimate", 0, resolved_undefined);
    realm.define_property(o, "storage", Value::Object(storage), ACC);
    realm.define_method(o, "sendBeacon", 2, nav_send_beacon);
    realm.define_method(o, "javaEnabled", 0, ret_false);
    realm.define_method(o, "vibrate", 1, ret_false);
    realm.define_method(o, "getBattery", 0, nav_get_battery);
    realm.define_method(o, "registerProtocolHandler", 2, noop);
    realm.define_method(o, "share", 1, rejected_not_supported);
    realm.define_method(o, "canShare", 1, ret_false);
    realm.define_method(o, "requestMediaKeySystemAccess", 2, rejected_not_supported);
    realm.extra_roots.push(Value::Object(o));
    o
}

fn permissions_query(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let status = object_from(realm, vec![("state", Value::str("denied")), ("onchange", Value::Null)]);
    if let V::Object(s) = status.v() {
        realm.define_method(s, "addEventListener", 2, noop);
    }
    let (p, resolve, _) = realm.new_promise();
    realm.call(Value::Object(resolve), Value::Undefined, &[status])?;
    Ok(Value::Object(p))
}

fn rejected_not_supported(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let (p, _, reject) = realm.new_promise();
    let e = crate::misc::dom_exception(realm, "NotSupportedError", "Not supported");
    realm.call(Value::Object(reject), Value::Undefined, &[e])?;
    Ok(Value::Object(p))
}

fn resolved_empty_array(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let (p, resolve, _) = realm.new_promise();
    let a = realm.new_array_value(vec![]);
    realm.call(Value::Object(resolve), Value::Undefined, &[a])?;
    Ok(Value::Object(p))
}

fn noop(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Undefined)
}

fn ret_false(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(false))
}

fn ret_true(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(true))
}

// ------------------------------------------------------- screen / viewport

/// Marks the `screen` object (brand check of the Screen attributes).
struct ScreenData;

fn this_screen(realm: &mut Realm, this: &Value) -> JsResult<()> {
    match this.v() {
        V::Object(o) if realm.host_data::<ScreenData>(o).is_some() => Ok(()),
        _ => Err(realm.throw_type_error("Illegal invocation")),
    }
}

fn viewport_width(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Number(with_env(|env| env.viewport().0) as f64))
}

fn viewport_height(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Number(with_env(|env| env.viewport().1) as f64))
}

fn screen_width(realm: &mut Realm, this: Value, a: &[Value]) -> JsResult<Value> {
    this_screen(realm, &this)?;
    viewport_width(realm, this, a)
}

fn screen_height(realm: &mut Realm, this: Value, a: &[Value]) -> JsResult<Value> {
    this_screen(realm, &this)?;
    viewport_height(realm, this, a)
}

fn screen_depth(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    this_screen(realm, &this)?;
    Ok(Value::Number(24.0))
}

fn make_screen(realm: &mut Realm) -> ObjectId {
    let p = define_class(realm, "Screen", None, illegal_ctor);
    let o = realm.new_object_with_proto(Some(p));
    realm.set_host_data(o, Box::new(ScreenData));
    realm.set_class_name(o, "Screen");
    for name in ["width", "availWidth"] {
        realm.define_accessor(p, name, Some(screen_width), None, ACC);
    }
    for name in ["height", "availHeight"] {
        realm.define_accessor(p, name, Some(screen_height), None, ACC);
    }
    realm.define_accessor(p, "colorDepth", Some(screen_depth), None, ACC);
    realm.define_accessor(p, "pixelDepth", Some(screen_depth), None, ACC);
    realm.define_property(o, "availLeft", Value::Number(0.0), ACC);
    realm.define_property(o, "availTop", Value::Number(0.0), ACC);
    let orient = object_from(realm, vec![("type", Value::str("landscape-primary")), ("angle", Value::Number(0.0)), ("onchange", Value::Null)]);
    if let V::Object(oo) = orient.v() {
        realm.define_method(oo, "addEventListener", 2, noop);
        realm.define_method(oo, "removeEventListener", 2, noop);
        realm.define_method(oo, "lock", 1, rejected_not_supported);
        realm.define_method(oo, "unlock", 0, noop);
    }
    realm.define_property(o, "orientation", orient, ACC);
    realm.extra_roots.push(Value::Object(o));
    o
}

fn win_inner_width(realm: &mut Realm, this: Value, a: &[Value]) -> JsResult<Value> {
    viewport_width(realm, this, a)
}

fn win_inner_height(realm: &mut Realm, this: Value, a: &[Value]) -> JsResult<Value> {
    viewport_height(realm, this, a)
}

fn win_scroll_x(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Number(with_env(|env| env.scroll().0) as f64))
}

fn win_scroll_y(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Number(with_env(|env| env.scroll().1) as f64))
}

fn scroll_args(realm: &mut Realm, args: &[Value]) -> JsResult<(Option<f64>, Option<f64>)> {
    match args.first() {
        Some(v) if v.is_object() => {
            let x = realm.get_str(v.clone(), "left")?;
            let y = realm.get_str(v.clone(), "top")?;
            let x = if x.is_undefined() { None } else { Some(realm.to_number(x)?) };
            let y = if y.is_undefined() { None } else { Some(realm.to_number(y)?) };
            Ok((x, y))
        }
        Some(_) => {
            let x = realm.to_number(arg(args, 0))?;
            let y = realm.to_number(arg(args, 1))?;
            Ok((Some(x), Some(y)))
        }
        None => Ok((None, None)),
    }
}

fn win_scroll_to(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let (x, y) = scroll_args(realm, args)?;
    with_env(|env| {
        let (cx, cy) = env.scroll();
        let nx = x.map(|v| v as f32).unwrap_or(cx);
        let ny = y.map(|v| v as f32).unwrap_or(cy);
        env.set_scroll(nx.max(0.0), ny.max(0.0));
    });
    Ok(Value::Undefined)
}

fn win_scroll_by(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let (x, y) = scroll_args(realm, args)?;
    with_env(|env| {
        let (cx, cy) = env.scroll();
        env.set_scroll((cx + x.unwrap_or(0.0) as f32).max(0.0), (cy + y.unwrap_or(0.0) as f32).max(0.0));
    });
    Ok(Value::Undefined)
}

// ----------------------------------------------------------------- timers

fn timer_callback_args(realm: &mut Realm, args: &[Value]) -> JsResult<(Value, f64, Vec<Value>)> {
    let cb = arg(args, 0);
    let cb = if realm.is_callable(&cb) {
        cb
    } else if let V::String(s) = cb.v() {
        // String code: compile to a function.
        let src = format!("(function() {{\n{}\n}})", s.to_rust_string());
        realm.eval(&src, "timer")?
    } else {
        return Err(realm.throw_type_error("The callback provided as parameter 1 is not a function."));
    };
    let delay = match (args.get(1)).map(Value::v) {
        None | Some(V::Undefined) => 0.0,
        Some(v) => realm.to_number(v.to_value())?,
    };
    let delay = if delay.is_nan() || delay < 0.0 { 0.0 } else { delay };
    let extra: Vec<Value> = args.iter().skip(2).cloned().collect();
    Ok((cb, delay, extra))
}

fn add_timer(realm: &mut Realm, cb: Value, delay: f64, args: Vec<Value>, repeat: bool) -> u32 {
    let now = with_env(|env| env.now_ms());
    root_value(realm, &cb);
    for a in &args {
        root_value(realm, a);
    }
    let st = state();
    let mut st = st.borrow_mut();
    let id = st.next_timer_id;
    st.next_timer_id += 1;
    st.timers.push(Timer { id, due: now + delay, callback: cb, args, interval: if repeat { Some(delay.max(4.0)) } else { None } });
    id
}

/// Queue a task on the event loop (a zero-delay timer running native `f`).
pub(crate) fn queue_task(realm: &mut Realm, name: &str, f: rabbithole_js::NativeFn) {
    let cb = realm.new_native_function(name, 0, f);
    add_timer(realm, Value::Object(cb), 0.0, Vec::new(), false);
}

fn set_timeout(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let (cb, delay, extra) = timer_callback_args(realm, args)?;
    Ok(Value::Number(add_timer(realm, cb, delay, extra, false) as f64))
}

fn set_interval(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let (cb, delay, extra) = timer_callback_args(realm, args)?;
    Ok(Value::Number(add_timer(realm, cb, delay, extra, true) as f64))
}

pub(crate) fn clear_timer(realm: &mut Realm, id: u32) {
    let removed = {
        let st = state();
        let mut st = st.borrow_mut();
        let pos = st.timers.iter().position(|t| t.id == id);
        pos.map(|p| st.timers.remove(p))
    };
    if let Some(t) = removed {
        unroot_value(realm, &t.callback);
        for a in &t.args {
            unroot_value(realm, a);
        }
    }
}

fn clear_timeout(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if let Some(v) = args.first() {
        if let Ok(n) = realm.to_number(v.clone()) {
            if n.is_finite() {
                clear_timer(realm, n as u32);
            }
        }
    }
    Ok(Value::Undefined)
}

fn request_animation_frame(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let cb = arg(args, 0);
    if !realm.is_callable(&cb) {
        return Err(realm.throw_type_error("The callback provided as parameter 1 is not a function."));
    }
    root_value(realm, &cb);
    let st = state();
    let mut st = st.borrow_mut();
    let id = st.next_timer_id;
    st.next_timer_id += 1;
    st.rafs.push(RafCallback { id, callback: cb });
    Ok(Value::Number(id as f64))
}

fn cancel_animation_frame(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = realm.to_number(arg(args, 0))? as u32;
    let removed = {
        let st = state();
        let mut st = st.borrow_mut();
        let pos = st.rafs.iter().position(|r| r.id == id);
        pos.map(|p| st.rafs.remove(p))
    };
    if let Some(r) = removed {
        unroot_value(realm, &r.callback);
    }
    Ok(Value::Undefined)
}

fn request_idle_callback(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let cb = arg(args, 0);
    let f = realm.new_native_function_full("idleWrapper", 0, idle_wrapper, None, vec![cb.clone()], false);
    root_value(realm, &cb);
    Ok(Value::Number(add_timer(realm, Value::Object(f), 1.0, vec![], false) as f64))
}

fn idle_wrapper(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let caps = native_captures(realm);
    let Some(cb) = caps.first().cloned() else { return Ok(Value::Undefined) };
    let deadline = realm.new_object();
    realm.define_method(deadline, "timeRemaining", 0, idle_time_remaining);
    realm.define_property(deadline, "didTimeout", Value::Bool(false), ACC);
    realm.call(cb, Value::Undefined, &[Value::Object(deadline)])
}

fn idle_time_remaining(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Number(50.0))
}

fn queue_microtask(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let cb = arg(args, 0);
    if !realm.is_callable(&cb) {
        return Err(realm.throw_type_error("The callback provided as parameter 1 is not a function."));
    }
    realm.enqueue_callback(cb, Value::Undefined, vec![]);
    Ok(Value::Undefined)
}

// ---------------------------------------------------------------- dialogs

fn win_alert(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let msg = arg_str_opt(realm, args, 0)?.unwrap_or_default();
    with_env(|env| env.alert(&msg));
    Ok(Value::Undefined)
}

fn win_confirm(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let msg = arg_str_opt(realm, args, 0)?.unwrap_or_default();
    Ok(Value::Bool(with_env(|env| env.confirm(&msg))))
}

fn win_prompt(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let msg = arg_str_opt(realm, args, 0)?.unwrap_or_default();
    let def = arg_str_opt(realm, args, 1)?.unwrap_or_default();
    Ok(match with_env(|env| env.prompt(&msg, &def)) {
        Some(s) => Value::str(&s),
        None => Value::Null,
    })
}

fn win_open(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let url = arg_str_opt(realm, args, 0)?.unwrap_or_default();
    if url.is_empty() {
        return Ok(Value::Null);
    }
    match resolve_url(&url) {
        Some(u) => with_env(|env| env.open_window(u)),
        None => return Err(crate::misc::dom_exception(realm, "SyntaxError", &format!("Failed to execute 'open' on 'Window': Unable to open a window with invalid URL '{url}'."))),
    }
    Ok(Value::Null)
}

fn win_post_message(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let data = arg(args, 0);
    root_value(realm, &data);
    let f = realm.new_native_function_full("postMessageDeliver", 0, post_message_deliver, None, vec![data], false);
    realm.enqueue_callback(Value::Object(f), Value::Undefined, vec![]);
    Ok(Value::Undefined)
}

fn post_message_deliver(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let caps = native_captures(realm);
    let data = caps.first().cloned().unwrap_or(Value::Undefined);
    let init = crate::EventInit::simple(false, false);
    let id = crate::events::create_event(realm, "MessageEvent", "message", &init, true);
    realm.define_property(id, "data", data.clone(), ACC);
    realm.define_property(id, "origin", Value::str(&current_url().origin()), ACC);
    let w = crate::events::window_value(realm);
    realm.define_property(id, "source", w, ACC);
    let ports = realm.new_array_value(vec![]);
    realm.define_property(id, "ports", ports, ACC);
    crate::events::dispatch_object(realm, TargetKey::Window, id, None);
    unroot_value(realm, &data);
    Ok(Value::Undefined)
}

fn win_get_selection(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    crate::misc::selection_object(realm)
}

fn win_self(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(crate::events::window_value(realm))
}

fn win_document(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Object(state().borrow().document_obj))
}

fn win_event(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let _ = realm;
    Ok(state().borrow().event_stack.last().map(|e| Value::Object(*e)).unwrap_or(Value::Undefined))
}

fn win_origin(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::str(&current_url().origin()))
}

fn win_is_secure(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(current_url().scheme == "https"))
}

fn win_structured_clone(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let v = arg(args, 0);
    if !v.is_object() {
        return Ok(v);
    }
    let json = realm.get_str(Value::Object(realm.global()), "JSON")?;
    let stringify = realm.get_str(json.clone(), "stringify")?;
    let parse = realm.get_str(json.clone(), "parse")?;
    let s = realm.call(stringify, json.clone(), &[v])?;
    if s.is_undefined() {
        return Ok(Value::Undefined);
    }
    realm.call(parse, json, &[s])
}

fn win_get_computed_style_pseudo(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Null)
}

// ---------------------------------------------------------------- Storage

struct StorageRef {
    local: bool,
}

fn storage_target(realm: &mut Realm, args: &[Value]) -> JsResult<bool> {
    match (arg(args, 0)).v() {
        V::Object(o) => match realm.host_data::<StorageRef>(o) {
            Some(r) => Ok(r.local),
            None => Err(realm.throw_type_error("Illegal invocation")),
        },
        _ => Err(realm.throw_type_error("Illegal invocation")),
    }
}

fn storage_this(realm: &mut Realm, this: &Value) -> JsResult<bool> {
    match host_obj(realm, this) {
        Some(o) => match realm.host_data::<StorageRef>(o) {
            Some(r) => Ok(r.local),
            None => Err(realm.throw_type_error("Illegal invocation")),
        },
        None => Err(realm.throw_type_error("Illegal invocation")),
    }
}

fn storage_get_item(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let local = storage_this(realm, &this)?;
    let k = arg_str(realm, args, 0)?;
    Ok(match with_env(|env| env.storage_get(local, &k)) {
        Some(v) => Value::str(&v),
        None => Value::Null,
    })
}

fn storage_set_item(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let local = storage_this(realm, &this)?;
    let k = arg_str(realm, args, 0)?;
    let v = arg_str(realm, args, 1)?;
    with_env(|env| env.storage_set(local, &k, Some(&v)));
    Ok(Value::Undefined)
}

fn storage_remove_item(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let local = storage_this(realm, &this)?;
    let k = arg_str(realm, args, 0)?;
    with_env(|env| env.storage_set(local, &k, None));
    Ok(Value::Undefined)
}

fn storage_clear(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let local = storage_this(realm, &this)?;
    with_env(|env| {
        for k in env.storage_keys(local) {
            env.storage_set(local, &k, None);
        }
    });
    Ok(Value::Undefined)
}

fn storage_key(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let local = storage_this(realm, &this)?;
    let i = realm.to_number(arg(args, 0))?.max(0.0) as usize;
    let keys = with_env(|env| env.storage_keys(local));
    Ok(keys.get(i).map(|k| Value::str(k)).unwrap_or(Value::Null))
}

fn storage_length(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let local = storage_this(realm, &this)?;
    Ok(Value::Number(with_env(|env| env.storage_keys(local).len()) as f64))
}

fn storage_proxy_get(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let local = storage_target(realm, args)?;
    let target = arg(args, 0);
    let Some(k) = arg(args, 1).as_str().map(|k| k.to_rust_string()) else { return Ok(Value::Undefined) };
    // Methods and length come from the target/prototype.
    let own = realm.get_str(target, &k)?;
    if !own.is_undefined() {
        return Ok(own);
    }
    Ok(match with_env(|env| env.storage_get(local, &k)) {
        Some(v) => Value::str(&v),
        None => Value::Undefined,
    })
}

fn storage_proxy_set(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let local = storage_target(realm, args)?;
    let k = realm.to_rust_string(arg(args, 1))?;
    let v = realm.to_rust_string(arg(args, 2))?;
    with_env(|env| env.storage_set(local, &k, Some(&v)));
    Ok(Value::Bool(true))
}

fn storage_proxy_delete(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let local = storage_target(realm, args)?;
    let k = realm.to_rust_string(arg(args, 1))?;
    with_env(|env| env.storage_set(local, &k, None));
    Ok(Value::Bool(true))
}

fn storage_proxy_has(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let local = storage_target(realm, args)?;
    let k = realm.to_rust_string(arg(args, 1))?;
    Ok(Value::Bool(with_env(|env| env.storage_get(local, &k)).is_some()))
}

fn storage_proxy_own_keys(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let local = storage_target(realm, args)?;
    let keys: Vec<Value> = with_env(|env| env.storage_keys(local)).iter().map(|k| Value::str(k)).collect();
    Ok(realm.new_array_value(keys))
}

fn storage_proxy_gopd(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let local = storage_target(realm, args)?;
    let k = realm.to_rust_string(arg(args, 1))?;
    match with_env(|env| env.storage_get(local, &k)) {
        Some(v) => Ok(object_from(realm, vec![("value", Value::str(&v)), ("writable", Value::Bool(true)), ("enumerable", Value::Bool(true)), ("configurable", Value::Bool(true))])),
        None => Ok(Value::Undefined),
    }
}

fn make_storage(realm: &mut Realm, local: bool) -> Value {
    let p = proto("Storage");
    let target = realm.new_object_with_proto(Some(p));
    realm.set_host_data(target, Box::new(StorageRef { local }));
    realm.set_class_name(target, "Storage");
    let v = crate::element::make_proxy(realm, target, &[("get", storage_proxy_get), ("set", storage_proxy_set), ("deleteProperty", storage_proxy_delete), ("has", storage_proxy_has), ("ownKeys", storage_proxy_own_keys), ("getOwnPropertyDescriptor", storage_proxy_gopd)]);
    realm.extra_roots.push(v.clone());
    v
}

// ------------------------------------------------------------ performance

fn perf_now(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let origin = state().borrow().time_origin;
    Ok(Value::Number(with_env(|env| env.now_ms()) - origin))
}

fn perf_entries(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(realm.new_array_value(vec![]))
}

fn perf_mark(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let name = arg_str_opt(realm, args, 0)?.unwrap_or_default();
    let t = perf_now(realm, Value::Undefined, &[])?;
    Ok(object_from(realm, vec![("name", Value::str(&name)), ("entryType", Value::str("mark")), ("startTime", t), ("duration", Value::Number(0.0))]))
}

fn perf_to_json(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let origin = state().borrow().time_origin;
    Ok(object_from(realm, vec![("timeOrigin", Value::Number(origin))]))
}

fn make_performance(realm: &mut Realm) -> ObjectId {
    let p = define_class(realm, "Performance", Some("EventTarget"), illegal_ctor);
    let o = realm.new_object_with_proto(Some(p));
    realm.define_method(o, "now", 0, perf_now);
    realm.define_method(o, "mark", 1, perf_mark);
    realm.define_method(o, "measure", 1, perf_mark);
    realm.define_method(o, "getEntries", 0, perf_entries);
    realm.define_method(o, "getEntriesByType", 1, perf_entries);
    realm.define_method(o, "getEntriesByName", 1, perf_entries);
    realm.define_method(o, "clearMarks", 0, noop);
    realm.define_method(o, "clearMeasures", 0, noop);
    realm.define_method(o, "clearResourceTimings", 0, noop);
    realm.define_method(o, "setResourceTimingBufferSize", 1, noop);
    realm.define_method(o, "toJSON", 0, perf_to_json);
    let origin = state().borrow().time_origin;
    realm.define_property(o, "timeOrigin", Value::Number(origin), ACC);
    let timing = object_from(realm, vec![("navigationStart", Value::Number(origin)), ("fetchStart", Value::Number(origin)), ("domLoading", Value::Number(origin)), ("domInteractive", Value::Number(origin)), ("domContentLoadedEventStart", Value::Number(origin)), ("domContentLoadedEventEnd", Value::Number(origin)), ("domComplete", Value::Number(origin)), ("loadEventStart", Value::Number(origin)), ("loadEventEnd", Value::Number(origin)), ("responseStart", Value::Number(origin)), ("responseEnd", Value::Number(origin))]);
    realm.define_property(o, "timing", timing, ACC);
    let nav = object_from(realm, vec![("type", Value::Number(0.0)), ("redirectCount", Value::Number(0.0))]);
    realm.define_property(o, "navigation", nav, ACC);
    realm.extra_roots.push(Value::Object(o));
    o
}

// --------------------------------------------------------------- crypto

fn crypto_get_random_values(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let v = arg(args, 0);
    let V::Object(o) = v.v() else {
        return Err(realm.throw_type_error("parameter 1 is not of type 'ArrayBufferView'"));
    };
    let len = realm.length_of(v.clone())?;
    let mut buf = vec![0u8; len * 8];
    with_env(|env| env.random_bytes(&mut buf));
    // Determine element bit width from the class name.
    let bits = match realm.obj(o).kind {
        rabbithole_js::ObjectKind::TypedArray(ref t) => t.kind.element_size() * 8,
        _ => 8,
    };
    for i in 0..len {
        let n: u64 = match bits {
            8 => buf[i] as u64,
            16 => u16::from_le_bytes([buf[2 * i], buf[2 * i + 1]]) as u64,
            32 => u32::from_le_bytes([buf[4 * i], buf[4 * i + 1], buf[4 * i + 2], buf[4 * i + 3]]) as u64,
            _ => u32::from_le_bytes([buf[4 * i], buf[4 * i + 1], buf[4 * i + 2], buf[4 * i + 3]]) as u64,
        };
        realm.set(v.clone(), PropertyKey::index(i as u32), Value::Number(n as f64))?;
    }
    Ok(v)
}

fn crypto_random_uuid(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let mut b = [0u8; 16];
    with_env(|env| env.random_bytes(&mut b));
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: Vec<String> = b.iter().map(|x| format!("{x:02x}")).collect();
    let s = format!("{}{}{}{}-{}{}-{}{}-{}{}-{}{}{}{}{}{}", hex[0], hex[1], hex[2], hex[3], hex[4], hex[5], hex[6], hex[7], hex[8], hex[9], hex[10], hex[11], hex[12], hex[13], hex[14], hex[15]);
    Ok(Value::str(&s))
}

fn make_crypto(realm: &mut Realm) -> ObjectId {
    let o = realm.new_object();
    realm.set_class_name(o, "Crypto");
    realm.define_method(o, "getRandomValues", 1, crypto_get_random_values);
    realm.define_method(o, "randomUUID", 0, crypto_random_uuid);
    let subtle = realm.new_object();
    for m in ["digest", "encrypt", "decrypt", "sign", "verify", "generateKey", "importKey", "exportKey", "deriveKey", "deriveBits"] {
        realm.define_method(subtle, m, 2, rejected_not_supported);
    }
    realm.define_property(o, "subtle", Value::Object(subtle), ACC);
    realm.extra_roots.push(Value::Object(o));
    o
}

// -------------------------------------------------------------- install

/// Build the window object (the global) and all its members.
pub(crate) fn install(realm: &mut Realm) {
    let g = realm.global();
    // window inherits from Window.prototype → EventTarget.prototype.
    let wp = define_class(realm, "Window", Some("EventTarget"), illegal_ctor);
    realm.obj_mut(g).set_proto(Some(wp));
    realm.set_class_name(g, "Window");
    state().borrow_mut().window = g;
    state().borrow_mut().protos.insert("__window", g);
    for name in ["window", "self", "top", "parent", "frames", "globalThis"] {
        realm.define_accessor(g, name, Some(win_self), None, ACC);
    }
    realm.define_accessor(g, "document", Some(win_document), None, ACC);
    let loc = make_location(realm);
    let lg = realm.new_native_function("get location", 0, location_getter);
    let ls_fn = realm.new_native_function("set location", 1, location_setter);
    realm.define_accessor_values(g, "location", Some(Value::Object(lg)), Some(Value::Object(ls_fn)), ACC);
    let _ = loc;
    let hist = make_history(realm);
    realm.define_property(g, "history", Value::Object(hist), ACC);
    let nav = make_navigator(realm);
    realm.define_property(g, "navigator", Value::Object(nav.clone_id()), ACC);
    realm.define_property(g, "clientInformation", Value::Object(nav), ACC);
    let screen = make_screen(realm);
    realm.define_property(g, "screen", Value::Object(screen), ACC);
    realm.define_accessor(g, "innerWidth", Some(win_inner_width), None, ACC);
    realm.define_accessor(g, "innerHeight", Some(win_inner_height), None, ACC);
    realm.define_accessor(g, "outerWidth", Some(win_inner_width), None, ACC);
    realm.define_accessor(g, "outerHeight", Some(win_inner_height), None, ACC);
    realm.define_accessor(g, "scrollX", Some(win_scroll_x), None, ACC);
    realm.define_accessor(g, "scrollY", Some(win_scroll_y), None, ACC);
    realm.define_accessor(g, "pageXOffset", Some(win_scroll_x), None, ACC);
    realm.define_accessor(g, "pageYOffset", Some(win_scroll_y), None, ACC);
    realm.define_accessor(g, "event", Some(win_event), None, ACC);
    realm.define_accessor(g, "origin", Some(win_origin), None, ACC);
    realm.define_accessor(g, "isSecureContext", Some(win_is_secure), None, ACC);
    realm.define_property(g, "devicePixelRatio", Value::Number(1.0), ALL);
    realm.define_property(g, "name", Value::str(""), ALL);
    realm.define_property(g, "status", Value::str(""), ALL);
    realm.define_property(g, "closed", Value::Bool(false), ACC);
    realm.define_property(g, "length", Value::Number(0.0), ACC);
    realm.define_property(g, "opener", Value::Null, ALL);
    realm.define_accessor(g, "frameElement", Some(win_frame_element), None, ACC);
    realm.define_property(g, "screenX", Value::Number(0.0), ACC);
    realm.define_property(g, "screenY", Value::Number(0.0), ACC);
    realm.define_property(g, "screenLeft", Value::Number(0.0), ACC);
    realm.define_property(g, "screenTop", Value::Number(0.0), ACC);
    realm.define_property(g, "speechSynthesis", Value::Undefined, ALL);
    realm.define_method(g, "scrollTo", 2, win_scroll_to);
    realm.define_method(g, "scroll", 2, win_scroll_to);
    realm.define_method(g, "scrollBy", 2, win_scroll_by);
    realm.define_method(g, "setTimeout", 2, set_timeout);
    realm.define_method(g, "setInterval", 2, set_interval);
    realm.define_method(g, "clearTimeout", 1, clear_timeout);
    realm.define_method(g, "clearInterval", 1, clear_timeout);
    realm.define_method(g, "requestAnimationFrame", 1, request_animation_frame);
    realm.define_method(g, "cancelAnimationFrame", 1, cancel_animation_frame);
    realm.define_method(g, "webkitRequestAnimationFrame", 1, request_animation_frame);
    realm.define_method(g, "requestIdleCallback", 1, request_idle_callback);
    realm.define_method(g, "cancelIdleCallback", 1, clear_timeout);
    realm.define_method(g, "queueMicrotask", 1, queue_microtask);
    realm.define_method(g, "alert", 1, win_alert);
    realm.define_method(g, "confirm", 1, win_confirm);
    realm.define_method(g, "prompt", 2, win_prompt);
    realm.define_method(g, "open", 1, win_open);
    realm.define_method(g, "close", 0, noop);
    realm.define_method(g, "focus", 0, noop);
    realm.define_method(g, "blur", 0, noop);
    realm.define_method(g, "print", 0, noop);
    realm.define_method(g, "stop", 0, noop);
    realm.define_method(g, "postMessage", 2, win_post_message);
    realm.define_method(g, "getSelection", 0, win_get_selection);
    realm.define_method(g, "structuredClone", 1, win_structured_clone);
    realm.define_method(g, "captureEvents", 1, noop);
    realm.define_method(g, "releaseEvents", 1, noop);
    realm.define_method(g, "moveTo", 2, noop);
    realm.define_method(g, "resizeTo", 2, noop);
    realm.define_method(g, "find", 1, ret_false);
    let _ = win_get_computed_style_pseudo;
    let _ = ret_true;
    crate::events::install_handler_props(realm, wp, crate::element::EVENT_HANDLER_NAMES);
    crate::events::install_handler_props(realm, wp, &["hashchange", "popstate", "beforeunload", "unload", "message", "pageshow", "pagehide", "online", "offline", "storage", "unhandledrejection", "rejectionhandled", "DOMContentLoaded", "orientationchange", "visibilitychange", "languagechange"]);
    // Storage
    define_class_storage(realm);
    let ls = make_storage(realm, true);
    let ss = make_storage(realm, false);
    realm.define_property(g, "localStorage", ls, ACC);
    realm.define_property(g, "sessionStorage", ss, ACC);
    let perf = make_performance(realm);
    realm.define_property(g, "performance", Value::Object(perf), ACC);
    let crypto = make_crypto(realm);
    realm.define_property(g, "crypto", Value::Object(crypto), ACC);
    let vv = object_from(realm, vec![("offsetLeft", Value::Number(0.0)), ("offsetTop", Value::Number(0.0)), ("pageLeft", Value::Number(0.0)), ("pageTop", Value::Number(0.0)), ("scale", Value::Number(1.0))]);
    if let V::Object(v) = vv.v() {
        realm.define_accessor(v, "width", Some(viewport_width), None, ACC);
        realm.define_accessor(v, "height", Some(viewport_height), None, ACC);
        realm.define_method(v, "addEventListener", 2, noop);
        realm.define_method(v, "removeEventListener", 2, noop);
    }
    realm.define_property(g, "visualViewport", vv, ACC);
    let _ = HIDDEN;
}

fn define_class_storage(realm: &mut Realm) {
    let p = define_class(realm, "Storage", None, illegal_ctor);
    realm.define_method(p, "getItem", 1, storage_get_item);
    realm.define_method(p, "setItem", 2, storage_set_item);
    realm.define_method(p, "removeItem", 1, storage_remove_item);
    realm.define_method(p, "clear", 0, storage_clear);
    realm.define_method(p, "key", 1, storage_key);
    realm.define_accessor(p, "length", Some(storage_length), None, ACC);
}

trait CloneId {
    fn clone_id(&self) -> ObjectId;
}

impl CloneId for ObjectId {
    fn clone_id(&self) -> ObjectId {
        *self
    }
}

/// `window.frameElement`: the `<iframe>` hosting this document, as a node
/// of the (same-origin) parent document; null for top-level and
/// cross-origin frames.
fn win_frame_element(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let Some((parent, node)) = with_env(|env| env.frame_element()) else { return Ok(Value::Null) };
    if parent >= crate::host::FRAME_DOC as u64 {
        return Ok(Value::Null);
    }
    Ok(crate::host::wrap(realm, crate::host::NodeRef { doc: crate::host::FRAME_DOC | parent as u32, node }))
}
