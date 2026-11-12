//! Array constructor and Array.prototype.

use super::*;
use crate::value::V;
use crate::realm::{SYM_IS_CONCAT_SPREADABLE, SYM_ITERATOR, SYM_SPECIES, SYM_UNSCOPABLES};

pub fn register(realm: &mut Realm) {
    let ap = realm.intrinsics.array_proto;
    let ctor = define_ctor(realm, "Array", 1, array_ctor, ap);
    realm.intrinsics.array_ctor = ctor;
    realm.define_method(ctor, "isArray", 1, is_array);
    realm.define_method(ctor, "from", 1, from);
    realm.define_method(ctor, "of", 0, of);
    realm.define_accessor(ctor, PropertyKey::Symbol(SYM_SPECIES), Some(species), None, CONFIGURABLE);
    let methods: [(&str, u32, NativeFn); 38] = [
        ("at", 1, at),
        ("concat", 1, concat),
        ("copyWithin", 2, copy_within),
        ("entries", 0, entries),
        ("every", 1, every),
        ("fill", 1, fill),
        ("filter", 1, filter),
        ("find", 1, find),
        ("findIndex", 1, find_index),
        ("findLast", 1, find_last),
        ("findLastIndex", 1, find_last_index),
        ("flat", 0, flat),
        ("flatMap", 1, flat_map),
        ("forEach", 1, for_each),
        ("includes", 1, includes),
        ("indexOf", 1, index_of),
        ("join", 1, join),
        ("keys", 0, keys),
        ("lastIndexOf", 1, last_index_of),
        ("map", 1, map),
        ("pop", 0, pop),
        ("push", 1, push),
        ("reduce", 1, reduce),
        ("reduceRight", 1, reduce_right),
        ("reverse", 0, reverse),
        ("shift", 0, shift),
        ("slice", 2, slice),
        ("some", 1, some),
        ("sort", 1, sort),
        ("splice", 2, splice),
        ("toLocaleString", 0, to_locale_string),
        ("toString", 0, to_string),
        ("unshift", 1, unshift),
        ("toReversed", 0, to_reversed),
        ("toSorted", 1, to_sorted),
        ("toSpliced", 2, to_spliced),
        ("with", 2, with),
        ("values", 0, values),
    ];
    for (name, arity, f) in methods {
        realm.define_method(ap, name, arity, f);
    }
    let values_fn = realm.obj(ap).props.get(&PropertyKey::str("values")).unwrap().value.clone();
    realm.define_property(ap, PropertyKey::Symbol(SYM_ITERATOR), values_fn.clone(), HIDDEN);
    realm.intrinsics.others.insert("ArrayProtoValues", values_fn.as_object().unwrap());
    let unscopables = realm.new_object_with_proto(None);
    for n in ["at", "copyWithin", "entries", "fill", "find", "findIndex", "findLast", "findLastIndex", "flat", "flatMap", "includes", "keys", "toReversed", "toSorted", "toSpliced", "values"] {
        realm.define_property(unscopables, n, Value::Bool(true), ALL);
    }
    realm.define_property(ap, PropertyKey::Symbol(SYM_UNSCOPABLES), Value::Object(unscopables), CONFIGURABLE);
}

fn species(_realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(this)
}

fn array_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let default = realm.intrinsics.array_proto;
    let proto = realm.proto_from_new_target(default);
    let arr = if args.len() == 1 {
        match args[0].v() {
            V::Number(n) => {
                let len = n as u32;
                if len as f64 != n {
                    return realm.range_error("Invalid array length");
                }
                let elems = if len <= 1 << 20 { vec![Value::Empty; len as usize] } else { Vec::new() };
                realm.alloc(Object::new(Some(proto), ObjectKind::Array { elements: elems, length: len }))
            }
            v => realm.alloc(Object::new(Some(proto), ObjectKind::Array { elements: vec![v.to_value()], length: 1 })),
        }
    } else {
        realm.alloc(Object::new(Some(proto), ObjectKind::Array { elements: args.to_vec(), length: args.len() as u32 }))
    };
    Ok(Value::Object(arr))
}

pub fn is_array_value(realm: &Realm, v: &Value) -> bool {
    match v.v() {
        V::Object(id) => match &realm.obj(id).kind {
            ObjectKind::Array { .. } => true,
            ObjectKind::Proxy { target, .. } => is_array_value(realm, &Value::Object(*target)),
            _ => false,
        },
        _ => false,
    }
}

fn is_array(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(is_array_value(realm, &arg(args, 0))))
}

fn from(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let source = arg(args, 0);
    let map_fn = arg(args, 1);
    let this_arg = arg(args, 2);
    let mapping = !map_fn.is_undefined();
    if mapping && !realm.is_callable(&map_fn) {
        return realm.type_error("Array.from: mapper is not a function");
    }
    let plain = this.as_object() == Some(realm.intrinsics.array_ctor) || !realm.is_constructor(&this);
    let uses_iterator = realm.get_method(source.clone(), PropertyKey::Symbol(SYM_ITERATOR))?.is_some();
    if uses_iterator {
        if plain {
            // A fresh array: defining its elements is unobservable, so
            // collect first (mapping as we go, closing on errors).
            if !mapping {
                let items = realm.iterate_to_vec(source)?;
                return Ok(realm.new_array_value(items));
            }
            let mut out = Vec::new();
            realm.for_each_iterated(source, |realm, v, i| {
                out.push(realm.call(map_fn.clone(), this_arg.clone(), &[v, Value::Number(i as f64)])?);
                Ok(())
            })?;
            return Ok(realm.new_array_value(out));
        }
        let a = realm.construct(this, &[], Value::Undefined)?;
        let Some(aid) = a.as_object() else { return realm.type_error("Array.from: constructor did not return an object") };
        let mut count = 0usize;
        realm.for_each_iterated(source, |realm, v, i| {
            let v = if mapping { realm.call(map_fn.clone(), this_arg.clone(), &[v, Value::Number(i as f64)])? } else { v };
            create_data_property_or_throw(realm, aid, i as f64, v)?;
            count = i + 1;
            Ok(())
        })?;
        set_length_or_throw(realm, a.clone(), count as f64)?;
        return Ok(a);
    }
    let obj = realm.to_object(source)?;
    let len = realm.length_of(Value::Object(obj))?;
    let a = if plain {
        if len > u32::MAX as usize {
            return realm.range_error("Invalid array length");
        }
        if !mapping {
            let items = realm.create_list_from_array_like(Value::Object(obj))?;
            return Ok(realm.new_array_value(items));
        }
        let mut out = Vec::with_capacity(len.min(1 << 20));
        // `Array.from({length: n}, fn)`: no element to read while the
        // object (and its prototypes) keep no index properties.
        let mut holes = realm.no_index_properties(obj);
        let mut nprops = realm.obj(obj).props.len();
        for i in 0..len {
            if holes && realm.obj(obj).props.len() != nprops {
                holes = realm.no_index_properties(obj);
                nprops = realm.obj(obj).props.len();
            }
            let v = if holes { Value::Undefined } else { realm.get(Value::Object(obj), index_key(i as f64))? };
            out.push(realm.call(map_fn.clone(), this_arg.clone(), &[v, Value::Number(i as f64)])?);
        }
        return Ok(realm.new_array_value(out));
    } else {
        realm.construct(this, &[Value::Number(len as f64)], Value::Undefined)?
    };
    let Some(aid) = a.as_object() else { return realm.type_error("Array.from: constructor did not return an object") };
    for i in 0..len {
        let v = realm.get(Value::Object(obj), index_key(i as f64))?;
        let v = if mapping { realm.call(map_fn.clone(), this_arg.clone(), &[v, Value::Number(i as f64)])? } else { v };
        create_data_property_or_throw(realm, aid, i as f64, v)?;
    }
    set_length_or_throw(realm, a.clone(), len as f64)?;
    Ok(a)
}

/// The property key of an integer index (array index or its string).
pub(crate) fn index_key(i: f64) -> PropertyKey {
    if i < u32::MAX as f64 {
        PropertyKey::index(i as u32)
    } else {
        PropertyKey::String(crate::value::JsStr::new(&crate::value::number_to_string(i)))
    }
}

/// CreateDataPropertyOrThrow(O, i, v).
pub(crate) fn create_data_property_or_throw(realm: &mut Realm, o: ObjectId, i: f64, v: Value) -> JsResult<()> {
    if !realm.define_own(o, index_key(i), Property::data(v, ALL))? {
        return realm.type_error(&format!("Cannot define property {}", crate::value::number_to_string(i)));
    }
    Ok(())
}

/// Set(O, "length", len, true).
pub(crate) fn set_length_or_throw(realm: &mut Realm, o: Value, len: f64) -> JsResult<()> {
    if !realm.set_str(o, "length", Value::Number(len))? {
        return realm.type_error("Cannot assign to read only property 'length'");
    }
    Ok(())
}

/// Create the result array via `this` constructor when it is a subclass.
fn construct_array_like(realm: &mut Realm, ctor: Value, items: Vec<Value>) -> JsResult<Value> {
    if ctor.as_object() == Some(realm.intrinsics.array_ctor) || !realm.is_constructor(&ctor) {
        return Ok(realm.new_array_value(items));
    }
    let len = items.len();
    let obj = realm.construct(ctor, &[Value::Number(len as f64)], Value::Undefined)?;
    let Some(o) = obj.as_object() else { return realm.type_error("constructor did not return an object") };
    for (i, v) in items.into_iter().enumerate() {
        create_data_property_or_throw(realm, o, i as f64, v)?;
    }
    set_length_or_throw(realm, obj.clone(), len as f64)?;
    Ok(obj)
}

fn of(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    construct_array_like(realm, this, args.to_vec())
}

// ------------------------------------------------------------- helpers

fn this_arr(realm: &mut Realm, this: &Value) -> JsResult<(ObjectId, usize)> {
    let id = realm.to_object(this.clone())?;
    let len = realm.length_of(Value::Object(id))?;
    Ok((id, len))
}

fn dense(realm: &Realm, id: ObjectId) -> Option<Vec<Value>> {
    match &realm.obj(id).kind {
        ObjectKind::Array { elements, length } if *length as usize == elements.len() && realm.obj(id).props.is_empty() => Some(elements.clone()),
        _ => None,
    }
}

/// A present dense element of an array (no key string needed).
#[inline]
fn dense_get(realm: &Realm, id: ObjectId, i: usize) -> Option<Value> {
    match &realm.obj(id).kind {
        ObjectKind::Array { elements, .. } => match elements.get(i) {
            Some(v) if !v.is_empty() => Some(v.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn get_i(realm: &mut Realm, id: ObjectId, i: usize) -> JsResult<Value> {
    if let Some(v) = dense_get(realm, id, i) {
        return Ok(v);
    }
    realm.get(Value::Object(id), PropertyKey::index(i as u32))
}

fn has_i(realm: &mut Realm, id: ObjectId, i: usize) -> JsResult<bool> {
    if let ObjectKind::Array { elements, .. } = &realm.obj(id).kind {
        if matches!(elements.get(i), Some(v) if !v.is_empty()) {
            return Ok(true);
        }
    }
    realm.has_property(id, &PropertyKey::index(i as u32))
}

fn set_i(realm: &mut Realm, id: ObjectId, i: usize, v: Value) -> JsResult<()> {
    let ok = realm.set(Value::Object(id), PropertyKey::index(i as u32), v)?;
    if !ok {
        return realm.type_error(&format!("Cannot assign to read only property '{}' of object", i));
    }
    Ok(())
}

fn set_len(realm: &mut Realm, id: ObjectId, len: usize) -> JsResult<()> {
    let ok = realm.set(Value::Object(id), crate::value::names::length(), Value::Number(len as f64))?;
    if !ok {
        return realm.type_error("Cannot assign to read only property 'length' of object");
    }
    Ok(())
}

fn callback(realm: &mut Realm, args: &[Value], name: &str) -> JsResult<Value> {
    let f = arg(args, 0);
    if !realm.is_callable(&f) {
        let d = realm.describe_for_error(&f);
        return realm.type_error(&format!("{} is not a function (Array.prototype.{})", d, name));
    }
    Ok(f)
}

/// Create the result array for species-aware methods.
fn species_create(realm: &mut Realm, original: ObjectId, len: usize) -> JsResult<ObjectId> {
    if !is_array_value(realm, &Value::Object(original)) {
        return Ok(realm.new_array(vec![]));
    }
    let c = realm.get_str(Value::Object(original), "constructor")?;
    let c = match c.v() {
        V::Object(cid) => {
            let s = realm.get(Value::Object(cid), PropertyKey::Symbol(SYM_SPECIES))?;
            if s.is_nullish() {
                Value::Undefined
            } else {
                s
            }
        }
        V::Undefined => Value::Undefined,
        _ => return realm.type_error("constructor is not an object"),
    };
    if c.is_undefined() || c.as_object() == Some(realm.intrinsics.array_ctor) {
        return Ok(realm.new_array(vec![]));
    }
    if !realm.is_constructor(&c) {
        return realm.type_error("species is not a constructor");
    }
    let r = realm.construct(c, &[Value::Number(len as f64)], Value::Undefined)?;
    match r.v() {
        V::Object(id) => Ok(id),
        _ => realm.type_error("species constructor returned non-object"),
    }
}

fn push_result(realm: &mut Realm, arr: ObjectId, index: usize, v: Value) -> JsResult<()> {
    if let ObjectKind::Array { elements, length } = &mut realm.obj_mut(arr).kind {
        if index == elements.len() {
            elements.push(v);
            *length = elements.len() as u32;
            return Ok(());
        }
    }
    realm.define_own(arr, PropertyKey::index(index as u32), Property::data(v, ALL))?;
    Ok(())
}

// ------------------------------------------------------------- methods

fn at(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    let n = to_integer(realm.to_number(arg(args, 0))?);
    let k = if n < 0.0 { len as f64 + n } else { n };
    if k < 0.0 || k >= len as f64 {
        return Ok(Value::Undefined);
    }
    get_i(realm, id, k as usize)
}

fn is_concat_spreadable(realm: &mut Realm, v: &Value) -> JsResult<bool> {
    if let V::Object(id) = v.v() {
        let s = realm.get(Value::Object(id), PropertyKey::Symbol(SYM_IS_CONCAT_SPREADABLE))?;
        if !s.is_undefined() {
            return Ok(s.truthy());
        }
        return Ok(is_array_value(realm, v));
    }
    Ok(false)
}

fn concat(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, _) = this_arr(realm, &this)?;
    let out = species_create(realm, id, 0)?;
    let mut n = 0usize;
    let mut items = vec![Value::Object(id)];
    items.extend_from_slice(args);
    for item in items {
        if is_concat_spreadable(realm, &item)? {
            let iid = item.as_object().unwrap();
            if let Some(d) = dense(realm, iid) {
                for v in d {
                    if !v.is_empty() {
                        push_result(realm, out, n, v)?;
                    }
                    n += 1;
                }
                continue;
            }
            let len = realm.length_of(item.clone())?;
            for i in 0..len {
                if has_i(realm, iid, i)? {
                    let v = get_i(realm, iid, i)?;
                    push_result(realm, out, n, v)?;
                }
                n += 1;
            }
        } else {
            push_result(realm, out, n, item)?;
            n += 1;
        }
    }
    set_len(realm, out, n)?;
    Ok(Value::Object(out))
}

fn copy_within(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    let target = realm.relative_index(arg(args, 0), len, 0)?;
    let start = realm.relative_index(arg(args, 1), len, 0)?;
    let end = realm.relative_index(arg(args, 2), len, len)?;
    let count = (end.saturating_sub(start)).min(len - target);
    let mut buf = Vec::with_capacity(count);
    for i in 0..count {
        let present = has_i(realm, id, start + i)?;
        buf.push(if present { Some(get_i(realm, id, start + i)?) } else { None });
    }
    for (i, v) in buf.into_iter().enumerate() {
        match v {
            Some(v) => set_i(realm, id, target + i, v)?,
            None => {
                realm.delete_property(id, &PropertyKey::index((target + i) as u32))?;
            }
        }
    }
    Ok(Value::Object(id))
}

fn entries(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = realm.to_object(this)?;
    Ok(iterator::create_array_iterator(realm, Value::Object(id), 2))
}
fn keys(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = realm.to_object(this)?;
    Ok(iterator::create_array_iterator(realm, Value::Object(id), 0))
}
fn values(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = realm.to_object(this)?;
    Ok(iterator::create_array_iterator(realm, Value::Object(id), 1))
}

fn every(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    // GC-safe: the object is rooted.
    let (id, len) = this_arr(realm, &this)?;
    realm.native_root(Value::Object(id));
    let f = callback(realm, args, "every")?;
    let t = arg(args, 1);
    for i in 0..len {
        if has_i(realm, id, i)? {
            let v = get_i(realm, id, i)?;
            if !realm.call(f.clone(), t.clone(), &[v, Value::Number(i as f64), Value::Object(id)])?.truthy() {
                return Ok(Value::Bool(false));
            }
        }
    }
    Ok(Value::Bool(true))
}

fn some(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    // GC-safe: the object is rooted.
    let (id, len) = this_arr(realm, &this)?;
    realm.native_root(Value::Object(id));
    let f = callback(realm, args, "some")?;
    let t = arg(args, 1);
    for i in 0..len {
        if has_i(realm, id, i)? {
            let v = get_i(realm, id, i)?;
            if realm.call(f.clone(), t.clone(), &[v, Value::Number(i as f64), Value::Object(id)])?.truthy() {
                return Ok(Value::Bool(true));
            }
        }
    }
    Ok(Value::Bool(false))
}

fn fill(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    let v = arg(args, 0);
    let start = realm.relative_index(arg(args, 1), len, 0)?;
    let end = realm.relative_index(arg(args, 2), len, len)?;
    for i in start..end {
        set_i(realm, id, i, v.clone())?;
    }
    Ok(Value::Object(id))
}

fn filter(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    // GC-safe: the object, the result and the element are rooted.
    let (id, len) = this_arr(realm, &this)?;
    realm.native_root(Value::Object(id));
    let f = callback(realm, args, "filter")?;
    let t = arg(args, 1);
    let out = species_create(realm, id, 0)?;
    realm.native_root(Value::Object(out));
    let held = realm.native_root(Value::Undefined);
    let mut n = 0;
    for i in 0..len {
        if has_i(realm, id, i)? {
            let v = get_i(realm, id, i)?;
            realm.set_native_root(held, v.clone());
            if realm.call(f.clone(), t.clone(), &[v.clone(), Value::Number(i as f64), Value::Object(id)])?.truthy() {
                push_result(realm, out, n, v)?;
                n += 1;
            }
        }
    }
    set_len(realm, out, n)?;
    Ok(Value::Object(out))
}

fn find_impl(realm: &mut Realm, this: Value, args: &[Value], from_end: bool, want_index: bool) -> JsResult<Value> {
    // GC-safe: the object and the element are rooted.
    let (id, len) = this_arr(realm, &this)?;
    realm.native_root(Value::Object(id));
    let f = callback(realm, args, "find")?;
    let t = arg(args, 1);
    let held = realm.native_root(Value::Undefined);
    let mut i: isize = if from_end { len as isize - 1 } else { 0 };
    while i >= 0 && (i as usize) < len {
        let v = get_i(realm, id, i as usize)?;
        realm.set_native_root(held, v.clone());
        if realm.call(f.clone(), t.clone(), &[v.clone(), Value::Number(i as f64), Value::Object(id)])?.truthy() {
            return Ok(if want_index { Value::Number(i as f64) } else { v });
        }
        i += if from_end { -1 } else { 1 };
    }
    Ok(if want_index { Value::Number(-1.0) } else { Value::Undefined })
}

fn find(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    find_impl(realm, this, args, false, false)
}
fn find_index(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    find_impl(realm, this, args, false, true)
}
fn find_last(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    find_impl(realm, this, args, true, false)
}
fn find_last_index(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    find_impl(realm, this, args, true, true)
}

fn flatten_into(realm: &mut Realm, out: &mut Vec<Value>, src: ObjectId, depth: f64, mapper: Option<(&Value, &Value)>) -> JsResult<()> {
    let len = realm.length_of(Value::Object(src))?;
    for i in 0..len {
        if !has_i(realm, src, i)? {
            continue;
        }
        let mut v = get_i(realm, src, i)?;
        if let Some((f, t)) = mapper {
            v = realm.call(f.clone(), t.clone(), &[v, Value::Number(i as f64), Value::Object(src)])?;
        }
        if depth > 0.0 && is_array_value(realm, &v) {
            flatten_into(realm, out, v.as_object().unwrap(), depth - 1.0, None)?;
        } else {
            out.push(v);
        }
    }
    Ok(())
}

fn flat(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, _) = this_arr(realm, &this)?;
    let depth = if arg(args, 0).is_undefined() { 1.0 } else { to_integer(realm.to_number(arg(args, 0))?) };
    let mut out = Vec::new();
    flatten_into(realm, &mut out, id, depth, None)?;
    Ok(realm.new_array_value(out))
}

fn flat_map(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, _) = this_arr(realm, &this)?;
    let f = callback(realm, args, "flatMap")?;
    let t = arg(args, 1);
    let mut out = Vec::new();
    flatten_into(realm, &mut out, id, 1.0, Some((&f, &t)))?;
    Ok(realm.new_array_value(out))
}

fn for_each(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    // GC-safe (see `NativeFunction::gc_safe`): the object is rooted.
    let (id, len) = this_arr(realm, &this)?;
    realm.native_root(Value::Object(id));
    let f = callback(realm, args, "forEach")?;
    let t = arg(args, 1);
    for i in 0..len {
        if has_i(realm, id, i)? {
            let v = get_i(realm, id, i)?;
            realm.call(f.clone(), t.clone(), &[v, Value::Number(i as f64), Value::Object(id)])?;
        }
    }
    Ok(Value::Undefined)
}

fn includes(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    if len == 0 {
        return Ok(Value::Bool(false));
    }
    let target = arg(args, 0);
    let start = realm.relative_index(arg(args, 1), len, 0)?;
    for i in start..len {
        let v = get_i(realm, id, i)?;
        if realm.same_value_zero(&v, &target) {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

fn index_of(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    if len == 0 {
        return Ok(Value::Number(-1.0));
    }
    let target = arg(args, 0);
    let start = realm.relative_index(arg(args, 1), len, 0)?;
    if let Some(d) = dense(realm, id) {
        for (i, v) in d.iter().enumerate().skip(start) {
            if realm.strict_equals(v, &target) {
                return Ok(Value::Number(i as f64));
            }
        }
        return Ok(Value::Number(-1.0));
    }
    for i in start..len {
        if has_i(realm, id, i)? {
            let v = get_i(realm, id, i)?;
            if realm.strict_equals(&v, &target) {
                return Ok(Value::Number(i as f64));
            }
        }
    }
    Ok(Value::Number(-1.0))
}

fn last_index_of(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    if len == 0 {
        return Ok(Value::Number(-1.0));
    }
    let target = arg(args, 0);
    let mut from = len as f64 - 1.0;
    if args.len() > 1 {
        let n = to_integer(realm.to_number(arg(args, 1))?);
        from = if n < 0.0 { len as f64 + n } else { n.min(len as f64 - 1.0) };
    }
    let mut i = from as isize;
    while i >= 0 {
        if has_i(realm, id, i as usize)? {
            let v = get_i(realm, id, i as usize)?;
            if realm.strict_equals(&v, &target) {
                return Ok(Value::Number(i as f64));
            }
        }
        i -= 1;
    }
    Ok(Value::Number(-1.0))
}

fn join(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    let sep = match arg(args, 0) {
        Value::Undefined => JsStr::new(","),
        v => realm.to_string(v)?,
    };
    // cycle guard
    if realm.join_stack.contains(&id) {
        return Ok(Value::str(""));
    }
    realm.join_stack.push(id);
    let mut out: Vec<u16> = Vec::new();
    let result = (|| -> JsResult<()> {
        for i in 0..len {
            if i > 0 {
                out.extend_from_slice(sep.as_units());
            }
            let v = get_i(realm, id, i)?;
            if !v.is_nullish() {
                let s = realm.to_string(v)?;
                out.extend_from_slice(s.as_units());
            }
        }
        Ok(())
    })();
    realm.join_stack.pop();
    result?;
    Ok(Value::String(JsStr::from_units(out)))
}

fn to_string(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = realm.to_object(this)?;
    let j = realm.get_str(Value::Object(id), "join")?;
    if realm.is_callable(&j) {
        return realm.call(j, Value::Object(id), &[]);
    }
    object::to_string(realm, Value::Object(id), &[])
}

fn to_locale_string(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    let mut parts = Vec::new();
    for i in 0..len {
        let v = get_i(realm, id, i)?;
        if v.is_nullish() {
            parts.push(String::new());
        } else {
            let s = realm.invoke(v, "toLocaleString", &[])?;
            parts.push(realm.to_rust_string(s)?);
        }
    }
    Ok(Value::str(&parts.join(",")))
}

fn map(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    // GC-safe: the object and the result are rooted.
    let (id, len) = this_arr(realm, &this)?;
    realm.native_root(Value::Object(id));
    let f = callback(realm, args, "map")?;
    let t = arg(args, 1);
    let out = species_create(realm, id, len)?;
    realm.native_root(Value::Object(out));
    for i in 0..len {
        if has_i(realm, id, i)? {
            let v = get_i(realm, id, i)?;
            let r = realm.call(f.clone(), t.clone(), &[v, Value::Number(i as f64), Value::Object(id)])?;
            push_result(realm, out, i, r)?;
        }
    }
    if let ObjectKind::Array { length, .. } = &mut realm.obj_mut(out).kind {
        if (*length as usize) < len {
            *length = len as u32;
        }
    }
    Ok(Value::Object(out))
}

fn pop(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    if len == 0 {
        set_len(realm, id, 0)?;
        return Ok(Value::Undefined);
    }
    let extensible = realm.obj(id).extensible;
    if let ObjectKind::Array { elements, length } = &mut realm.obj_mut(id).kind {
        if *length as usize == elements.len() && extensible {
            let v = elements.pop().unwrap();
            *length -= 1;
            return Ok(if v.is_empty() { Value::Undefined } else { v });
        }
    }
    let v = get_i(realm, id, len - 1)?;
    realm.delete_property(id, &PropertyKey::index((len - 1) as u32))?;
    set_len(realm, id, len - 1)?;
    Ok(v)
}

fn push(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    let fast = realm.obj(id).extensible && !realm.obj(id).props.contains(&crate::value::names::length());
    if let ObjectKind::Array { elements, length } = &mut realm.obj_mut(id).kind {
        if *length as usize == elements.len() && fast {
            elements.extend_from_slice(args);
            *length = elements.len() as u32;
            return Ok(Value::Number(*length as f64));
        }
    }
    for (i, a) in args.iter().enumerate() {
        set_i(realm, id, len + i, a.clone())?;
    }
    set_len(realm, id, len + args.len())?;
    Ok(Value::Number((len + args.len()) as f64))
}

fn reduce_impl(realm: &mut Realm, this: Value, args: &[Value], right: bool) -> JsResult<Value> {
    // GC-safe: the object and the accumulator are rooted.
    let (id, len) = this_arr(realm, &this)?;
    realm.native_root(Value::Object(id));
    let f = callback(realm, args, if right { "reduceRight" } else { "reduce" })?;
    let mut indices: Vec<usize> = (0..len).collect();
    if right {
        indices.reverse();
    }
    let mut it = indices.into_iter();
    let mut acc = if args.len() >= 2 {
        args[1].clone()
    } else {
        let mut found = None;
        for i in it.by_ref() {
            if has_i(realm, id, i)? {
                found = Some(get_i(realm, id, i)?);
                break;
            }
        }
        match found {
            Some(v) => v,
            None => return realm.type_error("Reduce of empty array with no initial value"),
        }
    };
    let held = realm.native_root(acc.clone());
    for i in it {
        if has_i(realm, id, i)? {
            let v = get_i(realm, id, i)?;
            acc = realm.call(f.clone(), Value::Undefined, &[acc, v, Value::Number(i as f64), Value::Object(id)])?;
            realm.set_native_root(held, acc.clone());
        }
    }
    Ok(acc)
}

fn reduce(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    reduce_impl(realm, this, args, false)
}
fn reduce_right(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    reduce_impl(realm, this, args, true)
}

fn reverse(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    if let ObjectKind::Array { elements, length } = &mut realm.obj_mut(id).kind {
        if *length as usize == elements.len() {
            elements.reverse();
            return Ok(Value::Object(id));
        }
    }
    let mut lo = 0;
    let mut hi = len;
    while lo + 1 < hi {
        hi -= 1;
        let lo_has = has_i(realm, id, lo)?;
        let hi_has = has_i(realm, id, hi)?;
        let lo_v = if lo_has { Some(get_i(realm, id, lo)?) } else { None };
        let hi_v = if hi_has { Some(get_i(realm, id, hi)?) } else { None };
        match hi_v {
            Some(v) => set_i(realm, id, lo, v)?,
            None => {
                realm.delete_property(id, &PropertyKey::index(lo as u32))?;
            }
        }
        match lo_v {
            Some(v) => set_i(realm, id, hi, v)?,
            None => {
                realm.delete_property(id, &PropertyKey::index(hi as u32))?;
            }
        }
        lo += 1;
    }
    Ok(Value::Object(id))
}

fn shift(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    if len == 0 {
        set_len(realm, id, 0)?;
        return Ok(Value::Undefined);
    }
    let extensible = realm.obj(id).extensible;
    if let ObjectKind::Array { elements, length } = &mut realm.obj_mut(id).kind {
        if *length as usize == elements.len() && extensible {
            let v = elements.remove(0);
            *length -= 1;
            return Ok(if v.is_empty() { Value::Undefined } else { v });
        }
    }
    let first = get_i(realm, id, 0)?;
    for i in 1..len {
        if has_i(realm, id, i)? {
            let v = get_i(realm, id, i)?;
            set_i(realm, id, i - 1, v)?;
        } else {
            realm.delete_property(id, &PropertyKey::index((i - 1) as u32))?;
        }
    }
    realm.delete_property(id, &PropertyKey::index((len - 1) as u32))?;
    set_len(realm, id, len - 1)?;
    Ok(first)
}

fn unshift(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    let extensible = realm.obj(id).extensible;
    if let ObjectKind::Array { elements, length } = &mut realm.obj_mut(id).kind {
        if *length as usize == elements.len() && extensible {
            let mut new: Vec<Value> = args.to_vec();
            new.append(elements);
            *elements = new;
            *length = elements.len() as u32;
            return Ok(Value::Number(*length as f64));
        }
    }
    let n = args.len();
    if n > 0 {
        let mut k = len;
        while k > 0 {
            k -= 1;
            if has_i(realm, id, k)? {
                let v = get_i(realm, id, k)?;
                set_i(realm, id, k + n, v)?;
            } else {
                realm.delete_property(id, &PropertyKey::index((k + n) as u32))?;
            }
        }
        for (j, a) in args.iter().enumerate() {
            set_i(realm, id, j, a.clone())?;
        }
    }
    set_len(realm, id, len + n)?;
    Ok(Value::Number((len + n) as f64))
}

fn slice(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    let start = realm.relative_index(arg(args, 0), len, 0)?;
    let end = realm.relative_index(arg(args, 1), len, len)?;
    let count = end.saturating_sub(start);
    let out = species_create(realm, id, count)?;
    if let Some(d) = dense(realm, id) {
        let is_plain = matches!(&realm.obj(out).kind, ObjectKind::Array { elements, .. } if elements.is_empty());
        if is_plain {
            let items: Vec<Value> = d[start.min(d.len())..end.min(d.len())].to_vec();
            if let ObjectKind::Array { elements, length } = &mut realm.obj_mut(out).kind {
                *elements = items;
                *length = elements.len() as u32;
            }
            return Ok(Value::Object(out));
        }
    }
    let mut n = 0;
    for i in start..end {
        if has_i(realm, id, i)? {
            let v = get_i(realm, id, i)?;
            push_result(realm, out, n, v)?;
        }
        n += 1;
    }
    set_len(realm, out, n)?;
    Ok(Value::Object(out))
}

/// Stable merge sort with a fallible comparator.
fn sort_values(realm: &mut Realm, items: Vec<Value>, cmp: &Value) -> JsResult<Vec<Value>> {
    fn compare(realm: &mut Realm, a: &Value, b: &Value, cmp: &Value) -> JsResult<std::cmp::Ordering> {
        use std::cmp::Ordering;
        if a.is_undefined() && b.is_undefined() {
            return Ok(Ordering::Equal);
        }
        if a.is_undefined() {
            return Ok(Ordering::Greater);
        }
        if b.is_undefined() {
            return Ok(Ordering::Less);
        }
        if !cmp.is_undefined() {
            let r = realm.call(cmp.clone(), Value::Undefined, &[a.clone(), b.clone()])?;
            let n = realm.to_number(r)?;
            return Ok(if n < 0.0 {
                Ordering::Less
            } else if n > 0.0 {
                Ordering::Greater
            } else {
                Ordering::Equal
            });
        }
        let sa = realm.to_string(a.clone())?;
        let sb = realm.to_string(b.clone())?;
        Ok(sa.cmp_units(&sb))
    }
    fn merge_sort(realm: &mut Realm, v: Vec<Value>, cmp: &Value) -> JsResult<Vec<Value>> {
        if v.len() <= 1 {
            return Ok(v);
        }
        if v.len() <= 8 {
            // insertion sort
            let mut out: Vec<Value> = Vec::with_capacity(v.len());
            for x in v {
                let mut pos = out.len();
                while pos > 0 && compare(realm, &out[pos - 1], &x, cmp)? == std::cmp::Ordering::Greater {
                    pos -= 1;
                }
                out.insert(pos, x);
            }
            return Ok(out);
        }
        let mid = v.len() / 2;
        let mut v = v;
        let right = v.split_off(mid);
        let left = merge_sort(realm, v, cmp)?;
        let right = merge_sort(realm, right, cmp)?;
        let mut out = Vec::with_capacity(left.len() + right.len());
        let (mut i, mut j) = (0, 0);
        while i < left.len() && j < right.len() {
            if compare(realm, &left[i], &right[j], cmp)? == std::cmp::Ordering::Greater {
                out.push(right[j].clone());
                j += 1;
            } else {
                out.push(left[i].clone());
                i += 1;
            }
        }
        out.extend_from_slice(&left[i..]);
        out.extend_from_slice(&right[j..]);
        Ok(out)
    }
    merge_sort(realm, items, cmp)
}

fn sort(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let cmp = arg(args, 0);
    if !cmp.is_undefined() && !realm.is_callable(&cmp) {
        return realm.type_error("The comparison function must be either a function or undefined");
    }
    let (id, len) = this_arr(realm, &this)?;
    let mut items = Vec::with_capacity(len.min(1 << 16));
    let mut holes = 0;
    for i in 0..len {
        if has_i(realm, id, i)? {
            items.push(get_i(realm, id, i)?);
        } else {
            holes += 1;
        }
    }
    let sorted = sort_values(realm, items, &cmp)?;
    let n = sorted.len();
    for (i, v) in sorted.into_iter().enumerate() {
        set_i(realm, id, i, v)?;
    }
    for i in n..n + holes {
        realm.delete_property(id, &PropertyKey::index(i as u32))?;
    }
    Ok(Value::Object(id))
}

fn splice(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    let start = realm.relative_index(arg(args, 0), len, 0)?;
    let delete_count = if args.is_empty() {
        0
    } else if args.len() == 1 {
        len - start
    } else {
        let n = to_integer(realm.to_number(arg(args, 1))?);
        (n.max(0.0) as usize).min(len - start)
    };
    let items: Vec<Value> = if args.len() > 2 { args[2..].to_vec() } else { Vec::new() };
    // Fast path for dense arrays
    let extensible = realm.obj(id).extensible;
    if let ObjectKind::Array { elements, length } = &mut realm.obj_mut(id).kind {
        if *length as usize == elements.len() && extensible {
            let removed: Vec<Value> = elements.splice(start..start + delete_count, items).map(|v| if v.is_empty() { Value::Undefined } else { v }).collect();
            *length = elements.len() as u32;
            return Ok(realm.new_array_value(removed));
        }
    }
    let removed_arr = species_create(realm, id, delete_count)?;
    for i in 0..delete_count {
        if has_i(realm, id, start + i)? {
            let v = get_i(realm, id, start + i)?;
            push_result(realm, removed_arr, i, v)?;
        }
    }
    set_len(realm, removed_arr, delete_count)?;
    let item_count = items.len();
    if item_count < delete_count {
        for k in start..len - delete_count {
            let from = k + delete_count;
            let to = k + item_count;
            if has_i(realm, id, from)? {
                let v = get_i(realm, id, from)?;
                set_i(realm, id, to, v)?;
            } else {
                realm.delete_property(id, &PropertyKey::index(to as u32))?;
            }
        }
        for k in (len - delete_count + item_count..len).rev() {
            realm.delete_property(id, &PropertyKey::index(k as u32))?;
        }
    } else if item_count > delete_count {
        for k in (start..len - delete_count).rev() {
            let from = k + delete_count;
            let to = k + item_count;
            if has_i(realm, id, from)? {
                let v = get_i(realm, id, from)?;
                set_i(realm, id, to, v)?;
            } else {
                realm.delete_property(id, &PropertyKey::index(to as u32))?;
            }
        }
    }
    for (i, v) in items.into_iter().enumerate() {
        set_i(realm, id, start + i, v)?;
    }
    set_len(realm, id, len - delete_count + item_count)?;
    Ok(Value::Object(removed_arr))
}

/// ArrayCreate's length check (before any element is read).
fn check_array_create(realm: &mut Realm, len: usize) -> JsResult<()> {
    if len > u32::MAX as usize {
        return realm.range_error("Invalid array length");
    }
    Ok(())
}

fn to_reversed(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    check_array_create(realm, len)?;
    let mut out = Vec::with_capacity(len.min(1 << 16));
    for i in (0..len).rev() {
        out.push(get_i(realm, id, i)?);
    }
    Ok(realm.new_array_value(out))
}

fn to_sorted(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let cmp = arg(args, 0);
    if !cmp.is_undefined() && !realm.is_callable(&cmp) {
        return realm.type_error("The comparison function must be either a function or undefined");
    }
    let (id, len) = this_arr(realm, &this)?;
    check_array_create(realm, len)?;
    let mut items = Vec::with_capacity(len.min(1 << 16));
    for i in 0..len {
        items.push(get_i(realm, id, i)?);
    }
    let sorted = sort_values(realm, items, &cmp)?;
    Ok(realm.new_array_value(sorted))
}

fn to_spliced(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    let start = realm.relative_index(arg(args, 0), len, 0)?;
    let skip = if args.is_empty() {
        0
    } else if args.len() == 1 {
        len - start
    } else {
        let n = to_integer(realm.to_number(arg(args, 1))?);
        (n.max(0.0) as usize).min(len - start)
    };
    let new_len = (len - skip) as f64 + args.len().saturating_sub(2) as f64;
    if new_len > 9007199254740991.0 {
        return realm.type_error("Invalid array length");
    }
    check_array_create(realm, new_len as usize)?;
    let mut out = Vec::new();
    for i in 0..start {
        out.push(get_i(realm, id, i)?);
    }
    if args.len() > 2 {
        out.extend_from_slice(&args[2..]);
    }
    for i in start + skip..len {
        out.push(get_i(realm, id, i)?);
    }
    Ok(realm.new_array_value(out))
}

fn with(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    let n = to_integer(realm.to_number(arg(args, 0))?);
    let k = if n < 0.0 { len as f64 + n } else { n };
    if k < 0.0 || k >= len as f64 {
        return realm.range_error("Invalid index");
    }
    check_array_create(realm, len)?;
    let mut out = Vec::with_capacity(len.min(1 << 16));
    for i in 0..len {
        out.push(if i == k as usize { arg(args, 1) } else { get_i(realm, id, i)? });
    }
    Ok(realm.new_array_value(out))
}
