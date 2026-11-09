//! The bytecode interpreter: frames, calls, generators, async and exceptions.

use crate::bytecode::{FunctionCode, Op};
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
        let scope = self.alloc(Object::new(None, ObjectKind::Scope(Box::new(ScopeData { slots: vec![Value::Undefined; slots], parent: None, consts: vec![] }))));
        let this = if code.flags.is_script { Value::Object(self.global) } else { Value::Undefined };
        let frame = Frame {
            code,
            pc: 0,
            stack: Vec::with_capacity(16),
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
                let proto = match self.get(new_target.clone(), PropertyKey::str("prototype"))? {
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
        let mut slots = vec![Value::Undefined; nslots];
        for (i, &slot) in code.param_slots.iter().enumerate() {
            slots[slot as usize] = args.get(i).cloned().unwrap_or(Value::Undefined);
        }
        let mut arguments_obj = None;
        if let Some(rs) = code.rest_slot {
            let start = code.param_slots.len();
            let rest: Vec<Value> = if args.len() > start { args[start..].to_vec() } else { Vec::new() };
            slots[rs as usize] = self.new_array_value(rest);
        }
        if let Some(s) = code.self_slot {
            slots[s as usize] = Value::Object(id);
        }
        if let Some(a) = code.arguments_slot {
            let ao = self.create_arguments_object(args, if flags.is_strict { None } else { Some(id) });
            slots[a as usize] = ao.clone();
            arguments_obj = Some(ao);
        } else if flags.is_arrow {
            arguments_obj = arrow_args;
        }
        let scope = self.alloc(Object::new(None, ObjectKind::Scope(Box::new(ScopeData { slots, parent: parent_scope, consts: vec![] }))));
        Ok(Frame {
            code,
            pc: 0,
            stack: Vec::with_capacity(16),
            scope,
            scope_depth: 0,
            this,
            func: Some(id),
            new_target,
            home_object: home,
            args: if flags.is_generator || flags.is_async { Vec::new() } else { Vec::new() },
            arguments_obj,
            generator: None,
            is_construct,
            entry: false,
        })
    }

    fn create_arguments_object(&mut self, args: &[Value], callee: Option<ObjectId>) -> Value {
        let proto = self.intrinsics.object_proto;
        let id = self.alloc(Object::new(Some(proto), ObjectKind::Arguments));
        for (i, a) in args.iter().enumerate() {
            self.define_property(id, PropertyKey::index(i as u32), a.clone(), ALL);
        }
        self.define_property(id, "length", Value::Number(args.len() as f64), HIDDEN);
        if let Some(c) = callee {
            self.define_property(id, "callee", Value::Object(c), HIDDEN);
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
            match self.get(Value::Object(id), PropertyKey::str("prototype"))? {
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
            let handler = frame.code.handlers.iter().find(|h| h.start <= pc && pc < h.end).copied();
            if let Some(h) = handler {
                frame.stack.truncate(h.stack_depth as usize);
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
            ObjectKind::Scope(sd) => sd.slots[slot as usize].clone(),
            _ => Value::Undefined,
        }
    }

    #[inline]
    fn slot_set(&mut self, scope: ObjectId, depth: u16, slot: u16, v: Value) {
        let s = self.scope_at(scope, depth);
        if let ObjectKind::Scope(sd) = &mut self.heap.get_mut(s).kind {
            sd.slots[slot as usize] = v;
        }
    }

    fn run_inner(&mut self) -> JsResult<Exit> {
        let mut counter: u32 = 0;
        loop {
            counter = counter.wrapping_add(1);
            if counter & 0x3ff == 0 {
                self.maybe_gc();
            }
            let fi = self.frames.len() - 1;
            let op = {
                let frame = &mut self.frames[fi];
                let op = frame.code.ops[frame.pc];
                frame.pc += 1;
                op
            };
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
                Op::GetGlobal(i) => {
                    let key = const_key!(i);
                    let v = self.get_global_binding(&key)?;
                    push!(v);
                }
                Op::SetGlobal(i) => {
                    let v = pop!();
                    let key = const_key!(i);
                    let strict = frame!().code.flags.is_strict;
                    self.set_global_binding(&key, v, strict)?;
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
                Op::PushScope(n) => {
                    let parent = frame!().scope;
                    let s = self.alloc(Object::new(None, ObjectKind::Scope(Box::new(ScopeData { slots: vec![Value::Undefined; n as usize], parent: Some(parent), consts: vec![] }))));
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
                    let (slots, parent) = match &self.heap.get(cur).kind {
                        ObjectKind::Scope(s) => (s.slots.clone(), s.parent),
                        _ => unreachable!(),
                    };
                    let s = self.alloc(Object::new(None, ObjectKind::Scope(Box::new(ScopeData { slots, parent, consts: vec![] }))));
                    frame!().scope = s;
                }
                Op::GetProp(i) => {
                    let obj = pop!();
                    let key = const_key!(i);
                    let v = self.get(obj, key)?;
                    push!(v);
                }
                Op::GetPropKeep(i) => {
                    let obj = peek!().clone();
                    let key = const_key!(i);
                    let v = self.get(obj, key)?;
                    push!(v);
                }
                Op::SetProp(i) => {
                    let v = pop!();
                    let obj = pop!();
                    let key = const_key!(i);
                    let ok = self.set(obj, key.clone(), v.clone())?;
                    if !ok && frame!().code.flags.is_strict {
                        let k = self.key_display(&key);
                        return self.type_error(&format!("Cannot assign to read only property '{}' of object", k));
                    }
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
                Op::DefineProp(i) => {
                    let v = pop!();
                    let obj = peek!().as_object().unwrap();
                    let key = const_key!(i);
                    self.define_own(obj, key, Property::data(v, ALL))?;
                }
                Op::DefineElem => {
                    let v = pop!();
                    let key = pop!();
                    let obj = peek!().as_object().unwrap();
                    let key = self.to_property_key(key)?;
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
                        Some(p) if p.is_accessor() => (p.getter.clone(), p.setter.clone()),
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
                Op::Call(argc) | Op::CallEval(argc) => {
                    let n = frame!().stack.len();
                    let base = n - argc as usize - 2;
                    let args: Vec<Value> = frame!().stack.drain(base + 2..).collect();
                    let this = pop!();
                    let f = pop!();
                    if let Op::CallEval(_) = op {
                        if f.as_object() == Some(self.intrinsics.eval_fn) {
                            let strict = frame!().code.flags.is_strict;
                            let v = self.direct_eval(args.first().cloned().unwrap_or(Value::Undefined), strict)?;
                            push!(v);
                            continue;
                        }
                    }
                    self.do_call(f, this, args)?;
                }
                Op::CallSpread => {
                    let arr = pop!();
                    let this = pop!();
                    let f = pop!();
                    let args = self.create_list_from_array_like(arr)?;
                    self.do_call(f, this, args)?;
                }
                Op::New(argc) => {
                    let n = frame!().stack.len();
                    let base = n - argc as usize - 1;
                    let args: Vec<Value> = frame!().stack.drain(base + 1..).collect();
                    let f = pop!();
                    self.do_construct(f, args)?;
                }
                Op::NewSpread => {
                    let arr = pop!();
                    let f = pop!();
                    let args = self.create_list_from_array_like(arr)?;
                    self.do_construct(f, args)?;
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
                        return Ok(Exit::Return(v));
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
                    push!(Value::Bool(if op == Op::Eq { r } else { !r }));
                }
                Op::StrictEq | Op::StrictNotEq => {
                    let b = pop!();
                    let a = pop!();
                    let r = self.strict_equals(&a, &b);
                    push!(Value::Bool(if op == Op::StrictEq { r } else { !r }));
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
                            Value::BigInt(b) => Value::BigInt(Rc::new(BigInt(-b.0))),
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
                        Value::BigInt(b) => Value::BigInt(Rc::new(BigInt(!b.0))),
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
                    let d = if op == Op::Inc { 1.0 } else { -1.0 };
                    let r = match v {
                        Value::Number(n) => Value::Number(n + d),
                        Value::BigInt(b) => Value::BigInt(Rc::new(BigInt(b.0 + d as i128))),
                        other => match self.to_numeric(other)? {
                            Value::Number(n) => Value::Number(n + d),
                            Value::BigInt(b) => Value::BigInt(Rc::new(BigInt(b.0 + d as i128))),
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
                        Value::String(_) | Value::Symbol(_) => v,
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
                Op::Callee => {
                    let v = frame!().func.map(Value::Object).unwrap_or(Value::Undefined);
                    push!(v);
                }
                Op::ImportMeta => {
                    let o = self.new_object();
                    push!(Value::Object(o));
                }
                Op::DynamicImport => {
                    pop!();
                    let (p, _, reject) = self.new_promise();
                    let e = self.new_error("TypeError", "Dynamic import is not supported");
                    self.call(Value::Object(reject), Value::Undefined, &[e])?;
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
                    return Ok(Exit::Yield(v, Box::new(f)));
                }
                Op::Await => {
                    let v = pop!();
                    let f = self.frames.pop().unwrap();
                    return Ok(Exit::Await(v, Box::new(f)));
                }
                Op::Debugger | Op::Nop => {}
            }
        }
    }

    // ------------------------------------------------------------ helpers

    fn slot_name(&self, fi: usize, depth: u16, slot: u16) -> String {
        let frame = &self.frames[fi];
        if depth as u32 == frame.scope_depth {
            if let Some(n) = frame.code.slot_names.get(slot as usize) {
                if !n.is_empty() {
                    return n.to_rust_string();
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
        // Fast path: array[index]
        if let (Value::Object(id), Value::Number(n)) = (&obj, &key) {
            if *n >= 0.0 && n.fract() == 0.0 && *n < 4294967295.0 {
                if let ObjectKind::Array { elements, .. } = &self.obj(*id).kind {
                    let i = *n as usize;
                    if i < elements.len() && !elements[i].is_empty() {
                        return Ok(elements[i].clone());
                    }
                }
            }
        }
        if let (Value::String(s), Value::Number(n)) = (&obj, &key) {
            if *n >= 0.0 && n.fract() == 0.0 && (*n as usize) < s.len() {
                let i = *n as usize;
                return Ok(Value::String(s.slice(i, i + 1)));
            }
        }
        let key = self.to_property_key(key)?;
        self.get(obj, key)
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

    fn do_call(&mut self, f: Value, this: Value, args: Vec<Value>) -> JsResult<()> {
        if let Value::Object(id) = &f {
            if let ObjectKind::Function(c) = &self.obj(*id).kind {
                let flags = c.code.flags;
                if !flags.is_generator && !flags.is_async && !flags.is_class_constructor {
                    let frame = self.prepare_frame(*id, this, &args, Value::Undefined, false)?;
                    self.push_frame(frame, false)?;
                    return Ok(());
                }
            }
        }
        let v = self.call(f, this, &args)?;
        self.frames.last_mut().unwrap().stack.push(v);
        Ok(())
    }

    fn do_construct(&mut self, f: Value, args: Vec<Value>) -> JsResult<()> {
        if let Value::Object(id) = &f {
            if let ObjectKind::Function(_) = &self.obj(*id).kind {
                if self.is_constructor(&f) {
                    let frame = self.prepare_frame(*id, Value::Empty, &args, f.clone(), true)?;
                    self.push_frame(frame, false)?;
                    return Ok(());
                }
            }
        }
        let v = self.construct(f.clone(), &args, f)?;
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
        self.define_property(id, "length", Value::Number(code.param_count as f64), CONFIGURABLE);
        self.define_property(id, "name", Value::String(code.name.clone()), CONFIGURABLE);
        if flags.is_generator {
            let gp = if flags.is_async { self.intrinsics.async_generator_proto } else { self.intrinsics.generator_proto };
            let p = self.alloc(Object::new(Some(gp), ObjectKind::Ordinary));
            self.define_property(id, "prototype", Value::Object(p), WRITABLE);
        } else if !flags.is_arrow && !flags.is_method && !flags.is_async && !flags.is_class_constructor {
            let p = self.new_object();
            self.define_property(p, "constructor", Value::Object(id), HIDDEN);
            self.define_property(id, "prototype", Value::Object(p), WRITABLE);
        }
        id
    }

    fn make_class(&mut self, fi: usize, code: Rc<FunctionCode>, superclass: Value) -> JsResult<(ObjectId, ObjectId)> {
        let (proto_parent, ctor_parent) = match &superclass {
            Value::Empty => (Some(self.intrinsics.object_proto), self.intrinsics.function_proto),
            Value::Null => (None, self.intrinsics.function_proto),
            Value::Object(s) if self.is_constructor(&superclass) => {
                let pp = self.get(superclass.clone(), PropertyKey::str("prototype"))?;
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

    fn for_in_start(&mut self, v: Value) -> JsResult<ObjectId> {
        let mut keys: Vec<JsStr> = Vec::new();
        let object = match v {
            Value::Undefined | Value::Null | Value::Empty => None,
            Value::Object(id) => Some(id),
            prim => Some(self.to_object(prim)?),
        };
        if let Some(id) = object {
            let mut seen: std::collections::HashSet<JsStr> = std::collections::HashSet::new();
            let mut cur = Some(id);
            while let Some(c) = cur {
                let own = self.own_keys(c)?;
                for k in own {
                    if let PropertyKey::String(s) = &k {
                        if seen.contains(s) {
                            continue;
                        }
                        seen.insert(s.clone());
                        let enumerable = self.get_own_property_any(c, &k)?.map(|p| p.enumerable()).unwrap_or(false);
                        if enumerable {
                            keys.push(s.clone());
                        }
                    }
                }
                cur = self.get_prototype_of(c)?;
            }
        }
        let proto = self.intrinsics.object_proto;
        Ok(self.alloc(Object::new(Some(proto), ObjectKind::ForInIterator { keys, index: 0, object })))
    }

    fn for_in_next(&mut self, it: ObjectId) -> JsResult<Option<JsStr>> {
        loop {
            let (key, object) = match &mut self.obj_mut(it).kind {
                ObjectKind::ForInIterator { keys, index, object } => {
                    if *index >= keys.len() {
                        return Ok(None);
                    }
                    let k = keys[*index].clone();
                    *index += 1;
                    (k, *object)
                }
                _ => return Ok(None),
            };
            // Skip keys deleted during iteration.
            if let Some(o) = object {
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
            (Value::BigInt(x), Value::BigInt(y)) => match x.0.checked_add(y.0) {
                Some(r) => Ok(Value::BigInt(Rc::new(BigInt(r)))),
                None => self.range_error("BigInt overflow"),
            },
            _ => self.type_error("Cannot mix BigInt and other types, use explicit conversions"),
        }
    }

    fn numeric_binop(&mut self, op: Op, a: Value, b: Value) -> JsResult<Value> {
        let na = self.to_numeric(a)?;
        let nb = self.to_numeric(b)?;
        match (na, nb) {
            (Value::Number(x), Value::Number(y)) => Ok(Value::Number(num_binop(op, x, y))),
            (Value::BigInt(x), Value::BigInt(y)) => {
                let (x, y) = (x.0, y.0);
                let r = match op {
                    Op::Sub => x.checked_sub(y),
                    Op::Mul => x.checked_mul(y),
                    Op::Div => {
                        if y == 0 {
                            return self.range_error("Division by zero");
                        }
                        x.checked_div(y)
                    }
                    Op::Mod => {
                        if y == 0 {
                            return self.range_error("Division by zero");
                        }
                        x.checked_rem(y)
                    }
                    Op::Exp => {
                        if y < 0 {
                            return self.range_error("Exponent must be non-negative");
                        }
                        x.checked_pow(y as u32)
                    }
                    Op::BitAnd => Some(x & y),
                    Op::BitOr => Some(x | y),
                    Op::BitXor => Some(x ^ y),
                    Op::Shl => x.checked_shl(y as u32),
                    Op::Shr => Some(x >> (y.clamp(0, 127) as u32)),
                    Op::UShr => return self.type_error("BigInts have no unsigned right shift, use >> instead"),
                    _ => None,
                };
                match r {
                    Some(v) => Ok(Value::BigInt(Rc::new(BigInt(v)))),
                    None => self.range_error("BigInt overflow"),
                }
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
        let (x, y) = match (&na, &nb) {
            (Value::Number(x), Value::Number(y)) => (*x, *y),
            (Value::BigInt(x), Value::BigInt(y)) => (x.0 as f64, y.0 as f64),
            (Value::BigInt(x), Value::Number(y)) => (x.0 as f64, *y),
            (Value::Number(x), Value::BigInt(y)) => (*x, y.0 as f64),
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
        Op::Mod => {
            if y.is_infinite() && x.is_finite() {
                x
            } else {
                x % y
            }
        }
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
