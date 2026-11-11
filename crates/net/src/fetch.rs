//! Resource fetching: the request pipeline (redirects, cookies,
//! referrer policy, Sec-Fetch-*, CORS for scripts) on top of the HTTP
//! cache (`cache`) and the connection layer (`client`: HTTP/2 + HTTP/1.1).

use crate::cache::{self, platform_cache_dir, CacheEntry, CacheMode, HttpCache, Lookup};
use crate::client::{Client, ClientError, Priority, WireRequest};
use crate::cookies::{CookieContext, CookieJar};
use crate::cors::{self, CredentialsMode, PreflightCache, RedirectMode, ReferrerPolicy, RequestMode, ResponseType};
use crate::url::{percent_decode, percent_encode_component, site_of, Url};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Response {
    /// Final URL after redirects.
    pub url: Url,
    pub status: u16,
    /// Lower-cased MIME type without parameters, e.g. `text/html`.
    pub content_type: String,
    /// Charset parameter from Content-Type, lower-cased.
    pub charset: Option<String>,
    pub body: Vec<u8>,
    /// All response headers (lower-cased names) of the final response.
    pub headers: Vec<(String, String)>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers.iter().find(|(k, _)| *k == name).map(|(_, v)| v.as_str())
    }

    /// File name suggested by `Content-Disposition`, if any.
    pub fn suggested_filename(&self) -> Option<String> {
        let cd = self.header("content-disposition")?;
        for part in cd.split(';') {
            let part = part.trim();
            if let Some(v) = part.strip_prefix("filename*=") {
                // RFC 5987: charset'lang'value
                let v = v.rsplit('\'').next().unwrap_or(v);
                return Some(crate::url::percent_decode(v));
            }
            if let Some(v) = part.strip_prefix("filename=") {
                return Some(v.trim_matches('"').to_string());
            }
        }
        None
    }
}

/// A response whose body is still being read.
pub struct StreamedResponse {
    pub url: Url,
    pub status: u16,
    pub content_type: String,
    pub charset: Option<String>,
    pub headers: Vec<(String, String)>,
    pub content_length: Option<u64>,
    pub reader: Box<dyn Read + Send>,
}

impl StreamedResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers.iter().find(|(k, _)| *k == name).map(|(_, v)| v.as_str())
    }

    /// Read the whole body (up to `limit` bytes) into a `Response`.
    pub fn into_response(mut self, limit: usize) -> Result<Response, FetchError> {
        let body = read_limited(&mut self.reader, limit)?;
        Ok(Response { url: self.url, status: self.status, content_type: self.content_type, charset: self.charset, body, headers: self.headers })
    }

    /// Stream the body to `dest`, reporting progress; returns false from
    /// `progress` to cancel (the partial file is removed).
    pub fn save_to(mut self, dest: &std::path::Path, mut progress: impl FnMut(u64, Option<u64>) -> bool) -> Result<u64, FetchError> {
        use std::io::Write;
        let mut file = std::fs::File::create(dest).map_err(|e| FetchError::Io(e.to_string()))?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut done: u64 = 0;
        loop {
            let n = self.reader.read(&mut buf).map_err(|e| FetchError::Io(e.to_string()))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(|e| FetchError::Io(e.to_string()))?;
            done += n as u64;
            if !progress(done, self.content_length) {
                drop(file);
                let _ = std::fs::remove_file(dest);
                return Err(FetchError::Other("cancelled".into()));
            }
        }
        Ok(done)
    }

    /// File name suggested by `Content-Disposition` or the URL path.
    pub fn suggested_filename(&self) -> String {
        if let Some(cd) = self.header("content-disposition") {
            for part in cd.split(';') {
                let part = part.trim();
                if let Some(v) = part.strip_prefix("filename*=") {
                    let v = v.rsplit('\'').next().unwrap_or(v);
                    let name = crate::url::percent_decode(v);
                    if !name.trim().is_empty() {
                        return sanitize_filename(&name);
                    }
                }
                if let Some(v) = part.strip_prefix("filename=") {
                    let name = v.trim_matches('"');
                    if !name.trim().is_empty() {
                        return sanitize_filename(name);
                    }
                }
            }
        }
        let last = self.url.path.rsplit('/').next().unwrap_or("");
        let name = crate::url::percent_decode(last);
        if name.trim().is_empty() {
            let ext = match self.content_type.as_str() {
                "application/pdf" => ".pdf",
                "application/zip" => ".zip",
                "image/png" => ".png",
                "image/jpeg" => ".jpg",
                _ => "",
            };
            format!("download{ext}")
        } else {
            sanitize_filename(&name)
        }
    }
}

/// Strip path separators and control characters from a file name.
pub fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c == '/' || c == '\\' || c == ':' || c == '*' || c == '?' || c == '"' || c == '<' || c == '>' || c == '|' || c.is_control() { '_' } else { c })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.').to_string();
    if trimmed.is_empty() { "download".into() } else { trimmed }
}

/// HTTP request body for POST/PUT.
#[derive(Debug, Clone)]
pub struct Body {
    pub content_type: String,
    pub data: Vec<u8>,
}

impl Body {
    /// `application/x-www-form-urlencoded` body from (name, value) pairs.
    pub fn form(fields: &[(String, String)]) -> Body {
        let data = fields
            .iter()
            .map(|(k, v)| format!("{}={}", form_encode(k), form_encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        Body { content_type: "application/x-www-form-urlencoded".into(), data: data.into_bytes() }
    }

    /// `multipart/form-data` body; `files` are (field name, file name, mime, bytes).
    pub fn multipart(fields: &[(String, String)], files: &[(String, String, String, Vec<u8>)]) -> Body {
        let boundary = format!("----RabbitHoleBoundary{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        let mut data = Vec::new();
        for (k, v) in fields {
            data.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"\r\n\r\n", k.replace('"', "%22")).as_bytes());
            data.extend_from_slice(v.as_bytes());
            data.extend_from_slice(b"\r\n");
        }
        for (k, name, mime, bytes) in files {
            data.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
                    k.replace('"', "%22"),
                    name.replace('"', "%22"),
                    if mime.is_empty() { "application/octet-stream" } else { mime }
                )
                .as_bytes(),
            );
            data.extend_from_slice(bytes);
            data.extend_from_slice(b"\r\n");
        }
        data.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        Body { content_type: format!("multipart/form-data; boundary={boundary}"), data }
    }

    pub fn text(content_type: &str, text: &str) -> Body {
        Body { content_type: content_type.into(), data: text.as_bytes().to_vec() }
    }
}

/// `application/x-www-form-urlencoded` encoding of one component
/// (spaces become `+`).
pub fn form_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// What a request is for (Fetch `destination`): selects the browser-like
/// `Accept` and `Sec-Fetch-*` headers and the default priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    /// Top-level navigation (typed URL, link, form).
    Document,
    Image,
    Style,
    Script,
    Font,
    /// `fetch()` / `XMLHttpRequest`.
    Empty,
}

impl Destination {
    /// Default network priority, as in Chrome: document > CSS > (sync)
    /// scripts > fonts > fetch/XHR > images.
    pub fn priority(self) -> Priority {
        match self {
            Destination::Document => Priority::Highest,
            Destination::Style => Priority::Highest,
            Destination::Script => Priority::High,
            Destination::Font => Priority::High,
            Destination::Empty => Priority::Medium,
            Destination::Image => Priority::Low,
        }
    }

    /// Sort key for scheduling within one priority class.
    fn rank(self) -> u8 {
        match self {
            Destination::Document => 0,
            Destination::Style => 1,
            Destination::Script => 2,
            Destination::Font => 3,
            Destination::Empty => 4,
            Destination::Image => 5,
        }
    }
}

/// A request: method, URL, optional body, extra headers and referer.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub url: Url,
    pub body: Option<Body>,
    pub headers: Vec<(String, String)>,
    /// The document that initiated the request (its URL is the source of
    /// the `Referer` header, filtered by `referrer_policy`, and of the
    /// request origin).
    pub referer: Option<Url>,
    /// Follow redirects (default true). `false` returns 3xx responses as
    /// they are (same as `redirect = Manual`).
    pub follow_redirects: bool,
    pub dest: Destination,
    /// Fetch-standard mode. `None` = a browser-internal load (no CORS
    /// enforcement, full response); `Some(..)` = script semantics (see
    /// `Fetcher::fetch_api`).
    pub mode: Option<RequestMode>,
    pub credentials: CredentialsMode,
    pub redirect: RedirectMode,
    pub referrer_policy: ReferrerPolicy,
    pub cache: CacheMode,
    /// Override the destination's default priority.
    pub priority: Option<Priority>,
}

impl Request {
    pub fn new(method: &str, url: Url) -> Request {
        Request {
            method: method.to_string(),
            url,
            body: None,
            headers: Vec::new(),
            referer: None,
            follow_redirects: true,
            dest: Destination::Document,
            mode: None,
            credentials: CredentialsMode::Include,
            redirect: RedirectMode::Follow,
            referrer_policy: ReferrerPolicy::default(),
            cache: CacheMode::Default,
            priority: None,
        }
    }
    pub fn get(url: Url) -> Request {
        Request::new("GET", url)
    }
    pub fn post(url: Url, body: Body) -> Request {
        let mut r = Request::new("POST", url);
        r.body = Some(body);
        r
    }
    pub fn with_dest(mut self, d: Destination) -> Request {
        self.dest = d;
        self
    }
    pub fn with_referer(mut self, r: Url) -> Request {
        self.referer = Some(r);
        self
    }
    pub fn with_header(mut self, k: &str, v: &str) -> Request {
        self.headers.push((k.into(), v.into()));
        self
    }
    /// Script (Fetch API) semantics with this mode and credentials.
    pub fn with_mode(mut self, mode: RequestMode, credentials: CredentialsMode) -> Request {
        self.mode = Some(mode);
        self.credentials = credentials;
        self
    }
    pub fn with_redirect(mut self, r: RedirectMode) -> Request {
        self.redirect = r;
        self
    }
    pub fn with_referrer_policy(mut self, p: ReferrerPolicy) -> Request {
        self.referrer_policy = p;
        self
    }
    pub fn with_cache_mode(mut self, c: CacheMode) -> Request {
        self.cache = c;
        self
    }
    pub fn with_priority(mut self, p: Priority) -> Request {
        self.priority = Some(p);
        self
    }
    /// Effective priority.
    pub fn effective_priority(&self) -> Priority {
        self.priority.unwrap_or_else(|| self.dest.priority())
    }
}

#[derive(Debug, Clone)]
pub enum FetchError {
    /// Blocked by the built-in ad/tracker blocker.
    Blocked(String),
    UnsupportedScheme(String),
    Io(String),
    Http(u16),
    Timeout,
    TooLarge,
    /// Blocked by the CORS / same-origin rules (a network error to
    /// scripts).
    Cors(String),
    /// The site's robots.txt disallows the URL for our bot user agent.
    Robots(String),
    Other(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Blocked(h) => write!(f, "blocked ad/tracker host: {h}"),
            FetchError::UnsupportedScheme(s) => write!(f, "unsupported URL scheme: {s}"),
            FetchError::Io(e) => write!(f, "I/O error: {e}"),
            FetchError::Http(code) => write!(f, "HTTP error {code}"),
            FetchError::Timeout => write!(f, "request timed out"),
            FetchError::TooLarge => write!(f, "response body too large"),
            FetchError::Cors(e) => write!(f, "CORS error: {e}"),
            FetchError::Robots(u) => write!(f, "blocked by robots.txt: {u} is disallowed for RabbitHoleBot (pass browser headers / disable robots to override)"),
            FetchError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for FetchError {}

impl From<ClientError> for FetchError {
    fn from(e: ClientError) -> Self {
        match e {
            ClientError::Timeout => FetchError::Timeout,
            ClientError::Io(m) => FetchError::Io(m),
        }
    }
}

/// A response to a Fetch-API request, with its type and redirect info.
#[derive(Debug, Clone)]
pub struct ApiResponse {
    /// Filtered per `kind`: opaque responses have status 0, no headers
    /// and no body; cors responses only expose safelisted/exposed
    /// headers.
    pub response: Response,
    pub kind: ResponseType,
    /// More than one URL was involved.
    pub redirected: bool,
    pub url_list: Vec<Url>,
}

/// Shared network state: connection pools, HTTP cache, CORS preflight
/// cache. One per browser profile; `Fetcher`s clone the `Arc`.
pub struct NetContext {
    pub client: Arc<Client>,
    pub cache: Option<Arc<HttpCache>>,
    pub preflight: PreflightCache,
    /// Parsed robots.txt per origin (bot profile).
    pub robots: crate::robots::RobotsCache,
    revalidating: Mutex<HashSet<String>>,
}

impl std::fmt::Debug for NetContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NetContext({:?})", self.cache)
    }
}

impl NetContext {
    /// A fresh context with the given cache.
    pub fn new(cache: Option<Arc<HttpCache>>) -> NetContext {
        NetContext { client: Arc::new(Client::new(6)), cache, preflight: PreflightCache::default(), robots: Default::default(), revalidating: Mutex::new(HashSet::new()) }
    }

    /// The process-wide default context: memory cache (64 MB) backed by
    /// the platform cache directory (256 MB), unless `RH_DISK_CACHE=0`
    /// (memory only) or `RH_HTTP_CACHE=0` (no cache).
    pub fn shared() -> Arc<NetContext> {
        static CTX: OnceLock<Arc<NetContext>> = OnceLock::new();
        CTX.get_or_init(|| {
            let off = |n: &str| std::env::var(n).map(|v| v == "0" || v.eq_ignore_ascii_case("off")).unwrap_or(false);
            let cache = if off("RH_HTTP_CACHE") {
                None
            } else if off("RH_DISK_CACHE") {
                Some(Arc::new(HttpCache::memory(64 << 20)))
            } else {
                match platform_cache_dir() {
                    Some(dir) => Some(Arc::new(HttpCache::with_disk(64 << 20, &dir, 256 << 20))),
                    None => Some(Arc::new(HttpCache::memory(64 << 20))),
                }
            };
            Arc::new(NetContext::new(cache))
        })
        .clone()
    }
}

#[derive(Debug, Clone)]
pub struct Fetcher {
    pub user_agent: String,
    pub timeout: Duration,
    pub max_body: usize,
    /// Extra request headers (name, value).
    pub headers: Vec<(String, String)>,
    /// Shared cookie jar (None = cookies disabled).
    pub cookies: Option<Arc<Mutex<CookieJar>>>,
    /// `Accept-Language` value.
    pub accept_language: String,
    /// Block requests to known ad/tracker domains (subresources only).
    pub block_trackers: bool,
    /// Connection pools and caches (shared by clones).
    pub net: Arc<NetContext>,
    /// Which identity the request headers present (see [`HeaderProfile`]).
    pub header_profile: HeaderProfile,
    /// Check robots.txt before top-level navigations (bot mode).
    pub respect_robots: bool,
    /// Optional request log (agent interface: network listing, waits).
    pub log: Option<Arc<crate::netlog::NetLog>>,
    /// Called after every HTTP request (devtools network view).
    pub observer: Option<NetObserver>,
}

/// One finished HTTP request, for a network log.
#[derive(Debug, Clone)]
pub struct NetEvent {
    pub method: String,
    pub url: Url,
    pub dest: Destination,
    /// None when the request failed.
    pub status: Option<u16>,
    pub content_type: String,
    /// Announced body length (Content-Length), when known.
    pub size: Option<u64>,
    /// Time to the response headers.
    pub duration: Duration,
    pub error: Option<String>,
}

/// Callback receiving [`NetEvent`]s (shared by fetcher clones).
#[derive(Clone)]
pub struct NetObserver(pub Arc<dyn Fn(&NetEvent) + Send + Sync>);

impl std::fmt::Debug for NetObserver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NetObserver")
    }
}


/// The identity a [`Fetcher`] presents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HeaderProfile {
    /// Chrome-like headers (User-Agent, `sec-ch-ua*` client hints): the GUI.
    #[default]
    Browser,
    /// An honest bot identity: [`bot_user_agent`] and no client hints
    /// (MCP server, CLI, programmatic use).
    Bot,
}

/// The User-Agent sent in [`HeaderProfile::Bot`] mode.
pub fn bot_user_agent() -> String {
    let v = env!("CARGO_PKG_VERSION");
    format!("Mozilla/5.0 (compatible; RabbitHoleBot/{v}; +https://github.com/i234dev/RabbitHoleBrowser) RabbitHole/{v}")
}

impl Default for Fetcher {
    fn default() -> Self {
        Fetcher {
            user_agent: default_user_agent().into(),
            timeout: Duration::from_secs(15),
            max_body: 32 * 1024 * 1024,
            headers: Vec::new(),
            cookies: None,
            accept_language: "es-ES,es;q=0.9,en;q=0.8".into(),
            block_trackers: true,
            net: NetContext::shared(),
            header_profile: HeaderProfile::Browser,
            respect_robots: false,
            log: None,
            observer: None,
        }
    }
}

impl Fetcher {
    /// A fetcher with its own connection pool and a memory-only cache
    /// (tests, private windows).
    pub fn isolated() -> Fetcher {
        Fetcher { net: Arc::new(NetContext::new(Some(Arc::new(HttpCache::memory(32 << 20))))), ..Fetcher::default() }
    }

    /// Replace the network context (e.g. a custom cache directory).
    pub fn with_context(mut self, net: Arc<NetContext>) -> Fetcher {
        self.net = net;
        self
    }

    /// Switch identity: [`HeaderProfile::Bot`] also sets the bot
    /// User-Agent and turns robots.txt checks on; `Browser` restores the
    /// Chrome-like User-Agent and turns them off.
    pub fn set_header_profile(&mut self, profile: HeaderProfile) {
        self.header_profile = profile;
        match profile {
            HeaderProfile::Bot => {
                self.user_agent = bot_user_agent();
                self.respect_robots = true;
            }
            HeaderProfile::Browser => {
                self.user_agent = default_user_agent().into();
                self.respect_robots = false;
            }
        }
    }

    /// Fail top-level navigations that the origin's robots.txt disallows
    /// for `RabbitHoleBot` (fetched once per origin and cached).
    fn check_robots(&self, req: &Request) -> Result<(), FetchError> {
        let top_level = req.dest == Destination::Document && req.mode.is_none_or(|m| m == RequestMode::Navigate);
        if !self.respect_robots || !top_level || !req.url.is_http() || req.url.path == "/robots.txt" {
            return Ok(());
        }
        let origin = req.url.origin();
        let robots = match self.net.robots.get(&origin) {
            Some(r) => r,
            None => {
                let mut f = self.clone();
                f.respect_robots = false;
                f.log = None;
                f.timeout = self.timeout.min(Duration::from_secs(8));
                let parsed = Url::parse_absolute(&format!("{origin}/robots.txt"))
                    .and_then(|u| f.request(&Request::get(u).with_dest(Destination::Empty)).ok())
                    .filter(|r| r.status == 200)
                    .map(|r| crate::robots::Robots::parse(&crate::charset::decode_text(&r.body, r.charset.as_deref()), crate::robots::BOT_TOKEN))
                    .unwrap_or_default();
                self.net.robots.put(&origin, parsed.clone());
                parsed
            }
        };
        let path = match &req.url.query {
            Some(q) => format!("{}?{}", req.url.path, q),
            None => req.url.path.clone(),
        };
        if robots.allows(&path) {
            Ok(())
        } else {
            Err(FetchError::Robots(req.url.to_string()))
        }
    }

    /// Counters (protocols, connections, cache hits).
    pub fn stats(&self) -> String {
        self.net.client.stats.snapshot()
    }

    /// Speak HTTP/2 with prior knowledge to a plain-http origin
    /// (`http://host:port`). For tests against local h2c servers.
    pub fn set_h2_prior_knowledge(&self, origin: &str) {
        let u = Url::parse_absolute(origin).expect("origin URL");
        let key = format!("{}://{}:{}", u.scheme, u.host.to_ascii_lowercase(), u.port.or(u.default_port()).unwrap_or(80));
        self.net.client.h2c.lock().unwrap().insert(key);
    }

    /// Drop pooled connections (the cache is kept).
    pub fn close_connections(&self) {
        self.net.client.clear();
    }
}


/// Split a Content-Type header into (lower-cased mime, lower-cased charset).
pub fn parse_content_type(header: &str) -> (String, Option<String>) {
    let mut parts = header.split(';');
    let mime = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let charset = parts.find_map(|p| {
        let p = p.trim();
        let (k, v) = p.split_once('=')?;
        if k.trim().eq_ignore_ascii_case("charset") {
            Some(v.trim().trim_matches(|c| c == '"' || c == '\'').to_ascii_lowercase())
        } else {
            None
        }
    });
    (mime, charset)
}

/// MIME type guessed from a file extension.
pub fn mime_for_extension(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "html" | "htm" | "xhtml" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "svg" => "image/svg+xml",
        "txt" | "md" | "markdown" | "rs" | "toml" | "log" => "text/plain",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// Lenient base64 decoder (standard and URL-safe alphabets, optional
/// padding, whitespace ignored). Returns None on an invalid character.
pub fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for c in input.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            c if c.is_ascii_whitespace() => continue,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

fn read_limited<R: Read>(mut reader: R, max: usize) -> Result<Vec<u8>, FetchError> {
    let mut body = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| {
            if e.kind() == std::io::ErrorKind::TimedOut || e.kind() == std::io::ErrorKind::WouldBlock {
                FetchError::Timeout
            } else {
                FetchError::Io(e.to_string())
            }
        })?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n]);
        if body.len() > max {
            return Err(FetchError::TooLarge);
        }
    }
    Ok(body)
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Wrap a body reader with the decoders for its `Content-Encoding`
/// (gzip, deflate (zlib or raw), br, zstd; applied in reverse order).
pub fn decode_content(headers: &[(String, String)], body: Box<dyn Read + Send>) -> Box<dyn Read + Send> {
    let Some(ce) = headers.iter().find(|(k, _)| k == "content-encoding").map(|(_, v)| v.to_ascii_lowercase()) else {
        return body;
    };
    let mut r = body;
    for coding in ce.split(',').map(|c| c.trim().to_string()).rev() {
        r = match coding.as_str() {
            "gzip" | "x-gzip" | "deflate" | "br" | "zstd" => Box::new(LazyDecoder { coding, inner: Some(r), dec: None }),
            _ => r, // identity / unknown: pass through
        };
    }
    r
}

/// Defers building the decoder until the first read, so empty bodies
/// (204, HEAD, redirects) labelled with a coding read as empty.
struct LazyDecoder {
    coding: String,
    inner: Option<Box<dyn Read + Send>>,
    dec: Option<Box<dyn Read + Send>>,
}

impl Read for LazyDecoder {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.dec.is_none() {
            let Some(inner) = self.inner.take() else { return Ok(0) };
            let mut br = std::io::BufReader::with_capacity(16 * 1024, inner);
            let first = {
                use std::io::BufRead;
                let b = br.fill_buf()?;
                if b.is_empty() {
                    return Ok(0);
                }
                b.to_vec()
            };
            self.dec = Some(match self.coding.as_str() {
                "gzip" | "x-gzip" => Box::new(flate2::read::MultiGzDecoder::new(br)),
                "deflate" => {
                    // RFC says zlib; some servers send raw deflate.
                    let zlib = first.len() >= 2 && first[0] & 0x0f == 8 && ((first[0] as u16) << 8 | first[1] as u16) % 31 == 0;
                    if zlib {
                        Box::new(flate2::read::ZlibDecoder::new(br))
                    } else {
                        Box::new(flate2::read::DeflateDecoder::new(br))
                    }
                }
                "br" => Box::new(brotli_decompressor::Decompressor::new(br, 64 * 1024)),
                "zstd" => match ruzstd::decoding::StreamingDecoder::new(br) {
                    Ok(d) => Box::new(d),
                    Err(e) => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("zstd: {e}"))),
                },
                _ => Box::new(br),
            });
        }
        self.dec.as_mut().unwrap().read(buf)
    }
}

/// Copies the body into the HTTP cache once it was read completely.
struct CacheTee {
    inner: Box<dyn Read + Send>,
    buf: Option<Vec<u8>>,
    cache: Arc<HttpCache>,
    key: String,
    status: u16,
    req_headers: Vec<(String, String)>,
    resp_headers: Vec<(String, String)>,
    times: (u64, u64),
}

impl Read for CacheTee {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        match self.inner.read(out) {
            Ok(0) => {
                if let Some(body) = self.buf.take() {
                    self.cache.store(&self.key, "GET", self.status, &self.req_headers, &self.resp_headers, body, self.times.0, self.times.1);
                }
                Ok(0)
            }
            Ok(n) => {
                if let Some(b) = &mut self.buf {
                    if b.len() + n > self.cache.max_entry {
                        self.buf = None;
                    } else {
                        b.extend_from_slice(&out[..n]);
                    }
                }
                Ok(n)
            }
            Err(e) => {
                self.buf = None;
                Err(e)
            }
        }
    }
}

/// One HTTP exchange (after cache handling).
struct Hop {
    status: u16,
    headers: Vec<(String, String)>,
    body: Box<dyn Read + Send>,
    content_length: Option<u64>,
}

/// Outcome of the redirect loop.
struct Outcome {
    url: Url,
    status: u16,
    headers: Vec<(String, String)>,
    body: Box<dyn Read + Send>,
    content_length: Option<u64>,
    kind: ResponseType,
    url_list: Vec<Url>,
}

fn has_header(h: &[(String, String)], name: &str) -> bool {
    h.iter().any(|(k, _)| k.eq_ignore_ascii_case(name))
}

/// Combine Sec-Fetch-Site values along a redirect chain (worst wins).
fn worse_site(a: &'static str, b: &'static str) -> &'static str {
    let rank = |s: &str| match s {
        "none" => 0,
        "same-origin" => 1,
        "same-site" => 2,
        _ => 3,
    };
    if rank(b) > rank(a) {
        b
    } else {
        a
    }
}

fn cors_disabled() -> bool {
    static OFF: OnceLock<bool> = OnceLock::new();
    *OFF.get_or_init(|| std::env::var("RH_NO_CORS").map(|v| v != "0").unwrap_or(false))
}

fn debug_net() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RH_DEBUG_NET").is_some())
}

impl Fetcher {
    /// Run a request, reporting it to the observer.
    fn run_observed(&self, req: &Request) -> Result<Outcome, FetchError> {
        let Some(obs) = &self.observer else { return self.run(req) };
        let start = std::time::Instant::now();
        let out = self.run(req);
        let ev = match &out {
            Ok(o) => NetEvent {
                method: req.method.to_ascii_uppercase(),
                url: o.url.clone(),
                dest: req.dest,
                status: Some(o.status),
                content_type: o.headers.iter().find(|(k, _)| k == "content-type").map(|(_, v)| v.clone()).unwrap_or_default(),
                size: o.content_length,
                duration: start.elapsed(),
                error: None,
            },
            Err(e) => NetEvent {
                method: req.method.to_ascii_uppercase(),
                url: req.url.clone(),
                dest: req.dest,
                status: None,
                content_type: String::new(),
                size: None,
                duration: start.elapsed(),
                error: Some(e.to_string()),
            },
        };
        (obs.0)(&ev);
        out
    }

    /// Fetch any supported URL: http/https (redirects followed, content
    /// codings decoded, HTTP cache), file, data (base64 or
    /// percent-encoded), about.
    pub fn fetch(&self, url: &Url) -> Result<Response, FetchError> {
        match url.scheme.as_str() {
            "http" | "https" => self.request(&Request::get(url.clone())),
            "file" => self.fetch_file(url),
            "data" => fetch_data(url),
            "about" => {
                if url.path == "blank" || url.path.is_empty() {
                    Ok(Response {
                        url: url.clone(),
                        status: 200,
                        content_type: "text/html".into(),
                        charset: Some("utf-8".into()),
                        body: Vec::new(),
                        headers: Vec::new(),
                    })
                } else {
                    Err(FetchError::UnsupportedScheme(url.to_string()))
                }
            }
            other => Err(FetchError::UnsupportedScheme(other.to_string())),
        }
    }

    /// Perform an arbitrary request (GET/POST/...): redirects (303 and
    /// 301/302 on POST become GET, as in browsers), cookies (SameSite
    /// aware), referrer policy, HTTP cache, HTTP/2 when available.
    pub fn request(&self, req: &Request) -> Result<Response, FetchError> {
        if !req.url.is_http() {
            return self.fetch(&req.url);
        }
        self.check_robots(req)?;
        let Some(log) = self.log.clone() else { return self.request_inner(req) };
        let id = log.begin(req);
        let r = self.request_inner(req);
        match &r {
            Ok(resp) => log.finish(id, resp.status, &resp.content_type, Some(resp.body.len() as u64), Some(&resp.body)),
            Err(e) => log.fail(id, &e.to_string()),
        }
        r
    }

    fn request_inner(&self, req: &Request) -> Result<Response, FetchError> {
        let out = self.run_observed(req)?;
        let api = req.mode.is_some();
        let (content_type, charset) = parse_content_type(out.headers.iter().find(|(k, _)| k == "content-type").map(|(_, v)| v.as_str()).unwrap_or(""));
        let body = if out.kind == ResponseType::Opaque || out.kind == ResponseType::OpaqueRedirect {
            Vec::new()
        } else {
            read_limited(out.body, self.max_body)?
        };
        if !api && out.status >= 400 && body.is_empty() {
            return Err(FetchError::Http(out.status));
        }
        Ok(Response { url: out.url, status: out.status, content_type, charset, body, headers: out.headers })
    }

    /// Fetch-API request (`fetch()` / `XMLHttpRequest`): `req.mode`
    /// (default `cors`) and `req.credentials` apply; cross-origin
    /// requests need CORS approval (with a preflight for non-simple
    /// requests), `no-cors` cross-origin responses are opaque, redirect
    /// modes are honoured. Errors are network errors for scripts.
    pub fn fetch_api(&self, req: &Request) -> Result<ApiResponse, FetchError> {
        let mut req = req.clone();
        if req.mode.is_none() {
            req.mode = Some(RequestMode::Cors);
            req.credentials = CredentialsMode::SameOrigin;
        }
        if !req.url.is_http() {
            if req.url.scheme == "data" || req.url.scheme == "about" || req.url.scheme == "file" {
                let r = self.fetch(&req.url)?;
                return Ok(ApiResponse { url_list: vec![r.url.clone()], response: r, kind: ResponseType::Basic, redirected: false });
            }
            return Err(FetchError::UnsupportedScheme(req.url.scheme.clone()));
        }
        let Some(log) = self.log.clone() else { return self.fetch_api_inner(&req) };
        let id = log.begin(&req);
        let r = self.fetch_api_inner(&req);
        match &r {
            Ok(a) => log.finish(id, a.response.status, &a.response.content_type, Some(a.response.body.len() as u64), Some(&a.response.body)),
            Err(e) => log.fail(id, &e.to_string()),
        }
        r
    }

    fn fetch_api_inner(&self, req: &Request) -> Result<ApiResponse, FetchError> {
        let out = self.run_observed(req)?;
        let (content_type, charset) = parse_content_type(out.headers.iter().find(|(k, _)| k == "content-type").map(|(_, v)| v.as_str()).unwrap_or(""));
        let opaque = matches!(out.kind, ResponseType::Opaque | ResponseType::OpaqueRedirect);
        let body = if opaque { Vec::new() } else { read_limited(out.body, self.max_body)? };
        let redirected = out.url_list.len() > 1;
        Ok(ApiResponse {
            response: Response { url: out.url, status: out.status, content_type, charset, body, headers: out.headers },
            kind: out.kind,
            redirected,
            url_list: out.url_list,
        })
    }

    /// Start a request and return the response headers plus a streaming
    /// body reader (for downloads of arbitrary size). Redirects, cookies
    /// and the cache are handled like `request`.
    pub fn request_stream(&self, req: &Request) -> Result<StreamedResponse, FetchError> {
        if !req.url.is_http() {
            let r = self.fetch(&req.url)?;
            let len = r.body.len() as u64;
            return Ok(StreamedResponse {
                url: r.url,
                status: r.status,
                content_type: r.content_type,
                charset: r.charset,
                headers: r.headers,
                content_length: Some(len),
                reader: Box::new(std::io::Cursor::new(r.body)),
            });
        }
        self.check_robots(req)?;
        let id = self.log.as_ref().map(|l| l.begin(req));
        let out = match self.run_observed(req) {
            Ok(o) => o,
            Err(e) => {
                if let (Some(l), Some(id)) = (&self.log, id) {
                    l.fail(id, &e.to_string());
                }
                return Err(e);
            }
        };
        let (content_type, charset) = parse_content_type(out.headers.iter().find(|(k, _)| k == "content-type").map(|(_, v)| v.as_str()).unwrap_or(""));
        if let (Some(l), Some(id)) = (&self.log, id) {
            l.finish(id, out.status, &content_type, out.content_length, None);
        }
        Ok(StreamedResponse { url: out.url, status: out.status, content_type, charset, headers: out.headers, content_length: out.content_length, reader: out.body })
    }

    /// Stream a URL to a file, reporting progress as (bytes so far, total
    /// if known). Returns the final response metadata (headers, no body).
    /// Returns Err(FetchError::Other("cancelled")) when `progress` returns false.
    pub fn download(&self, req: &Request, dest: &std::path::Path, mut progress: impl FnMut(u64, Option<u64>) -> bool) -> Result<Response, FetchError> {
        if !req.url.is_http() {
            let resp = self.fetch(&req.url)?;
            std::fs::write(dest, &resp.body).map_err(|e| FetchError::Io(e.to_string()))?;
            progress(resp.body.len() as u64, Some(resp.body.len() as u64));
            return Ok(Response { body: Vec::new(), ..resp });
        }
        let mut req = req.clone();
        req.cache = CacheMode::NoStore;
        let stream = self.request_stream(&req)?;
        if stream.status >= 400 {
            return Err(FetchError::Http(stream.status));
        }
        let meta = Response {
            url: stream.url.clone(),
            status: stream.status,
            content_type: stream.content_type.clone(),
            charset: stream.charset.clone(),
            body: Vec::new(),
            headers: stream.headers.clone(),
        };
        stream.save_to(dest, &mut progress)?;
        Ok(meta)
    }

    /// Build the request headers in Chrome's order. `origin_header` is the
    /// `Origin` value to send, if any.
    #[allow(clippy::too_many_arguments)]
    fn build_headers(&self, req: &Request, _method: &str, _url: &Url, site: &'static str, origin_header: Option<String>, referer: Option<String>, body: Option<&Body>, cookie: Option<String>) -> Vec<(String, String)> {
        let mut h: Vec<(String, String)> = Vec::with_capacity(20);
        let mode = req.mode.map(|m| m.as_str()).unwrap_or(default_mode(req.dest));
        if self.header_profile == HeaderProfile::Browser {
            let brand = format!("\"Chromium\";v=\"{CHROME_MAJOR}\", \"Google Chrome\";v=\"{CHROME_MAJOR}\", \"Not_A Brand\";v=\"99\"");
            h.push(("sec-ch-ua".into(), brand));
            h.push(("sec-ch-ua-mobile".into(), "?0".into()));
            h.push(("sec-ch-ua-platform".into(), platform_name().into()));
        }
        if req.dest == Destination::Document {
            h.push(("Upgrade-Insecure-Requests".into(), "1".into()));
        }
        if let Some(o) = origin_header {
            h.push(("Origin".into(), o));
        }
        if let Some(b) = body {
            h.push(("Content-Type".into(), b.content_type.clone()));
        }
        h.push(("User-Agent".into(), self.user_agent.clone()));
        h.push(("Accept".into(), default_accept(req.dest).into()));
        h.push(("Sec-Fetch-Site".into(), site.into()));
        h.push(("Sec-Fetch-Mode".into(), mode.into()));
        if req.dest == Destination::Document && mode == "navigate" {
            h.push(("Sec-Fetch-User".into(), "?1".into()));
        }
        h.push(("Sec-Fetch-Dest".into(), dest_name(req.dest).into()));
        if let Some(r) = referer {
            h.push(("Referer".into(), r));
        }
        h.push(("Accept-Encoding".into(), "gzip, deflate, br, zstd".into()));
        h.push(("Accept-Language".into(), self.accept_language.clone()));
        if let Some(c) = cookie {
            h.push(("Cookie".into(), c));
        }
        // Fetcher-level then request-level headers override defaults.
        for (k, v) in self.headers.iter().chain(req.headers.iter()) {
            if k.eq_ignore_ascii_case("content-type") && body.is_none() {
                continue;
            }
            if req.mode.is_some() && cors::is_forbidden_request_header(k) {
                continue;
            }
            match h.iter_mut().find(|(hk, _)| hk.eq_ignore_ascii_case(k)) {
                Some(slot) => slot.1 = v.clone(),
                None => h.push((k.clone(), v.clone())),
            }
        }
        h
    }

    /// One exchange with the HTTP cache in front of the network.
    #[allow(clippy::too_many_arguments)]
    fn hop(&self, method: &str, url: &Url, headers: &[(String, String)], body: Option<&Body>, priority: Priority, incremental: bool, mode: CacheMode, allow_swr: bool) -> Result<Hop, FetchError> {
        let key = url.without_fragment().to_string();
        let user_conditional = ["if-none-match", "if-modified-since", "if-match", "if-unmodified-since", "if-range", "range"].iter().any(|n| has_header(headers, n));
        let cache = self.net.cache.clone().filter(|_| method == "GET" && !user_conditional && mode != CacheMode::NoStore);
        let mut send_headers = headers.to_vec();
        let mut stale: Option<Arc<CacheEntry>> = None;
        if let Some(c) = &cache {
            match c.lookup(&key, headers, mode) {
                Lookup::Fresh(e) => {
                    self.net.client.stats.cache_hits.fetch_add(1, Ordering::Relaxed);
                    if debug_net() {
                        eprintln!("[net] cache hit {key}");
                    }
                    return Ok(hop_from_entry(&e));
                }
                Lookup::StaleWhileRevalidate(e) if allow_swr => {
                    self.net.client.stats.cache_hits.fetch_add(1, Ordering::Relaxed);
                    self.revalidate_in_background(url, headers, priority);
                    return Ok(hop_from_entry(&e));
                }
                Lookup::StaleWhileRevalidate(e) => {
                    send_headers.extend(e.validators());
                    stale = Some(e);
                }
                Lookup::Revalidate(e, validators) => {
                    send_headers.extend(validators);
                    stale = Some(e);
                }
                Lookup::Miss => {
                    if mode == CacheMode::OnlyIfCached {
                        return Err(FetchError::Other("only-if-cached: not in cache".into()));
                    }
                    self.net.client.stats.cache_misses.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        let t_req = cache::now_secs();
        let wire = WireRequest { method, url, headers: &send_headers, body: body.map(|b| b.data.as_slice()), priority, incremental };
        let t0 = std::time::Instant::now();
        let resp = self.net.client.send(&wire, self.timeout)?;
        let t_resp = cache::now_secs();
        if debug_net() {
            eprintln!("[net] {} {} {} -> {} in {:?}", resp.protocol, method, url, resp.status, t0.elapsed());
        }
        if let (Some(c), Some(entry), 304) = (&cache, &stale, resp.status) {
            self.net.client.stats.cache_revalidated.fetch_add(1, Ordering::Relaxed);
            let mut b = resp.body;
            let _ = std::io::copy(&mut b, &mut std::io::sink());
            let fresh = c.freshen(entry, &resp.headers, t_req, t_resp);
            return Ok(hop_from_entry(&fresh));
        }
        // Unsafe methods invalidate stored responses for the URL.
        if !matches!(method, "GET" | "HEAD" | "OPTIONS" | "TRACE") && resp.status < 400 {
            if let Some(c) = &self.net.cache {
                c.invalidate(&key);
                for h in ["location", "content-location"] {
                    if let Some(u) = resp.headers.iter().find(|(k, _)| k == h).and_then(|(_, v)| url.join(v.trim())) {
                        if u.origin() == url.origin() {
                            c.invalidate(&u.without_fragment().to_string());
                        }
                    }
                }
            }
        }
        let encoded = resp.headers.iter().any(|(k, v)| k == "content-encoding" && !v.trim().eq_ignore_ascii_case("identity"));
        let content_length = if encoded { None } else { resp.headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.trim().parse::<u64>().ok()) };
        let decoded = if method == "HEAD" { resp.body } else { decode_content(&resp.headers, resp.body) };
        let body: Box<dyn Read + Send> = match &cache {
            Some(c) if cache::storable(method, resp.status, headers, &resp.headers) => Box::new(CacheTee {
                inner: decoded,
                buf: Some(Vec::new()),
                cache: c.clone(),
                key,
                status: resp.status,
                req_headers: headers.to_vec(),
                resp_headers: resp.headers.clone(),
                times: (t_req, t_resp),
            }),
            _ => decoded,
        };
        Ok(Hop { status: resp.status, headers: resp.headers, body, content_length })
    }

    fn revalidate_in_background(&self, url: &Url, headers: &[(String, String)], priority: Priority) {
        let key = url.without_fragment().to_string();
        if !self.net.revalidating.lock().unwrap().insert(key.clone()) {
            return;
        }
        let me = self.clone();
        let url = url.clone();
        let headers = headers.to_vec();
        std::thread::spawn(move || {
            if let Ok(mut h) = me.hop("GET", &url, &headers, None, priority.min(Priority::Low), false, CacheMode::Default, false) {
                let _ = std::io::copy(&mut h.body, &mut std::io::sink());
            }
            me.net.revalidating.lock().unwrap().remove(&key);
        });
    }

    /// CORS preflight for a non-simple cross-origin request.
    fn preflight(&self, req: &Request, method: &str, url: &Url, origin: &str, include_creds: bool, unsafe_headers: &[String]) -> Result<(), FetchError> {
        if let Some(p) = self.net.preflight.get(origin, url, include_creds) {
            if p.allows(method, unsafe_headers, include_creds).is_ok() {
                return Ok(());
            }
        }
        let mut h: Vec<(String, String)> = vec![
            ("Accept".into(), "*/*".into()),
            ("Access-Control-Request-Method".into(), method.to_string()),
        ];
        if !unsafe_headers.is_empty() {
            h.push(("Access-Control-Request-Headers".into(), unsafe_headers.join(",")));
        }
        h.push(("Origin".into(), origin.to_string()));
        h.push(("User-Agent".into(), self.user_agent.clone()));
        h.push(("Sec-Fetch-Mode".into(), "cors".into()));
        h.push(("Sec-Fetch-Site".into(), fetch_site(req.referer.as_ref(), url).into()));
        h.push(("Sec-Fetch-Dest".into(), "empty".into()));
        if let Some(r) = req.referer.as_ref().and_then(|r| cors::referrer_for(r, url, req.referrer_policy)) {
            h.push(("Referer".into(), r));
        }
        h.push(("Accept-Encoding".into(), "gzip, deflate, br, zstd".into()));
        h.push(("Accept-Language".into(), self.accept_language.clone()));
        let mut hop = self.hop("OPTIONS", url, &h, None, Priority::Highest, false, CacheMode::NoStore, false)?;
        let _ = std::io::copy(&mut hop.body, &mut std::io::sink());
        let r = cors::check_preflight(hop.status, &hop.headers, origin, include_creds, method, unsafe_headers).map_err(FetchError::Cors)?;
        self.net.preflight.put(origin, url, include_creds, r);
        Ok(())
    }

    /// The redirect loop with cookies, referrer policy, Sec-Fetch-* and
    /// (for `req.mode = Some(..)`) the Fetch standard's CORS rules.
    fn run(&self, req: &Request) -> Result<Outcome, FetchError> {
        if self.block_trackers && req.dest != Destination::Document && is_tracker_host(&req.url.host) {
            return Err(FetchError::Blocked(req.url.host.clone()));
        }
        let api = req.mode.is_some_and(|m| m != RequestMode::Navigate) && !cors_disabled();
        let mode = req.mode.unwrap_or(RequestMode::Navigate);
        let client_origin: Option<Url> = req.referer.clone().filter(|r| r.is_http());
        let origin_str = client_origin.as_ref().map(|o| o.origin());
        let priority = req.effective_priority();
        let incremental = matches!(req.dest, Destination::Image | Destination::Document);
        let mut method = req.method.to_ascii_uppercase();
        let mut url = req.url.without_fragment();
        let mut body = req.body.clone();
        let mut policy = req.referrer_policy;
        let mut url_list = vec![url.clone()];
        let mut tainting = ResponseType::Basic;
        let mut tainted_origin = false;
        let mut site: &'static str = fetch_site(req.referer.as_ref(), &url);
        let top_level = req.dest == Destination::Document && req.mode.is_none_or(|m| m == RequestMode::Navigate);
        let mut cookie_ctx = CookieContext::new(req.referer.as_ref(), &url, top_level, &method);
        let author_headers: Vec<(String, String)> = req.headers.iter().filter(|(k, _)| !(req.mode.is_some() && cors::is_forbidden_request_header(k))).cloned().collect();
        for _hop in 0..20 {
            let same_origin = origin_str.as_ref().map(|o| *o == url.origin()).unwrap_or(true);
            if api {
                if tainting == ResponseType::Basic && same_origin && !tainted_origin {
                    // basic
                } else {
                    match mode {
                        RequestMode::SameOrigin => return Err(FetchError::Cors(format!("{url} is cross-origin and the request mode is same-origin"))),
                        RequestMode::NoCors => {
                            if req.redirect != RedirectMode::Follow {
                                return Err(FetchError::Cors("no-cors requests must follow redirects".into()));
                            }
                            if !cors::is_simple_method(&method) {
                                return Err(FetchError::Cors(format!("method {method} is not allowed in no-cors mode")));
                            }
                            tainting = ResponseType::Opaque;
                        }
                        _ => tainting = ResponseType::Cors,
                    }
                }
            }
            let include_creds = match (api, req.credentials) {
                (false, _) => true,
                (true, CredentialsMode::Include) => true,
                (true, CredentialsMode::SameOrigin) => tainting == ResponseType::Basic,
                (true, CredentialsMode::Omit) => false,
            };
            let origin_value = if tainted_origin { "null".to_string() } else { origin_str.clone().unwrap_or_else(|| "null".into()) };
            if tainting == ResponseType::Cors {
                let mut probe = author_headers.clone();
                if let Some(b) = &body {
                    probe.push(("content-type".into(), b.content_type.clone()));
                }
                let unsafe_headers = cors::unsafe_request_header_names(&probe);
                if !cors::is_simple_method(&method) || !unsafe_headers.is_empty() {
                    self.preflight(req, &method, &url, &origin_value, req.credentials == CredentialsMode::Include, &unsafe_headers)?;
                }
            }
            // Origin header: CORS requests, and unsafe methods.
            let origin_header = if client_origin.is_none() {
                None
            } else if tainting == ResponseType::Cors {
                Some(origin_value.clone())
            } else if !matches!(method.as_str(), "GET" | "HEAD") && (api || req.dest == Destination::Document || req.dest == Destination::Empty) {
                Some(if tainted_origin { "null".into() } else { cors::origin_for_unsafe_request(client_origin.as_ref().unwrap(), &url, policy) })
            } else if !api && req.dest == Destination::Font && !same_origin {
                // Fonts are CORS requests in browsers.
                Some(origin_value.clone())
            } else {
                None
            };
            let referer = req.referer.as_ref().and_then(|r| cors::referrer_for(r, &url, policy));
            let cookie = if include_creds {
                self.cookies.as_ref().and_then(|jar| jar.lock().unwrap().header_for_context(&url, &cookie_ctx))
            } else {
                None
            };
            let headers = self.build_headers(req, &method, &url, site, origin_header, referer, body.as_ref(), cookie);
            let mut hop = self.hop(&method, &url, &headers, body.as_ref(), priority, incremental, req.cache, true)?;
            if include_creds {
                if let Some(jar) = &self.cookies {
                    let set: Vec<String> = hop.headers.iter().filter(|(k, _)| k == "set-cookie").map(|(_, v)| v.clone()).collect();
                    if !set.is_empty() {
                        jar.lock().unwrap().store_from_response(&url, &set, &cookie_ctx);
                    }
                }
            }
            if tainting == ResponseType::Cors {
                cors::cors_check(&hop.headers, &origin_value, req.credentials == CredentialsMode::Include).map_err(FetchError::Cors)?;
            }
            let is_redirect = matches!(hop.status, 301 | 302 | 303 | 307 | 308);
            let location = hop.headers.iter().find(|(k, _)| k == "location").map(|(_, v)| v.trim().to_string());
            if let (true, Some(loc)) = (is_redirect, location) {
                let redirect_mode = if !req.follow_redirects { RedirectMode::Manual } else { req.redirect };
                match redirect_mode {
                    RedirectMode::Error => return Err(FetchError::Other(format!("redirect to {loc} with redirect mode 'error'"))),
                    RedirectMode::Manual => {
                        let kind = if api { ResponseType::OpaqueRedirect } else { ResponseType::Basic };
                        let (status, headers) = if kind == ResponseType::OpaqueRedirect { (0, Vec::new()) } else { (hop.status, hop.headers) };
                        return Ok(Outcome { url, status, headers, body: hop.body, content_length: hop.content_length, kind, url_list });
                    }
                    RedirectMode::Follow => {}
                }
                if let Some(next) = url.join(&loc) {
                    if next.is_http() {
                        // Drain the redirect body so the connection is reused.
                        let _ = read_limited(&mut hop.body, 256 * 1024);
                        if hop.status == 303 && method != "GET" && method != "HEAD" || ((hop.status == 301 || hop.status == 302) && method == "POST") {
                            method = "GET".into();
                            body = None;
                        }
                        if let Some(p) = hop.headers.iter().find(|(k, _)| k == "referrer-policy").and_then(|(_, v)| ReferrerPolicy::parse(v)) {
                            policy = p;
                        }
                        let next = next.without_fragment();
                        if api && tainting == ResponseType::Cors && next.origin() != url.origin() && !same_origin {
                            tainted_origin = true;
                        }
                        site = worse_site(site, fetch_site(req.referer.as_ref(), &next));
                        cookie_ctx.same_site_chain = cookie_ctx.same_site_chain && cookie_ctx.is_same_site(&next);
                        cookie_ctx.safe_method = matches!(method.as_str(), "GET" | "HEAD");
                        url = next;
                        url_list.push(url.clone());
                        continue;
                    }
                }
            }
            let (status, headers, body, kind) = match tainting {
                ResponseType::Opaque => (0, Vec::new(), Box::new(std::io::empty()) as Box<dyn Read + Send>, ResponseType::Opaque),
                ResponseType::Cors => (hop.status, cors::filter_cors_headers(&hop.headers, req.credentials == CredentialsMode::Include), hop.body, ResponseType::Cors),
                _ if api => (hop.status, cors::filter_basic_headers(&hop.headers), hop.body, ResponseType::Basic),
                _ => (hop.status, hop.headers, hop.body, ResponseType::Basic),
            };
            return Ok(Outcome { url, status, headers, body, content_length: hop.content_length, kind, url_list });
        }
        Err(FetchError::Other("too many redirects".into()))
    }

    /// Like `fetch_many`, for subresources of a page: browser headers for
    /// `dest`, with `initiator` as referer/initiator.
    /// The same URL listed several times (a font shared by several
    /// `@font-face` rules, an image used twice…) is fetched once.
    pub fn fetch_many_for(&self, urls: &[Url], parallelism: usize, dest: Destination, initiator: Option<&Url>) -> Vec<Result<Response, FetchError>> {
        let mut unique: Vec<&Url> = Vec::with_capacity(urls.len());
        let mut index: Vec<usize> = Vec::with_capacity(urls.len());
        let mut seen: HashMap<String, usize> = HashMap::new();
        for u in urls {
            let i = *seen.entry(u.to_string()).or_insert_with(|| {
                unique.push(u);
                unique.len() - 1
            });
            index.push(i);
        }
        let reqs: Vec<Request> = unique
            .iter()
            .map(|u| {
                let mut r = Request::get((*u).clone()).with_dest(dest);
                r.referer = initiator.cloned();
                r
            })
            .collect();
        let results = self.request_many(&reqs, parallelism);
        if unique.len() == urls.len() {
            return results;
        }
        index.into_iter().map(|i| results[i].clone()).collect()
    }

    /// Perform several requests concurrently, preserving result order.
    /// Requests are started by priority (document > CSS > scripts >
    /// fonts > fetch > images); per-host connection limits and HTTP/2
    /// multiplexing are handled by the connection pool, so `parallelism`
    /// is only a lower bound on the number of workers.
    pub fn request_many(&self, reqs: &[Request], parallelism: usize) -> Vec<Result<Response, FetchError>> {
        if reqs.is_empty() {
            return Vec::new();
        }
        let mut order: Vec<usize> = (0..reqs.len()).collect();
        order.sort_by_key(|&i| (std::cmp::Reverse(reqs[i].effective_priority()), reqs[i].dest.rank()));
        let workers = parallelism.max(1).max(reqs.len().min(24)).min(reqs.len());
        let results: Mutex<Vec<Option<Result<Response, FetchError>>>> = Mutex::new((0..reqs.len()).map(|_| None).collect());
        let next = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| loop {
                    let k = next.fetch_add(1, Ordering::SeqCst);
                    if k >= order.len() {
                        break;
                    }
                    let i = order[k];
                    let r = self.request(&reqs[i]);
                    results.lock().unwrap()[i] = Some(r);
                });
            }
        });
        results
            .into_inner()
            .unwrap()
            .into_iter()
            .map(|r| r.unwrap_or_else(|| Err(FetchError::Other("not fetched".into()))))
            .collect()
    }

    /// Start a request on a background thread; the result arrives on the
    /// returned channel.
    pub fn request_async(&self, req: Request) -> std::sync::mpsc::Receiver<Result<Response, FetchError>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let me = self.clone();
        std::thread::spawn(move || {
            let _ = tx.send(me.request(&req));
        });
        rx
    }

    /// Fetch-API request on a background thread.
    pub fn fetch_api_async(&self, req: Request) -> std::sync::mpsc::Receiver<Result<ApiResponse, FetchError>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let me = self.clone();
        std::thread::spawn(move || {
            let _ = tx.send(me.fetch_api(&req));
        });
        rx
    }

    pub fn fetch_many(&self, urls: &[Url], parallelism: usize) -> Vec<Result<Response, FetchError>> {
        let parallelism = parallelism.max(1).min(urls.len().max(1));
        let results: Mutex<Vec<Option<Result<Response, FetchError>>>> = Mutex::new((0..urls.len()).map(|_| None).collect());
        let next = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..parallelism {
                scope.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= urls.len() {
                        break;
                    }
                    let r = self.fetch(&urls[i]);
                    results.lock().unwrap()[i] = Some(r);
                });
            }
        });
        results
            .into_inner()
            .unwrap()
            .into_iter()
            .map(|r| r.unwrap_or_else(|| Err(FetchError::Other("not fetched".into()))))
            .collect()
    }
}

fn hop_from_entry(e: &CacheEntry) -> Hop {
    let len = e.body.len() as u64;
    Hop { status: e.status, headers: e.headers.clone(), body: Box::new(std::io::Cursor::new(ArcBytes(e.body.clone()))), content_length: Some(len) }
}

/// `AsRef<[u8]>` over shared bytes (cache bodies are not copied).
struct ArcBytes(Arc<Vec<u8>>);

impl AsRef<[u8]> for ArcBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

fn default_mode(dest: Destination) -> &'static str {
    match dest {
        Destination::Document => "navigate",
        Destination::Image | Destination::Style | Destination::Script => "no-cors",
        Destination::Font | Destination::Empty => "cors",
    }
}

fn default_accept(dest: Destination) -> &'static str {
    match dest {
        Destination::Document => "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7",
        Destination::Image => "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8",
        Destination::Style => "text/css,*/*;q=0.1",
        Destination::Script | Destination::Font | Destination::Empty => "*/*",
    }
}

fn dest_name(dest: Destination) -> &'static str {
    match dest {
        Destination::Document => "document",
        Destination::Image => "image",
        Destination::Style => "style",
        Destination::Script => "script",
        Destination::Font => "font",
        Destination::Empty => "empty",
    }
}


impl Fetcher {
    fn fetch_file(&self, url: &Url) -> Result<Response, FetchError> {
        let path = url.file_path().ok_or_else(|| FetchError::Other("invalid file URL".into()))?;
        let meta = std::fs::metadata(&path).map_err(|e| FetchError::Io(format!("{}: {e}", path.display())))?;
        if meta.is_dir() {
            let mut entries: Vec<(String, bool)> = std::fs::read_dir(&path)
                .map_err(|e| FetchError::Io(e.to_string()))?
                .filter_map(|e| e.ok())
                .map(|e| {
                    let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    (e.file_name().to_string_lossy().into_owned(), is_dir)
                })
                .collect();
            entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
            let base = if url.path.ends_with('/') { url.path.clone() } else { format!("{}/", url.path) };
            let mut html = format!(
                "<!doctype html><html><head><meta charset=utf-8><title>Index of {0}</title></head><body><h1>Index of {0}</h1><ul>",
                html_escape(&percent_decode(&url.path))
            );
            if path.parent().is_some() && url.path != "/" {
                html.push_str("<li><a href=\"../\">../</a></li>");
            }
            for (name, is_dir) in entries {
                let suffix = if is_dir { "/" } else { "" };
                let href = format!("{}{}{}", base, percent_encode_component(&name), suffix);
                html.push_str(&format!(
                    "<li><a href=\"{}\">{}{}</a></li>",
                    html_escape(&href),
                    html_escape(&name),
                    suffix
                ));
            }
            html.push_str("</ul></body></html>");
            return Ok(Response {
                url: url.clone(),
                status: 200,
                content_type: "text/html".into(),
                charset: Some("utf-8".into()),
                body: html.into_bytes(),
                headers: Vec::new(),
            });
        }
        if meta.len() as usize > self.max_body {
            return Err(FetchError::TooLarge);
        }
        let body = std::fs::read(&path).map_err(|e| FetchError::Io(format!("{}: {e}", path.display())))?;
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        Ok(Response {
            url: url.clone(),
            status: 200,
            content_type: mime_for_extension(ext).into(),
            charset: None,
            body,
            headers: Vec::new(),
        })
    }
}

/// Ad and tracker domains blocked when `Fetcher::block_trackers` is on
/// (a small built-in list in the spirit of Brave / uBlock's defaults).
const TRACKER_DOMAINS: &[&str] = &[
    "doubleclick.net", "googlesyndication.com", "googleadservices.com", "google-analytics.com", "googletagmanager.com",
    "googletagservices.com", "adservice.google.com", "pagead2.googlesyndication.com", "amazon-adsystem.com", "adsrvr.org",
    "criteo.com", "criteo.net", "taboola.com", "outbrain.com", "scorecardresearch.com", "quantserve.com", "quantcount.com",
    "hotjar.com", "hotjar.io", "facebook.net", "connect.facebook.net", "fuseplatform.net", "publift.com", "adnxs.com",
    "rubiconproject.com", "pubmatic.com", "openx.net", "casalemedia.com", "indexww.com", "smartadserver.com", "teads.tv",
    "yieldmo.com", "sharethrough.com", "33across.com", "moatads.com", "adsafeprotected.com", "doubleverify.com",
    "chartbeat.com", "chartbeat.net", "newrelic.com", "nr-data.net", "segment.io", "segment.com", "mixpanel.com",
    "cloudflareinsights.com", "clarity.ms", "bat.bing.com", "ads-twitter.com", "analytics.tiktok.com", "snap.licdn.com",
    "adform.net", "bidswitch.net", "media.net", "imasdk.googleapis.com", "securepubads.g.doubleclick.net", "seedtag.com",
    "richaudience.com", "permutive.com", "permutive.app", "lijit.com", "sovrn.com", "gumgum.com", "id5-sync.com",
];

/// Is `host` a known ad/tracker domain (or a subdomain of one)?
pub fn is_tracker_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    TRACKER_DOMAINS.iter().any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

/// Chrome major version we present as.
pub const CHROME_MAJOR: u32 = 142;

/// A Chrome User-Agent string for the current OS (what Chromium-based
/// browsers such as Edge or Brave also send).
pub fn default_user_agent() -> &'static str {
    if cfg!(target_os = "windows") {
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36"
    } else if cfg!(target_os = "macos") {
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36"
    } else {
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36"
    }
}

fn platform_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "\"Windows\""
    } else if cfg!(target_os = "macos") {
        "\"macOS\""
    } else {
        "\"Linux\""
    }
}

/// `Sec-Fetch-Site` for a request to `url` initiated from `initiator`.
fn fetch_site(initiator: Option<&Url>, url: &Url) -> &'static str {
    match initiator {
        None => "none",
        Some(i) if !i.is_http() => "none",
        Some(i) if i.origin() == url.origin() => "same-origin",
        Some(i) if i.scheme == url.scheme && site_of(&i.host) == site_of(&url.host) => "same-site",
        Some(_) => "cross-site",
    }
}

/// The headers Chrome sends for a request of `dest` to `url`.
pub fn browser_headers(dest: Destination, method: &str, url: &Url, initiator: Option<&Url>, accept_language: &str) -> Vec<(String, String)> {
    let mut h: Vec<(String, String)> = Vec::with_capacity(12);
    let mut push = |k: &str, v: &str| h.push((k.to_string(), v.to_string()));
    let brand = format!("\"Chromium\";v=\"{CHROME_MAJOR}\", \"Google Chrome\";v=\"{CHROME_MAJOR}\", \"Not_A Brand\";v=\"99\"");
    push("sec-ch-ua", &brand);
    push("sec-ch-ua-mobile", "?0");
    push("sec-ch-ua-platform", platform_name());
    let (accept, mode, dest_name) = match dest {
        Destination::Document => (
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7",
            "navigate",
            "document",
        ),
        Destination::Image => ("image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8", "no-cors", "image"),
        Destination::Style => ("text/css,*/*;q=0.1", "no-cors", "style"),
        Destination::Script => ("*/*", "no-cors", "script"),
        Destination::Font => ("*/*", "cors", "font"),
        Destination::Empty => ("*/*", "cors", "empty"),
    };
    if dest == Destination::Document {
        push("Upgrade-Insecure-Requests", "1");
    }
    push("Accept", accept);
    let site = fetch_site(initiator, url);
    push("Sec-Fetch-Site", site);
    push("Sec-Fetch-Mode", mode);
    if dest == Destination::Document {
        push("Sec-Fetch-User", "?1");
    }
    push("Sec-Fetch-Dest", dest_name);
    if mode == "cors" && (site != "same-origin" || method != "GET") {
        if let Some(i) = initiator.filter(|i| i.is_http()) {
            push("Origin", &i.origin());
        }
    }
    push("Accept-Encoding", "gzip, deflate, br, zstd");
    push("Accept-Language", accept_language);
    h
}

fn fetch_data(url: &Url) -> Result<Response, FetchError> {
    // path = "[<mediatype>][;base64],<data>"
    let (meta, data) = url
        .path
        .split_once(',')
        .ok_or_else(|| FetchError::Other("malformed data: URL".into()))?;
    let mut params: Vec<&str> = meta.split(';').map(|s| s.trim()).collect();
    let is_base64 = params.last().map(|p| p.eq_ignore_ascii_case("base64")).unwrap_or(false);
    if is_base64 {
        params.pop();
    }
    let mime = params.first().copied().unwrap_or("");
    let mime = if mime.is_empty() || !mime.contains('/') { "text/plain" } else { mime };
    let mut charset = params
        .iter()
        .skip(1)
        .find_map(|p| p.split_once('=').filter(|(k, _)| k.eq_ignore_ascii_case("charset")).map(|(_, v)| v.to_ascii_lowercase()));
    if charset.is_none() && mime == "text/plain" && params.first().map(|p| p.is_empty()).unwrap_or(true) {
        charset = Some("us-ascii".into());
    }
    let body = if is_base64 {
        base64_decode(&percent_decode(data)).ok_or_else(|| FetchError::Other("invalid base64 in data: URL".into()))?
    } else {
        percent_decode_bytes(data)
    };
    // Fetch §4.2 (data: URLs): the response carries Content-Type.
    let header = match &charset {
        Some(c) => format!("{};charset={}", mime.to_ascii_lowercase(), c),
        None => mime.to_ascii_lowercase(),
    };
    Ok(Response {
        url: url.clone(),
        status: 200,
        content_type: mime.to_ascii_lowercase(),
        charset,
        body,
        headers: vec![("content-type".into(), header)],
    })
}

/// Percent-decode to raw bytes (no UTF-8 validation).
fn percent_decode_bytes(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

pub fn fetch(url: &Url) -> Result<Response, FetchError> {
    Fetcher::default().fetch(url)
}

pub fn fetch_many(urls: &[Url], parallelism: usize) -> Vec<Result<Response, FetchError>> {
    Fetcher::default().fetch_many(urls, parallelism)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64() {
        assert_eq!(base64_decode("SGVsbG8=").unwrap(), b"Hello");
        assert_eq!(base64_decode("SGVsbG8").unwrap(), b"Hello");
        assert_eq!(base64_decode("SGVs\nbG8gd29y bGQ=").unwrap(), b"Hello world");
        assert_eq!(base64_decode("").unwrap(), b"");
        assert_eq!(base64_decode("_-8").unwrap(), base64_decode("/+8").unwrap());
        assert!(base64_decode("SGV*").is_none());
    }

    #[test]
    fn content_type_parsing() {
        assert_eq!(
            parse_content_type("Text/HTML; Charset=\"UTF-8\""),
            ("text/html".to_string(), Some("utf-8".to_string()))
        );
        assert_eq!(parse_content_type("image/png"), ("image/png".to_string(), None));
        assert_eq!(parse_content_type(""), (String::new(), None));
    }

    #[test]
    fn data_urls() {
        let u = Url::parse_absolute("data:text/html;charset=utf-8;base64,PGI+aGk8L2I+").unwrap();
        let r = fetch(&u).unwrap();
        assert_eq!(r.content_type, "text/html");
        assert_eq!(r.charset.as_deref(), Some("utf-8"));
        assert_eq!(r.body, b"<b>hi</b>");
        assert_eq!(r.headers, vec![("content-type".to_string(), "text/html;charset=utf-8".to_string())]);

        let u = Url::parse_absolute("data:,Hello%2C%20World%21").unwrap();
        let r = fetch(&u).unwrap();
        assert_eq!(r.content_type, "text/plain");
        assert_eq!(r.charset.as_deref(), Some("us-ascii"));
        assert_eq!(r.body, b"Hello, World!");

        let u = Url::parse_absolute("data:text/plain,a b").unwrap();
        assert_eq!(fetch(&u).unwrap().body, b"a b");

        let u = Url::parse_absolute("data:nocomma").unwrap();
        assert!(fetch(&u).is_err());
    }

    #[test]
    fn about_and_unsupported() {
        let r = fetch(&Url::parse_absolute("about:blank").unwrap()).unwrap();
        assert_eq!(r.content_type, "text/html");
        assert!(r.body.is_empty());
        assert!(matches!(
            fetch(&Url::parse_absolute("about:home").unwrap()),
            Err(FetchError::UnsupportedScheme(_))
        ));
        assert!(matches!(
            fetch(&Url::parse_absolute("gopher://x/").unwrap()),
            Err(FetchError::UnsupportedScheme(_))
        ));
    }

    #[test]
    fn file_urls() {
        let dir = std::env::temp_dir().join(format!("rabbithole-net-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("pa ge.html");
        std::fs::write(&file, "<p>hi</p>").unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();

        let u = Url::parse_absolute(&format!("file://{}/pa%20ge.html", dir.display())).unwrap();
        let r = fetch(&u).unwrap();
        assert_eq!(r.content_type, "text/html");
        assert_eq!(r.body, b"<p>hi</p>");

        let u = Url::parse_absolute(&format!("file://{}", dir.display())).unwrap();
        let r = fetch(&u).unwrap();
        assert_eq!(r.content_type, "text/html");
        let listing = String::from_utf8(r.body).unwrap();
        assert!(listing.contains("pa ge.html"), "{listing}");
        assert!(listing.contains("href=\"../\""), "{listing}");
        assert!(listing.contains("sub/"), "{listing}");
        assert!(listing.find("sub/").unwrap() < listing.find("pa ge.html").unwrap());

        let u = Url::parse_absolute(&format!("file://{}/missing.html", dir.display())).unwrap();
        assert!(matches!(fetch(&u), Err(FetchError::Io(_))));

        let many = fetch_many(
            &[
                Url::parse_absolute(&format!("file://{}/pa%20ge.html", dir.display())).unwrap(),
                Url::parse_absolute("data:,x").unwrap(),
                Url::parse_absolute("about:blank").unwrap(),
            ],
            2,
        );
        assert_eq!(many.len(), 3);
        assert_eq!(many[0].as_ref().unwrap().body, b"<p>hi</p>");
        assert_eq!(many[1].as_ref().unwrap().body, b"x");
        assert!(many[2].as_ref().unwrap().body.is_empty());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn mime_guess() {
        assert_eq!(mime_for_extension("HTM"), "text/html");
        assert_eq!(mime_for_extension("svg"), "image/svg+xml");
        assert_eq!(mime_for_extension("bin"), "application/octet-stream");
    }

    #[test]
    fn tracker_blocking() {
        assert!(super::is_tracker_host("www.googletagmanager.com"));
        assert!(super::is_tracker_host("cdn.fuseplatform.net"));
        assert!(!super::is_tracker_host("www.wikipedia.org"));
        assert!(!super::is_tracker_host("notdoubleclick.net"));
        let f = Fetcher::default();
        let req = Request::get(Url::parse_absolute("https://www.google-analytics.com/analytics.js").unwrap()).with_dest(Destination::Script);
        assert!(matches!(f.request(&req), Err(FetchError::Blocked(_))));
    }
}
