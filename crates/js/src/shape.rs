//! Hidden classes ("shapes") and the object property storage built on them.
//!
//! Objects with few properties share an immutable [`Shape`] describing their
//! key layout (keys in insertion order → slot index) and keep only a
//! `Vec<Property>` of values. Adding a property follows a transition to a
//! child shape, so objects built the same way end up with the same shape,
//! which is what lets the interpreter's inline caches replace a property
//! lookup by a shape-id comparison plus an indexed load.
//!
//! Shapes record keys only; property attributes live with the values (an
//! attribute change does not change the shape, and caches re-check the
//! attributes they depend on).
//!
//! Objects with many properties, or from which a property was deleted,
//! switch to dictionary mode (insertion-ordered entries plus a hash index).
//! A dictionary gets a fresh id on every change of its key set, so a cached
//! id is still proof that a key is absent or at a known index.

use crate::heap::Property;
use crate::value::{FxBuild, JsStr, PropertyKey};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

thread_local! {
    /// Property vectors of collected objects, reused by new objects (saves
    /// an allocation per object for short-lived ones).
    static VALUE_POOL: RefCell<Vec<Vec<Property>>> = const { RefCell::new(Vec::new()) };
}

/// An empty property vector, recycled when possible.
#[inline]
fn fresh_values() -> Vec<Property> {
    VALUE_POOL.with(|p| p.borrow_mut().pop()).unwrap_or_else(|| Vec::with_capacity(4))
}

/// Largest number of properties kept in shape mode.
pub const MAX_SHAPED: usize = 128;
/// Id of the empty root shape.
pub const ROOT_SHAPE_ID: u32 = 1;
/// Id of the shape of plain functions whose `prototype` object is not
/// created yet (see [`lazy_fn_shape`]).
pub const LAZY_FN_SHAPE_ID: u32 = 2;
/// First id handed out by [`fresh_id`].
const FIRST_FRESH_ID: u32 = 3;
/// Shapes up to this size are searched linearly (larger ones use an index).
const LINEAR_LOOKUP: usize = 8;

thread_local! {
    static NEXT_ID: Cell<u32> = const { Cell::new(FIRST_FRESH_ID) };
    static ROOT: Rc<Shape> = Rc::new(Shape {
        id: ROOT_SHAPE_ID,
        len: 0,
        ids: [0; LINEAR_LOOKUP],
        all_interned: true,
        keys: Rc::new(RefCell::new(Vec::new())),
        parent: None,
        transitions: RefCell::new(Transitions::default()),
        index: Rc::default(),
    });
}

/// A fresh id for a shape or a dictionary key set (never reused; 0 is
/// reserved as "no id").
pub fn fresh_id() -> u32 {
    NEXT_ID.with(|n| {
        let id = n.get();
        n.set(if id == u32::MAX { FIRST_FRESH_ID } else { id + 1 });
        id
    })
}

struct Transitions {
    small: Vec<(PropertyKey, Weak<Shape>)>,
    map: Option<HashMap<PropertyKey, Weak<Shape>, FxBuild>>,
    /// Every key in `small` is an interned string (an interned key that
    /// matches none by identity matches none).
    small_interned: bool,
}

impl Default for Transitions {
    fn default() -> Self {
        Transitions { small: Vec::new(), map: None, small_interned: true }
    }
}

/// Identity word of a key for [`Shape::ids`] (0 for symbols).
#[inline(always)]
fn id_word(key: &PropertyKey) -> u64 {
    match key {
        PropertyKey::String(s) => s.bits(),
        PropertyKey::Symbol(_) => 0,
    }
}

#[inline(always)]
fn interned_string(key: &PropertyKey) -> bool {
    matches!(key, PropertyKey::String(s) if s.is_interned())
}

pub struct Shape {
    pub id: u32,
    len: u32,
    /// Identity words of the first keys (string handles; 0 for symbols):
    /// the usual lookup compares these without leaving the shape.
    ids: [u64; LINEAR_LOOKUP],
    /// Every key is an interned string.
    all_interned: bool,
    /// Key storage shared along a linear transition chain; this shape's
    /// keys are `keys[..len]`.
    keys: Rc<RefCell<Vec<PropertyKey>>>,
    /// Keeps the transition path alive (so equal layouts keep equal ids).
    #[allow(dead_code)]
    parent: Option<Rc<Shape>>,
    transitions: RefCell<Transitions>,
    /// Slot of each key of `keys` (shared with it along the chain, so an
    /// object built one property at a time indexes each key once).
    index: Rc<RefCell<KeyIndex>>,
}

/// Hash index of a shared key vector: the slots of its first `built`
/// keys. A key appears once in the vector (it is appended only to the
/// shape at the end of it, for a key that shape lacks), so a shape of
/// length `len` has the key iff its slot is below `len`.
#[derive(Default)]
struct KeyIndex {
    map: HashMap<PropertyKey, u32, FxBuild>,
    built: usize,
}

impl std::fmt::Debug for Shape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Shape#{}({})", self.id, self.len)
    }
}

#[inline]
fn key_eq(a: &PropertyKey, b: &PropertyKey) -> bool {
    match (a, b) {
        (PropertyKey::String(x), PropertyKey::String(y)) => x == y,
        (PropertyKey::Symbol(x), PropertyKey::Symbol(y)) => x == y,
        _ => false,
    }
}

thread_local! {
    static LAZY_FN: OnceCell<Rc<Shape>> = const { OnceCell::new() };
}

/// The shape of a plain function whose `prototype` property is pending:
/// the keys of `ln` (`length`, `name`) under the reserved id
/// [`LAZY_FN_SHAPE_ID`], reached by no transition. Such a function must be
/// given its prototype (`Realm::materialize_prototype`) before its key set
/// is read or changed, or `prototype` is looked up on it; everything else
/// (its `length` and `name`, inherited properties, inline caches for other
/// keys) works on this shape as on any other.
pub fn lazy_fn_shape(ln: &Rc<Shape>) -> Rc<Shape> {
    LAZY_FN.with(|c| {
        c.get_or_init(|| {
            Rc::new(Shape {
                id: LAZY_FN_SHAPE_ID,
                len: ln.len,
                ids: ln.ids,
                all_interned: ln.all_interned,
                keys: ln.keys.clone(),
                parent: ln.parent.clone(),
                transitions: RefCell::new(Transitions::default()),
                index: OnceCell::new(),
            })
        })
        .clone()
    })
}

/// Is `key` the string `prototype`?
#[inline]
pub fn is_prototype_key(key: &PropertyKey) -> bool {
    matches!(key, PropertyKey::String(s) if s.eq_str("prototype"))
}

impl Shape {
    pub fn root() -> Rc<Shape> {
        ROOT.with(|r| r.clone())
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// Slot of `key`, if present.
    pub fn find(&self, key: &PropertyKey) -> Option<u32> {
        let n = self.len as usize;
        if n <= LINEAR_LOOKUP {
            // Identity first: interned names usually match by pointer.
            if let PropertyKey::String(k) = key {
                let b = k.bits();
                if let Some(i) = self.ids[..n].iter().position(|&w| w == b) {
                    return Some(i as u32);
                }
                // Interned names differ from each other by identity.
                if self.all_interned && k.is_interned() {
                    return None;
                }
            }
            let keys = self.keys.borrow();
            return keys[..n].iter().position(|x| key_eq(x, key)).map(|i| i as u32);
        }
        let mut index = self.index.borrow_mut();
        if index.built < n {
            let keys = self.keys.borrow();
            let KeyIndex { map, built } = &mut *index;
            map.reserve(keys.len() - *built);
            for (i, k) in keys.iter().enumerate().skip(*built) {
                map.insert(k.clone(), i as u32);
            }
            *built = keys.len();
        }
        index.map.get(key).copied().filter(|&i| i < self.len)
    }

    pub fn key_at(&self, slot: usize) -> PropertyKey {
        self.keys.borrow()[slot].clone()
    }

    pub fn keys(&self) -> Vec<PropertyKey> {
        self.keys.borrow()[..self.len as usize].to_vec()
    }

    /// The child shape with `key` appended (created on first use).
    pub fn add(self: &Rc<Shape>, key: &PropertyKey) -> Rc<Shape> {
        if let Some(s) = self.lookup_transition(key) {
            return s;
        }
        let n = self.len as usize;
        let (keys, index) = {
            let mut shared = self.keys.borrow_mut();
            if shared.len() == n {
                shared.push(key.clone());
                (self.keys.clone(), self.index.clone())
            } else {
                let mut v = Vec::with_capacity(n + 4);
                v.extend_from_slice(&shared[..n]);
                v.push(key.clone());
                (Rc::new(RefCell::new(v)), Rc::default())
            }
        };
        let mut ids = self.ids;
        if n < LINEAR_LOOKUP {
            ids[n] = id_word(key);
        }
        let child = Rc::new(Shape {
            id: fresh_id(),
            len: self.len + 1,
            ids,
            all_interned: self.all_interned && interned_string(key),
            keys,
            parent: Some(self.clone()),
            transitions: RefCell::new(Transitions::default()),
            index,
        });
        let mut t = self.transitions.borrow_mut();
        let weak = Rc::downgrade(&child);
        match &mut t.map {
            Some(m) => {
                m.insert(key.clone(), weak);
            }
            None => {
                if t.small.len() >= 16 {
                    t.small.retain(|(_, w)| w.strong_count() > 0);
                }
                if t.small.len() >= 16 {
                    let mut m: HashMap<PropertyKey, Weak<Shape>, FxBuild> = HashMap::default();
                    for (k, w) in t.small.drain(..) {
                        m.insert(k, w);
                    }
                    m.insert(key.clone(), weak);
                    t.map = Some(m);
                } else {
                    t.small_interned &= interned_string(key);
                    t.small.push((key.clone(), weak));
                }
            }
        }
        child
    }

    /// An existing transition for string key `key` (no key construction).
    #[inline]
    pub fn lookup_transition_str(&self, key: &JsStr) -> Option<Rc<Shape>> {
        let t = self.transitions.borrow();
        if t.map.is_none() {
            for (x, w) in t.small.iter() {
                if let PropertyKey::String(s) = x {
                    if s.ptr_eq(key) {
                        return w.upgrade();
                    }
                }
            }
        }
        drop(t);
        self.lookup_transition(&PropertyKey::String(key.clone()))
    }

    /// An existing transition for `key`.
    #[inline]
    pub fn lookup_transition(&self, key: &PropertyKey) -> Option<Rc<Shape>> {
        let t = self.transitions.borrow();
        match &t.map {
            Some(m) => m.get(key).and_then(|w| w.upgrade()),
            None => {
                if let PropertyKey::String(k) = key {
                    for (x, w) in t.small.iter() {
                        if let PropertyKey::String(s) = x {
                            if s.ptr_eq(k) {
                                return w.upgrade();
                            }
                        }
                    }
                    if t.small_interned && k.is_interned() {
                        return None;
                    }
                }
                t.small.iter().find(|(x, _)| key_eq(x, key)).and_then(|(_, w)| w.upgrade())
            }
        }
    }
}

#[derive(Debug, Clone)]
struct Dict {
    id: u32,
    entries: Vec<Option<(PropertyKey, Property)>>,
    index: HashMap<PropertyKey, usize, FxBuild>,
}

#[derive(Debug, Clone)]
enum Repr {
    Shaped(Rc<Shape>),
    Dict(Box<Dict>),
}

/// Iterator over the properties of a [`PropertyMap`] in insertion order
/// (allocates nothing).
pub enum PropIter<'a> {
    Shaped(std::iter::Zip<std::slice::Iter<'a, PropertyKey>, std::slice::Iter<'a, Property>>),
    Dict(std::slice::Iter<'a, Option<(PropertyKey, Property)>>),
}

impl<'a> Iterator for PropIter<'a> {
    type Item = (&'a PropertyKey, &'a Property);
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            PropIter::Shaped(it) => it.next(),
            PropIter::Dict(it) => it.find_map(|e| e.as_ref().map(|(k, p)| (k, p))),
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            PropIter::Shaped(it) => it.size_hint(),
            PropIter::Dict(it) => (0, it.size_hint().1),
        }
    }
}

/// Insertion-ordered property storage (shape mode or dictionary mode).
#[derive(Debug, Clone)]
pub struct PropertyMap {
    repr: Repr,
    /// Values of a shape-mode map (empty in dictionary mode). Outside the
    /// enum so that native code can find them at a fixed offset.
    values: Vec<Property>,
    /// Copy of the shape / dictionary id (read by every inline cache
    /// check without dereferencing the shape).
    sid: u32,
    /// Some inline cache relies on this object as a member of a prototype
    /// chain: key-set changes advance [`proto_epoch`].
    in_chain: Cell<bool>,
}

/// Advanced whenever an object that a deep prototype-chain inline cache
/// went through changes its key set or prototype (see
/// [`PropertyMap::mark_in_chain`]); such caches are valid while it is
/// unchanged.
static PROTO_EPOCH: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

#[inline(always)]
/// Address of the prototype epoch counter (read by native code).
pub(crate) fn proto_epoch_addr() -> usize {
    &PROTO_EPOCH as *const std::sync::atomic::AtomicU32 as usize
}

pub fn proto_epoch() -> u32 {
    PROTO_EPOCH.load(std::sync::atomic::Ordering::Relaxed)
}

#[inline(never)]
pub fn bump_proto_epoch() {
    PROTO_EPOCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

impl Default for PropertyMap {
    fn default() -> Self {
        PropertyMap::new()
    }
}

impl PropertyMap {
    /// Offsets of the shape id and of the shape-mode values (a
    /// `Vec<Property>`) in the map, for native code.
    #[cfg(feature = "jit")]
    pub(crate) const SID_OFFSET: usize = std::mem::offset_of!(PropertyMap, sid);
    #[cfg(feature = "jit")]
    pub(crate) const VALUES_OFFSET: usize = std::mem::offset_of!(PropertyMap, values);

    pub fn new() -> PropertyMap {
        let shape = Shape::root();
        let sid = shape.id;
        PropertyMap { repr: Repr::Shaped(shape), values: Vec::new(), sid, in_chain: Cell::new(false) }
    }

    /// A map in shape mode with `values` for the keys of `shape`, in order
    /// (objects created with a known layout skip the transitions).
    pub fn with_shape(shape: Rc<Shape>, props: &[Property]) -> PropertyMap {
        debug_assert_eq!(shape.len(), props.len());
        let mut values = fresh_values();
        values.extend_from_slice(props);
        let sid = shape.id;
        PropertyMap { repr: Repr::Shaped(shape), values, sid, in_chain: Cell::new(false) }
    }

    /// Shape id (shape mode) or key-set id (dictionary mode). Inline caches
    /// rely on it: in dictionary mode the id changes whenever the key set
    /// changes, and entry indices (slots) are stable while it does not.
    #[inline(always)]
    pub fn shape_id(&self) -> u32 {
        self.sid
    }

    #[inline]
    fn resync(&mut self) {
        let sid = match &self.repr {
            Repr::Shaped(shape) => shape.id,
            Repr::Dict(d) => d.id,
        };
        if sid != self.sid && self.in_chain.get() {
            bump_proto_epoch();
        }
        self.sid = sid;
    }

    /// Mark the object as part of a cached prototype chain (see
    /// [`proto_epoch`]).
    #[inline]
    pub fn mark_in_chain(&self) {
        self.in_chain.set(true);
    }

    #[inline]
    pub fn in_chain(&self) -> bool {
        self.in_chain.get()
    }

    #[inline]
    pub fn is_dict(&self) -> bool {
        matches!(self.repr, Repr::Dict(_))
    }

    /// A plain function's map whose `prototype` is still pending (see
    /// [`lazy_fn_shape`]).
    #[inline(always)]
    pub fn is_lazy_fn(&self) -> bool {
        self.sid == LAZY_FN_SHAPE_ID
    }

    /// End the pending state: switch to `full` (the lazy shape's keys plus
    /// `prototype`) with `prototype` as the new last value.
    pub fn end_lazy_fn(&mut self, full: Rc<Shape>, prototype: Property) {
        debug_assert!(self.is_lazy_fn() && full.len() == self.values.len() + 1);
        self.repr = Repr::Shaped(full);
        self.values.push(prototype);
        self.resync();
    }

    /// Checks (debug builds) that a pending function map is not asked about
    /// `prototype` before it has one.
    #[inline(always)]
    fn check_lazy_key(&self, key: &PropertyKey) {
        debug_assert!(!(self.is_lazy_fn() && is_prototype_key(key)), "`prototype` looked up on a function before it was created");
    }

    pub fn shape(&self) -> Option<&Rc<Shape>> {
        match &self.repr {
            Repr::Shaped(shape) => Some(shape),
            Repr::Dict(_) => None,
        }
    }

    pub fn len(&self) -> usize {
        match &self.repr {
            Repr::Shaped(_) => self.values.len(),
            Repr::Dict(d) => d.index.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Slot of `key`: value index (shape mode) or entry index (dictionary).
    #[inline]
    pub fn find_slot(&self, key: &PropertyKey) -> Option<u32> {
        self.check_lazy_key(key);
        match &self.repr {
            Repr::Shaped(shape) => shape.find(key),
            Repr::Dict(d) => d.index.get(key).map(|&i| i as u32),
        }
    }

    /// The property in `slot` (a slot obtained for the current shape id).
    #[inline(always)]
    pub fn slot(&self, slot: u32) -> Option<&Property> {
        match &self.repr {
            Repr::Shaped(_) => self.values.get(slot as usize),
            Repr::Dict(d) => d.entries.get(slot as usize).and_then(|e| e.as_ref()).map(|(_, p)| p),
        }
    }

    #[inline]
    pub fn slot_mut(&mut self, slot: u32) -> Option<&mut Property> {
        match &mut self.repr {
            Repr::Shaped(_) => self.values.get_mut(slot as usize),
            Repr::Dict(d) => d.entries.get_mut(slot as usize).and_then(|e| e.as_mut()).map(|(_, p)| p),
        }
    }

    /// Dictionary mode: the property at entry `slot` if its key is `key`
    /// (entry indices survive unrelated insertions, so this validates a
    /// cached index without hashing).
    #[inline]
    pub fn dict_slot_if(&self, slot: u32, key: &JsStr) -> Option<&Property> {
        match &self.repr {
            Repr::Dict(d) => match d.entries.get(slot as usize) {
                Some(Some((PropertyKey::String(k), p))) if k == key => Some(p),
                _ => None,
            },
            Repr::Shaped(_) => None,
        }
    }

    #[inline]
    pub fn dict_slot_if_mut(&mut self, slot: u32, key: &JsStr) -> Option<&mut Property> {
        match &mut self.repr {
            Repr::Dict(d) => match d.entries.get_mut(slot as usize) {
                Some(Some((PropertyKey::String(k), p))) if k == key => Some(p),
                _ => None,
            },
            Repr::Shaped(_) => None,
        }
    }

    #[inline]
    pub fn get(&self, key: &PropertyKey) -> Option<&Property> {
        self.check_lazy_key(key);
        match &self.repr {
            Repr::Shaped(shape) => shape.find(key).map(|i| &self.values[i as usize]),
            Repr::Dict(d) => d.index.get(key).and_then(|&i| d.entries[i].as_ref().map(|(_, p)| p)),
        }
    }

    pub fn get_mut(&mut self, key: &PropertyKey) -> Option<&mut Property> {
        self.check_lazy_key(key);
        match &mut self.repr {
            Repr::Shaped(shape) => match shape.find(key) {
                Some(i) => Some(&mut self.values[i as usize]),
                None => None,
            },
            Repr::Dict(d) => match d.index.get(key) {
                Some(&i) => d.entries[i].as_mut().map(|(_, p)| p),
                None => None,
            },
        }
    }

    pub fn contains(&self, key: &PropertyKey) -> bool {
        self.find_slot(key).is_some()
    }

    pub fn insert(&mut self, key: PropertyKey, prop: Property) {
        self.insert_inner(key, prop);
        self.resync();
    }

    fn insert_inner(&mut self, key: PropertyKey, prop: Property) {
        let values = &mut self.values;
        match &mut self.repr {
            Repr::Shaped(shape) => {
                if let Some(i) = shape.find(&key) {
                    values[i as usize] = prop;
                    return;
                }
                debug_assert!(shape.id != LAZY_FN_SHAPE_ID, "property added to a function before its `prototype`");
                if values.len() < MAX_SHAPED {
                    *shape = shape.add(&key);
                    if values.capacity() == 0 {
                        *values = fresh_values();
                    }
                    values.push(prop);
                    return;
                }
                self.to_dict();
                self.insert_inner(key, prop);
            }
            Repr::Dict(d) => {
                if let Some(&i) = d.index.get(&key) {
                    // Same key set: cached entry indices stay valid.
                    d.entries[i] = Some((key, prop));
                } else {
                    d.id = fresh_id();
                    d.index.insert(key.clone(), d.entries.len());
                    d.entries.push(Some((key, prop)));
                }
            }
        }
    }

    /// Append a property known to be absent (inline-cache transition): the
    /// caller has checked that the current shape transitions to `next` by
    /// adding exactly this key.
    #[inline]
    pub fn push_with_shape(&mut self, next: Rc<Shape>, prop: Property) {
        debug_assert!(!self.is_lazy_fn(), "property added to a function before its `prototype`");
        if let Repr::Shaped(shape) = &mut self.repr {
            let values = &mut self.values;
            if self.in_chain.get() {
                bump_proto_epoch();
            }
            self.sid = next.id;
            *shape = next;
            if values.capacity() == 0 {
                *values = fresh_values();
            }
            values.push(prop);
        }
    }

    /// Give the storage of a dead object back for reuse (the map is left
    /// empty).
    pub fn recycle(&mut self) {
        if let Repr::Shaped(_) = &mut self.repr {
            let values = &mut self.values;
            let cap = values.capacity();
            if (4..=16).contains(&cap) {
                let mut v = std::mem::take(values);
                v.clear();
                VALUE_POOL.with(|p| {
                    let mut p = p.borrow_mut();
                    if p.len() < 4096 {
                        p.push(v);
                    }
                });
            }
        }
    }

    pub fn remove(&mut self, key: &PropertyKey) -> Option<Property> {
        let r = self.remove_inner(key);
        self.resync();
        r
    }

    fn remove_inner(&mut self, key: &PropertyKey) -> Option<Property> {
        debug_assert!(!self.is_lazy_fn(), "property deleted from a function before its `prototype` exists");
        if let Repr::Shaped(shape) = &mut self.repr {
            let values = &mut self.values;
            let i = shape.find(key)? as usize;
            // Deleting the last added property returns to the parent layout.
            if i + 1 == values.len() {
                if let Some(parent) = &shape.parent {
                    let parent = parent.clone();
                    *shape = parent;
                    return values.pop();
                }
            }
            self.to_dict();
        }
        match &mut self.repr {
            Repr::Dict(d) => {
                let i = d.index.remove(key)?;
                d.id = fresh_id();
                let out = d.entries[i].take().map(|(_, p)| p);
                if d.entries.len() > 32 && d.index.len() * 2 < d.entries.len() {
                    let old = std::mem::take(&mut d.entries);
                    d.index.clear();
                    for e in old.into_iter().flatten() {
                        d.index.insert(e.0.clone(), d.entries.len());
                        d.entries.push(Some(e));
                    }
                }
                out
            }
            Repr::Shaped(_) => None,
        }
    }

    fn to_dict(&mut self) {
        if let Repr::Shaped(shape) = &mut self.repr {
            let keys = shape.keys();
            let values = std::mem::take(&mut self.values);
            let mut index = HashMap::with_capacity_and_hasher(keys.len(), FxBuild::default());
            let mut entries = Vec::with_capacity(keys.len());
            for (k, p) in keys.into_iter().zip(values) {
                index.insert(k.clone(), entries.len());
                entries.push(Some((k, p)));
            }
            self.repr = Repr::Dict(Box::new(Dict { id: fresh_id(), entries, index }));
        }
    }

    /// Keys in spec order: integer indices ascending, then strings in
    /// insertion order, then symbols in insertion order.
    pub fn keys(&self) -> Vec<PropertyKey> {
        debug_assert!(!self.is_lazy_fn(), "keys of a function listed before its `prototype` exists");
        // Usually every key is a plain string name: insertion order is
        // already the answer.
        let plain = |k: &PropertyKey| matches!(k, PropertyKey::String(s) if s.as_array_index().is_none());
        if self.iter().all(|(k, _)| plain(k)) {
            let mut out = Vec::with_capacity(self.len());
            out.extend(self.iter().map(|(k, _)| k.clone()));
            return out;
        }
        let mut ints: Vec<(u32, PropertyKey)> = Vec::new();
        let mut strs = Vec::new();
        let mut syms = Vec::new();
        for (k, _) in self.iter() {
            match k {
                PropertyKey::Symbol(_) => syms.push(k.clone()),
                PropertyKey::String(s) => match s.as_array_index() {
                    Some(i) => ints.push((i, k.clone())),
                    None => strs.push(k.clone()),
                },
            }
        }
        ints.sort_by_key(|(i, _)| *i);
        let mut out: Vec<PropertyKey> = ints.into_iter().map(|(_, k)| k).collect();
        out.extend(strs);
        out.extend(syms);
        out
    }

    pub fn iter(&self) -> PropIter<'_> {
        match &self.repr {
            Repr::Shaped(shape) => {
                let values = &self.values;
                // The shared key storage is append-only: keys[..len] stay valid
                // while this shape (and hence this borrow) is alive.
                let keys = shape.keys.as_ptr();
                // SAFETY: elements below `len` are never moved or dropped
                // while the shape lives; pushes may reallocate the Vec, so
                // collect references through a snapshot of the pointer
                // only for the duration of this borrow of `self` (no shape
                // transition can happen while `self` is borrowed).
                let slice: &[PropertyKey] = unsafe { &(&*keys)[..values.len()] };
                PropIter::Shaped(slice.iter().zip(values.iter()))
            }
            Repr::Dict(d) => PropIter::Dict(d.entries.iter()),
        }
    }

    /// Visit every property value (insertion order).
    #[inline]
    pub fn for_each_value(&self, mut f: impl FnMut(&Property)) {
        match &self.repr {
            Repr::Shaped(_) => self.values.iter().for_each(f),
            Repr::Dict(d) => {
                for (_, p) in d.entries.iter().flatten() {
                    f(p);
                }
            }
        }
    }

    /// Start loading the property storage (collector look-ahead).
    #[inline(always)]
    pub(crate) fn prefetch(&self) {
        match &self.repr {
            Repr::Shaped(_) => {
                if self.values.capacity() != 0 {
                    crate::alloc::prefetch(self.values.as_ptr());
                }
            }
            Repr::Dict(d) => crate::alloc::prefetch(&**d),
        }
    }

    pub fn values_mut(&mut self) -> Box<dyn Iterator<Item = &mut Property> + '_> {
        match &mut self.repr {
            Repr::Shaped(_) => Box::new(self.values.iter_mut()),
            Repr::Dict(d) => Box::new(d.entries.iter_mut().flatten().map(|(_, p)| p)),
        }
    }
}
