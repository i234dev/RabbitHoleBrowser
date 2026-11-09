//! Function.prototype and the Function constructor.

use super::*;
use crate::realm::SYM_HAS_INSTANCE;

pub fn register(realm: &mut Realm) {
    let fp = realm.intrinsics.function_proto;
    realm.define_property(fp, "length", Value::Number(0.0), CONFIGURABLE);
    realm.define_property(fp, "name", Value::str(""), CONFIGURABLE);
    realm.define_method(fp, "call", 1, call);
    realm.define_method(fp, "apply", 2, apply);
    realm.define_method(fp, "bind", 1, bind);
    realm.define_method(fp, "toString", 0, to_string);
    let hi = realm.new_native_function("[Symbol.hasInstance]", 1, has_instance);
    realm.define_property(fp, PropertyKey::Symbol(SYM_HAS_INSTANCE), Value::Object(hi), 0);
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
    let list = match arg(args, 1) {
        Value::Undefined | Value::Null => Vec::new(),
        v @ Value::Object(_) => realm.create_list_from_array_like(v)?,
        _ => return realm.type_error("CreateListFromArrayLike called on non-object"),
    };
    realm.call(this, t, &list)
}

fn bind(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let target = match this {
        Value::Object(id) if realm.is_callable(&this) => id,
        _ => return realm.type_error("Bind must be called on a function"),
    };
    let bound_this = arg(args, 0);
    let bound_args: Vec<Value> = if args.len() > 1 { args[1..].to_vec() } else { Vec::new() };
    let proto = realm.obj(target).proto;
    let n_bound = bound_args.len();
    let b = realm.alloc(Object::new(proto, ObjectKind::Bound(Box::new(BoundFunction { target, this: bound_this, args: bound_args }))));
    let len = realm.get_str(Value::Object(target), "length")?;
    let len = match len {
        Value::Number(n) => (n - n_bound as f64).max(0.0),
        _ => 0.0,
    };
    realm.define_property(b, "length", Value::Number(len), CONFIGURABLE);
    let name = realm.get_str(Value::Object(target), "name")?;
    let name = match name {
        Value::String(s) => s.to_rust_string(),
        _ => String::new(),
    };
    realm.define_property(b, "name", Value::str(&format!("bound {}", name)), CONFIGURABLE);
    Ok(Value::Object(b))
}

fn to_string(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = match this {
        Value::Object(id) if realm.is_callable(&this) => id,
        _ => return realm.type_error("Function.prototype.toString requires that 'this' be a Function"),
    };
    let s = match &realm.obj(id).kind {
        ObjectKind::Function(c) => match &c.code.source {
            Some(_) => format!("function {}() {{ [native code] }}", c.code.name),
            None => format!("function {}() {{ [native code] }}", c.code.name),
        },
        ObjectKind::Native(n) => format!("function {}() {{ [native code] }}", n.name),
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
    realm.run_script(code)
}
