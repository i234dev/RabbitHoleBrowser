//! Reflect and Proxy.

use super::*;
use crate::value::V;
use crate::realm::SYM_TO_STRING_TAG;

pub fn register(realm: &mut Realm) {
    let r = realm.new_object();
    realm.define_global("Reflect", Value::Object(r));
    realm.define_property(r, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("Reflect"), CONFIGURABLE);
    realm.define_method(r, "apply", 3, apply);
    realm.define_method(r, "construct", 2, construct);
    realm.define_method(r, "defineProperty", 3, define_property);
    realm.define_method(r, "deleteProperty", 2, delete_property);
    realm.define_method(r, "get", 2, get);
    realm.define_method(r, "getOwnPropertyDescriptor", 2, get_own_property_descriptor);
    realm.define_method(r, "getPrototypeOf", 1, get_prototype_of);
    realm.define_method(r, "has", 2, has);
    realm.define_method(r, "isExtensible", 1, is_extensible);
    realm.define_method(r, "ownKeys", 1, own_keys);
    realm.define_method(r, "preventExtensions", 1, prevent_extensions);
    realm.define_method(r, "set", 3, set);
    realm.define_method(r, "setPrototypeOf", 2, set_prototype_of);

    let proxy = realm.new_native_function_full("Proxy", 2, proxy_ctor, None, vec![], true);
    realm.define_global("Proxy", Value::Object(proxy));
    realm.define_method(proxy, "revocable", 2, revocable);
}

fn target_obj(realm: &mut Realm, v: &Value, name: &str) -> JsResult<ObjectId> {
    match v.v() {
        V::Object(id) => Ok(id),
        _ => realm.type_error(&format!("Reflect.{} called on non-object", name)),
    }
}

fn apply(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let f = arg(args, 0);
    if !realm.is_callable(&f) {
        return realm.type_error("Reflect.apply target is not callable");
    }
    let list = realm.create_list_from_array_like(arg(args, 2))?;
    realm.call(f, arg(args, 1), &list)
}

fn construct(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let f = arg(args, 0);
    if !realm.is_constructor(&f) {
        return realm.type_error("Reflect.construct target is not a constructor");
    }
    let nt = if args.len() > 2 { arg(args, 2) } else { f.clone() };
    if !realm.is_constructor(&nt) {
        return realm.type_error("Reflect.construct newTarget is not a constructor");
    }
    let list = realm.create_list_from_array_like(arg(args, 1))?;
    realm.construct(f, &list, nt)
}

fn define_property(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = target_obj(realm, &arg(args, 0), "defineProperty")?;
    let key = realm.to_property_key(arg(args, 1))?;
    let existing = realm.get_own_property_any(id, &key)?;
    let desc = realm.to_property_descriptor_merged(arg(args, 2), existing.as_ref())?;
    Ok(Value::Bool(realm.define_own(id, key, desc)?))
}

fn delete_property(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = target_obj(realm, &arg(args, 0), "deleteProperty")?;
    let key = realm.to_property_key(arg(args, 1))?;
    Ok(Value::Bool(realm.delete_property(id, &key)?))
}

fn get(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = target_obj(realm, &arg(args, 0), "get")?;
    let key = realm.to_property_key(arg(args, 1))?;
    let receiver = if args.len() > 2 { arg(args, 2) } else { Value::Object(id) };
    realm.get_from_object(id, &key, receiver)
}

fn get_own_property_descriptor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = target_obj(realm, &arg(args, 0), "getOwnPropertyDescriptor")?;
    let key = realm.to_property_key(arg(args, 1))?;
    match realm.get_own_property_any(id, &key)? {
        Some(p) => Ok(realm.from_property_descriptor(&p)),
        None => Ok(Value::Undefined),
    }
}

fn get_prototype_of(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = target_obj(realm, &arg(args, 0), "getPrototypeOf")?;
    Ok(match realm.get_prototype_of(id)? {
        Some(p) => Value::Object(p),
        None => Value::Null,
    })
}

fn has(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = target_obj(realm, &arg(args, 0), "has")?;
    let key = realm.to_property_key(arg(args, 1))?;
    Ok(Value::Bool(realm.has_property(id, &key)?))
}

fn is_extensible(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = target_obj(realm, &arg(args, 0), "isExtensible")?;
    Ok(Value::Bool(realm.obj(id).extensible))
}

fn own_keys(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = target_obj(realm, &arg(args, 0), "ownKeys")?;
    let keys = realm.own_keys(id)?;
    let vals: Vec<Value> = keys.into_iter().map(|k| k.to_value()).collect();
    Ok(realm.new_array_value(vals))
}

fn prevent_extensions(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = target_obj(realm, &arg(args, 0), "preventExtensions")?;
    if !super::typed_array::check_integrity(realm, id, false)? {
        return Ok(Value::Bool(false));
    }
    realm.obj_mut(id).extensible = false;
    Ok(Value::Bool(true))
}

fn set(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = target_obj(realm, &arg(args, 0), "set")?;
    let key = realm.to_property_key(arg(args, 1))?;
    let receiver = if args.len() > 3 { arg(args, 3) } else { Value::Object(id) };
    Ok(Value::Bool(realm.set_on_object(id, key, arg(args, 2), receiver)?))
}

fn set_prototype_of(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = target_obj(realm, &arg(args, 0), "setPrototypeOf")?;
    let proto = match (arg(args, 1)).v() {
        V::Object(p) => Some(p),
        V::Null => None,
        _ => return realm.type_error("Object prototype may only be an Object or null"),
    };
    if !realm.obj(id).extensible && realm.obj(id).proto != proto {
        return Ok(Value::Bool(false));
    }
    realm.obj_mut(id).set_proto(proto);
    Ok(Value::Bool(true))
}

fn proxy_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if !realm.is_construct_call() {
        return realm.type_error("Constructor Proxy requires 'new'");
    }
    create_proxy(realm, arg(args, 0), arg(args, 1))
}

fn create_proxy(realm: &mut Realm, target: Value, handler: Value) -> JsResult<Value> {
    let (t, h) = match (target.v(), handler.v()) {
        (V::Object(t), V::Object(h)) => (t, h),
        _ => return realm.type_error("Cannot create proxy with a non-object as target or handler"),
    };
    let p = realm.alloc(Object::new(None, ObjectKind::Proxy { target: t, handler: h, revoked: false }));
    Ok(Value::Object(p))
}

fn revocable(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let p = create_proxy(realm, arg(args, 0), arg(args, 1))?;
    let revoke = realm.new_native_function_full("", 0, revoke_fn, None, vec![p.clone()], false);
    let o = realm.new_object();
    realm.define_property(o, "proxy", p, ALL);
    realm.define_property(o, "revoke", Value::Object(revoke), ALL);
    Ok(Value::Object(o))
}

fn revoke_fn(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    if let Some(V::Object(p)) = (realm.current_native().and_then(|n| realm.native_captures(n).first().cloned())).as_ref().map(Value::v) {
        if let ObjectKind::Proxy { revoked, .. } = &mut realm.obj_mut(p).kind {
            *revoked = true;
        }
    }
    Ok(Value::Undefined)
}
