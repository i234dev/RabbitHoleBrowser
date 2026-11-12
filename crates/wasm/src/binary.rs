//! Binary format decoder (all sections, including custom/name and data
//! count). Function bodies are only split here; their instructions are
//! decoded by the validator/compiler.

use crate::module::*;
use crate::types::*;
use crate::Error;
use std::rc::Rc;

pub type Result<T> = std::result::Result<T, Error>;

pub fn malformed<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::Malformed(msg.into()))
}

/// Byte reader with LEB128 decoding.
#[derive(Clone)]
pub struct Reader<'a> {
    pub data: &'a [u8],
    pub pos: usize,
    pub end: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Reader<'a> {
        Reader { data, pos: 0, end: data.len() }
    }
    pub fn sub(data: &'a [u8], start: usize, end: usize) -> Reader<'a> {
        Reader { data, pos: start, end }
    }
    #[inline]
    pub fn eof(&self) -> bool {
        self.pos >= self.end
    }
    #[inline]
    pub fn byte(&mut self) -> Result<u8> {
        if self.pos >= self.end {
            return malformed("unexpected end");
        }
        let b = self.data[self.pos];
        self.pos += 1;
        Ok(b)
    }
    pub fn peek(&self) -> Option<u8> {
        if self.pos < self.end {
            Some(self.data[self.pos])
        } else {
            None
        }
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.end - self.pos < n {
            return malformed("unexpected end of section or function");
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    #[inline]
    pub fn u32(&mut self) -> Result<u32> {
        let b = self.byte()?;
        if b < 0x80 {
            return Ok(b as u32);
        }
        let mut result = (b & 0x7f) as u32;
        let mut shift = 7;
        loop {
            let b = self.byte()?;
            if shift == 28 {
                if b & 0x80 != 0 {
                    return malformed("integer representation too long");
                }
                if b & 0x70 != 0 {
                    return malformed("integer too large");
                }
            }
            result |= ((b & 0x7f) as u32) << shift;
            if b & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
        }
    }
    pub fn u64(&mut self) -> Result<u64> {
        let mut result = 0u64;
        let mut shift = 0;
        loop {
            let b = self.byte()?;
            if shift == 63 {
                if b & 0x80 != 0 {
                    return malformed("integer representation too long");
                }
                if b & 0x7e != 0 {
                    return malformed("integer too large");
                }
            }
            result |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
        }
    }
    fn signed(&mut self, bits: u32) -> Result<i64> {
        let mut result = 0i64;
        let mut shift = 0u32;
        let max_bytes = bits.div_ceil(7);
        let mut n = 0;
        loop {
            let b = self.byte()?;
            n += 1;
            if n == max_bytes {
                if b & 0x80 != 0 {
                    return malformed("integer representation too long");
                }
                // Remaining bits must be a sign extension.
                let used = bits - shift; // value bits in this byte
                let mask = (0x7fu8 >> (used - 1)) << (used - 1) & 0x7f;
                let v = b & mask;
                if v != 0 && v != mask {
                    return malformed("integer too large");
                }
            }
            result |= ((b & 0x7f) as i64) << shift;
            shift += 7;
            if b & 0x80 == 0 {
                if shift < 64 && b & 0x40 != 0 {
                    result |= -1i64 << shift;
                }
                return Ok(result);
            }
        }
    }
    pub fn s32(&mut self) -> Result<i32> {
        let b = self.peek();
        if let Some(b) = b {
            if b < 0x40 {
                self.pos += 1;
                return Ok(b as i32);
            }
        }
        Ok(self.signed(32)? as i32)
    }
    pub fn s33(&mut self) -> Result<i64> {
        self.signed(33)
    }
    pub fn s64(&mut self) -> Result<i64> {
        self.signed(64)
    }
    pub fn f32(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub fn f64(&mut self) -> Result<u64> {
        let b = self.bytes(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_le_bytes(a))
    }
    pub fn v128(&mut self) -> Result<u128> {
        let b = self.bytes(16)?;
        let mut a = [0u8; 16];
        a.copy_from_slice(b);
        Ok(u128::from_le_bytes(a))
    }
    pub fn name(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        let b = self.bytes(n)?;
        match std::str::from_utf8(b) {
            Ok(s) => Ok(s.to_string()),
            Err(_) => malformed("malformed UTF-8 encoding"),
        }
    }

    // ------------------------------------------------------------- types

    pub fn heap_type(&mut self) -> Result<HeapType> {
        let v = self.s33()?;
        if v >= 0 {
            return Ok(HeapType::Concrete(v as u32));
        }
        if v & 0x7f == 0x62 {
            return Ok(HeapType::Exact(self.u32()?));
        }
        abstract_heap((v & 0x7f) as u8).map_or_else(|| malformed("malformed heap type"), Ok)
    }

    pub fn ref_type_from(&mut self, b: u8) -> Result<RefType> {
        match b {
            0x63 => Ok(RefType { nullable: true, heap: self.heap_type()? }),
            0x64 => Ok(RefType { nullable: false, heap: self.heap_type()? }),
            _ => match abstract_heap(b) {
                Some(h) => Ok(RefType { nullable: true, heap: h }),
                None => malformed("malformed reference type"),
            },
        }
    }

    pub fn ref_type(&mut self) -> Result<RefType> {
        let b = self.byte()?;
        self.ref_type_from(b)
    }

    pub fn val_type_from(&mut self, b: u8) -> Result<ValType> {
        Ok(match b {
            0x7f => ValType::I32,
            0x7e => ValType::I64,
            0x7d => ValType::F32,
            0x7c => ValType::F64,
            0x7b => ValType::V128,
            _ => match self.ref_type_from(b) {
                Ok(r) => ValType::Ref(r),
                Err(_) => return malformed("malformed value type"),
            },
        })
    }

    pub fn val_type(&mut self) -> Result<ValType> {
        let b = self.byte()?;
        self.val_type_from(b)
    }

    fn limits(&mut self, mem: bool) -> Result<(Limits, bool, bool, u8)> {
        let flags = self.byte()?;
        let max_flag = if mem { 0x0f } else { 0x07 };
        if flags & !max_flag != 0 {
            return malformed(if mem { "malformed memory limits flags" } else { "malformed table limits flags" });
        }
        let has_max = flags & 1 != 0;
        let shared = flags & 2 != 0;
        let is64 = flags & 4 != 0;
        let (min, max) = if is64 {
            let min = self.u64()?;
            let max = if has_max { Some(self.u64()?) } else { None };
            (min, max)
        } else {
            let min = self.u32()? as u64;
            let max = if has_max { Some(self.u32()? as u64) } else { None };
            (min, max)
        };
        let mut page_log2 = 16;
        if flags & 8 != 0 {
            let p = self.u32()?;
            if p > 16 {
                return malformed("invalid custom page size");
            }
            page_log2 = p as u8;
        }
        if !mem && shared {
            return malformed("malformed table limits flags");
        }
        Ok((Limits { min, max }, shared, is64, page_log2))
    }

    pub fn table_type(&mut self) -> Result<TableType> {
        let elem = self.ref_type()?;
        let (limits, _, table64, _) = self.limits(false)?;
        Ok(TableType { elem, limits, table64 })
    }

    pub fn memory_type(&mut self) -> Result<MemoryType> {
        let (limits, shared, memory64, page_log2) = self.limits(true)?;
        Ok(MemoryType { limits, shared, memory64, page_log2 })
    }

    pub fn global_type(&mut self) -> Result<GlobalType> {
        let ty = self.val_type()?;
        let m = self.byte()?;
        if m > 1 {
            return malformed("malformed mutability");
        }
        Ok(GlobalType { ty, mutable: m == 1 })
    }

    fn storage_type(&mut self) -> Result<StorageType> {
        let b = self.byte()?;
        Ok(match b {
            0x78 => StorageType::I8,
            0x77 => StorageType::I16,
            _ => StorageType::Val(self.val_type_from(b)?),
        })
    }

    fn field_type(&mut self) -> Result<FieldType> {
        let storage = self.storage_type()?;
        let m = self.byte()?;
        if m > 1 {
            return malformed("malformed mutability");
        }
        Ok(FieldType { storage, mutable: m == 1 })
    }

    fn comp_type(&mut self, b: u8) -> Result<CompType> {
        Ok(match b {
            0x60 => {
                let n = self.u32()?;
                let mut params = Vec::with_capacity(n.min(1024) as usize);
                for _ in 0..n {
                    params.push(self.val_type()?);
                }
                let n = self.u32()?;
                let mut results = Vec::with_capacity(n.min(1024) as usize);
                for _ in 0..n {
                    results.push(self.val_type()?);
                }
                CompType::Func(FuncType::new(params, results))
            }
            0x5f => {
                let n = self.u32()?;
                let mut fields = Vec::with_capacity(n.min(1024) as usize);
                for _ in 0..n {
                    fields.push(self.field_type()?);
                }
                CompType::Struct(fields.into())
            }
            0x5e => CompType::Array(self.field_type()?),
            _ => return malformed("malformed type"),
        })
    }

    fn sub_type(&mut self, b: u8) -> Result<SubType> {
        let (is_final, supers, mut c) = match b {
            0x50 | 0x4f => {
                let n = self.u32()?;
                let mut supers = Vec::new();
                for _ in 0..n {
                    supers.push(self.u32()?);
                }
                (b == 0x4f, supers, self.byte()?)
            }
            _ => (true, Vec::new(), b),
        };
        let mut describes = None;
        let mut descriptor = None;
        if c == 0x4c {
            describes = Some(self.u32()?);
            c = self.byte()?;
        }
        if c == 0x4d {
            descriptor = Some(self.u32()?);
            c = self.byte()?;
        }
        if c == 0x4c || c == 0x4d {
            return malformed("malformed definition type");
        }
        Ok(SubType { is_final, supers: supers.into(), describes, descriptor, comp: self.comp_type(c)? })
    }

    /// A constant expression up to and including its `end`.
    pub fn const_expr(&mut self) -> Result<ConstExpr> {
        let mut ops = Vec::new();
        loop {
            let op = self.byte()?;
            let c = match op {
                0x0b => return Ok(ConstExpr { ops }),
                0x41 => ConstOp::I32(self.s32()?),
                0x42 => ConstOp::I64(self.s64()?),
                0x43 => ConstOp::F32(self.f32()?),
                0x44 => ConstOp::F64(self.f64()?),
                0xd0 => ConstOp::RefNull(self.heap_type()?),
                0xd2 => ConstOp::RefFunc(self.u32()?),
                0x23 => ConstOp::GlobalGet(self.u32()?),
                0x6a => ConstOp::I32Add,
                0x6b => ConstOp::I32Sub,
                0x6c => ConstOp::I32Mul,
                0x7c => ConstOp::I64Add,
                0x7d => ConstOp::I64Sub,
                0x7e => ConstOp::I64Mul,
                0xfd => {
                    let sub = self.u32()?;
                    if sub != 12 {
                        return Err(Error::Invalid("constant expression required".into()));
                    }
                    ConstOp::V128(self.v128()?)
                }
                0xfb => {
                    let sub = self.u32()?;
                    match sub {
                        0 => ConstOp::StructNew(self.u32()?),
                        1 => ConstOp::StructNewDefault(self.u32()?),
                        32 => ConstOp::StructNewDesc(self.u32()?),
                        33 => ConstOp::StructNewDefaultDesc(self.u32()?),
                        6 => ConstOp::ArrayNew(self.u32()?),
                        7 => ConstOp::ArrayNewDefault(self.u32()?),
                        8 => {
                            let t = self.u32()?;
                            ConstOp::ArrayNewFixed(t, self.u32()?)
                        }
                        26 => ConstOp::AnyConvertExtern,
                        27 => ConstOp::ExternConvertAny,
                        28 => ConstOp::RefI31,
                        _ => return Err(Error::Invalid("constant expression required".into())),
                    }
                }
                _ => {
                    // Skip a known non-constant instruction as invalid; an
                    // unknown opcode is malformed.
                    if crate::compile::is_known_opcode(op) {
                        return Err(Error::Invalid("constant expression required".into()));
                    }
                    return malformed("illegal opcode");
                }
            };
            ops.push(c);
        }
    }
}

pub fn abstract_heap(b: u8) -> Option<HeapType> {
    Some(match b {
        0x70 => HeapType::Func,
        0x6f => HeapType::Extern,
        0x6e => HeapType::Any,
        0x6d => HeapType::Eq,
        0x6c => HeapType::I31,
        0x6b => HeapType::Struct,
        0x6a => HeapType::Array,
        0x69 => HeapType::Exn,
        0x71 => HeapType::None,
        0x72 => HeapType::NoExtern,
        0x73 => HeapType::NoFunc,
        0x74 => HeapType::NoExn,
        _ => return None,
    })
}

fn section_order(id: u8) -> u8 {
    // Position of each known section in the required order.
    match id {
        1 => 1,
        2 => 2,
        3 => 3,
        4 => 4,
        5 => 5,
        13 => 6,
        6 => 7,
        7 => 8,
        8 => 9,
        9 => 10,
        12 => 11,
        10 => 12,
        11 => 13,
        _ => 0,
    }
}

/// Maximum number of locals of one function (as engines limit it).
pub const MAX_LOCALS: u64 = 50_000;

/// Decode a binary module (structure only; see [`crate::validate`]).
pub fn decode(bytes: &[u8]) -> Result<Module> {
    let mut m = Module { bytes: Rc::from(bytes), ..Default::default() };
    let data: &[u8] = bytes;
    let mut r = Reader::new(data);
    let magic = r.bytes(4).or_else(|_| malformed("unexpected end"))?;
    if magic != b"\0asm" {
        return malformed("magic header not detected");
    }
    let version = r.bytes(4).or_else(|_| malformed("unexpected end"))?;
    if version != [1, 0, 0, 0] {
        return malformed("unknown binary version");
    }
    let mut last = 0u8;
    let mut func_count: Option<u32> = None;
    let mut code_seen = false;
    while !r.eof() {
        let id = r.byte()?;
        let size = r.u32()? as usize;
        if r.end - r.pos < size {
            return malformed("section size mismatch: unexpected end");
        }
        let start = r.pos;
        let end = start + size;
        let mut s = Reader::sub(data, start, end);
        if id != 0 {
            let ord = section_order(id);
            if ord == 0 {
                return malformed("malformed section id");
            }
            if ord <= last {
                return malformed("unexpected content after last section");
            }
            last = ord;
        }
        match id {
            0 => {
                let name = s.name()?;
                if name == "name" {
                    let _ = decode_names(&mut m, Reader::sub(data, s.pos, end));
                }
                m.customs.push((name, s.pos..end));
                s.pos = end;
            }
            1 => {
                let n = s.u32()?;
                for _ in 0..n {
                    let b = s.byte()?;
                    if b == 0x4e {
                        let k = s.u32()?;
                        let start = m.types.len() as u32;
                        for _ in 0..k {
                            let b = s.byte()?;
                            m.types.push(s.sub_type(b)?);
                        }
                        m.rec_groups.push((start, k));
                    } else {
                        let start = m.types.len() as u32;
                        m.types.push(s.sub_type(b)?);
                        m.rec_groups.push((start, 1));
                    }
                }
            }
            2 => {
                let n = s.u32()?;
                for _ in 0..n {
                    let module = s.name()?;
                    let name = s.name()?;
                    match (name.is_empty(), s.peek()) {
                        // Compact import section: one module, many items.
                        (true, Some(0x7f)) => {
                            s.byte()?;
                            let k = s.u32()?;
                            for _ in 0..k {
                                let name = s.name()?;
                                let desc = import_desc(&mut s)?;
                                add_import(&mut m, module.clone(), name, desc);
                            }
                        }
                        (true, Some(0x7e)) => {
                            s.byte()?;
                            let desc = import_desc(&mut s)?;
                            let k = s.u32()?;
                            for _ in 0..k {
                                let name = s.name()?;
                                add_import(&mut m, module.clone(), name, desc.clone());
                            }
                        }
                        _ => {
                            let desc = import_desc(&mut s)?;
                            add_import(&mut m, module, name, desc);
                        }
                    }
                }
            }
            3 => {
                let n = s.u32()?;
                func_count = Some(n);
                for _ in 0..n {
                    m.funcs.push(s.u32()?);
                }
            }
            4 => {
                let n = s.u32()?;
                for _ in 0..n {
                    if s.peek() == Some(0x40) {
                        s.byte()?;
                        if s.byte()? != 0 {
                            return malformed("malformed table");
                        }
                        let t = s.table_type()?;
                        let init = s.const_expr()?;
                        m.tables.push(t);
                        m.table_inits.push(Some(init));
                    } else {
                        m.tables.push(s.table_type()?);
                        m.table_inits.push(None);
                    }
                }
            }
            5 => {
                let n = s.u32()?;
                for _ in 0..n {
                    m.memories.push(s.memory_type()?);
                }
            }
            13 => {
                let n = s.u32()?;
                for _ in 0..n {
                    if s.byte()? != 0 {
                        return malformed("malformed tag attribute");
                    }
                    m.tags.push(s.u32()?);
                }
            }
            6 => {
                let n = s.u32()?;
                for _ in 0..n {
                    let t = s.global_type()?;
                    let init = s.const_expr()?;
                    m.globals.push(t);
                    m.global_inits.push(init);
                }
            }
            7 => {
                let n = s.u32()?;
                for _ in 0..n {
                    let name = s.name()?;
                    let kind = match s.byte()? {
                        0 => ExternKind::Func,
                        1 => ExternKind::Table,
                        2 => ExternKind::Memory,
                        3 => ExternKind::Global,
                        4 => ExternKind::Tag,
                        _ => return malformed("malformed export kind"),
                    };
                    let index = s.u32()?;
                    m.exports.push(Export { name, kind, index });
                }
            }
            8 => {
                m.start = Some(s.u32()?);
            }
            9 => {
                let n = s.u32()?;
                for _ in 0..n {
                    m.elems.push(decode_elem(&mut s)?);
                }
            }
            12 => {
                m.data_count = Some(s.u32()?);
            }
            10 => {
                code_seen = true;
                let n = s.u32()?;
                if n != func_count.unwrap_or(0) {
                    return malformed("function and code section have inconsistent lengths");
                }
                for _ in 0..n {
                    let size = s.u32()? as usize;
                    if s.end - s.pos < size {
                        return malformed("unexpected end of section or function");
                    }
                    let body_end = s.pos + size;
                    let mut b = Reader::sub(data, s.pos, body_end);
                    let groups = b.u32()?;
                    let mut locals = Vec::new();
                    let mut total: u64 = 0;
                    for _ in 0..groups {
                        let c = b.u32()?;
                        total += c as u64;
                        if total > MAX_LOCALS {
                            return malformed("too many locals");
                        }
                        let t = b.val_type()?;
                        locals.push((c, t));
                    }
                    m.bodies.push(Body { locals, code: b.pos..body_end });
                    s.pos = body_end;
                }
            }
            11 => {
                let n = s.u32()?;
                if let Some(c) = m.data_count {
                    if c != n {
                        return malformed("data count and data section have inconsistent lengths");
                    }
                }
                for _ in 0..n {
                    let flags = s.u32()?;
                    let mode = match flags {
                        0 => DataMode::Active { memory: 0, offset: s.const_expr()? },
                        1 => DataMode::Passive,
                        2 => {
                            let memory = s.u32()?;
                            DataMode::Active { memory, offset: s.const_expr()? }
                        }
                        _ => return malformed("malformed data segment flags"),
                    };
                    let len = s.u32()? as usize;
                    let p = s.pos;
                    s.bytes(len)?;
                    m.datas.push(DataSegment { mode, data: p..p + len });
                }
            }
            _ => unreachable!(),
        }
        if s.pos != end {
            return malformed("section size mismatch");
        }
        r.pos = end;
    }
    if func_count.unwrap_or(0) != 0 && !code_seen {
        return malformed("function and code section have inconsistent lengths");
    }
    if let Some(c) = m.data_count {
        if c != 0 && m.datas.is_empty() {
            return malformed("data count and data section have inconsistent lengths");
        }
    }
    Ok(m)
}

fn import_desc(s: &mut Reader) -> Result<(ImportDesc, bool)> {
    Ok((match s.byte()? {
        0 => ImportDesc::Func(s.u32()?),
        0x20 => return Ok((ImportDesc::Func(s.u32()?), true)),
        1 => ImportDesc::Table(s.table_type()?),
        2 => ImportDesc::Memory(s.memory_type()?),
        3 => ImportDesc::Global(s.global_type()?),
        4 => {
            if s.byte()? != 0 {
                return malformed("malformed tag attribute");
            }
            ImportDesc::Tag(s.u32()?)
        }
        _ => return malformed("malformed import kind"),
    }, false))
}

fn add_import(m: &mut Module, module: String, name: String, (desc, exact): (ImportDesc, bool)) {
    if exact {
        m.exact_funcs.push(m.funcs.len() as u32);
    }
    match &desc {
        ImportDesc::Func(t) => {
            m.funcs.push(*t);
            m.num_imported_funcs += 1;
        }
        ImportDesc::Table(t) => {
            m.tables.push(*t);
            m.num_imported_tables += 1;
        }
        ImportDesc::Memory(t) => {
            m.memories.push(*t);
            m.num_imported_memories += 1;
        }
        ImportDesc::Global(t) => {
            m.globals.push(*t);
            m.num_imported_globals += 1;
        }
        ImportDesc::Tag(t) => {
            m.tags.push(*t);
            m.num_imported_tags += 1;
        }
    }
    m.imports.push(Import { module, name, desc });
}

fn decode_elem(s: &mut Reader) -> Result<ElemSegment> {
    let flags = s.u32()?;
    if flags > 7 {
        return malformed("malformed elements segment kind");
    }
    let passive_or_decl = flags & 1 != 0;
    let explicit_table = flags & 2 != 0;
    let exprs = flags & 4 != 0;
    let mode = if !passive_or_decl {
        let table = if explicit_table { s.u32()? } else { 0 };
        ElemMode::Active { table, offset: s.const_expr()? }
    } else if explicit_table {
        ElemMode::Declarative
    } else {
        ElemMode::Passive
    };
    // Element kind / type
    let ty = if passive_or_decl || explicit_table {
        if exprs {
            s.ref_type()?
        } else {
            if s.byte()? != 0 {
                return malformed("malformed element kind");
            }
            RefType { nullable: false, heap: HeapType::Func }
        }
    } else if exprs {
        RefType::FUNCREF
    } else {
        RefType { nullable: false, heap: HeapType::Func }
    };
    let n = s.u32()?;
    let mut items = Vec::with_capacity(n.min(4096) as usize);
    for _ in 0..n {
        if exprs {
            items.push(s.const_expr()?);
        } else {
            items.push(ConstExpr { ops: vec![ConstOp::RefFunc(s.u32()?)] });
        }
    }
    Ok(ElemSegment { mode, ty, items })
}

fn decode_names(m: &mut Module, mut r: Reader) -> Result<()> {
    while !r.eof() {
        let id = r.byte()?;
        let size = r.u32()? as usize;
        let end = r.pos + size;
        if end > r.end {
            return malformed("bad name section");
        }
        if id == 1 {
            let mut s = Reader::sub(r.data, r.pos, end);
            let n = s.u32()?;
            for _ in 0..n {
                let idx = s.u32()?;
                let name = s.name()?;
                m.func_names.insert(idx, name);
            }
        }
        r.pos = end;
    }
    Ok(())
}
