//! The boundary between native code, the run loop and the runtime.
//!
//! Native code calls helpers `extern "sysv64" fn(ctx, pc) -> status` (the
//! System V convention on every x86-64 target, so Windows needs no second
//! calling convention). A helper decodes `code.ops[pc]` itself, syncs the
//! frame's `pc` exactly like the interpreter does before anything that
//! can throw or call, runs the interpreter's own code for the instruction
//! and reports what native code does next. Helpers never unwind: a panic
//! is caught, stored in the realm, and resumed by [`Realm::jit_run`] once
//! no native frame is on the stack.

use super::*;
use crate::bytecode::Op;
use crate::value::Value;
use crate::vm::Ctl;

/// Context of a native activation (`rbx` in native code).
#[repr(C)]
pub(crate) struct Ctx {
    /// Base of the frame's slots; helpers rewrite it (the value stack may
    /// have moved) and native code reloads it after each helper.
    pub slots: *mut Value,
    pub consts: *const Value,
    pub realm: *mut Realm,
    pub tick: *mut u32,
    pub code: *const FunctionCode,
    /// Instruction to continue at after [`ST_JUMP`].
    pub pc: u32,
}

pub(crate) const CTX_SLOTS: i32 = 0;
pub(crate) const CTX_CONSTS: i32 = 8;
pub(crate) const CTX_TICK: i32 = 24;
pub(crate) const CTX_PC: i32 = 40;

/// Continue with the instruction's continuation in native code.
pub(crate) const ST_NEXT: u32 = 0;
/// Frames were pushed or popped: back to the run loop (exit native code).
pub(crate) const ST_RELOAD: u32 = 1;
/// The run loop is over (`loop_end` holds its result).
pub(crate) const ST_DONE: u32 = 2;
/// Continue in the interpreter at the frame's pc.
pub(crate) const ST_INTERP: u32 = 3;
/// A helper panicked (payload in `Realm::jit`).
pub(crate) const ST_PANIC: u32 = 4;
/// Continue native code at `ctx.pc` (the instruction jumped).
pub(crate) const ST_JUMP: u32 = 5;

/// Entry of a compiled function: `(ctx, pc) -> status` (never `ST_NEXT`
/// or `ST_JUMP`).
pub(crate) type Entry = unsafe extern "sysv64" fn(*mut Ctx, u32) -> u32;

impl Realm {
    /// Run-loop hook at each (re)load of the running frame (`pc` is its
    /// resume point): count fresh activations, compile hot code, and run
    /// native code when there is some. `Ctl::Next`: no native code ran;
    /// interpret the frame from `pc`.
    #[inline(never)]
    pub(crate) fn jit_reload(&mut self, code: &FunctionCode, pc: usize) -> Ctl {
        let s = &code.jit.s;
        let mut e = s.entry.get();
        if e == 0 {
            if pc != 0 {
                return Ctl::Next;
            }
            let n = s.counter.get().saturating_add(1);
            s.counter.set(n);
            if n < self.jit.threshold || s.state.get() != UNTRIED || !self.jit.on {
                return Ctl::Next;
            }
            compile(code, &mut self.jit.stats);
            e = s.entry.get();
            if e == 0 {
                return Ctl::Next;
            }
        }
        self.jit_run(code, e, pc)
    }

    /// Enter native code `entry` of `code` (the last frame's) at `pc`.
    #[inline(never)]
    pub(crate) fn jit_run(&mut self, code: &FunctionCode, entry: usize, pc: usize) -> Ctl {
        self.jit.entries += 1;
        if self.jit.stats.enabled {
            self.jit.stats.entries += 1;
        }
        // SAFETY: the run loop has a frame; its slots are within `vstack`.
        let slots = unsafe {
            let b = self.frames.last().unwrap_unchecked().base as usize;
            self.vstack.as_mut_ptr().add(b)
        };
        // A frame without a closure (script, eval) owns its code: returning
        // from it frees the code, native code included, while that code
        // still has to run its epilogue. Keep it until native code is done.
        // (A closure's code is owned by the closure, which stays alive: no
        // collection happens between a return and the native exit.)
        // SAFETY: as above.
        let _keep = unsafe { self.frames.last().unwrap_unchecked() }.extra.as_ref().and_then(|e| e.code.clone());
        let realm: *mut Realm = self;
        let mut ctx = Ctx {
            slots,
            consts: code.constants.as_ptr(),
            realm,
            // SAFETY: `realm` is valid; the field is only accessed through
            // this pointer (and `realm`) while native code runs.
            tick: unsafe { std::ptr::addr_of_mut!((*realm).tick) },
            code,
            pc: 0,
        };
        // SAFETY: `entry` is the native code of `code` (kept alive by the
        // frame's code), entered at an instruction of it; from here until
        // it returns, the realm is only used through `ctx.realm`.
        let st = unsafe {
            let f: Entry = std::mem::transmute(entry);
            f(&mut ctx, pc as u32)
        };
        match st {
            ST_RELOAD => Ctl::Reload,
            ST_DONE => Ctl::Done,
            ST_PANIC => match self.jit.panic.take() {
                Some(p) => std::panic::resume_unwind(p),
                None => panic!("native code panicked"),
            },
            _ => {
                // Native code declined to continue (an entry outside the
                // function): interpret this function from now on.
                code.jit.s.entry.set(0);
                code.jit.s.state.set(REJECTED);
                Ctl::Reload
            }
        }
    }
}

/// Run a helper body with the realm, catching panics, and refresh
/// `ctx.slots` when native code continues.
#[inline(always)]
unsafe fn guard(ctx: *mut Ctx, f: impl FnOnce(&mut Realm, *mut Ctx) -> u32) -> u32 {
    let realm = (*ctx).realm;
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut *realm, ctx))) {
        Ok(st) => {
            if st == ST_NEXT || st == ST_JUMP {
                let r = &mut *realm;
                let b = r.frames.last().map(|f| f.base as usize).unwrap_or(0);
                (*ctx).slots = r.vstack.as_mut_ptr().add(b);
            }
            st
        }
        Err(p) => {
            (*realm).jit.panic = Some(p);
            ST_PANIC
        }
    }
}

/// Status for native code after an out-of-line step of the instruction at
/// `pc`: continue with the next one, or wherever the instruction jumped.
#[inline(always)]
fn status(r: &mut Realm, ctx: *mut Ctx, c: Ctl, pc: u32) -> u32 {
    match c {
        Ctl::Next => {
            // SAFETY: native code runs the last frame.
            let fpc = unsafe { r.frames.last().unwrap_unchecked() }.pc;
            if fpc == pc as usize + 1 {
                ST_NEXT
            } else {
                if fpc <= pc as usize {
                    // A backward jump taken out of line ticks like one in
                    // native code (the watchdog must see every loop).
                    r.tick = r.tick.wrapping_sub(1);
                    if r.tick == 0 {
                        match r.tick_slow_jit(fpc) {
                            Ctl::Next => {}
                            Ctl::Reload => return ST_RELOAD,
                            Ctl::Done => return ST_DONE,
                        }
                    }
                }
                // SAFETY: `ctx` is the live context.
                unsafe { (*ctx).pc = fpc as u32 };
                ST_JUMP
            }
        }
        Ctl::Reload => ST_RELOAD,
        Ctl::Done => ST_DONE,
    }
}

/// The instruction at `pc` of the running code.
#[inline(always)]
unsafe fn op_at(ctx: *mut Ctx, pc: u32) -> Op {
    let code: &FunctionCode = &*(*ctx).code;
    *code.ops.get_unchecked(pc as usize)
}

/// Generic helper: the instruction at `pc`, completely, as the
/// interpreter's out-of-line path runs it.
pub(crate) unsafe extern "sysv64" fn h_exec(ctx: *mut Ctx, pc: u32) -> u32 {
    guard(ctx, |r, ctx| {
        let c = r.exec_at(pc as usize + 1);
        status(r, ctx, c, pc)
    })
}

/// The slow path of `IncJumpIfLt*` (`IncR r, r`); native code continues
/// with the loop test that follows.
pub(crate) unsafe extern "sysv64" fn h_inc(ctx: *mut Ctx, pc: u32) -> u32 {
    guard(ctx, |r, ctx| {
        let reg = match op_at(ctx, pc) {
            Op::IncJumpIfLtRK(x, _, _) | Op::IncJumpIfLtRR(x, _, _) => x,
            _ => unreachable!(),
        };
        let c = r.exec_with(pc as usize + 1, Op::IncR(reg, reg));
        status(r, ctx, c, pc)
    })
}

/// Safe point at a backward jump or call; `fpc` is the frame pc to sync
/// (the jump target, or the call's own resume pc).
pub(crate) unsafe extern "sysv64" fn h_tick(ctx: *mut Ctx, fpc: u32) -> u32 {
    guard(ctx, |r, _| {
        let c = r.tick_slow_jit(fpc as usize);
        match c {
            Ctl::Next => ST_NEXT,
            Ctl::Reload => ST_RELOAD,
            Ctl::Done => ST_DONE,
        }
    })
}

/// `ReturnR`: pop the frame (back to the run loop).
pub(crate) unsafe extern "sysv64" fn h_return(ctx: *mut Ctx, pc: u32) -> u32 {
    guard(ctx, |r, ctx| {
        let s = match op_at(ctx, pc) {
            Op::ReturnR(s) => s,
            _ => unreachable!(),
        };
        let v = std::mem::replace(&mut *(*ctx).slots.add(s as usize), Value::Undefined);
        let c = r.return_r(v);
        status(r, ctx, c, pc)
    })
}

/// `CallR`: calls of natives complete here; JavaScript callees get their
/// frame pushed and run from the run loop.
pub(crate) unsafe extern "sysv64" fn h_call(ctx: *mut Ctx, pc: u32) -> u32 {
    guard(ctx, |r, ctx| {
        let (b, argc, cic) = match op_at(ctx, pc) {
            Op::CallR(b, argc, cic, _) => (b, argc, cic),
            _ => unreachable!(),
        };
        let c = r.call_r(pc as usize + 1, b, argc, cic);
        status(r, ctx, c, pc)
    })
}

/// `NewR`.
pub(crate) unsafe extern "sysv64" fn h_new(ctx: *mut Ctx, pc: u32) -> u32 {
    guard(ctx, |r, ctx| {
        let (b, argc) = match op_at(ctx, pc) {
            Op::NewR(b, argc) => (b, argc),
            _ => unreachable!(),
        };
        let c = r.new_r(pc as usize + 1, b, argc);
        status(r, ctx, c, pc)
    })
}

/// Property, global, element, scope and `this` instructions: the run
/// loop's inline-cache fast paths, then the full instruction.
pub(crate) unsafe extern "sysv64" fn h_access(ctx: *mut Ctx, pc: u32) -> u32 {
    guard(ctx, |r, ctx| {
        let c = r.step_access(&*(*ctx).code, (*ctx).slots, pc as usize);
        status(r, ctx, c, pc)
    })
}

/// ToBoolean of a value native code could not decide (strings, objects,
/// BigInts): 0 or 1. Pure (no JavaScript runs).
pub(crate) unsafe extern "sysv64" fn h_truthy(ctx: *mut Ctx, bits: u64) -> u32 {
    let r = &*(*ctx).realm;
    // SAFETY: `bits` is a live value of a frame slot (borrowed, not owned).
    let v = std::mem::ManuallyDrop::new(Value::from_raw_bits(bits));
    r.to_boolean(&v) as u32
}

/// Strict equality of two values native code could not decide: 0 or 1.
pub(crate) unsafe extern "sysv64" fn h_strict_eq(ctx: *mut Ctx, a: u64, b: u64) -> u32 {
    let r = &*(*ctx).realm;
    let a = std::mem::ManuallyDrop::new(Value::from_raw_bits(a));
    let b = std::mem::ManuallyDrop::new(Value::from_raw_bits(b));
    r.strict_equals(&a, &b) as u32
}

/// `x % y` for numbers (pure).
pub(crate) extern "sysv64" fn h_mod(x: f64, y: f64) -> f64 {
    crate::vm::js_mod(x, y)
}

/// `x ** y` for numbers (pure).
pub(crate) extern "sysv64" fn h_pow(x: f64, y: f64) -> f64 {
    crate::vm::js_pow(x, y)
}
