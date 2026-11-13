//! Function.prototype and the Function constructor.

use super::*;
use crate::value::V;
use crate::realm::SYM_HAS_INSTANCE;

pub fn register(realm: &mut Realm) {
    let fp = realm.intrinsics.function_proto;
    realm.define_property(fp, "length", Value::Number(0.0), CONFIGURABLE);
    realm.define_property(fp, "name", Value::str(""), CONFIGURABLE);
    realm.define_method(fp, "call", 1, call);
    realm.define_method(fp, "apply", 2, apply);
    // Whole applications run inside `f.call(...)` / `f.apply(...)` reached
    // through other natives: they hold nothing but their arguments.
    realm.mark_gc_safe(fp, "call");
    realm.mark_gc_safe(fp, "apply");
    realm.define_method(fp, "bind", 1, bind);
    realm.define_method(fp, "toString", 0, to_string);
    let hi = realm.new_native_function("[Symbol.hasInstance]", 1, has_instance);
    realm.define_property(fp, PropertyKey::Symbol(SYM_HAS_INSTANCE), Value::Object(hi), 0);
    // %ThrowTypeError%: unique per realm, frozen, anonymous.
    let tte = realm.new_native_function("", 0, throw_type_error);
    realm.define_property(tte, "length", Value::Number(0.0), 0);
    realm.define_property(tte, "name", Value::str(""), 0);
    realm.obj_mut(tte).extensible = false;
    realm.intrinsics.others.insert("ThrowTypeError", tte);
    for name in ["caller", "arguments"] {
        realm.define_accessor_values(fp, name, Some(Value::Object(tte)), Some(Value::Object(tte)), CONFIGURABLE);
    }
    let ctor = define_ctor(realm, "Function", 1, function_ctor, fp);
    realm.intrinsics.function_ctor = ctor;

    // Generator / async function prototypes (the "constructor-less" ones).
    let gfp = realm.new_object_with_proto(Some(fp));
    let gp = realm.intrinsics.generator_proto;
    realm.define_property(gfp, "prototype", Value::Object(gp), CONFIGURABLE);
    realm.define_property(gp, "constructor", Value::Object(gfp), CONFIGURABLE);
    realm.define_property(gfp, PropertyKey::Symbol(crate::realm::SYM_TO_STRING_TAG), Value::str("GeneratorFunction"), CONFIGURABLE);
    realm.intrinsics.others.insert("GeneratorFunctionPrototype", gfp);
    let agfp = realm.new_object_with_proto(Some(fp));
    let agp = realm.intrinsics.async_generator_proto;
    realm.define_property(agfp, "prototype", Value::Object(agp), CONFIGURABLE);
    realm.define_property(agp, "constructor", Value::Object(agfp), CONFIGURABLE);
    realm.define_property(agfp, PropertyKey::Symbol(crate::realm::SYM_TO_STRING_TAG), Value::str("AsyncGeneratorFunction"), CONFIGURABLE);
    realm.intrinsics.others.insert("AsyncGeneratorFunctionPrototype", agfp);
    let afp = realm.new_object_with_proto(Some(fp));
    realm.define_property(afp, PropertyKey::Symbol(crate::realm::SYM_TO_STRING_TAG), Value::str("AsyncFunction"), CONFIGURABLE);
    realm.intrinsics.others.insert("AsyncFunctionPrototype", afp);
    // The (non-global) constructors reached through `.constructor`:
    // `new AsyncFunction(params..., body)` and friends (Alpine.js builds its
    // expression evaluators with AsyncFunction).
    for (proto, name, f) in [
        (gfp, "GeneratorFunction", generator_function_ctor as NativeFn),
        (agfp, "AsyncGeneratorFunction", async_generator_function_ctor as NativeFn),
        (afp, "AsyncFunction", async_function_ctor as NativeFn),
    ] {
        let c = realm.new_native_constructor(name, 1, f, proto);
        let fc = realm.intrinsics.function_ctor;
        realm.obj_mut(c).set_proto(Some(fc));
        realm.define_property(proto, "constructor", Value::Object(c), CONFIGURABLE);
    }
}

fn throw_type_error(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    // Web compatibility: `sloppyFunction.caller` / `.arguments` read null
    // (browsers give such functions own null-valued properties).
    if let V::Object(id) = this.v() {
        if let ObjectKind::Function(c) = &realm.obj(id).kind {
            let f = &c.code.flags;
            if !f.is_strict && !f.is_arrow && !f.is_async && !f.is_generator && !f.is_method && !f.is_class_constructor {
                return Ok(Value::Null);
            }
        }
    }
    realm.type_error("'caller', 'callee', and 'arguments' properties may not be accessed on strict mode functions or the arguments objects for calls to them")
}

fn async_function_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    create_dynamic_function(realm, args, "async function")
}

fn generator_function_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    create_dynamic_function(realm, args, "function*")
}

fn async_generator_function_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    create_dynamic_function(realm, args, "async function*")
}

fn call(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    if !realm.is_callable(&this) {
        return realm.type_error("Function.prototype.call called on non-function");
    }
    let t = arg(args, 0);
    let rest = if args.len() > 1 { &args[1..] } else { &[] };
    realm.call(this, t, rest)
}

fn apply(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    if !realm.is_callable(&this) {
        return realm.type_error("Function.prototype.apply was called on a non-function");
    }
    let t = arg(args, 0);
    let list = match (arg(args, 1)).v() {
        V::Undefined | V::Null => Vec::new(),
        v @ V::Object(_) => realm.create_list_from_array_like(v.to_value())?,
        _ => return realm.type_error("CreateListFromArrayLike called on non-object"),
    };
    realm.call(this, t, &list)
}

fn bind(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let target = match this.v() {
        V::Object(id) if realm.is_callable(&this) => id,
        _ => return realm.type_error("Bind must be called on a function"),
    };
    let bound_this = arg(args, 0);
    let bound_args: Vec<Value> = if args.len() > 1 { args[1..].to_vec() } else { Vec::new() };
    let proto = realm.get_prototype_of(target)?;
    let n_bound = bound_args.len();
    let b = realm.alloc(Object::new(proto, ObjectKind::Bound(Box::new(BoundFunction { target, this: bound_this, args: bound_args }))));
    // length: only an own "length" counts; ToIntegerOrInfinity of a number.
    let mut len = 0.0;
    if realm.get_own_property_any(target, &crate::value::names::length())?.is_some() {
        if let V::Number(n) = realm.get(Value::Object(target), crate::value::names::length())?.v() {
            len = if n == f64::INFINITY { n } else if n == f64::NEG_INFINITY { 0.0 } else { (to_integer(n) - n_bound as f64).max(0.0) };
        }
    }
    realm.define_property(b, "length", Value::Number(len), CONFIGURABLE);
    let name = realm.get(Value::Object(target), crate::value::names::name())?;
    let name = match name.v() {
        V::String(s) => s.to_rust_string(),
        _ => String::new(),
    };
    realm.define_property(b, "name", Value::str(&format!("bound {}", name)), CONFIGURABLE);
    Ok(Value::Object(b))
}

fn to_string(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = match this.v() {
        V::Object(id) if realm.is_callable(&this) => id,
        _ => return realm.type_error("Function.prototype.toString requires that 'this' be a Function"),
    };
    let s = match &realm.obj(id).kind {
        ObjectKind::Function(c) => match (&c.code.source, c.code.source_span) {
            (Some(src), Some((a, b))) if src.get(a as usize..b as usize).is_some() => src[a as usize..b as usize].to_string(),
            _ if c.code.flags.is_class_constructor => format!("class {} {{ [native code] }}", c.code.name),
            _ => format!("function {}() {{ [native code] }}", c.code.name),
        },
        ObjectKind::Native(n) => {
            // NativeFunction syntax: the name only when it is an (optionally
            // get/set-prefixed) identifier name or a [symbol] name.
            let name = n.name.to_rust_string();
            let bare = name.strip_prefix("get ").or_else(|| name.strip_prefix("set ")).unwrap_or(&name);
            let ok = bare.starts_with('[') && bare.ends_with(']') || bare.chars().enumerate().all(|(i, c)| c == '$' || c == '_' || c.is_alphabetic() || (i > 0 && c.is_alphanumeric()));
            if ok { format!("function {}() {{ [native code] }}", name) } else { "function () { [native code] }".to_string() }
        }
        _ => "function () { [native code] }".to_string(),
    };
    Ok(Value::str(&s))
}

fn has_instance(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let r = realm.ordinary_has_instance(this, arg(args, 0))?;
    Ok(Value::Bool(r))
}

/// `new Function(p1, p2, body)`
fn function_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    create_dynamic_function(realm, args, "function")
}

pub fn create_dynamic_function(realm: &mut Realm, args: &[Value], kind: &str) -> JsResult<Value> {
    let mut params = Vec::new();
    let mut body = String::new();
    if !args.is_empty() {
        for a in &args[..args.len() - 1] {
            params.push(realm.to_rust_string(a.clone())?);
        }
        body = realm.to_rust_string(args[args.len() - 1].clone())?;
    }
    let src = format!("({} anonymous({}\n) {{\n{}\n}})", kind, params.join(","), body);
    let code = realm.compile(&src, "Function", false)?;
    let nt = realm.new_target();
    let proto = if nt.is_object() {
        let fallback = match kind {
            "function" => realm.intrinsics.function_proto,
            "function*" => realm.intrinsics.others["GeneratorFunctionPrototype"],
            "async function*" => realm.intrinsics.others["AsyncGeneratorFunctionPrototype"],
            _ => realm.intrinsics.others["AsyncFunctionPrototype"],
        };
        Some(realm.proto_from_ctor(nt, fallback)?)
    } else {
        None
    };
    let f = realm.run_script(code)?;
    if let (Some(p), V::Object(id)) = (proto, f.v()) {
        realm.obj_mut(id).set_proto(Some(p));
    }
    Ok(f)
}
