//! Module-level validation: types, imports, functions, tables, memories,
//! globals, tags, exports, start, element and data segments, constant
//! expressions. Function bodies are validated (and translated) by
//! [`crate::compile`].

use crate::binary::Result;
use crate::module::*;
use crate::types::*;
use crate::Error;
use std::collections::HashSet;

pub fn invalid<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::Invalid(msg.into()))
}

/// Subtyping and type lookups for one module.
pub struct TypeCtx<'a> {
    pub types: &'a [SubType],
    pub canon: &'a [TypeId],
}

impl<'a> TypeCtx<'a> {
    pub fn comp(&self, idx: u32) -> Option<&'a CompType> {
        self.types.get(idx as usize).map(|t| &t.comp)
    }
    fn is_func(&self, idx: u32) -> bool {
        matches!(self.comp(idx), Some(CompType::Func(_)))
    }
    fn is_struct(&self, idx: u32) -> bool {
        matches!(self.comp(idx), Some(CompType::Struct(_)))
    }
    fn is_array(&self, idx: u32) -> bool {
        matches!(self.comp(idx), Some(CompType::Array(_)))
    }

    /// The top of the hierarchy a heap type belongs to.
    pub fn top(&self, h: HeapType) -> HeapType {
        match h {
            HeapType::Func | HeapType::NoFunc => HeapType::Func,
            HeapType::Extern | HeapType::NoExtern => HeapType::Extern,
            HeapType::Exn | HeapType::NoExn => HeapType::Exn,
            HeapType::Concrete(i) => {
                if self.is_func(i) {
                    HeapType::Func
                } else {
                    HeapType::Any
                }
            }
            _ => HeapType::Any,
        }
    }

    /// The bottom of the hierarchy a heap type belongs to.
    pub fn bottom(&self, h: HeapType) -> HeapType {
        match self.top(h) {
            HeapType::Func => HeapType::NoFunc,
            HeapType::Extern => HeapType::NoExtern,
            HeapType::Exn => HeapType::NoExn,
            _ => HeapType::None,
        }
    }

    pub fn heap_sub(&self, a: HeapType, b: HeapType) -> bool {
        use HeapType::*;
        if a == b {
            return true;
        }
        match (a, b) {
            (Concrete(x), Concrete(y)) => {
                let (cx, cy) = (self.canon.get(x as usize), self.canon.get(y as usize));
                match (cx, cy) {
                    (Some(&cx), Some(&cy)) => canonical_subtype(cx, cy),
                    _ => false,
                }
            }
            (None, Any | Eq | I31 | Struct | Array) => true,
            (None, Concrete(y)) => !self.is_func(y),
            (NoFunc, Func) => true,
            (NoFunc, Concrete(y)) => self.is_func(y),
            (NoExtern, Extern) => true,
            (NoExn, Exn) => true,
            (I31 | Struct | Array, Any | Eq) => true,
            (Eq, Any) => true,
            (Concrete(x), Func) => self.is_func(x),
            (Concrete(x), Any | Eq) => self.is_struct(x) || self.is_array(x),
            (Concrete(x), Struct) => self.is_struct(x),
            (Concrete(x), Array) => self.is_array(x),
            _ => false,
        }
    }

    pub fn ref_sub(&self, a: RefType, b: RefType) -> bool {
        (!a.nullable || b.nullable) && self.heap_sub(a.heap, b.heap)
    }

    pub fn val_sub(&self, a: ValType, b: ValType) -> bool {
        match (a, b) {
            (ValType::Ref(x), ValType::Ref(y)) => self.ref_sub(x, y),
            _ => a == b,
        }
    }

    /// Type equality (canonical for concrete references).
    pub fn val_eq(&self, a: ValType, b: ValType) -> bool {
        self.val_sub(a, b) && self.val_sub(b, a)
    }

    pub fn func_type(&self, idx: u32) -> Option<&'a FuncType> {
        match self.comp(idx) {
            Some(CompType::Func(f)) => Some(f),
            _ => None,
        }
    }
}

pub fn check_heap(types_len: u32, h: HeapType) -> Result<()> {
    if let HeapType::Concrete(i) = h {
        if i >= types_len {
            return invalid(format!("unknown type {i}"));
        }
    }
    Ok(())
}

pub fn check_val(types_len: u32, v: ValType) -> Result<()> {
    if let ValType::Ref(r) = v {
        check_heap(types_len, r.heap)?;
    }
    Ok(())
}

fn check_limits(l: Limits, max: u64, what: &str) -> Result<()> {
    if l.min > max {
        return invalid(format!("{what} size must be at most {max}"));
    }
    if let Some(m) = l.max {
        if m > max {
            return invalid(format!("{what} size must be at most {max}"));
        }
        if m < l.min {
            return invalid("size minimum must not be greater than maximum");
        }
    }
    Ok(())
}

pub fn check_memory_type(t: &MemoryType) -> Result<()> {
    let max = if t.memory64 { if t.page_log2 == 0 { u64::MAX } else { 1u64 << (64 - t.page_log2 as u32) } } else { 1u64 << (32 - t.page_log2 as u32) };
    if t.page_log2 != 16 && t.page_log2 != 0 {
        return invalid("invalid custom page size");
    }
    check_limits(t.limits, max, "memory")?;
    if t.shared && t.limits.max.is_none() {
        return invalid("shared memory must have maximum");
    }
    Ok(())
}

pub fn check_table_type(types_len: u32, t: &TableType) -> Result<()> {
    check_heap(types_len, t.elem.heap)?;
    let max = if t.table64 { u64::MAX } else { u32::MAX as u64 };
    check_limits(t.limits, max, "table")
}

/// Validate everything outside function bodies, then translate the bodies.
pub fn validate(m: &mut Module) -> Result<Vec<crate::ir::Code>> {
    let ntypes = m.types.len() as u32;
    // Types: references must point backwards or into the own rec group.
    for &(start, len) in &m.rec_groups {
        let end = start + len;
        for i in start..end {
            let st = &m.types[i as usize];
            let check = |v: ValType| -> Result<()> {
                if let ValType::Ref(RefType { heap: HeapType::Concrete(x), .. }) = v {
                    if x >= end {
                        return invalid(format!("unknown type {x}"));
                    }
                }
                Ok(())
            };
            match &st.comp {
                CompType::Func(f) => {
                    for &v in f.params.iter().chain(f.results.iter()) {
                        check(v)?;
                    }
                }
                CompType::Struct(fs) => {
                    for f in fs.iter() {
                        if let StorageType::Val(v) = f.storage {
                            check(v)?;
                        }
                    }
                }
                CompType::Array(f) => {
                    if let StorageType::Val(v) = f.storage {
                        check(v)?;
                    }
                }
            }
            if st.supers.len() > 1 {
                return invalid("multiple supertypes");
            }
            for &s in st.supers.iter() {
                if s >= i {
                    return invalid(format!("unknown type {s}"));
                }
            }
        }
    }
    m.canon = canonicalize(&m.types, &m.rec_groups);
    // Subtype declarations must match.
    {
        let tc = TypeCtx { types: &m.types, canon: &m.canon };
        for (i, st) in m.types.iter().enumerate() {
            for &s in st.supers.iter() {
                let sup = &m.types[s as usize];
                if sup.is_final {
                    return invalid("sub type of final type");
                }
                if !comp_sub(&tc, &st.comp, &sup.comp) {
                    return invalid(format!("sub type {i} does not match super type {s}"));
                }
            }
        }
    }
    let tc = TypeCtx { types: &m.types, canon: &m.canon };

    // Imports
    for imp in &m.imports {
        match &imp.desc {
            ImportDesc::Func(t) => {
                if tc.func_type(*t).is_none() {
                    return invalid(format!("unknown type {t}"));
                }
            }
            ImportDesc::Table(t) => check_table_type(ntypes, t)?,
            ImportDesc::Memory(t) => check_memory_type(t)?,
            ImportDesc::Global(g) => check_val(ntypes, g.ty)?,
            ImportDesc::Tag(t) => check_tag_type(&tc, *t)?,
        }
    }
    for &t in &m.funcs[m.num_imported_funcs as usize..] {
        if tc.func_type(t).is_none() {
            return invalid(format!("unknown type {t}"));
        }
    }
    for &t in &m.tags[m.num_imported_tags as usize..] {
        check_tag_type(&tc, t)?;
    }

    // Declared function references (C.refs): every ref.func outside
    // function bodies.
    let mut refs: HashSet<u32> = HashSet::new();
    let collect = |e: &ConstExpr, refs: &mut HashSet<u32>| {
        for op in &e.ops {
            if let ConstOp::RefFunc(f) = op {
                refs.insert(*f);
            }
        }
    };
    for g in &m.global_inits {
        collect(g, &mut refs);
    }
    for t in m.table_inits.iter().flatten() {
        collect(t, &mut refs);
    }
    for e in &m.elems {
        for it in &e.items {
            collect(it, &mut refs);
        }
    }
    for ex in &m.exports {
        if ex.kind == ExternKind::Func {
            refs.insert(ex.index);
        }
    }

    let nfuncs = m.funcs.len() as u32;
    let ctx = ConstCtx { tc: &tc, m, nfuncs };

    // Tables
    for (i, t) in m.tables.iter().enumerate().skip(m.num_imported_tables as usize) {
        check_table_type(ntypes, t)?;
        let init = &m.table_inits[i - m.num_imported_tables as usize];
        match init {
            Some(e) => ctx.check(e, ValType::Ref(t.elem), m.num_imported_globals, t.table64)?,
            None => {
                if !t.elem.nullable {
                    return invalid("type mismatch: non-defaultable table type requires initializer");
                }
            }
        }
    }
    for t in m.memories.iter().skip(m.num_imported_memories as usize) {
        check_memory_type(t)?;
    }
    // Globals: initializers may refer to previous (immutable) globals.
    for (i, g) in m.globals.iter().enumerate().skip(m.num_imported_globals as usize) {
        check_val(ntypes, g.ty)?;
        let init = &m.global_inits[i - m.num_imported_globals as usize];
        ctx.check(init, g.ty, i as u32, false)?;
    }
    // Exports
    let mut names = HashSet::new();
    for ex in &m.exports {
        if !names.insert(ex.name.as_str()) {
            return invalid("duplicate export name");
        }
        let (n, what) = match ex.kind {
            ExternKind::Func => (m.funcs.len(), "function"),
            ExternKind::Table => (m.tables.len(), "table"),
            ExternKind::Memory => (m.memories.len(), "memory"),
            ExternKind::Global => (m.globals.len(), "global"),
            ExternKind::Tag => (m.tags.len(), "tag"),
        };
        if ex.index as usize >= n {
            return invalid(format!("unknown {what} {}", ex.index));
        }
    }
    if let Some(s) = m.start {
        if s >= nfuncs {
            return invalid(format!("unknown function {s}"));
        }
        let ft = m.func_type(s);
        if !ft.params.is_empty() || !ft.results.is_empty() {
            return invalid("start function must not have parameters or results");
        }
    }
    // Element segments
    for e in &m.elems {
        check_heap(ntypes, e.ty.heap)?;
        for it in &e.items {
            ctx.check(it, ValType::Ref(e.ty), m.globals.len() as u32, false)?;
        }
        if let ElemMode::Active { table, offset } = &e.mode {
            let Some(t) = m.tables.get(*table as usize) else {
                return invalid(format!("unknown table {table}"));
            };
            ctx.check(offset, if t.table64 { ValType::I64 } else { ValType::I32 }, m.globals.len() as u32, false)?;
            if !tc.ref_sub(e.ty, t.elem) {
                return invalid("type mismatch: element segment does not match table");
            }
        }
    }
    // Data segments
    for d in &m.datas {
        if let DataMode::Active { memory, offset } = &d.mode {
            let Some(mt) = m.memories.get(*memory as usize) else {
                return invalid(format!("unknown memory {memory}"));
            };
            ctx.check(offset, if mt.memory64 { ValType::I64 } else { ValType::I32 }, m.globals.len() as u32, false)?;
        }
    }

    // Function bodies
    let mut codes = Vec::with_capacity(m.bodies.len());
    let fctx = crate::compile::ModuleCtx { m, tc: &tc, refs: &refs };
    for i in 0..m.bodies.len() {
        codes.push(crate::compile::compile_function(&fctx, i as u32)?);
    }
    Ok(codes)
}

fn check_tag_type(tc: &TypeCtx, t: u32) -> Result<()> {
    match tc.func_type(t) {
        Some(f) => {
            if !f.results.is_empty() {
                return invalid("non-empty tag result type");
            }
            Ok(())
        }
        None => invalid(format!("unknown type {t}")),
    }
}

fn field_sub(tc: &TypeCtx, a: &FieldType, b: &FieldType) -> bool {
    if a.mutable != b.mutable {
        return false;
    }
    let st = |s: StorageType, t: StorageType, f: &dyn Fn(ValType, ValType) -> bool| match (s, t) {
        (StorageType::Val(x), StorageType::Val(y)) => f(x, y),
        (x, y) => x == y,
    };
    if a.mutable {
        st(a.storage, b.storage, &|x, y| tc.val_eq(x, y))
    } else {
        st(a.storage, b.storage, &|x, y| tc.val_sub(x, y))
    }
}

fn comp_sub(tc: &TypeCtx, a: &CompType, b: &CompType) -> bool {
    match (a, b) {
        (CompType::Func(f), CompType::Func(g)) => {
            f.params.len() == g.params.len()
                && f.results.len() == g.results.len()
                && f.params.iter().zip(g.params.iter()).all(|(x, y)| tc.val_sub(*y, *x))
                && f.results.iter().zip(g.results.iter()).all(|(x, y)| tc.val_sub(*x, *y))
        }
        (CompType::Struct(f), CompType::Struct(g)) => f.len() >= g.len() && f.iter().zip(g.iter()).all(|(x, y)| field_sub(tc, x, y)),
        (CompType::Array(f), CompType::Array(g)) => field_sub(tc, f, g),
        _ => false,
    }
}

struct ConstCtx<'a> {
    tc: &'a TypeCtx<'a>,
    m: &'a Module,
    nfuncs: u32,
}

impl ConstCtx<'_> {
    /// Check a constant expression producing `expected`; `globals_visible`
    /// limits which globals `global.get` may read.
    fn check(&self, e: &ConstExpr, expected: ValType, globals_visible: u32, _t64: bool) -> Result<()> {
        let tc = self.tc;
        let ntypes = self.m.types.len() as u32;
        let mut stack: Vec<ValType> = Vec::new();
        let pop = |stack: &mut Vec<ValType>, t: ValType| -> Result<()> {
            match stack.pop() {
                Some(v) if tc.val_sub(v, t) => Ok(()),
                _ => invalid("type mismatch"),
            }
        };
        for op in &e.ops {
            match *op {
                ConstOp::I32(_) => stack.push(ValType::I32),
                ConstOp::I64(_) => stack.push(ValType::I64),
                ConstOp::F32(_) => stack.push(ValType::F32),
                ConstOp::F64(_) => stack.push(ValType::F64),
                ConstOp::V128(_) => stack.push(ValType::V128),
                ConstOp::RefNull(h) => {
                    check_heap(ntypes, h)?;
                    stack.push(ValType::Ref(RefType { nullable: true, heap: h }));
                }
                ConstOp::RefFunc(f) => {
                    if f >= self.nfuncs {
                        return invalid(format!("unknown function {f}"));
                    }
                    stack.push(ValType::Ref(RefType { nullable: false, heap: HeapType::Concrete(self.m.funcs[f as usize]) }));
                }
                ConstOp::GlobalGet(g) => {
                    let Some(gt) = self.m.globals.get(g as usize) else {
                        return invalid(format!("unknown global {g}"));
                    };
                    if g >= globals_visible {
                        return invalid(format!("unknown global {g}"));
                    }
                    if gt.mutable {
                        return invalid("constant expression required");
                    }
                    stack.push(gt.ty);
                }
                ConstOp::I32Add | ConstOp::I32Sub | ConstOp::I32Mul => {
                    pop(&mut stack, ValType::I32)?;
                    pop(&mut stack, ValType::I32)?;
                    stack.push(ValType::I32);
                }
                ConstOp::I64Add | ConstOp::I64Sub | ConstOp::I64Mul => {
                    pop(&mut stack, ValType::I64)?;
                    pop(&mut stack, ValType::I64)?;
                    stack.push(ValType::I64);
                }
                ConstOp::RefI31 => {
                    pop(&mut stack, ValType::I32)?;
                    stack.push(ValType::Ref(RefType { nullable: false, heap: HeapType::I31 }));
                }
                ConstOp::StructNew(t) => {
                    let Some(CompType::Struct(fs)) = tc.comp(t) else { return invalid(format!("unknown type {t}")) };
                    for f in fs.iter().rev() {
                        pop(&mut stack, f.storage.unpacked())?;
                    }
                    stack.push(ValType::Ref(RefType { nullable: false, heap: HeapType::Concrete(t) }));
                }
                ConstOp::StructNewDefault(t) => {
                    let Some(CompType::Struct(fs)) = tc.comp(t) else { return invalid(format!("unknown type {t}")) };
                    if fs.iter().any(|f| !f.storage.unpacked().defaultable()) {
                        return invalid("type mismatch: field type is not defaultable");
                    }
                    stack.push(ValType::Ref(RefType { nullable: false, heap: HeapType::Concrete(t) }));
                }
                ConstOp::ArrayNew(t) => {
                    let Some(CompType::Array(f)) = tc.comp(t) else { return invalid(format!("unknown type {t}")) };
                    pop(&mut stack, ValType::I32)?;
                    pop(&mut stack, f.storage.unpacked())?;
                    stack.push(ValType::Ref(RefType { nullable: false, heap: HeapType::Concrete(t) }));
                }
                ConstOp::ArrayNewDefault(t) => {
                    let Some(CompType::Array(f)) = tc.comp(t) else { return invalid(format!("unknown type {t}")) };
                    if !f.storage.unpacked().defaultable() {
                        return invalid("type mismatch: array type is not defaultable");
                    }
                    pop(&mut stack, ValType::I32)?;
                    stack.push(ValType::Ref(RefType { nullable: false, heap: HeapType::Concrete(t) }));
                }
                ConstOp::ArrayNewFixed(t, n) => {
                    let Some(CompType::Array(f)) = tc.comp(t) else { return invalid(format!("unknown type {t}")) };
                    for _ in 0..n {
                        pop(&mut stack, f.storage.unpacked())?;
                    }
                    stack.push(ValType::Ref(RefType { nullable: false, heap: HeapType::Concrete(t) }));
                }
                ConstOp::AnyConvertExtern => {
                    let nullable = match stack.pop() {
                        Some(ValType::Ref(r)) if tc.heap_sub(r.heap, HeapType::Extern) => r.nullable,
                        _ => return invalid("type mismatch"),
                    };
                    stack.push(ValType::Ref(RefType { nullable, heap: HeapType::Any }));
                }
                ConstOp::ExternConvertAny => {
                    let nullable = match stack.pop() {
                        Some(ValType::Ref(r)) if tc.heap_sub(r.heap, HeapType::Any) => r.nullable,
                        _ => return invalid("type mismatch"),
                    };
                    stack.push(ValType::Ref(RefType { nullable, heap: HeapType::Extern }));
                }
            }
        }
        if stack.len() != 1 || !tc.val_sub(stack[0], expected) {
            return invalid("type mismatch");
        }
        Ok(())
    }
}
