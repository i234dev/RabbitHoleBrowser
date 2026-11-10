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
    GetGlobal(u32),
    SetGlobal(u32),
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
    GetProp(u32),
    SetProp(u32),
    GetElem,
    SetElem,
    /// obj key -> obj value  (keeps obj for compound assignment)
    GetElemKeep,
    /// obj -> obj value
    GetPropKeep(u32),
    DeleteProp(u32),
    DeleteElem,
    /// Object literal helpers: obj value -> obj
    DefineProp(u32),
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
    /// func this args.. -> result
    Call(u16),
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
    CallEval(u16),
    /// Remember a textual description of the next callee for error messages.
    CalleeDesc(u32),
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

    // --- misc
    /// Script completion value tracking: value ->
    StoreCompletion,
    /// -> completion value
    LoadCompletion,
    Debugger,
    Nop,
}

/// Exception handler table entry: protects `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Handler {
    pub start: u32,
    pub end: u32,
    pub target: u32,
    /// Operand stack depth to restore before pushing the exception.
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
    /// Slots in the function's own scope object.
    pub scope_slots: u32,
    /// Scope slot of each positional parameter, in order.
    pub param_slots: Vec<u16>,
    /// Slot receiving the rest parameter array, if any.
    pub rest_slot: Option<u16>,
    /// Slot holding the `arguments` object, if used.
    pub arguments_slot: Option<u32>,
    /// Slot holding a named function expression's self binding.
    pub self_slot: Option<u32>,
    pub flags: CodeFlags,
    /// Names of each slot (debugging).
    pub slot_names: Vec<JsStr>,
    /// Slot names of each block scope pushed by `PushScope`.
    pub block_names: Vec<Rc<Vec<JsStr>>>,
    /// Template literal sites: (cooked strings, raw strings)
    pub templates: Vec<(Vec<Option<JsStr>>, Vec<JsStr>)>,
    /// Source text for `Function.prototype.toString`.
    pub source: Option<Rc<str>>,
    pub filename: Rc<str>,
    /// Module this code belongs to (`None` for classic scripts / eval).
    pub module: Option<u32>,
}

impl FunctionCode {
    pub fn position_at(&self, pc: usize) -> Pos {
        self.positions.get(pc.min(self.positions.len().saturating_sub(1))).copied().unwrap_or_default()
    }
}
