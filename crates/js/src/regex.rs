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
        let _ = flags;
        unimplemented!()
    }

    /// Canonical flag string in the order `dgimsuy`.
    pub fn to_string(&self) -> String {
        unimplemented!()
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

/// A compiled regular expression.
#[derive(Debug, Clone)]
pub struct Regex {
    pub source: String,
    pub flags: Flags,
    // Implementation-private fields follow (program / AST / group names).
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
}

impl Regex {
    /// Compile `pattern` with `flags` (as given in the literal / constructor).
    pub fn new(pattern: &str, flags: &str) -> Result<Regex, RegexError> {
        let _ = (pattern, flags);
        unimplemented!()
    }

    /// Number of capturing groups (excluding group 0).
    pub fn group_count(&self) -> usize {
        unimplemented!()
    }

    /// Named groups as (name, group index), in pattern order.
    pub fn group_names(&self) -> &[(String, usize)] {
        unimplemented!()
    }

    /// Find the first match in `subject` (UTF-16 code units) starting the
    /// search at code unit index `start`. With the sticky flag the match
    /// must begin exactly at `start`.
    pub fn exec_utf16(&self, subject: &[u16], start: usize) -> Option<Match> {
        let _ = (subject, start);
        unimplemented!()
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
