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
}

impl CScope {
    fn new(materialized: bool, is_function: bool) -> CScope {
        CScope { names: HashMap::default(), slot_count: 0, materialized, is_function, force_heap: false, tdz: Vec::new() }
    }
}

#[derive(Debug, Clone)]
enum ExitCtx {
    /// A runtime scope that must be popped when jumping out.
    Scope,
    /// Inline this finalizer. The index identifies the owning try context
    /// (whose handler ranges are suspended while the finalizer is inlined).
    Finally { body: Rc<Vec<Stmt>>, try_index: usize },
    /// Close the for-of iterator held in hidden slots.
    IterClose { iter: u16, next: u16 },
}

#[derive(Debug)]
struct TryCtx {
    /// Completed ranges + an open start.
    ranges: Vec<(u32, u32)>,
    open_start: Option<u32>,
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
}

impl FnState {
    fn new(name: &str, flags: CodeFlags, is_script: bool, filename: Rc<str>) -> FnState {
        let mut scopes = Vec::new();
        scopes.push(CScope::new(true, true));
        FnState {
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
        }
    }
}

pub struct Compiler {
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

/// Compile a whole script/module.
pub fn compile_program(program: &Program, filename: &str, source: &str) -> CResult<Rc<FunctionCode>> {
    let mut c = Compiler {
        fns: Vec::new(),
        filename: Rc::from(filename),
        source: Rc::from(source),
        pending_tweak: None,
        chain_stack: Vec::new(),
        loose_chain_exits: Vec::new(),
        module: None,
        captures: crate::captures::analyze_program(&program.body),
        internal_error: None,
    };
    let code = c.compile_script(program)?;
    c.check_internal()?;
    Ok(code)
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
        let heap = fs.force_heap || fs.scopes[si].force_heap || fs.captured.contains(name) || fs.nregs == u16::MAX;
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
        scope.names.insert(name.to_string(), Binding { loc, kind });
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
        Resolved::Global
    }

    fn emit_get_var(&mut self, name: &str) {
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
                if matches!(kind, BindKind::Let | BindKind::Const | BindKind::Class) {
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
        match self.resolve(name) {
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
                } else if matches!(kind, BindKind::Let | BindKind::Class) {
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
            Resolved::Reg { reg, .. } => {
                self.emit(Op::SetReg(reg));
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
            Pattern::Identifier(n) => out.push(n.clone()),
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
                if let Some(ForInit::VarDecl(d)) = init {
                    if d.kind == VarKind::Var {
                        for decl in &d.declarations {
                            Self::collect_pattern_names(&decl.target, out);
                        }
                    }
                }
                Self::collect_var_names_stmt(body, out);
            }
            StmtKind::ForIn { left, body, .. } | StmtKind::ForOf { left, body, .. } => {
                if let ForHead::VarDecl { kind: VarKind::Var, target } = left {
                    Self::collect_pattern_names(target, out);
                }
                Self::collect_var_names_stmt(body, out);
            }
            StmtKind::While { body, .. } | StmtKind::DoWhile { body, .. } | StmtKind::Labeled { body, .. } | StmtKind::With { body, .. } => {
                Self::collect_var_names_stmt(body, out)
            }
            StmtKind::Try { block, handler, finalizer } => {
                Self::collect_var_names(block, out);
                if let Some((_, h)) = handler {
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
                    Some(n) => out.push((n.clone(), BindKind::Class)),
                    None => out.push((DEFAULT_EXPORT_BINDING.to_string(), BindKind::Class)),
                },
                StmtKind::FunctionDecl(f) if block_functions => match &f.name {
                    Some(n) => out.push((n.clone(), BindKind::Function)),
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
                let name = f.name.clone().unwrap_or_else(|| DEFAULT_EXPORT_BINDING.to_string());
                let idx = self.compile_function(f, Some(if f.name.is_some() { &name } else { "default" }))?;
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
        fs.force_heap = program.is_module;
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
            for (n, k) in lex {
                // Track as script-level lexical for emit_init_var.
                self.fs().scopes[0].names.insert(n.clone(), Binding { loc: SCRIPT_LEX, kind: k });
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
        fs.class_depth = self.fs_ref().class_depth;
        let (captured, temp_keys) = self.captured_for(f);
        fs.captured = captured;
        self.fns.push(fs);
        self.set_pos(f.pos);

        // Parameters.
        let mut param_count = 0u32;
        let mut complex: Vec<(Loc, &Param)> = Vec::new();
        let mut seen_default = false;
        let mut param_slots: Vec<Loc> = Vec::new();
        let mut rest_slot: Option<Loc> = None;
        for p in &f.params {
            if !p.rest && !matches!(p.pattern, Pattern::Assign { .. }) && !seen_default {
                param_count += 1;
            } else {
                seen_default = true;
            }
            match &p.pattern {
                Pattern::Identifier(n) if !p.rest => {
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
            FunctionBody::Expr(_) => &[],
        };
        let mut vars = Vec::new();
        Self::collect_var_names(body_stmts, &mut vars);
        for v in vars {
            self.declare(&v, BindKind::Var);
        }
        let lex = Self::collect_lexical(body_stmts, true);
        for (n, k) in &lex {
            self.declare(n, *k);
        }
        // Rest/pattern params and defaults declare their names as vars too.
        for (_, p) in &complex {
            let mut names = Vec::new();
            Self::collect_pattern_names(&p.pattern, &mut names);
            for n in names {
                self.declare(&n, BindKind::Var);
            }
        }
        // Named function expression self-binding. It lives in an outer
        // environment per spec, so any parameter, `var`, lexical or inner
        // function declaration with the same name shadows it: only bind it
        // when nothing in the body declares the name.
        if let Some(n) = &f.name {
            if !f.is_arrow && !self.fs_ref().scopes[0].names.contains_key(n) {
                let loc = self.declare(n, BindKind::Hidden);
                self.fs().self_loc = Some(loc);
            }
        }
        // An `arguments` object referenced from nested arrows must live in
        // the heap scope: declare it now, before the scope layout is fixed.
        if !f.is_arrow && self.fs_ref().captured.contains("arguments") && !self.fs_ref().scopes[0].names.contains_key("arguments") {
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
        for (loc, p) in complex {
            // Rest arguments arrive as an array in the hidden binding.
            self.emit_load_loc(loc);
            self.compile_pattern_store(&p.pattern, true)?;
        }
        // Simple params with defaults: `function f(a = 1)`
        // (handled above since `Assign` patterns are complex).
        if self.fs_ref().flags.is_class_constructor && !self.fs_ref().flags.is_derived {
            self.emit(Op::InitFields);
        }
        self.hoist_functions(body_stmts)?;
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

    fn compile_stmt(&mut self, s: &Stmt) -> CResult<()> {
        self.set_pos(s.pos);
        match &s.kind {
            StmtKind::Empty | StmtKind::Debugger => {}
            StmtKind::Expr(e) => {
                if self.fns.len() == 1 && self.fs_ref().is_script {
                    self.compile_expr(e)?;
                    self.emit(Op::StoreCompletion);
                } else {
                    self.compile_expr_discard(e)?;
                }
            }
            StmtKind::VarDecl(d) => self.compile_var_decl(d)?,
            StmtKind::FunctionDecl(_) => {} // hoisted
            StmtKind::ClassDecl(c) => {
                self.compile_class(c)?;
                let name = c.name.clone().unwrap_or_default();
                self.emit_init_var(&name)?;
            }
            StmtKind::Block(b) => self.compile_block(b)?,
            StmtKind::If { test, consequent, alternate } => {
                self.compile_expr(test)?;
                let j = self.emit(Op::JumpIfFalse(0));
                self.compile_stmt(consequent)?;
                if let Some(alt) = alternate {
                    let j2 = self.emit(Op::Jump(0));
                    self.patch_jump(j);
                    self.compile_stmt(alt)?;
                    self.patch_jump(j2);
                } else {
                    self.patch_jump(j);
                }
            }
            StmtKind::While { test, body } => {
                let start = self.here();
                self.compile_expr(test)?;
                let j = self.emit(Op::JumpIfFalse(0));
                self.push_loop(None, true);
                self.compile_stmt(body)?;
                let lp = self.fs().loops.pop().unwrap();
                self.patch_continues(&lp, start as u32);
                self.emit(Op::Jump(start as u32));
                self.patch_jump(j);
                self.patch_breaks(&lp);
            }
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
            StmtKind::For { init, test, update, body } => self.compile_for(init, test, update, body, None)?,
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
                self.emit_exits(0)?;
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
            StmtKind::Try { block, handler, finalizer } => self.compile_try(block, handler.as_ref(), finalizer.as_ref())?,
            StmtKind::Switch { discriminant, cases } => self.compile_switch(discriminant, cases)?,
            StmtKind::Labeled { label, body } => {
                match &body.kind {
                    StmtKind::For { init, test, update, body: b } => self.compile_for(init, test, update, b, Some(label.clone()))?,
                    StmtKind::ForIn { left, right, body: b } => self.compile_for_in(left, right, b, Some(label.clone()))?,
                    StmtKind::ForOf { left, right, body: b, is_await } => self.compile_for_of(left, right, b, *is_await, Some(label.clone()))?,
                    StmtKind::While { test, body: b } => {
                        let start = self.here();
                        self.compile_expr(test)?;
                        let j = self.emit(Op::JumpIfFalse(0));
                        self.push_loop(Some(label.clone()), true);
                        self.compile_stmt(b)?;
                        let lp = self.fs().loops.pop().unwrap();
                        self.patch_continues(&lp, start as u32);
                        self.emit(Op::Jump(start as u32));
                        self.patch_jump(j);
                        self.patch_breaks(&lp);
                    }
                    StmtKind::DoWhile { body: b, test } => {
                        let start = self.here();
                        self.push_loop(Some(label.clone()), true);
                        self.compile_stmt(b)?;
                        let lp = self.fs().loops.pop().unwrap();
                        let cont = self.here();
                        self.patch_continues(&lp, cont as u32);
                        self.compile_expr(test)?;
                        self.emit(Op::JumpIfTrue(start as u32));
                        self.patch_breaks(&lp);
                    }
                    _ => {
                        self.push_loop(Some(label.clone()), false);
                        self.compile_stmt(body)?;
                        let lp = self.fs().loops.pop().unwrap();
                        self.patch_breaks(&lp);
                    }
                }
            }
            StmtKind::With { .. } => return self.err("'with' statements are not supported"),
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
                        named.name = Some("default".to_string());
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
                    self.compile_pattern_store(&decl.target, true)?;
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

    /// Emit the exit actions for contexts above `depth` (innermost first).
    fn emit_exits(&mut self, depth: usize) -> CResult<()> {
        let exits: Vec<ExitCtx> = self.fs_ref().exits[depth..].to_vec();
        for (i, ctx) in exits.iter().enumerate().rev() {
            match ctx {
                ExitCtx::Scope => {
                    self.emit(Op::PopScope);
                }
                ExitCtx::IterClose { iter, next } => {
                    self.emit(Op::GetReg(*iter));
                    self.emit(Op::GetReg(*next));
                    self.emit(Op::IteratorClose);
                }
                ExitCtx::Finally { body, try_index } => {
                    // Inline the finalizer with inner contexts removed and the
                    // owning try's handler ranges suspended.
                    let saved_exits = self.fs().exits.split_off(depth + i);
                    let saved_scope_depth = self.fs_ref().scope_depth;
                    let n_tries = self.fs_ref().tries.len();
                    let suspended: Vec<usize> = (*try_index..n_tries).collect();
                    let here = self.here() as u32;
                    for &t in &suspended {
                        let tc = &mut self.fs().tries[t];
                        if let Some(s) = tc.open_start.take() {
                            if here > s {
                                tc.ranges.push((s, here));
                            }
                        }
                    }
                    // Scopes popped by the exits above are reflected in scope_depth for handler bookkeeping.
                    let popped = exits[i + 1..].iter().filter(|e| matches!(e, ExitCtx::Scope)).count() as u32;
                    self.fs().scope_depth = saved_scope_depth - popped;
                    let body = body.clone();
                    self.compile_block(&body)?;
                    self.fs().scope_depth = saved_scope_depth;
                    let here = self.here() as u32;
                    for &t in &suspended {
                        self.fs().tries[t].open_start = Some(here);
                    }
                    self.fs().exits.extend(saved_exits);
                }
            }
        }
        Ok(())
    }

    fn compile_for(&mut self, init: &Option<ForInit>, test: &Option<Expr>, update: &Option<Expr>, body: &Stmt, label: Option<String>) -> CResult<()> {
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
        let start = self.here();
        let mut exit_jump = None;
        if let Some(t) = test {
            self.compile_expr(t)?;
            exit_jump = Some(self.emit(Op::JumpIfFalse(0)));
        }
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
        self.emit(Op::Jump(start as u32));
        if let Some(j) = exit_jump {
            self.patch_jump(j);
        }
        self.patch_breaks(&lp);
        self.pop_block_scope(mat);
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
        let start = self.here() as u32;
        self.fs().tries.push(TryCtx { ranges: Vec::new(), open_start: Some(start) });
        if let Some(f) = &finalizer_rc {
            self.fs().exits.push(ExitCtx::Finally { body: f.clone(), try_index });
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
            self.compile_block(f)?;
            self.fs().exits.push(ExitCtx::Finally { body: f.clone(), try_index });
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
                self.compile_block(f)?;
                end_jumps.push(self.emit(Op::Jump(0)));
                // exception path for finally
                let fin_pc = self.here() as u32;
                for (s, e) in finally_ranges {
                    self.fs().handlers.push(Handler { start: s, end: e, target: fin_pc, stack_depth: 0, scope_depth });
                }
                self.compile_block(f)?;
                self.emit(Op::Throw);
            }
        } else if let Some(f) = &finalizer_rc {
            self.fs().exits.pop();
            let fin_pc = self.here() as u32;
            for (s, e) in catch_ranges {
                self.fs().handlers.push(Handler { start: s, end: e, target: fin_pc, stack_depth: 0, scope_depth });
            }
            self.compile_block(f)?;
            self.emit(Op::Throw);
        }
        self.fs().tries.pop();
        for j in end_jumps {
            self.patch_jump(j);
        }
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
                            self.emit(Op::SetElem);
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
                self.emit(Op::GetIterator);
                for el in elements {
                    self.emit(Op::IteratorStep);
                    match el {
                        Some(p) => self.compile_pattern_store(p, is_decl)?,
                        None => {
                            self.emit(Op::Pop);
                        }
                    }
                }
                if let Some(r) = rest {
                    self.emit(Op::IteratorRest);
                    self.compile_pattern_store(r, is_decl)?;
                }
                self.emit(Op::IteratorCloseIfNotDone);
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
                let idx = self.compile_function(f, Some(n))?;
                self.emit(Op::MakeClosure(idx));
                Ok(())
            }
            (ExprKind::Arrow(f), Some(n)) => {
                self.set_pos(e.pos);
                let idx = self.compile_function(f, Some(n))?;
                self.emit(Op::MakeClosure(idx));
                Ok(())
            }
            (ExprKind::Class(c), Some(n)) if c.name.is_none() => {
                let mut c2 = (**c).clone();
                c2.name = Some(n.to_string());
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
                for ex in expressions {
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
                let idx = self.compile_function(f, None)?;
                self.emit(Op::MakeClosure(idx));
            }
            ExprKind::Arrow(f) => {
                let idx = self.compile_function(f, None)?;
                self.emit(Op::MakeClosure(idx));
            }
            ExprKind::Class(c) => self.compile_class_expr(c, true)?,
            ExprKind::Unary { op, operand } => match op {
                UnaryOp::Typeof => {
                    if let ExprKind::Identifier(n) = &operand.kind {
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
                    ExprKind::Identifier(_) => {
                        self.emit(Op::False);
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
                    if n == "%definefield" {
                        // Synthetic class field definition: this[key] = value
                        self.emit(Op::This);
                        if let (Some(ArrayElement::Expr(k)), Some(ArrayElement::Expr(v))) = (args.first(), args.get(1)) {
                            self.compile_expr(k)?;
                            let name = match &k.kind {
                                ExprKind::String(s) => Some(s.as_str()),
                                _ => None,
                            };
                            self.compile_expr_named(v, name)?;
                        }
                        self.emit(Op::DefineElem);
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
                        self.emit(Op::GetElem);
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
                    // [iter next] ; received = undefined
                    self.emit(Op::Undefined);
                    let start = self.here();
                    self.emit(Op::IteratorNextWith);
                    if is_async {
                        self.emit(Op::Await);
                    }
                    // [iter next result]
                    self.emit(Op::Dup);
                    let d = self.const_str("done");
                    self.emit_ic(|ic| Op::GetProp(d, ic));
                    let exit = self.emit(Op::JumpIfTrue(0));
                    let v = self.const_str("value");
                    self.emit_ic(|ic| Op::GetProp(v, ic));
                    self.emit(if is_async { Op::AsyncYield } else { Op::Yield });
                    self.emit(Op::Jump(start as u32));
                    self.patch_jump(exit);
                    // [iter next result] -> value
                    self.emit_ic(|ic| Op::GetProp(v, ic));
                    self.emit(Op::Rot3);
                    self.emit(Op::Pop);
                    self.emit(Op::Swap);
                    self.emit(Op::Pop);
                } else if self.fs_ref().flags.is_async {
                    self.emit(Op::AsyncYield);
                } else {
                    self.emit(Op::Yield);
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
                        self.emit(Op::GetElem);
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

    fn compile_update(&mut self, op: UpdateOp, prefix: bool, target: &Expr) -> CResult<()> {
        let inc = if op == UpdateOp::Increment { Op::Inc } else { Op::Dec };
        let undo = if op == UpdateOp::Increment { Op::Dec } else { Op::Inc };
        match &target.kind {
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
                        self.emit(Op::GetElemKeep);
                        self.emit(Op::ToNumeric);
                        self.emit(inc);
                        self.emit(Op::SetElem);
                    }
                    MemberProp::Computed(k) => {
                        self.compile_expr(k)?;
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
                                self.emit(Op::SetElem);
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
            let name = pattern_name(target);
            self.compile_expr_named(value, name)?;
            self.emit(Op::Dup);
            return self.compile_pattern_store(target, false);
        }
        let target_expr: &Expr = match target {
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
                match property {
                    MemberProp::Private(n) => self.emit_private_name(n)?,
                    MemberProp::Computed(k) => {
                        self.compile_expr(k)?;
                        self.emit(Op::ToPropertyKey);
                    }
                    _ => unreachable!(),
                }
                self.emit(Op::GetElemKeep);
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
                    self.emit(Op::SetElem);
                    let end = self.emit(Op::Jump(0));
                    self.patch_jump(j);
                    // stack: obj key val -> val
                    self.emit(Op::Rot3);
                    self.emit(Op::Pop);
                    self.emit(Op::Pop);
                    self.patch_jump(end);
                } else {
                    self.compile_expr(value)?;
                    self.emit(binop_for_assign(op));
                    self.emit(Op::SetElem);
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
                        PropertyKey::Identifier(n) | PropertyKey::String(n) => Some(n.clone()),
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
                            let idx = self.compile_method(f, static_name.as_deref(), false)?;
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
            decls.push((n.clone(), BindKind::Const));
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
        let ctor_fn: Function = match c.members.iter().find_map(|m| if let ClassMember::Constructor(f) = m { Some(f.clone()) } else { None }) {
            Some(f) => f,
            None => synth_constructor(has_super, c.pos),
        };
        let class_name = c.name.clone().unwrap_or_default();
        let ctor_idx = self.compile_function_with(&ctor_fn, Some(&class_name), move |fl| {
            fl.is_class_constructor = true;
            fl.is_derived = has_super;
        })?;
        self.emit(Op::MakeClass(ctor_idx));
        // stack: ctor proto — initialise the inner class binding before static elements run
        if let Some(n) = &c.name {
            self.emit(Op::Swap);
            self.emit(Op::Dup);
            self.emit_init_var(n)?;
            self.emit(Op::Swap);
        }
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
                    let idx = self.compile_method(value, name.as_deref(), *is_static)?;
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
                            self.emit_static_field(Some(&slot_name), key, value)?;
                        }
                    } else if *is_static {
                        self.emit_static_field(None, key, value)?;
                    } else {
                        instance_fields.push((key, value, usize::MAX));
                    }
                }
                ClassMember::StaticBlock(body) => {
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
            fi += 1;
        }
        let _ = fi;
        // Instance field initializer function.
        if !instance_fields.is_empty() {
            let mut stmts: Vec<Stmt> = Vec::new();
            let pos = c.pos;
            for (key, value, slot_idx) in &instance_fields {
                let value_expr = (*value).clone().unwrap_or(Expr { pos, kind: ExprKind::Undefined });
                let key_expr: ExprKind = match key {
                    PropertyKey::Identifier(n) | PropertyKey::String(n) => ExprKind::String(n.clone()),
                    PropertyKey::Number(n) => ExprKind::Number(*n),
                    PropertyKey::Computed(_) => ExprKind::Identifier(computed_field_slots[*slot_idx].clone()),
                    PropertyKey::Private(n) => ExprKind::Identifier(format!("#{}", n)),
                };
                // Represent as a synthetic statement: this[key] = value using DefineElem semantics.
                stmts.push(Stmt { pos, kind: StmtKind::Expr(Expr { pos, kind: ExprKind::Call {
                    callee: Box::new(Expr { pos, kind: ExprKind::Identifier("%definefield".into()) }),
                    args: vec![ArrayElement::Expr(Expr { pos, kind: key_expr }), ArrayElement::Expr(value_expr)],
                    optional: false,
                } }) });
            }
            let f = Function { name: None, params: vec![], body: FunctionBody::Block(stmts), is_arrow: false, is_async: false, is_generator: false, strict: true, pos };
            let idx = self.compile_function_with(&f, Some(""), |fl| {
                fl.is_field_init = true;
                fl.is_method = true;
            })?;
            self.emit(Op::MakeClosure(idx));
            self.emit(Op::SetClassFields);
        }
        self.emit(Op::Pop); // drop proto
        self.fs().class_depth -= 1;
        self.pop_block_scope(mat);
        Ok(())
    }

    /// Push the property key for a class member; returns a static name if known.
    fn emit_class_key(&mut self, key: &PropertyKey) -> CResult<Option<String>> {
        Ok(match key {
            PropertyKey::Identifier(n) | PropertyKey::String(n) => {
                self.emit_string(n);
                Some(n.clone())
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
        let f = Function { name: None, params: vec![], body: FunctionBody::Expr(Box::new(value_expr)), is_arrow: false, is_async: false, is_generator: false, strict: true, pos };
        let key_name = match key {
            PropertyKey::Identifier(n) | PropertyKey::String(n) => Some(n.clone()),
            _ => None,
        };
        let idx = self.compile_method(&f, key_name.as_deref(), true)?;
        self.emit(Op::MakeClosure(idx)); // proto ctor ctor key fn
        self.emit(Op::Rot3); // proto ctor key fn ctor  -- wait: Rot3 a b c -> b c a : (ctor key fn) -> (key fn ctor)
        self.emit(Op::Call(0, crate::bytecode::NO_DESC)); // proto ctor key value
        self.emit(Op::DefineElem); // proto ctor
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
            ExprKind::Identifier(n) => Some(n.clone()),
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
        }
    } else {
        Function { name: None, params: vec![], body: FunctionBody::Block(vec![]), is_arrow: false, is_async: false, is_generator: false, strict: true, pos }
    }
}

fn set_flags(code: &mut Rc<FunctionCode>, flags: CodeFlags) {
    if let Some(c) = Rc::get_mut(code) {
        c.flags = flags;
    }
}

fn finish(mut fs: FnState, module: Option<u32>) -> FunctionCode {
    crate::bytecode::peephole(&mut fs.ops, &mut fs.positions, &mut fs.handlers);
    let scope = &fs.scopes[0];
    let mut slot_names = vec![JsStr::empty(); scope.slot_count as usize];
    for (n, b) in &scope.names {
        if let Loc::Slot(s) = b.loc {
            if (s as usize) < slot_names.len() {
                slot_names[s as usize] = JsStr::new(n);
            }
        }
    }
    FunctionCode {
        name: fs.name,
        ops: fs.ops,
        positions: fs.positions,
        constants: fs.constants,
        functions: fs.functions,
        handlers: fs.handlers,
        param_count: fs.param_count,
        scope_slots: scope.slot_count as u32,
        nregs: fs.nregs as u32,
        reg_names: fs.reg_names,
        param_locs: fs.param_locs.iter().map(|l| l.to_var_loc()).collect(),
        rest_loc: fs.rest_loc.map(|l| l.to_var_loc()),
        arguments_loc: fs.arguments_loc.map(|l| l.to_var_loc()),
        self_loc: fs.self_loc.map(|l| l.to_var_loc()),
        flags: fs.flags,
        slot_names,
        block_names: fs.block_names,
        templates: fs.templates,
        ics: (0..fs.ic_count).map(|_| std::cell::Cell::new(crate::bytecode::PropIc::default())).collect(),
        source: fs.source,
        filename: fs.filename,
        module,
    }
}
