//! Threads proposal: atomic memory instructions (`0xfe` prefix).
//!
//! The engine runs one thread per store, so atomic accesses are plain
//! accesses with the alignment trap; `memory.atomic.wait*` returns
//! "not-equal" (1) or "timed-out" (2) immediately (no other thread can
//! notify) and `memory.atomic.notify` wakes nobody (0). Shared memories
//! are accepted; sharing them across workers needs SharedArrayBuffer
//! with cross-origin isolation, which the browser does not offer.

use crate::binary::{malformed, Result};
use crate::compile::FC;
use crate::ir::*;
use crate::runtime::{Instance, Trap};
use crate::types::ValType;
use crate::validate::invalid;

/// Text names and sub-opcodes.
pub const ATOMIC_NAMES: &[(&str, u32)] = &[
    ("memory.atomic.notify", 0x00),
    ("memory.atomic.wait32", 0x01),
    ("memory.atomic.wait64", 0x02),
    ("atomic.fence", 0x03),
    ("i32.atomic.load", 0x10),
    ("i64.atomic.load", 0x11),
    ("i32.atomic.load8_u", 0x12),
    ("i32.atomic.load16_u", 0x13),
    ("i64.atomic.load8_u", 0x14),
    ("i64.atomic.load16_u", 0x15),
    ("i64.atomic.load32_u", 0x16),
    ("i32.atomic.store", 0x17),
    ("i64.atomic.store", 0x18),
    ("i32.atomic.store8", 0x19),
    ("i32.atomic.store16", 0x1a),
    ("i64.atomic.store8", 0x1b),
    ("i64.atomic.store16", 0x1c),
    ("i64.atomic.store32", 0x1d),
];

const RMW_OPS: [&str; 7] = ["add", "sub", "and", "or", "xor", "xchg", "cmpxchg"];

/// Sub-opcode of a text name (including the generated rmw names).
pub fn atomic_opcode(name: &str) -> Option<u32> {
    if let Some(&(_, c)) = ATOMIC_NAMES.iter().find(|(n, _)| *n == name) {
        return Some(c);
    }
    for (g, op) in RMW_OPS.iter().enumerate() {
        let names = [
            format!("i32.atomic.rmw.{op}"),
            format!("i64.atomic.rmw.{op}"),
            format!("i32.atomic.rmw8.{op}_u"),
            format!("i32.atomic.rmw16.{op}_u"),
            format!("i64.atomic.rmw8.{op}_u"),
            format!("i64.atomic.rmw16.{op}_u"),
            format!("i64.atomic.rmw32.{op}_u"),
        ];
        if let Some(k) = names.iter().position(|n| n == name) {
            return Some(0x1e + 7 * g as u32 + k as u32);
        }
    }
    None
}

/// (access size, value type) of a load/store/rmw sub-opcode.
fn shape(sub: u32) -> (u32, ValType) {
    let k = match sub {
        0x10..=0x16 => sub - 0x10,
        0x17..=0x1d => sub - 0x17,
        _ => (sub - 0x1e) % 7,
    };
    const SIZES: [u32; 7] = [4, 8, 1, 2, 1, 2, 4];
    (SIZES[k as usize], if k == 0 || k == 2 || k == 3 { ValType::I32 } else { ValType::I64 })
}

/// Natural alignment of a sub-opcode (for the text encoder).
pub fn natural(sub: u32) -> u32 {
    match sub {
        0x00 | 0x01 => 4,
        0x02 => 8,
        _ => shape(sub).0,
    }
}

pub(crate) fn atomic_op(fc: &mut FC) -> Result<()> {
    use ValType::{I32, I64};
    let sub = fc.r.u32()?;
    if sub == 0x03 {
        if fc.r.byte()? != 0 {
            return malformed("malformed atomic.fence");
        }
        return Ok(());
    }
    if !(sub <= 0x02 || (0x10..=0x4e).contains(&sub)) {
        return malformed(format!("illegal opcode 0xfe {sub}"));
    }
    // memarg: alignment must be exactly natural.
    let mut align = fc.r.u32()?;
    let mut mem = 0;
    if align & 0x40 != 0 {
        align &= !0x40;
        mem = fc.r.u32()?;
    }
    let offset = fc.r.u64()?;
    let Some(mt) = fc.cx.m.memories.get(mem as usize).copied() else { return invalid(format!("unknown memory {mem}")) };
    if !mt.memory64 && offset > u32::MAX as u64 {
        return malformed("offset out of range");
    }
    let nat = natural(sub);
    if align >= 32 || (1u32 << align) != nat {
        return invalid("alignment must be exactly natural");
    }
    let at = fc.addr_type(mem);
    let (params, result): (Vec<ValType>, Option<ValType>) = match sub {
        0x00 => (vec![at, I32], Some(I32)),
        0x01 => (vec![at, I32, I64], Some(I32)),
        0x02 => (vec![at, I64, I64], Some(I32)),
        0x10..=0x16 => (vec![at], Some(shape(sub).1)),
        0x17..=0x1d => (vec![at, shape(sub).1], None),
        0x48..=0x4e => {
            let t = shape(sub).1;
            (vec![at, t, t], Some(t))
        }
        _ => {
            let t = shape(sub).1;
            (vec![at, t], Some(t))
        }
    };
    let base = fc.call_args_pub(&params)?;
    let mi = fc.memarg_index(mem, offset);
    fc.emit(Instr::with_x(Op::Atomic, sub as u16, base, 0, mi));
    if let Some(t) = result {
        fc.push(t);
    }
    Ok(())
}

/// Execute an atomic instruction (operands and result at `i.a`).
///
/// # Safety
/// `regs` must point at a frame covering the instruction's slots.
pub unsafe fn exec(inst: &Instance, code: &Code, i: Instr, regs: *mut u64) -> std::result::Result<(), Trap> {
    let sub = i.x as u32;
    let r = |k: u32| *regs.add((i.a + k) as usize);
    let ma = code.memargs[i.c as usize];
    let mem = &inst.memories[ma.memory as usize];
    let addr = if mem.ty.memory64 { r(0) } else { r(0) as u32 as u64 };
    let size: u64 = match sub {
        0x00 | 0x01 => 4,
        0x02 => 8,
        _ => shape(sub).0 as u64,
    };
    let (p, len) = mem.raw();
    let ea = addr.checked_add(ma.offset).ok_or(Trap::MemoryOutOfBounds)?;
    if ea.checked_add(size).is_none_or(|e| e > len as u64) {
        return Err(Trap::MemoryOutOfBounds);
    }
    if ea % size != 0 {
        return Err(Trap::Other("unaligned atomic".into()));
    }
    let ptr = p.add(ea as usize);
    let load = || -> u64 {
        let mut b = [0u8; 8];
        std::ptr::copy_nonoverlapping(ptr, b.as_mut_ptr(), size as usize);
        u64::from_le_bytes(b)
    };
    let store = |v: u64| {
        let b = v.to_le_bytes();
        std::ptr::copy_nonoverlapping(b.as_ptr(), ptr, size as usize);
    };
    let mask = if size == 8 { u64::MAX } else { (1u64 << (size * 8)) - 1 };
    let w = |v: u64| *regs.add(i.a as usize) = v;
    match sub {
        0x00 => w(0),
        0x01 | 0x02 => {
            if !mem.ty.shared {
                return Err(Trap::Other("expected shared memory".into()));
            }
            let expected = if sub == 0x01 { r(1) & 0xffff_ffff } else { r(1) };
            w(if load() != expected { 1 } else { 2 });
        }
        0x10..=0x16 => w(load()),
        0x17..=0x1d => store(r(1) & mask),
        0x48..=0x4e => {
            let old = load();
            if old == r(1) & mask {
                store(r(2) & mask);
            }
            w(old);
        }
        _ => {
            let old = load();
            let v = r(1);
            let new = match (sub - 0x1e) / 7 {
                0 => old.wrapping_add(v),
                1 => old.wrapping_sub(v),
                2 => old & v,
                3 => old | v,
                4 => old ^ v,
                _ => v,
            };
            store(new & mask);
            w(old);
        }
    }
    Ok(())
}
