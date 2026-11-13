//! ECMAScript tokenizer.
//!
//! The lexer is driven by the parser: it produces tokens on demand and can
//! be asked to *re-scan* from a token start when the grammar context
//! decides that a `/` begins a regular expression literal or that a `}`
//! continues a template literal. Tokens remember whether a line terminator
//! preceded them, which the parser needs for automatic semicolon insertion.

use crate::ast::{Name, Pos};

#[path = "prescan.rs"]
mod prescan;
pub use prescan::BlockEnd;
pub use prescan::SkippedBody;

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    /// Identifier or keyword (keywords are classified by the parser).
    Ident(Name),
    /// `#name`
    PrivateName(Name),
    Number(f64),
    BigInt(String),
    /// Cooked string value.
    Str(String),
    /// A template chunk: no-substitution template, template head (`...${`),
    /// middle or tail. `tail` is true when the chunk ends with a backtick.
    Template {
        cooked: Option<String>,
        raw: String,
        tail: bool,
    },
    Regex {
        pattern: String,
        flags: String,
    },
    Punct(&'static str),
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    /// Byte offsets into the source.
    pub start: usize,
    pub end: usize,
    pub pos: Pos,
    /// A line terminator occurred between the previous token and this one.
    pub newline_before: bool,
}

impl Token {
    pub fn is_punct(&self, p: &str) -> bool {
        matches!(&self.kind, TokenKind::Punct(q) if *q == p)
    }
    /// The token is the identifier `name` written without escapes (as
    /// keywords must be).
    pub fn is_ident(&self, name: &str) -> bool {
        matches!(&self.kind, TokenKind::Ident(n) if n == name && self.end - self.start == n.len())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LexError {
    pub message: String,
    pub pos: Pos,
}

/// Snapshot of the lexer state for speculative parsing.
#[derive(Debug, Clone, Copy)]
pub struct LexerState {
    pos: usize,
    line: u32,
    line_start: usize,
}

impl LexerState {
    /// The state at byte offset `pos` of line `line` (starting at byte
    /// `line_start`).
    pub fn at(pos: usize, line: u32, line_start: usize) -> LexerState {
        LexerState { pos, line, line_start }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }
}

pub struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    line: u32,
    line_start: usize,
    /// Scripts (not modules) honour HTML-like comments.
    html_comments: bool,
    /// Names seen so far: each distinct identifier is allocated once.
    names: std::collections::HashMap<&'a str, Name, crate::value::FxBuild>,
    /// While set, scanned tokens are kept in `cache`.
    recording: bool,
    /// Tokens scanned ahead by the parser (a cheap lookahead pass), by the
    /// position the scan started from, with the lexer state after them:
    /// the parse proper reuses them instead of scanning again. A token
    /// depends only on where scanning starts, so entries stay valid.
    cache: std::collections::HashMap<usize, (Token, LexerState), crate::value::FxBuild>,
    /// The escape just scanned was a lone surrogate (`\uD800`): the
    /// caller appends it in source-escaped form (see `value::SOURCE_ESC`).
    lone_surrogate: Option<u16>,
    /// When set, the pre-parser records the long blocks inside the bodies
    /// it skips (see [`prescan::BlockEnd`]).
    pub blocks: Option<Vec<prescan::BlockEnd>>,
}

// Punctuators ordered longest-first so greedy matching works.
const PUNCTS: &[&str] = &[
    ">>>=", "...", "===", "!==", "**=", "<<=", ">>=", ">>>", "&&=", "||=", "??=", "=>", "==", "!=", "<=", ">=",
    "&&", "||", "??", "?.", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<", ">>", "**", "{",
    "}", "(", ")", "[", "]", ";", ",", "<", ">", "+", "-", "*", "/", "%", "&", "|", "^", "!", "~", "?", ":",
    "=", ".", "@",
];

fn is_id_start(c: char) -> bool {
    if c.is_ascii() {
        return c == '$' || c == '_' || c.is_ascii_alphabetic();
    }
    // Unicode ID_Start (Unicode 17 tables shared with the regex engine).
    crate::regex::is_unicode_id_start(c as u32)
}

fn is_id_part(c: char) -> bool {
    if c.is_ascii() {
        return c == '$' || c == '_' || c.is_ascii_alphanumeric();
    }
    c == '\u{200c}' || c == '\u{200d}' || crate::regex::is_unicode_id_continue(c as u32)
}

fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str, is_module: bool) -> Lexer<'a> {
        let mut lx = Lexer {
            src,
            bytes: src.as_bytes(),
            pos: 0,
            line: 1,
            line_start: 0,
            html_comments: !is_module,
            names: std::collections::HashMap::default(),
            recording: false,
            cache: std::collections::HashMap::default(),
            lone_surrogate: None,
            blocks: None,
        };
        // Hashbang
        if src.starts_with("#!") {
            while let Some(c) = lx.peek_char() {
                if is_line_terminator(c) {
                    break;
                }
                lx.pos += c.len_utf8();
            }
        }
        lx
    }

    pub fn source(&self) -> &'a str {
        self.src
    }

    /// Keep the tokens scanned from now on for reuse (see `cache`).
    pub fn set_recording(&mut self, on: bool) {
        self.recording = on;
        if on && self.cache.len() > 1 << 16 {
            self.cache.clear();
        }
    }

    pub fn state(&self) -> LexerState {
        LexerState {
            pos: self.pos,
            line: self.line,
            line_start: self.line_start,
        }
    }

    pub fn restore(&mut self, st: &LexerState) {
        self.pos = st.pos;
        self.line = st.line;
        self.line_start = st.line_start;
    }

    fn cur_pos(&self) -> Pos {
        Pos {
            line: self.line,
            column: (self.pos - self.line_start) as u32 + 1,
        }
    }

    fn peek_char(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn peek_byte(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn byte_at(&self, off: usize) -> Option<u8> {
        self.bytes.get(self.pos + off).copied()
    }

    fn newline(&mut self, c: char) {
        // CR LF counts as one terminator.
        if c == '\r' && self.peek_byte() == Some(b'\n') {
            self.pos += 1;
        }
        self.line += 1;
        self.line_start = self.pos;
    }

    fn err(&self, message: impl Into<String>) -> LexError {
        LexError {
            message: message.into(),
            pos: self.cur_pos(),
        }
    }

    /// First byte of the next token when only ASCII blanks separate it
    /// (`Some(0)` at the end of input); `None` when a comment or non-ASCII
    /// character makes a simple scan unreliable.
    pub fn next_significant_byte(&self) -> Option<u8> {
        let b = self.bytes;
        let mut i = self.pos;
        loop {
            match b.get(i) {
                None => return Some(0),
                Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r') => i += 1,
                Some(c) if *c >= 0x80 || *c == b'/' => return None,
                Some(c) => return Some(*c),
            }
        }
    }

    /// After the current token: is the next token `=>` on the same line?
    /// `None` when that cannot be decided by a simple ASCII scan.
    pub fn next_is_arrow(&self) -> Option<bool> {
        let b = self.bytes;
        let mut i = self.pos;
        loop {
            match b.get(i) {
                Some(b' ') | Some(b'\t') => i += 1,
                Some(b'=') => return Some(b.get(i + 1) == Some(&b'>')),
                Some(b'\n') | Some(b'\r') => return Some(false),
                Some(c) if *c >= 0x80 || *c == b'/' => return None,
                _ => return Some(false),
            }
        }
    }

    /// Skip whitespace and comments. Returns true if a line terminator was
    /// crossed.
    fn skip_trivia(&mut self) -> Result<bool, LexError> {
        let mut newline = false;
        let mut at_line_start = self.pos == self.line_start || self.pos == 0;
        loop {
            // ASCII blanks are by far the most common trivia.
            while matches!(self.bytes.get(self.pos), Some(b' ') | Some(b'\t')) {
                self.pos += 1;
            }
            // Most tokens start right here: no trivia left to look at.
            match self.bytes.get(self.pos) {
                Some(&b) if b < 0x80 && !matches!(b, b'\n' | b'\r' | 0x0b | 0x0c | b'/' | b'<' | b'-') => return Ok(newline),
                _ => {}
            }
            let Some(c) = self.peek_char() else { return Ok(newline) };
            match c {
                '\n' | '\r' | '\u{2028}' | '\u{2029}' => {
                    self.pos += c.len_utf8();
                    self.newline(c);
                    newline = true;
                    at_line_start = true;
                }
                ' ' | '\t' | '\u{b}' | '\u{c}' | '\u{a0}' | '\u{feff}' => {
                    self.pos += c.len_utf8();
                }
                c if c.is_whitespace() => {
                    self.pos += c.len_utf8();
                }
                '/' if self.byte_at(1) == Some(b'/') => {
                    self.skip_line_comment();
                }
                '/' if self.byte_at(1) == Some(b'*') => {
                    self.pos += 2;
                    let mut closed = false;
                    while let Some(c) = self.peek_char() {
                        if c == '*' && self.byte_at(1) == Some(b'/') {
                            self.pos += 2;
                            closed = true;
                            break;
                        }
                        self.pos += c.len_utf8();
                        if is_line_terminator(c) {
                            self.newline(c);
                            newline = true;
                            at_line_start = true;
                        }
                    }
                    if !closed {
                        return Err(self.err("Unterminated comment"));
                    }
                }
                '<' if self.html_comments && self.src[self.pos..].starts_with("<!--") => {
                    self.skip_line_comment();
                }
                '-' if self.html_comments && at_line_start && self.src[self.pos..].starts_with("-->") => {
                    self.skip_line_comment();
                }
                _ => return Ok(newline),
            }
        }
    }

    fn skip_line_comment(&mut self) {
        while let Some(c) = self.peek_char() {
            if is_line_terminator(c) {
                break;
            }
            self.pos += c.len_utf8();
        }
    }

    /// Scan the next token in the "division allowed" goal (regex literals
    /// are produced by [`Lexer::rescan_regex`] on parser request).
    pub fn next_token(&mut self) -> Result<Token, LexError> {
        if !self.cache.is_empty() {
            if let Some((t, st)) = self.cache.remove(&self.pos) {
                self.restore(&st);
                return Ok(t);
            }
        }
        if self.recording {
            let from = self.pos;
            let t = self.scan_next()?;
            self.cache.insert(from, (t.clone(), self.state()));
            return Ok(t);
        }
        self.scan_next()
    }

    fn scan_next(&mut self) -> Result<Token, LexError> {
        let newline_before = self.skip_trivia()?;
        let start = self.pos;
        let pos = self.cur_pos();
        let kind = match self.bytes.get(self.pos) {
            None => TokenKind::Eof,
            // ASCII identifiers / keywords without decoding a char first.
            Some(&b) if b.is_ascii_alphabetic() || b == b'_' || b == b'$' => TokenKind::Ident(self.scan_identifier_name()?),
            Some(&b) if b < 0x80 => self.scan_token(b as char)?,
            Some(_) => match self.peek_char() {
                Some(c) => self.scan_token(c)?,
                None => TokenKind::Eof,
            },
        };
        Ok(Token {
            kind,
            start,
            end: self.pos,
            pos,
            newline_before,
        })
    }

    fn scan_token(&mut self, c: char) -> Result<TokenKind, LexError> {
        if c == '"' || c == '\'' {
            return self.scan_string(c);
        }
        if c == '`' {
            self.pos += 1;
            return self.scan_template_chunk();
        }
        if c.is_ascii_digit() || (c == '.' && matches!(self.byte_at(1), Some(b'0'..=b'9'))) {
            return self.scan_number();
        }
        if c == '#' {
            self.pos += 1;
            let name = self.scan_identifier_name()?;
            if name.is_empty() {
                return Err(self.err("Invalid private name"));
            }
            return Ok(TokenKind::PrivateName(name));
        }
        if is_id_start(c) || c == '\\' {
            let name = self.scan_identifier_name()?;
            return Ok(TokenKind::Ident(name));
        }
        if let Some(p) = self.scan_punct() {
            return Ok(TokenKind::Punct(p));
        }
        let b0 = self.bytes[self.pos];
        for p in PUNCTS {
            if p.as_bytes()[0] == b0 && self.src[self.pos..].starts_with(p) {
                // `?.` followed by a decimal digit is `?` then a number
                // (`a ?.5 : b`), per OptionalChainingPunctuator.
                if *p == "?." && self.src.as_bytes().get(self.pos + 2).map(|b| b.is_ascii_digit()).unwrap_or(false) {
                    continue;
                }
                self.pos += p.len();
                return Ok(TokenKind::Punct(p));
            }
        }
        Err(self.err(format!("Unexpected character '{c}'")))
    }

    /// Punctuator at the current position (longest match), decided on bytes.
    fn scan_punct(&mut self) -> Option<&'static str> {
        let b = self.bytes;
        let p = self.pos;
        let at = |i: usize| b.get(p + i).copied().unwrap_or(0);
        let (c1, c2, c3) = (at(1), at(2), at(3));
        let s: &'static str = match at(0) {
            b'{' => "{",
            b'}' => "}",
            b'(' => "(",
            b')' => ")",
            b'[' => "[",
            b']' => "]",
            b';' => ";",
            b',' => ",",
            b'~' => "~",
            b':' => ":",
            b'@' => "@",
            b'.' => {
                if c1 == b'.' && c2 == b'.' {
                    "..."
                } else {
                    "."
                }
            }
            b'?' => {
                if c1 == b'?' {
                    if c2 == b'=' {
                        "??="
                    } else {
                        "??"
                    }
                } else if c1 == b'.' && !c2.is_ascii_digit() {
                    "?."
                } else {
                    "?"
                }
            }
            b'=' => {
                if c1 == b'=' {
                    if c2 == b'=' {
                        "==="
                    } else {
                        "=="
                    }
                } else if c1 == b'>' {
                    "=>"
                } else {
                    "="
                }
            }
            b'!' => {
                if c1 == b'=' {
                    if c2 == b'=' {
                        "!=="
                    } else {
                        "!="
                    }
                } else {
                    "!"
                }
            }
            b'<' => {
                if c1 == b'<' {
                    if c2 == b'=' {
                        "<<="
                    } else {
                        "<<"
                    }
                } else if c1 == b'=' {
                    "<="
                } else {
                    "<"
                }
            }
            b'>' => {
                if c1 == b'>' {
                    if c2 == b'>' {
                        if c3 == b'=' {
                            ">>>="
                        } else {
                            ">>>"
                        }
                    } else if c2 == b'=' {
                        ">>="
                    } else {
                        ">>"
                    }
                } else if c1 == b'=' {
                    ">="
                } else {
                    ">"
                }
            }
            b'+' => {
                if c1 == b'+' {
                    "++"
                } else if c1 == b'=' {
                    "+="
                } else {
                    "+"
                }
            }
            b'-' => {
                if c1 == b'-' {
                    "--"
                } else if c1 == b'=' {
                    "-="
                } else {
                    "-"
                }
            }
            b'*' => {
                if c1 == b'*' {
                    if c2 == b'=' {
                        "**="
                    } else {
                        "**"
                    }
                } else if c1 == b'=' {
                    "*="
                } else {
                    "*"
                }
            }
            b'/' => {
                if c1 == b'=' {
                    "/="
                } else {
                    "/"
                }
            }
            b'%' => {
                if c1 == b'=' {
                    "%="
                } else {
                    "%"
                }
            }
            b'&' => {
                if c1 == b'&' {
                    if c2 == b'=' {
                        "&&="
                    } else {
                        "&&"
                    }
                } else if c1 == b'=' {
                    "&="
                } else {
                    "&"
                }
            }
            b'|' => {
                if c1 == b'|' {
                    if c2 == b'=' {
                        "||="
                    } else {
                        "||"
                    }
                } else if c1 == b'=' {
                    "|="
                } else {
                    "|"
                }
            }
            b'^' => {
                if c1 == b'=' {
                    "^="
                } else {
                    "^"
                }
            }
            _ => return None,
        };
        self.pos += s.len();
        Some(s)
    }

    fn scan_identifier_name(&mut self) -> Result<Name, LexError> {
        // ASCII fast path: the whole name is [A-Za-z0-9_$].
        {
            let b = self.bytes;
            let start = self.pos;
            let mut i = start;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$') {
                i += 1;
            }
            let at_end = i >= b.len() || (b[i] < 0x80 && b[i] != b'\\');
            if i > start && at_end && !b[start].is_ascii_digit() {
                self.pos = i;
                let src: &'a str = self.src;
                let text = &src[start..i];
                if let Some(n) = self.names.get(text) {
                    return Ok(n.clone());
                }
                let n = Name::new(text);
                self.names.insert(text, n.clone());
                return Ok(n);
            }
        }
        let mut name = String::new();
        let mut first = true;
        loop {
            let Some(c) = self.peek_char() else { break };
            if c == '\\' {
                if self.byte_at(1) != Some(b'u') {
                    return Err(self.err("Invalid escape in identifier"));
                }
                self.pos += 2;
                let ch = self.scan_unicode_escape_body()?;
                let ok = self.lone_surrogate.take().is_none() && if first { is_id_start(ch) } else { is_id_part(ch) };
                if !ok {
                    return Err(self.err("Invalid identifier escape"));
                }
                name.push(ch);
            } else if (first && is_id_start(c)) || (!first && is_id_part(c)) {
                name.push(c);
                self.pos += c.len_utf8();
            } else {
                break;
            }
            first = false;
        }
        Ok(Name::from(name))
    }

    /// After `\u`: parse `XXXX` or `{X...}`.
    fn scan_unicode_escape_body(&mut self) -> Result<char, LexError> {
        if self.peek_byte() == Some(b'{') {
            self.pos += 1;
            let start = self.pos;
            while matches!(self.peek_byte(), Some(b) if b.is_ascii_hexdigit()) {
                self.pos += 1;
            }
            let hex = &self.src[start..self.pos];
            if self.peek_byte() != Some(b'}') || hex.is_empty() {
                return Err(self.err("Invalid Unicode escape sequence"));
            }
            self.pos += 1;
            let cp = u32::from_str_radix(hex, 16).map_err(|_| self.err("Invalid Unicode escape sequence"))?;
            if (0xd800..0xe000).contains(&cp) {
                self.lone_surrogate = Some(cp as u16);
                return Ok('\0');
            }
            return char::from_u32(cp).ok_or_else(|| self.err("Invalid Unicode escape sequence"));
        }
        let cp = self.scan_hex_digits(4)?;
        if (0xd800..0xe000).contains(&cp) {
            self.lone_surrogate = Some(cp as u16);
            return Ok('\0');
        }
        Ok(char::from_u32(cp).unwrap_or('\u{fffd}'))
    }

    fn scan_hex_digits(&mut self, n: usize) -> Result<u32, LexError> {
        let start = self.pos;
        for _ in 0..n {
            match self.peek_byte() {
                Some(b) if b.is_ascii_hexdigit() => self.pos += 1,
                _ => return Err(self.err("Invalid hexadecimal escape sequence")),
            }
        }
        Ok(u32::from_str_radix(&self.src[start..self.pos], 16).unwrap())
    }

    fn scan_number(&mut self) -> Result<TokenKind, LexError> {
        let start = self.pos;
        let c0 = self.peek_byte().unwrap();
        if c0 == b'0' {
            // No numeric separators in legacy octal / non-octal decimal
            // literals (`0_1`, `07_7`, `08_1`).
            let mut i = 1;
            while let Some(b) = self.byte_at(i) {
                if b == b'_' {
                    return Err(self.err("Numeric separators are not allowed after a leading 0"));
                }
                if !b.is_ascii_digit() {
                    break;
                }
                i += 1;
            }
            match self.byte_at(1) {
                Some(b'x') | Some(b'X') => return self.scan_radix_number(16, 2),
                Some(b'o') | Some(b'O') => return self.scan_radix_number(8, 2),
                Some(b'b') | Some(b'B') => return self.scan_radix_number(2, 2),
                Some(b'0'..=b'9') => {
                    // Legacy octal (or decimal if it contains 8/9).
                    let mut i = 1;
                    let mut octal = true;
                    while let Some(b) = self.byte_at(i) {
                        if b.is_ascii_digit() {
                            if b >= b'8' {
                                octal = false;
                            }
                            i += 1;
                        } else {
                            break;
                        }
                    }
                    if octal {
                        self.pos += 1;
                        return self.scan_radix_number(8, 0);
                    }
                }
                _ => {}
            }
        }
        // Plain integer literal: digits only, no fraction/exponent/suffix.
        {
            let b = self.bytes;
            let mut i = start;
            let mut v: u64 = 0;
            while i < b.len() && b[i].is_ascii_digit() {
                v = v.wrapping_mul(10).wrapping_add((b[i] - b'0') as u64);
                i += 1;
            }
            let n = i - start;
            let next = b.get(i).copied().unwrap_or(0);
            if n > 0 && n <= 15 && !(c0 == b'0' && n > 1) && !matches!(next, b'.' | b'e' | b'E' | b'n' | b'_') {
                self.pos = i;
                self.check_after_number()?;
                return Ok(TokenKind::Number(v as f64));
            }
        }
        // Decimal
        let mut text = String::new();
        self.scan_digits(&mut text, |b| b.is_ascii_digit())?;
        let mut is_float = false;
        if self.peek_byte() == Some(b'.') {
            is_float = true;
            self.pos += 1;
            text.push('.');
            self.scan_digits(&mut text, |b| b.is_ascii_digit())?;
        }
        if matches!(self.peek_byte(), Some(b'e') | Some(b'E')) {
            let save = self.pos;
            let mut exp = String::from("e");
            self.pos += 1;
            if matches!(self.peek_byte(), Some(b'+') | Some(b'-')) {
                exp.push(self.peek_byte().unwrap() as char);
                self.pos += 1;
            }
            let before = exp.len();
            self.scan_digits(&mut exp, |b| b.is_ascii_digit())?;
            if exp.len() == before {
                self.pos = save;
            } else {
                is_float = true;
                text.push_str(&exp);
            }
        }
        if !is_float && self.peek_byte() == Some(b'n') {
            if text.len() > 1 && text.starts_with('0') {
                return Err(self.err("Invalid BigInt literal"));
            }
            self.pos += 1;
            self.check_after_number()?;
            return Ok(TokenKind::BigInt(text));
        }
        self.check_after_number()?;
        let value: f64 = if text.starts_with('.') {
            format!("0{text}").parse().unwrap_or(f64::NAN)
        } else {
            text.parse().unwrap_or(f64::NAN)
        };
        let _ = start;
        Ok(TokenKind::Number(value))
    }

    fn scan_digits(&mut self, out: &mut String, ok: impl Fn(u8) -> bool) -> Result<(), LexError> {
        let mut last_sep = false;
        let mut any = false;
        while let Some(b) = self.peek_byte() {
            if ok(b) {
                out.push(b as char);
                self.pos += 1;
                last_sep = false;
                any = true;
            } else if b == b'_' && any && !last_sep && self.byte_at(1).map(&ok).unwrap_or(false) {
                self.pos += 1;
                last_sep = true;
            } else {
                break;
            }
        }
        Ok(())
    }

    fn scan_radix_number(&mut self, radix: u32, prefix_len: usize) -> Result<TokenKind, LexError> {
        self.pos += prefix_len;
        let mut text = String::new();
        self.scan_digits(&mut text, |b| (b as char).is_digit(radix))?;
        if text.is_empty() {
            return Err(self.err("Invalid number"));
        }
        if self.peek_byte() == Some(b'n') && prefix_len > 0 {
            self.pos += 1;
            self.check_after_number()?;
            // Convert to decimal digits for the BigInt payload.
            let mut value: Vec<u32> = vec![0]; // little-endian base 1e9
            for ch in text.chars() {
                let d = ch.to_digit(radix).unwrap();
                let mut carry = d as u64;
                for limb in value.iter_mut() {
                    let v = *limb as u64 * radix as u64 + carry;
                    *limb = (v % 1_000_000_000) as u32;
                    carry = v / 1_000_000_000;
                }
                while carry > 0 {
                    value.push((carry % 1_000_000_000) as u32);
                    carry /= 1_000_000_000;
                }
            }
            let mut s = String::new();
            for (i, limb) in value.iter().rev().enumerate() {
                if i == 0 {
                    s.push_str(&limb.to_string());
                } else {
                    s.push_str(&format!("{limb:09}"));
                }
            }
            return Ok(TokenKind::BigInt(s));
        }
        self.check_after_number()?;
        let mut v = 0f64;
        for ch in text.chars() {
            v = v * radix as f64 + ch.to_digit(radix).unwrap() as f64;
        }
        Ok(TokenKind::Number(v))
    }

    fn check_after_number(&self) -> Result<(), LexError> {
        if let Some(c) = self.peek_char() {
            if is_id_start(c) || c.is_ascii_digit() {
                return Err(self.err("Identifier directly after number"));
            }
        }
        Ok(())
    }

    fn scan_string(&mut self, quote: char) -> Result<TokenKind, LexError> {
        self.pos += 1;
        let mut out = String::new();
        let q = quote as u8;
        loop {
            // Copy the run of ordinary characters in one go.
            let b = self.bytes;
            let start = self.pos;
            let mut i = start;
            while i < b.len() {
                let c = b[i];
                if c == q || c == b'\\' || c == b'\n' || c == b'\r' {
                    break;
                }
                i += 1;
            }
            if i > start {
                out.push_str(&self.src[start..i]);
                self.pos = i;
            }
            let Some(c) = self.peek_char() else {
                return Err(self.err("Unterminated string literal"));
            };
            match c {
                '\\' => {
                    self.pos += 1;
                    if let Some(ch) = self.scan_escape(false)? {
                        match self.lone_surrogate.take() {
                            Some(u) => crate::value::push_lone_surrogate(&mut out, u),
                            None => out.push(ch),
                        }
                    }
                }
                '\n' | '\r' => return Err(self.err("Unterminated string literal")),
                c if c == quote => {
                    self.pos += 1;
                    return Ok(TokenKind::Str(out));
                }
                c => {
                    out.push(c);
                    self.pos += c.len_utf8();
                }
            }
        }
    }

    /// After a backslash. Returns `None` for line continuations. In
    /// templates, invalid escapes yield an error the caller may turn into
    /// an undefined cooked value.
    fn scan_escape(&mut self, in_template: bool) -> Result<Option<char>, LexError> {
        let Some(c) = self.peek_char() else {
            return Err(self.err("Unterminated escape"));
        };
        self.pos += c.len_utf8();
        Ok(Some(match c {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            'b' => '\u{8}',
            'f' => '\u{c}',
            'v' => '\u{b}',
            '0' if !matches!(self.peek_byte(), Some(b'0'..=b'9')) => '\0',
            '0'..='7' => {
                if in_template {
                    return Err(self.err("Octal escape sequences are not allowed in template strings"));
                }
                let mut v = c.to_digit(8).unwrap();
                let max_len = if c <= '3' { 3 } else { 2 };
                let mut len = 1;
                while len < max_len {
                    match self.peek_byte() {
                        Some(b @ b'0'..=b'7') => {
                            v = v * 8 + (b - b'0') as u32;
                            self.pos += 1;
                            len += 1;
                        }
                        _ => break,
                    }
                }
                char::from_u32(v).unwrap()
            }
            '8' | '9' => {
                if in_template {
                    return Err(self.err("Invalid escape in template"));
                }
                c
            }
            'x' => {
                let v = self.scan_hex_digits(2)?;
                char::from_u32(v).unwrap()
            }
            'u' if self.peek_byte() != Some(b'{') => {
                let cp = self.scan_hex_digits(4)?;
                // A surrogate pair written as two escapes is one character.
                if (0xd800..0xdc00).contains(&cp) && self.src.as_bytes().get(self.pos..self.pos + 2) == Some(b"\\u".as_slice()) {
                    let save = self.pos;
                    self.pos += 2;
                    match self.scan_hex_digits(4) {
                        Ok(lo) if (0xdc00..0xe000).contains(&lo) => {
                            return Ok(Some(char::from_u32(0x10000 + ((cp - 0xd800) << 10) + (lo - 0xdc00)).unwrap_or('\u{fffd}')));
                        }
                        _ => self.pos = save,
                    }
                }
                if (0xd800..0xe000).contains(&cp) {
                    self.lone_surrogate = Some(cp as u16);
                    '\0'
                } else {
                    char::from_u32(cp).unwrap_or('\u{fffd}')
                }
            }
            'u' => self.scan_unicode_escape_body()?,
            '\r' => {
                if self.peek_byte() == Some(b'\n') {
                    self.pos += 1;
                }
                self.line += 1;
                self.line_start = self.pos;
                return Ok(None);
            }
            '\n' | '\u{2028}' | '\u{2029}' => {
                self.line += 1;
                self.line_start = self.pos;
                return Ok(None);
            }
            other => other,
        }))
    }

    /// Scan a template chunk; the opening backtick or `}` has been consumed.
    fn scan_template_chunk(&mut self) -> Result<TokenKind, LexError> {
        let mut cooked: Option<String> = Some(String::new());
        let mut raw = String::new();
        loop {
            let Some(c) = self.peek_char() else {
                return Err(self.err("Unterminated template literal"));
            };
            match c {
                '`' => {
                    self.pos += 1;
                    return Ok(TokenKind::Template { cooked, raw, tail: true });
                }
                '$' if self.byte_at(1) == Some(b'{') => {
                    self.pos += 2;
                    return Ok(TokenKind::Template { cooked, raw, tail: false });
                }
                '\\' => {
                    let esc_start = self.pos;
                    self.pos += 1;
                    let save_line = (self.line, self.line_start);
                    match self.scan_escape(true) {
                        Ok(Some(ch)) => {
                            let lone = self.lone_surrogate.take();
                            if let Some(ck) = cooked.as_mut() {
                                match lone {
                                    Some(u) => crate::value::push_lone_surrogate(ck, u),
                                    None => ck.push(ch),
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(_) => {
                            // Invalid escape: cooked becomes undefined; skip
                            // the escape char and keep going.
                            cooked = None;
                            self.line = save_line.0;
                            self.line_start = save_line.1;
                            self.pos = esc_start + 1;
                            if let Some(ch) = self.peek_char() {
                                self.pos += ch.len_utf8();
                            }
                        }
                    }
                    let mut r = self.src[esc_start..self.pos].replace("\r\n", "\n");
                    r = r.replace('\r', "\n");
                    raw.push_str(&r);
                }
                '\r' => {
                    self.pos += 1;
                    self.newline('\r');
                    raw.push('\n');
                    if let Some(ck) = cooked.as_mut() {
                        ck.push('\n');
                    }
                }
                c => {
                    self.pos += c.len_utf8();
                    if is_line_terminator(c) {
                        self.newline(c);
                    }
                    raw.push(c);
                    if let Some(ck) = cooked.as_mut() {
                        ck.push(c);
                    }
                }
            }
        }
    }

    /// Re-scan a `}` token as the continuation of a template literal.
    pub fn rescan_template_continuation(&mut self, tok: &Token) -> Result<Token, LexError> {
        debug_assert!(tok.is_punct("}"));
        self.pos = tok.start + 1;
        self.line = tok.pos.line;
        self.line_start = tok.start + 1 - (tok.pos.column as usize);
        let kind = self.scan_template_chunk()?;
        Ok(Token {
            kind,
            start: tok.start,
            end: self.pos,
            pos: tok.pos,
            newline_before: tok.newline_before,
        })
    }

    /// Re-scan a `/` or `/=` token as a regular expression literal.
    pub fn rescan_regex(&mut self, tok: &Token) -> Result<Token, LexError> {
        debug_assert!(tok.is_punct("/") || tok.is_punct("/="));
        self.pos = tok.start + 1;
        self.line = tok.pos.line;
        self.line_start = tok.start + 1 - (tok.pos.column as usize);
        let mut in_class = false;
        let pat_start = self.pos;
        loop {
            let Some(c) = self.peek_char() else {
                return Err(self.err("Unterminated regular expression"));
            };
            if is_line_terminator(c) {
                return Err(self.err("Unterminated regular expression"));
            }
            self.pos += c.len_utf8();
            match c {
                '\\' => {
                    let Some(n) = self.peek_char() else {
                        return Err(self.err("Unterminated regular expression"));
                    };
                    if is_line_terminator(n) {
                        return Err(self.err("Unterminated regular expression"));
                    }
                    self.pos += n.len_utf8();
                }
                '[' => in_class = true,
                ']' => in_class = false,
                '/' if !in_class => break,
                _ => {}
            }
        }
        let pattern = self.src[pat_start..self.pos - 1].to_string();
        let flags_start = self.pos;
        while let Some(c) = self.peek_char() {
            if is_id_part(c) {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
        let flags = self.src[flags_start..self.pos].to_string();
        for (i, f) in flags.chars().enumerate() {
            if !"dgimsuyv".contains(f) || flags[..i].contains(f) {
                return Err(self.err(format!("Invalid regular expression flags '{flags}'")));
            }
        }
        // Early errors: the pattern must parse.
        if let Err(e) = crate::regex::validate(&pattern, &flags) {
            return Err(self.err(format!("Invalid regular expression: /{pattern}/{flags}: {}", e.message)));
        }
        Ok(Token {
            kind: TokenKind::Regex { pattern, flags },
            start: tok.start,
            end: self.pos,
            pos: tok.pos,
            newline_before: tok.newline_before,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<TokenKind> {
        let mut lx = Lexer::new(src, false);
        let mut out = Vec::new();
        loop {
            let t = lx.next_token().unwrap();
            if t.kind == TokenKind::Eof {
                break;
            }
            out.push(t.kind);
        }
        out
    }

    #[test]
    fn numbers() {
        assert_eq!(toks("0x1F 0o17 0b101 017 019 1_000.5e-2 .5 10n 0xffn"), vec![
            TokenKind::Number(31.0),
            TokenKind::Number(15.0),
            TokenKind::Number(5.0),
            TokenKind::Number(15.0),
            TokenKind::Number(19.0),
            TokenKind::Number(10.005),
            TokenKind::Number(0.5),
            TokenKind::BigInt("10".into()),
            TokenKind::BigInt("255".into()),
        ]);
    }

    #[test]
    fn strings_and_escapes() {
        assert_eq!(
            toks(r#"'a\nb' "\x41B\u{43}" 'line\
cont' '\101'"#),
            vec![
                TokenKind::Str("a\nb".into()),
                TokenKind::Str("ABC".into()),
                TokenKind::Str("linecont".into()),
                TokenKind::Str("A".into()),
            ]
        );
    }

    #[test]
    fn puncts_and_idents() {
        assert_eq!(toks("a?.b ?? c **= d => #e"), vec![
            TokenKind::Ident("a".into()),
            TokenKind::Punct("?."),
            TokenKind::Ident("b".into()),
            TokenKind::Punct("??"),
            TokenKind::Ident("c".into()),
            TokenKind::Punct("**="),
            TokenKind::Ident("d".into()),
            TokenKind::Punct("=>"),
            TokenKind::PrivateName("e".into()),
        ]);
        assert_eq!(toks("\\u0061bc"), vec![TokenKind::Ident("abc".into())]);
    }

    #[test]
    fn comments_and_newlines() {
        let mut lx = Lexer::new("#!/usr/bin/env node\na // c\n/* x\ny */ b <!-- h\n--> z\nc", false);
        let a = lx.next_token().unwrap();
        assert!(a.is_ident("a") && a.pos.line == 2);
        let b = lx.next_token().unwrap();
        assert!(b.is_ident("b") && b.newline_before);
        assert_eq!(b.pos.line, 4);
        let c = lx.next_token().unwrap();
        assert!(c.is_ident("c") && c.newline_before);
    }

    #[test]
    fn template_and_regex_rescan() {
        let mut lx = Lexer::new("`a${1}b` /x[/]y/gi", false);
        let t = lx.next_token().unwrap();
        assert_eq!(t.kind, TokenKind::Template { cooked: Some("a".into()), raw: "a".into(), tail: false });
        let one = lx.next_token().unwrap();
        assert_eq!(one.kind, TokenKind::Number(1.0));
        let close = lx.next_token().unwrap();
        let cont = lx.rescan_template_continuation(&close).unwrap();
        assert_eq!(cont.kind, TokenKind::Template { cooked: Some("b".into()), raw: "b".into(), tail: true });
        let slash = lx.next_token().unwrap();
        let re = lx.rescan_regex(&slash).unwrap();
        assert_eq!(re.kind, TokenKind::Regex { pattern: "x[/]y".into(), flags: "gi".into() });
    }
}
