//! Pre-parser for lazy parsing: skips a function body without building
//! its tree.
//!
//! The scanner works on bytes and tracks what decides where the body
//! ends: brackets, string and template literals (with nested `${}`),
//! comments (including HTML-like ones in scripts) and regular expression
//! literals. Whether a `/` starts a regular expression depends on the
//! grammar; the scanner decides from the previous token like the "reader"
//! approach of sweet.js: a `/` after an operand (identifier, literal, `)`
//! of a call, `]`, `}` of an object literal, `++`/`--`) divides; anywhere
//! else it starts a regular expression. The kind of each `{` (block or
//! object literal) and of each `(` (control-statement header or not) is
//! recorded when it opens so its closing bracket can answer the question.
//!
//! It also collects the identifiers the body mentions (the capture
//! analysis of enclosing functions is name based) and the directive
//! prologue's `"use strict"`.
//!
//! Anything unusual (non-ASCII identifiers, escapes in identifiers,
//! malformed literals) makes it give up: the caller then parses the body
//! normally, so the pre-parser never has to be complete, only right when it
//! answers.

use super::{Lexer, LexerState};
use crate::ast::{Name, Pos};
use std::collections::HashSet;

/// What [`Lexer::skip_function_body`] learned about a skipped body.
pub struct SkippedBody {
    /// Lexer state just past the closing `}`.
    pub end: LexerState,
    /// Position of the closing `}`.
    pub close_pos: Pos,
    /// Distinct identifiers mentioned (not property names after `.`).
    pub names: Vec<Name>,
    /// `eval` is mentioned.
    pub eval: bool,
    /// The body starts with a `"use strict"` directive.
    pub use_strict: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    /// `(`: whether it is the header of `if`/`while`/`for`/`with` (a `/`
    /// after its `)` starts a regular expression).
    Paren(bool),
    Bracket,
    /// `{` of a block / function body / class body.
    Block,
    /// `{` of an object literal (or other expression braces).
    Object,
    /// `${` of a template literal.
    Template,
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

/// Reserved words that are never binding names.
fn is_keyword(w: &[u8]) -> bool {
    matches!(
        w,
        b"break" | b"case" | b"catch" | b"class" | b"const" | b"continue" | b"debugger" | b"default" | b"delete" | b"do" | b"else" | b"export" | b"extends" | b"false" | b"finally" | b"for" | b"function" | b"if" | b"import" | b"in" | b"instanceof" | b"new" | b"null" | b"return" | b"super" | b"switch" | b"this" | b"throw" | b"true" | b"try" | b"typeof" | b"var" | b"void" | b"while" | b"with"
    )
}

/// Words after which a `/` starts a regular expression and a `{` an
/// object literal (they are followed by an expression).
fn precedes_expression(w: &[u8]) -> bool {
    matches!(w, b"return" | b"typeof" | b"instanceof" | b"in" | b"of" | b"new" | b"delete" | b"void" | b"throw" | b"case" | b"yield" | b"await" | b"extends")
}

/// Words after which a `{` opens a block and a `/` a regular expression.
fn precedes_block(w: &[u8]) -> bool {
    matches!(w, b"else" | b"do" | b"try" | b"finally" | b"static")
}

impl<'a> Lexer<'a> {
    /// Skip a function body whose `{` was the last token scanned (the
    /// lexer is just past it). On success the lexer is just past the
    /// matching `}`; on `None` it is unchanged.
    pub fn skip_function_body(&mut self) -> Option<SkippedBody> {
        let save = self.state();
        let r = self.prescan();
        if r.is_none() {
            self.restore(&save);
        }
        r
    }

    fn prescan(&mut self) -> Option<SkippedBody> {
        let b = self.bytes;
        let mut i = self.pos;
        let mut line = self.line;
        let mut line_start = self.line_start;
        let mut levels: Vec<Level> = vec![Level::Block];
        // Pending `?` of conditionals, per level.
        let mut ternary: Vec<u32> = vec![0];
        // A `/` here starts a regular expression.
        let mut regex_ok = true;
        // A `{` here opens a block (else an object literal).
        let mut brace_block = true;
        // The previous token was `if`/`while`/`for`/`with` (or `for await`).
        let mut header_kw = false;
        // The previous token was `.` / `?.` (next identifier is a property).
        let mut after_dot = false;
        // Only trivia since the last line terminator (for `-->`).
        let mut at_line_start = false;
        let mut names: HashSet<&'a str, crate::value::FxBuild> = HashSet::default();
        let mut eval = false;
        let src = self.src;

        macro_rules! newline {
            ($after:expr) => {{
                line += 1;
                line_start = $after;
                at_line_start = true;
            }};
        }

        let use_strict = self.directive_prologue_strict()?;

        loop {
            let c = *b.get(i)?;
            match c {
                b' ' | b'\t' | 0x0b | 0x0c => i += 1,
                b'\n' => {
                    i += 1;
                    newline!(i);
                }
                b'\r' => {
                    i += 1;
                    if b.get(i) == Some(&b'\n') {
                        i += 1;
                    }
                    newline!(i);
                }
                0xE2 if b.get(i + 1) == Some(&0x80) && matches!(b.get(i + 2), Some(0xA8) | Some(0xA9)) => {
                    i += 3;
                    newline!(i);
                }
                0xC2 if b.get(i + 1) == Some(&0xA0) => i += 2,
                0xEF if b.get(i + 1) == Some(&0xBB) && b.get(i + 2) == Some(&0xBF) => i += 3,
                b'/' if b.get(i + 1) == Some(&b'/') => {
                    i = skip_line(b, i);
                }
                b'/' if b.get(i + 1) == Some(&b'*') => {
                    i += 2;
                    loop {
                        match *b.get(i)? {
                            b'*' if b.get(i + 1) == Some(&b'/') => {
                                i += 2;
                                break;
                            }
                            b'\n' => {
                                i += 1;
                                newline!(i);
                            }
                            b'\r' => {
                                i += 1;
                                if b.get(i) == Some(&b'\n') {
                                    i += 1;
                                }
                                newline!(i);
                            }
                            0xE2 if b.get(i + 1) == Some(&0x80) && matches!(b.get(i + 2), Some(0xA8) | Some(0xA9)) => {
                                i += 3;
                                newline!(i);
                            }
                            _ => i += 1,
                        }
                    }
                }
                b'<' if self.html_comments && b[i..].starts_with(b"<!--") => {
                    i = skip_line(b, i);
                }
                b'-' if self.html_comments && at_line_start && b[i..].starts_with(b"-->") => {
                    i = skip_line(b, i);
                }
                _ => {
                    // A token.
                    at_line_start = false;
                    let dot = std::mem::replace(&mut after_dot, false);
                    let header = std::mem::replace(&mut header_kw, false);
                    match c {
                        b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'$' => {
                            let s = i;
                            while i < b.len() && is_ident_byte(b[i]) {
                                i += 1;
                            }
                            if matches!(b.get(i), Some(&x) if x >= 0x80 || x == b'\\') {
                                return None;
                            }
                            let w = &b[s..i];
                            if dot {
                                regex_ok = false;
                                brace_block = true;
                            } else if is_keyword(w) || matches!(w, b"of" | b"yield" | b"await" | b"static" | b"let") {
                                if matches!(w, b"if" | b"while" | b"for" | b"with") || (w == b"await" && header) {
                                    header_kw = true;
                                }
                                if precedes_expression(w) {
                                    regex_ok = true;
                                    brace_block = false;
                                } else if precedes_block(w) {
                                    regex_ok = true;
                                    brace_block = true;
                                } else if matches!(w, b"this" | b"super" | b"null" | b"true" | b"false" | b"let") {
                                    regex_ok = false;
                                    brace_block = true;
                                } else {
                                    regex_ok = true;
                                    brace_block = true;
                                }
                                if !is_keyword(w) {
                                    // Contextual keywords can be names.
                                    names.insert(&src[s..i]);
                                }
                            } else {
                                if w == b"eval" {
                                    eval = true;
                                }
                                names.insert(&src[s..i]);
                                regex_ok = false;
                                brace_block = true;
                            }
                        }
                        b'0'..=b'9' => {
                            i = skip_number(b, i);
                            regex_ok = false;
                            brace_block = true;
                        }
                        b'.' if matches!(b.get(i + 1), Some(b'0'..=b'9')) => {
                            i = skip_number(b, i);
                            regex_ok = false;
                            brace_block = true;
                        }
                        b'"' | b'\'' => {
                            i += 1;
                            loop {
                                match *b.get(i)? {
                                    x if x == c => {
                                        i += 1;
                                        break;
                                    }
                                    b'\\' => {
                                        i += 1;
                                        match *b.get(i)? {
                                            b'\r' => {
                                                i += 1;
                                                if b.get(i) == Some(&b'\n') {
                                                    i += 1;
                                                }
                                                newline!(i);
                                            }
                                            b'\n' => {
                                                i += 1;
                                                newline!(i);
                                            }
                                            0xE2 if b.get(i + 1) == Some(&0x80) && matches!(b.get(i + 2), Some(0xA8) | Some(0xA9)) => {
                                                i += 3;
                                                newline!(i);
                                            }
                                            _ => i += 1,
                                        }
                                    }
                                    b'\n' | b'\r' => return None,
                                    0xE2 if b.get(i + 1) == Some(&0x80) && matches!(b.get(i + 2), Some(0xA8) | Some(0xA9)) => {
                                        // Allowed in strings (ES2019), but
                                        // keep line accounting simple.
                                        return None;
                                    }
                                    _ => i += 1,
                                }
                            }
                            regex_ok = false;
                            brace_block = true;
                        }
                        b'`' => {
                            i += 1;
                            match scan_template(b, &mut i, &mut line, &mut line_start)? {
                                true => {
                                    levels.push(Level::Template);
                                    ternary.push(0);
                                    regex_ok = true;
                                    brace_block = false;
                                }
                                false => {
                                    regex_ok = false;
                                    brace_block = true;
                                }
                            }
                        }
                        b'#' => {
                            // Private name.
                            let s = i;
                            i += 1;
                            while i < b.len() && is_ident_byte(b[i]) {
                                i += 1;
                            }
                            if i == s + 1 || matches!(b.get(i), Some(&x) if x >= 0x80 || x == b'\\') {
                                return None;
                            }
                            names.insert(&src[s..i]);
                            regex_ok = false;
                            brace_block = true;
                        }
                        b'/' if regex_ok => {
                            i += 1;
                            let mut class = false;
                            loop {
                                match *b.get(i)? {
                                    b'\\' => {
                                        i += 1;
                                        match *b.get(i)? {
                                            b'\n' | b'\r' => return None,
                                            _ => i += 1,
                                        }
                                    }
                                    b'[' => {
                                        class = true;
                                        i += 1;
                                    }
                                    b']' => {
                                        class = false;
                                        i += 1;
                                    }
                                    b'/' if !class => {
                                        i += 1;
                                        break;
                                    }
                                    b'\n' | b'\r' => return None,
                                    _ => i += 1,
                                }
                            }
                            while i < b.len() && is_ident_byte(b[i]) {
                                i += 1;
                            }
                            regex_ok = false;
                            brace_block = true;
                        }
                        b'(' => {
                            i += 1;
                            levels.push(Level::Paren(header));
                            ternary.push(0);
                            regex_ok = true;
                            brace_block = false;
                        }
                        b'[' => {
                            i += 1;
                            levels.push(Level::Bracket);
                            ternary.push(0);
                            regex_ok = true;
                            brace_block = false;
                        }
                        b'{' => {
                            i += 1;
                            levels.push(if brace_block { Level::Block } else { Level::Object });
                            ternary.push(0);
                            regex_ok = true;
                            brace_block = true;
                        }
                        b')' => {
                            i += 1;
                            let Level::Paren(h) = levels.pop()? else { return None };
                            ternary.pop();
                            regex_ok = h;
                            brace_block = true;
                        }
                        b']' => {
                            i += 1;
                            if levels.pop()? != Level::Bracket {
                                return None;
                            }
                            ternary.pop();
                            regex_ok = false;
                            brace_block = true;
                        }
                        b'}' => {
                            let close = i;
                            i += 1;
                            ternary.pop();
                            match levels.pop()? {
                                Level::Block => {
                                    if levels.is_empty() {
                                        let close_pos = Pos { line, column: (close - line_start) as u32 + 1 };
                                        let mut list: Vec<Name> = Vec::with_capacity(names.len());
                                        for n in names {
                                            list.push(match self.names.get(n) {
                                                Some(n) => n.clone(),
                                                None => {
                                                    let name = Name::new(n);
                                                    self.names.insert(n, name.clone());
                                                    name
                                                }
                                            });
                                        }
                                        self.pos = i;
                                        self.line = line;
                                        self.line_start = line_start;
                                        return Some(SkippedBody { end: self.state(), close_pos, names: list, eval, use_strict });
                                    }
                                    regex_ok = true;
                                    brace_block = true;
                                }
                                Level::Object => {
                                    regex_ok = false;
                                    brace_block = true;
                                }
                                Level::Template => match scan_template(b, &mut i, &mut line, &mut line_start)? {
                                    true => {
                                        levels.push(Level::Template);
                                        ternary.push(0);
                                        regex_ok = true;
                                        brace_block = false;
                                    }
                                    false => {
                                        regex_ok = false;
                                        brace_block = true;
                                    }
                                },
                                _ => return None,
                            }
                        }
                        b'?' => {
                            if b.get(i + 1) == Some(&b'.') && !matches!(b.get(i + 2), Some(b'0'..=b'9')) {
                                i += 2;
                                after_dot = true;
                            } else if b.get(i + 1) == Some(&b'?') {
                                i += if b.get(i + 2) == Some(&b'=') { 3 } else { 2 };
                            } else {
                                i += 1;
                                *ternary.last_mut()? += 1;
                            }
                            regex_ok = true;
                            brace_block = false;
                        }
                        b':' => {
                            i += 1;
                            let t = ternary.last_mut()?;
                            if *t > 0 {
                                *t -= 1;
                                brace_block = false;
                            } else {
                                // Object literal value, or a label / case.
                                brace_block = *levels.last()? != Level::Block;
                            }
                            regex_ok = true;
                        }
                        b'.' => {
                            if b[i..].starts_with(b"...") {
                                i += 3;
                                regex_ok = true;
                                brace_block = false;
                            } else {
                                i += 1;
                                after_dot = true;
                                regex_ok = false;
                            }
                        }
                        b'=' if b.get(i + 1) == Some(&b'>') => {
                            i += 2;
                            regex_ok = true;
                            brace_block = true;
                        }
                        b'+' | b'-' if b.get(i + 1) == Some(&c) => {
                            i += 2;
                            regex_ok = false;
                            brace_block = true;
                        }
                        b';' => {
                            i += 1;
                            regex_ok = true;
                            brace_block = true;
                        }
                        b',' | b'=' | b'!' | b'~' | b'<' | b'>' | b'&' | b'|' | b'^' | b'%' | b'*' | b'+' | b'-' | b'/' | b'@' => {
                            i += 1;
                            regex_ok = true;
                            brace_block = false;
                        }
                        _ => return None,
                    }
                }
            }
        }
    }

    /// Does the directive prologue of the body starting at `self.pos` (just
    /// past `{`) contain `"use strict"`? `None` when the prologue is not
    /// simple to read (the caller then parses normally).
    fn directive_prologue_strict(&self) -> Option<bool> {
        let b = self.bytes;
        let mut i = self.pos;
        let mut strict = false;
        loop {
            i = skip_blank(b, i)?;
            let q = *b.get(i)?;
            if q != b'"' && q != b'\'' {
                return Some(strict);
            }
            let s = i;
            i += 1;
            while *b.get(i)? != q {
                if matches!(b[i], b'\\' | b'\n' | b'\r') {
                    return None;
                }
                i += 1;
            }
            i += 1;
            let raw = &b[s + 1..i - 1];
            // Exactly a string statement?
            let j = skip_blank(b, i)?;
            match *b.get(j)? {
                b';' => i = j + 1,
                b'}' => i = j,
                _ if b[i..j].iter().any(|&x| x == b'\n' || x == b'\r') => {
                    // ASI only when the next line cannot continue the
                    // expression: keep it simple.
                    let n = b[j];
                    if !(n.is_ascii_alphabetic() || n == b'"' || n == b'\'' || n == b'_' || n == b'$') {
                        return None;
                    }
                    i = j;
                }
                _ => return Some(strict),
            }
            if raw == b"use strict" {
                strict = true;
            }
        }
    }
}

/// Skip spaces, line terminators and comments (ASCII only; `None` on
/// anything else).
fn skip_blank(b: &[u8], mut i: usize) -> Option<usize> {
    loop {
        match *b.get(i)? {
            b' ' | b'\t' | b'\n' | b'\r' => i += 1,
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while !matches!(*b.get(i)?, b'\n' | b'\r') {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while !(*b.get(i)? == b'*' && b.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i += 2;
            }
            x if x >= 0x80 => return None,
            _ => return Some(i),
        }
    }
}

/// Index of the line terminator ending the line comment at `i` (or the end).
fn skip_line(b: &[u8], mut i: usize) -> usize {
    while i < b.len() {
        match b[i] {
            b'\n' | b'\r' => break,
            0xE2 if b.get(i + 1) == Some(&0x80) && matches!(b.get(i + 2), Some(0xA8) | Some(0xA9)) => break,
            _ => i += 1,
        }
    }
    i
}

fn skip_number(b: &[u8], mut i: usize) -> usize {
    let hex = b.get(i) == Some(&b'0') && matches!(b.get(i + 1), Some(b'x') | Some(b'X'));
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_alphanumeric() || c == b'_' || c == b'.' {
            i += 1;
            if !hex && (c == b'e' || c == b'E') && matches!(b.get(i), Some(b'+') | Some(b'-')) {
                i += 1;
            }
        } else {
            break;
        }
    }
    i
}

/// Scan template characters from `i` (just past a backtick or the `}` of a
/// substitution). Returns `Some(true)` at a `${` (just past it) and
/// `Some(false)` at the closing backtick (just past it).
fn scan_template(b: &[u8], i: &mut usize, line: &mut u32, line_start: &mut usize) -> Option<bool> {
    loop {
        match *b.get(*i)? {
            b'`' => {
                *i += 1;
                return Some(false);
            }
            b'$' if b.get(*i + 1) == Some(&b'{') => {
                *i += 2;
                return Some(true);
            }
            b'\\' => {
                *i += 1;
                match *b.get(*i)? {
                    b'\n' | b'\r' | 0xE2 => {}
                    _ => *i += 1,
                }
            }
            b'\n' => {
                *i += 1;
                *line += 1;
                *line_start = *i;
            }
            b'\r' => {
                *i += 1;
                if b.get(*i) == Some(&b'\n') {
                    *i += 1;
                }
                *line += 1;
                *line_start = *i;
            }
            0xE2 if b.get(*i + 1) == Some(&0x80) && matches!(b.get(*i + 2), Some(0xA8) | Some(0xA9)) => {
                *i += 3;
                *line += 1;
                *line_start = *i;
            }
            _ => *i += 1,
        }
    }
}
