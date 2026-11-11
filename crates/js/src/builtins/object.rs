//! Object constructor and Object.prototype.

use super::*;
use crate::value::V;
use crate::realm::SYM_TO_STRING_TAG;

pub fn register(realm: &mut Realm) {
    let op = realm.intrinsics.object_proto;
    realm.define_method(op, "hasOwnProperty", 1, has_own_property);
    realm.define_method(op, "isPrototypeOf", 1, is_prototype_of);
    realm.define_method(op, "propertyIsEnumerable", 1, property_is_enumerable);
    realm.define_method(op, "toString", 0, to_string);
    realm.define_method(op, "toLocaleString", 0, to_locale_string);
    realm.define_method(op, "valueOf", 0, value_of);
    realm.define_method(op, "__defineGetter__", 2, define_getter_legacy);
    realm.define_method(op, "__defineSetter__", 2, define_setter_legacy);
    realm.define_method(op, "__lookupGetter__", 1, lookup_getter_legacy);
    realm.define_method(op, "__lookupSetter__", 1, lookup_setter_legacy);
    realm.define_accessor(op, "__proto__", Some(proto_getter), Some(proto_setter), CONFIGURABLE);
    let ctor = define_ctor(realm, "Object", 1, object_ctor, op);
    realm.intrinsics.object_ctor = ctor;
    realm.define_method(ctor, "keys", 1, keys);
    realm.define_method(ctor, "values", 1, values);
    realm.define_method(ctor, "entries", 1, entries);
    realm.define_method(ctor, "assign", 2, assign);
    realm.define_method(ctor, "create", 2, create);
    realm.define_method(ctor, "defineProperty", 3, define_property);
    realm.define_method(ctor, "defineProperties", 2, define_properties);
    realm.define_method(ctor, "getOwnPropertyDescriptor", 2, get_own_property_descriptor);
    realm.define_method(ctor, "getOwnPropertyDescriptors", 1, get_own_property_descriptors);
    realm.define_method(ctor, "getOwnPropertyNames", 1, get_own_property_names);
    realm.define_method(ctor, "getOwnPropertySymbols", 1, get_own_property_symbols);
    realm.define_method(ctor, "getPrototypeOf", 1, get_prototype_of);
    realm.define_method(ctor, "setPrototypeOf", 2, set_prototype_of);
    realm.define_method(ctor, "freeze", 1, freeze);
    realm.define_method(ctor, "isFrozen", 1, is_frozen);
    realm.define_method(ctor, "seal", 1, seal);
    realm.define_method(ctor, "isSealed", 1, is_sealed);
    realm.define_method(ctor, "preventExtensions", 1, prevent_extensions);
    realm.define_method(ctor, "isExtensible", 1, is_extensible);
    realm.define_method(ctor, "is", 2, is);
    realm.define_method(ctor, "fromEntries", 1, from_entries);
    realm.define_method(ctor, "hasOwn", 2, has_own);
    realm.define_method(ctor, "groupBy", 2, group_by);
}

fn object_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let nt = realm.new_target();
    if let V::Object(nt) = nt.v() {
        if nt != realm.intrinsics.object_ctor {
            let proto = realm.proto_from_new_target(realm.intrinsics.object_proto);
            return Ok(Value::Object(realm.new_object_with_proto(Some(proto))));
        }
    }
    match arg(args, 0) {
        Value::Undefined | Value::Null => Ok(Value::Object(realm.new_object())),
        v => Ok(Value::Object(realm.to_object(v)?)),
    }
}

fn has_own_property(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let key = realm.to_property_key(arg(args, 0))?;
    let id = realm.to_object(this)?;
    let r = realm.get_own_property_any(id, &key)?.is_some();
    Ok(Value::Bool(r))
}

fn has_own(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = realm.to_object(arg(args, 0))?;
    let key = realm.to_property_key(arg(args, 1))?;
    let r = realm.get_own_property_any(id, &key)?.is_some();
    Ok(Value::Bool(r))
}

fn is_prototype_of(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let v = match (arg(args, 0)).v() {
        V::Object(o) => o,
        _ => return Ok(Value::Bool(false)),
    };
    let me = realm.to_object(this)?;
    let mut cur = realm.get_prototype_of(v)?;
    while let Some(p) = cur {
        if p == me {
            return Ok(Value::Bool(true));
        }
        cur = realm.get_prototype_of(p)?;
    }
    Ok(Value::Bool(false))
}

fn property_is_enumerable(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let key = realm.to_property_key(arg(args, 0))?;
    let id = realm.to_object(this)?;
    let r = realm.get_own_property_any(id, &key)?.map(|p| p.enumerable()).unwrap_or(false);
    Ok(Value::Bool(r))
}

pub fn to_string(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let tag = match this.v() {
        V::Undefined => return Ok(Value::str("[object Undefined]")),
        V::Null => return Ok(Value::str("[object Null]")),
        _ => {
            let id = realm.to_object(this.clone())?;
            let builtin = class_of(realm, id);
            let t = realm.get(Value::Object(id), PropertyKey::Symbol(SYM_TO_STRING_TAG))?;
            match t.v() {
                V::String(s) => s.to_rust_string(),
                _ => builtin.to_string(),
            }
        }
    };
    Ok(Value::str(&format!("[object {}]", tag)))
}

fn to_locale_string(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    realm.invoke(this, "toString", &[])
}

fn value_of(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(Value::Object(realm.to_object(this)?))
}

fn proto_getter(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = realm.to_object(this)?;
    Ok(match realm.get_prototype_of(id)? {
        Some(p) => Value::Object(p),
        None => Value::Null,
    })
}

fn proto_setter(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    if let V::Object(id) = this.v() {
        match (arg(args, 0)).v() {
            V::Object(p) => {
                set_proto_checked(realm, id, Some(p))?;
            }
            V::Null => {
                set_proto_checked(realm, id, None)?;
            }
            _ => {}
        }
    }
    Ok(Value::Undefined)
}

fn set_proto_checked(realm: &mut Realm, id: ObjectId, proto: Option<ObjectId>) -> JsResult<bool> {
    if realm.obj(id).proto == proto {
        return Ok(true);
    }
    if !realm.obj(id).extensible {
        return Ok(false);
    }
    // cycle check
    let mut cur = proto;
    while let Some(c) = cur {
        if c == id {
            return realm.type_error("Cyclic __proto__ value");
        }
        cur = realm.obj(c).proto;
    }
    realm.obj_mut(id).set_proto(proto);
    Ok(true)
}

fn define_getter_legacy(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = realm.to_object(this)?;
    let key = realm.to_property_key(arg(args, 0))?;
    let f = arg(args, 1);
    if !realm.is_callable(&f) {
        return realm.type_error("Getter must be a function");
    }
    let existing = realm.get_own_property(id, &key);
    let setter = existing.and_then(|p| p.setter_cloned());
    realm.define_own(id, key, Property::accessor(Some(f), setter, ENUMERABLE | CONFIGURABLE))?;
    Ok(Value::Undefined)
}

fn define_setter_legacy(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = realm.to_object(this)?;
    let key = realm.to_property_key(arg(args, 0))?;
    let f = arg(args, 1);
    if !realm.is_callable(&f) {
        return realm.type_error("Setter must be a function");
    }
    let existing = realm.get_own_property(id, &key);
    let getter = existing.and_then(|p| p.getter_cloned());
    realm.define_own(id, key, Property::accessor(getter, Some(f), ENUMERABLE | CONFIGURABLE))?;
    Ok(Value::Undefined)
}

fn lookup_getter_legacy(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = realm.to_object(this)?;
    let key = realm.to_property_key(arg(args, 0))?;
    let mut cur = Some(id);
    while let Some(c) = cur {
        if let Some(p) = realm.get_own_property(c, &key) {
            return Ok(p.getter_cloned().unwrap_or(Value::Undefined));
        }
        cur = realm.obj(c).proto;
    }
    Ok(Value::Undefined)
}

fn lookup_setter_legacy(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = realm.to_object(this)?;
    let key = realm.to_property_key(arg(args, 0))?;
    let mut cur = Some(id);
    while let Some(c) = cur {
        if let Some(p) = realm.get_own_property(c, &key) {
            return Ok(p.setter_cloned().unwrap_or(Value::Undefined));
        }
        cur = realm.obj(c).proto;
    }
    Ok(Value::Undefined)
}

// ---------------------------------------------------------------- statics

fn keys(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = realm.to_object(arg(args, 0))?;
    let ks = realm.own_enumerable_string_keys(id)?;
    let vals: Vec<Value> = ks.into_iter().map(|k| k.to_value()).collect();
    Ok(realm.new_array_value(vals))
}

fn values(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = realm.to_object(arg(args, 0))?;
    let ks = realm.own_enumerable_string_keys(id)?;
    let mut vals = Vec::with_capacity(ks.len());
    for k in ks {
        vals.push(realm.get(Value::Object(id), k)?);
    }
    Ok(realm.new_array_value(vals))
}

fn entries(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = realm.to_object(arg(args, 0))?;
    let ks = realm.own_enumerable_string_keys(id)?;
    let mut vals = Vec::with_capacity(ks.len());
    for k in ks {
        let v = realm.get(Value::Object(id), k.clone())?;
        let pair = realm.new_array_value(vec![k.to_value(), v]);
        vals.push(pair);
    }
    Ok(realm.new_array_value(vals))
}

fn assign(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let target = realm.to_object(arg(args, 0))?;
    for src in args.iter().skip(1) {
        let sid = match src.v() {
            V::Undefined | V::Null => continue,
            V::Object(id) => id,
            prim => realm.to_object(prim.to_value())?,
        };
        let keys = realm.own_keys(sid)?;
        for k in keys {
            let p = realm.get_own_property_any(sid, &k)?;
            if let Some(p) = p {
                if p.enumerable() {
                    let v = realm.get_from_object(sid, &k, Value::Object(sid))?;
                    let ok = realm.set_on_object(target, k.clone(), v, Value::Object(target))?;
                    if !ok {
                        let kd = realm.key_display(&k);
                        return realm.type_error(&format!("Cannot assign to read only property '{}' of object", kd));
                    }
                }
            }
        }
    }
    Ok(Value::Object(target))
}

fn create(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let proto = match (arg(args, 0)).v() {
        V::Object(p) => Some(p),
        V::Null => None,
        _ => return realm.type_error("Object prototype may only be an Object or null"),
    };
    let o = realm.new_object_with_proto(proto);
    if let v @ V::Object(_) = (arg(args, 1)).v() {
        define_properties_impl(realm, o, v.to_value())?;
    }
    Ok(Value::Object(o))
}

fn define_property(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = match (arg(args, 0)).v() {
        V::Object(id) => id,
        _ => return realm.type_error("Object.defineProperty called on non-object"),
    };
    let key = realm.to_property_key(arg(args, 1))?;
    let existing = realm.get_own_property_any(id, &key)?;
    let desc = realm.to_property_descriptor_merged(arg(args, 2), existing.as_ref())?;
    let ok = realm.define_own(id, key.clone(), desc)?;
    if !ok {
        let k = realm.key_display(&key);
        return realm.type_error(&format!("Cannot redefine property: {}", k));
    }
    Ok(Value::Object(id))
}

fn define_properties_impl(realm: &mut Realm, id: ObjectId, props: Value) -> JsResult<()> {
    let pid = realm.to_object(props)?;
    let keys = realm.own_keys(pid)?;
    for k in keys {
        let p = realm.get_own_property_any(pid, &k)?;
        if let Some(p) = p {
            if !p.enumerable() {
                continue;
            }
            let dv = realm.get_from_object(pid, &k, Value::Object(pid))?;
            let existing = realm.get_own_property_any(id, &k)?;
            let desc = realm.to_property_descriptor_merged(dv, existing.as_ref())?;
            let ok = realm.define_own(id, k.clone(), desc)?;
            if !ok {
                let kd = realm.key_display(&k);
                return realm.type_error(&format!("Cannot redefine property: {}", kd));
            }
        }
    }
    Ok(())
}

fn define_properties(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = match (arg(args, 0)).v() {
        V::Object(id) => id,
        _ => return realm.type_error("Object.defineProperties called on non-object"),
    };
    define_properties_impl(realm, id, arg(args, 1))?;
    Ok(Value::Object(id))
}

fn get_own_property_descriptor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = realm.to_object(arg(args, 0))?;
    let key = realm.to_property_key(arg(args, 1))?;
    match realm.get_own_property_any(id, &key)? {
        Some(p) => Ok(realm.from_property_descriptor(&p)),
        None => Ok(Value::Undefined),
    }
}

fn get_own_property_descriptors(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = realm.to_object(arg(args, 0))?;
    let out = realm.new_object();
    let keys = realm.own_keys(id)?;
    for k in keys {
        if let Some(p) = realm.get_own_property_any(id, &k)? {
            let d = realm.from_property_descriptor(&p);
            realm.define_property(out, k, d, ALL);
        }
    }
    Ok(Value::Object(out))
}

fn get_own_property_names(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = realm.to_object(arg(args, 0))?;
    let keys = realm.own_keys(id)?;
    let vals: Vec<Value> = keys.into_iter().filter(|k| !k.is_symbol()).map(|k| k.to_value()).collect();
    Ok(realm.new_array_value(vals))
}

fn get_own_property_symbols(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = realm.to_object(arg(args, 0))?;
    let keys = realm.own_keys(id)?;
    let vals: Vec<Value> = keys.into_iter().filter(|k| k.is_symbol()).map(|k| k.to_value()).collect();
    Ok(realm.new_array_value(vals))
}

fn get_prototype_of(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = realm.to_object(arg(args, 0))?;
    Ok(match realm.get_prototype_of(id)? {
        Some(p) => Value::Object(p),
        None => Value::Null,
    })
}

fn set_prototype_of(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let target = arg(args, 0);
    let proto = match (arg(args, 1)).v() {
        V::Object(p) => Some(p),
        V::Null => None,
        _ => return realm.type_error("Object prototype may only be an Object or null"),
    };
    if let V::Object(id) = target.v() {
        if !set_proto_checked(realm, id, proto)? {
            return realm.type_error("Object.setPrototypeOf: object is not extensible");
        }
    } else if target.is_nullish() {
        return realm.type_error("Object.setPrototypeOf called on null or undefined");
    }
    Ok(target)
}

fn freeze(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let v = arg(args, 0);
    if let V::Object(id) = v.v() {
        let id = id;
        materialize_array(realm, id);
        let keys = realm.own_keys(id)?;
        for k in keys {
            if let Some(mut p) = realm.get_own_property(id, &k) {
                p.flags &= !CONFIGURABLE;
                if !p.is_accessor() {
                    p.flags &= !WRITABLE;
                }
                if let ObjectKind::Array { .. } = &realm.obj(id).kind {
                    if k.as_array_index().is_some() {
                        // dense elements moved to props by materialize_array
                    }
                }
                realm.obj_mut(id).props.insert(k, p);
            }
        }
        realm.obj_mut(id).extensible = false;
    }
    Ok(v)
}

/// Move dense array elements into the property map so per-element flags can be stored.
fn materialize_array(realm: &mut Realm, id: ObjectId) {
    let elements = match &mut realm.obj_mut(id).kind {
        ObjectKind::Array { elements, .. } => std::mem::take(elements),
        _ => return,
    };
    for (i, v) in elements.into_iter().enumerate() {
        if !v.is_empty() {
            realm.obj_mut(id).props.insert(PropertyKey::index(i as u32), Property::data(v, ALL));
        }
    }
}

fn is_frozen(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = match (arg(args, 0)).v() {
        V::Object(id) => id,
        _ => return Ok(Value::Bool(true)),
    };
    if realm.obj(id).extensible {
        return Ok(Value::Bool(false));
    }
    let keys = realm.own_keys(id)?;
    for k in keys {
        if let Some(p) = realm.get_own_property(id, &k) {
            if p.configurable() || (!p.is_accessor() && p.writable()) {
                return Ok(Value::Bool(false));
            }
        }
    }
    Ok(Value::Bool(true))
}

fn seal(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let v = arg(args, 0);
    if let V::Object(id) = v.v() {
        let id = id;
        materialize_array(realm, id);
        let keys = realm.own_keys(id)?;
        for k in keys {
            if let Some(mut p) = realm.get_own_property(id, &k) {
                p.flags &= !CONFIGURABLE;
                realm.obj_mut(id).props.insert(k, p);
            }
        }
        realm.obj_mut(id).extensible = false;
    }
    Ok(v)
}

fn is_sealed(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = match (arg(args, 0)).v() {
        V::Object(id) => id,
        _ => return Ok(Value::Bool(true)),
    };
    if realm.obj(id).extensible {
        return Ok(Value::Bool(false));
    }
    let keys = realm.own_keys(id)?;
    for k in keys {
        if let Some(p) = realm.get_own_property(id, &k) {
            if p.configurable() {
                return Ok(Value::Bool(false));
            }
        }
    }
    Ok(Value::Bool(true))
}

fn prevent_extensions(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let v = arg(args, 0);
    if let V::Object(id) = v.v() {
        realm.obj_mut(id).extensible = false;
    }
    Ok(v)
}

fn is_extensible(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(match (arg(args, 0)).v() {
        V::Object(id) => realm.obj(id).extensible,
        _ => false,
    }))
}

fn is(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(realm.same_value(&arg(args, 0), &arg(args, 1))))
}

fn from_entries(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let out = realm.new_object();
    let items = realm.iterate_to_vec(arg(args, 0))?;
    for item in items {
        let k = realm.get(item.clone(), PropertyKey::index(0))?;
        let v = realm.get(item, PropertyKey::index(1))?;
        let key = realm.to_property_key(k)?;
        realm.define_own(out, key, Property::data(v, ALL))?;
    }
    Ok(Value::Object(out))
}

fn group_by(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let items = realm.iterate_to_vec(arg(args, 0))?;
    let cb = arg(args, 1);
    let out = realm.new_object_with_proto(None);
    for (i, item) in items.into_iter().enumerate() {
        let k = realm.call(cb.clone(), Value::Undefined, &[item.clone(), Value::Number(i as f64)])?;
        let key = realm.to_property_key(k)?;
        let existing = realm.get_own_property(out, &key);
        match existing {
            Some(p) => {
                if let V::Object(arr) = p.value.v() {
                    if let ObjectKind::Array { elements, length } = &mut realm.obj_mut(arr).kind {
                        elements.push(item);
                        *length = elements.len() as u32;
                    }
                }
            }
            None => {
                let arr = realm.new_array_value(vec![item]);
                realm.define_property(out, key, arr, ALL);
            }
        }
    }
    Ok(Value::Object(out))
}
