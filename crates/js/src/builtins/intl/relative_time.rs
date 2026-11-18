//! `Intl.RelativeTimeFormat` (ECMA-402 §17).

use super::duration::{get_string_option, is_unicode_type};
use super::*;

#[derive(Debug, Clone)]
pub struct RelativeTimeOptions {
    locale: Locale,
    numbering_system: String,
    style: String,
    numeric: String,
}

/// CoerceOptionsToObject.
fn coerce_options(realm: &mut Realm, v: Value) -> JsResult<Option<ObjectId>> {
    if v.is_undefined() {
        return Ok(None);
    }
    Ok(Some(realm.to_object(v)?))
}

pub(super) fn ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if realm.new_target().is_undefined() {
        return realm.type_error("Constructor Intl.RelativeTimeFormat requires 'new'");
    }
    let mut locale = lookup_locale(realm, arg(args, 0))?;
    let opts = coerce_options(realm, arg(args, 1))?;
    get_string_option(realm, opts, "localeMatcher", &["lookup", "best fit"], Some("best fit"))?;
    let nu_opt = get_string_option(realm, opts, "numberingSystem", &[], None)?;
    if let Some(n) = &nu_opt {
        if !is_unicode_type(n) {
            return realm.range_error(&format!("Invalid numberingSystem: {}", n));
        }
    }
    let ext = locale.nu.clone().filter(|n| numbering_zero(n).is_some());
    let numbering_system = match (&nu_opt, &ext) {
        (Some(o), _) if numbering_zero(o).is_some() => o.clone(),
        (_, Some(e)) => e.clone(),
        _ => "latn".to_string(),
    };
    if ext.as_deref() == Some(numbering_system.as_str()) {
        locale.tag = format!("{}-u-nu-{}", locale.tag, numbering_system);
    }
    let style = get_string_option(realm, opts, "style", &["long", "short", "narrow"], Some("long"))?.unwrap();
    let numeric = get_string_option(realm, opts, "numeric", &["always", "auto"], Some("always"))?.unwrap();
    let o = RelativeTimeOptions { locale, numbering_system, style, numeric };
    Ok(Value::Object(new_intl_object(realm, "Intl.RelativeTimeFormat", o)?))
}

/// SingularRelativeTimeUnit.
fn singular_unit(unit: &str) -> Option<&'static str> {
    Some(match unit {
        "second" | "seconds" => "second",
        "minute" | "minutes" => "minute",
        "hour" | "hours" => "hour",
        "day" | "days" => "day",
        "week" | "weeks" => "week",
        "month" | "months" => "month",
        "quarter" | "quarters" => "quarter",
        "year" | "years" => "year",
        _ => return None,
    })
}

/// Plural category of |value| for the unit word.
fn plural(lang: &str, v: f64) -> &'static str {
    let integer = v.fract() == 0.0;
    match lang {
        "pl" => {
            if !integer {
                "other"
            } else if v == 1.0 {
                "one"
            } else {
                let i = v as i64;
                if (2..=4).contains(&(i % 10)) && !(12..=14).contains(&(i % 100)) {
                    "few"
                } else {
                    "many"
                }
            }
        }
        _ => {
            if integer && v == 1.0 {
                "one"
            } else {
                "other"
            }
        }
    }
}

/// The unit word for (language, style, unit, plural category).
fn unit_word(lang: &str, style: &str, unit: &str, cat: &str) -> String {
    match lang {
        "pl" => {
            let pick = |one: &str, few: &str, many: &str| -> String {
                match cat {
                    "one" => one,
                    "few" => few,
                    _ => many,
                }
                .to_string()
            };
            match (style, unit) {
                ("long", "second") => pick("sekundę", "sekundy", "sekund"),
                ("long", "minute") => pick("minutę", "minuty", "minut"),
                ("long", "hour") => pick("godzinę", "godziny", "godzin"),
                ("long", "week") => pick("tydzień", "tygodnie", "tygodni"),
                ("long", "month") => pick("miesiąc", "miesiące", "miesięcy"),
                ("long", "quarter") => pick("kwartał", "kwartały", "kwartałów"),
                (_, "day") => pick("dzień", "dni", "dni"),
                (_, "year") => pick("rok", "lata", "lat"),
                ("short", "second") => "sek.".into(),
                ("narrow", "second") => "s".into(),
                (_, "minute") => "min".into(),
                ("short", "hour") => "godz.".into(),
                ("narrow", "hour") => "g.".into(),
                (_, "week") => pick("tydz.", "tyg.", "tyg."),
                (_, "month") => "mies.".into(),
                _ => "kw.".into(),
            }
        }
        "es" => {
            let (one, other) = match unit {
                "second" => ("segundo", "segundos"),
                "minute" => ("minuto", "minutos"),
                "hour" => ("hora", "horas"),
                "day" => ("día", "días"),
                "week" => ("semana", "semanas"),
                "month" => ("mes", "meses"),
                "quarter" => ("trimestre", "trimestres"),
                _ => ("año", "años"),
            };
            if style == "long" {
                if cat == "one" { one } else { other }.to_string()
            } else {
                match unit {
                    "second" => "s".into(),
                    "minute" => "min".into(),
                    "hour" => "h".into(),
                    "day" => if cat == "one" { "día" } else { "días" }.into(),
                    "week" => "sem.".into(),
                    "month" => "m.".into(),
                    "quarter" => "trim.".into(),
                    _ => "a".into(),
                }
            }
        }
        _ => match style {
            "long" => {
                if cat == "one" {
                    unit.to_string()
                } else {
                    format!("{}s", unit)
                }
            }
            _ => match unit {
                "second" => "sec.".into(),
                "minute" => "min.".into(),
                "hour" => "hr.".into(),
                "day" => if cat == "one" { "day" } else { "days" }.into(),
                "week" => "wk.".into(),
                "month" => "mo.".into(),
                "quarter" => if cat == "one" { "qtr." } else { "qtrs." }.into(),
                _ => "yr.".into(),
            },
        },
    }
}

/// (prefix, suffix) around the number for past/future.
fn affixes(lang: &str, past: bool) -> (&'static str, &'static str) {
    match (lang, past) {
        ("pl", false) => ("za ", " "),
        ("pl", true) => ("", " temu"),
        ("es", false) => ("dentro de ", " "),
        ("es", true) => ("hace ", " "),
        (_, false) => ("in ", " "),
        (_, true) => ("", " ago"),
    }
}

/// The `numeric: "auto"` phrase for -1, 0, 1, if the locale has one.
fn auto_phrase(lang: &str, unit: &str, v: f64) -> Option<&'static str> {
    let n = if v == 0.0 { 0 } else if v == 1.0 { 1 } else if v == -1.0 { -1 } else { return None };
    Some(match (lang, unit, n) {
        ("en", "day", 0) => "today",
        ("en", "day", 1) => "tomorrow",
        ("en", "day", -1) => "yesterday",
        ("en", "second", 0) => "now",
        ("en", u, 0) => match u {
            "year" => "this year",
            "quarter" => "this quarter",
            "month" => "this month",
            "week" => "this week",
            "hour" => "this hour",
            _ => "this minute",
        },
        ("en", u, 1) => match u {
            "year" => "next year",
            "quarter" => "next quarter",
            "month" => "next month",
            "week" => "next week",
            _ => return None,
        },
        ("en", u, -1) => match u {
            "year" => "last year",
            "quarter" => "last quarter",
            "month" => "last month",
            "week" => "last week",
            _ => return None,
        },
        ("es", "day", 0) => "hoy",
        ("es", "day", 1) => "mañana",
        ("es", "day", -1) => "ayer",
        ("es", "second", 0) => "ahora",
        ("es", u, 0) => match u {
            "year" => "este año",
            "quarter" => "este trimestre",
            "month" => "este mes",
            "week" => "esta semana",
            "hour" => "esta hora",
            _ => "este minuto",
        },
        ("es", u, 1) => match u {
            "year" => "el próximo año",
            "month" => "el próximo mes",
            "week" => "la próxima semana",
            "quarter" => "el próximo trimestre",
            _ => return None,
        },
        ("es", u, -1) => match u {
            "year" => "el año pasado",
            "month" => "el mes pasado",
            "week" => "la semana pasada",
            "quarter" => "el trimestre pasado",
            _ => return None,
        },
        _ => return None,
    })
}

type Part = (&'static str, String, Option<&'static str>);

/// PartitionRelativeTimePattern.
fn partition(realm: &mut Realm, o: &RelativeTimeOptions, args: &[Value]) -> JsResult<Vec<Part>> {
    let value = realm.to_number(arg(args, 0))?;
    let unit_s = realm.to_rust_string(arg(args, 1))?;
    if !value.is_finite() {
        return realm.range_error("Value need to be finite number for Intl.RelativeTimeFormat.prototype.format()");
    }
    let Some(unit) = singular_unit(&unit_s) else {
        return realm.range_error(&format!("Invalid unit argument for format() '{}'", unit_s));
    };
    let lang = o.locale.lang.as_str();
    if o.numeric == "auto" {
        if let Some(p) = auto_phrase(lang, unit, value) {
            return Ok(vec![("literal", p.to_string(), None)]);
        }
    }
    let past = value < 0.0 || (value == 0.0 && value.is_sign_negative());
    let abs = value.abs();
    let nf_opts = realm.new_object();
    realm.define_property(nf_opts, "numberingSystem", Value::str(&o.numbering_system), ALL);
    let nfo = read_nf_options(realm, Value::str(&o.locale.tag), Value::Object(nf_opts))?;
    let parts = format_number_parts(abs, &nfo);
    let word = unit_word(lang, &o.style, unit, plural(lang, abs));
    let (pre, post) = affixes(lang, past);
    let mut out: Vec<Part> = Vec::new();
    if !pre.is_empty() {
        out.push(("literal", pre.to_string(), None));
    }
    for (t, v) in parts.parts {
        out.push((t, v, Some(unit)));
    }
    let suffix = if past {
        if lang == "es" {
            format!("{}{}", post, word)
        } else {
            format!(" {}{}", word, post)
        }
    } else {
        format!("{}{}", post, word)
    };
    out.push(("literal", suffix, None));
    Ok(out)
}

pub(super) fn format(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o: RelativeTimeOptions = host(realm, &this, "Intl.RelativeTimeFormat.prototype.format")?;
    let parts = partition(realm, &o, args)?;
    let s: String = parts.iter().map(|p| p.1.as_str()).collect();
    Ok(Value::str(&s))
}

pub(super) fn format_to_parts(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o: RelativeTimeOptions = host(realm, &this, "Intl.RelativeTimeFormat.prototype.formatToParts")?;
    let parts = partition(realm, &o, args)?;
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
    let o: RelativeTimeOptions = host(realm, &this, "Intl.RelativeTimeFormat.prototype.resolvedOptions")?;
    let obj = realm.new_object();
    realm.define_property(obj, "locale", Value::str(&o.locale.tag), ALL);
    realm.define_property(obj, "style", Value::str(&o.style), ALL);
    realm.define_property(obj, "numeric", Value::str(&o.numeric), ALL);
    realm.define_property(obj, "numberingSystem", Value::str(&o.numbering_system), ALL);
    Ok(Value::Object(obj))
}
