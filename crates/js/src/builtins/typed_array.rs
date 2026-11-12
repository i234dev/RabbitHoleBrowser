//! `ArrayBuffer`, `SharedArrayBuffer`, `DataView`, `%TypedArray%` and the
//! eleven concrete typed array classes.

use super::iterator::create_array_iterator;
use crate::value::V;
use super::*;
use crate::realm::{SYM_ITERATOR, SYM_SPECIES, SYM_TO_STRING_TAG};
use std::cell::RefCell;
use std::rc::Rc;

const KINDS: [TypedArrayKind; 11] = [
    TypedArrayKind::Int8,
    TypedArrayKind::Uint8,
    TypedArrayKind::Uint8Clamped,
    TypedArrayKind::Int16,
    TypedArrayKind::Uint16,
    TypedArrayKind::Int32,
    TypedArrayKind::Uint32,
    TypedArrayKind::Float32,
    TypedArrayKind::Float64,
    TypedArrayKind::BigInt64,
    TypedArrayKind::BigUint64,
];

fn proto_key(kind: TypedArrayKind) -> &'static str {
    match kind {
        TypedArrayKind::Int8 => "Int8ArrayPrototype",
        TypedArrayKind::Uint8 => "Uint8ArrayPrototype",
        TypedArrayKind::Uint8Clamped => "Uint8ClampedArrayPrototype",
        TypedArrayKind::Int16 => "Int16ArrayPrototype",
        TypedArrayKind::Uint16 => "Uint16ArrayPrototype",
        TypedArrayKind::Int32 => "Int32ArrayPrototype",
        TypedArrayKind::Uint32 => "Uint32ArrayPrototype",
        TypedArrayKind::Float32 => "Float32ArrayPrototype",
        TypedArrayKind::Float64 => "Float64ArrayPrototype",
        TypedArrayKind::BigInt64 => "BigInt64ArrayPrototype",
        TypedArrayKind::BigUint64 => "BigUint64ArrayPrototype",
    }
}

fn is_bigint_kind(kind: TypedArrayKind) -> bool {
    matches!(kind, TypedArrayKind::BigInt64 | TypedArrayKind::BigUint64)
}

pub fn register(realm: &mut Realm) {
    let op = realm.intrinsics.object_proto;
    let fp = realm.intrinsics.function_proto;

    // ---------------------------------------------------------- ArrayBuffer
    let abp = realm.new_object_with_proto(Some(op));
    realm.intrinsics.others.insert("ArrayBufferPrototype", abp);
    let ab_ctor = define_ctor(realm, "ArrayBuffer", 1, array_buffer_ctor, abp);
    realm.intrinsics.others.insert("ArrayBuffer", ab_ctor);
    realm.define_method(ab_ctor, "isView", 1, array_buffer_is_view);
    realm.define_accessor(ab_ctor, PropertyKey::Symbol(SYM_SPECIES), Some(species), None, CONFIGURABLE);
    realm.define_accessor(abp, "byteLength", Some(array_buffer_byte_length), None, CONFIGURABLE);
    realm.define_accessor(abp, "maxByteLength", Some(array_buffer_byte_length), None, CONFIGURABLE);
    realm.define_accessor(abp, "resizable", Some(return_false), None, CONFIGURABLE);
    realm.define_accessor(abp, "detached", Some(array_buffer_detached), None, CONFIGURABLE);
    realm.define_method(abp, "slice", 2, array_buffer_slice);
    realm.define_property(abp, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("ArrayBuffer"), CONFIGURABLE);

    // Shared buffers behave like ordinary buffers in a single-threaded engine.
    let sabp = realm.new_object_with_proto(Some(op));
    realm.intrinsics.others.insert("SharedArrayBufferPrototype", sabp);
    let sab_ctor = define_ctor(realm, "SharedArrayBuffer", 1, shared_array_buffer_ctor, sabp);
    realm.define_accessor(sab_ctor, PropertyKey::Symbol(SYM_SPECIES), Some(species), None, CONFIGURABLE);
    realm.define_accessor(sabp, "byteLength", Some(array_buffer_byte_length), None, CONFIGURABLE);
    realm.define_method(sabp, "slice", 2, array_buffer_slice);
    realm.define_property(sabp, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("SharedArrayBuffer"), CONFIGURABLE);

    // ------------------------------------------------------------- DataView
    let dvp = realm.new_object_with_proto(Some(op));
    realm.intrinsics.others.insert("DataViewPrototype", dvp);
    define_ctor(realm, "DataView", 1, data_view_ctor, dvp);
    realm.define_accessor(dvp, "buffer", Some(view_buffer), None, CONFIGURABLE);
    realm.define_accessor(dvp, "byteLength", Some(view_byte_length), None, CONFIGURABLE);
    realm.define_accessor(dvp, "byteOffset", Some(view_byte_offset), None, CONFIGURABLE);
    realm.define_property(dvp, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("DataView"), CONFIGURABLE);
    let dv_methods: &[(&str, u32, NativeFn)] = &[
        ("getInt8", 1, dv_get_int8),
        ("getUint8", 1, dv_get_uint8),
        ("getInt16", 1, dv_get_int16),
        ("getUint16", 1, dv_get_uint16),
        ("getInt32", 1, dv_get_int32),
        ("getUint32", 1, dv_get_uint32),
        ("getFloat32", 1, dv_get_float32),
        ("getFloat64", 1, dv_get_float64),
        ("getBigInt64", 1, dv_get_bigint64),
        ("getBigUint64", 1, dv_get_biguint64),
        ("setInt8", 2, dv_set_int8),
        ("setUint8", 2, dv_set_uint8),
        ("setInt16", 2, dv_set_int16),
        ("setUint16", 2, dv_set_uint16),
        ("setInt32", 2, dv_set_int32),
        ("setUint32", 2, dv_set_uint32),
        ("setFloat32", 2, dv_set_float32),
        ("setFloat64", 2, dv_set_float64),
        ("setBigInt64", 2, dv_set_bigint64),
        ("setBigUint64", 2, dv_set_biguint64),
    ];
    for (name, arity, f) in dv_methods {
        realm.define_method(dvp, name, *arity, *f);
    }

    // ---------------------------------------------------------- %TypedArray%
    let tap = realm.new_object_with_proto(Some(op));
    realm.intrinsics.others.insert("TypedArrayPrototype", tap);
    let ta_ctor = realm.new_native_function_full("TypedArray", 0, typed_array_abstract_ctor, None, vec![], true);
    realm.define_property(ta_ctor, "prototype", Value::Object(tap), 0);
    realm.define_property(tap, "constructor", Value::Object(ta_ctor), HIDDEN);
    realm.intrinsics.others.insert("TypedArray", ta_ctor);
    realm.define_method(ta_ctor, "from", 1, typed_array_from);
    realm.define_method(ta_ctor, "of", 0, typed_array_of);
    realm.define_accessor(ta_ctor, PropertyKey::Symbol(SYM_SPECIES), Some(species), None, CONFIGURABLE);
    realm.define_accessor(tap, "buffer", Some(view_buffer), None, CONFIGURABLE);
    realm.define_accessor(tap, "byteLength", Some(ta_byte_length), None, CONFIGURABLE);
    realm.define_accessor(tap, "byteOffset", Some(view_byte_offset), None, CONFIGURABLE);
    realm.define_accessor(tap, "length", Some(ta_length), None, CONFIGURABLE);
    realm.define_accessor(tap, PropertyKey::Symbol(SYM_TO_STRING_TAG), Some(ta_to_string_tag), None, CONFIGURABLE);
    let methods: &[(&str, u32, NativeFn)] = &[
        ("at", 1, ta_at),
        ("copyWithin", 2, ta_copy_within),
        ("entries", 0, ta_entries),
        ("every", 1, ta_every),
        ("fill", 1, ta_fill),
        ("filter", 1, ta_filter),
        ("find", 1, ta_find),
        ("findIndex", 1, ta_find_index),
        ("findLast", 1, ta_find_last),
        ("findLastIndex", 1, ta_find_last_index),
        ("forEach", 1, ta_for_each),
        ("includes", 1, ta_includes),
        ("indexOf", 1, ta_index_of),
        ("join", 1, ta_join),
        ("keys", 0, ta_keys),
        ("lastIndexOf", 1, ta_last_index_of),
        ("map", 1, ta_map),
        ("reduce", 1, ta_reduce),
        ("reduceRight", 1, ta_reduce_right),
        ("reverse", 0, ta_reverse),
        ("set", 1, ta_set),
        ("slice", 2, ta_slice),
        ("some", 1, ta_some),
        ("sort", 1, ta_sort),
        ("subarray", 2, ta_subarray),
        ("toLocaleString", 0, ta_to_locale_string),
        ("toReversed", 0, ta_to_reversed),
        ("toSorted", 1, ta_to_sorted),
        ("with", 2, ta_with),
        ("values", 0, ta_values),
    ];
    for (name, arity, f) in methods {
        realm.define_method(tap, name, *arity, *f);
    }
    let values_fn = realm.obj(tap).props.get(&PropertyKey::str("values")).unwrap().value.clone();
    realm.define_property(tap, PropertyKey::Symbol(SYM_ITERATOR), values_fn, HIDDEN);
    // toString is the same function object as Array.prototype.toString.
    let ap = realm.intrinsics.array_proto;
    if let Some(p) = realm.obj(ap).props.get(&PropertyKey::str("toString")) {
        let f = p.value.clone();
        realm.define_property(tap, "toString", f, HIDDEN);
    }

    // ------------------------------------------------------ concrete kinds
    for kind in KINDS {
        let proto = realm.new_object_with_proto(Some(tap));
        realm.intrinsics.others.insert(proto_key(kind), proto);
        let ctor = realm.new_native_function_full(kind.name(), 3, typed_array_ctor, Some(Rc::new(kind)), vec![], true);
        realm.obj_mut(ctor).set_proto(Some(ta_ctor));
        realm.define_property(ctor, "prototype", Value::Object(proto), 0);
        realm.define_property(proto, "constructor", Value::Object(ctor), HIDDEN);
        let bpe = Value::Number(kind.element_size() as f64);
        realm.define_property(ctor, "BYTES_PER_ELEMENT", bpe.clone(), 0);
        realm.define_property(proto, "BYTES_PER_ELEMENT", bpe, 0);
        realm.define_global(kind.name(), Value::Object(ctor));
        realm.intrinsics.others.insert(kind.name(), ctor);
    }
    let _ = fp;
}

fn species(_realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(this)
}

fn return_false(_realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(false))
}

// ---------------------------------------------------------- ArrayBuffer

thread_local! {
    /// Buffers whose storage was detached (see [`detach_buffer`]).
    static DETACHED: RefCell<Vec<std::rc::Weak<RefCell<Vec<u8>>>>> = const { RefCell::new(Vec::new()) };
}

/// Detach an ArrayBuffer's storage: its bytes move out (returned) and the
/// buffer reports `detached` with length 0. Hosts use it when they replace
/// the storage, e.g. a WebAssembly memory that grew.
pub fn detach_buffer(buf: &Rc<RefCell<Vec<u8>>>) -> Vec<u8> {
    let data = std::mem::take(&mut *buf.borrow_mut());
    DETACHED.with(|d| {
        let mut d = d.borrow_mut();
        d.retain(|w| w.strong_count() > 0);
        d.push(Rc::downgrade(buf));
    });
    data
}

/// Was the buffer detached with [`detach_buffer`]?
pub fn is_detached(buf: &Rc<RefCell<Vec<u8>>>) -> bool {
    DETACHED.with(|d| d.borrow().iter().any(|w| std::ptr::eq(w.as_ptr(), Rc::as_ptr(buf))))
}

fn array_buffer_detached(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (_, b) = this_buffer(realm, &this, "detached")?;
    Ok(Value::Bool(is_detached(&b)))
}

pub fn buffer_of(realm: &Realm, id: ObjectId) -> Option<Rc<RefCell<Vec<u8>>>> {
    match &realm.obj(id).kind {
        ObjectKind::ArrayBuffer(b) => Some(b.clone()),
        _ => None,
    }
}

fn this_buffer(realm: &mut Realm, this: &Value, method: &str) -> JsResult<(ObjectId, Rc<RefCell<Vec<u8>>>)> {
    if let V::Object(id) = this.v() {
        if let Some(b) = buffer_of(realm, id) {
            return Ok((id, b));
        }
    }
    realm.type_error(&format!("Method ArrayBuffer.prototype.{} called on incompatible receiver", method))
}

/// Allocate a new `ArrayBuffer` of `len` zeroed bytes.
pub fn allocate_buffer(realm: &mut Realm, len: usize, proto: Option<ObjectId>) -> ObjectId {
    let proto = proto.unwrap_or_else(|| realm.intrinsics.others["ArrayBufferPrototype"]);
    realm.alloc(Object::new(Some(proto), ObjectKind::ArrayBuffer(Rc::new(RefCell::new(vec![0u8; len])))))
}

fn buffer_ctor_impl(realm: &mut Realm, args: &[Value], proto_key: &str, name: &str) -> JsResult<Value> {
    if !realm.is_construct_call() {
        return realm.type_error(&format!("Constructor {} requires 'new'", name));
    }
    let len = realm.to_index(arg(args, 0)).map_err(|_| realm.throw_range_error("Invalid array buffer length"))?;
    if len > 2usize.pow(31) {
        return realm.range_error("Array buffer allocation failed");
    }
    let default = realm.intrinsics.others[proto_key];
    let proto = realm.proto_from_new_target(default);
    Ok(Value::Object(allocate_buffer(realm, len, Some(proto))))
}

fn array_buffer_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    buffer_ctor_impl(realm, args, "ArrayBufferPrototype", "ArrayBuffer")
}

fn shared_array_buffer_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    buffer_ctor_impl(realm, args, "SharedArrayBufferPrototype", "SharedArrayBuffer")
}

fn array_buffer_is_view(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(match (arg(args, 0)).v() {
        V::Object(id) => matches!(realm.obj(id).kind, ObjectKind::TypedArray(_) | ObjectKind::DataView(_)),
        _ => false,
    }))
}

fn array_buffer_byte_length(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (_, b) = this_buffer(realm, &this, "byteLength")?;
    let n = b.borrow().len();
    Ok(Value::Number(n as f64))
}

fn array_buffer_slice(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, b) = this_buffer(realm, &this, "slice")?;
    let len = b.borrow().len();
    let start = realm.relative_index(arg(args, 0), len, 0)?;
    let end = realm.relative_index(arg(args, 1), len, len)?;
    let new_len = end.saturating_sub(start);
    let default = realm.intrinsics.others["ArrayBuffer"];
    let ctor = realm.species_constructor(id, default)?;
    let out = realm.construct(ctor, &[Value::Number(new_len as f64)], Value::Undefined)?;
    let out_id = match out.v() {
        V::Object(o) if buffer_of(realm, o).is_some() => o,
        _ => return realm.type_error("ArrayBuffer species constructor did not return an ArrayBuffer"),
    };
    if out_id == id {
        return realm.type_error("ArrayBuffer species constructor returned the same buffer");
    }
    let dst = buffer_of(realm, out_id).unwrap();
    if dst.borrow().len() < new_len {
        return realm.type_error("ArrayBuffer species constructor returned a buffer that is too small");
    }
    let src = b.borrow();
    dst.borrow_mut()[..new_len].copy_from_slice(&src[start..start + new_len]);
    Ok(Value::Object(out_id))
}

// ------------------------------------------------------------- DataView

fn view_data(realm: &Realm, id: ObjectId) -> Option<(TypedArrayData, bool)> {
    match &realm.obj(id).kind {
        ObjectKind::TypedArray(t) => Some((copy_data(t), false)),
        ObjectKind::DataView(t) => Some((copy_data(t), true)),
        _ => None,
    }
}

fn copy_data(t: &TypedArrayData) -> TypedArrayData {
    TypedArrayData { kind: t.kind, buffer: t.buffer, byte_offset: t.byte_offset, length: t.length }
}

fn data_view_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if !realm.is_construct_call() {
        return realm.type_error("Constructor DataView requires 'new'");
    }
    let buffer = match (arg(args, 0)).v() {
        V::Object(id) if buffer_of(realm, id).is_some() => id,
        _ => return realm.type_error("First argument to DataView constructor must be an ArrayBuffer"),
    };
    let buf_len = buffer_of(realm, buffer).unwrap().borrow().len();
    let offset = realm.to_index(arg(args, 1))?;
    if offset > buf_len {
        return realm.range_error(&format!("Start offset {} is outside the bounds of the buffer", offset));
    }
    let length = if arg(args, 2).is_undefined() {
        buf_len - offset
    } else {
        let l = realm.to_index(arg(args, 2))?;
        if offset + l > buf_len {
            return realm.range_error(&format!("Invalid DataView length {}", l));
        }
        l
    };
    let default = realm.intrinsics.others["DataViewPrototype"];
    let proto = realm.proto_from_new_target(default);
    let data = TypedArrayData { kind: TypedArrayKind::Uint8, buffer, byte_offset: offset, length };
    Ok(Value::Object(realm.alloc(Object::new(Some(proto), ObjectKind::DataView(Box::new(data))))))
}

fn this_view(realm: &mut Realm, this: &Value, method: &str) -> JsResult<TypedArrayData> {
    if let V::Object(id) = this.v() {
        if let ObjectKind::DataView(t) = &realm.obj(id).kind {
            return Ok(copy_data(t));
        }
    }
    realm.type_error(&format!("Method DataView.prototype.{} called on incompatible receiver", method))
}

fn view_buffer(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    if let V::Object(id) = this.v() {
        if let Some((t, _)) = view_data(realm, id) {
            return Ok(Value::Object(t.buffer));
        }
    }
    realm.type_error("Receiver is not a typed array or DataView")
}

fn view_byte_offset(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    if let V::Object(id) = this.v() {
        if let Some((t, _)) = view_data(realm, id) {
            return Ok(Value::Number(t.byte_offset as f64));
        }
    }
    realm.type_error("Receiver is not a typed array or DataView")
}

fn view_byte_length(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let t = this_view(realm, &this, "byteLength")?;
    Ok(Value::Number(t.length as f64))
}

fn dv_bytes(realm: &mut Realm, this: &Value, args: &[Value], size: usize, method: &str) -> JsResult<(Rc<RefCell<Vec<u8>>>, usize, bool)> {
    let t = this_view(realm, this, method)?;
    let idx = realm.to_index(arg(args, 0))?;
    let little = if method.starts_with("get") { arg(args, 1).truthy() } else { arg(args, 2).truthy() };
    if idx + size > t.length {
        return realm.range_error("Offset is outside the bounds of the DataView");
    }
    let buf = buffer_of(realm, t.buffer).unwrap();
    if t.byte_offset + idx + size > buf.borrow().len() {
        return realm.type_error("Cannot perform DataView access on a detached ArrayBuffer");
    }
    Ok((buf, t.byte_offset + idx, little))
}

macro_rules! dv_get {
    ($name:ident, $ty:ty, $size:expr, $m:expr, $conv:expr) => {
        fn $name(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
            let (buf, off, little) = dv_bytes(realm, &this, args, $size, $m)?;
            let b = buf.borrow();
            let mut bytes = [0u8; $size];
            bytes.copy_from_slice(&b[off..off + $size]);
            let v = if little { <$ty>::from_le_bytes(bytes) } else { <$ty>::from_be_bytes(bytes) };
            Ok($conv(realm, v))
        }
    };
}

macro_rules! dv_set {
    ($name:ident, $ty:ty, $size:expr, $m:expr, $conv:expr) => {
        fn $name(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
            let v: $ty = $conv(realm, arg(args, 1))?;
            let (buf, off, little) = dv_bytes(realm, &this, args, $size, $m)?;
            let bytes = if little { v.to_le_bytes() } else { v.to_be_bytes() };
            buf.borrow_mut()[off..off + $size].copy_from_slice(&bytes);
            Ok(Value::Undefined)
        }
    };
}

fn num<T: Into<f64>>(_r: &mut Realm, v: T) -> Value {
    Value::Number(v.into())
}
fn big_i(r: &mut Realm, v: i64) -> Value {
    r.new_bigint(v as i128)
}
fn big_u(r: &mut Realm, v: u64) -> Value {
    r.new_bigint(v as i128)
}

dv_get!(dv_get_int8, i8, 1, "getInt8", num);
dv_get!(dv_get_uint8, u8, 1, "getUint8", num);
dv_get!(dv_get_int16, i16, 2, "getInt16", num);
dv_get!(dv_get_uint16, u16, 2, "getUint16", num);
dv_get!(dv_get_int32, i32, 4, "getInt32", num);
dv_get!(dv_get_uint32, u32, 4, "getUint32", num);
dv_get!(dv_get_float32, f32, 4, "getFloat32", num);
dv_get!(dv_get_float64, f64, 8, "getFloat64", num);
dv_get!(dv_get_bigint64, i64, 8, "getBigInt64", big_i);
dv_get!(dv_get_biguint64, u64, 8, "getBigUint64", big_u);

fn conv_i8(r: &mut Realm, v: Value) -> JsResult<i8> {
    Ok(to_int32(r.to_number(v)?) as i8)
}
fn conv_u8(r: &mut Realm, v: Value) -> JsResult<u8> {
    Ok(to_int32(r.to_number(v)?) as u8)
}
fn conv_i16(r: &mut Realm, v: Value) -> JsResult<i16> {
    Ok(to_int32(r.to_number(v)?) as i16)
}
fn conv_u16(r: &mut Realm, v: Value) -> JsResult<u16> {
    Ok(to_int32(r.to_number(v)?) as u16)
}
fn conv_i32(r: &mut Realm, v: Value) -> JsResult<i32> {
    Ok(to_int32(r.to_number(v)?))
}
fn conv_u32(r: &mut Realm, v: Value) -> JsResult<u32> {
    Ok(to_uint32(r.to_number(v)?))
}
fn conv_f32(r: &mut Realm, v: Value) -> JsResult<f32> {
    Ok(r.to_number(v)? as f32)
}
fn conv_f64(r: &mut Realm, v: Value) -> JsResult<f64> {
    r.to_number(v)
}
fn conv_i64(r: &mut Realm, v: Value) -> JsResult<i64> {
    Ok(to_bigint(r, v)? as i64)
}
fn conv_u64(r: &mut Realm, v: Value) -> JsResult<u64> {
    Ok(to_bigint(r, v)? as u64)
}

dv_set!(dv_set_int8, i8, 1, "setInt8", conv_i8);
dv_set!(dv_set_uint8, u8, 1, "setUint8", conv_u8);
dv_set!(dv_set_int16, i16, 2, "setInt16", conv_i16);
dv_set!(dv_set_uint16, u16, 2, "setUint16", conv_u16);
dv_set!(dv_set_int32, i32, 4, "setInt32", conv_i32);
dv_set!(dv_set_uint32, u32, 4, "setUint32", conv_u32);
dv_set!(dv_set_float32, f32, 4, "setFloat32", conv_f32);
dv_set!(dv_set_float64, f64, 8, "setFloat64", conv_f64);
dv_set!(dv_set_bigint64, i64, 8, "setBigInt64", conv_i64);
dv_set!(dv_set_biguint64, u64, 8, "setBigUint64", conv_u64);

/// ToBigInt for typed-array stores: BigInt values pass, booleans/strings
/// convert, numbers throw.
pub fn to_bigint(realm: &mut Realm, v: Value) -> JsResult<i128> {
    let p = realm.to_primitive(v, crate::interop::Hint::Number)?;
    match p.v() {
        V::BigInt(b) => Ok(b.to_i128_wrapping()),
        V::Bool(b) => Ok(b as i128),
        V::String(s) => match crate::value::BigInt::parse(s.to_rust_string().trim()) {
            Some(b) => Ok(b.to_i128_wrapping()),
            None => realm.type_error("Cannot convert string to a BigInt"),
        },
        V::Number(_) => realm.type_error("Cannot convert a Number to a BigInt"),
        _ => realm.type_error("Cannot convert value to a BigInt"),
    }
}

// --------------------------------------------------------- typed arrays

/// CanonicalNumericIndexString (ECMA-262 §7.1.21): Some(n) when the key
/// is a string `s` with ToString(ToNumber(s)) == s, or "-0".
pub fn canonical_numeric_index(key: &PropertyKey) -> Option<f64> {
    let s = key.as_str()?;
    // Fast answers: array indices, and names that cannot be numeric
    // (a canonical numeric string starts with a digit, '-', 'I' or 'N').
    if let Some(i) = s.as_array_index() {
        return Some(i as f64);
    }
    match s.as_units().first() {
        Some(&c) if (b'0' as u16..=b'9' as u16).contains(&c) || c == b'-' as u16 || c == b'I' as u16 || c == b'N' as u16 => {}
        _ => return None,
    }
    if s.eq_str("-0") {
        return Some(-0.0);
    }
    let n = crate::value::string_to_number(s);
    if crate::value::number_to_string(n) == s.to_rust_string() {
        Some(n)
    } else {
        None
    }
}

/// Is `n` a valid integer index into a typed array of `length`?
pub fn valid_integer_index(n: f64, length: usize) -> Option<usize> {
    if n.fract() != 0.0 || n.is_sign_negative() && n == 0.0 || n < 0.0 || n >= length as f64 {
        return None;
    }
    Some(n as usize)
}

/// Read element `i` (None when out of range).
pub fn ta_get(realm: &Realm, t: &TypedArrayData, i: usize) -> Option<Value> {
    typed_array_get(realm, t, i)
}

/// Write a JS value into element `i` with the kind's conversion.
pub fn ta_set_value(realm: &mut Realm, t: &TypedArrayData, i: usize, v: Value) -> JsResult<()> {
    if is_bigint_kind(t.kind) {
        let b = to_bigint(realm, v)?;
        if i >= t.length {
            return Ok(());
        }
        if let Some(buf) = buffer_of(realm, t.buffer) {
            let off = t.byte_offset + i * 8;
            let mut bb = buf.borrow_mut();
            if off + 8 <= bb.len() {
                let bytes = match t.kind {
                    TypedArrayKind::BigInt64 => (b as i64).to_le_bytes(),
                    _ => (b as u64).to_le_bytes(),
                };
                bb[off..off + 8].copy_from_slice(&bytes);
            }
        }
        return Ok(());
    }
    let n = realm.to_number(v)?;
    typed_array_set(realm, t, i, n);
    Ok(())
}

fn this_ta(realm: &mut Realm, this: &Value, method: &str) -> JsResult<(ObjectId, TypedArrayData)> {
    if let V::Object(id) = this.v() {
        if let ObjectKind::TypedArray(t) = &realm.obj(id).kind {
            return Ok((id, copy_data(t)));
        }
    }
    realm.type_error(&format!("Method %TypedArray%.prototype.{} called on incompatible receiver", method))
}

/// Create a typed array of `kind` with a fresh buffer.
pub fn create_typed_array(realm: &mut Realm, kind: TypedArrayKind, length: usize, proto: Option<ObjectId>) -> JsResult<ObjectId> {
    if length > 2usize.pow(30) {
        return realm.range_error("Invalid typed array length");
    }
    let buffer = allocate_buffer(realm, length * kind.element_size(), None);
    let proto = proto.unwrap_or_else(|| realm.intrinsics.others[proto_key(kind)]);
    let data = TypedArrayData { kind, buffer, byte_offset: 0, length };
    Ok(realm.alloc(Object::new(Some(proto), ObjectKind::TypedArray(Box::new(data)))))
}

fn typed_array_abstract_ctor(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    realm.type_error("Abstract class TypedArray not directly constructable")
}

fn typed_array_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let callee = realm.current_native().unwrap_or(realm.global);
    let kind = *realm.native_data::<TypedArrayKind>(callee).unwrap_or(Rc::new(TypedArrayKind::Uint8));
    if !realm.is_construct_call() {
        return realm.type_error(&format!("Constructor {} requires 'new'", kind.name()));
    }
    let default = realm.intrinsics.others[proto_key(kind)];
    let proto = realm.proto_from_new_target(default);
    let first = arg(args, 0);
    let id = match first.v() {
        V::Object(src) => {
            let src = src;
            if let Some(buf) = buffer_of(realm, src) {
                // (buffer, byteOffset, length)
                let esize = kind.element_size();
                let offset = realm.to_index(arg(args, 1))?;
                if offset % esize != 0 {
                    return realm.range_error(&format!("start offset of {} should be a multiple of {}", kind.name(), esize));
                }
                let buf_len = buf.borrow().len();
                let length = if arg(args, 2).is_undefined() {
                    if buf_len % esize != 0 {
                        return realm.range_error(&format!("byte length of {} should be a multiple of {}", kind.name(), esize));
                    }
                    if offset > buf_len {
                        return realm.range_error(&format!("Start offset {} is outside the bounds of the buffer", offset));
                    }
                    (buf_len - offset) / esize
                } else {
                    let l = realm.to_index(arg(args, 2))?;
                    if offset + l * esize > buf_len {
                        return realm.range_error(&format!("Invalid typed array length: {}", l));
                    }
                    l
                };
                let data = TypedArrayData { kind, buffer: src, byte_offset: offset, length };
                realm.alloc(Object::new(Some(proto), ObjectKind::TypedArray(Box::new(data))))
            } else if let ObjectKind::TypedArray(st) = &realm.obj(src).kind {
                let st = copy_data(st);
                if is_bigint_kind(st.kind) != is_bigint_kind(kind) {
                    return realm.type_error("Cannot mix BigInt and other types, use explicit conversions");
                }
                let id = create_typed_array(realm, kind, st.length, Some(proto))?;
                let dt = this_ta(realm, &Value::Object(id), "constructor")?.1;
                for i in 0..st.length {
                    let v = ta_get(realm, &st, i).unwrap_or(Value::Undefined);
                    ta_set_value(realm, &dt, i, v)?;
                }
                id
            } else {
                let uses_iterator = realm.get_method(Value::Object(src), PropertyKey::Symbol(SYM_ITERATOR))?.is_some();
                let values = if uses_iterator { realm.iterate_to_vec(Value::Object(src))? } else { realm.create_list_from_array_like(Value::Object(src))? };
                let id = create_typed_array(realm, kind, values.len(), Some(proto))?;
                let dt = this_ta(realm, &Value::Object(id), "constructor")?.1;
                for (i, v) in values.into_iter().enumerate() {
                    ta_set_value(realm, &dt, i, v)?;
                }
                id
            }
        }
        V::Undefined => create_typed_array(realm, kind, 0, Some(proto))?,
        v => {
            let len = realm.to_index(v.to_value()).map_err(|_| realm.throw_range_error("Invalid typed array length"))?;
            create_typed_array(realm, kind, len, Some(proto))?
        }
    };
    Ok(Value::Object(id))
}

/// TypedArraySpeciesCreate with a list of constructor arguments.
fn species_create(realm: &mut Realm, exemplar: ObjectId, kind: TypedArrayKind, args: &[Value]) -> JsResult<(ObjectId, TypedArrayData)> {
    let default = realm.intrinsics.others[kind.name()];
    let ctor = realm.species_constructor(exemplar, default)?;
    let out = realm.construct(ctor, args, Value::Undefined)?;
    match out.v() {
        V::Object(o) => match &realm.obj(o).kind {
            ObjectKind::TypedArray(t) => {
                if is_bigint_kind(t.kind) != is_bigint_kind(kind) {
                    return realm.type_error("Content type of the typed array species does not match");
                }
                let d = copy_data(t);
                if let Some(V::Number(n)) = args.first().map(Value::v) {
                    if args.len() == 1 && (d.length as f64) < n {
                        return realm.type_error("Species constructor returned a typed array that is too short");
                    }
                }
                Ok((o, d))
            }
            _ => realm.type_error("Species constructor did not return a TypedArray"),
        },
        _ => realm.type_error("Species constructor did not return a TypedArray"),
    }
}

/// A typed array whose buffer no longer covers it (its buffer was
/// detached, e.g. a WebAssembly memory that grew) has length 0.
fn ta_out_of_bounds(realm: &Realm, t: &TypedArrayData) -> bool {
    match buffer_of(realm, t.buffer) {
        Some(b) => t.byte_offset + t.length * t.kind.element_size() > b.borrow().len(),
        None => true,
    }
}

fn ta_byte_length(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "byteLength")?;
    if ta_out_of_bounds(realm, &t) {
        return Ok(Value::Number(0.0));
    }
    Ok(Value::Number((t.length * t.kind.element_size()) as f64))
}

fn ta_length(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "length")?;
    if ta_out_of_bounds(realm, &t) {
        return Ok(Value::Number(0.0));
    }
    Ok(Value::Number(t.length as f64))
}

fn ta_to_string_tag(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    if let V::Object(id) = this.v() {
        if let ObjectKind::TypedArray(t) = &realm.obj(id).kind {
            return Ok(Value::str(t.kind.name()));
        }
    }
    Ok(Value::Undefined)
}

fn callback_arg(realm: &mut Realm, args: &[Value], method: &str) -> JsResult<Value> {
    let f = arg(args, 0);
    if !realm.is_callable(&f) {
        let d = realm.describe_for_error(&f);
        return realm.type_error(&format!("{} is not a function (TypedArray.prototype.{})", d, method));
    }
    Ok(f)
}

fn elements(realm: &Realm, t: &TypedArrayData) -> Vec<Value> {
    (0..t.length).map(|i| ta_get(realm, t, i).unwrap_or(Value::Undefined)).collect()
}

fn ta_at(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "at")?;
    let n = realm.to_integer_or_infinity(arg(args, 0))?;
    let idx = if n < 0.0 { t.length as f64 + n } else { n };
    if idx < 0.0 || idx >= t.length as f64 {
        return Ok(Value::Undefined);
    }
    Ok(ta_get(realm, &t, idx as usize).unwrap_or(Value::Undefined))
}

fn ta_copy_within(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "copyWithin")?;
    let len = t.length;
    let target = realm.relative_index(arg(args, 0), len, 0)?;
    let start = realm.relative_index(arg(args, 1), len, 0)?;
    let end = realm.relative_index(arg(args, 2), len, len)?;
    let count = end.saturating_sub(start).min(len - target);
    if count > 0 {
        let es = t.kind.element_size();
        let buf = buffer_of(realm, t.buffer).unwrap();
        let mut b = buf.borrow_mut();
        let base = t.byte_offset;
        b.copy_within(base + start * es..base + (start + count) * es, base + target * es);
    }
    Ok(this)
}

fn ta_entries(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    this_ta(realm, &this, "entries")?;
    Ok(create_array_iterator(realm, this, 2))
}
fn ta_keys(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    this_ta(realm, &this, "keys")?;
    Ok(create_array_iterator(realm, this, 0))
}
fn ta_values(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    this_ta(realm, &this, "values")?;
    Ok(create_array_iterator(realm, this, 1))
}

fn ta_every(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "every")?;
    let f = callback_arg(realm, args, "every")?;
    let this_arg = arg(args, 1);
    for i in 0..t.length {
        let v = ta_get(realm, &t, i).unwrap_or(Value::Undefined);
        if !realm.call(f.clone(), this_arg.clone(), &[v, Value::Number(i as f64), this.clone()])?.truthy() {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

fn ta_some(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "some")?;
    let f = callback_arg(realm, args, "some")?;
    let this_arg = arg(args, 1);
    for i in 0..t.length {
        let v = ta_get(realm, &t, i).unwrap_or(Value::Undefined);
        if realm.call(f.clone(), this_arg.clone(), &[v, Value::Number(i as f64), this.clone()])?.truthy() {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

fn ta_fill(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "fill")?;
    let len = t.length;
    let value = if is_bigint_kind(t.kind) {
        let b = to_bigint(realm, arg(args, 0))?;
        realm.new_bigint(b)
    } else {
        Value::Number(realm.to_number(arg(args, 0))?)
    };
    let start = realm.relative_index(arg(args, 1), len, 0)?;
    let end = realm.relative_index(arg(args, 2), len, len)?;
    if t.kind == TypedArrayKind::Uint8 || t.kind == TypedArrayKind::Int8 {
        if let V::Number(n) = value.v() {
            let byte = to_int32(n) as u8;
            let buf = buffer_of(realm, t.buffer).unwrap();
            let mut b = buf.borrow_mut();
            if start < end {
                b[t.byte_offset + start..t.byte_offset + end].fill(byte);
            }
            return Ok(this);
        }
    }
    for i in start..end {
        ta_set_value(realm, &t, i, value.clone())?;
    }
    Ok(this)
}

fn ta_filter(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, t) = this_ta(realm, &this, "filter")?;
    let f = callback_arg(realm, args, "filter")?;
    let this_arg = arg(args, 1);
    let mut kept = Vec::new();
    for i in 0..t.length {
        let v = ta_get(realm, &t, i).unwrap_or(Value::Undefined);
        if realm.call(f.clone(), this_arg.clone(), &[v.clone(), Value::Number(i as f64), this.clone()])?.truthy() {
            kept.push(v);
        }
    }
    let (out, od) = species_create(realm, id, t.kind, &[Value::Number(kept.len() as f64)])?;
    for (i, v) in kept.into_iter().enumerate() {
        ta_set_value(realm, &od, i, v)?;
    }
    Ok(Value::Object(out))
}

fn find_impl(realm: &mut Realm, this: Value, args: &[Value], method: &str, from_end: bool, want_index: bool) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, method)?;
    let f = callback_arg(realm, args, method)?;
    let this_arg = arg(args, 1);
    let indices: Box<dyn Iterator<Item = usize>> = if from_end { Box::new((0..t.length).rev()) } else { Box::new(0..t.length) };
    for i in indices {
        let v = ta_get(realm, &t, i).unwrap_or(Value::Undefined);
        if realm.call(f.clone(), this_arg.clone(), &[v.clone(), Value::Number(i as f64), this.clone()])?.truthy() {
            return Ok(if want_index { Value::Number(i as f64) } else { v });
        }
    }
    Ok(if want_index { Value::Number(-1.0) } else { Value::Undefined })
}

fn ta_find(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    find_impl(realm, this, args, "find", false, false)
}
fn ta_find_index(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    find_impl(realm, this, args, "findIndex", false, true)
}
fn ta_find_last(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    find_impl(realm, this, args, "findLast", true, false)
}
fn ta_find_last_index(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    find_impl(realm, this, args, "findLastIndex", true, true)
}

fn ta_for_each(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "forEach")?;
    let f = callback_arg(realm, args, "forEach")?;
    let this_arg = arg(args, 1);
    for i in 0..t.length {
        let v = ta_get(realm, &t, i).unwrap_or(Value::Undefined);
        realm.call(f.clone(), this_arg.clone(), &[v, Value::Number(i as f64), this.clone()])?;
    }
    Ok(Value::Undefined)
}

fn ta_includes(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "includes")?;
    if t.length == 0 {
        return Ok(Value::Bool(false));
    }
    let target = arg(args, 0);
    let start = realm.relative_index(arg(args, 1), t.length, 0)?;
    for i in start..t.length {
        let v = ta_get(realm, &t, i).unwrap_or(Value::Undefined);
        if realm.same_value_zero(&v, &target) {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

fn ta_index_of(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "indexOf")?;
    if t.length == 0 {
        return Ok(Value::Number(-1.0));
    }
    let target = arg(args, 0);
    let start = realm.relative_index(arg(args, 1), t.length, 0)?;
    for i in start..t.length {
        let v = ta_get(realm, &t, i).unwrap_or(Value::Undefined);
        if realm.strict_equals(&v, &target) {
            return Ok(Value::Number(i as f64));
        }
    }
    Ok(Value::Number(-1.0))
}

fn ta_last_index_of(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "lastIndexOf")?;
    if t.length == 0 {
        return Ok(Value::Number(-1.0));
    }
    let target = arg(args, 0);
    let from = if args.len() > 1 {
        let n = realm.to_integer_or_infinity(arg(args, 1))?;
        if n < 0.0 {
            let k = t.length as f64 + n;
            if k < 0.0 {
                return Ok(Value::Number(-1.0));
            }
            k as usize
        } else {
            (n as usize).min(t.length - 1)
        }
    } else {
        t.length - 1
    };
    for i in (0..=from).rev() {
        let v = ta_get(realm, &t, i).unwrap_or(Value::Undefined);
        if realm.strict_equals(&v, &target) {
            return Ok(Value::Number(i as f64));
        }
    }
    Ok(Value::Number(-1.0))
}

fn ta_join(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "join")?;
    let sep = match arg(args, 0) {
        Value::Undefined => ",".to_string(),
        v => realm.to_rust_string(v)?,
    };
    let mut out = String::new();
    for i in 0..t.length {
        if i > 0 {
            out.push_str(&sep);
        }
        let v = ta_get(realm, &t, i).unwrap_or(Value::Undefined);
        out.push_str(&realm.to_rust_string(v)?);
    }
    Ok(Value::str(&out))
}

fn ta_to_locale_string(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "toLocaleString")?;
    let mut out = String::new();
    for i in 0..t.length {
        if i > 0 {
            out.push(',');
        }
        let v = ta_get(realm, &t, i).unwrap_or(Value::Undefined);
        let s = realm.invoke(v, "toLocaleString", args)?;
        out.push_str(&realm.to_rust_string(s)?);
    }
    Ok(Value::str(&out))
}

fn ta_map(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, t) = this_ta(realm, &this, "map")?;
    let f = callback_arg(realm, args, "map")?;
    let this_arg = arg(args, 1);
    let (out, od) = species_create(realm, id, t.kind, &[Value::Number(t.length as f64)])?;
    for i in 0..t.length {
        let v = ta_get(realm, &t, i).unwrap_or(Value::Undefined);
        let r = realm.call(f.clone(), this_arg.clone(), &[v, Value::Number(i as f64), this.clone()])?;
        ta_set_value(realm, &od, i, r)?;
    }
    Ok(Value::Object(out))
}

fn reduce_impl(realm: &mut Realm, this: Value, args: &[Value], method: &str, from_end: bool) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, method)?;
    let f = callback_arg(realm, args, method)?;
    let mut indices: Vec<usize> = (0..t.length).collect();
    if from_end {
        indices.reverse();
    }
    let mut iter = indices.into_iter();
    let mut acc = if args.len() >= 2 {
        args[1].clone()
    } else {
        match iter.next() {
            Some(i) => ta_get(realm, &t, i).unwrap_or(Value::Undefined),
            None => return realm.type_error("Reduce of empty array with no initial value"),
        }
    };
    for i in iter {
        let v = ta_get(realm, &t, i).unwrap_or(Value::Undefined);
        acc = realm.call(f.clone(), Value::Undefined, &[acc, v, Value::Number(i as f64), this.clone()])?;
    }
    Ok(acc)
}

fn ta_reduce(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    reduce_impl(realm, this, args, "reduce", false)
}
fn ta_reduce_right(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    reduce_impl(realm, this, args, "reduceRight", true)
}

fn ta_reverse(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "reverse")?;
    let es = t.kind.element_size();
    let buf = buffer_of(realm, t.buffer).unwrap();
    let mut b = buf.borrow_mut();
    let slice = &mut b[t.byte_offset..t.byte_offset + t.length * es];
    let (mut i, mut j) = (0usize, t.length);
    while i + 1 < j {
        j -= 1;
        for k in 0..es {
            slice.swap(i * es + k, j * es + k);
        }
        i += 1;
    }
    drop(b);
    Ok(this)
}

fn ta_to_reversed(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "toReversed")?;
    let out = create_typed_array(realm, t.kind, t.length, None)?;
    let od = this_ta(realm, &Value::Object(out), "toReversed")?.1;
    for i in 0..t.length {
        let v = ta_get(realm, &t, t.length - 1 - i).unwrap_or(Value::Undefined);
        ta_set_value(realm, &od, i, v)?;
    }
    Ok(Value::Object(out))
}

fn ta_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "set")?;
    let offset = realm.to_integer_or_infinity(arg(args, 1))?;
    if offset < 0.0 {
        return realm.range_error("offset is out of bounds");
    }
    let offset = offset as usize;
    let source = arg(args, 0);
    if let V::Object(sid) = source.v() {
        if let ObjectKind::TypedArray(st) = &realm.obj(sid).kind {
            let st = copy_data(st);
            if is_bigint_kind(st.kind) != is_bigint_kind(t.kind) {
                return realm.type_error("Cannot mix BigInt and other types, use explicit conversions");
            }
            if offset + st.length > t.length {
                return realm.range_error("offset is out of bounds");
            }
            if st.kind == t.kind {
                // Same layout: raw byte copy (handles overlapping views).
                let es = t.kind.element_size();
                let sbuf = buffer_of(realm, st.buffer).unwrap();
                let dbuf = buffer_of(realm, t.buffer).unwrap();
                let bytes: Vec<u8> = sbuf.borrow()[st.byte_offset..st.byte_offset + st.length * es].to_vec();
                dbuf.borrow_mut()[t.byte_offset + offset * es..t.byte_offset + (offset + st.length) * es].copy_from_slice(&bytes);
            } else {
                let vals = elements(realm, &st);
                for (i, v) in vals.into_iter().enumerate() {
                    ta_set_value(realm, &t, offset + i, v)?;
                }
            }
            return Ok(Value::Undefined);
        }
    }
    let src = realm.to_object(source)?;
    let len = realm.length_of(Value::Object(src))?;
    if offset + len > t.length {
        return realm.range_error("offset is out of bounds");
    }
    for i in 0..len {
        let v = realm.get(Value::Object(src), PropertyKey::index(i as u32))?;
        ta_set_value(realm, &t, offset + i, v)?;
    }
    Ok(Value::Undefined)
}

fn ta_slice(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, t) = this_ta(realm, &this, "slice")?;
    let len = t.length;
    let start = realm.relative_index(arg(args, 0), len, 0)?;
    let end = realm.relative_index(arg(args, 1), len, len)?;
    let count = end.saturating_sub(start);
    let (out, od) = species_create(realm, id, t.kind, &[Value::Number(count as f64)])?;
    if count > 0 {
        if od.kind == t.kind {
            let es = t.kind.element_size();
            let sbuf = buffer_of(realm, t.buffer).unwrap();
            let dbuf = buffer_of(realm, od.buffer).unwrap();
            let bytes: Vec<u8> = sbuf.borrow()[t.byte_offset + start * es..t.byte_offset + end * es].to_vec();
            dbuf.borrow_mut()[od.byte_offset..od.byte_offset + count * es].copy_from_slice(&bytes);
        } else {
            for i in 0..count {
                let v = ta_get(realm, &t, start + i).unwrap_or(Value::Undefined);
                ta_set_value(realm, &od, i, v)?;
            }
        }
    }
    Ok(Value::Object(out))
}

fn sort_elements(realm: &mut Realm, t: &TypedArrayData, cmp: Value) -> JsResult<Vec<Value>> {
    let items = elements(realm, t);
    if cmp.is_undefined() {
        let mut keyed: Vec<(f64, i128, Value)> = items
            .into_iter()
            .map(|v| match v.v() {
                V::Number(n) => (n, 0, v),
                V::BigInt(b) => (0.0, b.to_i128_wrapping(), v),
                _ => (0.0, 0, v),
            })
            .collect();
        if is_bigint_kind(t.kind) {
            keyed.sort_by(|a, b| a.1.cmp(&b.1));
        } else {
            keyed.sort_by(|a, b| {
                // NaN last; -0 before +0.
                match (a.0.is_nan(), b.0.is_nan()) {
                    (true, true) => std::cmp::Ordering::Equal,
                    (true, false) => std::cmp::Ordering::Greater,
                    (false, true) => std::cmp::Ordering::Less,
                    _ => a.0.partial_cmp(&b.0).unwrap().then_with(|| {
                        if a.0 == 0.0 && b.0 == 0.0 {
                            a.0.is_sign_negative().cmp(&b.0.is_sign_negative()).reverse()
                        } else {
                            std::cmp::Ordering::Equal
                        }
                    }),
                }
            });
        }
        return Ok(keyed.into_iter().map(|(_, _, v)| v).collect());
    }
    if !realm.is_callable(&cmp) {
        return realm.type_error("The comparison function must be either a function or undefined");
    }
    merge_sort(realm, items, &cmp)
}

/// Stable merge sort driven by a JS comparator.
fn merge_sort(realm: &mut Realm, items: Vec<Value>, cmp: &Value) -> JsResult<Vec<Value>> {
    if items.len() <= 1 {
        return Ok(items);
    }
    let mid = items.len() / 2;
    let right = items[mid..].to_vec();
    let left = items[..mid].to_vec();
    let left = merge_sort(realm, left, cmp)?;
    let right = merge_sort(realm, right, cmp)?;
    let mut out = Vec::with_capacity(left.len() + right.len());
    let (mut i, mut j) = (0, 0);
    while i < left.len() && j < right.len() {
        let r = realm.call(cmp.clone(), Value::Undefined, &[left[i].clone(), right[j].clone()])?;
        let n = realm.to_number(r)?;
        if n > 0.0 {
            out.push(right[j].clone());
            j += 1;
        } else {
            out.push(left[i].clone());
            i += 1;
        }
    }
    out.extend_from_slice(&left[i..]);
    out.extend_from_slice(&right[j..]);
    Ok(out)
}

fn ta_sort(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "sort")?;
    let sorted = sort_elements(realm, &t, arg(args, 0))?;
    for (i, v) in sorted.into_iter().enumerate() {
        ta_set_value(realm, &t, i, v)?;
    }
    Ok(this)
}

fn ta_to_sorted(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "toSorted")?;
    let cmp = arg(args, 0);
    if !cmp.is_undefined() && !realm.is_callable(&cmp) {
        return realm.type_error("The comparison function must be either a function or undefined");
    }
    let sorted = sort_elements(realm, &t, cmp)?;
    let out = create_typed_array(realm, t.kind, t.length, None)?;
    let od = this_ta(realm, &Value::Object(out), "toSorted")?.1;
    for (i, v) in sorted.into_iter().enumerate() {
        ta_set_value(realm, &od, i, v)?;
    }
    Ok(Value::Object(out))
}

fn ta_subarray(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, t) = this_ta(realm, &this, "subarray")?;
    let len = t.length;
    let start = realm.relative_index(arg(args, 0), len, 0)?;
    let end = realm.relative_index(arg(args, 1), len, len)?;
    let count = end.saturating_sub(start);
    let es = t.kind.element_size();
    let args = [Value::Object(t.buffer), Value::Number((t.byte_offset + start * es) as f64), Value::Number(count as f64)];
    let (out, _) = species_create(realm, id, t.kind, &args)?;
    Ok(Value::Object(out))
}

fn ta_with(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "with")?;
    let rel = realm.to_integer_or_infinity(arg(args, 0))?;
    let idx = if rel < 0.0 { t.length as f64 + rel } else { rel };
    let value = if is_bigint_kind(t.kind) {
        let b = to_bigint(realm, arg(args, 1))?;
        realm.new_bigint(b)
    } else {
        Value::Number(realm.to_number(arg(args, 1))?)
    };
    if idx < 0.0 || idx >= t.length as f64 {
        return realm.range_error("Invalid typed array index");
    }
    let out = create_typed_array(realm, t.kind, t.length, None)?;
    let od = this_ta(realm, &Value::Object(out), "with")?.1;
    for i in 0..t.length {
        let v = if i == idx as usize { value.clone() } else { ta_get(realm, &t, i).unwrap_or(Value::Undefined) };
        ta_set_value(realm, &od, i, v)?;
    }
    Ok(Value::Object(out))
}

/// `%TypedArray%.from(source, mapFn, thisArg)` — `this` is the constructor.
fn typed_array_from(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    if !realm.is_constructor(&this) {
        return realm.type_error("TypedArray.from: this is not a constructor");
    }
    let map_fn = arg(args, 1);
    if !map_fn.is_undefined() && !realm.is_callable(&map_fn) {
        return realm.type_error("TypedArray.from: mapFn is not a function");
    }
    let this_arg = arg(args, 2);
    let source = arg(args, 0);
    let uses_iterator = realm.get_method(source.clone(), PropertyKey::Symbol(SYM_ITERATOR))?.is_some();
    let values = if uses_iterator {
        realm.iterate_to_vec(source)?
    } else {
        let o = realm.to_object(source)?;
        realm.create_list_from_array_like(Value::Object(o))?
    };
    let out = realm.construct(this, &[Value::Number(values.len() as f64)], Value::Undefined)?;
    let (oid, od) = this_ta(realm, &out, "from")?;
    for (i, v) in values.into_iter().enumerate() {
        let v = if map_fn.is_undefined() { v } else { realm.call(map_fn.clone(), this_arg.clone(), &[v, Value::Number(i as f64)])? };
        ta_set_value(realm, &od, i, v)?;
    }
    Ok(Value::Object(oid))
}

fn typed_array_of(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    if !realm.is_constructor(&this) {
        return realm.type_error("TypedArray.of: this is not a constructor");
    }
    let out = realm.construct(this, &[Value::Number(args.len() as f64)], Value::Undefined)?;
    let (oid, od) = this_ta(realm, &out, "of")?;
    for (i, v) in args.iter().enumerate() {
        ta_set_value(realm, &od, i, v.clone())?;
    }
    Ok(Value::Object(oid))
}
