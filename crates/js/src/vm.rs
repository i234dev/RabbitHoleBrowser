//! The bytecode interpreter: frames, calls, generators, async and exceptions.

use crate::bytecode::{FunctionCode, Op, VarLoc};
use crate::heap::*;
use crate::interop::{Hint, JsResult};
use crate::realm::*;
use crate::value::*;
use std::rc::Rc;

/// An activation record.
pub struct Frame {
    pub code: Rc<FunctionCode>,
    pub pc: usize,
    pub stack: Vec<Value>,
    pub scope: ObjectId,
    pub scope_depth: u32,
    pub this: Value,
    pub func: Option<ObjectId>,
    pub new_target: Value,
    pub home_object: Option<ObjectId>,
    pub args: Vec<Value>,
    pub arguments_obj: Option<Value>,
    pub generator: Option<ObjectId>,
    pub is_construct: bool,
    /// Returning from this frame exits the run loop.
    pub entry: bool,
    /// Script completion value (last expression statement).
    pub completion: Value,
    /// Constant index describing the callee of the next call (for errors).
    pub callee_desc: Option<u32>,
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
        let id = match f {
            Value::Object(id) => id,
            _ => {
                let d = self.describe_for_error(&f);
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
                let frame = self.prepare_frame(id, this, args, Value::Undefined, false)?;
                self.push_frame(frame, true)?;
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
        if self.native_depth >= self.max_native_depth {
            return self.range_error("Maximum call stack size exceeded");
        }
        self.native_depth += 1;
        let saved = self.current_callee.replace(id);
        if !construct {
            self.new_target_stack.push(Value::Undefined);
        }
        let r = func(self, this, args);
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
        let id = match f {
            Value::Object(id) => id,
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
        let nregs = code.nregs as usize;
        let stack = self.take_stack(nregs);
        let frame = Frame {
            code,
            pc: 0,
            stack,
            scope,
            scope_depth: 0,
            this,
            func: None,
            new_target: Value::Undefined,
            home_object: None,
            args: Vec::new(),
            arguments_obj: None,
            generator: None,
            is_construct: false,
            entry: true,
            completion: Value::Undefined,
            callee_desc: None,
        };
        self.push_frame(frame, true)?;
        match self.run()? {
            Exit::Return(v) => Ok(v),
            _ => unreachable!(),
        }
    }

    fn push_frame(&mut self, mut frame: Frame, entry: bool) -> JsResult<()> {
        if self.frames.len() >= self.max_frames {
            return self.range_error("Maximum call stack size exceeded");
        }
        frame.entry = entry;
        self.frames.push(frame);
        Ok(())
    }

    /// `Call(argc)` of a plain closure (no `arguments`, rest parameter,
    /// generator/async or class constructor): builds the callee frame by
    /// moving the arguments straight from the caller's operand stack.
    /// Returns false (nothing done) when the general path is needed. The
    /// caller's frame must be synced.
    pub(crate) fn try_fast_call(&mut self, argc: usize) -> JsResult<bool> {
        let fi = self.frames.len() - 1;
        let n = self.frames[fi].stack.len();
        let base = n - argc - 2;
        let id = match &self.frames[fi].stack[base] {
            Value::Object(id) => *id,
            _ => return Ok(false),
        };
        let Some(obj) = self.heap.try_get(id) else { return Ok(false) };
        let ObjectKind::Function(c) = &obj.kind else { return Ok(false) };
        let flags = c.code.flags;
        if flags.is_generator || flags.is_async || flags.is_class_constructor || c.code.arguments_loc.is_some() || c.code.rest_loc.is_some() {
            return Ok(false);
        }
        let this = if flags.is_arrow {
            c.this_value.clone().unwrap_or(Value::Undefined)
        } else if flags.is_strict {
            self.frames[fi].stack[base + 1].clone()
        } else {
            match &self.frames[fi].stack[base + 1] {
                Value::Undefined | Value::Null | Value::Empty => Value::Object(self.global),
                v @ Value::Object(_) => v.clone(),
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
        let arguments_obj = if flags.is_arrow { c.arguments.clone() } else { None };
        let nslots = code.scope_slots as usize;
        let nregs = code.nregs as usize;
        let mut stack = self.take_stack(nregs);
        let mut slots = if nslots == 0 { Vec::new() } else { vec![Value::Undefined; nslots] };
        let frame_args = if flags.args_direct {
            let mut v = self.args_pool.pop().unwrap_or_default();
            v.extend_from_slice(&self.frames[fi].stack[base + 2..]);
            v
        } else {
            Vec::new()
        };
        {
            let caller = &mut self.frames[fi].stack;
            let mut args = caller.drain(base..);
            args.next(); // callee
            args.next(); // this (already taken)
            for &loc in code.param_locs.iter() {
                let v = args.next().unwrap_or(Value::Undefined);
                match loc {
                    VarLoc::Reg(r) => stack[r as usize] = v,
                    VarLoc::Slot(s) => slots[s as usize] = v,
                }
            }
        }
        if let Some(sl) = code.self_loc {
            match sl {
                VarLoc::Reg(r) => stack[r as usize] = Value::Object(id),
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
            stack,
            scope,
            scope_depth: 0,
            this,
            func: Some(id),
            new_target,
            home_object: home,
            args: frame_args,
            arguments_obj,
            generator: None,
            is_construct: false,
            entry: false,
            completion: Value::Undefined,
            callee_desc: None,
        });
        Ok(true)
    }

    /// `New(argc)` of a plain (non-derived, non-class-field) function
    /// constructor: allocates `this` from the callee's own `prototype`
    /// data property and builds the frame from the operand stack. Returns
    /// false when the general path is needed. The caller must be synced.
    pub(crate) fn try_fast_construct(&mut self, argc: usize) -> JsResult<bool> {
        let fi = self.frames.len() - 1;
        let n = self.frames[fi].stack.len();
        let base = n - argc - 1;
        let id = match &self.frames[fi].stack[base] {
            Value::Object(id) => *id,
            _ => return Ok(false),
        };
        let Some(obj) = self.heap.try_get(id) else { return Ok(false) };
        let ObjectKind::Function(c) = &obj.kind else { return Ok(false) };
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
            Some(p) if p.flags & ACCESSOR == 0 => match &p.value {
                Value::Object(p) => *p,
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
        let nregs = code.nregs as usize;
        let this = Value::Object(self.alloc(Object::new(Some(proto), ObjectKind::Ordinary)));
        let mut stack = self.take_stack(nregs);
        let mut slots = if nslots == 0 { Vec::new() } else { vec![Value::Undefined; nslots] };
        let frame_args = if flags.args_direct {
            let mut v = self.args_pool.pop().unwrap_or_default();
            v.extend_from_slice(&self.frames[fi].stack[base + 1..]);
            v
        } else {
            Vec::new()
        };
        {
            let caller = &mut self.frames[fi].stack;
            let mut args = caller.drain(base..);
            args.next(); // callee
            for &loc in code.param_locs.iter() {
                let v = args.next().unwrap_or(Value::Undefined);
                match loc {
                    VarLoc::Reg(r) => stack[r as usize] = v,
                    VarLoc::Slot(s) => slots[s as usize] = v,
                }
            }
        }
        if let Some(sl) = code.self_loc {
            match sl {
                VarLoc::Reg(r) => stack[r as usize] = Value::Object(id),
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
            stack,
            scope,
            scope_depth: 0,
            this,
            func: Some(id),
            new_target: Value::Object(id),
            home_object: home,
            args: frame_args,
            arguments_obj: None,
            generator: None,
            is_construct: true,
            entry: false,
            completion: Value::Undefined,
            callee_desc: None,
        });
        Ok(true)
    }

    /// Build the frame for calling closure `id`.
    pub(crate) fn prepare_frame(&mut self, id: ObjectId, this: Value, args: &[Value], new_target: Value, is_construct: bool) -> JsResult<Frame> {
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
                let proto = match self.get(new_target.clone(), names::prototype())? {
                    Value::Object(p) => p,
                    _ => self.intrinsics.object_proto,
                };
                Value::Object(self.alloc(Object::new(Some(proto), ObjectKind::Ordinary)))
            }
        } else if flags.is_strict {
            this
        } else {
            match this {
                Value::Undefined | Value::Null | Value::Empty => Value::Object(self.global),
                Value::Object(_) => this,
                prim => Value::Object(self.to_object(prim)?),
            }
        };
        let new_target = if flags.is_arrow { arrow_nt.unwrap_or(Value::Undefined) } else { new_target };
        let nslots = code.scope_slots as usize;
        let nregs = code.nregs as usize;
        let mut slots = if nslots == 0 { Vec::new() } else { vec![Value::Undefined; nslots] };
        let mut stack = self.take_stack(nregs);
        #[inline(always)]
        fn put(stack: &mut [Value], slots: &mut [Value], loc: VarLoc, v: Value) {
            match loc {
                VarLoc::Reg(r) => stack[r as usize] = v,
                VarLoc::Slot(s) => slots[s as usize] = v,
            }
        }
        for (i, &loc) in code.param_locs.iter().enumerate() {
            let v = args.get(i).cloned().unwrap_or(Value::Undefined);
            put(&mut stack, &mut slots, loc, v);
        }
        let mut arguments_obj = None;
        if let Some(rl) = code.rest_loc {
            let start = code.param_locs.len();
            let rest: Vec<Value> = if args.len() > start { args[start..].to_vec() } else { Vec::new() };
            let arr = self.new_array_value(rest);
            put(&mut stack, &mut slots, rl, arr);
        }
        if let Some(sl) = code.self_loc {
            put(&mut stack, &mut slots, sl, Value::Object(id));
        }
        if let Some(al) = code.arguments_loc {
            let ao = self.create_arguments_object(args, if flags.is_strict { None } else { Some(id) });
            put(&mut stack, &mut slots, al, ao.clone());
            arguments_obj = Some(ao);
        } else if flags.is_arrow {
            arguments_obj = arrow_args;
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
            stack,
            scope,
            scope_depth: 0,
            this,
            func: Some(id),
            new_target,
            home_object: home,
            args: if flags.args_direct { self.take_args_vec(args) } else { Vec::new() },
            arguments_obj,
            generator: None,
            is_construct,
            entry: false,
            completion: Value::Undefined,
            callee_desc: None,
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
        let is_async = flags.is_async;
        let is_gen = flags.is_generator;
        let proto = if is_gen {
            // generator object prototype = fn.prototype (if object) else intrinsic
            match self.get(Value::Object(id), names::prototype())? {
                Value::Object(p) => p,
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
        frame.generator = Some(gen);
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
        if state == GeneratorState::SuspendedYield {
            match mode {
                ResumeMode::Next => frame.stack.push(value.clone()),
                ResumeMode::Return => {
                    // Finally blocks are not run (inline-finally compilation).
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
                frame.stack.truncate(frame.code.nregs as usize + h.stack_depth as usize);
                while frame.scope_depth > h.scope_depth {
                    let parent = match &self.heap.get(frame.scope).kind {
                        ObjectKind::Scope(s) => s.parent,
                        _ => None,
                    };
                    frame.scope = parent.expect("scope underflow");
                    frame.scope_depth -= 1;
                }
                frame.stack.push(err);
                frame.pc = h.target as usize;
                return Ok(());
            }
            let f = self.frames.pop().unwrap();
            if let Some(g) = f.generator {
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
        if let Some(d) = self.deadline {
            if self.terminating || std::time::Instant::now() >= d {
                self.terminating = true;
                return Err(self.new_error("RangeError", "Script execution timed out"));
            }
        }
        Ok(())
    }

    /// The interpreter loop. Hot instructions are handled inline with the
    /// program counter and the operand stack pointer kept in locals;
    /// everything else goes through [`Realm::exec_op`] after the frame has
    /// been synced (`pc` and the stack length written back).
    #[allow(unused_unsafe, unused_assignments)]
    fn run_inner(&mut self) -> JsResult<Exit> {
        'reload: loop {
            let fi = self.frames.len() - 1;
            // A clone of the code keeps the op slice alive independently of
            // `self` borrows while this frame is current.
            let code = self.frames[fi].code.clone();
            let ops: &[Op] = &code.ops;
            let consts: &[Value] = &code.constants;
            let mut pc = self.frames[fi].pc;
            // Operand stack as raw pointers: `base` (registers start),
            // `sp` (next free slot) and `limit` (capacity end). The frame's
            // Vec length is only correct after `sync!`.
            let mut base: *mut Value;
            let mut sp: *mut Value;
            let mut limit: *mut Value;
            macro_rules! fr {
                () => {
                    // SAFETY: `fi` is the last frame; frames only change in
                    // instructions that then reload.
                    unsafe { self.frames.get_unchecked_mut(fi) }
                };
            }
            macro_rules! load_sp {
                () => {{
                    let st = &mut fr!().stack;
                    base = st.as_mut_ptr();
                    // SAFETY: within the Vec allocation.
                    unsafe {
                        sp = base.add(st.len());
                        limit = base.add(st.capacity());
                    }
                }};
            }
            macro_rules! save_sp {
                () => {
                    // SAFETY: every slot in [base, sp) is initialised.
                    unsafe { fr!().stack.set_len(sp.offset_from(base) as usize) }
                };
            }
            macro_rules! sync {
                () => {{
                    save_sp!();
                    fr!().pc = pc;
                }};
            }
            macro_rules! resume {
                () => {{
                    load_sp!();
                    pc = fr!().pc;
                }};
            }
            macro_rules! tri {
                ($e:expr) => {
                    match $e {
                        Ok(v) => v,
                        Err(e) => return Err(e),
                    }
                };
            }
            macro_rules! pop {
                () => {
                    // SAFETY: the compiler guarantees operands are present.
                    unsafe {
                        debug_assert!(sp > base);
                        sp = sp.sub(1);
                        std::ptr::read(sp)
                    }
                };
            }
            macro_rules! top {
                () => {
                    // SAFETY: as for pop.
                    unsafe { &mut *sp.sub(1) }
                };
            }
            macro_rules! push {
                ($v:expr) => {{
                    let v = $v;
                    if sp == limit {
                        save_sp!();
                        fr!().stack.reserve(64);
                        load_sp!();
                    }
                    // SAFETY: sp < limit.
                    unsafe {
                        std::ptr::write(sp, v);
                        sp = sp.add(1);
                    }
                }};
            }
            macro_rules! reg {
                ($r:expr) => {
                    // SAFETY: registers occupy the first `nregs` slots.
                    unsafe {
                        debug_assert!(($r as usize) < code.nregs as usize);
                        &mut *base.add($r as usize)
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
                        resume!();
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
            // Binary numeric op on the two top values when both are numbers.
            macro_rules! num2 {
                (|$x:ident, $y:ident| $e:expr) => {{
                    // SAFETY: two operands are present.
                    let (a, b) = unsafe { (&*sp.sub(2), &*sp.sub(1)) };
                    if let (Value::Number($x), Value::Number($y)) = (a, b) {
                        let ($x, $y) = (*$x, *$y);
                        let r = $e;
                        // SAFETY: both slots hold numbers (no destructor).
                        unsafe {
                            sp = sp.sub(1);
                            std::ptr::write(sp.sub(1), r);
                        }
                        continue;
                    }
                }};
            }
            // Out-of-line instruction.
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
                    resume!();
                }};
            }
            load_sp!();
            loop {
                let op = ops[pc];
                pc += 1;
                match op {
                    Op::GetReg(r) => {
                        let v = match reg!(r) {
                            Value::Empty => Value::Undefined,
                            v => v.clone(),
                        };
                        push!(v);
                    }
                    Op::GetRegChecked(r) => {
                        let v = reg!(r).clone();
                        if v.is_empty() {
                            sync!();
                            let name = self.reg_name(fi, r);
                            return Err(self.new_error("ReferenceError", &format!("Cannot access '{}' before initialization", name)));
                        }
                        push!(v);
                    }
                    Op::SetReg(r) => {
                        let v = pop!();
                        *reg!(r) = v;
                    }
                    Op::SetRegChecked(r) => {
                        if reg!(r).is_empty() {
                            sync!();
                            let name = self.reg_name(fi, r);
                            return Err(self.new_error("ReferenceError", &format!("Cannot access '{}' before initialization", name)));
                        }
                        let v = pop!();
                        *reg!(r) = v;
                    }
                    Op::IncReg(r) | Op::DecReg(r) => {
                        let d = if matches!(op, Op::IncReg(_)) { 1.0 } else { -1.0 };
                        if let Value::Number(n) = reg!(r) {
                            *n += d;
                        } else {
                            sync!();
                            let v = match reg!(r) {
                                Value::Empty => Value::Undefined,
                                v => v.clone(),
                            };
                            let nv = tri!(self.inc_value(v, d));
                            resume!();
                            *reg!(r) = nv;
                        }
                    }
                    Op::Const(i) => push!(consts[i as usize].clone()),
                    Op::GetReg2(a, b) => {
                        let va = match reg!(a) {
                            Value::Empty => Value::Undefined,
                            v => v.clone(),
                        };
                        push!(va);
                        let vb = match reg!(b) {
                            Value::Empty => Value::Undefined,
                            v => v.clone(),
                        };
                        push!(vb);
                    }
                    Op::BinConst(k, c) => {
                        let bop = crate::bytecode::BIN_OPS[k as usize];
                        if let (Value::Number(x), Value::Number(y)) = (&*top!(), &consts[c as usize]) {
                            let (x, y) = (*x, *y);
                            let r = match bop {
                                Op::Add => x + y,
                                Op::Sub => x - y,
                                Op::Mul => x * y,
                                Op::Div => x / y,
                                Op::Mod => js_mod(x, y),
                                Op::BitAnd => (to_int32(x) & to_int32(y)) as f64,
                                Op::BitOr => (to_int32(x) | to_int32(y)) as f64,
                                Op::BitXor => (to_int32(x) ^ to_int32(y)) as f64,
                                Op::Shl => to_int32(x).wrapping_shl(to_uint32(y) & 31) as f64,
                                Op::Shr => (to_int32(x) >> (to_uint32(y) & 31)) as f64,
                                _ => (to_uint32(x) >> (to_uint32(y) & 31)) as f64,
                            };
                            // SAFETY: the slot holds a number (no destructor).
                            unsafe { std::ptr::write(sp.sub(1), Value::Number(r)) };
                        } else {
                            push!(consts[c as usize].clone());
                            sync!();
                            tri!(self.binop_slow(bop));
                            resume!();
                        }
                    }
                    Op::JumpUnlessLtConst(c, t) => {
                        let lt = if let (Value::Number(x), Value::Number(y)) = (&*top!(), &consts[c as usize]) {
                            let r = *x < *y;
                            // SAFETY: the slot holds a number (no destructor).
                            unsafe { sp = sp.sub(1) };
                            r
                        } else {
                            push!(consts[c as usize].clone());
                            sync!();
                            let r = tri!(self.compare_top(Op::JumpUnlessLt(0)));
                            resume!();
                            r
                        };
                        if !lt {
                            jump!(t);
                        }
                    }
                    Op::StoreCompletion => {
                        let v = pop!();
                        fr!().completion = v;
                    }
                    Op::Undefined => push!(Value::Undefined),
                    Op::Null => push!(Value::Null),
                    Op::True => push!(Value::Bool(true)),
                    Op::False => push!(Value::Bool(false)),
                    Op::Empty => push!(Value::Empty),
                    Op::Pop => {
                        pop!();
                    }
                    Op::Dup => {
                        let v = top!().clone();
                        push!(v);
                    }
                    Op::Swap => {
                        // SAFETY: two operands are present.
                        unsafe { std::ptr::swap(sp.sub(1), sp.sub(2)) };
                    }
                    Op::GetLocal(d, s) => {
                        let scope = fr!().scope;
                        let v = self.slot_get(scope, d, s);
                        push!(if v.is_empty() { Value::Undefined } else { v });
                    }
                    Op::SetLocal(d, s) | Op::InitLocal(d, s) => {
                        let v = pop!();
                        let scope = fr!().scope;
                        self.slot_set(scope, d, s, v);
                    }
                    Op::Jump(t) => jump!(t),
                    Op::JumpIfFalse(t) => {
                        if !pop!().truthy() {
                            jump!(t);
                        }
                    }
                    Op::JumpIfTrue(t) => {
                        if pop!().truthy() {
                            jump!(t);
                        }
                    }
                    Op::JumpIfFalseKeep(t) => {
                        if !top!().truthy() {
                            jump!(t);
                        }
                    }
                    Op::JumpIfTrueKeep(t) => {
                        if top!().truthy() {
                            jump!(t);
                        }
                    }
                    Op::JumpIfNullish(t) => {
                        if pop!().is_nullish() {
                            jump!(t);
                        }
                    }
                    Op::JumpIfUndefined(t) => {
                        if pop!().is_undefined() {
                            jump!(t);
                        }
                    }
                    Op::JumpIfNotUndefined(t) => {
                        if !pop!().is_undefined() {
                            jump!(t);
                        }
                    }
                    Op::JumpIfStrictEq(t) => {
                        let b = pop!();
                        let a = pop!();
                        if self.strict_equals(&a, &b) {
                            jump!(t);
                        }
                    }
                    Op::JumpIfStrictNotEq(t) => {
                        let b = pop!();
                        let a = pop!();
                        if !self.strict_equals(&a, &b) {
                            jump!(t);
                        }
                    }
                    Op::JumpUnlessLt(t) | Op::JumpUnlessLtEq(t) | Op::JumpUnlessGt(t) | Op::JumpUnlessGtEq(t) => {
                        // SAFETY: two operands are present.
                        let (a, b) = unsafe { (&*sp.sub(2), &*sp.sub(1)) };
                        let c = if let (Value::Number(x), Value::Number(y)) = (a, b) {
                            let (x, y) = (*x, *y);
                            // SAFETY: both slots hold numbers (no destructor).
                            unsafe { sp = sp.sub(2) };
                            match op {
                                Op::JumpUnlessLt(_) => x < y,
                                Op::JumpUnlessLtEq(_) => x <= y,
                                Op::JumpUnlessGt(_) => x > y,
                                _ => x >= y,
                            }
                        } else {
                            sync!();
                            let c = tri!(self.compare_top(op));
                            resume!();
                            c
                        };
                        if !c {
                            jump!(t);
                        }
                    }
                    Op::Add => {
                        num2!(|x, y| Value::Number(x + y));
                        let both_str = matches!(unsafe { (&*sp.sub(2), &*sp.sub(1)) }, (Value::String(_), Value::String(_)));
                        if both_str {
                            let b = pop!();
                            let a = top!();
                            if let (Value::String(x), Value::String(y)) = (&*a, &b) {
                                *a = Value::String(x.concat(y));
                            }
                        } else {
                            sync!();
                            tri!(self.binop_slow(op));
                            resume!();
                        }
                    }
                    Op::Sub => {
                        num2!(|x, y| Value::Number(x - y));
                        slow!(op);
                    }
                    Op::Mul => {
                        num2!(|x, y| Value::Number(x * y));
                        slow!(op);
                    }
                    Op::Div => {
                        num2!(|x, y| Value::Number(x / y));
                        slow!(op);
                    }
                    Op::Mod => {
                        num2!(|x, y| Value::Number(js_mod(x, y)));
                        slow!(op);
                    }
                    Op::BitAnd => {
                        num2!(|x, y| Value::Number((to_int32(x) & to_int32(y)) as f64));
                        slow!(op);
                    }
                    Op::BitOr => {
                        num2!(|x, y| Value::Number((to_int32(x) | to_int32(y)) as f64));
                        slow!(op);
                    }
                    Op::BitXor => {
                        num2!(|x, y| Value::Number((to_int32(x) ^ to_int32(y)) as f64));
                        slow!(op);
                    }
                    Op::Shl => {
                        num2!(|x, y| Value::Number(to_int32(x).wrapping_shl(to_uint32(y) & 31) as f64));
                        slow!(op);
                    }
                    Op::Shr => {
                        num2!(|x, y| Value::Number((to_int32(x) >> (to_uint32(y) & 31)) as f64));
                        slow!(op);
                    }
                    Op::UShr => {
                        num2!(|x, y| Value::Number((to_uint32(x) >> (to_uint32(y) & 31)) as f64));
                        slow!(op);
                    }
                    Op::Lt => {
                        num2!(|x, y| Value::Bool(x < y));
                        slow!(op);
                    }
                    Op::LtEq => {
                        num2!(|x, y| Value::Bool(x <= y));
                        slow!(op);
                    }
                    Op::Gt => {
                        num2!(|x, y| Value::Bool(x > y));
                        slow!(op);
                    }
                    Op::GtEq => {
                        num2!(|x, y| Value::Bool(x >= y));
                        slow!(op);
                    }
                    Op::StrictEq | Op::StrictNotEq => {
                        let b = pop!();
                        let a = top!();
                        let r = self.strict_equals(a, &b);
                        *a = Value::Bool(if matches!(op, Op::StrictEq) { r } else { !r });
                    }
                    Op::Not => {
                        let a = top!();
                        let r = !a.truthy();
                        *a = Value::Bool(r);
                    }
                    Op::Inc | Op::Dec => {
                        if let Value::Number(n) = top!() {
                            *n += if matches!(op, Op::Inc) { 1.0 } else { -1.0 };
                            continue;
                        }
                        slow!(op);
                    }
                    Op::ToNumeric => {
                        if let Value::Number(_) = top!() {
                            continue;
                        }
                        slow!(op);
                    }
                    Op::Return => {
                        save_sp!();
                        let mut v = match fr!().stack.pop() {
                            Some(v) => v,
                            None => stack_underflow(),
                        };
                        let f = match self.frames.pop() {
                            Some(f) => f,
                            None => stack_underflow(),
                        };
                        if f.is_construct && !v.is_object() {
                            if f.this.is_empty() {
                                return Err(self.new_error("ReferenceError", "Must call super constructor in derived class before accessing 'this' or returning from derived constructor"));
                            }
                            v = f.this.clone();
                        }
                        if f.entry {
                            if let Some(g) = f.generator {
                                if let ObjectKind::Generator(gd) = &mut self.obj_mut(g).kind {
                                    gd.state = GeneratorState::Completed;
                                    gd.frame = None;
                                }
                            }
                            self.recycle_frame(f);
                            return Ok(Exit::Return(v));
                        }
                        self.recycle_frame(f);
                        match self.frames.last_mut() {
                            Some(caller) => caller.stack.push(v),
                            None => stack_underflow(),
                        }
                        continue 'reload;
                    }
                    Op::GetProp(k, ic) | Op::GetPropKeep(k, ic) | Op::GetMethod(k, ic) => {
                        let rec = code.ics[ic as usize].get();
                        let recv = top!();
                        let hit = if rec.kind == crate::ic::IC_LENGTH {
                            match &*recv {
                                Value::String(s) => Some(Value::Number(s.len() as f64)),
                                Value::Object(id) => match self.heap.try_get(*id).map(|o| &o.kind) {
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
                            Some(v) => match op {
                                Op::GetProp(..) => *top!() = v,
                                Op::GetPropKeep(..) => push!(v),
                                _ => {
                                    // obj -> method obj
                                    let recv = std::mem::replace(top!(), v);
                                    push!(recv);
                                }
                            },
                            None => {
                                let _ = k;
                                slow!(op);
                            }
                        }
                    }
                    Op::SetProp(k, ic) => {
                        let rec = code.ics[ic as usize].get();
                        if rec.kind != 0 {
                            let key = match &consts[k as usize] {
                                Value::String(s) => s,
                                _ => unreachable!(),
                            };
                            // SAFETY: two operands are present.
                            let recv = unsafe { &*sp.sub(2) };
                            let v = top!().clone();
                            match self.ic_set(&rec, recv, key, v) {
                                Ok(()) => {
                                    // obj v -> v
                                    let v = pop!();
                                    *top!() = v;
                                    continue;
                                }
                                Err(v) => drop(v),
                            }
                        }
                        slow!(op);
                    }
                    Op::DefineProp(k, ic) => {
                        let rec = code.ics[ic as usize].get();
                        if rec.kind == crate::ic::IC_ADD {
                            let key = match &consts[k as usize] {
                                Value::String(s) => s,
                                _ => unreachable!(),
                            };
                            // SAFETY: two operands are present.
                            let recv = unsafe { &*sp.sub(2) };
                            let v = top!().clone();
                            match self.ic_set(&rec, recv, key, v) {
                                Ok(()) => {
                                    pop!();
                                    continue;
                                }
                                Err(v) => drop(v),
                            }
                        }
                        slow!(op);
                    }
                    Op::GetGlobal(k, ic) => {
                        let rec = code.ics[ic as usize].get();
                        let key = match &consts[k as usize] {
                            Value::String(s) => s,
                            _ => unreachable!(),
                        };
                        match self.ic_get_global(&rec, key) {
                            Some(v) => {
                                let v = v.clone();
                                push!(v);
                            }
                            None => slow!(op),
                        }
                    }
                    Op::SetGlobal(k, ic) => {
                        let rec = code.ics[ic as usize].get();
                        let key = match &consts[k as usize] {
                            Value::String(s) => s,
                            _ => unreachable!(),
                        };
                        let v = top!().clone();
                        match self.ic_set_global(&rec, key, v) {
                            Ok(()) => {
                                pop!();
                            }
                            Err(v) => {
                                drop(v);
                                slow!(op);
                            }
                        }
                    }
                    Op::GetElem => {
                        // SAFETY: two operands are present.
                        let (o, k) = unsafe { (&*sp.sub(2), &*sp.sub(1)) };
                        match self.get_elem_fast(o, k) {
                            Some(v) => {
                                pop!();
                                *top!() = v;
                            }
                            None => slow!(op),
                        }
                    }
                    Op::SetElem => {
                        // SAFETY: three operands are present.
                        let (o, k) = unsafe { (&*sp.sub(3), &*sp.sub(2)) };
                        let v = top!().clone();
                        match self.set_elem_fast(o, k, v) {
                            Ok(()) => {
                                // obj key v -> v
                                let v = pop!();
                                pop!();
                                *top!() = v;
                            }
                            Err(v) => {
                                drop(v);
                                slow!(op);
                            }
                        }
                    }
                    Op::Call(argc, _) => {
                        tick!();
                        sync!();
                        if tri!(self.try_fast_call(argc as usize)) {
                            continue 'reload;
                        }
                        slow!(op);
                    }
                    Op::New(argc) => {
                        tick!();
                        sync!();
                        if tri!(self.try_fast_construct(argc as usize)) {
                            continue 'reload;
                        }
                        slow!(op);
                    }
                    Op::CallSpread | Op::NewSpread => {
                        tick!();
                        slow!(op);
                    }
                    _ => slow!(op),
                }
            }
        }
    }

    /// Relational comparison of the two top values (popped), for the fused
    /// compare-and-branch instructions.
    #[inline(never)]
    fn compare_top(&mut self, op: Op) -> JsResult<bool> {
        let fi = self.frames.len() - 1;
        let b = self.frames[fi].stack.pop().unwrap();
        let a = self.frames[fi].stack.pop().unwrap();
        let cmp = match op {
            Op::JumpUnlessLt(_) => Op::Lt,
            Op::JumpUnlessLtEq(_) => Op::LtEq,
            Op::JumpUnlessGt(_) => Op::Gt,
            _ => Op::GtEq,
        };
        self.compare_slow(cmp, a, b)
    }

    /// `v + d` after ToNumeric (increment / decrement).
    #[inline(never)]
    fn inc_value(&mut self, v: Value, d: f64) -> JsResult<Value> {
        Ok(match self.to_numeric(v)? {
            Value::Number(n) => Value::Number(n + d),
            Value::BigInt(b) => Value::BigInt(Rc::new(b.add(&BigInt::from_i128(d as i128)))),
            _ => unreachable!(),
        })
    }

    /// Slow path of the numeric/comparison operators: operands on the stack.
    #[inline(never)]
    fn binop_slow(&mut self, op: Op) -> JsResult<()> {
        let fi = self.frames.len() - 1;
        let b = self.frames[fi].stack.pop().unwrap();
        let a = self.frames[fi].stack.pop().unwrap();
        let r = match op {
            Op::Lt | Op::LtEq | Op::Gt | Op::GtEq => Value::Bool(self.compare_slow(op, a, b)?),
            Op::Add => self.add_slow(a, b)?,
            _ => match (&a, &b) {
                (Value::Number(x), Value::Number(y)) => Value::Number(num_binop(op, *x, *y)),
                _ => self.numeric_binop(op, a, b)?,
            },
        };
        self.frames[fi].stack.push(r);
        Ok(())
    }

    /// Return a finished frame's operand stack to the pool.
    #[inline]
    pub(crate) fn recycle_frame(&mut self, f: Frame) {
        let mut a = f.args;
        if a.capacity() != 0 && a.capacity() <= 64 && self.args_pool.len() < 64 {
            a.clear();
            self.args_pool.push(a);
        }
        let mut st = f.stack;
        if st.capacity() <= 256 && self.stack_pool.len() < 64 {
            st.clear();
            self.stack_pool.push(st);
        }
    }

    /// A pooled vector holding a copy of `args` (frames of `args_direct` code).
    pub(crate) fn take_args_vec(&mut self, args: &[Value]) -> Vec<Value> {
        let mut v = self.args_pool.pop().unwrap_or_default();
        v.extend_from_slice(args);
        v
    }

    /// `arguments[key]` read of an `args_direct` frame.
    fn args_get(&mut self, fi: usize, key: Value) -> JsResult<Value> {
        let f = &self.frames[fi];
        if let Value::Number(n) = key {
            let i = n as usize;
            // Like the arguments objects this engine creates, reads are
            // not mapped to the parameters.
            if i as f64 == n && i < f.args.len() {
                return Ok(f.args[i].clone());
            }
        }
        // Anything else: an equivalent (unobservable) arguments object.
        let args = f.args.clone();
        let callee = if f.code.flags.is_strict { None } else { f.func };
        let ao = self.create_arguments_object(&args, callee);
        let key = self.to_property_key(key)?;
        self.get(ao, key)
    }

    /// A cleared operand stack with room for `n` registers plus operands.
    #[inline]
    pub(crate) fn take_stack(&mut self, nregs: usize) -> Vec<Value> {
        let mut st = match self.stack_pool.pop() {
            Some(s) => s,
            None => Vec::with_capacity(nregs + 16),
        };
        st.resize(nregs, Value::Undefined);
        st
    }

    /// Execute one (non-hot) instruction of frame `fi`, whose `pc` already
    /// points past it. The hot instructions live in [`Realm::run_inner`].
    #[inline(never)]
    fn exec_op(&mut self, fi: usize, op: Op) -> JsResult<Flow> {
        {
            macro_rules! frame {
                () => {
                    self.frames[fi]
                };
            }
            macro_rules! pop {
                () => {
                    frame!().stack.pop().unwrap()
                };
            }
            macro_rules! push {
                ($v:expr) => {{
                    let v = $v;
                    frame!().stack.push(v)
                }};
            }
            macro_rules! peek {
                () => {
                    frame!().stack.last().unwrap()
                };
            }
            macro_rules! constant {
                ($i:expr) => {
                    frame!().code.constants[$i as usize].clone()
                };
            }
            macro_rules! const_key {
                ($i:expr) => {
                    match &frame!().code.constants[$i as usize] {
                        Value::String(s) => PropertyKey::String(s.clone()),
                        _ => unreachable!(),
                    }
                };
            }
            match op {
                Op::GetReg(r) => {
                    let v = frame!().stack[r as usize].clone();
                    push!(if v.is_empty() { Value::Undefined } else { v });
                }
                Op::GetRegChecked(r) => {
                    let v = frame!().stack[r as usize].clone();
                    if v.is_empty() {
                        let name = self.reg_name(fi, r);
                        return Err(self.new_error("ReferenceError", &format!("Cannot access '{}' before initialization", name)));
                    }
                    push!(v);
                }
                Op::SetReg(r) => {
                    let v = pop!();
                    frame!().stack[r as usize] = v;
                }
                Op::SetRegChecked(r) => {
                    let v = pop!();
                    if frame!().stack[r as usize].is_empty() {
                        let name = self.reg_name(fi, r);
                        return Err(self.new_error("ReferenceError", &format!("Cannot access '{}' before initialization", name)));
                    }
                    frame!().stack[r as usize] = v;
                }
                Op::Const(i) => push!(constant!(i)),
                Op::Undefined => push!(Value::Undefined),
                Op::Null => push!(Value::Null),
                Op::True => push!(Value::Bool(true)),
                Op::False => push!(Value::Bool(false)),
                Op::Empty => push!(Value::Empty),
                Op::Pop => {
                    pop!();
                }
                Op::Dup => {
                    let v = peek!().clone();
                    push!(v);
                }
                Op::Dup2 => {
                    let n = frame!().stack.len();
                    let a = frame!().stack[n - 2].clone();
                    let b = frame!().stack[n - 1].clone();
                    push!(a);
                    push!(b);
                }
                Op::Swap => {
                    let n = frame!().stack.len();
                    frame!().stack.swap(n - 1, n - 2);
                }
                Op::Rot3 => {
                    // a b c -> b c a
                    let n = frame!().stack.len();
                    let a = frame!().stack.remove(n - 3);
                    push!(a);
                }
                Op::Rot4 => {
                    let n = frame!().stack.len();
                    let a = frame!().stack.remove(n - 4);
                    push!(a);
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
                    let n = frame!().stack.len();
                    let key = frame!().stack[n - 1].clone();
                    let obj = frame!().stack[n - 2].clone();
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
                    if let Value::Object(fid) = &v {
                        let anonymous = match &self.obj(*fid).kind {
                            ObjectKind::Function(c) => c.code.name.is_empty(),
                            _ => false,
                        };
                        if anonymous {
                            let has_name = self.obj(*fid).props.get(&PropertyKey::str("name")).map(|p| matches!(&p.value, Value::String(s) if !s.is_empty())).unwrap_or(false);
                            if !has_name {
                                let name = match &key {
                                    PropertyKey::String(s) => Value::String(s.clone()),
                                    PropertyKey::Symbol(sym) => Value::str(&format!("[{}]", self.symbol_description(*sym))),
                                };
                                self.define_property(*fid, "name", name, CONFIGURABLE);
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
                    if let Value::Object(fid) = &f {
                        if let ObjectKind::Function(c) = &mut self.obj_mut(*fid).kind {
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
                    if let Value::Object(fid) = &f {
                        if let ObjectKind::Function(c) = &mut self.obj_mut(*fid).kind {
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
                    match v {
                        Value::Object(p) => self.obj_mut(obj).proto = Some(p),
                        Value::Null => self.obj_mut(obj).proto = None,
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
                Op::Call(argc, desc) | Op::CallEval(argc, desc) => {
                    let n = frame!().stack.len();
                    let base = n - argc as usize - 2;
                    let args = self.take_args(fi, base + 2);
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
                    self.do_call(f, this, &args)?;
                    self.give_args(args);
                }
                Op::CallSpread => {
                    let arr = pop!();
                    let this = pop!();
                    let f = pop!();
                    let args = self.create_list_from_array_like(arr)?;
                    self.do_call(f, this, &args)?;
                }
                Op::New(argc) => {
                    let n = frame!().stack.len();
                    let base = n - argc as usize - 1;
                    let args = self.take_args(fi, base + 1);
                    let f = pop!();
                    self.do_construct(f, &args)?;
                    self.give_args(args);
                }
                Op::NewSpread => {
                    let arr = pop!();
                    let f = pop!();
                    let args = self.create_list_from_array_like(arr)?;
                    self.do_construct(f, &args)?;
                }
                Op::SuperCall(argc) => {
                    let n = frame!().stack.len();
                    let base = n - argc as usize;
                    let args: Vec<Value> = frame!().stack.drain(base..).collect();
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
                    let mut v = pop!();
                    let f = self.frames.pop().unwrap();
                    if f.is_construct && !v.is_object() {
                        if f.this.is_empty() {
                            return Err(self.new_error("ReferenceError", "Must call super constructor in derived class before accessing 'this' or returning from derived constructor"));
                        }
                        v = f.this.clone();
                    }
                    if f.entry {
                        if let Some(g) = f.generator {
                            if let ObjectKind::Generator(gd) = &mut self.obj_mut(g).kind {
                                gd.state = GeneratorState::Completed;
                                gd.frame = None;
                            }
                        }
                        return Ok(Flow::Exit(Exit::Return(v)));
                    }
                    self.frames.last_mut().unwrap().stack.push(v);
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
                        pop!();
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
                    let r = match (&a, &b) {
                        (Value::Number(x), Value::Number(y)) => Value::Number(x + y),
                        (Value::String(x), Value::String(y)) => Value::String(x.concat(y)),
                        _ => self.add_slow(a, b)?,
                    };
                    push!(r);
                }
                Op::Sub | Op::Mul | Op::Div | Op::Mod | Op::Exp | Op::BitAnd | Op::BitOr | Op::BitXor | Op::Shl | Op::Shr | Op::UShr => {
                    let b = pop!();
                    let a = pop!();
                    let r = match (&a, &b) {
                        (Value::Number(x), Value::Number(y)) => Value::Number(num_binop(op, *x, *y)),
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
                    let r = match (&a, &b) {
                        (Value::Number(x), Value::Number(y)) => match op {
                            Op::Lt => x < y,
                            Op::LtEq => x <= y,
                            Op::Gt => x > y,
                            _ => x >= y,
                        },
                        _ => self.compare_slow(op, a, b)?,
                    };
                    push!(Value::Bool(r));
                }
                Op::In => {
                    let obj = pop!();
                    let key = pop!();
                    let id = match obj {
                        Value::Object(id) => id,
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
                    let r = match v {
                        Value::Number(n) => Value::Number(-n),
                        other => match self.to_numeric(other)? {
                            Value::BigInt(b) => Value::BigInt(Rc::new(b.neg())),
                            Value::Number(n) => Value::Number(-n),
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
                    let r = match self.to_numeric(v)? {
                        Value::BigInt(b) => Value::BigInt(Rc::new(b.not())),
                        Value::Number(n) => Value::Number(!to_int32(n) as f64),
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
                    pop!();
                    push!(Value::Undefined);
                }
                Op::Inc | Op::Dec => {
                    let v = pop!();
                    let d = if matches!(op, Op::Inc) { 1.0 } else { -1.0 };
                    let r = match v {
                        Value::Number(n) => Value::Number(n + d),
                        Value::BigInt(b) => Value::BigInt(Rc::new(b.add(&BigInt::from_i128(d as i128)))),
                        other => match self.to_numeric(other)? {
                            Value::Number(n) => Value::Number(n + d),
                            Value::BigInt(b) => Value::BigInt(Rc::new(b.add(&BigInt::from_i128(d as i128)))),
                            _ => unreachable!(),
                        },
                    };
                    push!(r);
                }
                Op::ToNumeric => {
                    let v = pop!();
                    let r = match v {
                        Value::Number(_) | Value::BigInt(_) => v,
                        other => self.to_numeric(other)?,
                    };
                    push!(r);
                }
                Op::ToString => {
                    let v = pop!();
                    let r = match v {
                        Value::String(_) => v,
                        other => Value::String(self.to_string(other)?),
                    };
                    push!(r);
                }
                Op::ToPropertyKey => {
                    let v = pop!();
                    let r = match v {
                        // Numbers stay numbers: every consumer canonicalizes keys, and
                        // element fast paths want numeric indices.
                        Value::String(_) | Value::Symbol(_) | Value::Number(_) => v,
                        other => self.to_property_key(other)?.to_value(),
                    };
                    push!(r);
                }
                Op::PrivateIn(d, s) => {
                    let obj = pop!();
                    let scope = frame!().scope;
                    let sym = self.slot_get(scope, d, s);
                    let id = match obj {
                        Value::Object(id) => id,
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
                    let len = frame!().stack.len();
                    let items: Vec<Value> = frame!().stack.drain(len - n as usize..).collect();
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
                    let n = frame!().stack.len();
                    let proto = frame!().stack[n - 1].as_object().unwrap();
                    let ctor = frame!().stack[n - 2].as_object().unwrap();
                    if let Value::Object(fid) = &f {
                        if let ObjectKind::Function(c) = &mut self.obj_mut(*fid).kind {
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
                    let v = frame!().arguments_obj.clone().unwrap_or(Value::Undefined);
                    push!(v);
                }
                Op::ArgsLength => {
                    let n = frame!().args.len();
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
                    let n = frame!().stack.len();
                    let next = frame!().stack[n - 1].clone();
                    let it = frame!().stack[n - 2].clone();
                    let r = self.call(next, it, &[received])?;
                    if !r.is_object() {
                        return self.type_error("Iterator result is not an object");
                    }
                    push!(r);
                }
                Op::IteratorStep => {
                    let n = frame!().stack.len();
                    let next = frame!().stack[n - 1].clone();
                    if next.is_empty() {
                        push!(Value::Undefined);
                    } else {
                        let it = frame!().stack[n - 2].clone();
                        match self.iterator_step(&it, &next)? {
                            Some(v) => push!(v),
                            None => {
                                frame!().stack[n - 1] = Value::Empty;
                                push!(Value::Undefined);
                            }
                        }
                    }
                }
                Op::IteratorRest => {
                    let n = frame!().stack.len();
                    let next = frame!().stack[n - 1].clone();
                    let mut items = Vec::new();
                    if !next.is_empty() {
                        let it = frame!().stack[n - 2].clone();
                        while let Some(v) = self.iterator_step(&it, &next)? {
                            items.push(v);
                        }
                        frame!().stack[n - 1] = Value::Empty;
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
                    let f = self.frames.pop().unwrap();
                    return Ok(Flow::Exit(Exit::Yield(v, Box::new(f))));
                }
                Op::Await => {
                    let v = pop!();
                    let f = self.frames.pop().unwrap();
                    return Ok(Flow::Exit(Exit::Await(v, Box::new(f))));
                }
                Op::StoreCompletion => {
                    let v = pop!();
                    frame!().completion = v;
                }
                Op::LoadCompletion => {
                    let v = frame!().completion.clone();
                    push!(v);
                }
                Op::Debugger | Op::Nop => {}
                Op::JumpUnlessLt(_)
                | Op::JumpUnlessLtEq(_)
                | Op::JumpUnlessGt(_)
                | Op::JumpUnlessGtEq(_)
                | Op::JumpIfStrictNotEq(_)
                | Op::IncReg(_)
                | Op::DecReg(_)
                | Op::GetReg2(..)
                | Op::BinConst(..)
                | Op::JumpUnlessLtConst(..) => {
                    unreachable!("handled by the hot loop")
                }
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
        let key = match key {
            // Integer keys: cached index strings, no number formatting.
            Value::Number(n) if (n as u32) as f64 == n && n < 4294967295.0 => PropertyKey::index(n as u32),
            other => self.to_property_key(other)?,
        };
        self.get(obj, key)
    }

    /// `obj[key]` for dense array elements and string characters.
    #[inline]
    pub(crate) fn get_elem_fast(&self, obj: &Value, key: &Value) -> Option<Value> {
        let Value::Number(n) = key else { return None };
        let i = *n as u32;
        if i as f64 != *n {
            return None;
        }
        match obj {
            Value::Object(id) => match &self.heap.try_get(*id)?.kind {
                ObjectKind::Array { elements, .. } => match elements.get(i as usize) {
                    Some(v) if !v.is_empty() => Some(v.clone()),
                    _ => None,
                },
                _ => None,
            },
            Value::String(s) if (i as usize) < s.len() => Some(Value::String(s.char_at(i as usize))),
            _ => None,
        }
    }

    /// `obj[key] = v` for an existing dense element or an append; gives `v`
    /// back when not applicable.
    #[inline]
    pub(crate) fn set_elem_fast(&mut self, obj: &Value, key: &Value, v: Value) -> Result<(), Value> {
        let (Value::Object(id), Value::Number(n)) = (obj, key) else { return Err(v) };
        let i = *n as u32;
        if i as f64 != *n || i >= (1 << 28) {
            return Err(v);
        }
        let Some(o) = self.heap.try_get_mut(*id) else { return Err(v) };
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
        let id = match obj {
            Value::Object(id) => id,
            Value::Undefined | Value::Null => return self.type_error("Cannot convert undefined or null to object"),
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
    fn take_args(&mut self, fi: usize, from: usize) -> Vec<Value> {
        let mut buf = std::mem::take(&mut self.arg_buf);
        buf.extend(self.frames[fi].stack.drain(from..));
        buf
    }

    #[inline]
    fn give_args(&mut self, mut buf: Vec<Value>) {
        buf.clear();
        if buf.capacity() <= 64 {
            self.arg_buf = buf;
        }
    }

    fn do_call(&mut self, f: Value, this: Value, args: &[Value]) -> JsResult<()> {
        if let Value::Object(id) = &f {
            match &self.obj(*id).kind {
                ObjectKind::Function(c) => {
                    let flags = c.code.flags;
                    if !flags.is_generator && !flags.is_async && !flags.is_class_constructor {
                        let frame = self.prepare_frame(*id, this, args, Value::Undefined, false)?;
                        self.push_frame(frame, false)?;
                        return Ok(());
                    }
                }
                ObjectKind::Native(n) => {
                    let func = n.func;
                    let v = self.call_native(*id, func, this, args, false)?;
                    self.frames.last_mut().unwrap().stack.push(v);
                    return Ok(());
                }
                _ => {}
            }
        }
        let v = self.call(f, this, args)?;
        self.frames.last_mut().unwrap().stack.push(v);
        Ok(())
    }

    fn do_construct(&mut self, f: Value, args: &[Value]) -> JsResult<()> {
        if let Value::Object(id) = &f {
            if let ObjectKind::Function(_) = &self.obj(*id).kind {
                if self.is_constructor(&f) {
                    let frame = self.prepare_frame(*id, Value::Empty, args, f.clone(), true)?;
                    self.push_frame(frame, false)?;
                    return Ok(());
                }
            }
        }
        let v = self.construct(f.clone(), args, f)?;
        self.frames.last_mut().unwrap().stack.push(v);
        Ok(())
    }

    fn direct_eval(&mut self, arg: Value, strict: bool) -> JsResult<Value> {
        let src = match arg {
            Value::String(s) => s.to_rust_string(),
            other => return Ok(other),
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
        match proto {
            Some(p) => self.get_from_object(p, &key, this),
            None => Ok(Value::Undefined),
        }
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
        let sid = match src {
            Value::Undefined | Value::Null | Value::Empty => return Ok(()),
            Value::Object(id) => id,
            prim => self.to_object(prim)?,
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
        if let Value::Object(id) = f {
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
            let existing = self.obj(*id).props.get(&PropertyKey::str("name")).map(|p| p.value.clone());
            let keep = matches!(&existing, Some(Value::String(s)) if !s.is_empty());
            if !keep {
                self.define_property(*id, "name", Value::str(&name), CONFIGURABLE);
            }
        }
    }

    /// Create a function object for `code` in the context of frame `fi`.
    pub(crate) fn make_closure(&mut self, fi: usize, code: Rc<FunctionCode>) -> ObjectId {
        let frame = &self.frames[fi];
        let flags = code.flags;
        let closure = Closure {
            code: code.clone(),
            scope: Some(frame.scope),
            this_value: if flags.is_arrow { Some(frame.this.clone()) } else { None },
            new_target: if flags.is_arrow { Some(frame.new_target.clone()) } else { None },
            home_object: if flags.is_arrow { frame.home_object } else { None },
            fields: None,
            arguments: if flags.is_arrow { frame.arguments_obj.clone() } else { None },
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
        let (proto_parent, ctor_parent) = match &superclass {
            Value::Empty => (Some(self.intrinsics.object_proto), self.intrinsics.function_proto),
            Value::Null => (None, self.intrinsics.function_proto),
            Value::Object(s) if self.is_constructor(&superclass) => {
                let pp = self.get(superclass.clone(), names::prototype())?;
                let pp = match pp {
                    Value::Object(p) => Some(p),
                    Value::Null => None,
                    _ => return self.type_error("Class extends value does not have valid prototype property"),
                };
                (pp, *s)
            }
            _ => {
                let d = self.describe_for_error(&superclass);
                return self.type_error(&format!("Class extends value {} is not a constructor or null", d));
            }
        };
        let ctor = self.make_closure(fi, code);
        self.obj_mut(ctor).proto = Some(ctor_parent);
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
        let object = match v {
            Value::Undefined | Value::Null | Value::Empty => None,
            Value::Object(id) => Some(id),
            prim => Some(self.to_object(prim)?),
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
        if matches!(pa, Value::String(_)) || matches!(pb, Value::String(_)) {
            let sa = self.to_string(pa)?;
            let sb = self.to_string(pb)?;
            return Ok(Value::String(sa.concat(&sb)));
        }
        let na = self.to_numeric(pa)?;
        let nb = self.to_numeric(pb)?;
        match (na, nb) {
            (Value::Number(x), Value::Number(y)) => Ok(Value::Number(x + y)),
            (Value::BigInt(x), Value::BigInt(y)) => Ok(Value::BigInt(Rc::new(x.add(&y)))),
            _ => self.type_error("Cannot mix BigInt and other types, use explicit conversions"),
        }
    }

    fn numeric_binop(&mut self, op: Op, a: Value, b: Value) -> JsResult<Value> {
        let na = self.to_numeric(a)?;
        let nb = self.to_numeric(b)?;
        match (na, nb) {
            (Value::Number(x), Value::Number(y)) => Ok(Value::Number(num_binop(op, x, y))),
            (Value::BigInt(x), Value::BigInt(y)) => {
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
        if let (Value::String(x), Value::String(y)) = (&pa, &pb) {
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
        let ord = match (&na, &nb) {
            (Value::BigInt(x), Value::BigInt(y)) => Some(x.cmp(y)),
            (Value::BigInt(x), Value::Number(y)) => x.cmp_f64(*y),
            (Value::Number(x), Value::BigInt(y)) => y.cmp_f64(*x).map(|o| o.reverse()),
            _ => None,
        };
        if let (Value::BigInt(_), _) | (_, Value::BigInt(_)) = (&na, &nb) {
            let Some(o) = ord else { return Ok(false) };
            return Ok(match op {
                Op::Lt => o.is_lt(),
                Op::LtEq => o.is_le(),
                Op::Gt => o.is_gt(),
                _ => o.is_ge(),
            });
        }
        let (x, y) = match (&na, &nb) {
            (Value::Number(x), Value::Number(y)) => (*x, *y),
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
    if let Some(Value::Object(g)) = gen {
        let v = args.first().cloned().unwrap_or(Value::Undefined);
        realm.async_step_dispatch(g, ResumeMode::Next, v);
    }
    Ok(Value::Undefined)
}

fn on_await_rejected(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let gen = realm.current_native().and_then(|f| realm.native_captures(f).first().cloned());
    if let Some(Value::Object(g)) = gen {
        let v = args.first().cloned().unwrap_or(Value::Undefined);
        realm.async_step_dispatch(g, ResumeMode::Throw, v);
    }
    Ok(Value::Undefined)
}
