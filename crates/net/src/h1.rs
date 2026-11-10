//! HTTP/1.1 client framing (RFC 9112): request serialization, response
//! head parsing, Content-Length / chunked / close-delimited bodies, and
//! keep-alive hand-back of the connection when a body is fully read.

use crate::transport::Stream;
use crate::url::Url;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::time::Instant;

pub struct H1Conn {
    pub io: BufReader<Stream>,
    /// Plain-HTTP request through a proxy: absolute-form request target.
    pub via_proxy: bool,
    pub idle_since: Instant,
    /// Served at least one request before (may have gone stale).
    pub reused: bool,
}

impl H1Conn {
    pub fn new(stream: Stream, via_proxy: bool) -> H1Conn {
        H1Conn { io: BufReader::with_capacity(32 * 1024, stream), via_proxy, idle_since: Instant::now(), reused: false }
    }
}

/// Serialize and send a request. `headers` must not contain Host,
/// Content-Length or Connection.
pub fn write_request(conn: &mut H1Conn, method: &str, url: &Url, headers: &[(String, String)], body: Option<&[u8]>, proxy_auth: Option<&str>) -> io::Result<()> {
    let target = if conn.via_proxy {
        url.without_fragment().to_string()
    } else {
        let mut t = if url.path.is_empty() { "/".to_string() } else { url.path.clone() };
        if let Some(q) = &url.query {
            t.push('?');
            t.push_str(q);
        }
        t
    };
    let mut head = String::with_capacity(512);
    head.push_str(method);
    head.push(' ');
    head.push_str(&target);
    head.push_str(" HTTP/1.1\r\nHost: ");
    head.push_str(&url.host_with_port());
    head.push_str("\r\nConnection: keep-alive\r\n");
    for (k, v) in headers {
        if k.eq_ignore_ascii_case("host") || k.eq_ignore_ascii_case("content-length") || k.eq_ignore_ascii_case("connection") {
            continue;
        }
        // Header injection guard.
        if k.contains(['\r', '\n', ':']) || v.contains(['\r', '\n']) {
            continue;
        }
        head.push_str(k);
        head.push_str(": ");
        head.push_str(v);
        head.push_str("\r\n");
    }
    if let Some(a) = proxy_auth.filter(|_| conn.via_proxy) {
        head.push_str("Proxy-Authorization: ");
        head.push_str(a);
        head.push_str("\r\n");
    }
    match body {
        Some(b) => head.push_str(&format!("Content-Length: {}\r\n", b.len())),
        None if matches!(method, "POST" | "PUT" | "PATCH") => head.push_str("Content-Length: 0\r\n"),
        None => {}
    }
    head.push_str("\r\n");
    let mut buf = head.into_bytes();
    let stream = conn.io.get_mut();
    match body {
        Some(b) if b.len() <= 64 * 1024 => {
            buf.extend_from_slice(b);
            stream.write_all(&buf)?;
        }
        Some(b) => {
            stream.write_all(&buf)?;
            stream.write_all(b)?;
        }
        None => stream.write_all(&buf)?,
    }
    stream.flush()
}

/// Parsed response head.
#[derive(Debug)]
pub struct Head {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// HTTP/1.0 response (no keep-alive unless asked).
    pub http10: bool,
}

impl Head {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

/// Read a response head, skipping interim 1xx responses (except 101).
/// Returns `Ok(None)` when the connection was closed before any byte.
pub fn read_head(io: &mut BufReader<Stream>) -> io::Result<Option<Head>> {
    loop {
        let mut total = 0usize;
        let mut line = Vec::new();
        // Status line (tolerate leading blank lines).
        loop {
            line.clear();
            let n = io.read_until(b'\n', &mut line)?;
            if n == 0 {
                if total == 0 {
                    return Ok(None);
                }
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed in response head"));
            }
            total += n;
            if !line.iter().all(|b| b.is_ascii_whitespace()) {
                break;
            }
        }
        let status_line = String::from_utf8_lossy(&line).trim_end().to_string();
        let mut parts = status_line.splitn(3, ' ');
        let version = parts.next().unwrap_or("");
        if !version.starts_with("HTTP/") {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("bad status line: {status_line:.80}")));
        }
        let status: u16 = parts.next().and_then(|s| s.parse().ok()).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad status code"))?;
        let mut headers: Vec<(String, String)> = Vec::new();
        loop {
            line.clear();
            let n = io.read_until(b'\n', &mut line)?;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed in response head"));
            }
            total += n;
            if total > 256 * 1024 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "response head too large"));
            }
            let text = String::from_utf8_lossy(&line);
            let text = text.trim_end_matches(['\r', '\n']);
            if text.is_empty() {
                break;
            }
            if text.starts_with([' ', '\t']) {
                // Obsolete line folding.
                if let Some(last) = headers.last_mut() {
                    last.1.push(' ');
                    last.1.push_str(text.trim());
                }
                continue;
            }
            if let Some((k, v)) = text.split_once(':') {
                headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
            }
        }
        if (100..200).contains(&status) && status != 101 {
            continue;
        }
        return Ok(Some(Head { status, headers, http10: version == "HTTP/1.0" }));
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Framing {
    Empty,
    Length(u64),
    Chunked,
    Close,
}

/// How the body of `head` (answering `method`) is delimited.
pub fn framing(method: &str, head: &Head) -> Framing {
    if method == "HEAD" || head.status == 204 || head.status == 304 || (100..200).contains(&head.status) {
        return Framing::Empty;
    }
    if let Some(te) = head.get("transfer-encoding") {
        if te.to_ascii_lowercase().split(',').any(|t| t.trim() == "chunked") {
            return Framing::Chunked;
        }
        return Framing::Close;
    }
    if let Some(cl) = head.get("content-length") {
        // Duplicate identical values ("5, 5") are tolerated.
        let first = cl.split(',').next().unwrap_or("").trim();
        if let Ok(n) = first.parse::<u64>() {
            return if n == 0 { Framing::Empty } else { Framing::Length(n) };
        }
    }
    Framing::Close
}

/// Whether the connection may be reused after this response.
pub fn keep_alive(head: &Head, framing: Framing) -> bool {
    if framing == Framing::Close {
        return false;
    }
    let conn = head.get("connection").map(|c| c.to_ascii_lowercase()).unwrap_or_default();
    if head.http10 {
        conn.contains("keep-alive")
    } else {
        !conn.split(',').any(|t| t.trim() == "close")
    }
}

type DoneFn = Box<dyn FnOnce(Option<H1Conn>) + Send>;

/// Response body reader; hands the connection back via `on_done` once
/// the body is complete (or `None` if it must not be reused).
pub struct H1Body {
    conn: Option<H1Conn>,
    framing: Framing,
    /// Bytes left in the current chunk / body.
    remaining: u64,
    /// Chunked: need to read the next chunk-size line.
    need_size: bool,
    finished: bool,
    reusable: bool,
    on_done: Option<DoneFn>,
}

impl H1Body {
    pub fn new(conn: H1Conn, framing: Framing, reusable: bool, on_done: DoneFn) -> H1Body {
        let remaining = match framing {
            Framing::Length(n) => n,
            _ => 0,
        };
        let mut b = H1Body { conn: Some(conn), framing, remaining, need_size: true, finished: false, reusable, on_done: Some(on_done) };
        if framing == Framing::Empty {
            b.finish(true);
        }
        b
    }

    fn finish(&mut self, clean: bool) {
        self.finished = true;
        if let Some(done) = self.on_done.take() {
            let conn = self.conn.take().filter(|_| clean && self.reusable).map(|mut c| {
                c.idle_since = Instant::now();
                c.reused = true;
                c
            });
            done(conn);
        }
    }

    fn read_line(io: &mut BufReader<Stream>) -> io::Result<String> {
        let mut line = Vec::new();
        let n = io.by_ref().take(4096).read_until(b'\n', &mut line)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed in chunked body"));
        }
        Ok(String::from_utf8_lossy(&line).trim().to_string())
    }

    fn read_inner(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.finished || buf.is_empty() {
            return Ok(0);
        }
        let io = &mut self.conn.as_mut().ok_or_else(|| io::Error::new(io::ErrorKind::Other, "connection gone"))?.io;
        match self.framing {
            Framing::Empty => Ok(0),
            Framing::Close => {
                let n = io.read(buf)?;
                if n == 0 {
                    self.finish(false);
                }
                Ok(n)
            }
            Framing::Length(_) => {
                let want = buf.len().min(self.remaining.min(usize::MAX as u64) as usize);
                let n = io.read(&mut buf[..want])?;
                if n == 0 {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed before end of body"));
                }
                self.remaining -= n as u64;
                if self.remaining == 0 {
                    self.finish(true);
                }
                Ok(n)
            }
            Framing::Chunked => {
                if self.need_size {
                    let line = Self::read_line(io)?;
                    let size_str = line.split(';').next().unwrap_or("").trim();
                    let size = u64::from_str_radix(size_str, 16).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, format!("bad chunk size {line:.40}")))?;
                    if size == 0 {
                        // Trailers until a blank line.
                        loop {
                            let t = Self::read_line(io)?;
                            if t.is_empty() {
                                break;
                            }
                        }
                        self.finish(true);
                        return Ok(0);
                    }
                    self.remaining = size;
                    self.need_size = false;
                }
                let want = buf.len().min(self.remaining.min(usize::MAX as u64) as usize);
                let n = io.read(&mut buf[..want])?;
                if n == 0 {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed in chunk"));
                }
                self.remaining -= n as u64;
                if self.remaining == 0 {
                    let crlf = Self::read_line(io)?;
                    if !crlf.is_empty() {
                        return Err(io::Error::new(io::ErrorKind::InvalidData, "missing CRLF after chunk"));
                    }
                    self.need_size = true;
                }
                Ok(n)
            }
        }
    }
}

impl Read for H1Body {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.read_inner(buf) {
            Ok(0) if !self.finished && self.framing == Framing::Chunked => {
                // A zero-size chunk was consumed by read_inner; report EOF.
                Ok(0)
            }
            Err(e) => {
                self.finished = true;
                self.reusable = false;
                if let Some(done) = self.on_done.take() {
                    done(None);
                }
                Err(e)
            }
            r => r,
        }
    }
}

impl Drop for H1Body {
    fn drop(&mut self) {
        if let Some(done) = self.on_done.take() {
            // Body not fully read: the connection cannot be reused.
            done(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    fn serve_once(response: &'static [u8]) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            s.write_all(response).unwrap();
        });
        port
    }

    fn get(port: u16) -> (Head, Vec<u8>, bool) {
        let c = crate::transport::connect("http", "127.0.0.1", port, crate::transport::Alpn::Http1, std::time::Duration::from_secs(5)).unwrap();
        let mut conn = H1Conn::new(c.stream, false);
        let url = Url::parse_absolute(&format!("http://127.0.0.1:{port}/x")).unwrap();
        write_request(&mut conn, "GET", &url, &[], None, None).unwrap();
        let head = read_head(&mut conn.io).unwrap().unwrap();
        let fr = framing("GET", &head);
        let ka = keep_alive(&head, fr);
        let returned = Arc::new(Mutex::new(false));
        let r2 = returned.clone();
        let mut body = H1Body::new(conn, fr, ka, Box::new(move |c| *r2.lock().unwrap() = c.is_some()));
        let mut out = Vec::new();
        body.read_to_end(&mut out).unwrap();
        drop(body);
        let r = *returned.lock().unwrap();
        (head, out, r)
    }

    #[test]
    fn chunked_with_trailers_and_interim() {
        let port = serve_once(b"HTTP/1.1 103 Early Hints\r\nLink: </a.css>\r\n\r\nHTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nX-Folded: a\r\n b\r\n\r\n5;ext=1\r\nhello\r\n6\r\n world\r\n0\r\nTrailer: x\r\n\r\n");
        let (head, body, reused) = get(port);
        assert_eq!(head.status, 200);
        assert_eq!(head.get("x-folded"), Some("a b"));
        assert_eq!(body, b"hello world");
        assert!(reused);
    }

    #[test]
    fn content_length_and_close() {
        let port = serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabcEXTRA");
        let (_, body, reused) = get(port);
        assert_eq!(body, b"abc");
        assert!(reused);
        let port = serve_once(b"HTTP/1.0 200 OK\r\n\r\nuntil close");
        let (head, body, reused) = get(port);
        assert!(head.http10);
        assert_eq!(body, b"until close");
        assert!(!reused);
    }
}
