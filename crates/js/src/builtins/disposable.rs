//! Explicit resource management: SuppressedError, DisposableStack,
//! AsyncDisposableStack and the runtime of `using` / `await using`
//! declarations (the `%Using*` functions the parser's lowering calls, see
//! `parser_using.rs`).
//!
//! A dispose capability is a [`SlotData`]: `list` holds the resource stack
//! as (value, method, hint) triples, `values[0]` the recorded error
//! (`Empty` when none), `values[1]` the value to await next and `num[1]`
//! the needsAwait / hasAwaited flags of DisposeResources.

use super::*;
use super::iterator_helpers::{new_slot_object, slot_object};
use crate::realm::{SYM_ASYNC_DISPOSE, SYM_DISPOSE, SYM_TO_STRING_TAG};

const TAG_CAP: u8 = 10;
const TAG_STACK: u8 = 11;
const TAG_ASYNC_STACK: u8 = 12;
const TAG_ASYNC_DRIVE: u8 = 13;

const HINT_SYNC: f64 = 0.0;
const HINT_ASYNC: f64 = 1.0;

const NEEDS_AWAIT: f64 = 1.0;
const HAS_AWAITED: f64 = 2.0;

const DISPOSED: u8 = 1;

pub fn register(realm: &mut Realm) {
    // SuppressedError
    let ep = realm.intrinsics.error_proto;
    let sp = realm.new_object_with_proto(Some(ep));
    realm.define_property(sp, "name", Value::str("SuppressedError"), HIDDEN);
    realm.define_property(sp, "message", Value::str(""), HIDDEN);
    let c = define_ctor(realm, "SuppressedError", 3, suppressed_error_ctor, sp);
    if let Some(&e) = realm.intrinsics.others.get("Error") {
        realm.obj_mut(c).set_proto(Some(e));
    }
    realm.intrinsics.others.insert("SuppressedErrorPrototype", sp);
    realm.intrinsics.others.insert("SuppressedError", c);

    // DisposableStack
    let op = realm.intrinsics.object_proto;
    let dp = realm.new_object_with_proto(Some(op));
    let dc = define_ctor(realm, "DisposableStack", 0, disposable_stack_ctor, dp);
    realm.intrinsics.others.insert("DisposableStack", dc);
    realm.intrinsics.others.insert("DisposableStackPrototype", dp);
    realm.define_accessor(dp, "disposed", Some(stack_disposed), None, CONFIGURABLE);
    let dispose = realm.define_method(dp, "dispose", 0, stack_dispose);
    realm.define_property(dp, PropertyKey::Symbol(SYM_DISPOSE), Value::Object(dispose), HIDDEN);
    realm.define_method(dp, "use", 1, stack_use);
    realm.define_method(dp, "adopt", 2, stack_adopt);
    realm.define_method(dp, "defer", 1, stack_defer);
    realm.define_method(dp, "move", 0, stack_move);
    realm.define_property(dp, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("DisposableStack"), CONFIGURABLE);

    // AsyncDisposableStack
    let ap = realm.new_object_with_proto(Some(op));
    let ac = define_ctor(realm, "AsyncDisposableStack", 0, async_stack_ctor, ap);
    realm.intrinsics.others.insert("AsyncDisposableStack", ac);
    realm.intrinsics.others.insert("AsyncDisposableStackPrototype", ap);
    realm.define_accessor(ap, "disposed", Some(async_stack_disposed), None, CONFIGURABLE);
    let dispose = realm.define_method(ap, "disposeAsync", 0, async_stack_dispose);
    realm.define_property(ap, PropertyKey::Symbol(SYM_ASYNC_DISPOSE), Value::Object(dispose), HIDDEN);
    realm.define_method(ap, "use", 1, async_stack_use);
    realm.define_method(ap, "adopt", 2, async_stack_adopt);
    realm.define_method(ap, "defer", 1, async_stack_defer);
    realm.define_method(ap, "move", 0, async_stack_move);
    realm.define_property(ap, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("AsyncDisposableStack"), CONFIGURABLE);

    // Runtime of the `using` lowering: global lexical bindings whose names
    // can't be written in source code.
    let fns: [(&str, u32, NativeFn); 7] = [
        ("%UsingNew", 0, using_new),
        ("%UsingAdd", 3, using_add),
        ("%UsingError", 2, using_error),
        ("%UsingStep", 1, using_step_fn),
        ("%UsingValue", 1, using_value),
        ("%UsingDispose", 1, using_dispose),
        ("%usingDecl", 2, using_decl_identity),
    ];
    for (name, arity, f) in fns {
        let fid = realm.new_native_function(name, arity, f);
        realm.global_lex.insert(JsStr::new(name), (Value::Object(fid), true));
    }
}

/// OrdinaryCreateFromConstructor's prototype lookup (abrupt completions of
/// `newTarget.prototype` propagate).
fn proto_from_ctor(realm: &mut Realm, default: ObjectId) -> JsResult<ObjectId> {
    let nt = realm.new_target();
    if nt.is_object() {
        if let V::Object(p) = realm.get(nt, PropertyKey::str("prototype"))?.v() {
            return Ok(p);
        }
    }
    Ok(default)
}

// ------------------------------------------------------- SuppressedError

fn suppressed_error_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let default = realm.intrinsics.others["SuppressedErrorPrototype"];
    let proto = proto_from_ctor(realm, default)?;
    let id = realm.alloc(Object::new(Some(proto), ObjectKind::Error));
    let msg = arg(args, 2);
    let mut msg_str = String::new();
    if !msg.is_undefined() {
        let s = realm.to_string(msg)?;
        msg_str = s.to_rust_string();
        realm.define_property(id, "message", Value::String(s), HIDDEN);
    }
    realm.define_property(id, "error", arg(args, 0), HIDDEN);
    realm.define_property(id, "suppressed", arg(args, 1), HIDDEN);
    let stack = realm.build_stack_string("SuppressedError", &msg_str);
    realm.define_property(id, "stack", Value::str(&stack), HIDDEN);
    Ok(Value::Object(id))
}

/// A new SuppressedError { error, suppressed } (DisposeResources).
fn new_suppressed_error(realm: &mut Realm, error: Value, suppressed: Value) -> Value {
    let proto = realm.intrinsics.others["SuppressedErrorPrototype"];
    let id = realm.alloc(Object::new(Some(proto), ObjectKind::Error));
    realm.define_property(id, "error", error, HIDDEN);
    realm.define_property(id, "suppressed", suppressed, HIDDEN);
    let stack = realm.build_stack_string("SuppressedError", "An error was suppressed during disposal");
    realm.define_property(id, "stack", Value::str(&stack), HIDDEN);
    Value::Object(id)
}

// ------------------------------------------------- dispose capabilities

fn cap(realm: &Realm, id: ObjectId) -> &SlotData {
    match &realm.obj(id).kind {
        ObjectKind::Slots(d) => d,
        _ => unreachable!("not a dispose capability"),
    }
}

fn cap_mut(realm: &mut Realm, id: ObjectId) -> &mut SlotData {
    match &mut realm.obj_mut(id).kind {
        ObjectKind::Slots(d) => d,
        _ => unreachable!("not a dispose capability"),
    }
}

fn new_cap_data(tag: u8) -> SlotData {
    SlotData { tag, values: vec![Value::Empty, Value::Undefined], ..Default::default() }
}

/// GetDisposeMethod(V, hint).
fn get_dispose_method(realm: &mut Realm, v: &Value, hint: f64) -> JsResult<Value> {
    if hint == HINT_ASYNC {
        if let Some(m) = realm.get_method(v.clone(), PropertyKey::Symbol(SYM_ASYNC_DISPOSE))? {
            return Ok(m);
        }
        return Ok(match realm.get_method(v.clone(), PropertyKey::Symbol(SYM_DISPOSE))? {
            Some(m) => Value::Object(realm.new_native_function_full("", 0, async_wrapped_dispose, None, vec![m], false)),
            None => Value::Undefined,
        });
    }
    Ok(realm.get_method(v.clone(), PropertyKey::Symbol(SYM_DISPOSE))?.unwrap_or(Value::Undefined))
}

/// The async-dispose wrapper of a sync `@@dispose` method: a promise for
/// undefined (rejected when the method throws).
fn async_wrapped_dispose(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let method = native_capture(realm, 0);
    let p = realm.new_promise_object();
    match realm.call(method, this, &[]) {
        Ok(_) => realm.resolve_promise(p, Value::Undefined),
        Err(e) => realm.reject_promise(p, e),
    }
    Ok(Value::Object(p))
}

fn native_capture(realm: &Realm, i: usize) -> Value {
    realm.current_native().and_then(|n| realm.native_captures(n).into_iter().nth(i)).unwrap_or(Value::Undefined)
}

/// AddDisposableResource(cap, V, hint[, method]).
fn add_disposable_resource(realm: &mut Realm, cap_id: ObjectId, v: Value, hint: f64, method: Option<Value>) -> JsResult<()> {
    let (value, method) = match method {
        None => {
            if v.is_nullish() {
                if hint == HINT_SYNC {
                    return Ok(());
                }
                (Value::Undefined, Value::Undefined)
            } else {
                if !v.is_object() {
                    let d = realm.describe_for_error(&v);
                    return realm.type_error(&format!("{} is not an object", d));
                }
                let m = get_dispose_method(realm, &v, hint)?;
                if m.is_undefined() {
                    let what = if hint == HINT_ASYNC { "Symbol.asyncDispose or Symbol.dispose" } else { "Symbol.dispose" };
                    return realm.type_error(&format!("The value has no callable {} method", what));
                }
                if !realm.is_callable(&m) {
                    return realm.type_error("The dispose method is not callable");
                }
                (v, m)
            }
        }
        Some(m) => {
            if !realm.is_callable(&m) {
                return realm.type_error("The dispose method is not callable");
            }
            (Value::Undefined, m)
        }
    };
    let d = cap_mut(realm, cap_id);
    d.list.push(value);
    d.list.push(method);
    d.list.push(Value::Number(hint));
    Ok(())
}

/// Record a throw completion: the first error as is, later ones as a
/// SuppressedError wrapping the previous completion.
fn record_error(realm: &mut Realm, cap_id: ObjectId, e: Value) {
    let prev = std::mem::replace(&mut cap_mut(realm, cap_id).values[0], Value::Empty);
    let err = if prev.is_empty() { e } else { new_suppressed_error(realm, e, prev) };
    cap_mut(realm, cap_id).values[0] = err;
}

fn has_flag(flags: f64, f: f64) -> bool {
    (flags as u32) & (f as u32) != 0
}

/// One step of DisposeResources: run disposers from the top of the stack
/// until one needs an `Await` (its operand goes to `values[1]`; returns
/// true) or the stack is empty (false). Errors are recorded.
fn dispose_step(realm: &mut Realm, cap_id: ObjectId) -> bool {
    loop {
        let (len, flags) = {
            let d = cap(realm, cap_id);
            (d.list.len(), d.num[1])
        };
        let needs_await = has_flag(flags, NEEDS_AWAIT);
        let has_awaited = has_flag(flags, HAS_AWAITED);
        if len < 3 {
            if needs_await && !has_awaited {
                let d = cap_mut(realm, cap_id);
                d.num[1] = flags - NEEDS_AWAIT;
                d.values[1] = Value::Undefined;
                return true;
            }
            return false;
        }
        let hint = match cap(realm, cap_id).list[len - 1].v() {
            V::Number(h) => h,
            _ => HINT_SYNC,
        };
        if hint == HINT_SYNC && needs_await && !has_awaited {
            let d = cap_mut(realm, cap_id);
            d.num[1] = flags - NEEDS_AWAIT;
            d.values[1] = Value::Undefined;
            return true;
        }
        let (value, method) = {
            let d = cap_mut(realm, cap_id);
            d.list.pop();
            let m = d.list.pop().unwrap_or(Value::Undefined);
            let v = d.list.pop().unwrap_or(Value::Undefined);
            (v, m)
        };
        if method.is_undefined() {
            let d = cap_mut(realm, cap_id);
            if !has_flag(d.num[1], NEEDS_AWAIT) {
                d.num[1] += NEEDS_AWAIT;
            }
            continue;
        }
        match realm.call(method, value, &[]) {
            Ok(r) => {
                if hint == HINT_ASYNC {
                    let d = cap_mut(realm, cap_id);
                    if !has_flag(d.num[1], HAS_AWAITED) {
                        d.num[1] += HAS_AWAITED;
                    }
                    d.values[1] = r;
                    return true;
                }
            }
            Err(e) => record_error(realm, cap_id, e),
        }
    }
}

/// Finish DisposeResources: the recorded error, if any.
fn take_completion(realm: &mut Realm, cap_id: ObjectId) -> JsResult<()> {
    let d = cap_mut(realm, cap_id);
    d.list.clear();
    d.num[1] = 0.0;
    d.values[1] = Value::Undefined;
    let e = std::mem::replace(&mut d.values[0], Value::Empty);
    if e.is_empty() {
        Ok(())
    } else {
        Err(e)
    }
}

/// Synchronous DisposeResources (awaits are skipped).
fn dispose_sync(realm: &mut Realm, cap_id: ObjectId) -> JsResult<()> {
    while dispose_step(realm, cap_id) {}
    take_completion(realm, cap_id)
}

// ------------------------------------------------- `using` runtime

fn using_cap(realm: &mut Realm, v: &Value) -> JsResult<ObjectId> {
    match slot_object(realm, v, TAG_CAP) {
        Some(id) => Ok(id),
        None => realm.type_error("invalid dispose capability"),
    }
}

fn using_new(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = realm.alloc(Object::new(None, ObjectKind::Slots(Box::new(new_cap_data(TAG_CAP)))));
    Ok(Value::Object(id))
}

fn using_add(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let c = using_cap(realm, &arg(args, 0))?;
    let hint = match arg(args, 2).v() {
        V::Number(h) => h,
        _ => HINT_SYNC,
    };
    add_disposable_resource(realm, c, arg(args, 1), hint, None)?;
    Ok(Value::Undefined)
}

fn using_error(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let c = using_cap(realm, &arg(args, 0))?;
    record_error(realm, c, arg(args, 1));
    Ok(Value::Undefined)
}

fn using_step_fn(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let c = using_cap(realm, &arg(args, 0))?;
    Ok(Value::Bool(dispose_step(realm, c)))
}

fn using_value(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let c = using_cap(realm, &arg(args, 0))?;
    Ok(std::mem::replace(&mut cap_mut(realm, c).values[1], Value::Undefined))
}

fn using_dispose(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let c = using_cap(realm, &arg(args, 0))?;
    dispose_sync(realm, c)?;
    Ok(Value::Undefined)
}

/// A `using` declaration the parser could not lower (module top level
/// with exports): the plain value, nothing is registered.
fn using_decl_identity(_realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(arg(args, 0))
}

// ------------------------------------------------------ DisposableStack

fn new_stack(realm: &mut Realm, tag: u8, proto: ObjectId) -> ObjectId {
    new_slot_object(realm, proto, new_cap_data(tag))
}

fn disposable_stack_ctor(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    if !realm.is_construct_call() {
        return realm.type_error("Constructor DisposableStack requires 'new'");
    }
    let default = realm.intrinsics.others["DisposableStackPrototype"];
    let proto = proto_from_ctor(realm, default)?;
    Ok(Value::Object(new_stack(realm, TAG_STACK, proto)))
}

fn this_stack(realm: &mut Realm, this: &Value, tag: u8, method: &str) -> JsResult<ObjectId> {
    match slot_object(realm, this, tag) {
        Some(id) => Ok(id),
        None => {
            let class = if tag == TAG_STACK { "DisposableStack" } else { "AsyncDisposableStack" };
            realm.type_error(&format!("{}.prototype.{} called on incompatible receiver", class, method))
        }
    }
}

fn this_pending_stack(realm: &mut Realm, this: &Value, tag: u8, method: &str) -> JsResult<ObjectId> {
    let id = this_stack(realm, this, tag, method)?;
    if cap(realm, id).state == DISPOSED {
        return Err(realm.new_error("ReferenceError", "The stack has already been disposed"));
    }
    Ok(id)
}

fn stack_disposed(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_stack(realm, &this, TAG_STACK, "disposed")?;
    Ok(Value::Bool(cap(realm, id).state == DISPOSED))
}

fn stack_dispose(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_stack(realm, &this, TAG_STACK, "dispose")?;
    if cap(realm, id).state == DISPOSED {
        return Ok(Value::Undefined);
    }
    cap_mut(realm, id).state = DISPOSED;
    dispose_sync(realm, id)?;
    Ok(Value::Undefined)
}

fn stack_use(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_pending_stack(realm, &this, TAG_STACK, "use")?;
    let v = arg(args, 0);
    add_disposable_resource(realm, id, v.clone(), HINT_SYNC, None)?;
    Ok(v)
}

/// The closure adopt() registers: `onDispose(value)`.
fn adopt_closure(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    let on_dispose = native_capture(realm, 0);
    let value = native_capture(realm, 1);
    realm.call(on_dispose, Value::Undefined, &[value])
}

fn adopt(realm: &mut Realm, this: Value, args: &[Value], tag: u8, hint: f64) -> JsResult<Value> {
    let id = this_pending_stack(realm, &this, tag, "adopt")?;
    let value = arg(args, 0);
    let on_dispose = arg(args, 1);
    if !realm.is_callable(&on_dispose) {
        return realm.type_error("onDispose is not a function");
    }
    let f = realm.new_native_function_full("", 0, adopt_closure, None, vec![on_dispose, value.clone()], false);
    add_disposable_resource(realm, id, Value::Undefined, hint, Some(Value::Object(f)))?;
    Ok(value)
}

fn defer(realm: &mut Realm, this: Value, args: &[Value], tag: u8, hint: f64) -> JsResult<Value> {
    let id = this_pending_stack(realm, &this, tag, "defer")?;
    let on_dispose = arg(args, 0);
    if !realm.is_callable(&on_dispose) {
        return realm.type_error("onDispose is not a function");
    }
    add_disposable_resource(realm, id, Value::Undefined, hint, Some(on_dispose))?;
    Ok(Value::Undefined)
}

fn move_stack(realm: &mut Realm, this: Value, tag: u8, proto_key: &str) -> JsResult<Value> {
    let id = this_pending_stack(realm, &this, tag, "move")?;
    let proto = realm.intrinsics.others[proto_key];
    let new_id = new_stack(realm, tag, proto);
    let list = std::mem::take(&mut cap_mut(realm, id).list);
    cap_mut(realm, new_id).list = list;
    cap_mut(realm, id).state = DISPOSED;
    Ok(Value::Object(new_id))
}

fn stack_adopt(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    adopt(realm, this, args, TAG_STACK, HINT_SYNC)
}

fn stack_defer(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    defer(realm, this, args, TAG_STACK, HINT_SYNC)
}

fn stack_move(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    move_stack(realm, this, TAG_STACK, "DisposableStackPrototype")
}

// ------------------------------------------------- AsyncDisposableStack

fn async_stack_ctor(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    if !realm.is_construct_call() {
        return realm.type_error("Constructor AsyncDisposableStack requires 'new'");
    }
    let default = realm.intrinsics.others["AsyncDisposableStackPrototype"];
    let proto = proto_from_ctor(realm, default)?;
    Ok(Value::Object(new_stack(realm, TAG_ASYNC_STACK, proto)))
}

fn async_stack_disposed(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_stack(realm, &this, TAG_ASYNC_STACK, "disposed")?;
    Ok(Value::Bool(cap(realm, id).state == DISPOSED))
}

fn async_stack_use(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_pending_stack(realm, &this, TAG_ASYNC_STACK, "use")?;
    let v = arg(args, 0);
    add_disposable_resource(realm, id, v.clone(), HINT_ASYNC, None)?;
    Ok(v)
}

fn async_stack_adopt(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    adopt(realm, this, args, TAG_ASYNC_STACK, HINT_ASYNC)
}

fn async_stack_defer(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    defer(realm, this, args, TAG_ASYNC_STACK, HINT_ASYNC)
}

fn async_stack_move(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    move_stack(realm, this, TAG_ASYNC_STACK, "AsyncDisposableStackPrototype")
}

fn async_stack_dispose(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let p = realm.new_promise_object();
    let id = match slot_object(realm, &this, TAG_ASYNC_STACK) {
        Some(id) => id,
        None => {
            let e = realm.throw_type_error("AsyncDisposableStack.prototype.disposeAsync called on incompatible receiver");
            realm.reject_promise(p, e);
            return Ok(Value::Object(p));
        }
    };
    if cap(realm, id).state == DISPOSED {
        realm.resolve_promise(p, Value::Undefined);
        return Ok(Value::Object(p));
    }
    cap_mut(realm, id).state = DISPOSED;
    async_drive(realm, id, p);
    Ok(Value::Object(p))
}

/// Run DisposeResources of an async stack, awaiting through promise
/// reactions; settles `p` at the end.
fn async_drive(realm: &mut Realm, id: ObjectId, p: ObjectId) {
    if dispose_step(realm, id) {
        let v = std::mem::replace(&mut cap_mut(realm, id).values[1], Value::Undefined);
        match crate::builtins::promise::promise_resolve(realm, v) {
            Ok(w) => {
                let data = SlotData { tag: TAG_ASYNC_DRIVE, values: vec![Value::Object(id), Value::Object(p)], ..Default::default() };
                let proto = realm.intrinsics.object_proto;
                let d = new_slot_object(realm, proto, data);
                crate::builtins::promise::perform_then_native(realm, w, async_drive_ok, async_drive_err, Value::Object(d));
                return;
            }
            Err(e) => {
                record_error(realm, id, e);
                return async_drive(realm, id, p);
            }
        }
    }
    match take_completion(realm, id) {
        Ok(()) => realm.resolve_promise(p, Value::Undefined),
        Err(e) => realm.reject_promise(p, e),
    }
}

fn async_drive_parts(realm: &Realm) -> Option<(ObjectId, ObjectId)> {
    let d = native_capture(realm, 0);
    let id = slot_object(realm, &d, TAG_ASYNC_DRIVE)?;
    let s = cap(realm, id);
    Some((s.values[0].as_object()?, s.values[1].as_object()?))
}

fn async_drive_ok(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    if let Some((id, p)) = async_drive_parts(realm) {
        async_drive(realm, id, p);
    }
    Ok(Value::Undefined)
}

fn async_drive_err(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if let Some((id, p)) = async_drive_parts(realm) {
        record_error(realm, id, arg(args, 0));
        async_drive(realm, id, p);
    }
    Ok(Value::Undefined)
}
