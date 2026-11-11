//! The bytecode interpreter: frames, calls, generators, async and exceptions.

use crate::bytecode::{FunctionCode, Op, VarLoc};
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
    pub code: Rc<FunctionCode>,
    pub pc: usize,
    pub base: u32,
    pub argc: u32,
    pub scope: ObjectId,
    pub scope_depth: u32,
    pub this: Value,
    pub func: Option<ObjectId>,
    pub new_target: Value,
    pub home_object: Option<ObjectId>,
    /// Caller slot receiving this frame's return value.
    pub ret_slot: u32,
    pub is_construct: bool,
    /// Returning from this frame exits the run loop.
    pub entry: bool,
    /// State only some frames need (boxed to keep calls cheap).
    pub extra: Option<Box<FrameExtra>>,
}

/// The rarely used parts of a frame.
#[derive(Default)]
pub struct FrameExtra {
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
    /// First `vstack` index used by the frame (its kept arguments).
    #[inline]
    pub fn bottom(&self) -> usize {
        self.base as usize - self.argc as usize
    }

    pub fn generator(&self) -> Option<ObjectId> {
        self.extra.as_ref().and_then(|e| e.generator)
    }

    fn extra_mut(&mut self) -> &mut FrameExtra {
        self.extra.get_or_insert_with(Default::default)
    }
}

impl Default for Value {
    fn default() -> Value {
        Value::Undefined
    }
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Frame({} pc={})", self.code.name, self.pc)
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
                        if let (Some(src), Some((a, b))) = (&f.code.source, f.code.source_span) {
                            let (a, b) = (a as usize, (b as usize).min(a as usize + 400));
                            eprintln!("[notfn] top function source: {}", src.get(a..b).unwrap_or("?"));
                        }
                        eprintln!("[notfn] pc={} op={:?}", f.pc, f.code.ops.get(f.pc.saturating_sub(1)));
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
                    return self.type_error(&format!("Class constructor {} cannot be invoked without 'new'", n));
                }
                if code.flags.is_generator || code.flags.is_async {
                    return self.start_generator_like(id, this, args);
                }
                if !self.push_plain_call(id, &this, args, 0, true)? {
                    let frame = self.prepare_frame(id, this, args, Value::Undefined, false)?;
                    self.push_frame(frame, true)?;
                }
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
        let saved = self.current_callee.replace(id);
        if !construct {
            self.new_target_stack.push(Value::Undefined);
        }
        let r = func(self, this, args);
        if crate::realm::take_post_native() {
            if let Some(h) = self.post_native {
                h(self);
            }
        }
        if !construct {
            self.new_target_stack.pop();
        }
        self.current_callee = saved;
        self.native_depth -= 1;
        r
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
            code,
            pc: 0,
            base: base as u32,
            argc: 0,
            scope,
            scope_depth: 0,
            this,
            func: None,
            new_target: Value::Undefined,
            home_object: None,
            ret_slot: 0,
            is_construct: false,
            entry: true,
            extra: Some(Box::default()),
        };
        self.push_frame(frame, true)?;
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
    pub(crate) fn try_fast_call(&mut self, fi: usize, b: usize, argc: usize) -> JsResult<bool> {
        let cb = self.frames[fi].base as usize + b;
        let id = match self.vstack[cb].v() {
            V::Object(id) => id,
            _ => return Ok(false),
        };
        let Some(obj) = self.heap.try_get(id) else { return Ok(false) };
        let ObjectKind::Function(c) = &obj.kind else { return Ok(false) };
        if c.code.lazy.is_some() {
            self.ensure_compiled(id)?;
            return self.try_fast_call(fi, b, argc);
        }
        let flags = c.code.flags;
        if flags.is_generator || flags.is_async || flags.is_class_constructor || c.code.arguments_loc.is_some() || c.code.rest_loc.is_some() || (flags.is_arrow && c.arguments.is_some()) {
            return Ok(false);
        }
        let this = if flags.is_arrow {
            c.this_value.clone().unwrap_or(Value::Undefined)
        } else if flags.is_strict {
            self.vstack[cb + 1].undef_if_empty()
        } else {
            match self.vstack[cb + 1].v() {
                V::Undefined | V::Null | V::Empty => Value::Object(self.global),
                V::Object(o) => Value::Object(o),
                _ => return Ok(false),
            }
        };
        if self.frames.len() >= self.max_frames {
            return self.range_error("Maximum call stack size exceeded");
        }
        let code = c.code.clone();
        let parent_scope = c.scope;
        let home = c.home_object;
        let new_target = if flags.is_arrow { c.new_target.clone().unwrap_or(Value::Undefined) } else { Value::Undefined };
        let nslots = code.scope_slots as usize;
        let mut slots = if nslots == 0 { Vec::new() } else { vec![Value::Undefined; nslots] };
        let kept = if flags.args_direct { argc } else { 0 };
        let bottom = self.vstack.len();
        if kept > 0 {
            self.vstack.extend_from_within(cb + 2..cb + 2 + argc);
        }
        let base = self.reserve_slots(code.frame_size());
        for (i, &loc) in code.param_locs.iter().enumerate() {
            let v = if i < argc { std::mem::replace(&mut self.vstack[cb + 2 + i], Value::Undefined) } else { Value::Undefined };
            match loc {
                VarLoc::Reg(r) => self.vstack[base + r as usize] = v,
                VarLoc::Slot(s) => slots[s as usize] = v,
            }
        }
        if let Some(sl) = code.self_loc {
            match sl {
                VarLoc::Reg(r) => self.vstack[base + r as usize] = Value::Object(id),
                VarLoc::Slot(s) => slots[s as usize] = Value::Object(id),
            }
        }
        let scope = if nslots > 0 {
            self.alloc(Object::new(None, ObjectKind::Scope(Box::new(ScopeData { slots, parent: parent_scope, names: None }))))
        } else {
            match parent_scope {
                Some(p) => p,
                None => self.alloc(Object::new(None, ObjectKind::Scope(Box::new(ScopeData { slots, parent: None, names: None })))),
            }
        };
        debug_assert_eq!(base - bottom, kept);
        self.frames.push(Frame {
            code,
            pc: 0,
            base: base as u32,
            argc: kept as u32,
            scope,
            scope_depth: 0,
            this,
            func: Some(id),
            new_target,
            home_object: home,
            ret_slot: b as u32,
            is_construct: false,
            entry: false,
            extra: None,
        });
        Ok(true)
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

    /// Push the frame for calling plain closure `id` (no `arguments`
    /// object, rest parameter, generator / async or class constructor) with
    /// arguments from a slice, e.g. a callback called by a native function.
    /// Returns false (nothing done) when the general path is needed.
    pub(crate) fn push_plain_call(&mut self, id: ObjectId, this: &Value, args: &[Value], ret_slot: u32, entry: bool) -> JsResult<bool> {
        let Some(obj) = self.heap.try_get(id) else { return Ok(false) };
        let ObjectKind::Function(c) = &obj.kind else { return Ok(false) };
        if c.code.lazy.is_some() {
            self.ensure_compiled(id)?;
            return self.push_plain_call(id, this, args, ret_slot, entry);
        }
        let flags = c.code.flags;
        if flags.is_generator || flags.is_async || flags.is_class_constructor || c.code.arguments_loc.is_some() || c.code.rest_loc.is_some() || (flags.is_arrow && c.arguments.is_some()) {
            return Ok(false);
        }
        let this = if flags.is_arrow {
            c.this_value.clone().unwrap_or(Value::Undefined)
        } else if flags.is_strict {
            this.undef_if_empty()
        } else {
            match this.v() {
                V::Undefined | V::Null | V::Empty => Value::Object(self.global),
                V::Object(o) => Value::Object(o),
                _ => return Ok(false),
            }
        };
        if self.frames.len() >= self.max_frames {
            return self.range_error("Maximum call stack size exceeded");
        }
        let code = c.code.clone();
        let parent_scope = c.scope;
        let home = c.home_object;
        let new_target = if flags.is_arrow { c.new_target.clone().unwrap_or(Value::Undefined) } else { Value::Undefined };
        let nslots = code.scope_slots as usize;
        let mut slots = if nslots == 0 { Vec::new() } else { vec![Value::Undefined; nslots] };
        let kept = if flags.args_direct { args.len() } else { 0 };
        if kept > 0 {
            self.vstack.extend_from_slice(args);
        }
        let base = self.reserve_slots(code.frame_size());
        for (i, &loc) in code.param_locs.iter().enumerate() {
            let Some(v) = args.get(i) else { break };
            match loc {
                VarLoc::Reg(r) => self.vstack[base + r as usize] = v.clone(),
                VarLoc::Slot(s) => slots[s as usize] = v.clone(),
            }
        }
        if let Some(sl) = code.self_loc {
            match sl {
                VarLoc::Reg(r) => self.vstack[base + r as usize] = Value::Object(id),
                VarLoc::Slot(s) => slots[s as usize] = Value::Object(id),
            }
        }
        let scope = if nslots > 0 {
            self.alloc(Object::new(None, ObjectKind::Scope(Box::new(ScopeData { slots, parent: parent_scope, names: None }))))
        } else {
            match parent_scope {
                Some(p) => p,
                None => self.alloc(Object::new(None, ObjectKind::Scope(Box::new(ScopeData { slots, parent: None, names: None })))),
            }
        };
        self.frames.push(Frame {
            code,
            pc: 0,
            base: base as u32,
            argc: kept as u32,
            scope,
            scope_depth: 0,
            this,
            func: Some(id),
            new_target,
            home_object: home,
            ret_slot,
            is_construct: false,
            entry,
            extra: None,
        });
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
        if c.code.lazy.is_some() {
            self.ensure_compiled(id)?;
            return self.try_fast_construct(fi, b, argc);
        }
        let flags = c.code.flags;
        if flags.is_arrow
            || flags.is_method
            || flags.is_generator
            || flags.is_async
            || flags.is_derived
            || flags.is_class_constructor
            || c.code.arguments_loc.is_some()
            || c.code.rest_loc.is_some()
        {
            return Ok(false);
        }
        let proto = match obj.props.get(&names::prototype()) {
            Some(p) if p.flags & ACCESSOR == 0 => match p.value.v() {
                V::Object(p) => p,
                _ => self.intrinsics.object_proto,
            },
            _ => return Ok(false),
        };
        if self.frames.len() >= self.max_frames {
            return self.range_error("Maximum call stack size exceeded");
        }
        let code = c.code.clone();
        let parent_scope = c.scope;
        let home = c.home_object;
        let nslots = code.scope_slots as usize;
        let this = Value::Object(self.alloc(Object::new(Some(proto), ObjectKind::Ordinary)));
        let mut slots = if nslots == 0 { Vec::new() } else { vec![Value::Undefined; nslots] };
        let kept = if flags.args_direct { argc } else { 0 };
        if kept > 0 {
            self.vstack.extend_from_within(cb + 1..cb + 1 + argc);
        }
        let base = self.reserve_slots(code.frame_size());
        for (i, &loc) in code.param_locs.iter().enumerate() {
            let v = if i < argc { std::mem::replace(&mut self.vstack[cb + 1 + i], Value::Undefined) } else { Value::Undefined };
            match loc {
                VarLoc::Reg(r) => self.vstack[base + r as usize] = v,
                VarLoc::Slot(s) => slots[s as usize] = v,
            }
        }
        if let Some(sl) = code.self_loc {
            match sl {
                VarLoc::Reg(r) => self.vstack[base + r as usize] = Value::Object(id),
                VarLoc::Slot(s) => slots[s as usize] = Value::Object(id),
            }
        }
        let scope = if nslots > 0 {
            self.alloc(Object::new(None, ObjectKind::Scope(Box::new(ScopeData { slots, parent: parent_scope, names: None }))))
        } else {
            match parent_scope {
                Some(p) => p,
                None => self.alloc(Object::new(None, ObjectKind::Scope(Box::new(ScopeData { slots, parent: None, names: None })))),
            }
        };
        self.frames.push(Frame {
            code,
            pc: 0,
            base: base as u32,
            argc: kept as u32,
            scope,
            scope_depth: 0,
            this,
            func: Some(id),
            new_target: Value::Object(id),
            home_object: home,
            ret_slot: b as u32,
            is_construct: true,
            entry: false,
            extra: None,
        });
        Ok(true)
    }

    /// Build the frame for calling closure `id`; its slots are reserved on
    /// top of the value stack (push it next, or detach them).
    pub(crate) fn prepare_frame(&mut self, id: ObjectId, this: Value, args: &[Value], new_target: Value, is_construct: bool) -> JsResult<Frame> {
        self.ensure_compiled(id)?;
        let (code, parent_scope, this_value, arrow_nt, home, arrow_args) = match &self.obj(id).kind {
            ObjectKind::Function(c) => (c.code.clone(), c.scope, c.this_value.clone(), c.new_target.clone(), c.home_object, c.arguments.clone()),
            _ => unreachable!(),
        };
        let flags = code.flags;
        // this binding
        let this = if flags.is_arrow {
            this_value.unwrap_or(Value::Undefined)
        } else if is_construct {
            if flags.is_derived {
                Value::Empty
            } else {
                let proto = match (self.get(new_target.clone(), names::prototype())?).v() {
                    V::Object(p) => p,
                    _ => self.intrinsics.object_proto,
                };
                Value::Object(self.alloc(Object::new(Some(proto), ObjectKind::Ordinary)))
            }
        } else if flags.is_strict {
            this
        } else {
            match this.v() {
                V::Undefined | V::Null | V::Empty => Value::Object(self.global),
                V::Object(_) => this,
                prim => Value::Object(self.to_object(prim.to_value())?),
            }
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
            arguments_obj = Some(self.create_arguments_object(args, if flags.is_strict { None } else { Some(id) }));
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
        Ok(Frame {
            code,
            pc: 0,
            base: base as u32,
            argc: kept as u32,
            scope,
            scope_depth: 0,
            this,
            func: Some(id),
            new_target,
            home_object: home,
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
        // Suspended at its start: the slots wait inside the frame.
        self.detach_slots(&mut frame);
        let is_async = flags.is_async;
        let is_gen = flags.is_generator;
        let proto = if is_gen {
            // generator object prototype = fn.prototype (if object) else intrinsic
            match (self.get(Value::Object(id), names::prototype())?).v() {
                V::Object(p) => p,
                _ => {
                    if is_async {
                        self.intrinsics.async_generator_proto
                    } else {
                        self.intrinsics.generator_proto
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
        if state == GeneratorState::SuspendedYield {
            match mode {
                ResumeMode::Next => {
                    let slot = frame.base as usize + frame.extra.as_ref().map_or(0, |e| e.resume_slot as usize);
                    self.vstack[slot] = value.clone();
                }
                ResumeMode::Return => {
                    // Finally blocks are not run (inline-finally compilation).
                    self.vstack.truncate(frame.bottom());
                    set_state(self, GeneratorState::Completed, None);
                    return Ok(Step::Return(value));
                }
                ResumeMode::Throw => {}
            }
        }
        if let Err(e) = self.push_frame(frame, true) {
            set_state(self, GeneratorState::Completed, None);
            return Err(e);
        }
        let result = if mode == ResumeMode::Throw {
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
            let handler = if self.terminating { None } else { frame.code.handlers.iter().find(|h| h.start <= pc && pc < h.end).copied() };
            if let Some(h) = handler {
                let slot = frame.code.nregs as usize + h.stack_depth as usize;
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
        loop {
            match self.run_inner() {
                Ok(exit) => return Ok(exit),
                Err(e) => self.unwind(e)?,
            }
        }
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

    /// GC / watchdog safe point, reached every few hundred backward jumps
    /// and calls.
    #[inline(never)]
    fn safepoint(&mut self) -> JsResult<()> {
        self.maybe_gc();
        if crate::sampler::js::enabled() && crate::sampler::js::take_pending() {
            let frames: Vec<String> = self
                .frames
                .iter()
                .rev()
                .take(12)
                .map(|f| format!("{}@{}:{}", f.code.name, f.code.filename, f.code.positions.first().map(|p| p.line).unwrap_or(0)))
                .collect();
            crate::sampler::js::sample(&frames);
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

    /// The interpreter loop. Register instructions address frame slots
    /// directly (registers, then the operand stack at static depths), so
    /// the loop keeps only the program counter and the slot base pointer
    /// in locals. Only fast paths live here; every miss, and every stack
    /// instruction, goes through [`Realm::exec_op`], which implements each
    /// instruction completely.
    #[allow(unused_unsafe)]
    fn run_inner(&mut self) -> JsResult<Exit> {
        'reload: loop {
            let fi = self.frames.len() - 1;
            // The frame's code, borrowed independently of `self` (no
            // reference count traffic per call / return).
            // SAFETY: frame `fi` holds a strong reference to its code for as
            // long as this loop runs it: every path that pops or replaces
            // the frame leaves through `continue 'reload` or a return.
            let code: &FunctionCode = unsafe { &*Rc::as_ptr(&self.frames[fi].code) };
            let ops: &[Op] = &code.ops;
            let consts: &[Value] = &code.constants;
            let mut pc = self.frames[fi].pc;
            // The frame's slots: the value stack only grows on calls (which
            // reload) and in out-of-line instructions (`slow!` reloads).
            let fbase = self.frames[fi].base as usize;
            debug_assert!(self.vstack.len() >= fbase + code.frame_size());
            // SAFETY: `fbase` is within the vector.
            let mut base: *mut Value = unsafe { self.vstack.as_mut_ptr().add(fbase) };
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
            macro_rules! sync {
                () => {
                    // SAFETY: `fi` is the last frame while this loop runs.
                    unsafe { self.frames.get_unchecked_mut(fi) }.pc = pc
                };
            }
            macro_rules! tri {
                ($e:expr) => {
                    match $e {
                        Ok(v) => v,
                        Err(e) => return Err(e),
                    }
                };
            }
            macro_rules! tick {
                () => {
                    self.tick = self.tick.wrapping_sub(1);
                    if self.tick == 0 {
                        self.tick = TICK_INTERVAL;
                        sync!();
                        tri!(self.safepoint());
                    }
                };
            }
            macro_rules! jump {
                ($t:expr) => {{
                    let t = $t as usize;
                    if t < pc {
                        pc = t;
                        tick!();
                    } else {
                        pc = t;
                    }
                }};
            }
            // Out-of-line execution of the current instruction.
            macro_rules! slow {
                ($op:expr) => {{
                    sync!();
                    match self.exec_op(fi, $op) {
                        Ok(Flow::Next) => {}
                        Ok(Flow::Reload) => continue 'reload,
                        Ok(Flow::Exit(e)) => return Ok(e),
                        Err(e) => return Err(e),
                    }
                    if self.frames.len() != fi + 1 {
                        continue 'reload;
                    }
                    // SAFETY: as in `sync`; the value stack may have moved.
                    pc = unsafe { self.frames.get_unchecked(fi) }.pc;
                    base = unsafe { self.vstack.as_mut_ptr().add(fbase) };
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
                let op = unsafe {
                    debug_assert!(pc < ops.len());
                    *ops.get_unchecked(pc)
                };
                pc += 1;
                match op {
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
                            let r = x.concat(y);
                            *sl!(d) = Value::String(r);
                            continue;
                        }
                        slow!(op);
                    }
                    Op::AddRK(d, a, c) => {
                        if let Some((x, y)) = num_k!(a, c) {
                            set_num!(d, x + y);
                            continue;
                        }
                        if let (V::String(x), V::String(y)) = (sl!(a).v(), k!(c).v()) {
                            let r = x.concat(y);
                            *sl!(d) = Value::String(r);
                            continue;
                        }
                        slow!(op);
                    }
                    Op::SubRR(d, a, b) => match num2!(a, b) {
                        Some((x, y)) => set_num!(d, x - y),
                        None => slow!(op),
                    },
                    Op::SubRK(d, a, c) => match num_k!(a, c) {
                        Some((x, y)) => set_num!(d, x - y),
                        None => slow!(op),
                    },
                    Op::MulRR(d, a, b) => match num2!(a, b) {
                        Some((x, y)) => set_num!(d, x * y),
                        None => slow!(op),
                    },
                    Op::MulRK(d, a, c) => match num_k!(a, c) {
                        Some((x, y)) => set_num!(d, x * y),
                        None => slow!(op),
                    },
                    Op::DivRR(d, a, b) => match num2!(a, b) {
                        Some((x, y)) => set_num!(d, x / y),
                        None => slow!(op),
                    },
                    Op::DivRK(d, a, c) => match num_k!(a, c) {
                        Some((x, y)) => set_num!(d, x / y),
                        None => slow!(op),
                    },
                    Op::ModRR(d, a, b) => match num2!(a, b) {
                        Some((x, y)) => set_num!(d, js_mod(x, y)),
                        None => slow!(op),
                    },
                    Op::ModRK(d, a, c) => match num_k!(a, c) {
                        Some((x, y)) => set_num!(d, js_mod(x, y)),
                        None => slow!(op),
                    },
                    Op::BinRR(kd, d, a, b) => match num2!(a, b) {
                        Some((x, y)) => set_num!(d, bin_by_index(kd, x, y)),
                        None => slow!(op),
                    },
                    Op::BinRK(kd, d, a, c) => match num_k!(a, c) {
                        Some((x, y)) => set_num!(d, bin_by_index(kd, x, y)),
                        None => slow!(op),
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
                                slow!(op);
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
                                slow!(op);
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
                        None => slow!(op),
                    },
                    Op::JumpUnlessLtRK(a, c, t) => match num_k!(a, c) {
                        Some((x, y)) => {
                            if !(x < y) {
                                jump!(t);
                            }
                        }
                        None => slow!(op),
                    },
                    Op::JumpUnlessLtEqRR(a, b, t) => match num2!(a, b) {
                        Some((x, y)) => {
                            if !(x <= y) {
                                jump!(t);
                            }
                        }
                        None => slow!(op),
                    },
                    Op::JumpUnlessLtEqRK(a, c, t) => match num_k!(a, c) {
                        Some((x, y)) => {
                            if !(x <= y) {
                                jump!(t);
                            }
                        }
                        None => slow!(op),
                    },
                    Op::JumpUnlessGtRR(a, b, t) => match num2!(a, b) {
                        Some((x, y)) => {
                            if !(x > y) {
                                jump!(t);
                            }
                        }
                        None => slow!(op),
                    },
                    Op::JumpUnlessGtRK(a, c, t) => match num_k!(a, c) {
                        Some((x, y)) => {
                            if !(x > y) {
                                jump!(t);
                            }
                        }
                        None => slow!(op),
                    },
                    Op::JumpUnlessGtEqRR(a, b, t) => match num2!(a, b) {
                        Some((x, y)) => {
                            if !(x >= y) {
                                jump!(t);
                            }
                        }
                        None => slow!(op),
                    },
                    Op::JumpUnlessGtEqRK(a, c, t) => match num_k!(a, c) {
                        Some((x, y)) => {
                            if !(x >= y) {
                                jump!(t);
                            }
                        }
                        None => slow!(op),
                    },
                    Op::JumpIfLtRR(a, b, t) => match num2!(a, b) {
                        Some((x, y)) => {
                            if x < y {
                                jump!(t);
                            }
                        }
                        None => slow!(op),
                    },
                    Op::JumpIfLtRK(a, c, t) => match num_k!(a, c) {
                        Some((x, y)) => {
                            if x < y {
                                jump!(t);
                            }
                        }
                        None => slow!(op),
                    },
                    Op::JumpIfLtEqRR(a, b, t) => match num2!(a, b) {
                        Some((x, y)) => {
                            if x <= y {
                                jump!(t);
                            }
                        }
                        None => slow!(op),
                    },
                    Op::JumpIfLtEqRK(a, c, t) => match num_k!(a, c) {
                        Some((x, y)) => {
                            if x <= y {
                                jump!(t);
                            }
                        }
                        None => slow!(op),
                    },
                    Op::JumpIfGtRR(a, b, t) => match num2!(a, b) {
                        Some((x, y)) => {
                            if x > y {
                                jump!(t);
                            }
                        }
                        None => slow!(op),
                    },
                    Op::JumpIfGtRK(a, c, t) => match num_k!(a, c) {
                        Some((x, y)) => {
                            if x > y {
                                jump!(t);
                            }
                        }
                        None => slow!(op),
                    },
                    Op::JumpIfGtEqRR(a, b, t) => match num2!(a, b) {
                        Some((x, y)) => {
                            if x >= y {
                                jump!(t);
                            }
                        }
                        None => slow!(op),
                    },
                    Op::JumpIfGtEqRK(a, c, t) => match num_k!(a, c) {
                        Some((x, y)) => {
                            if x >= y {
                                jump!(t);
                            }
                        }
                        None => slow!(op),
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
                        if sl!(s).truthy() {
                            jump!(t);
                        }
                    }
                    Op::JumpIfFalseR(s, t) => {
                        if !sl!(s).truthy() {
                            jump!(t);
                        }
                    }
                    Op::Jump(t) => jump!(t),
                    Op::NotR(d, s) => {
                        let r = !sl!(s).truthy();
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
                            slow!(Op::IncR(r, r));
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
                            slow!(Op::IncR(r, r));
                        }
                    }
                    Op::IncR(d, s) => match sl!(s).as_number() {
                        Some(n) => set_num!(d, n + 1.0),
                        None => slow!(op),
                    },
                    Op::DecR(d, s) => match sl!(s).as_number() {
                        Some(n) => set_num!(d, n - 1.0),
                        None => slow!(op),
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
                        _ => slow!(op),
                    },
                    Op::ToNumericR(d, s) => match sl!(s).v() {
                        V::Number(n) => set_num!(d, n),
                        _ => slow!(op),
                    },
                    Op::LoadThis(d) => {
                        // SAFETY: as in `sync`.
                        let v = (unsafe { self.frames.get_unchecked(fi) }.this).undef_if_empty();
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
                            let mut p = self.ic_get(&rec, recv);
                            if p.is_none() && recv.is_object() {
                                // Polymorphic site: the shared cache by (shape, key).
                                let kb = k!(code.ic_keys[ic as usize]).bits();
                                if let Some(m) = self.mega_get(recv, kb) {
                                    p = self.ic_get(&m, recv);
                                }
                            }
                            match p {
                                Some(p) if p.flags & ACCESSOR == 0 => Some(p.value.clone()),
                                Some(p) => {
                                    // Cached accessor: a native getter (DOM
                                    // attributes, `ta.length`, ...) is called
                                    // directly, out of line.
                                    if let Some(V::Object(g)) = p.getter().map(Value::v) {
                                        sync!();
                                        let r = tri!(self.call_cached_getter(g, o as usize, fi));
                                        // SAFETY: as in `slow`; the getter may have run
                                        // JavaScript (the value stack may have moved).
                                        base = unsafe { self.vstack.as_mut_ptr().add(fbase) };
                                        *sl!(d) = r;
                                        continue;
                                    }
                                    None
                                }
                                None => None,
                            }
                        };
                        match hit {
                            Some(v) => *sl!(d) = v,
                            None => slow!(op),
                        }
                    }
                    Op::SetPropR(o, v, ic) => {
                        let mut rec = code.ics[ic as usize].get();
                        let val = &*sl!(v);
                        if !val.is_empty() {
                            let key = match k!(code.ic_keys[ic as usize]).v() {
                                V::String(s) => s,
                                _ => unreachable!(),
                            };
                            for attempt in 0..2 {
                                if attempt == 1 {
                                    // Polymorphic site: the shared cache by (shape, key).
                                    match self.mega_get(sl!(o), key.bits() | 1) {
                                        Some(m) => rec = m,
                                        None => break,
                                    }
                                }
                                if rec.kind == crate::ic::IC_SETTER {
                                    if let Some(f) = self.ic_setter(&rec, sl!(o)) {
                                        let (recv, val) = (sl!(o).clone(), sl!(v).clone());
                                        sync!();
                                        tri!(self.call(Value::Object(f), recv, &[val]));
                                        if self.frames.len() != fi + 1 {
                                            continue 'reload;
                                        }
                                        // SAFETY: as in `slow`.
                                        base = unsafe { self.vstack.as_mut_ptr().add(fbase) };
                                        continue 'dispatch;
                                    }
                                } else if rec.kind != 0 {
                                    let val = sl!(v).clone();
                                    match self.ic_set(&rec, sl!(o), key, val) {
                                        Ok(()) => continue 'dispatch,
                                        Err(v) => drop(v),
                                    }
                                }
                            }
                        }
                        slow!(op);
                    }
                    Op::DefinePropR(o, v, ic) => {
                        let rec = code.ics[ic as usize].get();
                        let val = &*sl!(v);
                        if rec.kind == crate::ic::IC_ADD && !val.is_empty() {
                            let key = match k!(code.ic_keys[ic as usize]).v() {
                                V::String(s) => s,
                                _ => unreachable!(),
                            };
                            let val = val.clone();
                            if let Err(v) = self.ic_set(&rec, sl!(o), key, val) {
                                drop(v);
                                slow!(op);
                            }
                            continue;
                        }
                        slow!(op);
                    }
                    Op::GetGlobalR(d, ic) => {
                        let rec = code.ics[ic as usize].get();
                        let key = match k!(code.ic_keys[ic as usize]).v() {
                            V::String(s) => s,
                            _ => unreachable!(),
                        };
                        match self.ic_get_global(&rec, key) {
                            Some(v) => {
                                let v = v.clone();
                                *sl!(d) = v;
                            }
                            None => slow!(op),
                        }
                    }
                    Op::SetGlobalR(s, ic) => {
                        let rec = code.ics[ic as usize].get();
                        let val = &*sl!(s);
                        if !val.is_empty() {
                            let key = match k!(code.ic_keys[ic as usize]).v() {
                                V::String(s) => s,
                                _ => unreachable!(),
                            };
                            let val = val.clone();
                            match self.ic_set_global(&rec, key, val) {
                                Ok(()) => continue,
                                Err(v) => drop(v),
                            }
                        }
                        slow!(op);
                    }
                    Op::GetElemR(d, o, kk) => match self.get_elem_fast(sl!(o), sl!(kk)) {
                        Some(v) => *sl!(d) = v,
                        None => slow!(op),
                    },
                    Op::SetElemR(o, kk, v) => {
                        let val = sl!(v).clone();
                        if !val.is_empty() {
                            match self.set_elem_fast(sl!(o), sl!(kk), val) {
                                Ok(()) => continue,
                                Err(v) => drop(v),
                            }
                        }
                        slow!(op);
                    }
                    Op::GetLocalR(d, dp, s) => {
                        // SAFETY: as in `sync`.
                        let scope = unsafe { self.frames.get_unchecked(fi) }.scope;
                        let v = self.slot_get(scope, dp, s);
                        *sl!(d) = if v.is_empty() { Value::Undefined } else { v };
                    }
                    Op::SetLocalR(v, dp, s) => {
                        let val = sl!(v).undef_if_empty();
                        // SAFETY: as in `sync`.
                        let scope = unsafe { self.frames.get_unchecked(fi) }.scope;
                        self.slot_set(scope, dp, s, val);
                    }
                    Op::StoreCompletionR(s) => {
                        let v = sl!(s).clone();
                        // SAFETY: as in `sync`.
                        unsafe { self.frames.get_unchecked_mut(fi) }.extra_mut().completion = v;
                    }
                    Op::CallR(b, argc, _) => {
                        tick!();
                        sync!();
                        if tri!(self.try_fast_call(fi, b as usize, argc as usize)) {
                            continue 'reload;
                        }
                        if tri!(self.try_call_indirect(fi, b as usize, argc as usize)) {
                            continue 'reload;
                        }
                        if tri!(self.try_native_call(fi, b as usize, argc as usize)) {
                            // SAFETY: as in `slow`.
                            base = unsafe { self.vstack.as_mut_ptr().add(fbase) };
                            continue;
                        }
                        slow!(op);
                    }
                    Op::NewR(b, argc) => {
                        tick!();
                        sync!();
                        if tri!(self.try_fast_construct(fi, b as usize, argc as usize)) {
                            continue 'reload;
                        }
                        slow!(op);
                    }
                    Op::ReturnR(s) => {
                        let mut v = match std::mem::replace(sl!(s), Value::Undefined) {
                            Value::Empty => Value::Undefined,
                            v => v,
                        };
                        // SAFETY: as in `sync`.
                        let f = unsafe { self.frames.get_unchecked(fi) };
                        if f.is_construct && !v.is_object() {
                            if f.this.is_empty() {
                                return Err(self.new_error("ReferenceError", "Must call super constructor in derived class before accessing 'this' or returning from derived constructor"));
                            }
                            v = f.this.clone();
                        }
                        let f = self.pop_frame();
                        if f.entry {
                            if let Some(g) = f.generator() {
                                if let ObjectKind::Generator(gd) = &mut self.obj_mut(g).kind {
                                    gd.state = GeneratorState::Completed;
                                    gd.frame = None;
                                }
                            }
                            return Ok(Exit::Return(v));
                        }
                        let ret = f.ret_slot as usize;
                        match self.frames.last() {
                            Some(caller) => {
                                let at = caller.base as usize + ret;
                                self.vstack[at] = v;
                            }
                            None => stack_underflow(),
                        }
                        continue 'reload;
                    }
                    _ => slow!(op),
                }
            }
        }
    }

    fn inc_value(&mut self, v: Value, d: f64) -> JsResult<Value> {
        Ok(match (self.to_numeric(v)?).v() {
            V::Number(n) => Value::Number(n + d),
            V::BigInt(b) => Value::BigInt(Rc::new(b.add(&BigInt::from_i128(d as i128)))),
            _ => unreachable!(),
        })
    }

    /// `arguments[key]` read of an `args_direct` frame.
    fn args_get(&mut self, fi: usize, key: Value) -> JsResult<Value> {
        let f = &self.frames[fi];
        let (bottom, argc) = (f.bottom(), f.argc as usize);
        if let V::Number(n) = key.v() {
            let i = n as usize;
            // Like the arguments objects this engine creates, reads are
            // not mapped to the parameters.
            if i as f64 == n && i < argc {
                return Ok(self.vstack[bottom + i].clone());
            }
        }
        // Anything else: an equivalent (unobservable) arguments object.
        let args = self.vstack[bottom..bottom + argc].to_vec();
        let callee = if f.code.flags.is_strict { None } else { f.func };
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
                fb + f.code.nregs as usize + f.code.depths[f.pc - 1] as usize
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
                    let k = frame!().code.ic_keys[$ic as usize];
                    match (frame!().code.constants[k as usize]).v() {
                        V::String(s) => PropertyKey::String(s.clone()),
                        _ => unreachable!(),
                    }
                }};
            }
            macro_rules! constant {
                ($i:expr) => {
                    frame!().code.constants[$i as usize].clone()
                };
            }
            macro_rules! const_key {
                ($i:expr) => {
                    match (frame!().code.constants[$i as usize]).v() {
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
                    if sl!(s).truthy() {
                        jmp!(t);
                    }
                }
                Op::JumpIfFalseR(s, t) => {
                    if !sl!(s).truthy() {
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
                    frame!().code.ics[ic as usize].set(rec);
                    self.mega_put(&obj, &key, rec);
                    set!(d, v);
                }
                Op::SetPropR(o, v, ic) => {
                    let obj = rd!(o);
                    let val = rd!(v);
                    let key = ic_key!(ic);
                    let before = self.ic_before_set(&obj, &key);
                    let ok = self.set(obj.clone(), key.clone(), val)?;
                    if !ok && frame!().code.flags.is_strict {
                        let k = self.key_display(&key);
                        return self.type_error(&format!("Cannot assign to read only property '{}' of object", k));
                    }
                    let rec = if ok { self.ic_fill_set(&obj, &key, before, false) } else { Default::default() };
                    if ok {
                        self.mega_put_set(&obj, &key, before, rec);
                    }
                    frame!().code.ics[ic as usize].set(rec);
                }
                Op::DefinePropR(o, v, ic) => {
                    let objv = rd!(o);
                    let val = rd!(v);
                    let obj = objv.as_object().unwrap();
                    let key = ic_key!(ic);
                    let before = self.ic_before_set(&objv, &key);
                    self.define_own(obj, key.clone(), Property::data(val, ALL))?;
                    let rec = self.ic_fill_set(&objv, &key, before, true);
                    frame!().code.ics[ic as usize].set(rec);
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
                    if !ok && frame!().code.flags.is_strict {
                        let k = self.key_display(&key);
                        return self.type_error(&format!("Cannot assign to read only property '{}' of object", k));
                    }
                }
                Op::GetGlobalR(d, ic) => {
                    let key = ic_key!(ic);
                    let v = self.get_global_binding(&key)?;
                    let rec = self.ic_fill_global(key.as_str().unwrap());
                    frame!().code.ics[ic as usize].set(rec);
                    set!(d, v);
                }
                Op::SetGlobalR(s, ic) => {
                    let v = rd!(s);
                    let key = ic_key!(ic);
                    let strict = frame!().code.flags.is_strict;
                    self.set_global_binding(&key, v, strict)?;
                    let rec = self.ic_fill_global(key.as_str().unwrap());
                    frame!().code.ics[ic as usize].set(rec);
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
                    frame!().code.ics[ic as usize].set(rec);
                    push!(v);
                }
                Op::SetGlobal(i, ic) => {
                    let v = pop!();
                    let key = const_key!(i);
                    let strict = frame!().code.flags.is_strict;
                    self.set_global_binding(&key, v, strict)?;
                    let rec = self.ic_fill_global(key.as_str().unwrap());
                    frame!().code.ics[ic as usize].set(rec);
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
                    let names = frame!().code.block_names.get(names_idx as usize).cloned();
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
                    frame!().code.ics[ic as usize].set(rec);
                    push!(v);
                }
                Op::GetPropKeep(i, ic) => {
                    let obj = peek!().clone();
                    let key = const_key!(i);
                    let v = self.get(obj.clone(), key.clone())?;
                    let rec = self.ic_fill_get(&obj, &key);
                    frame!().code.ics[ic as usize].set(rec);
                    push!(v);
                }
                Op::GetMethod(i, ic) => {
                    let obj = peek!().clone();
                    let key = const_key!(i);
                    let v = self.get(obj.clone(), key.clone())?;
                    let rec = self.ic_fill_get(&obj, &key);
                    frame!().code.ics[ic as usize].set(rec);
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
                    if !ok && frame!().code.flags.is_strict {
                        let k = self.key_display(&key);
                        return self.type_error(&format!("Cannot assign to read only property '{}' of object", k));
                    }
                    let rec = if ok { self.ic_fill_set(&obj, &key, before, false) } else { Default::default() };
                    frame!().code.ics[ic as usize].set(rec);
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
                    if !ok && frame!().code.flags.is_strict {
                        let k = self.key_display(&key);
                        return self.type_error(&format!("Cannot assign to read only property '{}' of object", k));
                    }
                    push!(v);
                }
                Op::DeleteProp(i) => {
                    let obj = pop!();
                    let key = const_key!(i);
                    let r = self.delete_elem(obj, key, frame!().code.flags.is_strict)?;
                    push!(Value::Bool(r));
                }
                Op::DeleteElem => {
                    let key = pop!();
                    let obj = pop!();
                    let key = self.to_property_key(key)?;
                    let r = self.delete_elem(obj, key, frame!().code.flags.is_strict)?;
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
                    frame!().code.ics[ic as usize].set(rec);
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
                    let flags = if flag == 1 { ALL } else { HIDDEN };
                    self.define_own(obj, key, Property::data(f, flags))?;
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
                    let flags = if flag == 1 { ENUMERABLE | CONFIGURABLE } else { CONFIGURABLE };
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
                Op::GetSuperProp(i) => {
                    let key = const_key!(i);
                    let v = self.super_get(fi, key)?;
                    push!(v);
                }
                Op::GetSuperElem => {
                    let key = pop!();
                    let key = self.to_property_key(key)?;
                    let v = self.super_get(fi, key)?;
                    push!(v);
                }
                Op::SetSuperProp(i) => {
                    let v = pop!();
                    let key = const_key!(i);
                    self.super_set(fi, key, v.clone())?;
                    push!(v);
                }
                Op::SetSuperElem => {
                    let v = pop!();
                    let key = pop!();
                    let key = self.to_property_key(key)?;
                    self.super_set(fi, key, v.clone())?;
                    push!(v);
                }
                Op::Call(argc, desc) | Op::CallEval(argc, desc) | Op::CallR(_, argc, desc) => {
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
                            let strict = frame!().code.flags.is_strict;
                            let v = self.direct_eval(args.first().cloned().unwrap_or(Value::Undefined), strict)?;
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
                    if !pop!().truthy() {
                        frame!().pc = t as usize;
                    }
                }
                Op::JumpIfTrue(t) => {
                    if pop!().truthy() {
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
                    if !peek!().truthy() {
                        frame!().pc = t as usize;
                    }
                }
                Op::JumpIfTrueKeep(t) => {
                    if peek!().truthy() {
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
                    push!(Value::Bool(!v.truthy()));
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
                    let r = self.has_own_property(id, &key);
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
                    let key = (Rc::as_ptr(&frame!().code) as usize, idx);
                    let v = match self.template_cache.get(&key) {
                        Some(o) => Value::Object(*o),
                        None => {
                            let (cooked, raws) = frame!().code.templates[idx as usize].clone();
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
                    let code = frame!().code.functions[idx as usize].clone();
                    let f = self.make_closure(fi, code);
                    push!(Value::Object(f));
                }
                Op::MakeClass(idx) => {
                    let superclass = pop!();
                    let code = frame!().code.functions[idx as usize].clone();
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
                    let v = match frame!().code.module {
                        Some(m) => Value::Object(self.import_meta(crate::modules::ModuleId(m))),
                        None => Value::Object(self.new_object()),
                    };
                    push!(v);
                }
                Op::DynamicImport => {
                    let specifier = pop!();
                    let referrer = frame!().code.filename.clone();
                    let p = self.start_dynamic_import(specifier, &referrer);
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
                        while let Some(v) = self.iterator_step(&it, &next)? {
                            items.push(v);
                        }
                        self.vstack[n - 1] = Value::Empty;
                    }
                    let a = self.new_array(items);
                    push!(Value::Object(a));
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
                Op::Yield | Op::AsyncYield => {
                    let v = pop!();
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
        match self.frames[fi].code.reg_names.get(r as usize) {
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
            } else if depth as u32 == frame.scope_depth && frame.code.scope_slots > 0 {
                if let Some(n) = frame.code.slot_names.get(slot as usize) {
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
            if !matches!(t.kind, TypedArrayKind::BigInt64 | TypedArrayKind::BigUint64) && (i as usize) < t.length {
                crate::builtins::typed_array_set(self, t, i as usize, x);
                return Ok(());
            }
            return Err(v);
        }
        let Some(o) = self.heap.try_get_mut(id) else { return Err(v) };
        let no_props = o.props.is_empty();
        let extensible = o.extensible;
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

    /// Return `v` from the current frame.
    fn do_return(&mut self, mut v: Value) -> JsResult<Flow> {
        let f = self.frames.last().unwrap();
        if f.is_construct && !v.is_object() {
            if f.this.is_empty() {
                return Err(self.new_error("ReferenceError", "Must call super constructor in derived class before accessing 'this' or returning from derived constructor"));
            }
            v = f.this.clone();
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

    /// A binary operator on two values (the slow path of the register
    /// instructions).
    fn binop_values(&mut self, op: Op, a: Value, b: Value) -> JsResult<Value> {
        Ok(match op {
            Op::Add => match (a.v(), b.v()) {
                (V::Number(x), V::Number(y)) => Value::Number(x + y),
                (V::String(x), V::String(y)) => Value::String(x.concat(y)),
                (V::String(x), V::Number(_)) => {
                    let y = self.to_string(b.clone())?;
                    Value::String(x.concat(&y))
                }
                (V::Number(_), V::String(y)) => {
                    let x = self.to_string(a.clone())?;
                    Value::String(x.concat(y))
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
            Op::Not => Value::Bool(!v.truthy()),
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

    fn direct_eval(&mut self, arg: Value, strict: bool) -> JsResult<Value> {
        let src = match arg.v() {
            V::String(s) => s.to_rust_string(),
            other => return Ok(other.to_value()),
        };
        let src = if strict { format!("\"use strict\";{}", src) } else { src };
        let code = self.compile(&src, "eval", false)?;
        self.run_script(code)
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

    fn super_get(&mut self, fi: usize, key: PropertyKey) -> JsResult<Value> {
        let (home, this) = {
            let f = &self.frames[fi];
            (f.home_object, f.this.clone())
        };
        let home = match home {
            Some(h) => h,
            None => return self.type_error("'super' keyword unexpected here"),
        };
        let proto = self.get_prototype_of(home)?;
        let Some(p) = proto else { return Ok(Value::Undefined) };
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

    fn super_set(&mut self, fi: usize, key: PropertyKey, v: Value) -> JsResult<()> {
        let (home, this) = {
            let f = &self.frames[fi];
            (f.home_object, f.this.clone())
        };
        let home = match home {
            Some(h) => h,
            None => return self.type_error("'super' keyword unexpected here"),
        };
        if let Some(p) = self.get_prototype_of(home)? {
            self.set_on_object(p, key, v, this)?;
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
            home_object: if flags.is_arrow { frame.home_object } else { None },
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
        let id = self.alloc(Object::new(Some(proto), ObjectKind::Function(Box::new(closure))));
        self.define_property(id, names::length(), Value::Number(code.param_count as f64), CONFIGURABLE);
        self.define_property(id, names::name(), Value::String(code.name.clone()), CONFIGURABLE);
        if flags.is_generator {
            let gp = if flags.is_async { self.intrinsics.async_generator_proto } else { self.intrinsics.generator_proto };
            let p = self.alloc(Object::new(Some(gp), ObjectKind::Ordinary));
            self.define_property(id, "prototype", Value::Object(p), WRITABLE);
        } else if !flags.is_arrow && !flags.is_method && !flags.is_async && !flags.is_class_constructor {
            let p = self.new_object();
            self.define_property(p, names::constructor(), Value::Object(id), HIDDEN);
            self.define_property(id, names::prototype(), Value::Object(p), WRITABLE);
        }
        id
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
            ObjectKind::Ordinary | ObjectKind::Function(_) | ObjectKind::Native(_) | ObjectKind::Error | ObjectKind::Arguments => None,
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
            return Ok(self.alloc(Object::new(Some(proto), ObjectKind::ForInIterator { keys, index: 0, object, shape })));
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
        Ok(self.alloc(Object::new(Some(proto), ObjectKind::ForInIterator { keys, index: 0, object, shape: 0 })))
    }

    fn for_in_next(&mut self, it: ObjectId) -> JsResult<Option<JsStr>> {
        loop {
            let (key, object, shape) = match &mut self.obj_mut(it).kind {
                ObjectKind::ForInIterator { keys, index, object, shape } => {
                    if *index >= keys.len() {
                        return Ok(None);
                    }
                    let k = keys[*index].clone();
                    *index += 1;
                    (k, *object, *shape)
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
            return Ok(Value::String(sa.concat(&sb)));
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
