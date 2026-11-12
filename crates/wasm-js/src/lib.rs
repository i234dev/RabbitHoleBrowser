//! The WebAssembly JavaScript API: the `WebAssembly` namespace (`validate`,
//! `compile`, `instantiate`, `compileStreaming`, `instantiateStreaming`,
//! `Module`, `Instance`, `Memory`, `Table`, `Global`, `Tag`, `Exception`,
//! `CompileError`, `LinkError`, `RuntimeError`) over the engine of
//! `rabbithole-wasm`.
//!
//! Each realm owns one wasm [`Store`]; JavaScript wrappers of wasm objects
//! are cached per object (identity is preserved) and, like the store, live
//! as long as the realm. Values cross with the JS API conversions
//! (ToWebAssemblyValue / ToJSValue; i64 ↔ BigInt). `Memory.buffer` is an
//! `ArrayBuffer` aliasing linear memory; growing detaches it (the old
//! buffer becomes empty) and the next `buffer` read returns a new one.
//! Long-running wasm polls the realm watchdog at loop back-edges and calls.

use rabbithole_js::builtins::typed_array::to_bigint;
use rabbithole_js::heap::Object;
use rabbithole_js::{JsStr, ObjectId, ObjectKind, PropertyKey, Realm, Root, SymbolId, Value, CONFIGURABLE, ENUMERABLE, V, WRITABLE};
use rabbithole_wasm::module::ImportDesc;
use rabbithole_wasm::runtime::*;
use rabbithole_wasm::types::*;
use rabbithole_wasm::{gc, CompiledModule, Error as WasmError};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

type JsResult<T> = Result<T, Value>;

// ------------------------------------------------------------------ state

struct Protos {
    module: ObjectId,
    instance: ObjectId,
    memory: ObjectId,
    table: ObjectId,
    global: ObjectId,
    tag: ObjectId,
    exception: ObjectId,
}

struct State {
    store: Rc<Store>,
    /// JavaScript functions imported by modules (host functions).
    imports: Vec<Root>,
    func_objs: HashMap<u32, Root>,
    mem_objs: HashMap<usize, Root>,
    table_objs: HashMap<usize, Root>,
    global_objs: HashMap<usize, Root>,
    tag_objs: HashMap<usize, Root>,
    gc_objs: HashMap<u64, Root>,
    /// externref values: index + 1 is the reference bits.
    /// (value, object id) of each externref slot; None: free.
    externs: Vec<Option<(Root, Option<ObjectId>)>>,
    extern_free: Vec<u32>,
    extern_ids: HashMap<ObjectId, u32>,
    extern_live: usize,
    /// Live count that schedules the next sweep.
    extern_sweep_at: usize,
    extern_sweep_pending: bool,
    protos: Protos,
    errors: Vec<Root>,
    /// Private keys: instance → exports object, memory → buffer.
    key_exports: SymbolId,
    key_buffer: SymbolId,
}

struct JsImport(usize);
struct FuncRef(u32);
struct JsThrown(Root);
struct WModule(Rc<CompiledModule>);
#[allow(dead_code)]
struct WInstance(Rc<Instance>);
struct WMemory(Rc<MemInst>);
struct WTable(Rc<TableInst>);
struct WGlobal(Rc<GlobalInst>);
struct WTag(Rc<TagInst>);
struct WException(u32);
struct WGcObject(u64);

fn state_id(realm: &Realm) -> ObjectId {
    *realm.intrinsics.others.get("WasmState").expect("WebAssembly installed")
}

fn st(realm: &mut Realm) -> &mut State {
    let id = state_id(realm);
    realm.host_data_mut::<State>(id).expect("wasm state")
}

fn store(realm: &mut Realm) -> Rc<Store> {
    st(realm).store.clone()
}

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Undefined)
}

fn host<T: 'static>(realm: &Realm, v: &Value) -> Option<ObjectId> {
    match v.v() {
        V::Object(id) => realm.host_data::<T>(id).map(|_| id),
        _ => None,
    }
}

fn new_host_object(realm: &mut Realm, proto: ObjectId, data: Box<dyn std::any::Any>) -> ObjectId {
    let o = realm.new_object_with_proto(Some(proto));
    realm.set_host_data(o, data);
    o
}

/// Throw one of `WebAssembly.CompileError` (0), `LinkError` (1),
/// `RuntimeError` (2).
fn wasm_error(realm: &mut Realm, kind: usize, msg: &str) -> Value {
    let ctor = st(realm).errors.get(kind).map(|r| r.value());
    if let Some(c) = ctor {
        let m = realm.new_string(msg);
        if let Ok(e) = realm.construct(c, &[m], Value::Undefined) {
            return e;
        }
    }
    realm.new_error("Error", msg)
}

// ------------------------------------------------------------------ host

struct JsHost<'r> {
    realm: &'r mut Realm,
    timeout: Option<Value>,
}

impl Host for JsHost<'_> {
    fn call_host(&mut self, _store: &Store, func: &Rc<FuncInst>, args: &[Val]) -> Result<Vec<Val>, Trap> {
        let FuncKind::Host { data } = &func.kind else { return Err(Trap::Other("not a host function".into())) };
        let Some(imp) = data.downcast_ref::<JsImport>() else { return Err(Trap::Other("unknown host function".into())) };
        let callee = st(self.realm).imports[imp.0].value();
        let realm = &mut *self.realm;
        let mut jsargs = Vec::with_capacity(args.len());
        for (v, t) in args.iter().zip(func.ft.params.iter()) {
            jsargs.push(to_js(realm, *v, *t));
        }
        let thrown = |realm: &mut Realm, e: Value| -> Trap {
            if let Some(id) = host::<WException>(realm, &e) {
                if let Some(x) = realm.host_data::<WException>(id) {
                    return Trap::Exception(x.0);
                }
            }
            Trap::Host(Rc::new(JsThrown(realm.root(e))))
        };
        let ret = match realm.call(callee, Value::Undefined, &jsargs) {
            Ok(r) => r,
            Err(e) => return Err(thrown(realm, e)),
        };
        let results = &func.ft.results;
        let conv = |realm: &mut Realm| -> JsResult<Vec<Val>> {
            match results.len() {
                0 => Ok(vec![]),
                1 => Ok(vec![to_wasm(realm, ret.clone(), results[0])?]),
                n => {
                    let vals = realm.iterate_to_vec(ret.clone())?;
                    if vals.len() != n {
                        return Err(realm.throw_type_error("wrong number of results returned to WebAssembly"));
                    }
                    let mut out = Vec::with_capacity(n);
                    for (v, t) in vals.into_iter().zip(results.iter()) {
                        out.push(to_wasm(realm, v, *t)?);
                    }
                    Ok(out)
                }
            }
        };
        conv(realm).map_err(|e| thrown(realm, e))
    }

    fn check_interrupt(&mut self) -> bool {
        match self.realm.check_watchdog() {
            Ok(()) => false,
            Err(e) => {
                self.timeout = Some(e);
                true
            }
        }
    }
}

fn trap_to_js(realm: &mut Realm, t: Trap, timeout: Option<Value>) -> Value {
    match t {
        Trap::Host(rc) => match rc.downcast_ref::<JsThrown>() {
            Some(j) => j.0.value(),
            None => wasm_error(realm, 2, "host error"),
        },
        Trap::Interrupted => timeout.unwrap_or_else(|| realm.new_error("RangeError", "Script execution timed out")),
        Trap::Exception(e) => Value::Object(exception_obj(realm, e)),
        Trap::StackExhausted => realm.new_error("RangeError", "Maximum call stack size exceeded"),
        other => wasm_error(realm, 2, &other.message()),
    }
}

fn call_wasm(realm: &mut Realm, f: &Rc<FuncInst>, args: &[Val]) -> JsResult<Vec<Val>> {
    let store = store(realm);
    let mut h = JsHost { realm, timeout: None };
    let r = store.call(&mut h, f, args);
    let timeout = h.timeout.take();
    r.map_err(|t| trap_to_js(realm, t, timeout))
}

// ------------------------------------------------------------------ values

fn val_type_name(s: &str) -> Option<ValType> {
    Some(match s {
        "i32" => ValType::I32,
        "i64" => ValType::I64,
        "f32" => ValType::F32,
        "f64" => ValType::F64,
        "v128" => ValType::V128,
        "externref" => ValType::EXTERNREF,
        "anyfunc" | "funcref" => ValType::FUNCREF,
        "exnref" => ValType::Ref(RefType::EXNREF),
        "anyref" => ValType::Ref(RefType::ANYREF),
        _ => return None,
    })
}

/// ToJSValue.
fn to_js(realm: &mut Realm, v: Val, t: ValType) -> Value {
    match v {
        Val::I32(x) => Value::Number(x as f64),
        Val::I64(x) => realm.new_bigint(x as i128),
        Val::F32(b) => Value::Number(f32::from_bits(b) as f64),
        Val::F64(b) => Value::Number(f64::from_bits(b)),
        Val::V128(_) => Value::Undefined,
        Val::Ref(0) => Value::Null,
        Val::Ref(bits) => {
            let heap = match t {
                ValType::Ref(r) => r.heap,
                _ => HeapType::Extern,
            };
            let is_func = match heap {
                HeapType::Func | HeapType::NoFunc => true,
                HeapType::Concrete(id) | HeapType::Exact(id) => matches!(canonical_type(id).comp, CompType::Func(_)),
                _ => false,
            };
            if is_func {
                let s = store(realm);
                let f = s.func((bits - 1) as u32);
                Value::Object(func_obj(realm, &f))
            } else if heap == HeapType::Exn {
                Value::Object(exception_obj(realm, (bits - 1) as u32))
            } else if matches!(heap, HeapType::Extern | HeapType::NoExtern) {
                if bits & gc::EXTERN_ANY_BIT != 0 {
                    return any_to_js(realm, bits & !gc::EXTERN_ANY_BIT);
                }
                extern_value(realm, (bits - 1) as usize)
            } else {
                any_to_js(realm, bits)
            }
        }
    }
}

/// ToWebAssemblyValue.
fn to_wasm(realm: &mut Realm, v: Value, t: ValType) -> JsResult<Val> {
    Ok(match t {
        ValType::I32 => Val::I32(realm.to_int32(v)?),
        ValType::I64 => Val::I64(to_bigint(realm, v)? as i64),
        ValType::F32 => Val::F32((realm.to_number(v)? as f32).to_bits()),
        ValType::F64 => Val::F64(realm.to_number(v)?.to_bits()),
        ValType::V128 => return Err(realm.throw_type_error("v128 values cannot cross the JavaScript boundary")),
        ValType::Ref(r) => {
            if v.is_null() {
                if !r.nullable {
                    return Err(realm.throw_type_error("null is not allowed for a non-nullable reference"));
                }
                return Ok(Val::Ref(0));
            }
            let is_func = match r.heap {
                HeapType::Func | HeapType::NoFunc => true,
                HeapType::Concrete(id) | HeapType::Exact(id) => matches!(canonical_type(id).comp, CompType::Func(_)),
                _ => false,
            };
            if is_func {
                let addr = match v.v() {
                    V::Object(id) => realm.native_data::<FuncRef>(id).map(|f| f.0),
                    _ => None,
                };
                let Some(addr) = addr else { return Err(realm.throw_type_error("value is not an exported WebAssembly function")) };
                if let Some(want) = r.heap.index() {
                    let f = store(realm).func(addr);
                    let ok = if matches!(r.heap, HeapType::Exact(_)) { f.ty == want } else { canonical_subtype(f.ty, want) };
                    if !ok {
                        return Err(realm.throw_type_error("function type mismatch"));
                    }
                }
                Val::Ref(addr as u64 + 1)
            } else if r.heap == HeapType::Exn || r.heap == HeapType::NoExn {
                return Err(realm.throw_type_error("exnref values cannot cross the JavaScript boundary"));
            } else if matches!(r.heap, HeapType::Extern | HeapType::NoExtern) {
                if r.heap == HeapType::NoExtern {
                    return Err(realm.throw_type_error("only null is a valid nullexternref"));
                }
                match gc_object(realm, &v) {
                    Some(bits) => Val::Ref(gc::extern_convert_any(bits)),
                    None => Val::Ref(extern_ref(realm, v) as u64),
                }
            } else {
                // The any hierarchy: GC objects, i31 numbers or host values.
                let bits = match gc_object(realm, &v) {
                    Some(b) => b,
                    None => match v.v() {
                        V::Number(n) if n.fract() == 0.0 && (-1073741824.0..=1073741823.0).contains(&n) && !(n == 0.0 && n.is_sign_negative()) => gc::i31_bits(n as i32 as u32),
                        _ => gc::any_convert_extern(extern_ref(realm, v.clone()) as u64),
                    },
                };
                let s = store(realm);
                if !gc::ref_test(&s, bits, canon_heap_rt(r.heap), false) {
                    return Err(realm.throw_type_error("type incompatibility when transforming from/to JS"));
                }
                Val::Ref(bits)
            }
        }
    })
}

fn canon_heap_rt(h: HeapType) -> HeapType {
    h
}

/// The reference bits of a wrapper of a wasm GC object.
fn gc_object(realm: &mut Realm, v: &Value) -> Option<u64> {
    host::<WGcObject>(realm, v).map(|id| realm.host_data::<WGcObject>(id).unwrap().0)
}

/// ToJSValue for the `any` hierarchy.
fn any_to_js(realm: &mut Realm, bits: u64) -> Value {
    if bits & 1 == 1 {
        let v = ((bits >> 1) as u32) << 1;
        return Value::Number(((v as i32) >> 1) as f64);
    }
    if bits & 3 == 2 {
        return extern_value(realm, ((bits >> 2) - 1) as usize);
    }
    if let Some(r) = st(realm).gc_objs.get(&bits) {
        return r.value();
    }
    let o = realm.new_object_with_proto(None);
    realm.set_host_data(o, Box::new(WGcObject(bits)));
    realm.obj_mut(o).extensible = false;
    let r = realm.root_object(o);
    st(realm).gc_objs.insert(bits, r);
    Value::Object(o)
}

fn extern_value(realm: &mut Realm, i: usize) -> Value {
    match st(realm).externs.get(i) {
        Some(Some((r, _))) => r.value(),
        _ => Value::Undefined,
    }
}

/// The externref bits of a JavaScript value (object identity preserved).
/// Slots no longer referenced from wasm are reclaimed by [`sweep_externs`].
fn extern_ref(realm: &mut Realm, v: Value) -> u32 {
    let oid = match v.v() {
        V::Object(id) => Some(id),
        _ => None,
    };
    if let Some(id) = oid {
        if let Some(&i) = st(realm).extern_ids.get(&id) {
            return i + 1;
        }
    }
    let r = realm.root(v);
    let s = st(realm);
    let i = match s.extern_free.pop() {
        Some(i) => {
            s.externs[i as usize] = Some((r, oid));
            i
        }
        None => {
            s.externs.push(Some((r, oid)));
            (s.externs.len() - 1) as u32
        }
    };
    if let Some(id) = oid {
        s.extern_ids.insert(id, i);
    }
    s.extern_live += 1;
    if s.extern_live > s.extern_sweep_at && !s.extern_sweep_pending {
        s.extern_sweep_pending = true;
        let job = realm.new_native_function("", 0, sweep_job);
        realm.enqueue_callback(Value::Object(job), Value::Undefined, vec![]);
    }
    i + 1
}

fn sweep_job(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    sweep_externs(realm);
    Ok(Value::Undefined)
}

/// Free externref slots that nothing in wasm refers to any more. Runs as a
/// job (empty JavaScript stack, so no wasm frame or conversion is in
/// flight); the scan is conservative (tables, globals, GC objects and
/// exception payloads; any word that could encode a slot keeps it).
fn sweep_externs(realm: &mut Realm) {
    let store = store(realm);
    st(realm).extern_sweep_pending = false;
    if store.depth_now() != 0 {
        return;
    }
    let n = st(realm).externs.len();
    let mut marks = vec![false; n];
    let mut mark = |bits: u64| {
        for v in [bits, bits & !gc::EXTERN_ANY_BIT] {
            if v >= 1 && ((v - 1) as usize) < n {
                marks[(v - 1) as usize] = true;
            }
            if v & 3 == 2 && v >> 2 >= 1 && (((v >> 2) - 1) as usize) < n {
                marks[((v >> 2) - 1) as usize] = true;
            }
        }
    };
    let mut tables: Vec<Rc<TableInst>> = Vec::new();
    let mut globals: Vec<Rc<GlobalInst>> = Vec::new();
    for inst in store.instances.borrow().iter() {
        tables.extend(inst.tables.iter().cloned());
        globals.extend(inst.globals.iter().cloned());
    }
    let ids: Vec<ObjectId> = {
        let s = st(realm);
        s.table_objs.values().chain(s.global_objs.values()).filter_map(|r| r.object()).collect()
    };
    for id in ids {
        if let Some(t) = realm.host_data::<WTable>(id) {
            tables.push(t.0.clone());
        }
        if let Some(g) = realm.host_data::<WGlobal>(id) {
            globals.push(g.0.clone());
        }
    }
    for t in &tables {
        for &e in t.elems.borrow().iter() {
            mark(e);
        }
    }
    for g in &globals {
        mark(g.lo.get());
        mark(g.hi.get());
    }
    for o in store.gc.borrow().iter() {
        for &w in &o.data {
            mark(w);
        }
    }
    for e in store.exns.borrow().iter() {
        for &w in &e.payload {
            mark(w);
        }
    }
    let s = st(realm);
    let before = s.extern_live;
    for (i, m) in marks.iter().enumerate() {
        if !m {
            if let Some((_, oid)) = s.externs[i].take() {
                if let Some(id) = oid {
                    s.extern_ids.remove(&id);
                }
                s.extern_free.push(i as u32);
                s.extern_live -= 1;
            }
        }
    }
    s.extern_sweep_at = (s.extern_live * 2).max(4096);
    if stats::enabled() {
        eprintln!("[wasm] externref sweep: {before} -> {} live slots", s.extern_live);
    }
}

fn default_value(realm: &mut Realm, t: ValType) -> JsResult<Val> {
    match t {
        ValType::Ref(r) if r.heap == HeapType::Extern => to_wasm(realm, Value::Undefined, t),
        _ => Ok(Val::default_of(t)),
    }
}

// ------------------------------------------------------------------ wrappers

fn func_obj(realm: &mut Realm, f: &Rc<FuncInst>) -> ObjectId {
    if let Some(r) = st(realm).func_objs.get(&f.addr) {
        return r.object().unwrap();
    }
    let name = match &f.kind {
        FuncKind::Wasm { code, .. } => code.func_index.to_string(),
        FuncKind::Host { .. } => f.addr.to_string(),
    };
    let id = realm.new_native_function_full(&name, f.ft.params.len() as u32, call_export, Some(Rc::new(FuncRef(f.addr))), vec![], false);
    let r = realm.root_object(id);
    st(realm).func_objs.insert(f.addr, r);
    id
}

fn cached(realm: &mut Realm, which: u8, key: usize) -> Option<ObjectId> {
    let s = st(realm);
    let m = match which {
        0 => &s.mem_objs,
        1 => &s.table_objs,
        2 => &s.global_objs,
        _ => &s.tag_objs,
    };
    m.get(&key).and_then(|r| r.object())
}

fn cache(realm: &mut Realm, which: u8, key: usize, id: ObjectId) {
    let r = realm.root_object(id);
    let s = st(realm);
    let m = match which {
        0 => &mut s.mem_objs,
        1 => &mut s.table_objs,
        2 => &mut s.global_objs,
        _ => &mut s.tag_objs,
    };
    m.insert(key, r);
}

fn memory_obj(realm: &mut Realm, m: &Rc<MemInst>) -> ObjectId {
    let key = Rc::as_ptr(m) as usize;
    if let Some(id) = cached(realm, 0, key) {
        return id;
    }
    let p = st(realm).protos.memory;
    let id = new_host_object(realm, p, Box::new(WMemory(m.clone())));
    cache(realm, 0, key, id);
    id
}

fn table_obj(realm: &mut Realm, t: &Rc<TableInst>) -> ObjectId {
    let key = Rc::as_ptr(t) as usize;
    if let Some(id) = cached(realm, 1, key) {
        return id;
    }
    let p = st(realm).protos.table;
    let id = new_host_object(realm, p, Box::new(WTable(t.clone())));
    cache(realm, 1, key, id);
    id
}

fn global_obj(realm: &mut Realm, g: &Rc<GlobalInst>) -> ObjectId {
    let key = Rc::as_ptr(g) as usize;
    if let Some(id) = cached(realm, 2, key) {
        return id;
    }
    let p = st(realm).protos.global;
    let id = new_host_object(realm, p, Box::new(WGlobal(g.clone())));
    cache(realm, 2, key, id);
    id
}

fn tag_obj(realm: &mut Realm, t: &Rc<TagInst>) -> ObjectId {
    let key = Rc::as_ptr(t) as usize;
    if let Some(id) = cached(realm, 3, key) {
        return id;
    }
    let p = st(realm).protos.tag;
    let id = new_host_object(realm, p, Box::new(WTag(t.clone())));
    cache(realm, 3, key, id);
    id
}

fn exception_obj(realm: &mut Realm, exn: u32) -> ObjectId {
    let p = st(realm).protos.exception;
    new_host_object(realm, p, Box::new(WException(exn)))
}

fn extern_to_js(realm: &mut Realm, e: &Extern) -> Value {
    Value::Object(match e {
        Extern::Func(f) => func_obj(realm, f),
        Extern::Memory(m) => memory_obj(realm, m),
        Extern::Table(t) => table_obj(realm, t),
        Extern::Global(g) => global_obj(realm, g),
        Extern::Tag(t) => tag_obj(realm, t),
    })
}

fn module_obj(realm: &mut Realm, cm: Rc<CompiledModule>, proto: Option<ObjectId>) -> ObjectId {
    let p = proto.unwrap_or(st(realm).protos.module);
    new_host_object(realm, p, Box::new(WModule(cm)))
}

fn instance_obj(realm: &mut Realm, inst: Rc<Instance>, proto: Option<ObjectId>) -> ObjectId {
    // The exports object: null prototype, frozen.
    let exports = realm.new_object_with_proto(None);
    for (name, e) in inst.exports.clone() {
        let v = extern_to_js(realm, &e);
        realm.define_property(exports, PropertyKey::from(JsStr::new(&name)), v, ENUMERABLE);
    }
    realm.obj_mut(exports).extensible = false;
    let p = proto.unwrap_or(st(realm).protos.instance);
    let id = new_host_object(realm, p, Box::new(WInstance(inst)));
    let key = st(realm).key_exports;
    realm.define_property(id, PropertyKey::Symbol(key), Value::Object(exports), 0);
    id
}

// ------------------------------------------------------------------ natives: namespace

fn buffer_source(realm: &mut Realm, v: &Value) -> JsResult<Vec<u8>> {
    let id = match v.v() {
        V::Object(id) => id,
        _ => return Err(realm.throw_type_error("argument must be a BufferSource")),
    };
    let (buf, off, len) = match &realm.obj(id).kind {
        ObjectKind::ArrayBuffer(b) => return Ok(b.borrow().clone()),
        ObjectKind::TypedArray(t) | ObjectKind::DataView(t) => (t.buffer, t.byte_offset, t.length * t.kind.element_size()),
        _ => return Err(realm.throw_type_error("argument must be a BufferSource")),
    };
    match &realm.obj(buf).kind {
        ObjectKind::ArrayBuffer(b) => {
            let b = b.borrow();
            let end = (off + len).min(b.len());
            Ok(b[off.min(end)..end].to_vec())
        }
        _ => Err(realm.throw_type_error("argument must be a BufferSource")),
    }
}

/// `RH_WASM_STATS=1`: report time spent compiling and running wasm
/// (calls from JavaScript, including JavaScript imports they call).
mod stats {
    use std::cell::Cell;
    use std::time::Duration;
    thread_local! {
        static ON: bool = std::env::var_os("RH_WASM_STATS").is_some();
        static CALLS: Cell<(u64, Duration, Duration)> = const { Cell::new((0, Duration::ZERO, Duration::ZERO)) };
    }
    pub fn enabled() -> bool {
        ON.with(|o| *o)
    }
    pub fn call(d: Duration) {
        CALLS.with(|c| {
            let (n, total, reported) = c.get();
            let (n, total) = (n + 1, total + d);
            let mut rep = reported;
            if total - reported >= Duration::from_millis(500) || d >= Duration::from_millis(100) {
                eprintln!("[wasm] {n} calls from JS, {total:?} in wasm (last call {d:?})");
                rep = total;
            }
            c.set((n, total, rep));
        });
    }
    pub fn compiled(bytes: usize, d: Duration) {
        eprintln!("[wasm] compiled {bytes} bytes in {d:?}");
    }
}

fn compile_bytes(realm: &mut Realm, bytes: &[u8]) -> JsResult<Rc<CompiledModule>> {
    let t0 = std::time::Instant::now();
    let r = CompiledModule::new(bytes);
    if stats::enabled() {
        stats::compiled(bytes.len(), t0.elapsed());
    }
    match r {
        Ok(m) => Ok(Rc::new(m)),
        Err(e) => {
            let msg = match e {
                WasmError::Malformed(m) | WasmError::Invalid(m) => m,
            };
            Err(wasm_error(realm, 0, &format!("WebAssembly.Module(): {msg}")))
        }
    }
}

fn wa_validate(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let bytes = buffer_source(realm, &arg(args, 0))?;
    Ok(Value::Bool(CompiledModule::new(&bytes).is_ok()))
}

/// Run `job` from the job queue and settle `promise` with its result.
fn defer(realm: &mut Realm, data: Rc<dyn std::any::Any>, captures: Vec<Value>, f: rabbithole_js::NativeFn) -> Value {
    let (promise, resolve, reject) = realm.new_promise();
    let mut caps = vec![Value::Object(resolve), Value::Object(reject)];
    caps.extend(captures);
    let job = realm.new_native_function_full("", 0, f, Some(data), caps, false);
    realm.enqueue_callback(Value::Object(job), Value::Undefined, vec![]);
    Value::Object(promise)
}

fn settle(realm: &mut Realm, r: JsResult<Value>) -> JsResult<Value> {
    let caps = realm.current_native().map(|c| realm.native_captures(c)).unwrap_or_default();
    let (res, rej) = (caps[0].clone(), caps[1].clone());
    match r {
        Ok(v) => realm.call(res, Value::Undefined, &[v]),
        Err(e) => {
            if realm.timed_out() {
                return Err(e);
            }
            realm.call(rej, Value::Undefined, &[e])
        }
    }
}

struct Bytes(RefCell<Vec<u8>>);

fn wa_compile(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let bytes = match buffer_source(realm, &arg(args, 0)) {
        Ok(b) => b,
        Err(e) => {
            let (p, _, rej) = realm.new_promise();
            realm.call(Value::Object(rej), Value::Undefined, &[e])?;
            return Ok(Value::Object(p));
        }
    };
    Ok(defer(realm, Rc::new(Bytes(RefCell::new(bytes))), vec![], compile_job))
}

fn compile_job(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    let me = realm.current_native().unwrap();
    let data = realm.native_data::<Bytes>(me).unwrap();
    let bytes = std::mem::take(&mut *data.0.borrow_mut());
    let r = compile_bytes(realm, &bytes).map(|m| Value::Object(module_obj(realm, m, None)));
    settle(realm, r)
}

fn wa_instantiate(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let src = arg(args, 0);
    let imports = arg(args, 1);
    if let Some(id) = host::<WModule>(realm, &src) {
        return Ok(defer(realm, Rc::new(()), vec![Value::Object(id), imports], instantiate_module_job));
    }
    let bytes = match buffer_source(realm, &src) {
        Ok(b) => b,
        Err(e) => {
            let (p, _, rej) = realm.new_promise();
            realm.call(Value::Object(rej), Value::Undefined, &[e])?;
            return Ok(Value::Object(p));
        }
    };
    Ok(defer(realm, Rc::new(Bytes(RefCell::new(bytes))), vec![imports], instantiate_bytes_job))
}

fn instantiate_module_job(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    let me = realm.current_native().unwrap();
    let caps = realm.native_captures(me);
    let mid = caps[2].as_object().unwrap();
    let cm = realm.host_data::<WModule>(mid).unwrap().0.clone();
    let r = instantiate(realm, &cm, caps[3].clone()).map(|i| Value::Object(instance_obj(realm, i, None)));
    settle(realm, r)
}

fn instantiate_bytes_job(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    let me = realm.current_native().unwrap();
    let caps = realm.native_captures(me);
    let data = realm.native_data::<Bytes>(me).unwrap();
    let bytes = std::mem::take(&mut *data.0.borrow_mut());
    let r = (|| {
        let cm = compile_bytes(realm, &bytes)?;
        let inst = instantiate(realm, &cm, caps[2].clone())?;
        let mo = module_obj(realm, cm, None);
        let io = instance_obj(realm, inst, None);
        let result = realm.new_object();
        realm.define_property(result, "module", Value::Object(mo), rabbithole_js::ALL);
        realm.define_property(result, "instance", Value::Object(io), rabbithole_js::ALL);
        Ok(Value::Object(result))
    })();
    settle(realm, r)
}

/// Read the imports of `cm` from `imports` and instantiate.
fn instantiate(realm: &mut Realm, cm: &Rc<CompiledModule>, imports: Value) -> JsResult<Rc<Instance>> {
    let m = &cm.module;
    if !m.imports.is_empty() && !imports.is_object() {
        return Err(realm.throw_type_error("WebAssembly.Instance(): imports argument must be present and must be an object"));
    }
    let s = store(realm);
    let mut externs = Vec::with_capacity(m.imports.len());
    for imp in &m.imports {
        let o = realm.get_str(imports.clone(), &imp.module)?;
        if !o.is_object() {
            return Err(realm.throw_type_error(&format!("WebAssembly.Instance(): Import #0 \"{}\": module is not an object or function", imp.module)));
        }
        let v = realm.get_str(o, &imp.name)?;
        let link = |realm: &mut Realm, what: &str| wasm_error(realm, 1, &format!("WebAssembly.Instance(): Import \"{}\" \"{}\": {what}", imp.module, imp.name));
        let e = match &imp.desc {
            ImportDesc::Func(t) => {
                if !realm.is_callable(&v) {
                    return Err(link(realm, "function import requires a callable"));
                }
                let addr = match v.v() {
                    V::Object(id) => realm.native_data::<FuncRef>(id).map(|f| f.0),
                    _ => None,
                };
                match addr {
                    Some(a) => Extern::Func(s.func(a)),
                    None => {
                        let ft = canonical_func_type(m.canon[*t as usize]);
                        let r = realm.root(v.clone());
                        let st = st(realm);
                        st.imports.push(r);
                        let idx = st.imports.len() - 1;
                        Extern::Func(s.host_func(ft, Rc::new(JsImport(idx))))
                    }
                }
            }
            ImportDesc::Global(gt) => {
                if let Some(id) = host::<WGlobal>(realm, &v) {
                    Extern::Global(realm.host_data::<WGlobal>(id).unwrap().0.clone())
                } else {
                    let ty = canon_val(m, gt.ty);
                    if gt.mutable {
                        return Err(link(realm, "imported mutable global must be a WebAssembly.Global object"));
                    }
                    let ok = match ty {
                        ValType::I64 => matches!(v.v(), V::BigInt(_)),
                        ValType::I32 | ValType::F32 | ValType::F64 => matches!(v.v(), V::Number(_)),
                        ValType::V128 => false,
                        ValType::Ref(_) => true,
                    };
                    if !ok {
                        return Err(link(realm, "global import must be a number, valid Wasm reference, or WebAssembly.Global object"));
                    }
                    let val = to_wasm(realm, v, ty)?;
                    Extern::Global(s.new_global(GlobalType { ty, mutable: false }, val))
                }
            }
            ImportDesc::Memory(_) => match host::<WMemory>(realm, &v) {
                Some(id) => Extern::Memory(realm.host_data::<WMemory>(id).unwrap().0.clone()),
                None => return Err(link(realm, "memory import must be a WebAssembly.Memory object")),
            },
            ImportDesc::Table(_) => match host::<WTable>(realm, &v) {
                Some(id) => Extern::Table(realm.host_data::<WTable>(id).unwrap().0.clone()),
                None => return Err(link(realm, "table import requires a WebAssembly.Table")),
            },
            ImportDesc::Tag(_) => match host::<WTag>(realm, &v) {
                Some(id) => Extern::Tag(realm.host_data::<WTag>(id).unwrap().0.clone()),
                None => return Err(link(realm, "tag import requires a WebAssembly.Tag")),
            },
        };
        externs.push(e);
    }
    let mut h = JsHost { realm, timeout: None };
    let r = s.instantiate(&mut h, cm, &externs);
    let timeout = h.timeout.take();
    match r {
        Ok(i) => Ok(i),
        Err(InstantiateError::Link(msg)) => Err(wasm_error(realm, 1, &format!("WebAssembly.Instance(): {msg}"))),
        Err(InstantiateError::Trap(t)) => Err(trap_to_js(realm, t, timeout)),
    }
}

// ------------------------------------------------------------------ Module / Instance

fn require_new(realm: &mut Realm, what: &str) -> JsResult<()> {
    if !realm.is_construct_call() {
        return Err(realm.throw_type_error(&format!("WebAssembly.{what} must be invoked with 'new'")));
    }
    Ok(())
}

fn module_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    require_new(realm, "Module")?;
    let bytes = buffer_source(realm, &arg(args, 0))?;
    let cm = compile_bytes(realm, &bytes)?;
    let d = st(realm).protos.module;
    let p = realm.proto_from_new_target(d);
    Ok(Value::Object(module_obj(realm, cm, Some(p))))
}

fn this_module(realm: &mut Realm, v: &Value) -> JsResult<Rc<CompiledModule>> {
    match host::<WModule>(realm, v) {
        Some(id) => Ok(realm.host_data::<WModule>(id).unwrap().0.clone()),
        None => Err(realm.throw_type_error("argument must be a WebAssembly.Module")),
    }
}

fn kind_name(k: ExternKind) -> &'static str {
    k.name()
}

fn module_exports(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let cm = this_module(realm, &arg(args, 0))?;
    let mut out = Vec::new();
    for e in &cm.module.exports {
        let o = realm.new_object();
        let n = realm.new_string(&e.name);
        realm.define_property(o, "name", n, rabbithole_js::ALL);
        realm.define_property(o, "kind", Value::str(kind_name(e.kind)), rabbithole_js::ALL);
        out.push(Value::Object(o));
    }
    Ok(realm.new_array_value(out))
}

fn module_imports(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let cm = this_module(realm, &arg(args, 0))?;
    let mut out = Vec::new();
    for i in &cm.module.imports {
        let o = realm.new_object();
        let m = realm.new_string(&i.module);
        let n = realm.new_string(&i.name);
        realm.define_property(o, "module", m, rabbithole_js::ALL);
        realm.define_property(o, "name", n, rabbithole_js::ALL);
        realm.define_property(o, "kind", Value::str(kind_name(i.desc.kind())), rabbithole_js::ALL);
        out.push(Value::Object(o));
    }
    Ok(realm.new_array_value(out))
}

fn module_custom_sections(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let cm = this_module(realm, &arg(args, 0))?;
    if args.len() < 2 {
        return Err(realm.throw_type_error("WebAssembly.Module.customSections(): Argument 1 is required"));
    }
    let name = realm.to_rust_string(arg(args, 1))?;
    let abp = realm.intrinsics.get("ArrayBufferPrototype");
    let mut out = Vec::new();
    for data in cm.module.custom_sections(&name) {
        let rc = Rc::new(RefCell::new(data.to_vec()));
        let o = realm.alloc(Object::new(abp, ObjectKind::ArrayBuffer(rc)));
        out.push(Value::Object(o));
    }
    Ok(realm.new_array_value(out))
}

fn instance_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    require_new(realm, "Instance")?;
    let cm = this_module(realm, &arg(args, 0))?;
    let inst = instantiate(realm, &cm, arg(args, 1))?;
    let d = st(realm).protos.instance;
    let p = realm.proto_from_new_target(d);
    Ok(Value::Object(instance_obj(realm, inst, Some(p))))
}

fn instance_exports(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let Some(id) = host::<WInstance>(realm, &this) else { return Err(realm.throw_type_error("not a WebAssembly.Instance")) };
    let key = st(realm).key_exports;
    Ok(realm.get_own_property(id, &PropertyKey::Symbol(key)).map(|p| p.value).unwrap_or(Value::Undefined))
}

// ------------------------------------------------------------------ exported functions

fn call_export(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let me = realm.current_native().unwrap();
    let addr = realm.native_data::<FuncRef>(me).unwrap().0;
    let f = store(realm).func(addr);
    let ft = f.ft.clone();
    if ft.params.iter().chain(ft.results.iter()).any(|t| *t == ValType::V128) {
        return Err(realm.throw_type_error("type incompatibility when transforming from/to JS"));
    }
    let mut vals = Vec::with_capacity(ft.params.len());
    for (i, t) in ft.params.iter().enumerate() {
        vals.push(to_wasm(realm, arg(args, i), *t)?);
    }
    let t0 = stats::enabled().then(std::time::Instant::now);
    let res = call_wasm(realm, &f, &vals);
    if let Some(t0) = t0 {
        stats::call(t0.elapsed());
    }
    let res = res?;
    match res.len() {
        0 => Ok(Value::Undefined),
        1 => Ok(to_js(realm, res[0], ft.results[0])),
        _ => {
            let mut out = Vec::with_capacity(res.len());
            for (v, t) in res.iter().zip(ft.results.iter()) {
                out.push(to_js(realm, *v, *t));
            }
            Ok(realm.new_array_value(out))
        }
    }
}

// ------------------------------------------------------------------ Memory

fn get_opt(realm: &mut Realm, desc: &Value, key: &str) -> JsResult<Option<Value>> {
    let v = realm.get_str(desc.clone(), key)?;
    Ok(if v.is_undefined() { None } else { Some(v) })
}

/// [EnforceRange] unsigned integer up to `max`.
fn enforce_range(realm: &mut Realm, v: Value, max: f64, what: &str) -> JsResult<u64> {
    let n = realm.to_number(v)?;
    if !n.is_finite() {
        return Err(realm.throw_type_error(&format!("{what} must be a finite number")));
    }
    let n = n.trunc();
    if n < 0.0 || n > max {
        return Err(realm.throw_type_error(&format!("{what} is out of range")));
    }
    Ok(n as u64)
}

fn memory_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    require_new(realm, "Memory")?;
    let desc = arg(args, 0);
    if !desc.is_object() {
        return Err(realm.throw_type_error("WebAssembly.Memory(): Argument 0 must be a memory descriptor"));
    }
    let is64 = match get_opt(realm, &desc, "address")? {
        Some(a) => realm.to_rust_string(a)? == "i64",
        None => false,
    };
    let maximum = match get_opt(realm, &desc, "maximum")? {
        Some(v) => Some(enforce_range(realm, v, 4294967295.0, "maximum")?),
        None => None,
    };
    let initial = match get_opt(realm, &desc, "initial")? {
        Some(v) => enforce_range(realm, v, 4294967295.0, "initial")?,
        None => return Err(realm.throw_type_error("WebAssembly.Memory(): Property 'initial' is required")),
    };
    let limit = if is64 { 1u64 << 48 } else { 65536 };
    if initial > limit {
        return Err(realm.range_error::<Value>("WebAssembly.Memory(): Property 'initial': value is above the upper bound").unwrap_err());
    }
    if let Some(m) = maximum {
        if m > limit || m < initial {
            return Err(realm.range_error::<Value>("WebAssembly.Memory(): Property 'maximum' is out of range").unwrap_err());
        }
    }
    let shared = match get_opt(realm, &desc, "shared")? {
        Some(v) => realm.to_boolean(&v),
        None => false,
    };
    if shared && maximum.is_none() {
        return Err(realm.throw_type_error("WebAssembly.Memory(): shared memory must have a maximum"));
    }
    let ty = MemoryType { limits: Limits { min: initial, max: maximum }, shared, memory64: is64, page_log2: 16 };
    let Some(mem) = store(realm).new_memory(ty) else {
        return Err(realm.range_error::<Value>("WebAssembly.Memory(): could not allocate memory").unwrap_err());
    };
    let d = st(realm).protos.memory;
    let p = realm.proto_from_new_target(d);
    let id = new_host_object(realm, p, Box::new(WMemory(mem.clone())));
    cache(realm, 0, Rc::as_ptr(&mem) as usize, id);
    Ok(Value::Object(id))
}

fn this_memory(realm: &mut Realm, this: &Value) -> JsResult<(ObjectId, Rc<MemInst>)> {
    match host::<WMemory>(realm, this) {
        Some(id) => Ok((id, realm.host_data::<WMemory>(id).unwrap().0.clone())),
        None => Err(realm.throw_type_error("Receiver is not a WebAssembly.Memory")),
    }
}

fn memory_buffer(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (id, mem) = this_memory(realm, &this)?;
    let cur: MemBuf = mem.buf.borrow().clone();
    let key = PropertyKey::Symbol(st(realm).key_buffer);
    if let Some(p) = realm.get_own_property(id, &key) {
        if let V::Object(ab) = p.value.v() {
            if let ObjectKind::ArrayBuffer(b) = &realm.obj(ab).kind {
                if Rc::ptr_eq(b, &cur) {
                    return Ok(p.value.clone());
                }
            }
        }
    }
    let abp = realm.intrinsics.get("ArrayBufferPrototype");
    let ab = realm.alloc(Object::new(abp, ObjectKind::ArrayBuffer(cur)));
    realm.define_property(id, key, Value::Object(ab), 0);
    Ok(Value::Object(ab))
}

fn memory_grow(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, mem) = this_memory(realm, &this)?;
    let delta = enforce_range(realm, arg(args, 0), 4294967295.0, "delta")?;
    match mem.grow(delta) {
        Some(old) => Ok(Value::Number(old as f64)),
        None => Err(realm.range_error::<Value>("WebAssembly.Memory.grow(): Maximum memory size exceeded").unwrap_err()),
    }
}

// ------------------------------------------------------------------ Table

fn ref_type_name(realm: &mut Realm, v: Value) -> JsResult<RefType> {
    let s = realm.to_rust_string(v)?;
    match val_type_name(&s) {
        Some(ValType::Ref(r)) => Ok(r),
        _ => Err(realm.throw_type_error("WebAssembly.Table(): Descriptor property 'element' must be a WebAssembly reference type")),
    }
}

fn table_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    require_new(realm, "Table")?;
    let desc = arg(args, 0);
    if !desc.is_object() {
        return Err(realm.throw_type_error("WebAssembly.Table(): Argument 0 must be a table descriptor"));
    }
    let el = realm.get_str(desc.clone(), "element")?;
    let elem = ref_type_name(realm, el)?;
    let maximum = match get_opt(realm, &desc, "maximum")? {
        Some(v) => Some(enforce_range(realm, v, 4294967295.0, "maximum")?),
        None => None,
    };
    let initial = match get_opt(realm, &desc, "initial")? {
        Some(v) => enforce_range(realm, v, 4294967295.0, "initial")?,
        None => return Err(realm.throw_type_error("WebAssembly.Table(): Property 'initial' is required")),
    };
    if initial > 10_000_000 {
        return Err(realm.range_error::<Value>("WebAssembly.Table(): Property 'initial': value is above the upper bound").unwrap_err());
    }
    if let Some(m) = maximum {
        if m < initial {
            return Err(realm.range_error::<Value>("WebAssembly.Table(): Property 'maximum' is below 'initial'").unwrap_err());
        }
    }
    let t = ValType::Ref(elem);
    let init = if args.len() > 1 && !arg(args, 1).is_undefined() { to_wasm(realm, arg(args, 1), t)? } else { default_value(realm, t)? };
    let Val::Ref(bits) = init else { unreachable!() };
    let Some(tab) = store(realm).new_table(TableType { elem, limits: Limits { min: initial, max: maximum }, table64: false }, bits) else {
        return Err(realm.range_error::<Value>("WebAssembly.Table(): could not allocate table").unwrap_err());
    };
    let d = st(realm).protos.table;
    let p = realm.proto_from_new_target(d);
    let id = new_host_object(realm, p, Box::new(WTable(tab.clone())));
    cache(realm, 1, Rc::as_ptr(&tab) as usize, id);
    Ok(Value::Object(id))
}

fn this_table(realm: &mut Realm, this: &Value) -> JsResult<Rc<TableInst>> {
    match host::<WTable>(realm, this) {
        Some(id) => Ok(realm.host_data::<WTable>(id).unwrap().0.clone()),
        None => Err(realm.throw_type_error("Receiver is not a WebAssembly.Table")),
    }
}

fn table_length(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let t = this_table(realm, &this)?;
    Ok(Value::Number(t.size() as f64))
}

fn table_get(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let t = this_table(realm, &this)?;
    let i = enforce_range(realm, arg(args, 0), 4294967295.0, "index")?;
    let bits = t.elems.borrow().get(i as usize).copied();
    match bits {
        Some(b) => Ok(to_js(realm, Val::Ref(b), ValType::Ref(t.ty.elem))),
        None => Err(realm.range_error::<Value>("WebAssembly.Table.get(): invalid address").unwrap_err()),
    }
}

fn table_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let t = this_table(realm, &this)?;
    let i = enforce_range(realm, arg(args, 0), 4294967295.0, "index")?;
    let ty = ValType::Ref(t.ty.elem);
    let v = if args.len() > 1 { to_wasm(realm, arg(args, 1), ty)? } else { default_value(realm, ty)? };
    let Val::Ref(bits) = v else { unreachable!() };
    let mut el = t.elems.borrow_mut();
    match el.get_mut(i as usize) {
        Some(slot) => {
            *slot = bits;
            Ok(Value::Undefined)
        }
        None => {
            drop(el);
            Err(realm.range_error::<Value>("WebAssembly.Table.set(): invalid address").unwrap_err())
        }
    }
}

fn table_grow(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let t = this_table(realm, &this)?;
    let n = enforce_range(realm, arg(args, 0), 4294967295.0, "delta")?;
    let ty = ValType::Ref(t.ty.elem);
    let v = if args.len() > 1 { to_wasm(realm, arg(args, 1), ty)? } else { default_value(realm, ty)? };
    let Val::Ref(bits) = v else { unreachable!() };
    let limit = store(realm).table_limit;
    match t.grow(n, bits, limit) {
        Some(old) => Ok(Value::Number(old as f64)),
        None => Err(realm.range_error::<Value>("WebAssembly.Table.grow(): failed to grow table").unwrap_err()),
    }
}

// ------------------------------------------------------------------ Global

fn global_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    require_new(realm, "Global")?;
    let desc = arg(args, 0);
    if !desc.is_object() {
        return Err(realm.throw_type_error("WebAssembly.Global(): Argument 0 must be a global descriptor"));
    }
    let m = realm.get_str(desc.clone(), "mutable")?;
    let mutable = realm.to_boolean(&m);
    let tv = realm.get_str(desc.clone(), "value")?;
    let ts = realm.to_rust_string(tv)?;
    let Some(ty) = val_type_name(&ts) else {
        return Err(realm.throw_type_error("WebAssembly.Global(): Descriptor property 'value' must be a WebAssembly type"));
    };
    if ty == ValType::V128 {
        return Err(realm.throw_type_error("WebAssembly.Global(): v128 globals cannot be created from JavaScript"));
    }
    let v = if args.len() > 1 && !arg(args, 1).is_undefined() { to_wasm(realm, arg(args, 1), ty)? } else { default_value(realm, ty)? };
    let g = store(realm).new_global(GlobalType { ty, mutable }, v);
    let d = st(realm).protos.global;
    let p = realm.proto_from_new_target(d);
    let id = new_host_object(realm, p, Box::new(WGlobal(g.clone())));
    cache(realm, 2, Rc::as_ptr(&g) as usize, id);
    Ok(Value::Object(id))
}

fn this_global(realm: &mut Realm, this: &Value) -> JsResult<Rc<GlobalInst>> {
    match host::<WGlobal>(realm, this) {
        Some(id) => Ok(realm.host_data::<WGlobal>(id).unwrap().0.clone()),
        None => Err(realm.throw_type_error("Receiver is not a WebAssembly.Global")),
    }
}

fn global_get(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let g = this_global(realm, &this)?;
    if g.ty.ty == ValType::V128 {
        return Err(realm.throw_type_error("v128 globals cannot be read from JavaScript"));
    }
    Ok(to_js(realm, g.get(), g.ty.ty))
}

fn global_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let g = this_global(realm, &this)?;
    if !g.ty.mutable {
        return Err(realm.throw_type_error("Can't set the value of an immutable global."));
    }
    if args.is_empty() {
        return Err(realm.throw_type_error("Argument 0 is required"));
    }
    let v = to_wasm(realm, arg(args, 0), g.ty.ty)?;
    g.set(v);
    Ok(Value::Undefined)
}

// ------------------------------------------------------------------ Tag / Exception

fn tag_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    require_new(realm, "Tag")?;
    let desc = arg(args, 0);
    if !desc.is_object() {
        return Err(realm.throw_type_error("WebAssembly.Tag(): Argument 0 must be a tag type"));
    }
    let ps = realm.get_str(desc, "parameters")?;
    let list = realm.iterate_to_vec(ps)?;
    let mut params = Vec::new();
    for p in list {
        let s = realm.to_rust_string(p)?;
        match val_type_name(&s) {
            Some(t) => params.push(t),
            None => return Err(realm.throw_type_error("WebAssembly.Tag(): parameter is not a valid value type")),
        }
    }
    let tag = store(realm).new_tag(FuncType::new(params, vec![]));
    let d = st(realm).protos.tag;
    let p = realm.proto_from_new_target(d);
    let id = new_host_object(realm, p, Box::new(WTag(tag.clone())));
    cache(realm, 3, Rc::as_ptr(&tag) as usize, id);
    Ok(Value::Object(id))
}

fn this_tag(realm: &mut Realm, v: &Value) -> JsResult<Rc<TagInst>> {
    match host::<WTag>(realm, v) {
        Some(id) => Ok(realm.host_data::<WTag>(id).unwrap().0.clone()),
        None => Err(realm.throw_type_error("argument is not a WebAssembly.Tag")),
    }
}

fn exception_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    require_new(realm, "Exception")?;
    let tag = this_tag(realm, &arg(args, 0))?;
    let payload = realm.iterate_to_vec(arg(args, 1))?;
    if payload.len() != tag.ft.params.len() {
        return Err(realm.throw_type_error("WebAssembly.Exception(): number of payload values does not match the tag"));
    }
    let mut slots = Vec::new();
    for (v, t) in payload.into_iter().zip(tag.ft.params.iter()) {
        let w = to_wasm(realm, v, *t)?;
        let b = w.to_bits();
        slots.push(b as u64);
        if t.slots() == 2 {
            slots.push((b >> 64) as u64);
        }
    }
    let exn = store(realm).new_exn(tag, slots);
    let d = st(realm).protos.exception;
    let p = realm.proto_from_new_target(d);
    Ok(Value::Object(new_host_object(realm, p, Box::new(WException(exn)))))
}

fn this_exception(realm: &mut Realm, this: &Value) -> JsResult<u32> {
    match host::<WException>(realm, this) {
        Some(id) => Ok(realm.host_data::<WException>(id).unwrap().0),
        None => Err(realm.throw_type_error("Receiver is not a WebAssembly.Exception")),
    }
}

fn exception_get_arg(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let exn = this_exception(realm, &this)?;
    let tag = this_tag(realm, &arg(args, 0))?;
    let (etag, vals) = store(realm).exn_values(exn);
    if !Rc::ptr_eq(&etag, &tag) {
        return Err(realm.throw_type_error("WebAssembly.Exception.getArg(): First argument does not match the exception tag"));
    }
    let i = enforce_range(realm, arg(args, 1), 4294967295.0, "index")? as usize;
    match vals.get(i) {
        Some(v) => Ok(to_js(realm, *v, tag.ft.params[i])),
        None => Err(realm.range_error::<Value>("WebAssembly.Exception.getArg(): Index out of range").unwrap_err()),
    }
}

fn exception_is(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let exn = this_exception(realm, &this)?;
    let tag = this_tag(realm, &arg(args, 0))?;
    let (etag, _) = store(realm).exn_values(exn);
    Ok(Value::Bool(Rc::ptr_eq(&etag, &tag)))
}

// ------------------------------------------------------------------ install

const PRELUDE: &str = r#"(function (WA) {
  'use strict';
  const define = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, configurable: true, enumerable: false });
  for (const name of ['CompileError', 'LinkError', 'RuntimeError']) {
    const C = class extends Error {
      constructor(message, options) { super(message, options); }
    };
    Object.defineProperty(C, 'name', { value: name, configurable: true });
    define(C.prototype, 'name', name);
    define(WA, name, C);
  }
  const compile = WA.compile, instantiate = WA.instantiate;
  async function response(source) {
    const r = await source;
    if (typeof Response === 'undefined' || !(r instanceof Response))
      throw new TypeError("WebAssembly.compileStreaming(): Argument 0 must be a Response or a Promise of one");
    const ct = r.headers.get('Content-Type');
    if (!ct || ct.split(';')[0].trim().toLowerCase() !== 'application/wasm')
      throw new TypeError("WebAssembly.compileStreaming(): Incorrect response MIME type. Expected 'application/wasm'.");
    if (!r.ok) throw new TypeError("WebAssembly.compileStreaming(): HTTP status code is not ok");
    return r.arrayBuffer();
  }
  define(WA, 'compileStreaming', function compileStreaming(source) {
    return response(source).then(b => compile(b));
  });
  define(WA, 'instantiateStreaming', function instantiateStreaming(source, importObject) {
    return response(source).then(b => instantiate(b, importObject));
  });
  Object.defineProperty(WA, Symbol.toStringTag, { value: 'WebAssembly', configurable: true });
  for (const [C, tag] of [[WA.Module, 'WebAssembly.Module'], [WA.Instance, 'WebAssembly.Instance'], [WA.Memory, 'WebAssembly.Memory'],
                          [WA.Table, 'WebAssembly.Table'], [WA.Global, 'WebAssembly.Global'], [WA.Tag, 'WebAssembly.Tag'], [WA.Exception, 'WebAssembly.Exception']])
    Object.defineProperty(C.prototype, Symbol.toStringTag, { value: tag, configurable: true });
  Object.setPrototypeOf(WA.Exception.prototype, Error.prototype);
})"#;

fn ctor(realm: &mut Realm, ns: ObjectId, name: &str, arity: u32, f: rabbithole_js::NativeFn) -> (ObjectId, ObjectId) {
    let proto = realm.new_object();
    let c = realm.new_native_constructor(name, arity, f, proto);
    realm.define_property(ns, name, Value::Object(c), WRITABLE | CONFIGURABLE);
    (c, proto)
}

/// Install the `WebAssembly` namespace on the realm's global object.
pub fn install(realm: &mut Realm) {
    if realm.intrinsics.others.contains_key("WasmState") {
        return;
    }
    set_detach_hook(rabbithole_js::builtins::typed_array::detach_buffer);
    let ns = realm.new_object();
    realm.define_method(ns, "validate", 1, wa_validate);
    realm.define_method(ns, "compile", 1, wa_compile);
    realm.define_method(ns, "instantiate", 1, wa_instantiate);
    let (mc, module) = ctor(realm, ns, "Module", 1, module_ctor);
    realm.define_method(mc, "exports", 1, module_exports);
    realm.define_method(mc, "imports", 1, module_imports);
    realm.define_method(mc, "customSections", 2, module_custom_sections);
    let (_, instance) = ctor(realm, ns, "Instance", 1, instance_ctor);
    realm.define_accessor(instance, "exports", Some(instance_exports), None, CONFIGURABLE | ENUMERABLE);
    let (_, memory) = ctor(realm, ns, "Memory", 1, memory_ctor);
    realm.define_accessor(memory, "buffer", Some(memory_buffer), None, CONFIGURABLE | ENUMERABLE);
    realm.define_method(memory, "grow", 1, memory_grow);
    let (_, table) = ctor(realm, ns, "Table", 1, table_ctor);
    realm.define_accessor(table, "length", Some(table_length), None, CONFIGURABLE | ENUMERABLE);
    realm.define_method(table, "get", 1, table_get);
    realm.define_method(table, "set", 1, table_set);
    realm.define_method(table, "grow", 1, table_grow);
    let (_, global) = ctor(realm, ns, "Global", 1, global_ctor);
    realm.define_accessor(global, "value", Some(global_get), Some(global_set), CONFIGURABLE | ENUMERABLE);
    realm.define_method(global, "valueOf", 0, global_get);
    let (_, tag) = ctor(realm, ns, "Tag", 1, tag_ctor);
    let (_, exception) = ctor(realm, ns, "Exception", 2, exception_ctor);
    realm.define_method(exception, "getArg", 2, exception_get_arg);
    realm.define_method(exception, "is", 1, exception_is);

    let key_exports = realm.new_private_symbol(JsStr::new("wasm exports"));
    let key_buffer = realm.new_private_symbol(JsStr::new("wasm buffer"));
    let store = Store::new();
    // Compiled wasm frames stay above the JS engine's stack guard.
    store.native_floor.set(rabbithole_js::stack::limit());
    let state = State {
        store: Rc::new(store),
        imports: Vec::new(),
        func_objs: HashMap::new(),
        mem_objs: HashMap::new(),
        table_objs: HashMap::new(),
        global_objs: HashMap::new(),
        tag_objs: HashMap::new(),
        gc_objs: HashMap::new(),
        externs: Vec::new(),
        extern_free: Vec::new(),
        extern_ids: HashMap::new(),
        extern_live: 0,
        extern_sweep_at: 4096,
        extern_sweep_pending: false,
        protos: Protos { module, instance, memory, table, global, tag, exception },
        errors: Vec::new(),
        key_exports,
        key_buffer,
    };
    let sid = realm.new_object();
    realm.set_host_data(sid, Box::new(state));
    realm.intrinsics.others.insert("WasmState", sid);
    // Keep the prototypes alive through the namespace object (they are
    // reachable from the constructors) and the state object itself.
    realm.define_global("WebAssembly", Value::Object(ns));

    if let Ok(f) = realm.eval(PRELUDE, "rabbithole:wasm.js") {
        let _ = realm.call(f, Value::Undefined, &[Value::Object(ns)]);
    }
    let mut errors = Vec::new();
    for n in ["CompileError", "LinkError", "RuntimeError"] {
        if let Ok(c) = realm.get_str(Value::Object(ns), n) {
            errors.push(realm.root(c));
        }
    }
    st(realm).errors = errors;
}
