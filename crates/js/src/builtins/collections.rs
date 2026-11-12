//! `Map`, `Set`, `WeakMap` and `WeakSet`.
//!
//! `WeakMap`/`WeakSet` hold their keys strongly: the garbage collector has
//! no ephemeron support yet, so entries live as long as the collection.

use super::iterator::create_map_iterator;
use crate::value::V;
use super::*;
use crate::realm::{SYM_ITERATOR, SYM_SPECIES, SYM_TO_STRING_TAG};

pub fn register(realm: &mut Realm) {
    let op = realm.intrinsics.object_proto;

    // ------------------------------------------------------------- Map
    let mp = realm.new_object_with_proto(Some(op));
    realm.intrinsics.others.insert("MapPrototype", mp);
    let map_ctor = define_ctor(realm, "Map", 0, map_ctor, mp);
    realm.intrinsics.others.insert("Map", map_ctor);
    realm.define_accessor(map_ctor, PropertyKey::Symbol(SYM_SPECIES), Some(species), None, CONFIGURABLE);
    realm.define_method(map_ctor, "groupBy", 2, map_group_by);
    realm.define_method(mp, "get", 1, map_get);
    realm.define_method(mp, "set", 2, map_set);
    realm.define_method(mp, "has", 1, map_has);
    realm.define_method(mp, "delete", 1, map_delete);
    realm.define_method(mp, "getOrInsert", 2, map_get_or_insert);
    realm.define_method(mp, "getOrInsertComputed", 2, map_get_or_insert_computed);
    realm.define_method(mp, "clear", 0, map_clear);
    realm.define_method(mp, "forEach", 1, map_for_each);
    realm.define_method(mp, "keys", 0, map_keys);
    realm.define_method(mp, "values", 0, map_values);
    let entries = realm.define_method(mp, "entries", 0, map_entries);
    realm.define_property(mp, PropertyKey::Symbol(SYM_ITERATOR), Value::Object(entries), HIDDEN);
    realm.define_accessor(mp, "size", Some(map_size), None, CONFIGURABLE);
    realm.define_property(mp, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("Map"), CONFIGURABLE);

    // ------------------------------------------------------------- Set
    let sp = realm.new_object_with_proto(Some(op));
    realm.intrinsics.others.insert("SetPrototype", sp);
    let set_ctor = define_ctor(realm, "Set", 0, set_ctor, sp);
    realm.intrinsics.others.insert("Set", set_ctor);
    realm.define_accessor(set_ctor, PropertyKey::Symbol(SYM_SPECIES), Some(species), None, CONFIGURABLE);
    realm.define_method(sp, "add", 1, set_add);
    realm.define_method(sp, "has", 1, set_has);
    realm.define_method(sp, "delete", 1, set_delete);
    realm.define_method(sp, "clear", 0, set_clear);
    realm.define_method(sp, "forEach", 1, set_for_each);
    realm.define_method(sp, "entries", 0, set_entries);
    let values = realm.define_method(sp, "values", 0, set_values);
    realm.define_property(sp, "keys", Value::Object(values), HIDDEN);
    realm.define_property(sp, PropertyKey::Symbol(SYM_ITERATOR), Value::Object(values), HIDDEN);
    realm.define_accessor(sp, "size", Some(set_size), None, CONFIGURABLE);
    realm.define_property(sp, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("Set"), CONFIGURABLE);
    realm.define_method(sp, "union", 1, set_union);
    realm.define_method(sp, "intersection", 1, set_intersection);
    realm.define_method(sp, "difference", 1, set_difference);
    realm.define_method(sp, "symmetricDifference", 1, set_symmetric_difference);
    realm.define_method(sp, "isSubsetOf", 1, set_is_subset_of);
    realm.define_method(sp, "isSupersetOf", 1, set_is_superset_of);
    realm.define_method(sp, "isDisjointFrom", 1, set_is_disjoint_from);

    // --------------------------------------------------------- WeakMap
    let wmp = realm.new_object_with_proto(Some(op));
    realm.intrinsics.others.insert("WeakMapPrototype", wmp);
    define_ctor(realm, "WeakMap", 0, weak_map_ctor, wmp);
    realm.define_method(wmp, "get", 1, weak_map_get);
    realm.define_method(wmp, "set", 2, weak_map_set);
    realm.define_method(wmp, "has", 1, weak_map_has);
    realm.define_method(wmp, "delete", 1, weak_map_delete);
    realm.define_method(wmp, "getOrInsert", 2, weak_map_get_or_insert);
    realm.define_method(wmp, "getOrInsertComputed", 2, weak_map_get_or_insert_computed);
    realm.define_property(wmp, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("WeakMap"), CONFIGURABLE);

    // --------------------------------------------------------- WeakSet
    let wsp = realm.new_object_with_proto(Some(op));
    realm.intrinsics.others.insert("WeakSetPrototype", wsp);
    define_ctor(realm, "WeakSet", 0, weak_set_ctor, wsp);
    realm.define_method(wsp, "add", 1, weak_set_add);
    realm.define_method(wsp, "has", 1, weak_set_has);
    realm.define_method(wsp, "delete", 1, weak_set_delete);
    realm.define_property(wsp, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("WeakSet"), CONFIGURABLE);
}

fn species(_realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(this)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Map,
    Set,
    WeakMap,
    WeakSet,
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Map => "Map",
        Kind::Set => "Set",
        Kind::WeakMap => "WeakMap",
        Kind::WeakSet => "WeakSet",
    }
}

fn this_coll(realm: &mut Realm, this: &Value, kind: Kind, method: &str) -> JsResult<ObjectId> {
    if let V::Object(id) = this.v() {
        let ok = match (&realm.obj(id).kind, kind) {
            (ObjectKind::Map(_), Kind::Map) | (ObjectKind::Set(_), Kind::Set) | (ObjectKind::WeakMap(_), Kind::WeakMap) | (ObjectKind::WeakSet(_), Kind::WeakSet) => true,
            _ => false,
        };
        if ok {
            return Ok(id);
        }
    }
    realm.type_error(&format!("Method {}.prototype.{} called on incompatible receiver", kind_name(kind), method))
}

fn data(realm: &Realm, id: ObjectId) -> &MapData {
    match &realm.obj(id).kind {
        ObjectKind::Map(m) | ObjectKind::Set(m) | ObjectKind::WeakMap(m) | ObjectKind::WeakSet(m) => m,
        _ => unreachable!("not a collection"),
    }
}

fn data_mut(realm: &mut Realm, id: ObjectId) -> &mut MapData {
    match &mut realm.obj_mut(id).kind {
        ObjectKind::Map(m) | ObjectKind::Set(m) | ObjectKind::WeakMap(m) | ObjectKind::WeakSet(m) => m,
        _ => unreachable!("not a collection"),
    }
}

fn new_collection(realm: &mut Realm, kind: Kind) -> JsResult<ObjectId> {
    let default = realm.intrinsics.others[match kind {
        Kind::Map => "MapPrototype",
        Kind::Set => "SetPrototype",
        Kind::WeakMap => "WeakMapPrototype",
        Kind::WeakSet => "WeakSetPrototype",
    }];
    let proto = realm.proto_from_new_target(default);
    let d = Box::new(MapData::default());
    let k = match kind {
        Kind::Map => ObjectKind::Map(d),
        Kind::Set => ObjectKind::Set(d),
        Kind::WeakMap => ObjectKind::WeakMap(d),
        Kind::WeakSet => ObjectKind::WeakSet(d),
    };
    Ok(realm.alloc(Object::new(Some(proto), k)))
}

/// Entry `(key, value)` at index `i` of a collection, skipping tombstones.
fn entry_at(realm: &Realm, id: ObjectId, i: usize) -> Option<(Value, Value)> {
    data(realm, id).entries.get(i).and_then(|e| e.clone())
}

fn entry_count(realm: &Realm, id: ObjectId) -> usize {
    data(realm, id).entries.len()
}

fn require_object_key(realm: &mut Realm, v: &Value, what: &str) -> JsResult<()> {
    match v.v() {
        V::Object(_) => Ok(()),
        V::Symbol(s) if !realm.symbol_key_for(s).is_some() => Ok(()),
        _ => {
            let d = realm.describe_for_error(v);
            realm.type_error(&format!("Invalid value used {}: {}", what, d))
        }
    }
}

// ------------------------------------------------------------------ Map

fn map_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if !realm.is_construct_call() {
        return realm.type_error("Constructor Map requires 'new'");
    }
    let id = new_collection(realm, Kind::Map)?;
    let iterable = arg(args, 0);
    if !iterable.is_nullish() {
        add_entries_from_iterable(realm, id, iterable, "set")?;
    }
    Ok(Value::Object(id))
}

/// AddEntriesFromIterable: each item must be an object; `adder` is called
/// with (item[0], item[1]).
fn add_entries_from_iterable(realm: &mut Realm, target: ObjectId, iterable: Value, adder_name: &str) -> JsResult<()> {
    let adder = realm.get_str(Value::Object(target), adder_name)?;
    if !realm.is_callable(&adder) {
        return realm.type_error(&format!("'{}' is not a function", adder_name));
    }
    let (it, next) = realm.get_iterator(iterable)?;
    loop {
        let item = match realm.iterator_step(&it, &next)? {
            Some(v) => v,
            None => return Ok(()),
        };
        if !item.is_object() {
            let _ = realm.iterator_close(&it);
            let d = realm.describe_for_error(&item);
            return realm.type_error(&format!("Iterator value {} is not an entry object", d));
        }
        let k = realm.get(item.clone(), PropertyKey::index(0));
        let v = k.and_then(|k| realm.get(item.clone(), PropertyKey::index(1)).map(|v| (k, v)));
        let r = v.and_then(|(k, v)| realm.call(adder.clone(), Value::Object(target), &[k, v]));
        if let Err(e) = r {
            let _ = realm.iterator_close(&it);
            return Err(e);
        }
    }
}

fn map_get(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Map, "get")?;
    Ok(data(realm, id).get(&arg(args, 0)).cloned().unwrap_or(Value::Undefined))
}

fn map_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Map, "set")?;
    data_mut(realm, id).set(arg(args, 0), arg(args, 1));
    Ok(this)
}

/// getOrInsert / getOrInsertComputed (upsert) on a Map or WeakMap.
fn get_or_insert_impl(realm: &mut Realm, this: Value, args: &[Value], kind: Kind, computed: bool) -> JsResult<Value> {
    let name = if computed { "getOrInsertComputed" } else { "getOrInsert" };
    let id = this_coll(realm, &this, kind, name)?;
    let key = arg(args, 0);
    let cb = arg(args, 1);
    if computed && !realm.is_callable(&cb) {
        let d = realm.describe_for_error(&cb);
        return realm.type_error(&format!("{} is not a function", d));
    }
    if kind == Kind::WeakMap {
        require_object_key(realm, &key, "as weak map key")?;
    }
    if let Some(v) = data(realm, id).get(&key) {
        return Ok(v.clone());
    }
    let value = if computed {
        // The callback sees the canonical key (-0 becomes +0).
        let k = match key.v() {
            V::Number(n) if n == 0.0 => Value::Number(0.0),
            _ => key.clone(),
        };
        realm.call(cb, Value::Undefined, &[k])?
    } else {
        cb
    };
    data_mut(realm, id).set(key, value.clone());
    Ok(value)
}

fn map_get_or_insert(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    get_or_insert_impl(realm, this, args, Kind::Map, false)
}
fn map_get_or_insert_computed(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    get_or_insert_impl(realm, this, args, Kind::Map, true)
}
fn weak_map_get_or_insert(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    get_or_insert_impl(realm, this, args, Kind::WeakMap, false)
}
fn weak_map_get_or_insert_computed(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    get_or_insert_impl(realm, this, args, Kind::WeakMap, true)
}

fn map_has(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Map, "has")?;
    Ok(Value::Bool(data(realm, id).has(&arg(args, 0))))
}

fn map_delete(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Map, "delete")?;
    Ok(Value::Bool(data_mut(realm, id).delete(&arg(args, 0))))
}

/// `clear()` keeps positions while iterations are in progress
/// (ECMA-262 §24.1.3.1): live iterators see entries added afterwards.
fn clear_entries(d: &mut MapData) {
    d.clear();
}

fn map_clear(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Map, "clear")?;
    clear_entries(data_mut(realm, id));
    Ok(Value::Undefined)
}

fn map_size(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Map, "size")?;
    Ok(Value::Number(data(realm, id).size() as f64))
}

/// Index-based iteration that tolerates mutation during the callback.
fn for_each_impl(realm: &mut Realm, id: ObjectId, args: &[Value], is_set: bool) -> JsResult<Value> {
    let cb = arg(args, 0);
    if !realm.is_callable(&cb) {
        let d = realm.describe_for_error(&cb);
        return realm.type_error(&format!("{} is not a function", d));
    }
    let this_arg = arg(args, 1);
    // Entry positions stay put during the loop.
    data_mut(realm, id).iteration_started();
    let mut i = 0;
    let mut result = Ok(Value::Undefined);
    while i < entry_count(realm, id) {
        if let Some((k, v)) = entry_at(realm, id, i) {
            let args = if is_set { [k.clone(), k, Value::Object(id)] } else { [v, k, Value::Object(id)] };
            if let Err(e) = realm.call(cb.clone(), this_arg.clone(), &args) {
                result = Err(e);
                break;
            }
        }
        i += 1;
    }
    data_mut(realm, id).iteration_ended();
    result
}

fn map_for_each(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Map, "forEach")?;
    for_each_impl(realm, id, args, false)
}

fn map_keys(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Map, "keys")?;
    Ok(create_map_iterator(realm, id, 0, false))
}
fn map_values(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Map, "values")?;
    Ok(create_map_iterator(realm, id, 1, false))
}
fn map_entries(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Map, "entries")?;
    Ok(create_map_iterator(realm, id, 2, false))
}

fn map_group_by(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let items = arg(args, 0);
    if items.is_nullish() {
        return realm.type_error("Map.groupBy called on null or undefined");
    }
    let cb = arg(args, 1);
    if !realm.is_callable(&cb) {
        return realm.type_error("Map.groupBy: callback is not a function");
    }
    let proto = realm.intrinsics.others["MapPrototype"];
    let map = realm.alloc(Object::new(Some(proto), ObjectKind::Map(Box::new(MapData::default()))));
    let mut groups: Vec<(Value, Vec<Value>)> = Vec::new();
    realm.for_each_iterated(items, |realm, item, i| {
        let key = realm.call(cb.clone(), Value::Undefined, &[item.clone(), Value::Number(i as f64)])?;
        let key = match key.v() {
            V::Number(n) if n == 0.0 => Value::Number(0.0),
            k => k.to_value(),
        };
        match groups.iter_mut().find(|(k, _)| realm.same_value_zero(k, &key)) {
            Some((_, v)) => v.push(item),
            None => groups.push((key, vec![item])),
        }
        Ok(())
    })?;
    for (k, v) in groups {
        let arr = realm.new_array_value(v);
        data_mut(realm, map).set(k, arr);
    }
    Ok(Value::Object(map))
}

// ------------------------------------------------------------------ Set

fn set_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if !realm.is_construct_call() {
        return realm.type_error("Constructor Set requires 'new'");
    }
    let id = new_collection(realm, Kind::Set)?;
    let iterable = arg(args, 0);
    if !iterable.is_nullish() {
        let adder = realm.get_str(Value::Object(id), "add")?;
        if !realm.is_callable(&adder) {
            return realm.type_error("'add' is not a function");
        }
        let (it, next) = realm.get_iterator(iterable)?;
        while let Some(v) = realm.iterator_step(&it, &next)? {
            if let Err(e) = realm.call(adder.clone(), Value::Object(id), &[v]) {
                let _ = realm.iterator_close(&it);
                return Err(e);
            }
        }
    }
    Ok(Value::Object(id))
}

fn set_add(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Set, "add")?;
    let v = crate::heap::normalize_key(arg(args, 0));
    data_mut(realm, id).set(v.clone(), v);
    Ok(this)
}

fn set_has(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Set, "has")?;
    Ok(Value::Bool(data(realm, id).has(&arg(args, 0))))
}

fn set_delete(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Set, "delete")?;
    Ok(Value::Bool(data_mut(realm, id).delete(&arg(args, 0))))
}

fn set_clear(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Set, "clear")?;
    clear_entries(data_mut(realm, id));
    Ok(Value::Undefined)
}

fn set_size(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Set, "size")?;
    Ok(Value::Number(data(realm, id).size() as f64))
}

fn set_for_each(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Set, "forEach")?;
    for_each_impl(realm, id, args, true)
}

fn set_values(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Set, "values")?;
    Ok(create_map_iterator(realm, id, 1, true))
}
fn set_entries(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Set, "entries")?;
    Ok(create_map_iterator(realm, id, 2, true))
}

/// GetSetRecord (ES2025 §24.2.1.2): `size`, `has` and `keys` of a set-like.
struct SetRecord {
    obj: Value,
    size: f64,
    has: Value,
    keys: Value,
}

fn get_set_record(realm: &mut Realm, v: Value) -> JsResult<SetRecord> {
    if !v.is_object() {
        return realm.type_error("The argument must be a set-like object");
    }
    let raw_size = realm.get_str(v.clone(), "size")?;
    let size = realm.to_number(raw_size)?;
    if size.is_nan() {
        return realm.type_error("The 'size' property must be a number");
    }
    let size = to_integer(size);
    if size < 0.0 {
        return realm.range_error("The 'size' property must not be negative");
    }
    let has = realm.get_str(v.clone(), "has")?;
    if !realm.is_callable(&has) {
        return realm.type_error("The 'has' property must be callable");
    }
    let keys = realm.get_str(v.clone(), "keys")?;
    if !realm.is_callable(&keys) {
        return realm.type_error("The 'keys' property must be callable");
    }
    Ok(SetRecord { obj: v, size, has, keys })
}

fn record_has(realm: &mut Realm, r: &SetRecord, v: &Value) -> JsResult<bool> {
    Ok(realm.call(r.has.clone(), r.obj.clone(), &[v.clone()])?.truthy())
}

fn record_keys(realm: &mut Realm, r: &SetRecord) -> JsResult<(Value, Value)> {
    let it = realm.call(r.keys.clone(), r.obj.clone(), &[])?;
    if !it.is_object() {
        return realm.type_error("'keys' must return an object");
    }
    let next = realm.get_str(it.clone(), "next")?;
    Ok((it, next))
}

/// Whether `pred` holds for some element of the set, visiting the set's
/// entries by index while `pred` (which may run JavaScript) changes it:
/// deleted elements are skipped, added ones are visited.
fn any_live_element(realm: &mut Realm, id: ObjectId, mut pred: impl FnMut(&mut Realm, &Value) -> JsResult<bool>) -> JsResult<bool> {
    data_mut(realm, id).iteration_started();
    let mut i = 0;
    let mut result = Ok(false);
    while i < entry_count(realm, id) {
        if let Some((k, _)) = entry_at(realm, id, i) {
            match pred(realm, &k) {
                Ok(true) => {
                    result = Ok(true);
                    break;
                }
                Ok(false) => {}
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        i += 1;
    }
    data_mut(realm, id).iteration_ended();
    result
}

fn set_values_vec(realm: &Realm, id: ObjectId) -> Vec<Value> {
    data(realm, id).entries.iter().flatten().map(|(k, _)| k.clone()).collect()
}

fn new_set_from(realm: &mut Realm, values: Vec<Value>) -> Value {
    let proto = realm.intrinsics.others["SetPrototype"];
    let mut d = MapData::default();
    for v in values {
        let v = crate::heap::normalize_key(v);
        d.set(v.clone(), v);
    }
    Value::Object(realm.alloc(Object::new(Some(proto), ObjectKind::Set(Box::new(d)))))
}

fn set_union(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Set, "union")?;
    let other = get_set_record(realm, arg(args, 0))?;
    let mut values = set_values_vec(realm, id);
    let (it, next) = record_keys(realm, &other)?;
    while let Some(v) = realm.iterator_step(&it, &next)? {
        values.push(v);
    }
    Ok(new_set_from(realm, values))
}

fn set_intersection(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Set, "intersection")?;
    let other = get_set_record(realm, arg(args, 0))?;
    let mut out = Vec::new();
    if data(realm, id).size() as f64 <= other.size {
        for v in set_values_vec(realm, id) {
            if record_has(realm, &other, &v)? {
                out.push(v);
            }
        }
    } else {
        let (it, next) = record_keys(realm, &other)?;
        while let Some(v) = realm.iterator_step(&it, &next)? {
            if data(realm, id).has(&v) {
                out.push(v);
            }
        }
    }
    Ok(new_set_from(realm, out))
}

fn set_difference(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Set, "difference")?;
    let other = get_set_record(realm, arg(args, 0))?;
    let result = new_set_from(realm, set_values_vec(realm, id));
    let rid = result.as_object().unwrap();
    if data(realm, id).size() as f64 <= other.size {
        for v in set_values_vec(realm, id) {
            if record_has(realm, &other, &v)? {
                data_mut(realm, rid).delete(&v);
            }
        }
    } else {
        let (it, next) = record_keys(realm, &other)?;
        while let Some(v) = realm.iterator_step(&it, &next)? {
            data_mut(realm, rid).delete(&v);
        }
    }
    Ok(result)
}

fn set_symmetric_difference(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Set, "symmetricDifference")?;
    let other = get_set_record(realm, arg(args, 0))?;
    let result = new_set_from(realm, set_values_vec(realm, id));
    let rid = result.as_object().unwrap();
    let (it, next) = record_keys(realm, &other)?;
    while let Some(v) = realm.iterator_step(&it, &next)? {
        // Keys are canonicalized (-0 becomes +0).
        let v = match v.v() {
            V::Number(n) if n == 0.0 => Value::Number(0.0),
            _ => v,
        };
        if data(realm, id).has(&v) {
            data_mut(realm, rid).delete(&v);
        } else {
            let d = data_mut(realm, rid);
            if !d.has(&v) {
                d.set(v.clone(), v);
            }
        }
    }
    Ok(result)
}

fn set_is_subset_of(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Set, "isSubsetOf")?;
    let other = get_set_record(realm, arg(args, 0))?;
    if data(realm, id).size() as f64 > other.size {
        return Ok(Value::Bool(false));
    }
    let r = any_live_element(realm, id, |realm, v| Ok(!record_has(realm, &other, v)?))?;
    Ok(Value::Bool(!r))
}

fn set_is_superset_of(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Set, "isSupersetOf")?;
    let other = get_set_record(realm, arg(args, 0))?;
    if (data(realm, id).size() as f64) < other.size {
        return Ok(Value::Bool(false));
    }
    let (it, next) = record_keys(realm, &other)?;
    while let Some(v) = realm.iterator_step(&it, &next)? {
        if !data(realm, id).has(&v) {
            let _ = realm.iterator_close(&it);
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

fn set_is_disjoint_from(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::Set, "isDisjointFrom")?;
    let other = get_set_record(realm, arg(args, 0))?;
    if data(realm, id).size() as f64 <= other.size {
        if any_live_element(realm, id, |realm, v| record_has(realm, &other, v))? {
            return Ok(Value::Bool(false));
        }
    } else {
        let (it, next) = record_keys(realm, &other)?;
        while let Some(v) = realm.iterator_step(&it, &next)? {
            if data(realm, id).has(&v) {
                let _ = realm.iterator_close(&it);
                return Ok(Value::Bool(false));
            }
        }
    }
    Ok(Value::Bool(true))
}

// -------------------------------------------------------------- WeakMap

fn weak_map_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if !realm.is_construct_call() {
        return realm.type_error("Constructor WeakMap requires 'new'");
    }
    let id = new_collection(realm, Kind::WeakMap)?;
    let iterable = arg(args, 0);
    if !iterable.is_nullish() {
        add_entries_from_iterable(realm, id, iterable, "set")?;
    }
    Ok(Value::Object(id))
}

fn weak_map_get(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::WeakMap, "get")?;
    Ok(data(realm, id).get(&arg(args, 0)).cloned().unwrap_or(Value::Undefined))
}

fn weak_map_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::WeakMap, "set")?;
    let k = arg(args, 0);
    require_object_key(realm, &k, "as weak map key")?;
    data_mut(realm, id).set(k, arg(args, 1));
    Ok(this)
}

fn weak_map_has(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::WeakMap, "has")?;
    Ok(Value::Bool(data(realm, id).has(&arg(args, 0))))
}

fn weak_map_delete(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::WeakMap, "delete")?;
    Ok(Value::Bool(data_mut(realm, id).delete(&arg(args, 0))))
}

// -------------------------------------------------------------- WeakSet

fn weak_set_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if !realm.is_construct_call() {
        return realm.type_error("Constructor WeakSet requires 'new'");
    }
    let id = new_collection(realm, Kind::WeakSet)?;
    let iterable = arg(args, 0);
    if !iterable.is_nullish() {
        let adder = realm.get_str(Value::Object(id), "add")?;
        if !realm.is_callable(&adder) {
            return realm.type_error("'add' is not a function");
        }
        let (it, next) = realm.get_iterator(iterable)?;
        while let Some(v) = realm.iterator_step(&it, &next)? {
            if let Err(e) = realm.call(adder.clone(), Value::Object(id), &[v]) {
                let _ = realm.iterator_close(&it);
                return Err(e);
            }
        }
    }
    Ok(Value::Object(id))
}

fn weak_set_add(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::WeakSet, "add")?;
    let v = arg(args, 0);
    require_object_key(realm, &v, "in weak set")?;
    let d = data_mut(realm, id);
    if !d.has(&v) {
        d.set(v.clone(), v);
    }
    Ok(this)
}

fn weak_set_has(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::WeakSet, "has")?;
    Ok(Value::Bool(data(realm, id).has(&arg(args, 0))))
}

fn weak_set_delete(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_coll(realm, &this, Kind::WeakSet, "delete")?;
    Ok(Value::Bool(data_mut(realm, id).delete(&arg(args, 0))))
}
