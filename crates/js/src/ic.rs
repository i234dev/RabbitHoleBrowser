//! Inline caches for named property and global variable access.
//!
//! Each `GetProp` / `SetProp` / `DefineProp` / `GetGlobal` / `SetGlobal`
//! instruction owns a [`PropIc`] in its function's cache table. The slow
//! path performs the full lookup and then records how the property was
//! found (shape id + slot of the receiver, or of a prototype); the fast path
//! in the interpreter re-validates that record with a few id comparisons
//! instead of searching. A cache never changes semantics: every hit is
//! validated against current object state and falls back to the slow path.

use crate::bytecode::PropIc;
use crate::value::V;
use crate::heap::*;
use crate::realm::Realm;
use crate::value::*;
use std::collections::HashMap;

pub const IC_NONE: u8 = 0;
/// Own property of a receiver with shape `shape`, in `slot`.
pub const IC_OWN: u8 = 1;
/// Own property of a dictionary-mode receiver at entry `slot` (key-checked).
pub const IC_OWN_DICT: u8 = 2;
/// Receiver with shape `shape` lacks the key; it is found on `holder`
/// (reached through `mid` when that is nonzero) in `slot`.
pub const IC_PROTO: u8 = 3;
/// Primitive receiver: found in `slot` of its wrapper prototype `holder`.
pub const IC_PRIM: u8 = 4;
/// `length` of an array or a string primitive.
pub const IC_LENGTH: u8 = 5;
/// Store adding a new property: receiver shape `shape` transitions to the
/// shape with id `slot`; the prototype chain (`holder`, `mid`) is checked
/// by shape ids to still lack the key.
pub const IC_ADD: u8 = 6;
/// Script-level lexical binding `slot` (valid while the lexical epoch is `shape`).
pub const IC_GLOBAL_LEX: u8 = 7;
/// Global object property: shaped (`holder_shape` = shape id) or dictionary
/// (key-checked entry `slot`), valid while the lexical epoch is `shape`.
/// The record also caches a pointer to the property (`mid`/`mid_shape`),
/// usable while the prototype epoch is still `holder`.
pub const IC_GLOBAL_OBJ: u8 = 8;

/// Receiver with shape `shape` and prototype `mid` lacks the key; it is
/// found in `slot` of `holder` further up the chain. Valid while the
/// prototype epoch is `mid_shape` (every object passed is marked, see
/// [`crate::shape::proto_epoch`]).
pub const IC_PROTO_DEEP: u8 = 9;
/// Like [`IC_ADD`] for longer prototype chains: receiver shape `shape`
/// transitions to the shape with id `slot`; its prototype is `holder` and
/// the chain (proved to lack the key) is unchanged while the prototype
/// epoch is `mid_shape`.
pub const IC_ADD_DEEP: u8 = 10;
/// Store through an accessor property: `holder` (`NO_OBJ`: the receiver
/// itself, with shape `shape`) has it in `slot`; for an inherited one the
/// receiver has shape `shape` and prototype `mid`, and the chain is valid
/// while the prototype epoch is `mid_shape`.
pub const IC_SETTER: u8 = 11;

/// One slot of the megamorphic cache (see [`Realm::mega_get`]). The
/// entry keeps its name alive: names are compared by identity (their
/// word), and a freed string's address could be reused by another name.
#[derive(Clone, Default)]
pub struct MegaEntry {
    shape: u32,
    key: u64,
    /// Held only to keep the name alive (see above).
    #[allow(dead_code)]
    name: Option<JsStr>,
    ic: PropIc,
}

const MEGA_BITS: u32 = 12;

#[inline(always)]
fn mega_index(shape: u32, key: u64) -> usize {
    let h = (shape as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ key.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    (h >> (64 - MEGA_BITS)) as usize
}

/// Neither a receiver with shape `shape` and prototype `mid` (`NO_OBJ`:
/// none) nor its prototype chain has the key: the read is `undefined`.
/// Valid while the prototype epoch is `mid_shape`.
pub const IC_ABSENT: u8 = 12;

/// Debug switches: `RH_NO_MEGA` disables the shared (shape, name) cache,
/// `RH_NO_DEEP_IC` the prototype-epoch caches (deep chains, absent
/// properties, deep additions, inherited setters).
fn ic_disabled(which: u8) -> bool {
    static FLAGS: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    let f = *FLAGS.get_or_init(|| (std::env::var_os("RH_NO_MEGA").is_some() as u8) | ((std::env::var_os("RH_NO_DEEP_IC").is_some() as u8) << 1));
    f & which != 0
}

/// Longest prototype chain walked when filling a deep cache.
const MAX_CHAIN: usize = 48;

/// Object id 0 is a valid id, so "no object" is encoded as `u32::MAX`.
pub const NO_OBJ: u32 = u32::MAX;

/// Script-level `let`/`const`/`class` bindings, indexed so inline caches can
/// address them. `epoch` changes whenever a name is added (a new lexical
/// binding may shadow a global object property cached elsewhere).
#[derive(Default)]
pub struct GlobalLex {
    index: HashMap<JsStr, u32, FxBuild>,
    pub vals: Vec<(Value, bool)>,
    pub epoch: u32,
}

impl GlobalLex {
    pub fn get(&self, name: &JsStr) -> Option<&(Value, bool)> {
        self.index.get(name).map(|&i| &self.vals[i as usize])
    }
    pub fn slot_of(&self, name: &JsStr) -> Option<u32> {
        self.index.get(name).copied()
    }
    pub fn insert(&mut self, name: JsStr, v: (Value, bool)) {
        match self.index.get(&name) {
            Some(&i) => self.vals[i as usize] = v,
            None => {
                self.index.insert(name, self.vals.len() as u32);
                self.vals.push(v);
                self.epoch = self.epoch.wrapping_add(1);
            }
        }
    }
    pub fn values(&self) -> impl Iterator<Item = &(Value, bool)> {
        self.vals.iter()
    }
}

/// Keys that inline caches handle: plain string names that are not array
/// indices, numeric strings (typed arrays) or `length` (handled specially).
fn cacheable_key(key: &PropertyKey) -> Option<&JsStr> {
    match key {
        PropertyKey::String(s) => {
            if s.is_empty() || s.len() > 64 {
                return None;
            }
            let c = s.as_units()[0];
            // Digits, '-', 'I'nfinity and 'N'aN can be canonical numeric strings.
            if (b'0' as u16..=b'9' as u16).contains(&c) || c == b'-' as u16 || c == b'I' as u16 || c == b'N' as u16 {
                if crate::builtins::typed_array::canonical_numeric_index(key).is_some() {
                    return None;
                }
            }
            Some(s)
        }
        PropertyKey::Symbol(_) => None,
    }
}

/// A receiver whose reads and writes go through the inline caches at all:
/// proxies and module namespaces share the empty shape but forward
/// elsewhere, so no record may match them.
#[inline(always)]
fn cacheable_receiver(o: &Object) -> bool {
    !matches!(o.kind, ObjectKind::Proxy { .. } | ObjectKind::ModuleBinding { .. })
}

fn plain_kind(kind: &ObjectKind) -> bool {
    !matches!(kind, ObjectKind::Proxy { .. } | ObjectKind::TypedArray(_) | ObjectKind::ModuleBinding { .. })
}

impl Realm {
    /// Wrapper prototype used for property lookups on a primitive.
    #[inline]
    pub(crate) fn prim_proto(&self, v: &Value) -> Option<ObjectId> {
        match v.v() {
            V::String(_) => Some(self.intrinsics.string_proto),
            V::Number(_) => Some(self.intrinsics.number_proto),
            V::Bool(_) => Some(self.intrinsics.boolean_proto),
            V::Symbol(_) => Some(self.intrinsics.symbol_proto),
            _ => None,
        }
    }

    /// Fast path of a cached property read. `None` means "take the slow path".
    #[inline]
    pub(crate) fn ic_get(&self, ic: &PropIc, recv: &Value) -> Option<&Property> {
        match ic.kind {
            IC_OWN => {
                let o = self.heap.try_get(recv.as_object()?)?;
                if o.props.shape_id() == ic.shape {
                    o.props.slot(ic.slot)
                } else {
                    None
                }
            }
            IC_PROTO => {
                let o = self.heap.try_get(recv.as_object()?)?;
                if o.props.shape_id() != ic.shape || !cacheable_receiver(o) {
                    return None;
                }
                let mut next = o.proto?;
                if ic.mid != NO_OBJ {
                    if next.0 != ic.mid {
                        return None;
                    }
                    let m = self.heap.try_get(next)?;
                    if m.props.shape_id() != ic.mid_shape {
                        return None;
                    }
                    next = m.proto?;
                }
                if next.0 != ic.holder {
                    return None;
                }
                let h = self.heap.try_get(next)?;
                if h.props.shape_id() == ic.holder_shape {
                    h.props.slot(ic.slot)
                } else {
                    None
                }
            }
            IC_PRIM => {
                if self.prim_proto(recv)?.0 != ic.holder {
                    return None;
                }
                let h = self.heap.try_get(ObjectId(ic.holder))?;
                if h.props.shape_id() == ic.holder_shape {
                    h.props.slot(ic.slot)
                } else {
                    None
                }
            }
            IC_PROTO_DEEP => {
                let o = self.heap.try_get(recv.as_object()?)?;
                if o.props.shape_id() != ic.shape || o.proto?.0 != ic.mid || crate::shape::proto_epoch() != ic.mid_shape || !cacheable_receiver(o) {
                    return None;
                }
                let h = self.heap.try_get(ObjectId(ic.holder))?;
                if h.props.shape_id() == ic.holder_shape {
                    h.props.slot(ic.slot)
                } else {
                    None
                }
            }
            IC_ABSENT => {
                let o = self.heap.try_get(recv.as_object()?)?;
                if o.props.shape_id() != ic.shape || o.proto.map(|p| p.0).unwrap_or(NO_OBJ) != ic.mid || crate::shape::proto_epoch() != ic.mid_shape || !cacheable_receiver(o) {
                    return None;
                }
                Some(&ABSENT_PROPERTY)
            }
            IC_OWN_DICT => None,
            _ => None,
        }
    }

    /// Megamorphic lookup of a read of `key` (the interned name's word)
    /// from object `recv`.
    #[inline]
    pub(crate) fn mega_get(&self, recv: &Value, key: u64) -> Option<PropIc> {
        let o = self.heap.try_get(recv.as_object()?)?;
        let shape = o.props.shape_id();
        let e = self.mega.get(mega_index(shape, key))?;
        if e.shape == shape && e.key == key && e.ic.kind != IC_NONE {
            Some(e.ic)
        } else {
            None
        }
    }

    /// Remember a filled read cache record for `recv` / `key` in the
    /// megamorphic cache.
    pub(crate) fn mega_put(&mut self, recv: &Value, key: &PropertyKey, ic: PropIc) {
        if ic.kind == IC_NONE || ic.kind == IC_LENGTH || ic.kind == IC_PRIM || ic_disabled(1) {
            return;
        }
        let (Some(id), PropertyKey::String(k)) = (recv.as_object(), key) else { return };
        let Some(o) = self.heap.try_get(id) else { return };
        let shape = o.props.shape_id();
        if self.mega.is_empty() {
            self.mega = vec![MegaEntry::default(); 1 << MEGA_BITS];
        }
        let kb = k.bits();
        self.mega[mega_index(shape, kb)] = MegaEntry { shape, key: kb, name: Some(k.clone()), ic };
    }

    /// Remember a filled store cache record in the megamorphic cache, under
    /// the receiver's shape before the store.
    pub(crate) fn mega_put_set(&mut self, recv: &Value, key: &PropertyKey, before: Option<(u32, Option<u32>)>, ic: PropIc) {
        if !matches!(ic.kind, IC_OWN | IC_ADD | IC_ADD_DEEP | IC_SETTER) || ic_disabled(1) {
            return;
        }
        let (Some((shape, _)), Some(_), PropertyKey::String(k)) = (before, recv.as_object(), key) else { return };
        if self.mega.is_empty() {
            self.mega = vec![MegaEntry::default(); 1 << MEGA_BITS];
        }
        let kb = k.bits() | 1;
        self.mega[mega_index(shape, kb)] = MegaEntry { shape, key: kb, name: Some(k.clone()), ic };
    }

    /// Walk the prototype chain above `first` (the receiver's prototype)
    /// for `key`, marking every object passed so their changes invalidate
    /// deep caches. Returns (holder, holder shape id, slot).
    fn deep_find(&self, first: ObjectId, key: &PropertyKey) -> Option<(ObjectId, u32, u32)> {
        if ic_disabled(2) {
            return None;
        }
        let mut cur = Some(first);
        let mut n = 0;
        while let Some(c) = cur {
            n += 1;
            if n > MAX_CHAIN {
                return None;
            }
            let h = self.heap.try_get(c)?;
            if !plain_kind(&h.kind) || matches!(h.kind, ObjectKind::Array { .. } | ObjectKind::String(_)) {
                return None;
            }
            h.props.mark_in_chain();
            if let Some(slot) = h.props.find_slot(key) {
                return Some((c, h.props.shape_id(), slot));
            }
            cur = h.proto;
        }
        None
    }

    /// The whole chain above `first` lacks `key` (marking each object).
    fn deep_absent(&self, first: Option<ObjectId>, key: &PropertyKey) -> bool {
        if ic_disabled(2) {
            return false;
        }
        let mut cur = first;
        let mut n = 0;
        while let Some(c) = cur {
            n += 1;
            if n > MAX_CHAIN {
                return false;
            }
            let Some(h) = self.heap.try_get(c) else { return false };
            if !plain_kind(&h.kind) || matches!(h.kind, ObjectKind::Array { .. } | ObjectKind::String(_)) {
                return false;
            }
            h.props.mark_in_chain();
            if h.props.find_slot(key).is_some() {
                return false;
            }
            cur = h.proto;
        }
        true
    }

    /// Record how `key` was read from `recv` (after a successful slow read).
    pub(crate) fn ic_fill_get(&self, recv: &Value, key: &PropertyKey) -> PropIc {
        let mut ic = PropIc::default();
        // `length` is virtual on arrays and String objects: those are never
        // cached through their property maps (arrays and string primitives
        // get IC_LENGTH); other receivers (typed arrays, functions, plain
        // objects) cache it like any other key.
        let len_key = matches!(key, PropertyKey::String(s) if s.eq_str("length"));
        if len_key {
            match recv.v() {
                V::String(_) => {
                    ic.kind = IC_LENGTH;
                    return ic;
                }
                V::Object(id) if matches!(self.obj(id).kind, ObjectKind::Array { .. }) => {
                    ic.kind = IC_LENGTH;
                    return ic;
                }
                _ => {}
            }
        }
        let virtual_length = |k: &ObjectKind| len_key && matches!(k, ObjectKind::Array { .. } | ObjectKind::String(_));
        if cacheable_key(key).is_none() {
            return ic;
        }
        let (start, prim) = match recv.v() {
            V::Object(id) => (id, false),
            other => match self.prim_proto(&other.to_value()) {
                Some(p) => (p, true),
                None => return ic,
            },
        };
        let o = self.obj(start);
        // Typed arrays are ordinary for keys that are not canonical numeric
        // strings (`cacheable_key` excludes those): `ta.length` and method
        // reads are cacheable.
        if (!plain_kind(&o.kind) && !matches!(o.kind, ObjectKind::TypedArray(_))) || virtual_length(&o.kind) {
            return ic;
        }
        if let Some(slot) = o.props.find_slot(key) {
            ic.kind = if prim { IC_PRIM } else { IC_OWN };
            ic.slot = slot;
            if prim {
                ic.holder = start.0;
                ic.holder_shape = o.props.shape_id();
            } else {
                ic.shape = o.props.shape_id();
            }
            return ic;
        }
        if prim {
            return ic;
        }
        // Up to two prototype hops; every object passed must lack the key
        // (shape ids prove the absence stays true).
        let Some(p1) = o.proto else {
            if plain_kind(&o.kind) && !ic_disabled(2) {
                ic.kind = IC_ABSENT;
                ic.shape = o.props.shape_id();
                ic.mid = NO_OBJ;
                ic.mid_shape = crate::shape::proto_epoch();
            }
            return ic;
        };
        let h1 = self.obj(p1);
        if !plain_kind(&h1.kind) || virtual_length(&h1.kind) {
            return ic;
        }
        if let Some(slot) = h1.props.find_slot(key) {
            ic.kind = IC_PROTO;
            ic.shape = o.props.shape_id();
            ic.mid = NO_OBJ;
            ic.holder = p1.0;
            ic.holder_shape = h1.props.shape_id();
            ic.slot = slot;
            return ic;
        }
        let Some(p2) = h1.proto else { return ic };
        let h2 = self.obj(p2);
        if !plain_kind(&h2.kind) || virtual_length(&h2.kind) {
            return ic;
        }
        if let Some(slot) = h2.props.find_slot(key) {
            ic.kind = IC_PROTO;
            ic.shape = o.props.shape_id();
            ic.mid = p1.0;
            ic.mid_shape = h1.props.shape_id();
            ic.holder = p2.0;
            ic.holder_shape = h2.props.shape_id();
            ic.slot = slot;
            return ic;
        }
        // Deeper: class hierarchies, DOM interfaces (element -> HTMLElement
        // -> Element -> Node -> EventTarget).
        let epoch = crate::shape::proto_epoch();
        if let Some((holder, hs, slot)) = self.deep_find(p1, key) {
            // Marking may not have changed anything cached: the epoch only
            // moves on later changes.
            ic.kind = IC_PROTO_DEEP;
            ic.shape = o.props.shape_id();
            ic.mid = p1.0;
            ic.mid_shape = epoch;
            ic.holder = holder.0;
            ic.holder_shape = hs;
            ic.slot = slot;
        } else if plain_kind(&o.kind) && self.deep_absent(Some(p1), key) {
            ic.kind = IC_ABSENT;
            ic.shape = o.props.shape_id();
            ic.mid = p1.0;
            ic.mid_shape = epoch;
        }
        ic
    }

    /// Fast path of a cached store of `v` into `recv.key`. Returns `v` back
    /// when the cache does not apply.
    #[inline]
    pub(crate) fn ic_set(&mut self, ic: &PropIc, recv: &Value, key: &JsStr, v: Value) -> Result<(), Value> {
        let Some(id) = recv.as_object() else { return Err(v) };
        match ic.kind {
            IC_OWN => {
                let o = match self.heap.try_get_mut(id) {
                    Some(o) => o,
                    None => return Err(v),
                };
                if o.props.shape_id() != ic.shape {
                    return Err(v);
                }
                match o.props.slot_mut(ic.slot) {
                    Some(p) if p.flags & (ACCESSOR | WRITABLE) == WRITABLE => {
                        p.value = v;
                        Ok(())
                    }
                    _ => Err(v),
                }
            }
            IC_ADD_DEEP => {
                let o = match self.heap.try_get_mut(id) {
                    Some(o) => o,
                    None => return Err(v),
                };
                if o.props.shape_id() != ic.shape || !o.extensible || o.proto.map(|p| p.0) != Some(ic.holder) || crate::shape::proto_epoch() != ic.mid_shape || !cacheable_receiver(o) {
                    return Err(v);
                }
                let next = match o.props.shape().and_then(|s| s.lookup_transition_str(key)) {
                    Some(n) if n.id == ic.slot => n,
                    _ => return Err(v),
                };
                o.props.push_with_shape(next, Property::data(v, ALL));
                Ok(())
            }
            IC_ADD => {
                if !self.ic_add_valid(ic, id) {
                    return Err(v);
                }
                let o = self.heap.get_mut(id);
                let next = match o.props.shape().and_then(|s| s.lookup_transition_str(key)) {
                    Some(n) if n.id == ic.slot => n,
                    _ => return Err(v),
                };
                o.props.push_with_shape(next, Property::data(v, ALL));
                Ok(())
            }
            _ => Err(v),
        }
    }

    /// The setter an `IC_SETTER` record finds for `recv`, if still valid.
    #[inline]
    pub(crate) fn ic_setter(&self, ic: &PropIc, recv: &Value) -> Option<ObjectId> {
        let o = self.heap.try_get(recv.as_object()?)?;
        if o.props.shape_id() != ic.shape || !cacheable_receiver(o) {
            return None;
        }
        let p = if ic.holder == NO_OBJ {
            o.props.slot(ic.slot)?
        } else {
            if o.proto?.0 != ic.mid || crate::shape::proto_epoch() != ic.mid_shape {
                return None;
            }
            let h = self.heap.try_get(ObjectId(ic.holder))?;
            if h.props.shape_id() != ic.holder_shape {
                return None;
            }
            h.props.slot(ic.slot)?
        };
        if p.flags & ACCESSOR == 0 {
            return None;
        }
        p.setter()?.as_object()
    }

    /// The receiver and its prototype chain still match an `IC_ADD` record.
    #[inline]
    fn ic_add_valid(&self, ic: &PropIc, id: ObjectId) -> bool {
        let Some(o) = self.heap.try_get(id) else { return false };
        if o.props.shape_id() != ic.shape || !o.extensible || !cacheable_receiver(o) {
            return false;
        }
        if ic.holder == NO_OBJ {
            return o.proto.is_none();
        }
        let Some(p1) = o.proto else { return false };
        if p1.0 != ic.holder {
            return false;
        }
        let Some(h1) = self.heap.try_get(p1) else { return false };
        if h1.props.shape_id() != ic.holder_shape {
            return false;
        }
        if ic.mid == NO_OBJ {
            return h1.proto.is_none();
        }
        let Some(p2) = h1.proto else { return false };
        if p2.0 != ic.mid {
            return false;
        }
        let Some(h2) = self.heap.try_get(p2) else { return false };
        h2.props.shape_id() == ic.mid_shape && h2.proto.is_none()
    }

    /// Snapshot taken before a slow store, used to recognise a cacheable
    /// outcome afterwards: (shape id, slot of the key if present).
    pub(crate) fn ic_before_set(&self, recv: &Value, key: &PropertyKey) -> Option<(u32, Option<u32>)> {
        let id = recv.as_object()?;
        cacheable_key(key)?;
        let o = self.heap.try_get(id)?;
        if !plain_kind(&o.kind) {
            return None;
        }
        Some((o.props.shape_id(), o.props.find_slot(key)))
    }

    /// Record a slow store's outcome (`define` stores skip the prototype
    /// chain requirements).
    pub(crate) fn ic_fill_set(&self, recv: &Value, key: &PropertyKey, before: Option<(u32, Option<u32>)>, define: bool) -> PropIc {
        let mut ic = PropIc::default();
        let (Some((before_id, before_slot)), Some(id)) = (before, recv.as_object()) else { return ic };
        let o = self.obj(id);
        if o.props.is_dict() && before_slot.is_none() {
            // Additions to dictionaries are not cached (no transitions).
            return ic;
        }
        match before_slot {
            Some(slot) => {
                if o.props.shape_id() == before_id {
                    if let Some(p) = o.props.slot(slot) {
                        if p.flags & (ACCESSOR | WRITABLE) == WRITABLE {
                            ic.kind = IC_OWN;
                            ic.shape = before_id;
                            ic.slot = slot;
                        } else if p.flags & ACCESSOR != 0 && p.setter().is_some() && !define {
                            ic.kind = IC_SETTER;
                            ic.shape = before_id;
                            ic.holder = NO_OBJ;
                            ic.slot = slot;
                        }
                    }
                }
            }
            None => {
                if !define && o.props.shape_id() == before_id && o.props.find_slot(key).is_none() {
                    // Not added: an inherited setter took it.
                    let epoch = crate::shape::proto_epoch();
                    let Some(p1) = o.proto else { return ic };
                    if let Some((holder, hs, slot)) = self.deep_find(p1, key) {
                        let h = self.obj(holder);
                        if let Some(p) = h.props.slot(slot) {
                            if p.flags & ACCESSOR != 0 && p.setter().is_some() {
                                ic.kind = IC_SETTER;
                                ic.shape = before_id;
                                ic.mid = p1.0;
                                ic.mid_shape = epoch;
                                ic.holder = holder.0;
                                ic.holder_shape = hs;
                                ic.slot = slot;
                            }
                        }
                    }
                    return ic;
                }
                // Added as the newest property?
                let Some(shape) = o.props.shape() else { return ic };
                let n = shape.len();
                if n == 0 || o.props.find_slot(key) != Some((n - 1) as u32) {
                    return ic;
                }
                match o.props.slot((n - 1) as u32) {
                    Some(p) if p.flags == ALL => {}
                    _ => return ic,
                }
                ic.kind = IC_ADD;
                ic.shape = before_id;
                ic.slot = shape.id;
                if define {
                    // Object literals: the chain is irrelevant, but the
                    // validator still checks it; record it as it is.
                }
                // Record up to two prototypes, each lacking the key.
                ic.holder = NO_OBJ;
                ic.mid = NO_OBJ;
                if let Some(p1) = o.proto {
                    let h1 = self.obj(p1);
                    if !plain_kind(&h1.kind) || h1.props.find_slot(key).is_some() {
                        return PropIc::default();
                    }
                    ic.holder = p1.0;
                    ic.holder_shape = h1.props.shape_id();
                    if let Some(p2) = h1.proto {
                        let h2 = self.obj(p2);
                        if !plain_kind(&h2.kind) || h2.props.find_slot(key).is_some() {
                            return PropIc::default();
                        }
                        if h2.proto.is_some() {
                            // Longer chain: validated by the prototype epoch.
                            let epoch = crate::shape::proto_epoch();
                            if define || !self.deep_absent(Some(p1), key) {
                                return PropIc::default();
                            }
                            ic.kind = IC_ADD_DEEP;
                            ic.holder = p1.0;
                            ic.mid_shape = epoch;
                            return ic;
                        }
                        ic.mid = p2.0;
                        ic.mid_shape = h2.props.shape_id();
                    }
                }
            }
        }
        ic
    }

    /// Fast path of a cached global variable read: the record's cell
    /// pointer (the binding's value) while it is valid, else the slot-based
    /// check, which also refreshes the cell pointer.
    #[inline(always)]
    pub(crate) fn ic_get_global<'a>(&'a self, cell: &std::cell::Cell<PropIc>, key: &JsStr) -> Option<&'a Value> {
        let ic = cell.get();
        if ic.shape != self.global_lex.epoch {
            return None;
        }
        if ic.kind == IC_GLOBAL_LEX {
            // Lexical bindings are never removed: the slot stays valid.
            return match self.global_lex.vals.get(ic.slot as usize) {
                Some((v, _)) if !v.is_empty() => Some(v),
                _ => None,
            };
        }
        if ic.kind == IC_GLOBAL_OBJ && ic.holder == crate::shape::proto_epoch() {
            // SAFETY: the pointer was taken from the global object's
            // property storage, which has not moved since: key-set changes
            // of the global object (marked as watched) advance the epoch.
            let p = unsafe { &*(global_cell_ptr(&ic) as *const Property) };
            if p.flags & ACCESSOR == 0 {
                return Some(&p.value);
            }
            return None;
        }
        self.ic_get_global_slow(cell, key)
    }

    #[inline(never)]
    fn ic_get_global_slow<'a>(&'a self, cell: &std::cell::Cell<PropIc>, key: &JsStr) -> Option<&'a Value> {
        let mut ic = cell.get();
        if ic.shape != self.global_lex.epoch {
            return None;
        }
        match ic.kind {
            IC_GLOBAL_LEX => match self.global_lex.vals.get(ic.slot as usize) {
                Some((v, _)) if !v.is_empty() => Some(v),
                _ => None,
            },
            IC_GLOBAL_OBJ => {
                let g = self.heap.try_get(self.global)?;
                let p = if ic.holder_shape != 0 {
                    if g.props.shape_id() != ic.holder_shape {
                        return None;
                    }
                    g.props.slot(ic.slot)?
                } else {
                    g.props.dict_slot_if(ic.slot, key)?
                };
                if p.flags & ACCESSOR != 0 {
                    return None;
                }
                g.props.mark_in_chain();
                set_global_cell_ptr(&mut ic, p as *const Property as usize);
                ic.holder = crate::shape::proto_epoch();
                cell.set(ic);
                Some(&p.value)
            }
            _ => None,
        }
    }

    /// Fast path of a cached global variable store; gives `v` back on a miss.
    #[inline(always)]
    pub(crate) fn ic_set_global(&mut self, cell: &std::cell::Cell<PropIc>, key: &JsStr, v: Value) -> Result<(), Value> {
        let ic = cell.get();
        if ic.shape != self.global_lex.epoch {
            return Err(v);
        }
        if ic.kind == IC_GLOBAL_LEX {
            return match self.global_lex.vals.get_mut(ic.slot as usize) {
                Some((cur, false)) if !cur.is_empty() => {
                    *cur = v;
                    Ok(())
                }
                _ => Err(v),
            };
        }
        if ic.kind == IC_GLOBAL_OBJ && ic.holder == crate::shape::proto_epoch() {
            // SAFETY: as in `ic_get_global`; nothing else borrows the
            // property while this store runs.
            let p = unsafe { &mut *(global_cell_ptr(&ic) as *mut Property) };
            if p.flags & (ACCESSOR | WRITABLE) == WRITABLE {
                p.value = v;
                return Ok(());
            }
            return Err(v);
        }
        self.ic_set_global_slow(cell, key, v)
    }

    #[inline(never)]
    fn ic_set_global_slow(&mut self, cell: &std::cell::Cell<PropIc>, key: &JsStr, v: Value) -> Result<(), Value> {
        let mut ic = cell.get();
        if ic.shape != self.global_lex.epoch {
            return Err(v);
        }
        match ic.kind {
            IC_GLOBAL_LEX => match self.global_lex.vals.get_mut(ic.slot as usize) {
                Some(e) if !e.1 && !e.0.is_empty() => {
                    e.0 = v;
                    Ok(())
                }
                _ => Err(v),
            },
            IC_GLOBAL_OBJ => {
                let gid = self.global;
                let Some(g) = self.heap.try_get_mut(gid) else { return Err(v) };
                g.props.mark_in_chain();
                let p = if ic.holder_shape != 0 {
                    if g.props.shape_id() != ic.holder_shape {
                        return Err(v);
                    }
                    g.props.slot_mut(ic.slot)
                } else {
                    g.props.dict_slot_if_mut(ic.slot, key)
                };
                match p {
                    Some(p) if p.flags & (ACCESSOR | WRITABLE) == WRITABLE => {
                        p.value = v;
                        set_global_cell_ptr(&mut ic, p as *mut Property as usize);
                        ic.holder = crate::shape::proto_epoch();
                        cell.set(ic);
                        Ok(())
                    }
                    _ => Err(v),
                }
            }
            _ => Err(v),
        }
    }

    /// Record where a global name currently resolves.
    pub(crate) fn ic_fill_global(&self, key: &JsStr) -> PropIc {
        let mut ic = PropIc { shape: self.global_lex.epoch, ..Default::default() };
        if let Some(slot) = self.global_lex.slot_of(key) {
            ic.kind = IC_GLOBAL_LEX;
            ic.slot = slot;
            return ic;
        }
        let g = self.obj(self.global);
        if !plain_kind(&g.kind) {
            return PropIc::default();
        }
        let pk = PropertyKey::String(key.clone());
        if let Some(slot) = g.props.find_slot(&pk) {
            ic.kind = IC_GLOBAL_OBJ;
            ic.slot = slot;
            ic.holder_shape = if g.props.is_dict() { 0 } else { g.props.shape_id() };
            if let Some(p) = g.props.slot(slot) {
                // Key-set changes of the global object must advance the
                // prototype epoch: they may move its property storage.
                g.props.mark_in_chain();
                ic.holder = crate::shape::proto_epoch();
                set_global_cell_ptr(&mut ic, p as *const Property as usize);
            }
        }
        ic
    }
}

/// The binding pointer of a global inline cache record.
#[inline(always)]
fn global_cell_ptr(ic: &PropIc) -> usize {
    ((ic.mid_shape as u64) << 32 | ic.mid as u64) as usize
}

fn set_global_cell_ptr(ic: &mut PropIc, p: usize) {
    ic.mid = p as u32;
    ic.mid_shape = ((p as u64) >> 32) as u32;
}
