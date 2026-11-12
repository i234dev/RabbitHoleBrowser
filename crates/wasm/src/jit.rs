//! Baseline template compiler (x86-64) for the register IR.
//!
//! Each IR instruction becomes a short machine-code template over the
//! frame slots (`[rbx + slot*8]`), as described in docs/jit-design.md.
//! Native code uses the interpreter's frame layout and value stack, so the
//! two mix freely:
//!
//! * Whole functions run natively. Instructions without a template (and
//!   the slow path of those with one: possible traps such as out-of-bounds
//!   accesses or division by zero) call [`helper_exec`], which runs the
//!   interpreter's own code for that one instruction (`exec_simple`), so
//!   their semantics are exactly the interpreter's.
//! * Calls between compiled functions of the same instance are native
//!   calls (`rbx` moves to the callee frame); any other call (imports, host
//!   functions, `call_indirect`, uncompiled callees) goes through
//!   [`helper_call`].
//! * Errors (traps, exceptions, interrupts) are boxed into the context and
//!   reported with status 1, which every native frame propagates by
//!   returning; the interpreter then handles them at the call site.
//! * Functions with exception handlers or tail calls stay interpreted.
//!
//! Registers: rbx = frame slots, r12 = linear memory 0 base, r13 = its
//! length, r14 = [`JitCtx`]. The entry trampoline uses the System V
//! convention on every x86-64 target (`extern "sysv64"`), and so do the
//! helpers, so the same code runs on Windows.

use crate::interp::{exec_simple, mem0};
use crate::ir::*;
use crate::runtime::*;
use std::cell::Cell;

/// State shared by native code, its helpers and the interpreter.
#[repr(C)]
pub struct JitCtx {
    pub regs: *mut u64,
    pub mem: *mut u8,
    pub mem_len: u64,
    /// Pointers to the value cells of the instance's globals.
    pub globals: *const *mut u64,
    /// Back-edges until the next interrupt poll.
    pub fuel: u64,
    /// Native entries of the instance's functions (null: not compiled).
    pub entries: *const Cell<*const u8>,
    pub store: *const Store,
    pub inst: *const Instance,
    /// `*mut &mut dyn Host`
    pub host: *mut u8,
    /// Error of a failed helper (boxed `Trap`).
    pub trap: *mut Trap,
    pub depth: *mut usize,
    pub max_depth: u64,
    /// End of the value stack.
    pub stack_end: *mut u64,
    /// Lowest native stack pointer allowed.
    pub native_limit: u64,
    pub fuel_reset: u64,
}

const CTX_MEM: i32 = 8;
const CTX_LEN: i32 = 16;
const CTX_GLOBALS: i32 = 24;
const CTX_FUEL: i32 = 32;
const CTX_ENTRIES: i32 = 40;
const CTX_DEPTH: i32 = 80;
const CTX_MAX_DEPTH: i32 = 88;
const CTX_STACK_END: i32 = 96;
const CTX_NATIVE_LIMIT: i32 = 104;

pub struct Compiled {
    mem: ExecMem,
    /// Offset of the internal entry (frame already in rbx).
    entry: usize,
}

impl std::fmt::Debug for Compiled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Compiled({} bytes)", self.mem.len)
    }
}

type Trampoline = unsafe extern "sysv64" fn(*mut JitCtx, *mut u64, *const u8) -> u32;

impl Compiled {
    pub fn entry(&self) -> *const u8 {
        // SAFETY: inside the allocation.
        unsafe { self.mem.ptr.add(self.entry) }
    }
}

/// Run compiled function `entry` with its frame at `regs`.
///
/// # Safety
/// `ctx` must describe the store, instance and memory of the function,
/// and the frame must hold its arguments.
pub unsafe fn call(ctx: &mut JitCtx, regs: *mut u64, entry: *const u8) -> Result<(), Trap> {
    let t = trampoline();
    let f: Trampoline = std::mem::transmute(t);
    let st = f(ctx, regs, entry);
    if st == 0 {
        Ok(())
    } else {
        Err(take_trap(ctx))
    }
}

fn take_trap(ctx: &mut JitCtx) -> Trap {
    if ctx.trap.is_null() {
        return Trap::Other("native code failed".into());
    }
    // SAFETY: set by `set_trap` from a Box.
    let t = unsafe { Box::from_raw(ctx.trap) };
    ctx.trap = std::ptr::null_mut();
    *t
}

fn set_trap(ctx: &mut JitCtx, t: Trap) -> u32 {
    if !ctx.trap.is_null() {
        // SAFETY: as above.
        drop(unsafe { Box::from_raw(ctx.trap) });
    }
    ctx.trap = Box::into_raw(Box::new(t));
    1
}

/// Is the compiler enabled (x86-64, `RH_WASM_JIT` not `0`)?
pub fn enabled() -> bool {
    thread_local! {
        static ON: bool = cfg!(target_arch = "x86_64") && std::env::var("RH_WASM_JIT").map(|v| v != "0").unwrap_or(true);
    }
    ON.with(|o| *o)
}

/// Native code of `code`, compiled on first use (None: interpreted).
pub fn compiled(code: &Code) -> Option<&Compiled> {
    if !enabled() {
        return None;
    }
    code.jit.get_or_init(|| compile(code)).as_ref()
}

// ------------------------------------------------------------------ helpers

/// Execute instruction `pc` of `code` in the interpreter (no control
/// transfer). Returns 0, or 1 with the trap in the context.
unsafe extern "sysv64" fn helper_exec(ctx: *mut JitCtx, code: *const Code, pc: u32, regs: *mut u64) -> u32 {
    let ctx = &mut *ctx;
    let store = &*ctx.store;
    let i = *(&*code).instrs.get_unchecked(pc as usize);
    #[cfg(feature = "opcount")]
    crate::interp::count_op(i.op);
    let mut mem_p = ctx.mem;
    let mut mem_len = ctx.mem_len as usize;
    let r = exec_simple(store, ctx.inst, code, i, regs, &mut mem_p, &mut mem_len);
    ctx.mem = mem_p;
    ctx.mem_len = mem_len as u64;
    match r {
        Ok(()) => 0,
        Err(t) => set_trap(ctx, t),
    }
}

/// `ref.test`-style branch condition of `BrOnCast`/`BrOnCastFail`.
unsafe extern "sysv64" fn helper_cast(ctx: *mut JitCtx, code: *const Code, pc: u32, regs: *mut u64) -> u32 {
    let ctx = &mut *ctx;
    let i = *(&*code).instrs.get_unchecked(pc as usize);
    let (h, nullable) = (&*code).casts[i.b as usize];
    let ok = crate::gc::ref_test(&*ctx.store, *regs.add(i.a as usize), h, nullable);
    (ok == (i.op == Op::BrOnCast)) as u32
}

/// Interrupt poll at a loop back-edge.
unsafe extern "sysv64" fn helper_poll(ctx: *mut JitCtx) -> u32 {
    let ctx = &mut *ctx;
    ctx.fuel = ctx.fuel_reset;
    let host = &mut **(ctx.host as *mut &mut dyn Host);
    if host.check_interrupt() {
        return set_trap(ctx, Trap::Interrupted);
    }
    0
}

/// Raise a trap: 0 = unreachable, 1 = call stack exhausted.
unsafe extern "sysv64" fn helper_trap(ctx: *mut JitCtx, kind: u32) -> u32 {
    let t = if kind == 0 { Trap::Unreachable } else { Trap::StackExhausted };
    set_trap(&mut *ctx, t)
}

/// Throw instructions (`throw`, `throw_ref`, legacy `rethrow`).
unsafe extern "sysv64" fn helper_throw(ctx: *mut JitCtx, code: *const Code, pc: u32, regs: *mut u64) -> u32 {
    let ctx = &mut *ctx;
    let store = &*ctx.store;
    let i = *(&*code).instrs.get_unchecked(pc as usize);
    let t = match i.op {
        Op::Throw => {
            let tag = (&*ctx.inst).tags[i.b as usize].clone();
            let n = tag.ft.param_slots() as usize;
            let payload: Vec<u64> = (0..n).map(|k| *regs.add(i.a as usize + k)).collect();
            Trap::Exception(store.new_exn(tag, payload))
        }
        _ => {
            let e = *regs.add(i.a as usize);
            if e == 0 {
                Trap::NullReference
            } else {
                Trap::Exception((e - 1) as u32)
            }
        }
    };
    set_trap(ctx, t)
}

/// Any call the native code does not do itself: resolve the callee like
/// the interpreter, then run it natively, in the interpreter or on the
/// host. Arguments are already in place at the call base.
unsafe extern "sysv64" fn helper_call(ctx: *mut JitCtx, code: *const Code, pc: u32, regs: *mut u64) -> u32 {
    let ctx = &mut *ctx;
    match do_call(ctx, &*code, pc, regs) {
        Ok(()) => {
            let (p, l) = mem0(ctx.inst);
            ctx.mem = p;
            ctx.mem_len = l as u64;
            0
        }
        Err(t) => {
            let (p, l) = mem0(ctx.inst);
            ctx.mem = p;
            ctx.mem_len = l as u64;
            set_trap(ctx, t)
        }
    }
}

unsafe fn do_call(ctx: &mut JitCtx, code: &Code, pc: u32, regs: *mut u64) -> Result<(), Trap> {
    let store = &*ctx.store;
    let ins = &*ctx.inst;
    let i = *code.instrs.get_unchecked(pc as usize);
    #[cfg(feature = "opcount")]
    crate::interp::count_op(i.op);
    let f: &FuncInst = match i.op {
        Op::Call | Op::CallArgs => &ins.funcs[i.b as usize],
        Op::CallIndirect => {
            let (canon, ps) = code.call_types[i.c as usize];
            let t = &ins.tables[i.b as usize];
            let raw = *regs.add((i.a + ps) as usize);
            let idx = if t.ty.table64 { raw } else { raw as u32 as u64 };
            let e = {
                let el = t.elems.borrow();
                if idx >= el.len() as u64 {
                    return Err(Trap::UndefinedElement);
                }
                el[idx as usize]
            };
            if e == 0 {
                return Err(Trap::UninitializedElement);
            }
            let f = store.func_ref((e - 1) as u32);
            if f.ty != canon && !crate::types::canonical_subtype(f.ty, canon) {
                return Err(Trap::IndirectCallTypeMismatch);
            }
            f
        }
        _ => {
            let (_, ps) = code.call_types[i.c as usize];
            let e = *regs.add((i.a + ps) as usize);
            if e == 0 {
                return Err(Trap::NullFunctionReference);
            }
            store.func_ref((e - 1) as u32)
        }
    };
    let new_regs = regs.add(i.a as usize);
    let probe = 0u8;
    if (&probe as *const u8 as u64) < ctx.native_limit {
        return Err(Trap::StackExhausted);
    }
    let host = &mut **(ctx.host as *mut &mut dyn Host);
    match &f.kind {
        FuncKind::Wasm { instance, code: c } => {
            if *instance == ins.index {
                if let Some(n) = compiled(c) {
                    if matches!(i.op, Op::Call | Op::CallArgs) {
                        ins.native[i.b as usize].set(n.entry());
                    }
                    let d = &mut *ctx.depth;
                    if *d >= ctx.max_depth as usize {
                        return Err(Trap::StackExhausted);
                    }
                    *d += 1;
                    let r = call(ctx, new_regs, n.entry());
                    *(ctx.depth) -= 1;
                    return r;
                }
            }
            // Another instance or an interpreted function: nested run.
            let base = new_regs.offset_from((*store.stack.get()).as_ptr()) as usize;
            let saved_top = store.stack_top.get();
            store.stack_top.set(base);
            let r = crate::interp::run_at(store, host, *instance, c, base);
            store.stack_top.set(saved_top);
            r
        }
        FuncKind::Host { .. } => {
            let ft = &f.ft;
            let mut args = Vec::with_capacity(ft.params.len());
            let mut off = 0usize;
            for &t in ft.params.iter() {
                args.push(crate::interp::read_val(new_regs.add(off), t));
                off += t.slots() as usize;
            }
            let saved_top = store.stack_top.get();
            let frame_end = regs.add(code.frame_slots as usize + 2).offset_from((*store.stack.get()).as_ptr()) as usize;
            store.stack_top.set(frame_end);
            let fr = store.func(f.addr);
            *ctx.depth += 1;
            let res = host.call_host(store, &fr, &args);
            *ctx.depth -= 1;
            store.stack_top.set(saved_top);
            let vals = res?;
            let mut off = 0usize;
            for (v, &t) in vals.iter().zip(ft.results.iter()) {
                crate::interp::write_val(new_regs.add(off), *v);
                off += t.slots() as usize;
            }
            Ok(())
        }
    }
}


// ------------------------------------------------------------------ executable memory

struct ExecMem {
    ptr: *mut u8,
    len: usize,
}

#[cfg(unix)]
mod sys {
    extern "C" {
        fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
        fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
        fn munmap(addr: *mut u8, len: usize) -> i32;
    }
    const PROT_READ: i32 = 1;
    const PROT_WRITE: i32 = 2;
    const PROT_EXEC: i32 = 4;
    const MAP_PRIVATE: i32 = 2;
    #[cfg(target_os = "macos")]
    const MAP_ANON: i32 = 0x1000;
    #[cfg(not(target_os = "macos"))]
    const MAP_ANON: i32 = 0x20;
    pub unsafe fn alloc(len: usize) -> Option<*mut u8> {
        let p = mmap(std::ptr::null_mut(), len.max(1), PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
        if p.is_null() || p as isize == -1 {
            return None;
        }
        Some(p)
    }
    pub unsafe fn protect(p: *mut u8, len: usize) -> bool {
        mprotect(p, len.max(1), PROT_READ | PROT_EXEC) == 0
    }
    pub unsafe fn free(p: *mut u8, len: usize) {
        munmap(p, len.max(1));
    }
}

#[cfg(windows)]
mod sys {
    #[link(name = "kernel32")]
    extern "system" {
        fn VirtualAlloc(addr: *mut u8, size: usize, ty: u32, protect: u32) -> *mut u8;
        fn VirtualProtect(addr: *mut u8, size: usize, protect: u32, old: *mut u32) -> i32;
        fn VirtualFree(addr: *mut u8, size: usize, ty: u32) -> i32;
        fn FlushInstructionCache(process: isize, addr: *const u8, size: usize) -> i32;
        fn GetCurrentProcess() -> isize;
    }
    const MEM_COMMIT: u32 = 0x1000;
    const MEM_RESERVE: u32 = 0x2000;
    const MEM_RELEASE: u32 = 0x8000;
    const PAGE_READWRITE: u32 = 0x04;
    const PAGE_EXECUTE_READ: u32 = 0x20;
    pub unsafe fn alloc(len: usize) -> Option<*mut u8> {
        let p = VirtualAlloc(std::ptr::null_mut(), len.max(1), MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
        if p.is_null() {
            None
        } else {
            Some(p)
        }
    }
    pub unsafe fn protect(p: *mut u8, len: usize) -> bool {
        let mut old = 0;
        let ok = VirtualProtect(p, len.max(1), PAGE_EXECUTE_READ, &mut old) != 0;
        FlushInstructionCache(GetCurrentProcess(), p, len);
        ok
    }
    pub unsafe fn free(p: *mut u8, _len: usize) {
        VirtualFree(p, 0, MEM_RELEASE);
    }
}

#[cfg(not(any(unix, windows)))]
mod sys {
    pub unsafe fn alloc(_len: usize) -> Option<*mut u8> {
        None
    }
    pub unsafe fn protect(_p: *mut u8, _len: usize) -> bool {
        false
    }
    pub unsafe fn free(_p: *mut u8, _len: usize) {}
}

impl Drop for ExecMem {
    fn drop(&mut self) {
        // SAFETY: allocated by `sys::alloc` with this length.
        unsafe { sys::free(self.ptr, self.len) }
    }
}

/// The shared entry trampoline: `(ctx, frame, entry) -> status`.
fn trampoline() -> *const u8 {
    thread_local! {
        static T: Option<ExecMem> = build_trampoline();
    }
    T.with(|t| t.as_ref().map(|m| m.ptr as *const u8).unwrap_or(std::ptr::null()))
}

fn build_trampoline() -> Option<ExecMem> {
    let mut a = Asm { buf: Vec::new() };
    for r in [RBP, RBX, R12, R13, R14, R15] {
        a.push(r);
    }
    a.sub_rsp8();
    a.mov_rr64(R14, RDI);
    a.mov_rr64(RBX, RSI);
    a.load64(R12, Mem { base: R14, index: None, disp: CTX_MEM });
    a.load64(R13, Mem { base: R14, index: None, disp: CTX_LEN });
    a.buf.extend_from_slice(&[0xff, 0xd2]); // call rdx
    a.add_rsp8();
    for r in [R15, R14, R13, R12, RBX, RBP] {
        a.pop(r);
    }
    a.b(0xc3);
    finish(a.buf, &[])
}

/// Map `code`, patch absolute addresses (`abs`: (position, offset)), make
/// it executable.
fn finish(code: Vec<u8>, abs: &[(usize, usize)]) -> Option<ExecMem> {
    let len = code.len();
    // SAFETY: fresh mapping of `len` bytes, written before protecting.
    unsafe {
        let p = sys::alloc(len)?;
        std::ptr::copy_nonoverlapping(code.as_ptr(), p, len);
        for &(at, off) in abs {
            let v = (p as usize + off) as u64;
            std::ptr::copy_nonoverlapping(v.to_le_bytes().as_ptr(), p.add(at), 8);
        }
        if !sys::protect(p, len) {
            sys::free(p, len);
            return None;
        }
        Some(ExecMem { ptr: p, len })
    }
}

// ------------------------------------------------------------------ assembler

const RAX: u8 = 0;
const RCX: u8 = 1;
const RDX: u8 = 2;
const RBX: u8 = 3;
const RSP: u8 = 4;
const RBP: u8 = 5;
const RSI: u8 = 6;
const RDI: u8 = 7;
const R12: u8 = 12;
const R13: u8 = 13;
const R14: u8 = 14;
const R15: u8 = 15;

const CC_B: u8 = 2;
const CC_AE: u8 = 3;
const CC_E: u8 = 4;
const CC_NE: u8 = 5;
const CC_BE: u8 = 6;
const CC_A: u8 = 7;
const CC_L: u8 = 12;
const CC_GE: u8 = 13;
const CC_LE: u8 = 14;
const CC_G: u8 = 15;

#[derive(Clone, Copy)]
struct Mem {
    base: u8,
    index: Option<u8>,
    disp: i32,
}

fn slot(s: u32) -> Mem {
    Mem { base: RBX, index: None, disp: (s * 8) as i32 }
}

fn ctxm(disp: i32) -> Mem {
    Mem { base: R14, index: None, disp }
}

struct Asm {
    buf: Vec<u8>,
}

impl Asm {
    fn b(&mut self, x: u8) {
        self.buf.push(x);
    }
    fn d32(&mut self, x: i32) {
        self.buf.extend_from_slice(&x.to_le_bytes());
    }
    fn pos(&self) -> usize {
        self.buf.len()
    }
    fn op_mem(&mut self, prefix: Option<u8>, w: bool, opcode: &[u8], reg: u8, m: Mem) {
        if let Some(p) = prefix {
            self.b(p);
        }
        let rex = 0x40 | (w as u8) << 3 | ((reg >> 3) & 1) << 2 | ((m.index.unwrap_or(0) >> 3) & 1) << 1 | ((m.base >> 3) & 1);
        if rex != 0x40 {
            self.b(rex);
        }
        self.buf.extend_from_slice(opcode);
        let (md, small) = if (-128..128).contains(&m.disp) { (0b01, true) } else { (0b10, false) };
        let need_sib = m.index.is_some() || m.base & 7 == 4;
        let rm = if need_sib { 4 } else { m.base & 7 };
        self.b(md << 6 | (reg & 7) << 3 | rm);
        if need_sib {
            let idx = m.index.map(|i| i & 7).unwrap_or(4);
            self.b(idx << 3 | (m.base & 7));
        }
        if small {
            self.b(m.disp as i8 as u8);
        } else {
            self.d32(m.disp);
        }
    }
    fn op_rr(&mut self, prefix: Option<u8>, w: bool, opcode: &[u8], reg: u8, rm: u8) {
        if let Some(p) = prefix {
            self.b(p);
        }
        let rex = 0x40 | (w as u8) << 3 | ((reg >> 3) & 1) << 2 | ((rm >> 3) & 1);
        if rex != 0x40 {
            self.b(rex);
        }
        self.buf.extend_from_slice(opcode);
        self.b(0b11 << 6 | (reg & 7) << 3 | (rm & 7));
    }
    fn load64(&mut self, r: u8, m: Mem) {
        self.op_mem(None, true, &[0x8b], r, m);
    }
    fn load32(&mut self, r: u8, m: Mem) {
        self.op_mem(None, false, &[0x8b], r, m);
    }
    fn store64(&mut self, m: Mem, r: u8) {
        self.op_mem(None, true, &[0x89], r, m);
    }
    fn mov_rr64(&mut self, dst: u8, src: u8) {
        self.op_rr(None, true, &[0x89], src, dst);
    }
    fn mov_imm64(&mut self, r: u8, v: u64) {
        self.b(0x48 | ((r >> 3) & 1));
        self.b(0xb8 + (r & 7));
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn mov_imm32(&mut self, r: u8, v: u32) {
        if r >= 8 {
            self.b(0x41);
        }
        self.b(0xb8 + (r & 7));
        self.d32(v as i32);
    }
    fn push(&mut self, r: u8) {
        if r >= 8 {
            self.b(0x41);
        }
        self.b(0x50 + (r & 7));
    }
    fn pop(&mut self, r: u8) {
        if r >= 8 {
            self.b(0x41);
        }
        self.b(0x58 + (r & 7));
    }
    fn sub_rsp8(&mut self) {
        self.buf.extend_from_slice(&[0x48, 0x83, 0xec, 0x08]);
    }
    fn add_rsp8(&mut self) {
        self.buf.extend_from_slice(&[0x48, 0x83, 0xc4, 0x08]);
    }
    fn test_eax(&mut self) {
        self.buf.extend_from_slice(&[0x85, 0xc0]);
    }
    fn setcc_eax(&mut self, cc: u8) {
        self.buf.extend_from_slice(&[0x0f, 0x90 + cc, 0xc0, 0x0f, 0xb6, 0xc0]);
    }
    fn jmp(&mut self) -> usize {
        self.b(0xe9);
        let p = self.pos();
        self.d32(0);
        p
    }
    fn jcc(&mut self, cc: u8) -> usize {
        self.b(0x0f);
        self.b(0x80 + cc);
        let p = self.pos();
        self.d32(0);
        p
    }
    fn patch(&mut self, at: usize, target: usize) {
        let rel = target as i64 - (at as i64 + 4);
        self.buf[at..at + 4].copy_from_slice(&(rel as i32).to_le_bytes());
    }
    fn sse(&mut self, prefix: u8, op: u8, xmm: u8, m: Mem) {
        self.op_mem(Some(prefix), false, &[0x0f, op], xmm, m);
    }
    /// call an absolute address (through rax)
    fn call_abs(&mut self, f: usize) {
        self.mov_imm64(RAX, f as u64);
        self.buf.extend_from_slice(&[0xff, 0xd0]);
    }
}

// ------------------------------------------------------------------ compiler

struct Fix {
    at: usize,
    target_pc: u32,
}

struct Ctx<'a> {
    a: Asm,
    code: &'a Code,
    fixes: Vec<Fix>,
    /// jumps to the error exit (status already in eax)
    bails: Vec<usize>,
    /// absolute address slots to patch: (position, code offset)
    abs: Vec<(usize, usize)>,
    /// jump tables: (table position, entries)
    tables: Vec<(usize, Vec<u32>)>,
    /// slow paths emitted out of line: (jump position, pc, kind)
    slow: Vec<(usize, u32, Slow)>,
}

#[derive(Clone, Copy)]
enum Slow {
    /// run the instruction in the interpreter (helper_exec)
    Exec,
    /// generic call (helper_call), then continue after the call site
    Call(usize),
    /// interrupt poll, then continue at the given code offset
    Poll(usize),
}

impl Ctx<'_> {
    /// Call `helper(ctx, code, pc, rbx)`; bail on a non-zero status;
    /// reload the memory registers.
    fn helper4(&mut self, f: usize, pc: u32) {
        self.a.mov_rr64(RDI, R14);
        self.a.mov_imm64(RSI, self.code as *const Code as u64);
        self.a.mov_imm32(RDX, pc);
        self.a.mov_rr64(RCX, RBX);
        self.a.call_abs(f);
        self.a.test_eax();
        let j = self.a.jcc(CC_NE);
        self.bails.push(j);
        self.reload_mem();
    }
    fn reload_mem(&mut self) {
        self.a.load64(R12, ctxm(CTX_MEM));
        self.a.load64(R13, ctxm(CTX_LEN));
    }
    fn branch(&mut self, pc: u32, target: u32) {
        if target <= pc {
            self.fuel_check();
        }
        let at = self.a.jmp();
        self.fixes.push(Fix { at, target_pc: target });
    }
    fn fuel_check(&mut self) {
        // dec qword [r14+fuel]; jz poll (which returns here)
        self.a.op_mem(None, true, &[0xff], 1, ctxm(CTX_FUEL));
        let j = self.a.jcc(CC_E);
        let back = self.a.pos();
        self.slow.push((j, 0, Slow::Poll(back)));
    }
    fn cond_branch(&mut self, pc: u32, cc: u8, target: u32) {
        if target > pc {
            let at = self.a.jcc(cc);
            self.fixes.push(Fix { at, target_pc: target });
        } else {
            let skip = self.a.jcc(cc ^ 1);
            self.branch(pc, target);
            let here = self.a.pos();
            self.a.patch(skip, here);
        }
    }
    fn exec_slow(&mut self, j: usize, pc: u32) {
        self.slow.push((j, pc, Slow::Exec));
    }
}

/// Can this function be compiled?
fn supported(code: &Code) -> bool {
    code.handlers.is_empty()
        && code.instrs.len() <= 200_000
        && code.frame_slots < (1 << 26)
        && !code.instrs.iter().any(|i| matches!(i.op, Op::ReturnCall | Op::ReturnCallIndirect | Op::ReturnCallRef))
}

/// Compile a function (None: interpreted).
pub fn compile(code: &Code) -> Option<Compiled> {
    if !cfg!(target_arch = "x86_64") || !supported(code) {
        return None;
    }
    let mut c = Ctx { a: Asm { buf: Vec::with_capacity(code.instrs.len() * 24 + 256) }, code, fixes: vec![], bails: vec![], abs: vec![], tables: vec![], slow: vec![] };
    // Internal entry: rbx = frame, rsp = 8 (mod 16).
    c.a.sub_rsp8();
    // Value stack bound: lea rax, [rbx + frame*8]; cmp rax, [stack_end]; ja overflow
    c.a.op_mem(None, true, &[0x8d], RAX, Mem { base: RBX, index: None, disp: (code.frame_slots as i32 + 2) * 8 });
    c.a.op_mem(None, true, &[0x3b], RAX, ctxm(CTX_STACK_END));
    let ov = c.a.jcc(CC_A);
    // Calls count against the interrupt fuel too (recursion without loops
    // stays interruptible).
    c.fuel_check();
    // Zero the declared locals, load the constants.
    let (ps, ls) = (code.param_slots, code.local_slots);
    if ls - ps <= 24 {
        for k in ps..ls {
            // mov qword [rbx+k*8], 0
            c.a.op_mem(None, true, &[0xc7], 0, slot(k));
            c.a.d32(0);
        }
    } else {
        // lea rdi, [rbx+ps*8]; mov ecx, n; xor eax, eax; rep stosq
        c.a.op_mem(None, true, &[0x8d], RDI, slot(ps));
        c.a.mov_imm32(RCX, ls - ps);
        c.a.buf.extend_from_slice(&[0x31, 0xc0, 0xf3, 0x48, 0xab]);
    }
    for (k, &v) in code.consts.iter().enumerate() {
        c.a.mov_imm64(RAX, v);
        c.a.store64(slot(code.const_base + k as u32), RAX);
    }
    let mut offsets = vec![0u32; code.instrs.len()];
    for (pc, i) in code.instrs.iter().enumerate() {
        offsets[pc] = c.a.pos() as u32;
        emit(&mut c, pc as u32, i);
    }
    // Falling off the end cannot happen (functions end with a return).
    c.a.b(0x0f);
    c.a.b(0x0b); // ud2
    // Stack overflow: helper_trap(ctx, 1)
    let here = c.a.pos();
    c.a.patch(ov, here);
    c.a.mov_rr64(RDI, R14);
    c.a.mov_imm32(RSI, 1);
    c.a.call_abs(helper_trap as *const () as usize);
    let j = c.a.jmp();
    c.bails.push(j);
    // Out-of-line slow paths.
    let slow = std::mem::take(&mut c.slow);
    for (j, pc, kind) in slow {
        let here = c.a.pos();
        c.a.patch(j, here);
        match kind {
            Slow::Exec => {
                c.helper4(helper_exec as *const () as usize, pc);
                // the interpreter ran the instruction: continue after it
                let at = c.a.jmp();
                c.fixes.push(Fix { at, target_pc: pc + 1 });
            }
            Slow::Call(back) => {
                c.helper4(helper_call as *const () as usize, pc);
                let at = c.a.jmp();
                c.a.patch(at, back);
            }
            Slow::Poll(back) => {
                c.a.mov_rr64(RDI, R14);
                c.a.call_abs(helper_poll as *const () as usize);
                c.a.test_eax();
                let j = c.a.jcc(CC_NE);
                c.bails.push(j);
                let at = c.a.jmp();
                c.a.patch(at, back);
            }
        }
    }
    // Error exit: status in eax.
    let bail = c.a.pos();
    for j in std::mem::take(&mut c.bails) {
        c.a.patch(j, bail);
    }
    c.a.add_rsp8();
    c.a.b(0xc3);
    for f in std::mem::take(&mut c.fixes) {
        let t = offsets[f.target_pc as usize] as usize;
        c.a.patch(f.at, t);
    }
    // Jump tables (absolute addresses).
    for (pos, entries) in std::mem::take(&mut c.tables) {
        let tpos = c.a.pos();
        c.abs.push((pos, tpos));
        for e in entries {
            let at = c.a.pos();
            c.a.buf.extend_from_slice(&[0u8; 8]);
            c.abs.push((at, offsets[e as usize] as usize));
        }
    }
    let mem = finish(c.a.buf, &c.abs)?;
    Some(Compiled { mem, entry: 0 })
}

fn int_cc(op: Op) -> Option<(bool, u8)> {
    use Op::*;
    Some(match op {
        I32Eq | BrI32Eq => (false, CC_E),
        I32Ne | BrI32Ne => (false, CC_NE),
        I32LtS | BrI32LtS => (false, CC_L),
        I32LtU | BrI32LtU => (false, CC_B),
        I32GtS | BrI32GtS => (false, CC_G),
        I32GtU | BrI32GtU => (false, CC_A),
        I32LeS | BrI32LeS => (false, CC_LE),
        I32LeU | BrI32LeU => (false, CC_BE),
        I32GeS | BrI32GeS => (false, CC_GE),
        I32GeU | BrI32GeU => (false, CC_AE),
        I64Eq | BrI64Eq => (true, CC_E),
        I64Ne | BrI64Ne => (true, CC_NE),
        I64LtS | BrI64LtS => (true, CC_L),
        I64LtU | BrI64LtU => (true, CC_B),
        I64GtS | BrI64GtS => (true, CC_G),
        I64GtU | BrI64GtU => (true, CC_A),
        I64LeS | BrI64LeS => (true, CC_LE),
        I64LeU | BrI64LeU => (true, CC_BE),
        I64GeS | BrI64GeS => (true, CC_GE),
        I64GeU | BrI64GeU => (true, CC_AE),
        _ => return None,
    })
}

/// Bounds-checked effective address of memory 0 in rax; out of bounds
/// goes to the interpreter (which traps). False: offset too large here.
fn mem_addr(c: &mut Ctx, pc: u32, addr: u32, offset: u32, size: u32) -> bool {
    let end = offset as u64 + size as u64;
    if end >= 1 << 31 {
        return false;
    }
    c.a.load32(RAX, slot(addr));
    c.a.op_mem(None, true, &[0x8d], RCX, Mem { base: RAX, index: None, disp: end as i32 });
    c.a.op_rr(None, true, &[0x3b], RCX, R13);
    let j = c.a.jcc(CC_A);
    c.exec_slow(j, pc);
    true
}

fn heap(offset: u32) -> Mem {
    Mem { base: R12, index: Some(RAX), disp: offset as i32 }
}

fn emit(c: &mut Ctx, pc: u32, i: &Instr) {
    if !emit_fast(c, pc, i) {
        // Run it in the interpreter.
        match i.op {
            Op::Unreachable => {
                c.a.mov_rr64(RDI, R14);
                c.a.mov_imm32(RSI, 0);
                c.a.call_abs(helper_trap as *const () as usize);
                let j = c.a.jmp();
                c.bails.push(j);
            }
            Op::Throw | Op::ThrowRef | Op::Rethrow => {
                c.helper4(helper_throw as *const () as usize, pc);
                c.a.mov_imm32(RAX, 1);
                let j = c.a.jmp();
                c.bails.push(j);
            }
            _ => c.helper4(helper_exec as *const () as usize, pc),
        }
    }
}

/// Emit an instruction natively; false: use the generic helper.
fn emit_fast(c: &mut Ctx, pc: u32, i: &Instr) -> bool {
    use Op::*;
    let (d, b, cc_) = (i.a, i.b, i.c);
    let code = c.code;
    match i.op {
        Return => {
            c.a.add_rsp8();
            c.a.buf.extend_from_slice(&[0x31, 0xc0, 0xc3]); // xor eax,eax; ret
        }
        Return1 => {
            c.a.load64(RAX, slot(d));
            c.a.store64(slot(0), RAX);
            c.a.add_rsp8();
            c.a.buf.extend_from_slice(&[0x31, 0xc0, 0xc3]);
        }
        Call | CallArgs => {
            if i.op == CallArgs {
                let srcs = &code.arg_lists[cc_ as usize..cc_ as usize + i.x as usize];
                for (k, &s) in srcs.iter().enumerate() {
                    if s != d + k as u32 {
                        c.a.load64(RAX, slot(s));
                        c.a.store64(slot(d + k as u32), RAX);
                    }
                }
            }
            // rax = native entry of the callee (null: generic call)
            if b as u64 * 8 >= 1 << 31 {
                return false;
            }
            c.a.load64(RAX, ctxm(CTX_ENTRIES));
            c.a.load64(RAX, Mem { base: RAX, index: None, disp: (b * 8) as i32 });
            c.a.test_eax64();
            let slow_j = c.a.jcc(CC_E);
            // depth: rcx = depth ptr; rdx = *rcx + 1; cmp rdx, max; ja slow
            c.a.load64(RCX, ctxm(CTX_DEPTH));
            c.a.load64(RDX, Mem { base: RCX, index: None, disp: 0 });
            c.a.op_mem(None, true, &[0x3b], RDX, ctxm(CTX_MAX_DEPTH));
            let slow_d = c.a.jcc(CC_AE);
            // native stack: cmp rsp, [limit]; jb slow
            c.a.op_mem(None, true, &[0x3b], RSP, ctxm(CTX_NATIVE_LIMIT));
            let slow_s = c.a.jcc(CC_B);
            c.a.op_mem(None, true, &[0xff], 0, Mem { base: RCX, index: None, disp: 0 }); // inc qword [rcx]
            c.a.push(RBX);
            c.a.sub_rsp8();
            c.a.op_mem(None, true, &[0x8d], RBX, slot(d));
            c.a.buf.extend_from_slice(&[0xff, 0xd0]); // call rax
            c.a.add_rsp8();
            c.a.pop(RBX);
            c.a.load64(RCX, ctxm(CTX_DEPTH));
            c.a.op_mem(None, true, &[0xff], 1, Mem { base: RCX, index: None, disp: 0 }); // dec qword [rcx]
            c.a.test_eax();
            let j = c.a.jcc(CC_NE);
            c.bails.push(j);
            c.reload_mem();
            let back = c.a.pos();
            for j in [slow_j, slow_d, slow_s] {
                c.slow.push((j, pc, Slow::Call(back)));
            }
        }
        CallIndirect | CallRef => {
            c.helper4(helper_call as *const () as usize, pc);
        }
        BrOnCast | BrOnCastFail => {
            c.a.mov_rr64(RDI, R14);
            c.a.mov_imm64(RSI, code as *const Code as u64);
            c.a.mov_imm32(RDX, pc);
            c.a.mov_rr64(RCX, RBX);
            c.a.call_abs(helper_cast as *const () as usize);
            c.a.test_eax();
            c.cond_branch(pc, CC_NE, cc_);
        }
        BrTable => {
            // eax = min(index, n-1); jmp [table + rax*8]
            let n = cc_;
            let entries: Vec<u32> = code.br_tables[b as usize..(b + n) as usize].to_vec();
            if entries.iter().any(|&t| t <= pc) {
                c.fuel_check();
            }
            c.a.load32(RAX, slot(d));
            c.a.mov_imm32(RCX, n - 1);
            c.a.op_rr(None, false, &[0x3b], RAX, RCX); // cmp eax, ecx
            c.a.op_rr(None, false, &[0x0f, 0x47], RAX, RCX); // cmova eax, ecx
            c.a.b(0x48);
            c.a.b(0xb9); // mov rcx, imm64 (table address, patched)
            let pos = c.a.pos();
            c.a.buf.extend_from_slice(&[0u8; 8]);
            c.tables.push((pos, entries));
            // jmp [rcx + rax*8]
            c.a.buf.extend_from_slice(&[0xff, 0x24, 0xc1]);
        }
        Copy => {
            c.a.load64(RAX, slot(b));
            c.a.store64(slot(d), RAX);
        }
        Copy2 => {
            c.a.load64(RAX, slot(b));
            c.a.load64(RCX, slot(b + 1));
            c.a.store64(slot(d), RAX);
            c.a.store64(slot(d + 1), RCX);
        }
        Const => {
            c.a.mov_imm64(RAX, b as u64 | (cc_ as u64) << 32);
            c.a.store64(slot(d), RAX);
        }
        Select => {
            c.a.op_mem(None, false, &[0x83], 7, slot(cc_));
            c.a.b(0);
            let skip = c.a.jcc(CC_NE);
            c.a.load64(RAX, slot(b));
            c.a.store64(slot(d), RAX);
            let here = c.a.pos();
            c.a.patch(skip, here);
        }
        GlobalGet | GlobalSet => {
            if b as u64 * 8 >= 1 << 31 {
                return false;
            }
            c.a.load64(RCX, ctxm(CTX_GLOBALS));
            c.a.load64(RCX, Mem { base: RCX, index: None, disp: (b * 8) as i32 });
            if i.op == GlobalGet {
                c.a.load64(RAX, Mem { base: RCX, index: None, disp: 0 });
                c.a.store64(slot(d), RAX);
            } else {
                c.a.load64(RAX, slot(d));
                c.a.store64(Mem { base: RCX, index: None, disp: 0 }, RAX);
            }
        }
        I32Add | I32Sub | I32And | I32Or | I32Xor | I32Mul | I64Add | I64Sub | I64And | I64Or | I64Xor | I64Mul => {
            let w = matches!(i.op, I64Add | I64Sub | I64And | I64Or | I64Xor | I64Mul);
            if w {
                c.a.load64(RAX, slot(b));
            } else {
                c.a.load32(RAX, slot(b));
            }
            let op: &[u8] = match i.op {
                I32Add | I64Add => &[0x03],
                I32Sub | I64Sub => &[0x2b],
                I32And | I64And => &[0x23],
                I32Or | I64Or => &[0x0b],
                I32Xor | I64Xor => &[0x33],
                _ => &[0x0f, 0xaf],
            };
            c.a.op_mem(None, w, op, RAX, slot(cc_));
            c.a.store64(slot(d), RAX);
        }
        I32Shl | I32ShrS | I32ShrU | I32Rotl | I32Rotr | I64Shl | I64ShrS | I64ShrU | I64Rotl | I64Rotr => {
            let w = matches!(i.op, I64Shl | I64ShrS | I64ShrU | I64Rotl | I64Rotr);
            if w {
                c.a.load64(RAX, slot(b));
            } else {
                c.a.load32(RAX, slot(b));
            }
            c.a.load32(RCX, slot(cc_));
            let ext = match i.op {
                I32Shl | I64Shl => 4,
                I32ShrU | I64ShrU => 5,
                I32ShrS | I64ShrS => 7,
                I32Rotl | I64Rotl => 0,
                _ => 1,
            };
            c.a.op_rr(None, w, &[0xd3], ext, RAX);
            c.a.store64(slot(d), RAX);
        }
        I32Eqz | I64Eqz => {
            c.a.op_mem(None, i.op == I64Eqz, &[0x83], 7, slot(b));
            c.a.b(0);
            c.a.setcc_eax(CC_E);
            c.a.store64(slot(d), RAX);
        }
        I32Eq | I32Ne | I32LtS | I32LtU | I32GtS | I32GtU | I32LeS | I32LeU | I32GeS | I32GeU | I64Eq | I64Ne | I64LtS | I64LtU | I64GtS | I64GtU | I64LeS | I64LeU | I64GeS | I64GeU => {
            let (w, cc) = int_cc(i.op).unwrap();
            if w {
                c.a.load64(RAX, slot(b));
            } else {
                c.a.load32(RAX, slot(b));
            }
            c.a.op_mem(None, w, &[0x3b], RAX, slot(cc_));
            c.a.setcc_eax(cc);
            c.a.store64(slot(d), RAX);
        }
        I32DivU | I32RemU | I64DivU | I64RemU | I32DivS | I32RemS | I64DivS | I64RemS => {
            let w = matches!(i.op, I64DivU | I64RemU | I64DivS | I64RemS);
            let signed = matches!(i.op, I32DivS | I32RemS | I64DivS | I64RemS);
            // divisor in rcx; zero (and MIN / -1 for signed) -> interpreter
            if w {
                c.a.load64(RCX, slot(cc_));
            } else {
                c.a.load32(RCX, slot(cc_));
            }
            c.a.op_rr(None, w, &[0x85], RCX, RCX); // test rcx, rcx
            let j0 = c.a.jcc(CC_E);
            c.exec_slow(j0, pc);
            if signed {
                // cmp rcx, -1; je slow (overflow / rem of MIN by -1)
                c.a.op_rr(None, w, &[0x83], 7, RCX);
                c.a.b(0xff);
                let j1 = c.a.jcc(CC_E);
                c.exec_slow(j1, pc);
            }
            if w {
                c.a.load64(RAX, slot(b));
            } else {
                c.a.load32(RAX, slot(b));
            }
            if signed {
                if w {
                    c.a.buf.extend_from_slice(&[0x48, 0x99]); // cqo
                } else {
                    c.a.b(0x99); // cdq
                }
                c.a.op_rr(None, w, &[0xf7], 7, RCX); // idiv
            } else {
                c.a.buf.extend_from_slice(&[0x31, 0xd2]); // xor edx, edx
                c.a.op_rr(None, w, &[0xf7], 6, RCX); // div
            }
            let r = if matches!(i.op, I32RemU | I64RemU | I32RemS | I64RemS) { RDX } else { RAX };
            if !w {
                // zero-extend the 32-bit result
                c.a.op_rr(None, false, &[0x89], r, r);
            }
            c.a.store64(slot(d), r);
        }
        I32WrapI64 | I64ExtendI32U => {
            c.a.load32(RAX, slot(b));
            c.a.store64(slot(d), RAX);
        }
        I64ExtendI32S | I64Extend32S => {
            c.a.op_mem(None, true, &[0x63], RAX, slot(b));
            c.a.store64(slot(d), RAX);
        }
        I32Extend8S | I32Extend16S | I64Extend8S | I64Extend16S => {
            let w = matches!(i.op, I64Extend8S | I64Extend16S);
            let op = if matches!(i.op, I32Extend8S | I64Extend8S) { 0xbe } else { 0xbf };
            c.a.op_mem(None, w, &[0x0f, op], RAX, slot(b));
            if !w {
                c.a.op_rr(None, false, &[0x89], RAX, RAX);
            }
            c.a.store64(slot(d), RAX);
        }
        I32Load | I64Load | I32Load8S | I32Load8U | I32Load16S | I32Load16U | I64Load8S | I64Load8U | I64Load16S | I64Load16U | I64Load32S | I64Load32U => {
            let size = match i.op {
                I64Load => 8,
                I32Load | I64Load32S | I64Load32U => 4,
                I32Load16S | I32Load16U | I64Load16S | I64Load16U => 2,
                _ => 1,
            };
            if !mem_addr(c, pc, b, cc_, size) {
                return false;
            }
            let m = heap(cc_);
            match i.op {
                I32Load | I64Load32U => c.a.op_mem(None, false, &[0x8b], RAX, m),
                I64Load => c.a.op_mem(None, true, &[0x8b], RAX, m),
                I32Load8U | I64Load8U => c.a.op_mem(None, false, &[0x0f, 0xb6], RAX, m),
                I32Load16U | I64Load16U => c.a.op_mem(None, false, &[0x0f, 0xb7], RAX, m),
                I32Load8S => {
                    c.a.op_mem(None, false, &[0x0f, 0xbe], RAX, m);
                    c.a.op_rr(None, false, &[0x89], RAX, RAX);
                }
                I32Load16S => {
                    c.a.op_mem(None, false, &[0x0f, 0xbf], RAX, m);
                    c.a.op_rr(None, false, &[0x89], RAX, RAX);
                }
                I64Load8S => c.a.op_mem(None, true, &[0x0f, 0xbe], RAX, m),
                I64Load16S => c.a.op_mem(None, true, &[0x0f, 0xbf], RAX, m),
                _ => c.a.op_mem(None, true, &[0x63], RAX, m),
            }
            c.a.store64(slot(d), RAX);
        }
        I32Store | I64Store | I32Store8 | I32Store16 | I64Store32 => {
            let size = match i.op {
                I64Store => 8,
                I32Store | I64Store32 => 4,
                I32Store16 => 2,
                _ => 1,
            };
            if !mem_addr(c, pc, d, cc_, size) {
                return false;
            }
            c.a.load64(RDX, slot(b));
            let m = heap(cc_);
            match size {
                8 => c.a.op_mem(None, true, &[0x89], RDX, m),
                4 => c.a.op_mem(None, false, &[0x89], RDX, m),
                2 => c.a.op_mem(Some(0x66), false, &[0x89], RDX, m),
                _ => c.a.op_mem(None, false, &[0x88], RDX, m),
            }
        }
        Br => c.branch(pc, cc_),
        BrIfNez | BrIfEqz => {
            c.a.op_mem(None, false, &[0x83], 7, slot(d));
            c.a.b(0);
            c.cond_branch(pc, if i.op == BrIfNez { CC_NE } else { CC_E }, cc_);
        }
        BrI64Eqz | BrI64Nez | BrOnNull | BrOnNonNull => {
            c.a.op_mem(None, true, &[0x83], 7, slot(d));
            c.a.b(0);
            let cc = if matches!(i.op, BrI64Eqz | BrOnNull) { CC_E } else { CC_NE };
            c.cond_branch(pc, cc, cc_);
        }
        BrI32Eq | BrI32Ne | BrI32LtS | BrI32LtU | BrI32GtS | BrI32GtU | BrI32LeS | BrI32LeU | BrI32GeS | BrI32GeU | BrI64Eq | BrI64Ne | BrI64LtS | BrI64LtU | BrI64GtS | BrI64GtU | BrI64LeS | BrI64LeU | BrI64GeS | BrI64GeU => {
            let (w, cc) = int_cc(i.op).unwrap();
            if w {
                c.a.load64(RAX, slot(d));
            } else {
                c.a.load32(RAX, slot(d));
            }
            c.a.op_mem(None, w, &[0x3b], RAX, slot(b));
            c.cond_branch(pc, cc, cc_);
        }
        F64Add | F64Sub | F64Mul | F64Div | F32Add | F32Sub | F32Mul | F32Div => {
            let (pre, op) = match i.op {
                F64Add => (0xf2, 0x58),
                F64Sub => (0xf2, 0x5c),
                F64Mul => (0xf2, 0x59),
                F64Div => (0xf2, 0x5e),
                F32Add => (0xf3, 0x58),
                F32Sub => (0xf3, 0x5c),
                F32Mul => (0xf3, 0x59),
                _ => (0xf3, 0x5e),
            };
            c.a.sse(pre, 0x10, 0, slot(b));
            c.a.sse(pre, op, 0, slot(cc_));
            if pre == 0xf3 {
                c.a.op_rr(Some(0x66), false, &[0x0f, 0x7e], 0, RAX); // movd eax, xmm0
                c.a.store64(slot(d), RAX);
            } else {
                c.a.sse(pre, 0x11, 0, slot(d));
            }
        }
        F64Sqrt => {
            c.a.sse(0xf2, 0x51, 0, slot(b));
            c.a.sse(0xf2, 0x11, 0, slot(d));
        }
        F64Neg | F64Abs => {
            c.a.load64(RAX, slot(b));
            c.a.op_rr(None, true, &[0x0f, 0xba], if i.op == F64Neg { 7 } else { 6 }, RAX);
            c.a.b(63);
            c.a.store64(slot(d), RAX);
        }
        F32Neg | F32Abs => {
            c.a.load32(RAX, slot(b));
            c.a.op_rr(None, false, &[0x0f, 0xba], if i.op == F32Neg { 7 } else { 6 }, RAX);
            c.a.b(31);
            c.a.store64(slot(d), RAX);
        }
        F64Eq | F64Ne | F64Lt | F64Gt | F64Le | F64Ge | F32Eq | F32Ne | F32Lt | F32Gt | F32Le | F32Ge => {
            let dbl = matches!(i.op, F64Eq | F64Ne | F64Lt | F64Gt | F64Le | F64Ge);
            let (ld, cmp_pre) = if dbl { (0xf2, Some(0x66)) } else { (0xf3, None) };
            let (x, y) = match i.op {
                F64Lt | F64Le | F32Lt | F32Le => (cc_, b),
                _ => (b, cc_),
            };
            c.a.sse(ld, 0x10, 0, slot(x));
            c.a.op_mem(cmp_pre, false, &[0x0f, 0x2e], 0, slot(y));
            match i.op {
                F64Eq | F32Eq => c.a.buf.extend_from_slice(&[0x0f, 0x94, 0xc0, 0x0f, 0x9b, 0xc1, 0x20, 0xc8, 0x0f, 0xb6, 0xc0]),
                F64Ne | F32Ne => c.a.buf.extend_from_slice(&[0x0f, 0x95, 0xc0, 0x0f, 0x9a, 0xc1, 0x08, 0xc8, 0x0f, 0xb6, 0xc0]),
                F64Lt | F64Gt | F32Lt | F32Gt => c.a.setcc_eax(CC_A),
                _ => c.a.setcc_eax(CC_AE),
            }
            c.a.store64(slot(d), RAX);
        }
        F64ConvertI32S | F64ConvertI64S | F32ConvertI32S | F32ConvertI64S => {
            let w = matches!(i.op, F64ConvertI64S | F32ConvertI64S);
            let pre = if matches!(i.op, F64ConvertI32S | F64ConvertI64S) { 0xf2 } else { 0xf3 };
            c.a.buf.extend_from_slice(&[0x66, 0x0f, 0xef, 0xc0]); // pxor xmm0, xmm0
            c.a.op_mem(Some(pre), w, &[0x0f, 0x2a], 0, slot(b));
            if pre == 0xf3 {
                c.a.op_rr(Some(0x66), false, &[0x0f, 0x7e], 0, RAX);
                c.a.store64(slot(d), RAX);
            } else {
                c.a.sse(0xf2, 0x11, 0, slot(d));
            }
        }
        I32Clz | I64Clz => {
            // bits-1 - bsr(x); x == 0: bsr leaves ZF set -> -1 -> bits
            let w = i.op == I64Clz;
            c.a.mov_imm64(RCX, u64::MAX);
            c.a.op_mem(None, w, &[0x0f, 0xbd], RAX, slot(b)); // bsr
            c.a.op_rr(None, w, &[0x0f, 0x44], RAX, RCX); // cmovz
            c.a.mov_imm32(RDX, if w { 63 } else { 31 });
            c.a.op_rr(None, w, &[0x2b], RDX, RAX); // sub rdx, rax
            if !w {
                c.a.op_rr(None, false, &[0x89], RDX, RDX);
            }
            c.a.store64(slot(d), RDX);
        }
        I32Ctz | I64Ctz => {
            let w = i.op == I64Ctz;
            c.a.mov_imm32(RCX, if w { 64 } else { 32 });
            c.a.op_mem(None, w, &[0x0f, 0xbc], RAX, slot(b)); // bsf
            c.a.op_rr(None, w, &[0x0f, 0x44], RAX, RCX); // cmovz
            c.a.op_rr(None, false, &[0x89], RAX, RAX);
            c.a.store64(slot(d), RAX);
        }
        MemoryCopy | MemoryFill if code.mem0_32 && b == 0 && (i.op == MemoryFill || cc_ == 0) => {
            // operands at d, d+1, d+2 (u32); bounds -> interpreter (traps)
            let base = d;
            c.a.load32(RDI, slot(base));
            c.a.load32(RCX, slot(base + 2));
            c.a.op_mem(None, true, &[0x8d], RAX, Mem { base: RDI, index: Some(RCX), disp: 0 }); // lea rax,[rdi+rcx]
            c.a.op_rr(None, true, &[0x3b], RAX, R13);
            let j = c.a.jcc(CC_A);
            c.exec_slow(j, pc);
            if i.op == MemoryFill {
                c.a.load32(RAX, slot(base + 1));
                c.a.op_rr(None, true, &[0x03], RDI, R12); // add rdi, r12
                c.a.buf.extend_from_slice(&[0xf3, 0xaa]); // rep stosb
            } else {
                c.a.load32(RSI, slot(base + 1));
                c.a.op_mem(None, true, &[0x8d], RAX, Mem { base: RSI, index: Some(RCX), disp: 0 });
                c.a.op_rr(None, true, &[0x3b], RAX, R13);
                let j = c.a.jcc(CC_A);
                c.exec_slow(j, pc);
                // overlapping with dst after src: copy backwards (helper)
                c.a.op_rr(None, true, &[0x3b], RDI, RSI); // cmp rdi, rsi
                let fwd = c.a.jcc(CC_BE);
                c.a.op_rr(None, true, &[0x3b], RDI, RAX); // cmp rdi, rsi+n
                let fwd2 = c.a.jcc(CC_AE);
                let back = c.a.jmp();
                c.exec_slow(back, pc);
                let here = c.a.pos();
                c.a.patch(fwd, here);
                c.a.patch(fwd2, here);
                c.a.op_rr(None, true, &[0x03], RDI, R12);
                c.a.op_rr(None, true, &[0x03], RSI, R12);
                c.a.buf.extend_from_slice(&[0xf3, 0xa4]); // rep movsb
            }
        }
        F64PromoteF32 => {
            c.a.buf.extend_from_slice(&[0x66, 0x0f, 0xef, 0xc0]);
            c.a.sse(0xf3, 0x5a, 0, slot(b));
            c.a.sse(0xf2, 0x11, 0, slot(d));
        }
        _ => return false,
    }
    let _ = (R15, RBP);
    true
}

impl Asm {
    fn test_eax64(&mut self) {
        self.buf.extend_from_slice(&[0x48, 0x85, 0xc0]);
    }
}

/// Native entry cells of an instance's functions.
pub fn entry_cells(n: usize) -> Box<[Cell<*const u8>]> {
    (0..n).map(|_| Cell::new(std::ptr::null())).collect()
}

