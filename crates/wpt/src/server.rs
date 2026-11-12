//! A small loopback HTTP server implementing the parts of wptserve that
//! testharness.js tests rely on: static files with `.headers` /
//! `__dir__.headers`, generated wrappers for `.any.js`, `.window.js` and
//! `.worker.js` tests, `{{...}}` substitutions (`.sub.` files and
//! `?pipe=sub`), the common pipes (`sub`, `header`, `status`, `slice`,
//! `raw`, `trickle` without the delays), `.asis` files, and native
//! replacements for a few of the Python handlers tests use most. Other
//! Python handlers answer `501`, so the tests that need them fail fast.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::handlers;

/// The host names tests see. `localhost` and its subdomains all resolve
/// to the loopback interface, so cross-origin tests get distinct origins
/// without touching `/etc/hosts`.
pub const HOST: &str = "localhost";
/// Base of the `hosts[alt][...]` names (a different "site").
pub const ALT_HOST: &str = "alt.localhost";

/// Shared server configuration.
pub struct Config {
    pub root: PathBuf,
    /// The two HTTP ports (`ports[http][0]` and `[1]`).
    pub ports: [u16; 2],
    /// Key/value store for stash-based handlers (`/common/dispatcher`,
    /// `stash.py` style tests).
    pub stash: Mutex<HashMap<String, String>>,
}

pub struct Server {
    pub config: Arc<Config>,
}

impl Server {
    /// Bind both ports on 127.0.0.1 and ::1 and serve `root` from
    /// background threads.
    pub fn start(root: &Path) -> std::io::Result<Server> {
        let (l0, l0v6) = bind_pair()?;
        let (l1, l1v6) = bind_pair()?;
        let ports = [l0.local_addr()?.port(), l1.local_addr()?.port()];
        let config = Arc::new(Config { root: root.to_path_buf(), ports, stash: Mutex::new(HashMap::new()) });
        for l in [Some(l0), l0v6, Some(l1), l1v6].into_iter().flatten() {
            let cfg = config.clone();
            std::thread::spawn(move || {
                for stream in l.incoming().flatten() {
                    let cfg = cfg.clone();
                    std::thread::spawn(move || {
                        let _ = handle_connection(stream, &cfg);
                    });
                }
            });
        }
        Ok(Server { config })
    }

    /// Base URL of the main origin (`http://localhost:PORT`).
    pub fn base(&self) -> String {
        format!("http://{}:{}", HOST, self.config.ports[0])
    }
}

/// Listen on 127.0.0.1 and (when available) ::1 with the same port.
fn bind_pair() -> std::io::Result<(TcpListener, Option<TcpListener>)> {
    for _ in 0..20 {
        let v4 = TcpListener::bind("127.0.0.1:0")?;
        let port = v4.local_addr()?.port();
        match TcpListener::bind(("::1", port)) {
            Ok(v6) => return Ok((v4, Some(v6))),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(_) => return Ok((v4, None)),
        }
    }
    Ok((TcpListener::bind("127.0.0.1:0")?, None))
}

/// A parsed HTTP request.
pub struct Request {
    pub method: String,
    /// Percent-decoded path.
    pub path: String,
    /// Raw query string (without `?`).
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Host header (`host:port`).
    pub host: String,
    /// The local port the request arrived on.
    pub port: u16,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// Decoded query parameters, in order.
    pub fn query_pairs(&self) -> Vec<(String, String)> {
        parse_form(&self.query)
    }

    /// First value of a query parameter.
    pub fn get(&self, name: &str) -> Option<String> {
        self.query_pairs().into_iter().find(|(k, _)| k == name).map(|(_, v)| v)
    }

    pub fn hostname(&self) -> &str {
        let h = self.host.as_str();
        if let Some(rest) = h.strip_prefix('[') {
            return rest.split(']').next().unwrap_or(rest);
        }
        h.rsplit_once(':').map(|(a, _)| a).unwrap_or(h)
    }
}

/// `application/x-www-form-urlencoded` parsing.
pub fn parse_form(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|s| !s.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (form_decode(k), form_decode(v))
        })
        .collect()
}

fn form_decode(s: &str) -> String {
    percent_decode_bytes(&s.replace('+', " "))
}

pub fn percent_decode_bytes(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

/// An HTTP response.
pub struct Response {
    pub status: u16,
    pub reason: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Write `body` as the whole response (`.asis` files).
    pub raw: bool,
}

impl Response {
    pub fn new(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Response {
        let mut r = Response { status, reason: reason(status).to_string(), headers: Vec::new(), body: body.into(), raw: false };
        if !content_type.is_empty() {
            r.headers.push(("Content-Type".into(), content_type.into()));
        }
        r
    }

    pub fn text(status: u16, body: &str) -> Response {
        Response::new(status, "text/plain", body.as_bytes().to_vec())
    }

    pub fn set_header(&mut self, name: &str, value: &str) {
        self.headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
        self.headers.push((name.to_string(), value.to_string()));
    }

    pub fn add_header(&mut self, name: &str, value: &str) {
        self.headers.push((name.to_string(), value.to_string()));
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        416 => "Range Not Satisfiable",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        _ => "Status",
    }
}

fn handle_connection(stream: TcpStream, cfg: &Config) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let port = stream.local_addr()?.port();
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut stream = stream;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let line = line.trim_end().to_string();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split(' ');
        let method = parts.next().unwrap_or("GET").to_string();
        let target = parts.next().unwrap_or("/").to_string();
        let mut headers = Vec::new();
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h)? == 0 {
                break;
            }
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                headers.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
        let len = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.parse::<usize>().ok())
            .unwrap_or(0);
        let mut body = vec![0u8; len];
        reader.read_exact(&mut body)?;
        let chunked = headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("transfer-encoding") && v.to_ascii_lowercase().contains("chunked"));
        if chunked {
            body = read_chunked(&mut reader)?;
        }
        let target = target.split('#').next().unwrap_or("").to_string();
        let (raw_path, query) = match target.split_once('?') {
            Some((p, q)) => (p.to_string(), q.to_string()),
            None => (target.clone(), String::new()),
        };
        let raw_path = match raw_path.find("://") {
            // Absolute-form request target.
            Some(i) => raw_path[i + 3..].find('/').map(|j| raw_path[i + 3 + j..].to_string()).unwrap_or_else(|| "/".into()),
            None => raw_path,
        };
        let host = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("host")).map(|(_, v)| v.clone()).unwrap_or_else(|| format!("{HOST}:{port}"));
        let req = Request { method, path: percent_decode_bytes(&raw_path), query, headers, body, host, port };
        let resp = respond(&req, cfg);
        write_response(&mut stream, &req, resp)?;
        // One request per connection keeps the server simple.
        return Ok(());
    }
}

fn read_chunked(reader: &mut BufReader<TcpStream>) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let mut size = String::new();
        reader.read_line(&mut size)?;
        let n = usize::from_str_radix(size.trim().split(';').next().unwrap_or("0"), 16).unwrap_or(0);
        if n == 0 {
            let mut end = String::new();
            let _ = reader.read_line(&mut end);
            return Ok(out);
        }
        let mut chunk = vec![0u8; n];
        reader.read_exact(&mut chunk)?;
        out.extend_from_slice(&chunk);
        let mut crlf = String::new();
        reader.read_line(&mut crlf)?;
    }
}

fn write_response(stream: &mut TcpStream, req: &Request, resp: Response) -> std::io::Result<()> {
    if resp.raw {
        stream.write_all(&resp.body)?;
        return stream.flush();
    }
    let mut head = format!("HTTP/1.1 {} {}\r\n", resp.status, resp.reason);
    let mut has_len = false;
    for (k, v) in &resp.headers {
        if k.eq_ignore_ascii_case("content-length") {
            has_len = true;
        }
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    if !has_len {
        head.push_str(&format!("Content-Length: {}\r\n", resp.body.len()));
    }
    head.push_str("Connection: close\r\n\r\n");
    stream.write_all(head.as_bytes())?;
    if req.method != "HEAD" {
        stream.write_all(&resp.body)?;
    }
    stream.flush()
}

/// Content type from a file extension (what wptserve guesses).
pub fn content_type_for(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html",
        "xhtml" | "xht" => "application/xhtml+xml",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "txt" | "dat" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "wasm" => "application/wasm",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "ogv" => "video/ogg",
        "ogg" | "oga" => "audio/ogg",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "pdf" => "application/pdf",
        "vtt" => "text/vtt",
        "manifest" | "webmanifest" => "application/manifest+json",
        _ => "application/octet-stream",
    }
}

/// Map a URL path onto the checkout, refusing `..` escapes.
fn fs_path(cfg: &Config, url_path: &str) -> Option<PathBuf> {
    let rel = url_path.trim_start_matches('/');
    let p = Path::new(rel);
    if p.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    Some(cfg.root.join(p))
}

const REPORT_JS: &str = include_str!("testharnessreport.js");
const VENDOR_JS: &str = include_str!("testdriver-vendor.js");

fn respond(req: &Request, cfg: &Config) -> Response {
    if req.method == "OPTIONS" && !req.path.ends_with(".py") {
        let mut r = Response::new(200, "", Vec::new());
        r.add_header("Access-Control-Allow-Origin", req.header("origin").unwrap_or("*"));
        r.add_header("Access-Control-Allow-Methods", "GET, POST, PUT, DELETE, OPTIONS");
        r.add_header("Access-Control-Allow-Headers", req.header("access-control-request-headers").unwrap_or("*"));
        return r;
    }
    match req.path.as_str() {
        "/resources/testharnessreport.js" => return Response::new(200, "text/javascript", REPORT_JS),
        "/resources/testdriver-vendor.js" => return Response::new(200, "text/javascript", VENDOR_JS),
        _ => {}
    }
    if let Some(r) = handlers::handle(req, cfg) {
        return apply_pipes(req, cfg, r, false);
    }
    // wptserve's rewrites.
    let url_path = match req.path.as_str() {
        "/resources/WebIDLParser.js" => "/resources/webidl2/lib/webidl2.js",
        p => p,
    };
    let Some(path) = fs_path(cfg, url_path) else {
        return Response::text(400, "bad path");
    };
    if path.is_file() {
        return serve_file(req, cfg, &path);
    }
    if path.is_dir() {
        let index = path.join("index.html");
        if index.is_file() {
            return serve_file(req, cfg, &index);
        }
        return Response::text(404, "directory");
    }
    if let Some(r) = wrapper(req, cfg) {
        return r;
    }
    if req.path.ends_with(".py") {
        return Response::text(501, "python handler not implemented by the RabbitHole WPT server");
    }
    Response::text(404, "not found")
}

/// Generated test documents for `.any.js`, `.window.js`, `.worker.js`.
fn wrapper(req: &Request, cfg: &Config) -> Option<Response> {
    let p = req.path.as_str();
    let query = if req.query.is_empty() { String::new() } else { format!("?{}", req.query) };
    let read_meta = |js_path: &str| -> Option<Vec<(String, String)>> {
        let src = std::fs::read_to_string(fs_path(cfg, js_path)?).ok()?;
        Some(parse_meta(&src))
    };
    if let Some(base) = p.strip_suffix(".any.html") {
        let js = format!("{base}.any.js");
        let meta = read_meta(&js)?;
        let body = format!(
            "<!doctype html>\n<meta charset=utf-8>\n{}\n<script>\nself.GLOBAL = {{\n  isWindow: function() {{ return true; }},\n  isWorker: function() {{ return false; }},\n  isShadowRealm: function() {{ return false; }},\n}};\n</script>\n<script src=\"/resources/testharness.js\"></script>\n<script src=\"/resources/testharnessreport.js\"></script>\n{}\n<div id=log></div>\n<script src=\"{}\"></script>\n",
            html_meta(&meta),
            html_scripts(&meta),
            js
        );
        return Some(Response::new(200, "text/html", body));
    }
    if let Some(base) = p.strip_suffix(".any.worker.html") {
        let js = format!("{base}.any.js");
        let meta = read_meta(&js)?;
        let body = format!(
            "<!doctype html>\n<meta charset=utf-8>\n{}\n<script src=\"/resources/testharness.js\"></script>\n<script src=\"/resources/testharnessreport.js\"></script>\n<div id=log></div>\n<script>\nfetch_tests_from_worker(new Worker(\"{base}.any.worker.js{query}\"));\n</script>\n",
            html_meta(&meta)
        );
        return Some(Response::new(200, "text/html", body));
    }
    if let Some(base) = p.strip_suffix(".any.worker.js") {
        let js = format!("{base}.any.js");
        let meta = read_meta(&js)?;
        let mut scripts = String::new();
        let mut title = String::new();
        for (k, v) in &meta {
            if k == "script" {
                scripts.push_str(&format!("importScripts(\"{}\");\n", js_escape(v)));
            } else if k == "title" {
                title = format!("self.META_TITLE = \"{}\";\n", js_escape(v));
            }
        }
        let body = format!(
            "{title}\nself.GLOBAL = {{\n  isWindow: function() {{ return false; }},\n  isWorker: function() {{ return true; }},\n  isShadowRealm: function() {{ return false; }},\n}};\nimportScripts(\"/resources/testharness.js\");\n{scripts}\nimportScripts(\"{js}\");\ndone();\n"
        );
        return Some(Response::new(200, "text/javascript", body));
    }
    if let Some(base) = p.strip_suffix(".window.html") {
        let js = format!("{base}.window.js");
        let meta = read_meta(&js)?;
        let body = format!(
            "<!doctype html>\n<meta charset=utf-8>\n{}\n<script src=\"/resources/testharness.js\"></script>\n<script src=\"/resources/testharnessreport.js\"></script>\n{}\n<div id=log></div>\n<script src=\"{}\"></script>\n",
            html_meta(&meta),
            html_scripts(&meta),
            js
        );
        return Some(Response::new(200, "text/html", body));
    }
    if let Some(base) = p.strip_suffix(".worker.html") {
        let js = format!("{base}.worker.js");
        let meta = read_meta(&js)?;
        let body = format!(
            "<!doctype html>\n<meta charset=utf-8>\n{}\n<script src=\"/resources/testharness.js\"></script>\n<script src=\"/resources/testharnessreport.js\"></script>\n<div id=log></div>\n<script>\nfetch_tests_from_worker(new Worker(\"{js}{query}\"));\n</script>\n",
            html_meta(&meta)
        );
        return Some(Response::new(200, "text/html", body));
    }
    None
}

/// `// META: key=value` lines at the top of a `.js` test.
pub fn parse_meta(src: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in src.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        let Some(rest) = l.strip_prefix("//") else { break };
        let Some(rest) = rest.trim_start().strip_prefix("META:") else {
            continue;
        };
        if let Some((k, v)) = rest.trim().split_once('=') {
            out.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    out
}

fn html_escape_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;")
}

fn js_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn html_meta(meta: &[(String, String)]) -> String {
    let mut out = String::new();
    for (k, v) in meta {
        match k.as_str() {
            "timeout" if v == "long" => out.push_str("<meta name=\"timeout\" content=\"long\">\n"),
            "title" => out.push_str(&format!("<title>{}</title>\n", v.replace('&', "&amp;").replace('<', "&lt;"))),
            _ => {}
        }
    }
    out
}

fn html_scripts(meta: &[(String, String)]) -> String {
    let mut out = String::new();
    for (k, v) in meta {
        if k == "script" {
            out.push_str(&format!("<script src=\"{}\"></script>\n", html_escape_attr(v)));
        }
    }
    out
}

/// Headers from a `.headers` file (`Name: value` per line).
fn read_headers_file(path: &Path) -> Vec<(String, String)> {
    let Ok(s) = std::fs::read_to_string(path) else { return Vec::new() };
    s.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

fn serve_file(req: &Request, cfg: &Config, path: &Path) -> Response {
    let Ok(body) = std::fs::read(path) else {
        return Response::text(500, "unreadable");
    };
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
    let is_sub = name.contains(".sub.");
    if name.ends_with(".asis") {
        let body = if is_sub { substitute(req, cfg, &String::from_utf8_lossy(&body), &mut HashMap::new()).into_bytes() } else { body };
        let mut r = Response::new(200, "", body);
        r.raw = true;
        return r;
    }
    let ct = content_type_for(name.trim_end_matches(".sub"));
    let mut resp = Response::new(200, ct, body);
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut extra = read_headers_file(&dir.join("__dir__.headers"));
    extra.extend(read_headers_file(&path.with_file_name(format!("{name}.headers"))));
    let mut subbed_vars = HashMap::new();
    for (k, v) in extra {
        let v = if is_sub { substitute(req, cfg, &v, &mut subbed_vars) } else { v };
        if k.eq_ignore_ascii_case("content-type") {
            resp.set_header(&k, &v);
        } else {
            resp.add_header(&k, &v);
        }
    }
    if is_sub {
        let text = String::from_utf8_lossy(&resp.body).into_owned();
        resp.body = substitute(req, cfg, &text, &mut subbed_vars).into_bytes();
    }
    if let Some(range) = req.header("range") {
        if let Some(r) = apply_range(&resp, range) {
            resp = r;
        }
    }
    apply_pipes(req, cfg, resp, is_sub)
}

/// `Range: bytes=a-b` on static files.
fn apply_range(resp: &Response, range: &str) -> Option<Response> {
    let spec = range.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let len = resp.body.len();
    let (start, end) = if a.is_empty() {
        let n: usize = b.parse().ok()?;
        (len.saturating_sub(n), len.saturating_sub(1))
    } else {
        let s: usize = a.parse().ok()?;
        let e: usize = if b.is_empty() { len.saturating_sub(1) } else { b.parse::<usize>().ok()?.min(len.saturating_sub(1)) };
        (s, e)
    };
    if start >= len || start > end {
        let mut r = Response::new(416, "", Vec::new());
        r.add_header("Content-Range", &format!("bytes */{len}"));
        return Some(r);
    }
    let mut r = Response::new(206, "", resp.body[start..=end].to_vec());
    for (k, v) in &resp.headers {
        r.add_header(k, v);
    }
    r.add_header("Content-Range", &format!("bytes {start}-{end}/{len}"));
    r.add_header("Accept-Ranges", "bytes");
    Some(r)
}

/// `?pipe=a|b(c,d)`.
fn apply_pipes(req: &Request, cfg: &Config, mut resp: Response, already_subbed: bool) -> Response {
    let Some(pipe) = req.get("pipe") else { return resp };
    for (name, args) in parse_pipes(&pipe) {
        match name.as_str() {
            "sub" if !already_subbed => {
                let text = String::from_utf8_lossy(&resp.body).into_owned();
                resp.body = substitute(req, cfg, &text, &mut HashMap::new()).into_bytes();
            }
            "status" => {
                if let Some(code) = args.first().and_then(|c| c.parse::<u16>().ok()) {
                    resp.status = code;
                    resp.reason = args.get(1).cloned().unwrap_or_else(|| reason(code).to_string());
                }
            }
            "header" => {
                if args.len() >= 2 {
                    let append = args.get(2).map(|a| a.eq_ignore_ascii_case("true")).unwrap_or(false);
                    if append {
                        resp.add_header(&args[0], &args[1]);
                    } else {
                        resp.set_header(&args[0], &args[1]);
                    }
                }
            }
            "slice" => {
                let len = resp.body.len() as i64;
                let idx = |s: Option<&String>, d: i64| -> usize {
                    match s.and_then(|s| if s == "null" { None } else { s.parse::<i64>().ok() }) {
                        Some(v) if v < 0 => (len + v).max(0) as usize,
                        Some(v) => v.min(len) as usize,
                        None => d as usize,
                    }
                };
                let a = idx(args.first(), 0);
                let b = idx(args.get(1), len);
                resp.body = if a < b { resp.body[a..b].to_vec() } else { Vec::new() };
            }
            "gzip" => {}
            _ => {}
        }
    }
    resp
}

fn parse_pipes(s: &str) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut cur = String::new();
    let mut parts = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                cur.push(c);
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                depth -= 1;
                cur.push(c);
            }
            '|' if depth == 0 => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    parts.push(cur);
    for p in parts {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        match p.find('(') {
            Some(i) if p.ends_with(')') => {
                let inner = &p[i + 1..p.len() - 1];
                let mut args = Vec::new();
                let mut a = String::new();
                let mut it = inner.chars();
                while let Some(c) = it.next() {
                    match c {
                        '\\' => {
                            if let Some(n) = it.next() {
                                a.push(n);
                            }
                        }
                        ',' => args.push(std::mem::take(&mut a).trim().to_string()),
                        _ => a.push(c),
                    }
                }
                args.push(a.trim().to_string());
                out.push((p[..i].to_string(), args));
            }
            _ => out.push((p.to_string(), Vec::new())),
        }
    }
    out
}

static UUID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// A random (v4-shaped) UUID.
pub fn uuid() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut bytes = [0u8; 16];
    for half in 0..2 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(UUID_COUNTER.fetch_add(1, Ordering::Relaxed));
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        bytes[half * 8..half * 8 + 8].copy_from_slice(&h.finish().to_le_bytes());
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let h: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// Host name for `domains[name]`.
pub fn domain(name: &str) -> String {
    match name {
        "" => HOST.to_string(),
        "élève" => format!("xn--lve-6lad.{HOST}"),
        "天気の良い日" => format!("xn--n8j6ds53lwwkrqhv28a.{HOST}"),
        sub => format!("{sub}.{HOST}"),
    }
}

/// `{{...}}` template substitution (wptserve's `sub` pipe).
pub fn substitute(req: &Request, cfg: &Config, text: &str, vars: &mut HashMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let Some(j) = after.find("}}") else {
            out.push_str(&rest[i..]);
            return out;
        };
        let expr = &after[..j];
        match eval_sub(req, cfg, expr.trim(), vars) {
            Some(v) => out.push_str(&v),
            None => {
                out.push_str("{{");
                out.push_str(expr);
                out.push_str("}}");
            }
        }
        rest = &after[j + 2..];
    }
    out.push_str(rest);
    out
}

fn bracket_args(expr: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = expr;
    while let Some(i) = rest.find('[') {
        let Some(j) = rest[i..].find(']') else { break };
        out.push(rest[i + 1..i + j].trim_matches(|c| c == '"' || c == '\'').to_string());
        rest = &rest[i + j + 1..];
    }
    out
}

fn eval_sub(req: &Request, cfg: &Config, expr: &str, vars: &mut HashMap<String, String>) -> Option<String> {
    if let Some(v) = expr.strip_prefix('$') {
        if let Some((name, e)) = v.split_once(':') {
            let val = eval_sub(req, cfg, e.trim(), vars)?;
            vars.insert(name.trim().to_string(), val.clone());
            return Some(val);
        }
        return vars.get(v.trim()).cloned();
    }
    let head = expr.split(|c| c == '[' || c == '(').next().unwrap_or("").trim();
    let args = bracket_args(expr);
    let scheme_port = |scheme: &str, idx: usize| -> u16 {
        match scheme {
            "http" | "https" | "h2" | "ws" | "wss" | "webtransport-h3" => cfg.ports[idx.min(1)],
            _ => cfg.ports[0],
        }
    };
    match head {
        "host" => Some(HOST.to_string()),
        "hosts" => {
            let base = if args.first().map(|s| s.as_str()) == Some("alt") { ALT_HOST } else { HOST };
            match args.get(1).map(|s| s.as_str()).unwrap_or("") {
                "" => Some(base.to_string()),
                sub => Some(format!("{}.{}", domain(sub).trim_end_matches(&format!(".{HOST}")), base)),
            }
        }
        "domains" => Some(domain(args.first().map(|s| s.as_str()).unwrap_or(""))),
        "ports" => {
            let scheme = args.first().map(|s| s.as_str()).unwrap_or("http");
            let idx = args.get(1).and_then(|i| i.parse::<usize>().ok()).unwrap_or(0);
            Some(scheme_port(scheme, idx).to_string())
        }
        "location" => {
            let key = args.first().map(|s| s.as_str()).unwrap_or("");
            let hostname = req.hostname().to_string();
            let port = req.port.to_string();
            let q = if req.query.is_empty() { String::new() } else { format!("?{}", req.query) };
            Some(match key {
                "scheme" => "http".into(),
                "host" => format!("{hostname}:{port}"),
                "hostname" => hostname,
                "port" => port,
                "path" | "pathname" => req.path.clone(),
                "query" => q,
                "server" => format!("http://{hostname}:{port}"),
                "href" => format!("http://{hostname}:{port}{}{q}", req.path),
                _ => return None,
            })
        }
        "GET" => Some(req.get(args.first()?).unwrap_or_default()),
        "headers" => Some(req.header(args.first()?).unwrap_or("").to_string()),
        "header_or_default" => {
            let inner = expr.split_once('(')?.1.trim_end_matches(')');
            let (name, def) = inner.split_once(',').unwrap_or((inner, ""));
            let name = name.trim().trim_matches(|c| c == '"' || c == '\'');
            let def = def.trim().trim_matches(|c| c == '"' || c == '\'');
            Some(req.header(name).unwrap_or(def).to_string())
        }
        "uuid" => Some(uuid()),
        "url_base" => Some("/".into()),
        "fs_path" => {
            let inner = expr.split_once('(')?.1.trim_end_matches(')').trim().trim_matches(|c| c == '"' || c == '\'');
            Some(fs_path(cfg, inner)?.to_string_lossy().into_owned())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipes_parse() {
        let p = parse_pipes("sub|header(Content-Type,text/plain)|status(404)");
        assert_eq!(p.len(), 3);
        assert_eq!(p[1].0, "header");
        assert_eq!(p[1].1, vec!["Content-Type".to_string(), "text/plain".to_string()]);
        let p = parse_pipes("header(X,a\\,b)");
        assert_eq!(p[0].1, vec!["X".to_string(), "a,b".to_string()]);
    }

    #[test]
    fn meta_lines() {
        let m = parse_meta("// META: title=Foo\n// META: script=/common/a.js\n// META: global=window,worker\n\ntest(...)\n// META: script=no");
        assert_eq!(m.len(), 3);
        assert_eq!(m[1], ("script".into(), "/common/a.js".into()));
    }
}
