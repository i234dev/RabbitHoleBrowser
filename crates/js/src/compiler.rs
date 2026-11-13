//! AST → bytecode compiler with static scope resolution.
//!
//! Each function gets its own `FunctionCode`. Variables resolve at compile
//! time to `(depth, slot)` pairs addressing runtime scope objects; blocks
//! that declare lexical bindings push their own scope. `try/finally` is
//! compiled by inlining the finalizer at every exit (return / break /
//! continue / fall-through) plus a catch-all handler that rethrows.

use crate::ast::*;
use crate::bytecode::{CodeFlags, FunctionCode, Handler, Op, VarLoc};
use crate::captures::CaptureMap;
use crate::value::{FxBuild, JsStr, Value};
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq)]
pub struct CompileError {
    pub message: String,
    pub pos: Pos,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SyntaxError: {} ({}:{})", self.message, self.pos.line, self.pos.column)
    }
}

type CResult<T> = Result<T, CompileError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BindKind {
    Var,
    Let,
    Const,
    Function,
    Param,
    Class,
    /// Hidden compiler temporaries (never TDZ-checked).
    Hidden,
    /// Private name symbol.
    Private,
    /// Module import binding: the slot holds a reference to the exporter's
    /// binding (or a namespace object), written when the module is linked.
    Import,
}

/// Where a binding lives: a frame register (never captured by a nested
/// function) or a slot of the runtime scope object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Loc {
    Reg(u16),
    Slot(u16),
}

impl Loc {
    fn to_var_loc(self) -> VarLoc {
        match self {
            Loc::Reg(r) => VarLoc::Reg(r),
            Loc::Slot(s) => VarLoc::Slot(s),
        }
    }
}

/// Placeholder location of script-level lexical names (they are globals).
const SCRIPT_LEX: Loc = Loc::Slot(u16::MAX);

#[derive(Debug, Clone)]
struct Binding {
    loc: Loc,
    kind: BindKind,
    /// A register `let`/`const`/`class` binding whose declaration has been
    /// compiled: later references (in compile order, which within one
    /// block instance is execution order) need no TDZ check.
    initialized: bool,
}

#[derive(Debug)]
struct CScope {
    names: HashMap<String, Binding, FxBuild>,
    /// Heap slots of the runtime scope object.
    slot_count: u16,
    /// Creates a runtime scope object.
    materialized: bool,
    is_function: bool,
    /// Every binding of this scope lives on the heap (class scopes, module
    /// environments).
    force_heap: bool,
    /// Lexical bindings needing TDZ initialisation at entry.
    tdz: Vec<Loc>,
    /// The shared scope of a switch's case clauses: a jump to a later
    /// case can skip a declaration, so its bindings always keep TDZ checks.
    is_switch: bool,
    /// Snapshot for lazily compiled nested functions, with the
    /// (slot count, materialized, name count) it was taken at.
    snap: Option<((u16, bool, usize), Rc<LazyScope>)>,
    /// The scope of a `with` body: the hidden binding holding its object.
    /// Names that are not bound inside it are looked up on that object
    /// first (see [`Compiler::with_chain`]).
    with_name: Option<String>,
}

/// Compile-time view of an enclosing scope kept for a lazily compiled
/// function: its heap bindings (nested functions never see registers: the
/// capture analysis keeps every name they mention in heap slots).
#[derive(Debug)]
pub struct LazyScope {
    names: HashMap<String, (u16, BindKind), FxBuild>,
    materialized: bool,
    parent: Option<Rc<LazyScope>>,
    /// A function's top scope (a var scope).
    is_function: bool,
    /// The scope's `with` object binding (see [`CScope::with_name`]).
    with_name: Option<String>,
    /// The top scope of a script (its names are globals).
    is_script: bool,
}

/// Hidden binding of a sloppy function that contains a direct `eval`: an
/// object (null prototype) holding the `var`s that eval code declares in
/// the function's variable environment.
pub const EVAL_VARS: &str = "%evalvars";

impl EvalSite {
    /// Private names (without `#`) of the classes enclosing the call.
    pub fn private_names(&self) -> Vec<crate::ast::Name> {
        let mut out = Vec::new();
        let mut node = self.env.clone();
        while let Some(n) = node {
            for k in n.names.keys() {
                if let Some(p) = k.strip_prefix('#') {
                    out.push(p.into());
                }
            }
            node = n.parent.clone();
        }
        out
    }
}

/// What a direct `eval` call site knows at compile time.
#[derive(Debug)]
pub struct EvalSite {
    /// The scope chain at the call (heap bindings, with names).
    pub env: Option<Rc<LazyScope>>,
    pub strict: bool,
    pub class_depth: u32,
    /// Context flags for early errors in the eval code.
    pub in_function: bool,
    pub in_method: bool,
    pub in_derived_ctor: bool,
    pub in_field_init: bool,
    /// The call is in a parameter initializer of a non-arrow function:
    /// eval code may not declare `var arguments`.
    pub in_params: bool,
}

/// What a lazy [`FunctionCode`] keeps to compile its function on first
/// call.
#[derive(Debug)]
pub struct LazyCode {
    func: Rc<Function>,
    name: String,
    env: Option<Rc<LazyScope>>,
    parent_strict: bool,
    class_depth: u32,
    module: Option<u32>,
    filename: Rc<str>,
    source: Rc<str>,
}

impl CScope {
    fn new(materialized: bool, is_function: bool) -> CScope {
        CScope { names: HashMap::default(), slot_count: 0, materialized, is_function, force_heap: false, tdz: Vec::new(), is_switch: false, snap: None, with_name: None }
    }
}

#[derive(Debug, Clone)]
enum ExitCtx {
    /// A runtime scope that must be popped when jumping out.
    Scope,
    /// Inline this finalizer. The index identifies the owning try context
    /// (whose handler ranges are suspended while the finalizer is inlined).
    /// `scopes` is the number of compile-time scopes outside the try:
    /// the finalizer must not see bindings declared inside it.
    Finally { body: Rc<Vec<Stmt>>, try_index: usize, scopes: usize },
    /// Close the for-of iterator held in hidden slots.
    IterClose { iter: u16, next: u16 },
    /// Close an array destructuring's iterator unless it is done.
    DestructClose { iter: u16, next: u16 },
}

#[derive(Debug)]
struct TryCtx {
    /// Completed ranges + an open start.
    ranges: Vec<(u32, u32)>,
    open_start: Option<u32>,
    /// `exits.len()` when the try started: a jump to exit depth `d` leaves
    /// this try when `exit_depth >= d`.
    exit_depth: usize,
}

#[derive(Debug)]
struct LoopCtx {
    label: Option<String>,
    is_loop: bool,
    is_switch: bool,
    break_jumps: Vec<usize>,
    continue_jumps: Vec<usize>,
    /// Exit-context depth to unwind to when breaking out.
    break_depth: usize,
    /// Exit-context depth to unwind to when continuing.
    continue_depth: usize,
}

struct FnState {
    /// Byte range of the function's source text.
    span: Option<(u32, u32)>,
    ops: Vec<Op>,
    positions: Vec<Pos>,
    constants: Vec<Value>,
    const_strings: HashMap<String, u32, FxBuild>,
    functions: Vec<Rc<FunctionCode>>,
    handlers: Vec<Handler>,
    templates: Vec<(Vec<Option<JsStr>>, Vec<JsStr>)>,
    block_names: Vec<Rc<Vec<JsStr>>>,
    scopes: Vec<CScope>,
    exits: Vec<ExitCtx>,
    tries: Vec<TryCtx>,
    loops: Vec<LoopCtx>,
    flags: CodeFlags,
    name: JsStr,
    param_count: u32,
    param_locs: Vec<Loc>,
    rest_loc: Option<Loc>,
    arguments_loc: Option<Loc>,
    self_loc: Option<Loc>,
    hidden_counter: u32,
    /// Names referenced from functions nested in this one: bindings with
    /// these names live in heap scopes, all others in registers.
    captured: Rc<crate::captures::NameSet>,
    /// All bindings on the heap (module environment).
    force_heap: bool,
    nregs: u16,
    reg_names: Vec<JsStr>,
    ic_count: u32,
    cur_pos: Pos,
    /// Number of runtime scopes currently pushed within this function.
    scope_depth: u32,
    is_script: bool,
    /// The outer "environment" function of a module (holds the module
    /// scope); it has no implicit `arguments` binding.
    is_module_env: bool,
    filename: Rc<str>,
    source: Option<Rc<str>>,
    class_depth: u32,
    /// Direct eval call sites in this function.
    eval_sites: Vec<Rc<EvalSite>>,
    /// The function compiled from direct eval code.
    is_eval: bool,
    /// Annex B.3.3 block function declarations (by node address) that
    /// also assign a var of the function / script / eval var scope.
    annexb: std::collections::HashSet<usize>,
    /// Index of the scope holding the function's `var`s (1 when parameter
    /// expressions give the body its own variable environment).
    var_scope: usize,
    /// Compiling the parameter initializers.
    in_params: bool,
    /// Inside finally bodies: their statements' values are not the
    /// completion value.
    no_completion: u32,
}

impl FnState {
    fn new(name: &str, flags: CodeFlags, is_script: bool, filename: Rc<str>) -> FnState {
        let mut scopes = Vec::new();
        scopes.push(CScope::new(true, true));
        FnState {
            span: None,
            ops: Vec::new(),
            positions: Vec::new(),
            constants: Vec::new(),
            const_strings: HashMap::default(),
            functions: Vec::new(),
            handlers: Vec::new(),
            templates: Vec::new(),
            block_names: Vec::new(),
            scopes,
            exits: Vec::new(),
            tries: Vec::new(),
            loops: Vec::new(),
            flags,
            name: JsStr::new(name),
            param_count: 0,
            param_locs: Vec::new(),
            rest_loc: None,
            arguments_loc: None,
            self_loc: None,
            hidden_counter: 0,
            captured: Rc::new(crate::captures::NameSet::default()),
            force_heap: false,
            nregs: 0,
            reg_names: Vec::new(),
            ic_count: 0,
            cur_pos: Pos::default(),
            scope_depth: 0,
            is_script,
            is_module_env: false,
            filename,
            source: None,
            class_depth: 0,
            eval_sites: Vec::new(),
            is_eval: false,
            annexb: Default::default(),
            var_scope: 0,
            in_params: false,
            no_completion: 0,
        }
    }
}

pub struct Compiler {
    /// Track the script completion value (`eval` results).
    completion: bool,
    fns: Vec<FnState>,
    filename: Rc<str>,
    source: Rc<str>,
    pending_tweak: Option<Box<dyn FnOnce(&mut CodeFlags)>>,
    chain_stack: Vec<Vec<(usize, u32)>>,
    loose_chain_exits: Vec<(usize, u32)>,
    /// Module id stamped on every produced `FunctionCode` (module mode).
    module: Option<u32>,
    /// Captured-name sets of the program's functions.
    captures: CaptureMap,
    /// Set when the capture analysis missed a reference (a register binding
    /// resolved from a nested function); reported as a compile error.
    internal_error: Option<String>,
    /// Scopes enclosing the outermost function being compiled (lazy
    /// compilation of a nested function).
    outer: Option<Rc<LazyScope>>,
    /// The function a lazy compilation is for (compiled, not deferred).
    eager_root: *const Function,
    /// Defer compiling nested functions until their first call.
    lazy: bool,
}

/// Name of the hidden binding initialised by `export default <expr>` and
/// anonymous `export default function/class`.
pub const DEFAULT_EXPORT_BINDING: &str = "*default*";

/// Output of [`compile_module`].
#[derive(Debug)]
pub struct CompiledModule {
    /// The module "environment" function: run once (with `this` undefined)
    /// it initialises TDZ slots, hoists top-level function declarations
    /// into the module scope and returns the module body as an async arrow
    /// closure over that scope.
    pub code: Rc<FunctionCode>,
    /// Module-scope slot of every top-level binding (locals, imports and
    /// `*default*`), used to wire exports and imports at link time.
    pub slots: HashMap<String, u16>,
}

#[derive(Debug, Clone, Copy)]
enum Resolved {
    Local { depth: u16, slot: u16, kind: BindKind },
    Reg { reg: u16, kind: BindKind },
    Global,
}

/// Lazy compilation of nested functions (on unless `RH_EAGER_COMPILE` is set).
fn lazy_enabled() -> bool {
    thread_local!(static LAZY: bool = std::env::var_os("RH_EAGER_COMPILE").is_none());
    LAZY.with(|l| *l)
}

/// Compile the function of a lazy [`FunctionCode`] (see [`LazyCode`]).
pub fn compile_lazy(code: &FunctionCode) -> CResult<Rc<FunctionCode>> {
    let lz = code.lazy.as_ref().expect("not a lazy function");
    let mut c = Compiler {
        completion: false,
        fns: Vec::new(),
        filename: lz.filename.clone(),
        source: lz.source.clone(),
        pending_tweak: None,
        chain_stack: Vec::new(),
        loose_chain_exits: Vec::new(),
        module: lz.module,
        captures: CaptureMap::default(),
        internal_error: None,
        outer: lz.env.clone(),
        eager_root: Rc::as_ptr(&lz.func),
        lazy: true,
    };
    // A transparent parent: no bindings of its own, not a scope at run
    // time; names resolve through `outer`.
    let mut parent = FnState::new("", CodeFlags { is_strict: lz.parent_strict, ..Default::default() }, true, lz.filename.clone());
    parent.scopes[0].materialized = false;
    parent.class_depth = lz.class_depth;
    c.fns.push(parent);
    if code.flags.is_method {
        // Known while compiling the body (direct eval's `super` checks).
        c.pending_tweak = Some(Box::new(|fl: &mut CodeFlags| fl.is_method = true));
    }
    let idx = c.compile_function(&lz.func, Some(&lz.name))?;
    c.check_internal()?;
    let mut parent = c.fns.pop().unwrap();
    if code.flags.is_method {
        let mut flags = parent.functions[idx as usize].flags;
        flags.is_method = true;
        set_flags(&mut parent.functions[idx as usize], flags);
    }
    Ok(parent.functions[idx as usize].clone())
}

/// Compile a whole script/module.
pub fn compile_program(program: &Program, filename: &str, source: &str) -> CResult<Rc<FunctionCode>> {
    compile_program_with(program, filename, source, true)
}

/// Compile a script; without `completion` its completion value (the
/// result of `eval`) is not tracked and the script evaluates to undefined.
pub fn compile_program_with(program: &Program, filename: &str, source: &str, completion: bool) -> CResult<Rc<FunctionCode>> {
    let mut c = Compiler {
        completion,
        fns: Vec::new(),
        filename: Rc::from(filename),
        source: Rc::from(source),
        pending_tweak: None,
        chain_stack: Vec::new(),
        loose_chain_exits: Vec::new(),
        module: None,
        captures: crate::captures::analyze_program(&program.body),
        internal_error: None,
        outer: None,
        eager_root: std::ptr::null(),
        lazy: lazy_enabled(),
    };
    let code = c.compile_script(program)?;
    c.check_internal()?;
    Ok(code)
}

/// Compile direct `eval` code for the call site `site`: an arrow function
/// (it shares the caller's `this`, `new.target`, `super` and `arguments`)
/// closed over the caller's scope chain, returning the completion value.
/// Sloppy eval code declares its `var`s and functions in the caller's
/// variable environment (EvalDeclarationInstantiation).
pub fn compile_eval(program: &Program, site: &EvalSite, source: &str) -> CResult<Rc<FunctionCode>> {
    let strict = site.strict || program.strict;
    let filename: Rc<str> = Rc::from("eval");
    let mut c = Compiler {
        completion: true,
        fns: Vec::new(),
        filename: filename.clone(),
        source: Rc::from(source),
        pending_tweak: None,
        chain_stack: Vec::new(),
        loose_chain_exits: Vec::new(),
        module: None,
        captures: crate::captures::analyze_program(&program.body),
        internal_error: None,
        outer: site.env.clone(),
        eager_root: std::ptr::null(),
        lazy: lazy_enabled(),
    };
    // A transparent parent (no bindings, no runtime scope).
    let mut parent = FnState::new("", CodeFlags { is_arrow: true, is_strict: strict, ..Default::default() }, false, filename.clone());
    parent.scopes[0].materialized = false;
    parent.scopes[0].is_function = false;
    parent.class_depth = site.class_depth;
    c.fns.push(parent);
    // Indirect eval (no caller scope) runs like a script: `this` is the global object.
    let flags = CodeFlags { is_arrow: true, is_strict: strict, has_simple_params: true, is_script: site.env.is_none(), ..Default::default() };
    let mut fs = FnState::new("", flags, false, filename);
    fs.is_eval = true;
    fs.source = Some(c.source.clone());
    fs.class_depth = site.class_depth;
    fs.captured = c.captures.get(&std::ptr::null()).cloned().unwrap_or_default();
    if fs.captured.contains(crate::captures::EVAL_ALL) {
        fs.force_heap = true;
    }
    // Sloppy eval code has no variable environment of its own.
    fs.scopes[0].is_function = strict;
    c.fns.push(fs);
    c.compile_eval_body(program, strict, site.in_params)?;
    c.check_internal()?;
    let fs = c.fns.pop().unwrap();
    Ok(Rc::new(finish(fs, None)))
}

/// Compile an ECMAScript module (see [`CompiledModule`]). `module_id` is
/// recorded on every function so `import.meta` can find its module.
pub fn compile_module(program: &Program, filename: &str, source: &str, module_id: u32) -> CResult<CompiledModule> {
    let mut c = Compiler {
        fns: Vec::new(),
        filename: Rc::from(filename),
        source: Rc::from(source),
        pending_tweak: None,
        chain_stack: Vec::new(),
        loose_chain_exits: Vec::new(),
        module: Some(module_id),
        captures: crate::captures::analyze_program(&program.body),
        internal_error: None,
        completion: false,
        outer: None,
        eager_root: std::ptr::null(),
        lazy: lazy_enabled(),
    };
    let m = c.compile_module_program(program)?;
    c.check_internal()?;
    Ok(m)
}

impl Compiler {
    fn fs(&mut self) -> &mut FnState {
        self.fns.last_mut().unwrap()
    }
    fn fs_ref(&self) -> &FnState {
        self.fns.last().unwrap()
    }

    fn err<T>(&self, msg: &str) -> CResult<T> {
        Err(CompileError { message: msg.to_string(), pos: self.fs_ref().cur_pos })
    }

    // ------------------------------------------------------------ emission

    fn emit(&mut self, op: Op) -> usize {
        let fs = self.fs();
        fs.ops.push(op);
        fs.positions.push(fs.cur_pos);
        fs.ops.len() - 1
    }

    /// Emit an instruction that owns a fresh inline cache.
    fn emit_ic(&mut self, f: impl FnOnce(u32) -> Op) -> usize {
        let fs = self.fs();
        let ic = fs.ic_count;
        fs.ic_count += 1;
        self.emit(f(ic))
    }

    fn here(&self) -> usize {
        self.fs_ref().ops.len()
    }

    fn patch_jump(&mut self, at: usize) {
        let target = self.here() as u32;
        self.patch_jump_to(at, target);
    }

    fn patch_jump_to(&mut self, at: usize, target: u32) {
        let op = &mut self.fs().ops[at];
        *op = match *op {
            Op::Jump(_) => Op::Jump(target),
            Op::JumpIfFalse(_) => Op::JumpIfFalse(target),
            Op::JumpIfTrue(_) => Op::JumpIfTrue(target),
            Op::JumpIfNullish(_) => Op::JumpIfNullish(target),
            Op::JumpIfNotNullishKeep(_) => Op::JumpIfNotNullishKeep(target),
            Op::JumpIfFalseKeep(_) => Op::JumpIfFalseKeep(target),
            Op::JumpIfTrueKeep(_) => Op::JumpIfTrueKeep(target),
            Op::JumpIfUndefined(_) => Op::JumpIfUndefined(target),
            Op::JumpIfNotUndefined(_) => Op::JumpIfNotUndefined(target),
            Op::JumpIfStrictEq(_) => Op::JumpIfStrictEq(target),
            other => panic!("patching non-jump {:?}", other),
        };
    }

    fn const_str(&mut self, s: &str) -> u32 {
        let fs = self.fs();
        if let Some(&i) = fs.const_strings.get(s) {
            return i;
        }
        let i = fs.constants.len() as u32;
        fs.constants.push(Value::String(JsStr::intern(s)));
        fs.const_strings.insert(s.to_string(), i);
        i
    }

    fn const_num(&mut self, n: f64) -> u32 {
        let fs = self.fs();
        let i = fs.constants.len() as u32;
        fs.constants.push(Value::Number(n));
        i
    }

    fn emit_string(&mut self, s: &str) {
        let i = self.const_str(s);
        self.emit(Op::Const(i));
    }

    fn emit_number(&mut self, n: f64) {
        let i = self.const_num(n);
        self.emit(Op::Const(i));
    }

    fn set_pos(&mut self, pos: Pos) {
        if pos.line != 0 {
            self.fs().cur_pos = pos;
        }
    }

    // --------------------------------------------------------------- scopes

    fn check_internal(&self) -> CResult<()> {
        match &self.internal_error {
            Some(m) => Err(CompileError { message: format!("internal compiler error: {}", m), pos: Pos::default() }),
            None => Ok(()),
        }
    }

    /// Declare `name` in scope `si` of function `fi`, choosing a register
    /// unless a nested function may capture it.
    fn declare_at(&mut self, fi: usize, si: usize, name: &str, kind: BindKind) -> Loc {
        let fs = &mut self.fns[fi];
        if let Some(b) = fs.scopes[si].names.get(name) {
            return b.loc;
        }
        let heap = fs.force_heap || fs.scopes[si].force_heap || crate::captures::is_captured(&fs.captured, name) || fs.nregs == u16::MAX;
        let loc = if heap {
            let scope = &mut fs.scopes[si];
            let s = scope.slot_count;
            scope.slot_count += 1;
            Loc::Slot(s)
        } else {
            let r = fs.nregs;
            fs.nregs += 1;
            fs.reg_names.push(JsStr::new(name));
            Loc::Reg(r)
        };
        let scope = &mut fs.scopes[si];
        if matches!(kind, BindKind::Let | BindKind::Const | BindKind::Class) {
            scope.tdz.push(loc);
        }
        scope.names.insert(name.to_string(), Binding { loc, kind, initialized: false });
        loc
    }

    fn declare(&mut self, name: &str, kind: BindKind) -> Loc {
        let fi = self.fns.len() - 1;
        let si = self.fns[fi].scopes.len() - 1;
        self.declare_at(fi, si, name, kind)
    }

    /// A compiler temporary: always a register of the current function.
    fn hidden_slot(&mut self, tag: &str) -> u16 {
        let fs = self.fs();
        fs.hidden_counter += 1;
        let name = format!("%{}{}", tag, fs.hidden_counter);
        let r = fs.nregs;
        fs.nregs += 1;
        fs.reg_names.push(JsStr::new(&name));
        r
    }

    /// Emit a load of a binding of the current function's top scope.
    fn emit_load_loc(&mut self, loc: Loc) {
        match loc {
            Loc::Reg(r) => self.emit(Op::GetReg(r)),
            Loc::Slot(s) => self.emit(Op::GetLocal(0, s)),
        };
    }

    /// Emit an initialising store (pops) into a binding of the current top scope.
    fn emit_init_loc(&mut self, loc: Loc) {
        match loc {
            Loc::Reg(r) => self.emit(Op::SetReg(r)),
            Loc::Slot(s) => self.emit(Op::InitLocal(0, s)),
        };
    }

    /// Whether `arguments.length` / `arguments[i]` reads in the current
    /// function can be served from the frame's actual arguments: a plain
    /// (non-arrow) function whose `arguments` never escapes into writes,
    /// deletes or direct eval, and that declares no `arguments` binding.
    fn args_direct_ok(&self) -> bool {
        let fs = self.fs_ref();
        if fs.flags.is_arrow || fs.is_script || fs.is_module_env || fs.flags.is_field_init || fs.force_heap {
            return false;
        }
        if fs.captured.contains(crate::captures::ARGS_ESCAPE) {
            return false;
        }
        fs.scopes.iter().all(|sc| sc.names.get("arguments").is_none_or(|b| matches!(b.kind, BindKind::Hidden)))
    }

    fn resolve(&mut self, name: &str) -> Resolved {
        let mut depth: u16 = 0;
        let nfns = self.fns.len();
        for fi in (0..nfns).rev() {
            let nscopes = self.fns[fi].scopes.len();
            for si in (0..nscopes).rev() {
                let scope = &self.fns[fi].scopes[si];
                if let Some(b) = scope.names.get(name) {
                    match b.loc {
                        Loc::Reg(reg) => {
                            if fi != nfns - 1 {
                                // The capture analysis guarantees this never
                                // happens; fail loudly instead of miscompiling.
                                let kind = b.kind;
                                let _ = kind;
                                self.internal_error.get_or_insert_with(|| format!("captured register binding '{}'", name));
                                return Resolved::Global;
                            }
                            return Resolved::Reg { reg, kind: b.kind };
                        }
                        Loc::Slot(slot) => {
                            if !scope.materialized {
                                // Script-level lexical names (globals).
                                continue;
                            }
                            return Resolved::Local { depth, slot, kind: b.kind };
                        }
                    }
                }
                if scope.is_function && name == "arguments" && !self.fns[fi].flags.is_arrow && !self.fns[fi].is_script && !self.fns[fi].is_module_env {
                    // Implicit `arguments` binding.
                    let materialized = scope.materialized;
                    let loc = self.declare_at(fi, si, "arguments", BindKind::Hidden);
                    self.fns[fi].arguments_loc = Some(loc);
                    self.fns[fi].flags.uses_arguments = true;
                    return match loc {
                        Loc::Reg(reg) if fi == nfns - 1 => Resolved::Reg { reg, kind: BindKind::Var },
                        Loc::Reg(_) => {
                            self.internal_error.get_or_insert_with(|| "captured register 'arguments'".to_string());
                            Resolved::Global
                        }
                        Loc::Slot(slot) => {
                            if !materialized {
                                self.internal_error.get_or_insert_with(|| "'arguments' in an unmaterialized scope".to_string());
                            }
                            Resolved::Local { depth, slot, kind: BindKind::Var }
                        }
                    };
                }
                if scope.materialized {
                    depth += 1;
                }
            }
        }
        let mut node = self.outer.clone();
        while let Some(n) = node {
            if let Some(&(slot, kind)) = n.names.get(name) {
                if n.materialized {
                    return Resolved::Local { depth, slot, kind };
                }
                // Script-level lexical names (globals): keep looking.
            }
            if n.materialized {
                depth += 1;
            }
            node = n.parent.clone();
        }
        Resolved::Global
    }

    /// Snapshot of the scope chain at this point of the compilation, for a
    /// nested function compiled later (snapshots are reused while a scope
    /// is unchanged).
    fn env_snapshot(&mut self) -> Option<Rc<LazyScope>> {
        let mut parent = self.outer.clone();
        for fi in 0..self.fns.len() {
            for si in 0..self.fns[fi].scopes.len() {
                let is_script_top = si == 0 && self.fns[fi].is_script;
                let sc = &mut self.fns[fi].scopes[si];
                let fp = (sc.slot_count, sc.materialized, sc.names.len());
                let reuse = match &sc.snap {
                    Some((f, n)) if *f == fp && opt_ptr_eq(&n.parent, &parent) => Some(n.clone()),
                    _ => None,
                };
                let node = match reuse {
                    Some(n) => n,
                    None => {
                        let names = sc
                            .names
                            .iter()
                            .filter_map(|(k, b)| match b.loc {
                                Loc::Slot(s) => Some((k.clone(), (s, b.kind))),
                                Loc::Reg(_) => None,
                            })
                            .collect();
                        let n = Rc::new(LazyScope {
                            names,
                            materialized: sc.materialized,
                            parent: parent.clone(),
                            is_function: sc.is_function,
                            with_name: sc.with_name.clone(),
                            is_script: is_script_top,
                        });
                        sc.snap = Some((fp, n.clone()));
                        n
                    }
                };
                parent = Some(node);
            }
        }
        parent
    }

    /// Compile a function expression / declaration node, or defer it until
    /// its first call (lazy compilation).
    fn compile_function_rc(&mut self, f: &Rc<Function>, name: Option<&str>) -> CResult<u32> {
        if !self.lazy || self.pending_tweak.is_some() || std::ptr::eq(Rc::as_ptr(f), self.eager_root) || self.in_with() {
            return self.compile_function(f, name);
        }
        let fname = f.name.as_deref().or(name).unwrap_or("").to_string();
        let parent_strict = self.fs_ref().flags.is_strict;
        let flags = CodeFlags {
            is_arrow: f.is_arrow,
            is_async: f.is_async,
            is_generator: f.is_generator,
            is_strict: f.strict || parent_strict,
            has_simple_params: f.params.iter().all(|p| matches!(p.pattern, Pattern::Identifier(_)) && !p.rest),
            ..Default::default()
        };
        let mut param_count = 0u32;
        for p in &f.params {
            if p.rest || matches!(p.pattern, Pattern::Assign { .. }) {
                break;
            }
            param_count += 1;
        }
        let env = self.env_snapshot();
        let lazy = LazyCode {
            func: f.clone(),
            name: fname.clone(),
            env,
            parent_strict,
            class_depth: self.fs_ref().class_depth,
            module: self.module,
            filename: self.fs_ref().filename.clone(),
            source: self.source.clone(),
        };
        let code = FunctionCode {
            name: JsStr::new(&fname),
            ops: Vec::new(),
            positions: Vec::new(),
            constants: Vec::new(),
            functions: Vec::new(),
            handlers: Vec::new(),
            param_count,
            scope_slots: 0,
            nregs: 0,
            reg_names: Vec::new(),
            param_locs: Vec::new(),
            rest_loc: None,
            arguments_loc: None,
            self_loc: None,
            flags,
            call_flags: 0,
            slot_names: Vec::new(),
            block_names: Vec::new(),
            templates: Vec::new(),
            ics: Box::new([]),
            call_ics: Box::new([]),
            ic_keys: Box::new([]),
            depths: Vec::new(),
            max_depth: 0,
            source: Some(self.source.clone()),
            source_span: (f.span.1 > f.span.0).then_some(f.span),
            filename: self.fs_ref().filename.clone(),
            module: self.module,
            lazy: Some(Box::new(lazy)),
            eval_sites: Vec::new(),
            compiled: std::cell::OnceCell::new(),
            jit: Default::default(),
        };
        let parent = self.fs();
        parent.functions.push(Rc::new(code));
        Ok((parent.functions.len() - 1) as u32)
    }

    /// Whether the register binding `name` of the current function is
    /// known to be initialized at this point (see `Binding::initialized`).
    fn reg_initialized(&self, name: &str) -> bool {
        for sc in self.fs_ref().scopes.iter().rev() {
            if let Some(b) = sc.names.get(name) {
                return b.initialized;
            }
        }
        false
    }

    fn mark_reg_initialized(&mut self, name: &str) {
        for sc in self.fs().scopes.iter_mut().rev() {
            if let Some(b) = sc.names.get_mut(name) {
                if !sc.is_switch && matches!(b.loc, Loc::Reg(_)) {
                    b.initialized = true;
                }
                return;
            }
        }
    }

    /// Hidden bindings of the `with` objects (innermost first) that a
    /// reference to `name` must consult before its static binding: every
    /// `with` scope between the reference and the scope declaring `name`.
    fn with_chain(&self, name: &str) -> Vec<String> {
        let mut out = Vec::new();
        if name.starts_with('%') || name.starts_with('#') {
            return out;
        }
        if !self.in_with() {
            return out;
        }
        for f in self.fns.iter().rev() {
            for s in f.scopes.iter().rev() {
                if s.names.contains_key(name) {
                    return out;
                }
                if let Some(w) = &s.with_name {
                    out.push(w.clone());
                }
            }
        }
        let mut node = self.outer.clone();
        while let Some(n) = node {
            if n.names.contains_key(name) && !n.is_script {
                return out;
            }
            if let Some(w) = &n.with_name {
                out.push(w.clone());
            }
            node = n.parent.clone();
        }
        out
    }

    /// Inside `with`: is the code being compiled nested in a `with` body
    /// (nested functions are then compiled eagerly, with the `with` scopes
    /// in view)?
    fn in_with(&self) -> bool {
        if self.fns.iter().any(|f| f.scopes.iter().any(|s| s.with_name.is_some())) {
            return true;
        }
        let mut node = self.outer.as_ref();
        while let Some(n) = node {
            if n.with_name.is_some() {
                return true;
            }
            node = n.parent.as_ref();
        }
        false
    }

    /// For each object of `chain`: push it, test `name` on it and jump to
    /// the code `found` emits when present; the returned jumps lead past
    /// the static fallback. Stack is unchanged on the fallthrough path.
    fn emit_with_dispatch(&mut self, name: &str, chain: &[String], found: &mut dyn FnMut(&mut Self, &str, u32)) -> Vec<usize> {
        let i = self.const_str(name);
        let mut ends = Vec::new();
        for w in chain {
            self.emit_get_var(w);
            self.emit(Op::WithHas(i));
            let skip = self.emit(Op::JumpIfFalse(0));
            found(self, w, i);
            ends.push(self.emit(Op::Jump(0)));
            self.patch_jump(skip);
        }
        ends
    }

    fn emit_get_var(&mut self, name: &str) {
        let chain = self.with_chain(name);
        if !chain.is_empty() {
            let ends = self.emit_with_dispatch(name, &chain, &mut |c, w, i| {
                c.emit_get_var(w);
                c.emit_ic(|ic| Op::GetProp(i, ic));
            });
            self.emit_get_var_static(name);
            for j in ends {
                self.patch_jump(j);
            }
            return;
        }
        self.emit_get_var_static(name);
    }

    fn emit_get_var_static(&mut self, name: &str) {
        match self.resolve(name) {
            Resolved::Local { depth, slot, kind } => {
                if kind == BindKind::Import {
                    self.emit(Op::GetImport(depth, slot));
                } else if matches!(kind, BindKind::Let | BindKind::Const | BindKind::Class) {
                    self.emit(Op::GetLocalChecked(depth, slot));
                } else {
                    self.emit(Op::GetLocal(depth, slot));
                }
            }
            Resolved::Reg { reg, kind } => {
                if matches!(kind, BindKind::Let | BindKind::Const | BindKind::Class) && !self.reg_initialized(name) {
                    self.emit(Op::GetRegChecked(reg));
                } else {
                    self.emit(Op::GetReg(reg));
                }
            }
            Resolved::Global => {
                let i = self.const_str(name);
                self.emit_ic(|ic| Op::GetGlobal(i, ic));
            }
        }
    }

    /// Store top of stack (pops) into a variable (assignment semantics).
    fn emit_set_var(&mut self, name: &str) -> CResult<()> {
        let chain = self.with_chain(name);
        if !chain.is_empty() {
            let ends = self.emit_with_dispatch(name, &chain, &mut |c, w, i| {
                c.emit_get_var(w);
                c.emit(Op::Swap);
                c.emit_ic(|ic| Op::SetProp(i, ic));
                c.emit(Op::Pop);
            });
            self.emit_set_var_static(name)?;
            for j in ends {
                self.patch_jump(j);
            }
            return Ok(());
        }
        self.emit_set_var_static(name)
    }

    fn emit_set_var_static(&mut self, name: &str) -> CResult<()> {
        // A named function expression's own name is an immutable binding:
        // assigning to it is ignored (sloppy) or a TypeError (strict).
        let resolved = self.resolve(name);
        let is_self = match (&resolved, self.fs_ref().self_loc) {
            (Resolved::Local { depth: 0, slot, kind: BindKind::Hidden }, Some(Loc::Slot(s))) => *slot == s,
            (Resolved::Reg { reg, kind: BindKind::Hidden }, Some(Loc::Reg(r))) => *reg == r,
            _ => false,
        };
        if is_self && name != "arguments" {
            if self.fs_ref().flags.is_strict {
                self.emit(Op::Pop);
                let m = self.const_str("Assignment to constant variable.");
                self.emit(Op::ThrowConst(1, m));
            } else {
                self.emit(Op::Pop);
            }
            return Ok(());
        }
        match resolved {
            Resolved::Local { depth, slot, kind } => {
                if kind == BindKind::Import {
                    // Imports are immutable bindings.
                    self.emit(Op::Pop);
                    let m = self.const_str("Assignment to constant variable.");
                    self.emit(Op::ThrowConst(1, m));
                } else if kind == BindKind::Const {
                    // Still evaluate TDZ: reading first mirrors engines; then throw.
                    self.emit(Op::GetLocalChecked(depth, slot));
                    self.emit(Op::Pop);
                    let m = self.const_str("Assignment to constant variable.");
                    self.emit(Op::ThrowConst(1, m));
                } else if matches!(kind, BindKind::Let | BindKind::Class) {
                    self.emit(Op::SetLocalChecked(depth, slot));
                } else {
                    self.emit(Op::SetLocal(depth, slot));
                }
            }
            Resolved::Reg { reg, kind } => {
                if kind == BindKind::Const {
                    self.emit(Op::GetRegChecked(reg));
                    self.emit(Op::Pop);
                    let m = self.const_str("Assignment to constant variable.");
                    self.emit(Op::ThrowConst(1, m));
                } else if matches!(kind, BindKind::Let | BindKind::Class) && !self.reg_initialized(name) {
                    self.emit(Op::SetRegChecked(reg));
                } else {
                    self.emit(Op::SetReg(reg));
                }
            }
            Resolved::Global => {
                let i = self.const_str(name);
                self.emit_ic(|ic| Op::SetGlobal(i, ic));
            }
        }
        Ok(())
    }

    /// Initialise a declaration binding (pops).
    fn emit_init_var(&mut self, name: &str) -> CResult<()> {
        match self.resolve(name) {
            Resolved::Local { depth, slot, .. } => {
                self.emit(Op::InitLocal(depth, slot));
            }
            Resolved::Reg { reg, kind } => {
                self.emit(Op::SetReg(reg));
                if matches!(kind, BindKind::Let | BindKind::Const | BindKind::Class) {
                    self.mark_reg_initialized(name);
                }
            }
            Resolved::Global => {
                let i = self.const_str(name);
                // Global lexical declarations use InitGlobalLex; vars use SetGlobal.
                if self.fs_ref().is_script && self.is_global_lex(name) {
                    self.emit(Op::InitGlobalLex(i));
                } else {
                    self.emit_ic(|ic| Op::SetGlobal(i, ic));
                }
            }
        }
        Ok(())
    }

    fn is_global_lex(&self, name: &str) -> bool {
        self.fs_ref().scopes[0].names.get(name).map(|b| matches!(b.kind, BindKind::Let | BindKind::Const | BindKind::Class)).unwrap_or(false)
    }

    /// Enter a block scope declaring `decls`. Bindings captured by nested
    /// functions go to a runtime scope object (returns true when one was
    /// pushed); the others are registers. TDZ bindings are reset to empty.
    fn push_block_scope(&mut self, decls: &[(String, BindKind)]) -> bool {
        self.push_block_scope_with(decls, false)
    }

    fn push_block_scope_with(&mut self, decls: &[(String, BindKind)], force_heap: bool) -> bool {
        let mut scope = CScope::new(false, false);
        scope.force_heap = force_heap;
        self.fs().scopes.push(scope);
        for (n, k) in decls {
            self.declare(n, *k);
        }
        let fs = self.fs();
        let scope = fs.scopes.last_mut().unwrap();
        let n = scope.slot_count;
        let tdz = scope.tdz.clone();
        let materialized = n > 0;
        scope.materialized = materialized;
        if materialized {
            let mut names = vec![JsStr::empty(); n as usize];
            for (name, b) in &scope.names {
                if let Loc::Slot(s) = b.loc {
                    names[s as usize] = JsStr::new(name);
                }
            }
            let names_idx = self.fs_ref().block_names.len() as u16;
            self.fs().block_names.push(Rc::new(names));
            self.emit(Op::PushScope(n, names_idx));
            self.fs().scope_depth += 1;
            self.fs().exits.push(ExitCtx::Scope);
        }
        for loc in tdz {
            self.emit(Op::Empty);
            self.emit_init_loc(loc);
        }
        materialized
    }

    fn pop_block_scope(&mut self, materialized: bool) {
        self.fs().scopes.pop();
        if materialized {
            self.emit(Op::PopScope);
            self.fs().scope_depth -= 1;
            self.fs().exits.pop();
        }
    }

    // ----------------------------------------------------------- hoisting

    fn collect_var_names(stmts: &[Stmt], out: &mut Vec<String>) {
        for s in stmts {
            Self::collect_var_names_stmt(s, out);
        }
    }

    pub(crate) fn collect_pattern_names(p: &Pattern, out: &mut Vec<String>) {
        match p {
            Pattern::Identifier(n) => out.push(n.to_string()),
            Pattern::Expr(_) => {}
            Pattern::Object { properties, rest } => {
                for pr in properties {
                    Self::collect_pattern_names(&pr.value, out);
                }
                if let Some(r) = rest {
                    Self::collect_pattern_names(r, out);
                }
            }
            Pattern::Array { elements, rest } => {
                for e in elements.iter().flatten() {
                    Self::collect_pattern_names(e, out);
                }
                if let Some(r) = rest {
                    Self::collect_pattern_names(r, out);
                }
            }
            Pattern::Assign { target, .. } => Self::collect_pattern_names(target, out),
        }
    }

    fn collect_var_names_stmt(s: &Stmt, out: &mut Vec<String>) {
        match &s.kind {
            StmtKind::VarDecl(d) if d.kind == VarKind::Var => {
                for decl in &d.declarations {
                    Self::collect_pattern_names(&decl.target, out);
                }
            }
            StmtKind::Block(b) => Self::collect_var_names(b, out),
            StmtKind::If { consequent, alternate, .. } => {
                Self::collect_var_names_stmt(consequent, out);
                if let Some(a) = alternate {
                    Self::collect_var_names_stmt(a, out);
                }
            }
            StmtKind::For { init, body, .. } => {
                if let Some(ForInit::VarDecl(d)) = init.as_deref() {
                    if d.kind == VarKind::Var {
                        for decl in &d.declarations {
                            Self::collect_pattern_names(&decl.target, out);
                        }
                    }
                }
                Self::collect_var_names_stmt(body, out);
            }
            StmtKind::ForIn { left, body, .. } | StmtKind::ForOf { left, body, .. } => {
                if let ForHead::VarDecl { kind: VarKind::Var, target } = &**left {
                    Self::collect_pattern_names(target, out);
                }
                Self::collect_var_names_stmt(body, out);
            }
            StmtKind::While { body, .. } | StmtKind::DoWhile { body, .. } | StmtKind::Labeled { body, .. } | StmtKind::With { body, .. } => {
                Self::collect_var_names_stmt(body, out)
            }
            StmtKind::Try { block, handler, finalizer } => {
                Self::collect_var_names(block, out);
                if let Some((_, h)) = handler.as_deref() {
                    Self::collect_var_names(h, out);
                }
                if let Some(f) = finalizer {
                    Self::collect_var_names(f, out);
                }
            }
            StmtKind::Switch { cases, .. } => {
                for c in cases {
                    Self::collect_var_names(&c.body, out);
                }
            }
            StmtKind::Export(ExportDecl::Decl(d)) => Self::collect_var_names_stmt(d, out),
            _ => {}
        }
    }

    /// Annex B.3.3: plain function declarations nested in blocks (of
    /// sloppy code) that also get a `var` binding in the enclosing var
    /// scope — those a `var` of the same name would not make an early
    /// error. `outer` holds the names that already block it (top-level
    /// lexical declarations, parameters).
    fn collect_annexb(stmts: &[Stmt], outer: Vec<String>) -> Vec<(String, usize)> {
        let mut out = Vec::new();
        let mut stack = vec![outer];
        for s in stmts {
            Self::annexb_stmt(s, &mut stack, &mut out);
        }
        out
    }

    fn annexb_block(stmts: &[Stmt], stack: &mut Vec<Vec<String>>, out: &mut Vec<(String, usize)>) {
        for s in stmts {
            if let StmtKind::FunctionDecl(f) = &s.kind {
                if f.is_async || f.is_generator {
                    continue;
                }
                if let Some(n) = &f.name {
                    if !stack.iter().any(|set| set.iter().any(|x| x == &**n)) {
                        out.push((n.to_string(), Rc::as_ptr(f) as usize));
                    }
                }
            }
        }
        let lex: Vec<String> = Self::collect_lexical(stmts, true).into_iter().map(|(n, _)| n).collect();
        stack.push(lex);
        for s in stmts {
            Self::annexb_stmt(s, stack, out);
        }
        stack.pop();
    }

    /// A statement in a position where Annex B allows a function
    /// declaration (`if (x) function f() {}`, labels): as if in a block.
    fn annexb_sub(s: &Stmt, stack: &mut Vec<Vec<String>>, out: &mut Vec<(String, usize)>) {
        if matches!(s.kind, StmtKind::FunctionDecl(_)) {
            Self::annexb_block(std::slice::from_ref(s), stack, out);
        } else {
            Self::annexb_stmt(s, stack, out);
        }
    }

    fn annexb_stmt(s: &Stmt, stack: &mut Vec<Vec<String>>, out: &mut Vec<(String, usize)>) {
        let lexical_names = |kind: VarKind, p: &Pattern| -> Vec<String> {
            let mut names = Vec::new();
            if kind != VarKind::Var {
                Self::collect_pattern_names(p, &mut names);
            }
            names
        };
        match &s.kind {
            StmtKind::Block(b) => Self::annexb_block(b, stack, out),
            StmtKind::If { consequent, alternate, .. } => {
                Self::annexb_sub(consequent, stack, out);
                if let Some(a) = alternate {
                    Self::annexb_sub(a, stack, out);
                }
            }
            StmtKind::For { init, body, .. } => {
                let mut names = Vec::new();
                if let Some(ForInit::VarDecl(d)) = init.as_deref() {
                    for decl in &d.declarations {
                        names.extend(lexical_names(d.kind, &decl.target));
                    }
                }
                stack.push(names);
                Self::annexb_stmt(body, stack, out);
                stack.pop();
            }
            StmtKind::ForIn { left, body, .. } | StmtKind::ForOf { left, body, .. } => {
                let names = match &**left {
                    ForHead::VarDecl { kind, target } => lexical_names(*kind, target),
                    _ => Vec::new(),
                };
                stack.push(names);
                Self::annexb_stmt(body, stack, out);
                stack.pop();
            }
            StmtKind::While { body, .. } | StmtKind::DoWhile { body, .. } | StmtKind::With { body, .. } => Self::annexb_stmt(body, stack, out),
            StmtKind::Labeled { body, .. } => Self::annexb_sub(body, stack, out),
            StmtKind::Try { block, handler, finalizer } => {
                Self::annexb_block(block, stack, out);
                if let Some((param, body)) = handler.as_deref() {
                    // B.3.5: a simple catch parameter does not block `var`.
                    let mut names = Vec::new();
                    if let Some(p) = param {
                        if !matches!(p, Pattern::Identifier(_)) {
                            Self::collect_pattern_names(p, &mut names);
                        }
                    }
                    stack.push(names);
                    Self::annexb_block(body, stack, out);
                    stack.pop();
                }
                if let Some(f) = finalizer {
                    Self::annexb_block(f, stack, out);
                }
            }
            StmtKind::Switch { cases, .. } => {
                // The case clauses share one block scope.
                for c in cases {
                    for s in &c.body {
                        if let StmtKind::FunctionDecl(f) = &s.kind {
                            if let (false, false, Some(n)) = (f.is_async, f.is_generator, &f.name) {
                                if !stack.iter().any(|set| set.iter().any(|x| x == &**n)) {
                                    out.push((n.to_string(), Rc::as_ptr(f) as usize));
                                }
                            }
                        }
                    }
                }
                let lex: Vec<String> = cases.iter().flat_map(|c| Self::collect_lexical(&c.body, true)).map(|(n, _)| n).collect();
                stack.push(lex);
                for c in cases {
                    for s in &c.body {
                        Self::annexb_stmt(s, stack, out);
                    }
                }
                stack.pop();
            }
            _ => {}
        }
    }

    /// Lexical declarations directly in a statement list.
    fn collect_lexical(stmts: &[Stmt], block_functions: bool) -> Vec<(String, BindKind)> {
        let mut out = Vec::new();
        for s in stmts {
            let s = match &s.kind {
                StmtKind::Export(ExportDecl::Decl(d)) => d,
                StmtKind::Export(ExportDecl::DefaultDecl(d)) => d,
                _ => s,
            };
            match &s.kind {
                StmtKind::VarDecl(d) if d.kind != VarKind::Var => {
                    let kind = if d.kind == VarKind::Const { BindKind::Const } else { BindKind::Let };
                    for decl in &d.declarations {
                        let mut names = Vec::new();
                        Self::collect_pattern_names(&decl.target, &mut names);
                        for n in names {
                            out.push((n, kind));
                        }
                    }
                }
                StmtKind::ClassDecl(c) => match &c.name {
                    Some(n) => out.push((n.to_string(), BindKind::Class)),
                    None => out.push((DEFAULT_EXPORT_BINDING.to_string(), BindKind::Class)),
                },
                StmtKind::FunctionDecl(f) if block_functions => match &f.name {
                    Some(n) => out.push((n.to_string(), BindKind::Function)),
                    None => out.push((DEFAULT_EXPORT_BINDING.to_string(), BindKind::Function)),
                },
                _ => {}
            }
        }
        out
    }

    /// SyntaxError for duplicate lexical declarations (and let/var clashes).
    fn check_duplicates(&self, lex: &[(String, BindKind)], vars: &[String]) -> CResult<()> {
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for (n, k) in lex {
            if !seen.insert(n.as_str()) && *k != BindKind::Function {
                return self.err(&format!("Identifier '{}' has already been declared", n));
            }
            if *k != BindKind::Function && vars.iter().any(|v| v == n) {
                return self.err(&format!("Identifier '{}' has already been declared", n));
            }
        }
        Ok(())
    }

    fn hoist_functions(&mut self, stmts: &[Stmt]) -> CResult<()> {
        for s in stmts {
            let s = match &s.kind {
                StmtKind::Export(ExportDecl::Decl(d)) => d,
                StmtKind::Export(ExportDecl::DefaultDecl(d)) => d,
                _ => s,
            };
            if let StmtKind::FunctionDecl(f) = &s.kind {
                // Anonymous `export default function () {}` is named
                // "default" but bound to the hidden `*default*` slot.
                let name = f.name.as_deref().unwrap_or(DEFAULT_EXPORT_BINDING).to_string();
                let idx = self.compile_function_rc(f, Some(if f.name.is_some() { &name } else { "default" }))?;
                self.emit(Op::MakeClosure(idx));
                match self.resolve(&name) {
                    Resolved::Local { depth, slot, .. } => {
                        self.emit(Op::InitLocal(depth, slot));
                    }
                    Resolved::Reg { reg, .. } => {
                        self.emit(Op::SetReg(reg));
                    }
                    Resolved::Global => {
                        let i = self.const_str(&name);
                        self.emit(Op::DeclareGlobalFunction(i));
                    }
                }
            }
        }
        Ok(())
    }

    /// Captured-name set for a function node: from the program-wide
    /// analysis, or computed now for synthesized / cloned nodes (whose
    /// nested functions are then registered until `release` is called).
    fn captured_for(&mut self, f: &Function) -> (Rc<crate::captures::NameSet>, Vec<*const Function>) {
        let key = f as *const Function;
        if let Some(set) = self.captures.get(&key) {
            return (set.clone(), Vec::new());
        }
        let mut local = crate::captures::analyze_function(f);
        let set = local.remove(&key).unwrap_or_default();
        let mut added = Vec::new();
        for (k, v) in local {
            if !k.is_null() && !self.captures.contains_key(&k) {
                self.captures.insert(k, v);
                added.push(k);
            }
        }
        (set, added)
    }

    // --------------------------------------------------------------- script

    fn compile_script(&mut self, program: &Program) -> CResult<Rc<FunctionCode>> {
        let mut flags = CodeFlags::default();
        flags.is_script = !program.is_module;
        flags.is_strict = program.strict || program.is_module;
        let filename = self.filename.clone();
        let mut fs = FnState::new("", flags, !program.is_module, filename);
        fs.source = Some(self.source.clone());
        fs.captured = self.captures.get(&std::ptr::null()).cloned().unwrap_or_default();
        fs.force_heap = program.is_module || fs.captured.contains(crate::captures::EVAL_ALL);
        self.fns.push(fs);

        if program.is_module {
            // Modules behave like a function scope.
            let mut vars = Vec::new();
            Self::collect_var_names(&program.body, &mut vars);
            for v in vars {
                self.declare(&v, BindKind::Var);
            }
            let lex = Self::collect_lexical(&program.body, true);
            for (n, k) in lex {
                self.declare(&n, k);
            }
            let tdz = self.fs_ref().scopes[0].tdz.clone();
            for loc in tdz {
                self.emit(Op::Empty);
                self.emit_init_loc(loc);
            }
        } else {
            let mut vars = Vec::new();
            Self::collect_var_names(&program.body, &mut vars);
            for v in &vars {
                let i = self.const_str(v);
                self.emit(Op::DeclareGlobalVar(i));
            }
            let lex = Self::collect_lexical(&program.body, false);
            self.check_duplicates(&lex, &vars)?;
            if !self.fs_ref().flags.is_strict {
                // Annex B.3.3.2: block functions also bind a global var.
                let outer: Vec<String> = lex.iter().map(|(n, _)| n.clone()).collect();
                for (n, ptr) in Self::collect_annexb(&program.body, outer) {
                    if !vars.contains(&n) {
                        let i = self.const_str(&n);
                        self.emit(Op::DeclareGlobalVar(i));
                    }
                    self.fs().annexb.insert(ptr);
                }
            }
            for (n, k) in lex {
                // Track as script-level lexical for emit_init_var.
                self.fs().scopes[0].names.insert(n.clone(), Binding { loc: SCRIPT_LEX, kind: k, initialized: false });
                let i = self.const_str(&n);
                self.emit(Op::DeclareGlobalLex(i, k == BindKind::Const));
            }
            // The script scope itself is not a runtime scope: resolution
            // falls through to globals because names map to a slot of a
            // non-materialized scope.
            self.fs().scopes[0].materialized = false;
        }
        self.hoist_functions(&program.body)?;
        for s in program.body.iter() {
            self.compile_stmt(s)?;
        }
        if program.is_module {
            self.emit(Op::Undefined);
        } else {
            self.emit(Op::LoadCompletion);
        }
        self.emit(Op::Return);
        let fs = self.fns.pop().unwrap();
        Ok(Rc::new(finish(fs, self.module)))
    }

    // ----------------------------------------------------------------- eval

    /// The body of direct eval code (see [`compile_eval`]).
    fn compile_eval_body(&mut self, program: &Program, strict: bool, site_in_params: bool) -> CResult<()> {
        let body = &program.body;
        let mut vars = Vec::new();
        Self::collect_var_names(body, &mut vars);
        let lex = Self::collect_lexical(body, false);
        let funcs: Vec<Rc<Function>> = body
            .iter()
            .filter_map(|s| match &s.kind {
                StmtKind::FunctionDecl(f) => Some(f.clone()),
                _ => None,
            })
            .collect();
        let mut var_like = vars.clone();
        for f in &funcs {
            if let Some(n) = &f.name {
                var_like.push(n.to_string());
            }
        }
        self.check_duplicates(&lex, &var_like)?;
        if strict {
            for v in &vars {
                self.declare(v, BindKind::Var);
            }
            for (n, k) in Self::collect_lexical(body, true) {
                self.declare(&n, k);
            }
            self.eval_scope_entry();
            self.hoist_functions(body)?;
        } else {
            for (n, k) in &lex {
                self.declare(n, *k);
            }
            if site_in_params && var_like.iter().any(|n| n == "arguments") {
                return self.err("'arguments' cannot be declared by eval code in parameters");
            }
            // Var names the caller's lexical bindings would clash with.
            for n in &var_like {
                if self.eval_var_conflict(n) {
                    return self.err(&format!("Identifier '{}' has already been declared", n));
                }
            }
            let lex_names: Vec<String> = lex.iter().map(|(n, _)| n.clone()).collect();
            let mut annexb_names = Vec::new();
            for (n, ptr) in Self::collect_annexb(body, lex_names) {
                if self.eval_var_conflict(&n) {
                    continue;
                }
                self.fs().annexb.insert(ptr);
                annexb_names.push(n);
            }
            self.eval_scope_entry();
            let mut declared: Vec<&String> = Vec::new();
            for n in var_like.iter().chain(annexb_names.iter()) {
                if !declared.contains(&n) {
                    declared.push(n);
                    self.emit_declare_eval_var(n);
                }
            }
            for f in &funcs {
                let name = f.name.as_deref().unwrap_or("").to_string();
                let idx = self.compile_function_rc(f, Some(&name))?;
                self.emit(Op::MakeClosure(idx));
                self.emit_store_eval_var(&name)?;
            }
        }
        for s in body {
            self.compile_stmt(s)?;
        }
        self.emit(Op::LoadCompletion);
        self.emit(Op::Return);
        Ok(())
    }

    /// Materialize the eval function's scope and put its lexical
    /// bindings in their TDZ.
    fn eval_scope_entry(&mut self) {
        let has_heap = self.fs_ref().scopes[0].slot_count > 0;
        self.fs().scopes[0].materialized = has_heap;
        let tdz = self.fs_ref().scopes[0].tdz.clone();
        for loc in tdz {
            self.emit(Op::Empty);
            self.emit_init_loc(loc);
        }
    }

    /// Would `var name` in sloppy eval code clash with a lexical binding
    /// between the eval and its variable environment (EvalDeclarationInstantiation)?
    fn eval_var_conflict(&self, name: &str) -> bool {
        let mut node = self.outer.clone();
        while let Some(n) = node {
            if let Some(&(_, kind)) = n.names.get(name) {
                if matches!(kind, BindKind::Let | BindKind::Const | BindKind::Class) {
                    return true;
                }
            }
            if n.is_function || n.is_script {
                return false;
            }
            node = n.parent.clone();
        }
        false
    }

    /// Instantiate `var name` of sloppy eval code in the caller's variable
    /// environment, unless a binding exists there already.
    fn emit_declare_eval_var(&mut self, name: &str) {
        let mut node = self.outer.clone();
        while let Some(n) = node {
            if n.names.contains_key(name) && !n.is_script && n.materialized {
                return;
            }
            if n.is_function || n.is_script {
                if n.with_name.is_some() {
                    self.emit_get_var(EVAL_VARS);
                    let i = self.const_str(name);
                    self.emit(Op::DeclareEvalVar(i));
                    return;
                }
                if !n.is_script {
                    return;
                }
                break;
            }
            node = n.parent.clone();
        }
        let i = self.const_str(name);
        self.emit(Op::DeclareEvalGlobalVar(i));
    }

    // --------------------------------------------------------------- module

    /// Module compilation: an outer environment function owning the module
    /// scope (all top-level bindings, including imports) and an inner async
    /// arrow holding the body. Splitting them lets the module loader create
    /// the environment (and hoist functions) at link time, before any
    /// module in the graph evaluates — which is what makes cyclic imports
    /// of hoisted functions work.
    fn compile_module_program(&mut self, program: &Program) -> CResult<CompiledModule> {
        let flags = CodeFlags { is_strict: true, ..Default::default() };
        let mut fs = FnState::new("<module>", flags, false, self.filename.clone());
        fs.is_module_env = true;
        fs.force_heap = true;
        fs.source = Some(self.source.clone());
        self.fns.push(fs);

        let mut vars = Vec::new();
        Self::collect_var_names(&program.body, &mut vars);
        let mut lex = Self::collect_lexical(&program.body, true);
        for s in &program.body {
            match &s.kind {
                StmtKind::Import(d) => {
                    let locals = d.default.iter().chain(d.namespace.iter()).chain(d.named.iter().map(|(_, local)| local));
                    lex.extend(locals.map(|l| (l.clone(), BindKind::Import)));
                }
                StmtKind::Export(ExportDecl::DefaultExpr(_)) => lex.push((DEFAULT_EXPORT_BINDING.to_string(), BindKind::Let)),
                _ => {}
            }
        }
        self.check_duplicates(&lex, &vars)?;
        for v in &vars {
            self.declare(v, BindKind::Var);
        }
        for (n, k) in &lex {
            self.declare(n, *k);
        }
        let tdz = self.fs_ref().scopes[0].tdz.clone();
        for loc in tdz {
            self.emit(Op::Empty);
            self.emit_init_loc(loc);
        }
        self.hoist_functions(&program.body)?;

        // The body: an async arrow (so `this` is the environment's
        // undefined `this`, there is no `arguments`, and top-level `await`
        // works). Bodies without `await` complete synchronously.
        let body_flags = CodeFlags { is_arrow: true, is_async: true, is_strict: true, has_simple_params: true, ..Default::default() };
        let mut body = FnState::new("<module>", body_flags, false, self.filename.clone());
        body.source = Some(self.source.clone());
        body.captured = self.captures.get(&std::ptr::null()).cloned().unwrap_or_default();
        body.scopes[0].materialized = false;
        self.fns.push(body);
        for s in &program.body {
            self.compile_stmt(s)?;
        }
        self.emit(Op::Undefined);
        self.emit(Op::Return);
        let body = self.fns.pop().unwrap();
        let body_code = Rc::new(finish(body, self.module));
        let env = self.fs();
        env.functions.push(body_code);
        let idx = (env.functions.len() - 1) as u32;
        self.emit(Op::MakeClosure(idx));
        self.emit(Op::Return);

        let env = self.fns.pop().unwrap();
        let slots = env
            .scopes[0]
            .names
            .iter()
            .filter_map(|(n, b)| match b.loc {
                Loc::Slot(s) => Some((n.clone(), s)),
                Loc::Reg(_) => None,
            })
            .collect();
        Ok(CompiledModule { code: Rc::new(finish(env, self.module)), slots })
    }

    // ------------------------------------------------------------ functions

    fn compile_function(&mut self, f: &Function, name: Option<&str>) -> CResult<u32> {
        if let FunctionBody::Lazy(lb) = &f.body {
            // Skipped by the pre-parser: parse the body now.
            let (stmts, strict) = crate::parser::parse_lazy_body(f, lb).map_err(|e| CompileError { message: e.message, pos: Pos { line: e.line, column: e.column } })?;
            let full = Function { body: FunctionBody::Block(stmts), strict: f.strict || strict, ..f.clone() };
            return self.compile_function(&full, name);
        }
        let name = f.name.as_deref().or(name).unwrap_or("");
        let mut flags = CodeFlags {
            is_arrow: f.is_arrow,
            is_async: f.is_async,
            is_generator: f.is_generator,
            is_strict: f.strict || self.fs_ref().flags.is_strict,
            has_simple_params: f.params.iter().all(|p| matches!(p.pattern, Pattern::Identifier(_)) && !p.rest),
            ..Default::default()
        };
        flags.is_method = false;
        if let Some(t) = self.pending_tweak.take() {
            t(&mut flags);
        }
        let filename = self.fs_ref().filename.clone();
        let mut fs = FnState::new(name, flags, false, filename);
        fs.source = Some(self.source.clone());
        fs.span = (f.span.1 > f.span.0).then_some(f.span);
        fs.class_depth = self.fs_ref().class_depth;
        let (captured, temp_keys) = self.captured_for(f);
        fs.captured = captured;
        if fs.captured.contains(crate::captures::EVAL_ALL) {
            fs.force_heap = true;
        }
        let eval_vars = fs.captured.contains(crate::captures::EVAL_HERE) && !fs.flags.is_strict;
        self.fns.push(fs);
        self.set_pos(f.pos);
        if eval_vars {
            // Sloppy direct eval may declare vars here: they live on an
            // object consulted before enclosing scopes.
            let loc = self.declare(EVAL_VARS, BindKind::Hidden);
            self.emit(Op::NewEvalVars);
            self.emit_init_loc(loc);
            self.fs().scopes[0].with_name = Some(EVAL_VARS.to_string());
        }

        // Parameters.
        let mut param_count = 0u32;
        let mut complex: Vec<(Loc, &Param)> = Vec::new();
        let mut seen_default = false;
        let mut param_slots: Vec<Loc> = Vec::new();
        let mut rest_slot: Option<Loc> = None;
        // Parameter expressions: every parameter is bound in order, in its
        // TDZ until then (`function f(a = b, b) {}` throws).
        let simple_params = f.params.iter().all(|p| matches!(p.pattern, Pattern::Identifier(_)) && !p.rest);
        for p in &f.params {
            if !p.rest && !matches!(p.pattern, Pattern::Assign { .. }) && !seen_default {
                param_count += 1;
            } else {
                seen_default = true;
            }
            match &p.pattern {
                Pattern::Identifier(n) if !p.rest && simple_params => {
                    let slot = self.declare(n, BindKind::Param);
                    param_slots.push(slot);
                }
                _ => {
                    let fs = self.fs();
                    fs.hidden_counter += 1;
                    let hname = format!("%param{}", fs.hidden_counter);
                    let slot = self.declare(&hname, BindKind::Hidden);
                    if p.rest {
                        rest_slot = Some(slot);
                    } else {
                        param_slots.push(slot);
                    }
                    complex.push((slot, p));
                }
            }
        }
        self.fs().param_count = param_count;
        self.fs().param_locs = param_slots;
        self.fs().rest_loc = rest_slot;
        // Vars and lexicals.
        let body_stmts: &[Stmt] = match &f.body {
            FunctionBody::Block(b) => b,
            FunctionBody::Expr(_) | FunctionBody::Lazy(_) => &[],
        };
        let mut vars = Vec::new();
        Self::collect_var_names(body_stmts, &mut vars);
        let lex = Self::collect_lexical(body_stmts, true);
        // With parameter expressions the body's vars live in an
        // environment of their own (closures in defaults do not see them).
        let separate_vars = !f.params.iter().all(|p| matches!(p.pattern, Pattern::Identifier(_)) && !p.rest) && matches!(f.body, FunctionBody::Block(_));
        let mut body_decls: Vec<(String, BindKind)> = Vec::new();
        if separate_vars {
            body_decls.extend(vars.iter().map(|v| (v.clone(), BindKind::Var)));
            body_decls.extend(lex.iter().cloned());
        } else {
            for v in &vars {
                self.declare(v, BindKind::Var);
            }
            for (n, k) in &lex {
                self.declare(n, *k);
            }
        }
        if !self.fs_ref().flags.is_strict {
            // Annex B.3.3.1: block functions also bind a function-level var.
            let mut outer: Vec<String> = lex.iter().filter(|(_, k)| *k != BindKind::Function).map(|(n, _)| n.clone()).collect();
            for p in &f.params {
                Self::collect_pattern_names(&p.pattern, &mut outer);
            }
            for (n, ptr) in Self::collect_annexb(body_stmts, outer) {
                if n != "arguments" {
                    if separate_vars {
                        body_decls.push((n.clone(), BindKind::Var));
                    } else {
                        self.declare(&n, BindKind::Var);
                    }
                }
                self.fs().annexb.insert(ptr);
            }
        }
        // Rest/pattern params and defaults declare their names as vars too.
        for (_, p) in &complex {
            let mut names = Vec::new();
            Self::collect_pattern_names(&p.pattern, &mut names);
            for n in names {
                self.declare(&n, if simple_params { BindKind::Var } else { BindKind::Let });
            }
        }
        // Named function expression self-binding. It lives in an outer
        // environment per spec, so any parameter, `var`, lexical or inner
        // function declaration with the same name shadows it: only bind it
        // when nothing in the body declares the name.
        if let Some(n) = &f.name {
            if f.self_binding && !self.fs_ref().scopes[0].names.contains_key(&**n) && !body_decls.iter().any(|(b, _)| b == &**n) {
                let loc = self.declare(n, BindKind::Hidden);
                self.fs().self_loc = Some(loc);
            }
        }
        // An `arguments` object referenced from nested arrows must live in
        // the heap scope: declare it now, before the scope layout is fixed.
        if !f.is_arrow && crate::captures::is_captured(&self.fs_ref().captured, "arguments") && !self.fs_ref().scopes[0].names.contains_key("arguments") {
            let loc = self.declare("arguments", BindKind::Hidden);
            self.fs().arguments_loc = Some(loc);
            self.fs().flags.uses_arguments = true;
        }
        // The function scope object exists only if some binding is captured.
        let has_heap = self.fs_ref().scopes[0].slot_count > 0;
        self.fs().scopes[0].materialized = has_heap;
        let tdz = self.fs_ref().scopes[0].tdz.clone();
        for loc in tdz {
            self.emit(Op::Empty);
            self.emit_init_loc(loc);
        }
        // Parameter destructuring / defaults.
        self.fs().in_params = true;
        for (loc, p) in complex {
            // Rest arguments arrive as an array in the hidden binding.
            self.emit_load_loc(loc);
            self.compile_pattern_store(&p.pattern, true)?;
        }
        self.fs().in_params = false;
        // Simple params with defaults: `function f(a = 1)`
        // (handled above since `Assign` patterns are complex).
        if separate_vars {
            // Vars named like a parameter (or `arguments`) start with its value.
            let copies: Vec<String> = vars.iter().filter(|v| self.fs_ref().scopes[0].names.contains_key(v.as_str())).cloned().collect();
            for n in &copies {
                self.emit_get_var(n);
            }
            self.push_block_scope_with(&body_decls, false);
            for n in copies.iter().rev() {
                self.emit_init_var(n)?;
            }
            self.fs().var_scope = self.fs_ref().scopes.len() - 1;
        }
        if self.fs_ref().flags.is_class_constructor && !self.fs_ref().flags.is_derived {
            self.emit(Op::InitFields);
        }
        self.hoist_functions(body_stmts)?;
        if f.is_generator {
            // FunctionDeclarationInstantiation (argument binding, defaults,
            // destructuring) runs at the call: the generator object is
            // returned from this initial suspension.
            self.emit(Op::Undefined);
            self.emit(Op::Yield);
            self.emit(Op::Pop);
        }
        match &f.body {
            FunctionBody::Block(b) => {
                for s in b {
                    self.compile_stmt(s)?;
                }
                self.emit(Op::Undefined);
                self.emit(Op::Return);
            }
            FunctionBody::Expr(e) => {
                self.compile_expr(e)?;
                self.emit(Op::Return);
            }
            FunctionBody::Lazy(_) => unreachable!("lazy bodies are parsed first"),
        }
        let fs = self.fns.pop().unwrap();
        for k in temp_keys {
            self.captures.remove(&k);
        }
        let code = Rc::new(finish(fs, self.module));
        let parent = self.fs();
        parent.functions.push(code);
        Ok((parent.functions.len() - 1) as u32)
    }

    /// Compile a function whose `FnState` flags need tweaking (class constructors, methods).
    fn compile_function_with(&mut self, f: &Function, name: Option<&str>, tweak: impl FnOnce(&mut CodeFlags) + 'static) -> CResult<u32> {
        // Compile normally then patch flags of the produced code.
        let idx = self.compile_function_inner(f, name, tweak)?;
        Ok(idx)
    }

    fn compile_function_inner(&mut self, f: &Function, name: Option<&str>, tweak: impl FnOnce(&mut CodeFlags) + 'static) -> CResult<u32> {
        // Temporarily push a marker: we reuse compile_function but need the
        // flags set *before* the body compiles (InitFields emission). So we
        // replicate the prologue through a flag hook.
        self.pending_tweak = Some(Box::new(tweak));
        self.compile_function(f, name)
    }

    // -------------------------------------------------------------- stmts

    fn compile_block(&mut self, stmts: &[Stmt]) -> CResult<()> {
        let decls = Self::collect_lexical(stmts, true);
        self.check_duplicates(&decls, &[])?;
        let mat = self.push_block_scope(&decls);
        self.hoist_functions(stmts)?;
        for s in stmts {
            self.compile_stmt(s)?;
        }
        self.pop_block_scope(mat);
        Ok(())
    }

    /// What a direct `eval` here needs to compile its code later.
    fn eval_site(&mut self) -> EvalSite {
        let env = self.env_snapshot();
        let strict = self.fs_ref().flags.is_strict;
        let class_depth = self.fs_ref().class_depth;
        // The innermost non-arrow function decides super / new.target.
        let mut ctx = (false, false, false, false);
        for f in self.fns.iter().rev() {
            if f.flags.is_arrow || f.is_eval {
                continue;
            }
            if !f.is_script && !f.is_module_env {
                ctx = (true, f.flags.is_method || f.flags.is_class_constructor || f.flags.is_field_init, f.flags.is_class_constructor && f.flags.is_derived, f.flags.is_field_init);
            }
            break;
        }
        let in_params = self.fs_ref().in_params && !self.fs_ref().flags.is_arrow;
        EvalSite { env, strict, class_depth, in_function: ctx.0, in_method: ctx.1, in_derived_ctor: ctx.2, in_field_init: ctx.3, in_params }
    }

    /// The body of `if`/`else`: a function declaration there (Annex B.3.4)
    /// behaves as if wrapped in a block.
    fn compile_sub_stmt(&mut self, s: &Stmt) -> CResult<()> {
        if matches!(s.kind, StmtKind::FunctionDecl(_)) {
            return self.compile_block(std::slice::from_ref(s));
        }
        self.compile_stmt(s)
    }

    /// Store the top of the stack (pops) into `name` of the var scope
    /// (function top scope, script globals or the eval's var scope),
    /// skipping block bindings in between (Annex B.3.3 function copies).
    fn emit_store_var_scope(&mut self, name: &str) -> CResult<()> {
        let fs = self.fs_ref();
        if fs.is_eval {
            return self.emit_store_eval_var(name);
        }
        if fs.is_script {
            let i = self.const_str(name);
            self.emit_ic(|ic| Op::SetGlobal(i, ic));
            return Ok(());
        }
        let vs = fs.var_scope;
        let depth = fs.scopes[vs + 1..].iter().filter(|s| s.materialized).count() as u16;
        let mat0 = fs.scopes[vs].materialized;
        match fs.scopes[vs].names.get(name).map(|b| b.loc) {
            Some(Loc::Reg(r)) => {
                self.emit(Op::SetReg(r));
            }
            Some(Loc::Slot(s)) => {
                if mat0 {
                    self.emit(Op::SetLocal(depth, s));
                } else {
                    self.emit(Op::Pop);
                }
            }
            None => {
                self.emit(Op::Pop);
            }
        }
        Ok(())
    }

    /// Eval code: store into a var of the caller's variable environment
    /// (an existing binding, the caller's eval-var object or a global).
    fn emit_store_eval_var(&mut self, name: &str) -> CResult<()> {
        // Depth of the eval function's own runtime scopes.
        let own: u16 = self.fns.iter().skip(1).flat_map(|f| f.scopes.iter()).filter(|s| s.materialized).count() as u16;
        let mut depth = own;
        let mut node = self.outer.clone();
        while let Some(n) = node {
            if let Some(&(slot, _)) = n.names.get(name) {
                if n.materialized && !n.is_script {
                    self.emit(Op::SetLocal(depth, slot));
                    return Ok(());
                }
            }
            if n.is_function || n.is_script {
                if n.with_name.is_some() {
                    // The caller's eval-var object.
                    self.emit_get_var(EVAL_VARS);
                    self.emit(Op::Swap);
                    let i = self.const_str(name);
                    self.emit_ic(|ic| Op::SetProp(i, ic));
                    self.emit(Op::Pop);
                    return Ok(());
                }
                break;
            }
            if n.materialized {
                depth += 1;
            }
            node = n.parent.clone();
        }
        let i = self.const_str(name);
        self.emit_ic(|ic| Op::SetGlobal(i, ic));
        Ok(())
    }

    fn compile_stmt(&mut self, s: &Stmt) -> CResult<()> {
        self.set_pos(s.pos);
        // Completion values (eval / scripts): these statements complete
        // with undefined unless their body produces a value
        // (UpdateEmpty(result, undefined)).
        if self.completion
            && self.fs_ref().no_completion == 0
            && ((self.fns.len() == 1 && self.fs_ref().is_script) || self.fs_ref().is_eval)
            && matches!(
                s.kind,
                StmtKind::If { .. } | StmtKind::For { .. } | StmtKind::ForIn { .. } | StmtKind::ForOf { .. } | StmtKind::While { .. } | StmtKind::DoWhile { .. } | StmtKind::Switch { .. } | StmtKind::Try { .. } | StmtKind::With { .. }
            )
            // Not the try a `using` declaration is lowered to (its catch
            // binding is a hidden `%` name): the block's value stands.
            && !matches!(&s.kind, StmtKind::Try { handler: Some(h), .. } if matches!(&h.0, Some(Pattern::Identifier(n)) if n.starts_with('%')))
        {
            self.emit(Op::Undefined);
            self.emit(Op::StoreCompletion);
        }
        match &s.kind {
            StmtKind::Empty | StmtKind::Debugger => {}
            StmtKind::Expr(e) => {
                if self.completion && self.fs_ref().no_completion == 0 && ((self.fns.len() == 1 && self.fs_ref().is_script) || self.fs_ref().is_eval) {
                    self.compile_expr(e)?;
                    self.emit(Op::StoreCompletion);
                } else {
                    self.compile_expr_discard(e)?;
                }
            }
            StmtKind::VarDecl(d) => self.compile_var_decl(d)?,
            StmtKind::FunctionDecl(f) => {
                // Hoisted; an Annex B block function also copies its value
                // to the var scope's binding when its declaration is reached.
                if self.fs_ref().annexb.contains(&(Rc::as_ptr(f) as usize)) {
                    let name = f.name.as_deref().unwrap_or("").to_string();
                    self.emit_get_var_static(&name);
                    self.emit_store_var_scope(&name)?;
                }
            }
            StmtKind::ClassDecl(c) => {
                self.compile_class(c)?;
                let name = c.name.clone().unwrap_or_default();
                self.emit_init_var(&name)?;
            }
            StmtKind::Block(b) => self.compile_block(b)?,
            StmtKind::If { test, consequent, alternate } => {
                self.compile_expr(test)?;
                let j = self.emit(Op::JumpIfFalse(0));
                self.compile_sub_stmt(consequent)?;
                if let Some(alt) = alternate {
                    let j2 = self.emit(Op::Jump(0));
                    self.patch_jump(j);
                    self.compile_sub_stmt(alt)?;
                    self.patch_jump(j2);
                } else {
                    self.patch_jump(j);
                }
            }
            StmtKind::While { test, body } => self.compile_while(test, body, None)?,
            StmtKind::DoWhile { body, test } => {
                let start = self.here();
                self.push_loop(None, true);
                self.compile_stmt(body)?;
                let lp = self.fs().loops.pop().unwrap();
                let cont = self.here();
                self.patch_continues(&lp, cont as u32);
                self.compile_expr(test)?;
                self.emit(Op::JumpIfTrue(start as u32));
                self.patch_breaks(&lp);
            }
            StmtKind::For { init, test, update, body } => self.compile_for(init.as_deref(), test.as_deref(), update.as_deref(), body, None)?,
            StmtKind::ForIn { left, right, body } => self.compile_for_in(left, right, body, None)?,
            StmtKind::ForOf { left, right, body, is_await } => self.compile_for_of(left, right, body, *is_await, None)?,
            StmtKind::Return(arg) => {
                match arg {
                    Some(e) => self.compile_expr(e)?,
                    None => {
                        self.emit(Op::Undefined);
                    }
                }
                if self.fs_ref().flags.is_async && self.fs_ref().flags.is_generator {
                    // return in async generator awaits the value
                    self.emit(Op::Await);
                }
                self.emit_return_exits()?;
                self.emit(Op::Return);
            }
            StmtKind::Break(label) => {
                let (idx, depth) = self.find_loop(label.as_deref(), false)?;
                self.emit_exits(depth)?;
                let j = self.emit(Op::Jump(0));
                self.fs().loops[idx].break_jumps.push(j);
            }
            StmtKind::Continue(label) => {
                let (idx, depth) = self.find_loop(label.as_deref(), true)?;
                self.emit_exits(depth)?;
                let j = self.emit(Op::Jump(0));
                self.fs().loops[idx].continue_jumps.push(j);
            }
            StmtKind::Throw(e) => {
                self.compile_expr(e)?;
                self.emit(Op::Throw);
            }
            StmtKind::Try { block, handler, finalizer } => self.compile_try(block, handler.as_deref(), finalizer.as_ref())?,
            StmtKind::Switch { discriminant, cases } => self.compile_switch(discriminant, cases)?,
            StmtKind::Labeled { label, body } => {
                match &body.kind {
                    StmtKind::For { init, test, update, body: b } => self.compile_for(init.as_deref(), test.as_deref(), update.as_deref(), b, Some(label.to_string()))?,
                    StmtKind::ForIn { left, right, body: b } => self.compile_for_in(left, right, b, Some(label.to_string()))?,
                    StmtKind::ForOf { left, right, body: b, is_await } => self.compile_for_of(left, right, b, *is_await, Some(label.to_string()))?,
                    StmtKind::While { test, body: b } => self.compile_while(test, b, Some(label.to_string()))?,
                    StmtKind::DoWhile { body: b, test } => {
                        let start = self.here();
                        self.push_loop(Some(label.to_string()), true);
                        self.compile_stmt(b)?;
                        let lp = self.fs().loops.pop().unwrap();
                        let cont = self.here();
                        self.patch_continues(&lp, cont as u32);
                        self.compile_expr(test)?;
                        self.emit(Op::JumpIfTrue(start as u32));
                        self.patch_breaks(&lp);
                    }
                    _ => {
                        self.push_loop(Some(label.to_string()), false);
                        self.compile_stmt(body)?;
                        let lp = self.fs().loops.pop().unwrap();
                        self.patch_breaks(&lp);
                    }
                }
            }
            StmtKind::With { object, body } => {
                if self.fs_ref().flags.is_strict {
                    return self.err("Strict mode code may not include a with statement");
                }
                self.compile_expr(object)?;
                self.emit(Op::ToObject);
                // The object lives in a hidden heap binding of the body's
                // scope, so closures created inside the body see it too.
                let fs = self.fs();
                fs.hidden_counter += 1;
                let hidden = format!("%with{}", fs.hidden_counter);
                let mat = self.push_block_scope_with(&[(hidden.clone(), BindKind::Var)], true);
                self.emit_init_var(&hidden)?;
                self.fs().scopes.last_mut().unwrap().with_name = Some(hidden);
                let r = self.compile_stmt(body);
                self.pop_block_scope(mat);
                r?;
            }
            // Import bindings are wired by the module linker.
            StmtKind::Import(_) => {}
            StmtKind::Export(e) => match e {
                ExportDecl::DefaultExpr(ex) => {
                    self.compile_expr_named(ex, Some("default"))?;
                    if self.module.is_some() {
                        self.emit_init_var(DEFAULT_EXPORT_BINDING)?;
                    } else {
                        self.emit(Op::Pop);
                    }
                }
                ExportDecl::DefaultDecl(d) => match &d.kind {
                    StmtKind::ClassDecl(c) if c.name.is_none() => {
                        let mut named = c.clone();
                        named.name = Some("default".into());
                        self.compile_class(&named)?;
                        self.emit_init_var(DEFAULT_EXPORT_BINDING)?;
                    }
                    _ => self.compile_stmt(d)?,
                },
                ExportDecl::Decl(d) => self.compile_stmt(d)?,
                ExportDecl::Named { .. } | ExportDecl::All { .. } => {}
            },
        }
        Ok(())
    }

    fn compile_var_decl(&mut self, d: &VarDecl) -> CResult<()> {
        for decl in &d.declarations {
            match &decl.init {
                Some(init) => {
                    self.compile_expr_named(init, pattern_name(&decl.target))?;
                    // `var x = e` is an assignment to whatever `x` resolves
                    // to (a `with` object, a var declared by eval...).
                    self.compile_pattern_store(&decl.target, d.kind != VarKind::Var || !self.in_with())?;
                }
                None => {
                    if d.kind != VarKind::Var {
                        self.emit(Op::Undefined);
                        self.compile_pattern_store(&decl.target, true)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn push_loop(&mut self, label: Option<String>, is_loop: bool) {
        let depth = self.fs_ref().exits.len();
        self.fs().loops.push(LoopCtx { label, is_loop, is_switch: false, break_jumps: Vec::new(), continue_jumps: Vec::new(), break_depth: depth, continue_depth: depth });
    }

    fn push_switch(&mut self) {
        let depth = self.fs_ref().exits.len();
        self.fs().loops.push(LoopCtx { label: None, is_loop: false, is_switch: true, break_jumps: Vec::new(), continue_jumps: Vec::new(), break_depth: depth, continue_depth: depth });
    }

    /// Make `continue` in the innermost loop unwind only to the current depth.
    fn set_continue_depth(&mut self) {
        let depth = self.fs_ref().exits.len();
        if let Some(l) = self.fs().loops.last_mut() {
            l.continue_depth = depth;
        }
    }

    fn patch_breaks(&mut self, lp: &LoopCtx) {
        for &j in &lp.break_jumps {
            self.patch_jump(j);
        }
    }

    fn patch_continues(&mut self, lp: &LoopCtx, target: u32) {
        for &j in &lp.continue_jumps {
            self.patch_jump_to(j, target);
        }
    }

    /// Find the loop for break/continue; returns (index, exit depth).
    fn find_loop(&self, label: Option<&str>, is_continue: bool) -> CResult<(usize, usize)> {
        let loops = &self.fs_ref().loops;
        for i in (0..loops.len()).rev() {
            let l = &loops[i];
            let matches = match label {
                Some(lb) => l.label.as_deref() == Some(lb) && (!is_continue || l.is_loop),
                None => l.is_loop || (!is_continue && l.is_switch),
            };
            if matches {
                return Ok((i, if is_continue { l.continue_depth } else { l.break_depth }));
            }
        }
        self.err(if is_continue { "Illegal continue statement" } else { "Illegal break statement" })
    }

    /// The exits of a `return` (value on the stack): finalizers run at
    /// stack depth 0 (a `try` in one assumes so), so the value waits in a
    /// hidden register meanwhile.
    fn emit_return_exits(&mut self) -> CResult<()> {
        if self.fs_ref().exits.iter().any(|e| matches!(e, ExitCtx::Finally { .. })) {
            let ret = self.hidden_slot("ret");
            self.emit(Op::SetReg(ret));
            self.emit_exits(0)?;
            self.emit(Op::GetReg(ret));
            Ok(())
        } else {
            self.emit_exits(0)
        }
    }

    /// Emit the exit actions for contexts above `depth` (innermost first).
    fn emit_exits(&mut self, depth: usize) -> CResult<()> {
        // The code of each exit runs outside the tries nested in it: their
        // handlers must not cover it (an iterator close that throws while
        // `return` leaves a for-of is not caught by a try in its body; a
        // finalizer runs outside its own try), while enclosing tries still
        // apply. Suspended ranges reopen after the last exit.
        let mut left = Vec::new();
        let r = self.emit_exits_inner(depth, &mut left);
        let here = self.here() as u32;
        for t in left {
            self.fs().tries[t].open_start = Some(here);
        }
        r
    }

    /// Close the handler ranges of the open tries with `exit_depth >= min`.
    fn suspend_tries(&mut self, min: usize, left: &mut Vec<usize>) {
        let here = self.here() as u32;
        for (t, tc) in self.fs().tries.iter_mut().enumerate() {
            if tc.exit_depth >= min {
                if let Some(s) = tc.open_start.take() {
                    if here > s {
                        tc.ranges.push((s, here));
                    }
                    left.push(t);
                }
            }
        }
    }

    fn emit_exits_inner(&mut self, depth: usize, left: &mut Vec<usize>) -> CResult<()> {
        let exits: Vec<ExitCtx> = self.fs_ref().exits[depth..].to_vec();
        for (i, ctx) in exits.iter().enumerate().rev() {
            let ai = depth + i;
            match ctx {
                ExitCtx::Scope => {
                    self.suspend_tries(ai + 1, left);
                    self.emit(Op::PopScope);
                }
                ExitCtx::IterClose { iter, next } => {
                    self.suspend_tries(ai + 1, left);
                    self.emit(Op::GetReg(*iter));
                    self.emit(Op::GetReg(*next));
                    self.emit(Op::IteratorClose);
                }
                ExitCtx::DestructClose { iter, next } => {
                    self.suspend_tries(ai + 1, left);
                    self.emit(Op::IteratorCloseIfNotDoneReg(*iter, *next));
                }
                ExitCtx::Finally { body, try_index, scopes } => {
                    // Inline the finalizer with inner contexts removed, its
                    // own try (and those inside it) no longer handling.
                    let here = self.here() as u32;
                    let n_tries = self.fs_ref().tries.len();
                    for t in *try_index..n_tries {
                        let tc = &mut self.fs().tries[t];
                        if let Some(s) = tc.open_start.take() {
                            if here > s {
                                tc.ranges.push((s, here));
                            }
                            left.push(t);
                        }
                    }
                    let saved_exits = self.fs().exits.split_off(ai);
                    let saved_scope_depth = self.fs_ref().scope_depth;
                    // Scopes popped by the exits above are reflected in scope_depth for handler bookkeeping.
                    let popped = exits[i + 1..].iter().filter(|e| matches!(e, ExitCtx::Scope)).count() as u32;
                    self.fs().scope_depth = saved_scope_depth - popped;
                    // Those runtime scopes are gone while the finalizer runs:
                    // hide their compile-time counterparts too, or names in
                    // the finalizer would resolve one scope too deep.
                    // Bindings kept in registers (scopes that were not
                    // materialized) must be hidden as well: a `let` inside
                    // the try block would otherwise shadow the finalizer's
                    // outer name of the same spelling.
                    let keep = (*scopes).min(self.fs_ref().scopes.len());
                    let hidden_scopes = self.fs().scopes.split_off(keep);
                    let body = body.clone();
                    self.fs().no_completion += 1;
                    let r = self.compile_block(&body);
                    self.fs().no_completion -= 1;
                    self.fs().scopes.extend(hidden_scopes);
                    r?;
                    self.fs().scope_depth = saved_scope_depth;
                    self.fs().exits.extend(saved_exits);
                }
            }
        }
        Ok(())
    }

    fn compile_for(&mut self, init: Option<&ForInit>, test: Option<&Expr>, update: Option<&Expr>, body: &Stmt, label: Option<String>) -> CResult<()> {
        let mut decls = Vec::new();
        let mut per_iteration = false;
        if let Some(ForInit::VarDecl(d)) = init {
            if d.kind != VarKind::Var {
                let kind = if d.kind == VarKind::Const { BindKind::Const } else { BindKind::Let };
                for decl in &d.declarations {
                    let mut names = Vec::new();
                    Self::collect_pattern_names(&decl.target, &mut names);
                    for n in names {
                        decls.push((n, kind));
                    }
                }
                per_iteration = d.kind == VarKind::Let;
            }
        }
        let mat = self.push_block_scope(&decls);
        match init {
            Some(ForInit::VarDecl(d)) => self.compile_var_decl(d)?,
            Some(ForInit::Expr(e)) => {
                self.compile_expr(e)?;
                self.emit(Op::Pop);
            }
            None => {}
        }
        // Rotated loop: the test sits at the bottom, so an iteration runs
        // one conditional jump instead of a test, a branch and a jump.
        let to_test = test.as_ref().map(|_| self.emit(Op::Jump(0)));
        let start = self.here();
        self.push_loop(label, true);
        self.compile_stmt(body)?;
        let lp = self.fs().loops.pop().unwrap();
        let cont = self.here();
        self.patch_continues(&lp, cont as u32);
        if per_iteration && mat {
            self.emit(Op::CopyScope);
        }
        if let Some(u) = update {
            self.compile_expr_discard(u)?;
        }
        match test {
            Some(t) => {
                self.patch_jump(to_test.unwrap());
                self.compile_expr(t)?;
                self.emit(Op::JumpIfTrue(start as u32));
            }
            None => {
                self.emit(Op::Jump(start as u32));
            }
        }
        self.patch_breaks(&lp);
        self.pop_block_scope(mat);
        Ok(())
    }

    /// `while (test) body`, rotated like `for` loops.
    fn compile_while(&mut self, test: &Expr, body: &Stmt, label: Option<String>) -> CResult<()> {
        let to_test = self.emit(Op::Jump(0));
        let start = self.here();
        self.push_loop(label, true);
        self.compile_stmt(body)?;
        let lp = self.fs().loops.pop().unwrap();
        let cont = self.here();
        self.patch_continues(&lp, cont as u32);
        self.patch_jump(to_test);
        self.compile_expr(test)?;
        self.emit(Op::JumpIfTrue(start as u32));
        self.patch_breaks(&lp);
        Ok(())
    }

    fn for_head_decls(left: &ForHead) -> Vec<(String, BindKind)> {
        match left {
            ForHead::VarDecl { kind, target } if *kind != VarKind::Var => {
                let k = if *kind == VarKind::Const { BindKind::Const } else { BindKind::Let };
                let mut names = Vec::new();
                Self::collect_pattern_names(target, &mut names);
                names.into_iter().map(|n| (n, k)).collect()
            }
            _ => Vec::new(),
        }
    }

    /// `{ [k]: a, b, ...rest } = obj`: computed keys are only known at run
    /// time, so the excluded-key list is collected in a hidden array while
    /// the properties are read (ES2023 §8.6.3 RestBindingInitialization).
    /// Stack on entry: [obj]; consumed on exit.
    fn compile_object_pattern_dynamic_rest(&mut self, properties: &[ObjectPatternProp], rest: &Pattern, is_decl: bool) -> CResult<()> {
        let keys = self.hidden_slot("restkeys");
        self.emit(Op::NewArray(0));
        self.emit(Op::SetReg(keys));
        for p in properties {
            self.emit(Op::Dup); // [obj, obj]
            match &p.key {
                PropertyKey::Identifier(n) | PropertyKey::String(n) => {
                    let i = self.const_str(n);
                    self.emit(Op::GetReg(keys));
                    self.emit(Op::Const(i));
                    self.emit(Op::ArrayPush);
                    self.emit(Op::Pop);
                    self.emit_ic(|ic| Op::GetProp(i, ic));
                }
                PropertyKey::Number(n) => {
                    let i = self.const_str(&crate::value::number_to_string(*n));
                    self.emit(Op::GetReg(keys));
                    self.emit(Op::Const(i));
                    self.emit(Op::ArrayPush);
                    self.emit(Op::Pop);
                    self.emit_ic(|ic| Op::GetProp(i, ic));
                }
                PropertyKey::Computed(e) => {
                    self.compile_expr(e)?;
                    self.emit(Op::ToPropertyKey); // [obj, obj, key]
                    self.emit(Op::Dup); // [obj, obj, key, key]
                    self.emit(Op::GetReg(keys)); // [.., key, key, arr]
                    self.emit(Op::Swap); // [.., key, arr, key]
                    self.emit(Op::ArrayPush); // [.., key, arr]
                    self.emit(Op::Pop); // [obj, obj, key]
                    self.emit(Op::GetElem);
                }
                PropertyKey::Private(_) => return self.err("Unexpected private name"),
            }
            self.compile_pattern_store(&p.value, is_decl)?;
        }
        self.emit(Op::NewObject); // [obj, rest]
        self.emit(Op::Swap); // [rest, obj]
        self.emit(Op::GetReg(keys));
        self.emit(Op::CopyDataPropsExcluding); // [rest]
        self.compile_pattern_store(rest, is_decl)
    }

    fn compile_for_head_store(&mut self, left: &ForHead) -> CResult<()> {
        match left {
            ForHead::VarDecl { target, .. } => self.compile_pattern_store(target, true),
            ForHead::Pattern(p) => self.compile_pattern_store(p, false),
        }
    }

    fn compile_for_in(&mut self, left: &ForHead, right: &Expr, body: &Stmt, label: Option<String>) -> CResult<()> {
        let it = self.hidden_slot("forin");
        self.compile_expr(right)?;
        self.emit(Op::ForInStart);
        self.emit(Op::SetReg(it));
        let start = self.here();
        self.emit(Op::GetReg(it));
        self.emit(Op::ForInNext);
        let exit = self.emit(Op::JumpIfTrue(0));
        self.push_loop(label, true);
        let decls = Self::for_head_decls(left);
        let mat = self.push_block_scope(&decls);
        self.set_continue_depth();
        self.compile_for_head_store(left)?;
        self.compile_stmt(body)?;
        let lp = self.fs().loops.pop().unwrap();
        let cont = self.here();
        self.patch_continues(&lp, cont as u32);
        self.pop_block_scope(mat);
        self.emit(Op::Jump(start as u32));
        self.patch_jump(exit);
        self.emit(Op::Pop); // leftover key (undefined) when done
        let after = self.emit(Op::Jump(0));
        // breaks land here: their exits already popped the iteration scope
        for &j in &lp.break_jumps {
            self.patch_jump(j);
        }
        self.patch_jump(after);
        Ok(())
    }

    fn compile_for_of(&mut self, left: &ForHead, right: &Expr, body: &Stmt, is_await: bool, label: Option<String>) -> CResult<()> {
        let iter = self.hidden_slot("iter");
        let next = self.hidden_slot("next");
        self.compile_expr(right)?;
        self.emit(if is_await { Op::GetAsyncIterator } else { Op::GetIterator });
        self.emit(Op::SetReg(next));
        self.emit(Op::SetReg(iter));
        let start = self.here();
        self.emit(Op::GetReg(iter));
        self.emit(Op::GetReg(next));
        if is_await {
            self.emit(Op::IteratorNextResult);
            self.emit(Op::Await);
            // result -> value done
            self.emit(Op::Dup);
            let d = self.const_str("done");
            self.emit_ic(|ic| Op::GetProp(d, ic));
            let exit = self.emit(Op::JumpIfTrue(0));
            let v = self.const_str("value");
            self.emit_ic(|ic| Op::GetProp(v, ic));
            self.compile_for_of_body(left, body, label, start, iter, next, is_await, exit)
        } else {
            self.emit(Op::IteratorNext);
            let exit = self.emit(Op::JumpIfTrue(0));
            self.compile_for_of_body(left, body, label, start, iter, next, is_await, exit)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn compile_for_of_body(&mut self, left: &ForHead, body: &Stmt, label: Option<String>, start: usize, iter: u16, next: u16, is_async: bool, exit: usize) -> CResult<()> {
        // Protect body with iterator close on throw.
        let try_start = self.here() as u32;
        self.push_loop(label, true);
        self.fs().exits.push(ExitCtx::IterClose { iter, next });
        let decls = Self::for_head_decls(left);
        let mat = self.push_block_scope(&decls);
        self.set_continue_depth();
        self.compile_for_head_store(left)?;
        self.compile_stmt(body)?;
        let lp = self.fs().loops.pop().unwrap();
        let cont = self.here();
        self.patch_continues(&lp, cont as u32);
        self.pop_block_scope(mat);
        self.fs().exits.pop();
        let try_end = self.here() as u32;
        self.emit(Op::Jump(start as u32));
        // throw handler: close iterator, rethrow
        let handler_pc = self.here() as u32;
        let scope_depth = self.fs_ref().scope_depth;
        self.emit(Op::GetReg(iter));
        self.emit(Op::GetReg(next));
        self.emit(Op::IteratorClose);
        self.emit(Op::Throw);
        self.fs().handlers.push(Handler { start: try_start, end: try_end, target: handler_pc, stack_depth: 0, scope_depth });
        self.patch_jump(exit);
        self.emit(Op::Pop); // leftover value / result object
        let _ = is_async;
        let after = self.emit(Op::Jump(0));
        for &j in &lp.break_jumps {
            self.patch_jump(j);
        }
        self.patch_jump(after);
        Ok(())
    }

    fn compile_switch(&mut self, discriminant: &Expr, cases: &[SwitchCase]) -> CResult<()> {
        let d = self.hidden_slot("switch");
        self.compile_expr(discriminant)?;
        self.emit(Op::SetReg(d));
        let mut decls = Vec::new();
        for c in cases {
            decls.extend(Self::collect_lexical(&c.body, true));
        }
        let mat = self.push_block_scope(&decls);
        if let Some(sc) = self.fs().scopes.last_mut() {
            sc.is_switch = true;
        }
        for c in cases {
            self.hoist_functions(&c.body)?;
        }
        let mut case_jumps = Vec::new();
        for c in cases {
            if let Some(t) = &c.test {
                self.emit(Op::GetReg(d));
                self.compile_expr(t)?;
                let j = self.emit(Op::JumpIfStrictEq(0));
                case_jumps.push(Some(j));
            } else {
                case_jumps.push(None);
            }
        }
        let default_jump = self.emit(Op::Jump(0));
        self.push_switch();
        let mut default_target = None;
        for (i, c) in cases.iter().enumerate() {
            let here = self.here() as u32;
            match case_jumps[i] {
                Some(j) => self.patch_jump_to(j, here),
                None => default_target = Some(here),
            }
            for s in &c.body {
                self.compile_stmt(s)?;
            }
        }
        let lp = self.fs().loops.pop().unwrap();
        match default_target {
            Some(t) => self.patch_jump_to(default_jump, t),
            None => self.patch_jump(default_jump),
        }
        // `break` is registered inside the case block scope, so its exit did
        // not pop that scope: breaks land on the scope pop like fallthrough.
        let pop_at = self.here() as u32;
        for &j in &lp.break_jumps {
            self.patch_jump_to(j, pop_at);
        }
        self.pop_block_scope(mat);
        Ok(())
    }

    fn compile_try(&mut self, block: &[Stmt], handler: Option<&(Option<Pattern>, Vec<Stmt>)>, finalizer: Option<&Vec<Stmt>>) -> CResult<()> {
        let finalizer_rc = finalizer.map(|f| Rc::new(f.clone()));
        let try_index = self.fs_ref().tries.len();
        let scope_depth = self.fs_ref().scope_depth;
        let scopes = self.fs_ref().scopes.len();
        let start = self.here() as u32;
        let exit_depth = self.fs_ref().exits.len();
        self.fs().tries.push(TryCtx { ranges: Vec::new(), open_start: Some(start), exit_depth });
        if let Some(f) = &finalizer_rc {
            self.fs().exits.push(ExitCtx::Finally { body: f.clone(), try_index, scopes });
        }
        // --- protected block
        self.compile_block(block)?;
        let block_end = self.here() as u32;
        // close the catch-protected range
        let mut catch_ranges = self.take_ranges(try_index, block_end);
        // normal exit: inline finally
        let mut end_jumps = Vec::new();
        if let Some(f) = &finalizer_rc {
            self.fs().exits.pop();
            self.compile_finally_body(f)?;
            self.fs().exits.push(ExitCtx::Finally { body: f.clone(), try_index, scopes });
        }
        end_jumps.push(self.emit(Op::Jump(0)));
        // --- catch handler
        if let Some((param, body)) = handler {
            let catch_pc = self.here() as u32;
            // handlers for the block ranges
            for (s, e) in catch_ranges.drain(..) {
                self.fs().handlers.push(Handler { start: s, end: e, target: catch_pc, stack_depth: 0, scope_depth });
            }
            // the catch body is protected by the finally handler
            self.fs().tries[try_index].open_start = Some(catch_pc);
            let mut decls = Vec::new();
            if let Some(p) = param {
                let mut names = Vec::new();
                Self::collect_pattern_names(p, &mut names);
                for n in names {
                    decls.push((n, BindKind::Let));
                }
            }
            let mat = self.push_block_scope(&decls);
            match param {
                Some(p) => self.compile_pattern_store(p, true)?,
                None => {
                    self.emit(Op::Pop);
                }
            }
            self.compile_block(body)?;
            self.pop_block_scope(mat);
            let catch_end = self.here() as u32;
            let finally_ranges = self.take_ranges(try_index, catch_end);
            if let Some(f) = &finalizer_rc {
                self.fs().exits.pop();
                self.compile_finally_body(f)?;
                end_jumps.push(self.emit(Op::Jump(0)));
                // exception path for finally
                let fin_pc = self.here() as u32;
                for (s, e) in finally_ranges {
                    self.fs().handlers.push(Handler { start: s, end: e, target: fin_pc, stack_depth: 0, scope_depth });
                }
                self.compile_finally_rethrow(f)?;
            }
        } else if let Some(f) = &finalizer_rc {
            self.fs().exits.pop();
            let fin_pc = self.here() as u32;
            for (s, e) in catch_ranges {
                self.fs().handlers.push(Handler { start: s, end: e, target: fin_pc, stack_depth: 0, scope_depth });
            }
            self.compile_finally_rethrow(f)?;
        }
        self.fs().tries.pop();
        for j in end_jumps {
            self.patch_jump(j);
        }
        Ok(())
    }

    /// A finalizer body (its statements do not set the completion value).
    fn compile_finally_body(&mut self, f: &[Stmt]) -> CResult<()> {
        self.fs().no_completion += 1;
        let r = self.compile_block(f);
        self.fs().no_completion -= 1;
        r
    }

    /// The exception path of a finalizer: the exception waits in a
    /// register (the finalizer may `break`/`continue`/`return` past the
    /// rethrow with an empty operand stack), then is rethrown.
    fn compile_finally_rethrow(&mut self, f: &[Stmt]) -> CResult<()> {
        let exc = self.hidden_slot("finexc");
        self.emit(Op::SetReg(exc));
        self.compile_finally_body(f)?;
        self.emit(Op::GetReg(exc));
        self.emit(Op::Throw);
        Ok(())
    }

    fn take_ranges(&mut self, try_index: usize, end: u32) -> Vec<(u32, u32)> {
        let tc = &mut self.fs().tries[try_index];
        if let Some(s) = tc.open_start.take() {
            if end > s {
                tc.ranges.push((s, end));
            }
        }
        std::mem::take(&mut tc.ranges)
    }

    // ------------------------------------------------------------ patterns

    /// Store the value on top of the stack into `pattern` (pops it).
    fn compile_pattern_store(&mut self, pattern: &Pattern, is_decl: bool) -> CResult<()> {
        match pattern {
            Pattern::Identifier(n) => {
                if is_decl {
                    self.emit_init_var(n)
                } else {
                    self.emit_set_var(n)
                }
            }
            Pattern::Expr(e) => match &e.kind {
                ExprKind::Member { object, property, optional: false } => {
                    self.compile_expr(object)?;
                    match property {
                        MemberProp::Identifier(name) => {
                            self.emit(Op::Swap);
                            let i = self.const_str(name);
                            self.emit_ic(|ic| Op::SetProp(i, ic));
                            self.emit(Op::Pop);
                        }
                        MemberProp::Private(name) => {
                            self.emit_private_name(name)?;
                            self.emit(Op::Rot3);
                            self.emit(Op::SetPrivate);
                            self.emit(Op::Pop);
                        }
                        MemberProp::Computed(k) => {
                            self.compile_expr(k)?;
                            self.emit(Op::ToPropertyKey);
                            self.emit(Op::Rot3);
                            self.emit(Op::SetElem);
                            self.emit(Op::Pop);
                        }
                    }
                    Ok(())
                }
                ExprKind::Identifier(n) => self.emit_set_var(n),
                _ => self.err("Invalid assignment target"),
            },
            Pattern::Assign { target, default } => {
                self.emit(Op::Dup);
                let j = self.emit(Op::JumpIfNotUndefined(0));
                self.emit(Op::Pop);
                self.compile_expr_named(default, pattern_name(target))?;
                self.patch_jump(j);
                self.compile_pattern_store(target, is_decl)
            }
            Pattern::Object { properties, rest } => {
                let has_computed = properties.iter().any(|p| matches!(p.key, PropertyKey::Computed(_)));
                if rest.is_some() && has_computed {
                    return self.compile_object_pattern_dynamic_rest(properties, rest.as_deref().unwrap(), is_decl);
                }
                let mut rest_keys: Vec<u32> = Vec::new();
                if properties.is_empty() {
                    // RequireObjectCoercible: `({} = null)` throws.
                    self.emit(Op::Dup);
                    self.emit(Op::ToObject);
                    self.emit(Op::Pop);
                }
                for p in properties {
                    self.emit(Op::Dup);
                    match &p.key {
                        PropertyKey::Identifier(n) | PropertyKey::String(n) => {
                            let i = self.const_str(n);
                            self.emit_ic(|ic| Op::GetProp(i, ic));
                            rest_keys.push(i);
                        }
                        PropertyKey::Number(n) => {
                            let i = self.const_str(&crate::value::number_to_string(*n));
                            self.emit_ic(|ic| Op::GetProp(i, ic));
                            rest_keys.push(i);
                        }
                        PropertyKey::Computed(e) => {
                            self.compile_expr(e)?;
                            self.emit(Op::ToPropertyKey);
                            self.emit(Op::GetElem);
                        }
                        PropertyKey::Private(_) => return self.err("Unexpected private name"),
                    }
                    self.compile_pattern_store(&p.value, is_decl)?;
                }
                if let Some(r) = rest {
                    self.emit(Op::NewObject);
                    self.emit(Op::Swap);
                    for &k in &rest_keys {
                        self.emit(Op::Const(k));
                    }
                    self.emit(Op::NewArray(rest_keys.len() as u32));
                    self.emit(Op::CopyDataPropsExcluding);
                    self.compile_pattern_store(r, is_decl)?;
                } else {
                    self.emit(Op::Pop);
                }
                Ok(())
            }
            Pattern::Array { elements, rest } => {
                // The iterator lives in registers; an abrupt completion
                // while destructuring closes it (unless it is done).
                self.emit(Op::GetIterator);
                let iter = self.hidden_slot("diter");
                let next = self.hidden_slot("dnext");
                self.emit(Op::SetReg(next));
                self.emit(Op::SetReg(iter));
                let try_start = self.here() as u32;
                self.fs().exits.push(ExitCtx::DestructClose { iter, next });
                let r: CResult<()> = (|| {
                    for el in elements {
                        // A property target is evaluated before the step.
                        if let Some(p) = el {
                            if self.emit_member_target_ref(p)? {
                                self.emit(Op::IteratorStepReg(iter, next));
                                self.emit_member_target_store(p)?;
                                continue;
                            }
                        }
                        self.emit(Op::IteratorStepReg(iter, next));
                        match el {
                            Some(p) => self.compile_pattern_store(p, is_decl)?,
                            None => {
                                self.emit(Op::Pop);
                            }
                        }
                    }
                    if let Some(r) = rest {
                        if self.emit_member_target_ref(r)? {
                            self.emit(Op::IteratorRestReg(iter, next));
                            self.emit_member_target_store(r)?;
                        } else {
                            self.emit(Op::IteratorRestReg(iter, next));
                            self.compile_pattern_store(r, is_decl)?;
                        }
                    }
                    Ok(())
                })();
                self.fs().exits.pop();
                r?;
                let try_end = self.here() as u32;
                self.emit(Op::IteratorCloseIfNotDoneReg(iter, next));
                let done = self.emit(Op::Jump(0));
                let handler_pc = self.here() as u32;
                let scope_depth = self.fs_ref().scope_depth;
                self.emit(Op::IteratorCloseOnThrow(iter, next));
                self.emit(Op::Throw);
                self.fs().handlers.push(Handler { start: try_start, end: try_end, target: handler_pc, stack_depth: 0, scope_depth });
                self.patch_jump(done);
                Ok(())
            }
        }
    }

    // --------------------------------------------------------- expressions

    /// Compile an expression, naming anonymous functions/classes after `name`.
    fn compile_expr_named(&mut self, e: &Expr, name: Option<&str>) -> CResult<()> {
        match (&e.kind, name) {
            (ExprKind::Function(f), Some(n)) if f.name.is_none() => {
                self.set_pos(e.pos);
                let idx = self.compile_function_rc(f, Some(n))?;
                self.emit(Op::MakeClosure(idx));
                Ok(())
            }
            (ExprKind::Arrow(f), Some(n)) => {
                self.set_pos(e.pos);
                let idx = self.compile_function_rc(f, Some(n))?;
                self.emit(Op::MakeClosure(idx));
                Ok(())
            }
            (ExprKind::Class(c), Some(n)) if c.name.is_none() => {
                let mut c2 = (**c).clone();
                c2.name = Some(n.into());
                self.compile_class_expr(&c2, true)
            }
            _ => self.compile_expr(e),
        }
    }

    /// Compile an expression whose value is not used (leaves nothing).
    fn compile_expr_discard(&mut self, e: &Expr) -> CResult<()> {
        match &e.kind {
            // The value of a postfix update is unused: the prefix form is
            // cheaper (no ToNumeric of the old value, no undo).
            ExprKind::Update { op, target, .. } => {
                self.set_pos(e.pos);
                self.compile_update(*op, true, target)?;
            }
            ExprKind::Sequence(list) => {
                for x in list {
                    self.compile_expr_discard(x)?;
                }
                return Ok(());
            }
            _ => self.compile_expr(e)?,
        }
        self.emit(Op::Pop);
        Ok(())
    }

    fn compile_expr(&mut self, e: &Expr) -> CResult<()> {
        self.set_pos(e.pos);
        match &e.kind {
            ExprKind::Identifier(n) => {
                if n == "undefined" && matches!(self.resolve(n), Resolved::Global) {
                    self.emit(Op::Undefined);
                } else {
                    self.emit_get_var(n);
                }
            }
            ExprKind::Null => {
                self.emit(Op::Null);
            }
            ExprKind::Undefined => {
                self.emit(Op::Undefined);
            }
            ExprKind::This => {
                let derived = self.fs_ref().flags.is_derived;
                self.emit(if derived { Op::ThisChecked } else { Op::This });
            }
            ExprKind::Super => return self.err("'super' keyword unexpected here"),
            ExprKind::Bool(b) => {
                self.emit(if *b { Op::True } else { Op::False });
            }
            ExprKind::Number(n) => self.emit_number(*n),
            ExprKind::BigInt(s) => {
                let fs = self.fs();
                let i = fs.constants.len() as u32;
                let b = crate::value::BigInt::parse(s).unwrap_or_default();
                fs.constants.push(Value::BigInt(Rc::new(b)));
                self.emit(Op::Const(i));
            }
            ExprKind::String(s) => self.emit_string(s),
            ExprKind::Template { quasis, expressions, .. } => {
                let first = quasis[0].clone().unwrap_or_default();
                self.emit_string(&first);
                for (i, ex) in expressions.iter().enumerate() {
                    self.compile_expr(ex)?;
                    self.emit(Op::ToString);
                    self.emit(Op::Add);
                    let q = quasis.get(i + 1).cloned().flatten().unwrap_or_default();
                    if !q.is_empty() {
                        self.emit_string(&q);
                        self.emit(Op::Add);
                    }
                }
            }
            ExprKind::TaggedTemplate { tag, quasis, raws, expressions } => {
                self.compile_callee(tag)?;
                let fs = self.fs();
                let idx = fs.templates.len() as u32;
                fs.templates.push((quasis.iter().map(|q| q.as_ref().map(|s| JsStr::new(s))).collect(), raws.iter().map(|r| JsStr::new(r)).collect()));
                self.emit(Op::TemplateObject(idx));
                for ex in expressions.iter() {
                    self.compile_expr(ex)?;
                }
                self.emit(Op::Call((expressions.len() + 1) as u16, crate::bytecode::NO_DESC));
            }
            ExprKind::Regex { pattern, flags } => {
                let p = self.const_str(pattern);
                let f = self.const_str(flags);
                self.emit(Op::NewRegExp(p, f));
            }
            ExprKind::Array(elements) => {
                let simple = elements.iter().all(|e| matches!(e, ArrayElement::Expr(_)));
                if simple && elements.len() < 1024 {
                    for el in elements {
                        if let ArrayElement::Expr(x) = el {
                            self.compile_expr(x)?;
                        }
                    }
                    self.emit(Op::NewArray(elements.len() as u32));
                } else {
                    self.emit(Op::NewArray(0));
                    for el in elements {
                        match el {
                            ArrayElement::Hole => {
                                self.emit(Op::ArrayHole);
                            }
                            ArrayElement::Expr(x) => {
                                self.compile_expr(x)?;
                                self.emit(Op::ArrayPush);
                            }
                            ArrayElement::Spread(x) => {
                                self.compile_expr(x)?;
                                self.emit(Op::ArraySpread);
                            }
                        }
                    }
                }
            }
            ExprKind::Object(props) => self.compile_object_literal(props)?,
            ExprKind::Function(f) => {
                let idx = self.compile_function_rc(f, None)?;
                self.emit(Op::MakeClosure(idx));
            }
            ExprKind::Arrow(f) => {
                let idx = self.compile_function_rc(f, None)?;
                self.emit(Op::MakeClosure(idx));
            }
            ExprKind::Class(c) => self.compile_class_expr(c, true)?,
            ExprKind::Unary { op, operand } => match op {
                UnaryOp::Typeof => {
                    if let ExprKind::Identifier(n) = &operand.kind {
                        let chain = self.with_chain(n);
                        if !chain.is_empty() {
                            let ends = self.emit_with_dispatch(n, &chain, &mut |c, w, i| {
                                c.emit_get_var(w);
                                c.emit_ic(|ic| Op::GetProp(i, ic));
                                c.emit(Op::Typeof);
                            });
                            if let Resolved::Global = self.resolve(n) {
                                let i = self.const_str(n);
                                self.emit(Op::TypeofGlobal(i));
                            } else {
                                self.emit_get_var_static(n);
                                self.emit(Op::Typeof);
                            }
                            for j in ends {
                                self.patch_jump(j);
                            }
                            return Ok(());
                        }
                        if let Resolved::Global = self.resolve(n) {
                            let i = self.const_str(n);
                            self.emit(Op::TypeofGlobal(i));
                            return Ok(());
                        }
                    }
                    self.compile_expr(operand)?;
                    self.emit(Op::Typeof);
                }
                UnaryOp::Delete => match &operand.kind {
                    ExprKind::Member { object, property, .. } => {
                        self.compile_expr(object)?;
                        match property {
                            MemberProp::Identifier(n) => {
                                let i = self.const_str(n);
                                self.emit(Op::DeleteProp(i));
                            }
                            MemberProp::Computed(k) => {
                                self.compile_expr(k)?;
                                self.emit(Op::DeleteElem);
                            }
                            MemberProp::Private(_) => return self.err("Private fields can not be deleted"),
                        }
                    }
                    ExprKind::Identifier(n) => {
                        let chain = self.with_chain(n);
                        let ends = self.emit_with_dispatch(n, &chain, &mut |c, w, i| {
                            c.emit_get_var(w);
                            c.emit(Op::DeleteProp(i));
                        });
                        if matches!(self.resolve(n), Resolved::Global) {
                            // A global property (true when unresolvable).
                            let i = self.const_str(n);
                            self.emit(Op::DeleteGlobal(i));
                        } else {
                            self.emit(Op::False);
                        }
                        for j in ends {
                            self.patch_jump(j);
                        }
                    }
                    _ => {
                        self.compile_expr(operand)?;
                        self.emit(Op::Pop);
                        self.emit(Op::True);
                    }
                },
                _ => {
                    self.compile_expr(operand)?;
                    self.emit(match op {
                        UnaryOp::Neg => Op::Neg,
                        UnaryOp::Plus => Op::Plus,
                        UnaryOp::Not => Op::Not,
                        UnaryOp::BitNot => Op::BitNot,
                        UnaryOp::Void => Op::Void,
                        _ => unreachable!(),
                    });
                }
            },
            ExprKind::Update { op, prefix, target } => self.compile_update(*op, *prefix, target)?,
            ExprKind::Binary { op, left, right } => {
                if let (BinaryOp::In, ExprKind::PrivateIn { .. }) = (op, &left.kind) {
                    unreachable!()
                }
                self.compile_expr(left)?;
                self.compile_expr(right)?;
                self.emit(match op {
                    BinaryOp::Add => Op::Add,
                    BinaryOp::Sub => Op::Sub,
                    BinaryOp::Mul => Op::Mul,
                    BinaryOp::Div => Op::Div,
                    BinaryOp::Mod => Op::Mod,
                    BinaryOp::Exp => Op::Exp,
                    BinaryOp::Eq => Op::Eq,
                    BinaryOp::NotEq => Op::NotEq,
                    BinaryOp::StrictEq => Op::StrictEq,
                    BinaryOp::StrictNotEq => Op::StrictNotEq,
                    BinaryOp::Lt => Op::Lt,
                    BinaryOp::LtEq => Op::LtEq,
                    BinaryOp::Gt => Op::Gt,
                    BinaryOp::GtEq => Op::GtEq,
                    BinaryOp::Shl => Op::Shl,
                    BinaryOp::Shr => Op::Shr,
                    BinaryOp::UShr => Op::UShr,
                    BinaryOp::BitAnd => Op::BitAnd,
                    BinaryOp::BitOr => Op::BitOr,
                    BinaryOp::BitXor => Op::BitXor,
                    BinaryOp::In => Op::In,
                    BinaryOp::Instanceof => Op::Instanceof,
                });
            }
            ExprKind::PrivateIn { name, object } => {
                self.compile_expr(object)?;
                let (d, s) = self.resolve_private(name)?;
                self.emit(Op::PrivateIn(d, s));
            }
            ExprKind::Logical { op, left, right } => {
                self.compile_expr(left)?;
                let j = match op {
                    LogicalOp::And => self.emit(Op::JumpIfFalseKeep(0)),
                    LogicalOp::Or => self.emit(Op::JumpIfTrueKeep(0)),
                    LogicalOp::Nullish => self.emit(Op::JumpIfNotNullishKeep(0)),
                };
                if *op != LogicalOp::Nullish {
                    self.emit(Op::Pop);
                }
                self.compile_expr(right)?;
                self.patch_jump(j);
            }
            ExprKind::Assign { op, target, value } => self.compile_assign(*op, target, value)?,
            ExprKind::Conditional { test, consequent, alternate } => {
                self.compile_expr(test)?;
                let j = self.emit(Op::JumpIfFalse(0));
                self.compile_expr(consequent)?;
                let j2 = self.emit(Op::Jump(0));
                self.patch_jump(j);
                self.compile_expr(alternate)?;
                self.patch_jump(j2);
            }
            ExprKind::Call { callee, args, optional } => {
                if let ExprKind::Super = callee.kind {
                    return self.compile_super_call(args);
                }
                if let ExprKind::Identifier(n) = &callee.kind {
                    if n == "%copyprivate" {
                        // Synthetic: install the class's private methods.
                        self.emit(Op::This);
                        self.emit_get_var("%privtpl");
                        self.emit(Op::CopyPrivate);
                        return Ok(());
                    }
                    if n == "%definefield" {
                        // Synthetic class field definition: this[key] = value
                        self.emit(Op::This);
                        let mut private = false;
                        if let (Some(ArrayElement::Expr(k)), Some(ArrayElement::Expr(v))) = (args.first(), args.get(1)) {
                            self.compile_expr(k)?;
                            let name = match &k.kind {
                                ExprKind::String(s) => Some(s.as_str()),
                                ExprKind::Identifier(s) if s.starts_with('#') => {
                                    private = true;
                                    Some(&**s)
                                }
                                _ => None,
                            };
                            self.compile_expr_named(v, name)?;
                        }
                        self.emit(if private { Op::DefinePrivate } else { Op::DefineElem });
                        return Ok(());
                    }
                }
                let is_eval = matches!(&callee.kind, ExprKind::Identifier(n) if n == "eval");
                let desc = match callee_description(callee) {
                    Some(d) => self.const_str(&d),
                    None => crate::bytecode::NO_DESC,
                };
                self.compile_callee(callee)?;
                if *optional {
                    // stack: fn this
                    self.emit(Op::Swap);
                    self.emit(Op::Dup);
                    let j = self.emit(Op::JumpIfNullish(0));
                    self.emit(Op::Swap);
                    self.chain_exit(j, 2);
                }
                self.compile_args_and_call(args, is_eval, false, desc)?;
            }
            ExprKind::New { callee, args } => {
                self.compile_expr(callee)?;
                self.compile_args_and_call(args, false, true, crate::bytecode::NO_DESC)?;
            }
            ExprKind::Member { object, property, optional } => {
                if let ExprKind::Super = object.kind {
                    match property {
                        MemberProp::Identifier(n) => {
                            let i = self.const_str(n);
                            self.emit(Op::GetSuperProp(i));
                        }
                        MemberProp::Computed(k) => {
                            self.compile_expr(k)?;
                            self.emit(Op::ToPropertyKey);
                            self.emit(Op::GetSuperElem);
                        }
                        MemberProp::Private(_) => return self.err("Unexpected private field"),
                    }
                    return Ok(());
                }
                if !*optional && matches!(&object.kind, ExprKind::Identifier(n) if n == "arguments") && self.args_direct_ok() {
                    match property {
                        MemberProp::Identifier(p) if p == "length" => {
                            self.fs().flags.args_direct = true;
                            self.emit(Op::ArgsLength);
                            return Ok(());
                        }
                        MemberProp::Computed(k) => {
                            self.fs().flags.args_direct = true;
                            self.compile_expr(k)?;
                            self.set_pos(e.pos);
                            self.emit(Op::ArgsGet);
                            return Ok(());
                        }
                        _ => {}
                    }
                }
                self.compile_expr(object)?;
                if *optional {
                    self.emit(Op::Dup);
                    let j = self.emit(Op::JumpIfNullish(0));
                    self.chain_exit(j, 1);
                }
                match property {
                    MemberProp::Identifier(n) => {
                        let i = self.const_str(n);
                        self.emit_ic(|ic| Op::GetProp(i, ic));
                    }
                    MemberProp::Private(n) => {
                        self.emit_private_name(n)?;
                        self.emit(Op::GetPrivate);
                    }
                    MemberProp::Computed(k) => {
                        self.compile_expr(k)?;
                        self.emit(Op::GetElem);
                    }
                }
            }
            ExprKind::OptionalChain(inner) => {
                self.chain_stack.push(Vec::new());
                self.compile_expr(inner)?;
                let exits = self.chain_stack.pop().unwrap();
                if !exits.is_empty() {
                    let done = self.emit(Op::Jump(0));
                    let mut to_end = Vec::new();
                    for (j, pops) in exits {
                        self.patch_jump(j);
                        for _ in 0..pops {
                            self.emit(Op::Pop);
                        }
                        self.emit(Op::Undefined);
                        to_end.push(self.emit(Op::Jump(0)));
                    }
                    self.patch_jump(done);
                    for j in to_end {
                        self.patch_jump(j);
                    }
                }
            }
            ExprKind::Sequence(list) => {
                for (i, x) in list.iter().enumerate() {
                    self.compile_expr(x)?;
                    if i + 1 < list.len() {
                        self.emit(Op::Pop);
                    }
                }
            }
            ExprKind::Spread(_) => return self.err("Unexpected spread"),
            ExprKind::Yield { argument, delegate } => {
                match argument {
                    Some(a) => self.compile_expr(a)?,
                    None => {
                        self.emit(Op::Undefined);
                    }
                }
                if *delegate {
                    let is_async = self.fs_ref().flags.is_async;
                    self.emit(if is_async { Op::GetAsyncIterator } else { Op::GetIterator });
                    // [iter next received mode]: first a `next(undefined)`.
                    self.emit(Op::Undefined);
                    self.emit(Op::Undefined);
                    let start = self.here();
                    self.emit(Op::YieldStarStep(is_async));
                    // [iter next out]
                    let mut to_return = Vec::new();
                    if is_async {
                        self.emit(Op::YStarIs(3));
                        let l1 = self.emit(Op::JumpIfFalse(0));
                        self.emit(Op::Await);
                        self.emit(Op::YieldStarCheck);
                        self.patch_jump(l1);
                        self.emit(Op::YStarIs(4));
                        let l2 = self.emit(Op::JumpIfFalse(0));
                        self.emit(Op::Await);
                        to_return.push(self.emit(Op::Jump(0)));
                        self.patch_jump(l2);
                    }
                    self.emit(Op::YStarIs(1));
                    let done = self.emit(Op::JumpIfTrue(0));
                    self.emit(Op::YStarIs(2));
                    to_return.push(self.emit(Op::JumpIfTrue(0)));
                    self.emit(if is_async { Op::AsyncYield } else { Op::YieldRaw });
                    self.emit(Op::ResumeMode);
                    self.emit(Op::Jump(start as u32));
                    // Return completion from the delegate (or `return()`
                    // without an inner `return`): finally blocks run.
                    for j in to_return {
                        self.patch_jump(j);
                    }
                    self.emit_return_exits()?;
                    self.emit(Op::Return);
                    self.patch_jump(done);
                    // [iter next value] -> value
                    self.emit(Op::Rot3);
                    self.emit(Op::Pop);
                    self.emit(Op::Swap);
                    self.emit(Op::Pop);
                } else {
                    let is_async = self.fs_ref().flags.is_async;
                    if is_async {
                        // Yield in an async generator awaits its operand first.
                        self.emit(Op::Await);
                    }
                    self.emit(if is_async { Op::AsyncYield } else { Op::Yield });
                    // `generator.return(v)` resumes here with a return
                    // completion: run the finally blocks / iterator closes
                    // a `return v` at this point would.
                    self.emit(Op::ResumeIsReturn);
                    let j = self.emit(Op::JumpIfFalse(0));
                    if is_async {
                        self.emit(Op::Await);
                    }
                    self.emit_return_exits()?;
                    self.emit(Op::Return);
                    self.patch_jump(j);
                }
            }
            ExprKind::Await(a) => {
                self.compile_expr(a)?;
                self.emit(Op::Await);
            }
            ExprKind::NewTarget => {
                self.emit(Op::NewTarget);
            }
            ExprKind::ImportMeta => {
                self.emit(Op::ImportMeta);
            }
            ExprKind::DynamicImport(a) => {
                self.compile_expr(a)?;
                self.emit(Op::DynamicImport);
            }
        }
        Ok(())
    }

    fn chain_exit(&mut self, jump: usize, pops: u32) {
        if let Some(list) = self.chain_stack.last_mut() {
            list.push((jump, pops));
        } else {
            // Not inside an OptionalChain wrapper (parser always wraps); be safe.
            let here = self.here();
            let _ = here;
            self.loose_chain_exits.push((jump, pops));
        }
    }

    /// Push `fn this` for a call.
    fn compile_callee(&mut self, callee: &Expr) -> CResult<()> {
        match &callee.kind {
            ExprKind::Member { object, property, optional } => {
                if let ExprKind::Super = object.kind {
                    // super.method(...) : this = current this
                    match property {
                        MemberProp::Identifier(n) => {
                            let i = self.const_str(n);
                            self.emit(Op::GetSuperProp(i));
                        }
                        MemberProp::Computed(k) => {
                            self.compile_expr(k)?;
                            self.emit(Op::ToPropertyKey);
                            self.emit(Op::GetSuperElem);
                        }
                        MemberProp::Private(_) => return self.err("Unexpected private field"),
                    }
                    let derived = self.fs_ref().flags.is_derived;
                    self.emit(if derived { Op::ThisChecked } else { Op::This });
                    return Ok(());
                }
                self.set_pos(callee.pos);
                self.compile_expr(object)?;
                if *optional {
                    self.emit(Op::Dup);
                    let j = self.emit(Op::JumpIfNullish(0));
                    self.chain_exit(j, 1);
                }
                self.emit(Op::Dup);
                match property {
                    MemberProp::Identifier(n) => {
                        let i = self.const_str(n);
                        self.emit_ic(|ic| Op::GetProp(i, ic));
                    }
                    MemberProp::Private(n) => {
                        self.emit_private_name(n)?;
                        self.emit(Op::GetPrivate);
                    }
                    MemberProp::Computed(k) => {
                        self.compile_expr(k)?;
                        self.emit(Op::GetElem);
                    }
                }
                self.emit(Op::Swap);
            }
            ExprKind::OptionalChain(inner) if matches!(inner.kind, ExprKind::Member { .. }) => {
                // (a?.b)(...) — chain wraps only the callee
                self.chain_stack.push(Vec::new());
                self.compile_callee(inner)?;
                let exits = self.chain_stack.pop().unwrap();
                if !exits.is_empty() {
                    let done = self.emit(Op::Jump(0));
                    let mut to_end = Vec::new();
                    for (j, pops) in exits {
                        self.patch_jump(j);
                        for _ in 0..pops {
                            self.emit(Op::Pop);
                        }
                        self.emit(Op::Undefined);
                        self.emit(Op::Undefined);
                        to_end.push(self.emit(Op::Jump(0)));
                    }
                    self.patch_jump(done);
                    for j in to_end {
                        self.patch_jump(j);
                    }
                }
            }
            ExprKind::Identifier(n) if !self.with_chain(n).is_empty() => {
                // f() inside `with (o)`: o.f() when o has f.
                let chain = self.with_chain(n);
                let ends = self.emit_with_dispatch(n, &chain, &mut |c, w, i| {
                    c.emit_get_var(w);
                    if w == EVAL_VARS {
                        // A var created by direct eval: a plain call.
                        c.emit_ic(|ic| Op::GetProp(i, ic));
                        c.emit(Op::Undefined);
                    } else {
                        c.emit(Op::Dup);
                        c.emit_ic(|ic| Op::GetProp(i, ic));
                        c.emit(Op::Swap);
                    }
                });
                self.emit_get_var_static(n);
                self.emit(Op::Undefined);
                for j in ends {
                    self.patch_jump(j);
                }
            }
            _ => {
                self.compile_expr(callee)?;
                self.emit(Op::Undefined);
            }
        }
        Ok(())
    }

    fn compile_args_and_call(&mut self, args: &[ArrayElement], is_eval: bool, is_new: bool, desc: u32) -> CResult<()> {
        let has_spread = args.iter().any(|a| matches!(a, ArrayElement::Spread(_)));
        if has_spread {
            self.emit(Op::NewArray(0));
            for a in args {
                match a {
                    ArrayElement::Expr(x) => {
                        self.compile_expr(x)?;
                        self.emit(Op::ArrayPush);
                    }
                    ArrayElement::Spread(x) => {
                        self.compile_expr(x)?;
                        self.emit(Op::ArraySpread);
                    }
                    ArrayElement::Hole => {
                        self.emit(Op::ArrayHole);
                    }
                }
            }
            self.emit(if is_new { Op::NewSpread } else { Op::CallSpread });
        } else {
            for a in args {
                if let ArrayElement::Expr(x) = a {
                    self.compile_expr(x)?;
                }
            }
            let n = args.len() as u16;
            if is_eval && !is_new {
                let site = self.eval_site();
                let fs = self.fs();
                fs.eval_sites.push(Rc::new(site));
                let idx = (fs.eval_sites.len() - 1) as u32;
                self.emit(Op::EvalSite(idx));
            }
            self.emit(if is_new {
                Op::New(n)
            } else if is_eval {
                Op::CallEval(n, desc)
            } else {
                Op::Call(n, desc)
            });
        }
        Ok(())
    }

    fn compile_super_call(&mut self, args: &[ArrayElement]) -> CResult<()> {
        let has_spread = args.iter().any(|a| matches!(a, ArrayElement::Spread(_)));
        if has_spread {
            self.emit(Op::NewArray(0));
            for a in args {
                match a {
                    ArrayElement::Expr(x) => {
                        self.compile_expr(x)?;
                        self.emit(Op::ArrayPush);
                    }
                    ArrayElement::Spread(x) => {
                        self.compile_expr(x)?;
                        self.emit(Op::ArraySpread);
                    }
                    ArrayElement::Hole => {
                        self.emit(Op::ArrayHole);
                    }
                }
            }
            self.emit(Op::SuperCallSpread);
        } else {
            for a in args {
                if let ArrayElement::Expr(x) = a {
                    self.compile_expr(x)?;
                }
            }
            self.emit(Op::SuperCall(args.len() as u16));
        }
        Ok(())
    }

    /// The member expression an assignment-pattern element targets
    /// (`[a.b = 1] = …`), if any.
    fn member_target(p: &Pattern) -> Option<(&Expr, &MemberProp, Option<&Expr>)> {
        let (t, default) = match p {
            Pattern::Assign { target, default } => (&**target, Some(&**default)),
            p => (p, None),
        };
        match t {
            Pattern::Expr(e) => match &e.kind {
                ExprKind::Member { object, property, optional: false } if !matches!(object.kind, ExprKind::Super) => Some((object, property, default)),
                _ => None,
            },
            _ => None,
        }
    }

    /// Evaluate a member target's object (and key) ahead of the value
    /// (DestructuringAssignmentEvaluation order). False when `p` is not one.
    fn emit_member_target_ref(&mut self, p: &Pattern) -> CResult<bool> {
        let Some((object, property, _)) = Self::member_target(p) else { return Ok(false) };
        self.compile_expr(object)?;
        match property {
            MemberProp::Identifier(_) => {}
            MemberProp::Computed(k) => {
                self.compile_expr(k)?;
                self.emit(Op::ToPropertyKey);
            }
            MemberProp::Private(n) => self.emit_private_name(n)?,
        }
        Ok(true)
    }

    /// [ref… value] -> []: apply the default, then PutValue.
    fn emit_member_target_store(&mut self, p: &Pattern) -> CResult<()> {
        let Some((_, property, default)) = Self::member_target(p) else { return Ok(()) };
        if let Some(d) = default {
            self.emit(Op::Dup);
            let j = self.emit(Op::JumpIfNotUndefined(0));
            self.emit(Op::Pop);
            self.compile_expr(d)?;
            self.patch_jump(j);
        }
        match property {
            MemberProp::Identifier(name) => {
                let i = self.const_str(name);
                self.emit_ic(|ic| Op::SetProp(i, ic));
            }
            MemberProp::Computed(_) => {
                self.emit(Op::SetElem);
            }
            MemberProp::Private(_) => {
                self.emit(Op::SetPrivate);
            }
        }
        self.emit(Op::Pop);
        Ok(())
    }

    /// Inside `with` (or a function with eval vars): resolve `name` once,
    /// pushing its base — the object that has it, or undefined for the
    /// static binding. Returns false (nothing emitted) outside `with`.
    fn emit_with_ref(&mut self, name: &str) -> bool {
        let chain = self.with_chain(name);
        if chain.is_empty() {
            return false;
        }
        let i = self.const_str(name);
        let mut done = Vec::new();
        for w in &chain {
            self.emit_get_var(w);
            self.emit(Op::WithHas(i));
            let skip = self.emit(Op::JumpIfFalse(0));
            self.emit_get_var(w);
            done.push(self.emit(Op::Jump(0)));
            self.patch_jump(skip);
        }
        self.emit(Op::Undefined);
        for j in done {
            self.patch_jump(j);
        }
        true
    }

    /// [base] -> [base value] for a reference from [`Self::emit_with_ref`].
    fn emit_ref_get(&mut self, name: &str) {
        let i = self.const_str(name);
        self.emit(Op::Dup);
        let st = self.emit(Op::JumpIfUndefined(0));
        self.emit(Op::Dup);
        self.emit_ic(|ic| Op::GetProp(i, ic));
        let end = self.emit(Op::Jump(0));
        self.patch_jump(st);
        self.emit_get_var_static(name);
        self.patch_jump(end);
    }

    /// [base value] -> [value] (PutValue on the resolved reference).
    fn emit_ref_set_keep(&mut self, name: &str) -> CResult<()> {
        let i = self.const_str(name);
        // [base v] -> [v v base]; test the base
        self.emit(Op::Dup);
        self.emit(Op::Rot3);
        self.emit(Op::Dup);
        let st = self.emit(Op::JumpIfUndefined(0));
        if self.fs_ref().flags.is_strict {
            // Strict code: the binding must still exist on the object.
            self.emit(Op::Dup);
            self.emit(Op::WithHas(i));
            let ok = self.emit(Op::JumpIfTrue(0));
            let m = self.const_str(&format!("{name} is not defined"));
            self.emit(Op::ThrowConst(0, m));
            self.patch_jump(ok);
        }
        self.emit(Op::Swap);
        self.emit_ic(|ic| Op::SetProp(i, ic));
        self.emit(Op::Pop);
        let end = self.emit(Op::Jump(0));
        self.patch_jump(st);
        self.emit(Op::Pop);
        self.emit_set_var_static(name)?;
        self.patch_jump(end);
        Ok(())
    }

    fn compile_update(&mut self, op: UpdateOp, prefix: bool, target: &Expr) -> CResult<()> {
        let inc = if op == UpdateOp::Increment { Op::Inc } else { Op::Dec };
        let undo = if op == UpdateOp::Increment { Op::Dec } else { Op::Inc };
        match &target.kind {
            ExprKind::Identifier(n) if self.emit_with_ref(n) => {
                self.emit_ref_get(n);
                if !prefix {
                    self.emit(Op::ToNumeric);
                }
                self.emit(inc);
                self.emit_ref_set_keep(n)?;
                if !prefix {
                    self.emit(undo);
                }
            }
            ExprKind::Identifier(n) => {
                self.emit_get_var(n);
                if !prefix {
                    // The old value (result of the postfix form) is numeric.
                    self.emit(Op::ToNumeric);
                }
                self.emit(inc);
                self.emit(Op::Dup);
                self.emit_set_var(n)?;
                if !prefix {
                    self.emit(undo);
                }
            }
            ExprKind::Member { object, property, .. } => {
                self.compile_expr(object)?;
                match property {
                    MemberProp::Identifier(name) => {
                        let i = self.const_str(name);
                        self.emit_ic(|ic| Op::GetPropKeep(i, ic));
                        self.emit(Op::ToNumeric);
                        self.emit(inc);
                        self.emit_ic(|ic| Op::SetProp(i, ic));
                    }
                    MemberProp::Private(name) => {
                        self.emit_private_name(name)?;
                        self.emit(Op::GetPrivateKeep);
                        self.emit(Op::ToNumeric);
                        self.emit(inc);
                        self.emit(Op::SetPrivate);
                    }
                    MemberProp::Computed(k) => {
                        self.compile_expr(k)?;
                        self.emit(Op::CheckCoercibleUnder);
                        self.emit(Op::ToPropertyKey);
                        self.emit(Op::GetElemKeep);
                        self.emit(Op::ToNumeric);
                        self.emit(inc);
                        self.emit(Op::SetElem);
                    }
                }
                if !prefix {
                    self.emit(undo);
                }
            }
            _ => return self.err("Invalid left-hand side expression in postfix operation"),
        }
        Ok(())
    }

    fn compile_assign(&mut self, op: AssignOp, target: &Pattern, value: &Expr) -> CResult<()> {
        if op == AssignOp::Assign {
            // Member targets evaluate object/key before the value.
            if let Pattern::Expr(e) = target {
                if let ExprKind::Member { object, property, optional: false } = &e.kind {
                    if !matches!(object.kind, ExprKind::Super) {
                        self.compile_expr(object)?;
                        match property {
                            MemberProp::Identifier(name) => {
                                self.compile_expr(value)?;
                                let i = self.const_str(name);
                                self.emit_ic(|ic| Op::SetProp(i, ic));
                            }
                            MemberProp::Private(name) => {
                                self.emit_private_name(name)?;
                                self.compile_expr(value)?;
                                self.emit(Op::SetPrivate);
                            }
                            MemberProp::Computed(k) => {
                                self.compile_expr(k)?;
                                self.emit(Op::ToPropertyKey);
                                self.compile_expr(value)?;
                                self.emit(Op::SetElem);
                            }
                        }
                        return Ok(());
                    } else {
                        match property {
                            MemberProp::Identifier(name) => {
                                self.compile_expr(value)?;
                                let i = self.const_str(name);
                                self.emit(Op::SetSuperProp(i));
                            }
                            MemberProp::Computed(k) => {
                                self.compile_expr(k)?;
                                self.emit(Op::ToPropertyKey);
                                self.compile_expr(value)?;
                                self.emit(Op::SetSuperElem);
                            }
                            MemberProp::Private(_) => return self.err("Unexpected private field"),
                        }
                        return Ok(());
                    }
                }
            }
            if let Pattern::Identifier(n) = target {
                // Inside `with`: the reference is resolved before the value.
                if self.emit_with_ref(n) {
                    self.compile_expr_named(value, Some(n))?;
                    return self.emit_ref_set_keep(n);
                }
            }
            let name = pattern_name(target);
            self.compile_expr_named(value, name)?;
            self.emit(Op::Dup);
            return self.compile_pattern_store(target, false);
        }
        let target_expr: &Expr = match target {
            Pattern::Identifier(n) if self.emit_with_ref(n) => {
                let logical = matches!(op, AssignOp::And | AssignOp::Or | AssignOp::Nullish);
                self.emit_ref_get(n);
                if logical {
                    let j = match op {
                        AssignOp::And => self.emit(Op::JumpIfFalseKeep(0)),
                        AssignOp::Or => self.emit(Op::JumpIfTrueKeep(0)),
                        _ => self.emit(Op::JumpIfNotNullishKeep(0)),
                    };
                    if op != AssignOp::Nullish {
                        self.emit(Op::Pop);
                    }
                    self.compile_expr_named(value, Some(n))?;
                    self.emit_ref_set_keep(n)?;
                    let end = self.emit(Op::Jump(0));
                    self.patch_jump(j);
                    // [base cur] -> [cur]
                    self.emit(Op::Swap);
                    self.emit(Op::Pop);
                    self.patch_jump(end);
                } else {
                    self.compile_expr(value)?;
                    self.emit(binop_for_assign(op));
                    self.emit_ref_set_keep(n)?;
                }
                return Ok(());
            }
            Pattern::Identifier(n) => {
                // simple variable compound assignment
                let logical = matches!(op, AssignOp::And | AssignOp::Or | AssignOp::Nullish);
                self.emit_get_var(n);
                if logical {
                    let j = match op {
                        AssignOp::And => self.emit(Op::JumpIfFalseKeep(0)),
                        AssignOp::Or => self.emit(Op::JumpIfTrueKeep(0)),
                        _ => self.emit(Op::JumpIfNotNullishKeep(0)),
                    };
                    if op != AssignOp::Nullish {
                        self.emit(Op::Pop);
                    }
                    self.compile_expr_named(value, Some(n))?;
                    self.emit(Op::Dup);
                    self.emit_set_var(n)?;
                    self.patch_jump(j);
                } else {
                    self.compile_expr(value)?;
                    self.emit(binop_for_assign(op));
                    self.emit(Op::Dup);
                    self.emit_set_var(n)?;
                }
                return Ok(());
            }
            Pattern::Expr(e) => e,
            _ => return self.err("Invalid left-hand side in assignment"),
        };
        let (object, property) = match &target_expr.kind {
            ExprKind::Member { object, property, .. } => (object, property),
            _ => return self.err("Invalid left-hand side in assignment"),
        };
        if let ExprKind::Super = object.kind {
            return self.err("Compound assignment to super properties is not supported");
        }
        self.compile_expr(object)?;
        let logical = matches!(op, AssignOp::And | AssignOp::Or | AssignOp::Nullish);
        match property {
            MemberProp::Identifier(name) => {
                let i = self.const_str(name);
                self.emit_ic(|ic| Op::GetPropKeep(i, ic));
                if logical {
                    let j = match op {
                        AssignOp::And => self.emit(Op::JumpIfFalseKeep(0)),
                        AssignOp::Or => self.emit(Op::JumpIfTrueKeep(0)),
                        _ => self.emit(Op::JumpIfNotNullishKeep(0)),
                    };
                    if op != AssignOp::Nullish {
                        self.emit(Op::Pop);
                    }
                    self.compile_expr(value)?;
                    self.emit_ic(|ic| Op::SetProp(i, ic));
                    let end = self.emit(Op::Jump(0));
                    self.patch_jump(j);
                    // stack: obj val -> val
                    self.emit(Op::Swap);
                    self.emit(Op::Pop);
                    self.patch_jump(end);
                } else {
                    self.compile_expr(value)?;
                    self.emit(binop_for_assign(op));
                    self.emit_ic(|ic| Op::SetProp(i, ic));
                }
            }
            MemberProp::Private(_) | MemberProp::Computed(_) => {
                let private = matches!(property, MemberProp::Private(_));
                let (get_keep, set) = if private { (Op::GetPrivateKeep, Op::SetPrivate) } else { (Op::GetElemKeep, Op::SetElem) };
                match property {
                    MemberProp::Private(n) => self.emit_private_name(n)?,
                    MemberProp::Computed(k) => {
                        self.compile_expr(k)?;
                        self.emit(Op::CheckCoercibleUnder);
                        self.emit(Op::ToPropertyKey);
                    }
                    _ => unreachable!(),
                }
                self.emit(get_keep);
                if logical {
                    let j = match op {
                        AssignOp::And => self.emit(Op::JumpIfFalseKeep(0)),
                        AssignOp::Or => self.emit(Op::JumpIfTrueKeep(0)),
                        _ => self.emit(Op::JumpIfNotNullishKeep(0)),
                    };
                    if op != AssignOp::Nullish {
                        self.emit(Op::Pop);
                    }
                    self.compile_expr(value)?;
                    self.emit(set);
                    let end = self.emit(Op::Jump(0));
                    self.patch_jump(j);
                    // stack: obj key val -> val
                    self.emit(Op::Swap);
                    self.emit(Op::Pop);
                    self.emit(Op::Swap);
                    self.emit(Op::Pop);
                    self.patch_jump(end);
                } else {
                    self.compile_expr(value)?;
                    self.emit(binop_for_assign(op));
                    self.emit(set);
                }
            }
        }
        Ok(())
    }

    fn compile_object_literal(&mut self, props: &[ObjectProperty]) -> CResult<()> {
        self.emit(Op::NewObject);
        for p in props {
            match p {
                ObjectProperty::Spread(e) => {
                    self.compile_expr(e)?;
                    self.emit(Op::CopyDataProps);
                }
                ObjectProperty::Property { key, value, shorthand } => {
                    let static_name: Option<String> = match key {
                        PropertyKey::Identifier(n) | PropertyKey::String(n) => Some(n.to_string()),
                        PropertyKey::Number(n) => Some(crate::value::number_to_string(*n)),
                        _ => None,
                    };
                    match value {
                        PropertyValue::Init(e) => {
                            if let (Some(n), false) = (&static_name, shorthand) {
                                if n == "__proto__" && !matches!(key, PropertyKey::Computed(_)) {
                                    if let PropertyKey::Identifier(_) | PropertyKey::String(_) = key {
                                        self.compile_expr(e)?;
                                        self.emit(Op::SetProtoLiteral);
                                        continue;
                                    }
                                }
                            }
                            match key {
                                PropertyKey::Computed(k) => {
                                    self.compile_expr(k)?;
                                    self.emit(Op::ToPropertyKey);
                                    self.compile_expr(e)?;
                                    self.emit(Op::DefineElem);
                                }
                                _ => {
                                    let n = static_name.clone().unwrap();
                                    self.compile_expr_named(e, Some(&n))?;
                                    let i = self.const_str(&n);
                                    self.emit_ic(|ic| Op::DefineProp(i, ic));
                                }
                            }
                        }
                        PropertyValue::Method(f) | PropertyValue::Getter(f) | PropertyValue::Setter(f) => {
                            match key {
                                PropertyKey::Computed(k) => {
                                    self.compile_expr(k)?;
                                    self.emit(Op::ToPropertyKey);
                                }
                                _ => {
                                    let n = static_name.clone().unwrap();
                                    self.emit_string(&n);
                                }
                            }
                            let idx = self.compile_method_rc(f, static_name.as_deref())?;
                            self.emit(Op::MakeClosure(idx));
                            match value {
                                PropertyValue::Method(_) => self.emit(Op::DefineMethod(1)),
                                PropertyValue::Getter(_) => self.emit(Op::DefineGetter(1)),
                                _ => self.emit(Op::DefineSetter(1)),
                            };
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// A method / accessor from the source (compiled lazily like functions).
    fn compile_method_rc(&mut self, f: &Rc<Function>, name: Option<&str>) -> CResult<u32> {
        let idx = self.compile_function_rc(f, name)?;
        let fs = self.fs();
        let mut flags = fs.functions[idx as usize].flags;
        flags.is_method = true;
        set_flags(&mut fs.functions[idx as usize], flags);
        Ok(idx)
    }

    fn compile_method(&mut self, f: &Function, name: Option<&str>, _is_static: bool) -> CResult<u32> {
        let idx = self.compile_function(f, name)?;
        // mark as method (no prototype, not constructible)
        let fs = self.fs();
        let code = &fs.functions[idx as usize];
        let mut flags = code.flags;
        flags.is_method = true;
        set_flags(&mut fs.functions[idx as usize], flags);
        Ok(idx)
    }

    // -------------------------------------------------------------- classes

    fn compile_class(&mut self, c: &Class) -> CResult<()> {
        self.compile_class_expr(c, false)
    }

    fn resolve_private(&mut self, name: &str) -> CResult<(u16, u16)> {
        match self.resolve(&format!("#{}", name)) {
            Resolved::Local { depth, slot, .. } => Ok((depth, slot)),
            Resolved::Reg { .. } => self.err("internal compiler error: private name in a register"),
            Resolved::Global => self.err(&format!("Private field '#{}' must be declared in an enclosing class", name)),
        }
    }

    fn emit_private_name(&mut self, name: &str) -> CResult<()> {
        let (d, s) = self.resolve_private(name)?;
        self.emit(Op::GetLocal(d, s));
        Ok(())
    }

    /// Leaves the constructor on the stack.
    fn compile_class_expr(&mut self, c: &Class, _is_expr: bool) -> CResult<()> {
        self.set_pos(c.pos);
        // Class scope: name binding + private names + computed field keys.
        let mut decls: Vec<(String, BindKind)> = Vec::new();
        if let Some(n) = &c.name {
            decls.push((n.to_string(), BindKind::Const));
        }
        let mut private_names: Vec<String> = Vec::new();
        for m in &c.members {
            let key = match m {
                ClassMember::Method { key, .. } | ClassMember::Getter { key, .. } | ClassMember::Setter { key, .. } | ClassMember::Field { key, .. } => Some(key),
                _ => None,
            };
            if let Some(PropertyKey::Private(n)) = key {
                let full = format!("#{}", n);
                if !private_names.contains(&full) {
                    private_names.push(full.clone());
                    decls.push((full, BindKind::Private));
                }
            }
        }
        // Instance private methods/accessors live on a template object
        // copied into each instance before its fields are initialized.
        let has_private_methods = c.members.iter().any(|m| {
            matches!(m, ClassMember::Method { key: PropertyKey::Private(_), is_static: false, .. } | ClassMember::Getter { key: PropertyKey::Private(_), is_static: false, .. } | ClassMember::Setter { key: PropertyKey::Private(_), is_static: false, .. })
        });
        if has_private_methods {
            decls.push(("%privtpl".to_string(), BindKind::Hidden));
        }
        // Hidden slots for computed field keys (evaluated once).
        let mut computed_field_slots: Vec<String> = Vec::new();
        let mut field_index = 0;
        for m in &c.members {
            if let ClassMember::Field { key: PropertyKey::Computed(_), .. } = m {
                let n = format!("%fieldkey{}", field_index);
                computed_field_slots.push(n.clone());
                decls.push((n, BindKind::Hidden));
            }
            field_index += 1;
        }
        let _ = field_index;
        // Class scope bindings are referenced from methods and synthesized
        // field initializers: keep them all on the heap.
        let mat = self.push_block_scope_with(&decls, true);
        self.fs().class_depth += 1;
        for pn in &private_names {
            let desc = self.const_str(pn);
            self.emit(Op::NewPrivateName(desc));
            self.emit_init_var(pn)?;
        }
        // Superclass
        let has_super = c.superclass.is_some();
        match &c.superclass {
            Some(s) => self.compile_expr(s)?,
            None => {
                self.emit(Op::Empty);
            }
        }
        // Constructor
        let mut ctor_fn: Function = match c.members.iter().find_map(|m| if let ClassMember::Constructor(f) = m { Some(f.clone()) } else { None }) {
            Some(f) => f,
            None => synth_constructor(has_super, c.pos),
        };
        // The class's source text is its constructor's toString().
        ctor_fn.span = c.span;
        let class_name = c.name.clone().unwrap_or_default();
        let ctor_idx = self.compile_function_with(&ctor_fn, Some(&class_name), move |fl| {
            fl.is_class_constructor = true;
            fl.is_derived = has_super;
        })?;
        self.emit(Op::MakeClass(ctor_idx));
        // stack: ctor proto
        let mut deferred_static: Vec<(Option<String>, Option<(&PropertyKey, &Option<Expr>)>, Option<&Vec<Stmt>>)> = Vec::new();
        let mut instance_fields: Vec<(&PropertyKey, &Option<Expr>, usize)> = Vec::new();
        let mut computed_i = 0;
        let mut fi = 0;
        for m in &c.members {
            match m {
                ClassMember::Constructor(_) => {}
                ClassMember::Method { key, value, is_static } | ClassMember::Getter { key, value, is_static } | ClassMember::Setter { key, value, is_static } => {
                    // target object on top
                    if *is_static {
                        self.emit(Op::Swap);
                    }
                    self.emit(Op::Dup);
                    let name = self.emit_class_key(key)?;
                    let idx = self.compile_method_rc(value, name.as_deref())?;
                    self.emit(Op::MakeClosure(idx));
                    let flag = if matches!(key, PropertyKey::Private(_)) { 0 } else { 0 };
                    match m {
                        ClassMember::Method { .. } => self.emit(Op::DefineMethod(flag)),
                        ClassMember::Getter { .. } => self.emit(Op::DefineGetter(flag)),
                        _ => self.emit(Op::DefineSetter(flag)),
                    };
                    self.emit(Op::Pop);
                    if *is_static {
                        self.emit(Op::Swap);
                    }
                }
                ClassMember::Field { key, value, is_static } => {
                    if let PropertyKey::Computed(k) = key {
                        // evaluate key now into hidden slot
                        self.compile_expr(k)?;
                        self.emit(Op::ToPropertyKey);
                        let slot_name = computed_field_slots[computed_i].clone();
                        computed_i += 1;
                        self.emit_init_var(&slot_name)?;
                        if !*is_static {
                            instance_fields.push((key, value, computed_i - 1));
                        } else {
                            // static field with computed key
                            deferred_static.push((Some(slot_name), Some((key, value)), None));
                        }
                    } else if *is_static {
                        deferred_static.push((None, Some((key, value)), None));
                    } else {
                        instance_fields.push((key, value, usize::MAX));
                    }
                }
                ClassMember::StaticBlock(body) => deferred_static.push((None, None, Some(body))),
            }
            fi += 1;
        }
        let _ = fi;
        // The inner class binding is initialized once every element is
        // defined (computed keys see it in its TDZ).
        if let Some(n) = &c.name {
            self.emit(Op::Swap);
            self.emit(Op::Dup);
            self.emit_init_var(n)?;
            self.emit(Op::Swap);
        }
        if has_private_methods {
            // stack: ctor proto
            self.emit(Op::Dup);
            self.emit(Op::TakePrivate);
            self.emit_init_var("%privtpl")?;
        }
        self.emit_instance_fields(c, &instance_fields, &computed_field_slots, has_private_methods)?;
        // Static fields and blocks run last, in order.
        for (slot, field, block) in deferred_static {
            if let Some((key, value)) = field {
                self.emit_static_field(slot.as_deref(), key, value)?;
            }
            if let Some(body) = block {
                    // run as a method with this = ctor
                    let f = Function {
                        name: None,
                        params: vec![],
                        body: FunctionBody::Block(body.clone()),
                        is_arrow: false,
                        is_async: false,
                        is_generator: false,
                        strict: true,
                        pos: c.pos,
                        span: (0, 0),
                        self_binding: false,
                    };
                    self.emit(Op::Swap); // proto ctor
                    self.emit(Op::Dup); // proto ctor ctor
                    let idx = self.compile_method(&f, Some(""), true)?;
                    self.emit(Op::MakeClosure(idx)); // proto ctor ctor fn
                    self.emit(Op::Swap); // proto ctor fn ctor
                    self.emit(Op::Call(0, crate::bytecode::NO_DESC)); // proto ctor result
                    self.emit(Op::Pop);
                    self.emit(Op::Swap); // ctor proto
            }
        }
        self.emit(Op::Pop); // drop proto
        self.fs().class_depth -= 1;
        self.pop_block_scope(mat);
        Ok(())
    }

    /// The instance field initializer function of a class (private
    /// methods first, then fields in order). Stack: ctor proto.
    fn emit_instance_fields(&mut self, c: &Class, instance_fields: &[(&PropertyKey, &Option<Expr>, usize)], computed_field_slots: &[String], has_private_methods: bool) -> CResult<()> {
        if !instance_fields.is_empty() || has_private_methods {
            let mut stmts: Vec<Stmt> = Vec::new();
            let pos = c.pos;
            if has_private_methods {
                stmts.push(Stmt { pos, kind: StmtKind::Expr(Expr { pos, kind: ExprKind::Call {
                    callee: Box::new(Expr { pos, kind: ExprKind::Identifier("%copyprivate".into()) }),
                    args: vec![],
                    optional: false,
                } }) });
            }
            for (key, value, slot_idx) in instance_fields {
                let value_expr = (*value).clone().unwrap_or(Expr { pos, kind: ExprKind::Undefined });
                let key_expr: ExprKind = match key {
                    PropertyKey::Identifier(n) | PropertyKey::String(n) => ExprKind::String(n.to_string()),
                    PropertyKey::Number(n) => ExprKind::Number(*n),
                    PropertyKey::Computed(_) => ExprKind::Identifier(computed_field_slots[*slot_idx].as_str().into()),
                    PropertyKey::Private(n) => ExprKind::Identifier(format!("#{}", n).into()),
                };
                // Represent as a synthetic statement: this[key] = value using DefineElem semantics.
                stmts.push(Stmt { pos, kind: StmtKind::Expr(Expr { pos, kind: ExprKind::Call {
                    callee: Box::new(Expr { pos, kind: ExprKind::Identifier("%definefield".into()) }),
                    args: vec![ArrayElement::Expr(Expr { pos, kind: key_expr }), ArrayElement::Expr(value_expr)],
                    optional: false,
                } }) });
            }
            let f = Function { name: None, params: vec![], body: FunctionBody::Block(stmts), is_arrow: false, is_async: false, is_generator: false, strict: true, pos, span: (0, 0), self_binding: false };
            let idx = self.compile_function_with(&f, Some(""), |fl| {
                fl.is_field_init = true;
                fl.is_method = true;
            })?;
            self.emit(Op::MakeClosure(idx));
            self.emit(Op::SetClassFields);
        }
        Ok(())
    }

    /// Push the property key for a class member; returns a static name if known.
    fn emit_class_key(&mut self, key: &PropertyKey) -> CResult<Option<String>> {
        Ok(match key {
            PropertyKey::Identifier(n) | PropertyKey::String(n) => {
                self.emit_string(n);
                Some(n.to_string())
            }
            PropertyKey::Number(n) => {
                let s = crate::value::number_to_string(*n);
                self.emit_string(&s);
                Some(s)
            }
            PropertyKey::Computed(e) => {
                self.compile_expr(e)?;
                self.emit(Op::ToPropertyKey);
                None
            }
            PropertyKey::Private(n) => {
                self.emit_private_name(n)?;
                Some(format!("#{}", n))
            }
        })
    }

    fn emit_static_field(&mut self, slot_name: Option<&str>, key: &PropertyKey, value: &Option<Expr>) -> CResult<()> {
        // stack: ctor proto ; define on ctor with this = ctor for the initializer
        self.emit(Op::Swap); // proto ctor
        self.emit(Op::Dup); // proto ctor ctor
        match slot_name {
            Some(n) => self.emit_get_var(n),
            None => {
                self.emit_class_key(key)?;
            }
        }
        // proto ctor ctor key
        // initializer as method called with this=ctor
        let pos = self.fs_ref().cur_pos;
        let value_expr = value.clone().unwrap_or(Expr { pos, kind: ExprKind::Undefined });
        let f = Function { name: None, params: vec![], body: FunctionBody::Expr(Box::new(value_expr)), is_arrow: false, is_async: false, is_generator: false, strict: true, pos, span: (0, 0), self_binding: false };
        let key_name = match key {
            PropertyKey::Identifier(n) | PropertyKey::String(n) => Some(n.clone()),
            _ => None,
        };
        let idx = self.compile_method(&f, key_name.as_deref(), true)?;
        self.emit(Op::MakeClosure(idx)); // proto ctor ctor key fn
        self.emit(Op::Rot3); // proto ctor key fn ctor  -- wait: Rot3 a b c -> b c a : (ctor key fn) -> (key fn ctor)
        self.emit(Op::Call(0, crate::bytecode::NO_DESC)); // proto ctor key value
        self.emit(if matches!(key, PropertyKey::Private(_)) { Op::DefinePrivate } else { Op::DefineElem }); // proto ctor
        self.emit(Op::Swap); // ctor proto
        Ok(())
    }
}

fn binop_for_assign(op: AssignOp) -> Op {
    match op {
        AssignOp::Add => Op::Add,
        AssignOp::Sub => Op::Sub,
        AssignOp::Mul => Op::Mul,
        AssignOp::Div => Op::Div,
        AssignOp::Mod => Op::Mod,
        AssignOp::Exp => Op::Exp,
        AssignOp::Shl => Op::Shl,
        AssignOp::Shr => Op::Shr,
        AssignOp::UShr => Op::UShr,
        AssignOp::BitAnd => Op::BitAnd,
        AssignOp::BitOr => Op::BitOr,
        AssignOp::BitXor => Op::BitXor,
        _ => Op::Nop,
    }
}

/// Short source-like description of a callee (`o.f`, `a[0]`, `f`).
fn callee_description(e: &Expr) -> Option<String> {
    fn describe(e: &Expr, depth: usize) -> Option<String> {
        if depth > 3 {
            return None;
        }
        match &e.kind {
            ExprKind::Identifier(n) => Some(n.to_string()),
            ExprKind::This => Some("this".into()),
            ExprKind::Member { object, property, .. } => {
                let o = describe(object, depth + 1)?;
                Some(match property {
                    MemberProp::Identifier(n) => format!("{}.{}", o, n),
                    MemberProp::Private(n) => format!("{}.#{}", o, n),
                    MemberProp::Computed(k) => match &k.kind {
                        ExprKind::Number(n) => format!("{}[{}]", o, crate::value::number_to_string(*n)),
                        ExprKind::String(s) => format!("{}[\"{}\"]", o, s),
                        ExprKind::Identifier(n) => format!("{}[{}]", o, n),
                        _ => format!("{}[...]", o),
                    },
                })
            }
            ExprKind::OptionalChain(inner) => describe(inner, depth + 1),
            ExprKind::Call { callee, .. } => Some(format!("{}(...)", describe(callee, depth + 1)?)),
            _ => None,
        }
    }
    describe(e, 0)
}

fn pattern_name(p: &Pattern) -> Option<&str> {
    match p {
        Pattern::Identifier(n) => Some(n.as_str()),
        _ => None,
    }
}

fn synth_constructor(derived: bool, pos: Pos) -> Function {
    if derived {
        Function {
            name: None,
            params: vec![Param { pattern: Pattern::Identifier("args".into()), rest: true }],
            body: FunctionBody::Block(vec![Stmt {
                pos,
                kind: StmtKind::Expr(Expr {
                    pos,
                    kind: ExprKind::Call {
                        callee: Box::new(Expr { pos, kind: ExprKind::Super }),
                        args: vec![ArrayElement::Spread(Expr { pos, kind: ExprKind::Identifier("args".into()) })],
                        optional: false,
                    },
                }),
            }]),
            is_arrow: false,
            is_async: false,
            is_generator: false,
            strict: true,
            pos,
            span: (0, 0),
            self_binding: false,
        }
    } else {
        Function { name: None, params: vec![], body: FunctionBody::Block(vec![]), is_arrow: false, is_async: false, is_generator: false, strict: true, pos, span: (0, 0), self_binding: false }
    }
}

fn set_flags(code: &mut Rc<FunctionCode>, flags: CodeFlags) {
    if let Some(c) = Rc::get_mut(code) {
        c.flags = flags;
    }
}

fn finish(mut fs: FnState, module: Option<u32>) -> FunctionCode {
    let mut ic_keys = vec![0u32; fs.ic_count as usize];
    let tr = match crate::regs::translate(&fs.ops, &fs.positions, &fs.handlers, &mut fs.constants, fs.nregs as u32, &mut ic_keys) {
        Ok(t) => t,
        // Malformed stack code is a compiler bug; fail loudly at run time
        // rather than miscompiling.
        Err(e) => {
            let msg = format!("internal compiler error: {}", e);
            if std::env::var_os("RH_DEBUG_REGS").is_some() {
                eprintln!("[regs] {} in {}", msg, fs.name);
            }
            let k = fs.constants.len() as u32;
            fs.constants.push(Value::String(JsStr::new(&msg)));
            crate::regs::Translated { ops: vec![Op::ThrowConst(2, k)], positions: vec![Pos::default()], handlers: Vec::new(), depths: vec![0], max_depth: 1 }
        }
    };
    let mut tr = tr;
    // Number the call sites (each gets a cache).
    let mut ncalls = 0usize;
    for op in tr.ops.iter_mut() {
        if let Op::CallR(_, _, cic, _) = op {
            if ncalls < crate::bytecode::NO_CALL_IC as usize {
                *cic = ncalls as u16;
                ncalls += 1;
            }
        }
    }
    let scope = &fs.scopes[0];
    let mut slot_names = vec![JsStr::empty(); scope.slot_count as usize];
    for (n, b) in &scope.names {
        if let Loc::Slot(s) = b.loc {
            if (s as usize) < slot_names.len() {
                slot_names[s as usize] = JsStr::new(n);
            }
        }
    }
    let mut code = FunctionCode {
        name: fs.name,
        ops: tr.ops,
        positions: tr.positions,
        constants: fs.constants,
        functions: fs.functions,
        handlers: tr.handlers,
        param_count: fs.param_count,
        scope_slots: scope.slot_count as u32,
        nregs: fs.nregs as u32,
        reg_names: fs.reg_names,
        param_locs: fs.param_locs.iter().map(|l| l.to_var_loc()).collect(),
        rest_loc: fs.rest_loc.map(|l| l.to_var_loc()),
        arguments_loc: fs.arguments_loc.map(|l| l.to_var_loc()),
        self_loc: fs.self_loc.map(|l| l.to_var_loc()),
        flags: fs.flags,
        call_flags: 0,
        slot_names,
        block_names: fs.block_names,
        templates: fs.templates,
        ics: (0..fs.ic_count).map(|_| std::cell::Cell::new(crate::bytecode::PropIc::default())).collect(),
        call_ics: (0..ncalls).map(|_| std::cell::Cell::new(crate::bytecode::CallIc::default())).collect(),
        lazy: None,
        eval_sites: std::mem::take(&mut fs.eval_sites),
        compiled: std::cell::OnceCell::new(),
        jit: Default::default(),
        ic_keys: ic_keys.into(),
        depths: tr.depths,
        max_depth: tr.max_depth,
        source: fs.source,
        source_span: fs.span,
        filename: fs.filename,
        module,
    };
    code.call_flags = code.compute_call_flags();
    code
}

fn opt_ptr_eq(a: &Option<Rc<LazyScope>>, b: &Option<Rc<LazyScope>>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => Rc::ptr_eq(x, y),
        (None, None) => true,
        _ => false,
    }
}
