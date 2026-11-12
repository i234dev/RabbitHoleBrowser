//! Bytecode definitions for the stack VM.

use crate::ast::Pos;
use crate::value::{JsStr, Value};
use std::rc::Rc;

/// One instruction. Operands are inline; indices refer to the owning
/// [`FunctionCode`]'s constant / function tables.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    // --- constants & stack
    Const(u32),
    Undefined,
    Null,
    True,
    False,
    Empty,
    Pop,
    Dup,
    /// Duplicate the top two values (a b -> a b a b).
    Dup2,
    Swap,
    /// a b c -> b c a  (rotate the third value to the top)
    Rot3,
    /// a b c d -> b c d a
    Rot4,

    // --- variables
    /// Push frame register `r` (bindings never captured by closures).
    GetReg(u16),
    /// Like GetReg but throws a ReferenceError when the register is in its TDZ.
    GetRegChecked(u16),
    /// Pop into register `r`.
    SetReg(u16),
    /// Pop into register `r`, throwing if it is still in its TDZ.
    SetRegChecked(u16),
    GetLocal(u16, u16),
    /// Like GetLocal but throws a ReferenceError when the slot is in its TDZ.
    GetLocalChecked(u16, u16),
    SetLocal(u16, u16),
    /// Store without TDZ/const checks (declaration initialisation).
    InitLocal(u16, u16),
    /// Store with const check (slot flagged const at compile time → TypeError).
    SetLocalChecked(u16, u16),
    /// Read a module import binding: the slot holds a module binding
    /// reference (dereferenced live, with a TDZ check) or a namespace object.
    GetImport(u16, u16),
    /// (name const, inline cache)
    GetGlobal(u32, u32),
    SetGlobal(u32, u32),
    /// `typeof ident` for unresolved identifiers (no ReferenceError).
    TypeofGlobal(u32),
    DeclareGlobalVar(u32),
    DeclareGlobalFunction(u32),
    /// (name, is_const)
    DeclareGlobalLex(u32, bool),
    InitGlobalLex(u32),
    /// Push a new declarative scope with n slots; second operand indexes
    /// `FunctionCode::block_names` (for error messages).
    PushScope(u16, u16),
    PopScope,
    /// Replace the current scope with a copy (per-iteration bindings).
    CopyScope,
    /// Mark slot as const (used by SetLocalChecked) — compile-time only; not an op.
    // --- properties
    /// (name const, inline cache)
    GetProp(u32, u32),
    SetProp(u32, u32),
    GetElem,
    SetElem,
    /// obj key -> obj value  (keeps obj for compound assignment)
    GetElemKeep,
    /// obj -> obj value
    GetPropKeep(u32, u32),
    /// obj -> method obj (`Dup; GetProp; Swap` fused for method calls)
    GetMethod(u32, u32),
    DeleteProp(u32),
    DeleteElem,
    /// Object literal helpers: obj value -> obj
    DefineProp(u32, u32),
    /// obj key value -> obj
    DefineElem,
    /// obj key fn -> obj  (also sets fn's home object)
    DefineMethod(u8),
    /// obj key fn -> obj
    DefineGetter(u8),
    DefineSetter(u8),
    /// obj src -> obj  (copy own enumerable props)
    CopyDataProps,
    /// Like CopyDataProps but excludes keys listed in an array: obj src excluded -> obj
    CopyDataPropsExcluding,
    /// Define `__proto__: value` in a literal: obj value -> obj
    SetProtoLiteral,
    GetSuperProp(u32),
    SetSuperProp(u32),
    GetSuperElem,
    SetSuperElem,

    // --- calls
    /// func this args.. -> result; second operand: constant describing the
    /// callee for "is not a function" errors (`NO_DESC` when none).
    Call(u16, u32),
    /// func this argsArray -> result
    CallSpread,
    /// func args.. -> result
    New(u16),
    /// func argsArray -> result
    NewSpread,
    /// args.. -> result (derived constructor `super(...)`)
    SuperCall(u16),
    SuperCallSpread,
    /// Direct eval: func this args.. with special handling when func is the intrinsic eval
    CallEval(u16, u32),
    Return,
    Throw,
    /// Throw a ReferenceError/TypeError with a constant message: (kind, msg const)
    ThrowConst(u8, u32),

    // --- control flow
    Jump(u32),
    JumpIfFalse(u32),
    JumpIfTrue(u32),
    /// Jump if top is null/undefined (pops) 
    JumpIfNullish(u32),
    /// Jump if top is not null/undefined (keeps value), else pops
    JumpIfNotNullishKeep(u32),
    /// Keep value and jump when falsy (for `&&`/`||`): peeks
    JumpIfFalseKeep(u32),
    JumpIfTrueKeep(u32),
    /// Jump if top is undefined (pops)
    JumpIfUndefined(u32),
    /// Jump if top is not undefined (pops)
    JumpIfNotUndefined(u32),
    /// Jump if the two top values are strictly equal (pops both)
    JumpIfStrictEq(u32),
    // --- operators
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Exp,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    UShr,
    Eq,
    NotEq,
    StrictEq,
    StrictNotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    In,
    Instanceof,
    /// `with` lookup: pop an object, push whether the constant name is a
    /// property of it that `Symbol.unscopables` does not hide.
    WithHas(u32),
    /// Pop a value, push `ToObject(value)` (TypeError for null/undefined).
    ToObject,
    Neg,
    Plus,
    Not,
    BitNot,
    Typeof,
    Void,
    Inc,
    Dec,
    ToNumeric,
    ToString,
    ToPropertyKey,
    /// `#name in obj`: obj -> bool, with the private symbol in a local (depth, slot)
    PrivateIn(u16, u16),

    // --- literals
    NewObject,
    /// Create array from the top n values
    NewArray(u32),
    /// arr value -> arr
    ArrayPush,
    /// arr -> arr (push a hole)
    ArrayHole,
    /// arr iterable -> arr
    ArraySpread,
    /// Create regexp: (pattern const, flags const)
    NewRegExp(u32, u32),
    /// Get (cached) template strings array for site idx
    TemplateObject(u32),
    MakeClosure(u32),
    /// superclass(or Empty) -> ctor proto   ; operand: function index of constructor
    MakeClass(u32),
    /// ctor proto fn -> ctor proto ; attach instance field initializer fn
    SetClassFields,
    /// Create a new private-name symbol with the given description: -> symbol
    NewPrivateName(u32),
    /// Run instance field initializers for the current function's class on `this`
    InitFields,

    // --- this / function meta
    This,
    /// `this` in derived constructors (throws if uninitialized)
    ThisChecked,
    NewTarget,
    /// Push the `arguments` object (created lazily per frame)
    Arguments,
    /// `arguments.length` without an arguments object.
    ArgsLength,
    /// key -> `arguments[key]` without an arguments object.
    ArgsGet,
    /// Push the current function object
    Callee,
    ImportMeta,
    DynamicImport,

    // --- iteration
    /// iterable -> iterator nextMethod
    GetIterator,
    /// iterable -> asyncIterator nextMethod
    GetAsyncIterator,
    /// iterator next -> iterator next result_object
    IteratorNextResult,
    /// iterator next -> iterator next value done(bool)
    IteratorNext,
    /// iterator next -> (calls return() if present) 
    IteratorClose,
    /// obj -> keyIterator
    ForInStart,
    /// keyIterator -> keyIterator key done
    ForInNext,
    /// For destructuring: iterator next -> iterator next value   (undefined when done; marks done in next slot as Empty)
    IteratorStep,
    /// iterator next -> iterator next restArray
    IteratorRest,
    /// Close iterator only if not done: iterator next ->
    IteratorCloseIfNotDone,

    // --- generators / async
    /// value -> received
    Yield,
    /// iterator next received -> iterator next result  (used by `yield*`)
    IteratorNextWith,
    /// value -> awaited
    Await,
    /// Async generator yield (awaits value first)
    AsyncYield,


    // --- register forms, produced by `regs::translate`. Operands are
    // absolute frame slots: the function's registers come first, then the
    // operand stack (the value at stack depth `d` lives in slot
    // `nregs + d`). Constants are `u16`/`u32` constant-table indices.
    /// `d = s` with GetReg semantics (an uninitialized slot reads as undefined).
    Mov(u16, u16),
    /// `d = s` copying the raw value.
    MovRaw(u16, u16),
    /// `d = constant`
    LoadK(u16, u32),
    /// `d = s`, throwing the TDZ ReferenceError when register `s` is uninitialized.
    MovChecked(u16, u16),
    /// Throw the TDZ ReferenceError when register `r` is uninitialized.
    CheckInit(u16),
    /// `d = a OP b` (operands: d, a, b)
    AddRR(u16, u16, u16),
    SubRR(u16, u16, u16),
    MulRR(u16, u16, u16),
    DivRR(u16, u16, u16),
    ModRR(u16, u16, u16),
    /// `d = a OP const`
    AddRK(u16, u16, u16),
    SubRK(u16, u16, u16),
    MulRK(u16, u16, u16),
    DivRK(u16, u16, u16),
    ModRK(u16, u16, u16),
    /// Other binary operators by `BIN_OPS` index: (op, d, a, b) / (op, d, a, const)
    BinRR(u8, u16, u16, u16),
    BinRK(u8, u16, u16, u16),
    /// Comparisons by `CMP_OPS` index, boolean result: (op, d, a, b) / (op, d, a, const)
    CmpRR(u8, u16, u16, u16),
    CmpRK(u8, u16, u16, u16),
    /// Fused compare-and-branch: jump to the target unless `a OP b`.
    JumpUnlessLtRR(u16, u16, u32),
    JumpUnlessLtEqRR(u16, u16, u32),
    JumpUnlessGtRR(u16, u16, u32),
    JumpUnlessGtEqRR(u16, u16, u32),
    JumpUnlessLtRK(u16, u16, u32),
    JumpUnlessLtEqRK(u16, u16, u32),
    JumpUnlessGtRK(u16, u16, u32),
    JumpUnlessGtEqRK(u16, u16, u32),
    /// Loop latch `r++; if (r < b) goto t` (see `regs::fuse_loop_latches`):
    /// replaces an `IncR(r, r)` followed by `JumpIfLtRR(r, b, t)`, which
    /// stays in place as the loop's entry test.
    IncJumpIfLtRR(u16, u16, u32),
    /// `r++; if (r < k) goto t` (with `JumpIfLtRK`).
    IncJumpIfLtRK(u16, u16, u32),
    /// Fused compare-and-branch: jump to the target when `a OP b`.
    JumpIfLtRR(u16, u16, u32),
    JumpIfLtEqRR(u16, u16, u32),
    JumpIfGtRR(u16, u16, u32),
    JumpIfGtEqRR(u16, u16, u32),
    JumpIfLtRK(u16, u16, u32),
    JumpIfLtEqRK(u16, u16, u32),
    JumpIfGtRK(u16, u16, u32),
    JumpIfGtEqRK(u16, u16, u32),
    /// Jump when `a === b` / `a !== b`.
    JumpIfStrictEqRR(u16, u16, u32),
    JumpIfStrictNotEqRR(u16, u16, u32),
    JumpIfStrictEqRK(u16, u16, u32),
    JumpIfStrictNotEqRK(u16, u16, u32),
    /// Conditional jumps on a slot (the slot is not consumed).
    JumpIfTrueR(u16, u32),
    JumpIfFalseR(u16, u32),
    JumpIfNullishR(u16, u32),
    JumpIfNotNullishR(u16, u32),
    JumpIfUndefinedR(u16, u32),
    JumpIfNotUndefinedR(u16, u32),
    /// Unary operators: (d, s)
    NotR(u16, u16),
    IncR(u16, u16),
    DecR(u16, u16),
    ToNumericR(u16, u16),
    /// Other unary operators by `UN_OPS` index: (op, d, s)
    UnR(u8, u16, u16),
    /// `d = this`
    LoadThis(u16),
    /// (d, obj, inline cache); the key is `FunctionCode::ic_keys[ic]`.
    GetPropR(u16, u16, u32),
    /// (obj, value, inline cache)
    SetPropR(u16, u16, u32),
    /// Object literal data property: (obj, value, inline cache)
    DefinePropR(u16, u16, u32),
    /// (d, obj, key)
    GetElemR(u16, u16, u16),
    /// (obj, key, value)
    SetElemR(u16, u16, u16),
    /// (d, inline cache) / (value, inline cache)
    GetGlobalR(u16, u32),
    SetGlobalR(u16, u32),
    /// (d, scope depth, slot) / (value, scope depth, slot)
    GetLocalR(u16, u16, u16),
    SetLocalR(u16, u16, u16),
    /// Call with the callee at slot `base`, `this` at `base + 1` and the
    /// arguments after it; the result goes to `base`. (base, argc, call
    /// site cache index or [`NO_CALL_IC`], desc)
    CallR(u16, u16, u16, u32),
    /// Construct with the callee at `base` and the arguments after it. (base, argc)
    NewR(u16, u16),
    ReturnR(u16),
    StoreCompletionR(u16),

    // --- misc
    /// Script completion value tracking: value ->
    StoreCompletion,
    /// -> completion value
    LoadCompletion,
    Debugger,
    Nop,
}

/// Inline cache record of one property or global access site (see `ic.rs`).
/// `repr(C)`: native code reads it (field offsets in `jit::emit`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[repr(C)]
pub struct PropIc {
    pub kind: u8,
    pub shape: u32,
    pub slot: u32,
    pub holder: u32,
    pub holder_shape: u32,
    pub mid: u32,
    pub mid_shape: u32,
}

/// Binary operators of `BinRR` / `BinRK`, by index.
pub const BIN_OPS: [Op; 12] = [Op::Add, Op::Sub, Op::Mul, Op::Div, Op::Mod, Op::BitAnd, Op::BitOr, Op::BitXor, Op::Shl, Op::Shr, Op::UShr, Op::Exp];

/// Comparison operators of `CmpRR` / `CmpRK`, by index.
pub const CMP_OPS: [Op; 8] = [Op::Lt, Op::LtEq, Op::Gt, Op::GtEq, Op::StrictEq, Op::StrictNotEq, Op::Eq, Op::NotEq];

/// Unary operators of `UnR`, by index.
pub const UN_OPS: [Op; 6] = [Op::Neg, Op::Plus, Op::BitNot, Op::Typeof, Op::ToString, Op::ToPropertyKey];


/// `Call` without a callee description.
pub const NO_DESC: u32 = u32::MAX;

/// `CallR` without a call site cache (more sites than indices).
pub const NO_CALL_IC: u16 = u16::MAX;

/// Call site cache: the closure last called from a `CallR` site and what
/// its frame needs, valid while that closure cannot have been freed (no
/// collection since: object ids are only reused after a sweep).
#[derive(Debug, Clone, Copy)]
pub struct CallIc {
    /// Value bits of the callee (0: empty).
    pub callee: u64,
    /// `Heap::collections` when filled.
    pub epoch: u32,
    /// The closure's scope (`u32::MAX`: none).
    pub scope: u32,
    /// The closure's compiled code (owned by the closure).
    pub code: *const FunctionCode,
    /// Strict mode code (no `this` coercion).
    pub strict: bool,
}

impl Default for CallIc {
    fn default() -> Self {
        CallIc { callee: 0, epoch: 0, scope: u32::MAX, code: std::ptr::null(), strict: false }
    }
}

/// Location of a parameter-like binding: frame register or function scope slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarLoc {
    Reg(u16),
    Slot(u16),
}

/// Exception handler table entry: protects `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Handler {
    pub start: u32,
    pub end: u32,
    pub target: u32,
    /// Operand stack depth (above the registers) to restore before pushing the exception.
    pub stack_depth: u32,
    /// Scope depth (number of PushScope above function scope) to restore.
    pub scope_depth: u32,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct CodeFlags {
    pub is_arrow: bool,
    pub is_async: bool,
    pub is_generator: bool,
    pub is_strict: bool,
    pub uses_arguments: bool,
    pub uses_this: bool,
    pub is_class_constructor: bool,
    pub is_derived: bool,
    pub is_method: bool,
    pub has_simple_params: bool,
    /// Script top level (vars become globals)
    pub is_script: bool,
    pub is_field_init: bool,
    /// `arguments.length` / `arguments[i]` reads are compiled to
    /// `ArgsLength` / `ArgsGet`: the frame keeps the actual arguments.
    pub args_direct: bool,
}

/// Compiled function.
#[derive(Debug)]
pub struct FunctionCode {
    pub name: JsStr,
    pub ops: Vec<Op>,
    /// Source position of each op (parallel to `ops`).
    pub positions: Vec<Pos>,
    pub constants: Vec<Value>,
    pub functions: Vec<Rc<FunctionCode>>,
    pub handlers: Vec<Handler>,
    /// Number of formal parameters (for `length`).
    pub param_count: u32,
    /// Slots in the function's own scope object (0: no scope object is
    /// created and the frame runs in the closure's scope).
    pub scope_slots: u32,
    /// Frame registers (non-captured locals and temporaries).
    pub nregs: u32,
    /// Name of each register (TDZ error messages).
    pub reg_names: Vec<JsStr>,
    /// Location of each positional parameter, in order.
    pub param_locs: Vec<VarLoc>,
    /// Binding receiving the rest parameter array, if any.
    pub rest_loc: Option<VarLoc>,
    /// Binding holding the `arguments` object, if used.
    pub arguments_loc: Option<VarLoc>,
    /// Binding holding a named function expression's self binding.
    pub self_loc: Option<VarLoc>,
    pub flags: CodeFlags,
    /// What the call fast paths need to know, decided once when the
    /// function is compiled (`CALL_*` bits; 0 for lazy code).
    pub call_flags: u8,
    /// Names of each slot (debugging).
    pub slot_names: Vec<JsStr>,
    /// Slot names of each block scope pushed by `PushScope`.
    pub block_names: Vec<Rc<Vec<JsStr>>>,
    /// Template literal sites: (cooked strings, raw strings)
    pub templates: Vec<(Vec<Option<JsStr>>, Vec<JsStr>)>,
    /// Source text for `Function.prototype.toString`.
    pub source: Option<Rc<str>>,
    /// Byte range of this function's text in `source`.
    pub source_span: Option<(u32, u32)>,
    pub filename: Rc<str>,
    /// Module this code belongs to (`None` for classic scripts / eval).
    pub module: Option<u32>,
    /// Inline caches of the property / global instructions.
    pub ics: Box<[std::cell::Cell<PropIc>]>,
    /// Call site caches of the `CallR` instructions.
    pub call_ics: Box<[std::cell::Cell<CallIc>]>,
    /// Key constant of each inline cache (register-form instructions
    /// carry only the cache index).
    pub ic_keys: Box<[u32]>,
    /// Operand stack depth before each instruction: stack instructions
    /// find their operands at slot `nregs + depth - 1` downwards.
    pub depths: Vec<u16>,
    /// Maximum operand stack depth (frames have `nregs + max_depth` slots).
    pub max_depth: u32,
    /// Set for a function not compiled yet (see `compiler::compile_lazy`);
    /// only the name, flags, parameter count and source are meaningful.
    pub lazy: Option<Box<crate::compiler::LazyCode>>,
    /// The compiled code of a lazy function, once compiled.
    pub compiled: std::cell::OnceCell<Rc<FunctionCode>>,
    /// Baseline compiler state (hotness counter, native code).
    pub jit: JitSlot,
}

/// Per-function state of the baseline compiler (empty without the `jit`
/// feature).
#[derive(Default)]
pub struct JitSlot {
    #[cfg(feature = "jit")]
    pub(crate) s: crate::jit::Slot,
}

impl std::fmt::Debug for JitSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JitSlot")
    }
}

/// `call_flags`: a plain function — not a generator, async function or
/// class constructor, no `arguments` object, no rest parameter — whose
/// frame can be built straight from the caller's slots.
pub const CALL_PLAIN: u8 = 1;
/// `call_flags`: positional parameter `i` lives in register `i` (one copy
/// of the arguments sets them all).
pub const CALL_PARAMS_IN_REGS: u8 = 2;
/// `call_flags`: `new` can build the frame directly (plain, and not an
/// arrow, method or derived constructor).
pub const CALL_PLAIN_CONSTRUCT: u8 = 4;
/// `call_flags`: the function keeps its actual arguments below its frame
/// (`args_direct`).
pub const CALL_ARGS_DIRECT: u8 = 8;
/// `call_flags`: strict mode (no `this` coercion).
pub const CALL_STRICT: u8 = 16;
/// `call_flags`: an arrow function (lexical `this` / `new.target`).
pub const CALL_ARROW: u8 = 32;
/// `call_flags`: plain, parameters in registers, no scope object, no self
/// binding and no kept arguments: the frame is just registers.
pub const CALL_SIMPLE: u8 = 64;

impl FunctionCode {
    /// Compute [`FunctionCode::call_flags`] from the compiled code.
    pub fn compute_call_flags(&self) -> u8 {
        if self.lazy.is_some() {
            return 0;
        }
        let f = &self.flags;
        let mut out = 0;
        let plain = !f.is_generator && !f.is_async && !f.is_class_constructor && self.arguments_loc.is_none() && self.rest_loc.is_none();
        if plain {
            out |= CALL_PLAIN;
            if !f.is_arrow && !f.is_method && !f.is_derived {
                out |= CALL_PLAIN_CONSTRUCT;
            }
        }
        if self.param_locs.iter().enumerate().all(|(i, l)| *l == VarLoc::Reg(i as u16)) && (self.param_locs.len() as u32) <= self.nregs {
            out |= CALL_PARAMS_IN_REGS;
        }
        if f.args_direct {
            out |= CALL_ARGS_DIRECT;
        }
        if f.is_strict {
            out |= CALL_STRICT;
        }
        if f.is_arrow {
            out |= CALL_ARROW;
        }
        if out & (CALL_PLAIN | CALL_PARAMS_IN_REGS | CALL_ARGS_DIRECT) == CALL_PLAIN | CALL_PARAMS_IN_REGS && self.scope_slots == 0 && self.self_loc.is_none() {
            out |= CALL_SIMPLE;
        }
        out
    }

    /// Number of value slots of a frame running this code.
    #[inline]
    pub fn frame_size(&self) -> usize {
        self.nregs as usize + self.max_depth as usize
    }

    pub fn position_at(&self, pc: usize) -> Pos {
        self.positions.get(pc.min(self.positions.len().saturating_sub(1))).copied().unwrap_or_default()
    }
}

impl Op {
    /// Branch target of a jump instruction.
    pub fn jump_target(&self) -> Option<u32> {
        match *self {
            Op::Jump(t)
            | Op::JumpIfFalse(t)
            | Op::JumpIfTrue(t)
            | Op::JumpIfNullish(t)
            | Op::JumpIfNotNullishKeep(t)
            | Op::JumpIfFalseKeep(t)
            | Op::JumpIfTrueKeep(t)
            | Op::JumpIfUndefined(t)
            | Op::JumpIfNotUndefined(t)
            | Op::JumpIfStrictEq(t)
            | Op::JumpUnlessLtRR(_, _, t)
            | Op::JumpUnlessLtEqRR(_, _, t)
            | Op::JumpUnlessGtRR(_, _, t)
            | Op::JumpUnlessGtEqRR(_, _, t)
            | Op::JumpUnlessLtRK(_, _, t)
            | Op::JumpUnlessLtEqRK(_, _, t)
            | Op::JumpUnlessGtRK(_, _, t)
            | Op::JumpUnlessGtEqRK(_, _, t)
            | Op::IncJumpIfLtRR(_, _, t)
            | Op::IncJumpIfLtRK(_, _, t)
            | Op::JumpIfLtRR(_, _, t)
            | Op::JumpIfLtEqRR(_, _, t)
            | Op::JumpIfGtRR(_, _, t)
            | Op::JumpIfGtEqRR(_, _, t)
            | Op::JumpIfLtRK(_, _, t)
            | Op::JumpIfLtEqRK(_, _, t)
            | Op::JumpIfGtRK(_, _, t)
            | Op::JumpIfGtEqRK(_, _, t)
            | Op::JumpIfStrictEqRR(_, _, t)
            | Op::JumpIfStrictNotEqRR(_, _, t)
            | Op::JumpIfStrictEqRK(_, _, t)
            | Op::JumpIfStrictNotEqRK(_, _, t)
            | Op::JumpIfTrueR(_, t)
            | Op::JumpIfFalseR(_, t)
            | Op::JumpIfNullishR(_, t)
            | Op::JumpIfNotNullishR(_, t)
            | Op::JumpIfUndefinedR(_, t)
            | Op::JumpIfNotUndefinedR(_, t) => Some(t),
            _ => None,
        }
    }

    /// The same jump with another target.
    pub fn with_target(self, t: u32) -> Op {
        match self {
            Op::Jump(_) => Op::Jump(t),
            Op::JumpIfFalse(_) => Op::JumpIfFalse(t),
            Op::JumpIfTrue(_) => Op::JumpIfTrue(t),
            Op::JumpIfNullish(_) => Op::JumpIfNullish(t),
            Op::JumpIfNotNullishKeep(_) => Op::JumpIfNotNullishKeep(t),
            Op::JumpIfFalseKeep(_) => Op::JumpIfFalseKeep(t),
            Op::JumpIfTrueKeep(_) => Op::JumpIfTrueKeep(t),
            Op::JumpIfUndefined(_) => Op::JumpIfUndefined(t),
            Op::JumpIfNotUndefined(_) => Op::JumpIfNotUndefined(t),
            Op::JumpIfStrictEq(_) => Op::JumpIfStrictEq(t),
            Op::JumpUnlessLtRR(a, b, _) => Op::JumpUnlessLtRR(a, b, t),
            Op::JumpUnlessLtEqRR(a, b, _) => Op::JumpUnlessLtEqRR(a, b, t),
            Op::JumpUnlessGtRR(a, b, _) => Op::JumpUnlessGtRR(a, b, t),
            Op::JumpUnlessGtEqRR(a, b, _) => Op::JumpUnlessGtEqRR(a, b, t),
            Op::JumpUnlessLtRK(a, b, _) => Op::JumpUnlessLtRK(a, b, t),
            Op::JumpUnlessLtEqRK(a, b, _) => Op::JumpUnlessLtEqRK(a, b, t),
            Op::JumpUnlessGtRK(a, b, _) => Op::JumpUnlessGtRK(a, b, t),
            Op::JumpUnlessGtEqRK(a, b, _) => Op::JumpUnlessGtEqRK(a, b, t),
            Op::IncJumpIfLtRR(a, b, _) => Op::IncJumpIfLtRR(a, b, t),
            Op::IncJumpIfLtRK(a, b, _) => Op::IncJumpIfLtRK(a, b, t),
            Op::JumpIfLtRR(a, b, _) => Op::JumpIfLtRR(a, b, t),
            Op::JumpIfLtEqRR(a, b, _) => Op::JumpIfLtEqRR(a, b, t),
            Op::JumpIfGtRR(a, b, _) => Op::JumpIfGtRR(a, b, t),
            Op::JumpIfGtEqRR(a, b, _) => Op::JumpIfGtEqRR(a, b, t),
            Op::JumpIfLtRK(a, b, _) => Op::JumpIfLtRK(a, b, t),
            Op::JumpIfLtEqRK(a, b, _) => Op::JumpIfLtEqRK(a, b, t),
            Op::JumpIfGtRK(a, b, _) => Op::JumpIfGtRK(a, b, t),
            Op::JumpIfGtEqRK(a, b, _) => Op::JumpIfGtEqRK(a, b, t),
            Op::JumpIfStrictEqRR(a, b, _) => Op::JumpIfStrictEqRR(a, b, t),
            Op::JumpIfStrictNotEqRR(a, b, _) => Op::JumpIfStrictNotEqRR(a, b, t),
            Op::JumpIfStrictEqRK(a, b, _) => Op::JumpIfStrictEqRK(a, b, t),
            Op::JumpIfStrictNotEqRK(a, b, _) => Op::JumpIfStrictNotEqRK(a, b, t),
            Op::JumpIfTrueR(s, _) => Op::JumpIfTrueR(s, t),
            Op::JumpIfFalseR(s, _) => Op::JumpIfFalseR(s, t),
            Op::JumpIfNullishR(s, _) => Op::JumpIfNullishR(s, t),
            Op::JumpIfNotNullishR(s, _) => Op::JumpIfNotNullishR(s, t),
            Op::JumpIfUndefinedR(s, _) => Op::JumpIfUndefinedR(s, t),
            Op::JumpIfNotUndefinedR(s, _) => Op::JumpIfNotUndefinedR(s, t),
            other => other,
        }
    }
}


impl Op {
    /// Operand stack effect `(pops, pushes)` of a stack instruction (for a
    /// conditional jump: on the fall-through edge; see `taken_effect`).
    pub fn stack_effect(&self) -> (u32, u32) {
        use Op::*;
        match *self {
            Const(_) | Undefined | Null | True | False | Empty => (0, 1),
            Pop => (1, 0),
            Dup => (1, 2),
            Dup2 => (2, 4),
            Swap => (2, 2),
            Rot3 => (3, 3),
            Rot4 => (4, 4),
            GetReg(_) | GetRegChecked(_) => (0, 1),
            SetReg(_) | SetRegChecked(_) => (1, 0),
            GetLocal(..) | GetLocalChecked(..) | GetImport(..) => (0, 1),
            SetLocal(..) | InitLocal(..) | SetLocalChecked(..) => (1, 0),
            GetGlobal(..) | TypeofGlobal(_) => (0, 1),
            SetGlobal(..) | DeclareGlobalFunction(_) | InitGlobalLex(_) => (1, 0),
            DeclareGlobalVar(_) | DeclareGlobalLex(..) => (0, 0),
            PushScope(..) | PopScope | CopyScope => (0, 0),
            GetProp(..) => (1, 1),
            SetProp(..) => (2, 1),
            GetElem => (2, 1),
            SetElem => (3, 1),
            GetElemKeep => (2, 3),
            GetPropKeep(..) | GetMethod(..) => (1, 2),
            DeleteProp(_) => (1, 1),
            DeleteElem => (2, 1),
            DefineProp(..) => (2, 1),
            DefineElem | DefineMethod(_) | DefineGetter(_) | DefineSetter(_) => (3, 1),
            CopyDataProps => (2, 1),
            CopyDataPropsExcluding => (3, 1),
            SetProtoLiteral => (2, 1),
            GetSuperProp(_) => (0, 1),
            SetSuperProp(_) | GetSuperElem => (1, 1),
            SetSuperElem => (2, 1),
            Call(argc, _) | CallEval(argc, _) => (argc as u32 + 2, 1),
            CallSpread => (3, 1),
            New(argc) => (argc as u32 + 1, 1),
            NewSpread => (2, 1),
            SuperCall(argc) => (argc as u32, 1),
            SuperCallSpread => (1, 1),
            Return | Throw => (1, 0),
            ThrowConst(..) => (0, 0),
            Jump(_) => (0, 0),
            JumpIfFalse(_) | JumpIfTrue(_) | JumpIfNullish(_) | JumpIfUndefined(_) | JumpIfNotUndefined(_) => (1, 0),
            JumpIfNotNullishKeep(_) => (1, 0),
            JumpIfFalseKeep(_) | JumpIfTrueKeep(_) => (1, 1),
            JumpIfStrictEq(_) => (2, 0),
            Add | Sub | Mul | Div | Mod | Exp | BitAnd | BitOr | BitXor | Shl | Shr | UShr | Eq | NotEq | StrictEq | StrictNotEq | Lt | LtEq | Gt | GtEq | In | Instanceof => (2, 1),
            Neg | Plus | Not | BitNot | Typeof | Void | Inc | Dec | ToNumeric | ToString | ToPropertyKey | PrivateIn(..) => (1, 1),
            WithHas(_) | ToObject => (1, 1),
            NewObject => (0, 1),
            NewArray(n) => (n, 1),
            ArrayPush | ArraySpread => (2, 1),
            ArrayHole => (1, 1),
            NewRegExp(..) | TemplateObject(_) | MakeClosure(_) | NewPrivateName(_) => (0, 1),
            MakeClass(_) => (1, 2),
            SetClassFields => (3, 2),
            InitFields => (0, 0),
            This | ThisChecked | NewTarget | Arguments | ArgsLength | Callee | ImportMeta => (0, 1),
            ArgsGet | DynamicImport => (1, 1),
            GetIterator | GetAsyncIterator => (1, 2),
            IteratorNextResult => (2, 1),
            IteratorNext => (2, 2),
            IteratorClose | IteratorCloseIfNotDone => (2, 0),
            ForInStart => (1, 1),
            ForInNext => (1, 2),
            IteratorStep | IteratorRest => (2, 3),
            Yield | AsyncYield | Await => (1, 1),
            IteratorNextWith => (3, 3),
            StoreCompletion => (1, 0),
            LoadCompletion => (0, 1),
            Debugger | Nop => (0, 0),
            // Register forms never appear in the stack code that the
            // depth analysis runs on.
            _ => (0, 0),
        }
    }

    /// Stack effect on the taken edge of a conditional jump.
    pub fn taken_effect(&self) -> (u32, u32) {
        match *self {
            Op::JumpIfNotNullishKeep(_) => (1, 1),
            other => other.stack_effect(),
        }
    }

    /// No fall-through to the next instruction.
    pub fn is_terminator(&self) -> bool {
        matches!(self, Op::Jump(_) | Op::Return | Op::Throw | Op::ThrowConst(..))
    }
}

/// Operand stack depth before each instruction (above the registers), by
/// a forward data-flow pass over the control-flow graph; `None` marks
/// unreachable code. Fails when two paths reach an instruction with
/// different depths (a compiler bug).
pub fn compute_depths(ops: &[Op], handlers: &[Handler]) -> Result<Vec<Option<u32>>, String> {
    let n = ops.len();
    let mut depth: Vec<Option<u32>> = vec![None; n + 1];
    let mut work: Vec<usize> = Vec::new();
    let set = |depth: &mut Vec<Option<u32>>, work: &mut Vec<usize>, at: usize, d: u32| -> Result<(), String> {
        if at > n {
            return Err(format!("jump past end ({at})"));
        }
        match depth[at] {
            Some(x) if x == d => Ok(()),
            Some(x) => Err(format!("stack depth mismatch at {at}: {x} vs {d}")),
            None => {
                depth[at] = Some(d);
                work.push(at);
                Ok(())
            }
        }
    };
    if n > 0 {
        set(&mut depth, &mut work, 0, 0)?;
    }
    for h in handlers {
        set(&mut depth, &mut work, h.target as usize, h.stack_depth + 1)?;
    }
    while let Some(i) = work.pop() {
        if i >= n {
            continue;
        }
        let d = depth[i].unwrap();
        let op = ops[i];
        if let Some(t) = op.jump_target() {
            let (p, q) = op.taken_effect();
            if d < p {
                return Err(format!("stack underflow at {i} ({op:?})"));
            }
            set(&mut depth, &mut work, t as usize, d - p + q)?;
        }
        if !op.is_terminator() {
            let (p, q) = op.stack_effect();
            if d < p {
                return Err(format!("stack underflow at {i} ({op:?})"));
            }
            set(&mut depth, &mut work, i + 1, d - p + q)?;
        }
    }
    Ok(depth)
}
