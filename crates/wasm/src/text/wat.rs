//! WAT module encoder: text format module fields → binary module.

use super::num;
use super::{err, Sx};
use crate::types::*;
use crate::Error;
use std::collections::HashMap;

type R<T> = Result<T, Error>;

// ------------------------------------------------------------------ binary writing

pub fn leb_u32(out: &mut Vec<u8>, mut v: u32) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

pub fn leb_u64(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

pub fn leb_s64(out: &mut Vec<u8>, mut v: i64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if (v == 0 && b & 0x40 == 0) || (v == -1 && b & 0x40 != 0) {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn write_name(out: &mut Vec<u8>, s: &[u8]) {
    leb_u32(out, s.len() as u32);
    out.extend_from_slice(s);
}

fn heap_code(h: HeapType) -> Option<u8> {
    Some(match h {
        HeapType::Func => 0x70,
        HeapType::Extern => 0x6f,
        HeapType::Any => 0x6e,
        HeapType::Eq => 0x6d,
        HeapType::I31 => 0x6c,
        HeapType::Struct => 0x6b,
        HeapType::Array => 0x6a,
        HeapType::Exn => 0x69,
        HeapType::None => 0x71,
        HeapType::NoExtern => 0x72,
        HeapType::NoFunc => 0x73,
        HeapType::NoExn => 0x74,
        HeapType::Concrete(_) | HeapType::Exact(_) => return None,
    })
}

pub fn write_heap(out: &mut Vec<u8>, h: HeapType) {
    match heap_code(h) {
        Some(c) => out.push(c),
        None => {
            if let HeapType::Concrete(i) = h {
                leb_s64(out, i as i64);
            }
            if let HeapType::Exact(i) = h {
                out.push(0x62);
                leb_u32(out, i);
            }
        }
    }
}

pub fn write_ref(out: &mut Vec<u8>, r: RefType) {
    if r.nullable {
        if let Some(c) = heap_code(r.heap) {
            out.push(c);
            return;
        }
        out.push(0x63);
    } else {
        out.push(0x64);
    }
    write_heap(out, r.heap);
}

pub fn write_val(out: &mut Vec<u8>, v: ValType) {
    match v {
        ValType::I32 => out.push(0x7f),
        ValType::I64 => out.push(0x7e),
        ValType::F32 => out.push(0x7d),
        ValType::F64 => out.push(0x7c),
        ValType::V128 => out.push(0x7b),
        ValType::Ref(r) => write_ref(out, r),
    }
}

fn write_section(out: &mut Vec<u8>, id: u8, body: &[u8]) {
    out.push(id);
    leb_u32(out, body.len() as u32);
    out.extend_from_slice(body);
}

// ------------------------------------------------------------------ name spaces

#[derive(Default)]
struct Space {
    names: HashMap<String, u32>,
    count: u32,
}

impl Space {
    fn add(&mut self, name: Option<&str>, what: &str) -> R<u32> {
        let idx = self.count;
        self.count += 1;
        if let Some(n) = name {
            if self.names.insert(n.to_string(), idx).is_some() {
                return err(format!("duplicate {what} {n}"));
            }
        }
        Ok(idx)
    }
    fn resolve(&self, sx: &Sx, what: &str) -> R<u32> {
        match sx {
            Sx::Id(n) => self.names.get(n).copied().ok_or_else(|| Error::Malformed(format!("unknown {what} ${n}"))),
            Sx::Atom(a) => num::parse_u32(a).ok_or_else(|| Error::Malformed(format!("unknown {what} {a}"))),
            _ => err(format!("expected {what} index")),
        }
    }
}

fn is_index(sx: &Sx) -> bool {
    match sx {
        Sx::Id(_) => true,
        Sx::Atom(a) => a.starts_with(|c: char| c.is_ascii_digit()),
        _ => false,
    }
}

fn opt_id(items: &[Sx], i: &mut usize) -> Option<String> {
    if let Some(Sx::Id(n)) = items.get(*i) {
        *i += 1;
        Some(n.clone())
    } else {
        None
    }
}

fn string(sx: &Sx) -> R<Vec<u8>> {
    match sx {
        Sx::Str(s) => Ok(s.clone()),
        _ => err("expected string"),
    }
}

fn utf8_name(sx: &Sx) -> R<Vec<u8>> {
    let s = string(sx)?;
    if std::str::from_utf8(&s).is_err() {
        return err("malformed UTF-8 encoding");
    }
    Ok(s)
}

// ------------------------------------------------------------------ module context

#[derive(Clone, Copy, PartialEq)]
enum ImportKind {
    Func,
    Table,
    Memory,
    Global,
    Tag,
}

struct Ctx {
    types: Vec<SubType>,
    rec_groups: Vec<(u32, u32)>,
    type_names: Space,
    field_names: Vec<HashMap<String, u32>>,
    funcs: Space,
    tables: Space,
    mems: Space,
    globals: Space,
    tags: Space,
    elems: Space,
    datas: Space,
    func_types: Vec<u32>,
    tag_types: Vec<u32>,
    table64: Vec<bool>,
    mem64: Vec<bool>,
}

impl Ctx {
    fn heap(&self, sx: &Sx) -> R<HeapType> {
        if let Sx::Atom(a) = sx {
            let h = match a.as_str() {
                "func" => HeapType::Func,
                "extern" => HeapType::Extern,
                "any" => HeapType::Any,
                "eq" => HeapType::Eq,
                "i31" => HeapType::I31,
                "struct" => HeapType::Struct,
                "array" => HeapType::Array,
                "exn" => HeapType::Exn,
                "none" => HeapType::None,
                "nofunc" => HeapType::NoFunc,
                "noextern" => HeapType::NoExtern,
                "noexn" => HeapType::NoExn,
                _ => return Ok(HeapType::Concrete(self.type_names.resolve(sx, "type")?)),
            };
            return Ok(h);
        }
        if sx.is_list_of("exact") {
            let l = sx.list().unwrap();
            if l.len() != 2 {
                return err("malformed exact heap type");
            }
            return Ok(HeapType::Exact(self.type_names.resolve(&l[1], "type")?));
        }
        Ok(HeapType::Concrete(self.type_names.resolve(sx, "type")?))
    }

    fn ref_type(&self, sx: &Sx) -> R<Option<RefType>> {
        match sx {
            Sx::Atom(a) => {
                let h = match a.as_str() {
                    "funcref" => HeapType::Func,
                    "externref" => HeapType::Extern,
                    "anyref" => HeapType::Any,
                    "eqref" => HeapType::Eq,
                    "i31ref" => HeapType::I31,
                    "structref" => HeapType::Struct,
                    "arrayref" => HeapType::Array,
                    "exnref" => HeapType::Exn,
                    "nullref" => HeapType::None,
                    "nullfuncref" => HeapType::NoFunc,
                    "nullexternref" => HeapType::NoExtern,
                    "nullexnref" => HeapType::NoExn,
                    _ => return Ok(None),
                };
                Ok(Some(RefType { nullable: true, heap: h }))
            }
            Sx::List(l) if l.first().and_then(|h| h.atom()) == Some("ref") => {
                let mut i = 1;
                let nullable = matches!(l.get(1), Some(Sx::Atom(a)) if a == "null");
                if nullable {
                    i = 2;
                }
                let Some(h) = l.get(i) else { return err("expected heap type") };
                if l.len() != i + 1 {
                    return err("unexpected token in reference type");
                }
                Ok(Some(RefType { nullable, heap: self.heap(h)? }))
            }
            _ => Ok(None),
        }
    }

    fn val_type(&self, sx: &Sx) -> R<ValType> {
        if let Sx::Atom(a) = sx {
            match a.as_str() {
                "i32" => return Ok(ValType::I32),
                "i64" => return Ok(ValType::I64),
                "f32" => return Ok(ValType::F32),
                "f64" => return Ok(ValType::F64),
                "v128" => return Ok(ValType::V128),
                _ => {}
            }
        }
        match self.ref_type(sx)? {
            Some(r) => Ok(ValType::Ref(r)),
            None => err(format!("unexpected token, expected value type: {sx:?}")),
        }
    }

    fn storage_type(&self, sx: &Sx) -> R<StorageType> {
        match sx.atom() {
            Some("i8") => Ok(StorageType::I8),
            Some("i16") => Ok(StorageType::I16),
            _ => Ok(StorageType::Val(self.val_type(sx)?)),
        }
    }

    fn field_type(&self, sx: &Sx) -> R<FieldType> {
        if sx.is_list_of("mut") {
            let l = sx.list().unwrap();
            if l.len() != 2 {
                return err("malformed mutable field");
            }
            return Ok(FieldType { storage: self.storage_type(&l[1])?, mutable: true });
        }
        Ok(FieldType { storage: self.storage_type(sx)?, mutable: false })
    }

    /// Find or append a plain function type.
    fn func_type_index(&mut self, params: &[ValType], results: &[ValType]) -> u32 {
        for (i, t) in self.types.iter().enumerate() {
            if let CompType::Func(f) = &t.comp {
                if t.is_final && t.supers.is_empty() && *f.params == *params && *f.results == *results {
                    // Only singleton recursion groups.
                    if self.rec_groups.iter().any(|&(s, n)| s == i as u32 && n == 1) {
                        return i as u32;
                    }
                }
            }
        }
        let idx = self.types.len() as u32;
        self.types.push(SubType::func(FuncType::new(params.to_vec(), results.to_vec())));
        self.rec_groups.push((idx, 1));
        self.field_names.push(HashMap::new());
        idx
    }

    /// Parse `(type x)? (param ...)* (result ...)*` at `items[*i..]`.
    /// Returns the type index and named params.
    fn type_use(&mut self, items: &[Sx], i: &mut usize, allow_names: bool) -> R<(u32, Vec<Option<String>>)> {
        let mut explicit = None;
        if let Some(s) = items.get(*i) {
            if s.is_list_of("type") {
                let l = s.list().unwrap();
                if l.len() != 2 {
                    return err("malformed type use");
                }
                explicit = Some(self.type_names.resolve(&l[1], "type")?);
                *i += 1;
            }
        }
        let (params, names, results, had_inline) = self.params_results(items, i, allow_names)?;
        match explicit {
            Some(t) => {
                if had_inline {
                    let ok = match self.types.get(t as usize).and_then(|st| st.as_func()) {
                        Some(f) => *f.params == *params && *f.results == *results,
                        None => false,
                    };
                    if !ok {
                        return err("inline function type");
                    }
                    Ok((t, names))
                } else {
                    let n = self.types.get(t as usize).and_then(|st| st.as_func()).map(|f| f.params.len()).unwrap_or(0);
                    Ok((t, vec![None; n]))
                }
            }
            None => Ok((self.func_type_index(&params, &results), names)),
        }
    }

    #[allow(clippy::type_complexity)]
    fn params_results(&self, items: &[Sx], i: &mut usize, allow_names: bool) -> R<(Vec<ValType>, Vec<Option<String>>, Vec<ValType>, bool)> {
        let mut params = Vec::new();
        let mut names = Vec::new();
        let mut results = Vec::new();
        let mut had = false;
        while let Some(s) = items.get(*i) {
            if s.is_list_of("param") {
                if !results.is_empty() {
                    return err("unexpected token: param after result");
                }
                let l = s.list().unwrap();
                if let Some(Sx::Id(n)) = l.get(1) {
                    if !allow_names {
                        return err("unexpected token: named parameter");
                    }
                    if l.len() != 3 {
                        return err("malformed named parameter");
                    }
                    params.push(self.val_type(&l[2])?);
                    names.push(Some(n.clone()));
                } else {
                    for t in &l[1..] {
                        params.push(self.val_type(t)?);
                        names.push(None);
                    }
                }
                had = true;
                *i += 1;
            } else if s.is_list_of("result") {
                let l = s.list().unwrap();
                for t in &l[1..] {
                    results.push(self.val_type(t)?);
                }
                had = true;
                *i += 1;
            } else {
                break;
            }
        }
        Ok((params, names, results, had))
    }
}

// ------------------------------------------------------------------ instruction table

#[derive(Clone, Copy, PartialEq, Debug)]
enum Imm {
    Nil,
    Label,
    BrTable,
    Func,
    CallIndirect,
    Type,
    TypeField,
    TypeN,
    TypeData,
    TypeElem,
    TypeType,
    Local,
    Global,
    Table,
    TableCopy,
    TableInit,
    Elem,
    Data,
    MemInit,
    Mem,
    MemCopy,
    MemArg(u32),
    I32,
    I64,
    F32,
    F64,
    V128,
    HeapType,
    Select,
    Tag,
    RefTypeImm,
    BrCast,
    Lane,
    MemArgLane(u32),
    Shuffle,
    Zero,
}

/// (prefix, opcode, immediates)
fn lookup(name: &str) -> Option<(Option<u8>, u32, Imm)> {
    use Imm::*;
    let (p, c, i): (Option<u8>, u32, Imm) = match name {
        "unreachable" => (None, 0x00, Nil),
        "nop" => (None, 0x01, Nil),
        "br" => (None, 0x0c, Label),
        "br_if" => (None, 0x0d, Label),
        "br_table" => (None, 0x0e, BrTable),
        "return" => (None, 0x0f, Nil),
        "call" => (None, 0x10, Func),
        "call_indirect" => (None, 0x11, CallIndirect),
        "return_call" => (None, 0x12, Func),
        "return_call_indirect" => (None, 0x13, CallIndirect),
        "call_ref" => (None, 0x14, Type),
        "return_call_ref" => (None, 0x15, Type),
        "throw" => (None, 0x08, Tag),
        "rethrow" => (None, 0x09, Label),
        "throw_ref" => (None, 0x0a, Nil),
        "drop" => (None, 0x1a, Nil),
        "select" => (None, 0x1b, Select),
        "local.get" => (None, 0x20, Local),
        "local.set" => (None, 0x21, Local),
        "local.tee" => (None, 0x22, Local),
        "global.get" => (None, 0x23, Global),
        "global.set" => (None, 0x24, Global),
        "table.get" => (None, 0x25, Table),
        "table.set" => (None, 0x26, Table),
        "i32.load" => (None, 0x28, MemArg(4)),
        "i64.load" => (None, 0x29, MemArg(8)),
        "f32.load" => (None, 0x2a, MemArg(4)),
        "f64.load" => (None, 0x2b, MemArg(8)),
        "i32.load8_s" => (None, 0x2c, MemArg(1)),
        "i32.load8_u" => (None, 0x2d, MemArg(1)),
        "i32.load16_s" => (None, 0x2e, MemArg(2)),
        "i32.load16_u" => (None, 0x2f, MemArg(2)),
        "i64.load8_s" => (None, 0x30, MemArg(1)),
        "i64.load8_u" => (None, 0x31, MemArg(1)),
        "i64.load16_s" => (None, 0x32, MemArg(2)),
        "i64.load16_u" => (None, 0x33, MemArg(2)),
        "i64.load32_s" => (None, 0x34, MemArg(4)),
        "i64.load32_u" => (None, 0x35, MemArg(4)),
        "i32.store" => (None, 0x36, MemArg(4)),
        "i64.store" => (None, 0x37, MemArg(8)),
        "f32.store" => (None, 0x38, MemArg(4)),
        "f64.store" => (None, 0x39, MemArg(8)),
        "i32.store8" => (None, 0x3a, MemArg(1)),
        "i32.store16" => (None, 0x3b, MemArg(2)),
        "i64.store8" => (None, 0x3c, MemArg(1)),
        "i64.store16" => (None, 0x3d, MemArg(2)),
        "i64.store32" => (None, 0x3e, MemArg(4)),
        "memory.size" => (None, 0x3f, Mem),
        "memory.grow" => (None, 0x40, Mem),
        "i32.const" => (None, 0x41, I32),
        "i64.const" => (None, 0x42, I64),
        "f32.const" => (None, 0x43, F32),
        "f64.const" => (None, 0x44, F64),
        "ref.null" => (None, 0xd0, HeapType),
        "ref.is_null" => (None, 0xd1, Nil),
        "ref.func" => (None, 0xd2, Func),
        "ref.eq" => (None, 0xd3, Nil),
        "ref.as_non_null" => (None, 0xd4, Nil),
        "br_on_null" => (None, 0xd5, Label),
        "br_on_non_null" => (None, 0xd6, Label),
        "i32.trunc_sat_f32_s" => (Some(0xfc), 0, Nil),
        "i32.trunc_sat_f32_u" => (Some(0xfc), 1, Nil),
        "i32.trunc_sat_f64_s" => (Some(0xfc), 2, Nil),
        "i32.trunc_sat_f64_u" => (Some(0xfc), 3, Nil),
        "i64.trunc_sat_f32_s" => (Some(0xfc), 4, Nil),
        "i64.trunc_sat_f32_u" => (Some(0xfc), 5, Nil),
        "i64.trunc_sat_f64_s" => (Some(0xfc), 6, Nil),
        "i64.trunc_sat_f64_u" => (Some(0xfc), 7, Nil),
        "memory.init" => (Some(0xfc), 8, MemInit),
        "data.drop" => (Some(0xfc), 9, Data),
        "memory.copy" => (Some(0xfc), 10, MemCopy),
        "memory.fill" => (Some(0xfc), 11, Mem),
        "table.init" => (Some(0xfc), 12, TableInit),
        "elem.drop" => (Some(0xfc), 13, Elem),
        "i64.add128" => (Some(0xfc), 19, Nil),
        "i64.sub128" => (Some(0xfc), 20, Nil),
        "i64.mul_wide_s" => (Some(0xfc), 21, Nil),
        "i64.mul_wide_u" => (Some(0xfc), 22, Nil),
        "table.copy" => (Some(0xfc), 14, TableCopy),
        "table.grow" => (Some(0xfc), 15, Table),
        "table.size" => (Some(0xfc), 16, Table),
        "table.fill" => (Some(0xfc), 17, Table),
        "struct.new" => (Some(0xfb), 0, Type),
        "struct.new_default" => (Some(0xfb), 1, Type),
        "struct.new_desc" => (Some(0xfb), 32, Type),
        "struct.new_default_desc" => (Some(0xfb), 33, Type),
        "ref.get_desc" => (Some(0xfb), 34, Type),
        "ref.cast_desc_eq" => (Some(0xfb), 35, RefTypeImm),
        "br_on_cast_desc_eq" => (Some(0xfb), 37, BrCast),
        "br_on_cast_desc_eq_fail" => (Some(0xfb), 38, BrCast),
        "struct.get" => (Some(0xfb), 2, TypeField),
        "struct.get_s" => (Some(0xfb), 3, TypeField),
        "struct.get_u" => (Some(0xfb), 4, TypeField),
        "struct.set" => (Some(0xfb), 5, TypeField),
        "array.new" => (Some(0xfb), 6, Type),
        "array.new_default" => (Some(0xfb), 7, Type),
        "array.new_fixed" => (Some(0xfb), 8, TypeN),
        "array.new_data" => (Some(0xfb), 9, TypeData),
        "array.new_elem" => (Some(0xfb), 10, TypeElem),
        "array.get" => (Some(0xfb), 11, Type),
        "array.get_s" => (Some(0xfb), 12, Type),
        "array.get_u" => (Some(0xfb), 13, Type),
        "array.set" => (Some(0xfb), 14, Type),
        "array.len" => (Some(0xfb), 15, Nil),
        "array.fill" => (Some(0xfb), 16, Type),
        "array.copy" => (Some(0xfb), 17, TypeType),
        "array.init_data" => (Some(0xfb), 18, TypeData),
        "array.init_elem" => (Some(0xfb), 19, TypeElem),
        "ref.test" => (Some(0xfb), 20, RefTypeImm),
        "ref.cast" => (Some(0xfb), 22, RefTypeImm),
        "br_on_cast" => (Some(0xfb), 24, BrCast),
        "br_on_cast_fail" => (Some(0xfb), 25, BrCast),
        "any.convert_extern" => (Some(0xfb), 26, Nil),
        "extern.convert_any" => (Some(0xfb), 27, Nil),
        "ref.i31" => (Some(0xfb), 28, Nil),
        "i31.get_s" => (Some(0xfb), 29, Nil),
        "i31.get_u" => (Some(0xfb), 30, Nil),
        _ => {
            if let Some(c) = numeric_opcode(name) {
                return Some((Option::None, c as u32, Nil));
            }
            if let Some(c) = crate::atomics::atomic_opcode(name) {
                let imm = if c == 3 { Zero } else { MemArg(crate::atomics::natural(c)) };
                return Some((Some(0xfe), c, imm));
            }
            if let Some(&(_, c)) = crate::simd::SIMD_NAMES.iter().find(|(n, _)| *n == name) {
                let imm = match c {
                    0..=11 | 92 | 93 => MemArg(simd_natural(c)),
                    12 => V128,
                    13 => Shuffle,
                    21..=34 => Lane,
                    84..=91 => MemArgLane(simd_natural(c)),
                    _ => Nil,
                };
                return Some((Some(0xfd), c, imm));
            }
            return Option::None;
        }
    };
    Some((p, c, i))
}

fn simd_natural(c: u32) -> u32 {
    match c {
        0 | 11 => 16,
        1..=6 | 10 | 93 => 8,
        7 | 84 | 88 => 1,
        8 | 85 | 89 => 2,
        9 | 86 | 90 | 92 => 4,
        87 | 91 => 8,
        _ => 16,
    }
}

const NUMERIC: &[&str] = &[
    "i32.eqz", "i32.eq", "i32.ne", "i32.lt_s", "i32.lt_u", "i32.gt_s", "i32.gt_u", "i32.le_s", "i32.le_u", "i32.ge_s", "i32.ge_u", "i64.eqz", "i64.eq", "i64.ne", "i64.lt_s", "i64.lt_u",
    "i64.gt_s", "i64.gt_u", "i64.le_s", "i64.le_u", "i64.ge_s", "i64.ge_u", "f32.eq", "f32.ne", "f32.lt", "f32.gt", "f32.le", "f32.ge", "f64.eq", "f64.ne", "f64.lt", "f64.gt", "f64.le",
    "f64.ge", "i32.clz", "i32.ctz", "i32.popcnt", "i32.add", "i32.sub", "i32.mul", "i32.div_s", "i32.div_u", "i32.rem_s", "i32.rem_u", "i32.and", "i32.or", "i32.xor", "i32.shl", "i32.shr_s",
    "i32.shr_u", "i32.rotl", "i32.rotr", "i64.clz", "i64.ctz", "i64.popcnt", "i64.add", "i64.sub", "i64.mul", "i64.div_s", "i64.div_u", "i64.rem_s", "i64.rem_u", "i64.and", "i64.or",
    "i64.xor", "i64.shl", "i64.shr_s", "i64.shr_u", "i64.rotl", "i64.rotr", "f32.abs", "f32.neg", "f32.ceil", "f32.floor", "f32.trunc", "f32.nearest", "f32.sqrt", "f32.add", "f32.sub",
    "f32.mul", "f32.div", "f32.min", "f32.max", "f32.copysign", "f64.abs", "f64.neg", "f64.ceil", "f64.floor", "f64.trunc", "f64.nearest", "f64.sqrt", "f64.add", "f64.sub", "f64.mul",
    "f64.div", "f64.min", "f64.max", "f64.copysign", "i32.wrap_i64", "i32.trunc_f32_s", "i32.trunc_f32_u", "i32.trunc_f64_s", "i32.trunc_f64_u", "i64.extend_i32_s", "i64.extend_i32_u",
    "i64.trunc_f32_s", "i64.trunc_f32_u", "i64.trunc_f64_s", "i64.trunc_f64_u", "f32.convert_i32_s", "f32.convert_i32_u", "f32.convert_i64_s", "f32.convert_i64_u", "f32.demote_f64",
    "f64.convert_i32_s", "f64.convert_i32_u", "f64.convert_i64_s", "f64.convert_i64_u", "f64.promote_f32", "i32.reinterpret_f32", "i64.reinterpret_f64", "f32.reinterpret_i32",
    "f64.reinterpret_i64", "i32.extend8_s", "i32.extend16_s", "i64.extend8_s", "i64.extend16_s", "i64.extend32_s",
];

fn numeric_opcode(name: &str) -> Option<u8> {
    NUMERIC.iter().position(|n| *n == name).map(|p| 0x45 + p as u8)
}

// ------------------------------------------------------------------ function body encoder

struct Body<'c> {
    cx: &'c mut Ctx,
    locals: HashMap<String, u32>,
    labels: Vec<Option<String>>,
    out: Vec<u8>,
}

impl Body<'_> {
    fn label(&self, sx: &Sx) -> R<u32> {
        match sx {
            Sx::Id(n) => {
                for (d, l) in self.labels.iter().rev().enumerate() {
                    if l.as_deref() == Some(n.as_str()) {
                        return Ok(d as u32);
                    }
                }
                err(format!("unknown label ${n}"))
            }
            Sx::Atom(a) => num::parse_u32(a).ok_or_else(|| Error::Malformed("unknown label".into())),
            _ => err("expected label"),
        }
    }

    fn local(&self, sx: &Sx) -> R<u32> {
        match sx {
            Sx::Id(n) => self.locals.get(n).copied().ok_or_else(|| Error::Malformed(format!("unknown local ${n}"))),
            Sx::Atom(a) => num::parse_u32(a).ok_or_else(|| Error::Malformed("unknown local".into())),
            _ => err("expected local"),
        }
    }

    /// Block type at items[*i..].
    fn block_type(&mut self, items: &[Sx], i: &mut usize) -> R<()> {
        let has_type = items.get(*i).is_some_and(|s| s.is_list_of("type"));
        if !has_type {
            let save = *i;
            let (params, _, results, _) = self.cx.params_results(items, i, false)?;
            if params.is_empty() && results.len() <= 1 {
                match results.first() {
                    None => self.out.push(0x40),
                    Some(&t) => write_val(&mut self.out, t),
                }
                return Ok(());
            }
            *i = save;
        }
        let (t, _) = self.cx.type_use(items, i, false)?;
        leb_s64(&mut self.out, t as i64);
        Ok(())
    }

    /// A sequence of instructions (plain and folded).
    fn instrs(&mut self, items: &[Sx]) -> R<()> {
        let mut i = 0;
        while i < items.len() {
            self.instr(items, &mut i)?;
        }
        Ok(())
    }

    fn end_label(&mut self, items: &[Sx], i: &mut usize, label: &Option<String>) -> R<()> {
        if let Some(Sx::Id(n)) = items.get(*i) {
            if label.as_deref() != Some(n.as_str()) {
                return err("mismatching label");
            }
            *i += 1;
        }
        Ok(())
    }

    /// One plain instruction at items[*i] (or a folded one if a list).
    fn instr(&mut self, items: &[Sx], i: &mut usize) -> R<()> {
        let s = &items[*i];
        if let Sx::List(l) = s {
            *i += 1;
            return self.folded(l);
        }
        let Some(name) = s.atom() else { return err("unexpected token") };
        *i += 1;
        match name {
            "block" | "loop" | "if" | "try_table" | "try" => {
                let label = opt_id(items, i);
                let code = match name {
                    "block" => 0x02,
                    "loop" => 0x03,
                    "if" => 0x04,
                    "try" => 0x06,
                    _ => 0x1f,
                };
                self.out.push(code);
                self.block_type(items, i)?;
                if name == "try_table" {
                    self.catches(items, i)?;
                }
                self.labels.push(label);
                Ok(())
            }
            "else" => {
                let label = self.labels.last().cloned().flatten();
                self.end_label(items, i, &label)?;
                self.out.push(0x05);
                Ok(())
            }
            "catch" => {
                let label = self.labels.last().cloned().flatten();
                let _ = label;
                self.out.push(0x07);
                let Some(t) = items.get(*i) else { return err("expected tag") };
                let t = self.cx.tags.resolve(t, "tag")?;
                *i += 1;
                leb_u32(&mut self.out, t);
                Ok(())
            }
            "catch_all" => {
                self.out.push(0x19);
                Ok(())
            }
            "delegate" => {
                self.labels.pop();
                self.out.push(0x18);
                let Some(l) = items.get(*i) else { return err("expected label") };
                let d = self.label(l)?;
                *i += 1;
                leb_u32(&mut self.out, d);
                Ok(())
            }
            "end" => {
                let label = self.labels.pop().flatten();
                self.end_label(items, i, &label)?;
                self.out.push(0x0b);
                Ok(())
            }
            _ => self.plain(name, items, i),
        }
    }

    fn catches(&mut self, items: &[Sx], i: &mut usize) -> R<()> {
        let mut n = 0;
        let mut buf = Vec::new();
        while let Some(s) = items.get(*i) {
            let Some(h) = s.head() else { break };
            let l = s.list().unwrap();
            let (code, has_tag) = match h {
                "catch" => (0, true),
                "catch_ref" => (1, true),
                "catch_all" => (2, false),
                "catch_all_ref" => (3, false),
                _ => break,
            };
            buf.push(code);
            let mut k = 1;
            if has_tag {
                let Some(t) = l.get(k) else { return err("expected tag") };
                leb_u32(&mut buf, self.cx.tags.resolve(t, "tag")?);
                k += 1;
            }
            let Some(lb) = l.get(k) else { return err("expected label") };
            leb_u32(&mut buf, self.label(lb)?);
            if l.len() != k + 1 {
                return err("unexpected token in catch clause");
            }
            n += 1;
            *i += 1;
        }
        leb_u32(&mut self.out, n);
        self.out.extend_from_slice(&buf);
        Ok(())
    }

    fn folded(&mut self, l: &[Sx]) -> R<()> {
        let Some(name) = l.first().and_then(|h| h.atom()) else { return err("unexpected token") };
        let mut i = 1;
        match name {
            "block" | "loop" | "try_table" => {
                let label = opt_id(l, &mut i);
                self.out.push(match name {
                    "block" => 0x02,
                    "loop" => 0x03,
                    _ => 0x1f,
                });
                self.block_type(l, &mut i)?;
                if name == "try_table" {
                    self.catches(l, &mut i)?;
                }
                self.labels.push(label);
                self.instrs(&l[i..])?;
                self.labels.pop();
                self.out.push(0x0b);
                Ok(())
            }
            "if" => {
                let label = opt_id(l, &mut i);
                let mut bt = Vec::new();
                std::mem::swap(&mut bt, &mut self.out);
                self.block_type(l, &mut i)?;
                std::mem::swap(&mut bt, &mut self.out);
                // Condition operands.
                while i < l.len() && !l[i].is_list_of("then") {
                    if !matches!(l[i], Sx::List(_)) {
                        return err("unexpected token in if");
                    }
                    self.instr(l, &mut i)?;
                }
                self.out.push(0x04);
                self.out.extend_from_slice(&bt);
                self.labels.push(label);
                let Some(then) = l.get(i).filter(|s| s.is_list_of("then")) else { return err("expected then") };
                self.instrs(&then.list().unwrap()[1..])?;
                i += 1;
                if let Some(e) = l.get(i) {
                    if !e.is_list_of("else") {
                        return err("unexpected token after then");
                    }
                    self.out.push(0x05);
                    self.instrs(&e.list().unwrap()[1..])?;
                    i += 1;
                }
                if i != l.len() {
                    return err("unexpected token in if");
                }
                self.labels.pop();
                self.out.push(0x0b);
                Ok(())
            }
            "try" => {
                let label = opt_id(l, &mut i);
                self.out.push(0x06);
                self.block_type(l, &mut i)?;
                self.labels.push(label);
                let Some(d) = l.get(i).filter(|s| s.is_list_of("do")) else { return err("expected do") };
                self.instrs(&d.list().unwrap()[1..])?;
                i += 1;
                while let Some(c) = l.get(i) {
                    let cl = c.list().unwrap_or(&[]);
                    match c.head() {
                        Some("catch") => {
                            self.out.push(0x07);
                            let Some(t) = cl.get(1) else { return err("expected tag") };
                            leb_u32(&mut self.out, self.cx.tags.resolve(t, "tag")?);
                            self.instrs(&cl[2..])?;
                        }
                        Some("catch_all") => {
                            self.out.push(0x19);
                            self.instrs(&cl[1..])?;
                        }
                        Some("delegate") => {
                            self.labels.pop();
                            self.out.push(0x18);
                            let Some(lb) = cl.get(1) else { return err("expected label") };
                            let d = self.label(lb)?;
                            leb_u32(&mut self.out, d);
                            return Ok(());
                        }
                        _ => return err("unexpected token in try"),
                    }
                    i += 1;
                }
                self.labels.pop();
                self.out.push(0x0b);
                Ok(())
            }
            _ => {
                // (op immediates... operands...)
                let mut tmp = Vec::new();
                std::mem::swap(&mut tmp, &mut self.out);
                let r = self.plain(name, l, &mut i);
                std::mem::swap(&mut tmp, &mut self.out);
                r?;
                // Remaining items are folded operands.
                while i < l.len() {
                    if !matches!(l[i], Sx::List(_)) {
                        return err(format!("unexpected token in folded {name}"));
                    }
                    self.instr(l, &mut i)?;
                }
                self.out.extend_from_slice(&tmp);
                Ok(())
            }
        }
    }

    fn next_index(&self, items: &[Sx], i: &mut usize) -> Option<Sx> {
        match items.get(*i) {
            Some(s) if is_index(s) => {
                *i += 1;
                Some(s.clone())
            }
            _ => None,
        }
    }

    fn need<'a>(&self, items: &'a [Sx], i: &mut usize, what: &str) -> R<&'a Sx> {
        match items.get(*i) {
            Some(s) => {
                *i += 1;
                Ok(s)
            }
            None => err(format!("expected {what}")),
        }
    }

    fn memarg(&mut self, items: &[Sx], i: &mut usize, natural: u32, lane: bool) -> R<()> {
        // memidx? offset=? align=? (lane)?
        let mut mem = 0;
        let mut toks: Vec<&Sx> = Vec::new();
        let mut k = *i;
        while let Some(s) = items.get(k) {
            match s {
                Sx::Id(_) => toks.push(s),
                Sx::Atom(a) if a.starts_with(|c: char| c.is_ascii_digit()) || a.starts_with("offset=") || a.starts_with("align=") => toks.push(s),
                _ => break,
            }
            k += 1;
        }
        // The lane index is the last bare number.
        let mut lane_tok = None;
        if lane {
            match toks.last() {
                Some(Sx::Atom(a)) if a.starts_with(|c: char| c.is_ascii_digit()) => {
                    lane_tok = toks.pop();
                }
                _ => return err("expected lane index"),
            }
        }
        let mut j = 0;
        if let Some(t) = toks.first() {
            let is_mem = match t {
                Sx::Id(_) => true,
                Sx::Atom(a) => a.starts_with(|c: char| c.is_ascii_digit()),
                _ => false,
            };
            if is_mem {
                mem = self.cx.mems.resolve(t, "memory")?;
                j = 1;
            }
        }
        let mut offset: u64 = 0;
        let mut align: u64 = natural as u64;
        let mut seen_align = false;
        for t in &toks[j..] {
            let Sx::Atom(a) = t else { return err("unexpected token") };
            if let Some(v) = a.strip_prefix("offset=") {
                if seen_align {
                    return err("unexpected token");
                }
                offset = num::parse_u64(v).ok_or_else(|| Error::Malformed("i32 constant".into()))?;
            } else if let Some(v) = a.strip_prefix("align=") {
                align = num::parse_u64(v).ok_or_else(|| Error::Malformed("alignment".into()))?;
                if align == 0 || !align.is_power_of_two() {
                    return err("alignment");
                }
                seen_align = true;
            } else {
                return err("unexpected token");
            }
        }
        if !self.cx.mem64.get(mem as usize).copied().unwrap_or(false) && offset > u32::MAX as u64 {
            return err("i32 constant out of range");
        }
        *i = k;
        let a = align.trailing_zeros();
        if mem != 0 {
            leb_u32(&mut self.out, a | 0x40);
            leb_u32(&mut self.out, mem);
        } else {
            leb_u32(&mut self.out, a);
        }
        leb_u64(&mut self.out, offset);
        if let Some(Sx::Atom(l)) = lane_tok {
            let v = num::parse_u32(l).filter(|&v| v < 256).ok_or_else(|| Error::Malformed("malformed lane index".into()))?;
            self.out.push(v as u8);
        }
        Ok(())
    }

    fn plain(&mut self, name: &str, items: &[Sx], i: &mut usize) -> R<()> {
        let Some((prefix, code, imm)) = lookup(name) else { return err(format!("unknown operator {name}")) };
        if let Some(p) = prefix {
            self.out.push(p);
            leb_u32(&mut self.out, code);
        } else {
            self.out.push(code as u8);
        }
        match imm {
            Imm::Nil => {}
            Imm::Zero => self.out.push(0),
            Imm::Label => {
                let s = self.need(items, i, "label")?;
                let d = self.label(s)?;
                leb_u32(&mut self.out, d);
            }
            Imm::BrTable => {
                let mut ls = Vec::new();
                while let Some(s) = self.next_index(items, i) {
                    ls.push(self.label(&s)?);
                }
                if ls.is_empty() {
                    return err("expected label");
                }
                leb_u32(&mut self.out, ls.len() as u32 - 1);
                for l in ls {
                    leb_u32(&mut self.out, l);
                }
            }
            Imm::Func => {
                let s = self.need(items, i, "function")?;
                let f = self.cx.funcs.resolve(s, "function")?;
                leb_u32(&mut self.out, f);
            }
            Imm::CallIndirect => {
                let table = match self.next_index(items, i) {
                    Some(s) => self.cx.tables.resolve(&s, "table")?,
                    None => 0,
                };
                let (t, names) = self.cx.type_use(items, i, false)?;
                let _ = names;
                leb_u32(&mut self.out, t);
                leb_u32(&mut self.out, table);
            }
            Imm::Type => {
                let s = self.need(items, i, "type")?;
                let t = self.cx.type_names.resolve(s, "type")?;
                leb_u32(&mut self.out, t);
            }
            Imm::TypeField => {
                let s = self.need(items, i, "type")?;
                let t = self.cx.type_names.resolve(s, "type")?;
                let f = self.need(items, i, "field")?;
                let fi = match f {
                    Sx::Id(n) => *self.cx.field_names.get(t as usize).and_then(|m| m.get(n)).ok_or_else(|| Error::Malformed(format!("unknown field ${n}")))?,
                    Sx::Atom(a) => num::parse_u32(a).ok_or_else(|| Error::Malformed("unknown field".into()))?,
                    _ => return err("expected field"),
                };
                leb_u32(&mut self.out, t);
                leb_u32(&mut self.out, fi);
            }
            Imm::TypeN => {
                let s = self.need(items, i, "type")?;
                let t = self.cx.type_names.resolve(s, "type")?;
                let n = self.need(items, i, "count")?;
                let n = n.atom().and_then(num::parse_u32).ok_or_else(|| Error::Malformed("expected count".into()))?;
                leb_u32(&mut self.out, t);
                leb_u32(&mut self.out, n);
            }
            Imm::TypeData | Imm::TypeElem | Imm::TypeType => {
                let s = self.need(items, i, "type")?;
                let t = self.cx.type_names.resolve(s, "type")?;
                let s2 = self.need(items, i, "index")?;
                let x = match imm {
                    Imm::TypeData => self.cx.datas.resolve(s2, "data")?,
                    Imm::TypeElem => self.cx.elems.resolve(s2, "elem")?,
                    _ => self.cx.type_names.resolve(s2, "type")?,
                };
                leb_u32(&mut self.out, t);
                leb_u32(&mut self.out, x);
            }
            Imm::Local => {
                let s = self.need(items, i, "local")?;
                let l = self.local(s)?;
                leb_u32(&mut self.out, l);
            }
            Imm::Global => {
                let s = self.need(items, i, "global")?;
                let g = self.cx.globals.resolve(s, "global")?;
                leb_u32(&mut self.out, g);
            }
            Imm::Table => {
                let t = match self.next_index(items, i) {
                    Some(s) => self.cx.tables.resolve(&s, "table")?,
                    None => 0,
                };
                leb_u32(&mut self.out, t);
            }
            Imm::TableCopy => {
                let (d, s) = match self.next_index(items, i) {
                    Some(d) => {
                        let Some(s) = self.next_index(items, i) else { return err("expected table") };
                        (self.cx.tables.resolve(&d, "table")?, self.cx.tables.resolve(&s, "table")?)
                    }
                    None => (0, 0),
                };
                leb_u32(&mut self.out, d);
                leb_u32(&mut self.out, s);
            }
            Imm::TableInit => {
                let a = self.next_index(items, i);
                let b = self.next_index(items, i);
                let (t, e) = match (a, b) {
                    (Some(t), Some(e)) => (self.cx.tables.resolve(&t, "table")?, self.cx.elems.resolve(&e, "elem")?),
                    (Some(e), None) => (0, self.cx.elems.resolve(&e, "elem")?),
                    _ => return err("expected elem"),
                };
                leb_u32(&mut self.out, e);
                leb_u32(&mut self.out, t);
            }
            Imm::Elem => {
                let s = self.need(items, i, "elem")?;
                let e = self.cx.elems.resolve(s, "elem")?;
                leb_u32(&mut self.out, e);
            }
            Imm::Data => {
                let s = self.need(items, i, "data")?;
                let d = self.cx.datas.resolve(s, "data")?;
                leb_u32(&mut self.out, d);
            }
            Imm::MemInit => {
                let a = self.next_index(items, i);
                let b = self.next_index(items, i);
                let (m, d) = match (a, b) {
                    (Some(m), Some(d)) => (self.cx.mems.resolve(&m, "memory")?, self.cx.datas.resolve(&d, "data")?),
                    (Some(d), None) => (0, self.cx.datas.resolve(&d, "data")?),
                    _ => return err("expected data"),
                };
                leb_u32(&mut self.out, d);
                leb_u32(&mut self.out, m);
            }
            Imm::Mem => {
                let m = match self.next_index(items, i) {
                    Some(s) => self.cx.mems.resolve(&s, "memory")?,
                    None => 0,
                };
                leb_u32(&mut self.out, m);
            }
            Imm::MemCopy => {
                let (d, s) = match self.next_index(items, i) {
                    Some(d) => {
                        let Some(s) = self.next_index(items, i) else { return err("expected memory") };
                        (self.cx.mems.resolve(&d, "memory")?, self.cx.mems.resolve(&s, "memory")?)
                    }
                    None => (0, 0),
                };
                leb_u32(&mut self.out, d);
                leb_u32(&mut self.out, s);
            }
            Imm::MemArg(n) => self.memarg(items, i, n, false)?,
            Imm::MemArgLane(n) => self.memarg(items, i, n, true)?,
            Imm::I32 => {
                let s = self.need(items, i, "i32 constant")?;
                let v = s.atom().and_then(|a| num::parse_int(a, 32)).ok_or_else(|| Error::Malformed("i32 constant out of range".into()))?;
                leb_s64(&mut self.out, v as u32 as i32 as i64);
            }
            Imm::I64 => {
                let s = self.need(items, i, "i64 constant")?;
                let v = s.atom().and_then(|a| num::parse_int(a, 64)).ok_or_else(|| Error::Malformed("i64 constant out of range".into()))?;
                leb_s64(&mut self.out, v as i64);
            }
            Imm::F32 => {
                let s = self.need(items, i, "f32 constant")?;
                let v = s.atom().and_then(num::parse_f32).ok_or_else(|| Error::Malformed("f32 constant out of range".into()))?;
                self.out.extend_from_slice(&v.to_le_bytes());
            }
            Imm::F64 => {
                let s = self.need(items, i, "f64 constant")?;
                let v = s.atom().and_then(num::parse_f64).ok_or_else(|| Error::Malformed("f64 constant out of range".into()))?;
                self.out.extend_from_slice(&v.to_le_bytes());
            }
            Imm::V128 => {
                let v = v128_const(items, i)?;
                self.out.extend_from_slice(&v.to_le_bytes());
            }
            Imm::Shuffle => {
                for _ in 0..16 {
                    let s = self.need(items, i, "lane index")?;
                    let v = s.atom().and_then(num::parse_u32).filter(|&v| v < 256).ok_or_else(|| Error::Malformed("malformed lane index".into()))?;
                    self.out.push(v as u8);
                }
            }
            Imm::Lane => {
                let s = self.need(items, i, "lane index")?;
                let v = s.atom().and_then(num::parse_u32).filter(|&v| v < 256).ok_or_else(|| Error::Malformed("malformed lane index".into()))?;
                self.out.push(v as u8);
            }
            Imm::HeapType => {
                let s = self.need(items, i, "heap type")?;
                let h = self.cx.heap(s)?;
                write_heap(&mut self.out, h);
            }
            Imm::Select => {
                let mut ts = Vec::new();
                while let Some(s) = items.get(*i) {
                    if !s.is_list_of("result") {
                        break;
                    }
                    for t in &s.list().unwrap()[1..] {
                        ts.push(self.cx.val_type(t)?);
                    }
                    *i += 1;
                }
                if items.get(*i).is_some_and(|s| s.is_list_of("param") || s.is_list_of("type")) {
                    return err("unexpected token in select");
                }
                if !ts.is_empty() {
                    let o = self.out.pop();
                    debug_assert_eq!(o, Some(0x1b));
                    self.out.push(0x1c);
                    leb_u32(&mut self.out, ts.len() as u32);
                    for t in ts {
                        write_val(&mut self.out, t);
                    }
                } else if items.get(*i).is_some_and(|s| s.is_list_of("result")) {
                    return err("unexpected token");
                }
            }
            Imm::Tag => {
                let s = self.need(items, i, "tag")?;
                let t = self.cx.tags.resolve(s, "tag")?;
                leb_u32(&mut self.out, t);
            }
            Imm::RefTypeImm => {
                let s = self.need(items, i, "reference type")?;
                let Some(r) = self.cx.ref_type(s)? else { return err("expected reference type") };
                if r.nullable {
                    // ref.test null / ref.cast null are the next opcodes.
                    let o = self.out.pop().unwrap();
                    self.out.push(o + 1);
                }
                write_heap(&mut self.out, r.heap);
            }
            Imm::BrCast => {
                let s = self.need(items, i, "label")?;
                let l = self.label(s)?;
                let a = self.need(items, i, "reference type")?;
                let b = self.need(items, i, "reference type")?;
                let (Some(r1), Some(r2)) = (self.cx.ref_type(a)?, self.cx.ref_type(b)?) else { return err("expected reference type") };
                let flags = (r1.nullable as u8) | ((r2.nullable as u8) << 1);
                self.out.push(flags);
                leb_u32(&mut self.out, l);
                write_heap(&mut self.out, r1.heap);
                write_heap(&mut self.out, r2.heap);
            }
        }
        Ok(())
    }
}

pub fn v128_const(items: &[Sx], i: &mut usize) -> R<u128> {
    let Some(shape) = items.get(*i).and_then(|s| s.atom()) else { return err("expected shape") };
    *i += 1;
    let (n, bits) = match shape {
        "i8x16" => (16, 8),
        "i16x8" => (8, 16),
        "i32x4" | "f32x4" => (4, 32),
        "i64x2" | "f64x2" => (2, 64),
        _ => return err("unexpected token: unknown shape"),
    };
    let mut v: u128 = 0;
    for k in 0..n {
        let Some(a) = items.get(*i).and_then(|s| s.atom()) else { return err("wrong number of lane literals") };
        *i += 1;
        let lane: u64 = match shape {
            "f32x4" => num::parse_f32(a).ok_or_else(|| Error::Malformed("constant out of range".into()))? as u64,
            "f64x2" => num::parse_f64(a).ok_or_else(|| Error::Malformed("constant out of range".into()))?,
            _ => num::parse_int(a, bits).ok_or_else(|| Error::Malformed("constant out of range".into()))?,
        };
        let mask = if bits == 64 { u64::MAX } else { (1u64 << bits) - 1 };
        v |= ((lane & mask) as u128) << (k * bits);
    }
    Ok(v)
}

// ------------------------------------------------------------------ module encoder

fn limits(items: &[Sx], i: &mut usize, is64: bool) -> R<(u64, Option<u64>)> {
    let parse = |s: &Sx| -> R<u64> {
        let a = s.atom().ok_or_else(|| Error::Malformed("expected limit".into()))?;
        if is64 {
            num::parse_u64(a).ok_or_else(|| Error::Malformed("i64 constant out of range".into()))
        } else {
            num::parse_u32(a).map(|v| v as u64).ok_or_else(|| Error::Malformed("i32 constant out of range".into()))
        }
    };
    let Some(min) = items.get(*i) else { return err("expected limits") };
    if !min.atom().is_some_and(|a| a.starts_with(|c: char| c.is_ascii_digit())) {
        return err("expected limits");
    }
    let min = parse(min)?;
    *i += 1;
    let mut max = None;
    if let Some(s) = items.get(*i) {
        if s.atom().is_some_and(|a| a.starts_with(|c: char| c.is_ascii_digit())) {
            max = Some(parse(s)?);
            *i += 1;
        }
    }
    Ok((min, max))
}

fn write_limits(out: &mut Vec<u8>, min: u64, max: Option<u64>, shared: bool, is64: bool, page_log2: Option<u32>) {
    let mut flags = 0u8;
    if max.is_some() {
        flags |= 1;
    }
    if shared {
        flags |= 2;
    }
    if is64 {
        flags |= 4;
    }
    if page_log2.is_some() {
        flags |= 8;
    }
    out.push(flags);
    leb_u64(out, min);
    if let Some(m) = max {
        leb_u64(out, m);
    }
    if let Some(p) = page_log2 {
        leb_u32(out, p);
    }
}

struct ExportEntry {
    name: Vec<u8>,
    kind: u8,
    index: u32,
}

/// Collect `(export "name")` abbreviations at items[*i..].
fn inline_exports(items: &[Sx], i: &mut usize, kind: u8, index: u32, exports: &mut Vec<ExportEntry>) -> R<()> {
    while let Some(s) = items.get(*i) {
        if !s.is_list_of("export") {
            break;
        }
        let l = s.list().unwrap();
        if l.len() != 2 {
            return err("malformed export");
        }
        exports.push(ExportEntry { name: utf8_name(&l[1])?, kind, index });
        *i += 1;
    }
    Ok(())
}

fn inline_import(items: &[Sx], i: &mut usize) -> R<Option<(Vec<u8>, Vec<u8>)>> {
    if let Some(s) = items.get(*i) {
        if s.is_list_of("import") {
            let l = s.list().unwrap();
            if l.len() != 3 {
                return err("malformed import");
            }
            *i += 1;
            return Ok(Some((utf8_name(&l[1])?, utf8_name(&l[2])?)));
        }
    }
    Ok(None)
}

fn field_id(l: &[Sx]) -> Option<&str> {
    match l.get(1) {
        Some(Sx::Id(n)) => Some(n),
        _ => None,
    }
}

/// Expand compact import fields (`(import "m" (item "a" desc) ...)` and
/// `(import "m" (item "a") (item "b") desc)`) into plain imports.
fn expand_compact_imports(fields: &[Sx]) -> R<Vec<Sx>> {
    let mut out = Vec::with_capacity(fields.len());
    for f in fields {
        let compact = f.head() == Some("import") && f.list().is_some_and(|l| l.len() >= 2 && !matches!(l.get(2), Some(Sx::Str(_))));
        if !compact {
            out.push(f.clone());
            continue;
        }
        let l = f.list().unwrap();
        let module = l[1].clone();
        let rest = &l[2..];
        let shared = rest.iter().find(|s| !s.is_list_of("item"));
        if let Some(d) = shared {
            if matches!(d.list().and_then(|l| l.get(1)), Some(Sx::Id(_))) {
                return err("identifier not allowed");
            }
            if !rest.iter().any(|s| s.is_list_of("item")) {
                // No items: the description's type use still counts.
                out.push(Sx::List(vec![Sx::Atom("implicit-type-use".into()), d.clone()]));
            }
        }
        for it in rest.iter().filter(|s| s.is_list_of("item")) {
            let il = it.list().unwrap();
            let name = il.get(1).cloned().ok_or_else(|| Error::Malformed("malformed import item".into()))?;
            let desc = match (il.get(2), shared) {
                (Some(d), None) if il.len() == 3 => d.clone(),
                (None, Some(d)) => d.clone(),
                _ => return err("malformed import item"),
            };
            out.push(Sx::List(vec![Sx::Atom("import".into()), module.clone(), name, desc]));
        }
        if rest.iter().filter(|s| !s.is_list_of("item")).count() > 1 {
            return err("malformed compact import");
        }
    }
    Ok(out)
}

pub fn encode_module(fields: &[Sx]) -> R<Vec<u8>> {
    let expanded = expand_compact_imports(fields)?;
    let fields: &[Sx] = &expanded;
    let mut cx = Ctx {
        types: Vec::new(),
        rec_groups: Vec::new(),
        type_names: Space::default(),
        field_names: Vec::new(),
        funcs: Space::default(),
        tables: Space::default(),
        mems: Space::default(),
        globals: Space::default(),
        tags: Space::default(),
        elems: Space::default(),
        datas: Space::default(),
        func_types: Vec::new(),
        tag_types: Vec::new(),
        table64: Vec::new(),
        mem64: Vec::new(),
    };
    for f in fields {
        if !matches!(f, Sx::List(_)) {
            return err("unexpected token in module");
        }
    }
    // Phase A: type names (including rec groups) and field names.
    let mut type_defs: Vec<&Sx> = Vec::new();
    let mut groups: Vec<(u32, u32)> = Vec::new();
    for f in fields {
        match f.head() {
            Some("type") => {
                let l = f.list().unwrap();
                let idx = cx.type_names.add(field_id(l), "type")?;
                let _ = idx;
                groups.push((type_defs.len() as u32, 1));
                type_defs.push(f);
            }
            Some("rec") => {
                let l = f.list().unwrap();
                let start = type_defs.len() as u32;
                for t in &l[1..] {
                    if !t.is_list_of("type") {
                        return err("unexpected token in rec");
                    }
                    cx.type_names.add(field_id(t.list().unwrap()), "type")?;
                    type_defs.push(t);
                }
                groups.push((start, l.len() as u32 - 1));
            }
            _ => {}
        }
    }
    // Field names of struct types.
    for t in &type_defs {
        let l = t.list().unwrap();
        let mut names = HashMap::new();
        let is_comp = |s: &&Sx| matches!(s, Sx::List(_)) && !s.is_list_of("describes") && !s.is_list_of("descriptor");
        let def = l.iter().skip(1).find(is_comp);
        let mut comp = def;
        if let Some(d) = def {
            if d.is_list_of("sub") {
                comp = d.list().unwrap().iter().skip(1).find(is_comp);
            }
        }
        if let Some(c) = comp {
            if c.is_list_of("struct") {
                let mut k = 0u32;
                for fd in &c.list().unwrap()[1..] {
                    if fd.is_list_of("field") {
                        let fl = fd.list().unwrap();
                        if let Some(Sx::Id(n)) = fl.get(1) {
                            if names.insert(n.clone(), k).is_some() {
                                return err(format!("duplicate field ${n}"));
                            }
                            k += 1;
                        } else {
                            k += fl.len() as u32 - 1;
                        }
                    }
                }
            }
        }
        cx.field_names.push(names);
    }
    // Encode explicit types.
    for t in &type_defs {
        let l = t.list().unwrap();
        let mut i = 1;
        opt_id(l, &mut i);
        if l.get(i).is_none() {
            return err("expected type definition");
        }
        let st = sub_type(&cx, &l[i..])?;
        cx.types.push(st);
    }
    cx.rec_groups = groups;

    // Phase B: index spaces and names.
    let mut seen_def = false;
    let mut exports: Vec<ExportEntry> = Vec::new();
    let mut imports_out: Vec<u8> = Vec::new();
    let mut n_imports = 0u32;
    let mut funcs_out: Vec<(u32, &Sx)> = Vec::new(); // (type, field) of defined funcs
    let mut tables_out: Vec<u8> = Vec::new();
    let mut n_tables = 0u32;
    let mut mems_out: Vec<u8> = Vec::new();
    let mut n_mems = 0u32;
    let mut globals_def: Vec<(&Sx, usize)> = Vec::new();
    let mut tags_out: Vec<u8> = Vec::new();
    let mut n_tags = 0u32;
    let mut start: Option<&Sx> = None;
    // Element and data segments in order: (field, kind) where kind is
    // explicit or an inline table/memory abbreviation.
    enum Seg<'a> {
        Elem(&'a Sx),
        InlineElem { table: u32, items: &'a [Sx], ty: RefType, is64: bool },
        Data(&'a Sx),
        InlineData { mem: u32, data: Vec<u8>, is64: bool },
    }
    let mut segs: Vec<Seg> = Vec::new();
    let mut explicit_exports: Vec<&Sx> = Vec::new();

    // First pass to register names of all index spaces (funcs may be
    // referenced before their definition).
    for f in fields {
        let l = f.list().unwrap();
        let id = field_id(l);
        match f.head().unwrap() {
            "func" => {
                cx.funcs.add(id, "function")?;
            }
            "table" => {
                cx.tables.add(id, "table")?;
                if l.iter().any(|s| s.is_list_of("elem")) {
                    cx.elems.add(None, "elem")?;
                }
            }
            "memory" => {
                cx.mems.add(id, "memory")?;
                if l.iter().any(|s| s.is_list_of("data")) {
                    cx.datas.add(None, "data")?;
                }
            }
            "global" => {
                cx.globals.add(id, "global")?;
            }
            "tag" => {
                cx.tags.add(id, "tag")?;
            }
            "elem" => {
                cx.elems.add(id, "elem")?;
            }
            "data" => {
                cx.datas.add(id, "data")?;
            }
            "import" => {
                let Some(desc) = l.get(3) else { return err("malformed import") };
                let did = desc.list().and_then(field_id);
                match desc.head() {
                    Some("func") => cx.funcs.add(did, "function")?,
                    Some("table") => cx.tables.add(did, "table")?,
                    Some("memory") => cx.mems.add(did, "memory")?,
                    Some("global") => cx.globals.add(did, "global")?,
                    Some("tag") => cx.tags.add(did, "tag")?,
                    _ => return err("malformed import kind"),
                };
            }
            "type" | "rec" | "export" | "start" | "implicit-type-use" => {}
            h => return err(format!("unknown module field {h}")),
        }
    }
    // Reset counts (names stay), recount in the real pass.
    let names_snapshot = (cx.funcs.count, cx.tables.count, cx.mems.count, cx.globals.count, cx.tags.count);
    let _ = names_snapshot;
    let mut idx_func = 0u32;
    let mut idx_table = 0u32;
    let mut idx_mem = 0u32;
    let mut idx_global = 0u32;
    let mut idx_tag = 0u32;
    let mut global_types: Vec<(ValType, bool)> = Vec::new();

    for f in fields {
        let l = f.list().unwrap();
        let mut i = 1;
        let id = opt_id(l, &mut i);
        let _ = id;
        match f.head().unwrap() {
            "type" | "rec" => {}
            "implicit-type-use" => {
                let d = l[1].list().unwrap_or(&[]);
                if matches!(l[1].head(), Some("func" | "tag")) {
                    let mut k = 1;
                    cx.type_use(d, &mut k, true)?;
                }
            }
            "import" => {
                if seen_def {
                    return err("import after definition");
                }
                if l.len() != 4 {
                    return err("malformed import");
                }
                let module = utf8_name(&l[1])?;
                let name = utf8_name(&l[2])?;
                let desc = l[3].list().unwrap_or(&[]);
                let mut k = 1;
                opt_id(desc, &mut k);
                write_name(&mut imports_out, &module);
                write_name(&mut imports_out, &name);
                n_imports += 1;
                let kind = match l[3].head() {
                    Some("func") => ImportKind::Func,
                    Some("table") => ImportKind::Table,
                    Some("memory") => ImportKind::Memory,
                    Some("global") => ImportKind::Global,
                    Some("tag") => ImportKind::Tag,
                    _ => return err("malformed import kind"),
                };
                encode_import_desc(&mut cx, kind, desc, &mut k, &mut imports_out, &mut global_types)?;
                if k != desc.len() {
                    return err("unexpected token in import");
                }
                match kind {
                    ImportKind::Func => idx_func += 1,
                    ImportKind::Table => idx_table += 1,
                    ImportKind::Memory => idx_mem += 1,
                    ImportKind::Global => idx_global += 1,
                    ImportKind::Tag => idx_tag += 1,
                }
            }
            "func" => {
                let index = idx_func;
                idx_func += 1;
                inline_exports(l, &mut i, 0, index, &mut exports)?;
                if let Some((m, n)) = inline_import(l, &mut i)? {
                    if seen_def {
                        return err("import after definition");
                    }
                    write_name(&mut imports_out, &m);
                    write_name(&mut imports_out, &n);
                    n_imports += 1;
                    let mut k = i;
                    encode_import_desc(&mut cx, ImportKind::Func, l, &mut k, &mut imports_out, &mut global_types)?;
                    if k != l.len() {
                        return err("unexpected token in imported function");
                    }
                } else {
                    seen_def = true;
                    let mut k = i;
                    let (t, _) = cx.type_use(l, &mut k, true)?;
                    cx.func_types.push(t);
                    funcs_out.push((t, f));
                }
            }
            "table" => {
                let index = idx_table;
                idx_table += 1;
                inline_exports(l, &mut i, 1, index, &mut exports)?;
                if let Some((m, n)) = inline_import(l, &mut i)? {
                    if seen_def {
                        return err("import after definition");
                    }
                    write_name(&mut imports_out, &m);
                    write_name(&mut imports_out, &n);
                    n_imports += 1;
                    let mut k = i;
                    encode_import_desc(&mut cx, ImportKind::Table, l, &mut k, &mut imports_out, &mut global_types)?;
                    if k != l.len() {
                        return err("unexpected token in imported table");
                    }
                    continue;
                }
                seen_def = true;
                n_tables += 1;
                let mut is64 = false;
                if l.get(i).and_then(|s| s.atom()) == Some("i64") {
                    is64 = true;
                    i += 1;
                } else if l.get(i).and_then(|s| s.atom()) == Some("i32") {
                    i += 1;
                }
                cx.table64.push(is64);
                // Inline elements: reftype (elem ...)
                if let Some(rt) = l.get(i).map(|s| cx.ref_type(s)).transpose()?.flatten() {
                    if let Some(e) = l.get(i + 1).filter(|s| s.is_list_of("elem")) {
                        let items = &e.list().unwrap()[1..];
                        let n = items.len() as u64;
                        write_ref(&mut tables_out, rt);
                        write_limits(&mut tables_out, n, Some(n), false, is64, None);
                        segs.push(Seg::InlineElem { table: index, items, ty: rt, is64 });
                        if l.len() != i + 2 {
                            return err("unexpected token in table");
                        }
                        continue;
                    }
                }
                let (min, max) = limits(l, &mut i, is64)?;
                let Some(rt) = l.get(i).map(|s| cx.ref_type(s)).transpose()?.flatten() else { return err("expected reference type") };
                i += 1;
                if i < l.len() {
                    // Table with initializer expression.
                    let mut b = Body { cx: &mut cx, locals: HashMap::new(), labels: vec![], out: Vec::new() };
                    b.instrs(&l[i..])?;
                    let init = b.out;
                    tables_out.push(0x40);
                    tables_out.push(0x00);
                    write_ref(&mut tables_out, rt);
                    write_limits(&mut tables_out, min, max, false, is64, None);
                    tables_out.extend_from_slice(&init);
                    tables_out.push(0x0b);
                } else {
                    write_ref(&mut tables_out, rt);
                    write_limits(&mut tables_out, min, max, false, is64, None);
                }
            }
            "memory" => {
                let index = idx_mem;
                idx_mem += 1;
                inline_exports(l, &mut i, 2, index, &mut exports)?;
                if let Some((m, n)) = inline_import(l, &mut i)? {
                    if seen_def {
                        return err("import after definition");
                    }
                    write_name(&mut imports_out, &m);
                    write_name(&mut imports_out, &n);
                    n_imports += 1;
                    let mut k = i;
                    encode_import_desc(&mut cx, ImportKind::Memory, l, &mut k, &mut imports_out, &mut global_types)?;
                    if k != l.len() {
                        return err("unexpected token in imported memory");
                    }
                    continue;
                }
                seen_def = true;
                n_mems += 1;
                let mut is64 = false;
                if l.get(i).and_then(|s| s.atom()) == Some("i64") {
                    is64 = true;
                    i += 1;
                } else if l.get(i).and_then(|s| s.atom()) == Some("i32") {
                    i += 1;
                }
                cx.mem64.push(is64);
                let mut page_log2 = None;
                if let Some(d) = l.get(i).filter(|s| s.is_list_of("pagesize")) {
                    page_log2 = Some(page_size(d)?);
                    i += 1;
                }
                if let Some(d) = l.get(i).filter(|s| s.is_list_of("data")) {
                    let mut data = Vec::new();
                    for s in &d.list().unwrap()[1..] {
                        data.extend_from_slice(&string(s)?);
                    }
                    let ps = 1u64 << page_log2.unwrap_or(16);
                    let pages = (data.len() as u64).div_ceil(ps);
                    write_limits(&mut mems_out, pages, Some(pages), false, is64, page_log2);
                    segs.push(Seg::InlineData { mem: index, data, is64 });
                    if l.len() != i + 1 {
                        return err("unexpected token in memory");
                    }
                    continue;
                }
                let (min, max) = limits(l, &mut i, is64)?;
                let mut shared = false;
                if l.get(i).and_then(|s| s.atom()) == Some("shared") {
                    shared = true;
                    i += 1;
                }
                if let Some(d) = l.get(i).filter(|s| s.is_list_of("pagesize")) {
                    page_log2 = Some(page_size(d)?);
                    i += 1;
                }
                if i != l.len() {
                    return err("unexpected token in memory");
                }
                write_limits(&mut mems_out, min, max, shared, is64, page_log2);
            }
            "global" => {
                let index = idx_global;
                idx_global += 1;
                inline_exports(l, &mut i, 3, index, &mut exports)?;
                if let Some((m, n)) = inline_import(l, &mut i)? {
                    if seen_def {
                        return err("import after definition");
                    }
                    write_name(&mut imports_out, &m);
                    write_name(&mut imports_out, &n);
                    n_imports += 1;
                    let mut k = i;
                    encode_import_desc(&mut cx, ImportKind::Global, l, &mut k, &mut imports_out, &mut global_types)?;
                    if k != l.len() {
                        return err("unexpected token in imported global");
                    }
                    continue;
                }
                seen_def = true;
                globals_def.push((f, i));
            }
            "tag" => {
                let index = idx_tag;
                idx_tag += 1;
                inline_exports(l, &mut i, 4, index, &mut exports)?;
                if let Some((m, n)) = inline_import(l, &mut i)? {
                    if seen_def {
                        return err("import after definition");
                    }
                    write_name(&mut imports_out, &m);
                    write_name(&mut imports_out, &n);
                    n_imports += 1;
                    let mut k = i;
                    encode_import_desc(&mut cx, ImportKind::Tag, l, &mut k, &mut imports_out, &mut global_types)?;
                    if k != l.len() {
                        return err("unexpected token in imported tag");
                    }
                    continue;
                }
                seen_def = true;
                n_tags += 1;
                let (t, _) = cx.type_use(l, &mut i, true)?;
                if i != l.len() {
                    return err("unexpected token in tag");
                }
                cx.tag_types.push(t);
                tags_out.push(0);
                leb_u32(&mut tags_out, t);
            }
            "export" => explicit_exports.push(f),
            "start" => {
                if start.is_some() {
                    return err("multiple start sections");
                }
                start = Some(f);
            }
            "elem" => segs.push(Seg::Elem(f)),
            "data" => segs.push(Seg::Data(f)),
            _ => {}
        }
    }

    // Globals (need full index spaces for init expressions).
    let mut globals_out = Vec::new();
    for (f, i0) in &globals_def {
        let l = f.list().unwrap();
        let mut i = *i0;
        let Some(gt) = l.get(i) else { return err("expected global type") };
        let (ty, mutable) = global_type(&cx, gt)?;
        i += 1;
        write_val(&mut globals_out, ty);
        globals_out.push(mutable as u8);
        let mut b = Body { cx: &mut cx, locals: HashMap::new(), labels: vec![], out: Vec::new() };
        b.instrs(&l[i..])?;
        globals_out.extend_from_slice(&b.out);
        globals_out.push(0x0b);
    }

    // Explicit exports.
    for f in &explicit_exports {
        let l = f.list().unwrap();
        if l.len() != 3 {
            return err("malformed export");
        }
        let name = utf8_name(&l[1])?;
        let d = l[2].list().unwrap_or(&[]);
        if d.len() != 2 {
            return err("malformed export");
        }
        let (kind, index) = match l[2].head() {
            Some("func") => (0, cx.funcs.resolve(&d[1], "function")?),
            Some("table") => (1, cx.tables.resolve(&d[1], "table")?),
            Some("memory") => (2, cx.mems.resolve(&d[1], "memory")?),
            Some("global") => (3, cx.globals.resolve(&d[1], "global")?),
            Some("tag") => (4, cx.tags.resolve(&d[1], "tag")?),
            _ => return err("malformed export kind"),
        };
        exports.push(ExportEntry { name, kind, index });
    }

    // Segments.
    let mut elems_out = Vec::new();
    let mut n_elems = 0u32;
    let mut datas_out = Vec::new();
    let mut n_datas = 0u32;
    for s in &segs {
        match s {
            Seg::InlineElem { table, items, ty, is64 } => {
                n_elems += 1;
                let all_funcs = items.iter().all(is_index) && *ty == RefType::FUNCREF;
                let off = if *is64 { vec![0x42, 0x00, 0x0b] } else { vec![0x41, 0x00, 0x0b] };
                if all_funcs {
                    if *table == 0 {
                        elems_out.push(0);
                    } else {
                        elems_out.push(2);
                        leb_u32(&mut elems_out, *table);
                    }
                    elems_out.extend_from_slice(&off);
                    if *table != 0 {
                        elems_out.push(0);
                    }
                    leb_u32(&mut elems_out, items.len() as u32);
                    for it in items.iter() {
                        leb_u32(&mut elems_out, cx.funcs.resolve(it, "function")?);
                    }
                } else {
                    elems_out.push(6);
                    leb_u32(&mut elems_out, *table);
                    elems_out.extend_from_slice(&off);
                    write_ref(&mut elems_out, *ty);
                    leb_u32(&mut elems_out, items.len() as u32);
                    for it in items.iter() {
                        if is_index(it) {
                            elems_out.push(0xd2);
                            leb_u32(&mut elems_out, cx.funcs.resolve(it, "function")?);
                            elems_out.push(0x0b);
                        } else {
                            let e = elem_expr(&mut cx, it)?;
                            elems_out.extend_from_slice(&e);
                        }
                    }
                }
            }
            Seg::InlineData { mem, data, is64 } => {
                n_datas += 1;
                if *mem == 0 {
                    datas_out.push(0);
                } else {
                    datas_out.push(2);
                    leb_u32(&mut datas_out, *mem);
                }
                datas_out.extend_from_slice(if *is64 { &[0x42, 0x00, 0x0b] } else { &[0x41, 0x00, 0x0b] });
                write_name(&mut datas_out, data);
            }
            Seg::Elem(f) => {
                n_elems += 1;
                encode_elem(&mut cx, f.list().unwrap(), &mut elems_out)?;
            }
            Seg::Data(f) => {
                n_datas += 1;
                encode_data(&mut cx, f.list().unwrap(), &mut datas_out)?;
            }
        }
    }

    // Function bodies.
    let mut code_out = Vec::new();
    for (t, f) in &funcs_out {
        let l = f.list().unwrap();
        let mut i = 1;
        opt_id(l, &mut i);
        while l.get(i).is_some_and(|s| s.is_list_of("export")) {
            i += 1;
        }
        let (_, names) = cx.type_use(l, &mut i, true)?;
        let _ = t;
        let mut locals: HashMap<String, u32> = HashMap::new();
        let mut nl = 0u32;
        for n in names {
            if let Some(n) = n {
                if locals.insert(n.clone(), nl).is_some() {
                    return err(format!("duplicate local ${n}"));
                }
            }
            nl += 1;
        }
        let mut decls: Vec<ValType> = Vec::new();
        while let Some(s) = l.get(i) {
            if !s.is_list_of("local") {
                break;
            }
            let ll = s.list().unwrap();
            if let Some(Sx::Id(n)) = ll.get(1) {
                if ll.len() != 3 {
                    return err("malformed local");
                }
                if locals.insert(n.clone(), nl).is_some() {
                    return err(format!("duplicate local ${n}"));
                }
                decls.push(cx.val_type(&ll[2])?);
                nl += 1;
            } else {
                for t in &ll[1..] {
                    decls.push(cx.val_type(t)?);
                    nl += 1;
                }
            }
            i += 1;
        }
        let mut b = Body { cx: &mut cx, locals, labels: vec![], out: Vec::new() };
        b.instrs(&l[i..])?;
        if !b.labels.is_empty() {
            return err("unclosed block");
        }
        let body = b.out;
        let mut fb = Vec::new();
        // Run-length local declarations.
        let mut runs: Vec<(u32, ValType)> = Vec::new();
        for t in decls {
            match runs.last_mut() {
                Some((n, lt)) if *lt == t => *n += 1,
                _ => runs.push((1, t)),
            }
        }
        leb_u32(&mut fb, runs.len() as u32);
        for (n, t) in runs {
            leb_u32(&mut fb, n);
            write_val(&mut fb, t);
        }
        fb.extend_from_slice(&body);
        fb.push(0x0b);
        leb_u32(&mut code_out, fb.len() as u32);
        code_out.extend_from_slice(&fb);
    }

    // Assemble.
    let mut out = b"\0asm\x01\0\0\0".to_vec();
    if !cx.types.is_empty() {
        let mut s = Vec::new();
        leb_u32(&mut s, cx.rec_groups.len() as u32);
        for &(start, n) in &cx.rec_groups {
            let explicit_rec = n != 1 || type_defs.get(start as usize).is_some_and(|_| is_in_rec(fields, start));
            if explicit_rec {
                s.push(0x4e);
                leb_u32(&mut s, n);
            }
            for k in start..start + n {
                write_sub_type(&mut s, &cx.types[k as usize]);
            }
        }
        write_section(&mut out, 1, &s);
    }
    if n_imports > 0 {
        let mut s = Vec::new();
        leb_u32(&mut s, n_imports);
        s.extend_from_slice(&imports_out);
        write_section(&mut out, 2, &s);
    }
    if !funcs_out.is_empty() {
        let mut s = Vec::new();
        leb_u32(&mut s, funcs_out.len() as u32);
        for (t, _) in &funcs_out {
            leb_u32(&mut s, *t);
        }
        write_section(&mut out, 3, &s);
    }
    if n_tables > 0 {
        let mut s = Vec::new();
        leb_u32(&mut s, n_tables);
        s.extend_from_slice(&tables_out);
        write_section(&mut out, 4, &s);
    }
    if n_mems > 0 {
        let mut s = Vec::new();
        leb_u32(&mut s, n_mems);
        s.extend_from_slice(&mems_out);
        write_section(&mut out, 5, &s);
    }
    if n_tags > 0 {
        let mut s = Vec::new();
        leb_u32(&mut s, n_tags);
        s.extend_from_slice(&tags_out);
        write_section(&mut out, 13, &s);
    }
    if !globals_def.is_empty() {
        let mut s = Vec::new();
        leb_u32(&mut s, globals_def.len() as u32);
        s.extend_from_slice(&globals_out);
        write_section(&mut out, 6, &s);
    }
    if !exports.is_empty() {
        let mut s = Vec::new();
        leb_u32(&mut s, exports.len() as u32);
        for e in &exports {
            write_name(&mut s, &e.name);
            s.push(e.kind);
            leb_u32(&mut s, e.index);
        }
        write_section(&mut out, 7, &s);
    }
    if let Some(f) = start {
        let l = f.list().unwrap();
        if l.len() != 2 {
            return err("malformed start");
        }
        let mut s = Vec::new();
        leb_u32(&mut s, cx.funcs.resolve(&l[1], "function")?);
        write_section(&mut out, 8, &s);
    }
    if n_elems > 0 {
        let mut s = Vec::new();
        leb_u32(&mut s, n_elems);
        s.extend_from_slice(&elems_out);
        write_section(&mut out, 9, &s);
    }
    if n_datas > 0 {
        let mut s = Vec::new();
        leb_u32(&mut s, n_datas);
        write_section(&mut out, 12, &s);
    }
    if !funcs_out.is_empty() {
        let mut s = Vec::new();
        leb_u32(&mut s, funcs_out.len() as u32);
        s.extend_from_slice(&code_out);
        write_section(&mut out, 10, &s);
    }
    if n_datas > 0 {
        let mut s = Vec::new();
        leb_u32(&mut s, n_datas);
        s.extend_from_slice(&datas_out);
        write_section(&mut out, 11, &s);
    }
    Ok(out)
}

/// Was explicit type `idx` declared inside a `(rec ...)`?
fn is_in_rec(fields: &[Sx], idx: u32) -> bool {
    let mut k = 0u32;
    for f in fields {
        match f.head() {
            Some("type") => {
                if k == idx {
                    return false;
                }
                k += 1;
            }
            Some("rec") => {
                let n = f.list().unwrap().len() as u32 - 1;
                if idx >= k && idx < k + n {
                    return true;
                }
                k += n;
            }
            _ => {}
        }
    }
    false
}

fn page_size(d: &Sx) -> R<u32> {
    let l = d.list().unwrap();
    let Some(v) = l.get(1).and_then(|s| s.atom()).and_then(num::parse_u64) else { return err("malformed page size") };
    if !v.is_power_of_two() || l.len() != 2 {
        return err("invalid custom page size");
    }
    Ok(v.trailing_zeros())
}

fn global_type(cx: &Ctx, sx: &Sx) -> R<(ValType, bool)> {
    if sx.is_list_of("mut") {
        let l = sx.list().unwrap();
        if l.len() != 2 {
            return err("malformed global type");
        }
        return Ok((cx.val_type(&l[1])?, true));
    }
    Ok((cx.val_type(sx)?, false))
}

fn encode_import_desc(cx: &mut Ctx, kind: ImportKind, items: &[Sx], k: &mut usize, out: &mut Vec<u8>, global_types: &mut Vec<(ValType, bool)>) -> R<()> {
    match kind {
        ImportKind::Func => {
            let t = match items.get(*k) {
                Some(e) if e.is_list_of("exact") => {
                    let el = e.list().unwrap();
                    let mut j = 1;
                    let (t, _) = cx.type_use(el, &mut j, true)?;
                    if j != el.len() {
                        return err("unexpected token in exact type use");
                    }
                    *k += 1;
                    out.push(0x20);
                    t
                }
                _ => {
                    out.push(0);
                    cx.type_use(items, k, true)?.0
                }
            };
            cx.func_types.push(t);
            leb_u32(out, t);
        }
        ImportKind::Table => {
            out.push(1);
            let mut is64 = false;
            if items.get(*k).and_then(|s| s.atom()) == Some("i64") {
                is64 = true;
                *k += 1;
            } else if items.get(*k).and_then(|s| s.atom()) == Some("i32") {
                *k += 1;
            }
            cx.table64.push(is64);
            let (min, max) = limits(items, k, is64)?;
            let Some(rt) = items.get(*k).map(|s| cx.ref_type(s)).transpose()?.flatten() else { return err("expected reference type") };
            *k += 1;
            write_ref(out, rt);
            write_limits(out, min, max, false, is64, None);
        }
        ImportKind::Memory => {
            out.push(2);
            let mut is64 = false;
            if items.get(*k).and_then(|s| s.atom()) == Some("i64") {
                is64 = true;
                *k += 1;
            } else if items.get(*k).and_then(|s| s.atom()) == Some("i32") {
                *k += 1;
            }
            cx.mem64.push(is64);
            let (min, max) = limits(items, k, is64)?;
            let mut shared = false;
            if items.get(*k).and_then(|s| s.atom()) == Some("shared") {
                shared = true;
                *k += 1;
            }
            let mut pl = None;
            if let Some(d) = items.get(*k).filter(|s| s.is_list_of("pagesize")) {
                pl = Some(page_size(d)?);
                *k += 1;
            }
            write_limits(out, min, max, shared, is64, pl);
        }
        ImportKind::Global => {
            out.push(3);
            let Some(gt) = items.get(*k) else { return err("expected global type") };
            let (t, m) = global_type(cx, gt)?;
            *k += 1;
            write_val(out, t);
            out.push(m as u8);
            global_types.push((t, m));
        }
        ImportKind::Tag => {
            out.push(4);
            out.push(0);
            let (t, _) = cx.type_use(items, k, true)?;
            cx.tag_types.push(t);
            leb_u32(out, t);
        }
    }
    Ok(())
}

/// `(describes x)? (descriptor y)? comptype` at the end of a type or sub
/// type definition.
fn desc_comp(cx: &Ctx, l: &[Sx]) -> R<(Option<u32>, Option<u32>, CompType)> {
    let mut i = 0;
    let clause = |name: &str, i: &mut usize| -> R<Option<u32>> {
        match l.get(*i) {
            Some(s) if s.is_list_of(name) => {
                let cl = s.list().unwrap();
                if cl.len() != 2 {
                    return err("unexpected token");
                }
                *i += 1;
                Ok(Some(cx.type_names.resolve(&cl[1], "type")?))
            }
            _ => Ok(None),
        }
    };
    let describes = clause("describes", &mut i)?;
    let descriptor = clause("descriptor", &mut i)?;
    let Some(c) = l.get(i) else { return err("expected composite type") };
    if l.len() != i + 1 {
        return err("unexpected token in type");
    }
    Ok((describes, descriptor, comp_type(cx, c)?))
}

fn sub_type(cx: &Ctx, defs: &[Sx]) -> R<SubType> {
    let def = &defs[0];
    if defs.len() == 1 && def.is_list_of("sub") {
        let l = def.list().unwrap();
        let mut i = 1;
        let mut is_final = false;
        if l.get(1).and_then(|s| s.atom()) == Some("final") {
            is_final = true;
            i = 2;
        }
        let mut supers = Vec::new();
        while let Some(s) = l.get(i) {
            if !is_index(s) {
                break;
            }
            supers.push(cx.type_names.resolve(s, "type")?);
            i += 1;
        }
        let (describes, descriptor, comp) = desc_comp(cx, &l[i..])?;
        return Ok(SubType { is_final, supers: supers.into(), describes, descriptor, comp });
    }
    let (describes, descriptor, comp) = desc_comp(cx, defs)?;
    Ok(SubType { is_final: true, supers: Box::new([]), describes, descriptor, comp })
}

fn comp_type(cx: &Ctx, c: &Sx) -> R<CompType> {
    let l = c.list().unwrap_or(&[]);
    match c.head() {
        Some("func") => {
            let mut i = 1;
            let (params, _, results, _) = cx.params_results(l, &mut i, true)?;
            if i != l.len() {
                return err("unexpected token in function type");
            }
            Ok(CompType::Func(FuncType::new(params, results)))
        }
        Some("struct") => {
            let mut fields = Vec::new();
            for f in &l[1..] {
                if !f.is_list_of("field") {
                    return err("unexpected token in struct type");
                }
                let fl = f.list().unwrap();
                let mut k = 1;
                if let Some(Sx::Id(_)) = fl.get(1) {
                    k = 2;
                    if fl.len() != 3 {
                        return err("malformed named field");
                    }
                }
                for t in &fl[k..] {
                    fields.push(cx.field_type(t)?);
                }
            }
            Ok(CompType::Struct(fields.into()))
        }
        Some("array") => {
            if l.len() != 2 {
                return err("malformed array type");
            }
            Ok(CompType::Array(cx.field_type(&l[1])?))
        }
        _ => err("unexpected token: expected composite type"),
    }
}

fn write_sub_type(out: &mut Vec<u8>, st: &SubType) {
    if !st.is_final || !st.supers.is_empty() {
        out.push(if st.is_final { 0x4f } else { 0x50 });
        leb_u32(out, st.supers.len() as u32);
        for &s in st.supers.iter() {
            leb_u32(out, s);
        }
    }
    if let Some(x) = st.describes {
        out.push(0x4c);
        leb_u32(out, x);
    }
    if let Some(x) = st.descriptor {
        out.push(0x4d);
        leb_u32(out, x);
    }
    let field = |out: &mut Vec<u8>, f: &FieldType| {
        match f.storage {
            StorageType::I8 => out.push(0x78),
            StorageType::I16 => out.push(0x77),
            StorageType::Val(v) => write_val(out, v),
        }
        out.push(f.mutable as u8);
    };
    match &st.comp {
        CompType::Func(f) => {
            out.push(0x60);
            leb_u32(out, f.params.len() as u32);
            for &p in f.params.iter() {
                write_val(out, p);
            }
            leb_u32(out, f.results.len() as u32);
            for &r in f.results.iter() {
                write_val(out, r);
            }
        }
        CompType::Struct(fs) => {
            out.push(0x5f);
            leb_u32(out, fs.len() as u32);
            for f in fs.iter() {
                field(out, f);
            }
        }
        CompType::Array(f) => {
            out.push(0x5e);
            field(out, f);
        }
    }
}

/// An element expression: `(item instr*)` or a single folded instruction.
fn elem_expr(cx: &mut Ctx, sx: &Sx) -> R<Vec<u8>> {
    let mut b = Body { cx, locals: HashMap::new(), labels: vec![], out: Vec::new() };
    if sx.is_list_of("item") {
        b.instrs(&sx.list().unwrap()[1..])?;
    } else {
        b.instrs(std::slice::from_ref(sx))?;
    }
    let mut out = b.out;
    out.push(0x0b);
    Ok(out)
}

fn offset_expr(cx: &mut Ctx, sx: &Sx) -> R<Vec<u8>> {
    let mut b = Body { cx, locals: HashMap::new(), labels: vec![], out: Vec::new() };
    if sx.is_list_of("offset") {
        b.instrs(&sx.list().unwrap()[1..])?;
    } else {
        b.instrs(std::slice::from_ref(sx))?;
    }
    let mut out = b.out;
    out.push(0x0b);
    Ok(out)
}

fn encode_elem(cx: &mut Ctx, l: &[Sx], out: &mut Vec<u8>) -> R<()> {
    let mut i = 1;
    opt_id(l, &mut i);
    // Mode
    enum Mode {
        Passive,
        Declare,
        Active(u32, Vec<u8>),
    }
    let mut mode = Mode::Passive;
    if l.get(i).and_then(|s| s.atom()) == Some("declare") {
        mode = Mode::Declare;
        i += 1;
    } else {
        let mut table = None;
        if let Some(t) = l.get(i).filter(|s| s.is_list_of("table")) {
            let tl = t.list().unwrap();
            if tl.len() != 2 {
                return err("malformed table use");
            }
            table = Some(cx.tables.resolve(&tl[1], "table")?);
            i += 1;
        } else if let Some(s) = l.get(i) {
            // Legacy: a bare table index before the offset.
            if is_index(s) && l.get(i + 1).is_some_and(|n| matches!(n, Sx::List(_))) && !matches!(l.get(i + 1).and_then(|n| n.head()), Some("ref" | "item")) {
                table = Some(cx.tables.resolve(s, "table")?);
                i += 1;
            }
        }
        if let Some(s) = l.get(i) {
            if let Sx::List(_) = s {
                let h = s.head();
                let is_offset = h == Some("offset") || (!matches!(h, Some("ref" | "item")) && cx.ref_type(s)?.is_none() && lookup(h.unwrap_or("")).is_some());
                if is_offset {
                    let e = offset_expr(cx, s)?;
                    mode = Mode::Active(table.unwrap_or(0), e);
                    i += 1;
                }
            }
        }
        if table.is_some() && !matches!(mode, Mode::Active(..)) {
            return err("expected offset");
        }
    }
    // Element list
    let rest = &l[i..];
    let (is_funcs, ty, items): (bool, RefType, &[Sx]) = if rest.first().and_then(|s| s.atom()) == Some("func") {
        (true, RefType { nullable: false, heap: HeapType::Func }, &rest[1..])
    } else if let Some(rt) = rest.first().map(|s| cx.ref_type(s)).transpose()?.flatten() {
        (false, rt, &rest[1..])
    } else {
        // Abbreviation: function indices only.
        (true, RefType { nullable: false, heap: HeapType::Func }, rest)
    };
    if is_funcs {
        for it in items {
            if !is_index(it) {
                return err("unexpected token in elem");
            }
        }
    }
    match mode {
        Mode::Active(t, e) => {
            if is_funcs {
                if t == 0 && !l.iter().any(|s| s.is_list_of("table")) {
                    out.push(0);
                    out.extend_from_slice(&e);
                } else {
                    out.push(2);
                    leb_u32(out, t);
                    out.extend_from_slice(&e);
                    out.push(0);
                }
            } else if t == 0 && ty == RefType::FUNCREF && !l.iter().any(|s| s.is_list_of("table")) {
                out.push(4);
                out.extend_from_slice(&e);
            } else {
                out.push(6);
                leb_u32(out, t);
                out.extend_from_slice(&e);
                write_ref(out, ty);
            }
        }
        Mode::Passive => {
            if is_funcs {
                out.push(1);
                out.push(0);
            } else {
                out.push(5);
                write_ref(out, ty);
            }
        }
        Mode::Declare => {
            if is_funcs {
                out.push(3);
                out.push(0);
            } else {
                out.push(7);
                write_ref(out, ty);
            }
        }
    }
    leb_u32(out, items.len() as u32);
    for it in items {
        if is_funcs {
            leb_u32(out, cx.funcs.resolve(it, "function")?);
        } else {
            let e = elem_expr(cx, it)?;
            out.extend_from_slice(&e);
        }
    }
    Ok(())
}

fn encode_data(cx: &mut Ctx, l: &[Sx], out: &mut Vec<u8>) -> R<()> {
    let mut i = 1;
    opt_id(l, &mut i);
    let mut mem = None;
    if let Some(m) = l.get(i).filter(|s| s.is_list_of("memory")) {
        let ml = m.list().unwrap();
        if ml.len() != 2 {
            return err("malformed memory use");
        }
        mem = Some(cx.mems.resolve(&ml[1], "memory")?);
        i += 1;
    } else if let Some(s) = l.get(i) {
        if is_index(s) {
            mem = Some(cx.mems.resolve(s, "memory")?);
            i += 1;
        }
    }
    let mut active = None;
    if let Some(s) = l.get(i) {
        if let Sx::List(_) = s {
            active = Some(offset_expr(cx, s)?);
            i += 1;
        }
    }
    match active {
        Some(e) => {
            let m = mem.unwrap_or(0);
            if m == 0 && !l.iter().any(|s| s.is_list_of("memory")) {
                out.push(0);
            } else {
                out.push(2);
                leb_u32(out, m);
            }
            out.extend_from_slice(&e);
        }
        None => {
            if mem.is_some() {
                return err("expected offset");
            }
            out.push(1);
        }
    }
    let mut data = Vec::new();
    for s in &l[i..] {
        data.extend_from_slice(&string(s)?);
    }
    write_name(out, &data);
    Ok(())
}
