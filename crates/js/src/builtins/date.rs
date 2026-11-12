//! `Date`: ECMAScript time values, local time zone support (TZif + POSIX
//! rules on Unix, `GetTimeZoneInformation` on Windows), parsing and
//! formatting.

use super::*;
use crate::value::V;
use crate::realm::SYM_TO_PRIMITIVE;
use std::sync::OnceLock;

pub const MS_PER_SECOND: f64 = 1000.0;
pub const MS_PER_MINUTE: f64 = 60_000.0;
pub const MS_PER_HOUR: f64 = 3_600_000.0;
pub const MS_PER_DAY: f64 = 86_400_000.0;
const MAX_TIME: f64 = 8.64e15;

pub const MONTH_NAMES: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
pub const MONTH_LONG: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
pub const DAY_NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
pub const DAY_LONG: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

// ------------------------------------------------------------- spec math

/// Day number of a time value (ECMA-262 §21.4.1.2).
pub fn day(t: f64) -> f64 {
    (t / MS_PER_DAY).floor()
}

pub fn time_within_day(t: f64) -> f64 {
    t.rem_euclid(MS_PER_DAY)
}

pub fn days_in_year(y: f64) -> f64 {
    if y % 4.0 != 0.0 {
        365.0
    } else if y % 100.0 != 0.0 {
        366.0
    } else if y % 400.0 != 0.0 {
        365.0
    } else {
        366.0
    }
}

pub fn day_from_year(y: f64) -> f64 {
    365.0 * (y - 1970.0) + ((y - 1969.0) / 4.0).floor() - ((y - 1901.0) / 100.0).floor() + ((y - 1601.0) / 400.0).floor()
}

pub fn time_from_year(y: f64) -> f64 {
    MS_PER_DAY * day_from_year(y)
}

pub fn year_from_time(t: f64) -> f64 {
    let mut y = (t / (MS_PER_DAY * 365.2425)).floor() + 1970.0;
    while time_from_year(y) > t {
        y -= 1.0;
    }
    while time_from_year(y + 1.0) <= t {
        y += 1.0;
    }
    y
}

pub fn in_leap_year(t: f64) -> bool {
    days_in_year(year_from_time(t)) == 366.0
}

/// Cumulative day counts at the start of each month (non-leap year).
const MONTH_START: [f64; 13] = [0.0, 31.0, 59.0, 90.0, 120.0, 151.0, 181.0, 212.0, 243.0, 273.0, 304.0, 334.0, 365.0];

fn month_start(m: usize, leap: bool) -> f64 {
    MONTH_START[m] + if leap && m >= 2 { 1.0 } else { 0.0 }
}

pub fn month_from_time(t: f64) -> f64 {
    let leap = in_leap_year(t);
    let d = day(t) - day_from_year(year_from_time(t));
    for m in 0..12 {
        if d < month_start(m + 1, leap) {
            return m as f64;
        }
    }
    11.0
}

pub fn date_from_time(t: f64) -> f64 {
    let leap = in_leap_year(t);
    let d = day(t) - day_from_year(year_from_time(t));
    let m = month_from_time(t) as usize;
    d - month_start(m, leap) + 1.0
}

pub fn week_day(t: f64) -> f64 {
    (day(t) + 4.0).rem_euclid(7.0)
}

pub fn hour_from_time(t: f64) -> f64 {
    (t / MS_PER_HOUR).floor().rem_euclid(24.0)
}
pub fn min_from_time(t: f64) -> f64 {
    (t / MS_PER_MINUTE).floor().rem_euclid(60.0)
}
pub fn sec_from_time(t: f64) -> f64 {
    (t / MS_PER_SECOND).floor().rem_euclid(60.0)
}
pub fn ms_from_time(t: f64) -> f64 {
    t.rem_euclid(MS_PER_SECOND)
}

pub fn make_time(hour: f64, min: f64, sec: f64, ms: f64) -> f64 {
    if !hour.is_finite() || !min.is_finite() || !sec.is_finite() || !ms.is_finite() {
        return f64::NAN;
    }
    to_integer(hour) * MS_PER_HOUR + to_integer(min) * MS_PER_MINUTE + to_integer(sec) * MS_PER_SECOND + to_integer(ms)
}

pub fn make_day(year: f64, month: f64, date: f64) -> f64 {
    if !year.is_finite() || !month.is_finite() || !date.is_finite() {
        return f64::NAN;
    }
    let (y, m, dt) = (to_integer(year), to_integer(month), to_integer(date));
    let ym = y + (m / 12.0).floor();
    if ym.abs() > 400_000.0 {
        return f64::NAN;
    }
    let mn = m.rem_euclid(12.0) as usize;
    let leap = days_in_year(ym) == 366.0;
    let day_of_year_start = day_from_year(ym) + month_start(mn, leap);
    day_of_year_start + dt - 1.0
}

pub fn make_date(day: f64, time: f64) -> f64 {
    if !day.is_finite() || !time.is_finite() {
        return f64::NAN;
    }
    let tv = day * MS_PER_DAY + time;
    if !tv.is_finite() {
        return f64::NAN;
    }
    tv
}

pub fn time_clip(t: f64) -> f64 {
    if !t.is_finite() || t.abs() > MAX_TIME {
        return f64::NAN;
    }
    to_integer(t) + 0.0
}

/// Broken-down fields of a time value (UTC or already-local).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fields {
    pub year: f64,
    pub month: f64,
    pub day: f64,
    pub hour: f64,
    pub minute: f64,
    pub second: f64,
    pub ms: f64,
    pub weekday: f64,
}

pub fn fields(t: f64) -> Fields {
    Fields {
        year: year_from_time(t),
        month: month_from_time(t),
        day: date_from_time(t),
        hour: hour_from_time(t),
        minute: min_from_time(t),
        second: sec_from_time(t),
        ms: ms_from_time(t),
        weekday: week_day(t),
    }
}

/// Fields of `t` in local time.
pub fn local_fields(t: f64) -> Fields {
    fields(local_time(t))
}

// ----------------------------------------------------------- time zones

/// Transition-based time zone description.
#[derive(Debug, Clone, Default)]
pub struct TzInfo {
    /// (transition instant in seconds since the epoch, offset from UTC in
    /// seconds, abbreviation, is DST)
    pub transitions: Vec<(i64, i32, String, bool)>,
    /// Offset used before the first transition.
    pub initial: (i32, String, bool),
    /// POSIX TZ rule for instants after the last transition.
    pub footer: Option<PosixTz>,
    /// IANA name (e.g. `Europe/Madrid`) when known.
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct PosixTz {
    pub std_name: String,
    pub std_off: i32,
    pub dst: Option<PosixDst>,
}

#[derive(Debug, Clone)]
pub struct PosixDst {
    pub name: String,
    pub off: i32,
    pub start: PosixRule,
    pub end: PosixRule,
}

#[derive(Debug, Clone)]
pub enum PosixRule {
    /// `Jn`: 1-based day of the year, February 29 never counted.
    Julian1(u32, i32),
    /// `n`: 0-based day of the year, leap days counted.
    Julian0(u32, i32),
    /// `Mm.w.d`: month, week (5 = last), weekday (0 = Sunday), seconds after local midnight.
    Month(u32, u32, u32, i32),
}

fn read_i32(b: &[u8], off: usize) -> Option<i32> {
    Some(i32::from_be_bytes(b.get(off..off + 4)?.try_into().ok()?))
}
fn read_i64(b: &[u8], off: usize) -> Option<i64> {
    Some(i64::from_be_bytes(b.get(off..off + 8)?.try_into().ok()?))
}

/// Parse a TZif file (RFC 8536), preferring the 64-bit v2+ block.
pub fn parse_tzif(data: &[u8]) -> Option<TzInfo> {
    if data.len() < 44 || &data[0..4] != b"TZif" {
        return None;
    }
    let version = data[4];
    let counts = |hdr: usize| -> Option<[usize; 6]> {
        let mut c = [0usize; 6];
        for (i, slot) in c.iter_mut().enumerate() {
            *slot = read_i32(data, hdr + 20 + i * 4)? as usize;
        }
        Some(c)
    };
    // [isutcnt, isstdcnt, leapcnt, timecnt, typecnt, charcnt]
    let c1 = counts(0)?;
    let v1_len = c1[3] * 4 + c1[3] + c1[4] * 6 + c1[5] + c1[2] * 8 + c1[1] + c1[0];
    let (hdr, c, time_size) = if version >= b'2' {
        let hdr2 = 44 + v1_len;
        if data.len() < hdr2 + 44 || &data[hdr2..hdr2 + 4] != b"TZif" {
            return None;
        }
        (hdr2, counts(hdr2)?, 8)
    } else {
        (0, c1, 4)
    };
    let [_isutcnt, _isstdcnt, leapcnt, timecnt, typecnt, charcnt] = c;
    let mut p = hdr + 44;
    let mut times = Vec::with_capacity(timecnt);
    for i in 0..timecnt {
        let t = if time_size == 8 { read_i64(data, p + i * 8)? } else { read_i32(data, p + i * 4)? as i64 };
        times.push(t);
    }
    p += timecnt * time_size;
    let idx = data.get(p..p + timecnt)?.to_vec();
    p += timecnt;
    let mut types = Vec::with_capacity(typecnt);
    for i in 0..typecnt {
        let base = p + i * 6;
        let off = read_i32(data, base)?;
        let isdst = *data.get(base + 4)? != 0;
        let abbr = *data.get(base + 5)? as usize;
        types.push((off, isdst, abbr));
    }
    p += typecnt * 6;
    let chars = data.get(p..p + charcnt)?;
    let abbr_at = |i: usize| -> String {
        let s = &chars[i.min(chars.len())..];
        let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
        String::from_utf8_lossy(&s[..end]).into_owned()
    };
    p += charcnt + leapcnt * (time_size + 4) + c[1] + c[0];
    let mut info = TzInfo::default();
    if let Some(first) = types.first() {
        info.initial = (first.0, abbr_at(first.2), first.1);
    }
    // The initial type is the first non-DST type when available.
    if let Some(t) = types.iter().find(|t| !t.1) {
        info.initial = (t.0, abbr_at(t.2), t.1);
    }
    for (i, &t) in times.iter().enumerate() {
        let ty = types.get(idx[i] as usize)?;
        info.transitions.push((t, ty.0, abbr_at(ty.2), ty.1));
    }
    if version >= b'2' {
        if let Some(rest) = data.get(p..) {
            let text = String::from_utf8_lossy(rest);
            let footer = text.trim_matches(|c| c == '\n' || c == '\r');
            if !footer.is_empty() {
                info.footer = parse_posix_tz(footer);
            }
        }
    }
    Some(info)
}

/// Parse a POSIX `TZ` string such as `CET-1CEST,M3.5.0,M10.5.0/3`.
pub fn parse_posix_tz(s: &str) -> Option<PosixTz> {
    let b = s.as_bytes();
    let mut i = 0;
    fn name(b: &[u8], i: &mut usize) -> Option<String> {
        if *i < b.len() && b[*i] == b'<' {
            let start = *i + 1;
            while *i < b.len() && b[*i] != b'>' {
                *i += 1;
            }
            let n = String::from_utf8_lossy(&b[start..*i]).into_owned();
            *i += 1;
            return Some(n);
        }
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_alphabetic() {
            *i += 1;
        }
        if *i - start < 3 {
            return None;
        }
        Some(String::from_utf8_lossy(&b[start..*i]).into_owned())
    }
    fn offset(b: &[u8], i: &mut usize) -> Option<i32> {
        let mut sign = 1;
        if *i < b.len() && (b[*i] == b'+' || b[*i] == b'-') {
            if b[*i] == b'-' {
                sign = -1;
            }
            *i += 1;
        }
        let mut parts = [0i32; 3];
        for (k, part) in parts.iter_mut().enumerate() {
            let start = *i;
            while *i < b.len() && b[*i].is_ascii_digit() {
                *i += 1;
            }
            if start == *i {
                if k == 0 {
                    return None;
                }
                break;
            }
            *part = std::str::from_utf8(&b[start..*i]).ok()?.parse().ok()?;
            if *i < b.len() && b[*i] == b':' {
                *i += 1;
            } else {
                break;
            }
        }
        Some(sign * (parts[0] * 3600 + parts[1] * 60 + parts[2]))
    }
    fn rule(b: &[u8], i: &mut usize) -> Option<PosixRule> {
        let kind = *b.get(*i)?;
        let mut time = 7200;
        let r = if kind == b'M' {
            *i += 1;
            let mut nums = [0u32; 3];
            for (k, n) in nums.iter_mut().enumerate() {
                let start = *i;
                while *i < b.len() && b[*i].is_ascii_digit() {
                    *i += 1;
                }
                *n = std::str::from_utf8(&b[start..*i]).ok()?.parse().ok()?;
                if k < 2 {
                    if b.get(*i) != Some(&b'.') {
                        return None;
                    }
                    *i += 1;
                }
            }
            PosixRule::Month(nums[0], nums[1], nums[2], 0)
        } else {
            let julian1 = kind == b'J';
            if julian1 {
                *i += 1;
            }
            let start = *i;
            while *i < b.len() && b[*i].is_ascii_digit() {
                *i += 1;
            }
            let n: u32 = std::str::from_utf8(&b[start..*i]).ok()?.parse().ok()?;
            if julian1 {
                PosixRule::Julian1(n, 0)
            } else {
                PosixRule::Julian0(n, 0)
            }
        };
        if b.get(*i) == Some(&b'/') {
            *i += 1;
            time = offset(b, i)?;
        }
        Some(match r {
            PosixRule::Month(m, w, d, _) => PosixRule::Month(m, w, d, time),
            PosixRule::Julian1(n, _) => PosixRule::Julian1(n, time),
            PosixRule::Julian0(n, _) => PosixRule::Julian0(n, time),
        })
    }
    let std_name = name(b, &mut i)?;
    // POSIX offsets are positive west of Greenwich; store UTC offsets.
    let std_off = -offset(b, &mut i)?;
    if i >= b.len() {
        return Some(PosixTz { std_name, std_off, dst: None });
    }
    let dst_name = name(b, &mut i)?;
    let mut dst_off = std_off + 3600;
    if i < b.len() && b[i] != b',' {
        dst_off = -offset(b, &mut i)?;
    }
    let (start, end) = if i < b.len() && b[i] == b',' {
        i += 1;
        let s = rule(b, &mut i)?;
        if b.get(i) != Some(&b',') {
            return None;
        }
        i += 1;
        let e = rule(b, &mut i)?;
        (s, e)
    } else {
        // Default US rules.
        (PosixRule::Month(3, 2, 0, 7200), PosixRule::Month(11, 1, 0, 7200))
    };
    Some(PosixTz { std_name, std_off, dst: Some(PosixDst { name: dst_name, off: dst_off, start, end }) })
}

impl PosixRule {
    /// Seconds since the epoch of this rule's transition in `year`, given
    /// the local offset in effect before the transition.
    fn instant(&self, year: i64, local_off: i32) -> i64 {
        let leap = days_in_year(year as f64) == 366.0;
        let (doy, time) = match *self {
            PosixRule::Julian1(n, t) => {
                let mut d = n as i64 - 1;
                if leap && n >= 60 {
                    d += 1;
                }
                (d, t)
            }
            PosixRule::Julian0(n, t) => (n as i64, t),
            PosixRule::Month(m, w, d, t) => {
                let m = m.clamp(1, 12) as usize;
                let first_doy = month_start(m - 1, leap) as i64;
                let first_wd = ((day_from_year(year as f64) as i64 + first_doy + 4).rem_euclid(7)) as u32;
                let mut dom = (d + 7 - first_wd) % 7; // 0-based day of month of the first `d`
                let days_in_month = (month_start(m, leap) - month_start(m - 1, leap)) as u32;
                let w = w.clamp(1, 5);
                dom += (w - 1) * 7;
                while dom >= days_in_month {
                    dom -= 7;
                }
                (first_doy + dom as i64, t)
            }
        };
        let day_num = day_from_year(year as f64) as i64 + doy;
        day_num * 86400 + time as i64 - local_off as i64
    }
}

impl PosixTz {
    /// (offset seconds, abbreviation, is DST) at a UTC instant.
    pub fn offset_at(&self, secs: i64) -> (i32, &str, bool) {
        let Some(dst) = &self.dst else {
            return (self.std_off, &self.std_name, false);
        };
        // Year in standard local time.
        let year = year_from_time((secs + self.std_off as i64) as f64 * 1000.0) as i64;
        let start = dst.start.instant(year, self.std_off);
        let end = dst.end.instant(year, dst.off);
        let in_dst = if start < end { secs >= start && secs < end } else { !(secs >= end && secs < start) };
        if in_dst {
            (dst.off, &dst.name, true)
        } else {
            (self.std_off, &self.std_name, false)
        }
    }
}

impl TzInfo {
    /// Offset (seconds east of UTC) and abbreviation at a UTC instant.
    pub fn offset_at(&self, secs: i64) -> (i32, String, bool) {
        if self.transitions.is_empty() {
            if let Some(f) = &self.footer {
                let (o, n, d) = f.offset_at(secs);
                return (o, n.to_string(), d);
            }
            return (self.initial.0, self.initial.1.clone(), self.initial.2);
        }
        if secs < self.transitions[0].0 {
            return (self.initial.0, self.initial.1.clone(), self.initial.2);
        }
        let last = self.transitions.last().unwrap();
        if secs >= last.0 {
            if let Some(f) = &self.footer {
                let (o, n, d) = f.offset_at(secs);
                return (o, n.to_string(), d);
            }
            return (last.1, last.2.clone(), last.3);
        }
        let i = self.transitions.partition_point(|t| t.0 <= secs);
        let t = &self.transitions[i - 1];
        (t.1, t.2.clone(), t.3)
    }
}

fn load_tz() -> TzInfo {
    #[cfg(unix)]
    {
        let mut candidates: Vec<(std::path::PathBuf, String)> = Vec::new();
        if let Ok(tz) = std::env::var("TZ") {
            let tz = tz.trim_start_matches(':').to_string();
            if tz.eq_ignore_ascii_case("UTC") || tz.is_empty() {
                return utc_tz();
            }
            if tz.starts_with('/') {
                candidates.push((std::path::PathBuf::from(&tz), tz.clone()));
            } else {
                candidates.push((std::path::PathBuf::from("/usr/share/zoneinfo").join(&tz), tz.clone()));
            }
        }
        let name_from_link = std::fs::read_link("/etc/localtime")
            .ok()
            .and_then(|p| {
                let s = p.to_string_lossy().into_owned();
                s.split_once("zoneinfo/").map(|(_, n)| n.to_string())
            })
            .unwrap_or_default();
        candidates.push((std::path::PathBuf::from("/etc/localtime"), name_from_link));
        for (path, name) in candidates {
            if let Ok(data) = std::fs::read(&path) {
                if let Some(mut info) = parse_tzif(&data) {
                    info.name = if name.is_empty() { "Local".into() } else { name };
                    return info;
                }
            }
        }
        if let Ok(tz) = std::env::var("TZ") {
            // A bare POSIX rule string.
            if let Some(p) = parse_posix_tz(&tz) {
                return TzInfo { transitions: vec![], initial: (p.std_off, p.std_name.clone(), false), footer: Some(p), name: tz };
            }
        }
        utc_tz()
    }
    #[cfg(windows)]
    {
        win::load().unwrap_or_else(utc_tz)
    }
    #[cfg(not(any(unix, windows)))]
    {
        utc_tz()
    }
}

fn utc_tz() -> TzInfo {
    TzInfo { transitions: vec![], initial: (0, "UTC".into(), false), footer: None, name: "UTC".into() }
}

#[cfg(windows)]
mod win {
    use super::TzInfo;

    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    #[repr(C)]
    struct TimeZoneInformation {
        bias: i32,
        standard_name: [u16; 32],
        standard_date: SystemTime,
        standard_bias: i32,
        daylight_name: [u16; 32],
        daylight_date: SystemTime,
        daylight_bias: i32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetTimeZoneInformation(info: *mut TimeZoneInformation) -> u32;
    }

    pub fn load() -> Option<TzInfo> {
        let mut info = TimeZoneInformation {
            bias: 0,
            standard_name: [0; 32],
            standard_date: SystemTime::default(),
            standard_bias: 0,
            daylight_name: [0; 32],
            daylight_date: SystemTime::default(),
            daylight_bias: 0,
        };
        // SAFETY: the struct layout matches TIME_ZONE_INFORMATION and the
        // pointer is valid for the call's duration.
        let id = unsafe { GetTimeZoneInformation(&mut info) };
        if id == u32::MAX {
            return None;
        }
        let (bias, name, dst) = if id == 2 {
            (info.bias + info.daylight_bias, &info.daylight_name, true)
        } else {
            (info.bias + info.standard_bias, &info.standard_name, false)
        };
        let end = name.iter().position(|&c| c == 0).unwrap_or(32);
        let name = String::from_utf16_lossy(&name[..end]);
        let off = -bias * 60;
        Some(TzInfo { transitions: vec![], initial: (off, name.clone(), dst), footer: None, name })
    }
}

static TZ: OnceLock<TzInfo> = OnceLock::new();

pub fn tz() -> &'static TzInfo {
    TZ.get_or_init(load_tz)
}

static NAMED_TZ: OnceLock<std::sync::Mutex<std::collections::HashMap<String, Option<&'static TzInfo>>>> = OnceLock::new();

/// Load an IANA zone (`Europe/Madrid`) from the system zoneinfo database;
/// None when the name is unknown or no database exists.
pub fn tz_named(name: &str) -> Option<&'static TzInfo> {
    if name.is_empty() || name.len() > 64 || name.contains("..") || !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+')) {
        return None;
    }
    if name.eq_ignore_ascii_case(&tz().name) {
        return Some(tz());
    }
    let cache = NAMED_TZ.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(v) = guard.get(name) {
        return *v;
    }
    let mut found: Option<&'static TzInfo> = None;
    for dir in ["/usr/share/zoneinfo", "/usr/lib/zoneinfo", "/usr/share/lib/zoneinfo", "/etc/zoneinfo"] {
        let path = std::path::Path::new(dir).join(name);
        if let Ok(data) = std::fs::read(&path) {
            if let Some(mut info) = parse_tzif(&data) {
                // Canonical capitalisation from the file name on disk.
                info.name = name.to_string();
                found = Some(Box::leak(Box::new(info)));
                break;
            }
        }
    }
    guard.insert(name.to_string(), found);
    found
}

/// Offset (ms) and abbreviation of `zone` at a UTC instant.
pub fn zone_offset(zone: &TzInfo, t: f64) -> (f64, String) {
    if !t.is_finite() {
        return (0.0, "UTC".into());
    }
    let (off, name, _) = zone.offset_at((t / 1000.0).floor() as i64);
    (off as f64 * 1000.0, name)
}

/// Local time zone offset at a UTC instant, in ms, plus abbreviation.
pub fn local_tza(t: f64) -> (f64, String) {
    if !t.is_finite() {
        return (0.0, "UTC".into());
    }
    let (off, name, _) = tz().offset_at((t / 1000.0).floor() as i64);
    (off as f64 * 1000.0, name)
}

pub fn local_time(t: f64) -> f64 {
    t + local_tza(t).0
}

/// Convert a local time value to UTC (ECMA-262 `UTC(t)`).
pub fn utc_from_local(t: f64) -> f64 {
    if !t.is_finite() {
        return f64::NAN;
    }
    let guess = local_tza(t).0;
    let u = t - guess;
    let off2 = local_tza(u).0;
    if off2 != guess {
        t - off2
    } else {
        u
    }
}

/// Long time zone name for `toString`, derived from the abbreviation.
pub fn long_zone_name(abbr: &str) -> String {
    match abbr {
        "CET" => "Central European Standard Time".into(),
        "CEST" => "Central European Summer Time".into(),
        "WET" => "Western European Standard Time".into(),
        "WEST" => "Western European Summer Time".into(),
        "EET" => "Eastern European Standard Time".into(),
        "EEST" => "Eastern European Summer Time".into(),
        "GMT" => "Greenwich Mean Time".into(),
        "BST" => "British Summer Time".into(),
        "IST" => "India Standard Time".into(),
        "UTC" | "UCT" | "Z" => "Coordinated Universal Time".into(),
        "EST" => "Eastern Standard Time".into(),
        "EDT" => "Eastern Daylight Time".into(),
        "CST" => "Central Standard Time".into(),
        "CDT" => "Central Daylight Time".into(),
        "MST" => "Mountain Standard Time".into(),
        "MDT" => "Mountain Daylight Time".into(),
        "PST" => "Pacific Standard Time".into(),
        "PDT" => "Pacific Daylight Time".into(),
        "AKST" => "Alaska Standard Time".into(),
        "AKDT" => "Alaska Daylight Time".into(),
        "HST" => "Hawaii-Aleutian Standard Time".into(),
        "JST" => "Japan Standard Time".into(),
        "KST" => "Korean Standard Time".into(),
        "AEST" => "Australian Eastern Standard Time".into(),
        "AEDT" => "Australian Eastern Daylight Time".into(),
        "MSK" => "Moscow Standard Time".into(),
        other => other.to_string(),
    }
}

// ------------------------------------------------------------ formatting

fn pad2(n: f64) -> String {
    format!("{:02}", n as i64)
}

fn year_string(y: f64) -> String {
    if y >= 0.0 {
        format!("{:04}", y as i64)
    } else {
        format!("-{:04}", -(y as i64))
    }
}

fn offset_string(off_ms: f64) -> String {
    let mins = (off_ms / MS_PER_MINUTE).round() as i64;
    let sign = if mins < 0 { '-' } else { '+' };
    let mins = mins.abs();
    format!("GMT{}{:02}{:02}", sign, mins / 60, mins % 60)
}

pub fn to_date_string(t: f64) -> String {
    if t.is_nan() {
        return "Invalid Date".into();
    }
    let f = local_fields(t);
    format!("{} {} {} {}", DAY_NAMES[f.weekday as usize], MONTH_NAMES[f.month as usize], pad2(f.day), year_string(f.year))
}

pub fn to_time_string(t: f64) -> String {
    if t.is_nan() {
        return "Invalid Date".into();
    }
    let f = local_fields(t);
    let (off, abbr) = local_tza(t);
    format!("{}:{}:{} {} ({})", pad2(f.hour), pad2(f.minute), pad2(f.second), offset_string(off), long_zone_name(&abbr))
}

pub fn to_string(t: f64) -> String {
    if t.is_nan() {
        return "Invalid Date".into();
    }
    format!("{} {}", to_date_string(t), to_time_string(t))
}

pub fn to_utc_string(t: f64) -> String {
    if t.is_nan() {
        return "Invalid Date".into();
    }
    let f = fields(t);
    format!(
        "{}, {} {} {} {}:{}:{} GMT",
        DAY_NAMES[f.weekday as usize],
        pad2(f.day),
        MONTH_NAMES[f.month as usize],
        year_string(f.year),
        pad2(f.hour),
        pad2(f.minute),
        pad2(f.second)
    )
}

pub fn to_iso_string(t: f64) -> String {
    let f = fields(t);
    let y = f.year as i64;
    let year = if (0..=9999).contains(&y) { format!("{:04}", y) } else if y < 0 { format!("-{:06}", -y) } else { format!("+{:06}", y) };
    format!("{}-{}-{}T{}:{}:{}.{:03}Z", year, pad2(f.month + 1.0), pad2(f.day), pad2(f.hour), pad2(f.minute), pad2(f.second), f.ms as i64)
}

// --------------------------------------------------------------- parsing

/// `Date.parse` semantics: ISO 8601 first, then a tolerant free-form
/// parser. Returns NaN when the string cannot be understood.
pub fn parse_date(s: &str) -> f64 {
    let s = s.trim();
    if s.is_empty() {
        return f64::NAN;
    }
    if let Some(t) = parse_iso(s) {
        return t;
    }
    if s.starts_with("-000000") {
        // Minus zero is not a valid extended year (and not a loose date).
        return f64::NAN;
    }
    parse_loose(s).unwrap_or(f64::NAN)
}

fn parse_iso(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let mut i = 0;
    let digits = |i: &mut usize, n: usize| -> Option<i64> {
        if *i + n > b.len() || !b[*i..*i + n].iter().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let v = std::str::from_utf8(&b[*i..*i + n]).ok()?.parse().ok()?;
        *i += n;
        Some(v)
    };
    let year = match b.first()? {
        b'+' | b'-' => {
            let neg = b[0] == b'-';
            i += 1;
            let y = digits(&mut i, 6)?;
            if neg && y == 0 {
                return None;
            }
            if neg {
                -y
            } else {
                y
            }
        }
        _ => digits(&mut i, 4)?,
    };
    let mut month = 1;
    let mut dayv = 1;
    let (mut h, mut m, mut sec, mut ms) = (0i64, 0i64, 0i64, 0i64);
    let mut offset: Option<i64> = None;
    let mut has_time = false;
    if i < b.len() && b[i] == b'-' {
        i += 1;
        month = digits(&mut i, 2)?;
        if i < b.len() && b[i] == b'-' {
            i += 1;
            dayv = digits(&mut i, 2)?;
        }
    }
    if i < b.len() && (b[i] == b'T' || b[i] == b't' || b[i] == b' ') {
        i += 1;
        has_time = true;
        h = digits(&mut i, 2)?;
        if b.get(i) != Some(&b':') {
            return None;
        }
        i += 1;
        m = digits(&mut i, 2)?;
        if i < b.len() && b[i] == b':' {
            i += 1;
            sec = digits(&mut i, 2)?;
            if i < b.len() && (b[i] == b'.' || b[i] == b',') {
                i += 1;
                let start = i;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                if i == start {
                    return None;
                }
                let frac = &s[start..i];
                let scaled: String = frac.chars().chain("000".chars()).take(3).collect();
                ms = scaled.parse().ok()?;
            }
        }
        if i < b.len() {
            match b[i] {
                b'Z' | b'z' => {
                    offset = Some(0);
                    i += 1;
                }
                b'+' | b'-' => {
                    let sign = if b[i] == b'-' { -1 } else { 1 };
                    i += 1;
                    let oh = digits(&mut i, 2)?;
                    let mut om = 0;
                    if i < b.len() && b[i] == b':' {
                        i += 1;
                        om = digits(&mut i, 2)?;
                    } else if i + 2 <= b.len() {
                        om = digits(&mut i, 2)?;
                    }
                    offset = Some(sign * (oh * 60 + om));
                }
                _ => {}
            }
        }
    }
    if i != b.len() {
        return None;
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&dayv) || h > 24 || m > 59 || sec > 59 || (h == 24 && (m != 0 || sec != 0 || ms != 0)) {
        return None;
    }
    let dayn = make_day(year as f64, (month - 1) as f64, dayv as f64);
    let time = make_time(h as f64, m as f64, sec as f64, ms as f64);
    let t = make_date(dayn, time);
    let t = match offset {
        Some(off) => t - off as f64 * MS_PER_MINUTE,
        None if has_time => utc_from_local(t),
        None => t,
    };
    Some(time_clip(t))
}

fn month_from_name(w: &str) -> Option<i64> {
    if w.len() < 3 {
        return None;
    }
    let p = &w[..3];
    MONTH_NAMES.iter().position(|m| m.eq_ignore_ascii_case(p)).map(|i| i as i64)
}

fn zone_from_abbr(w: &str) -> Option<i64> {
    Some(match w.to_ascii_uppercase().as_str() {
        "GMT" | "UTC" | "UT" | "Z" => 0,
        "EST" => -300,
        "EDT" => -240,
        "CST" => -360,
        "CDT" => -300,
        "MST" => -420,
        "MDT" => -360,
        "PST" => -480,
        "PDT" => -420,
        "CET" => 60,
        "CEST" => 120,
        "BST" => 60,
        "WET" => 0,
        "EET" => 120,
        "EEST" => 180,
        "JST" => 540,
        _ => return None,
    })
}

fn parse_loose(s: &str) -> Option<f64> {
    let mut year: Option<i64> = None;
    let mut month: Option<i64> = None;
    let mut dayv: Option<i64> = None;
    let mut time: Option<(i64, i64, i64, i64)> = None;
    let mut offset: Option<i64> = None;
    let mut pm: Option<bool> = None;
    let mut numbers: Vec<i64> = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    let mut depth = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'(' {
            depth += 1;
            i += 1;
            continue;
        }
        if c == b')' {
            depth = (depth - 1).max(0);
            i += 1;
            continue;
        }
        if depth > 0 {
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphabetic() || b[i] == b'.') {
                i += 1;
            }
            let w = s[start..i].trim_end_matches('.');
            let lw = w.to_ascii_lowercase();
            if lw == "am" || lw == "a.m" {
                pm = Some(false);
            } else if lw == "pm" || lw == "p.m" {
                pm = Some(true);
            } else if let Some(m) = month_from_name(w) {
                if DAY_NAMES.iter().any(|d| d.eq_ignore_ascii_case(&w[..3.min(w.len())])) && month.is_some() {
                    // weekday
                } else {
                    month = Some(m);
                }
            } else if DAY_NAMES.iter().any(|d| w.len() >= 3 && d.eq_ignore_ascii_case(&w[..3])) {
                // weekday name: ignore
            } else if let Some(z) = zone_from_abbr(w) {
                offset = Some(z);
                // GMT+0100 forms
                if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
                    let sign = if b[i] == b'-' { -1 } else { 1 };
                    i += 1;
                    let start = i;
                    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b':') {
                        i += 1;
                    }
                    let num: String = s[start..i].chars().filter(|c| c.is_ascii_digit()).collect();
                    let (oh, om) = match num.len() {
                        1 | 2 => (num.parse::<i64>().unwrap_or(0), 0),
                        3 => (num[..1].parse().unwrap_or(0), num[1..].parse().unwrap_or(0)),
                        4 => (num[..2].parse().unwrap_or(0), num[2..].parse().unwrap_or(0)),
                        _ => (0, 0),
                    };
                    offset = Some(z + sign * (oh * 60 + om));
                }
            } else if lw == "t" {
                // ISO-like separator
            } else {
                return None;
            }
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            let n: i64 = s[start..i].parse().ok()?;
            if i < b.len() && b[i] == b':' {
                // time
                let mut parts = vec![n];
                let mut msv = 0;
                while i < b.len() && b[i] == b':' && parts.len() < 3 {
                    i += 1;
                    let st = i;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                    parts.push(s[st..i].parse().ok()?);
                }
                if i < b.len() && b[i] == b'.' {
                    i += 1;
                    let st = i;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                    let frac: String = s[st..i].chars().chain("000".chars()).take(3).collect();
                    msv = frac.parse().unwrap_or(0);
                }
                time = Some((parts[0], parts[1], *parts.get(2).unwrap_or(&0), msv));
                continue;
            }
            if i < b.len() && (b[i] == b'/' || b[i] == b'-' || b[i] == b'.') && i + 1 < b.len() && b[i + 1].is_ascii_digit() {
                let sep = b[i];
                let mut parts = vec![n];
                while i < b.len() && b[i] == sep && parts.len() < 3 {
                    i += 1;
                    let st = i;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                    parts.push(s[st..i].parse().ok()?);
                }
                let digits_first = i - start;
                if parts[0] > 31 || (digits_first >= 4 && sep == b'-') || parts[0] >= 1000 {
                    year = Some(parts[0]);
                    month = Some(parts[1] - 1);
                    if parts.len() > 2 {
                        dayv = Some(parts[2]);
                    }
                } else if parts.len() == 3 {
                    if sep == b'.' {
                        // European d.m.y
                        dayv = Some(parts[0]);
                        month = Some(parts[1] - 1);
                        year = Some(parts[2]);
                    } else {
                        month = Some(parts[0] - 1);
                        dayv = Some(parts[1]);
                        year = Some(parts[2]);
                    }
                } else {
                    month = Some(parts[0] - 1);
                    dayv = Some(parts[1]);
                }
                continue;
            }
            numbers.push(n);
            continue;
        }
        if c == b'+' || c == b'-' {
            // numeric zone offset like +0100 / -05:00 (only after a time)
            if time.is_some() && i + 1 < b.len() && b[i + 1].is_ascii_digit() {
                let sign = if c == b'-' { -1 } else { 1 };
                i += 1;
                let start = i;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b':') {
                    i += 1;
                }
                let num: String = s[start..i].chars().filter(|c| c.is_ascii_digit()).collect();
                let (oh, om) = match num.len() {
                    1 | 2 => (num.parse::<i64>().unwrap_or(0), 0),
                    3 => (num[..1].parse().unwrap_or(0), num[1..].parse().unwrap_or(0)),
                    _ => (num[..2].parse().unwrap_or(0), num[2..4].parse().unwrap_or(0)),
                };
                offset = Some(sign * (oh * 60 + om));
                continue;
            }
            i += 1;
            continue;
        }
        // separators
        i += 1;
    }
    // Assign loose numbers: day then year.
    for n in numbers {
        if dayv.is_none() && (1..=31).contains(&n) && (month.is_some() || year.is_none()) && !(n >= 32) {
            if month.is_some() || year.is_some() {
                dayv = Some(n);
                continue;
            }
        }
        if year.is_none() {
            year = Some(n);
        } else if dayv.is_none() && (1..=31).contains(&n) {
            dayv = Some(n);
        } else {
            return None;
        }
    }
    let year = year?;
    let year = if (0..50).contains(&year) && s.len() < 32 && !s.contains(&format!("{:04}", year)) {
        year + 2000
    } else if (50..100).contains(&year) && !s.contains(&format!("{:04}", year)) {
        year + 1900
    } else {
        year
    };
    let month = month.unwrap_or(0);
    let dayv = dayv.unwrap_or(1);
    let (mut h, m, sec, ms) = time.unwrap_or((0, 0, 0, 0));
    if let Some(pm) = pm {
        if h > 12 {
            return None;
        }
        if pm && h < 12 {
            h += 12;
        } else if !pm && h == 12 {
            h = 0;
        }
    }
    if !(0..=11).contains(&month) || !(1..=31).contains(&dayv) || h > 24 || m > 59 || sec > 59 {
        return None;
    }
    let t = make_date(make_day(year as f64, month as f64, dayv as f64), make_time(h as f64, m as f64, sec as f64, ms as f64));
    let t = match offset {
        Some(off) => t - off as f64 * MS_PER_MINUTE,
        None => utc_from_local(t),
    };
    Some(time_clip(t))
}

/// Current time in ms since the epoch.
pub fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0).floor()
}

// ------------------------------------------------------------- builtins

pub fn register(realm: &mut Realm) {
    let op = realm.intrinsics.object_proto;
    let dp = realm.new_object_with_proto(Some(op));
    realm.intrinsics.others.insert("DatePrototype", dp);
    let ctor = define_ctor(realm, "Date", 7, date_ctor, dp);
    realm.define_method(ctor, "now", 0, date_now);
    realm.define_method(ctor, "parse", 1, date_parse);
    realm.define_method(ctor, "UTC", 7, date_utc);
    let methods: &[(&str, u32, NativeFn)] = &[
        ("getTime", 0, get_time),
        ("valueOf", 0, get_time),
        ("getTimezoneOffset", 0, get_timezone_offset),
        ("getFullYear", 0, get_full_year),
        ("getUTCFullYear", 0, get_utc_full_year),
        ("getYear", 0, get_year),
        ("getMonth", 0, get_month),
        ("getUTCMonth", 0, get_utc_month),
        ("getDate", 0, get_date),
        ("getUTCDate", 0, get_utc_date),
        ("getDay", 0, get_day),
        ("getUTCDay", 0, get_utc_day),
        ("getHours", 0, get_hours),
        ("getUTCHours", 0, get_utc_hours),
        ("getMinutes", 0, get_minutes),
        ("getUTCMinutes", 0, get_utc_minutes),
        ("getSeconds", 0, get_seconds),
        ("getUTCSeconds", 0, get_utc_seconds),
        ("getMilliseconds", 0, get_milliseconds),
        ("getUTCMilliseconds", 0, get_utc_milliseconds),
        ("setTime", 1, set_time),
        ("setMilliseconds", 1, set_milliseconds),
        ("setUTCMilliseconds", 1, set_utc_milliseconds),
        ("setSeconds", 2, set_seconds),
        ("setUTCSeconds", 2, set_utc_seconds),
        ("setMinutes", 3, set_minutes),
        ("setUTCMinutes", 3, set_utc_minutes),
        ("setHours", 4, set_hours),
        ("setUTCHours", 4, set_utc_hours),
        ("setDate", 1, set_date),
        ("setUTCDate", 1, set_utc_date),
        ("setMonth", 2, set_month),
        ("setUTCMonth", 2, set_utc_month),
        ("setFullYear", 3, set_full_year),
        ("setUTCFullYear", 3, set_utc_full_year),
        ("setYear", 1, set_year),
        ("toString", 0, to_string_m),
        ("toDateString", 0, to_date_string_m),
        ("toTimeString", 0, to_time_string_m),
        ("toISOString", 0, to_iso_string_m),
        ("toUTCString", 0, to_utc_string_m),
        ("toJSON", 1, to_json),
        ("toLocaleString", 0, to_locale_string),
        ("toLocaleDateString", 0, to_locale_date_string),
        ("toLocaleTimeString", 0, to_locale_time_string),
    ];
    for (name, arity, f) in methods {
        realm.define_method(dp, name, *arity, *f);
    }
    // Annex B: toGMTString is the same function object as toUTCString.
    if let Some(v) = realm.obj(dp).props.get(&PropertyKey::str("toUTCString")).map(|p| p.value.clone()) {
        realm.define_property(dp, "toGMTString", v, HIDDEN);
    }
    let tp = realm.new_native_function("[Symbol.toPrimitive]", 1, to_primitive);
    realm.define_property(dp, PropertyKey::Symbol(SYM_TO_PRIMITIVE), Value::Object(tp), CONFIGURABLE);
}

fn this_time(realm: &mut Realm, this: &Value, method: &str) -> JsResult<(ObjectId, f64)> {
    if let V::Object(id) = this.v() {
        if let ObjectKind::Date(t) = &realm.obj(id).kind {
            return Ok((id, *t));
        }
    }
    realm.type_error(&format!("this is not a Date object. (Date.prototype.{})", method))
}

fn set_time_value(realm: &mut Realm, id: ObjectId, t: f64) -> Value {
    let t = time_clip(t);
    realm.obj_mut(id).kind = ObjectKind::Date(t);
    Value::Number(t)
}

fn date_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if !realm.is_construct_call() {
        return Ok(Value::str(&to_string(now_ms())));
    }
    let t = match args.len() {
        0 => now_ms(),
        1 => {
            let v = args[0].clone();
            if let V::Object(id) = v.v() {
                if let ObjectKind::Date(t) = &realm.obj(id).kind {
                    let t = *t;
                    return finish_ctor(realm, t);
                }
            }
            let prim = realm.to_primitive(v, crate::interop::Hint::Default)?;
            match prim.v() {
                V::String(s) => parse_date(&s.to_rust_string()),
                other => time_clip(realm.to_number(other.to_value())?),
            }
        }
        _ => {
            let mut nums = [f64::NAN, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
            for (i, slot) in nums.iter_mut().enumerate() {
                if let Some(a) = args.get(i) {
                    *slot = realm.to_number(a.clone())?;
                }
            }
            let mut y = nums[0];
            if y.is_finite() {
                let yi = to_integer(y);
                if (0.0..=99.0).contains(&yi) {
                    y = 1900.0 + yi;
                }
            }
            let d = make_day(y, nums[1], nums[2]);
            let tm = make_time(nums[3], nums[4], nums[5], nums[6]);
            time_clip(utc_from_local(make_date(d, tm)))
        }
    };
    finish_ctor(realm, t)
}

fn finish_ctor(realm: &mut Realm, t: f64) -> JsResult<Value> {
    let default = realm.intrinsics.others["DatePrototype"];
    let proto = realm.proto_from_new_target(default);
    Ok(Value::Object(realm.alloc(Object::new(Some(proto), ObjectKind::Date(time_clip(t))))))
}

fn date_now(_realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(Value::Number(now_ms()))
}

fn date_parse(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let s = realm.to_rust_string(arg(args, 0))?;
    Ok(Value::Number(parse_date(&s)))
}

fn date_utc(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let mut nums = [f64::NAN, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
    for (i, slot) in nums.iter_mut().enumerate() {
        if let Some(a) = args.get(i) {
            *slot = realm.to_number(a.clone())?;
        }
    }
    let mut y = nums[0];
    if y.is_finite() {
        let yi = to_integer(y);
        if (0.0..=99.0).contains(&yi) {
            y = 1900.0 + yi;
        }
    }
    let d = make_day(y, nums[1], nums[2]);
    let tm = make_time(nums[3], nums[4], nums[5], nums[6]);
    Ok(Value::Number(time_clip(make_date(d, tm))))
}

macro_rules! getter {
    ($name:ident, $local:expr, $f:expr) => {
        fn $name(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
            let (_, t) = this_time(realm, &this, stringify!($name))?;
            if t.is_nan() {
                return Ok(Value::Number(f64::NAN));
            }
            let t = if $local { local_time(t) } else { t };
            Ok(Value::Number($f(t)))
        }
    };
}

getter!(get_full_year, true, year_from_time);
getter!(get_utc_full_year, false, year_from_time);
getter!(get_month, true, month_from_time);
getter!(get_utc_month, false, month_from_time);
getter!(get_date, true, date_from_time);
getter!(get_utc_date, false, date_from_time);
getter!(get_day, true, week_day);
getter!(get_utc_day, false, week_day);
getter!(get_hours, true, hour_from_time);
getter!(get_utc_hours, false, hour_from_time);
getter!(get_minutes, true, min_from_time);
getter!(get_utc_minutes, false, min_from_time);
getter!(get_seconds, true, sec_from_time);
getter!(get_utc_seconds, false, sec_from_time);
getter!(get_milliseconds, true, ms_from_time);
getter!(get_utc_milliseconds, false, ms_from_time);

fn get_time(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_time(realm, &this, "getTime")?;
    Ok(Value::Number(t))
}

fn get_year(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_time(realm, &this, "getYear")?;
    if t.is_nan() {
        return Ok(Value::Number(f64::NAN));
    }
    Ok(Value::Number(year_from_time(local_time(t)) - 1900.0))
}

fn get_timezone_offset(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_time(realm, &this, "getTimezoneOffset")?;
    if t.is_nan() {
        return Ok(Value::Number(f64::NAN));
    }
    Ok(Value::Number((t - local_time(t)) / MS_PER_MINUTE))
}

fn set_time(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, _) = this_time(realm, &this, "setTime")?;
    let t = realm.to_number(arg(args, 0))?;
    Ok(set_time_value(realm, id, t))
}

/// Shared implementation of the `set*` family: `fields` lists which of
/// (hour, min, sec, ms) or (year, month, date) the arguments replace.
fn set_parts(realm: &mut Realm, this: Value, args: &[Value], method: &str, local: bool, first: usize, max: usize, date_part: bool) -> JsResult<Value> {
    let (id, t0) = this_time(realm, &this, method)?;
    let mut nums = Vec::with_capacity(max);
    for a in args.iter().take(max) {
        nums.push(realm.to_number(a.clone())?);
    }
    if nums.is_empty() {
        return Ok(set_time_value(realm, id, f64::NAN));
    }
    let t = if t0.is_nan() {
        if date_part && first == 0 {
            // setFullYear on an invalid date starts from +0.
            0.0
        } else {
            // An invalid date stays as it is (the arguments were converted
            // first and may have changed it).
            return Ok(Value::Number(f64::NAN));
        }
    } else if local {
        local_time(t0)
    } else {
        t0
    };
    let result = if date_part {
        let mut parts = [year_from_time(t), month_from_time(t), date_from_time(t)];
        for (k, n) in nums.iter().enumerate() {
            parts[first + k] = *n;
        }
        make_date(make_day(parts[0], parts[1], parts[2]), time_within_day(t))
    } else {
        let mut parts = [hour_from_time(t), min_from_time(t), sec_from_time(t), ms_from_time(t)];
        for (k, n) in nums.iter().enumerate() {
            parts[first + k] = *n;
        }
        make_date(day(t), make_time(parts[0], parts[1], parts[2], parts[3]))
    };
    let u = if local { utc_from_local(result) } else { result };
    Ok(set_time_value(realm, id, u))
}

fn set_milliseconds(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    set_parts(realm, this, args, "setMilliseconds", true, 3, 1, false)
}
fn set_utc_milliseconds(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    set_parts(realm, this, args, "setUTCMilliseconds", false, 3, 1, false)
}
fn set_seconds(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    set_parts(realm, this, args, "setSeconds", true, 2, 2, false)
}
fn set_utc_seconds(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    set_parts(realm, this, args, "setUTCSeconds", false, 2, 2, false)
}
fn set_minutes(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    set_parts(realm, this, args, "setMinutes", true, 1, 3, false)
}
fn set_utc_minutes(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    set_parts(realm, this, args, "setUTCMinutes", false, 1, 3, false)
}
fn set_hours(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    set_parts(realm, this, args, "setHours", true, 0, 4, false)
}
fn set_utc_hours(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    set_parts(realm, this, args, "setUTCHours", false, 0, 4, false)
}
fn set_date(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    set_parts(realm, this, args, "setDate", true, 2, 1, true)
}
fn set_utc_date(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    set_parts(realm, this, args, "setUTCDate", false, 2, 1, true)
}
fn set_month(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    set_parts(realm, this, args, "setMonth", true, 1, 2, true)
}
fn set_utc_month(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    set_parts(realm, this, args, "setUTCMonth", false, 1, 2, true)
}
fn set_full_year(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    set_parts(realm, this, args, "setFullYear", true, 0, 3, true)
}
fn set_utc_full_year(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    set_parts(realm, this, args, "setUTCFullYear", false, 0, 3, true)
}

fn set_year(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (id, t0) = this_time(realm, &this, "setYear")?;
    let mut y = realm.to_number(arg(args, 0))?;
    if y.is_nan() {
        return Ok(set_time_value(realm, id, f64::NAN));
    }
    let yi = to_integer(y);
    if (0.0..=99.0).contains(&yi) {
        y = 1900.0 + yi;
    }
    let t = if t0.is_nan() { 0.0 } else { local_time(t0) };
    let d = make_day(y, month_from_time(t), date_from_time(t));
    let r = utc_from_local(make_date(d, time_within_day(t)));
    Ok(set_time_value(realm, id, r))
}

fn to_string_m(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_time(realm, &this, "toString")?;
    Ok(Value::str(&to_string(t)))
}
fn to_date_string_m(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_time(realm, &this, "toDateString")?;
    Ok(Value::str(&to_date_string(t)))
}
fn to_time_string_m(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_time(realm, &this, "toTimeString")?;
    Ok(Value::str(&to_time_string(t)))
}
fn to_utc_string_m(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_time(realm, &this, "toUTCString")?;
    Ok(Value::str(&to_utc_string(t)))
}
fn to_iso_string_m(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_time(realm, &this, "toISOString")?;
    if t.is_nan() {
        return realm.range_error("Invalid time value");
    }
    Ok(Value::str(&to_iso_string(t)))
}

fn to_json(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let o = realm.to_object(this)?;
    let tv = realm.to_primitive(Value::Object(o), crate::interop::Hint::Number)?;
    if let V::Number(n) = tv.v() {
        if !n.is_finite() {
            return Ok(Value::Null);
        }
    }
    realm.invoke(Value::Object(o), "toISOString", &[])
}

fn to_primitive(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_object(realm, &this, "Date.prototype[Symbol.toPrimitive]")?;
    let hint = match (arg(args, 0)).v() {
        V::String(s) if s.eq_str("number") => crate::interop::Hint::Number,
        V::String(s) if s.eq_str("string") || s.eq_str("default") => crate::interop::Hint::String,
        _ => return realm.type_error("Invalid hint"),
    };
    ordinary_to_primitive(realm, id, hint)
}

/// OrdinaryToPrimitive (ECMA-262 §7.1.1.1) without the exotic hook.
pub fn ordinary_to_primitive(realm: &mut Realm, id: ObjectId, hint: crate::interop::Hint) -> JsResult<Value> {
    let order: [&str; 2] = if hint == crate::interop::Hint::String { ["toString", "valueOf"] } else { ["valueOf", "toString"] };
    for name in order {
        let m = realm.get_str(Value::Object(id), name)?;
        if realm.is_callable(&m) {
            let r = realm.call(m, Value::Object(id), &[])?;
            if !r.is_object() {
                return Ok(r);
            }
        }
    }
    realm.type_error("Cannot convert object to primitive value")
}

fn to_locale_string(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_time(realm, &this, "toLocaleString")?;
    super::intl::date_to_locale(realm, t, args, super::intl::DateDefaults::DateTime)
}
fn to_locale_date_string(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_time(realm, &this, "toLocaleDateString")?;
    super::intl::date_to_locale(realm, t, args, super::intl::DateDefaults::Date)
}
fn to_locale_time_string(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, t) = this_time(realm, &this, "toLocaleTimeString")?;
    super::intl::date_to_locale(realm, t, args, super::intl::DateDefaults::Time)
}
