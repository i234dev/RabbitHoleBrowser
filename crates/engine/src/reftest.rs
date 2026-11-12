//! Reftests in the web-platform-tests format: a test page and the
//! reference pages it names with `<link rel=match|mismatch href=…>` are
//! rendered at 800×600 and compared pixel by pixel, within the tolerance
//! of a `<meta name=fuzzy>`; `class="reftest-wait"` on the root element
//! delays the screenshot until the test removes it.
//!
//! WPT tests use root-relative URLs (`/fonts/Ahem.ttf`), so the suite is
//! served over HTTP from its checkout by [`serve_dir`]. The runner that
//! drives whole directories lives in `examples/wpt_reftest.rs`.

use crate::diff::{diff_frames, DiffOptions, Frame};
use crate::{Engine, Viewport};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const WIDTH: f32 = 800.0;
pub const HEIGHT: f32 = 600.0;

/// `match` / `mismatch` reference of a test.
#[derive(Debug, Clone, PartialEq)]
pub struct Reference {
    pub href: String,
    pub mismatch: bool,
}

/// Allowed difference: inclusive ranges of the largest per-channel
/// difference and of the number of differing pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fuzzy {
    pub max_difference: (u32, u32),
    pub total_pixels: (u64, u64),
}

/// What a test file declares.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TestMeta {
    pub references: Vec<Reference>,
    /// Fuzzy annotations, optionally restricted to one reference URL.
    pub fuzzy: Vec<(Option<String>, Fuzzy)>,
}

/// The value of attribute `name` in the attribute text of a tag.
fn attr_value(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut i = 0;
    while let Some(pos) = lower[i..].find(name) {
        let start = i + pos;
        let before_ok = start == 0 || bytes[start - 1].is_ascii_whitespace();
        let mut j = start + name.len();
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if before_ok && j < bytes.len() && bytes[j] == b'=' {
            j += 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            let raw = &tag[j..];
            let v = match raw.chars().next() {
                Some(q @ ('"' | '\'')) => raw[1..].split(q).next().unwrap_or(""),
                _ => raw.split(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/').next().unwrap_or(""),
            };
            return Some(v.to_string());
        }
        i = start + name.len();
    }
    None
}

/// Every `<tag …>` start tag in `src` (attribute text only).
fn start_tags<'a>(src: &'a str, tag: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    let lower = src.to_ascii_lowercase();
    let open = format!("<{tag}");
    let mut found = Vec::new();
    let mut i = 0;
    while let Some(p) = lower[i..].find(&open) {
        let s = i + p + open.len();
        let next = lower.as_bytes().get(s).copied().unwrap_or(b'>');
        if next.is_ascii_whitespace() || next == b'>' || next == b'/' {
            let e = lower[s..].find('>').map(|e| s + e).unwrap_or(lower.len());
            found.push((s, e));
            i = e;
        } else {
            i = s;
        }
    }
    found.into_iter().map(move |(s, e)| &src[s..e])
}

fn parse_range(s: &str) -> Option<(u64, u64)> {
    let s = s.trim();
    match s.split_once('-') {
        Some((a, b)) => Some((a.trim().parse().ok()?, b.trim().parse().ok()?)),
        None => {
            let v = s.parse().ok()?;
            Some((v, v))
        }
    }
}

/// Parse `[url:]maxDifference=a-b;totalPixels=c-d` (names optional).
pub fn parse_fuzzy(content: &str) -> Option<(Option<String>, Fuzzy)> {
    let mut content = content.trim();
    let mut url = None;
    // A reference URL prefix ends with ':' before the first range.
    if let Some(colon) = content.find(':') {
        let head = &content[..colon];
        if !head.contains('=') && !head.contains(';') && head.chars().any(|c| !c.is_ascii_digit() && c != '-') {
            url = Some(head.trim().to_string());
            content = &content[colon + 1..];
        }
    }
    let mut md = None;
    let mut tp = None;
    for (i, part) in content.split(';').enumerate() {
        let (name, val) = match part.split_once('=') {
            Some((n, v)) => (n.trim().to_ascii_lowercase(), v),
            None => (if i == 0 { "maxdifference".into() } else { "totalpixels".into() }, part),
        };
        let r = parse_range(val)?;
        match name.as_str() {
            "maxdifference" => md = Some((r.0 as u32, r.1 as u32)),
            "totalpixels" => tp = Some(r),
            _ => return None,
        }
    }
    Some((url, Fuzzy { max_difference: md.unwrap_or((0, 0)), total_pixels: tp.unwrap_or((0, 0)) }))
}

/// Read the references and fuzzy annotations of a test's source.
pub fn parse_meta(src: &str) -> TestMeta {
    let mut meta = TestMeta::default();
    for tag in start_tags(src, "link") {
        let Some(rel) = attr_value(tag, "rel") else { continue };
        let rel = rel.trim().to_ascii_lowercase();
        if rel != "match" && rel != "mismatch" {
            continue;
        }
        if let Some(href) = attr_value(tag, "href") {
            meta.references.push(Reference { href: href.trim().to_string(), mismatch: rel == "mismatch" });
        }
    }
    for tag in start_tags(src, "meta") {
        if attr_value(tag, "name").is_some_and(|n| n.eq_ignore_ascii_case("fuzzy")) {
            if let Some(f) = attr_value(tag, "content").and_then(|c| parse_fuzzy(&c)) {
                meta.fuzzy.push(f);
            }
        }
    }
    meta
}

/// Does the comparison of a test with one reference pass?
pub fn compare_passes(test: &Frame, reference: &Frame, mismatch: bool, fuzzy: Option<Fuzzy>) -> (bool, u64, u8) {
    let r = diff_frames(test, reference, &DiffOptions { tolerance: 0, merge_distance: 0 });
    let equal = r.mismatched == 0
        || fuzzy.is_some_and(|f| {
            let md = r.max_delta as u32;
            md >= f.max_difference.0 && md <= f.max_difference.1 && r.mismatched >= f.total_pixels.0 && r.mismatched <= f.total_pixels.1
        });
    (equal != mismatch, r.mismatched, r.max_delta)
}

/// Does the root element still have the `reftest-wait` class?
fn waiting(page: &crate::Page) -> bool {
    page.doc
        .html_element()
        .and_then(|e| page.doc.attr(e, "class"))
        .is_some_and(|c| c.split_ascii_whitespace().any(|c| c == "reftest-wait"))
}

/// Load `url`, let it settle (and drop `reftest-wait`, bounded by
/// `max_wait`) and paint the 800×600 viewport.
pub fn render(engine: &mut Engine, url: &str, max_wait: Duration) -> Result<Frame, String> {
    let vp = Viewport { width: WIDTH, height: HEIGHT };
    let mut page = engine.load(url, vp).map_err(|e| e.to_string())?;
    engine.settle_for_snapshot(&mut page, max_wait.min(Duration::from_secs(2)));
    let deadline = Instant::now() + max_wait;
    while waiting(&page) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
        let _ = engine.tick(&mut page);
    }
    let _ = engine.tick(&mut page);
    Ok(engine.snapshot(&page))
}

/// Outcome of one reftest.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Pass,
    /// Differing pixels and largest channel difference against the first
    /// failing reference.
    Fail { pixels: u64, max_delta: u8 },
    /// The test declares no reference we can use.
    Skip(String),
    Error(String),
}

/// Run the reftest at `test_url` (whose source is `src`). References are
/// resolved against the test URL; a `.png` reference is decoded from
/// `fetch_png(url)`.
pub fn run_test(engine: &mut Engine, test_url: &str, src: &str, max_wait: Duration, dump: Option<&Path>) -> Outcome {
    let meta = parse_meta(src);
    if meta.references.is_empty() {
        return Outcome::Skip("no reference".into());
    }
    let base = match rabbithole_net::Url::parse(test_url) {
        Some(u) => u,
        None => return Outcome::Error("bad url".into()),
    };
    let test = match render(engine, test_url, max_wait) {
        Ok(f) => f,
        Err(e) => return Outcome::Error(e),
    };
    for r in &meta.references {
        let Some(ref_url) = base.join(&r.href) else { return Outcome::Error(format!("bad reference {}", r.href)) };
        let ref_url = ref_url.to_string();
        let frame = if ref_url.to_ascii_lowercase().ends_with(".png") {
            match engine.loader.fetcher.fetch(&rabbithole_net::Url::parse(&ref_url).unwrap()) {
                Ok(resp) => match Frame::from_image_bytes(&resp.body) {
                    Ok(f) => f,
                    Err(e) => return Outcome::Error(e),
                },
                Err(e) => return Outcome::Error(e.to_string()),
            }
        } else {
            match render(engine, &ref_url, max_wait) {
                Ok(f) => f,
                Err(e) => return Outcome::Error(format!("reference: {e}")),
            }
        };
        let fuzzy = meta
            .fuzzy
            .iter()
            .find(|(u, _)| u.as_ref().is_some_and(|u| base.join(u).map(|j| j.to_string()) == Some(ref_url.clone())))
            .or_else(|| meta.fuzzy.iter().find(|(u, _)| u.is_none()))
            .map(|(_, f)| *f);
        let (ok, pixels, max_delta) = compare_passes(&test, &frame, r.mismatch, fuzzy);
        if !ok {
            if let Some(dir) = dump {
                let _ = std::fs::create_dir_all(dir);
                let _ = test.save_png(&dir.join("test.png"));
                let _ = frame.save_png(&dir.join("ref.png"));
                let (_, img) = crate::diff::diff_frames_image(&test, &frame, &DiffOptions::default());
                let _ = img.save_png(&dir.join("diff.png"));
            }
            return Outcome::Fail { pixels, max_delta };
        }
    }
    Outcome::Pass
}

/// Is this path (relative to the suite root, `/`-separated) a reftest
/// file candidate (not a reference or support file)?
pub fn is_test_candidate(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or("");
    let ext_ok = [".html", ".htm", ".xht", ".xhtml"].iter().any(|e| name.ends_with(e));
    if !ext_ok {
        return false;
    }
    if lower.split('/').any(|seg| matches!(seg, "reference" | "references" | "support" | "resources" | "tools" | "crashtests")) {
        return false;
    }
    if name.contains("-ref.") || name.contains("-ref-") || name.starts_with("ref-") || name.contains("-notref") || name.contains("-manual.") {
        return false;
    }
    // Server-side substitutions and secure contexts need wptserve.
    !(name.contains(".sub.") || name.contains(".https.") || name.contains(".window.") || name.contains(".any."))
}

/// All reftests below `dir` (relative paths from `root`, sorted).
pub fn find_tests(root: &Path, dir: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.join(dir)];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let Ok(rel) = p.strip_prefix(root) else { continue };
            let rel = rel.to_string_lossy().replace('\\', "/");
            if !is_test_candidate(&rel) {
                continue;
            }
            let Ok(src) = std::fs::read_to_string(&p) else { continue };
            if !parse_meta(&src).references.is_empty() {
                out.push(rel);
            }
        }
    }
    out.sort();
    out
}

// ------------------------------------------------------------ the server

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("xht" | "xhtml") => "application/xhtml+xml",
        Some("css") => "text/css",
        Some("js") => "text/javascript",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("json") => "application/json",
        Some("txt") => "text/plain",
        _ => "application/octet-stream",
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn handle(mut stream: TcpStream, root: &Path) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() {
        return;
    }
    // Drain the headers.
    loop {
        let mut l = String::new();
        match reader.read_line(&mut l) {
            Ok(0) | Err(_) => break,
            Ok(_) if l.trim().is_empty() => break,
            _ => {}
        }
    }
    let target = first.split_whitespace().nth(1).unwrap_or("/");
    let path = target.split(['?', '#']).next().unwrap_or("/");
    let rel = percent_decode(path.trim_start_matches('/'));
    let safe = !rel.split('/').any(|s| s == "..");
    let file: PathBuf = root.join(&rel);
    let (status, body, ct) = match (safe, std::fs::read(&file)) {
        (true, Ok(b)) if file.is_file() => ("200 OK", b, content_type(&file)),
        _ => ("404 Not Found", b"not found".to_vec(), "text/plain"),
    };
    let head = format!("HTTP/1.1 {status}\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n", body.len());
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body);
}

/// Serve the files under `root` on a free localhost port (one thread per
/// connection, for the life of the process). Returns the base URL
/// (`http://127.0.0.1:PORT/`).
pub fn serve_dir(root: PathBuf) -> std::io::Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            let root = root.clone();
            std::thread::spawn(move || handle(s, &root));
        }
    });
    Ok(format!("http://127.0.0.1:{port}/"))
}

/// An engine set up like WPT's reference browsers: system fonts plus the
/// Ahem test font (from `<root>/fonts/Ahem.ttf`) installed as a family.
pub fn test_engine(root: &Path) -> Engine {
    let mut engine = Engine::new();
    if let Ok(data) = std::fs::read(root.join("fonts/Ahem.ttf")) {
        engine.fonts.add_font_data("Ahem", 400, false, &data);
    }
    engine.loader.max_fonts = 32;
    engine
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_links_and_fuzzy() {
        let src = r#"<!DOCTYPE html><link rel="help" href="x"><LINK REL=match href="ref.html"><link href='other-ref.html' rel="mismatch">
            <meta name="fuzzy" content="maxDifference=0-2;totalPixels=0-300"><meta name=fuzzy content="ref.html:10-20;5">"#;
        let m = parse_meta(src);
        assert_eq!(m.references, vec![Reference { href: "ref.html".into(), mismatch: false }, Reference { href: "other-ref.html".into(), mismatch: true }]);
        assert_eq!(m.fuzzy[0], (None, Fuzzy { max_difference: (0, 2), total_pixels: (0, 300) }));
        assert_eq!(m.fuzzy[1], (Some("ref.html".into()), Fuzzy { max_difference: (10, 20), total_pixels: (5, 5) }));
    }

    #[test]
    fn test_candidates() {
        assert!(is_test_candidate("css/css-flexbox/align-content-001.htm"));
        assert!(!is_test_candidate("css/css-flexbox/reference/align-content-001-ref.htm"));
        assert!(!is_test_candidate("css/css-flexbox/x-ref.html"));
        assert!(!is_test_candidate("css/css-flexbox/support/a.html"));
        assert!(!is_test_candidate("css/x/a.sub.html"));
    }
}
