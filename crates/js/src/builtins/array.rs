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
    if let Some(g) = realm.obj(ctor).props.get(&PropertyKey::Symbol(SYM_SPECIES)).and_then(|p| p.getter().and_then(|g| g.as_object())) {
        realm.intrinsics.others.insert("ArraySpeciesGetter", g);
    }
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
    Ok(Value::Bool(is_array_checked(realm, &arg(args, 0))?))
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

/// The largest length of an array-like (2^53 - 1).
const MAX_SAFE_LEN: usize = 9007199254740991;

/// The property key of an integer index (array index or its string).
pub(crate) fn index_key(i: f64) -> PropertyKey {
    if i >= 0.0 && i < u32::MAX as f64 {
        PropertyKey::index(i as u32)
    } else {
        PropertyKey::String(crate::value::JsStr::new(&crate::value::number_to_string(i)))
    }
}

/// The property key of integer index `i` (any value up to 2^53 - 1).
#[inline]
pub(crate) fn key_of(i: usize) -> PropertyKey {
    if i < u32::MAX as usize {
        PropertyKey::index(i as u32)
    } else {
        PropertyKey::String(crate::value::JsStr::new(&i.to_string()))
    }
}

/// The integer index a property key denotes (canonical decimal below 2^53).
fn integer_key(k: &PropertyKey) -> Option<usize> {
    let s = k.as_str()?;
    if let Some(i) = s.as_array_index() {
        return Some(i as usize);
    }
    let u = s.as_units();
    if u.is_empty() || u.len() > 16 || (u.len() > 1 && u[0] == b'0' as u16) {
        return None;
    }
    let mut n: usize = 0;
    for &c in u {
        if !(b'0' as u16..=b'9' as u16).contains(&c) {
            return None;
        }
        n = n * 10 + (c - b'0' as u16) as usize;
    }
    if n <= MAX_SAFE_LEN { Some(n) } else { None }
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

/// An array whose elements can be moved around directly: extensible,
/// writable length, no other own properties and every index below the
/// length stored in `elements` (holes only when no prototype has
/// indexed properties, so a hole reads as absent).
fn simple_dense(realm: &Realm, id: ObjectId) -> bool {
    let o = realm.obj(id);
    match &o.kind {
        ObjectKind::Array { elements, length } if o.extensible && o.props.is_empty() && *length as usize == elements.len() => {
            !elements.iter().any(|v| v.is_empty()) || o.proto.is_none_or(|p| realm.no_index_properties(p))
        }
        _ => false,
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
    realm.get(Value::Object(id), key_of(i))
}

fn has_i(realm: &mut Realm, id: ObjectId, i: usize) -> JsResult<bool> {
    if let ObjectKind::Array { elements, .. } = &realm.obj(id).kind {
        if matches!(elements.get(i), Some(v) if !v.is_empty()) {
            return Ok(true);
        }
    }
    realm.has_property(id, &key_of(i))
}

/// Set(O, i, v, true).
fn set_i(realm: &mut Realm, id: ObjectId, i: usize, v: Value) -> JsResult<()> {
    let ok = realm.set(Value::Object(id), key_of(i), v)?;
    if !ok {
        return realm.type_error(&format!("Cannot assign to read only property '{}' of object", i));
    }
    Ok(())
}

/// DeletePropertyOrThrow(O, i).
fn del_i(realm: &mut Realm, id: ObjectId, i: usize) -> JsResult<()> {
    if !realm.delete_property(id, &key_of(i))? {
        return realm.type_error(&format!("Cannot delete property '{}' of object", i));
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

/// Index ranges longer than this on objects with few indexed properties
/// are walked through their existing keys (see `sparse_keys`).
const SPARSE_RANGE: usize = 1 << 16;

/// The integer keys in `[lo, hi)` of an object whose other indices are
/// unobservably absent, ascending: an ordinary object or array whose own
/// indexed properties are all data properties and whose prototypes have
/// no indexed properties. `None` when the range is short (a plain loop is
/// as fast) or visiting only these keys could be observed.
fn sparse_keys(realm: &Realm, id: ObjectId, lo: usize, hi: usize) -> Option<Vec<usize>> {
    if hi <= lo || hi - lo <= SPARSE_RANGE {
        return None;
    }
    let o = realm.obj(id);
    let mut out = Vec::new();
    match &o.kind {
        ObjectKind::Ordinary => {}
        ObjectKind::Array { elements, .. } => {
            for (i, v) in elements.iter().enumerate().take(hi).skip(lo) {
                if !v.is_empty() {
                    out.push(i);
                }
            }
        }
        _ => return None,
    }
    if let Some(p) = o.proto {
        if !realm.no_index_properties(p) {
            return None;
        }
    }
    for (k, p) in o.props.iter() {
        if let Some(i) = integer_key(k) {
            if p.is_accessor() {
                return None;
            }
            if i >= lo && i < hi {
                out.push(i);
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    Some(out)
}

/// Move the elements in `[lo, hi)` (ascending, or descending when `down`)
/// to `index + delta`: Set the target when the source is present, else
/// DeletePropertyOrThrow it (the loops of copyWithin, shift, unshift and
/// splice).
fn move_range(realm: &mut Realm, id: ObjectId, lo: usize, hi: usize, delta: isize, down: bool) -> JsResult<()> {
    if hi <= lo {
        return Ok(());
    }
    if let Some(keys) = sparse_keys(realm, id, lo, hi) {
        let tlo = (lo as isize + delta) as usize;
        let thi = (hi as isize + delta) as usize;
        if let Some(targets) = sparse_keys(realm, id, tlo, thi) {
            // Sources that exist, plus sources whose existing target goes.
            let mut todo = keys;
            todo.extend(targets.into_iter().map(|t| (t as isize - delta) as usize));
            todo.sort_unstable();
            todo.dedup();
            if down {
                todo.reverse();
            }
            for f in todo {
                move_one(realm, id, f, (f as isize + delta) as usize)?;
            }
            return Ok(());
        }
    }
    if down {
        let mut f = hi;
        while f > lo {
            f -= 1;
            move_one(realm, id, f, (f as isize + delta) as usize)?;
        }
    } else {
        for f in lo..hi {
            move_one(realm, id, f, (f as isize + delta) as usize)?;
        }
    }
    Ok(())
}

fn move_one(realm: &mut Realm, id: ObjectId, from: usize, to: usize) -> JsResult<()> {
    if has_i(realm, id, from)? {
        let v = get_i(realm, id, from)?;
        set_i(realm, id, to, v)
    } else {
        del_i(realm, id, to)
    }
}

/// DeletePropertyOrThrow for every index in `[lo, hi)`, descending.
fn delete_range_down(realm: &mut Realm, id: ObjectId, lo: usize, hi: usize) -> JsResult<()> {
    if let Some(keys) = sparse_keys(realm, id, lo, hi) {
        for k in keys.into_iter().rev() {
            del_i(realm, id, k)?;
        }
        return Ok(());
    }
    let mut k = hi;
    while k > lo {
        k -= 1;
        del_i(realm, id, k)?;
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

/// IsArray(v): proxies answer for their target (revoked ones throw).
pub(crate) fn is_array_checked(realm: &mut Realm, v: &Value) -> JsResult<bool> {
    let mut cur = match v.v() {
        V::Object(id) => id,
        _ => return Ok(false),
    };
    loop {
        match &realm.obj(cur).kind {
            ObjectKind::Array { .. } => return Ok(true),
            ObjectKind::Proxy { target, revoked, .. } => {
                if *revoked {
                    return realm.type_error("Cannot perform 'IsArray' on a proxy that has been revoked");
                }
                cur = *target;
            }
            _ => return Ok(false),
        }
    }
}

/// ArrayCreate(len) with the intrinsic prototype.
fn array_create(realm: &mut Realm, len: usize) -> JsResult<ObjectId> {
    if len > u32::MAX as usize {
        return realm.range_error("Invalid array length");
    }
    let proto = realm.intrinsics.array_proto;
    Ok(realm.alloc(Object::new(Some(proto), ObjectKind::Array { elements: Vec::new(), length: len as u32 })))
}

/// `o` is a plain array whose `constructor[Symbol.species]` is the
/// unmodified intrinsic one (ArraySpeciesCreate is then ArrayCreate and
/// its lookups are unobservable).
fn species_is_default(realm: &Realm, o: ObjectId) -> bool {
    let ap = realm.intrinsics.array_proto;
    let obj = realm.obj(o);
    if !matches!(obj.kind, ObjectKind::Array { .. }) || obj.proto != Some(ap) || obj.props.contains(&crate::value::names::constructor()) {
        return false;
    }
    let ctor = realm.intrinsics.array_ctor;
    match realm.obj(ap).props.get(&crate::value::names::constructor()) {
        Some(p) if !p.is_accessor() && p.value.as_object() == Some(ctor) => {}
        _ => return false,
    }
    let Some(getter) = realm.intrinsics.others.get("ArraySpeciesGetter").copied() else { return false };
    match realm.obj(ctor).props.get(&PropertyKey::Symbol(SYM_SPECIES)) {
        Some(p) if p.is_accessor() => p.getter().and_then(|g| g.as_object()) == Some(getter),
        _ => false,
    }
}

/// ArraySpeciesCreate(original, len); also whether the result is a fresh
/// intrinsic array (its elements may then be written directly).
fn array_species_create(realm: &mut Realm, original: ObjectId, len: usize) -> JsResult<(ObjectId, bool)> {
    if species_is_default(realm, original) || !is_array_checked(realm, &Value::Object(original))? {
        return Ok((array_create(realm, len)?, true));
    }
    let mut c = realm.get(Value::Object(original), crate::value::names::constructor())?;
    // The %Array% of another realm stands for this realm's.
    if let V::Object(cid) = c.v() {
        if !realm.realms.is_empty() && realm.is_constructor(&c) {
            let r = realm.function_realm(cid)?;
            if r != realm.current_realm && realm.realm_intrinsics(r).array_ctor == cid {
                c = Value::Undefined;
            }
        }
    }
    let c = match c.v() {
        V::Object(cid) => {
            let s = realm.get(Value::Object(cid), PropertyKey::Symbol(SYM_SPECIES))?;
            if s.is_null() {
                Value::Undefined
            } else {
                s
            }
        }
        _ => c,
    };
    if c.is_undefined() {
        return Ok((array_create(realm, len)?, true));
    }
    if !realm.is_constructor(&c) {
        return realm.type_error("object.constructor[Symbol.species] is not a constructor");
    }
    let r = realm.construct(c, &[Value::Number(len as f64)], Value::Undefined)?;
    match r.v() {
        V::Object(id) => Ok((id, false)),
        _ => realm.type_error("species constructor returned non-object"),
    }
}

/// Create the result array for species-aware methods.
fn species_create(realm: &mut Realm, original: ObjectId, len: usize) -> JsResult<ObjectId> {
    Ok(array_species_create(realm, original, len)?.0)
}

/// CreateDataPropertyOrThrow(arr, index, v), appending in place when
/// possible.
fn push_result(realm: &mut Realm, arr: ObjectId, index: usize, v: Value) -> JsResult<()> {
    let obj = realm.obj_mut(arr);
    let plain = obj.extensible && obj.props.is_empty();
    if let ObjectKind::Array { elements, length } = &mut obj.kind {
        if index == elements.len() && plain && index < u32::MAX as usize - 1 {
            elements.push(v);
            if (*length as usize) < elements.len() {
                *length = elements.len() as u32;
            }
            return Ok(());
        }
    }
    if !realm.define_own(arr, key_of(index), Property::data(v, ALL))? {
        return realm.type_error(&format!("Cannot add property {}, object is not extensible", index));
    }
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
        return is_array_checked(realm, v);
    }
    Ok(false)
}

fn concat(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = realm.to_object(this)?;
    let (out, fresh) = array_species_create(realm, id, 0)?;
    let mut n = 0usize;
    let mut items = vec![Value::Object(id)];
    items.extend_from_slice(args);
    for item in items {
        if is_concat_spreadable(realm, &item)? {
            let iid = item.as_object().unwrap();
            if fresh {
                if let Some(d) = dense(realm, iid) {
                    for v in d {
                        if !v.is_empty() {
                            push_result(realm, out, n, v)?;
                        }
                        n += 1;
                    }
                    continue;
                }
            }
            let len = realm.length_of(item.clone())?;
            if n + len > MAX_SAFE_LEN {
                return realm.type_error("Invalid array length");
            }
            if let Some(keys) = sparse_keys(realm, iid, 0, len) {
                for k in keys {
                    let v = get_i(realm, iid, k)?;
                    push_result(realm, out, n + k, v)?;
                }
                n += len;
                continue;
            }
            for i in 0..len {
                if has_i(realm, iid, i)? {
                    let v = get_i(realm, iid, i)?;
                    push_result(realm, out, n, v)?;
                }
                n += 1;
            }
        } else {
            if n >= MAX_SAFE_LEN {
                return realm.type_error("Invalid array length");
            }
            push_result(realm, out, n, item)?;
            n += 1;
        }
    }
    set_len(realm, out, n)?;
    Ok(Value::Object(out))
}

fn copy_within(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    let to = realm.relative_index(arg(args, 0), len, 0)?;
    let from = realm.relative_index(arg(args, 1), len, 0)?;
    let fin = realm.relative_index(arg(args, 2), len, len)?;
    let count = fin.saturating_sub(from).min(len - to);
    if count == 0 {
        return Ok(Value::Object(id));
    }
    if from < to && to < from + count {
        move_range(realm, id, from, from + count, to as isize - from as isize, true)?;
    } else {
        move_range(realm, id, from, from + count, to as isize - from as isize, false)?;
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
    if start < end && simple_dense(realm, id) {
        if let ObjectKind::Array { elements, .. } = &mut realm.obj_mut(id).kind {
            for e in &mut elements[start..end] {
                *e = v.clone();
            }
            return Ok(Value::Object(id));
        }
    }
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

/// FlattenIntoArray: appends to `target` from `next`; returns the next
/// target index.
#[allow(clippy::too_many_arguments)]
fn flatten_into(realm: &mut Realm, target: ObjectId, src: ObjectId, src_len: usize, start: usize, depth: f64, mapper: Option<(&Value, &Value)>) -> JsResult<usize> {
    let mut n = start;
    for i in 0..src_len {
        if !has_i(realm, src, i)? {
            continue;
        }
        let mut v = get_i(realm, src, i)?;
        if let Some((f, t)) = mapper {
            v = realm.call(f.clone(), t.clone(), &[v, Value::Number(i as f64), Value::Object(src)])?;
        }
        if depth > 0.0 && is_array_checked(realm, &v)? {
            let eid = v.as_object().unwrap();
            let elen = realm.length_of(v.clone())?;
            n = flatten_into(realm, target, eid, elen, n, depth - 1.0, None)?;
        } else {
            if n >= MAX_SAFE_LEN {
                return realm.type_error("Invalid array length");
            }
            push_result(realm, target, n, v)?;
            n += 1;
        }
    }
    Ok(n)
}

fn flat(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    let depth = if arg(args, 0).is_undefined() { 1.0 } else { to_integer(realm.to_number(arg(args, 0))?).max(0.0) };
    let out = species_create(realm, id, 0)?;
    flatten_into(realm, out, id, len, 0, depth, None)?;
    Ok(Value::Object(out))
}

fn flat_map(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    let f = callback(realm, args, "flatMap")?;
    let t = arg(args, 1);
    let out = species_create(realm, id, 0)?;
    flatten_into(realm, out, id, len, 0, 1.0, Some((&f, &t)))?;
    Ok(Value::Object(out))
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
    if let Some(keys) = sparse_keys(realm, id, start, len) {
        // Absent indices read as undefined.
        if target.is_undefined() && keys.len() < len - start {
            return Ok(Value::Bool(true));
        }
        for k in keys {
            let v = get_i(realm, id, k)?;
            if realm.same_value_zero(&v, &target) {
                return Ok(Value::Bool(true));
            }
        }
        return Ok(Value::Bool(false));
    }
    for i in start..len {
        let v = get_i(realm, id, i)?;
        if realm.same_value_zero(&v, &target) {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

/// Strict-equality search of the present elements of a plain array from
/// `start`; `None` when a hole or a non-array needs the generic loop.
fn dense_index_of(realm: &Realm, id: ObjectId, start: usize, target: &Value) -> Option<Option<usize>> {
    let o = realm.obj(id);
    match &o.kind {
        ObjectKind::Array { elements, length } if *length as usize == elements.len() && o.props.is_empty() => {
            for (i, v) in elements.iter().enumerate().skip(start) {
                if v.is_empty() {
                    return None;
                }
                if realm.strict_equals(v, target) {
                    return Some(Some(i));
                }
            }
            Some(None)
        }
        _ => None,
    }
}

fn index_of(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    if len == 0 {
        return Ok(Value::Number(-1.0));
    }
    let target = arg(args, 0);
    let start = realm.relative_index(arg(args, 1), len, 0)?;
    if let Some(r) = dense_index_of(realm, id, start, &target) {
        return Ok(Value::Number(r.map_or(-1.0, |i| i as f64)));
    }
    if let Some(keys) = sparse_keys(realm, id, start, len) {
        for k in keys {
            let v = get_i(realm, id, k)?;
            if realm.strict_equals(&v, &target) {
                return Ok(Value::Number(k as f64));
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
    if from < 0.0 {
        return Ok(Value::Number(-1.0));
    }
    let from = from as usize;
    if let Some(keys) = sparse_keys(realm, id, 0, from + 1) {
        for k in keys.into_iter().rev() {
            let v = get_i(realm, id, k)?;
            if realm.strict_equals(&v, &target) {
                return Ok(Value::Number(k as f64));
            }
        }
        return Ok(Value::Number(-1.0));
    }
    let mut i = from + 1;
    while i > 0 {
        i -= 1;
        if has_i(realm, id, i)? {
            let v = get_i(realm, id, i)?;
            if realm.strict_equals(&v, &target) {
                return Ok(Value::Number(i as f64));
            }
        }
    }
    Ok(Value::Number(-1.0))
}

fn join(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    let sep = match arg(args, 0) {
        Value::Undefined => JsStr::new(","),
        v => realm.to_string(v)?,
    };
    if len > 1 && (len - 1).saturating_mul(sep.len()) > crate::value::MAX_STRING_LEN {
        return realm.range_error("Invalid string length");
    }
    // cycle guard
    if realm.join_stack.contains(&id) {
        return Ok(Value::str(""));
    }
    realm.join_stack.push(id);
    let mut out: Vec<u16> = Vec::new();
    let result = (|| -> JsResult<()> {
        if sep.is_empty() {
            if let Some(keys) = sparse_keys(realm, id, 0, len) {
                for k in keys {
                    let v = get_i(realm, id, k)?;
                    if !v.is_nullish() {
                        let s = realm.to_string(v)?;
                        out.extend_from_slice(s.as_units());
                        if out.len() > crate::value::MAX_STRING_LEN {
                            return realm.range_error("Invalid string length");
                        }
                    }
                }
                return Ok(());
            }
        }
        for i in 0..len {
            if i > 0 {
                out.extend_from_slice(sep.as_units());
            }
            let v = get_i(realm, id, i)?;
            if !v.is_nullish() {
                let s = realm.to_string(v)?;
                out.extend_from_slice(s.as_units());
            }
            if out.len() > crate::value::MAX_STRING_LEN {
                return realm.range_error("Invalid string length");
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
    if realm.join_stack.contains(&id) {
        return Ok(Value::str(""));
    }
    realm.join_stack.push(id);
    let mut out: Vec<u16> = Vec::new();
    let result = (|| -> JsResult<()> {
        for i in 0..len {
            if i > 0 {
                out.push(b',' as u16);
            }
            let v = get_i(realm, id, i)?;
            if !v.is_nullish() {
                let s = realm.invoke(v, "toLocaleString", &[])?;
                let s = realm.to_string(s)?;
                out.extend_from_slice(s.as_units());
            }
            if out.len() > crate::value::MAX_STRING_LEN {
                return realm.range_error("Invalid string length");
            }
        }
        Ok(())
    })();
    realm.join_stack.pop();
    result?;
    Ok(Value::String(JsStr::from_units(out)))
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

/// An array whose `length` is writable and whose elements all live in
/// `elements` (`length == elements.len()`) and that may grow or shrink.
#[inline]
fn growable(o: &Object) -> bool {
    o.extensible && matches!(&o.kind, ObjectKind::Array { elements, length } if *length as usize == elements.len()) && !o.props.contains(&crate::value::names::length())
}

fn pop(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    if len == 0 {
        set_len(realm, id, 0)?;
        return Ok(Value::Undefined);
    }
    let obj = realm.obj_mut(id);
    if growable(obj) {
        if let ObjectKind::Array { elements, length } = &mut obj.kind {
            if elements.last().is_some_and(|v| !v.is_empty()) {
                let v = elements.pop().unwrap();
                *length -= 1;
                return Ok(v);
            }
        }
    }
    let v = get_i(realm, id, len - 1)?;
    del_i(realm, id, len - 1)?;
    set_len(realm, id, len - 1)?;
    Ok(v)
}

fn push(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    let obj = realm.obj_mut(id);
    if growable(obj) {
        if let ObjectKind::Array { elements, length } = &mut obj.kind {
            if elements.len() + args.len() < u32::MAX as usize {
                elements.extend_from_slice(args);
                *length = elements.len() as u32;
                return Ok(Value::Number(*length as f64));
            }
        }
    }
    if len + args.len() > MAX_SAFE_LEN {
        return realm.type_error("Pushing elements past 2^53 - 1");
    }
    for (i, a) in args.iter().enumerate() {
        set_i(realm, id, len + i, a.clone())?;
    }
    set_len(realm, id, len + args.len())?;
    Ok(Value::Number((len + args.len()) as f64))
}

fn reverse(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    if simple_dense(realm, id) {
        if let ObjectKind::Array { elements, .. } = &mut realm.obj_mut(id).kind {
            elements.reverse();
            return Ok(Value::Object(id));
        }
    }
    let middle = len / 2;
    let swap = |realm: &mut Realm, lo: usize| -> JsResult<()> {
        let hi = len - lo - 1;
        let lo_has = has_i(realm, id, lo)?;
        let lo_v = if lo_has { Some(get_i(realm, id, lo)?) } else { None };
        let hi_has = has_i(realm, id, hi)?;
        let hi_v = if hi_has { Some(get_i(realm, id, hi)?) } else { None };
        match (lo_v, hi_v) {
            (Some(l), Some(h)) => {
                set_i(realm, id, lo, h)?;
                set_i(realm, id, hi, l)?;
            }
            (None, Some(h)) => {
                set_i(realm, id, lo, h)?;
                del_i(realm, id, hi)?;
            }
            (Some(l), None) => {
                del_i(realm, id, lo)?;
                set_i(realm, id, hi, l)?;
            }
            (None, None) => {}
        }
        Ok(())
    };
    if let Some(keys) = sparse_keys(realm, id, 0, len) {
        let mut lows: Vec<usize> = keys.into_iter().map(|k| if k < middle { k } else { len - k - 1 }).filter(|&k| k < middle).collect();
        lows.sort_unstable();
        lows.dedup();
        for lo in lows {
            swap(realm, lo)?;
        }
        return Ok(Value::Object(id));
    }
    for lo in 0..middle {
        swap(realm, lo)?;
    }
    Ok(Value::Object(id))
}

fn shift(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    if len == 0 {
        set_len(realm, id, 0)?;
        return Ok(Value::Undefined);
    }
    if simple_dense(realm, id) {
        if let ObjectKind::Array { elements, length } = &mut realm.obj_mut(id).kind {
            let v = elements.remove(0);
            *length -= 1;
            return Ok(if v.is_empty() { Value::Undefined } else { v });
        }
    }
    let first = get_i(realm, id, 0)?;
    move_range(realm, id, 1, len, -1, false)?;
    del_i(realm, id, len - 1)?;
    set_len(realm, id, len - 1)?;
    Ok(first)
}

fn unshift(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, len) = this_arr(realm, &this)?;
    let n = args.len();
    if simple_dense(realm, id) && len + n < u32::MAX as usize {
        if let ObjectKind::Array { elements, length } = &mut realm.obj_mut(id).kind {
            elements.splice(0..0, args.iter().cloned());
            *length = elements.len() as u32;
            return Ok(Value::Number(*length as f64));
        }
    }
    if n > 0 {
        if len + n > MAX_SAFE_LEN {
            return realm.type_error("Unshifting elements past 2^53 - 1");
        }
        move_range(realm, id, 0, len, n as isize, true)?;
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
    let (out, fresh) = array_species_create(realm, id, count)?;
    if fresh {
        if let Some(d) = dense(realm, id) {
            let items: Vec<Value> = d[start.min(d.len())..end.min(d.len()).max(start.min(d.len()))].to_vec();
            if !items.iter().any(|v| v.is_empty()) || realm.obj(id).proto.is_none_or(|p| realm.no_index_properties(p)) {
                if let ObjectKind::Array { elements, length } = &mut realm.obj_mut(out).kind {
                    *elements = items;
                    *length = elements.len() as u32;
                }
                return Ok(Value::Object(out));
            }
        }
    }
    if let Some(keys) = sparse_keys(realm, id, start, end) {
        for k in keys {
            let v = get_i(realm, id, k)?;
            push_result(realm, out, k - start, v)?;
        }
        set_len(realm, out, count)?;
        return Ok(Value::Object(out));
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

fn reduce_impl(realm: &mut Realm, this: Value, args: &[Value], right: bool) -> JsResult<Value> {
    // GC-safe: the object and the accumulator are rooted.
    let (id, len) = this_arr(realm, &this)?;
    realm.native_root(Value::Object(id));
    let f = callback(realm, args, if right { "reduceRight" } else { "reduce" })?;
    let mut it: Box<dyn Iterator<Item = usize>> = if right { Box::new((0..len).rev()) } else { Box::new(0..len) };
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
    if let Some(keys) = sparse_keys(realm, id, 0, len) {
        holes = len - keys.len();
        for k in keys {
            items.push(get_i(realm, id, k)?);
        }
    } else {
        for i in 0..len {
            if has_i(realm, id, i)? {
                items.push(get_i(realm, id, i)?);
            } else {
                holes += 1;
            }
        }
    }
    let sorted = sort_values(realm, items, &cmp)?;
    let n = sorted.len();
    for (i, v) in sorted.into_iter().enumerate() {
        set_i(realm, id, i, v)?;
    }
    // The holes go last, deleted in ascending order.
    if let Some(keys) = sparse_keys(realm, id, n, n + holes) {
        for k in keys {
            del_i(realm, id, k)?;
        }
    } else {
        for i in n..n + holes {
            del_i(realm, id, i)?;
        }
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
        (n.max(0.0).min((len - start) as f64)) as usize
    };
    let items: &[Value] = if args.len() > 2 { &args[2..] } else { &[] };
    let item_count = items.len();
    if len + item_count - delete_count > MAX_SAFE_LEN {
        return realm.type_error("Invalid array length");
    }
    let (removed_arr, fresh) = array_species_create(realm, id, delete_count)?;
    // Plain arrays: move the elements directly (unobservable).
    if fresh && simple_dense(realm, id) && realm.length_of(Value::Object(id))? == len && len + item_count < u32::MAX as usize {
        let removed: Vec<Value> = if let ObjectKind::Array { elements, length } = &mut realm.obj_mut(id).kind {
            let r: Vec<Value> = elements.splice(start..start + delete_count, items.iter().cloned()).collect();
            *length = elements.len() as u32;
            r
        } else {
            unreachable!()
        };
        if let ObjectKind::Array { elements, length } = &mut realm.obj_mut(removed_arr).kind {
            *elements = removed;
            *length = elements.len() as u32;
        }
        return Ok(Value::Object(removed_arr));
    }
    if let Some(keys) = sparse_keys(realm, id, start, start + delete_count) {
        for k in keys {
            let v = get_i(realm, id, k)?;
            push_result(realm, removed_arr, k - start, v)?;
        }
    } else {
        for i in 0..delete_count {
            if has_i(realm, id, start + i)? {
                let v = get_i(realm, id, start + i)?;
                push_result(realm, removed_arr, i, v)?;
            }
        }
    }
    set_len(realm, removed_arr, delete_count)?;
    if item_count < delete_count {
        move_range(realm, id, start + delete_count, len, item_count as isize - delete_count as isize, false)?;
        delete_range_down(realm, id, len - delete_count + item_count, len)?;
    } else if item_count > delete_count {
        move_range(realm, id, start + delete_count, len, (item_count - delete_count) as isize, true)?;
    }
    for (i, v) in items.iter().enumerate() {
        set_i(realm, id, start + i, v.clone())?;
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
        (n.max(0.0).min((len - start) as f64)) as usize
    };
    let new_len = (len - skip) as f64 + args.len().saturating_sub(2) as f64;
    if new_len > 9007199254740991.0 {
        return realm.type_error("Invalid array length");
    }
    check_array_create(realm, new_len as usize)?;
    let mut out = Vec::with_capacity((new_len as usize).min(1 << 16));
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
