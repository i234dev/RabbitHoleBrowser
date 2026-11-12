//! Static semantics: the early errors of ECMAScript that the grammar alone
//! does not express, checked on the tree after parsing.
//!
//! The parser reports what it sees while parsing (reserved words, `return`
//! outside functions, `super` placement, label targets, ...). This pass adds
//! the rules that need a wider view: declarations and scopes (duplicate
//! lexical names, lexical/var clashes, parameters vs. body declarations,
//! catch parameters, for heads), parameter lists (duplicates, `"use strict"`
//! with non-simple parameters, names that become strict through the body's
//! directive), declarations in single-statement positions, class bodies
//! (private names, constructors, `prototype`), object literals (duplicate
//! `__proto__`, cover-initialized names), context-dependent expressions
//! (`new.target`, `arguments` in class field initializers and static blocks,
//! `yield` / `await` in parameters, `await` in static blocks, `delete` of
//! private references) and module items (placement, duplicate and
//! undeclared exports).
//!
//! It runs on every script, module and eval program (see
//! [`crate::parser::parse_program`]); [`EarlyContext`] carries what eval
//! code inherits from its caller. Function bodies the parser skipped (lazy
//! parsing) are checked when they are parsed; the pre-scan's identifier list
//! tells this pass when a skipped body depends on context it knows now (a
//! private name, `arguments` in a class field...), in which case it parses
//! the body right away.

use crate::ast::*;
use crate::parser::ParseError;

/// The context a program is checked in: the defaults for scripts and
/// modules, or what direct eval code inherits from the calling code.
#[derive(Debug, Clone, Default)]
pub struct EarlyContext {
    /// Strict mode code (the caller is strict).
    pub strict: bool,
    /// Inside a non-arrow function (or a class field initializer / static
    /// block): `new.target` is allowed.
    pub in_function: bool,
    /// `super.x` is allowed (inside a method, field initializer or static
    /// block).
    pub in_method: bool,
    /// `super()` is allowed (inside a derived class constructor).
    pub in_derived_ctor: bool,
    /// Inside a class field initializer or static block: `arguments` is not
    /// allowed.
    pub in_class_field_init: bool,
    /// Private names (without `#`) of the enclosing classes.
    pub private_names: Vec<Name>,
}

type R = Result<(), ParseError>;

fn error(pos: Pos, message: impl Into<String>) -> ParseError {
    ParseError { message: message.into(), line: pos.line, column: pos.column }
}

/// Check a parsed program (script, module or eval code).
pub fn check_program(program: &Program, ctx: &EarlyContext) -> R {
    let mut c = Checker {
        is_module: program.is_module,
        strict: program.strict || ctx.strict,
        new_target: ctx.in_function || ctx.in_class_field_init,
        no_arguments: ctx.in_class_field_init,
        static_block: false,
        in_params: false,
        check_privates: true,
        privates: ctx.private_names.iter().map(|n| n.as_str()).collect(),
        scopes: Vec::new(),
    };
    c.push_scope(ScopeKind::Function);
    c.top_level(&program.body, program.is_module)
}

/// Check a function body parsed late (lazy parsing) as far as the context
/// recorded with it allows (private names are not checked: the enclosing
/// classes are not known any more).
pub fn check_lazy_body(f: &Function, lb: &LazyBody, body: &[Stmt], strict: bool) -> R {
    let mut c = Checker {
        is_module: lb.is_module,
        strict,
        new_target: true,
        no_arguments: f.is_arrow && lb.in_class_body,
        static_block: false,
        in_params: false,
        check_privates: false,
        privates: Vec::new(),
        scopes: Vec::new(),
    };
    c.function_inner(f, Body::Stmts(body), strict, FnKind::of(f))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScopeKind {
    /// A var scope: function body, script, module, static block.
    Function,
    Block,
    /// A catch clause parameter scope (`simple`: a single identifier).
    Catch { simple: bool },
}

struct Scope<'a> {
    kind: ScopeKind,
    lexical: Vec<&'a str>,
    /// Built for scopes with many lexical names.
    set: Option<std::collections::HashSet<&'a str, crate::value::FxBuild>>,
    /// Var names declared at module top level (for exports).
    vars: Vec<&'a str>,
}

impl<'a> Scope<'a> {
    fn has(&self, n: &str) -> bool {
        match &self.set {
            Some(s) => s.contains(n),
            None => self.lexical.iter().any(|&m| m == n),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FnKind {
    /// Function declaration / expression.
    Plain,
    Arrow,
    /// Object / class method, accessor or constructor.
    Method,
}

impl FnKind {
    fn of(f: &Function) -> FnKind {
        if f.is_arrow {
            FnKind::Arrow
        } else {
            FnKind::Plain
        }
    }
}

#[derive(Clone, Copy)]
enum Body<'a> {
    Stmts(&'a [Stmt]),
    Expr(&'a Expr),
    /// Not parsed (lazy): only the parameters are checked.
    Skipped,
}

/// Where a statement that is not a list item appears.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SubPos {
    If,
    Loop,
    With,
    /// Body of a labelled statement.
    Label,
}

struct Checker<'a> {
    is_module: bool,
    strict: bool,
    /// `new.target` allowed.
    new_target: bool,
    /// `arguments` not allowed (class field initializer / static block).
    no_arguments: bool,
    /// `await` is not an identifier (in a class static block, also in its
    /// arrow functions, and in async arrow parameters).
    static_block: bool,
    /// In a parameter list: `yield` / `await` expressions are not allowed.
    in_params: bool,
    /// Private names are checked against `privates` (unknown for bodies
    /// parsed late).
    check_privates: bool,
    /// Private names of the enclosing classes.
    privates: Vec<&'a str>,
    scopes: Vec<Scope<'a>>,
}

fn is_strict_reserved(n: &str) -> bool {
    matches!(n, "implements" | "interface" | "let" | "package" | "private" | "protected" | "public" | "static" | "yield")
}

/// BoundNames of a binding pattern.
fn bound_names<'b>(p: &'b Pattern, out: &mut Vec<&'b str>) {
    match p {
        Pattern::Identifier(n) => out.push(n.as_str()),
        Pattern::Expr(_) => {}
        Pattern::Object { properties, rest } => {
            for prop in properties {
                bound_names(&prop.value, out);
            }
            if let Some(r) = rest {
                bound_names(r, out);
            }
        }
        Pattern::Array { elements, rest } => {
            for el in elements.iter().flatten() {
                bound_names(el, out);
            }
            if let Some(r) = rest {
                bound_names(r, out);
            }
        }
        Pattern::Assign { target, .. } => bound_names(target, out),
    }
}

/// Index of the first name that repeats an earlier one.
fn has_duplicate(names: &[&str]) -> Option<usize> {
    if names.len() <= 24 {
        (1..names.len()).find(|&i| names[..i].contains(&names[i]))
    } else {
        let mut seen: std::collections::HashSet<&str, crate::value::FxBuild> = Default::default();
        names.iter().position(|n| !seen.insert(n))
    }
}

/// The body starts with a `"use strict"` directive.
fn has_use_strict(body: &[Stmt]) -> bool {
    for s in body {
        match &s.kind {
            StmtKind::Expr(Expr { kind: ExprKind::String(v), .. }) => {
                if v == "use strict" {
                    return true;
                }
            }
            _ => return false,
        }
    }
    false
}

fn is_private_reference(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Member { property: MemberProp::Private(_), .. } => true,
        ExprKind::OptionalChain(inner) => is_private_reference(inner),
        _ => false,
    }
}

fn key_is(key: &PropertyKey, name: &str) -> bool {
    match key {
        PropertyKey::Identifier(n) | PropertyKey::String(n) => n == name,
        _ => false,
    }
}

/// A labelled statement whose (innermost) body is a function declaration.
fn is_labelled_function(s: &Stmt) -> bool {
    match &s.kind {
        StmtKind::Labeled { body, .. } => match &body.kind {
            StmtKind::FunctionDecl(_) => true,
            _ => is_labelled_function(body),
        },
        _ => false,
    }
}

/// The declaration a statement list item stands for (export wrappers and
/// labels removed).
fn declaration(s: &Stmt) -> &Stmt {
    match &s.kind {
        StmtKind::Export(ExportDecl::Decl(d)) | StmtKind::Export(ExportDecl::DefaultDecl(d)) => declaration(d),
        StmtKind::Labeled { body, .. } => declaration(body),
        _ => s,
    }
}

fn redeclared(pos: Pos, name: &str) -> ParseError {
    error(pos, format!("Identifier '{name}' has already been declared"))
}

impl<'a> Checker<'a> {
    fn push_scope(&mut self, kind: ScopeKind) {
        self.scopes.push(Scope { kind, lexical: Vec::new(), set: None, vars: Vec::new() });
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Record the lexically declared names of a statement list in the
    /// current scope, checking duplicates. `top`: the list is a function /
    /// script / static block body, where function declarations are var
    /// scoped (`module`: a module body, where they are lexical).
    fn declare_lexical(&mut self, stmts: impl Iterator<Item = &'a Stmt>, top: bool, module: bool) -> R {
        // (name, bound by a plain function declaration, position)
        let mut found: Vec<(&'a str, bool, Pos)> = Vec::new();
        for item in stmts {
            if let StmtKind::Import(d) = &item.kind {
                for n in d.default.iter().chain(d.namespace.iter()).chain(d.named.iter().map(|(_, l)| l)) {
                    if n == "eval" || n == "arguments" {
                        return Err(error(item.pos, format!("Unexpected '{n}' as an import binding in strict mode")));
                    }
                    found.push((n.as_str(), false, item.pos));
                }
                continue;
            }
            let s = declaration(item);
            match &s.kind {
                StmtKind::VarDecl(d) if d.kind != VarKind::Var => {
                    let mut names = Vec::new();
                    for decl in &d.declarations {
                        bound_names(&decl.target, &mut names);
                    }
                    for n in names {
                        if n == "let" {
                            return Err(error(s.pos, "let is disallowed as a lexically bound name"));
                        }
                        found.push((n, false, s.pos));
                    }
                }
                StmtKind::ClassDecl(c) => {
                    if let Some(n) = &c.name {
                        found.push((n.as_str(), false, s.pos));
                    }
                }
                StmtKind::FunctionDecl(f) if !top || module => {
                    if let Some(n) = &f.name {
                        found.push((n.as_str(), !f.is_async && !f.is_generator, s.pos));
                    }
                }
                _ => {}
            }
        }
        if found.is_empty() {
            return Ok(());
        }
        let names: Vec<&'a str> = found.iter().map(|f| f.0).collect();
        if has_duplicate(&names).is_some() {
            // Annex B: sloppy blocks may repeat plain function declarations.
            for i in 1..found.len() {
                for j in 0..i {
                    if found[i].0 == found[j].0 && (top || self.strict || !found[i].1 || !found[j].1) {
                        return Err(redeclared(found[i].2, found[i].0));
                    }
                }
            }
        }
        let scope = self.scopes.last_mut().unwrap();
        scope.lexical.extend(names);
        if scope.lexical.len() > 24 {
            scope.set = Some(scope.lexical.iter().copied().collect());
        }
        Ok(())
    }

    /// A var-scoped declaration of `name`: it may not also be declared
    /// lexically in any scope up to its var scope.
    fn declare_var(&mut self, name: &'a str, pos: Pos, _for_of: bool) -> R {
        for i in (0..self.scopes.len()).rev() {
            let sc = &self.scopes[i];
            match sc.kind {
                ScopeKind::Catch { simple } => {
                    // Annex B: `catch (e) { var e }` (also in a for-of head).
                    if sc.has(name) && !simple {
                        return Err(redeclared(pos, name));
                    }
                }
                ScopeKind::Block => {
                    if sc.has(name) {
                        return Err(redeclared(pos, name));
                    }
                }
                ScopeKind::Function => {
                    if sc.has(name) {
                        return Err(redeclared(pos, name));
                    }
                    if self.is_module {
                        self.scopes[i].vars.push(name);
                    }
                    break;
                }
            }
        }
        Ok(())
    }

    fn binding_name(&self, n: &str, pos: Pos) -> R {
        if self.static_block && n == "await" {
            return Err(error(pos, "Unexpected 'await' in class static block"));
        }
        Ok(())
    }

    fn reference(&self, n: &str, pos: Pos) -> R {
        if self.no_arguments && n == "arguments" {
            return Err(error(pos, "'arguments' is not allowed in class field initializer or static initialization block"));
        }
        if self.static_block && n == "await" {
            return Err(error(pos, "Unexpected 'await' in class static block"));
        }
        Ok(())
    }

    fn check_private(&self, n: &str, pos: Pos) -> R {
        if self.check_privates && !self.privates.contains(&n) {
            return Err(error(pos, format!("Private field '#{n}' must be declared in an enclosing class")));
        }
        Ok(())
    }

    // ------------------------------------------------------------ bodies

    /// A script / module / static block body (the current scope is its var
    /// scope).
    fn top_level(&mut self, stmts: &'a [Stmt], module: bool) -> R {
        self.declare_lexical(stmts.iter(), true, module)?;
        if !module {
            return self.top_level_rest(stmts);
        }
        for s in stmts {
            match &s.kind {
                StmtKind::Import(_) => {}
                StmtKind::Export(e) => match e {
                    ExportDecl::DefaultExpr(x) => self.expr(x)?,
                    ExportDecl::DefaultDecl(d) | ExportDecl::Decl(d) => self.stmt(d)?,
                    ExportDecl::Named { .. } | ExportDecl::All { .. } => {}
                },
                _ => self.stmt(s)?,
            }
        }
        self.check_exports(stmts)
    }

    /// The statements of a non-module body whose lexical declarations are
    /// recorded: top-level function declarations are var scoped.
    fn top_level_rest(&mut self, stmts: &'a [Stmt]) -> R {
        for s in stmts {
            let d = declaration(s);
            if let StmtKind::FunctionDecl(f) = &d.kind {
                if let Some(n) = &f.name {
                    self.declare_var(n.as_str(), d.pos, false)?;
                }
            }
        }
        for s in stmts {
            self.stmt(s)?;
        }
        Ok(())
    }

    fn check_exports(&mut self, stmts: &'a [Stmt]) -> R {
        let mut exported: Vec<(&str, Pos)> = Vec::new();
        for s in stmts {
            let StmtKind::Export(e) = &s.kind else { continue };
            match e {
                ExportDecl::DefaultExpr(_) | ExportDecl::DefaultDecl(_) => exported.push(("default", s.pos)),
                ExportDecl::Decl(d) => match &d.kind {
                    StmtKind::VarDecl(v) => {
                        let mut names = Vec::new();
                        for decl in &v.declarations {
                            bound_names(&decl.target, &mut names);
                        }
                        exported.extend(names.into_iter().map(|n| (n, s.pos)));
                    }
                    StmtKind::FunctionDecl(f) => exported.extend(f.name.as_deref().map(|n| (n, s.pos))),
                    StmtKind::ClassDecl(c) => exported.extend(c.name.as_deref().map(|n| (n, s.pos))),
                    _ => {}
                },
                ExportDecl::Named { specifiers, source } => {
                    for (local, name) in specifiers {
                        if source.is_none() {
                            let sc = &self.scopes[0];
                            if !sc.has(local) && !sc.vars.iter().any(|v| *v == local.as_str()) {
                                return Err(error(s.pos, format!("Export '{local}' is not defined in module")));
                            }
                        }
                        exported.push((name.as_str(), s.pos));
                    }
                }
                ExportDecl::All { namespace, .. } => {
                    if let Some(ns) = namespace {
                        exported.push((ns.as_str(), s.pos));
                    }
                }
            }
        }
        let names: Vec<&str> = exported.iter().map(|e| e.0).collect();
        if let Some(i) = has_duplicate(&names) {
            return Err(error(exported[i].1, format!("Duplicate export of '{}'", exported[i].0)));
        }
        Ok(())
    }

    /// A function: parameters, then its body (parsed now when it was
    /// skipped and depends on context known only here).
    fn function(&mut self, f: &'a Function, kind: FnKind) -> R {
        match &f.body {
            FunctionBody::Block(stmts) => self.function_inner(f, Body::Stmts(stmts), f.strict, kind),
            FunctionBody::Expr(e) => self.function_inner(f, Body::Expr(e), f.strict, kind),
            FunctionBody::Lazy(lb) => {
                if self.lazy_needs_parse(f, lb) {
                    let (stmts, strict) = crate::parser::parse_lazy_body(f, lb)?;
                    let mut sub = Checker {
                        is_module: self.is_module,
                        strict: self.strict,
                        new_target: self.new_target,
                        no_arguments: self.no_arguments,
                        static_block: self.static_block,
                        in_params: self.in_params,
                        check_privates: self.check_privates,
                        privates: self.privates.clone(),
                        scopes: Vec::new(),
                    };
                    return sub.function_inner(f, Body::Stmts(&stmts), strict, kind);
                }
                self.function_inner(f, Body::Skipped, f.strict, kind)
            }
        }
    }

    /// Does a skipped body mention something whose validity depends on
    /// context known here (private names, `arguments` in a class field)?
    fn lazy_needs_parse(&self, f: &Function, lb: &LazyBody) -> bool {
        lb.names.iter().any(|n| match n.strip_prefix('#') {
            Some(p) => self.check_privates && !self.privates.contains(&p),
            None => f.is_arrow && self.no_arguments && n == "arguments",
        })
    }

    fn function_inner(&mut self, f: &'a Function, body: Body<'a>, strict: bool, kind: FnKind) -> R {
        // The name of a declaration is bound outside the function (a
        // function expression's inside it).
        if let Some(n) = &f.name {
            if !f.self_binding {
                self.binding_name(n, f.pos)?;
            }
            if strict && (n == "eval" || n == "arguments" || is_strict_reserved(n)) {
                return Err(error(f.pos, format!("Unexpected eval, arguments or strict mode reserved word '{n}'")));
            }
        }
        let saved = (self.strict, self.new_target, self.no_arguments, self.static_block, self.in_params);
        self.strict = strict;
        if kind != FnKind::Arrow {
            self.new_target = true;
            self.no_arguments = false;
        }
        // `await` stays an invalid identifier in the parameters of arrows
        // within static blocks (see `function_parts`).
        if kind != FnKind::Arrow || f.is_async {
            self.static_block = false;
        }
        let r = self.function_parts(f, body, strict, kind);
        (self.strict, self.new_target, self.no_arguments, self.static_block, self.in_params) = saved;
        r
    }

    fn function_parts(&mut self, f: &'a Function, body: Body<'a>, strict: bool, kind: FnKind) -> R {
        let mut names: Vec<&'a str> = Vec::new();
        for p in &f.params {
            bound_names(&p.pattern, &mut names);
        }
        let simple = f.params.iter().all(|p| !p.rest && matches!(p.pattern, Pattern::Identifier(_)));
        if strict || !simple || kind != FnKind::Plain {
            if let Some(i) = has_duplicate(&names) {
                return Err(error(f.pos, format!("Duplicate parameter name '{}' not allowed in this context", names[i])));
            }
        }
        if strict {
            for n in &names {
                if *n == "eval" || *n == "arguments" || is_strict_reserved(n) {
                    return Err(error(f.pos, format!("Unexpected eval, arguments or strict mode reserved word '{n}'")));
                }
            }
        }
        if let Body::Stmts(stmts) = body {
            if !simple && has_use_strict(stmts) {
                return Err(error(f.pos, "Illegal 'use strict' directive in function with non-simple parameter list"));
            }
        }
        self.in_params = true;
        // The parameters of an async arrow are reparsed as such: `await`
        // is not an identifier anywhere in them.
        let saved_block = self.static_block;
        if f.is_async && f.is_arrow {
            self.static_block = true;
        }
        for p in &f.params {
            self.pattern(&p.pattern, true, f.pos)?;
        }
        // An arrow's parameters are in the static block, its body is not.
        self.static_block = saved_block && kind != FnKind::Arrow;
        self.in_params = false;
        match body {
            Body::Stmts(stmts) => {
                let depth = self.scopes.len();
                self.push_scope(ScopeKind::Function);
                let r = (|| -> R {
                    self.declare_lexical(stmts.iter(), true, false)?;
                    if let Some(n) = names.iter().find(|n| self.scopes[depth].has(n)) {
                        return Err(redeclared(f.pos, n));
                    }
                    self.top_level_rest(stmts)
                })();
                self.scopes.truncate(depth);
                r
            }
            Body::Expr(e) => self.expr(e),
            Body::Skipped => Ok(()),
        }
    }

    fn class(&mut self, c: &'a Class) -> R {
        let saved_strict = self.strict;
        self.strict = true;
        let r = self.class_inner(c);
        self.strict = saved_strict;
        r
    }

    fn class_inner(&mut self, c: &'a Class) -> R {
        if let Some(sc) = &c.superclass {
            self.expr(sc)?;
        }
        // Private names declared by the class: (name, 0 other / 1 getter /
        // 2 setter, static).
        let mut declared: Vec<(&'a str, u8, bool)> = Vec::new();
        let mut ctors = 0;
        for m in &c.members {
            let (key, kind, is_static, pos) = match m {
                ClassMember::Constructor(f) => {
                    ctors += 1;
                    if ctors > 1 {
                        return Err(error(f.pos, "A class may only have one constructor"));
                    }
                    continue;
                }
                ClassMember::Method { key, value, is_static } => (key, 0u8, *is_static, value.pos),
                ClassMember::Getter { key, value, is_static } => (key, 1, *is_static, value.pos),
                ClassMember::Setter { key, value, is_static } => (key, 2, *is_static, value.pos),
                ClassMember::Field { key, is_static, .. } => (key, 3, *is_static, c.pos),
                ClassMember::StaticBlock(_) => continue,
            };
            if is_static && key_is(key, "prototype") {
                return Err(error(pos, "Classes may not have a static property named 'prototype'"));
            }
            if !is_static && (kind == 1 || kind == 2) && key_is(key, "constructor") {
                return Err(error(pos, "Class constructor may not be an accessor"));
            }
            if kind == 3 && key_is(key, "constructor") {
                return Err(error(pos, "Classes may not have a field named 'constructor'"));
            }
            if let PropertyKey::Private(n) = key {
                if n == "constructor" {
                    return Err(error(pos, "Classes may not have a private field named '#constructor'"));
                }
                let kind = if kind == 3 { 0 } else { kind };
                let mut prev = declared.iter().filter(|d| d.0 == n.as_str());
                let ok = match (prev.next(), prev.next()) {
                    (None, _) => true,
                    (Some(p), None) => p.2 == is_static && ((p.1 == 1 && kind == 2) || (p.1 == 2 && kind == 1)),
                    _ => false,
                };
                if !ok {
                    return Err(error(pos, format!("Identifier '#{n}' has already been declared")));
                }
                declared.push((n.as_str(), kind, is_static));
            }
        }
        let mark = self.privates.len();
        self.privates.extend(declared.iter().map(|d| d.0));
        let r = self.class_members(c);
        self.privates.truncate(mark);
        r
    }

    fn class_members(&mut self, c: &'a Class) -> R {
        for m in &c.members {
            match m {
                ClassMember::Constructor(f) => self.function(f, FnKind::Method)?,
                ClassMember::Method { key, value, .. } | ClassMember::Getter { key, value, .. } | ClassMember::Setter { key, value, .. } => {
                    self.key(key)?;
                    self.function(value, FnKind::Method)?;
                }
                ClassMember::Field { key, value, .. } => {
                    self.key(key)?;
                    if let Some(v) = value {
                        let saved = (self.new_target, self.no_arguments, self.static_block, self.in_params);
                        self.new_target = true;
                        self.no_arguments = true;
                        self.static_block = false;
                        self.in_params = false;
                        let r = self.expr(v);
                        (self.new_target, self.no_arguments, self.static_block, self.in_params) = saved;
                        r?;
                    }
                }
                ClassMember::StaticBlock(stmts) => {
                    let saved = (self.new_target, self.no_arguments, self.static_block, self.in_params);
                    self.new_target = true;
                    self.no_arguments = true;
                    self.static_block = true;
                    self.in_params = false;
                    let depth = self.scopes.len();
                    self.push_scope(ScopeKind::Function);
                    let r = self.top_level(stmts, false);
                    self.scopes.truncate(depth);
                    (self.new_target, self.no_arguments, self.static_block, self.in_params) = saved;
                    r?;
                }
            }
        }
        Ok(())
    }

    fn key(&mut self, key: &'a PropertyKey) -> R {
        if let PropertyKey::Computed(e) = key {
            self.expr(e)?;
        }
        Ok(())
    }

    // -------------------------------------------------------- statements

    fn block(&mut self, stmts: &'a [Stmt]) -> R {
        self.push_scope(ScopeKind::Block);
        let r = (|| -> R {
            self.declare_lexical(stmts.iter(), false, false)?;
            for s in stmts {
                self.stmt(s)?;
            }
            Ok(())
        })();
        self.pop_scope();
        r
    }

    /// A statement in a single-statement position (if / loop / with /
    /// label body).
    fn sub(&mut self, s: &'a Stmt, at: SubPos) -> R {
        match &s.kind {
            StmtKind::FunctionDecl(f) => {
                let ok = matches!(at, SubPos::If | SubPos::Label) && !self.strict && !f.is_async && !f.is_generator;
                if !ok {
                    return Err(error(s.pos, "Function declarations are not allowed in this position"));
                }
                // Annex B: as if in a block of its own.
                return self.block(std::slice::from_ref(s));
            }
            StmtKind::ClassDecl(_) => return Err(error(s.pos, "Class declarations are not allowed in a single-statement context")),
            StmtKind::VarDecl(d) if d.kind != VarKind::Var => {
                return Err(error(s.pos, "Lexical declaration cannot appear in a single-statement context"));
            }
            StmtKind::Labeled { .. } if at != SubPos::Label && is_labelled_function(s) => {
                return Err(error(s.pos, "Labelled function declarations are not allowed in this position"));
            }
            _ => {}
        }
        self.stmt(s)
    }

    fn var_decl(&mut self, d: &'a VarDecl, pos: Pos) -> R {
        for decl in &d.declarations {
            if d.kind == VarKind::Var {
                if let Pattern::Identifier(n) = &decl.target {
                    self.declare_var(n, pos, false)?;
                } else {
                    let mut names = Vec::new();
                    bound_names(&decl.target, &mut names);
                    for n in names {
                        self.declare_var(n, pos, false)?;
                    }
                }
            }
            self.pattern(&decl.target, true, pos)?;
            if let Some(init) = &decl.init {
                self.expr(init)?;
            }
        }
        Ok(())
    }

    /// Push the scope of a loop head (holding its `let` / `const` names).
    fn loop_head_scope(&mut self, kind: VarKind, targets: &mut dyn Iterator<Item = &'a Pattern>, pos: Pos) -> R {
        self.push_scope(ScopeKind::Block);
        if kind == VarKind::Var {
            return Ok(());
        }
        let mut names = Vec::new();
        for t in targets {
            bound_names(t, &mut names);
        }
        if names.contains(&"let") {
            return Err(error(pos, "let is disallowed as a lexically bound name"));
        }
        if let Some(i) = has_duplicate(&names) {
            return Err(redeclared(pos, names[i]));
        }
        self.scopes.last_mut().unwrap().lexical = names;
        Ok(())
    }

    fn stmt(&mut self, s: &'a Stmt) -> R {
        let pos = s.pos;
        match &s.kind {
            StmtKind::Empty | StmtKind::Debugger | StmtKind::Break(_) | StmtKind::Continue(_) => Ok(()),
            StmtKind::Expr(e) | StmtKind::Throw(e) => self.expr(e),
            StmtKind::Return(e) => match e {
                Some(e) => self.expr(e),
                None => Ok(()),
            },
            StmtKind::VarDecl(d) => self.var_decl(d, pos),
            StmtKind::FunctionDecl(f) => self.function(f, FnKind::Plain),
            StmtKind::ClassDecl(c) => {
                if let Some(n) = &c.name {
                    self.binding_name(n, pos)?;
                }
                self.class(c)
            }
            StmtKind::Block(b) => self.block(b),
            StmtKind::If { test, consequent, alternate } => {
                self.expr(test)?;
                self.sub(consequent, SubPos::If)?;
                match alternate {
                    Some(a) => self.sub(a, SubPos::If),
                    None => Ok(()),
                }
            }
            StmtKind::For { init, test, update, body } => {
                let r = (|| -> R {
                    match init.as_deref() {
                        Some(ForInit::VarDecl(d)) => {
                            self.loop_head_scope(d.kind, &mut d.declarations.iter().map(|x| &x.target), pos)?;
                            self.var_decl(d, pos)?;
                        }
                        Some(ForInit::Expr(e)) => {
                            self.push_scope(ScopeKind::Block);
                            self.expr(e)?;
                        }
                        None => self.push_scope(ScopeKind::Block),
                    }
                    if let Some(t) = test {
                        self.expr(t)?;
                    }
                    if let Some(u) = update {
                        self.expr(u)?;
                    }
                    self.sub(body, SubPos::Loop)
                })();
                self.pop_scope();
                r
            }
            StmtKind::ForIn { left, right, body } | StmtKind::ForOf { left, right, body, .. } => {
                let is_of = matches!(s.kind, StmtKind::ForOf { .. });
                let r = (|| -> R {
                    match &**left {
                        ForHead::VarDecl { kind, target } => {
                            self.loop_head_scope(*kind, &mut std::iter::once(target), pos)?;
                            if *kind == VarKind::Var {
                                let mut names = Vec::new();
                                bound_names(target, &mut names);
                                for n in names {
                                    self.declare_var(n, pos, is_of)?;
                                }
                            }
                            self.pattern(target, true, pos)?;
                        }
                        ForHead::Pattern(p) => {
                            self.push_scope(ScopeKind::Block);
                            self.pattern(p, false, pos)?;
                        }
                    }
                    self.expr(right)?;
                    self.sub(body, SubPos::Loop)
                })();
                self.pop_scope();
                r
            }
            StmtKind::While { test, body } | StmtKind::DoWhile { body, test } => {
                self.expr(test)?;
                self.sub(body, SubPos::Loop)
            }
            StmtKind::Try { block, handler, finalizer } => {
                self.block(block)?;
                if let Some(h) = handler {
                    self.catch(&h.0, &h.1, pos)?;
                }
                match finalizer {
                    Some(f) => self.block(f),
                    None => Ok(()),
                }
            }
            StmtKind::Switch { discriminant, cases } => {
                self.expr(discriminant)?;
                self.push_scope(ScopeKind::Block);
                let r = (|| -> R {
                    self.declare_lexical(cases.iter().flat_map(|c| c.body.iter()), false, false)?;
                    for c in cases {
                        if let Some(t) = &c.test {
                            self.expr(t)?;
                        }
                        for s in &c.body {
                            self.stmt(s)?;
                        }
                    }
                    Ok(())
                })();
                self.pop_scope();
                r
            }
            StmtKind::Labeled { label, body } => {
                if self.static_block && label == "await" {
                    return Err(error(pos, "Unexpected 'await' in class static block"));
                }
                self.sub(body, SubPos::Label)
            }
            StmtKind::With { object, body } => {
                self.expr(object)?;
                self.sub(body, SubPos::With)
            }
            StmtKind::Import(_) | StmtKind::Export(_) => Err(error(pos, "Import and export declarations may only appear at the top level of a module")),
        }
    }

    fn catch(&mut self, param: &'a Option<Pattern>, body: &'a [Stmt], pos: Pos) -> R {
        let mut names = Vec::new();
        if let Some(p) = param {
            bound_names(p, &mut names);
        }
        if let Some(i) = has_duplicate(&names) {
            return Err(redeclared(pos, names[i]));
        }
        let simple = matches!(param, Some(Pattern::Identifier(_)));
        self.push_scope(ScopeKind::Catch { simple });
        self.scopes.last_mut().unwrap().lexical = names;
        let r = (|| -> R {
            if let Some(p) = param {
                self.pattern(p, true, pos)?;
            }
            self.push_scope(ScopeKind::Block);
            let r = (|| -> R {
                self.declare_lexical(body.iter(), false, false)?;
                let n = self.scopes.len();
                if let Some(dup) = self.scopes[n - 1].lexical.iter().find(|x| self.scopes[n - 2].lexical.contains(x)) {
                    return Err(redeclared(pos, dup));
                }
                for s in body {
                    self.stmt(s)?;
                }
                Ok(())
            })();
            self.pop_scope();
            r
        })();
        self.pop_scope();
        r
    }

    // ------------------------------------------------------- expressions

    fn pattern(&mut self, p: &'a Pattern, binding: bool, pos: Pos) -> R {
        match p {
            Pattern::Identifier(n) => {
                if binding {
                    self.binding_name(n, pos)
                } else {
                    self.reference(n, pos)
                }
            }
            Pattern::Expr(e) => self.expr(e),
            Pattern::Object { properties, rest } => {
                for prop in properties {
                    self.key(&prop.key)?;
                    self.pattern(&prop.value, binding, pos)?;
                }
                match rest {
                    Some(r) => self.pattern(r, binding, pos),
                    None => Ok(()),
                }
            }
            Pattern::Array { elements, rest } => {
                for el in elements.iter().flatten() {
                    self.pattern(el, binding, pos)?;
                }
                match rest {
                    Some(r) => self.pattern(r, binding, pos),
                    None => Ok(()),
                }
            }
            Pattern::Assign { target, default } => {
                self.pattern(target, binding, pos)?;
                self.expr(default)
            }
        }
    }

    fn elements(&mut self, els: &'a [ArrayElement]) -> R {
        for el in els {
            match el {
                ArrayElement::Hole => {}
                ArrayElement::Expr(e) | ArrayElement::Spread(e) => self.expr(e)?,
            }
        }
        Ok(())
    }

    fn object(&mut self, props: &'a [ObjectProperty]) -> R {
        let mut proto = false;
        for p in props {
            match p {
                ObjectProperty::Spread(x) => self.expr(x)?,
                ObjectProperty::Property { key, value, shorthand } => {
                    self.key(key)?;
                    match value {
                        PropertyValue::Init(v) => {
                            if !*shorthand && key_is(key, "__proto__") {
                                if proto {
                                    return Err(error(v.pos, "Duplicate __proto__ fields are not allowed in object literals"));
                                }
                                proto = true;
                            }
                            if *shorthand && matches!(v.kind, ExprKind::Assign { .. }) {
                                return Err(error(v.pos, "Invalid shorthand property initializer"));
                            }
                            self.expr(v)?;
                        }
                        PropertyValue::Method(f) | PropertyValue::Getter(f) | PropertyValue::Setter(f) => self.function(f, FnKind::Method)?,
                    }
                }
            }
        }
        Ok(())
    }

    fn expr(&mut self, e: &'a Expr) -> R {
        let pos = e.pos;
        match &e.kind {
            ExprKind::Identifier(n) => self.reference(n, pos),
            ExprKind::Null | ExprKind::Undefined | ExprKind::This | ExprKind::Super | ExprKind::Bool(_) | ExprKind::Number(_) | ExprKind::BigInt(_) | ExprKind::String(_) | ExprKind::Regex { .. } | ExprKind::ImportMeta => Ok(()),
            ExprKind::Template { expressions, .. } => {
                for x in expressions {
                    self.expr(x)?;
                }
                Ok(())
            }
            ExprKind::TaggedTemplate { tag, expressions, .. } => {
                self.expr(tag)?;
                for x in expressions.iter() {
                    self.expr(x)?;
                }
                Ok(())
            }
            ExprKind::Array(els) => self.elements(els),
            ExprKind::Object(props) => self.object(props),
            ExprKind::Function(f) => self.function(f, FnKind::Plain),
            ExprKind::Arrow(f) => self.function(f, FnKind::Arrow),
            ExprKind::Class(c) => {
                if let Some(n) = &c.name {
                    self.binding_name(n, pos)?;
                }
                self.class(c)
            }
            ExprKind::Unary { op, operand } => {
                if *op == UnaryOp::Delete && is_private_reference(operand) {
                    return Err(error(pos, "Private fields can not be deleted"));
                }
                self.expr(operand)
            }
            ExprKind::Update { target, .. } => self.expr(target),
            ExprKind::Binary { left, right, .. } | ExprKind::Logical { left, right, .. } => {
                self.expr(left)?;
                self.expr(right)
            }
            ExprKind::Assign { target, value, .. } => {
                self.pattern(target, false, pos)?;
                self.expr(value)
            }
            ExprKind::Conditional { test, consequent, alternate } => {
                self.expr(test)?;
                self.expr(consequent)?;
                self.expr(alternate)
            }
            ExprKind::Call { callee, args, .. } | ExprKind::New { callee, args } => {
                self.expr(callee)?;
                self.elements(args)
            }
            ExprKind::Member { object, property, .. } => {
                self.expr(object)?;
                match property {
                    MemberProp::Identifier(_) => Ok(()),
                    MemberProp::Private(n) => {
                        if matches!(object.kind, ExprKind::Super) {
                            return Err(error(pos, "Unexpected private field after 'super'"));
                        }
                        self.check_private(n, pos)
                    }
                    MemberProp::Computed(x) => self.expr(x),
                }
            }
            ExprKind::OptionalChain(x) | ExprKind::Spread(x) | ExprKind::DynamicImport(x) => self.expr(x),
            ExprKind::Sequence(xs) => {
                for x in xs {
                    self.expr(x)?;
                }
                Ok(())
            }
            ExprKind::Yield { argument, .. } => {
                if self.in_params {
                    return Err(error(pos, "Yield expression not allowed in formal parameter"));
                }
                match argument {
                    Some(a) => self.expr(a),
                    None => Ok(()),
                }
            }
            ExprKind::Await(x) => {
                if self.in_params {
                    return Err(error(pos, "Illegal await-expression in formal parameters"));
                }
                if self.static_block {
                    return Err(error(pos, "Unexpected 'await' in class static block"));
                }
                self.expr(x)
            }
            ExprKind::NewTarget => {
                if !self.new_target {
                    return Err(error(pos, "new.target expression is not allowed here"));
                }
                Ok(())
            }
            ExprKind::PrivateIn { name, object } => {
                self.check_private(name, pos)?;
                self.expr(object)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    fn rejects(src: &str) {
        assert!(crate::parser::parse_program(src, false).is_err(), "should be a SyntaxError: {src}");
    }

    fn accepts(src: &str) {
        if let Err(e) = crate::parser::parse_program(src, false) {
            panic!("should parse: {src}: {e}");
        }
    }

    #[test]
    fn declarations() {
        rejects("let a; let a;");
        rejects("let a; var a;");
        rejects("var a; let a;");
        rejects("{ let a; { var a; } }");
        rejects("const a = 1; function a() {}");
        rejects("function f(a) { let a; }");
        rejects("switch (0) { case 1: let f; default: let f }");
        rejects("try {} catch (e) { let e; }");
        rejects("try {} catch ([e]) { var e; }");
        accepts("try {} catch (e) { for (var e of []) {} }");
        rejects("for (let x of []) { var x; }");
        rejects("let let = 1;");
        rejects("'use strict'; { function f() {} function f() {} }");
        rejects("{ function f() {} let f; }");
        accepts("{ function f() {} function f() {} }");
        accepts("var a; var a; function a() {}");
        accepts("try {} catch (e) { var e; }");
        accepts("function f(a) { var a; }");
        accepts("let a; { let a; }");
        accepts("function f() {} var f;");
        accepts("for (let i = 0; i < 1; i++) { let i; }");
    }

    #[test]
    fn parameters_and_strictness() {
        rejects("function f(a, a) { 'use strict'; }");
        rejects("(a, a) => 1");
        rejects("function f(a = 0) { 'use strict'; }");
        rejects("function eval() { 'use strict'; }");
        rejects("function f(static) { 'use strict'; }");
        rejects("function* g(a = yield) {}");
        rejects("async function f(a = await 1) {}");
        rejects("async (a = await => {}) => {}");
        rejects("({ m(a, a) {} })");
        rejects("'use strict'; 010");
        rejects("function f() { '\\01'; 'use strict'; }");
        rejects("0_1");
        rejects("08n");
        accepts("function f(a, a) {}");
        accepts("010; '\\01'");
    }

    #[test]
    fn statement_positions() {
        rejects("while (0) function f() {}");
        rejects("if (1) class C {}");
        rejects("if (1) const a = 1;");
        rejects("if (1) l: function f() {}");
        rejects("'use strict'; if (1) function f() {}");
        rejects("l: function* g() {}");
        rejects("if (1) let [a] = 1;");
        accepts("if (1) function f() {}");
        accepts("l: function f() {}");
        accepts("if (0) let\nx = 1");
    }

    #[test]
    fn classes_and_objects() {
        rejects("class C { #x; #x; }");
        rejects("class C { m() { this.#y; } }");
        rejects("class C { #x; m() { delete this.#x; } }");
        rejects("class C { #constructor() {} }");
        rejects("class C { constructor() {} constructor() {} }");
        rejects("class C { static prototype() {} }");
        rejects("class C { get constructor() {} }");
        rejects("class C { x = arguments; }");
        rejects("class C { static { await; } }");
        rejects("class C extends B { m() { super.#x; } #x; }");
        rejects("this.#x");
        rejects("({ __proto__: 1, __proto__: 2 })");
        rejects("({ a = 1 })");
        rejects("({a}) = 1");
        rejects("[(a = 1)] = []");
        rejects("new.target");
        rejects("() => new.target");
        accepts("class C { get #x() {} set #x(v) {} m() { return this.#x; } }");
        accepts("class C { #x; static m(o) { return #x in o; } }");
        accepts("class C { x = () => function () { return arguments; }; }");
        accepts("({ __proto__: 1, __proto__ })");
        accepts("({ __proto__: a, __proto__: b } = {})");
        accepts("(a) = 1; [(a)] = []; ({ a: (b) } = {});");
        accepts("function f() { return () => new.target; }");
    }

    #[test]
    fn modules() {
        let m = |s: &str| crate::parser::parse_program(s, true);
        assert!(m("export { x };").is_err());
        assert!(m("var x; export { x, x };").is_err());
        assert!(m("export default 1; export default 2;").is_err());
        assert!(m("function f() {} function f() {}").is_err());
        assert!(m("var x; export { x as y }; export { x };").is_ok());
        assert!(m("import { a } from 'm'; export { a };").is_ok());
    }
}
