//! `Intl.DateTimeFormat` (ECMA-402 §11) and the locale-sensitive `Date`
//! methods built on it (`toLocaleString`, `toLocaleDateString`,
//! `toLocaleTimeString`).
//!
//! Option processing follows CreateDateTimeFormat step by step (reading
//! order, validation, ResolveLocale for the `ca`, `hc` and `nu` keys, hour
//! cycle resolution, time zone identifiers including UTC offsets). Formats
//! are CLDR-style patterns generated from the requested components with a
//! small amount of built-in locale data (English in full; Spanish, German,
//! French, Italian, Portuguese, Japanese, Chinese and Korean names and
//! orders; other locales use English names with their numeric orders).
//! Calendars: gregorian, ISO 8601, buddhist, japanese, roc and the tabular
//! islamic ones.

use super::super::date as dt;
use crate::descriptor::PropDesc;
use super::*;

// ------------------------------------------------------------------ types

/// Which `Date.prototype.toLocale*String` method is formatting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateDefaults {
    DateTime,
    Date,
    Time,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Required {
    Date,
    Time,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Defaults {
    Date,
    Time,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hc {
    H11,
    H12,
    H23,
    H24,
}

impl Hc {
    fn parse(s: &str) -> Option<Hc> {
        Some(match s {
            "h11" => Hc::H11,
            "h12" => Hc::H12,
            "h23" => Hc::H23,
            "h24" => Hc::H24,
            _ => return None,
        })
    }
    fn name(self) -> &'static str {
        match self {
            Hc::H11 => "h11",
            Hc::H12 => "h12",
            Hc::H23 => "h23",
            Hc::H24 => "h24",
        }
    }
    fn is12(self) -> bool {
        matches!(self, Hc::H11 | Hc::H12)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    Full,
    Long,
    Medium,
    Short,
}

const STYLES: &[&str] = &["full", "long", "medium", "short"];

fn parse_style(s: &str) -> Style {
    match s {
        "full" => Style::Full,
        "long" => Style::Long,
        "medium" => Style::Medium,
        _ => Style::Short,
    }
}

fn style_name(s: Style) -> &'static str {
    STYLES[s as usize]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Cal {
    Gregory,
    Iso8601,
    Buddhist,
    Japanese,
    Roc,
    IslamicCivil,
    IslamicTbla,
}

/// Supported calendars, sorted (also `Intl.supportedValuesOf("calendar")`).
pub(super) const CALENDARS: &[(&str, Cal)] = &[
    ("buddhist", Cal::Buddhist),
    ("gregory", Cal::Gregory),
    ("islamic-civil", Cal::IslamicCivil),
    ("islamic-tbla", Cal::IslamicTbla),
    ("iso8601", Cal::Iso8601),
    ("japanese", Cal::Japanese),
    ("roc", Cal::Roc),
];

fn calendar_by_name(name: &str) -> Option<Cal> {
    CALENDARS.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

fn calendar_name(c: Cal) -> &'static str {
    CALENDARS.iter().find(|(_, k)| *k == c).map(|(n, _)| *n).unwrap_or("gregory")
}

/// CanonicalizeUValue for `ca` (CLDR calendar aliases).
fn canonical_calendar(s: &str) -> String {
    match s {
        "islamicc" => "islamic-civil".into(),
        "ethiopic-amete-alem" => "ethioaa".into(),
        "gregorian" => "gregory".into(),
        other => other.into(),
    }
}

impl Cal {
    /// Calendars whose formats always show the era with the year.
    fn implicit_era(self) -> bool {
        !matches!(self, Cal::Gregory | Cal::Iso8601)
    }
}

#[derive(Debug, Clone)]
enum Zone {
    /// A fixed offset in minutes (offset time zones, UTC without tzdata).
    Offset(i32),
    Named(&'static dt::TzInfo),
}

// Component option slots, in the reading order of Table 16.
const C_WEEKDAY: usize = 0;
const C_ERA: usize = 1;
const C_YEAR: usize = 2;
const C_MONTH: usize = 3;
const C_DAY: usize = 4;
const C_DAY_PERIOD: usize = 5;
const C_HOUR: usize = 6;
const C_MINUTE: usize = 7;
const C_SECOND: usize = 8;
const C_TZ_NAME: usize = 9;

const NARROW_SHORT_LONG: &[&str] = &["narrow", "short", "long"];
const TWO_DIGIT_NUMERIC: &[&str] = &["2-digit", "numeric"];

const COMPONENTS: [(&str, &[&str]); 10] = [
    ("weekday", NARROW_SHORT_LONG),
    ("era", NARROW_SHORT_LONG),
    ("year", TWO_DIGIT_NUMERIC),
    ("month", &["2-digit", "numeric", "narrow", "short", "long"]),
    ("day", TWO_DIGIT_NUMERIC),
    ("dayPeriod", NARROW_SHORT_LONG),
    ("hour", TWO_DIGIT_NUMERIC),
    ("minute", TWO_DIGIT_NUMERIC),
    ("second", TWO_DIGIT_NUMERIC),
    ("timeZoneName", &["short", "long", "shortOffset", "longOffset", "shortGeneric", "longGeneric"]),
];

#[derive(Debug, Clone, Default)]
struct Comps {
    v: [Option<&'static str>; 10],
    fsd: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Lit(String),
    F(char, usize),
}

/// The internal slots of an `Intl.DateTimeFormat`.
#[derive(Debug, Clone)]
pub struct Dtf {
    locale: String,
    lang: String,
    region: String,
    cal: Cal,
    nu: String,
    tz_id: String,
    zone: Zone,
    hc: Option<Hc>,
    comps: Comps,
    date_style: Option<Style>,
    time_style: Option<Style>,
    pattern: Vec<Tok>,
}

// ---------------------------------------------------------------- options

fn get_value(realm: &mut Realm, opts: Option<ObjectId>, name: &str) -> JsResult<Value> {
    match opts {
        Some(o) => realm.get_str(Value::Object(o), name),
        None => Ok(Value::Undefined),
    }
}

/// GetOption(options, name, string, values, default) returning the
/// matching entry of `values`.
fn get_enum(realm: &mut Realm, opts: Option<ObjectId>, name: &str, values: &[&'static str], default: Option<&'static str>) -> JsResult<Option<&'static str>> {
    let v = get_value(realm, opts, name)?;
    if v.is_undefined() {
        return Ok(default);
    }
    let s = realm.to_rust_string(v)?;
    match values.iter().find(|x| **x == s) {
        Some(x) => Ok(Some(x)),
        None => realm.range_error(&format!("Value {} out of range for Intl.DateTimeFormat options property {}", s, name)),
    }
}

fn get_string(realm: &mut Realm, opts: Option<ObjectId>, name: &str) -> JsResult<Option<String>> {
    let v = get_value(realm, opts, name)?;
    if v.is_undefined() {
        return Ok(None);
    }
    Ok(Some(realm.to_rust_string(v)?))
}

fn get_bool(realm: &mut Realm, opts: Option<ObjectId>, name: &str) -> JsResult<Option<bool>> {
    let v = get_value(realm, opts, name)?;
    if v.is_undefined() {
        return Ok(None);
    }
    Ok(Some(v.truthy()))
}

/// GetNumberOption.
fn get_number(realm: &mut Realm, opts: Option<ObjectId>, name: &str, min: f64, max: f64) -> JsResult<Option<usize>> {
    let v = get_value(realm, opts, name)?;
    if v.is_undefined() {
        return Ok(None);
    }
    let n = realm.to_number(v)?;
    if n.is_nan() || n < min || n > max {
        return realm.range_error(&format!("{} value is out of range.", name));
    }
    Ok(Some(n.floor() as usize))
}

// ------------------------------------------------------------ time zones

/// Available IANA time zone identifiers: lower-cased name → (identifier,
/// zoneinfo file holding its rules).
fn zone_table() -> &'static std::collections::HashMap<String, (String, String)> {
    static ZONES: std::sync::OnceLock<std::collections::HashMap<String, (String, String)>> = std::sync::OnceLock::new();
    ZONES.get_or_init(|| {
        let mut map = std::collections::HashMap::new();
        fn walk(base: &std::path::Path, rel: &str, map: &mut std::collections::HashMap<String, (String, String)>, depth: usize) {
            let Ok(rd) = std::fs::read_dir(base.join(rel)) else { return };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let path = if rel.is_empty() { name.clone() } else { format!("{}/{}", rel, name) };
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    if depth < 3 && rel.is_empty() && !matches!(name.as_str(), "posix" | "right" | "SystemV") || depth > 0 && depth < 3 {
                        walk(base, &path, map, depth + 1);
                    }
                    continue;
                }
                let valid = name.starts_with(|c: char| c.is_ascii_uppercase())
                    && path.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+'))
                    && !matches!(path.as_str(), "Factory" | "SECURITY");
                if valid {
                    map.insert(path.to_ascii_lowercase(), (path.clone(), path));
                }
            }
        }
        for &dir in dt::system_zoneinfo_dirs() {
            let base = std::path::Path::new(dir);
            if !base.is_dir() {
                continue;
            }
            walk(base, "", &mut map, 0);
            // Links the system did not install as files (`backward`).
            if let Ok(zi) = std::fs::read_to_string(base.join("tzdata.zi")) {
                for line in zi.lines() {
                    let mut it = line.split_whitespace();
                    if it.next() != Some("L") {
                        continue;
                    }
                    let (Some(target), Some(link)) = (it.next(), it.next()) else { continue };
                    let lower = link.to_ascii_lowercase();
                    if !map.contains_key(&lower) {
                        if let Some((_, file)) = map.get(&target.to_ascii_lowercase()).cloned() {
                            map.insert(lower, (link.to_string(), file));
                        }
                    }
                }
            }
            break;
        }
        // The embedded database covers hosts without one (Windows) and
        // names the system copy lacks.
        for (name, _) in dt::tzdb::names() {
            map.entry(name.to_ascii_lowercase()).or_insert_with(|| (name.to_string(), name.to_string()));
        }
        for utc in ["UTC", "Etc/UTC", "GMT", "Etc/GMT"] {
            map.entry(utc.to_ascii_lowercase()).or_insert_with(|| (utc.to_string(), String::new()));
        }
        map
    })
}

/// Primary time zone identifiers for `Intl.supportedValuesOf("timeZone")`:
/// the tzdata Zone entries (`UTC` for `Etc/UTC`), sorted.
pub(super) fn time_zone_ids() -> Vec<String> {
    let mut out = vec!["UTC".to_string()];
    let keep = |name: &str| !matches!(name, "Etc/UTC" | "Etc/GMT" | "Factory") && zone_table().contains_key(&name.to_ascii_lowercase());
    let mut system = false;
    for &dir in dt::system_zoneinfo_dirs() {
        let Ok(zi) = std::fs::read_to_string(std::path::Path::new(dir).join("tzdata.zi")) else { continue };
        for line in zi.lines() {
            let mut it = line.split_whitespace();
            if it.next() != Some("Z") {
                continue;
            }
            let Some(name) = it.next() else { continue };
            if keep(name) {
                out.push(name.to_string());
            }
        }
        system = true;
        break;
    }
    if !system {
        out.extend(dt::tzdb::names().into_iter().filter(|&(n, zone)| zone && keep(n)).map(|(n, _)| n.to_string()));
    }
    out.sort();
    out.dedup();
    out
}

fn is_utc_id(id: &str) -> bool {
    matches!(id, "UTC" | "Etc/UTC" | "Etc/UCT" | "UCT" | "Etc/Universal" | "Universal" | "Etc/Zulu" | "Zulu")
}

/// Parse a UTC offset time zone (`±HH`, `±HHMM`, `±HH:MM`) into minutes.
fn parse_offset_zone(s: &str) -> Option<i32> {
    let b = s.as_bytes();
    let sign = match b.first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let rest = &s[1..];
    let digits = |x: &str| x.len() == 2 && x.bytes().all(|c| c.is_ascii_digit());
    let (h, m) = match rest.len() {
        2 => (rest, "00"),
        4 => (&rest[..2], &rest[2..]),
        5 if rest.as_bytes()[2] == b':' => (&rest[..2], &rest[3..]),
        _ => return None,
    };
    if !digits(h) || !digits(m) {
        return None;
    }
    let (h, m): (i32, i32) = (h.parse().ok()?, m.parse().ok()?);
    if h > 23 || m > 59 {
        return None;
    }
    Some(sign * (h * 60 + m))
}

fn format_offset_id(mins: i32) -> String {
    let sign = if mins < 0 { '-' } else { '+' };
    format!("{}{:02}:{:02}", sign, mins.abs() / 60, mins.abs() % 60)
}

/// Resolve a `timeZone` option value: (identifier, zone).
fn resolve_time_zone(s: &str) -> Option<(String, Zone)> {
    if let Some(m) = parse_offset_zone(s) {
        return Some((format_offset_id(m), Zone::Offset(m)));
    }
    let (id, file) = zone_table().get(&s.to_ascii_lowercase())?.clone();
    let zone = if file.is_empty() { None } else { dt::tz_named(&file).map(Zone::Named) };
    match zone {
        Some(z) => Some((id, z)),
        None if is_utc_id(&id) || id == "GMT" || id == "Etc/GMT" => Some((id, Zone::Offset(0))),
        None => None,
    }
}

/// DefaultTimeZone: the host's zone.
fn default_time_zone() -> (String, Zone) {
    let info = dt::tz();
    if !info.name.is_empty() {
        if let Some((id, _)) = zone_table().get(&info.name.to_ascii_lowercase()) {
            return (id.clone(), Zone::Named(info));
        }
    }
    let off = info.offset_at(0).0;
    if info.transitions.is_empty() && info.footer.is_none() && off == 0 {
        return ("UTC".into(), Zone::Offset(0));
    }
    ("UTC".into(), Zone::Named(info))
}

// ---------------------------------------------------------- locale data

struct Names {
    months: [&'static str; 12],
    months_short: [&'static str; 12],
    days: [&'static str; 7],
    days_short: [&'static str; 7],
    am_pm: [&'static str; 2],
    eras_short: [&'static str; 2],
    eras_long: [&'static str; 2],
}

const EN: Names = Names {
    months: dt::MONTH_LONG,
    months_short: dt::MONTH_NAMES,
    days: dt::DAY_LONG,
    days_short: dt::DAY_NAMES,
    am_pm: ["AM", "PM"],
    eras_short: ["BC", "AD"],
    eras_long: ["Before Christ", "Anno Domini"],
};
const ES: Names = Names {
    months: ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"],
    months_short: ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sept", "oct", "nov", "dic"],
    days: ["domingo", "lunes", "martes", "miércoles", "jueves", "viernes", "sábado"],
    days_short: ["dom", "lun", "mar", "mié", "jue", "vie", "sáb"],
    am_pm: ["a.\u{a0}m.", "p.\u{a0}m."],
    eras_short: ["a. C.", "d. C."],
    eras_long: ["antes de Cristo", "después de Cristo"],
};
const DE: Names = Names {
    months: ["Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober", "November", "Dezember"],
    months_short: ["Jan.", "Feb.", "März", "Apr.", "Mai", "Juni", "Juli", "Aug.", "Sept.", "Okt.", "Nov.", "Dez."],
    days: ["Sonntag", "Montag", "Dienstag", "Mittwoch", "Donnerstag", "Freitag", "Samstag"],
    days_short: ["So.", "Mo.", "Di.", "Mi.", "Do.", "Fr.", "Sa."],
    am_pm: ["AM", "PM"],
    eras_short: ["v. Chr.", "n. Chr."],
    eras_long: ["v. Chr.", "n. Chr."],
};
const FR: Names = Names {
    months: ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre"],
    months_short: ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."],
    days: ["dimanche", "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi"],
    days_short: ["dim.", "lun.", "mar.", "mer.", "jeu.", "ven.", "sam."],
    am_pm: ["AM", "PM"],
    eras_short: ["av. J.-C.", "ap. J.-C."],
    eras_long: ["avant Jésus-Christ", "après Jésus-Christ"],
};
const IT: Names = Names {
    months: ["gennaio", "febbraio", "marzo", "aprile", "maggio", "giugno", "luglio", "agosto", "settembre", "ottobre", "novembre", "dicembre"],
    months_short: ["gen", "feb", "mar", "apr", "mag", "giu", "lug", "ago", "set", "ott", "nov", "dic"],
    days: ["domenica", "lunedì", "martedì", "mercoledì", "giovedì", "venerdì", "sabato"],
    days_short: ["dom", "lun", "mar", "mer", "gio", "ven", "sab"],
    am_pm: ["AM", "PM"],
    eras_short: ["a.C.", "d.C."],
    eras_long: ["avanti Cristo", "dopo Cristo"],
};
const PT: Names = Names {
    months: ["janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro", "novembro", "dezembro"],
    months_short: ["jan.", "fev.", "mar.", "abr.", "mai.", "jun.", "jul.", "ago.", "set.", "out.", "nov.", "dez."],
    days: ["domingo", "segunda-feira", "terça-feira", "quarta-feira", "quinta-feira", "sexta-feira", "sábado"],
    days_short: ["dom.", "seg.", "ter.", "qua.", "qui.", "sex.", "sáb."],
    am_pm: ["AM", "PM"],
    eras_short: ["a.C.", "d.C."],
    eras_long: ["antes de Cristo", "depois de Cristo"],
};
const CJK_MONTHS: [&str; 12] = ["1月", "2月", "3月", "4月", "5月", "6月", "7月", "8月", "9月", "10月", "11月", "12月"];
const JA: Names = Names {
    months: CJK_MONTHS,
    months_short: CJK_MONTHS,
    days: ["日曜日", "月曜日", "火曜日", "水曜日", "木曜日", "金曜日", "土曜日"],
    days_short: ["日", "月", "火", "水", "木", "金", "土"],
    am_pm: ["午前", "午後"],
    eras_short: ["紀元前", "西暦"],
    eras_long: ["紀元前", "西暦"],
};
const ZH: Names = Names {
    months: CJK_MONTHS,
    months_short: CJK_MONTHS,
    days: ["星期日", "星期一", "星期二", "星期三", "星期四", "星期五", "星期六"],
    days_short: ["周日", "周一", "周二", "周三", "周四", "周五", "周六"],
    am_pm: ["上午", "下午"],
    eras_short: ["公元前", "公元"],
    eras_long: ["公元前", "公元"],
};
const KO: Names = Names {
    months: ["1월", "2월", "3월", "4월", "5월", "6월", "7월", "8월", "9월", "10월", "11월", "12월"],
    months_short: ["1월", "2월", "3월", "4월", "5월", "6월", "7월", "8월", "9월", "10월", "11월", "12월"],
    days: ["일요일", "월요일", "화요일", "수요일", "목요일", "금요일", "토요일"],
    days_short: ["일", "월", "화", "수", "목", "금", "토"],
    am_pm: ["오전", "오후"],
    eras_short: ["BC", "AD"],
    eras_long: ["기원전", "서기"],
};

const ISLAMIC_MONTHS: [&str; 12] =
    ["Muharram", "Safar", "Rabiʻ I", "Rabiʻ II", "Jumada I", "Jumada II", "Rajab", "Shaʻban", "Ramadan", "Shawwal", "Dhuʻl-Qiʻdah", "Dhuʻl-Hijjah"];
const ISLAMIC_MONTHS_SHORT: [&str; 12] = ["Muh.", "Saf.", "Rab. I", "Rab. II", "Jum. I", "Jum. II", "Raj.", "Sha.", "Ram.", "Shaw.", "Dhuʻl-Q.", "Dhuʻl-H."];

/// Japanese eras since Meiji: (start year, month, day, name, narrow).
const JAPANESE_ERAS: [(i64, i64, i64, &str, &str); 5] = [
    (1868, 9, 8, "Meiji", "M"),
    (1912, 7, 30, "Taishō", "T"),
    (1926, 12, 25, "Shōwa", "S"),
    (1989, 1, 8, "Heisei", "H"),
    (2019, 5, 1, "Reiwa", "R"),
];

struct Loc<'a> {
    lang: &'a str,
    region: &'a str,
}

impl Loc<'_> {
    fn us(&self) -> bool {
        self.lang == "en" && matches!(self.region, "" | "US" | "PH" | "UM" | "VI" | "GU" | "PR" | "AS" | "MP")
    }

    fn names(&self) -> &'static Names {
        match self.lang {
            "es" => &ES,
            "de" => &DE,
            "fr" => &FR,
            "it" => &IT,
            "pt" => &PT,
            "ja" => &JA,
            "zh" => &ZH,
            "ko" => &KO,
            _ => &EN,
        }
    }

    fn default_hc(&self) -> Hc {
        let twelve = self.us()
            || matches!(self.region, "IN" | "PK" | "BD" | "EG" | "SA" | "TW" | "KR")
            || matches!(self.lang, "ar" | "hi" | "bn" | "ko" | "ur" | "fil" | "mr" | "ta" | "te" | "kn" | "ml" | "gu" | "pa" | "ne")
            || (self.lang == "en" && matches!(self.region, "CA" | "AU" | "NZ"));
        if twelve {
            Hc::H12
        } else {
            Hc::H23
        }
    }

    fn hc12(&self) -> Hc {
        if self.lang == "ja" {
            Hc::H11
        } else {
            Hc::H12
        }
    }

    /// Whether 24-hour clocks zero-pad the hour ("HH").
    fn pads_24h(&self) -> bool {
        !matches!(self.lang, "es" | "ja" | "fi" | "ca" | "ko")
    }

    fn cjk(&self) -> bool {
        matches!(self.lang, "ja" | "zh")
    }
}

/// Quote literal text for a pattern.
fn q(s: &str) -> String {
    if s.chars().any(|c| c.is_ascii_alphabetic() || c == '\'') {
        format!("'{}'", s.replace('\'', "''"))
    } else {
        s.to_string()
    }
}

fn numeric_date(l: &Loc, y: Option<&str>, m: Option<&str>, d: Option<&str>) -> String {
    // (order, separator, pad day, pad month, suffix)
    let (order, sep, pad_d, pad_m, suffix): (&str, &str, bool, bool, &str) = match l.lang {
        "en" if l.us() => ("mdy", "/", false, false, ""),
        "en" | "fr" | "pt" => ("dmy", "/", true, true, ""),
        "ru" | "uk" | "tr" | "be" | "kk" | "az" => ("dmy", ".", true, true, ""),
        "pl" => ("dmy", ".", false, true, ""),
        "de" | "nb" | "no" | "nn" | "da" | "fi" | "is" | "cs" | "sk" | "hr" | "sl" | "sr" | "bs" | "mk" | "et" | "lv" | "ro" | "bg" => ("dmy", ".", false, false, ""),
        "nl" => ("dmy", "-", false, false, ""),
        "ja" | "zh" => ("ymd", "/", false, false, ""),
        "ko" => ("ymd", ". ", false, false, "."),
        "hu" => ("ymd", ". ", true, true, "."),
        "sv" | "lt" => ("ymd", "-", true, true, ""),
        _ => ("dmy", "/", false, false, ""),
    };
    let present = [y.is_some(), m.is_some(), d.is_some()].iter().filter(|b| **b).count();
    if present == 1 {
        if let Some(m) = m {
            return if l.cjk() { format!("{}月", m) } else { m.to_string() };
        }
        if let Some(d) = d {
            return if l.cjk() { format!("{}日", d) } else { d.to_string() };
        }
        if let Some(y) = y {
            return if l.cjk() { format!("{}年", y) } else { y.to_string() };
        }
    }
    let mut parts = Vec::new();
    for c in order.chars() {
        let p = match c {
            'y' => y.map(str::to_string),
            'm' => m.map(|m| if pad_m && present == 3 { "MM".to_string() } else { m.to_string() }),
            _ => d.map(|d| if pad_d { "dd".to_string() } else { d.to_string() }),
        };
        if let Some(p) = p {
            parts.push(p);
        }
    }
    let mut s = parts.join(&q(sep));
    s.push_str(suffix);
    s
}

fn text_date(l: &Loc, m: &str, d: Option<&str>, y: Option<&str>) -> String {
    let (dmy, dm, my): (&str, &str, &str) = match l.lang {
        "en" if l.us() => ("{M} {d}, {y}", "{M} {d}", "{M} {y}"),
        "es" if m.len() >= 4 => ("{d} 'de' {M} 'de' {y}", "{d} 'de' {M}", "{M} 'de' {y}"),
        "pt" => ("{d} 'de' {M} 'de' {y}", "{d} 'de' {M}", "{M} 'de' {y}"),
        "de" => ("{d}. {M} {y}", "{d}. {M}", "{M} {y}"),
        "ja" | "zh" => ("{y}年{M}{d}日", "{M}{d}日", "{y}年{M}"),
        "ko" => ("{y}년 {M} {d}일", "{M} {d}일", "{y}년 {M}"),
        _ => ("{d} {M} {y}", "{d} {M}", "{M} {y}"),
    };
    let t = match (d, y) {
        (Some(_), Some(_)) => dmy,
        (Some(_), None) => dm,
        (None, Some(_)) => my,
        (None, None) => "{M}",
    };
    t.replace("{M}", m).replace("{d}", d.unwrap_or("")).replace("{y}", y.unwrap_or(""))
}

fn weekday_join(l: &Loc, e: &str, date: &str, numeric: bool) -> String {
    match l.lang {
        "en" | "es" | "de" | "pt" => format!("{}, {}", e, date),
        "ja" if numeric => format!("{}({})", date, e),
        "ja" | "zh" => format!("{}{}", date, e),
        "ko" => format!("{} {}", date, e),
        _ => format!("{} {}", e, date),
    }
}

fn gen_date(l: &Loc, c: &Comps, cal: Cal) -> String {
    let y = c.v[C_YEAR].map(|w| if w == "2-digit" { "yy" } else { "y" });
    let d = c.v[C_DAY].map(|w| if w == "2-digit" { "dd" } else { "d" });
    let textual = matches!(c.v[C_MONTH], Some("short" | "long" | "narrow"));
    let mut date = if textual {
        let m = match c.v[C_MONTH] {
            Some("long") => "MMMM",
            Some("narrow") => "MMMMM",
            _ => "MMM",
        };
        text_date(l, m, d, y)
    } else if y.is_some() || c.v[C_MONTH].is_some() || d.is_some() {
        let m = c.v[C_MONTH].map(|w| if w == "2-digit" { "MM" } else { "M" });
        numeric_date(l, y, m, d)
    } else {
        String::new()
    };
    if let Some(w) = c.v[C_WEEKDAY] {
        let e = match w {
            "long" => "EEEE",
            "narrow" => "EEEEE",
            _ => "EEE",
        };
        date = if date.is_empty() { e.to_string() } else { weekday_join(l, e, &date, !textual) };
    }
    if c.v[C_ERA].is_some() || (cal.implicit_era() && y.is_some()) {
        let g = match c.v[C_ERA] {
            Some("long") => "GGGG",
            Some("narrow") => "GGGGG",
            _ if cal == Cal::Japanese && !textual => "GGGGG",
            _ => "G",
        };
        date = if date.is_empty() { g.to_string() } else { format!("{} {}", date, g) };
    }
    date
}

fn gen_time(l: &Loc, c: &Comps, hc: Option<Hc>) -> String {
    let hc12 = hc.map(Hc::is12).unwrap_or(false);
    let mut parts: Vec<String> = Vec::new();
    if let Some(w) = c.v[C_HOUR] {
        let ch = match hc.unwrap_or(Hc::H23) {
            Hc::H11 => 'K',
            Hc::H12 => 'h',
            Hc::H23 => 'H',
            Hc::H24 => 'k',
        };
        let two = w == "2-digit" || (!hc12 && l.pads_24h());
        parts.push(std::iter::repeat_n(ch, if two { 2 } else { 1 }).collect());
    }
    if let Some(w) = c.v[C_MINUTE] {
        parts.push(if c.v[C_HOUR].is_some() || c.v[C_SECOND].is_some() || w == "2-digit" { "mm" } else { "m" }.into());
    }
    if let Some(w) = c.v[C_SECOND] {
        parts.push(if c.v[C_MINUTE].is_some() || c.v[C_HOUR].is_some() || w == "2-digit" { "ss" } else { "s" }.into());
    }
    let mut time = parts.join(":");
    if let Some(n) = c.fsd {
        let s = "S".repeat(n);
        time = if time.is_empty() { s } else { format!("{}.{}", time, s) };
    }
    let period = match c.v[C_DAY_PERIOD] {
        Some("long") => Some("BBBB"),
        Some("narrow") => Some("BBBBB"),
        Some(_) => Some("B"),
        None => None,
    };
    if c.v[C_HOUR].is_some() {
        if hc12 {
            let a = period.unwrap_or("a");
            time = match l.lang {
                "ja" | "zh" => format!("{}{}", a, time),
                "ko" => format!("{} {}", a, time),
                "es" => format!("{}\u{a0}{}", time, a),
                _ => format!("{} {}", time, a),
            };
        }
    } else if let Some(b) = period {
        time = if time.is_empty() { b.to_string() } else { format!("{} {}", time, b) };
    }
    if let Some(z) = c.v[C_TZ_NAME] {
        let zf = match z {
            "long" => "zzzz",
            "shortOffset" => "O",
            "longOffset" => "OOOO",
            "shortGeneric" => "v",
            "longGeneric" => "vvvv",
            _ => "z",
        };
        time = if time.is_empty() { zf.to_string() } else { format!("{} {}", time, zf) };
    }
    time
}

/// The connector between a date and a time pattern.
fn date_time_joiner(l: &Loc, date_style: Option<Style>) -> &'static str {
    let full = matches!(date_style, Some(Style::Full | Style::Long));
    match l.lang {
        "en" if full => " 'at' ",
        "de" if full => " 'um' ",
        "fr" if full => " 'à' ",
        "pt" if full => " 'às' ",
        "fr" | "ja" | "zh" | "ko" => " ",
        _ => ", ",
    }
}

fn date_style_pattern(l: &Loc, s: Style, cal: Cal) -> String {
    let table: Option<[&str; 4]> = match l.lang {
        "en" if l.us() => Some(["EEEE, MMMM d, y", "MMMM d, y", "MMM d, y", "M/d/yy"]),
        "en" => Some(["EEEE d MMMM y", "d MMMM y", "d MMM y", "dd/MM/y"]),
        "es" => Some(["EEEE, d 'de' MMMM 'de' y", "d 'de' MMMM 'de' y", "d MMM y", "d/M/yy"]),
        "de" => Some(["EEEE, d. MMMM y", "d. MMMM y", "dd.MM.y", "dd.MM.yy"]),
        "fr" => Some(["EEEE d MMMM y", "d MMMM y", "d MMM y", "dd/MM/y"]),
        "it" => Some(["EEEE d MMMM y", "d MMMM y", "d MMM y", "dd/MM/yy"]),
        "pt" => Some(["EEEE, d 'de' MMMM 'de' y", "d 'de' MMMM 'de' y", "d 'de' MMM 'de' y", "dd/MM/y"]),
        "ja" => Some(["y年M月d日EEEE", "y年M月d日", "y/MM/dd", "y/MM/dd"]),
        "zh" => Some(["y年M月d日EEEE", "y年M月d日", "y年M月d日", "y/M/d"]),
        "ko" => Some(["y년 M월 d일 EEEE", "y년 M월 d일", "y. M. d.", "yy. M. d."]),
        _ => None,
    };
    let mut p = match table {
        Some(t) => t[s as usize].to_string(),
        None => {
            let mut c = Comps::default();
            c.v[C_YEAR] = Some(if s == Style::Short { "2-digit" } else { "numeric" });
            c.v[C_MONTH] = Some(match s {
                Style::Full | Style::Long => "long",
                Style::Medium => "short",
                Style::Short => "numeric",
            });
            c.v[C_DAY] = Some("numeric");
            if s == Style::Full {
                c.v[C_WEEKDAY] = Some("long");
            }
            gen_date(l, &c, Cal::Gregory)
        }
    };
    if cal.implicit_era() && !p.contains('G') {
        p.push_str(if cal == Cal::Japanese && s == Style::Short { " GGGGG" } else { " G" });
    }
    p
}

fn time_style_comps(s: Style) -> Comps {
    let mut c = Comps::default();
    c.v[C_HOUR] = Some("numeric");
    c.v[C_MINUTE] = Some("2-digit");
    if s != Style::Short {
        c.v[C_SECOND] = Some("2-digit");
    }
    match s {
        Style::Full => c.v[C_TZ_NAME] = Some("long"),
        Style::Long => c.v[C_TZ_NAME] = Some("short"),
        _ => {}
    }
    c
}

fn build_pattern(d: &Dtf) -> String {
    let l = Loc { lang: &d.lang, region: &d.region };
    let (date, time) = if d.date_style.is_some() || d.time_style.is_some() {
        (d.date_style.map(|s| date_style_pattern(&l, s, d.cal)).unwrap_or_default(), d.time_style.map(|s| gen_time(&l, &time_style_comps(s), d.hc)).unwrap_or_default())
    } else {
        (gen_date(&l, &d.comps, d.cal), gen_time(&l, &d.comps, d.hc))
    };
    match (date.is_empty(), time.is_empty()) {
        (false, false) => format!("{}{}{}", date, date_time_joiner(&l, d.date_style), time),
        (true, _) => time,
        (false, true) => date,
    }
}

fn parse_pattern(p: &str) -> Vec<Tok> {
    let chars: Vec<char> = p.chars().collect();
    let mut out: Vec<Tok> = Vec::new();
    let push_lit = |out: &mut Vec<Tok>, s: &str| {
        if let Some(Tok::Lit(prev)) = out.last_mut() {
            prev.push_str(s);
        } else {
            out.push(Tok::Lit(s.to_string()));
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            if chars.get(i + 1) == Some(&'\'') {
                push_lit(&mut out, "'");
                i += 2;
                continue;
            }
            let mut lit = String::new();
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        lit.push('\'');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                lit.push(chars[i]);
                i += 1;
            }
            push_lit(&mut out, &lit);
        } else if c.is_ascii_alphabetic() {
            let mut n = 1;
            while chars.get(i + n) == Some(&c) {
                n += 1;
            }
            out.push(Tok::F(c, n));
            i += n;
        } else {
            push_lit(&mut out, &c.to_string());
            i += 1;
        }
    }
    out
}

// ------------------------------------------------------------- creation

fn u_keywords(tag: &str) -> Vec<(String, String)> {
    match locale_id::parse(tag) {
        Some(mut id) => {
            locale_id::canonicalize_id(&mut id);
            id.u_keywords
        }
        None => Vec::new(),
    }
}

/// CreateDateTimeFormat (§11.1.2) minus the object allocation.
fn create(realm: &mut Realm, locales: Value, options: Value, required: Required, defaults: Defaults) -> JsResult<Dtf> {
    let requested = canonical_locale_list(realm, locales)?;
    let opts = match options.v() {
        V::Undefined => None,
        _ => Some(realm.to_object(options)?),
    };
    get_enum(realm, opts, "localeMatcher", &["lookup", "best fit"], Some("best fit"))?;
    let calendar = get_string(realm, opts, "calendar")?;
    if let Some(c) = &calendar {
        if !super::duration::is_unicode_type(c) {
            return realm.range_error(&format!("Invalid calendar : {}", c));
        }
    }
    let numbering = get_string(realm, opts, "numberingSystem")?;
    if let Some(n) = &numbering {
        if !super::duration::is_unicode_type(n) {
            return realm.range_error(&format!("Invalid numberingSystem : {}", n));
        }
    }
    let hour12 = get_bool(realm, opts, "hour12")?;
    let hour_cycle = get_enum(realm, opts, "hourCycle", &["h11", "h12", "h23", "h24"], None)?;

    // ResolveLocale with the relevant extension keys « ca, hc, nu ».
    let found = requested.iter().find(|t| is_available(t)).and_then(|t| Locale::parse(t));
    let (base, keywords) = match &found {
        Some(l) => (l.clone(), u_keywords(&l.full)),
        None => (Locale::default_locale(), Vec::new()),
    };
    let kw = |k: &str| keywords.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());

    let mut cal = Cal::Gregory;
    let mut ca_kw = None;
    if let Some(v) = kw("ca") {
        let v = canonical_calendar(&v);
        if let Some(c) = calendar_by_name(&v) {
            cal = c;
            ca_kw = Some(v);
        }
    }
    if let Some(o) = &calendar {
        let o = canonical_calendar(&o.to_ascii_lowercase());
        if let Some(c) = calendar_by_name(&o) {
            if c != cal {
                cal = c;
                ca_kw = None;
            }
        }
    }

    let mut hc_val: Option<Hc> = None;
    let mut hc_kw = None;
    if let Some(v) = kw("hc") {
        if let Some(h) = Hc::parse(&v) {
            hc_val = Some(h);
            hc_kw = Some(v);
        }
    }
    if hour12.is_some() {
        // hourCycle is null: the extension is not used.
        if hc_val.is_some() {
            hc_val = None;
            hc_kw = None;
        }
    } else if let Some(h) = hour_cycle.and_then(Hc::parse) {
        if Some(h) != hc_val {
            hc_val = Some(h);
            hc_kw = None;
        }
    }

    let mut nu = "latn".to_string();
    let mut nu_kw = None;
    if let Some(v) = kw("nu") {
        if numbering_zero(&v).is_some() {
            nu = v.clone();
            nu_kw = Some(v);
        }
    }
    if let Some(o) = &numbering {
        let o = o.to_ascii_lowercase();
        if numbering_zero(&o).is_some() && o != nu {
            nu = o;
            nu_kw = None;
        }
    }

    let mut locale = base.tag.clone();
    let mut ext = String::new();
    for (k, v) in [("ca", &ca_kw), ("hc", &hc_kw), ("nu", &nu_kw)] {
        if let Some(v) = v {
            ext.push_str(&format!("-{}-{}", k, v));
        }
    }
    if !ext.is_empty() {
        locale = format!("{}-u{}", locale, ext);
    }

    // Time zone.
    let tz_value = get_value(realm, opts, "timeZone")?;
    let (tz_id, zone) = if tz_value.is_undefined() {
        default_time_zone()
    } else {
        let s = realm.to_rust_string(tz_value)?;
        match resolve_time_zone(&s) {
            Some(r) => r,
            None => return realm.range_error(&format!("Invalid time zone specified: {}", s)),
        }
    };

    // Components (Table 16), with fractionalSecondDigits after second.
    let mut comps = Comps::default();
    for (i, (name, values)) in COMPONENTS.iter().enumerate() {
        if i == C_TZ_NAME {
            comps.fsd = get_number(realm, opts, "fractionalSecondDigits", 1.0, 3.0)?;
        }
        comps.v[i] = get_enum(realm, opts, name, values, None)?;
    }
    get_enum(realm, opts, "formatMatcher", &["basic", "best fit"], Some("best fit"))?;
    let date_style = get_enum(realm, opts, "dateStyle", STYLES, None)?.map(parse_style);
    let time_style = get_enum(realm, opts, "timeStyle", STYLES, None)?.map(parse_style);

    if date_style.is_some() || time_style.is_some() {
        if comps.v.iter().any(Option::is_some) || comps.fsd.is_some() {
            return realm.type_error("Can't set option dateStyle/timeStyle together with other date-time components");
        }
        if required == Required::Date && time_style.is_some() {
            return realm.type_error("Invalid option : timeStyle");
        }
        if required == Required::Time && date_style.is_some() {
            return realm.type_error("Invalid option : dateStyle");
        }
    } else {
        let date_fields = [C_WEEKDAY, C_YEAR, C_MONTH, C_DAY];
        let time_fields = [C_DAY_PERIOD, C_HOUR, C_MINUTE, C_SECOND];
        let mut need_defaults = true;
        if matches!(required, Required::Date | Required::Any) && date_fields.iter().any(|&i| comps.v[i].is_some()) {
            need_defaults = false;
        }
        if matches!(required, Required::Time | Required::Any) && (time_fields.iter().any(|&i| comps.v[i].is_some()) || comps.fsd.is_some()) {
            need_defaults = false;
        }
        if need_defaults && matches!(defaults, Defaults::Date | Defaults::All) {
            for i in [C_YEAR, C_MONTH, C_DAY] {
                comps.v[i] = Some("numeric");
            }
        }
        if need_defaults && matches!(defaults, Defaults::Time | Defaults::All) {
            for i in [C_HOUR, C_MINUTE, C_SECOND] {
                comps.v[i] = Some("numeric");
            }
        }
    }

    let lang = base.lang.clone();
    let region = base.region.clone().unwrap_or_default();
    let l = Loc { lang: &lang, region: &region };
    let has_hour = time_style.is_some() || (date_style.is_none() && comps.v[C_HOUR].is_some());
    let hc = if has_hour {
        let mut hc = hc_val.unwrap_or_else(|| l.default_hc());
        match hour12 {
            Some(true) => hc = l.hc12(),
            Some(false) => hc = Hc::H23,
            None => {}
        }
        Some(hc)
    } else {
        None
    };

    let mut d = Dtf { locale, lang: lang.clone(), region: region.clone(), cal, nu, tz_id, zone, hc, comps, date_style, time_style, pattern: Vec::new() };
    d.pattern = parse_pattern(&build_pattern(&d));
    Ok(d)
}

// ------------------------------------------------------------ formatting

/// A date in the formatter's calendar.
struct CalDate {
    era: usize,
    year: i64,
    month: usize,
    day: i64,
}

fn floor_div(a: i64, b: i64) -> i64 {
    a.div_euclid(b)
}

fn islamic_to_jdn(y: i64, m: i64, d: i64, epoch: i64) -> i64 {
    d + (59 * (m - 1) + 1) / 2 + (y - 1) * 354 + floor_div(3 + 11 * y, 30) + epoch - 1
}

fn islamic_from_jdn(jdn: i64, epoch: i64) -> (i64, i64, i64) {
    let year = floor_div(30 * (jdn - epoch) + 10646, 10631);
    let x = 2 * (jdn - (29 + islamic_to_jdn(year, 1, 1, epoch)));
    let month = (-floor_div(-x, 59) + 1).clamp(1, 12);
    let day = jdn - islamic_to_jdn(year, month, 1, epoch) + 1;
    (year, month, day)
}

fn to_calendar(cal: Cal, f: &dt::Fields, local_ms: f64) -> CalDate {
    let (y, m, d) = (f.year as i64, f.month as usize, f.day as i64);
    match cal {
        Cal::Gregory | Cal::Iso8601 => {
            if y <= 0 {
                CalDate { era: 0, year: 1 - y, month: m, day: d }
            } else {
                CalDate { era: 1, year: y, month: m, day: d }
            }
        }
        Cal::Buddhist => CalDate { era: 0, year: y + 543, month: m, day: d },
        Cal::Roc => {
            let r = y - 1911;
            if r > 0 {
                CalDate { era: 1, year: r, month: m, day: d }
            } else {
                CalDate { era: 0, year: 1 - r, month: m, day: d }
            }
        }
        Cal::Japanese => {
            let key = (y, m as i64 + 1, d);
            match JAPANESE_ERAS.iter().rposition(|&(ey, em, ed, _, _)| key >= (ey, em, ed)) {
                Some(i) => CalDate { era: 2 + i, year: y - JAPANESE_ERAS[i].0 + 1, month: m, day: d },
                None if y <= 0 => CalDate { era: 0, year: 1 - y, month: m, day: d },
                None => CalDate { era: 1, year: y, month: m, day: d },
            }
        }
        Cal::IslamicCivil | Cal::IslamicTbla => {
            let epoch = if cal == Cal::IslamicCivil { 1_948_440 } else { 1_948_439 };
            let days = (local_ms / dt::MS_PER_DAY).floor() as i64;
            let (iy, im, id) = islamic_from_jdn(days + 2_440_588, epoch);
            CalDate { era: 0, year: iy, month: (im - 1) as usize, day: id }
        }
    }
}

fn era_name(d: &Dtf, era: usize, width: usize) -> String {
    let l = Loc { lang: &d.lang, region: &d.region };
    let names = l.names();
    let pick = |short: &str, long: &str, narrow: &str| -> String {
        match width {
            4 => long.to_string(),
            5 => narrow.to_string(),
            _ => short.to_string(),
        }
    };
    match d.cal {
        Cal::Buddhist => "BE".into(),
        Cal::IslamicCivil | Cal::IslamicTbla => "AH".into(),
        Cal::Roc => {
            if era == 1 {
                "Minguo".into()
            } else {
                "B.R.O.C.".into()
            }
        }
        Cal::Japanese if era >= 2 => {
            let (_, _, _, name, narrow) = JAPANESE_ERAS[era - 2];
            pick(name, name, narrow)
        }
        _ => {
            let short = names.eras_short[era.min(1)];
            let narrow: String = if d.lang == "en" { short.chars().take(1).collect() } else { short.to_string() };
            pick(short, names.eras_long[era.min(1)], &narrow)
        }
    }
}

fn narrow_of(s: &str) -> String {
    s.chars().take(1).collect::<String>().to_uppercase()
}

fn pad(n: i64, width: usize) -> String {
    if n < 0 {
        format!("-{:0w$}", -n, w = width)
    } else {
        format!("{:0w$}", n, w = width)
    }
}

/// English flexible day periods (CLDR `dayPeriods` format names).
fn flexible_day_period(h: i64, m: i64, s: i64, narrow: bool) -> &'static str {
    if h == 12 && m == 0 && s == 0 {
        return if narrow { "n" } else { "noon" };
    }
    match h {
        6..=11 => "in the morning",
        12..=17 => "in the afternoon",
        18..=20 => "in the evening",
        _ => "at night",
    }
}

const US_ZONES: [(&str, &str, &str, &str); 6] = [
    ("EST", "EDT", "ET", "Eastern Time"),
    ("CST", "CDT", "CT", "Central Time"),
    ("MST", "MDT", "MT", "Mountain Time"),
    ("PST", "PDT", "PT", "Pacific Time"),
    ("AKST", "AKDT", "AKT", "Alaska Time"),
    ("HST", "HDT", "HST", "Hawaii-Aleutian Time"),
];

fn offset_name(off_ms: f64, long: bool) -> String {
    let mins = (off_ms / 60000.0).round() as i64;
    if mins == 0 {
        return "GMT".into();
    }
    let sign = if mins < 0 { '-' } else { '+' };
    let (h, m) = (mins.abs() / 60, mins.abs() % 60);
    if long {
        format!("GMT{}{:02}:{:02}", sign, h, m)
    } else if m != 0 {
        format!("GMT{}{}:{:02}", sign, h, m)
    } else {
        format!("GMT{}{}", sign, h)
    }
}

fn zone_name(d: &Dtf, kind: char, count: usize, off_ms: f64, abbr: &str) -> String {
    let utc = is_utc_id(&d.tz_id);
    let us = US_ZONES.iter().find(|z| z.0 == abbr || z.1 == abbr);
    match (kind, count >= 4) {
        ('O', long) => offset_name(off_ms, long),
        ('v', long) => {
            if utc {
                return if long { "Coordinated Universal Time".into() } else { "UTC".into() };
            }
            if let Some(z) = us {
                return if long { z.3.into() } else { z.2.into() };
            }
            if long {
                let name = dt::long_zone_name(abbr);
                if name != abbr {
                    return name.replace(" Standard Time", " Time").replace(" Summer Time", " Time").replace(" Daylight Time", " Time");
                }
            }
            offset_name(off_ms, long)
        }
        (_, true) => {
            if utc {
                return "Coordinated Universal Time".into();
            }
            if matches!(d.zone, Zone::Named(_)) {
                let name = dt::long_zone_name(abbr);
                if name != abbr {
                    return name;
                }
            }
            offset_name(off_ms, true)
        }
        (_, false) => {
            if utc {
                return "UTC".into();
            }
            if matches!(d.zone, Zone::Named(_)) && d.lang == "en" && (us.is_some() || (abbr == "GMT" && off_ms == 0.0)) {
                return abbr.into();
            }
            offset_name(off_ms, false)
        }
    }
}

fn decimal_separator(nu: &str) -> &'static str {
    match nu {
        "arab" | "arabext" => "\u{66b}",
        _ => ".",
    }
}

/// FormatDateTimePattern: the (type, value) parts of `t` (a clipped time
/// value).
fn format_parts(d: &Dtf, t: f64) -> Vec<(&'static str, String)> {
    let (off_ms, abbr) = match &d.zone {
        Zone::Offset(m) => (*m as f64 * 60000.0, String::new()),
        Zone::Named(info) => dt::zone_offset(info, t),
    };
    let local = t + off_ms;
    let f = dt::fields(local);
    let cd = to_calendar(d.cal, &f, local);
    let l = Loc { lang: &d.lang, region: &d.region };
    let names = l.names();
    let islamic = matches!(d.cal, Cal::IslamicCivil | Cal::IslamicTbla);
    let (h, mi, s) = (f.hour as i64, f.minute as i64, f.second as i64);
    let num = |n: i64, count: usize| transliterate_digits(&pad(n, count.min(2)), &d.nu);
    let mut out: Vec<(&'static str, String)> = Vec::new();
    for (i, tok) in d.pattern.iter().enumerate() {
        let (ty, v) = match tok {
            Tok::Lit(s) => {
                let s = if s == "." && matches!(d.pattern.get(i + 1), Some(Tok::F('S', _))) { decimal_separator(&d.nu).to_string() } else { s.clone() };
                ("literal", s)
            }
            Tok::F(c, n) => {
                let n = *n;
                match c {
                    'G' => ("era", era_name(d, cd.era, n)),
                    'y' => {
                        if n == 2 {
                            ("year", transliterate_digits(&pad(cd.year.rem_euclid(100), 2), &d.nu))
                        } else {
                            ("year", transliterate_digits(&cd.year.to_string(), &d.nu))
                        }
                    }
                    'M' | 'L' => {
                        let v = match n {
                            1 | 2 => num(cd.month as i64 + 1, n),
                            _ => {
                                let (long, short) = if islamic { (ISLAMIC_MONTHS[cd.month], ISLAMIC_MONTHS_SHORT[cd.month]) } else { (names.months[cd.month], names.months_short[cd.month]) };
                                match n {
                                    3 => short.to_string(),
                                    4 => long.to_string(),
                                    _ => {
                                        if l.cjk() || d.lang == "ko" {
                                            transliterate_digits(&(cd.month + 1).to_string(), &d.nu)
                                        } else {
                                            narrow_of(long)
                                        }
                                    }
                                }
                            }
                        };
                        ("month", v)
                    }
                    'd' => ("day", num(cd.day, n)),
                    'E' | 'c' => {
                        let wd = f.weekday as usize;
                        let v = match n {
                            4 => names.days[wd].to_string(),
                            5 => {
                                if l.cjk() || d.lang == "ko" {
                                    names.days_short[wd].chars().last().map(String::from).unwrap_or_default()
                                } else {
                                    narrow_of(names.days[wd])
                                }
                            }
                            _ => names.days_short[wd].to_string(),
                        };
                        ("weekday", v)
                    }
                    'a' => ("dayPeriod", names.am_pm[(h >= 12) as usize].to_string()),
                    'B' => {
                        let v = if d.lang == "en" {
                            let shows_min = d.pattern.iter().any(|t| matches!(t, Tok::F('m', _)));
                            let (m0, s0) = if shows_min { (mi, s) } else { (0, 0) };
                            flexible_day_period(h, m0, s0, n == 5).to_string()
                        } else {
                            names.am_pm[(h >= 12) as usize].to_string()
                        };
                        ("dayPeriod", v)
                    }
                    'h' => ("hour", num(if h % 12 == 0 { 12 } else { h % 12 }, n)),
                    'K' => ("hour", num(h % 12, n)),
                    'H' => ("hour", num(h, n)),
                    'k' => ("hour", num(if h == 0 { 24 } else { h }, n)),
                    'm' => ("minute", num(mi, n)),
                    's' => ("second", num(s, n)),
                    'S' => {
                        let ms = format!("{:03}", f.ms as i64);
                        ("fractionalSecond", transliterate_digits(&ms[..n.min(3)], &d.nu))
                    }
                    'z' | 'O' | 'v' | 'V' => ("timeZoneName", zone_name(d, *c, n, off_ms, &abbr)),
                    _ => ("literal", String::new()),
                }
            }
        };
        out.push((ty, v));
    }
    out
}

fn format_string(d: &Dtf, t: f64) -> String {
    format_parts(d, t).into_iter().map(|(_, v)| v).collect()
}

/// Coarseness of a pattern field for range formatting: 0 era/year,
/// 1 month, 2 day/weekday, 3 day period, 4+ time fields.
fn field_rank(c: char) -> u8 {
    match c {
        'G' | 'y' | 'U' | 'r' => 0,
        'M' | 'L' => 1,
        'd' | 'E' | 'c' => 2,
        'a' | 'B' => 3,
        'h' | 'H' | 'K' | 'k' => 4,
        'm' => 5,
        's' => 6,
        'S' => 7,
        _ => 8,
    }
}

/// PartitionDateTimeRangePattern: (type, value, source) parts.
fn range_parts(d: &Dtf, x: f64, y: f64) -> Vec<(&'static str, String, &'static str)> {
    let a = format_parts(d, x);
    let b = format_parts(d, y);
    let differs: Vec<bool> = a.iter().zip(&b).map(|(p, q)| p.1 != q.1).collect();
    if !differs.iter().any(|x| *x) {
        return a.into_iter().map(|(t, v)| (t, v, "shared")).collect();
    }
    let fields: Vec<Option<(char, usize)>> = d.pattern.iter().map(|t| if let Tok::F(c, n) = t { Some((*c, *n)) } else { None }).collect();
    let min_rank = fields.iter().zip(&differs).filter(|(_, d)| **d).filter_map(|(f, _)| f.map(|(c, _)| field_rank(c))).min().unwrap_or(0);
    let has_time = fields.iter().flatten().any(|(c, _)| field_rank(*c) >= 3 && field_rank(*c) <= 7);
    let numeric_month = fields.iter().flatten().any(|(c, n)| matches!(c, 'M' | 'L') && *n <= 2);
    let in_span: Vec<bool> = if min_rank <= 2 {
        if has_time || numeric_month || min_rank == 0 {
            vec![true; fields.len()]
        } else {
            fields.iter().map(|f| matches!(f, Some((c, _)) if field_rank(*c) >= min_rank && field_rank(*c) <= 2)).collect()
        }
    } else if min_rank <= 7 {
        let period_differs = fields.iter().zip(&differs).any(|(f, d)| *d && matches!(f, Some((c, _)) if field_rank(*c) == 3));
        fields.iter().map(|f| matches!(f, Some((c, _)) if (4..=7).contains(&field_rank(*c)) || (period_differs && field_rank(*c) == 3))).collect()
    } else {
        vec![true; fields.len()]
    };
    let first = in_span.iter().position(|x| *x).unwrap_or(0);
    let last = in_span.iter().rposition(|x| *x).unwrap_or(fields.len().saturating_sub(1));
    let mut out = Vec::new();
    for (t, v) in a.iter().take(first) {
        out.push((*t, v.clone(), "shared"));
    }
    for (t, v) in a.iter().take(last + 1).skip(first) {
        out.push((*t, v.clone(), "startRange"));
    }
    out.push(("literal", " – ".to_string(), "shared"));
    for (t, v) in b.iter().take(last + 1).skip(first) {
        out.push((*t, v.clone(), "endRange"));
    }
    for (t, v) in a.iter().skip(last + 1) {
        out.push((*t, v.clone(), "shared"));
    }
    out
}

// ------------------------------------------------------------ natives

/// The realm's [[FallbackSymbol]] and the private key caching bound
/// `format` functions.
fn symbols(realm: &mut Realm) -> (SymbolId, SymbolId) {
    let holder = realm.intrinsics.others["%Intl.DateTimeFormat.symbols%"];
    let get = |realm: &Realm, k: &str| match realm.obj(holder).props.get(&PropertyKey::str(k)).map(|p| p.value.clone()) {
        Some(v) => match v.v() {
            V::Symbol(s) => s,
            _ => unreachable!(),
        },
        None => unreachable!(),
    };
    (get(realm, "fallback"), get(realm, "bound"))
}

pub(super) fn setup(realm: &mut Realm, ctor: ObjectId, proto: ObjectId) {
    let fallback = realm.new_symbol(Some(JsStr::new("IntlLegacyConstructedSymbol")));
    let bound = realm.new_private_symbol(JsStr::new("BoundFormat"));
    let holder = realm.new_object();
    realm.define_property(holder, "fallback", Value::Symbol(fallback), 0);
    realm.define_property(holder, "bound", Value::Symbol(bound), 0);
    realm.intrinsics.others.insert("%Intl.DateTimeFormat.symbols%", holder);
    realm.intrinsics.others.insert("%Intl.DateTimeFormat%", ctor);
    realm.define_accessor(proto, "format", Some(format_getter), None, CONFIGURABLE);
    realm.define_method(proto, "formatToParts", 1, format_to_parts);
    realm.define_method(proto, "formatRange", 2, format_range);
    realm.define_method(proto, "formatRangeToParts", 2, format_range_to_parts);
    realm.define_method(proto, "resolvedOptions", 0, resolved_options);
}

pub(super) fn ctor(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let nt = realm.new_target();
    let default = realm.intrinsics.others["Intl.DateTimeFormat"];
    let proto = realm.proto_from_new_target_checked(default)?;
    let dtf = create(realm, arg(args, 0), arg(args, 1), Required::Any, Defaults::Date)?;
    let id = realm.new_object_with_proto(Some(proto));
    realm.set_host_data(id, Box::new(dtf));
    if nt.is_undefined() && this.is_object() {
        let c = realm.intrinsics.others["%Intl.DateTimeFormat%"];
        if realm.ordinary_has_instance(Value::Object(c), this.clone())? {
            let (fallback, _) = symbols(realm);
            let this_id = this.as_object().unwrap();
            let desc = PropDesc { value: Some(Value::Object(id)), writable: Some(false), enumerable: Some(false), configurable: Some(false), ..Default::default() };
            realm.define_property_or_throw(this_id, PropertyKey::Symbol(fallback), desc)?;
            return Ok(this);
        }
    }
    Ok(Value::Object(id))
}

/// UnwrapDateTimeFormat.
fn unwrap(realm: &mut Realm, this: Value, what: &str) -> JsResult<ObjectId> {
    let V::Object(id) = this.v() else {
        return realm.type_error(&format!("Method {} called on incompatible receiver", what));
    };
    if realm.host_data::<Dtf>(id).is_some() {
        return Ok(id);
    }
    let c = realm.intrinsics.others["%Intl.DateTimeFormat%"];
    if realm.ordinary_has_instance(Value::Object(c), this.clone())? {
        let (fallback, _) = symbols(realm);
        let v = realm.get(this, PropertyKey::Symbol(fallback))?;
        if let V::Object(inner) = v.v() {
            if realm.host_data::<Dtf>(inner).is_some() {
                return Ok(inner);
            }
        }
    }
    realm.type_error(&format!("Method {} called on incompatible receiver", what))
}

fn dtf_of(realm: &mut Realm, this: &Value, what: &str) -> JsResult<Dtf> {
    if let V::Object(id) = this.v() {
        if let Some(d) = realm.host_data::<Dtf>(id) {
            return Ok(d.clone());
        }
    }
    realm.type_error(&format!("Method {} called on incompatible receiver", what))
}

/// ToNumber of a format argument, then TimeClip; NaN is a RangeError.
fn clipped_time(realm: &mut Realm, v: Value) -> JsResult<f64> {
    let x = realm.to_number(v)?;
    clip_or_throw(realm, x)
}

fn clip_or_throw(realm: &mut Realm, x: f64) -> JsResult<f64> {
    let t = dt::time_clip(x);
    if t.is_nan() {
        return realm.range_error("Invalid time value");
    }
    Ok(t)
}

fn format_getter(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = unwrap(realm, this, "get Intl.DateTimeFormat.prototype.format")?;
    let (_, bound) = symbols(realm);
    let key = PropertyKey::Symbol(bound);
    if let Some(p) = realm.obj(id).props.get(&key) {
        return Ok(p.value.clone());
    }
    let f = realm.new_native_function_full("", 1, format_bound, None, vec![Value::Object(id)], false);
    realm.define_property(id, key, Value::Object(f), 0);
    Ok(Value::Object(f))
}

fn format_bound(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let this = realm.current_native().map(|n| realm.native_captures(n)).and_then(|c| c.into_iter().next()).unwrap_or(Value::Undefined);
    let d = dtf_of(realm, &this, "Intl.DateTimeFormat.prototype.format")?;
    let date = arg(args, 0);
    let t = if date.is_undefined() { clip_or_throw(realm, dt::now_ms())? } else { clipped_time(realm, date)? };
    Ok(Value::str(&format_string(&d, t)))
}

fn parts_array(realm: &mut Realm, parts: Vec<(&'static str, String, Option<&'static str>)>) -> Value {
    let mut out = Vec::with_capacity(parts.len());
    for (t, v, source) in parts {
        let obj = realm.new_object();
        realm.define_property(obj, "type", Value::str(t), ALL);
        realm.define_property(obj, "value", Value::str(&v), ALL);
        if let Some(s) = source {
            realm.define_property(obj, "source", Value::str(s), ALL);
        }
        out.push(Value::Object(obj));
    }
    realm.new_array_value(out)
}

fn format_to_parts(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let d = dtf_of(realm, &this, "Intl.DateTimeFormat.prototype.formatToParts")?;
    let date = arg(args, 0);
    let t = if date.is_undefined() { clip_or_throw(realm, dt::now_ms())? } else { clipped_time(realm, date)? };
    let parts = format_parts(&d, t).into_iter().map(|(t, v)| (t, v, None)).collect();
    Ok(parts_array(realm, parts))
}

fn range_args(realm: &mut Realm, this: &Value, args: &[Value], what: &str) -> JsResult<(Dtf, f64, f64)> {
    let d = dtf_of(realm, this, what)?;
    let (s, e) = (arg(args, 0), arg(args, 1));
    if s.is_undefined() || e.is_undefined() {
        return realm.type_error("startDate and endDate are required");
    }
    let x = realm.to_number(s)?;
    let y = realm.to_number(e)?;
    let x = clip_or_throw(realm, x)?;
    let y = clip_or_throw(realm, y)?;
    Ok((d, x, y))
}

fn format_range(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (d, x, y) = range_args(realm, &this, args, "Intl.DateTimeFormat.prototype.formatRange")?;
    let s: String = range_parts(&d, x, y).into_iter().map(|(_, v, _)| v).collect();
    Ok(Value::str(&s))
}

fn format_range_to_parts(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (d, x, y) = range_args(realm, &this, args, "Intl.DateTimeFormat.prototype.formatRangeToParts")?;
    let parts = range_parts(&d, x, y).into_iter().map(|(t, v, s)| (t, v, Some(s))).collect();
    Ok(parts_array(realm, parts))
}

fn resolved_options(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = unwrap(realm, this, "Intl.DateTimeFormat.prototype.resolvedOptions")?;
    let d = realm.host_data::<Dtf>(id).cloned().unwrap();
    let obj = realm.new_object();
    realm.define_property(obj, "locale", Value::str(&d.locale), ALL);
    realm.define_property(obj, "calendar", Value::str(calendar_name(d.cal)), ALL);
    realm.define_property(obj, "numberingSystem", Value::str(&d.nu), ALL);
    realm.define_property(obj, "timeZone", Value::str(&d.tz_id), ALL);
    if let Some(hc) = d.hc {
        realm.define_property(obj, "hourCycle", Value::str(hc.name()), ALL);
        realm.define_property(obj, "hour12", Value::Bool(hc.is12()), ALL);
    }
    if d.date_style.is_none() && d.time_style.is_none() {
        for (i, (name, _)) in COMPONENTS.iter().enumerate() {
            if i == C_TZ_NAME {
                if let Some(n) = d.comps.fsd {
                    realm.define_property(obj, "fractionalSecondDigits", Value::Number(n as f64), ALL);
                }
            }
            if let Some(v) = d.comps.v[i] {
                realm.define_property(obj, *name, Value::str(v), ALL);
            }
        }
    }
    if let Some(s) = d.date_style {
        realm.define_property(obj, "dateStyle", Value::str(style_name(s)), ALL);
    }
    if let Some(s) = d.time_style {
        realm.define_property(obj, "timeStyle", Value::str(style_name(s)), ALL);
    }
    Ok(Value::Object(obj))
}

/// `Date.prototype.toLocale{,Date,Time}String` (ECMA-402 §20.4).
pub fn date_to_locale(realm: &mut Realm, t: f64, args: &[Value], defaults: DateDefaults) -> JsResult<Value> {
    if t.is_nan() {
        return Ok(Value::str("Invalid Date"));
    }
    let (required, defaults) = match defaults {
        DateDefaults::DateTime => (Required::Any, Defaults::All),
        DateDefaults::Date => (Required::Date, Defaults::Date),
        DateDefaults::Time => (Required::Time, Defaults::Time),
    };
    let d = create(realm, arg(args, 0), arg(args, 1), required, defaults)?;
    Ok(Value::str(&format_string(&d, t)))
}
