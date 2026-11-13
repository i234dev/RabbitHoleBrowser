//! Native stand-ins for the wptserve Python handlers (`*.py`) that tests
//! in the directories we run use most.

use crate::server::{Config, Request, Response};

/// A response for `req` when a native handler covers its path.
pub fn handle(req: &Request, cfg: &Config) -> Option<Response> {
    let name = req.path.rsplit('/').next().unwrap_or("");
    if !name.ends_with(".py") {
        return None;
    }
    match req.path.as_str() {
        "/common/slow.py" => Some(slow(req)),
        "/html/syntax/speculative-parsing/resources/stash.py" => Some(speculative_stash(req, cfg)),
        "/FileAPI/file/resources/echo-content-escaped.py" => Some(echo_content(req, true)),
        "/fetch/api/resources/echo-content.py" => Some(echo_content(req, false)),
        _ => None,
    }
}

/// `/common/slow.py?delay=MS`: an empty response after `delay` ms
/// (default 2 s).
fn slow(req: &Request) -> Response {
    let ms = req.get("delay").and_then(|d| d.parse::<f64>().ok()).unwrap_or(2000.0);
    std::thread::sleep(std::time::Duration::from_millis(ms.clamp(0.0, 30_000.0) as u64));
    Response::new(200, "", Vec::new())
}

/// The speculative parsing tests' stash: `action=put` records the request's
/// `Origin` / `Accept` / `Referer` headers (and the raw `encodingcheck`
/// parameter) under `uuid`; `action=take` returns and forgets them.
fn speculative_stash(req: &Request, cfg: &Config) -> Response {
    let uuid = req.get("uuid").unwrap_or_default();
    let mut stash = cfg.stash.lock().unwrap_or_else(|e| e.into_inner());
    if req.get("action").as_deref() == Some("put") {
        let check = req.query.split_once("&encodingcheck=").map(|(_, v)| v).unwrap_or("");
        let mut lines = vec![format!("param-encodingcheck: {check}")];
        for (k, v) in &req.headers {
            if matches!(k.as_str(), "Origin" | "Accept" | "Referer") || ["origin", "accept", "referer"].contains(&k.to_ascii_lowercase().as_str()) {
                lines.push(format!("{k}: {v}"));
            }
        }
        stash.insert(format!("speculative:{uuid}"), lines.join("\r\n"));
        return Response::new(200, "text/plain", Vec::new());
    }
    let v = stash.remove(&format!("speculative:{uuid}")).unwrap_or_default();
    Response::new(200, "text/plain", v)
}

/// `echo-content.py` / `echo-content-escaped.py`: the request body back
/// (escaped: control and non-ASCII bytes as `\xNN`, backslashes doubled,
/// CRLF kept), with the request method and body headers as `X-Request-*`.
fn echo_content(req: &Request, escaped: bool) -> Response {
    let body = if escaped {
        let mut out: Vec<u8> = Vec::with_capacity(req.body.len());
        let b = &req.body;
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            if c == b'\r' && b.get(i + 1) == Some(&b'\n') {
                out.extend_from_slice(b"\r\n");
                i += 2;
                continue;
            }
            if c <= 0x1f || c >= 0x7f {
                out.extend_from_slice(format!("\\x{c:02x}").as_bytes());
            } else if c == b'\\' {
                out.extend_from_slice(b"\\\\");
            } else {
                out.push(c);
            }
            i += 1;
        }
        out
    } else {
        req.body.clone()
    };
    let mut r = Response::new(200, if escaped { "text/plain; charset=UTF-8" } else { "text/plain" }, body);
    r.add_header("X-Request-Method", &req.method);
    r.add_header("X-Request-Content-Length", req.header("content-length").unwrap_or("NO"));
    r.add_header("X-Request-Content-Type", req.header("content-type").unwrap_or("NO"));
    r
}
