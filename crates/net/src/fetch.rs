//! Resource fetching.

use crate::cookies::CookieJar;
use crate::url::{percent_decode, percent_encode_component, Url};
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
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

/// A request: method, URL, optional body, extra headers and referer.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub url: Url,
    pub body: Option<Body>,
    pub headers: Vec<(String, String)>,
    pub referer: Option<Url>,
    /// Follow redirects (default true).
    pub follow_redirects: bool,
}

impl Request {
    pub fn get(url: Url) -> Request {
        Request { method: "GET".into(), url, body: None, headers: Vec::new(), referer: None, follow_redirects: true }
    }
    pub fn post(url: Url, body: Body) -> Request {
        Request { method: "POST".into(), url, body: Some(body), headers: Vec::new(), referer: None, follow_redirects: true }
    }
    pub fn with_referer(mut self, r: Url) -> Request {
        self.referer = Some(r);
        self
    }
    pub fn with_header(mut self, k: &str, v: &str) -> Request {
        self.headers.push((k.into(), v.into()));
        self
    }
}

#[derive(Debug, Clone)]
pub enum FetchError {
    UnsupportedScheme(String),
    Io(String),
    Http(u16),
    Timeout,
    TooLarge,
    Other(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::UnsupportedScheme(s) => write!(f, "unsupported URL scheme: {s}"),
            FetchError::Io(e) => write!(f, "I/O error: {e}"),
            FetchError::Http(code) => write!(f, "HTTP error {code}"),
            FetchError::Timeout => write!(f, "request timed out"),
            FetchError::TooLarge => write!(f, "response body too large"),
            FetchError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for FetchError {}

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
    /// Cached HTTP agent (connection pool + TLS config), rebuilt when the
    /// settings it depends on change.
    agent: Arc<Mutex<Option<(String, ureq::Agent)>>>,
}

impl Default for Fetcher {
    fn default() -> Self {
        Fetcher {
            user_agent: "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 RabbitHole/0.2".into(),
            timeout: Duration::from_secs(15),
            max_body: 32 * 1024 * 1024,
            headers: Vec::new(),
            cookies: None,
            accept_language: "en-US,en;q=0.9,es;q=0.8".into(),
            agent: Arc::new(Mutex::new(None)),
        }
    }
}

impl Fetcher {
    /// The shared agent for this fetcher's settings. Agents keep a pool of
    /// open connections, so repeated requests to one host reuse TLS
    /// sessions instead of handshaking every time.
    fn agent(&self) -> ureq::Agent {
        let sig = format!("{}|{}", self.user_agent, self.timeout.as_millis());
        let mut guard = self.agent.lock().unwrap();
        if let Some((s, a)) = guard.as_ref() {
            if *s == sig {
                return a.clone();
            }
        }
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(self.timeout)
            .timeout_read(self.timeout)
            .timeout_write(self.timeout)
            .user_agent(&self.user_agent)
            .redirects(0)
            .max_idle_connections_per_host(6)
            .build();
        *guard = Some((sig, agent.clone()));
        agent
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

impl Fetcher {
    /// Fetch any supported URL: http/https (redirects followed, gzip
    /// handled), file, data (base64 or percent-encoded), about.
    pub fn fetch(&self, url: &Url) -> Result<Response, FetchError> {
        match url.scheme.as_str() {
            "http" | "https" => self.fetch_http(url),
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

    /// Perform an arbitrary HTTP request (GET/POST/...): manual redirect
    /// handling (so cookies set on intermediate hops are kept, and 303 /
    /// 301 / 302 on POST turn into GET per browsers), cookie jar, referer.
    pub fn request(&self, req: &Request) -> Result<Response, FetchError> {
        match req.url.scheme.as_str() {
            "http" | "https" => {}
            _ => return self.fetch(&req.url),
        }
        let agent = self.agent();
        let mut method = req.method.clone();
        let mut url = req.url.without_fragment();
        let mut body = req.body.clone();
        let mut referer = req.referer.clone();
        for _hop in 0..12 {
            let target = url.to_string();
            let mut r = agent
                .request(&method, &target)
                .set("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8")
                .set("Accept-Language", &self.accept_language);
            for (k, v) in &self.headers {
                r = r.set(k, v);
            }
            for (k, v) in &req.headers {
                r = r.set(k, v);
            }
            if let Some(rf) = &referer {
                if rf.is_http() && !(rf.scheme == "https" && url.scheme == "http") {
                    r = r.set("Referer", &rf.without_fragment().to_string());
                }
            }
            if let Some(jar) = &self.cookies {
                if let Some(h) = jar.lock().unwrap().header_for(&url) {
                    r = r.set("Cookie", &h);
                }
            }
            let result = match &body {
                Some(b) => r.set("Content-Type", &b.content_type).send_bytes(&b.data),
                None => r.call(),
            };
            let resp = match result {
                Ok(r) => r,
                Err(ureq::Error::Status(_, r)) => r,
                Err(ureq::Error::Transport(t)) => {
                    let msg = t.to_string();
                    if msg.to_ascii_lowercase().contains("timed out") {
                        return Err(FetchError::Timeout);
                    }
                    return Err(FetchError::Io(msg));
                }
            };
            let status = resp.status();
            let headers: Vec<(String, String)> = resp
                .headers_names()
                .into_iter()
                .flat_map(|n| {
                    let n_l = n.to_ascii_lowercase();
                    resp.all(&n).into_iter().map(move |v| (n_l.clone(), v.to_string())).collect::<Vec<_>>()
                })
                .collect();
            if let Some(jar) = &self.cookies {
                let set: Vec<String> = headers.iter().filter(|(k, _)| k == "set-cookie").map(|(_, v)| v.clone()).collect();
                if !set.is_empty() {
                    jar.lock().unwrap().store_from_headers(&url, &set);
                }
            }
            let is_redirect = matches!(status, 301 | 302 | 303 | 307 | 308);
            if is_redirect && req.follow_redirects {
                if let Some(loc) = headers.iter().find(|(k, _)| k == "location").map(|(_, v)| v.clone()) {
                    if let Some(next) = url.join(loc.trim()) {
                        if next.is_http() {
                            // Drain the body of the redirect response.
                            let _ = read_limited(resp.into_reader(), 1024 * 1024);
                            if status == 303 || ((status == 301 || status == 302) && method == "POST") {
                                method = "GET".into();
                                body = None;
                            }
                            referer = Some(url.clone());
                            url = next.without_fragment();
                            continue;
                        }
                    }
                }
            }
            let (content_type, charset) = parse_content_type(resp.header("Content-Type").unwrap_or(""));
            let data = read_limited(resp.into_reader(), self.max_body)?;
            if status >= 400 && data.is_empty() {
                return Err(FetchError::Http(status));
            }
            return Ok(Response { url: url.clone(), status, content_type, charset, body: data, headers });
        }
        Err(FetchError::Other("too many redirects".into()))
    }

    fn fetch_http(&self, url: &Url) -> Result<Response, FetchError> {
        self.request(&Request::get(url.clone()))
    }

    /// Start a request and return the response headers plus a streaming
    /// body reader (for downloads of arbitrary size). Redirects and cookies
    /// are handled like `request`.
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
        let agent = self.agent();
        let mut method = req.method.clone();
        let mut url = req.url.without_fragment();
        let mut body = req.body.clone();
        let mut referer = req.referer.clone();
        for _hop in 0..12 {
            let target = url.to_string();
            let mut r = agent.request(&method, &target).set("Accept", "*/*").set("Accept-Language", &self.accept_language);
            for (k, v) in &self.headers {
                r = r.set(k, v);
            }
            for (k, v) in &req.headers {
                r = r.set(k, v);
            }
            if let Some(rf) = &referer {
                if rf.is_http() && !(rf.scheme == "https" && url.scheme == "http") {
                    r = r.set("Referer", &rf.without_fragment().to_string());
                }
            }
            if let Some(jar) = &self.cookies {
                if let Some(h) = jar.lock().unwrap().header_for(&url) {
                    r = r.set("Cookie", &h);
                }
            }
            let result = match &body {
                Some(b) => r.set("Content-Type", &b.content_type).send_bytes(&b.data),
                None => r.call(),
            };
            let resp = match result {
                Ok(r) => r,
                Err(ureq::Error::Status(_, r)) => r,
                Err(ureq::Error::Transport(t)) => {
                    let msg = t.to_string();
                    if msg.to_ascii_lowercase().contains("timed out") {
                        return Err(FetchError::Timeout);
                    }
                    return Err(FetchError::Io(msg));
                }
            };
            let status = resp.status();
            let headers: Vec<(String, String)> = resp
                .headers_names()
                .into_iter()
                .flat_map(|n| {
                    let n_l = n.to_ascii_lowercase();
                    resp.all(&n).into_iter().map(move |v| (n_l.clone(), v.to_string())).collect::<Vec<_>>()
                })
                .collect();
            if let Some(jar) = &self.cookies {
                let set: Vec<String> = headers.iter().filter(|(k, _)| k == "set-cookie").map(|(_, v)| v.clone()).collect();
                if !set.is_empty() {
                    jar.lock().unwrap().store_from_headers(&url, &set);
                }
            }
            if matches!(status, 301 | 302 | 303 | 307 | 308) && req.follow_redirects {
                if let Some(loc) = headers.iter().find(|(k, _)| k == "location").map(|(_, v)| v.clone()) {
                    if let Some(next) = url.join(loc.trim()) {
                        if next.is_http() {
                            let _ = read_limited(resp.into_reader(), 1024 * 1024);
                            if status == 303 || ((status == 301 || status == 302) && method == "POST") {
                                method = "GET".into();
                                body = None;
                            }
                            referer = Some(url.clone());
                            url = next.without_fragment();
                            continue;
                        }
                    }
                }
            }
            let (content_type, charset) = parse_content_type(resp.header("Content-Type").unwrap_or(""));
            let content_length = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.trim().parse::<u64>().ok());
            return Ok(StreamedResponse {
                url: url.clone(),
                status,
                content_type,
                charset,
                headers,
                content_length,
                reader: Box::new(resp.into_reader()),
            });
        }
        Err(FetchError::Other("too many redirects".into()))
    }

    /// Stream a URL to a file, reporting progress as (bytes so far, total
    /// if known). Returns the final response metadata (headers, no body).
    /// Returns Err(FetchError::Other("cancelled")) when `progress` returns false.
    pub fn download(&self, req: &Request, dest: &std::path::Path, mut progress: impl FnMut(u64, Option<u64>) -> bool) -> Result<Response, FetchError> {
        use std::io::{Read, Write};
        if !req.url.is_http() {
            let resp = self.fetch(&req.url)?;
            std::fs::write(dest, &resp.body).map_err(|e| FetchError::Io(e.to_string()))?;
            progress(resp.body.len() as u64, Some(resp.body.len() as u64));
            return Ok(Response { body: Vec::new(), ..resp });
        }
        // Resolve redirects/cookies with a HEAD-less approach: issue the
        // request manually without body limits.
        let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(600)).user_agent(&self.user_agent).redirects(10).build();
        let target = req.url.without_fragment().to_string();
        let mut r = agent.request(&req.method, &target).set("Accept", "*/*").set("Accept-Language", &self.accept_language);
        for (k, v) in &self.headers {
            r = r.set(k, v);
        }
        for (k, v) in &req.headers {
            r = r.set(k, v);
        }
        if let Some(rf) = &req.referer {
            r = r.set("Referer", &rf.without_fragment().to_string());
        }
        if let Some(jar) = &self.cookies {
            if let Some(h) = jar.lock().unwrap().header_for(&req.url) {
                r = r.set("Cookie", &h);
            }
        }
        let resp = match &req.body {
            Some(b) => r.set("Content-Type", &b.content_type).send_bytes(&b.data),
            None => r.call(),
        };
        let resp = match resp {
            Ok(r) => r,
            Err(ureq::Error::Status(code, _)) => return Err(FetchError::Http(code)),
            Err(ureq::Error::Transport(t)) => return Err(FetchError::Io(t.to_string())),
        };
        let final_url = Url::parse_absolute(resp.get_url()).unwrap_or_else(|| req.url.clone());
        let status = resp.status();
        let headers: Vec<(String, String)> = resp
            .headers_names()
            .into_iter()
            .map(|n| (n.to_ascii_lowercase(), resp.header(&n).unwrap_or("").to_string()))
            .collect();
        let total = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse::<u64>().ok());
        let (content_type, charset) = parse_content_type(resp.header("Content-Type").unwrap_or(""));
        let mut reader = resp.into_reader();
        let mut file = std::fs::File::create(dest).map_err(|e| FetchError::Io(e.to_string()))?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut done: u64 = 0;
        loop {
            let n = reader.read(&mut buf).map_err(|e| FetchError::Io(e.to_string()))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(|e| FetchError::Io(e.to_string()))?;
            done += n as u64;
            if !progress(done, total) {
                drop(file);
                let _ = std::fs::remove_file(dest);
                return Err(FetchError::Other("cancelled".into()));
            }
        }
        Ok(Response { url: final_url, status, content_type, charset, body: Vec::new(), headers })
    }

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

    /// Fetch several URLs concurrently on a small thread pool, preserving
    /// order. Errors are per-item.
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
    Ok(Response {
        url: url.clone(),
        status: 200,
        content_type: mime.to_ascii_lowercase(),
        charset,
        body,
        headers: Vec::new(),
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
}
