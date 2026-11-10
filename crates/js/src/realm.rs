//! The realm: heap, intrinsics, global object, job queue and the embedding API.

use crate::bytecode::FunctionCode;
use crate::heap::*;
use crate::value::*;
use crate::vm::Frame;
use std::any::Any;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

pub const SYM_ITERATOR: SymbolId = SymbolId(0);
pub const SYM_ASYNC_ITERATOR: SymbolId = SymbolId(1);
pub const SYM_HAS_INSTANCE: SymbolId = SymbolId(2);
pub const SYM_TO_PRIMITIVE: SymbolId = SymbolId(3);
pub const SYM_TO_STRING_TAG: SymbolId = SymbolId(4);
pub const SYM_SPECIES: SymbolId = SymbolId(5);
pub const SYM_IS_CONCAT_SPREADABLE: SymbolId = SymbolId(6);
pub const SYM_UNSCOPABLES: SymbolId = SymbolId(7);
pub const SYM_MATCH: SymbolId = SymbolId(8);
pub const SYM_MATCH_ALL: SymbolId = SymbolId(9);
pub const SYM_REPLACE: SymbolId = SymbolId(10);
pub const SYM_SEARCH: SymbolId = SymbolId(11);
pub const SYM_SPLIT: SymbolId = SymbolId(12);

pub const WELL_KNOWN_SYMBOLS: [(&str, SymbolId); 13] = [
    ("iterator", SYM_ITERATOR),
    ("asyncIterator", SYM_ASYNC_ITERATOR),
    ("hasInstance", SYM_HAS_INSTANCE),
    ("toPrimitive", SYM_TO_PRIMITIVE),
    ("toStringTag", SYM_TO_STRING_TAG),
    ("species", SYM_SPECIES),
    ("isConcatSpreadable", SYM_IS_CONCAT_SPREADABLE),
    ("unscopables", SYM_UNSCOPABLES),
    ("match", SYM_MATCH),
    ("matchAll", SYM_MATCH_ALL),
    ("replace", SYM_REPLACE),
    ("search", SYM_SEARCH),
    ("split", SYM_SPLIT),
];

#[derive(Debug, Clone)]
pub struct SymbolInfo {
    pub description: Option<JsStr>,
    pub is_private: bool,
    pub registered: bool,
}

/// Commonly used intrinsic objects. Less common prototypes live in `others`.
pub struct Intrinsics {
    pub object_proto: ObjectId,
    pub function_proto: ObjectId,
    pub array_proto: ObjectId,
    pub string_proto: ObjectId,
    pub number_proto: ObjectId,
    pub boolean_proto: ObjectId,
    pub symbol_proto: ObjectId,
    pub bigint_proto: ObjectId,
    pub error_proto: ObjectId,
    pub iterator_proto: ObjectId,
    pub array_iterator_proto: ObjectId,
    pub generator_proto: ObjectId,
    pub async_generator_proto: ObjectId,
    pub promise_proto: ObjectId,
    pub promise_ctor: ObjectId,
    pub object_ctor: ObjectId,
    pub array_ctor: ObjectId,
    pub function_ctor: ObjectId,
    pub eval_fn: ObjectId,
    pub others: HashMap<&'static str, ObjectId>,
}

impl Intrinsics {
    pub fn get(&self, name: &str) -> Option<ObjectId> {
        match name {
            "ObjectPrototype" => Some(self.object_proto),
            "FunctionPrototype" => Some(self.function_proto),
            "ArrayPrototype" => Some(self.array_proto),
            "StringPrototype" => Some(self.string_proto),
            "NumberPrototype" => Some(self.number_proto),
            "BooleanPrototype" => Some(self.boolean_proto),
            "SymbolPrototype" => Some(self.symbol_proto),
            "BigIntPrototype" => Some(self.bigint_proto),
            "ErrorPrototype" => Some(self.error_proto),
            "IteratorPrototype" => Some(self.iterator_proto),
            "ArrayIteratorPrototype" => Some(self.array_iterator_proto),
            "GeneratorPrototype" => Some(self.generator_proto),
            "AsyncGeneratorPrototype" => Some(self.async_generator_proto),
            "PromisePrototype" => Some(self.promise_proto),
            "Promise" => Some(self.promise_ctor),
            "Object" => Some(self.object_ctor),
            "Array" => Some(self.array_ctor),
            "Function" => Some(self.function_ctor),
            _ => self.others.get(name).copied(),
        }
    }
}

pub enum Job {
    Reaction { reaction: PromiseReaction, argument: Value },
    ResolveThenable { promise: ObjectId, thenable: Value, then: Value },
    Callback { func: Value, this: Value, args: Vec<Value> },
}

#[derive(Default)]
pub struct RootTable {
    next: u64,
    pub values: HashMap<u64, Value>,
}

/// Keeps a value alive across garbage collections until dropped.
pub struct Root {
    id: u64,
    table: Rc<RefCell<RootTable>>,
    value: Value,
}

impl Root {
    pub fn value(&self) -> Value {
        self.value.clone()
    }
    pub fn object(&self) -> Option<ObjectId> {
        self.value.as_object()
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        self.table.borrow_mut().values.remove(&self.id);
    }
}

impl std::fmt::Debug for Root {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Root({:?})", self.value)
    }
}

pub type ConsoleSink = Box<dyn FnMut(&str, String)>;

pub struct Realm {
    pub heap: Heap,
    pub intrinsics: Intrinsics,
    pub symbols: Vec<SymbolInfo>,
    symbol_registry: HashMap<JsStr, SymbolId>,
    pub global: ObjectId,
    pub global_lex: crate::ic::GlobalLex,
    pub jobs: VecDeque<Job>,
    /// Values of jobs currently running (popped from `jobs`), kept alive.
    pub job_roots: Vec<Vec<Value>>,
    pub frames: Vec<Frame>,
    roots: Rc<RefCell<RootTable>>,
    pub console_sink: Option<ConsoleSink>,
    pub template_cache: HashMap<(usize, u32), ObjectId>,
    /// Depth of native calls currently on the Rust stack.
    pub native_depth: usize,
    /// `new.target` for the innermost native constructor call.
    pub new_target_stack: Vec<Value>,
    pub gc_threshold: usize,
    /// When set, running JavaScript past this instant is interrupted with an
    /// uncatchable "Script execution timed out" error (watchdog).
    pub deadline: Option<std::time::Instant>,
    /// A timeout is unwinding: `catch`/`finally` handlers are skipped.
    pub(crate) terminating: bool,
    pub gc_enabled: bool,
    pub unhandled_rejections: Vec<Value>,
    pub max_frames: usize,
    pub max_native_depth: usize,
    /// Host hook for `import()` / modules (unused by the core).
    pub host_data: Option<Box<dyn Any>>,
    /// Extra GC roots maintained by the host (e.g. DOM node wrappers).
    pub extra_roots: Vec<Value>,
    pub ops_executed: u64,
    /// The native function object currently executing (for captures).
    pub current_callee: Option<ObjectId>,
    /// Arrays currently being joined (cycle guard for `Array.prototype.join`).
    pub join_stack: Vec<ObjectId>,
    /// ECMAScript module records and the host module hooks.
    pub modules: crate::modules::ModuleRegistry,
    /// Countdown to the next GC / watchdog safe point in the VM loop.
    pub(crate) tick: u32,
    /// Recycled frame operand stacks.
    pub(crate) stack_pool: Vec<Vec<Value>>,
    /// Reusable buffer for call arguments taken off the operand stack.
    pub(crate) arg_buf: Vec<Value>,
}

impl Default for Realm {
    fn default() -> Self {
        Self::new()
    }
}

impl Realm {
    pub fn new() -> Realm {
        let mut heap = Heap::new();
        let object_proto = heap.alloc(Object::new(None, ObjectKind::Ordinary));
        let function_proto = heap.alloc(Object::new(
            Some(object_proto),
            ObjectKind::Native(Box::new(NativeFunction { func: |_, _, _| Ok(Value::Undefined), name: JsStr::empty(), data: None, captures: vec![], is_constructor: false })),
        ));
        let mk = |heap: &mut Heap, proto: ObjectId| heap.alloc(Object::new(Some(proto), ObjectKind::Ordinary));
        let array_proto = heap.alloc(Object::new(Some(object_proto), ObjectKind::Array { elements: vec![], length: 0 }));
        let string_proto = heap.alloc(Object::new(Some(object_proto), ObjectKind::String(JsStr::empty())));
        let number_proto = heap.alloc(Object::new(Some(object_proto), ObjectKind::Number(0.0)));
        let boolean_proto = heap.alloc(Object::new(Some(object_proto), ObjectKind::Boolean(false)));
        let symbol_proto = mk(&mut heap, object_proto);
        let bigint_proto = mk(&mut heap, object_proto);
        let error_proto = mk(&mut heap, object_proto);
        let iterator_proto = mk(&mut heap, object_proto);
        let array_iterator_proto = mk(&mut heap, iterator_proto);
        let generator_proto = mk(&mut heap, iterator_proto);
        let async_generator_proto = mk(&mut heap, object_proto);
        let promise_proto = mk(&mut heap, object_proto);
        let global = mk(&mut heap, object_proto);
        let intrinsics = Intrinsics {
            object_proto,
            function_proto,
            array_proto,
            string_proto,
            number_proto,
            boolean_proto,
            symbol_proto,
            bigint_proto,
            error_proto,
            iterator_proto,
            array_iterator_proto,
            generator_proto,
            async_generator_proto,
            promise_proto,
            promise_ctor: object_proto,
            object_ctor: object_proto,
            array_ctor: object_proto,
            function_ctor: object_proto,
            eval_fn: object_proto,
            others: HashMap::new(),
        };
        let mut symbols = Vec::new();
        for (name, _) in WELL_KNOWN_SYMBOLS.iter() {
            symbols.push(SymbolInfo { description: Some(JsStr::new(&format!("Symbol.{}", name))), is_private: false, registered: false });
        }
        let mut realm = Realm {
            heap,
            intrinsics,
            symbols,
            symbol_registry: HashMap::new(),
            global,
            global_lex: Default::default(),
            jobs: VecDeque::new(),
            job_roots: Vec::new(),
            frames: Vec::new(),
            roots: Rc::new(RefCell::new(RootTable::default())),
            console_sink: None,
            template_cache: HashMap::new(),
            native_depth: 0,
            new_target_stack: Vec::new(),
            gc_threshold: 200_000,
            deadline: None,
            terminating: false,
            gc_enabled: true,
            unhandled_rejections: Vec::new(),
            max_frames: 10_000,
            max_native_depth: 180,
            host_data: None,
            extra_roots: Vec::new(),
            ops_executed: 0,
            current_callee: None,
            join_stack: Vec::new(),
            modules: Default::default(),
            tick: crate::vm::TICK_INTERVAL,
            stack_pool: Vec::new(),
            arg_buf: Vec::new(),
        };
        crate::builtins::register_builtins(&mut realm);
        realm
    }

    // ------------------------------------------------------------ objects

    pub fn alloc(&mut self, obj: Object) -> ObjectId {
        self.heap.alloc(obj)
    }

    pub fn obj(&self, id: ObjectId) -> &Object {
        self.heap.get(id)
    }

    pub fn obj_mut(&mut self, id: ObjectId) -> &mut Object {
        self.heap.get_mut(id)
    }

    pub fn global(&self) -> ObjectId {
        self.global
    }

    pub fn new_object(&mut self) -> ObjectId {
        let proto = self.intrinsics.object_proto;
        self.alloc(Object::new(Some(proto), ObjectKind::Ordinary))
    }

    pub fn new_object_with_proto(&mut self, proto: Option<ObjectId>) -> ObjectId {
        self.alloc(Object::new(proto, ObjectKind::Ordinary))
    }

    pub fn new_array(&mut self, values: Vec<Value>) -> ObjectId {
        let proto = self.intrinsics.array_proto;
        let length = values.len() as u32;
        self.alloc(Object::new(Some(proto), ObjectKind::Array { elements: values, length }))
    }

    pub fn new_array_value(&mut self, values: Vec<Value>) -> Value {
        Value::Object(self.new_array(values))
    }

    pub fn new_string(&mut self, s: &str) -> Value {
        Value::String(JsStr::new(s))
    }

    pub fn new_native_function(&mut self, name: &str, arity: u32, func: NativeFn) -> ObjectId {
        self.new_native_function_full(name, arity, func, None, Vec::new(), false)
    }

    /// Native function with captured Rust data and/or captured JS values
    /// (kept alive by the GC).
    pub fn new_native_function_full(&mut self, name: &str, arity: u32, func: NativeFn, data: Option<Rc<dyn Any>>, captures: Vec<Value>, is_constructor: bool) -> ObjectId {
        let proto = self.intrinsics.function_proto;
        let id = self.alloc(Object::new(Some(proto), ObjectKind::Native(Box::new(NativeFunction { func, name: JsStr::new(name), data, captures, is_constructor }))));
        let o = self.heap.get_mut(id);
        o.props.insert(PropertyKey::str("length"), Property::data(Value::Number(arity as f64), CONFIGURABLE));
        o.props.insert(PropertyKey::str("name"), Property::data(Value::str(name), CONFIGURABLE));
        id
    }

    /// Native constructor: creates the `prototype` object (with `constructor`
    /// back-link) using `proto` as the prototype object's prototype.
    pub fn new_native_constructor(&mut self, name: &str, arity: u32, func: NativeFn, prototype: ObjectId) -> ObjectId {
        let f = self.new_native_function_full(name, arity, func, None, Vec::new(), true);
        self.obj_mut(f).props.insert(PropertyKey::str("prototype"), Property::data(Value::Object(prototype), 0));
        self.obj_mut(prototype).props.insert(PropertyKey::str("constructor"), Property::data(Value::Object(f), HIDDEN));
        f
    }

    /// Data captured by the native function currently executing (looked up on the callee object).
    pub fn native_data<T: 'static>(&self, func: ObjectId) -> Option<Rc<T>> {
        match &self.obj(func).kind {
            ObjectKind::Native(n) => n.data.clone().and_then(|d| d.downcast::<T>().ok()),
            _ => None,
        }
    }

    pub fn native_captures(&self, func: ObjectId) -> Vec<Value> {
        match &self.obj(func).kind {
            ObjectKind::Native(n) => n.captures.clone(),
            _ => Vec::new(),
        }
    }

    pub fn current_native(&self) -> Option<ObjectId> {
        self.current_callee
    }

    /// Define a data property (own) with the given flags.
    pub fn define_property(&mut self, obj: ObjectId, key: impl Into<PropertyKey>, value: Value, flags: u8) {
        self.obj_mut(obj).props.insert(key.into(), Property::data(value, flags));
    }

    /// Define a method (non-enumerable).
    pub fn define_method(&mut self, obj: ObjectId, name: &str, arity: u32, func: NativeFn) -> ObjectId {
        let f = self.new_native_function(name, arity, func);
        self.define_property(obj, name, Value::Object(f), HIDDEN);
        f
    }

    pub fn define_symbol_method(&mut self, obj: ObjectId, sym: SymbolId, name: &str, arity: u32, func: NativeFn) -> ObjectId {
        let f = self.new_native_function(name, arity, func);
        self.define_property(obj, sym, Value::Object(f), HIDDEN);
        f
    }

    pub fn define_accessor(&mut self, obj: ObjectId, key: impl Into<PropertyKey>, getter: Option<NativeFn>, setter: Option<NativeFn>, flags: u8) {
        let key = key.into();
        let name = match &key {
            PropertyKey::String(s) => s.to_rust_string(),
            PropertyKey::Symbol(s) => format!("[{}]", self.symbol_description(*s)),
        };
        let g = getter.map(|f| Value::Object(self.new_native_function(&format!("get {}", name), 0, f)));
        let s = setter.map(|f| Value::Object(self.new_native_function(&format!("set {}", name), 1, f)));
        self.obj_mut(obj).props.insert(key, Property::accessor(g, s, flags));
    }

    pub fn define_accessor_values(&mut self, obj: ObjectId, key: impl Into<PropertyKey>, getter: Option<Value>, setter: Option<Value>, flags: u8) {
        self.obj_mut(obj).props.insert(key.into(), Property::accessor(getter, setter, flags));
    }

    pub fn define_global(&mut self, name: &str, value: Value) {
        let g = self.global;
        self.define_property(g, name, value, HIDDEN);
    }

    pub fn set_host_data(&mut self, obj: ObjectId, data: Box<dyn Any>) {
        self.obj_mut(obj).kind = ObjectKind::Host(data);
    }

    pub fn host_data<T: 'static>(&self, obj: ObjectId) -> Option<&T> {
        match &self.obj(obj).kind {
            ObjectKind::Host(d) => d.downcast_ref::<T>(),
            _ => None,
        }
    }

    pub fn host_data_mut<T: 'static>(&mut self, obj: ObjectId) -> Option<&mut T> {
        match &mut self.obj_mut(obj).kind {
            ObjectKind::Host(d) => d.downcast_mut::<T>(),
            _ => None,
        }
    }

    pub fn set_class_name(&mut self, obj: ObjectId, name: &'static str) {
        self.obj_mut(obj).class_name = Some(name);
    }

    // ------------------------------------------------------------ symbols

    pub fn new_symbol(&mut self, description: Option<JsStr>) -> SymbolId {
        self.symbols.push(SymbolInfo { description, is_private: false, registered: false });
        SymbolId((self.symbols.len() - 1) as u32)
    }

    pub fn new_private_symbol(&mut self, description: JsStr) -> SymbolId {
        self.symbols.push(SymbolInfo { description: Some(description), is_private: true, registered: false });
        SymbolId((self.symbols.len() - 1) as u32)
    }

    pub fn symbol_for(&mut self, key: JsStr) -> SymbolId {
        if let Some(s) = self.symbol_registry.get(&key) {
            return *s;
        }
        self.symbols.push(SymbolInfo { description: Some(key.clone()), is_private: false, registered: true });
        let id = SymbolId((self.symbols.len() - 1) as u32);
        self.symbol_registry.insert(key, id);
        id
    }

    pub fn symbol_key_for(&self, sym: SymbolId) -> Option<JsStr> {
        let info = &self.symbols[sym.0 as usize];
        if info.registered {
            info.description.clone()
        } else {
            None
        }
    }

    pub fn symbol_description(&self, sym: SymbolId) -> String {
        self.symbols.get(sym.0 as usize).and_then(|s| s.description.as_ref()).map(|d| d.to_rust_string()).unwrap_or_default()
    }

    pub fn is_private_symbol(&self, sym: SymbolId) -> bool {
        self.symbols.get(sym.0 as usize).map(|s| s.is_private).unwrap_or(false)
    }

    // ------------------------------------------------------------- errors

    pub fn error_proto_for(&self, kind: &str) -> ObjectId {
        match kind {
            "Error" => self.intrinsics.error_proto,
            other => self.intrinsics.others.get(format!("{}Prototype", other).as_str()).copied().unwrap_or(self.intrinsics.error_proto),
        }
    }

    /// Create an error object of the given kind ("TypeError", ...).
    pub fn new_error(&mut self, kind: &str, message: &str) -> Value {
        let proto = self.error_proto_for(kind);
        let id = self.alloc(Object::new(Some(proto), ObjectKind::Error));
        self.define_property(id, "message", Value::str(message), HIDDEN);
        let stack = self.build_stack_string(kind, message);
        self.define_property(id, "stack", Value::String(JsStr::new(&stack)), HIDDEN);
        Value::Object(id)
    }

    pub fn throw_type_error(&mut self, message: &str) -> Value {
        self.new_error("TypeError", message)
    }
    pub fn throw_range_error(&mut self, message: &str) -> Value {
        self.new_error("RangeError", message)
    }
    pub fn throw_reference_error(&mut self, message: &str) -> Value {
        self.new_error("ReferenceError", message)
    }
    pub fn throw_syntax_error(&mut self, message: &str) -> Value {
        self.new_error("SyntaxError", message)
    }

    pub fn type_error<T>(&mut self, message: &str) -> Result<T, Value> {
        Err(self.new_error("TypeError", message))
    }
    pub fn range_error<T>(&mut self, message: &str) -> Result<T, Value> {
        Err(self.new_error("RangeError", message))
    }

    /// Render a stack trace from the current frames.
    pub fn build_stack_string(&self, kind: &str, message: &str) -> String {
        let mut s = if message.is_empty() { kind.to_string() } else { format!("{}: {}", kind, message) };
        for f in self.frames.iter().rev().take(32) {
            let pos = f.code.position_at(f.pc.saturating_sub(1));
            let name = if f.code.name.is_empty() { "<anonymous>".to_string() } else { f.code.name.to_rust_string() };
            s.push_str(&format!("\n    at {} ({}:{}:{})", name, f.code.filename, pos.line, pos.column));
        }
        s
    }

    // ------------------------------------------------------------- roots

    pub fn root(&mut self, value: Value) -> Root {
        let mut t = self.roots.borrow_mut();
        t.next += 1;
        let id = t.next;
        t.values.insert(id, value.clone());
        Root { id, table: self.roots.clone(), value }
    }

    pub fn root_object(&mut self, id: ObjectId) -> Root {
        self.root(Value::Object(id))
    }

    // ------------------------------------------------------------ console

    pub fn set_console_sink(&mut self, sink: ConsoleSink) {
        self.console_sink = Some(sink);
    }

    pub fn console_emit(&mut self, level: &str, text: String) {
        match &mut self.console_sink {
            Some(s) => s(level, text),
            None => eprintln!("[console.{}] {}", level, text),
        }
    }

    // --------------------------------------------------------------- GC

    /// Run a full garbage collection now.
    pub fn gc(&mut self) -> usize {
        let mut roots: Vec<ObjectId> = Vec::with_capacity(1024);
        let i = &self.intrinsics;
        roots.extend([
            i.object_proto, i.function_proto, i.array_proto, i.string_proto, i.number_proto, i.boolean_proto, i.symbol_proto,
            i.bigint_proto, i.error_proto, i.iterator_proto, i.array_iterator_proto, i.generator_proto, i.async_generator_proto,
            i.promise_proto, i.promise_ctor, i.object_ctor, i.array_ctor, i.function_ctor, i.eval_fn, self.global,
        ]);
        roots.extend(i.others.values().copied());
        for (v, _) in self.global_lex.values() {
            if let Value::Object(o) = v {
                roots.push(*o);
            }
        }
        let mut tmp = Vec::new();
        for f in &self.frames {
            trace_frame(f, &mut tmp);
        }
        for j in &self.jobs {
            match j {
                Job::Reaction { reaction, argument } => {
                    trace_reaction(reaction, &mut tmp);
                    tmp.push(argument.clone());
                }
                Job::ResolveThenable { promise, thenable, then } => {
                    tmp.push(Value::Object(*promise));
                    tmp.push(thenable.clone());
                    tmp.push(then.clone());
                }
                Job::Callback { func, this, args } => {
                    tmp.push(func.clone());
                    tmp.push(this.clone());
                    tmp.extend(args.iter().cloned());
                }
            }
        }
        tmp.extend(self.roots.borrow().values.values().cloned());
        tmp.extend(self.extra_roots.iter().cloned());
        for roots in &self.job_roots {
            tmp.extend(roots.iter().cloned());
        }
        tmp.extend(self.new_target_stack.iter().cloned());
        tmp.extend(self.unhandled_rejections.iter().cloned());
        tmp.extend(self.template_cache.values().map(|o| Value::Object(*o)));
        self.modules.trace(&mut tmp);
        if let Some(c) = self.current_callee {
            tmp.push(Value::Object(c));
        }
        for v in tmp {
            if let Value::Object(o) = v {
                roots.push(o);
            }
        }
        self.heap.collect(&roots)
    }

    /// Called by the VM at safe points.
    /// Set (or clear) the execution deadline for subsequent JavaScript.
    pub fn set_deadline(&mut self, deadline: Option<std::time::Instant>) {
        self.deadline = deadline;
        self.terminating = false;
        crate::regex::set_deadline(deadline);
    }

    /// Did the last run hit the deadline?
    pub fn timed_out(&self) -> bool {
        self.terminating
    }

    pub(crate) fn maybe_gc(&mut self) {
        if self.gc_enabled && self.native_depth == 0 && self.heap.allocated_since_gc > self.gc_threshold && std::env::var_os("RH_NO_GC").is_none() {
            let freed = self.gc();
            // Adapt threshold to the live heap size.
            self.gc_threshold = (self.heap.live_count.max(50_000)).max(freed / 2);
        }
    }

    // --------------------------------------------------------- evaluation

    /// Compile and run a script in the global scope.
    pub fn eval(&mut self, source: &str, filename: &str) -> Result<Value, Value> {
        let code = self.compile(source, filename, false)?;
        self.run_script(code)
    }

    /// Compile, link (through the installed module loader) and evaluate a
    /// module; see [`Realm::run_module`]. Returns the evaluation promise.
    pub fn eval_module(&mut self, source: &str, filename: &str) -> Result<Value, Value> {
        self.run_module(source, filename)
    }

    pub fn compile(&mut self, source: &str, filename: &str, is_module: bool) -> Result<Rc<FunctionCode>, Value> {
        let program = match crate::parser::parse_program(source, is_module) {
            Ok(p) => p,
            Err(e) => {
                let msg = format!("{} ({}:{}:{})", e.message, filename, e.line, e.column);
                return Err(self.new_error("SyntaxError", &msg));
            }
        };
        match crate::compiler::compile_program(&program, filename, source) {
            Ok(c) => Ok(c),
            Err(e) => Err(self.new_error("SyntaxError", &e.message)),
        }
    }

    /// Enqueue a host callback to run on the job queue.
    pub fn enqueue_callback(&mut self, func: Value, this: Value, args: Vec<Value>) {
        self.jobs.push_back(Job::Callback { func, this, args });
    }

    /// Drain the job (microtask) queue. Errors thrown by jobs are reported
    /// to the console and do not stop the drain.
    pub fn run_jobs(&mut self) {
        while let Some(job) = self.jobs.pop_front() {
            // Keep the running job's values alive: it is no longer in the
            // queue, and the code it runs may trigger a collection.
            let mut job_roots = Vec::new();
            match &job {
                Job::Reaction { reaction, argument } => {
                    crate::heap::trace_reaction(reaction, &mut job_roots);
                    job_roots.push(argument.clone());
                }
                Job::ResolveThenable { promise, thenable, then } => {
                    job_roots.push(Value::Object(*promise));
                    job_roots.push(thenable.clone());
                    job_roots.push(then.clone());
                }
                Job::Callback { func, this, args } => {
                    job_roots.push(func.clone());
                    job_roots.push(this.clone());
                    job_roots.extend(args.iter().cloned());
                }
            }
            self.job_roots.push(job_roots);
            let r = match job {
                Job::Reaction { reaction, argument } => self.run_reaction(reaction, argument),
                Job::ResolveThenable { promise, thenable, then } => self.run_resolve_thenable(promise, thenable, then),
                Job::Callback { func, this, args } => self.call(func, this, &args).map(|_| ()),
            };
            self.job_roots.pop();
            if let Err(e) = r {
                let msg = self.error_to_string(&e);
                self.console_emit("error", format!("Uncaught (in job) {}", msg));
            }
        }
    }

    pub fn has_pending_jobs(&self) -> bool {
        !self.jobs.is_empty()
    }

    /// Best-effort textual form of a thrown value (for logging).
    pub fn error_to_string(&mut self, v: &Value) -> String {
        if let Value::Object(o) = v {
            if let Some(p) = self.obj(*o).props.get(&PropertyKey::str("stack")) {
                if let Value::String(s) = &p.value {
                    return s.to_rust_string();
                }
            }
        }
        match self.to_string(v.clone()) {
            Ok(s) => s.to_rust_string(),
            Err(_) => "<error>".into(),
        }
    }

    pub fn is_callable(&self, v: &Value) -> bool {
        match v {
            Value::Object(o) => match &self.obj(*o).kind {
                ObjectKind::Proxy { target, .. } => self.is_callable(&Value::Object(*target)),
                k => k.is_callable(),
            },
            _ => false,
        }
    }

    pub fn is_constructor(&self, v: &Value) -> bool {
        match v {
            Value::Object(o) => match &self.obj(*o).kind {
                ObjectKind::Function(c) => !c.code.flags.is_arrow && !c.code.flags.is_method && !c.code.flags.is_generator && !c.code.flags.is_async,
                ObjectKind::Native(n) => n.is_constructor,
                ObjectKind::Bound(b) => self.is_constructor(&Value::Object(b.target)),
                ObjectKind::Proxy { target, .. } => self.is_constructor(&Value::Object(*target)),
                _ => false,
            },
            _ => false,
        }
    }

    /// `new.target` inside a native constructor (Undefined when called).
    pub fn new_target(&self) -> Value {
        self.new_target_stack.last().cloned().unwrap_or(Value::Undefined)
    }

    pub fn is_construct_call(&self) -> bool {
        !self.new_target().is_undefined()
    }

    /// Prototype for an object created by a native constructor, honouring
    /// subclassing via `new.target.prototype`; falls back to `default`.
    pub fn proto_from_new_target(&mut self, default: ObjectId) -> ObjectId {
        if let Value::Object(nt) = self.new_target() {
            if let Ok(Value::Object(p)) = self.get(Value::Object(nt), PropertyKey::str("prototype")) {
                return p;
            }
        }
        default
    }

    // ------------------------------------------------------------ promises

    /// Create a pending promise plus native resolve/reject functions.
    pub fn new_promise(&mut self) -> (ObjectId, ObjectId, ObjectId) {
        crate::builtins::promise::new_promise_capability(self)
    }

    pub fn new_promise_object(&mut self) -> ObjectId {
        let proto = self.intrinsics.promise_proto;
        self.alloc(Object::new(Some(proto), ObjectKind::Promise(Box::new(PromiseData { state: PromiseState::Pending, value: Value::Undefined, fulfill_reactions: vec![], reject_reactions: vec![], handled: false }))))
    }

    pub fn resolve_promise(&mut self, promise: ObjectId, value: Value) {
        crate::builtins::promise::resolve_promise(self, promise, value);
    }

    pub fn reject_promise(&mut self, promise: ObjectId, reason: Value) {
        crate::builtins::promise::reject_promise(self, promise, reason);
    }

    pub fn take_unhandled_rejections(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.unhandled_rejections)
    }
}
