//! `Intl.NumberFormat` (ECMA-402 §16, including the NumberFormat v3
//! additions): option processing, exact decimal rounding of Intl
//! mathematical values (Numbers, BigInts and decimal strings), notations,
//! `formatToParts`, `formatRange`/`formatRangeToParts` and
//! `resolvedOptions`, plus `Number`/`BigInt.prototype.toLocaleString`.
//!
//! Locale data is built in for English and a handful of other languages
//! (separators, currency/percent patterns, compact names, a few units);
//! other languages fall back to English patterns.

use super::*;
use crate::descriptor::PropDesc;
use crate::interop::Hint;
use std::cmp::Ordering;

// ------------------------------------------------------------- decimals

/// A non-negative exact decimal `digits × 10^exp`; `digits` has no leading
/// or trailing zeros (empty means zero).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Dec {
    digits: Vec<u8>,
    exp: i64,
}

impl Dec {
    fn zero() -> Dec {
        Dec { digits: Vec::new(), exp: 0 }
    }

    fn new(mut digits: Vec<u8>, mut exp: i64) -> Dec {
        let lead = digits.iter().take_while(|d| **d == 0).count();
        digits.drain(..lead);
        while digits.last() == Some(&0) {
            digits.pop();
            exp += 1;
        }
        if digits.is_empty() {
            exp = 0;
        }
        Dec { digits, exp }
    }

    fn is_zero(&self) -> bool {
        self.digits.is_empty()
    }

    /// floor(log10(self)) for a non-zero value.
    fn magnitude(&self) -> i64 {
        self.digits.len() as i64 - 1 + self.exp
    }

    fn scale(&self, k: i64) -> Dec {
        if self.is_zero() {
            return Dec::zero();
        }
        Dec { digits: self.digits.clone(), exp: self.exp + k }
    }

    /// The digit at position 10^p.
    fn digit_at(&self, p: i64) -> u8 {
        if self.is_zero() {
            return 0;
        }
        let i = self.magnitude() - p;
        if i < 0 || i >= self.digits.len() as i64 {
            0
        } else {
            self.digits[i as usize]
        }
    }

    fn is_integer(&self) -> bool {
        self.is_zero() || self.exp >= 0
    }

    /// Plain decimal text ("digits[.digits]").
    #[cfg(test)]
    fn to_plain(&self) -> String {
        if self.is_zero() {
            return "0".into();
        }
        let ds: String = self.digits.iter().map(|d| (b'0' + d) as char).collect();
        if self.exp >= 0 {
            format!("{}{}", ds, "0".repeat(self.exp as usize))
        } else {
            let point = self.digits.len() as i64 + self.exp;
            if point > 0 {
                format!("{}.{}", &ds[..point as usize], &ds[point as usize..])
            } else {
                format!("0.{}{}", "0".repeat((-point) as usize), ds)
            }
        }
    }

    fn to_f64(&self) -> f64 {
        if self.is_zero() {
            return 0.0;
        }
        let ds: String = self.digits.iter().map(|d| (b'0' + d) as char).collect();
        format!("{}e{}", ds, self.exp).parse().unwrap_or(f64::NAN)
    }
}

fn divmod_small(digits: &[u8], k: u32) -> (Vec<u8>, u32) {
    let mut q = Vec::with_capacity(digits.len());
    let mut r: u64 = 0;
    for &d in digits {
        r = r * 10 + d as u64;
        q.push((r / k as u64) as u8);
        r %= k as u64;
    }
    (q, r as u32)
}

fn mul_small(digits: &[u8], k: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(digits.len() + 5);
    let mut carry: u64 = 0;
    for &d in digits.iter().rev() {
        let v = d as u64 * k as u64 + carry;
        out.push((v % 10) as u8);
        carry = v / 10;
    }
    while carry > 0 {
        out.push((carry % 10) as u8);
        carry /= 10;
    }
    out.reverse();
    out
}

fn add_one(digits: &[u8]) -> Vec<u8> {
    let mut out = digits.to_vec();
    let mut i = out.len();
    loop {
        if i == 0 {
            out.insert(0, 1);
            break;
        }
        i -= 1;
        if out[i] == 9 {
            out[i] = 0;
        } else {
            out[i] += 1;
            break;
        }
    }
    out
}

/// Unsigned rounding modes (ECMA-402 table "Conversion from rounding mode
/// to unsigned rounding mode").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unsigned {
    Infinity,
    Zero,
    HalfInfinity,
    HalfZero,
    HalfEven,
}

/// Round `x` to a multiple of `inc × 10^pos`.
fn round_dec(x: &Dec, pos: i64, inc: u32, mode: Unsigned) -> Dec {
    if x.is_zero() {
        return Dec::zero();
    }
    let mag = x.magnitude();
    let q: Vec<u8> = if mag < pos { Vec::new() } else { (0..=(mag - pos)).map(|i| x.digit_at(mag - i)).collect() };
    let f1 = x.digit_at(pos - 1);
    let rest = x.exp < pos - 1 && (x.exp..pos - 1).any(|p| x.digit_at(p) != 0);
    let frac_zero = f1 == 0 && !rest;
    let frac_vs_half = if f1 > 5 || (f1 == 5 && rest) {
        Ordering::Greater
    } else if f1 == 5 {
        Ordering::Equal
    } else {
        Ordering::Less
    };
    let (quot, rem) = divmod_small(&q, inc);
    if rem == 0 && frac_zero {
        return x.clone();
    }
    let (a, k) = (2 * rem as u64, inc as u64);
    let half = if a > k {
        Ordering::Greater
    } else if a + 2 <= k {
        Ordering::Less
    } else if a == k {
        if frac_zero {
            Ordering::Equal
        } else {
            Ordering::Greater
        }
    } else {
        frac_vs_half
    };
    let up = match mode {
        Unsigned::Zero => false,
        Unsigned::Infinity => true,
        Unsigned::HalfZero => half == Ordering::Greater,
        Unsigned::HalfInfinity => half != Ordering::Less,
        Unsigned::HalfEven => match half {
            Ordering::Greater => true,
            Ordering::Less => false,
            Ordering::Equal => quot.last().map(|d| d % 2 == 1).unwrap_or(false),
        },
    };
    let n = if up { add_one(&quot) } else { quot };
    Dec::new(mul_small(&n, inc), pos)
}

/// The result of ToRawPrecision / ToRawFixed.
struct Raw {
    rounded: Dec,
    s: String,
    int_digits: usize,
    magnitude: i64,
}

fn trim_cut(s: &mut String, mut cut: usize) {
    if !s.contains('.') {
        return;
    }
    while cut > 0 && s.ends_with('0') {
        s.pop();
        cut -= 1;
    }
    if s.ends_with('.') {
        s.pop();
    }
}

fn to_raw_precision(x: &Dec, min_p: usize, max_p: usize, mode: Unsigned) -> Raw {
    let p = max_p as i64;
    let (m, e, rounded) = if x.is_zero() {
        ("0".repeat(max_p), 0i64, Dec::zero())
    } else {
        let r = round_dec(x, x.magnitude() - p + 1, 1, mode);
        let e = r.magnitude();
        let mut m: String = r.digits.iter().map(|d| (b'0' + d) as char).collect();
        while (m.len() as i64) < p {
            m.push('0');
        }
        (m, e, r)
    };
    let (mut s, int_digits) = if e >= p - 1 {
        (format!("{}{}", m, "0".repeat((e - p + 1) as usize)), (e + 1) as usize)
    } else if e >= 0 {
        let k = (e + 1) as usize;
        (format!("{}.{}", &m[..k], &m[k..]), k)
    } else {
        (format!("0.{}{}", "0".repeat((-(e + 1)) as usize), m), 1)
    };
    if max_p > min_p {
        trim_cut(&mut s, max_p - min_p);
    }
    Raw { rounded, s, int_digits, magnitude: e - p + 1 }
}

fn to_raw_fixed(x: &Dec, min_f: usize, max_f: usize, inc: u32, mode: Unsigned) -> Raw {
    let f = max_f as i64;
    let r = round_dec(x, -f, inc, mode);
    let mut m = if r.is_zero() {
        "0".to_string()
    } else {
        let mut s: String = r.digits.iter().map(|d| (b'0' + d) as char).collect();
        s.push_str(&"0".repeat((r.exp + f).max(0) as usize));
        s
    };
    let int_digits;
    if max_f != 0 {
        let mut k = m.len();
        if k <= max_f {
            m = format!("{}{}", "0".repeat(max_f + 1 - k), m);
            k = max_f + 1;
        }
        let (a, b) = m.split_at(k - max_f);
        int_digits = a.len();
        m = format!("{}.{}", a, b);
    } else {
        int_digits = m.len();
    }
    trim_cut(&mut m, max_f - min_f);
    Raw { rounded: r, s: m, int_digits, magnitude: -f }
}

// ------------------------------------------------- mathematical values

/// An Intl mathematical value (§16.5.16 ToIntlMathematicalValue).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum MathValue {
    NaN,
    Infinity(bool),
    /// (negative, magnitude); negative zero is `Finite(true, 0)`.
    Finite(bool, Dec),
}

impl MathValue {
    pub(super) fn from_f64(x: f64) -> MathValue {
        if x.is_nan() {
            return MathValue::NaN;
        }
        if x.is_infinite() {
            return MathValue::Infinity(x < 0.0);
        }
        if x == 0.0 {
            return MathValue::Finite(x.is_sign_negative(), Dec::zero());
        }
        let s = format!("{:e}", x.abs());
        let (mant, e) = s.split_once('e').unwrap_or((&s, "0"));
        let e: i64 = e.parse().unwrap_or(0);
        let digits: Vec<u8> = mant.bytes().filter(|b| b.is_ascii_digit()).map(|b| b - b'0').collect();
        let n = digits.len() as i64;
        MathValue::Finite(x < 0.0, Dec::new(digits, e - (n - 1)))
    }

    fn from_decimal_digits(digits: &str, negative: bool) -> MathValue {
        let d: Vec<u8> = digits.bytes().filter(|b| b.is_ascii_digit()).map(|b| b - b'0').collect();
        MathValue::Finite(negative, Dec::new(d, 0))
    }

    /// StringIntlMV of a StringNumericLiteral (NaN when malformed), with
    /// RoundMVResult's overflow to infinity and underflow to zero.
    fn from_str(t: &str) -> MathValue {
        if t.is_empty() {
            return MathValue::Finite(false, Dec::zero());
        }
        let b = t.as_bytes();
        if b.len() > 2 && b[0] == b'0' {
            let radix = match b[1] {
                b'x' | b'X' => 16,
                b'o' | b'O' => 8,
                b'b' | b'B' => 2,
                _ => 0,
            };
            if radix != 0 {
                let rest = &t[2..];
                if !rest.chars().all(|c| c.is_digit(radix)) {
                    return MathValue::NaN;
                }
                return match crate::bigint::BigInt::parse_radix(rest, radix) {
                    Some(n) => MathValue::from_decimal_digits(&n.to_string_radix(10), false),
                    None => MathValue::NaN,
                };
            }
        }
        let (neg, body) = match b[0] {
            b'-' => (true, &t[1..]),
            b'+' => (false, &t[1..]),
            _ => (false, t),
        };
        if body == "Infinity" {
            return MathValue::Infinity(neg);
        }
        let (mant, exp) = match body.find(['e', 'E']) {
            Some(i) => (&body[..i], Some(&body[i + 1..])),
            None => (body, None),
        };
        let (ip, fp) = match mant.split_once('.') {
            Some((a, b)) => (a, b),
            None => (mant, ""),
        };
        if (ip.is_empty() && fp.is_empty()) || !ip.bytes().all(|c| c.is_ascii_digit()) || !fp.bytes().all(|c| c.is_ascii_digit()) {
            return MathValue::NaN;
        }
        let mut e: i64 = 0;
        if let Some(x) = exp {
            let digits = x.strip_prefix(['+', '-']).unwrap_or(x);
            if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
                return MathValue::NaN;
            }
            // Clamp absurd exponents; they overflow or underflow anyway.
            let v: i64 = digits.parse().unwrap_or(i64::MAX / 4).min(1_000_000_000);
            e = if x.starts_with('-') { -v } else { v };
        }
        let digits: Vec<u8> = ip.bytes().chain(fp.bytes()).map(|c| c - b'0').collect();
        let d = Dec::new(digits, e - fp.len() as i64);
        if d.is_zero() {
            return MathValue::Finite(neg, d);
        }
        let approx = d.to_f64();
        if approx.is_infinite() {
            return MathValue::Infinity(neg);
        }
        if approx == 0.0 {
            return MathValue::Finite(neg, Dec::zero());
        }
        MathValue::Finite(neg, d)
    }
}

/// ToIntlMathematicalValue.
pub(super) fn to_intl_mv(realm: &mut Realm, v: Value) -> JsResult<MathValue> {
    let prim = realm.to_primitive(v, Hint::Number)?;
    match prim.v() {
        V::BigInt(b) => Ok(MathValue::from_decimal_digits(&b.to_string_radix(10), b.is_negative())),
        V::String(s) => {
            let t = s.trim_js(true, true).to_rust_string();
            Ok(MathValue::from_str(&t))
        }
        _ => {
            let n = realm.to_number(prim)?;
            Ok(MathValue::from_f64(n))
        }
    }
}

// --------------------------------------------------------------- options

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NfStyle {
    Decimal,
    Currency,
    Percent,
    Unit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Notation {
    Standard,
    Scientific,
    Engineering,
    Compact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoundingType {
    Fraction,
    Significant,
    More,
    Less,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grouping {
    False,
    Min2,
    Auto,
    Always,
}

#[derive(Debug, Clone)]
pub struct NfOptions {
    /// The resolved locale (with a `-u-nu-` extension when it applies).
    pub(super) locale: String,
    lang: String,
    region: String,
    script: String,
    pub(super) numbering_system: String,
    pub(super) style: NfStyle,
    currency: Option<String>,
    currency_display: &'static str,
    currency_sign: &'static str,
    unit: Option<String>,
    unit_display: &'static str,
    min_int: usize,
    min_frac: Option<usize>,
    max_frac: Option<usize>,
    min_sig: Option<usize>,
    max_sig: Option<usize>,
    rounding_type: RoundingType,
    rounding_priority: &'static str,
    rounding_increment: u32,
    rounding_mode: &'static str,
    trailing_zero_display: &'static str,
    notation: Notation,
    compact_display: &'static str,
    use_grouping: Grouping,
    sign_display: &'static str,
}

/// GetOption for a string option restricted to `allowed`.
pub(super) fn get_option(realm: &mut Realm, opts: Option<ObjectId>, name: &str, allowed: &[&'static str], default: &'static str) -> JsResult<&'static str> {
    let Some(o) = opts else { return Ok(default) };
    let v = realm.get_str(Value::Object(o), name)?;
    if v.is_undefined() {
        return Ok(default);
    }
    let s = realm.to_rust_string(v)?;
    match allowed.iter().find(|a| **a == s) {
        Some(a) => Ok(a),
        None => realm.range_error(&format!("Value {} out of range for Intl.NumberFormat options property {}", s, name)),
    }
}

/// DefaultNumberOption.
fn default_number_option(realm: &mut Realm, v: Value, min: f64, max: f64, name: &str) -> JsResult<Option<usize>> {
    if v.is_undefined() {
        return Ok(None);
    }
    let n = realm.to_number(v)?;
    if n.is_nan() || n < min || n > max {
        return realm.range_error(&format!("{} value is out of range.", name));
    }
    Ok(Some(n.floor() as usize))
}

fn get_value(realm: &mut Realm, opts: Option<ObjectId>, name: &str) -> JsResult<Value> {
    match opts {
        Some(o) => realm.get_str(Value::Object(o), name),
        None => Ok(Value::Undefined),
    }
}

/// The sanctioned single units (ECMA-402 table 2).
const SIMPLE_UNITS: &[&str] = &[
    "acre", "bit", "byte", "celsius", "centimeter", "day", "degree", "fahrenheit", "fluid-ounce", "foot", "gallon", "gigabit", "gigabyte", "gram", "hectare",
    "hour", "inch", "kilobit", "kilobyte", "kilogram", "kilometer", "liter", "megabit", "megabyte", "meter", "microsecond", "mile", "mile-scandinavian",
    "milliliter", "millimeter", "millisecond", "minute", "month", "nanosecond", "ounce", "percent", "petabyte", "pound", "second", "stone", "terabit",
    "terabyte", "week", "yard", "year",
];

/// IsWellFormedUnitIdentifier.
fn is_well_formed_unit(u: &str) -> bool {
    if SIMPLE_UNITS.contains(&u) {
        return true;
    }
    match u.split_once("-per-") {
        Some((a, b)) => SIMPLE_UNITS.contains(&a) && SIMPLE_UNITS.contains(&b),
        None => false,
    }
}

/// IsWellFormedCurrencyCode.
fn is_well_formed_currency(c: &str) -> bool {
    c.len() == 3 && c.bytes().all(|b| b.is_ascii_alphabetic())
}

fn currency_digits(code: &str) -> usize {
    match code {
        "BIF" | "CLP" | "DJF" | "GNF" | "ISK" | "JPY" | "KMF" | "KRW" | "PYG" | "RWF" | "UGX" | "UYI" | "VND" | "VUV" | "XAF" | "XOF" | "XPF" => 0,
        "BHD" | "IQD" | "JOD" | "KWD" | "LYD" | "OMR" | "TND" => 3,
        "CLF" | "UYW" => 4,
        _ => 2,
    }
}

/// The resolved locale: (locale string, data tag, numbering system).
fn resolve_nf_locale(realm: &mut Realm, requested: &[String], nu_option: Option<&str>, extra: &[&str]) -> (String, locale_id::LocaleId, String) {
    let mut found: Option<(locale_id::LocaleId, Option<String>)> = None;
    for tag in requested {
        let Some(id) = locale_id::parse(tag) else { continue };
        let mut base = id.clone();
        base.u_attributes.clear();
        base.u_keywords.clear();
        if is_available(&locale_id::to_string(&base)) || extra.contains(&base.language.as_str()) {
            let nu = id.u_keywords.iter().find(|(k, _)| k == "nu").map(|(_, v)| v.clone());
            found = Some((base, nu));
            break;
        }
    }
    let _ = realm;
    let (base, ext_nu) = found.unwrap_or_else(|| (locale_id::parse("en-US").unwrap(), None));
    let mut value = "latn".to_string();
    let mut keyword: Option<String> = None;
    if let Some(req) = ext_nu {
        if !req.is_empty() && numbering_zero(&req).is_some() {
            value = req.clone();
            keyword = Some(req);
        }
    }
    if let Some(opt) = nu_option {
        let opt = opt.to_ascii_lowercase();
        if numbering_zero(&opt).is_some() && opt != value {
            value = opt;
            keyword = None;
        }
    }
    let mut locale = locale_id::to_string(&base);
    if let Some(k) = keyword {
        let ext = format!("-u-nu-{}", k);
        match locale.find("-x-") {
            Some(i) => locale.insert_str(i, &ext),
            None => locale.push_str(&ext),
        }
    }
    (locale, base, value)
}

/// InitializeNumberFormat's option processing (§16.1.1), shared with the
/// other services that format numbers.
pub fn read_nf_options(realm: &mut Realm, locales: Value, options: Value) -> JsResult<NfOptions> {
    let requested = canonical_locale_list(realm, locales)?;
    let opts = options_object(realm, options)?;
    get_option(realm, opts, "localeMatcher", &["lookup", "best fit"], "best fit")?;
    let nu = opt_str(realm, opts, "numberingSystem")?;
    if let Some(n) = &nu {
        if !duration::is_unicode_type(n) {
            return realm.range_error(&format!("Invalid numberingSystem: {}", n));
        }
    }
    let (locale, id, numbering_system) = resolve_nf_locale(realm, &requested, nu.as_deref(), &[]);

    // SetNumberFormatUnitOptions
    let style = match get_option(realm, opts, "style", &["decimal", "percent", "currency", "unit"], "decimal")? {
        "percent" => NfStyle::Percent,
        "currency" => NfStyle::Currency,
        "unit" => NfStyle::Unit,
        _ => NfStyle::Decimal,
    };
    let currency = opt_str(realm, opts, "currency")?;
    match &currency {
        None if style == NfStyle::Currency => return realm.type_error("Currency code is required with currency style."),
        Some(c) if !is_well_formed_currency(c) => return realm.range_error(&format!("Invalid currency code : {}", c)),
        _ => {}
    }
    let currency_display = get_option(realm, opts, "currencyDisplay", &["code", "symbol", "narrowSymbol", "name"], "symbol")?;
    let currency_sign = get_option(realm, opts, "currencySign", &["standard", "accounting"], "standard")?;
    let unit = opt_str(realm, opts, "unit")?;
    match &unit {
        None if style == NfStyle::Unit => return realm.type_error("Unit is required with unit style."),
        Some(u) if !is_well_formed_unit(u) => return realm.range_error(&format!("Invalid unit argument for Intl.NumberFormat() '{}'", u)),
        _ => {}
    }
    let unit_display = get_option(realm, opts, "unitDisplay", &["short", "narrow", "long"], "short")?;

    let notation = match get_option(realm, opts, "notation", &["standard", "scientific", "engineering", "compact"], "standard")? {
        "scientific" => Notation::Scientific,
        "engineering" => Notation::Engineering,
        "compact" => Notation::Compact,
        _ => Notation::Standard,
    };
    let (mnfd_default, mxfd_default) = if style == NfStyle::Currency && notation == Notation::Standard {
        let d = currency_digits(&currency.as_deref().unwrap_or("").to_ascii_uppercase());
        (d, d)
    } else {
        (0, if style == NfStyle::Percent { 0 } else { 3 })
    };

    let Digits { min_int, min_frac, max_frac, min_sig, max_sig, rounding_type, rounding_priority, rounding_increment, rounding_mode, trailing_zero_display } =
        set_digit_options(realm, opts, mnfd_default, mxfd_default, notation)?;

    let compact_display = get_option(realm, opts, "compactDisplay", &["short", "long"], "short")?;
    let default_grouping = if notation == Notation::Compact { "min2" } else { "auto" };
    let use_grouping = {
        let v = get_value(realm, opts, "useGrouping")?;
        let g: Option<&'static str> = if v.is_undefined() {
            Some(default_grouping)
        } else if matches!(v.v(), V::Bool(true)) {
            Some("always")
        } else if !v.truthy() {
            None
        } else {
            let s = realm.to_rust_string(v)?;
            match s.as_str() {
                "min2" => Some("min2"),
                "auto" => Some("auto"),
                "always" => Some("always"),
                "true" | "false" => Some(default_grouping),
                _ => return realm.range_error(&format!("Value {} out of range for Intl.NumberFormat options property useGrouping", s)),
            }
        };
        match g {
            None => Grouping::False,
            Some("min2") => Grouping::Min2,
            Some("always") => Grouping::Always,
            _ => Grouping::Auto,
        }
    };
    let sign_display = get_option(realm, opts, "signDisplay", &["auto", "never", "always", "exceptZero", "negative"], "auto")?;

    Ok(NfOptions {
        locale,
        lang: id.language.clone(),
        region: id.region.clone().map(|r| r.to_ascii_uppercase()).unwrap_or_default(),
        script: id.script.clone().unwrap_or_default(),
        numbering_system,
        style,
        currency: if style == NfStyle::Currency { currency.map(|c| c.to_ascii_uppercase()) } else { None },
        currency_display,
        currency_sign,
        unit: if style == NfStyle::Unit { unit } else { None },
        unit_display,
        min_int,
        min_frac,
        max_frac,
        min_sig,
        max_sig,
        rounding_type,
        rounding_priority,
        rounding_increment,
        rounding_mode,
        trailing_zero_display,
        notation,
        compact_display,
        use_grouping,
        sign_display,
    })
}

/// The digit options of SetNumberFormatDigitOptions.
#[derive(Debug, Clone)]
pub(super) struct Digits {
    min_int: usize,
    min_frac: Option<usize>,
    max_frac: Option<usize>,
    min_sig: Option<usize>,
    max_sig: Option<usize>,
    rounding_type: RoundingType,
    rounding_priority: &'static str,
    rounding_increment: u32,
    rounding_mode: &'static str,
    trailing_zero_display: &'static str,
}

/// SetNumberFormatDigitOptions.
fn set_digit_options(realm: &mut Realm, opts: Option<ObjectId>, mnfd_default: usize, mut mxfd_default: usize, notation: Notation) -> JsResult<Digits> {
    let mnid = get_value(realm, opts, "minimumIntegerDigits")?;
    let min_int = default_number_option(realm, mnid, 1.0, 21.0, "minimumIntegerDigits")?.unwrap_or(1);
    let mnfd = get_value(realm, opts, "minimumFractionDigits")?;
    let mxfd = get_value(realm, opts, "maximumFractionDigits")?;
    let mnsd = get_value(realm, opts, "minimumSignificantDigits")?;
    let mxsd = get_value(realm, opts, "maximumSignificantDigits")?;
    let ri = get_value(realm, opts, "roundingIncrement")?;
    let rounding_increment = default_number_option(realm, ri, 1.0, 5000.0, "roundingIncrement")?.unwrap_or(1) as u32;
    if ![1, 2, 5, 10, 20, 25, 50, 100, 200, 250, 500, 1000, 2000, 2500, 5000].contains(&rounding_increment) {
        return realm.range_error("roundingIncrement value is out of range.");
    }
    let rounding_mode = get_option(
        realm,
        opts,
        "roundingMode",
        &["ceil", "floor", "expand", "trunc", "halfCeil", "halfFloor", "halfExpand", "halfTrunc", "halfEven"],
        "halfExpand",
    )?;
    let priority = get_option(realm, opts, "roundingPriority", &["auto", "morePrecision", "lessPrecision"], "auto")?;
    let trailing_zero_display = get_option(realm, opts, "trailingZeroDisplay", &["auto", "stripIfInteger"], "auto")?;
    if rounding_increment != 1 {
        mxfd_default = mnfd_default;
    }
    let has_sd = !mnsd.is_undefined() || !mxsd.is_undefined();
    let has_fd = !mnfd.is_undefined() || !mxfd.is_undefined();
    let mut need_sd = true;
    let mut need_fd = true;
    if priority == "auto" {
        need_sd = has_sd;
        if need_sd || (!has_fd && notation == Notation::Compact) {
            need_fd = false;
        }
    }
    let (mut min_sig, mut max_sig, mut min_frac, mut max_frac) = (None, None, None, None);
    if need_sd {
        if has_sd {
            let a = default_number_option(realm, mnsd, 1.0, 21.0, "minimumSignificantDigits")?.unwrap_or(1);
            let b = default_number_option(realm, mxsd, a as f64, 21.0, "maximumSignificantDigits")?.unwrap_or(21);
            min_sig = Some(a);
            max_sig = Some(b);
        } else {
            min_sig = Some(1);
            max_sig = Some(21);
        }
    }
    if need_fd {
        if has_fd {
            let a = default_number_option(realm, mnfd, 0.0, 100.0, "minimumFractionDigits")?;
            let b = default_number_option(realm, mxfd, 0.0, 100.0, "maximumFractionDigits")?;
            let (a, b) = match (a, b) {
                (None, Some(b)) => (mnfd_default.min(b), b),
                (Some(a), None) => (a, mxfd_default.max(a)),
                (Some(a), Some(b)) => {
                    if a > b {
                        return realm.range_error("maximumFractionDigits value is out of range.");
                    }
                    (a, b)
                }
                (None, None) => (mnfd_default, mxfd_default),
            };
            min_frac = Some(a);
            max_frac = Some(b);
        } else {
            min_frac = Some(mnfd_default);
            max_frac = Some(mxfd_default);
        }
    }
    let (rounding_type, rounding_priority) = if !need_sd && !need_fd {
        min_frac = Some(0);
        max_frac = Some(0);
        min_sig = Some(1);
        max_sig = Some(2);
        (RoundingType::More, "morePrecision")
    } else if priority == "auto" {
        (if has_sd { RoundingType::Significant } else { RoundingType::Fraction }, "auto")
    } else if priority == "morePrecision" {
        (RoundingType::More, "morePrecision")
    } else {
        (RoundingType::Less, "lessPrecision")
    };
    if rounding_increment != 1 {
        if rounding_type != RoundingType::Fraction {
            return realm.type_error("roundingIncrement requires fraction-digit rounding");
        }
        if max_frac != min_frac {
            return realm.range_error("roundingIncrement requires equal minimumFractionDigits and maximumFractionDigits");
        }
    }
    Ok(Digits { min_int, min_frac, max_frac, min_sig, max_sig, rounding_type, rounding_priority, rounding_increment, rounding_mode, trailing_zero_display })
}

// ----------------------------------------------------------- locale data

type CompactTable = &'static [(i64, &'static str, &'static str)];

struct LocData {
    group: &'static str,
    decimal: &'static str,
    min_grouping: usize,
    indian: bool,
    nan: &'static str,
    percent: &'static str,
    currency_prefix: bool,
    accounting_parens: bool,
    range_sep: &'static str,
    compact_short: CompactTable,
    compact_long: CompactTable,
    /// kilometer-per-hour patterns (short, narrow, long).
    kmh: Option<[&'static str; 3]>,
}

const EN_SHORT: CompactTable = &[(3, "{0}[K]", "{0}[K]"), (6, "{0}[M]", "{0}[M]"), (9, "{0}[B]", "{0}[B]"), (12, "{0}[T]", "{0}[T]")];
const EN_LONG: CompactTable = &[
    (3, "{0} [thousand]", "{0} [thousand]"),
    (6, "{0} [million]", "{0} [million]"),
    (9, "{0} [billion]", "{0} [billion]"),
    (12, "{0} [trillion]", "{0} [trillion]"),
];
const EN_IN_SHORT: CompactTable = &[(3, "{0}[K]", "{0}[K]"), (5, "{0}[L]", "{0}[L]"), (7, "{0}[Cr]", "{0}[Cr]")];
const EN_IN_LONG: CompactTable = &[(3, "{0} [thousand]", "{0} [thousand]"), (5, "{0} [lakh]", "{0} [lakh]"), (7, "{0} [crore]", "{0} [crore]")];
const DE_SHORT: CompactTable = &[(6, "{0}\u{a0}[Mio.]", "{0}\u{a0}[Mio.]"), (9, "{0}\u{a0}[Mrd.]", "{0}\u{a0}[Mrd.]"), (12, "{0}\u{a0}[Bio.]", "{0}\u{a0}[Bio.]")];
const DE_LONG: CompactTable = &[
    (3, "{0} [Tausend]", "{0} [Tausend]"),
    (6, "{0} [Million]", "{0} [Millionen]"),
    (9, "{0} [Milliarde]", "{0} [Milliarden]"),
    (12, "{0} [Billion]", "{0} [Billionen]"),
];
const ES_SHORT: CompactTable = &[(3, "{0}\u{a0}[mil]", "{0}\u{a0}[mil]"), (6, "{0}\u{a0}[M]", "{0}\u{a0}[M]"), (9, "{0}\u{a0}[mil\u{a0}M]", "{0}\u{a0}[mil\u{a0}M]"), (12, "{0}\u{a0}[B]", "{0}\u{a0}[B]")];
const ES_LONG: CompactTable = &[
    (3, "{0} [mil]", "{0} [mil]"),
    (6, "{0} [millón]", "{0} [millones]"),
    (9, "{0} [mil millones]", "{0} [mil millones]"),
    (12, "{0} [billón]", "{0} [billones]"),
];
const JA_COMPACT: CompactTable = &[(4, "{0}[万]", "{0}[万]"), (8, "{0}[億]", "{0}[億]"), (12, "{0}[兆]", "{0}[兆]"), (16, "{0}[京]", "{0}[京]")];
const KO_COMPACT: CompactTable = &[(3, "{0}[천]", "{0}[천]"), (4, "{0}[만]", "{0}[만]"), (8, "{0}[억]", "{0}[억]"), (12, "{0}[조]", "{0}[조]")];
const ZH_HANS_COMPACT: CompactTable = &[(4, "{0}[万]", "{0}[万]"), (8, "{0}[亿]", "{0}[亿]"), (12, "{0}[万亿]", "{0}[万亿]")];
const ZH_HANT_COMPACT: CompactTable = &[(4, "{0}[萬]", "{0}[萬]"), (8, "{0}[億]", "{0}[億]"), (12, "{0}[兆]", "{0}[兆]")];

fn locale_data(o: &NfOptions) -> LocData {
    let (lang, region) = (o.lang.as_str(), o.region.as_str());
    let (group, decimal, min_grouping) = match lang {
        "de" if region == "CH" || region == "LI" => ("’", ".", 1),
        "pt" if region == "PT" => ("\u{a0}", ",", 2),
        "pt" => (".", ",", 1),
        "es" => (".", ",", 2),
        "pl" => ("\u{a0}", ",", 2),
        "de" | "it" | "nl" | "da" | "id" | "tr" | "el" | "ro" | "hr" | "sl" | "sr" => (".", ",", 1),
        "fr" => ("\u{202f}", ",", 1),
        "ru" | "cs" | "sv" | "nb" | "no" | "fi" | "uk" | "sk" | "bg" | "hu" | "lt" | "lv" | "et" => ("\u{a0}", ",", 1),
        _ => (",", ".", 1),
    };
    let traditional = lang == "zh" && (o.script == "Hant" || matches!(region, "TW" | "HK" | "MO"));
    let (compact_short, compact_long) = match lang {
        "en" if region == "IN" => (EN_IN_SHORT, EN_IN_LONG),
        "de" => (DE_SHORT, DE_LONG),
        "es" => (ES_SHORT, ES_LONG),
        "ja" => (JA_COMPACT, JA_COMPACT),
        "ko" => (KO_COMPACT, KO_COMPACT),
        "zh" if traditional => (ZH_HANT_COMPACT, ZH_HANT_COMPACT),
        "zh" => (ZH_HANS_COMPACT, ZH_HANS_COMPACT),
        _ => (EN_SHORT, EN_LONG),
    };
    let kmh = match lang {
        "de" => Some(["{0} [km/h]", "{0} [km/h]", "{0} [Kilometer pro Stunde]"]),
        "ja" => Some(["{0} [km/h]", "{0}[km/h]", "[時速 ]{0}[ キロメートル]"]),
        "ko" => Some(["{0}[km/h]", "{0}[km/h]", "[시속] {0}[킬로미터]"]),
        "zh" if traditional => Some(["{0} [公里/小時]", "{0}[公里/小時]", "[每小時 ]{0}[ 公里]"]),
        "zh" => Some(["{0}[公里/小时]", "{0}[公里/小时]", "[每小时]{0}[公里]"]),
        _ => None,
    };
    LocData {
        group,
        decimal,
        min_grouping,
        indian: (lang == "en" && region == "IN") || lang == "hi",
        nan: if traditional { "非數值" } else { "NaN" },
        percent: if matches!(lang, "en" | "ja" | "zh" | "ko" | "pt" | "it" | "nl" | "id" | "th" | "he" | "hi" | "ms") { "%" } else { "\u{a0}%" },
        currency_prefix: matches!(lang, "en" | "ja" | "zh" | "ko" | "he" | "th" | "hi" | "id" | "ms" | "nl"),
        accounting_parens: matches!(lang, "en" | "ja" | "zh" | "ko"),
        range_sep: match lang {
            "pt" if region == "PT" => " - ",
            "ja" => "～",
            "ko" => "~",
            "zh" => "-",
            _ => "–",
        },
        compact_short,
        compact_long,
        kmh,
    }
}

fn currency_symbol(code: &str, lang: &str) -> String {
    match code {
        "USD" => match lang {
            "en" | "de" | "ja" | "nl" => "$",
            "fr" => "$US",
            _ => "US$",
        }
        .into(),
        "EUR" => "€".into(),
        "GBP" => "£".into(),
        "JPY" => if lang == "ja" { "￥" } else { "¥" }.into(),
        "CNY" => if lang == "zh" { "¥" } else { "CN¥" }.into(),
        "INR" => "₹".into(),
        "KRW" => "₩".into(),
        "BRL" => "R$".into(),
        "CAD" => if lang == "en" { "CA$" } else { "CA$" }.into(),
        "AUD" => "A$".into(),
        "MXN" => "MX$".into(),
        "ILS" => "₪".into(),
        "NZD" => "NZ$".into(),
        "HKD" => "HK$".into(),
        "TWD" => if lang == "zh" { "$" } else { "NT$" }.into(),
        "VND" => "₫".into(),
        "XAF" => "FCFA".into(),
        "XOF" => "F\u{202f}CFA".into(),
        "PHP" => "₱".into(),
        other => other.into(),
    }
}

fn narrow_currency_symbol(code: &str, lang: &str) -> String {
    match code {
        "USD" | "CAD" | "AUD" | "MXN" | "NZD" | "HKD" | "TWD" | "SGD" | "ARS" | "CLP" | "COP" => "$".into(),
        "JPY" | "CNY" => "¥".into(),
        "RUB" => "₽".into(),
        "TRY" => "₺".into(),
        "PLN" => "zł".into(),
        "BRL" => "R$".into(),
        _ => currency_symbol(code, lang),
    }
}

fn currency_name(code: &str, plural: bool) -> String {
    let (one, many) = match code {
        "USD" => ("US dollar", "US dollars"),
        "EUR" => ("euro", "euros"),
        "GBP" => ("British pound", "British pounds"),
        "JPY" => ("Japanese yen", "Japanese yen"),
        "CNY" => ("Chinese yuan", "Chinese yuan"),
        "INR" => ("Indian rupee", "Indian rupees"),
        "KRW" => ("South Korean won", "South Korean won"),
        "MXN" => ("Mexican peso", "Mexican pesos"),
        "CAD" => ("Canadian dollar", "Canadian dollars"),
        "AUD" => ("Australian dollar", "Australian dollars"),
        "CHF" => ("Swiss franc", "Swiss francs"),
        "BRL" => ("Brazilian real", "Brazilian reals"),
        "RUB" => ("Russian ruble", "Russian rubles"),
        "KWD" => ("Kuwaiti dinar", "Kuwaiti dinars"),
        _ => return code.to_string(),
    };
    if plural { many } else { one }.into()
}

/// English unit data: (unit, short symbol, narrow symbol, long singular, long plural).
const UNIT_NAMES: &[(&str, &str, &str, &str, &str)] = &[
    ("acre", "ac", "ac", "acre", "acres"),
    ("bit", "bit", "bit", "bit", "bits"),
    ("byte", "byte", "B", "byte", "bytes"),
    ("celsius", "°C", "°C", "degree Celsius", "degrees Celsius"),
    ("centimeter", "cm", "cm", "centimeter", "centimeters"),
    ("day", "day", "d", "day", "days"),
    ("degree", "deg", "°", "degree", "degrees"),
    ("fahrenheit", "°F", "°", "degree Fahrenheit", "degrees Fahrenheit"),
    ("fluid-ounce", "fl oz", "fl oz", "fluid ounce", "fluid ounces"),
    ("foot", "ft", "′", "foot", "feet"),
    ("gallon", "gal", "gal", "gallon", "gallons"),
    ("gigabit", "Gb", "Gb", "gigabit", "gigabits"),
    ("gigabyte", "GB", "GB", "gigabyte", "gigabytes"),
    ("gram", "g", "g", "gram", "grams"),
    ("hectare", "ha", "ha", "hectare", "hectares"),
    ("hour", "hr", "h", "hour", "hours"),
    ("inch", "in", "″", "inch", "inches"),
    ("kilobit", "kb", "kb", "kilobit", "kilobits"),
    ("kilobyte", "kB", "kB", "kilobyte", "kilobytes"),
    ("kilogram", "kg", "kg", "kilogram", "kilograms"),
    ("kilometer", "km", "km", "kilometer", "kilometers"),
    ("liter", "L", "L", "liter", "liters"),
    ("megabit", "Mb", "Mb", "megabit", "megabits"),
    ("megabyte", "MB", "MB", "megabyte", "megabytes"),
    ("meter", "m", "m", "meter", "meters"),
    ("microsecond", "μs", "μs", "microsecond", "microseconds"),
    ("mile", "mi", "mi", "mile", "miles"),
    ("mile-scandinavian", "smi", "smi", "mile-scandinavian", "miles-scandinavian"),
    ("milliliter", "mL", "mL", "milliliter", "milliliters"),
    ("millimeter", "mm", "mm", "millimeter", "millimeters"),
    ("millisecond", "ms", "ms", "millisecond", "milliseconds"),
    ("minute", "min", "m", "minute", "minutes"),
    ("month", "mth", "m", "month", "months"),
    ("nanosecond", "ns", "ns", "nanosecond", "nanoseconds"),
    ("ounce", "oz", "oz", "ounce", "ounces"),
    ("percent", "%", "%", "percent", "percent"),
    ("petabyte", "PB", "PB", "petabyte", "petabytes"),
    ("pound", "lb", "lb", "pound", "pounds"),
    ("second", "sec", "s", "second", "seconds"),
    ("stone", "st", "st", "stone", "stones"),
    ("terabit", "Tb", "Tb", "terabit", "terabits"),
    ("terabyte", "TB", "TB", "terabyte", "terabytes"),
    ("week", "wk", "w", "week", "weeks"),
    ("yard", "yd", "yd", "yard", "yards"),
    ("year", "yr", "y", "year", "years"),
];

fn unit_entry(u: &str) -> (&'static str, &'static str, &'static str, &'static str) {
    UNIT_NAMES.iter().find(|e| e.0 == u).map(|e| (e.1, e.2, e.3, e.4)).unwrap_or(("", "", "", ""))
}

/// The unit pattern for `unit` ("{0}" is the number, "[...]" unit text).
fn unit_pattern(unit: &str, display: &str, one: bool, ld: &LocData) -> String {
    if unit == "kilometer-per-hour" {
        if let Some(p) = ld.kmh {
            return match display {
                "narrow" => p[1],
                "long" => p[2],
                _ => p[0],
            }
            .to_string();
        }
    }
    let (short, narrow, long_one, long_other) = match unit {
        "kilometer-per-hour" => ("km/h", "km/h", "kilometer per hour", "kilometers per hour"),
        "mile-per-hour" => ("mph", "mph", "mile per hour", "miles per hour"),
        "meter-per-second" => ("m/s", "m/s", "meter per second", "meters per second"),
        "mile-per-gallon" => ("mpg", "mpg", "mile per gallon", "miles per gallon"),
        _ => match unit.split_once("-per-") {
            Some((a, b)) => {
                let (sa, na, la1, la2) = unit_entry(a);
                let (sb, nb, lb1, _) = unit_entry(b);
                let short = format!("{}/{}", sa, sb);
                let narrow = format!("{}/{}", na, nb);
                let long = format!("{} per {}", if one { la1 } else { la2 }, lb1);
                return match display {
                    "narrow" => format!("{{0}}[{}]", narrow),
                    "long" => format!("{{0}} [{}]", long),
                    _ => format!("{{0}} [{}]", short),
                };
            }
            None => unit_entry(unit),
        },
    };
    match display {
        "narrow" => format!("{{0}}[{}]", narrow),
        "long" => format!("{{0}} [{}]", if one { long_one } else { long_other }),
        _ => {
            if matches!(unit, "percent" | "celsius" | "fahrenheit") {
                format!("{{0}}[{}]", short)
            } else if matches!(unit, "day" | "week" | "month" | "year" | "hour" | "byte") && !one {
                let plural = match unit {
                    "day" => "days",
                    "week" => "wks",
                    "month" => "mths",
                    "year" => "yrs",
                    "hour" => "hr",
                    _ => "byte",
                };
                format!("{{0}} [{}]", plural)
            } else {
                format!("{{0}} [{}]", short)
            }
        }
    }
}

type Part = (&'static str, String);

/// Split a "{0}"-pattern into the parts before and after the number;
/// bracketed text becomes `kind` parts, the rest literals.
fn parse_pattern(p: &str, kind: &'static str) -> (Vec<Part>, Vec<Part>) {
    let (pre, post) = p.split_once("{0}").unwrap_or((p, ""));
    fn pieces(s: &str, kind: &'static str) -> Vec<Part> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut in_br = false;
        for c in s.chars() {
            match c {
                '[' if !in_br => {
                    if !cur.is_empty() {
                        out.push(("literal", std::mem::take(&mut cur)));
                    }
                    in_br = true;
                }
                ']' if in_br => {
                    out.push((kind, std::mem::take(&mut cur)));
                    in_br = false;
                }
                _ => {
                    // Spaces inside brackets at the edges are literals.
                    cur.push(c);
                }
            }
        }
        if !cur.is_empty() {
            out.push(("literal", cur));
        }
        // Move leading/trailing spaces of unit pieces into literals.
        let mut fixed = Vec::new();
        for (t, v) in out {
            if t == kind && (v.starts_with(' ') || v.ends_with(' ')) {
                let lead = v.len() - v.trim_start().len();
                let trail = v.len() - v.trim_end().len();
                if lead > 0 {
                    fixed.push(("literal", v[..lead].to_string()));
                }
                fixed.push((t, v.trim().to_string()));
                if trail > 0 {
                    fixed.push(("literal", v[v.len() - trail..].to_string()));
                }
            } else {
                fixed.push((t, v));
            }
        }
        // Merge adjacent literals.
        let mut merged: Vec<Part> = Vec::new();
        for (t, v) in fixed {
            if t == "literal" {
                if let Some(last) = merged.last_mut() {
                    if last.0 == "literal" {
                        last.1.push_str(&v);
                        continue;
                    }
                }
            }
            merged.push((t, v));
        }
        merged
    }
    (pieces(pre, kind), pieces(post, kind))
}

// ------------------------------------------------------------ formatting

fn unsigned_mode(mode: &str, negative: bool) -> Unsigned {
    match (mode, negative) {
        ("ceil", false) | ("floor", true) | ("expand", _) => Unsigned::Infinity,
        ("ceil", true) | ("floor", false) | ("trunc", _) => Unsigned::Zero,
        ("halfCeil", false) | ("halfFloor", true) | ("halfExpand", _) => Unsigned::HalfInfinity,
        ("halfCeil", true) | ("halfFloor", false) | ("halfTrunc", _) => Unsigned::HalfZero,
        _ => Unsigned::HalfEven,
    }
}

/// FormatNumericToString: the rounded value and its digits ("123.45").
fn format_numeric_to_string(o: &NfOptions, negative: bool, x: &Dec) -> (Dec, String) {
    let mode = unsigned_mode(o.rounding_mode, negative);
    let sig = |x: &Dec| to_raw_precision(x, o.min_sig.unwrap_or(1), o.max_sig.unwrap_or(21), mode);
    let fix = |x: &Dec| to_raw_fixed(x, o.min_frac.unwrap_or(0), o.max_frac.unwrap_or(3), o.rounding_increment, mode);
    let raw = match o.rounding_type {
        RoundingType::Significant => sig(x),
        RoundingType::Fraction => fix(x),
        RoundingType::More | RoundingType::Less => {
            let s = sig(x);
            let f = fix(x);
            let pick_s = if o.rounding_type == RoundingType::More { s.magnitude <= f.magnitude } else { s.magnitude > f.magnitude };
            if pick_s {
                s
            } else {
                f
            }
        }
    };
    let mut s = raw.s;
    if o.trailing_zero_display == "stripIfInteger" && raw.rounded.is_integer() {
        if let Some(i) = s.find('.') {
            s.truncate(i);
        }
    }
    if raw.int_digits < o.min_int {
        s = format!("{}{}", "0".repeat(o.min_int - raw.int_digits), s);
    }
    (raw.rounded, s)
}

fn compact_table(o: &NfOptions, ld: &LocData) -> CompactTable {
    if o.compact_display == "long" {
        ld.compact_long
    } else {
        ld.compact_short
    }
}

fn exponent_for_magnitude(o: &NfOptions, ld: &LocData, mag: i64) -> i64 {
    match o.notation {
        Notation::Standard => 0,
        Notation::Scientific => mag,
        Notation::Engineering => mag.div_euclid(3) * 3,
        Notation::Compact => compact_table(o, ld).iter().rev().find(|e| e.0 <= mag).map(|e| e.0).unwrap_or(0),
    }
}

/// ComputeExponent.
fn compute_exponent(o: &NfOptions, ld: &LocData, x: &Dec) -> i64 {
    if x.is_zero() || o.notation == Notation::Standard {
        return 0;
    }
    let mag = x.magnitude();
    let exponent = exponent_for_magnitude(o, ld, mag);
    let (r, _) = format_numeric_to_string(o, false, &x.scale(-exponent));
    if r.is_zero() || r.magnitude() == mag - exponent {
        return exponent;
    }
    exponent_for_magnitude(o, ld, mag + 1)
}

fn should_group(len: usize, o: &NfOptions, ld: &LocData) -> bool {
    match o.use_grouping {
        Grouping::False => false,
        Grouping::Always => len > 3,
        Grouping::Auto => len >= 3 + ld.min_grouping,
        Grouping::Min2 => len >= 3 + ld.min_grouping.max(2),
    }
}

/// Integer and fraction parts of digits "123.45", grouped and transliterated.
fn push_number_parts(out: &mut Vec<Part>, s: &str, o: &NfOptions, ld: &LocData) {
    let (int, frac) = match s.split_once('.') {
        Some((a, b)) => (a, Some(b)),
        None => (s, None),
    };
    let nu = o.numbering_system.as_str();
    if should_group(int.len(), o, ld) {
        let mut groups: Vec<&str> = Vec::new();
        let mut end = int.len();
        let mut size = 3;
        while end > size {
            groups.push(&int[end - size..end]);
            end -= size;
            if ld.indian {
                size = 2;
            }
        }
        groups.push(&int[..end]);
        groups.reverse();
        for (i, g) in groups.iter().enumerate() {
            if i > 0 {
                out.push(("group", ld.group.to_string()));
            }
            out.push(("integer", transliterate_digits(g, nu)));
        }
    } else {
        out.push(("integer", transliterate_digits(int, nu)));
    }
    if let Some(f) = frac {
        out.push(("decimal", ld.decimal.to_string()));
        out.push(("fraction", transliterate_digits(f, nu)));
    }
}

/// A formatted number split the way range collapsing needs it.
#[derive(Clone, Debug, Default)]
struct Formatted {
    outer_pre: Vec<Part>,
    mid_pre: Vec<Part>,
    number: Vec<Part>,
    mid_suf: Vec<Part>,
    outer_suf: Vec<Part>,
}

impl Formatted {
    fn into_parts(self) -> Vec<Part> {
        let mut v = self.outer_pre;
        v.extend(self.mid_pre);
        v.extend(self.number);
        v.extend(self.mid_suf);
        v.extend(self.outer_suf);
        v
    }

    fn text(parts: &[Part]) -> String {
        parts.iter().map(|p| p.1.as_str()).collect()
    }
}

/// PartitionNumberPattern.
fn partition(o: &NfOptions, v: &MathValue) -> Formatted {
    let ld = locale_data(o);
    let lang = o.lang.as_str();
    let mut f = Formatted::default();
    let (negative, is_nan, is_zero, digits) = match v {
        MathValue::NaN => {
            f.number.push(("nan", ld.nan.to_string()));
            (false, true, false, String::from("NaN"))
        }
        MathValue::Infinity(neg) => {
            f.number.push(("infinity", "∞".to_string()));
            (*neg, false, false, String::from("∞"))
        }
        MathValue::Finite(neg, d) => {
            let mut x = d.clone();
            let mut exponent = 0;
            if !(*neg && x.is_zero()) {
                if o.style == NfStyle::Percent {
                    x = x.scale(2);
                }
                exponent = compute_exponent(o, &ld, &x);
                x = x.scale(-exponent);
            }
            let (r, s) = format_numeric_to_string(o, *neg, &x);
            let mut compact_post = Vec::new();
            match o.notation {
                Notation::Compact if exponent != 0 => {
                    if let Some(e) = compact_table(o, &ld).iter().find(|e| e.0 == exponent) {
                        let (pre, post) = parse_pattern(if s == "1" { e.1 } else { e.2 }, "compact");
                        f.number.extend(pre);
                        compact_post = post;
                    }
                }
                _ => {}
            }
            push_number_parts(&mut f.number, &s, o, &ld);
            match o.notation {
                Notation::Scientific | Notation::Engineering => {
                    f.number.push(("exponentSeparator", "E".to_string()));
                    if exponent < 0 {
                        f.number.push(("exponentMinusSign", "-".to_string()));
                    }
                    f.number.push(("exponentInteger", transliterate_digits(&exponent.abs().to_string(), &o.numbering_system)));
                }
                _ => {}
            }
            f.number.extend(compact_post);
            (*neg, false, r.is_zero(), s)
        }
    };
    // Sign selection (GetNumberFormatPattern).
    let sign: Option<bool> = match o.sign_display {
        "never" => None,
        "always" => Some(negative),
        "exceptZero" => {
            if is_nan || is_zero {
                None
            } else {
                Some(negative)
            }
        }
        "negative" => {
            if negative && !is_zero && !is_nan {
                Some(true)
            } else {
                None
            }
        }
        _ => {
            if negative {
                Some(true)
            } else {
                None
            }
        }
    };
    let one = digits == "1";
    let accounting = sign == Some(true)
        && o.style == NfStyle::Currency
        && o.currency_sign == "accounting"
        && ld.accounting_parens
        && o.currency_display != "name";
    if accounting {
        f.mid_pre.push(("literal", "(".to_string()));
    } else {
        match sign {
            Some(true) => f.mid_pre.push(("minusSign", "-".to_string())),
            Some(false) => f.mid_pre.push(("plusSign", "+".to_string())),
            None => {}
        }
    }
    match o.style {
        NfStyle::Currency => {
            let code = o.currency.clone().unwrap_or_default();
            if o.currency_display == "name" {
                f.outer_suf.push(("literal", " ".to_string()));
                f.outer_suf.push(("currency", currency_name(&code, !one)));
            } else {
                let sym = match o.currency_display {
                    "code" => code.clone(),
                    "narrowSymbol" => narrow_currency_symbol(&code, lang),
                    _ => currency_symbol(&code, lang),
                };
                if ld.currency_prefix {
                    let alpha = sym.chars().last().map(|c| c.is_alphabetic()).unwrap_or(false);
                    f.mid_pre.push(("currency", sym));
                    if alpha || lang == "nl" {
                        f.mid_pre.push(("literal", "\u{a0}".to_string()));
                    }
                } else {
                    f.mid_suf.push(("literal", "\u{a0}".to_string()));
                    f.mid_suf.push(("currency", sym));
                }
            }
        }
        NfStyle::Percent => {
            if ld.percent.len() > 1 {
                f.mid_suf.push(("literal", "\u{a0}".to_string()));
            }
            f.mid_suf.push(("percentSign", "%".to_string()));
        }
        NfStyle::Unit => {
            let u = o.unit.clone().unwrap_or_default();
            let (pre, post) = parse_pattern(&unit_pattern(&u, o.unit_display, one, &ld), "unit");
            f.outer_pre = pre;
            f.outer_suf = post;
        }
        NfStyle::Decimal => {}
    }
    if accounting {
        f.mid_suf.push(("literal", ")".to_string()));
    }
    f
}

pub struct NumberParts {
    pub parts: Vec<(&'static str, String)>,
}

impl NumberParts {
    pub fn to_string(&self) -> String {
        self.parts.iter().map(|(_, s)| s.as_str()).collect()
    }
}

pub(super) fn format_value_parts(v: &MathValue, o: &NfOptions) -> NumberParts {
    NumberParts { parts: partition(o, v).into_parts() }
}

pub fn format_number_parts(x: f64, o: &NfOptions) -> NumberParts {
    format_value_parts(&MathValue::from_f64(x), o)
}

/// Format a BigInt given its decimal digits (no sign) exactly.
pub fn format_bigint_parts(digits: &str, negative: bool, o: &NfOptions) -> NumberParts {
    format_value_parts(&MathValue::from_decimal_digits(digits, negative), o)
}

pub fn format_number(x: f64, o: &NfOptions) -> String {
    format_number_parts(x, o).to_string()
}

/// PartitionNumberRangePattern (with ICU-style collapsing of shared
/// affixes): (type, value, source) triples.
fn partition_range(o: &NfOptions, x: &MathValue, y: &MathValue) -> Vec<(&'static str, String, &'static str)> {
    let fx = partition(o, x);
    let fy = partition(o, y);
    let px = fx.clone().into_parts();
    let py = fy.clone().into_parts();
    if Formatted::text(&px) == Formatted::text(&py) {
        // FormatApproximately
        let mut out: Vec<(&'static str, String, &'static str)> = fx.outer_pre.into_iter().map(|(t, v)| (t, v, "shared")).collect();
        out.push(("approximatelySign", "~".to_string(), "shared"));
        for (t, v) in fx.mid_pre.into_iter().chain(fx.number).chain(fx.mid_suf).chain(fx.outer_suf) {
            out.push((t, v, "shared"));
        }
        return out;
    }
    let ld = locale_data(o);
    let collapse_outer = fx.outer_pre == fy.outer_pre && fx.outer_suf == fy.outer_suf;
    let mid_len = Formatted::text(&fx.mid_pre).chars().count() + Formatted::text(&fx.mid_suf).chars().count();
    let collapse_middle = collapse_outer && fx.mid_pre == fy.mid_pre && fx.mid_suf == fy.mid_suf && mid_len > 1;
    let outer_len = Formatted::text(&fx.outer_pre).chars().count() + Formatted::text(&fx.outer_suf).chars().count();
    let repeat = (!collapse_middle && mid_len > 0) || (!collapse_outer && outer_len > 0);
    let mut sep = ld.range_sep.to_string();
    if repeat && !sep.starts_with(' ') {
        sep = format!(" {} ", sep);
    }
    let mut out: Vec<(&'static str, String, &'static str)> = Vec::new();
    let push = |out: &mut Vec<(&'static str, String, &'static str)>, parts: Vec<Part>, src: &'static str| {
        for (t, v) in parts {
            out.push((t, v, src));
        }
    };
    if collapse_outer {
        push(&mut out, fx.outer_pre.clone(), "shared");
        if collapse_middle {
            push(&mut out, fx.mid_pre.clone(), "shared");
            push(&mut out, fx.number, "startRange");
            out.push(("literal", sep, "shared"));
            push(&mut out, fy.number, "endRange");
            push(&mut out, fx.mid_suf, "shared");
        } else {
            push(&mut out, fx.mid_pre, "startRange");
            push(&mut out, fx.number, "startRange");
            push(&mut out, fx.mid_suf, "startRange");
            out.push(("literal", sep, "shared"));
            push(&mut out, fy.mid_pre, "endRange");
            push(&mut out, fy.number, "endRange");
            push(&mut out, fy.mid_suf, "endRange");
        }
        push(&mut out, fx.outer_suf, "shared");
    } else {
        push(&mut out, px, "startRange");
        out.push(("literal", sep, "shared"));
        push(&mut out, py, "endRange");
    }
    out
}

// ------------------------------------------------- PluralRules support

/// The number formatting state of an `Intl.PluralRules`: InitializePluralRules
/// reads `notation` and `compactDisplay` (already read by the caller) and
/// the digit options, and resolves the locale without extension keys.
pub(super) fn plural_nf_options(
    realm: &mut Realm,
    requested: &[String],
    opts: Option<ObjectId>,
    notation: &str,
    compact_display: &'static str,
    extra_languages: &[&str],
) -> JsResult<NfOptions> {
    let notation = match notation {
        "scientific" => Notation::Scientific,
        "engineering" => Notation::Engineering,
        "compact" => Notation::Compact,
        _ => Notation::Standard,
    };
    let Digits { min_int, min_frac, max_frac, min_sig, max_sig, rounding_type, rounding_priority, rounding_increment, rounding_mode, trailing_zero_display } =
        set_digit_options(realm, opts, 0, 3, notation)?;
    let (_, id, _) = resolve_nf_locale(realm, requested, None, extra_languages);
    Ok(NfOptions {
        locale: locale_id::to_string(&id),
        lang: id.language.clone(),
        region: id.region.clone().map(|r| r.to_ascii_uppercase()).unwrap_or_default(),
        script: id.script.clone().unwrap_or_default(),
        numbering_system: "latn".into(),
        style: NfStyle::Decimal,
        currency: None,
        currency_display: "symbol",
        currency_sign: "standard",
        unit: None,
        unit_display: "short",
        min_int,
        min_frac,
        max_frac,
        min_sig,
        max_sig,
        rounding_type,
        rounding_priority,
        rounding_increment,
        rounding_mode,
        trailing_zero_display,
        notation,
        compact_display,
        use_grouping: Grouping::False,
        sign_display: "auto",
    })
}

/// The resolvedOptions digit entries shared with PluralRules, in table
/// order: (minimumIntegerDigits, fraction digits, significant digits).
pub(super) fn digit_resolved(o: &NfOptions) -> (usize, Option<(usize, usize)>, Option<(usize, usize)>) {
    (o.min_int, o.min_frac.zip(o.max_frac), o.min_sig.zip(o.max_sig))
}

/// (roundingIncrement, roundingMode, roundingPriority, trailingZeroDisplay).
pub(super) fn rounding_resolved(o: &NfOptions) -> (u32, &'static str, &'static str, &'static str) {
    (o.rounding_increment, o.rounding_mode, o.rounding_priority, o.trailing_zero_display)
}

pub(super) fn notation_name(o: &NfOptions) -> (&'static str, Option<&'static str>) {
    match o.notation {
        Notation::Standard => ("standard", None),
        Notation::Scientific => ("scientific", None),
        Notation::Engineering => ("engineering", None),
        Notation::Compact => ("compact", Some(o.compact_display)),
    }
}

pub(super) fn locale_of(o: &NfOptions) -> (&str, &str) {
    (&o.locale, &o.lang)
}

/// The plural operands source of ResolvePlural: the formatted digits of
/// |x| ("1.50") and the compact/scientific exponent; None if not finite.
pub(super) fn plural_digits(o: &NfOptions, x: &MathValue) -> Option<(String, i64)> {
    let MathValue::Finite(neg, d) = x else { return None };
    let ld = locale_data(o);
    let exponent = compute_exponent(o, &ld, d);
    let (_, s) = format_numeric_to_string(o, *neg, &d.scale(-exponent));
    Some((s, exponent))
}

// ------------------------------------------------------------- natives

/// Captures of the NumberFormat natives: [%NumberFormat%, FallbackSymbol,
/// BoundFormat slot symbol].
fn captures(realm: &Realm) -> (Value, Option<SymbolId>, Option<SymbolId>) {
    let caps = realm.current_native().map(|n| realm.native_captures(n)).unwrap_or_default();
    let sym = |i: usize| caps.get(i).and_then(|v| if let V::Symbol(s) = v.v() { Some(s) } else { None });
    (caps.first().cloned().unwrap_or(Value::Undefined), sym(1), sym(2))
}

fn is_nf(realm: &Realm, v: &Value) -> bool {
    matches!(v.v(), V::Object(id) if realm.host_data::<NfOptions>(id).is_some())
}

/// UnwrapNumberFormat.
fn unwrap_nf(realm: &mut Realm, this: Value, what: &str) -> JsResult<Value> {
    if !this.is_object() {
        return realm.type_error(&format!("Method {} called on incompatible receiver", what));
    }
    if !is_nf(realm, &this) {
        let (ctor, fallback, _) = captures(realm);
        if let Some(sym) = fallback {
            if realm.ordinary_has_instance(ctor, this.clone())? {
                let v = realm.get(this, PropertyKey::Symbol(sym))?;
                if is_nf(realm, &v) {
                    return Ok(v);
                }
                return realm.type_error(&format!("Method {} called on incompatible receiver", what));
            }
        }
        return realm.type_error(&format!("Method {} called on incompatible receiver", what));
    }
    Ok(this)
}

fn nf_ctor(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = read_nf_options(realm, arg(args, 0), arg(args, 1))?;
    let nf = Value::Object(new_intl_object(realm, "Intl.NumberFormat", o)?);
    // ChainNumberFormat (normative optional legacy behaviour).
    if realm.new_target().is_undefined() && this.is_object() {
        let (ctor, fallback, _) = captures(realm);
        if let Some(sym) = fallback {
            if realm.ordinary_has_instance(ctor, this.clone())? {
                let id = this.as_object().unwrap();
                let desc = PropDesc { value: Some(nf), writable: Some(false), enumerable: Some(false), configurable: Some(false), ..Default::default() };
                realm.define_property_or_throw(id, PropertyKey::Symbol(sym), desc)?;
                return Ok(this);
            }
        }
    }
    Ok(nf)
}

fn nf_format_getter(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let nf = unwrap_nf(realm, this, "get Intl.NumberFormat.prototype.format")?;
    let (_, _, slot) = captures(realm);
    let id = nf.as_object().unwrap();
    if let Some(slot) = slot {
        if let Some(p) = realm.obj(id).props.get(&PropertyKey::Symbol(slot)) {
            return Ok(p.value.clone());
        }
    }
    let b = realm.new_native_function_full("", 1, nf_format_bound, None, vec![nf], false);
    if let Some(slot) = slot {
        realm.define_property(id, PropertyKey::Symbol(slot), Value::Object(b), 0);
    }
    Ok(Value::Object(b))
}

fn nf_format_bound(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let this = bound_this(realm);
    let o: NfOptions = host(realm, &this, "Intl.NumberFormat.prototype.format")?;
    let x = to_intl_mv(realm, arg(args, 0))?;
    Ok(Value::str(&format_value_parts(&x, &o).to_string()))
}

fn parts_array(realm: &mut Realm, parts: Vec<(&'static str, String, Option<&'static str>)>) -> Value {
    let mut out = Vec::with_capacity(parts.len());
    for (t, v, src) in parts {
        let obj = realm.new_object();
        realm.define_property(obj, "type", Value::str(t), ALL);
        realm.define_property(obj, "value", Value::str(&v), ALL);
        if let Some(s) = src {
            realm.define_property(obj, "source", Value::str(s), ALL);
        }
        out.push(Value::Object(obj));
    }
    realm.new_array_value(out)
}

fn nf_format_to_parts(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o: NfOptions = host(realm, &this, "Intl.NumberFormat.prototype.formatToParts")?;
    let x = to_intl_mv(realm, arg(args, 0))?;
    let parts = format_value_parts(&x, &o).parts.into_iter().map(|(t, v)| (t, v, None)).collect();
    Ok(parts_array(realm, parts))
}

fn range_args(realm: &mut Realm, this: &Value, args: &[Value], what: &str) -> JsResult<(NfOptions, MathValue, MathValue)> {
    let o: NfOptions = host(realm, this, what)?;
    let (a, b) = (arg(args, 0), arg(args, 1));
    if a.is_undefined() || b.is_undefined() {
        return realm.type_error("start and end are required");
    }
    let x = to_intl_mv(realm, a)?;
    let y = to_intl_mv(realm, b)?;
    if x == MathValue::NaN || y == MathValue::NaN {
        return realm.range_error("Invalid number range: NaN");
    }
    Ok((o, x, y))
}

fn nf_format_range(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, x, y) = range_args(realm, &this, args, "Intl.NumberFormat.prototype.formatRange")?;
    let s: String = partition_range(&o, &x, &y).into_iter().map(|p| p.1).collect();
    Ok(Value::str(&s))
}

fn nf_format_range_to_parts(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, x, y) = range_args(realm, &this, args, "Intl.NumberFormat.prototype.formatRangeToParts")?;
    let parts = partition_range(&o, &x, &y).into_iter().map(|(t, v, s)| (t, v, Some(s))).collect();
    Ok(parts_array(realm, parts))
}

fn nf_resolved_options(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let nf = unwrap_nf(realm, this, "Intl.NumberFormat.prototype.resolvedOptions")?;
    let o: NfOptions = host(realm, &nf, "Intl.NumberFormat.prototype.resolvedOptions")?;
    let obj = realm.new_object();
    let set_s = |realm: &mut Realm, k: &str, v: &str| realm.define_property(obj, k, Value::str(v), ALL);
    let set_n = |realm: &mut Realm, k: &str, v: f64| realm.define_property(obj, k, Value::Number(v), ALL);
    set_s(realm, "locale", &o.locale);
    set_s(realm, "numberingSystem", &o.numbering_system);
    set_s(
        realm,
        "style",
        match o.style {
            NfStyle::Decimal => "decimal",
            NfStyle::Currency => "currency",
            NfStyle::Percent => "percent",
            NfStyle::Unit => "unit",
        },
    );
    if let Some(c) = &o.currency {
        set_s(realm, "currency", c);
        set_s(realm, "currencyDisplay", o.currency_display);
        set_s(realm, "currencySign", o.currency_sign);
    }
    if let Some(u) = &o.unit {
        set_s(realm, "unit", u);
        set_s(realm, "unitDisplay", o.unit_display);
    }
    set_n(realm, "minimumIntegerDigits", o.min_int as f64);
    if let (Some(a), Some(b)) = (o.min_frac, o.max_frac) {
        set_n(realm, "minimumFractionDigits", a as f64);
        set_n(realm, "maximumFractionDigits", b as f64);
    }
    if let (Some(a), Some(b)) = (o.min_sig, o.max_sig) {
        set_n(realm, "minimumSignificantDigits", a as f64);
        set_n(realm, "maximumSignificantDigits", b as f64);
    }
    let g = match o.use_grouping {
        Grouping::False => Value::Bool(false),
        Grouping::Min2 => Value::str("min2"),
        Grouping::Auto => Value::str("auto"),
        Grouping::Always => Value::str("always"),
    };
    realm.define_property(obj, "useGrouping", g, ALL);
    set_s(
        realm,
        "notation",
        match o.notation {
            Notation::Standard => "standard",
            Notation::Scientific => "scientific",
            Notation::Engineering => "engineering",
            Notation::Compact => "compact",
        },
    );
    if o.notation == Notation::Compact {
        set_s(realm, "compactDisplay", o.compact_display);
    }
    set_s(realm, "signDisplay", o.sign_display);
    set_n(realm, "roundingIncrement", o.rounding_increment as f64);
    set_s(realm, "roundingMode", o.rounding_mode);
    set_s(realm, "roundingPriority", o.rounding_priority);
    set_s(realm, "trailingZeroDisplay", o.trailing_zero_display);
    Ok(Value::Object(obj))
}

/// A numeric argument as a Number (BigInts converted), for services that
/// format plain Numbers.
#[allow(dead_code)]
pub(super) fn number_arg(realm: &mut Realm, v: Value) -> JsResult<f64> {
    match (realm.to_numeric(v)?).v() {
        V::BigInt(b) => Ok(b.to_f64()),
        V::Number(n) => Ok(n),
        _ => Ok(f64::NAN),
    }
}

/// `Number.prototype.toLocaleString(locales, options)`.
pub fn number_to_locale_string(realm: &mut Realm, n: f64, args: &[Value]) -> JsResult<Value> {
    let o = read_nf_options(realm, arg(args, 0), arg(args, 1))?;
    Ok(Value::str(&format_number(n, &o)))
}

fn number_to_locale(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = match this.v() {
        V::Number(n) => n,
        V::Object(id) => match &realm.obj(id).kind {
            ObjectKind::Number(n) => *n,
            _ => return realm.type_error("Number.prototype.toLocaleString requires that 'this' be a Number"),
        },
        _ => return realm.type_error("Number.prototype.toLocaleString requires that 'this' be a Number"),
    };
    number_to_locale_string(realm, n, args)
}

fn bigint_to_locale(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = match this.v() {
        V::BigInt(b) => b.clone(),
        V::Object(id) => match &realm.obj(id).kind {
            ObjectKind::BigInt(b) => (**b).clone(),
            _ => return realm.type_error("BigInt.prototype.toLocaleString requires that 'this' be a BigInt"),
        },
        _ => return realm.type_error("BigInt.prototype.toLocaleString requires that 'this' be a BigInt"),
    };
    let o = read_nf_options(realm, arg(args, 0), arg(args, 1))?;
    Ok(Value::str(&format_bigint_parts(n.to_string_radix(10).trim_start_matches('-'), n.is_negative(), &o).to_string()))
}

/// Install `Intl.NumberFormat` on `intl` and the numeric `toLocaleString`s.
pub(super) fn register(realm: &mut Realm, intl: ObjectId) {
    let op = realm.intrinsics.object_proto;
    let proto = realm.new_object_with_proto(Some(op));
    let c = realm.new_native_constructor("NumberFormat", 0, nf_ctor, proto);
    realm.define_property(intl, "NumberFormat", Value::Object(c), HIDDEN);
    realm.define_property(proto, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("Intl.NumberFormat"), CONFIGURABLE);
    realm.define_method(c, "supportedLocalesOf", 1, supported_locales_of);
    realm.intrinsics.others.insert("Intl.NumberFormat", proto);

    let fallback = realm.new_symbol(Some(JsStr::new("IntlLegacyConstructedSymbol")));
    let slot = realm.new_private_symbol(JsStr::new("BoundFormat"));
    let caps = vec![Value::Object(c), Value::Symbol(fallback), Value::Symbol(slot)];
    if let ObjectKind::Native(n) = &mut realm.obj_mut(c).kind {
        n.captures = caps.clone();
    }
    let getter = realm.new_native_function_full("get format", 0, nf_format_getter, None, caps.clone(), false);
    realm.define_accessor_values(proto, "format", Some(Value::Object(getter)), None, CONFIGURABLE);
    realm.define_method(proto, "formatToParts", 1, nf_format_to_parts);
    realm.define_method(proto, "formatRange", 2, nf_format_range);
    realm.define_method(proto, "formatRangeToParts", 2, nf_format_range_to_parts);
    let ro = realm.new_native_function_full("resolvedOptions", 0, nf_resolved_options, None, caps, false);
    realm.define_property(proto, "resolvedOptions", Value::Object(ro), HIDDEN);

    let np = realm.intrinsics.number_proto;
    realm.define_method(np, "toLocaleString", 0, number_to_locale);
    let bp = realm.intrinsics.bigint_proto;
    realm.define_method(bp, "toLocaleString", 0, bigint_to_locale);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Dec {
        match MathValue::from_str(s) {
            MathValue::Finite(_, d) => d,
            _ => panic!(),
        }
    }

    #[test]
    fn rounding() {
        assert_eq!(round_dec(&d("1.25"), -1, 1, Unsigned::HalfEven).to_plain(), "1.2");
        assert_eq!(round_dec(&d("1.35"), -1, 1, Unsigned::HalfEven).to_plain(), "1.4");
        assert_eq!(round_dec(&d("1.3125"), -2, 25, Unsigned::HalfInfinity).to_plain(), "1.25");
        assert_eq!(round_dec(&d("1.375"), -2, 25, Unsigned::HalfInfinity).to_plain(), "1.5");
        assert_eq!(round_dec(&d("999.9"), 1, 1, Unsigned::HalfInfinity).to_plain(), "1000");
        assert_eq!(to_raw_precision(&d("0.0009999"), 3, 3, Unsigned::HalfInfinity).s, "0.00100");
        assert_eq!(to_raw_fixed(&d("12344501000000000000000000000000000"), 1, 3, 1, Unsigned::HalfInfinity).s, "12344501000000000000000000000000000.0");
    }
}
