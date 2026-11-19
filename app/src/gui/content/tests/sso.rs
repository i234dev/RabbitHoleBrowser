//! A Cloudflare Access-like single sign-on through the GUI's real content
//! thread: typed URL -> app -> 302 to the access team domain (whose 302
//! sets the CSRF cookie) -> 302 to the identity provider -> sign-in form
//! -> auto-submitted cross-site `form_post` to the callback -> 302 back to
//! the app. Three sites on one loopback server (`*.localhost`).
use super::*;
use std::collections::HashMap;
use std::io::BufRead;
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    host: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
}

struct Sites {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
}

impl Drop for Sites {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn respond(port: u16, req: &Seen) -> (u16, Vec<(String, String)>, String) {
    let cookie = req.headers.get("cookie").cloned().unwrap_or_default();
    let has = |pair: &str| cookie.split("; ").any(|c| c == pair);
    let html = |body: String| (200, vec![("Content-Type".to_string(), "text/html; charset=utf-8".to_string())], body);
    let redirect = |to: String, cookies: &[&str]| {
        let mut h = vec![("Location".to_string(), to)];
        h.extend(cookies.iter().map(|c| ("Set-Cookie".to_string(), c.to_string())));
        (302, h, String::new())
    };
    let team = format!("http://team.access.localhost:{port}");
    let idp = format!("http://accounts.idp.localhost:{port}");
    let app = format!("http://app.localhost:{port}");
    match (req.host.as_str(), req.path.split('?').next().unwrap_or("")) {
        ("app.localhost", "/") if has("CF_Authorization=app-token") => html("<!doctype html><title>Welcome</title><p id=who>in".into()),
        ("app.localhost", "/") => redirect(format!("{team}/cdn-cgi/access/login/app.localhost?redirect_url=%2F"), &[]),
        ("app.localhost", "/cdn-cgi/access/authorized") => redirect(format!("{app}/"), &["CF_Authorization=app-token; Path=/; HttpOnly; Secure; SameSite=Lax"]),
        ("team.access.localhost", "/cdn-cgi/access/login/app.localhost") => redirect(
            format!("{idp}/o/oauth2/auth?state=1234&response_mode=form_post"),
            &["CF_Session=\"csrf|1234\"; Path=/; Max-Age=14400; HttpOnly; Secure; SameSite=None", "CF_Device=d; Path=/; Max-Age=86400; HttpOnly; Secure; SameSite=Strict"],
        ),
        ("team.access.localhost", "/cdn-cgi/access/callback") if req.method == "POST" && has("CF_Session=\"csrf|1234\"") && req.body.contains("state=1234") => {
            redirect(format!("{app}/cdn-cgi/access/authorized?token=t"), &["CF_Authorization=team-token; Path=/; HttpOnly; Secure; SameSite=None"])
        }
        ("team.access.localhost", "/cdn-cgi/access/callback") => (403, vec![("Content-Type".into(), "text/html".into())], "<!doctype html><title>Forbidden</title>no CSRF value available in the session cookie".into()),
        // The IdP: a sign-in page (same-origin POST), then the form_post
        // response page that submits itself on load.
        ("accounts.idp.localhost", "/o/oauth2/auth") => html(
            "<!doctype html><title>Sign in</title><form id=f method=post action=/signin><input name=user value=me><input type=password name=pw value=secret><input type=hidden name=state value=1234></form>".into(),
        ),
        ("accounts.idp.localhost", "/signin") if req.method == "POST" => (
            200,
            vec![("Content-Type".into(), "text/html".into()), ("Set-Cookie".into(), "SID=idp; Path=/; Secure; HttpOnly; SameSite=Lax".into())],
            format!("<!doctype html><title>Redirecting</title><body onload=\"document.forms[0].submit()\"><form method=post action=\"{team}/cdn-cgi/access/callback\"><input type=hidden name=code value=c0de><input type=hidden name=state value=1234></form>"),
        ),
        _ => (404, vec![("Content-Type".into(), "text/plain".into())], "not found".into()),
    }
}

fn sites() -> Sites {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let (log, halt) = (seen.clone(), stop.clone());
    std::thread::spawn(move || {
        while !halt.load(Ordering::Relaxed) {
            let Ok((stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            };
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            if reader.read_line(&mut first).is_err() {
                continue;
            }
            let mut parts = first.split_whitespace();
            let method = parts.next().unwrap_or("").to_string();
            let path = parts.next().unwrap_or("/").to_string();
            let mut headers = HashMap::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some((k, v)) = line.split_once(':') {
                    headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                }
            }
            let len = headers.get("content-length").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
            let mut body = vec![0u8; len];
            let _ = reader.read_exact(&mut body);
            let host = headers.get("host").cloned().unwrap_or_default().split(':').next().unwrap_or("").to_string();
            let req = Seen { method, host, path, headers, body: String::from_utf8_lossy(&body).into_owned() };
            log.lock().unwrap().push(req.clone());
            let (status, extra, body) = respond(port, &req);
            let mut out = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n", body.len());
            for (k, v) in extra {
                out.push_str(&format!("{k}: {v}\r\n"));
            }
            out.push_str("\r\n");
            out.push_str(&body);
            let mut stream = stream;
            let _ = stream.write_all(out.as_bytes());
        }
    });
    Sites { port, seen, stop }
}

/// Play the UI thread: page-initiated navigations come back as
/// `FromContent::Navigate` and are sent to the content thread again (as
/// `Browser::navigate_request` does), until a document titled `title`
/// commits.
fn drive_until(h: &ContentHandle, rx: &Received, tab: u64, title: &str) -> Vec<String> {
    let mut titles = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let Some((_, msg)) = wait_for(rx, tab, left, |m| matches!(m, FromContent::Navigate { .. } | FromContent::Committed { .. })) else {
            panic!("no commit of {title:?}; committed so far: {titles:?}");
        };
        match msg {
            FromContent::Navigate { request, push_history } => h.send(ToContent::Navigate { request, push_history }),
            FromContent::Committed { title: t, .. } => {
                titles.push(t.clone());
                if t == title {
                    return titles;
                }
                assert_ne!(t, "Forbidden", "callback rejected the request: {titles:?}");
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn gui_sso_cross_site_form_post_callback_keeps_session_cookie() {
    let s = sites();
    let jar = Arc::new(Mutex::new(rabbithole_engine::net::CookieJar::new()));
    let mut settings = config(ViewSize::new(800, 600, 1.0));
    settings.loader.fetcher.cookies = Some(jar.clone());
    let (sink, rx) = sink();
    let h = ContentHandle::spawn(91, 1, settings, sink);
    h.send(ToContent::SetActive(true));
    // Typed URL: no referrer, no initiator.
    h.send(ToContent::Navigate { request: Request::get(url(&format!("http://app.localhost:{}/", s.port))), push_history: true });
    drive_until(&h, &rx, 91, "Sign in");
    assert!(jar.lock().unwrap().cookies().iter().any(|c| c.name == "CF_Session" && c.value == "\"csrf|1234\""), "Set-Cookie of a 302 is stored before following it");
    // HttpOnly cookies are invisible to (and cannot be clobbered by) scripts.
    h.send(ToContent::Eval("document.cookie='SID=forged; path=/'; document.forms.f.submit(); 1".into()));
    let titles = drive_until(&h, &rx, 91, "Welcome");
    assert!(titles.contains(&"Redirecting".to_string()), "{titles:?}");
    let seen = s.seen.lock().unwrap().clone();
    let signin = seen.iter().find(|r| r.path == "/signin").expect("sign-in POST");
    assert_eq!(signin.headers.get("sec-fetch-site").map(String::as_str), Some("same-origin"));
    let callback = seen.iter().find(|r| r.path.starts_with("/cdn-cgi/access/callback")).expect("callback request");
    assert_eq!(callback.method, "POST");
    let cookie = callback.headers.get("cookie").cloned().unwrap_or_default();
    assert!(cookie.contains("CF_Session=\"csrf|1234\""), "SameSite=None cookie on the cross-site form_post: {cookie:?}");
    assert!(!cookie.contains("CF_Device"), "Strict cookies stay off cross-site requests: {cookie:?}");
    assert_eq!(callback.headers.get("origin").cloned(), Some(format!("http://accounts.idp.localhost:{}", s.port)));
    assert_eq!(callback.headers.get("referer").cloned(), Some(format!("http://accounts.idp.localhost:{}/", s.port)), "strict-origin-when-cross-origin");
    assert_eq!(callback.headers.get("sec-fetch-site").map(String::as_str), Some("cross-site"));
    assert!(callback.body.contains("code=c0de") && callback.body.contains("state=1234"), "{}", callback.body);
    let first = &seen[0];
    assert_eq!(first.headers.get("sec-fetch-site").map(String::as_str), Some("none"), "typed navigation");
    assert!(!first.headers.contains_key("referer"));
    let login = seen.iter().find(|r| r.path.starts_with("/cdn-cgi/access/login")).unwrap();
    assert_eq!(login.headers.get("sec-fetch-site").map(String::as_str), Some("none"), "browser-initiated redirect chain");
    let welcome = seen.iter().rev().find(|r| r.host == "app.localhost" && r.path == "/").unwrap();
    assert!(welcome.headers.get("cookie").is_some_and(|c| c.contains("CF_Authorization=app-token")));
    let jar = jar.lock().unwrap();
    let sid: Vec<_> = jar.cookies().iter().filter(|c| c.name == "SID").collect();
    assert_eq!(sid.len(), 1, "document.cookie cannot shadow an HttpOnly cookie");
    assert_eq!(sid[0].value, "idp");
}
