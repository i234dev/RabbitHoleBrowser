//! The standard library. Each module registers its intrinsics on the realm.
//!
//! Extension point for other modules: write `pub fn register(realm: &mut Realm)`
//! and call it from [`register_builtins`].

pub mod array;
pub mod collections;
pub mod date;
pub mod error;
pub mod extras;
pub mod function;
pub mod global;
pub mod intl;
pub mod iterator;
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
use crate::interop::JsResult;
use crate::realm::Realm;
use crate::value::*;

pub fn register_builtins(realm: &mut Realm) {
    function::register(realm);
    object::register(realm);
    symbol::register(realm);
    error::register(realm);
    iterator::register(realm);
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
}

/// `args[i]` or undefined.
#[inline]
pub fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Undefined)
}

/// Require `this` to be an object.
pub fn this_object(realm: &mut Realm, this: &Value, method: &str) -> JsResult<ObjectId> {
    match this {
        Value::Object(id) => Ok(*id),
        _ => realm.type_error(&format!("{} called on non-object", method)),
    }
}

/// Register a global constructor with the given prototype object.
pub fn define_ctor(realm: &mut Realm, name: &str, arity: u32, func: NativeFn, proto: ObjectId) -> ObjectId {
    let c = realm.new_native_constructor(name, arity, func, proto);
    realm.define_global(name, Value::Object(c));
    c
}

/// Read element `index` of a typed array (None when out of range).
pub fn typed_array_get(realm: &Realm, t: &TypedArrayData, index: usize) -> Option<Value> {
    if index >= t.length {
        return None;
    }
    let buf = match &realm.obj(t.buffer).kind {
        ObjectKind::ArrayBuffer(b) => b.clone(),
        _ => return None,
    };
    let b = buf.borrow();
    let size = t.kind.element_size();
    let off = t.byte_offset + index * size;
    if off + size > b.len() {
        return None;
    }
    let bytes = &b[off..off + size];
    Some(match t.kind {
        TypedArrayKind::Int8 => Value::Number(bytes[0] as i8 as f64),
        TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped => Value::Number(bytes[0] as f64),
        TypedArrayKind::Int16 => Value::Number(i16::from_le_bytes([bytes[0], bytes[1]]) as f64),
        TypedArrayKind::Uint16 => Value::Number(u16::from_le_bytes([bytes[0], bytes[1]]) as f64),
        TypedArrayKind::Int32 => Value::Number(i32::from_le_bytes(bytes.try_into().unwrap()) as f64),
        TypedArrayKind::Uint32 => Value::Number(u32::from_le_bytes(bytes.try_into().unwrap()) as f64),
        TypedArrayKind::Float32 => Value::Number(f32::from_le_bytes(bytes.try_into().unwrap()) as f64),
        TypedArrayKind::Float64 => Value::Number(f64::from_le_bytes(bytes.try_into().unwrap())),
        TypedArrayKind::BigInt64 => Value::BigInt(std::rc::Rc::new(BigInt(i64::from_le_bytes(bytes.try_into().unwrap()) as i128))),
        TypedArrayKind::BigUint64 => Value::BigInt(std::rc::Rc::new(BigInt(u64::from_le_bytes(bytes.try_into().unwrap()) as i128))),
    })
}

/// Write a number into element `index` of a typed array (ignored when out of range).
pub fn typed_array_set(realm: &Realm, t: &TypedArrayData, index: usize, n: f64) {
    if index >= t.length {
        return;
    }
    let buf = match &realm.obj(t.buffer).kind {
        ObjectKind::ArrayBuffer(b) => b.clone(),
        _ => return,
    };
    let mut b = buf.borrow_mut();
    let size = t.kind.element_size();
    let off = t.byte_offset + index * size;
    if off + size > b.len() {
        return;
    }
    let dst = &mut b[off..off + size];
    match t.kind {
        TypedArrayKind::Int8 => dst[0] = to_int32(n) as i8 as u8,
        TypedArrayKind::Uint8 => dst[0] = to_int32(n) as u8,
        TypedArrayKind::Uint8Clamped => dst[0] = if n.is_nan() { 0 } else { n.clamp(0.0, 255.0).round_ties_even() as u8 },
        TypedArrayKind::Int16 => dst.copy_from_slice(&(to_int32(n) as i16).to_le_bytes()),
        TypedArrayKind::Uint16 => dst.copy_from_slice(&(to_int32(n) as u16).to_le_bytes()),
        TypedArrayKind::Int32 => dst.copy_from_slice(&to_int32(n).to_le_bytes()),
        TypedArrayKind::Uint32 => dst.copy_from_slice(&to_uint32(n).to_le_bytes()),
        TypedArrayKind::Float32 => dst.copy_from_slice(&(n as f32).to_le_bytes()),
        TypedArrayKind::Float64 => dst.copy_from_slice(&n.to_le_bytes()),
        TypedArrayKind::BigInt64 => dst.copy_from_slice(&(n as i64).to_le_bytes()),
        TypedArrayKind::BigUint64 => dst.copy_from_slice(&(n as u64).to_le_bytes()),
    }
}

/// Helper for `Object.prototype.toString`-style class names.
pub fn class_of(realm: &Realm, id: ObjectId) -> &'static str {
    let o = realm.obj(id);
    if let Some(c) = o.class_name {
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
        ObjectKind::Arguments => "Arguments",
        _ => "Object",
    }
}
