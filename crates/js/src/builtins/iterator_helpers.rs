//! The `Iterator` global and the iterator helpers: Iterator.from / concat /
//! zip / zipKeyed, %IteratorHelperPrototype%, %WrapForValidIteratorPrototype%,
//! the lazy helpers (map, filter, take, drop, flatMap, chunks, windows), the
//! eager ones (reduce, toArray, forEach, some, every, find, includes) and the
//! `Symbol.dispose` / `Symbol.asyncDispose` methods of the iterator
//! prototypes.
//!
//! Helpers are state machines over [`SlotData`] instead of real generators:
//! each `next()` runs the spec closure up to its next `Yield`.

use super::*;
use crate::realm::{SYM_ASYNC_DISPOSE, SYM_DISPOSE, SYM_ITERATOR, SYM_TO_STRING_TAG};

pub(crate) const TAG_HELPER: u8 = 1;
pub(crate) const TAG_WRAP: u8 = 2;

// generator states
const ST_START: u8 = 0;
const ST_YIELD: u8 = 1;
const ST_RUNNING: u8 = 2;
const ST_DONE: u8 = 3;

// helper kinds
const K_MAP: u8 = 0;
const K_FILTER: u8 = 1;
const K_TAKE: u8 = 2;
const K_DROP: u8 = 3;
const K_FLATMAP: u8 = 4;
const K_CONCAT: u8 = 5;
const K_ZIP: u8 = 6;
const K_CHUNKS: u8 = 7;
const K_WINDOWS: u8 = 8;

// helper value slots
const S_ITER: usize = 0;
const S_NEXT: usize = 1;
const S_FN: usize = 2;
const S_INNER: usize = 3;
const S_INNER_NEXT: usize = 4;

// zip modes (num[1])
const Z_SHORTEST: f64 = 0.0;
const Z_LONGEST: f64 = 1.0;
const Z_STRICT: f64 = 2.0;

pub fn register(realm: &mut Realm) {
    let ip = realm.intrinsics.iterator_proto;
    let ctor = define_ctor(realm, "Iterator", 0, iterator_ctor, ip);
    realm.intrinsics.others.insert("Iterator", ctor);
    // `constructor` and `@@toStringTag` are accessors on Iterator.prototype.
    realm.define_accessor(ip, PropertyKey::Symbol(SYM_TO_STRING_TAG), Some(get_to_string_tag), Some(set_to_string_tag), CONFIGURABLE);
    realm.define_accessor(ip, "constructor", Some(get_constructor), Some(set_constructor), CONFIGURABLE);
    realm.define_method(ctor, "from", 1, iterator_from);
    realm.define_method(ctor, "concat", 0, iterator_concat);
    realm.define_method(ctor, "zip", 1, iterator_zip);
    realm.define_method(ctor, "zipKeyed", 1, iterator_zip_keyed);

    realm.define_method(ip, "map", 1, proto_map);
    realm.define_method(ip, "filter", 1, proto_filter);
    realm.define_method(ip, "take", 1, proto_take);
    realm.define_method(ip, "drop", 1, proto_drop);
    realm.define_method(ip, "flatMap", 1, proto_flat_map);
    realm.define_method(ip, "reduce", 1, proto_reduce);
    realm.define_method(ip, "toArray", 0, proto_to_array);
    realm.define_method(ip, "forEach", 1, proto_for_each);
    realm.define_method(ip, "some", 1, proto_some);
    realm.define_method(ip, "every", 1, proto_every);
    realm.define_method(ip, "find", 1, proto_find);
    realm.define_method(ip, "includes", 1, proto_includes);
    realm.define_method(ip, "chunks", 1, proto_chunks);
    realm.define_method(ip, "windows", 1, proto_windows);
    realm.define_symbol_method(ip, SYM_DISPOSE, "[Symbol.dispose]", 0, iterator_dispose);

    // %IteratorHelperPrototype%
    let hp = realm.new_object_with_proto(Some(ip));
    realm.define_method(hp, "next", 0, helper_next);
    realm.define_method(hp, "return", 0, helper_return);
    realm.define_property(hp, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("Iterator Helper"), CONFIGURABLE);
    realm.intrinsics.others.insert("IteratorHelperPrototype", hp);

    // %WrapForValidIteratorPrototype%
    let wp = realm.new_object_with_proto(Some(ip));
    realm.define_method(wp, "next", 0, wrap_next);
    realm.define_method(wp, "return", 0, wrap_return);
    realm.intrinsics.others.insert("WrapForValidIteratorPrototype", wp);

    // %AsyncIteratorPrototype%[@@asyncDispose]
    if let Some(&aip) = realm.intrinsics.others.get("AsyncIteratorPrototype") {
        realm.define_symbol_method(aip, SYM_ASYNC_DISPOSE, "[Symbol.asyncDispose]", 0, async_iterator_dispose);
    }
}

// ------------------------------------------------------------ utilities

fn slots(realm: &Realm, id: ObjectId) -> &SlotData {
    match &realm.obj(id).kind {
        ObjectKind::Slots(d) => d,
        _ => unreachable!("not a slot object"),
    }
}

fn slots_mut(realm: &mut Realm, id: ObjectId) -> &mut SlotData {
    match &mut realm.obj_mut(id).kind {
        ObjectKind::Slots(d) => d,
        _ => unreachable!("not a slot object"),
    }
}

/// The object id of `v` when it carries internal slots with `tag`.
pub(crate) fn slot_object(realm: &Realm, v: &Value, tag: u8) -> Option<ObjectId> {
    if let V::Object(id) = v.v() {
        if let ObjectKind::Slots(d) = &realm.obj(id).kind {
            if d.tag == tag {
                return Some(id);
            }
        }
    }
    None
}

pub(crate) fn new_slot_object(realm: &mut Realm, proto: ObjectId, data: SlotData) -> ObjectId {
    realm.alloc(Object::new(Some(proto), ObjectKind::Slots(Box::new(data))))
}

fn require_object(realm: &mut Realm, this: &Value, method: &str) -> JsResult<()> {
    if this.is_object() {
        Ok(())
    } else {
        realm.type_error(&format!("Iterator.prototype.{} called on non-object", method))
    }
}

/// IteratorClose(iterated, throw completion): close, ignoring any error of
/// the close itself, and hand back the original error.
fn close_with_error(realm: &mut Realm, it: &Value, err: Value) -> Value {
    let _ = realm.iterator_close(it);
    err
}

/// IteratorClose with a fresh TypeError.
fn close_type_error<T>(realm: &mut Realm, it: &Value, msg: &str) -> JsResult<T> {
    let e = realm.throw_type_error(msg);
    Err(close_with_error(realm, it, e))
}

fn close_range_error<T>(realm: &mut Realm, it: &Value, msg: &str) -> JsResult<T> {
    let e = realm.new_error("RangeError", msg);
    Err(close_with_error(realm, it, e))
}

/// GetIteratorDirect: (iterator, next method).
fn get_iterator_direct(realm: &mut Realm, o: &Value) -> JsResult<(Value, Value)> {
    let next = realm.get_str(o.clone(), "next")?;
    Ok((o.clone(), next))
}

/// GetIteratorFlattenable(obj, iterate-string-primitives | reject-primitives).
pub(crate) fn get_iterator_flattenable(realm: &mut Realm, obj: Value, allow_strings: bool) -> JsResult<(Value, Value)> {
    if !obj.is_object() && !(allow_strings && obj.is_string()) {
        let d = realm.describe_for_error(&obj);
        return realm.type_error(&format!("{} is not an iterator or iterable object", d));
    }
    let method = realm.get_method(obj.clone(), PropertyKey::Symbol(SYM_ITERATOR))?;
    let iterator = match method {
        None => obj,
        Some(m) => realm.call(m, obj, &[])?,
    };
    if !iterator.is_object() {
        return realm.type_error("iterator is not an object");
    }
    get_iterator_direct(realm, &iterator)
}

/// IteratorStep (without reading `value`): Some(result) or None when done.
fn iterator_step_result(realm: &mut Realm, it: &Value, next: &Value) -> JsResult<Option<Value>> {
    let r = realm.call(next.clone(), it.clone(), &[])?;
    if !r.is_object() {
        return realm.type_error("Iterator result is not an object");
    }
    if realm.get_str(r.clone(), "done")?.truthy() {
        return Ok(None);
    }
    Ok(Some(r))
}

/// Callable argument check that closes the receiver on failure.
fn require_callable(realm: &mut Realm, o: &Value, f: &Value, what: &str) -> JsResult<()> {
    if realm.is_callable(f) {
        Ok(())
    } else {
        let d = realm.describe_for_error(f);
        close_type_error(realm, o, &format!("{} is not a function ({})", d, what))
    }
}

// ---------------------------------------------------------- constructor

fn iterator_ctor(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    let nt = realm.new_target();
    let me = realm.intrinsics.others["Iterator"];
    match nt.v() {
        V::Object(f) if f != me => {}
        _ => return realm.type_error("Iterator is an abstract class and cannot be constructed directly"),
    }
    let ip = realm.intrinsics.iterator_proto;
    let proto = realm.proto_from_new_target_checked(ip)?;
    Ok(Value::Object(realm.new_object_with_proto(Some(proto))))
}

/// SetterThatIgnoresPrototypeProperties(this, home, p, v).
fn setter_ignoring_proto(realm: &mut Realm, this: Value, home: ObjectId, key: PropertyKey, v: Value) -> JsResult<Value> {
    let id = match this.v() {
        V::Object(id) => id,
        _ => return realm.type_error("setter called on non-object"),
    };
    if id == home {
        return realm.type_error("Cannot assign to read only property of the iterator prototype");
    }
    match realm.get_own_property_any(id, &key)? {
        None => {
            if !realm.define_own(id, key, Property::data(v, ALL))? {
                return realm.type_error("Cannot define property");
            }
        }
        Some(_) => {
            if !realm.set(this, key, v)? {
                return realm.type_error("Cannot assign to read only property");
            }
        }
    }
    Ok(Value::Undefined)
}

fn get_to_string_tag(_realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(Value::str("Iterator"))
}

fn set_to_string_tag(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let ip = realm.intrinsics.iterator_proto;
    setter_ignoring_proto(realm, this, ip, PropertyKey::Symbol(SYM_TO_STRING_TAG), arg(args, 0))
}

fn get_constructor(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(Value::Object(realm.intrinsics.others["Iterator"]))
}

fn set_constructor(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let ip = realm.intrinsics.iterator_proto;
    setter_ignoring_proto(realm, this, ip, PropertyKey::str("constructor"), arg(args, 0))
}

// --------------------------------------------------------- Iterator.from

fn iterator_from(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let (it, next) = get_iterator_flattenable(realm, arg(args, 0), true)?;
    let ctor = Value::Object(realm.intrinsics.others["Iterator"]);
    if realm.ordinary_has_instance(ctor, it.clone())? {
        return Ok(it);
    }
    let wp = realm.intrinsics.others["WrapForValidIteratorPrototype"];
    let data = SlotData { tag: TAG_WRAP, values: vec![it, next], ..Default::default() };
    Ok(Value::Object(new_slot_object(realm, wp, data)))
}

fn this_wrap(realm: &mut Realm, this: &Value) -> JsResult<(Value, Value)> {
    match slot_object(realm, this, TAG_WRAP) {
        Some(id) => {
            let d = slots(realm, id);
            Ok((d.values[0].clone(), d.values[1].clone()))
        }
        None => realm.type_error("next method called on incompatible receiver"),
    }
}

fn wrap_next(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (it, next) = this_wrap(realm, &this)?;
    realm.call(next, it, &[])
}

fn wrap_return(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (it, _) = this_wrap(realm, &this)?;
    match realm.get_method(it.clone(), PropertyKey::str("return"))? {
        None => Ok(realm.create_iter_result(Value::Undefined, true)),
        Some(f) => realm.call(f, it, &[]),
    }
}

// -------------------------------------------------- %IteratorHelperPrototype%

fn new_helper(realm: &mut Realm, kind: u8, iter: Value, next: Value, f: Value, n: f64) -> Value {
    let hp = realm.intrinsics.others["IteratorHelperPrototype"];
    let data = SlotData {
        tag: TAG_HELPER,
        kind,
        state: ST_START,
        num: [0.0, n],
        values: vec![iter, next, f, Value::Undefined, Value::Undefined],
        list: Vec::new(),
    };
    Value::Object(new_slot_object(realm, hp, data))
}

fn this_helper(realm: &mut Realm, this: &Value) -> JsResult<ObjectId> {
    match slot_object(realm, this, TAG_HELPER) {
        Some(id) => Ok(id),
        None => realm.type_error("Iterator Helper method called on incompatible receiver"),
    }
}

fn helper_next(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_helper(realm, &this)?;
    match slots(realm, id).state {
        ST_RUNNING => return realm.type_error("Generator is already running"),
        ST_DONE => return Ok(realm.create_iter_result(Value::Undefined, true)),
        _ => {}
    }
    slots_mut(realm, id).state = ST_RUNNING;
    let r = helper_step(realm, id);
    match r {
        Ok(Some(v)) => {
            slots_mut(realm, id).state = ST_YIELD;
            Ok(realm.create_iter_result(v, false))
        }
        Ok(None) => {
            finish_helper(realm, id);
            Ok(realm.create_iter_result(Value::Undefined, true))
        }
        Err(e) => {
            finish_helper(realm, id);
            Err(e)
        }
    }
}

fn finish_helper(realm: &mut Realm, id: ObjectId) {
    let d = slots_mut(realm, id);
    d.state = ST_DONE;
    for v in d.values.iter_mut() {
        *v = Value::Undefined;
    }
    d.list.clear();
}

fn helper_return(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_helper(realm, &this)?;
    let (state, kind) = {
        let d = slots(realm, id);
        (d.state, d.kind)
    };
    match state {
        ST_START => {
            // Completed before running: close the underlying iterators.
            let it = slots(realm, id).values[S_ITER].clone();
            slots_mut(realm, id).state = ST_DONE;
            let r = match kind {
                K_CONCAT => Ok(()),
                K_ZIP => zip_close_all(realm, id, Ok(())),
                _ => realm.iterator_close(&it),
            };
            finish_helper(realm, id);
            r?;
            Ok(realm.create_iter_result(Value::Undefined, true))
        }
        ST_RUNNING => realm.type_error("Generator is already running"),
        ST_DONE => Ok(realm.create_iter_result(Value::Undefined, true)),
        _ => {
            slots_mut(realm, id).state = ST_RUNNING;
            let r = helper_abrupt_return(realm, id, kind);
            finish_helper(realm, id);
            r?;
            Ok(realm.create_iter_result(Value::Undefined, true))
        }
    }
}

/// A `return` completion delivered at the closure's `Yield`.
fn helper_abrupt_return(realm: &mut Realm, id: ObjectId, kind: u8) -> JsResult<()> {
    let (it, inner, finishing) = {
        let d = slots(realm, id);
        (d.values[S_ITER].clone(), d.values[S_INNER].clone(), d.values[S_INNER].truthy())
    };
    match kind {
        K_FLATMAP => {
            if inner.is_object() {
                if let Err(e) = realm.iterator_close(&inner) {
                    return Err(close_with_error(realm, &it, e));
                }
            }
            realm.iterator_close(&it)
        }
        K_CONCAT => {
            if inner.is_object() {
                realm.iterator_close(&inner)
            } else {
                Ok(())
            }
        }
        K_ZIP => zip_close_all(realm, id, Ok(())),
        // The final partial chunk / window is yielded after exhaustion.
        K_CHUNKS | K_WINDOWS if finishing => Ok(()),
        _ => realm.iterator_close(&it),
    }
}

/// Run the helper's closure up to its next `Yield` (Some) or its end (None).
fn helper_step(realm: &mut Realm, id: ObjectId) -> JsResult<Option<Value>> {
    let (kind, it, next, f) = {
        let d = slots(realm, id);
        (d.kind, d.values[S_ITER].clone(), d.values[S_NEXT].clone(), d.values[S_FN].clone())
    };
    match kind {
        K_MAP => {
            let v = match realm.iterator_step(&it, &next)? {
                Some(v) => v,
                None => return Ok(None),
            };
            let c = bump_counter(realm, id);
            match realm.call(f, Value::Undefined, &[v, Value::Number(c)]) {
                Ok(m) => Ok(Some(m)),
                Err(e) => Err(close_with_error(realm, &it, e)),
            }
        }
        K_FILTER => loop {
            let v = match realm.iterator_step(&it, &next)? {
                Some(v) => v,
                None => return Ok(None),
            };
            let c = bump_counter(realm, id);
            match realm.call(f.clone(), Value::Undefined, &[v.clone(), Value::Number(c)]) {
                Ok(sel) => {
                    if realm.to_boolean(&sel) {
                        return Ok(Some(v));
                    }
                }
                Err(e) => return Err(close_with_error(realm, &it, e)),
            }
        },
        K_TAKE => {
            let rem = slots(realm, id).num[1];
            if rem == 0.0 {
                realm.iterator_close(&it)?;
                return Ok(None);
            }
            if rem != f64::INFINITY {
                slots_mut(realm, id).num[1] = rem - 1.0;
            }
            realm.iterator_step(&it, &next)
        }
        K_DROP => {
            loop {
                let rem = slots(realm, id).num[1];
                if rem <= 0.0 {
                    break;
                }
                if rem != f64::INFINITY {
                    slots_mut(realm, id).num[1] = rem - 1.0;
                }
                if iterator_step_result(realm, &it, &next)?.is_none() {
                    return Ok(None);
                }
            }
            realm.iterator_step(&it, &next)
        }
        K_FLATMAP => loop {
            let (inner, inner_next) = {
                let d = slots(realm, id);
                (d.values[S_INNER].clone(), d.values[S_INNER_NEXT].clone())
            };
            if inner.is_object() {
                match realm.iterator_step(&inner, &inner_next) {
                    Ok(Some(v)) => return Ok(Some(v)),
                    Ok(None) => {
                        let d = slots_mut(realm, id);
                        d.values[S_INNER] = Value::Undefined;
                        d.values[S_INNER_NEXT] = Value::Undefined;
                        continue;
                    }
                    Err(e) => return Err(close_with_error(realm, &it, e)),
                }
            }
            let v = match realm.iterator_step(&it, &next)? {
                Some(v) => v,
                None => return Ok(None),
            };
            let c = bump_counter(realm, id);
            let mapped = match realm.call(f.clone(), Value::Undefined, &[v, Value::Number(c)]) {
                Ok(m) => m,
                Err(e) => return Err(close_with_error(realm, &it, e)),
            };
            match get_iterator_flattenable(realm, mapped, false) {
                Ok((i, n)) => {
                    let d = slots_mut(realm, id);
                    d.values[S_INNER] = i;
                    d.values[S_INNER_NEXT] = n;
                }
                Err(e) => return Err(close_with_error(realm, &it, e)),
            }
        },
        K_CONCAT => loop {
            let (inner, inner_next) = {
                let d = slots(realm, id);
                (d.values[S_INNER].clone(), d.values[S_INNER_NEXT].clone())
            };
            if inner.is_object() {
                match realm.iterator_step(&inner, &inner_next)? {
                    Some(v) => return Ok(Some(v)),
                    None => {
                        let d = slots_mut(realm, id);
                        d.values[S_INNER] = Value::Undefined;
                        d.values[S_INNER_NEXT] = Value::Undefined;
                        continue;
                    }
                }
            }
            let (idx, item, method) = {
                let d = slots(realm, id);
                let idx = d.num[1] as usize;
                if idx * 2 >= d.list.len() {
                    return Ok(None);
                }
                (idx, d.list[idx * 2].clone(), d.list[idx * 2 + 1].clone())
            };
            slots_mut(realm, id).num[1] = (idx + 1) as f64;
            let iter = realm.call(method, item, &[])?;
            if !iter.is_object() {
                return realm.type_error("iterator is not an object");
            }
            let n = realm.get_str(iter.clone(), "next")?;
            let d = slots_mut(realm, id);
            d.values[S_INNER] = iter;
            d.values[S_INNER_NEXT] = n;
        },
        K_ZIP => zip_step(realm, id),
        K_CHUNKS => {
            if slots(realm, id).values[S_INNER].truthy() {
                return Ok(None);
            }
            let size = slots(realm, id).num[1];
            loop {
                match realm.iterator_step(&it, &next)? {
                    None => {
                        let d = slots_mut(realm, id);
                        if d.list.is_empty() {
                            return Ok(None);
                        }
                        d.values[S_INNER] = Value::Bool(true);
                        let buf = std::mem::take(&mut d.list);
                        return Ok(Some(realm.new_array_value(buf)));
                    }
                    Some(v) => {
                        let d = slots_mut(realm, id);
                        d.list.push(v);
                        if d.list.len() as f64 >= size {
                            let buf = std::mem::take(&mut d.list);
                            return Ok(Some(realm.new_array_value(buf)));
                        }
                    }
                }
            }
        }
        K_WINDOWS => {
            if slots(realm, id).values[S_INNER].truthy() {
                return Ok(None);
            }
            let size = slots(realm, id).num[1];
            let allow_partial = f.truthy();
            loop {
                match realm.iterator_step(&it, &next)? {
                    None => {
                        let d = slots_mut(realm, id);
                        if allow_partial && !d.list.is_empty() && (d.list.len() as f64) < size {
                            d.values[S_INNER] = Value::Bool(true);
                            let buf = d.list.clone();
                            return Ok(Some(realm.new_array_value(buf)));
                        }
                        return Ok(None);
                    }
                    Some(v) => {
                        let d = slots_mut(realm, id);
                        if d.list.len() as f64 >= size {
                            d.list.remove(0);
                        }
                        d.list.push(v);
                        if d.list.len() as f64 >= size {
                            let buf = d.list.clone();
                            return Ok(Some(realm.new_array_value(buf)));
                        }
                    }
                }
            }
        }
        _ => Ok(None),
    }
}

fn bump_counter(realm: &mut Realm, id: ObjectId) -> f64 {
    let d = slots_mut(realm, id);
    let c = d.num[0];
    d.num[0] = c + 1.0;
    c
}

// ------------------------------------------------- lazy prototype methods

fn proto_map(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    lazy_with_fn(realm, this, args, K_MAP, "map")
}

fn proto_filter(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    lazy_with_fn(realm, this, args, K_FILTER, "filter")
}

fn proto_flat_map(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    lazy_with_fn(realm, this, args, K_FLATMAP, "flatMap")
}

fn lazy_with_fn(realm: &mut Realm, this: Value, args: &[Value], kind: u8, name: &str) -> JsResult<Value> {
    require_object(realm, &this, name)?;
    let f = arg(args, 0);
    require_callable(realm, &this, &f, name)?;
    let (it, next) = get_iterator_direct(realm, &this)?;
    Ok(new_helper(realm, kind, it, next, f, 0.0))
}

/// Shared argument handling of take / drop.
fn limit_arg(realm: &mut Realm, this: &Value, limit: Value) -> JsResult<f64> {
    let n = match realm.to_number(limit) {
        Ok(n) => n,
        Err(e) => return Err(close_with_error(realm, this, e)),
    };
    if n.is_nan() {
        return close_range_error(realm, this, "limit must be a number");
    }
    let n = if n.is_infinite() { n } else { n.trunc() + 0.0 };
    if n < 0.0 {
        return close_range_error(realm, this, "limit must be positive");
    }
    if n.is_finite() && n > 9007199254740991.0 {
        return close_range_error(realm, this, "limit is too large");
    }
    Ok(n)
}

fn proto_take(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    require_object(realm, &this, "take")?;
    let n = limit_arg(realm, &this, arg(args, 0))?;
    let (it, next) = get_iterator_direct(realm, &this)?;
    Ok(new_helper(realm, K_TAKE, it, next, Value::Undefined, n))
}

fn proto_drop(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    require_object(realm, &this, "drop")?;
    let n = limit_arg(realm, &this, arg(args, 0))?;
    let (it, next) = get_iterator_direct(realm, &this)?;
    Ok(new_helper(realm, K_DROP, it, next, Value::Undefined, n))
}

/// chunkSize / windowSize: an integral Number in [1, 2^32 - 1], not coerced.
fn size_arg(realm: &mut Realm, this: &Value, v: &Value, what: &str) -> JsResult<f64> {
    let n = match v.v() {
        V::Number(n) => n,
        _ => return close_type_error(realm, this, &format!("{} must be a number", what)),
    };
    if !(n.is_finite() && n.trunc() == n) {
        return close_type_error(realm, this, &format!("{} must be an integer", what));
    }
    if !(n >= 1.0 && n <= 4294967295.0) {
        return close_range_error(realm, this, &format!("{} must be an integer in [1, 2^32-1]", what));
    }
    Ok(n)
}

fn proto_chunks(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    require_object(realm, &this, "chunks")?;
    let n = size_arg(realm, &this, &arg(args, 0), "chunkSize")?;
    let (it, next) = get_iterator_direct(realm, &this)?;
    Ok(new_helper(realm, K_CHUNKS, it, next, Value::Undefined, n))
}

fn proto_windows(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    require_object(realm, &this, "windows")?;
    let n = size_arg(realm, &this, &arg(args, 0), "windowSize")?;
    let undersized = arg(args, 1);
    let allow_partial = match undersized.v() {
        V::Undefined => false,
        V::String(s) if s.eq_str("only-full") => false,
        V::String(s) if s.eq_str("allow-partial") => true,
        _ => return close_type_error(realm, &this, "undersized must be \"only-full\" or \"allow-partial\""),
    };
    let (it, next) = get_iterator_direct(realm, &this)?;
    Ok(new_helper(realm, K_WINDOWS, it, next, Value::Bool(allow_partial), n))
}

// ------------------------------------------------ eager prototype methods

fn proto_reduce(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    require_object(realm, &this, "reduce")?;
    let f = arg(args, 0);
    require_callable(realm, &this, &f, "reduce")?;
    let (it, next) = get_iterator_direct(realm, &this)?;
    let (mut acc, mut counter) = if args.len() < 2 {
        match realm.iterator_step(&it, &next)? {
            Some(v) => (v, 1.0),
            None => return realm.type_error("Reduce of empty iterator with no initial value"),
        }
    } else {
        (args[1].clone(), 0.0)
    };
    while let Some(v) = realm.iterator_step(&it, &next)? {
        match realm.call(f.clone(), Value::Undefined, &[acc, v, Value::Number(counter)]) {
            Ok(r) => acc = r,
            Err(e) => return Err(close_with_error(realm, &it, e)),
        }
        counter += 1.0;
    }
    Ok(acc)
}

fn proto_to_array(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    require_object(realm, &this, "toArray")?;
    let (it, next) = get_iterator_direct(realm, &this)?;
    let mut out = Vec::new();
    while let Some(v) = realm.iterator_step(&it, &next)? {
        out.push(v);
    }
    Ok(realm.new_array_value(out))
}

fn proto_for_each(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    require_object(realm, &this, "forEach")?;
    let f = arg(args, 0);
    require_callable(realm, &this, &f, "forEach")?;
    let (it, next) = get_iterator_direct(realm, &this)?;
    let mut counter = 0.0;
    while let Some(v) = realm.iterator_step(&it, &next)? {
        if let Err(e) = realm.call(f.clone(), Value::Undefined, &[v, Value::Number(counter)]) {
            return Err(close_with_error(realm, &it, e));
        }
        counter += 1.0;
    }
    Ok(Value::Undefined)
}

/// some / every / find: 0, 1, 2.
fn predicate_search(realm: &mut Realm, this: Value, args: &[Value], mode: u8, name: &str) -> JsResult<Value> {
    require_object(realm, &this, name)?;
    let f = arg(args, 0);
    require_callable(realm, &this, &f, name)?;
    let (it, next) = get_iterator_direct(realm, &this)?;
    let mut counter = 0.0;
    while let Some(v) = realm.iterator_step(&it, &next)? {
        let r = match realm.call(f.clone(), Value::Undefined, &[v.clone(), Value::Number(counter)]) {
            Ok(r) => realm.to_boolean(&r),
            Err(e) => return Err(close_with_error(realm, &it, e)),
        };
        let found = match mode {
            0 if r => Some(Value::Bool(true)),
            1 if !r => Some(Value::Bool(false)),
            2 if r => Some(v),
            _ => None,
        };
        if let Some(result) = found {
            realm.iterator_close(&it)?;
            return Ok(result);
        }
        counter += 1.0;
    }
    Ok(match mode {
        0 => Value::Bool(false),
        1 => Value::Bool(true),
        _ => Value::Undefined,
    })
}

fn proto_some(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    predicate_search(realm, this, args, 0, "some")
}

fn proto_every(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    predicate_search(realm, this, args, 1, "every")
}

fn proto_find(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    predicate_search(realm, this, args, 2, "find")
}

fn proto_includes(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    require_object(realm, &this, "includes")?;
    let search = arg(args, 0);
    let skip_arg = arg(args, 1);
    let to_skip = match skip_arg.v() {
        V::Undefined => 0.0,
        V::Number(n) if n.is_infinite() || (n.is_finite() && n.trunc() == n) => n,
        _ => return close_type_error(realm, &this, "skippedElements must be an integral number"),
    };
    if to_skip < 0.0 {
        return close_range_error(realm, &this, "skippedElements must not be negative");
    }
    if to_skip.is_finite() && to_skip > 9007199254740991.0 {
        return close_range_error(realm, &this, "skippedElements is too large");
    }
    let (it, next) = get_iterator_direct(realm, &this)?;
    let mut skipped = 0.0;
    while let Some(v) = realm.iterator_step(&it, &next)? {
        if skipped < to_skip {
            skipped += 1.0;
        } else if realm.same_value_zero(&v, &search) {
            realm.iterator_close(&it)?;
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

// ------------------------------------------------------- Symbol.dispose

fn iterator_dispose(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    if let Some(f) = realm.get_method(this.clone(), PropertyKey::str("return"))? {
        realm.call(f, this, &[])?;
    }
    Ok(Value::Undefined)
}

fn async_iterator_dispose(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (p, res, rej) = realm.new_promise();
    let r = (|| -> JsResult<Option<ObjectId>> {
        match realm.get_method(this.clone(), PropertyKey::str("return"))? {
            None => Ok(None),
            Some(f) => {
                let result = realm.call(f, this.clone(), &[])?;
                Ok(Some(crate::builtins::promise::promise_resolve(realm, result)?))
            }
        }
    })();
    match r {
        Ok(None) => {
            realm.call(Value::Object(res), Value::Undefined, &[Value::Undefined])?;
        }
        Ok(Some(wrapper)) => {
            crate::builtins::promise::perform_then_native(realm, wrapper, dispose_fulfilled, dispose_rejected, Value::Object(p));
        }
        Err(e) => {
            realm.call(Value::Object(rej), Value::Undefined, &[e])?;
        }
    }
    Ok(Value::Object(p))
}

fn native_capture(realm: &Realm) -> Value {
    realm.current_native().and_then(|n| realm.native_captures(n).into_iter().next()).unwrap_or(Value::Undefined)
}

fn dispose_fulfilled(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    if let V::Object(p) = native_capture(realm).v() {
        realm.resolve_promise(p, Value::Undefined);
    }
    Ok(Value::Undefined)
}

fn dispose_rejected(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if let V::Object(p) = native_capture(realm).v() {
        realm.reject_promise(p, arg(args, 0));
    }
    Ok(Value::Undefined)
}

// ------------------------------------------------------- Iterator.concat

fn iterator_concat(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let mut list = Vec::with_capacity(args.len() * 2);
    for item in args {
        if !item.is_object() {
            let d = realm.describe_for_error(item);
            return realm.type_error(&format!("{} is not an object", d));
        }
        let method = match realm.get_method(item.clone(), PropertyKey::Symbol(SYM_ITERATOR))? {
            Some(m) => m,
            None => {
                let d = realm.describe_for_error(item);
                return realm.type_error(&format!("{} is not iterable", d));
            }
        };
        list.push(item.clone());
        list.push(method);
    }
    let h = new_helper(realm, K_CONCAT, Value::Undefined, Value::Undefined, Value::Undefined, 0.0);
    if let V::Object(id) = h.v() {
        slots_mut(realm, id).list = list;
    }
    Ok(h)
}

// ---------------------------------------------------- Iterator.zip / zipKeyed

/// IteratorCloseAll over the iterators of a zip helper that are still open
/// (in reverse order).
fn zip_close_all(realm: &mut Realm, id: ObjectId, mut completion: JsResult<()>) -> JsResult<()> {
    let n = slots(realm, id).list.len() / 4;
    for i in (0..n).rev() {
        let (it, open) = {
            let d = slots(realm, id);
            (d.list[i * 4].clone(), d.list[i * 4 + 3].truthy())
        };
        if !open {
            continue;
        }
        slots_mut(realm, id).list[i * 4 + 3] = Value::Bool(false);
        let r = realm.iterator_close(&it);
        if completion.is_ok() {
            completion = r;
        }
    }
    completion
}

/// IteratorCloseAll over a plain list of iterators (argument processing).
fn close_all_list(realm: &mut Realm, iters: &[(Value, Value)], err: Value) -> Value {
    for (it, _) in iters.iter().rev() {
        let _ = realm.iterator_close(it);
    }
    err
}

fn zip_step(realm: &mut Realm, id: ObjectId) -> JsResult<Option<Value>> {
    let (n, mode) = {
        let d = slots(realm, id);
        (d.list.len() / 4, d.num[1])
    };
    if n == 0 {
        return Ok(None);
    }
    let mut results = Vec::with_capacity(n);
    for i in 0..n {
        let (it, next, pad, open) = {
            let d = slots(realm, id);
            (d.list[i * 4].clone(), d.list[i * 4 + 1].clone(), d.list[i * 4 + 2].clone(), d.list[i * 4 + 3].truthy())
        };
        if !open {
            results.push(pad);
            continue;
        }
        match realm.iterator_step(&it, &next) {
            Err(e) => {
                slots_mut(realm, id).list[i * 4 + 3] = Value::Bool(false);
                return zip_close_all(realm, id, Err(e)).map(|_| None);
            }
            Ok(Some(v)) => results.push(v),
            Ok(None) => {
                slots_mut(realm, id).list[i * 4 + 3] = Value::Bool(false);
                if mode == Z_SHORTEST {
                    zip_close_all(realm, id, Ok(()))?;
                    return Ok(None);
                } else if mode == Z_STRICT {
                    if i != 0 {
                        let e = realm.throw_type_error("Iterator.zip: iterators have different lengths (strict mode)");
                        return zip_close_all(realm, id, Err(e)).map(|_| None);
                    }
                    for k in 1..n {
                        let (itk, nextk) = {
                            let d = slots(realm, id);
                            (d.list[k * 4].clone(), d.list[k * 4 + 1].clone())
                        };
                        match iterator_step_result(realm, &itk, &nextk) {
                            Err(e) => {
                                slots_mut(realm, id).list[k * 4 + 3] = Value::Bool(false);
                                return zip_close_all(realm, id, Err(e)).map(|_| None);
                            }
                            Ok(None) => slots_mut(realm, id).list[k * 4 + 3] = Value::Bool(false),
                            Ok(Some(_)) => {
                                let e = realm.throw_type_error("Iterator.zip: iterators have different lengths (strict mode)");
                                return zip_close_all(realm, id, Err(e)).map(|_| None);
                            }
                        }
                    }
                    return Ok(None);
                } else {
                    let any_open = (0..n).any(|j| slots(realm, id).list[j * 4 + 3].truthy());
                    if !any_open {
                        return Ok(None);
                    }
                    results.push(pad);
                }
            }
        }
    }
    let keys = slots(realm, id).values[S_FN].clone();
    match keys.v() {
        V::Object(k) => {
            let keys = match &realm.obj(k).kind {
                ObjectKind::Array { elements, .. } => elements.clone(),
                _ => Vec::new(),
            };
            let o = realm.new_object_with_proto(None);
            for (key, v) in keys.into_iter().zip(results) {
                let pk = realm.to_property_key(key)?;
                realm.define_property(o, pk, v, ALL);
            }
            Ok(Some(Value::Object(o)))
        }
        _ => Ok(Some(realm.new_array_value(results))),
    }
}

/// Shared options handling: (mode, padding option).
fn zip_options(realm: &mut Realm, options: Value) -> JsResult<(f64, Value)> {
    let options = match options {
        Value::Undefined => return Ok((Z_SHORTEST, Value::Undefined)),
        o if o.is_object() => o,
        _ => return realm.type_error("options must be an object"),
    };
    let mode = realm.get_str(options.clone(), "mode")?;
    let mode = match mode.v() {
        V::Undefined => Z_SHORTEST,
        V::String(s) if s.eq_str("shortest") => Z_SHORTEST,
        V::String(s) if s.eq_str("longest") => Z_LONGEST,
        V::String(s) if s.eq_str("strict") => Z_STRICT,
        _ => return realm.type_error("mode must be \"shortest\", \"longest\" or \"strict\""),
    };
    let mut padding = Value::Undefined;
    if mode == Z_LONGEST {
        padding = realm.get_str(options, "padding")?;
        if !padding.is_undefined() && !padding.is_object() {
            return realm.type_error("padding must be an object");
        }
    }
    Ok((mode, padding))
}

fn new_zip(realm: &mut Realm, iters: Vec<(Value, Value)>, padding: Vec<Value>, mode: f64, keys: Value) -> Value {
    let mut list = Vec::with_capacity(iters.len() * 4);
    for ((it, next), pad) in iters.into_iter().zip(padding) {
        list.push(it);
        list.push(next);
        list.push(pad);
        list.push(Value::Bool(true));
    }
    let h = new_helper(realm, K_ZIP, Value::Undefined, Value::Undefined, keys, mode);
    if let V::Object(id) = h.v() {
        slots_mut(realm, id).list = list;
    }
    h
}

fn iterator_zip(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let iterables = arg(args, 0);
    if !iterables.is_object() {
        return realm.type_error("Iterator.zip requires an iterable object");
    }
    let (mode, padding_opt) = zip_options(realm, arg(args, 1))?;
    let (input, input_next) = realm.get_iterator(iterables)?;
    let mut iters: Vec<(Value, Value)> = Vec::new();
    loop {
        let next = match realm.iterator_step(&input, &input_next) {
            Ok(Some(v)) => v,
            Ok(None) => break,
            Err(e) => return Err(close_all_list(realm, &iters, e)),
        };
        match get_iterator_flattenable(realm, next, false) {
            Ok(r) => iters.push(r),
            Err(e) => {
                let e = close_all_list(realm, &iters, e);
                let _ = realm.iterator_close(&input);
                return Err(e);
            }
        }
    }
    let n = iters.len();
    let mut padding = Vec::with_capacity(n);
    if mode == Z_LONGEST && !padding_opt.is_undefined() {
        let (pit, pnext) = match realm.get_iterator(padding_opt) {
            Ok(r) => r,
            Err(e) => return Err(close_all_list(realm, &iters, e)),
        };
        let mut using_iterator = true;
        for _ in 0..n {
            if using_iterator {
                match realm.iterator_step(&pit, &pnext) {
                    Ok(Some(v)) => {
                        padding.push(v);
                        continue;
                    }
                    Ok(None) => using_iterator = false,
                    Err(e) => return Err(close_all_list(realm, &iters, e)),
                }
            }
            padding.push(Value::Undefined);
        }
        if using_iterator {
            if let Err(e) = realm.iterator_close(&pit) {
                return Err(close_all_list(realm, &iters, e));
            }
        }
    } else {
        padding.resize(n, Value::Undefined);
    }
    Ok(new_zip(realm, iters, padding, mode, Value::Undefined))
}

fn iterator_zip_keyed(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let iterables = arg(args, 0);
    let src = match iterables.v() {
        V::Object(id) => id,
        _ => return realm.type_error("Iterator.zipKeyed requires an object"),
    };
    let (mode, padding_opt) = zip_options(realm, arg(args, 1))?;
    let mut iters: Vec<(Value, Value)> = Vec::new();
    let mut keys: Vec<Value> = Vec::new();
    let all_keys = realm.own_keys(src)?;
    for key in all_keys {
        let desc = match realm.get_own_property_any(src, &key) {
            Ok(d) => d,
            Err(e) => return Err(close_all_list(realm, &iters, e)),
        };
        if !matches!(desc, Some(ref p) if p.enumerable()) {
            continue;
        }
        let value = match realm.get(iterables.clone(), key.clone()) {
            Ok(v) => v,
            Err(e) => return Err(close_all_list(realm, &iters, e)),
        };
        if value.is_undefined() {
            continue;
        }
        match get_iterator_flattenable(realm, value, false) {
            Ok(r) => iters.push(r),
            Err(e) => return Err(close_all_list(realm, &iters, e)),
        }
        keys.push(match key {
            PropertyKey::String(s) => Value::String(s),
            PropertyKey::Symbol(s) => Value::Symbol(s),
        });
    }
    let mut padding = Vec::with_capacity(keys.len());
    if mode == Z_LONGEST && !padding_opt.is_undefined() {
        for key in &keys {
            let pk = realm.to_property_key(key.clone())?;
            match realm.get(padding_opt.clone(), pk) {
                Ok(v) => padding.push(v),
                Err(e) => return Err(close_all_list(realm, &iters, e)),
            }
        }
    } else {
        padding.resize(keys.len(), Value::Undefined);
    }
    let keys_arr = realm.new_array_value(keys);
    Ok(new_zip(realm, iters, padding, mode, keys_arr))
}
