//! Network listing, response bodies, and API hints: which JSON responses
//! the content on the page came from.

use std::collections::HashSet;

use rabbithole_engine::net::{NetLogEntry, Url};
use serde_json::Value;

use crate::ax::{clip_text, esc};
use crate::browser::{fence, human_size, Tab};

fn path_of(url: &str, origin: &str) -> String {
    match Url::parse(url) {
        Some(u) => {
            let mut s = if u.origin() == origin { String::new() } else { u.origin() };
            s.push_str(&u.path);
            if let Some(q) = &u.query {
                s.push('?');
                s.push_str(q);
            }
            clip_text(&s, 140)
        }
        None => clip_text(url, 140),
    }
}

/// One line per request.
pub fn line(e: &NetLogEntry, origin: &str) -> String {
    let status = match (&e.error, e.status) {
        (Some(err), _) => format!("ERR({})", clip_text(err, 40)),
        (None, Some(s)) => s.to_string(),
        (None, None) => "pending".into(),
    };
    let ct = e.content_type.split(';').next().unwrap_or("").to_string();
    let size = e.size.map(human_size).unwrap_or_else(|| "-".into());
    let time = match e.duration {
        Some(d) => format!("{} ms", d.as_millis()),
        None => format!("{:.1} s so far", e.started.elapsed().as_secs_f32()),
    };
    format!("#{} {} {} {} {} {} {} {}", e.id, e.method, status, e.kind(), if ct.is_empty() { "-".into() } else { ct }, size, time, esc(&path_of(&e.url, origin)))
}

/// `network` listing with optional filters.
pub fn list(tab: &Tab, filter: Option<&str>, kind: Option<&str>, all: bool, limit: usize) -> String {
    let origin = tab.page().map(|p| p.url.origin()).unwrap_or_default();
    let entries = tab.netlog.entries();
    let f = filter.map(|f| f.to_lowercase());
    let sel: Vec<&NetLogEntry> = entries
        .iter()
        .filter(|e| all || !matches!(e.kind(), "image" | "font" | "stylesheet"))
        .filter(|e| kind.map(|k| e.kind() == k || (k == "xhr" && e.kind() == "fetch")).unwrap_or(true))
        .filter(|e| f.as_ref().map(|f| e.url.to_lowercase().contains(f)).unwrap_or(true))
        .collect();
    let total = sel.len();
    let shown: Vec<String> = sel.iter().rev().take(limit).rev().map(|e| line(e, &origin)).collect();
    let mut out = String::new();
    if total > shown.len() {
        out.push_str(&format!("({} earlier requests not shown)\n", total - shown.len()));
    }
    if shown.is_empty() {
        out.push_str("(no matching requests)");
    } else {
        out.push_str(&fence(&shown.join("\n")));
    }
    let hidden = entries.iter().filter(|e| matches!(e.kind(), "image" | "font" | "stylesheet")).count();
    if !all && hidden > 0 {
        out.push_str(&format!("\n({hidden} image/font/stylesheet requests hidden; all=true lists them)"));
    }
    let h = page_hints(tab);
    if !h.is_empty() {
        out.push('\n');
        out.push_str(&h.join("\n"));
    }
    out
}

/// A response body (JSON pretty-printed), fenced.
pub fn body(tab: &Tab, id: u64, max_chars: usize) -> Result<String, String> {
    let e = tab.netlog.get(id).ok_or_else(|| format!("no request #{id} in this tab's log"))?;
    let origin = tab.page().map(|p| p.url.origin()).unwrap_or_default();
    let mut out = line(&e, &origin);
    if let Some(rb) = &e.request_body {
        out.push_str(&format!("\nrequest body: {}", fence(&clip_text(rb, 2000))));
    }
    let Some(b) = &e.body else {
        out.push_str("\n(body not kept: only API calls, documents and textual responses up to 2 MB are)");
        return Ok(out);
    };
    let text = String::from_utf8_lossy(b);
    let pretty = serde_json::from_str::<Value>(&text).ok().and_then(|v| serde_json::to_string_pretty(&v).ok()).unwrap_or_else(|| text.to_string());
    let shown = clip_text(&pretty, max_chars);
    out.push('\n');
    out.push_str(&fence(&shown));
    if pretty.chars().count() > max_chars {
        out.push_str(&format!("\n(body cut at {max_chars} chars of {})", pretty.chars().count()));
    }
    Ok(out)
}

/// String values (≥ 6 chars) of a JSON document.
fn json_strings(v: &Value, out: &mut HashSet<String>, budget: &mut usize) {
    if *budget == 0 {
        return;
    }
    match v {
        Value::String(s) => {
            let t = s.split_whitespace().collect::<Vec<_>>().join(" ");
            if t.chars().count() >= 6 && !t.starts_with("http") {
                *budget -= 1;
                out.insert(clip_text(&t, 60).trim_end_matches('…').to_lowercase());
            }
        }
        Value::Array(a) => a.iter().for_each(|x| json_strings(x, out, budget)),
        Value::Object(o) => o.values().for_each(|x| json_strings(x, out, budget)),
        _ => {}
    }
}

fn api_json(tab: &Tab) -> Vec<(NetLogEntry, HashSet<String>)> {
    tab.netlog
        .entries()
        .into_iter()
        .filter(|e| e.api && e.status.map(|s| s < 400).unwrap_or(false))
        .filter_map(|e| {
            let b = e.body.clone()?;
            let v: Value = serde_json::from_slice(&b).ok()?;
            let mut set = HashSet::new();
            let mut budget = 400;
            json_strings(&v, &mut set, &mut budget);
            (!set.is_empty()).then_some((e, set))
        })
        .collect()
}

/// Hints for lines just added by an action.
pub fn hints(tab: &Tab, diff_lines: &[String]) -> Vec<String> {
    let added: Vec<String> = diff_lines.iter().filter(|l| l.starts_with("+ ") || l.starts_with("~ ")).map(|l| l.to_lowercase()).collect();
    if added.len() < 2 {
        return Vec::new();
    }
    let origin = tab.page().map(|p| p.url.origin()).unwrap_or_default();
    let mut out = Vec::new();
    for (e, set) in api_json(tab).into_iter().rev().take(10) {
        let matched = added.iter().filter(|l| set.iter().any(|s| l.contains(s.as_str()))).count();
        if matched >= 2 {
            out.push(format!(
                "API hint: the new content ({matched} items) comes from {} {} — read it as JSON with network id={}",
                e.method,
                esc(&path_of(&e.url, &origin)),
                e.id
            ));
            break;
        }
    }
    out
}

/// Hints for the current page content.
pub fn page_hints(tab: &Tab) -> Vec<String> {
    let Some(page) = tab.page() else { return Vec::new() };
    let text = page.doc.body().map(|b| page.doc.text_content(b)).unwrap_or_default();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    let origin = page.url.origin();
    let mut out = Vec::new();
    for (e, set) in api_json(tab).into_iter().rev().take(20) {
        let matched = set.iter().filter(|s| text.contains(s.as_str())).count();
        if matched >= 3 {
            out.push(format!("API hint: {matched} values shown on the page come from {} {} (network id={})", e.method, esc(&path_of(&e.url, &origin)), e.id));
        }
        if out.len() >= 3 {
            break;
        }
    }
    out
}
