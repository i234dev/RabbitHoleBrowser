//! Object model and garbage-collected heap.

use crate::bytecode::FunctionCode;
use crate::value::{BigInt, JsStr, ObjectId, PropertyKey, SymbolId, Value};
use crate::vm::Frame;
use std::any::Any;
use std::collections::HashMap;
use std::rc::Rc;

pub const WRITABLE: u8 = 1;
pub const ENUMERABLE: u8 = 2;
pub const CONFIGURABLE: u8 = 4;
pub const ACCESSOR: u8 = 8;
pub const ALL: u8 = WRITABLE | ENUMERABLE | CONFIGURABLE;
/// Built-in methods: writable + configurable, not enumerable.
pub const HIDDEN: u8 = WRITABLE | CONFIGURABLE;

#[derive(Clone, Debug)]
pub struct Property {
    pub value: Value,
    pub getter: Option<Value>,
    pub setter: Option<Value>,
    pub flags: u8,
}

impl Property {
    pub fn data(value: Value, flags: u8) -> Property {
        Property { value, getter: None, setter: None, flags: flags & !ACCESSOR }
    }
    pub fn accessor(getter: Option<Value>, setter: Option<Value>, flags: u8) -> Property {
        Property { value: Value::Undefined, getter, setter, flags: (flags & !WRITABLE) | ACCESSOR }
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

/// Insertion-ordered property storage.
#[derive(Default, Debug, Clone)]
pub struct PropertyMap {
    entries: Vec<Option<(PropertyKey, Property)>>,
    index: HashMap<PropertyKey, usize>,
}

impl PropertyMap {
    pub fn new() -> PropertyMap {
        PropertyMap::default()
    }
    pub fn len(&self) -> usize {
        self.index.len()
    }
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
    pub fn get(&self, key: &PropertyKey) -> Option<&Property> {
        self.index.get(key).and_then(|&i| self.entries[i].as_ref().map(|(_, p)| p))
    }
    pub fn get_mut(&mut self, key: &PropertyKey) -> Option<&mut Property> {
        match self.index.get(key) {
            Some(&i) => self.entries[i].as_mut().map(|(_, p)| p),
            None => None,
        }
    }
    pub fn contains(&self, key: &PropertyKey) -> bool {
        self.index.contains_key(key)
    }
    pub fn insert(&mut self, key: PropertyKey, prop: Property) {
        if let Some(&i) = self.index.get(&key) {
            self.entries[i] = Some((key, prop));
        } else {
            self.index.insert(key.clone(), self.entries.len());
            self.entries.push(Some((key, prop)));
        }
    }
    pub fn remove(&mut self, key: &PropertyKey) -> Option<Property> {
        let i = self.index.remove(key)?;
        let out = self.entries[i].take().map(|(_, p)| p);
        if self.entries.len() > 32 && self.index.len() * 2 < self.entries.len() {
            self.compact();
        }
        out
    }
    fn compact(&mut self) {
        let old = std::mem::take(&mut self.entries);
        self.index.clear();
        for e in old.into_iter().flatten() {
            self.index.insert(e.0.clone(), self.entries.len());
            self.entries.push(Some(e));
        }
    }
    /// Keys in spec order: integer indices ascending, then strings in
    /// insertion order, then symbols in insertion order.
    pub fn keys(&self) -> Vec<PropertyKey> {
        let mut ints: Vec<(u32, PropertyKey)> = Vec::new();
        let mut strs = Vec::new();
        let mut syms = Vec::new();
        for (k, _) in self.entries.iter().flatten() {
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
    pub fn iter(&self) -> impl Iterator<Item = (&PropertyKey, &Property)> {
        self.entries.iter().flatten().map(|(k, p)| (k, p))
    }
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut Property> {
        self.entries.iter_mut().flatten().map(|(_, p)| p)
    }
}

pub type NativeFn = fn(&mut crate::realm::Realm, Value, &[Value]) -> Result<Value, Value>;

pub struct NativeFunction {
    pub func: NativeFn,
    pub name: JsStr,
    pub data: Option<Rc<dyn Any>>,
    /// Values kept alive for the native (e.g. captured JS callbacks).
    pub captures: Vec<Value>,
    pub is_constructor: bool,
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
    /// Bitmask-ish list of const slots (small; checked on SetLocalChecked).
    pub consts: Vec<bool>,
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

/// Hashable form of a value for Map/Set (SameValueZero).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ValueKey {
    Undefined,
    Null,
    Bool(bool),
    Number(u64),
    String(JsStr),
    Symbol(SymbolId),
    BigInt(i128),
    Object(ObjectId),
}

impl ValueKey {
    pub fn from_value(v: &Value) -> ValueKey {
        match v {
            Value::Undefined | Value::Empty => ValueKey::Undefined,
            Value::Null => ValueKey::Null,
            Value::Bool(b) => ValueKey::Bool(*b),
            Value::Number(n) => {
                let n = if n.is_nan() { f64::NAN } else if *n == 0.0 { 0.0 } else { *n };
                ValueKey::Number(n.to_bits())
            }
            Value::String(s) => ValueKey::String(s.clone()),
            Value::Symbol(s) => ValueKey::Symbol(*s),
            Value::BigInt(b) => ValueKey::BigInt(b.0),
            Value::Object(o) => ValueKey::Object(*o),
        }
    }
}

#[derive(Debug, Default)]
pub struct MapData {
    pub entries: Vec<Option<(Value, Value)>>,
    pub index: HashMap<ValueKey, usize>,
}

impl MapData {
    pub fn get(&self, k: &Value) -> Option<&Value> {
        self.index.get(&ValueKey::from_value(k)).and_then(|&i| self.entries[i].as_ref().map(|(_, v)| v))
    }
    pub fn has(&self, k: &Value) -> bool {
        self.index.contains_key(&ValueKey::from_value(k))
    }
    pub fn set(&mut self, k: Value, v: Value) {
        let key = ValueKey::from_value(&k);
        let k = match k {
            Value::Number(n) if n == 0.0 => Value::Number(0.0),
            other => other,
        };
        if let Some(&i) = self.index.get(&key) {
            self.entries[i] = Some((k, v));
        } else {
            self.index.insert(key, self.entries.len());
            self.entries.push(Some((k, v)));
        }
    }
    pub fn delete(&mut self, k: &Value) -> bool {
        match self.index.remove(&ValueKey::from_value(k)) {
            Some(i) => {
                self.entries[i] = None;
                true
            }
            None => false,
        }
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.index.clear();
    }
    pub fn size(&self) -> usize {
        self.index.len()
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
    pub regex: crate::regex::Regex,
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
    ForInIterator { keys: Vec<JsStr>, index: usize, object: Option<ObjectId> },
    /// Host-defined (DOM nodes etc.)
    Host(Box<dyn Any>),
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
            ObjectKind::ForInIterator { .. } => "ForIn Iterator",
            ObjectKind::Host(_) => "Object",
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
    /// `Object.prototype.toString` tag override for host objects.
    pub class_name: Option<&'static str>,
}

impl Object {
    pub fn new(proto: Option<ObjectId>, kind: ObjectKind) -> Object {
        Object { proto, props: PropertyMap::new(), extensible: true, kind, class_name: None }
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

/// Mark-and-sweep garbage collected arena.
pub struct Heap {
    objects: Vec<Option<Object>>,
    free: Vec<u32>,
    marks: Vec<bool>,
    pub allocated_since_gc: usize,
    pub live_count: usize,
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

impl Heap {
    pub fn new() -> Heap {
        Heap { objects: Vec::with_capacity(1024), free: Vec::new(), marks: Vec::new(), allocated_since_gc: 0, live_count: 0 }
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
        self.objects[id.0 as usize].as_ref().expect("use of freed object")
    }

    pub fn get_mut(&mut self, id: ObjectId) -> &mut Object {
        self.objects[id.0 as usize].as_mut().expect("use of freed object")
    }

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
        self.marks.clear();
        self.marks.resize(self.objects.len(), false);
        let mut work: Vec<ObjectId> = Vec::with_capacity(roots.len());
        for &r in roots {
            self.mark_push(r, &mut work);
        }
        let mut children: Vec<Value> = Vec::new();
        while let Some(id) = work.pop() {
            children.clear();
            if let Some(obj) = &self.objects[id.0 as usize] {
                trace_object(obj, &mut children);
            }
            for v in &children {
                if let Value::Object(o) = v {
                    self.mark_push(*o, &mut work);
                }
            }
        }
        let mut freed = 0;
        for (i, slot) in self.objects.iter_mut().enumerate() {
            if slot.is_some() && !self.marks[i] {
                *slot = None;
                self.free.push(i as u32);
                freed += 1;
            }
        }
        self.live_count -= freed;
        self.allocated_since_gc = 0;
        freed
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
    for (_, p) in obj.props.iter() {
        push_val(out, &p.value);
        push_opt(out, &p.getter);
        push_opt(out, &p.setter);
    }
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
        ObjectKind::Map(m) | ObjectKind::Set(m) | ObjectKind::WeakMap(m) | ObjectKind::WeakSet(m) => {
            for (k, v) in m.entries.iter().flatten() {
                push_val(out, k);
                push_val(out, v);
            }
        }
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
        ObjectKind::ForInIterator { object, .. } => push_id(out, *object),
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

pub fn trace_frame(f: &Frame, out: &mut Vec<Value>) {
    for v in &f.stack {
        push_val(out, v);
    }
    out.push(Value::Object(f.scope));
    push_val(out, &f.this);
    push_id(out, f.func);
    push_val(out, &f.new_target);
    push_id(out, f.home_object);
    push_opt(out, &f.arguments_obj);
    for v in &f.args {
        push_val(out, v);
    }
    push_id(out, f.generator);
}
