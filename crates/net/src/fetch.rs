//! Resource fetching.

use crate::url::{percent_decode, percent_encode_component, Url};
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
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
}

impl Default for Fetcher {
    fn default() -> Self {
        Fetcher {
            user_agent: "Mozilla/5.0 (compatible; RabbitHole/0.1; +https://github.com/i234dev/RabbitHoleBrowser)".into(),
            timeout: Duration::from_secs(15),
            max_body: 32 * 1024 * 1024,
            headers: Vec::new(),
        }
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
                    })
                } else {
                    Err(FetchError::UnsupportedScheme(url.to_string()))
                }
            }
            other => Err(FetchError::UnsupportedScheme(other.to_string())),
        }
    }

    fn fetch_http(&self, url: &Url) -> Result<Response, FetchError> {
        let agent = ureq::AgentBuilder::new()
            .timeout(self.timeout)
            .user_agent(&self.user_agent)
            .redirects(10)
            .build();
        let target = url.without_fragment().to_string();
        let mut req = agent
            .get(&target)
            .set("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8")
            .set("Accept-Language", "en,es;q=0.8");
        for (k, v) in &self.headers {
            req = req.set(k, v);
        }
        let resp = match req.call() {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) => {
                let final_url = Url::parse_absolute(r.get_url()).unwrap_or_else(|| url.clone());
                let (content_type, charset) = parse_content_type(r.header("Content-Type").unwrap_or(""));
                let body = read_limited(r.into_reader(), self.max_body)?;
                if body.is_empty() {
                    return Err(FetchError::Http(code));
                }
                return Ok(Response {
                    url: final_url,
                    status: code,
                    content_type,
                    charset,
                    body,
                });
            }
            Err(ureq::Error::Transport(t)) => {
                let msg = t.to_string();
                if msg.to_ascii_lowercase().contains("timed out") {
                    return Err(FetchError::Timeout);
                }
                return Err(FetchError::Io(msg));
            }
        };
        let final_url = Url::parse_absolute(resp.get_url()).unwrap_or_else(|| url.clone());
        let status = resp.status();
        let (content_type, charset) = parse_content_type(resp.header("Content-Type").unwrap_or(""));
        let body = read_limited(resp.into_reader(), self.max_body)?;
        Ok(Response {
            url: final_url,
            status,
            content_type,
            charset,
            body,
        })
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
