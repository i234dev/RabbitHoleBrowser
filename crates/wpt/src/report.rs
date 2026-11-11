//! Pass-rate summaries, the committed baseline format and `--compare`.
//!
//! Baseline format (one block per test page, sorted by URL):
//!
//! ```text
//! /dom/nodes/Node-appendChild.html	OK	12/15
//! 	FAIL	name of a failing subtest
//! 	TIMEOUT	another one
//! ```
//!
//! Subtest names have tabs and newlines escaped and are cut at 200
//! characters so the file stays readable in diffs.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;

use crate::runner::{Subtest, TestResult};

/// Directory key of a test URL at `depth` path components.
pub fn dir_key(url: &str, depth: usize) -> String {
    let path = url.split('?').next().unwrap_or(url).trim_start_matches('/');
    let parts: Vec<&str> = path.split('/').collect();
    let dirs = &parts[..parts.len().saturating_sub(1)];
    dirs.iter().take(depth).copied().collect::<Vec<_>>().join("/")
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Tally {
    pub files: usize,
    pub files_ok: usize,
    pub passed: usize,
    pub total: usize,
}

impl Tally {
    pub fn add(&mut self, passed: usize, total: usize, harness_ok: bool) {
        self.files += 1;
        self.files_ok += usize::from(harness_ok);
        self.passed += passed;
        self.total += total;
    }

    pub fn pct(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            100.0 * self.passed as f64 / self.total as f64
        }
    }
}

/// Per-directory tallies at `depth`, plus the grand total under "".
pub fn tally(entries: &[BaselineEntry], depth: usize) -> BTreeMap<String, Tally> {
    let mut out: BTreeMap<String, Tally> = BTreeMap::new();
    for e in entries {
        let ok = e.harness == "OK";
        out.entry(dir_key(&e.url, depth)).or_default().add(e.passed, e.total, ok);
        out.entry(String::new()).or_default().add(e.passed, e.total, ok);
    }
    out
}

/// Text table of per-directory pass rates.
pub fn summary_table(entries: &[BaselineEntry], depth: usize) -> String {
    let t = tally(entries, depth);
    let mut s = String::new();
    let _ = writeln!(s, "{:<52} {:>6} {:>9} {:>9} {:>7}", "directory", "files", "passed", "total", "pass%");
    for (k, v) in &t {
        if k.is_empty() {
            continue;
        }
        let _ = writeln!(s, "{:<52} {:>6} {:>9} {:>9} {:>6.1}%", k, v.files, v.passed, v.total, v.pct());
    }
    if let Some(v) = t.get("") {
        let _ = writeln!(s, "{:<52} {:>6} {:>9} {:>9} {:>6.1}%", "TOTAL", v.files, v.passed, v.total, v.pct());
    }
    s
}

/// One test page in a baseline.
#[derive(Debug, Clone, PartialEq)]
pub struct BaselineEntry {
    pub url: String,
    pub harness: String,
    pub passed: usize,
    pub total: usize,
    /// (status, escaped name) of every subtest that did not pass.
    pub failing: Vec<(String, String)>,
}

const NAME_LIMIT: usize = 200;

/// Escape and shorten a subtest name for the baseline.
pub fn clean_name(name: &str) -> String {
    let mut s = String::new();
    for c in name.chars() {
        match c {
            '\t' => s.push_str("\\t"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\\' => s.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(s, "\\x{:02x}", c as u32);
            }
            c => s.push(c),
        }
        if s.len() >= NAME_LIMIT {
            s.push('…');
            break;
        }
    }
    s
}

impl BaselineEntry {
    pub fn from_result(r: &TestResult) -> BaselineEntry {
        let (passed, total) = r.counts();
        let failing = r
            .subtests
            .iter()
            .filter(|s: &&Subtest| s.status != "PASS")
            .map(|s| (s.status.clone(), clean_name(&s.name)))
            .collect();
        BaselineEntry { url: r.url.clone(), harness: r.harness.clone(), passed, total, failing }
    }
}

pub fn write_baseline(entries: &[BaselineEntry]) -> String {
    let mut sorted: Vec<&BaselineEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.url.cmp(&b.url));
    let mut s = String::new();
    for e in sorted {
        let _ = writeln!(s, "{}\t{}\t{}/{}", e.url, e.harness, e.passed, e.total);
        for (st, name) in &e.failing {
            let _ = writeln!(s, "\t{st}\t{name}");
        }
    }
    s
}

pub fn parse_baseline(text: &str) -> Vec<BaselineEntry> {
    let mut out: Vec<BaselineEntry> = Vec::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('\t') {
            if let (Some(e), Some((st, name))) = (out.last_mut(), rest.split_once('\t')) {
                e.failing.push((st.to_string(), name.to_string()));
            }
            continue;
        }
        let mut it = line.split('\t');
        let (Some(url), Some(h), Some(c)) = (it.next(), it.next(), it.next()) else { continue };
        let (p, t) = c.split_once('/').unwrap_or(("0", "0"));
        out.push(BaselineEntry {
            url: url.to_string(),
            harness: h.to_string(),
            passed: p.parse().unwrap_or(0),
            total: t.parse().unwrap_or(0),
            failing: Vec::new(),
        });
    }
    out
}

/// Differences between a baseline and a new run (only pages present in
/// both are compared subtest by subtest).
#[derive(Debug, Default)]
pub struct Comparison {
    /// (url, subtest) that passed (were not listed as failing) before.
    pub regressions: Vec<(String, String)>,
    /// (url, subtest) listed as failing before and passing now.
    pub fixes: Vec<(String, String)>,
    /// Pages whose pass count dropped (also catches renamed subtests).
    pub count_drops: Vec<(String, usize, usize)>,
    pub before: Tally,
    pub after: Tally,
}

pub fn compare(old: &[BaselineEntry], new: &[BaselineEntry]) -> Comparison {
    let mut c = Comparison::default();
    let old_map: HashMap<&str, &BaselineEntry> = old.iter().map(|e| (e.url.as_str(), e)).collect();
    for n in new {
        let Some(o) = old_map.get(n.url.as_str()) else { continue };
        c.before.add(o.passed, o.total, o.harness == "OK");
        c.after.add(n.passed, n.total, n.harness == "OK");
        let old_fail: HashSet<&str> = o.failing.iter().map(|(_, n)| n.as_str()).collect();
        let new_fail: HashSet<&str> = n.failing.iter().map(|(_, n)| n.as_str()).collect();
        // A subtest failing now that the old run did not list as failing
        // regressed only if the old run had subtests at all.
        if o.total > 0 && !(o.failing.is_empty() && o.passed == 0) {
            for (_, name) in &n.failing {
                if !old_fail.contains(name.as_str()) {
                    c.regressions.push((n.url.clone(), name.clone()));
                }
            }
        }
        for (_, name) in &o.failing {
            if !new_fail.contains(name.as_str()) && n.passed > 0 {
                c.fixes.push((n.url.clone(), name.clone()));
            }
        }
        if n.passed < o.passed {
            c.count_drops.push((n.url.clone(), o.passed, n.passed));
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(url: &str, passed: usize, total: usize, failing: &[&str]) -> BaselineEntry {
        BaselineEntry {
            url: url.into(),
            harness: "OK".into(),
            passed,
            total,
            failing: failing.iter().map(|f| ("FAIL".to_string(), f.to_string())).collect(),
        }
    }

    #[test]
    fn baseline_round_trip() {
        let e = vec![entry("/a/b.html", 1, 3, &["x\\ty", "z"]), entry("/a/c.html", 2, 2, &[])];
        let text = write_baseline(&e);
        assert_eq!(parse_baseline(&text), e);
    }

    #[test]
    fn dir_keys() {
        assert_eq!(dir_key("/dom/nodes/a.html?x=/y", 2), "dom/nodes");
        assert_eq!(dir_key("/url/a.any.html", 2), "url");
        assert_eq!(dir_key("/html/syntax/parsing/x.html", 2), "html/syntax");
    }

    #[test]
    fn regressions_and_fixes() {
        let old = vec![entry("/a.html", 1, 3, &["b", "c"])];
        let new = vec![entry("/a.html", 1, 3, &["a", "c"])];
        let c = compare(&old, &new);
        assert_eq!(c.regressions, vec![("/a.html".to_string(), "a".to_string())]);
        assert_eq!(c.fixes, vec![("/a.html".to_string(), "b".to_string())]);
    }
}
