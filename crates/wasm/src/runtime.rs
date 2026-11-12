//! Runtime structures: the store, function/table/memory/global/tag
//! instances, module instantiation and linking.

use crate::ir::Code;
use crate::module::*;
use crate::types::*;
use std::any::Any;
use std::cell::{Cell, RefCell, UnsafeCell};
use std::fmt;
use std::rc::Rc;

/// A runtime value. References are raw 64-bit cells: 0 is null; function
/// references are store function addresses + 1; external references are
/// host-defined non-zero bits; exception references are exception
/// addresses + 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Val {
    I32(i32),
    I64(i64),
    F32(u32),
    F64(u64),
    V128(u128),
    Ref(u64),
}

impl Val {
    pub fn default_of(t: ValType) -> Val {
        match t {
            ValType::I32 => Val::I32(0),
            ValType::I64 => Val::I64(0),
            ValType::F32 => Val::F32(0),
            ValType::F64 => Val::F64(0),
            ValType::V128 => Val::V128(0),
            ValType::Ref(_) => Val::Ref(0),
        }
    }
    pub fn to_bits(self) -> u128 {
        match self {
            Val::I32(v) => v as u32 as u128,
            Val::I64(v) => v as u64 as u128,
            Val::F32(v) => v as u128,
            Val::F64(v) => v as u128,
            Val::V128(v) => v,
            Val::Ref(v) => v as u128,
        }
    }
    pub fn from_bits(t: ValType, b: u128) -> Val {
        match t {
            ValType::I32 => Val::I32(b as u32 as i32),
            ValType::I64 => Val::I64(b as u64 as i64),
            ValType::F32 => Val::F32(b as u32),
            ValType::F64 => Val::F64(b as u64),
            ValType::V128 => Val::V128(b),
            ValType::Ref(_) => Val::Ref(b as u64),
        }
    }
}

#[derive(Clone)]
pub enum Trap {
    Unreachable,
    MemoryOutOfBounds,
    TableOutOfBounds,
    UndefinedElement,
    UninitializedElement,
    IndirectCallTypeMismatch,
    IntegerOverflow,
    IntegerDivideByZero,
    InvalidConversion,
    StackExhausted,
    NullReference,
    NullFunctionReference,
    /// Execution interrupted by the host (watchdog).
    Interrupted,
    /// An uncaught wasm exception (store exception address).
    Exception(u32),
    /// Error raised by a host function (e.g. a JavaScript exception).
    Host(Rc<dyn Any>),
    /// Other runtime error with a message.
    Other(String),
}

impl Trap {
    pub fn message(&self) -> String {
        match self {
            Trap::Unreachable => "unreachable".into(),
            Trap::MemoryOutOfBounds => "out of bounds memory access".into(),
            Trap::TableOutOfBounds => "out of bounds table access".into(),
            Trap::UndefinedElement => "undefined element".into(),
            Trap::UninitializedElement => "uninitialized element".into(),
            Trap::IndirectCallTypeMismatch => "indirect call type mismatch".into(),
            Trap::IntegerOverflow => "integer overflow".into(),
            Trap::IntegerDivideByZero => "integer divide by zero".into(),
            Trap::InvalidConversion => "invalid conversion to integer".into(),
            Trap::StackExhausted => "call stack exhausted".into(),
            Trap::NullReference => "null reference".into(),
            Trap::NullFunctionReference => "null function reference".into(),
            Trap::Interrupted => "interrupted".into(),
            Trap::Exception(_) => "uncaught exception".into(),
            Trap::Host(_) => "host error".into(),
            Trap::Other(s) => s.clone(),
        }
    }
}

impl fmt::Debug for Trap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

/// A module after validation and translation (shared by its instances).
pub struct CompiledModule {
    pub module: Module,
    pub codes: Vec<Rc<Code>>,
}

impl fmt::Debug for CompiledModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CompiledModule({} funcs)", self.codes.len())
    }
}

impl CompiledModule {
    /// Decode, validate and translate a binary module.
    pub fn new(bytes: &[u8]) -> Result<CompiledModule, crate::Error> {
        let mut module = crate::binary::decode(bytes)?;
        let codes = crate::validate::validate(&mut module)?;
        Ok(CompiledModule { module, codes: codes.into_iter().map(Rc::new).collect() })
    }

    /// Import descriptors with canonical-type function signatures.
    pub fn import_type(&self, i: usize) -> &ImportDesc {
        &self.module.imports[i].desc
    }
}

pub enum FuncKind {
    Wasm { instance: u32, code: Rc<Code> },
    Host { data: Rc<dyn Any> },
}

pub struct FuncInst {
    /// Canonical type id.
    pub ty: TypeId,
    pub ft: FuncType,
    pub kind: FuncKind,
    /// Store address.
    pub addr: u32,
}

pub struct TableInst {
    pub elems: RefCell<Vec<u64>>,
    pub ty: TableType,
    /// Canonical element heap type check context: canonical id for
    /// concrete element types.
    pub elem_canon: Option<TypeId>,
}

impl TableInst {
    pub fn size(&self) -> u64 {
        self.elems.borrow().len() as u64
    }
    pub fn grow(&self, n: u64, init: u64, limit: u64) -> Option<u64> {
        let mut e = self.elems.borrow_mut();
        let old = e.len() as u64;
        let new = old.checked_add(n)?;
        let max = self.ty.limits.max.unwrap_or(if self.ty.table64 { u64::MAX } else { u32::MAX as u64 });
        if new > max || new > limit {
            return None;
        }
        if e.try_reserve(n as usize).is_err() {
            return None;
        }
        e.resize(new as usize, init);
        Some(old)
    }
}

/// Shared byte buffer of a linear memory (the same type as a JavaScript
/// `ArrayBuffer`'s storage, so `Memory.buffer` aliases it).
pub type MemBuf = Rc<RefCell<Vec<u8>>>;

/// A zero-filled byte vector from the allocator's zeroed pages (lazily
/// committed by the OS); None when the allocation fails.
pub fn zeroed_vec(n: usize) -> Option<Vec<u8>> {
    if n == 0 {
        return Some(Vec::new());
    }
    let layout = std::alloc::Layout::array::<u8>(n).ok()?;
    // SAFETY: non-zero size; the pointer is checked and owned by the Vec
    // with the same layout (u8, capacity n).
    unsafe {
        let p = std::alloc::alloc_zeroed(layout);
        if p.is_null() {
            return None;
        }
        Some(Vec::from_raw_parts(p, n, n))
    }
}

pub struct MemInst {
    /// Current buffer; replaced (the old one emptied: detached) on grow.
    pub buf: RefCell<MemBuf>,
    pub ty: MemoryType,
    /// Byte size limit enforced by the embedder.
    pub limit: u64,
}

impl MemInst {
    pub fn new(ty: MemoryType, limit: u64) -> Option<MemInst> {
        let bytes = ty.limits.min.checked_mul(1u64 << ty.page_log2)?;
        if bytes > limit {
            return None;
        }
        let v = zeroed_vec(bytes as usize)?;
        Some(MemInst { buf: RefCell::new(Rc::new(RefCell::new(v))), ty, limit })
    }
    pub fn page_size(&self) -> u64 {
        1u64 << self.ty.page_log2
    }
    pub fn len(&self) -> usize {
        self.buf.borrow().borrow().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn pages(&self) -> u64 {
        self.len() as u64 >> self.ty.page_log2
    }
    /// Raw data pointer and length (valid until the next grow or host
    /// call).
    #[inline]
    pub fn raw(&self) -> (*mut u8, usize) {
        let b = self.buf.borrow();
        // SAFETY: the vector is only reallocated by `grow`, which callers
        // must follow by refreshing the pointer.
        let v = unsafe { &mut *b.as_ptr() };
        (v.as_mut_ptr(), v.len())
    }
    /// Grow by `delta` pages; returns the old size in pages.
    pub fn grow(&self, delta: u64) -> Option<u64> {
        let old = self.pages();
        let new = old.checked_add(delta)?;
        let maxp = self.ty.limits.max.unwrap_or(if self.ty.memory64 { u64::MAX >> self.ty.page_log2 } else { (1u64 << 32) >> self.ty.page_log2 });
        if new > maxp {
            return None;
        }
        let bytes = new.checked_mul(self.page_size())?;
        if bytes > self.limit || bytes > isize::MAX as u64 {
            return None;
        }
        if delta == 0 {
            return Some(old);
        }
        let mut v = zeroed_vec(bytes as usize)?;
        let cur = self.buf.borrow().clone();
        let old_data = std::mem::take(&mut *cur.borrow_mut());
        v[..old_data.len()].copy_from_slice(&old_data);
        drop(old_data);
        *self.buf.borrow_mut() = Rc::new(RefCell::new(v));
        Some(old)
    }
}

pub struct GlobalInst {
    pub ty: GlobalType,
    pub lo: Cell<u64>,
    pub hi: Cell<u64>,
}

impl GlobalInst {
    pub fn new(ty: GlobalType, v: u128) -> GlobalInst {
        GlobalInst { ty, lo: Cell::new(v as u64), hi: Cell::new((v >> 64) as u64) }
    }
    pub fn get(&self) -> Val {
        Val::from_bits(self.ty.ty, self.lo.get() as u128 | (self.hi.get() as u128) << 64)
    }
    pub fn set(&self, v: Val) {
        let b = v.to_bits();
        self.lo.set(b as u64);
        self.hi.set((b >> 64) as u64);
    }
}

pub struct TagInst {
    pub ty: TypeId,
    pub ft: FuncType,
}

pub struct Exn {
    pub tag: Rc<TagInst>,
    pub payload: Vec<u64>,
}

#[derive(Clone)]
pub enum Extern {
    Func(Rc<FuncInst>),
    Table(Rc<TableInst>),
    Memory(Rc<MemInst>),
    Global(Rc<GlobalInst>),
    Tag(Rc<TagInst>),
}

impl Extern {
    pub fn kind(&self) -> ExternKind {
        match self {
            Extern::Func(_) => ExternKind::Func,
            Extern::Table(_) => ExternKind::Table,
            Extern::Memory(_) => ExternKind::Memory,
            Extern::Global(_) => ExternKind::Global,
            Extern::Tag(_) => ExternKind::Tag,
        }
    }
}

pub struct Instance {
    pub module: Rc<CompiledModule>,
    pub index: u32,
    pub funcs: Vec<Rc<FuncInst>>,
    pub tables: Vec<Rc<TableInst>>,
    pub memories: Vec<Rc<MemInst>>,
    pub globals: Vec<Rc<GlobalInst>>,
    pub tags: Vec<Rc<TagInst>>,
    pub elems: RefCell<Vec<Rc<[u64]>>>,
    pub data_dropped: RefCell<Vec<bool>>,
    pub exports: Vec<(String, Extern)>,
}

impl Instance {
    pub fn export(&self, name: &str) -> Option<&Extern> {
        self.exports.iter().find(|(n, _)| n == name).map(|(_, e)| e)
    }
    pub fn data_bytes(&self, seg: u32) -> &[u8] {
        if self.data_dropped.borrow()[seg as usize] {
            return &[];
        }
        let r = self.module.module.datas[seg as usize].data.clone();
        &self.module.module.bytes[r]
    }
}

/// Embedder callbacks.
pub trait Host {
    /// Call a host function.
    fn call_host(&mut self, store: &Store, func: &Rc<FuncInst>, args: &[Val]) -> Result<Vec<Val>, Trap>;
    /// Polled at loop back-edges and calls; return true to interrupt.
    fn check_interrupt(&mut self) -> bool {
        false
    }
}

/// Owns all runtime objects of one embedding (a JavaScript realm, a test
/// script). Objects are never freed before the store.
pub struct Store {
    pub funcs: RefCell<Vec<Rc<FuncInst>>>,
    pub instances: RefCell<Vec<Rc<Instance>>>,
    pub exns: RefCell<Vec<Exn>>,
    /// GC structs and arrays.
    pub gc: RefCell<Vec<crate::gc::GcObj>>,
    pub(crate) stack: UnsafeCell<Vec<u64>>,
    /// First free value stack slot (nested invocations start there).
    pub(crate) stack_top: Cell<usize>,
    pub(crate) depth: Cell<usize>,
    /// Maximum value stack size in slots.
    pub max_stack_slots: usize,
    /// Maximum call depth (all nested invocations together).
    pub max_depth: usize,
    /// Maximum linear memory size in bytes.
    pub memory_limit: u64,
    /// Maximum table size in elements.
    pub table_limit: u64,
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

impl Store {
    pub fn new() -> Store {
        Store {
            funcs: RefCell::new(Vec::new()),
            instances: RefCell::new(Vec::new()),
            exns: RefCell::new(Vec::new()),
            gc: RefCell::new(Vec::new()),
            stack: UnsafeCell::new(Vec::new()),
            stack_top: Cell::new(0),
            depth: Cell::new(0),
            max_stack_slots: 4 << 20,
            max_depth: 60_000,
            memory_limit: 4 << 30,
            table_limit: 10_000_000,
        }
    }

    pub fn func(&self, addr: u32) -> Rc<FuncInst> {
        self.funcs.borrow()[addr as usize].clone()
    }

    pub fn instance(&self, idx: u32) -> Rc<Instance> {
        self.instances.borrow()[idx as usize].clone()
    }

    /// Create a host function.
    pub fn host_func(&self, ft: FuncType, data: Rc<dyn Any>) -> Rc<FuncInst> {
        let ty = intern_func_type(&ft);
        let mut fs = self.funcs.borrow_mut();
        let f = Rc::new(FuncInst { ty, ft, kind: FuncKind::Host { data }, addr: fs.len() as u32 });
        fs.push(f.clone());
        f
    }

    pub fn new_table(&self, ty: TableType, init: u64) -> Option<Rc<TableInst>> {
        if ty.limits.min > self.table_limit {
            return None;
        }
        let mut v = Vec::new();
        v.try_reserve_exact(ty.limits.min as usize).ok()?;
        v.resize(ty.limits.min as usize, init);
        Some(Rc::new(TableInst { elems: RefCell::new(v), ty, elem_canon: None }))
    }

    pub fn new_memory(&self, ty: MemoryType) -> Option<Rc<MemInst>> {
        MemInst::new(ty, self.memory_limit).map(Rc::new)
    }

    pub fn new_global(&self, ty: GlobalType, v: Val) -> Rc<GlobalInst> {
        Rc::new(GlobalInst::new(ty, v.to_bits()))
    }

    pub fn new_tag(&self, ft: FuncType) -> Rc<TagInst> {
        let ty = intern_func_type(&ft);
        Rc::new(TagInst { ty, ft })
    }

    pub fn new_exn(&self, tag: Rc<TagInst>, payload: Vec<u64>) -> u32 {
        let mut e = self.exns.borrow_mut();
        e.push(Exn { tag, payload });
        (e.len() - 1) as u32
    }

    /// Payload of an exception as values.
    pub fn exn_values(&self, exn: u32) -> (Rc<TagInst>, Vec<Val>) {
        let e = self.exns.borrow();
        let x = &e[exn as usize];
        let mut out = Vec::new();
        let mut i = 0;
        for &t in x.tag.ft.params.iter() {
            let b = if t.slots() == 2 { x.payload[i] as u128 | (x.payload[i + 1] as u128) << 64 } else { x.payload[i] as u128 };
            i += t.slots() as usize;
            out.push(Val::from_bits(t, b));
        }
        (x.tag.clone(), out)
    }

    /// Call a function with arguments (types are checked by the caller).
    pub fn call(&self, host: &mut dyn Host, func: &Rc<FuncInst>, args: &[Val]) -> Result<Vec<Val>, Trap> {
        crate::interp::invoke(self, host, func, args)
    }

    /// Instantiate a module with resolved imports (in import order).
    pub fn instantiate(&self, host: &mut dyn Host, cm: &Rc<CompiledModule>, imports: &[Extern]) -> Result<Rc<Instance>, InstantiateError> {
        let m = &cm.module;
        if imports.len() != m.imports.len() {
            return Err(InstantiateError::Link("wrong number of imports".into()));
        }
        let mut funcs = Vec::new();
        let mut tables = Vec::new();
        let mut memories = Vec::new();
        let mut globals = Vec::new();
        let mut tags = Vec::new();
        for (imp, ext) in m.imports.iter().zip(imports.iter()) {
            let err = |what: &str| InstantiateError::Link(format!("incompatible import type for {}.{}: {what}", imp.module, imp.name));
            match (&imp.desc, ext) {
                (ImportDesc::Func(t), Extern::Func(f)) => {
                    let want = m.canon[*t as usize];
                    if !canonical_subtype(f.ty, want) {
                        return Err(err("function type mismatch"));
                    }
                    funcs.push(f.clone());
                }
                (ImportDesc::Table(tt), Extern::Table(t)) => {
                    let cur = t.size();
                    if t.ty.table64 != tt.table64 || !limits_match(cur, t.ty.limits.max, tt.limits) {
                        return Err(err("table limits mismatch"));
                    }
                    if !ref_type_eq(m, t, tt.elem) {
                        return Err(err("table element type mismatch"));
                    }
                    tables.push(t.clone());
                }
                (ImportDesc::Memory(mt), Extern::Memory(mem)) => {
                    if mem.ty.memory64 != mt.memory64 || mem.ty.shared != mt.shared || mem.ty.page_log2 != mt.page_log2 || !limits_match(mem.pages(), mem.ty.limits.max, mt.limits) {
                        return Err(err("memory limits mismatch"));
                    }
                    memories.push(mem.clone());
                }
                (ImportDesc::Global(gt), Extern::Global(g)) => {
                    if g.ty.mutable != gt.mutable || !global_type_ok(m, g.ty.ty, gt.ty, gt.mutable) {
                        return Err(err("global type mismatch"));
                    }
                    globals.push(g.clone());
                }
                (ImportDesc::Tag(t), Extern::Tag(tag)) => {
                    if tag.ty != m.canon[*t as usize] {
                        return Err(err("tag type mismatch"));
                    }
                    tags.push(tag.clone());
                }
                _ => return Err(err("kind mismatch")),
            }
        }
        let inst_index = self.instances.borrow().len() as u32;
        // Defined functions.
        {
            let mut fs = self.funcs.borrow_mut();
            for (i, code) in cm.codes.iter().enumerate() {
                let t = m.funcs[m.num_imported_funcs as usize + i];
                let f = Rc::new(FuncInst {
                    ty: m.canon[t as usize],
                    ft: canonical_func_type(m.canon[t as usize]),
                    kind: FuncKind::Wasm { instance: inst_index, code: code.clone() },
                    addr: fs.len() as u32,
                });
                fs.push(f.clone());
                funcs.push(f);
            }
        }
        for &t in &m.tags[m.num_imported_tags as usize..] {
            let ty = m.canon[t as usize];
            tags.push(Rc::new(TagInst { ty, ft: canonical_func_type(ty) }));
        }
        // Globals (initializers see previous globals).
        for (i, g) in m.global_inits.iter().enumerate() {
            let gt = m.globals[m.num_imported_globals as usize + i];
            let v = eval_const(self, m, g, &globals, &funcs).map_err(InstantiateError::Trap)?;
            let gt = GlobalType { ty: canon_val(m, gt.ty), mutable: gt.mutable };
            globals.push(Rc::new(GlobalInst::new(gt, v)));
        }
        for (i, tt) in m.tables[m.num_imported_tables as usize..].iter().enumerate() {
            let init = match &m.table_inits[i] {
                Some(e) => eval_const(self, m, e, &globals, &funcs).map_err(InstantiateError::Trap)? as u64,
                None => 0,
            };
            let elem = match canon_val(m, ValType::Ref(tt.elem)) {
                ValType::Ref(r) => r,
                _ => unreachable!(),
            };
            let t = self.new_table(TableType { elem, ..*tt }, init).ok_or_else(|| InstantiateError::Trap(Trap::Other("table allocation failed".into())))?;
            tables.push(t);
        }
        for mt in &m.memories[m.num_imported_memories as usize..] {
            let mem = self.new_memory(*mt).ok_or_else(|| InstantiateError::Trap(Trap::Other("memory allocation failed".into())))?;
            memories.push(mem);
        }
        let mut elems: Vec<Rc<[u64]>> = Vec::with_capacity(m.elems.len());
        for e in &m.elems {
            let mut v = Vec::with_capacity(e.items.len());
            for it in &e.items {
                v.push(eval_const(self, m, it, &globals, &funcs).map_err(InstantiateError::Trap)? as u64);
            }
            elems.push(v.into());
        }
        let mut exports = Vec::new();
        for ex in &m.exports {
            let i = ex.index as usize;
            let e = match ex.kind {
                ExternKind::Func => Extern::Func(funcs[i].clone()),
                ExternKind::Table => Extern::Table(tables[i].clone()),
                ExternKind::Memory => Extern::Memory(memories[i].clone()),
                ExternKind::Global => Extern::Global(globals[i].clone()),
                ExternKind::Tag => Extern::Tag(tags[i].clone()),
            };
            exports.push((ex.name.clone(), e));
        }
        let inst = Rc::new(Instance {
            module: cm.clone(),
            index: inst_index,
            funcs,
            tables,
            memories,
            globals,
            tags,
            elems: RefCell::new(elems),
            data_dropped: RefCell::new(vec![false; m.datas.len()]),
            exports,
        });
        self.instances.borrow_mut().push(inst.clone());
        // Active and declarative element segments.
        for (i, e) in m.elems.iter().enumerate() {
            match &e.mode {
                ElemMode::Active { table, offset } => {
                    let off = eval_const(self, m, offset, &inst.globals, &inst.funcs).map_err(InstantiateError::Trap)? as u64;
                    let t = &inst.tables[*table as usize];
                    let items = inst.elems.borrow()[i].clone();
                    {
                        let mut el = t.elems.borrow_mut();
                        let end = off.checked_add(items.len() as u64);
                        match end {
                            Some(end) if end <= el.len() as u64 => {
                                el[off as usize..end as usize].copy_from_slice(&items);
                            }
                            _ => return Err(InstantiateError::Trap(Trap::TableOutOfBounds)),
                        }
                    }
                    inst.elems.borrow_mut()[i] = Rc::new([]);
                }
                ElemMode::Declarative => inst.elems.borrow_mut()[i] = Rc::new([]),
                ElemMode::Passive => {}
            }
        }
        for (i, d) in m.datas.iter().enumerate() {
            if let DataMode::Active { memory, offset } = &d.mode {
                let off = eval_const(self, m, offset, &inst.globals, &inst.funcs).map_err(InstantiateError::Trap)? as u64;
                let mem = &inst.memories[*memory as usize];
                let bytes = &m.bytes[d.data.clone()];
                let (p, len) = mem.raw();
                match off.checked_add(bytes.len() as u64) {
                    Some(end) if end <= len as u64 => {
                        // SAFETY: bounds checked above.
                        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.add(off as usize), bytes.len()) };
                    }
                    _ => return Err(InstantiateError::Trap(Trap::MemoryOutOfBounds)),
                }
                inst.data_dropped.borrow_mut()[i] = true;
            }
        }
        if let Some(s) = m.start {
            let f = inst.funcs[s as usize].clone();
            self.call(host, &f, &[]).map_err(InstantiateError::Trap)?;
        }
        Ok(inst)
    }
}

#[derive(Debug)]
pub enum InstantiateError {
    Link(String),
    Trap(Trap),
}

fn limits_match(cur_min: u64, cur_max: Option<u64>, want: Limits) -> bool {
    if cur_min < want.min {
        return false;
    }
    match want.max {
        None => true,
        Some(wm) => matches!(cur_max, Some(cm) if cm <= wm),
    }
}

pub fn canon_val(m: &Module, v: ValType) -> ValType {
    match v {
        ValType::Ref(RefType { nullable, heap: HeapType::Concrete(i) }) => ValType::Ref(RefType { nullable, heap: HeapType::Concrete(m.canon[i as usize]) }),
        o => o,
    }
}

/// Subtyping between canonical value types.
pub fn canon_val_sub(a: ValType, b: ValType) -> bool {
    use HeapType::*;
    match (a, b) {
        (ValType::Ref(x), ValType::Ref(y)) => {
            if x.nullable && !y.nullable {
                return false;
            }
            let is_func = |i: u32| matches!(canonical_type(i).comp, CompType::Func(_));
            let is_struct = |i: u32| matches!(canonical_type(i).comp, CompType::Struct(_));
            let is_array = |i: u32| matches!(canonical_type(i).comp, CompType::Array(_));
            match (x.heap, y.heap) {
                (p, q) if p == q => true,
                (Concrete(p), Concrete(q)) => canonical_subtype(p, q),
                (Concrete(p), Func) => is_func(p),
                (Concrete(p), Any | Eq) => !is_func(p),
                (Concrete(p), Struct) => is_struct(p),
                (Concrete(p), Array) => is_array(p),
                (None, Any | Eq | I31 | Struct | Array) => true,
                (None, Concrete(q)) => !is_func(q),
                (NoFunc, Func) => true,
                (NoFunc, Concrete(q)) => is_func(q),
                (NoExtern, Extern) | (NoExn, Exn) => true,
                (I31 | Struct | Array, Any | Eq) | (Eq, Any) => true,
                _ => false,
            }
        }
        _ => a == b,
    }
}

fn ref_type_eq(m: &Module, t: &TableInst, want: RefType) -> bool {
    let want = canon_val(m, ValType::Ref(want));
    let have = ValType::Ref(t.ty.elem);
    canon_val_sub(have, want) && canon_val_sub(want, have)
}

fn global_type_ok(m: &Module, have: ValType, want: ValType, mutable: bool) -> bool {
    let want = canon_val(m, want);
    if mutable {
        canon_val_sub(have, want) && canon_val_sub(want, have)
    } else {
        canon_val_sub(have, want)
    }
}

/// Evaluate a constant expression.
pub fn eval_const(store: &Store, m: &Module, e: &ConstExpr, globals: &[Rc<GlobalInst>], funcs: &[Rc<FuncInst>]) -> Result<u128, Trap> {
    use crate::gc::*;
    let mut st: Vec<u128> = Vec::with_capacity(4);
    for op in &e.ops {
        match *op {
            ConstOp::I32(v) => st.push(v as u32 as u128),
            ConstOp::I64(v) => st.push(v as u64 as u128),
            ConstOp::F32(v) => st.push(v as u128),
            ConstOp::F64(v) => st.push(v as u128),
            ConstOp::V128(v) => st.push(v),
            ConstOp::RefNull(_) => st.push(0),
            ConstOp::RefFunc(f) => st.push(funcs[f as usize].addr as u128 + 1),
            ConstOp::GlobalGet(g) => {
                let g = &globals[g as usize];
                st.push(g.lo.get() as u128 | (g.hi.get() as u128) << 64);
            }
            ConstOp::I32Add | ConstOp::I32Sub | ConstOp::I32Mul => {
                let b = st.pop().unwrap() as u32;
                let a = st.pop().unwrap() as u32;
                st.push(match op {
                    ConstOp::I32Add => a.wrapping_add(b),
                    ConstOp::I32Sub => a.wrapping_sub(b),
                    _ => a.wrapping_mul(b),
                } as u128);
            }
            ConstOp::I64Add | ConstOp::I64Sub | ConstOp::I64Mul => {
                let b = st.pop().unwrap() as u64;
                let a = st.pop().unwrap() as u64;
                st.push(match op {
                    ConstOp::I64Add => a.wrapping_add(b),
                    ConstOp::I64Sub => a.wrapping_sub(b),
                    _ => a.wrapping_mul(b),
                } as u128);
            }
            ConstOp::AnyConvertExtern => {
                let v = st.pop().unwrap() as u64;
                st.push(any_convert_extern(v) as u128);
            }
            ConstOp::ExternConvertAny => {
                let v = st.pop().unwrap() as u64;
                st.push(extern_convert_any(v) as u128);
            }
            ConstOp::RefI31 => {
                let v = st.pop().unwrap() as u32;
                st.push(i31_bits(v) as u128);
            }
            ConstOp::StructNew(t) | ConstOp::StructNewDefault(t) => {
                let ty = m.canon[t as usize];
                let CompType::Struct(fs) = canonical_type(ty).comp else { return Err(Trap::Other("not a struct".into())) };
                let mut vals: Vec<u128> = Vec::new();
                if matches!(op, ConstOp::StructNew(_)) {
                    let at = st.len() - fs.len();
                    vals = st.split_off(at);
                } else {
                    vals.resize(fs.len(), 0);
                }
                let mut data = Vec::new();
                for (f, v) in fs.iter().zip(vals) {
                    let k = kind_of(f.storage);
                    data.push(match k {
                        K_I8 => v as u64 & 0xff,
                        K_I16 => v as u64 & 0xffff,
                        _ => v as u64,
                    });
                    if k == K_V128 {
                        data.push((v >> 64) as u64);
                    }
                }
                st.push(alloc(store, GcObj { ty, is_array: false, kind: K_VAL, data }) as u128);
            }
            ConstOp::ArrayNew(t) | ConstOp::ArrayNewDefault(t) | ConstOp::ArrayNewFixed(t, _) => {
                let ty = m.canon[t as usize];
                let CompType::Array(f) = canonical_type(ty).comp else { return Err(Trap::Other("not an array".into())) };
                let k = kind_of(f.storage);
                let elems: Vec<u128> = match *op {
                    ConstOp::ArrayNewFixed(_, n) => {
                        let at = st.len() - n as usize;
                        st.split_off(at)
                    }
                    ConstOp::ArrayNew(_) => {
                        let n = st.pop().unwrap() as u32 as usize;
                        let v = st.pop().unwrap();
                        if n > 1 << 24 {
                            return Err(Trap::Other("out of memory".into()));
                        }
                        vec![v; n]
                    }
                    _ => {
                        let n = st.pop().unwrap() as u32 as usize;
                        if n > 1 << 24 {
                            return Err(Trap::Other("out of memory".into()));
                        }
                        vec![0; n]
                    }
                };
                let mut data = Vec::with_capacity(elems.len());
                for v in elems {
                    data.push(match k {
                        K_I8 => v as u64 & 0xff,
                        K_I16 => v as u64 & 0xffff,
                        _ => v as u64,
                    });
                    if k == K_V128 {
                        data.push((v >> 64) as u64);
                    }
                }
                st.push(alloc(store, GcObj { ty, is_array: true, kind: k, data }) as u128);
            }
        }
    }
    Ok(st.pop().unwrap_or(0))
}
