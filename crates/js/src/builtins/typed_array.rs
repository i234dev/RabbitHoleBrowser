//! `ArrayBuffer`, `SharedArrayBuffer`, `DataView`, `%TypedArray%`, the
//! twelve concrete typed array classes, `Uint8Array` base64/hex and
//! `Atomics`.
//!
//! Buffers may be resizable (`maxByteLength`), growable (shared), detached
//! or immutable; views may track the length of a resizable buffer
//! (`TypedArrayData::fixed_length == None`) and go out of bounds when it
//! shrinks. Element reads of an out-of-bounds or detached view give
//! `undefined` and writes are ignored.

use super::iterator::create_array_iterator;
use super::*;
use crate::realm::{SYM_ITERATOR, SYM_SPECIES, SYM_TO_STRING_TAG};
use crate::value::V;
use std::cell::RefCell;
use std::rc::Rc;

const KINDS: [TypedArrayKind; 12] = [
    TypedArrayKind::Int8,
    TypedArrayKind::Uint8,
    TypedArrayKind::Uint8Clamped,
    TypedArrayKind::Int16,
    TypedArrayKind::Uint16,
    TypedArrayKind::Int32,
    TypedArrayKind::Uint32,
    TypedArrayKind::Float16,
    TypedArrayKind::Float32,
    TypedArrayKind::Float64,
    TypedArrayKind::BigInt64,
    TypedArrayKind::BigUint64,
];

/// Largest buffer the engine allocates (bigger requests throw RangeError).
const MAX_BYTE_LENGTH: usize = 1 << 32;
/// Largest `maxByteLength` accepted for a resizable buffer (never reserved
/// up front; growing past `MAX_BYTE_LENGTH` still fails).
const MAX_MAX_BYTE_LENGTH: usize = 1 << 40;

fn proto_key(kind: TypedArrayKind) -> &'static str {
    match kind {
        TypedArrayKind::Int8 => "Int8ArrayPrototype",
        TypedArrayKind::Uint8 => "Uint8ArrayPrototype",
        TypedArrayKind::Uint8Clamped => "Uint8ClampedArrayPrototype",
        TypedArrayKind::Int16 => "Int16ArrayPrototype",
        TypedArrayKind::Uint16 => "Uint16ArrayPrototype",
        TypedArrayKind::Int32 => "Int32ArrayPrototype",
        TypedArrayKind::Uint32 => "Uint32ArrayPrototype",
        TypedArrayKind::Float16 => "Float16ArrayPrototype",
        TypedArrayKind::Float32 => "Float32ArrayPrototype",
        TypedArrayKind::Float64 => "Float64ArrayPrototype",
        TypedArrayKind::BigInt64 => "BigInt64ArrayPrototype",
        TypedArrayKind::BigUint64 => "BigUint64ArrayPrototype",
    }
}

fn is_bigint_kind(kind: TypedArrayKind) -> bool {
    kind.is_bigint()
}

pub fn register(realm: &mut Realm) {
    let op = realm.intrinsics.object_proto;

    // ---------------------------------------------------------- ArrayBuffer
    let abp = realm.new_object_with_proto(Some(op));
    realm.intrinsics.others.insert("ArrayBufferPrototype", abp);
    let ab_ctor = define_ctor(realm, "ArrayBuffer", 1, array_buffer_ctor, abp);
    realm.intrinsics.others.insert("ArrayBuffer", ab_ctor);
    realm.define_method(ab_ctor, "isView", 1, array_buffer_is_view);
    realm.define_accessor(ab_ctor, PropertyKey::Symbol(SYM_SPECIES), Some(species), None, CONFIGURABLE);
    realm.define_accessor(abp, "byteLength", Some(ab_byte_length), None, CONFIGURABLE);
    realm.define_accessor(abp, "maxByteLength", Some(ab_max_byte_length), None, CONFIGURABLE);
    realm.define_accessor(abp, "resizable", Some(ab_resizable), None, CONFIGURABLE);
    realm.define_accessor(abp, "detached", Some(ab_detached), None, CONFIGURABLE);
    realm.define_accessor(abp, "immutable", Some(ab_immutable), None, CONFIGURABLE);
    realm.define_method(abp, "resize", 1, ab_resize);
    realm.define_method(abp, "slice", 2, ab_slice);
    realm.define_method(abp, "sliceToImmutable", 2, ab_slice_to_immutable);
    realm.define_method(abp, "transfer", 0, ab_transfer);
    realm.define_method(abp, "transferToFixedLength", 0, ab_transfer_to_fixed_length);
    realm.define_method(abp, "transferToImmutable", 0, ab_transfer_to_immutable);
    realm.define_property(abp, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("ArrayBuffer"), CONFIGURABLE);

    // ---------------------------------------------------- SharedArrayBuffer
    let sabp = realm.new_object_with_proto(Some(op));
    realm.intrinsics.others.insert("SharedArrayBufferPrototype", sabp);
    let sab_ctor = define_ctor(realm, "SharedArrayBuffer", 1, shared_array_buffer_ctor, sabp);
    realm.intrinsics.others.insert("SharedArrayBuffer", sab_ctor);
    realm.define_accessor(sab_ctor, PropertyKey::Symbol(SYM_SPECIES), Some(species), None, CONFIGURABLE);
    realm.define_accessor(sabp, "byteLength", Some(sab_byte_length), None, CONFIGURABLE);
    realm.define_accessor(sabp, "growable", Some(sab_growable), None, CONFIGURABLE);
    realm.define_accessor(sabp, "maxByteLength", Some(sab_max_byte_length), None, CONFIGURABLE);
    realm.define_method(sabp, "grow", 1, sab_grow);
    realm.define_method(sabp, "slice", 2, sab_slice);
    realm.define_property(sabp, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("SharedArrayBuffer"), CONFIGURABLE);

    // ------------------------------------------------------------- DataView
    let dvp = realm.new_object_with_proto(Some(op));
    realm.intrinsics.others.insert("DataViewPrototype", dvp);
    define_ctor(realm, "DataView", 1, data_view_ctor, dvp);
    realm.define_accessor(dvp, "buffer", Some(dv_buffer), None, CONFIGURABLE);
    realm.define_accessor(dvp, "byteLength", Some(dv_byte_length), None, CONFIGURABLE);
    realm.define_accessor(dvp, "byteOffset", Some(dv_byte_offset), None, CONFIGURABLE);
    realm.define_property(dvp, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("DataView"), CONFIGURABLE);
    let dv_methods: &[(&str, u32, NativeFn)] = &[
        ("getInt8", 1, dv_get_int8),
        ("getUint8", 1, dv_get_uint8),
        ("getInt16", 1, dv_get_int16),
        ("getUint16", 1, dv_get_uint16),
        ("getInt32", 1, dv_get_int32),
        ("getUint32", 1, dv_get_uint32),
        ("getFloat16", 1, dv_get_float16),
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
        ("setFloat16", 2, dv_set_float16),
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
    realm.define_accessor(tap, "buffer", Some(ta_buffer), None, CONFIGURABLE);
    realm.define_accessor(tap, "byteLength", Some(ta_byte_length), None, CONFIGURABLE);
    realm.define_accessor(tap, "byteOffset", Some(ta_byte_offset), None, CONFIGURABLE);
    realm.define_accessor(tap, "length", Some(ta_length_getter), None, CONFIGURABLE);
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
        if kind == TypedArrayKind::Uint8 {
            realm.define_method(ctor, "fromBase64", 1, u8_from_base64);
            realm.define_method(ctor, "fromHex", 1, u8_from_hex);
            realm.define_method(proto, "toBase64", 0, u8_to_base64);
            realm.define_method(proto, "toHex", 0, u8_to_hex);
            realm.define_method(proto, "setFromBase64", 1, u8_set_from_base64);
            realm.define_method(proto, "setFromHex", 1, u8_set_from_hex);
        }
    }

    register_atomics(realm);
}

fn species(_realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(this)
}

/// GetPrototypeFromConstructor(NewTarget, default) — errors propagate.
fn proto_from_ctor(realm: &mut Realm, default_key: &str) -> JsResult<ObjectId> {
    let default = realm.intrinsics.others[default_key];
    let nt = realm.new_target();
    realm.proto_from_ctor(nt, default)
}

/// Resolve a relative index (already ToIntegerOrInfinity'd) against `len`.
fn resolve_rel(n: f64, len: usize) -> usize {
    if n < 0.0 {
        let k = len as f64 + n;
        if k < 0.0 {
            0
        } else {
            k as usize
        }
    } else if n >= len as f64 {
        len
    } else {
        n as usize
    }
}

fn idx_key(i: usize) -> PropertyKey {
    if i < u32::MAX as usize {
        PropertyKey::index(i as u32)
    } else {
        PropertyKey::str(&i.to_string())
    }
}

fn rel_arg(realm: &mut Realm, v: Value, len: usize, default: usize) -> JsResult<usize> {
    if v.is_undefined() {
        return Ok(default);
    }
    let n = realm.to_integer_or_infinity(v)?;
    Ok(resolve_rel(n, len))
}

// ------------------------------------------------------- buffer storage

thread_local! {
    /// Storage detached by a host through [`detach_buffer`].
    static DETACHED: RefCell<Vec<std::rc::Weak<RefCell<Vec<u8>>>>> = const { RefCell::new(Vec::new()) };
}

/// Detach an ArrayBuffer's storage: its bytes move out (returned) and every
/// buffer object using it reports `detached` with length 0. Hosts use it
/// when they replace the storage, e.g. a WebAssembly memory that grew.
pub fn detach_buffer(buf: &Rc<RefCell<Vec<u8>>>) -> Vec<u8> {
    let data = std::mem::take(&mut *buf.borrow_mut());
    DETACHED.with(|d| {
        let mut d = d.borrow_mut();
        d.retain(|w| w.strong_count() > 0);
        d.push(Rc::downgrade(buf));
    });
    data
}

/// Was the storage detached with [`detach_buffer`]?
pub fn is_detached(buf: &Rc<RefCell<Vec<u8>>>) -> bool {
    if !buf.borrow().is_empty() {
        return false;
    }
    DETACHED.with(|d| d.borrow().iter().any(|w| std::ptr::eq(w.as_ptr(), Rc::as_ptr(buf))))
}

fn data_detached(ab: &ArrayBufferData) -> bool {
    ab.detached || is_detached(&ab.bytes)
}

/// Allocate `len` zeroed bytes, or None when that is impossible (never
/// aborts the process).
fn try_zeroed(len: usize) -> Option<Vec<u8>> {
    if len > MAX_BYTE_LENGTH {
        return None;
    }
    if len == 0 {
        return Some(Vec::new());
    }
    let layout = std::alloc::Layout::array::<u8>(len).ok()?;
    // SAFETY: layout has non-zero size; the pointer comes from the global
    // allocator with exactly this layout, so Vec may own it.
    unsafe {
        let p = std::alloc::alloc_zeroed(layout);
        if p.is_null() {
            return None;
        }
        Some(Vec::from_raw_parts(p, len, len))
    }
}

fn alloc_bytes(realm: &mut Realm, len: usize) -> JsResult<Vec<u8>> {
    match try_zeroed(len) {
        Some(v) => Ok(v),
        None => realm.range_error("Array buffer allocation failed"),
    }
}

/// Resize `v` to `len` bytes (zero-filling), RangeError when impossible.
fn resize_bytes(realm: &mut Realm, v: &mut Vec<u8>, len: usize) -> JsResult<()> {
    if len > MAX_BYTE_LENGTH {
        return realm.range_error("Array buffer allocation failed");
    }
    if len > v.len() && v.try_reserve_exact(len - v.len()).is_err() {
        return realm.range_error("Array buffer allocation failed");
    }
    v.resize(len, 0);
    Ok(())
}

pub fn array_buffer_data(realm: &Realm, id: ObjectId) -> Option<&ArrayBufferData> {
    match &realm.obj(id).kind {
        ObjectKind::ArrayBuffer(b) => Some(&**b),
        _ => None,
    }
}

fn ab_mut(realm: &mut Realm, id: ObjectId) -> &mut ArrayBufferData {
    match &mut realm.obj_mut(id).kind {
        ObjectKind::ArrayBuffer(b) => &mut **b,
        _ => unreachable!("not an ArrayBuffer"),
    }
}

/// The byte storage of an ArrayBuffer / SharedArrayBuffer object.
pub fn buffer_of(realm: &Realm, id: ObjectId) -> Option<Rc<RefCell<Vec<u8>>>> {
    array_buffer_data(realm, id).map(|b| b.bytes.clone())
}

/// IsDetachedBuffer.
pub fn is_detached_buffer(realm: &Realm, id: ObjectId) -> bool {
    array_buffer_data(realm, id).map(data_detached).unwrap_or(false)
}

fn is_shared(realm: &Realm, id: ObjectId) -> bool {
    array_buffer_data(realm, id).map(|b| b.shared).unwrap_or(false)
}

fn is_immutable(realm: &Realm, id: ObjectId) -> bool {
    array_buffer_data(realm, id).map(|b| b.immutable).unwrap_or(false)
}

fn is_fixed_length(realm: &Realm, id: ObjectId) -> bool {
    array_buffer_data(realm, id).map(|b| b.max_byte_length.is_none()).unwrap_or(true)
}

/// Current byte length (0 when detached).
fn byte_len(realm: &Realm, id: ObjectId) -> usize {
    array_buffer_data(realm, id).map(|b| b.bytes.borrow().len()).unwrap_or(0)
}

/// DetachArrayBuffer (what `$262.detachArrayBuffer` and `transfer` do):
/// TypeError for anything but a detachable, non-shared ArrayBuffer.
pub fn detach_array_buffer(realm: &mut Realm, value: Value) -> JsResult<()> {
    let id = match value.v() {
        V::Object(id) if array_buffer_data(realm, id).is_some() => id,
        _ => return realm.type_error("detachArrayBuffer: not an ArrayBuffer"),
    };
    let ab = array_buffer_data(realm, id).unwrap();
    if ab.shared {
        return realm.type_error("Cannot detach a SharedArrayBuffer");
    }
    if ab.immutable {
        return realm.type_error("Cannot detach an immutable ArrayBuffer");
    }
    detach_object(realm, id);
    Ok(())
}

/// Detach and return the bytes (cloned when a host shares the storage).
fn detach_object(realm: &mut Realm, id: ObjectId) -> Vec<u8> {
    let ab = ab_mut(realm, id);
    let old = std::mem::replace(&mut ab.bytes, Rc::new(RefCell::new(Vec::new())));
    ab.detached = true;
    match Rc::try_unwrap(old) {
        Ok(cell) => cell.into_inner(),
        Err(rc) => {
            let v = rc.borrow().clone();
            v
        }
    }
}

/// Allocate a new `ArrayBuffer` of `len` zeroed bytes (RangeError when the
/// memory is not available).
pub fn allocate_buffer(realm: &mut Realm, len: usize, proto: Option<ObjectId>) -> JsResult<ObjectId> {
    let proto = proto.unwrap_or_else(|| realm.intrinsics.others["ArrayBufferPrototype"]);
    let Some(bytes) = zeroed_bytes(len) else {
        return realm.range_error("Array buffer allocation failed");
    };
    Ok(realm.alloc(Object::new(Some(proto), ObjectKind::ArrayBuffer(ArrayBufferData::new(bytes)))))
}

/// `len` zero bytes, or `None` when the allocation fails.
pub fn zeroed_bytes(len: usize) -> Option<Vec<u8>> {
    let mut v: Vec<u8> = Vec::new();
    v.try_reserve_exact(len).ok()?;
    v.resize(len, 0);
    Some(v)
}

/// Allocate a new `ArrayBuffer` of `len` zeroed bytes; RangeError when too big.
pub fn try_allocate_buffer(realm: &mut Realm, len: usize, proto: Option<ObjectId>) -> JsResult<ObjectId> {
    let bytes = alloc_bytes(realm, len)?;
    let proto = proto.unwrap_or_else(|| realm.intrinsics.others["ArrayBufferPrototype"]);
    Ok(realm.alloc(Object::new(Some(proto), ObjectKind::ArrayBuffer(ArrayBufferData::new(bytes)))))
}

fn new_buffer_object(realm: &mut Realm, proto: ObjectId, bytes: Vec<u8>, max: Option<usize>, shared: bool, immutable: bool) -> ObjectId {
    let mut data = ArrayBufferData::new(bytes);
    data.max_byte_length = max;
    data.shared = shared;
    data.immutable = immutable;
    realm.alloc(Object::new(Some(proto), ObjectKind::ArrayBuffer(data)))
}

// ---------------------------------------------------------- ArrayBuffer

fn this_ab(realm: &mut Realm, this: &Value, method: &str, shared: bool) -> JsResult<ObjectId> {
    if let V::Object(id) = this.v() {
        if let Some(b) = array_buffer_data(realm, id) {
            if b.shared == shared {
                return Ok(id);
            }
        }
    }
    let class = if shared { "SharedArrayBuffer" } else { "ArrayBuffer" };
    realm.type_error(&format!("Method {}.prototype.{} called on incompatible receiver", class, method))
}

/// GetArrayBufferMaxByteLengthOption.
fn max_byte_length_option(realm: &mut Realm, options: Value) -> JsResult<Option<usize>> {
    if !options.is_object() {
        return Ok(None);
    }
    let m = realm.get(options, PropertyKey::str("maxByteLength"))?;
    if m.is_undefined() {
        return Ok(None);
    }
    Ok(Some(realm.to_index(m)?))
}

fn buffer_ctor_impl(realm: &mut Realm, args: &[Value], shared: bool) -> JsResult<Value> {
    let name = if shared { "SharedArrayBuffer" } else { "ArrayBuffer" };
    if !realm.is_construct_call() {
        return realm.type_error(&format!("Constructor {} requires 'new'", name));
    }
    let len = realm.to_index(arg(args, 0))?;
    let max = max_byte_length_option(realm, arg(args, 1))?;
    if let Some(m) = max {
        if len > m {
            return realm.range_error("byteLength exceeds maxByteLength");
        }
    }
    let proto = proto_from_ctor(realm, if shared { "SharedArrayBufferPrototype" } else { "ArrayBufferPrototype" })?;
    if let Some(m) = max {
        if m > MAX_MAX_BYTE_LENGTH {
            return realm.range_error("Invalid array buffer max length");
        }
    }
    let bytes = alloc_bytes(realm, len)?;
    Ok(Value::Object(new_buffer_object(realm, proto, bytes, max, shared, false)))
}

fn array_buffer_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    buffer_ctor_impl(realm, args, false)
}

fn shared_array_buffer_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    buffer_ctor_impl(realm, args, true)
}

fn array_buffer_is_view(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(match (arg(args, 0)).v() {
        V::Object(id) => matches!(realm.obj(id).kind, ObjectKind::TypedArray(_) | ObjectKind::DataView(_)),
        _ => false,
    }))
}

fn ab_byte_length(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_ab(realm, &this, "byteLength", false)?;
    Ok(Value::Number(byte_len(realm, id) as f64))
}

fn ab_max_byte_length(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_ab(realm, &this, "maxByteLength", false)?;
    if is_detached_buffer(realm, id) {
        return Ok(Value::Number(0.0));
    }
    let ab = array_buffer_data(realm, id).unwrap();
    let n = ab.max_byte_length.unwrap_or_else(|| ab.bytes.borrow().len());
    Ok(Value::Number(n as f64))
}

fn ab_resizable(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_ab(realm, &this, "resizable", false)?;
    Ok(Value::Bool(!is_fixed_length(realm, id)))
}

fn ab_detached(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_ab(realm, &this, "detached", false)?;
    Ok(Value::Bool(is_detached_buffer(realm, id)))
}

fn ab_immutable(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_ab(realm, &this, "immutable", false)?;
    Ok(Value::Bool(is_immutable(realm, id)))
}

fn ab_resize(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = match this.v() {
        V::Object(id) => match array_buffer_data(realm, id) {
            Some(b) if !b.shared && b.max_byte_length.is_some() => id,
            _ => return realm.type_error("Method ArrayBuffer.prototype.resize called on incompatible receiver"),
        },
        _ => return realm.type_error("Method ArrayBuffer.prototype.resize called on incompatible receiver"),
    };
    let new_len = realm.to_index(arg(args, 0))?;
    if is_detached_buffer(realm, id) {
        return realm.type_error("Cannot resize a detached ArrayBuffer");
    }
    let max = array_buffer_data(realm, id).unwrap().max_byte_length.unwrap();
    if new_len > max {
        return realm.range_error("ArrayBuffer.prototype.resize: Invalid length parameter");
    }
    let bytes = array_buffer_data(realm, id).unwrap().bytes.clone();
    let mut b = bytes.borrow_mut();
    resize_bytes(realm, &mut b, new_len)?;
    Ok(Value::Undefined)
}

fn ab_slice(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_ab(realm, &this, "slice", false)?;
    if is_detached_buffer(realm, id) {
        return realm.type_error("Cannot slice a detached ArrayBuffer");
    }
    let len = byte_len(realm, id);
    let first = rel_arg(realm, arg(args, 0), len, 0)?;
    let fin = rel_arg(realm, arg(args, 1), len, len)?;
    let new_len = fin.saturating_sub(first);
    let default = realm.intrinsics.others["ArrayBuffer"];
    let ctor = realm.species_constructor(id, default)?;
    let out = realm.construct(ctor, &[Value::Number(new_len as f64)], Value::Undefined)?;
    let out_id = match out.v() {
        V::Object(o) if array_buffer_data(realm, o).is_some() => o,
        _ => return realm.type_error("ArrayBuffer species constructor did not return an ArrayBuffer"),
    };
    if is_shared(realm, out_id) {
        return realm.type_error("ArrayBuffer species constructor returned a SharedArrayBuffer");
    }
    if is_detached_buffer(realm, out_id) {
        return realm.type_error("ArrayBuffer species constructor returned a detached ArrayBuffer");
    }
    if is_immutable(realm, out_id) {
        return realm.type_error("ArrayBuffer species constructor returned an immutable ArrayBuffer");
    }
    if out_id == id {
        return realm.type_error("ArrayBuffer species constructor returned the same buffer");
    }
    if byte_len(realm, out_id) < new_len {
        return realm.type_error("ArrayBuffer species constructor returned a buffer that is too small");
    }
    if is_detached_buffer(realm, id) {
        return realm.type_error("Cannot slice a detached ArrayBuffer");
    }
    let cur = byte_len(realm, id);
    if first < cur {
        let count = new_len.min(cur - first);
        let src = buffer_of(realm, id).unwrap();
        let dst = buffer_of(realm, out_id).unwrap();
        let s = src.borrow();
        dst.borrow_mut()[..count].copy_from_slice(&s[first..first + count]);
    }
    Ok(Value::Object(out_id))
}

fn ab_slice_to_immutable(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_ab(realm, &this, "sliceToImmutable", false)?;
    if is_detached_buffer(realm, id) {
        return realm.type_error("Cannot slice a detached ArrayBuffer");
    }
    let len = byte_len(realm, id);
    let first = rel_arg(realm, arg(args, 0), len, 0)?;
    let fin = rel_arg(realm, arg(args, 1), len, len)?;
    let new_len = fin.saturating_sub(first);
    if is_detached_buffer(realm, id) {
        return realm.type_error("Cannot slice a detached ArrayBuffer");
    }
    let cur = byte_len(realm, id);
    if cur < fin {
        return realm.range_error("ArrayBuffer was resized below the end of the slice");
    }
    let mut bytes = alloc_bytes(realm, new_len)?;
    if new_len > 0 {
        let src = buffer_of(realm, id).unwrap();
        bytes.copy_from_slice(&src.borrow()[first..first + new_len]);
    }
    let proto = realm.intrinsics.others["ArrayBufferPrototype"];
    Ok(Value::Object(new_buffer_object(realm, proto, bytes, None, false, true)))
}

#[derive(PartialEq)]
enum Preserve {
    Resizability,
    FixedLength,
    Immutable,
}

/// ArrayBufferCopyAndDetach.
fn copy_and_detach(realm: &mut Realm, this: Value, new_length: Value, mode: Preserve, method: &str) -> JsResult<Value> {
    let id = this_ab(realm, &this, method, false)?;
    let new_len = if new_length.is_undefined() { byte_len(realm, id) } else { realm.to_index(new_length)? };
    if is_detached_buffer(realm, id) {
        return realm.type_error(&format!("ArrayBuffer.prototype.{}: buffer is detached", method));
    }
    if is_immutable(realm, id) {
        return realm.type_error(&format!("ArrayBuffer.prototype.{}: buffer is immutable", method));
    }
    let max = if mode == Preserve::Resizability { array_buffer_data(realm, id).unwrap().max_byte_length } else { None };
    if let Some(m) = max {
        if new_len > m {
            return realm.range_error("New length exceeds maxByteLength");
        }
    }
    if new_len > MAX_BYTE_LENGTH {
        return realm.range_error("Array buffer allocation failed");
    }
    let mut bytes = detach_object(realm, id);
    if new_len != bytes.len() {
        if let Err(e) = resize_bytes(realm, &mut bytes, new_len) {
            // Allocation failed: put the storage back.
            let ab = ab_mut(realm, id);
            ab.bytes = Rc::new(RefCell::new(bytes));
            ab.detached = false;
            return Err(e);
        }
    }
    let proto = realm.intrinsics.others["ArrayBufferPrototype"];
    Ok(Value::Object(new_buffer_object(realm, proto, bytes, max, false, mode == Preserve::Immutable)))
}

fn ab_transfer(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    copy_and_detach(realm, this, arg(args, 0), Preserve::Resizability, "transfer")
}

fn ab_transfer_to_fixed_length(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    copy_and_detach(realm, this, arg(args, 0), Preserve::FixedLength, "transferToFixedLength")
}

fn ab_transfer_to_immutable(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    copy_and_detach(realm, this, arg(args, 0), Preserve::Immutable, "transferToImmutable")
}

// ---------------------------------------------------- SharedArrayBuffer

fn sab_byte_length(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_ab(realm, &this, "byteLength", true)?;
    Ok(Value::Number(byte_len(realm, id) as f64))
}

fn sab_growable(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_ab(realm, &this, "growable", true)?;
    Ok(Value::Bool(!is_fixed_length(realm, id)))
}

fn sab_max_byte_length(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_ab(realm, &this, "maxByteLength", true)?;
    let ab = array_buffer_data(realm, id).unwrap();
    let n = ab.max_byte_length.unwrap_or_else(|| ab.bytes.borrow().len());
    Ok(Value::Number(n as f64))
}

fn sab_grow(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = match this.v() {
        V::Object(id) => match array_buffer_data(realm, id) {
            Some(b) if b.shared && b.max_byte_length.is_some() => id,
            _ => return realm.type_error("Method SharedArrayBuffer.prototype.grow called on incompatible receiver"),
        },
        _ => return realm.type_error("Method SharedArrayBuffer.prototype.grow called on incompatible receiver"),
    };
    let new_len = realm.to_index(arg(args, 0))?;
    let max = array_buffer_data(realm, id).unwrap().max_byte_length.unwrap();
    if new_len > max {
        return realm.range_error("SharedArrayBuffer.prototype.grow: Invalid length parameter");
    }
    let cur = byte_len(realm, id);
    if new_len < cur {
        return realm.range_error("SharedArrayBuffer.prototype.grow: cannot shrink");
    }
    let bytes = buffer_of(realm, id).unwrap();
    let mut b = bytes.borrow_mut();
    resize_bytes(realm, &mut b, new_len)?;
    Ok(Value::Undefined)
}

fn sab_slice(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_ab(realm, &this, "slice", true)?;
    let len = byte_len(realm, id);
    let first = rel_arg(realm, arg(args, 0), len, 0)?;
    let fin = rel_arg(realm, arg(args, 1), len, len)?;
    let new_len = fin.saturating_sub(first);
    let default = realm.intrinsics.others["SharedArrayBuffer"];
    let ctor = realm.species_constructor(id, default)?;
    let out = realm.construct(ctor, &[Value::Number(new_len as f64)], Value::Undefined)?;
    let out_id = match out.v() {
        V::Object(o) if array_buffer_data(realm, o).is_some() && is_shared(realm, o) => o,
        _ => return realm.type_error("SharedArrayBuffer species constructor did not return a SharedArrayBuffer"),
    };
    if out_id == id || Rc::ptr_eq(&buffer_of(realm, out_id).unwrap(), &buffer_of(realm, id).unwrap()) {
        return realm.type_error("SharedArrayBuffer species constructor returned the same buffer");
    }
    if byte_len(realm, out_id) < new_len {
        return realm.type_error("SharedArrayBuffer species constructor returned a buffer that is too small");
    }
    let cur = byte_len(realm, id);
    if first < cur {
        let count = new_len.min(cur - first);
        let src = buffer_of(realm, id).unwrap();
        let dst = buffer_of(realm, out_id).unwrap();
        let s = src.borrow();
        dst.borrow_mut()[..count].copy_from_slice(&s[first..first + count]);
    }
    Ok(Value::Object(out_id))
}

// ------------------------------------------------------ element encoding

/// A converted element value: a Number, or the low 64 bits of a BigInt.
#[derive(Clone, Copy, Debug)]
pub enum Num {
    F(f64),
    B(u64),
}

/// f64 → IEEE binary16 bits, rounding to nearest, ties to even.
pub fn f64_to_f16_bits(x: f64) -> u16 {
    let bits = x.to_bits();
    let sign = ((bits >> 48) & 0x8000) as u16;
    if x.is_nan() {
        return sign | 0x7e00;
    }
    let a = x.abs();
    if a >= 65520.0 {
        return sign | 0x7c00;
    }
    if a < 6.103515625e-05 {
        // Subnormal (or zero): units of 2^-24; exact scaling.
        let m = (a * 16777216.0).round_ties_even() as u16;
        return sign | m;
    }
    let exp = ((bits >> 52) & 0x7ff) as i32 - 1023;
    let mant = bits & ((1u64 << 52) - 1);
    let mut m = (mant >> 42) as u32;
    let rem = mant & ((1u64 << 42) - 1);
    let half = 1u64 << 41;
    if rem > half || (rem == half && m & 1 == 1) {
        m += 1;
    }
    let mut e = (exp + 15) as u32;
    if m == 1024 {
        m = 0;
        e += 1;
    }
    if e >= 31 {
        return sign | 0x7c00;
    }
    sign | ((e as u16) << 10) | m as u16
}

pub fn f16_bits_to_f64(h: u16) -> f64 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = ((h >> 10) & 0x1f) as i32;
    let m = (h & 0x3ff) as f64;
    if e == 0 {
        sign * m * 2f64.powi(-24)
    } else if e == 31 {
        if m == 0.0 {
            sign * f64::INFINITY
        } else {
            f64::NAN
        }
    } else {
        sign * (1.0 + m / 1024.0) * 2f64.powi(e - 15)
    }
}

/// Little-endian bytes of `num` stored as `kind` (first `element_size` bytes).
fn encode(kind: TypedArrayKind, num: Num) -> [u8; 8] {
    let mut out = [0u8; 8];
    let n = match num {
        Num::F(n) => n,
        Num::B(b) => {
            out = b.to_le_bytes();
            return out;
        }
    };
    match kind {
        TypedArrayKind::Int8 | TypedArrayKind::Uint8 => out[0] = to_int32(n) as u8,
        TypedArrayKind::Uint8Clamped => out[0] = if n.is_nan() { 0 } else { n.clamp(0.0, 255.0).round_ties_even() as u8 },
        TypedArrayKind::Int16 | TypedArrayKind::Uint16 => out[..2].copy_from_slice(&(to_int32(n) as u16).to_le_bytes()),
        TypedArrayKind::Int32 | TypedArrayKind::Uint32 => out[..4].copy_from_slice(&to_uint32(n).to_le_bytes()),
        TypedArrayKind::Float16 => out[..2].copy_from_slice(&f64_to_f16_bits(n).to_le_bytes()),
        TypedArrayKind::Float32 => out[..4].copy_from_slice(&(n as f32).to_le_bytes()),
        TypedArrayKind::Float64 => out = n.to_le_bytes(),
        TypedArrayKind::BigInt64 => out = (n as i64).to_le_bytes(),
        TypedArrayKind::BigUint64 => out = (n as u64).to_le_bytes(),
    }
    out
}

/// Decode little-endian `bytes` (at least `element_size` long) as `kind`.
fn decode(kind: TypedArrayKind, bytes: &[u8]) -> Value {
    match kind {
        TypedArrayKind::Int8 => Value::Number(bytes[0] as i8 as f64),
        TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped => Value::Number(bytes[0] as f64),
        TypedArrayKind::Int16 => Value::Number(i16::from_le_bytes([bytes[0], bytes[1]]) as f64),
        TypedArrayKind::Uint16 => Value::Number(u16::from_le_bytes([bytes[0], bytes[1]]) as f64),
        TypedArrayKind::Int32 => Value::Number(i32::from_le_bytes(bytes[..4].try_into().unwrap()) as f64),
        TypedArrayKind::Uint32 => Value::Number(u32::from_le_bytes(bytes[..4].try_into().unwrap()) as f64),
        TypedArrayKind::Float16 => Value::Number(f16_bits_to_f64(u16::from_le_bytes([bytes[0], bytes[1]]))),
        TypedArrayKind::Float32 => Value::Number(f32::from_le_bytes(bytes[..4].try_into().unwrap()) as f64),
        TypedArrayKind::Float64 => Value::Number(f64::from_le_bytes(bytes[..8].try_into().unwrap())),
        TypedArrayKind::BigInt64 => Value::BigInt(Rc::new(BigInt::from_i64(i64::from_le_bytes(bytes[..8].try_into().unwrap())))),
        TypedArrayKind::BigUint64 => Value::BigInt(Rc::new(BigInt::from_u64(u64::from_le_bytes(bytes[..8].try_into().unwrap())))),
    }
}

/// ToNumber / ToBigInt for a store into an element of `kind`.
fn to_num(realm: &mut Realm, kind: TypedArrayKind, v: Value) -> JsResult<Num> {
    if is_bigint_kind(kind) {
        Ok(Num::B(to_bigint(realm, v)? as u64))
    } else {
        Ok(Num::F(realm.to_number(v)?))
    }
}

/// ToBigInt for typed-array stores: BigInt values pass, booleans/strings
/// convert, numbers throw.
pub fn to_bigint(realm: &mut Realm, v: Value) -> JsResult<i128> {
    let p = realm.to_primitive(v, crate::interop::Hint::Number)?;
    match p.v() {
        V::BigInt(b) => Ok(b.to_i128_wrapping()),
        V::Bool(b) => Ok(b as i128),
        V::String(s) => match crate::value::BigInt::parse(s.to_rust_string().trim()) {
            Some(b) => Ok(b.to_i128_wrapping()),
            None => Err(realm.throw_syntax_error("Cannot convert string to a BigInt")),
        },
        V::Number(_) => realm.type_error("Cannot convert a Number to a BigInt"),
        _ => realm.type_error("Cannot convert value to a BigInt"),
    }
}

// --------------------------------------------------- typed array records

/// TypedArrayLength of a view, None when out of bounds or detached.
#[inline]
pub fn view_length(realm: &Realm, t: &TypedArrayData) -> Option<usize> {
    let ab = array_buffer_data(realm, t.buffer)?;
    let len = ab.bytes.borrow().len();
    if len == 0 && data_detached(ab) {
        return None;
    }
    t.length_in(len)
}

/// Current element count of a typed array (0 when out of bounds/detached).
pub fn typed_array_length(realm: &Realm, t: &TypedArrayData) -> usize {
    view_length(realm, t).unwrap_or(0)
}

/// Read element `index` (None when not a valid index right now).
#[inline]
pub fn get_element(realm: &Realm, t: &TypedArrayData, index: usize) -> Option<Value> {
    let ObjectKind::ArrayBuffer(ab) = &realm.obj(t.buffer).kind else { return None };
    let b = ab.bytes.borrow();
    if index >= t.length_in(b.len())? {
        return None;
    }
    let size = t.kind.element_size();
    let off = t.byte_offset + index * size;
    Some(decode(t.kind, &b[off..off + size]))
}

/// Write an already converted value into element `index` (ignored when it
/// is not a valid index right now or the buffer is immutable).
#[inline]
pub fn set_element(realm: &Realm, t: &TypedArrayData, index: usize, num: Num) {
    let ObjectKind::ArrayBuffer(ab) = &realm.obj(t.buffer).kind else { return };
    if ab.immutable {
        return;
    }
    let mut b = ab.bytes.borrow_mut();
    match t.length_in(b.len()) {
        Some(len) if index < len => {}
        _ => return,
    }
    let size = t.kind.element_size();
    let off = t.byte_offset + index * size;
    let bytes = encode(t.kind, num);
    b[off..off + size].copy_from_slice(&bytes[..size]);
}

/// Read element `i` (None when out of range).
pub fn ta_get(realm: &Realm, t: &TypedArrayData, i: usize) -> Option<Value> {
    get_element(realm, t, i)
}

/// TypedArraySetElement with an integer index: convert, then store when
/// the index is (still) valid.
pub fn ta_set_value(realm: &mut Realm, t: &TypedArrayData, i: usize, v: Value) -> JsResult<()> {
    let num = to_num(realm, t.kind, v)?;
    set_element(realm, t, i, num);
    Ok(())
}

/// TypedArraySetElement with a canonical numeric index `n`.
pub fn ta_set_element(realm: &mut Realm, t: &TypedArrayData, n: f64, v: Value) -> JsResult<()> {
    let num = to_num(realm, t.kind, v)?;
    if let Some(i) = ta_valid_index(realm, t, n) {
        set_element(realm, t, i, num);
    }
    Ok(())
}

/// IsValidIntegerIndex(O, n).
pub fn ta_valid_index(realm: &Realm, t: &TypedArrayData, n: f64) -> Option<usize> {
    if n.fract() != 0.0 || (n == 0.0 && n.is_sign_negative()) || n < 0.0 {
        return None;
    }
    let len = view_length(realm, t)?;
    if n >= len as f64 {
        return None;
    }
    Some(n as usize)
}

/// Is the typed array's buffer immutable?
pub fn ta_immutable(realm: &Realm, t: &TypedArrayData) -> bool {
    is_immutable(realm, t.buffer)
}

/// Can the typed array be made non-extensible (fixed length over a
/// fixed-length buffer)?
pub fn ta_fixed_length(realm: &Realm, t: &TypedArrayData) -> bool {
    t.fixed_length.is_some() && is_fixed_length(realm, t.buffer)
}

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

/// The array iterator's view of a typed array's length: TypeError once it
/// is out of bounds or detached.
pub fn iteration_length(realm: &mut Realm, t: &TypedArrayData) -> JsResult<usize> {
    match view_length(realm, t) {
        Some(n) => Ok(n),
        None => realm.type_error("Cannot iterate over a typed array whose buffer is detached or out of bounds"),
    }
}

fn ta_data(realm: &Realm, id: ObjectId) -> Option<TypedArrayData> {
    match &realm.obj(id).kind {
        ObjectKind::TypedArray(t) => Some(**t),
        _ => None,
    }
}

/// RequireInternalSlot(O, [[TypedArrayName]]).
fn this_ta(realm: &mut Realm, this: &Value, method: &str) -> JsResult<(ObjectId, TypedArrayData)> {
    if let V::Object(id) = this.v() {
        if let Some(t) = ta_data(realm, id) {
            return Ok((id, t));
        }
    }
    realm.type_error(&format!("Method %TypedArray%.prototype.{} called on incompatible receiver", method))
}

/// ValidateTypedArray(O, order, accessMode): the object, its record and
/// its current length.
fn validate_ta(realm: &mut Realm, this: &Value, method: &str, write: bool) -> JsResult<(ObjectId, TypedArrayData, usize)> {
    let (id, t) = this_ta(realm, this, method)?;
    if write && is_immutable(realm, t.buffer) {
        return realm.type_error(&format!("%TypedArray%.prototype.{}: the buffer is immutable", method));
    }
    match view_length(realm, &t) {
        Some(len) => Ok((id, t, len)),
        None => realm.type_error(&format!("%TypedArray%.prototype.{}: the buffer is detached or out of bounds", method)),
    }
}

/// Create a typed array of `kind` with a fresh buffer.
pub fn create_typed_array(realm: &mut Realm, kind: TypedArrayKind, length: usize, proto: Option<ObjectId>) -> JsResult<ObjectId> {
    let byte_len = match length.checked_mul(kind.element_size()) {
        Some(n) if n <= MAX_BYTE_LENGTH => n,
        _ => return realm.range_error("Invalid typed array length"),
    };
    let buffer = try_allocate_buffer(realm, byte_len, None)?;
    let proto = proto.unwrap_or_else(|| realm.intrinsics.others[proto_key(kind)]);
    let data = TypedArrayData { kind, buffer, byte_offset: 0, fixed_length: Some(length) };
    Ok(realm.alloc(Object::new(Some(proto), ObjectKind::TypedArray(Box::new(data)))))
}

fn typed_array_abstract_ctor(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    realm.type_error("Abstract class TypedArray not directly constructable")
}

/// IteratorToList(GetIteratorFromMethod(obj, method)).
fn iterate_with_method(realm: &mut Realm, obj: Value, method: Value) -> JsResult<Vec<Value>> {
    if let V::Object(id) = obj.v() {
        let pristine = realm.intrinsics.others.get("ArrayProtoValues").map(|f| method.as_object() == Some(*f)).unwrap_or(false);
        if pristine && matches!(realm.obj(id).kind, ObjectKind::Array { .. }) {
            return realm.iterate_to_vec(obj);
        }
    }
    let it = realm.call(method, obj, &[])?;
    if !it.is_object() {
        return realm.type_error("Result of the Symbol.iterator method is not an object");
    }
    let next = realm.get(it.clone(), PropertyKey::str("next"))?;
    let mut out = Vec::new();
    while let Some(v) = realm.iterator_step(&it, &next)? {
        out.push(v);
    }
    Ok(out)
}

fn typed_array_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let callee = realm.current_native().unwrap_or(realm.global);
    let kind = *realm.native_data::<TypedArrayKind>(callee).unwrap_or(Rc::new(TypedArrayKind::Uint8));
    if !realm.is_construct_call() {
        return realm.type_error(&format!("Constructor {} requires 'new'", kind.name()));
    }
    let pkey = proto_key(kind);
    let first = arg(args, 0);
    let src = match first.v() {
        V::Object(src) => src,
        _ => {
            // new TA(length)
            let len = realm.to_index(first.clone())?;
            let proto = proto_from_ctor(realm, pkey)?;
            return Ok(Value::Object(create_typed_array(realm, kind, len, Some(proto))?));
        }
    };
    let proto = proto_from_ctor(realm, pkey)?;
    let esize = kind.element_size();
    if let Some(st) = ta_data(realm, src) {
        // InitializeTypedArrayFromTypedArray
        let Some(len) = view_length(realm, &st) else {
            return realm.type_error("Cannot construct a typed array from a detached or out-of-bounds typed array");
        };
        if is_bigint_kind(st.kind) != is_bigint_kind(kind) {
            return realm.type_error("Cannot mix BigInt and other types, use explicit conversions");
        }
        let id = create_typed_array(realm, kind, len, Some(proto))?;
        let dt = ta_data(realm, id).unwrap();
        if st.kind == kind {
            let src_bytes = buffer_of(realm, st.buffer).unwrap();
            let dst_bytes = buffer_of(realm, dt.buffer).unwrap();
            let s = src_bytes.borrow();
            dst_bytes.borrow_mut()[..len * esize].copy_from_slice(&s[st.byte_offset..st.byte_offset + len * esize]);
        } else {
            for i in 0..len {
                let v = get_element(realm, &st, i).unwrap_or(Value::Undefined);
                let num = value_to_num(&v, kind);
                set_element(realm, &dt, i, num);
            }
        }
        return Ok(Value::Object(id));
    }
    if array_buffer_data(realm, src).is_some() {
        // InitializeTypedArrayFromArrayBuffer
        let offset = realm.to_index(arg(args, 1))?;
        if offset % esize != 0 {
            return realm.range_error(&format!("start offset of {} should be a multiple of {}", kind.name(), esize));
        }
        let fixed_buffer = is_fixed_length(realm, src);
        let new_length = if arg(args, 2).is_undefined() { None } else { Some(realm.to_index(arg(args, 2))?) };
        if is_detached_buffer(realm, src) {
            return realm.type_error("Cannot construct a typed array on a detached ArrayBuffer");
        }
        let buf_len = byte_len(realm, src);
        let fixed_length = match new_length {
            None if !fixed_buffer => {
                if offset > buf_len {
                    return realm.range_error(&format!("Start offset {} is outside the bounds of the buffer", offset));
                }
                None
            }
            None => {
                if buf_len % esize != 0 {
                    return realm.range_error(&format!("byte length of {} should be a multiple of {}", kind.name(), esize));
                }
                if offset > buf_len {
                    return realm.range_error(&format!("Start offset {} is outside the bounds of the buffer", offset));
                }
                Some((buf_len - offset) / esize)
            }
            Some(l) => {
                if l.checked_mul(esize).and_then(|b| b.checked_add(offset)).map(|end| end > buf_len).unwrap_or(true) {
                    return realm.range_error(&format!("Invalid typed array length: {}", l));
                }
                Some(l)
            }
        };
        let data = TypedArrayData { kind, buffer: src, byte_offset: offset, fixed_length };
        return Ok(Value::Object(realm.alloc(Object::new(Some(proto), ObjectKind::TypedArray(Box::new(data))))));
    }
    // Iterable or array-like object.
    let using_iterator = realm.get_method(first.clone(), PropertyKey::Symbol(SYM_ITERATOR))?;
    if let Some(m) = using_iterator {
        let values = iterate_with_method(realm, first, m)?;
        let id = create_typed_array(realm, kind, values.len(), Some(proto))?;
        let dt = ta_data(realm, id).unwrap();
        for (i, v) in values.into_iter().enumerate() {
            ta_set_element(realm, &dt, i as f64, v)?;
        }
        return Ok(Value::Object(id));
    }
    let len = realm.length_of(first.clone())?;
    let id = create_typed_array(realm, kind, len, Some(proto))?;
    let dt = ta_data(realm, id).unwrap();
    for i in 0..len {
        let v = realm.get(first.clone(), idx_key(i))?;
        ta_set_element(realm, &dt, i as f64, v)?;
    }
    Ok(Value::Object(id))
}

/// Convert an element value read from one typed array for a store into
/// another of the same content type (no user code can run).
fn value_to_num(v: &Value, kind: TypedArrayKind) -> Num {
    match v.v() {
        V::Number(n) => Num::F(n),
        V::BigInt(b) => Num::B(b.to_i128_wrapping() as u64),
        _ => {
            if is_bigint_kind(kind) {
                Num::B(0)
            } else {
                Num::F(f64::NAN)
            }
        }
    }
}

/// TypedArrayCreateFromConstructor.
fn create_from_ctor(realm: &mut Realm, ctor: Value, args: &[Value], write: bool) -> JsResult<(ObjectId, TypedArrayData)> {
    let out = realm.construct(ctor, args, Value::Undefined)?;
    let (id, t, len) = validate_ta(realm, &out, "constructor", write)?;
    if let [a] = args {
        if let V::Number(n) = a.v() {
            if (len as f64) < n {
                return realm.type_error("Derived TypedArray constructor created an array which was too small");
            }
        }
    }
    Ok((id, t))
}

/// TypedArraySpeciesCreate.
fn species_create(realm: &mut Realm, exemplar: ObjectId, kind: TypedArrayKind, args: &[Value], write: bool) -> JsResult<(ObjectId, TypedArrayData)> {
    let default = realm.intrinsics.others[kind.name()];
    let ctor = realm.species_constructor(exemplar, default)?;
    let (id, t) = create_from_ctor(realm, ctor, args, write)?;
    if is_bigint_kind(t.kind) != is_bigint_kind(kind) {
        return realm.type_error("Content type of the typed array species does not match");
    }
    Ok((id, t))
}

// ---------------------------------------------------- %TypedArray% getters

fn ta_buffer(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "buffer")?;
    Ok(Value::Object(t.buffer))
}

fn ta_byte_length(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "byteLength")?;
    Ok(Value::Number((typed_array_length(realm, &t) * t.kind.element_size()) as f64))
}

fn ta_byte_offset(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "byteOffset")?;
    if view_length(realm, &t).is_none() {
        return Ok(Value::Number(0.0));
    }
    Ok(Value::Number(t.byte_offset as f64))
}

fn ta_length_getter(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "length")?;
    Ok(Value::Number(typed_array_length(realm, &t) as f64))
}

fn ta_to_string_tag(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    if let V::Object(id) = this.v() {
        if let ObjectKind::TypedArray(t) = &realm.obj(id).kind {
            return Ok(Value::str(t.kind.name()));
        }
    }
    Ok(Value::Undefined)
}

// --------------------------------------------- %TypedArray%.prototype.*

fn callback_arg(realm: &mut Realm, args: &[Value], method: &str) -> JsResult<Value> {
    let f = arg(args, 0);
    if !realm.is_callable(&f) {
        let d = realm.describe_for_error(&f);
        return realm.type_error(&format!("{} is not a function (TypedArray.prototype.{})", d, method));
    }
    Ok(f)
}

#[inline]
fn get_or_undef(realm: &Realm, t: &TypedArrayData, i: usize) -> Value {
    get_element(realm, t, i).unwrap_or(Value::Undefined)
}

fn ta_at(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t, len) = validate_ta(realm, &this, "at", false)?;
    let n = realm.to_integer_or_infinity(arg(args, 0))?;
    let k = if n < 0.0 { len as f64 + n } else { n };
    if k < 0.0 || k >= len as f64 {
        return Ok(Value::Undefined);
    }
    Ok(get_or_undef(realm, &t, k as usize))
}

fn ta_copy_within(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t, len) = validate_ta(realm, &this, "copyWithin", true)?;
    let target = rel_arg(realm, arg(args, 0), len, 0)?;
    let start = rel_arg(realm, arg(args, 1), len, 0)?;
    let end = rel_arg(realm, arg(args, 2), len, len)?;
    let count = (end as i64 - start as i64).min(len as i64 - target as i64);
    if count > 0 {
        let Some(len) = view_length(realm, &t) else {
            return realm.type_error("%TypedArray%.prototype.copyWithin: the buffer is detached or out of bounds");
        };
        let es = t.kind.element_size();
        let limit = len * es + t.byte_offset;
        let to = target * es + t.byte_offset;
        let from = start * es + t.byte_offset;
        let count_bytes = count as usize * es;
        let buf = buffer_of(realm, t.buffer).unwrap();
        let mut b = buf.borrow_mut();
        // Clip to what is still inside the (possibly shrunk) view.
        let n = count_bytes.min(limit.saturating_sub(from)).min(limit.saturating_sub(to));
        if n > 0 {
            b.copy_within(from..from + n, to);
        }
    }
    Ok(this)
}

fn ta_entries(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    validate_ta(realm, &this, "entries", false)?;
    Ok(create_array_iterator(realm, this, 2))
}
fn ta_keys(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    validate_ta(realm, &this, "keys", false)?;
    Ok(create_array_iterator(realm, this, 0))
}
fn ta_values(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    validate_ta(realm, &this, "values", false)?;
    Ok(create_array_iterator(realm, this, 1))
}

fn ta_every(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t, len) = validate_ta(realm, &this, "every", false)?;
    let f = callback_arg(realm, args, "every")?;
    let this_arg = arg(args, 1);
    for i in 0..len {
        let v = get_or_undef(realm, &t, i);
        if !realm.call(f.clone(), this_arg.clone(), &[v, Value::Number(i as f64), this.clone()])?.truthy() {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

fn ta_some(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t, len) = validate_ta(realm, &this, "some", false)?;
    let f = callback_arg(realm, args, "some")?;
    let this_arg = arg(args, 1);
    for i in 0..len {
        let v = get_or_undef(realm, &t, i);
        if realm.call(f.clone(), this_arg.clone(), &[v, Value::Number(i as f64), this.clone()])?.truthy() {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

fn ta_fill(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t, len) = validate_ta(realm, &this, "fill", true)?;
    let num = to_num(realm, t.kind, arg(args, 0))?;
    let start = rel_arg(realm, arg(args, 1), len, 0)?;
    let end = rel_arg(realm, arg(args, 2), len, len)?;
    let Some(len) = view_length(realm, &t) else {
        return realm.type_error("%TypedArray%.prototype.fill: the buffer is detached or out of bounds");
    };
    let end = end.min(len);
    if start < end {
        let es = t.kind.element_size();
        let bytes = encode(t.kind, num);
        let buf = buffer_of(realm, t.buffer).unwrap();
        let mut b = buf.borrow_mut();
        let region = &mut b[t.byte_offset + start * es..t.byte_offset + end * es];
        if es == 1 {
            region.fill(bytes[0]);
        } else {
            for chunk in region.chunks_exact_mut(es) {
                chunk.copy_from_slice(&bytes[..es]);
            }
        }
    }
    Ok(this)
}

fn ta_filter(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, t, len) = validate_ta(realm, &this, "filter", false)?;
    let f = callback_arg(realm, args, "filter")?;
    let this_arg = arg(args, 1);
    let mut kept = Vec::new();
    for i in 0..len {
        let v = get_or_undef(realm, &t, i);
        if realm.call(f.clone(), this_arg.clone(), &[v.clone(), Value::Number(i as f64), this.clone()])?.truthy() {
            kept.push(v);
        }
    }
    let (out, od) = species_create(realm, id, t.kind, &[Value::Number(kept.len() as f64)], true)?;
    for (i, v) in kept.into_iter().enumerate() {
        ta_set_element(realm, &od, i as f64, v)?;
    }
    Ok(Value::Object(out))
}

fn find_impl(realm: &mut Realm, this: Value, args: &[Value], method: &str, from_end: bool, want_index: bool) -> JsResult<Value> {
    let (_, t, len) = validate_ta(realm, &this, method, false)?;
    let f = callback_arg(realm, args, method)?;
    let this_arg = arg(args, 1);
    for j in 0..len {
        let i = if from_end { len - 1 - j } else { j };
        let v = get_or_undef(realm, &t, i);
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
    let (_, t, len) = validate_ta(realm, &this, "forEach", false)?;
    let f = callback_arg(realm, args, "forEach")?;
    let this_arg = arg(args, 1);
    for i in 0..len {
        let v = get_or_undef(realm, &t, i);
        realm.call(f.clone(), this_arg.clone(), &[v, Value::Number(i as f64), this.clone()])?;
    }
    Ok(Value::Undefined)
}

/// Start index for includes/indexOf: None when the search is empty.
fn search_start(realm: &mut Realm, from: Value, len: usize) -> JsResult<Option<usize>> {
    let n = realm.to_integer_or_infinity(from)?;
    if n == f64::INFINITY {
        return Ok(None);
    }
    Ok(Some(resolve_rel(if n == f64::NEG_INFINITY { 0.0 } else { n }, len)))
}

fn ta_includes(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t, len) = validate_ta(realm, &this, "includes", false)?;
    if len == 0 {
        return Ok(Value::Bool(false));
    }
    let Some(start) = search_start(realm, arg(args, 1), len)? else { return Ok(Value::Bool(false)) };
    let target = arg(args, 0);
    for i in start..len {
        let v = get_or_undef(realm, &t, i);
        if realm.same_value_zero(&v, &target) {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

fn ta_index_of(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t, len) = validate_ta(realm, &this, "indexOf", false)?;
    if len == 0 {
        return Ok(Value::Number(-1.0));
    }
    let Some(start) = search_start(realm, arg(args, 1), len)? else { return Ok(Value::Number(-1.0)) };
    let target = arg(args, 0);
    for i in start..len {
        if let Some(v) = get_element(realm, &t, i) {
            if realm.strict_equals(&v, &target) {
                return Ok(Value::Number(i as f64));
            }
        }
    }
    Ok(Value::Number(-1.0))
}

fn ta_last_index_of(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t, len) = validate_ta(realm, &this, "lastIndexOf", false)?;
    if len == 0 {
        return Ok(Value::Number(-1.0));
    }
    let n = if args.len() > 1 { realm.to_integer_or_infinity(arg(args, 1))? } else { len as f64 - 1.0 };
    if n == f64::NEG_INFINITY {
        return Ok(Value::Number(-1.0));
    }
    let k = if n >= 0.0 { n.min(len as f64 - 1.0) } else { len as f64 + n };
    if k < 0.0 {
        return Ok(Value::Number(-1.0));
    }
    let target = arg(args, 0);
    for i in (0..=k as usize).rev() {
        if let Some(v) = get_element(realm, &t, i) {
            if realm.strict_equals(&v, &target) {
                return Ok(Value::Number(i as f64));
            }
        }
    }
    Ok(Value::Number(-1.0))
}

fn ta_join(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t, len) = validate_ta(realm, &this, "join", false)?;
    let sep = match arg(args, 0) {
        Value::Undefined => ",".to_string(),
        v => realm.to_rust_string(v)?,
    };
    let mut out = String::new();
    for i in 0..len {
        if i > 0 {
            out.push_str(&sep);
        }
        if let Some(v) = get_element(realm, &t, i) {
            out.push_str(&realm.to_rust_string(v)?);
        }
    }
    Ok(Value::str(&out))
}

fn ta_to_locale_string(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t, len) = validate_ta(realm, &this, "toLocaleString", false)?;
    let mut out = String::new();
    for i in 0..len {
        if i > 0 {
            out.push(',');
        }
        let v = get_or_undef(realm, &t, i);
        if !v.is_nullish() {
            let s = realm.invoke(v, "toLocaleString", args)?;
            out.push_str(&realm.to_rust_string(s)?);
        }
    }
    Ok(Value::str(&out))
}

fn ta_map(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, t, len) = validate_ta(realm, &this, "map", false)?;
    let f = callback_arg(realm, args, "map")?;
    let this_arg = arg(args, 1);
    let (out, od) = species_create(realm, id, t.kind, &[Value::Number(len as f64)], true)?;
    for i in 0..len {
        let v = get_or_undef(realm, &t, i);
        let r = realm.call(f.clone(), this_arg.clone(), &[v, Value::Number(i as f64), this.clone()])?;
        ta_set_element(realm, &od, i as f64, r)?;
    }
    Ok(Value::Object(out))
}

fn reduce_impl(realm: &mut Realm, this: Value, args: &[Value], method: &str, from_end: bool) -> JsResult<Value> {
    let (_, t, len) = validate_ta(realm, &this, method, false)?;
    let f = callback_arg(realm, args, method)?;
    let idx = |j: usize| if from_end { len - 1 - j } else { j };
    let mut j = 0;
    let mut acc = if args.len() >= 2 {
        args[1].clone()
    } else {
        if len == 0 {
            return realm.type_error("Reduce of empty array with no initial value");
        }
        j = 1;
        get_or_undef(realm, &t, idx(0))
    };
    while j < len {
        let i = idx(j);
        let v = get_or_undef(realm, &t, i);
        acc = realm.call(f.clone(), Value::Undefined, &[acc, v, Value::Number(i as f64), this.clone()])?;
        j += 1;
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
    let (_, t, len) = validate_ta(realm, &this, "reverse", true)?;
    let es = t.kind.element_size();
    let buf = buffer_of(realm, t.buffer).unwrap();
    let mut b = buf.borrow_mut();
    let slice = &mut b[t.byte_offset..t.byte_offset + len * es];
    let (mut i, mut j) = (0usize, len);
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
    let (_, t, len) = validate_ta(realm, &this, "toReversed", false)?;
    let out = create_typed_array(realm, t.kind, len, None)?;
    let od = ta_data(realm, out).unwrap();
    for i in 0..len {
        let v = get_or_undef(realm, &t, len - 1 - i);
        let num = value_to_num(&v, t.kind);
        set_element(realm, &od, i, num);
    }
    Ok(Value::Object(out))
}

fn ta_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_ta(realm, &this, "set")?;
    if is_immutable(realm, t.buffer) {
        return realm.type_error("%TypedArray%.prototype.set: the buffer is immutable");
    }
    let offset = realm.to_integer_or_infinity(arg(args, 1))?;
    if offset < 0.0 {
        return realm.range_error("offset is out of bounds");
    }
    let source = arg(args, 0);
    if let V::Object(sid) = source.v() {
        if let Some(st) = ta_data(realm, sid) {
            // SetTypedArrayFromTypedArray
            let Some(target_len) = view_length(realm, &t) else {
                return realm.type_error("%TypedArray%.prototype.set: the target is detached or out of bounds");
            };
            let Some(src_len) = view_length(realm, &st) else {
                return realm.type_error("%TypedArray%.prototype.set: the source is detached or out of bounds");
            };
            if is_bigint_kind(st.kind) != is_bigint_kind(t.kind) {
                return realm.type_error("Cannot mix BigInt and other types, use explicit conversions");
            }
            if offset == f64::INFINITY || src_len as f64 + offset > target_len as f64 {
                return realm.range_error("offset is out of bounds");
            }
            let offset = offset as usize;
            if st.kind == t.kind {
                // Same layout: raw byte copy (handles overlapping views).
                let es = t.kind.element_size();
                let sbuf = buffer_of(realm, st.buffer).unwrap();
                let dbuf = buffer_of(realm, t.buffer).unwrap();
                let bytes: Vec<u8> = sbuf.borrow()[st.byte_offset..st.byte_offset + src_len * es].to_vec();
                dbuf.borrow_mut()[t.byte_offset + offset * es..t.byte_offset + (offset + src_len) * es].copy_from_slice(&bytes);
            } else {
                let vals: Vec<Value> = (0..src_len).map(|i| get_or_undef(realm, &st, i)).collect();
                for (i, v) in vals.into_iter().enumerate() {
                    let num = value_to_num(&v, t.kind);
                    set_element(realm, &t, offset + i, num);
                }
            }
            return Ok(Value::Undefined);
        }
    }
    // SetTypedArrayFromArrayLike
    let Some(target_len) = view_length(realm, &t) else {
        return realm.type_error("%TypedArray%.prototype.set: the target is detached or out of bounds");
    };
    let src = realm.to_object(source)?;
    let src_len = realm.length_of(Value::Object(src))?;
    if offset == f64::INFINITY || src_len as f64 + offset > target_len as f64 {
        return realm.range_error("offset is out of bounds");
    }
    let offset = offset as usize;
    for i in 0..src_len {
        let v = realm.get(Value::Object(src), idx_key(i))?;
        ta_set_element(realm, &t, (offset + i) as f64, v)?;
    }
    Ok(Value::Undefined)
}

fn ta_slice(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, t, len) = validate_ta(realm, &this, "slice", false)?;
    let start = rel_arg(realm, arg(args, 0), len, 0)?;
    let end = rel_arg(realm, arg(args, 1), len, len)?;
    let count = end.saturating_sub(start);
    let (out, od) = species_create(realm, id, t.kind, &[Value::Number(count as f64)], true)?;
    if count > 0 {
        let Some(len) = view_length(realm, &t) else {
            return realm.type_error("%TypedArray%.prototype.slice: the buffer is detached or out of bounds");
        };
        let end = end.min(len);
        let count = end.saturating_sub(start);
        if count == 0 {
        } else if od.kind == t.kind {
            let es = t.kind.element_size();
            let sbuf = buffer_of(realm, t.buffer).unwrap();
            let dbuf = buffer_of(realm, od.buffer).unwrap();
            let src = t.byte_offset + start * es;
            let n = count * es;
            if Rc::ptr_eq(&sbuf, &dbuf) {
                // Same storage: the spec's forward byte-by-byte copy.
                let mut b = sbuf.borrow_mut();
                let n = n.min(b.len().saturating_sub(od.byte_offset)).min(b.len().saturating_sub(src));
                for k in 0..n {
                    b[od.byte_offset + k] = b[src + k];
                }
            } else {
                let s = sbuf.borrow();
                let mut d = dbuf.borrow_mut();
                let n = n.min(d.len().saturating_sub(od.byte_offset));
                d[od.byte_offset..od.byte_offset + n].copy_from_slice(&s[src..src + n]);
            }
        } else {
            for i in 0..count {
                let v = get_or_undef(realm, &t, start + i);
                let num = value_to_num(&v, od.kind);
                set_element(realm, &od, i, num);
            }
        }
    }
    Ok(Value::Object(out))
}

/// SortIndexedProperties for a typed array's current elements.
fn sort_elements(realm: &mut Realm, t: &TypedArrayData, len: usize, cmp: Value) -> JsResult<Vec<Value>> {
    let items: Vec<Value> = (0..len).map(|i| get_or_undef(realm, t, i)).collect();
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

fn comparator_arg(realm: &mut Realm, args: &[Value]) -> JsResult<Value> {
    let cmp = arg(args, 0);
    if !cmp.is_undefined() && !realm.is_callable(&cmp) {
        return realm.type_error("The comparison function must be either a function or undefined");
    }
    Ok(cmp)
}

fn ta_sort(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let cmp = comparator_arg(realm, args)?;
    let (_, t, len) = validate_ta(realm, &this, "sort", true)?;
    let sorted = sort_elements(realm, &t, len, cmp)?;
    for (i, v) in sorted.into_iter().enumerate() {
        let num = value_to_num(&v, t.kind);
        set_element(realm, &t, i, num);
    }
    Ok(this)
}

fn ta_to_sorted(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let cmp = comparator_arg(realm, args)?;
    let (_, t, len) = validate_ta(realm, &this, "toSorted", false)?;
    let out = create_typed_array(realm, t.kind, len, None)?;
    let od = ta_data(realm, out).unwrap();
    let sorted = sort_elements(realm, &t, len, cmp)?;
    for (i, v) in sorted.into_iter().enumerate() {
        let num = value_to_num(&v, t.kind);
        set_element(realm, &od, i, num);
    }
    Ok(Value::Object(out))
}

fn ta_subarray(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, t) = this_ta(realm, &this, "subarray")?;
    let src_len = typed_array_length(realm, &t);
    let start = rel_arg(realm, arg(args, 0), src_len, 0)?;
    let es = t.kind.element_size();
    let begin = Value::Number((t.byte_offset + start * es) as f64);
    let end_arg = arg(args, 1);
    let out = if t.fixed_length.is_none() && end_arg.is_undefined() {
        species_create(realm, id, t.kind, &[Value::Object(t.buffer), begin], false)?.0
    } else {
        let end = rel_arg(realm, end_arg, src_len, src_len)?;
        let count = end.saturating_sub(start);
        species_create(realm, id, t.kind, &[Value::Object(t.buffer), begin, Value::Number(count as f64)], false)?.0
    };
    Ok(Value::Object(out))
}

fn ta_with(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t, len) = validate_ta(realm, &this, "with", false)?;
    let rel = realm.to_integer_or_infinity(arg(args, 0))?;
    let idx = if rel < 0.0 { len as f64 + rel } else { rel };
    let num = to_num(realm, t.kind, arg(args, 1))?;
    if ta_valid_index(realm, &t, idx).is_none() {
        return realm.range_error("Invalid typed array index");
    }
    let idx = idx as usize;
    let out = create_typed_array(realm, t.kind, len, None)?;
    let od = ta_data(realm, out).unwrap();
    for i in 0..len {
        if i == idx {
            set_element(realm, &od, i, num);
        } else {
            // A shrunk source reads undefined, which ToBigInt rejects.
            let v = get_or_undef(realm, &t, i);
            ta_set_element(realm, &od, i as f64, v)?;
        }
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
    let using_iterator = realm.get_method(source.clone(), PropertyKey::Symbol(SYM_ITERATOR))?;
    if let Some(m) = using_iterator {
        let values = iterate_with_method(realm, source, m)?;
        let (oid, od) = create_from_ctor(realm, this, &[Value::Number(values.len() as f64)], true)?;
        for (i, v) in values.into_iter().enumerate() {
            let v = if map_fn.is_undefined() { v } else { realm.call(map_fn.clone(), this_arg.clone(), &[v, Value::Number(i as f64)])? };
            ta_set_element(realm, &od, i as f64, v)?;
        }
        return Ok(Value::Object(oid));
    }
    let o = realm.to_object(source)?;
    let len = realm.length_of(Value::Object(o))?;
    let (oid, od) = create_from_ctor(realm, this, &[Value::Number(len as f64)], true)?;
    for i in 0..len {
        let v = realm.get(Value::Object(o), idx_key(i))?;
        let v = if map_fn.is_undefined() { v } else { realm.call(map_fn.clone(), this_arg.clone(), &[v, Value::Number(i as f64)])? };
        ta_set_element(realm, &od, i as f64, v)?;
    }
    Ok(Value::Object(oid))
}

fn typed_array_of(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    if !realm.is_constructor(&this) {
        return realm.type_error("TypedArray.of: this is not a constructor");
    }
    let (oid, od) = create_from_ctor(realm, this, &[Value::Number(args.len() as f64)], true)?;
    for (i, v) in args.iter().enumerate() {
        ta_set_element(realm, &od, i as f64, v.clone())?;
    }
    Ok(Value::Object(oid))
}

// ------------------------------------------------------------- DataView

fn data_view_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if !realm.is_construct_call() {
        return realm.type_error("Constructor DataView requires 'new'");
    }
    let buffer = match (arg(args, 0)).v() {
        V::Object(id) if array_buffer_data(realm, id).is_some() => id,
        _ => return realm.type_error("First argument to DataView constructor must be an ArrayBuffer"),
    };
    let offset = realm.to_index(arg(args, 1))?;
    if is_detached_buffer(realm, buffer) {
        return realm.type_error("Cannot construct a DataView on a detached ArrayBuffer");
    }
    let buf_len = byte_len(realm, buffer);
    if offset > buf_len {
        return realm.range_error(&format!("Start offset {} is outside the bounds of the buffer", offset));
    }
    let fixed_buffer = is_fixed_length(realm, buffer);
    let length_arg = arg(args, 2);
    let view_len = if length_arg.is_undefined() {
        if fixed_buffer {
            Some(buf_len - offset)
        } else {
            None
        }
    } else {
        let l = realm.to_index(length_arg.clone())?;
        if offset.checked_add(l).map(|e| e > buf_len).unwrap_or(true) {
            return realm.range_error(&format!("Invalid DataView length {}", l));
        }
        Some(l)
    };
    let proto = proto_from_ctor(realm, "DataViewPrototype")?;
    if is_detached_buffer(realm, buffer) {
        return realm.type_error("Cannot construct a DataView on a detached ArrayBuffer");
    }
    let buf_len = byte_len(realm, buffer);
    if offset > buf_len {
        return realm.range_error(&format!("Start offset {} is outside the bounds of the buffer", offset));
    }
    if !length_arg.is_undefined() && offset + view_len.unwrap() > buf_len {
        return realm.range_error("Invalid DataView length");
    }
    let data = TypedArrayData { kind: TypedArrayKind::Uint8, buffer, byte_offset: offset, fixed_length: view_len };
    Ok(Value::Object(realm.alloc(Object::new(Some(proto), ObjectKind::DataView(Box::new(data))))))
}

fn this_view(realm: &mut Realm, this: &Value, method: &str) -> JsResult<TypedArrayData> {
    if let V::Object(id) = this.v() {
        if let ObjectKind::DataView(t) = &realm.obj(id).kind {
            return Ok(**t);
        }
    }
    realm.type_error(&format!("Method DataView.prototype.{} called on incompatible receiver", method))
}

fn dv_buffer(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let t = this_view(realm, &this, "buffer")?;
    Ok(Value::Object(t.buffer))
}

fn dv_byte_length(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let t = this_view(realm, &this, "byteLength")?;
    match view_length(realm, &t) {
        Some(n) => Ok(Value::Number(n as f64)),
        None => Ok(Value::Number(0.0)),
    }
}

fn dv_byte_offset(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let t = this_view(realm, &this, "byteOffset")?;
    match view_length(realm, &t) {
        Some(_) => Ok(Value::Number(t.byte_offset as f64)),
        None => Ok(Value::Number(0.0)),
    }
}

/// GetViewValue.
fn dv_get(realm: &mut Realm, this: &Value, args: &[Value], kind: TypedArrayKind, method: &str) -> JsResult<Value> {
    let t = this_view(realm, this, method)?;
    let idx = realm.to_index(arg(args, 0))?;
    let little = arg(args, 1).truthy();
    let Some(view_size) = view_length(realm, &t) else {
        return realm.type_error(&format!("DataView.prototype.{}: the buffer is detached or out of bounds", method));
    };
    let size = kind.element_size();
    if idx.saturating_add(size) > view_size {
        return realm.range_error("Offset is outside the bounds of the DataView");
    }
    let buf = buffer_of(realm, t.buffer).unwrap();
    let b = buf.borrow();
    let off = t.byte_offset + idx;
    let mut bytes = [0u8; 8];
    bytes[..size].copy_from_slice(&b[off..off + size]);
    if !little {
        bytes[..size].reverse();
    }
    Ok(decode(kind, &bytes))
}

/// SetViewValue.
fn dv_set(realm: &mut Realm, this: &Value, args: &[Value], kind: TypedArrayKind, method: &str) -> JsResult<Value> {
    let t = this_view(realm, this, method)?;
    if is_immutable(realm, t.buffer) {
        return realm.type_error(&format!("DataView.prototype.{}: the buffer is immutable", method));
    }
    let idx = realm.to_index(arg(args, 0))?;
    let num = to_num(realm, kind, arg(args, 1))?;
    let little = arg(args, 2).truthy();
    let Some(view_size) = view_length(realm, &t) else {
        return realm.type_error(&format!("DataView.prototype.{}: the buffer is detached or out of bounds", method));
    };
    let size = kind.element_size();
    if idx.saturating_add(size) > view_size {
        return realm.range_error("Offset is outside the bounds of the DataView");
    }
    let mut bytes = encode(kind, num);
    if !little {
        bytes[..size].reverse();
    }
    let buf = buffer_of(realm, t.buffer).unwrap();
    let off = t.byte_offset + idx;
    buf.borrow_mut()[off..off + size].copy_from_slice(&bytes[..size]);
    Ok(Value::Undefined)
}

macro_rules! dv_methods {
    ($($get:ident, $set:ident, $kind:ident, $gname:expr, $sname:expr;)*) => {
        $(
            fn $get(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
                dv_get(realm, &this, args, TypedArrayKind::$kind, $gname)
            }
            fn $set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
                dv_set(realm, &this, args, TypedArrayKind::$kind, $sname)
            }
        )*
    };
}

dv_methods! {
    dv_get_int8, dv_set_int8, Int8, "getInt8", "setInt8";
    dv_get_uint8, dv_set_uint8, Uint8, "getUint8", "setUint8";
    dv_get_int16, dv_set_int16, Int16, "getInt16", "setInt16";
    dv_get_uint16, dv_set_uint16, Uint16, "getUint16", "setUint16";
    dv_get_int32, dv_set_int32, Int32, "getInt32", "setInt32";
    dv_get_uint32, dv_set_uint32, Uint32, "getUint32", "setUint32";
    dv_get_float16, dv_set_float16, Float16, "getFloat16", "setFloat16";
    dv_get_float32, dv_set_float32, Float32, "getFloat32", "setFloat32";
    dv_get_float64, dv_set_float64, Float64, "getFloat64", "setFloat64";
    dv_get_bigint64, dv_set_bigint64, BigInt64, "getBigInt64", "setBigInt64";
    dv_get_biguint64, dv_set_biguint64, BigUint64, "getBigUint64", "setBigUint64";
}

// ------------------------------------------------- Uint8Array base64/hex

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn this_u8(realm: &mut Realm, this: &Value, method: &str) -> JsResult<(ObjectId, TypedArrayData)> {
    if let V::Object(id) = this.v() {
        if let Some(t) = ta_data(realm, id) {
            if t.kind == TypedArrayKind::Uint8 {
                return Ok((id, t));
            }
        }
    }
    realm.type_error(&format!("Uint8Array.prototype.{} called on incompatible receiver", method))
}

/// GetOptionsObject.
fn options_object(realm: &mut Realm, v: Value) -> JsResult<Option<Value>> {
    match v.v() {
        V::Undefined => Ok(None),
        V::Object(_) => Ok(Some(v.clone())),
        _ => realm.type_error("options must be an object"),
    }
}

/// The `alphabet` option: true for base64url.
fn alphabet_option(realm: &mut Realm, opts: &Option<Value>) -> JsResult<bool> {
    let Some(o) = opts else { return Ok(false) };
    let a = realm.get(o.clone(), PropertyKey::str("alphabet"))?;
    match a.v() {
        V::Undefined => Ok(false),
        V::String(s) if s.eq_str("base64") => Ok(false),
        V::String(s) if s.eq_str("base64url") => Ok(true),
        _ => realm.type_error("alphabet must be \"base64\" or \"base64url\""),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum LastChunk {
    Loose,
    Strict,
    StopBeforePartial,
}

fn last_chunk_option(realm: &mut Realm, opts: &Option<Value>) -> JsResult<LastChunk> {
    let Some(o) = opts else { return Ok(LastChunk::Loose) };
    let a = realm.get(o.clone(), PropertyKey::str("lastChunkHandling"))?;
    match a.v() {
        V::Undefined => Ok(LastChunk::Loose),
        V::String(s) if s.eq_str("loose") => Ok(LastChunk::Loose),
        V::String(s) if s.eq_str("strict") => Ok(LastChunk::Strict),
        V::String(s) if s.eq_str("stop-before-partial") => Ok(LastChunk::StopBeforePartial),
        _ => realm.type_error("lastChunkHandling must be \"loose\", \"strict\" or \"stop-before-partial\""),
    }
}

struct Decoded {
    read: usize,
    bytes: Vec<u8>,
    error: bool,
}

fn b64_value(c: u16) -> Option<u32> {
    let c = u8::try_from(c).ok()?;
    match c {
        b'A'..=b'Z' => Some((c - b'A') as u32),
        b'a'..=b'z' => Some((c - b'a' + 26) as u32),
        b'0'..=b'9' => Some((c - b'0' + 52) as u32),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// DecodeBase64Chunk: None when strict and the padding bits are non-zero.
fn decode_chunk(chunk: &[u32], strict: bool, out: &mut Vec<u8>) -> bool {
    let n = chunk.len();
    let mut v = 0u32;
    for i in 0..4 {
        v = (v << 6) | chunk.get(i).copied().unwrap_or(0);
    }
    let b = [(v >> 16) as u8, (v >> 8) as u8, v as u8];
    match n {
        2 => {
            if strict && b[1] != 0 {
                return false;
            }
            out.push(b[0]);
        }
        3 => {
            if strict && b[2] != 0 {
                return false;
            }
            out.extend_from_slice(&b[..2]);
        }
        _ => out.extend_from_slice(&b),
    }
    true
}

fn skip_ws(s: &[u16], mut i: usize) -> usize {
    while i < s.len() && matches!(s[i], 0x09 | 0x0A | 0x0C | 0x0D | 0x20) {
        i += 1;
    }
    i
}

/// FromBase64.
fn from_base64(s: &[u16], url: bool, last: LastChunk, max_len: usize) -> Decoded {
    let mut d = Decoded { read: 0, bytes: Vec::new(), error: false };
    if max_len == 0 {
        return d;
    }
    let mut chunk: Vec<u32> = Vec::with_capacity(4);
    let mut index = 0;
    let len = s.len();
    loop {
        index = skip_ws(s, index);
        if index == len {
            if !chunk.is_empty() {
                match last {
                    LastChunk::StopBeforePartial => return d,
                    LastChunk::Loose => {
                        if chunk.len() == 1 {
                            d.error = true;
                            return d;
                        }
                        decode_chunk(&chunk, false, &mut d.bytes);
                    }
                    LastChunk::Strict => {
                        d.error = true;
                        return d;
                    }
                }
            }
            d.read = len;
            return d;
        }
        let mut c = s[index];
        index += 1;
        if c == b'=' as u16 {
            if chunk.len() < 2 {
                d.error = true;
                return d;
            }
            index = skip_ws(s, index);
            if chunk.len() == 2 {
                if index == len {
                    if last == LastChunk::StopBeforePartial {
                        return d;
                    }
                    d.error = true;
                    return d;
                }
                if s[index] == b'=' as u16 {
                    index = skip_ws(s, index + 1);
                }
            }
            if index < len {
                d.error = true;
                return d;
            }
            if !decode_chunk(&chunk, last == LastChunk::Strict, &mut d.bytes) {
                d.error = true;
                return d;
            }
            d.read = len;
            return d;
        }
        if url {
            if c == b'+' as u16 || c == b'/' as u16 {
                d.error = true;
                return d;
            }
            if c == b'-' as u16 {
                c = b'+' as u16;
            } else if c == b'_' as u16 {
                c = b'/' as u16;
            }
        }
        let Some(v) = b64_value(c) else {
            d.error = true;
            return d;
        };
        let remaining = max_len - d.bytes.len();
        if (remaining == 1 && chunk.len() == 2) || (remaining == 2 && chunk.len() == 3) {
            return d;
        }
        chunk.push(v);
        if chunk.len() == 4 {
            decode_chunk(&chunk, false, &mut d.bytes);
            chunk.clear();
            d.read = index;
            if d.bytes.len() == max_len {
                return d;
            }
        }
    }
}

fn hex_value(c: u16) -> Option<u8> {
    let c = u8::try_from(c).ok()?;
    (c as char).to_digit(16).map(|v| v as u8)
}

/// FromHex.
fn from_hex(s: &[u16], max_len: usize) -> Decoded {
    let mut d = Decoded { read: 0, bytes: Vec::new(), error: false };
    if s.len() % 2 != 0 {
        d.error = true;
        return d;
    }
    while d.read < s.len() && d.bytes.len() < max_len {
        match (hex_value(s[d.read]), hex_value(s[d.read + 1])) {
            (Some(a), Some(b)) => {
                d.bytes.push(a << 4 | b);
                d.read += 2;
            }
            _ => {
                d.error = true;
                return d;
            }
        }
    }
    d
}

fn string_arg(realm: &mut Realm, v: Value) -> JsResult<JsStr> {
    match v.v() {
        V::String(s) => Ok(s.clone()),
        _ => realm.type_error("argument must be a string"),
    }
}

fn new_u8_array(realm: &mut Realm, bytes: &[u8]) -> JsResult<Value> {
    let id = create_typed_array(realm, TypedArrayKind::Uint8, bytes.len(), None)?;
    let t = ta_data(realm, id).unwrap();
    buffer_of(realm, t.buffer).unwrap().borrow_mut().copy_from_slice(bytes);
    Ok(Value::Object(id))
}

fn u8_from_base64(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let s = string_arg(realm, arg(args, 0))?;
    let opts = options_object(realm, arg(args, 1))?;
    let url = alphabet_option(realm, &opts)?;
    let last = last_chunk_option(realm, &opts)?;
    let d = from_base64(s.as_units(), url, last, usize::MAX);
    if d.error {
        return Err(realm.throw_syntax_error("Invalid base64 string"));
    }
    new_u8_array(realm, &d.bytes)
}

fn u8_from_hex(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let s = string_arg(realm, arg(args, 0))?;
    let d = from_hex(s.as_units(), usize::MAX);
    if d.error {
        return Err(realm.throw_syntax_error("Invalid hex string"));
    }
    new_u8_array(realm, &d.bytes)
}

/// GetUint8ArrayBytes.
fn u8_bytes(realm: &mut Realm, t: &TypedArrayData, method: &str) -> JsResult<Vec<u8>> {
    let Some(len) = view_length(realm, t) else {
        return realm.type_error(&format!("Uint8Array.prototype.{}: the buffer is detached or out of bounds", method));
    };
    let buf = buffer_of(realm, t.buffer).unwrap();
    let b = buf.borrow();
    Ok(b[t.byte_offset..t.byte_offset + len].to_vec())
}

fn u8_to_base64(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_u8(realm, &this, "toBase64")?;
    let opts = options_object(realm, arg(args, 0))?;
    let url = alphabet_option(realm, &opts)?;
    let omit = match &opts {
        Some(o) => realm.get(o.clone(), PropertyKey::str("omitPadding"))?.truthy(),
        None => false,
    };
    let bytes = u8_bytes(realm, &t, "toBase64")?;
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let enc = |i: u32| -> char {
        let c = B64[i as usize];
        (if url && c == b'+' {
            b'-'
        } else if url && c == b'/' {
            b'_'
        } else {
            c
        }) as char
    };
    for ch in bytes.chunks(3) {
        let v = (ch[0] as u32) << 16 | (*ch.get(1).unwrap_or(&0) as u32) << 8 | *ch.get(2).unwrap_or(&0) as u32;
        out.push(enc(v >> 18 & 63));
        out.push(enc(v >> 12 & 63));
        if ch.len() > 1 {
            out.push(enc(v >> 6 & 63));
        } else if !omit {
            out.push('=');
        }
        if ch.len() > 2 {
            out.push(enc(v & 63));
        } else if !omit {
            out.push('=');
        }
    }
    Ok(Value::str(&out))
}

fn u8_to_hex(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_u8(realm, &this, "toHex")?;
    let bytes = u8_bytes(realm, &t, "toHex")?;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{:02x}", b));
    }
    Ok(Value::str(&out))
}

fn set_from_result(realm: &mut Realm, t: &TypedArrayData, d: Decoded, what: &str) -> JsResult<Value> {
    if !d.bytes.is_empty() {
        let buf = buffer_of(realm, t.buffer).unwrap();
        buf.borrow_mut()[t.byte_offset..t.byte_offset + d.bytes.len()].copy_from_slice(&d.bytes);
    }
    if d.error {
        return Err(realm.throw_syntax_error(&format!("Invalid {} string", what)));
    }
    let o = realm.new_object();
    realm.define_property(o, "read", Value::Number(d.read as f64), ALL);
    realm.define_property(o, "written", Value::Number(d.bytes.len() as f64), ALL);
    Ok(Value::Object(o))
}

fn u8_set_from_base64(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_u8(realm, &this, "setFromBase64")?;
    if is_immutable(realm, t.buffer) {
        return realm.type_error("Uint8Array.prototype.setFromBase64: the buffer is immutable");
    }
    let s = string_arg(realm, arg(args, 0))?;
    let opts = options_object(realm, arg(args, 1))?;
    let url = alphabet_option(realm, &opts)?;
    let last = last_chunk_option(realm, &opts)?;
    let Some(len) = view_length(realm, &t) else {
        return realm.type_error("Uint8Array.prototype.setFromBase64: the buffer is detached or out of bounds");
    };
    let d = from_base64(s.as_units(), url, last, len);
    set_from_result(realm, &t, d, "base64")
}

fn u8_set_from_hex(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_u8(realm, &this, "setFromHex")?;
    if is_immutable(realm, t.buffer) {
        return realm.type_error("Uint8Array.prototype.setFromHex: the buffer is immutable");
    }
    let s = string_arg(realm, arg(args, 0))?;
    let Some(len) = view_length(realm, &t) else {
        return realm.type_error("Uint8Array.prototype.setFromHex: the buffer is detached or out of bounds");
    };
    let d = from_hex(s.as_units(), len);
    set_from_result(realm, &t, d, "hex")
}

// --------------------------------------------------------------- Atomics

fn register_atomics(realm: &mut Realm) {
    let op = realm.intrinsics.object_proto;
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
        ("waitAsync", 4, atomics_wait_async),
        ("notify", 3, atomics_notify),
        ("pause", 0, atomics_pause),
    ];
    for (name, arity, f) in ops {
        realm.define_method(atomics, name, *arity, *f);
    }
    // Pending Atomics.waitAsync waiters: [buffer, byteIndex, resolve] triples.
    let waiters = realm.new_array(Vec::new());
    realm.intrinsics.others.insert("AtomicsWaiterList", waiters);
}

/// ValidateIntegerTypedArray.
fn atomics_validate(realm: &mut Realm, v: &Value, waitable: bool, write: bool) -> JsResult<(TypedArrayData, usize)> {
    let t = match v.v() {
        V::Object(id) => match ta_data(realm, id) {
            Some(t) => t,
            None => return realm.type_error("Atomics operation on a non-typed-array"),
        },
        _ => return realm.type_error("Atomics operation on a non-typed-array"),
    };
    if write && is_immutable(realm, t.buffer) {
        return realm.type_error("Atomics operation on an immutable buffer");
    }
    let Some(len) = view_length(realm, &t) else {
        return realm.type_error("Atomics operation on a detached or out-of-bounds typed array");
    };
    let ok = if waitable {
        matches!(t.kind, TypedArrayKind::Int32 | TypedArrayKind::BigInt64)
    } else {
        !matches!(t.kind, TypedArrayKind::Float16 | TypedArrayKind::Float32 | TypedArrayKind::Float64 | TypedArrayKind::Uint8Clamped)
    };
    if !ok {
        return realm.type_error("Atomics operations are not allowed on this typed array type");
    }
    Ok((t, len))
}

/// ValidateAtomicAccess: the element index.
fn atomic_access(realm: &mut Realm, len: usize, index: Value) -> JsResult<usize> {
    let i = realm.to_index(index)?;
    if i >= len {
        return realm.range_error("Invalid atomic access index");
    }
    Ok(i)
}

/// RevalidateAtomicAccess.
fn atomics_revalidate(realm: &mut Realm, t: &TypedArrayData, i: usize) -> JsResult<()> {
    let Some(len) = view_length(realm, t) else {
        return realm.type_error("Atomics operation on a detached or out-of-bounds typed array");
    };
    if i >= len {
        return realm.range_error("Invalid atomic access index");
    }
    Ok(())
}

/// The value operand: ToBigInt or ToIntegerOrInfinity.
fn atomics_operand(realm: &mut Realm, kind: TypedArrayKind, v: Value) -> JsResult<Num> {
    if is_bigint_kind(kind) {
        Ok(Num::B(to_bigint(realm, v)? as u64))
    } else {
        Ok(Num::F(realm.to_integer_or_infinity(v)?))
    }
}

/// Raw element bits (little-endian, zero-extended) at element `i`.
fn raw_bits(realm: &Realm, t: &TypedArrayData, i: usize) -> u64 {
    let size = t.kind.element_size();
    let buf = buffer_of(realm, t.buffer).unwrap();
    let b = buf.borrow();
    let off = t.byte_offset + i * size;
    let mut bytes = [0u8; 8];
    bytes[..size].copy_from_slice(&b[off..off + size]);
    u64::from_le_bytes(bytes)
}

fn write_bits(realm: &Realm, t: &TypedArrayData, i: usize, bits: u64) {
    let size = t.kind.element_size();
    let buf = buffer_of(realm, t.buffer).unwrap();
    let off = t.byte_offset + i * size;
    buf.borrow_mut()[off..off + size].copy_from_slice(&bits.to_le_bytes()[..size]);
}

fn num_bits(kind: TypedArrayKind, n: Num) -> u64 {
    u64::from_le_bytes(encode(kind, n))
}

fn atomics_rmw(realm: &mut Realm, args: &[Value], op: fn(u64, u64) -> u64) -> JsResult<Value> {
    let (t, len) = atomics_validate(realm, &arg(args, 0), false, true)?;
    let i = atomic_access(realm, len, arg(args, 1))?;
    let v = atomics_operand(realm, t.kind, arg(args, 2))?;
    atomics_revalidate(realm, &t, i)?;
    let old = raw_bits(realm, &t, i);
    let new = op(old, num_bits(t.kind, v));
    write_bits(realm, &t, i, new);
    Ok(decode(t.kind, &old.to_le_bytes()))
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
    let (t, len) = atomics_validate(realm, &arg(args, 0), false, true)?;
    let i = atomic_access(realm, len, arg(args, 1))?;
    let expected = atomics_operand(realm, t.kind, arg(args, 2))?;
    let replacement = atomics_operand(realm, t.kind, arg(args, 3))?;
    atomics_revalidate(realm, &t, i)?;
    let size = t.kind.element_size();
    let mask = if size == 8 { u64::MAX } else { (1u64 << (size * 8)) - 1 };
    let old = raw_bits(realm, &t, i);
    if old & mask == num_bits(t.kind, expected) & mask {
        write_bits(realm, &t, i, num_bits(t.kind, replacement));
    }
    Ok(decode(t.kind, &old.to_le_bytes()))
}

fn atomics_load(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let (t, len) = atomics_validate(realm, &arg(args, 0), false, false)?;
    let i = atomic_access(realm, len, arg(args, 1))?;
    atomics_revalidate(realm, &t, i)?;
    Ok(get_or_undef(realm, &t, i))
}

fn atomics_store(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let (t, len) = atomics_validate(realm, &arg(args, 0), false, true)?;
    let i = atomic_access(realm, len, arg(args, 1))?;
    let v = arg(args, 2);
    let (num, ret) = if is_bigint_kind(t.kind) {
        let b = to_bigint(realm, v)?;
        (Num::B(b as u64), realm.new_bigint(b))
    } else {
        let n = realm.to_integer_or_infinity(v)? + 0.0;
        (Num::F(n), Value::Number(n))
    };
    atomics_revalidate(realm, &t, i)?;
    set_element(realm, &t, i, num);
    Ok(ret)
}

fn atomics_is_lock_free(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let n = realm.to_integer_or_infinity(arg(args, 0))?;
    Ok(Value::Bool(n == 1.0 || n == 2.0 || n == 4.0 || n == 8.0))
}

/// Shared part of Atomics.wait / waitAsync (DoWait up to the comparison):
/// (value equal?, timeout in ms, buffer, byte index).
fn wait_common(realm: &mut Realm, args: &[Value]) -> JsResult<(bool, f64, ObjectId, usize)> {
    let (t, len) = atomics_validate(realm, &arg(args, 0), true, false)?;
    if !is_shared(realm, t.buffer) {
        return realm.type_error("Atomics.wait requires a shared typed array");
    }
    let i = atomic_access(realm, len, arg(args, 1))?;
    let v = if t.kind == TypedArrayKind::BigInt64 {
        Num::B(to_bigint(realm, arg(args, 2))? as u64)
    } else {
        Num::F(realm.to_int32(arg(args, 2))? as f64)
    };
    let q = realm.to_number(arg(args, 3))?;
    let timeout = if q.is_nan() || q == f64::INFINITY {
        f64::INFINITY
    } else if q == f64::NEG_INFINITY {
        0.0
    } else {
        q.max(0.0)
    };
    let cur = raw_bits(realm, &t, i);
    let size = t.kind.element_size();
    let mask = if size == 8 { u64::MAX } else { (1u64 << (size * 8)) - 1 };
    Ok((cur & mask == num_bits(t.kind, v) & mask, timeout, t.buffer, t.byte_offset + i * size))
}

fn atomics_wait(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    wait_common(realm, args)?;
    // AgentCanSuspend() is false: the engine runs on the page's main thread.
    realm.type_error("Atomics.wait cannot be called in this context")
}

fn atomics_wait_async(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let (equal, timeout, buffer, byte_index) = wait_common(realm, args)?;
    let o = realm.new_object();
    if !equal || timeout == 0.0 {
        realm.define_property(o, "async", Value::Bool(false), ALL);
        realm.define_property(o, "value", Value::str(if equal { "timed-out" } else { "not-equal" }), ALL);
        return Ok(Value::Object(o));
    }
    // The waiter settles with "ok" when notified. There are no timers
    // here, so a finite timeout never fires.
    let (p, resolve, _) = realm.new_promise();
    let entry = realm.new_array_value(vec![Value::Object(buffer), Value::Number(byte_index as f64), Value::Object(resolve)]);
    let list = realm.intrinsics.others["AtomicsWaiterList"];
    if let ObjectKind::Array { elements, length } = &mut realm.obj_mut(list).kind {
        elements.push(entry);
        *length = elements.len() as u32;
    }
    realm.define_property(o, "async", Value::Bool(true), ALL);
    realm.define_property(o, "value", Value::Object(p), ALL);
    Ok(Value::Object(o))
}

fn atomics_notify(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let (t, len) = atomics_validate(realm, &arg(args, 0), true, false)?;
    let i = atomic_access(realm, len, arg(args, 1))?;
    let count = arg(args, 2);
    let count = if count.is_undefined() { f64::INFINITY } else { realm.to_integer_or_infinity(count)?.max(0.0) };
    if !is_shared(realm, t.buffer) {
        return Ok(Value::Number(0.0));
    }
    let byte_index = (t.byte_offset + i * t.kind.element_size()) as f64;
    let list = realm.intrinsics.others["AtomicsWaiterList"];
    let entries: Vec<Value> = match &realm.obj(list).kind {
        ObjectKind::Array { elements, .. } => elements.clone(),
        _ => Vec::new(),
    };
    let mut keep = Vec::new();
    let mut woken = Vec::new();
    for e in entries {
        let parts: Vec<Value> = match e.v() {
            V::Object(eid) => match &realm.obj(eid).kind {
                ObjectKind::Array { elements, .. } => elements.clone(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        let matches = parts.len() == 3 && parts[0].as_object() == Some(t.buffer) && parts[1].as_number() == Some(byte_index);
        if matches && (woken.len() as f64) < count {
            woken.push(parts[2].clone());
        } else {
            keep.push(e);
        }
    }
    if let ObjectKind::Array { elements, length } = &mut realm.obj_mut(list).kind {
        *length = keep.len() as u32;
        *elements = keep;
    }
    let n = woken.len();
    for resolve in woken {
        realm.call(resolve, Value::Undefined, &[Value::str("ok")])?;
    }
    Ok(Value::Number(n as f64))
}

fn atomics_pause(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let n = arg(args, 0);
    match n.v() {
        V::Undefined => {}
        V::Number(x) if x.is_finite() && x.fract() == 0.0 => {}
        _ => return realm.type_error("Atomics.pause: argument must be an integral Number"),
    }
    std::hint::spin_loop();
    Ok(Value::Undefined)
}

/// [[PreventExtensions]] of a typed array (Ok(false) for a length-tracking
/// or resizable-backed view), plus the element check of `seal` / `freeze`
/// (TypeError when it has elements: they cannot become non-configurable).
/// Ok(true) for any other object.
pub fn check_integrity(realm: &mut Realm, id: ObjectId, seal_or_freeze: bool) -> JsResult<bool> {
    let Some(t) = ta_data(realm, id) else { return Ok(true) };
    if !ta_fixed_length(realm, &t) {
        return Ok(false);
    }
    if seal_or_freeze && typed_array_length(realm, &t) > 0 {
        return realm.type_error("Cannot freeze or seal a typed array with elements");
    }
    Ok(true)
}
