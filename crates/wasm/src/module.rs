//! Decoded module representation.

use crate::types::*;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConstOp {
    I32(i32),
    I64(i64),
    F32(u32),
    F64(u64),
    V128(u128),
    RefNull(HeapType),
    RefFunc(u32),
    GlobalGet(u32),
    I32Add,
    I32Sub,
    I32Mul,
    I64Add,
    I64Sub,
    I64Mul,
    RefI31,
    StructNew(u32),
    StructNewDefault(u32),
    ArrayNew(u32),
    ArrayNewDefault(u32),
    ArrayNewFixed(u32, u32),
    AnyConvertExtern,
    ExternConvertAny,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ConstExpr {
    pub ops: Vec<ConstOp>,
}

#[derive(Clone, Debug)]
pub enum ImportDesc {
    Func(u32),
    Table(TableType),
    Memory(MemoryType),
    Global(GlobalType),
    Tag(u32),
}

impl ImportDesc {
    pub fn kind(&self) -> ExternKind {
        match self {
            ImportDesc::Func(_) => ExternKind::Func,
            ImportDesc::Table(_) => ExternKind::Table,
            ImportDesc::Memory(_) => ExternKind::Memory,
            ImportDesc::Global(_) => ExternKind::Global,
            ImportDesc::Tag(_) => ExternKind::Tag,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Import {
    pub module: String,
    pub name: String,
    pub desc: ImportDesc,
}

#[derive(Clone, Debug)]
pub struct Export {
    pub name: String,
    pub kind: ExternKind,
    pub index: u32,
}

#[derive(Clone, Debug)]
pub enum ElemMode {
    Active { table: u32, offset: ConstExpr },
    Passive,
    Declarative,
}

#[derive(Clone, Debug)]
pub struct ElemSegment {
    pub mode: ElemMode,
    pub ty: RefType,
    pub items: Vec<ConstExpr>,
}

#[derive(Clone, Debug)]
pub enum DataMode {
    Active { memory: u32, offset: ConstExpr },
    Passive,
}

#[derive(Clone, Debug)]
pub struct DataSegment {
    pub mode: DataMode,
    pub data: Range<usize>,
}

#[derive(Clone, Debug)]
pub struct Body {
    /// (count, type) runs as declared.
    pub locals: Vec<(u32, ValType)>,
    /// Byte range of the instruction sequence (after the locals).
    pub code: Range<usize>,
}

#[derive(Default, Debug)]
pub struct Module {
    pub bytes: Rc<[u8]>,
    pub types: Vec<SubType>,
    /// Recursion groups as (first type index, count).
    pub rec_groups: Vec<(u32, u32)>,
    /// Canonical id of every type index (filled by validation).
    pub canon: Vec<TypeId>,
    pub imports: Vec<Import>,
    /// Type index of every function (imports first).
    pub funcs: Vec<u32>,
    pub tables: Vec<TableType>,
    /// Initializer of each defined table (None: null of the element type).
    pub table_inits: Vec<Option<ConstExpr>>,
    pub memories: Vec<MemoryType>,
    pub globals: Vec<GlobalType>,
    pub global_inits: Vec<ConstExpr>,
    pub tags: Vec<u32>,
    pub exports: Vec<Export>,
    pub start: Option<u32>,
    pub elems: Vec<ElemSegment>,
    pub datas: Vec<DataSegment>,
    pub data_count: Option<u32>,
    pub bodies: Vec<Body>,
    pub customs: Vec<(String, Range<usize>)>,
    pub func_names: HashMap<u32, String>,
    pub num_imported_funcs: u32,
    pub num_imported_tables: u32,
    pub num_imported_memories: u32,
    pub num_imported_globals: u32,
    pub num_imported_tags: u32,
}

impl Module {
    pub fn func_type(&self, func: u32) -> &FuncType {
        let t = self.funcs[func as usize];
        self.types[t as usize].as_func().expect("function type")
    }
    pub fn type_func(&self, ty: u32) -> Option<&FuncType> {
        self.types.get(ty as usize).and_then(|t| t.as_func())
    }
    pub fn custom_sections<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a [u8]> + 'a {
        self.customs.iter().filter(move |(n, _)| n == name).map(move |(_, r)| &self.bytes[r.clone()])
    }
}
