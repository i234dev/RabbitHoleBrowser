//! Promises and the microtask machinery.

use super::*;
use crate::value::V;
use crate::realm::{Job, SYM_SPECIES, SYM_TO_STRING_TAG};

pub fn register(realm: &mut Realm) {
    let pp = realm.intrinsics.promise_proto;
    realm.define_method(pp, "then", 2, then);
    realm.define_method(pp, "catch", 1, catch);
    realm.define_method(pp, "finally", 1, finally);
    realm.define_property(pp, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("Promise"), CONFIGURABLE);
    let ctor = define_ctor(realm, "Promise", 1, promise_ctor, pp);
    realm.intrinsics.promise_ctor = ctor;
    realm.define_method(ctor, "resolve", 1, promise_resolve_static);
    realm.define_method(ctor, "reject", 1, promise_reject_static);
    realm.define_method(ctor, "all", 1, all);
    realm.define_method(ctor, "allSettled", 1, all_settled);
    realm.define_method(ctor, "race", 1, race);
    realm.define_method(ctor, "any", 1, any);
    realm.define_method(ctor, "withResolvers", 0, with_resolvers);
    realm.define_accessor(ctor, PropertyKey::Symbol(SYM_SPECIES), Some(species), None, CONFIGURABLE);
    if let Some(g) = realm.obj(ctor).props.get(&PropertyKey::Symbol(SYM_SPECIES)).and_then(|p| p.getter().and_then(|g| g.as_object())) {
        realm.intrinsics.others.insert("PromiseSpeciesGetter", g);
    }
}

/// SpeciesConstructor(promise, %Promise%), answered without lookups for a
/// plain promise whose constructor and species are the intrinsic ones.
fn promise_species(realm: &mut Realm, p: ObjectId) -> JsResult<Value> {
    let pc = realm.intrinsics.promise_ctor;
    let pp = realm.intrinsics.promise_proto;
    let ctor_key = crate::value::names::constructor();
    let plain = {
        let o = realm.obj(p);
        o.proto == Some(pp)
            && !o.props.contains(&ctor_key)
            && matches!(realm.obj(pp).props.get(&ctor_key), Some(d) if !d.is_accessor() && d.value.as_object() == Some(pc))
            && match (realm.obj(pc).props.get(&PropertyKey::Symbol(SYM_SPECIES)), realm.intrinsics.others.get("PromiseSpeciesGetter")) {
                (Some(d), Some(g)) => d.is_accessor() && d.getter().and_then(|v| v.as_object()) == Some(*g),
                _ => false,
            }
    };
    if plain {
        return Ok(Value::Object(pc));
    }
    realm.species_constructor(p, pc)
}

/// PromiseResolve(C, x).
fn promise_resolve_with(realm: &mut Realm, c: Value, v: Value) -> JsResult<Value> {
    if c.as_object() == Some(realm.intrinsics.promise_ctor) {
        return Ok(Value::Object(promise_resolve(realm, v)?));
    }
    if is_promise(realm, &v) {
        let ctor = realm.get_str(v.clone(), "constructor")?;
        if realm.same_value(&ctor, &c) {
            return Ok(v);
        }
    }
    let (p, res, _rej) = new_capability_with_ctor(realm, c)?;
    realm.call(res, Value::Undefined, &[v])?;
    Ok(p)
}

fn species(_realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(this)
}

fn is_promise(realm: &Realm, v: &Value) -> bool {
    matches!(v.v(), V::Object(id) if matches!(realm.obj(id).kind, ObjectKind::Promise(_)))
}

/// Create a promise and its resolving functions.
pub fn new_promise_capability(realm: &mut Realm) -> (ObjectId, ObjectId, ObjectId) {
    let p = realm.new_promise_object();
    let (res, rej) = create_resolving_functions(realm, p);
    (p, res, rej)
}

fn create_resolving_functions(realm: &mut Realm, p: ObjectId) -> (ObjectId, ObjectId) {
    // Shared "already resolved" flag lives in the captures as a boolean cell object.
    let flag = realm.new_object_with_proto(None);
    realm.define_property(flag, "done", Value::Bool(false), ALL);
    let res = realm.new_native_function_full("", 1, resolve_fn, None, vec![Value::Object(p), Value::Object(flag)], false);
    let rej = realm.new_native_function_full("", 1, reject_fn, None, vec![Value::Object(p), Value::Object(flag)], false);
    (res, rej)
}

fn resolving_target(realm: &mut Realm) -> Option<ObjectId> {
    let f = realm.current_native()?;
    let caps = realm.native_captures(f);
    let p = caps.first()?.as_object()?;
    let flag = caps.get(1)?.as_object()?;
    let done = realm.obj(flag).props.get(&PropertyKey::str("done")).map(|p| p.value.truthy()).unwrap_or(false);
    if done {
        return None;
    }
    realm.define_property(flag, "done", Value::Bool(true), ALL);
    Some(p)
}

fn resolve_fn(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if let Some(p) = resolving_target(realm) {
        resolve_promise(realm, p, arg(args, 0));
    }
    Ok(Value::Undefined)
}

fn reject_fn(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if let Some(p) = resolving_target(realm) {
        reject_promise(realm, p, arg(args, 0));
    }
    Ok(Value::Undefined)
}

/// The promise resolve function logic (handles thenables).
pub fn resolve_promise(realm: &mut Realm, promise: ObjectId, resolution: Value) {
    if let ObjectKind::Promise(pd) = &realm.obj(promise).kind {
        if pd.state != PromiseState::Pending {
            return;
        }
    } else {
        return;
    }
    if resolution.as_object() == Some(promise) {
        let e = realm.new_error("TypeError", "Chaining cycle detected for promise");
        reject_promise(realm, promise, e);
        return;
    }
    if let V::Object(_) = resolution.v() {
        let then = match realm.get_str(resolution.clone(), "then") {
            Ok(t) => t,
            Err(e) => {
                reject_promise(realm, promise, e);
                return;
            }
        };
        if realm.is_callable(&then) {
            realm.jobs.push_back(Job::ResolveThenable { promise, thenable: resolution, then });
            return;
        }
    }
    fulfill_promise(realm, promise, resolution);
}

fn fulfill_promise(realm: &mut Realm, promise: ObjectId, value: Value) {
    let reactions = match &mut realm.obj_mut(promise).kind {
        ObjectKind::Promise(pd) => {
            pd.state = PromiseState::Fulfilled;
            pd.value = value.clone();
            pd.reject_reactions.clear();
            std::mem::take(&mut pd.fulfill_reactions)
        }
        _ => return,
    };
    for r in reactions {
        realm.jobs.push_back(Job::Reaction { reaction: r, argument: value.clone() });
    }
}

pub fn reject_promise(realm: &mut Realm, promise: ObjectId, reason: Value) {
    let (reactions, handled) = match &mut realm.obj_mut(promise).kind {
        ObjectKind::Promise(pd) => {
            if pd.state != PromiseState::Pending {
                return;
            }
            pd.state = PromiseState::Rejected;
            pd.value = reason.clone();
            pd.fulfill_reactions.clear();
            (std::mem::take(&mut pd.reject_reactions), pd.handled)
        }
        _ => return,
    };
    if !handled && reactions.is_empty() {
        realm.unhandled_rejections.push(reason.clone());
    }
    for r in reactions {
        realm.jobs.push_back(Job::Reaction { reaction: r, argument: reason.clone() });
    }
}

/// PromiseResolve(%Promise%, v): returns a promise object id.
pub fn promise_resolve(realm: &mut Realm, v: Value) -> JsResult<ObjectId> {
    if let V::Object(id) = v.v() {
        if let ObjectKind::Promise(_) = &realm.obj(id).kind {
            // Only when its constructor is %Promise%.
            let ctor = realm.get_str(v.clone(), "constructor")?;
            if ctor.as_object() == Some(realm.intrinsics.promise_ctor) {
                return Ok(id);
            }
        }
    }
    let (p, res, _rej) = new_promise_capability(realm);
    realm.call(Value::Object(res), Value::Undefined, &[v])?;
    Ok(p)
}

/// Attach reactions; `capability` is the derived promise (if any).
fn perform_then(realm: &mut Realm, promise: ObjectId, on_fulfilled: Option<Value>, on_rejected: Option<Value>, capability: Option<(Value, Value, Value)>) {
    let fr = PromiseReaction { handler: on_fulfilled, capability: capability.clone(), native: None, is_reject: false };
    let rr = PromiseReaction { handler: on_rejected, capability, native: None, is_reject: true };
    attach_reactions(realm, promise, fr, rr);
}

fn attach_reactions(realm: &mut Realm, promise: ObjectId, fr: PromiseReaction, rr: PromiseReaction) {
    // HostPromiseRejectionTracker "handle": a handler attached to an
    // already-rejected, unhandled promise takes it off the unhandled list.
    let late = match &realm.obj(promise).kind {
        ObjectKind::Promise(pd) if pd.state == PromiseState::Rejected && !pd.handled => Some(pd.value.clone()),
        _ => None,
    };
    if let Some(reason) = late {
        let pos = realm.unhandled_rejections.iter().rposition(|v| match (v.v(), reason.v()) {
            (V::Object(a), V::Object(b)) => a == b,
            (_, b) => !b.is_object() && realm.strict_equals(v, &reason),
        });
        if let Some(pos) = pos {
            realm.unhandled_rejections.remove(pos);
        }
    }
    let (state, value) = match &mut realm.obj_mut(promise).kind {
        ObjectKind::Promise(pd) => {
            pd.handled = true;
            match pd.state {
                PromiseState::Pending => {
                    pd.fulfill_reactions.push(fr);
                    pd.reject_reactions.push(rr);
                    return;
                }
                s => (s, pd.value.clone()),
            }
        }
        _ => return,
    };
    let reaction = if state == PromiseState::Fulfilled { fr } else { rr };
    realm.jobs.push_back(Job::Reaction { reaction, argument: value });
}

/// Native-continuation `then` used by `await`: `on_ok(value)` / `on_err(reason)`
/// are called with `data` as the first capture of a temporary native function.
pub fn perform_then_native(realm: &mut Realm, promise: ObjectId, on_ok: NativeFn, on_err: NativeFn, data: Value) {
    let fr = PromiseReaction { handler: None, capability: None, native: Some((on_ok, data.clone())), is_reject: false };
    let rr = PromiseReaction { handler: None, capability: None, native: Some((on_err, data)), is_reject: true };
    attach_reactions(realm, promise, fr, rr);
}

impl Realm {
    pub(crate) fn run_reaction(&mut self, reaction: PromiseReaction, argument: Value) -> JsResult<()> {
        if let Some((f, data)) = reaction.native {
            // Call through a temporary native function so `current_native` captures work.
            let nf = self.new_native_function_full("", 1, f, None, vec![data], false);
            self.call(Value::Object(nf), Value::Undefined, &[argument])?;
            return Ok(());
        }
        let is_reject = reaction.is_reject;
        let result = match &reaction.handler {
            Some(h) => self.call(h.clone(), Value::Undefined, &[argument.clone()]),
            None => {
                if is_reject {
                    Err(argument.clone())
                } else {
                    Ok(argument.clone())
                }
            }
        };
        if let Some((_p, res, rej)) = reaction.capability {
            match result {
                Ok(v) => {
                    self.call(res, Value::Undefined, &[v])?;
                }
                Err(e) => {
                    self.call(rej, Value::Undefined, &[e])?;
                }
            }
        } else if let Err(e) = result {
            return Err(e);
        }
        Ok(())
    }

    pub(crate) fn run_resolve_thenable(&mut self, promise: ObjectId, thenable: Value, then: Value) -> JsResult<()> {
        let (res, rej) = create_resolving_functions(self, promise);
        if let Err(e) = self.call(then, thenable, &[Value::Object(res), Value::Object(rej)]) {
            self.call(Value::Object(rej), Value::Undefined, &[e])?;
        }
        Ok(())
    }
}

fn promise_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if !realm.is_construct_call() {
        return realm.type_error("Promise constructor cannot be invoked without 'new'");
    }
    let executor = arg(args, 0);
    if !realm.is_callable(&executor) {
        return realm.type_error("Promise resolver is not a function");
    }
    let default_proto = realm.intrinsics.promise_proto;
    let proto = realm.proto_from_new_target(default_proto);
    let p = realm.alloc(Object::new(Some(proto), ObjectKind::Promise(Box::new(PromiseData { state: PromiseState::Pending, value: Value::Undefined, fulfill_reactions: vec![], reject_reactions: vec![], handled: false }))));
    let (res, rej) = create_resolving_functions(realm, p);
    if let Err(e) = realm.call(executor, Value::Undefined, &[Value::Object(res), Value::Object(rej)]) {
        realm.call(Value::Object(rej), Value::Undefined, &[e])?;
    }
    Ok(Value::Object(p))
}

fn then(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let p = match this.v() {
        V::Object(id) if is_promise(realm, &this) => id,
        _ => return realm.type_error("Promise.prototype.then called on incompatible receiver"),
    };
    let on_ok = arg(args, 0);
    let on_err = arg(args, 1);
    let c = promise_species(realm, p)?;
    let cap = new_capability_with_ctor(realm, c)?;
    let np = cap.0.clone();
    let on_ok = if realm.is_callable(&on_ok) { Some(on_ok) } else { None };
    let on_err = if realm.is_callable(&on_err) { Some(on_err) } else { None };
    perform_then(realm, p, on_ok, on_err, Some(cap));
    Ok(np)
}

fn catch(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    realm.invoke(this, "then", &[Value::Undefined, arg(args, 0)])
}

fn finally(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(pid) = this.as_object() else { return realm.type_error("Promise.prototype.finally called on a non-object") };
    let c = if is_promise(realm, &this) { promise_species(realm, pid)? } else { realm.species_constructor(pid, realm.intrinsics.promise_ctor)? };
    let on_finally = arg(args, 0);
    if !realm.is_callable(&on_finally) {
        return realm.invoke(this, "then", &[on_finally.clone(), on_finally]);
    }
    let then_finally = realm.new_native_function_full("", 1, finally_fulfilled, None, vec![on_finally.clone(), c.clone()], false);
    let catch_finally = realm.new_native_function_full("", 1, finally_rejected, None, vec![on_finally, c], false);
    realm.invoke(this, "then", &[Value::Object(then_finally), Value::Object(catch_finally)])
}

/// The onFinally callback and the constructor captured by finally's closures.
fn finally_captures(realm: &mut Realm) -> (Value, Value) {
    let caps = realm.current_native().map(|n| realm.native_captures(n).to_vec()).unwrap_or_default();
    let f = caps.first().cloned().unwrap_or(Value::Undefined);
    let c = caps.get(1).cloned().unwrap_or(Value::Object(realm.intrinsics.promise_ctor));
    (f, c)
}

fn finally_fulfilled(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let (f, c) = finally_captures(realm);
    let value = arg(args, 0);
    let r = realm.call(f, Value::Undefined, &[])?;
    let p = promise_resolve_with(realm, c, r)?;
    let value_thunk = realm.new_native_function_full("", 0, return_capture, None, vec![value], false);
    realm.invoke(p, "then", &[Value::Object(value_thunk)])
}

fn finally_rejected(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let (f, c) = finally_captures(realm);
    let reason = arg(args, 0);
    let r = realm.call(f, Value::Undefined, &[])?;
    let p = promise_resolve_with(realm, c, r)?;
    let thrower = realm.new_native_function_full("", 0, throw_capture, None, vec![reason], false);
    realm.invoke(p, "then", &[Value::Object(thrower)])
}

fn return_capture(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(realm.current_native().and_then(|n| realm.native_captures(n).first().cloned()).unwrap_or(Value::Undefined))
}

fn throw_capture(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    Err(realm.current_native().and_then(|n| realm.native_captures(n).first().cloned()).unwrap_or(Value::Undefined))
}

fn promise_resolve_static(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let v = arg(args, 0);
    if !this.is_object() {
        return realm.type_error("Promise.resolve called on non-object");
    }
    if this.as_object() == Some(realm.intrinsics.promise_ctor) {
        return Ok(Value::Object(promise_resolve(realm, v)?));
    }
    if let Some(id) = v.as_object() {
        if matches!(realm.obj(id).kind, ObjectKind::Promise(_)) {
            let ctor = realm.get_str(v.clone(), "constructor")?;
            if realm.same_value(&ctor, &this) {
                return Ok(v);
            }
        }
    }
    // Subclass: new C(resolve => resolve(v))
    let (p, res, _rej) = new_capability_with_ctor(realm, this)?;
    realm.call(res, Value::Undefined, &[v])?;
    Ok(p)
}

fn promise_reject_static(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (p, _res, rej) = new_capability_with_ctor(realm, this)?;
    realm.call(rej, Value::Undefined, &[arg(args, 0)])?;
    Ok(p)
}

/// NewPromiseCapability(C) for arbitrary constructors.
pub(crate) fn new_capability_with_ctor(realm: &mut Realm, c: Value) -> JsResult<(Value, Value, Value)> {
    if c.as_object() == Some(realm.intrinsics.promise_ctor) {
        let (p, res, rej) = new_promise_capability(realm);
        return Ok((Value::Object(p), Value::Object(res), Value::Object(rej)));
    }
    if !realm.is_constructor(&c) {
        return realm.type_error("Promise capability: not a constructor");
    }
    let holder = realm.new_object_with_proto(None);
    let executor = realm.new_native_function_full("", 2, capability_executor, None, vec![Value::Object(holder)], false);
    let p = realm.construct(c, &[Value::Object(executor)], Value::Undefined)?;
    let p = match p.v() {
        V::Object(_) => p,
        _ => return realm.type_error("Promise constructor returned non-object"),
    };
    let res = realm.get_str(Value::Object(holder), "resolve")?;
    let rej = realm.get_str(Value::Object(holder), "reject")?;
    if !realm.is_callable(&res) || !realm.is_callable(&rej) {
        return realm.type_error("Promise resolve or reject function is not callable");
    }
    Ok((p, res, rej))
}

fn capability_executor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let holder = realm.current_native().and_then(|n| realm.native_captures(n).first().cloned()).and_then(|v| v.as_object());
    if let Some(h) = holder {
        let set = |realm: &Realm, k: &str| realm.obj(h).props.get(&PropertyKey::str(k)).map(|p| !p.value.is_undefined()).unwrap_or(false);
        if set(realm, "resolve") || set(realm, "reject") {
            return realm.type_error("Promise executor has already been invoked with non-undefined arguments");
        }
        realm.define_property(h, "resolve", arg(args, 0), ALL);
        realm.define_property(h, "reject", arg(args, 1), ALL);
    }
    Ok(Value::Undefined)
}

fn with_resolvers(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (p, res, rej) = new_capability_with_ctor(realm, this)?;
    let o = realm.new_object();
    realm.define_property(o, "promise", p, ALL);
    realm.define_property(o, "resolve", res, ALL);
    realm.define_property(o, "reject", rej, ALL);
    Ok(Value::Object(o))
}

// ------------------------------------------------------- combinators
//
// Promise.all / allSettled / any / race (ECMA-262 §27.2.4): the iterable
// is stepped one value at a time; an error from the loop body closes the
// iterator (unless the iterator itself failed) and rejects the result.

#[derive(Clone, Copy, PartialEq, Eq)]
enum Combinator {
    All,
    AllSettled,
    Any,
    Race,
}

/// Elements of a hidden array object (the List of values / errors).
fn list_push(realm: &mut Realm, list: ObjectId, v: Value) {
    if let ObjectKind::Array { elements, length } = &mut realm.obj_mut(list).kind {
        elements.push(v);
        *length = elements.len() as u32;
    }
}

fn list_set(realm: &mut Realm, list: ObjectId, i: usize, v: Value) {
    if let ObjectKind::Array { elements, .. } = &mut realm.obj_mut(list).kind {
        if let Some(slot) = elements.get_mut(i) {
            *slot = v;
        }
    }
}

/// CreateArrayFromList(list).
fn list_to_array(realm: &mut Realm, list: ObjectId) -> Value {
    let items = match &realm.obj(list).kind {
        ObjectKind::Array { elements, .. } => elements.clone(),
        _ => Vec::new(),
    };
    realm.new_array_value(items)
}

/// State shared by the element functions of one combinator call:
/// slot 0 `values`/`errors` list, slot 1 remaining count, slot 2 resolve
/// (or reject for `any`).
fn new_state(realm: &mut Realm, settle: Value) -> ObjectId {
    let list = realm.new_array(Vec::new());
    let state = realm.new_object_with_proto(None);
    realm.define_property(state, "list", Value::Object(list), ALL);
    realm.define_property(state, "remaining", Value::Number(1.0), ALL);
    realm.define_property(state, "settle", settle, ALL);
    state
}

fn state_list(realm: &Realm, state: ObjectId) -> ObjectId {
    realm.obj(state).props.get(&PropertyKey::str("list")).and_then(|p| p.value.as_object()).expect("combinator list")
}

fn state_bump(realm: &mut Realm, state: ObjectId, delta: f64) -> f64 {
    let n = realm.obj(state).props.get(&PropertyKey::str("remaining")).and_then(|p| p.value.as_number()).unwrap_or(0.0) + delta;
    realm.define_property(state, "remaining", Value::Number(n), ALL);
    n
}

fn state_settle(realm: &Realm, state: ObjectId) -> Value {
    realm.obj(state).props.get(&PropertyKey::str("settle")).map(|p| p.value.clone()).unwrap_or(Value::Undefined)
}

/// Finish `all`/`allSettled` (resolve with the values) or `any` (reject
/// with an AggregateError) once nothing remains.
fn state_finish(realm: &mut Realm, state: ObjectId, kind: Combinator) -> JsResult<Value> {
    let list = state_list(realm, state);
    let arr = list_to_array(realm, list);
    let settle = state_settle(realm, state);
    let v = if kind == Combinator::Any {
        let e = realm.new_error("AggregateError", "All promises were rejected");
        if let Some(eid) = e.as_object() {
            realm.define_property(eid, "errors", arr, WRITABLE | CONFIGURABLE);
        }
        e
    } else {
        arr
    };
    realm.call(settle, Value::Undefined, &[v])
}

fn combinator(realm: &mut Realm, this: Value, args: &[Value], kind: Combinator) -> JsResult<Value> {
    let c = this;
    let (p, res, rej) = new_capability_with_ctor(realm, c.clone())?;
    // GetPromiseResolve(C)
    let promise_resolve = match realm.get_str(c.clone(), "resolve") {
        Ok(f) if realm.is_callable(&f) => f,
        Ok(_) => {
            let e = realm.throw_type_error("Promise resolve is not a function");
            realm.call(rej, Value::Undefined, &[e])?;
            return Ok(p);
        }
        Err(e) => {
            realm.call(rej, Value::Undefined, &[e])?;
            return Ok(p);
        }
    };
    let (it, next) = match realm.get_iterator(arg(args, 0)) {
        Ok(x) => x,
        Err(e) => {
            realm.call(rej, Value::Undefined, &[e])?;
            return Ok(p);
        }
    };
    let state = new_state(realm, if kind == Combinator::Any { rej.clone() } else { res.clone() });
    let mut index = 0usize;
    let mut iterator_done = false;
    let r: JsResult<()> = (|| loop {
        let step = realm.iterator_step(&it, &next);
        let value = match step {
            Err(e) => {
                iterator_done = true;
                return Err(e);
            }
            Ok(None) => {
                iterator_done = true;
                if kind != Combinator::Race && state_bump(realm, state, -1.0) == 0.0 {
                    state_finish(realm, state, kind)?;
                }
                return Ok(());
            }
            Ok(Some(v)) => v,
        };
        if kind != Combinator::Race {
            let list = state_list(realm, state);
            list_push(realm, list, Value::Undefined);
        }
        let next_promise = realm.call(promise_resolve.clone(), c.clone(), &[value])?;
        let mk = |realm: &mut Realm, f: NativeFn, called: ObjectId| -> Value {
            Value::Object(realm.new_native_function_full("", 1, f, None, vec![Value::Object(state), Value::Number(index as f64), Value::Object(called)], false))
        };
        let (on_ok, on_err) = match kind {
            Combinator::Race => (res.clone(), rej.clone()),
            Combinator::All => {
                let called = realm.new_object_with_proto(None);
                (mk(realm, all_element, called), rej.clone())
            }
            Combinator::AllSettled => {
                let called = realm.new_object_with_proto(None);
                (mk(realm, settled_fulfilled, called), mk(realm, settled_rejected, called))
            }
            Combinator::Any => {
                let called = realm.new_object_with_proto(None);
                (res.clone(), mk(realm, any_rejected, called))
            }
        };
        if kind != Combinator::Race {
            state_bump(realm, state, 1.0);
        }
        realm.invoke(next_promise, "then", &[on_ok, on_err])?;
        index += 1;
    })();
    if let Err(e) = r {
        if realm.terminating {
            return Err(e);
        }
        if !iterator_done {
            let _ = realm.iterator_close(&it);
        }
        realm.call(rej, Value::Undefined, &[e])?;
    }
    Ok(p)
}

fn all(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    combinator(realm, this, args, Combinator::All)
}

fn all_settled(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    combinator(realm, this, args, Combinator::AllSettled)
}

fn race(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    combinator(realm, this, args, Combinator::Race)
}

fn any(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    combinator(realm, this, args, Combinator::Any)
}

/// Element function captures: (state, index, alreadyCalled holder).
/// Returns None when this element (or its sibling) already ran.
fn element_enter(realm: &mut Realm) -> Option<(ObjectId, usize)> {
    let caps = realm.current_native().map(|n| realm.native_captures(n)).unwrap_or_default();
    let state = caps.first().and_then(|v| v.as_object())?;
    let index = caps.get(1).and_then(|v| v.as_number()).unwrap_or(0.0) as usize;
    let called = caps.get(2).and_then(|v| v.as_object())?;
    if realm.obj(called).props.get(&PropertyKey::str("called")).is_some() {
        return None;
    }
    realm.define_property(called, "called", Value::Bool(true), ALL);
    Some((state, index))
}

fn element_store(realm: &mut Realm, state: ObjectId, index: usize, v: Value, kind: Combinator) -> JsResult<Value> {
    let list = state_list(realm, state);
    list_set(realm, list, index, v);
    if state_bump(realm, state, -1.0) == 0.0 {
        state_finish(realm, state, kind)?;
    }
    Ok(Value::Undefined)
}

fn all_element(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let Some((state, index)) = element_enter(realm) else { return Ok(Value::Undefined) };
    element_store(realm, state, index, arg(args, 0), Combinator::All)
}

fn settled_common(realm: &mut Realm, args: &[Value], fulfilled: bool) -> JsResult<Value> {
    let Some((state, index)) = element_enter(realm) else { return Ok(Value::Undefined) };
    let o = realm.new_object();
    realm.define_property(o, "status", Value::str(if fulfilled { "fulfilled" } else { "rejected" }), ALL);
    realm.define_property(o, if fulfilled { "value" } else { "reason" }, arg(args, 0), ALL);
    element_store(realm, state, index, Value::Object(o), Combinator::AllSettled)
}

fn settled_fulfilled(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    settled_common(realm, args, true)
}

fn settled_rejected(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    settled_common(realm, args, false)
}

fn any_rejected(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let Some((state, index)) = element_enter(realm) else { return Ok(Value::Undefined) };
    element_store(realm, state, index, arg(args, 0), Combinator::Any)
}
