//! Internal `about:` pages generated as HTML.

use crate::bookmarks::Bookmarks;
use crate::history::History;
use crate::profile::Profile;

pub const STYLE: &str = "<style>
body{font-family:sans-serif;margin:0;background:#f8f9fa;color:#202124}
header{background:#1f1b2e;color:#f4efe6;padding:18px 32px;border-bottom:4px solid #e0a84a}
header h1{margin:0;font-size:22px}
main{max-width:900px;margin:0 auto;padding:20px 32px}
h2{font-size:16px;color:#5f6368;margin:24px 0 8px 0;border-bottom:1px solid #dadce0;padding-bottom:4px}
ul{list-style:none;padding:0;margin:0}
li{padding:7px 10px;border-bottom:1px solid #eceff1;background:#fff}
li a{color:#1a0dab;text-decoration:none}
li .u{color:#5f6368;font-size:12px;margin-left:8px}
.muted{color:#5f6368}
.empty{padding:24px;text-align:center;color:#5f6368;background:#fff;border:1px dashed #dadce0}
.bar{height:6px;background:#e8eaed;border-radius:3px;overflow:hidden;margin-top:4px}
.bar div{height:100%;background:#1a73e8}
pre{background:#fff;border:1px solid #dadce0;padding:12px;white-space:pre-wrap;overflow-wrap:anywhere;font-size:13px}
.actions a{color:#c5221f;font-size:12px;margin-left:8px}
table{border-collapse:collapse;width:100%}td{padding:4px 8px;border-bottom:1px solid #eceff1}
</style>";

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn shell(title: &str, body: &str) -> String {
    format!("<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title>{STYLE}</head><body><header><h1>{}</h1></header><main>{body}</main></body></html>", escape(title), escape(title))
}

/// `about:history`: the last visits grouped by day.
pub fn history_page(history: &History) -> String {
    let recent = history.recent(500);
    if recent.is_empty() {
        return shell("History", "<div class=\"empty\">No pages visited yet.</div>");
    }
    let mut body = String::new();
    let mut current_day: Option<u64> = None;
    for e in recent {
        let day = e.timestamp / 86400;
        if current_day != Some(day) {
            if current_day.is_some() {
                body.push_str("</ul>");
            }
            body.push_str(&format!("<h2>{}</h2><ul>", day_label(day)));
            current_day = Some(day);
        }
        let title = if e.title.trim().is_empty() { e.url.as_str() } else { e.title.as_str() };
        body.push_str(&format!(
            "<li><span class=\"muted\">{}</span> &nbsp; <a href=\"{}\">{}</a><span class=\"u\">{}</span></li>",
            time_label(e.timestamp),
            escape(&e.url),
            escape(title),
            escape(&e.url)
        ));
    }
    body.push_str("</ul><p class=\"muted\"><a href=\"rabbithole://history/clear\">Clear history</a></p>");
    shell("History", &body)
}

/// `about:bookmarks`
pub fn bookmarks_page(bookmarks: &Bookmarks) -> String {
    if bookmarks.items.is_empty() {
        return shell("Bookmarks", "<div class=\"empty\">No bookmarks yet. Press Ctrl+D on a page to bookmark it.</div>");
    }
    let mut body = String::from("<ul>");
    for b in &bookmarks.items {
        let title = if b.title.trim().is_empty() { b.url.as_str() } else { b.title.as_str() };
        body.push_str(&format!(
            "<li><a href=\"{0}\">{1}</a><span class=\"u\">{0}</span><span class=\"actions\"><a href=\"rabbithole://bookmark/remove?{2}\">remove</a></span></li>",
            escape(&b.url),
            escape(title),
            escape(&percent_encode(&b.url))
        ));
    }
    body.push_str("</ul>");
    shell("Bookmarks", &body)
}

/// A download row for `about:downloads`.
pub struct DownloadRow {
    pub id: u64,
    pub url: String,
    pub filename: String,
    pub path: String,
    pub done: u64,
    pub total: Option<u64>,
    pub state: String,
    pub active: bool,
}

pub fn downloads_page(rows: &[DownloadRow]) -> String {
    if rows.is_empty() {
        return shell("Downloads", "<div class=\"empty\">No downloads yet.</div>");
    }
    let mut body = String::from("<ul>");
    for r in rows.iter().rev() {
        let size = match r.total {
            Some(t) if t > 0 => format!("{} / {}", human_size(r.done), human_size(t)),
            _ => human_size(r.done),
        };
        let pct = match r.total {
            Some(t) if t > 0 => ((r.done as f64 / t as f64) * 100.0).clamp(0.0, 100.0),
            _ => {
                if r.active {
                    0.0
                } else {
                    100.0
                }
            }
        };
        let cancel = if r.active {
            format!("<span class=\"actions\"><a href=\"rabbithole://download/cancel/{}\">cancel</a></span>", r.id)
        } else {
            String::new()
        };
        body.push_str(&format!(
            "<li><b>{}</b>{cancel}<br><span class=\"u\">{}</span><br><span class=\"muted\">{} — {} — {}</span><div class=\"bar\"><div style=\"width:{:.0}%\"></div></div></li>",
            escape(&r.filename),
            escape(&r.url),
            escape(&r.state),
            size,
            escape(&r.path),
            pct
        ));
    }
    body.push_str("</ul>");
    shell("Downloads", &body)
}

/// `about:text`: the readable text of a page.
pub fn text_page(url: &str, text: &str) -> String {
    shell(&format!("Text of {url}"), &format!("<pre>{}</pre>", escape(text)))
}

/// `about:version`
pub fn version_page(font_faces: usize, profile: &Profile, cookies: usize) -> String {
    let body = format!(
        "<table><tr><td>RabbitHole</td><td>{}</td></tr><tr><td>Engine</td><td>rabbithole-engine {}</td></tr><tr><td>Platform</td><td>{} {}</td></tr><tr><td>Font faces</td><td>{}</td></tr><tr><td>Cookies</td><td>{}</td></tr><tr><td>Profile</td><td>{}</td></tr></table><p class=\"muted\">A web browser written from scratch: own HTML, CSS, layout, rasterizer and JavaScript engine.</p>",
        env!("CARGO_PKG_VERSION"),
        rabbithole_engine::VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH,
        font_faces,
        cookies,
        escape(&profile.dir.display().to_string())
    );
    shell("About RabbitHole", &body)
}

pub fn blank_page() -> String {
    "<!doctype html><html><head><title></title></head><body></body></html>".to_string()
}

/// Error page shown when a navigation fails.
pub fn error_page(url: &str, error: &str) -> String {
    let body = format!(
        "<p>RabbitHole could not load <code>{0}</code>.</p><pre>{1}</pre><p><a href=\"{0}\">Retry</a> &nbsp; <a href=\"about:home\">Go home</a></p>",
        escape(url),
        escape(error)
    );
    shell("Could not load page", &body)
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// Civil date (UTC) for a day number since the epoch.
fn day_label(day: u64) -> String {
    let z = day as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let today = crate::profile::now_secs() / 86400;
    if day == today {
        "Today".into()
    } else if day + 1 == today {
        "Yesterday".into()
    } else {
        format!("{y:04}-{m:02}-{d:02}")
    }
}

fn time_label(ts: u64) -> String {
    let s = ts % 86400;
    format!("{:02}:{:02}", s / 3600, (s % 3600) / 60)
}

pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_contain_links() {
        let mut h = History::default();
        h.add("https://example.com/", "Example <b>");
        let page = history_page(&h);
        assert!(page.contains("href=\"https://example.com/\""));
        assert!(page.contains("Example &lt;b&gt;"));
        assert!(page.contains("rabbithole://history/clear"));
        let mut b = Bookmarks::default();
        b.toggle("https://rust-lang.org/", "Rust");
        let page = bookmarks_page(&b);
        assert!(page.contains("href=\"https://rust-lang.org/\""));
        assert!(page.contains("rabbithole://bookmark/remove?https%3A%2F%2Frust-lang.org%2F"));
        let rows = vec![DownloadRow {
            id: 3,
            url: "https://x/f.zip".into(),
            filename: "f.zip".into(),
            path: "/tmp/f.zip".into(),
            done: 512,
            total: Some(1024),
            state: "Downloading".into(),
            active: true,
        }];
        let page = downloads_page(&rows);
        assert!(page.contains("rabbithole://download/cancel/3"));
        assert!(page.contains("width:50%"));
        assert!(history_page(&History::default()).contains("No pages visited"));
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(100), "100 B");
    }

    #[test]
    fn dates() {
        assert_eq!(day_label(0), "1970-01-01");
        assert_eq!(day_label(19723), "2024-01-01");
        assert_eq!(time_label(3661), "01:01");
    }
}
