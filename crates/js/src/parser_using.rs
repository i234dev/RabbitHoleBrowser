//! `using` / `await using` declarations (explicit resource management).
//!
//! The parser lowers them to plain statements: a statement list holding a
//! `using` declaration becomes
//!
//! ```text
//! const %using = %UsingNew();
//! try {
//!     ...                                  // the original statements, with
//!     const x = init; %UsingAdd(%using, x, hint);   // for `using x = init`
//! } catch (%usingErr) {
//!     %UsingError(%using, %usingErr);      // record the body's exception
//! } finally {
//!     // only with `await using`: await each async disposal in turn
//!     while (%UsingStep(%using)) {
//!         try { await %UsingValue(%using); } catch (%usingErr) { %UsingError(%using, %usingErr); }
//!     }
//!     %UsingDispose(%using);               // dispose the rest; throw the
//! }                                        // recorded (Suppressed)Error
//! ```
//!
//! The `%` names can't be written in source code: the cap is an ordinary
//! block-scoped `const` (nested blocks shadow it) and the `%Using*`
//! functions live in the realm's global lexical scope (see
//! `builtins::disposable`). The compiler and the VM need nothing new.

use super::*;

/// Callee marking a parsed `using` declarator's initializer until the
/// enclosing statement list is lowered: `%usingDecl(init, hint)`.
const MARK: &str = "%usingDecl";
const CAP: &str = "%using";
const ERR: &str = "%usingErr";
const FOR_VALUE: &str = "%usingValue";

/// Hint of a `using` declarator: 0 sync, 1 async (`await using`).
pub(super) const HINT_SYNC: f64 = 0.0;
pub(super) const HINT_ASYNC: f64 = 1.0;

fn ident(pos: Pos, name: &str) -> Expr {
    Expr { pos, kind: ExprKind::Identifier(Name::new(name)) }
}

fn call(pos: Pos, f: &str, args: Vec<Expr>) -> Expr {
    Expr {
        pos,
        kind: ExprKind::Call { callee: Box::new(ident(pos, f)), args: args.into_iter().map(ArrayElement::Expr).collect(), optional: false },
    }
}

/// `const %usingTmp = e;`: a runtime call as a declaration, which (unlike
/// an expression statement) leaves the completion value alone.
fn call_stmt(e: Expr) -> Stmt {
    const_decl(e.pos, Pattern::Identifier(Name::new("%usingTmp")), e)
}

fn expr_stmt(e: Expr) -> Stmt {
    Stmt { pos: e.pos, kind: StmtKind::Expr(e) }
}

fn const_decl(pos: Pos, target: Pattern, init: Expr) -> Stmt {
    Stmt { pos, kind: StmtKind::VarDecl(VarDecl { kind: VarKind::Const, declarations: vec![VarDeclarator { target, init: Some(init) }] }) }
}

/// The hint of a lowered-later `using` declaration statement.
pub(super) fn using_decl_hint(s: &Stmt) -> Option<f64> {
    if let StmtKind::VarDecl(VarDecl { kind: VarKind::Const, declarations }) = &s.kind {
        if let Some(VarDeclarator { init: Some(Expr { kind: ExprKind::Call { callee, args, .. }, .. }), .. }) = declarations.first() {
            if matches!(&callee.kind, ExprKind::Identifier(n) if n.as_str() == MARK) {
                if let Some(ArrayElement::Expr(Expr { kind: ExprKind::Number(h), .. })) = args.get(1) {
                    return Some(*h);
                }
            }
        }
    }
    None
}

fn catch_record(pos: Pos) -> Option<Box<(Option<Pattern>, Vec<Stmt>)>> {
    Some(Box::new((Some(Pattern::Identifier(Name::new(ERR))), vec![call_stmt(call(pos, "%UsingError", vec![ident(pos, CAP), ident(pos, ERR)]))])))
}

/// Wrap `inner` (which registers resources on `%using`) in the
/// capability declaration and the disposing try / finally.
fn wrap(pos: Pos, inner: Vec<Stmt>, is_async: bool) -> Vec<Stmt> {
    let mut fin = Vec::new();
    if is_async {
        let awaited = Expr { pos, kind: ExprKind::Await(Box::new(call(pos, "%UsingValue", vec![ident(pos, CAP)]))) };
        let step = Stmt { pos, kind: StmtKind::Try { block: vec![call_stmt(awaited)], handler: catch_record(pos), finalizer: None } };
        fin.push(Stmt {
            pos,
            kind: StmtKind::While { test: Box::new(call(pos, "%UsingStep", vec![ident(pos, CAP)])), body: Box::new(Stmt { pos, kind: StmtKind::Block(vec![step]) }) },
        });
    }
    fin.push(call_stmt(call(pos, "%UsingDispose", vec![ident(pos, CAP)])));
    vec![
        const_decl(pos, Pattern::Identifier(Name::new(CAP)), call(pos, "%UsingNew", vec![])),
        Stmt { pos, kind: StmtKind::Try { block: inner, handler: catch_record(pos), finalizer: Some(fin) } },
    ]
}

/// `const x = init; %UsingAdd(%using, x, hint);` for each declarator.
fn expand_decl(s: Stmt, out: &mut Vec<Stmt>) {
    let pos = s.pos;
    let decl = match s.kind {
        StmtKind::VarDecl(d) => d,
        _ => unreachable!(),
    };
    for d in decl.declarations {
        let (init, hint) = match d.init {
            Some(Expr { kind: ExprKind::Call { args, .. }, .. }) => {
                let mut it = args.into_iter();
                let init = match it.next() {
                    Some(ArrayElement::Expr(e)) => e,
                    _ => Expr { pos, kind: ExprKind::Undefined },
                };
                let hint = match it.next() {
                    Some(ArrayElement::Expr(e)) => e,
                    _ => Expr { pos, kind: ExprKind::Number(HINT_SYNC) },
                };
                (init, hint)
            }
            _ => continue,
        };
        let name = match &d.target {
            Pattern::Identifier(n) => n.clone(),
            _ => continue,
        };
        // `const x = init, %usingAdd_x = %UsingAdd(%using, x, hint);`: a
        // declaration, so the block's completion value stays untouched.
        let add = call(pos, "%UsingAdd", vec![ident(pos, CAP), Expr { pos, kind: ExprKind::Identifier(name.clone()) }, hint]);
        let reg = VarDeclarator { target: Pattern::Identifier(Name::new(&format!("%usingAdd_{}", name.as_str()))), init: Some(add) };
        out.push(Stmt {
            pos,
            kind: StmtKind::VarDecl(VarDecl { kind: VarKind::Const, declarations: vec![VarDeclarator { target: d.target, init: Some(init) }, reg] }),
        });
    }
}

/// Lower the `using` declarations of a statement list (no-op without any).
/// Leading directives stay in front when `directives` is set (function
/// bodies).
pub(super) fn lower_statement_list(body: Vec<Stmt>, directives: bool) -> Vec<Stmt> {
    if !body.iter().any(|s| using_decl_hint(s).is_some()) {
        return body;
    }
    let pos = body.first().map(|s| s.pos).unwrap_or_default();
    let mut out = Vec::new();
    let mut inner = Vec::with_capacity(body.len() + 4);
    let mut is_async = false;
    let mut prologue = directives;
    for s in body {
        if prologue {
            if matches!(&s.kind, StmtKind::Expr(Expr { kind: ExprKind::String(_), .. })) {
                out.push(s);
                continue;
            }
            prologue = false;
        }
        match using_decl_hint(&s) {
            Some(h) => {
                is_async |= h == HINT_ASYNC;
                expand_decl(s, &mut inner);
            }
            None => inner.push(s),
        }
    }
    out.extend(wrap(pos, inner, is_async));
    out
}

fn pattern_names(p: &Pattern, out: &mut Vec<Name>) {
    match p {
        Pattern::Identifier(n) => out.push(n.clone()),
        Pattern::Expr(_) => {}
        Pattern::Object { properties, rest } => {
            for prop in properties {
                pattern_names(&prop.value, out);
            }
            if let Some(r) = rest {
                pattern_names(r, out);
            }
        }
        Pattern::Array { elements, rest } => {
            for e in elements.iter().flatten() {
                pattern_names(e, out);
            }
            if let Some(r) = rest {
                pattern_names(r, out);
            }
        }
        Pattern::Assign { target, .. } => pattern_names(target, out),
    }
}

fn let_decl(pos: Pos, names: Vec<Name>) -> Stmt {
    Stmt {
        pos,
        kind: StmtKind::VarDecl(VarDecl { kind: VarKind::Let, declarations: names.into_iter().map(|n| VarDeclarator { target: Pattern::Identifier(n), init: None }).collect() }),
    }
}

fn assign_stmt(pos: Pos, target: Pattern, value: Expr) -> Stmt {
    expr_stmt(Expr { pos, kind: ExprKind::Assign { op: AssignOp::Assign, target: Box::new(target), value: Box::new(value) } })
}

/// Module top level with `using` and exports: the module's lexical
/// bindings must stay at the top level (exports refer to them), so they
/// become top-level `let`s (losing `const`-ness and the TDZ) assigned
/// inside the disposing `try`; imports, re-exports and function
/// declarations stay in front.
fn lower_module_with_exports(body: Vec<Stmt>) -> Vec<Stmt> {
    let pos = body.first().map(|s| s.pos).unwrap_or_default();
    let mut outer = Vec::new();
    let mut inner = Vec::new();
    let mut is_async = false;
    // Split a lexical declaration into top-level bindings + assignments.
    fn hoist(s: Stmt, exported: bool, outer: &mut Vec<Stmt>, inner: &mut Vec<Stmt>, is_async: &mut bool) {
        let pos = s.pos;
        if let Some(h) = using_decl_hint(&s) {
            *is_async |= h == HINT_ASYNC;
            let mut expanded = Vec::new();
            expand_decl(s, &mut expanded);
            for e in expanded {
                match e.kind {
                    StmtKind::VarDecl(d) => {
                        for v in d.declarations {
                            let mut names = Vec::new();
                            pattern_names(&v.target, &mut names);
                            push_binding(pos, names, exported, outer);
                            if let Some(init) = v.init {
                                inner.push(assign_stmt(pos, v.target, init));
                            }
                        }
                    }
                    _ => inner.push(e),
                }
            }
            return;
        }
        match s.kind {
            StmtKind::VarDecl(d) if d.kind != VarKind::Var => {
                for v in d.declarations {
                    let mut names = Vec::new();
                    pattern_names(&v.target, &mut names);
                    push_binding(pos, names, exported, outer);
                    let init = v.init.unwrap_or(Expr { pos, kind: ExprKind::Undefined });
                    inner.push(assign_stmt(pos, v.target, init));
                }
            }
            StmtKind::ClassDecl(c) if c.name.is_some() => {
                let name = c.name.clone().unwrap();
                push_binding(pos, vec![name.clone()], exported, outer);
                inner.push(assign_stmt(pos, Pattern::Identifier(name), Expr { pos, kind: ExprKind::Class(Box::new(c)) }));
            }
            kind => {
                let s = Stmt { pos, kind };
                if exported {
                    outer.push(Stmt { pos, kind: StmtKind::Export(ExportDecl::Decl(Box::new(s))) });
                } else if matches!(s.kind, StmtKind::FunctionDecl(_)) {
                    outer.push(s);
                } else {
                    inner.push(s);
                }
            }
        }
    }
    fn push_binding(pos: Pos, names: Vec<Name>, exported: bool, outer: &mut Vec<Stmt>) {
        let d = let_decl(pos, names);
        outer.push(if exported { Stmt { pos, kind: StmtKind::Export(ExportDecl::Decl(Box::new(d))) } } else { d });
    }
    for s in body {
        let pos = s.pos;
        match s.kind {
            StmtKind::Import(_) | StmtKind::Export(ExportDecl::Named { .. }) | StmtKind::Export(ExportDecl::All { .. }) | StmtKind::Export(ExportDecl::DefaultDecl(_)) => outer.push(s),
            StmtKind::Export(ExportDecl::Decl(d)) => hoist(*d, true, &mut outer, &mut inner, &mut is_async),
            StmtKind::Export(ExportDecl::DefaultExpr(e)) => {
                let name = Name::new("%default");
                outer.push(let_decl(pos, vec![name.clone()]));
                outer.push(Stmt { pos, kind: StmtKind::Export(ExportDecl::Named { specifiers: vec![("%default".to_string(), "default".to_string())], source: None }) });
                inner.push(assign_stmt(pos, Pattern::Identifier(name), e));
            }
            kind => hoist(Stmt { pos, kind }, false, &mut outer, &mut inner, &mut is_async),
        }
    }
    outer.extend(wrap(pos, inner, is_async));
    outer
}

impl Parser<'_> {
    /// Script / module top level: `using` is an early error in scripts; a
    /// module body is lowered when it exports nothing (imports move to the
    /// front). Otherwise the declarations stay marked and register nothing.
    pub(super) fn lower_top_level_using(&self, body: Vec<Stmt>) -> PResult<Vec<Stmt>> {
        let first = match body.iter().find(|s| using_decl_hint(s).is_some()) {
            Some(s) => s.pos,
            None => return Ok(body),
        };
        if !self.ctx.is_module {
            return Err(self.error_at(first, "using declarations are not allowed at the top level of a script"));
        }
        if body.iter().any(|s| matches!(s.kind, StmtKind::Export(_))) {
            return Ok(lower_module_with_exports(body));
        }
        let (mut out, inner): (Vec<Stmt>, Vec<Stmt>) = body.into_iter().partition(|s| matches!(s.kind, StmtKind::Import(_)));
        out.extend(lower_statement_list(inner, false));
        Ok(out)
    }

    fn await_allowed(&self) -> bool {
        self.ctx.in_async || (self.ctx.is_module && !self.ctx.in_function)
    }

    /// Whether the current tokens start a `using` (Some(HINT_SYNC)) or
    /// `await using` (Some(HINT_ASYNC)) declaration. `in_for`: in a
    /// for-statement head, where `for (using of ...)` is a for-of over the
    /// identifier `using`.
    pub(super) fn using_declaration_start(&mut self, in_for: bool) -> PResult<Option<f64>> {
        // Escaped keywords (`using`) never start a declaration.
        let hint = match &self.tok.kind {
            TokenKind::Ident(n) if n.as_str() == "using" && self.tok.end - self.tok.start == 5 => HINT_SYNC,
            TokenKind::Ident(n) if n.as_str() == "await" && self.tok.end - self.tok.start == 5 && self.await_allowed() => HINT_ASYNC,
            _ => return Ok(None),
        };
        let snap = self.snapshot();
        let r = self.using_declaration_lookahead(hint, in_for);
        self.restore(snap);
        Ok(if r? { Some(hint) } else { None })
    }

    fn using_declaration_lookahead(&mut self, hint: f64, in_for: bool) -> PResult<bool> {
        if hint == HINT_ASYNC {
            self.advance()?;
            let ok = matches!(&self.tok.kind, TokenKind::Ident(n) if n.as_str() == "using") && self.tok.end - self.tok.start == 5 && !self.tok.newline_before;
            if !ok {
                return Ok(false);
            }
        }
        self.advance()?; // using
        if self.tok.newline_before {
            return Ok(false);
        }
        let name = match &self.tok.kind {
            TokenKind::Ident(n) => n.clone(),
            _ => return Ok(false),
        };
        if name.as_str() == "in" || name.as_str() == "instanceof" {
            return Ok(false);
        }
        if in_for && name.as_str() == "of" {
            // `for (using of ...` is a for-of over the identifier `using`
            // (`for (using of of xs)` too); `for (await using of of xs)`
            // declares `of`.
            self.advance()?;
            if self.is_punct("=") || self.is_punct(";") {
                return Ok(true);
            }
            return Ok(hint == HINT_ASYNC && self.is_ident("of"));
        }
        Ok(true)
    }

    /// Parse a `using` / `await using` declaration (current token is
    /// `using` / `await`) into a marked `const` declaration; the enclosing
    /// statement list lowers it. `in_for_of_head`: no initializers (the
    /// caller checks for `of`).
    pub(super) fn parse_using_declaration(&mut self, hint: f64, in_for_head: bool) -> PResult<VarDecl> {
        if hint == HINT_ASYNC {
            self.advance()?; // await
        }
        self.advance()?; // using
        let mut declarations = Vec::new();
        loop {
            let pos = self.tok.pos;
            if self.is_punct("[") || self.is_punct("{") {
                return Err(self.error("using declarations may not contain binding patterns"));
            }
            let name = self.expect_binding_identifier()?;
            if name.as_str() == "await" && self.ctx.in_class_body && !self.ctx.in_function {
                return Err(self.error_at(pos, "'await' is not a valid identifier name in a class static block"));
            }
            if name.as_str() == "let" {
                return Err(self.error_at(pos, "let is disallowed as a lexically bound name"));
            }
            let init = if self.eat_punct("=")? {
                Some(self.parse_assignment()?)
            } else {
                if !in_for_head {
                    return Err(self.error("Missing initializer in using declaration"));
                }
                None
            };
            let init = init.map(|e| {
                let p = e.pos;
                call(p, MARK, vec![e, Expr { pos: p, kind: ExprKind::Number(hint) }])
            });
            declarations.push(VarDeclarator { target: Pattern::Identifier(name), init });
            if !self.eat_punct(",")? {
                break;
            }
        }
        Ok(VarDecl { kind: VarKind::Const, declarations })
    }

    /// `for ( using x ...` / `for ( await using x ...` — current token is
    /// `using` / `await`; `is_await` is `for await`.
    pub(super) fn parse_for_using(&mut self, hint: f64, is_await: bool) -> PResult<StmtKind> {
        let pos = self.tok.pos;
        let decl = self.with_no_in(true, |p| p.parse_using_declaration(hint, true))?;
        if self.is_ident("of") {
            if decl.declarations.len() != 1 {
                return Err(self.error("Invalid left-hand side in for-of loop: must have a single binding"));
            }
            let d = decl.declarations.into_iter().next().unwrap();
            if d.init.is_some() {
                return Err(self.error("for-of loop variable declaration may not have an initializer"));
            }
            let target = d.target;
            self.advance()?; // of
            let right = self.parse_assignment()?;
            self.expect_punct(")")?;
            let body = self.parse_loop_body()?;
            // for (const %usingValue of right) { using target = %usingValue; body }
            let value = Expr { pos, kind: ExprKind::Number(hint) };
            let marked = Stmt {
                pos,
                kind: StmtKind::VarDecl(VarDecl {
                    kind: VarKind::Const,
                    declarations: vec![VarDeclarator { target, init: Some(call(pos, MARK, vec![ident(pos, FOR_VALUE), value])) }],
                }),
            };
            let inner = lower_statement_list(vec![marked, body], false);
            return Ok(StmtKind::ForOf {
                left: Box::new(ForHead::VarDecl { kind: VarKind::Const, target: Pattern::Identifier(Name::new(FOR_VALUE)) }),
                right: Box::new(right),
                body: Box::new(Stmt { pos, kind: StmtKind::Block(inner) }),
                is_await,
            });
        }
        if self.is_ident("in") {
            return Err(self.error("using declarations are not allowed in for-in loops"));
        }
        if is_await {
            return Err(self.error("for await requires an of clause"));
        }
        for d in &decl.declarations {
            if d.init.is_none() {
                return Err(self.error("Missing initializer in using declaration"));
            }
        }
        // for (using x = a; test; update) body
        //   => { using x = a; for (; test; update) body }
        self.expect_punct(";")?;
        let test = if self.is_punct(";") { None } else { Some(self.parse_expression()?) };
        self.expect_punct(";")?;
        let update = if self.is_punct(")") { None } else { Some(self.parse_expression()?) };
        self.expect_punct(")")?;
        let body = Box::new(self.parse_loop_body()?);
        let lp = Stmt { pos, kind: StmtKind::For { init: None, test: test.map(Box::new), update: update.map(Box::new), body } };
        let decl = Stmt { pos, kind: StmtKind::VarDecl(decl) };
        Ok(StmtKind::Block(lower_statement_list(vec![decl, lp], false)))
    }
}
