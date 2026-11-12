//! GC proposal: structs, arrays, i31 references, casts and
//! extern/any conversions; custom descriptors (exact types, descriptor
//! structs, `ref.get_desc`, `ref.cast_desc_eq`, `br_on_cast_desc_eq`).
//!
//! Reference bits in the `any` hierarchy:
//! * `0`: null;
//! * `v << 1 | 1`: an `i31ref` holding `v` (31 bits);
//! * `(index + 1) << 2`: a struct or array in [`crate::runtime::Store::gc`];
//! * `e << 2 | 2`: an external reference `e` internalized with
//!   `any.convert_extern`.
//!
//! `extern.convert_any` of an internal value sets bit 63 (host external
//! references never use it). Objects are owned by the store and freed
//! with it (no collection yet).

use crate::binary::{malformed, Result};
use crate::compile::FC;
use crate::ir::*;
use crate::runtime::{Instance, Store, Trap};
use crate::types::*;
use crate::validate::invalid;

pub const EXTERN_ANY_BIT: u64 = 1 << 63;

/// Storage kind of a field or array element.
pub const K_I8: u8 = 0;
pub const K_I16: u8 = 1;
pub const K_VAL: u8 = 2;
pub const K_V128: u8 = 3;

pub struct GcObj {
    pub ty: TypeId,
    pub is_array: bool,
    /// Array element kind.
    pub kind: u8,
    pub data: Vec<u64>,
    /// Descriptor reference (custom descriptors; 0: none).
    pub desc: u64,
}

pub fn kind_of(s: StorageType) -> u8 {
    match s {
        StorageType::I8 => K_I8,
        StorageType::I16 => K_I16,
        StorageType::Val(ValType::V128) => K_V128,
        StorageType::Val(_) => K_VAL,
    }
}

fn stride(kind: u8) -> usize {
    if kind == K_V128 {
        2
    } else {
        1
    }
}

#[inline]
pub fn obj_bits(index: usize) -> u64 {
    (index as u64 + 1) << 2
}

#[inline]
pub fn obj_index(bits: u64) -> Option<usize> {
    if bits != 0 && bits & 3 == 0 && bits & EXTERN_ANY_BIT == 0 {
        Some((bits >> 2) as usize - 1)
    } else {
        None
    }
}

pub fn i31_bits(v: u32) -> u64 {
    ((v & 0x7fff_ffff) as u64) << 1 | 1
}

pub fn any_convert_extern(e: u64) -> u64 {
    if e == 0 {
        0
    } else if e & EXTERN_ANY_BIT != 0 {
        e & !EXTERN_ANY_BIT
    } else {
        e << 2 | 2
    }
}

pub fn extern_convert_any(a: u64) -> u64 {
    if a == 0 {
        0
    } else if a & 3 == 2 {
        a >> 2
    } else {
        a | EXTERN_ANY_BIT
    }
}

// ------------------------------------------------------------------ casts

/// Does reference `bits` match heap type `h` (canonical ids in
/// `Concrete`)? `h` determines the hierarchy.
pub fn ref_test(store: &Store, bits: u64, h: HeapType, nullable: bool) -> bool {
    if bits == 0 {
        return nullable;
    }
    match h {
        HeapType::Any | HeapType::Func | HeapType::Extern | HeapType::Exn => true,
        HeapType::None | HeapType::NoFunc | HeapType::NoExtern | HeapType::NoExn => false,
        HeapType::I31 => bits & 1 == 1,
        HeapType::Eq => bits & 1 == 1 || obj_index(bits).is_some(),
        HeapType::Struct | HeapType::Array => match obj_index(bits) {
            Some(i) => store.gc.borrow()[i].is_array == (h == HeapType::Array),
            None => false,
        },
        HeapType::Exact(c) => {
            if matches!(canonical_type(c).comp, CompType::Func(_)) {
                store.func((bits - 1) as u32).ty == c
            } else {
                match obj_index(bits) {
                    Some(i) => store.gc.borrow()[i].ty == c,
                    None => false,
                }
            }
        }
        HeapType::Concrete(c) => {
            if matches!(canonical_type(c).comp, CompType::Func(_)) {
                let f = store.func((bits - 1) as u32);
                canonical_subtype(f.ty, c)
            } else {
                match obj_index(bits) {
                    Some(i) => {
                        let t = store.gc.borrow()[i].ty;
                        canonical_subtype(t, c)
                    }
                    None => false,
                }
            }
        }
    }
}

// ------------------------------------------------------------------ translation

pub const STRUCT_NEW: u16 = 0;
pub const STRUCT_NEW_DEFAULT: u16 = 1;
pub const STRUCT_GET: u16 = 2;
pub const STRUCT_GET_S: u16 = 3;
pub const STRUCT_GET_U: u16 = 4;
pub const STRUCT_SET: u16 = 5;
pub const ARRAY_NEW: u16 = 6;
pub const ARRAY_NEW_DEFAULT: u16 = 7;
pub const ARRAY_NEW_FIXED: u16 = 8;
pub const ARRAY_NEW_DATA: u16 = 9;
pub const ARRAY_NEW_ELEM: u16 = 10;
pub const ARRAY_GET: u16 = 11;
pub const ARRAY_GET_S: u16 = 12;
pub const ARRAY_GET_U: u16 = 13;
pub const ARRAY_SET: u16 = 14;
pub const ARRAY_LEN: u16 = 15;
pub const ARRAY_FILL: u16 = 16;
pub const ARRAY_COPY: u16 = 17;
pub const ARRAY_INIT_DATA: u16 = 18;
pub const ARRAY_INIT_ELEM: u16 = 19;
pub const REF_TEST: u16 = 20;
pub const REF_CAST: u16 = 22;
pub const ANY_CONVERT_EXTERN: u16 = 26;
pub const EXTERN_CONVERT_ANY: u16 = 27;
pub const REF_I31: u16 = 28;
pub const I31_GET_S: u16 = 29;
pub const I31_GET_U: u16 = 30;
pub const STRUCT_NEW_DESC: u16 = 32;
pub const STRUCT_NEW_DEFAULT_DESC: u16 = 33;
pub const REF_GET_DESC: u16 = 34;
pub const REF_CAST_DESC_EQ: u16 = 35;
pub const REF_CAST_DESC_EQ_NULL: u16 = 36;
/// Condition of `br_on_cast_desc_eq(_fail)`: x bit 8 = target nullable.
pub const DESC_TEST: u16 = 37;

fn rt(nullable: bool, heap: HeapType) -> ValType {
    ValType::Ref(RefType { nullable, heap })
}

fn struct_fields<'a>(fc: &FC<'a>, t: u32) -> Result<&'a [FieldType]> {
    match fc.cx.tc.comp(t) {
        Some(CompType::Struct(fs)) => Ok(fs),
        Some(_) => invalid(format!("type mismatch: type {t} is not a struct")),
        None => invalid(format!("unknown type {t}")),
    }
}

fn array_field(fc: &FC, t: u32) -> Result<FieldType> {
    match fc.cx.tc.comp(t) {
        Some(CompType::Array(f)) => Ok(*f),
        Some(_) => invalid(format!("type mismatch: type {t} is not an array")),
        None => invalid(format!("unknown type {t}")),
    }
}

/// Slot offset of field `f` of a struct type.
fn field_offset(fs: &[FieldType], f: usize) -> u32 {
    fs[..f].iter().map(|x| if kind_of(x.storage) == K_V128 { 2 } else { 1 }).sum()
}

fn canon_heap(fc: &FC, h: HeapType) -> HeapType {
    match h {
        HeapType::Concrete(i) => HeapType::Concrete(fc.cx.m.canon[i as usize]),
        HeapType::Exact(i) => HeapType::Exact(fc.cx.m.canon[i as usize]),
        o => o,
    }
}

fn data_index(fc: &mut FC) -> Result<u32> {
    let d = fc.r.u32()?;
    let Some(dc) = fc.cx.m.data_count else { return malformed("data count section required") };
    if d >= dc {
        return invalid(format!("unknown data segment {d}"));
    }
    Ok(d)
}

fn ref_type_imm(fc: &mut FC, nullable: bool) -> Result<RefType> {
    let h = fc.r.heap_type()?;
    crate::validate::check_heap(fc.cx.m.types.len() as u32, h)?;
    Ok(RefType { nullable, heap: h })
}

/// Descriptor operand type of a descriptor cast to heap type `h`.
fn desc_operand(fc: &FC, h: HeapType) -> Result<ValType> {
    let Some(t) = h.index() else { return invalid(format!("type {h} does not have a descriptor")) };
    let Some(d) = fc.cx.m.types.get(t as usize).and_then(|s| s.descriptor) else {
        return invalid(format!("type {t} does not have a descriptor"));
    };
    Ok(rt(true, if matches!(h, HeapType::Exact(_)) { HeapType::Exact(d) } else { HeapType::Concrete(d) }))
}

pub(crate) fn gc_op(fc: &mut FC) -> Result<()> {
    use ValType::I32;
    let sub = fc.r.u32()?;
    let x = sub as u16;
    match x {
        STRUCT_NEW | STRUCT_NEW_DEFAULT => {
            let t = fc.r.u32()?;
            let fs = struct_fields(fc, t)?;
            let canon = fc.cx.m.canon[t as usize];
            if fc.cx.m.types[t as usize].descriptor.is_some() {
                return invalid("type mismatch: type with a descriptor needs struct.new_desc");
            }
            if x == STRUCT_NEW {
                let types: Vec<ValType> = fs.iter().map(|f| f.storage.unpacked()).collect();
                let base = fc.call_args_pub(&types)?;
                fc.push(rt(false, HeapType::Exact(t)));
                fc.emit(Instr::with_x(Op::Gc, x, base, canon, 0));
            } else {
                if fs.iter().any(|f| !f.storage.unpacked().defaultable()) {
                    return invalid("type mismatch: field type is not defaultable");
                }
                let d = fc.push(rt(false, HeapType::Exact(t)));
                fc.emit(Instr::with_x(Op::Gc, x, d, canon, 0));
            }
        }
        STRUCT_NEW_DESC | STRUCT_NEW_DEFAULT_DESC => {
            let t = fc.r.u32()?;
            let fs = struct_fields(fc, t)?;
            let canon = fc.cx.m.canon[t as usize];
            let Some(dt) = fc.cx.m.types[t as usize].descriptor else { return invalid(format!("type {t} does not have a descriptor")) };
            let desc_ty = rt(true, HeapType::Exact(dt));
            if x == STRUCT_NEW_DESC {
                let mut types: Vec<ValType> = fs.iter().map(|f| f.storage.unpacked()).collect();
                let nslots: u32 = types.iter().map(|t| t.slots()).sum();
                types.push(desc_ty);
                let base = fc.call_args_pub(&types)?;
                fc.push(rt(false, HeapType::Exact(t)));
                fc.emit(Instr::with_x(Op::Gc, x, base, canon, nslots));
            } else {
                if fs.iter().any(|f| !f.storage.unpacked().defaultable()) {
                    return invalid("type mismatch: field type is not defaultable");
                }
                let o = fc.pop_expect(desc_ty)?;
                let d = fc.push(rt(false, HeapType::Exact(t)));
                fc.emit(Instr::with_x(Op::Gc, x, d, canon, o.src()));
            }
        }
        REF_GET_DESC => {
            let t = fc.r.u32()?;
            struct_fields(fc, t)?;
            let Some(dt) = fc.cx.m.types[t as usize].descriptor else { return invalid("type without descriptor") };
            let o = fc.pop_expect(rt(true, HeapType::Concrete(t)))?;
            let exact = match o.ty {
                // exact only for exactly the inspected type (or bottom)
                Some(ValType::Ref(r)) => match r.heap {
                    HeapType::Exact(e) => fc.cx.m.canon[e as usize] == fc.cx.m.canon[t as usize],
                    HeapType::None => true,
                    _ => false,
                },
                _ => true,
            };
            let d = fc.push(rt(false, if exact { HeapType::Exact(dt) } else { HeapType::Concrete(dt) }));
            fc.emit(Instr::with_x(Op::Gc, x, d, o.src(), 0));
        }
        REF_CAST_DESC_EQ | REF_CAST_DESC_EQ_NULL => {
            let target = ref_type_imm(fc, x == REF_CAST_DESC_EQ_NULL)?;
            let desc_ty = desc_operand(fc, target.heap)?;
            let dsc = fc.pop_expect(desc_ty)?;
            let o = fc.pop_ref_pub()?;
            if let Some(ValType::Ref(src)) = o.ty {
                if fc.cx.tc.top(src.heap) != fc.cx.tc.top(target.heap) {
                    return invalid("type mismatch: cast to a type of another hierarchy");
                }
            }
            fc.emit(Instr::with_x(Op::Gc, x, o.src(), dsc.src(), 0));
            fc.push_loc_pub(ValType::Ref(target), o);
        }
        37 | 38 => {
            let flags = fc.r.byte()?;
            if flags > 3 {
                return malformed("malformed br_on_cast flags");
            }
            let l = fc.r.u32()?;
            let rt1 = ref_type_imm(fc, flags & 1 != 0)?;
            let rt2 = ref_type_imm(fc, flags & 2 != 0)?;
            let desc_ty = desc_operand(fc, rt2.heap)?;
            // custom descriptors: the types only need a common supertype
            if fc.cx.tc.top(rt1.heap) != fc.cx.tc.top(rt2.heap) {
                return invalid("type mismatch: br_on_cast types are in different hierarchies");
            }
            let (types, idx) = fc.label_types_pub(l)?;
            let Some((&last, rest)) = types.split_last() else { return invalid("type mismatch: br_on_cast label has no reference result") };
            let ValType::Ref(_) = last else { return invalid("type mismatch: br_on_cast label has no reference result") };
            let dsc = fc.pop_expect(desc_ty)?;
            let o = fc.pop_expect(ValType::Ref(rt1))?;
            let diff = RefType { nullable: rt1.nullable && !rt2.nullable, heap: rt1.heap };
            let (branch_ty, fall_ty) = if sub == 37 { (rt2, diff) } else { (diff, rt2) };
            if !fc.cx.tc.val_sub(ValType::Ref(branch_ty), last) {
                return invalid("type mismatch: br_on_cast type does not match the label");
            }
            fc.check_top_pub(rest)?;
            fc.retype_top_pub(rest)?;
            // condition in a scratch slot above the reference
            fc.push_loc_pub(ValType::Ref(rt1), o);
            let flag = fc.push(I32);
            fc.emit(Instr::with_x(Op::Gc, DESC_TEST | (rt2.nullable as u16) << 8, flag, o.src(), dsc.src()));
            fc.pop()?;
            fc.pop()?;
            fc.push_loc_pub(ValType::Ref(branch_ty), o);
            let (taken, inverse) = if sub == 37 { (Op::BrIfNez, Op::BrIfEqz) } else { (Op::BrIfEqz, Op::BrIfNez) };
            fc.br_on_pub(flag, 0, taken, inverse, &types, idx);
            fc.pop()?;
            fc.push_loc_pub(ValType::Ref(fall_ty), o);
        }
        STRUCT_GET | STRUCT_GET_S | STRUCT_GET_U => {
            let t = fc.r.u32()?;
            let f = fc.r.u32()?;
            let fs = struct_fields(fc, t)?;
            let Some(ft) = fs.get(f as usize).copied() else { return invalid(format!("unknown field {f}")) };
            let packed = matches!(ft.storage, StorageType::I8 | StorageType::I16);
            if packed != (x != STRUCT_GET) {
                return invalid(if packed { "type mismatch: packed field needs get_s/get_u" } else { "type mismatch: field is not packed" });
            }
            let off = field_offset(fs, f as usize);
            let r = fc.pop_expect(rt(true, HeapType::Concrete(t)))?;
            let d = fc.push(ft.storage.unpacked());
            fc.emit(Instr::with_x(Op::Gc, x | (kind_of(ft.storage) as u16) << 8, d, r.src(), off));
        }
        STRUCT_SET => {
            let t = fc.r.u32()?;
            let f = fc.r.u32()?;
            let fs = struct_fields(fc, t)?;
            let Some(ft) = fs.get(f as usize).copied() else { return invalid(format!("unknown field {f}")) };
            if !ft.mutable {
                return invalid("field is immutable");
            }
            let off = field_offset(fs, f as usize);
            fc.materialize_top(2);
            fc.pop_expect(ft.storage.unpacked())?;
            let r = fc.pop_expect(rt(true, HeapType::Concrete(t)))?;
            fc.emit(Instr::with_x(Op::Gc, x | (kind_of(ft.storage) as u16) << 8, r.slot, off, 0));
        }
        ARRAY_NEW | ARRAY_NEW_DEFAULT | ARRAY_NEW_FIXED => {
            let t = fc.r.u32()?;
            let ft = array_field(fc, t)?;
            let canon = fc.cx.m.canon[t as usize];
            let et = ft.storage.unpacked();
            let k = (kind_of(ft.storage) as u16) << 8;
            match x {
                ARRAY_NEW => {
                    let base = fc.call_args_pub(&[et, I32])?;
                    fc.push(rt(false, HeapType::Exact(t)));
                    fc.emit(Instr::with_x(Op::Gc, x | k, base, canon, 0));
                }
                ARRAY_NEW_DEFAULT => {
                    if !et.defaultable() {
                        return invalid("type mismatch: array type is not defaultable");
                    }
                    let base = fc.call_args_pub(&[I32])?;
                    fc.push(rt(false, HeapType::Exact(t)));
                    fc.emit(Instr::with_x(Op::Gc, x | k, base, canon, 0));
                }
                _ => {
                    let n = fc.r.u32()?;
                    if n > 10_000 {
                        return invalid("array.new_fixed too large");
                    }
                    let types = vec![et; n as usize];
                    let base = fc.call_args_pub(&types)?;
                    fc.push(rt(false, HeapType::Exact(t)));
                    fc.emit(Instr::with_x(Op::Gc, x | k, base, canon, n));
                }
            }
        }
        ARRAY_NEW_DATA | ARRAY_NEW_ELEM => {
            let t = fc.r.u32()?;
            let ft = array_field(fc, t)?;
            let canon = fc.cx.m.canon[t as usize];
            let seg = if x == ARRAY_NEW_DATA {
                if !ft.storage.unpacked().is_num() && ft.storage.unpacked() != ValType::V128 && matches!(ft.storage, StorageType::Val(_)) {
                    return invalid("type mismatch: array type is not numeric or vector");
                }
                data_index(fc)?
            } else {
                let e = fc.r.u32()?;
                let Some(seg) = fc.cx.m.elems.get(e as usize) else { return invalid(format!("unknown elem segment {e}")) };
                let StorageType::Val(et) = ft.storage else { return invalid("type mismatch: array type is not a reference") };
                if !fc.cx.tc.val_sub(ValType::Ref(seg.ty), et) {
                    return invalid("type mismatch: elem segment type");
                }
                e
            };
            let base = fc.call_args_pub(&[I32, I32])?;
            fc.push(rt(false, HeapType::Exact(t)));
            let k = (kind_of(ft.storage) as u16) << 8;
            fc.emit(Instr::with_x(Op::Gc, x | k, base, canon, seg));
        }
        ARRAY_GET | ARRAY_GET_S | ARRAY_GET_U => {
            let t = fc.r.u32()?;
            let ft = array_field(fc, t)?;
            let packed = matches!(ft.storage, StorageType::I8 | StorageType::I16);
            if packed != (x != ARRAY_GET) {
                return invalid(if packed { "type mismatch: packed array needs get_s/get_u" } else { "type mismatch: array is not packed" });
            }
            let i = fc.pop_expect(I32)?;
            let r = fc.pop_expect(rt(true, HeapType::Concrete(t)))?;
            let d = fc.push(ft.storage.unpacked());
            fc.emit(Instr::with_x(Op::Gc, x, d, r.src(), i.src()));
        }
        ARRAY_SET => {
            let t = fc.r.u32()?;
            let ft = array_field(fc, t)?;
            if !ft.mutable {
                return invalid("array is immutable");
            }
            let base = fc.call_args_pub(&[rt(true, HeapType::Concrete(t)), I32, ft.storage.unpacked()])?;
            fc.emit(Instr::with_x(Op::Gc, x, base, 0, 0));
        }
        ARRAY_LEN => {
            let r = fc.pop_expect(rt(true, HeapType::Array))?;
            let d = fc.push(I32);
            fc.emit(Instr::with_x(Op::Gc, x, d, r.src(), 0));
        }
        ARRAY_FILL => {
            let t = fc.r.u32()?;
            let ft = array_field(fc, t)?;
            if !ft.mutable {
                return invalid("array is immutable");
            }
            let base = fc.call_args_pub(&[rt(true, HeapType::Concrete(t)), I32, ft.storage.unpacked(), I32])?;
            fc.emit(Instr::with_x(Op::Gc, x, base, 0, 0));
        }
        ARRAY_COPY => {
            let t1 = fc.r.u32()?;
            let t2 = fc.r.u32()?;
            let f1 = array_field(fc, t1)?;
            let f2 = array_field(fc, t2)?;
            if !f1.mutable {
                return invalid("array is immutable");
            }
            let ok = match (f2.storage, f1.storage) {
                (StorageType::Val(a), StorageType::Val(b)) => fc.cx.tc.val_sub(a, b),
                (a, b) => a == b,
            };
            if !ok {
                return invalid("type mismatch: array types do not match");
            }
            let base = fc.call_args_pub(&[rt(true, HeapType::Concrete(t1)), I32, rt(true, HeapType::Concrete(t2)), I32, I32])?;
            fc.emit(Instr::with_x(Op::Gc, x, base, 0, 0));
        }
        ARRAY_INIT_DATA | ARRAY_INIT_ELEM => {
            let t = fc.r.u32()?;
            let ft = array_field(fc, t)?;
            if !ft.mutable {
                return invalid("array is immutable");
            }
            let seg = if x == ARRAY_INIT_DATA {
                if matches!(ft.storage, StorageType::Val(ValType::Ref(_))) {
                    return invalid("type mismatch: array type is not numeric or vector");
                }
                data_index(fc)?
            } else {
                let e = fc.r.u32()?;
                let Some(seg) = fc.cx.m.elems.get(e as usize) else { return invalid(format!("unknown elem segment {e}")) };
                let StorageType::Val(et) = ft.storage else { return invalid("type mismatch: array type is not a reference") };
                if !fc.cx.tc.val_sub(ValType::Ref(seg.ty), et) {
                    return invalid("type mismatch: elem segment type");
                }
                e
            };
            let base = fc.call_args_pub(&[rt(true, HeapType::Concrete(t)), I32, I32, I32])?;
            fc.emit(Instr::with_x(Op::Gc, x, base, seg, 0));
        }
        20..=23 => {
            let nullable = sub == 21 || sub == 23;
            let target = ref_type_imm(fc, nullable)?;
            let o = fc.pop_ref_pub()?;
            if let Some(ValType::Ref(src)) = o.ty {
                if fc.cx.tc.top(src.heap) != fc.cx.tc.top(target.heap) {
                    return invalid("type mismatch: cast to a type of another hierarchy");
                }
            }
            let ci = fc.cast_index(canon_heap(fc, target.heap), nullable);
            if sub <= 21 {
                let d = fc.push(I32);
                fc.emit(Instr::with_x(Op::Gc, REF_TEST, d, o.src(), ci));
            } else {
                fc.emit(Instr::with_x(Op::Gc, REF_CAST, o.src(), 0, ci));
                fc.push_loc_pub(ValType::Ref(target), o);
            }
        }
        24 | 25 => {
            let flags = fc.r.byte()?;
            if flags > 3 {
                return malformed("malformed br_on_cast flags");
            }
            let l = fc.r.u32()?;
            let rt1 = ref_type_imm(fc, flags & 1 != 0)?;
            let rt2 = ref_type_imm(fc, flags & 2 != 0)?;
            if !fc.cx.tc.ref_sub(rt2, rt1) {
                return invalid("type mismatch: br_on_cast target is not a subtype of the source");
            }
            let (types, idx) = fc.label_types_pub(l)?;
            let Some((&last, rest)) = types.split_last() else { return invalid("type mismatch: br_on_cast label has no reference result") };
            let ValType::Ref(_) = last else { return invalid("type mismatch: br_on_cast label has no reference result") };
            let o = fc.pop_expect(ValType::Ref(rt1))?;
            // Type on the branch and on fall-through.
            let diff = RefType { nullable: rt1.nullable && !rt2.nullable, heap: rt1.heap };
            let (branch_ty, fall_ty) = if sub == 24 { (rt2, diff) } else { (diff, rt2) };
            if !fc.cx.tc.val_sub(ValType::Ref(branch_ty), last) {
                return invalid("type mismatch: br_on_cast type does not match the label");
            }
            fc.check_top_pub(rest)?;
            fc.retype_top_pub(rest)?;
            fc.push_loc_pub(ValType::Ref(branch_ty), o);
            let ci = fc.cast_index(canon_heap(fc, rt2.heap), rt2.nullable);
            let (taken, inverse) = if sub == 24 { (Op::BrOnCast, Op::BrOnCastFail) } else { (Op::BrOnCastFail, Op::BrOnCast) };
            fc.br_on_pub(o.src(), ci, taken, inverse, &types, idx);
            fc.pop()?;
            fc.push_loc_pub(ValType::Ref(fall_ty), o);
        }
        ANY_CONVERT_EXTERN | EXTERN_CONVERT_ANY => {
            let o = fc.pop_ref_pub()?;
            let (from, to) = if x == ANY_CONVERT_EXTERN { (HeapType::Extern, HeapType::Any) } else { (HeapType::Any, HeapType::Extern) };
            let nullable = match o.ty {
                Some(ValType::Ref(r)) => {
                    if !fc.cx.tc.heap_sub(r.heap, from) {
                        return invalid("type mismatch: conversion operand");
                    }
                    r.nullable
                }
                _ => true,
            };
            let d = fc.push(rt(nullable, to));
            fc.emit(Instr::with_x(Op::Gc, x, d, o.src(), 0));
        }
        REF_I31 => {
            let o = fc.pop_expect(I32)?;
            let d = fc.push(rt(false, HeapType::I31));
            fc.emit(Instr::with_x(Op::Gc, x, d, o.src(), 0));
        }
        I31_GET_S | I31_GET_U => {
            let o = fc.pop_expect(rt(true, HeapType::I31))?;
            let d = fc.push(I32);
            fc.emit(Instr::with_x(Op::Gc, x, d, o.src(), 0));
        }
        _ => return malformed(format!("illegal opcode 0xfb {sub}")),
    }
    Ok(())
}

// ------------------------------------------------------------------ execution

fn pack(kind: u8, v: u64) -> u64 {
    match kind {
        K_I8 => v & 0xff,
        K_I16 => v & 0xffff,
        _ => v,
    }
}

fn unpack(kind: u8, v: u64, signed: bool) -> u64 {
    match (kind, signed) {
        (K_I8, true) => v as i8 as i32 as u32 as u64,
        (K_I16, true) => v as i16 as i32 as u32 as u64,
        _ => v,
    }
}

pub fn alloc(store: &Store, obj: GcObj) -> u64 {
    let mut g = store.gc.borrow_mut();
    g.push(obj);
    obj_bits(g.len() - 1)
}

/// Bytes of a data segment as array elements of `kind`.
fn data_elems(bytes: &[u8], kind: u8, ty: TypeId) -> Vec<u64> {
    let size = match kind {
        K_I8 => 1,
        K_I16 => 2,
        K_V128 => 16,
        _ => match canonical_type(ty).comp {
            CompType::Array(FieldType { storage: StorageType::Val(ValType::I32 | ValType::F32), .. }) => 4,
            _ => 8,
        },
    };
    let mut out = Vec::with_capacity(bytes.len() / size * stride(kind));
    for c in bytes.chunks_exact(size) {
        let mut b = [0u8; 16];
        b[..size].copy_from_slice(c);
        let v = u128::from_le_bytes(b);
        out.push(v as u64);
        if kind == K_V128 {
            out.push((v >> 64) as u64);
        }
    }
    out
}

fn data_elem_size(kind: u8, ty: TypeId) -> u64 {
    match kind {
        K_I8 => 1,
        K_I16 => 2,
        K_V128 => 16,
        _ => match canonical_type(ty).comp {
            CompType::Array(FieldType { storage: StorageType::Val(ValType::I32 | ValType::F32), .. }) => 4,
            _ => 8,
        },
    }
}

/// Execute a GC instruction.
///
/// # Safety
/// `regs` must point at a frame covering the instruction's slots.
pub unsafe fn exec(store: &Store, inst: &Instance, code: &Code, i: Instr, regs: *mut u64) -> std::result::Result<(), Trap> {
    let r = |s: u32| *regs.add(s as usize);
    let w = |s: u32, v: u64| *regs.add(s as usize) = v;
    let sub = i.x & 0xff;
    let kind = (i.x >> 8) as u8;
    match sub {
        STRUCT_NEW | STRUCT_NEW_DEFAULT => {
            let st = canonical_type(i.b);
            let CompType::Struct(fs) = &st.comp else { return Err(Trap::Other("not a struct".into())) };
            let n: usize = fs.iter().map(|f| stride(kind_of(f.storage))).sum();
            let mut data = vec![0u64; n];
            if sub == STRUCT_NEW {
                let mut off = 0;
                for f in fs.iter() {
                    let k = kind_of(f.storage);
                    data[off] = pack(k, r(i.a + off as u32));
                    if k == K_V128 {
                        data[off + 1] = r(i.a + off as u32 + 1);
                    }
                    off += stride(k);
                }
            }
            let bits = alloc(store, GcObj { ty: i.b, is_array: false, kind: K_VAL, data, desc: 0 });
            w(i.a, bits);
        }
        STRUCT_NEW_DESC | STRUCT_NEW_DEFAULT_DESC => {
            let desc = if sub == STRUCT_NEW_DESC { r(i.a + i.c) } else { r(i.c) };
            if desc == 0 {
                return Err(Trap::Other("null descriptor reference".into()));
            }
            let st = canonical_type(i.b);
            let CompType::Struct(fs) = &st.comp else { return Err(Trap::Other("not a struct".into())) };
            let n: usize = fs.iter().map(|f| stride(kind_of(f.storage))).sum();
            let mut data = vec![0u64; n];
            if sub == STRUCT_NEW_DESC {
                let mut off = 0;
                for f in fs.iter() {
                    let k = kind_of(f.storage);
                    data[off] = pack(k, r(i.a + off as u32));
                    if k == K_V128 {
                        data[off + 1] = r(i.a + off as u32 + 1);
                    }
                    off += stride(k);
                }
            }
            let bits = alloc(store, GcObj { ty: i.b, is_array: false, kind: K_VAL, data, desc });
            w(i.a, bits);
        }
        REF_GET_DESC => {
            let Some(o) = obj_index(r(i.b)) else { return Err(Trap::NullReference) };
            let d = store.gc.borrow()[o].desc;
            w(i.a, d);
        }
        REF_CAST_DESC_EQ | REF_CAST_DESC_EQ_NULL | DESC_TEST => {
            let (v, desc, nullable) = if sub == DESC_TEST { (r(i.b), r(i.c), kind != 0) } else { (r(i.a), r(i.b), sub == REF_CAST_DESC_EQ_NULL) };
            if desc == 0 {
                return Err(Trap::Other("null descriptor reference".into()));
            }
            let ok = if v == 0 {
                nullable
            } else {
                match obj_index(v) {
                    Some(o) => store.gc.borrow()[o].desc == desc,
                    None => false,
                }
            };
            if sub == DESC_TEST {
                w(i.a, ok as u64);
            } else if !ok {
                return Err(Trap::Other("descriptor cast failure".into()));
            }
        }
        STRUCT_GET | STRUCT_GET_S | STRUCT_GET_U => {
            let Some(o) = obj_index(r(i.b)) else { return Err(Trap::NullReference) };
            let g = store.gc.borrow();
            let d = &g[o].data;
            let v = d[i.c as usize];
            if kind == K_V128 {
                let hi = d[i.c as usize + 1];
                drop(g);
                w(i.a, v);
                w(i.a + 1, hi);
            } else {
                drop(g);
                w(i.a, unpack(kind, v, sub == STRUCT_GET_S));
            }
        }
        STRUCT_SET => {
            let Some(o) = obj_index(r(i.a)) else { return Err(Trap::NullReference) };
            let mut g = store.gc.borrow_mut();
            let d = &mut g[o].data;
            d[i.b as usize] = pack(kind, r(i.a + 1));
            if kind == K_V128 {
                d[i.b as usize + 1] = r(i.a + 2);
            }
        }
        ARRAY_NEW | ARRAY_NEW_DEFAULT | ARRAY_NEW_FIXED | ARRAY_NEW_DATA | ARRAY_NEW_ELEM => {
            let s = stride(kind);
            let data = match sub {
                ARRAY_NEW => {
                    let n = r(i.a + s as u32) as u32 as usize;
                    if n > 1 << 26 {
                        return Err(Trap::Other("out of memory".into()));
                    }
                    let mut d = Vec::with_capacity(n * s);
                    for _ in 0..n {
                        d.push(pack(kind, r(i.a)));
                        if s == 2 {
                            d.push(r(i.a + 1));
                        }
                    }
                    d
                }
                ARRAY_NEW_DEFAULT => {
                    let n = r(i.a) as u32 as usize;
                    if n > 1 << 26 {
                        return Err(Trap::Other("out of memory".into()));
                    }
                    vec![0; n * s]
                }
                ARRAY_NEW_FIXED => {
                    let n = i.c as usize * s;
                    (0..n).map(|k| if s == 1 { pack(kind, r(i.a + k as u32)) } else { r(i.a + k as u32) }).collect()
                }
                ARRAY_NEW_DATA => {
                    let off = r(i.a) as u32 as u64;
                    let n = r(i.a + 1) as u32 as u64;
                    let bytes = inst.data_bytes(i.c);
                    let size = data_elem_size(kind, i.b);
                    let end = n.checked_mul(size).and_then(|l| l.checked_add(off));
                    match end {
                        Some(e) if e <= bytes.len() as u64 => data_elems(&bytes[off as usize..e as usize], kind, i.b),
                        _ => return Err(Trap::Other("out of bounds memory access".into())),
                    }
                }
                _ => {
                    let off = r(i.a) as u32 as u64;
                    let n = r(i.a + 1) as u32 as u64;
                    let seg = inst.elems.borrow()[i.c as usize].clone();
                    if off + n > seg.len() as u64 {
                        return Err(Trap::TableOutOfBounds);
                    }
                    seg[off as usize..(off + n) as usize].to_vec()
                }
            };
            let bits = alloc(store, GcObj { ty: i.b, is_array: true, kind, data, desc: 0 });
            w(i.a, bits);
        }
        ARRAY_GET | ARRAY_GET_S | ARRAY_GET_U => {
            let Some(o) = obj_index(r(i.b)) else { return Err(Trap::NullReference) };
            let idx = r(i.c) as u32 as usize;
            let g = store.gc.borrow();
            let obj = &g[o];
            let s = stride(obj.kind);
            if (idx + 1) * s > obj.data.len() {
                return Err(Trap::Other("out of bounds array access".into()));
            }
            let v = obj.data[idx * s];
            let k = obj.kind;
            let hi = if s == 2 { obj.data[idx * 2 + 1] } else { 0 };
            drop(g);
            w(i.a, unpack(k, v, sub == ARRAY_GET_S));
            if s == 2 {
                w(i.a + 1, hi);
            }
        }
        ARRAY_SET => {
            let Some(o) = obj_index(r(i.a)) else { return Err(Trap::NullReference) };
            let idx = r(i.a + 1) as u32 as usize;
            let mut g = store.gc.borrow_mut();
            let obj = &mut g[o];
            let s = stride(obj.kind);
            if (idx + 1) * s > obj.data.len() {
                return Err(Trap::Other("out of bounds array access".into()));
            }
            obj.data[idx * s] = pack(obj.kind, r(i.a + 2));
            if s == 2 {
                obj.data[idx * 2 + 1] = r(i.a + 3);
            }
        }
        ARRAY_LEN => {
            let Some(o) = obj_index(r(i.b)) else { return Err(Trap::NullReference) };
            let g = store.gc.borrow();
            let n = g[o].data.len() / stride(g[o].kind);
            drop(g);
            w(i.a, n as u64);
        }
        ARRAY_FILL => {
            let Some(o) = obj_index(r(i.a)) else { return Err(Trap::NullReference) };
            let off = r(i.a + 1) as u32 as usize;
            let mut g = store.gc.borrow_mut();
            let obj = &mut g[o];
            let s = stride(obj.kind);
            let (v, hi) = (pack(obj.kind, r(i.a + 2)), r(i.a + 3));
            let n = r(i.a + 2 + s as u32) as u32 as usize;
            if (off + n) * s > obj.data.len() {
                return Err(Trap::Other("out of bounds array access".into()));
            }
            for k in off..off + n {
                obj.data[k * s] = v;
                if s == 2 {
                    obj.data[k * 2 + 1] = hi;
                }
            }
        }
        ARRAY_COPY => {
            let (Some(d), Some(sidx)) = (obj_index(r(i.a)), obj_index(r(i.a + 2))) else { return Err(Trap::NullReference) };
            let doff = r(i.a + 1) as u32 as usize;
            let soff = r(i.a + 3) as u32 as usize;
            let n = r(i.a + 4) as u32 as usize;
            let mut g = store.gc.borrow_mut();
            let s = stride(g[d].kind);
            if (doff + n) * s > g[d].data.len() || (soff + n) * s > g[sidx].data.len() {
                return Err(Trap::Other("out of bounds array access".into()));
            }
            if d == sidx {
                g[d].data.copy_within(soff * s..(soff + n) * s, doff * s);
            } else {
                let src: Vec<u64> = g[sidx].data[soff * s..(soff + n) * s].to_vec();
                g[d].data[doff * s..(doff + n) * s].copy_from_slice(&src);
            }
        }
        ARRAY_INIT_DATA | ARRAY_INIT_ELEM => {
            let Some(o) = obj_index(r(i.a)) else { return Err(Trap::NullReference) };
            let doff = r(i.a + 1) as u32 as u64;
            let soff = r(i.a + 2) as u32 as u64;
            let n = r(i.a + 3) as u32 as u64;
            let (ty, k, len) = {
                let g = store.gc.borrow();
                (g[o].ty, g[o].kind, (g[o].data.len() / stride(g[o].kind)) as u64)
            };
            if doff + n > len {
                return Err(Trap::Other("out of bounds array access".into()));
            }
            let vals = if sub == ARRAY_INIT_DATA {
                let bytes = inst.data_bytes(i.b);
                let size = data_elem_size(k, ty);
                if soff + n * size > bytes.len() as u64 {
                    return Err(Trap::MemoryOutOfBounds);
                }
                data_elems(&bytes[soff as usize..(soff + n * size) as usize], k, ty)
            } else {
                let seg = inst.elems.borrow()[i.b as usize].clone();
                if soff + n > seg.len() as u64 {
                    return Err(Trap::TableOutOfBounds);
                }
                seg[soff as usize..(soff + n) as usize].to_vec()
            };
            let s = stride(k) as u64;
            let mut g = store.gc.borrow_mut();
            g[o].data[(doff * s) as usize..((doff + n) * s) as usize].copy_from_slice(&vals);
        }
        REF_TEST => {
            let (h, nullable) = code.casts[i.c as usize];
            w(i.a, ref_test(store, r(i.b), h, nullable) as u64);
        }
        REF_CAST => {
            let (h, nullable) = code.casts[i.c as usize];
            if !ref_test(store, r(i.a), h, nullable) {
                return Err(Trap::Other("cast failure".into()));
            }
        }
        ANY_CONVERT_EXTERN => w(i.a, any_convert_extern(r(i.b))),
        EXTERN_CONVERT_ANY => w(i.a, extern_convert_any(r(i.b))),
        REF_I31 => w(i.a, i31_bits(r(i.b) as u32)),
        I31_GET_S | I31_GET_U => {
            let b = r(i.b);
            if b == 0 {
                return Err(Trap::NullReference);
            }
            let v = (b >> 1) as u32 & 0x7fff_ffff;
            let v = if sub == I31_GET_S { ((v << 1) as i32 >> 1) as u32 } else { v };
            w(i.a, v as u64);
        }
        _ => return Err(Trap::Other("unsupported GC instruction".into())),
    }
    Ok(())
}
