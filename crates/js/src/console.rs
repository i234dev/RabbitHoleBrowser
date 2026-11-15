//! Console evaluations own their lexical scope; explicit globals persist.
use crate::ast::*;
use crate::{Realm, Value};

impl Realm {
    pub fn eval_console(&mut self, source: &str) -> Result<Value, Value> {
        let escaped = crate::value::escape_source(source);
        // Prefer a real top-level Await node even for `await(p)`, whose
        // spelling also parses as a call to an identifier in classic Script.
        let (program, asynchronous) = match crate::parser::parse_console(&escaped, true) {
            Ok(program) if program.body.iter().any(stmt_await) => (program, true),
            _ => (crate::parser::parse_console(&escaped, false).map_err(|e| {
                self.new_error("SyntaxError", &format!("{} (console:{}:{})", e.message, e.line, e.column))
            })?, false),
        };
        let code = crate::compiler::compile_console(&program, &escaped, asynchronous)
            .map_err(|e| self.new_error("SyntaxError", &e.message))?;
        self.adopt_code(&code);
        self.run_script(code)
    }
}

fn key_await(key: &PropertyKey) -> bool { matches!(key, PropertyKey::Computed(e) if expr_await(e)) }
fn pattern_await(p: &Pattern) -> bool {
    match p {
        Pattern::Identifier(_) => false,
        Pattern::Expr(e) => expr_await(e),
        Pattern::Assign { target, default } => pattern_await(target) || expr_await(default),
        Pattern::Array { elements, rest } => elements.iter().flatten().any(pattern_await) || rest.as_deref().is_some_and(pattern_await),
        Pattern::Object { properties, rest } => properties.iter().any(|p| key_await(&p.key) || pattern_await(&p.value)) || rest.as_deref().is_some_and(pattern_await),
    }
}
fn decl_await(d: &VarDecl) -> bool { d.declarations.iter().any(|d| pattern_await(&d.target) || d.init.as_ref().is_some_and(expr_await)) }
fn head_await(h: &ForHead) -> bool { match h { ForHead::VarDecl { target, .. } | ForHead::Pattern(target) => pattern_await(target) } }
fn class_await(c: &Class) -> bool {
    c.superclass.as_deref().is_some_and(expr_await) || c.members.iter().any(|m| match m {
        ClassMember::Constructor(_) => false,
        ClassMember::Method { key, .. } | ClassMember::Getter { key, .. } | ClassMember::Setter { key, .. } => key_await(key),
        ClassMember::Field { key, value, .. } => key_await(key) || value.as_ref().is_some_and(expr_await),
        ClassMember::StaticBlock(body) => body.iter().any(stmt_await),
    })
}
fn stmt_await(s: &Stmt) -> bool {
    match &s.kind {
        StmtKind::Expr(e) | StmtKind::Throw(e) => expr_await(e),
        StmtKind::VarDecl(d) => decl_await(d),
        StmtKind::FunctionDecl(_) => false,
        StmtKind::ClassDecl(c) => class_await(c),
        StmtKind::Block(body) => body.iter().any(stmt_await),
        StmtKind::If { test, consequent, alternate } => expr_await(test) || stmt_await(consequent) || alternate.as_deref().is_some_and(stmt_await),
        StmtKind::For { init, test, update, body } => init.as_deref().is_some_and(|i| match i { ForInit::VarDecl(d) => decl_await(d), ForInit::Expr(e) => expr_await(e) })
            || test.as_deref().is_some_and(expr_await) || update.as_deref().is_some_and(expr_await) || stmt_await(body),
        StmtKind::ForIn { left, right, body } => head_await(left) || expr_await(right) || stmt_await(body),
        StmtKind::ForOf { left, right, body, is_await } => *is_await || head_await(left) || expr_await(right) || stmt_await(body),
        StmtKind::While { test, body } | StmtKind::DoWhile { test, body } => expr_await(test) || stmt_await(body),
        StmtKind::Return(e) => e.as_ref().is_some_and(expr_await),
        StmtKind::Try { block, handler, finalizer } => block.iter().any(stmt_await)
            || handler.as_deref().is_some_and(|(p, b)| p.as_ref().is_some_and(pattern_await) || b.iter().any(stmt_await))
            || finalizer.as_ref().is_some_and(|b| b.iter().any(stmt_await)),
        StmtKind::Switch { discriminant, cases } => expr_await(discriminant) || cases.iter().any(|c| c.test.as_ref().is_some_and(expr_await) || c.body.iter().any(stmt_await)),
        StmtKind::Labeled { body, .. } => stmt_await(body),
        StmtKind::With { object, body } => expr_await(object) || stmt_await(body),
        StmtKind::Empty | StmtKind::Break(_) | StmtKind::Continue(_) | StmtKind::Debugger | StmtKind::Import(_) | StmtKind::Export(_) => false,
    }
}
fn element_await(a: &ArrayElement) -> bool { match a { ArrayElement::Expr(e) | ArrayElement::Spread(e) => expr_await(e), ArrayElement::Hole => false } }
fn expr_await(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Await(_) => true,
        ExprKind::Function(_) | ExprKind::Arrow(_) => false,
        ExprKind::Class(c) => class_await(c),
        ExprKind::Array(v) => v.iter().any(element_await),
        ExprKind::Object(v) => v.iter().any(|p| match p {
            ObjectProperty::Spread(e) => expr_await(e),
            ObjectProperty::Property { key, value, .. } => key_await(key) || match value { PropertyValue::Init(e) => expr_await(e), _ => false },
        }),
        ExprKind::Template { expressions, .. } => expressions.iter().any(expr_await),
        ExprKind::TaggedTemplate { tag, expressions, .. } => expr_await(tag) || expressions.iter().any(expr_await),
        ExprKind::Unary { operand, .. } => expr_await(operand),
        ExprKind::Update { target, .. } => expr_await(target),
        ExprKind::Binary { left, right, .. } | ExprKind::Logical { left, right, .. } => expr_await(left) || expr_await(right),
        ExprKind::Assign { target, value, .. } => pattern_await(target) || expr_await(value),
        ExprKind::Conditional { test, consequent, alternate } => expr_await(test) || expr_await(consequent) || expr_await(alternate),
        ExprKind::Call { callee, args, .. } | ExprKind::New { callee, args } => expr_await(callee) || args.iter().any(element_await),
        ExprKind::Member { object, property, .. } => expr_await(object) || matches!(property, MemberProp::Computed(e) if expr_await(e)),
        ExprKind::OptionalChain(e) | ExprKind::Spread(e) | ExprKind::PrivateIn { object: e, .. } => expr_await(e),
        ExprKind::Sequence(v) => v.iter().any(expr_await),
        ExprKind::Yield { argument, .. } => argument.as_deref().is_some_and(expr_await),
        ExprKind::DynamicImport(e, options) => expr_await(e) || options.as_deref().is_some_and(expr_await),
        _ => false,
    }
}
