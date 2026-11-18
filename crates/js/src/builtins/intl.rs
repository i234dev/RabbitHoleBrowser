//! A small `Intl` implementation: `DateTimeFormat`, `NumberFormat`,
//! `Collator`, `PluralRules`, `RelativeTimeFormat`, `ListFormat` and the
//! locale-sensitive `toLocaleString`/`localeCompare` methods.
//!
//! Locale data is built in for English (US and other regions), Spanish,
//! German, French, Italian, Portuguese, Dutch and East Asian numerics;
//! other locales fall back to English conventions. The default locale is
//! `en-US`.

use super::date::{fields, local_fields, local_tza, long_zone_name, tz, DAY_LONG, DAY_NAMES, MONTH_LONG, MONTH_NAMES};
use crate::value::V;
use super::*;
use crate::realm::SYM_TO_STRING_TAG;

mod duration;
pub mod locale_id;
mod locale_obj;
mod list_format;
mod relative_time;
pub use list_format::{format_list, ListOptions};
use list_format::list_parts;

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

fn opt_bool(realm: &mut Realm, opts: Option<ObjectId>, name: &str) -> JsResult<Option<bool>> {
    let Some(o) = opts else { return Ok(None) };
    let v = realm.get_str(Value::Object(o), name)?;
    if v.is_undefined() {
        return Ok(None);
    }
    Ok(Some(v.truthy()))
}

fn opt_num(realm: &mut Realm, opts: Option<ObjectId>, name: &str, min: f64, max: f64) -> JsResult<Option<usize>> {
    let Some(o) = opts else { return Ok(None) };
    let v = realm.get_str(Value::Object(o), name)?;
    if v.is_undefined() {
        return Ok(None);
    }
    let n = realm.to_number(v)?;
    if n.is_nan() || n < min || n > max {
        return realm.range_error(&format!("{} value is out of range.", name));
    }
    Ok(Some(n.floor() as usize))
}

// ------------------------------------------------------- DateTimeFormat

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateDefaults {
    DateTime,
    Date,
    Time,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Width {
    Numeric,
    TwoDigit,
    Long,
    Short,
    Narrow,
}

fn parse_width(s: &str) -> Option<Width> {
    Some(match s {
        "numeric" => Width::Numeric,
        "2-digit" => Width::TwoDigit,
        "long" => Width::Long,
        "short" => Width::Short,
        "narrow" => Width::Narrow,
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    Full,
    Long,
    Medium,
    Short,
}

fn parse_style(s: &str) -> Option<Style> {
    Some(match s {
        "full" => Style::Full,
        "long" => Style::Long,
        "medium" => Style::Medium,
        "short" => Style::Short,
        _ => return None,
    })
}

#[derive(Debug, Clone)]
pub struct DtOptions {
    locale: Locale,
    weekday: Option<Width>,
    year: Option<Width>,
    month: Option<Width>,
    day: Option<Width>,
    hour: Option<Width>,
    minute: Option<Width>,
    second: Option<Width>,
    fractional: usize,
    hour12: Option<bool>,
    /// Some("UTC") for UTC, None for local.
    time_zone: Option<String>,
    time_zone_name: Option<Width>,
    date_style: Option<Style>,
    time_style: Option<Style>,
}

impl DtOptions {
    fn has_components(&self) -> bool {
        self.weekday.is_some() || self.year.is_some() || self.month.is_some() || self.day.is_some() || self.hour.is_some() || self.minute.is_some() || self.second.is_some() || self.fractional > 0 || self.time_zone_name.is_some()
    }
}

fn read_dt_options(realm: &mut Realm, locales: Value, options: Value, defaults: DateDefaults) -> JsResult<DtOptions> {
    let locale = resolve_locale(realm, locales)?;
    let opts = options_object(realm, options)?;
    let w = |realm: &mut Realm, name: &str| -> JsResult<Option<Width>> {
        match opt_str(realm, opts, name)? {
            Some(s) => match parse_width(&s) {
                Some(w) => Ok(Some(w)),
                None => realm.range_error(&format!("Value {} out of range for Intl.DateTimeFormat options property {}", s, name)),
            },
            None => Ok(None),
        }
    };
    let mut o = DtOptions {
        locale,
        weekday: w(realm, "weekday")?,
        year: w(realm, "year")?,
        month: w(realm, "month")?,
        day: w(realm, "day")?,
        hour: w(realm, "hour")?,
        minute: w(realm, "minute")?,
        second: w(realm, "second")?,
        fractional: opt_num(realm, opts, "fractionalSecondDigits", 1.0, 3.0)?.unwrap_or(0),
        hour12: opt_bool(realm, opts, "hour12")?,
        time_zone: None,
        time_zone_name: w(realm, "timeZoneName")?,
        date_style: None,
        time_style: None,
    };
    if let Some(hc) = opt_str(realm, opts, "hourCycle")? {
        if o.hour12.is_none() {
            o.hour12 = Some(hc == "h11" || hc == "h12");
        }
    }
    if let Some(ds) = opt_str(realm, opts, "dateStyle")? {
        o.date_style = Some(parse_style(&ds).ok_or_else(|| realm.throw_range_error("Invalid dateStyle"))?);
    }
    if let Some(ts) = opt_str(realm, opts, "timeStyle")? {
        o.time_style = Some(parse_style(&ts).ok_or_else(|| realm.throw_range_error("Invalid timeStyle"))?);
    }
    if (o.date_style.is_some() || o.time_style.is_some()) && o.has_components() {
        return realm.type_error("Can't set option dateStyle/timeStyle together with other date-time components");
    }
    if let Some(tz_name) = opt_str(realm, opts, "timeZone")? {
        let up = tz_name.to_ascii_uppercase();
        if up == "UTC" || up == "GMT" || up == "ETC/UTC" || up == "ETC/GMT" || up == "Z" {
            o.time_zone = Some("UTC".into());
        } else if tz_name.eq_ignore_ascii_case(&tz().name) {
            o.time_zone = None;
        } else if let Some(info) = super::date::tz_named(&tz_name) {
            o.time_zone = Some(info.name.clone());
        } else {
            return realm.range_error(&format!("Invalid time zone specified: {}", tz_name));
        }
    }
    if o.date_style.is_none() && o.time_style.is_none() && !o.has_components() {
        match defaults {
            DateDefaults::Date | DateDefaults::DateTime => {
                o.year = Some(Width::Numeric);
                o.month = Some(Width::Numeric);
                o.day = Some(Width::Numeric);
            }
            DateDefaults::Time => {}
        }
        match defaults {
            DateDefaults::Time | DateDefaults::DateTime => {
                o.hour = Some(Width::Numeric);
                o.minute = Some(Width::TwoDigit);
                o.second = Some(Width::TwoDigit);
            }
            DateDefaults::Date => {}
        }
    } else if o.date_style.is_none() && o.time_style.is_none() {
        // `toLocaleDateString` with only time components (or vice versa)
        // still adds the default date / time parts.
        let has_date = o.weekday.is_some() || o.year.is_some() || o.month.is_some() || o.day.is_some();
        let has_time = o.hour.is_some() || o.minute.is_some() || o.second.is_some() || o.fractional > 0;
        let need_date = (defaults == DateDefaults::Date && !has_date) || (defaults == DateDefaults::DateTime && !has_date && !has_time);
        let need_time = (defaults == DateDefaults::Time && !has_time) || (defaults == DateDefaults::DateTime && !has_date && !has_time);
        if need_date {
            o.year = Some(Width::Numeric);
            o.month = Some(Width::Numeric);
            o.day = Some(Width::Numeric);
        }
        if need_time {
            o.hour = Some(Width::Numeric);
            o.minute = Some(Width::TwoDigit);
            o.second = Some(Width::TwoDigit);
        }
    }
    if let Some(s) = o.date_style {
        match s {
            Style::Full => {
                o.weekday = Some(Width::Long);
                o.month = Some(Width::Long);
            }
            Style::Long => o.month = Some(Width::Long),
            Style::Medium => o.month = Some(Width::Short),
            Style::Short => o.month = Some(Width::Numeric),
        }
        o.year = Some(if s == Style::Short && o.locale.lang != "en" { Width::TwoDigit } else { Width::Numeric });
        o.day = Some(Width::Numeric);
    }
    if let Some(s) = o.time_style {
        o.hour = Some(Width::Numeric);
        o.minute = Some(Width::TwoDigit);
        if s != Style::Short {
            o.second = Some(Width::TwoDigit);
        }
        match s {
            Style::Full => o.time_zone_name = Some(Width::Long),
            Style::Long => o.time_zone_name = Some(Width::Short),
            _ => {}
        }
    }
    Ok(o)
}

struct LocaleNames {
    months_long: [&'static str; 12],
    months_short: [&'static str; 12],
    days_long: [&'static str; 7],
    days_short: [&'static str; 7],
}

const ES: LocaleNames = LocaleNames {
    months_long: ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"],
    months_short: ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sept", "oct", "nov", "dic"],
    days_long: ["domingo", "lunes", "martes", "miércoles", "jueves", "viernes", "sábado"],
    days_short: ["dom", "lun", "mar", "mié", "jue", "vie", "sáb"],
};
const DE: LocaleNames = LocaleNames {
    months_long: ["Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober", "November", "Dezember"],
    months_short: ["Jan.", "Feb.", "März", "Apr.", "Mai", "Juni", "Juli", "Aug.", "Sept.", "Okt.", "Nov.", "Dez."],
    days_long: ["Sonntag", "Montag", "Dienstag", "Mittwoch", "Donnerstag", "Freitag", "Samstag"],
    days_short: ["So.", "Mo.", "Di.", "Mi.", "Do.", "Fr.", "Sa."],
};
const FR: LocaleNames = LocaleNames {
    months_long: ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre"],
    months_short: ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."],
    days_long: ["dimanche", "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi"],
    days_short: ["dim.", "lun.", "mar.", "mer.", "jeu.", "ven.", "sam."],
};
const IT: LocaleNames = LocaleNames {
    months_long: ["gennaio", "febbraio", "marzo", "aprile", "maggio", "giugno", "luglio", "agosto", "settembre", "ottobre", "novembre", "dicembre"],
    months_short: ["gen", "feb", "mar", "apr", "mag", "giu", "lug", "ago", "set", "ott", "nov", "dic"],
    days_long: ["domenica", "lunedì", "martedì", "mercoledì", "giovedì", "venerdì", "sabato"],
    days_short: ["dom", "lun", "mar", "mer", "gio", "ven", "sab"],
};
const PT: LocaleNames = LocaleNames {
    months_long: ["janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro", "novembro", "dezembro"],
    months_short: ["jan.", "fev.", "mar.", "abr.", "mai.", "jun.", "jul.", "ago.", "set.", "out.", "nov.", "dez."],
    days_long: ["domingo", "segunda-feira", "terça-feira", "quarta-feira", "quinta-feira", "sexta-feira", "sábado"],
    days_short: ["dom.", "seg.", "ter.", "qua.", "qui.", "sex.", "sáb."],
};
const EN: LocaleNames = LocaleNames { months_long: MONTH_LONG, months_short: MONTH_NAMES, days_long: DAY_LONG, days_short: DAY_NAMES };

fn names_for(lang: &str) -> &'static LocaleNames {
    match lang {
        "es" => &ES,
        "de" => &DE,
        "fr" => &FR,
        "it" => &IT,
        "pt" => &PT,
        _ => &EN,
    }
}

fn two(n: f64) -> String {
    format!("{:02}", n as i64)
}

/// Format a time value with resolved options.
pub fn format_date(t: f64, o: &DtOptions) -> String {
    if t.is_nan() {
        return "Invalid Date".into();
    }
    let utc = o.time_zone.as_deref() == Some("UTC");
    let zone = o.time_zone.as_deref().filter(|z| *z != "UTC").and_then(super::date::tz_named);
    let f = if utc {
        fields(t)
    } else if let Some(z) = zone {
        fields(t + super::date::zone_offset(z, t).0)
    } else {
        local_fields(t)
    };
    let lang = o.locale.lang.as_str();
    let region = o.locale.region.as_deref().unwrap_or("");
    let names = names_for(lang);
    let us = lang == "en" && (region == "US" || region.is_empty() || region == "PH");
    let m = f.month as usize;
    let wd = f.weekday as usize;

    // ---- date part
    let mut date = String::new();
    let textual_month = matches!(o.month, Some(Width::Long | Width::Short | Width::Narrow));
    let year_s = o.year.map(|w| if w == Width::TwoDigit { two(f.year.rem_euclid(100.0)) } else { format!("{}", f.year as i64) });
    let day_s = o.day.map(|w| if w == Width::TwoDigit { two(f.day) } else { format!("{}", f.day as i64) });
    let weekday_s = o.weekday.map(|w| match w {
        Width::Long => names.days_long[wd].to_string(),
        Width::Narrow => names.days_short[wd].chars().take(1).collect::<String>().to_uppercase(),
        _ => names.days_short[wd].to_string(),
    });
    if textual_month {
        let month_s = match o.month {
            Some(Width::Long) => names.months_long[m].to_string(),
            Some(Width::Narrow) => names.months_long[m].chars().take(1).collect::<String>().to_uppercase(),
            _ => names.months_short[m].to_string(),
        };
        match lang {
            "es" | "pt" => {
                let mut parts = Vec::new();
                if let Some(d) = &day_s {
                    parts.push(d.clone());
                }
                parts.push(month_s);
                if let Some(y) = &year_s {
                    parts.push(y.clone());
                }
                date = parts.join(" de ");
                if let Some(w) = weekday_s {
                    date = format!("{}, {}", w, date);
                }
            }
            "de" => {
                if let Some(d) = &day_s {
                    date.push_str(&format!("{}. ", d));
                }
                date.push_str(&month_s);
                if let Some(y) = &year_s {
                    date.push_str(&format!(" {}", y));
                }
                if let Some(w) = weekday_s {
                    date = format!("{}, {}", w, date);
                }
            }
            "fr" | "it" | "nl" => {
                if let Some(d) = &day_s {
                    date.push_str(&format!("{} ", d));
                }
                date.push_str(&month_s);
                if let Some(y) = &year_s {
                    date.push_str(&format!(" {}", y));
                }
                if let Some(w) = weekday_s {
                    date = format!("{} {}", w, date);
                }
            }
            "en" if !us => {
                if let Some(d) = &day_s {
                    date.push_str(&format!("{} ", d));
                }
                date.push_str(&month_s);
                if let Some(y) = &year_s {
                    date.push_str(&format!(" {}", y));
                }
                if let Some(w) = weekday_s {
                    date = format!("{}, {}", w, date);
                }
            }
            _ => {
                date.push_str(&month_s);
                if let Some(d) = &day_s {
                    date.push_str(&format!(" {}", d));
                }
                if let Some(y) = &year_s {
                    if day_s.is_some() {
                        date.push(',');
                    }
                    date.push_str(&format!(" {}", y));
                }
                if let Some(w) = weekday_s {
                    date = format!("{}, {}", w, date);
                }
            }
        }
    } else if o.year.is_some() || o.month.is_some() || o.day.is_some() {
        let month_s = o.month.map(|w| if w == Width::TwoDigit { two(f.month + 1.0) } else { format!("{}", m + 1) });
        let (sep, order): (&str, [usize; 3]) = match lang {
            "en" if us => ("/", [1, 2, 0]),
            "en" => ("/", [2, 1, 0]),
            "de" | "ru" | "pl" | "cs" | "fi" | "nb" | "da" | "tr" => (".", [2, 1, 0]),
            "ja" | "zh" | "ko" | "hu" | "lt" => ("/", [0, 1, 2]),
            "nl" => ("-", [2, 1, 0]),
            "sv" => ("-", [0, 1, 2]),
            _ => ("/", [2, 1, 0]),
        };
        // Locales that zero-pad day/month by default in numeric form.
        let pad = matches!(lang, "en" if !us) || matches!(lang, "fr" | "ru" | "pl" | "nl" | "pt" if o.day == Some(Width::Numeric));
        let comps = [year_s.clone(), month_s.map(|s| if pad && s.len() == 1 { format!("0{}", s) } else { s }), day_s.clone().map(|s| if pad && s.len() == 1 { format!("0{}", s) } else { s })];
        let mut parts = Vec::new();
        for idx in order {
            if let Some(c) = &comps[idx] {
                parts.push(c.clone());
            }
        }
        date = parts.join(sep);
        if lang == "ja" || lang == "zh" || lang == "ko" {
            if o.month.is_some() && o.year.is_none() && o.day.is_none() {
                date = format!("{}月", m + 1);
            }
        }
        if let Some(w) = weekday_s {
            date = if lang == "en" || lang == "es" || lang == "de" || lang == "it" || lang == "pt" { format!("{}, {}", w, date) } else { format!("{} {}", w, date) };
        }
    } else if let Some(w) = weekday_s {
        date = w;
    }

    // ---- time part
    let mut time = String::new();
    if o.hour.is_some() || o.minute.is_some() || o.second.is_some() {
        let hour12 = o.hour12.unwrap_or(us || region == "IN" || lang == "ar" || lang == "hi" || (lang == "en" && (region == "CA" || region == "AU" || region == "NZ")));
        let mut h = f.hour;
        let mut suffix = String::new();
        if hour12 {
            suffix = if h < 12.0 { " AM" } else { " PM" }.to_string();
            h %= 12.0;
            if h == 0.0 {
                h = 12.0;
            }
            if lang == "es" {
                suffix = if f.hour < 12.0 { "\u{a0}a.\u{a0}m." } else { "\u{a0}p.\u{a0}m." }.to_string();
            }
        }
        let mut parts = Vec::new();
        if let Some(w) = o.hour {
            let pads_hour = matches!(lang, "de" | "fr" | "it" | "pt" | "ru" | "pl" | "nl" | "cs" | "sv" | "nb" | "da" | "fi" | "tr" | "hu" | "uk" | "ro");
            let pad = w == Width::TwoDigit || (!hour12 && pads_hour) || (!hour12 && lang == "en" && !us);
            parts.push(if pad { two(h) } else { format!("{}", h as i64) });
        }
        if o.minute.is_some() {
            parts.push(if o.hour.is_some() { two(f.minute) } else { format!("{}", f.minute as i64) });
        }
        if o.second.is_some() {
            let mut s = if o.minute.is_some() { two(f.second) } else { format!("{}", f.second as i64) };
            if o.fractional > 0 {
                let frac = format!("{:03}", f.ms as i64);
                s.push('.');
                s.push_str(&frac[..o.fractional]);
            }
            parts.push(s);
        }
        time = parts.join(":");
        time.push_str(&suffix);
        if let Some(w) = o.time_zone_name {
            let name = if utc {
                "UTC".to_string()
            } else {
                let (off, abbr) = match zone {
                    Some(z) => super::date::zone_offset(z, t),
                    None => local_tza(t),
                };
                match w {
                    Width::Long => long_zone_name(&abbr),
                    _ => {
                        if abbr.chars().all(|c| c.is_ascii_alphabetic()) && !abbr.is_empty() {
                            abbr
                        } else {
                            let mins = (off / 60000.0).round() as i64;
                            format!("GMT{}{}", if mins < 0 { '-' } else { '+' }, (mins.abs() / 60).to_string() + &if mins % 60 != 0 { format!(":{:02}", mins.abs() % 60) } else { String::new() })
                        }
                    }
                }
            };
            time.push(' ');
            time.push_str(&name);
        }
    }

    match (date.is_empty(), time.is_empty()) {
        (false, false) => {
            let joiner = match (lang, o.date_style) {
                ("en", Some(Style::Full | Style::Long)) => " at ",
                ("fr", _) => " ",
                ("de", Some(Style::Full | Style::Long)) => " um ",
                ("es", Some(Style::Full | Style::Long)) => ", ",
                _ => ", ",
            };
            format!("{}{}{}", date, joiner, time)
        }
        (true, false) => time,
        _ => date,
    }
}

/// `Date.prototype.toLocale{,Date,Time}String`.
pub fn date_to_locale(realm: &mut Realm, t: f64, args: &[Value], defaults: DateDefaults) -> JsResult<Value> {
    if t.is_nan() {
        return Ok(Value::str("Invalid Date"));
    }
    let o = read_dt_options(realm, arg(args, 0), arg(args, 1), defaults)?;
    Ok(Value::str(&format_date(t, &o)))
}

// --------------------------------------------------------- NumberFormat

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NfStyle {
    Decimal,
    Currency,
    Percent,
    Unit,
}

#[derive(Debug, Clone)]
pub struct NfOptions {
    locale: Locale,
    style: NfStyle,
    currency: Option<String>,
    currency_display: String,
    unit: Option<String>,
    unit_display: String,
    min_int: usize,
    min_frac: usize,
    max_frac: usize,
    min_sig: Option<usize>,
    max_sig: Option<usize>,
    use_grouping: bool,
    compact: bool,
    compact_long: bool,
    sign_display: String,
    numbering_system: String,
}

fn currency_digits(code: &str) -> usize {
    match code {
        "JPY" | "KRW" | "VND" | "CLP" | "ISK" | "HUF" | "IDR" | "PYG" | "UGX" | "XAF" | "XOF" => 0,
        "BHD" | "IQD" | "JOD" | "KWD" | "LYD" | "OMR" | "TND" => 3,
        _ => 2,
    }
}

fn currency_symbol(code: &str, lang: &str) -> String {
    match code {
        "USD" => if lang == "en" { "$" } else { "US$" }.into(),
        "EUR" => "€".into(),
        "GBP" => "£".into(),
        "JPY" => "¥".into(),
        "CNY" => "CN¥".into(),
        "INR" => "₹".into(),
        "KRW" => "₩".into(),
        "BRL" => "R$".into(),
        "CAD" => "CA$".into(),
        "AUD" => "A$".into(),
        "MXN" => "MX$".into(),
        "CHF" => "CHF".into(),
        "RUB" => "RUB".into(),
        "ILS" => "₪".into(),
        "TRY" => "TRY".into(),
        "NZD" => "NZ$".into(),
        "HKD" => "HK$".into(),
        "SEK" | "NOK" | "DKK" | "PLN" | "CZK" | "HUF" => code.into(),
        other => other.into(),
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
        "MXN" => ("Mexican peso", "Mexican pesos"),
        "CAD" => ("Canadian dollar", "Canadian dollars"),
        "AUD" => ("Australian dollar", "Australian dollars"),
        "CHF" => ("Swiss franc", "Swiss francs"),
        _ => return code.to_string(),
    };
    if plural { many } else { one }.into()
}

fn read_nf_options(realm: &mut Realm, locales: Value, options: Value) -> JsResult<NfOptions> {
    let locale = resolve_locale(realm, locales)?;
    let opts = options_object(realm, options)?;
    let style = match opt_str(realm, opts, "style")?.as_deref() {
        None | Some("decimal") => NfStyle::Decimal,
        Some("currency") => NfStyle::Currency,
        Some("percent") => NfStyle::Percent,
        Some("unit") => NfStyle::Unit,
        Some(other) => return realm.range_error(&format!("Value {} out of range for Intl.NumberFormat options property style", other)),
    };
    let currency = opt_str(realm, opts, "currency")?.map(|c| c.to_ascii_uppercase());
    if style == NfStyle::Currency {
        match &currency {
            Some(c) if c.len() == 3 && c.chars().all(|ch| ch.is_ascii_alphabetic()) => {}
            Some(c) => return realm.range_error(&format!("Invalid currency code : {}", c)),
            None => return realm.type_error("Currency code is required with currency style."),
        }
    }
    let unit = opt_str(realm, opts, "unit")?;
    if style == NfStyle::Unit && unit.is_none() {
        return realm.type_error("Unit is required with unit style.");
    }
    let default_frac = match style {
        NfStyle::Currency => currency_digits(currency.as_deref().unwrap_or("")),
        NfStyle::Percent => 0,
        _ => 0,
    };
    let default_max = match style {
        NfStyle::Currency => default_frac,
        NfStyle::Percent => 0,
        _ => 3,
    };
    let min_int = opt_num(realm, opts, "minimumIntegerDigits", 1.0, 21.0)?.unwrap_or(1);
    let mut min_frac = opt_num(realm, opts, "minimumFractionDigits", 0.0, 100.0)?;
    let mut max_frac = opt_num(realm, opts, "maximumFractionDigits", 0.0, 100.0)?;
    match (min_frac, max_frac) {
        (Some(a), Some(b)) if a > b => return realm.range_error("maximumFractionDigits value is out of range."),
        (Some(a), None) => max_frac = Some(a.max(default_max)),
        (None, Some(b)) => min_frac = Some(b.min(default_frac)),
        _ => {}
    }
    let min_sig = opt_num(realm, opts, "minimumSignificantDigits", 1.0, 21.0)?;
    let max_sig = opt_num(realm, opts, "maximumSignificantDigits", 1.0, 21.0)?;
    let notation = opt_str(realm, opts, "notation")?.unwrap_or_else(|| "standard".into());
    let compact = notation == "compact";
    let use_grouping = match opts {
        Some(o) => {
            let v = realm.get_str(Value::Object(o), "useGrouping")?;
            match v.v() {
                V::Undefined => !compact || true,
                V::String(s) => !s.eq_str("false"),
                v => v.truthy(),
            }
        }
        None => true,
    };
    let nf_locale = locale.clone();
    Ok(NfOptions {
        locale: nf_locale,
        style,
        currency,
        currency_display: opt_str(realm, opts, "currencyDisplay")?.unwrap_or_else(|| "symbol".into()),
        unit,
        unit_display: opt_str(realm, opts, "unitDisplay")?.unwrap_or_else(|| "short".into()),
        min_int,
        min_frac: min_frac.unwrap_or(default_frac),
        max_frac: max_frac.unwrap_or(default_max),
        min_sig: if min_sig.is_some() || max_sig.is_some() { Some(min_sig.unwrap_or(1)) } else { None },
        max_sig: if min_sig.is_some() || max_sig.is_some() { Some(max_sig.unwrap_or(21)) } else { None },
        use_grouping,
        compact,
        compact_long: opt_str(realm, opts, "compactDisplay")?.as_deref() == Some("long"),
        sign_display: opt_str(realm, opts, "signDisplay")?.unwrap_or_else(|| "auto".into()),
        numbering_system: {
            let nu = opt_str(realm, opts, "numberingSystem")?;
            if let Some(n) = &nu {
                if !duration::is_unicode_type(n) {
                    return realm.range_error(&format!("Invalid numberingSystem: {}", n));
                }
            }
            nu.filter(|n| numbering_zero(n).is_some())
                .or_else(|| locale.nu.clone().filter(|n| numbering_zero(n).is_some()))
                .unwrap_or_else(|| "latn".into())
        },
    })
}

/// (group separator, decimal separator, minimum grouping digits)
fn separators(lang: &str, region: &str) -> (&'static str, &'static str, usize) {
    match lang {
        "de" if region == "CH" => ("’", ".", 1),
        "de" | "it" | "nl" | "da" | "id" | "tr" | "el" | "ro" | "hr" | "sl" | "sr" => (".", ",", 1),
        "es" | "pt" | "pl" => (".", ",", 2),
        "fr" => ("\u{202f}", ",", 1),
        "ru" | "cs" | "sv" | "nb" | "fi" | "uk" | "sk" | "bg" | "hu" | "lt" | "lv" | "et" => ("\u{a0}", ",", 1),
        _ => (",", ".", 1),
    }
}

/// Round `x` to `digits` fraction digits (half away from zero) and return
/// the decimal digits string (no sign, no trailing-zero trimming).
fn fixed(x: f64, digits: usize) -> String {
    let s = format!("{:.*}", digits, x.abs());
    // `format!` rounds the exact binary value half-to-even; nudge halves up
    // the way ICU's half-expand rounding would for decimal-looking inputs.
    let scaled = x.abs() * 10f64.powi(digits as i32);
    let frac = scaled - scaled.floor();
    if (frac - 0.5).abs() < 1e-9 {
        let up = (scaled.floor() + 1.0) / 10f64.powi(digits as i32);
        return format!("{:.*}", digits, up);
    }
    s
}

fn group_int(int: &str, sep: &str, min_group: usize) -> String {
    if int.len() < 3 + min_group {
        return int.to_string();
    }
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push_str(sep);
        }
        out.push(c);
    }
    out
}

pub struct NumberParts {
    pub parts: Vec<(&'static str, String)>,
}

impl NumberParts {
    pub fn to_string(&self) -> String {
        self.parts.iter().map(|(_, s)| s.as_str()).collect()
    }
}

/// Format with significant-digit constraints.
fn significant(x: f64, min_sig: usize, max_sig: usize) -> String {
    if x == 0.0 {
        return if min_sig > 1 { format!("0.{}", "0".repeat(min_sig - 1)) } else { "0".into() };
    }
    let mag = x.abs().log10().floor() as i32;
    let decimals = (max_sig as i32 - 1 - mag).max(0) as usize;
    // Integer digits beyond the significant count are rounded away.
    let x = if mag + 1 > max_sig as i32 {
        let scale = 10f64.powi(mag + 1 - max_sig as i32);
        (x.abs() / scale + 0.5).floor() * scale
    } else {
        x.abs()
    };
    let s = fixed(x, decimals);
    // Trim trailing zeros beyond min_sig significant digits.
    let (int, frac) = s.split_once('.').map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or((s.clone(), String::new()));
    let int_sig = if int == "0" { 0 } else { int.len() };
    let mut frac = frac;
    while !frac.is_empty() && frac.ends_with('0') && int_sig + frac.len() > min_sig {
        frac.pop();
    }
    if frac.is_empty() {
        int
    } else {
        format!("{}.{}", int, frac)
    }
}

pub fn format_number_parts(x: f64, o: &NfOptions) -> NumberParts {
    format_parts_impl(x, None, o)
}

/// Format a BigInt given its decimal digits (no sign) exactly.
pub fn format_bigint_parts(digits: &str, negative: bool, o: &NfOptions) -> NumberParts {
    let approx: f64 = digits.parse::<f64>().unwrap_or(0.0) * if negative { -1.0 } else { 1.0 };
    format_parts_impl(approx, Some(digits.to_string()), o)
}

fn format_parts_impl(x: f64, exact_digits: Option<String>, o: &NfOptions) -> NumberParts {
    let lang = o.locale.lang.as_str();
    let region = o.locale.region.as_deref().unwrap_or("");
    let (gsep, dsep, min_group) = separators(lang, region);
    let mut parts: Vec<(&'static str, String)> = Vec::new();
    let negative = x.is_sign_negative() && x != 0.0 || (x == 0.0 && x.is_sign_negative() && o.sign_display == "negative");
    let mut value = x.abs();
    if o.style == NfStyle::Percent {
        value *= 100.0;
    }
    let mut compact_suffix = String::new();
    if o.compact && value.is_finite() && value >= 1000.0 {
        let (div, short, long) = if value >= 1e12 {
            (1e12, if lang == "es" { "B" } else { "T" }, if lang == "es" { "billones" } else { "trillion" })
        } else if value >= 1e9 {
            (1e9, if lang == "es" { "mil M" } else { "B" }, if lang == "es" { "mil millones" } else { "billion" })
        } else if value >= 1e6 {
            (1e6, if lang == "es" { "M" } else { "M" }, if lang == "es" { "millones" } else { "million" })
        } else {
            (1e3, if lang == "es" { "mil" } else { "K" }, if lang == "es" { "mil" } else { "thousand" })
        };
        value /= div;
        compact_suffix = if o.compact_long { format!(" {}", long) } else if lang == "es" { format!("\u{a0}{}", short) } else { short.to_string() };
    }
    let exact_ok = exact_digits.is_some() && !o.compact && o.min_sig.is_none() && o.style != NfStyle::Percent;
    let body = if let (true, Some(d)) = (exact_ok, exact_digits.as_ref()) {
        if o.min_frac > 0 {
            format!("{}.{}", d, "0".repeat(o.min_frac))
        } else {
            d.clone()
        }
    } else if !value.is_finite() {
        if value.is_nan() { "NaN".to_string() } else { "∞".to_string() }
    } else if let (Some(min_sig), Some(max_sig)) = (o.min_sig, o.max_sig) {
        significant(value, min_sig, max_sig)
    } else if o.compact && !compact_suffix.is_empty() {
        // Compact: up to 2 significant digits for values < 100 of the unit.
        let s = if value < 10.0 { fixed(value, 1) } else { fixed(value, 0) };
        let s = if s.ends_with(".0") { s.trim_end_matches(".0").to_string() } else { s };
        s
    } else {
        let s = fixed(value, o.max_frac);
        let (int, frac) = s.split_once('.').map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or((s.clone(), String::new()));
        let mut frac = frac;
        while frac.len() > o.min_frac && frac.ends_with('0') {
            frac.pop();
        }
        if frac.is_empty() {
            int
        } else {
            format!("{}.{}", int, frac)
        }
    };
    let (int, frac) = body.split_once('.').map(|(a, b)| (a.to_string(), Some(b.to_string()))).unwrap_or((body.clone(), None));
    let int = if int.len() < o.min_int && value.is_finite() { format!("{}{}", "0".repeat(o.min_int - int.len()), int) } else { int };
    // Sign
    let show_sign = match o.sign_display.as_str() {
        "always" => true,
        "never" => false,
        "exceptZero" => x != 0.0 && value != 0.0,
        _ => negative,
    };
    let sign_part: Option<(&'static str, String)> = if show_sign { Some(if negative { ("minusSign", "-".into()) } else { ("plusSign", "+".into()) }) } else { None };
    // Currency / unit affixes
    let mut prefix: Vec<(&'static str, String)> = Vec::new();
    let mut suffix: Vec<(&'static str, String)> = Vec::new();
    match o.style {
        NfStyle::Currency => {
            let code = o.currency.clone().unwrap_or_default();
            let sym = match o.currency_display.as_str() {
                "code" => code.clone(),
                "name" => String::new(),
                "narrowSymbol" => currency_symbol(&code, lang).trim_start_matches(|c: char| c.is_ascii_uppercase()).to_string(),
                _ => currency_symbol(&code, lang),
            };
            if o.currency_display == "name" {
                suffix.push(("literal", " ".into()));
                suffix.push(("currency", currency_name(&code, value != 1.0)));
            } else if matches!(lang, "en" | "ja" | "zh" | "ko" | "he" | "th" | "hi" | "id" | "ms") {
                prefix.push(("currency", sym.clone()));
                if o.currency_display == "code" || sym.chars().all(|c| c.is_ascii_alphabetic()) {
                    prefix.push(("literal", "\u{a0}".into()));
                }
            } else {
                suffix.push(("literal", "\u{a0}".into()));
                suffix.push(("currency", sym));
            }
        }
        NfStyle::Percent => {
            if matches!(lang, "en" | "ja" | "zh" | "ko" | "pt" | "it" | "nl" | "id" | "th") {
                suffix.push(("percentSign", "%".into()));
            } else {
                suffix.push(("literal", "\u{a0}".into()));
                suffix.push(("percentSign", "%".into()));
            }
        }
        NfStyle::Unit => {
            let u = o.unit.clone().unwrap_or_default();
            let (short, long_one, long_many) = unit_names(&u);
            let text = match o.unit_display.as_str() {
                "long" => if body == "1" { long_one } else { long_many }.to_string(),
                "narrow" => short.trim().to_string(),
                _ => format!(" {}", short),
            };
            if o.unit_display == "long" {
                suffix.push(("literal", " ".into()));
            }
            suffix.push(("unit", text));
        }
        NfStyle::Decimal => {}
    }
    if let Some(s) = sign_part {
        parts.push(s);
    }
    parts.extend(prefix);
    if !value.is_finite() {
        parts.push((if value.is_nan() { "nan" } else { "infinity" }, body.clone()));
    } else {
        let grouped = if o.use_grouping { group_int(&int, gsep, min_group) } else { int.clone() };
        for piece in grouped.split(gsep) {
            if !parts.is_empty() && matches!(parts.last(), Some(("integer", _))) {
                parts.push(("group", gsep.to_string()));
            }
            parts.push(("integer", transliterate_digits(piece, &o.numbering_system)));
        }
        if let Some(f) = frac {
            parts.push(("decimal", dsep.to_string()));
            parts.push(("fraction", transliterate_digits(&f, &o.numbering_system)));
        }
        if !compact_suffix.is_empty() {
            parts.push(("compact", compact_suffix));
        }
    }
    parts.extend(suffix);
    NumberParts { parts }
}

fn unit_names(u: &str) -> (&'static str, &'static str, &'static str) {
    match u {
        "kilometer" => ("km", "kilometer", "kilometers"),
        "meter" => ("m", "meter", "meters"),
        "centimeter" => ("cm", "centimeter", "centimeters"),
        "mile" => ("mi", "mile", "miles"),
        "kilogram" => ("kg", "kilogram", "kilograms"),
        "gram" => ("g", "gram", "grams"),
        "pound" => ("lb", "pound", "pounds"),
        "liter" => ("L", "liter", "liters"),
        "second" => ("sec", "second", "seconds"),
        "minute" => ("min", "minute", "minutes"),
        "hour" => ("hr", "hour", "hours"),
        "day" => ("day", "day", "days"),
        "week" => ("wk", "week", "weeks"),
        "month" => ("mth", "month", "months"),
        "year" => ("yr", "year", "years"),
        "byte" => ("byte", "byte", "bytes"),
        "kilobyte" => ("kB", "kilobyte", "kilobytes"),
        "megabyte" => ("MB", "megabyte", "megabytes"),
        "gigabyte" => ("GB", "gigabyte", "gigabytes"),
        "percent" => ("%", "percent", "percent"),
        "celsius" => ("°C", "degree Celsius", "degrees Celsius"),
        "fahrenheit" => ("°F", "degree Fahrenheit", "degrees Fahrenheit"),
        "kilometer-per-hour" => ("km/h", "kilometer per hour", "kilometers per hour"),
        "mile-per-hour" => ("mph", "mile per hour", "miles per hour"),
        _ => ("", "", ""),
    }
}

pub fn format_number(x: f64, o: &NfOptions) -> String {
    format_number_parts(x, o).to_string()
}

/// `Number.prototype.toLocaleString(locales, options)`.
pub fn number_to_locale_string(realm: &mut Realm, n: f64, args: &[Value]) -> JsResult<Value> {
    let o = read_nf_options(realm, arg(args, 0), arg(args, 1))?;
    Ok(Value::str(&format_number(n, &o)))
}

// ------------------------------------------------------------ Collator

#[derive(Debug, Clone)]
pub struct CollatorOptions {
    locale: Locale,
    sensitivity: String,
    numeric: bool,
    ignore_punctuation: bool,
    usage: String,
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
        let case = ka.iter().map(|k| k.2).cmp(kb.iter().map(|k| k.2));
        if case != Ordering::Equal {
            return case;
        }
    }
    if sens == "variant" {
        return a.cmp(b);
    }
    Ordering::Equal
}

fn read_collator_options(realm: &mut Realm, locales: Value, options: Value) -> JsResult<CollatorOptions> {
    let locale = resolve_locale(realm, locales)?;
    let opts = options_object(realm, options)?;
    let usage = opt_str(realm, opts, "usage")?.unwrap_or_else(|| "sort".into());
    let sensitivity = opt_str(realm, opts, "sensitivity")?.unwrap_or_else(|| if usage == "search" { "variant".into() } else { "variant".into() });
    if !matches!(sensitivity.as_str(), "base" | "accent" | "case" | "variant") {
        return realm.range_error("Value out of range for Intl.Collator options property sensitivity");
    }
    Ok(CollatorOptions {
        numeric: opt_bool(realm, opts, "numeric")?.unwrap_or(locale.tag.contains("-u-kn")),
        ignore_punctuation: opt_bool(realm, opts, "ignorePunctuation")?.unwrap_or(false),
        locale,
        sensitivity,
        usage,
    })
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

#[derive(Debug, Clone)]
pub struct PluralOptions {
    locale: Locale,
    ordinal: bool,
}

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

    let (_, dtf) = mk(realm, "DateTimeFormat", "Intl.DateTimeFormat", dtf_ctor, 0);
    realm.define_accessor(dtf, "format", Some(dtf_format_getter), None, CONFIGURABLE);
    realm.define_method(dtf, "formatToParts", 1, dtf_format_to_parts);
    realm.define_method(dtf, "formatRange", 2, dtf_format_range);
    realm.define_method(dtf, "resolvedOptions", 0, dtf_resolved_options);

    let (_, nf) = mk(realm, "NumberFormat", "Intl.NumberFormat", nf_ctor, 0);
    realm.define_accessor(nf, "format", Some(nf_format_getter), None, CONFIGURABLE);
    realm.define_method(nf, "formatToParts", 1, nf_format_to_parts);
    realm.define_method(nf, "formatRange", 2, nf_format_range);
    realm.define_method(nf, "resolvedOptions", 0, nf_resolved_options);

    let (_, coll) = mk(realm, "Collator", "Intl.Collator", collator_ctor, 0);
    realm.define_accessor(coll, "compare", Some(collator_compare_getter), None, CONFIGURABLE);
    realm.define_method(coll, "resolvedOptions", 0, collator_resolved_options);

    let (_, pr) = mk(realm, "PluralRules", "Intl.PluralRules", plural_ctor, 0);
    realm.define_method(pr, "select", 1, plural_select);
    realm.define_method(pr, "selectRange", 2, plural_select_range);
    realm.define_method(pr, "resolvedOptions", 0, plural_resolved_options);

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

    let (_, dn) = mk(realm, "DisplayNames", "Intl.DisplayNames", display_names_ctor, 2);
    realm.define_method(dn, "of", 1, display_names_of);
    realm.define_method(dn, "resolvedOptions", 0, display_names_resolved_options);

    let (_, df) = mk(realm, "DurationFormat", "Intl.DurationFormat", duration::ctor, 0);
    realm.define_method(df, "format", 1, duration::format);
    realm.define_method(df, "formatToParts", 1, duration::format_to_parts);
    realm.define_method(df, "resolvedOptions", 0, duration::resolved_options);

    let (_, seg) = mk(realm, "Segmenter", "Intl.Segmenter", segmenter_ctor, 0);
    realm.define_method(seg, "segment", 1, segmenter_segment);
    realm.define_method(seg, "resolvedOptions", 0, segmenter_resolved_options);

    // Locale-sensitive methods on the primitives route through Intl.
    let np = realm.intrinsics.number_proto;
    realm.define_method(np, "toLocaleString", 0, number_to_locale);
    let bp = realm.intrinsics.bigint_proto;
    realm.define_method(bp, "toLocaleString", 0, bigint_to_locale);
    let sp = realm.intrinsics.string_proto;
    realm.define_method(sp, "localeCompare", 1, string_locale_compare);
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
        "calendar" => vec!["gregory"],
        "collation" => vec!["default"],
        "currency" => vec!["AUD", "BRL", "CAD", "CHF", "CNY", "EUR", "GBP", "HKD", "INR", "JPY", "KRW", "MXN", "NZD", "RUB", "USD"],
        "numberingSystem" => {
            let mut v: Vec<&str> = NUMBERING_SYSTEMS.iter().map(|(n, _)| *n).collect();
            v.push("hanidec");
            v.sort_unstable();
            v
        }
        "timeZone" => vec!["UTC"],
        "unit" => vec!["byte", "celsius", "centimeter", "day", "gram", "hour", "kilogram", "kilometer", "liter", "meter", "mile", "minute", "month", "percent", "pound", "second", "week", "year"],
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

/// A `format`/`compare` getter returning a function bound to `this`.
fn bound_getter(realm: &mut Realm, this: Value, name: &str, f: NativeFn, what: &str) -> JsResult<Value> {
    if !matches!(this.v(), V::Object(id) if matches!(realm.obj(id).kind, ObjectKind::Host(_))) {
        return realm.type_error(&format!("Method {} called on incompatible receiver", what));
    }
    let b = realm.new_native_function_full(name, 1, f, None, vec![this], false);
    Ok(Value::Object(b))
}

fn bound_this(realm: &Realm) -> Value {
    realm.current_native().map(|n| realm.native_captures(n)).and_then(|c| c.into_iter().next()).unwrap_or(Value::Undefined)
}

// DateTimeFormat
fn dtf_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let o = read_dt_options(realm, arg(args, 0), arg(args, 1), DateDefaults::Date)?;
    Ok(Value::Object(new_intl_object(realm, "Intl.DateTimeFormat", o)?))
}

fn dtf_format_getter(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    bound_getter(realm, this, "format", dtf_format_bound, "get Intl.DateTimeFormat.prototype.format")
}

fn date_arg(realm: &mut Realm, v: Value) -> JsResult<f64> {
    match v.v() {
        V::Undefined => Ok(super::date::now_ms()),
        V::Object(id) => match &realm.obj(id).kind {
            ObjectKind::Date(t) => Ok(*t),
            _ => realm.to_number(Value::Object(id)),
        },
        other => realm.to_number(other.to_value()),
    }
}

fn dtf_format_bound(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let this = bound_this(realm);
    let o: DtOptions = host(realm, &this, "Intl.DateTimeFormat.prototype.format")?;
    let t = date_arg(realm, arg(args, 0))?;
    if t.is_nan() {
        return realm.range_error("Invalid time value");
    }
    Ok(Value::str(&format_date(t, &o)))
}

fn dtf_format_to_parts(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o: DtOptions = host(realm, &this, "Intl.DateTimeFormat.prototype.formatToParts")?;
    let t = date_arg(realm, arg(args, 0))?;
    if t.is_nan() {
        return realm.range_error("Invalid time value");
    }
    let s = format_date(t, &o);
    // Split into literal / numeric / alphabetic runs.
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut kind = "";
    let classify = |c: char| if c.is_ascii_digit() { "number" } else if c.is_alphabetic() { "word" } else { "literal" };
    for c in s.chars() {
        let k = classify(c);
        if k != kind && !cur.is_empty() {
            parts.push((kind, std::mem::take(&mut cur)));
        }
        kind = k;
        cur.push(c);
    }
    if !cur.is_empty() {
        parts.push((kind, cur));
    }
    let f = match o.time_zone.as_deref() {
        Some("UTC") => fields(t),
        Some(z) => match super::date::tz_named(z) {
            Some(info) => fields(t + super::date::zone_offset(info, t).0),
            None => local_fields(t),
        },
        None => local_fields(t),
    };
    let mut out = Vec::new();
    let mut nums_seen = 0;
    for (k, v) in parts {
        let typ = match k {
            "number" => {
                nums_seen += 1;
                let n: f64 = v.parse().unwrap_or(-1.0);
                if o.year.is_some() && (n == f.year || (o.year == Some(Width::TwoDigit) && n == f.year.rem_euclid(100.0))) && nums_seen <= 3 && v.len() >= 2 {
                    "year"
                } else if o.hour.is_some() && (n == f.hour || n == f.hour % 12.0 || (n == 12.0 && f.hour % 12.0 == 0.0)) && nums_seen > (o.year.is_some() as i32 + o.month.is_some() as i32 + o.day.is_some() as i32) {
                    "hour"
                } else if o.minute.is_some() && n == f.minute && nums_seen > (o.year.is_some() as i32 + o.month.is_some() as i32 + o.day.is_some() as i32 + 1) {
                    "minute"
                } else if o.second.is_some() && n == f.second {
                    "second"
                } else if o.month.is_some() && n == f.month + 1.0 {
                    "month"
                } else if o.day.is_some() && n == f.day {
                    "day"
                } else {
                    "literal"
                }
            }
            "word" => {
                if v == "AM" || v == "PM" {
                    "dayPeriod"
                } else if o.weekday.is_some() && names_for(&o.locale.lang).days_long.iter().chain(names_for(&o.locale.lang).days_short.iter()).any(|d| d.trim_end_matches('.').eq_ignore_ascii_case(v.trim_end_matches('.'))) {
                    "weekday"
                } else if o.month.is_some() && names_for(&o.locale.lang).months_long.iter().chain(names_for(&o.locale.lang).months_short.iter()).any(|d| d.trim_end_matches('.').eq_ignore_ascii_case(v.trim_end_matches('.'))) {
                    "month"
                } else if o.time_zone_name.is_some() {
                    "timeZoneName"
                } else {
                    "literal"
                }
            }
            _ => "literal",
        };
        let obj = realm.new_object();
        realm.define_property(obj, "type", Value::str(typ), ALL);
        realm.define_property(obj, "value", Value::str(&v), ALL);
        out.push(Value::Object(obj));
    }
    Ok(realm.new_array_value(out))
}

fn dtf_format_range(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o: DtOptions = host(realm, &this, "Intl.DateTimeFormat.prototype.formatRange")?;
    let a = date_arg(realm, arg(args, 0))?;
    let b = date_arg(realm, arg(args, 1))?;
    if a.is_nan() || b.is_nan() {
        return realm.range_error("Invalid time value");
    }
    let (sa, sb) = (format_date(a, &o), format_date(b, &o));
    Ok(Value::str(&if sa == sb { sa } else { format!("{} – {}", sa, sb) }))
}

fn dtf_resolved_options(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let o: DtOptions = host(realm, &this, "Intl.DateTimeFormat.prototype.resolvedOptions")?;
    let obj = realm.new_object();
    realm.define_property(obj, "locale", Value::str(&o.locale.tag), ALL);
    realm.define_property(obj, "calendar", Value::str("gregory"), ALL);
    realm.define_property(obj, "numberingSystem", Value::str("latn"), ALL);
    let tz_name = o.time_zone.clone().unwrap_or_else(|| tz().name.clone());
    realm.define_property(obj, "timeZone", Value::str(&tz_name), ALL);
    let w = |w: Option<Width>| -> Value {
        match w {
            Some(Width::Numeric) => Value::str("numeric"),
            Some(Width::TwoDigit) => Value::str("2-digit"),
            Some(Width::Long) => Value::str("long"),
            Some(Width::Short) => Value::str("short"),
            Some(Width::Narrow) => Value::str("narrow"),
            None => Value::Undefined,
        }
    };
    for (name, v) in [("weekday", o.weekday), ("year", o.year), ("month", o.month), ("day", o.day), ("hour", o.hour), ("minute", o.minute), ("second", o.second), ("timeZoneName", o.time_zone_name)] {
        if v.is_some() {
            realm.define_property(obj, name, w(v), ALL);
        }
    }
    if o.hour.is_some() {
        let us = o.locale.lang == "en" && o.locale.region.as_deref().map(|r| r == "US").unwrap_or(true);
        let h12 = o.hour12.unwrap_or(us);
        realm.define_property(obj, "hour12", Value::Bool(h12), ALL);
        realm.define_property(obj, "hourCycle", Value::str(if h12 { "h12" } else { "h23" }), ALL);
    }
    Ok(Value::Object(obj))
}

// NumberFormat
fn nf_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let o = read_nf_options(realm, arg(args, 0), arg(args, 1))?;
    Ok(Value::Object(new_intl_object(realm, "Intl.NumberFormat", o)?))
}

fn nf_format_getter(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    bound_getter(realm, this, "format", nf_format_bound, "get Intl.NumberFormat.prototype.format")
}

fn number_arg(realm: &mut Realm, v: Value) -> JsResult<f64> {
    match (realm.to_numeric(v)?).v() {
        V::BigInt(b) => Ok(b.to_f64()),
        V::Number(n) => Ok(n),
        _ => Ok(f64::NAN),
    }
}

/// Format any numeric value, keeping BigInts exact.
fn format_numeric(realm: &mut Realm, v: Value, o: &NfOptions) -> JsResult<String> {
    match (realm.to_numeric(v)?).v() {
        V::BigInt(b) => Ok(format_bigint_parts(&b.to_string_radix(10).trim_start_matches('-'), b.is_negative(), o).to_string()),
        V::Number(n) => Ok(format_number(n, o)),
        _ => Ok("NaN".into()),
    }
}

fn nf_format_bound(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let this = bound_this(realm);
    let o: NfOptions = host(realm, &this, "Intl.NumberFormat.prototype.format")?;
    let s = format_numeric(realm, arg(args, 0), &o)?;
    Ok(Value::str(&s))
}

fn nf_format_to_parts(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o: NfOptions = host(realm, &this, "Intl.NumberFormat.prototype.formatToParts")?;
    let n = number_arg(realm, arg(args, 0))?;
    let parts = format_number_parts(n, &o);
    let mut out = Vec::new();
    for (k, v) in parts.parts {
        let obj = realm.new_object();
        realm.define_property(obj, "type", Value::str(k), ALL);
        realm.define_property(obj, "value", Value::str(&v), ALL);
        out.push(Value::Object(obj));
    }
    Ok(realm.new_array_value(out))
}

fn nf_format_range(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o: NfOptions = host(realm, &this, "Intl.NumberFormat.prototype.formatRange")?;
    let a = number_arg(realm, arg(args, 0))?;
    let b = number_arg(realm, arg(args, 1))?;
    let (sa, sb) = (format_number(a, &o), format_number(b, &o));
    Ok(Value::str(&if sa == sb { format!("~{}", sa) } else { format!("{}–{}", sa, sb) }))
}

fn nf_resolved_options(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let o: NfOptions = host(realm, &this, "Intl.NumberFormat.prototype.resolvedOptions")?;
    let obj = realm.new_object();
    realm.define_property(obj, "locale", Value::str(&o.locale.tag), ALL);
    realm.define_property(obj, "numberingSystem", Value::str(&o.numbering_system), ALL);
    let style = match o.style {
        NfStyle::Decimal => "decimal",
        NfStyle::Currency => "currency",
        NfStyle::Percent => "percent",
        NfStyle::Unit => "unit",
    };
    realm.define_property(obj, "style", Value::str(style), ALL);
    if let Some(c) = &o.currency {
        realm.define_property(obj, "currency", Value::str(c), ALL);
        realm.define_property(obj, "currencyDisplay", Value::str(&o.currency_display), ALL);
        realm.define_property(obj, "currencySign", Value::str("standard"), ALL);
    }
    if let Some(u) = &o.unit {
        realm.define_property(obj, "unit", Value::str(u), ALL);
        realm.define_property(obj, "unitDisplay", Value::str(&o.unit_display), ALL);
    }
    realm.define_property(obj, "minimumIntegerDigits", Value::Number(o.min_int as f64), ALL);
    if let (Some(a), Some(b)) = (o.min_sig, o.max_sig) {
        realm.define_property(obj, "minimumSignificantDigits", Value::Number(a as f64), ALL);
        realm.define_property(obj, "maximumSignificantDigits", Value::Number(b as f64), ALL);
    } else {
        realm.define_property(obj, "minimumFractionDigits", Value::Number(o.min_frac as f64), ALL);
        realm.define_property(obj, "maximumFractionDigits", Value::Number(o.max_frac as f64), ALL);
    }
    realm.define_property(obj, "useGrouping", Value::str(if o.use_grouping { "auto" } else { "false" }), ALL);
    realm.define_property(obj, "notation", Value::str(if o.compact { "compact" } else { "standard" }), ALL);
    if o.compact {
        realm.define_property(obj, "compactDisplay", Value::str(if o.compact_long { "long" } else { "short" }), ALL);
    }
    realm.define_property(obj, "signDisplay", Value::str(&o.sign_display), ALL);
    realm.define_property(obj, "roundingMode", Value::str("halfExpand"), ALL);
    Ok(Value::Object(obj))
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
    Ok(Value::Object(new_intl_object(realm, "Intl.Collator", o)?))
}

fn collator_compare_getter(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    bound_getter(realm, this, "compare", collator_compare_bound, "get Intl.Collator.prototype.compare")
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
    realm.define_property(obj, "collation", Value::str("default"), ALL);
    realm.define_property(obj, "numeric", Value::Bool(o.numeric), ALL);
    realm.define_property(obj, "caseFirst", Value::str("false"), ALL);
    Ok(Value::Object(obj))
}

// PluralRules
fn plural_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let locale = resolve_locale(realm, arg(args, 0))?;
    let opts = options_object(realm, arg(args, 1))?;
    let ordinal = opt_str(realm, opts, "type")?.as_deref() == Some("ordinal");
    Ok(Value::Object(new_intl_object(realm, "Intl.PluralRules", PluralOptions { locale, ordinal })?))
}

fn plural_select(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o: PluralOptions = host(realm, &this, "Intl.PluralRules.prototype.select")?;
    let n = realm.to_number(arg(args, 0))?;
    Ok(Value::str(plural_category(n, &o.locale.lang, o.ordinal)))
}

fn plural_select_range(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o: PluralOptions = host(realm, &this, "Intl.PluralRules.prototype.selectRange")?;
    let n = realm.to_number(arg(args, 1))?;
    Ok(Value::str(plural_category(n, &o.locale.lang, o.ordinal)))
}

fn plural_resolved_options(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let o: PluralOptions = host(realm, &this, "Intl.PluralRules.prototype.resolvedOptions")?;
    let obj = realm.new_object();
    realm.define_property(obj, "locale", Value::str(&o.locale.tag), ALL);
    realm.define_property(obj, "type", Value::str(if o.ordinal { "ordinal" } else { "cardinal" }), ALL);
    let cats: Vec<Value> = if o.ordinal { vec!["few", "one", "two", "other"] } else { vec!["one", "other"] }.into_iter().map(Value::str).collect();
    let arr = realm.new_array_value(cats);
    realm.define_property(obj, "pluralCategories", arr, ALL);
    Ok(Value::Object(obj))
}

#[derive(Debug, Clone)]
struct DisplayNamesOptions {
    locale: Locale,
    kind: String,
}

fn display_names_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let locale = resolve_locale(realm, arg(args, 0))?;
    let opts = options_object(realm, arg(args, 1))?;
    let Some(kind) = opt_str(realm, opts, "type")? else { return realm.type_error("Required option 'type' is missing") };
    Ok(Value::Object(new_intl_object(realm, "Intl.DisplayNames", DisplayNamesOptions { locale, kind })?))
}

fn display_names_of(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o: DisplayNamesOptions = host(realm, &this, "Intl.DisplayNames.prototype.of")?;
    let code = realm.to_rust_string(arg(args, 0))?;
    let es = o.locale.lang == "es";
    let name: Option<&str> = match (o.kind.as_str(), code.to_ascii_uppercase().as_str()) {
        ("region", "US") => Some(if es { "Estados Unidos" } else { "United States" }),
        ("region", "ES") => Some(if es { "España" } else { "Spain" }),
        ("region", "GB") => Some(if es { "Reino Unido" } else { "United Kingdom" }),
        ("region", "DE") => Some(if es { "Alemania" } else { "Germany" }),
        ("region", "FR") => Some(if es { "Francia" } else { "France" }),
        ("region", "MX") => Some(if es { "México" } else { "Mexico" }),
        ("region", "JP") => Some(if es { "Japón" } else { "Japan" }),
        ("language", "EN") => Some(if es { "inglés" } else { "English" }),
        ("language", "ES") => Some(if es { "español" } else { "Spanish" }),
        ("language", "DE") => Some(if es { "alemán" } else { "German" }),
        ("language", "FR") => Some(if es { "francés" } else { "French" }),
        ("currency", "USD") => Some(if es { "dólar estadounidense" } else { "US Dollar" }),
        ("currency", "EUR") => Some(if es { "euro" } else { "Euro" }),
        _ => None,
    };
    Ok(Value::str(name.unwrap_or(&code)))
}

fn display_names_resolved_options(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let o: DisplayNamesOptions = host(realm, &this, "Intl.DisplayNames.prototype.resolvedOptions")?;
    let obj = realm.new_object();
    realm.define_property(obj, "locale", Value::str(&o.locale.tag), ALL);
    realm.define_property(obj, "type", Value::str(&o.kind), ALL);
    realm.define_property(obj, "style", Value::str("long"), ALL);
    Ok(Value::Object(obj))
}

// Segmenter
#[derive(Debug, Clone)]
struct SegmenterOptions {
    locale: Locale,
    granularity: String,
}

fn segmenter_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let locale = resolve_locale(realm, arg(args, 0))?;
    let opts = options_object(realm, arg(args, 1))?;
    let granularity = opt_str(realm, opts, "granularity")?.unwrap_or_else(|| "grapheme".into());
    Ok(Value::Object(new_intl_object(realm, "Intl.Segmenter", SegmenterOptions { locale, granularity })?))
}

/// `segment()` returns an array-like iterable of `{segment, index, input, isWordLike}`.
fn segmenter_segment(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o: SegmenterOptions = host(realm, &this, "Intl.Segmenter.prototype.segment")?;
    let input = realm.to_string(arg(args, 0))?;
    let s = input.to_rust_string();
    let mut segs: Vec<(String, usize, bool)> = Vec::new();
    match o.granularity.as_str() {
        "word" => {
            let mut cur = String::new();
            let mut start = 0;
            let mut cur_word: Option<bool> = None;
            for (i, c) in s.char_indices() {
                let is_word = c.is_alphanumeric() || c == '_' || c == '\'';
                if cur_word.map(|w| w != is_word || !is_word && !c.is_whitespace()).unwrap_or(false) || (cur_word == Some(false) && is_word) {
                    segs.push((std::mem::take(&mut cur), start, cur_word.unwrap_or(false)));
                    start = i;
                }
                if cur.is_empty() {
                    start = i;
                }
                cur.push(c);
                cur_word = Some(is_word);
            }
            if !cur.is_empty() {
                segs.push((cur, start, cur_word.unwrap_or(false)));
            }
        }
        "sentence" => {
            let mut cur = String::new();
            let mut start = 0;
            for (i, c) in s.char_indices() {
                if cur.is_empty() {
                    start = i;
                }
                cur.push(c);
                if matches!(c, '.' | '!' | '?') {
                    segs.push((std::mem::take(&mut cur), start, false));
                }
            }
            if !cur.is_empty() {
                segs.push((cur, start, false));
            }
        }
        _ => {
            for (i, c) in s.char_indices() {
                segs.push((c.to_string(), i, false));
            }
        }
    }
    // Convert byte offsets to UTF-16 indices.
    let mut out = Vec::new();
    for (seg, byte_start, word) in segs {
        let idx = s[..byte_start].encode_utf16().count();
        let obj = realm.new_object();
        realm.define_property(obj, "segment", Value::str(&seg), ALL);
        realm.define_property(obj, "index", Value::Number(idx as f64), ALL);
        realm.define_property(obj, "input", Value::String(input.clone()), ALL);
        if o.granularity == "word" {
            realm.define_property(obj, "isWordLike", Value::Bool(word), ALL);
        }
        out.push(Value::Object(obj));
    }
    Ok(realm.new_array_value(out))
}

fn segmenter_resolved_options(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let o: SegmenterOptions = host(realm, &this, "Intl.Segmenter.prototype.resolvedOptions")?;
    let obj = realm.new_object();
    realm.define_property(obj, "locale", Value::str(&o.locale.tag), ALL);
    realm.define_property(obj, "granularity", Value::str(&o.granularity), ALL);
    Ok(Value::Object(obj))
}
