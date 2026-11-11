//! Finding the testharness.js tests (and their variants) in a checkout.

use std::path::{Path, PathBuf};

use crate::server::parse_meta;

/// One test to load: a URL path (plus variant query).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TestId {
    /// `/dom/nodes/Node-appendChild.html?variant`.
    pub url: String,
    /// `<meta name=timeout content=long>` / `// META: timeout=long`.
    pub long: bool,
}

/// Directories excluded anywhere in a path (support files, not tests).
const SUPPORT_DIRS: &[&str] = &["resources", "support", "tools", "reference", "references", "crashtests", "__pycache__"];

/// Collect the tests under `rel` (a directory or a file, relative to
/// `root`), sorted.
pub fn discover(root: &Path, rel: &str) -> Vec<TestId> {
    let mut out = Vec::new();
    let start = root.join(rel.trim_start_matches('/'));
    if start.is_file() {
        tests_in_file(root, &start, &mut out);
    } else {
        walk(root, &start, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<TestId>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        if p.is_dir() {
            if SUPPORT_DIRS.contains(&name.as_str()) || name.starts_with('.') {
                continue;
            }
            walk(root, &p, out);
        } else {
            tests_in_file(root, &p, out);
        }
    }
}

fn url_of(root: &Path, p: &Path) -> Option<String> {
    let rel = p.strip_prefix(root).ok()?;
    let s: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    Some(format!("/{}", s.join("/")))
}

/// Is the file name one the runner never loads directly?
fn skipped_name(name: &str) -> bool {
    name.contains("-manual.")
        || name.contains("-crash.")
        || name.contains("-ref.")
        || name.contains("-notref.")
        || name.contains(".sharedworker.")
        || name.contains(".serviceworker.")
        || name.contains(".shadowrealm")
        || name.starts_with('_')
        || name.starts_with('.')
}

fn tests_in_file(root: &Path, p: &Path, out: &mut Vec<TestId>) {
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if skipped_name(name) {
        return;
    }
    let Some(url) = url_of(root, p) else { return };
    if url.split('/').any(|c| SUPPORT_DIRS.contains(&c)) {
        return;
    }
    if let Some(base) = url.strip_suffix(".any.js") {
        let Ok(src) = std::fs::read_to_string(p) else { return };
        let meta = parse_meta(&src);
        let long = meta.iter().any(|(k, v)| k == "timeout" && v == "long");
        let globals: Vec<String> = meta
            .iter()
            .filter(|(k, _)| k == "global")
            .flat_map(|(_, v)| v.split(',').map(|g| g.trim().to_string()).collect::<Vec<_>>())
            .collect();
        let (window, worker) = if globals.is_empty() {
            (true, true)
        } else {
            (
                globals.iter().any(|g| g == "window"),
                globals.iter().any(|g| g == "worker" || g == "dedicatedworker" || g == "dedicatedworker-module"),
            )
        };
        let variants = variants(&meta);
        for v in &variants {
            if window {
                out.push(TestId { url: format!("{base}.any.html{v}"), long });
            }
            if worker {
                out.push(TestId { url: format!("{base}.any.worker.html{v}"), long });
            }
        }
        return;
    }
    for (suffix, page) in [(".window.js", ".window.html"), (".worker.js", ".worker.html")] {
        if let Some(base) = url.strip_suffix(suffix) {
            if base.ends_with(".any") {
                return;
            }
            let Ok(src) = std::fs::read_to_string(p) else { return };
            let meta = parse_meta(&src);
            let long = meta.iter().any(|(k, v)| k == "timeout" && v == "long");
            for v in variants(&meta) {
                out.push(TestId { url: format!("{base}{page}{v}"), long });
            }
            return;
        }
    }
    let lower = name.to_ascii_lowercase();
    if !(lower.ends_with(".html") || lower.ends_with(".htm") || lower.ends_with(".xhtml") || lower.ends_with(".xht")) {
        return;
    }
    let Ok(bytes) = std::fs::read(p) else { return };
    let src = String::from_utf8_lossy(&bytes);
    if !src.contains("/resources/testharness.js") {
        return;
    }
    // Reftests that also load testharness.js are not testharness tests.
    let lower_src = src.to_ascii_lowercase();
    if lower_src.contains("rel=\"match\"") || lower_src.contains("rel=match") || lower_src.contains("rel='match'") {
        return;
    }
    let long = lower_src.contains("name=\"timeout\" content=\"long\"") || lower_src.contains("name=timeout content=long");
    for v in html_variants(&src) {
        out.push(TestId { url: format!("{url}{v}"), long });
    }
}

fn variants(meta: &[(String, String)]) -> Vec<String> {
    let v: Vec<String> = meta.iter().filter(|(k, _)| k == "variant").map(|(_, v)| v.clone()).collect();
    if v.is_empty() {
        vec![String::new()]
    } else {
        v
    }
}

/// `<meta name="variant" content="?...">` values (or one empty variant).
fn html_variants(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let lower = src.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find("<meta") {
        let start = from + i;
        let end = lower[start..].find('>').map(|e| start + e).unwrap_or(lower.len());
        let tag = &src[start..end];
        let tag_l = &lower[start..end];
        if tag_l.contains("name=\"variant\"") || tag_l.contains("name=variant") || tag_l.contains("name='variant'") {
            if let Some(c) = attr_value(tag, tag_l, "content") {
                out.push(c);
            }
        }
        from = end;
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn attr_value(tag: &str, tag_l: &str, name: &str) -> Option<String> {
    let i = tag_l.find(&format!("{name}="))? + name.len() + 1;
    let rest = &tag[i..];
    let q = rest.chars().next()?;
    if q == '"' || q == '\'' {
        let r = &rest[1..];
        Some(r[..r.find(q)?].to_string())
    } else {
        Some(rest.split(|c: char| c.is_whitespace() || c == '>').next()?.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_variant_meta() {
        let v = html_variants("<meta name=\"variant\" content=\"?1-10\">\n<meta name=variant content='?11-20'><meta charset=utf-8>");
        assert_eq!(v, vec!["?1-10".to_string(), "?11-20".to_string()]);
        assert_eq!(html_variants("<p>"), vec![String::new()]);
    }
}
