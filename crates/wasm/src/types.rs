//! WebAssembly types: value, reference, heap, function, composite (GC) and
//! definition types, plus the process-wide canonical type registry used to
//! compare types across modules (iso-recursive equivalence).

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

/// Heap types. `Concrete` holds a module-relative type index while a module
/// is decoded/validated; canonical forms live in the [`registry`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum HeapType {
    Func,
    Extern,
    Any,
    Eq,
    I31,
    Struct,
    Array,
    Exn,
    NoFunc,
    NoExtern,
    None,
    NoExn,
    Concrete(u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RefType {
    pub nullable: bool,
    pub heap: HeapType,
}

impl RefType {
    pub const FUNCREF: RefType = RefType { nullable: true, heap: HeapType::Func };
    pub const EXTERNREF: RefType = RefType { nullable: true, heap: HeapType::Extern };
    pub const EXNREF: RefType = RefType { nullable: true, heap: HeapType::Exn };
    pub const ANYREF: RefType = RefType { nullable: true, heap: HeapType::Any };
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ValType {
    I32,
    I64,
    F32,
    F64,
    V128,
    Ref(RefType),
}

impl ValType {
    pub const FUNCREF: ValType = ValType::Ref(RefType::FUNCREF);
    pub const EXTERNREF: ValType = ValType::Ref(RefType::EXTERNREF);

    /// Number of 64-bit interpreter slots a value of this type occupies.
    #[inline]
    pub fn slots(self) -> u32 {
        if self == ValType::V128 {
            2
        } else {
            1
        }
    }
    pub fn is_ref(self) -> bool {
        matches!(self, ValType::Ref(_))
    }
    pub fn is_num(self) -> bool {
        matches!(self, ValType::I32 | ValType::I64 | ValType::F32 | ValType::F64)
    }
    /// Has a default value (defaultable locals, struct fields...).
    pub fn defaultable(self) -> bool {
        match self {
            ValType::Ref(r) => r.nullable,
            _ => true,
        }
    }
}

impl fmt::Display for HeapType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            HeapType::Func => "func",
            HeapType::Extern => "extern",
            HeapType::Any => "any",
            HeapType::Eq => "eq",
            HeapType::I31 => "i31",
            HeapType::Struct => "struct",
            HeapType::Array => "array",
            HeapType::Exn => "exn",
            HeapType::NoFunc => "nofunc",
            HeapType::NoExtern => "noextern",
            HeapType::None => "none",
            HeapType::NoExn => "noexn",
            HeapType::Concrete(i) => return write!(f, "{}", i & !REC_BIT),
        };
        f.write_str(s)
    }
}

impl fmt::Display for ValType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValType::I32 => f.write_str("i32"),
            ValType::I64 => f.write_str("i64"),
            ValType::F32 => f.write_str("f32"),
            ValType::F64 => f.write_str("f64"),
            ValType::V128 => f.write_str("v128"),
            ValType::Ref(r) => write!(f, "(ref {}{})", if r.nullable { "null " } else { "" }, r.heap),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct FuncType {
    pub params: Box<[ValType]>,
    pub results: Box<[ValType]>,
}

impl FuncType {
    pub fn new(params: Vec<ValType>, results: Vec<ValType>) -> FuncType {
        FuncType { params: params.into(), results: results.into() }
    }
    pub fn param_slots(&self) -> u32 {
        self.params.iter().map(|t| t.slots()).sum()
    }
    pub fn result_slots(&self) -> u32 {
        self.results.iter().map(|t| t.slots()).sum()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StorageType {
    I8,
    I16,
    Val(ValType),
}

impl StorageType {
    pub fn unpacked(self) -> ValType {
        match self {
            StorageType::I8 | StorageType::I16 => ValType::I32,
            StorageType::Val(v) => v,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FieldType {
    pub storage: StorageType,
    pub mutable: bool,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum CompType {
    Func(FuncType),
    Struct(Box<[FieldType]>),
    Array(FieldType),
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct SubType {
    pub is_final: bool,
    pub supers: Box<[u32]>,
    pub comp: CompType,
}

impl SubType {
    pub fn func(ft: FuncType) -> SubType {
        SubType { is_final: true, supers: Box::new([]), comp: CompType::Func(ft) }
    }
    pub fn as_func(&self) -> Option<&FuncType> {
        match &self.comp {
            CompType::Func(f) => Some(f),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    pub min: u64,
    pub max: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MemoryType {
    pub limits: Limits,
    pub shared: bool,
    pub memory64: bool,
    /// log2 of the page size (16 = 64 KiB; custom-page-sizes allows 0).
    pub page_log2: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TableType {
    pub elem: RefType,
    pub limits: Limits,
    pub table64: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GlobalType {
    pub ty: ValType,
    pub mutable: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExternKind {
    Func,
    Table,
    Memory,
    Global,
    Tag,
}

impl ExternKind {
    pub fn name(self) -> &'static str {
        match self {
            ExternKind::Func => "function",
            ExternKind::Table => "table",
            ExternKind::Memory => "memory",
            ExternKind::Global => "global",
            ExternKind::Tag => "tag",
        }
    }
}

// ------------------------------------------------------------------ registry

/// Marks a type index relative to its own recursion group in canonical keys.
pub const REC_BIT: u32 = 0x8000_0000;

/// Canonical type id (process-wide).
pub type TypeId = u32;

struct Registry {
    groups: HashMap<Vec<SubType>, TypeId>,
    /// Canonical sub types; `Concrete` indices are canonical ids.
    types: Vec<SubType>,
    /// Depth in the declared subtype hierarchy (for GC casts).
    depth: Vec<u32>,
}

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);

fn with_registry<R>(f: impl FnOnce(&mut Registry) -> R) -> R {
    let mut g = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    let r = g.get_or_insert_with(|| Registry { groups: HashMap::new(), types: Vec::new(), depth: Vec::new() });
    f(r)
}

fn map_heap(h: HeapType, f: &impl Fn(u32) -> u32) -> HeapType {
    match h {
        HeapType::Concrete(i) => HeapType::Concrete(f(i)),
        o => o,
    }
}

fn map_val(v: ValType, f: &impl Fn(u32) -> u32) -> ValType {
    match v {
        ValType::Ref(r) => ValType::Ref(RefType { nullable: r.nullable, heap: map_heap(r.heap, f) }),
        o => o,
    }
}

fn map_sub(s: &SubType, f: &impl Fn(u32) -> u32) -> SubType {
    let field = |ft: &FieldType| FieldType {
        mutable: ft.mutable,
        storage: match ft.storage {
            StorageType::Val(v) => StorageType::Val(map_val(v, f)),
            o => o,
        },
    };
    SubType {
        is_final: s.is_final,
        supers: s.supers.iter().map(|&i| f(i)).collect(),
        comp: match &s.comp {
            CompType::Func(ft) => CompType::Func(FuncType {
                params: ft.params.iter().map(|&v| map_val(v, f)).collect(),
                results: ft.results.iter().map(|&v| map_val(v, f)).collect(),
            }),
            CompType::Struct(fs) => CompType::Struct(fs.iter().map(field).collect()),
            CompType::Array(ft) => CompType::Array(field(ft)),
        },
    }
}

/// Canonicalize the recursion groups of a module. `types` are the module's
/// sub types (module-relative indices), `groups` the (start, len) of each
/// recursion group in order. Returns the canonical id of every type index.
pub fn canonicalize(types: &[SubType], groups: &[(u32, u32)]) -> Vec<TypeId> {
    let mut ids: Vec<TypeId> = Vec::with_capacity(types.len());
    with_registry(|reg| {
        for &(start, len) in groups {
            let end = start + len;
            let known = ids.clone();
            let key: Vec<SubType> = (start..end)
                .map(|i| {
                    map_sub(&types[i as usize], &|x| {
                        if x >= start && x < end {
                            (x - start) | REC_BIT
                        } else {
                            known.get(x as usize).copied().unwrap_or(u32::MAX >> 1)
                        }
                    })
                })
                .collect();
            let base = match reg.groups.get(&key) {
                Some(&b) => b,
                None => {
                    let base = reg.types.len() as u32;
                    for st in &key {
                        let canon = map_sub(st, &|x| if x & REC_BIT != 0 { base + (x & !REC_BIT) } else { x });
                        let depth = canon.supers.first().map(|&s| reg.depth.get(s as usize).copied().unwrap_or(0) + 1).unwrap_or(0);
                        reg.types.push(canon);
                        reg.depth.push(depth);
                    }
                    reg.groups.insert(key, base);
                    base
                }
            };
            for i in 0..len {
                ids.push(base + i);
            }
        }
    });
    ids
}

/// The canonical sub type of a canonical id.
pub fn canonical_type(id: TypeId) -> SubType {
    with_registry(|r| r.types[id as usize].clone())
}

/// Is canonical type `a` a declared subtype (reflexive, transitive) of `b`?
pub fn canonical_subtype(mut a: TypeId, b: TypeId) -> bool {
    if a == b {
        return true;
    }
    with_registry(|r| loop {
        match r.types.get(a as usize).and_then(|t| t.supers.first()) {
            Some(&s) => {
                if s == b {
                    return true;
                }
                a = s;
            }
            None => return false,
        }
    })
}

/// Function type of a canonical id (panics if it is not a function type).
pub fn canonical_func_type(id: TypeId) -> FuncType {
    with_registry(|r| match &r.types[id as usize].comp {
        CompType::Func(f) => f.clone(),
        _ => FuncType::default(),
    })
}

/// Canonical id of a standalone function type (whose references are
/// canonical ids already), e.g. for host functions.
pub fn intern_func_type(ft: &FuncType) -> TypeId {
    let st = SubType::func(ft.clone());
    canonicalize_canonical(&st)
}

fn canonicalize_canonical(st: &SubType) -> TypeId {
    with_registry(|reg| {
        let key = vec![st.clone()];
        if let Some(&b) = reg.groups.get(&key) {
            return b;
        }
        let base = reg.types.len() as u32;
        reg.types.push(st.clone());
        reg.depth.push(0);
        reg.groups.insert(key, base);
        base
    })
}
