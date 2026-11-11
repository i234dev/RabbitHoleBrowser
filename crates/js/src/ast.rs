//! Abstract syntax tree for ECMAScript 2022 scripts and modules.
//!
//! The tree is deliberately plain data (no interior mutability, no Rc) so
//! the parser, the compiler and tests can all share it freely. Positions
//! are 1-based line/column of the first token of each node.


/// An identifier (or other name) from the source. Cheap to clone: equal
/// names produced by one lexer share their text.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Name(std::rc::Rc<str>);

impl Default for Name {
    fn default() -> Name {
        Name::new("")
    }
}

impl Name {
    pub fn new(s: &str) -> Name {
        Name(std::rc::Rc::from(s))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for Name {
    type Target = str;
    #[inline]
    fn deref(&self) -> &str {
        &self.0
    }
}

impl std::borrow::Borrow<str> for Name {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Name {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl PartialEq<str> for Name {
    fn eq(&self, other: &str) -> bool {
        &*self.0 == other
    }
}

impl PartialEq<&str> for Name {
    fn eq(&self, other: &&str) -> bool {
        &*self.0 == *other
    }
}

impl PartialEq<String> for Name {
    fn eq(&self, other: &String) -> bool {
        &*self.0 == other.as_str()
    }
}

impl PartialEq<Name> for str {
    fn eq(&self, other: &Name) -> bool {
        self == &*other.0
    }
}

impl PartialEq<Name> for &str {
    fn eq(&self, other: &Name) -> bool {
        *self == &*other.0
    }
}

impl PartialEq<Name> for String {
    fn eq(&self, other: &Name) -> bool {
        self.as_str() == &*other.0
    }
}

impl From<&str> for Name {
    fn from(s: &str) -> Name {
        Name::new(s)
    }
}

impl From<String> for Name {
    fn from(s: String) -> Name {
        Name(std::rc::Rc::from(s))
    }
}

impl From<&String> for Name {
    fn from(s: &String) -> Name {
        Name::new(s)
    }
}

impl std::fmt::Debug for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&*self.0, f)
    }
}

impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pos {
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub body: Vec<Stmt>,
    pub is_module: bool,
    /// `"use strict"` directive at the top.
    pub strict: bool,
}

// ------------------------------------------------------------------ statements

#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub pos: Pos,
    pub kind: StmtKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    Empty,
    Expr(Expr),
    VarDecl(VarDecl),
    FunctionDecl(std::rc::Rc<Function>),
    ClassDecl(Class),
    Block(Vec<Stmt>),
    // Large parts of statements are boxed: statements are moved around a
    // lot while parsing, so `Stmt` is kept small.
    If {
        test: Box<Expr>,
        consequent: Box<Stmt>,
        alternate: Option<Box<Stmt>>,
    },
    For {
        init: Option<Box<ForInit>>,
        test: Option<Box<Expr>>,
        update: Option<Box<Expr>>,
        body: Box<Stmt>,
    },
    ForIn {
        left: Box<ForHead>,
        right: Box<Expr>,
        body: Box<Stmt>,
    },
    ForOf {
        left: Box<ForHead>,
        right: Box<Expr>,
        body: Box<Stmt>,
        is_await: bool,
    },
    While {
        test: Box<Expr>,
        body: Box<Stmt>,
    },
    DoWhile {
        body: Box<Stmt>,
        test: Box<Expr>,
    },
    Return(Option<Expr>),
    Break(Option<Name>),
    Continue(Option<Name>),
    Throw(Expr),
    Try {
        block: Vec<Stmt>,
        /// (binding pattern, handler body); the pattern is `None` for `catch {}`.
        handler: Option<Box<(Option<Pattern>, Vec<Stmt>)>>,
        finalizer: Option<Vec<Stmt>>,
    },
    Switch {
        discriminant: Box<Expr>,
        cases: Vec<SwitchCase>,
    },
    Labeled {
        label: Name,
        body: Box<Stmt>,
    },
    Debugger,
    /// `with (obj) stmt` — parsed for completeness; the compiler may reject it.
    With {
        object: Box<Expr>,
        body: Box<Stmt>,
    },
    Import(ImportDecl),
    Export(ExportDecl),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SwitchCase {
    /// `None` for `default:`
    pub test: Option<Expr>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarKind {
    Var,
    Let,
    Const,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VarDecl {
    pub kind: VarKind,
    pub declarations: Vec<VarDeclarator>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VarDeclarator {
    pub target: Pattern,
    pub init: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ForInit {
    VarDecl(VarDecl),
    Expr(Expr),
}

/// Left-hand side of `for (... in/of ...)`.
#[derive(Debug, Clone, PartialEq)]
pub enum ForHead {
    /// `for (let x of ...)` / `for (var [a, b] in ...)`
    VarDecl { kind: VarKind, target: Pattern },
    /// `for (x of ...)`, `for (obj.prop of ...)`
    Pattern(Pattern),
}

// ---------------------------------------------------------------- patterns

/// Binding / assignment targets (destructuring).
#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    Identifier(Name),
    /// Only valid as an assignment target (not in declarations): `a.b = ...`
    Expr(Box<Expr>),
    Object {
        properties: Vec<ObjectPatternProp>,
        rest: Option<Box<Pattern>>,
    },
    Array {
        /// `None` entries are elisions: `[, b]`
        elements: Vec<Option<Pattern>>,
        rest: Option<Box<Pattern>>,
    },
    /// `target = default`
    Assign {
        target: Box<Pattern>,
        default: Box<Expr>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObjectPatternProp {
    pub key: PropertyKey,
    pub value: Pattern,
    /// `{ a }` shorthand (key is an identifier, value is the same identifier).
    pub shorthand: bool,
}

// -------------------------------------------------------------- expressions

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub pos: Pos,
    pub kind: ExprKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Identifier(Name),
    Null,
    Undefined,
    This,
    Super,
    Bool(bool),
    Number(f64),
    BigInt(String),
    String(String),
    /// Quasis are the cooked strings (`None` when the cooked value is
    /// invalid, which is only allowed in tagged templates); `raws` keep the
    /// raw text for `String.raw` and tag functions.
    // (Rare, large parts are boxed to keep `Expr` small.)
    Template {
        quasis: Box<Vec<Option<String>>>,
        raws: Box<Vec<String>>,
        expressions: Vec<Expr>,
    },
    TaggedTemplate {
        tag: Box<Expr>,
        quasis: Box<Vec<Option<String>>>,
        raws: Box<Vec<String>>,
        expressions: Box<Vec<Expr>>,
    },
    Regex {
        pattern: Box<str>,
        flags: Box<str>,
    },
    Array(Vec<ArrayElement>),
    Object(Vec<ObjectProperty>),
    Function(std::rc::Rc<Function>),
    Arrow(std::rc::Rc<Function>),
    Class(Box<Class>),
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Update {
        op: UpdateOp,
        prefix: bool,
        target: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Logical {
        op: LogicalOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Assign {
        op: AssignOp,
        target: Box<Pattern>,
        value: Box<Expr>,
    },
    Conditional {
        test: Box<Expr>,
        consequent: Box<Expr>,
        alternate: Box<Expr>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<ArrayElement>,
        /// `callee?.(args)`
        optional: bool,
    },
    New {
        callee: Box<Expr>,
        args: Vec<ArrayElement>,
    },
    /// `obj.prop` / `obj[expr]` / `obj?.prop`
    Member {
        object: Box<Expr>,
        property: MemberProp,
        optional: bool,
    },
    /// Wraps an optional chain so short-circuiting stops here: `a?.b.c`.
    OptionalChain(Box<Expr>),
    /// `a, b, c`
    Sequence(Vec<Expr>),
    Spread(Box<Expr>),
    Yield {
        argument: Option<Box<Expr>>,
        delegate: bool,
    },
    Await(Box<Expr>),
    /// `new.target`
    NewTarget,
    /// `import.meta`
    ImportMeta,
    /// `import(expr)`
    DynamicImport(Box<Expr>),
    /// `#priv in obj`
    PrivateIn {
        name: Name,
        object: Box<Expr>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum MemberProp {
    Identifier(Name),
    /// `obj.#field`
    Private(Name),
    Computed(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArrayElement {
    /// Elision (`[1, , 3]`)
    Hole,
    Expr(Expr),
    Spread(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PropertyKey {
    Identifier(Name),
    String(Name),
    Number(f64),
    Computed(Box<Expr>),
    /// `#name` (class private members only)
    Private(Name),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ObjectProperty {
    /// `key: value`, `key` (shorthand), `key() {}`, `get key() {}`, ...
    Property {
        key: PropertyKey,
        value: PropertyValue,
        shorthand: bool,
    },
    /// `...expr`
    Spread(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    Init(Expr),
    Method(std::rc::Rc<Function>),
    Getter(std::rc::Rc<Function>),
    Setter(std::rc::Rc<Function>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Plus,
    Not,
    BitNot,
    Typeof,
    Void,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateOp {
    Increment,
    Decrement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Exp,
    Eq,
    NotEq,
    StrictEq,
    StrictNotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Shl,
    Shr,
    UShr,
    BitAnd,
    BitOr,
    BitXor,
    In,
    Instanceof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicalOp {
    And,
    Or,
    Nullish,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Exp,
    Shl,
    Shr,
    UShr,
    BitAnd,
    BitOr,
    BitXor,
    And,
    Or,
    Nullish,
}

// ---------------------------------------------------------------- functions

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    /// Binding name (declarations, named expressions); `None` for
    /// anonymous functions and arrows. Methods carry their key separately.
    pub name: Option<Name>,
    pub params: Vec<Param>,
    pub body: FunctionBody,
    pub is_arrow: bool,
    pub is_async: bool,
    pub is_generator: bool,
    pub strict: bool,
    pub pos: Pos,
    /// Byte range of the source text (`Function.prototype.toString`);
    /// (0, 0) when unknown.
    pub span: (u32, u32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub pattern: Pattern,
    /// `...rest`
    pub rest: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionBody {
    Block(Vec<Stmt>),
    /// Arrow function with an expression body.
    Expr(Box<Expr>),
    /// A block body skipped by the pre-parser: parsed when the function is
    /// compiled (on its first call), see [`LazyBody`].
    Lazy(std::rc::Rc<LazyBody>),
}

/// A function body the parser skipped (lazy parsing): where it is and
/// what the enclosing code needs to know about it without its tree.
#[derive(Debug, Clone, PartialEq)]
pub struct LazyBody {
    /// The whole source text the body is part of.
    pub source: std::rc::Rc<str>,
    /// Byte offset of the opening `{` and just past the closing `}`.
    pub start: usize,
    pub end: usize,
    /// Line and line-start offset at `start` (for positions).
    pub line: u32,
    pub line_start: usize,
    /// Every distinct identifier mentioned in the body (except property
    /// names after `.`), private names as `#name`: the closure capture
    /// analysis of enclosing functions is name based.
    pub names: Box<[Name]>,
    /// The body mentions `eval` (maybe a direct eval).
    pub eval: bool,
    /// Parse context of the body.
    pub is_module: bool,
    pub in_class_body: bool,
    pub super_call_ok: bool,
    pub super_prop_ok: bool,
}

// ------------------------------------------------------------------ classes

#[derive(Debug, Clone, PartialEq)]
pub struct Class {
    pub name: Option<Name>,
    pub superclass: Option<Box<Expr>>,
    pub members: Vec<ClassMember>,
    pub pos: Pos,
    /// Byte range of the source text; (0, 0) when unknown.
    pub span: (u32, u32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClassMember {
    Constructor(Function),
    Method {
        key: PropertyKey,
        value: std::rc::Rc<Function>,
        is_static: bool,
    },
    Getter {
        key: PropertyKey,
        value: std::rc::Rc<Function>,
        is_static: bool,
    },
    Setter {
        key: PropertyKey,
        value: std::rc::Rc<Function>,
        is_static: bool,
    },
    Field {
        key: PropertyKey,
        value: Option<Expr>,
        is_static: bool,
    },
    /// `static { ... }`
    StaticBlock(Vec<Stmt>),
}

// ------------------------------------------------------------------ modules

#[derive(Debug, Clone, PartialEq)]
pub struct ImportDecl {
    pub source: String,
    /// `import x from "m"`
    pub default: Option<String>,
    /// `import * as ns from "m"`
    pub namespace: Option<String>,
    /// `import { a as b } from "m"` → (imported, local)
    pub named: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExportDecl {
    /// `export default expr`
    DefaultExpr(Expr),
    /// `export default function/class`
    DefaultDecl(Box<Stmt>),
    /// `export var/let/const/function/class ...`
    Decl(Box<Stmt>),
    /// `export { a as b }` optionally `from "m"` → (local, exported)
    Named {
        specifiers: Vec<(String, String)>,
        source: Option<String>,
    },
    /// `export * from "m"` / `export * as ns from "m"`
    All {
        source: String,
        namespace: Option<String>,
    },
}
