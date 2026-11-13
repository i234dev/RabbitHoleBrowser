//! Closure capture analysis.
//!
//! The compiler keeps a function's bindings in frame registers unless some
//! nested function may refer to them, in which case they live in a heap
//! scope object shared with the closures. This pass computes, for every
//! function of a program, the set of names referenced anywhere inside the
//! functions nested in it. The analysis is name based (it ignores
//! shadowing), so it over-approximates: a binding is only kept in a
//! register when no nested function mentions its name at all.
//!
//! One walk over the program fills the sets for all functions: when an
//! identifier is seen at function nesting level `L`, its name is added to
//! the sets of the enclosing functions at levels `< L`. Since a name present
//! in an inner ancestor's set is necessarily present in all outer ones, the
//! upward walk stops at the first ancestor that already has it.

use crate::ast::*;
use crate::value::FxBuild;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Captured-name sets keyed by the address of the `Function` node (the
/// program root uses the null address).
pub type CaptureMap = HashMap<*const Function, Rc<NameSet>, FxBuild>;
pub type NameSet = HashSet<String, FxBuild>;

/// Marker in a function's own set: its `arguments` object may be written
/// through, deleted from or reached by direct `eval`, so reads like
/// `arguments[i]` / `arguments.length` cannot be served from the frame.
pub const ARGS_ESCAPE: &str = "\u{0}args";

/// Marker in a function's own set: it refers to its `arguments` (directly
/// or from nested arrows).
pub const ARGS_USED: &str = "\u{0}argsused";

/// Marker in a function's own set: its `arguments` object may be created
/// (a use other than `arguments[k]` / `arguments.length` reads in the
/// function itself, which the frame serves without an object).
pub const ARGS_OBJ: &str = "\u{0}argsobj";

/// Marker in a function's own set: an arrow function is nested in it
/// (through arrows only): its `this` / `super()` may be used from there.
pub const HAS_ARROW: &str = "\u{0}arrow";

/// Marker in a function's set: direct `eval` code runs inside it (or inside
/// a function nested in it) and may reference any of its bindings, so all
/// of them live in heap scopes, where the eval code finds them.
pub const EVAL_ALL: &str = "\u{0}eval";

/// Marker in a function's set: it contains a direct `eval` call itself
/// (not only in nested functions): sloppy eval code may add `var`s to its
/// variable environment.
pub const EVAL_HERE: &str = "\u{0}evalhere";

/// Whether `set` makes the binding `name` live in a heap scope.
#[inline]
pub fn is_captured(set: &NameSet, name: &str) -> bool {
    set.contains(name) || set.contains(EVAL_ALL)
}

fn is_arguments_member(e: &Expr) -> bool {
    matches!(&e.kind, ExprKind::Member { object, .. } if matches!(&object.kind, ExprKind::Identifier(n) if n == "arguments"))
}

pub fn analyze_program(body: &[Stmt]) -> CaptureMap {
    let mut a = Analyzer { stack: vec![std::ptr::null()], arrow: vec![false], sets: vec![NameSet::default()], out: HashMap::default() };
    for s in body {
        a.stmt(s);
    }
    let root = a.sets.pop().unwrap();
    a.out.insert(std::ptr::null(), Rc::new(root));
    a.out
}

/// Analyze a single function (used for synthesized or cloned function
/// nodes that the program-wide pass did not see).
pub fn analyze_function(f: &Function) -> CaptureMap {
    let mut a = Analyzer { stack: vec![std::ptr::null()], arrow: vec![false], sets: vec![NameSet::default()], out: HashMap::default() };
    a.function(f);
    a.out
}

struct Analyzer {
    /// Enclosing function keys, outermost first.
    stack: Vec<*const Function>,
    /// Whether each level is an arrow function.
    arrow: Vec<bool>,
    sets: Vec<NameSet>,
    out: CaptureMap,
}

impl Analyzer {
    /// The innermost non-arrow function's `arguments` escapes.
    fn args_escape(&mut self) {
        for i in (1..self.sets.len()).rev() {
            if !self.arrow[i] {
                self.sets[i].insert(ARGS_ESCAPE.to_string());
                return;
            }
        }
    }

    /// A reference to `arguments` at the current level (`obj`: one the
    /// frame cannot serve without an arguments object).
    fn args_mark(&mut self, obj: bool) {
        let last = self.sets.len() - 1;
        for i in (1..self.sets.len()).rev() {
            if !self.arrow[i] {
                self.sets[i].insert(ARGS_USED.to_string());
                if obj || i != last {
                    self.sets[i].insert(ARGS_OBJ.to_string());
                }
                return;
            }
        }
    }

    /// A direct `eval` at the current level: its code may reach every
    /// binding (and `arguments`) of every enclosing function.
    fn direct_eval(&mut self) {
        for (i, set) in self.sets.iter_mut().enumerate() {
            if i > 0 {
                set.insert(ARGS_ESCAPE.to_string());
            }
            set.insert(EVAL_ALL.to_string());
        }
        if let Some(set) = self.sets.last_mut() {
            set.insert(EVAL_HERE.to_string());
        }
    }

    /// A member expression used as a write target / delete operand.
    fn target_expr(&mut self, e: &Expr) {
        if is_arguments_member(e) {
            self.args_escape();
        }
        self.expr(e);
    }

    fn name(&mut self, n: &str) {
        // Every enclosing function except the innermost one sees `n` as
        // referenced from a nested function.
        let len = self.sets.len();
        if len < 2 {
            return;
        }
        for i in (0..len - 1).rev() {
            if self.sets[i].contains(n) {
                break;
            }
            self.sets[i].insert(n.to_string());
        }
    }

    fn function(&mut self, f: &Function) {
        if f.is_arrow {
            for i in (1..self.sets.len()).rev() {
                if !self.arrow[i] {
                    self.sets[i].insert(HAS_ARROW.to_string());
                    break;
                }
            }
        }
        self.stack.push(f as *const Function);
        self.arrow.push(f.is_arrow);
        self.sets.push(NameSet::default());
        // The function's own name is not a reference: a declaration binds it
        // in the enclosing scope (where only actual references capture it)
        // and an expression binds it inside itself.
        for p in &f.params {
            self.pattern(&p.pattern);
        }
        let mut known = true;
        match &f.body {
            FunctionBody::Block(b) => {
                for s in b {
                    self.stmt(s);
                }
            }
            FunctionBody::Expr(e) => self.expr(e),
            FunctionBody::Lazy(lb) => {
                // A skipped body: what it mentions is referenced from a
                // nested function for every enclosing one. Its own set is
                // computed when it is parsed (see `analyze_function`).
                if lb.eval {
                    self.direct_eval();
                }
                if f.is_arrow && lb.names.iter().any(|n| n.as_str() == "arguments") {
                    // It may write through the enclosing `arguments`.
                    self.args_escape();
                    self.args_mark(true);
                }
                for n in lb.names.iter() {
                    self.name(n);
                }
                known = false;
            }
        }
        let set = self.sets.pop().unwrap();
        self.arrow.pop();
        let key = self.stack.pop().unwrap();
        if known {
            self.out.insert(key, Rc::new(set));
        }
    }

    /// Code compiled into a synthetic function of its own (class field
    /// initializers, static blocks): a nested level without an AST node.
    fn synthetic<F: FnOnce(&mut Self)>(&mut self, f: F) {
        self.stack.push(std::ptr::null());
        self.arrow.push(false);
        self.sets.push(NameSet::default());
        f(self);
        self.sets.pop();
        self.arrow.pop();
        self.stack.pop();
    }

    fn stmts(&mut self, list: &[Stmt]) {
        for s in list {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Empty | StmtKind::Debugger | StmtKind::Break(_) | StmtKind::Continue(_) => {}
            StmtKind::Expr(e) | StmtKind::Throw(e) => self.expr(e),
            StmtKind::VarDecl(d) => self.var_decl(d),
            StmtKind::FunctionDecl(f) => self.function(f),
            StmtKind::ClassDecl(c) => self.class(c),
            StmtKind::Block(b) => self.stmts(b),
            StmtKind::If { test, consequent, alternate } => {
                self.expr(test);
                self.stmt(consequent);
                if let Some(a) = alternate {
                    self.stmt(a);
                }
            }
            StmtKind::For { init, test, update, body } => {
                match init.as_deref() {
                    Some(ForInit::VarDecl(d)) => self.var_decl(d),
                    Some(ForInit::Expr(e)) => self.expr(e),
                    None => {}
                }
                if let Some(t) = test {
                    self.expr(t);
                }
                if let Some(u) = update {
                    self.expr(u);
                }
                self.stmt(body);
            }
            StmtKind::ForIn { left, right, body } | StmtKind::ForOf { left, right, body, .. } => {
                match &**left {
                    ForHead::VarDecl { target, .. } => self.pattern(target),
                    ForHead::Pattern(p) => self.pattern(p),
                }
                self.expr(right);
                self.stmt(body);
            }
            StmtKind::While { test, body } | StmtKind::DoWhile { body, test } => {
                self.expr(test);
                self.stmt(body);
            }
            StmtKind::Return(e) => {
                if let Some(e) = e {
                    self.expr(e);
                }
            }
            StmtKind::Try { block, handler, finalizer } => {
                self.stmts(block);
                if let Some((p, body)) = handler.as_deref() {
                    if let Some(p) = p {
                        self.pattern(p);
                    }
                    self.stmts(body);
                }
                if let Some(f) = finalizer {
                    self.stmts(f);
                }
            }
            StmtKind::Switch { discriminant, cases } => {
                self.expr(discriminant);
                for c in cases {
                    if let Some(t) = &c.test {
                        self.expr(t);
                    }
                    self.stmts(&c.body);
                }
            }
            StmtKind::Labeled { body, .. } => self.stmt(body),
            StmtKind::With { object, body } => {
                self.expr(object);
                self.stmt(body);
            }
            StmtKind::Import(d) => {
                for n in d.default.iter().chain(d.namespace.iter()) {
                    self.name(n);
                }
                for (_, local) in &d.named {
                    self.name(local);
                }
            }
            StmtKind::Export(e) => match e {
                ExportDecl::DefaultExpr(x) => self.expr(x),
                ExportDecl::DefaultDecl(s) | ExportDecl::Decl(s) => self.stmt(s),
                ExportDecl::Named { specifiers, .. } => {
                    for (local, _) in specifiers {
                        self.name(local);
                    }
                }
                ExportDecl::All { .. } => {}
            },
        }
    }

    fn var_decl(&mut self, d: &VarDecl) {
        for decl in &d.declarations {
            self.pattern(&decl.target);
            if let Some(i) = &decl.init {
                self.expr(i);
            }
        }
    }

    fn pattern(&mut self, p: &Pattern) {
        match p {
            Pattern::Identifier(n) => self.name(n),
            Pattern::Expr(e) => self.target_expr(e),
            Pattern::Object { properties, rest } => {
                for pr in properties {
                    self.prop_key(&pr.key);
                    self.pattern(&pr.value);
                }
                if let Some(r) = rest {
                    self.pattern(r);
                }
            }
            Pattern::Array { elements, rest } => {
                for el in elements.iter().flatten() {
                    self.pattern(el);
                }
                if let Some(r) = rest {
                    self.pattern(r);
                }
            }
            Pattern::Assign { target, default } => {
                self.pattern(target);
                self.expr(default);
            }
        }
    }

    fn prop_key(&mut self, k: &PropertyKey) {
        match k {
            PropertyKey::Identifier(_) | PropertyKey::String(_) | PropertyKey::Number(_) => {}
            PropertyKey::Computed(e) => self.expr(e),
            PropertyKey::Private(n) => self.name(&format!("#{}", n)),
        }
    }

    fn elements(&mut self, list: &[ArrayElement]) {
        for a in list {
            match a {
                ArrayElement::Hole => {}
                ArrayElement::Expr(e) | ArrayElement::Spread(e) => self.expr(e),
            }
        }
    }

    fn class(&mut self, c: &Class) {
        if let Some(n) = &c.name {
            self.name(n);
        }
        if let Some(s) = &c.superclass {
            self.expr(s);
        }
        for m in &c.members {
            match m {
                ClassMember::Constructor(f) => self.function(f),
                ClassMember::Method { key, value, .. } | ClassMember::Getter { key, value, .. } | ClassMember::Setter { key, value, .. } => {
                    self.prop_key(key);
                    self.function(value);
                }
                ClassMember::Field { key, value, .. } => {
                    self.prop_key(key);
                    if let Some(v) = value {
                        self.synthetic(|a| a.expr(v));
                    }
                }
                ClassMember::StaticBlock(body) => self.synthetic(|a| a.stmts(body)),
            }
        }
    }

    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Identifier(n) => {
                if n == "arguments" {
                    self.args_mark(true);
                }
                self.name(n)
            }
            ExprKind::Member { object, property: property @ (MemberProp::Identifier(_) | MemberProp::Computed(_)), optional: false }
                if matches!(&object.kind, ExprKind::Identifier(n) if n == "arguments") && !matches!(property, MemberProp::Identifier(p) if p != "length") =>
            {
                // `arguments[k]` / `arguments.length`: served by the frame
                // unless written through (see `target_expr`).
                self.args_mark(false);
                self.name("arguments");
                if let MemberProp::Computed(k) = property {
                    self.expr(k);
                }
            }
            ExprKind::Null
            | ExprKind::Undefined
            | ExprKind::This
            | ExprKind::Super
            | ExprKind::Bool(_)
            | ExprKind::Number(_)
            | ExprKind::BigInt(_)
            | ExprKind::String(_)
            | ExprKind::Regex { .. }
            | ExprKind::NewTarget
            | ExprKind::ImportMeta => {}
            ExprKind::Template { expressions, .. } => {
                for x in expressions {
                    self.expr(x);
                }
            }
            ExprKind::TaggedTemplate { tag, expressions, .. } => {
                if is_arguments_member(tag) {
                    self.args_mark(true);
                }
                self.expr(tag);
                for x in expressions.iter() {
                    self.expr(x);
                }
            }
            ExprKind::Array(list) => self.elements(list),
            ExprKind::Object(props) => {
                for p in props {
                    match p {
                        ObjectProperty::Spread(x) => self.expr(x),
                        ObjectProperty::Property { key, value, .. } => {
                            self.prop_key(key);
                            match value {
                                PropertyValue::Init(x) => self.expr(x),
                                PropertyValue::Method(f) | PropertyValue::Getter(f) | PropertyValue::Setter(f) => self.function(f),
                            }
                        }
                    }
                }
            }
            ExprKind::Function(f) | ExprKind::Arrow(f) => self.function(f),
            ExprKind::Class(c) => self.class(c),
            ExprKind::Unary { op: UnaryOp::Delete, operand } => self.target_expr(operand),
            ExprKind::Unary { operand, .. } => self.expr(operand),
            ExprKind::Update { target, .. } => self.target_expr(target),
            ExprKind::Binary { left, right, .. } | ExprKind::Logical { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            ExprKind::Assign { target, value, .. } => {
                self.pattern(target);
                self.expr(value);
            }
            ExprKind::Conditional { test, consequent, alternate } => {
                self.expr(test);
                self.expr(consequent);
                self.expr(alternate);
            }
            ExprKind::Call { callee, args, .. } | ExprKind::New { callee, args } => {
                if matches!(&callee.kind, ExprKind::Identifier(n) if n == "eval") {
                    self.direct_eval();
                }
                if is_arguments_member(callee) {
                    // `arguments[k]()`: the object is the receiver.
                    self.args_mark(true);
                }
                self.expr(callee);
                self.elements(args);
            }
            ExprKind::Member { object, property, .. } => {
                self.expr(object);
                match property {
                    MemberProp::Identifier(_) => {}
                    MemberProp::Private(n) => self.name(&format!("#{}", n)),
                    MemberProp::Computed(k) => self.expr(k),
                }
            }
            ExprKind::OptionalChain(x) | ExprKind::Spread(x) | ExprKind::Await(x) | ExprKind::DynamicImport(x, None) => self.expr(x),
            ExprKind::DynamicImport(x, Some(o)) => {
                self.expr(x);
                self.expr(o);
            }
            ExprKind::Sequence(list) => {
                for x in list {
                    self.expr(x);
                }
            }
            ExprKind::Yield { argument, .. } => {
                if let Some(a) = argument {
                    self.expr(a);
                }
            }
            ExprKind::PrivateIn { name, object } => {
                self.name(&format!("#{}", name));
                self.expr(object);
            }
        }
    }
}
