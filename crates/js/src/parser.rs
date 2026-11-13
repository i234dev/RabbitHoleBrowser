//! ECMAScript 2022 parser (recursive descent with precedence climbing)
//! producing [`crate::ast`].
//!
//! The parser drives the lexer: it decides when a `/` starts a regular
//! expression and when a `}` continues a template literal, and it keeps
//! the context flags (function / generator / async / loop / switch /
//! strict) needed for early errors and automatic semicolon insertion.

use crate::ast::*;
use crate::lexer::{LexError, Lexer, LexerState, Token, TokenKind};

#[path = "parser_using.rs"]
mod using_decl;

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub line: u32,
    pub column: u32,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SyntaxError: {} ({}:{})", self.message, self.line, self.column)
    }
}

impl std::error::Error for ParseError {}

impl From<LexError> for ParseError {
    fn from(e: LexError) -> Self {
        ParseError {
            message: e.message,
            line: e.pos.line,
            column: e.pos.column,
        }
    }
}

type PResult<T> = Result<T, ParseError>;

/// Parse a script (or module when `is_module`).
pub fn parse_program(source: &str, is_module: bool) -> Result<Program, ParseError> {
    parse_program_in(source, is_module, &crate::early::EarlyContext::default())
}

/// Parse a program and check its early errors (see [`crate::early`]) in a
/// context: the default one for scripts and modules, or what direct eval
/// code inherits from its caller.
pub fn parse_program_in(source: &str, is_module: bool, ctx: &crate::early::EarlyContext) -> Result<Program, ParseError> {
    let program = parse_program_unchecked(source, is_module, ctx)?;
    crate::early::check_program(&program, ctx)?;
    Ok(program)
}

fn parse_program_unchecked(source: &str, is_module: bool, ctx: &crate::early::EarlyContext) -> Result<Program, ParseError> {
    let mut p = Parser::new(source, is_module)?;
    p.set_early_context(ctx);
    if source.len() < LAZY_MIN_SOURCE {
        p.lazy = false;
    }
    match p.parse_program() {
        Ok(program) => Ok(program),
        Err(_) if p.skipped => {
            // The pre-parser skipped function bodies: parse everything to
            // report the error exactly where the full grammar finds it.
            let mut p = Parser::new(source, is_module)?;
            p.set_early_context(ctx);
            p.lazy = false;
            p.parse_program()
        }
        Err(e) => Err(e),
    }
}

/// Lazy parsing of function bodies (on unless `RH_EAGER_PARSE` is set).
fn lazy_parse_enabled() -> bool {
    thread_local!(static LAZY: bool = std::env::var_os("RH_EAGER_PARSE").is_none());
    LAZY.with(|l| *l)
}

/// Bodies shorter than this (in bytes) are parsed right away: skipping
/// them saves little and parsing them later costs a parser setup.
const LAZY_MIN_BODY: usize = 96;

/// Scripts shorter than this (in bytes) are parsed completely. The
/// pre-parser only finds where a body ends, so a syntax error inside a
/// skipped body is reported when the function is first compiled instead of
/// rejecting the whole script up front as the specification requires. Small
/// scripts (inline scripts, eval code, `Function` bodies, tests) get the
/// exact behaviour for a negligible cost; large bundles, where skipping
/// bodies that never run saves real time, keep lazy parsing.
const LAZY_MIN_SOURCE: usize = 16 * 1024;

/// Parse the body of a function whose body the pre-parser skipped
/// ([`FunctionBody::Lazy`]): its statements and resulting strictness.
/// Functions nested in it are skipped in turn.
pub fn parse_lazy_body(f: &Function, lb: &LazyBody) -> Result<(Vec<Stmt>, bool), ParseError> {
    let mut lx = Lexer::new(&lb.source, lb.is_module);
    lx.restore(&crate::lexer::LexerState::at(lb.start, lb.line, lb.line_start));
    let tok = lx.next_token()?;
    let mut p = Parser {
        lx,
        tok,
        prev_end: lb.start,
        prev_line: lb.line,
        ctx: Context {
            in_function: true,
            in_generator: f.is_generator,
            in_async: f.is_async,
            in_loop: false,
            in_switch: false,
            strict: f.strict,
            is_module: lb.is_module,
            labels: Vec::new(),
            in_class_body: lb.in_class_body,
            super_call_ok: lb.super_call_ok,
            super_prop_ok: lb.super_prop_ok,
        },
        no_in: false,
        not_arrow: std::collections::HashSet::default(),
        lazy: lazy_parse_enabled(),
        skipped: false,
        lazy_src: Some(lb.source.clone()),
        parenthesized: Default::default(),
        rest_comma: Default::default(),
    };
    let r = p.parse_function_body_block()?;
    if p.prev_end != lb.end {
        return Err(ParseError { message: "Invalid or unexpected token (lazily parsed function body)".into(), line: lb.line, column: (lb.start - lb.line_start) as u32 + 1 });
    }
    crate::early::check_lazy_body(f, lb, &r.0, r.1)?;
    Ok(r)
}

/// Parse a standalone expression (used by the engine for `eval`-like
/// helpers and tests).
pub fn parse_expression(source: &str) -> Result<Expr, ParseError> {
    let mut p = Parser::new(source, false)?;
    p.lazy = false;
    let e = p.parse_expression()?;
    if p.tok.kind != TokenKind::Eof {
        return Err(p.unexpected());
    }
    Ok(e)
}

#[derive(Debug, Clone)]
struct Context {
    in_function: bool,
    in_generator: bool,
    in_async: bool,
    in_loop: bool,
    in_switch: bool,
    strict: bool,
    is_module: bool,
    /// Enclosing labels: (name, labels a loop statement)
    labels: Vec<(String, bool)>,
    /// Inside a class field initializer / static block (arguments banned, super allowed).
    in_class_body: bool,
    /// `super()` is allowed (derived constructor).
    super_call_ok: bool,
    /// `super.x` allowed (methods).
    super_prop_ok: bool,
}

struct Snapshot {
    lx: LexerState,
    tok: Token,
    prev_end: usize,
    prev_line: u32,
}

/// Result of a binary sub-parse, remembering whether the top node is an
/// unparenthesized logical operator (for the `??` mixing rule).
struct BinResult {
    expr: Expr,
    logical: Option<LogicalOp>,
}

pub struct Parser<'a> {
    lx: Lexer<'a>,
    tok: Token,
    prev_end: usize,
    prev_line: u32,
    ctx: Context,
    /// `in` is not a binary operator in a for-statement init.
    no_in: bool,
    /// Byte offsets of `(` tokens already known not to start arrow
    /// parameters. Without this, nested `(a=(b=(c=...` retries every level
    /// as an arrow head and parsing becomes exponential in the depth.
    not_arrow: std::collections::HashSet<usize, crate::value::FxBuild>,
    /// Skip function bodies with the pre-parser (lazy parsing).
    lazy: bool,
    /// Some body was skipped.
    skipped: bool,
    /// The source, shared by the skipped bodies (made on first use).
    lazy_src: Option<std::rc::Rc<str>>,
    /// Positions of object / array literals and assignments written in
    /// parentheses: not valid destructuring targets (`({a}) = b`).
    parenthesized: std::collections::HashSet<(u32, u32), crate::value::FxBuild>,
    /// Positions of spread arguments followed by a comma (`[...a, ]`): not
    /// valid rest elements.
    rest_comma: std::collections::HashSet<(u32, u32), crate::value::FxBuild>,
}

fn is_reserved_word(n: &str) -> bool {
    matches!(
        n,
        "break" | "case" | "catch" | "class" | "const" | "continue" | "debugger" | "default" | "delete" | "do" | "else" | "enum" | "export" | "extends" | "false" | "finally" | "for" | "function" | "if" | "import" | "in" | "instanceof" | "new" | "null" | "return" | "super" | "switch" | "this" | "throw" | "true" | "try" | "typeof" | "var" | "void" | "while" | "with"
    )
}

/// A string literal's source text contains a legacy octal escape (`\01`,
/// `\7`) or a non-octal decimal escape (`\8`, `\9`).
fn has_legacy_octal_escape(raw: &[u8]) -> bool {
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'\\' {
            match raw.get(i + 1) {
                Some(b'1'..=b'9') => return true,
                Some(b'0') if matches!(raw.get(i + 2), Some(b'0'..=b'9')) => return true,
                _ => {}
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    false
}

fn is_strict_reserved_word(n: &str) -> bool {
    matches!(n, "implements" | "interface" | "let" | "package" | "private" | "protected" | "public" | "static" | "yield")
}

fn binary_op(p: &str) -> Option<(BinaryOp, u8)> {
    Some(match p {
        "|" => (BinaryOp::BitOr, 3),
        "^" => (BinaryOp::BitXor, 4),
        "&" => (BinaryOp::BitAnd, 5),
        "==" => (BinaryOp::Eq, 6),
        "!=" => (BinaryOp::NotEq, 6),
        "===" => (BinaryOp::StrictEq, 6),
        "!==" => (BinaryOp::StrictNotEq, 6),
        "<" => (BinaryOp::Lt, 7),
        "<=" => (BinaryOp::LtEq, 7),
        ">" => (BinaryOp::Gt, 7),
        ">=" => (BinaryOp::GtEq, 7),
        "<<" => (BinaryOp::Shl, 8),
        ">>" => (BinaryOp::Shr, 8),
        ">>>" => (BinaryOp::UShr, 8),
        "+" => (BinaryOp::Add, 9),
        "-" => (BinaryOp::Sub, 9),
        "*" => (BinaryOp::Mul, 10),
        "/" => (BinaryOp::Div, 10),
        "%" => (BinaryOp::Mod, 10),
        _ => return None,
    })
}

fn assign_op(p: &str) -> Option<AssignOp> {
    Some(match p {
        "=" => AssignOp::Assign,
        "+=" => AssignOp::Add,
        "-=" => AssignOp::Sub,
        "*=" => AssignOp::Mul,
        "/=" => AssignOp::Div,
        "%=" => AssignOp::Mod,
        "**=" => AssignOp::Exp,
        "<<=" => AssignOp::Shl,
        ">>=" => AssignOp::Shr,
        ">>>=" => AssignOp::UShr,
        "&=" => AssignOp::BitAnd,
        "|=" => AssignOp::BitOr,
        "^=" => AssignOp::BitXor,
        "&&=" => AssignOp::And,
        "||=" => AssignOp::Or,
        "??=" => AssignOp::Nullish,
        _ => return None,
    })
}

/// The keyword identifier token `t` (spelling `n`) stands for, or "": a
/// keyword written with Unicode escapes (`var`) is not one.
fn keyword_token(t: &Token, n: &str) -> &'static str {
    if t.end - t.start != n.len() {
        return "";
    }
    keyword(n)
}

/// The keyword an identifier token spells (as a static string), or "".
/// Lets the parser dispatch on keywords without cloning token text.
fn keyword(n: &str) -> &'static str {
    match n {
        "var" => "var",
        "if" => "if",
        "for" => "for",
        "while" => "while",
        "do" => "do",
        "return" => "return",
        "break" => "break",
        "continue" => "continue",
        "throw" => "throw",
        "try" => "try",
        "switch" => "switch",
        "debugger" => "debugger",
        "with" => "with",
        "function" => "function",
        "class" => "class",
        "async" => "async",
        "const" => "const",
        "let" => "let",
        "import" => "import",
        "export" => "export",
        "this" => "this",
        "null" => "null",
        "true" => "true",
        "false" => "false",
        "new" => "new",
        _ => "",
    }
}

impl<'a> Parser<'a> {
    pub fn new(source: &'a str, is_module: bool) -> PResult<Parser<'a>> {
        let mut lx = Lexer::new(source, is_module);
        let tok = lx.next_token()?;
        Ok(Parser {
            lx,
            tok,
            prev_end: 0,
            prev_line: 1,
            ctx: Context {
                in_function: false,
                in_generator: false,
                in_async: is_module,
                in_loop: false,
                in_switch: false,
                strict: is_module,
                is_module,
                labels: Vec::new(),
                in_class_body: false,
                super_call_ok: false,
                super_prop_ok: false,
            },
            no_in: false,
            not_arrow: std::collections::HashSet::default(),
            lazy: lazy_parse_enabled(),
            skipped: false,
            lazy_src: None,
            parenthesized: Default::default(),
            rest_comma: Default::default(),
        })
    }

    /// Start in the context of eval code (see [`crate::early::EarlyContext`]).
    fn set_early_context(&mut self, ctx: &crate::early::EarlyContext) {
        self.ctx.strict |= ctx.strict;
        self.ctx.super_prop_ok = ctx.in_method;
        self.ctx.super_call_ok = ctx.in_derived_ctor;
        self.ctx.in_class_body = ctx.in_class_field_init;
    }

    // ------------------------------------------------------------ helpers

    fn advance(&mut self) -> PResult<Token> {
        let next = self.lx.next_token()?;
        self.prev_end = self.tok.end;
        self.prev_line = self.tok.pos.line;
        Ok(std::mem::replace(&mut self.tok, next))
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            lx: self.lx.state(),
            tok: self.tok.clone(),
            prev_end: self.prev_end,
            prev_line: self.prev_line,
        }
    }

    fn restore(&mut self, s: Snapshot) {
        self.lx.restore(&s.lx);
        self.tok = s.tok;
        self.prev_end = s.prev_end;
        self.prev_line = s.prev_line;
    }

    fn error_at(&self, pos: Pos, message: impl Into<String>) -> ParseError {
        ParseError {
            message: message.into(),
            line: pos.line,
            column: pos.column,
        }
    }

    fn error(&self, message: impl Into<String>) -> ParseError {
        self.error_at(self.tok.pos, message)
    }

    fn unexpected(&self) -> ParseError {
        let what = match &self.tok.kind {
            TokenKind::Eof => "end of input".to_string(),
            TokenKind::Ident(n) => format!("token '{n}'"),
            TokenKind::Punct(p) => format!("token '{p}'"),
            TokenKind::Str(_) => "string".to_string(),
            TokenKind::Number(_) | TokenKind::BigInt(_) => "number".to_string(),
            TokenKind::Template { .. } => "template string".to_string(),
            TokenKind::Regex { .. } => "regular expression".to_string(),
            TokenKind::PrivateName(n) => format!("private name '#{n}'"),
        };
        self.error(format!("Unexpected {what}"))
    }

    fn is_punct(&self, p: &str) -> bool {
        self.tok.is_punct(p)
    }

    fn eat_punct(&mut self, p: &str) -> PResult<bool> {
        if self.is_punct(p) {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn expect_punct(&mut self, p: &str) -> PResult<()> {
        if self.is_punct(p) {
            self.advance()?;
            Ok(())
        } else {
            Err(self.error(format!("Expected '{p}' but found {}", self.describe())))
        }
    }

    fn describe(&self) -> String {
        match &self.tok.kind {
            TokenKind::Eof => "end of input".into(),
            TokenKind::Ident(n) => format!("'{n}'"),
            TokenKind::Punct(p) => format!("'{p}'"),
            _ => "token".into(),
        }
    }

    fn is_ident(&self, name: &str) -> bool {
        self.tok.is_ident(name)
    }

    fn eat_ident(&mut self, name: &str) -> PResult<bool> {
        if self.is_ident(name) {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn expect_ident(&mut self, name: &str) -> PResult<()> {
        if self.eat_ident(name)? {
            Ok(())
        } else {
            Err(self.error(format!("Expected '{name}'")))
        }
    }

    fn is_reserved(&self, name: &str) -> bool {
        // Every reserved word is 2..=10 lowercase ASCII letters.
        let b = name.as_bytes();
        if b.len() < 2 || b.len() > 10 || !b[0].is_ascii_lowercase() {
            return false;
        }
        is_reserved_word(name)
            || (self.ctx.strict && is_strict_reserved_word(name))
            || (name == "yield" && (self.ctx.in_generator || self.ctx.strict))
            || (name == "await" && (self.ctx.in_async || self.ctx.is_module))
    }

    /// Current token as a binding identifier (not reserved).
    fn ident_name_if_binding(&self) -> Option<Name> {
        match &self.tok.kind {
            TokenKind::Ident(n) if !self.is_reserved(n) => Some(n.clone()),
            _ => None,
        }
    }

    fn expect_binding_identifier(&mut self) -> PResult<Name> {
        match self.ident_name_if_binding() {
            Some(n) => {
                if self.ctx.strict && (n == "eval" || n == "arguments") {
                    return Err(self.error(format!("Unexpected eval or arguments in strict mode")));
                }
                self.advance()?;
                Ok(n)
            }
            None => Err(self.error(format!("Expected identifier but found {}", self.describe()))),
        }
    }

    /// Any IdentifierName (keywords allowed), e.g. after `.`.
    fn expect_identifier_name(&mut self) -> PResult<Name> {
        match &self.tok.kind {
            TokenKind::Ident(n) => {
                let n = n.clone();
                self.advance()?;
                Ok(n)
            }
            _ => Err(self.error(format!("Expected property name but found {}", self.describe()))),
        }
    }

    /// Legacy octal and non-octal decimal literals (`010`, `08`) and the
    /// matching string escapes are not allowed in strict mode code.
    fn check_strict_literal(&self) -> PResult<()> {
        if !self.ctx.strict {
            return Ok(());
        }
        let raw = self.lx.source()[self.tok.start..self.tok.end].as_bytes();
        let bad = match &self.tok.kind {
            TokenKind::Number(_) | TokenKind::BigInt(_) => raw.len() > 1 && raw[0] == b'0' && raw[1].is_ascii_digit(),
            TokenKind::Str(_) => raw.contains(&b'\\') && has_legacy_octal_escape(raw),
            _ => false,
        };
        if bad {
            return Err(self.error("Octal literals and octal escape sequences are not allowed in strict mode"));
        }
        Ok(())
    }

    /// A string module export name must be well-formed Unicode: no lone
    /// surrogate escapes (`"\uD83D"`).
    fn check_module_export_name(&self) -> PResult<()> {
        let raw = self.lx.source()[self.tok.start..self.tok.end].as_bytes();
        if !raw.contains(&b'\\') {
            return Ok(());
        }
        // Code units written as escapes, in order (others as 0).
        let mut units: Vec<u32> = Vec::new();
        let mut i = 0;
        while i < raw.len() {
            if raw[i] == b'\\' && raw.get(i + 1) == Some(&b'u') {
                let (v, len) = if raw.get(i + 2) == Some(&b'{') {
                    let end = raw[i..].iter().position(|&b| b == b'}').map(|p| i + p).unwrap_or(raw.len());
                    (std::str::from_utf8(&raw[i + 3..end]).ok().and_then(|h| u32::from_str_radix(h, 16).ok()).unwrap_or(0), end + 1 - i)
                } else {
                    (std::str::from_utf8(raw.get(i + 2..i + 6).unwrap_or(b"")).ok().and_then(|h| u32::from_str_radix(h, 16).ok()).unwrap_or(0), 6)
                };
                units.push(v);
                i += len;
            } else {
                units.push(0);
                i += if raw[i] == b'\\' { 2 } else { 1 };
            }
        }
        let mut k = 0;
        while k < units.len() {
            match units[k] {
                0xD800..=0xDBFF if matches!(units.get(k + 1), Some(0xDC00..=0xDFFF)) => k += 2,
                0xD800..=0xDFFF => return Err(self.error("Module export names must be well-formed Unicode strings")),
                _ => k += 1,
            }
        }
        Ok(())
    }

    fn consume_semicolon(&mut self) -> PResult<()> {
        if self.eat_punct(";")? {
            return Ok(());
        }
        if self.is_punct("}") || self.tok.kind == TokenKind::Eof || self.tok.newline_before {
            return Ok(());
        }
        Err(self.unexpected())
    }

    fn with_ctx<T>(&mut self, ctx: Context, f: impl FnOnce(&mut Self) -> PResult<T>) -> PResult<T> {
        let saved = std::mem::replace(&mut self.ctx, ctx);
        let saved_no_in = std::mem::replace(&mut self.no_in, false);
        let r = f(self);
        self.ctx = saved;
        self.no_in = saved_no_in;
        r
    }

    fn with_no_in<T>(&mut self, no_in: bool, f: impl FnOnce(&mut Self) -> PResult<T>) -> PResult<T> {
        let saved = std::mem::replace(&mut self.no_in, no_in);
        let r = f(self);
        self.no_in = saved;
        r
    }

    // ------------------------------------------------------------ program

    pub fn parse_program(&mut self) -> PResult<Program> {
        let mut body = Vec::new();
        let strict = self.parse_directives(&mut body)?;
        if strict {
            self.ctx.strict = true;
        }
        while self.tok.kind != TokenKind::Eof {
            body.push(self.parse_statement_list_item()?);
        }
        let body = self.lower_top_level_using(body)?;
        Ok(Program {
            body,
            is_module: self.ctx.is_module,
            strict: self.ctx.strict,
        })
    }

    /// Parse the directive prologue; pushes the directive statements into
    /// `body` and returns whether `"use strict"` was seen.
    fn parse_directives(&mut self, body: &mut Vec<Stmt>) -> PResult<bool> {
        let mut strict = false;
        // A directive with an octal escape, before or after "use strict".
        let mut octal: Option<Pos> = None;
        while let TokenKind::Str(s) = &self.tok.kind {
            let raw = &self.lx.source()[self.tok.start..self.tok.end];
            let is_use_strict = raw == "\"use strict\"" || raw == "'use strict'";
            if octal.is_none() && has_legacy_octal_escape(raw.as_bytes()) {
                octal = Some(self.tok.pos);
            }
            let s = s.clone();
            // Only a directive if the statement is exactly the string literal.
            let snap = self.snapshot();
            let pos = self.tok.pos;
            self.advance()?;
            let is_directive = self.is_punct(";") || self.is_punct("}") || self.tok.kind == TokenKind::Eof || self.tok.newline_before;
            if !is_directive {
                self.restore(snap);
                break;
            }
            self.consume_semicolon()?;
            if is_use_strict {
                strict = true;
                self.ctx.strict = true;
            }
            body.push(Stmt {
                pos,
                kind: StmtKind::Expr(Expr {
                    pos,
                    kind: ExprKind::String(s),
                }),
            });
        }
        if let (true, Some(pos)) = (strict || self.ctx.strict, octal) {
            return Err(self.error_at(pos, "Octal escape sequences are not allowed in strict mode"));
        }
        Ok(strict)
    }

    // ---------------------------------------------------------- statements

    fn parse_statement_list_item(&mut self) -> PResult<Stmt> {
        let pos = self.tok.pos;
        if let Some(hint) = self.using_declaration_start(false)? {
            let d = self.parse_using_declaration(hint, false)?;
            self.consume_semicolon()?;
            return Ok(Stmt { pos, kind: StmtKind::VarDecl(d) });
        }
        match &self.tok.kind {
            TokenKind::Ident(n) => match keyword_token(&self.tok, n) {
                "function" => {
                    let f = self.parse_function(false, false, true)?;
                    return Ok(Stmt { pos, kind: StmtKind::FunctionDecl(std::rc::Rc::new(f)) });
                }
                "async" if self.peek_is_function_no_newline()? => {
                    self.advance()?;
                    let f = self.parse_function(true, false, true)?;
                    return Ok(Stmt { pos, kind: StmtKind::FunctionDecl(std::rc::Rc::new(f)) });
                }
                "class" => {
                    let c = self.parse_class(true)?;
                    return Ok(Stmt { pos, kind: StmtKind::ClassDecl(c) });
                }
                "const" => {
                    let d = self.parse_var_decl(VarKind::Const)?;
                    self.consume_semicolon()?;
                    return Ok(Stmt { pos, kind: StmtKind::VarDecl(d) });
                }
                "let" if self.let_is_declaration()? => {
                    let d = self.parse_var_decl(VarKind::Let)?;
                    self.consume_semicolon()?;
                    return Ok(Stmt { pos, kind: StmtKind::VarDecl(d) });
                }
                "import" if self.ctx.is_module && !self.peek_is_punct_any(&["(", "."])? => {
                    return self.parse_import();
                }
                "export" if self.ctx.is_module => {
                    return self.parse_export();
                }
                _ => {}
            },
            _ => {}
        }
        self.parse_statement()
    }

    fn peek_is_function_no_newline(&mut self) -> PResult<bool> {
        let snap = self.snapshot();
        self.advance()?;
        let r = self.is_ident("function") && !self.tok.newline_before;
        self.restore(snap);
        Ok(r)
    }

    fn peek_is_punct_any(&mut self, ps: &[&str]) -> PResult<bool> {
        let snap = self.snapshot();
        self.advance()?;
        let r = ps.iter().any(|p| self.is_punct(p));
        self.restore(snap);
        Ok(r)
    }

    /// `let` starts a declaration when followed by an identifier, `[` or `{`.
    fn let_is_declaration(&mut self) -> PResult<bool> {
        let snap = self.snapshot();
        self.advance()?;
        let r = match &self.tok.kind {
            TokenKind::Ident(n) => {
                // `let \n let` etc. are allowed; `let in` / `let instanceof` are not declarations.
                !(n == "in" || n == "instanceof") || self.ctx.strict
            }
            TokenKind::Punct("[") | TokenKind::Punct("{") => true,
            _ => false,
        };
        self.restore(snap);
        Ok(r)
    }

    fn parse_statement(&mut self) -> PResult<Stmt> {
        let pos = self.tok.pos;
        let kind = match &self.tok.kind {
            TokenKind::Punct("{") => StmtKind::Block(self.parse_block()?),
            TokenKind::Punct(";") => {
                self.advance()?;
                StmtKind::Empty
            }
            TokenKind::Ident(name) => match keyword_token(&self.tok, name) {
                "var" => {
                    let d = self.parse_var_decl(VarKind::Var)?;
                    self.consume_semicolon()?;
                    StmtKind::VarDecl(d)
                }
                "if" => self.parse_if()?,
                "for" => self.parse_for()?,
                "while" => {
                    self.advance()?;
                    self.expect_punct("(")?;
                    let test = self.parse_expression()?;
                    self.expect_punct(")")?;
                    let body = Box::new(self.parse_loop_body()?);
                    StmtKind::While { test: Box::new(test), body }
                }
                "do" => {
                    self.advance()?;
                    let body = Box::new(self.parse_loop_body()?);
                    self.expect_ident("while")?;
                    self.expect_punct("(")?;
                    let test = self.parse_expression()?;
                    self.expect_punct(")")?;
                    // Semicolon after do-while is optional.
                    self.eat_punct(";")?;
                    StmtKind::DoWhile { body, test: Box::new(test) }
                }
                "return" => {
                    if !self.ctx.in_function {
                        return Err(self.error("Illegal return statement"));
                    }
                    self.advance()?;
                    let arg = if self.is_punct(";") || self.is_punct("}") || self.tok.kind == TokenKind::Eof || self.tok.newline_before {
                        None
                    } else {
                        Some(self.parse_expression()?)
                    };
                    self.consume_semicolon()?;
                    StmtKind::Return(arg)
                }
                kw @ ("break" | "continue") => {
                    let is_break = kw == "break";
                    self.advance()?;
                    let label = if !self.tok.newline_before {
                        match &self.tok.kind {
                            TokenKind::Ident(l) if !is_reserved_word(l) => {
                                let l = l.clone();
                                self.advance()?;
                                Some(l)
                            }
                            _ => None,
                        }
                    } else {
                        None
                    };
                    match &label {
                        Some(l) => {
                            let found = self.ctx.labels.iter().find(|(n, _)| n == l);
                            match found {
                                None => return Err(self.error_at(pos, format!("Undefined label '{l}'"))),
                                Some((_, is_loop)) => {
                                    if !is_break && !is_loop {
                                        return Err(self.error_at(pos, format!("Illegal continue statement: '{l}' does not denote an iteration statement")));
                                    }
                                }
                            }
                        }
                        None => {
                            if is_break && !self.ctx.in_loop && !self.ctx.in_switch {
                                return Err(self.error_at(pos, "Illegal break statement"));
                            }
                            if !is_break && !self.ctx.in_loop {
                                return Err(self.error_at(pos, "Illegal continue statement: no surrounding iteration statement"));
                            }
                        }
                    }
                    self.consume_semicolon()?;
                    if is_break {
                        StmtKind::Break(label)
                    } else {
                        StmtKind::Continue(label)
                    }
                }
                "throw" => {
                    self.advance()?;
                    if self.tok.newline_before {
                        return Err(self.error("Illegal newline after throw"));
                    }
                    let e = self.parse_expression()?;
                    self.consume_semicolon()?;
                    StmtKind::Throw(e)
                }
                "try" => self.parse_try()?,
                "switch" => self.parse_switch()?,
                "debugger" => {
                    self.advance()?;
                    self.consume_semicolon()?;
                    StmtKind::Debugger
                }
                "with" => {
                    if self.ctx.strict {
                        return Err(self.error("Strict mode code may not include a with statement"));
                    }
                    self.advance()?;
                    self.expect_punct("(")?;
                    let object = self.parse_expression()?;
                    self.expect_punct(")")?;
                    let body = Box::new(self.parse_statement()?);
                    StmtKind::With { object: Box::new(object), body }
                }
                "function" => {
                    // Function declaration in statement position (sloppy mode allows it).
                    let f = self.parse_function(false, false, true)?;
                    StmtKind::FunctionDecl(std::rc::Rc::new(f))
                }
                "class" => {
                    let c = self.parse_class(true)?;
                    StmtKind::ClassDecl(c)
                }
                "async" if self.peek_is_function_no_newline()? => {
                    self.advance()?;
                    let f = self.parse_function(true, false, true)?;
                    StmtKind::FunctionDecl(std::rc::Rc::new(f))
                }
                "const" => {
                    let d = self.parse_var_decl(VarKind::Const)?;
                    self.consume_semicolon()?;
                    StmtKind::VarDecl(d)
                }
                "let" if self.let_is_declaration()? => {
                    // A single-statement position: `let` on its own line
                    // is an identifier (ASI), `let [` never starts an
                    // expression statement, anything else would be a
                    // declaration.
                    let snap = self.snapshot();
                    self.advance()?;
                    let asi = self.tok.newline_before && !self.is_punct("[");
                    self.restore(snap);
                    if !asi {
                        return Err(self.error("Lexical declaration cannot appear in a single-statement context"));
                    }
                    self.parse_expression_statement()?
                }
                "import" if self.ctx.is_module && !self.peek_is_punct_any(&["(", "."])? => return self.parse_import(),
                "export" if self.ctx.is_module => return self.parse_export(),
                _ => {
                    // Labeled statement? (cheap byte peek first)
                    let maybe_label = matches!(self.lx.next_significant_byte(), Some(b':') | None);
                    if maybe_label {
                        let name = match &self.tok.kind {
                            TokenKind::Ident(n) => n.clone(),
                            _ => unreachable!(),
                        };
                        if !self.is_reserved(&name) && self.peek_is_punct_any(&[":"])? {
                            self.advance()?;
                            self.advance()?; // ':'
                            return self.parse_labeled(pos, name);
                        }
                    }
                    self.parse_expression_statement()?
                }
            },
            _ => self.parse_expression_statement()?,
        };
        Ok(Stmt { pos, kind })
    }

    fn parse_expression_statement(&mut self) -> PResult<StmtKind> {
        let e = self.parse_expression()?;
        self.consume_semicolon()?;
        Ok(StmtKind::Expr(e))
    }

    fn parse_labeled(&mut self, pos: Pos, label: Name) -> PResult<Stmt> {
        let label_s = label.to_string();
        if self.ctx.labels.iter().any(|(n, _)| *n == label) {
            return Err(self.error_at(pos, format!("Label '{label}' has already been declared")));
        }
        let is_loop = self.is_ident("for") || self.is_ident("while") || self.is_ident("do");
        self.ctx.labels.push((label_s, is_loop));
        let body = self.parse_statement();
        self.ctx.labels.pop();
        let body = Box::new(body?);
        Ok(Stmt {
            pos,
            kind: StmtKind::Labeled { label, body },
        })
    }

    fn parse_block(&mut self) -> PResult<Vec<Stmt>> {
        self.expect_punct("{")?;
        let mut body = Vec::new();
        while !self.is_punct("}") {
            if self.tok.kind == TokenKind::Eof {
                return Err(self.error("Unexpected end of input"));
            }
            body.push(self.parse_statement_list_item()?);
        }
        self.advance()?;
        self.lower_using_list(body, false)
    }

    fn parse_loop_body(&mut self) -> PResult<Stmt> {
        let saved = self.ctx.in_loop;
        self.ctx.in_loop = true;
        let r = self.parse_statement();
        self.ctx.in_loop = saved;
        r
    }

    fn parse_var_decl(&mut self, kind: VarKind) -> PResult<VarDecl> {
        self.advance()?; // var / let / const
        let mut declarations = Vec::new();
        loop {
            let target = self.parse_binding_target()?;
            let init = if self.eat_punct("=")? {
                Some(self.parse_assignment()?)
            } else {
                None
            };
            if init.is_none() && !self.no_in {
                if kind == VarKind::Const {
                    return Err(self.error("Missing initializer in const declaration"));
                }
                if !matches!(target, Pattern::Identifier(_)) {
                    return Err(self.error("Missing initializer in destructuring declaration"));
                }
            }
            declarations.push(VarDeclarator { target, init });
            if !self.eat_punct(",")? {
                break;
            }
        }
        Ok(VarDecl { kind, declarations })
    }

    fn parse_if(&mut self) -> PResult<StmtKind> {
        self.advance()?;
        self.expect_punct("(")?;
        let test = self.parse_expression()?;
        self.expect_punct(")")?;
        let consequent = Box::new(self.parse_statement()?);
        let alternate = if self.eat_ident("else")? {
            Some(Box::new(self.parse_statement()?))
        } else {
            None
        };
        Ok(StmtKind::If {
            test: Box::new(test),
            consequent,
            alternate,
        })
    }

    fn parse_for(&mut self) -> PResult<StmtKind> {
        self.advance()?; // for
        let is_await = if self.is_ident("await") {
            if !self.ctx.in_async {
                return Err(self.error("for await is only valid in async functions"));
            }
            self.advance()?;
            true
        } else {
            false
        };
        self.expect_punct("(")?;
        if let Some(hint) = self.using_declaration_start(true)? {
            return self.parse_for_using(hint, is_await);
        }
        let mut init: Option<ForInit> = None;
        if self.is_punct(";") {
            // no init
        } else {
            let is_decl = self.is_ident("var")
                || self.is_ident("const")
                || (self.is_ident("let") && self.let_is_declaration()?);
            if is_decl {
                let kind = match self.tok.kind {
                    TokenKind::Ident(ref n) if n == "var" => VarKind::Var,
                    TokenKind::Ident(ref n) if n == "let" => VarKind::Let,
                    _ => VarKind::Const,
                };
                let decl = self.with_no_in(true, |p| p.parse_var_decl(kind))?;
                if self.is_ident("of") || self.is_ident("in") {
                    let is_of = self.is_ident("of");
                    if decl.declarations.len() != 1 {
                        return Err(self.error("Invalid left-hand side in for-in/of loop: must have a single binding"));
                    }
                    let d = decl.declarations.into_iter().next().unwrap();
                    if d.init.is_some() && (is_of || self.ctx.strict || kind != VarKind::Var || !matches!(d.target, Pattern::Identifier(_))) {
                        return Err(self.error("for-in/of loop variable declaration may not have an initializer"));
                    }
                    let head = ForHead::VarDecl { kind, target: d.target };
                    return self.parse_for_in_of(head, is_of, is_await);
                }
                // Validate missing initializers now that we know it's a plain for.
                for d in &decl.declarations {
                    if d.init.is_none() && (kind == VarKind::Const || !matches!(d.target, Pattern::Identifier(_))) {
                        return Err(self.error("Missing initializer in declaration"));
                    }
                }
                init = Some(ForInit::VarDecl(decl));
            } else {
                let start_pos = self.tok.pos;
                let starts_with_let = self.is_ident("let");
                let expr = self.with_no_in(true, |p| p.parse_expression())?;
                if self.is_ident("of") || self.is_ident("in") {
                    let is_of = self.is_ident("of");
                    if is_of && starts_with_let {
                        return Err(self.error_at(start_pos, "The left-hand side of a for-of loop may not be 'let'"));
                    }
                    if is_of && matches!(expr.kind, ExprKind::Identifier(ref n) if n == "async") && !is_await {
                        // `for (async of ...)` is a syntax error per spec.
                        return Err(self.error_at(start_pos, "The left-hand side of a for-of loop may not be 'async'"));
                    }
                    if matches!(expr.kind, ExprKind::Assign { .. }) {
                        return Err(self.error_at(start_pos, "Invalid left-hand side in for-in/of loop"));
                    }
                    let pattern = self.expr_to_pattern(expr, false)?;
                    return self.parse_for_in_of(ForHead::Pattern(pattern), is_of, is_await);
                }
                init = Some(ForInit::Expr(expr));
            }
        }
        if is_await {
            return Err(self.error("for await requires an of clause"));
        }
        self.expect_punct(";")?;
        let test = if self.is_punct(";") { None } else { Some(self.parse_expression()?) };
        self.expect_punct(";")?;
        let update = if self.is_punct(")") { None } else { Some(self.parse_expression()?) };
        self.expect_punct(")")?;
        let body = Box::new(self.parse_loop_body()?);
        Ok(StmtKind::For {
            init: init.map(Box::new),
            test: test.map(Box::new),
            update: update.map(Box::new),
            body,
        })
    }

    fn parse_for_in_of(&mut self, left: ForHead, is_of: bool, is_await: bool) -> PResult<StmtKind> {
        self.advance()?; // of / in
        let right = if is_of { self.parse_assignment()? } else { self.parse_expression()? };
        self.expect_punct(")")?;
        let body = Box::new(self.parse_loop_body()?);
        Ok(if is_of {
            StmtKind::ForOf {
                left: Box::new(left),
                right: Box::new(right),
                body,
                is_await,
            }
        } else {
            StmtKind::ForIn { left: Box::new(left), right: Box::new(right), body }
        })
    }

    fn parse_try(&mut self) -> PResult<StmtKind> {
        self.advance()?;
        let block = self.parse_block()?;
        let mut handler = None;
        if self.eat_ident("catch")? {
            let param = if self.eat_punct("(")? {
                let p = self.parse_binding_target()?;
                self.expect_punct(")")?;
                Some(p)
            } else {
                None
            };
            let body = self.parse_block()?;
            handler = Some((param, body));
        }
        let finalizer = if self.eat_ident("finally")? { Some(self.parse_block()?) } else { None };
        if handler.is_none() && finalizer.is_none() {
            return Err(self.error("Missing catch or finally after try"));
        }
        Ok(StmtKind::Try {
            block,
            handler: handler.map(Box::new),
            finalizer,
        })
    }

    fn parse_switch(&mut self) -> PResult<StmtKind> {
        self.advance()?;
        self.expect_punct("(")?;
        let discriminant = self.parse_expression()?;
        self.expect_punct(")")?;
        self.expect_punct("{")?;
        let saved = self.ctx.in_switch;
        self.ctx.in_switch = true;
        let mut cases = Vec::new();
        let mut seen_default = false;
        let result = (|| -> PResult<()> {
            while !self.is_punct("}") {
                let test = if self.eat_ident("case")? {
                    Some(self.parse_expression()?)
                } else if self.eat_ident("default")? {
                    if seen_default {
                        return Err(self.error("More than one default clause in switch statement"));
                    }
                    seen_default = true;
                    None
                } else {
                    return Err(self.unexpected());
                };
                self.expect_punct(":")?;
                let mut body = Vec::new();
                while !self.is_ident("case") && !self.is_ident("default") && !self.is_punct("}") {
                    if self.tok.kind == TokenKind::Eof {
                        return Err(self.error("Unexpected end of input"));
                    }
                    let item = self.parse_statement_list_item()?;
                    if using_decl::using_decl_hint(&item).is_some() {
                        return Err(self.error_at(item.pos, "using declarations are not allowed directly in switch case clauses"));
                    }
                    body.push(item);
                }
                cases.push(SwitchCase { test, body });
            }
            Ok(())
        })();
        self.ctx.in_switch = saved;
        result?;
        self.advance()?; // }
        Ok(StmtKind::Switch { discriminant: Box::new(discriminant), cases })
    }

    // ------------------------------------------------------- import/export

    fn parse_import(&mut self) -> PResult<Stmt> {
        let pos = self.tok.pos;
        self.advance()?; // import
        let mut decl = ImportDecl {
            source: String::new(),
            default: None,
            namespace: None,
            named: Vec::new(),
        };
        if let TokenKind::Str(s) = &self.tok.kind {
            decl.source = s.clone();
            self.advance()?;
            self.consume_semicolon()?;
            return Ok(Stmt { pos, kind: StmtKind::Import(decl) });
        }
        if let Some(name) = self.ident_name_if_binding() {
            self.advance()?;
            decl.default = Some(name.to_string());
            if !self.eat_punct(",")? {
                return self.finish_import(pos, decl);
            }
        }
        if self.eat_punct("*")? {
            self.expect_ident("as")?;
            decl.namespace = Some(self.expect_binding_identifier()?.to_string());
        } else if self.eat_punct("{")? {
            while !self.is_punct("}") {
                let imported = match &self.tok.kind {
                    TokenKind::Str(s) => {
                        let s = s.clone();
                        self.check_module_export_name()?;
                        self.advance()?;
                        s
                    }
                    _ => self.expect_identifier_name()?.to_string(),
                };
                let local = if self.eat_ident("as")? {
                    self.expect_binding_identifier()?.to_string()
                } else {
                    if self.is_reserved(&imported) {
                        return Err(self.error(format!("Unexpected reserved word '{imported}'")));
                    }
                    imported.clone()
                };
                decl.named.push((imported, local));
                if !self.eat_punct(",")? {
                    break;
                }
            }
            self.expect_punct("}")?;
        } else {
            return Err(self.unexpected());
        }
        self.finish_import(pos, decl)
    }

    fn finish_import(&mut self, pos: Pos, mut decl: ImportDecl) -> PResult<Stmt> {
        self.expect_ident("from")?;
        match &self.tok.kind {
            TokenKind::Str(s) => {
                decl.source = s.clone();
                self.advance()?;
            }
            _ => return Err(self.error("Expected module specifier string")),
        }
        self.skip_import_attributes()?;
        self.consume_semicolon()?;
        Ok(Stmt { pos, kind: StmtKind::Import(decl) })
    }

    /// `with { type: "json" }` / legacy `assert { ... }` — parsed and ignored.
    fn skip_import_attributes(&mut self) -> PResult<()> {
        if (self.is_ident("with") || self.is_ident("assert")) && !self.tok.newline_before {
            self.advance()?;
            self.expect_punct("{")?;
            // `key: "value"` entries; duplicate keys are an early error.
            let mut keys: Vec<String> = Vec::new();
            while !self.is_punct("}") {
                let key = match &self.tok.kind {
                    TokenKind::Ident(n) => n.to_string(),
                    TokenKind::Str(s) => s.clone(),
                    _ => return Err(self.unexpected()),
                };
                if keys.contains(&key) {
                    return Err(self.error(format!("Duplicate import attribute '{key}'")));
                }
                keys.push(key);
                self.advance()?;
                self.expect_punct(":")?;
                if !matches!(self.tok.kind, TokenKind::Str(_)) {
                    return Err(self.error("Import attribute values must be strings"));
                }
                self.advance()?;
                if !self.eat_punct(",")? {
                    break;
                }
            }
            self.expect_punct("}")?;
        }
        Ok(())
    }

    fn parse_export(&mut self) -> PResult<Stmt> {
        let pos = self.tok.pos;
        self.advance()?; // export
        let kind = if self.eat_ident("default")? {
            let dpos = self.tok.pos;
            if self.is_ident("function") || (self.is_ident("async") && self.peek_is_function_no_newline()?) {
                let is_async = self.eat_ident("async")?;
                let f = self.parse_function(is_async, false, false)?;
                StmtKind::Export(ExportDecl::DefaultDecl(Box::new(Stmt {
                    pos: dpos,
                    kind: StmtKind::FunctionDecl(std::rc::Rc::new(f)),
                })))
            } else if self.is_ident("class") {
                let c = self.parse_class(false)?;
                StmtKind::Export(ExportDecl::DefaultDecl(Box::new(Stmt {
                    pos: dpos,
                    kind: StmtKind::ClassDecl(c),
                })))
            } else {
                let e = self.parse_assignment()?;
                self.consume_semicolon()?;
                StmtKind::Export(ExportDecl::DefaultExpr(e))
            }
        } else if self.eat_punct("*")? {
            let namespace = if self.eat_ident("as")? { Some(self.expect_identifier_name()?.to_string()) } else { None };
            self.expect_ident("from")?;
            let source = match &self.tok.kind {
                TokenKind::Str(s) => s.clone(),
                _ => return Err(self.error("Expected module specifier string")),
            };
            self.advance()?;
            self.skip_import_attributes()?;
            self.consume_semicolon()?;
            StmtKind::Export(ExportDecl::All { source, namespace })
        } else if self.eat_punct("{")? {
            let mut specifiers = Vec::new();
            // A string local name needs `from` (`export { "a" }` is an error).
            let mut string_local: Option<Pos> = None;
            while !self.is_punct("}") {
                let local = match &self.tok.kind {
                    TokenKind::Str(s) => {
                        let s = s.clone();
                        string_local = string_local.or(Some(self.tok.pos));
                        self.check_module_export_name()?;
                        self.advance()?;
                        s
                    }
                    _ => self.expect_identifier_name()?.to_string(),
                };
                let exported = if self.eat_ident("as")? {
                    match &self.tok.kind {
                        TokenKind::Str(s) => {
                            let s = s.clone();
                            self.check_module_export_name()?;
                            self.advance()?;
                            s
                        }
                        _ => self.expect_identifier_name()?.to_string(),
                    }
                } else {
                    local.clone()
                };
                specifiers.push((local, exported));
                if !self.eat_punct(",")? {
                    break;
                }
            }
            self.expect_punct("}")?;
            let source = if self.eat_ident("from")? {
                match &self.tok.kind {
                    TokenKind::Str(s) => {
                        let s = s.clone();
                        self.advance()?;
                        self.skip_import_attributes()?;
                        Some(s)
                    }
                    _ => return Err(self.error("Expected module specifier string")),
                }
            } else {
                None
            };
            if let (None, Some(p)) = (&source, string_local) {
                return Err(self.error_at(p, "A string export name needs a 'from' clause"));
            }
            self.consume_semicolon()?;
            StmtKind::Export(ExportDecl::Named { specifiers, source })
        } else {
            let decl = self.parse_statement_list_item()?;
            if using_decl::using_decl_hint(&decl).is_some() {
                return Err(self.error_at(decl.pos, "using declarations cannot be exported"));
            }
            match decl.kind {
                StmtKind::VarDecl(_) | StmtKind::FunctionDecl(_) | StmtKind::ClassDecl(_) => {}
                _ => return Err(self.error_at(decl.pos, "Unexpected token after export")),
            }
            StmtKind::Export(ExportDecl::Decl(Box::new(decl)))
        };
        Ok(Stmt { pos, kind })
    }

    // ----------------------------------------------------------- patterns

    /// Binding target: identifier, array pattern or object pattern.
    fn parse_binding_target(&mut self) -> PResult<Pattern> {
        match &self.tok.kind {
            TokenKind::Punct("[") => self.parse_array_binding_pattern(),
            TokenKind::Punct("{") => self.parse_object_binding_pattern(),
            _ => Ok(Pattern::Identifier(self.expect_binding_identifier()?)),
        }
    }

    /// Binding element: target with an optional `= default`.
    fn parse_binding_element(&mut self) -> PResult<Pattern> {
        let target = self.parse_binding_target()?;
        if self.eat_punct("=")? {
            let default = self.with_no_in(false, |p| p.parse_assignment())?;
            return Ok(Pattern::Assign {
                target: Box::new(target),
                default: Box::new(default),
            });
        }
        Ok(target)
    }

    fn parse_array_binding_pattern(&mut self) -> PResult<Pattern> {
        self.expect_punct("[")?;
        let mut elements = Vec::new();
        let mut rest = None;
        loop {
            if self.eat_punct("]")? {
                break;
            }
            if self.is_punct(",") {
                self.advance()?;
                elements.push(None);
                continue;
            }
            if self.eat_punct("...")? {
                rest = Some(Box::new(self.parse_binding_target()?));
                self.eat_punct(",")?;
                self.expect_punct("]")?;
                break;
            }
            elements.push(Some(self.parse_binding_element()?));
            if !self.is_punct("]") {
                self.expect_punct(",")?;
            }
        }
        Ok(Pattern::Array { elements, rest })
    }

    fn parse_object_binding_pattern(&mut self) -> PResult<Pattern> {
        self.expect_punct("{")?;
        let mut properties = Vec::new();
        let mut rest = None;
        loop {
            if self.eat_punct("}")? {
                break;
            }
            if self.eat_punct("...")? {
                rest = Some(Box::new(Pattern::Identifier(self.expect_binding_identifier()?)));
                self.eat_punct(",")?;
                self.expect_punct("}")?;
                break;
            }
            let key_pos = self.tok.pos;
            let (key, ident_name) = self.parse_property_key()?;
            if matches!(key, PropertyKey::Private(_)) {
                return Err(self.error_at(key_pos, "Unexpected private name in binding pattern"));
            }
            if self.eat_punct(":")? {
                let value = self.parse_binding_element()?;
                properties.push(ObjectPatternProp {
                    key,
                    value,
                    shorthand: false,
                });
            } else {
                let name = match ident_name {
                    Some(n) if !self.is_reserved(&n) => n,
                    _ => return Err(self.error_at(key_pos, "Invalid shorthand property in binding pattern")),
                };
                let mut value = Pattern::Identifier(name);
                if self.eat_punct("=")? {
                    let default = self.with_no_in(false, |p| p.parse_assignment())?;
                    value = Pattern::Assign {
                        target: Box::new(value),
                        default: Box::new(default),
                    };
                }
                properties.push(ObjectPatternProp {
                    key,
                    value,
                    shorthand: true,
                });
            }
            if !self.is_punct("}") {
                self.expect_punct(",")?;
            }
        }
        Ok(Pattern::Object { properties, rest })
    }

    /// Convert an expression (parsed as a cover grammar) into an assignment
    /// pattern. `binding` requires identifiers only (arrow parameters).
    fn expr_to_pattern(&self, expr: Expr, binding: bool) -> PResult<Pattern> {
        let pos = expr.pos;
        if matches!(expr.kind, ExprKind::Object(_) | ExprKind::Array(_) | ExprKind::Assign { .. }) && self.parenthesized.contains(&(pos.line, pos.column)) {
            return Err(self.error_at(pos, "Invalid destructuring assignment target"));
        }
        match expr.kind {
            ExprKind::Identifier(name) => {
                if self.ctx.strict && (name == "eval" || name == "arguments") {
                    return Err(self.error_at(pos, "Invalid assignment target in strict mode"));
                }
                Ok(Pattern::Identifier(name))
            }
            ExprKind::Member { optional: false, .. } if !binding => Ok(Pattern::Expr(Box::new(expr))),
            ExprKind::Array(elements) => {
                let mut out = Vec::new();
                let mut rest = None;
                let n = elements.len();
                for (i, el) in elements.into_iter().enumerate() {
                    match el {
                        ArrayElement::Hole => out.push(None),
                        ArrayElement::Expr(e) => out.push(Some(self.expr_to_pattern(e, binding)?)),
                        ArrayElement::Spread(e) => {
                            if i != n - 1 || self.rest_comma.contains(&(e.pos.line, e.pos.column)) {
                                return Err(self.error_at(e.pos, "Rest element must be last element"));
                            }
                            let epos = e.pos;
                            let r = self.expr_to_pattern(e, binding)?;
                            if matches!(r, Pattern::Assign { .. }) {
                                return Err(self.error_at(epos, "Invalid rest element"));
                            }
                            rest = Some(Box::new(r));
                        }
                    }
                }
                Ok(Pattern::Array { elements: out, rest })
            }
            ExprKind::Object(props) => {
                let mut properties = Vec::new();
                let mut rest = None;
                let n = props.len();
                for (i, p) in props.into_iter().enumerate() {
                    match p {
                        ObjectProperty::Spread(e) => {
                            if i != n - 1 || self.rest_comma.contains(&(e.pos.line, e.pos.column)) {
                                return Err(self.error_at(e.pos, "Rest element must be last element"));
                            }
                            let epos = e.pos;
                            let r = self.expr_to_pattern(e, binding)?;
                            if !matches!(r, Pattern::Identifier(_) | Pattern::Expr(_)) {
                                return Err(self.error_at(epos, "Invalid rest element"));
                            }
                            rest = Some(Box::new(r));
                        }
                        ObjectProperty::Property { key, value, shorthand } => {
                            let value = match value {
                                PropertyValue::Init(e) => self.expr_to_pattern(e, binding)?,
                                _ => return Err(self.error_at(pos, "Invalid destructuring assignment target")),
                            };
                            properties.push(ObjectPatternProp { key, value, shorthand });
                        }
                    }
                }
                Ok(Pattern::Object { properties, rest })
            }
            ExprKind::Assign {
                op: AssignOp::Assign,
                target,
                value,
            } => Ok(Pattern::Assign {
                target,
                default: value,
            }),
            _ => Err(self.error_at(pos, "Invalid assignment target")),
        }
    }

    // --------------------------------------------------------- expressions

    pub fn parse_expression(&mut self) -> PResult<Expr> {
        let pos = self.tok.pos;
        let first = self.parse_assignment()?;
        if !self.is_punct(",") {
            return Ok(first);
        }
        let mut exprs = vec![first];
        while self.eat_punct(",")? {
            exprs.push(self.parse_assignment()?);
        }
        Ok(Expr {
            pos,
            kind: ExprKind::Sequence(exprs),
        })
    }

    pub fn parse_assignment(&mut self) -> PResult<Expr> {
        let pos = self.tok.pos;
        // yield
        if self.is_ident("yield") && self.ctx.in_generator {
            return self.parse_yield();
        }
        // Arrow functions
        if let Some(arrow) = self.try_parse_arrow()? {
            return Ok(arrow);
        }
        let start_pos = self.tok.pos;
        let left = self.parse_conditional()?;
        let op = match &self.tok.kind {
            TokenKind::Punct(p) => assign_op(p),
            _ => None,
        };
        let Some(op) = op else {
            return Ok(left);
        };
        let target = if op == AssignOp::Assign {
            if matches!(left.kind, ExprKind::Assign { .. }) {
                return Err(self.error_at(start_pos, "Invalid left-hand side in assignment"));
            }
            self.expr_to_pattern(left, false)?
        } else {
            match left.kind {
                ExprKind::Identifier(_) | ExprKind::Member { optional: false, .. } => self.expr_to_pattern(left, false)?,
                _ => return Err(self.error_at(start_pos, "Invalid left-hand side in assignment")),
            }
        };
        self.advance()?;
        let value = Box::new(self.parse_assignment()?);
        Ok(Expr {
            pos,
            kind: ExprKind::Assign { op, target: Box::new(target), value },
        })
    }

    fn parse_yield(&mut self) -> PResult<Expr> {
        let pos = self.tok.pos;
        self.advance()?;
        let mut delegate = false;
        let mut argument = None;
        if !self.tok.newline_before {
            if self.eat_punct("*")? {
                delegate = true;
                argument = Some(Box::new(self.parse_assignment()?));
            } else if !matches!(
                self.tok.kind,
                TokenKind::Punct(")") | TokenKind::Punct("]") | TokenKind::Punct("}") | TokenKind::Punct(",") | TokenKind::Punct(";") | TokenKind::Punct(":") | TokenKind::Eof
            ) && !self.is_ident("in")
                && !self.is_ident("of")
                && !self.is_punct("?")
            {
                argument = Some(Box::new(self.parse_assignment()?));
            }
        }
        Ok(Expr {
            pos,
            kind: ExprKind::Yield { argument, delegate },
        })
    }

    /// Detect and parse an arrow function at the current position.
    fn try_parse_arrow(&mut self) -> PResult<Option<Expr>> {
        let pos = self.tok.pos;
        let is_async;
        match &self.tok.kind {
            TokenKind::Ident(n) if n == "async" && self.tok.is_ident("async") => {
                let snap = self.snapshot();
                self.advance()?;
                if self.tok.newline_before {
                    self.restore(snap);
                    return Ok(None);
                }
                // async x => ...
                if let Some(name) = self.ident_name_if_binding_async() {
                    let snap2 = self.snapshot();
                    self.advance()?;
                    if self.is_punct("=>") && !self.tok.newline_before {
                        self.advance()?;
                        let params = vec![Param {
                            pattern: Pattern::Identifier(name),
                            rest: false,
                        }];
                        return Ok(Some(self.parse_arrow_body(pos, params, true)?));
                    }
                    self.restore(snap2);
                }
                if !self.is_punct("(") {
                    self.restore(snap);
                    return Ok(None);
                }
                is_async = true;
                // fall through to parenthesized params attempt with snap
                match self.try_parse_arrow_params()? {
                    Some(params) => return Ok(Some(self.parse_arrow_body(pos, params, is_async)?)),
                    None => {
                        self.restore(snap);
                        return Ok(None);
                    }
                }
            }
            TokenKind::Ident(n) if !self.is_reserved(n) => {
                if self.lx.next_is_arrow() == Some(false) {
                    return Ok(None);
                }
                let name = n.clone();
                let snap = self.snapshot();
                self.advance()?;
                if self.is_punct("=>") && !self.tok.newline_before {
                    self.advance()?;
                    if self.ctx.strict && (name == "eval" || name == "arguments") {
                        return Err(self.error_at(pos, "Unexpected eval or arguments in strict mode"));
                    }
                    let params = vec![Param {
                        pattern: Pattern::Identifier(name),
                        rest: false,
                    }];
                    return Ok(Some(self.parse_arrow_body(pos, params, false)?));
                }
                self.restore(snap);
                Ok(None)
            }
            TokenKind::Punct("(") => {
                let start = self.tok.start;
                if self.not_arrow.contains(&start) {
                    return Ok(None);
                }
                if !self.paren_may_start_arrow() {
                    self.not_arrow.insert(start);
                    return Ok(None);
                }
                let snap = self.snapshot();
                match self.try_parse_arrow_params()? {
                    Some(params) => Ok(Some(self.parse_arrow_body(pos, params, false)?)),
                    None => {
                        self.restore(snap);
                        Ok(None)
                    }
                }
            }
            _ => Ok(None),
        }
    }

    /// Cheap token scan from the current `(`: false when it certainly does
    /// not start arrow parameters (the contents are not a parameter list,
    /// or no `=>` follows the matching `)`), so the speculative parameter
    /// parse can be skipped. Regular expressions and templates make the
    /// scan give up (true: let the parser decide).
    fn paren_may_start_arrow(&mut self) -> bool {
        let snap = self.snapshot();
        self.lx.set_recording(true);
        let r = self.scan_arrow_params().unwrap_or(true);
        self.lx.set_recording(false);
        self.restore(snap);
        r
    }

    fn scan_arrow_params(&mut self) -> PResult<bool> {
        self.advance()?; // (
        loop {
            // A parameter: binding identifier / pattern, optionally rest.
            match &self.tok.kind {
                TokenKind::Punct(")") => break,
                TokenKind::Punct("...") => {
                    self.advance()?;
                }
                _ => {}
            }
            match &self.tok.kind {
                TokenKind::Ident(_) => {
                    self.advance()?;
                }
                TokenKind::Punct("[") | TokenKind::Punct("{") => {
                    if !self.scan_balanced()? {
                        return Ok(true);
                    }
                }
                _ => return Ok(false),
            }
            match &self.tok.kind {
                TokenKind::Punct(")") => break,
                TokenKind::Punct(",") => {
                    self.advance()?;
                }
                TokenKind::Punct("=") => {
                    // Default value: any expression up to `,` / `)` at this level.
                    self.advance()?;
                    loop {
                        match &self.tok.kind {
                            TokenKind::Punct(",") => {
                                self.advance()?;
                                break;
                            }
                            TokenKind::Punct(")") => break,
                            TokenKind::Punct("(") | TokenKind::Punct("[") | TokenKind::Punct("{") => {
                                if !self.scan_balanced()? {
                                    return Ok(true);
                                }
                            }
                            TokenKind::Punct("/") | TokenKind::Punct("/=") | TokenKind::Template { .. } | TokenKind::Punct("]") | TokenKind::Punct("}") | TokenKind::Eof => return Ok(true),
                            _ => {
                                self.advance()?;
                            }
                        }
                    }
                }
                _ => return Ok(false),
            }
        }
        // At the closing `)`.
        self.advance()?;
        Ok(self.is_punct("=>") && !self.tok.newline_before)
    }

    /// Skip a bracketed group starting at the current `(` / `[` / `{`;
    /// false when its tokens cannot be told apart without parsing.
    fn scan_balanced(&mut self) -> PResult<bool> {
        let mut depth = 0usize;
        loop {
            match &self.tok.kind {
                TokenKind::Punct("(") | TokenKind::Punct("[") | TokenKind::Punct("{") => depth += 1,
                TokenKind::Punct(")") | TokenKind::Punct("]") | TokenKind::Punct("}") => {
                    depth -= 1;
                    if depth == 0 {
                        self.advance()?;
                        return Ok(true);
                    }
                }
                TokenKind::Punct("/") | TokenKind::Punct("/=") | TokenKind::Template { .. } | TokenKind::Eof => return Ok(false),
                _ => {}
            }
            self.advance()?;
        }
    }

    /// Like `ident_name_if_binding` but used right after `async`, where
    /// `await` may not be a parameter name.
    fn ident_name_if_binding_async(&self) -> Option<Name> {
        match &self.tok.kind {
            TokenKind::Ident(n) if !self.is_reserved(n) && n != "await" => Some(n.clone()),
            _ => None,
        }
    }

    /// Try `( params ) =>`. On any failure returns Ok(None) and the caller
    /// restores the snapshot.
    fn try_parse_arrow_params(&mut self) -> PResult<Option<Vec<Param>>> {
        let start = self.tok.start;
        if self.not_arrow.contains(&start) {
            return Ok(None);
        }
        let r = self.try_parse_arrow_params_uncached()?;
        if r.is_none() {
            self.not_arrow.insert(start);
        }
        Ok(r)
    }

    fn try_parse_arrow_params_uncached(&mut self) -> PResult<Option<Vec<Param>>> {
        let result: PResult<Vec<Param>> = (|| {
            self.expect_punct("(")?;
            let params = self.parse_formal_params_until_paren()?;
            Ok(params)
        })();
        let params = match result {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };
        if self.is_punct("=>") && !self.tok.newline_before {
            self.advance()?;
            Ok(Some(params))
        } else {
            Ok(None)
        }
    }

    /// Formal parameters after `(`, consuming the closing `)`.
    fn parse_formal_params_until_paren(&mut self) -> PResult<Vec<Param>> {
        let mut params = Vec::new();
        loop {
            if self.eat_punct(")")? {
                break;
            }
            if self.eat_punct("...")? {
                let pattern = self.parse_binding_target()?;
                params.push(Param { pattern, rest: true });
                self.expect_punct(")")?;
                break;
            }
            let pattern = self.parse_binding_element()?;
            params.push(Param { pattern, rest: false });
            if !self.is_punct(")") {
                self.expect_punct(",")?;
            }
        }
        Ok(params)
    }

    fn parse_arrow_body(&mut self, pos: Pos, params: Vec<Param>, is_async: bool) -> PResult<Expr> {
        let ctx = Context {
            in_function: true,
            in_generator: false,
            in_async: is_async,
            in_loop: false,
            in_switch: false,
            strict: self.ctx.strict,
            is_module: self.ctx.is_module,
            labels: Vec::new(),
            in_class_body: self.ctx.in_class_body,
            super_call_ok: self.ctx.super_call_ok,
            super_prop_ok: self.ctx.super_prop_ok,
        };
        let no_in = self.no_in;
        let simple = params.iter().all(|p| matches!(p.pattern, Pattern::Identifier(_)) && !p.rest);
        let (body, strict) = self.with_ctx(ctx, |p| {
            if p.is_punct("{") {
                p.parse_function_body(simple, false)
            } else {
                p.no_in = no_in;
                let e = p.parse_assignment()?;
                Ok((FunctionBody::Expr(Box::new(e)), p.ctx.strict))
            }
        })?;
        Ok(Expr {
            pos,
            kind: ExprKind::Arrow(std::rc::Rc::new(Function {
                name: None,
                params,
                body,
                is_arrow: true,
                is_async,
                is_generator: false,
                strict,
                pos,
                span: (0, 0),
                self_binding: false,
            })),
        })
    }

    fn parse_conditional(&mut self) -> PResult<Expr> {
        let pos = self.tok.pos;
        let test = self.parse_binary(0)?.expr;
        if !self.eat_punct("?")? {
            return Ok(test);
        }
        let consequent = self.with_no_in(false, |p| p.parse_assignment())?;
        self.expect_punct(":")?;
        let alternate = self.parse_assignment()?;
        Ok(Expr {
            pos,
            kind: ExprKind::Conditional {
                test: Box::new(test),
                consequent: Box::new(consequent),
                alternate: Box::new(alternate),
            },
        })
    }

    fn current_binary_op(&self) -> Option<(u8, BinOrLogical)> {
        match &self.tok.kind {
            TokenKind::Punct(p) => {
                if let Some((op, prec)) = binary_op(p) {
                    return Some((prec, BinOrLogical::Binary(op)));
                }
                match *p {
                    "||" => Some((1, BinOrLogical::Logical(LogicalOp::Or))),
                    "??" => Some((1, BinOrLogical::Logical(LogicalOp::Nullish))),
                    "&&" => Some((2, BinOrLogical::Logical(LogicalOp::And))),
                    _ => None,
                }
            }
            TokenKind::Ident(n) => match n.as_str() {
                "instanceof" => Some((7, BinOrLogical::Binary(BinaryOp::Instanceof))),
                "in" if !self.no_in => Some((7, BinOrLogical::Binary(BinaryOp::In))),
                _ => None,
            },
            _ => None,
        }
    }

    fn parse_binary(&mut self, min_prec: u8) -> PResult<BinResult> {
        let pos = self.tok.pos;
        // `#x in obj`
        let mut left = if let TokenKind::PrivateName(name) = &self.tok.kind {
            if min_prec > 7 {
                // `#x in` is a RelationalExpression, not an operand of a
                // tighter operator (`#x in #y in o`).
                return Err(self.error_at(pos, "Unexpected private name"));
            }
            let name = name.clone();
            self.advance()?;
            if !self.is_ident("in") || self.no_in {
                return Err(self.error_at(pos, "Unexpected private name"));
            }
            self.advance()?;
            let object = self.parse_binary(8)?.expr;
            BinResult {
                expr: Expr {
                    pos,
                    kind: ExprKind::PrivateIn {
                        name,
                        object: Box::new(object),
                    },
                },
                logical: None,
            }
        } else {
            BinResult {
                expr: self.parse_unary()?,
                logical: None,
            }
        };
        loop {
            let Some((prec, op)) = self.current_binary_op() else { break };
            if prec < min_prec {
                break;
            }
            let op_pos = self.tok.pos;
            self.advance()?;
            let right = self.parse_binary(prec + 1)?;
            match op {
                BinOrLogical::Binary(bop) => {
                    left = BinResult {
                        expr: Expr {
                            pos,
                            kind: ExprKind::Binary {
                                op: bop,
                                left: Box::new(left.expr),
                                right: Box::new(right.expr),
                            },
                        },
                        logical: None,
                    };
                }
                BinOrLogical::Logical(lop) => {
                    let mixes = |other: Option<LogicalOp>| match (lop, other) {
                        (LogicalOp::Nullish, Some(LogicalOp::And) | Some(LogicalOp::Or)) => true,
                        (LogicalOp::And | LogicalOp::Or, Some(LogicalOp::Nullish)) => true,
                        _ => false,
                    };
                    if mixes(left.logical) || mixes(right.logical) {
                        return Err(self.error_at(op_pos, "Cannot mix '??' with '||' or '&&' without parentheses"));
                    }
                    left = BinResult {
                        expr: Expr {
                            pos,
                            kind: ExprKind::Logical {
                                op: lop,
                                left: Box::new(left.expr),
                                right: Box::new(right.expr),
                            },
                        },
                        logical: Some(lop),
                    };
                }
            }
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> PResult<Expr> {
        self.parse_unary_impl(true)
    }

    /// `exp_allowed` is false for the operand of a unary operator, where a
    /// following `**` is a syntax error (`-2 ** 2`).
    fn parse_unary_impl(&mut self, exp_allowed: bool) -> PResult<Expr> {
        let pos = self.tok.pos;
        let op = match &self.tok.kind {
            TokenKind::Punct("-") => Some(UnaryOp::Neg),
            TokenKind::Punct("+") => Some(UnaryOp::Plus),
            TokenKind::Punct("!") => Some(UnaryOp::Not),
            TokenKind::Punct("~") => Some(UnaryOp::BitNot),
            TokenKind::Ident(n) if n == "typeof" => Some(UnaryOp::Typeof),
            TokenKind::Ident(n) if n == "void" => Some(UnaryOp::Void),
            TokenKind::Ident(n) if n == "delete" => Some(UnaryOp::Delete),
            _ => None,
        };
        if let Some(op) = op {
            self.advance()?;
            let operand = self.parse_unary_impl(false)?;
            if self.is_punct("**") {
                return Err(self.error("Unary operator used immediately before exponentiation expression. Parenthesis must be used to disambiguate operator precedence"));
            }
            if op == UnaryOp::Delete && self.ctx.strict {
                if let ExprKind::Identifier(_) = operand.kind {
                    return Err(self.error_at(pos, "Delete of an unqualified identifier in strict mode"));
                }
            }
            return Ok(Expr {
                pos,
                kind: ExprKind::Unary {
                    op,
                    operand: Box::new(operand),
                },
            });
        }
        if self.is_ident("await") && (self.ctx.in_async || (self.ctx.is_module && !self.ctx.in_function)) {
            self.advance()?;
            let operand = self.parse_unary_impl(false)?;
            if self.is_punct("**") {
                return Err(self.error("Unary operator used immediately before exponentiation expression. Parenthesis must be used to disambiguate operator precedence"));
            }
            return Ok(Expr {
                pos,
                kind: ExprKind::Await(Box::new(operand)),
            });
        }
        if self.is_punct("++") || self.is_punct("--") {
            let op = if self.is_punct("++") { UpdateOp::Increment } else { UpdateOp::Decrement };
            self.advance()?;
            let target = self.parse_unary()?;
            self.check_simple_target(&target)?;
            return Ok(Expr {
                pos,
                kind: ExprKind::Update {
                    op,
                    prefix: true,
                    target: Box::new(target),
                },
            });
        }
        let expr = self.parse_postfix()?;
        if self.is_punct("**") {
            if !exp_allowed {
                return Err(self.error("Unary operator used immediately before exponentiation expression. Parenthesis must be used to disambiguate operator precedence"));
            }
            self.advance()?;
            let right = self.parse_unary()?;
            return Ok(Expr {
                pos,
                kind: ExprKind::Binary {
                    op: BinaryOp::Exp,
                    left: Box::new(expr),
                    right: Box::new(right),
                },
            });
        }
        Ok(expr)
    }

    fn check_simple_target(&self, e: &Expr) -> PResult<()> {
        match &e.kind {
            ExprKind::Identifier(n) => {
                if self.ctx.strict && (n == "eval" || n == "arguments") {
                    return Err(self.error_at(e.pos, "Invalid update target in strict mode"));
                }
                Ok(())
            }
            ExprKind::Member { optional: false, .. } => Ok(()),
            _ => Err(self.error_at(e.pos, "Invalid left-hand side expression in prefix/postfix operation")),
        }
    }

    fn parse_postfix(&mut self) -> PResult<Expr> {
        let pos = self.tok.pos;
        let expr = self.parse_lhs()?;
        if (self.is_punct("++") || self.is_punct("--")) && !self.tok.newline_before {
            let op = if self.is_punct("++") { UpdateOp::Increment } else { UpdateOp::Decrement };
            self.check_simple_target(&expr)?;
            self.advance()?;
            return Ok(Expr {
                pos,
                kind: ExprKind::Update {
                    op,
                    prefix: false,
                    target: Box::new(expr),
                },
            });
        }
        Ok(expr)
    }

    /// LeftHandSideExpression: new/call/member chains.
    fn parse_lhs(&mut self) -> PResult<Expr> {
        let pos = self.tok.pos;
        let base = if self.is_ident("new") {
            self.parse_new()?
        } else if self.is_ident("super") {
            self.advance()?;
            if self.is_punct("(") {
                if !self.ctx.super_call_ok {
                    return Err(self.error_at(pos, "'super' keyword unexpected here"));
                }
            } else if !(self.is_punct(".") || self.is_punct("[")) || !self.ctx.super_prop_ok {
                return Err(self.error_at(pos, "'super' keyword unexpected here"));
            }
            Expr {
                pos,
                kind: ExprKind::Super,
            }
        } else if self.is_ident("import") {
            self.advance()?;
            if self.eat_punct(".")? {
                self.expect_ident("meta")?;
                if !self.ctx.is_module {
                    return Err(self.error_at(pos, "Cannot use 'import.meta' outside a module"));
                }
                Expr {
                    pos,
                    kind: ExprKind::ImportMeta,
                }
            } else {
                self.expect_punct("(")?;
                let arg = self.with_no_in(false, |p| p.parse_assignment())?;
                // Optional second argument (import attributes).
                if self.eat_punct(",")? && !self.is_punct(")") {
                    self.with_no_in(false, |p| p.parse_assignment())?;
                    self.eat_punct(",")?;
                }
                self.expect_punct(")")?;
                Expr {
                    pos,
                    kind: ExprKind::DynamicImport(Box::new(arg)),
                }
            }
        } else {
            self.parse_primary()?
        };
        self.parse_call_tail(base, true)
    }

    fn parse_new(&mut self) -> PResult<Expr> {
        let pos = self.tok.pos;
        self.advance()?; // new
        if self.eat_punct(".")? {
            self.expect_ident("target")?;
            // Where `new.target` may appear is checked in `early`.
            return Ok(Expr {
                pos,
                kind: ExprKind::NewTarget,
            });
        }
        let callee = if self.is_ident("new") {
            self.parse_new()?
        } else if self.is_ident("super") {
            return Err(self.error("'super' keyword unexpected here"));
        } else if self.is_ident("import") {
            return Err(self.error("Cannot use new with import"));
        } else {
            let p = self.parse_primary()?;
            self.parse_call_tail(p, false)?
        };
        let args = if self.is_punct("(") { self.parse_arguments()? } else { Vec::new() };
        Ok(Expr {
            pos,
            kind: ExprKind::New {
                callee: Box::new(callee),
                args,
            },
        })
    }

    /// Member accesses, calls (when `allow_call`) and tagged templates.
    fn parse_call_tail(&mut self, mut expr: Expr, allow_call: bool) -> PResult<Expr> {
        let pos = expr.pos;
        let mut in_chain = false;
        loop {
            match &self.tok.kind {
                TokenKind::Punct(".") => {
                    self.advance()?;
                    let property = if let TokenKind::PrivateName(n) = &self.tok.kind {
                        let n = n.clone();
                        self.advance()?;
                        MemberProp::Private(n)
                    } else {
                        MemberProp::Identifier(self.expect_identifier_name()?)
                    };
                    expr = Expr {
                        pos,
                        kind: ExprKind::Member {
                            object: Box::new(expr),
                            property,
                            optional: false,
                        },
                    };
                }
                TokenKind::Punct("?.") => {
                    if !allow_call {
                        return Err(self.error("Invalid optional chain from new expression"));
                    }
                    self.advance()?;
                    in_chain = true;
                    if self.is_punct("(") {
                        let args = self.parse_arguments()?;
                        expr = Expr {
                            pos,
                            kind: ExprKind::Call {
                                callee: Box::new(expr),
                                args,
                                optional: true,
                            },
                        };
                    } else if self.eat_punct("[")? {
                        let property = self.with_no_in(false, |p| p.parse_expression())?;
                        self.expect_punct("]")?;
                        expr = Expr {
                            pos,
                            kind: ExprKind::Member {
                                object: Box::new(expr),
                                property: MemberProp::Computed(Box::new(property)),
                                optional: true,
                            },
                        };
                    } else if let TokenKind::Template { .. } = self.tok.kind {
                        return Err(self.error("Invalid tagged template on optional chain"));
                    } else {
                        let property = if let TokenKind::PrivateName(n) = &self.tok.kind {
                            let n = n.clone();
                            self.advance()?;
                            MemberProp::Private(n)
                        } else {
                            MemberProp::Identifier(self.expect_identifier_name()?)
                        };
                        expr = Expr {
                            pos,
                            kind: ExprKind::Member {
                                object: Box::new(expr),
                                property,
                                optional: true,
                            },
                        };
                    }
                }
                TokenKind::Punct("[") => {
                    self.advance()?;
                    let property = self.with_no_in(false, |p| p.parse_expression())?;
                    self.expect_punct("]")?;
                    expr = Expr {
                        pos,
                        kind: ExprKind::Member {
                            object: Box::new(expr),
                            property: MemberProp::Computed(Box::new(property)),
                            optional: false,
                        },
                    };
                }
                TokenKind::Punct("(") if allow_call => {
                    let args = self.parse_arguments()?;
                    expr = Expr {
                        pos,
                        kind: ExprKind::Call {
                            callee: Box::new(expr),
                            args,
                            optional: false,
                        },
                    };
                }
                TokenKind::Template { .. } => {
                    if in_chain {
                        return Err(self.error("Invalid tagged template on optional chain"));
                    }
                    let (quasis, raws, expressions) = self.parse_template_parts(true)?;
                    expr = Expr {
                        pos,
                        kind: ExprKind::TaggedTemplate {
                            tag: Box::new(expr),
                            quasis: Box::new(quasis),
                            raws: Box::new(raws),
                            expressions: Box::new(expressions),
                        },
                    };
                }
                _ => break,
            }
        }
        if in_chain {
            expr = Expr {
                pos,
                kind: ExprKind::OptionalChain(Box::new(expr)),
            };
        }
        Ok(expr)
    }

    fn parse_arguments(&mut self) -> PResult<Vec<ArrayElement>> {
        self.expect_punct("(")?;
        let mut args = Vec::new();
        self.with_no_in(false, |p| {
            loop {
                if p.eat_punct(")")? {
                    break;
                }
                if p.eat_punct("...")? {
                    args.push(ArrayElement::Spread(p.parse_assignment()?));
                } else {
                    args.push(ArrayElement::Expr(p.parse_assignment()?));
                }
                if !p.is_punct(")") {
                    p.expect_punct(",")?;
                }
            }
            Ok(())
        })?;
        Ok(args)
    }

    fn parse_primary(&mut self) -> PResult<Expr> {
        let pos = self.tok.pos;
        let kind = match &self.tok.kind {
            TokenKind::Number(n) => {
                let n = *n;
                self.check_strict_literal()?;
                self.advance()?;
                ExprKind::Number(n)
            }
            TokenKind::BigInt(_) => match self.advance()?.kind {
                TokenKind::BigInt(s) => ExprKind::BigInt(s),
                _ => unreachable!(),
            },
            TokenKind::Str(_) => {
                self.check_strict_literal()?;
                match self.advance()?.kind {
                    TokenKind::Str(s) => ExprKind::String(s),
                    _ => unreachable!(),
                }
            }
            TokenKind::Template { .. } => {
                let (quasis, raws, expressions) = self.parse_template_parts(false)?;
                ExprKind::Template {
                    quasis: Box::new(quasis),
                    raws: Box::new(raws),
                    expressions,
                }
            }
            TokenKind::Punct("/") | TokenKind::Punct("/=") => {
                let tok = self.tok.clone();
                let re = self.lx.rescan_regex(&tok)?;
                self.tok = re;
                let TokenKind::Regex { pattern, flags } = self.tok.kind.clone() else { unreachable!() };
                self.advance()?;
                ExprKind::Regex { pattern: pattern.into(), flags: flags.into() }
            }
            TokenKind::Punct("(") => return self.parse_paren(),
            TokenKind::Punct("[") => return self.parse_array_literal(),
            TokenKind::Punct("{") => return self.parse_object_literal(),
            TokenKind::Ident(name) => match keyword_token(&self.tok, name) {
                "this" => {
                    self.advance()?;
                    ExprKind::This
                }
                "null" => {
                    self.advance()?;
                    ExprKind::Null
                }
                "true" => {
                    self.advance()?;
                    ExprKind::Bool(true)
                }
                "false" => {
                    self.advance()?;
                    ExprKind::Bool(false)
                }
                "function" => {
                    let f = self.parse_function(false, true, false)?;
                    ExprKind::Function(std::rc::Rc::new(f))
                }
                "async" if self.peek_is_function_no_newline()? => {
                    self.advance()?;
                    let f = self.parse_function(true, true, false)?;
                    ExprKind::Function(std::rc::Rc::new(f))
                }
                "class" => {
                    let c = self.parse_class(false)?;
                    ExprKind::Class(Box::new(c))
                }
                "new" => return self.parse_new(),
                _ => {
                    if matches!(&self.tok.kind, TokenKind::Ident(n) if self.is_reserved(n)) {
                        return Err(self.unexpected());
                    }
                    match self.advance()?.kind {
                        TokenKind::Ident(name) => ExprKind::Identifier(name),
                        _ => unreachable!(),
                    }
                }
            },
            _ => return Err(self.unexpected()),
        };
        Ok(Expr { pos, kind })
    }

    fn parse_paren(&mut self) -> PResult<Expr> {
        self.expect_punct("(")?;
        let e = self.with_no_in(false, |p| p.parse_expression())?;
        self.expect_punct(")")?;
        if matches!(e.kind, ExprKind::Object(_) | ExprKind::Array(_) | ExprKind::Assign { .. }) {
            self.parenthesized.insert((e.pos.line, e.pos.column));
        }
        Ok(e)
    }

    /// Parse template chunks starting at the current Template token.
    fn parse_template_parts(&mut self, tagged: bool) -> PResult<(Vec<Option<String>>, Vec<String>, Vec<Expr>)> {
        let mut quasis = Vec::new();
        let mut raws = Vec::new();
        let mut expressions = Vec::new();
        loop {
            let TokenKind::Template { cooked, raw, tail } = self.tok.kind.clone() else {
                return Err(self.error("Expected template continuation"));
            };
            if cooked.is_none() && !tagged {
                return Err(self.error("Invalid escape sequence in template"));
            }
            quasis.push(cooked);
            raws.push(raw);
            if tail {
                self.advance()?;
                break;
            }
            self.advance()?;
            let e = self.with_no_in(false, |p| p.parse_expression())?;
            expressions.push(e);
            if !self.is_punct("}") {
                return Err(self.error("Expected '}' in template literal"));
            }
            let tok = self.tok.clone();
            let cont = self.lx.rescan_template_continuation(&tok)?;
            self.tok = cont;
        }
        Ok((quasis, raws, expressions))
    }

    fn parse_array_literal(&mut self) -> PResult<Expr> {
        let pos = self.tok.pos;
        self.expect_punct("[")?;
        let mut elements = Vec::new();
        self.with_no_in(false, |p| {
            loop {
                if p.eat_punct("]")? {
                    break;
                }
                if p.is_punct(",") {
                    p.advance()?;
                    elements.push(ArrayElement::Hole);
                    continue;
                }
                let el = if p.eat_punct("...")? {
                    let e = p.parse_assignment()?;
                    if p.is_punct(",") {
                        // `[...a,]` is no assignment pattern.
                        p.rest_comma.insert((e.pos.line, e.pos.column));
                    }
                    ArrayElement::Spread(e)
                } else {
                    ArrayElement::Expr(p.parse_assignment()?)
                };
                elements.push(el);
                if !p.is_punct("]") {
                    p.expect_punct(",")?;
                }
            }
            Ok(())
        })?;
        Ok(Expr {
            pos,
            kind: ExprKind::Array(elements),
        })
    }

    /// Parses a property key. Also returns the identifier text when the key
    /// is a plain identifier (for shorthand detection).
    fn parse_property_key(&mut self) -> PResult<(PropertyKey, Option<Name>)> {
        if matches!(self.tok.kind, TokenKind::Str(_) | TokenKind::Number(_)) {
            self.check_strict_literal()?;
        }
        Ok(match &self.tok.kind {
            TokenKind::Ident(_) | TokenKind::Str(_) | TokenKind::BigInt(_) | TokenKind::PrivateName(_) => match self.advance()?.kind {
                TokenKind::Ident(n) => (PropertyKey::Identifier(n.clone()), Some(n)),
                TokenKind::Str(s) | TokenKind::BigInt(s) => (PropertyKey::String(s.into()), None),
                TokenKind::PrivateName(n) => (PropertyKey::Private(n), None),
                _ => unreachable!(),
            },
            TokenKind::Number(n) => {
                let n = *n;
                self.advance()?;
                (PropertyKey::Number(n), None)
            }
            TokenKind::Punct("[") => {
                self.advance()?;
                let e = self.with_no_in(false, |p| p.parse_assignment())?;
                self.expect_punct("]")?;
                (PropertyKey::Computed(Box::new(e)), None)
            }
            _ => return Err(self.unexpected()),
        })
    }

    fn parse_object_literal(&mut self) -> PResult<Expr> {
        let pos = self.tok.pos;
        self.expect_punct("{")?;
        let mut props = Vec::new();
        self.with_no_in(false, |p| {
            loop {
                if p.eat_punct("}")? {
                    break;
                }
                if p.eat_punct("...")? {
                    let e = p.parse_assignment()?;
                    if p.is_punct(",") {
                        p.rest_comma.insert((e.pos.line, e.pos.column));
                    }
                    props.push(ObjectProperty::Spread(e));
                } else {
                    props.push(p.parse_object_property()?);
                }
                if !p.is_punct("}") {
                    p.expect_punct(",")?;
                }
            }
            Ok(())
        })?;
        Ok(Expr {
            pos,
            kind: ExprKind::Object(props),
        })
    }

    /// Is the current token something that can follow a modifier
    /// (`get`/`set`/`async`/`static`) as a property key?
    fn token_starts_key(&self) -> bool {
        matches!(
            self.tok.kind,
            TokenKind::Ident(_) | TokenKind::Str(_) | TokenKind::Number(_) | TokenKind::BigInt(_) | TokenKind::PrivateName(_) | TokenKind::Punct("[")
        )
    }

    fn parse_object_property(&mut self) -> PResult<ObjectProperty> {
        let pos = self.tok.pos;
        // Modifiers
        let mut is_async = false;
        let mut is_generator = false;
        let mut accessor: Option<bool> = None; // Some(true)=getter, Some(false)=setter
        if self.is_ident("async") {
            let snap = self.snapshot();
            self.advance()?;
            if (self.token_starts_key() || self.is_punct("*")) && !self.tok.newline_before {
                is_async = true;
            } else {
                self.restore(snap);
            }
        }
        if !is_async && (self.is_ident("get") || self.is_ident("set")) {
            let is_get = self.is_ident("get");
            let snap = self.snapshot();
            self.advance()?;
            if self.token_starts_key() {
                accessor = Some(is_get);
            } else {
                self.restore(snap);
            }
        }
        if self.eat_punct("*")? {
            is_generator = true;
        }
        let (key, ident_name) = self.parse_property_key()?;
        if matches!(key, PropertyKey::Private(_)) {
            return Err(self.error_at(pos, "Private names are not allowed in object literals"));
        }
        if self.is_punct("(") {
            let f = self.parse_method_function(pos, is_async, is_generator, accessor, false)?;
            let value = match accessor {
                Some(true) => PropertyValue::Getter(std::rc::Rc::new(f)),
                Some(false) => PropertyValue::Setter(std::rc::Rc::new(f)),
                None => PropertyValue::Method(std::rc::Rc::new(f)),
            };
            return Ok(ObjectProperty::Property {
                key,
                value,
                shorthand: false,
            });
        }
        if is_async || is_generator || accessor.is_some() {
            return Err(self.unexpected());
        }
        if self.eat_punct(":")? {
            let value = self.parse_assignment()?;
            return Ok(ObjectProperty::Property {
                key,
                value: PropertyValue::Init(value),
                shorthand: false,
            });
        }
        // Shorthand (possibly with a cover initializer `a = 1` for patterns).
        let name = match ident_name {
            Some(n) if !self.is_reserved(&n) => n,
            _ => return Err(self.error_at(pos, "Invalid shorthand property initializer")),
        };
        let ident = Expr {
            pos,
            kind: ExprKind::Identifier(name),
        };
        if self.eat_punct("=")? {
            let default = self.parse_assignment()?;
            let target = self.expr_to_pattern(ident, false)?;
            return Ok(ObjectProperty::Property {
                key,
                value: PropertyValue::Init(Expr {
                    pos,
                    kind: ExprKind::Assign {
                        op: AssignOp::Assign,
                        target: Box::new(target),
                        value: Box::new(default),
                    },
                }),
                shorthand: true,
            });
        }
        Ok(ObjectProperty::Property {
            key,
            value: PropertyValue::Init(ident),
            shorthand: true,
        })
    }

    /// Method body: `( params ) { body }` with method context.
    fn parse_method_function(&mut self, pos: Pos, is_async: bool, is_generator: bool, accessor: Option<bool>, super_call_ok: bool) -> PResult<Function> {
        let ctx = Context {
            in_function: true,
            in_generator: is_generator,
            in_async: is_async,
            in_loop: false,
            in_switch: false,
            strict: self.ctx.strict,
            is_module: self.ctx.is_module,
            labels: Vec::new(),
            in_class_body: false,
            super_call_ok,
            super_prop_ok: true,
        };
        self.with_ctx(ctx, |p| {
            p.expect_punct("(")?;
            let params = p.parse_formal_params_until_paren()?;
            match accessor {
                Some(true) if !params.is_empty() => return Err(p.error_at(pos, "Getter must not have any formal parameters")),
                Some(false) if params.len() != 1 || params[0].rest => return Err(p.error_at(pos, "Setter must have exactly one formal parameter")),
                _ => {}
            }
            let simple = params.iter().all(|p| matches!(p.pattern, Pattern::Identifier(_)) && !p.rest);
            let (body, strict) = p.parse_function_body(simple, false)?;
            Ok(Function {
                name: None,
                params,
                body,
                is_arrow: false,
                is_async,
                is_generator,
                strict,
                pos,
                span: (0, 0),
                self_binding: false,
            })
        })
    }

    /// `{ body }` of a function: skipped by the pre-parser when lazy
    /// parsing applies (see [`FunctionBody::Lazy`]), else parsed.
    fn parse_function_body(&mut self, simple_params: bool, eager: bool) -> PResult<(FunctionBody, bool)> {
        if self.lazy && !eager && self.is_punct("{") {
            if let Some(r) = self.skip_function_body(simple_params)? {
                return Ok(r);
            }
        }
        let (stmts, strict) = self.parse_function_body_block()?;
        Ok((FunctionBody::Block(stmts), strict))
    }

    fn skip_function_body(&mut self, simple_params: bool) -> PResult<Option<(FunctionBody, bool)>> {
        let open_start = self.tok.start;
        let open_pos = self.tok.pos;
        let after_open = self.lx.state();
        let Some(sk) = self.lx.skip_function_body() else { return Ok(None) };
        if sk.end.pos() - open_start < LAZY_MIN_BODY || (sk.use_strict && !simple_params) {
            // Small (or an early error to report): parse it now.
            self.lx.restore(&after_open);
            return Ok(None);
        }
        let source = match &self.lazy_src {
            Some(s) => s.clone(),
            None => {
                let s: std::rc::Rc<str> = std::rc::Rc::from(self.lx.source());
                self.lazy_src = Some(s.clone());
                s
            }
        };
        let body = LazyBody {
            source,
            start: open_start,
            end: sk.end.pos(),
            line: open_pos.line,
            line_start: open_start - (open_pos.column as usize - 1),
            names: sk.names.into_boxed_slice(),
            eval: sk.eval,
            is_module: self.ctx.is_module,
            in_class_body: self.ctx.in_class_body,
            super_call_ok: self.ctx.super_call_ok,
            super_prop_ok: self.ctx.super_prop_ok,
        };
        self.skipped = true;
        // Step past the closing `}` like `advance` would.
        self.prev_end = sk.end.pos();
        self.prev_line = sk.close_pos.line;
        self.tok = self.lx.next_token()?;
        Ok(Some((FunctionBody::Lazy(std::rc::Rc::new(body)), self.ctx.strict || sk.use_strict)))
    }

    /// `{ body }` of a function, handling the directive prologue. Returns
    /// the statements and the resulting strictness.
    fn parse_function_body_block(&mut self) -> PResult<(Vec<Stmt>, bool)> {
        self.expect_punct("{")?;
        let mut body = Vec::new();
        self.parse_directives(&mut body)?;
        while !self.is_punct("}") {
            if self.tok.kind == TokenKind::Eof {
                return Err(self.error("Unexpected end of input"));
            }
            body.push(self.parse_statement_list_item()?);
        }
        self.advance()?;
        Ok((self.lower_using_list(body, true)?, self.ctx.strict))
    }

    /// `function [*] [name] ( params ) { body }` — current token is `function`.
    fn parse_function(&mut self, is_async: bool, is_expression: bool, is_declaration: bool) -> PResult<Function> {
        let pos = self.tok.pos;
        let mut start = self.tok.start as u32;
        if is_async {
            // The source text (toString) starts at `async`.
            let before = self.lx.source()[..start as usize].trim_end();
            if before.ends_with("async") {
                start = (before.len() - 5) as u32;
            }
        }
        // Functions right after `(` or `!` are usually called at once
        // (`(function(){...})()`, callbacks): parse them eagerly.
        let eager = !is_async && self.prev_end > 0 && matches!(self.lx.source().as_bytes().get(self.prev_end - 1), Some(b'(') | Some(b'!'));
        self.expect_ident("function")?;
        let is_generator = self.eat_punct("*")?;
        let name = if self.is_punct("(") {
            if is_declaration {
                return Err(self.error("Function statements require a function name"));
            }
            None
        } else {
            // The name of a function expression is bound inside its own
            // generator/async context.
            let saved = self.ctx.clone();
            if is_expression {
                self.ctx.in_generator = is_generator;
                self.ctx.in_async = is_async;
            }
            let n = self.expect_binding_identifier();
            self.ctx = saved;
            Some(n?)
        };
        let name_is_some = name.is_some();
        let ctx = Context {
            in_function: true,
            in_generator: is_generator,
            in_async: is_async,
            in_loop: false,
            in_switch: false,
            strict: self.ctx.strict,
            is_module: self.ctx.is_module,
            labels: Vec::new(),
            in_class_body: false,
            super_call_ok: false,
            super_prop_ok: false,
        };
        self.with_ctx(ctx, |p| {
            p.expect_punct("(")?;
            let params = p.parse_formal_params_until_paren()?;
            let simple = params.iter().all(|p| matches!(p.pattern, Pattern::Identifier(_)) && !p.rest);
            let (body, strict) = p.parse_function_body(simple, eager)?;
            Ok(Function {
                name,
                params,
                body,
                is_arrow: false,
                is_async,
                is_generator,
                strict,
                pos,
                span: (start, p.prev_end as u32),
                self_binding: is_expression && name_is_some,
            })
        })
    }

    fn parse_class(&mut self, is_declaration: bool) -> PResult<Class> {
        let pos = self.tok.pos;
        let start = self.tok.start as u32;
        self.expect_ident("class")?;
        // Class bodies are always strict.
        let saved_strict = self.ctx.strict;
        self.ctx.strict = true;
        let result = (|| -> PResult<Class> {
            let name = if self.is_ident("extends") || self.is_punct("{") {
                if is_declaration {
                    return Err(self.error("Class statements require a class name"));
                }
                None
            } else {
                Some(self.expect_binding_identifier()?)
            };
            let superclass = if self.eat_ident("extends")? {
                let e = self.parse_lhs()?;
                Some(Box::new(e))
            } else {
                None
            };
            self.expect_punct("{")?;
            let mut members = Vec::new();
            while !self.is_punct("}") {
                if self.eat_punct(";")? {
                    continue;
                }
                if self.tok.kind == TokenKind::Eof {
                    return Err(self.error("Unexpected end of input"));
                }
                members.push(self.parse_class_member(superclass.is_some())?);
            }
            self.advance()?;
            Ok(Class {
                name,
                superclass,
                members,
                pos,
                span: (start, self.prev_end as u32),
            })
        })();
        self.ctx.strict = saved_strict;
        result
    }

    fn parse_class_member(&mut self, derived: bool) -> PResult<ClassMember> {
        let pos = self.tok.pos;
        let mut is_static = false;
        if self.is_ident("static") {
            let snap = self.snapshot();
            self.advance()?;
            if self.is_punct("{") {
                // static block
                let ctx = Context {
                    in_function: false,
                    in_generator: false,
                    in_async: false,
                    in_loop: false,
                    in_switch: false,
                    strict: true,
                    is_module: self.ctx.is_module,
                    labels: Vec::new(),
                    in_class_body: true,
                    super_call_ok: false,
                    super_prop_ok: true,
                };
                let body = self.with_ctx(ctx, |p| p.parse_block())?;
                return Ok(ClassMember::StaticBlock(body));
            }
            if self.token_starts_key() || self.is_punct("*") {
                is_static = true;
            } else {
                self.restore(snap);
            }
        }
        let mut is_async = false;
        let mut is_generator = false;
        let mut accessor: Option<bool> = None;
        if self.is_ident("async") {
            let snap = self.snapshot();
            self.advance()?;
            if (self.token_starts_key() || self.is_punct("*")) && !self.tok.newline_before {
                is_async = true;
            } else {
                self.restore(snap);
            }
        }
        if !is_async && (self.is_ident("get") || self.is_ident("set")) {
            let is_get = self.is_ident("get");
            let snap = self.snapshot();
            self.advance()?;
            if self.token_starts_key() {
                accessor = Some(is_get);
            } else {
                self.restore(snap);
            }
        }
        if self.eat_punct("*")? {
            is_generator = true;
        }
        let (key, _) = self.parse_property_key()?;
        if self.is_punct("(") {
            let is_ctor = !is_static
                && accessor.is_none()
                && matches!(&key, PropertyKey::Identifier(n) if n == "constructor")
                || matches!(&key, PropertyKey::String(s) if s == "constructor" && !is_static && accessor.is_none());
            if is_ctor {
                if is_async || is_generator {
                    return Err(self.error_at(pos, "Class constructor may not be an async method or generator"));
                }
                let f = self.parse_method_function(pos, false, false, None, derived)?;
                return Ok(ClassMember::Constructor(f));
            }
            let f = self.parse_method_function(pos, is_async, is_generator, accessor, false)?;
            return Ok(match accessor {
                Some(true) => ClassMember::Getter { key, value: std::rc::Rc::new(f), is_static },
                Some(false) => ClassMember::Setter { key, value: std::rc::Rc::new(f), is_static },
                None => ClassMember::Method { key, value: std::rc::Rc::new(f), is_static },
            });
        }
        if is_async || is_generator || accessor.is_some() {
            return Err(self.unexpected());
        }
        // Field
        if matches!(&key, PropertyKey::Identifier(n) if n == "constructor") || matches!(&key, PropertyKey::String(s) if s == "constructor") {
            return Err(self.error_at(pos, "Classes may not have a field named 'constructor'"));
        }
        let value = if self.eat_punct("=")? {
            let ctx = Context {
                in_function: false,
                in_generator: false,
                in_async: false,
                in_loop: false,
                in_switch: false,
                strict: true,
                is_module: self.ctx.is_module,
                labels: Vec::new(),
                in_class_body: true,
                super_call_ok: false,
                super_prop_ok: true,
            };
            Some(self.with_ctx(ctx, |p| p.parse_assignment())?)
        } else {
            None
        };
        self.consume_semicolon()?;
        Ok(ClassMember::Field { key, value, is_static })
    }
}

enum BinOrLogical {
    Binary(BinaryOp),
    Logical(LogicalOp),
}
