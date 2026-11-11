//! The omnibox (address bar) logic, independent of the window: turning
//! typed text into a URL or a search, ranking suggestions from history and
//! bookmarks, inline autocompletion and the security indicator.

use crate::bookmarks::Bookmarks;
use crate::history::History;
use crate::settings::SearchEngine;

/// Where a suggestion comes from (drawn with a different icon).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionKind {
    /// "Search for …" with the configured engine.
    Search,
    /// The typed text is itself an address.
    Url,
    Bookmark,
    History,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub kind: SuggestionKind,
    /// URL to navigate to.
    pub url: String,
    /// Text shown (title or "Search DuckDuckGo for …").
    pub title: String,
}

/// Percent-encode a search query (space as `+`).
pub fn encode_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Heuristic: does the typed text look like a URL rather than a search?
pub fn looks_like_url(s: &str) -> bool {
    if s.contains(char::is_whitespace) {
        return false;
    }
    if s.contains("://") {
        return true;
    }
    let lower = s.to_ascii_lowercase();
    if ["about:", "file:", "data:", "mailto:", "rabbithole:", "view-source:"].iter().any(|p| lower.starts_with(p)) {
        return true;
    }
    if lower == "localhost" || lower.starts_with("localhost:") || lower.starts_with("localhost/") {
        return true;
    }
    if lower.starts_with('/') && std::path::Path::new(s).exists() {
        return true;
    }
    let host = lower.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    // host:port
    let host_only = match host.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => h,
        _ => host,
    };
    if !host_only.is_empty() && host_only.split('.').count() == 4 && host_only.split('.').all(|p| p.parse::<u8>().is_ok()) {
        return true; // IPv4
    }
    if host_only.starts_with('[') && host_only.ends_with(']') {
        return true; // IPv6
    }
    if host_only.contains('.') && !host_only.starts_with('.') && !host_only.ends_with('.') {
        let tld = host_only.rsplit('.').next().unwrap_or("");
        return tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()) || (tld.len() >= 2 && tld.starts_with("xn--"));
    }
    false
}

/// Turn typed text into a URL (search fallback with `engine`).
pub fn resolve_input(input: &str, engine: SearchEngine, homepage: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return homepage.to_string();
    }
    // "? query" forces a search.
    if let Some(q) = trimmed.strip_prefix('?') {
        return engine.url(&encode_query(q.trim()));
    }
    if looks_like_url(trimmed) {
        trimmed.to_string()
    } else {
        engine.url(&encode_query(trimmed))
    }
}

/// The URL without the scheme and `www.` (for matching typed prefixes).
pub fn strip_url(url: &str) -> &str {
    let u = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    u.strip_prefix("www.").unwrap_or(u)
}

/// Rank suggestions for the typed text. The first row is the "what Enter
/// does" row (search or URL); then bookmarks and history ranked by
/// prefix matches, visit counts and recency.
pub fn suggest(input: &str, history: &History, bookmarks: &Bookmarks, engine: SearchEngine, limit: usize) -> Vec<Suggestion> {
    let q = input.trim();
    let mut out = Vec::new();
    if q.is_empty() {
        return out;
    }
    let ql = q.to_lowercase();
    if looks_like_url(q) {
        out.push(Suggestion { kind: SuggestionKind::Url, url: q.to_string(), title: String::new() });
    } else {
        out.push(Suggestion { kind: SuggestionKind::Search, url: engine.url(&encode_query(q)), title: format!("{q} — Search with {}", engine.name()) });
    }
    let counts = history.visit_counts();
    let n_entries = history.entries.len().max(1) as f32;
    // (score, suggestion)
    let mut scored: Vec<(f32, Suggestion)> = Vec::new();
    let score = |url: &str, title: &str, bonus: f32, recency: f32, visits: u32| -> Option<f32> {
        let ul = url.to_lowercase();
        let stripped = strip_url(&ul);
        let tl = title.to_lowercase();
        let mut s = if stripped.starts_with(&ql) {
            100.0
        } else if ul.starts_with(&ql) {
            90.0
        } else if stripped.split(['/', '.', '-', '?', '=']).any(|w| w.starts_with(&ql)) {
            60.0
        } else if tl.split_whitespace().any(|w| w.starts_with(&ql)) {
            50.0
        } else if ul.contains(&ql) || tl.contains(&ql) {
            25.0
        } else {
            return None;
        };
        // Shorter URLs (site roots) win among prefix matches.
        s -= (stripped.len() as f32 / 20.0).min(10.0);
        s += bonus + (visits as f32).ln_1p() * 8.0 + recency * 10.0;
        Some(s)
    };
    for b in &bookmarks.items {
        if let Some(s) = score(&b.url, &b.title, 6.0, 0.5, counts.get(b.url.as_str()).copied().unwrap_or(0)) {
            scored.push((s, Suggestion { kind: SuggestionKind::Bookmark, url: b.url.clone(), title: b.title.clone() }));
        }
    }
    let mut seen = std::collections::HashSet::new();
    for (i, e) in history.entries.iter().enumerate().rev() {
        if !seen.insert(e.url.as_str()) {
            continue;
        }
        let recency = i as f32 / n_entries;
        if let Some(s) = score(&e.url, &e.title, 0.0, recency, counts.get(e.url.as_str()).copied().unwrap_or(1)) {
            scored.push((s, Suggestion { kind: SuggestionKind::History, url: e.url.clone(), title: e.title.clone() }));
        }
        if seen.len() > 3000 {
            break;
        }
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut urls = std::collections::HashSet::new();
    urls.insert(out[0].url.clone());
    for (_, s) in scored {
        if out.len() >= limit {
            break;
        }
        if urls.insert(s.url.clone()) {
            out.push(s);
        }
    }
    out
}

/// Inline completion: when the typed text is a prefix of a known site,
/// the text to append (selected, so typing on replaces it). Completes to
/// the end of the host (`git` → `github.com/`) or of the next path
/// segment.
pub fn inline_completion(typed: &str, candidates: &[Suggestion]) -> Option<String> {
    if typed.is_empty() || typed.contains(char::is_whitespace) {
        return None;
    }
    let tl = typed.to_lowercase();
    for c in candidates.iter().filter(|c| matches!(c.kind, SuggestionKind::Bookmark | SuggestionKind::History)) {
        let full = c.url.as_str();
        // Compare against the URL with and without scheme / www.
        let forms = [full, full.split_once("://").map(|(_, r)| r).unwrap_or(full), strip_url(full)];
        for form in forms {
            if form.len() > typed.len() && form.to_lowercase().starts_with(&tl) && form.is_char_boundary(typed.len()) {
                let rest = &form[typed.len()..];
                // Up to and including the next '/'.
                let cut = rest.find('/').map(|i| i + 1).unwrap_or(rest.len());
                let completion = &rest[..cut];
                if completion.is_empty() {
                    continue;
                }
                return Some(completion.to_string());
            }
        }
    }
    None
}

/// Connection security shown at the left of the address bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    /// https.
    Secure,
    /// Plain http to a remote host.
    Insecure,
    /// Internal pages, local files, data: URLs, localhost.
    Local,
    /// Nothing loaded.
    None,
}

pub fn security_of(url: Option<&rabbithole_engine::net::Url>) -> Security {
    let Some(u) = url else { return Security::None };
    match u.scheme.as_str() {
        "https" | "wss" => Security::Secure,
        "http" | "ws" => {
            let h = u.host.as_str();
            if h == "localhost" || h == "127.0.0.1" || h == "[::1]" || h.ends_with(".localhost") {
                Security::Local
            } else {
                Security::Insecure
            }
        }
        _ => Security::Local,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_detection() {
        assert!(looks_like_url("example.com"));
        assert!(looks_like_url("https://example.com/x?y"));
        assert!(looks_like_url("about:home"));
        assert!(looks_like_url("localhost:8080"));
        assert!(looks_like_url("sub.example.co.uk/path"));
        assert!(looks_like_url("192.168.1.1:8000/x"));
        assert!(looks_like_url("example.com:8080"));
        assert!(!looks_like_url("rust browser engine"));
        assert!(!looks_like_url("hello"));
        assert!(!looks_like_url("what is 2.5"));
        assert!(!looks_like_url("3.14"));
    }

    #[test]
    fn search_fallback() {
        let d = SearchEngine::DuckDuckGo;
        assert_eq!(resolve_input("  example.org ", d, "about:home"), "example.org");
        assert_eq!(resolve_input("rust & ünicode", d, "about:home"), "https://duckduckgo.com/html/?q=rust+%26+%C3%BCnicode");
        assert_eq!(resolve_input("", d, "about:home"), "about:home");
        assert_eq!(resolve_input("? example.org", SearchEngine::Google, ""), "https://www.google.com/search?q=example.org");
    }

    fn sample() -> (History, Bookmarks) {
        let mut h = History::default();
        h.add("https://github.com/rust-lang/rust", "rust-lang/rust");
        h.add("https://github.com/", "GitHub");
        h.add("https://github.com/", "GitHub");
        h.add("https://news.example.org/item?id=1", "Gitting things done");
        let mut b = Bookmarks::default();
        b.toggle("https://www.gitlab.com/", "GitLab");
        (h, b)
    }

    #[test]
    fn ranking() {
        let (h, b) = sample();
        let s = suggest("git", &h, &b, SearchEngine::DuckDuckGo, 6);
        assert_eq!(s[0].kind, SuggestionKind::Search);
        // The frequently visited root wins; the bookmark ranks high too.
        assert_eq!(s[1].url, "https://github.com/");
        assert!(s.iter().any(|x| x.kind == SuggestionKind::Bookmark && x.url == "https://www.gitlab.com/"));
        // Title-only matches come last.
        assert_eq!(s.last().unwrap().url, "https://news.example.org/item?id=1");
        let s = suggest("github.com", &h, &b, SearchEngine::DuckDuckGo, 6);
        assert_eq!(s[0].kind, SuggestionKind::Url);
        assert!(suggest("   ", &h, &b, SearchEngine::DuckDuckGo, 6).is_empty());
    }

    #[test]
    fn inline() {
        let (h, b) = sample();
        let s = suggest("gith", &h, &b, SearchEngine::DuckDuckGo, 6);
        assert_eq!(inline_completion("gith", &s).as_deref(), Some("ub.com/"));
        let s = suggest("github.com/r", &h, &b, SearchEngine::DuckDuckGo, 6);
        assert_eq!(inline_completion("github.com/r", &s).as_deref(), Some("ust-lang/"));
        let s = suggest("https://gith", &h, &b, SearchEngine::DuckDuckGo, 6);
        assert_eq!(inline_completion("https://gith", &s).as_deref(), Some("ub.com/"));
        let s = suggest("gitl", &h, &b, SearchEngine::DuckDuckGo, 6);
        assert_eq!(inline_completion("gitl", &s).as_deref(), Some("ab.com/"));
        assert_eq!(inline_completion("git hub", &s), None);
    }

    #[test]
    fn security() {
        use rabbithole_engine::net::Url;
        assert_eq!(security_of(Url::parse("https://a.com/").as_ref()), Security::Secure);
        assert_eq!(security_of(Url::parse("http://a.com/").as_ref()), Security::Insecure);
        assert_eq!(security_of(Url::parse("http://localhost:8000/").as_ref()), Security::Local);
        assert_eq!(security_of(Url::parse("about:history").as_ref()), Security::Local);
        assert_eq!(security_of(None), Security::None);
    }
}
