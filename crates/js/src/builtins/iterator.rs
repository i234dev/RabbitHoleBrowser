//! Iterator prototypes, generators and async generators.

use super::*;
use crate::value::V;
use crate::realm::{SYM_ASYNC_ITERATOR, SYM_ITERATOR, SYM_TO_STRING_TAG};
use crate::vm::{ResumeMode, Step};

pub fn register(realm: &mut Realm) {
    let ip = realm.intrinsics.iterator_proto;
    realm.define_symbol_method(ip, SYM_ITERATOR, "[Symbol.iterator]", 0, return_this);
    // Array iterator
    let aip = realm.intrinsics.array_iterator_proto;
    realm.define_method(aip, "next", 0, array_iterator_next);
    realm.define_property(aip, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("Array Iterator"), CONFIGURABLE);
    // String iterator
    let sip = realm.new_object_with_proto(Some(ip));
    realm.define_method(sip, "next", 0, string_iterator_next);
    realm.define_property(sip, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("String Iterator"), CONFIGURABLE);
    realm.intrinsics.others.insert("StringIteratorPrototype", sip);
    // Map / Set iterators
    let mip = realm.new_object_with_proto(Some(ip));
    realm.define_method(mip, "next", 0, map_iterator_next);
    realm.define_property(mip, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("Map Iterator"), CONFIGURABLE);
    realm.intrinsics.others.insert("MapIteratorPrototype", mip);
    let setip = realm.new_object_with_proto(Some(ip));
    realm.define_method(setip, "next", 0, map_iterator_next);
    realm.define_property(setip, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("Set Iterator"), CONFIGURABLE);
    realm.intrinsics.others.insert("SetIteratorPrototype", setip);
    // Generators
    let gp = realm.intrinsics.generator_proto;
    realm.define_method(gp, "next", 1, generator_next);
    realm.define_method(gp, "return", 1, generator_return);
    realm.define_method(gp, "throw", 1, generator_throw);
    realm.define_property(gp, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("Generator"), CONFIGURABLE);
    // Async iterator prototype + async generators
    let aip2 = realm.new_object_with_proto(Some(realm.intrinsics.object_proto));
    realm.define_symbol_method(aip2, SYM_ASYNC_ITERATOR, "[Symbol.asyncIterator]", 0, return_this);
    realm.intrinsics.others.insert("AsyncIteratorPrototype", aip2);
    let agp = realm.intrinsics.async_generator_proto;
    realm.obj_mut(agp).set_proto(Some(aip2));
    realm.define_method(agp, "next", 1, async_generator_next);
    realm.define_method(agp, "return", 1, async_generator_return);
    realm.define_method(agp, "throw", 1, async_generator_throw);
    realm.define_property(agp, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("AsyncGenerator"), CONFIGURABLE);
    // Async-from-sync iterator prototype
    let afs = realm.new_object_with_proto(Some(aip2));
    realm.define_method(afs, "next", 1, afs_next);
    realm.define_method(afs, "return", 1, afs_return);
    realm.define_method(afs, "throw", 1, afs_throw);
    realm.intrinsics.others.insert("AsyncFromSyncIteratorPrototype", afs);
}

fn return_this(_realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(this)
}

/// kind: 0 keys, 1 values, 2 entries
pub fn create_array_iterator(realm: &mut Realm, target: Value, kind: u8) -> Value {
    let proto = realm.intrinsics.array_iterator_proto;
    Value::Object(realm.alloc(Object::new(Some(proto), ObjectKind::ArrayIterator { target, index: 0, kind })))
}

fn iter_result(realm: &mut Realm, step: Option<Value>) -> Value {
    match step {
        Some(v) => realm.create_iter_result(v, false),
        None => realm.create_iter_result(Value::Undefined, true),
    }
}

/// One step of an iterator object whose `next` is a built-in one, without
/// creating the result object (`None`: not such an iterator). Calling the
/// built-in `next` and reading `done` / `value` from its fresh result is
/// not observably different.
pub fn fast_step(realm: &mut Realm, it: &Value, next: &Value) -> Option<JsResult<Option<Value>>> {
    let (V::Object(nid), V::Object(iid)) = (next.v(), it.v()) else { return None };
    let f = match &realm.heap.try_get(nid)?.kind {
        ObjectKind::Native(n) => n.func as usize,
        _ => return None,
    };
    let kind = &realm.heap.try_get(iid)?.kind;
    if f == array_iterator_next as NativeFn as usize && matches!(kind, ObjectKind::ArrayIterator { .. }) {
        return Some(array_iterator_step(realm, iid));
    }
    if f == generator_next as NativeFn as usize {
        if let ObjectKind::Generator(g) = kind {
            if !g.is_async && g.promise.is_none() {
                return Some(match realm.generator_resume_raw(iid, ResumeMode::Next, Value::Undefined) {
                    Ok(Step::Yield(v)) => Ok(Some(v)),
                    Ok(Step::Return(_)) => Ok(None),
                    Ok(Step::Await(_)) => realm.type_error("await in a non-async generator"),
                    Err(e) => Err(e),
                });
            }
        }
        return None;
    }
    if f == map_iterator_next as NativeFn as usize && matches!(kind, ObjectKind::MapIterator { .. }) {
        return Some(map_iterator_step(realm, iid));
    }
    if f == string_iterator_next as NativeFn as usize && matches!(kind, ObjectKind::StringIterator { .. }) {
        return Some(Ok(string_iterator_step(realm, iid)));
    }
    None
}

fn array_iterator_next(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_object(realm, &this, "Array Iterator.prototype.next")?;
    if !matches!(realm.obj(id).kind, ObjectKind::ArrayIterator { .. }) {
        return realm.type_error("next method called on incompatible receiver");
    }
    let step = array_iterator_step(realm, id)?;
    Ok(iter_result(realm, step))
}

fn array_iterator_step(realm: &mut Realm, id: ObjectId) -> JsResult<Option<Value>> {
    let (target, index, kind) = match &realm.obj(id).kind {
        ObjectKind::ArrayIterator { target, index, kind } => (target.clone(), *index, *kind),
        _ => return realm.type_error("next method called on incompatible receiver"),
    };
    if target.is_undefined() {
        return Ok(None);
    }
    let len = match target.v() {
        V::Object(t) => match &realm.obj(t).kind {
            ObjectKind::Array { length, .. } => *length as usize,
            ObjectKind::TypedArray(ta) => ta.length,
            _ => realm.length_of(target.clone())?,
        },
        _ => realm.length_of(target.clone())?,
    };
    if index as usize >= len {
        if let ObjectKind::ArrayIterator { target, .. } = &mut realm.obj_mut(id).kind {
            *target = Value::Undefined;
        }
        return Ok(None);
    }
    if let ObjectKind::ArrayIterator { index: i, .. } = &mut realm.obj_mut(id).kind {
        *i += 1;
    }
    // A present element of a dense array needs no property lookup.
    if kind == 1 {
        if let V::Object(t) = target.v() {
            if let ObjectKind::Array { elements, .. } = &realm.obj(t).kind {
                if let Some(v) = elements.get(index as usize) {
                    if !v.is_empty() {
                        return Ok(Some(v.clone()));
                    }
                }
            }
        }
    }
    let result = match kind {
        0 => Value::Number(index as f64),
        1 => realm.get(target, PropertyKey::index(index))?,
        _ => {
            let v = realm.get(target, PropertyKey::index(index))?;
            realm.new_array_value(vec![Value::Number(index as f64), v])
        }
    };
    Ok(Some(result))
}

pub fn create_string_iterator(realm: &mut Realm, s: JsStr) -> Value {
    let proto = realm.intrinsics.others["StringIteratorPrototype"];
    Value::Object(realm.alloc(Object::new(Some(proto), ObjectKind::StringIterator { string: s, index: 0 })))
}

fn string_iterator_next(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_object(realm, &this, "String Iterator.prototype.next")?;
    if !matches!(realm.obj(id).kind, ObjectKind::StringIterator { .. }) {
        return realm.type_error("next method called on incompatible receiver");
    }
    let step = string_iterator_step(realm, id);
    Ok(iter_result(realm, step))
}

fn string_iterator_step(realm: &mut Realm, id: ObjectId) -> Option<Value> {
    let (s, index) = match &realm.obj(id).kind {
        ObjectKind::StringIterator { string, index } => (string.clone(), *index),
        _ => return None,
    };
    let units = s.as_units();
    if index >= units.len() {
        return None;
    }
    let first = units[index];
    let len = if (0xD800..0xDC00).contains(&first) && index + 1 < units.len() && (0xDC00..0xE000).contains(&units[index + 1]) { 2 } else { 1 };
    if let ObjectKind::StringIterator { index: i, .. } = &mut realm.obj_mut(id).kind {
        *i += len;
    }
    Some(if len == 1 { Value::String(JsStr::from_unit(first)) } else { Value::String(s.slice(index, index + len)) })
}

/// kind: 0 keys, 1 values, 2 entries
pub fn create_map_iterator(realm: &mut Realm, target: ObjectId, kind: u8, is_set: bool) -> Value {
    let proto = realm.intrinsics.others[if is_set { "SetIteratorPrototype" } else { "MapIteratorPrototype" }];
    if let ObjectKind::Map(m) | ObjectKind::Set(m) | ObjectKind::WeakMap(m) | ObjectKind::WeakSet(m) = &mut realm.obj_mut(target).kind {
        m.iteration_started();
    }
    Value::Object(realm.alloc(Object::new(Some(proto), ObjectKind::MapIterator { target, index: 0, kind })))
}

fn map_iterator_next(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_object(realm, &this, "Map Iterator.prototype.next")?;
    let step = map_iterator_step(realm, id)?;
    Ok(iter_result(realm, step))
}

fn map_iterator_step(realm: &mut Realm, id: ObjectId) -> JsResult<Option<Value>> {
    let (target, mut index, kind) = match &realm.obj(id).kind {
        ObjectKind::MapIterator { target, index, kind } => (*target, *index, *kind),
        _ => return realm.type_error("next method called on incompatible receiver"),
    };
    if index == crate::heap::MAP_ITER_DONE {
        return Ok(None);
    }
    let entry = {
        let data = match &realm.obj(target).kind {
            ObjectKind::Map(m) | ObjectKind::Set(m) | ObjectKind::WeakMap(m) | ObjectKind::WeakSet(m) => m,
            _ => return realm.type_error("next method called on incompatible receiver"),
        };
        let mut found = None;
        while index < data.entries.len() {
            if let Some((k, v)) = &data.entries[index] {
                found = Some((k.clone(), v.clone()));
                index += 1;
                break;
            }
            index += 1;
        }
        found
    };
    let done = entry.is_none();
    if let ObjectKind::MapIterator { index: i, .. } = &mut realm.obj_mut(id).kind {
        *i = if done { crate::heap::MAP_ITER_DONE } else { index };
    }
    if done {
        if let ObjectKind::Map(m) | ObjectKind::Set(m) | ObjectKind::WeakMap(m) | ObjectKind::WeakSet(m) = &mut realm.obj_mut(target).kind {
            m.iteration_ended();
        }
    }
    Ok(entry.map(|(k, v)| match kind {
        0 => k,
        1 => v,
        _ => realm.new_array_value(vec![k, v]),
    }))
}

// ------------------------------------------------------------- generators

fn this_generator(realm: &mut Realm, this: &Value, is_async: bool) -> JsResult<ObjectId> {
    if let V::Object(id) = this.v() {
        if let ObjectKind::Generator(g) = &realm.obj(id).kind {
            if g.is_async == is_async && g.promise.is_none() {
                return Ok(id);
            }
        }
    }
    realm.type_error("next method called on incompatible receiver")
}

fn generator_next(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let g = this_generator(realm, &this, false)?;
    realm.generator_resume(g, ResumeMode::Next, arg(args, 0))
}

fn generator_return(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let g = this_generator(realm, &this, false)?;
    realm.generator_resume(g, ResumeMode::Return, arg(args, 0))
}

fn generator_throw(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let g = this_generator(realm, &this, false)?;
    realm.generator_resume(g, ResumeMode::Throw, arg(args, 0))
}

// ------------------------------------------------------- async generators

fn async_generator_enqueue(realm: &mut Realm, this: Value, kind: GenRequestKind, value: Value) -> JsResult<Value> {
    let (p, _res, rej) = realm.new_promise();
    let g = match this_generator(realm, &this, true) {
        Ok(g) => g,
        Err(e) => {
            realm.call(Value::Object(rej), Value::Undefined, &[e])?;
            return Ok(Value::Object(p));
        }
    };
    let executing = match &mut realm.obj_mut(g).kind {
        ObjectKind::Generator(gd) => {
            gd.queue.push_back((kind, value, p));
            gd.state == GeneratorState::Executing
        }
        _ => unreachable!(),
    };
    if !executing {
        async_gen_drive(realm, g, None);
    }
    Ok(Value::Object(p))
}

fn async_generator_next(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    async_generator_enqueue(realm, this, GenRequestKind::Next, arg(args, 0))
}
fn async_generator_return(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    async_generator_enqueue(realm, this, GenRequestKind::Return, arg(args, 0))
}
fn async_generator_throw(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    async_generator_enqueue(realm, this, GenRequestKind::Throw, arg(args, 0))
}

/// Continuation after an `await` inside an async generator.
pub(crate) fn async_generator_resume_step(realm: &mut Realm, gen: ObjectId, mode: ResumeMode, value: Value) {
    if let ObjectKind::Generator(gd) = &mut realm.obj_mut(gen).kind {
        gd.state = GeneratorState::SuspendedYield;
    }
    async_gen_drive(realm, gen, Some((mode, value)));
}

fn settle_request(realm: &mut Realm, p: ObjectId, result: Result<(Value, bool), Value>) {
    match result {
        Ok((v, done)) => {
            let r = realm.create_iter_result(v, done);
            realm.resolve_promise(p, r);
        }
        Err(e) => realm.reject_promise(p, e),
    }
}

/// Process queued requests until the generator awaits or the queue drains.
fn async_gen_drive(realm: &mut Realm, gen: ObjectId, mut resume: Option<(ResumeMode, Value)>) {
    loop {
        let (state, front) = match &realm.obj(gen).kind {
            ObjectKind::Generator(gd) => (gd.state, gd.queue.front().cloned()),
            _ => return,
        };
        let (kind, value, p) = match front {
            Some(f) => f,
            None => return,
        };
        if state == GeneratorState::Executing && resume.is_none() {
            return;
        }
        if state == GeneratorState::Completed {
            if let ObjectKind::Generator(gd) = &mut realm.obj_mut(gen).kind {
                gd.queue.pop_front();
            }
            match kind {
                GenRequestKind::Next => settle_request(realm, p, Ok((Value::Undefined, true))),
                GenRequestKind::Return => settle_request(realm, p, Ok((value, true))),
                GenRequestKind::Throw => settle_request(realm, p, Err(value)),
            }
            continue;
        }
        let (mode, v) = match resume.take() {
            Some(r) => r,
            None => (
                match kind {
                    GenRequestKind::Next => ResumeMode::Next,
                    GenRequestKind::Return => ResumeMode::Return,
                    GenRequestKind::Throw => ResumeMode::Throw,
                },
                value,
            ),
        };
        let step = realm.generator_resume_raw(gen, mode, v);
        match step {
            Ok(Step::Yield(v)) => {
                if let ObjectKind::Generator(gd) = &mut realm.obj_mut(gen).kind {
                    gd.queue.pop_front();
                }
                settle_request(realm, p, Ok((v, false)));
            }
            Ok(Step::Await(v)) => {
                if let ObjectKind::Generator(gd) = &mut realm.obj_mut(gen).kind {
                    gd.state = GeneratorState::Executing;
                }
                realm.await_value(gen, v);
                return;
            }
            Ok(Step::Return(v)) => {
                if let ObjectKind::Generator(gd) = &mut realm.obj_mut(gen).kind {
                    gd.queue.pop_front();
                    gd.state = GeneratorState::Completed;
                }
                settle_request(realm, p, Ok((v, true)));
            }
            Err(e) => {
                if let ObjectKind::Generator(gd) = &mut realm.obj_mut(gen).kind {
                    gd.queue.pop_front();
                    gd.state = GeneratorState::Completed;
                }
                settle_request(realm, p, Err(e));
            }
        }
    }
}

// ------------------------------------------------- async-from-sync iterator

pub fn create_async_from_sync_iterator(realm: &mut Realm, it: Value, next: Value) -> Value {
    let proto = realm.intrinsics.others["AsyncFromSyncIteratorPrototype"];
    let o = realm.new_object_with_proto(Some(proto));
    realm.define_property(o, "__sync", it, 0);
    realm.define_property(o, "__next", next, 0);
    Value::Object(o)
}

fn afs_parts(realm: &mut Realm, this: &Value) -> JsResult<(Value, Value)> {
    let id = this_object(realm, this, "AsyncFromSyncIterator")?;
    let it = realm.get_str(Value::Object(id), "__sync")?;
    let next = realm.get_str(Value::Object(id), "__next")?;
    Ok((it, next))
}

/// Wrap a sync iterator result: resolve `value` then produce {value, done}.
fn afs_continue(realm: &mut Realm, result: Value) -> JsResult<Value> {
    let (p, res, rej) = realm.new_promise();
    let done = realm.get_str(result.clone(), "done")?.truthy();
    let value = realm.get_str(result, "value")?;
    let vp = match crate::builtins::promise::promise_resolve(realm, value) {
        Ok(vp) => vp,
        Err(e) => {
            realm.call(Value::Object(rej), Value::Undefined, &[e])?;
            return Ok(Value::Object(p));
        }
    };
    let on_ok = realm.new_native_function_full("", 1, afs_unwrap, None, vec![Value::Object(res), Value::Bool(done)], false);
    realm.invoke(Value::Object(vp), "then", &[Value::Object(on_ok), Value::Object(rej)])?;
    Ok(Value::Object(p))
}

fn afs_unwrap(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let caps = realm.current_native().map(|n| realm.native_captures(n)).unwrap_or_default();
    let res = caps.first().cloned().unwrap_or(Value::Undefined);
    let done = caps.get(1).map(|v| v.truthy()).unwrap_or(true);
    let r = realm.create_iter_result(arg(args, 0), done);
    realm.call(res, Value::Undefined, &[r])
}

fn afs_next(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (it, next) = afs_parts(realm, &this)?;
    match realm.call(next, it, &[arg(args, 0)]) {
        Ok(r) => afs_continue(realm, r),
        Err(e) => {
            let (p, _res, rej) = realm.new_promise();
            realm.call(Value::Object(rej), Value::Undefined, &[e])?;
            Ok(Value::Object(p))
        }
    }
}

fn afs_return(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (it, _next) = afs_parts(realm, &this)?;
    let ret = realm.get_method(it.clone(), PropertyKey::str("return"))?;
    match ret {
        None => {
            let (p, res, _rej) = realm.new_promise();
            let r = realm.create_iter_result(arg(args, 0), true);
            realm.call(Value::Object(res), Value::Undefined, &[r])?;
            Ok(Value::Object(p))
        }
        Some(f) => match realm.call(f, it, &[arg(args, 0)]) {
            Ok(r) => afs_continue(realm, r),
            Err(e) => {
                let (p, _res, rej) = realm.new_promise();
                realm.call(Value::Object(rej), Value::Undefined, &[e])?;
                Ok(Value::Object(p))
            }
        },
    }
}

fn afs_throw(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (it, _next) = afs_parts(realm, &this)?;
    let th = realm.get_method(it.clone(), PropertyKey::str("throw"))?;
    match th {
        None => {
            let (p, _res, rej) = realm.new_promise();
            realm.call(Value::Object(rej), Value::Undefined, &[arg(args, 0)])?;
            Ok(Value::Object(p))
        }
        Some(f) => match realm.call(f, it, &[arg(args, 0)]) {
            Ok(r) => afs_continue(realm, r),
            Err(e) => {
                let (p, _res, rej) = realm.new_promise();
                realm.call(Value::Object(rej), Value::Undefined, &[e])?;
                Ok(Value::Object(p))
            }
        },
    }
}
