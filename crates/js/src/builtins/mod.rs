//! The standard library. Each module registers its intrinsics on the realm.
//!
//! Extension point for other modules: write `pub fn register(realm: &mut Realm)`
//! and call it from [`register_builtins`].

pub mod array;
pub mod collections;
pub mod date;
pub mod disposable;
pub mod error;
pub mod extras;
pub mod function;
pub mod global;
pub mod intl;
pub mod iterator;
pub mod iterator_helpers;
pub mod json;
pub mod math;
pub mod number;
pub mod object;
pub mod promise;
pub mod reflect;
pub mod regexp;
pub mod string;
pub mod symbol;
pub mod typed_array;

use crate::heap::*;
use crate::value::V;
use crate::interop::JsResult;
use crate::realm::Realm;
use crate::value::*;

pub fn register_builtins(realm: &mut Realm) {
    function::register(realm);
    object::register(realm);
    symbol::register(realm);
    error::register(realm);
    iterator::register(realm);
    iterator_helpers::register(realm);
    array::register(realm);
    string::register(realm);
    number::register(realm);
    math::register(realm);
    json::register(realm);
    promise::register(realm);
    reflect::register(realm);
    regexp::register(realm);
    global::register(realm);
    date::register(realm);
    collections::register(realm);
    typed_array::register(realm);
    intl::register(realm);
    extras::register(realm);
    disposable::register(realm);
}

/// `args[i]` or undefined.
#[inline]
pub fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Undefined)
}

/// Require `this` to be an object.
pub fn this_object(realm: &mut Realm, this: &Value, method: &str) -> JsResult<ObjectId> {
    match this.v() {
        V::Object(id) => Ok(id),
        _ => realm.type_error(&format!("{} called on non-object", method)),
    }
}

/// Register a global constructor with the given prototype object.
pub fn define_ctor(realm: &mut Realm, name: &str, arity: u32, func: NativeFn, proto: ObjectId) -> ObjectId {
    let c = realm.new_native_constructor(name, arity, func, proto);
    realm.define_global(name, Value::Object(c));
    c
}

/// Read element `index` of a typed array (None when out of range, out of
/// bounds or detached).
#[inline]
pub fn typed_array_get(realm: &Realm, t: &TypedArrayData, index: usize) -> Option<Value> {
    typed_array::get_element(realm, t, index)
}

/// Write a number into element `index` of a typed array (ignored when out
/// of range, out of bounds, detached or immutable).
#[inline]
pub fn typed_array_set(realm: &Realm, t: &TypedArrayData, index: usize, n: f64) {
    typed_array::set_element(realm, t, index, typed_array::Num::F(n));
}

/// Helper for `Object.prototype.toString`-style class names.
pub fn class_of(realm: &Realm, id: ObjectId) -> &'static str {
    let o = realm.obj(id);
    if let Some(c) = o.class_name() {
        return c;
    }
    match &o.kind {
        ObjectKind::Array { .. } => "Array",
        ObjectKind::Function(_) | ObjectKind::Native(_) | ObjectKind::Bound(_) => "Function",
        ObjectKind::Error => "Error",
        ObjectKind::Boolean(_) => "Boolean",
        ObjectKind::Number(_) => "Number",
        ObjectKind::String(_) => "String",
        ObjectKind::Date(_) => "Date",
        ObjectKind::RegExp(_) => "RegExp",
        ObjectKind::Arguments | ObjectKind::MappedArguments(_) => "Arguments",
        _ => "Object",
    }
}
