//! Global functions, console and the global object itself.

use super::*;
use crate::value::V;

pub fn register(realm: &mut Realm) {
    let g = realm.global;
    realm.define_property(g, "globalThis", Value::Object(g), HIDDEN);
    realm.define_property(g, "undefined", Value::Undefined, 0);
    realm.define_property(g, "NaN", Value::Number(f64::NAN), 0);
    realm.define_property(g, "Infinity", Value::Number(f64::INFINITY), 0);
    realm.define_method(g, "isNaN", 1, is_nan);
    realm.define_method(g, "isFinite", 1, is_finite);
    realm.define_method(g, "parseInt", 2, parse_int);
    realm.define_method(g, "parseFloat", 1, parse_float);
    realm.define_method(g, "encodeURIComponent", 1, encode_uri_component);
    realm.define_method(g, "encodeURI", 1, encode_uri);
    realm.define_method(g, "decodeURIComponent", 1, decode_uri_component);
    realm.define_method(g, "decodeURI", 1, decode_uri);
    realm.define_method(g, "escape", 1, escape);
    realm.define_method(g, "unescape", 1, unescape);
    realm.define_method(g, "queueMicrotask", 1, queue_microtask);
    realm.define_method(g, "structuredClone", 1, structured_clone);
    let eval_fn = realm.define_method(g, "eval", 1, eval);
    realm.intrinsics.eval_fn = eval_fn;

    let console = realm.new_object();
    realm.define_global("console", Value::Object(console));
    for (name, level) in [("log", "log"), ("info", "info"), ("warn", "warn"), ("error", "error"), ("debug", "debug"), ("trace", "trace"), ("dir", "log"), ("table", "log"), ("group", "log"), ("groupCollapsed", "log"), ("groupEnd", "log")] {
        let f = match level {
            "info" => console_info,
            "warn" => console_warn,
            "error" => console_error,
            "debug" => console_debug,
            "trace" => console_trace,
            _ => console_log,
        };
        realm.define_method(console, name, 0, f);
    }
    realm.define_method(console, "assert", 2, console_assert);
    realm.define_method(console, "count", 1, console_count);
    realm.define_method(console, "time", 1, console_noop);
    realm.define_method(console, "timeEnd", 1, console_noop);
    realm.define_method(console, "timeLog", 1, console_noop);
    realm.define_method(console, "clear", 0, console_noop);
}

fn is_nan(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(realm.to_number(arg(args, 0))?.is_nan()))
}

fn is_finite(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(realm.to_number(arg(args, 0))?.is_finite()))
}

pub fn parse_int(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let s = realm.to_string(arg(args, 0))?;
    let s = s.trim_js(true, false).to_rust_string();
    let mut radix = realm.to_int32(arg(args, 1))?;
    let mut chars = s.as_str();
    let mut sign = 1.0;
    if let Some(r) = chars.strip_prefix('-') {
        sign = -1.0;
        chars = r;
    } else if let Some(r) = chars.strip_prefix('+') {
        chars = r;
    }
    let mut strip_prefix = true;
    if radix != 0 {
        if !(2..=36).contains(&radix) {
            return Ok(Value::Number(f64::NAN));
        }
        if radix != 16 {
            strip_prefix = false;
        }
    } else {
        radix = 10;
    }
    if strip_prefix && (chars.starts_with("0x") || chars.starts_with("0X")) {
        chars = &chars[2..];
        radix = 16;
    }
    let mut value = 0.0f64;
    let mut any = false;
    for c in chars.chars() {
        match c.to_digit(radix as u32) {
            Some(d) => {
                value = value * radix as f64 + d as f64;
                any = true;
            }
            None => break,
        }
    }
    if !any {
        return Ok(Value::Number(f64::NAN));
    }
    Ok(Value::Number(sign * value))
}

pub fn parse_float(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let s = realm.to_string(arg(args, 0))?;
    let s = s.trim_js(true, false).to_rust_string();
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    if s[i..].starts_with("Infinity") {
        return Ok(Value::Number(if b.first() == Some(&b'-') { f64::NEG_INFINITY } else { f64::INFINITY }));
    }
    let ds = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let mut digits = i - ds;
    if i < b.len() && b[i] == b'.' {
        let save = i;
        i += 1;
        let fs = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        digits += i - fs;
        if i == fs && digits == 0 {
            i = save;
        }
    }
    if digits == 0 {
        return Ok(Value::Number(f64::NAN));
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let save = i;
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let es = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == es {
            i = save;
        }
    }
    let text = &s[..i];
    let text = text.strip_suffix('.').unwrap_or(text);
    Ok(Value::Number(text.parse::<f64>().unwrap_or(f64::NAN)))
}

fn uri_encode(realm: &mut Realm, v: Value, unescaped: &str) -> JsResult<Value> {
    let s = realm.to_string(v)?;
    let units = s.as_units();
    let mut out = String::new();
    let mut i = 0;
    while i < units.len() {
        let c = units[i];
        if c < 128 && (unescaped.as_bytes().contains(&(c as u8)) || (c as u8).is_ascii_alphanumeric()) {
            out.push(c as u8 as char);
            i += 1;
            continue;
        }
        let cp = if (0xDC00..0xE000).contains(&c) {
            return Err(realm.new_error("URIError", "URI malformed"));
        } else if (0xD800..0xDC00).contains(&c) {
            if i + 1 >= units.len() || !(0xDC00..0xE000).contains(&units[i + 1]) {
                return Err(realm.new_error("URIError", "URI malformed"));
            }
            let cp = 0x10000 + ((c as u32 - 0xD800) << 10) + (units[i + 1] as u32 - 0xDC00);
            i += 1;
            cp
        } else {
            c as u32
        };
        let ch = char::from_u32(cp).unwrap();
        let mut buf = [0u8; 4];
        for b in ch.encode_utf8(&mut buf).bytes() {
            out.push_str(&format!("%{:02X}", b));
        }
        i += 1;
    }
    Ok(Value::str(&out))
}

fn encode_uri_component(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    uri_encode(realm, arg(args, 0), "-_.!~*'()")
}

fn encode_uri(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    uri_encode(realm, arg(args, 0), "-_.!~*'();/?:@&=+$,#")
}

fn uri_decode(realm: &mut Realm, v: Value, reserved: &str) -> JsResult<Value> {
    let s = realm.to_string(v)?;
    let units = s.as_units();
    let mut out: Vec<u16> = Vec::with_capacity(units.len());
    let mut i = 0;
    let hex = |u: &[u16], i: usize| -> Option<u8> {
        if i + 2 >= u.len() + 0 && i + 2 > u.len() {
            return None;
        }
        let h = String::from_utf16_lossy(&u[i + 1..i + 3]);
        u8::from_str_radix(&h, 16).ok()
    };
    while i < units.len() {
        let c = units[i];
        if c != b'%' as u16 {
            out.push(c);
            i += 1;
            continue;
        }
        if i + 2 >= units.len() {
            return Err(realm.new_error("URIError", "URI malformed"));
        }
        let b0 = match hex(units, i) {
            Some(b) => b,
            None => return Err(realm.new_error("URIError", "URI malformed")),
        };
        if b0 < 0x80 {
            if reserved.as_bytes().contains(&b0) {
                out.extend_from_slice(&units[i..i + 3]);
            } else {
                out.push(b0 as u16);
            }
            i += 3;
            continue;
        }
        let n = if b0 & 0xE0 == 0xC0 {
            2
        } else if b0 & 0xF0 == 0xE0 {
            3
        } else if b0 & 0xF8 == 0xF0 {
            4
        } else {
            return Err(realm.new_error("URIError", "URI malformed"));
        };
        let mut bytes = vec![b0];
        let mut j = i + 3;
        for _ in 1..n {
            if j + 2 >= units.len() || units[j] != b'%' as u16 {
                return Err(realm.new_error("URIError", "URI malformed"));
            }
            match hex(units, j) {
                Some(b) if b & 0xC0 == 0x80 => bytes.push(b),
                _ => return Err(realm.new_error("URIError", "URI malformed")),
            }
            j += 3;
        }
        match std::str::from_utf8(&bytes) {
            Ok(st) => out.extend(st.encode_utf16()),
            Err(_) => return Err(realm.new_error("URIError", "URI malformed")),
        }
        i = j;
    }
    Ok(Value::String(JsStr::from_units(out)))
}

fn decode_uri_component(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    uri_decode(realm, arg(args, 0), "")
}

fn decode_uri(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    uri_decode(realm, arg(args, 0), ";/?:@&=+$,#")
}

fn escape(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let s = realm.to_string(arg(args, 0))?;
    let mut out = String::new();
    for &u in s.as_units() {
        if u < 128 && ((u as u8).is_ascii_alphanumeric() || b"@*_+-./".contains(&(u as u8))) {
            out.push(u as u8 as char);
        } else if u < 256 {
            out.push_str(&format!("%{:02X}", u));
        } else {
            out.push_str(&format!("%u{:04X}", u));
        }
    }
    Ok(Value::str(&out))
}

fn unescape(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let s = realm.to_string(arg(args, 0))?;
    let u = s.as_units();
    let mut out: Vec<u16> = Vec::new();
    let mut i = 0;
    while i < u.len() {
        if u[i] == b'%' as u16 {
            if i + 5 < u.len() + 0 && u.get(i + 1) == Some(&(b'u' as u16)) && i + 6 <= u.len() {
                if let Ok(v) = u16::from_str_radix(&String::from_utf16_lossy(&u[i + 2..i + 6]), 16) {
                    out.push(v);
                    i += 6;
                    continue;
                }
            }
            if i + 3 <= u.len() {
                if let Ok(v) = u8::from_str_radix(&String::from_utf16_lossy(&u[i + 1..i + 3]), 16) {
                    out.push(v as u16);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(u[i]);
        i += 1;
    }
    Ok(Value::String(JsStr::from_units(out)))
}

fn queue_microtask(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let f = arg(args, 0);
    if !realm.is_callable(&f) {
        return realm.type_error("The callback provided as parameter 1 is not a function");
    }
    realm.enqueue_callback(f, Value::Undefined, vec![]);
    Ok(Value::Undefined)
}

fn structured_clone(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    fn clone_value(realm: &mut Realm, v: Value, seen: &mut Vec<(ObjectId, ObjectId)>) -> JsResult<Value> {
        let id = match v.v() {
            V::Object(id) => id,
            _ => return Ok(v),
        };
        if let Some((_, c)) = seen.iter().find(|(o, _)| *o == id) {
            return Ok(Value::Object(*c));
        }
        if realm.is_callable(&v) {
            return Err(realm.new_error("DataCloneError", "function could not be cloned"));
        }
        let out = match &realm.obj(id).kind {
            ObjectKind::Array { .. } => realm.new_array(vec![]),
            ObjectKind::Date(d) => {
                let p = realm.intrinsics.others.get("DatePrototype").copied();
                realm.alloc(Object::new(p, ObjectKind::Date(*d)))
            }
            ObjectKind::Map(m) => {
                let data = m.snapshot();
                let p = realm.intrinsics.others.get("MapPrototype").copied();
                realm.alloc(Object::new(p, ObjectKind::Map(Box::new(data))))
            }
            ObjectKind::Set(m) => {
                let data = m.snapshot();
                let p = realm.intrinsics.others.get("SetPrototype").copied();
                realm.alloc(Object::new(p, ObjectKind::Set(Box::new(data))))
            }
            ObjectKind::Boolean(b) => {
                let p = realm.intrinsics.boolean_proto;
                realm.alloc(Object::new(Some(p), ObjectKind::Boolean(*b)))
            }
            ObjectKind::Number(n) => {
                let p = realm.intrinsics.number_proto;
                realm.alloc(Object::new(Some(p), ObjectKind::Number(*n)))
            }
            ObjectKind::String(s) => {
                let p = realm.intrinsics.string_proto;
                realm.alloc(Object::new(Some(p), ObjectKind::String(s.clone())))
            }
            _ => realm.new_object(),
        };
        seen.push((id, out));
        let keys = realm.own_enumerable_string_keys(id)?;
        for k in keys {
            let pv = realm.get(Value::Object(id), k.clone())?;
            let cv = clone_value(realm, pv, seen)?;
            realm.define_own(out, k, Property::data(cv, ALL))?;
        }
        Ok(Value::Object(out))
    }
    let mut seen = Vec::new();
    clone_value(realm, arg(args, 0), &mut seen)
}

fn eval(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let src = match (arg(args, 0)).v() {
        V::String(s) => s.to_rust_string(),
        other => return Ok(other.to_value()),
    };
    // Indirect eval: global scope, but its lexical declarations stay in
    // an environment of their own and its vars are deletable globals.
    let program = match crate::parser::parse_program_in(&src, false, &Default::default()) {
        Ok(p) => p,
        Err(e) => {
            let msg = format!("{} (eval:{}:{})", e.message, e.line, e.column);
            return Err(realm.new_error("SyntaxError", &msg));
        }
    };
    let site = crate::compiler::EvalSite { env: None, strict: false, class_depth: 0, in_function: false, in_method: false, in_derived_ctor: false, in_field_init: false, in_params: false };
    let code = match crate::compiler::compile_eval(&program, &site, &src) {
        Ok(c) => c,
        Err(e) => return Err(realm.new_error("SyntaxError", &e.message)),
    };
    realm.run_script(code)
}

// ----------------------------------------------------------------- console

/// Render values the way DevTools would (strings bare, objects inspected).
pub fn format_console_args(realm: &mut Realm, args: &[Value]) -> String {
    let mut parts = Vec::new();
    for (i, a) in args.iter().enumerate() {
        parts.push(match a.v() {
            V::String(s) if i == 0 || true => s.to_rust_string(),
            other => inspect(realm, &other.to_value(), 0),
        });
    }
    parts.join(" ")
}

pub fn inspect(realm: &mut Realm, v: &Value, depth: usize) -> String {
    match v.v() {
        V::String(s) => format!("'{}'", s),
        V::Object(id) => {
            if depth > 2 {
                return "[Object]".into();
            }
            let id = id;
            if realm.is_callable(v) {
                let n = realm.function_name(id);
                return format!("[Function: {}]", if n.is_empty() { "(anonymous)".to_string() } else { n.to_rust_string() });
            }
            match &realm.obj(id).kind {
                ObjectKind::Array { .. } => {
                    let items = realm.create_list_from_array_like(v.clone()).unwrap_or_default();
                    let parts: Vec<String> = items.iter().take(100).map(|x| inspect(realm, x, depth + 1)).collect();
                    format!("[ {} ]", parts.join(", "))
                }
                ObjectKind::Error => realm.error_to_string(v),
                ObjectKind::Date(_) | ObjectKind::RegExp(_) | ObjectKind::Symbol(_) | ObjectKind::Number(_) | ObjectKind::String(_) | ObjectKind::Boolean(_) => {
                    realm.to_string(v.clone()).map(|s| s.to_rust_string()).unwrap_or_else(|_| "[object]".into())
                }
                ObjectKind::Promise(p) => format!("Promise {{ {} }}", match p.state {
                    PromiseState::Pending => "<pending>".to_string(),
                    PromiseState::Fulfilled => {
                        let val = p.value.clone();
                        inspect(realm, &val, depth + 1)
                    }
                    PromiseState::Rejected => {
                        let val = p.value.clone();
                        format!("<rejected> {}", inspect(realm, &val, depth + 1))
                    }
                }),
                ObjectKind::Map(m) => format!("Map({})", m.size()),
                ObjectKind::Set(m) => format!("Set({})", m.size()),
                _ => {
                    let keys = realm.own_enumerable_string_keys(id).unwrap_or_default();
                    if keys.is_empty() {
                        return "{}".into();
                    }
                    let mut parts = Vec::new();
                    for k in keys.iter().take(50) {
                        let pv = realm.get(Value::Object(id), k.clone()).unwrap_or(Value::Undefined);
                        parts.push(format!("{}: {}", realm.key_display(k), inspect(realm, &pv, depth + 1)));
                    }
                    format!("{{ {} }}", parts.join(", "))
                }
            }
        }
        V::Symbol(s) => format!("Symbol({})", realm.symbol_description(s)),
        V::BigInt(b) => format!("{}n", b.to_string_radix(10)),
        V::Number(n) => number_to_string(n),
        V::Bool(b) => b.to_string(),
        V::Null => "null".into(),
        V::Undefined | V::Empty => "undefined".into(),
    }
}

fn console_emit(realm: &mut Realm, level: &str, args: &[Value]) -> JsResult<Value> {
    let text = format_console_args(realm, args);
    realm.console_emit(level, text);
    Ok(Value::Undefined)
}

fn console_log(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    console_emit(realm, "log", args)
}
fn console_info(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    console_emit(realm, "info", args)
}
fn console_warn(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    console_emit(realm, "warn", args)
}
fn console_error(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    console_emit(realm, "error", args)
}
fn console_debug(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    console_emit(realm, "debug", args)
}
fn console_trace(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let mut text = format_console_args(realm, args);
    text.push('\n');
    text.push_str(&realm.build_stack_string("Trace", ""));
    realm.console_emit("trace", text);
    Ok(Value::Undefined)
}
fn console_assert(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if !arg(args, 0).truthy() {
        let rest = if args.len() > 1 { &args[1..] } else { &[] };
        let mut text = "Assertion failed".to_string();
        if !rest.is_empty() {
            text.push_str(": ");
            text.push_str(&format_console_args(realm, rest));
        }
        realm.console_emit("error", text);
    }
    Ok(Value::Undefined)
}
fn console_count(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let label = if arg(args, 0).is_undefined() { "default".to_string() } else { realm.to_rust_string(arg(args, 0))? };
    realm.console_emit("log", format!("{}: 1", label));
    Ok(Value::Undefined)
}
fn console_noop(_realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(Value::Undefined)
}
