//! `Intl.DurationFormat` (ECMA-402 §13).
//!
//! Units are formatted with the engine's own `Intl.NumberFormat` (unit
//! style, or numeric with a time separator) and joined with a unit-type
//! `Intl.ListFormat`, as PartitionDurationFormatPattern specifies.

use super::*;

/// The ten duration units in order, with their NumberFormat singular.
const UNITS: [(&str, &str); 10] = [
    ("years", "year"),
    ("months", "month"),
    ("weeks", "week"),
    ("days", "day"),
    ("hours", "hour"),
    ("minutes", "minute"),
    ("seconds", "second"),
    ("milliseconds", "millisecond"),
    ("microseconds", "microsecond"),
    ("nanoseconds", "nanosecond"),
];

#[derive(Debug, Clone)]
pub struct DurationOptions {
    locale: Locale,
    numbering_system: String,
    style: String,
    /// (style, display) per unit, in [`UNITS`] order.
    units: [(String, String); 10],
    fractional_digits: Option<u32>,
}

/// GetOptionsObject: undefined gives an empty bag; other non-objects throw.
fn get_options_object(realm: &mut Realm, v: Value) -> JsResult<Option<ObjectId>> {
    match v.v() {
        V::Undefined => Ok(None),
        V::Object(id) => Ok(Some(id)),
        _ => realm.type_error("Options must be an object"),
    }
}

/// GetOption with a list of allowed string values.
fn get_string_option(realm: &mut Realm, opts: Option<ObjectId>, name: &str, allowed: &[&str], default: Option<&str>) -> JsResult<Option<String>> {
    let v = match opts {
        Some(o) => realm.get_str(Value::Object(o), name)?,
        None => Value::Undefined,
    };
    if v.is_undefined() {
        return Ok(default.map(str::to_string));
    }
    let s = realm.to_rust_string(v)?;
    if !allowed.is_empty() && !allowed.contains(&s.as_str()) {
        return realm.range_error(&format!("Value {} out of range for Intl.DurationFormat options property {}", s, name));
    }
    Ok(Some(s))
}

/// A `type` sequence of the Unicode locale extension: (3*8alphanum) *("-" (3*8alphanum)).
pub(super) fn is_unicode_type(s: &str) -> bool {
    !s.is_empty() && s.split('-').all(|p| (3..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// GetDurationUnitOptions (§13.5.6).
#[allow(clippy::too_many_arguments)]
fn unit_options(
    realm: &mut Realm,
    opts: Option<ObjectId>,
    unit: &str,
    base_style: &str,
    styles: &[&str],
    digital_base: &str,
    prev_style: &str,
    two_digit_hours: bool,
) -> JsResult<(String, String)> {
    let mut style = get_string_option(realm, opts, unit, styles, None)?;
    let mut display_default = "always";
    if style.is_none() {
        if base_style == "digital" {
            if !matches!(unit, "hours" | "minutes" | "seconds") {
                display_default = "auto";
            }
            style = Some(digital_base.to_string());
        } else if matches!(prev_style, "fractional" | "numeric" | "2-digit") {
            if !matches!(unit, "minutes" | "seconds") {
                display_default = "auto";
            }
            style = Some("numeric".to_string());
        } else {
            display_default = "auto";
            style = Some(base_style.to_string());
        }
    }
    let mut style = style.unwrap();
    if style == "numeric" && matches!(unit, "milliseconds" | "microseconds" | "nanoseconds") {
        style = "fractional".to_string();
        display_default = "auto";
    }
    let display = get_string_option(realm, opts, &format!("{}Display", unit), &["auto", "always"], Some(display_default))?.unwrap();
    if display == "always" && style == "fractional" {
        return realm.range_error(&format!("Intl.DurationFormat: {}Display cannot be \"always\" for a fractional {} style", unit, unit));
    }
    if matches!(prev_style, "fractional" | "numeric" | "2-digit") {
        if !matches!(style.as_str(), "fractional" | "numeric" | "2-digit") {
            return realm.range_error(&format!("Intl.DurationFormat: {} style \"{}\" cannot follow a numeric style", unit, style));
        }
        if matches!(unit, "minutes" | "seconds") {
            style = "2-digit".to_string();
        }
    }
    if unit == "hours" && two_digit_hours {
        style = "2-digit".to_string();
    }
    Ok((style, display))
}

pub(super) fn ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if realm.new_target().is_undefined() {
        return realm.type_error("Constructor Intl.DurationFormat requires 'new'");
    }
    let requested = canonical_locale_list(realm, arg(args, 0))?;
    let opts = get_options_object(realm, arg(args, 1))?;
    get_string_option(realm, opts, "localeMatcher", &["lookup", "best fit"], Some("best fit"))?;
    let nu_opt = get_string_option(realm, opts, "numberingSystem", &[], None)?;
    if let Some(nu) = &nu_opt {
        if !is_unicode_type(nu) {
            return realm.range_error(&format!("Invalid numberingSystem: {}", nu));
        }
    }
    let mut locale = requested.first().and_then(|t| Locale::parse(t)).unwrap_or_else(Locale::default_locale);
    // ResolveLocale for the "nu" key: the option wins over the extension;
    // the extension stays in the locale only when it is the one used.
    let ext = locale.nu.clone().filter(|n| numbering_zero(n).is_some());
    let numbering_system = match (&nu_opt, &ext) {
        (Some(o), _) if numbering_zero(o).is_some() => o.clone(),
        (_, Some(e)) => e.clone(),
        _ => "latn".to_string(),
    };
    if ext.as_deref() == Some(numbering_system.as_str()) {
        locale.tag = format!("{}-u-nu-{}", locale.tag, numbering_system);
    }
    let style = get_string_option(realm, opts, "style", &["long", "short", "narrow", "digital"], Some("short"))?.unwrap();
    let mut prev = String::new();
    let mut units: [(String, String); 10] = Default::default();
    for (i, (unit, _)) in UNITS.iter().enumerate() {
        let (styles, digital_base): (&[&str], &str) = match i {
            0..=3 => (&["long", "short", "narrow"], "short"),
            4..=6 => (&["long", "short", "narrow", "numeric", "2-digit"], "numeric"),
            _ => (&["long", "short", "narrow", "numeric"], "numeric"),
        };
        let u = unit_options(realm, opts, unit, &style, styles, digital_base, &prev, false)?;
        if (4..=8).contains(&i) {
            prev = u.0.clone();
        }
        units[i] = u;
    }
    let fractional_digits = match opts {
        Some(o) => {
            let v = realm.get_str(Value::Object(o), "fractionalDigits")?;
            if v.is_undefined() {
                None
            } else {
                let n = realm.to_number(v)?;
                if n.is_nan() || !(0.0..=9.0).contains(&n) {
                    return realm.range_error("fractionalDigits value is out of range.");
                }
                Some(n.floor() as u32)
            }
        }
        None => None,
    };
    let o = DurationOptions { locale, numbering_system, style, units, fractional_digits };
    Ok(Value::Object(new_intl_object(realm, "Intl.DurationFormat", o)?))
}

/// A Duration Record: the ten units as exact integers.
type DurationRecord = [f64; 10];

/// ToIntegerIfIntegral.
fn integer_if_integral(realm: &mut Realm, v: Value) -> JsResult<f64> {
    let n = realm.to_number(v)?;
    if !n.is_finite() || n.fract() != 0.0 {
        return realm.range_error("Duration values must be integers");
    }
    Ok(n)
}

/// ToDurationRecord (with a minimal ISO 8601 duration string reader in
/// place of Temporal's ToTemporalDuration).
fn to_duration_record(realm: &mut Realm, input: Value) -> JsResult<DurationRecord> {
    let id = match input.v() {
        V::Object(id) => id,
        V::String(s) => {
            return match parse_iso_duration(&s.to_rust_string()) {
                Some(r) if is_valid_duration(&r) => Ok(r),
                _ => realm.range_error("Invalid duration string"),
            };
        }
        _ => return realm.type_error("Duration must be an object"),
    };
    let mut r: DurationRecord = [0.0; 10];
    let mut any = false;
    // Property reads in alphabetical order.
    for name in ["days", "hours", "microseconds", "milliseconds", "minutes", "months", "nanoseconds", "seconds", "weeks", "years"] {
        let v = realm.get_str(Value::Object(id), name)?;
        if !v.is_undefined() {
            any = true;
            let n = integer_if_integral(realm, v)?;
            let i = UNITS.iter().position(|(u, _)| *u == name).unwrap();
            r[i] = n;
        }
    }
    if !any {
        return realm.type_error("Duration object has no duration properties");
    }
    if !is_valid_duration(&r) {
        return realm.range_error("Invalid duration");
    }
    Ok(r)
}

/// `[-+]P[nY][nM][nW][nD][T[nH][nM][n[.f]S]]` (fractions only on the last
/// time unit, up to nanoseconds).
fn parse_iso_duration(s: &str) -> Option<DurationRecord> {
    let mut r: DurationRecord = [0.0; 10];
    let b = s.as_bytes();
    let mut i = 0;
    let sign = match b.first() {
        Some(b'-') => {
            i += 1;
            -1.0
        }
        Some(b'+') => {
            i += 1;
            1.0
        }
        _ => 1.0,
    };
    if !matches!(b.get(i), Some(b'P') | Some(b'p')) {
        return None;
    }
    i += 1;
    let mut time = false;
    let mut seen = false;
    while i < b.len() {
        if matches!(b[i], b'T' | b't') {
            if time {
                return None;
            }
            time = true;
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return None;
        }
        let int: f64 = s[start..i].parse().ok()?;
        let mut frac = String::new();
        if i < b.len() && (b[i] == b'.' || b[i] == b',') {
            i += 1;
            let fs = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i == fs || i - fs > 9 {
                return None;
            }
            frac = s[fs..i].to_string();
        }
        let unit = *b.get(i)? as char;
        i += 1;
        let idx = match (time, unit.to_ascii_uppercase()) {
            (false, 'Y') => 0,
            (false, 'M') => 1,
            (false, 'W') => 2,
            (false, 'D') => 3,
            (true, 'H') => 4,
            (true, 'M') => 5,
            (true, 'S') => 6,
            _ => return None,
        };
        r[idx] = int;
        if !frac.is_empty() {
            if idx != 6 {
                return None;
            }
            let ns: f64 = format!("{:0<9}", frac).parse().ok()?;
            r[7] = (ns / 1e6).floor();
            r[8] = ((ns / 1e3) % 1e3).floor();
            r[9] = ns % 1e3;
        }
        seen = true;
    }
    if !seen {
        return None;
    }
    for v in r.iter_mut() {
        *v *= sign;
        if *v == 0.0 {
            *v = 0.0;
        }
    }
    Some(r)
}

/// IsValidDuration (§13.5.5 / Temporal).
fn is_valid_duration(r: &DurationRecord) -> bool {
    let mut sign = 0.0;
    for &v in r {
        if !v.is_finite() {
            return false;
        }
        if v != 0.0 {
            let s = v.signum();
            if sign != 0.0 && s != sign {
                return false;
            }
            sign = s;
        }
    }
    if r[0].abs() >= 4294967296.0 || r[1].abs() >= 4294967296.0 || r[2].abs() >= 4294967296.0 {
        return false;
    }
    // |normalized seconds| < 2^53, computed exactly in nanoseconds.
    if r[3..].iter().any(|v| v.abs() >= 1e30) {
        return false;
    }
    let ns: i128 = (r[3] as i128) * 86_400_000_000_000 + (r[4] as i128) * 3_600_000_000_000 + (r[5] as i128) * 60_000_000_000 + (r[6] as i128) * 1_000_000_000 + (r[7] as i128) * 1_000_000 + (r[8] as i128) * 1_000 + r[9] as i128;
    ns.unsigned_abs() < (1u128 << 53) * 1_000_000_000
}

/// The value of `seconds`/`milliseconds`/`microseconds` with the smaller
/// units as an exact decimal fraction (ComputeFractionalDigits), as a string.
fn fractional_value(r: &DurationRecord, unit_index: usize) -> Value {
    let exponent: u32 = match unit_index {
        6 => 9,
        7 => 6,
        _ => 3,
    };
    if r[unit_index + 1..].iter().all(|v| *v == 0.0) {
        return Value::Number(r[unit_index]);
    }
    let mut ns: i128 = r[9] as i128;
    if exponent >= 3 {
        ns += (r[8] as i128) * 1_000;
    }
    if exponent >= 6 {
        ns += (r[7] as i128) * 1_000_000;
    }
    if exponent >= 9 {
        ns += (r[6] as i128) * 1_000_000_000;
    }
    let e = 10i128.pow(exponent);
    let q = ns / e;
    let rem = (ns % e).abs();
    let frac = format!("{:0>width$}", rem, width = exponent as usize);
    let frac = frac.trim_end_matches('0');
    let neg = ns < 0 && q == 0;
    let s = if frac.is_empty() { format!("{}", q) } else { format!("{}{}.{}", if neg { "-" } else { "" }, q, frac) };
    Value::str(&s)
}

/// One formatted part: (type, value, unit).
type Part = (&'static str, String, Option<&'static str>);

/// PartitionDurationFormatPattern.
fn partition(realm: &mut Realm, o: &DurationOptions, r: &DurationRecord) -> JsResult<Vec<Part>> {
    let mut result: Vec<Vec<Part>> = Vec::new();
    let mut need_separator = false;
    let mut display_negative_sign = true;
    let negative = r.iter().any(|v| *v < 0.0);
    for (i, (_, nf_unit)) in UNITS.iter().enumerate() {
        let (style, display) = (&o.units[i].0, &o.units[i].1);
        // Duration record fields are mathematical values: no negative zero.
        let mut value = Value::Number(if r[i] == 0.0 { 0.0 } else { r[i] });
        let mut nf: Vec<(&str, Value)> = Vec::new();
        let mut done = false;
        if (6..=8).contains(&i) && matches!(o.units[i + 1].0.as_str(), "numeric" | "fractional") {
            value = fractional_value(r, i);
            nf.push(("maximumFractionDigits", Value::Number(o.fractional_digits.unwrap_or(9) as f64)));
            nf.push(("minimumFractionDigits", Value::Number(o.fractional_digits.unwrap_or(0) as f64)));
            nf.push(("roundingMode", Value::str("trunc")));
            done = true;
        }
        let display_required = i == 5 && need_separator && (o.units[6].1 == "always" || r[6..].iter().any(|v| *v != 0.0));
        let is_zero = matches!(value.v(), V::Number(n) if n == 0.0);
        if !is_zero || display != "auto" || display_required {
            if display_negative_sign {
                display_negative_sign = false;
                if is_zero && negative {
                    value = Value::Number(-0.0);
                }
            } else {
                nf.push(("signDisplay", Value::str("never")));
            }
            nf.push(("numberingSystem", Value::str(&o.numbering_system)));
            if style == "2-digit" {
                nf.push(("minimumIntegerDigits", Value::Number(2.0)));
            }
            let numeric = matches!(style.as_str(), "numeric" | "2-digit" | "fractional");
            if !numeric {
                nf.push(("style", Value::str("unit")));
                nf.push(("unit", Value::str(nf_unit)));
                nf.push(("unitDisplay", Value::str(style)));
            } else {
                nf.push(("useGrouping", Value::Bool(false)));
            }
            let opts = realm.new_object();
            for (k, v) in nf {
                realm.define_property(opts, k, v, ALL);
            }
            let nfo = read_nf_options(realm, Value::str(&o.locale.tag), Value::Object(opts))?;
            let x = number_format::to_intl_mv(realm, value)?;
            let parts = number_format::format_value_parts(&x, &nfo);
            let mut list: Vec<Part> = if need_separator { result.pop().unwrap_or_default() } else { Vec::new() };
            if need_separator {
                list.push(("literal", ":".to_string(), None));
            }
            for (t, v) in parts.parts {
                list.push((t, v, Some(*nf_unit)));
            }
            if !need_separator && numeric {
                need_separator = true;
            }
            result.push(list);
        }
        if done {
            break;
        }
    }
    let list_style = if o.style == "digital" { "short" } else { o.style.as_str() };
    let lo = ListOptions { locale: o.locale.clone(), kind: "unit".into(), style: list_style.into() };
    let strings: Vec<String> = result.iter().map(|p| p.iter().map(|x| x.1.as_str()).collect()).collect();
    let mut out = Vec::new();
    let mut groups = result.into_iter();
    for (t, v) in list_parts(&strings, &lo) {
        if t == "element" {
            out.extend(groups.next().unwrap_or_default());
        } else {
            out.push(("literal", v, None));
        }
    }
    Ok(out)
}

pub(super) fn format(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o: DurationOptions = host(realm, &this, "Intl.DurationFormat.prototype.format")?;
    let r = to_duration_record(realm, arg(args, 0))?;
    let parts = partition(realm, &o, &r)?;
    let s: String = parts.iter().map(|p| p.1.as_str()).collect();
    Ok(Value::str(&s))
}

pub(super) fn format_to_parts(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o: DurationOptions = host(realm, &this, "Intl.DurationFormat.prototype.formatToParts")?;
    let r = to_duration_record(realm, arg(args, 0))?;
    let parts = partition(realm, &o, &r)?;
    let mut out = Vec::new();
    for (t, v, unit) in parts {
        let obj = realm.new_object();
        realm.define_property(obj, "type", Value::str(t), ALL);
        realm.define_property(obj, "value", Value::str(&v), ALL);
        if let Some(u) = unit {
            realm.define_property(obj, "unit", Value::str(u), ALL);
        }
        out.push(Value::Object(obj));
    }
    Ok(realm.new_array_value(out))
}

pub(super) fn resolved_options(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let o: DurationOptions = host(realm, &this, "Intl.DurationFormat.prototype.resolvedOptions")?;
    let obj = realm.new_object();
    realm.define_property(obj, "locale", Value::str(&o.locale.tag), ALL);
    realm.define_property(obj, "numberingSystem", Value::str(&o.numbering_system), ALL);
    realm.define_property(obj, "style", Value::str(&o.style), ALL);
    for (i, (unit, _)) in UNITS.iter().enumerate() {
        let style = if o.units[i].0 == "fractional" { "numeric" } else { o.units[i].0.as_str() };
        realm.define_property(obj, *unit, Value::str(style), ALL);
        realm.define_property(obj, format!("{}Display", unit).as_str(), Value::str(&o.units[i].1), ALL);
    }
    if let Some(d) = o.fractional_digits {
        realm.define_property(obj, "fractionalDigits", Value::Number(d as f64), ALL);
    }
    Ok(Value::Object(obj))
}
