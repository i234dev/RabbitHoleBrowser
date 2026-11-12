//! Internal `about:` pages generated as HTML (rendered by our own engine
//! like any other page). Their buttons are `rabbithole://` links and
//! forms, handled by the UI (see `App::handle_internal_action`).

use std::sync::atomic::{AtomicBool, Ordering};

use crate::autocookies::{AutoCookies, ConsentMode, SiteRule};
use crate::bookmarks::Bookmarks;
use crate::history::History;
use crate::profile::Profile;
use crate::settings::{SearchEngine, Settings};

static DARK: AtomicBool = AtomicBool::new(false);

/// Generate internal pages in dark colours.
pub fn set_dark(dark: bool) {
    DARK.store(dark, Ordering::Relaxed);
}

pub fn is_dark() -> bool {
    DARK.load(Ordering::Relaxed)
}

const STYLE_LIGHT: &str = "<style>
body{font-family:sans-serif;margin:0;background:#f8f9fa;color:#202124}
header{background:#1f1b2e;color:#f4efe6;padding:18px 32px;border-bottom:4px solid #e0a84a}
header h1{margin:0;font-size:22px}
main{max-width:920px;margin:0 auto;padding:20px 32px}
h2{font-size:16px;color:#5f6368;margin:24px 0 8px 0;border-bottom:1px solid #dadce0;padding-bottom:4px}
h3{font-size:14px;margin:18px 0 6px 0}
ul{list-style:none;padding:0;margin:0}
li{padding:7px 10px;border-bottom:1px solid #eceff1;background:#fff}
a{color:#1a0dab;text-decoration:none}
li .u{color:#5f6368;font-size:12px;margin-left:8px}
.muted{color:#5f6368}
.empty{padding:24px;text-align:center;color:#5f6368;background:#fff;border:1px dashed #dadce0}
.bar{height:6px;background:#e8eaed;border-radius:3px;overflow:hidden;margin-top:4px}
.bar div{height:100%;background:#1a73e8}
pre{background:#fff;border:1px solid #dadce0;padding:12px;white-space:pre-wrap;overflow-wrap:anywhere;font-size:13px}
.actions{float:right}
.actions a{color:#c5221f;font-size:12px;margin-left:10px}
.actions a.n{color:#1a73e8}
table{border-collapse:collapse;width:100%}td{padding:6px 8px;border-bottom:1px solid #eceff1;vertical-align:middle}
td.k{width:38%;color:#3c4043}
.card{background:#fff;border:1px solid #dadce0;border-radius:8px;padding:6px 14px;margin-bottom:14px}
input[type=text],input[type=search],select{padding:5px 8px;border:1px solid #c4c7cc;border-radius:4px;font-size:13px;background:#fff;color:#202124}
input[type=text]{width:280px}
button,input[type=submit]{padding:5px 12px;border:1px solid #c4c7cc;border-radius:4px;background:#f1f3f4;color:#202124;font-size:13px}
.on{color:#188038;font-weight:bold}.off{color:#c5221f;font-weight:bold}
.toggle{padding:3px 10px;border:1px solid #c4c7cc;border-radius:12px;font-size:12px;margin-left:8px;color:#202124;background:#f1f3f4}
.err{max-width:640px;margin:60px auto;padding:0 24px}
.err .icon{font-size:48px;color:#5f6368}
.err h1{font-size:24px;font-weight:normal;margin:12px 0}
.err .btn{display:inline-block;background:#1a73e8;color:#fff;padding:8px 18px;border-radius:4px;margin-right:10px}
.err .btn2{display:inline-block;color:#1a73e8;padding:8px 6px}
.err details,.err .details{margin-top:24px;color:#5f6368;font-size:13px}
.err code{background:#eceff1;padding:1px 4px;border-radius:3px}
</style>";

const STYLE_DARK: &str = "<style>
body{font-family:sans-serif;margin:0;background:#1e1f22;color:#e8eaed}
header{background:#141218;color:#f4efe6;padding:18px 32px;border-bottom:4px solid #e0a84a}
header h1{margin:0;font-size:22px}
main{max-width:920px;margin:0 auto;padding:20px 32px}
h2{font-size:16px;color:#9aa0a6;margin:24px 0 8px 0;border-bottom:1px solid #3c4043;padding-bottom:4px}
h3{font-size:14px;margin:18px 0 6px 0}
ul{list-style:none;padding:0;margin:0}
li{padding:7px 10px;border-bottom:1px solid #303134;background:#28292c}
a{color:#8ab4f8;text-decoration:none}
li .u{color:#9aa0a6;font-size:12px;margin-left:8px}
.muted{color:#9aa0a6}
.empty{padding:24px;text-align:center;color:#9aa0a6;background:#28292c;border:1px dashed #3c4043}
.bar{height:6px;background:#3c4043;border-radius:3px;overflow:hidden;margin-top:4px}
.bar div{height:100%;background:#8ab4f8}
pre{background:#28292c;border:1px solid #3c4043;padding:12px;white-space:pre-wrap;overflow-wrap:anywhere;font-size:13px;color:#e8eaed}
.actions{float:right}
.actions a{color:#f28b82;font-size:12px;margin-left:10px}
.actions a.n{color:#8ab4f8}
table{border-collapse:collapse;width:100%}td{padding:6px 8px;border-bottom:1px solid #303134;vertical-align:middle}
td.k{width:38%;color:#bdc1c6}
.card{background:#28292c;border:1px solid #3c4043;border-radius:8px;padding:6px 14px;margin-bottom:14px}
input[type=text],input[type=search],select{padding:5px 8px;border:1px solid #5f6368;border-radius:4px;font-size:13px;background:#202124;color:#e8eaed}
input[type=text]{width:280px}
button,input[type=submit]{padding:5px 12px;border:1px solid #5f6368;border-radius:4px;background:#3c4043;color:#e8eaed;font-size:13px}
.on{color:#81c995;font-weight:bold}.off{color:#f28b82;font-weight:bold}
.toggle{padding:3px 10px;border:1px solid #5f6368;border-radius:12px;font-size:12px;margin-left:8px;color:#e8eaed;background:#3c4043}
.err{max-width:640px;margin:60px auto;padding:0 24px}
.err .icon{font-size:48px;color:#9aa0a6}
.err h1{font-size:24px;font-weight:normal;margin:12px 0}
.err .btn{display:inline-block;background:#8ab4f8;color:#202124;padding:8px 18px;border-radius:4px;margin-right:10px}
.err .btn2{display:inline-block;color:#8ab4f8;padding:8px 6px}
.err details,.err .details{margin-top:24px;color:#9aa0a6;font-size:13px}
.err code{background:#303134;padding:1px 4px;border-radius:3px}
</style>";

/// The stylesheet for the current theme.
pub fn style() -> &'static str {
    if is_dark() {
        STYLE_DARK
    } else {
        STYLE_LIGHT
    }
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn shell(title: &str, body: &str) -> String {
    format!("<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title>{}</head><body><header><h1>{}</h1></header><main>{body}</main></body></html>", escape(title), style(), escape(title))
}

/// Parse `a=1&b=2` (form encoding: `+` is a space).
pub fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            let dec = |s: &str| rabbithole_engine::net::url::percent_decode(&s.replace('+', " "));
            (dec(k), dec(v))
        })
        .collect()
}

pub fn query_value(q: Option<&str>, key: &str) -> Option<String> {
    parse_query(q?).into_iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

// ------------------------------------------------------------------ history

/// `about:history`: visits grouped by day, filtered by `query`.
pub fn history_page(history: &History, query: &str) -> String {
    let query = query.trim();
    let mut body = format!(
        "<form action=\"about:history\" method=\"get\"><input type=\"search\" name=\"q\" value=\"{}\" placeholder=\"Search history\" style=\"width:360px\"> <input type=\"submit\" value=\"Search\">{}</form>",
        escape(query),
        if query.is_empty() { String::new() } else { " &nbsp; <a href=\"about:history\">Show all</a>".to_string() }
    );
    let entries: Vec<_> = if query.is_empty() { history.recent(500) } else { history.search(query, 500) };
    if entries.is_empty() {
        let msg = if query.is_empty() { "No pages visited yet.".to_string() } else { format!("No history entries match “{}”.", escape(query)) };
        body.push_str(&format!("<div class=\"empty\" style=\"margin-top:16px\">{msg}</div>"));
        return shell("History", &body);
    }
    let mut current_day: Option<u64> = None;
    for e in entries {
        let day = e.timestamp / 86400;
        if current_day != Some(day) {
            if current_day.is_some() {
                body.push_str("</ul>");
            }
            body.push_str(&format!("<h2>{}<span class=\"actions\"><a href=\"rabbithole://history/delete-day?{day}\">delete this day</a></span></h2><ul>", day_label(day)));
            current_day = Some(day);
        }
        let title = if e.title.trim().is_empty() { e.url.as_str() } else { e.title.as_str() };
        body.push_str(&format!(
            "<li><span class=\"actions\"><a href=\"rabbithole://history/delete?{}\" title=\"Remove from history\">remove</a></span><span class=\"muted\">{}</span> &nbsp; <a href=\"{}\">{}</a><span class=\"u\">{}</span></li>",
            escape(&percent_encode(&e.url)),
            time_label(e.timestamp),
            escape(&e.url),
            escape(title),
            escape(&shorten(&e.url, 70))
        ));
    }
    body.push_str("</ul><p class=\"muted\"><a href=\"rabbithole://history/clear\">Clear all browsing history</a></p>");
    shell("History", &body)
}

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

// ---------------------------------------------------------------- bookmarks

/// `about:bookmarks`: the bookmark manager. `edit` names the bookmark
/// whose edit form is open.
pub fn bookmarks_page(bookmarks: &Bookmarks, edit: Option<&str>, bar_shown: bool, notice: Option<&str>) -> String {
    let mut body = String::new();
    if let Some(n) = notice {
        body.push_str(&format!("<div class=\"card\">{}</div>", escape(n)));
    }
    body.push_str(&format!(
        "<p class=\"muted\">{} bookmarks. Ctrl+D bookmarks the current page. <a class=\"toggle\" href=\"rabbithole://settings/set?key=show_bookmarks_bar&amp;value={}&amp;back=bookmarks\">{} bookmarks bar</a> <a class=\"toggle\" href=\"rabbithole://bookmark/export\">Export to HTML</a></p>",
        bookmarks.items.len(),
        if bar_shown { 0 } else { 1 },
        if bar_shown { "Hide" } else { "Show" }
    ));
    if bookmarks.items.is_empty() {
        body.push_str("<div class=\"empty\">No bookmarks yet. Press Ctrl+D on a page to bookmark it.</div>");
    }
    let mut folders: Vec<&str> = vec![""];
    folders.extend(bookmarks.folders());
    for folder in folders {
        let items: Vec<_> = bookmarks.items.iter().filter(|b| b.folder == folder).collect();
        if items.is_empty() {
            continue;
        }
        if !folder.is_empty() {
            body.push_str(&format!("<h2>{}</h2>", escape(folder)));
        } else {
            body.push_str("<h2>Bookmarks</h2>");
        }
        body.push_str("<ul>");
        for b in items {
            let enc = escape(&percent_encode(&b.url));
            if edit == Some(b.url.as_str()) {
                body.push_str(&format!(
                    "<li><form action=\"rabbithole://bookmark/save\" method=\"get\"><input type=\"hidden\" name=\"url\" value=\"{}\">Title <input type=\"text\" name=\"title\" value=\"{}\"> Folder <input type=\"text\" name=\"folder\" value=\"{}\" style=\"width:140px\"> <input type=\"submit\" value=\"Save\"> <a href=\"about:bookmarks\">Cancel</a></form><span class=\"u\">{}</span></li>",
                    escape(&b.url),
                    escape(&b.title),
                    escape(&b.folder),
                    escape(&b.url)
                ));
                continue;
            }
            body.push_str(&format!(
                "<li><span class=\"actions\"><a class=\"n\" href=\"rabbithole://bookmark/up?{enc}\">▲</a><a class=\"n\" href=\"rabbithole://bookmark/down?{enc}\">▼</a><a class=\"n\" href=\"about:bookmarks?edit={enc}\">edit</a><a href=\"rabbithole://bookmark/remove?{enc}\">remove</a></span><a href=\"{0}\">{1}</a><span class=\"u\">{2}</span></li>",
                escape(&b.url),
                escape(b.label()),
                escape(&shorten(&b.url, 60))
            ));
        }
        body.push_str("</ul>");
    }
    body.push_str(
        "<h2>Import</h2><form action=\"rabbithole://bookmark/import\" method=\"get\"><p class=\"muted\">Path of a bookmarks HTML file exported from any browser:</p><input type=\"text\" name=\"path\" style=\"width:420px\" placeholder=\"/home/me/bookmarks.html\"> <input type=\"submit\" value=\"Import\"></form>",
    );
    shell("Bookmarks", &body)
}

// ---------------------------------------------------------------- downloads

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
    pub paused: bool,
    /// Pause/resume possible (the server accepts byte ranges).
    pub resumable: bool,
    pub finished: bool,
    pub failed: bool,
    /// Bytes per second while active.
    pub speed: Option<u64>,
}

pub fn downloads_page(rows: &[DownloadRow], dir: &str, ask: bool) -> String {
    let mut body = format!(
        "<p class=\"muted\">Saving to <code>{}</code> <a class=\"toggle\" href=\"rabbithole://download/open-folder\">Open folder</a> <a class=\"toggle\" href=\"rabbithole://settings/set?key=ask_download&amp;value={}&amp;back=downloads\">{}</a>{}</p>",
        escape(dir),
        if ask { 0 } else { 1 },
        if ask { "Ask where to save: on" } else { "Ask where to save: off" },
        if rows.iter().any(|r| !r.active && !r.paused) { " <a class=\"toggle\" href=\"rabbithole://download/clear\">Clear finished</a>" } else { "" }
    );
    if rows.is_empty() {
        body.push_str("<div class=\"empty\">No downloads yet.</div>");
        return shell("Downloads", &body);
    }
    body.push_str("<ul>");
    for r in rows.iter().rev() {
        let size = match r.total {
            Some(t) if t > 0 => format!("{} / {}", human_size(r.done), human_size(t)),
            _ => human_size(r.done),
        };
        let speed = r.speed.filter(|_| r.active && !r.paused).map(|s| format!(" — {}/s", human_size(s))).unwrap_or_default();
        let pct = match r.total {
            Some(t) if t > 0 => ((r.done as f64 / t as f64) * 100.0).clamp(0.0, 100.0),
            _ => {
                if r.finished {
                    100.0
                } else {
                    0.0
                }
            }
        };
        let mut actions = String::new();
        let id = r.id;
        if r.active && !r.paused && r.resumable {
            actions.push_str(&format!("<a class=\"n\" href=\"rabbithole://download/pause/{id}\">pause</a>"));
        }
        if r.paused {
            actions.push_str(&format!("<a class=\"n\" href=\"rabbithole://download/resume/{id}\">resume</a>"));
        }
        if r.active || r.paused {
            actions.push_str(&format!("<a href=\"rabbithole://download/cancel/{id}\">cancel</a>"));
        }
        if r.finished {
            actions.push_str(&format!("<a class=\"n\" href=\"rabbithole://download/open/{id}\">open</a><a class=\"n\" href=\"rabbithole://download/show/{id}\">show in folder</a>"));
        }
        if r.failed {
            actions.push_str(&format!("<a class=\"n\" href=\"rabbithole://download/retry/{id}\">retry</a>"));
        }
        if !r.active && !r.paused {
            actions.push_str(&format!("<a href=\"rabbithole://download/remove/{id}\">remove from list</a>"));
        }
        let bar = if r.finished { String::new() } else { format!("<div class=\"bar\"><div style=\"width:{pct:.0}%\"></div></div>") };
        body.push_str(&format!(
            "<li><span class=\"actions\">{actions}</span><b>{}</b><br><span class=\"u\">{}</span><br><span class=\"muted\">{} — {}{speed} — {}</span>{bar}</li>",
            escape(&r.filename),
            escape(&shorten(&r.url, 80)),
            escape(&r.state),
            size,
            escape(&r.path),
        ));
    }
    body.push_str("</ul>");
    shell("Downloads", &body)
}

// ----------------------------------------------------------------- settings

/// A site with stored data, for the settings page.
pub struct SiteData {
    pub host: String,
    pub cookies: usize,
    pub zoom: Option<f32>,
}

fn toggle_row(label: &str, help: &str, key: &str, on: bool) -> String {
    format!(
        "<tr><td class=\"k\">{label}<br><span class=\"muted\" style=\"font-size:12px\">{help}</span></td><td><span class=\"{}\">{}</span> <a class=\"toggle\" href=\"rabbithole://settings/set?key={key}&amp;value={}\">{}</a></td></tr>",
        if on { "on" } else { "off" },
        if on { "On" } else { "Off" },
        if on { 0 } else { 1 },
        if on { "Turn off" } else { "Turn on" }
    )
}

fn text_row(label: &str, help: &str, key: &str, value: &str, width: u32) -> String {
    format!(
        "<tr><td class=\"k\">{label}<br><span class=\"muted\" style=\"font-size:12px\">{help}</span></td><td><form action=\"rabbithole://settings/set\" method=\"get\"><input type=\"hidden\" name=\"key\" value=\"{key}\"><input type=\"text\" name=\"value\" value=\"{}\" style=\"width:{width}px\"> <input type=\"submit\" value=\"Save\"></form></td></tr>",
        escape(value)
    )
}

fn select_row(label: &str, key: &str, options: &[(&str, &str)], current: &str) -> String {
    let opts: String = options.iter().map(|(v, l)| format!("<option value=\"{v}\"{}>{}</option>", if *v == current { " selected" } else { "" }, escape(l))).collect();
    format!(
        "<tr><td class=\"k\">{label}</td><td><form action=\"rabbithole://settings/set\" method=\"get\"><input type=\"hidden\" name=\"key\" value=\"{key}\"><select name=\"value\">{opts}</select> <input type=\"submit\" value=\"Save\"></form></td></tr>"
    )
}

/// `about:settings`
pub fn settings_page(s: &Settings, sites: &[SiteData], default_download_dir: &str, notice: Option<&str>, private: bool) -> String {
    let mut body = String::new();
    if let Some(n) = notice {
        body.push_str(&format!("<div class=\"card\">{}</div>", escape(n)));
    }
    if private {
        body.push_str("<div class=\"card\">This is a private window: changes here apply to this window only and are forgotten when it closes.</div>");
    }
    body.push_str("<h2>On startup</h2><div class=\"card\"><table>");
    body.push_str(&toggle_row("Restore previous session", "Reopen the tabs that were open (also after a crash)", "restore_session", s.restore_session));
    body.push_str(&text_row("Home page", "about:home is the built-in start page", "homepage", &s.homepage, 320));
    body.push_str("</table></div>");

    body.push_str("<h2>Search</h2><div class=\"card\"><table>");
    let engines: Vec<(&str, &str)> = SearchEngine::ALL.iter().map(|e| (e.id(), e.name())).collect();
    body.push_str(&select_row("Search engine used in the address bar", "search_engine", &engines, s.search_engine.id()));
    body.push_str("</table></div>");

    body.push_str("<h2>Appearance</h2><div class=\"card\"><table>");
    body.push_str(&select_row("Theme", "theme", &[("system", "Follow the system"), ("light", "Light"), ("dark", "Dark")], s.theme.id()));
    let zooms: Vec<(String, String)> = [50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200].iter().map(|z| (format!("{:.2}", *z as f32 / 100.0), format!("{z}%"))).collect();
    let zref: Vec<(&str, &str)> = zooms.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let cur = format!("{:.2}", s.default_zoom);
    let cur = zref.iter().map(|(v, _)| *v).find(|v| *v == cur).unwrap_or("1.00").to_string();
    body.push_str(&select_row("Default zoom", "default_zoom", &zref, &cur));
    body.push_str(&toggle_row("Bookmarks bar", "Ctrl+Shift+B", "show_bookmarks_bar", s.show_bookmarks_bar));
    body.push_str(&toggle_row("Smooth scrolling", "Animate wheel and keyboard scrolling", "smooth_scrolling", s.smooth_scrolling));
    body.push_str("</table></div>");

    body.push_str("<h2>Content</h2><div class=\"card\"><table>");
    body.push_str(&toggle_row("JavaScript", "Applies when a page is (re)loaded", "javascript", s.javascript));
    body.push_str(&toggle_row("Images", "", "images", s.images));
    body.push_str(&text_row("Preferred languages", "Sent to sites as Accept-Language", "accept_language", &s.accept_language, 260));
    body.push_str("</table></div>");

    body.push_str("<h2>Privacy</h2><div class=\"card\"><table>");
    body.push_str(&toggle_row("Block ads and trackers", "Requests to known tracking domains are not made", "block_trackers", s.block_trackers));
    body.push_str(&toggle_row("Block third-party cookies", "Cross-site subresources get no cookies and cannot set any", "block_third_party_cookies", s.block_third_party_cookies));
    body.push_str(&select_row("Cookie dialogs<br><span class=\"muted\" style=\"font-size:12px\">Answer European cookie-consent banners automatically</span>", "cookie_consent", &[("off", "Off (ask me)"), ("reject", "Auto-reject"), ("accept", "Auto-accept")], s.cookie_consent.id()));
    body.push_str("<tr><td class=\"k\">Handled cookie dialogs</td><td><a class=\"toggle\" href=\"about:autocookies\">Log and per-site rules</a></td></tr>");
    body.push_str("<tr><td class=\"k\">Clear browsing data</td><td><a class=\"toggle\" href=\"rabbithole://data/clear-history\">History</a><a class=\"toggle\" href=\"rabbithole://data/clear-cookies\">All cookies and site data</a><a class=\"toggle\" href=\"rabbithole://data/clear-cache\">Cache</a></td></tr>");
    body.push_str("</table></div>");

    body.push_str("<h2>Downloads</h2><div class=\"card\"><table>");
    let dir_help = format!("Empty: {}", escape(default_download_dir));
    body.push_str(&text_row("Download folder", &dir_help, "download_dir", &s.download_dir, 320));
    body.push_str(&toggle_row("Ask where to save each file", "", "ask_download", s.ask_download));
    body.push_str("</table></div>");

    body.push_str("<h2>Site data</h2>");
    if sites.is_empty() {
        body.push_str("<div class=\"empty\">No site stores cookies or settings.</div>");
    } else {
        body.push_str("<ul>");
        for site in sites {
            let mut what = Vec::new();
            if site.cookies > 0 {
                what.push(format!("{} cookie{}", site.cookies, if site.cookies == 1 { "" } else { "s" }));
            }
            if let Some(z) = site.zoom {
                what.push(format!("zoom {:.0}%", z * 100.0));
            }
            body.push_str(&format!(
                "<li><span class=\"actions\"><a href=\"rabbithole://data/clear-site?{}\">clear</a></span><b>{}</b><span class=\"u\">{}</span></li>",
                escape(&percent_encode(&site.host)),
                escape(&site.host),
                escape(&what.join(", "))
            ));
        }
        body.push_str("</ul>");
    }
    body.push_str("<p class=\"muted\">Settings are stored in <code>settings.txt</code> in the profile folder.</p>");
    shell("Settings", &body)
}

// ------------------------------------------------------------- autocookies

/// `about:autocookies`: the log of cookie dialogs handled automatically,
/// filtered by `query`, plus the global mode and per-site rules.
pub fn autocookies_page(a: &AutoCookies, mode: ConsentMode, query: &str, notice: Option<&str>, private: bool) -> String {
    let query = query.trim();
    let mut body = String::new();
    if let Some(n) = notice.filter(|n| !n.is_empty()) {
        body.push_str(&format!("<div class=\"card\">{}</div>", escape(n)));
    }
    if private {
        body.push_str("<div class=\"card\">Private window: dialogs handled here are not saved to the log.</div>");
    }
    // Global mode.
    body.push_str("<h2>Cookie dialogs</h2><div class=\"card\"><table><tr><td class=\"k\">When a site shows a cookie-consent dialog<br><span class=\"muted\" style=\"font-size:12px\">Only buttons inside the consent dialog are ever clicked</span></td><td>");
    for m in ConsentMode::ALL {
        if m == mode {
            body.push_str(&format!("<span class=\"toggle\" style=\"background:#1a73e8;color:#fff;border-color:#1a73e8\">{}</span>", m.label()));
        } else {
            body.push_str(&format!("<a class=\"toggle\" href=\"rabbithole://autocookies/mode?value={}\">{}</a>", m.id(), m.label()));
        }
    }
    body.push_str("</td></tr></table></div>");

    // Log.
    let entries = a.search(query);
    body.push_str(&format!(
        "<h2>Handled dialogs<span class=\"actions\">{}</span></h2><form action=\"about:autocookies\" method=\"get\"><input type=\"search\" name=\"q\" value=\"{}\" placeholder=\"Search sites, buttons, actions\" style=\"width:360px\"> <input type=\"submit\" value=\"Search\">{}</form>",
        if a.log.is_empty() { String::new() } else { "<a href=\"rabbithole://autocookies/clear\">clear all</a>".to_string() },
        escape(query),
        if query.is_empty() { String::new() } else { " &nbsp; <a href=\"about:autocookies\">Show all</a>".to_string() }
    ));
    if entries.is_empty() {
        let msg = if !query.is_empty() {
            format!("No entries match “{}”.", escape(query))
        } else if mode == ConsentMode::Off {
            "Nothing handled yet. Choose Auto-reject or Auto-accept above.".to_string()
        } else {
            "Nothing handled yet.".to_string()
        };
        body.push_str(&format!("<div class=\"empty\" style=\"margin-top:12px\">{msg}</div>"));
    } else {
        let qs = if query.is_empty() { String::new() } else { format!("&amp;q={}", escape(&percent_encode(query))) };
        body.push_str("<div class=\"card\" style=\"margin-top:12px\"><table><tr class=\"muted\"><td>When</td><td>Site</td><td>Action</td><td>Button</td><td></td></tr>");
        for e in entries.iter().take(500) {
            let colour = match e.outcome {
                rabbithole_engine::consent::ConsentOutcome::Rejected => "on",
                rabbithole_engine::consent::ConsentOutcome::Accepted => "",
                _ => "off",
            };
            let vendor = if e.vendor.is_empty() { String::new() } else { format!("<br><span class=\"muted\" style=\"font-size:11px\">{}</span>", escape(&e.vendor)) };
            body.push_str(&format!(
                "<tr><td class=\"muted\" style=\"white-space:nowrap\">{} {}</td><td><b>{}</b>{vendor}</td><td class=\"{colour}\">{}</td><td>{}</td><td style=\"text-align:right\"><span class=\"actions\"><a href=\"rabbithole://autocookies/delete?{}{qs}\">delete</a></span></td></tr>",
                date_label(e.time),
                time_label(e.time),
                escape(&e.host),
                e.action_label(),
                if e.button.is_empty() { "—".to_string() } else { format!("“{}”", escape(&shorten(&e.button, 40))) },
                e.id,
            ));
        }
        body.push_str("</table></div>");
    }

    // Per-site rules.
    body.push_str("<h2>Per-site rules</h2><p class=\"muted\">A rule for a site also covers its subdomains and wins over the global setting.</p>");
    let hosts = a.hosts();
    if !hosts.is_empty() {
        body.push_str("<div class=\"card\"><table>");
        for h in hosts.iter().take(300) {
            let cur = a.sites.get(h).copied();
            let mut links = String::new();
            let opts: [(Option<SiteRule>, &str); 4] = [(None, "Default"), (Some(SiteRule::Reject), "Always reject"), (Some(SiteRule::Accept), "Always accept"), (Some(SiteRule::Never), "Never touch")];
            for (r, label) in opts {
                if r == cur {
                    links.push_str(&format!("<span class=\"toggle\" style=\"background:#1a73e8;color:#fff;border-color:#1a73e8\">{label}</span>"));
                } else {
                    links.push_str(&format!("<a class=\"toggle\" href=\"rabbithole://autocookies/site?host={}&amp;rule={}\">{label}</a>", escape(&percent_encode(h)), r.map(|r| r.id()).unwrap_or("default")));
                }
            }
            body.push_str(&format!("<tr><td class=\"k\"><b>{}</b></td><td>{links}</td></tr>", escape(h)));
        }
        body.push_str("</table></div>");
    }
    body.push_str("<form action=\"rabbithole://autocookies/site\" method=\"get\" style=\"margin-top:10px\">Add a rule for <input type=\"text\" name=\"host\" placeholder=\"example.com\" style=\"width:220px\"> <select name=\"rule\"><option value=\"reject\">Always reject</option><option value=\"accept\">Always accept</option><option value=\"never\">Never touch</option></select> <input type=\"submit\" value=\"Add\"></form>");
    body.push_str("<p class=\"muted\">The log is stored in <code>autocookies.txt</code> and the rules in <code>autocookies-sites.txt</code> in the profile folder.</p>");
    shell("Cookie dialogs handled automatically", &body)
}

// ------------------------------------------------------------------- others

/// `about:text`: the readable text of a page.
pub fn text_page(url: &str, text: &str) -> String {
    shell(&format!("Text of {url}"), &format!("<pre>{}</pre>", escape(text)))
}

/// `about:version`
pub fn version_page(font_faces: usize, profile: &Profile, cookies: usize) -> String {
    let body = format!(
        "<table><tr><td>RabbitHole</td><td>{}</td></tr><tr><td>Engine</td><td>rabbithole-engine {}</td></tr><tr><td>Platform</td><td>{} {}</td></tr><tr><td>Font faces</td><td>{}</td></tr><tr><td>Cookies</td><td>{}</td></tr><tr><td>Profile</td><td>{}</td></tr></table><p class=\"muted\">A web browser written from scratch: own HTML, CSS, layout, rasterizer and JavaScript engine.</p><h2>Internal pages</h2><ul><li><a href=\"about:settings\">about:settings</a></li><li><a href=\"about:history\">about:history</a></li><li><a href=\"about:bookmarks\">about:bookmarks</a></li><li><a href=\"about:downloads\">about:downloads</a></li><li><a href=\"about:home\">about:home</a></li></ul>",
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

// -------------------------------------------------------------- error pages

/// What went wrong with a navigation, for a friendly error page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Dns,
    Tls,
    Offline,
    Refused,
    Timeout,
    Blocked,
    Http(u16),
    BadAddress,
    Unsupported,
    /// The tab's content thread died (a browser bug, not the site's).
    Crashed,
    Other,
}

/// Classify a fetch error message (from `FetchError`'s Display and the
/// OS / TLS messages it wraps).
pub fn classify_error(error: &str) -> ErrorKind {
    let e = error.to_ascii_lowercase();
    if e.starts_with("blocked ad/tracker") {
        ErrorKind::Blocked
    } else if e.starts_with("tab crashed") {
        ErrorKind::Crashed
    } else if let Some(code) = e.strip_prefix("http error ").and_then(|c| c.trim().parse().ok()) {
        ErrorKind::Http(code)
    } else if e.contains("certificate") || e.contains("tls") || e.contains("handshake") || e.contains("alert") || e.contains("ssl") {
        ErrorKind::Tls
    } else if e.contains("network is unreachable") || e.contains("unreachable network") || e.contains("os error 10051") || e.contains("os error 101") || e.contains("temporary failure in name resolution") || e.contains("no route to host") {
        ErrorKind::Offline
    } else if e.contains("failed to lookup") || e.contains("name or service not known") || e.contains("no such host") || e.contains("no addresses for") || e.contains("nodename nor servname") || e.contains("os error 11001") {
        ErrorKind::Dns
    } else if e.contains("refused") {
        ErrorKind::Refused
    } else if e.contains("timed out") || e.contains("timeout") {
        ErrorKind::Timeout
    } else if e.contains("unsupported url scheme") {
        ErrorKind::Unsupported
    } else if e.contains("could not be understood") {
        ErrorKind::BadAddress
    } else {
        ErrorKind::Other
    }
}

fn host_of(url: &str) -> String {
    rabbithole_engine::net::Url::parse(url).map(|u| u.host).filter(|h| !h.is_empty()).unwrap_or_else(|| url.to_string())
}

/// Error page shown when a navigation fails, with a retry button.
pub fn error_page(url: &str, error: &str) -> String {
    let kind = classify_error(error);
    let host = escape(&host_of(url));
    let (icon, title, text, tips): (&str, String, String, &[&str]) = match kind {
        ErrorKind::Dns => ("?", "This site can’t be found".into(), format!("The server address of <b>{host}</b> could not be found."), &["Check the address for typing errors.", "Check your network connection.", "If it is a new domain, it may not be published yet."]),
        ErrorKind::Tls => (
            "⚠",
            "Your connection is not private".into(),
            format!("RabbitHole could not establish a secure connection to <b>{host}</b>: its certificate or TLS setup was rejected. Attackers might be trying to intercept the connection, so the page was not loaded."),
            &["Check that your computer’s date and time are correct.", "The site may use an expired, self-signed or misconfigured certificate."],
        ),
        ErrorKind::Offline => ("⌁", "You’re offline".into(), "The network is unreachable. RabbitHole will load the page when you retry with a working connection.".into(), &["Check cables, Wi‑Fi or mobile data.", "Check proxy or VPN settings."]),
        ErrorKind::Refused => ("✕", "This site can’t be reached".into(), format!("<b>{host}</b> refused to connect."), &["The server may be down or not accepting connections on this port."]),
        ErrorKind::Timeout => ("◷", "This site took too long to respond".into(), format!("<b>{host}</b> did not answer in time."), &["The server may be overloaded; try again in a moment.", "Check your network connection."]),
        ErrorKind::Blocked => ("⛔", "Blocked by the tracker blocker".into(), format!("<b>{host}</b> is on the list of known ad and tracking domains, so RabbitHole did not contact it."), &["You can turn off ad and tracker blocking in Settings."]),
        ErrorKind::Http(code) if code >= 500 => ("!", format!("Server error ({code})"), format!("<b>{host}</b> had a problem answering this request."), &["This is a problem with the site, not with your connection. Try again later."]),
        ErrorKind::Http(404) => ("?", "Page not found (404)".into(), format!("<b>{host}</b> has no page at this address."), &["Check the address, or go to the site’s home page."]),
        ErrorKind::Http(code) => ("!", format!("HTTP error {code}"), format!("<b>{host}</b> answered with status {code}."), &[]),
        ErrorKind::BadAddress => ("?", "Invalid address".into(), "The address could not be understood.".into(), &[]),
        ErrorKind::Unsupported => ("?", "Can’t open this kind of address".into(), "RabbitHole does not support this URL scheme.".into(), &[]),
        ErrorKind::Crashed => ("!", "This tab crashed".into(), format!("RabbitHole hit an internal error while showing <b>{host}</b> and stopped this tab; the rest of the browser is unaffected."), &["Reload the page to try again.", "If it keeps happening, the details below help to report the bug."]),
        ErrorKind::Other => ("!", "This page could not be loaded".into(), format!("Something went wrong while loading <b>{host}</b>."), &[]),
    };
    let tips: String = if tips.is_empty() { String::new() } else { format!("<ul style=\"list-style:disc;padding-left:20px\">{}</ul>", tips.iter().map(|t| format!("<li style=\"background:none;border:none;padding:2px\">{t}</li>")).collect::<String>()) };
    let extra = if kind == ErrorKind::Blocked { "<a class=\"btn2\" href=\"about:settings\">Settings</a>" } else { "" };
    let home = if kind == ErrorKind::Http(404) {
        rabbithole_engine::net::Url::parse(url).filter(|u| u.is_http()).map(|u| format!("<a class=\"btn2\" href=\"{}://{}/\">Site home</a>", escape(&u.scheme), escape(&u.host))).unwrap_or_default()
    } else {
        String::new()
    };
    let retry = if kind == ErrorKind::Crashed { "Reload" } else { "Retry" };
    let body = format!(
        "<div class=\"err\"><div class=\"icon\">{icon}</div><h1>{}</h1><p>{text}</p>{tips}<p style=\"margin-top:22px\"><a class=\"btn\" href=\"{}\">{retry}</a>{home}{extra}<a class=\"btn2\" href=\"about:home\">Home</a></p><div class=\"details\">URL: <code>{}</code><br>Error: <code>{}</code></div></div>",
        escape(&title),
        escape(url),
        escape(url),
        escape(error)
    );
    format!("<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title>{}</head><body>{body}</body></html>", escape(&title), style())
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

/// `YYYY-MM-DD` (or Today / Yesterday) for a timestamp.
fn date_label(ts: u64) -> String {
    day_label(ts / 86400)
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
        h.add("https://rust-lang.org/", "Rust");
        let page = history_page(&h, "");
        assert!(page.contains("href=\"https://example.com/\""));
        assert!(page.contains("Example &lt;b&gt;"));
        assert!(page.contains("rabbithole://history/clear"));
        assert!(page.contains("rabbithole://history/delete?https%3A%2F%2Fexample.com%2F"));
        let page = history_page(&h, "rust");
        assert!(page.contains("rust-lang.org") && !page.contains("href=\"https://example.com/\""));
        assert!(history_page(&h, "zzz").contains("No history entries match"));
        let mut b = Bookmarks::default();
        b.toggle("https://rust-lang.org/", "Rust");
        let page = bookmarks_page(&b, None, false, None);
        assert!(page.contains("href=\"https://rust-lang.org/\""));
        assert!(page.contains("rabbithole://bookmark/remove?https%3A%2F%2Frust-lang.org%2F"));
        assert!(bookmarks_page(&b, Some("https://rust-lang.org/"), true, None).contains("rabbithole://bookmark/save"));
        let rows = vec![DownloadRow {
            id: 3,
            url: "https://x/f.zip".into(),
            filename: "f.zip".into(),
            path: "/tmp/f.zip".into(),
            done: 512,
            total: Some(1024),
            state: "Downloading".into(),
            active: true,
            paused: false,
            resumable: true,
            finished: false,
            failed: false,
            speed: Some(2048),
        }];
        let page = downloads_page(&rows, "/tmp", false);
        assert!(page.contains("rabbithole://download/cancel/3"));
        assert!(page.contains("rabbithole://download/pause/3"));
        assert!(page.contains("width:50%"));
        assert!(page.contains("2.0 KB/s"));
        assert!(history_page(&History::default(), "").contains("No pages visited"));
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(100), "100 B");
    }

    #[test]
    fn crash_page_offers_reload() {
        let page = error_page("https://example.com/a", "Tab crashed: index out of bounds at x.rs:1:2");
        assert!(page.contains("This tab crashed"), "{page}");
        assert!(page.contains("href=\"https://example.com/a\">Reload</a>"), "{page}");
        assert!(page.contains("index out of bounds"));
    }

    #[test]
    fn settings_page_has_controls() {
        let s = Settings::default();
        let sites = vec![SiteData { host: "example.com".into(), cookies: 2, zoom: Some(1.5) }];
        let page = settings_page(&s, &sites, "/home/u/Downloads", None, false);
        assert!(page.contains("rabbithole://settings/set?key=javascript&amp;value=0"));
        assert!(page.contains("<option value=\"duckduckgo\" selected>"));
        assert!(page.contains("rabbithole://data/clear-site?example.com"));
        assert!(page.contains("2 cookies, zoom 150%"));
        assert!(page.contains("<option value=\"1.00\" selected>"));
    }

    #[test]
    fn query_parsing() {
        assert_eq!(parse_query("key=homepage&value=https%3A%2F%2Fa.b%2F+x"), vec![("key".into(), "homepage".into()), ("value".into(), "https://a.b/ x".into())]);
        assert_eq!(query_value(Some("a=1&b=2"), "b").as_deref(), Some("2"));
        assert_eq!(query_value(None, "b"), None);
    }

    #[test]
    fn error_classification() {
        assert_eq!(classify_error("I/O error: failed to lookup address information: Name or service not known"), ErrorKind::Dns);
        assert_eq!(classify_error("I/O error: No such host is known. (os error 11001)"), ErrorKind::Dns);
        assert_eq!(classify_error("I/O error: invalid peer certificate: UnknownIssuer"), ErrorKind::Tls);
        assert_eq!(classify_error("I/O error: Network is unreachable (os error 101)"), ErrorKind::Offline);
        assert_eq!(classify_error("I/O error: Connection refused (os error 111)"), ErrorKind::Refused);
        assert_eq!(classify_error("request timed out"), ErrorKind::Timeout);
        assert_eq!(classify_error("blocked ad/tracker host: ads.example"), ErrorKind::Blocked);
        assert_eq!(classify_error("HTTP error 404"), ErrorKind::Http(404));
        assert_eq!(classify_error("HTTP error 503"), ErrorKind::Http(503));
        assert_eq!(classify_error("weird"), ErrorKind::Other);
        let page = error_page("https://nowhere.invalid/x", "I/O error: failed to lookup address information");
        assert!(page.contains("This site can’t be found"));
        assert!(page.contains("href=\"https://nowhere.invalid/x\">Retry"));
        assert!(error_page("https://a.com/missing", "HTTP error 404").contains("href=\"https://a.com/\">Site home"));
        assert!(error_page("https://bad.com/", "invalid peer certificate: Expired").contains("not private"));
    }

    #[test]
    fn dates() {
        assert_eq!(day_label(0), "1970-01-01");
        assert_eq!(day_label(19723), "2024-01-01");
        assert_eq!(time_label(3661), "01:01");
    }
}
