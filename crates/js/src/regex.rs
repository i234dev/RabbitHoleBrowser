//! ECMAScript regular expressions: parser + backtracking matcher.
//!
//! Supports the full ES2018 syntax: alternation, groups (capturing,
//! non-capturing, named), quantifiers (greedy and lazy, `{n,m}`), character
//! classes and escapes (`\d \w \s \b \B`, unicode escapes, `\p{..}` with the
//! common general categories), anchors, lookahead/lookbehind, backreferences
//! (numbered and named), and the flags `g i m s u y d`.
//!
//! Matching works on UTF-16 code units (like JavaScript), so indices
//! returned here are UTF-16 offsets into the subject.
//!
//! # Design
//!
//! The pattern is parsed into a small AST ([`Node`]) which is compiled into
//! a linear program ([`Inst`]) for a backtracking machine. The machine keeps
//! an explicit backtrack stack holding both alternatives (`Split` targets)
//! and undo records for captures / loop registers, so matching never
//! recurses on the input length; recursion only happens for nested
//! lookaround, bounded by the pattern's nesting depth. Lookbehind bodies are
//! compiled in backward direction (terms reversed, consuming instructions
//! flagged `back`), exactly as the specification describes.

use std::collections::HashMap;

thread_local! {
    /// Watchdog deadline shared with the VM (see `Realm::set_deadline`):
    /// a runaway match (catastrophic backtracking) gives up once it passes.
    static DEADLINE: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
    static EXPIRED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Set the matcher deadline for this thread (None disables it).
pub fn set_deadline(d: Option<std::time::Instant>) {
    DEADLINE.with(|c| c.set(d));
    EXPIRED.with(|c| c.set(false));
}

/// Move the matcher deadline later (host time excluded from the watchdog).
pub(crate) fn shift_deadline(by: std::time::Duration) {
    if !EXPIRED.with(|c| c.get()) {
        DEADLINE.with(|c| c.set(c.get().map(|d| d + by)));
    }
}

/// Check the deadline (cheap; call every few thousand steps).
fn deadline_passed() -> bool {
    if EXPIRED.with(|c| c.get()) {
        return true;
    }
    match DEADLINE.with(|c| c.get()) {
        Some(d) if std::time::Instant::now() >= d => {
            EXPIRED.with(|c| c.set(true));
            true
        }
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flags {
    pub global: bool,
    pub ignore_case: bool,
    pub multiline: bool,
    pub dot_all: bool,
    pub unicode: bool,
    pub sticky: bool,
    pub has_indices: bool,
}

impl Flags {
    /// Parse a flag string such as `"gi"`. Unknown or repeated flags → None.
    pub fn parse(flags: &str) -> Option<Flags> {
        let mut f = Flags::default();
        for c in flags.chars() {
            let slot = match c {
                'g' => &mut f.global,
                'i' => &mut f.ignore_case,
                'm' => &mut f.multiline,
                's' => &mut f.dot_all,
                'u' => &mut f.unicode,
                'y' => &mut f.sticky,
                'd' => &mut f.has_indices,
                _ => return None,
            };
            if *slot {
                return None;
            }
            *slot = true;
        }
        Some(f)
    }

    /// Canonical flag string in the order `dgimsuy`.
    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> String {
        let mut s = String::new();
        if self.has_indices {
            s.push('d');
        }
        if self.global {
            s.push('g');
        }
        if self.ignore_case {
            s.push('i');
        }
        if self.multiline {
            s.push('m');
        }
        if self.dot_all {
            s.push('s');
        }
        if self.unicode {
            s.push('u');
        }
        if self.sticky {
            s.push('y');
        }
        s
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegexError {
    pub message: String,
}

impl std::fmt::Display for RegexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Invalid regular expression: {}", self.message)
    }
}

impl std::error::Error for RegexError {}

fn err<T>(message: &str) -> Result<T, RegexError> {
    Err(RegexError {
        message: message.to_string(),
    })
}

/// A compiled regular expression.
#[derive(Debug, Clone)]
pub struct Regex {
    pub source: String,
    pub flags: Flags,
    program: Vec<Inst>,
    classes: Vec<CharClass>,
    group_count: usize,
    group_names: Vec<(String, usize)>,
    /// Number of loop registers used by the program.
    registers: usize,
    /// First code unit required at the match start, when the program
    /// starts with a literal (used to skip ahead quickly).
    first_unit: Option<u16>,
}

/// Result of a successful match. `captures[0]` is the whole match; other
/// entries are `None` for groups that did not participate. Offsets are in
/// UTF-16 code units.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub captures: Vec<Option<(usize, usize)>>,
}

impl Match {
    pub fn start(&self) -> usize {
        self.captures[0].map(|(s, _)| s).unwrap_or(0)
    }
    pub fn end(&self) -> usize {
        self.captures[0].map(|(_, e)| e).unwrap_or(0)
    }
    /// Capture `i` as a Rust string (decoding the UTF-16 slice lossily).
    pub fn group_str(&self, subject: &[u16], i: usize) -> Option<String> {
        self.captures
            .get(i)
            .copied()
            .flatten()
            .map(|(s, e)| String::from_utf16_lossy(&subject[s..e]))
    }
}

impl Regex {
    /// Compile `pattern` with `flags` (as given in the literal / constructor).
    pub fn new(pattern: &str, flags: &str) -> Result<Regex, RegexError> {
        let flags = match Flags::parse(flags) {
            Some(f) => f,
            None => return err(&format!("Invalid flags supplied to RegExp constructor '{flags}'")),
        };
        let chars: Vec<char> = pattern.chars().collect();
        let mut parser = Parser::new(&chars, flags);
        let mut node = parser.parse()?;
        node.resolve_names(&parser.names_map());
        let mut compiler = Compiler {
            program: Vec::new(),
            classes: Vec::new(),
            registers: 0,
        };
        compiler.compile(&node, false)?;
        compiler.program.push(Inst::Match);
        let first_unit = match compiler.program.first() {
            Some(Inst::Char { unit, back: false })
                if !flags.ignore_case && *unit <= 0xFFFF && !(flags.unicode && (0xD800..=0xDFFF).contains(unit)) =>
            {
                Some(*unit as u16)
            }
            _ => None,
        };
        Ok(Regex {
            source: pattern.to_string(),
            flags,
            program: compiler.program,
            classes: compiler.classes,
            group_count: parser.group_count,
            group_names: parser.group_names,
            registers: compiler.registers,
            first_unit,
        })
    }

    /// Number of capturing groups (excluding group 0).
    pub fn group_count(&self) -> usize {
        self.group_count
    }

    /// Named groups as (name, group index), in pattern order.
    pub fn group_names(&self) -> &[(String, usize)] {
        &self.group_names
    }

    /// Find the first match in `subject` (UTF-16 code units) starting the
    /// search at code unit index `start`. With the sticky flag the match
    /// must begin exactly at `start`.
    pub fn exec_utf16(&self, subject: &[u16], start: usize) -> Option<Match> {
        if start > subject.len() {
            return None;
        }
        let mut machine = Machine {
            regex: self,
            subject,
            caps: vec![None; (self.group_count + 1) * 2],
            regs: vec![usize::MAX; self.registers],
            stack: Vec::new(),
        };
        if self.flags.sticky {
            return machine.run_at(start);
        }
        let mut pos = start;
        loop {
            if let Some(first) = self.first_unit {
                match subject[pos..].iter().position(|&u| u == first) {
                    Some(off) => pos += off,
                    None => return None,
                }
            }
            if let Some(m) = machine.run_at(pos) {
                return Some(m);
            }
            if pos >= subject.len() || EXPIRED.with(|c| c.get()) {
                return None;
            }
            // In unicode mode advance by whole code points.
            if self.flags.unicode && is_high(subject[pos]) && pos + 1 < subject.len() && is_low(subject[pos + 1]) {
                pos += 2;
            } else {
                pos += 1;
            }
        }
    }

    /// Convenience over [`Regex::exec_utf16`] for Rust strings; `start` and
    /// returned offsets are still UTF-16 code unit indices.
    pub fn exec(&self, subject: &str, start: usize) -> Option<Match> {
        let units: Vec<u16> = subject.encode_utf16().collect();
        self.exec_utf16(&units, start)
    }

    pub fn is_match(&self, subject: &str) -> bool {
        self.exec(subject, 0).is_some()
    }
}

// ------------------------------------------------------------------ AST

#[derive(Debug, Clone)]
enum Node {
    Empty,
    /// A literal code point (or code unit in non-unicode mode).
    Char(u32),
    Any,
    Class(CharClass),
    Alternation(Vec<Node>),
    Sequence(Vec<Node>),
    Group {
        index: Option<usize>,
        body: Box<Node>,
    },
    Repeat {
        body: Box<Node>,
        min: u32,
        max: Option<u32>,
        greedy: bool,
        inner: (usize, usize),
    },
    Assert(AssertKind),
    Look {
        ahead: bool,
        negate: bool,
        body: Box<Node>,
    },
    Backref(usize),
    NamedBackref(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssertKind {
    Start,
    End,
    WordBoundary,
    NotWordBoundary,
}

/// A set of code points, as sorted, non-overlapping ranges.
#[derive(Debug, Clone, Default)]
struct CharClass {
    ranges: Vec<(u32, u32)>,
    negate: bool,
}

impl CharClass {
    fn add(&mut self, lo: u32, hi: u32) {
        self.ranges.push((lo, hi));
    }
    fn add_class(&mut self, other: &CharClass) {
        if other.negate {
            // Complement of the other's ranges.
            let mut next = 0u32;
            for &(lo, hi) in &other.normalized().ranges {
                if lo > next {
                    self.ranges.push((next, lo - 1));
                }
                next = hi.saturating_add(1);
            }
            if next <= 0x10FFFF {
                self.ranges.push((next, 0x10FFFF));
            }
        } else {
            self.ranges.extend_from_slice(&other.ranges);
        }
    }
    fn normalized(&self) -> CharClass {
        let mut r = self.ranges.clone();
        r.sort_unstable();
        let mut out: Vec<(u32, u32)> = Vec::with_capacity(r.len());
        for (lo, hi) in r {
            if let Some(last) = out.last_mut() {
                if lo <= last.1.saturating_add(1) {
                    last.1 = last.1.max(hi);
                    continue;
                }
            }
            out.push((lo, hi));
        }
        CharClass {
            ranges: out,
            negate: self.negate,
        }
    }
    fn contains_raw(&self, c: u32) -> bool {
        // Binary search over sorted ranges.
        let mut lo = 0usize;
        let mut hi = self.ranges.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            let (a, b) = self.ranges[mid];
            if c < a {
                hi = mid;
            } else if c > b {
                lo = mid + 1;
            } else {
                return true;
            }
        }
        false
    }
    fn matches(&self, c: u32, ignore_case: bool, unicode: bool) -> bool {
        let mut hit = self.contains_raw(c);
        if !hit && ignore_case {
            for v in case_variants(c, unicode) {
                if self.contains_raw(v) {
                    hit = true;
                    break;
                }
            }
        }
        hit != self.negate
    }
}

fn digit_class() -> CharClass {
    CharClass {
        ranges: vec![('0' as u32, '9' as u32)],
        negate: false,
    }
}

fn word_class() -> CharClass {
    CharClass {
        ranges: vec![
            ('0' as u32, '9' as u32),
            ('A' as u32, 'Z' as u32),
            ('_' as u32, '_' as u32),
            ('a' as u32, 'z' as u32),
        ],
        negate: false,
    }
}

fn space_class() -> CharClass {
    CharClass {
        ranges: vec![
            (0x09, 0x0D),
            (0x20, 0x20),
            (0xA0, 0xA0),
            (0x1680, 0x1680),
            (0x2000, 0x200A),
            (0x2028, 0x2029),
            (0x202F, 0x202F),
            (0x205F, 0x205F),
            (0x3000, 0x3000),
            (0xFEFF, 0xFEFF),
        ],
        negate: false,
    }
}

fn is_word_unit(u: u32) -> bool {
    matches!(u, 0x30..=0x39 | 0x41..=0x5A | 0x5F | 0x61..=0x7A)
}

fn is_line_terminator(u: u32) -> bool {
    matches!(u, 0x0A | 0x0D | 0x2028 | 0x2029)
}

fn is_high(u: u16) -> bool {
    (0xD800..=0xDBFF).contains(&u)
}

fn is_low(u: u16) -> bool {
    (0xDC00..=0xDFFF).contains(&u)
}

/// Single-character case variants of `c` (upper and lower forms) used for
/// case-insensitive comparisons.
fn case_variants(c: u32, unicode: bool) -> Vec<u32> {
    let mut out = Vec::new();
    let Some(ch) = char::from_u32(c) else {
        return out;
    };
    let mut push = |v: char| {
        let v = v as u32;
        if v != c && !out.contains(&v) {
            out.push(v);
        }
    };
    let mut up = ch.to_uppercase();
    if let (Some(u), None) = (up.next(), up.next()) {
        // Non-unicode mode: a non-ASCII char never folds to an ASCII one.
        if unicode || !(c >= 128 && (u as u32) < 128) {
            push(u);
        }
    }
    let mut low = ch.to_lowercase();
    if let (Some(l), None) = (low.next(), low.next()) {
        if unicode || !(c < 128 && (l as u32) >= 128) {
            push(l);
        }
    }
    if unicode {
        // A few extra simple case foldings.
        match c {
            0x1E9E => push('ß'),
            0xDF => push('\u{1E9E}'),
            0x212A => push('k'),
            0x6B | 0x4B => push('\u{212A}'),
            0x17F => push('s'),
            0x73 | 0x53 => push('\u{17F}'),
            _ => {}
        }
    }
    out
}

fn fold_eq(a: u32, b: u32, unicode: bool) -> bool {
    if a == b {
        return true;
    }
    case_variants(a, unicode).contains(&b)
}

// --------------------------------------------------------------- parser

struct Parser<'a> {
    chars: &'a [char],
    pos: usize,
    flags: Flags,
    group_count: usize,
    group_names: Vec<(String, usize)>,
    /// Total number of capturing groups in the pattern (pre-scanned) so
    /// `\N` can be classified as backreference vs. legacy octal.
    total_groups: usize,
    has_named_groups: bool,
    /// Named backreferences seen, validated after parsing.
    named_refs: Vec<String>,
}

impl<'a> Parser<'a> {
    fn new(chars: &'a [char], flags: Flags) -> Self {
        let (total_groups, has_named_groups) = prescan_groups(chars);
        Parser {
            chars,
            pos: 0,
            flags,
            group_count: 0,
            group_names: Vec::new(),
            total_groups,
            has_named_groups,
            named_refs: Vec::new(),
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, off: usize) -> Option<char> {
        self.chars.get(self.pos + off).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn unicode(&self) -> bool {
        self.flags.unicode
    }

    fn parse(&mut self) -> Result<Node, RegexError> {
        let node = self.parse_disjunction()?;
        if self.pos < self.chars.len() {
            if self.peek() == Some(')') {
                return err("Unmatched ')'");
            }
            return err("Unexpected character");
        }
        for name in &self.named_refs {
            if !self.group_names.iter().any(|(n, _)| n == name) {
                return err("Invalid named capture referenced");
            }
        }
        Ok(node)
    }

    fn parse_disjunction(&mut self) -> Result<Node, RegexError> {
        let mut alts = vec![self.parse_alternative()?];
        while self.eat('|') {
            alts.push(self.parse_alternative()?);
        }
        if alts.len() == 1 {
            Ok(alts.pop().unwrap())
        } else {
            Ok(Node::Alternation(alts))
        }
    }

    fn parse_alternative(&mut self) -> Result<Node, RegexError> {
        let mut terms = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let term = self.parse_term()?;
            terms.push(term);
        }
        match terms.len() {
            0 => Ok(Node::Empty),
            1 => Ok(terms.pop().unwrap()),
            _ => Ok(Node::Sequence(terms)),
        }
    }

    fn parse_term(&mut self) -> Result<Node, RegexError> {
        let groups_before = self.group_count;
        let c = self.peek().unwrap();
        let atom = match c {
            '^' => {
                self.pos += 1;
                return Ok(Node::Assert(AssertKind::Start));
            }
            '$' => {
                self.pos += 1;
                return Ok(Node::Assert(AssertKind::End));
            }
            '\\' if self.peek_at(1) == Some('b') => {
                self.pos += 2;
                return Ok(Node::Assert(AssertKind::WordBoundary));
            }
            '\\' if self.peek_at(1) == Some('B') => {
                self.pos += 2;
                return Ok(Node::Assert(AssertKind::NotWordBoundary));
            }
            '(' => {
                if self.peek_at(1) == Some('?') {
                    match (self.peek_at(2), self.peek_at(3)) {
                        (Some('='), _) | (Some('!'), _) => {
                            let negate = self.peek_at(2) == Some('!');
                            self.pos += 3;
                            let body = self.parse_disjunction()?;
                            if !self.eat(')') {
                                return err("Unterminated group");
                            }
                            let node = Node::Look {
                                ahead: true,
                                negate,
                                body: Box::new(body),
                            };
                            // Annex B: lookaheads may be quantified in non-unicode mode.
                            if !self.unicode() {
                                return self.parse_quantifier(node, groups_before);
                            }
                            if self.at_quantifier() {
                                return err("Nothing to repeat");
                            }
                            return Ok(node);
                        }
                        (Some('<'), Some('=')) | (Some('<'), Some('!')) => {
                            let negate = self.peek_at(3) == Some('!');
                            self.pos += 4;
                            let body = self.parse_disjunction()?;
                            if !self.eat(')') {
                                return err("Unterminated group");
                            }
                            if self.at_quantifier() {
                                return err("Nothing to repeat");
                            }
                            return Ok(Node::Look {
                                ahead: false,
                                negate,
                                body: Box::new(body),
                            });
                        }
                        _ => {}
                    }
                }
                self.parse_group()?
            }
            ')' => return err("Unmatched ')'"),
            '*' | '+' | '?' => return err("Nothing to repeat"),
            '{' => {
                if self.unicode() {
                    return err("Nothing to repeat");
                }
                // Annex B: a `{` that is not a valid quantifier is a literal.
                if self.try_parse_braces().is_some() {
                    return err("Nothing to repeat");
                }
                self.pos += 1;
                Node::Char('{' as u32)
            }
            '}' => {
                if self.unicode() {
                    return err("Lone quantifier brackets");
                }
                self.pos += 1;
                Node::Char('}' as u32)
            }
            ']' => {
                if self.unicode() {
                    return err("Lone quantifier brackets");
                }
                self.pos += 1;
                Node::Char(']' as u32)
            }
            '[' => {
                self.pos += 1;
                Node::Class(self.parse_class()?)
            }
            '.' => {
                self.pos += 1;
                Node::Any
            }
            '\\' => {
                self.pos += 1;
                self.parse_atom_escape()?
            }
            _ => {
                self.pos += 1;
                self.literal(c)
            }
        };
        self.parse_quantifier(atom, groups_before)
    }

    /// Wrap a literal char: in non-unicode mode astral characters become a
    /// sequence of two surrogate code units.
    fn literal(&self, c: char) -> Node {
        let cp = c as u32;
        if !self.unicode() && cp > 0xFFFF {
            let mut buf = [0u16; 2];
            let units = c.encode_utf16(&mut buf);
            Node::Sequence(units.iter().map(|&u| Node::Char(u as u32)).collect())
        } else {
            Node::Char(cp)
        }
    }

    fn at_quantifier(&self) -> bool {
        matches!(self.peek(), Some('*') | Some('+') | Some('?'))
            || (self.peek() == Some('{') && self.try_parse_braces().is_some())
    }

    /// Parse `{n}`, `{n,}` or `{n,m}` at the current position without
    /// consuming; returns (min, max, length in chars).
    fn try_parse_braces(&self) -> Option<(u32, Option<u32>, usize)> {
        let mut i = self.pos;
        if self.chars.get(i) != Some(&'{') {
            return None;
        }
        i += 1;
        let start = i;
        while i < self.chars.len() && self.chars[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return None;
        }
        let min: u32 = self.chars[start..i].iter().collect::<String>().parse().unwrap_or(u32::MAX);
        let mut max = Some(min);
        if self.chars.get(i) == Some(&',') {
            i += 1;
            let s2 = i;
            while i < self.chars.len() && self.chars[i].is_ascii_digit() {
                i += 1;
            }
            if i == s2 {
                max = None;
            } else {
                max = Some(self.chars[s2..i].iter().collect::<String>().parse().unwrap_or(u32::MAX));
            }
        }
        if self.chars.get(i) != Some(&'}') {
            return None;
        }
        Some((min, max, i + 1 - self.pos))
    }

    fn parse_quantifier(&mut self, atom: Node, groups_before: usize) -> Result<Node, RegexError> {
        let (min, max) = match self.peek() {
            Some('*') => {
                self.pos += 1;
                (0, None)
            }
            Some('+') => {
                self.pos += 1;
                (1, None)
            }
            Some('?') => {
                self.pos += 1;
                (0, Some(1))
            }
            Some('{') => match self.try_parse_braces() {
                Some((min, max, len)) => {
                    self.pos += len;
                    if let Some(m) = max {
                        if m < min {
                            return err("numbers out of order in {} quantifier");
                        }
                    }
                    (min, max)
                }
                None => return Ok(atom),
            },
            _ => return Ok(atom),
        };
        let greedy = !self.eat('?');
        if matches!(atom, Node::Assert(_)) {
            return err("Nothing to repeat");
        }
        if self.at_quantifier() {
            return err("Nothing to repeat");
        }
        Ok(Node::Repeat {
            body: Box::new(atom),
            min,
            max,
            greedy,
            inner: (groups_before + 1, self.group_count),
        })
    }

    fn parse_group(&mut self) -> Result<Node, RegexError> {
        // at '('
        self.pos += 1;
        let mut index = None;
        if self.eat('?') {
            if self.eat(':') {
                // non-capturing
            } else if self.eat('<') {
                let name = self.parse_group_name()?;
                if self.group_names.iter().any(|(n, _)| *n == name) {
                    return err("Duplicate capture group name");
                }
                self.group_count += 1;
                index = Some(self.group_count);
                self.group_names.push((name, self.group_count));
            } else {
                return err("Invalid group");
            }
        } else {
            self.group_count += 1;
            index = Some(self.group_count);
        }
        let body = self.parse_disjunction()?;
        if !self.eat(')') {
            return err("Unterminated group");
        }
        Ok(Node::Group {
            index,
            body: Box::new(body),
        })
    }

    /// Parse `name>` of a named group / `\k<name>`.
    fn parse_group_name(&mut self) -> Result<String, RegexError> {
        let mut name = String::new();
        loop {
            match self.peek() {
                Some('>') => {
                    self.pos += 1;
                    break;
                }
                Some('\\') if self.peek_at(1) == Some('u') => {
                    self.pos += 2;
                    match self.parse_unicode_escape_body(true) {
                        Some(cp) => match char::from_u32(cp) {
                            Some(c) => name.push(c),
                            None => return err("Invalid capture group name"),
                        },
                        None => return err("Invalid capture group name"),
                    }
                }
                Some(c) if c == '$' || c == '_' || c.is_alphanumeric() || c == '\u{200C}' || c == '\u{200D}' => {
                    if name.is_empty() && c.is_ascii_digit() {
                        return err("Invalid capture group name");
                    }
                    name.push(c);
                    self.pos += 1;
                }
                _ => return err("Invalid capture group name"),
            }
        }
        if name.is_empty() {
            return err("Invalid capture group name");
        }
        Ok(name)
    }

    /// After `\u`: parse `HHHH` or `{H+}` (unicode mode). Returns the code
    /// point, combining a surrogate pair written as `😀` in
    /// unicode mode. Does not consume on failure.
    fn parse_unicode_escape_body(&mut self, allow_braces: bool) -> Option<u32> {
        if allow_braces && self.peek() == Some('{') {
            let save = self.pos;
            self.pos += 1;
            let start = self.pos;
            while matches!(self.peek(), Some(c) if c.is_ascii_hexdigit()) {
                self.pos += 1;
            }
            if self.pos == start || self.peek() != Some('}') {
                self.pos = save;
                return None;
            }
            let digits: String = self.chars[start..self.pos].iter().collect();
            self.pos += 1;
            let cp = u32::from_str_radix(&digits, 16).ok()?;
            if cp > 0x10FFFF {
                self.pos = save;
                return None;
            }
            return Some(cp);
        }
        let hex4 = |p: &Self, at: usize| -> Option<u32> {
            if at + 4 > p.chars.len() {
                return None;
            }
            let s: String = p.chars[at..at + 4].iter().collect();
            if !s.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            u32::from_str_radix(&s, 16).ok()
        };
        let first = hex4(self, self.pos)?;
        self.pos += 4;
        if self.unicode() && (0xD800..=0xDBFF).contains(&first) && self.peek() == Some('\\') && self.peek_at(1) == Some('u') {
            if let Some(second) = hex4(self, self.pos + 2) {
                if (0xDC00..=0xDFFF).contains(&second) {
                    self.pos += 6;
                    return Some(0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00));
                }
            }
        }
        Some(first)
    }

    /// Escapes shared by atoms and classes. Returns a class for `\d` etc.
    /// `in_class` enables `\b` = backspace and `\-`.
    fn parse_char_escape(&mut self, in_class: bool) -> Result<Escape, RegexError> {
        let c = match self.peek() {
            Some(c) => c,
            None => return err("\\ at end of pattern"),
        };
        self.pos += 1;
        let u = self.unicode();
        Ok(match c {
            'd' => Escape::Class(digit_class()),
            'D' => Escape::Class(negated(digit_class())),
            'w' => {
                let mut cls = word_class();
                if u && self.flags.ignore_case {
                    cls.add(0x17F, 0x17F);
                    cls.add(0x212A, 0x212A);
                }
                Escape::Class(cls)
            }
            'W' => {
                let mut cls = word_class();
                if u && self.flags.ignore_case {
                    cls.add(0x17F, 0x17F);
                    cls.add(0x212A, 0x212A);
                }
                Escape::Class(negated(cls))
            }
            's' => Escape::Class(space_class()),
            'S' => Escape::Class(negated(space_class())),
            't' => Escape::Char(0x09),
            'n' => Escape::Char(0x0A),
            'v' => Escape::Char(0x0B),
            'f' => Escape::Char(0x0C),
            'r' => Escape::Char(0x0D),
            'b' if in_class => Escape::Char(0x08),
            '-' if in_class && u => Escape::Char('-' as u32),
            'c' => match self.peek() {
                Some(l) if l.is_ascii_alphabetic() => {
                    self.pos += 1;
                    Escape::Char((l as u32) % 32)
                }
                Some(l) if in_class && !u && (l.is_ascii_digit() || l == '_') => {
                    self.pos += 1;
                    Escape::Char((l as u32) % 32)
                }
                _ => {
                    if u {
                        return err("Invalid unicode escape");
                    }
                    // Annex B: `\c` followed by a non-letter is a literal backslash.
                    self.pos -= 1;
                    Escape::Char('\\' as u32)
                }
            },
            '0' if !matches!(self.peek(), Some(d) if d.is_ascii_digit()) => Escape::Char(0),
            'x' => {
                let save = self.pos;
                let hex: String = self.chars[self.pos..].iter().take(2).collect();
                if hex.len() == 2 && hex.chars().all(|h| h.is_ascii_hexdigit()) {
                    self.pos += 2;
                    Escape::Char(u32::from_str_radix(&hex, 16).unwrap())
                } else {
                    if u {
                        return err("Invalid escape");
                    }
                    self.pos = save;
                    Escape::Char('x' as u32)
                }
            }
            'u' => match self.parse_unicode_escape_body(u) {
                Some(cp) => Escape::Char(cp),
                None => {
                    if u {
                        return err("Invalid Unicode escape");
                    }
                    Escape::Char('u' as u32)
                }
            },
            'p' | 'P' if u => {
                let negate = c == 'P';
                if !self.eat('{') {
                    return err("Invalid property name");
                }
                let start = self.pos;
                while matches!(self.peek(), Some(ch) if ch != '}') {
                    self.pos += 1;
                }
                if !self.eat('}') {
                    return err("Invalid property name");
                }
                let name: String = self.chars[start..self.pos - 1].iter().collect();
                let cls = unicode_property_class(&name).ok_or_else(|| RegexError {
                    message: "Invalid property name".into(),
                })?;
                Escape::Class(if negate { negated(cls) } else { cls })
            }
            '0'..='7' if !u => {
                // Legacy octal escape (up to 3 digits, value <= 0o377).
                let mut value = c as u32 - '0' as u32;
                let mut digits = 1;
                while digits < 3 {
                    match self.peek() {
                        Some(d @ '0'..='7') => {
                            let nv = value * 8 + (d as u32 - '0' as u32);
                            if nv > 0o377 {
                                break;
                            }
                            value = nv;
                            self.pos += 1;
                            digits += 1;
                        }
                        _ => break,
                    }
                }
                Escape::Char(value)
            }
            _ => {
                if u {
                    // Identity escapes: only syntax characters and '/'.
                    if "^$\\.*+?()[]{}|/".contains(c) {
                        Escape::Char(c as u32)
                    } else {
                        return err("Invalid escape");
                    }
                } else {
                    if c == 'k' && self.has_named_groups {
                        return err("Invalid named reference");
                    }
                    Escape::Char(c as u32)
                }
            }
        })
    }

    fn parse_atom_escape(&mut self) -> Result<Node, RegexError> {
        // after '\'
        match self.peek() {
            None => return err("\\ at end of pattern"),
            Some(d @ '1'..='9') => {
                // Decimal escape: backreference if <= total group count.
                let start = self.pos;
                let mut n: usize = 0;
                while let Some(ch) = self.peek() {
                    if !ch.is_ascii_digit() {
                        break;
                    }
                    n = n.saturating_mul(10).saturating_add(ch as usize - '0' as usize);
                    self.pos += 1;
                }
                if n <= self.total_groups {
                    return Ok(Node::Backref(n));
                }
                if self.unicode() {
                    return err("Invalid escape");
                }
                // Annex B: legacy octal or identity escape.
                self.pos = start;
                if d == '8' || d == '9' {
                    self.pos += 1;
                    return Ok(Node::Char(d as u32));
                }
                return match self.parse_char_escape(false)? {
                    Escape::Char(c) => Ok(Node::Char(c)),
                    Escape::Class(cls) => Ok(Node::Class(cls)),
                };
            }
            Some('k') => {
                if self.unicode() || self.has_named_groups {
                    self.pos += 1;
                    if !self.eat('<') {
                        return err("Invalid named reference");
                    }
                    let name = self.parse_group_name().map_err(|_| RegexError {
                        message: "Invalid named reference".into(),
                    })?;
                    self.named_refs.push(name.clone());
                    return Ok(Node::NamedBackref(name));
                }
                self.pos += 1;
                return Ok(Node::Char('k' as u32));
            }
            _ => {}
        }
        match self.parse_char_escape(false)? {
            Escape::Char(c) => {
                if !self.unicode() && c > 0xFFFF {
                    return Ok(self.literal(char::from_u32(c).unwrap_or('\u{FFFD}')));
                }
                Ok(Node::Char(c))
            }
            Escape::Class(cls) => Ok(Node::Class(cls)),
        }
    }

    fn parse_class(&mut self) -> Result<CharClass, RegexError> {
        // after '['
        let mut cls = CharClass::default();
        if self.eat('^') {
            cls.negate = true;
        }
        loop {
            let c = match self.peek() {
                None => return err("Unterminated character class"),
                Some(']') => {
                    self.pos += 1;
                    break;
                }
                Some(c) => c,
            };
            let lo = self.parse_class_atom(c)?;
            // Range?
            if self.peek() == Some('-') && self.peek_at(1).is_some() && self.peek_at(1) != Some(']') {
                let save = self.pos;
                self.pos += 1;
                let hc = self.peek().unwrap();
                let hi = self.parse_class_atom(hc)?;
                match (&lo, &hi) {
                    (Escape::Char(a), Escape::Char(b)) => {
                        if a > b {
                            return err("Range out of order in character class");
                        }
                        cls.add(*a, *b);
                        continue;
                    }
                    _ => {
                        if self.unicode() {
                            return err("Invalid character class");
                        }
                        // Annex B: treat as the atoms plus a literal '-'.
                        let _ = save;
                        push_escape(&mut cls, &lo, self.unicode());
                        cls.add('-' as u32, '-' as u32);
                        push_escape(&mut cls, &hi, self.unicode());
                        continue;
                    }
                }
            }
            push_escape(&mut cls, &lo, self.unicode());
        }
        Ok(cls)
    }

    fn parse_class_atom(&mut self, c: char) -> Result<Escape, RegexError> {
        if c == '\\' {
            self.pos += 1;
            if self.unicode() && self.peek() == Some('B') {
                return err("Invalid escape");
            }
            if !self.unicode() && self.peek() == Some('B') {
                self.pos += 1;
                return Ok(Escape::Char('B' as u32));
            }
            // In classes, `\k` and digits are plain escapes (octal/identity).
            match self.peek() {
                Some(d @ '1'..='9') if !self.unicode() => {
                    if d == '8' || d == '9' {
                        self.pos += 1;
                        return Ok(Escape::Char(d as u32));
                    }
                    return self.parse_char_escape(true);
                }
                Some('1'..='9') => return err("Invalid class escape"),
                Some('k') if !self.unicode() => {
                    self.pos += 1;
                    return Ok(Escape::Char('k' as u32));
                }
                _ => {}
            }
            return self.parse_char_escape(true);
        }
        self.pos += 1;
        if !self.unicode() && (c as u32) > 0xFFFF {
            // Non-unicode: the two surrogate code units individually.
            let mut buf = [0u16; 2];
            let units = c.encode_utf16(&mut buf);
            let mut sub = CharClass::default();
            for &u in units.iter() {
                sub.add(u as u32, u as u32);
            }
            return Ok(Escape::Class(sub));
        }
        Ok(Escape::Char(c as u32))
    }
}

enum Escape {
    Char(u32),
    Class(CharClass),
}

fn push_escape(cls: &mut CharClass, e: &Escape, _unicode: bool) {
    match e {
        Escape::Char(c) => cls.add(*c, *c),
        Escape::Class(sub) => cls.add_class(sub),
    }
}

fn negated(mut cls: CharClass) -> CharClass {
    cls.negate = !cls.negate;
    cls
}

/// Count capturing groups and detect named groups before parsing.
fn prescan_groups(chars: &[char]) -> (usize, bool) {
    let mut count = 0;
    let mut named = false;
    let mut i = 0;
    let mut in_class = false;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 1,
            '[' => in_class = true,
            ']' => in_class = false,
            '(' if !in_class => {
                if chars.get(i + 1) == Some(&'?') {
                    if chars.get(i + 2) == Some(&'<') && !matches!(chars.get(i + 3), Some('=') | Some('!')) {
                        count += 1;
                        named = true;
                    }
                } else {
                    count += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    (count, named)
}

/// Approximate Unicode property classes using `char` predicates over the
/// whole code space (computed lazily per property; cached in the class).
fn unicode_property_class(name: &str) -> Option<CharClass> {
    let (key, value) = match name.split_once('=') {
        Some((k, v)) => (k, v),
        None => ("General_Category", name),
    };
    let pred: Box<dyn Fn(char) -> bool> = match key {
        "General_Category" | "gc" => match value {
            "L" | "Letter" => Box::new(|c: char| c.is_alphabetic() && !c.is_numeric()),
            "Lu" | "Uppercase_Letter" => Box::new(|c: char| c.is_uppercase()),
            "Ll" | "Lowercase_Letter" => Box::new(|c: char| c.is_lowercase()),
            "Lt" | "Titlecase_Letter" => Box::new(|c: char| matches!(c, '\u{1C5}' | '\u{1C8}' | '\u{1CB}' | '\u{1F2}')),
            "Lm" | "Modifier_Letter" => Box::new(|c: char| matches!(c as u32, 0x2B0..=0x2C1 | 0x2C6..=0x2D1 | 0x2E0..=0x2E4 | 0x2EC | 0x2EE | 0x374 | 0x37A | 0x559 | 0x640 | 0x3005 | 0x3031..=0x3035 | 0x309D | 0x309E | 0x30FC..=0x30FE)),
            "Lo" | "Other_Letter" => Box::new(|c: char| c.is_alphabetic() && !c.is_uppercase() && !c.is_lowercase() && !c.is_numeric()),
            "N" | "Number" => Box::new(|c: char| c.is_numeric()),
            "Nd" | "Decimal_Number" | "digit" => Box::new(|c: char| c.is_numeric() && c.to_digit(10).is_some() || is_decimal_digit(c)),
            "Nl" | "Letter_Number" => Box::new(|c: char| matches!(c as u32, 0x16EE..=0x16F0 | 0x2160..=0x2182 | 0x2185..=0x2188 | 0x3007 | 0x3021..=0x3029 | 0x3038..=0x303A)),
            "No" | "Other_Number" => Box::new(|c: char| c.is_numeric() && !is_decimal_digit(c)),
            "P" | "Punctuation" | "punct" => Box::new(|c: char| c.is_ascii_punctuation() && !"$+<=>^`|~".contains(c) || is_unicode_punct(c)),
            "S" | "Symbol" => Box::new(|c: char| "$+<=>^`|~".contains(c) || is_unicode_symbol(c)),
            "Z" | "Separator" => Box::new(|c: char| c.is_whitespace() && !matches!(c, '\t' | '\n' | '\u{B}' | '\u{C}' | '\r' | '\u{85}')),
            "Zs" | "Space_Separator" => Box::new(|c: char| c.is_whitespace() && !matches!(c, '\t' | '\n' | '\u{B}' | '\u{C}' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}')),
            "Zl" | "Line_Separator" => Box::new(|c: char| c == '\u{2028}'),
            "Zp" | "Paragraph_Separator" => Box::new(|c: char| c == '\u{2029}'),
            "C" | "Other" => Box::new(|c: char| c.is_control() || (0xD800..=0xDFFF).contains(&(c as u32))),
            "Cc" | "Control" => Box::new(|c: char| c.is_control()),
            "Cf" | "Format" => Box::new(|c: char| matches!(c as u32, 0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x180E | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB | 0xE0001 | 0xE0020..=0xE007F)),
            "Co" | "Private_Use" => Box::new(|c: char| matches!(c as u32, 0xE000..=0xF8FF | 0xF0000..=0xFFFFD | 0x100000..=0x10FFFD)),
            "Cs" | "Surrogate" => Box::new(|c: char| (0xD800..=0xDFFF).contains(&(c as u32))),
            "Cn" | "Unassigned" => Box::new(|c: char| matches!(c as u32, 0xFDD0..=0xFDEF) || (c as u32 & 0xFFFE) == 0xFFFE),
            "Mc" | "Spacing_Mark" => Box::new(|c: char| matches!(c as u32, 0x903 | 0x93B | 0x93E..=0x940 | 0x949..=0x94C | 0x94E | 0x94F | 0x982 | 0x983 | 0x9BE..=0x9C0 | 0x9C7 | 0x9C8 | 0x9CB | 0x9CC | 0xA03 | 0xA3E..=0xA40 | 0xA83 | 0xABE..=0xAC0 | 0xB02 | 0xB03 | 0xBBE | 0xBBF | 0xC01..=0xC03 | 0xD02 | 0xD03 | 0xF3E | 0xF3F | 0x102B | 0x102C | 0x1031 | 0x1038 | 0x17B6 | 0x17BE..=0x17C5 | 0x1B04 | 0x302E | 0x302F | 0xA823 | 0xA824 | 0xA827 | 0xAAEB | 0x1D165 | 0x1D166)),
            "Me" | "Enclosing_Mark" => Box::new(|c: char| matches!(c as u32, 0x488 | 0x489 | 0x1ABE | 0x20DD..=0x20E0 | 0x20E2..=0x20E4 | 0xA670..=0xA672)),
            "Pc" | "Connector_Punctuation" => Box::new(|c: char| matches!(c as u32, 0x5F | 0x203F | 0x2040 | 0x2054 | 0xFE33 | 0xFE34 | 0xFE4D..=0xFE4F | 0xFF3F)),
            "Pd" | "Dash_Punctuation" => Box::new(|c: char| matches!(c as u32, 0x2D | 0x58A | 0x5BE | 0x1400 | 0x1806 | 0x2010..=0x2015 | 0x2E17 | 0x2E1A | 0x2E3A | 0x2E3B | 0x2E40 | 0x301C | 0x3030 | 0x30A0 | 0xFE31 | 0xFE32 | 0xFE58 | 0xFE63 | 0xFF0D)),
            "Ps" | "Open_Punctuation" => Box::new(|c: char| matches!(c, '(' | '[' | '{' | '\u{F3A}' | '\u{F3C}' | '\u{169B}' | '\u{201A}' | '\u{201E}' | '\u{2045}' | '\u{207D}' | '\u{208D}' | '\u{2308}' | '\u{230A}' | '\u{2329}' | '\u{2768}' | '\u{276A}' | '\u{27E6}' | '\u{2983}' | '\u{3008}' | '\u{300A}' | '\u{300C}' | '\u{300E}' | '\u{3010}' | '\u{FF08}' | '\u{FF3B}' | '\u{FF5B}')),
            "Pe" | "Close_Punctuation" => Box::new(|c: char| matches!(c, ')' | ']' | '}' | '\u{F3B}' | '\u{F3D}' | '\u{169C}' | '\u{2046}' | '\u{207E}' | '\u{208E}' | '\u{2309}' | '\u{230B}' | '\u{232A}' | '\u{2769}' | '\u{276B}' | '\u{27E7}' | '\u{2984}' | '\u{3009}' | '\u{300B}' | '\u{300D}' | '\u{300F}' | '\u{3011}' | '\u{FF09}' | '\u{FF3D}' | '\u{FF5D}')),
            "Pi" | "Initial_Punctuation" => Box::new(|c: char| matches!(c, '\u{AB}' | '\u{2018}' | '\u{201B}' | '\u{201C}' | '\u{201F}' | '\u{2039}' | '\u{2E02}' | '\u{2E04}' | '\u{2E09}' | '\u{2E0C}' | '\u{2E1C}' | '\u{2E20}')),
            "Pf" | "Final_Punctuation" => Box::new(|c: char| matches!(c, '\u{BB}' | '\u{2019}' | '\u{201D}' | '\u{203A}' | '\u{2E03}' | '\u{2E05}' | '\u{2E0A}' | '\u{2E0D}' | '\u{2E1D}' | '\u{2E21}')),
            "Po" | "Other_Punctuation" => Box::new(|c: char| matches!(c, '!' | '"' | '#' | '%' | '&' | '\'' | '*' | ',' | '.' | '/' | ':' | ';' | '?' | '@' | '\\') || (is_unicode_punct(c) && !matches!(c, '(' | ')' | '[' | ']' | '{' | '}' | '-' | '_' | '\u{AB}' | '\u{BB}' | '\u{2010}'..='\u{2015}' | '\u{2018}'..='\u{201F}'))),
            "Sm" | "Math_Symbol" => Box::new(|c: char| matches!(c, '+' | '<' | '=' | '>' | '|' | '~' | '\u{AC}' | '\u{B1}' | '\u{D7}' | '\u{F7}' | '\u{3F6}' | '\u{2044}' | '\u{2052}' | '\u{2118}' | '\u{2140}'..='\u{2144}' | '\u{214B}' | '\u{2190}'..='\u{2194}' | '\u{2200}'..='\u{22FF}' | '\u{2320}' | '\u{2321}' | '\u{27C0}'..='\u{27FF}' | '\u{2900}'..='\u{2AFF}' | '\u{FB29}' | '\u{FE62}' | '\u{FE64}'..='\u{FE66}' | '\u{FF0B}' | '\u{FF1C}'..='\u{FF1E}' | '\u{FF5C}' | '\u{FF5E}' | '\u{FFE2}' | '\u{FFE9}'..='\u{FFEC}')),
            "Sc" | "Currency_Symbol" => Box::new(|c: char| matches!(c, '$' | '\u{A2}'..='\u{A5}' | '\u{58F}' | '\u{60B}' | '\u{9F2}' | '\u{9F3}' | '\u{E3F}' | '\u{17DB}' | '\u{20A0}'..='\u{20C0}' | '\u{FDFC}' | '\u{FE69}' | '\u{FF04}' | '\u{FFE0}' | '\u{FFE1}' | '\u{FFE5}' | '\u{FFE6}')),
            "Sk" | "Modifier_Symbol" => Box::new(|c: char| matches!(c, '^' | '`' | '\u{A8}' | '\u{AF}' | '\u{B4}' | '\u{B8}' | '\u{2C2}'..='\u{2C5}' | '\u{2D2}'..='\u{2DF}' | '\u{2E5}'..='\u{2EB}' | '\u{2ED}' | '\u{2EF}'..='\u{2FF}' | '\u{375}' | '\u{384}' | '\u{385}' | '\u{1FBD}' | '\u{1FBF}'..='\u{1FC1}' | '\u{1FCD}'..='\u{1FCF}' | '\u{1FDD}'..='\u{1FDF}' | '\u{1FED}'..='\u{1FEF}' | '\u{1FFD}' | '\u{1FFE}' | '\u{309B}' | '\u{309C}' | '\u{FF3E}' | '\u{FF40}' | '\u{FFE3}' | '\u{1F3FB}'..='\u{1F3FF}')),
            "So" | "Other_Symbol" => Box::new(|c: char| is_unicode_symbol(c) && !matches!(c, '$' | '+' | '<' | '=' | '>' | '^' | '`' | '|' | '~')),
            "M" | "Mark" | "Combining_Mark" | "Mn" | "Nonspacing_Mark" => Box::new(|c: char| matches!(c as u32, 0x300..=0x36F | 0x483..=0x489 | 0x591..=0x5BD | 0x610..=0x61A | 0x64B..=0x65F | 0x900..=0x903 | 0x93A..=0x94F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)),
            "Alphabetic" | "Alpha" => Box::new(|c: char| c.is_alphabetic()),
            "Uppercase" | "Upper" => Box::new(|c: char| c.is_uppercase()),
            "Lowercase" | "Lower" => Box::new(|c: char| c.is_lowercase()),
            "White_Space" | "space" => Box::new(|c: char| c.is_whitespace()),
            "Any" => Box::new(|_| true),
            "Assigned" => Box::new(|c: char| !matches!(c as u32, 0xE000..=0xF8FF | 0xF0000..=0x10FFFF)),
            "Bidi_Control" | "Bidi_C" => Box::new(|c: char| matches!(c as u32, 0x61C | 0x200E | 0x200F | 0x202A..=0x202E | 0x2066..=0x2069)),
            "Join_Control" | "Join_C" => Box::new(|c: char| matches!(c as u32, 0x200C | 0x200D)),
            "Default_Ignorable_Code_Point" | "DI" => Box::new(|c: char| matches!(c as u32, 0xAD | 0x34F | 0x61C | 0x115F | 0x1160 | 0x17B4 | 0x17B5 | 0x180B..=0x180F | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x206F | 0x3164 | 0xFE00..=0xFE0F | 0xFEFF | 0xFFA0 | 0xFFF0..=0xFFF8 | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0xE0000..=0xE0FFF)),
            "Variation_Selector" | "VS" => Box::new(|c: char| matches!(c as u32, 0x180B..=0x180D | 0x180F | 0xFE00..=0xFE0F | 0xE0100..=0xE01EF)),
            "Regional_Indicator" | "RI" => Box::new(|c: char| matches!(c as u32, 0x1F1E6..=0x1F1FF)),
            "Emoji_Modifier" | "EMod" => Box::new(|c: char| matches!(c as u32, 0x1F3FB..=0x1F3FF)),
            "Emoji_Component" | "EComp" => Box::new(|c: char| matches!(c as u32, 0x23 | 0x2A | 0x30..=0x39 | 0x200D | 0x20E3 | 0xFE0F | 0x1F1E6..=0x1F1FF | 0x1F3FB..=0x1F3FF | 0x1F9B0..=0x1F9B3 | 0xE0020..=0xE007F)),
            "ID_Start" | "IDS" | "XID_Start" | "XIDS" => Box::new(|c: char| c.is_alphabetic()),
            "ID_Continue" | "IDC" | "XID_Continue" | "XIDC" => Box::new(|c: char| c.is_alphanumeric() || c == '_' || matches!(c as u32, 0x300..=0x36F | 0x200C | 0x200D)),
            "Cased" => Box::new(|c: char| c.is_uppercase() || c.is_lowercase()),
            "Dash" => Box::new(|c: char| matches!(c as u32, 0x2D | 0x58A | 0x5BE | 0x1400 | 0x1806 | 0x2010..=0x2015 | 0x2053 | 0x207B | 0x208B | 0x2212 | 0x2E17 | 0x2E1A | 0x2E3A | 0x2E3B | 0x2E40 | 0x301C | 0x3030 | 0x30A0 | 0xFE31 | 0xFE32 | 0xFE58 | 0xFE63 | 0xFF0D)),
            "Hex_Digit" | "Hex" | "ASCII_Hex_Digit" | "AHex" => Box::new(|c: char| c.is_ascii_hexdigit()),
            "Ideographic" | "Ideo" => Box::new(|c: char| matches!(c as u32, 0x3006 | 0x3007 | 0x3021..=0x3029 | 0x3038..=0x303A | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAD9 | 0x20000..=0x323AF)),
            "ASCII" => Box::new(|c: char| c.is_ascii()),
            "Emoji" | "Emoji_Presentation" | "Extended_Pictographic" => Box::new(|c: char| matches!(c as u32, 0x1F300..=0x1FAFF | 0x2600..=0x27BF | 0x1F000..=0x1F2FF | 0x23 | 0x2A | 0x30..=0x39 | 0xA9 | 0xAE | 0x203C | 0x2049 | 0x2122 | 0x2139 | 0x2194..=0x21AA | 0x231A..=0x23FF | 0x25AA..=0x25FE | 0x2B05..=0x2B55 | 0x3030 | 0x303D | 0x3297 | 0x3299)),
            _ => return None,
        },
        "Script" | "sc" | "Script_Extensions" | "scx" => match value {
            "Latin" | "Latn" => Box::new(|c: char| matches!(c as u32, 0x41..=0x5A | 0x61..=0x7A | 0xAA | 0xBA | 0xC0..=0xD6 | 0xD8..=0xF6 | 0xF8..=0x2B8 | 0x2E0..=0x2E4 | 0x1D00..=0x1D25 | 0x1D2C..=0x1D5C | 0x1D62..=0x1D65 | 0x1D6B..=0x1D77 | 0x1D79..=0x1DBE | 0x1E00..=0x1EFF | 0x2071 | 0x207F | 0x2090..=0x209C | 0x212A | 0x212B | 0x2132 | 0x214E | 0x2160..=0x2188 | 0x2C60..=0x2C7F | 0xA722..=0xA787 | 0xA78B..=0xA7FF | 0xAB30..=0xAB5A | 0xAB5C..=0xAB64 | 0xFB00..=0xFB06 | 0xFF21..=0xFF3A | 0xFF41..=0xFF5A)),
            "Greek" | "Grek" => Box::new(|c: char| matches!(c as u32, 0x370..=0x373 | 0x375..=0x377 | 0x37A..=0x37D | 0x37F | 0x384 | 0x386 | 0x388..=0x3E1 | 0x3F0..=0x3FF | 0x1D26..=0x1D2A | 0x1F00..=0x1FFE)),
            "Cyrillic" | "Cyrl" => Box::new(|c: char| matches!(c as u32, 0x400..=0x484 | 0x487..=0x52F | 0x1C80..=0x1C88 | 0x1D2B | 0x1D78 | 0x2DE0..=0x2DFF | 0xA640..=0xA69F)),
            "Han" | "Hani" => Box::new(|c: char| matches!(c as u32, 0x2E80..=0x2E99 | 0x2E9B..=0x2EF3 | 0x2F00..=0x2FD5 | 0x3005 | 0x3007 | 0x3021..=0x3029 | 0x3038..=0x303B | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFA6D | 0xFA70..=0xFAD9 | 0x20000..=0x2FA1F)),
            "Hiragana" | "Hira" => Box::new(|c: char| matches!(c as u32, 0x3041..=0x3096 | 0x309D..=0x309F)),
            "Katakana" | "Kana" => Box::new(|c: char| matches!(c as u32, 0x30A1..=0x30FA | 0x30FD..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9D)),
            "Arabic" | "Arab" => Box::new(|c: char| matches!(c as u32, 0x600..=0x604 | 0x606..=0x60B | 0x60D..=0x61A | 0x61C..=0x61E | 0x620..=0x63F | 0x641..=0x64A | 0x656..=0x66F | 0x671..=0x6DC | 0x6DE..=0x6FF | 0x750..=0x77F | 0x8A0..=0x8FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFC)),
            "Syriac" | "Syrc" => Box::new(|c: char| matches!(c as u32, 0x700..=0x70D | 0x70F..=0x74A | 0x74D..=0x74F | 0x860..=0x86A)),
            "Thaana" | "Thaa" => Box::new(|c: char| matches!(c as u32, 0x780..=0x7B1)),
            "Mandaic" | "Mand" => Box::new(|c: char| matches!(c as u32, 0x840..=0x85B | 0x85E)),
            "Nko" | "Nkoo" => Box::new(|c: char| matches!(c as u32, 0x7C0..=0x7FA | 0x7FD..=0x7FF)),
            "Samaritan" | "Samr" => Box::new(|c: char| matches!(c as u32, 0x800..=0x82D | 0x830..=0x83E)),
            "Armenian" | "Armn" => Box::new(|c: char| matches!(c as u32, 0x531..=0x556 | 0x559..=0x58A | 0x58D..=0x58F | 0xFB13..=0xFB17)),
            "Georgian" | "Geor" => Box::new(|c: char| matches!(c as u32, 0x10A0..=0x10C5 | 0x10C7 | 0x10CD | 0x10D0..=0x10FA | 0x10FC..=0x10FF | 0x1C90..=0x1CBA | 0x1CBD..=0x1CBF | 0x2D00..=0x2D25)),
            "Ethiopic" | "Ethi" => Box::new(|c: char| matches!(c as u32, 0x1200..=0x139F | 0x2D80..=0x2DDE | 0xAB01..=0xAB2E)),
            "Bengali" | "Beng" => Box::new(|c: char| matches!(c as u32, 0x980..=0x9FE)),
            "Gurmukhi" | "Guru" => Box::new(|c: char| matches!(c as u32, 0xA01..=0xA76)),
            "Gujarati" | "Gujr" => Box::new(|c: char| matches!(c as u32, 0xA81..=0xAFF)),
            "Tamil" | "Taml" => Box::new(|c: char| matches!(c as u32, 0xB82..=0xBFA | 0x11FC0..=0x11FFF)),
            "Telugu" | "Telu" => Box::new(|c: char| matches!(c as u32, 0xC00..=0xC7F)),
            "Kannada" | "Knda" => Box::new(|c: char| matches!(c as u32, 0xC80..=0xCF3)),
            "Malayalam" | "Mlym" => Box::new(|c: char| matches!(c as u32, 0xD00..=0xD7F)),
            "Sinhala" | "Sinh" => Box::new(|c: char| matches!(c as u32, 0xD81..=0xDF4 | 0x111E1..=0x111F4)),
            "Lao" | "Laoo" => Box::new(|c: char| matches!(c as u32, 0xE81..=0xEDF)),
            "Tibetan" | "Tibt" => Box::new(|c: char| matches!(c as u32, 0xF00..=0xFDA)),
            "Myanmar" | "Mymr" => Box::new(|c: char| matches!(c as u32, 0x1000..=0x109F | 0xA9E0..=0xA9FE | 0xAA60..=0xAA7F)),
            "Khmer" | "Khmr" => Box::new(|c: char| matches!(c as u32, 0x1780..=0x17F9 | 0x19E0..=0x19FF)),
            "Mongolian" | "Mong" => Box::new(|c: char| matches!(c as u32, 0x1800..=0x18AA)),
            "Bopomofo" | "Bopo" => Box::new(|c: char| matches!(c as u32, 0x2EA..=0x2EB | 0x3105..=0x312F | 0x31A0..=0x31BF)),
            "Inherited" | "Zinh" | "Qaai" => Box::new(|c: char| matches!(c as u32, 0x300..=0x36F | 0x485 | 0x486 | 0x64B..=0x655 | 0x670 | 0x1AB0..=0x1ACE | 0x1DC0..=0x1DFF | 0x200C | 0x200D | 0x20D0..=0x20F0 | 0xFE00..=0xFE0F | 0xFE20..=0xFE2D | 0xE0100..=0xE01EF)),
            "Hebrew" | "Hebr" => Box::new(|c: char| matches!(c as u32, 0x591..=0x5C7 | 0x5D0..=0x5EA | 0x5EF..=0x5F4 | 0xFB1D..=0xFB4F)),
            "Devanagari" | "Deva" => Box::new(|c: char| matches!(c as u32, 0x900..=0x97F | 0xA8E0..=0xA8FF)),
            "Thai" => Box::new(|c: char| matches!(c as u32, 0xE01..=0xE3A | 0xE40..=0xE5B)),
            "Hangul" | "Hang" => Box::new(|c: char| matches!(c as u32, 0x1100..=0x11FF | 0x3131..=0x318E | 0xA960..=0xA97C | 0xAC00..=0xD7A3 | 0xD7B0..=0xD7FB)),
            "Common" | "Zyyy" => Box::new(|c: char| !c.is_alphabetic()),
            _ => return None,
        },
        _ => return None,
    };
    let mut cls = CharClass::default();
    let mut run: Option<(u32, u32)> = None;
    for cp in 0u32..=0x10FFFF {
        let hit = char::from_u32(cp).map(&pred).unwrap_or(false);
        match (hit, run) {
            (true, Some((lo, _))) => run = Some((lo, cp)),
            (true, None) => run = Some((cp, cp)),
            (false, Some((lo, hi))) => {
                cls.add(lo, hi);
                run = None;
            }
            (false, None) => {}
        }
    }
    if let Some((lo, hi)) = run {
        cls.add(lo, hi);
    }
    Some(cls)
}

fn is_decimal_digit(c: char) -> bool {
    c.is_numeric() && c.to_digit(10).is_some()
        || matches!(c as u32, 0x660..=0x669 | 0x6F0..=0x6F9 | 0x966..=0x96F | 0x9E6..=0x9EF | 0xE50..=0xE59 | 0xFF10..=0xFF19)
}

fn is_unicode_punct(c: char) -> bool {
    matches!(c as u32, 0xA1 | 0xA7 | 0xAB | 0xB6 | 0xB7 | 0xBB | 0xBF | 0x2010..=0x2027 | 0x2030..=0x2043 | 0x2045..=0x2051 | 0x2053..=0x205E | 0x3001..=0x3003 | 0x3008..=0x3011 | 0x3014..=0x301F | 0xFF01..=0xFF03 | 0xFF05..=0xFF0A | 0xFF0C..=0xFF0F | 0xFF1A | 0xFF1B | 0xFF1F | 0xFF20 | 0xFF3B..=0xFF3D | 0xFF3F | 0xFF5B | 0xFF5D)
}

fn is_unicode_symbol(c: char) -> bool {
    matches!(c as u32, 0xA2..=0xA6 | 0xA8 | 0xA9 | 0xAC | 0xAE..=0xB1 | 0xB4 | 0xB8 | 0xD7 | 0xF7 | 0x2044 | 0x2052 | 0x20A0..=0x20C0 | 0x2100..=0x214F | 0x2190..=0x23FF | 0x2500..=0x27BF | 0x2900..=0x2BFF | 0x1F000..=0x1FAFF)
}

// ------------------------------------------------------------- compiler

#[derive(Debug, Clone)]
enum Inst {
    /// Match one literal (code point in unicode mode, code unit otherwise).
    Char { unit: u32, back: bool },
    /// `.`
    Any { back: bool },
    /// Character class by index.
    Class { id: usize, back: bool },
    /// Try `first`, on failure resume at `second`.
    Split { first: usize, second: usize },
    Jmp(usize),
    /// Record the current position in capture slot `slot`.
    Save(usize),
    /// Clear capture slots `from..=to` (group indices).
    ResetCaps { from: usize, to: usize },
    /// Store the current position in loop register `reg`.
    SetReg(usize),
    /// Fail if the position equals register `reg` (empty-iteration check).
    CheckProgress(usize),
    Assert(AssertKind),
    Backref { group: usize, back: bool },
    /// Lookaround: the body is `pc+1 .. end` (exclusive), ending in `Match`.
    /// Direction is baked into the body's instructions (`back` flags).
    Look { negate: bool, end: usize },
    Match,
}

struct Compiler {
    program: Vec<Inst>,
    classes: Vec<CharClass>,
    registers: usize,
}

const MAX_PROGRAM: usize = 200_000;

impl Compiler {
    fn emit(&mut self, inst: Inst) -> Result<usize, RegexError> {
        if self.program.len() >= MAX_PROGRAM {
            return err("Regular expression too large");
        }
        self.program.push(inst);
        Ok(self.program.len() - 1)
    }

    fn compile(&mut self, node: &Node, back: bool) -> Result<(), RegexError> {
        match node {
            Node::Empty => Ok(()),
            Node::Char(c) => {
                self.emit(Inst::Char { unit: *c, back })?;
                Ok(())
            }
            Node::Any => {
                self.emit(Inst::Any { back })?;
                Ok(())
            }
            Node::Class(cls) => {
                let id = self.classes.len();
                self.classes.push(cls.normalized());
                self.emit(Inst::Class { id, back })?;
                Ok(())
            }
            Node::Sequence(items) => {
                if back {
                    for item in items.iter().rev() {
                        self.compile(item, back)?;
                    }
                } else {
                    for item in items {
                        self.compile(item, back)?;
                    }
                }
                Ok(())
            }
            Node::Alternation(alts) => {
                // split L1, L2; L1: alt0; jmp end; L2: split ...
                let mut jumps = Vec::new();
                for (i, alt) in alts.iter().enumerate() {
                    if i + 1 < alts.len() {
                        let split = self.emit(Inst::Split { first: 0, second: 0 })?;
                        self.program[split] = Inst::Split {
                            first: split + 1,
                            second: 0,
                        };
                        self.compile(alt, back)?;
                        let j = self.emit(Inst::Jmp(0))?;
                        jumps.push(j);
                        let next = self.program.len();
                        if let Inst::Split { second, .. } = &mut self.program[split] {
                            *second = next;
                        }
                    } else {
                        self.compile(alt, back)?;
                    }
                }
                let end = self.program.len();
                for j in jumps {
                    self.program[j] = Inst::Jmp(end);
                }
                Ok(())
            }
            Node::Group { index, body } => {
                if let Some(i) = index {
                    let (first, second) = if back { (i * 2 + 1, i * 2) } else { (i * 2, i * 2 + 1) };
                    self.emit(Inst::Save(first))?;
                    self.compile(body, back)?;
                    self.emit(Inst::Save(second))?;
                } else {
                    self.compile(body, back)?;
                }
                Ok(())
            }
            Node::Assert(kind) => {
                self.emit(Inst::Assert(*kind))?;
                Ok(())
            }
            Node::Backref(n) => {
                self.emit(Inst::Backref { group: *n, back })?;
                Ok(())
            }
            Node::NamedBackref(_) => unreachable!("resolved before compile"),
            Node::Look { ahead, negate, body } => {
                let look = self.emit(Inst::Look {
                    negate: *negate,
                    end: 0,
                })?;
                self.compile(body, !*ahead)?;
                self.emit(Inst::Match)?;
                let end = self.program.len();
                if let Inst::Look { end: e, .. } = &mut self.program[look] {
                    *e = end;
                }
                Ok(())
            }
            Node::Repeat {
                body,
                min,
                max,
                greedy,
                inner,
            } => self.compile_repeat(body, *min, *max, *greedy, *inner, back),
        }
    }

    fn compile_body_iteration(&mut self, body: &Node, inner: (usize, usize), back: bool) -> Result<(), RegexError> {
        if inner.0 <= inner.1 {
            self.emit(Inst::ResetCaps {
                from: inner.0,
                to: inner.1,
            })?;
        }
        self.compile(body, back)
    }

    fn compile_repeat(
        &mut self,
        body: &Node,
        min: u32,
        max: Option<u32>,
        greedy: bool,
        inner: (usize, usize),
        back: bool,
    ) -> Result<(), RegexError> {
        if max == Some(0) {
            return Ok(());
        }
        // Mandatory copies.
        for _ in 0..min {
            self.compile_body_iteration(body, inner, back)?;
        }
        match max {
            None => {
                // Star loop with empty-iteration protection.
                let reg = self.registers;
                self.registers += 1;
                let split = self.emit(Inst::Split { first: 0, second: 0 })?;
                let body_start = self.program.len();
                self.emit(Inst::SetReg(reg))?;
                self.compile_body_iteration(body, inner, back)?;
                self.emit(Inst::CheckProgress(reg))?;
                self.emit(Inst::Jmp(split))?;
                let end = self.program.len();
                self.program[split] = if greedy {
                    Inst::Split {
                        first: body_start,
                        second: end,
                    }
                } else {
                    Inst::Split {
                        first: end,
                        second: body_start,
                    }
                };
            }
            Some(max) => {
                // Optional copies: each one is `split body` nested so that
                // skipping one skips all the following ones.
                let optional = max - min;
                let mut splits = Vec::new();
                for _ in 0..optional {
                    let split = self.emit(Inst::Split { first: 0, second: 0 })?;
                    splits.push(split);
                    let reg = self.registers;
                    self.registers += 1;
                    self.emit(Inst::SetReg(reg))?;
                    self.compile_body_iteration(body, inner, back)?;
                    self.emit(Inst::CheckProgress(reg))?;
                }
                let end = self.program.len();
                for split in splits {
                    self.program[split] = if greedy {
                        Inst::Split {
                            first: split + 1,
                            second: end,
                        }
                    } else {
                        Inst::Split {
                            first: end,
                            second: split + 1,
                        }
                    };
                }
            }
        }
        Ok(())
    }
}

// --------------------------------------------------------------- machine

enum Backtrack {
    Alt { pc: usize, pos: usize },
    RestoreCap { slot: usize, old: Option<usize> },
    RestoreReg { reg: usize, old: usize },
}

struct Machine<'a> {
    regex: &'a Regex,
    subject: &'a [u16],
    caps: Vec<Option<usize>>,
    regs: Vec<usize>,
    stack: Vec<Backtrack>,
}

impl Machine<'_> {
    fn run_at(&mut self, start: usize) -> Option<Match> {
        for c in self.caps.iter_mut() {
            *c = None;
        }
        self.stack.clear();
        self.caps[0] = Some(start);
        if let Some(end) = self.run(0, start, 0) {
            self.caps[1] = Some(end);
            Some(Match {
                captures: (0..=self.regex.group_count)
                    .map(|g| match (self.caps[g * 2], self.caps[g * 2 + 1]) {
                        (Some(s), Some(e)) => Some((s, e)),
                        _ => None,
                    })
                    .collect(),
            })
        } else {
            None
        }
    }

    /// Read the character at `pos` moving forward (or the one before `pos`
    /// moving backward). Returns (code point or unit, new position).
    fn read(&self, pos: usize, back: bool) -> Option<(u32, usize)> {
        let s = self.subject;
        let unicode = self.regex.flags.unicode;
        if back {
            if pos == 0 {
                return None;
            }
            let u = s[pos - 1];
            if unicode && is_low(u) && pos >= 2 && is_high(s[pos - 2]) {
                let cp = 0x10000 + (((s[pos - 2] as u32) - 0xD800) << 10) + ((u as u32) - 0xDC00);
                return Some((cp, pos - 2));
            }
            Some((u as u32, pos - 1))
        } else {
            if pos >= s.len() {
                return None;
            }
            let u = s[pos];
            if unicode && is_high(u) && pos + 1 < s.len() && is_low(s[pos + 1]) {
                let cp = 0x10000 + (((u as u32) - 0xD800) << 10) + ((s[pos + 1] as u32) - 0xDC00);
                return Some((cp, pos + 2));
            }
            Some((u as u32, pos + 1))
        }
    }

    fn set_cap(&mut self, slot: usize, value: Option<usize>) {
        let old = self.caps[slot];
        self.stack.push(Backtrack::RestoreCap { slot, old });
        self.caps[slot] = value;
    }

    /// Execute from `pc` at `pos`; `base` is the backtrack stack depth this
    /// run owns (entries below it belong to an enclosing run). Returns the
    /// end position on success. On failure the stack is unwound to `base`
    /// and captures/registers are restored.
    fn run(&mut self, mut pc: usize, mut pos: usize, base: usize) -> Option<usize> {
        let regex = self.regex;
        let prog = &regex.program;
        let flags = regex.flags;
        let mut steps: u32 = 0;
        loop {
            steps = steps.wrapping_add(1);
            if steps & 0xffff == 0 && deadline_passed() {
                self.stack.truncate(base);
                return None;
            }
            let ok = match &prog[pc] {
                Inst::Match => return Some(pos),
                Inst::Char { unit, back } => match self.read(pos, *back) {
                    Some((c, np)) if c == *unit || (flags.ignore_case && fold_eq(*unit, c, flags.unicode)) => {
                        pos = np;
                        pc += 1;
                        true
                    }
                    _ => false,
                },
                Inst::Any { back } => match self.read(pos, *back) {
                    Some((c, np)) if flags.dot_all || !is_line_terminator(c) => {
                        pos = np;
                        pc += 1;
                        true
                    }
                    _ => false,
                },
                Inst::Class { id, back } => match self.read(pos, *back) {
                    Some((c, np)) if regex.classes[*id].matches(c, flags.ignore_case, flags.unicode) => {
                        pos = np;
                        pc += 1;
                        true
                    }
                    _ => false,
                },
                Inst::Split { first, second } => {
                    self.stack.push(Backtrack::Alt { pc: *second, pos });
                    pc = *first;
                    true
                }
                Inst::Jmp(t) => {
                    pc = *t;
                    true
                }
                Inst::Save(slot) => {
                    let slot = *slot;
                    self.set_cap(slot, Some(pos));
                    pc += 1;
                    true
                }
                Inst::ResetCaps { from, to } => {
                    for g in *from..=*to {
                        if self.caps[g * 2].is_some() || self.caps[g * 2 + 1].is_some() {
                            self.set_cap(g * 2, None);
                            self.set_cap(g * 2 + 1, None);
                        }
                    }
                    pc += 1;
                    true
                }
                Inst::SetReg(reg) => {
                    let reg = *reg;
                    let old = self.regs[reg];
                    self.stack.push(Backtrack::RestoreReg { reg, old });
                    self.regs[reg] = pos;
                    pc += 1;
                    true
                }
                Inst::CheckProgress(reg) => {
                    if self.regs[*reg] == pos {
                        false
                    } else {
                        pc += 1;
                        true
                    }
                }
                Inst::Assert(kind) => {
                    let s = self.subject;
                    let ok = match kind {
                        AssertKind::Start => pos == 0 || (flags.multiline && is_line_terminator(s[pos - 1] as u32)),
                        AssertKind::End => pos == s.len() || (flags.multiline && is_line_terminator(s[pos] as u32)),
                        AssertKind::WordBoundary | AssertKind::NotWordBoundary => {
                            let before = pos > 0 && self.is_word_at(pos - 1);
                            let after = pos < s.len() && self.is_word_at(pos);
                            (before != after) == (*kind == AssertKind::WordBoundary)
                        }
                    };
                    if ok {
                        pc += 1;
                    }
                    ok
                }
                Inst::Backref { group, back } => {
                    let g = *group;
                    match (self.caps[g * 2], self.caps[g * 2 + 1]) {
                        (Some(s), Some(e)) if s <= e => {
                            let len = e - s;
                            let ok = if *back {
                                if pos < len {
                                    false
                                } else {
                                    self.units_equal(s, pos - len, len)
                                }
                            } else if pos + len > self.subject.len() {
                                false
                            } else {
                                self.units_equal(s, pos, len)
                            };
                            if ok {
                                if *back {
                                    pos -= len;
                                } else {
                                    pos += len;
                                }
                                pc += 1;
                            }
                            ok
                        }
                        // Unset group: matches empty.
                        _ => {
                            pc += 1;
                            true
                        }
                    }
                }
                Inst::Look { negate, end } => {
                    let negate = *negate;
                    let end = *end;
                    let sub_base = self.stack.len();
                    let result = self.run(pc + 1, pos, sub_base);
                    match (result.is_some(), negate) {
                        (true, false) => {
                            // Commit: keep captures, discard the sub-run's
                            // alternatives but keep its undo records so
                            // outer backtracking restores them.
                            self.stack.retain_from(sub_base, |b| !matches!(b, Backtrack::Alt { .. }));
                            pc = end;
                            true
                        }
                        (true, true) => {
                            // Negative lookaround matched: undo and fail.
                            self.unwind_to(sub_base);
                            false
                        }
                        (false, false) => false,
                        (false, true) => {
                            pc = end;
                            true
                        }
                    }
                }
            };
            if !ok {
                // Backtrack.
                loop {
                    if self.stack.len() <= base {
                        return None;
                    }
                    match self.stack.pop().unwrap() {
                        Backtrack::Alt { pc: npc, pos: npos } => {
                            pc = npc;
                            pos = npos;
                            break;
                        }
                        Backtrack::RestoreCap { slot, old } => self.caps[slot] = old,
                        Backtrack::RestoreReg { reg, old } => self.regs[reg] = old,
                    }
                }
            }
        }
    }

    fn unwind_to(&mut self, base: usize) {
        while self.stack.len() > base {
            match self.stack.pop().unwrap() {
                Backtrack::Alt { .. } => {}
                Backtrack::RestoreCap { slot, old } => self.caps[slot] = old,
                Backtrack::RestoreReg { reg, old } => self.regs[reg] = old,
            }
        }
    }

    fn is_word_at(&self, i: usize) -> bool {
        let u = self.subject[i] as u32;
        if is_word_unit(u) {
            return true;
        }
        self.regex.flags.unicode && self.regex.flags.ignore_case && (u == 0x17F || u == 0x212A)
    }

    fn units_equal(&self, a: usize, b: usize, len: usize) -> bool {
        let s = self.subject;
        let flags = self.regex.flags;
        if !flags.ignore_case {
            return s[a..a + len] == s[b..b + len];
        }
        for i in 0..len {
            if !fold_eq(s[a + i] as u32, s[b + i] as u32, flags.unicode) {
                return false;
            }
        }
        true
    }
}

trait RetainFrom<T> {
    fn retain_from(&mut self, from: usize, keep: impl FnMut(&T) -> bool);
}

impl<T> RetainFrom<T> for Vec<T> {
    fn retain_from(&mut self, from: usize, mut keep: impl FnMut(&T) -> bool) {
        let tail: Vec<T> = self.drain(from..).filter(|x| keep(x)).collect();
        self.extend(tail);
    }
}

// Resolve named backreferences to indices before compiling.
impl Node {
    fn resolve_names(&mut self, names: &HashMap<String, usize>) {
        match self {
            Node::NamedBackref(name) => {
                let idx = names.get(name).copied().unwrap_or(0);
                *self = Node::Backref(idx);
            }
            Node::Alternation(items) | Node::Sequence(items) => {
                for i in items {
                    i.resolve_names(names);
                }
            }
            Node::Group { body, .. } | Node::Repeat { body, .. } | Node::Look { body, .. } => body.resolve_names(names),
            _ => {}
        }
    }
}

impl Parser<'_> {
    fn names_map(&self) -> HashMap<String, usize> {
        self.group_names.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pattern: &str, flags: &str, subject: &str) -> Option<Vec<Option<String>>> {
        let re = Regex::new(pattern, flags).unwrap_or_else(|e| panic!("{pattern}: {e}"));
        let units: Vec<u16> = subject.encode_utf16().collect();
        re.exec_utf16(&units, 0)
            .map(|mt| (0..mt.captures.len()).map(|i| mt.group_str(&units, i)).collect())
    }

    fn whole(pattern: &str, flags: &str, subject: &str) -> Option<String> {
        m(pattern, flags, subject).and_then(|c| c[0].clone())
    }

    fn error(pattern: &str, flags: &str) -> String {
        match Regex::new(pattern, flags) {
            Ok(_) => panic!("{pattern} should fail"),
            Err(e) => e.message,
        }
    }

    #[test]
    fn flags_parse() {
        let f = Flags::parse("gimsuyd").unwrap();
        assert!(f.global && f.ignore_case && f.multiline && f.dot_all && f.unicode && f.sticky && f.has_indices);
        assert_eq!(f.to_string(), "dgimsuy");
        assert!(Flags::parse("gg").is_none());
        assert!(Flags::parse("x").is_none());
    }

    #[test]
    fn literals_and_alternation() {
        assert_eq!(whole("abc", "", "xxabcxx").as_deref(), Some("abc"));
        assert_eq!(whole("a|ab", "", "abc").as_deref(), Some("a"));
        assert_eq!(whole("ab|a", "", "abc").as_deref(), Some("ab"));
        assert_eq!(whole("a||b", "", "b").as_deref(), Some(""));
        assert_eq!(whole("x", "", "abc"), None);
        assert_eq!(whole("", "", "abc").as_deref(), Some(""));
    }

    #[test]
    fn quantifiers() {
        assert_eq!(whole("a*", "", "aaab").as_deref(), Some("aaa"));
        assert_eq!(whole("a+", "", "baaab").as_deref(), Some("aaa"));
        assert_eq!(whole("a?b", "", "b").as_deref(), Some("b"));
        assert_eq!(whole("a{2}", "", "aaaa").as_deref(), Some("aa"));
        assert_eq!(whole("a{2,}", "", "aaaa").as_deref(), Some("aaaa"));
        assert_eq!(whole("a{1,3}", "", "aaaa").as_deref(), Some("aaa"));
        assert_eq!(whole("a{1,3}?", "", "aaaa").as_deref(), Some("a"));
        assert_eq!(whole("a*?b", "", "aaab").as_deref(), Some("aaab"));
        assert_eq!(whole("<.+?>", "", "<a><b>").as_deref(), Some("<a>"));
        assert_eq!(whole("<.+>", "", "<a><b>").as_deref(), Some("<a><b>"));
        assert_eq!(whole("a{,2}", "", "a{,2}").as_deref(), Some("a{,2}"));
        assert_eq!(whole("x{2,1", "", "x{2,1").as_deref(), Some("x{2,1"));
    }

    #[test]
    fn catastrophic_patterns_terminate() {
        assert_eq!(whole("(a*)*b", "", "aaaa"), None);
        assert_eq!(whole("(a*)*b", "", "aaaab").as_deref(), Some("aaaab"));
        assert_eq!(whole("(a|b)*c", "", "ababab"), None);
        assert_eq!(whole("(?:a*)*", "", "aaa").as_deref(), Some("aaa"));
        assert_eq!(whole("(a?)*?b", "", "aab").as_deref(), Some("aab"));
    }

    #[test]
    fn long_input_no_stack_overflow() {
        let s: String = std::iter::repeat("ab").take(100_000).collect();
        assert_eq!(whole("(a|b)*", "", &s).map(|x| x.len()), Some(200_000));
        assert_eq!(whole("(?:a|b)+$", "", &s).map(|x| x.len()), Some(200_000));
        let re = Regex::new("\\s+", "g").unwrap();
        let text = "a ".repeat(50_000);
        let mut count = 0;
        let mut pos = 0;
        let units: Vec<u16> = text.encode_utf16().collect();
        while let Some(mt) = re.exec_utf16(&units, pos) {
            count += 1;
            pos = mt.end();
        }
        assert_eq!(count, 50_000);
    }

    #[test]
    fn groups_and_captures() {
        assert_eq!(
            m("(\\d+)-(\\d+)", "", "tel 12-345").unwrap(),
            vec![Some("12-345".into()), Some("12".into()), Some("345".into())]
        );
        assert_eq!(m("(a)|b", "", "b").unwrap(), vec![Some("b".into()), None]);
        assert_eq!(m("(?:(a)|b)*", "", "ab").unwrap(), vec![Some("ab".into()), None]);
        assert_eq!(m("(?:(a)|(b))+", "", "ab").unwrap(), vec![Some("ab".into()), None, Some("b".into())]);
        assert_eq!(m("(z)((a+)?(b+)?(c))*", "", "zaacbbbcac").unwrap(), vec![
            Some("zaacbbbcac".into()),
            Some("z".into()),
            Some("ac".into()),
            Some("a".into()),
            None,
            Some("c".into())
        ]);
        assert_eq!(m("(?:x)(y)", "", "xy").unwrap(), vec![Some("xy".into()), Some("y".into())]);
        let re = Regex::new("(a)(?:b)(c)", "").unwrap();
        assert_eq!(re.group_count(), 2);
    }

    #[test]
    fn named_groups() {
        let re = Regex::new("(?<y>\\d{4})-(?<m>\\d{2})", "").unwrap();
        assert_eq!(re.group_names(), &[("y".to_string(), 1), ("m".to_string(), 2)]);
        assert_eq!(
            m("(?<y>\\d{4})-(?<m>\\d{2})", "", "on 2024-05-01").unwrap(),
            vec![Some("2024-05".into()), Some("2024".into()), Some("05".into())]
        );
        assert_eq!(whole("(?<q>['\"]).*?\\k<q>", "", "say \"hi\" now").as_deref(), Some("\"hi\""));
        assert_eq!(error("(?<a>x)(?<a>y)", ""), "Duplicate capture group name");
        assert_eq!(error("(?<a>x)\\k<b>", ""), "Invalid named capture referenced");
        // Without named groups, \k is an identity escape in non-unicode mode.
        assert_eq!(whole("\\k", "", "k").as_deref(), Some("k"));
        assert_eq!(error("\\k", "u"), "Invalid named reference");
    }

    #[test]
    fn backreferences() {
        assert_eq!(whole("(a)\\1", "", "aa").as_deref(), Some("aa"));
        assert_eq!(whole("(a)\\1", "", "ab"), None);
        assert_eq!(whole("(\\w+) \\1", "", "hello hello world").as_deref(), Some("hello hello"));
        assert_eq!(whole("(a)\\1", "i", "aA").as_deref(), Some("aA"));
        // Forward / unset references match empty.
        assert_eq!(whole("\\1(a)", "", "a").as_deref(), Some("a"));
        assert_eq!(whole("(a)|\\1b", "", "b").as_deref(), Some("b"));
        // \2 with one group is an octal escape in non-unicode mode.
        assert_eq!(whole("(a)\\2", "", "a\u{2}").as_deref(), Some("a\u{2}"));
        assert_eq!(error("(a)\\2", "u"), "Invalid escape");
    }

    #[test]
    fn lookaround() {
        assert_eq!(whole("(?<=\\$)\\d+", "", "cost $42 now").as_deref(), Some("42"));
        assert_eq!(whole("(?<!\\$)\\b\\d+", "", "$42 or 17").as_deref(), Some("17"));
        assert_eq!(whole("\\d+(?= dollars)", "", "100 dollars").as_deref(), Some("100"));
        assert_eq!(whole("\\d+(?! dollars)", "", "100 dollars").as_deref(), Some("10"));
        assert_eq!(whole("a(?!b)", "", "abac").as_deref(), Some("a"));
        assert_eq!(m("a(?!b)", "", "abac").unwrap(), vec![Some("a".into())]);
        // Captures inside positive lookahead are kept.
        assert_eq!(m("(?=(\\w+))\\w", "", "abc").unwrap(), vec![Some("a".into()), Some("abc".into())]);
        // Captures inside negative lookahead are undefined.
        assert_eq!(m("(?!(a))b", "", "b").unwrap(), vec![Some("b".into()), None]);
        // Lookbehind with captures and variable length.
        assert_eq!(m("(?<=(\\d+)(\\d+))$", "", "1053").unwrap(), vec![Some("".into()), Some("1".into()), Some("053".into())]);
        assert_eq!(m("(?<=\\1(.))x", "", "aax").unwrap(), vec![Some("x".into()), Some("a".into())]);
        // Quantified lookahead is allowed in non-unicode mode only.
        assert_eq!(whole("(?=a)*a", "", "a").as_deref(), Some("a"));
        assert_eq!(error("(?=a)*a", "u"), "Nothing to repeat");
        assert_eq!(error("(?<=a)*a", ""), "Nothing to repeat");
    }

    #[test]
    fn anchors_and_multiline() {
        assert_eq!(whole("^b", "", "a\nb"), None);
        assert_eq!(whole("^b", "m", "a\nb").as_deref(), Some("b"));
        assert_eq!(whole("a$", "", "a\nb"), None);
        assert_eq!(whole("a$", "m", "a\nb").as_deref(), Some("a"));
        assert_eq!(whole("^$", "", "").as_deref(), Some(""));
        assert_eq!(whole("^.*$", "", "ab\ncd"), None);
        assert_eq!(whole("^.*$", "s", "ab\ncd").as_deref(), Some("ab\ncd"));
        assert_eq!(whole(".", "", "\u{2028}"), None);
        assert_eq!(whole(".", "s", "\u{2028}").as_deref(), Some("\u{2028}"));
    }

    #[test]
    fn word_boundaries() {
        assert_eq!(whole("\\bcat\\b", "", "concat cat").as_deref(), Some("cat"));
        assert_eq!(m("\\bcat\\b", "", "concat cat").map(|_| ()), Some(()));
        let re = Regex::new("\\bcat\\b", "").unwrap();
        assert_eq!(re.exec("concat cat", 0).unwrap().start(), 7);
        assert_eq!(whole("\\b", "", "abc").as_deref(), Some(""));
        assert_eq!(Regex::new("\\b", "").unwrap().exec("abc", 1).unwrap().start(), 3);
        assert_eq!(whole("\\Bb", "", "ab").as_deref(), Some("b"));
        assert_eq!(whole("\\Ba", "", "ab"), None);
        assert_eq!(whole("c\\b", "", "abc").as_deref(), Some("c"));
    }

    #[test]
    fn classes() {
        assert_eq!(whole("[a-c]+", "", "xxabcabdd").as_deref(), Some("abcab"));
        assert_eq!(whole("[^a-c]+", "", "abcxyzabc").as_deref(), Some("xyz"));
        assert_eq!(whole("[\\]]", "", "a]").as_deref(), Some("]"));
        assert_eq!(whole("[^]", "", "\n").as_deref(), Some("\n"));
        assert_eq!(whole("[]", "", "abc"), None);
        assert_eq!(whole("[]a", "", "abc"), None);
        assert_eq!(whole("[a-]+", "", "x-a-").as_deref(), Some("-a-"));
        assert_eq!(whole("[\\w-]+", "", "foo-bar baz").as_deref(), Some("foo-bar"));
        assert_eq!(whole("[\\d\\s]+", "", "a1 2 3b").as_deref(), Some("1 2 3"));
        assert_eq!(whole("[\\b]", "", "\u{8}").as_deref(), Some("\u{8}"));
        assert_eq!(whole("[.]", "", "a.b").as_deref(), Some("."));
        assert_eq!(whole("[\\-]", "", "-").as_deref(), Some("-"));
        assert_eq!(whole("[\\x41-\\x43]+", "", "ABCD").as_deref(), Some("ABC"));
        assert_eq!(whole("[\\u0041]", "", "A").as_deref(), Some("A"));
        assert_eq!(error("[b-a]", ""), "Range out of order in character class");
        assert_eq!(error("[a", ""), "Unterminated character class");
        // Annex B: class escape in a range becomes literals.
        assert_eq!(whole("[\\d-x]+", "", "1-x").as_deref(), Some("1-x"));
        assert_eq!(error("[\\d-x]", "u"), "Invalid character class");
        assert_eq!(whole("]", "", "]").as_deref(), Some("]"));
        assert_eq!(error("]", "u"), "Lone quantifier brackets");
        assert_eq!(whole("}", "", "}").as_deref(), Some("}"));
    }

    #[test]
    fn escapes() {
        assert_eq!(whole("\\d+", "", "ab123c").as_deref(), Some("123"));
        assert_eq!(whole("\\D+", "", "ab123c").as_deref(), Some("ab"));
        assert_eq!(whole("\\w+", "", "héllo").as_deref(), Some("h"));
        assert_eq!(whole("\\W+", "", "ab, cd").as_deref(), Some(", "));
        assert_eq!(whole("\\s+", "", "a \t\n\u{a0}b").as_deref(), Some(" \t\n\u{a0}"));
        assert_eq!(whole("\\S+", "", "  ab ").as_deref(), Some("ab"));
        assert_eq!(whole("\\t\\n\\v\\f\\r", "", "\t\n\u{b}\u{c}\r").as_deref(), Some("\t\n\u{b}\u{c}\r"));
        assert_eq!(whole("\\0", "", "\0").as_deref(), Some("\0"));
        assert_eq!(whole("\\cJ", "", "\n").as_deref(), Some("\n"));
        assert_eq!(whole("\\c1", "", "\\c1").as_deref(), Some("\\c1"));
        assert_eq!(whole("\\x41", "", "A").as_deref(), Some("A"));
        assert_eq!(whole("\\xZ", "", "xZ").as_deref(), Some("xZ"));
        assert_eq!(whole("\\u00e9", "", "é").as_deref(), Some("é"));
        assert_eq!(whole("\\u{1F600}", "u", "😀").as_deref(), Some("😀"));
        assert_eq!(whole("\\uD83D\\uDE00", "u", "😀").as_deref(), Some("😀"));
        assert_eq!(whole("\\uD83D\\uDE00", "", "😀").as_deref(), Some("😀"));
        assert_eq!(whole("\\u{2}", "", "uu").as_deref(), Some("uu"));
        assert_eq!(whole("\\/\\.\\*", "", "/.*").as_deref(), Some("/.*"));
        assert_eq!(whole("\\a", "", "a").as_deref(), Some("a"));
        assert_eq!(error("\\a", "u"), "Invalid escape");
        assert_eq!(whole("\\8\\9", "", "89").as_deref(), Some("89"));
        assert_eq!(whole("\\101", "", "A").as_deref(), Some("A"));
        assert_eq!(whole("\\p{L}+", "u", "12 héllo 34").as_deref(), Some("héllo"));
        assert_eq!(whole("\\p{Lu}", "u", "abcDef").as_deref(), Some("D"));
        assert_eq!(whole("\\P{L}+", "u", "ab12cd").as_deref(), Some("12"));
        assert_eq!(whole("\\p{Script=Greek}+", "u", "abc αβγ").as_deref(), Some("αβγ"));
        assert_eq!(whole("\\p{N}+", "u", "x٣٤y").as_deref(), Some("٣٤"));
        assert_eq!(error("\\p{Nope}", "u"), "Invalid property name");
        let bidi = Regex::new("[\\p{Bidi_Control}\\u0590-\\u08ff]", "u").unwrap();
        assert!(bidi.is_match("a\u{202E}b"));
        assert!(!bidi.is_match("abc"));
        let rtl = Regex::new("[\\p{Script=Arabic}\\p{Script=Syriac}\\p{Script=Thaana}\\p{Script=Mandaic}]", "u").unwrap();
        assert!(rtl.is_match("\u{0710}"));
        let marks = Regex::new("[\\p{Mn}\\p{Mc}\\p{Me}]", "u").unwrap();
        assert!(marks.is_match("e\u{301}"));
        for cat in ["Pc", "Pd", "Ps", "Pe", "Pi", "Pf", "Po", "Sm", "Sc", "Sk", "So", "Cf", "Co", "Cs", "Cn"] {
            assert!(Regex::new(&format!("\\p{{{cat}}}"), "u").is_ok(), "{cat}");
        }
        assert_eq!(whole("\\p{L}", "", "p{L}").as_deref(), Some("p{L}"));
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(whole("abc", "i", "xABCx").as_deref(), Some("ABC"));
        assert_eq!(whole("é", "i", "É").as_deref(), Some("É"));
        assert_eq!(whole("[a-z]+", "i", "HeLLo").as_deref(), Some("HeLLo"));
        assert_eq!(whole("[^a-z]", "i", "ABC"), None);
        assert_eq!(whole("Σ", "i", "σ").as_deref(), Some("σ"));
        assert_eq!(whole("ß", "i", "ẞ"), None);
        assert_eq!(whole("ß", "iu", "ẞ").as_deref(), Some("ẞ"));
        assert_eq!(whole("^[a-z0-9._%+-]+@", "i", "John.Doe@x").as_deref(), Some("John.Doe@"));
        // Non-unicode: ſ must not match 's'.
        assert_eq!(whole("s", "i", "\u{17f}"), None);
        assert_eq!(whole("s", "iu", "\u{17f}").as_deref(), Some("\u{17f}"));
    }

    #[test]
    fn unicode_mode() {
        assert_eq!(whole("^.$", "u", "😀").as_deref(), Some("😀"));
        assert_eq!(whole("^.$", "", "😀"), None);
        assert_eq!(whole("^..$", "", "😀").as_deref(), Some("😀"));
        assert_eq!(whole("[😀-😂]", "u", "😁").as_deref(), Some("😁"));
        assert_eq!(whole("^[^x]$", "u", "😀").as_deref(), Some("😀"));
        assert_eq!(whole("😀+", "", "😀😀").as_deref(), Some("😀😀"));
        assert_eq!(whole("😀+", "u", "😀😀").as_deref(), Some("😀😀"));
        // Search in unicode mode steps over whole code points.
        let re = Regex::new("\\uDE00", "u").unwrap();
        assert!(re.exec("😀", 0).is_none());
        let re = Regex::new("\\uDE00", "").unwrap();
        assert_eq!(re.exec("😀", 0).unwrap().start(), 1);
        assert_eq!(whole("(?<=😀)x", "u", "😀x").as_deref(), Some("x"));
    }

    #[test]
    fn sticky_and_offsets() {
        let re = Regex::new("b", "y").unwrap();
        assert!(re.exec("ab", 0).is_none());
        assert_eq!(re.exec("ab", 1).unwrap().start(), 1);
        let re = Regex::new("b", "").unwrap();
        assert_eq!(re.exec("ab", 0).unwrap().captures[0], Some((1, 2)));
        assert!(re.exec("ab", 2).is_none());
        assert!(re.exec("ab", 5).is_none());
        let re = Regex::new("$", "").unwrap();
        assert_eq!(re.exec("ab", 0).unwrap().captures[0], Some((2, 2)));
        // UTF-16 offsets for astral characters.
        let re = Regex::new("x", "").unwrap();
        assert_eq!(re.exec("😀x", 0).unwrap().captures[0], Some((2, 3)));
        let units: Vec<u16> = "😀x".encode_utf16().collect();
        assert_eq!(re.exec_utf16(&units, 0).unwrap().group_str(&units, 0).as_deref(), Some("x"));
    }

    #[test]
    fn errors() {
        assert_eq!(error("(", ""), "Unterminated group");
        assert_eq!(error(")", ""), "Unmatched ')'");
        assert_eq!(error("a)", ""), "Unmatched ')'");
        assert_eq!(error("*a", ""), "Nothing to repeat");
        assert_eq!(error("a**", ""), "Nothing to repeat");
        assert_eq!(error("^*", ""), "Nothing to repeat");
        assert_eq!(error("a{2,1}", ""), "numbers out of order in {} quantifier");
        assert_eq!(error("(?x)", ""), "Invalid group");
        assert_eq!(error("\\", ""), "\\ at end of pattern");
        assert_eq!(error("a{", "u"), "Nothing to repeat");
        assert_eq!(error("\\u{110000}", "u"), "Invalid Unicode escape");
        assert_eq!(error("(?<1a>x)", ""), "Invalid capture group name");
        assert_eq!(error("a", "gg"), "Invalid flags supplied to RegExp constructor 'gg'");
    }

    #[test]
    fn realistic_patterns() {
        let email = "^[a-z0-9._%+-]+@[a-z0-9.-]+\\.[a-z]{2,}$";
        assert!(Regex::new(email, "i").unwrap().is_match("Jane.Doe@Example.co.uk"));
        assert!(!Regex::new(email, "i").unwrap().is_match("nope@"));
        let url = "^(https?):\\/\\/([^/:]+)(?::(\\d+))?(\\/[^?#]*)?(?:\\?([^#]*))?(?:#(.*))?$";
        assert_eq!(
            m(url, "", "https://example.com:8080/a/b?x=1#frag").unwrap(),
            vec![
                Some("https://example.com:8080/a/b?x=1#frag".into()),
                Some("https".into()),
                Some("example.com".into()),
                Some("8080".into()),
                Some("/a/b".into()),
                Some("x=1".into()),
                Some("frag".into())
            ]
        );
        let date = "(?<year>\\d{4})-(?<month>\\d{2})-(?<day>\\d{2})";
        assert_eq!(whole(date, "", "Due 2026-11-09.").as_deref(), Some("2026-11-09"));
        assert_eq!(whole("\\$\\{([^}]+)\\}", "g", "a ${b} c").as_deref(), Some("${b}"));
        assert_eq!(whole("^\\s*$", "", "   ").as_deref(), Some("   "));
        assert_eq!(whole("(\\w+)@(\\w+)\\.com", "", "me@site.com").as_deref(), Some("me@site.com"));
        assert_eq!(whole("[A-Z][a-z]+(?:\\s[A-Z][a-z]+)*", "", "hello John Smith here").as_deref(), Some("John Smith"));
        assert_eq!(whole("^(?:(?:25[0-5]|2[0-4]\\d|1?\\d?\\d)\\.){3}(?:25[0-5]|2[0-4]\\d|1?\\d?\\d)$", "", "192.168.1.110").as_deref(), Some("192.168.1.110"));
        assert_eq!(whole("<\\/?([a-z][a-z0-9]*)\\b[^>]*>", "i", "x <DIV class='a'> y").as_deref(), Some("<DIV class='a'>"));
    }
}
