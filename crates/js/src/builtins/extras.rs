//! Remaining ES2023 pieces: `WeakRef`, `FinalizationRegistry`, `Atomics`,
//! `Promise.try`, legacy `RegExp` statics (`RegExp.$1`, `RegExp.lastMatch`).
//!
//! `WeakRef` holds its target strongly (no ephemerons in the collector), so
//! `deref()` always returns the object; `FinalizationRegistry` callbacks
//! never run.

use super::typed_array::{ta_get, ta_set_value};
use crate::value::V;
use super::*;
use crate::realm::SYM_TO_STRING_TAG;

pub fn register(realm: &mut Realm) {
    let op = realm.intrinsics.object_proto;

    // ---------------------------------------------------------- WeakRef
    let target_sym = realm.symbol_for(JsStr::new("rabbithole.weakref.target"));
    let sym_obj = realm.new_object_with_proto(None);
    realm.define_property(sym_obj, "sym", Value::Symbol(target_sym), 0);
    realm.intrinsics.others.insert("WeakRefTargetKey", sym_obj);
    let wrp = realm.new_object_with_proto(Some(op));
    realm.intrinsics.others.insert("WeakRefPrototype", wrp);
    define_ctor(realm, "WeakRef", 1, weak_ref_ctor, wrp);
    realm.define_method(wrp, "deref", 0, weak_ref_deref);
    realm.define_property(wrp, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("WeakRef"), CONFIGURABLE);

    // ------------------------------------------------ FinalizationRegistry
    let frp = realm.new_object_with_proto(Some(op));
    realm.intrinsics.others.insert("FinalizationRegistryPrototype", frp);
    define_ctor(realm, "FinalizationRegistry", 1, finalization_registry_ctor, frp);
    realm.define_method(frp, "register", 2, finalization_register);
    realm.define_method(frp, "unregister", 1, finalization_unregister);
    realm.define_property(frp, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("FinalizationRegistry"), CONFIGURABLE);

    // ---------------------------------------------------------- Atomics
    let atomics = realm.new_object_with_proto(Some(op));
    realm.define_global("Atomics", Value::Object(atomics));
    realm.define_property(atomics, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("Atomics"), CONFIGURABLE);
    let ops: &[(&str, u32, NativeFn)] = &[
        ("add", 3, atomics_add),
        ("and", 3, atomics_and),
        ("compareExchange", 4, atomics_compare_exchange),
        ("exchange", 3, atomics_exchange),
        ("isLockFree", 1, atomics_is_lock_free),
        ("load", 2, atomics_load),
        ("or", 3, atomics_or),
        ("store", 3, atomics_store),
        ("sub", 3, atomics_sub),
        ("xor", 3, atomics_xor),
        ("wait", 4, atomics_wait),
        ("notify", 3, atomics_notify),
    ];
    for (name, arity, f) in ops {
        realm.define_method(atomics, name, *arity, *f);
    }

    // ------------------------------------------------------ Promise.try
    let pc = realm.intrinsics.promise_ctor;
    realm.define_method(pc, "try", 1, promise_try);

    // --------------------------------------------------- Array.fromAsync
    // Written in JavaScript: it is an async function that needs `for await`.
    let _ = realm.eval(ARRAY_FROM_ASYNC, "<builtins/Array.fromAsync>");

    // ------------------------------------------------- RegExp legacy statics
    if let Some(rc) = realm.intrinsics.others.get("RegExp").copied().or_else(|| {
        let g = realm.global;
        realm.obj(g).props.get(&PropertyKey::str("RegExp")).and_then(|p| p.value.as_object())
    }) {
        for (name, f) in [
            ("input", regexp_static_input as NativeFn),
            ("$_", regexp_static_input),
            ("lastMatch", regexp_static_last_match),
            ("$&", regexp_static_last_match),
            ("lastParen", regexp_static_last_paren),
            ("$+", regexp_static_last_paren),
            ("leftContext", regexp_static_left_context),
            ("$`", regexp_static_left_context),
            ("rightContext", regexp_static_right_context),
            ("$'", regexp_static_right_context),
        ] {
            realm.define_accessor(rc, name, Some(f), Some(regexp_static_noop_set), CONFIGURABLE);
        }
        realm.define_accessor(rc, "$1", Some(regexp_static_1), Some(regexp_static_noop_set), CONFIGURABLE);
        realm.define_accessor(rc, "$2", Some(regexp_static_2), Some(regexp_static_noop_set), CONFIGURABLE);
        realm.define_accessor(rc, "$3", Some(regexp_static_3), Some(regexp_static_noop_set), CONFIGURABLE);
        realm.define_accessor(rc, "$4", Some(regexp_static_4), Some(regexp_static_noop_set), CONFIGURABLE);
        realm.define_accessor(rc, "$5", Some(regexp_static_5), Some(regexp_static_noop_set), CONFIGURABLE);
        realm.define_accessor(rc, "$6", Some(regexp_static_6), Some(regexp_static_noop_set), CONFIGURABLE);
        realm.define_accessor(rc, "$7", Some(regexp_static_7), Some(regexp_static_noop_set), CONFIGURABLE);
        realm.define_accessor(rc, "$8", Some(regexp_static_8), Some(regexp_static_noop_set), CONFIGURABLE);
        realm.define_accessor(rc, "$9", Some(regexp_static_9), Some(regexp_static_noop_set), CONFIGURABLE);
        realm.intrinsics.others.insert("RegExp", rc);
    }
}

const ARRAY_FROM_ASYNC: &str = r#"
Object.defineProperty(Array, "fromAsync", { writable: true, configurable: true, enumerable: false,
  value: async function fromAsync(items, mapFn, thisArg) {
    if (mapFn !== undefined && typeof mapFn !== "function") throw new TypeError("Array.fromAsync: mapFn is not a function");
    const C = typeof this === "function" ? this : Array;
    const out = C === Array ? [] : new C();
    let i = 0;
    if (items == null) throw new TypeError("Array.fromAsync requires an iterable or array-like");
    if (items[Symbol.asyncIterator] !== undefined || items[Symbol.iterator] !== undefined) {
      for await (const v of items) { out[i] = mapFn ? await mapFn.call(thisArg, v, i) : v; i++; }
    } else {
      const o = Object(items);
      const len = Math.min(Math.max(Math.trunc(Number(o.length)) || 0, 0), 9007199254740991);
      for (; i < len; i++) { const v = await o[i]; out[i] = mapFn ? await mapFn.call(thisArg, v, i) : v; }
    }
    out.length = i;
    return out;
  } });
"#;

// ------------------------------------------------------------ WeakRef

fn weak_ref_key(realm: &Realm) -> PropertyKey {
    let holder = realm.intrinsics.others["WeakRefTargetKey"];
    match (realm.obj(holder).props.get(&PropertyKey::str("sym")).map(|p| p.value.clone())).as_ref().map(Value::v) {
        Some(V::Symbol(s)) => PropertyKey::Symbol(s),
        _ => PropertyKey::str("__target"),
    }
}

fn can_be_held_weakly(realm: &Realm, v: &Value) -> bool {
    match v.v() {
        V::Object(_) => true,
        V::Symbol(s) => realm.symbol_key_for(s).is_none(),
        _ => false,
    }
}

fn weak_ref_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if !realm.is_construct_call() {
        return realm.type_error("Constructor WeakRef requires 'new'");
    }
    let target = arg(args, 0);
    if !can_be_held_weakly(realm, &target) {
        return realm.type_error("WeakRef: invalid target");
    }
    let default = realm.intrinsics.others["WeakRefPrototype"];
    let proto = realm.proto_from_new_target(default);
    let id = realm.new_object_with_proto(Some(proto));
    realm.set_class_name(id, "WeakRef");
    let key = weak_ref_key(realm);
    realm.define_property(id, key, target, 0);
    Ok(Value::Object(id))
}

fn weak_ref_deref(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_object(realm, &this, "WeakRef.prototype.deref")?;
    if realm.obj(id).class_name() != Some("WeakRef") {
        return realm.type_error("WeakRef.prototype.deref called on incompatible receiver");
    }
    let key = weak_ref_key(realm);
    Ok(realm.obj(id).props.get(&key).map(|p| p.value.clone()).unwrap_or(Value::Undefined))
}

// ----------------------------------------------------- FinalizationRegistry

fn finalization_registry_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if !realm.is_construct_call() {
        return realm.type_error("Constructor FinalizationRegistry requires 'new'");
    }
    let cb = arg(args, 0);
    if !realm.is_callable(&cb) {
        return realm.type_error("FinalizationRegistry: cleanup must be callable");
    }
    let default = realm.intrinsics.others["FinalizationRegistryPrototype"];
    let proto = realm.proto_from_new_target(default);
    let id = realm.new_object_with_proto(Some(proto));
    realm.set_class_name(id, "FinalizationRegistry");
    Ok(Value::Object(id))
}

fn finalization_register(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_object(realm, &this, "FinalizationRegistry.prototype.register")?;
    if realm.obj(id).class_name() != Some("FinalizationRegistry") {
        return realm.type_error("FinalizationRegistry.prototype.register called on incompatible receiver");
    }
    let target = arg(args, 0);
    if !can_be_held_weakly(realm, &target) {
        return realm.type_error("FinalizationRegistry.prototype.register: invalid target");
    }
    if realm.same_value(&target, &arg(args, 1)) {
        return realm.type_error("FinalizationRegistry.prototype.register: target and holdings must not be same");
    }
    let token = arg(args, 2);
    if !token.is_undefined() && !can_be_held_weakly(realm, &token) {
        return realm.type_error("FinalizationRegistry.prototype.register: invalid unregister token");
    }
    Ok(Value::Undefined)
}

fn finalization_unregister(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_object(realm, &this, "FinalizationRegistry.prototype.unregister")?;
    if realm.obj(id).class_name() != Some("FinalizationRegistry") {
        return realm.type_error("FinalizationRegistry.prototype.unregister called on incompatible receiver");
    }
    let token = arg(args, 0);
    if !can_be_held_weakly(realm, &token) {
        return realm.type_error("Invalid unregisterToken");
    }
    Ok(Value::Bool(false))
}

// ------------------------------------------------------------ Atomics

fn atomics_target(realm: &mut Realm, args: &[Value]) -> JsResult<(TypedArrayData, usize)> {
    let t = match (arg(args, 0)).v() {
        V::Object(id) => match &realm.obj(id).kind {
            ObjectKind::TypedArray(t) => TypedArrayData { kind: t.kind, buffer: t.buffer, byte_offset: t.byte_offset, length: t.length },
            _ => return realm.type_error("[object Object] is not an integer shared typed array."),
        },
        _ => return realm.type_error("Atomics operation on non-typed-array"),
    };
    if matches!(t.kind, TypedArrayKind::Float32 | TypedArrayKind::Float64 | TypedArrayKind::Uint8Clamped) {
        return realm.type_error("Atomics operations are not allowed on this typed array type");
    }
    let idx = realm.to_index(arg(args, 1))?;
    if idx >= t.length {
        return realm.range_error("Invalid atomic access index");
    }
    Ok((t, idx))
}

fn atomics_rmw(realm: &mut Realm, args: &[Value], op: fn(i128, i128) -> i128) -> JsResult<Value> {
    let (t, idx) = atomics_target(realm, args)?;
    let old = ta_get(realm, &t, idx).unwrap_or(Value::Number(0.0));
    let operand = arg(args, 2);
    let (a, b, is_big) = match (old.v(), operand.v()) {
        (V::BigInt(o), _) => (o.to_i128_wrapping(), super::typed_array::to_bigint(realm, operand.clone())?, true),
        (V::Number(o), _) => (o as i128, to_integer(realm.to_number(operand.clone())?) as i128, false),
        _ => (0, 0, false),
    };
    let new = op(a, b);
    let new_v = if is_big { realm.new_bigint(new) } else { Value::Number(new as f64) };
    ta_set_value(realm, &t, idx, new_v)?;
    Ok(old)
}

fn atomics_add(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    atomics_rmw(realm, args, |a, b| a.wrapping_add(b))
}
fn atomics_sub(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    atomics_rmw(realm, args, |a, b| a.wrapping_sub(b))
}
fn atomics_and(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    atomics_rmw(realm, args, |a, b| a & b)
}
fn atomics_or(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    atomics_rmw(realm, args, |a, b| a | b)
}
fn atomics_xor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    atomics_rmw(realm, args, |a, b| a ^ b)
}
fn atomics_exchange(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    atomics_rmw(realm, args, |_, b| b)
}

fn atomics_compare_exchange(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let (t, idx) = atomics_target(realm, args)?;
    let old = ta_get(realm, &t, idx).unwrap_or(Value::Number(0.0));
    // Coerce the expected value through the array type so comparisons match.
    let expected = arg(args, 2);
    let replacement = arg(args, 3);
    let matches = match (old.v(), expected.v()) {
        (V::BigInt(o), e) => super::typed_array::to_bigint(realm, e.to_value())? == o.to_i128_wrapping(),
        (V::Number(o), e) => {
            let n = realm.to_number(e.to_value())?;
            // Apply the element type's wrap-around before comparing.
            let tmp = super::typed_array::create_typed_array(realm, t.kind, 1, None)?;
            let td = match &realm.obj(tmp).kind {
                ObjectKind::TypedArray(d) => TypedArrayData { kind: d.kind, buffer: d.buffer, byte_offset: d.byte_offset, length: d.length },
                _ => unreachable!(),
            };
            typed_array_set(realm, &td, 0, n);
            ta_get(realm, &td, 0).and_then(|v| v.as_number()) == Some(o)
        }
        _ => false,
    };
    if matches {
        ta_set_value(realm, &t, idx, replacement)?;
    }
    Ok(old)
}

fn atomics_load(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let (t, idx) = atomics_target(realm, args)?;
    Ok(ta_get(realm, &t, idx).unwrap_or(Value::Undefined))
}

fn atomics_store(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let (t, idx) = atomics_target(realm, args)?;
    let v = arg(args, 2);
    let stored = if matches!(t.kind, TypedArrayKind::BigInt64 | TypedArrayKind::BigUint64) {
        let b = super::typed_array::to_bigint(realm, v)?;
        realm.new_bigint(b)
    } else {
        Value::Number(to_integer(realm.to_number(v)?) + 0.0)
    };
    ta_set_value(realm, &t, idx, stored.clone())?;
    Ok(stored)
}

fn atomics_is_lock_free(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let n = realm.to_number(arg(args, 0))?;
    Ok(Value::Bool(matches!(n as i64, 1 | 2 | 4 | 8)))
}

fn atomics_wait(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let (t, idx) = atomics_target(realm, args)?;
    let cur = ta_get(realm, &t, idx).unwrap_or(Value::Undefined);
    let expected = arg(args, 2);
    let eq = match (cur.v(), expected.v()) {
        (V::Number(a), e) => realm.to_number(e.to_value())? == a,
        (V::BigInt(a), e) => super::typed_array::to_bigint(realm, e.to_value())? == a.to_i128_wrapping(),
        _ => false,
    };
    // Single-threaded: nobody can wake us, so a matching wait times out.
    Ok(Value::str(if eq { "timed-out" } else { "not-equal" }))
}

fn atomics_notify(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    atomics_target(realm, args)?;
    Ok(Value::Number(0.0))
}

// --------------------------------------------------------- Promise.try

fn promise_try(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    if !realm.is_constructor(&this) {
        return realm.type_error("Promise.try called on non-constructor");
    }
    let (p, res, rej) = realm.new_promise();
    let f = arg(args, 0);
    let rest: Vec<Value> = args.iter().skip(1).cloned().collect();
    match realm.call(f, Value::Undefined, &rest) {
        Ok(v) => {
            realm.call(Value::Object(res), Value::Undefined, &[v])?;
        }
        Err(e) => {
            realm.call(Value::Object(rej), Value::Undefined, &[e])?;
        }
    }
    // Subclass support: resolve through the receiver when it is not %Promise%.
    if this.as_object() != Some(realm.intrinsics.promise_ctor) {
        let resolve = realm.get_str(this.clone(), "resolve")?;
        return realm.call(resolve, this, &[Value::Object(p)]);
    }
    Ok(Value::Object(p))
}

// ------------------------------------------------- RegExp legacy statics

/// Capture `n` of the last built-in match ("" when absent).
fn static_group(realm: &mut Realm, n: usize) -> JsResult<Value> {
    Ok(match &realm.regexp_last {
        Some((s, caps)) => match caps.get(n) {
            Some(Some((a, b))) => Value::String(s.slice(*a, *b)),
            _ => Value::str(""),
        },
        None => Value::str(""),
    })
}

fn regexp_static_noop_set(_realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(Value::Undefined)
}

fn regexp_static_input(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(match &realm.regexp_last {
        Some((s, _)) => Value::String(s.clone()),
        None => Value::str(""),
    })
}

fn regexp_static_last_match(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    static_group(realm, 0)
}

fn regexp_static_last_paren(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    let n = realm.regexp_last.as_ref().map(|(_, c)| c.len()).unwrap_or(0);
    if n <= 1 {
        return Ok(Value::str(""));
    }
    static_group(realm, n - 1)
}

fn regexp_static_left_context(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(match &realm.regexp_last {
        Some((s, caps)) => match caps.first() {
            Some(Some((a, _))) => Value::String(s.slice(0, *a)),
            _ => Value::str(""),
        },
        None => Value::str(""),
    })
}

fn regexp_static_right_context(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(match &realm.regexp_last {
        Some((s, caps)) => match caps.first() {
            Some(Some((_, b))) => Value::String(s.slice(*b, s.len())),
            _ => Value::str(""),
        },
        None => Value::str(""),
    })
}

macro_rules! static_n {
    ($name:ident, $n:expr) => {
        fn $name(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
            static_group(realm, $n)
        }
    };
}
static_n!(regexp_static_1, 1);
static_n!(regexp_static_2, 2);
static_n!(regexp_static_3, 3);
static_n!(regexp_static_4, 4);
static_n!(regexp_static_5, 5);
static_n!(regexp_static_6, 6);
static_n!(regexp_static_7, 7);
static_n!(regexp_static_8, 8);
static_n!(regexp_static_9, 9);
