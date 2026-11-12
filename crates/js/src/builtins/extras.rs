//! Remaining ES2023 pieces: `WeakRef`, `FinalizationRegistry`, `Atomics`,
//! `Promise.try`, legacy `RegExp` statics (`RegExp.$1`, `RegExp.lastMatch`).
//!
//! `WeakRef` holds its target strongly (no ephemerons in the collector), so
//! `deref()` always returns the object; `FinalizationRegistry` callbacks
//! never run.

use crate::value::V;
use super::*;
use crate::realm::SYM_TO_STRING_TAG;

pub fn register(realm: &mut Realm) {
    let op = realm.intrinsics.object_proto;

    // ---------------------------------------------------------- WeakRef
    let target_sym = realm.new_private_symbol(JsStr::new("WeakRefTarget"));
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

    // Atomics: see typed_array.rs.

    // ------------------------------------------------------ Promise.try
    let pc = realm.intrinsics.promise_ctor;
    realm.define_method(pc, "try", 1, promise_try);

    // --------------------------------------------------- Array.fromAsync
    // The steps run as a JavaScript async function (they await); a native
    // wrapper gives Array.fromAsync its built-in shape.
    if let Ok(factory) = realm.eval(ARRAY_FROM_ASYNC, "<builtins/Array.fromAsync>") {
        let helpers: [(&str, u32, NativeFn); 4] = [("isConstructor", 1, from_async_is_constructor), ("defineData", 3, from_async_define_data), ("lengthOf", 1, from_async_length_of), ("arrayCreate", 1, from_async_array_create)];
        let hv: Vec<Value> = helpers.iter().map(|(n, a, f)| Value::Object(realm.new_native_function(n, *a, *f))).collect();
        if let Some(imp) = realm.call(factory, Value::Undefined, &hv).ok().and_then(|v| v.as_object()) {
            realm.intrinsics.others.insert("ArrayFromAsyncImpl", imp);
            let ac = realm.intrinsics.array_ctor;
            realm.define_method(ac, "fromAsync", 1, array_from_async);
        }
    }

    // ------------------------------------------------- RegExp legacy statics
    if let Some(rc) = realm.intrinsics.others.get("RegExp").copied().or_else(|| {
        let g = realm.global;
        realm.obj(g).props.get(&PropertyKey::str("RegExp")).and_then(|p| p.value.as_object())
    }) {
        realm.define_accessor(rc, "input", Some(regexp_static_input), Some(regexp_static_input_set), CONFIGURABLE);
        realm.define_accessor(rc, "$_", Some(regexp_static_input), Some(regexp_static_input_set), CONFIGURABLE);
        for (name, f) in [
            ("lastMatch", regexp_static_last_match as NativeFn),
            ("$&", regexp_static_last_match),
            ("lastParen", regexp_static_last_paren),
            ("$+", regexp_static_last_paren),
            ("leftContext", regexp_static_left_context),
            ("$`", regexp_static_left_context),
            ("rightContext", regexp_static_right_context),
            ("$'", regexp_static_right_context),
        ] {
            realm.define_accessor(rc, name, Some(f), None, CONFIGURABLE);
        }
        realm.define_accessor(rc, "$1", Some(regexp_static_1), None, CONFIGURABLE);
        realm.define_accessor(rc, "$2", Some(regexp_static_2), None, CONFIGURABLE);
        realm.define_accessor(rc, "$3", Some(regexp_static_3), None, CONFIGURABLE);
        realm.define_accessor(rc, "$4", Some(regexp_static_4), None, CONFIGURABLE);
        realm.define_accessor(rc, "$5", Some(regexp_static_5), None, CONFIGURABLE);
        realm.define_accessor(rc, "$6", Some(regexp_static_6), None, CONFIGURABLE);
        realm.define_accessor(rc, "$7", Some(regexp_static_7), None, CONFIGURABLE);
        realm.define_accessor(rc, "$8", Some(regexp_static_8), None, CONFIGURABLE);
        realm.define_accessor(rc, "$9", Some(regexp_static_9), None, CONFIGURABLE);
        realm.intrinsics.others.insert("RegExp", rc);
    }
}

const ARRAY_FROM_ASYNC: &str = r#"
(function (isConstructor, defineData, lengthOf, arrayCreate) {
  "use strict";
  const ReflectApply = Reflect.apply, ReflectConstruct = Reflect.construct;
  const asyncIter = Symbol.asyncIterator, syncIter = Symbol.iterator;
  const TypeErr = TypeError, Obj = Object;
  const isObject = (v) => (typeof v === "object" && v !== null) || typeof v === "function";
  const getMethod = (o, k) => {
    const m = o[k];
    if (m === undefined || m === null) return undefined;
    if (typeof m !== "function") throw new TypeErr("Array.fromAsync: iterator method is not callable");
    return m;
  };
  // AsyncIteratorClose(iteratorRecord, throw completion): the original error wins.
  const closeAfterError = async (it, err) => {
    try {
      const ret = it["return"];
      if (ret !== undefined && ret !== null) await ReflectApply(ret, it, []);
    } catch (_) {}
    throw err;
  };
  const syncCloseAfterError = (it, err) => {
    try {
      const ret = it["return"];
      if (ret !== undefined && ret !== null) ReflectApply(ret, it, []);
    } catch (_) {}
    throw err;
  };
  return async function fromAsync(C, asyncItems, mapfn, thisArg) {
    let mapping = false;
    if (mapfn !== undefined) {
      if (typeof mapfn !== "function") throw new TypeErr("Array.fromAsync: mapper is not a function");
      mapping = true;
    }
    if (asyncItems === undefined || asyncItems === null) throw new TypeErr("Array.fromAsync requires an iterable or array-like");
    const usingAsync = getMethod(asyncItems, asyncIter);
    const usingSync = usingAsync === undefined ? getMethod(asyncItems, syncIter) : undefined;
    if (usingAsync !== undefined || usingSync !== undefined) {
      const isAsync = usingAsync !== undefined;
      const it = ReflectApply(isAsync ? usingAsync : usingSync, asyncItems, []);
      if (!isObject(it)) throw new TypeErr("Result of the Symbol.iterator method is not an object");
      const next = it.next;
      const A = isConstructor(C) ? ReflectConstruct(C, []) : arrayCreate(0);
      let k = 0;
      for (;;) {
        if (k >= 9007199254740991) {
          const e = new TypeErr("Array.fromAsync: too many elements");
          if (isAsync) await closeAfterError(it, e); else syncCloseAfterError(it, e);
        }
        let result = ReflectApply(next, it, []);
        let value;
        if (isAsync) {
          result = await result;
          if (!isObject(result)) throw new TypeErr("Iterator result is not an object");
          if (result.done) { A.length = k; return A; }
          value = result.value;
        } else {
          if (!isObject(result)) throw new TypeErr("Iterator result is not an object");
          const done = !!result.done;
          value = result.value;
          if (done) { await value; A.length = k; return A; }
          try { value = await value; } catch (e) { syncCloseAfterError(it, e); }
        }
        let mapped = value;
        if (mapping) {
          try {
            mapped = await ReflectApply(mapfn, thisArg, [value, k]);
          } catch (e) {
            if (isAsync) await closeAfterError(it, e); else syncCloseAfterError(it, e);
          }
        }
        try {
          defineData(A, k, mapped);
        } catch (e) {
          if (isAsync) await closeAfterError(it, e); else syncCloseAfterError(it, e);
        }
        k++;
      }
    }
    const arrayLike = Obj(asyncItems);
    const len = lengthOf(arrayLike);
    const A = isConstructor(C) ? ReflectConstruct(C, [len]) : arrayCreate(len);
    for (let k = 0; k < len; k++) {
      let v = await arrayLike[k];
      if (mapping) v = await ReflectApply(mapfn, thisArg, [v, k]);
      defineData(A, k, v);
    }
    A.length = len;
    return A;
  };
})
"#;

fn array_from_async(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let imp = realm.intrinsics.others["ArrayFromAsyncImpl"];
    let call_args = [this, arg(args, 0), arg(args, 1), arg(args, 2)];
    realm.call(Value::Object(imp), Value::Undefined, &call_args)
}
fn from_async_is_constructor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(realm.is_constructor(&arg(args, 0))))
}
fn from_async_define_data(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(o) = arg(args, 0).as_object() else { return realm.type_error("not an object") };
    let k = realm.to_number(arg(args, 1))?;
    super::array::create_data_property_or_throw(realm, o, k, arg(args, 2))?;
    Ok(Value::Undefined)
}
fn from_async_length_of(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let n = realm.length_of(arg(args, 0))?;
    Ok(Value::Number(n as f64))
}
fn from_async_array_create(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let n = realm.to_number(arg(args, 0))?;
    if n > u32::MAX as f64 {
        return realm.range_error("Invalid array length");
    }
    let proto = realm.intrinsics.array_proto;
    Ok(Value::Object(realm.alloc(Object::new(Some(proto), ObjectKind::Array { elements: Vec::new(), length: n as u32 }))))
}

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
    // Registered unregister tokens (cleanup callbacks never run).
    let cells = realm.new_array(Vec::new());
    let key = weak_ref_key(realm);
    realm.define_property(id, key, Value::Object(cells), 0);
    Ok(Value::Object(id))
}

fn registry_cells(realm: &Realm, id: ObjectId) -> Option<ObjectId> {
    let key = weak_ref_key(realm);
    realm.obj(id).props.get(&key).and_then(|p| p.value.as_object())
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
    if !token.is_undefined() {
        if let Some(cells) = registry_cells(realm, id) {
            if let ObjectKind::Array { elements, length } = &mut realm.obj_mut(cells).kind {
                elements.push(token);
                *length = elements.len() as u32;
            }
        }
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
    let Some(cells) = registry_cells(realm, id) else { return Ok(Value::Bool(false)) };
    let tokens = match &realm.obj(cells).kind {
        ObjectKind::Array { elements, .. } => elements.clone(),
        _ => Vec::new(),
    };
    let kept: Vec<Value> = tokens.iter().filter(|t| !realm.same_value(t, &token)).cloned().collect();
    let removed = kept.len() != tokens.len();
    if let ObjectKind::Array { elements, length } = &mut realm.obj_mut(cells).kind {
        *elements = kept;
        *length = elements.len() as u32;
    }
    Ok(Value::Bool(removed))
}

// --------------------------------------------------------- Promise.try

fn promise_try(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    if !this.is_object() {
        return realm.type_error("Promise.try called on non-object");
    }
    let (p, res, rej) = super::promise::new_capability_with_ctor(realm, this)?;
    let f = arg(args, 0);
    let rest: Vec<Value> = args.iter().skip(1).cloned().collect();
    match realm.call(f, Value::Undefined, &rest) {
        Ok(v) => {
            realm.call(res, Value::Undefined, &[v])?;
        }
        Err(e) => {
            realm.call(rej, Value::Undefined, &[e])?;
        }
    }
    Ok(p)
}

// ------------------------------------------------- RegExp legacy statics

/// GetLegacyRegExpStaticProperty: the receiver must be %RegExp% itself.
fn legacy_receiver(realm: &mut Realm, this: &Value) -> JsResult<()> {
    if this.as_object() != realm.intrinsics.others.get("RegExp").copied() {
        return realm.type_error("RegExp legacy static accessor called on an incompatible receiver");
    }
    Ok(())
}

/// Capture `n` of the last built-in match ("" when absent).
fn static_group(realm: &mut Realm, this: &Value, n: usize) -> JsResult<Value> {
    legacy_receiver(realm, this)?;
    Ok(match &realm.regexp_last {
        Some((s, caps)) => match caps.get(n) {
            Some(Some((a, b))) => Value::String(s.slice(*a, *b)),
            _ => Value::str(""),
        },
        None => Value::str(""),
    })
}

fn regexp_static_input_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    legacy_receiver(realm, &this)?;
    realm.to_string(args.first().cloned().unwrap_or(Value::Undefined))?;
    Ok(Value::Undefined)
}

fn regexp_static_input(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    legacy_receiver(realm, &this)?;
    Ok(match &realm.regexp_last {
        Some((s, _)) => Value::String(s.clone()),
        None => Value::str(""),
    })
}

fn regexp_static_last_match(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    static_group(realm, &this, 0)
}

fn regexp_static_last_paren(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    legacy_receiver(realm, &this)?;
    let n = realm.regexp_last.as_ref().map(|(_, c)| c.len()).unwrap_or(0);
    if n <= 1 {
        return Ok(Value::str(""));
    }
    static_group(realm, &this, n - 1)
}

fn regexp_static_left_context(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    legacy_receiver(realm, &this)?;
    Ok(match &realm.regexp_last {
        Some((s, caps)) => match caps.first() {
            Some(Some((a, _))) => Value::String(s.slice(0, *a)),
            _ => Value::str(""),
        },
        None => Value::str(""),
    })
}

fn regexp_static_right_context(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    legacy_receiver(realm, &this)?;
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
        fn $name(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
            static_group(realm, &this, $n)
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
