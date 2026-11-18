//! `String.prototype.toLocaleLowerCase` / `toLocaleUpperCase` (ECMA-402
//! §19.1.2-3): TransformCase with the language-sensitive SpecialCasing
//! rules of Turkish, Azeri and Lithuanian.

use super::*;
use crate::regex::has_property;

/// Combining class 230 (Above), approximated by the above-base marks.
fn is_above(c: u32) -> bool {
    matches!(c, 0x0300..=0x0314 | 0x033D..=0x0344 | 0x0346 | 0x034A..=0x034C | 0x0350..=0x0352 | 0x0357 | 0x035B | 0x0363..=0x036F | 0x0483..=0x0487 | 0x0592..=0x0595 | 0x0597..=0x0599 | 0x059C..=0x05A1 | 0x0610..=0x0617 | 0x0653..=0x0654 | 0x1DC0..=0x1DC1 | 0x1DC3..=0x1DC9 | 0x20D0..=0x20D1 | 0x20D4..=0x20D7 | 0x20DB..=0x20DC | 0x20E1 | 0x20E7 | 0x20E9 | 0xFE20..=0xFE26 | 0x1D185..=0x1D189 | 0x1D1AA..=0x1D1AD | 0x1D242..=0x1D244)
}

/// A combining mark (non-zero combining class).
fn is_mark(c: u32) -> bool {
    has_property(c, "Mn") || has_property(c, "Me")
}

fn transform(s: &[char], lang: &str, upper: bool) -> String {
    let mut out = String::with_capacity(s.len());
    let n = s.len();
    for (i, &ch) in s.iter().enumerate() {
        let c = ch as u32;
        // Marks after position i until the next base character.
        let following = || s[i + 1..].iter().map(|&x| x as u32).take_while(|&x| is_mark(x));
        let more_above = || following().any(is_above);
        let before_dot = || following().take_while(|&x| !is_above(x) || x == 0x0307).any(|x| x == 0x0307);
        let after = |test: &dyn Fn(u32) -> bool| -> bool {
            let mut k = i;
            while k > 0 {
                k -= 1;
                let p = s[k] as u32;
                if test(p) {
                    return true;
                }
                if !is_mark(p) || is_above(p) {
                    return false;
                }
            }
            false
        };
        match (lang, upper) {
            ("tr" | "az", false) => {
                match c {
                    0x0130 => out.push('i'),
                    0x0307 if after(&|p| p == 0x0049) => {}
                    0x0049 if !before_dot() => out.push('\u{131}'),
                    0x0049 => out.push('i'),
                    _ => out.extend(ch.to_lowercase()),
                }
                continue;
            }
            ("tr" | "az", true) => {
                if c == 0x0069 {
                    out.push('\u{130}');
                } else {
                    out.extend(ch.to_uppercase());
                }
                continue;
            }
            ("lt", false) => {
                match c {
                    0x0049 if more_above() => out.push_str("i\u{307}"),
                    0x004A if more_above() => out.push_str("j\u{307}"),
                    0x012E if more_above() => out.push_str("\u{12F}\u{307}"),
                    0x00CC => out.push_str("i\u{307}\u{300}"),
                    0x00CD => out.push_str("i\u{307}\u{301}"),
                    0x0128 => out.push_str("i\u{307}\u{303}"),
                    _ => out.extend(ch.to_lowercase()),
                }
                continue;
            }
            ("lt", true) => {
                if c == 0x0307 && after(&|p| has_property(p, "Soft_Dotted")) {
                    continue;
                }
                out.extend(ch.to_uppercase());
                continue;
            }
            _ => {}
        }
        let _ = n;
        if upper {
            out.extend(ch.to_uppercase());
        } else {
            out.push_str(&ch.to_string().to_lowercase());
        }
    }
    out
}

fn transform_case(realm: &mut Realm, this: Value, args: &[Value], upper: bool) -> JsResult<Value> {
    if this.is_nullish() {
        return realm.type_error("String.prototype.toLocaleLowerCase called on null or undefined");
    }
    let s = realm.to_string(this)?;
    let requested = canonical_locale_list(realm, arg(args, 0))?;
    let lang = requested.first().and_then(|t| Locale::parse(t)).map(|l| l.lang).unwrap_or_else(|| "en".to_string());
    if !matches!(lang.as_str(), "tr" | "az" | "lt") {
        return Ok(Value::String(if upper { s.to_upper() } else { s.to_lower() }));
    }
    // Lone surrogates are kept: map the well-formed runs.
    let units = s.as_units();
    let mut out: Vec<u16> = Vec::with_capacity(units.len());
    let mut run: Vec<char> = Vec::new();
    for r in char::decode_utf16(units.iter().copied()) {
        match r {
            Ok(c) => run.push(c),
            Err(e) => {
                out.extend(transform(&run, &lang, upper).encode_utf16());
                run.clear();
                out.push(e.unpaired_surrogate());
            }
        }
    }
    out.extend(transform(&run, &lang, upper).encode_utf16());
    Ok(Value::String(JsStr::from_units(out)))
}

pub(super) fn to_locale_lower_case(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    transform_case(realm, this, args, false)
}

pub(super) fn to_locale_upper_case(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    transform_case(realm, this, args, true)
}
