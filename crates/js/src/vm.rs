//! The bytecode interpreter: frames, calls, generators, async and exceptions.

use crate::bytecode::{FunctionCode, Op, VarLoc, CALL_ARGS_DIRECT, CALL_ARROW, CALL_PARAMS_IN_REGS, CALL_PLAIN, CALL_PLAIN_CONSTRUCT, CALL_SIMPLE, CALL_STRICT};
use crate::value::V;
use crate::heap::*;
use crate::interop::{Hint, JsResult};
use crate::realm::*;
use crate::value::*;
use std::rc::Rc;

/// An activation record. Its value slots (registers, then the operand
/// stack) live in [`Realm::vstack`] from `base` on; code using
/// `arguments.length` / `arguments[i]` directly keeps the actual arguments
/// in the `argc` values just below `base`.
pub struct Frame {
    /// The running code, not reference counted (calls and returns cost no
    /// count traffic): a function frame's code is owned by its closure
    /// (`func`, which the frame keeps alive), any other frame's by
    /// `extra.code`. A closure's code is only ever replaced while it is
    /// still lazy, and lazy code never runs.
    pub(crate) code: std::ptr::NonNull<FunctionCode>,
    pub pc: usize,
    pub base: u32,
    pub argc: u32,
    pub scope: ObjectId,
    pub scope_depth: u32,
    pub this: Value,
    pub func: Option<ObjectId>,
    pub new_target: Value,
    /// Caller slot receiving this frame's return value.
    pub ret_slot: u32,
    pub is_construct: bool,
    /// Returning from this frame exits the run loop.
    pub entry: bool,
    /// State only some frames need (boxed to keep calls cheap).
    pub extra: Option<Box<FrameExtra>>,
}

/// Interpreter stack heights at some point (a native call, a host entry
/// point), to restore them after a Rust panic unwound past it (see
/// [`Realm::stack_mark`] / [`Realm::unwind_to`]).
pub struct StackMark {
    frames: usize,
    vstack: usize,
    new_target: usize,
    join: usize,
    job_roots: usize,
    native_depth: usize,
    gc_block: usize,
    native_roots: usize,
    callee: Option<ObjectId>,
    terminating: bool,
}

/// The rarely used parts of a frame.
#[derive(Default)]
pub struct FrameExtra {
    /// Owner of the code of a frame without a closure (scripts, eval).
    pub code: Option<Rc<FunctionCode>>,
    pub arguments_obj: Option<Value>,
    pub generator: Option<ObjectId>,
    /// Script completion value (last expression statement).
    pub completion: Value,
    /// Slot receiving the value a suspended (yield / await) frame resumes with.
    pub resume_slot: u32,
    /// The value slots (and kept arguments) of a suspended frame.
    pub saved: Vec<Value>,
}

impl Frame {
    /// The code this frame runs.
    #[inline(always)]
    pub fn code(&self) -> &FunctionCode {
        // SAFETY: see the `code` field: the code outlives the frame.
        unsafe { self.code.as_ref() }
    }

    /// First `vstack` index used by the frame (its kept arguments).
    #[inline]
    pub fn bottom(&self) -> usize {
        self.base as usize - self.argc as usize
    }

    pub fn generator(&self) -> Option<ObjectId> {
        self.extra.as_ref().and_then(|e| e.generator)
    }

    pub(crate) fn extra_mut(&mut self) -> &mut FrameExtra {
        self.extra.get_or_insert_with(Default::default)
    }
}

/// Where [`Realm::push_closure_frame`] takes the arguments from.
pub(crate) enum ArgSrc<'a> {
    /// `vstack` slots from this index on (moved out, left undefined).
    Stack(usize),
    /// A slice outside the value stack (cloned).
    Slice(&'a [Value]),
}

impl Default for Value {
    fn default() -> Value {
        Value::Undefined
    }
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Frame({} pc={})", self.code().name, self.pc)
    }
}

/// How a `run` invocation ended.
pub enum Exit {
    Return(Value),
    Yield(Value, Box<Frame>),
    Await(Value, Box<Frame>),
}

/// What the run loop does after an out-of-line instruction.
pub(crate) enum Flow {
    /// Continue with the current frame.
    Next,
    /// Frames were pushed or popped: reload the current frame.
    #[allow(dead_code)]
    Reload,
    Exit(Exit),
}

/// What the run loop does after an out-of-line step (a small value
/// returned in a register; an exit or error waits in `Realm::loop_end`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ctl {
    /// Continue the last frame at its synced pc (its slots may have moved).
    Next,
    /// Frames were pushed or popped: reload the last frame.
    Reload,
    /// The loop is over: `loop_end` holds the result.
    Done,
}

/// Result of resuming a generator-like frame (frame kept inside the generator object).
#[derive(Debug)]
pub enum Step {
    Return(Value),
    Yield(Value),
    Await(Value),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResumeMode {
    Next,
    Throw,
    Return,
}

impl Realm {
    // ---------------------------------------------------------------- calls

    /// Call any callable value.
    pub fn call(&mut self, f: Value, this: Value, args: &[Value]) -> JsResult<Value> {
        let id = match f.v() {
            V::Object(id) => id,
            _ => {
                let d = self.describe_for_error(&f);
                if std::env::var_os("RH_DEBUG_NOTFN").is_some() {
                    let native = self.current_callee.map(|n| self.function_name(n).to_rust_string()).unwrap_or_default();
                    eprintln!("[notfn] {d} called from native `{native}` at\n{}", self.build_stack_string("TypeError", "not a function"));
                    if let Some(f) = self.frames.last() {
                        if let (Some(src), Some((a, b))) = (&f.code().source, f.code().source_span) {
                            let (a, b) = (a as usize, (b as usize).min(a as usize + 400));
                            eprintln!("[notfn] top function source: {}", src.get(a..b).unwrap_or("?"));
                        }
                        eprintln!("[notfn] pc={} op={:?}", f.pc, f.code().ops.get(f.pc.saturating_sub(1)));
                    }
                }
                return self.type_error(&format!("{} is not a function", d));
            }
        };
        match &self.obj(id).kind {
            ObjectKind::Function(c) => {
                let code = c.code.clone();
                if code.flags.is_class_constructor {
                    let n = code.name.to_rust_string();
                    // The error comes from the constructor's realm.
                    return self.in_realm(code.realm.get(), |r| r.type_error(&format!("Class constructor {} cannot be invoked without 'new'", n)));
                }
                if code.flags.is_generator || code.flags.is_async {
                    return self.start_generator_like(id, this, args);
                }
                if !self.push_plain_call(id, &this, args, 0, true)? {
                    let frame = self.prepare_frame(id, this, args, Value::Undefined, false)?;
                    self.push_frame(frame, true)?;
                }
                self.entry_tick();
                match self.run()? {
                    Exit::Return(v) => Ok(v),
                    _ => unreachable!("non-generator frame suspended"),
                }
            }
            ObjectKind::Native(n) => {
                let func = n.func;
                self.call_native(id, func, this, args, false)
            }
            ObjectKind::Bound(b) => {
                let target = Value::Object(b.target);
                let bthis = b.this.clone();
                let mut all = b.args.clone();
                all.extend_from_slice(args);
                self.call(target, bthis, &all)
            }
            ObjectKind::Proxy { target, handler, revoked } => {
                let (target, handler, revoked) = (*target, *handler, *revoked);
                if revoked {
                    return self.type_error("Cannot perform 'apply' on a proxy that has been revoked");
                }
                let trap = self.get_method(Value::Object(handler), PropertyKey::str("apply"))?;
                match trap {
                    Some(t) => {
                        let arr = self.new_array_value(args.to_vec());
                        self.call(t, Value::Object(handler), &[Value::Object(target), this, arr])
                    }
                    None => self.call(Value::Object(target), this, args),
                }
            }
            _ => {
                let d = self.describe_for_error(&f);
                self.type_error(&format!("{} is not a function", d))
            }
        }
    }

    fn call_native(&mut self, id: ObjectId, func: NativeFn, this: Value, args: &[Value], construct: bool) -> JsResult<Value> {
        if self.native_depth >= self.max_native_depth || crate::stack::exhausted() {
            return self.range_error("Maximum call stack size exceeded");
        }
        self.native_depth += 1;
        let (gc_safe, frealm) = match self.heap.try_get(id).map(|o| &o.kind) {
            Some(ObjectKind::Native(n)) => (n.gc_safe, n.realm),
            _ => (false, self.current_realm),
        };
        let caller_realm = self.current_realm;
        if frealm != caller_realm {
            self.switch_realm(frealm);
        }
        let roots = self.native_roots.len();
        if gc_safe {
            self.native_roots.push(this.clone());
            self.native_roots.extend_from_slice(args);
        } else {
            self.gc_block += 1;
        }
        let saved = self.current_callee.replace(id);
        if !construct {
            self.new_target_stack.push(Value::Undefined);
        }
        // A Rust panic inside the native (a host binding bug) becomes a
        // catchable JS error here instead of unwinding through the
        // interpreter and killing the thread.
        let mark = self.stack_mark();
        let r = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| func(self, this, args))) {
            Ok(r) => r,
            Err(payload) => Err(self.recover_from_panic(&*payload, &mark, id)),
        };
        if crate::realm::take_post_native() {
            if let Some(h) = self.post_native {
                h(self);
            }
        }
        if !construct {
            self.new_target_stack.pop();
        }
        self.current_callee = saved;
        if gc_safe {
            self.native_roots.truncate(roots);
        } else {
            self.gc_block -= 1;
        }
        self.native_depth -= 1;
        if self.current_realm != caller_realm {
            self.switch_realm(caller_realm);
        }
        r
    }

    /// Restore the interpreter state saved at `mark` after a native function
    /// panicked (frames it pushed through nested calls are dropped, their
    /// generators completed) and build the error thrown in its place.
    #[cold]
    #[inline(never)]
    fn recover_from_panic(&mut self, payload: &(dyn std::any::Any + Send), mark: &StackMark, callee: ObjectId) -> Value {
        let (msg, loc) = crate::panics::describe(payload);
        self.unwind_to(mark);
        let name = match self.heap.try_get(callee).map(|o| &o.kind) {
            Some(ObjectKind::Native(n)) if !n.name.is_empty() => n.name.to_rust_string(),
            _ => "<native>".to_string(),
        };
        let at = if loc.is_empty() { String::new() } else { format!(" at {loc}") };
        self.console_emit("error", format!("internal error in {name}(): {msg}{at} (Rust panic caught; the page keeps running)"));
        self.new_error("Error", &format!("internal error: {msg}"))
    }

    /// The interpreter's stack heights now.
    #[inline]
    pub fn stack_mark(&self) -> StackMark {
        StackMark {
            frames: self.frames.len(),
            vstack: self.vstack.len(),
            new_target: self.new_target_stack.len(),
            join: self.join_stack.len(),
            job_roots: self.job_roots.len(),
            native_depth: self.native_depth,
            gc_block: self.gc_block,
            native_roots: self.native_roots.len(),
            callee: self.current_callee,
            terminating: self.terminating,
        }
    }

    /// Restore the stacks to `mark` after a Rust panic unwound past it:
    /// frames pushed since are dropped (their generators completed), and
    /// the value / `new.target` / join / job-root stacks cut back.
    #[cold]
    pub fn unwind_to(&mut self, mark: &StackMark) {
        while self.frames.len() > mark.frames {
            let popped = self.frames.pop().expect("frame");
            if let Some(g) = popped.generator() {
                if let Some(o) = self.heap.try_get_mut(g) {
                    if let ObjectKind::Generator(gd) = &mut o.kind {
                        gd.state = GeneratorState::Completed;
                        gd.frame = None;
                    }
                }
            }
        }
        self.vstack.truncate(mark.vstack);
        // Slots the native's caller owns must still exist (the native may
        // have been entered with arguments still on the stack).
        if self.vstack.len() < mark.vstack {
            self.vstack.resize(mark.vstack, Value::Undefined);
        }
        self.new_target_stack.truncate(mark.new_target);
        self.join_stack.truncate(mark.join);
        self.job_roots.truncate(mark.job_roots);
        self.native_depth = mark.native_depth;
        self.gc_block = mark.gc_block;
        self.native_roots.truncate(mark.native_roots);
        self.current_callee = mark.callee;
        self.terminating = mark.terminating;
    }

    /// `[[Construct]]`.
    pub fn construct(&mut self, f: Value, args: &[Value], new_target: Value) -> JsResult<Value> {
        let new_target = if new_target.is_undefined() { f.clone() } else { new_target };
        let id = match f.v() {
            V::Object(id) => id,
            _ => {
                let d = self.describe_for_error(&f);
                return self.type_error(&format!("{} is not a constructor", d));
            }
        };
        if !self.is_constructor(&f) {
            let d = self.describe_for_error(&f);
            return self.type_error(&format!("{} is not a constructor", d));
        }
        match &self.obj(id).kind {
            ObjectKind::Function(_) => {
                let frame = self.prepare_frame(id, Value::Empty, args, new_target, true)?;
                self.push_frame(frame, true)?;
                match self.run()? {
                    Exit::Return(v) => Ok(v),
                    _ => unreachable!(),
                }
            }
            ObjectKind::Native(n) => {
                let func = n.func;
                self.new_target_stack.push(new_target);
                let r = self.call_native(id, func, Value::Undefined, args, true);
                self.new_target_stack.pop();
                r
            }
            ObjectKind::Bound(b) => {
                let target = Value::Object(b.target);
                let mut all = b.args.clone();
                all.extend_from_slice(args);
                let nt = if new_target.as_object() == Some(id) { target.clone() } else { new_target };
                self.construct(target, &all, nt)
            }
            ObjectKind::Proxy { target, handler, .. } => {
                let (target, handler) = (*target, *handler);
                let trap = self.get_method(Value::Object(handler), PropertyKey::str("construct"))?;
                match trap {
                    Some(t) => {
                        let arr = self.new_array_value(args.to_vec());
                        let r = self.call(t, Value::Object(handler), &[Value::Object(target), arr, new_target])?;
                        if !r.is_object() {
                            return self.type_error("proxy [[Construct]] must return an object");
                        }
                        Ok(r)
                    }
                    None => self.construct(Value::Object(target), args, new_target),
                }
            }
            _ => self.type_error("not a constructor"),
        }
    }

    /// Run a compiled script in the global scope.
    pub fn run_script(&mut self, code: Rc<FunctionCode>) -> JsResult<Value> {
        let slots = code.scope_slots as usize;
        let scope = self.alloc(Object::new(None, ObjectKind::Scope(Box::new(ScopeData { slots: vec![Value::Undefined; slots], parent: None, names: None }))));
        let this = if code.flags.is_script { Value::Object(self.global) } else { Value::Undefined };
        let base = self.reserve_slots(code.frame_size());
        let frame = Frame {
            code: std::ptr::NonNull::from(&*code),
            pc: 0,
            base: base as u32,
            argc: 0,
            scope,
            scope_depth: 0,
            this,
            func: None,
            new_target: Value::Undefined,
            ret_slot: 0,
            is_construct: false,
            entry: true,
            extra: Some(Box::new(FrameExtra { code: Some(code.clone()), ..Default::default() })),
        };
        self.push_frame(frame, true)?;
        // `code` stays alive until the run is over: returning from the
        // frame drops the frame's reference while native code of it may
        // still be finishing.
        match self.run()? {
            Exit::Return(v) => Ok(v),
            _ => unreachable!(),
        }
    }

    /// Push a frame whose slots are already reserved on top of `vstack`
    /// (on failure they are released).
    fn push_frame(&mut self, mut frame: Frame, entry: bool) -> JsResult<()> {
        if self.frames.len() >= self.max_frames {
            self.vstack.truncate(frame.bottom());
            return self.range_error("Maximum call stack size exceeded");
        }
        frame.entry = entry;
        self.frames.push(frame);
        Ok(())
    }

    /// `n` fresh (undefined) slots on top of the value stack; returns the
    /// index of the first.
    #[inline]
    pub(crate) fn reserve_slots(&mut self, n: usize) -> usize {
        let base = self.vstack.len();
        self.vstack.resize(base + n, Value::Undefined);
        base
    }

    /// Pop the current frame and release its slots.
    #[inline]
    pub(crate) fn pop_frame(&mut self) -> Frame {
        let f = self.frames.pop().expect("frame underflow");
        self.vstack.truncate(f.bottom());
        f
    }

    /// Move a frame's slots off the value stack (the frame suspends).
    fn detach_slots(&mut self, f: &mut Frame) {
        let bottom = f.bottom();
        let extra = f.extra_mut();
        extra.saved.clear();
        extra.saved.extend(self.vstack.drain(bottom..));
    }

    /// Put a suspended frame's slots back on top of the value stack.
    fn attach_slots(&mut self, f: &mut Frame) {
        let bottom = self.vstack.len();
        f.base = (bottom + f.argc as usize) as u32;
        if let Some(e) = f.extra.as_mut() {
            self.vstack.append(&mut e.saved);
        }
    }

    /// `Call(argc)` of a plain closure (no `arguments`, rest parameter,
    /// generator/async or class constructor): builds the callee frame by
    /// moving the arguments straight from the caller's operand stack.
    /// Returns false (nothing done) when the general path is needed. The
    /// caller's frame must be synced.
    #[inline(always)]
    pub(crate) fn try_fast_call(&mut self, fi: usize, b: usize, argc: usize) -> JsResult<bool> {
        // SAFETY: `fi` is the running frame.
        let cb = unsafe { self.frames.get_unchecked(fi) }.base as usize + b;
        let id = match self.vstack[cb].v() {
            V::Object(id) => id,
            _ => return Ok(false),
        };
        let Some(obj) = self.heap.try_get(id) else { return Ok(false) };
        let ObjectKind::Function(c) = &obj.kind else { return Ok(false) };
        let cf = c.code.call_flags;
        if cf & CALL_PLAIN == 0 {
            if c.code.lazy.is_some() {
                self.ensure_compiled(id)?;
                return self.try_fast_call(fi, b, argc);
            }
            return Ok(false);
        }
        let (this, new_target) = if cf & CALL_ARROW != 0 {
            if c.arguments.is_some() {
                return Ok(false);
            }
            (c.this_value.clone().unwrap_or(Value::Undefined), c.new_target.clone().unwrap_or(Value::Undefined))
        } else if cf & CALL_STRICT != 0 {
            (self.vstack[cb + 1].undef_if_empty(), Value::Undefined)
        } else {
            match self.vstack[cb + 1].v() {
                V::Undefined | V::Null | V::Empty => (Value::Object(self.realm_global(c.code.realm.get())), Value::Undefined),
                V::Object(o) => (Value::Object(o), Value::Undefined),
                _ => return Ok(false),
            }
        };
        let code = std::ptr::NonNull::from(&*c.code);
        let parent_scope = c.scope;
        self.push_closure_frame(id, code, parent_scope, ArgSrc::Stack(cb + 2), argc, this, new_target, b as u32, false, false)?;
        Ok(true)
    }

    /// Push the frame of a call to closure `id`, whose compiled `code` has
    /// [`CALL_PLAIN`]: registers start undefined, parameters take the
    /// arguments, the scope object is created when the function has one.
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_closure_frame(
        &mut self,
        id: ObjectId,
        code: std::ptr::NonNull<FunctionCode>,
        parent_scope: Option<ObjectId>,
        args: ArgSrc,
        argc: usize,
        this: Value,
        new_target: Value,
        ret_slot: u32,
        is_construct: bool,
        entry: bool,
    ) -> JsResult<()> {
        // SAFETY: the closure `id` owns the code and stays alive (it is
        // in the caller's slots or argument list, then rooted by the frame).
        let cr: &FunctionCode = unsafe { &*code.as_ptr() };
        let cf = cr.call_flags;
        if let (true, ArgSrc::Stack(src), Some(scope)) = (cf & CALL_SIMPLE != 0, &args, parent_scope) {
            return self.push_simple_frame(id, cr, scope, *src, argc, this, new_target, ret_slot, is_construct, entry);
        }
        if self.frames.len() >= self.max_frames {
            return self.range_error("Maximum call stack size exceeded");
        }
        let fsize = cr.frame_size();
        let kept = if cf & CALL_ARGS_DIRECT != 0 { argc } else { 0 };
        // No reallocation below: source pointers into the stack stay valid.
        self.vstack.reserve(kept + fsize);
        let bottom = self.vstack.len();
        let base = bottom + kept;
        let nslots = cr.scope_slots as usize;
        let mut slots: Vec<Value> = Vec::new();
        // SAFETY: capacity was reserved above; source slots (caller frame
        // or an outside slice) do not overlap the new slots; every new slot
        // is written before `set_len` makes it visible.
        unsafe {
            let p = self.vstack.as_mut_ptr();
            let (src, take): (*mut Value, bool) = match args {
                ArgSrc::Stack(i) => (p.add(i), true),
                ArgSrc::Slice(s) => (s.as_ptr() as *mut Value, false),
            };
            for i in 0..kept {
                std::ptr::write(p.add(bottom + i), (*src.add(i)).clone());
            }
            let regs = p.add(base);
            for i in 0..fsize {
                std::ptr::write(regs.add(i), Value::Undefined);
            }
            self.vstack.set_len(base + fsize);
            let arg = |i: usize| -> Value {
                if take {
                    std::ptr::replace(src.add(i), Value::Undefined)
                } else {
                    (*src.add(i)).clone()
                }
            };
            if cf & CALL_PARAMS_IN_REGS != 0 {
                let n = argc.min(cr.param_locs.len());
                for i in 0..n {
                    std::ptr::write(regs.add(i), arg(i));
                }
            } else {
                if nslots > 0 {
                    slots = vec![Value::Undefined; nslots];
                }
                for (i, &loc) in cr.param_locs.iter().enumerate() {
                    if i >= argc {
                        break;
                    }
                    let v = arg(i);
                    match loc {
                        VarLoc::Reg(r) => *regs.add(r as usize) = v,
                        VarLoc::Slot(s) => slots[s as usize] = v,
                    }
                }
            }
        }
        if nslots > 0 && slots.is_empty() {
            slots = vec![Value::Undefined; nslots];
        }
        if let Some(sl) = cr.self_loc {
            match sl {
                VarLoc::Reg(r) => self.vstack[base + r as usize] = Value::Object(id),
                VarLoc::Slot(s) => slots[s as usize] = Value::Object(id),
            }
        }
        let scope = match parent_scope {
            Some(p) if nslots == 0 => p,
            _ => self.alloc(Object::new(None, ObjectKind::Scope(Box::new(ScopeData { slots, parent: parent_scope, names: None })))),
        };
        let frame = Frame {
            code,
            pc: 0,
            base: base as u32,
            argc: kept as u32,
            scope,
            scope_depth: 0,
            this,
            func: Some(id),
            new_target,
            ret_slot,
            is_construct,
            entry,
            extra: None,
        };
        if self.frames.len() == self.frames.capacity() {
            self.frames.reserve(1);
        }
        // SAFETY: capacity checked just above.
        unsafe {
            let n = self.frames.len();
            std::ptr::write(self.frames.as_mut_ptr().add(n), frame);
            self.frames.set_len(n + 1);
        }
        Ok(())
    }

    /// `CallR` of `f.call(this, ...)`, `f.apply(this, array)` or a bound
    /// function whose target is a plain closure: the target's frame is
    /// pushed directly (no native call and nested run). Returns false when
    /// the general path is needed.
    pub(crate) fn try_call_indirect(&mut self, fi: usize, b: usize, argc: usize) -> JsResult<bool> {
        let cb = self.frames[fi].base as usize + b;
        let Some(id) = self.vstack[cb].as_object() else { return Ok(false) };
        let Some(obj) = self.heap.try_get(id) else { return Ok(false) };
        let mut buf = std::mem::take(&mut self.arg_buf);
        buf.clear();
        let (target, this) = match &obj.kind {
            ObjectKind::Native(n) if n.intrinsic == INTRINSIC_FN_CALL => {
                let Some(t) = self.vstack[cb + 1].as_object() else { return Ok(false) };
                let this = if argc > 0 { self.vstack[cb + 2].clone() } else { Value::Undefined };
                if argc > 1 {
                    buf.extend_from_slice(&self.vstack[cb + 3..cb + 2 + argc]);
                }
                (t, this)
            }
            ObjectKind::Native(n) if n.intrinsic == INTRINSIC_FN_APPLY => {
                let Some(t) = self.vstack[cb + 1].as_object() else { return Ok(false) };
                let this = if argc > 0 { self.vstack[cb + 2].clone() } else { Value::Undefined };
                if argc > 1 {
                    let list = &self.vstack[cb + 3];
                    if let Some(l) = list.as_object() {
                        match self.heap.try_get(l).map(|o| &o.kind) {
                            Some(ObjectKind::Array { elements, length }) if elements.len() == *length as usize && elements.iter().all(|v| !v.is_empty()) && elements.len() <= 1024 => {
                                buf.extend_from_slice(elements);
                            }
                            _ => {
                                self.arg_buf = buf;
                                return Ok(false);
                            }
                        }
                    } else if !list.is_nullish() {
                        self.arg_buf = buf;
                        return Ok(false);
                    }
                }
                (t, this)
            }
            ObjectKind::Bound(bf) => {
                buf.extend_from_slice(&bf.args);
                buf.extend_from_slice(&self.vstack[cb + 2..cb + 2 + argc]);
                (bf.target, bf.this.clone())
            }
            _ => {
                self.arg_buf = buf;
                return Ok(false);
            }
        };
        let ok = match self.heap.try_get(target).map(|o| &o.kind) {
            Some(ObjectKind::Function(_)) => self.push_plain_call(target, &this, &buf, b as u32, false),
            _ => Ok(false),
        };
        buf.clear();
        if buf.capacity() <= 64 {
            self.arg_buf = buf;
        }
        ok
    }

    /// [`Realm::push_closure_frame`] for [`CALL_SIMPLE`] code with the
    /// arguments in the caller's slots: undefined registers, the arguments
    /// moved into the parameter registers, the closure's scope.
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn push_simple_frame(
        &mut self,
        id: ObjectId,
        cr: &FunctionCode,
        scope: ObjectId,
        src: usize,
        argc: usize,
        this: Value,
        new_target: Value,
        ret_slot: u32,
        is_construct: bool,
        entry: bool,
    ) -> JsResult<()> {
        let n = self.frames.len();
        if n >= self.max_frames {
            return self.range_error("Maximum call stack size exceeded");
        }
        let fsize = cr.frame_size();
        self.vstack.reserve(fsize);
        if n == self.frames.capacity() {
            self.frames.reserve(1);
        }
        let base = self.vstack.len();
        let nargs = argc.min(cr.param_locs.len());
        // SAFETY: capacities reserved above; the source slots (caller's
        // operand stack, below `base`) and the new slots do not overlap;
        // every new slot / the frame is written before the length grows.
        unsafe {
            let p = self.vstack.as_mut_ptr();
            let regs = p.add(base);
            let from = p.add(src);
            for i in 0..nargs {
                std::ptr::write(regs.add(i), std::ptr::read(from.add(i)));
                std::ptr::write(from.add(i), Value::Undefined);
            }
            for i in nargs..fsize {
                std::ptr::write(regs.add(i), Value::Undefined);
            }
            self.vstack.set_len(base + fsize);
            std::ptr::write(
                self.frames.as_mut_ptr().add(n),
                Frame {
                    code: std::ptr::NonNull::from(cr),
                    pc: 0,
                    base: base as u32,
                    argc: 0,
                    scope,
                    scope_depth: 0,
                    this,
                    func: Some(id),
                    new_target,
                    ret_slot,
                    is_construct,
                    entry,
                    extra: None,
                },
            );
            self.frames.set_len(n + 1);
        }
        Ok(())
    }

    /// Push the frame for calling plain closure `id` (no `arguments`
    /// object, rest parameter, generator / async or class constructor) with
    /// arguments from a slice, e.g. a callback called by a native function.
    /// Returns false (nothing done) when the general path is needed.
    pub(crate) fn push_plain_call(&mut self, id: ObjectId, this: &Value, args: &[Value], ret_slot: u32, entry: bool) -> JsResult<bool> {
        let Some(obj) = self.heap.try_get(id) else { return Ok(false) };
        let ObjectKind::Function(c) = &obj.kind else { return Ok(false) };
        let cf = c.code.call_flags;
        if cf & CALL_PLAIN == 0 {
            if c.code.lazy.is_some() {
                self.ensure_compiled(id)?;
                return self.push_plain_call(id, this, args, ret_slot, entry);
            }
            return Ok(false);
        }
        let (this, new_target) = if cf & CALL_ARROW != 0 {
            if c.arguments.is_some() {
                return Ok(false);
            }
            (c.this_value.clone().unwrap_or(Value::Undefined), c.new_target.clone().unwrap_or(Value::Undefined))
        } else if cf & CALL_STRICT != 0 {
            (this.undef_if_empty(), Value::Undefined)
        } else {
            match this.v() {
                V::Undefined | V::Null | V::Empty => (Value::Object(self.realm_global(c.code.realm.get())), Value::Undefined),
                V::Object(o) => (Value::Object(o), Value::Undefined),
                _ => return Ok(false),
            }
        };
        let code = std::ptr::NonNull::from(&*c.code);
        let parent_scope = c.scope;
        self.push_closure_frame(id, code, parent_scope, ArgSrc::Slice(args), args.len(), this, new_target, ret_slot, false, entry)?;
        Ok(true)
    }

    /// Compile closure `id`'s function if it is still lazy (once per
    /// function: closures of the same code share the result).
    #[inline(never)]
    pub(crate) fn ensure_compiled(&mut self, id: ObjectId) -> JsResult<()> {
        let code = match &self.obj(id).kind {
            ObjectKind::Function(c) if c.code.lazy.is_some() => c.code.clone(),
            _ => return Ok(()),
        };
        let compiled = match code.compiled.get() {
            Some(c) => c.clone(),
            None => {
                let c = match crate::compiler::compile_lazy(&code) {
                    Ok(c) => c,
                    Err(e) => return Err(self.new_error("SyntaxError", &e.message)),
                };
                if code.realm.get() != 0 {
                    c.set_realm(code.realm.get());
                }
                let _ = code.compiled.set(c.clone());
                c
            }
        };
        if let ObjectKind::Function(c) = &mut self.obj_mut(id).kind {
            c.code = compiled;
        }
        Ok(())
    }

    /// `CallR` of a native function: arguments are handed over straight
    /// from the caller's slots and the result is written back to slot `b`.
    #[inline(never)]
    pub(crate) fn try_native_call(&mut self, fi: usize, b: usize, argc: usize) -> JsResult<bool> {
        let cb = self.frames[fi].base as usize + b;
        let id = match self.vstack[cb].v() {
            V::Object(id) => id,
            _ => return Ok(false),
        };
        let Some(obj) = self.heap.try_get(id) else { return Ok(false) };
        let ObjectKind::Native(n) = &obj.kind else { return Ok(false) };
        if n.intrinsic != 0 {
            if let Some(v) = self.inline_intrinsic(n.intrinsic, cb, argc) {
                self.vstack[cb] = v;
                return Ok(true);
            }
        }
        let func = n.func;
        let this = self.vstack[cb + 1].undef_if_empty();
        let mut args = std::mem::take(&mut self.arg_buf);
        args.extend_from_slice(&self.vstack[cb + 2..cb + 2 + argc]);
        let r = self.call_native(id, func, this, &args, false);
        args.clear();
        if args.capacity() <= 64 {
            self.arg_buf = args;
        }
        let v = r?;
        let at = self.frames[fi].base as usize + b;
        self.vstack[at] = v;
        Ok(true)
    }

    /// Call the getter `g` with the value in slot `o` of frame `fi` as
    /// receiver (an inline-cache hit on an accessor): natives directly,
    /// JavaScript getters through a nested run.
    #[inline(never)]
    fn call_cached_getter(&mut self, g: ObjectId, o: usize, fi: usize) -> JsResult<Value> {
        let this = self.vstack[self.frames[fi].base as usize + o].clone();
        if let Some(ObjectKind::Native(n)) = self.heap.try_get(g).map(|x| &x.kind) {
            let f = n.func;
            return self.call_native(g, f, this, &[], false);
        }
        self.call(Value::Object(g), this, &[])
    }

    /// A tagged built-in computed without the native call when its
    /// receiver / arguments (at `cb + 1`, `cb + 2`...) are the common case;
    /// `None` falls back to the native.
    #[inline]
    fn inline_intrinsic(&self, tag: u8, cb: usize, argc: usize) -> Option<Value> {
        match tag {
            INTRINSIC_CHAR_CODE_AT => {
                let V::String(s) = self.vstack[cb + 1].v() else { return None };
                let i = match argc {
                    0 => 0.0,
                    _ => match self.vstack[cb + 2].v() {
                        V::Number(n) => n,
                        _ => return None,
                    },
                };
                let u = s.as_units();
                let k = i as usize;
                Some(if k as f64 == i && k < u.len() {
                    Value::Number(u[k] as f64)
                } else if i.is_nan() || (i > -1.0 && i < 1.0) {
                    // NaN and (-1, 1) truncate to 0.
                    match u.first() {
                        Some(&c) => Value::Number(c as f64),
                        None => Value::Number(f64::NAN),
                    }
                } else if i.trunc() as usize >= u.len() || i < 0.0 {
                    Value::Number(f64::NAN)
                } else {
                    Value::Number(u[i.trunc() as usize] as f64)
                })
            }
            INTRINSIC_FROM_CHAR_CODE if argc == 1 => match self.vstack[cb + 2].v() {
                V::Number(n) => Some(Value::String(JsStr::from_unit(to_uint32(n) as u16))),
                _ => None,
            },
            INTRINSIC_IMUL if argc >= 2 => match (self.vstack[cb + 2].v(), self.vstack[cb + 3].v()) {
                (V::Number(a), V::Number(b)) => Some(Value::Number(to_int32(a).wrapping_mul(to_int32(b)) as f64)),
                _ => None,
            },
            _ => None,
        }
    }

    /// `New(argc)` of a plain (non-derived, non-class-field) function
    /// constructor: allocates `this` from the callee's own `prototype`
    /// data property and builds the frame from the caller's slots (callee
    /// at `b`, arguments after it). Returns false when the general path is
    /// needed. The caller must be synced.
    pub(crate) fn try_fast_construct(&mut self, fi: usize, b: usize, argc: usize) -> JsResult<bool> {
        let cb = self.frames[fi].base as usize + b;
        let id = match self.vstack[cb].v() {
            V::Object(id) => id,
            _ => return Ok(false),
        };
        let Some(obj) = self.heap.try_get(id) else { return Ok(false) };
        let ObjectKind::Function(c) = &obj.kind else { return Ok(false) };
        let cf = c.code.call_flags;
        if cf & CALL_PLAIN_CONSTRUCT == 0 {
            if c.code.lazy.is_some() {
                self.ensure_compiled(id)?;
                return self.try_fast_construct(fi, b, argc);
            }
            return Ok(false);
        }
        let proto = match obj.props.get(&names::prototype()) {
            Some(p) if p.flags & ACCESSOR == 0 => match p.value.v() {
                V::Object(p) => p,
                _ => self.realm_intrinsics(c.code.realm.get()).object_proto,
            },
            _ => return Ok(false),
        };
        if self.frames.len() >= self.max_frames {
            return self.range_error("Maximum call stack size exceeded");
        }
        let code = std::ptr::NonNull::from(&*c.code);
        let parent_scope = c.scope;
        let this = Value::Object(self.alloc(Object::new(Some(proto), ObjectKind::Ordinary)));
        self.push_closure_frame(id, code, parent_scope, ArgSrc::Stack(cb + 1), argc, this, Value::Object(id), b as u32, true, false)?;
        Ok(true)
    }

    /// GlobalDeclarationInstantiation steps 1-8: a script whose declarations
    /// clash with the global environment declares nothing.
    fn global_decl_check(&mut self, d: &crate::bytecode::GlobalDecls) -> JsResult<()> {
        let g = self.global;
        for n in &d.lex {
            let key = PropertyKey::String(n.clone());
            // HasLexicalDeclaration / HasRestrictedGlobalProperty (var
            // declarations of scripts are non-configurable properties).
            let restricted = self.get_own_property_any(g, &key)?.is_some_and(|p| !p.configurable());
            if self.global_lex.get(n).is_some() || restricted {
                return Err(self.new_error("SyntaxError", &format!("Identifier '{}' has already been declared", n)));
            }
        }
        for n in d.vars.iter().chain(d.funcs.iter()) {
            if self.global_lex.get(n).is_some() {
                return Err(self.new_error("SyntaxError", &format!("Identifier '{}' has already been declared", n)));
            }
        }
        for n in &d.funcs {
            // CanDeclareGlobalFunction.
            let key = PropertyKey::String(n.clone());
            let ok = match self.get_own_property_any(g, &key)? {
                None => self.is_extensible_any(g)?,
                Some(p) => p.configurable() || (!p.is_accessor() && p.writable() && p.enumerable()),
            };
            if !ok {
                return Err(self.new_error("TypeError", &format!("Cannot declare global function '{}'", n)));
            }
        }
        for n in &d.vars {
            // CanDeclareGlobalVar.
            let key = PropertyKey::String(n.clone());
            if self.get_own_property_any(g, &key)?.is_none() && !self.is_extensible_any(g)? {
                return Err(self.new_error("TypeError", &format!("Cannot declare global variable '{}'", n)));
            }
        }
        Ok(())
    }

    /// OrdinaryCallBindThis of a sloppy function in the running realm.
    fn sloppy_this(&mut self, this: Value) -> JsResult<Value> {
        Ok(match this.v() {
            V::Undefined | V::Null | V::Empty => Value::Object(self.global),
            V::Object(_) => this,
            prim => Value::Object(self.to_object(prim.to_value())?),
        })
    }

    /// Build the frame for calling closure `id`; its slots are reserved on
    /// top of the value stack (push it next, or detach them).
    pub(crate) fn prepare_frame(&mut self, id: ObjectId, this: Value, args: &[Value], new_target: Value, is_construct: bool) -> JsResult<Frame> {
        self.ensure_compiled(id)?;
        let (code, parent_scope, this_value, arrow_nt, arrow_args) = match &self.obj(id).kind {
            ObjectKind::Function(c) => (c.code.clone(), c.scope, c.this_value.clone(), c.new_target.clone(), c.arguments.clone()),
            _ => unreachable!(),
        };
        if code.realm.get() != self.current_realm {
            // `this` wrappers, the global, `arguments` objects: the callee's
            // realm's.
            return self.in_realm(code.realm.get(), |r| r.prepare_frame(id, this, args, new_target, is_construct));
        }
        let flags = code.flags;
        // this binding
        let this = if flags.is_arrow {
            this_value.unwrap_or(Value::Undefined)
        } else if is_construct {
            if flags.is_derived {
                Value::Empty
            } else {
                let default = self.intrinsics.object_proto;
                let proto = self.proto_from_ctor(new_target.clone(), default)?;
                Value::Object(self.alloc(Object::new(Some(proto), ObjectKind::Ordinary)))
            }
        } else if flags.is_strict {
            this
        } else {
            self.sloppy_this(this)?
        };
        let new_target = if flags.is_arrow { arrow_nt.unwrap_or(Value::Undefined) } else { new_target };
        let nslots = code.scope_slots as usize;
        let mut slots = if nslots == 0 { Vec::new() } else { vec![Value::Undefined; nslots] };
        let mut arguments_obj = None;
        let mut rest_arr = None;
        if code.rest_loc.is_some() {
            let start = code.param_locs.len();
            let rest: Vec<Value> = if args.len() > start { args[start..].to_vec() } else { Vec::new() };
            rest_arr = Some(self.new_array_value(rest));
        }
        if code.arguments_loc.is_some() {
            // Unmapped arguments (strict code or non-simple parameters) have
            // the poison-pill callee.
            arguments_obj = Some(self.create_arguments_object(args, if flags.is_strict || !flags.has_simple_params { None } else { Some(id) }));
        } else if flags.is_arrow {
            arguments_obj = arrow_args;
        }
        // Slots last: nothing below may run script code that pushes frames.
        let kept = if flags.args_direct { args.len() } else { 0 };
        if kept > 0 {
            self.vstack.extend_from_slice(args);
        }
        let base = self.reserve_slots(code.frame_size());
        let stack = &mut self.vstack[base..];
        #[inline(always)]
        fn put(stack: &mut [Value], slots: &mut [Value], loc: VarLoc, v: Value) {
            match loc {
                VarLoc::Reg(r) => stack[r as usize] = v,
                VarLoc::Slot(s) => slots[s as usize] = v,
            }
        }
        for (i, &loc) in code.param_locs.iter().enumerate() {
            let v = args.get(i).cloned().unwrap_or(Value::Undefined);
            put(stack, &mut slots, loc, v);
        }
        if let (Some(rl), Some(arr)) = (code.rest_loc, rest_arr) {
            put(stack, &mut slots, rl, arr);
        }
        if let Some(sl) = code.self_loc {
            put(stack, &mut slots, sl, Value::Object(id));
        }
        if let (Some(al), Some(ao)) = (code.arguments_loc, &arguments_obj) {
            put(stack, &mut slots, al, ao.clone());
        }
        let scope = if nslots > 0 {
            self.alloc(Object::new(None, ObjectKind::Scope(Box::new(ScopeData { slots, parent: parent_scope, names: None }))))
        } else {
            match parent_scope {
                Some(p) => p,
                None => self.alloc(Object::new(None, ObjectKind::Scope(Box::new(ScopeData { slots, parent: None, names: None })))),
            }
        };
        if let (Some(map), Some(V::Object(ao)), true) = (&code.arg_map, arguments_obj.as_ref().map(Value::v), nslots > 0) {
            // Mapped arguments: index i aliases parameter i while both exist.
            let n = args.len().min(map.len());
            let slots: Vec<u32> = map[..n].iter().map(|l| if let Some(VarLoc::Slot(s)) = l { *s as u32 } else { ArgMap::UNMAPPED }).collect();
            if slots.iter().any(|&s| s != ArgMap::UNMAPPED) {
                self.obj_mut(ao).kind = ObjectKind::MappedArguments(Box::new(ArgMap { scope, slots }));
            }
        }
        Ok(Frame {
            code: std::ptr::NonNull::from(&*code),
            pc: 0,
            base: base as u32,
            argc: kept as u32,
            scope,
            scope_depth: 0,
            this,
            func: Some(id),
            new_target,
            ret_slot: 0,
            is_construct,
            entry: false,
            extra: arguments_obj.map(|a| Box::new(FrameExtra { arguments_obj: Some(a), ..Default::default() })),
        })
    }

    fn create_arguments_object(&mut self, args: &[Value], callee: Option<ObjectId>) -> Value {
        let proto = self.intrinsics.object_proto;
        let id = self.alloc(Object::new(Some(proto), ObjectKind::Arguments));
        for (i, a) in args.iter().enumerate() {
            self.define_property(id, PropertyKey::index(i as u32), a.clone(), ALL);
        }
        self.define_property(id, names::length(), Value::Number(args.len() as f64), HIDDEN);
        if let Some(c) = callee {
            self.define_property(id, names::callee(), Value::Object(c), HIDDEN);
        } else if let Some(tte) = self.intrinsics.others.get("ThrowTypeError").copied() {
            // Unmapped (strict) arguments: callee is a poison-pill accessor.
            self.define_accessor_values(id, names::callee(), Some(Value::Object(tte)), Some(Value::Object(tte)), 0);
        }
        if let Some(values) = self.intrinsics.others.get("ArrayProtoValues").copied() {
            self.define_property(id, PropertyKey::Symbol(SYM_ITERATOR), Value::Object(values), HIDDEN);
        }
        Value::Object(id)
    }

    // ----------------------------------------------------------- generators

    /// Calling a generator / async function: create the generator object
    /// (and for async functions, start it immediately).
    fn start_generator_like(&mut self, id: ObjectId, this: Value, args: &[Value]) -> JsResult<Value> {
        let code = match &self.obj(id).kind {
            ObjectKind::Function(c) => c.code.clone(),
            _ => unreachable!(),
        };
        let flags = code.flags;
        let mut frame = self.prepare_frame(id, this, args, Value::Undefined, false)?;
        let is_async = flags.is_async;
        let is_gen = flags.is_generator;
        if is_gen {
            // Run FunctionDeclarationInstantiation now (errors in argument
            // binding throw from the call); the body starts suspended at
            // the compiler's initial `Yield`.
            let fi = self.realm_intrinsics(code.realm.get());
            let fallback = if is_async { fi.async_generator_proto } else { fi.generator_proto };
            let gen = self.alloc(Object::new(
                Some(fallback),
                ObjectKind::Generator(Box::new(GeneratorData { state: GeneratorState::Executing, frame: None, is_async, promise: None, queue: Default::default() })),
            ));
            frame.extra_mut().generator = Some(gen);
            self.push_frame(frame, true)?;
            let parked = match self.run() {
                Ok(Exit::Yield(_, f)) => Some(f),
                Ok(_) => None,
                Err(e) => {
                    if let ObjectKind::Generator(g) = &mut self.obj_mut(gen).kind {
                        g.state = GeneratorState::Completed;
                    }
                    return Err(e);
                }
            };
            if let ObjectKind::Generator(g) = &mut self.obj_mut(gen).kind {
                g.state = if parked.is_some() { GeneratorState::SuspendedStart } else { GeneratorState::Completed };
                g.frame = parked;
            }
            if let V::Object(p) = self.get(Value::Object(id), names::prototype())?.v() {
                self.obj_mut(gen).set_proto(Some(p));
            }
            return Ok(Value::Object(gen));
        }
        // Suspended at its start: the slots wait inside the frame.
        self.detach_slots(&mut frame);
        let proto = if is_gen {
            // generator object prototype = fn.prototype (if object) else intrinsic
            match (self.get(Value::Object(id), names::prototype())?).v() {
                V::Object(p) => p,
                _ => {
                    let fi = self.realm_intrinsics(code.realm.get());
                    if is_async {
                        fi.async_generator_proto
                    } else {
                        fi.generator_proto
                    }
                }
            }
        } else {
            self.intrinsics.object_proto
        };
        let promise = if is_async && !is_gen { Some(self.new_promise_object()) } else { None };
        let gen = self.alloc(Object::new(
            Some(proto),
            ObjectKind::Generator(Box::new(GeneratorData { state: GeneratorState::SuspendedStart, frame: None, is_async, promise, queue: Default::default() })),
        ));
        frame.extra_mut().generator = Some(gen);
        if let ObjectKind::Generator(g) = &mut self.obj_mut(gen).kind {
            g.frame = Some(Box::new(frame));
        }
        if is_async && !is_gen {
            // async function: run until the first await
            self.async_function_step(gen, ResumeMode::Next, Value::Undefined);
            return Ok(Value::Object(promise.unwrap()));
        }
        Ok(Value::Object(gen))
    }

    /// Resume a suspended generator frame. Returns the raw exit.
    pub(crate) fn generator_resume_raw(&mut self, gen: ObjectId, mode: ResumeMode, value: Value) -> JsResult<Step> {
        let (state, frame) = match &mut self.obj_mut(gen).kind {
            ObjectKind::Generator(g) => (g.state, g.frame.take()),
            _ => return self.type_error("not a generator"),
        };
        let set_state = |realm: &mut Realm, st: GeneratorState, fr: Option<Box<Frame>>| {
            if let ObjectKind::Generator(g) = &mut realm.obj_mut(gen).kind {
                g.state = st;
                g.frame = fr;
            }
        };
        match state {
            GeneratorState::Executing => {
                set_state(self, state, frame);
                return self.type_error("Generator is already running");
            }
            GeneratorState::Completed => {
                set_state(self, state, None);
                return match mode {
                    ResumeMode::Next => Ok(Step::Return(Value::Undefined)),
                    ResumeMode::Return => Ok(Step::Return(value)),
                    ResumeMode::Throw => Err(value),
                };
            }
            GeneratorState::SuspendedStart => {
                if mode != ResumeMode::Next {
                    set_state(self, GeneratorState::Completed, None);
                    return match mode {
                        ResumeMode::Return => Ok(Step::Return(value)),
                        _ => Err(value),
                    };
                }
            }
            GeneratorState::SuspendedYield => {}
        }
        let mut frame = match frame {
            Some(f) => *f,
            None => return self.type_error("Generator frame missing"),
        };
        set_state(self, GeneratorState::Executing, None);
        self.attach_slots(&mut frame);
        if state == GeneratorState::SuspendedStart && frame.code().flags.is_generator {
            // Parked at the initial yield: its result slot is popped.
            let slot = frame.base as usize + frame.extra.as_ref().map_or(0, |e| e.resume_slot as usize);
            self.vstack[slot] = Value::Undefined;
        }
        // `yield*` dispatches every resumption itself.
        let mut throw_in = mode == ResumeMode::Throw;
        if state == GeneratorState::SuspendedYield && matches!(frame.code().ops.get(frame.pc), Some(Op::ResumeMode)) {
            let slot = frame.base as usize + frame.extra.as_ref().map_or(0, |e| e.resume_slot as usize);
            self.vstack[slot] = value.clone();
            self.resume_mode = match mode {
                ResumeMode::Next => 0,
                ResumeMode::Throw => 1,
                ResumeMode::Return => 2,
            };
            throw_in = false;
        } else if state == GeneratorState::SuspendedYield {
            match mode {
                ResumeMode::Next => {
                    let slot = frame.base as usize + frame.extra.as_ref().map_or(0, |e| e.resume_slot as usize);
                    self.vstack[slot] = value.clone();
                }
                ResumeMode::Return => {
                    if matches!(frame.code().ops.get(frame.pc), Some(Op::ResumeIsReturn)) {
                        // The yield has a compiled return path (finally
                        // blocks, iterator closes): resume into it.
                        let slot = frame.base as usize + frame.extra.as_ref().map_or(0, |e| e.resume_slot as usize);
                        self.vstack[slot] = value.clone();
                        self.resume_mode = 2;
                    } else {
                        self.vstack.truncate(frame.bottom());
                        set_state(self, GeneratorState::Completed, None);
                        return Ok(Step::Return(value));
                    }
                }
                ResumeMode::Throw => {}
            }
        }
        if let Err(e) = self.push_frame(frame, true) {
            set_state(self, GeneratorState::Completed, None);
            return Err(e);
        }
        self.entry_tick();
        let result = if throw_in {
            match self.unwind(value) {
                Ok(()) => self.run(),
                Err(e) => Err(e),
            }
        } else {
            self.run()
        };
        match result {
            Ok(Exit::Return(v)) => {
                set_state(self, GeneratorState::Completed, None);
                Ok(Step::Return(v))
            }
            Ok(Exit::Yield(v, f)) => {
                set_state(self, GeneratorState::SuspendedYield, Some(f));
                Ok(Step::Yield(v))
            }
            Ok(Exit::Await(v, f)) => {
                set_state(self, GeneratorState::SuspendedYield, Some(f));
                Ok(Step::Await(v))
            }
            Err(e) => {
                set_state(self, GeneratorState::Completed, None);
                Err(e)
            }
        }
    }

    /// `generator.next/throw/return` for sync generators: returns an iterator result.
    pub fn generator_resume(&mut self, gen: ObjectId, mode: ResumeMode, value: Value) -> JsResult<Value> {
        match self.generator_resume_raw(gen, mode, value)? {
            Step::Return(v) => Ok(self.create_iter_result(v, true)),
            // `yield*` hands on the inner iterator's result object as is.
            Step::Yield(v) if std::mem::replace(&mut self.yield_raw, false) => Ok(v),
            Step::Yield(v) => Ok(self.create_iter_result(v, false)),
            Step::Await(_) => self.type_error("await in a non-async generator"),
        }
    }

    /// Drive an async function: run until completion or the next await.
    pub(crate) fn async_function_step(&mut self, gen: ObjectId, mode: ResumeMode, value: Value) {
        let promise = match &self.obj(gen).kind {
            ObjectKind::Generator(g) => g.promise,
            _ => None,
        };
        let promise = match promise {
            Some(p) => p,
            None => return,
        };
        match self.generator_resume_raw(gen, mode, value) {
            Ok(Step::Return(v)) => self.resolve_promise(promise, v),
            Ok(Step::Await(v)) | Ok(Step::Yield(v)) => self.await_value(gen, v),
            Err(e) => self.reject_promise(promise, e),
        }
    }

    /// Subscribe the generator `gen` to the settlement of `v`.
    pub(crate) fn await_value(&mut self, gen: ObjectId, v: Value) {
        let p = match crate::builtins::promise::promise_resolve(self, v) {
            Ok(p) => p,
            Err(e) => {
                self.async_step_dispatch(gen, ResumeMode::Throw, e);
                return;
            }
        };
        crate::builtins::promise::perform_then_native(self, p, on_await_fulfilled, on_await_rejected, Value::Object(gen));
    }

    pub(crate) fn async_step_dispatch(&mut self, gen: ObjectId, mode: ResumeMode, value: Value) {
        let is_gen = match &self.obj(gen).kind {
            ObjectKind::Generator(g) => g.promise.is_none(),
            _ => false,
        };
        if is_gen {
            crate::builtins::iterator::async_generator_resume_step(self, gen, mode, value);
        } else {
            self.async_function_step(gen, mode, value);
        }
    }

    // ------------------------------------------------------------ run loop

    /// Find a handler for a thrown value, popping frames as needed. Returns
    /// `Err` when the exception escapes the entry frame.
    pub(crate) fn unwind(&mut self, err: Value) -> Result<(), Value> {
        loop {
            let frame = match self.frames.last_mut() {
                Some(f) => f,
                None => return Err(err),
            };
            let pc = frame.pc.saturating_sub(1) as u32;
            // A watchdog timeout cannot be caught by the script.
            let handler = if self.terminating { None } else { frame.code().handlers.iter().find(|h| h.start <= pc && pc < h.end).copied() };
            if let Some(h) = handler {
                let slot = frame.code().nregs as usize + h.stack_depth as usize;
                while frame.scope_depth > h.scope_depth {
                    let parent = match &self.heap.get(frame.scope).kind {
                        ObjectKind::Scope(s) => s.parent,
                        _ => None,
                    };
                    frame.scope = parent.expect("scope underflow");
                    frame.scope_depth -= 1;
                }
                frame.pc = h.target as usize;
                let at = frame.base as usize + slot;
                self.vstack[at] = err;
                return Ok(());
            }
            let f = self.pop_frame();
            if let Some(g) = f.generator() {
                if let ObjectKind::Generator(gd) = &mut self.obj_mut(g).kind {
                    gd.state = GeneratorState::Completed;
                    gd.frame = None;
                }
            }
            if f.entry {
                return Err(err);
            }
        }
    }

    /// Execute until the entry frame returns or suspends.
    pub(crate) fn run(&mut self) -> JsResult<Exit> {
        // Each re-entry from Rust nests a new interpreter loop on the
        // native stack: stop before it overflows (the error surfaces at
        // the call site, before the callee runs).
        if crate::stack::exhausted() {
            let err = self.new_error("RangeError", "Maximum call stack size exceeded");
            while let Some(f) = self.frames.last().map(|f| f.entry) {
                let popped = self.pop_frame();
                if let Some(g) = popped.generator() {
                    if let ObjectKind::Generator(gd) = &mut self.obj_mut(g).kind {
                        gd.state = GeneratorState::Completed;
                        gd.frame = None;
                    }
                }
                if f {
                    break;
                }
            }
            return Err(err);
        }
        let caller_realm = self.current_realm;
        let outer_entry_realm = std::mem::replace(&mut self.entry_realm, caller_realm);
        let r = loop {
            match self.run_inner() {
                Ok(exit) => break Ok(exit),
                Err(e) => {
                    if let Err(e) = self.unwind(e) {
                        break Err(e);
                    }
                }
            }
        };
        self.entry_realm = outer_entry_realm;
        if self.current_realm != caller_realm {
            self.switch_realm(caller_realm);
        }
        r
    }

    #[inline]
    fn scope_at(&self, mut scope: ObjectId, depth: u16) -> ObjectId {
        for _ in 0..depth {
            scope = match &self.heap.get(scope).kind {
                ObjectKind::Scope(s) => s.parent.expect("scope chain too short"),
                _ => panic!("not a scope"),
            };
        }
        scope
    }

    #[inline]
    fn slot_get(&self, scope: ObjectId, depth: u16, slot: u16) -> Value {
        let s = self.scope_at(scope, depth);
        match &self.heap.get(s).kind {
            ObjectKind::Scope(sd) => match sd.slots.get(slot as usize) {
                Some(v) => v.clone(),
                None => {
                    // A compiler scope bug must not take the whole browser
                    // down; report it when debugging and read `undefined`.
                    if std::env::var("RH_DEBUG_SLOTS").is_ok() {
                        eprintln!("[slot] read d={depth} s={slot} len={} at\n{}", sd.slots.len(), self.build_stack_string("Debug", "slot"));
                    }
                    Value::Undefined
                }
            },
            _ => Value::Undefined,
        }
    }

    #[inline]
    fn slot_set(&mut self, scope: ObjectId, depth: u16, slot: u16, v: Value) {
        let s = self.scope_at(scope, depth);
        if let ObjectKind::Scope(sd) = &mut self.heap.get_mut(s).kind {
            if let Some(dst) = sd.slots.get_mut(slot as usize) {
                *dst = v;
            }
        }
    }

    /// Calls from Rust (callbacks of built-ins, host events) count as calls
    /// for the collector: a callback without loops or calls of its own,
    /// run many times by a native, still reaches safe points. Called once
    /// the callee's frame holds its arguments.
    #[inline]
    fn entry_tick(&mut self) {
        self.tick = self.tick.wrapping_sub(1);
        if self.tick == 0 {
            self.tick = self.tick_interval();
            self.maybe_gc();
        }
    }

    /// GC / watchdog safe point, reached every few hundred backward jumps
    /// and calls.
    #[inline(never)]
    fn safepoint(&mut self) -> JsResult<()> {
        if self.gcx.stress == crate::realm::GC_STRESS_FULL && self.gc_enabled && self.gc_block == 0 {
            self.gc();
        }
        self.maybe_gc();
        if crate::sampler::js::enabled() && crate::sampler::js::take_pending() {
            let frames: Vec<String> = self
                .frames
                .iter()
                .rev()
                .take(12)
                .map(|f| format!("{}@{}:{}", f.code().name, f.code().filename, f.code().positions.first().map(|p| p.line).unwrap_or(0)))
                .collect();
            crate::sampler::js::sample(&frames);
        }
        self.check_watchdog()
    }

    /// The watchdog part of a safe point, for hosts running long work on
    /// behalf of a script outside the interpreter loop (WebAssembly loops
    /// poll it): past the deadline, an uncatchable "Script execution timed
    /// out" error to propagate.
    pub fn check_watchdog(&mut self) -> JsResult<()> {
        if self.heap.live_count > self.heap_limit {
            return Err(self.out_of_memory());
        }
        if let Some(d) = self.deadline {
            if self.terminating || self.watch_now() >= d {
                if !self.terminating && self.apply_watchdog_credit() && self.watch_now() < self.deadline.unwrap_or(d) {
                    return Ok(());
                }
                self.terminating = true;
                return Err(self.new_error("RangeError", "Script execution timed out"));
            }
        }
        Ok(())
    }

    /// Stop the running script: the heap is over [`Realm::heap_limit`] or
    /// a native operation would need more memory than it may take. The
    /// error is uncatchable like a timeout (handlers are skipped while it
    /// unwinds); the next task starts normally.
    #[cold]
    pub fn out_of_memory(&mut self) -> Value {
        self.terminating = true;
        self.new_error("RangeError", "Out of memory")
    }

    /// The interpreter loop. Register instructions address frame slots
    /// directly (registers, then the operand stack at static depths), so
    /// the loop keeps only the code, the program counter and the slot base
    /// pointer in locals. Only fast paths live here; every miss, and every
    /// stack instruction, goes through [`Realm::exec_op`], which implements
    /// each instruction completely.
    ///
    /// Register pressure matters more than anything else here: the frame
    /// index, the frame's slot offset and the current instruction are not
    /// kept live across the loop (out-of-line paths recompute them from the
    /// last frame and re-read the instruction at `pc - 1`), and everything
    /// that is neither a fast path nor straight-line code (calls, returns,
    /// accessor and setter calls, megamorphic lookups) lives in
    /// `#[inline(never)]` functions returning a [`Flow`], so the hot values
    /// stay in callee-saved registers instead of being spilled.
    fn run_inner(&mut self) -> JsResult<Exit> {
        // With native code (or its statistics) on, the loop instance with
        // the compiler hooks; otherwise one identical to a build without
        // the compiler.
        #[cfg(feature = "jit")]
        if self.jit.active {
            self.run_loop::<true>();
        } else {
            self.run_loop::<false>();
        }
        #[cfg(not(feature = "jit"))]
        self.run_loop::<false>();
        // SAFETY: `run_loop` only returns after setting `loop_end`.
        unsafe { self.loop_end.take().unwrap_unchecked() }
    }

    /// The loop of [`Realm::run_inner`]. It returns nothing (its result
    /// goes to `loop_end`): a returned `JsResult<Exit>` is written through
    /// a hidden pointer that would hold a register for the whole loop.
    #[allow(unused_unsafe, unused_assignments)]
    #[inline(never)]
    fn run_loop<const JIT: bool>(&mut self) {
        'reload: loop {
            // The frame's code, borrowed independently of `self` (no
            // reference count traffic per call / return).
            // SAFETY: the code of the last frame lives as long as the frame
            // (see `Frame::code`), and this loop runs it only while it is
            // the last frame: every path that pops or replaces the frame
            // leaves through `continue 'reload` or a return.
            let (code, mut pc, mut base): (&FunctionCode, usize, *mut Value) = unsafe {
                let f = self.frames.last().unwrap_unchecked();
                debug_assert!(self.vstack.len() >= f.base as usize + f.code().frame_size());
                (f.code.as_ref(), f.pc, self.vstack.as_mut_ptr().add(f.base as usize))
            };
            if code.realm.get() != self.current_realm {
                self.switch_realm(code.realm.get());
            }
            #[cfg(feature = "jit")]
            if JIT {
                match self.jit_reload(code, pc) {
                    Ctl::Next => {}
                    Ctl::Reload => continue 'reload,
                    Ctl::Done => return,
                }
            }
            let ops: &[Op] = &code.ops;
            let consts: &[Value] = &code.constants;
            macro_rules! sl {
                ($i:expr) => {
                    // SAFETY: operands are slots below `frame_size` (checked
                    // by the translator).
                    unsafe {
                        debug_assert!(($i as usize) < code.frame_size());
                        &mut *base.add($i as usize)
                    }
                };
            }
            macro_rules! k {
                ($i:expr) => {
                    // SAFETY: constant indices come from the compiler.
                    unsafe { consts.get_unchecked($i as usize) }
                };
            }
            macro_rules! frame {
                () => {
                    // SAFETY: the running frame is the last one.
                    unsafe { self.frames.last_mut().unwrap_unchecked() }
                };
            }
            // Result of an out-of-line step: `Next` continues the same
            // frame at its (synced) pc, with the value stack possibly moved.
            macro_rules! out {
                ($e:expr) => {{
                    match $e {
                        Ctl::Next => {
                            let f = frame!();
                            pc = f.pc;
                            // SAFETY: the frame's slots are within the vector.
                            base = unsafe { self.vstack.as_mut_ptr().add(f.base as usize) };
                        }
                        Ctl::Reload => continue 'reload,
                        Ctl::Done => return,
                    }
                }};
            }
            // `osr`: a backward jump (the frame may continue in native
            // code); calls tick before running, past their own pc.
            macro_rules! tick {
                () => {
                    tick!(false)
                };
                ($osr:expr) => {
                    self.tick = self.tick.wrapping_sub(1);
                    if self.tick == 0 {
                        out!(self.tick_slow(pc, $osr));
                    }
                };
            }
            macro_rules! jump {
                ($t:expr) => {{
                    let t = $t as usize;
                    if t < pc {
                        pc = t;
                        tick!(true);
                    } else {
                        pc = t;
                    }
                }};
            }
            // Out-of-line execution of the current instruction (re-read
            // from the code, so it need not stay live).
            macro_rules! slow {
                () => {{
                    out!(self.exec_at(pc))
                }};
            }
            // Out-of-line execution of another instruction in its place.
            macro_rules! slow_op {
                ($op:expr) => {{
                    out!(self.exec_with(pc, $op))
                }};
            }
            macro_rules! num2 {
                ($a:expr, $b:expr) => {{
                    let (x, y) = (&*sl!($a), &*sl!($b));
                    if x.is_number() && y.is_number() {
                        Some((x.number_unchecked(), y.number_unchecked()))
                    } else {
                        None
                    }
                }};
            }
            macro_rules! num_k {
                ($a:expr, $k:expr) => {{
                    let (x, y) = (&*sl!($a), k!($k));
                    if x.is_number() && y.is_number() {
                        Some((x.number_unchecked(), y.number_unchecked()))
                    } else {
                        None
                    }
                }};
            }
            macro_rules! set_num {
                ($d:expr, $v:expr) => {{
                    let r = $v;
                    sl!($d).set_number(r);
                }};
            }
            'dispatch: loop {
                // SAFETY: every path ends in a jump, return or throw, so the
                // program counter stays within the code.
                // Matched by reference: each arm loads its own operands
                // (a copy of the whole instruction would be decoded into
                // registers before the dispatch jump, for every arm).
                let op: &Op = unsafe {
                    debug_assert!(pc < ops.len());
                    ops.get_unchecked(pc)
                };
                pc += 1;
                match *op {
                    Op::Mov(d, s) => {
                        let v = sl!(s).undef_if_empty();
                        *sl!(d) = v;
                    }
                    Op::MovRaw(d, s) => {
                        let v = sl!(s).clone();
                        *sl!(d) = v;
                    }
                    Op::LoadK(d, c) => {
                        let v = k!(c).clone();
                        *sl!(d) = v;
                    }
                    Op::AddRR(d, a, b) => {
                        if let Some((x, y)) = num2!(a, b) {
                            set_num!(d, x + y);
                            continue;
                        }
                        if let (V::String(x), V::String(y)) = (sl!(a).v(), sl!(b).v()) {
                            if x.len() + y.len() > crate::value::MAX_STRING_LEN {
                                slow!();
                                continue;
                            }
                            let r = x.concat(y);
                            *sl!(d) = Value::String(r);
                            continue;
                        }
                        slow!();
                    }
                    Op::AddRK(d, a, c) => {
                        if let Some((x, y)) = num_k!(a, c) {
                            set_num!(d, x + y);
                            continue;
                        }
                        if let (V::String(x), V::String(y)) = (sl!(a).v(), k!(c).v()) {
                            if x.len() + y.len() > crate::value::MAX_STRING_LEN {
                                slow!();
                                continue;
                            }
                            let r = x.concat(y);
                            *sl!(d) = Value::String(r);
                            continue;
                        }
                        slow!();
                    }
                    Op::SubRR(d, a, b) => match num2!(a, b) {
                        Some((x, y)) => set_num!(d, x - y),
                        None => slow!(),
                    },
                    Op::SubRK(d, a, c) => match num_k!(a, c) {
                        Some((x, y)) => set_num!(d, x - y),
                        None => slow!(),
                    },
                    Op::MulRR(d, a, b) => match num2!(a, b) {
                        Some((x, y)) => set_num!(d, x * y),
                        None => slow!(),
                    },
                    Op::MulRK(d, a, c) => match num_k!(a, c) {
                        Some((x, y)) => set_num!(d, x * y),
                        None => slow!(),
                    },
                    Op::DivRR(d, a, b) => match num2!(a, b) {
                        Some((x, y)) => set_num!(d, x / y),
                        None => slow!(),
                    },
                    Op::DivRK(d, a, c) => match num_k!(a, c) {
                        Some((x, y)) => set_num!(d, x / y),
                        None => slow!(),
                    },
                    Op::ModRR(d, a, b) => match num2!(a, b) {
                        Some((x, y)) => set_num!(d, js_mod(x, y)),
                        None => slow!(),
                    },
                    Op::ModRK(d, a, c) => match num_k!(a, c) {
                        Some((x, y)) => set_num!(d, js_mod(x, y)),
                        None => slow!(),
                    },
                    Op::BinRR(kd, d, a, b) => match num2!(a, b) {
                        Some((x, y)) => set_num!(d, bin_by_index(kd, x, y)),
                        None => slow!(),
                    },
                    Op::BinRK(kd, d, a, c) => match num_k!(a, c) {
                        Some((x, y)) => set_num!(d, bin_by_index(kd, x, y)),
                        None => slow!(),
                    },
                    Op::CmpRR(kd, d, a, b) => {
                        let r = match (kd, num2!(a, b)) {
                            (0, Some((x, y))) => x < y,
                            (1, Some((x, y))) => x <= y,
                            (2, Some((x, y))) => x > y,
                            (3, Some((x, y))) => x >= y,
                            (4 | 6, Some((x, y))) => x == y,
                            (5 | 7, Some((x, y))) => x != y,
                            (4, None) => self.strict_equals(sl!(a), sl!(b)),
                            (5, None) => !self.strict_equals(sl!(a), sl!(b)),
                            _ => {
                                slow!();
                                continue;
                            }
                        };
                        *sl!(d) = Value::Bool(r);
                    }
                    Op::CmpRK(kd, d, a, c) => {
                        let r = match (kd, num_k!(a, c)) {
                            (0, Some((x, y))) => x < y,
                            (1, Some((x, y))) => x <= y,
                            (2, Some((x, y))) => x > y,
                            (3, Some((x, y))) => x >= y,
                            (4 | 6, Some((x, y))) => x == y,
                            (5 | 7, Some((x, y))) => x != y,
                            (4, None) => self.strict_equals(sl!(a), k!(c)),
                            (5, None) => !self.strict_equals(sl!(a), k!(c)),
                            _ => {
                                slow!();
                                continue;
                            }
                        };
                        *sl!(d) = Value::Bool(r);
                    }
                    Op::JumpUnlessLtRR(a, b, t) => match num2!(a, b) {
                        Some((x, y)) => {
                            if !(x < y) {
                                jump!(t);
                            }
                        }
                        None => slow!(),
                    },
                    Op::JumpUnlessLtRK(a, c, t) => match num_k!(a, c) {
                        Some((x, y)) => {
                            if !(x < y) {
                                jump!(t);
                            }
                        }
                        None => slow!(),
                    },
                    Op::JumpUnlessLtEqRR(a, b, t) => match num2!(a, b) {
                        Some((x, y)) => {
                            if !(x <= y) {
                                jump!(t);
                            }
                        }
                        None => slow!(),
                    },
                    Op::JumpUnlessLtEqRK(a, c, t) => match num_k!(a, c) {
                        Some((x, y)) => {
                            if !(x <= y) {
                                jump!(t);
                            }
                        }
                        None => slow!(),
                    },
                    Op::JumpUnlessGtRR(a, b, t) => match num2!(a, b) {
                        Some((x, y)) => {
                            if !(x > y) {
                                jump!(t);
                            }
                        }
                        None => slow!(),
                    },
                    Op::JumpUnlessGtRK(a, c, t) => match num_k!(a, c) {
                        Some((x, y)) => {
                            if !(x > y) {
                                jump!(t);
                            }
                        }
                        None => slow!(),
                    },
                    Op::JumpUnlessGtEqRR(a, b, t) => match num2!(a, b) {
                        Some((x, y)) => {
                            if !(x >= y) {
                                jump!(t);
                            }
                        }
                        None => slow!(),
                    },
                    Op::JumpUnlessGtEqRK(a, c, t) => match num_k!(a, c) {
                        Some((x, y)) => {
                            if !(x >= y) {
                                jump!(t);
                            }
                        }
                        None => slow!(),
                    },
                    Op::JumpIfLtRR(a, b, t) => match num2!(a, b) {
                        Some((x, y)) => {
                            if x < y {
                                jump!(t);
                            }
                        }
                        None => slow!(),
                    },
                    Op::JumpIfLtRK(a, c, t) => match num_k!(a, c) {
                        Some((x, y)) => {
                            if x < y {
                                jump!(t);
                            }
                        }
                        None => slow!(),
                    },
                    Op::JumpIfLtEqRR(a, b, t) => match num2!(a, b) {
                        Some((x, y)) => {
                            if x <= y {
                                jump!(t);
                            }
                        }
                        None => slow!(),
                    },
                    Op::JumpIfLtEqRK(a, c, t) => match num_k!(a, c) {
                        Some((x, y)) => {
                            if x <= y {
                                jump!(t);
                            }
                        }
                        None => slow!(),
                    },
                    Op::JumpIfGtRR(a, b, t) => match num2!(a, b) {
                        Some((x, y)) => {
                            if x > y {
                                jump!(t);
                            }
                        }
                        None => slow!(),
                    },
                    Op::JumpIfGtRK(a, c, t) => match num_k!(a, c) {
                        Some((x, y)) => {
                            if x > y {
                                jump!(t);
                            }
                        }
                        None => slow!(),
                    },
                    Op::JumpIfGtEqRR(a, b, t) => match num2!(a, b) {
                        Some((x, y)) => {
                            if x >= y {
                                jump!(t);
                            }
                        }
                        None => slow!(),
                    },
                    Op::JumpIfGtEqRK(a, c, t) => match num_k!(a, c) {
                        Some((x, y)) => {
                            if x >= y {
                                jump!(t);
                            }
                        }
                        None => slow!(),
                    },
                    Op::JumpIfStrictEqRR(a, b, t) => {
                        if self.strict_equals(sl!(a), sl!(b)) {
                            jump!(t);
                        }
                    }
                    Op::JumpIfStrictNotEqRR(a, b, t) => {
                        if !self.strict_equals(sl!(a), sl!(b)) {
                            jump!(t);
                        }
                    }
                    Op::JumpIfStrictEqRK(a, c, t) => {
                        if self.strict_equals(sl!(a), k!(c)) {
                            jump!(t);
                        }
                    }
                    Op::JumpIfStrictNotEqRK(a, c, t) => {
                        if !self.strict_equals(sl!(a), k!(c)) {
                            jump!(t);
                        }
                    }
                    Op::JumpIfTrueR(s, t) => {
                        if self.to_boolean(sl!(s)) {
                            jump!(t);
                        }
                    }
                    Op::JumpIfFalseR(s, t) => {
                        if !self.to_boolean(sl!(s)) {
                            jump!(t);
                        }
                    }
                    Op::Jump(t) => jump!(t),
                    Op::NotR(d, s) => {
                        let r = !self.to_boolean(sl!(s));
                        *sl!(d) = Value::Bool(r);
                    }
                    Op::IncJumpIfLtRK(r, c, t) => {
                        let (x, y) = (&*sl!(r), k!(c));
                        if x.is_number() && y.is_number() {
                            let (n, lim) = (x.number_unchecked() + 1.0, y.number_unchecked());
                            sl!(r).set_number(n);
                            if n < lim {
                                jump!(t);
                            } else {
                                // Skip the loop's entry test that follows.
                                pc += 1;
                            }
                        } else {
                            // The increment, then the test that follows.
                            slow_op!(Op::IncR(r, r));
                        }
                    }
                    Op::IncJumpIfLtRR(r, b, t) => {
                        let (x, y) = (&*sl!(r), &*sl!(b));
                        if x.is_number() && y.is_number() {
                            let (n, lim) = (x.number_unchecked() + 1.0, y.number_unchecked());
                            sl!(r).set_number(n);
                            if n < lim {
                                jump!(t);
                            } else {
                                pc += 1;
                            }
                        } else {
                            slow_op!(Op::IncR(r, r));
                        }
                    }
                    Op::IncR(d, s) => match sl!(s).as_number() {
                        Some(n) => set_num!(d, n + 1.0),
                        None => slow!(),
                    },
                    Op::DecR(d, s) => match sl!(s).as_number() {
                        Some(n) => set_num!(d, n - 1.0),
                        None => slow!(),
                    },
                    Op::UnR(k, d, s) => match (k, sl!(s).v()) {
                        // Neg, Plus, BitNot of numbers; ToPropertyKey of
                        // numbers and strings is the value itself.
                        (0, V::Number(n)) => set_num!(d, -n),
                        (1 | 5, V::Number(n)) => set_num!(d, n),
                        (2, V::Number(n)) => set_num!(d, !to_int32(n) as f64),
                        (5, V::String(_)) => {
                            let v = sl!(s).clone();
                            *sl!(d) = v;
                        }
                        _ => slow!(),
                    },
                    Op::ToNumericR(d, s) => match sl!(s).v() {
                        V::Number(n) => set_num!(d, n),
                        _ => slow!(),
                    },
                    Op::LoadThis(d) => {
                        let v = frame!().this.undef_if_empty();
                        *sl!(d) = v;
                    }
                    Op::GetPropR(d, o, ic) => {
                        let rec = code.ics[ic as usize].get();
                        let recv = &*sl!(o);
                        let hit = if rec.kind == crate::ic::IC_LENGTH {
                            match recv.v() {
                                V::String(s) => Some(Value::Number(s.len() as f64)),
                                V::Object(id) => match self.heap.try_get(id).map(|o| &o.kind) {
                                    Some(ObjectKind::Array { length, .. }) => Some(Value::Number(*length as f64)),
                                    _ => None,
                                },
                                _ => None,
                            }
                        } else {
                            match self.ic_get(&rec, recv) {
                                Some(p) if p.flags & ACCESSOR == 0 => Some(p.value.clone()),
                                _ => None,
                            }
                        };
                        match hit {
                            Some(v) => *sl!(d) = v,
                            None => out!(self.get_prop_miss(pc, d, o, ic)),
                        }
                    }
                    Op::SetPropR(o, v, ic) => {
                        let rec = code.ics[ic as usize].get();
                        let val = &*sl!(v);
                        if rec.kind != 0 && rec.kind != crate::ic::IC_SETTER && !val.is_empty() {
                            // SAFETY: property caches always have a string key.
                            let key = unsafe { k!(*code.ic_keys.get_unchecked(ic as usize)).as_js_str_unchecked() };
                            let val = val.clone();
                            match self.ic_set(&rec, sl!(o), key, val) {
                                Ok(()) => continue 'dispatch,
                                Err(v) => drop(v),
                            }
                        }
                        out!(self.set_prop_miss(pc, o, v, ic));
                    }
                    Op::DefinePropR(o, v, ic) => {
                        let rec = code.ics[ic as usize].get();
                        let val = &*sl!(v);
                        if rec.kind == crate::ic::IC_ADD && !val.is_empty() {
                            // SAFETY: property caches always have a string key.
                            let key = unsafe { k!(*code.ic_keys.get_unchecked(ic as usize)).as_js_str_unchecked() };
                            let val = val.clone();
                            match self.ic_set(&rec, sl!(o), key, val) {
                                Ok(()) => continue 'dispatch,
                                Err(v) => drop(v),
                            }
                        }
                        slow!();
                    }
                    Op::GetGlobalR(d, ic) => {
                        let cell = &code.ics[ic as usize];
                        // SAFETY: global caches always have a string key.
                        let key = unsafe { k!(*code.ic_keys.get_unchecked(ic as usize)).as_js_str_unchecked() };
                        match self.ic_get_global(cell, key) {
                            Some(v) => {
                                let v = v.clone();
                                *sl!(d) = v;
                            }
                            None => slow!(),
                        }
                    }
                    Op::SetGlobalR(s, ic) => {
                        let cell = &code.ics[ic as usize];
                        let val = &*sl!(s);
                        if !val.is_empty() {
                            // SAFETY: global caches always have a string key.
                            let key = unsafe { k!(*code.ic_keys.get_unchecked(ic as usize)).as_js_str_unchecked() };
                            let val = val.clone();
                            match self.ic_set_global(cell, key, val) {
                                Ok(()) => continue,
                                Err(v) => drop(v),
                            }
                        }
                        slow!();
                    }
                    Op::GetElemR(d, o, kk) => match self.get_elem_fast(sl!(o), sl!(kk)) {
                        Some(v) => *sl!(d) = v,
                        None => slow!(),
                    },
                    Op::SetElemR(o, kk, v) => {
                        let val = sl!(v).clone();
                        if !val.is_empty() {
                            match self.set_elem_fast(sl!(o), sl!(kk), val) {
                                Ok(()) => continue,
                                Err(v) => drop(v),
                            }
                        }
                        slow!();
                    }
                    Op::GetLocalR(d, dp, s) => {
                        let scope = frame!().scope;
                        let v = self.slot_get(scope, dp, s);
                        *sl!(d) = if v.is_empty() { Value::Undefined } else { v };
                    }
                    Op::SetLocalR(v, dp, s) => {
                        let val = sl!(v).undef_if_empty();
                        let scope = frame!().scope;
                        self.slot_set(scope, dp, s, val);
                    }
                    Op::StoreCompletionR(s) => {
                        let v = sl!(s).clone();
                        frame!().extra_mut().completion = v;
                    }
                    Op::CallR(b, argc, cic, _) => {
                        tick!();
                        out!(self.call_r(pc, b, argc, cic));
                    }
                    Op::NewR(b, argc) => {
                        tick!();
                        out!(self.new_r(pc, b, argc));
                    }
                    Op::ReturnR(s) => {
                        let v = std::mem::replace(sl!(s), Value::Undefined);
                        out!(self.return_r(v));
                    }
                    _ => slow!(),
                }
            }
        }
    }

    /// One property / global / element / scope / `this` instruction for
    /// native code: the run loop's fast paths (the same inline caches),
    /// then the out-of-line paths. `pc` is the instruction's index.
    #[cfg(feature = "jit")]
    #[inline(never)]
    pub(crate) fn step_access(&mut self, code: &FunctionCode, base: *mut Value, pc: usize) -> Ctl {
        macro_rules! sl {
            ($i:expr) => {
                // SAFETY: validated slot operands of the running frame.
                unsafe { &mut *base.add($i as usize) }
            };
        }
        macro_rules! key {
            ($ic:expr) => {
                // SAFETY: property and global caches have a string key.
                unsafe { code.constants.get_unchecked(*code.ic_keys.get_unchecked($ic as usize) as usize).as_js_str_unchecked() }
            };
        }
        let next = pc + 1;
        // SAFETY: native code runs the last frame.
        let f = unsafe { self.frames.last_mut().unwrap_unchecked() };
        f.pc = next;
        match code.ops[pc] {
            Op::GetPropR(d, o, ic) => {
                let rec = code.ics[ic as usize].get();
                let recv = &*sl!(o);
                let hit = if rec.kind == crate::ic::IC_LENGTH {
                    match recv.v() {
                        V::String(s) => Some(Value::Number(s.len() as f64)),
                        V::Object(id) => match self.heap.try_get(id).map(|o| &o.kind) {
                            Some(ObjectKind::Array { length, .. }) => Some(Value::Number(*length as f64)),
                            _ => None,
                        },
                        _ => None,
                    }
                } else {
                    match self.ic_get(&rec, recv) {
                        Some(p) if p.flags & ACCESSOR == 0 => Some(p.value.clone()),
                        _ => None,
                    }
                };
                match hit {
                    Some(v) => {
                        *sl!(d) = v;
                        Ctl::Next
                    }
                    None => self.get_prop_miss(next, d, o, ic),
                }
            }
            Op::SetPropR(o, v, ic) => {
                let rec = code.ics[ic as usize].get();
                let val = &*sl!(v);
                if rec.kind != 0 && rec.kind != crate::ic::IC_SETTER && !val.is_empty() {
                    let val = val.clone();
                    match self.ic_set(&rec, sl!(o), key!(ic), val) {
                        Ok(()) => return Ctl::Next,
                        Err(v) => drop(v),
                    }
                }
                self.set_prop_miss(next, o, v, ic)
            }
            Op::DefinePropR(o, v, ic) => {
                let rec = code.ics[ic as usize].get();
                let val = &*sl!(v);
                if rec.kind == crate::ic::IC_ADD && !val.is_empty() {
                    let val = val.clone();
                    match self.ic_set(&rec, sl!(o), key!(ic), val) {
                        Ok(()) => return Ctl::Next,
                        Err(v) => drop(v),
                    }
                }
                self.exec_at(next)
            }
            Op::GetGlobalR(d, ic) => match self.ic_get_global(&code.ics[ic as usize], key!(ic)) {
                Some(v) => {
                    let v = v.clone();
                    *sl!(d) = v;
                    Ctl::Next
                }
                None => self.exec_at(next),
            },
            Op::SetGlobalR(s, ic) => {
                let val = &*sl!(s);
                if !val.is_empty() {
                    let val = val.clone();
                    match self.ic_set_global(&code.ics[ic as usize], key!(ic), val) {
                        Ok(()) => return Ctl::Next,
                        Err(v) => drop(v),
                    }
                }
                self.exec_at(next)
            }
            Op::GetElemR(d, o, kk) => match self.get_elem_fast(sl!(o), sl!(kk)) {
                Some(v) => {
                    *sl!(d) = v;
                    Ctl::Next
                }
                None => self.exec_at(next),
            },
            Op::SetElemR(o, kk, v) => {
                let val = sl!(v).clone();
                if !val.is_empty() {
                    match self.set_elem_fast(sl!(o), sl!(kk), val) {
                        Ok(()) => return Ctl::Next,
                        Err(v) => drop(v),
                    }
                }
                self.exec_at(next)
            }
            Op::GetLocalR(d, dp, s) => {
                let scope = f.scope;
                let v = self.slot_get(scope, dp, s);
                *sl!(d) = if v.is_empty() { Value::Undefined } else { v };
                Ctl::Next
            }
            Op::SetLocalR(v, dp, s) => {
                let val = sl!(v).undef_if_empty();
                let scope = f.scope;
                self.slot_set(scope, dp, s, val);
                Ctl::Next
            }
            Op::LoadThis(d) => {
                let v = f.this.undef_if_empty();
                *sl!(d) = v;
                Ctl::Next
            }
            Op::StoreCompletionR(s) => {
                let v = sl!(s).clone();
                f.extra_mut().completion = v;
                Ctl::Next
            }
            _ => self.exec_at(next),
        }
    }

    /// Convert the result of an out-of-line step for the run loop, keeping
    /// an exit or error in `loop_end`.
    #[inline(always)]
    fn ctl(&mut self, r: JsResult<Flow>) -> Ctl {
        match r {
            Ok(Flow::Next) => Ctl::Next,
            Ok(Flow::Reload) => Ctl::Reload,
            Ok(Flow::Exit(e)) => {
                self.loop_end = Some(Ok(e));
                Ctl::Done
            }
            Err(e) => {
                self.loop_end = Some(Err(e));
                Ctl::Done
            }
        }
    }

    /// A safe point reached by the run loop's tick counter.
    #[inline(never)]
    fn tick_slow(&mut self, pc: usize, osr: bool) -> Ctl {
        let r = self.tick_slow_impl(pc, osr);
        self.ctl(r)
    }

    #[inline(always)]
    fn tick_slow_impl(&mut self, pc: usize, osr: bool) -> JsResult<Flow> {
        self.tick = self.tick_interval();
        // SAFETY: called by the run loop, which has a frame.
        unsafe { self.frames.last_mut().unwrap_unchecked() }.pc = pc;
        self.safepoint()?;
        #[cfg(feature = "jit")]
        if self.jit.active && self.jit_osr(osr) {
            // Loop OSR: continue the frame in its native code from here
            // (every value is in the frame: nothing to reconstruct).
            return Ok(Flow::Reload);
        }
        #[cfg(not(feature = "jit"))]
        let _ = osr;
        Ok(Flow::Next)
    }

    /// Ticks between safe points (1 under `RH_GC_STRESS`).
    #[inline(always)]
    pub(crate) fn tick_interval(&self) -> u32 {
        if self.gcx.stress != 0 {
            return 1;
        }
        TICK_INTERVAL
    }

    /// A safe point reached by native code at a backward jump or call;
    /// `fpc` is the frame pc to sync.
    #[cfg(feature = "jit")]
    pub(crate) fn tick_slow_jit(&mut self, fpc: usize) -> Ctl {
        self.tick = self.tick_interval();
        // SAFETY: native code runs the last frame.
        unsafe { self.frames.last_mut().unwrap_unchecked() }.pc = fpc;
        if self.jit.stats.enabled {
            self.jit.stats.samples += 1;
            self.jit.stats.hot_samples += 1;
            self.jit.stats.jit_samples += 1;
        }
        let r = self.safepoint().map(|_| Flow::Next);
        self.ctl(r)
    }

    /// Hotness bookkeeping at an interpreter safe-point tick; true when
    /// the frame should continue in native code (a backward jump, `osr`,
    /// into a function that has or now gets native code).
    #[cfg(feature = "jit")]
    #[inline(never)]
    fn jit_osr(&mut self, osr: bool) -> bool {
        // SAFETY: the run loop has a frame; its code outlives this call.
        let code: &FunctionCode = unsafe { self.frames.last().unwrap_unchecked().code.as_ref() };
        let s = &code.jit.s;
        let n = s.counter.get().saturating_add(crate::jit::LOOP_WEIGHT);
        s.counter.set(n);
        if self.jit.stats.enabled {
            self.jit.stats.samples += 1;
            if n >= self.jit.threshold {
                self.jit.stats.hot_samples += 1;
            }
        }
        if !self.jit.on || !crate::jit::LOOP_OSR {
            return false;
        }
        if !osr {
            // A call's tick (the frame is past the call, not at a loop
            // edge): if this function is hot, catch the very next tick,
            // which in a loop is soon its backward jump.
            if n >= self.jit.threshold && s.state.get() != crate::jit::REJECTED {
                self.tick = 1;
            }
            return false;
        }
        if s.entry.get() == 0 && n >= self.jit.threshold {
            crate::jit::compile(code, &mut self.jit.stats);
        }
        s.entry.get() != 0
    }

    /// Out-of-line execution of the instruction before `pc` in the last
    /// frame (see [`Realm::exec_op`]).
    #[inline(never)]
    pub(crate) fn exec_at(&mut self, pc: usize) -> Ctl {
        let r = self.exec_at_impl(pc);
        self.ctl(r)
    }

    #[inline(always)]
    fn exec_at_impl(&mut self, pc: usize) -> JsResult<Flow> {
        let fi = self.frames.len() - 1;
        // SAFETY: `fi` is the running frame and `pc - 1` its instruction.
        let op = unsafe {
            let f = self.frames.get_unchecked_mut(fi);
            f.pc = pc;
            *f.code().ops.get_unchecked(pc - 1)
        };
        self.exec_in(fi, op)
    }

    /// Out-of-line execution of `op` in place of the instruction before `pc`.
    #[inline(never)]
    pub(crate) fn exec_with(&mut self, pc: usize, op: Op) -> Ctl {
        let r = self.exec_with_impl(pc, op);
        self.ctl(r)
    }

    #[inline(always)]
    fn exec_with_impl(&mut self, pc: usize, op: Op) -> JsResult<Flow> {
        let fi = self.frames.len() - 1;
        // SAFETY: `fi` is the running frame.
        unsafe { self.frames.get_unchecked_mut(fi) }.pc = pc;
        self.exec_in(fi, op)
    }

    #[inline(always)]
    fn exec_in(&mut self, fi: usize, op: Op) -> JsResult<Flow> {
        let flow = self.exec_op(fi, op)?;
        if matches!(flow, Flow::Next) && self.frames.len() != fi + 1 {
            return Ok(Flow::Reload);
        }
        Ok(flow)
    }

    /// `GetPropR` after an inline-cache miss: the megamorphic cache,
    /// cached accessors, then the full instruction.
    #[inline(never)]
    pub(crate) fn get_prop_miss(&mut self, pc: usize, d: u16, o: u16, ic: u32) -> Ctl {
        let r = self.get_prop_miss_impl(pc, d, o, ic);
        self.ctl(r)
    }

    #[inline(always)]
    fn get_prop_miss_impl(&mut self, pc: usize, d: u16, o: u16, ic: u32) -> JsResult<Flow> {
        let fi = self.frames.len() - 1;
        // SAFETY: `fi` is the running frame.
        let f = unsafe { self.frames.get_unchecked_mut(fi) };
        f.pc = pc;
        // SAFETY: the code outlives this call (the frame stays).
        let code: &FunctionCode = unsafe { f.code.as_ref() };
        let fb = f.base as usize;
        let rec = code.ics[ic as usize].get();
        let recv = &self.vstack[fb + o as usize];
        let mut p = if rec.kind == crate::ic::IC_LENGTH { None } else { self.ic_get(&rec, recv) };
        if p.is_none() && recv.is_object() {
            // Polymorphic site: the shared cache by (shape, key).
            let kb = code.constants[code.ic_keys[ic as usize] as usize].bits();
            if let Some(m) = self.mega_get(recv, kb) {
                p = self.ic_get(&m, recv);
                // A site whose first lookups were all served by the shared
                // cache never filled its own record: adopt the shared one
                // (a monomorphic site would otherwise take this path on
                // every execution).
                if p.is_some() && rec.kind == crate::ic::IC_NONE {
                    code.ics[ic as usize].set(m);
                }
            }
        }
        match p {
            Some(p) if p.flags & ACCESSOR == 0 => {
                let v = p.value.clone();
                self.vstack[fb + d as usize] = v;
                return Ok(Flow::Next);
            }
            Some(p) => {
                // Cached accessor: a native getter (DOM attributes,
                // `ta.length`, ...) is called directly.
                if let Some(V::Object(g)) = p.getter().map(Value::v) {
                    let r = self.call_cached_getter(g, o as usize, fi)?;
                    // The getter may have run JavaScript (the value stack
                    // may have moved).
                    let at = self.frames[fi].base as usize + d as usize;
                    self.vstack[at] = r;
                    return Ok(Flow::Next);
                }
            }
            None => {}
        }
        let op = code.ops[pc - 1];
        self.exec_in(fi, op)
    }

    /// `SetPropR` after an inline-cache miss: setters, the megamorphic
    /// cache, then the full instruction.
    #[inline(never)]
    pub(crate) fn set_prop_miss(&mut self, pc: usize, o: u16, v: u16, ic: u32) -> Ctl {
        let r = self.set_prop_miss_impl(pc, o, v, ic);
        self.ctl(r)
    }

    #[inline(always)]
    fn set_prop_miss_impl(&mut self, pc: usize, o: u16, v: u16, ic: u32) -> JsResult<Flow> {
        let fi = self.frames.len() - 1;
        // SAFETY: `fi` is the running frame.
        let f = unsafe { self.frames.get_unchecked_mut(fi) };
        f.pc = pc;
        // SAFETY: the code outlives this call (the frame stays).
        let code: &FunctionCode = unsafe { f.code.as_ref() };
        let fb = f.base as usize;
        let (os, vs) = (fb + o as usize, fb + v as usize);
        let mut rec = code.ics[ic as usize].get();
        if !self.vstack[vs].is_empty() {
            let key = match code.constants[code.ic_keys[ic as usize] as usize].v() {
                V::String(s) => s,
                _ => unreachable!(),
            };
            let site_empty = rec.kind == crate::ic::IC_NONE;
            for attempt in 0..2 {
                if attempt == 1 {
                    // Polymorphic site: the shared cache by (shape, key).
                    match self.mega_get(&self.vstack[os], key.bits() | 1) {
                        Some(m) => rec = m,
                        None => break,
                    }
                }
                if rec.kind == crate::ic::IC_SETTER {
                    if let Some(f) = self.ic_setter(&rec, &self.vstack[os]) {
                        if attempt == 1 && site_empty {
                            code.ics[ic as usize].set(rec);
                        }
                        let (recv, val) = (self.vstack[os].clone(), self.vstack[vs].clone());
                        self.call(Value::Object(f), recv, &[val])?;
                        return Ok(if self.frames.len() != fi + 1 { Flow::Reload } else { Flow::Next });
                    }
                } else if rec.kind != 0 {
                    let val = self.vstack[vs].clone();
                    let recv = self.vstack[os].clone();
                    match self.ic_set(&rec, &recv, key, val) {
                        Ok(()) => {
                            // As for reads: an empty site adopts the shared
                            // record that served it.
                            if attempt == 1 && site_empty {
                                code.ics[ic as usize].set(rec);
                            }
                            return Ok(Flow::Next);
                        }
                        Err(v) => drop(v),
                    }
                }
            }
        }
        let op = code.ops[pc - 1];
        self.exec_in(fi, op)
    }

    /// `CallR`: call-site cache hit, plain closures, bound/`call`/`apply`
    /// targets and natives without leaving the loop's frame model; the
    /// general case through [`Realm::exec_op`].
    #[inline(always)]
    pub(crate) fn call_r(&mut self, pc: usize, b: u16, argc: u16, cic: u16) -> Ctl {
        let r = self.call_r_impl(pc, b, argc, cic);
        self.ctl(r)
    }

    #[inline(always)]
    fn call_r_impl(&mut self, pc: usize, b: u16, argc: u16, cic: u16) -> JsResult<Flow> {
        let fi = self.frames.len() - 1;
        // SAFETY: `fi` is the running frame.
        let f = unsafe { self.frames.get_unchecked_mut(fi) };
        f.pc = pc;
        // SAFETY: the code outlives this call (the frame stays).
        let code: &FunctionCode = unsafe { f.code.as_ref() };
        let fbase = f.base as usize;
        if let Some(cell) = code.call_ics.get(cic as usize) {
            let rec = cell.get();
            let cb = fbase + b as usize;
            let callee = self.vstack[cb].bits();
            if rec.callee == callee && rec.epoch == self.heap.collections {
                // Same closure as last time: no lookup or checks.
                let this = &self.vstack[cb + 1];
                let this = if rec.strict {
                    Some(this.undef_if_empty())
                } else {
                    match this.v() {
                        V::Undefined | V::Null | V::Empty => Some(Value::Object(self.global)),
                        V::Object(o) => Some(Value::Object(o)),
                        _ => None,
                    }
                };
                if let Some(this) = this {
                    let scope = if rec.scope == u32::MAX { None } else { Some(ObjectId(rec.scope)) };
                    // SAFETY: the closure owning the code is alive (it is in slot `b`).
                    let code_ptr = unsafe { std::ptr::NonNull::new_unchecked(rec.code as *mut FunctionCode) };
                    self.push_closure_frame(ObjectId(callee as u32), code_ptr, scope, ArgSrc::Stack(cb + 2), argc as usize, this, Value::Undefined, b as u32, false, false)?;
                    return Ok(Flow::Reload);
                }
            }
            if self.try_fast_call(fi, b as usize, argc as usize)? {
                self.fill_call_ic(cell, callee);
                return Ok(Flow::Reload);
            }
        } else if self.try_fast_call(fi, b as usize, argc as usize)? {
            return Ok(Flow::Reload);
        }
        if self.try_call_indirect(fi, b as usize, argc as usize)? {
            return Ok(Flow::Reload);
        }
        if self.try_native_call(fi, b as usize, argc as usize)? {
            return Ok(if self.frames.len() != fi + 1 { Flow::Reload } else { Flow::Next });
        }
        let op = code.ops[pc - 1];
        self.exec_in(fi, op)
    }

    /// `NewR`: plain function constructors, else the general path.
    #[inline(never)]
    pub(crate) fn new_r(&mut self, pc: usize, b: u16, argc: u16) -> Ctl {
        let r = self.new_r_impl(pc, b, argc);
        self.ctl(r)
    }

    #[inline(always)]
    fn new_r_impl(&mut self, pc: usize, b: u16, argc: u16) -> JsResult<Flow> {
        let fi = self.frames.len() - 1;
        // SAFETY: `fi` is the running frame.
        unsafe { self.frames.get_unchecked_mut(fi) }.pc = pc;
        if self.try_fast_construct(fi, b as usize, argc as usize)? {
            return Ok(Flow::Reload);
        }
        // SAFETY: as above.
        let op = unsafe { *self.frames.get_unchecked(fi).code().ops.get_unchecked(pc - 1) };
        self.exec_in(fi, op)
    }

    /// `ReturnR` of value `v` (already taken out of its slot): pops the
    /// frame and hands the value to the caller's slot or out of the loop.
    #[inline(always)]
    pub(crate) fn return_r(&mut self, v: Value) -> Ctl {
        let r = self.return_r_impl(v);
        self.ctl(r)
    }

    #[inline(always)]
    fn return_r_impl(&mut self, v: Value) -> JsResult<Flow> {
        let mut v = match v {
            Value::Empty => Value::Undefined,
            v => v,
        };
        let fi = self.frames.len() - 1;
        // SAFETY: `fi` is the running frame.
        if unsafe { self.frames.get_unchecked(fi) }.is_construct && !v.is_object() {
            if let Some(e) = self.construct_return_error(&v) {
                return Err(e);
            }
            v = unsafe { self.frames.get_unchecked(fi) }.this.clone();
        }
        let f = unsafe { self.frames.get_unchecked(fi) };
        if !f.entry && fi > 0 {
            // Plain return to the calling frame: drop the frame where it
            // is (no move out of the vector).
            let (ret, bottom) = (f.ret_slot as usize, f.bottom());
            // SAFETY: `fi` is the last frame; it is dropped exactly once
            // after the length excludes it.
            unsafe {
                self.frames.set_len(fi);
                std::ptr::drop_in_place(self.frames.as_mut_ptr().add(fi));
            }
            self.vstack.truncate(bottom);
            // SAFETY: `fi - 1` is the caller (fi > 0).
            let at = unsafe { self.frames.get_unchecked(fi - 1) }.base as usize + ret;
            self.vstack[at] = v;
            return Ok(Flow::Reload);
        }
        let f = self.pop_frame();
        if f.entry {
            if let Some(g) = f.generator() {
                if let ObjectKind::Generator(gd) = &mut self.obj_mut(g).kind {
                    gd.state = GeneratorState::Completed;
                    gd.frame = None;
                }
            }
            return Ok(Flow::Exit(Exit::Return(v)));
        }
        let ret = f.ret_slot as usize;
        match self.frames.last() {
            Some(caller) => {
                let at = caller.base as usize + ret;
                self.vstack[at] = v;
            }
            None => stack_underflow(),
        }
        Ok(Flow::Reload)
    }

    fn inc_value(&mut self, v: Value, d: f64) -> JsResult<Value> {
        Ok(match (self.to_numeric(v)?).v() {
            V::Number(n) => Value::Number(n + d),
            V::BigInt(b) => Value::BigInt(Rc::new(b.add(&BigInt::from_i128(d as i128)))),
            _ => unreachable!(),
        })
    }

    /// The current value of the parameter binding `loc` of frame `fi`.
    fn read_param(&self, fi: usize, loc: VarLoc) -> Value {
        let f = &self.frames[fi];
        match loc {
            VarLoc::Reg(r) => self.vstack[f.base as usize + r as usize].clone(),
            VarLoc::Slot(s) => {
                let scope = self.scope_at(f.scope, f.scope_depth as u16);
                match &self.heap.get(scope).kind {
                    ObjectKind::Scope(sd) => sd.slots.get(s as usize).cloned().unwrap_or(Value::Undefined),
                    _ => Value::Undefined,
                }
            }
        }
    }

    /// `arguments[key]` read of an `args_direct` frame.
    fn args_get(&mut self, fi: usize, key: Value) -> JsResult<Value> {
        let f = &self.frames[fi];
        let (bottom, argc) = (f.bottom(), f.argc as usize);
        if let V::Number(n) = key.v() {
            let i = n as usize;
            if i as f64 == n && i < argc {
                // A sloppy function's argument reads its parameter (the
                // binding may have been assigned since).
                if let Some(Some(loc)) = f.code().arg_map.as_ref().and_then(|m| m.get(i)) {
                    return Ok(self.read_param(fi, *loc));
                }
                return Ok(self.vstack[bottom + i].clone());
            }
        }
        // Anything else: an equivalent (unobservable) arguments object.
        let mut args = self.vstack[bottom..bottom + argc].to_vec();
        if let Some(map) = self.frames[fi].code().arg_map.as_ref() {
            for (i, a) in args.iter_mut().enumerate() {
                if let Some(Some(loc)) = map.get(i) {
                    *a = self.read_param(fi, *loc);
                }
            }
        }
        let f = &self.frames[fi];
        let callee = if f.code().flags.is_strict || !f.code().flags.has_simple_params { None } else { f.func };
        let ao = self.create_arguments_object(&args, callee);
        let key = self.to_property_key(key)?;
        self.get(ao, key)
    }

    /// Execute one (non-hot) instruction of frame `fi`, whose `pc` already
    /// points past it. The hot instructions live in [`Realm::run_inner`].
    #[inline(never)]
    #[allow(unused_assignments)]
    fn exec_op(&mut self, fi: usize, op: Op) -> JsResult<Flow> {
        {
            macro_rules! frame {
                () => {
                    self.frames[fi]
                };
            }
            // Stack instructions find their operands below the static
            // depth; `sp` and `fb` are value-stack indices.
            let fb: usize = self.frames[fi].base as usize;
            let mut sp: usize = {
                let f = &self.frames[fi];
                fb + f.code().nregs as usize + f.code().depths[f.pc - 1] as usize
            };
            macro_rules! pop {
                () => {{
                    sp -= 1;
                    std::mem::replace(&mut self.vstack[sp], Value::Undefined)
                }};
            }
            macro_rules! push {
                ($v:expr) => {{
                    let v = $v;
                    self.vstack[sp] = v;
                    sp += 1;
                }};
            }
            macro_rules! peek {
                () => {
                    &self.vstack[sp - 1]
                };
            }
            macro_rules! sl {
                ($i:expr) => {
                    self.vstack[fb + $i as usize]
                };
            }
            macro_rules! rd {
                ($i:expr) => {
                    sl!($i).undef_if_empty()
                };
            }
            macro_rules! set {
                ($i:expr, $v:expr) => {{
                    let v = $v;
                    sl!($i) = v;
                }};
            }
            macro_rules! jmp {
                ($t:expr) => {
                    frame!().pc = $t as usize
                };
            }
            macro_rules! ic_key {
                ($ic:expr) => {{
                    let k = frame!().code().ic_keys[$ic as usize];
                    match (frame!().code().constants[k as usize]).v() {
                        V::String(s) => PropertyKey::String(s.clone()),
                        _ => unreachable!(),
                    }
                }};
            }
            macro_rules! constant {
                ($i:expr) => {
                    frame!().code().constants[$i as usize].clone()
                };
            }
            macro_rules! const_key {
                ($i:expr) => {
                    match (frame!().code().constants[$i as usize]).v() {
                        V::String(s) => PropertyKey::String(s.clone()),
                        _ => unreachable!(),
                    }
                };
            }
            match op {
                // ---- register forms (complete semantics; the interpreter
                // loop only has their fast paths)
                Op::Mov(d, s) => {
                    let v = rd!(s);
                    set!(d, v);
                }
                Op::MovRaw(d, s) => {
                    let v = sl!(s).clone();
                    set!(d, v);
                }
                Op::LoadK(d, c) => {
                    let v = constant!(c);
                    set!(d, v);
                }
                Op::MovChecked(d, s) => {
                    let v = sl!(s).clone();
                    if v.is_empty() {
                        let name = self.reg_name(fi, s);
                        return Err(self.new_error("ReferenceError", &format!("Cannot access '{}' before initialization", name)));
                    }
                    set!(d, v);
                }
                Op::CheckInit(r) => {
                    if sl!(r).is_empty() {
                        let name = self.reg_name(fi, r);
                        return Err(self.new_error("ReferenceError", &format!("Cannot access '{}' before initialization", name)));
                    }
                }
                Op::AddRR(d, a, b) | Op::SubRR(d, a, b) | Op::MulRR(d, a, b) | Op::DivRR(d, a, b) | Op::ModRR(d, a, b) => {
                    let bop = match op {
                        Op::AddRR(..) => Op::Add,
                        Op::SubRR(..) => Op::Sub,
                        Op::MulRR(..) => Op::Mul,
                        Op::DivRR(..) => Op::Div,
                        _ => Op::Mod,
                    };
                    let (x, y) = (rd!(a), rd!(b));
                    let r = self.binop_values(bop, x, y)?;
                    set!(d, r);
                }
                Op::AddRK(d, a, c) | Op::SubRK(d, a, c) | Op::MulRK(d, a, c) | Op::DivRK(d, a, c) | Op::ModRK(d, a, c) => {
                    let bop = match op {
                        Op::AddRK(..) => Op::Add,
                        Op::SubRK(..) => Op::Sub,
                        Op::MulRK(..) => Op::Mul,
                        Op::DivRK(..) => Op::Div,
                        _ => Op::Mod,
                    };
                    let (x, y) = (rd!(a), constant!(c));
                    let r = self.binop_values(bop, x, y)?;
                    set!(d, r);
                }
                Op::BinRR(k, d, a, b) => {
                    let (x, y) = (rd!(a), rd!(b));
                    let r = self.binop_values(crate::bytecode::BIN_OPS[k as usize], x, y)?;
                    set!(d, r);
                }
                Op::BinRK(k, d, a, c) => {
                    let (x, y) = (rd!(a), constant!(c));
                    let r = self.binop_values(crate::bytecode::BIN_OPS[k as usize], x, y)?;
                    set!(d, r);
                }
                Op::CmpRR(k, d, a, b) => {
                    let (x, y) = (rd!(a), rd!(b));
                    let r = self.binop_values(crate::bytecode::CMP_OPS[k as usize], x, y)?;
                    set!(d, r);
                }
                Op::CmpRK(k, d, a, c) => {
                    let (x, y) = (rd!(a), constant!(c));
                    let r = self.binop_values(crate::bytecode::CMP_OPS[k as usize], x, y)?;
                    set!(d, r);
                }
                Op::JumpUnlessLtRR(a, b, t) | Op::JumpUnlessLtEqRR(a, b, t) | Op::JumpUnlessGtRR(a, b, t) | Op::JumpUnlessGtEqRR(a, b, t) => {
                    let cmp = match op {
                        Op::JumpUnlessLtRR(..) => Op::Lt,
                        Op::JumpUnlessLtEqRR(..) => Op::LtEq,
                        Op::JumpUnlessGtRR(..) => Op::Gt,
                        _ => Op::GtEq,
                    };
                    let (x, y) = (rd!(a), rd!(b));
                    if !self.binop_values(cmp, x, y)?.truthy() {
                        jmp!(t);
                    }
                }
                Op::JumpUnlessLtRK(a, c, t) | Op::JumpUnlessLtEqRK(a, c, t) | Op::JumpUnlessGtRK(a, c, t) | Op::JumpUnlessGtEqRK(a, c, t) => {
                    let cmp = match op {
                        Op::JumpUnlessLtRK(..) => Op::Lt,
                        Op::JumpUnlessLtEqRK(..) => Op::LtEq,
                        Op::JumpUnlessGtRK(..) => Op::Gt,
                        _ => Op::GtEq,
                    };
                    let (x, y) = (rd!(a), constant!(c));
                    if !self.binop_values(cmp, x, y)?.truthy() {
                        jmp!(t);
                    }
                }
                Op::JumpIfLtRR(a, b, t) | Op::JumpIfLtEqRR(a, b, t) | Op::JumpIfGtRR(a, b, t) | Op::JumpIfGtEqRR(a, b, t) => {
                    let cmp = match op {
                        Op::JumpIfLtRR(..) => Op::Lt,
                        Op::JumpIfLtEqRR(..) => Op::LtEq,
                        Op::JumpIfGtRR(..) => Op::Gt,
                        _ => Op::GtEq,
                    };
                    let (x, y) = (rd!(a), rd!(b));
                    if self.binop_values(cmp, x, y)?.truthy() {
                        jmp!(t);
                    }
                }
                Op::JumpIfLtRK(a, c, t) | Op::JumpIfLtEqRK(a, c, t) | Op::JumpIfGtRK(a, c, t) | Op::JumpIfGtEqRK(a, c, t) => {
                    let cmp = match op {
                        Op::JumpIfLtRK(..) => Op::Lt,
                        Op::JumpIfLtEqRK(..) => Op::LtEq,
                        Op::JumpIfGtRK(..) => Op::Gt,
                        _ => Op::GtEq,
                    };
                    let (x, y) = (rd!(a), constant!(c));
                    if self.binop_values(cmp, x, y)?.truthy() {
                        jmp!(t);
                    }
                }
                Op::JumpIfStrictEqRR(a, b, t) | Op::JumpIfStrictNotEqRR(a, b, t) => {
                    let eq = self.strict_equals(&rd!(a), &rd!(b));
                    if eq == matches!(op, Op::JumpIfStrictEqRR(..)) {
                        jmp!(t);
                    }
                }
                Op::JumpIfStrictEqRK(a, c, t) | Op::JumpIfStrictNotEqRK(a, c, t) => {
                    let eq = self.strict_equals(&rd!(a), &constant!(c));
                    if eq == matches!(op, Op::JumpIfStrictEqRK(..)) {
                        jmp!(t);
                    }
                }
                Op::JumpIfTrueR(s, t) => {
                    if self.to_boolean(&sl!(s)) {
                        jmp!(t);
                    }
                }
                Op::JumpIfFalseR(s, t) => {
                    if !self.to_boolean(&sl!(s)) {
                        jmp!(t);
                    }
                }
                Op::JumpIfNullishR(s, t) => {
                    if matches!(sl!(s), Value::Undefined | Value::Null | Value::Empty) {
                        jmp!(t);
                    }
                }
                Op::JumpIfNotNullishR(s, t) => {
                    if !matches!(sl!(s), Value::Undefined | Value::Null | Value::Empty) {
                        jmp!(t);
                    }
                }
                Op::JumpIfUndefinedR(s, t) => {
                    if matches!(sl!(s), Value::Undefined | Value::Empty) {
                        jmp!(t);
                    }
                }
                Op::JumpIfNotUndefinedR(s, t) => {
                    if !matches!(sl!(s), Value::Undefined | Value::Empty) {
                        jmp!(t);
                    }
                }
                Op::IncJumpIfLtRR(r, ..) | Op::IncJumpIfLtRK(r, ..) => {
                    // Out of line: the increment; the loop test follows.
                    return self.exec_op(fi, Op::IncR(r, r));
                }
                Op::NotR(d, s) | Op::IncR(d, s) | Op::DecR(d, s) | Op::ToNumericR(d, s) => {
                    let uop = match op {
                        Op::NotR(..) => Op::Not,
                        Op::IncR(..) => Op::Inc,
                        Op::DecR(..) => Op::Dec,
                        _ => Op::ToNumeric,
                    };
                    let v = rd!(s);
                    let r = self.unary_value(uop, v)?;
                    set!(d, r);
                }
                Op::UnR(k, d, s) => {
                    let v = rd!(s);
                    let r = self.unary_value(crate::bytecode::UN_OPS[k as usize], v)?;
                    set!(d, r);
                }
                Op::LoadThis(d) => {
                    let t = frame!().this.clone();
                    set!(d, if t.is_empty() { Value::Undefined } else { t });
                }
                Op::GetPropR(d, o, ic) => {
                    let obj = rd!(o);
                    let key = ic_key!(ic);
                    let v = self.get(obj.clone(), key.clone())?;
                    let rec = self.ic_fill_get(&obj, &key);
                    frame!().code().ics[ic as usize].set(rec);
                    self.mega_put(&obj, &key, rec);
                    set!(d, v);
                }
                Op::SetPropR(o, v, ic) => {
                    let obj = rd!(o);
                    let val = rd!(v);
                    let key = ic_key!(ic);
                    let before = self.ic_before_set(&obj, &key);
                    let ok = self.set(obj.clone(), key.clone(), val)?;
                    if !ok && frame!().code().flags.is_strict {
                        let k = self.key_display(&key);
                        return self.type_error(&format!("Cannot assign to read only property '{}' of object", k));
                    }
                    let rec = if ok { self.ic_fill_set(&obj, &key, before, false) } else { Default::default() };
                    if ok {
                        self.mega_put_set(&obj, &key, before, rec);
                    }
                    frame!().code().ics[ic as usize].set(rec);
                }
                Op::DefinePropR(o, v, ic) => {
                    let objv = rd!(o);
                    let val = rd!(v);
                    let obj = objv.as_object().unwrap();
                    let key = ic_key!(ic);
                    let before = self.ic_before_set(&objv, &key);
                    self.define_own(obj, key.clone(), Property::data(val, ALL))?;
                    let rec = self.ic_fill_set(&objv, &key, before, true);
                    frame!().code().ics[ic as usize].set(rec);
                }
                Op::GetElemR(d, o, k) => {
                    let obj = rd!(o);
                    let key = rd!(k);
                    let v = self.get_elem(obj, key)?;
                    set!(d, v);
                }
                Op::SetElemR(o, k, v) => {
                    let obj = rd!(o);
                    let key = rd!(k);
                    let val = rd!(v);
                    let key = self.to_property_key(key)?;
                    let ok = self.set(obj, key.clone(), val)?;
                    if !ok && frame!().code().flags.is_strict {
                        let k = self.key_display(&key);
                        return self.type_error(&format!("Cannot assign to read only property '{}' of object", k));
                    }
                }
                Op::GetGlobalR(d, ic) => {
                    let key = ic_key!(ic);
                    let v = self.get_global_binding(&key)?;
                    let rec = self.ic_fill_global(key.as_str().unwrap());
                    frame!().code().ics[ic as usize].set(rec);
                    set!(d, v);
                }
                Op::SetGlobalR(s, ic) => {
                    let v = rd!(s);
                    let key = ic_key!(ic);
                    let strict = frame!().code().flags.is_strict;
                    self.set_global_binding(&key, v, strict)?;
                    let rec = self.ic_fill_global(key.as_str().unwrap());
                    frame!().code().ics[ic as usize].set(rec);
                }
                Op::GetLocalR(d, dp, s) => {
                    let scope = frame!().scope;
                    let v = self.slot_get(scope, dp, s);
                    set!(d, if v.is_empty() { Value::Undefined } else { v });
                }
                Op::SetLocalR(v, dp, s) => {
                    let val = rd!(v);
                    let scope = frame!().scope;
                    self.slot_set(scope, dp, s, val);
                }
                Op::StoreCompletionR(s) => {
                    let v = sl!(s).clone();
                    frame!().extra_mut().completion = v;
                }
                Op::GetReg(r) => {
                    let v = sl!(r).clone();
                    push!(if v.is_empty() { Value::Undefined } else { v });
                }
                Op::GetRegChecked(r) => {
                    let v = sl!(r).clone();
                    if v.is_empty() {
                        let name = self.reg_name(fi, r);
                        return Err(self.new_error("ReferenceError", &format!("Cannot access '{}' before initialization", name)));
                    }
                    push!(v);
                }
                Op::SetReg(r) => {
                    let v = pop!();
                    sl!(r) = v;
                }
                Op::SetRegChecked(r) => {
                    let v = pop!();
                    if sl!(r).is_empty() {
                        let name = self.reg_name(fi, r);
                        return Err(self.new_error("ReferenceError", &format!("Cannot access '{}' before initialization", name)));
                    }
                    sl!(r) = v;
                }
                Op::Const(i) => push!(constant!(i)),
                Op::Undefined => push!(Value::Undefined),
                Op::Null => push!(Value::Null),
                Op::True => push!(Value::Bool(true)),
                Op::False => push!(Value::Bool(false)),
                Op::Empty => push!(Value::Empty),
                Op::Pop => {
                    let _ = pop!();
                }
                Op::Dup => {
                    let v = peek!().clone();
                    push!(v);
                }
                Op::Dup2 => {
                    let a = self.vstack[sp - 2].clone();
                    let b = self.vstack[sp - 1].clone();
                    push!(a);
                    push!(b);
                }
                Op::Swap => {
                    self.vstack.swap(sp - 1, sp - 2);
                }
                Op::Rot3 => {
                    // a b c -> b c a
                    self.vstack[sp - 3..sp].rotate_left(1);
                }
                Op::Rot4 => {
                    self.vstack[sp - 4..sp].rotate_left(1);
                }
                Op::GetLocal(d, s) => {
                    let scope = frame!().scope;
                    let v = self.slot_get(scope, d, s);
                    push!(if v.is_empty() { Value::Undefined } else { v });
                }
                Op::GetLocalChecked(d, s) => {
                    let scope = frame!().scope;
                    let v = self.slot_get(scope, d, s);
                    if v.is_empty() {
                        let name = self.slot_name(fi, d, s);
                        return Err(self.new_error("ReferenceError", &format!("Cannot access '{}' before initialization", name)));
                    }
                    push!(v);
                }
                Op::SetLocal(d, s) | Op::InitLocal(d, s) => {
                    let v = pop!();
                    let scope = frame!().scope;
                    self.slot_set(scope, d, s, v);
                }
                Op::SetLocalChecked(d, s) => {
                    let v = pop!();
                    let scope = frame!().scope;
                    if self.slot_get(scope, d, s).is_empty() {
                        let name = self.slot_name(fi, d, s);
                        return Err(self.new_error("ReferenceError", &format!("Cannot access '{}' before initialization", name)));
                    }
                    self.slot_set(scope, d, s, v);
                }
                Op::GetImport(d, s) => {
                    let scope = frame!().scope;
                    let v = self.slot_get(scope, d, s);
                    let v = self.read_import_slot(v)?;
                    push!(v);
                }
                Op::GetGlobal(i, ic) => {
                    let key = const_key!(i);
                    let v = self.get_global_binding(&key)?;
                    let rec = self.ic_fill_global(key.as_str().unwrap());
                    frame!().code().ics[ic as usize].set(rec);
                    push!(v);
                }
                Op::SetGlobal(i, ic) => {
                    let v = pop!();
                    let key = const_key!(i);
                    let strict = frame!().code().flags.is_strict;
                    self.set_global_binding(&key, v, strict)?;
                    let rec = self.ic_fill_global(key.as_str().unwrap());
                    frame!().code().ics[ic as usize].set(rec);
                }
                Op::TypeofGlobal(i) => {
                    let key = const_key!(i);
                    let name = key.as_str().unwrap().clone();
                    let v = if let Some((v, _)) = self.global_lex.get(&name) {
                        Some(v.clone())
                    } else if self.has_property(self.global, &key)? {
                        Some(self.get_from_object(self.global, &key, Value::Object(self.global))?)
                    } else {
                        None
                    };
                    let t = match v {
                        Some(v) if !v.is_empty() => self.type_of(&v),
                        _ => "undefined",
                    };
                    push!(Value::str(t));
                }
                Op::GlobalDeclCheck => {
                    let code = frame!().code;
                    // SAFETY: the running frame's code outlives this instruction.
                    if let Some(d) = unsafe { code.as_ref() }.global_decls.as_deref() {
                        self.global_decl_check(d)?;
                    }
                }
                Op::DeclareGlobalVar(i) => {
                    let key = const_key!(i);
                    let g = self.global;
                    if !self.has_own_property(g, &key) {
                        self.define_property(g, key, Value::Undefined, WRITABLE | ENUMERABLE);
                    }
                }
                Op::DeclareGlobalFunction(i) => {
                    let f = pop!();
                    let key = const_key!(i);
                    let g = self.global;
                    let flags = self.get_own_property(g, &key).map(|p| if p.configurable() { WRITABLE | ENUMERABLE } else { p.flags }).unwrap_or(WRITABLE | ENUMERABLE);
                    self.define_property(g, key, f, flags);
                }
                Op::DeclareGlobalLex(i, is_const) => {
                    let name = const_key!(i).as_str().unwrap().clone();
                    self.global_lex.insert(name, (Value::Empty, is_const));
                }
                Op::InitGlobalLex(i) => {
                    let v = pop!();
                    let name = const_key!(i).as_str().unwrap().clone();
                    let c = self.global_lex.get(&name).map(|(_, c)| *c).unwrap_or(false);
                    self.global_lex.insert(name, (v, c));
                }
                Op::PushScope(n, names_idx) => {
                    let parent = frame!().scope;
                    let names = frame!().code().block_names.get(names_idx as usize).cloned();
                    let s = self.alloc(Object::new(None, ObjectKind::Scope(Box::new(ScopeData { slots: vec![Value::Undefined; n as usize], parent: Some(parent), names }))));
                    frame!().scope = s;
                    frame!().scope_depth += 1;
                }
                Op::PopScope => {
                    let cur = frame!().scope;
                    let parent = match &self.heap.get(cur).kind {
                        ObjectKind::Scope(s) => s.parent.expect("scope underflow"),
                        _ => unreachable!(),
                    };
                    frame!().scope = parent;
                    frame!().scope_depth -= 1;
                }
                Op::CopyScope => {
                    let cur = frame!().scope;
                    let (slots, parent, names) = match &self.heap.get(cur).kind {
                        ObjectKind::Scope(s) => (s.slots.clone(), s.parent, s.names.clone()),
                        _ => unreachable!(),
                    };
                    let s = self.alloc(Object::new(None, ObjectKind::Scope(Box::new(ScopeData { slots, parent, names }))));
                    frame!().scope = s;
                }
                Op::GetProp(i, ic) => {
                    let obj = pop!();
                    let key = const_key!(i);
                    let v = self.get(obj.clone(), key.clone())?;
                    let rec = self.ic_fill_get(&obj, &key);
                    frame!().code().ics[ic as usize].set(rec);
                    push!(v);
                }
                Op::GetPropKeep(i, ic) => {
                    let obj = peek!().clone();
                    let key = const_key!(i);
                    let v = self.get(obj.clone(), key.clone())?;
                    let rec = self.ic_fill_get(&obj, &key);
                    frame!().code().ics[ic as usize].set(rec);
                    push!(v);
                }
                Op::GetMethod(i, ic) => {
                    let obj = peek!().clone();
                    let key = const_key!(i);
                    let v = self.get(obj.clone(), key.clone())?;
                    let rec = self.ic_fill_get(&obj, &key);
                    frame!().code().ics[ic as usize].set(rec);
                    let o = pop!();
                    push!(v);
                    push!(o);
                }
                Op::SetProp(i, ic) => {
                    let v = pop!();
                    let obj = pop!();
                    let key = const_key!(i);
                    let before = self.ic_before_set(&obj, &key);
                    let ok = self.set(obj.clone(), key.clone(), v.clone())?;
                    if !ok && frame!().code().flags.is_strict {
                        let k = self.key_display(&key);
                        return self.type_error(&format!("Cannot assign to read only property '{}' of object", k));
                    }
                    let rec = if ok { self.ic_fill_set(&obj, &key, before, false) } else { Default::default() };
                    frame!().code().ics[ic as usize].set(rec);
                    push!(v);
                }
                Op::GetElem => {
                    let key = pop!();
                    let obj = pop!();
                    let v = self.get_elem(obj, key)?;
                    push!(v);
                }
                Op::GetElemKeep => {
                    let key = self.vstack[sp - 1].clone();
                    let obj = self.vstack[sp - 2].clone();
                    let v = self.get_elem(obj, key)?;
                    push!(v);
                }
                Op::SetElem => {
                    let v = pop!();
                    let key = pop!();
                    let obj = pop!();
                    let key = self.to_property_key(key)?;
                    let ok = self.set(obj, key.clone(), v.clone())?;
                    if !ok && frame!().code().flags.is_strict {
                        let k = self.key_display(&key);
                        return self.type_error(&format!("Cannot assign to read only property '{}' of object", k));
                    }
                    push!(v);
                }
                Op::DeleteProp(i) => {
                    let obj = pop!();
                    let key = const_key!(i);
                    let r = self.delete_elem(obj, key, frame!().code().flags.is_strict)?;
                    push!(Value::Bool(r));
                }
                Op::DeleteElem => {
                    let key = pop!();
                    let obj = pop!();
                    let key = self.to_property_key(key)?;
                    let r = self.delete_elem(obj, key, frame!().code().flags.is_strict)?;
                    push!(Value::Bool(r));
                }
                Op::DefineProp(i, ic) => {
                    let v = pop!();
                    let objv = peek!().clone();
                    let obj = objv.as_object().unwrap();
                    let key = const_key!(i);
                    let before = self.ic_before_set(&objv, &key);
                    self.define_own(obj, key.clone(), Property::data(v, ALL))?;
                    let rec = self.ic_fill_set(&objv, &key, before, true);
                    frame!().code().ics[ic as usize].set(rec);
                }
                Op::DefineElem => {
                    let v = pop!();
                    let key = pop!();
                    let obj = peek!().as_object().unwrap();
                    let key = self.to_property_key(key)?;
                    // Anonymous functions take the computed key as their name.
                    if let V::Object(fid) = v.v() {
                        let anonymous = match &self.obj(fid).kind {
                            ObjectKind::Function(c) => c.code.name.is_empty(),
                            _ => false,
                        };
                        if anonymous {
                            let has_name = self.obj(fid).props.get(&PropertyKey::str("name")).map(|p| matches!(p.value.v(), V::String(s) if !s.is_empty())).unwrap_or(false);
                            if !has_name {
                                let name = match &key {
                                    PropertyKey::String(s) => Value::String(s.clone()),
                                    PropertyKey::Symbol(sym) => Value::str(&format!("[{}]", self.symbol_description(*sym))),
                                };
                                self.define_property(fid, "name", name, CONFIGURABLE);
                            }
                        }
                    }
                    self.define_own(obj, key, Property::data(v, ALL))?;
                }
                Op::DefineMethod(flag) => {
                    let f = pop!();
                    let key = pop!();
                    let obj = peek!().as_object().unwrap();
                    let key = self.to_property_key(key)?;
                    self.set_function_name(&f, &key, "");
                    if let V::Object(fid) = f.v() {
                        if let ObjectKind::Function(c) = &mut self.obj_mut(fid).kind {
                            c.home_object = Some(obj);
                        }
                    }
                    if matches!(key, PropertyKey::Symbol(sym) if self.is_private_symbol(sym)) {
                        // Private method: a non-writable private element.
                        self.private_define(&Value::Object(obj), &key.to_value(), Property::data(f, 0))?;
                    } else {
                        let flags = if flag == 1 { ALL } else { HIDDEN };
                        self.define_own(obj, key, Property::data(f, flags))?;
                    }
                }
                Op::DefineGetter(flag) | Op::DefineSetter(flag) => {
                    let f = pop!();
                    let key = pop!();
                    let obj = peek!().as_object().unwrap();
                    let key = self.to_property_key(key)?;
                    let is_getter = matches!(op, Op::DefineGetter(_));
                    self.set_function_name(&f, &key, if is_getter { "get" } else { "set" });
                    if let V::Object(fid) = f.v() {
                        if let ObjectKind::Function(c) = &mut self.obj_mut(fid).kind {
                            c.home_object = Some(obj);
                        }
                    }
                    let existing = self.get_own_property(obj, &key);
                    let (mut g, mut s) = match &existing {
                        Some(p) if p.is_accessor() => (p.getter_cloned(), p.setter_cloned()),
                        _ => (None, None),
                    };
                    if is_getter {
                        g = Some(f);
                    } else {
                        s = Some(f);
                    }
                    let private = matches!(key, PropertyKey::Symbol(sym) if self.is_private_symbol(sym));
                    let flags = if private {
                        0
                    } else if flag == 1 {
                        ENUMERABLE | CONFIGURABLE
                    } else {
                        CONFIGURABLE
                    };
                    self.obj_mut(obj).props.insert(key, Property::accessor(g, s, flags));
                }
                Op::CopyDataProps => {
                    let src = pop!();
                    let obj = peek!().as_object().unwrap();
                    self.copy_data_properties(obj, src, &[])?;
                }
                Op::CopyDataPropsExcluding => {
                    let excluded = pop!();
                    let src = pop!();
                    let obj = peek!().as_object().unwrap();
                    let ex = self.create_list_from_array_like(excluded)?;
                    let mut keys = Vec::new();
                    for e in ex {
                        keys.push(self.to_property_key(e)?);
                    }
                    self.copy_data_properties(obj, src, &keys)?;
                }
                Op::SetProtoLiteral => {
                    let v = pop!();
                    let obj = peek!().as_object().unwrap();
                    match v.v() {
                        V::Object(p) => self.obj_mut(obj).set_proto(Some(p)),
                        V::Null => self.obj_mut(obj).set_proto(None),
                        _ => {}
                    }
                }
                Op::SetHome(n) => {
                    let f = self.vstack[sp - 1].clone();
                    let home = self.vstack[sp - 1 - n as usize].as_object();
                    if let (V::Object(fid), Some(h)) = (f.v(), home) {
                        if let ObjectKind::Function(c) = &mut self.obj_mut(fid).kind {
                            c.home_object = Some(h);
                        }
                    }
                }
                Op::SuperBase => {
                    let v = self.super_base(fi)?;
                    push!(v);
                }
                Op::GetSuperProp(i) => {
                    let base = pop!();
                    let this = pop!();
                    let base = self.super_base_object(&base, Some(&const_key!(i)))?;
                    let v = self.super_get(base, const_key!(i), this)?;
                    push!(v);
                }
                Op::GetSuperPropKeep(i) => {
                    let base = self.vstack[sp - 1].clone();
                    let this = self.vstack[sp - 2].clone();
                    let base = self.super_base_object(&base, Some(&const_key!(i)))?;
                    let v = self.super_get(base, const_key!(i), this)?;
                    push!(v);
                }
                Op::GetSuperElem => {
                    let base = pop!();
                    let key = pop!();
                    let this = pop!();
                    let base = self.super_base_object(&base, None)?;
                    let key = self.to_property_key(key)?;
                    let v = self.super_get(base, key, this)?;
                    push!(v);
                }
                Op::GetSuperElemKeep => {
                    let base = self.vstack[sp - 1].clone();
                    let key = self.vstack[sp - 2].clone();
                    let this = self.vstack[sp - 3].clone();
                    let base = self.super_base_object(&base, None)?;
                    let key = self.to_property_key(key)?;
                    self.vstack[sp - 2] = key.to_value();
                    let v = self.super_get(base, key, this)?;
                    push!(v);
                }
                Op::SetSuperProp(i) => {
                    let v = pop!();
                    let base = pop!();
                    let this = pop!();
                    let base = self.super_base_object(&base, Some(&const_key!(i)))?;
                    self.super_set(fi, base, const_key!(i), v.clone(), this)?;
                    push!(v);
                }
                Op::SetSuperElem => {
                    let v = pop!();
                    let base = pop!();
                    let key = pop!();
                    let this = pop!();
                    let base = self.super_base_object(&base, None)?;
                    let key = self.to_property_key(key)?;
                    self.super_set(fi, base, key, v.clone(), this)?;
                    push!(v);
                }
                Op::Call(argc, desc) | Op::CallEval(argc, desc) | Op::CallR(_, argc, _, desc) => {
                    if let Op::CallR(b, ..) = op {
                        sp = fb + b as usize + argc as usize + 2;
                    }
                    let base = sp - argc as usize - 2;
                    let args = self.take_args(base + 2, sp);
                    sp = base + 2;
                    let this = pop!();
                    let f = pop!();
                    if desc != crate::bytecode::NO_DESC && !self.is_callable(&f) {
                        let desc = constant!(desc);
                        let desc = desc.as_str().map(|s| s.to_rust_string()).unwrap_or_default();
                        return self.type_error(&format!("{} is not a function", desc));
                    }
                    if let Op::CallEval(..) = op {
                        if f.as_object() == Some(self.intrinsics.eval_fn) {
                            let site = std::mem::replace(&mut self.eval_site, u32::MAX);
                            let site = frame!().code().eval_sites.get(site as usize).cloned();
                            let strict = frame!().code().flags.is_strict;
                            let r = self.direct_eval(fi, args.first().cloned().unwrap_or(Value::Undefined), strict, site);
                            self.give_args(args);
                            let v = r?;
                            push!(v);
                            return Ok(Flow::Next);
                        }
                    }
                    let r = self.do_call(f, this, &args, base - fb);
                    self.give_args(args);
                    r?;
                }
                Op::CallSpread => {
                    let arr = pop!();
                    let this = pop!();
                    let f = pop!();
                    let args = self.create_list_from_array_like(arr)?;
                    self.do_call(f, this, &args, sp - fb)?;
                }
                Op::New(argc) | Op::NewR(_, argc) => {
                    if let Op::NewR(b, _) = op {
                        sp = fb + b as usize + argc as usize + 1;
                    }
                    let base = sp - argc as usize - 1;
                    let args = self.take_args(base + 1, sp);
                    sp = base + 1;
                    let f = pop!();
                    let r = self.do_construct(f, &args, base - fb);
                    self.give_args(args);
                    r?;
                }
                Op::NewSpread => {
                    let arr = pop!();
                    let f = pop!();
                    let args = self.create_list_from_array_like(arr)?;
                    self.do_construct(f, &args, sp - fb)?;
                }
                Op::SuperCall(argc) => {
                    let base = sp - argc as usize;
                    let args: Vec<Value> = (base..sp).map(|i| std::mem::replace(&mut self.vstack[i], Value::Undefined)).collect();
                    sp = base;
                    let v = self.super_call(fi, args)?;
                    push!(v);
                }
                Op::SuperCallSpread => {
                    let arr = pop!();
                    let args = self.create_list_from_array_like(arr)?;
                    let v = self.super_call(fi, args)?;
                    push!(v);
                }
                Op::Return => {
                    let v = pop!();
                    return self.do_return(v);
                }
                Op::ReturnR(s) => {
                    let v = rd!(s);
                    return self.do_return(v);
                }
                Op::Throw => {
                    let v = pop!();
                    return Err(v);
                }
                Op::ThrowConst(kind, i) => {
                    let msg = constant!(i);
                    let msg = msg.as_str().map(|s| s.to_rust_string()).unwrap_or_default();
                    let k = match kind {
                        0 => "ReferenceError",
                        1 => "TypeError",
                        _ => "SyntaxError",
                    };
                    return Err(self.new_error(k, &msg));
                }
                Op::Jump(t) => frame!().pc = t as usize,
                Op::JumpIfFalse(t) => {
                    if !{ let v = pop!(); self.to_boolean(&v) } {
                        frame!().pc = t as usize;
                    }
                }
                Op::JumpIfTrue(t) => {
                    if { let v = pop!(); self.to_boolean(&v) } {
                        frame!().pc = t as usize;
                    }
                }
                Op::JumpIfNullish(t) => {
                    if pop!().is_nullish() {
                        frame!().pc = t as usize;
                    }
                }
                Op::JumpIfNotNullishKeep(t) => {
                    if !peek!().is_nullish() {
                        frame!().pc = t as usize;
                    } else {
                        let _ = pop!();
                    }
                }
                Op::JumpIfFalseKeep(t) => {
                    if !{ let v = peek!().clone(); self.to_boolean(&v) } {
                        frame!().pc = t as usize;
                    }
                }
                Op::JumpIfTrueKeep(t) => {
                    if { let v = peek!().clone(); self.to_boolean(&v) } {
                        frame!().pc = t as usize;
                    }
                }
                Op::JumpIfUndefined(t) => {
                    if pop!().is_undefined() {
                        frame!().pc = t as usize;
                    }
                }
                Op::JumpIfNotUndefined(t) => {
                    if !pop!().is_undefined() {
                        frame!().pc = t as usize;
                    }
                }
                Op::JumpIfStrictEq(t) => {
                    let b = pop!();
                    let a = pop!();
                    if self.strict_equals(&a, &b) {
                        frame!().pc = t as usize;
                    }
                }
                Op::Add => {
                    let b = pop!();
                    let a = pop!();
                    let r = match (a.v(), b.v()) {
                        (V::Number(x), V::Number(y)) => Value::Number(x + y),
                        (V::String(x), V::String(y)) => Value::String(x.concat(y)),
                        _ => self.add_slow(a, b)?,
                    };
                    push!(r);
                }
                Op::Sub | Op::Mul | Op::Div | Op::Mod | Op::Exp | Op::BitAnd | Op::BitOr | Op::BitXor | Op::Shl | Op::Shr | Op::UShr => {
                    let b = pop!();
                    let a = pop!();
                    let r = match (a.v(), b.v()) {
                        (V::Number(x), V::Number(y)) => Value::Number(num_binop(op, x, y)),
                        _ => self.numeric_binop(op, a, b)?,
                    };
                    push!(r);
                }
                Op::Eq | Op::NotEq => {
                    let b = pop!();
                    let a = pop!();
                    let r = self.loose_equals(a, b)?;
                    push!(Value::Bool(if matches!(op, Op::Eq) { r } else { !r }));
                }
                Op::StrictEq | Op::StrictNotEq => {
                    let b = pop!();
                    let a = pop!();
                    let r = self.strict_equals(&a, &b);
                    push!(Value::Bool(if matches!(op, Op::StrictEq) { r } else { !r }));
                }
                Op::Lt | Op::LtEq | Op::Gt | Op::GtEq => {
                    let b = pop!();
                    let a = pop!();
                    let r = match (a.v(), b.v()) {
                        (V::Number(x), V::Number(y)) => match op {
                            Op::Lt => x < y,
                            Op::LtEq => x <= y,
                            Op::Gt => x > y,
                            _ => x >= y,
                        },
                        _ => self.compare_slow(op, a, b)?,
                    };
                    push!(Value::Bool(r));
                }
                Op::WithHas(i) => {
                    let obj = pop!();
                    let key = const_key!(i);
                    let found = match obj.v() {
                        V::Object(id) => {
                            if self.has_property(id, &key)? {
                                // Symbol.unscopables hides names from `with`.
                                match (self.get(Value::Object(id), PropertyKey::Symbol(SYM_UNSCOPABLES))?).v() {
                                    V::Object(u) => {
                                        let hidden = self.get(Value::Object(u), key)?;
                                        !self.to_boolean(&hidden)
                                    }
                                    _ => true,
                                }
                            } else {
                                false
                            }
                        }
                        _ => false,
                    };
                    push!(Value::Bool(found));
                }
                Op::ToObject => {
                    let v = pop!();
                    let id = self.to_object(v)?;
                    push!(Value::Object(id));
                }
                Op::In => {
                    let obj = pop!();
                    let key = pop!();
                    let id = match obj.v() {
                        V::Object(id) => id,
                        _ => {
                            let k = self.describe_for_error(&key);
                            return self.type_error(&format!("Cannot use 'in' operator to search for '{}' in a non-object", k));
                        }
                    };
                    let key = self.to_property_key(key)?;
                    let r = self.has_property(id, &key)?;
                    push!(Value::Bool(r));
                }
                Op::Instanceof => {
                    let target = pop!();
                    let v = pop!();
                    let r = self.instance_of(v, target)?;
                    push!(Value::Bool(r));
                }
                Op::Neg => {
                    let v = pop!();
                    let r = match v.v() {
                        V::Number(n) => Value::Number(-n),
                        other => match (self.to_numeric(other.to_value())?).v() {
                            V::BigInt(b) => Value::BigInt(Rc::new(b.neg())),
                            V::Number(n) => Value::Number(-n),
                            _ => unreachable!(),
                        },
                    };
                    push!(r);
                }
                Op::Plus => {
                    let v = pop!();
                    let n = self.to_number(v)?;
                    push!(Value::Number(n));
                }
                Op::Not => {
                    let v = pop!();
                    push!(Value::Bool(!self.to_boolean(&v)));
                }
                Op::BitNot => {
                    let v = pop!();
                    let r = match (self.to_numeric(v)?).v() {
                        V::BigInt(b) => Value::BigInt(Rc::new(b.not())),
                        V::Number(n) => Value::Number(!to_int32(n) as f64),
                        _ => unreachable!(),
                    };
                    push!(r);
                }
                Op::Typeof => {
                    let v = pop!();
                    let t = self.type_of(&v);
                    push!(Value::str(t));
                }
                Op::Void => {
                    let _ = pop!();
                    push!(Value::Undefined);
                }
                Op::Inc | Op::Dec => {
                    let v = pop!();
                    let d = if matches!(op, Op::Inc) { 1.0 } else { -1.0 };
                    let r = match v.v() {
                        V::Number(n) => Value::Number(n + d),
                        V::BigInt(b) => Value::BigInt(Rc::new(b.add(&BigInt::from_i128(d as i128)))),
                        other => match (self.to_numeric(other.to_value())?).v() {
                            V::Number(n) => Value::Number(n + d),
                            V::BigInt(b) => Value::BigInt(Rc::new(b.add(&BigInt::from_i128(d as i128)))),
                            _ => unreachable!(),
                        },
                    };
                    push!(r);
                }
                Op::ToNumeric => {
                    let v = pop!();
                    let r = match v.v() {
                        V::Number(_) | V::BigInt(_) => v,
                        other => self.to_numeric(other.to_value())?,
                    };
                    push!(r);
                }
                Op::ToString => {
                    let v = pop!();
                    let r = match v.v() {
                        V::String(_) => v,
                        other => Value::String(self.to_string(other.to_value())?),
                    };
                    push!(r);
                }
                Op::ToPropertyKey => {
                    let v = pop!();
                    let r = match v.v() {
                        // Numbers stay numbers: every consumer canonicalizes keys, and
                        // element fast paths want numeric indices.
                        V::String(_) | V::Symbol(_) | V::Number(_) => v,
                        other => self.to_property_key(other.to_value())?.to_value(),
                    };
                    push!(r);
                }
                Op::PrivateIn(d, s) => {
                    let obj = pop!();
                    let scope = frame!().scope;
                    let sym = self.slot_get(scope, d, s);
                    let id = match obj.v() {
                        V::Object(id) => id,
                        _ => return self.type_error("Cannot use 'in' operator to search for a private field in a non-object"),
                    };
                    let key = self.to_property_key(sym)?;
                    let r = self.obj(id).props.contains(&key);
                    push!(Value::Bool(r));
                }
                Op::NewObject => {
                    let o = self.new_object();
                    push!(Value::Object(o));
                }
                Op::NewArray(n) => {
                    let start = sp - n as usize;
                    let items: Vec<Value> = (start..sp).map(|i| std::mem::replace(&mut self.vstack[i], Value::Undefined)).collect();
                    sp = start;
                    let a = self.new_array(items);
                    push!(Value::Object(a));
                }
                Op::ArrayPush => {
                    let v = pop!();
                    let arr = peek!().as_object().unwrap();
                    if let ObjectKind::Array { elements, length } = &mut self.obj_mut(arr).kind {
                        elements.push(v);
                        *length = elements.len() as u32;
                    }
                }
                Op::ArrayHole => {
                    let arr = peek!().as_object().unwrap();
                    if let ObjectKind::Array { elements, length } = &mut self.obj_mut(arr).kind {
                        elements.push(Value::Empty);
                        *length = elements.len() as u32;
                    }
                }
                Op::ArraySpread => {
                    let v = pop!();
                    let items = self.iterate_to_vec(v)?;
                    let arr = peek!().as_object().unwrap();
                    if let ObjectKind::Array { elements, length } = &mut self.obj_mut(arr).kind {
                        elements.extend(items);
                        *length = elements.len() as u32;
                    }
                }
                Op::NewRegExp(p, f) => {
                    let pattern = constant!(p);
                    let flags = constant!(f);
                    let p = pattern.as_str().unwrap().to_rust_string();
                    let fl = flags.as_str().unwrap().to_rust_string();
                    let r = crate::builtins::regexp::regexp_create(self, &p, &fl)?;
                    push!(r);
                }
                Op::TemplateObject(idx) => {
                    let key = (frame!().code.as_ptr() as usize, idx);
                    let v = match self.template_cache.get(&key) {
                        Some(o) => Value::Object(*o),
                        None => {
                            let (cooked, raws) = frame!().code().templates[idx as usize].clone();
                            let raw_arr = self.new_array(raws.into_iter().map(Value::String).collect());
                            self.obj_mut(raw_arr).extensible = false;
                            let arr = self.new_array(cooked.into_iter().map(|c| c.map(Value::String).unwrap_or(Value::Undefined)).collect());
                            self.define_property(arr, "raw", Value::Object(raw_arr), 0);
                            self.obj_mut(arr).extensible = false;
                            self.template_cache.insert(key, arr);
                            Value::Object(arr)
                        }
                    };
                    push!(v);
                }
                Op::MakeClosure(idx) => {
                    let code = frame!().code().functions[idx as usize].clone();
                    let f = self.make_closure(fi, code);
                    push!(Value::Object(f));
                }
                Op::MakeClass(idx) => {
                    let superclass = pop!();
                    let code = frame!().code().functions[idx as usize].clone();
                    let (ctor, proto) = self.make_class(fi, code, superclass)?;
                    push!(Value::Object(ctor));
                    push!(Value::Object(proto));
                }
                Op::SetClassFields => {
                    let f = pop!();
                    let proto = self.vstack[sp - 1].as_object().unwrap();
                    let ctor = self.vstack[sp - 2].as_object().unwrap();
                    if let V::Object(fid) = f.v() {
                        if let ObjectKind::Function(c) = &mut self.obj_mut(fid).kind {
                            c.home_object = Some(proto);
                        }
                    }
                    if let ObjectKind::Function(c) = &mut self.obj_mut(ctor).kind {
                        c.fields = Some(f);
                    }
                }
                Op::NewPrivateName(i) => {
                    let desc = constant!(i).as_str().unwrap().clone();
                    let s = self.new_private_symbol(desc);
                    push!(Value::Symbol(s));
                }
                Op::InitFields => {
                    let func = frame!().func;
                    let this = frame!().this.clone();
                    self.run_field_initializers(func, this)?;
                }
                Op::This => {
                    let t = frame!().this.clone();
                    push!(if t.is_empty() { Value::Undefined } else { t });
                }
                Op::ThisChecked => {
                    let t = frame!().this.clone();
                    if t.is_empty() {
                        return Err(self.new_error("ReferenceError", "Must call super constructor in derived class before accessing 'this' or returning from derived constructor"));
                    }
                    push!(t);
                }
                Op::NewTarget => {
                    let v = frame!().new_target.clone();
                    push!(v);
                }
                Op::Arguments => {
                    let v = frame!().extra.as_ref().and_then(|e| e.arguments_obj.clone()).unwrap_or(Value::Undefined);
                    push!(v);
                }
                Op::ArgsLength => {
                    let n = frame!().argc;
                    push!(Value::Number(n as f64));
                }
                Op::ArgsGet => {
                    let key = pop!();
                    let v = self.args_get(fi, key)?;
                    push!(v);
                }
                Op::Callee => {
                    let v = frame!().func.map(Value::Object).unwrap_or(Value::Undefined);
                    push!(v);
                }
                Op::ImportMeta => {
                    let v = match frame!().code().module {
                        Some(m) => Value::Object(self.import_meta(crate::modules::ModuleId(m))),
                        None => Value::Object(self.new_object()),
                    };
                    push!(v);
                }
                Op::DynamicImport => {
                    let specifier = pop!();
                    let referrer = frame!().code().filename.clone();
                    let p = self.start_dynamic_import(specifier, &referrer, Value::Undefined);
                    push!(Value::Object(p));
                }
                Op::DynamicImportWith => {
                    let options = pop!();
                    let specifier = pop!();
                    let referrer = frame!().code().filename.clone();
                    let p = self.start_dynamic_import(specifier, &referrer, options);
                    push!(Value::Object(p));
                }
                Op::GetIterator => {
                    let v = pop!();
                    let (it, next) = self.get_iterator(v)?;
                    push!(it);
                    push!(next);
                }
                Op::GetAsyncIterator => {
                    let v = pop!();
                    let (it, next) = self.get_async_iterator(v)?;
                    push!(it);
                    push!(next);
                }
                Op::IteratorNext => {
                    let next = pop!();
                    let it = pop!();
                    match self.iterator_step(&it, &next)? {
                        Some(v) => {
                            push!(v);
                            push!(Value::Bool(false));
                        }
                        None => {
                            push!(Value::Undefined);
                            push!(Value::Bool(true));
                        }
                    }
                }
                Op::IteratorNextResult => {
                    let next = pop!();
                    let it = pop!();
                    let r = self.call(next, it, &[])?;
                    push!(r);
                }
                Op::IteratorNextWith => {
                    let received = pop!();
                    let next = self.vstack[sp - 1].clone();
                    let it = self.vstack[sp - 2].clone();
                    let r = self.call(next, it, &[received])?;
                    if !r.is_object() {
                        return self.type_error("Iterator result is not an object");
                    }
                    push!(r);
                }
                Op::IteratorStep => {
                    let n = sp;
                    let next = self.vstack[n - 1].clone();
                    if next.is_empty() {
                        push!(Value::Undefined);
                    } else {
                        let it = self.vstack[n - 2].clone();
                        match self.iterator_step(&it, &next)? {
                            Some(v) => push!(v),
                            None => {
                                self.vstack[n - 1] = Value::Empty;
                                push!(Value::Undefined);
                            }
                        }
                    }
                }
                Op::IteratorRest => {
                    let n = sp;
                    let next = self.vstack[n - 1].clone();
                    let mut items = Vec::new();
                    if !next.is_empty() {
                        let it = self.vstack[n - 2].clone();
                        items = self.collect_iterator(&it, &next)?;
                        self.vstack[n - 1] = Value::Empty;
                    }
                    let a = self.new_array(items);
                    push!(Value::Object(a));
                }
                Op::IteratorStepReg(ri, rn) => {
                    let next = sl!(rn).clone();
                    if next.is_empty() {
                        push!(Value::Undefined);
                    } else {
                        let it = sl!(ri).clone();
                        match self.iterator_step(&it, &next) {
                            Ok(Some(v)) => push!(v),
                            Ok(None) => {
                                sl!(rn) = Value::Empty;
                                push!(Value::Undefined);
                            }
                            Err(e) => {
                                sl!(rn) = Value::Empty;
                                return Err(e);
                            }
                        }
                    }
                }
                Op::IteratorRestReg(ri, rn) => {
                    let next = sl!(rn).clone();
                    let mut items = Vec::new();
                    if !next.is_empty() {
                        let it = sl!(ri).clone();
                        // Rooted while the iterator runs (it may collect).
                        let r = self.collect_iterator(&it, &next);
                        sl!(rn) = Value::Empty;
                        items = r?;
                    }
                    let a = self.new_array(items);
                    push!(Value::Object(a));
                }
                Op::IteratorCloseIfNotDoneReg(ri, rn) => {
                    if !sl!(rn).is_empty() {
                        sl!(rn) = Value::Empty;
                        let it = sl!(ri).clone();
                        self.iterator_close(&it)?;
                    }
                }
                Op::IteratorCloseOnThrow(ri, rn) => {
                    if !sl!(rn).is_empty() && !self.terminating {
                        sl!(rn) = Value::Empty;
                        let it = sl!(ri).clone();
                        let _ = self.iterator_close(&it);
                    }
                }
                Op::IteratorCloseIfNotDone => {
                    let next = pop!();
                    let it = pop!();
                    if !next.is_empty() {
                        self.iterator_close(&it)?;
                    }
                }
                Op::IteratorClose => {
                    let _next = pop!();
                    let it = pop!();
                    self.iterator_close(&it)?;
                }
                Op::IteratorCloseQuiet => {
                    let _next = pop!();
                    let it = pop!();
                    if !self.terminating {
                        let _ = self.iterator_close(&it);
                    }
                }
                Op::ForInStart => {
                    let v = pop!();
                    let it = self.for_in_start(v)?;
                    push!(Value::Object(it));
                }
                Op::ForInNext => {
                    let it = pop!().as_object().unwrap();
                    let r = self.for_in_next(it)?;
                    match r {
                        Some(k) => {
                            push!(Value::String(k));
                            push!(Value::Bool(false));
                        }
                        None => {
                            push!(Value::Undefined);
                            push!(Value::Bool(true));
                        }
                    }
                }
                Op::YieldStarStep(is_async) => {
                    let mode = pop!();
                    let received = pop!();
                    let next = self.vstack[sp - 1].clone();
                    let it = self.vstack[sp - 2].clone();
                    let mode = mode.as_number().unwrap_or(0.0) as u8;
                    let out = self.yield_star_step(&it, &next, received, mode, is_async)?;
                    push!(out);
                }
                Op::YieldStarCheck => {
                    let r = pop!();
                    let mode = self.ystar.1;
                    let out = self.yield_star_check(r, mode, true)?;
                    push!(out);
                }
                Op::YStarIs(k) => {
                    push!(Value::Bool(self.ystar.0 == k));
                }
                Op::ResumeMode => {
                    let m = std::mem::replace(&mut self.resume_mode, 0);
                    push!(Value::Number(m as f64));
                }
                Op::ResumeIsReturn => {
                    let r = std::mem::replace(&mut self.resume_mode, 0) == 2;
                    push!(Value::Bool(r));
                }
                Op::CheckCoercibleUnder => {
                    if self.vstack[sp - 2].is_nullish() {
                        let k = self.vstack[sp - 1].clone();
                        let what = if self.vstack[sp - 2].is_null() { "null" } else { "undefined" };
                        let key = match k.v() {
                            V::String(s) => format!(" (reading '{}')", s.to_rust_string()),
                            _ => String::new(),
                        };
                        return self.type_error(&format!("Cannot read properties of {what}{key}"));
                    }
                }
                Op::DeleteGlobal(i) => {
                    let key = const_key!(i);
                    let lexical = key.as_str().map(|n| self.global_lex.get(n).is_some()).unwrap_or(false);
                    let r = if lexical {
                        false
                    } else {
                        let g = self.global;
                        self.delete_property(g, &key)?
                    };
                    push!(Value::Bool(r));
                }
                Op::EvalSite(i) => {
                    self.eval_site = i;
                }
                Op::NewEvalVars => {
                    let o = self.new_object_with_proto(None);
                    push!(Value::Object(o));
                }
                Op::DeclareEvalVar(i) => {
                    let obj = pop!();
                    let key = const_key!(i);
                    if let Some(o) = obj.as_object() {
                        if !self.obj(o).props.contains(&key) {
                            self.define_property(o, key, Value::Undefined, WRITABLE | ENUMERABLE | CONFIGURABLE);
                        }
                    }
                }
                Op::DeclareEvalGlobalFunction(i) => {
                    let f = pop!();
                    let key = const_key!(i);
                    let g = self.global;
                    match self.get_own_property(g, &key) {
                        Some(p) if !p.configurable() => {
                            self.set(Value::Object(g), key, f)?;
                        }
                        _ => {
                            self.define_property(g, key, f, WRITABLE | ENUMERABLE | CONFIGURABLE);
                        }
                    }
                }
                Op::DeclareEvalGlobalVar(i) => {
                    let key = const_key!(i);
                    if let Some(name) = key.as_str() {
                        if self.global_lex.get(name).is_some() {
                            let n = name.to_rust_string();
                            return Err(self.new_error("SyntaxError", &format!("Identifier '{n}' has already been declared")));
                        }
                    }
                    let g = self.global;
                    if !self.has_own_property(g, &key) {
                        if !self.obj(g).extensible {
                            let k = self.key_display(&key);
                            return self.type_error(&format!("Cannot declare global variable '{k}'"));
                        }
                        self.define_property(g, key, Value::Undefined, WRITABLE | ENUMERABLE | CONFIGURABLE);
                    }
                }
                Op::GetPrivate => {
                    let key = pop!();
                    let obj = pop!();
                    let v = self.private_get(&obj, &key)?;
                    push!(v);
                }
                Op::GetPrivateKeep => {
                    let key = self.vstack[sp - 1].clone();
                    let obj = self.vstack[sp - 2].clone();
                    let v = self.private_get(&obj, &key)?;
                    push!(v);
                }
                Op::SetPrivate => {
                    let v = pop!();
                    let key = pop!();
                    let obj = pop!();
                    self.private_set(&obj, &key, v.clone())?;
                    push!(v);
                }
                Op::DefinePrivate => {
                    let v = pop!();
                    let key = pop!();
                    let obj = peek!().clone();
                    self.private_define(&obj, &key, Property::data(v, WRITABLE))?;
                }
                Op::CopyPrivate => {
                    let template = pop!();
                    let obj = peek!().clone();
                    self.private_copy(&obj, &template)?;
                }
                Op::TakePrivate => {
                    let proto = pop!();
                    let t = self.private_take(&proto);
                    push!(t);
                }
                Op::Yield | Op::AsyncYield | Op::YieldRaw => {
                    let v = pop!();
                    if matches!(op, Op::YieldRaw) {
                        self.yield_raw = true;
                    }
                    frame!().extra_mut().resume_slot = (sp - fb) as u32;
                    let mut f = self.frames.pop().unwrap();
                    self.detach_slots(&mut f);
                    return Ok(Flow::Exit(Exit::Yield(v, Box::new(f))));
                }
                Op::Await => {
                    let v = pop!();
                    frame!().extra_mut().resume_slot = (sp - fb) as u32;
                    let mut f = self.frames.pop().unwrap();
                    self.detach_slots(&mut f);
                    return Ok(Flow::Exit(Exit::Await(v, Box::new(f))));
                }
                Op::StoreCompletion => {
                    let v = pop!();
                    frame!().extra_mut().completion = v;
                }
                Op::LoadCompletion => {
                    let v = frame!().extra.as_ref().map(|e| e.completion.clone()).unwrap_or(Value::Undefined);
                    push!(v);
                }
                Op::Debugger | Op::Nop => {}
            }
        }
        Ok(Flow::Next)
    }

    // ------------------------------------------------------------ helpers

    fn reg_name(&self, fi: usize, r: u16) -> String {
        match self.frames[fi].code().reg_names.get(r as usize) {
            Some(n) if !n.is_empty() => n.to_rust_string(),
            _ => "variable".to_string(),
        }
    }

    fn slot_name(&self, fi: usize, depth: u16, slot: u16) -> String {
        let frame = &self.frames[fi];
        let scope = self.scope_at(frame.scope, depth);
        if let ObjectKind::Scope(sd) = &self.heap.get(scope).kind {
            if let Some(names) = &sd.names {
                if let Some(n) = names.get(slot as usize) {
                    if !n.is_empty() {
                        return n.to_rust_string();
                    }
                }
            } else if depth as u32 == frame.scope_depth && frame.code().scope_slots > 0 {
                if let Some(n) = frame.code().slot_names.get(slot as usize) {
                    if !n.is_empty() {
                        return n.to_rust_string();
                    }
                }
            }
        }
        "variable".to_string()
    }

    fn get_global_binding(&mut self, key: &PropertyKey) -> JsResult<Value> {
        let name = key.as_str().unwrap();
        if let Some((v, _)) = self.global_lex.get(name) {
            if v.is_empty() {
                let n = name.to_rust_string();
                return Err(self.new_error("ReferenceError", &format!("Cannot access '{}' before initialization", n)));
            }
            return Ok(v.clone());
        }
        let g = self.global;
        // Fast path: own data property.
        if let Some(p) = self.obj(g).props.get(key) {
            if !p.is_accessor() {
                return Ok(p.value.clone());
            }
        }
        if self.has_property(g, key)? {
            return self.get_from_object(g, key, Value::Object(g));
        }
        let n = name.to_rust_string();
        Err(self.new_error("ReferenceError", &format!("{} is not defined", n)))
    }

    fn set_global_binding(&mut self, key: &PropertyKey, v: Value, strict: bool) -> JsResult<()> {
        let name = key.as_str().unwrap();
        if let Some((cur, is_const)) = self.global_lex.get(name) {
            if *is_const {
                return self.type_error("Assignment to constant variable.");
            }
            if cur.is_empty() {
                let n = name.to_rust_string();
                return Err(self.new_error("ReferenceError", &format!("Cannot access '{}' before initialization", n)));
            }
            self.global_lex.insert(name.clone(), (v, false));
            return Ok(());
        }
        let g = self.global;
        if strict && !self.has_property(g, key)? {
            let n = name.to_rust_string();
            return Err(self.new_error("ReferenceError", &format!("{} is not defined", n)));
        }
        let ok = self.set_on_object(g, key.clone(), v, Value::Object(g))?;
        if !ok && strict {
            let n = name.to_rust_string();
            return self.type_error(&format!("Cannot assign to read only property '{}' of object", n));
        }
        Ok(())
    }

    fn get_elem(&mut self, obj: Value, key: Value) -> JsResult<Value> {
        if let Some(v) = self.get_elem_fast(&obj, &key) {
            return Ok(v);
        }
        let key = match key.v() {
            // Integer keys: cached index strings, no number formatting.
            V::Number(n) if (n as u32) as f64 == n && n < 4294967295.0 => PropertyKey::index(n as u32),
            other => self.to_property_key(other.to_value())?,
        };
        if let (true, PropertyKey::String(k)) = (obj.is_object(), &key) {
            // `obj[name]` with a string name: the shared cache by (shape, name).
            if let Some(ic) = self.mega_get(&obj, k.bits()) {
                if let Some(prop) = self.ic_get(&ic, &obj) {
                    if prop.flags & ACCESSOR == 0 {
                        return Ok(prop.value.clone());
                    }
                }
            }
            let v = self.get(obj.clone(), key.clone())?;
            let rec = self.ic_fill_get(&obj, &key);
            self.mega_put(&obj, &key, rec);
            return Ok(v);
        }
        self.get(obj, key)
    }

    /// `obj[key]` for dense array elements and string characters.
    #[inline]
    pub(crate) fn get_elem_fast(&self, obj: &Value, key: &Value) -> Option<Value> {
        let V::Number(n) = key.v() else { return None };
        let i = n as u32;
        if i as f64 != n {
            return None;
        }
        match obj.v() {
            V::Object(id) => match &self.heap.try_get(id)?.kind {
                ObjectKind::Array { elements, .. } => match elements.get(i as usize) {
                    Some(v) if !v.is_empty() => Some(v.clone()),
                    _ => None,
                },
                ObjectKind::TypedArray(t) if !matches!(t.kind, TypedArrayKind::BigInt64 | TypedArrayKind::BigUint64) => crate::builtins::typed_array_get(self, t, i as usize),
                _ => None,
            },
            V::String(s) if (i as usize) < s.len() => Some(Value::String(s.char_at(i as usize))),
            _ => None,
        }
    }

    /// `obj[key] = v` for an existing dense element or an append; gives `v`
    /// back when not applicable.
    #[inline]
    pub(crate) fn set_elem_fast(&mut self, obj: &Value, key: &Value, v: Value) -> Result<(), Value> {
        let (V::Object(id), V::Number(n)) = (obj.v(), key.v()) else { return Err(v) };
        let i = n as u32;
        if i as f64 != n || i >= (1 << 28) {
            return Err(v);
        }
        if let (Some(ObjectKind::TypedArray(t)), V::Number(x)) = (self.heap.try_get(id).map(|o| &o.kind), v.v()) {
            if !matches!(t.kind, TypedArrayKind::BigInt64 | TypedArrayKind::BigUint64) {
                // A Number needs no conversion: out-of-range writes are ignored.
                crate::builtins::typed_array_set(self, t, i as usize, x);
                return Ok(());
            }
            return Err(v);
        }
        let array_proto = Some(self.intrinsics.array_proto);
        let Some(o) = self.heap.try_get_mut(id) else { return Err(v) };
        let no_props = o.props.is_empty();
        // Appends need the plain prototype (a typed array prototype decides).
        let extensible = o.extensible && o.proto == array_proto;
        match &mut o.kind {
            ObjectKind::Array { elements, length } => {
                let i = i as usize;
                if i < elements.len() {
                    if elements[i].is_empty() {
                        return Err(v);
                    }
                    elements[i] = v;
                    Ok(())
                } else if i == elements.len() && *length as usize == i && no_props && extensible {
                    elements.push(v);
                    *length = (i + 1) as u32;
                    Ok(())
                } else {
                    Err(v)
                }
            }
            _ => Err(v),
        }
    }

    fn delete_elem(&mut self, obj: Value, key: PropertyKey, strict: bool) -> JsResult<bool> {
        let id = match obj.v() {
            V::Object(id) => id,
            V::Undefined | V::Null => return self.type_error("Cannot convert undefined or null to object"),
            _ => return Ok(true),
        };
        let r = self.delete_property(id, &key)?;
        if !r && strict {
            let k = self.key_display(&key);
            return self.type_error(&format!("Cannot delete property '{}' of object", k));
        }
        Ok(r)
    }

    /// Move the operands from `from` up out of frame `fi` into the reusable
    /// argument buffer.
    #[inline]
    /// Move the values of value-stack slots `from..to` into a (pooled)
    /// argument buffer.
    fn take_args(&mut self, from: usize, to: usize) -> Vec<Value> {
        let mut buf = std::mem::take(&mut self.arg_buf);
        for v in &mut self.vstack[from..to] {
            buf.push(std::mem::replace(v, Value::Undefined));
        }
        buf
    }

    #[inline]
    fn give_args(&mut self, mut buf: Vec<Value>) {
        buf.clear();
        if buf.capacity() <= 64 {
            self.arg_buf = buf;
        }
    }

    /// Call from the current frame; the result goes to its slot `ret`
    /// (directly, or when a pushed JS frame returns).
    fn do_call(&mut self, f: Value, this: Value, args: &[Value], ret: usize) -> JsResult<()> {
        if let V::Object(id) = f.v() {
            match &self.obj(id).kind {
                ObjectKind::Function(c) => {
                    let flags = c.code.flags;
                    if !flags.is_generator && !flags.is_async && !flags.is_class_constructor {
                        if self.push_plain_call(id, &this, args, ret as u32, false)? {
                            return Ok(());
                        }
                        let mut frame = self.prepare_frame(id, this, args, Value::Undefined, false)?;
                        frame.ret_slot = ret as u32;
                        self.push_frame(frame, false)?;
                        return Ok(());
                    }
                }
                ObjectKind::Native(n) => {
                    let func = n.func;
                    let v = self.call_native(id, func, this, args, false)?;
                    self.set_current_slot(ret, v);
                    return Ok(());
                }
                _ => {}
            }
        }
        let v = self.call(f, this, args)?;
        self.set_current_slot(ret, v);
        Ok(())
    }

    /// Write slot `slot` of the current frame.
    #[inline]
    fn set_current_slot(&mut self, slot: usize, v: Value) {
        let at = self.frames.last().unwrap().base as usize + slot;
        self.vstack[at] = v;
    }

    fn do_construct(&mut self, f: Value, args: &[Value], ret: usize) -> JsResult<()> {
        if let V::Object(id) = f.v() {
            if let ObjectKind::Function(_) = &self.obj(id).kind {
                if self.is_constructor(&f) {
                    let mut frame = self.prepare_frame(id, Value::Empty, args, f.clone(), true)?;
                    frame.ret_slot = ret as u32;
                    self.push_frame(frame, false)?;
                    return Ok(());
                }
            }
        }
        let v = self.construct(f.clone(), args, f)?;
        self.set_current_slot(ret, v);
        Ok(())
    }

    /// A constructor returning the non-object `v`: the error, if any
    /// (a derived constructor may only return undefined, and only once
    /// `this` is bound). It is thrown to the caller: the constructor's own
    /// handlers no longer apply.
    #[cold]
    fn construct_return_error(&mut self, v: &Value) -> Option<Value> {
        let f = self.frames.last().unwrap();
        let derived = f.code().flags.is_derived;
        // Thrown in the caller's realm (after the callee's context is gone).
        let n = self.frames.len();
        let caller_realm = if f.entry || n < 2 { self.entry_realm } else { self.frames[n - 2].code().realm.get() };
        let e = if derived && !v.is_undefined() {
            self.in_realm(caller_realm, |r| r.new_error("TypeError", "Derived constructors may only return object or undefined"))
        } else if f.this.is_empty() {
            self.in_realm(caller_realm, |r| r.new_error("ReferenceError", "Must call super constructor in derived class before accessing 'this' or returning from derived constructor"))
        } else {
            return None;
        };
        let f = self.frames.last_mut().unwrap();
        f.pc = f.code().ops.len() + 1;
        Some(e)
    }

    /// Return `v` from the current frame.
    fn do_return(&mut self, mut v: Value) -> JsResult<Flow> {
        let f = self.frames.last().unwrap();
        if f.is_construct && !v.is_object() {
            if let Some(e) = self.construct_return_error(&v) {
                return Err(e);
            }
            v = self.frames.last().unwrap().this.clone();
        }
        let f = self.pop_frame();
        if f.entry {
            if let Some(g) = f.generator() {
                if let ObjectKind::Generator(gd) = &mut self.obj_mut(g).kind {
                    gd.state = GeneratorState::Completed;
                    gd.frame = None;
                }
            }
            return Ok(Flow::Exit(Exit::Return(v)));
        }
        let ret = f.ret_slot as usize;
        self.set_current_slot(ret, v);
        Ok(Flow::Next)
    }

    /// String concatenation of `+`: a RangeError past the maximum length.
    fn concat(&mut self, x: &JsStr, y: &JsStr) -> JsResult<Value> {
        if x.len() + y.len() > crate::value::MAX_STRING_LEN {
            return self.range_error("Invalid string length");
        }
        Ok(Value::String(x.concat(y)))
    }

    /// A binary operator on two values (the slow path of the register
    /// instructions).
    fn binop_values(&mut self, op: Op, a: Value, b: Value) -> JsResult<Value> {
        Ok(match op {
            Op::Add => match (a.v(), b.v()) {
                (V::Number(x), V::Number(y)) => Value::Number(x + y),
                (V::String(x), V::String(y)) => self.concat(x, y)?,
                (V::String(x), V::Number(_)) => {
                    let y = self.to_string(b.clone())?;
                    self.concat(x, &y)?
                }
                (V::Number(_), V::String(y)) => {
                    let x = self.to_string(a.clone())?;
                    self.concat(&x, y)?
                }
                _ => self.add_slow(a, b)?,
            },
            Op::Lt | Op::LtEq | Op::Gt | Op::GtEq => Value::Bool(match (a.v(), b.v()) {
                (V::Number(x), V::Number(y)) => match op {
                    Op::Lt => x < y,
                    Op::LtEq => x <= y,
                    Op::Gt => x > y,
                    _ => x >= y,
                },
                _ => self.compare_slow(op, a, b)?,
            }),
            Op::Eq | Op::NotEq => {
                let r = self.loose_equals(a, b)?;
                Value::Bool(if matches!(op, Op::Eq) { r } else { !r })
            }
            Op::StrictEq | Op::StrictNotEq => {
                let r = self.strict_equals(&a, &b);
                Value::Bool(if matches!(op, Op::StrictEq) { r } else { !r })
            }
            _ => match (a.v(), b.v()) {
                (V::Number(x), V::Number(y)) => Value::Number(num_binop(op, x, y)),
                _ => self.numeric_binop(op, a, b)?,
            },
        })
    }

    /// A unary operator on a value.
    fn unary_value(&mut self, op: Op, v: Value) -> JsResult<Value> {
        Ok(match op {
            Op::Not => Value::Bool(!self.to_boolean(&v)),
            Op::Inc => self.inc_value(v, 1.0)?,
            Op::Dec => self.inc_value(v, -1.0)?,
            Op::ToNumeric => match v.v() {
                V::Number(_) | V::BigInt(_) => v,
                other => self.to_numeric(other.to_value())?,
            },
            Op::Neg => match v.v() {
                V::Number(n) => Value::Number(-n),
                other => match (self.to_numeric(other.to_value())?).v() {
                    V::BigInt(b) => Value::BigInt(Rc::new(b.neg())),
                    V::Number(n) => Value::Number(-n),
                    _ => unreachable!(),
                },
            },
            Op::Plus => Value::Number(self.to_number(v)?),
            Op::BitNot => match (self.to_numeric(v)?).v() {
                V::BigInt(b) => Value::BigInt(Rc::new(b.not())),
                V::Number(n) => Value::Number(!to_int32(n) as f64),
                _ => unreachable!(),
            },
            Op::Typeof => Value::str(self.type_of(&v)),
            Op::Void => Value::Undefined,
            Op::ToString => match v.v() {
                V::String(_) => v,
                other => Value::String(self.to_string(other.to_value())?),
            },
            Op::ToPropertyKey => match v.v() {
                // Numbers stay numbers: every consumer canonicalizes keys,
                // and element fast paths want numeric indices.
                V::String(_) | V::Symbol(_) | V::Number(_) => v,
                other => self.to_property_key(other.to_value())?.to_value(),
            },
            _ => unreachable!("not a unary operator"),
        })
    }

    /// Direct `eval(src)` from frame `fi`: the code runs in the caller's
    /// scope (see `compiler::compile_eval`).
    fn direct_eval(&mut self, fi: usize, arg: Value, strict: bool, site: Option<Rc<crate::compiler::EvalSite>>) -> JsResult<Value> {
        let src = match arg.v() {
            V::String(s) => s.to_source_string(),
            other => return Ok(other.to_value()),
        };
        let Some(site) = site else {
            let src = if strict { format!("\"use strict\";{}", src) } else { src };
            let code = self.compile_escaped(&src, "eval", false, true)?;
            return self.run_script(code);
        };
        let ctx = crate::early::EarlyContext {
            strict: site.strict,
            in_function: site.in_function,
            in_method: site.in_method,
            in_derived_ctor: site.in_derived_ctor,
            in_class_field_init: site.in_field_init,
            private_names: site.private_names(),
        };
        let program = match crate::parser::parse_program_in(&src, false, &ctx) {
            Ok(mut p) => {
                p.strict |= site.strict;
                p
            }
            Err(e) => {
                let msg = format!("{} (eval:{}:{})", e.message, e.line, e.column);
                return Err(self.new_error("SyntaxError", &msg));
            }
        };
        let code = match crate::compiler::compile_eval(&program, &site, &src) {
            Ok(c) => c,
            Err(e) => return Err(self.new_error("SyntaxError", &e.message)),
        };
        self.adopt_code(&code);
        let f = self.make_closure(fi, code);
        self.call(Value::Object(f), Value::Undefined, &[])
    }

    fn super_call(&mut self, fi: usize, args: Vec<Value>) -> JsResult<Value> {
        let (func, new_target, this) = {
            let f = &self.frames[fi];
            (f.func, f.new_target.clone(), f.this.clone())
        };
        if !this.is_empty() {
            return Err(self.new_error("ReferenceError", "Super constructor may only be called once"));
        }
        let func = match func {
            Some(f) => f,
            None => return self.type_error("'super' keyword unexpected here"),
        };
        let parent = self.obj(func).proto;
        let parent = match parent {
            Some(p) if self.is_constructor(&Value::Object(p)) => p,
            _ => return self.type_error("Super constructor is not a constructor"),
        };
        let result = self.construct(Value::Object(parent), &args, new_target)?;
        self.frames[fi].this = result.clone();
        self.run_field_initializers(Some(func), result.clone())?;
        Ok(result)
    }

    fn run_field_initializers(&mut self, func: Option<ObjectId>, this: Value) -> JsResult<()> {
        let fields = match func {
            Some(f) => match &self.obj(f).kind {
                ObjectKind::Function(c) => c.fields.clone(),
                _ => None,
            },
            None => None,
        };
        if let Some(init) = fields {
            self.call(init, this, &[])?;
        }
        Ok(())
    }

    /// GetSuperBase: the prototype of the running function's home object
    /// (null when it has none).
    fn super_base(&mut self, fi: usize) -> JsResult<Value> {
        let home = self.frame_home_object(&self.frames[fi]);
        let Some(home) = home else { return self.type_error("'super' keyword unexpected here") };
        Ok(match self.get_prototype_of(home)? {
            Some(p) => Value::Object(p),
            None => Value::Null,
        })
    }

    /// ToObject of a super reference's base (a null prototype throws).
    fn super_base_object(&mut self, base: &Value, key: Option<&PropertyKey>) -> JsResult<ObjectId> {
        match base.v() {
            V::Object(o) => Ok(o),
            _ => {
                let k = key.map(|k| self.key_display(k)).unwrap_or_default();
                self.type_error(&format!("Cannot read properties of null (reading 'super.{k}')"))
            }
        }
    }

    fn super_get(&mut self, p: ObjectId, key: PropertyKey, this: Value) -> JsResult<Value> {
        // `super.m` in class hierarchies: the lookup starts at the home
        // object's prototype, cached like an ordinary read from it.
        if let PropertyKey::String(k) = &key {
            let start = Value::Object(p);
            if let Some(ic) = self.mega_get(&start, k.bits()) {
                let hit = match self.ic_get(&ic, &start) {
                    Some(prop) if prop.flags & ACCESSOR == 0 => Some(Ok(prop.value.clone())),
                    Some(prop) => prop.getter().cloned().map(Err),
                    None => None,
                };
                match hit {
                    Some(Ok(v)) => return Ok(v),
                    Some(Err(g)) if !g.is_undefined() => return self.call(g, this, &[]),
                    _ => {}
                }
            }
            let v = self.get_from_object(p, &key, this)?;
            let rec = self.ic_fill_get(&start, &key);
            self.mega_put(&start, &key, rec);
            return Ok(v);
        }
        self.get_from_object(p, &key, this)
    }

    fn super_set(&mut self, fi: usize, base: ObjectId, key: PropertyKey, v: Value, this: Value) -> JsResult<()> {
        let ok = self.set_on_object(base, key.clone(), v, this)?;
        if !ok && self.frames[fi].code().flags.is_strict {
            let k = self.key_display(&key);
            return self.type_error(&format!("Cannot assign to property '{k}' through super"));
        }
        Ok(())
    }

    fn copy_data_properties(&mut self, target: ObjectId, src: Value, excluded: &[PropertyKey]) -> JsResult<()> {
        let sid = match src.v() {
            V::Undefined | V::Null | V::Empty => return Ok(()),
            V::Object(id) => id,
            prim => self.to_object(prim.to_value())?,
        };
        let keys = self.own_keys(sid)?;
        for k in keys {
            if excluded.contains(&k) {
                continue;
            }
            let p = self.get_own_property_any(sid, &k)?;
            if let Some(p) = p {
                if !p.enumerable() {
                    continue;
                }
                let v = self.get_from_object(sid, &k, Value::Object(sid))?;
                self.define_own(target, k, Property::data(v, ALL))?;
            }
        }
        Ok(())
    }

    // ----------------------------------------------------------- yield*
    //
    // Kinds left in `ystar.0`: 0 yield `out`, 1 done (yield* evaluates to
    // `out`), 2 return `out`, 3 await `out` then YieldStarCheck, 4 await
    // `out` then return it.

    fn yield_star_step(&mut self, it: &Value, next: &Value, received: Value, mode: u8, is_async: bool) -> JsResult<Value> {
        self.ystar = (0, mode);
        let r = match mode {
            1 => match self.get_method(it.clone(), PropertyKey::str("throw"))? {
                Some(m) => self.call(m, it.clone(), &[received])?,
                None => {
                    // The delegate cannot take the exception: close it.
                    if let Some(ret) = self.get_method(it.clone(), PropertyKey::str("return"))? {
                        let v = self.call(ret, it.clone(), &[])?;
                        if !is_async && !v.is_object() {
                            return self.type_error("Iterator result is not an object");
                        }
                    }
                    return self.type_error("The iterator does not provide a 'throw' method");
                }
            },
            2 => match self.get_method(it.clone(), PropertyKey::str("return"))? {
                Some(m) => self.call(m, it.clone(), &[received])?,
                None => {
                    self.ystar = (if is_async { 4 } else { 2 }, mode);
                    return Ok(received);
                }
            },
            _ => self.call(next.clone(), it.clone(), &[received])?,
        };
        if is_async {
            self.ystar = (3, mode);
            return Ok(r);
        }
        self.yield_star_check(r, mode, false)
    }

    fn yield_star_check(&mut self, r: Value, mode: u8, is_async: bool) -> JsResult<Value> {
        if !r.is_object() {
            return self.type_error("Iterator result is not an object");
        }
        let done = self.get(r.clone(), PropertyKey::str("done"))?;
        if self.to_boolean(&done) {
            let v = self.get(r, PropertyKey::str("value"))?;
            self.ystar = (if mode == 2 { 2 } else { 1 }, mode);
            return Ok(v);
        }
        self.ystar = (0, mode);
        if is_async {
            return self.get(r, PropertyKey::str("value"));
        }
        Ok(r)
    }

    // ------------------------------------------------ private elements
    //
    // A private element is an own property keyed by the private name's
    // symbol: fields are writable data properties, methods non-writable
    // data properties, accessors accessor properties. They are reached only
    // through these operations, which read the object's own properties
    // directly (no prototype chain, no Proxy traps).

    fn private_key(&mut self, key: &Value) -> JsResult<(PropertyKey, String)> {
        match key.as_symbol() {
            Some(s) => Ok((PropertyKey::Symbol(s), self.symbol_description(s))),
            None => self.type_error("invalid private name"),
        }
    }

    fn private_lookup(&mut self, obj: &Value, key: &Value, verb: &str) -> JsResult<(ObjectId, PropertyKey, Property)> {
        let (k, name) = self.private_key(key)?;
        let found = obj.as_object().and_then(|id| self.obj(id).props.get(&k).cloned().map(|p| (id, p)));
        match found {
            Some((id, p)) => Ok((id, k, p)),
            None => self.type_error(&format!("Cannot {verb} private member {name} from an object whose class did not declare it")),
        }
    }

    pub(crate) fn private_get(&mut self, obj: &Value, key: &Value) -> JsResult<Value> {
        let (_, k, p) = self.private_lookup(obj, key, "read")?;
        if p.is_accessor() {
            return match p.getter_cloned() {
                Some(g) if !g.is_undefined() => self.call(g, obj.clone(), &[]),
                _ => {
                    let name = self.key_display(&k);
                    self.type_error(&format!("'{name}' was defined without a getter"))
                }
            };
        }
        Ok(p.value)
    }

    pub(crate) fn private_set(&mut self, obj: &Value, key: &Value, v: Value) -> JsResult<()> {
        let (id, k, p) = self.private_lookup(obj, key, "write")?;
        if p.is_accessor() {
            return match p.setter_cloned() {
                Some(s) if !s.is_undefined() => self.call(s, obj.clone(), &[v]).map(|_| ()),
                _ => {
                    let name = self.key_display(&k);
                    self.type_error(&format!("'{name}' was defined without a setter"))
                }
            };
        }
        if !p.writable() {
            let name = self.key_display(&k);
            return self.type_error(&format!("Private method '{name}' is not writable"));
        }
        if let Some(slot) = self.obj_mut(id).props.get_mut(&k) {
            slot.value = v;
        }
        Ok(())
    }

    /// PrivateFieldAdd / PrivateMethodOrAccessorAdd.
    pub(crate) fn private_define(&mut self, obj: &Value, key: &Value, prop: Property) -> JsResult<()> {
        let (k, name) = self.private_key(key)?;
        let Some(id) = obj.as_object() else { return self.type_error(&format!("Cannot define private member {name} on a non-object")) };
        if self.obj(id).props.contains(&k) {
            return self.type_error(&format!("Cannot initialize {name} twice on the same object"));
        }
        self.obj_mut(id).props.insert(k, prop);
        Ok(())
    }

    /// Install every private method/accessor of a class (kept on a
    /// template object) on a new instance.
    fn private_copy(&mut self, obj: &Value, template: &Value) -> JsResult<()> {
        let Some(t) = template.as_object() else { return Ok(()) };
        let items: Vec<(PropertyKey, Property)> = self.obj(t).props.iter().map(|(k, p)| (k.clone(), p.clone())).collect();
        for (k, p) in items {
            if let PropertyKey::Symbol(s) = k {
                self.private_define(obj, &Value::Symbol(s), p)?;
            }
        }
        Ok(())
    }

    /// Move the private-keyed properties the class body defined on its
    /// prototype into a fresh template object.
    fn private_take(&mut self, proto: &Value) -> Value {
        let t = self.new_object_with_proto(None);
        if let Some(p) = proto.as_object() {
            let keys: Vec<PropertyKey> = self.obj(p).props.keys().into_iter().filter(|k| matches!(k, PropertyKey::Symbol(s) if self.is_private_symbol(*s))).collect();
            for k in keys {
                if let Some(prop) = self.obj_mut(p).props.remove(&k) {
                    self.obj_mut(t).props.insert(k, prop);
                }
            }
        }
        Value::Object(t)
    }

    pub(crate) fn set_function_name(&mut self, f: &Value, key: &PropertyKey, prefix: &str) {
        if let V::Object(id) = f.v() {
            let base = match key {
                PropertyKey::String(s) => s.to_rust_string(),
                PropertyKey::Symbol(s) => {
                    if self.is_private_symbol(*s) {
                        self.symbol_description(*s)
                    } else {
                        format!("[{}]", self.symbol_description(*s))
                    }
                }
            };
            let name = if prefix.is_empty() { base } else { format!("{} {}", prefix, base) };
            let existing = self.obj(id).props.get(&PropertyKey::str("name")).map(|p| p.value.clone());
            let keep = matches!(existing.as_ref().map(Value::v), Some(V::String(s)) if !s.is_empty());
            if !keep {
                self.define_property(id, "name", Value::str(&name), CONFIGURABLE);
            }
        }
    }

    /// Remember the plain, non-arrow closure just called through a `CallR`
    /// site (its frame is the last one).
    #[inline(never)]
    fn fill_call_ic(&self, cell: &std::cell::Cell<crate::bytecode::CallIc>, callee: u64) {
        let Some(f) = self.frames.last() else { return };
        let Some(id) = f.func else { return };
        if id.0 as u64 != callee & 0xffff_ffff {
            return;
        }
        let Some(ObjectKind::Function(c)) = self.heap.try_get(id).map(|o| &o.kind) else { return };
        // A callee of another realm binds a sloppy `this` to its own
        // global: no cached shortcut for it.
        if c.code.call_flags & (CALL_PLAIN | CALL_ARROW) != CALL_PLAIN || c.code.realm.get() != self.current_realm {
            return;
        }
        cell.set(crate::bytecode::CallIc {
            callee,
            epoch: self.heap.collections,
            scope: c.scope.map(|s| s.0).unwrap_or(u32::MAX),
            code: &*c.code as *const FunctionCode,
            strict: c.code.call_flags & CALL_STRICT != 0,
        });
    }

    /// The `[[HomeObject]]` of the function a frame runs (`super` lookups).
    pub(crate) fn frame_home_object(&self, f: &Frame) -> Option<ObjectId> {
        match self.heap.try_get(f.func?).map(|o| &o.kind) {
            Some(ObjectKind::Function(c)) => c.home_object,
            _ => None,
        }
    }

    /// Create a function object for `code` in the context of frame `fi`.
    pub(crate) fn make_closure(&mut self, fi: usize, code: Rc<FunctionCode>) -> ObjectId {
        // A lazy function compiled through another closure: use its code.
        let code = match code.compiled.get() {
            Some(c) => c.clone(),
            None => code,
        };
        let frame = &self.frames[fi];
        let flags = code.flags;
        let closure = Closure {
            code: code.clone(),
            scope: Some(frame.scope),
            this_value: if flags.is_arrow { Some(frame.this.clone()) } else { None },
            new_target: if flags.is_arrow { Some(frame.new_target.clone()) } else { None },
            home_object: if flags.is_arrow { self.frame_home_object(frame) } else { None },
            fields: None,
            arguments: if flags.is_arrow { frame.extra.as_ref().and_then(|e| e.arguments_obj.clone()) } else { None },
        };
        let proto = if flags.is_generator && flags.is_async {
            self.intrinsics.others.get("AsyncGeneratorFunctionPrototype").copied().unwrap_or(self.intrinsics.function_proto)
        } else if flags.is_generator {
            self.intrinsics.others.get("GeneratorFunctionPrototype").copied().unwrap_or(self.intrinsics.function_proto)
        } else if flags.is_async {
            self.intrinsics.others.get("AsyncFunctionPrototype").copied().unwrap_or(self.intrinsics.function_proto)
        } else {
            self.intrinsics.function_proto
        };
        let length = Property::data(Value::Number(code.param_count as f64), CONFIGURABLE);
        let name = Property::data(Value::String(code.name.clone()), CONFIGURABLE);
        if flags.is_generator {
            let id = self.alloc(Object::new(Some(proto), ObjectKind::Function(Box::new(closure))));
            self.define_property(id, names::length(), length.value, CONFIGURABLE);
            self.define_property(id, names::name(), name.value, CONFIGURABLE);
            let gp = if flags.is_async { self.intrinsics.async_generator_proto } else { self.intrinsics.generator_proto };
            let p = self.alloc(Object::new(Some(gp), ObjectKind::Ordinary));
            self.define_property(id, "prototype", Value::Object(p), WRITABLE);
            id
        } else if !flags.is_arrow && !flags.is_method && !flags.is_async && !flags.is_class_constructor {
            // Layouts known in advance (the same shapes the property
            // definitions would reach): no transitions per closure.
            let shapes = self.closure_shapes();
            let mut obj = Object::new(Some(proto), ObjectKind::Function(Box::new(closure)));
            let p = self.new_object();
            obj.props = PropertyMap::with_shape(shapes.1, &[length, name, Property::data(Value::Object(p), WRITABLE)]);
            let id = self.alloc(obj);
            self.obj_mut(p).props = PropertyMap::with_shape(shapes.2, &[Property::data(Value::Object(id), HIDDEN)]);
            id
        } else {
            let shapes = self.closure_shapes();
            let mut obj = Object::new(Some(proto), ObjectKind::Function(Box::new(closure)));
            obj.props = PropertyMap::with_shape(shapes.0, &[length, name]);
            self.alloc(obj)
        }
    }

    /// Shapes of closures: (length, name), (length, name, prototype) and
    /// their prototype objects' (constructor).
    fn closure_shapes(&mut self) -> (Rc<crate::shape::Shape>, Rc<crate::shape::Shape>, Rc<crate::shape::Shape>) {
        if let Some(s) = &self.closure_shapes {
            return s.clone();
        }
        let root = crate::shape::Shape::root();
        let ln = root.add(&names::length()).add(&names::name());
        let lnp = ln.add(&names::prototype());
        let c = root.add(&names::constructor());
        self.closure_shapes = Some((ln.clone(), lnp.clone(), c.clone()));
        (ln, lnp, c)
    }

    fn make_class(&mut self, fi: usize, code: Rc<FunctionCode>, superclass: Value) -> JsResult<(ObjectId, ObjectId)> {
        let (proto_parent, ctor_parent) = match superclass.v() {
            V::Empty => (Some(self.intrinsics.object_proto), self.intrinsics.function_proto),
            V::Null => (None, self.intrinsics.function_proto),
            V::Object(s) if self.is_constructor(&superclass) => {
                let pp = self.get(superclass.clone(), names::prototype())?;
                let pp = match pp.v() {
                    V::Object(p) => Some(p),
                    V::Null => None,
                    _ => return self.type_error("Class extends value does not have valid prototype property"),
                };
                (pp, s)
            }
            _ => {
                let d = self.describe_for_error(&superclass);
                return self.type_error(&format!("Class extends value {} is not a constructor or null", d));
            }
        };
        let ctor = self.make_closure(fi, code);
        self.obj_mut(ctor).set_proto(Some(ctor_parent));
        let proto = self.alloc(Object::new(proto_parent, ObjectKind::Ordinary));
        self.define_property(proto, "constructor", Value::Object(ctor), HIDDEN);
        self.define_property(ctor, "prototype", Value::Object(proto), 0);
        if let ObjectKind::Function(c) = &mut self.obj_mut(ctor).kind {
            c.home_object = Some(proto);
        }
        Ok((ctor, proto))
    }

    /// Is own property `k` of `c` enumerable (without cloning it)?
    fn own_enumerable(&mut self, c: ObjectId, k: &PropertyKey) -> JsResult<bool> {
        let obj = self.obj(c);
        match &obj.kind {
            ObjectKind::Proxy { .. } | ObjectKind::Array { .. } | ObjectKind::String(_) | ObjectKind::TypedArray(_) => {
                Ok(self.get_own_property_any(c, k)?.map(|p| p.enumerable()).unwrap_or(false))
            }
            _ => Ok(obj.props.get(k).map(|p| p.enumerable()).unwrap_or(false)),
        }
    }

    /// Enumerable string keys of a for-in over `id` when nothing exotic is
    /// involved: an ordinary object or array whose prototypes contribute no
    /// enumerable string keys (the common case). Returns the keys and the
    /// receiver's shape id.
    fn for_in_fast_keys(&self, id: ObjectId) -> Option<(Vec<JsStr>, u32)> {
        let obj = self.heap.try_get(id)?;
        let elements = match &obj.kind {
            ObjectKind::Ordinary | ObjectKind::Function(_) | ObjectKind::Native(_) | ObjectKind::Error | ObjectKind::Arguments | ObjectKind::MappedArguments(_) => None,
            ObjectKind::Array { elements, length } if *length as usize == elements.len() => Some(elements),
            _ => return None,
        };
        if obj.props.is_dict() {
            return None;
        }
        let mut cur = obj.proto;
        let mut depth = 0;
        while let Some(p) = cur {
            let po = self.heap.try_get(p)?;
            match &po.kind {
                ObjectKind::Ordinary | ObjectKind::Function(_) | ObjectKind::Native(_) | ObjectKind::Error => {}
                ObjectKind::Array { elements, .. } if elements.is_empty() => {}
                _ => return None,
            }
            let mut enumerable = false;
            po.props.for_each_value(|pr| enumerable |= pr.flags & ENUMERABLE != 0);
            if enumerable {
                // Could be a symbol-keyed property: let the general path decide.
                return None;
            }
            cur = po.proto;
            depth += 1;
            if depth > 64 {
                return None;
            }
        }
        let mut keys = Vec::with_capacity(obj.props.len() + elements.map_or(0, |e| e.len()));
        if let Some(el) = elements {
            for (i, v) in el.iter().enumerate() {
                if !v.is_empty() {
                    keys.push(JsStr::from_index(i as u32));
                }
            }
        }
        let mut ints: Vec<(u32, JsStr)> = Vec::new();
        for (k, p) in obj.props.iter() {
            if p.flags & ENUMERABLE == 0 {
                continue;
            }
            if let PropertyKey::String(s) = k {
                match s.as_array_index() {
                    Some(i) => ints.push((i, s.clone())),
                    None => keys.push(s.clone()),
                }
            }
        }
        if !ints.is_empty() {
            if elements.is_some() {
                return None;
            }
            ints.sort_by_key(|(i, _)| *i);
            let mut all: Vec<JsStr> = ints.into_iter().map(|(_, s)| s).collect();
            all.extend(keys);
            keys = all;
        }
        Some((keys, obj.props.shape_id()))
    }

    fn for_in_start(&mut self, v: Value) -> JsResult<ObjectId> {
        let mut keys: Vec<JsStr> = Vec::new();
        let object = match v.v() {
            V::Undefined | V::Null | V::Empty => None,
            V::Object(id) => Some(id),
            prim => Some(self.to_object(prim.to_value())?),
        };
        if let Some((keys, shape)) = object.and_then(|id| self.for_in_fast_keys(id)) {
            let proto = self.intrinsics.object_proto;
            return Ok(self.alloc(Object::new(Some(proto), ObjectKind::ForInIterator(Box::new(crate::heap::ForInState { keys, index: 0, object, shape })))));
        }
        if let Some(id) = object {
            // Names seen on objects already visited (shadowing, even by
            // non-enumerable properties). Only materialised once a
            // prototype contributes enumerable names, which is rare.
            let mut seen: Option<std::collections::HashSet<JsStr, FxBuild>> = None;
            let mut visited: Vec<Vec<PropertyKey>> = Vec::new();
            let mut cur = Some(id);
            let mut first = true;
            while let Some(c) = cur {
                let own = self.own_keys(c)?;
                let mut en = Vec::with_capacity(own.len());
                let mut any = false;
                for k in &own {
                    let e = match k {
                        PropertyKey::Symbol(_) => false,
                        PropertyKey::String(_) => self.own_enumerable(c, k)?,
                    };
                    any |= e;
                    en.push(e);
                }
                if first {
                    for (k, e) in own.iter().zip(&en) {
                        if *e {
                            if let PropertyKey::String(s) = k {
                                keys.push(s.clone());
                            }
                        }
                    }
                } else if any {
                    let set = seen.get_or_insert_with(|| {
                        let mut set = std::collections::HashSet::default();
                        for ks in visited.drain(..) {
                            for k in ks {
                                if let PropertyKey::String(s) = k {
                                    set.insert(s);
                                }
                            }
                        }
                        set
                    });
                    for (k, e) in own.iter().zip(&en) {
                        if let PropertyKey::String(s) = k {
                            if set.insert(s.clone()) && *e {
                                keys.push(s.clone());
                            }
                        }
                    }
                }
                if !any || first {
                    match &mut seen {
                        Some(set) => {
                            for k in own {
                                if let PropertyKey::String(s) = k {
                                    set.insert(s);
                                }
                            }
                        }
                        None => visited.push(own),
                    }
                }
                first = false;
                cur = self.get_prototype_of(c)?;
            }
        }
        let proto = self.intrinsics.object_proto;
        Ok(self.alloc(Object::new(Some(proto), ObjectKind::ForInIterator(Box::new(crate::heap::ForInState { keys, index: 0, object, shape: 0 })))))
    }

    fn for_in_next(&mut self, it: ObjectId) -> JsResult<Option<JsStr>> {
        loop {
            let (key, object, shape) = match &mut self.obj_mut(it).kind {
                ObjectKind::ForInIterator(f) => {
                    if f.index >= f.keys.len() {
                        return Ok(None);
                    }
                    let k = f.keys[f.index].clone();
                    f.index += 1;
                    (k, f.object, f.shape)
                }
                _ => return Ok(None),
            };
            // Skip keys deleted during iteration (an unchanged shape proves
            // none was; arrays may shrink, so their elements are checked).
            if let Some(o) = object {
                if shape != 0 {
                    if let Some(obj) = self.heap.try_get(o) {
                        if obj.props.shape_id() == shape && !obj.is_array() {
                            return Ok(Some(key));
                        }
                    }
                }
                if !self.has_property(o, &PropertyKey::String(key.clone()))? {
                    continue;
                }
            }
            return Ok(Some(key));
        }
    }

    fn add_slow(&mut self, a: Value, b: Value) -> JsResult<Value> {
        let pa = self.to_primitive(a, Hint::Default)?;
        let pb = self.to_primitive(b, Hint::Default)?;
        if matches!(pa.v(), V::String(_)) || matches!(pb.v(), V::String(_)) {
            let sa = self.to_string(pa)?;
            let sb = self.to_string(pb)?;
            return self.concat(&sa, &sb);
        }
        let na = self.to_numeric(pa)?;
        let nb = self.to_numeric(pb)?;
        match (na.v(), nb.v()) {
            (V::Number(x), V::Number(y)) => Ok(Value::Number(x + y)),
            (V::BigInt(x), V::BigInt(y)) => Ok(Value::BigInt(Rc::new(x.add(&y)))),
            _ => self.type_error("Cannot mix BigInt and other types, use explicit conversions"),
        }
    }

    fn numeric_binop(&mut self, op: Op, a: Value, b: Value) -> JsResult<Value> {
        let na = self.to_numeric(a)?;
        let nb = self.to_numeric(b)?;
        match (na.v(), nb.v()) {
            (V::Number(x), V::Number(y)) => Ok(Value::Number(num_binop(op, x, y))),
            (V::BigInt(x), V::BigInt(y)) => {
                let shift_amount = |y: &BigInt| -> i64 { y.to_i128().map(|v| v.clamp(-(1 << 40), 1 << 40) as i64).unwrap_or(if y.is_negative() { -(1 << 40) } else { 1 << 40 }) };
                let r = match op {
                    Op::Sub => x.sub(&y),
                    Op::Mul => x.mul(&y),
                    Op::Div => match x.div(&y) {
                        Some(v) => v,
                        None => return self.range_error("Division by zero"),
                    },
                    Op::Mod => match x.rem(&y) {
                        Some(v) => v,
                        None => return self.range_error("Division by zero"),
                    },
                    Op::Exp => {
                        if y.is_negative() {
                            return self.range_error("Exponent must be non-negative");
                        }
                        match x.pow(&y) {
                            Some(v) => v,
                            None => return self.range_error("Maximum BigInt size exceeded"),
                        }
                    }
                    Op::BitAnd => x.bitand(&y),
                    Op::BitOr => x.bitor(&y),
                    Op::BitXor => x.bitxor(&y),
                    Op::Shl => {
                        let n = shift_amount(&y);
                        if n > (1 << 30) {
                            return self.range_error("Maximum BigInt size exceeded");
                        }
                        x.shl(n)
                    }
                    Op::Shr => x.shr(shift_amount(&y)),
                    Op::UShr => return self.type_error("BigInts have no unsigned right shift, use >> instead"),
                    _ => return self.type_error("Unsupported BigInt operation"),
                };
                Ok(Value::BigInt(Rc::new(r)))
            }
            _ => self.type_error("Cannot mix BigInt and other types, use explicit conversions"),
        }
    }

    fn compare_slow(&mut self, op: Op, a: Value, b: Value) -> JsResult<bool> {
        let pa = self.to_primitive(a, Hint::Number)?;
        let pb = self.to_primitive(b, Hint::Number)?;
        if let (V::String(x), V::String(y)) = (pa.v(), pb.v()) {
            let o = x.cmp_units(y);
            return Ok(match op {
                Op::Lt => o.is_lt(),
                Op::LtEq => o.is_le(),
                Op::Gt => o.is_gt(),
                _ => o.is_ge(),
            });
        }
        let na = self.to_numeric(pa)?;
        let nb = self.to_numeric(pb)?;
        let ord = match (na.v(), nb.v()) {
            (V::BigInt(x), V::BigInt(y)) => Some(x.cmp(y)),
            (V::BigInt(x), V::Number(y)) => x.cmp_f64(y),
            (V::Number(x), V::BigInt(y)) => y.cmp_f64(x).map(|o| o.reverse()),
            _ => None,
        };
        if let (V::BigInt(_), _) | (_, V::BigInt(_)) = (na.v(), nb.v()) {
            let Some(o) = ord else { return Ok(false) };
            return Ok(match op {
                Op::Lt => o.is_lt(),
                Op::LtEq => o.is_le(),
                Op::Gt => o.is_gt(),
                _ => o.is_ge(),
            });
        }
        let (x, y) = match (na.v(), nb.v()) {
            (V::Number(x), V::Number(y)) => (x, y),
            _ => unreachable!(),
        };
        if x.is_nan() || y.is_nan() {
            return Ok(false);
        }
        Ok(match op {
            Op::Lt => x < y,
            Op::LtEq => x <= y,
            Op::Gt => x > y,
            _ => x >= y,
        })
    }
}

/// `num_binop` of `BIN_OPS[kd]`, dispatched on the index (inlined into the
/// interpreter loop; the bitwise operators dominate hash / codec loops).
#[inline(always)]
fn bin_by_index(kd: u8, x: f64, y: f64) -> f64 {
    match kd {
        5 => (to_int32(x) & to_int32(y)) as f64,
        6 => (to_int32(x) | to_int32(y)) as f64,
        7 => (to_int32(x) ^ to_int32(y)) as f64,
        8 => to_int32(x).wrapping_shl(to_uint32(y) & 31) as f64,
        9 => (to_int32(x) >> (to_uint32(y) & 31)) as f64,
        10 => (to_uint32(x) >> (to_uint32(y) & 31)) as f64,
        _ => num_binop(crate::bytecode::BIN_OPS[kd as usize], x, y),
    }
}

fn num_binop(op: Op, x: f64, y: f64) -> f64 {
    match op {
        Op::Sub => x - y,
        Op::Mul => x * y,
        Op::Div => x / y,
        Op::Mod => js_mod(x, y),
        Op::Exp => js_pow(x, y),
        Op::BitAnd => (to_int32(x) & to_int32(y)) as f64,
        Op::BitOr => (to_int32(x) | to_int32(y)) as f64,
        Op::BitXor => (to_int32(x) ^ to_int32(y)) as f64,
        Op::Shl => to_int32(x).wrapping_shl(to_uint32(y) & 31) as f64,
        Op::Shr => (to_int32(x) >> (to_uint32(y) & 31)) as f64,
        Op::UShr => (to_uint32(x) >> (to_uint32(y) & 31)) as f64,
        _ => f64::NAN,
    }
}

/// `String.prototype.charCodeAt` (see `NativeFunction::intrinsic`).
pub const INTRINSIC_CHAR_CODE_AT: u8 = 1;
/// `String.fromCharCode` with one argument.
pub const INTRINSIC_FROM_CHAR_CODE: u8 = 2;
/// `Math.imul`.
pub const INTRINSIC_IMUL: u8 = 3;
/// `Function.prototype.call` (see [`Realm::try_call_indirect`]).
pub const INTRINSIC_FN_CALL: u8 = 4;
/// `Function.prototype.apply`.
pub const INTRINSIC_FN_APPLY: u8 = 5;

/// Instructions (backward jumps and calls) between GC / watchdog checks.
pub(crate) const TICK_INTERVAL: u32 = 1024;

#[cold]
#[inline(never)]
fn stack_underflow() -> ! {
    panic!("operand stack underflow")
}

/// `x % y` with an exact integer fast path.
#[inline(always)]
pub fn js_mod(x: f64, y: f64) -> f64 {
    let (xi, yi) = (x as i32, y as i32);
    if yi > 0 && xi as f64 == x && yi as f64 == y && !(x == 0.0 && x.is_sign_negative()) {
        let r = xi % yi;
        return if r == 0 && xi < 0 { -0.0 } else { r as f64 };
    }
    if y.is_infinite() && x.is_finite() {
        x
    } else {
        x % y
    }
}

pub fn js_pow(x: f64, y: f64) -> f64 {
    if y.is_nan() {
        return f64::NAN;
    }
    if y == 0.0 {
        return 1.0;
    }
    if (x == 1.0 || x == -1.0) && y.is_infinite() {
        return f64::NAN;
    }
    x.powf(y)
}

/// Promise reaction used by `await`: resume the generator with the value.
fn on_await_fulfilled(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let gen = realm.current_native().and_then(|f| realm.native_captures(f).first().cloned());
    if let Some(V::Object(g)) = gen.as_ref().map(Value::v) {
        let v = args.first().cloned().unwrap_or(Value::Undefined);
        realm.async_step_dispatch(g, ResumeMode::Next, v);
    }
    Ok(Value::Undefined)
}

fn on_await_rejected(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let gen = realm.current_native().and_then(|f| realm.native_captures(f).first().cloned());
    if let Some(V::Object(g)) = gen.as_ref().map(Value::v) {
        let v = args.first().cloned().unwrap_or(Value::Undefined);
        realm.async_step_dispatch(g, ResumeMode::Throw, v);
    }
    Ok(Value::Undefined)
}
