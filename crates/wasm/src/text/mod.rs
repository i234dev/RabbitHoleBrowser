//! The WebAssembly text format: an S-expression reader, the WAT module
//! encoder (text → binary) and the `.wast` script format.

pub mod num;
pub mod wast;
pub mod wat;

use crate::Error;

/// An S-expression node.
#[derive(Clone, Debug, PartialEq)]
pub enum Sx {
    /// Keyword, number or other reserved token.
    Atom(String),
    /// Identifier (`$name` or `$"name"`), without the `$`.
    Id(String),
    Str(Vec<u8>),
    List(Vec<Sx>),
}

impl Sx {
    pub fn atom(&self) -> Option<&str> {
        match self {
            Sx::Atom(a) => Some(a),
            _ => None,
        }
    }
    pub fn list(&self) -> Option<&[Sx]> {
        match self {
            Sx::List(l) => Some(l),
            _ => None,
        }
    }
    /// A list whose head is the keyword `kw`.
    pub fn is_list_of(&self, kw: &str) -> bool {
        matches!(self, Sx::List(l) if matches!(l.first(), Some(Sx::Atom(a)) if a == kw))
    }
    pub fn head(&self) -> Option<&str> {
        match self {
            Sx::List(l) => l.first().and_then(|h| h.atom()),
            _ => None,
        }
    }
}

pub fn err<T>(msg: impl Into<String>) -> Result<T, Error> {
    Err(Error::Malformed(msg.into()))
}

fn is_idchar(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"!#$%&'*+-./:<=>?@\\^_`|~".contains(&c)
}

/// Characters of reserved tokens (valid only inside annotations).
fn is_reserved(c: u8) -> bool {
    b",[]{}".contains(&c)
}

fn parse_string(src: &[u8], pos: &mut usize) -> Result<Vec<u8>, Error> {
    // at opening quote
    *pos += 1;
    let mut out = Vec::new();
    loop {
        let Some(&c) = src.get(*pos) else { return err("unclosed string") };
        *pos += 1;
        match c {
            b'"' => return Ok(out),
            b'\\' => {
                let Some(&e) = src.get(*pos) else { return err("unclosed string") };
                *pos += 1;
                match e {
                    b't' => out.push(b'\t'),
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b'"' => out.push(b'"'),
                    b'\'' => out.push(b'\''),
                    b'\\' => out.push(b'\\'),
                    b'u' => {
                        if src.get(*pos) != Some(&b'{') {
                            return err("illegal escape");
                        }
                        *pos += 1;
                        let mut v: u32 = 0;
                        let mut n = 0;
                        loop {
                            let Some(&h) = src.get(*pos) else { return err("unclosed string") };
                            *pos += 1;
                            if h == b'}' {
                                break;
                            }
                            if h == b'_' && n > 0 {
                                continue;
                            }
                            let d = (h as char).to_digit(16).ok_or_else(|| Error::Malformed("illegal escape".into()))?;
                            v = v.checked_mul(16).and_then(|v| v.checked_add(d)).ok_or_else(|| Error::Malformed("illegal escape".into()))?;
                            n += 1;
                        }
                        let Some(ch) = char::from_u32(v) else { return err("illegal escape") };
                        if n == 0 {
                            return err("illegal escape");
                        }
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    }
                    h1 if (h1 as char).is_ascii_hexdigit() => {
                        let Some(&h2) = src.get(*pos) else { return err("illegal escape") };
                        if !(h2 as char).is_ascii_hexdigit() {
                            return err("illegal escape");
                        }
                        *pos += 1;
                        let v = (h1 as char).to_digit(16).unwrap() * 16 + (h2 as char).to_digit(16).unwrap();
                        out.push(v as u8);
                    }
                    _ => return err("illegal escape"),
                }
            }
            b'\n' => return err("newline in string"),
            c if c < 0x20 || c == 0x7f => return err("illegal character in string"),
            c => out.push(c),
        }
    }
}

/// Read all S-expressions of a text (annotations `(@...)` are dropped).
pub fn parse_sexprs(text: &str) -> Result<Vec<Sx>, Error> {
    let src = text.as_bytes();
    let mut pos = 0;
    let mut stack: Vec<Vec<Sx>> = vec![Vec::new()];
    // Depth of annotation lists being skipped.
    let mut skip_depth: Vec<usize> = Vec::new();
    while pos < src.len() {
        let c = src[pos];
        match c {
            b' ' | b'\t' | b'\n' | b'\r' => pos += 1,
            b';' if src.get(pos + 1) == Some(&b';') => {
                while pos < src.len() && src[pos] != b'\n' && src[pos] != b'\r' {
                    pos += 1;
                }
            }
            b'(' if src.get(pos + 1) == Some(&b';') => {
                // Block comment (nested).
                let mut depth = 1;
                pos += 2;
                while depth > 0 {
                    if pos + 1 >= src.len() {
                        return err("unclosed comment");
                    }
                    if src[pos] == b'(' && src[pos + 1] == b';' {
                        depth += 1;
                        pos += 2;
                    } else if src[pos] == b';' && src[pos + 1] == b')' {
                        depth -= 1;
                        pos += 2;
                    } else {
                        pos += 1;
                    }
                }
            }
            b'(' => {
                pos += 1;
                if src.get(pos) == Some(&b'@') {
                    if skip_depth.is_empty() {
                        // The annotation id: idchars or a non-empty string.
                        match src.get(pos + 1) {
                            Some(&b'"') => {
                                let mut p2 = pos + 1;
                                let s = parse_string(src, &mut p2)?;
                                if s.is_empty() {
                                    return err("empty annotation id");
                                }
                                if std::str::from_utf8(&s).is_err() {
                                    return err("malformed UTF-8 encoding");
                                }
                            }
                            Some(&c) if is_idchar(c) => {}
                            _ => return err("empty annotation id"),
                        }
                    }
                    skip_depth.push(stack.len());
                }
                stack.push(Vec::new());
            }
            b')' => {
                pos += 1;
                if stack.len() <= 1 {
                    return err("unexpected )");
                }
                let l = stack.pop().unwrap();
                if skip_depth.last() == Some(&stack.len()) {
                    skip_depth.pop();
                    continue;
                }
                stack.last_mut().unwrap().push(Sx::List(l));
            }
            b'"' => {
                let s = parse_string(src, &mut pos)?;
                stack.last_mut().unwrap().push(Sx::Str(s));
                if let Some(&n) = src.get(pos) {
                    if (is_idchar(n) || n == b'"') && skip_depth.is_empty() {
                        return err("unknown operator");
                    }
                }
            }
            _ if is_idchar(c) => {
                let start = pos;
                if c == b'$' && src.get(pos + 1) == Some(&b'"') {
                    pos += 1;
                    let s = parse_string(src, &mut pos)?;
                    let Ok(s) = String::from_utf8(s) else { return err("malformed UTF-8 encoding") };
                    if s.is_empty() {
                        return err("empty identifier");
                    }
                    if let Some(&n) = src.get(pos) {
                        if (is_idchar(n) || n == b'"') && skip_depth.is_empty() {
                            return err("unknown operator");
                        }
                    }
                    stack.last_mut().unwrap().push(Sx::Id(s));
                    continue;
                }
                while pos < src.len() && is_idchar(src[pos]) {
                    pos += 1;
                }
                if let Some(&n) = src.get(pos) {
                    if n == b'"' && skip_depth.is_empty() {
                        return err("unknown operator");
                    }
                }
                let tok = std::str::from_utf8(&src[start..pos]).unwrap().to_string();
                if let Some(id) = tok.strip_prefix('$') {
                    if id.is_empty() {
                        return err("empty identifier");
                    }
                    stack.last_mut().unwrap().push(Sx::Id(id.to_string()));
                } else {
                    stack.last_mut().unwrap().push(Sx::Atom(tok));
                }
            }
            _ if (is_reserved(c) || c == b';') && !skip_depth.is_empty() => {
                // Reserved token inside an annotation: skipped.
                pos += 1;
                while pos < src.len() && (is_idchar(src[pos]) || is_reserved(src[pos]) || (src[pos] == b';' && src.get(pos + 1) != Some(&b';'))) {
                    pos += 1;
                }
            }
            _ => return err(format!("unexpected character {:?}", c as char)),
        }
    }
    if stack.len() != 1 {
        return err("unclosed (");
    }
    Ok(stack.pop().unwrap())
}

/// Convert a text module (`(module ...)` or bare module fields) to binary.
pub fn wat_to_binary(text: &str) -> Result<Vec<u8>, Error> {
    let sx = parse_sexprs(text)?;
    if sx.len() == 1 && sx[0].is_list_of("module") {
        let l = sx[0].list().unwrap();
        let mut i = 1;
        if matches!(l.get(1), Some(Sx::Id(_))) {
            i = 2;
        }
        if matches!(l.get(i), Some(Sx::Atom(a)) if a == "binary") {
            let mut out = Vec::new();
            for s in &l[i + 1..] {
                match s {
                    Sx::Str(b) => out.extend_from_slice(b),
                    _ => return err("expected string"),
                }
            }
            return Ok(out);
        }
        return wat::encode_module(&l[i..]);
    }
    wat::encode_module(&sx)
}
