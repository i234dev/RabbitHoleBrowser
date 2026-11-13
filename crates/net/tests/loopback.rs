//! Integration tests against local loopback servers: HTTP/1.1 keep-alive
//! and content codings, HTTP cache revalidation, CORS, redirect modes,
//! WebSocket echo, EventSource, and HTTP/2 (h2c prior knowledge) with
//! multiplexing, flow control, CONTINUATION, GOAWAY and fallback.

use rabbithole_net::h2::{frame, hpack};
use rabbithole_net::websocket::{accept_key, encode_frame, read_frame};
use rabbithole_net::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ------------------------------------------------------------ h1 server

#[derive(Debug, Clone)]
struct Req {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Req {
    fn header(&self, n: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.as_str())
    }
}

type Handler = Arc<dyn Fn(&Req) -> (u16, Vec<(String, String)>, Vec<u8>) + Send + Sync>;

struct Server {
    port: u16,
    log: Arc<Mutex<Vec<Req>>>,
    conns: Arc<AtomicUsize>,
}

impl Server {
    fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
    fn url(&self, path: &str) -> Url {
        Url::parse_absolute(&format!("{}{}", self.origin(), path)).unwrap()
    }
    fn requests(&self) -> Vec<Req> {
        self.log.lock().unwrap().clone()
    }
}

fn read_h1_request(r: &mut BufReader<TcpStream>) -> Option<Req> {
    let mut line = String::new();
    if r.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let mut headers = Vec::new();
    loop {
        let mut l = String::new();
        r.read_line(&mut l).ok()?;
        let l = l.trim_end();
        if l.is_empty() {
            break;
        }
        let (k, v) = l.split_once(':')?;
        headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
    }
    let len = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse::<usize>().ok()).unwrap_or(0);
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).ok()?;
    Some(Req { method, path, headers, body })
}

fn write_h1_response(s: &mut TcpStream, status: u16, headers: &[(String, String)], body: &[u8], head_only: bool) {
    let mut out = format!("HTTP/1.1 {status} X\r\n");
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-length") || k.eq_ignore_ascii_case("transfer-encoding")) {
        out.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    out.push_str("\r\n");
    let mut bytes = out.into_bytes();
    if !head_only && status != 304 && status != 204 {
        bytes.extend_from_slice(body);
    }
    let _ = s.write_all(&bytes);
}

fn serve(handler: Handler) -> Server {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let log = Arc::new(Mutex::new(Vec::new()));
    let conns = Arc::new(AtomicUsize::new(0));
    let (log2, conns2) = (log.clone(), conns.clone());
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { continue };
            conns2.fetch_add(1, Ordering::SeqCst);
            let (h, log) = (handler.clone(), log2.clone());
            std::thread::spawn(move || {
                let mut w = s.try_clone().unwrap();
                let mut r = BufReader::new(s);
                while let Some(req) = read_h1_request(&mut r) {
                    log.lock().unwrap().push(req.clone());
                    let (st, hs, body) = h(&req);
                    write_h1_response(&mut w, st, &hs, &body, req.method == "HEAD");
                }
            });
        }
    });
    Server { port, log, conns }
}

fn hv(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

fn fetcher() -> Fetcher {
    let mut f = Fetcher::isolated();
    f.timeout = Duration::from_secs(5);
    f.cookies = Some(Arc::new(Mutex::new(CookieJar::new())));
    f
}

// ---------------------------------------------------------------- tests

#[test]
fn http1_keepalive_codings_and_headers() {
    let srv = serve(Arc::new(|req: &Req| {
        let text = format!("hello from {}", req.path).repeat(50);
        match req.path.as_str() {
            "/gzip" => {
                let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                e.write_all(text.as_bytes()).unwrap();
                (200, hv(&[("content-encoding", "gzip"), ("content-type", "text/plain")]), e.finish().unwrap())
            }
            "/deflate-raw" => {
                let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                e.write_all(text.as_bytes()).unwrap();
                (200, hv(&[("content-encoding", "deflate")]), e.finish().unwrap())
            }
            "/zlib" => {
                let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                e.write_all(text.as_bytes()).unwrap();
                (200, hv(&[("content-encoding", "deflate")]), e.finish().unwrap())
            }
            "/zstd" => (200, hv(&[("content-encoding", "zstd")]), ruzstd::encoding::compress_to_vec(text.as_bytes(), ruzstd::encoding::CompressionLevel::Fastest)),
            "/chunked" => (200, hv(&[("transfer-encoding", "chunked")]), b"5\r\nhello\r\n0\r\n\r\n".to_vec()),
            "/empty-gzip" => (204, hv(&[("content-encoding", "gzip")]), vec![]),
            "/cookie" => (200, hv(&[("set-cookie", "sid=42; Path=/")]), b"ok".to_vec()),
            _ => (200, hv(&[("content-type", "text/plain; charset=utf-8")]), text.into_bytes()),
        }
    }));
    let f = fetcher();
    for p in ["/gzip", "/deflate-raw", "/zlib", "/zstd", "/plain"] {
        let r = f.request(&Request::get(srv.url(p)).with_cache_mode(CacheMode::NoStore)).unwrap();
        assert_eq!(String::from_utf8(r.body).unwrap(), format!("hello from {p}").repeat(50), "{p}");
    }
    let r = f.request(&Request::get(srv.url("/chunked"))).unwrap();
    assert_eq!(r.body, b"hello");
    let r = f.request(&Request::get(srv.url("/empty-gzip"))).unwrap_or_else(|e| panic!("{e}"));
    assert!(r.body.is_empty());
    // Sequential requests reuse one keep-alive connection.
    assert_eq!(srv.conns.load(Ordering::SeqCst), 1, "{}", f.stats());
    // Cookies round-trip; Chrome-like headers.
    f.request(&Request::get(srv.url("/cookie"))).unwrap();
    f.request(&Request::get(srv.url("/after")).with_dest(Destination::Image).with_referer(srv.url("/page?x=1"))).unwrap();
    let last = srv.requests().pop().unwrap();
    assert_eq!(last.header("cookie"), Some("sid=42"));
    assert_eq!(last.header("sec-fetch-dest"), Some("image"));
    assert_eq!(last.header("sec-fetch-site"), Some("same-origin"));
    assert_eq!(last.header("referer"), Some(format!("{}/page?x=1", srv.origin()).as_str()));
    assert_eq!(last.header("accept-encoding"), Some("gzip, deflate, br, zstd"));
    assert!(last.header("user-agent").unwrap().contains("Chrome/"));
}

#[test]
fn duplicate_urls_in_a_batch_are_fetched_once() {
    let srv = serve(Arc::new(|req: &Req| (200, hv(&[("cache-control", "no-store")]), req.path.clone().into_bytes())));
    let f = fetcher();
    let font = srv.url("/icons.woff");
    let other = srv.url("/other.woff");
    let urls = vec![font.clone(), font.clone(), other.clone(), font.clone(), other];
    let res = f.fetch_many_for(&urls, 6, Destination::Font, None);
    assert_eq!(res.len(), 5);
    let bodies: Vec<String> = res.iter().map(|r| String::from_utf8_lossy(&r.as_ref().unwrap().body).into_owned()).collect();
    assert_eq!(bodies, ["/icons.woff", "/icons.woff", "/other.woff", "/icons.woff", "/other.woff"]);
    let hits = srv.requests().iter().filter(|r| r.path == "/icons.woff").count();
    assert_eq!(hits, 1, "one request per distinct URL");
    assert_eq!(srv.requests().len(), 2);
}

#[test]
fn parallel_requests_respect_per_host_limit() {
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let (a, p) = (active.clone(), peak.clone());
    let srv = serve(Arc::new(move |_req: &Req| {
        let n = a.fetch_add(1, Ordering::SeqCst) + 1;
        p.fetch_max(n, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(30));
        a.fetch_sub(1, Ordering::SeqCst);
        (200, hv(&[("cache-control", "no-store")]), b"x".to_vec())
    }));
    let f = fetcher();
    let urls: Vec<Url> = (0..30).map(|i| srv.url(&format!("/r{i}"))).collect();
    let t = std::time::Instant::now();
    let res = f.fetch_many_for(&urls, 8, Destination::Image, None);
    assert!(res.iter().all(|r| r.is_ok()));
    assert!(peak.load(Ordering::SeqCst) <= 6, "peak {}", peak.load(Ordering::SeqCst));
    assert!(peak.load(Ordering::SeqCst) >= 4, "peak {}", peak.load(Ordering::SeqCst));
    // 30 requests * 30 ms / 6 connections ~ 150 ms (not 900 ms).
    assert!(t.elapsed() < Duration::from_millis(700), "{:?}", t.elapsed());
    // Priority: when queued behind the limit, CSS goes before images.
    let order = Arc::new(Mutex::new(Vec::new()));
    let o2 = order.clone();
    let srv = serve(Arc::new(move |req: &Req| {
        o2.lock().unwrap().push(req.path.clone());
        std::thread::sleep(Duration::from_millis(20));
        (200, hv(&[("cache-control", "no-store")]), b"x".to_vec())
    }));
    let mut reqs: Vec<Request> = (0..12).map(|i| Request::get(srv.url(&format!("/img{i}"))).with_dest(Destination::Image)).collect();
    reqs.push(Request::get(srv.url("/style")).with_dest(Destination::Style));
    let _ = f.request_many(&reqs, 1);
    let pos = order.lock().unwrap().iter().position(|p| p == "/style").unwrap();
    // Up to 6 images may already be in flight (the per-host limit) before
    // the CSS request is queued; it must be the next one to start.
    assert!(pos <= 6, "CSS started at position {pos}");
}

#[test]
fn http_cache_revalidation() {
    let srv = serve(Arc::new(|req: &Req| match req.path.as_str() {
        "/etag" => {
            if req.header("if-none-match") == Some("\"v1\"") {
                (304, hv(&[("etag", "\"v1\""), ("cache-control", "max-age=0")]), vec![])
            } else {
                (200, hv(&[("etag", "\"v1\""), ("cache-control", "max-age=0"), ("content-type", "text/css")]), b"body{}".to_vec())
            }
        }
        "/fresh" => (200, hv(&[("cache-control", "public, max-age=600")]), b"fresh".to_vec()),
        "/lm" => {
            if req.header("if-modified-since").is_some() {
                (304, vec![], vec![])
            } else {
                (200, hv(&[("last-modified", "Mon, 01 Jan 2001 00:00:00 GMT"), ("cache-control", "no-cache")]), b"lm".to_vec())
            }
        }
        "/nostore" => (200, hv(&[("cache-control", "no-store")]), b"n".to_vec()),
        "/vary" => {
            let lang = req.header("accept-language").unwrap_or("").to_string();
            (200, hv(&[("cache-control", "max-age=600"), ("vary", "Accept-Language")]), lang.into_bytes())
        }
        "/post" => (303, hv(&[("location", "/fresh")]), vec![]),
        _ => (404, vec![], vec![]),
    }));
    let f = fetcher();
    let count = |p: &str| srv.requests().iter().filter(|r| r.path == p).count();
    // ETag revalidation: second fetch sends If-None-Match, gets 304, body from cache.
    assert_eq!(f.fetch(&srv.url("/etag")).unwrap().body, b"body{}");
    let r = f.fetch(&srv.url("/etag")).unwrap();
    assert_eq!(r.body, b"body{}");
    assert_eq!(r.status, 200);
    assert_eq!(r.content_type, "text/css");
    let reqs: Vec<Req> = srv.requests().into_iter().filter(|r| r.path == "/etag").collect();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[1].header("if-none-match"), Some("\"v1\""));
    // Fresh: served from cache without a request.
    f.fetch(&srv.url("/fresh")).unwrap();
    assert_eq!(f.fetch(&srv.url("/fresh")).unwrap().body, b"fresh");
    assert_eq!(count("/fresh"), 1);
    // Reload mode bypasses; no-store mode does not store.
    f.request(&Request::get(srv.url("/fresh")).with_cache_mode(CacheMode::Reload)).unwrap();
    assert_eq!(count("/fresh"), 2);
    // Last-Modified + no-cache: always revalidated.
    f.fetch(&srv.url("/lm")).unwrap();
    assert_eq!(f.fetch(&srv.url("/lm")).unwrap().body, b"lm");
    assert!(srv.requests().iter().filter(|r| r.path == "/lm").nth(1).unwrap().header("if-modified-since").is_some());
    // no-store never cached.
    f.fetch(&srv.url("/nostore")).unwrap();
    f.fetch(&srv.url("/nostore")).unwrap();
    assert_eq!(count("/nostore"), 2);
    // Vary: distinct entries per Accept-Language.
    let mut es = f.clone();
    es.accept_language = "es".into();
    let mut en = f.clone();
    en.accept_language = "en".into();
    assert_eq!(es.fetch(&srv.url("/vary")).unwrap().body, b"es");
    assert_eq!(en.fetch(&srv.url("/vary")).unwrap().body, b"en");
    assert_eq!(es.fetch(&srv.url("/vary")).unwrap().body, b"es");
    assert_eq!(count("/vary"), 2);
    // A POST invalidates; its 303 is followed with GET.
    let r = f.request(&Request::post(srv.url("/post"), Body::text("text/plain", "x"))).unwrap();
    assert_eq!(r.body, b"fresh");
    // Disk cache in a temporary directory survives a new context.
    let dir = std::env::temp_dir().join(format!("rh-net-disk-{}", std::process::id()));
    let mk = || {
        let mut c = HttpCache::with_disk(1 << 20, &dir, 1 << 24);
        c.persist_loopback = true;
        Arc::new(NetContext::new(Some(Arc::new(c))))
    };
    let f1 = fetcher().with_context(mk());
    f1.fetch(&srv.url("/fresh")).unwrap();
    let before = count("/fresh");
    let f2 = fetcher().with_context(mk());
    assert_eq!(f2.fetch(&srv.url("/fresh")).unwrap().body, b"fresh");
    assert_eq!(count("/fresh"), before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cors_semantics() {
    let page = serve(Arc::new(|_req: &Req| (200, vec![], b"page".to_vec())));
    let api = serve(Arc::new(|req: &Req| {
        let origin = req.header("origin").unwrap_or("").to_string();
        match req.path.as_str() {
            "/open" => (200, hv(&[("access-control-allow-origin", "*"), ("x-hidden", "1"), ("x-shown", "2"), ("access-control-expose-headers", "X-Shown")]), b"open".to_vec()),
            "/closed" => (200, vec![], b"secret".to_vec()),
            "/creds" => (200, hv(&[("access-control-allow-origin", &origin), ("access-control-allow-credentials", "true"), ("set-cookie", "c=1; SameSite=None; Secure")]), b"creds".to_vec()),
            "/put" if req.method == "OPTIONS" => (
                204,
                hv(&[("access-control-allow-origin", &origin), ("access-control-allow-methods", "PUT"), ("access-control-allow-headers", "x-token, content-type"), ("access-control-max-age", "60")]),
                vec![],
            ),
            "/put" => (200, hv(&[("access-control-allow-origin", &origin)]), req.body.clone()),
            "/redir" => (302, hv(&[("location", "/open"), ("access-control-allow-origin", "*")]), vec![]),
            _ => (404, vec![], vec![]),
        }
    }));
    let f = fetcher();
    let doc = page.url("/index.html");
    let api_req = |p: &str| Request::get(api.url(p)).with_dest(Destination::Empty).with_referer(doc.clone()).with_mode(RequestMode::Cors, CredentialsMode::SameOrigin);
    // Simple cross-origin GET with ACAO *.
    let r = f.fetch_api(&api_req("/open")).unwrap();
    assert_eq!(r.kind, ResponseType::Cors);
    assert_eq!(r.response.body, b"open");
    assert!(r.response.header("x-shown").is_some() && r.response.header("x-hidden").is_none());
    let sent = api.requests().pop().unwrap();
    assert_eq!(sent.header("origin"), Some(page.origin().as_str()));
    assert_eq!(sent.header("sec-fetch-mode"), Some("cors"));
    // Missing ACAO: network error.
    assert!(matches!(f.fetch_api(&api_req("/closed")), Err(FetchError::Cors(_))));
    // no-cors: opaque.
    let r = f.fetch_api(&api_req("/closed").with_mode(RequestMode::NoCors, CredentialsMode::Include)).unwrap();
    assert_eq!(r.kind, ResponseType::Opaque);
    assert_eq!(r.response.status, 0);
    assert!(r.response.body.is_empty() && r.response.headers.is_empty());
    // same-origin mode to a cross-origin URL fails; same-origin is basic.
    assert!(f.fetch_api(&api_req("/open").with_mode(RequestMode::SameOrigin, CredentialsMode::SameOrigin)).is_err());
    let r = f.fetch_api(&Request::get(page.url("/x")).with_referer(doc.clone()).with_mode(RequestMode::Cors, CredentialsMode::SameOrigin)).unwrap();
    assert_eq!(r.kind, ResponseType::Basic);
    // Credentials: include needs ACAC + exact origin; * is rejected.
    let r = f.fetch_api(&api_req("/creds").with_mode(RequestMode::Cors, CredentialsMode::Include)).unwrap();
    assert_eq!(r.response.body, b"creds");
    assert!(f.fetch_api(&api_req("/open").with_mode(RequestMode::Cors, CredentialsMode::Include)).is_err());
    // Non-simple request: preflight, then the actual PUT; cached after.
    let mut put = api_req("/put").with_header("X-Token", "abc");
    put.method = "PUT".into();
    put.body = Some(Body::text("application/json", "{\"a\":1}"));
    let r = f.fetch_api(&put).unwrap();
    assert_eq!(r.response.body, b"{\"a\":1}");
    let log = api.requests();
    let n = log.len();
    assert_eq!(log[n - 2].method, "OPTIONS");
    assert_eq!(log[n - 2].header("access-control-request-method"), Some("PUT"));
    assert_eq!(log[n - 2].header("access-control-request-headers"), Some("content-type,x-token"));
    assert_eq!(log[n - 1].method, "PUT");
    f.fetch_api(&put).unwrap();
    assert_eq!(api.requests().iter().filter(|r| r.method == "OPTIONS").count(), 1, "preflight cached");
    // Disallowed header: preflight rejects.
    let mut bad = put.clone();
    bad.url = api.url("/put");
    bad.headers.push(("X-Other".into(), "1".into()));
    assert!(matches!(f.fetch_api(&bad), Err(FetchError::Cors(_))));
    // Redirect modes.
    let r = f.fetch_api(&api_req("/redir").with_redirect(RedirectMode::Manual)).unwrap();
    assert_eq!(r.kind, ResponseType::OpaqueRedirect);
    assert_eq!(r.response.status, 0);
    assert!(f.fetch_api(&api_req("/redir").with_redirect(RedirectMode::Error)).is_err());
    let r = f.fetch_api(&api_req("/redir")).unwrap();
    assert!(r.redirected);
    assert_eq!(r.response.body, b"open");
    // Forbidden headers set by script are dropped.
    f.fetch_api(&api_req("/open").with_header("Cookie", "evil=1").with_header("Sec-Fetch-Site", "none")).unwrap();
    let sent = api.requests().pop().unwrap();
    assert_ne!(sent.header("cookie"), Some("evil=1"));
    assert_eq!(sent.header("sec-fetch-site"), Some("same-site"));
}

#[test]
fn referrer_policy_and_origin_on_navigation_post() {
    let srv = serve(Arc::new(|_req: &Req| (200, vec![], b"ok".to_vec())));
    let other = serve(Arc::new(|_req: &Req| (200, vec![], b"ok".to_vec())));
    let f = fetcher();
    let doc = srv.url("/a/page?secret=1");
    // Cross-origin: only the origin (strict-origin-when-cross-origin).
    f.request(&Request::get(other.url("/x")).with_dest(Destination::Image).with_referer(doc.clone())).unwrap();
    assert_eq!(other.requests().pop().unwrap().header("referer"), Some(format!("{}/", srv.origin()).as_str()));
    // no-referrer.
    f.request(&Request::get(other.url("/y")).with_dest(Destination::Image).with_referer(doc.clone()).with_referrer_policy(ReferrerPolicy::NoReferrer)).unwrap();
    assert!(other.requests().pop().unwrap().header("referer").is_none());
    // Form POST navigation sends Origin.
    f.request(&Request::post(other.url("/form"), Body::form(&[("a".into(), "b".into())])).with_referer(doc)).unwrap();
    let r = other.requests().pop().unwrap();
    assert_eq!(r.header("origin"), Some(srv.origin().as_str()));
    assert_eq!(r.body, b"a=b");
    assert_eq!(r.header("sec-fetch-mode"), Some("navigate"));
}

#[test]
fn bot_profile_robots_and_request_log() {
    let srv = serve(Arc::new(|req: &Req| match req.path.as_str() {
        "/robots.txt" => (200, hv(&[("content-type", "text/plain")]), b"User-agent: *\nDisallow: /\n\nUser-agent: RabbitHoleBot\nDisallow: /private\n".to_vec()),
        "/api" => (200, hv(&[("content-type", "application/json")]), b"{\"a\":1}".to_vec()),
        _ => (200, hv(&[("content-type", "text/html")]), b"<p>hi".to_vec()),
    }));
    // Browser profile (default): Chrome UA + client hints, robots ignored.
    let f = fetcher();
    f.request(&Request::get(srv.url("/private/x"))).unwrap();
    let r = srv.requests().pop().unwrap();
    assert!(r.header("user-agent").unwrap().contains("Chrome/"));
    assert!(r.header("sec-ch-ua").is_some());
    // Bot profile: honest UA, no client hints, robots.txt respected.
    let mut f = fetcher();
    f.set_header_profile(HeaderProfile::Bot);
    let log = NetLog::new();
    f.log = Some(log.clone());
    f.request(&Request::get(srv.url("/public"))).unwrap();
    let r = srv.requests().pop().unwrap();
    let ua = r.header("user-agent").unwrap();
    assert!(ua.starts_with("Mozilla/5.0 (compatible; RabbitHoleBot/") && ua.contains("RabbitHole/"), "{ua}");
    assert!(r.headers.iter().all(|(k, _)| !k.starts_with("sec-ch-ua")));
    let err = f.request(&Request::get(srv.url("/private/page"))).unwrap_err();
    assert!(matches!(err, FetchError::Robots(_)), "{err}");
    assert!(err.to_string().contains("robots.txt"));
    // Subresources and API calls are not subject to robots.txt.
    f.request(&Request::get(srv.url("/private/img")).with_dest(Destination::Image)).unwrap();
    let robots_fetches = srv.requests().iter().filter(|r| r.path == "/robots.txt").count();
    assert_eq!(robots_fetches, 1, "robots.txt is cached per origin");
    // The log has method/url/status/type/size and keeps API bodies.
    f.fetch_api(&Request::get(srv.url("/api")).with_referer(srv.url("/public"))).unwrap();
    let entries = log.entries();
    let api = entries.iter().find(|e| e.url.ends_with("/api")).unwrap();
    assert_eq!(api.kind(), "fetch");
    assert_eq!(api.status, Some(200));
    assert_eq!(api.size, Some(7));
    assert_eq!(api.body.as_deref().map(|b| b.as_slice()), Some(&b"{\"a\":1}"[..]));
    assert!(entries.iter().any(|e| e.url.ends_with("/public") && e.kind() == "document"));
    assert!(log.in_flight().is_empty());
    // Switching back restores the browser identity.
    f.set_header_profile(HeaderProfile::Browser);
    f.request(&Request::get(srv.url("/private/page"))).unwrap();
}

// ------------------------------------------------------------ websocket

fn ws_server() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { continue };
            std::thread::spawn(move || {
                let mut w = s.try_clone().unwrap();
                let mut r = BufReader::new(s);
                let req = read_h1_request(&mut r).unwrap();
                let key = req.header("sec-websocket-key").unwrap().to_string();
                let proto = req.header("sec-websocket-protocol").map(|p| p.split(',').next().unwrap().trim().to_string());
                let mut resp = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n", accept_key(&key));
                if let Some(p) = &proto {
                    resp.push_str(&format!("Sec-WebSocket-Protocol: {p}\r\n"));
                }
                resp.push_str("\r\n");
                w.write_all(resp.as_bytes()).unwrap();
                // Server frames are unmasked: build them by hand.
                let send = |w: &mut TcpStream, op: u8, data: &[u8]| {
                    let mut f = vec![0x80 | op];
                    if data.len() < 126 {
                        f.push(data.len() as u8);
                    } else if data.len() <= 0xffff {
                        f.push(126);
                        f.extend_from_slice(&(data.len() as u16).to_be_bytes());
                    } else {
                        f.push(127);
                        f.extend_from_slice(&(data.len() as u64).to_be_bytes());
                    }
                    f.extend_from_slice(data);
                    w.write_all(&f).unwrap();
                };
                send(&mut w, 0x9, b"ping!");
                loop {
                    let Ok(fr) = read_frame(&mut r, 1 << 20) else { break };
                    match fr.opcode {
                        0x1 | 0x2 => send(&mut w, fr.opcode, &fr.payload),
                        0xA => send(&mut w, 0x1, b"got pong"),
                        0x8 => {
                            send(&mut w, 0x8, &fr.payload);
                            break;
                        }
                        _ => {}
                    }
                }
            });
        }
    });
    port
}

#[test]
fn websocket_echo() {
    let port = ws_server();
    let f = fetcher();
    let url = Url::parse_absolute(&format!("ws://127.0.0.1:{port}/chat")).unwrap();
    let page = Url::parse_absolute(&format!("http://127.0.0.1:{port}/")).unwrap();
    let ws = WebSocket::connect(&f, &url, &["chat".to_string(), "superchat".to_string()], Some(&page), None).unwrap();
    assert_eq!(ws.ready_state(), rabbithole_net::websocket::CONNECTING);
    assert!(ws.send_text("too early").is_err());
    let t = Duration::from_secs(5);
    assert_eq!(ws.next_event(t), Some(WsEvent::Open { protocol: "chat".into(), extensions: String::new() }));
    assert_eq!(ws.protocol(), "chat");
    // The server pinged us; our pong triggers a text reply.
    assert_eq!(ws.next_event(t), Some(WsEvent::Message(WsMessage::Text("got pong".into()))));
    ws.send_text("héllo").unwrap();
    assert_eq!(ws.next_event(t), Some(WsEvent::Message(WsMessage::Text("héllo".into()))));
    let big: Vec<u8> = (0..70000u32).map(|i| i as u8).collect();
    ws.send_binary(&big).unwrap();
    assert_eq!(ws.next_event(t), Some(WsEvent::Message(WsMessage::Binary(big))));
    assert!(ws.close(Some(1001), "").is_err(), "1001 is reserved for user agents");
    ws.close(Some(1000), "bye").unwrap();
    assert_eq!(ws.next_event(t), Some(WsEvent::Close { code: 1000, reason: "bye".into(), was_clean: true }));
    assert_eq!(ws.ready_state(), rabbithole_net::websocket::CLOSED);
    assert_eq!(ws.buffered_amount(), 0);
    // A failed handshake: Error then Close 1006.
    let srv = serve(Arc::new(|_r: &Req| (404, vec![], vec![])));
    let bad = Url::parse_absolute(&format!("ws://127.0.0.1:{}/", srv.port)).unwrap();
    let ws = WebSocket::connect(&f, &bad, &[], None, None).unwrap();
    assert!(matches!(ws.next_event(t), Some(WsEvent::Error(_))));
    assert!(matches!(ws.next_event(t), Some(WsEvent::Close { code: 1006, was_clean: false, .. })));
    assert!(WebSocket::connect(&f, &Url::parse_absolute("ftp://x/").unwrap(), &[], None, None).is_err());
}

// ---------------------------------------------------------- eventsource

#[test]
fn event_source_stream_and_reconnect() {
    let srv = serve(Arc::new(|req: &Req| {
        if req.path == "/bad" {
            return (200, hv(&[("content-type", "text/plain")]), b"no".to_vec());
        }
        let body = if req.header("last-event-id") == Some("2") { "data: resumed\n\n".to_string() } else { "retry: 100\n: hi\nid: 1\ndata: one\n\nevent: custom\nid: 2\ndata: two\ndata: lines\n\n".to_string() };
        (200, hv(&[("content-type", "text/event-stream"), ("connection", "close"), ("cache-control", "no-cache")]), body.into_bytes())
    }));
    let f = fetcher();
    let page = srv.url("/page");
    let es = EventSource::connect(&f, &srv.url("/events"), false, Some(&page), None).unwrap();
    let t = Duration::from_secs(5);
    assert_eq!(es.next_event(t), Some(SseEvent::Open));
    let origin = srv.origin();
    assert_eq!(es.next_event(t), Some(SseEvent::Message { event: "message".into(), data: "one".into(), last_event_id: "1".into(), origin: origin.clone() }));
    assert_eq!(es.next_event(t), Some(SseEvent::Message { event: "custom".into(), data: "two\nlines".into(), last_event_id: "2".into(), origin: origin.clone() }));
    assert!(matches!(es.next_event(t), Some(SseEvent::Error { reconnecting: true, .. })));
    assert_eq!(es.next_event(t), Some(SseEvent::Open));
    assert!(matches!(es.next_event(t), Some(SseEvent::Message { ref data, .. }) if data == "resumed"));
    es.close();
    assert_eq!(es.ready_state(), rabbithole_net::eventsource::CLOSED);
    let reqs = srv.requests();
    assert_eq!(reqs[0].header("accept"), Some("text/event-stream"));
    // Wrong MIME type: error without reconnection.
    let es = EventSource::connect(&f, &srv.url("/bad"), false, Some(&page), None).unwrap();
    assert!(matches!(es.next_event(t), Some(SseEvent::Error { reconnecting: false, .. })));
}

// ------------------------------------------------------------ h2 server

#[derive(Clone, Copy, PartialEq)]
enum H2Mode {
    Normal,
    /// GOAWAY after the first response.
    GoAway,
    /// Send a protocol violation right away.
    Broken,
}

struct H2Server {
    port: u16,
    conns: Arc<AtomicUsize>,
    h1_requests: Arc<AtomicUsize>,
}

fn h2_server(mode: H2Mode) -> H2Server {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let conns = Arc::new(AtomicUsize::new(0));
    let h1 = Arc::new(AtomicUsize::new(0));
    let (c2, h1b) = (conns.clone(), h1.clone());
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { continue };
            let n = c2.fetch_add(1, Ordering::SeqCst);
            let h1c = h1b.clone();
            std::thread::spawn(move || h2_conn(s, mode, n, h1c));
        }
    });
    H2Server { port, conns, h1_requests: h1 }
}

fn h2_conn(s: TcpStream, mode: H2Mode, conn_no: usize, h1: Arc<AtomicUsize>) {
    let mut w = s.try_clone().unwrap();
    let mut r = BufReader::new(s);
    let mut pre = [0u8; 24];
    {
        let buf = r.fill_buf().unwrap();
        if !buf.starts_with(b"PRI") {
            // HTTP/1.1 fallback connection.
            while let Some(req) = read_h1_request(&mut r) {
                h1.fetch_add(1, Ordering::SeqCst);
                write_h1_response(&mut w, 200, &hv(&[("x-proto", "h1")]), format!("h1 {}", req.path).as_bytes(), false);
            }
            return;
        }
    }
    r.read_exact(&mut pre).unwrap();
    assert_eq!(&pre, frame::PREFACE);
    let send = |w: &mut TcpStream, f: frame::Frame| {
        let _ = w.write_all(&f.to_bytes());
    };
    if mode == H2Mode::Broken {
        send(&mut w, frame::Frame::new(frame::DATA, 0, 0, b"bad".to_vec()));
        std::thread::sleep(Duration::from_millis(200));
        return;
    }
    send(&mut w, frame::settings_frame(&[(frame::SETTINGS_MAX_CONCURRENT_STREAMS, 50), (frame::SETTINGS_HEADER_TABLE_SIZE, 1024)]));
    let mut dec = hpack::Decoder::new(65536);
    let mut enc = hpack::Encoder::new(4096);
    let mut pending: std::collections::HashMap<u32, (Vec<(String, String)>, Vec<u8>)> = Default::default();
    let mut block: Option<(u32, bool, Vec<u8>)> = None;
    let mut served = 0;
    loop {
        let Ok(f) = frame::Frame::read(&mut r, 1 << 20) else { return };
        let mut complete: Option<u32> = None;
        match f.kind {
            frame::SETTINGS if !f.has(frame::FLAG_ACK) => send(&mut w, frame::Frame::new(frame::SETTINGS, frame::FLAG_ACK, 0, vec![])),
            frame::HEADERS | frame::CONTINUATION => {
                let (id, end, mut b) = match block.take() {
                    Some(x) => x,
                    None => (f.stream, f.has(frame::FLAG_END_STREAM), Vec::new()),
                };
                b.extend_from_slice(if f.kind == frame::HEADERS { f.header_block_fragment().unwrap() } else { &f.payload });
                if f.has(frame::FLAG_END_HEADERS) {
                    let hs = dec.decode(&b).expect("hpack");
                    pending.insert(id, (hs, Vec::new()));
                    if end {
                        complete = Some(id);
                    }
                } else {
                    block = Some((id, end, b));
                }
            }
            frame::DATA => {
                let len = f.payload.len() as u32;
                if let Some(p) = pending.get_mut(&f.stream) {
                    p.1.extend_from_slice(&f.payload);
                }
                if len > 0 {
                    send(&mut w, frame::window_update(0, len));
                    send(&mut w, frame::window_update(f.stream, len));
                }
                if f.has(frame::FLAG_END_STREAM) {
                    complete = Some(f.stream);
                }
            }
            frame::PING if !f.has(frame::FLAG_ACK) => send(&mut w, frame::Frame::new(frame::PING, frame::FLAG_ACK, 0, f.payload.clone())),
            _ => {}
        }
        let Some(id) = complete else { continue };
        let (hs, body) = pending.remove(&id).unwrap();
        let path = hs.iter().find(|(k, _)| k == ":path").unwrap().1.clone();
        let method = hs.iter().find(|(k, _)| k == ":method").unwrap().1.clone();
        let resp_body: Vec<u8> = match path.as_str() {
            "/big" => (0..300_000u32).map(|i| (i % 251) as u8).collect(),
            "/echo" => body.clone(),
            p => format!("{method} {p} conn{conn_no}").into_bytes(),
        };
        let mut rh = vec![(":status".to_string(), "200".to_string()), ("x-proto".to_string(), "h2".to_string()), ("content-length".to_string(), resp_body.len().to_string())];
        if path == "/bigheaders" {
            rh.push(("x-big".into(), "v".repeat(40_000)));
        }
        let blk = enc.encode(&rh);
        let mut out = Vec::new();
        frame::header_frames(id, &blk, false, 16384, None, &mut out);
        let _ = w.write_all(&out);
        for (i, chunk) in resp_body.chunks(16384).enumerate() {
            let last = (i + 1) * 16384 >= resp_body.len();
            send(&mut w, frame::Frame::new(frame::DATA, if last { frame::FLAG_END_STREAM } else { 0 }, id, chunk.to_vec()));
        }
        if resp_body.is_empty() {
            send(&mut w, frame::Frame::new(frame::DATA, frame::FLAG_END_STREAM, id, vec![]));
        }
        served += 1;
        if mode == H2Mode::GoAway && served == 1 {
            send(&mut w, frame::goaway(id, frame::NO_ERROR, b""));
            std::thread::sleep(Duration::from_millis(100));
            return;
        }
    }
}

fn h2_fetcher(port: u16) -> Fetcher {
    let f = fetcher();
    f.set_h2_prior_knowledge(&format!("http://127.0.0.1:{port}"));
    f
}

#[test]
fn http2_multiplexing_flow_control_continuation() {
    let srv = h2_server(H2Mode::Normal);
    let f = h2_fetcher(srv.port);
    let base = format!("http://127.0.0.1:{}", srv.port);
    let urls: Vec<Url> = (0..40).map(|i| Url::parse_absolute(&format!("{base}/r{i}")).unwrap()).collect();
    let res = f.fetch_many_for(&urls, 8, Destination::Image, None);
    for (i, r) in res.iter().enumerate() {
        let r = r.as_ref().unwrap();
        assert_eq!(String::from_utf8_lossy(&r.body), format!("GET /r{i} conn0"));
        assert_eq!(r.header("x-proto"), Some("h2"));
    }
    assert_eq!(srv.conns.load(Ordering::SeqCst), 1, "all streams on one connection: {}", f.stats());
    // Large response body (receive flow control) and large request body
    // (send flow control: the server's initial window is 65535).
    let big = f.fetch(&Url::parse_absolute(&format!("{base}/big")).unwrap()).unwrap();
    assert_eq!(big.body.len(), 300_000);
    assert!(big.body.iter().enumerate().all(|(i, b)| *b == (i % 251) as u8));
    let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 7) as u8).collect();
    let echo = f.request(&Request::post(Url::parse_absolute(&format!("{base}/echo")).unwrap(), Body { content_type: "application/octet-stream".into(), data: payload.clone() })).unwrap();
    assert_eq!(echo.body, payload);
    // Header block split over CONTINUATION frames.
    let bh = f.fetch(&Url::parse_absolute(&format!("{base}/bigheaders")).unwrap()).unwrap();
    assert_eq!(bh.header("x-big").map(|v| v.len()), Some(40_000));
    assert!(f.stats().contains("h2 requests"), "{}", f.stats());
}

#[test]
fn http2_goaway_retries_on_new_connection() {
    let srv = h2_server(H2Mode::GoAway);
    let f = h2_fetcher(srv.port);
    let base = format!("http://127.0.0.1:{}", srv.port);
    for i in 0..4 {
        let r = f.fetch(&Url::parse_absolute(&format!("{base}/g{i}")).unwrap()).unwrap();
        assert!(String::from_utf8_lossy(&r.body).starts_with(&format!("GET /g{i} conn")));
    }
    assert!(srv.conns.load(Ordering::SeqCst) >= 4);
}

#[test]
fn http2_protocol_error_falls_back_to_http1() {
    let srv = h2_server(H2Mode::Broken);
    let f = h2_fetcher(srv.port);
    let r = f.fetch(&Url::parse_absolute(&format!("http://127.0.0.1:{}/x", srv.port)).unwrap()).unwrap();
    assert_eq!(r.body, b"h1 /x");
    assert_eq!(r.header("x-proto"), Some("h1"));
    assert_eq!(srv.h1_requests.load(Ordering::SeqCst), 1);
    assert!(f.stats().contains("h2 fallbacks 1"), "{}", f.stats());
    // Later requests go straight to HTTP/1.1.
    f.fetch(&Url::parse_absolute(&format!("http://127.0.0.1:{}/y", srv.port)).unwrap()).unwrap();
    assert_eq!(srv.h1_requests.load(Ordering::SeqCst), 2);
}

// ----------------------------------------------------------- live tests

fn live(url: &str) -> (Response, String) {
    let f = Fetcher::isolated();
    let r = f.fetch(&Url::parse_absolute(url).unwrap()).unwrap_or_else(|e| panic!("{url}: {e}"));
    (r, f.stats())
}

#[test]
#[ignore]
fn live_http2_google() {
    let (r, stats) = live("https://www.google.com/");
    assert_eq!(r.status, 200);
    assert!(String::from_utf8_lossy(&r.body).to_ascii_lowercase().contains("<html"));
    assert!(stats.contains("h2 requests 1"), "{stats}");
}

#[test]
#[ignore]
fn live_http2_cloudflare_and_wikipedia() {
    for u in ["https://www.cloudflare.com/", "https://en.wikipedia.org/wiki/HTTP/2"] {
        let (r, stats) = live(u);
        assert_eq!(r.status, 200, "{u}");
        assert!(r.body.len() > 10_000, "{u}");
        assert!(!stats.contains("h2 requests 0"), "{u}: {stats}");
    }
}

#[test]
#[ignore]
fn live_websocket_echo() {
    let f = Fetcher::isolated();
    let ws = WebSocket::connect(&f, &Url::parse_absolute("wss://echo.websocket.org/").unwrap(), &[], None, None).unwrap();
    let t = Duration::from_secs(10);
    assert!(matches!(ws.next_event(t), Some(WsEvent::Open { .. })));
    // The server greets first, then echoes.
    let _greeting = ws.next_event(t);
    ws.send_text("rabbithole").unwrap();
    assert_eq!(ws.next_event(t), Some(WsEvent::Message(WsMessage::Text("rabbithole".into()))));
}

#[test]
fn origin_only_initiator_preserves_cors_and_samesite_without_a_referrer() {
    // This source is inert metadata; only the loopback destination is contacted.
    let source = Url::parse_absolute("http://source.example.test/").unwrap();
    let origin = source.origin();
    let allow = origin.clone();
    let server = serve(Arc::new(move |req: &Req| match req.path.as_str() {
        "/set" => (200, hv(&[("set-cookie", "strict=one; SameSite=Strict; Path=/")]), b"set".to_vec()),
        "/allowed" => (200, hv(&[("access-control-allow-origin", &allow), ("access-control-allow-credentials", "true")]), b"allowed".to_vec()),
        _ => (200, vec![], b"closed".to_vec()),
    }));
    let f = fetcher();
    f.request(&Request::get(server.url("/set"))).unwrap();
    let mut request = Request::get(server.url("/allowed")).with_dest(Destination::Empty).with_mode(RequestMode::Cors, CredentialsMode::Include);
    request.initiator = Some(source);
    request.referrer_policy = ReferrerPolicy::NoReferrer;
    assert_eq!(f.fetch_api(&request).unwrap().kind, ResponseType::Cors);
    let sent = server.requests().pop().unwrap();
    assert_eq!(sent.header("origin"), Some(origin.as_str()));
    assert_eq!(sent.header("sec-fetch-site"), Some("cross-site"));
    assert_eq!(sent.header("referer"), None);
    assert_eq!(sent.header("cookie"), None, "NoReferrer must not turn cross-site into a browser-initiated same-site request");
    request.url = server.url("/closed");
    assert!(matches!(f.fetch_api(&request), Err(FetchError::Cors(_))), "missing referrer must not bypass CORS when an initiator exists");
    f.request(&Request::get(server.url("/baseline"))).unwrap();
    assert_eq!(server.requests().pop().unwrap().header("cookie"), Some("strict=one"), "positive control: Strict cookie was stored and same-site requests still send it");
}
