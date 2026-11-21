//! Object model and garbage-collected heap.

use crate::bytecode::FunctionCode;
use crate::value::V;
use crate::value::{BigInt, JsStr, ObjectId, SymbolId, Value};
use crate::vm::Frame;
use std::any::Any;
use std::rc::Rc;

pub const WRITABLE: u8 = 1;
pub const ENUMERABLE: u8 = 2;
pub const CONFIGURABLE: u8 = 4;
pub const ACCESSOR: u8 = 8;
pub const ALL: u8 = WRITABLE | ENUMERABLE | CONFIGURABLE;
/// Built-in methods: writable + configurable, not enumerable.
pub const HIDDEN: u8 = WRITABLE | CONFIGURABLE;

/// Getter / setter pair of an accessor property (boxed: rare).
#[derive(Clone, Debug)]
pub struct Accessors {
    pub getter: Option<Value>,
    pub setter: Option<Value>,
}

/// A property slot: 32 bytes (data value + flags; accessors are boxed).
#[derive(Clone, Debug)]
pub struct Property {
    pub value: Value,
    acc: Option<Box<Accessors>>,
    pub flags: u8,
}

/// What a read of an absent property finds (inline caches of misses).
pub(crate) static ABSENT_PROPERTY: Property = Property { value: Value::Undefined, acc: None, flags: 0 };

impl Property {
    #[inline]
    pub fn data(value: Value, flags: u8) -> Property {
        Property { value, acc: None, flags: flags & !ACCESSOR }
    }
    pub fn accessor(getter: Option<Value>, setter: Option<Value>, flags: u8) -> Property {
        Property { value: Value::Undefined, acc: Some(Box::new(Accessors { getter, setter })), flags: (flags & !WRITABLE) | ACCESSOR }
    }
    #[inline]
    pub fn getter(&self) -> Option<&Value> {
        self.acc.as_ref().and_then(|a| a.getter.as_ref())
    }
    #[inline]
    pub fn setter(&self) -> Option<&Value> {
        self.acc.as_ref().and_then(|a| a.setter.as_ref())
    }
    pub fn getter_cloned(&self) -> Option<Value> {
        self.getter().cloned()
    }
    pub fn setter_cloned(&self) -> Option<Value> {
        self.setter().cloned()
    }
    pub fn set_getter(&mut self, g: Option<Value>) {
        self.acc.get_or_insert_with(|| Box::new(Accessors { getter: None, setter: None })).getter = g;
    }
    pub fn set_setter(&mut self, s: Option<Value>) {
        self.acc.get_or_insert_with(|| Box::new(Accessors { getter: None, setter: None })).setter = s;
    }
    pub fn is_accessor(&self) -> bool {
        self.flags & ACCESSOR != 0
    }
    pub fn writable(&self) -> bool {
        self.flags & WRITABLE != 0
    }
    pub fn enumerable(&self) -> bool {
        self.flags & ENUMERABLE != 0
    }
    pub fn configurable(&self) -> bool {
        self.flags & CONFIGURABLE != 0
    }
}

pub use crate::shape::PropertyMap;

pub type NativeFn = fn(&mut crate::realm::Realm, Value, &[Value]) -> Result<Value, Value>;

pub struct NativeFunction {
    pub func: NativeFn,
    pub name: JsStr,
    pub data: Option<Rc<dyn Any>>,
    /// Values kept alive for the native (e.g. captured JS callbacks).
    pub captures: Vec<Value>,
    pub is_constructor: bool,
    /// Built-ins the interpreter computes inline on `CallR` when the
    /// arguments are simple (see `vm::INTRINSIC_*`); 0 for the others.
    pub intrinsic: u8,
    /// The garbage collector may run while this native calls back into
    /// JavaScript: it keeps every object it holds across such calls
    /// reachable from its receiver, its arguments (rooted for the duration
    /// of the call) or [`crate::Realm::native_root`] slots. Other natives
    /// hold values the collector cannot see, so collection waits until
    /// they return.
    pub gc_safe: bool,
    /// Index of the realm the function belongs to (its [[Realm]]): calls
    /// run with that realm's intrinsics and global object.
    pub realm: u16,
}

impl std::fmt::Debug for NativeFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NativeFunction({})", self.name)
    }
}

#[derive(Debug)]
pub struct Closure {
    pub code: Rc<FunctionCode>,
    pub scope: Option<ObjectId>,
    /// Lexical `this` for arrows.
    pub this_value: Option<Value>,
    /// Lexical `new.target` for arrows.
    pub new_target: Option<Value>,
    pub home_object: Option<ObjectId>,
    /// Instance field initializer (class constructors).
    pub fields: Option<Value>,
    /// Lexical `arguments` object for arrows (if any).
    pub arguments: Option<Value>,
}

#[derive(Debug)]
pub struct BoundFunction {
    pub target: ObjectId,
    pub this: Value,
    pub args: Vec<Value>,
}

#[derive(Debug)]
pub struct ScopeData {
    pub slots: ScopeSlots,
    pub parent: Option<ObjectId>,
    /// Slot names (for TDZ error messages); `None` for function scopes,
    /// whose names live on the `FunctionCode`.
    pub names: Option<Rc<Vec<JsStr>>>,
}

/// The fixed-size slot array of a scope, stored right after its
/// [`ScopeData`] in the same allocation (see [`ScopeBox`]). Native code
/// reads `ptr` and `len` (see `jit::layout`).
#[repr(C)]
pub struct ScopeSlots {
    ptr: std::ptr::NonNull<Value>,
    len: usize,
}

impl ScopeSlots {
    /// Word indices of the slots pointer and of the length (`repr(C)`).
    #[cfg(feature = "jit")]
    pub(crate) const WORDS: (usize, usize) = (0, 1);
}

impl std::ops::Deref for ScopeSlots {
    type Target = [Value];
    #[inline(always)]
    fn deref(&self) -> &[Value] {
        // SAFETY: `ptr` points to `len` initialized values owned by the
        // enclosing `ScopeBox` allocation.
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl std::ops::DerefMut for ScopeSlots {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut [Value] {
        // SAFETY: as in `deref`; `&mut self` gives exclusive access.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl std::fmt::Debug for ScopeSlots {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

/// An owned [`ScopeData`] and its slots in one allocation (a scope per
/// call is common enough that a box plus a slot vector cost twice).
pub struct ScopeBox(std::ptr::NonNull<ScopeData>);

impl ScopeBox {
    /// Header layout and the offset of the slots after it.
    #[inline(always)]
    fn layout(n: usize) -> (std::alloc::Layout, usize) {
        let (l, off) = std::alloc::Layout::new::<ScopeData>().extend(std::alloc::Layout::array::<Value>(n).expect("scope size")).expect("scope size");
        (l.pad_to_align(), off)
    }

    /// A scope of `n` slots, each `undefined`.
    #[inline]
    pub fn new(n: usize, parent: Option<ObjectId>, names: Option<Rc<Vec<JsStr>>>) -> ScopeBox {
        // SAFETY: every slot is written before the box exists.
        unsafe { Self::build(n, parent, names, |p| for i in 0..n { std::ptr::write(p.add(i), Value::Undefined) }) }
    }

    /// A scope holding `values`.
    pub fn from_values(values: Vec<Value>, parent: Option<ObjectId>, names: Option<Rc<Vec<JsStr>>>) -> ScopeBox {
        let n = values.len();
        // SAFETY: the values are moved into the `n` slots.
        unsafe {
            Self::build(n, parent, names, |p| {
                for (i, v) in values.into_iter().enumerate() {
                    std::ptr::write(p.add(i), v);
                }
            })
        }
    }

    /// A copy of this scope (same parent and names, cloned values).
    pub fn copy(&self) -> ScopeBox {
        let src: &[Value] = &self.slots;
        // SAFETY: each slot is written with a clone of the source slot.
        unsafe {
            Self::build(src.len(), self.parent, self.names.clone(), |p| {
                for (i, v) in src.iter().enumerate() {
                    std::ptr::write(p.add(i), v.clone());
                }
            })
        }
    }

    /// SAFETY: `init` must initialize all `n` slots.
    #[inline(always)]
    unsafe fn build(n: usize, parent: Option<ObjectId>, names: Option<Rc<Vec<JsStr>>>, init: impl FnOnce(*mut Value)) -> ScopeBox {
        let (layout, off) = Self::layout(n);
        let base = std::alloc::alloc(layout);
        if base.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        let slots = base.add(off) as *mut Value;
        init(slots);
        let head = base as *mut ScopeData;
        std::ptr::write(head, ScopeData { slots: ScopeSlots { ptr: std::ptr::NonNull::new_unchecked(slots), len: n }, parent, names });
        ScopeBox(std::ptr::NonNull::new_unchecked(head))
    }
}

impl std::ops::Deref for ScopeBox {
    type Target = ScopeData;
    #[inline(always)]
    fn deref(&self) -> &ScopeData {
        // SAFETY: the box owns a live `ScopeData`.
        unsafe { self.0.as_ref() }
    }
}

impl std::ops::DerefMut for ScopeBox {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut ScopeData {
        // SAFETY: the box owns a live `ScopeData`; `&mut self` is exclusive.
        unsafe { self.0.as_mut() }
    }
}

impl Drop for ScopeBox {
    fn drop(&mut self) {
        // SAFETY: the allocation was made by `build` with the layout for
        // `len` slots; values and header are dropped once, then freed.
        unsafe {
            let head = self.0.as_ptr();
            let n = (*head).slots.len;
            std::ptr::drop_in_place(std::ptr::slice_from_raw_parts_mut((*head).slots.ptr.as_ptr(), n));
            std::ptr::drop_in_place(&mut (*head).names);
            std::alloc::dealloc(head as *mut u8, Self::layout(n).0);
        }
    }
}

impl std::fmt::Debug for ScopeBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratorState {
    SuspendedStart,
    SuspendedYield,
    Executing,
    Completed,
}

#[derive(Debug)]
pub struct GeneratorData {
    pub state: GeneratorState,
    pub frame: Option<Box<Frame>>,
    pub is_async: bool,
    /// For async functions: the promise resolved on completion.
    pub promise: Option<ObjectId>,
    /// Async generator request queue: (kind, value, promise capability)
    pub queue: std::collections::VecDeque<(GenRequestKind, Value, ObjectId)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenRequestKind {
    Next,
    Return,
    Throw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromiseState {
    Pending,
    Fulfilled,
    Rejected,
}

#[derive(Debug, Clone)]
pub struct PromiseReaction {
    /// Called with the settled value; `None` means pass-through.
    pub handler: Option<Value>,
    /// Derived promise capability (promise, resolve, reject).
    pub capability: Option<(Value, Value, Value)>,
    /// Native continuation (used by await / host): receives the value.
    pub native: Option<(crate::heap::NativeFn, Value)>,
    /// True for reject reactions (pass-through rethrows).
    pub is_reject: bool,
}

#[derive(Debug)]
pub struct PromiseData {
    pub state: PromiseState,
    pub value: Value,
    pub fulfill_reactions: Vec<PromiseReaction>,
    pub reject_reactions: Vec<PromiseReaction>,
    pub handled: bool,
}

/// The key a Map/Set stores for `k`: -0 becomes +0 (SameValueZero keys).
#[inline]
pub fn normalize_key(k: Value) -> Value {
    match k.v() {
        V::Number(n) if n == 0.0 => Value::Number(0.0),
        V::Empty => Value::Undefined,
        _ => k,
    }
}

#[inline(always)]
fn mix64(x: u64) -> u32 {
    let x = (x ^ (x >> 29)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    ((x >> 32) ^ x) as u32
}

/// Hash of a (normalized) key, consistent with [`key_eq`]: numbers by
/// value (all NaNs alike), strings by contents (cached per string),
/// objects / symbols / other primitives by identity.
#[inline]
pub fn hash_key(k: &Value) -> u32 {
    match k.v() {
        V::Number(n) => mix64(if n.is_nan() { f64::NAN.to_bits() } else { n.to_bits() }),
        V::String(s) => s.hash32(),
        V::BigInt(b) => {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            b.hash(&mut h);
            mix64(h.finish())
        }
        _ => mix64(k.bits()),
    }
}

/// SameValueZero on normalized keys.
#[inline]
pub fn key_eq(a: &Value, b: &Value) -> bool {
    if a.bits() == b.bits() {
        return true;
    }
    match (a.v(), b.v()) {
        (V::Number(x), V::Number(y)) => x == y || (x.is_nan() && y.is_nan()),
        (V::String(x), V::String(y)) => x == y,
        (V::BigInt(x), V::BigInt(y)) => x == y,
        _ => false,
    }
}

/// `index` of a finished `MapIterator` (it stays done).
pub const MAP_ITER_DONE: usize = usize::MAX;

const SLOT_EMPTY: u32 = u32::MAX;
const SLOT_DELETED: u32 = u32::MAX - 1;

/// Map / Set storage: entries in insertion order (`None` for deleted
/// ones, so iterators keep their positions) indexed by an open-addressing
/// hash table of entry numbers. Lookups allocate nothing. Deleted entries
/// are compacted away once they are the majority and no iteration is in
/// progress (see `iterators`).
#[derive(Debug, Default, Clone)]
pub struct MapData {
    pub entries: Vec<Option<(Value, Value)>>,
    hashes: Vec<u32>,
    table: Vec<u32>,
    live: usize,
    table_deleted: usize,
    /// Unfinished iterators and running `forEach` loops over this
    /// collection: entry positions must stay put while there are any.
    pub iterators: u32,
}

impl MapData {
    /// Entry number and table slot of `k` (normalized), if present.
    #[inline]
    fn find(&self, k: &Value, h: u32) -> Option<(usize, usize)> {
        if self.table.is_empty() {
            return None;
        }
        let mask = self.table.len() - 1;
        let mut i = h as usize & mask;
        loop {
            let e = self.table[i];
            if e == SLOT_EMPTY {
                return None;
            }
            if e != SLOT_DELETED && self.hashes[e as usize] == h {
                if let Some((key, _)) = &self.entries[e as usize] {
                    if key_eq(key, k) {
                        return Some((e as usize, i));
                    }
                }
            }
            i = (i + 1) & mask;
        }
    }

    fn lookup(&self, k: &Value) -> Option<(usize, usize)> {
        let k = match k.v() {
            V::Number(n) if n == 0.0 => Value::Number(0.0),
            V::Empty => Value::Undefined,
            _ => k.clone(),
        };
        let h = hash_key(&k);
        self.find(&k, h)
    }

    fn rebuild(&mut self, cap: usize) {
        self.table.clear();
        self.table.resize(cap, SLOT_EMPTY);
        self.table_deleted = 0;
        let mask = cap - 1;
        for (e, entry) in self.entries.iter().enumerate() {
            if entry.is_some() {
                let mut i = self.hashes[e] as usize & mask;
                while self.table[i] != SLOT_EMPTY {
                    i = (i + 1) & mask;
                }
                self.table[i] = e as u32;
            }
        }
    }

    pub fn get(&self, k: &Value) -> Option<&Value> {
        let (e, _) = self.lookup(k)?;
        self.entries[e].as_ref().map(|(_, v)| v)
    }
    pub fn has(&self, k: &Value) -> bool {
        self.lookup(k).is_some()
    }
    /// Add or update; an existing entry keeps its key and position.
    pub fn set(&mut self, k: Value, v: Value) {
        let k = normalize_key(k);
        let h = hash_key(&k);
        if let Some((e, _)) = self.find(&k, h) {
            if let Some(entry) = &mut self.entries[e] {
                entry.1 = v;
            }
            return;
        }
        if (self.live + self.table_deleted + 1) * 2 > self.table.len() {
            self.compact_if_sparse();
            let want = ((self.live + 1) * 2).next_power_of_two().max(8);
            let cap = if want > self.table.len() { want } else { self.table.len() };
            self.rebuild(cap);
        }
        let e = self.entries.len() as u32;
        if e >= SLOT_DELETED {
            return;
        }
        self.entries.push(Some((k, v)));
        self.hashes.push(h);
        let mask = self.table.len() - 1;
        let mut i = h as usize & mask;
        while self.table[i] != SLOT_EMPTY && self.table[i] != SLOT_DELETED {
            i = (i + 1) & mask;
        }
        if self.table[i] == SLOT_DELETED {
            self.table_deleted -= 1;
        }
        self.table[i] = e;
        self.live += 1;
    }
    pub fn delete(&mut self, k: &Value) -> bool {
        match self.lookup(k) {
            Some((e, slot)) => {
                self.entries[e] = None;
                self.table[slot] = SLOT_DELETED;
                self.table_deleted += 1;
                self.live -= 1;
                self.compact_if_sparse();
                true
            }
            None => false,
        }
    }
    /// Remove every entry. With iterations in progress the positions stay
    /// (as deleted entries), so iterators see entries added afterwards.
    pub fn clear(&mut self) {
        if self.iterators == 0 {
            self.entries.clear();
            self.hashes.clear();
            self.table.clear();
        } else {
            for e in self.entries.iter_mut() {
                *e = None;
            }
            let cap = self.table.len();
            self.table.clear();
            self.table.resize(cap, SLOT_EMPTY);
        }
        self.live = 0;
        self.table_deleted = 0;
    }
    pub fn size(&self) -> usize {
        self.live
    }
    /// Drop deleted entries when they are the majority and nothing
    /// iterates (positions change).
    pub fn compact_if_sparse(&mut self) {
        let dead = self.entries.len() - self.live;
        if self.iterators != 0 || dead < 16 || dead * 2 < self.entries.len() {
            return;
        }
        let mut w = 0;
        for r in 0..self.entries.len() {
            if self.entries[r].is_some() {
                self.entries.swap(w, r);
                self.hashes[w] = self.hashes[r];
                w += 1;
            }
        }
        self.entries.truncate(w);
        self.hashes.truncate(w);
        self.entries.shrink_to(w * 2);
        self.hashes.shrink_to(w * 2);
        let cap = (w * 2).next_power_of_two().max(8);
        self.rebuild(cap);
    }
    /// A copy without deleted entries or iteration state.
    pub fn snapshot(&self) -> MapData {
        let mut d = MapData::default();
        for (k, v) in self.entries.iter().flatten() {
            d.set(k.clone(), v.clone());
        }
        d
    }
    /// An iteration over the entries starts / ends (see `iterators`).
    pub fn iteration_started(&mut self) {
        self.iterators += 1;
    }
    pub fn iteration_ended(&mut self) {
        self.iterators = self.iterators.saturating_sub(1);
        self.compact_if_sparse();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedArrayKind {
    Int8,
    Uint8,
    Uint8Clamped,
    Int16,
    Uint16,
    Int32,
    Uint32,
    Float32,
    Float64,
    BigInt64,
    BigUint64,
    Float16,
}

impl TypedArrayKind {
    pub fn element_size(&self) -> usize {
        match self {
            TypedArrayKind::Int8 | TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped => 1,
            TypedArrayKind::Int16 | TypedArrayKind::Uint16 | TypedArrayKind::Float16 => 2,
            TypedArrayKind::Int32 | TypedArrayKind::Uint32 | TypedArrayKind::Float32 => 4,
            _ => 8,
        }
    }
    /// BigInt64Array / BigUint64Array (content type BigInt).
    #[inline]
    pub fn is_bigint(&self) -> bool {
        matches!(self, TypedArrayKind::BigInt64 | TypedArrayKind::BigUint64)
    }
    pub fn name(&self) -> &'static str {
        match self {
            TypedArrayKind::Int8 => "Int8Array",
            TypedArrayKind::Uint8 => "Uint8Array",
            TypedArrayKind::Uint8Clamped => "Uint8ClampedArray",
            TypedArrayKind::Int16 => "Int16Array",
            TypedArrayKind::Uint16 => "Uint16Array",
            TypedArrayKind::Int32 => "Int32Array",
            TypedArrayKind::Uint32 => "Uint32Array",
            TypedArrayKind::Float32 => "Float32Array",
            TypedArrayKind::Float64 => "Float64Array",
            TypedArrayKind::BigInt64 => "BigInt64Array",
            TypedArrayKind::BigUint64 => "BigUint64Array",
            TypedArrayKind::Float16 => "Float16Array",
        }
    }
}

/// A typed array or DataView: a window on an `ArrayBuffer` object.
#[derive(Debug, Clone, Copy)]
pub struct TypedArrayData {
    /// Element type (`Uint8` for a DataView).
    pub kind: TypedArrayKind,
    pub buffer: ObjectId,
    pub byte_offset: usize,
    /// Element count (byte count for a DataView); `None` for a view that
    /// tracks the length of a resizable buffer.
    pub fixed_length: Option<usize>,
}

impl TypedArrayData {
    /// Current length in elements for a buffer of `buf_len` bytes; `None`
    /// when the view is out of bounds (IsTypedArrayOutOfBounds /
    /// IsViewOutOfBounds). A detached buffer has `buf_len` 0.
    #[inline]
    pub fn length_in(&self, buf_len: usize) -> Option<usize> {
        let es = self.kind.element_size();
        match self.fixed_length {
            Some(n) => {
                if self.byte_offset + n * es <= buf_len {
                    Some(n)
                } else {
                    None
                }
            }
            None => {
                if self.byte_offset <= buf_len {
                    Some((buf_len - self.byte_offset) / es)
                } else {
                    None
                }
            }
        }
    }
}

/// The storage of an `ArrayBuffer` / `SharedArrayBuffer` object.
#[derive(Debug)]
pub struct ArrayBufferData {
    /// The bytes; hosts (WebAssembly memories) may share the `Rc`. For a
    /// `SharedArrayBuffer` this vector aliases [`ArrayBufferData::block`]
    /// and never frees it (see the `Drop` impl).
    pub bytes: Rc<std::cell::RefCell<Vec<u8>>>,
    /// `Some(max)` for a resizable (growable, when shared) buffer.
    pub max_byte_length: Option<usize>,
    pub shared: bool,
    pub detached: bool,
    pub immutable: bool,
    /// Memory of a `SharedArrayBuffer`, shared with other agents (threads).
    pub block: Option<std::sync::Arc<SharedBlock>>,
}

impl ArrayBufferData {
    pub fn new(bytes: Vec<u8>) -> Box<ArrayBufferData> {
        Self::from_rc(Rc::new(std::cell::RefCell::new(bytes)))
    }
    pub fn from_rc(bytes: Rc<std::cell::RefCell<Vec<u8>>>) -> Box<ArrayBufferData> {
        Box::new(ArrayBufferData { bytes, max_byte_length: None, shared: false, detached: false, immutable: false, block: None })
    }
    /// A `SharedArrayBuffer` over `block` (growable when `max` is set).
    pub fn shared(block: std::sync::Arc<SharedBlock>, max: Option<usize>) -> Box<ArrayBufferData> {
        // SAFETY: the vector aliases the block's allocation (capacity =
        // the block's capacity, length = its current length, initialised:
        // the block is zeroed); it is never grown past that capacity and
        // never deallocated (ArrayBufferData::drop forgets it).
        let v = unsafe { Vec::from_raw_parts(block.as_ptr(), block.len(), block.capacity()) };
        let mut d = Self::new(v);
        d.shared = true;
        d.max_byte_length = max;
        d.block = Some(block);
        d
    }
    /// Bring the local view of a shared buffer's length up to date with
    /// growth by other agents.
    #[inline]
    pub fn sync_shared_len(&self) {
        if let Some(block) = &self.block {
            let n = block.len();
            if let Ok(mut b) = self.bytes.try_borrow_mut() {
                if b.len() < n {
                    // SAFETY: n <= capacity, and bytes up to n are zeroed or written.
                    unsafe { b.set_len(n) };
                }
            }
        }
    }
}

impl Drop for ArrayBufferData {
    fn drop(&mut self) {
        if self.block.is_none() {
            return;
        }
        // The vector does not own the shared allocation: forget it. If the
        // cell is somehow borrowed, keep the whole Rc alive instead (leak,
        // never free memory another agent uses).
        match self.bytes.try_borrow_mut() {
            Ok(mut b) => std::mem::forget(std::mem::take(&mut *b)),
            Err(_) => std::mem::forget(self.bytes.clone()),
        }
    }
}

/// The memory of a `SharedArrayBuffer`: one zeroed allocation of the
/// buffer's maximum size, shared by every agent holding the buffer. Its
/// current length only grows.
pub struct SharedBlock {
    ptr: std::ptr::NonNull<u8>,
    cap: usize,
    len: std::sync::atomic::AtomicUsize,
}

// SAFETY: the block is plain memory; concurrent access goes through
// atomic operations (Atomics) or racy byte copies, as in any engine.
unsafe impl Send for SharedBlock {}
unsafe impl Sync for SharedBlock {}

impl std::fmt::Debug for SharedBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SharedBlock({} of {})", self.len(), self.cap)
    }
}

impl SharedBlock {
    /// A zeroed block of `len` bytes that can grow to `cap`; None when the
    /// memory is not available.
    pub fn new(len: usize, cap: usize) -> Option<std::sync::Arc<SharedBlock>> {
        let cap = cap.max(len);
        let ptr = if cap == 0 {
            std::ptr::NonNull::dangling()
        } else {
            // 16-byte aligned so every element is aligned for atomics.
            let layout = std::alloc::Layout::from_size_align(cap, 16).ok()?;
            // SAFETY: non-zero size.
            std::ptr::NonNull::new(unsafe { std::alloc::alloc_zeroed(layout) })?
        };
        Some(std::sync::Arc::new(SharedBlock { ptr, cap, len: std::sync::atomic::AtomicUsize::new(len) }))
    }
    pub fn as_ptr(&self) -> *mut u8 {
        self.ptr.as_ptr()
    }
    pub fn len(&self) -> usize {
        self.len.load(std::sync::atomic::Ordering::SeqCst)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn capacity(&self) -> usize {
        self.cap
    }
    /// Grow to `new_len` (never shrinks). Err(current) when `new_len` is
    /// smaller than the current length; false-y on exceeding capacity.
    pub fn grow_to(&self, new_len: usize) -> Result<(), usize> {
        use std::sync::atomic::Ordering;
        let mut cur = self.len.load(Ordering::SeqCst);
        loop {
            if new_len < cur || new_len > self.cap {
                return Err(cur);
            }
            match self.len.compare_exchange(cur, new_len, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return Ok(()),
                Err(c) => cur = c,
            }
        }
    }
}

impl Drop for SharedBlock {
    fn drop(&mut self) {
        if self.cap > 0 {
            // SAFETY: allocated in `new` with this layout.
            unsafe { std::alloc::dealloc(self.ptr.as_ptr(), std::alloc::Layout::from_size_align(self.cap, 16).unwrap()) };
        }
    }
}

/// The parameter map of a mapped arguments object: index `i` reads and
/// writes slot `slots[i]` of the function scope `scope` while mapped
/// ([`ArgMap::UNMAPPED`] once deleted, redefined as an accessor or made
/// non-writable).
#[derive(Debug)]
pub struct ArgMap {
    pub scope: ObjectId,
    pub slots: Vec<u32>,
}

impl ArgMap {
    pub const UNMAPPED: u32 = u32::MAX;
}

#[derive(Debug)]
pub struct RegExpData {
    /// Compiled program, shared by every RegExp with the same source and
    /// flags (see `builtins::regexp::compiled`).
    pub regex: Rc<crate::regex::Regex>,
    /// `source` (EscapeRegExpPattern of the original source).
    pub source: JsStr,
    pub flags: JsStr,
    /// [[OriginalSource]].
    pub original_source: JsStr,
    /// [[LegacyFeaturesEnabled]] (Annex B `compile`): created by %RegExp% itself.
    pub legacy: bool,
    /// [[Realm]]: the realm whose %RegExp% created it.
    pub realm: u16,
}

/// The internal shape of an object.
pub enum ObjectKind {
    Ordinary,
    /// Dense elements (`Value::Empty` = hole). `length` may exceed
    /// `elements.len()` for sparse arrays whose extra indices live in the
    /// property map.
    Array { elements: Vec<Value>, length: u32 },
    Function(Box<Closure>),
    Native(Box<NativeFunction>),
    Bound(Box<BoundFunction>),
    Scope(ScopeBox),
    Error,
    Arguments,
    /// A sloppy function's arguments object whose index properties alias
    /// the parameters (see `Realm::create_arguments_object`).
    MappedArguments(Box<ArgMap>),
    Boolean(bool),
    Number(f64),
    String(JsStr),
    Symbol(SymbolId),
    BigInt(Rc<BigInt>),
    Date(f64),
    RegExp(Box<RegExpData>),
    Map(Box<MapData>),
    Set(Box<MapData>),
    WeakMap(Box<MapData>),
    WeakSet(Box<MapData>),
    Generator(Box<GeneratorData>),
    Promise(Box<PromiseData>),
    ArrayBuffer(Box<ArrayBufferData>),
    TypedArray(Box<TypedArrayData>),
    DataView(Box<TypedArrayData>),
    Proxy { target: ObjectId, handler: ObjectId, revoked: bool },
    /// Iterator over array/string/map/set/arguments
    ArrayIterator { target: Value, index: u32, kind: u8 },
    MapIterator { target: ObjectId, index: usize, kind: u8 },
    StringIterator { string: JsStr, index: usize },
    RegExpStringIterator { regexp: ObjectId, string: JsStr, global: bool, unicode: bool, done: bool },
    /// For-in key enumeration state
    /// `shape`: the receiver's shape id when the keys were collected by the
    /// fast path (while it is unchanged no key can have been deleted); 0
    /// means every key is re-checked with `[[HasProperty]]`.
    ForInIterator(Box<ForInState>),
    /// Host-defined (DOM nodes etc.)
    Host(Box<dyn Any>),
    /// Reference to a module's top-level binding (an import resolved at
    /// link time). Never exposed to scripts: it lives in import slots and
    /// in namespace getters, and is dereferenced on every read so imports
    /// are live bindings.
    ModuleBinding { scope: ObjectId, slot: u32, name: JsStr },
    /// A module namespace object: its exports are stored as accessors
    /// over the live bindings but behave as writable, non-configurable
    /// data properties (see `Realm::namespace_get_own`).
    ModuleNamespace,
    /// Internal slots of natively implemented built-ins (iterator helpers,
    /// disposable stacks, ...); `SlotData::tag` says which.
    Slots(Box<SlotData>),
}

/// Generic internal-slot storage: `values` are fixed per-tag slots, `list`
/// a variable-length list; both are traced by the GC.
#[derive(Default)]
pub struct SlotData {
    pub tag: u8,
    pub kind: u8,
    pub state: u8,
    pub num: [f64; 2],
    pub values: Vec<Value>,
    pub list: Vec<Value>,
}

impl std::fmt::Debug for ObjectKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl ObjectKind {
    pub fn name(&self) -> &'static str {
        match self {
            ObjectKind::Ordinary => "Object",
            ObjectKind::Array { .. } => "Array",
            ObjectKind::Function(_) | ObjectKind::Native(_) | ObjectKind::Bound(_) => "Function",
            ObjectKind::Scope(_) => "Scope",
            ObjectKind::Error => "Error",
            ObjectKind::Arguments | ObjectKind::MappedArguments(_) => "Arguments",
            ObjectKind::Boolean(_) => "Boolean",
            ObjectKind::Number(_) => "Number",
            ObjectKind::String(_) => "String",
            ObjectKind::Symbol(_) => "Symbol",
            ObjectKind::BigInt(_) => "BigInt",
            ObjectKind::Date(_) => "Date",
            ObjectKind::RegExp(_) => "RegExp",
            ObjectKind::Map(_) => "Map",
            ObjectKind::Set(_) => "Set",
            ObjectKind::WeakMap(_) => "WeakMap",
            ObjectKind::WeakSet(_) => "WeakSet",
            ObjectKind::Generator(_) => "Generator",
            ObjectKind::Promise(_) => "Promise",
            ObjectKind::ArrayBuffer(_) => "ArrayBuffer",
            ObjectKind::TypedArray(_) => "TypedArray",
            ObjectKind::DataView(_) => "DataView",
            ObjectKind::Proxy { .. } => "Proxy",
            ObjectKind::ArrayIterator { .. } => "Array Iterator",
            ObjectKind::MapIterator { .. } => "Map Iterator",
            ObjectKind::StringIterator { .. } => "String Iterator",
            ObjectKind::RegExpStringIterator { .. } => "RegExp String Iterator",
            ObjectKind::ForInIterator(_) => "ForIn Iterator",
            ObjectKind::Host(_) => "Object",
            ObjectKind::ModuleBinding { .. } => "ModuleBinding",
            ObjectKind::ModuleNamespace => "Module",
            ObjectKind::Slots(_) => "Object",
        }
    }
    pub fn is_callable(&self) -> bool {
        matches!(self, ObjectKind::Function(_) | ObjectKind::Native(_) | ObjectKind::Bound(_))
    }
}

pub struct Object {
    pub proto: Option<ObjectId>,
    pub props: PropertyMap,
    pub extensible: bool,
    pub kind: ObjectKind,
    /// `Object.prototype.toString` tag override for host objects (an
    /// index into the interned class names, 0 for none; see
    /// [`Object::class_name`]).
    pub class_tag: u16,
    /// [[IsHTMLDDA]] (`document.all`): falsy, `typeof` "undefined", loosely
    /// equal to null and undefined (see [`crate::Realm::make_htmldda`]).
    pub htmldda: bool,
    /// A host object that supports indexed properties (see
    /// [`IndexedHostHook`]); only consulted for array-index keys.
    pub indexed: bool,
}

/// Indexed property getter for host objects (WebIDL legacy platform
/// objects with an indexed getter and no indexed setter, e.g.
/// `DOMTokenList`), installed once by the embedder. For a `Host` object
/// whose `indexed` flag is set, array-index keys are answered by `get`
/// at every access (live), as read-only, enumerable, configurable own
/// properties that cannot be defined, written or deleted; `len` bounds
/// them for key enumeration. Every other key, and every other object,
/// takes the ordinary paths.
#[derive(Clone, Copy)]
pub struct IndexedHostHook {
    pub get: fn(&dyn Any, u32) -> Option<Value>,
    pub len: fn(&dyn Any) -> u32,
}

static INDEXED_HOST_HOOK: std::sync::OnceLock<IndexedHostHook> = std::sync::OnceLock::new();

/// Install the indexed host hook (the first call wins).
pub fn set_indexed_host_hook(hook: IndexedHostHook) {
    let _ = INDEXED_HOST_HOOK.set(hook);
}

impl Object {
    /// Item `i` of an indexed host object (None: no such own property).
    #[cold]
    pub fn indexed_item(&self, i: u32) -> Option<Value> {
        match (&self.kind, INDEXED_HOST_HOOK.get()) {
            (ObjectKind::Host(d), Some(h)) if self.indexed => (h.get)(&**d, i),
            _ => None,
        }
    }
    /// Number of indexed properties of an indexed host object.
    #[cold]
    pub fn indexed_len(&self) -> u32 {
        match (&self.kind, INDEXED_HOST_HOOK.get()) {
            (ObjectKind::Host(d), Some(h)) if self.indexed => (h.len)(&**d),
            _ => 0,
        }
    }
}

impl Object {
    /// Start loading the separately allocated parts the collector will
    /// read (sweep look-ahead): the property storage and the boxed kind.
    #[inline(always)]
    fn prefetch_parts(&self) {
        use crate::alloc::prefetch;
        self.props.prefetch();
        match &self.kind {
            ObjectKind::Array { elements, .. } if elements.capacity() != 0 => prefetch(elements.as_ptr()),
            ObjectKind::Function(c) => prefetch(&**c),
            ObjectKind::Scope(s) => prefetch(&**s),
            ObjectKind::Native(n) => prefetch(&**n),
            ObjectKind::Bound(b) => prefetch(&**b),
            ObjectKind::Promise(p) => prefetch(&**p),
            ObjectKind::Generator(g) => prefetch(&**g),
            ObjectKind::Map(m) | ObjectKind::Set(m) => prefetch(&**m),
            ObjectKind::ForInIterator(f) => prefetch(&**f),
            ObjectKind::Slots(d) => prefetch(&**d),
            _ => {}
        }
    }

    /// Second step of [`Object::prefetch_parts`]: what the boxed kind
    /// points to (read once the box itself is loaded).
    #[inline(always)]
    fn prefetch_inner_parts(&self) {
        use crate::alloc::prefetch;
        match &self.kind {
            ObjectKind::Scope(s) if s.slots.len() > 3 => prefetch(s.slots.as_ptr().wrapping_add(4)),
            ObjectKind::Function(c) => prefetch(std::rc::Rc::as_ptr(&c.code)),
            _ => {}
        }
    }

    pub fn new(proto: Option<ObjectId>, kind: ObjectKind) -> Object {
        Object { proto, props: PropertyMap::new(), extensible: true, kind, class_tag: 0, htmldda: false, indexed: false }
    }
    /// `Object.prototype.toString` tag override for host objects.
    pub fn class_name(&self) -> Option<&'static str> {
        class_name_of(self.class_tag)
    }
    pub fn set_class_name(&mut self, name: &str) {
        self.class_tag = intern_class_name(name);
    }
    /// Change the prototype (`[[SetPrototypeOf]]` without checks); keeps
    /// prototype-chain inline caches honest.
    #[inline]
    pub fn set_proto(&mut self, proto: Option<ObjectId>) {
        if self.props.in_chain() && self.proto != proto {
            crate::shape::bump_proto_epoch();
        }
        self.proto = proto;
    }
    pub fn is_callable(&self) -> bool {
        match &self.kind {
            ObjectKind::Proxy { target: _, .. } => true, // refined by Realm::is_callable
            k => k.is_callable(),
        }
    }
    pub fn is_array(&self) -> bool {
        matches!(self.kind, ObjectKind::Array { .. })
    }
    pub fn array_elements(&self) -> Option<&Vec<Value>> {
        match &self.kind {
            ObjectKind::Array { elements, .. } => Some(elements),
            _ => None,
        }
    }
}

thread_local! {
    /// Debug aid (RH_DEBUG_GC): what each freed slot held.
    static FREED_INFO: std::cell::RefCell<std::collections::HashMap<u32, String>> = std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Interned host class names (few dozen; shared by all threads).
static CLASS_NAMES: std::sync::RwLock<Vec<&'static str>> = std::sync::RwLock::new(Vec::new());

/// The tag of a class name (interned once: objects carry two bytes).
pub fn intern_class_name(name: &str) -> u16 {
    if let Some(i) = CLASS_NAMES.read().unwrap_or_else(|e| e.into_inner()).iter().position(|n| *n == name) {
        return i as u16 + 1;
    }
    let mut names = CLASS_NAMES.write().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = names.iter().position(|n| *n == name) {
        return i as u16 + 1;
    }
    if names.len() >= u16::MAX as usize - 1 {
        return 0;
    }
    names.push(Box::leak(name.to_string().into_boxed_str()));
    names.len() as u16
}

fn class_name_of(tag: u16) -> Option<&'static str> {
    if tag == 0 {
        return None;
    }
    CLASS_NAMES.read().unwrap_or_else(|e| e.into_inner()).get(tag as usize - 1).copied()
}

/// State of a `for-in` enumeration.
#[derive(Debug)]
pub struct ForInState {
    pub keys: Vec<JsStr>,
    pub index: usize,
    pub object: Option<ObjectId>,
    /// The receiver's shape id when the keys were collected by the fast
    /// path (while it is unchanged no key can have been deleted); 0 means
    /// every key is re-checked with `[[HasProperty]]`.
    pub shape: u32,
}

/// Bytes of one heap slot (an object's inline part).
/// A reference to a freed (or never allocated) object: a bug, reported
/// with what is known about the object (kept out of line so that
/// [`Heap::get`] inlines).
#[cold]
#[inline(never)]
fn freed_object(id: ObjectId) -> ! {
    let info = FREED_INFO.with(|m| m.borrow().get(&id.0).cloned()).unwrap_or_default();
    panic!("use of freed object #{} {}", id.0, info)
}

pub const SLOT_BYTES: usize = std::mem::size_of::<Option<Object>>();

// Colours of the collector (`Heap::marks`, meaningful during a cycle).
/// Not reached (yet); freed by the sweep if it stays so.
const WHITE: u8 = 0;
/// Reached, waiting in the grey stack to be scanned.
const GREY: u8 = 1;
/// A weak collection scanned by the marker: its entries wait for the
/// ephemeron pass.
const BLACK_WEAK: u8 = 2;
/// Scanned (or allocated during the cycle): nothing left to do.
pub(crate) const BLACK: u8 = 3;

/// Where the incremental collector is (see [`Heap::start_cycle`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcPhase {
    Idle,
    Marking,
    Sweeping,
}

/// How much one collector step may do: a number of work units (objects
/// scanned or slots swept) and/or a deadline.
#[derive(Debug, Clone, Copy)]
pub struct GcBudget {
    units: usize,
    deadline: Option<std::time::Instant>,
}

impl GcBudget {
    /// Units between two looks at the clock.
    const CHECK: usize = 64;
    pub fn unlimited() -> GcBudget {
        GcBudget { units: usize::MAX, deadline: None }
    }
    pub fn units(n: usize) -> GcBudget {
        GcBudget { units: n.max(1), deadline: None }
    }
    pub fn until(deadline: std::time::Instant) -> GcBudget {
        GcBudget { units: Self::CHECK, deadline: Some(deadline) }
    }
    /// Spend `n` units; `true` when the budget is exhausted.
    #[inline]
    fn spend(&mut self, n: usize) -> bool {
        if self.units > n {
            self.units -= n;
            return false;
        }
        match self.deadline {
            Some(d) if std::time::Instant::now() < d => {
                self.units = Self::CHECK;
                false
            }
            _ => {
                self.units = 0;
                true
            }
        }
    }
}

/// Garbage collected arena with an incremental mark-and-sweep collector.
///
/// A collection cycle ([`Heap::start_cycle`]) greys the roots, then marks
/// in steps ([`Heap::mark_step`]) interleaved with the program, then sweeps
/// in steps ([`Heap::sweep_step`]). Marking is snapshot-at-the-beginning:
/// everything reachable when the cycle started, plus everything allocated
/// since (allocated black), survives it. That holds because every change
/// to an object goes through [`Heap::get_mut`] / [`Heap::try_get_mut`]
/// (or native code that checks [`Heap::marking`] and takes those paths
/// while it is set), whose write barrier scans an unscanned object before
/// it can lose a reference. Roots are not barriered: they are part of the
/// snapshot. Slots freed by a sweep are reused only once it finishes (and
/// `collections` moves), so ids stay unique within a cycle.
pub struct Heap {
    objects: Vec<Option<Object>>,
    free: Vec<u32>,
    marks: Vec<u8>,
    /// The write barrier is armed (the cycle is marking); read by native
    /// code, which then leaves stores to the helpers.
    pub(crate) marking: bool,
    phase: GcPhase,
    grey: Vec<u32>,
    /// Weak collections scanned by the marker (ephemeron pass).
    weak: Vec<u32>,
    /// Ephemeron values waiting for their (unmarked) key, by key, as of
    /// the last ephemeron pass: scanning the key shades them, so a chain
    /// key -> value -> key ... is marked in one pass instead of one link
    /// per pass.
    ephemerons: std::collections::HashMap<u32, Vec<u32>, crate::value::FxBuild>,
    /// Sweep position and end (the table length when marking finished).
    sweep_at: usize,
    sweep_end: usize,
    /// Slots freed by the running sweep (reused once it finishes).
    swept: Vec<u32>,
    pub allocated_since_gc: usize,
    pub live_count: usize,
    /// Objects allocated since the heap was made (pacing).
    pub allocated_total: usize,
    /// Objects freed by the last finished cycle.
    pub last_freed: usize,
    /// Collections finished so far (object ids are only reused after one).
    pub collections: u32,
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

fn is_weak_kind(k: &ObjectKind) -> bool {
    matches!(k, ObjectKind::WeakMap(_) | ObjectKind::WeakSet(_))
}

/// Grey `id` if it is white (allocated-black and out-of-range ids are left).
#[inline(always)]
fn shade(marks: &mut [u8], grey: &mut Vec<u32>, id: ObjectId) {
    if let Some(m) = marks.get_mut(id.0 as usize) {
        if *m == WHITE {
            *m = GREY;
            grey.push(id.0);
        }
    }
}

impl Heap {
    /// Offset of the object table (a `Vec<Option<Object>>`) in the heap,
    /// for native code.
    #[cfg(feature = "jit")]
    pub(crate) const OBJECTS_OFFSET: usize = std::mem::offset_of!(Heap, objects);
    /// Offset of [`Heap::marking`] (one byte), for native code.
    #[cfg(feature = "jit")]
    pub(crate) const MARKING_OFFSET: usize = std::mem::offset_of!(Heap, marking);
    /// Offset of the mark bytes (a `Vec<u8>`, one per object table slot
    /// while a cycle runs), for native code.
    #[cfg(feature = "jit")]
    pub(crate) const MARKS_OFFSET: usize = std::mem::offset_of!(Heap, marks);

    pub fn new() -> Heap {
        Heap {
            objects: Vec::with_capacity(1024),
            free: Vec::new(),
            marks: Vec::new(),
            marking: false,
            phase: GcPhase::Idle,
            grey: Vec::new(),
            weak: Vec::new(),
            ephemerons: Default::default(),
            sweep_at: 0,
            sweep_end: 0,
            swept: Vec::new(),
            allocated_since_gc: 0,
            live_count: 0,
            allocated_total: 0,
            last_freed: 0,
            collections: 1,
        }
    }

    pub fn alloc(&mut self, obj: Object) -> ObjectId {
        self.allocated_since_gc += 1;
        self.allocated_total += 1;
        self.live_count += 1;
        let i = if let Some(i) = self.free.pop() {
            self.objects[i as usize] = Some(obj);
            i
        } else {
            if self.objects.len() == self.objects.capacity() {
                self.grow();
            }
            self.objects.push(Some(obj));
            (self.objects.len() - 1) as u32
        };
        if self.phase != GcPhase::Idle {
            self.alloc_black(i as usize);
        }
        ObjectId(i)
    }

    /// Objects allocated during a cycle survive it.
    #[cold]
    fn alloc_black(&mut self, i: usize) {
        if i >= self.marks.len() {
            self.marks.resize(i + 1, WHITE);
        }
        self.marks[i] = BLACK;
    }

    /// Make room for more slots: double, or (when that much memory is not
    /// available) grow in smaller steps; out of memory, unwind with a panic
    /// (a catchable error at the nearest native call or host entry point)
    /// instead of aborting the process.
    #[cold]
    fn grow(&mut self) {
        let cap = self.objects.capacity();
        for extra in [cap.max(1024), cap / 4, cap / 16, 1024] {
            if extra > 0 && self.objects.try_reserve_exact(extra).is_ok() {
                return;
            }
        }
        panic!("out of memory: the JavaScript heap cannot grow past {cap} objects");
    }

    #[inline]
    pub fn get(&self, id: ObjectId) -> &Object {
        match self.objects.get(id.0 as usize).and_then(|o| o.as_ref()) {
            Some(o) => o,
            None => freed_object(id),
        }
    }

    /// The object, for changing it (runs the write barrier while marking).
    #[inline]
    pub fn get_mut(&mut self, id: ObjectId) -> &mut Object {
        self.barrier(id);
        self.objects[id.0 as usize].as_mut().expect("use of freed object")
    }

    #[inline]
    pub fn try_get_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
        self.barrier(id);
        self.objects.get_mut(id.0 as usize).and_then(|o| o.as_mut())
    }

    #[inline]
    pub fn try_get(&self, id: ObjectId) -> Option<&Object> {
        self.objects.get(id.0 as usize).and_then(|o| o.as_ref())
    }

    /// Write barrier: while marking, an object about to change is scanned
    /// first if it was not yet (snapshot at the beginning: the references
    /// it held when the cycle started are kept). Callers that write into
    /// an object without [`Heap::get_mut`] must call it first.
    #[inline(always)]
    pub fn barrier(&mut self, id: ObjectId) {
        if self.marking {
            if let Some(&m) = self.marks.get(id.0 as usize) {
                if m != BLACK {
                    self.barrier_slow(id.0 as usize);
                }
            }
        }
    }

    #[cold]
    #[inline(never)]
    fn barrier_slow(&mut self, i: usize) {
        let Heap { objects, marks, grey, weak, .. } = self;
        if marks[i] == BLACK_WEAK {
            return;
        }
        let Some(obj) = objects[i].as_ref() else { return };
        visit_refs(obj, &mut |id| shade(marks, grey, id));
        if is_weak_kind(&obj.kind) {
            // Its entries wait for the ephemeron pass; values it is about
            // to lose are shaded by `WeakMap.prototype.set` / `delete`
            // (see `Heap::shade_value`).
            marks[i] = BLACK_WEAK;
            weak.push(i as u32);
        } else {
            marks[i] = BLACK;
        }
    }

    /// While marking: keep `v` for this cycle. For values about to be
    /// dropped from a structure the barrier does not see through (the
    /// entries of weak collections).
    #[inline]
    pub fn shade_value(&mut self, v: &Value) {
        if self.marking {
            if let Some(id) = v.as_object() {
                shade(&mut self.marks, &mut self.grey, id);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    pub fn phase(&self) -> GcPhase {
        self.phase
    }

    /// Start a collection cycle: everything reachable from `roots` now (or
    /// allocated from now on) survives it. Finishes a cycle in progress
    /// first.
    pub fn start_cycle(&mut self, roots: &[ObjectId]) {
        if self.phase != GcPhase::Idle {
            self.finish_cycle();
        }
        self.marks.clear();
        self.marks.resize(self.objects.len(), WHITE);
        self.grey.clear();
        self.weak.clear();
        self.ephemerons.clear();
        for &r in roots {
            if self.objects.get(r.0 as usize).is_some_and(|o| o.is_some()) {
                shade(&mut self.marks, &mut self.grey, r);
            }
        }
        self.allocated_since_gc = 0;
        self.phase = GcPhase::Marking;
        self.marking = true;
    }

    /// Mark within `budget`; `true` once marking is complete (the cycle
    /// then sweeps).
    pub fn mark_step(&mut self, budget: &mut GcBudget) -> bool {
        if self.phase != GcPhase::Marking {
            return true;
        }
        // Objects taken from the grey stack wait in a short queue before
        // being scanned: their slot is prefetched when they enter it and
        // their parts (property storage, boxed kind) half-way through, so
        // that the scan finds them loaded instead of missing the cache on
        // each in turn.
        const QUEUE: usize = 16;
        const QUEUE_PARTS: usize = 8;
        let mut queue = [0u32; QUEUE];
        loop {
            {
                let Heap { objects, marks, grey, weak, ephemerons, .. } = self;
                let (mut head, mut len) = (0usize, 0usize);
                loop {
                    while len < QUEUE {
                        let Some(j) = grey.pop() else { break };
                        if marks[j as usize] != GREY {
                            continue;
                        }
                        let slot: *const Option<Object> = &objects[j as usize];
                        crate::alloc::prefetch(slot);
                        crate::alloc::prefetch((slot as *const u8).wrapping_add(64));
                        queue[(head + len) % QUEUE] = j;
                        len += 1;
                    }
                    if len == 0 {
                        break;
                    }
                    if len > QUEUE_PARTS {
                        if let Some(o) = &objects[queue[(head + QUEUE_PARTS) % QUEUE] as usize] {
                            o.prefetch_parts();
                        }
                    }
                    let i = queue[head] as usize;
                    head = (head + 1) % QUEUE;
                    len -= 1;
                    if marks[i] != GREY {
                        continue;
                    }
                    if !ephemerons.is_empty() {
                        if let Some(vs) = ephemerons.remove(&(i as u32)) {
                            for v in vs {
                                shade(marks, grey, ObjectId(v));
                            }
                        }
                    }
                    let Some(obj) = objects[i].as_ref() else {
                        marks[i] = BLACK;
                        continue;
                    };
                    let mut n = 1;
                    visit_refs(obj, &mut |id| {
                        n += 1;
                        shade(marks, grey, id)
                    });
                    if is_weak_kind(&obj.kind) {
                        marks[i] = BLACK_WEAK;
                        weak.push(i as u32);
                    } else {
                        marks[i] = BLACK;
                    }
                    if budget.spend(1 + n / 16) {
                        // The queued objects are still grey: back to the stack.
                        for k in 0..len {
                            grey.push(queue[(head + k) % QUEUE]);
                        }
                        return false;
                    }
                }
            }
            // WeakMap / WeakSet entries whose key is reachable keep their
            // value (which may make more keys reachable): repeat until
            // nothing new is marked.
            let Heap { objects, marks, grey, weak, ephemerons, .. } = self;
            ephemerons.clear();
            for &w in weak.iter() {
                if let Some(Object { kind: ObjectKind::WeakMap(m) | ObjectKind::WeakSet(m), .. }) = &objects[w as usize] {
                    for (k, v) in m.entries.iter().flatten() {
                        if let Some(vid) = v.as_object() {
                            if marks.get(vid.0 as usize).is_some_and(|&m| m != WHITE) {
                                continue;
                            }
                            match k.as_object().map(|kid| (kid, marks.get(kid.0 as usize).copied())) {
                                Some((kid, Some(WHITE))) => ephemerons.entry(kid.0).or_default().push(vid.0),
                                Some((_, None)) => {}
                                _ => shade(marks, grey, vid),
                            }
                        }
                    }
                }
            }
            if grey.is_empty() {
                break;
            }
        }
        self.marking = false;
        self.phase = GcPhase::Sweeping;
        self.sweep_at = 0;
        self.sweep_end = self.objects.len();
        self.swept.clear();
        true
    }

    /// Sweep within `budget`: free unmarked objects, drop the entries of
    /// weak collections whose keys died. `true` once the cycle is done.
    pub fn sweep_step(&mut self, budget: &mut GcBudget) -> bool {
        match self.phase {
            GcPhase::Idle => return true,
            GcPhase::Marking => return false,
            GcPhase::Sweeping => {}
        }
        let debug = std::env::var_os("RH_DEBUG_GC").is_some();
        // Look-ahead (in slots) of the prefetches of what dead objects
        // own: freeing them reads their scattered parts, which would
        // otherwise each miss the cache in turn.
        const AHEAD: usize = 16;
        const AHEAD_INNER: usize = 8;
        while self.sweep_at < self.sweep_end {
            let i = self.sweep_at;
            self.sweep_at += 1;
            // The slots themselves, further ahead (the look-ahead above
            // reads them).
            crate::alloc::prefetch(self.objects.as_ptr().wrapping_add(i + 2 * AHEAD));
            if i + AHEAD < self.sweep_end && self.marks[i + AHEAD] == WHITE {
                if let Some(o) = &self.objects[i + AHEAD] {
                    o.prefetch_parts();
                }
            }
            if i + AHEAD_INNER < self.sweep_end && self.marks[i + AHEAD_INNER] == WHITE {
                if let Some(o) = &self.objects[i + AHEAD_INNER] {
                    o.prefetch_inner_parts();
                }
            }
            if self.objects[i].is_none() {
                continue;
            }
            if self.marks[i] == WHITE {
                // Unfinished iterators that die end their iteration.
                if let Some(Object { kind: ObjectKind::MapIterator { target, index, .. }, .. }) = &self.objects[i] {
                    let t = target.0 as usize;
                    if *index != MAP_ITER_DONE && self.marks.get(t).is_some_and(|&m| m != WHITE) {
                        if let Some(Object { kind: ObjectKind::Map(m) | ObjectKind::Set(m) | ObjectKind::WeakMap(m) | ObjectKind::WeakSet(m), .. }) = self.objects[t].as_mut() {
                            m.iteration_ended();
                        }
                    }
                }
                let slot = &mut self.objects[i];
                if debug {
                    let o = slot.as_ref().unwrap();
                    let keys: Vec<String> = o.props.iter().take(6).map(|(k, _)| format!("{k:?}")).collect();
                    FREED_INFO.with(|m| m.borrow_mut().insert(i as u32, format!("{} {:?} props={:?}", o.kind.name(), o.class_name(), keys)));
                }
                if let Some(o) = slot.as_mut() {
                    o.props.recycle();
                }
                *slot = None;
                self.swept.push(i as u32);
            } else if let Some(Object { kind: ObjectKind::WeakMap(m) | ObjectKind::WeakSet(m), .. }) = self.objects[i].as_mut() {
                // Entries whose key is unreachable go with it.
                let marks = &self.marks;
                let dead: Vec<Value> = m.entries.iter().flatten().filter(|(k, _)| k.as_object().is_some_and(|o| marks.get(o.0 as usize).is_some_and(|&c| c == WHITE))).map(|(k, _)| k.clone()).collect();
                for k in &dead {
                    m.delete(k);
                }
            }
            if budget.spend(1) {
                return false;
            }
        }
        let freed = self.swept.len();
        self.live_count -= freed;
        self.last_freed = freed;
        // Reuse low slots first.
        self.swept.reverse();
        self.free.append(&mut self.swept);
        self.collections = self.collections.wrapping_add(1).max(1);
        self.phase = GcPhase::Idle;
        true
    }

    /// Run the cycle in progress to its end; returns the objects it freed.
    pub fn finish_cycle(&mut self) -> usize {
        if self.phase == GcPhase::Idle {
            return 0;
        }
        self.mark_step(&mut GcBudget::unlimited());
        self.sweep_step(&mut GcBudget::unlimited());
        self.last_freed
    }

    /// Run a whole collection now. `roots` must contain every externally
    /// reachable object id; everything else reachable is found by tracing.
    pub fn collect(&mut self, roots: &[ObjectId]) -> usize {
        let stats = crate::gcstats::enabled();
        self.finish_cycle();
        let t0 = std::time::Instant::now();
        self.start_cycle(roots);
        self.mark_step(&mut GcBudget::unlimited());
        let t1 = std::time::Instant::now();
        self.sweep_step(&mut GcBudget::unlimited());
        if stats {
            crate::gcstats::record_phase(1, t1 - t0);
            crate::gcstats::record_phase(3, t1.elapsed());
        }
        self.last_freed
    }

}

/// Call `f` with every object directly referenced by `obj` (strong
/// references: the entries of weak collections are left out).
#[inline]
pub fn visit_refs(obj: &Object, f: &mut impl FnMut(ObjectId)) {
    #[inline(always)]
    fn val(f: &mut impl FnMut(ObjectId), v: &Value) {
        if let Some(o) = v.as_object() {
            f(o);
        }
    }
    #[inline(always)]
    fn opt(f: &mut impl FnMut(ObjectId), v: &Option<Value>) {
        if let Some(v) = v {
            val(f, v);
        }
    }
    if let Some(p) = obj.proto {
        f(p);
    }
    obj.props.for_each_value(|p| {
        val(f, &p.value);
        if let Some(a) = &p.acc {
            opt(f, &a.getter);
            opt(f, &a.setter);
        }
    });
    match &obj.kind {
        ObjectKind::Array { elements, .. } => {
            for v in elements {
                val(f, v);
            }
        }
        ObjectKind::Function(c) => {
            if let Some(s) = c.scope {
                f(s);
            }
            opt(f, &c.this_value);
            opt(f, &c.new_target);
            if let Some(h) = c.home_object {
                f(h);
            }
            opt(f, &c.fields);
            opt(f, &c.arguments);
        }
        ObjectKind::Native(n) => {
            for v in &n.captures {
                val(f, v);
            }
        }
        ObjectKind::Bound(b) => {
            f(b.target);
            val(f, &b.this);
            for v in &b.args {
                val(f, v);
            }
        }
        ObjectKind::Scope(s) => {
            if let Some(p) = s.parent {
                f(p);
            }
            for v in s.slots.iter() {
                val(f, v);
            }
        }
        ObjectKind::Map(m) | ObjectKind::Set(m) => {
            for (k, v) in m.entries.iter().flatten() {
                val(f, k);
                val(f, v);
            }
        }
        // Ephemerons: an entry keeps its value alive only while its key is
        // alive otherwise (see `Heap::mark_step`).
        ObjectKind::WeakMap(_) | ObjectKind::WeakSet(_) => {}
        ObjectKind::Generator(g) => {
            if let Some(fr) = &g.frame {
                visit_frame(fr, f);
            }
            if let Some(p) = g.promise {
                f(p);
            }
            for (_, v, p) in &g.queue {
                val(f, v);
                f(*p);
            }
        }
        ObjectKind::Promise(p) => {
            val(f, &p.value);
            for r in p.fulfill_reactions.iter().chain(p.reject_reactions.iter()) {
                opt(f, &r.handler);
                if let Some((a, b, c)) = &r.capability {
                    val(f, a);
                    val(f, b);
                    val(f, c);
                }
                if let Some((_, v)) = &r.native {
                    val(f, v);
                }
            }
        }
        ObjectKind::TypedArray(t) | ObjectKind::DataView(t) => f(t.buffer),
        ObjectKind::Proxy { target, handler, .. } => {
            f(*target);
            f(*handler);
        }
        ObjectKind::ArrayIterator { target, .. } => val(f, target),
        ObjectKind::MapIterator { target, .. } => f(*target),
        ObjectKind::RegExpStringIterator { regexp, .. } => f(*regexp),
        ObjectKind::ForInIterator(s) => {
            if let Some(o) = s.object {
                f(o);
            }
        }
        ObjectKind::ModuleBinding { scope, .. } => f(*scope),
        ObjectKind::MappedArguments(m) => f(m.scope),
        ObjectKind::Slots(d) => {
            for v in d.values.iter().chain(d.list.iter()) {
                val(f, v);
            }
        }
        _ => {}
    }
}

/// Objects referenced by a suspended frame (see [`trace_frame`]).
pub fn visit_frame(fr: &Frame, f: &mut impl FnMut(ObjectId)) {
    let mut val = |v: &Value| {
        if let Some(o) = v.as_object() {
            f(o);
        }
    };
    val(&Value::Object(fr.scope));
    val(&fr.this);
    if let Some(func) = fr.func {
        val(&Value::Object(func));
    }
    val(&fr.new_target);
    if let Some(e) = &fr.extra {
        if let Some(a) = &e.arguments_obj {
            val(a);
        }
        if let Some(g) = e.generator {
            val(&Value::Object(g));
        }
        val(&e.completion);
        for v in &e.saved {
            val(v);
        }
    }
}

/// Collect every value directly referenced by `obj`.
pub fn trace_object(obj: &Object, out: &mut Vec<Value>) {
    visit_refs(obj, &mut |id| out.push(Value::Object(id)));
}

fn push_val(out: &mut Vec<Value>, v: &Value) {
    if v.is_object() {
        out.push(v.clone());
    }
}

fn push_opt(out: &mut Vec<Value>, v: &Option<Value>) {
    if let Some(v) = v {
        push_val(out, v);
    }
}

fn push_id(out: &mut Vec<Value>, id: Option<ObjectId>) {
    if let Some(id) = id {
        out.push(Value::Object(id));
    }
}

pub fn trace_reaction(r: &PromiseReaction, out: &mut Vec<Value>) {
    push_opt(out, &r.handler);
    if let Some((a, b, c)) = &r.capability {
        push_val(out, a);
        push_val(out, b);
        push_val(out, c);
    }
    if let Some((_, v)) = &r.native {
        push_val(out, v);
    }
}

/// Values referenced by a frame itself; the slots of an active frame are
/// traced with the realm's value stack, those of a suspended one here.
pub fn trace_frame(f: &Frame, out: &mut Vec<Value>) {
    out.push(Value::Object(f.scope));
    push_val(out, &f.this);
    push_id(out, f.func);
    push_val(out, &f.new_target);
    if let Some(e) = &f.extra {
        push_opt(out, &e.arguments_obj);
        push_id(out, e.generator);
        push_val(out, &e.completion);
        for v in &e.saved {
            push_val(out, v);
        }
    }
}
