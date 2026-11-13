//! URLs per the WHATWG URL Standard: the basic URL parser state machine
//! (with state overrides for the `URL` setters), host parsing (domains
//! with IDNA to ASCII, IPv4 in all its legacy forms, IPv6, opaque hosts),
//! percent-encode sets and serialization.

use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Url {
    /// Lower-cased scheme without ':'
    pub scheme: String,
    pub username: String,
    pub password: String,
    /// Serialized host (`[...]` for IPv6). Empty for an empty or null host
    /// (see `has_host`).
    pub host: String,
    /// The host is non-null (the URL has an authority, `scheme://...`).
    pub has_host: bool,
    pub port: Option<u16>,
    /// Serialized path: `/a/b` for a list path (`""` for an empty list),
    /// the raw string for an opaque path (see `opaque_path`).
    pub path: String,
    /// The path is opaque (`mailto:x`, `data:...`, `about:blank`).
    pub opaque_path: bool,
    pub query: Option<String>,
    pub fragment: Option<String>,
}

/// The special schemes and their default ports.
pub fn special_default_port(scheme: &str) -> Option<Option<u16>> {
    match scheme {
        "http" | "ws" => Some(Some(80)),
        "https" | "wss" => Some(Some(443)),
        "ftp" => Some(Some(21)),
        "file" => Some(None),
        _ => None,
    }
}

pub fn is_special_scheme(scheme: &str) -> bool {
    special_default_port(scheme).is_some()
}

// ------------------------------------------------------- percent-encoding

/// Percent-encode sets (URL Standard §1.3).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EncodeSet {
    C0Control,
    Fragment,
    Query,
    SpecialQuery,
    Path,
    Userinfo,
    Component,
    FormUrlencoded,
}

/// Is byte `c` in percent-encode set `set`?
pub fn in_set(set: EncodeSet, c: u8) -> bool {
    use EncodeSet::*;
    let c0 = c < 0x20 || c > 0x7e;
    match set {
        C0Control => c0,
        Fragment => c0 || matches!(c, b' ' | b'"' | b'<' | b'>' | b'`'),
        Query => c0 || matches!(c, b' ' | b'"' | b'#' | b'<' | b'>'),
        SpecialQuery => in_set(Query, c) || c == b'\'',
        Path => in_set(Query, c) || matches!(c, b'?' | b'^' | b'`' | b'{' | b'}'),
        Userinfo => in_set(Path, c) || matches!(c, b'/' | b':' | b';' | b'=' | b'@' | b'[' | b'\\' | b']' | b'|'),
        Component => in_set(Userinfo, c) || matches!(c, b'$' | b'%' | b'&' | b'+' | b','),
        FormUrlencoded => in_set(Component, c) || matches!(c, b'!' | b'\'' | b'(' | b')' | b'~'),
    }
}

/// UTF-8 percent-encode `s` with `set`.
pub fn percent_encode(s: &str, set: EncodeSet) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if in_set(set, b) {
            out.push_str(&format!("%{b:02X}"));
        } else {
            out.push(b as char);
        }
    }
    out
}

fn push_encoded(out: &mut String, c: char, set: EncodeSet) {
    if (c as u32) < 0x80 && !in_set(set, c as u8) {
        out.push(c);
        return;
    }
    let mut buf = [0u8; 4];
    for &b in c.encode_utf8(&mut buf).as_bytes() {
        if in_set(set, b) {
            out.push_str(&format!("%{b:02X}"));
        } else {
            out.push(b as char);
        }
    }
}

/// Percent-encode bytes that cannot appear verbatim in a URL path or query:
/// spaces, control characters, non-ASCII, `"`, `<`, `>`, `` ` ``, `{`, `}`,
/// `|`, `\`, `^`. Existing `%` sequences are left untouched.
pub fn percent_encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        let keep = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'.' | b'_' | b'~' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b','
                    | b';' | b'=' | b':' | b'@' | b'/' | b'?' | b'%' | b'[' | b']' | b'#'
            );
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Percent-decode to bytes.
pub fn percent_decode_to_bytes(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'%' {
            if let (Some(h), Some(l)) = (s.get(i + 1).and_then(|c| (*c as char).to_digit(16)), s.get(i + 2).and_then(|c| (*c as char).to_digit(16))) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(s[i]);
        i += 1;
    }
    out
}

/// Decode `%XX` escapes (invalid UTF-8 becomes U+FFFD). `+` is left as-is.
pub fn percent_decode(s: &str) -> String {
    String::from_utf8_lossy(&percent_decode_to_bytes(s.as_bytes())).into_owned()
}

// ------------------------------------------------------------------ hosts

fn is_forbidden_host(c: char) -> bool {
    matches!(c, '\0' | '\t' | '\n' | '\r' | ' ' | '#' | '/' | ':' | '<' | '>' | '?' | '@' | '[' | '\\' | ']' | '^' | '|')
}

fn is_forbidden_domain(c: char) -> bool {
    is_forbidden_host(c) || (c as u32) <= 0x1f || c == '%' || c == '\u{7f}'
}

/// Host parser (§3.5). `not_special`: parse as an opaque host.
pub fn parse_host(input: &str, not_special: bool) -> Option<String> {
    if let Some(rest) = input.strip_prefix('[') {
        let inner = rest.strip_suffix(']')?;
        let addr = parse_ipv6(inner)?;
        return Some(format!("[{}]", serialize_ipv6(&addr)));
    }
    if not_special {
        if input.chars().any(is_forbidden_host) {
            return None;
        }
        return Some(percent_encode(input, EncodeSet::C0Control));
    }
    let bytes = percent_decode_to_bytes(input.as_bytes());
    let domain = String::from_utf8_lossy(&bytes).into_owned();
    let ascii = domain_to_ascii(&domain, false)?;
    if ascii.chars().any(is_forbidden_domain) {
        return None;
    }
    if ends_in_number(&ascii) {
        let v4 = parse_ipv4(&ascii)?;
        return Some(serialize_ipv4(v4));
    }
    Some(ascii)
}

fn ends_in_number(s: &str) -> bool {
    let mut parts: Vec<&str> = s.split('.').collect();
    if parts.last() == Some(&"") {
        if parts.len() == 1 {
            return false;
        }
        parts.pop();
    }
    let last = parts.last().copied().unwrap_or("");
    if !last.is_empty() && last.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    parse_ipv4_number(last).is_some()
}

fn parse_ipv4_number(s: &str) -> Option<u64> {
    if s.is_empty() {
        return None;
    }
    let (digits, radix) = if s.len() >= 2 && (s.starts_with("0x") || s.starts_with("0X")) {
        (&s[2..], 16)
    } else if s.len() >= 2 && s.starts_with('0') {
        (&s[1..], 8)
    } else {
        (s, 10)
    };
    if digits.is_empty() {
        return Some(0);
    }
    if !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    // Saturate: anything this large fails the range checks anyway.
    let mut v: u64 = 0;
    for c in digits.chars() {
        v = v.saturating_mul(radix as u64).saturating_add(c.to_digit(radix).unwrap() as u64);
    }
    Some(v)
}

fn parse_ipv4(s: &str) -> Option<u32> {
    let mut parts: Vec<&str> = s.split('.').collect();
    if parts.last() == Some(&"") && parts.len() > 1 {
        parts.pop();
    }
    if parts.len() > 4 {
        return None;
    }
    let mut nums = Vec::with_capacity(4);
    for p in &parts {
        nums.push(parse_ipv4_number(p)?);
    }
    let n = nums.len();
    if nums[..n - 1].iter().any(|&x| x > 255) {
        return None;
    }
    if nums[n - 1] >= 256u64.pow((5 - n) as u32) {
        return None;
    }
    let mut v: u64 = nums[n - 1];
    for (i, &x) in nums[..n - 1].iter().enumerate() {
        v += x * 256u64.pow((3 - i) as u32);
    }
    Some(v as u32)
}

fn serialize_ipv4(v: u32) -> String {
    format!("{}.{}.{}.{}", v >> 24, (v >> 16) & 255, (v >> 8) & 255, v & 255)
}

fn parse_ipv6(input: &str) -> Option<[u16; 8]> {
    let c: Vec<char> = input.chars().collect();
    let mut addr = [0u16; 8];
    let mut piece = 0usize;
    let mut compress: Option<usize> = None;
    let mut p = 0usize;
    if c.first() == Some(&':') {
        if c.get(1) != Some(&':') {
            return None;
        }
        p += 2;
        piece += 1;
        compress = Some(piece);
    }
    while p < c.len() {
        if piece == 8 {
            return None;
        }
        if c[p] == ':' {
            if compress.is_some() {
                return None;
            }
            p += 1;
            piece += 1;
            compress = Some(piece);
            continue;
        }
        let mut value: u32 = 0;
        let mut length = 0;
        while length < 4 && p < c.len() && c[p].is_ascii_hexdigit() {
            value = value * 16 + c[p].to_digit(16).unwrap();
            p += 1;
            length += 1;
        }
        if p < c.len() && c[p] == '.' {
            if length == 0 {
                return None;
            }
            p -= length;
            if piece > 6 {
                return None;
            }
            let mut numbers_seen = 0;
            while p < c.len() {
                let mut ipv4_piece: Option<u32> = None;
                if numbers_seen > 0 {
                    if c[p] == '.' && numbers_seen < 4 {
                        p += 1;
                    } else {
                        return None;
                    }
                }
                if p >= c.len() || !c[p].is_ascii_digit() {
                    return None;
                }
                while p < c.len() && c[p].is_ascii_digit() {
                    let n = c[p].to_digit(10).unwrap();
                    ipv4_piece = match ipv4_piece {
                        None => Some(n),
                        Some(0) => return None,
                        Some(v) => Some(v * 10 + n),
                    };
                    if ipv4_piece.unwrap() > 255 {
                        return None;
                    }
                    p += 1;
                }
                addr[piece] = (addr[piece] as u32 * 0x100 + ipv4_piece.unwrap()) as u16;
                numbers_seen += 1;
                if numbers_seen == 2 || numbers_seen == 4 {
                    piece += 1;
                }
            }
            if numbers_seen != 4 {
                return None;
            }
            break;
        } else if p < c.len() && c[p] == ':' {
            p += 1;
            if p >= c.len() {
                return None;
            }
        } else if p < c.len() {
            return None;
        }
        addr[piece] = value as u16;
        piece += 1;
    }
    if let Some(comp) = compress {
        let mut swaps = piece - comp;
        piece = 7;
        while piece != 0 && swaps > 0 {
            addr.swap(piece, comp + swaps - 1);
            piece -= 1;
            swaps -= 1;
        }
    } else if piece != 8 {
        return None;
    }
    Some(addr)
}

fn serialize_ipv6(a: &[u16; 8]) -> String {
    // The first longest run of two or more zero pieces is compressed.
    let mut best: Option<(usize, usize)> = None;
    let mut i = 0;
    while i < 8 {
        if a[i] == 0 {
            let start = i;
            while i < 8 && a[i] == 0 {
                i += 1;
            }
            let len = i - start;
            if len >= 2 && best.map(|(_, l)| len > l).unwrap_or(true) {
                best = Some((start, len));
            }
        } else {
            i += 1;
        }
    }
    let mut out = String::new();
    let mut ignore0 = false;
    for (i, v) in a.iter().enumerate() {
        if ignore0 && *v == 0 {
            continue;
        }
        ignore0 = false;
        if let Some((s, _)) = best {
            if s == i {
                out.push_str(if i == 0 { "::" } else { ":" });
                ignore0 = true;
                continue;
            }
        }
        out.push_str(&format!("{v:x}"));
        if i != 7 {
            out.push(':');
        }
    }
    out
}

// ------------------------------------------------------------- IDNA (UTS 46)

/// UTS 46 status of a code point: (status, mapping) from the IDNA
/// mapping table (0 valid, 1 ignored, 2 mapped, 3 deviation, 4 disallowed).
fn idna_status(c: char) -> (u8, &'static [u32]) {
    use crate::idna_table::{MAPPINGS, ROWS};
    let u = c as u32;
    let i = ROWS.partition_point(|r| r.1 < u);
    match ROWS.get(i) {
        Some(&(first, _, status, off, len)) if first <= u => (status, &MAPPINGS[off as usize..off as usize + len as usize]),
        _ => (4, &[]),
    }
}

/// Map one code point per UTS 46 (nontransitional processing). `None`
/// means the code point is disallowed.
fn idna_map(c: char, out: &mut String) -> Option<()> {
    match idna_status(c) {
        (0 | 3, _) => out.push(c),
        (1, _) => {}
        (2, m) => out.extend(m.iter().filter_map(|&u| char::from_u32(u))),
        _ => return None,
    }
    Some(())
}

/// Is every code point of a decoded punycode label valid as is?
#[allow(dead_code)]
fn idna_label_chars_valid(label: &str) -> bool {
    label.chars().all(|c| matches!(idna_status(c).0, 0 | 3))
}

/// domain to ASCII (UTS 46 ToASCII with the URL Standard's flags).
pub fn domain_to_ascii(domain: &str, be_strict: bool) -> Option<String> {
    if domain.is_ascii() {
        let lower = domain.to_ascii_lowercase();
        // Fast path: no punycode labels to validate.
        if !lower.split('.').any(|l| l.starts_with("xn--")) {
            if lower.is_empty() {
                return None;
            }
            return Some(lower);
        }
    }
    let mut mapped = String::with_capacity(domain.len());
    for c in domain.chars() {
        idna_map(c, &mut mapped)?;
    }
    let mut labels = Vec::new();
    for label in mapped.split('.') {
        if let Some(puny) = label.strip_prefix("xn--") {
            if puny.is_empty() {
                labels.push(label.to_string());
                continue;
            }
            if !puny.is_ascii() {
                return None;
            }
            let decoded = punycode_decode(puny)?;
            if decoded.is_empty() || decoded.is_ascii() {
                return None;
            }
            if !label_valid(&decoded) {
                return None;
            }
            labels.push(label.to_string());
        } else if label.is_ascii() {
            labels.push(label.to_string());
        } else {
            if !label_valid(label) {
                return None;
            }
            labels.push(format!("xn--{}", punycode_encode(label)?));
        }
    }
    let out = labels.join(".");
    if out.is_empty() {
        return None;
    }
    if be_strict && (out.len() > 253 || labels.iter().any(|l| l.is_empty() || l.len() > 63)) {
        return None;
    }
    Some(out)
}

/// Validity criteria that need no tables: no leading combining mark, no
/// `--` in positions 3-4 for non-punycode, no disallowed characters.
fn label_valid(label: &str) -> bool {
    let first = label.chars().next();
    if let Some(f) = first {
        let u = f as u32;
        // General category M (combining marks), the common ranges.
        if (0x0300..=0x036F).contains(&u) || (0x0483..=0x0489).contains(&u) || (0x0591..=0x05BD).contains(&u) || (0x20D0..=0x20FF).contains(&u) || (0xFE20..=0xFE2F).contains(&u) {
            return false;
        }
    }
    !label.chars().any(|c| is_forbidden_domain(c) || c == '.')
}

const PUNY_BASE: u32 = 36;
const PUNY_TMIN: u32 = 1;
const PUNY_TMAX: u32 = 26;
const PUNY_SKEW: u32 = 38;
const PUNY_DAMP: u32 = 700;
const PUNY_INITIAL_BIAS: u32 = 72;
const PUNY_INITIAL_N: u32 = 128;

fn puny_adapt(mut delta: u32, num_points: u32, first: bool) -> u32 {
    delta /= if first { PUNY_DAMP } else { 2 };
    delta += delta / num_points;
    let mut k = 0;
    while delta > ((PUNY_BASE - PUNY_TMIN) * PUNY_TMAX) / 2 {
        delta /= PUNY_BASE - PUNY_TMIN;
        k += PUNY_BASE;
    }
    k + (((PUNY_BASE - PUNY_TMIN + 1) * delta) / (delta + PUNY_SKEW))
}

fn puny_digit(d: u32) -> char {
    (if d < 26 { b'a' + d as u8 } else { b'0' + (d as u8 - 26) }) as char
}

pub fn punycode_encode(input: &str) -> Option<String> {
    let cps: Vec<u32> = input.chars().map(|c| c as u32).collect();
    let mut out: String = cps.iter().filter(|&&c| c < 0x80).map(|&c| c as u8 as char).collect();
    let b = out.len() as u32;
    let mut h = b;
    if b > 0 {
        out.push('-');
    }
    let mut n = PUNY_INITIAL_N;
    let mut delta: u32 = 0;
    let mut bias = PUNY_INITIAL_BIAS;
    while (h as usize) < cps.len() {
        let m = *cps.iter().filter(|&&c| c >= n).min()?;
        delta = delta.checked_add((m - n).checked_mul(h + 1)?)?;
        n = m;
        for &c in &cps {
            if c < n {
                delta = delta.checked_add(1)?;
            }
            if c == n {
                let mut q = delta;
                let mut k = PUNY_BASE;
                loop {
                    let t = if k <= bias { PUNY_TMIN } else if k >= bias + PUNY_TMAX { PUNY_TMAX } else { k - bias };
                    if q < t {
                        break;
                    }
                    out.push(puny_digit(t + (q - t) % (PUNY_BASE - t)));
                    q = (q - t) / (PUNY_BASE - t);
                    k += PUNY_BASE;
                }
                out.push(puny_digit(q));
                bias = puny_adapt(delta, h + 1, h == b);
                delta = 0;
                h += 1;
            }
        }
        delta += 1;
        n += 1;
    }
    Some(out)
}

pub fn punycode_decode(input: &str) -> Option<String> {
    let (basic, rest) = match input.rfind('-') {
        Some(i) => (&input[..i], &input[i + 1..]),
        None => ("", input),
    };
    let mut out: Vec<u32> = basic.chars().map(|c| c as u32).collect();
    let mut n = PUNY_INITIAL_N;
    let mut i: u32 = 0;
    let mut bias = PUNY_INITIAL_BIAS;
    let bytes = rest.as_bytes();
    let mut p = 0;
    while p < bytes.len() {
        let old_i = i;
        let mut w: u32 = 1;
        let mut k = PUNY_BASE;
        loop {
            let c = *bytes.get(p)?;
            p += 1;
            let digit = match c {
                b'a'..=b'z' => c - b'a',
                b'A'..=b'Z' => c - b'A',
                b'0'..=b'9' => c - b'0' + 26,
                _ => return None,
            } as u32;
            i = i.checked_add(digit.checked_mul(w)?)?;
            let t = if k <= bias { PUNY_TMIN } else if k >= bias + PUNY_TMAX { PUNY_TMAX } else { k - bias };
            if digit < t {
                break;
            }
            w = w.checked_mul(PUNY_BASE - t)?;
            k += PUNY_BASE;
        }
        let len = out.len() as u32 + 1;
        bias = puny_adapt(i - old_i, len, old_i == 0);
        n = n.checked_add(i / len)?;
        i %= len;
        out.insert(i as usize, n);
        i += 1;
    }
    out.into_iter().map(char::from_u32).collect()
}

// ------------------------------------------------------------- the parser

/// Parser states (§4.4), also used as state overrides by the setters.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    SchemeStart,
    Scheme,
    NoScheme,
    SpecialRelativeOrAuthority,
    PathOrAuthority,
    Relative,
    RelativeSlash,
    SpecialAuthoritySlashes,
    SpecialAuthorityIgnoreSlashes,
    Authority,
    Host,
    Hostname,
    Port,
    File,
    FileSlash,
    FileHost,
    PathStart,
    Path,
    OpaquePath,
    Query,
    Fragment,
}

/// Mutable form of a URL used while parsing (path as a list).
struct Rec {
    scheme: String,
    username: String,
    password: String,
    host: Option<String>,
    port: Option<u16>,
    path: Vec<String>,
    opaque: Option<String>,
    query: Option<String>,
    fragment: Option<String>,
}

impl Rec {
    fn empty() -> Rec {
        Rec { scheme: String::new(), username: String::new(), password: String::new(), host: None, port: None, path: Vec::new(), opaque: None, query: None, fragment: None }
    }

    fn from_url(u: &Url) -> Rec {
        let (path, opaque) = if u.opaque_path {
            (Vec::new(), Some(u.path.clone()))
        } else if u.path.is_empty() {
            (Vec::new(), None)
        } else {
            (u.path.strip_prefix('/').unwrap_or(&u.path).split('/').map(|s| s.to_string()).collect(), None)
        };
        Rec {
            scheme: u.scheme.clone(),
            username: u.username.clone(),
            password: u.password.clone(),
            host: if u.has_host { Some(u.host.clone()) } else { None },
            port: u.port,
            path,
            opaque,
            query: u.query.clone(),
            fragment: u.fragment.clone(),
        }
    }

    fn into_url(self) -> Url {
        let opaque_path = self.opaque.is_some();
        let path = match self.opaque {
            Some(o) => o,
            None => {
                let mut s = String::new();
                for seg in &self.path {
                    s.push('/');
                    s.push_str(seg);
                }
                s
            }
        };
        Url {
            scheme: self.scheme,
            username: self.username,
            password: self.password,
            has_host: self.host.is_some(),
            host: self.host.unwrap_or_default(),
            port: self.port,
            path,
            opaque_path,
            query: self.query,
            fragment: self.fragment,
        }
    }

    fn special(&self) -> bool {
        is_special_scheme(&self.scheme)
    }

    fn includes_credentials(&self) -> bool {
        !self.username.is_empty() || !self.password.is_empty()
    }

    fn shorten_path(&mut self) {
        if self.scheme == "file" && self.path.len() == 1 && is_normalized_drive_letter(&self.path[0]) {
            return;
        }
        self.path.pop();
    }
}

fn is_drive_letter(s: &str) -> bool {
    let c: Vec<char> = s.chars().collect();
    c.len() == 2 && c[0].is_ascii_alphabetic() && (c[1] == ':' || c[1] == '|')
}

fn is_normalized_drive_letter(s: &str) -> bool {
    is_drive_letter(s) && s.ends_with(':')
}

fn starts_with_drive_letter(c: &[char]) -> bool {
    c.len() >= 2 && c[0].is_ascii_alphabetic() && (c[1] == ':' || c[1] == '|') && (c.len() == 2 || matches!(c[2], '/' | '\\' | '?' | '#'))
}

fn is_single_dot(s: &str) -> bool {
    s == "." || s.eq_ignore_ascii_case("%2e")
}

fn is_double_dot(s: &str) -> bool {
    matches!(s.to_ascii_lowercase().as_str(), ".." | ".%2e" | "%2e." | "%2e%2e")
}

/// Why `basic_parse` stopped.
#[derive(Debug, PartialEq)]
enum Outcome {
    Done,
    Failure,
}

/// The basic URL parser (§4.4) over `rec`; with `state_override` it edits
/// an existing record (the URL setters).
fn basic_parse(input: &str, base: Option<&Rec>, rec: &mut Rec, state_override: Option<State>, encoding: crate::encoding::Encoding) -> Outcome {
    let trimmed = if state_override.is_none() { input.trim_matches(|c: char| c <= ' ') } else { input };
    let chars: Vec<char> = trimmed.chars().filter(|&c| !matches!(c, '\t' | '\n' | '\r')).collect();
    let mut state = state_override.unwrap_or(State::SchemeStart);
    let mut buffer = String::new();
    let mut at_sign_seen = false;
    let mut inside_brackets = false;
    let mut password_token_seen = false;
    let mut p: isize = 0;
    let ovr = state_override.is_some();
    loop {
        let c: Option<char> = if p >= 0 { chars.get(p as usize).copied() } else { None };
        let cref = &chars;
        let pp = p.max(0) as usize;
        let remaining = move |off: usize| -> &[char] { &cref[(pp + off).min(cref.len())..] };
        match state {
            State::SchemeStart => match c {
                Some(ch) if ch.is_ascii_alphabetic() => {
                    buffer.push(ch.to_ascii_lowercase());
                    state = State::Scheme;
                }
                _ if !ovr => {
                    state = State::NoScheme;
                    p -= 1;
                }
                _ => return Outcome::Failure,
            },
            State::Scheme => match c {
                Some(ch) if ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.') => buffer.push(ch.to_ascii_lowercase()),
                Some(':') => {
                    if ovr {
                        if rec.special() != is_special_scheme(&buffer) {
                            return Outcome::Done;
                        }
                        if (rec.includes_credentials() || rec.port.is_some()) && buffer == "file" {
                            return Outcome::Done;
                        }
                        if rec.scheme == "file" && rec.host.as_deref() == Some("") {
                            return Outcome::Done;
                        }
                    }
                    rec.scheme = std::mem::take(&mut buffer);
                    if ovr {
                        if let Some(Some(d)) = special_default_port(&rec.scheme) {
                            if rec.port == Some(d) {
                                rec.port = None;
                            }
                        }
                        return Outcome::Done;
                    }
                    if rec.scheme == "file" {
                        state = State::File;
                    } else if rec.special() && base.map(|b| b.scheme == rec.scheme).unwrap_or(false) {
                        state = State::SpecialRelativeOrAuthority;
                    } else if rec.special() {
                        state = State::SpecialAuthoritySlashes;
                    } else if remaining(1).first() == Some(&'/') {
                        state = State::PathOrAuthority;
                        p += 1;
                    } else {
                        rec.opaque = Some(String::new());
                        state = State::OpaquePath;
                    }
                }
                _ if !ovr => {
                    buffer.clear();
                    state = State::NoScheme;
                    p = -1;
                }
                _ => return Outcome::Failure,
            },
            State::NoScheme => {
                let Some(b) = base else { return Outcome::Failure };
                if b.opaque.is_some() && c != Some('#') {
                    return Outcome::Failure;
                } else if b.opaque.is_some() {
                    rec.scheme = b.scheme.clone();
                    rec.path = b.path.clone();
                    rec.opaque = b.opaque.clone();
                    rec.query = b.query.clone();
                    rec.fragment = Some(String::new());
                    state = State::Fragment;
                } else if b.scheme != "file" {
                    state = State::Relative;
                    p -= 1;
                } else {
                    state = State::File;
                    p -= 1;
                }
            }
            State::SpecialRelativeOrAuthority => {
                if c == Some('/') && remaining(1).first() == Some(&'/') {
                    state = State::SpecialAuthorityIgnoreSlashes;
                    p += 1;
                } else {
                    state = State::Relative;
                    p -= 1;
                }
            }
            State::PathOrAuthority => {
                if c == Some('/') {
                    state = State::Authority;
                } else {
                    state = State::Path;
                    p -= 1;
                }
            }
            State::Relative => {
                let b = base.expect("relative state needs a base");
                rec.scheme = b.scheme.clone();
                if c == Some('/') || (rec.special() && c == Some('\\')) {
                    state = State::RelativeSlash;
                } else {
                    rec.username = b.username.clone();
                    rec.password = b.password.clone();
                    rec.host = b.host.clone();
                    rec.port = b.port;
                    rec.path = b.path.clone();
                    rec.query = b.query.clone();
                    match c {
                        Some('?') => {
                            rec.query = Some(String::new());
                            state = State::Query;
                        }
                        Some('#') => {
                            rec.fragment = Some(String::new());
                            state = State::Fragment;
                        }
                        Some(_) => {
                            rec.query = None;
                            rec.shorten_path();
                            state = State::Path;
                            p -= 1;
                        }
                        None => {}
                    }
                }
            }
            State::RelativeSlash => {
                if rec.special() && (c == Some('/') || c == Some('\\')) {
                    state = State::SpecialAuthorityIgnoreSlashes;
                } else if c == Some('/') {
                    state = State::Authority;
                } else {
                    let b = base.expect("base");
                    rec.username = b.username.clone();
                    rec.password = b.password.clone();
                    rec.host = b.host.clone();
                    rec.port = b.port;
                    state = State::Path;
                    p -= 1;
                }
            }
            State::SpecialAuthoritySlashes => {
                if c == Some('/') && remaining(1).first() == Some(&'/') {
                    state = State::SpecialAuthorityIgnoreSlashes;
                    p += 1;
                } else {
                    state = State::SpecialAuthorityIgnoreSlashes;
                    p -= 1;
                }
            }
            State::SpecialAuthorityIgnoreSlashes => {
                if c != Some('/') && c != Some('\\') {
                    state = State::Authority;
                    p -= 1;
                }
            }
            State::Authority => {
                if c == Some('@') {
                    if at_sign_seen {
                        buffer.insert_str(0, "%40");
                    }
                    at_sign_seen = true;
                    for cp in buffer.chars() {
                        if cp == ':' && !password_token_seen {
                            password_token_seen = true;
                            continue;
                        }
                        if password_token_seen {
                            push_encoded(&mut rec.password, cp, EncodeSet::Userinfo);
                        } else {
                            push_encoded(&mut rec.username, cp, EncodeSet::Userinfo);
                        }
                    }
                    buffer.clear();
                } else if matches!(c, None | Some('/') | Some('?') | Some('#')) || (rec.special() && c == Some('\\')) {
                    if at_sign_seen && buffer.is_empty() {
                        return Outcome::Failure;
                    }
                    p -= buffer.chars().count() as isize + 1;
                    buffer.clear();
                    state = State::Host;
                } else if let Some(ch) = c {
                    buffer.push(ch);
                }
            }
            State::Host | State::Hostname => {
                if ovr && rec.scheme == "file" {
                    p -= 1;
                    state = State::FileHost;
                } else if c == Some(':') && !inside_brackets {
                    if buffer.is_empty() {
                        return Outcome::Failure;
                    }
                    if state_override == Some(State::Hostname) {
                        return Outcome::Failure;
                    }
                    let Some(h) = parse_host(&buffer, !rec.special()) else { return Outcome::Failure };
                    rec.host = Some(h);
                    buffer.clear();
                    state = State::Port;
                } else if matches!(c, None | Some('/') | Some('?') | Some('#')) || (rec.special() && c == Some('\\')) {
                    p -= 1;
                    if rec.special() && buffer.is_empty() {
                        return Outcome::Failure;
                    } else if ovr && buffer.is_empty() && (rec.includes_credentials() || rec.port.is_some()) {
                        return Outcome::Failure;
                    }
                    let Some(h) = parse_host(&buffer, !rec.special()) else { return Outcome::Failure };
                    rec.host = Some(h);
                    buffer.clear();
                    state = State::PathStart;
                    if ovr {
                        return Outcome::Done;
                    }
                } else if let Some(ch) = c {
                    if ch == '[' {
                        inside_brackets = true;
                    }
                    if ch == ']' {
                        inside_brackets = false;
                    }
                    buffer.push(ch);
                }
            }
            State::Port => {
                if let Some(d) = c.filter(|ch| ch.is_ascii_digit()) {
                    buffer.push(d);
                } else if matches!(c, None | Some('/') | Some('?') | Some('#')) || (rec.special() && c == Some('\\')) || ovr {
                    if !buffer.is_empty() {
                        let digits = buffer.trim_start_matches('0');
                        if digits.len() > 5 {
                            return Outcome::Failure;
                        }
                        let port: u32 = if digits.is_empty() { 0 } else { digits.parse().unwrap_or(u32::MAX) };
                        if port > 65535 {
                            return Outcome::Failure;
                        }
                        let port = port as u16;
                        rec.port = if special_default_port(&rec.scheme) == Some(Some(port)) { None } else { Some(port) };
                        buffer.clear();
                        if ovr {
                            return Outcome::Done;
                        }
                    }
                    if ovr {
                        return Outcome::Failure;
                    }
                    state = State::PathStart;
                    p -= 1;
                } else {
                    return Outcome::Failure;
                }
            }
            State::File => {
                rec.scheme = "file".into();
                rec.host = Some(String::new());
                if c == Some('/') || c == Some('\\') {
                    state = State::FileSlash;
                } else if let Some(b) = base.filter(|b| b.scheme == "file") {
                    rec.host = b.host.clone();
                    rec.path = b.path.clone();
                    rec.query = b.query.clone();
                    match c {
                        Some('?') => {
                            rec.query = Some(String::new());
                            state = State::Query;
                        }
                        Some('#') => {
                            rec.fragment = Some(String::new());
                            state = State::Fragment;
                        }
                        Some(_) => {
                            rec.query = None;
                            if !starts_with_drive_letter(remaining(0)) {
                                rec.shorten_path();
                            } else {
                                rec.path.clear();
                            }
                            state = State::Path;
                            p -= 1;
                        }
                        None => {}
                    }
                } else {
                    state = State::Path;
                    p -= 1;
                }
            }
            State::FileSlash => {
                if c == Some('/') || c == Some('\\') {
                    state = State::FileHost;
                } else {
                    if let Some(b) = base.filter(|b| b.scheme == "file") {
                        rec.host = b.host.clone();
                        if !starts_with_drive_letter(remaining(0)) {
                            if let Some(first) = b.path.first().filter(|f| is_normalized_drive_letter(f)) {
                                rec.path.push(first.clone());
                            }
                        }
                    }
                    state = State::Path;
                    p -= 1;
                }
            }
            State::FileHost => {
                if matches!(c, None | Some('/') | Some('\\') | Some('?') | Some('#')) {
                    p -= 1;
                    if !ovr && is_drive_letter(&buffer) {
                        state = State::Path;
                    } else if buffer.is_empty() {
                        rec.host = Some(String::new());
                        if ovr {
                            return Outcome::Done;
                        }
                        state = State::PathStart;
                    } else {
                        let Some(mut h) = parse_host(&buffer, !rec.special()) else { return Outcome::Failure };
                        if h == "localhost" {
                            h.clear();
                        }
                        rec.host = Some(h);
                        if ovr {
                            return Outcome::Done;
                        }
                        buffer.clear();
                        state = State::PathStart;
                    }
                } else if let Some(ch) = c {
                    buffer.push(ch);
                }
            }
            State::PathStart => {
                if rec.special() {
                    state = State::Path;
                    if c != Some('/') && c != Some('\\') {
                        p -= 1;
                    }
                } else if !ovr && c == Some('?') {
                    rec.query = Some(String::new());
                    state = State::Query;
                } else if !ovr && c == Some('#') {
                    rec.fragment = Some(String::new());
                    state = State::Fragment;
                } else if c.is_some() {
                    state = State::Path;
                    if c != Some('/') {
                        p -= 1;
                    }
                } else if ovr && rec.host.is_none() {
                    rec.path.push(String::new());
                }
            }
            State::Path => {
                let slash = c == Some('/') || (rec.special() && c == Some('\\'));
                if c.is_none() || slash || (!ovr && matches!(c, Some('?') | Some('#'))) {
                    if is_double_dot(&buffer) {
                        rec.shorten_path();
                        if !slash {
                            rec.path.push(String::new());
                        }
                    } else if is_single_dot(&buffer) && !slash {
                        rec.path.push(String::new());
                    } else if !is_single_dot(&buffer) {
                        if rec.scheme == "file" && rec.path.is_empty() && is_drive_letter(&buffer) {
                            buffer = format!("{}:", &buffer[..1]);
                        }
                        rec.path.push(std::mem::take(&mut buffer));
                    }
                    buffer.clear();
                    if c == Some('?') {
                        rec.query = Some(String::new());
                        state = State::Query;
                    }
                    if c == Some('#') {
                        rec.fragment = Some(String::new());
                        state = State::Fragment;
                    }
                } else if let Some(ch) = c {
                    push_encoded(&mut buffer, ch, EncodeSet::Path);
                }
            }
            State::OpaquePath => {
                let path = rec.opaque.get_or_insert_with(String::new);
                match c {
                    Some('?') => {
                        rec.query = Some(String::new());
                        state = State::Query;
                    }
                    Some('#') => {
                        rec.fragment = Some(String::new());
                        state = State::Fragment;
                    }
                    Some(' ') => {
                        if matches!(remaining(1).first(), Some('?') | Some('#')) {
                            path.push_str("%20");
                        } else {
                            path.push(' ');
                        }
                    }
                    Some(ch) => push_encoded(path, ch, EncodeSet::C0Control),
                    None => {}
                }
            }
            State::Query => {
                if (!ovr && c == Some('#')) || c.is_none() {
                    let set = if rec.special() { EncodeSet::SpecialQuery } else { EncodeSet::Query };
                    // A document's legacy encoding applies to special,
                    // non-WebSocket URLs' queries.
                    let legacy = encoding != crate::encoding::Encoding::UTF_8 && rec.special() && rec.scheme != "ws" && rec.scheme != "wss";
                    let q = rec.query.get_or_insert_with(String::new);
                    if legacy {
                        q.push_str(&crate::encoding::percent_encode_after_encoding(encoding, &buffer, |b| in_set(set, b), false));
                    } else {
                        for ch in buffer.chars() {
                            push_encoded(q, ch, set);
                        }
                    }
                    buffer.clear();
                    if c == Some('#') {
                        rec.fragment = Some(String::new());
                        state = State::Fragment;
                    }
                } else if let Some(ch) = c {
                    buffer.push(ch);
                }
            }
            State::Fragment => {
                if let Some(ch) = c {
                    push_encoded(rec.fragment.get_or_insert_with(String::new), ch, EncodeSet::Fragment);
                }
            }
        }
        // Stop once the pointer rests on EOF after a run.
        if p >= 0 && p as usize >= chars.len() {
            break;
        }
        p += 1;
    }
    Outcome::Done
}

/// Parse `input` against an optional base (the URL parser, §4.4).
pub fn parse_with_base(input: &str, base: Option<&Url>) -> Option<Url> {
    parse_with_encoding(input, base, crate::encoding::Encoding::UTF_8)
}

/// The URL parser with a query encoding (a document's character
/// encoding when resolving its URLs: `<a href>`, forms, ...).
pub fn parse_with_encoding(input: &str, base: Option<&Url>, encoding: crate::encoding::Encoding) -> Option<Url> {
    let base_rec = base.map(Rec::from_url);
    let mut rec = Rec::empty();
    match basic_parse(input, base_rec.as_ref(), &mut rec, None, encoding.output_encoding()) {
        Outcome::Done => Some(rec.into_url()),
        Outcome::Failure => None,
    }
}

impl Url {
    /// Parse an absolute URL. Accepts input a user might type: a bare
    /// `example.com/x` gets `https://` prepended.
    pub fn parse(input: &str) -> Option<Url> {
        let input = input.trim();
        if input.is_empty() {
            return None;
        }
        if let Some((scheme, rest)) = input.split_once(':') {
            let s = scheme.to_ascii_lowercase();
            // Guard against "localhost:8080/x" being read as scheme "localhost".
            let known = is_special_scheme(&s)
                || matches!(s.as_str(), "data" | "about" | "mailto" | "javascript" | "blob" | "tel" | "view-source")
                || rest.starts_with("//");
            if known {
                if let Some(u) = Url::parse_absolute(input) {
                    return Some(u);
                }
            }
        }
        if input.starts_with('/') && std::path::Path::new(input).exists() {
            return Url::parse_absolute(&format!("file://{}", percent_encode_component(input)));
        }
        let head = input.split(['/', '?', '#']).next().unwrap_or("");
        let host_part = head.split(':').next().unwrap_or("");
        if host_part == "localhost" || host_part.starts_with("127.") {
            return Url::parse_absolute(&format!("http://{input}"));
        }
        let looks_like_host = !input.contains(char::is_whitespace)
            && !host_part.is_empty()
            && (host_part.contains('.') || input.starts_with("www."))
            && !host_part.starts_with('.')
            && !host_part.ends_with('.');
        if looks_like_host {
            return Url::parse_absolute(&format!("https://{input}"));
        }
        None
    }

    /// Strict absolute-URL parse (no scheme guessing).
    pub fn parse_absolute(input: &str) -> Option<Url> {
        parse_with_base(input, None)
    }

    /// Resolve `reference` against `self` (the URL parser with a base).
    pub fn join(&self, reference: &str) -> Option<Url> {
        parse_with_base(reference, Some(self))
    }

    pub fn is_special(&self) -> bool {
        is_special_scheme(&self.scheme)
    }

    pub fn is_http(&self) -> bool {
        self.scheme == "http" || self.scheme == "https"
    }

    pub fn default_port(&self) -> Option<u16> {
        special_default_port(&self.scheme).flatten()
    }

    /// `host` plus `:port` when a non-default port is set.
    pub fn host_with_port(&self) -> String {
        match self.port {
            Some(p) => format!("{}:{}", self.host, p),
            None => self.host.clone(),
        }
    }

    /// Serialized origin: a tuple origin for the special schemes with
    /// hosts, the wrapped URL's origin for `blob:`, otherwise `null`.
    pub fn origin(&self) -> String {
        match self.scheme.as_str() {
            "http" | "https" | "ws" | "wss" | "ftp" => format!("{}://{}", self.scheme, self.host_with_port()),
            "blob" => match Url::parse_absolute(&self.path) {
                Some(inner) if inner.is_http() => inner.origin(),
                _ => "null".to_string(),
            },
            _ => "null".to_string(),
        }
    }

    /// Same URL without the fragment.
    pub fn without_fragment(&self) -> Url {
        Url { fragment: None, ..self.clone() }
    }

    /// Filesystem path for a `file:` URL (percent-decoded).
    pub fn file_path(&self) -> Option<PathBuf> {
        if self.scheme != "file" {
            return None;
        }
        let decoded = percent_decode(&self.path);
        #[cfg(windows)]
        {
            // "/C:/Users/..." -> "C:/Users/..."
            let b = decoded.as_bytes();
            if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
                return Some(PathBuf::from(&decoded[1..]));
            }
        }
        Some(PathBuf::from(decoded))
    }

    // ---------------------------------------------------- URL API accessors

    /// `URL.prototype.host`.
    pub fn host_component(&self) -> String {
        if !self.has_host {
            return String::new();
        }
        self.host_with_port()
    }

    /// `URL.prototype.pathname`.
    pub fn pathname(&self) -> String {
        self.path.clone()
    }

    /// `URL.prototype.search`.
    pub fn search(&self) -> String {
        match &self.query {
            Some(q) if !q.is_empty() => format!("?{q}"),
            _ => String::new(),
        }
    }

    /// `URL.prototype.hash`.
    pub fn hash(&self) -> String {
        match &self.fragment {
            Some(f) if !f.is_empty() => format!("#{f}"),
            _ => String::new(),
        }
    }

    fn cannot_have_credentials_or_port(&self) -> bool {
        !self.has_host || self.host.is_empty() || self.scheme == "file"
    }

    fn edit(&mut self, input: &str, state: State) {
        // The setters run the parser on the URL itself: what it changed
        // before failing stays changed (`host = "example.com:"`).
        let mut rec = Rec::from_url(self);
        let _ = basic_parse(input, None, &mut rec, Some(state), crate::encoding::Encoding::UTF_8);
        *self = rec.into_url();
    }

    pub fn set_protocol(&mut self, v: &str) {
        self.edit(&format!("{v}:"), State::SchemeStart);
    }

    pub fn set_username(&mut self, v: &str) {
        if self.cannot_have_credentials_or_port() {
            return;
        }
        self.username = percent_encode(v, EncodeSet::Userinfo);
    }

    pub fn set_password(&mut self, v: &str) {
        if self.cannot_have_credentials_or_port() {
            return;
        }
        self.password = percent_encode(v, EncodeSet::Userinfo);
    }

    pub fn set_host(&mut self, v: &str) {
        if self.opaque_path {
            return;
        }
        self.edit(v, State::Host);
    }

    pub fn set_hostname(&mut self, v: &str) {
        if self.opaque_path {
            return;
        }
        self.edit(v, State::Hostname);
    }

    pub fn set_port(&mut self, v: &str) {
        if self.cannot_have_credentials_or_port() {
            return;
        }
        if v.is_empty() {
            self.port = None;
        } else {
            self.edit(v, State::Port);
        }
    }

    pub fn set_pathname(&mut self, v: &str) {
        if self.opaque_path {
            return;
        }
        let mut rec = Rec::from_url(self);
        rec.path.clear();
        if basic_parse(v, None, &mut rec, Some(State::PathStart), crate::encoding::Encoding::UTF_8) == Outcome::Done {
            *self = rec.into_url();
        }
    }

    pub fn set_search(&mut self, v: &str) {
        if v.is_empty() {
            self.query = None;
            return;
        }
        let input = v.strip_prefix('?').unwrap_or(v);
        let mut rec = Rec::from_url(self);
        rec.query = Some(String::new());
        if basic_parse(input, None, &mut rec, Some(State::Query), crate::encoding::Encoding::UTF_8) == Outcome::Done {
            *self = rec.into_url();
        }
    }

    pub fn set_hash(&mut self, v: &str) {
        if v.is_empty() {
            self.fragment = None;
            return;
        }
        let input = v.strip_prefix('#').unwrap_or(v);
        let mut rec = Rec::from_url(self);
        rec.fragment = Some(String::new());
        if basic_parse(input, None, &mut rec, Some(State::Fragment), crate::encoding::Encoding::UTF_8) == Outcome::Done {
            *self = rec.into_url();
        }
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:", self.scheme)?;
        if self.has_host {
            f.write_str("//")?;
            if !self.username.is_empty() || !self.password.is_empty() {
                f.write_str(&self.username)?;
                if !self.password.is_empty() {
                    write!(f, ":{}", self.password)?;
                }
                f.write_str("@")?;
            }
            f.write_str(&self.host)?;
            if let Some(p) = self.port {
                write!(f, ":{p}")?;
            }
        } else if !self.opaque_path && self.path.starts_with("//") {
            f.write_str("/.")?;
        }
        f.write_str(&self.path)?;
        if let Some(q) = &self.query {
            write!(f, "?{q}")?;
        }
        if let Some(fr) = &self.fragment {
            write!(f, "#{fr}")?;
        }
        Ok(())
    }
}

/// Registrable domain (an approximation of eTLD+1 without the full
/// public suffix list): `www.bbc.co.uk` -> `bbc.co.uk`.
pub fn site_of(host: &str) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() <= 2 || host.parse::<std::net::IpAddr>().is_ok() || host.starts_with('[') {
        return host;
    }
    let n = labels.len();
    // Two-part public suffixes like co.uk, com.es, com.au.
    let second = labels[n - 2];
    let take = if matches!(second, "co" | "com" | "org" | "net" | "gov" | "edu" | "ac" | "gob" | "nom") && labels[n - 1].len() == 2 { 3 } else { 2 };
    labels[n - take.min(n)..].join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_absolute_basics() {
        let u = Url::parse_absolute("HTTP://User:pw@Example.COM:80/a/./b/../c?x=1#frag").unwrap();
        assert_eq!(u.scheme, "http");
        assert_eq!(u.host, "example.com");
        assert_eq!(u.port, None);
        assert_eq!(u.path, "/a/c");
        assert_eq!(u.query.as_deref(), Some("x=1"));
        assert_eq!(u.fragment.as_deref(), Some("frag"));
        assert_eq!(u.username, "User");
        assert_eq!(u.to_string(), "http://User:pw@example.com/a/c?x=1#frag");

        let u = Url::parse_absolute("https://example.com:8443").unwrap();
        assert_eq!(u.port, Some(8443));
        assert_eq!(u.path, "/");
        assert_eq!(u.origin(), "https://example.com:8443");
        assert_eq!(u.host_with_port(), "example.com:8443");

        let u = Url::parse_absolute("http://[::1]:8080/x").unwrap();
        assert_eq!(u.host, "[::1]");
        assert_eq!(u.port, Some(8080));

        let u = Url::parse_absolute("file:///tmp/a b/ñ.html").unwrap();
        assert_eq!(u.host, "");
        assert_eq!(u.path, "/tmp/a%20b/%C3%B1.html");
        assert_eq!(u.file_path().unwrap(), PathBuf::from("/tmp/a b/ñ.html"));
        assert_eq!(u.to_string(), "file:///tmp/a%20b/%C3%B1.html");

        let u = Url::parse_absolute("data:text/plain;base64,SGVsbG8=").unwrap();
        assert_eq!(u.scheme, "data");
        assert_eq!(u.path, "text/plain;base64,SGVsbG8=");
        assert_eq!(u.to_string(), "data:text/plain;base64,SGVsbG8=");
        assert_eq!(u.origin(), "null");

        let u = Url::parse_absolute("about:blank").unwrap();
        assert_eq!(u.path, "blank");

        assert!(Url::parse_absolute("example.com").is_none());
        assert!(Url::parse_absolute("://x").is_none());
        assert!(Url::parse_absolute("1http://x").is_none());
        assert!(Url::parse_absolute("http://exa mple.com/").is_none());
        assert!(Url::parse_absolute("http://example.com:abc/").is_none());
    }

    #[test]
    fn parse_user_input() {
        assert_eq!(Url::parse("example.com/x").unwrap().to_string(), "https://example.com/x");
        assert_eq!(Url::parse("  www.example.com  ").unwrap().to_string(), "https://www.example.com/");
        assert_eq!(Url::parse("localhost:8080/x").unwrap().to_string(), "http://localhost:8080/x");
        assert_eq!(Url::parse("localhost").unwrap().to_string(), "http://localhost/");
        assert_eq!(Url::parse("http://example.com").unwrap().to_string(), "http://example.com/");
        assert_eq!(Url::parse("about:blank").unwrap().to_string(), "about:blank");
        assert!(Url::parse("hello world").is_none());
        assert!(Url::parse("").is_none());
        assert!(Url::parse("justaword").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn parse_user_input_local_path() {
        // The root exists on every Unix host; /tmp is neither guaranteed to
        // exist nor an absolute native path on Windows.
        assert_eq!(Url::parse("/").unwrap().to_string(), "file:///");
    }

    #[cfg(windows)]
    #[test]
    fn file_url_windows_path() {
        // File URL parsing does not require the target to exist. Keep the
        // drive prefix when converting a URL to a native Windows path.
        let url = Url::parse("file:///C:/Users/example/a%20b.html").unwrap();
        assert_eq!(url.to_string(), "file:///C:/Users/example/a%20b.html");
        assert_eq!(url.file_path().unwrap(), PathBuf::from(r"C:\Users\example\a b.html"));
    }

    #[test]
    fn rfc3986_normal_examples() {
        let base = Url::parse_absolute("http://a/b/c/d;p?q").unwrap();
        let cases = [
            ("g:h", "g:h"),
            ("g", "http://a/b/c/g"),
            ("./g", "http://a/b/c/g"),
            ("g/", "http://a/b/c/g/"),
            ("/g", "http://a/g"),
            ("//g", "http://g/"),
            ("?y", "http://a/b/c/d;p?y"),
            ("g?y", "http://a/b/c/g?y"),
            ("#s", "http://a/b/c/d;p?q#s"),
            ("g#s", "http://a/b/c/g#s"),
            ("g?y#s", "http://a/b/c/g?y#s"),
            (";x", "http://a/b/c/;x"),
            ("g;x", "http://a/b/c/g;x"),
            ("g;x?y#s", "http://a/b/c/g;x?y#s"),
            ("", "http://a/b/c/d;p?q"),
            (".", "http://a/b/c/"),
            ("./", "http://a/b/c/"),
            ("..", "http://a/b/"),
            ("../", "http://a/b/"),
            ("../g", "http://a/b/g"),
            ("../..", "http://a/"),
            ("../../", "http://a/"),
            ("../../g", "http://a/g"),
            // abnormal
            ("../../../g", "http://a/g"),
            ("../../../../g", "http://a/g"),
            ("/./g", "http://a/g"),
            ("/../g", "http://a/g"),
            ("g.", "http://a/b/c/g."),
            (".g", "http://a/b/c/.g"),
            ("g..", "http://a/b/c/g.."),
            ("..g", "http://a/b/c/..g"),
            ("./../g", "http://a/b/g"),
            ("./g/.", "http://a/b/c/g/"),
            ("g/./h", "http://a/b/c/g/h"),
            ("g/../h", "http://a/b/c/h"),
            ("g;x=1/./y", "http://a/b/c/g;x=1/y"),
            ("g;x=1/../y", "http://a/b/c/y"),
        ];
        for (reference, expected) in cases {
            let got = base.join(reference).unwrap_or_else(|| panic!("join failed for {reference:?}"));
            assert_eq!(got.to_string(), expected, "reference {reference:?}");
        }
    }

    #[test]
    fn join_edge_cases() {
        let base = Url::parse_absolute("https://example.com/dir/page.html?q=1#top").unwrap();
        assert_eq!(base.join("other.html").unwrap().to_string(), "https://example.com/dir/other.html");
        assert_eq!(base.join("//cdn.example.org/x.css").unwrap().to_string(), "https://cdn.example.org/x.css");
        assert_eq!(base.join("#sec").unwrap().to_string(), "https://example.com/dir/page.html?q=1#sec");
        assert_eq!(base.join("?a=b").unwrap().to_string(), "https://example.com/dir/page.html?a=b");
        assert_eq!(base.join("img/ñ x.png").unwrap().path, "/dir/img/%C3%B1%20x.png");
        let data = Url::parse_absolute("data:text/html,hi").unwrap();
        assert!(data.join("x.html").is_none());
        assert!(data.join("http://a/").is_some());
        let root = Url::parse_absolute("http://a").unwrap();
        assert_eq!(root.join("x").unwrap().to_string(), "http://a/x");
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("a%20b%C3%B1%zz%4"), "a bñ%zz%4");
        assert_eq!(percent_decode("plain"), "plain");
    }

    #[test]
    fn without_fragment() {
        let u = Url::parse_absolute("http://a/b#c").unwrap();
        assert_eq!(u.without_fragment().to_string(), "http://a/b");
    }

    #[test]
    fn join_on_non_special_base_with_path() {
        let base = Url::parse_absolute("x:/").unwrap();
        let u = base.join("/static/client/a.svg?v=1#f").unwrap();
        assert_eq!(u.to_string(), "x:/static/client/a.svg?v=1#f");
        assert_eq!(u.path, "/static/client/a.svg");
        let opaque = Url::parse_absolute("mailto:a@b.c").unwrap();
        assert!(opaque.join("x").is_none());
    }
}
