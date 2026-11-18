//! A small `Intl` implementation: `DateTimeFormat`, `NumberFormat`,
//! `Collator`, `PluralRules`, `RelativeTimeFormat`, `ListFormat` and the
//! locale-sensitive `toLocaleString`/`localeCompare` methods.
//!
//! Locale data is built in for English (US and other regions), Spanish,
//! German, French, Italian, Portuguese, Dutch and East Asian numerics;
//! other locales fall back to English conventions. The default locale is
//! `en-US`.

use crate::value::V;
use super::*;
use crate::realm::SYM_TO_STRING_TAG;

mod date_time_format;
mod duration;
mod number_format;
mod plural_rules;
pub use number_format::{format_bigint_parts, format_number, format_number_parts, number_to_locale_string, read_nf_options, NfOptions, NumberParts};
#[allow(unused_imports)]
use number_format::number_arg;
pub mod locale_id;
mod locale_obj;
mod list_format;
mod relative_time;
mod segmenter;
mod display_names;
mod case_mapping;
pub use list_format::{format_list, ListOptions};
use list_format::list_parts;

pub use date_time_format::{date_to_locale, DateDefaults};

// ------------------------------------------------------------- locales

#[derive(Debug, Clone, PartialEq)]
pub struct Locale {
    /// The base name (language-script-region-variants), canonical.
    pub tag: String,
    pub lang: String,
    pub region: Option<String>,
    /// The `-u-nu-` (numbering system) extension value, if any.
    pub nu: Option<String>,
    /// The whole canonical tag, extensions included.
    pub full: String,
}

impl Locale {
    pub fn default_locale() -> Locale {
        Locale::parse("en-US").unwrap()
    }

    /// A structurally valid BCP 47 tag, canonicalized (UTS #35); None if
    /// malformed.
    pub fn parse(tag: &str) -> Option<Locale> {
        let mut id = locale_id::parse(tag)?;
        locale_id::canonicalize_id(&mut id);
        let nu = id.u_keywords.iter().find(|(k, _)| k == "nu").map(|(_, v)| v.clone()).filter(|v| !v.is_empty());
        Some(Locale {
            tag: locale_id::base_name(&id),
            lang: id.language.clone(),
            region: id.region.as_ref().map(|r| r.to_ascii_uppercase()),
            nu,
            full: locale_id::to_string(&id),
        })
    }
}

/// Languages with locale data here (LookupSupportedLocales' available set).
const AVAILABLE_LANGUAGES: &[&str] = &[
    "af", "am", "ar", "az", "be", "bg", "bn", "bs", "ca", "cs", "cy", "da", "de", "el", "en", "es", "et", "eu", "fa", "fi", "fil", "fr", "ga", "gl", "gu",
    "he", "hi", "hr", "hu", "hy", "id", "is", "it", "ja", "ka", "kk", "km", "kn", "ko", "ky", "lo", "lt", "lv", "mk", "ml", "mn", "mr", "ms", "my", "nb",
    "ne", "nl", "nn", "no", "pa", "pl", "pt", "ro", "ru", "si", "sk", "sl", "sq", "sr", "sv", "sw", "ta", "te", "th", "tr", "uk", "ur", "uz", "vi", "zh", "zu",
];

/// BestAvailableLocale: the requested locale (or a prefix of it) we have.
fn is_available(tag: &str) -> bool {
    let lang = tag.split('-').next().unwrap_or("");
    AVAILABLE_LANGUAGES.contains(&lang)
}

/// Resolve the `locales` argument (string, array of strings or undefined).
pub fn resolve_locale(realm: &mut Realm, v: Value) -> JsResult<Locale> {
    match v.v() {
        V::Undefined => Ok(Locale::default_locale()),
        V::String(s) => match Locale::parse(&s.to_rust_string()) {
            Some(l) => Ok(l),
            None => realm.range_error(&format!("Incorrect locale information provided: {}", s)),
        },
        V::Object(id) => {
            if realm.obj(id).is_array() {
                let items = realm.create_list_from_array_like(Value::Object(id))?;
                match items.into_iter().next() {
                    Some(first) => resolve_locale(realm, first),
                    None => Ok(Locale::default_locale()),
                }
            } else {
                let s = realm.to_rust_string(Value::Object(id))?;
                match Locale::parse(&s) {
                    Some(l) => Ok(l),
                    None => realm.range_error("Incorrect locale information provided"),
                }
            }
        }
        other => {
            let s = realm.to_rust_string(other.to_value())?;
            match Locale::parse(&s) {
                Some(l) => Ok(l),
                None => realm.range_error("Incorrect locale information provided"),
            }
        }
    }
}

/// CanonicalizeLocaleList.
fn canonical_locale_list(realm: &mut Realm, v: Value) -> JsResult<Vec<String>> {
    let mut out = Vec::new();
    let add = |realm: &mut Realm, item: Value, out: &mut Vec<String>| -> JsResult<()> {
        let tag = match item.v() {
            V::Object(id) => match realm.host_data::<locale_obj::LocaleObject>(id) {
                Some(l) => l.tag.clone(),
                _ => realm.to_rust_string(item.clone())?,
            },
            V::String(s) => s.to_rust_string(),
            _ => return realm.type_error("Language ID should be string or object."),
        };
        match Locale::parse(&tag) {
            Some(l) => {
                if !out.contains(&l.full) {
                    out.push(l.full);
                }
                Ok(())
            }
            None => realm.range_error(&format!("Incorrect locale information provided: {}", tag)),
        }
    };
    match v.v() {
        V::Undefined => return Ok(out),
        V::String(_) => {
            add(realm, v, &mut out)?;
            return Ok(out);
        }
        V::Object(id) if realm.host_data::<locale_obj::LocaleObject>(id).is_some() => {
            add(realm, v, &mut out)?;
            return Ok(out);
        }
        _ => {}
    }
    let o = realm.to_object(v)?;
    let len = realm.length_of(Value::Object(o))?;
    for k in 0..len {
        let key = PropertyKey::index(k as u32);
        if realm.has_property(o, &key)? {
            let item = realm.get(Value::Object(o), key)?;
            add(realm, item, &mut out)?;
        }
    }
    Ok(out)
}

/// ResolveLocale's locale part (lookup matcher): the first requested
/// locale with available data, else the default locale. Extensions are
/// left to each service.
pub(super) fn lookup_locale(realm: &mut Realm, locales: Value) -> JsResult<Locale> {
    let requested = canonical_locale_list(realm, locales)?;
    for t in &requested {
        if is_available(t) {
            if let Some(l) = Locale::parse(t) {
                return Ok(l);
            }
        }
    }
    Ok(Locale::default_locale())
}

// ---------------------------------------------------- numbering systems

/// Numbering systems with ten contiguous decimal digits (ECMA-402 table
/// "Numbering systems with simple digit mappings"): the code point of zero.
const NUMBERING_SYSTEMS: &[(&str, u32)] = &[
    ("adlm", 0x1E950), ("ahom", 0x11730), ("arab", 0x0660), ("arabext", 0x06F0), ("bali", 0x1B50),
    ("beng", 0x09E6), ("bhks", 0x11C50), ("brah", 0x11066), ("cakm", 0x11136), ("cham", 0xAA50),
    ("deva", 0x0966), ("diak", 0x11950), ("fullwide", 0xFF10), ("gara", 0x10D40), ("gong", 0x11DA0),
    ("gonm", 0x11D50), ("gujr", 0x0AE6), ("gukh", 0x16130), ("guru", 0x0A66), ("hmng", 0x16B50),
    ("hmnp", 0x1E140), ("java", 0xA9D0), ("kali", 0xA900), ("kawi", 0x11F50), ("khmr", 0x17E0),
    ("knda", 0x0CE6), ("krai", 0x16D70), ("lana", 0x1A80), ("lanatham", 0x1A90), ("laoo", 0x0ED0),
    ("latn", 0x0030), ("lepc", 0x1C40), ("limb", 0x1946), ("mathbold", 0x1D7CE), ("mathdbl", 0x1D7D8),
    ("mathmono", 0x1D7F6), ("mathsanb", 0x1D7EC), ("mathsans", 0x1D7E2), ("mlym", 0x0D66), ("modi", 0x11650),
    ("mong", 0x1810), ("mroo", 0x16A60), ("mtei", 0xABF0), ("mymr", 0x1040), ("mymrepka", 0x116DA),
    ("mymrpao", 0x116D0), ("mymrshan", 0x1090), ("mymrtlng", 0xA9F0), ("nagm", 0x1E4F0), ("newa", 0x11450),
    ("nkoo", 0x07C0), ("olck", 0x1C50), ("onao", 0x1E5F1), ("orya", 0x0B66), ("osma", 0x104A0),
    ("outlined", 0x1CCF0), ("rohg", 0x10D30), ("saur", 0xA8D0), ("segment", 0x1FBF0), ("shrd", 0x111D0),
    ("sind", 0x112F0), ("sinh", 0x0DE6), ("sora", 0x110F0), ("sund", 0x1BB0), ("sunu", 0x11BF0),
    ("takr", 0x116C0), ("talu", 0x19D0), ("tamldec", 0x0BE6), ("telu", 0x0C66), ("thai", 0x0E50),
    ("tibt", 0x0F20), ("tirh", 0x114D0), ("tnsa", 0x16AC0), ("tols", 0x11DE0), ("vaii", 0xA620),
    ("wara", 0x118E0), ("wcho", 0x1E2F0),
];

/// The zero digit of a supported numbering system.
pub(super) fn numbering_zero(name: &str) -> Option<u32> {
    if name == "hanidec" {
        return Some(0);
    }
    NUMBERING_SYSTEMS.iter().find(|(n, _)| *n == name).map(|(_, z)| *z)
}

/// Replace ASCII digits by the numbering system's digits.
fn transliterate_digits(s: &str, system: &str) -> String {
    if system == "latn" || system.is_empty() {
        return s.to_string();
    }
    const HANIDEC: [char; 10] = ['〇', '一', '二', '三', '四', '五', '六', '七', '八', '九'];
    let Some(zero) = numbering_zero(system) else { return s.to_string() };
    s.chars()
        .map(|c| match c.to_digit(10) {
            Some(d) if c.is_ascii_digit() => {
                if system == "hanidec" {
                    HANIDEC[d as usize]
                } else {
                    char::from_u32(zero + d).unwrap_or(c)
                }
            }
            _ => c,
        })
        .collect()
}

// ----------------------------------------------------------- options

fn options_object(realm: &mut Realm, v: Value) -> JsResult<Option<ObjectId>> {
    match v {
        Value::Undefined => Ok(None),
        Value::Null => realm.type_error("Cannot convert undefined or null to object"),
        other => Ok(Some(realm.to_object(other)?)),
    }
}

fn opt_str(realm: &mut Realm, opts: Option<ObjectId>, name: &str) -> JsResult<Option<String>> {
    let Some(o) = opts else { return Ok(None) };
    let v = realm.get_str(Value::Object(o), name)?;
    if v.is_undefined() {
        return Ok(None);
    }
    Ok(Some(realm.to_rust_string(v)?))
}




// ------------------------------------------------------------ Collator

#[derive(Debug, Clone)]
pub struct CollatorOptions {
    locale: Locale,
    sensitivity: String,
    numeric: bool,
    ignore_punctuation: bool,
    usage: String,
    collation: String,
    case_first: String,
}

/// Strip diacritics from Latin letters; returns (base, had_accent).
fn base_char(c: char) -> (char, bool) {
    let b = match c {
        'À'..='Å' | 'à'..='å' => Some(if c.is_uppercase() { 'A' } else { 'a' }),
        'Ç' => Some('C'),
        'ç' => Some('c'),
        'È'..='Ë' => Some('E'),
        'è'..='ë' => Some('e'),
        'Ì'..='Ï' => Some('I'),
        'ì'..='ï' => Some('i'),
        'Ñ' => Some('N'),
        'ñ' => Some('n'),
        'Ò'..='Ö' | 'Ø' => Some('O'),
        'ò'..='ö' | 'ø' => Some('o'),
        'Ù'..='Ü' => Some('U'),
        'ù'..='ü' => Some('u'),
        'Ý' => Some('Y'),
        'ý' | 'ÿ' => Some('y'),
        '\u{100}'..='\u{17f}' => {
            const BASES: &[u8] = b"AaAaAaCcCcCcCcDdDdEeEeEeEeEeGgGgGgGgHhHhIiIiIiIiIiJjJjKkkLlLlLlLlLlNnNnNnnNnOoOoOoOoRrRrRrSsSsSsSsTtTtTtUuUuUuUuUuUuWwYyYZzZzZzs";
            let i = c as usize - 0x100;
            BASES.get(i).map(|&b| b as char)
        }
        _ => None,
    };
    match b {
        Some(b) => (b, true),
        None => (c, false),
    }
}

fn collation_key(s: &str, o: &CollatorOptions) -> Vec<(u32, u8, u8)> {
    // (primary, accent, case) per character; digit runs become one key when numeric.
    let mut out = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if o.numeric && c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let digits: String = chars[start..i].iter().collect();
            let n: u64 = digits.trim_start_matches('0').parse().unwrap_or(0);
            out.push((0x3000_0000 + (n.min(0x0fff_ffff) as u32), 0, 0));
            continue;
        }
        i += 1;
        if o.ignore_punctuation && (c.is_ascii_punctuation() || c.is_whitespace()) {
            continue;
        }
        let (base, accented) = base_char(c);
        let lower = base.to_lowercase().next().unwrap_or(base);
        let is_upper = base != lower;
        // Letters sort after punctuation/digits; keep Unicode order within letters.
        let primary = lower as u32;
        out.push((primary, if accented { c as u32 as u8 | 1 } else { 0 }, if is_upper { 1 } else { 0 }));
        // German phonebook collation: ä, ö, ü sort as ae, oe, ue.
        if o.collation == "phonebk" && matches!(c, 'ä' | 'ö' | 'ü' | 'Ä' | 'Ö' | 'Ü') {
            out.push(('e' as u32, 0, 0));
        }
    }
    out
}

/// NFC form of `s`: canonically equivalent strings collate as equal.
fn nfc(s: &str) -> std::borrow::Cow<'_, str> {
    if s.is_ascii() {
        return std::borrow::Cow::Borrowed(s);
    }
    let units: Vec<u16> = s.encode_utf16().collect();
    let n = super::string::normalization::normalize(&units, "NFC");
    if n == units {
        std::borrow::Cow::Borrowed(s)
    } else {
        std::borrow::Cow::Owned(String::from_utf16_lossy(&n))
    }
}

pub fn collator_compare(a: &str, b: &str, o: &CollatorOptions) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (a, b) = (nfc(a), nfc(b));
    let (a, b) = (a.as_ref(), b.as_ref());
    let ka = collation_key(a, o);
    let kb = collation_key(b, o);
    let primary = ka.iter().map(|k| k.0).cmp(kb.iter().map(|k| k.0));
    if primary != Ordering::Equal {
        return primary;
    }
    let sens = o.sensitivity.as_str();
    if sens == "accent" || sens == "variant" {
        let acc = ka.iter().map(|k| k.1).cmp(kb.iter().map(|k| k.1));
        if acc != Ordering::Equal {
            return acc;
        }
    }
    if sens == "case" || sens == "variant" {
        // Key 1 marks an upper-case letter: lower case sorts first unless
        // caseFirst is "upper".
        let case = ka.iter().map(|k| k.2).cmp(kb.iter().map(|k| k.2));
        if case != Ordering::Equal {
            return if o.case_first == "upper" { case.reverse() } else { case };
        }
    }
    if sens == "variant" {
        return a.cmp(b);
    }
    Ordering::Equal
}

/// InitializeCollator: options in specification order, the `co`, `kf`
/// and `kn` keys resolved against the locale's Unicode extension.
fn read_collator_options(realm: &mut Realm, locales: Value, options: Value) -> JsResult<CollatorOptions> {
    let mut locale = lookup_locale(realm, locales)?;
    let opts = if options.is_undefined() { None } else { Some(realm.to_object(options)?) };
    let usage = duration::get_string_option(realm, opts, "usage", &["sort", "search"], Some("sort"))?.unwrap();
    duration::get_string_option(realm, opts, "localeMatcher", &["lookup", "best fit"], Some("best fit"))?;
    let collation_opt = duration::get_string_option(realm, opts, "collation", &[], None)?;
    if let Some(c) = &collation_opt {
        if !duration::is_unicode_type(c) {
            return realm.range_error(&format!("Invalid collation: {}", c));
        }
    }
    let numeric_opt = match opts {
        Some(o) => {
            let v = realm.get_str(Value::Object(o), "numeric")?;
            if v.is_undefined() { None } else { Some(if realm.to_boolean(&v) { "true" } else { "false" }.to_string()) }
        }
        None => None,
    };
    let case_first_opt = duration::get_string_option(realm, opts, "caseFirst", &["upper", "lower", "false"], None)?;
    // ResolveLocale for the relevant extension keys.
    let ext: Vec<(String, String)> = locale_id::parse(&locale_full_requested(&locale)).map(|id| id.u_keywords).unwrap_or_default();
    let ext_value = |k: &str| ext.iter().find(|(key, _)| key == k).map(|(_, v)| if v.is_empty() { "true".to_string() } else { v.clone() });
    let supported_co = |c: &str| matches!((locale.lang.as_str(), c), ("de", "phonebk") | ("de", "eor") | ("es", "trad") | ("zh", "pinyin") | ("zh", "stroke") | (_, "emoji") | (_, "eor"));
    let mut used: Vec<(&str, String)> = Vec::new();
    let mut resolve = |key: &'static str, opt: &Option<String>, ok: &dyn Fn(&str) -> bool| -> Option<String> {
        let from_ext = ext_value(key).filter(|v| ok(v));
        match (opt, &from_ext) {
            (Some(o), Some(e)) if o == e => {
                used.push((key, e.clone()));
                Some(o.clone())
            }
            (Some(o), _) if ok(o) => Some(o.clone()),
            (Some(_), Some(e)) => {
                used.push((key, e.clone()));
                Some(e.clone())
            }
            (Some(_), None) => None,
            (None, Some(e)) => {
                used.push((key, e.clone()));
                Some(e.clone())
            }
            (None, None) => None,
        }
    };
    let collation = if usage == "search" { None } else { resolve("co", &collation_opt, &|c| supported_co(c)) };
    let numeric = resolve("kn", &numeric_opt, &|v| matches!(v, "true" | "false"));
    let case_first = resolve("kf", &case_first_opt, &|v| matches!(v, "upper" | "lower" | "false"));
    if !used.is_empty() {
        used.sort_by(|a, b| a.0.cmp(b.0));
        let kw: Vec<String> = used.iter().map(|(k, v)| if v == "true" { k.to_string() } else { format!("{}-{}", k, v) }).collect();
        locale.tag = format!("{}-u-{}", locale.tag, kw.join("-"));
    }
    let sensitivity = duration::get_string_option(realm, opts, "sensitivity", &["base", "accent", "case", "variant"], Some("variant"))?.unwrap();
    let ignore_punctuation = match opts {
        Some(o) => {
            let v = realm.get_str(Value::Object(o), "ignorePunctuation")?;
            if v.is_undefined() { locale.lang == "th" } else { realm.to_boolean(&v) }
        }
        None => locale.lang == "th",
    };
    Ok(CollatorOptions {
        numeric: numeric.as_deref() == Some("true"),
        ignore_punctuation,
        locale,
        sensitivity,
        usage,
        collation: collation.unwrap_or_else(|| "default".into()),
        case_first: case_first.unwrap_or_else(|| "false".into()),
    })
}

/// The requested tag (with extensions) a lookup resolved to.
fn locale_full_requested(l: &Locale) -> String {
    l.full.clone()
}

/// `String.prototype.localeCompare(that, locales, options)`.
pub fn locale_compare(realm: &mut Realm, a: &str, b: &str, args: &[Value]) -> JsResult<Value> {
    let o = read_collator_options(realm, arg(args, 1), arg(args, 2))?;
    Ok(Value::Number(match collator_compare(a, b, &o) {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
        std::cmp::Ordering::Greater => 1.0,
    }))
}

// ---------------------------------------------------------- PluralRules

pub fn plural_category(n: f64, lang: &str, ordinal: bool) -> &'static str {
    if ordinal {
        return match lang {
            "en" => {
                let i = n.abs() as i64;
                if n.fract() != 0.0 {
                    "other"
                } else if i % 10 == 1 && i % 100 != 11 {
                    "one"
                } else if i % 10 == 2 && i % 100 != 12 {
                    "two"
                } else if i % 10 == 3 && i % 100 != 13 {
                    "few"
                } else {
                    "other"
                }
            }
            _ => "other",
        };
    }
    let is_int = n.fract() == 0.0;
    match lang {
        "fr" | "pt" => {
            if n.abs() < 2.0 {
                "one"
            } else {
                "other"
            }
        }
        "ru" | "uk" | "pl" | "cs" | "sk" | "hr" | "sr" => {
            if !is_int {
                return "other";
            }
            let i = n.abs() as i64;
            if i % 10 == 1 && i % 100 != 11 {
                "one"
            } else if (2..=4).contains(&(i % 10)) && !(12..=14).contains(&(i % 100)) {
                "few"
            } else {
                "many"
            }
        }
        "ar" => {
            let i = n.abs() as i64;
            if n == 0.0 {
                "zero"
            } else if n == 1.0 {
                "one"
            } else if n == 2.0 {
                "two"
            } else if is_int && (3..=10).contains(&(i % 100)) {
                "few"
            } else if is_int && (11..=99).contains(&(i % 100)) {
                "many"
            } else {
                "other"
            }
        }
        "ja" | "zh" | "ko" | "th" | "vi" | "id" | "ms" | "tr" => "other",
        _ => {
            if n == 1.0 && is_int {
                "one"
            } else {
                "other"
            }
        }
    }
}

// --------------------------------------------------------- registration

pub fn register(realm: &mut Realm) {
    let op = realm.intrinsics.object_proto;
    let intl = realm.new_object_with_proto(Some(op));
    realm.define_global("Intl", Value::Object(intl));
    realm.define_property(intl, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("Intl"), CONFIGURABLE);
    realm.define_method(intl, "getCanonicalLocales", 1, get_canonical_locales);
    realm.define_method(intl, "supportedValuesOf", 1, supported_values_of);

    let mk = |realm: &mut Realm, name: &str, tag: &'static str, ctor: NativeFn, arity: u32| -> (ObjectId, ObjectId) {
        let proto = realm.new_object_with_proto(Some(op));
        let c = realm.new_native_constructor(name, arity, ctor, proto);
        realm.define_property(intl, name, Value::Object(c), HIDDEN);
        realm.define_property(proto, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str(tag), CONFIGURABLE);
        realm.define_method(c, "supportedLocalesOf", 1, supported_locales_of);
        realm.intrinsics.others.insert(tag, proto);
        (c, proto)
    };

    let (dtf_ctor, dtf) = mk(realm, "DateTimeFormat", "Intl.DateTimeFormat", date_time_format::ctor, 0);
    date_time_format::setup(realm, dtf_ctor, dtf);

    number_format::register(realm, intl);

    let (_, coll) = mk(realm, "Collator", "Intl.Collator", collator_ctor, 0);
    realm.define_accessor(coll, "compare", Some(collator_compare_getter), None, CONFIGURABLE);
    realm.define_method(coll, "resolvedOptions", 0, collator_resolved_options);

    let (_, pr) = mk(realm, "PluralRules", "Intl.PluralRules", plural_rules::ctor, 0);
    realm.define_method(pr, "select", 1, plural_rules::select);
    realm.define_method(pr, "selectRange", 2, plural_rules::select_range);
    realm.define_method(pr, "resolvedOptions", 0, plural_rules::resolved_options);

    let (_, rtf) = mk(realm, "RelativeTimeFormat", "Intl.RelativeTimeFormat", relative_time::ctor, 0);
    realm.define_method(rtf, "format", 2, relative_time::format);
    realm.define_method(rtf, "formatToParts", 2, relative_time::format_to_parts);
    realm.define_method(rtf, "resolvedOptions", 0, relative_time::resolved_options);

    let (_, lf) = mk(realm, "ListFormat", "Intl.ListFormat", list_format::ctor, 0);
    realm.define_method(lf, "format", 1, list_format::format);
    realm.define_method(lf, "formatToParts", 1, list_format::format_to_parts);
    realm.define_method(lf, "resolvedOptions", 0, list_format::resolved_options);

    let (_, loc) = mk(realm, "Locale", "Intl.Locale", locale_obj::ctor, 1);
    realm.define_method(loc, "toString", 0, locale_obj::to_string);
    realm.define_method(loc, "maximize", 0, locale_obj::maximize);
    realm.define_method(loc, "minimize", 0, locale_obj::minimize);
    for (name, f) in [
        ("baseName", locale_obj::base_name as NativeFn),
        ("calendar", locale_obj::calendar),
        ("caseFirst", locale_obj::case_first),
        ("collation", locale_obj::collation),
        ("hourCycle", locale_obj::hour_cycle),
        ("numeric", locale_obj::numeric),
        ("numberingSystem", locale_obj::numbering_system),
        ("language", locale_obj::language),
        ("script", locale_obj::script),
        ("region", locale_obj::region),
        ("variants", locale_obj::variants),
    ] {
        realm.define_accessor(loc, name, Some(f), None, CONFIGURABLE);
    }

    let (_, dn) = mk(realm, "DisplayNames", "Intl.DisplayNames", display_names::ctor, 2);
    realm.define_method(dn, "of", 1, display_names::of);
    realm.define_method(dn, "resolvedOptions", 0, display_names::resolved_options);

    let (_, df) = mk(realm, "DurationFormat", "Intl.DurationFormat", duration::ctor, 0);
    realm.define_method(df, "format", 1, duration::format);
    realm.define_method(df, "formatToParts", 1, duration::format_to_parts);
    realm.define_method(df, "resolvedOptions", 0, duration::resolved_options);

    let (_, seg) = mk(realm, "Segmenter", "Intl.Segmenter", segmenter::ctor, 0);
    realm.define_method(seg, "segment", 1, segmenter::segment);
    realm.define_method(seg, "resolvedOptions", 0, segmenter::resolved_options);
    segmenter::register_prototypes(realm);

    // Locale-sensitive methods on the primitives route through Intl.
    let sp = realm.intrinsics.string_proto;
    realm.define_method(sp, "localeCompare", 1, string_locale_compare);
    realm.define_method(sp, "toLocaleLowerCase", 0, case_mapping::to_locale_lower_case);
    realm.define_method(sp, "toLocaleUpperCase", 0, case_mapping::to_locale_upper_case);
}

fn supported_locales_of(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let list = canonical_locale_list(realm, arg(args, 0))?;
    // SupportedLocales: validate the options (localeMatcher) after the list.
    let opts = arg(args, 1);
    if !opts.is_undefined() {
        let o = realm.to_object(opts)?;
        let m = realm.get_str(Value::Object(o), "localeMatcher")?;
        if !m.is_undefined() {
            let m = realm.to_rust_string(m)?;
            if m != "lookup" && m != "best fit" {
                return realm.range_error(&format!("Value {} out of range for localeMatcher", m));
            }
        }
    }
    let vals = list.into_iter().filter(|t| is_available(t)).map(|s| Value::str(&s)).collect();
    Ok(realm.new_array_value(vals))
}

fn get_canonical_locales(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let list = canonical_locale_list(realm, arg(args, 0))?;
    let vals = list.into_iter().map(|s| Value::str(&s)).collect();
    Ok(realm.new_array_value(vals))
}

fn supported_values_of(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let key = realm.to_rust_string(arg(args, 0))?;
    let vals: Vec<&str> = match key.as_str() {
        "calendar" => date_time_format::CALENDARS.iter().map(|(n, _)| *n).collect(),
        "collation" => vec!["emoji", "eor", "phonebk", "pinyin", "stroke", "trad"],
        "currency" => vec!["AUD", "BRL", "CAD", "CHF", "CNY", "EUR", "GBP", "HKD", "INR", "JPY", "KRW", "MXN", "NZD", "RUB", "USD"],
        "numberingSystem" => {
            let mut v: Vec<&str> = NUMBERING_SYSTEMS.iter().map(|(n, _)| *n).collect();
            v.push("hanidec");
            v.sort_unstable();
            v
        }
        "timeZone" => {
            let vals = date_time_format::time_zone_ids().iter().map(|z| Value::str(z)).collect();
            return Ok(realm.new_array_value(vals));
        }
        "unit" => number_format::SIMPLE_UNITS.to_vec(),
        _ => return realm.range_error(&format!("Invalid key : {}", key)),
    };
    let vals = vals.into_iter().map(Value::str).collect();
    Ok(realm.new_array_value(vals))
}

fn new_intl_object<T: 'static>(realm: &mut Realm, proto_key: &str, data: T) -> JsResult<ObjectId> {
    let default = realm.intrinsics.others[proto_key];
    let proto = realm.proto_from_new_target_checked(default)?;
    let id = realm.new_object_with_proto(Some(proto));
    realm.set_host_data(id, Box::new(data));
    Ok(id)
}

fn host<T: 'static + Clone>(realm: &mut Realm, this: &Value, what: &str) -> JsResult<T> {
    if let V::Object(id) = this.v() {
        if let Some(d) = realm.host_data::<T>(id) {
            return Ok(d.clone());
        }
    }
    realm.type_error(&format!("Method {} called on incompatible receiver", what))
}


fn bound_this(realm: &Realm) -> Value {
    realm.current_native().map(|n| realm.native_captures(n)).and_then(|c| c.into_iter().next()).unwrap_or(Value::Undefined)
}


fn string_locale_compare(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    if this.is_nullish() {
        return realm.type_error("String.prototype.localeCompare called on null or undefined");
    }
    let a = realm.to_rust_string(this)?;
    let b = realm.to_rust_string(arg(args, 0))?;
    locale_compare(realm, &a, &b, args)
}

// Collator
fn collator_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let o = read_collator_options(realm, arg(args, 0), arg(args, 1))?;
    // (Intl.Collator called as a function also constructs one.)
    Ok(Value::Object(new_intl_object(realm, "Intl.Collator", o)?))
}

fn collator_compare_getter(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    if !matches!(this.v(), V::Object(id) if realm.host_data::<CollatorOptions>(id).is_some()) {
        return realm.type_error("Method get Intl.Collator.prototype.compare called on incompatible receiver");
    }
    let b = realm.new_native_function_full("", 2, collator_compare_bound, None, vec![this], false);
    Ok(Value::Object(b))
}

fn collator_compare_bound(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let this = bound_this(realm);
    let o: CollatorOptions = host(realm, &this, "Intl.Collator.prototype.compare")?;
    let a = realm.to_rust_string(arg(args, 0))?;
    let b = realm.to_rust_string(arg(args, 1))?;
    Ok(Value::Number(match collator_compare(&a, &b, &o) {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
        std::cmp::Ordering::Greater => 1.0,
    }))
}

fn collator_resolved_options(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let o: CollatorOptions = host(realm, &this, "Intl.Collator.prototype.resolvedOptions")?;
    let obj = realm.new_object();
    realm.define_property(obj, "locale", Value::str(&o.locale.tag), ALL);
    realm.define_property(obj, "usage", Value::str(&o.usage), ALL);
    realm.define_property(obj, "sensitivity", Value::str(&o.sensitivity), ALL);
    realm.define_property(obj, "ignorePunctuation", Value::Bool(o.ignore_punctuation), ALL);
    realm.define_property(obj, "collation", Value::str(&o.collation), ALL);
    realm.define_property(obj, "numeric", Value::Bool(o.numeric), ALL);
    realm.define_property(obj, "caseFirst", Value::str(&o.case_first), ALL);
    Ok(Value::Object(obj))
}


