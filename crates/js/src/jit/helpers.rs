//! The boundary between native code, the run loop and the runtime.
//!
//! Native code calls helpers `extern "sysv64" fn(ctx, ...) -> status` (the
//! System V convention on every x86-64 target, so Windows needs no second
//! calling convention). Most helpers decode `code.ops[pc]` themselves; calls
//! and returns receive static operands from the emitter. Each syncs the
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
pub(crate) const CTX_REALM: i32 = 16;
pub(crate) const CTX_TICK: i32 = 24;
pub(crate) const CTX_PC: i32 = 40;

/// Continue with the instruction's continuation in native code.
pub(crate) const ST_NEXT: usize = 0;
/// Frames were pushed or popped: back to the run loop (exit native code).
pub(crate) const ST_RELOAD: usize = 1;
/// The run loop is over (`loop_end` holds its result).
pub(crate) const ST_DONE: usize = 2;
/// Continue in the interpreter at the frame's pc.
pub(crate) const ST_INTERP: usize = 3;
/// A helper panicked (payload in `Realm::jit`).
pub(crate) const ST_PANIC: usize = 4;
/// Continue native code at `ctx.pc` (the instruction jumped).
pub(crate) const ST_JUMP: usize = 5;

/// Entry of a compiled function: `(ctx, pc) -> status` (never `ST_NEXT`
/// or `ST_JUMP`).
pub(crate) type Entry = unsafe extern "sysv64" fn(*mut Ctx, u32) -> usize;

impl Realm {
    /// Run-loop hook at each (re)load of the running frame (`pc` is its
    /// resume point): count fresh activations, compile hot code, and run
    /// native code when there is some. `Ctl::Next`: no native code ran;
    /// interpret the frame from `pc`.
    #[inline(always)]
    pub(crate) fn jit_reload(&mut self, code: &FunctionCode, pc: usize) -> Ctl {
        // Inlined into the run loop: the common outcomes (interpret a
        // function without native code, or a short one called from
        // interpreted code) cost a few loads.
        let s = &code.jit.s;
        let e = s.entry.get();
        if e == 0 {
            if pc != 0 {
                return Ctl::Next;
            }
            let n = s.counter.get().saturating_add(1);
            s.counter.set(n);
            if n < self.jit.threshold {
                return Ctl::Next;
            }
            return self.jit_hot(code);
        }
        if pc == 0 && !s.from_interp.get() {
            // A short loop-free function called from interpreted code.
            return Ctl::Next;
        }
        self.jit_run(code, e, pc)
    }

    /// A fresh activation of a function past the threshold without native
    /// code: compile it (once) and run it natively when worthwhile.
    #[inline(never)]
    fn jit_hot(&mut self, code: &FunctionCode) -> Ctl {
        let s = &code.jit.s;
        if s.state.get() != UNTRIED || !self.jit.on {
            return Ctl::Next;
        }
        compile(code, &mut self.jit.stats);
        let e = s.entry.get();
        if e == 0 || !s.from_interp.get() {
            return Ctl::Next;
        }
        self.jit_run(code, e, 0)
    }

    /// Enter native code `entry` of `code` (the last frame's) at `pc`.
    /// Calls and returns between functions with native code continue in
    /// native code (the helpers switch frames); native code leaves when
    /// the new last frame has none (the native stack does not grow).
    #[inline(never)]
    pub(crate) fn jit_run(&mut self, code: &FunctionCode, entry: usize, pc: usize) -> Ctl {
        let code: *const FunctionCode = code;
        {
            self.jit.entries += 1;
            if self.jit.stats.enabled {
                self.jit.stats.entries += 1;
            }
            // SAFETY: the run loop has a frame; its slots are within `vstack`.
            let f = unsafe { self.frames.last().unwrap_unchecked() };
            let slots = unsafe { self.vstack.as_mut_ptr().add(f.base as usize) };
            // Native code may outlive the frame running it until it leaves
            // (a return pops the frame first): a closure's code is owned by
            // the closure, which no collection can free before native code
            // leaves; a script's code is held by `run_script` for the whole
            // run.
            let realm: *mut Realm = self;
            let mut ctx = Ctx {
                slots,
                // SAFETY: `code` is the last frame's code.
                consts: unsafe { (*code).constants.as_ptr() },
                realm,
                // SAFETY: `realm` is valid; the field is only accessed
                // through this pointer while native code runs.
                tick: unsafe { std::ptr::addr_of_mut!((*realm).tick) },
                code,
                pc: 0,
            };
            // SAFETY: `entry` is the native code of `code` (kept alive by
            // the frame's code), entered at an instruction of it; until it
            // returns, the realm is only used through `ctx.realm`.
            let st = unsafe {
                let f: Entry = std::mem::transmute(entry);
                f(&mut ctx, pc as u32)
            };
            match st {
                // Frames were pushed or popped and the new last frame has
                // no native code to continue in (the helper's frame switch
                // tried, counting and compiling): back to the run loop.
                ST_RELOAD => return Ctl::Reload,
                ST_DONE => return Ctl::Done,
                ST_PANIC => match self.jit.panic.take() {
                    Some(p) => std::panic::resume_unwind(p),
                    None => panic!("native code panicked"),
                },
                _ => {
                    // Native code declined to continue (an entry outside
                    // the function): interpret this function from now on.
                    // SAFETY: `code` is the last frame's code.
                    let c = unsafe { &*ctx.code };
                    c.jit.s.entry.set(0);
                    c.jit.s.state.set(REJECTED);
                    return Ctl::Reload;
                }
            }
        }
    }
}

/// Run a helper body with the realm, catching panics, and refresh
/// `ctx.slots` when native code continues.
#[inline(always)]
unsafe fn guard(ctx: *mut Ctx, f: impl FnOnce(&mut Realm, *mut Ctx) -> usize) -> usize {
    let realm = (*ctx).realm;
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut *realm, ctx))) {
        Ok(st) => {
            if st == ST_NEXT || st == ST_JUMP {
                let r = &mut *realm;
                let b = r.frames.last().map(|f| f.base as usize).unwrap_or(0);
                (*ctx).slots = r.vstack.as_mut_ptr().add(b);
            } else if st == ST_RELOAD {
                // A call pushed a frame or a return popped one: continue
                // with the new last frame's native code when it has some.
                let a = switch_frame(ctx);
                if a != 0 {
                    return a;
                }
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
fn status(r: &mut Realm, ctx: *mut Ctx, c: Ctl, pc: u32) -> usize {
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
pub(crate) unsafe extern "sysv64" fn h_exec(ctx: *mut Ctx, pc: u32) -> usize {
    guard(ctx, |r, ctx| {
        let c = r.exec_at(pc as usize + 1);
        status(r, ctx, c, pc)
    })
}

/// The slow path of `IncJumpIfLt*` (`IncR r, r`); native code continues
/// with the loop test that follows.
pub(crate) unsafe extern "sysv64" fn h_inc(ctx: *mut Ctx, pc: u32) -> usize {
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
pub(crate) unsafe extern "sysv64" fn h_tick(ctx: *mut Ctx, fpc: u32) -> usize {
    guard(ctx, |r, _| {
        let c = r.tick_slow_jit(fpc as usize);
        match c {
            Ctl::Next => ST_NEXT,
            Ctl::Reload => ST_RELOAD,
            Ctl::Done => ST_DONE,
        }
    })
}

/// `ReturnR`: the emitter passes the source slot directly. Frame teardown
/// remains shared with the interpreter, including constructors and entry frames.
pub(crate) unsafe extern "sysv64" fn h_return(ctx: *mut Ctx, slot: u32) -> usize {
    guard(ctx, |r, ctx| {
        let v = std::mem::replace(&mut *(*ctx).slots.add(slot as usize), Value::Undefined);
        match r.return_r(v) {
            Ctl::Reload => ST_RELOAD,
            Ctl::Done => ST_DONE,
            Ctl::Next => unreachable!("return cannot continue in the popped frame"),
        }
    })
}

/// `CallR`: static base/argc/cache operands are packed by the emitter instead
/// of decoding bytecode on every activation. The call implementation, pc
/// synchronization, unwind status and frame switch remain shared with the VM.
pub(crate) unsafe extern "sysv64" fn h_call(ctx: *mut Ctx, pc: u32, args: u64) -> usize {
    guard(ctx, |r, ctx| {
        let c = r.call_r(pc as usize + 1, args as u16, (args >> 16) as u16, (args >> 32) as u16);
        status(r, ctx, c, pc)
    })
}

/// `NewR`.
pub(crate) unsafe extern "sysv64" fn h_new(ctx: *mut Ctx, pc: u32) -> usize {
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
pub(crate) unsafe extern "sysv64" fn h_access(ctx: *mut Ctx, pc: u32) -> usize {
    guard(ctx, |r, ctx| {
        let c = r.step_access(&*(*ctx).code, (*ctx).slots, pc as usize);
        status(r, ctx, c, pc)
    })
}

/// After a call pushed a frame or a return popped one: the address of the
/// new last frame's native code (inner entry, ctx rewritten for it, the pc
/// to enter at in `ctx.pc`) to continue without leaving native code, or 0
/// to go back to the run loop. Counts and compiles like the run loop's
/// hook.
#[inline(always)]
unsafe fn switch_frame(ctx: *mut Ctx) -> usize {
    let r = &mut *(*ctx).realm;
    let Some(f) = r.frames.last() else { return 0 };
    let (pc, base) = (f.pc, f.base as usize);
    // SAFETY: the frame's code outlives the frame (see `Frame::code`).
    let code: &FunctionCode = &*f.code.as_ptr();
    if code.realm.get() != r.current_realm {
        // A frame of another realm: the run loop switches realms.
        return 0;
    }
    let s = &code.jit.s;
    let mut e = s.entry.get();
    if e == 0 {
        if pc != 0 {
            return 0;
        }
        e = switch_compile(r, code);
        if e == 0 {
            return 0;
        }
    }
    if r.jit.stats.enabled {
        r.jit.stats.switches += 1;
    }
    let c = &mut *ctx;
    c.slots = r.vstack.as_mut_ptr().add(base);
    c.consts = code.constants.as_ptr();
    c.code = code;
    c.pc = pc as u32;
    e + super::emit::INNER_ENTRY
}

/// [`switch_frame`] to a fresh activation of a function without native
/// code: count it and compile it past the threshold; its entry or 0.
#[inline(never)]
fn switch_compile(r: &mut Realm, code: &FunctionCode) -> usize {
    let s = &code.jit.s;
    let n = s.counter.get().saturating_add(1);
    s.counter.set(n);
    if n < r.jit.threshold || s.state.get() != UNTRIED {
        return 0;
    }
    // Compilation runs no JavaScript; a panic in it is a bug, left to the
    // run loop (which compiles again and reports it).
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| compile(code, &mut r.jit.stats))).is_err() {
        return 0;
    }
    s.entry.get()
}

/// Fast paths of the most frequent accesses, without syncing the frame:
/// an inline-cache hit neither throws nor runs JavaScript nor moves the
/// value stack. Anything else takes [`h_access`].
#[inline(always)]
unsafe fn fast_access(ctx: *mut Ctx, pc: u32, f: impl FnOnce(&mut Realm, &FunctionCode, *mut Value) -> bool) -> usize {
    // No unwinding guard here: an inline-cache hit only reads and writes
    // plain data (a panic in it would be memory corruption, and aborts);
    // the miss path runs guarded.
    let realm = (*ctx).realm;
    let code: &FunctionCode = &*(*ctx).code;
    let slots = (*ctx).slots;
    if f(&mut *realm, code, slots) {
        ST_NEXT
    } else {
        h_access(ctx, pc)
    }
}

/// `AddRR` / `AddRK` of two strings (concatenation); anything else in
/// full.
pub(crate) unsafe extern "sysv64" fn h_add(ctx: *mut Ctx, pc: u32) -> usize {
    fast_access(ctx, pc, |_, code, slots| {
        let (d, a, b) = match *code.ops.get_unchecked(pc as usize) {
            Op::AddRR(d, a, b) => (d, &*slots.add(a as usize), &*slots.add(b as usize)),
            Op::AddRK(d, a, c) => (d, &*slots.add(a as usize), code.constants.get_unchecked(c as usize)),
            _ => return false,
        };
        match (a.v(), b.v()) {
            (crate::value::V::String(x), crate::value::V::String(y)) if x.len() + y.len() <= crate::value::MAX_STRING_LEN => {
                let r = x.concat(y);
                *slots.add(d as usize) = Value::String(r);
                true
            }
            _ => false,
        }
    })
}

/// `GetGlobalR` hit.
pub(crate) unsafe extern "sysv64" fn h_get_global(ctx: *mut Ctx, pc: u32) -> usize {
    fast_access(ctx, pc, |r, code, slots| {
        let Op::GetGlobalR(d, ic) = *code.ops.get_unchecked(pc as usize) else { return false };
        let key = code.constants.get_unchecked(*code.ic_keys.get_unchecked(ic as usize) as usize).as_js_str_unchecked();
        match r.ic_get_global(code.ics.get_unchecked(ic as usize), key) {
            Some(v) => {
                let v = v.clone();
                *slots.add(d as usize) = v;
                true
            }
            None => false,
        }
    })
}

/// `SetGlobalR` hit.
pub(crate) unsafe extern "sysv64" fn h_set_global(ctx: *mut Ctx, pc: u32) -> usize {
    fast_access(ctx, pc, |r, code, slots| {
        let Op::SetGlobalR(s, ic) = *code.ops.get_unchecked(pc as usize) else { return false };
        let val = &*slots.add(s as usize);
        if val.is_empty() {
            return false;
        }
        let key = code.constants.get_unchecked(*code.ic_keys.get_unchecked(ic as usize) as usize).as_js_str_unchecked();
        match r.ic_set_global(code.ics.get_unchecked(ic as usize), key, val.clone()) {
            Ok(()) => true,
            Err(v) => {
                drop(v);
                false
            }
        }
    })
}

/// `GetPropR` hit on a data property (own, prototype, primitive, length).
pub(crate) unsafe extern "sysv64" fn h_get_prop(ctx: *mut Ctx, pc: u32) -> usize {
    fast_access(ctx, pc, |r, code, slots| {
        let Op::GetPropR(d, o, ic) = *code.ops.get_unchecked(pc as usize) else { return false };
        let rec = code.ics.get_unchecked(ic as usize).get();
        let recv = &*slots.add(o as usize);
        let v = if rec.kind == crate::ic::IC_LENGTH {
            match recv.v() {
                crate::value::V::String(s) => Value::Number(s.len() as f64),
                crate::value::V::Object(id) => match r.heap.try_get(id).map(|o| &o.kind) {
                    Some(crate::heap::ObjectKind::Array { length, .. }) => Value::Number(*length as f64),
                    _ => return false,
                },
                _ => return false,
            }
        } else {
            match r.ic_get(&rec, recv) {
                Some(p) if p.flags & crate::heap::ACCESSOR == 0 => p.value.clone(),
                _ => return false,
            }
        };
        *slots.add(d as usize) = v;
        true
    })
}

/// `SetPropR` hit on a writable data property or a cached transition.
pub(crate) unsafe extern "sysv64" fn h_set_prop(ctx: *mut Ctx, pc: u32) -> usize {
    fast_access(ctx, pc, |r, code, slots| {
        let Op::SetPropR(o, v, ic) = *code.ops.get_unchecked(pc as usize) else { return false };
        let rec = code.ics.get_unchecked(ic as usize).get();
        let val = &*slots.add(v as usize);
        if rec.kind == 0 || rec.kind == crate::ic::IC_SETTER || val.is_empty() {
            return false;
        }
        let key = code.constants.get_unchecked(*code.ic_keys.get_unchecked(ic as usize) as usize).as_js_str_unchecked();
        match r.ic_set(&rec, &*slots.add(o as usize), key, val.clone()) {
            Ok(()) => true,
            Err(v) => {
                drop(v);
                false
            }
        }
    })
}

/// The scope `depth` levels up the running frame's scope chain (`None`:
/// something unexpected; the full instruction decides).
#[inline(always)]
fn scope_up(r: &Realm, depth: u16) -> Option<&crate::heap::ScopeData> {
    // SAFETY: native code runs the last frame.
    let mut id = unsafe { r.frames.last().unwrap_unchecked() }.scope;
    let mut n = depth;
    loop {
        let crate::heap::ObjectKind::Scope(sd) = &r.heap.try_get(id)?.kind else { return None };
        if n == 0 {
            return Some(sd);
        }
        id = sd.parent?;
        n -= 1;
    }
}

/// `GetLocalR`: a closure variable (no JavaScript runs, nothing throws).
pub(crate) unsafe extern "sysv64" fn h_get_local(ctx: *mut Ctx, pc: u32) -> usize {
    fast_access(ctx, pc, |r, code, slots| {
        let Op::GetLocalR(d, depth, s) = *code.ops.get_unchecked(pc as usize) else { return false };
        let Some(v) = scope_up(r, depth).and_then(|sd| sd.slots.get(s as usize)) else { return false };
        let v = if v.is_empty() { Value::Undefined } else { v.clone() };
        *slots.add(d as usize) = v;
        true
    })
}

/// `SetLocalR`: store into a closure variable.
pub(crate) unsafe extern "sysv64" fn h_set_local(ctx: *mut Ctx, pc: u32) -> usize {
    fast_access(ctx, pc, |r, code, slots| {
        let Op::SetLocalR(v, depth, s) = *code.ops.get_unchecked(pc as usize) else { return false };
        let val = (*slots.add(v as usize)).undef_if_empty();
        // SAFETY: native code runs the last frame.
        let mut id = r.frames.last().unwrap_unchecked().scope;
        for _ in 0..depth {
            match r.heap.try_get(id).map(|o| &o.kind) {
                Some(crate::heap::ObjectKind::Scope(sd)) => match sd.parent {
                    Some(p) => id = p,
                    None => return false,
                },
                _ => return false,
            }
        }
        match r.heap.try_get_mut(id).map(|o| &mut o.kind) {
            Some(crate::heap::ObjectKind::Scope(sd)) => match sd.slots.get_mut(s as usize) {
                Some(slot) => {
                    // The old value is dropped after the store (a string
                    // drop runs no JavaScript).
                    let old = std::mem::replace(slot, val);
                    drop(old);
                    true
                }
                None => false,
            },
            _ => false,
        }
    })
}

/// `DefinePropR` adding a property through a cached transition.
pub(crate) unsafe extern "sysv64" fn h_define_prop(ctx: *mut Ctx, pc: u32) -> usize {
    fast_access(ctx, pc, |r, code, slots| {
        let Op::DefinePropR(o, v, ic) = *code.ops.get_unchecked(pc as usize) else { return false };
        let rec = code.ics.get_unchecked(ic as usize).get();
        let val = &*slots.add(v as usize);
        if rec.kind != crate::ic::IC_ADD || val.is_empty() {
            return false;
        }
        let key = code.constants.get_unchecked(*code.ic_keys.get_unchecked(ic as usize) as usize).as_js_str_unchecked();
        match r.ic_set(&rec, &*slots.add(o as usize), key, val.clone()) {
            Ok(()) => true,
            Err(v) => {
                drop(v);
                false
            }
        }
    })
}

/// `StoreCompletionR` (script completion value).
pub(crate) unsafe extern "sysv64" fn h_store_completion(ctx: *mut Ctx, pc: u32) -> usize {
    fast_access(ctx, pc, |r, code, slots| {
        let Op::StoreCompletionR(s) = *code.ops.get_unchecked(pc as usize) else { return false };
        let v = (*slots.add(s as usize)).clone();
        // SAFETY: native code runs the last frame.
        r.frames.last_mut().unwrap_unchecked().extra_mut().completion = v;
        true
    })
}

/// `GetElemR` on arrays / typed arrays / strings with an index in range.
pub(crate) unsafe extern "sysv64" fn h_get_elem(ctx: *mut Ctx, pc: u32) -> usize {
    fast_access(ctx, pc, |r, code, slots| {
        let Op::GetElemR(d, o, k) = *code.ops.get_unchecked(pc as usize) else { return false };
        match r.get_elem_fast(&*slots.add(o as usize), &*slots.add(k as usize)) {
            Some(v) => {
                *slots.add(d as usize) = v;
                true
            }
            None => false,
        }
    })
}

/// `SetElemR` on arrays / typed arrays with an index in range.
pub(crate) unsafe extern "sysv64" fn h_set_elem(ctx: *mut Ctx, pc: u32) -> usize {
    fast_access(ctx, pc, |r, code, slots| {
        let Op::SetElemR(o, k, v) = *code.ops.get_unchecked(pc as usize) else { return false };
        let val = (*slots.add(v as usize)).clone();
        if val.is_empty() {
            return false;
        }
        match r.set_elem_fast(&*slots.add(o as usize), &*slots.add(k as usize), val) {
            Ok(()) => true,
            Err(v) => {
                drop(v);
                false
            }
        }
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

/// Release the last reference of a string or BigInt (native code found
/// its count at 1 and restored it). Pure: no JavaScript runs.
pub(crate) unsafe extern "sysv64" fn h_drop(bits: u64) {
    drop(Value::from_raw_bits(bits));
}

/// Loose equality of two values native code could not decide, when it
/// runs no JavaScript: 0 or 1; 2 when it might (objects or booleans
/// against other types, conversions): the full instruction decides.
pub(crate) unsafe extern "sysv64" fn h_loose_eq(ctx: *mut Ctx, a: u64, b: u64) -> u32 {
    use crate::value::V;
    let r = &*(*ctx).realm;
    let a = std::mem::ManuallyDrop::new(Value::from_raw_bits(a));
    let b = std::mem::ManuallyDrop::new(Value::from_raw_bits(b));
    match (a.v(), b.v()) {
        (V::Undefined | V::Null | V::Empty, V::Undefined | V::Null | V::Empty) => 1,
        (V::Undefined | V::Null | V::Empty, V::Object(_)) => r.is_htmldda(&b) as u32,
        (V::Object(_), V::Undefined | V::Null | V::Empty) => r.is_htmldda(&a) as u32,
        (V::Undefined | V::Null | V::Empty, _) | (_, V::Undefined | V::Null | V::Empty) => 0,
        (V::String(_), V::String(_)) | (V::Number(_), V::Number(_)) | (V::Bool(_), V::Bool(_)) | (V::Symbol(_), V::Symbol(_)) | (V::Object(_), V::Object(_)) => r.strict_equals(&a, &b) as u32,
        (V::Symbol(_), V::String(_) | V::Number(_)) | (V::String(_) | V::Number(_), V::Symbol(_)) => 0,
        _ => 2,
    }
}

thread_local! {
    /// `typeof` results, shared (no allocation per `typeof`).
    static TYPEOF: [Value; 8] = ["undefined", "object", "boolean", "number", "string", "symbol", "bigint", "function"].map(|s| Value::String(crate::value::JsStr::intern(s)));
}

/// `UnR` typeof (pure): anything else in full.
pub(crate) unsafe extern "sysv64" fn h_unr(ctx: *mut Ctx, pc: u32) -> usize {
    fast_access(ctx, pc, |r, code, slots| {
        let Op::UnR(3, d, s) = *code.ops.get_unchecked(pc as usize) else { return false };
        let t = r.type_of(&*slots.add(s as usize));
        let i = match t {
            "undefined" => 0,
            "object" => 1,
            "boolean" => 2,
            "number" => 3,
            "string" => 4,
            "symbol" => 5,
            "bigint" => 6,
            "function" => 7,
            _ => return false,
        };
        let v = TYPEOF.with(|t| t[i].clone());
        *slots.add(d as usize) = v;
        true
    })
}

/// `x % y` for numbers (pure).
pub(crate) extern "sysv64" fn h_mod(x: f64, y: f64) -> f64 {
    crate::vm::js_mod(x, y)
}

/// `x ** y` for numbers (pure).
pub(crate) extern "sysv64" fn h_pow(x: f64, y: f64) -> f64 {
    crate::vm::js_pow(x, y)
}
