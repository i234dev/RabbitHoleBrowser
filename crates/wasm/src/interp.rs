//! The interpreter: one dispatch loop over the register IR with an explicit
//! frame stack (wasm-to-wasm calls never recurse on the native stack).

use crate::ir::*;
use crate::runtime::*;
use crate::types::ValType;
use std::rc::Rc;

/// Back-edges and calls between interrupt polls.
const FUEL: u32 = 1 << 16;

struct Frame {
    code: *const Code,
    pc: usize,
    fp: usize,
    inst: *const Instance,
}

#[inline]
fn stack_ptr(store: &Store) -> *mut u64 {
    // SAFETY: the stack vector is only resized through `ensure`.
    unsafe { (*store.stack.get()).as_mut_ptr() }
}

/// Make the value stack at least `n` slots long; returns its base pointer.
#[inline]
fn ensure(store: &Store, n: usize) -> Result<*mut u64, Trap> {
    // SAFETY: no reference into the vector is held across this call (the
    // interpreter keeps raw pointers and refreshes them).
    let v = unsafe { &mut *store.stack.get() };
    if v.len() < n {
        if n > store.max_stack_slots {
            return Err(Trap::StackExhausted);
        }
        let new = (n.max(v.len() * 2)).clamp(4096, store.max_stack_slots.max(n));
        v.resize(new, 0);
    }
    Ok(v.as_mut_ptr())
}

fn write_val(p: *mut u64, v: Val) {
    // SAFETY: callers ensured the stack covers the slots written.
    unsafe {
        match v {
            Val::V128(x) => {
                *p = x as u64;
                *p.add(1) = (x >> 64) as u64;
            }
            o => *p = o.to_bits() as u64,
        }
    }
}

fn read_val(p: *const u64, t: ValType) -> Val {
    // SAFETY: as above.
    unsafe {
        if t == ValType::V128 {
            Val::V128(*p as u128 | (*p.add(1) as u128) << 64)
        } else {
            Val::from_bits(t, *p as u128)
        }
    }
}

/// Call `func` from the host.
pub fn invoke(store: &Store, host: &mut dyn Host, func: &Rc<FuncInst>, args: &[Val]) -> Result<Vec<Val>, Trap> {
    let base = store.stack_top.get();
    let ft = &func.ft;
    let need = base + ft.param_slots().max(ft.result_slots()) as usize + 2;
    let sp = ensure(store, need)?;
    let mut off = base;
    for (v, t) in args.iter().zip(ft.params.iter()) {
        write_val(unsafe { sp.add(off) }, *v);
        off += t.slots() as usize;
    }
    if store.depth.get() >= store.max_depth {
        return Err(Trap::StackExhausted);
    }
    match &func.kind {
        FuncKind::Host { .. } => {
            store.depth.set(store.depth.get() + 1);
            let r = host.call_host(store, func, args);
            store.depth.set(store.depth.get() - 1);
            r
        }
        FuncKind::Wasm { instance, code } => {
            let saved_depth = store.depth.get();
            let r = run(store, host, *instance, code, base);
            store.depth.set(saved_depth);
            r?;
            let sp = stack_ptr(store);
            let mut out = Vec::with_capacity(ft.results.len());
            let mut off = base;
            for &t in ft.results.iter() {
                out.push(read_val(unsafe { sp.add(off) }, t));
                off += t.slots() as usize;
            }
            Ok(out)
        }
    }
}

#[inline]
fn mem0(inst: *const Instance) -> (*mut u8, usize) {
    // SAFETY: instances live as long as the store.
    let inst = unsafe { &*inst };
    match inst.memories.first() {
        Some(m) => m.raw(),
        None => (std::ptr::null_mut(), 0),
    }
}

/// Enter a function at `fp`: check limits, zero declared locals, load
/// constants. Returns the new frame register pointer.
#[inline]
fn enter(store: &Store, code: &Code, fp: usize) -> Result<*mut u64, Trap> {
    let d = store.depth.get() + 1;
    if d > store.max_depth {
        return Err(Trap::StackExhausted);
    }
    store.depth.set(d);
    let sp = ensure(store, fp + code.frame_slots as usize + 2)?;
    // SAFETY: the frame fits in the stack (ensured above).
    unsafe {
        let regs = sp.add(fp);
        let ps = code.param_slots as usize;
        let ls = code.local_slots as usize;
        if ls > ps {
            std::ptr::write_bytes(regs.add(ps), 0, ls - ps);
        }
        let nc = code.consts.len();
        if nc > 0 {
            std::ptr::copy_nonoverlapping(code.consts.as_ptr(), regs.add(code.const_base as usize), nc);
        }
        Ok(regs)
    }
}

fn f32_min(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        return a + b;
    }
    if a == b {
        return f32::from_bits(a.to_bits() | b.to_bits());
    }
    if a < b {
        a
    } else {
        b
    }
}
fn f32_max(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        return a + b;
    }
    if a == b {
        return f32::from_bits(a.to_bits() & b.to_bits());
    }
    if a > b {
        a
    } else {
        b
    }
}
fn f64_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return a + b;
    }
    if a == b {
        return f64::from_bits(a.to_bits() | b.to_bits());
    }
    if a < b {
        a
    } else {
        b
    }
}
fn f64_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return a + b;
    }
    if a == b {
        return f64::from_bits(a.to_bits() & b.to_bits());
    }
    if a > b {
        a
    } else {
        b
    }
}

/// Quiet a NaN result (rounding a signalling NaN must give an arithmetic NaN).
#[inline]
pub(crate) fn q32(x: f32) -> f32 {
    if x.is_nan() {
        f32::from_bits(x.to_bits() | 0x0040_0000)
    } else {
        x
    }
}
#[inline]
pub(crate) fn q64(x: f64) -> f64 {
    if x.is_nan() {
        f64::from_bits(x.to_bits() | 0x0008_0000_0000_0000)
    } else {
        x
    }
}

pub(crate) fn f32_nearest(a: f32) -> f32 {
    a.round_ties_even()
}
pub(crate) fn f64_nearest(a: f64) -> f64 {
    a.round_ties_even()
}

#[inline]
fn trunc_check(x: f64, lo: f64, hi: f64) -> Result<f64, Trap> {
    if x.is_nan() {
        return Err(Trap::InvalidConversion);
    }
    let t = x.trunc();
    if t < lo || t >= hi {
        return Err(Trap::IntegerOverflow);
    }
    Ok(t)
}

const I32_LO: f64 = -2147483648.0;
const I32_HI: f64 = 2147483648.0;
const U32_HI: f64 = 4294967296.0;
const I64_LO: f64 = -9223372036854775808.0;
const I64_HI: f64 = 9223372036854775808.0;
const U64_HI: f64 = 18446744073709551616.0;

fn find_handler(store: &Store, code: &Code, inst: &Instance, pc: usize, exn: u32) -> Option<usize> {
    let mut max_depth = u32::MAX;
    let pc = pc as u32;
    let tag = store.exns.borrow()[exn as usize].tag.clone();
    for (k, h) in code.handlers.iter().enumerate() {
        if pc < h.start || pc >= h.end || h.depth > max_depth {
            continue;
        }
        match h.kind {
            CatchKind::Delegate => max_depth = h.tag,
            CatchKind::CatchAll | CatchKind::CatchAllRef => return Some(k),
            CatchKind::Catch | CatchKind::CatchRef => {
                if Rc::ptr_eq(&inst.tags[h.tag as usize], &tag) {
                    return Some(k);
                }
            }
        }
    }
    None
}

#[inline(always)]
unsafe fn ld<T: Copy>(p: *mut u8, ea: usize) -> T {
    std::ptr::read_unaligned(p.add(ea) as *const T)
}
#[inline(always)]
unsafe fn st<T: Copy>(p: *mut u8, ea: usize, v: T) {
    std::ptr::write_unaligned(p.add(ea) as *mut T, v)
}

/// Load/store helper for the generic memory path: effective address.
#[inline]
fn effective(mem: &MemInst, addr: u64, offset: u64, size: usize) -> Result<usize, Trap> {
    let a = if mem.ty.memory64 { addr } else { addr as u32 as u64 };
    let ea = a.checked_add(offset).ok_or(Trap::MemoryOutOfBounds)?;
    let len = mem.len() as u64;
    if ea.checked_add(size as u64).is_none_or(|e| e > len) {
        return Err(Trap::MemoryOutOfBounds);
    }
    Ok(ea as usize)
}

#[allow(clippy::cognitive_complexity, unused_unsafe)]
fn run(store: &Store, host: &mut dyn Host, inst_idx: u32, code_rc: &Rc<Code>, base: usize) -> Result<(), Trap> {
    let mut frames: Vec<Frame> = Vec::new();
    let mut inst: *const Instance = Rc::as_ptr(&store.instance(inst_idx));
    let mut code: *const Code = Rc::as_ptr(code_rc);
    let mut fp = base;
    let mut pc: usize = 0;
    // SAFETY: code and instances are owned by the store for its lifetime.
    let mut regs = enter(store, unsafe { &*code }, fp)?;
    let (mut mem_p, mut mem_len) = mem0(inst);
    let mut fuel = FUEL;

    macro_rules! r {
        ($s:expr) => {
            unsafe { *regs.add($s as usize) }
        };
    }
    macro_rules! w {
        ($s:expr, $v:expr) => {{
            let v: u64 = $v;
            unsafe { *regs.add($s as usize) = v }
        }};
    }
    macro_rules! r32 {
        ($s:expr) => {
            r!($s) as u32
        };
    }
    macro_rules! rf32 {
        ($s:expr) => {
            f32::from_bits(r!($s) as u32)
        };
    }
    macro_rules! rf64 {
        ($s:expr) => {
            f64::from_bits(r!($s))
        };
    }
    macro_rules! b2 {
        ($e:expr) => {
            ($e) as u64
        };
    }
    macro_rules! poll {
        () => {{
            fuel -= 1;
            if fuel == 0 {
                fuel = FUEL;
                if host.check_interrupt() {
                    return Err(Trap::Interrupted);
                }
            }
        }};
    }
    macro_rules! jump {
        ($t:expr) => {{
            let t = $t as usize;
            if t < pc {
                poll!();
            }
            pc = t;
        }};
    }
    macro_rules! ea {
        ($i:expr, $n:expr) => {{
            let ea = r32!($i.b) as usize + $i.c as usize;
            if ea + $n > mem_len {
                break Err(Trap::MemoryOutOfBounds);
            }
            ea
        }};
    }
    macro_rules! sea {
        ($i:expr, $n:expr) => {{
            let ea = r32!($i.a) as usize + $i.c as usize;
            if ea + $n > mem_len {
                break Err(Trap::MemoryOutOfBounds);
            }
            ea
        }};
    }
    macro_rules! try_trap {
        ($e:expr) => {
            match $e {
                Ok(v) => v,
                Err(t) => break Err(t),
            }
        };
    }

    // The result of the loop: Ok on return from the entry frame, Err for a
    // trap or an exception to unwind (handled below the loop).
    loop {
        let res: Result<(), Trap> = loop {
            // SAFETY: translation guarantees pc and all slots are in range.
            let i = unsafe { *(&*code).instrs.get_unchecked(pc) };
            pc += 1;
            match i.op {
                Op::Unreachable => break Err(Trap::Unreachable),
                Op::Br => jump!(i.c),
                Op::BrIfNez => {
                    if r32!(i.a) != 0 {
                        jump!(i.c)
                    }
                }
                Op::BrIfEqz => {
                    if r32!(i.a) == 0 {
                        jump!(i.c)
                    }
                }
                Op::BrTable => {
                    let idx = (r32!(i.a)).min(i.c - 1);
                    let t = unsafe { *(&*code).br_tables.get_unchecked((i.b + idx) as usize) };
                    jump!(t);
                }
                Op::BrI32Eq => {
                    if r32!(i.a) == r32!(i.b) {
                        jump!(i.c)
                    }
                }
                Op::BrI32Ne => {
                    if r32!(i.a) != r32!(i.b) {
                        jump!(i.c)
                    }
                }
                Op::BrI32LtS => {
                    if (r32!(i.a) as i32) < (r32!(i.b) as i32) {
                        jump!(i.c)
                    }
                }
                Op::BrI32LtU => {
                    if r32!(i.a) < r32!(i.b) {
                        jump!(i.c)
                    }
                }
                Op::BrI32GtS => {
                    if (r32!(i.a) as i32) > (r32!(i.b) as i32) {
                        jump!(i.c)
                    }
                }
                Op::BrI32GtU => {
                    if r32!(i.a) > r32!(i.b) {
                        jump!(i.c)
                    }
                }
                Op::BrI32LeS => {
                    if (r32!(i.a) as i32) <= (r32!(i.b) as i32) {
                        jump!(i.c)
                    }
                }
                Op::BrI32LeU => {
                    if r32!(i.a) <= r32!(i.b) {
                        jump!(i.c)
                    }
                }
                Op::BrI32GeS => {
                    if (r32!(i.a) as i32) >= (r32!(i.b) as i32) {
                        jump!(i.c)
                    }
                }
                Op::BrI32GeU => {
                    if r32!(i.a) >= r32!(i.b) {
                        jump!(i.c)
                    }
                }
                Op::BrI64Eq => {
                    if r!(i.a) == r!(i.b) {
                        jump!(i.c)
                    }
                }
                Op::BrI64Ne => {
                    if r!(i.a) != r!(i.b) {
                        jump!(i.c)
                    }
                }
                Op::BrI64LtS => {
                    if (r!(i.a) as i64) < (r!(i.b) as i64) {
                        jump!(i.c)
                    }
                }
                Op::BrI64LtU => {
                    if r!(i.a) < r!(i.b) {
                        jump!(i.c)
                    }
                }
                Op::BrI64GtS => {
                    if (r!(i.a) as i64) > (r!(i.b) as i64) {
                        jump!(i.c)
                    }
                }
                Op::BrI64GtU => {
                    if r!(i.a) > r!(i.b) {
                        jump!(i.c)
                    }
                }
                Op::BrI64LeS => {
                    if (r!(i.a) as i64) <= (r!(i.b) as i64) {
                        jump!(i.c)
                    }
                }
                Op::BrI64LeU => {
                    if r!(i.a) <= r!(i.b) {
                        jump!(i.c)
                    }
                }
                Op::BrI64GeS => {
                    if (r!(i.a) as i64) >= (r!(i.b) as i64) {
                        jump!(i.c)
                    }
                }
                Op::BrI64GeU => {
                    if r!(i.a) >= r!(i.b) {
                        jump!(i.c)
                    }
                }
                Op::BrI64Eqz | Op::BrOnNull => {
                    if r!(i.a) == 0 {
                        jump!(i.c)
                    }
                }
                Op::BrI64Nez | Op::BrOnNonNull => {
                    if r!(i.a) != 0 {
                        jump!(i.c)
                    }
                }
                Op::Return => {
                    store.depth.set(store.depth.get() - 1);
                    match frames.pop() {
                        None => return Ok(()),
                        Some(f) => {
                            code = f.code;
                            pc = f.pc;
                            fp = f.fp;
                            regs = unsafe { stack_ptr(store).add(fp) };
                            if f.inst != inst {
                                inst = f.inst;
                                (mem_p, mem_len) = mem0(inst);
                            }
                        }
                    }
                }
                Op::Call | Op::CallIndirect | Op::CallRef | Op::ReturnCall | Op::ReturnCallIndirect | Op::ReturnCallRef => {
                    poll!();
                    let ins = unsafe { &*inst };
                    let f: Rc<FuncInst> = match i.op {
                        Op::Call | Op::ReturnCall => ins.funcs[i.b as usize].clone(),
                        Op::CallIndirect | Op::ReturnCallIndirect => {
                            let (canon, ps) = unsafe { (&*code).call_types[i.c as usize] };
                            let t = &ins.tables[i.b as usize];
                            let raw = r!(i.a + ps);
                            let idx = if t.ty.table64 { raw } else { raw as u32 as u64 };
                            let e = {
                                let el = t.elems.borrow();
                                if idx >= el.len() as u64 {
                                    break Err(Trap::UndefinedElement);
                                }
                                el[idx as usize]
                            };
                            if e == 0 {
                                break Err(Trap::UninitializedElement);
                            }
                            let f = store.func((e - 1) as u32);
                            if f.ty != canon && !crate::types::canonical_subtype(f.ty, canon) {
                                break Err(Trap::IndirectCallTypeMismatch);
                            }
                            f
                        }
                        _ => {
                            let (_, ps) = unsafe { (&*code).call_types[i.c as usize] };
                            let e = r!(i.a + ps);
                            if e == 0 {
                                break Err(Trap::NullFunctionReference);
                            }
                            store.func((e - 1) as u32)
                        }
                    };
                    let tail = matches!(i.op, Op::ReturnCall | Op::ReturnCallIndirect | Op::ReturnCallRef);
                    let mut new_fp = fp + i.a as usize;
                    if tail {
                        // Move the arguments down to the frame base.
                        let n = f.ft.param_slots() as usize;
                        unsafe { std::ptr::copy(regs.add(i.a as usize), regs, n) };
                        new_fp = fp;
                    }
                    match &f.kind {
                        FuncKind::Wasm { instance, code: c } => {
                            if tail {
                                store.depth.set(store.depth.get() - 1);
                            } else {
                                frames.push(Frame { code, pc, fp, inst });
                            }
                            if *instance != unsafe { (&*inst).index } {
                                inst = Rc::as_ptr(&store.instance(*instance));
                                (mem_p, mem_len) = mem0(inst);
                            }
                            code = Rc::as_ptr(c);
                            fp = new_fp;
                            pc = 0;
                            regs = match enter(store, unsafe { &*code }, fp) {
                                Ok(r) => r,
                                Err(t) => break Err(t),
                            };
                        }
                        FuncKind::Host { .. } => {
                            let ft = &f.ft;
                            let mut args = Vec::with_capacity(ft.params.len());
                            let mut off = new_fp;
                            let sp = stack_ptr(store);
                            for &t in ft.params.iter() {
                                args.push(read_val(unsafe { sp.add(off) }, t));
                                off += t.slots() as usize;
                            }
                            let saved_top = store.stack_top.get();
                            store.stack_top.set(fp + unsafe { (&*code).frame_slots } as usize + 2);
                            store.depth.set(store.depth.get() + 1);
                            let res = host.call_host(store, &f, &args);
                            store.depth.set(store.depth.get() - 1);
                            store.stack_top.set(saved_top);
                            regs = unsafe { stack_ptr(store).add(fp) };
                            (mem_p, mem_len) = mem0(inst);
                            let vals = match res {
                                Ok(v) => v,
                                Err(t) => break Err(t),
                            };
                            let sp = stack_ptr(store);
                            let mut off = new_fp;
                            for (v, &t) in vals.iter().zip(ft.results.iter()) {
                                write_val(unsafe { sp.add(off) }, *v);
                                off += t.slots() as usize;
                            }
                            if tail {
                                // Return from the current frame.
                                store.depth.set(store.depth.get() - 1);
                                match frames.pop() {
                                    None => return Ok(()),
                                    Some(fr) => {
                                        code = fr.code;
                                        pc = fr.pc;
                                        fp = fr.fp;
                                        regs = unsafe { stack_ptr(store).add(fp) };
                                        if fr.inst != inst {
                                            inst = fr.inst;
                                            (mem_p, mem_len) = mem0(inst);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Op::Throw => {
                    let ins = unsafe { &*inst };
                    let tag = ins.tags[i.b as usize].clone();
                    let n = tag.ft.param_slots() as usize;
                    let payload: Vec<u64> = (0..n).map(|k| r!(i.a as usize + k)).collect();
                    let e = store.new_exn(tag, payload);
                    break Err(Trap::Exception(e));
                }
                Op::ThrowRef | Op::Rethrow => {
                    let e = r!(i.a);
                    if e == 0 {
                        break Err(Trap::NullReference);
                    }
                    break Err(Trap::Exception((e - 1) as u32));
                }
                Op::Copy => w!(i.a, r!(i.b)),
                Op::Copy2 => {
                    let (x, y) = (r!(i.b), r!(i.b + 1));
                    w!(i.a, x);
                    w!(i.a + 1, y);
                }
                Op::Const => w!(i.a, i.b as u64 | (i.c as u64) << 32),
                Op::Select => {
                    if r32!(i.c) == 0 {
                        w!(i.a, r!(i.b))
                    }
                }
                Op::Select2 => {
                    if r32!(i.c) == 0 {
                        let (x, y) = (r!(i.b), r!(i.b + 1));
                        w!(i.a, x);
                        w!(i.a + 1, y);
                    }
                }
                Op::GlobalGet => {
                    let g = unsafe { (&*inst).globals.get_unchecked(i.b as usize) };
                    w!(i.a, g.lo.get())
                }
                Op::GlobalSet => {
                    let g = unsafe { (&*inst).globals.get_unchecked(i.b as usize) };
                    g.lo.set(r!(i.a))
                }
                Op::GlobalGet2 => {
                    let g = unsafe { (&*inst).globals.get_unchecked(i.b as usize) };
                    w!(i.a, g.lo.get());
                    w!(i.a + 1, g.hi.get());
                }
                Op::GlobalSet2 => {
                    let g = unsafe { (&*inst).globals.get_unchecked(i.b as usize) };
                    g.lo.set(r!(i.a));
                    g.hi.set(r!(i.a + 1));
                }
                Op::I32Load => {
                    let ea = ea!(i, 4);
                    w!(i.a, unsafe { ld::<u32>(mem_p, ea) } as u64)
                }
                Op::I64Load => {
                    let ea = ea!(i, 8);
                    w!(i.a, unsafe { ld::<u64>(mem_p, ea) })
                }
                Op::I32Load8S => {
                    let ea = ea!(i, 1);
                    w!(i.a, unsafe { ld::<i8>(mem_p, ea) } as i32 as u32 as u64)
                }
                Op::I32Load8U => {
                    let ea = ea!(i, 1);
                    w!(i.a, unsafe { ld::<u8>(mem_p, ea) } as u64)
                }
                Op::I32Load16S => {
                    let ea = ea!(i, 2);
                    w!(i.a, unsafe { ld::<i16>(mem_p, ea) } as i32 as u32 as u64)
                }
                Op::I32Load16U => {
                    let ea = ea!(i, 2);
                    w!(i.a, unsafe { ld::<u16>(mem_p, ea) } as u64)
                }
                Op::I64Load8S => {
                    let ea = ea!(i, 1);
                    w!(i.a, unsafe { ld::<i8>(mem_p, ea) } as i64 as u64)
                }
                Op::I64Load8U => {
                    let ea = ea!(i, 1);
                    w!(i.a, unsafe { ld::<u8>(mem_p, ea) } as u64)
                }
                Op::I64Load16S => {
                    let ea = ea!(i, 2);
                    w!(i.a, unsafe { ld::<i16>(mem_p, ea) } as i64 as u64)
                }
                Op::I64Load16U => {
                    let ea = ea!(i, 2);
                    w!(i.a, unsafe { ld::<u16>(mem_p, ea) } as u64)
                }
                Op::I64Load32S => {
                    let ea = ea!(i, 4);
                    w!(i.a, unsafe { ld::<i32>(mem_p, ea) } as i64 as u64)
                }
                Op::I64Load32U => {
                    let ea = ea!(i, 4);
                    w!(i.a, unsafe { ld::<u32>(mem_p, ea) } as u64)
                }
                Op::I32Store => {
                    let ea = sea!(i, 4);
                    unsafe { st::<u32>(mem_p, ea, r32!(i.b)) }
                }
                Op::I64Store => {
                    let ea = sea!(i, 8);
                    unsafe { st::<u64>(mem_p, ea, r!(i.b)) }
                }
                Op::I32Store8 => {
                    let ea = sea!(i, 1);
                    unsafe { st::<u8>(mem_p, ea, r!(i.b) as u8) }
                }
                Op::I32Store16 => {
                    let ea = sea!(i, 2);
                    unsafe { st::<u16>(mem_p, ea, r!(i.b) as u16) }
                }
                Op::I64Store32 => {
                    let ea = sea!(i, 4);
                    unsafe { st::<u32>(mem_p, ea, r!(i.b) as u32) }
                }
                Op::MemLoad => {
                    let ma = unsafe { (&*code).memargs[i.c as usize] };
                    let mem = unsafe { &(&*inst).memories[ma.memory as usize] };
                    let size = match i.x {
                        0x28 | 0x2a | 0x34 | 0x35 => 4,
                        0x29 | 0x2b => 8,
                        0x2c | 0x2d | 0x30 | 0x31 => 1,
                        _ => 2,
                    };
                    let ea = try_trap!(effective(mem, r!(i.b), ma.offset, size));
                    let (p, _) = mem.raw();
                    let v = unsafe {
                        match i.x {
                            0x28 | 0x2a | 0x35 => ld::<u32>(p, ea) as u64,
                            0x29 | 0x2b => ld::<u64>(p, ea),
                            0x2c => ld::<i8>(p, ea) as i32 as u32 as u64,
                            0x2d | 0x31 => ld::<u8>(p, ea) as u64,
                            0x2e => ld::<i16>(p, ea) as i32 as u32 as u64,
                            0x2f | 0x33 => ld::<u16>(p, ea) as u64,
                            0x30 => ld::<i8>(p, ea) as i64 as u64,
                            0x32 => ld::<i16>(p, ea) as i64 as u64,
                            _ => ld::<i32>(p, ea) as i64 as u64,
                        }
                    };
                    w!(i.a, v)
                }
                Op::MemStore => {
                    let ma = unsafe { (&*code).memargs[i.c as usize] };
                    let mem = unsafe { &(&*inst).memories[ma.memory as usize] };
                    let size = match i.x {
                        0x36 | 0x38 | 0x3e => 4,
                        0x37 | 0x39 => 8,
                        0x3a | 0x3c => 1,
                        _ => 2,
                    };
                    let ea = try_trap!(effective(mem, r!(i.a), ma.offset, size));
                    let (p, _) = mem.raw();
                    let v = r!(i.b);
                    unsafe {
                        match size {
                            1 => st::<u8>(p, ea, v as u8),
                            2 => st::<u16>(p, ea, v as u16),
                            4 => st::<u32>(p, ea, v as u32),
                            _ => st::<u64>(p, ea, v),
                        }
                    }
                }
                Op::MemorySize => {
                    let mem = unsafe { &(&*inst).memories[i.b as usize] };
                    w!(i.a, mem.pages())
                }
                Op::MemoryGrow => {
                    let mem = unsafe { &(&*inst).memories[i.c as usize] };
                    let n = if mem.ty.memory64 { r!(i.b) } else { r32!(i.b) as u64 };
                    let res = match mem.grow(n) {
                        Some(old) => old,
                        None => {
                            if mem.ty.memory64 {
                                u64::MAX
                            } else {
                                u32::MAX as u64
                            }
                        }
                    };
                    w!(i.a, res);
                    (mem_p, mem_len) = mem0(inst);
                }
                Op::MemoryFill => {
                    let mem = unsafe { &(&*inst).memories[i.b as usize] };
                    let m64 = mem.ty.memory64;
                    let d = if m64 { r!(i.a) } else { r32!(i.a) as u64 };
                    let v = r!(i.a + 1) as u8;
                    let n = if m64 { r!(i.a + 2) } else { r32!(i.a + 2) as u64 };
                    let (p, len) = mem.raw();
                    if d.checked_add(n).is_none_or(|e| e > len as u64) {
                        break Err(Trap::MemoryOutOfBounds);
                    }
                    unsafe { std::ptr::write_bytes(p.add(d as usize), v, n as usize) };
                }
                Op::MemoryCopy => {
                    let dm = unsafe { &(&*inst).memories[i.b as usize] };
                    let sm = unsafe { &(&*inst).memories[i.c as usize] };
                    let d = if dm.ty.memory64 { r!(i.a) } else { r32!(i.a) as u64 };
                    let s = if sm.ty.memory64 { r!(i.a + 1) } else { r32!(i.a + 1) as u64 };
                    let n = if dm.ty.memory64 && sm.ty.memory64 { r!(i.a + 2) } else { r32!(i.a + 2) as u64 };
                    let (dp, dl) = dm.raw();
                    let (sp, sl) = sm.raw();
                    if d.checked_add(n).is_none_or(|e| e > dl as u64) || s.checked_add(n).is_none_or(|e| e > sl as u64) {
                        break Err(Trap::MemoryOutOfBounds);
                    }
                    unsafe { std::ptr::copy(sp.add(s as usize), dp.add(d as usize), n as usize) };
                }
                Op::MemoryInit => {
                    let ins = unsafe { &*inst };
                    let mem = &ins.memories[i.c as usize];
                    let d = if mem.ty.memory64 { r!(i.a) } else { r32!(i.a) as u64 };
                    let s = r32!(i.a + 1) as u64;
                    let n = r32!(i.a + 2) as u64;
                    let data = ins.data_bytes(i.b);
                    let (p, len) = mem.raw();
                    if d.checked_add(n).is_none_or(|e| e > len as u64) || s + n > data.len() as u64 {
                        break Err(Trap::MemoryOutOfBounds);
                    }
                    unsafe { std::ptr::copy_nonoverlapping(data.as_ptr().add(s as usize), p.add(d as usize), n as usize) };
                }
                Op::DataDrop => {
                    let ins = unsafe { &*inst };
                    ins.data_dropped.borrow_mut()[i.b as usize] = true;
                }
                Op::TableGet => {
                    let t = unsafe { &(&*inst).tables[i.c as usize] };
                    let idx = if t.ty.table64 { r!(i.b) } else { r32!(i.b) as u64 };
                    let el = t.elems.borrow();
                    if idx >= el.len() as u64 {
                        break Err(Trap::TableOutOfBounds);
                    }
                    let v = el[idx as usize];
                    drop(el);
                    w!(i.a, v)
                }
                Op::TableSet => {
                    let t = unsafe { &(&*inst).tables[i.c as usize] };
                    let idx = if t.ty.table64 { r!(i.a) } else { r32!(i.a) as u64 };
                    let mut el = t.elems.borrow_mut();
                    if idx >= el.len() as u64 {
                        break Err(Trap::TableOutOfBounds);
                    }
                    el[idx as usize] = r!(i.b);
                }
                Op::TableSize => {
                    let t = unsafe { &(&*inst).tables[i.c as usize] };
                    w!(i.a, t.size())
                }
                Op::TableGrow => {
                    let t = unsafe { &(&*inst).tables[i.c as usize] };
                    let init = r!(i.a);
                    let n = if t.ty.table64 { r!(i.a + 1) } else { r32!(i.a + 1) as u64 };
                    let res = match t.grow(n, init, store.table_limit) {
                        Some(old) => old,
                        None => {
                            if t.ty.table64 {
                                u64::MAX
                            } else {
                                u32::MAX as u64
                            }
                        }
                    };
                    w!(i.a, res)
                }
                Op::TableFill => {
                    let t = unsafe { &(&*inst).tables[i.c as usize] };
                    let t64 = t.ty.table64;
                    let d = if t64 { r!(i.a) } else { r32!(i.a) as u64 };
                    let v = r!(i.a + 1);
                    let n = if t64 { r!(i.a + 2) } else { r32!(i.a + 2) as u64 };
                    let mut el = t.elems.borrow_mut();
                    if d.checked_add(n).is_none_or(|e| e > el.len() as u64) {
                        break Err(Trap::TableOutOfBounds);
                    }
                    el[d as usize..(d + n) as usize].fill(v);
                }
                Op::TableCopy => {
                    let ins = unsafe { &*inst };
                    let dt = &ins.tables[i.b as usize];
                    let stt = &ins.tables[i.c as usize];
                    let d = if dt.ty.table64 { r!(i.a) } else { r32!(i.a) as u64 };
                    let s = if stt.ty.table64 { r!(i.a + 1) } else { r32!(i.a + 1) as u64 };
                    let n = if dt.ty.table64 && stt.ty.table64 { r!(i.a + 2) } else { r32!(i.a + 2) as u64 };
                    if Rc::ptr_eq(dt, stt) {
                        let mut el = dt.elems.borrow_mut();
                        let len = el.len() as u64;
                        if d.checked_add(n).is_none_or(|e| e > len) || s.checked_add(n).is_none_or(|e| e > len) {
                            break Err(Trap::TableOutOfBounds);
                        }
                        el.copy_within(s as usize..(s + n) as usize, d as usize);
                    } else {
                        let src = stt.elems.borrow();
                        let mut dst = dt.elems.borrow_mut();
                        if d.checked_add(n).is_none_or(|e| e > dst.len() as u64) || s.checked_add(n).is_none_or(|e| e > src.len() as u64) {
                            break Err(Trap::TableOutOfBounds);
                        }
                        dst[d as usize..(d + n) as usize].copy_from_slice(&src[s as usize..(s + n) as usize]);
                    }
                }
                Op::TableInit => {
                    let ins = unsafe { &*inst };
                    let t = &ins.tables[i.c as usize];
                    let d = if t.ty.table64 { r!(i.a) } else { r32!(i.a) as u64 };
                    let s = r32!(i.a + 1) as u64;
                    let n = r32!(i.a + 2) as u64;
                    let seg = ins.elems.borrow()[i.b as usize].clone();
                    let mut el = t.elems.borrow_mut();
                    if d.checked_add(n).is_none_or(|e| e > el.len() as u64) || s + n > seg.len() as u64 {
                        break Err(Trap::TableOutOfBounds);
                    }
                    el[d as usize..(d + n) as usize].copy_from_slice(&seg[s as usize..(s + n) as usize]);
                }
                Op::ElemDrop => {
                    let ins = unsafe { &*inst };
                    ins.elems.borrow_mut()[i.b as usize] = Rc::new([]);
                }
                Op::RefIsNull => w!(i.a, b2!(r!(i.b) == 0)),
                Op::RefFunc => {
                    let ins = unsafe { &*inst };
                    w!(i.a, ins.funcs[i.b as usize].addr as u64 + 1)
                }
                Op::RefAsNonNull => {
                    if r!(i.a) == 0 {
                        break Err(Trap::NullReference);
                    }
                }
                Op::RefEq => w!(i.a, b2!(r!(i.b) == r!(i.c))),

                // ---------------------------------------------------- i32
                Op::I32Eqz => w!(i.a, b2!(r32!(i.b) == 0)),
                Op::I32Eq => w!(i.a, b2!(r32!(i.b) == r32!(i.c))),
                Op::I32Ne => w!(i.a, b2!(r32!(i.b) != r32!(i.c))),
                Op::I32LtS => w!(i.a, b2!((r32!(i.b) as i32) < (r32!(i.c) as i32))),
                Op::I32LtU => w!(i.a, b2!(r32!(i.b) < r32!(i.c))),
                Op::I32GtS => w!(i.a, b2!((r32!(i.b) as i32) > (r32!(i.c) as i32))),
                Op::I32GtU => w!(i.a, b2!(r32!(i.b) > r32!(i.c))),
                Op::I32LeS => w!(i.a, b2!((r32!(i.b) as i32) <= (r32!(i.c) as i32))),
                Op::I32LeU => w!(i.a, b2!(r32!(i.b) <= r32!(i.c))),
                Op::I32GeS => w!(i.a, b2!((r32!(i.b) as i32) >= (r32!(i.c) as i32))),
                Op::I32GeU => w!(i.a, b2!(r32!(i.b) >= r32!(i.c))),
                Op::I32Clz => w!(i.a, r32!(i.b).leading_zeros() as u64),
                Op::I32Ctz => w!(i.a, r32!(i.b).trailing_zeros() as u64),
                Op::I32Popcnt => w!(i.a, r32!(i.b).count_ones() as u64),
                Op::I32Add => w!(i.a, r32!(i.b).wrapping_add(r32!(i.c)) as u64),
                Op::I32Sub => w!(i.a, r32!(i.b).wrapping_sub(r32!(i.c)) as u64),
                Op::I32Mul => w!(i.a, r32!(i.b).wrapping_mul(r32!(i.c)) as u64),
                Op::I32DivS => {
                    let (x, y) = (r32!(i.b) as i32, r32!(i.c) as i32);
                    if y == 0 {
                        break Err(Trap::IntegerDivideByZero);
                    }
                    if x == i32::MIN && y == -1 {
                        break Err(Trap::IntegerOverflow);
                    }
                    w!(i.a, (x / y) as u32 as u64)
                }
                Op::I32DivU => {
                    let (x, y) = (r32!(i.b), r32!(i.c));
                    if y == 0 {
                        break Err(Trap::IntegerDivideByZero);
                    }
                    w!(i.a, (x / y) as u64)
                }
                Op::I32RemS => {
                    let (x, y) = (r32!(i.b) as i32, r32!(i.c) as i32);
                    if y == 0 {
                        break Err(Trap::IntegerDivideByZero);
                    }
                    w!(i.a, x.wrapping_rem(y) as u32 as u64)
                }
                Op::I32RemU => {
                    let (x, y) = (r32!(i.b), r32!(i.c));
                    if y == 0 {
                        break Err(Trap::IntegerDivideByZero);
                    }
                    w!(i.a, (x % y) as u64)
                }
                Op::I32And => w!(i.a, (r32!(i.b) & r32!(i.c)) as u64),
                Op::I32Or => w!(i.a, (r32!(i.b) | r32!(i.c)) as u64),
                Op::I32Xor => w!(i.a, (r32!(i.b) ^ r32!(i.c)) as u64),
                Op::I32Shl => w!(i.a, r32!(i.b).wrapping_shl(r32!(i.c)) as u64),
                Op::I32ShrS => w!(i.a, (r32!(i.b) as i32).wrapping_shr(r32!(i.c)) as u32 as u64),
                Op::I32ShrU => w!(i.a, r32!(i.b).wrapping_shr(r32!(i.c)) as u64),
                Op::I32Rotl => w!(i.a, r32!(i.b).rotate_left(r32!(i.c) & 31) as u64),
                Op::I32Rotr => w!(i.a, r32!(i.b).rotate_right(r32!(i.c) & 31) as u64),

                // ---------------------------------------------------- i64
                Op::I64Eqz => w!(i.a, b2!(r!(i.b) == 0)),
                Op::I64Eq => w!(i.a, b2!(r!(i.b) == r!(i.c))),
                Op::I64Ne => w!(i.a, b2!(r!(i.b) != r!(i.c))),
                Op::I64LtS => w!(i.a, b2!((r!(i.b) as i64) < (r!(i.c) as i64))),
                Op::I64LtU => w!(i.a, b2!(r!(i.b) < r!(i.c))),
                Op::I64GtS => w!(i.a, b2!((r!(i.b) as i64) > (r!(i.c) as i64))),
                Op::I64GtU => w!(i.a, b2!(r!(i.b) > r!(i.c))),
                Op::I64LeS => w!(i.a, b2!((r!(i.b) as i64) <= (r!(i.c) as i64))),
                Op::I64LeU => w!(i.a, b2!(r!(i.b) <= r!(i.c))),
                Op::I64GeS => w!(i.a, b2!((r!(i.b) as i64) >= (r!(i.c) as i64))),
                Op::I64GeU => w!(i.a, b2!(r!(i.b) >= r!(i.c))),
                Op::I64Clz => w!(i.a, r!(i.b).leading_zeros() as u64),
                Op::I64Ctz => w!(i.a, r!(i.b).trailing_zeros() as u64),
                Op::I64Popcnt => w!(i.a, r!(i.b).count_ones() as u64),
                Op::I64Add => w!(i.a, r!(i.b).wrapping_add(r!(i.c))),
                Op::I64Sub => w!(i.a, r!(i.b).wrapping_sub(r!(i.c))),
                Op::I64Mul => w!(i.a, r!(i.b).wrapping_mul(r!(i.c))),
                Op::I64DivS => {
                    let (x, y) = (r!(i.b) as i64, r!(i.c) as i64);
                    if y == 0 {
                        break Err(Trap::IntegerDivideByZero);
                    }
                    if x == i64::MIN && y == -1 {
                        break Err(Trap::IntegerOverflow);
                    }
                    w!(i.a, (x / y) as u64)
                }
                Op::I64DivU => {
                    let (x, y) = (r!(i.b), r!(i.c));
                    if y == 0 {
                        break Err(Trap::IntegerDivideByZero);
                    }
                    w!(i.a, x / y)
                }
                Op::I64RemS => {
                    let (x, y) = (r!(i.b) as i64, r!(i.c) as i64);
                    if y == 0 {
                        break Err(Trap::IntegerDivideByZero);
                    }
                    w!(i.a, x.wrapping_rem(y) as u64)
                }
                Op::I64RemU => {
                    let (x, y) = (r!(i.b), r!(i.c));
                    if y == 0 {
                        break Err(Trap::IntegerDivideByZero);
                    }
                    w!(i.a, x % y)
                }
                Op::I64And => w!(i.a, r!(i.b) & r!(i.c)),
                Op::I64Or => w!(i.a, r!(i.b) | r!(i.c)),
                Op::I64Xor => w!(i.a, r!(i.b) ^ r!(i.c)),
                Op::I64Shl => w!(i.a, r!(i.b).wrapping_shl(r!(i.c) as u32)),
                Op::I64ShrS => w!(i.a, (r!(i.b) as i64).wrapping_shr(r!(i.c) as u32) as u64),
                Op::I64ShrU => w!(i.a, r!(i.b).wrapping_shr(r!(i.c) as u32)),
                Op::I64Rotl => w!(i.a, r!(i.b).rotate_left((r!(i.c) & 63) as u32)),
                Op::I64Rotr => w!(i.a, r!(i.b).rotate_right((r!(i.c) & 63) as u32)),

                // ---------------------------------------------------- f32
                Op::F32Eq => w!(i.a, b2!(rf32!(i.b) == rf32!(i.c))),
                Op::F32Ne => w!(i.a, b2!(rf32!(i.b) != rf32!(i.c))),
                Op::F32Lt => w!(i.a, b2!(rf32!(i.b) < rf32!(i.c))),
                Op::F32Gt => w!(i.a, b2!(rf32!(i.b) > rf32!(i.c))),
                Op::F32Le => w!(i.a, b2!(rf32!(i.b) <= rf32!(i.c))),
                Op::F32Ge => w!(i.a, b2!(rf32!(i.b) >= rf32!(i.c))),
                Op::F32Abs => w!(i.a, (r32!(i.b) & 0x7fff_ffff) as u64),
                Op::F32Neg => w!(i.a, (r32!(i.b) ^ 0x8000_0000) as u64),
                Op::F32Ceil => w!(i.a, q32(rf32!(i.b).ceil()).to_bits() as u64),
                Op::F32Floor => w!(i.a, q32(rf32!(i.b).floor()).to_bits() as u64),
                Op::F32Trunc => w!(i.a, q32(rf32!(i.b).trunc()).to_bits() as u64),
                Op::F32Nearest => w!(i.a, q32(f32_nearest(rf32!(i.b))).to_bits() as u64),
                Op::F32Sqrt => w!(i.a, rf32!(i.b).sqrt().to_bits() as u64),
                Op::F32Add => w!(i.a, (rf32!(i.b) + rf32!(i.c)).to_bits() as u64),
                Op::F32Sub => w!(i.a, (rf32!(i.b) - rf32!(i.c)).to_bits() as u64),
                Op::F32Mul => w!(i.a, (rf32!(i.b) * rf32!(i.c)).to_bits() as u64),
                Op::F32Div => w!(i.a, (rf32!(i.b) / rf32!(i.c)).to_bits() as u64),
                Op::F32Min => w!(i.a, f32_min(rf32!(i.b), rf32!(i.c)).to_bits() as u64),
                Op::F32Max => w!(i.a, f32_max(rf32!(i.b), rf32!(i.c)).to_bits() as u64),
                Op::F32Copysign => w!(i.a, ((r32!(i.b) & 0x7fff_ffff) | (r32!(i.c) & 0x8000_0000)) as u64),

                // ---------------------------------------------------- f64
                Op::F64Eq => w!(i.a, b2!(rf64!(i.b) == rf64!(i.c))),
                Op::F64Ne => w!(i.a, b2!(rf64!(i.b) != rf64!(i.c))),
                Op::F64Lt => w!(i.a, b2!(rf64!(i.b) < rf64!(i.c))),
                Op::F64Gt => w!(i.a, b2!(rf64!(i.b) > rf64!(i.c))),
                Op::F64Le => w!(i.a, b2!(rf64!(i.b) <= rf64!(i.c))),
                Op::F64Ge => w!(i.a, b2!(rf64!(i.b) >= rf64!(i.c))),
                Op::F64Abs => w!(i.a, r!(i.b) & 0x7fff_ffff_ffff_ffff),
                Op::F64Neg => w!(i.a, r!(i.b) ^ 0x8000_0000_0000_0000),
                Op::F64Ceil => w!(i.a, q64(rf64!(i.b).ceil()).to_bits()),
                Op::F64Floor => w!(i.a, q64(rf64!(i.b).floor()).to_bits()),
                Op::F64Trunc => w!(i.a, q64(rf64!(i.b).trunc()).to_bits()),
                Op::F64Nearest => w!(i.a, q64(f64_nearest(rf64!(i.b))).to_bits()),
                Op::F64Sqrt => w!(i.a, rf64!(i.b).sqrt().to_bits()),
                Op::F64Add => w!(i.a, (rf64!(i.b) + rf64!(i.c)).to_bits()),
                Op::F64Sub => w!(i.a, (rf64!(i.b) - rf64!(i.c)).to_bits()),
                Op::F64Mul => w!(i.a, (rf64!(i.b) * rf64!(i.c)).to_bits()),
                Op::F64Div => w!(i.a, (rf64!(i.b) / rf64!(i.c)).to_bits()),
                Op::F64Min => w!(i.a, f64_min(rf64!(i.b), rf64!(i.c)).to_bits()),
                Op::F64Max => w!(i.a, f64_max(rf64!(i.b), rf64!(i.c)).to_bits()),
                Op::F64Copysign => w!(i.a, (r!(i.b) & 0x7fff_ffff_ffff_ffff) | (r!(i.c) & 0x8000_0000_0000_0000)),

                // ---------------------------------------------------- conversions
                Op::I32WrapI64 => w!(i.a, r32!(i.b) as u64),
                Op::I32TruncF32S => {
                    let t = try_trap!(trunc_check(rf32!(i.b) as f64, I32_LO, I32_HI));
                    w!(i.a, t as i32 as u32 as u64)
                }
                Op::I32TruncF32U => {
                    let t = try_trap!(trunc_check(rf32!(i.b) as f64, -0.0, U32_HI));
                    w!(i.a, t as u32 as u64)
                }
                Op::I32TruncF64S => {
                    let t = try_trap!(trunc_check(rf64!(i.b), I32_LO, I32_HI));
                    w!(i.a, t as i32 as u32 as u64)
                }
                Op::I32TruncF64U => {
                    let t = try_trap!(trunc_check(rf64!(i.b), -0.0, U32_HI));
                    w!(i.a, t as u32 as u64)
                }
                Op::I64ExtendI32S => w!(i.a, r32!(i.b) as i32 as i64 as u64),
                Op::I64ExtendI32U => w!(i.a, r32!(i.b) as u64),
                Op::I64TruncF32S => {
                    let t = try_trap!(trunc_check(rf32!(i.b) as f64, I64_LO, I64_HI));
                    w!(i.a, t as i64 as u64)
                }
                Op::I64TruncF32U => {
                    let t = try_trap!(trunc_check(rf32!(i.b) as f64, -0.0, U64_HI));
                    w!(i.a, t as u64)
                }
                Op::I64TruncF64S => {
                    let t = try_trap!(trunc_check(rf64!(i.b), I64_LO, I64_HI));
                    w!(i.a, t as i64 as u64)
                }
                Op::I64TruncF64U => {
                    let t = try_trap!(trunc_check(rf64!(i.b), -0.0, U64_HI));
                    w!(i.a, t as u64)
                }
                Op::F32ConvertI32S => w!(i.a, (r32!(i.b) as i32 as f32).to_bits() as u64),
                Op::F32ConvertI32U => w!(i.a, (r32!(i.b) as f32).to_bits() as u64),
                Op::F32ConvertI64S => w!(i.a, (r!(i.b) as i64 as f32).to_bits() as u64),
                Op::F32ConvertI64U => w!(i.a, (r!(i.b) as f32).to_bits() as u64),
                Op::F32DemoteF64 => w!(i.a, (rf64!(i.b) as f32).to_bits() as u64),
                Op::F64ConvertI32S => w!(i.a, (r32!(i.b) as i32 as f64).to_bits()),
                Op::F64ConvertI32U => w!(i.a, (r32!(i.b) as f64).to_bits()),
                Op::F64ConvertI64S => w!(i.a, (r!(i.b) as i64 as f64).to_bits()),
                Op::F64ConvertI64U => w!(i.a, (r!(i.b) as f64).to_bits()),
                Op::F64PromoteF32 => w!(i.a, (rf32!(i.b) as f64).to_bits()),
                Op::I32Extend8S => w!(i.a, r!(i.b) as i8 as i32 as u32 as u64),
                Op::I32Extend16S => w!(i.a, r!(i.b) as i16 as i32 as u32 as u64),
                Op::I64Extend8S => w!(i.a, r!(i.b) as i8 as i64 as u64),
                Op::I64Extend16S => w!(i.a, r!(i.b) as i16 as i64 as u64),
                Op::I64Extend32S => w!(i.a, r!(i.b) as i32 as i64 as u64),
                Op::I32TruncSatF32S => w!(i.a, rf32!(i.b) as i32 as u32 as u64),
                Op::I32TruncSatF32U => w!(i.a, rf32!(i.b) as u32 as u64),
                Op::I32TruncSatF64S => w!(i.a, rf64!(i.b) as i32 as u32 as u64),
                Op::I32TruncSatF64U => w!(i.a, rf64!(i.b) as u32 as u64),
                Op::I64TruncSatF32S => w!(i.a, rf32!(i.b) as i64 as u64),
                Op::I64TruncSatF32U => w!(i.a, rf32!(i.b) as u64),
                Op::I64TruncSatF64S => w!(i.a, rf64!(i.b) as i64 as u64),
                Op::I64TruncSatF64U => w!(i.a, rf64!(i.b) as u64),

                // ---------------------------------------------------- SIMD
                Op::Simd => unsafe { crate::simd::exec(&*code, i, regs) },
                Op::Simd3 => unsafe { crate::simd::exec3(i, regs) },
                Op::Shuffle => unsafe { crate::simd::shuffle(&*code, i, regs) },
                Op::SimdLoad | Op::SimdStore => {
                    let ma = unsafe { (&*code).memargs[i.c as usize] };
                    let mem = unsafe { &(&*inst).memories[ma.memory as usize] };
                    try_trap!(unsafe { crate::simd::mem_op(mem, ma.offset, i, regs) });
                }
                Op::Gc => break Err(Trap::Other("unsupported instruction".into())),
            }
        };
        // A trap or exception.
        let err = res.unwrap_err();
        let Trap::Exception(exn) = err else {
            return Err(err);
        };
        // Unwind to a handler.
        let mut throw_pc = pc - 1;
        loop {
            let c = unsafe { &*code };
            let ins = unsafe { &*inst };
            if let Some(k) = find_handler(store, c, ins, throw_pc, exn) {
                let h = &c.handlers[k];
                let (payload, ref_bits) = {
                    let e = store.exns.borrow();
                    (e[exn as usize].payload.clone(), exn as u64 + 1)
                };
                let mut d = h.dst as usize;
                if matches!(h.kind, CatchKind::Catch | CatchKind::CatchRef) {
                    for v in &payload {
                        w!(d, *v);
                        d += 1;
                    }
                }
                if matches!(h.kind, CatchKind::CatchRef | CatchKind::CatchAllRef) {
                    w!(d, ref_bits);
                }
                if h.exn_slot != u32::MAX {
                    w!(h.exn_slot, ref_bits);
                }
                if h.target == u32::MAX - 1 {
                    // Caught to the function label: return.
                    store.depth.set(store.depth.get() - 1);
                    match frames.pop() {
                        None => return Ok(()),
                        Some(f) => {
                            code = f.code;
                            pc = f.pc;
                            fp = f.fp;
                            regs = unsafe { stack_ptr(store).add(fp) };
                            if f.inst != inst {
                                inst = f.inst;
                                (mem_p, mem_len) = mem0(inst);
                            }
                        }
                    }
                } else {
                    pc = h.target as usize;
                }
                break;
            }
            store.depth.set(store.depth.get() - 1);
            match frames.pop() {
                None => return Err(Trap::Exception(exn)),
                Some(f) => {
                    code = f.code;
                    throw_pc = f.pc - 1;
                    fp = f.fp;
                    regs = unsafe { stack_ptr(store).add(fp) };
                    if f.inst != inst {
                        inst = f.inst;
                        (mem_p, mem_len) = mem0(inst);
                    }
                }
            }
        }
    }
}
