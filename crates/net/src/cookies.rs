//! An HTTP cookie jar (RFC 6265): parsing `Set-Cookie`, domain/path
//! matching, expiry, and persistence in the Netscape `cookies.txt` format.

use crate::url::Url;
use std::collections::HashMap;
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
    /// Parse one `Set-Cookie` header value received from `url`.
    pub fn parse(header: &str, url: &Url) -> Option<Cookie> {
        let mut parts = header.split(';');
        let nv = parts.next()?.trim();
        let (name, value) = match nv.find('=') {
            Some(i) => (nv[..i].trim(), nv[i + 1..].trim()),
            None => return None,
        };
        if name.is_empty() {
            return None;
        }
        let value = value.trim_matches('"');
        let mut c = Cookie {
            name: name.to_string(),
            value: value.to_string(),
            domain: url.host.to_ascii_lowercase(),
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
        for attr in parts {
            let attr = attr.trim();
            let (k, v) = match attr.find('=') {
                Some(i) => (attr[..i].trim().to_ascii_lowercase(), attr[i + 1..].trim()),
                None => (attr.to_ascii_lowercase(), ""),
            };
            match k.as_str() {
                "domain" => {
                    let d = v.trim_start_matches('.').to_ascii_lowercase();
                    if d.is_empty() {
                        continue;
                    }
                    // Reject cookies for unrelated domains or public suffixes (approx: must contain a dot or be localhost).
                    if !domain_matches(&d, &url.host) {
                        return None;
                    }
                    if !d.contains('.') && d != "localhost" {
                        return None;
                    }
                    c.domain = d;
                    c.host_only = false;
                }
                "path" => {
                    if v.starts_with('/') {
                        c.path = v.to_string();
                    }
                }
                "secure" => c.secure = true,
                "httponly" => c.http_only = true,
                "max-age" => {
                    if let Ok(n) = v.parse::<i64>() {
                        max_age = Some(n);
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
        let secure_origin = url.scheme == "https" || matches!(url.host.as_str(), "localhost" | "127.0.0.1" | "[::1]");
        // Secure cookies only from secure origins; SameSite=None and
        // Partitioned need Secure; cookie prefixes (RFC 6265bis 4.1.3).
        if c.secure && !secure_origin {
            return None;
        }
        if (c.same_site == "None" || partitioned) && !c.secure {
            return None;
        }
        if c.name.starts_with("__Secure-") && !c.secure {
            return None;
        }
        if c.name.starts_with("__Host-") && (!c.secure || !c.host_only || c.path != "/") {
            return None;
        }
        if partitioned {
            // Filled in by the jar from the request context.
            c.partition_key = Some(String::new());
        }
        if let Some(n) = max_age {
            c.expires = Some(if n <= 0 { 0 } else { now() + n as u64 });
        } else if let Some(t) = expires {
            c.expires = Some(t);
        }
        Some(c)
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
}

impl CookieJar {
    pub fn new() -> CookieJar {
        CookieJar::default()
    }

    /// Store a cookie (replacing one with the same name/domain/path);
    /// expired cookies delete.
    pub fn store(&mut self, cookie: Cookie) {
        self.cookies.retain(|c| !(c.name == cookie.name && c.domain == cookie.domain && c.path == cookie.path && c.partition_key == cookie.partition_key));
        if !cookie.is_expired() {
            self.cookies.push(cookie);
        }
        self.dirty = true;
    }

    /// Apply every `Set-Cookie` header of a response from `url`.
    pub fn store_from_headers(&mut self, url: &Url, set_cookie_values: &[String]) {
        for v in set_cookie_values {
            if let Some(c) = Cookie::parse(v, url) {
                self.store(c);
            }
        }
    }

    /// Apply the `Set-Cookie` headers of a response, honouring SameSite
    /// (cross-site subresource responses cannot set Lax/Strict cookies)
    /// and keying `Partitioned` cookies by the top-level site.
    pub fn store_from_response(&mut self, url: &Url, set_cookie_values: &[String], ctx: &CookieContext) {
        let same_site = ctx.same_site_chain && ctx.is_same_site(url);
        for v in set_cookie_values {
            let Some(mut c) = Cookie::parse(v, url) else { continue };
            if !same_site && !ctx.top_level_navigation && c.same_site != "None" {
                continue;
            }
            if c.partition_key.is_some() {
                c.partition_key = Some(ctx.partition_site(url));
            }
            self.store(c);
        }
    }

    /// Cookies to send for a request to `url` made in `ctx` (SameSite
    /// rules: Strict only same-site; Lax/unspecified also on top-level
    /// safe navigations; unspecified cookies younger than two minutes
    /// also on top-level POSTs).
    pub fn header_for_context(&self, url: &Url, ctx: &CookieContext) -> Option<String> {
        let same_site = ctx.same_site_chain && ctx.is_same_site(url);
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

    fn header_filtered(&self, url: &Url, keep: impl Fn(&Cookie) -> bool) -> Option<String> {
        let host = url.host.to_ascii_lowercase();
        let path = if url.path.is_empty() { "/" } else { url.path.as_str() };
        let secure = url.scheme == "https" || matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]");
        let mut matched: Vec<&Cookie> = self
            .cookies
            .iter()
            .filter(|c| !c.is_expired())
            .filter(|c| keep(c))
            .filter(|c| if c.host_only { c.domain == host } else { domain_matches(&c.domain, &host) })
            .filter(|c| path_matches(&c.path, path))
            .filter(|c| !c.secure || secure)
            .collect();
        if matched.is_empty() {
            return None;
        }
        // Longer paths first, then creation time.
        matched.sort_by(|a, b| b.path.len().cmp(&a.path.len()).then(a.created.cmp(&b.created)));
        Some(matched.iter().map(|c| format!("{}={}", c.name, c.value)).collect::<Vec<_>>().join("; "))
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
        self.cookies.retain(|c| !domain_matches(&c.domain, &host) && !domain_matches(&host, &c.domain));
        self.dirty = true;
    }

    pub fn clear(&mut self) {
        self.cookies.clear();
        self.dirty = true;
    }

    /// Remove expired and session cookies (call on shutdown before saving
    /// if session cookies must not persist).
    pub fn purge_session(&mut self) {
        self.cookies.retain(|c| c.expires.is_some() && !c.is_expired());
    }

    /// Cookies visible to scripts for `url` (`document.cookie`): excludes HttpOnly.
    pub fn script_cookies(&self, url: &Url) -> String {
        let host = url.host.to_ascii_lowercase();
        let path = if url.path.is_empty() { "/" } else { url.path.as_str() };
        let secure = url.scheme == "https";
        self.cookies
            .iter()
            .filter(|c| !c.is_expired() && !c.http_only)
            .filter(|c| if c.host_only { c.domain == host } else { domain_matches(&c.domain, &host) })
            .filter(|c| path_matches(&c.path, path) && (!c.secure || secure))
            .map(|c| format!("{}={}", c.name, c.value))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// Serialize in Netscape `cookies.txt` format (persistent cookies only).
    pub fn to_netscape(&self) -> String {
        let mut out = String::from("# Netscape HTTP Cookie File\n# Written by RabbitHole\n\n");
        for c in &self.cookies {
            let Some(exp) = c.expires else { continue };
            if c.is_expired() || c.partition_key.is_some() {
                continue;
            }
            let domain = if c.host_only { c.domain.clone() } else { format!(".{}", c.domain) };
            let name = if c.http_only { format!("#HttpOnly_{}", domain) } else { domain };
            out.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                name,
                if c.host_only { "FALSE" } else { "TRUE" },
                c.path,
                if c.secure { "TRUE" } else { "FALSE" },
                exp,
                c.name,
                c.value
            ));
        }
        out
    }

    pub fn from_netscape(text: &str) -> CookieJar {
        let mut jar = CookieJar::new();
        for line in text.lines() {
            let (http_only, line) = match line.strip_prefix("#HttpOnly_") {
                Some(rest) => (true, rest),
                None => (false, line),
            };
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 7 {
                continue;
            }
            let host_only = f[1].eq_ignore_ascii_case("FALSE") && !f[0].starts_with('.');
            let expires = f[4].parse::<u64>().ok();
            let c = Cookie {
                name: f[5].to_string(),
                value: f[6].to_string(),
                domain: f[0].trim_start_matches('.').to_ascii_lowercase(),
                host_only,
                path: f[2].to_string(),
                secure: f[3].eq_ignore_ascii_case("TRUE"),
                http_only,
                expires,
                same_site: String::new(),
                created: now(),
                partition_key: None,
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
        std::fs::write(path, self.to_netscape())?;
        self.dirty = false;
        Ok(())
    }

    pub fn load(path: &Path) -> std::io::Result<CookieJar> {
        let text = std::fs::read_to_string(path)?;
        Ok(CookieJar::from_netscape(&text))
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
}
