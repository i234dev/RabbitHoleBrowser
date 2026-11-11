//! Core value types: UTF-16 strings, symbols, object ids and `Value`.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

/// An immutable JavaScript string stored as UTF-16 code units.
///
/// Concatenation of long strings builds a rope (no copying); the flat
/// representation is produced lazily the first time the code units are
/// needed and then cached, so `s += "x"` loops stay linear.
#[derive(Clone)]
pub struct JsStr(Rc<StrInner>);

enum StrInner {
    Flat(Box<[u16]>),
    Rope { left: JsStr, right: JsStr, len: usize, flat: std::cell::OnceCell<Box<[u16]>> },
}

impl Drop for StrInner {
    fn drop(&mut self) {
        // Unlink deep ropes iteratively to avoid recursive drops. Only
        // children that are ropes themselves need it; the placeholder left
        // behind is created here (not taken from a thread-local: strings
        // can be dropped while thread-local storage is being torn down).
        let StrInner::Rope { left, right, .. } = self else { return };
        let is_rope = |s: &JsStr| matches!(&*s.0, StrInner::Rope { .. });
        if !is_rope(left) && !is_rope(right) {
            return;
        }
        let hole = JsStr(Rc::new(StrInner::Flat(Box::default())));
        let mut stack = vec![std::mem::replace(left, hole.clone()), std::mem::replace(right, hole.clone())];
        while let Some(s) = stack.pop() {
            if let Ok(mut inner) = Rc::try_unwrap(s.0) {
                if let StrInner::Rope { left, right, .. } = &mut inner {
                    stack.push(std::mem::replace(left, hole.clone()));
                    stack.push(std::mem::replace(right, hole.clone()));
                }
            }
        }
    }
}

const ROPE_MIN: usize = 48;

impl JsStr {
    pub fn new(s: &str) -> JsStr {
        JsStr(Rc::new(StrInner::Flat(s.encode_utf16().collect::<Vec<u16>>().into())))
    }
    pub fn from_units(units: Vec<u16>) -> JsStr {
        JsStr(Rc::new(StrInner::Flat(units.into())))
    }
    pub fn from_slice(units: &[u16]) -> JsStr {
        JsStr(Rc::new(StrInner::Flat(units.into())))
    }
    /// One-code-unit string (Latin-1 range cached).
    pub fn from_unit(u: u16) -> JsStr {
        thread_local! {
            static UNITS: Vec<JsStr> = (0..256u16).map(|u| JsStr::from_units(vec![u])).collect();
        }
        if u < 256 {
            return UNITS.with(|t| t[u as usize].clone());
        }
        JsStr::from_units(vec![u])
    }
    /// The code unit at `i` as a string (`s[i]`, `s.charAt(i)`).
    pub fn char_at(&self, i: usize) -> JsStr {
        JsStr::from_unit(self.as_units()[i])
    }
    /// Decimal string of an array index (small ones are cached).
    pub fn from_index(i: u32) -> JsStr {
        thread_local! {
            static SMALL: Vec<JsStr> = (0..1024u32).map(|i| JsStr::new(&i.to_string())).collect();
            // Larger indices are cached lazily (bounded).
            static LARGE: std::cell::RefCell<Vec<Option<JsStr>>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        if i < 1024 {
            return SMALL.with(|t| t[i as usize].clone());
        }
        if i < (1 << 16) {
            return LARGE.with(|t| {
                let mut t = t.borrow_mut();
                if t.len() <= i as usize {
                    t.resize(i as usize + 1, None);
                }
                t[i as usize].get_or_insert_with(|| JsStr::from_units(Self::index_units(i))).clone()
            });
        }
        JsStr::from_units(Self::index_units(i))
    }
    fn index_units(mut i: u32) -> Vec<u16> {
        let mut buf = [0u16; 10];
        let mut n = 0;
        loop {
            buf[9 - n] = b'0' as u16 + (i % 10) as u16;
            n += 1;
            i /= 10;
            if i == 0 {
                break;
            }
        }
        buf[10 - n..].to_vec()
    }
    pub fn empty() -> JsStr {
        thread_local! {
            static EMPTY: JsStr = JsStr(Rc::new(StrInner::Flat(Vec::new().into())));
        }
        EMPTY.with(|e| e.clone())
    }
    /// The code units (flattening a rope on first use).
    pub fn as_units(&self) -> &[u16] {
        match &*self.0 {
            StrInner::Flat(b) => b,
            StrInner::Rope { flat, .. } => flat.get_or_init(|| self.flatten()),
        }
    }
    fn flatten(&self) -> Box<[u16]> {
        let mut out: Vec<u16> = Vec::with_capacity(self.len());
        // Iterative in-order traversal (ropes can be very deep).
        let mut stack: Vec<&JsStr> = vec![self];
        while let Some(s) = stack.pop() {
            match &*s.0 {
                StrInner::Flat(b) => out.extend_from_slice(b),
                StrInner::Rope { left, right, flat, .. } => {
                    if let Some(f) = flat.get() {
                        out.extend_from_slice(f);
                    } else {
                        stack.push(right);
                        stack.push(left);
                    }
                }
            }
        }
        out.into()
    }
    pub fn len(&self) -> usize {
        match &*self.0 {
            StrInner::Flat(b) => b.len(),
            StrInner::Rope { len, .. } => *len,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Lossy conversion (lone surrogates become U+FFFD).
    pub fn to_rust_string(&self) -> String {
        String::from_utf16_lossy(self.as_units())
    }
    pub fn concat(&self, other: &JsStr) -> JsStr {
        if other.is_empty() {
            return self.clone();
        }
        if self.is_empty() {
            return other.clone();
        }
        let len = self.len() + other.len();
        if len < ROPE_MIN {
            let mut v = Vec::with_capacity(len);
            v.extend_from_slice(self.as_units());
            v.extend_from_slice(other.as_units());
            return JsStr(Rc::new(StrInner::Flat(v.into())));
        }
        JsStr(Rc::new(StrInner::Rope { left: self.clone(), right: other.clone(), len, flat: std::cell::OnceCell::new() }))
    }
    pub fn slice(&self, start: usize, end: usize) -> JsStr {
        let end = end.min(self.len());
        let start = start.min(end);
        if start == 0 && end == self.len() {
            return self.clone();
        }
        JsStr::from_slice(&self.as_units()[start..end])
    }
    pub fn starts_with(&self, other: &JsStr) -> bool {
        self.as_units().starts_with(other.as_units())
    }
    pub fn ends_with(&self, other: &JsStr) -> bool {
        self.as_units().ends_with(other.as_units())
    }
    /// Index of `needle` at or after `from`, in code units.
    pub fn index_of(&self, needle: &JsStr, from: usize) -> Option<usize> {
        let h = self.as_units();
        let n = needle.as_units();
        if n.is_empty() {
            return if from <= h.len() { Some(from) } else { None };
        }
        if n.len() > h.len() {
            return None;
        }
        (from..=h.len() - n.len()).find(|&i| h[i..i + n.len()] == n[..])
    }
    pub fn last_index_of(&self, needle: &JsStr, from: usize) -> Option<usize> {
        let h = self.as_units();
        let n = needle.as_units();
        if n.len() > h.len() {
            return None;
        }
        let max = (h.len() - n.len()).min(from);
        (0..=max).rev().find(|&i| h[i..i + n.len()] == n[..])
    }
    pub fn eq_str(&self, s: &str) -> bool {
        let mut it = self.as_units().iter();
        for u in s.encode_utf16() {
            if it.next() != Some(&u) {
                return false;
            }
        }
        it.next().is_none()
    }
    /// Canonical numeric array index (`"0"`, `"42"`, but not `"01"` or `"4294967295"`).
    pub fn as_array_index(&self) -> Option<u32> {
        let u = self.as_units();
        if u.is_empty() || u.len() > 10 {
            return None;
        }
        if u.len() > 1 && u[0] == b'0' as u16 {
            return None;
        }
        let mut n: u64 = 0;
        for &c in u.iter() {
            if !(b'0' as u16..=b'9' as u16).contains(&c) {
                return None;
            }
            n = n * 10 + (c - b'0' as u16) as u64;
        }
        if n < u32::MAX as u64 {
            Some(n as u32)
        } else {
            None
        }
    }
    /// Upper-case using Rust's Unicode tables.
    pub fn to_upper(&self) -> JsStr {
        JsStr::new(&self.to_rust_string().to_uppercase())
    }
    pub fn to_lower(&self) -> JsStr {
        JsStr::new(&self.to_rust_string().to_lowercase())
    }
    pub fn chars_lossy(&self) -> std::char::DecodeUtf16<std::iter::Copied<std::slice::Iter<'_, u16>>> {
        char::decode_utf16(self.as_units().iter().copied())
    }
    pub fn trim_js(&self, start: bool, end: bool) -> JsStr {
        let u = self.as_units();
        let mut s = 0;
        let mut e = u.len();
        if start {
            while s < e && is_js_whitespace_unit(u[s]) {
                s += 1;
            }
        }
        if end {
            while e > s && is_js_whitespace_unit(u[e - 1]) {
                e -= 1;
            }
        }
        self.slice(s, e)
    }
    pub fn repeat(&self, n: usize) -> JsStr {
        let u = self.as_units();
        let mut v = Vec::with_capacity(u.len() * n);
        for _ in 0..n {
            v.extend_from_slice(u);
        }
        JsStr::from_units(v)
    }
    /// Lexicographic comparison by code units (as JS `<` on strings).
    pub fn cmp_units(&self, other: &JsStr) -> std::cmp::Ordering {
        self.as_units().cmp(other.as_units())
    }
    pub fn ptr_eq(&self, other: &JsStr) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// Frequently used property names, interned once per thread (avoids an
/// interner lookup on hot allocation paths).
pub mod names {
    use super::{JsStr, PropertyKey};
    macro_rules! name {
        ($f:ident, $s:expr) => {
            #[inline]
            pub fn $f() -> PropertyKey {
                thread_local!(static S: JsStr = JsStr::intern($s));
                PropertyKey::String(S.with(|s| s.clone()))
            }
        };
    }
    name!(length, "length");
    name!(name, "name");
    name!(callee, "callee");
    name!(prototype, "prototype");
    name!(constructor, "constructor");
    name!(value, "value");
    name!(done, "done");
}

/// JS WhiteSpace + LineTerminator code units.
pub fn is_js_whitespace_unit(c: u16) -> bool {
    matches!(
        c,
        0x09 | 0x0a | 0x0b | 0x0c | 0x0d | 0x20 | 0xa0 | 0x1680 | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000 | 0xfeff
    )
}

impl PartialEq for JsStr {
    fn eq(&self, other: &JsStr) -> bool {
        Rc::ptr_eq(&self.0, &other.0) || (self.len() == other.len() && self.as_units() == other.as_units())
    }
}
impl Eq for JsStr {}
impl Hash for JsStr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_units().hash(state)
    }
}

/// Small fast non-cryptographic hasher (multiply-rotate, as used by many
/// compilers) for engine-internal maps keyed by property names.
#[derive(Default, Clone, Copy)]
pub struct FxHasher {
    hash: u64,
}

const FX_SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl FxHasher {
    #[inline]
    fn add(&mut self, w: u64) {
        self.hash = (self.hash.rotate_left(5) ^ w).wrapping_mul(FX_SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            self.add(u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut buf = [0u8; 8];
            buf[..rest.len()].copy_from_slice(rest);
            self.add(u64::from_le_bytes(buf));
        }
    }
    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(i as u64);
    }
    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add(i as u64);
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(i as u64);
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

pub type FxBuild = std::hash::BuildHasherDefault<FxHasher>;

thread_local! {
    static INTERNED: std::cell::RefCell<std::collections::HashMap<Box<str>, JsStr, FxBuild>> = std::cell::RefCell::new(std::collections::HashMap::default());
}

impl JsStr {
    /// A shared instance for short names (property keys, identifiers), so
    /// that equal names compare by pointer in property lookups.
    pub fn intern(s: &str) -> JsStr {
        if s.len() > 32 {
            return JsStr::new(s);
        }
        INTERNED.with(|m| {
            if let Some(j) = m.borrow().get(s) {
                return j.clone();
            }
            let j = JsStr::new(s);
            m.borrow_mut().insert(s.into(), j.clone());
            j
        })
    }
}
impl fmt::Debug for JsStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.to_rust_string())
    }
}
impl fmt::Display for JsStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_rust_string())
    }
}
impl From<&str> for JsStr {
    fn from(s: &str) -> JsStr {
        JsStr::new(s)
    }
}
impl From<String> for JsStr {
    fn from(s: String) -> JsStr {
        JsStr::new(&s)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct SymbolId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct ObjectId(pub u32);

pub use crate::bigint::BigInt;

/// A JavaScript value.
///
/// `repr(u64)` gives every variant a full-word tag followed by an 8-byte
/// payload, so values move as two machine words: the interpreter's operand
/// stack then never mixes partial-width stores with full-width loads (which
/// defeats store-to-load forwarding and costs ~70% in the dispatch loop).
#[derive(Debug)]
#[repr(u64)]
pub enum Value {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(JsStr),
    Symbol(SymbolId),
    BigInt(Rc<BigInt>),
    Object(ObjectId),
    /// Internal marker used for array holes and uninitialized (TDZ)
    /// bindings. It never escapes to JavaScript code.
    Empty,
}

impl Clone for Value {
    #[inline(always)]
    fn clone(&self) -> Value {
        match self {
            Value::String(s) => Value::String(s.clone()),
            Value::BigInt(b) => Value::BigInt(b.clone()),
            // SAFETY: every other variant is plain data without a destructor,
            // so a bitwise copy is a valid clone (and compiles to a 16-byte
            // move instead of a per-variant reconstruction).
            _ => unsafe { std::ptr::read(self) },
        }
    }
}

impl Value {
    pub fn is_undefined(&self) -> bool {
        matches!(self, Value::Undefined)
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
    pub fn is_nullish(&self) -> bool {
        matches!(self, Value::Undefined | Value::Null)
    }
    pub fn is_object(&self) -> bool {
        matches!(self, Value::Object(_))
    }
    pub fn is_empty(&self) -> bool {
        matches!(self, Value::Empty)
    }
    pub fn as_object(&self) -> Option<ObjectId> {
        match self {
            Value::Object(o) => Some(*o),
            _ => None,
        }
    }
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&JsStr> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }
    pub fn str(s: &str) -> Value {
        Value::String(JsStr::new(s))
    }
    pub fn num(n: f64) -> Value {
        Value::Number(n)
    }
    pub fn type_of(&self) -> &'static str {
        match self {
            Value::Undefined | Value::Empty => "undefined",
            Value::Null => "object",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Symbol(_) => "symbol",
            Value::BigInt(_) => "bigint",
            Value::Object(_) => "object",
        }
    }
    /// ToBoolean (no side effects).
    pub fn truthy(&self) -> bool {
        match self {
            Value::Undefined | Value::Null | Value::Empty => false,
            Value::Bool(b) => *b,
            Value::Number(n) => !(*n == 0.0 || n.is_nan()),
            Value::String(s) => !s.is_empty(),
            Value::Symbol(_) => true,
            Value::BigInt(b) => !b.is_zero(),
            Value::Object(_) => true,
        }
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Value {
        Value::Bool(b)
    }
}
impl From<f64> for Value {
    fn from(n: f64) -> Value {
        Value::Number(n)
    }
}
impl From<i32> for Value {
    fn from(n: i32) -> Value {
        Value::Number(n as f64)
    }
}
impl From<u32> for Value {
    fn from(n: u32) -> Value {
        Value::Number(n as f64)
    }
}
impl From<usize> for Value {
    fn from(n: usize) -> Value {
        Value::Number(n as f64)
    }
}
impl From<&str> for Value {
    fn from(s: &str) -> Value {
        Value::String(JsStr::new(s))
    }
}
impl From<String> for Value {
    fn from(s: String) -> Value {
        Value::String(JsStr::new(&s))
    }
}
impl From<JsStr> for Value {
    fn from(s: JsStr) -> Value {
        Value::String(s)
    }
}
impl From<ObjectId> for Value {
    fn from(o: ObjectId) -> Value {
        Value::Object(o)
    }
}

/// A property key: a string or a symbol.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum PropertyKey {
    String(JsStr),
    Symbol(SymbolId),
}

impl PropertyKey {
    pub fn str(s: &str) -> PropertyKey {
        PropertyKey::String(JsStr::intern(s))
    }
    pub fn index(i: u32) -> PropertyKey {
        PropertyKey::String(JsStr::from_index(i))
    }
    pub fn as_array_index(&self) -> Option<u32> {
        match self {
            PropertyKey::String(s) => s.as_array_index(),
            PropertyKey::Symbol(_) => None,
        }
    }
    pub fn as_str(&self) -> Option<&JsStr> {
        match self {
            PropertyKey::String(s) => Some(s),
            _ => None,
        }
    }
    pub fn is_symbol(&self) -> bool {
        matches!(self, PropertyKey::Symbol(_))
    }
    pub fn to_value(&self) -> Value {
        match self {
            PropertyKey::String(s) => Value::String(s.clone()),
            PropertyKey::Symbol(s) => Value::Symbol(*s),
        }
    }
}

impl From<&str> for PropertyKey {
    fn from(s: &str) -> PropertyKey {
        PropertyKey::str(s)
    }
}
impl From<JsStr> for PropertyKey {
    fn from(s: JsStr) -> PropertyKey {
        PropertyKey::String(s)
    }
}
impl From<u32> for PropertyKey {
    fn from(i: u32) -> PropertyKey {
        PropertyKey::index(i)
    }
}
impl From<SymbolId> for PropertyKey {
    fn from(s: SymbolId) -> PropertyKey {
        PropertyKey::Symbol(s)
    }
}

/// Number → string per `Number::toString` (radix 10).
pub fn number_to_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x == 0.0 {
        return "0".into();
    }
    // Integers below 2^53 print as plain decimal digits.
    if x.abs() < 9007199254740992.0 && x == (x as i64) as f64 {
        return (x as i64).to_string();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    let neg = x < 0.0;
    let a = x.abs();
    // Shortest round-trip digits via Rust's `{:e}` formatting.
    let e = format!("{:e}", a);
    let (mant, exp) = e.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n = exp + 1;
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if k <= n && n <= 21 {
        out.push_str(&digits);
        for _ in 0..(n - k) {
            out.push('0');
        }
    } else if 0 < n && n <= 21 {
        out.push_str(&digits[..n as usize]);
        out.push('.');
        out.push_str(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        for _ in 0..(-n) {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        let e = n - 1;
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if e >= 0 { '+' } else { '-' });
        out.push_str(&e.abs().to_string());
    }
    out
}

/// Number → string in an arbitrary radix (2..=36).
pub fn number_to_string_radix(x: f64, radix: u32) -> String {
    if radix == 10 {
        return number_to_string(x);
    }
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if x == 0.0 {
        return "0".into();
    }
    let neg = x < 0.0;
    let a = x.abs();
    let int_part = a.trunc();
    let mut frac = a - int_part;
    let mut int_digits = Vec::new();
    let mut n = int_part;
    if n == 0.0 {
        int_digits.push('0');
    }
    while n >= 1.0 {
        let d = (n % radix as f64) as u32;
        int_digits.push(std::char::from_digit(d, radix).unwrap());
        n = (n / radix as f64).trunc();
    }
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    out.extend(int_digits.iter().rev());
    if frac > 0.0 {
        out.push('.');
        // Emit digits until the fraction is exhausted or precision runs out.
        let mut count = 0;
        let mut delta = 0.5 * (f64::EPSILON * a).max(f64::MIN_POSITIVE);
        while frac >= delta && count < 1100 {
            frac *= radix as f64;
            delta *= radix as f64;
            let d = frac.floor() as u32;
            frac -= d as f64;
            out.push(std::char::from_digit(d, radix).unwrap());
            count += 1;
            if frac > 0.5 || (frac == 0.5 && (d & 1) == 1) {
                if frac + delta > 1.0 {
                    // round up and stop
                    let mut chars: Vec<char> = out.chars().collect();
                    let mut i = chars.len() - 1;
                    loop {
                        let c = chars[i];
                        if c == '.' {
                            i -= 1;
                            continue;
                        }
                        let d = c.to_digit(radix).unwrap();
                        if d + 1 < radix {
                            chars[i] = std::char::from_digit(d + 1, radix).unwrap();
                            break;
                        } else {
                            chars[i] = '0';
                            if i == 0 || (i == 1 && neg) {
                                chars.insert(i, '1');
                                break;
                            }
                            i -= 1;
                        }
                    }
                    out = chars.into_iter().collect();
                    break;
                }
            }
        }
        while out.ends_with('0') {
            out.pop();
        }
        if out.ends_with('.') {
            out.pop();
        }
    }
    out
}

/// StringToNumber per spec: whitespace trimming, hex/octal/binary literals,
/// `Infinity`, decimal literals; anything else is NaN.
pub fn string_to_number(s: &JsStr) -> f64 {
    let t = s.trim_js(true, true);
    if t.is_empty() {
        return 0.0;
    }
    let st = t.to_rust_string();
    let b = st.as_bytes();
    if b.len() > 2 && b[0] == b'0' {
        let radix = match b[1] {
            b'x' | b'X' => 16,
            b'o' | b'O' => 8,
            b'b' | b'B' => 2,
            _ => 0,
        };
        if radix != 0 {
            let mut v = 0.0f64;
            for &c in &b[2..] {
                let d = match (c as char).to_digit(radix) {
                    Some(d) => d,
                    None => return f64::NAN,
                };
                v = v * radix as f64 + d as f64;
            }
            return v;
        }
    }
    match st.as_str() {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    if !is_str_decimal_literal(&st) {
        return f64::NAN;
    }
    st.parse::<f64>().unwrap_or(f64::NAN)
}

/// Validate `[+-]digits[.digits][e[+-]digits]` (digits may be empty on one side of the point).
fn is_str_decimal_literal(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i - int_start;
    let mut frac_digits = 0;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let fs = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        frac_digits = i - fs;
    }
    if int_digits == 0 && frac_digits == 0 {
        return false;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let es = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == es {
            return false;
        }
    }
    i == b.len()
}

/// ToInt32.
#[inline]
pub fn to_int32(n: f64) -> i32 {
    // |n| < 2^63: truncation to i64 then wrapping to 32 bits is exactly
    // ToInt32's modulo arithmetic (NaN fails the comparison).
    if n.abs() < 9.2e18 {
        return (n as i64) as i32;
    }
    to_int32_slow(n)
}

#[cold]
fn to_int32_slow(n: f64) -> i32 {
    if !n.is_finite() {
        return 0;
    }
    let m = n.trunc() % 4294967296.0;
    let m = if m < 0.0 { m + 4294967296.0 } else { m };
    (m as u64 as u32) as i32
}

#[inline]
pub fn to_uint32(n: f64) -> u32 {
    to_int32(n) as u32
}

/// ToIntegerOrInfinity.
pub fn to_integer(n: f64) -> f64 {
    if n.is_nan() {
        0.0
    } else if n.is_infinite() {
        n
    } else {
        n.trunc() + 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_formatting() {
        assert_eq!(number_to_string(0.1), "0.1");
        assert_eq!(number_to_string(1e21), "1e+21");
        assert_eq!(number_to_string(123456789012345680000.0), "123456789012345680000");
        assert_eq!(number_to_string(1e-7), "1e-7");
        assert_eq!(number_to_string(0.000001), "0.000001");
        assert_eq!(number_to_string(-1.5), "-1.5");
        assert_eq!(number_to_string(100.0), "100");
        assert_eq!(number_to_string(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(number_to_string_radix(255.0, 16), "ff");
        assert_eq!(number_to_string_radix(0.5, 2), "0.1");
        assert_eq!(number_to_string_radix(-10.0, 2), "-1010");
    }

    #[test]
    fn string_parsing() {
        assert_eq!(string_to_number(&JsStr::new("  12.5 ")), 12.5);
        assert_eq!(string_to_number(&JsStr::new("0x1f")), 31.0);
        assert!(string_to_number(&JsStr::new("abc")).is_nan());
        assert!(string_to_number(&JsStr::new("inf")).is_nan());
        assert_eq!(string_to_number(&JsStr::new("")), 0.0);
        assert_eq!(string_to_number(&JsStr::new("-Infinity")), f64::NEG_INFINITY);
        assert_eq!(string_to_number(&JsStr::new(".5")), 0.5);
        assert_eq!(string_to_number(&JsStr::new("1e3")), 1000.0);
        assert_eq!(to_int32(4294967296.0 + 5.0), 5);
        assert_eq!(to_int32(-1.0), -1);
        assert_eq!(to_uint32(-1.0), u32::MAX);
    }

    #[test]
    fn array_index() {
        assert_eq!(JsStr::new("0").as_array_index(), Some(0));
        assert_eq!(JsStr::new("42").as_array_index(), Some(42));
        assert_eq!(JsStr::new("01").as_array_index(), None);
        assert_eq!(JsStr::new("4294967295").as_array_index(), None);
        assert_eq!(JsStr::new("-1").as_array_index(), None);
    }
}
