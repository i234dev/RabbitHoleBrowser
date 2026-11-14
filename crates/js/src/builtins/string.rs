//! String constructor and String.prototype.

use super::*;
use crate::value::V;
use crate::realm::{SYM_ITERATOR, SYM_MATCH, SYM_MATCH_ALL, SYM_REPLACE, SYM_SEARCH, SYM_SPLIT};

mod normalization;

pub fn register(realm: &mut Realm) {
    let sp = realm.intrinsics.string_proto;
    let ctor = define_ctor(realm, "String", 1, string_ctor, sp);
    realm.define_method(ctor, "fromCharCode", 1, from_char_code);
    realm.define_method(ctor, "fromCodePoint", 1, from_code_point);
    realm.define_method(ctor, "raw", 1, raw);
    let methods: [(&str, u32, NativeFn); 36] = [
        ("at", 1, at),
        ("charAt", 1, char_at),
        ("charCodeAt", 1, char_code_at),
        ("codePointAt", 1, code_point_at),
        ("concat", 1, concat),
        ("endsWith", 1, ends_with),
        ("includes", 1, includes),
        ("indexOf", 1, index_of),
        ("lastIndexOf", 1, last_index_of),
        ("localeCompare", 1, locale_compare),
        ("match", 1, match_),
        ("matchAll", 1, match_all),
        ("normalize", 0, normalize),
        ("padEnd", 1, pad_end),
        ("padStart", 1, pad_start),
        ("repeat", 1, repeat),
        ("replace", 2, replace),
        ("replaceAll", 2, replace_all),
        ("search", 1, search),
        ("slice", 2, slice),
        ("split", 2, split),
        ("startsWith", 1, starts_with),
        ("substring", 2, substring),
        ("substr", 2, substr),
        ("toLowerCase", 0, to_lower_case),
        ("toLocaleLowerCase", 0, to_lower_case),
        ("toUpperCase", 0, to_upper_case),
        ("toLocaleUpperCase", 0, to_upper_case),
        ("toString", 0, to_string),
        ("trim", 0, trim),
        ("trimStart", 0, trim_start),
        ("trimEnd", 0, trim_end),
        ("valueOf", 0, to_string),
        ("isWellFormed", 0, is_well_formed),
        ("toWellFormed", 0, to_well_formed),
        ("anchor", 1, anchor),
    ];
    for (name, arity, f) in methods {
        realm.define_method(sp, name, arity, f);
    }
    // Annex B HTML methods.
    let html: [(&str, u32, NativeFn); 12] = [
        ("big", 0, html_big),
        ("blink", 0, html_blink),
        ("bold", 0, html_bold),
        ("fixed", 0, html_fixed),
        ("fontcolor", 1, html_fontcolor),
        ("fontsize", 1, html_fontsize),
        ("italics", 0, html_italics),
        ("link", 1, html_link),
        ("small", 0, html_small),
        ("strike", 0, html_strike),
        ("sub", 0, html_sub),
        ("sup", 0, html_sup),
    ];
    for (name, arity, f) in html {
        realm.define_method(sp, name, arity, f);
    }
    let trim_left = realm.obj(sp).props.get(&PropertyKey::str("trimStart")).unwrap().value.clone();
    realm.define_property(sp, "trimLeft", trim_left, HIDDEN);
    let trim_right = realm.obj(sp).props.get(&PropertyKey::str("trimEnd")).unwrap().value.clone();
    realm.define_property(sp, "trimRight", trim_right, HIDDEN);
    realm.define_symbol_method(sp, SYM_ITERATOR, "[Symbol.iterator]", 0, iterator);
}

fn string_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let s = if args.is_empty() {
        JsStr::empty()
    } else {
        match args[0].v() {
            V::Symbol(sym) if !realm.is_construct_call() => JsStr::new(&format!("Symbol({})", realm.symbol_description(sym))),
            v => realm.to_string(v.to_value())?,
        }
    };
    if realm.is_construct_call() {
        let default = realm.intrinsics.string_proto;
        let proto = realm.proto_from_new_target_checked(default)?;
        return Ok(Value::Object(realm.alloc(Object::new(Some(proto), ObjectKind::String(s)))));
    }
    Ok(Value::String(s))
}

fn from_char_code(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if let [a] = args {
        if let V::Number(n) = a.v() {
            // One code unit: shared strings for the first 256.
            return Ok(Value::String(JsStr::from_unit(to_uint32(n) as u16)));
        }
    }
    let mut units = Vec::with_capacity(args.len());
    for a in args {
        let n = realm.to_number(a.clone())?;
        units.push(to_uint32(n) as u16);
    }
    Ok(Value::String(JsStr::from_units(units)))
}

fn from_code_point(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let mut s = String::new();
    for a in args {
        let n = realm.to_number(a.clone())?;
        if n.fract() != 0.0 || !(0.0..=1114111.0).contains(&n) {
            return realm.range_error(&format!("Invalid code point {}", number_to_string(n)));
        }
        match char::from_u32(n as u32) {
            Some(c) => s.push(c),
            None => {
                // lone surrogate: build via UTF-16
                let mut units: Vec<u16> = s.encode_utf16().collect();
                units.push(n as u16);
                return Ok(Value::String(JsStr::from_units(units)));
            }
        }
    }
    Ok(Value::String(JsStr::new(&s)))
}

fn raw(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let strings = realm.to_object(arg(args, 0))?;
    let raw = realm.get_str(Value::Object(strings), "raw")?;
    let raw = realm.to_object(raw)?;
    let len = realm.length_of(Value::Object(raw))?;
    let mut out: Vec<u16> = Vec::new();
    for i in 0..len {
        let seg = realm.get(Value::Object(raw), PropertyKey::index(i as u32))?;
        let seg = realm.to_string(seg)?;
        out.extend_from_slice(seg.as_units());
        if i + 1 < len {
            if let Some(sub) = args.get(i + 1) {
                let s = realm.to_string(sub.clone())?;
                out.extend_from_slice(s.as_units());
            }
        }
    }
    Ok(Value::String(JsStr::from_units(out)))
}

/// RequireObjectCoercible(this) + ToString.
pub fn this_str(realm: &mut Realm, this: &Value, method: &str) -> JsResult<JsStr> {
    match this.v() {
        V::String(s) => Ok(s.clone()),
        V::Undefined | V::Null => realm.type_error(&format!("String.prototype.{} called on null or undefined", method)),
        other => realm.to_string(other.to_value()),
    }
}

fn at(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_str(realm, &this, "at")?;
    let n = to_integer(realm.to_number(arg(args, 0))?);
    let len = s.len() as f64;
    let k = if n < 0.0 { len + n } else { n };
    if k < 0.0 || k >= len {
        return Ok(Value::Undefined);
    }
    Ok(Value::String(s.slice(k as usize, k as usize + 1)))
}

fn char_at(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_str(realm, &this, "charAt")?;
    let n = to_integer(realm.to_number(arg(args, 0))?);
    if n < 0.0 || n >= s.len() as f64 {
        return Ok(Value::str(""));
    }
    Ok(Value::String(s.slice(n as usize, n as usize + 1)))
}

fn char_code_at(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_str(realm, &this, "charCodeAt")?;
    let n = to_integer(realm.to_number(arg(args, 0))?);
    if n < 0.0 || n >= s.len() as f64 {
        return Ok(Value::Number(f64::NAN));
    }
    Ok(Value::Number(s.as_units()[n as usize] as f64))
}

fn code_point_at(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_str(realm, &this, "codePointAt")?;
    let n = to_integer(realm.to_number(arg(args, 0))?);
    let u = s.as_units();
    if n < 0.0 || n >= u.len() as f64 {
        return Ok(Value::Undefined);
    }
    let i = n as usize;
    let first = u[i] as u32;
    if (0xD800..0xDC00).contains(&first) && i + 1 < u.len() {
        let second = u[i + 1] as u32;
        if (0xDC00..0xE000).contains(&second) {
            return Ok(Value::Number(((first - 0xD800) * 0x400 + (second - 0xDC00) + 0x10000) as f64));
        }
    }
    Ok(Value::Number(first as f64))
}

fn concat(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let mut s = this_str(realm, &this, "concat")?;
    for a in args {
        let t = realm.to_string(a.clone())?;
        s = s.concat(&t);
    }
    Ok(Value::String(s))
}

fn is_regexp(realm: &mut Realm, v: &Value) -> JsResult<bool> {
    if let V::Object(id) = v.v() {
        let m = realm.get(Value::Object(id), PropertyKey::Symbol(SYM_MATCH))?;
        if !m.is_undefined() {
            return Ok(m.truthy());
        }
        return Ok(matches!(realm.obj(id).kind, ObjectKind::RegExp(_)));
    }
    Ok(false)
}

fn ends_with(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_str(realm, &this, "endsWith")?;
    if is_regexp(realm, &arg(args, 0))? {
        return realm.type_error("First argument to String.prototype.endsWith must not be a regular expression");
    }
    let search = realm.to_string(arg(args, 0))?;
    let end = match arg(args, 1) {
        Value::Undefined => s.len(),
        v => (to_integer(realm.to_number(v)?).max(0.0) as usize).min(s.len()),
    };
    if search.len() > end {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(s.as_units()[end - search.len()..end] == *search.as_units()))
}

fn starts_with(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_str(realm, &this, "startsWith")?;
    if is_regexp(realm, &arg(args, 0))? {
        return realm.type_error("First argument to String.prototype.startsWith must not be a regular expression");
    }
    let search = realm.to_string(arg(args, 0))?;
    let start = (to_integer(realm.to_number(arg(args, 1))?).max(0.0) as usize).min(s.len());
    if start + search.len() > s.len() {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(s.as_units()[start..start + search.len()] == *search.as_units()))
}

fn includes(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_str(realm, &this, "includes")?;
    if is_regexp(realm, &arg(args, 0))? {
        return realm.type_error("First argument to String.prototype.includes must not be a regular expression");
    }
    let search = realm.to_string(arg(args, 0))?;
    let start = (to_integer(realm.to_number(arg(args, 1))?).max(0.0) as usize).min(s.len());
    Ok(Value::Bool(s.index_of(&search, start).is_some()))
}

fn index_of(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_str(realm, &this, "indexOf")?;
    let search = realm.to_string(arg(args, 0))?;
    let start = (to_integer(realm.to_number(arg(args, 1))?).max(0.0) as usize).min(s.len());
    Ok(Value::Number(s.index_of(&search, start).map(|i| i as f64).unwrap_or(-1.0)))
}

fn last_index_of(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_str(realm, &this, "lastIndexOf")?;
    let search = realm.to_string(arg(args, 0))?;
    let pos = realm.to_number(arg(args, 1))?;
    let from = if pos.is_nan() { s.len() } else { (to_integer(pos).max(0.0) as usize).min(s.len()) };
    Ok(Value::Number(s.last_index_of(&search, from).map(|i| i as f64).unwrap_or(-1.0)))
}

fn locale_compare(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_str(realm, &this, "localeCompare")?;
    let t = realm.to_string(arg(args, 0))?;
    let a = s.to_rust_string();
    let b = t.to_rust_string();
    // Case-insensitive primary comparison, then case as tie-break (approximates locale collation).
    let al = a.to_lowercase();
    let bl = b.to_lowercase();
    let o = al.cmp(&bl).then_with(|| b.cmp(&a));
    Ok(Value::Number(match o {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
        std::cmp::Ordering::Greater => 1.0,
    }))
}

fn match_(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    if this.is_nullish() {
        return realm.type_error("String.prototype.match called on null or undefined");
    }
    let pat = arg(args, 0);
    if pat.is_object() {
        if let Some(m) = realm.get_method(pat.clone(), PropertyKey::Symbol(SYM_MATCH))? {
            return realm.call(m, pat, &[this]);
        }
    }
    let s = realm.to_string(this)?;
    let rx = regexp::regexp_create_from(realm, pat, Value::Undefined)?;
    // Invoke(rx, @@match, « S ») (ECMA-262 §22.1.3.13).
    let m = realm.get(rx.clone(), PropertyKey::Symbol(SYM_MATCH))?;
    realm.call(m, rx, &[Value::String(s)])
}

fn match_all(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    if this.is_nullish() {
        return realm.type_error("String.prototype.matchAll called on null or undefined");
    }
    let pat = arg(args, 0);
    if pat.is_object() {
        if is_regexp(realm, &pat)? {
            let flags = realm.get_str(pat.clone(), "flags")?;
            let flags = realm.to_string(flags)?;
            if !flags.to_rust_string().contains('g') {
                return realm.type_error("String.prototype.matchAll called with a non-global RegExp argument");
            }
        }
        if let Some(m) = realm.get_method(pat.clone(), PropertyKey::Symbol(SYM_MATCH_ALL))? {
            return realm.call(m, pat, &[this]);
        }
    }
    let s = realm.to_string(this)?;
    let rx = regexp::regexp_create_from(realm, pat, Value::str("g"))?;
    let m = realm.get(rx.clone(), PropertyKey::Symbol(SYM_MATCH_ALL))?;
    realm.call(m, rx, &[Value::String(s)])
}

fn normalize(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_str(realm, &this, "normalize")?;
    let form = match arg(args, 0) {
        Value::Undefined => "NFC".to_string(),
        v => realm.to_rust_string(v)?,
    };
    if !matches!(form.as_str(), "NFC" | "NFD" | "NFKC" | "NFKD") {
        return realm.range_error("The normalization form should be one of NFC, NFD, NFKC, NFKD.");
    }
    if s.as_units().iter().all(|&unit| unit < 0x80) {
        return Ok(Value::String(s));
    }
    Ok(Value::String(JsStr::from_units(normalization::normalize(s.as_units(), &form))))
}

fn pad(realm: &mut Realm, this: Value, args: &[Value], at_start: bool) -> JsResult<Value> {
    let s = this_str(realm, &this, "padStart")?;
    let max = realm.to_length(arg(args, 0))?;
    if max <= s.len() {
        return Ok(Value::String(s));
    }
    let filler = match arg(args, 1) {
        Value::Undefined => JsStr::new(" "),
        v => realm.to_string(v)?,
    };
    if filler.is_empty() {
        return Ok(Value::String(s));
    }
    if max > crate::value::MAX_STRING_LEN {
        return realm.range_error("Invalid string length");
    }
    let fill_len = max - s.len();
    let mut fill: Vec<u16> = Vec::with_capacity(fill_len);
    while fill.len() < fill_len {
        fill.extend_from_slice(filler.as_units());
    }
    fill.truncate(fill_len);
    let mut out = Vec::with_capacity(max);
    if at_start {
        out.extend(fill);
        out.extend_from_slice(s.as_units());
    } else {
        out.extend_from_slice(s.as_units());
        out.extend(fill);
    }
    Ok(Value::String(JsStr::from_units(out)))
}

fn pad_start(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    pad(realm, this, args, true)
}
fn pad_end(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    pad(realm, this, args, false)
}

fn repeat(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_str(realm, &this, "repeat")?;
    let n = to_integer(realm.to_number(arg(args, 0))?);
    if n < 0.0 || n.is_infinite() {
        return realm.range_error("Invalid count value");
    }
    if (n as usize).saturating_mul(s.len()) > (1 << 29) {
        return realm.range_error("Invalid string length");
    }
    Ok(Value::String(s.repeat(n as usize)))
}

/// GetSubstitution: expand `$` patterns in a replacement template.
/// Append to a string under construction, failing with a RangeError (not
/// an allocation abort) past the maximum string length.
pub(crate) fn extend_checked(realm: &mut Realm, out: &mut Vec<u16>, units: &[u16]) -> JsResult<()> {
    if out.len() + units.len() > crate::value::MAX_STRING_LEN || out.try_reserve(units.len()).is_err() {
        return realm.range_error("Invalid string length");
    }
    out.extend_from_slice(units);
    Ok(())
}

pub fn get_substitution(realm: &mut Realm, matched: &JsStr, string: &JsStr, position: usize, captures: &[Value], named: &Value, replacement: &JsStr) -> JsResult<JsStr> {
    let r = replacement.as_units();
    let mut out: Vec<u16> = Vec::with_capacity(r.len());
    let m = captures.len();
    let mut i = 0;
    while i < r.len() {
        let c = r[i];
        if c == b'$' as u16 && i + 1 < r.len() {
            let n = r[i + 1];
            match n {
                x if x == b'$' as u16 => {
                    out.push(b'$' as u16);
                    i += 2;
                }
                x if x == b'&' as u16 => {
                    extend_checked(realm, &mut out, matched.as_units())?;
                    i += 2;
                }
                x if x == b'`' as u16 => {
                    extend_checked(realm, &mut out, &string.as_units()[..position.min(string.len())])?;
                    i += 2;
                }
                x if x == b'\'' as u16 => {
                    let tail = (position + matched.len()).min(string.len());
                    extend_checked(realm, &mut out, &string.as_units()[tail..])?;
                    i += 2;
                }
                x if (b'0' as u16..=b'9' as u16).contains(&x) => {
                    let d1 = (x - b'0' as u16) as usize;
                    let mut consumed = 2;
                    let mut idx = d1;
                    if i + 2 < r.len() && (b'0' as u16..=b'9' as u16).contains(&r[i + 2]) {
                        let d2 = (r[i + 2] - b'0' as u16) as usize;
                        let two = d1 * 10 + d2;
                        if two >= 1 && two <= m {
                            idx = two;
                            consumed = 3;
                        }
                    }
                    if idx >= 1 && idx <= m {
                        let cap = &captures[idx - 1];
                        if !cap.is_undefined() {
                            let cs = realm.to_string(cap.clone())?;
                            extend_checked(realm, &mut out, cs.as_units())?;
                        }
                        i += consumed;
                    } else {
                        out.push(c);
                        i += 1;
                    }
                }
                x if x == b'<' as u16 => {
                    if named.is_undefined() {
                        out.push(c);
                        i += 1;
                    } else {
                        // find '>'
                        let close = r[i + 2..].iter().position(|&u| u == b'>' as u16);
                        match close {
                            Some(off) => {
                                let name = JsStr::from_slice(&r[i + 2..i + 2 + off]);
                                let v = realm.get(named.clone(), PropertyKey::String(name))?;
                                if !v.is_undefined() {
                                    let vs = realm.to_string(v)?;
                                    extend_checked(realm, &mut out, vs.as_units())?;
                                }
                                i += 3 + off;
                            }
                            None => {
                                out.push(c);
                                i += 1;
                            }
                        }
                    }
                }
                _ => {
                    out.push(c);
                    i += 1;
                }
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    Ok(JsStr::from_units(out))
}

fn replace(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    if this.is_nullish() {
        return realm.type_error("String.prototype.replace called on null or undefined");
    }
    let pat = arg(args, 0);
    let rep = arg(args, 1);
    if pat.is_object() {
        if let Some(m) = realm.get_method(pat.clone(), PropertyKey::Symbol(SYM_REPLACE))? {
            return realm.call(m, pat, &[this, rep]);
        }
    }
    let s = realm.to_string(this)?;
    let search = realm.to_string(pat)?;
    let functional = realm.is_callable(&rep);
    let rep_str = if functional { JsStr::empty() } else { realm.to_string(rep.clone())? };
    let pos = match s.index_of(&search, 0) {
        Some(p) => p,
        None => return Ok(Value::String(s)),
    };
    let replacement = if functional {
        let r = realm.call(rep, Value::Undefined, &[Value::String(search.clone()), Value::Number(pos as f64), Value::String(s.clone())])?;
        realm.to_string(r)?
    } else {
        get_substitution(realm, &search, &s, pos, &[], &Value::Undefined, &rep_str)?
    };
    let mut out: Vec<u16> = Vec::with_capacity(s.len() + replacement.len());
    out.extend_from_slice(&s.as_units()[..pos]);
    out.extend_from_slice(replacement.as_units());
    out.extend_from_slice(&s.as_units()[pos + search.len()..]);
    Ok(Value::String(JsStr::from_units(out)))
}

fn replace_all(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    if this.is_nullish() {
        return realm.type_error("String.prototype.replaceAll called on null or undefined");
    }
    let pat = arg(args, 0);
    let rep = arg(args, 1);
    if pat.is_object() {
        if is_regexp(realm, &pat)? {
            let flags = realm.get_str(pat.clone(), "flags")?;
            let flags = realm.to_string(flags)?;
            if !flags.to_rust_string().contains('g') {
                return realm.type_error("replaceAll must be called with a global RegExp");
            }
        }
        if let Some(m) = realm.get_method(pat.clone(), PropertyKey::Symbol(SYM_REPLACE))? {
            return realm.call(m, pat, &[this, rep]);
        }
    }
    let s = realm.to_string(this)?;
    let search = realm.to_string(pat)?;
    let functional = realm.is_callable(&rep);
    let rep_str = if functional { JsStr::empty() } else { realm.to_string(rep.clone())? };
    let advance = search.len().max(1);
    let mut positions = Vec::new();
    let mut p = s.index_of(&search, 0);
    while let Some(pos) = p {
        positions.push(pos);
        p = s.index_of(&search, pos + advance);
    }
    let mut out: Vec<u16> = Vec::new();
    let mut end_of_last = 0;
    for pos in positions {
        let replacement = if functional {
            let r = realm.call(rep.clone(), Value::Undefined, &[Value::String(search.clone()), Value::Number(pos as f64), Value::String(s.clone())])?;
            realm.to_string(r)?
        } else {
            get_substitution(realm, &search, &s, pos, &[], &Value::Undefined, &rep_str)?
        };
        out.extend_from_slice(&s.as_units()[end_of_last..pos]);
        out.extend_from_slice(replacement.as_units());
        end_of_last = pos + search.len();
    }
    out.extend_from_slice(&s.as_units()[end_of_last.min(s.len())..]);
    Ok(Value::String(JsStr::from_units(out)))
}

fn search(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    if this.is_nullish() {
        return realm.type_error("String.prototype.search called on null or undefined");
    }
    let pat = arg(args, 0);
    if pat.is_object() {
        if let Some(m) = realm.get_method(pat.clone(), PropertyKey::Symbol(SYM_SEARCH))? {
            return realm.call(m, pat, &[this]);
        }
    }
    let s = realm.to_string(this)?;
    let rx = regexp::regexp_create_from(realm, pat, Value::Undefined)?;
    let m = realm.get(rx.clone(), PropertyKey::Symbol(SYM_SEARCH))?;
    realm.call(m, rx, &[Value::String(s)])
}

fn slice(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_str(realm, &this, "slice")?;
    let len = s.len();
    let start = realm.relative_index(arg(args, 0), len, 0)?;
    let end = realm.relative_index(arg(args, 1), len, len)?;
    if start >= end {
        return Ok(Value::str(""));
    }
    Ok(Value::String(s.slice(start, end)))
}

fn split(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    if this.is_nullish() {
        return realm.type_error("String.prototype.split called on null or undefined");
    }
    let sep = arg(args, 0);
    let limit = arg(args, 1);
    if sep.is_object() {
        if let Some(m) = realm.get_method(sep.clone(), PropertyKey::Symbol(SYM_SPLIT))? {
            return realm.call(m, sep, &[this, limit]);
        }
    }
    let s = realm.to_string(this)?;
    let lim = if limit.is_undefined() { u32::MAX as usize } else { realm.to_uint32(limit)? as usize };
    let sep_s = realm.to_string(sep.clone())?;
    if lim == 0 {
        return Ok(realm.new_array_value(vec![]));
    }
    if sep.is_undefined() {
        return Ok(realm.new_array_value(vec![Value::String(s)]));
    }
    let mut parts = Vec::new();
    if sep_s.is_empty() {
        for i in 0..s.len().min(lim) {
            parts.push(Value::String(s.slice(i, i + 1)));
        }
        return Ok(realm.new_array_value(parts));
    }
    let mut start = 0;
    while let Some(p) = s.index_of(&sep_s, start) {
        parts.push(Value::String(s.slice(start, p)));
        if parts.len() >= lim {
            return Ok(realm.new_array_value(parts));
        }
        start = p + sep_s.len();
    }
    parts.push(Value::String(s.slice(start, s.len())));
    parts.truncate(lim);
    Ok(realm.new_array_value(parts))
}

fn substring(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_str(realm, &this, "substring")?;
    let len = s.len() as f64;
    let a = to_integer(realm.to_number(arg(args, 0))?).clamp(0.0, len);
    let b = match arg(args, 1) {
        Value::Undefined => len,
        v => to_integer(realm.to_number(v)?).clamp(0.0, len),
    };
    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
    Ok(Value::String(s.slice(lo as usize, hi as usize)))
}

fn substr(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let s = this_str(realm, &this, "substr")?;
    let len = s.len();
    let start = realm.relative_index(arg(args, 0), len, 0)?;
    let count = match arg(args, 1) {
        Value::Undefined => len - start,
        v => (to_integer(realm.to_number(v)?).max(0.0) as usize).min(len - start),
    };
    Ok(Value::String(s.slice(start, start + count)))
}

fn to_lower_case(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let s = this_str(realm, &this, "toLowerCase")?;
    Ok(Value::String(s.to_lower()))
}

fn to_upper_case(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let s = this_str(realm, &this, "toUpperCase")?;
    Ok(Value::String(s.to_upper()))
}

fn to_string(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    match this.v() {
        V::String(s) => Ok(Value::String(s.clone())),
        V::Object(id) => match &realm.obj(id).kind {
            ObjectKind::String(s) => Ok(Value::String(s.clone())),
            _ => realm.type_error("String.prototype.toString requires that 'this' be a String"),
        },
        _ => realm.type_error("String.prototype.toString requires that 'this' be a String"),
    }
}

fn trim(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let s = this_str(realm, &this, "trim")?;
    Ok(Value::String(s.trim_js(true, true)))
}
fn trim_start(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let s = this_str(realm, &this, "trimStart")?;
    Ok(Value::String(s.trim_js(true, false)))
}
fn trim_end(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let s = this_str(realm, &this, "trimEnd")?;
    Ok(Value::String(s.trim_js(false, true)))
}

fn is_well_formed(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let s = this_str(realm, &this, "isWellFormed")?;
    Ok(Value::Bool(s.chars_lossy().all(|c| c.is_ok())))
}

fn to_well_formed(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let s = this_str(realm, &this, "toWellFormed")?;
    Ok(Value::String(JsStr::new(&s.to_rust_string())))
}

/// CreateHTML (Annex B.2.2.2.1).
fn create_html(realm: &mut Realm, this: Value, tag: &str, attr: Option<(&str, Value)>) -> JsResult<Value> {
    if this.is_nullish() {
        return realm.type_error(&format!("String.prototype.{} called on null or undefined", tag));
    }
    let s = realm.to_string(this)?;
    let mut out: Vec<u16> = Vec::with_capacity(s.len() + 16);
    out.push(b'<' as u16);
    out.extend(tag.encode_utf16());
    if let Some((name, v)) = attr {
        let v = realm.to_string(v)?;
        out.push(b' ' as u16);
        out.extend(name.encode_utf16());
        out.extend("=\"".encode_utf16());
        for &u in v.as_units() {
            if u == b'"' as u16 {
                out.extend("&quot;".encode_utf16());
            } else {
                out.push(u);
            }
        }
        out.push(b'"' as u16);
    }
    out.push(b'>' as u16);
    out.extend_from_slice(s.as_units());
    out.extend("</".encode_utf16());
    out.extend(tag.encode_utf16());
    out.push(b'>' as u16);
    Ok(Value::String(JsStr::from_units(out)))
}

fn anchor(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    create_html(realm, this, "a", Some(("name", arg(args, 0))))
}
fn html_big(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    create_html(realm, this, "big", None)
}
fn html_blink(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    create_html(realm, this, "blink", None)
}
fn html_bold(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    create_html(realm, this, "b", None)
}
fn html_fixed(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    create_html(realm, this, "tt", None)
}
fn html_fontcolor(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    create_html(realm, this, "font", Some(("color", arg(args, 0))))
}
fn html_fontsize(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    create_html(realm, this, "font", Some(("size", arg(args, 0))))
}
fn html_italics(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    create_html(realm, this, "i", None)
}
fn html_link(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    create_html(realm, this, "a", Some(("href", arg(args, 0))))
}
fn html_small(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    create_html(realm, this, "small", None)
}
fn html_strike(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    create_html(realm, this, "strike", None)
}
fn html_sub(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    create_html(realm, this, "sub", None)
}
fn html_sup(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    create_html(realm, this, "sup", None)
}

fn iterator(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let s = this_str(realm, &this, "[Symbol.iterator]")?;
    Ok(iterator::create_string_iterator(realm, s))
}

#[cfg(test)]
mod normalization_tests {
    use crate::Realm;

    #[test]
    fn normalization_coerces_receiver_before_form_and_preserves_utf16() {
        let mut realm = Realm::new();
        let result = realm.eval(r#"
            var log = [];
            var boxed = new String('ignored');
            boxed[Symbol.toPrimitive] = function(hint) { log.push(hint); return 'A\u030A'; };
            var form = {toString() { log.push('form'); return 'NFC'; }};
            var good = String.prototype.normalize.call(boxed, form) === '\u00C5'
                && log.join(',') === 'string,form'
                && '\uD800A\u030A\uDC00'.normalize() === '\uD800\u00C5\uDC00'
                && '\u1100\u1161\u11A8'.normalize() === '\uAC01'
                && '\uFB03'.normalize('NFKC') === 'ffi';
            boxed.toString = function() { return 'overridden'; };
            delete boxed[Symbol.toPrimitive];
            good = good && boxed.normalize() === 'overridden' && boxed.charAt(0) === 'o';
            var sentinel = {};
            boxed.toString = function() { throw sentinel; };
            try { boxed.normalize(form); good = false; }
            catch (e) { if (e !== sentinel) good = false; }
            good && log.join(',') === 'string,form'
        "#, "normalize-coercion.js").unwrap_or_else(|e| panic!("{}", realm.error_to_string(&e)));
        assert_eq!(result.as_bool(), Some(true));
    }

    #[test]
    fn ascii_fast_path_still_coerces_and_validates_form() {
        let mut realm = Realm::new();
        let result = realm.eval(r#"
            var calls = 0, good = true;
            for (var str of ['', 'ascii']) {
                good = good && str.normalize({toString() { calls++; return 'NFD'; }}) === str;
                try { str.normalize('invalid'); good = false; }
                catch (e) { if (!(e instanceof RangeError)) good = false; }
                try { str.normalize(Symbol()); good = false; }
                catch (e) { if (!(e instanceof TypeError)) good = false; }
            }
            good && calls === 2
        "#, "normalize-ascii.js").unwrap_or_else(|e| panic!("{}", realm.error_to_string(&e)));
        assert_eq!(result.as_bool(), Some(true));
    }
}
