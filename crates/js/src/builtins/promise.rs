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
            // Only when its constructor is %Promise% (approximation: accept all promises)
            return Ok(id);
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
    let on_ok = if realm.is_callable(&on_ok) { Some(on_ok) } else { None };
    let on_err = if realm.is_callable(&on_err) { Some(on_err) } else { None };
    let (np, res, rej) = new_promise_capability(realm);
    perform_then(realm, p, on_ok, on_err, Some((Value::Object(np), Value::Object(res), Value::Object(rej))));
    Ok(Value::Object(np))
}

fn catch(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    realm.invoke(this, "then", &[Value::Undefined, arg(args, 0)])
}

fn finally(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let on_finally = arg(args, 0);
    if !realm.is_callable(&on_finally) {
        return realm.invoke(this, "then", &[on_finally.clone(), on_finally]);
    }
    let then_finally = realm.new_native_function_full("", 1, finally_fulfilled, None, vec![on_finally.clone()], false);
    let catch_finally = realm.new_native_function_full("", 1, finally_rejected, None, vec![on_finally], false);
    realm.invoke(this, "then", &[Value::Object(then_finally), Value::Object(catch_finally)])
}

fn finally_fulfilled(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let f = realm.current_native().and_then(|n| realm.native_captures(n).first().cloned()).unwrap_or(Value::Undefined);
    let value = arg(args, 0);
    let r = realm.call(f, Value::Undefined, &[])?;
    let p = promise_resolve(realm, r)?;
    let value_thunk = realm.new_native_function_full("", 0, return_capture, None, vec![value], false);
    realm.invoke(Value::Object(p), "then", &[Value::Object(value_thunk)])
}

fn finally_rejected(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let f = realm.current_native().and_then(|n| realm.native_captures(n).first().cloned()).unwrap_or(Value::Undefined);
    let reason = arg(args, 0);
    let r = realm.call(f, Value::Undefined, &[])?;
    let p = promise_resolve(realm, r)?;
    let thrower = realm.new_native_function_full("", 0, throw_capture, None, vec![reason], false);
    realm.invoke(Value::Object(p), "then", &[Value::Object(thrower)])
}

fn return_capture(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(realm.current_native().and_then(|n| realm.native_captures(n).first().cloned()).unwrap_or(Value::Undefined))
}

fn throw_capture(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    Err(realm.current_native().and_then(|n| realm.native_captures(n).first().cloned()).unwrap_or(Value::Undefined))
}

fn promise_resolve_static(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let v = arg(args, 0);
    if this.as_object() == Some(realm.intrinsics.promise_ctor) || !realm.is_constructor(&this) {
        return Ok(Value::Object(promise_resolve(realm, v)?));
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
fn new_capability_with_ctor(realm: &mut Realm, c: Value) -> JsResult<(Value, Value, Value)> {
    if c.as_object() == Some(realm.intrinsics.promise_ctor) || !realm.is_constructor(&c) {
        let (p, res, rej) = new_promise_capability(realm);
        return Ok((Value::Object(p), Value::Object(res), Value::Object(rej)));
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

/// Shared state for the combinators: (result promise capability, values array, remaining count).
fn combinator_setup(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<(Value, Value, Value, Vec<Value>, ObjectId)> {
    let (p, res, rej) = new_capability_with_ctor(realm, this)?;
    let items = match realm.iterate_to_vec(arg(args, 0)) {
        Ok(items) => items,
        Err(e) => {
            realm.call(rej.clone(), Value::Undefined, &[e])?;
            return Ok((p, res, rej, Vec::new(), realm.new_object_with_proto(None)));
        }
    };
    let state = realm.new_object_with_proto(None);
    let values = realm.new_array_value(vec![Value::Undefined; items.len()]);
    realm.define_property(state, "values", values, ALL);
    realm.define_property(state, "remaining", Value::Number(items.len() as f64), ALL);
    realm.define_property(state, "resolve", res.clone(), ALL);
    realm.define_property(state, "reject", rej.clone(), ALL);
    Ok((p, res, rej, items, state))
}

fn all(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (p, res, rej, items, state) = combinator_setup(realm, this.clone(), args)?;
    if items.is_empty() {
        let values = realm.get_str(Value::Object(state), "values").unwrap_or(Value::Undefined);
        if !values.is_undefined() {
            realm.call(res, Value::Undefined, &[values])?;
        }
        return Ok(p);
    }
    for (i, item) in items.into_iter().enumerate() {
        let next = realm.invoke(this.clone(), "resolve", &[item])?;
        let on_ok = realm.new_native_function_full("", 1, all_element, None, vec![Value::Object(state), Value::Number(i as f64)], false);
        realm.invoke(next, "then", &[Value::Object(on_ok), rej.clone()])?;
    }
    Ok(p)
}

fn all_element(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let caps = realm.current_native().map(|n| realm.native_captures(n)).unwrap_or_default();
    let state = caps.first().and_then(|v| v.as_object()).unwrap();
    let index = caps.get(1).and_then(|v| v.as_number()).unwrap_or(0.0) as u32;
    let done_key = PropertyKey::str(&format!("done{}", index));
    if realm.has_own_property(state, &done_key) {
        return Ok(Value::Undefined);
    }
    realm.define_property(state, done_key, Value::Bool(true), ALL);
    let values = realm.get_str(Value::Object(state), "values")?;
    realm.set(values.clone(), PropertyKey::index(index), arg(args, 0))?;
    let remaining = realm.get_str(Value::Object(state), "remaining")?.as_number().unwrap_or(0.0) - 1.0;
    realm.define_property(state, "remaining", Value::Number(remaining), ALL);
    if remaining <= 0.0 {
        let res = realm.get_str(Value::Object(state), "resolve")?;
        realm.call(res, Value::Undefined, &[values])?;
    }
    Ok(Value::Undefined)
}

fn all_settled(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (p, res, _rej, items, state) = combinator_setup(realm, this.clone(), args)?;
    if items.is_empty() {
        let values = realm.get_str(Value::Object(state), "values").unwrap_or(Value::Undefined);
        if !values.is_undefined() {
            realm.call(res, Value::Undefined, &[values])?;
        }
        return Ok(p);
    }
    for (i, item) in items.into_iter().enumerate() {
        let next = realm.invoke(this.clone(), "resolve", &[item])?;
        let on_ok = realm.new_native_function_full("", 1, settled_fulfilled, None, vec![Value::Object(state), Value::Number(i as f64)], false);
        let on_err = realm.new_native_function_full("", 1, settled_rejected, None, vec![Value::Object(state), Value::Number(i as f64)], false);
        realm.invoke(next, "then", &[Value::Object(on_ok), Value::Object(on_err)])?;
    }
    Ok(p)
}

fn settled_common(realm: &mut Realm, args: &[Value], fulfilled: bool) -> JsResult<Value> {
    let caps = realm.current_native().map(|n| realm.native_captures(n)).unwrap_or_default();
    let state = caps.first().and_then(|v| v.as_object()).unwrap();
    let index = caps.get(1).and_then(|v| v.as_number()).unwrap_or(0.0) as u32;
    let done_key = PropertyKey::str(&format!("done{}", index));
    if realm.has_own_property(state, &done_key) {
        return Ok(Value::Undefined);
    }
    realm.define_property(state, done_key, Value::Bool(true), ALL);
    let o = realm.new_object();
    realm.define_property(o, "status", Value::str(if fulfilled { "fulfilled" } else { "rejected" }), ALL);
    realm.define_property(o, if fulfilled { "value" } else { "reason" }, arg(args, 0), ALL);
    let values = realm.get_str(Value::Object(state), "values")?;
    realm.set(values.clone(), PropertyKey::index(index), Value::Object(o))?;
    let remaining = realm.get_str(Value::Object(state), "remaining")?.as_number().unwrap_or(0.0) - 1.0;
    realm.define_property(state, "remaining", Value::Number(remaining), ALL);
    if remaining <= 0.0 {
        let res = realm.get_str(Value::Object(state), "resolve")?;
        realm.call(res, Value::Undefined, &[values])?;
    }
    Ok(Value::Undefined)
}

fn settled_fulfilled(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    settled_common(realm, args, true)
}
fn settled_rejected(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    settled_common(realm, args, false)
}

fn race(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (p, res, rej, items, _state) = combinator_setup(realm, this.clone(), args)?;
    for item in items {
        let next = realm.invoke(this.clone(), "resolve", &[item])?;
        realm.invoke(next, "then", &[res.clone(), rej.clone()])?;
    }
    Ok(p)
}

fn any(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (p, res, rej, items, state) = combinator_setup(realm, this.clone(), args)?;
    if items.is_empty() {
        let e = realm.new_error("AggregateError", "All promises were rejected");
        realm.call(rej, Value::Undefined, &[e])?;
        return Ok(p);
    }
    for (i, item) in items.into_iter().enumerate() {
        let next = realm.invoke(this.clone(), "resolve", &[item])?;
        let on_err = realm.new_native_function_full("", 1, any_rejected, None, vec![Value::Object(state), Value::Number(i as f64)], false);
        realm.invoke(next, "then", &[res.clone(), Value::Object(on_err)])?;
    }
    Ok(p)
}

fn any_rejected(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let caps = realm.current_native().map(|n| realm.native_captures(n)).unwrap_or_default();
    let state = caps.first().and_then(|v| v.as_object()).unwrap();
    let index = caps.get(1).and_then(|v| v.as_number()).unwrap_or(0.0) as u32;
    let values = realm.get_str(Value::Object(state), "values")?;
    realm.set(values.clone(), PropertyKey::index(index), arg(args, 0))?;
    let remaining = realm.get_str(Value::Object(state), "remaining")?.as_number().unwrap_or(0.0) - 1.0;
    realm.define_property(state, "remaining", Value::Number(remaining), ALL);
    if remaining <= 0.0 {
        let rej = realm.get_str(Value::Object(state), "reject")?;
        let e = realm.new_error("AggregateError", "All promises were rejected");
        if let V::Object(eid) = e.v() {
            realm.define_property(eid, "errors", values, HIDDEN);
        }
        realm.call(rej, Value::Undefined, &[e])?;
    }
    Ok(Value::Undefined)
}
