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
pub const IC_GLOBAL_OBJ: u8 = 8;

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

fn plain_kind(kind: &ObjectKind) -> bool {
    !matches!(kind, ObjectKind::Proxy { .. } | ObjectKind::TypedArray(_) | ObjectKind::ModuleBinding { .. })
}

impl Realm {
    /// Wrapper prototype used for property lookups on a primitive.
    #[inline]
    pub(crate) fn prim_proto(&self, v: &Value) -> Option<ObjectId> {
        match v {
            Value::String(_) => Some(self.intrinsics.string_proto),
            Value::Number(_) => Some(self.intrinsics.number_proto),
            Value::Bool(_) => Some(self.intrinsics.boolean_proto),
            Value::Symbol(_) => Some(self.intrinsics.symbol_proto),
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
                if o.props.shape_id() != ic.shape {
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
            IC_OWN_DICT => None,
            _ => None,
        }
    }

    /// Record how `key` was read from `recv` (after a successful slow read).
    pub(crate) fn ic_fill_get(&self, recv: &Value, key: &PropertyKey) -> PropIc {
        let mut ic = PropIc::default();
        if let PropertyKey::String(s) = key {
            if s.eq_str("length") {
                match recv {
                    Value::String(_) => ic.kind = IC_LENGTH,
                    Value::Object(id) if matches!(self.obj(*id).kind, ObjectKind::Array { .. }) => ic.kind = IC_LENGTH,
                    _ => {}
                }
                return ic;
            }
        }
        if cacheable_key(key).is_none() {
            return ic;
        }
        let (start, prim) = match recv {
            Value::Object(id) => (*id, false),
            other => match self.prim_proto(other) {
                Some(p) => (p, true),
                None => return ic,
            },
        };
        let o = self.obj(start);
        if !plain_kind(&o.kind) {
            return ic;
        }
        if let Some(slot) = o.props.find_slot(key) {
            if o.props.is_dict() {
                return ic;
            }
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
        let Some(p1) = o.proto else { return ic };
        let h1 = self.obj(p1);
        if !plain_kind(&h1.kind) {
            return ic;
        }
        if let Some(slot) = h1.props.find_slot(key) {
            if h1.props.is_dict() {
                return ic;
            }
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
        if !plain_kind(&h2.kind) {
            return ic;
        }
        if let Some(slot) = h2.props.find_slot(key) {
            if h2.props.is_dict() {
                return ic;
            }
            ic.kind = IC_PROTO;
            ic.shape = o.props.shape_id();
            ic.mid = p1.0;
            ic.mid_shape = h1.props.shape_id();
            ic.holder = p2.0;
            ic.holder_shape = h2.props.shape_id();
            ic.slot = slot;
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
            IC_ADD => {
                if !self.ic_add_valid(ic, id) {
                    return Err(v);
                }
                let o = self.heap.get_mut(id);
                let next = match o.props.shape().and_then(|s| s.lookup_transition(&PropertyKey::String(key.clone()))) {
                    Some(n) if n.id == ic.slot => n,
                    _ => return Err(v),
                };
                o.props.push_with_shape(next, Property::data(v, ALL));
                Ok(())
            }
            _ => Err(v),
        }
    }

    /// The receiver and its prototype chain still match an `IC_ADD` record.
    #[inline]
    fn ic_add_valid(&self, ic: &PropIc, id: ObjectId) -> bool {
        let Some(o) = self.heap.try_get(id) else { return false };
        if o.props.shape_id() != ic.shape || !o.extensible {
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
        if !plain_kind(&o.kind) || o.props.is_dict() {
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
        if o.props.is_dict() {
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
                        }
                    }
                }
            }
            None => {
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
                        if !plain_kind(&h2.kind) || h2.props.find_slot(key).is_some() || h2.proto.is_some() {
                            return PropIc::default();
                        }
                        ic.mid = p2.0;
                        ic.mid_shape = h2.props.shape_id();
                    }
                }
            }
        }
        ic
    }

    /// Fast path of a cached global variable read.
    #[inline]
    pub(crate) fn ic_get_global(&self, ic: &PropIc, key: &JsStr) -> Option<&Value> {
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
                Some(&p.value)
            }
            _ => None,
        }
    }

    /// Fast path of a cached global variable store; gives `v` back on a miss.
    #[inline]
    pub(crate) fn ic_set_global(&mut self, ic: &PropIc, key: &JsStr, v: Value) -> Result<(), Value> {
        if ic.shape != self.global_lex.epoch {
            return Err(v);
        }
        match ic.kind {
            IC_GLOBAL_LEX => match self.global_lex.vals.get_mut(ic.slot as usize) {
                Some((cur, false)) if !cur.is_empty() => {
                    *cur = v;
                    Ok(())
                }
                _ => Err(v),
            },
            IC_GLOBAL_OBJ => {
                let gid = self.global;
                let Some(g) = self.heap.try_get_mut(gid) else { return Err(v) };
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
        }
        ic
    }
}
