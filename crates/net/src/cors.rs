//! Fetch-standard request semantics used by scripts: request modes,
//! credentials modes, redirect modes, referrer policies, CORS checks,
//! CORS preflight (with a preflight cache) and response filtering
//! (basic / cors / opaque / opaqueredirect).

use crate::url::Url;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Request mode (Fetch `request.mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestMode {
    Navigate,
    SameOrigin,
    NoCors,
    Cors,
}

impl RequestMode {
    pub fn as_str(self) -> &'static str {
        match self {
            RequestMode::Navigate => "navigate",
            RequestMode::SameOrigin => "same-origin",
            RequestMode::NoCors => "no-cors",
            RequestMode::Cors => "cors",
        }
    }

    pub fn parse(s: &str) -> Option<RequestMode> {
        Some(match s {
            "navigate" => RequestMode::Navigate,
            "same-origin" => RequestMode::SameOrigin,
            "no-cors" => RequestMode::NoCors,
            "cors" => RequestMode::Cors,
            _ => return None,
        })
    }
}

/// Credentials mode (Fetch `request.credentials`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CredentialsMode {
    Omit,
    #[default]
    SameOrigin,
    Include,
}

impl CredentialsMode {
    pub fn parse(s: &str) -> Option<CredentialsMode> {
        Some(match s {
            "omit" => CredentialsMode::Omit,
            "same-origin" => CredentialsMode::SameOrigin,
            "include" => CredentialsMode::Include,
            _ => return None,
        })
    }
}

/// Redirect mode (Fetch `request.redirect`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RedirectMode {
    #[default]
    Follow,
    Error,
    Manual,
}

impl RedirectMode {
    pub fn parse(s: &str) -> Option<RedirectMode> {
        Some(match s {
            "follow" => RedirectMode::Follow,
            "error" => RedirectMode::Error,
            "manual" => RedirectMode::Manual,
            _ => return None,
        })
    }
}

/// Response type (Fetch `response.type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseType {
    Basic,
    Cors,
    Default,
    Opaque,
    OpaqueRedirect,
}

impl ResponseType {
    pub fn as_str(self) -> &'static str {
        match self {
            ResponseType::Basic => "basic",
            ResponseType::Cors => "cors",
            ResponseType::Default => "default",
            ResponseType::Opaque => "opaque",
            ResponseType::OpaqueRedirect => "opaqueredirect",
        }
    }
}

/// Referrer policy (W3C Referrer Policy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReferrerPolicy {
    NoReferrer,
    NoReferrerWhenDowngrade,
    Origin,
    OriginWhenCrossOrigin,
    SameOrigin,
    StrictOrigin,
    /// The default in modern browsers.
    #[default]
    StrictOriginWhenCrossOrigin,
    UnsafeUrl,
}

impl ReferrerPolicy {
    /// Parse a policy token; for a `Referrer-Policy` header with several
    /// comma-separated tokens the last recognized one wins.
    pub fn parse(s: &str) -> Option<ReferrerPolicy> {
        s.split(',').filter_map(|t| ReferrerPolicy::parse_one(t.trim())).last()
    }

    fn parse_one(s: &str) -> Option<ReferrerPolicy> {
        Some(match s.to_ascii_lowercase().as_str() {
            "no-referrer" => ReferrerPolicy::NoReferrer,
            "no-referrer-when-downgrade" => ReferrerPolicy::NoReferrerWhenDowngrade,
            "origin" => ReferrerPolicy::Origin,
            "origin-when-cross-origin" => ReferrerPolicy::OriginWhenCrossOrigin,
            "same-origin" => ReferrerPolicy::SameOrigin,
            "strict-origin" => ReferrerPolicy::StrictOrigin,
            "strict-origin-when-cross-origin" => ReferrerPolicy::StrictOriginWhenCrossOrigin,
            "unsafe-url" => ReferrerPolicy::UnsafeUrl,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ReferrerPolicy::NoReferrer => "no-referrer",
            ReferrerPolicy::NoReferrerWhenDowngrade => "no-referrer-when-downgrade",
            ReferrerPolicy::Origin => "origin",
            ReferrerPolicy::OriginWhenCrossOrigin => "origin-when-cross-origin",
            ReferrerPolicy::SameOrigin => "same-origin",
            ReferrerPolicy::StrictOrigin => "strict-origin",
            ReferrerPolicy::StrictOriginWhenCrossOrigin => "strict-origin-when-cross-origin",
            ReferrerPolicy::UnsafeUrl => "unsafe-url",
        }
    }
}

fn is_downgrade(from: &Url, to: &Url) -> bool {
    from.scheme == "https" && to.scheme != "https"
}

/// The `Referer` value for a request to `target` from `referrer` under
/// `policy` (None = no header).
pub fn referrer_for(referrer: &Url, target: &Url, policy: ReferrerPolicy) -> Option<String> {
    if !referrer.is_http() {
        return None;
    }
    let full = {
        let s = strip_for_referrer(referrer);
        // Browsers strip the referrer to its origin above 4 KB.
        if s.len() > 4096 {
            format!("{}/", referrer.origin())
        } else {
            s
        }
    };
    let origin = format!("{}/", referrer.origin());
    let same = referrer.origin() == target.origin();
    match policy {
        ReferrerPolicy::NoReferrer => None,
        ReferrerPolicy::UnsafeUrl => Some(full),
        ReferrerPolicy::Origin => Some(origin),
        ReferrerPolicy::NoReferrerWhenDowngrade => (!is_downgrade(referrer, target)).then_some(full),
        ReferrerPolicy::OriginWhenCrossOrigin => Some(if same { full } else { origin }),
        ReferrerPolicy::SameOrigin => same.then_some(full),
        ReferrerPolicy::StrictOrigin => (!is_downgrade(referrer, target)).then_some(origin),
        ReferrerPolicy::StrictOriginWhenCrossOrigin => {
            if same {
                Some(full)
            } else if is_downgrade(referrer, target) {
                None
            } else {
                Some(origin)
            }
        }
    }
}

/// Value of the `Origin` header for a non-CORS request with an unsafe
/// method (Fetch "append a request Origin header"), or None.
pub fn origin_for_unsafe_request(origin: &Url, target: &Url, policy: ReferrerPolicy) -> String {
    let null = match policy {
        ReferrerPolicy::NoReferrer => true,
        ReferrerPolicy::NoReferrerWhenDowngrade | ReferrerPolicy::StrictOrigin | ReferrerPolicy::StrictOriginWhenCrossOrigin => is_downgrade(origin, target),
        ReferrerPolicy::SameOrigin => origin.origin() != target.origin(),
        _ => false,
    };
    if null {
        "null".into()
    } else {
        origin.origin()
    }
}

/// CORS-safelisted method.
pub fn is_simple_method(m: &str) -> bool {
    matches!(m, "GET" | "HEAD" | "POST")
}

/// Forbidden request headers scripts may not set.
pub fn is_forbidden_request_header(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    matches!(
        n.as_str(),
        "accept-charset"
            | "accept-encoding"
            | "access-control-request-headers"
            | "access-control-request-method"
            | "connection"
            | "content-length"
            | "cookie"
            | "cookie2"
            | "date"
            | "dnt"
            | "expect"
            | "host"
            | "keep-alive"
            | "origin"
            | "referer"
            | "set-cookie"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "via"
    ) || n.starts_with("proxy-")
        || n.starts_with("sec-")
}

/// CORS-safelisted request header (name + value check).
pub fn is_safelisted_request_header(name: &str, value: &str) -> bool {
    if value.len() > 128 {
        return false;
    }
    let unsafe_byte = |b: u8| (b < 0x20 && b != b'\t') || matches!(b, b'"' | b'(' | b')' | b':' | b'<' | b'>' | b'?' | b'@' | b'[' | b'\\' | b']' | b'{' | b'}' | 0x7f);
    match name.to_ascii_lowercase().as_str() {
        "accept" => !value.bytes().any(unsafe_byte),
        "accept-language" | "content-language" => value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'*' | b',' | b'-' | b'.' | b';' | b'=')),
        "content-type" => {
            if value.bytes().any(unsafe_byte) {
                return false;
            }
            let mime = value.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
            matches!(mime.as_str(), "application/x-www-form-urlencoded" | "multipart/form-data" | "text/plain")
        }
        "range" => {
            let v = value.trim();
            v.strip_prefix("bytes=").map(|r| !r.contains(',') && r.contains('-')).unwrap_or(false)
        }
        _ => false,
    }
}

/// Names of the (author) request headers that are not safelisted, sorted
/// and lower-cased, for `Access-Control-Request-Headers`.
pub fn unsafe_request_header_names(headers: &[(String, String)]) -> Vec<String> {
    let mut names: Vec<String> = headers.iter().filter(|(k, v)| !is_safelisted_request_header(k, v)).map(|(k, _)| k.to_ascii_lowercase()).collect();
    names.sort();
    names.dedup();
    names
}

/// CORS check of a response (Fetch 4.9): `Access-Control-Allow-Origin`
/// must match the request origin (`*` only without credentials), and
/// `Access-Control-Allow-Credentials: true` is needed with credentials.
pub fn cors_check(resp_headers: &[(String, String)], origin: &str, include_credentials: bool) -> Result<(), String> {
    let get = |n: &str| resp_headers.iter().find(|(k, _)| k == n).map(|(_, v)| v.trim());
    let acao = get("access-control-allow-origin").ok_or_else(|| "No 'Access-Control-Allow-Origin' header is present on the requested resource".to_string())?;
    if acao == "*" && !include_credentials {
        return Ok(());
    }
    if acao != origin {
        return Err(format!("The 'Access-Control-Allow-Origin' header has a value '{acao}' that is not equal to the supplied origin '{origin}'"));
    }
    if include_credentials && get("access-control-allow-credentials") != Some("true") {
        return Err("The value of the 'Access-Control-Allow-Credentials' header in the response is not 'true' while the request's credentials mode is 'include'".into());
    }
    Ok(())
}

fn split_list(v: Option<&str>) -> Vec<String> {
    v.map(|v| v.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect()).unwrap_or_default()
}

/// Result of a successful preflight, cached per (origin, URL, credentials).
#[derive(Debug, Clone)]
pub struct PreflightResult {
    pub methods: Vec<String>,
    pub headers: Vec<String>,
    pub any_method: bool,
    pub any_header: bool,
    pub expires: Instant,
}

impl PreflightResult {
    pub fn allows(&self, method: &str, unsafe_headers: &[String], credentials: bool) -> Result<(), String> {
        if !is_simple_method(method) && !self.methods.iter().any(|m| m == method) && !(self.any_method && !credentials) {
            return Err(format!("Method {method} is not allowed by Access-Control-Allow-Methods in preflight response"));
        }
        for h in unsafe_headers {
            let listed = self.headers.iter().any(|a| a.eq_ignore_ascii_case(h));
            // `*` never covers Authorization.
            let wildcard = self.any_header && !credentials && h != "authorization";
            if !listed && !wildcard {
                return Err(format!("Request header field {h} is not allowed by Access-Control-Allow-Headers in preflight response"));
            }
        }
        Ok(())
    }
}

/// Validate a preflight response.
pub fn check_preflight(status: u16, resp_headers: &[(String, String)], origin: &str, credentials: bool, method: &str, unsafe_headers: &[String]) -> Result<PreflightResult, String> {
    if !(200..300).contains(&status) {
        return Err(format!("Response to preflight request doesn't pass access control check: HTTP status {status}"));
    }
    cors_check(resp_headers, origin, credentials)?;
    let get = |n: &str| resp_headers.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str());
    let methods = split_list(get("access-control-allow-methods"));
    let headers = split_list(get("access-control-allow-headers"));
    let max_age = get("access-control-max-age").and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(5).min(7200);
    let r = PreflightResult {
        any_method: methods.iter().any(|m| m == "*"),
        any_header: headers.iter().any(|h| h == "*"),
        methods,
        headers,
        expires: Instant::now() + Duration::from_secs(max_age),
    };
    r.allows(method, unsafe_headers, credentials)?;
    Ok(r)
}

/// Preflight cache.
#[derive(Debug, Default)]
pub struct PreflightCache {
    map: Mutex<HashMap<(String, String, bool), PreflightResult>>,
}

impl PreflightCache {
    pub fn get(&self, origin: &str, url: &Url, credentials: bool) -> Option<PreflightResult> {
        let key = (origin.to_string(), url.without_fragment().to_string(), credentials);
        let mut m = self.map.lock().unwrap();
        match m.get(&key) {
            Some(r) if r.expires > Instant::now() => Some(r.clone()),
            Some(_) => {
                m.remove(&key);
                None
            }
            None => None,
        }
    }

    pub fn put(&self, origin: &str, url: &Url, credentials: bool, r: PreflightResult) {
        let mut m = self.map.lock().unwrap();
        if m.len() > 1000 {
            let now = Instant::now();
            m.retain(|_, v| v.expires > now);
        }
        m.insert((origin.to_string(), url.without_fragment().to_string(), credentials), r);
    }
}

/// CORS-safelisted response header names.
const SAFELISTED_RESPONSE: &[&str] = &["cache-control", "content-language", "content-length", "content-type", "expires", "last-modified", "pragma"];

/// Headers visible to script for a `cors` response.
pub fn filter_cors_headers(headers: &[(String, String)], include_credentials: bool) -> Vec<(String, String)> {
    let exposed = split_list(headers.iter().find(|(k, _)| k == "access-control-expose-headers").map(|(_, v)| v.as_str()));
    let all = exposed.iter().any(|e| e == "*") && !include_credentials;
    headers
        .iter()
        .filter(|(k, _)| k != "set-cookie" && k != "set-cookie2")
        .filter(|(k, _)| all || SAFELISTED_RESPONSE.contains(&k.as_str()) || exposed.iter().any(|e| e.eq_ignore_ascii_case(k)))
        .cloned()
        .collect()
}

/// Headers visible for a `basic` response (everything but Set-Cookie).
pub fn filter_basic_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    headers.iter().filter(|(k, _)| k != "set-cookie" && k != "set-cookie2").cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse_absolute(s).unwrap()
    }

    fn hv(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn referrer_policies() {
        let from = u("https://a.com/page?q=1#frag");
        let same = u("https://a.com/x");
        let cross = u("https://b.com/x");
        let down = u("http://b.com/x");
        let p = ReferrerPolicy::default();
        assert_eq!(referrer_for(&from, &same, p).unwrap(), "https://a.com/page?q=1");
        assert_eq!(referrer_for(&from, &cross, p).unwrap(), "https://a.com/");
        assert!(referrer_for(&from, &down, p).is_none());
        assert!(referrer_for(&from, &same, ReferrerPolicy::NoReferrer).is_none());
        assert_eq!(referrer_for(&from, &down, ReferrerPolicy::UnsafeUrl).unwrap(), "https://a.com/page?q=1");
        assert!(referrer_for(&from, &cross, ReferrerPolicy::SameOrigin).is_none());
        assert_eq!(referrer_for(&from, &cross, ReferrerPolicy::NoReferrerWhenDowngrade).unwrap(), "https://a.com/page?q=1");
        assert_eq!(ReferrerPolicy::parse("unknown, origin, bogus"), Some(ReferrerPolicy::Origin));
        assert_eq!(origin_for_unsafe_request(&from, &down, p), "null");
        assert_eq!(origin_for_unsafe_request(&from, &cross, p), "https://a.com");
    }

    #[test]
    fn safelist() {
        assert!(is_safelisted_request_header("Content-Type", "text/plain;charset=UTF-8"));
        assert!(!is_safelisted_request_header("content-type", "application/json"));
        assert!(is_safelisted_request_header("accept", "*/*"));
        assert!(!is_safelisted_request_header("x-custom", "1"));
        assert!(is_safelisted_request_header("range", "bytes=0-100"));
        assert_eq!(unsafe_request_header_names(&hv(&[("X-B", "1"), ("accept", "a"), ("Content-Type", "application/json")])), vec!["content-type", "x-b"]);
        assert!(is_forbidden_request_header("Sec-Fetch-Mode"));
        assert!(is_forbidden_request_header("Cookie"));
        assert!(!is_forbidden_request_header("X-Requested-With"));
    }

    #[test]
    fn cors_checks() {
        let o = "https://a.com";
        assert!(cors_check(&hv(&[("access-control-allow-origin", "*")]), o, false).is_ok());
        assert!(cors_check(&hv(&[("access-control-allow-origin", "*")]), o, true).is_err());
        assert!(cors_check(&hv(&[("access-control-allow-origin", "https://a.com")]), o, true).is_err());
        assert!(cors_check(&hv(&[("access-control-allow-origin", "https://a.com"), ("access-control-allow-credentials", "true")]), o, true).is_ok());
        assert!(cors_check(&hv(&[("access-control-allow-origin", "https://b.com")]), o, false).is_err());
        assert!(cors_check(&[], o, false).is_err());
        let pre = hv(&[("access-control-allow-origin", "*"), ("access-control-allow-methods", "PUT, DELETE"), ("access-control-allow-headers", "X-A, content-type")]);
        let r = check_preflight(204, &pre, o, false, "PUT", &["content-type".into(), "x-a".into()]).unwrap();
        assert!(r.allows("PATCH", &[], false).is_err());
        assert!(r.allows("GET", &["x-b".into()], false).is_err());
        let star = hv(&[("access-control-allow-origin", "*"), ("access-control-allow-methods", "*"), ("access-control-allow-headers", "*")]);
        let r = check_preflight(200, &star, o, false, "PATCH", &["x-z".into()]).unwrap();
        assert!(r.allows("PATCH", &["authorization".into()], false).is_err());
        assert!(check_preflight(500, &star, o, false, "PATCH", &[]).is_err());
        let h = hv(&[("content-type", "a"), ("x-secret", "1"), ("x-pub", "2"), ("set-cookie", "a=b"), ("access-control-expose-headers", "X-Pub")]);
        let f = filter_cors_headers(&h, false);
        assert!(f.iter().any(|(k, _)| k == "content-type") && f.iter().any(|(k, _)| k == "x-pub"));
        assert!(!f.iter().any(|(k, _)| k == "x-secret" || k == "set-cookie"));
    }
}

/// A referrer URL as sent: no fragment and no credentials (Referrer
/// Policy "strip url for use as a referrer").
fn strip_for_referrer(u: &Url) -> String {
    Url { username: String::new(), password: String::new(), fragment: None, ..u.clone() }.to_string()
}
