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
use std::cell::{Cell, OnceCell, RefCell};
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
/// Shapes up to this size are searched linearly (larger ones use an index).
const LINEAR_LOOKUP: usize = 8;

thread_local! {
    static NEXT_ID: Cell<u32> = const { Cell::new(2) };
    static ROOT: Rc<Shape> = Rc::new(Shape {
        id: 1,
        len: 0,
        keys: Rc::new(RefCell::new(Vec::new())),
        parent: None,
        transitions: RefCell::new(Transitions::default()),
        index: OnceCell::new(),
    });
}

/// A fresh id for a shape or a dictionary key set (never reused; 0 is
/// reserved as "no id").
pub fn fresh_id() -> u32 {
    NEXT_ID.with(|n| {
        let id = n.get();
        n.set(if id == u32::MAX { 2 } else { id + 1 });
        id
    })
}

#[derive(Default)]
struct Transitions {
    small: Vec<(PropertyKey, Weak<Shape>)>,
    map: Option<HashMap<PropertyKey, Weak<Shape>, FxBuild>>,
}

pub struct Shape {
    pub id: u32,
    len: u32,
    /// Key storage shared along a linear transition chain; this shape's
    /// keys are `keys[..len]`.
    keys: Rc<RefCell<Vec<PropertyKey>>>,
    /// Keeps the transition path alive (so equal layouts keep equal ids).
    #[allow(dead_code)]
    parent: Option<Rc<Shape>>,
    transitions: RefCell<Transitions>,
    index: OnceCell<HashMap<PropertyKey, u32, FxBuild>>,
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
            let keys = self.keys.borrow();
            // Identity first: interned names usually match by pointer.
            if let PropertyKey::String(k) = key {
                for (i, x) in keys[..n].iter().enumerate() {
                    if let PropertyKey::String(s) = x {
                        if s.ptr_eq(k) {
                            return Some(i as u32);
                        }
                    }
                }
            }
            return keys[..n].iter().position(|x| key_eq(x, key)).map(|i| i as u32);
        }
        let index = self.index.get_or_init(|| {
            let keys = self.keys.borrow();
            let mut m = HashMap::with_capacity_and_hasher(n, FxBuild::default());
            for (i, k) in keys[..n].iter().enumerate() {
                m.insert(k.clone(), i as u32);
            }
            m
        });
        index.get(key).copied()
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
        let keys = {
            let mut shared = self.keys.borrow_mut();
            if shared.len() == n {
                shared.push(key.clone());
                self.keys.clone()
            } else {
                let mut v = Vec::with_capacity(n + 4);
                v.extend_from_slice(&shared[..n]);
                v.push(key.clone());
                Rc::new(RefCell::new(v))
            }
        };
        let child = Rc::new(Shape {
            id: fresh_id(),
            len: self.len + 1,
            keys,
            parent: Some(self.clone()),
            transitions: RefCell::new(Transitions::default()),
            index: OnceCell::new(),
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
    Shaped { shape: Rc<Shape>, values: Vec<Property> },
    Dict(Box<Dict>),
}

/// Insertion-ordered property storage (shape mode or dictionary mode).
#[derive(Debug, Clone)]
pub struct PropertyMap {
    repr: Repr,
    /// Copy of the shape / dictionary id (read by every inline cache
    /// check without dereferencing the shape).
    sid: u32,
}

impl Default for PropertyMap {
    fn default() -> Self {
        PropertyMap::new()
    }
}

impl PropertyMap {
    pub fn new() -> PropertyMap {
        let shape = Shape::root();
        let sid = shape.id;
        PropertyMap { repr: Repr::Shaped { shape, values: Vec::new() }, sid }
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
        self.sid = match &self.repr {
            Repr::Shaped { shape, .. } => shape.id,
            Repr::Dict(d) => d.id,
        };
    }

    #[inline]
    pub fn is_dict(&self) -> bool {
        matches!(self.repr, Repr::Dict(_))
    }

    pub fn shape(&self) -> Option<&Rc<Shape>> {
        match &self.repr {
            Repr::Shaped { shape, .. } => Some(shape),
            Repr::Dict(_) => None,
        }
    }

    pub fn len(&self) -> usize {
        match &self.repr {
            Repr::Shaped { values, .. } => values.len(),
            Repr::Dict(d) => d.index.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Slot of `key`: value index (shape mode) or entry index (dictionary).
    #[inline]
    pub fn find_slot(&self, key: &PropertyKey) -> Option<u32> {
        match &self.repr {
            Repr::Shaped { shape, .. } => shape.find(key),
            Repr::Dict(d) => d.index.get(key).map(|&i| i as u32),
        }
    }

    /// The property in `slot` (a slot obtained for the current shape id).
    #[inline(always)]
    pub fn slot(&self, slot: u32) -> Option<&Property> {
        match &self.repr {
            Repr::Shaped { values, .. } => values.get(slot as usize),
            Repr::Dict(d) => d.entries.get(slot as usize).and_then(|e| e.as_ref()).map(|(_, p)| p),
        }
    }

    #[inline]
    pub fn slot_mut(&mut self, slot: u32) -> Option<&mut Property> {
        match &mut self.repr {
            Repr::Shaped { values, .. } => values.get_mut(slot as usize),
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
            Repr::Shaped { .. } => None,
        }
    }

    #[inline]
    pub fn dict_slot_if_mut(&mut self, slot: u32, key: &JsStr) -> Option<&mut Property> {
        match &mut self.repr {
            Repr::Dict(d) => match d.entries.get_mut(slot as usize) {
                Some(Some((PropertyKey::String(k), p))) if k == key => Some(p),
                _ => None,
            },
            Repr::Shaped { .. } => None,
        }
    }

    #[inline]
    pub fn get(&self, key: &PropertyKey) -> Option<&Property> {
        match &self.repr {
            Repr::Shaped { shape, values } => shape.find(key).map(|i| &values[i as usize]),
            Repr::Dict(d) => d.index.get(key).and_then(|&i| d.entries[i].as_ref().map(|(_, p)| p)),
        }
    }

    pub fn get_mut(&mut self, key: &PropertyKey) -> Option<&mut Property> {
        match &mut self.repr {
            Repr::Shaped { shape, values } => match shape.find(key) {
                Some(i) => Some(&mut values[i as usize]),
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
        match &mut self.repr {
            Repr::Shaped { shape, values } => {
                if let Some(i) = shape.find(&key) {
                    values[i as usize] = prop;
                    return;
                }
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
        if let Repr::Shaped { shape, values } = &mut self.repr {
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
        if let Repr::Shaped { values, .. } = &mut self.repr {
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
        if let Repr::Shaped { shape, values } = &mut self.repr {
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
            Repr::Shaped { .. } => None,
        }
    }

    fn to_dict(&mut self) {
        if let Repr::Shaped { shape, values } = &mut self.repr {
            let keys = shape.keys();
            let values = std::mem::take(values);
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

    pub fn iter(&self) -> Box<dyn Iterator<Item = (&PropertyKey, &Property)> + '_> {
        match &self.repr {
            Repr::Shaped { shape, values } => {
                // The shared key storage is append-only: keys[..len] stay valid
                // while this shape (and hence this borrow) is alive.
                let keys = shape.keys.as_ptr();
                // SAFETY: elements below `len` are never moved or dropped
                // while the shape lives; pushes may reallocate the Vec, so
                // collect references through a snapshot of the pointer
                // only for the duration of this borrow of `self` (no shape
                // transition can happen while `self` is borrowed).
                let slice: &[PropertyKey] = unsafe { &(&*keys)[..values.len()] };
                Box::new(slice.iter().zip(values.iter()))
            }
            Repr::Dict(d) => Box::new(d.entries.iter().flatten().map(|(k, p)| (k, p))),
        }
    }

    /// Visit every property value (insertion order).
    #[inline]
    pub fn for_each_value(&self, mut f: impl FnMut(&Property)) {
        match &self.repr {
            Repr::Shaped { values, .. } => values.iter().for_each(f),
            Repr::Dict(d) => {
                for (_, p) in d.entries.iter().flatten() {
                    f(p);
                }
            }
        }
    }

    pub fn values_mut(&mut self) -> Box<dyn Iterator<Item = &mut Property> + '_> {
        match &mut self.repr {
            Repr::Shaped { values, .. } => Box::new(values.iter_mut()),
            Repr::Dict(d) => Box::new(d.entries.iter_mut().flatten().map(|(_, p)| p)),
        }
    }
}
