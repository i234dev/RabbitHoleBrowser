//! Error constructors.

use super::*;
use crate::value::V;

const KINDS: [&str; 7] = ["Error", "TypeError", "RangeError", "SyntaxError", "ReferenceError", "EvalError", "URIError"];

pub fn register(realm: &mut Realm) {
    let ep = realm.intrinsics.error_proto;
    realm.define_property(ep, "name", Value::str("Error"), HIDDEN);
    realm.define_property(ep, "message", Value::str(""), HIDDEN);
    realm.define_method(ep, "toString", 0, to_string);
    let error_ctor = define_ctor(realm, "Error", 1, error_ctor, ep);
    realm.define_method(error_ctor, "captureStackTrace", 1, capture_stack_trace);
    realm.define_property(error_ctor, "stackTraceLimit", Value::Number(10.0), ALL);
    realm.intrinsics.others.insert("Error", error_ctor);
    for kind in KINDS.iter().skip(1) {
        let proto = realm.new_object_with_proto(Some(ep));
        realm.define_property(proto, "name", Value::str(kind), HIDDEN);
        realm.define_property(proto, "message", Value::str(""), HIDDEN);
        let f: NativeFn = match *kind {
            "TypeError" => type_error_ctor,
            "RangeError" => range_error_ctor,
            "SyntaxError" => syntax_error_ctor,
            "ReferenceError" => reference_error_ctor,
            "EvalError" => eval_error_ctor,
            _ => uri_error_ctor,
        };
        let c = define_ctor(realm, kind, 1, f, proto);
        realm.obj_mut(c).set_proto(Some(error_ctor));
        let key: &'static str = Box::leak(format!("{}Prototype", kind).into_boxed_str());
        realm.intrinsics.others.insert(key, proto);
        realm.intrinsics.others.insert(kind, c);
    }
    // AggregateError
    let ap = realm.new_object_with_proto(Some(ep));
    realm.define_property(ap, "name", Value::str("AggregateError"), HIDDEN);
    realm.define_property(ap, "message", Value::str(""), HIDDEN);
    let c = define_ctor(realm, "AggregateError", 2, aggregate_error_ctor, ap);
    realm.obj_mut(c).set_proto(Some(error_ctor));
    realm.intrinsics.others.insert("AggregateErrorPrototype", ap);
    realm.intrinsics.others.insert("AggregateError", c);
}

fn make_error(realm: &mut Realm, kind: &str, args: &[Value], msg_index: usize) -> JsResult<Value> {
    let default_proto = realm.error_proto_for(kind);
    let proto = realm.proto_from_new_target(default_proto);
    let id = realm.alloc(Object::new(Some(proto), ObjectKind::Error));
    let msg = arg(args, msg_index);
    let msg_str = if msg.is_undefined() {
        String::new()
    } else {
        let s = realm.to_string(msg)?;
        realm.define_property(id, "message", Value::String(s.clone()), HIDDEN);
        s.to_rust_string()
    };
    if let V::Object(opts) = (arg(args, msg_index + 1)).v() {
        if realm.has_property(opts, &PropertyKey::str("cause"))? {
            let cause = realm.get_str(Value::Object(opts), "cause")?;
            realm.define_property(id, "cause", cause, HIDDEN);
        }
    }
    // name for stack: from the prototype chain
    let name = realm.get_str(Value::Object(id), "name")?;
    let name = match name.v() {
        V::String(s) => s.to_rust_string(),
        _ => kind.to_string(),
    };
    let stack = realm.build_stack_string(&name, &msg_str);
    realm.define_property(id, "stack", Value::str(&stack), HIDDEN);
    Ok(Value::Object(id))
}

fn error_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    make_error(realm, "Error", args, 0)
}
fn type_error_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    make_error(realm, "TypeError", args, 0)
}
fn range_error_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    make_error(realm, "RangeError", args, 0)
}
fn syntax_error_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    make_error(realm, "SyntaxError", args, 0)
}
fn reference_error_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    make_error(realm, "ReferenceError", args, 0)
}
fn eval_error_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    make_error(realm, "EvalError", args, 0)
}
fn uri_error_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    make_error(realm, "URIError", args, 0)
}
fn aggregate_error_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let e = make_error(realm, "AggregateError", args, 1)?;
    let errors = realm.iterate_to_vec(arg(args, 0))?;
    let arr = realm.new_array_value(errors);
    if let V::Object(id) = e.v() {
        realm.define_property(id, "errors", arr, HIDDEN);
    }
    Ok(e)
}

fn to_string(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    if !this.is_object() {
        return realm.type_error("Error.prototype.toString called on non-object");
    }
    let name = realm.get_str(this.clone(), "name")?;
    let name = if name.is_undefined() { "Error".to_string() } else { realm.to_rust_string(name)? };
    let msg = realm.get_str(this, "message")?;
    let msg = if msg.is_undefined() { String::new() } else { realm.to_rust_string(msg)? };
    Ok(Value::str(&if name.is_empty() {
        msg
    } else if msg.is_empty() {
        name
    } else {
        format!("{}: {}", name, msg)
    }))
}

fn capture_stack_trace(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if let V::Object(id) = (arg(args, 0)).v() {
        let stack = realm.build_stack_string("Error", "");
        realm.define_property(id, "stack", Value::str(&stack), HIDDEN);
    }
    Ok(Value::Undefined)
}
