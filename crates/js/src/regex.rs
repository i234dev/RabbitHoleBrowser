//! ECMAScript regular expressions: parser + backtracking matcher.
//!
//! Implements the ES2025 pattern grammar with Annex B: alternation, groups
//! (capturing, non-capturing, named — including duplicate names in
//! different alternatives — and `(?ims-ims:…)` modifiers), quantifiers,
//! character classes (with `/v` set operations, nested classes and `\q{…}`
//! strings), escapes (`\d \w \s \b \B`, unicode escapes, `\p{…}` property
//! escapes over the full Unicode data in `regex_unicode.rs`), anchors,
//! lookahead/lookbehind, backreferences and the flags `d g i m s u v y`.
//!
//! Matching works on UTF-16 code units (like JavaScript), so indices
//! returned here are UTF-16 offsets into the subject. In unicode mode
//! (`u` or `v`) the pattern and the subject are read as code points.
//!
//! # Design
//!
//! The pattern is parsed into a small AST ([`Node`]) which is compiled into
//! a linear program ([`Inst`]) for a backtracking machine. Case-insensitive
//! matching is resolved at compile time: literals and classes are closed
//! under case equivalence, so the matcher only compares code points. The
//! machine keeps an explicit backtrack stack holding both alternatives
//! (`Split` targets) and undo records for captures / loop registers, so
//! matching never recurses on the input length; recursion only happens for
//! nested lookaround, bounded by the pattern's nesting depth. Lookbehind
//! bodies are compiled in backward direction (terms reversed, consuming
//! instructions flagged `back`), exactly as the specification describes.
//! A quantified single-character atom (`\s+`, `.*`, `[a-z]{2,}`) runs as
//! one instruction with one backtrack record, and large counted quantifiers
//! use counter registers instead of unrolled copies.

use std::collections::{BTreeSet, HashMap};
use std::sync::OnceLock;

#[path = "regex_unicode.rs"]
mod tables;

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
    pub unicode_sets: bool,
    pub sticky: bool,
    pub has_indices: bool,
}

impl Flags {
    /// Parse a flag string such as `"gi"`. Unknown or repeated flags, or
    /// both `u` and `v` → None.
    pub fn parse(flags: &str) -> Option<Flags> {
        let mut f = Flags::default();
        for c in flags.chars() {
            let slot = match c {
                'g' => &mut f.global,
                'i' => &mut f.ignore_case,
                'm' => &mut f.multiline,
                's' => &mut f.dot_all,
                'u' => &mut f.unicode,
                'v' => &mut f.unicode_sets,
                'y' => &mut f.sticky,
                'd' => &mut f.has_indices,
                _ => return None,
            };
            if *slot {
                return None;
            }
            *slot = true;
        }
        if f.unicode && f.unicode_sets {
            return None;
        }
        Some(f)
    }

    /// `u` or `v`: the pattern and subject are read as code points.
    pub fn either_unicode(&self) -> bool {
        self.unicode || self.unicode_sets
    }

    /// Canonical flag string in the order `dgimsuvy`.
    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> String {
        let mut s = String::new();
        for (on, c) in [
            (self.has_indices, 'd'),
            (self.global, 'g'),
            (self.ignore_case, 'i'),
            (self.multiline, 'm'),
            (self.dot_all, 's'),
            (self.unicode, 'u'),
            (self.unicode_sets, 'v'),
            (self.sticky, 'y'),
        ] {
            if on {
                s.push(c);
            }
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
    /// Distinct group names in order of first appearance, each with every
    /// group index carrying it (several with duplicate named groups).
    named_groups: Vec<(String, Vec<usize>)>,
    /// Number of loop registers used by the program.
    registers: usize,
    /// Code units one of which is required at the match start, when the
    /// program starts with a literal (used to skip ahead quickly).
    first_units: Option<(u16, u16)>,
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

/// The pattern as parser input: code points in unicode mode (lone
/// surrogates kept), UTF-16 code units otherwise.
fn pattern_input(units: &[u16], unicode: bool) -> Vec<u32> {
    if !unicode {
        return units.iter().map(|&u| u as u32).collect();
    }
    let mut out = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        let u = units[i];
        if is_high(u) && i + 1 < units.len() && is_low(units[i + 1]) {
            out.push(0x10000 + (((u as u32) - 0xD800) << 10) + ((units[i + 1] as u32) - 0xDC00));
            i += 2;
        } else {
            out.push(u as u32);
            i += 1;
        }
    }
    out
}

/// Check a pattern / flags pair for early errors (regular expression
/// literals are validated when the script is parsed).
pub fn validate(pattern: &str, flags: &str) -> Result<(), RegexError> {
    let Some(flags) = Flags::parse(flags) else {
        return err(&format!("Invalid regular expression flags '{flags}'"));
    };
    let units: Vec<u16> = pattern.encode_utf16().collect();
    let input = pattern_input(&units, flags.either_unicode());
    let mut parser = Parser::new(&input, flags);
    parser.parse().map(|_| ())
}

impl Regex {
    /// Compile `pattern` with `flags` (as given in the literal / constructor).
    pub fn new(pattern: &str, flags: &str) -> Result<Regex, RegexError> {
        let units: Vec<u16> = pattern.encode_utf16().collect();
        Self::new_utf16(&units, flags)
    }

    /// Compile a pattern given as UTF-16 code units (may hold lone surrogates).
    pub fn new_utf16(pattern: &[u16], flags: &str) -> Result<Regex, RegexError> {
        let flags = match Flags::parse(flags) {
            Some(f) => f,
            None => return err(&format!("Invalid flags supplied to RegExp constructor '{flags}'")),
        };
        let input = pattern_input(pattern, flags.either_unicode());
        let mut parser = Parser::new(&input, flags);
        let mut node = parser.parse()?;
        let names = parser.names_map();
        node.resolve_names(&names);
        let mut compiler = Compiler {
            program: Vec::new(),
            classes: Vec::new(),
            registers: 0,
        };
        compiler.compile(&node, false)?;
        compiler.program.push(Inst::Match);
        let unicode = flags.either_unicode();
        // A literal unit usable for skipping: BMP, and not a surrogate in
        // unicode mode (where it could be half of a pair read as one).
        let unit_ok = |u: u32| u <= 0xFFFF && !(unicode && (0xD800..=0xDFFF).contains(&u));
        let first_units = match compiler.program.first() {
            Some(Inst::Char { unit, back: false }) if unit_ok(*unit) => Some((*unit as u16, *unit as u16)),
            Some(Inst::Char2 { a, b, back: false }) if unit_ok(*a) && unit_ok(*b) => Some((*a as u16, *b as u16)),
            Some(Inst::Star { single: Single::Char(unit), min, .. }) if *min > 0 && unit_ok(*unit) => Some((*unit as u16, *unit as u16)),
            Some(Inst::Star { single: Single::Char2(a, b), min, .. }) if *min > 0 && unit_ok(*a) && unit_ok(*b) => Some((*a as u16, *b as u16)),
            _ => None,
        };
        let mut named_groups: Vec<(String, Vec<usize>)> = Vec::new();
        for (name, idx) in &parser.group_names {
            match named_groups.iter_mut().find(|(n, _)| n == name) {
                Some((_, v)) => v.push(*idx),
                None => named_groups.push((name.clone(), vec![*idx])),
            }
        }
        Ok(Regex {
            source: String::from_utf16_lossy(pattern),
            flags,
            program: compiler.program,
            classes: compiler.classes,
            group_count: parser.group_count,
            group_names: parser.group_names,
            named_groups,
            registers: compiler.registers,
            first_units,
        })
    }

    /// Number of capturing groups (excluding group 0).
    pub fn group_count(&self) -> usize {
        self.group_count
    }

    /// Named groups as (name, group index), in pattern order (a duplicated
    /// name appears once per group).
    pub fn group_names(&self) -> &[(String, usize)] {
        &self.group_names
    }

    /// Distinct group names in order of first appearance with their group
    /// indices.
    pub fn named_groups(&self) -> &[(String, Vec<usize>)] {
        &self.named_groups
    }

    /// Find the first match in `subject` (UTF-16 code units) starting the
    /// search at code unit index `start`. With the sticky flag the match
    /// must begin exactly at `start`.
    pub fn exec_utf16(&self, subject: &[u16], start: usize) -> Option<Match> {
        self.search_utf16(subject, start, self.flags.sticky)
    }

    /// [`Regex::exec_utf16`] with the sticky behaviour given explicitly.
    pub fn search_utf16(&self, subject: &[u16], start: usize, sticky: bool) -> Option<Match> {
        if start > subject.len() {
            return None;
        }
        let mut machine = Machine {
            regex: self,
            subject,
            unicode: self.flags.either_unicode(),
            caps: vec![None; (self.group_count + 1) * 2],
            regs: vec![usize::MAX; self.registers],
            stack: Vec::new(),
        };
        if sticky {
            return machine.run_at(start);
        }
        let mut pos = start;
        loop {
            if let Some((a, b)) = self.first_units {
                let found = if a == b { subject[pos..].iter().position(|&u| u == a) } else { subject[pos..].iter().position(|&u| u == a || u == b) };
                match found {
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
            if machine.unicode && is_high(subject[pos]) && pos + 1 < subject.len() && is_low(subject[pos + 1]) {
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

// ------------------------------------------------------- code point sets

/// Sorted, non-overlapping, non-adjacent inclusive ranges.
type Ranges = Vec<(u32, u32)>;

const MAX_CP: u32 = 0x10FFFF;

fn normalize(mut r: Ranges) -> Ranges {
    r.sort_unstable();
    let mut out: Ranges = Vec::with_capacity(r.len());
    for (lo, hi) in r {
        if let Some(last) = out.last_mut() {
            if lo <= last.1.saturating_add(1) {
                last.1 = last.1.max(hi);
                continue;
            }
        }
        out.push((lo, hi));
    }
    out
}

fn ranges_contain(r: &[(u32, u32)], c: u32) -> bool {
    let mut lo = 0usize;
    let mut hi = r.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        let (a, b) = r[mid];
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

fn complement(r: &[(u32, u32)]) -> Ranges {
    let mut out = Vec::with_capacity(r.len() + 1);
    let mut next = 0u32;
    for &(lo, hi) in r {
        if lo > next {
            out.push((next, lo - 1));
        }
        next = hi.saturating_add(1);
    }
    if next <= MAX_CP {
        out.push((next, MAX_CP));
    }
    out
}

fn union(a: &[(u32, u32)], b: &[(u32, u32)]) -> Ranges {
    if b.is_empty() {
        return a.to_vec();
    }
    if a.is_empty() {
        return b.to_vec();
    }
    let mut v = a.to_vec();
    v.extend_from_slice(b);
    normalize(v)
}

fn intersect(a: &[(u32, u32)], b: &[(u32, u32)]) -> Ranges {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let lo = a[i].0.max(b[j].0);
        let hi = a[i].1.min(b[j].1);
        if lo <= hi {
            out.push((lo, hi));
        }
        if a[i].1 < b[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

fn subtract(a: &[(u32, u32)], b: &[(u32, u32)]) -> Ranges {
    if b.is_empty() {
        return a.to_vec();
    }
    intersect(a, &complement(b))
}

fn range_size(r: &[(u32, u32)]) -> u64 {
    r.iter().map(|&(a, b)| (b - a) as u64 + 1).sum()
}

// ------------------------------------------------------------ unicode data

fn decode(bytes: &[u8]) -> Ranges {
    let mut out = Vec::new();
    let mut i = 0;
    let mut prev = 0u32;
    let read = |i: &mut usize| -> u32 {
        let mut v = 0u32;
        let mut shift = 0;
        loop {
            let b = bytes[*i];
            *i += 1;
            v |= ((b & 0x7F) as u32) << shift;
            if b & 0x80 == 0 {
                return v;
            }
            shift += 7;
        }
    };
    while i < bytes.len() {
        let lo = prev + read(&mut i);
        let hi = lo + read(&mut i);
        out.push((lo, hi));
        prev = hi;
    }
    out
}

fn lookup<T: Copy>(list: &[(&str, T)], name: &str) -> Option<T> {
    list.binary_search_by(|(k, _)| (*k).cmp(name)).ok().map(|i| list[i].1)
}

const GC_GROUPS: &[(&str, &[&str])] = &[
    ("C", &["Cc", "Cf", "Cn", "Co", "Cs"]),
    ("L", &["Ll", "Lm", "Lo", "Lt", "Lu"]),
    ("LC", &["Ll", "Lt", "Lu"]),
    ("M", &["Mc", "Me", "Mn"]),
    ("N", &["Nd", "Nl", "No"]),
    ("P", &["Pc", "Pd", "Pe", "Pf", "Pi", "Po", "Ps"]),
    ("S", &["Sc", "Sk", "Sm", "So"]),
    ("Z", &["Zl", "Zp", "Zs"]),
];

fn general_category(value: &str) -> Option<Ranges> {
    let short = lookup(tables::GC_ALIASES, value)?;
    if let Some((_, leaves)) = GC_GROUPS.iter().find(|(g, _)| *g == short) {
        let mut all = Vec::new();
        for leaf in leaves.iter() {
            all.extend(decode(lookup(tables::GC, leaf)?));
        }
        return Some(normalize(all));
    }
    Some(decode(lookup(tables::GC, short)?))
}

fn binary_property(name: &str) -> Option<Ranges> {
    let canonical = lookup(tables::BINARY_ALIASES, name)?;
    Some(decode(lookup(tables::BINARY, canonical)?))
}

/// A property of strings (`v` mode): its single code points and sequences.
fn string_property(name: &str) -> Option<(Ranges, Vec<Vec<u32>>)> {
    let parts: &[&str] = match name {
        "RGI_Emoji" => &[
            "Basic_Emoji",
            "Emoji_Keycap_Sequence",
            "RGI_Emoji_Flag_Sequence",
            "RGI_Emoji_Modifier_Sequence",
            "RGI_Emoji_Tag_Sequence",
            "RGI_Emoji_ZWJ_Sequence",
        ],
        "Basic_Emoji"
        | "Emoji_Keycap_Sequence"
        | "RGI_Emoji_Flag_Sequence"
        | "RGI_Emoji_Modifier_Sequence"
        | "RGI_Emoji_Tag_Sequence"
        | "RGI_Emoji_ZWJ_Sequence" => std::slice::from_ref(&name),
        _ => return None,
    };
    let mut singles = Vec::new();
    let mut strings = Vec::new();
    for part in parts {
        if *part == "Basic_Emoji" {
            singles = decode(tables::BASIC_EMOJI_SINGLE);
        }
        let data = lookup(tables::STRING_PROPS, part)?;
        for seq in data.split(' ').filter(|s| !s.is_empty()) {
            strings.push(seq.split(',').map(|h| u32::from_str_radix(h, 16).unwrap_or(0)).collect());
        }
    }
    Some((singles, strings))
}

/// Resolve `\p{name=value}` / `\p{value}`. Returns the code point set and,
/// for properties of strings (only when `strings_ok`), the sequences.
fn unicode_property(body: &[u32], strings_ok: bool) -> Option<(Ranges, Vec<Vec<u32>>)> {
    let text: String = body.iter().map(|&c| char::from_u32(c).unwrap_or('\u{0}')).collect();
    if text.is_empty() || !text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '=') {
        return None;
    }
    match text.split_once('=') {
        Some((name, value)) => {
            if value.contains('=') || name.is_empty() || value.is_empty() {
                return None;
            }
            let set = match name {
                "General_Category" | "gc" => general_category(value)?,
                "Script" | "sc" => decode(lookup(tables::SC, lookup(tables::SC_ALIASES, value)?)?),
                "Script_Extensions" | "scx" => decode(lookup(tables::SCX, lookup(tables::SC_ALIASES, value)?)?),
                _ => return None,
            };
            Some((set, Vec::new()))
        }
        None => {
            if let Some(set) = general_category(&text) {
                return Some((set, Vec::new()));
            }
            if let Some(set) = binary_property(&text) {
                return Some((set, Vec::new()));
            }
            if strings_ok {
                return string_property(&text);
            }
            None
        }
    }
}

fn id_start() -> &'static Ranges {
    static S: OnceLock<Ranges> = OnceLock::new();
    S.get_or_init(|| binary_property("ID_Start").unwrap_or_default())
}

fn id_continue() -> &'static Ranges {
    static S: OnceLock<Ranges> = OnceLock::new();
    S.get_or_init(|| binary_property("ID_Continue").unwrap_or_default())
}

/// Simple case folding (scf).
fn simple_fold(c: u32) -> u32 {
    match tables::SCF.binary_search_by_key(&c, |&(k, _)| k) {
        Ok(i) => tables::SCF[i].1,
        Err(_) => c,
    }
}

/// Canonicalize (ECMA-262 22.2.2.7.3) for case-insensitive matching.
fn canonicalize(c: u32, unicode: bool) -> u32 {
    if unicode {
        return simple_fold(c);
    }
    if c > 0xFFFF {
        return c;
    }
    match tables::UPPER16.binary_search_by_key(&(c as u16), |&(k, _)| k) {
        Ok(i) => tables::UPPER16[i].1 as u32,
        Err(_) => c,
    }
}

/// Case equivalence classes (characters with the same canonical form), for
/// unicode (simple case folding) or legacy (toUppercase) canonicalization.
struct Orbits {
    /// (character, orbit index), sorted by character.
    index: Vec<(u32, u32)>,
    orbits: Vec<Vec<u32>>,
}

fn orbits(unicode: bool) -> &'static Orbits {
    fn build(unicode: bool) -> Orbits {
        let mut chars: Vec<u32> = if unicode {
            tables::SCF.iter().flat_map(|&(a, b)| [a, b]).collect()
        } else {
            tables::UPPER16.iter().flat_map(|&(a, b)| [a as u32, b as u32]).collect()
        };
        chars.sort_unstable();
        chars.dedup();
        let mut by_canon: HashMap<u32, Vec<u32>> = HashMap::new();
        for c in chars {
            by_canon.entry(canonicalize(c, unicode)).or_default().push(c);
        }
        let mut orbits: Vec<Vec<u32>> = by_canon.into_values().filter(|v| v.len() > 1).collect();
        orbits.sort_unstable();
        let mut index = Vec::new();
        for (i, o) in orbits.iter().enumerate() {
            for &c in o {
                index.push((c, i as u32));
            }
        }
        index.sort_unstable();
        Orbits { index, orbits }
    }
    static U: OnceLock<Orbits> = OnceLock::new();
    static L: OnceLock<Orbits> = OnceLock::new();
    if unicode {
        U.get_or_init(|| build(true))
    } else {
        L.get_or_init(|| build(false))
    }
}

fn orbit_of(c: u32, unicode: bool) -> Option<&'static [u32]> {
    let o = orbits(unicode);
    match o.index.binary_search_by_key(&c, |&(k, _)| k) {
        Ok(i) => Some(&o.orbits[o.index[i].1 as usize]),
        Err(_) => None,
    }
}

/// Close a set under case equivalence: every character whose canonical
/// form equals that of a member is added.
fn case_closure(r: &[(u32, u32)], unicode: bool) -> Ranges {
    let mut extra = Vec::new();
    if range_size(r) <= 64 {
        for &(a, b) in r {
            for c in a..=b {
                if let Some(o) = orbit_of(c, unicode) {
                    extra.extend(o.iter().map(|&m| (m, m)));
                }
            }
        }
    } else {
        for o in &orbits(unicode).orbits {
            if o.iter().any(|&m| ranges_contain(r, m)) {
                extra.extend(o.iter().filter(|&&m| !ranges_contain(r, m)).map(|&m| (m, m)));
            }
        }
    }
    if extra.is_empty() {
        return r.to_vec();
    }
    extra.extend_from_slice(r);
    normalize(extra)
}

/// Characters that change under simple case folding.
fn folding_chars() -> &'static Ranges {
    static S: OnceLock<Ranges> = OnceLock::new();
    S.get_or_init(|| normalize(tables::SCF.iter().map(|&(a, _)| (a, a)).collect()))
}

/// MaybeSimpleCaseFolding for `v` mode with `i`: map every member to its
/// simple case folding.
fn fold_set(r: &[(u32, u32)]) -> Ranges {
    let mut out = subtract(r, folding_chars());
    let mut extra = Vec::new();
    for &(a, b) in tables::SCF {
        if ranges_contain(r, a) {
            extra.push((b, b));
        }
    }
    if !extra.is_empty() {
        out.extend(extra);
        out = normalize(out);
    }
    out
}

// ------------------------------------------------------------------ AST

#[derive(Debug, Clone)]
enum Node {
    Empty,
    /// A literal code point (or code unit in non-unicode mode), exact.
    Char(u32),
    Any {
        dot_all: bool,
    },
    /// A set of characters (case closure already applied).
    Set(CharClass),
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
    Backref {
        groups: Vec<usize>,
        ignore_case: bool,
    },
    NamedBackref {
        name: String,
        ignore_case: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssertKind {
    Start,
    End,
    LineStart,
    LineEnd,
    WordBoundary { extended: bool },
    NotWordBoundary { extended: bool },
}

/// A set of code points, as sorted, non-overlapping ranges.
#[derive(Debug, Clone, Default)]
struct CharClass {
    ranges: Ranges,
    negate: bool,
}

impl CharClass {
    #[inline]
    fn matches(&self, c: u32) -> bool {
        ranges_contain(&self.ranges, c) != self.negate
    }
}

fn digit_ranges() -> Ranges {
    vec![('0' as u32, '9' as u32)]
}

fn word_ranges() -> Ranges {
    vec![('0' as u32, '9' as u32), ('A' as u32, 'Z' as u32), ('_' as u32, '_' as u32), ('a' as u32, 'z' as u32)]
}

fn space_ranges() -> Ranges {
    vec![
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
    ]
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

fn is_syntax_char(c: u32) -> bool {
    matches!(char::from_u32(c), Some('^' | '$' | '\\' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|'))
}

// --------------------------------------------------------------- parser

#[derive(Debug, Clone, Copy)]
struct LocalFlags {
    ignore_case: bool,
    multiline: bool,
    dot_all: bool,
}

/// A class operand in `v` mode: code points plus strings (sequences whose
/// length is not 1), and whether it may contain strings syntactically.
#[derive(Debug, Clone, Default)]
struct VSet {
    ranges: Ranges,
    strings: BTreeSet<Vec<u32>>,
    may_strings: bool,
}

impl VSet {
    fn chars(ranges: Ranges) -> VSet {
        VSet {
            ranges,
            strings: BTreeSet::new(),
            may_strings: false,
        }
    }
    fn union_with(&mut self, other: VSet) {
        self.ranges = union(&self.ranges, &other.ranges);
        self.strings.extend(other.strings);
        self.may_strings |= other.may_strings;
    }
}

/// An escape inside a class (non-`v` mode): one character or a set.
enum ClassAtom {
    Char(u32),
    Set(Ranges),
}

const MAX_DEPTH: u32 = 400;

struct Parser<'a> {
    src: &'a [u32],
    pos: usize,
    /// `u` or `v`.
    unicode: bool,
    vmode: bool,
    /// The [NamedCaptureGroups] grammar parameter (`\k` is a reference).
    named_mode: bool,
    local: LocalFlags,
    group_count: usize,
    /// Total number of capturing groups in the pattern (pre-scanned) so
    /// `\N` can be classified as backreference vs. legacy octal.
    total_groups: usize,
    group_names: Vec<(String, usize)>,
    /// For each entry of `group_names`: the (disjunction, alternative)
    /// path leading to it, to tell whether two groups can both participate.
    name_paths: Vec<Vec<(u32, u32)>>,
    disj_stack: Vec<(u32, u32)>,
    next_disj: u32,
    /// Named backreferences seen, validated after parsing.
    named_refs: Vec<String>,
    depth: u32,
}

fn ch(c: char) -> u32 {
    c as u32
}

impl<'a> Parser<'a> {
    fn new(src: &'a [u32], flags: Flags) -> Self {
        let vmode = flags.unicode_sets;
        let (total_groups, has_named_groups) = prescan_groups(src, vmode);
        let unicode = flags.either_unicode();
        Parser {
            src,
            pos: 0,
            unicode,
            vmode,
            named_mode: unicode || has_named_groups,
            local: LocalFlags {
                ignore_case: flags.ignore_case,
                multiline: flags.multiline,
                dot_all: flags.dot_all,
            },
            group_count: 0,
            total_groups,
            group_names: Vec::new(),
            name_paths: Vec::new(),
            disj_stack: Vec::new(),
            next_disj: 0,
            named_refs: Vec::new(),
            depth: 0,
        }
    }

    fn peek(&self) -> Option<u32> {
        self.src.get(self.pos).copied()
    }

    fn peek_at(&self, off: usize) -> Option<u32> {
        self.src.get(self.pos + off).copied()
    }

    fn is(&self, c: char) -> bool {
        self.peek() == Some(c as u32)
    }

    fn is_at(&self, off: usize, c: char) -> bool {
        self.peek_at(off) == Some(c as u32)
    }

    fn eat(&mut self, c: char) -> bool {
        if self.is(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn parse(&mut self) -> Result<Node, RegexError> {
        let node = self.parse_disjunction()?;
        if self.pos < self.src.len() {
            if self.is(')') {
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
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return err("Regular expression too deeply nested");
        }
        let id = self.next_disj;
        self.next_disj += 1;
        self.disj_stack.push((id, 0));
        let mut alts = vec![self.parse_alternative()?];
        while self.eat('|') {
            if let Some(top) = self.disj_stack.last_mut() {
                top.1 += 1;
            }
            alts.push(self.parse_alternative()?);
        }
        self.disj_stack.pop();
        self.depth -= 1;
        if alts.len() == 1 {
            Ok(alts.pop().unwrap())
        } else {
            Ok(Node::Alternation(alts))
        }
    }

    fn parse_alternative(&mut self) -> Result<Node, RegexError> {
        let mut terms = Vec::new();
        while let Some(c) = self.peek() {
            if c == ch('|') || c == ch(')') {
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
        let atom = match char::from_u32(c).unwrap_or('\u{FFFD}') {
            '^' => {
                self.pos += 1;
                return Ok(Node::Assert(if self.local.multiline { AssertKind::LineStart } else { AssertKind::Start }));
            }
            '$' => {
                self.pos += 1;
                return Ok(Node::Assert(if self.local.multiline { AssertKind::LineEnd } else { AssertKind::End }));
            }
            '\\' if self.is_at(1, 'b') || self.is_at(1, 'B') => {
                let extended = self.unicode && self.local.ignore_case;
                let kind = if self.is_at(1, 'b') {
                    AssertKind::WordBoundary { extended }
                } else {
                    AssertKind::NotWordBoundary { extended }
                };
                self.pos += 2;
                if self.at_quantifier() {
                    return err("Nothing to repeat");
                }
                return Ok(Node::Assert(kind));
            }
            '(' if self.is_at(1, '?') && (self.is_at(2, '=') || self.is_at(2, '!')) => {
                let negate = self.is_at(2, '!');
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
                if !self.unicode {
                    return self.parse_quantifier(node, groups_before);
                }
                if self.at_quantifier() {
                    return err("Nothing to repeat");
                }
                return Ok(node);
            }
            '(' if self.is_at(1, '?') && self.is_at(2, '<') && (self.is_at(3, '=') || self.is_at(3, '!')) => {
                let negate = self.is_at(3, '!');
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
            '(' => self.parse_group()?,
            ')' => return err("Unmatched ')'"),
            '*' | '+' | '?' => return err("Nothing to repeat"),
            '{' => {
                if self.unicode {
                    return err("Nothing to repeat");
                }
                // Annex B: a `{` that is not a valid quantifier is a literal.
                if self.try_parse_braces().is_some() {
                    return err("Nothing to repeat");
                }
                self.pos += 1;
                self.char_node(c)
            }
            '}' | ']' => {
                if self.unicode {
                    return err("Lone quantifier brackets");
                }
                self.pos += 1;
                self.char_node(c)
            }
            '[' => {
                self.pos += 1;
                if self.vmode {
                    let set = self.parse_class_v()?;
                    self.vset_node(set)
                } else {
                    self.parse_class()?
                }
            }
            '.' => {
                self.pos += 1;
                Node::Any {
                    dot_all: self.local.dot_all,
                }
            }
            '\\' => {
                self.pos += 1;
                self.parse_atom_escape()?
            }
            _ => {
                self.pos += 1;
                self.char_node(c)
            }
        };
        self.parse_quantifier(atom, groups_before)
    }

    /// A literal character, case-closed when ignoring case.
    fn char_node(&self, c: u32) -> Node {
        if self.local.ignore_case {
            if let Some(o) = orbit_of(c, self.unicode) {
                return Node::Set(CharClass {
                    ranges: normalize(o.iter().map(|&m| (m, m)).collect()),
                    negate: false,
                });
            }
        }
        Node::Char(c)
    }

    /// A set node (case-closed when ignoring case).
    fn set_node(&self, ranges: Ranges, negate: bool) -> Node {
        let ranges = if self.local.ignore_case { case_closure(&ranges, self.unicode) } else { ranges };
        Node::Set(CharClass { ranges, negate })
    }

    fn at_quantifier(&self) -> bool {
        matches!(self.peek(), Some(c) if c == ch('*') || c == ch('+') || c == ch('?'))
            || (self.is('{') && self.try_parse_braces().is_some())
    }

    /// Parse `{n}`, `{n,}` or `{n,m}` at the current position without
    /// consuming; returns (min, max, length).
    fn try_parse_braces(&self) -> Option<(u64, Option<u64>, usize)> {
        let mut i = self.pos;
        if self.src.get(i) != Some(&ch('{')) {
            return None;
        }
        i += 1;
        let digits = |i: &mut usize| -> Option<u64> {
            let start = *i;
            let mut v: u64 = 0;
            while let Some(&d) = self.src.get(*i) {
                if !(ch('0')..=ch('9')).contains(&d) {
                    break;
                }
                v = v.saturating_mul(10).saturating_add((d - ch('0')) as u64);
                *i += 1;
            }
            if *i == start {
                None
            } else {
                Some(v)
            }
        };
        let min = digits(&mut i)?;
        let mut max = Some(min);
        if self.src.get(i) == Some(&ch(',')) {
            i += 1;
            max = digits(&mut i);
        }
        if self.src.get(i) != Some(&ch('}')) {
            return None;
        }
        Some((min, max, i + 1 - self.pos))
    }

    fn parse_quantifier(&mut self, atom: Node, groups_before: usize) -> Result<Node, RegexError> {
        let (min, max) = match self.peek().and_then(char::from_u32) {
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
                None => {
                    if self.unicode {
                        return err("Incomplete quantifier");
                    }
                    return Ok(atom);
                }
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
        let clamp = |v: u64| v.min(u32::MAX as u64) as u32;
        Ok(Node::Repeat {
            body: Box::new(atom),
            min: clamp(min),
            max: max.map(clamp),
            greedy,
            inner: (groups_before + 1, self.group_count),
        })
    }

    fn parse_group(&mut self) -> Result<Node, RegexError> {
        // at '('
        self.pos += 1;
        let mut index = None;
        let saved = self.local;
        if self.eat('?') {
            if self.eat(':') {
                // non-capturing
            } else if self.eat('<') {
                let name = self.parse_group_name()?;
                self.group_count += 1;
                let path = self.disj_stack.clone();
                for (i, (n, _)) in self.group_names.iter().enumerate() {
                    if *n == name && might_both_participate(&self.name_paths[i], &path) {
                        return err("Duplicate capture group name");
                    }
                }
                index = Some(self.group_count);
                self.group_names.push((name, self.group_count));
                self.name_paths.push(path);
            } else {
                self.parse_modifiers()?;
            }
        } else {
            self.group_count += 1;
            index = Some(self.group_count);
        }
        let body = self.parse_disjunction()?;
        self.local = saved;
        if !self.eat(')') {
            return err("Unterminated group");
        }
        Ok(Node::Group {
            index,
            body: Box::new(body),
        })
    }

    /// `(?ims-ims:` after `(?`: apply the modifiers to the local flags.
    fn parse_modifiers(&mut self) -> Result<(), RegexError> {
        let mut seen = [false; 3];
        let mut add = [false; 3];
        let mut remove = [false; 3];
        let mut removing = false;
        let mut any_add = false;
        let mut any_remove = false;
        loop {
            let Some(c) = self.peek().and_then(char::from_u32) else {
                return err("Invalid group");
            };
            self.pos += 1;
            let slot = match c {
                'i' => 0,
                'm' => 1,
                's' => 2,
                '-' if !removing => {
                    removing = true;
                    continue;
                }
                ':' => break,
                _ => return err("Invalid group"),
            };
            if seen[slot] {
                return err("Repeated flag in modifiers");
            }
            seen[slot] = true;
            if removing {
                remove[slot] = true;
                any_remove = true;
            } else {
                add[slot] = true;
                any_add = true;
            }
        }
        if !any_add && !any_remove {
            // `(?-:` (plain `(?:` never gets here).
            return err("Invalid group modifiers");
        }
        let apply = |on: bool, off: bool, cur: bool| if on { true } else if off { false } else { cur };
        self.local.ignore_case = apply(add[0], remove[0], self.local.ignore_case);
        self.local.multiline = apply(add[1], remove[1], self.local.multiline);
        self.local.dot_all = apply(add[2], remove[2], self.local.dot_all);
        Ok(())
    }

    /// One code point of a group name: a raw character (in non-unicode
    /// mode a surrogate pair combines) or a `\u` escape.
    fn group_name_char(&mut self) -> Result<Option<u32>, RegexError> {
        let Some(c) = self.peek() else {
            return err("Invalid capture group name");
        };
        if c == ch('\\') {
            self.pos += 1;
            if !self.eat('u') {
                return err("Invalid capture group name");
            }
            return match self.parse_unicode_escape(true) {
                Some(cp) => Ok(Some(cp)),
                None => err("Invalid Unicode escape"),
            };
        }
        if c == ch('>') {
            return Ok(None);
        }
        self.pos += 1;
        if !self.unicode && (0xD800..=0xDBFF).contains(&c) {
            if let Some(n) = self.peek() {
                if (0xDC00..=0xDFFF).contains(&n) {
                    self.pos += 1;
                    return Ok(Some(0x10000 + ((c - 0xD800) << 10) + (n - 0xDC00)));
                }
            }
        }
        Ok(Some(c))
    }

    /// Parse `name>` of a named group / `\k<name>`.
    fn parse_group_name(&mut self) -> Result<String, RegexError> {
        let mut name = String::new();
        loop {
            let Some(c) = self.group_name_char()? else {
                self.pos += 1; // '>'
                break;
            };
            let ok = if name.is_empty() {
                c == ch('$') || c == ch('_') || ranges_contain(id_start(), c)
            } else {
                c == ch('$') || c == 0x200C || c == 0x200D || ranges_contain(id_continue(), c)
            };
            if !ok {
                return err("Invalid capture group name");
            }
            name.push(char::from_u32(c).unwrap());
        }
        if name.is_empty() {
            return err("Invalid capture group name");
        }
        Ok(name)
    }

    /// After `\u`: parse `HHHH` or (`braces`) `{H+}`. In unicode mode
    /// (or `braces`) a surrogate pair written as two escapes combines.
    /// Does not consume on failure.
    fn parse_unicode_escape(&mut self, unicode: bool) -> Option<u32> {
        if unicode && self.is('{') {
            let save = self.pos;
            self.pos += 1;
            let mut v: u32 = 0;
            let start = self.pos;
            while let Some(d) = self.peek().and_then(|c| char::from_u32(c)?.to_digit(16)) {
                v = v.saturating_mul(16).saturating_add(d);
                if v > MAX_CP {
                    v = MAX_CP + 1;
                }
                self.pos += 1;
            }
            if self.pos == start || !self.is('}') || v > MAX_CP {
                self.pos = save;
                return None;
            }
            self.pos += 1;
            return Some(v);
        }
        let hex4 = |p: &Self, at: usize| -> Option<u32> {
            let mut v = 0;
            for i in 0..4 {
                let d = char::from_u32(*p.src.get(at + i)?)?.to_digit(16)?;
                v = v * 16 + d;
            }
            Some(v)
        };
        let first = hex4(self, self.pos)?;
        self.pos += 4;
        if unicode && (0xD800..=0xDBFF).contains(&first) && self.is('\\') && self.is_at(1, 'u') {
            if let Some(second) = hex4(self, self.pos + 2) {
                if (0xDC00..=0xDFFF).contains(&second) {
                    self.pos += 6;
                    return Some(0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00));
                }
            }
        }
        Some(first)
    }

    /// A CharacterEscape (after `\`), shared by atoms and classes.
    /// `in_class` enables `\b` = backspace, `\-` and Annex B `\c_`.
    fn parse_character_escape(&mut self, in_class: bool) -> Result<u32, RegexError> {
        let Some(c) = self.peek() else {
            return err("\\ at end of pattern");
        };
        self.pos += 1;
        let u = self.unicode;
        let cc = char::from_u32(c).unwrap_or('\u{FFFD}');
        Ok(match cc {
            't' => 0x09,
            'n' => 0x0A,
            'v' => 0x0B,
            'f' => 0x0C,
            'r' => 0x0D,
            'b' if in_class => 0x08,
            '-' if in_class && u => ch('-'),
            'c' => match self.peek().and_then(char::from_u32) {
                Some(l) if l.is_ascii_alphabetic() => {
                    self.pos += 1;
                    (l as u32) % 32
                }
                Some(l) if in_class && !u && (l.is_ascii_digit() || l == '_') => {
                    self.pos += 1;
                    (l as u32) % 32
                }
                _ => {
                    if u {
                        return err("Invalid unicode escape");
                    }
                    // Annex B: `\c` followed by a non-letter is a literal backslash.
                    self.pos -= 1;
                    ch('\\')
                }
            },
            '0' if !matches!(self.peek(), Some(d) if (ch('0')..=ch('9')).contains(&d)) => 0,
            '0'..='9' => {
                if u {
                    return err("Invalid escape");
                }
                if cc >= '8' {
                    c
                } else {
                    // Legacy octal escape (up to 3 digits, value <= 0o377).
                    let mut value = c - ch('0');
                    let mut digits = 1;
                    while digits < 3 {
                        match self.peek() {
                            Some(d) if (ch('0')..=ch('7')).contains(&d) => {
                                let nv = value * 8 + (d - ch('0'));
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
                    value
                }
            }
            'x' => {
                let h = |p: &Self, i: usize| p.peek_at(i).and_then(char::from_u32).and_then(|c| c.to_digit(16));
                match (h(self, 0), h(self, 1)) {
                    (Some(a), Some(b)) => {
                        self.pos += 2;
                        a * 16 + b
                    }
                    _ => {
                        if u {
                            return err("Invalid escape");
                        }
                        ch('x')
                    }
                }
            }
            'u' => match self.parse_unicode_escape(u) {
                Some(cp) => cp,
                None => {
                    if u {
                        return err("Invalid Unicode escape");
                    }
                    ch('u')
                }
            },
            _ => {
                if u {
                    // Identity escapes: only syntax characters and '/'.
                    if is_syntax_char(c) || c == ch('/') {
                        c
                    } else {
                        return err("Invalid escape");
                    }
                } else {
                    if c == ch('k') && self.named_mode {
                        return err("Invalid named reference");
                    }
                    c
                }
            }
        })
    }

    /// `\d \D \s \S \w \W \p{…} \P{…}` (after `\`, at the letter): the set
    /// and, in `v` mode for properties of strings, its strings.
    fn try_class_escape(&mut self) -> Result<Option<VSet>, RegexError> {
        let Some(c) = self.peek().and_then(char::from_u32) else {
            return Ok(None);
        };
        let negate = c.is_ascii_uppercase();
        let vi = self.vmode && self.local.ignore_case;
        let mut strings = BTreeSet::new();
        let mut may_strings = false;
        let base = match c.to_ascii_lowercase() {
            'd' => digit_ranges(),
            's' => space_ranges(),
            'w' => {
                let mut w = word_ranges();
                if self.unicode && self.local.ignore_case {
                    w.push((0x17F, 0x17F));
                    w.push((0x212A, 0x212A));
                    w = normalize(w);
                }
                w
            }
            'p' if self.unicode => {
                self.pos += 1;
                if !self.eat('{') {
                    return err("Invalid property name");
                }
                let start = self.pos;
                while matches!(self.peek(), Some(x) if x != ch('}')) {
                    self.pos += 1;
                }
                if !self.eat('}') {
                    return err("Invalid property name");
                }
                let body = &self.src[start..self.pos - 1];
                let Some((set, strs)) = unicode_property(body, self.vmode && !negate) else {
                    return err("Invalid property name");
                };
                if !strs.is_empty() {
                    may_strings = true;
                    strings = if vi { strs.into_iter().map(|s| s.into_iter().map(simple_fold).collect::<Vec<u32>>()).collect() } else { strs.into_iter().collect() };
                }
                self.pos -= 1; // re-point at '}' for the shared advance below
                set
            }
            _ => return Ok(None),
        };
        self.pos += 1;
        let mut ranges = if vi { fold_set(&base) } else { base };
        if negate {
            ranges = if vi { subtract(&complement(folding_chars()), &ranges) } else { complement(&ranges) };
        }
        Ok(Some(VSet {
            ranges,
            strings,
            may_strings,
        }))
    }

    fn parse_atom_escape(&mut self) -> Result<Node, RegexError> {
        // after '\'
        let Some(c) = self.peek() else {
            return err("\\ at end of pattern");
        };
        if (ch('1')..=ch('9')).contains(&c) {
            // Decimal escape: backreference if <= total group count.
            let start = self.pos;
            let mut n: usize = 0;
            while let Some(d) = self.peek() {
                if !(ch('0')..=ch('9')).contains(&d) {
                    break;
                }
                n = n.saturating_mul(10).saturating_add((d - ch('0')) as usize);
                self.pos += 1;
            }
            if n <= self.total_groups {
                return Ok(Node::Backref {
                    groups: vec![n],
                    ignore_case: self.local.ignore_case,
                });
            }
            if self.unicode {
                return err("Invalid escape");
            }
            // Annex B: legacy octal or identity escape.
            self.pos = start;
            let c = self.parse_character_escape(false)?;
            return Ok(self.char_node(c));
        }
        if c == ch('k') {
            if self.named_mode {
                self.pos += 1;
                if !self.eat('<') {
                    return err("Invalid named reference");
                }
                let name = self.parse_group_name().map_err(|_| RegexError {
                    message: "Invalid named reference".into(),
                })?;
                self.named_refs.push(name.clone());
                return Ok(Node::NamedBackref {
                    name,
                    ignore_case: self.local.ignore_case,
                });
            }
            self.pos += 1;
            return Ok(self.char_node(c));
        }
        if let Some(set) = self.try_class_escape()? {
            if !set.strings.is_empty() {
                return Ok(self.vset_node(set));
            }
            return Ok(self.set_node(set.ranges, false));
        }
        let c = self.parse_character_escape(false)?;
        Ok(self.char_node(c))
    }

    /// A non-`v` class, after `[`.
    fn parse_class(&mut self) -> Result<Node, RegexError> {
        let negate = self.eat('^');
        let mut ranges: Ranges = Vec::new();
        let push = |ranges: &mut Ranges, a: &ClassAtom| match a {
            ClassAtom::Char(c) => ranges.push((*c, *c)),
            ClassAtom::Set(s) => ranges.extend_from_slice(s),
        };
        loop {
            match self.peek() {
                None => return err("Unterminated character class"),
                Some(c) if c == ch(']') => {
                    self.pos += 1;
                    break;
                }
                _ => {}
            }
            let lo = self.parse_class_atom()?;
            // Range?
            if self.is('-') && self.peek_at(1).is_some() && !self.is_at(1, ']') {
                self.pos += 1;
                let hi = self.parse_class_atom()?;
                match (&lo, &hi) {
                    (ClassAtom::Char(a), ClassAtom::Char(b)) => {
                        if a > b {
                            return err("Range out of order in character class");
                        }
                        ranges.push((*a, *b));
                    }
                    _ => {
                        if self.unicode {
                            return err("Invalid character class");
                        }
                        // Annex B: treat as the atoms plus a literal '-'.
                        push(&mut ranges, &lo);
                        ranges.push((ch('-'), ch('-')));
                        push(&mut ranges, &hi);
                    }
                }
                continue;
            }
            push(&mut ranges, &lo);
        }
        Ok(self.set_node(normalize(ranges), negate))
    }

    fn parse_class_atom(&mut self) -> Result<ClassAtom, RegexError> {
        let c = self.peek().unwrap();
        self.pos += 1;
        if c != ch('\\') {
            return Ok(ClassAtom::Char(c));
        }
        match self.peek().and_then(char::from_u32) {
            None => return err("\\ at end of pattern"),
            Some('B') => {
                if self.unicode {
                    return err("Invalid escape");
                }
                self.pos += 1;
                return Ok(ClassAtom::Char(ch('B')));
            }
            Some('k') if !self.unicode && !self.named_mode => {
                self.pos += 1;
                return Ok(ClassAtom::Char(ch('k')));
            }
            _ => {}
        }
        if let Some(set) = self.try_class_escape()? {
            return Ok(ClassAtom::Set(set.ranges));
        }
        Ok(ClassAtom::Char(self.parse_character_escape(true)?))
    }

    // ---- `v` mode classes

    /// After `[`: `[^…]` or `[…]` with set operations, through `]`.
    fn parse_class_v(&mut self) -> Result<VSet, RegexError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return err("Regular expression too deeply nested");
        }
        let negate = self.eat('^');
        let mut set = self.parse_class_set_expression()?;
        if negate {
            if set.may_strings {
                return err("Negated character class may contain strings");
            }
            set.ranges = if self.local.ignore_case {
                subtract(&complement(folding_chars()), &set.ranges)
            } else {
                complement(&set.ranges)
            };
        }
        self.depth -= 1;
        Ok(set)
    }

    fn at_double(&self, c: char) -> bool {
        self.is(c) && self.is_at(1, c)
    }

    fn parse_class_set_expression(&mut self) -> Result<VSet, RegexError> {
        if self.eat(']') {
            return Ok(VSet::default());
        }
        let (mut acc, first_is_range) = self.parse_class_set_operand(true)?;
        if self.at_double('&') {
            if first_is_range {
                return err("Invalid set operation in character class");
            }
            while self.at_double('&') {
                self.pos += 2;
                if self.is('&') {
                    return err("Invalid set operation in character class");
                }
                let (op, _) = self.parse_class_set_operand(false)?;
                acc.ranges = intersect(&acc.ranges, &op.ranges);
                acc.strings.retain(|s| op.strings.contains(s));
                acc.may_strings = acc.may_strings && op.may_strings;
            }
            if !self.eat(']') {
                return err("Invalid set operation in character class");
            }
            return Ok(acc);
        }
        if self.at_double('-') {
            if first_is_range {
                return err("Invalid set operation in character class");
            }
            while self.at_double('-') {
                self.pos += 2;
                let (op, _) = self.parse_class_set_operand(false)?;
                acc.ranges = subtract(&acc.ranges, &op.ranges);
                acc.strings.retain(|s| !op.strings.contains(s));
            }
            if !self.eat(']') {
                return err("Invalid set operation in character class");
            }
            return Ok(acc);
        }
        // Union.
        loop {
            match self.peek() {
                None => return err("Unterminated character class"),
                Some(c) if c == ch(']') => {
                    self.pos += 1;
                    return Ok(acc);
                }
                _ => {}
            }
            if self.at_double('&') || self.at_double('-') {
                return err("Invalid set operation in character class");
            }
            let (op, _) = self.parse_class_set_operand(true)?;
            acc.union_with(op);
        }
    }

    /// A ClassSetOperand, or (when `allow_range`) a ClassSetRange. Returns
    /// the set and whether it was a range.
    fn parse_class_set_operand(&mut self, allow_range: bool) -> Result<(VSet, bool), RegexError> {
        let vi = self.local.ignore_case;
        let Some(c) = self.peek() else {
            return err("Unterminated character class");
        };
        if c == ch('[') {
            self.pos += 1;
            return Ok((self.parse_class_v()?, false));
        }
        if c == ch('\\') {
            if self.is_at(1, 'q') && self.is_at(2, '{') {
                self.pos += 3;
                let mut set = self.parse_class_string_disjunction()?;
                if vi {
                    set.ranges = fold_set(&set.ranges);
                    set.strings = std::mem::take(&mut set.strings).into_iter().map(|s| s.into_iter().map(simple_fold).collect::<Vec<u32>>()).collect();
                }
                return Ok((set, false));
            }
            self.pos += 1;
            if let Some(set) = self.try_class_escape()? {
                return Ok((set, false));
            }
            self.pos -= 1;
        }
        let a = self.parse_class_set_character()?;
        if allow_range && self.is('-') && !self.is_at(1, '-') {
            self.pos += 1;
            let b = self.parse_class_set_character()?;
            if a > b {
                return err("Range out of order in character class");
            }
            let r = vec![(a, b)];
            return Ok((VSet::chars(if vi { fold_set(&r) } else { r }), true));
        }
        let r = vec![(a, a)];
        Ok((VSet::chars(if vi { fold_set(&r) } else { r }), false))
    }

    /// A ClassSetCharacter (in `v` classes and `\q{…}`).
    fn parse_class_set_character(&mut self) -> Result<u32, RegexError> {
        let Some(c) = self.peek() else {
            return err("Unterminated character class");
        };
        if c == ch('\\') {
            self.pos += 1;
            let Some(n) = self.peek() else {
                return err("\\ at end of pattern");
            };
            if n == ch('b') {
                self.pos += 1;
                return Ok(0x08);
            }
            if "&-!#%,:;<=>@`~".contains(char::from_u32(n).unwrap_or('\0')) {
                self.pos += 1;
                return Ok(n);
            }
            return self.parse_character_escape(false);
        }
        let cc = char::from_u32(c).unwrap_or('\0');
        if "()[]{}/-|".contains(cc) && cc != '\0' {
            return err("Invalid character in character class");
        }
        if "&!#$%*+,.:;<=>?@^`~".contains(cc) && cc != '\0' && self.peek_at(1) == Some(c) {
            return err("Invalid set operation in character class");
        }
        self.pos += 1;
        Ok(c)
    }

    /// `\q{a|bc|}` after `\q{`, through `}`.
    fn parse_class_string_disjunction(&mut self) -> Result<VSet, RegexError> {
        let mut set = VSet::default();
        let mut cur: Vec<u32> = Vec::new();
        loop {
            match self.peek() {
                None => return err("Unterminated class string disjunction"),
                Some(c) if c == ch('}') || c == ch('|') => {
                    self.pos += 1;
                    let s = std::mem::take(&mut cur);
                    if s.len() == 1 {
                        set.ranges = union(&set.ranges, &[(s[0], s[0])]);
                    } else {
                        set.may_strings = true;
                        set.strings.insert(s);
                    }
                    if c == ch('}') {
                        return Ok(set);
                    }
                }
                _ => cur.push(self.parse_class_set_character()?),
            }
        }
    }

    /// The node for a `v` set: strings (longest first), then the code
    /// points, then the empty string.
    fn vset_node(&self, set: VSet) -> Node {
        let chars = if self.local.ignore_case { case_closure(&set.ranges, true) } else { set.ranges };
        if set.strings.is_empty() {
            return Node::Set(CharClass {
                ranges: chars,
                negate: false,
            });
        }
        let mut strings: Vec<Vec<u32>> = set.strings.into_iter().collect();
        strings.sort_by(|a, b| b.len().cmp(&a.len()));
        let mut alts = Vec::new();
        let mut has_empty = false;
        for s in strings {
            if s.is_empty() {
                has_empty = true;
                continue;
            }
            alts.push(Node::Sequence(s.iter().map(|&c| self.char_node(c)).collect()));
        }
        if !chars.is_empty() {
            alts.push(Node::Set(CharClass {
                ranges: chars,
                negate: false,
            }));
        }
        if has_empty {
            alts.push(Node::Empty);
        }
        if alts.len() == 1 {
            alts.pop().unwrap()
        } else {
            // Non-capturing wrapper so a quantifier applies to the whole set.
            Node::Group {
                index: None,
                body: Box::new(Node::Alternation(alts)),
            }
        }
    }

    fn names_map(&self) -> HashMap<String, Vec<usize>> {
        let mut m: HashMap<String, Vec<usize>> = HashMap::new();
        for (n, i) in &self.group_names {
            m.entry(n.clone()).or_default().push(*i);
        }
        m
    }
}

/// Two groups (given their disjunction paths) can both participate in a
/// match unless they sit in different alternatives of a common disjunction.
fn might_both_participate(a: &[(u32, u32)], b: &[(u32, u32)]) -> bool {
    for (x, y) in a.iter().zip(b) {
        if x.0 != y.0 {
            return true;
        }
        if x.1 != y.1 {
            return false;
        }
    }
    true
}

/// Count capturing groups and detect named groups before parsing.
fn prescan_groups(src: &[u32], vmode: bool) -> (usize, bool) {
    let mut count = 0;
    let mut named = false;
    let mut i = 0;
    let mut class_depth = 0u32;
    while i < src.len() {
        let c = src[i];
        if c == ch('\\') {
            i += 2;
            continue;
        }
        if c == ch('[') {
            if vmode || class_depth == 0 {
                class_depth += 1;
            }
        } else if c == ch(']') {
            class_depth = class_depth.saturating_sub(1);
        } else if c == ch('(') && class_depth == 0 {
            if src.get(i + 1) == Some(&ch('?')) {
                if src.get(i + 2) == Some(&ch('<')) && !matches!(src.get(i + 3), Some(&x) if x == ch('=') || x == ch('!')) {
                    count += 1;
                    named = true;
                }
            } else {
                count += 1;
            }
        }
        i += 1;
    }
    (count, named)
}

// ------------------------------------------------------------- compiler

/// A single-character matcher (body of a fast quantifier loop).
#[derive(Debug, Clone, Copy)]
enum Single {
    Char(u32),
    Char2(u32, u32),
    Any { dot_all: bool },
    Class(usize),
}

#[derive(Debug, Clone)]
enum Inst {
    /// Match one literal (code point in unicode mode, code unit otherwise).
    Char { unit: u32, back: bool },
    /// Match either of two literals (a case pair).
    Char2 { a: u32, b: u32, back: bool },
    /// `.`
    Any { back: bool, dot_all: bool },
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
    /// Backreference to the first participating group of `groups`.
    Backref { groups: Box<[usize]>, back: bool, ignore_case: bool },
    /// Lookaround: the body is `pc+1 .. end` (exclusive), ending in `Match`.
    /// Direction is baked into the body's instructions (`back` flags).
    Look { negate: bool, end: usize },
    /// A forward quantified single-character matcher.
    Star { single: Single, min: u32, max: u32, greedy: bool },
    /// Counted loop: reset counter `reg`.
    CountInit(usize),
    /// Counted loop head: enter the body (`pc+1`) or leave to `exit`.
    CountBranch { reg: usize, min: u32, max: u32, greedy: bool, exit: usize },
    /// Counted loop tail: empty check against `preg`, bump `reg`, jump to `head`.
    CountNext { reg: usize, preg: usize, min: u32, head: usize },
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

    fn reg(&mut self) -> usize {
        self.registers += 1;
        self.registers - 1
    }

    /// The single-character matcher for `node`, if it is one.
    fn single(&mut self, node: &Node) -> Option<Single> {
        Some(match node {
            Node::Group { index: None, body } => return self.single(body),
            Node::Char(c) => Single::Char(*c),
            Node::Any { dot_all } => Single::Any { dot_all: *dot_all },
            Node::Set(cls) => {
                if !cls.negate {
                    match cls.ranges.as_slice() {
                        [(a, b)] if a == b => return Some(Single::Char(*a)),
                        [(a, b)] if *b == *a + 1 => return Some(Single::Char2(*a, *b)),
                        [(a, b), (c, d)] if a == b && c == d => return Some(Single::Char2(*a, *c)),
                        _ => {}
                    }
                }
                let id = self.classes.len();
                self.classes.push(cls.clone());
                Single::Class(id)
            }
            _ => return None,
        })
    }

    fn emit_single(&mut self, s: Single, back: bool) -> Result<(), RegexError> {
        self.emit(match s {
            Single::Char(unit) => Inst::Char { unit, back },
            Single::Char2(a, b) => Inst::Char2 { a, b, back },
            Single::Any { dot_all } => Inst::Any { back, dot_all },
            Single::Class(id) => Inst::Class { id, back },
        })?;
        Ok(())
    }

    fn compile(&mut self, node: &Node, back: bool) -> Result<(), RegexError> {
        match node {
            Node::Empty => Ok(()),
            Node::Char(_) | Node::Any { .. } | Node::Set(_) => {
                let s = self.single(node).unwrap();
                self.emit_single(s, back)
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
                        self.compile(alt, back)?;
                        let j = self.emit(Inst::Jmp(0))?;
                        jumps.push(j);
                        let next = self.program.len();
                        self.program[split] = Inst::Split {
                            first: split + 1,
                            second: next,
                        };
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
            Node::Backref { groups, ignore_case } => {
                self.emit(Inst::Backref {
                    groups: groups.clone().into_boxed_slice(),
                    back,
                    ignore_case: *ignore_case,
                })?;
                Ok(())
            }
            Node::NamedBackref { .. } => unreachable!("resolved before compile"),
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
        if !back {
            if let Some(single) = self.single(body) {
                self.emit(Inst::Star {
                    single,
                    min,
                    max: max.unwrap_or(u32::MAX),
                    greedy,
                })?;
                return Ok(());
            }
        }
        let small = match max {
            None => min <= 3,
            Some(m) => m <= 8,
        };
        if !small {
            return self.compile_counted(body, min, max.unwrap_or(u32::MAX), greedy, inner, back);
        }
        // Mandatory copies.
        for _ in 0..min {
            self.compile_body_iteration(body, inner, back)?;
        }
        match max {
            None => {
                // Star loop with empty-iteration protection.
                let reg = self.reg();
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
                    let reg = self.reg();
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

    /// A loop with a counter register (large or unbounded counts).
    fn compile_counted(
        &mut self,
        body: &Node,
        min: u32,
        max: u32,
        greedy: bool,
        inner: (usize, usize),
        back: bool,
    ) -> Result<(), RegexError> {
        let reg = self.reg();
        let preg = self.reg();
        self.emit(Inst::CountInit(reg))?;
        let head = self.emit(Inst::CountBranch {
            reg,
            min,
            max,
            greedy,
            exit: 0,
        })?;
        self.emit(Inst::SetReg(preg))?;
        self.compile_body_iteration(body, inner, back)?;
        self.emit(Inst::CountNext { reg, preg, min, head })?;
        let exit = self.program.len();
        if let Inst::CountBranch { exit: e, .. } = &mut self.program[head] {
            *e = exit;
        }
        Ok(())
    }
}

// --------------------------------------------------------------- machine

enum Backtrack {
    Alt { pc: usize, pos: usize },
    RestoreCap { slot: usize, old: Option<usize> },
    RestoreReg { reg: usize, old: usize },
    /// Greedy single-character loop: retry the continuation `pc` one
    /// character shorter (never below `min_pos`).
    StarGreedy { pc: usize, min_pos: usize, pos: usize },
    /// Lazy single-character loop at `pc` (the `Star`): retry with one
    /// more character, `count` taken so far.
    StarLazy { pc: usize, pos: usize, count: u32 },
}

struct Machine<'a> {
    regex: &'a Regex,
    subject: &'a [u16],
    unicode: bool,
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
    #[inline]
    fn read(&self, pos: usize, back: bool) -> Option<(u32, usize)> {
        let s = self.subject;
        if back {
            if pos == 0 {
                return None;
            }
            let u = s[pos - 1];
            if self.unicode && is_low(u) && pos >= 2 && is_high(s[pos - 2]) {
                let cp = 0x10000 + (((s[pos - 2] as u32) - 0xD800) << 10) + ((u as u32) - 0xDC00);
                return Some((cp, pos - 2));
            }
            Some((u as u32, pos - 1))
        } else {
            if pos >= s.len() {
                return None;
            }
            let u = s[pos];
            if self.unicode && is_high(u) && pos + 1 < s.len() && is_low(s[pos + 1]) {
                let cp = 0x10000 + (((u as u32) - 0xD800) << 10) + ((s[pos + 1] as u32) - 0xDC00);
                return Some((cp, pos + 2));
            }
            Some((u as u32, pos + 1))
        }
    }

    #[inline]
    fn single_at(&self, single: &Single, pos: usize) -> Option<usize> {
        let (c, np) = self.read(pos, false)?;
        let ok = match single {
            Single::Char(u) => c == *u,
            Single::Char2(a, b) => c == *a || c == *b,
            Single::Any { dot_all } => *dot_all || !is_line_terminator(c),
            Single::Class(id) => self.regex.classes[*id].matches(c),
        };
        if ok {
            Some(np)
        } else {
            None
        }
    }

    /// One character back from `pos`, not below `floor`.
    fn step_back(&self, pos: usize, floor: usize) -> usize {
        let s = self.subject;
        if self.unicode && pos >= floor + 2 && is_low(s[pos - 1]) && is_high(s[pos - 2]) {
            pos - 2
        } else {
            pos - 1
        }
    }

    fn set_cap(&mut self, slot: usize, value: Option<usize>) {
        let old = self.caps[slot];
        self.stack.push(Backtrack::RestoreCap { slot, old });
        self.caps[slot] = value;
    }

    fn set_reg(&mut self, reg: usize, value: usize) {
        let old = self.regs[reg];
        self.stack.push(Backtrack::RestoreReg { reg, old });
        self.regs[reg] = value;
    }

    /// Execute from `pc` at `pos`; `base` is the backtrack stack depth this
    /// run owns (entries below it belong to an enclosing run). Returns the
    /// end position on success. On failure the stack is unwound to `base`
    /// and captures/registers are restored.
    fn run(&mut self, mut pc: usize, mut pos: usize, base: usize) -> Option<usize> {
        let regex = self.regex;
        let prog = &regex.program;
        let mut steps: u32 = 0;
        loop {
            steps = steps.wrapping_add(1);
            if steps & 0xffff == 0 && deadline_passed() {
                self.unwind_to(base);
                return None;
            }
            let ok = match &prog[pc] {
                Inst::Match => return Some(pos),
                Inst::Char { unit, back } => match self.read(pos, *back) {
                    Some((c, np)) if c == *unit => {
                        pos = np;
                        pc += 1;
                        true
                    }
                    _ => false,
                },
                Inst::Char2 { a, b, back } => match self.read(pos, *back) {
                    Some((c, np)) if c == *a || c == *b => {
                        pos = np;
                        pc += 1;
                        true
                    }
                    _ => false,
                },
                Inst::Any { back, dot_all } => match self.read(pos, *back) {
                    Some((c, np)) if *dot_all || !is_line_terminator(c) => {
                        pos = np;
                        pc += 1;
                        true
                    }
                    _ => false,
                },
                Inst::Class { id, back } => match self.read(pos, *back) {
                    Some((c, np)) if regex.classes[*id].matches(c) => {
                        pos = np;
                        pc += 1;
                        true
                    }
                    _ => false,
                },
                Inst::Star { single, min, max, greedy } => {
                    let (min, max) = (*min, *max);
                    let mut count = 0u32;
                    let mut p = pos;
                    let mut ok = true;
                    while count < min {
                        match self.single_at(single, p) {
                            Some(np) => {
                                p = np;
                                count += 1;
                            }
                            None => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok {
                        if *greedy {
                            let min_pos = p;
                            while count < max {
                                match self.single_at(single, p) {
                                    Some(np) => {
                                        p = np;
                                        count += 1;
                                    }
                                    None => break,
                                }
                            }
                            if p > min_pos {
                                self.stack.push(Backtrack::StarGreedy { pc: pc + 1, min_pos, pos: p });
                            }
                        } else if count < max {
                            self.stack.push(Backtrack::StarLazy { pc, pos: p, count });
                        }
                        pos = p;
                        pc += 1;
                    }
                    ok
                }
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
                    self.set_reg(*reg, pos);
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
                Inst::CountInit(reg) => {
                    self.set_reg(*reg, 0);
                    pc += 1;
                    true
                }
                Inst::CountBranch { reg, min, max, greedy, exit } => {
                    let k = self.regs[*reg];
                    if k < *min as usize {
                        pc += 1;
                    } else if k >= *max as usize {
                        pc = *exit;
                    } else if *greedy {
                        self.stack.push(Backtrack::Alt { pc: *exit, pos });
                        pc += 1;
                    } else {
                        self.stack.push(Backtrack::Alt { pc: pc + 1, pos });
                        pc = *exit;
                    }
                    true
                }
                Inst::CountNext { reg, preg, min, head } => {
                    let k = self.regs[*reg];
                    if k >= *min as usize && self.regs[*preg] == pos {
                        false
                    } else {
                        self.set_reg(*reg, k + 1);
                        pc = *head;
                        true
                    }
                }
                Inst::Assert(kind) => {
                    let s = self.subject;
                    let ok = match kind {
                        AssertKind::Start => pos == 0,
                        AssertKind::End => pos == s.len(),
                        AssertKind::LineStart => pos == 0 || is_line_terminator(s[pos - 1] as u32),
                        AssertKind::LineEnd => pos == s.len() || is_line_terminator(s[pos] as u32),
                        AssertKind::WordBoundary { extended } | AssertKind::NotWordBoundary { extended } => {
                            let before = pos > 0 && is_word_at(s[pos - 1], *extended);
                            let after = pos < s.len() && is_word_at(s[pos], *extended);
                            (before != after) == matches!(kind, AssertKind::WordBoundary { .. })
                        }
                    };
                    if ok {
                        pc += 1;
                    }
                    ok
                }
                Inst::Backref { groups, back, ignore_case } => {
                    let found = groups.iter().find_map(|&g| match (self.caps[g * 2], self.caps[g * 2 + 1]) {
                        (Some(s), Some(e)) if s <= e => Some((s, e)),
                        _ => None,
                    });
                    match found {
                        Some((s, e)) => match self.backref_match(s, e, pos, *back, *ignore_case) {
                            Some(np) => {
                                pos = np;
                                pc += 1;
                                true
                            }
                            None => false,
                        },
                        // Unset group: matches empty.
                        None => {
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
                            self.stack
                                .retain_from(sub_base, |b| matches!(b, Backtrack::RestoreCap { .. } | Backtrack::RestoreReg { .. }));
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
                        Backtrack::StarGreedy { pc: npc, min_pos, pos: p } => {
                            let np = self.step_back(p, min_pos);
                            if np > min_pos {
                                self.stack.push(Backtrack::StarGreedy { pc: npc, min_pos, pos: np });
                            }
                            pc = npc;
                            pos = np;
                            break;
                        }
                        Backtrack::StarLazy { pc: spc, pos: p, count } => {
                            let Inst::Star { single, max, .. } = &prog[spc] else { unreachable!() };
                            if let Some(np) = self.single_at(single, p) {
                                if count + 1 < *max {
                                    self.stack.push(Backtrack::StarLazy {
                                        pc: spc,
                                        pos: np,
                                        count: count + 1,
                                    });
                                }
                                pc = spc + 1;
                                pos = np;
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    fn unwind_to(&mut self, base: usize) {
        while self.stack.len() > base {
            match self.stack.pop().unwrap() {
                Backtrack::RestoreCap { slot, old } => self.caps[slot] = old,
                Backtrack::RestoreReg { reg, old } => self.regs[reg] = old,
                _ => {}
            }
        }
    }

    /// Match the text of capture `s..e` at `pos` (ending at `pos` when
    /// `back`); returns the new position.
    fn backref_match(&self, s: usize, e: usize, pos: usize, back: bool, ignore_case: bool) -> Option<usize> {
        let sub = self.subject;
        let len = e - s;
        let start = if back {
            pos.checked_sub(len)?
        } else {
            if pos + len > sub.len() {
                return None;
            }
            pos
        };
        if !ignore_case {
            if sub[s..e] != sub[start..start + len] {
                return None;
            }
        } else if self.unicode {
            let (mut ai, mut bi) = (s, start);
            while ai < e {
                let (ca, na) = self.read(ai, false)?;
                let (cb, nb) = self.read(bi, false)?;
                if na - ai != nb - bi || simple_fold(ca) != simple_fold(cb) {
                    return None;
                }
                ai = na;
                bi = nb;
            }
        } else {
            for i in 0..len {
                if canonicalize(sub[s + i] as u32, false) != canonicalize(sub[start + i] as u32, false) {
                    return None;
                }
            }
        }
        Some(if back { start } else { pos + len })
    }
}

fn is_word_at(u: u16, extended: bool) -> bool {
    let u = u as u32;
    is_word_unit(u) || (extended && (u == 0x17F || u == 0x212A))
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
    fn resolve_names(&mut self, names: &HashMap<String, Vec<usize>>) {
        match self {
            Node::NamedBackref { name, ignore_case } => {
                let groups = names.get(name.as_str()).cloned().unwrap_or_default();
                *self = Node::Backref {
                    groups,
                    ignore_case: *ignore_case,
                };
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
        // Non-unicode: the quantifier applies to the trailing surrogate only.
        assert_eq!(whole("😀+", "", "😀😀").as_deref(), Some("😀"));
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
        assert_eq!(error("a{", "u"), "Incomplete quantifier");
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

    #[test]
    fn property_escapes() {
        assert_eq!(whole("\\p{Script=Latin}+", "u", "αabcβ").as_deref(), Some("abc"));
        assert_eq!(whole("\\p{sc=Grek}+", "u", "abc αβγ").as_deref(), Some("αβγ"));
        assert_eq!(whole("\\p{scx=Hira}+", "u", "x\u{30FC}あy").as_deref(), Some("\u{30FC}あ"));
        assert_eq!(whole("\\p{Lu}", "u", "aÉb").as_deref(), Some("É"));
        assert_eq!(whole("\\p{RI}+", "u", "x🇪🇸y").as_deref(), Some("🇪🇸"));
        assert!(Regex::new("\\p{ASCII_Hex_Digit}", "u").is_ok());
        for bad in ["\\p{ascii}", "\\p{Script=latin}", "\\p{General_Category}", "\\p{Block=Basic_Latin}", "\\p{Lu=Lu}", "\\p{ L}", "\\p", "\\p{RGI_Emoji}"] {
            assert!(Regex::new(bad, "u").is_err(), "{bad}");
        }
        assert!(Regex::new("\\p{RGI_Emoji}", "v").is_ok());
        assert!(Regex::new("\\P{RGI_Emoji}", "v").is_err());
        assert_eq!(whole("^\\p{RGI_Emoji}$", "v", "👨‍👩‍👧").as_deref(), Some("👨‍👩‍👧"));
        assert_eq!(whole("^\\p{Emoji_Keycap_Sequence}$", "v", "1\u{FE0F}\u{20E3}").as_deref(), Some("1\u{FE0F}\u{20E3}"));
    }

    #[test]
    fn unicode_sets() {
        assert_eq!(whole("[\\d--[0-4]]+", "v", "0156").as_deref(), Some("56"));
        assert_eq!(whole("[\\p{ASCII_Hex_Digit}&&\\p{Lowercase}]+", "v", "xAbcdz").as_deref(), Some("bcd"));
        assert_eq!(whole("[\\q{abc|d}x]+", "v", "-abcxdab").as_deref(), Some("abcxd"));
        assert_eq!(whole("[[a-z]--[aeiou]]+", "v", "aebcdi").as_deref(), Some("bcd"));
        assert!(Regex::new("[^\\q{ab}]", "v").is_err());
        assert!(Regex::new("[^\\q{a}]", "v").is_ok());
        assert!(Regex::new("[a&&&b]", "v").is_err());
        assert!(Regex::new("[a-z&&b]", "v").is_err());
        assert!(Regex::new("[(]", "v").is_err());
        assert!(Regex::new("[a--]", "v").is_err());
        assert!(Regex::new("[!!]", "v").is_err());
        assert!(Regex::new("[\\!]", "v").is_ok());
        assert!(Regex::new("a", "uv").is_err());
        // u vs v: complement of a case-folded set.
        assert_eq!(whole("\\P{Lowercase}", "ui", "a").as_deref(), Some("a"));
        assert_eq!(whole("\\P{Lowercase}", "vi", "a"), None);
        assert_eq!(whole("[^\\P{Lowercase}]", "vi", "A").as_deref(), Some("A"));
    }

    #[test]
    fn modifiers() {
        assert_eq!(whole("(?i:a)b", "", "Ab").as_deref(), Some("Ab"));
        assert_eq!(whole("(?i:a)b", "", "AB"), None);
        assert_eq!(whole("(?-i:a)b", "i", "aB").as_deref(), Some("aB"));
        assert_eq!(whole("(?-i:a)b", "i", "AB"), None);
        assert_eq!(whole("(?m:^b)", "", "a\nb").as_deref(), Some("b"));
        assert_eq!(whole("(?s:.)", "", "\n").as_deref(), Some("\n"));
        assert_eq!(whole("(?i:\\b)", "u", "\u{17F}"), Some("".into()));
        for bad in ["(?ii:a)", "(?i-i:a)", "(?-:a)", "(?x:a)", "(?i)", "(?i-mm:a)"] {
            assert!(Regex::new(bad, "").is_err(), "{bad}");
        }
    }

    #[test]
    fn duplicate_named_groups() {
        let re = Regex::new("(?<a>x)|(?<a>y)", "").unwrap();
        assert_eq!(re.named_groups(), &[("a".to_string(), vec![1, 2])]);
        assert_eq!(whole("(?:(?<a>x)|(?<a>y))\\k<a>", "", "yy").as_deref(), Some("yy"));
        assert!(Regex::new("(?<a>x)(?<a>y)", "").is_err());
        assert!(Regex::new("(?:(?<a>x)|(?<a>y))(?<a>z)", "").is_err());
        assert!(Regex::new("(?<a>x)|(?:(?<a>y)|(?<a>z))", "").is_ok());
        assert!(Regex::new("(?<a>x)|(?<b>(?<a>y))", "").is_ok());
    }

    #[test]
    fn counted_loops() {
        assert_eq!(whole("(?:ab){10}", "", &"ab".repeat(12)).map(|s| s.len()), Some(20));
        assert_eq!(whole("(?:ab){10,}", "", &"ab".repeat(12)).map(|s| s.len()), Some(24));
        assert_eq!(whole("(?:ab){3,10}?c", "", &format!("{}c", "ab".repeat(5))).map(|s| s.len()), Some(11));
        assert_eq!(whole("(?:a|b){20}", "", &"ab".repeat(5)), None);
        assert!(Regex::new("a{9007199254740991}", "").is_ok());
        assert!(Regex::new("(?:a|b){0,9007199254740991}", "").is_ok());
        assert_eq!(whole("(a*){20,}b", "", "aab").as_deref(), Some("aab"));
        assert_eq!(whole("a{2,3}?", "", "aaaa").as_deref(), Some("aa"));
        assert_eq!(whole("^(?:a+){2,}$", "", "aaaa").as_deref(), Some("aaaa"));
    }
}
