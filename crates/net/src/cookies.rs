//! An HTTP cookie jar (RFC 6265): parsing `Set-Cookie`, domain/path
//! matching, expiry, and persistence in the Netscape `cookies.txt` format.

use crate::url::Url;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    /// Lower-cased domain without a leading dot.
    pub domain: String,
    /// True when the cookie came with an explicit `Domain` attribute
    /// (then it also matches subdomains).
    pub host_only: bool,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    /// Unix seconds; `None` = session cookie.
    pub expires: Option<u64>,
    /// `Lax`, `Strict`, `None`, or empty when unspecified (treated as
    /// Lax, like Chrome, with the 2-minute top-level POST allowance).
    pub same_site: String,
    pub created: u64,
    /// CHIPS: `Partitioned` cookies are keyed by the top-level site they
    /// were set under and only sent in that context.
    pub partition_key: Option<String>,
}

/// The context a request is made in, for SameSite and partitioning.
#[derive(Debug, Clone, Default)]
pub struct CookieContext {
    /// URL of the initiating document (None = browser-initiated, e.g. a
    /// typed URL: treated as same-site).
    pub initiator: Option<Url>,
    /// Top-level navigation (link click, form submission).
    pub top_level_navigation: bool,
    /// GET/HEAD/OPTIONS/TRACE.
    pub safe_method: bool,
    /// Every URL of the redirect chain so far is same-site with the
    /// initiator.
    pub same_site_chain: bool,
}

impl CookieContext {
    /// Context for a request to `url`.
    pub fn new(initiator: Option<&Url>, url: &Url, top_level_navigation: bool, method: &str) -> CookieContext {
        let mut c = CookieContext {
            initiator: initiator.filter(|i| i.is_http()).cloned(),
            top_level_navigation,
            safe_method: matches!(method, "GET" | "HEAD" | "OPTIONS" | "TRACE"),
            same_site_chain: true,
        };
        c.same_site_chain = c.is_same_site(url);
        c
    }

    /// Schemeful same-site check of `url` against the initiator.
    pub fn is_same_site(&self, url: &Url) -> bool {
        match &self.initiator {
            None => true,
            Some(i) => i.scheme == url.scheme && crate::url::site_of(&i.host) == crate::url::site_of(&url.host),
        }
    }

    /// The top-level site used as partition key.
    pub fn partition_site(&self, url: &Url) -> String {
        match (&self.initiator, self.top_level_navigation) {
            (Some(i), false) => format!("{}://{}", i.scheme, crate::url::site_of(&i.host)),
            _ => format!("{}://{}", url.scheme, crate::url::site_of(&url.host)),
        }
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs()
}

/// Longest lifetime of a cookie (RFC 6265bis §5.6.1/§5.6.2: 400 days).
const MAX_COOKIE_AGE: u64 = 400 * 86400;
/// Largest name + value accepted (RFC 6265bis §5.6, step 4).
const MAX_NAME_VALUE: usize = 4096;
/// Largest attribute value accepted (RFC 6265bis §5.6, step 7).
const MAX_ATTRIBUTE_VALUE: usize = 1024;
/// Cookies kept per registrable domain and in total (Chrome's limits).
const MAX_PER_SITE: usize = 180;
const MAX_TOTAL: usize = 3300;

/// Is `url` a secure context for cookies: `https`/`wss`, or a
/// potentially trustworthy loopback origin (`localhost`, `*.localhost`,
/// `127.0.0.0/8`, `[::1]`), as in Chrome?
pub fn is_secure_url(url: &Url) -> bool {
    match url.scheme.as_str() {
        "https" | "wss" => true,
        "http" | "ws" => is_loopback_host(&url.host),
        _ => false,
    }
}

/// `localhost`, `*.localhost` or a loopback IP address.
pub fn is_loopback_host(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    let bare = h.trim_start_matches('[').trim_end_matches(']');
    h == "localhost" || h.ends_with(".localhost") || bare.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

fn is_ip_host(host: &str) -> bool {
    host.starts_with('[') || host.parse::<std::net::IpAddr>().is_ok()
}

/// Control characters that make a cookie line invalid (RFC 6265bis
/// §5.6, step 2: %x00-08 / %x0A-1F / %x7F; HTAB is allowed).
fn has_forbidden_ctl(s: &str) -> bool {
    s.bytes().any(|b| (b < 0x20 && b != b'\t') || b == 0x7f)
}

/// Default cookie path per RFC 6265 §5.1.4.
pub fn default_path(url_path: &str) -> String {
    if !url_path.starts_with('/') {
        return "/".into();
    }
    match url_path.rfind('/') {
        Some(0) | None => "/".into(),
        Some(i) => url_path[..i].to_string(),
    }
}

fn domain_matches(cookie_domain: &str, host: &str) -> bool {
    host == cookie_domain || (host.ends_with(cookie_domain) && host[..host.len() - cookie_domain.len()].ends_with('.'))
}

fn path_matches(cookie_path: &str, req_path: &str) -> bool {
    if req_path == cookie_path {
        return true;
    }
    if req_path.starts_with(cookie_path) {
        return cookie_path.ends_with('/') || req_path[cookie_path.len()..].starts_with('/');
    }
    false
}

/// Parse an HTTP date (RFC 1123 / RFC 850 / asctime) into Unix seconds.
pub fn parse_http_date(s: &str) -> Option<u64> {
    let s = s.trim();
    let toks: Vec<&str> = s.split(|c: char| c == ' ' || c == '-' || c == ',' || c == '\t').filter(|t| !t.is_empty()).collect();
    // Find day, month, year, time among the tokens (RFC 6265 §5.1.1 algorithm).
    let mut day = None;
    let mut month = None;
    let mut year = None;
    let mut time = None;
    for t in &toks {
        if time.is_none() && t.contains(':') {
            let parts: Vec<&str> = t.split(':').collect();
            if parts.len() == 3 {
                if let (Ok(h), Ok(m), Ok(sec)) = (parts[0].parse::<u64>(), parts[1].parse::<u64>(), parts[2].parse::<u64>()) {
                    time = Some((h, m, sec));
                    continue;
                }
            }
        }
        if month.is_none() && t.len() >= 3 {
            let m3 = t[..3].to_ascii_lowercase();
            let idx = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"]
                .iter()
                .position(|m| *m == m3);
            if let Some(i) = idx {
                month = Some(i as u64 + 1);
                continue;
            }
        }
        if let Ok(n) = t.parse::<u64>() {
            if day.is_none() && (1..=31).contains(&n) && t.len() <= 2 {
                day = Some(n);
            } else if year.is_none() {
                year = Some(if n < 70 { n + 2000 } else if n < 100 { n + 1900 } else { n });
            }
        }
    }
    let (day, month, year, (h, m, sec)) = (day?, month?, year?, time?);
    if year < 1601 || h > 23 || m > 59 || sec > 59 {
        return None;
    }
    // Days from civil (Howard Hinnant's algorithm).
    let y = if month <= 2 { year as i64 - 1 } else { year as i64 };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    if days < 0 {
        return Some(0);
    }
    Some(days as u64 * 86400 + h * 3600 + m * 60 + sec)
}

impl Cookie {
    /// Parse one `Set-Cookie` header value received from `url`
    /// (RFC 6265bis §5.6 and the storage requirements of §5.7 that only
    /// depend on the cookie and the request URL).
    pub fn parse(header: &str, url: &Url) -> Option<Cookie> {
        // A CR, LF or NUL truncates the line; other controls void it.
        let header = match header.find(['\r', '\n', '\0']) {
            Some(i) => &header[..i],
            None => header,
        };
        let (nv, attrs) = match header.find(';') {
            Some(i) => (&header[..i], &header[i + 1..]),
            None => (header, ""),
        };
        if has_forbidden_ctl(nv) {
            return None;
        }
        // A pair without `=` is a nameless cookie (the whole text is the
        // value), as in RFC 6265bis and browsers.
        let (name, value) = match nv.find('=') {
            Some(i) => (nv[..i].trim_matches([' ', '\t']), nv[i + 1..].trim_matches([' ', '\t'])),
            None => ("", nv.trim_matches([' ', '\t'])),
        };
        if name.is_empty() && value.is_empty() {
            return None;
        }
        if name.len() + value.len() > MAX_NAME_VALUE {
            return None;
        }
        let host = url.host.to_ascii_lowercase();
        let mut c = Cookie {
            name: name.to_string(),
            // Quotes are part of the value (RFC 6265bis keeps them).
            value: value.to_string(),
            domain: host.clone(),
            host_only: true,
            path: default_path(&url.path),
            secure: false,
            http_only: false,
            expires: None,
            same_site: String::new(),
            created: now(),
            partition_key: None,
        };
        let mut partitioned = false;
        let mut max_age: Option<i64> = None;
        let mut expires: Option<u64> = None;
        let mut domain_attr: Option<String> = None;
        for attr in attrs.split(';') {
            let attr = attr.trim_matches([' ', '\t']);
            if attr.is_empty() {
                continue;
            }
            let (k, v) = match attr.find('=') {
                Some(i) => (attr[..i].trim_matches([' ', '\t']).to_ascii_lowercase(), attr[i + 1..].trim_matches([' ', '\t'])),
                None => (attr.to_ascii_lowercase(), ""),
            };
            if v.len() > MAX_ATTRIBUTE_VALUE || has_forbidden_ctl(v) {
                continue;
            }
            match k.as_str() {
                "domain" => {
                    // The last Domain attribute wins; an empty one is ignored.
                    let d = v.strip_prefix('.').unwrap_or(v).to_ascii_lowercase();
                    if !d.is_empty() {
                        domain_attr = Some(d);
                    }
                }
                "path" => {
                    c.path = if v.starts_with('/') { v.to_string() } else { default_path(&url.path) };
                }
                "secure" => c.secure = true,
                "httponly" => c.http_only = true,
                "max-age" => {
                    let digits = v.strip_prefix('-').unwrap_or(v);
                    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                        max_age = Some(v.parse::<i64>().unwrap_or(if v.starts_with('-') { i64::MIN } else { i64::MAX }));
                    }
                }
                "expires" => {
                    if let Some(t) = parse_http_date(v) {
                        expires = Some(t);
                    }
                }
                "samesite" => {
                    c.same_site = match v.to_ascii_lowercase().as_str() {
                        "strict" => "Strict".into(),
                        "lax" => "Lax".into(),
                        "none" => "None".into(),
                        _ => String::new(),
                    }
                }
                "partitioned" => partitioned = true,
                _ => {}
            }
        }
        if let Some(d) = domain_attr {
            if is_ip_host(&host) {
                // IP hosts only domain-match themselves.
                if d != host.trim_start_matches('[').trim_end_matches(']') && d != host {
                    return None;
                }
            } else {
                if !domain_matches(&d, &host) {
                    return None;
                }
                // A Domain shorter than the registrable domain is (part
                // of) a public suffix (RFC 6265bis §5.7, step 9).
                let site = crate::url::site_of(&host);
                if d.len() < site.len() {
                    return None;
                }
                c.host_only = false;
                c.domain = d;
            }
        }
        let secure_origin = is_secure_url(url);
        // Secure cookies only from secure origins; SameSite=None and
        // Partitioned need Secure; cookie prefixes (RFC 6265bis 4.1.3).
        if c.secure && !secure_origin {
            return None;
        }
        if (c.same_site == "None" || partitioned) && !c.secure {
            return None;
        }
        let lower = c.name.to_ascii_lowercase();
        if lower.starts_with("__secure-") && !c.secure {
            return None;
        }
        if lower.starts_with("__host-") && (!c.secure || !c.host_only || c.path != "/") {
            return None;
        }
        if c.name.is_empty() {
            let v = c.value.to_ascii_lowercase();
            if v.starts_with("__secure-") || v.starts_with("__host-") {
                return None;
            }
        }
        if partitioned {
            // Filled in by the jar from the request context.
            c.partition_key = Some(String::new());
        }
        let t = now();
        if let Some(n) = max_age {
            c.expires = Some(if n <= 0 { 0 } else { t + (n as u64).min(MAX_COOKIE_AGE) });
        } else if let Some(e) = expires {
            c.expires = Some(e.min(t + MAX_COOKIE_AGE));
        }
        Some(c)
    }

    /// The `name=value` pair as sent in a `Cookie` header (a nameless
    /// cookie is just its value).
    pub fn pair(&self) -> String {
        if self.name.is_empty() {
            self.value.clone()
        } else {
            format!("{}={}", self.name, self.value)
        }
    }

    pub fn is_expired(&self) -> bool {
        self.expires.map(|t| t <= now()).unwrap_or(false)
    }
}

/// In-memory cookie store.
#[derive(Debug, Default, Clone)]
pub struct CookieJar {
    cookies: Vec<Cookie>,
    /// Set when a cookie changed since the last `save`.
    pub dirty: bool,
    /// Browser setting: cross-site subresource requests neither send nor
    /// store cookies (top-level navigations are unaffected).
    pub block_third_party: bool,
    /// Cookies stored or removed since the last [`CookieJar::sync`]: this
    /// process's version of them wins over the file's.
    touched: HashSet<CookieKey>,
    /// `clear()` since the last sync: the file's cookies are not merged.
    cleared: bool,
    /// Hash of the file's contents when last read or written. Contents,
    /// not the modification time: two writes within the file system's
    /// timestamp granularity (common on Windows) keep the same time.
    synced: Option<u64>,
}

/// What identifies a stored cookie (RFC 6265bis §5.7, step 23).
type CookieKey = (String, String, bool, String, Option<String>);

fn key_of(c: &Cookie) -> CookieKey {
    (c.name.clone(), c.domain.clone(), c.host_only, c.path.clone(), c.partition_key.clone())
}

impl CookieJar {
    pub fn new() -> CookieJar {
        CookieJar::default()
    }

    /// Store a cookie received over HTTP (replacing one with the same
    /// name/domain/path); expired cookies delete.
    pub fn store(&mut self, cookie: Cookie) {
        self.store_checked(cookie, true, true);
    }

    /// RFC 6265bis §5.7 storage: `from_http` is false for `document.cookie`
    /// (which can neither create nor replace HttpOnly cookies);
    /// `secure_origin` false applies "leave secure cookies alone". Returns
    /// whether the jar changed.
    fn store_checked(&mut self, mut cookie: Cookie, from_http: bool, secure_origin: bool) -> bool {
        if !from_http && cookie.http_only {
            return false;
        }
        if !cookie.secure && !secure_origin {
            let shadows = self.cookies.iter().any(|c| {
                c.secure
                    && c.name == cookie.name
                    && (domain_matches(&c.domain, &cookie.domain) || domain_matches(&cookie.domain, &c.domain))
                    && path_matches(&c.path, &cookie.path)
            });
            if shadows {
                return false;
            }
        }
        let old = self.cookies.iter().position(|c| c.name == cookie.name && c.domain == cookie.domain && c.host_only == cookie.host_only && c.path == cookie.path && c.partition_key == cookie.partition_key);
        if let Some(i) = old {
            if self.cookies[i].http_only && !from_http {
                return false;
            }
            // Keep the original creation time (and so the cookie order).
            cookie.created = self.cookies[i].created;
            self.touched.insert(key_of(&cookie));
            if cookie.is_expired() {
                self.cookies.remove(i);
            } else {
                self.cookies[i] = cookie;
            }
            self.dirty = true;
            return true;
        }
        if cookie.is_expired() {
            return false;
        }
        let site = crate::url::site_of(&cookie.domain);
        self.touched.insert(key_of(&cookie));
        self.cookies.push(cookie);
        self.evict(&site);
        self.dirty = true;
        true
    }

    /// Enforce the per-site and global limits: expired cookies go first,
    /// then the least recently created ones.
    fn evict(&mut self, site: &str) {
        let per_site = self.cookies.iter().filter(|c| crate::url::site_of(&c.domain) == site).count();
        if per_site <= MAX_PER_SITE && self.cookies.len() <= MAX_TOTAL {
            return;
        }
        self.cookies.retain(|c| !c.is_expired());
        while self.cookies.iter().filter(|c| crate::url::site_of(&c.domain) == site).count() > MAX_PER_SITE {
            let oldest = self.cookies.iter().enumerate().filter(|(_, c)| crate::url::site_of(&c.domain) == site).min_by_key(|(_, c)| c.created).map(|(i, _)| i);
            match oldest {
                Some(i) => {
                    let gone = self.cookies.remove(i);
                    self.touched.insert(key_of(&gone));
                }
                None => break,
            }
        }
        while self.cookies.len() > MAX_TOTAL {
            let oldest = self.cookies.iter().enumerate().min_by_key(|(_, c)| c.created).map(|(i, _)| i).unwrap_or(0);
            let gone = self.cookies.remove(oldest);
            self.touched.insert(key_of(&gone));
        }
    }

    /// Apply every `Set-Cookie` header of a response from `url`
    /// (no SameSite context: treated as same-site).
    pub fn store_from_headers(&mut self, url: &Url, set_cookie_values: &[String]) {
        let secure = is_secure_url(url);
        for v in set_cookie_values {
            if let Some(mut c) = Cookie::parse(v, url) {
                if c.partition_key.is_some() {
                    c.partition_key = Some(format!("{}://{}", url.scheme, crate::url::site_of(&url.host)));
                }
                self.store_checked(c, true, secure);
            }
        }
    }

    /// `document.cookie = value` for a document at `url`: HttpOnly
    /// cookies can be neither created nor overwritten.
    pub fn store_from_script(&mut self, url: &Url, value: &str) {
        if !url.is_http() {
            return;
        }
        if let Some(mut c) = Cookie::parse(value, url) {
            if c.partition_key.is_some() {
                c.partition_key = Some(format!("{}://{}", url.scheme, crate::url::site_of(&url.host)));
            }
            self.store_checked(c, false, is_secure_url(url));
        }
    }

    /// Apply the `Set-Cookie` headers of a response, honouring SameSite
    /// (cross-site subresource responses cannot set Lax/Strict cookies)
    /// and keying `Partitioned` cookies by the top-level site.
    pub fn store_from_response(&mut self, url: &Url, set_cookie_values: &[String], ctx: &CookieContext) {
        let same_site = ctx.same_site_chain && ctx.is_same_site(url);
        if self.block_third_party && !same_site && !ctx.top_level_navigation {
            return;
        }
        let secure = is_secure_url(url);
        for v in set_cookie_values {
            let Some(mut c) = Cookie::parse(v, url) else { continue };
            if !same_site && !ctx.top_level_navigation && c.same_site != "None" {
                continue;
            }
            if c.partition_key.is_some() {
                c.partition_key = Some(ctx.partition_site(url));
            }
            self.store_checked(c, true, secure);
        }
    }

    /// Cookies to send for a request to `url` made in `ctx` (SameSite
    /// rules: Strict only same-site; Lax/unspecified also on top-level
    /// safe navigations; unspecified cookies younger than two minutes
    /// also on top-level POSTs).
    pub fn header_for_context(&self, url: &Url, ctx: &CookieContext) -> Option<String> {
        let same_site = ctx.same_site_chain && ctx.is_same_site(url);
        if self.block_third_party && !same_site && !ctx.top_level_navigation {
            return None;
        }
        let partition = ctx.partition_site(url);
        let t = now();
        self.header_filtered(url, |c| {
            if let Some(p) = &c.partition_key {
                if *p != partition {
                    return false;
                }
            }
            if same_site {
                return true;
            }
            match c.same_site.as_str() {
                "None" => true,
                "Strict" => false,
                "Lax" => ctx.top_level_navigation && ctx.safe_method,
                _ => ctx.top_level_navigation && (ctx.safe_method || t.saturating_sub(c.created) < 120),
            }
        })
    }

    /// Cookies to send for a request to `url`, as the `Cookie` header
    /// value (`a=1; b=2`), or None when there are none (same-site
    /// context).
    pub fn header_for(&self, url: &Url) -> Option<String> {
        let partition = format!("{}://{}", url.scheme, crate::url::site_of(&url.host));
        self.header_filtered(url, |c| c.partition_key.as_ref().map(|p| *p == partition).unwrap_or(true))
    }

    /// Cookies matching `url` (domain, path, Secure, expiry), in the
    /// RFC 6265bis §5.8.3 order: longer paths first, then earlier creation.
    fn matching<'a>(&'a self, url: &Url, keep: impl Fn(&Cookie) -> bool) -> Vec<&'a Cookie> {
        let host = url.host.to_ascii_lowercase();
        let path = if url.path.is_empty() { "/" } else { url.path.as_str() };
        let secure = is_secure_url(url);
        let mut matched: Vec<&Cookie> = self
            .cookies
            .iter()
            .filter(|c| !c.is_expired())
            .filter(|c| if c.host_only { c.domain == host } else { domain_matches(&c.domain, &host) })
            .filter(|c| path_matches(&c.path, path))
            .filter(|c| !c.secure || secure)
            .filter(|c| keep(c))
            .collect();
        // Stable: equal paths and times keep the storage order.
        matched.sort_by(|a, b| b.path.len().cmp(&a.path.len()).then(a.created.cmp(&b.created)));
        matched
    }

    fn header_filtered(&self, url: &Url, keep: impl Fn(&Cookie) -> bool) -> Option<String> {
        let matched = self.matching(url, keep);
        if matched.is_empty() {
            return None;
        }
        Some(matched.iter().map(|c| c.pair()).collect::<Vec<_>>().join("; "))
    }

    pub fn cookies(&self) -> &[Cookie] {
        &self.cookies
    }

    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    /// Remove all cookies for a host (and its subdomains).
    pub fn clear_host(&mut self, host: &str) {
        let host = host.to_ascii_lowercase();
        let touched = &mut self.touched;
        self.cookies.retain(|c| {
            let keep = !domain_matches(&c.domain, &host) && !domain_matches(&host, &c.domain);
            if !keep {
                touched.insert(key_of(c));
            }
            keep
        });
        self.dirty = true;
    }

    pub fn clear(&mut self) {
        self.cookies.clear();
        self.cleared = true;
        self.dirty = true;
    }

    /// Remove expired and session cookies (call on shutdown before saving
    /// if session cookies must not persist).
    pub fn purge_session(&mut self) {
        self.cookies.retain(|c| c.expires.is_some() && !c.is_expired());
    }

    /// Cookies visible to scripts of a top-level document at `url`
    /// (`document.cookie`): excludes HttpOnly, and Partitioned cookies of
    /// other top-level sites.
    pub fn script_cookies(&self, url: &Url) -> String {
        if !url.is_http() {
            return String::new();
        }
        let partition = format!("{}://{}", url.scheme, crate::url::site_of(&url.host));
        self.matching(url, |c| !c.http_only && c.partition_key.as_ref().is_none_or(|p| *p == partition)).iter().map(|c| c.pair()).collect::<Vec<_>>().join("; ")
    }

    /// Serialize in Netscape `cookies.txt` format (persistent cookies only).
    pub fn to_netscape(&self) -> String {
        self.netscape(false)
    }

    /// Netscape format including session cookies (expiry `0`, as curl
    /// writes them): for saving a whole session.
    pub fn to_netscape_with_session(&self) -> String {
        self.netscape(true)
    }

    /// The seven Netscape columns. Attributes the format lacks (SameSite,
    /// creation time) go in a preceding `#RabbitHole-Cookie:` comment that
    /// other tools ignore; Partitioned cookies are written as a whole
    /// comment line so tools without partitioning never send them.
    fn netscape(&self, session: bool) -> String {
        let mut out = String::from("# Netscape HTTP Cookie File\n# Written by RabbitHole\n\n");
        for c in &self.cookies {
            let exp = match c.expires {
                Some(e) => e,
                None if session => 0,
                None => continue,
            };
            if c.is_expired() {
                continue;
            }
            let domain = if c.host_only { c.domain.clone() } else { format!(".{}", c.domain) };
            let name = if c.http_only { format!("#HttpOnly_{}", domain) } else { domain };
            out.push_str(&format!("#RabbitHole-Cookie: samesite={}; created={}\n", if c.same_site.is_empty() { "Unspecified" } else { &c.same_site }, c.created));
            let line = format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                name,
                if c.host_only { "FALSE" } else { "TRUE" },
                c.path,
                if c.secure { "TRUE" } else { "FALSE" },
                exp,
                c.name,
                c.value
            );
            match &c.partition_key {
                Some(p) => out.push_str(&format!("#RabbitHole-Partitioned: {p}\t{line}")),
                None => out.push_str(&line),
            }
        }
        out
    }

    pub fn from_netscape(text: &str) -> CookieJar {
        let mut jar = CookieJar::new();
        let mut meta: Option<(String, Option<u64>)> = None;
        for raw in text.lines() {
            if let Some(m) = raw.strip_prefix("#RabbitHole-Cookie:") {
                let mut same_site = String::new();
                let mut created = None;
                for part in m.split(';') {
                    match part.trim().split_once('=') {
                        Some(("samesite", v)) => same_site = if matches!(v, "Strict" | "Lax" | "None") { v.to_string() } else { String::new() },
                        Some(("created", v)) => created = v.parse::<u64>().ok(),
                        _ => {}
                    }
                }
                meta = Some((same_site, created));
                continue;
            }
            let (partition_key, line) = match raw.strip_prefix("#RabbitHole-Partitioned: ").and_then(|r| r.split_once('\t')) {
                Some((p, rest)) => (Some(p.to_string()), rest),
                None => (None, raw),
            };
            let (http_only, line) = match line.strip_prefix("#HttpOnly_") {
                Some(rest) => (true, rest),
                None => (false, line),
            };
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let (same_site, created) = meta.take().unwrap_or_default();
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 7 {
                continue;
            }
            let host_only = f[1].eq_ignore_ascii_case("FALSE") && !f[0].starts_with('.');
            // `0` marks a session cookie.
            let expires = f[4].parse::<u64>().ok().filter(|&e| e != 0);
            let c = Cookie {
                name: f[5].to_string(),
                value: f[6..].join("\t"),
                domain: f[0].trim_start_matches('.').to_ascii_lowercase(),
                host_only,
                path: f[2].to_string(),
                secure: f[3].eq_ignore_ascii_case("TRUE"),
                http_only,
                expires,
                same_site,
                created: created.unwrap_or_else(now),
                partition_key,
            };
            if !c.is_expired() {
                jar.cookies.push(c);
            }
        }
        jar
    }

    pub fn save(&mut self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Write a sibling file and rename it, so a reader (another window
        // process sharing the profile) never sees a half-written jar.
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        let text = self.to_netscape();
        std::fs::write(&tmp, &text)?;
        std::fs::rename(&tmp, path)?;
        self.dirty = false;
        self.touched.clear();
        self.cleared = false;
        self.synced = Some(content_hash(&text));
        Ok(())
    }

    pub fn load(path: &Path) -> std::io::Result<CookieJar> {
        let text = std::fs::read_to_string(path)?;
        let mut jar = CookieJar::from_netscape(&text);
        jar.synced = Some(content_hash(&text));
        Ok(jar)
    }

    /// Share the persistent jar with other processes using the same file
    /// (browser windows): when the file changed since this jar last read
    /// or wrote it, merge it — cookies this process stored or removed
    /// since then keep its version, the others take the file's (new,
    /// updated, or deleted persistent cookies) — then write it if this jar
    /// has changes. Session cookies stay in memory.
    pub fn sync(&mut self, path: &Path) -> std::io::Result<()> {
        if let Ok(text) = std::fs::read_to_string(path) {
            let hash = content_hash(&text);
            if Some(hash) != self.synced {
                self.merge(CookieJar::from_netscape(&text));
                self.synced = Some(hash);
            }
        }
        if self.dirty {
            self.save(path)?;
        }
        Ok(())
    }

    fn merge(&mut self, disk: CookieJar) {
        if self.cleared {
            return;
        }
        let on_disk: HashSet<CookieKey> = disk.cookies.iter().map(key_of).collect();
        let touched = &self.touched;
        // Persistent cookies another process deleted.
        self.cookies.retain(|c| c.expires.is_none() || touched.contains(&key_of(c)) || on_disk.contains(&key_of(c)));
        for c in disk.cookies {
            let k = key_of(&c);
            if self.touched.contains(&k) {
                continue;
            }
            match self.cookies.iter().position(|m| key_of(m) == k) {
                Some(i) => self.cookies[i] = c,
                None => self.cookies.push(c),
            }
        }
    }

    /// Count of cookies per domain (diagnostics / settings UI).
    pub fn by_domain(&self) -> HashMap<String, usize> {
        let mut m = HashMap::new();
        for c in &self.cookies {
            *m.entry(c.domain.clone()).or_insert(0) += 1;
        }
        m
    }
}

/// Hash identifying a cookie file's contents (change detection between
/// window processes).
fn content_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse_absolute(s).unwrap()
    }

    #[test]
    fn parse_and_match() {
        let u = url("https://www.example.com/a/b/c");
        let c = Cookie::parse("sid=abc123; Path=/a; HttpOnly; Secure; Max-Age=3600", &u).unwrap();
        assert_eq!(c.name, "sid");
        assert_eq!(c.path, "/a");
        assert!(c.http_only && c.secure && c.host_only);
        assert!(c.expires.unwrap() > now());
        let mut jar = CookieJar::new();
        jar.store(c);
        jar.store(Cookie::parse("pref=dark; Domain=example.com; Path=/", &u).unwrap());
        assert_eq!(jar.header_for(&url("https://www.example.com/a/x")).unwrap(), "sid=abc123; pref=dark");
        // Secure cookie not sent over http; domain cookie still is.
        assert_eq!(jar.header_for(&url("http://www.example.com/a/x")).unwrap(), "pref=dark");
        // Path mismatch.
        assert_eq!(jar.header_for(&url("https://www.example.com/other")).unwrap(), "pref=dark");
        // Subdomain gets the Domain cookie but not the host-only one.
        assert_eq!(jar.header_for(&url("https://shop.example.com/a/")).unwrap(), "pref=dark");
        // Unrelated host.
        assert!(jar.header_for(&url("https://example.org/")).is_none());
        // Script cookies exclude HttpOnly.
        assert_eq!(jar.script_cookies(&url("https://www.example.com/a/")), "pref=dark");
    }

    #[test]
    fn rejects_foreign_domain_and_expires() {
        let u = url("http://a.example.com/");
        assert!(Cookie::parse("x=1; Domain=evil.com", &u).is_none());
        let c = Cookie::parse("x=1; Max-Age=0", &u).unwrap();
        assert!(c.is_expired());
        let mut jar = CookieJar::new();
        jar.store(Cookie::parse("x=1", &u).unwrap());
        jar.store(c);
        assert!(jar.is_empty());
    }

    #[test]
    fn same_site_and_partitioned() {
        let site = url("https://shop.example.com/");
        let mut jar = CookieJar::new();
        let first = CookieContext::new(None, &site, true, "GET");
        jar.store_from_response(
            &site,
            &["s=1; SameSite=Strict".into(), "l=2; SameSite=Lax".into(), "n=3; SameSite=None; Secure".into(), "d=4".into(), "bad=5; SameSite=None".into()],
            &first,
        );
        assert_eq!(jar.len(), 4, "SameSite=None without Secure is rejected");
        let names = |h: Option<String>| h.unwrap_or_default();
        // Same-site subresource from www.example.com: everything.
        let www = url("https://www.example.com/page");
        let ctx = CookieContext::new(Some(&www), &site, false, "GET");
        assert_eq!(names(jar.header_for_context(&site, &ctx)).matches('=').count(), 4);
        // Cross-site subresource: only SameSite=None.
        let other = url("https://news.other.org/");
        let ctx = CookieContext::new(Some(&other), &site, false, "GET");
        assert_eq!(names(jar.header_for_context(&site, &ctx)), "n=3");
        // Cross-site top-level GET navigation: Lax + unspecified + None.
        let ctx = CookieContext::new(Some(&other), &site, true, "GET");
        let h = names(jar.header_for_context(&site, &ctx));
        assert!(h.contains("l=2") && h.contains("d=4") && h.contains("n=3") && !h.contains("s=1"), "{h}");
        // Cross-site top-level POST: None + fresh unspecified (Lax+POST).
        let ctx = CookieContext::new(Some(&other), &site, true, "POST");
        let h = names(jar.header_for_context(&site, &ctx));
        assert!(h.contains("d=4") && h.contains("n=3") && !h.contains("l=2"), "{h}");
        // A cross-site subresource response cannot set Lax cookies.
        let ctx = CookieContext::new(Some(&other), &site, false, "GET");
        jar.store_from_response(&site, &["x=9".into(), "p=7; Secure; SameSite=None; Partitioned".into()], &ctx);
        assert!(!jar.cookies().iter().any(|c| c.name == "x"));
        // Partitioned cookie only under the same top-level site.
        let h = names(jar.header_for_context(&site, &ctx));
        assert!(h.contains("p=7"), "{h}");
        let third = url("https://third.net/");
        let ctx3 = CookieContext::new(Some(&third), &site, false, "GET");
        assert!(!names(jar.header_for_context(&site, &ctx3)).contains("p=7"));
        // Prefixes and insecure origins.
        let http = url("http://shop.example.com/");
        assert!(Cookie::parse("a=1; Secure", &http).is_none());
        assert!(Cookie::parse("__Host-a=1; Secure; Path=/; Domain=example.com", &site).is_none());
        assert!(Cookie::parse("__Host-a=1; Secure; Path=/", &site).is_some());
        assert!(Cookie::parse("__Secure-a=1", &site).is_none());
    }

    #[test]
    fn http_dates() {
        assert_eq!(parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT"), Some(784111777));
        assert_eq!(parse_http_date("Sunday, 06-Nov-94 08:49:37 GMT"), Some(784111777));
        assert_eq!(parse_http_date("Sun Nov  6 08:49:37 1994"), Some(784111777));
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
    }

    #[test]
    fn netscape_round_trip() {
        let u = url("https://www.example.com/");
        let mut jar = CookieJar::new();
        jar.store(Cookie::parse("a=1; Max-Age=1000; Domain=example.com; HttpOnly", &u).unwrap());
        jar.store(Cookie::parse("session=2", &u).unwrap());
        let text = jar.to_netscape();
        let back = CookieJar::from_netscape(&text);
        assert_eq!(back.len(), 1);
        assert_eq!(back.cookies()[0].name, "a");
        assert!(back.cookies()[0].http_only);
        assert!(!back.cookies()[0].host_only);
    }

    #[test]
    fn third_party_blocking() {
        let site = Url::parse("https://site.example/").unwrap();
        let other = Url::parse("https://tracker.other/").unwrap();
        let mut jar = CookieJar::new();
        jar.block_third_party = true;
        let tracker = Url::parse("https://tracker.other/pixel").unwrap();
        let sub = CookieContext::new(Some(&site), &tracker, false, "GET");
        jar.store_from_response(&tracker, &["id=1; SameSite=None; Secure".to_string()], &sub);
        assert!(jar.is_empty(), "third-party Set-Cookie ignored");
        let top = CookieContext::new(None, &other, true, "GET");
        jar.store_from_response(&tracker, &["id=1; SameSite=None; Secure".to_string()], &top);
        assert_eq!(jar.len(), 1, "first-party visit stores it");
        assert!(jar.header_for_context(&tracker, &sub).is_none(), "not sent cross-site");
        jar.block_third_party = false;
        assert!(jar.header_for_context(&tracker, &sub).is_some());
    }

    #[test]
    fn rfc6265bis_parsing() {
        let u = url("https://www.example.com/a/b");
        // Quotes belong to the value; whitespace around the pair goes.
        assert_eq!(Cookie::parse(" q = \"a b\" ; Path=/", &u).unwrap().value, "\"a b\"");
        // Nameless cookie.
        let c = Cookie::parse("justvalue", &u).unwrap();
        assert_eq!((c.name.as_str(), c.value.as_str()), ("", "justvalue"));
        assert_eq!(c.pair(), "justvalue");
        assert!(Cookie::parse("=", &u).is_none());
        assert!(Cookie::parse("__Host-x", &u).is_none(), "nameless value with a prefix");
        // Controls: CR/LF truncate, others void the cookie.
        assert_eq!(Cookie::parse("a=1\r\nSet-Cookie: b=2", &u).unwrap().value, "1");
        assert!(Cookie::parse("a=1\x01", &u).is_none());
        // Size limits.
        assert!(Cookie::parse(&format!("a={}", "v".repeat(4096)), &u).is_none());
        assert!(Cookie::parse(&format!("a={}", "v".repeat(4095)), &u).is_some());
        let long_path = format!("a=1; Path=/{}", "p".repeat(1100));
        assert_eq!(Cookie::parse(&long_path, &u).unwrap().path, "/a", "oversized attribute ignored");
        // Max-Age wins over Expires in either order; invalid Max-Age ignored.
        let c = Cookie::parse("a=1; Max-Age=60; Expires=Thu, 01 Jan 1970 00:00:00 GMT", &u).unwrap();
        assert!(!c.is_expired());
        let c = Cookie::parse("a=1; Max-Age=+60; Expires=Thu, 01 Jan 1970 00:00:01 GMT", &u).unwrap();
        assert!(c.is_expired(), "'+60' is not a valid Max-Age");
        assert!(Cookie::parse("a=1; Max-Age=-5", &u).unwrap().is_expired());
        // Lifetimes are capped at 400 days.
        let c = Cookie::parse("a=1; Expires=Fri, 31 Dec 9999 23:59:59 GMT", &u).unwrap();
        assert!(c.expires.unwrap() <= now() + MAX_COOKIE_AGE);
        let c = Cookie::parse("a=1; Max-Age=999999999999999999999", &u).unwrap();
        assert!(c.expires.unwrap() <= now() + MAX_COOKIE_AGE);
        // Path: non-absolute values fall back to the default path.
        assert_eq!(Cookie::parse("a=1; Path=rel", &u).unwrap().path, "/a");
        assert_eq!(Cookie::parse("a=1; Path=", &u).unwrap().path, "/a");
        // Domain: last one wins, leading dot ignored, public suffixes rejected.
        let c = Cookie::parse("a=1; Domain=other.org; Domain=.Example.com", &u).unwrap();
        assert_eq!((c.domain.as_str(), c.host_only), ("example.com", false));
        assert!(Cookie::parse("a=1; Domain=com", &u).is_none());
        assert!(Cookie::parse("a=1; Domain=co.uk", &url("https://shop.example.co.uk/")).is_none());
        // IP hosts: Domain must be the host itself.
        let ip = url("http://127.0.0.1:8080/");
        assert!(Cookie::parse("a=1; Domain=0.0.1", &ip).is_none());
        assert!(Cookie::parse("a=1; Domain=127.0.0.1", &ip).unwrap().host_only);
        // Prefixes are case-insensitive.
        assert!(Cookie::parse("__secure-a=1", &u).is_none());
        assert!(Cookie::parse("__HOST-a=1; Secure; Path=/a", &u).is_none());
        // SameSite=None and Partitioned need Secure; unknown SameSite is unspecified.
        assert!(Cookie::parse("a=1; SameSite=None", &u).is_none());
        assert!(Cookie::parse("a=1; Partitioned", &u).is_none());
        assert_eq!(Cookie::parse("a=1; SameSite=bogus", &u).unwrap().same_site, "");
    }

    #[test]
    fn localhost_is_a_secure_context() {
        for (u, secure) in [("http://localhost/", true), ("http://app.localhost:8080/", true), ("http://127.0.0.2/", true), ("http://[::1]/", true), ("https://x.test/", true), ("http://x.test/", false), ("http://localhost.example/", false)] {
            assert_eq!(is_secure_url(&url(u)), secure, "{u}");
        }
        let app = url("http://app.localhost:8080/");
        let mut jar = CookieJar::new();
        jar.store_from_headers(&app, &["s=1; Secure; SameSite=None".into()]);
        assert_eq!(jar.header_for(&app).as_deref(), Some("s=1"));
        assert_eq!(jar.script_cookies(&app), "s=1");
    }

    #[test]
    fn replacement_keeps_creation_order() {
        let u = url("https://www.example.com/");
        let mut jar = CookieJar::new();
        jar.store_from_headers(&u, &["a=1".into(), "b=2".into(), "c=3; Path=/x".into()]);
        jar.cookies[0].created -= 10;
        jar.store_from_headers(&u, &["a=9".into()]);
        assert_eq!(jar.header_for(&url("https://www.example.com/x/y")).unwrap(), "c=3; a=9; b=2");
        // Several Set-Cookie headers of one response, all applied in order.
        jar.store_from_headers(&u, &["b=3".into(), "b=4".into()]);
        assert_eq!(jar.header_for(&u).unwrap(), "a=9; b=4");
    }

    #[test]
    fn scripts_cannot_touch_http_only_or_secure_cookies() {
        let u = url("https://www.example.com/");
        let mut jar = CookieJar::new();
        jar.store_from_headers(&u, &["sid=1; HttpOnly; Path=/".into(), "vis=1".into()]);
        jar.store_from_script(&u, "sid=evil; path=/");
        jar.store_from_script(&u, "new=1; HttpOnly");
        jar.store_from_script(&u, "vis=2");
        assert_eq!(jar.header_for(&u).unwrap(), "sid=1; vis=2");
        assert_eq!(jar.script_cookies(&u), "vis=2");
        // Leave secure cookies alone: an insecure origin cannot shadow them.
        let mut jar = CookieJar::new();
        jar.store_from_headers(&u, &["tok=s; Secure; Domain=example.com; Path=/".into()]);
        jar.store_from_headers(&url("http://www.example.com/"), &["tok=plain; Path=/".into()]);
        assert_eq!(jar.len(), 1);
        assert_eq!(jar.cookies()[0].value, "s");
    }

    #[test]
    fn persistence_keeps_same_site_partition_and_creation() {
        let u = url("https://team.example.com/");
        let mut jar = CookieJar::new();
        let top = CookieContext::new(None, &u, true, "GET");
        jar.store_from_response(&u, &["n=1; Max-Age=999; Secure; SameSite=None; HttpOnly".into(), "s=2; Max-Age=999; SameSite=Strict".into(), "p=3; Max-Age=999; Secure; SameSite=None; Partitioned".into(), "d=4; Max-Age=999".into()], &top);
        jar.cookies[3].created -= 1000;
        let text = jar.to_netscape();
        // Partitioned cookies are invisible to tools that do not know them.
        assert!(text.lines().filter(|l| l.contains("\tp\t")).all(|l| l.starts_with('#')));
        let back = CookieJar::from_netscape(&text);
        assert_eq!(back.len(), 4);
        let get = |n: &str| back.cookies().iter().find(|c| c.name == n).unwrap().clone();
        assert_eq!(get("n").same_site, "None");
        assert!(get("n").http_only);
        assert_eq!(get("s").same_site, "Strict");
        assert_eq!(get("d").same_site, "");
        assert_eq!(get("d").created, jar.cookies()[3].created);
        assert_eq!(get("p").partition_key.as_deref(), Some("https://example.com"));
        // A cross-site POST still carries the SameSite=None cookie after a restart.
        let other = url("https://idp.other.org/");
        let ctx = CookieContext::new(Some(&other), &u, true, "POST");
        assert_eq!(back.header_for_context(&u, &ctx).unwrap(), "n=1; p=3");
        // Old seven-column files still load.
        let plain = CookieJar::from_netscape(".example.com\tTRUE\t/\tFALSE\t9999999999\tk\tv\twith\ttabs\n");
        assert_eq!(plain.cookies()[0].value, "v\twith\ttabs");
    }

    #[test]
    fn per_site_limit_evicts_oldest() {
        let u = url("https://www.example.com/");
        let mut jar = CookieJar::new();
        for i in 0..(MAX_PER_SITE + 5) {
            jar.store_from_headers(&u, &[format!("c{i}=1")]);
            if let Some(c) = jar.cookies.iter_mut().find(|c| c.name == format!("c{i}")) {
                c.created = i as u64;
            }
        }
        assert_eq!(jar.len(), MAX_PER_SITE);
        assert!(!jar.cookies().iter().any(|c| c.name == "c0"));
        assert!(jar.cookies().iter().any(|c| c.name == format!("c{}", MAX_PER_SITE + 4)));
    }

    #[test]
    fn windows_sharing_a_profile_merge_instead_of_overwriting() {
        let dir = std::env::temp_dir().join(format!("rh-cookie-sync-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("cookies.txt");
        let u = url("https://app.example.com/");
        let mut a = CookieJar::new();
        a.store_from_headers(&u, &["old=1; Max-Age=999".into(), "gone=1; Max-Age=999".into()]);
        a.sync(&path).unwrap();
        // A second window starts from the file.
        let mut b = CookieJar::load(&path).unwrap();
        a.store_from_headers(&u, &["CF_Authorization=a; Max-Age=999; Secure; SameSite=None".into(), "gone=; Max-Age=0".into()]);
        a.sync(&path).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        b.store_from_headers(&u, &["b=1; Max-Age=999".into(), "s=session".into()]);
        b.sync(&path).unwrap();
        let names = |j: &CookieJar| {
            let mut v: Vec<String> = j.cookies().iter().map(|c| c.pair()).collect();
            v.sort();
            v
        };
        assert_eq!(names(&b), ["CF_Authorization=a", "b=1", "old=1", "s=session"], "B took A's cookie and deletion");
        assert_eq!(names(&CookieJar::load(&path).unwrap()), ["CF_Authorization=a", "b=1", "old=1"], "file keeps both windows' cookies");
        a.sync(&path).unwrap();
        assert_eq!(names(&a), ["CF_Authorization=a", "b=1", "old=1"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
