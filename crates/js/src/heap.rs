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
    pub slots: Vec<Value>,
    pub parent: Option<ObjectId>,
    /// Slot names (for TDZ error messages); `None` for function scopes,
    /// whose names live on the `FunctionCode`.
    pub names: Option<Rc<Vec<JsStr>>>,
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
}

impl TypedArrayKind {
    pub fn element_size(&self) -> usize {
        match self {
            TypedArrayKind::Int8 | TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped => 1,
            TypedArrayKind::Int16 | TypedArrayKind::Uint16 => 2,
            TypedArrayKind::Int32 | TypedArrayKind::Uint32 | TypedArrayKind::Float32 => 4,
            _ => 8,
        }
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
        }
    }
}

#[derive(Debug)]
pub struct TypedArrayData {
    pub kind: TypedArrayKind,
    pub buffer: ObjectId,
    pub byte_offset: usize,
    pub length: usize,
}

#[derive(Debug)]
pub struct RegExpData {
    /// Compiled program, shared by every RegExp with the same source and
    /// flags (see `builtins::regexp::compiled`).
    pub regex: Rc<crate::regex::Regex>,
    pub source: JsStr,
    pub flags: JsStr,
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
    Scope(Box<ScopeData>),
    Error,
    Arguments,
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
    ArrayBuffer(Rc<std::cell::RefCell<Vec<u8>>>),
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
            ObjectKind::Arguments => "Arguments",
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
}

impl Object {
    pub fn new(proto: Option<ObjectId>, kind: ObjectKind) -> Object {
        Object { proto, props: PropertyMap::new(), extensible: true, kind, class_tag: 0, htmldda: false }
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
pub const SLOT_BYTES: usize = std::mem::size_of::<Option<Object>>();

/// Mark-and-sweep garbage collected arena.
pub struct Heap {
    objects: Vec<Option<Object>>,
    free: Vec<u32>,
    marks: Vec<bool>,
    pub allocated_since_gc: usize,
    pub live_count: usize,
    /// Collections run so far (object ids are only reused after one).
    pub collections: u32,
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

impl Heap {
    pub fn new() -> Heap {
        Heap { objects: Vec::with_capacity(1024), free: Vec::new(), marks: Vec::new(), allocated_since_gc: 0, live_count: 0, collections: 1 }
    }

    pub fn alloc(&mut self, obj: Object) -> ObjectId {
        self.allocated_since_gc += 1;
        self.live_count += 1;
        if let Some(i) = self.free.pop() {
            self.objects[i as usize] = Some(obj);
            ObjectId(i)
        } else {
            self.objects.push(Some(obj));
            ObjectId((self.objects.len() - 1) as u32)
        }
    }

    pub fn get(&self, id: ObjectId) -> &Object {
        match self.objects[id.0 as usize].as_ref() {
            Some(o) => o,
            None => {
                let info = FREED_INFO.with(|m| m.borrow().get(&id.0).cloned()).unwrap_or_default();
                panic!("use of freed object #{} {}", id.0, info)
            }
        }
    }

    pub fn get_mut(&mut self, id: ObjectId) -> &mut Object {
        self.objects[id.0 as usize].as_mut().expect("use of freed object")
    }

    #[inline]
    pub fn try_get_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
        self.objects.get_mut(id.0 as usize).and_then(|o| o.as_mut())
    }

    #[inline]
    pub fn try_get(&self, id: ObjectId) -> Option<&Object> {
        self.objects.get(id.0 as usize).and_then(|o| o.as_ref())
    }

    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// Run a collection. `roots` must contain every externally reachable
    /// object id; everything else reachable is found by tracing.
    pub fn collect(&mut self, roots: &[ObjectId]) -> usize {
        self.collections = self.collections.wrapping_add(1).max(1);
        self.marks.clear();
        self.marks.resize(self.objects.len(), false);
        let mut work: Vec<ObjectId> = Vec::with_capacity(roots.len());
        for &r in roots {
            self.mark_push(r, &mut work);
        }
        let mut children: Vec<Value> = Vec::new();
        let mut weak: Vec<ObjectId> = Vec::new();
        loop {
            while let Some(id) = work.pop() {
                children.clear();
                if let Some(obj) = &self.objects[id.0 as usize] {
                    if matches!(obj.kind, ObjectKind::WeakMap(_) | ObjectKind::WeakSet(_)) {
                        weak.push(id);
                    }
                    trace_object(obj, &mut children);
                }
                for v in &children {
                    if let V::Object(o) = v.v() {
                        self.mark_push(o, &mut work);
                    }
                }
            }
            // WeakMap / WeakSet entries whose key is reachable keep their
            // value (which may make more keys reachable): repeat until
            // nothing new is marked.
            children.clear();
            for &w in &weak {
                if let Some(Object { kind: ObjectKind::WeakMap(m) | ObjectKind::WeakSet(m), .. }) = &self.objects[w.0 as usize] {
                    for (k, v) in m.entries.iter().flatten() {
                        if self.value_alive(k) && !self.value_alive(v) {
                            children.push(v.clone());
                        }
                    }
                }
            }
            if children.is_empty() {
                break;
            }
            for v in std::mem::take(&mut children) {
                if let V::Object(o) = v.v() {
                    self.mark_push(o, &mut work);
                }
            }
        }
        // Entries whose key is unreachable go with it.
        for &w in &weak {
            let dead: Vec<Value> = match &self.objects[w.0 as usize] {
                Some(Object { kind: ObjectKind::WeakMap(m) | ObjectKind::WeakSet(m), .. }) => m.entries.iter().flatten().filter(|(k, _)| !self.value_alive(k)).map(|(k, _)| k.clone()).collect(),
                _ => continue,
            };
            if let Some(Object { kind: ObjectKind::WeakMap(m) | ObjectKind::WeakSet(m), .. }) = &mut self.objects[w.0 as usize] {
                for k in &dead {
                    m.delete(k);
                }
            }
        }
        let mut freed = 0;
        let debug = std::env::var_os("RH_DEBUG_GC").is_some();
        // Unfinished iterators that die end their iteration.
        let mut ended: Vec<u32> = Vec::new();
        for (i, slot) in self.objects.iter_mut().enumerate() {
            if slot.is_some() && !self.marks[i] {
                if let Some(Object { kind: ObjectKind::MapIterator { target, index, .. }, .. }) = slot {
                    if *index != MAP_ITER_DONE {
                        ended.push(target.0);
                    }
                }
                if debug {
                    let o = slot.as_ref().unwrap();
                    let keys: Vec<String> = o.props.iter().take(6).map(|(k, _)| format!("{k:?}")).collect();
                    FREED_INFO.with(|m| m.borrow_mut().insert(i as u32, format!("{} {:?} props={:?}", o.kind.name(), o.class_name(), keys)));
                }
                if let Some(o) = slot.as_mut() {
                    o.props.recycle();
                }
                *slot = None;
                self.free.push(i as u32);
                freed += 1;
            }
        }
        for t in ended {
            if let Some(Object { kind: ObjectKind::Map(m) | ObjectKind::Set(m) | ObjectKind::WeakMap(m) | ObjectKind::WeakSet(m), .. }) = &mut self.objects[t as usize] {
                m.iteration_ended();
            }
        }
        self.live_count -= freed;
        self.allocated_since_gc = 0;
        freed
    }

    /// During a collection: whether `v` is marked (non-objects always are).
    fn value_alive(&self, v: &Value) -> bool {
        match v.v() {
            V::Object(o) => self.marks.get(o.0 as usize).copied().unwrap_or(false),
            _ => true,
        }
    }

    fn mark_push(&mut self, id: ObjectId, work: &mut Vec<ObjectId>) {
        let i = id.0 as usize;
        if i < self.marks.len() && !self.marks[i] && self.objects[i].is_some() {
            self.marks[i] = true;
            work.push(id);
        }
    }
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

/// Collect every value directly referenced by `obj`.
pub fn trace_object(obj: &Object, out: &mut Vec<Value>) {
    push_id(out, obj.proto);
    obj.props.for_each_value(|p| {
        push_val(out, &p.value);
        if let Some(g) = p.getter() {
            push_val(out, g);
        }
        if let Some(s) = p.setter() {
            push_val(out, s);
        }
    });
    match &obj.kind {
        ObjectKind::Array { elements, .. } => {
            for v in elements {
                push_val(out, v);
            }
        }
        ObjectKind::Function(c) => {
            push_id(out, c.scope);
            push_opt(out, &c.this_value);
            push_opt(out, &c.new_target);
            push_id(out, c.home_object);
            push_opt(out, &c.fields);
            push_opt(out, &c.arguments);
        }
        ObjectKind::Native(n) => {
            for v in &n.captures {
                push_val(out, v);
            }
        }
        ObjectKind::Bound(b) => {
            out.push(Value::Object(b.target));
            push_val(out, &b.this);
            for v in &b.args {
                push_val(out, v);
            }
        }
        ObjectKind::Scope(s) => {
            push_id(out, s.parent);
            for v in &s.slots {
                push_val(out, v);
            }
        }
        ObjectKind::Map(m) | ObjectKind::Set(m) => {
            for (k, v) in m.entries.iter().flatten() {
                push_val(out, k);
                push_val(out, v);
            }
        }
        // Ephemerons: an entry keeps its value alive only while its key is
        // alive otherwise (see `Heap::collect`).
        ObjectKind::WeakMap(_) | ObjectKind::WeakSet(_) => {}
        ObjectKind::Generator(g) => {
            if let Some(f) = &g.frame {
                trace_frame(f, out);
            }
            push_id(out, g.promise);
            for (_, v, p) in &g.queue {
                push_val(out, v);
                out.push(Value::Object(*p));
            }
        }
        ObjectKind::Promise(p) => {
            push_val(out, &p.value);
            for r in p.fulfill_reactions.iter().chain(p.reject_reactions.iter()) {
                trace_reaction(r, out);
            }
        }
        ObjectKind::TypedArray(t) | ObjectKind::DataView(t) => out.push(Value::Object(t.buffer)),
        ObjectKind::Proxy { target, handler, .. } => {
            out.push(Value::Object(*target));
            out.push(Value::Object(*handler));
        }
        ObjectKind::ArrayIterator { target, .. } => push_val(out, target),
        ObjectKind::MapIterator { target, .. } => out.push(Value::Object(*target)),
        ObjectKind::RegExpStringIterator { regexp, .. } => out.push(Value::Object(*regexp)),
        ObjectKind::ForInIterator(f) => push_id(out, f.object),
        ObjectKind::ModuleBinding { scope, .. } => out.push(Value::Object(*scope)),
        _ => {}
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
