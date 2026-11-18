//! `Intl.PluralRules` (ECMA-402 §17): option processing shared with
//! NumberFormat's digit options, plural operands taken from the formatted
//! number (so rounding and notation are honoured) and CLDR cardinal and
//! ordinal rules for common languages.

use super::number_format::{digit_resolved, locale_of, notation_name, plural_digits, plural_nf_options, rounding_resolved, to_intl_mv, MathValue};
use super::*;

#[derive(Debug, Clone)]
pub struct PluralRulesData {
    nf: NfOptions,
    ordinal: bool,
}

/// Languages with plural rules here beyond the shared available locales.
const EXTRA_LANGUAGES: &[&str] = &["be", "bo", "dz", "fil", "gv", "ig", "jv", "sah", "ses", "sg", "su", "to", "wo", "yo"];

/// CLDR plural operands of a formatted number.
struct Operands {
    n: f64,
    /// The integer digits; `i` is their value when small enough.
    i_small: Option<u64>,
    /// The integer value modulo 10^9.
    i_mod: u64,
    v: usize,
    f: u64,
    t: u64,
    e: i64,
}

fn operands(s: &str, e: i64) -> Operands {
    // Shift the decimal point by the exponent (compact/scientific).
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    let (mut int, mut frac) = (int.to_string(), frac.to_string());
    if e > 0 {
        let take = (e as usize).min(frac.len());
        int.push_str(&frac[..take]);
        frac = frac[take..].to_string();
        int.push_str(&"0".repeat(e as usize - take));
    }
    let int_trim = int.trim_start_matches('0');
    let i_small = if int_trim.len() <= 18 { Some(int_trim.parse().unwrap_or(0)) } else { None };
    let tail = &int_trim[int_trim.len().saturating_sub(9)..];
    let i_mod = tail.parse().unwrap_or(0);
    let t_str = frac.trim_end_matches('0');
    let f = frac.get(..frac.len().min(18)).unwrap_or("").parse().unwrap_or(0);
    let t = t_str.get(..t_str.len().min(18)).unwrap_or("").parse().unwrap_or(0);
    let n: f64 = format!("{}.{}", if int.is_empty() { "0" } else { &int }, if frac.is_empty() { "0" } else { &frac }).parse().unwrap_or(0.0);
    Operands { n, i_small, i_mod, v: frac.len(), f, t, e }
}

impl Operands {
    fn i_is(&self, k: u64) -> bool {
        self.i_small == Some(k)
    }
    fn n_is(&self, k: f64) -> bool {
        self.n == k
    }
    fn n_int(&self) -> bool {
        self.n.fract() == 0.0
    }
    /// n % m for integral n (None when n has a fraction).
    fn n_mod(&self, m: u64) -> Option<u64> {
        if self.t == 0 {
            Some(self.i_mod % m)
        } else {
            None
        }
    }
    /// The French/Spanish/Italian/Portuguese "many" rule.
    fn romance_many(&self) -> bool {
        (self.e == 0 && !self.i_is(0) && self.i_mod % 1_000_000 == 0 && self.v == 0) || !(0..=5).contains(&self.e)
    }
}

fn data_lang(o: &NfOptions) -> String {
    let (locale, lang) = locale_of(o);
    if lang == "pt" && locale.contains("-PT") {
        "pt-PT".into()
    } else {
        lang.to_string()
    }
}

fn cardinal(lang: &str, op: &Operands) -> &'static str {
    let iv0 = op.v == 0;
    match lang {
        "ja" | "zh" | "ko" | "th" | "vi" | "id" | "ms" | "km" | "lo" | "my" | "yo" | "ig" | "jv" | "su" | "bo" | "dz" | "sah" | "ses" | "sg" | "to" | "wo" => "other",
        "fr" => {
            if op.i_is(0) || op.i_is(1) {
                "one"
            } else if op.romance_many() {
                "many"
            } else {
                "other"
            }
        }
        "pt" => {
            if op.i_is(0) || op.i_is(1) {
                "one"
            } else if op.romance_many() {
                "many"
            } else {
                "other"
            }
        }
        "es" => {
            if op.n_is(1.0) {
                "one"
            } else if op.romance_many() {
                "many"
            } else {
                "other"
            }
        }
        "it" | "ca" | "pt-PT" => {
            if op.i_is(1) && iv0 {
                "one"
            } else if op.romance_many() {
                "many"
            } else {
                "other"
            }
        }
        "fa" | "hi" | "bn" | "gu" | "kn" | "zu" | "am" | "mr" => {
            if op.i_is(0) || op.n_is(1.0) {
                "one"
            } else {
                "other"
            }
        }
        "tr" | "el" | "bg" | "hu" | "eu" | "af" | "az" | "ka" | "kk" | "ky" | "mn" | "ne" | "sq" | "uz" | "ta" | "te" | "ml" | "nb" | "no" | "nn" => {
            if op.n_is(1.0) {
                "one"
            } else {
                "other"
            }
        }
        "da" => {
            if op.n_is(1.0) || (op.t != 0 && (op.i_is(0) || op.i_is(1))) {
                "one"
            } else {
                "other"
            }
        }
        "is" => {
            if (op.t == 0 && op.i_mod % 10 == 1 && op.i_mod % 100 != 11) || (op.t % 10 == 1 && op.t % 100 != 11) {
                "one"
            } else {
                "other"
            }
        }
        "ru" | "uk" | "be" => {
            let (i10, i100) = (op.i_mod % 10, op.i_mod % 100);
            if !iv0 {
                "other"
            } else if i10 == 1 && i100 != 11 {
                "one"
            } else if (2..=4).contains(&i10) && !(12..=14).contains(&i100) {
                "few"
            } else {
                "many"
            }
        }
        "pl" => {
            let (i10, i100) = (op.i_mod % 10, op.i_mod % 100);
            if op.i_is(1) && iv0 {
                "one"
            } else if iv0 && (2..=4).contains(&i10) && !(12..=14).contains(&i100) {
                "few"
            } else if iv0 {
                "many"
            } else {
                "other"
            }
        }
        "cs" | "sk" => {
            if op.i_is(1) && iv0 {
                "one"
            } else if iv0 && (op.i_is(2) || op.i_is(3) || op.i_is(4)) {
                "few"
            } else if !iv0 {
                "many"
            } else {
                "other"
            }
        }
        "hr" | "sr" | "bs" => {
            let (i10, i100, f10, f100) = (op.i_mod % 10, op.i_mod % 100, op.f % 10, op.f % 100);
            if (iv0 && i10 == 1 && i100 != 11) || (f10 == 1 && f100 != 11) {
                "one"
            } else if (iv0 && (2..=4).contains(&i10) && !(12..=14).contains(&i100)) || ((2..=4).contains(&f10) && !(12..=14).contains(&f100)) {
                "few"
            } else {
                "other"
            }
        }
        "sl" => {
            let i100 = op.i_mod % 100;
            if iv0 && i100 == 1 {
                "one"
            } else if iv0 && i100 == 2 {
                "two"
            } else if (iv0 && (3..=4).contains(&i100)) || !iv0 {
                "few"
            } else {
                "other"
            }
        }
        "gv" => {
            let (i10, i100) = (op.i_mod % 10, op.i_mod % 100);
            if iv0 && i10 == 1 {
                "one"
            } else if iv0 && i10 == 2 {
                "two"
            } else if iv0 && [0, 20, 40, 60, 80].contains(&i100) {
                "few"
            } else if !iv0 {
                "many"
            } else {
                "other"
            }
        }
        "he" => {
            if (op.i_is(1) && iv0) || (op.i_is(0) && !iv0) {
                "one"
            } else if op.i_is(2) && iv0 {
                "two"
            } else {
                "other"
            }
        }
        "ro" => {
            let n100 = op.n_mod(100);
            if op.i_is(1) && iv0 {
                "one"
            } else if !iv0 || op.n_is(0.0) || (!op.n_is(1.0) && n100.map(|m| (1..=19).contains(&m)).unwrap_or(false)) {
                "few"
            } else {
                "other"
            }
        }
        "lt" => {
            let n10 = op.n_mod(10);
            let n100 = op.n_mod(100);
            if n10 == Some(1) && !n100.map(|m| (11..=19).contains(&m)).unwrap_or(false) {
                "one"
            } else if n10.map(|m| (2..=9).contains(&m)).unwrap_or(false) && !n100.map(|m| (11..=19).contains(&m)).unwrap_or(false) {
                "few"
            } else if op.f != 0 {
                "many"
            } else {
                "other"
            }
        }
        "lv" => {
            let n10 = op.n_mod(10);
            let n100 = op.n_mod(100);
            let (f10, f100) = (op.f % 10, op.f % 100);
            if n10 == Some(0) || n100.map(|m| (11..=19).contains(&m)).unwrap_or(false) || (op.v == 2 && (11..=19).contains(&f100)) {
                "zero"
            } else if (n10 == Some(1) && n100 != Some(11)) || (op.v == 2 && f10 == 1 && f100 != 11) || (op.v != 2 && f10 == 1) {
                "one"
            } else {
                "other"
            }
        }
        "ar" => {
            let n100 = op.n_mod(100);
            if op.n_is(0.0) {
                "zero"
            } else if op.n_is(1.0) {
                "one"
            } else if op.n_is(2.0) {
                "two"
            } else if n100.map(|m| (3..=10).contains(&m)).unwrap_or(false) {
                "few"
            } else if n100.map(|m| (11..=99).contains(&m)).unwrap_or(false) {
                "many"
            } else {
                "other"
            }
        }
        "cy" => {
            if op.n_is(0.0) {
                "zero"
            } else if op.n_is(1.0) {
                "one"
            } else if op.n_is(2.0) {
                "two"
            } else if op.n_is(3.0) {
                "few"
            } else if op.n_is(6.0) {
                "many"
            } else {
                "other"
            }
        }
        "ga" => {
            let n = op.n;
            if n == 1.0 {
                "one"
            } else if n == 2.0 {
                "two"
            } else if op.n_int() && (3.0..=6.0).contains(&n) {
                "few"
            } else if op.n_int() && (7.0..=10.0).contains(&n) {
                "many"
            } else {
                "other"
            }
        }
        // English, German, Dutch, Swedish, Finnish, Estonian, ...
        _ => {
            if op.i_is(1) && iv0 {
                "one"
            } else {
                "other"
            }
        }
    }
}

fn ordinal(lang: &str, op: &Operands) -> &'static str {
    match lang {
        "en" => {
            let (n10, n100) = (op.n_mod(10), op.n_mod(100));
            if n10 == Some(1) && n100 != Some(11) {
                "one"
            } else if n10 == Some(2) && n100 != Some(12) {
                "two"
            } else if n10 == Some(3) && n100 != Some(13) {
                "few"
            } else {
                "other"
            }
        }
        "fr" | "ms" | "vi" | "fil" | "ga" | "hy" | "lo" | "ro" => {
            if op.n_is(1.0) {
                "one"
            } else {
                "other"
            }
        }
        _ => "other",
    }
}

/// The categories a language's rules can produce, in CLDR order.
fn categories(lang: &str, is_ordinal: bool) -> &'static [&'static str] {
    if is_ordinal {
        return match lang {
            "en" => &["one", "two", "few", "other"],
            "fr" | "ms" | "vi" | "fil" | "ga" | "hy" | "lo" | "ro" => &["one", "other"],
            _ => &["other"],
        };
    }
    match lang {
        "ja" | "zh" | "ko" | "th" | "vi" | "id" | "ms" | "km" | "lo" | "my" | "yo" | "ig" | "jv" | "su" | "bo" | "dz" | "sah" | "ses" | "sg" | "to" | "wo" => {
            &["other"]
        }
        "fr" | "pt" | "es" | "it" | "ca" | "pt-PT" => &["one", "many", "other"],
        "ru" | "uk" | "be" | "pl" | "cs" | "sk" | "lt" => &["one", "few", "many", "other"],
        "hr" | "sr" | "bs" | "ro" => &["one", "few", "other"],
        "sl" => &["one", "two", "few", "other"],
        "gv" => &["one", "two", "few", "many", "other"],
        "he" => &["one", "two", "other"],
        "lv" => &["zero", "one", "other"],
        "ar" | "cy" => &["zero", "one", "two", "few", "many", "other"],
        "ga" => &["one", "two", "few", "many", "other"],
        _ => &["one", "other"],
    }
}

/// ResolvePlural.
fn resolve_plural(o: &PluralRulesData, n: f64) -> &'static str {
    let Some((s, e)) = plural_digits(&o.nf, &MathValue::from_f64(n)) else { return "other" };
    let op = operands(&s, e);
    let lang = data_lang(&o.nf);
    if o.ordinal {
        ordinal(&lang, &op)
    } else {
        cardinal(&lang, &op)
    }
}

pub(super) fn ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if realm.new_target().is_undefined() {
        return realm.type_error("Constructor Intl.PluralRules requires 'new'");
    }
    let requested = canonical_locale_list(realm, arg(args, 0))?;
    let opts = options_object(realm, arg(args, 1))?;
    number_format::get_option(realm, opts, "localeMatcher", &["lookup", "best fit"], "best fit")?;
    let kind = number_format::get_option(realm, opts, "type", &["cardinal", "ordinal"], "cardinal")?;
    let notation = number_format::get_option(realm, opts, "notation", &["standard", "scientific", "engineering", "compact"], "standard")?;
    let compact_display = number_format::get_option(realm, opts, "compactDisplay", &["short", "long"], "short")?;
    let nf = plural_nf_options(realm, &requested, opts, notation, compact_display, EXTRA_LANGUAGES)?;
    let data = PluralRulesData { nf, ordinal: kind == "ordinal" };
    Ok(Value::Object(new_intl_object(realm, "Intl.PluralRules", data)?))
}

pub(super) fn select(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o: PluralRulesData = host(realm, &this, "Intl.PluralRules.prototype.select")?;
    let n = realm.to_number(arg(args, 0))?;
    Ok(Value::str(resolve_plural(&o, n)))
}

pub(super) fn select_range(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o: PluralRulesData = host(realm, &this, "Intl.PluralRules.prototype.selectRange")?;
    let (a, b) = (arg(args, 0), arg(args, 1));
    if a.is_undefined() || b.is_undefined() {
        return realm.type_error("start and end are required");
    }
    let x = to_intl_mv(realm, a)?;
    let y = to_intl_mv(realm, b)?;
    if x == MathValue::NaN || y == MathValue::NaN {
        return realm.range_error("Invalid number range: NaN");
    }
    // ResolvePluralRange: the end's category (CLDR plural ranges mostly
    // resolve to it).
    let end = match &y {
        MathValue::Finite(..) => {
            let (s, e) = plural_digits(&o.nf, &y).unwrap_or(("0".into(), 0));
            let op = operands(&s, e);
            let lang = data_lang(&o.nf);
            if o.ordinal {
                ordinal(&lang, &op)
            } else {
                cardinal(&lang, &op)
            }
        }
        _ => "other",
    };
    Ok(Value::str(end))
}

pub(super) fn resolved_options(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let o: PluralRulesData = host(realm, &this, "Intl.PluralRules.prototype.resolvedOptions")?;
    let obj = realm.new_object();
    let (locale, _) = locale_of(&o.nf);
    realm.define_property(obj, "locale", Value::str(locale), ALL);
    realm.define_property(obj, "type", Value::str(if o.ordinal { "ordinal" } else { "cardinal" }), ALL);
    let (notation, compact) = notation_name(&o.nf);
    realm.define_property(obj, "notation", Value::str(notation), ALL);
    if let Some(c) = compact {
        realm.define_property(obj, "compactDisplay", Value::str(c), ALL);
    }
    let (min_int, frac, sig) = digit_resolved(&o.nf);
    realm.define_property(obj, "minimumIntegerDigits", Value::Number(min_int as f64), ALL);
    if let Some((a, b)) = frac {
        realm.define_property(obj, "minimumFractionDigits", Value::Number(a as f64), ALL);
        realm.define_property(obj, "maximumFractionDigits", Value::Number(b as f64), ALL);
    }
    if let Some((a, b)) = sig {
        realm.define_property(obj, "minimumSignificantDigits", Value::Number(a as f64), ALL);
        realm.define_property(obj, "maximumSignificantDigits", Value::Number(b as f64), ALL);
    }
    let cats: Vec<Value> = categories(&data_lang(&o.nf), o.ordinal).iter().map(|c| Value::str(c)).collect();
    let arr = realm.new_array_value(cats);
    realm.define_property(obj, "pluralCategories", arr, ALL);
    let (inc, mode, priority, tzd) = rounding_resolved(&o.nf);
    realm.define_property(obj, "roundingIncrement", Value::Number(inc as f64), ALL);
    realm.define_property(obj, "roundingMode", Value::str(mode), ALL);
    realm.define_property(obj, "roundingPriority", Value::str(priority), ALL);
    realm.define_property(obj, "trailingZeroDisplay", Value::str(tzd), ALL);
    Ok(Value::Object(obj))
}
