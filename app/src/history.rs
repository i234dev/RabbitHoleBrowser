//! Browsing history: an append-only text file, one visit per line.

use crate::profile::now_secs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub timestamp: u64,
    pub url: String,
    pub title: String,
}

/// Most recent visits kept in memory (newest last).
#[derive(Debug, Default)]
pub struct History {
    pub entries: Vec<HistoryEntry>,
    /// Lines not yet flushed to disk.
    pending: Vec<String>,
}

/// Maximum entries kept in memory / shown.
pub const MAX_ENTRIES: usize = 5000;

fn clean(s: &str) -> String {
    s.chars().map(|c| if c == '\t' || c == '\n' || c == '\r' { ' ' } else { c }).collect()
}

impl History {
    pub fn parse(text: &str) -> History {
        let mut entries = Vec::new();
        for line in text.lines() {
            let mut parts = line.splitn(3, '\t');
            let (Some(ts), Some(url)) = (parts.next(), parts.next()) else { continue };
            let Ok(timestamp) = ts.trim().parse::<u64>() else { continue };
            if url.trim().is_empty() {
                continue;
            }
            entries.push(HistoryEntry {
                timestamp,
                url: url.trim().to_string(),
                title: parts.next().unwrap_or("").trim().to_string(),
            });
        }
        if entries.len() > MAX_ENTRIES {
            entries.drain(..entries.len() - MAX_ENTRIES);
        }
        History { entries, pending: Vec::new() }
    }

    pub fn load(path: &Path) -> History {
        std::fs::read_to_string(path).map(|t| History::parse(&t)).unwrap_or_default()
    }

    pub fn serialize_entry(e: &HistoryEntry) -> String {
        format!("{}\t{}\t{}", e.timestamp, clean(&e.url), clean(&e.title))
    }

    /// Record a visit (internal `about:` pages are not recorded).
    pub fn add(&mut self, url: &str, title: &str) {
        if url.starts_with("about:") || url.starts_with("rabbithole:") {
            return;
        }
        let e = HistoryEntry { timestamp: now_secs(), url: url.to_string(), title: title.to_string() };
        self.pending.push(History::serialize_entry(&e));
        self.entries.push(e);
        if self.entries.len() > MAX_ENTRIES {
            self.entries.remove(0);
        }
    }

    /// Append pending lines to the file.
    pub fn flush(&mut self, path: &Path) {
        if self.pending.is_empty() {
            return;
        }
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            for line in &self.pending {
                let _ = writeln!(f, "{line}");
            }
        }
        self.pending.clear();
    }

    /// Newest entries first, deduplicated by URL (latest visit kept).
    pub fn recent(&self, limit: usize) -> Vec<&HistoryEntry> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for e in self.entries.iter().rev() {
            if seen.insert(e.url.as_str()) {
                out.push(e);
                if out.len() >= limit {
                    break;
                }
            }
        }
        out
    }

    /// Entries matching `query` (case-insensitive substring of URL or title), newest first.
    pub fn search(&self, query: &str, limit: usize) -> Vec<&HistoryEntry> {
        let q = query.to_lowercase();
        self.recent(MAX_ENTRIES)
            .into_iter()
            .filter(|e| e.url.to_lowercase().contains(&q) || e.title.to_lowercase().contains(&q))
            .take(limit)
            .collect()
    }

    /// Number of visits per URL (for omnibox ranking).
    pub fn visit_counts(&self) -> std::collections::HashMap<&str, u32> {
        let mut m = std::collections::HashMap::new();
        for e in &self.entries {
            *m.entry(e.url.as_str()).or_insert(0) += 1;
        }
        m
    }

    /// Delete every visit of `url` and rewrite the file.
    pub fn remove_url(&mut self, url: &str, path: &Path) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.url != url);
        let changed = self.entries.len() != before;
        if changed {
            self.rewrite(path);
        }
        changed
    }

    /// Delete the visits of one day (days since the epoch, UTC).
    pub fn remove_day(&mut self, day: u64, path: &Path) {
        self.entries.retain(|e| e.timestamp / 86400 != day);
        self.rewrite(path);
    }

    /// Replace the file with the in-memory entries.
    pub fn rewrite(&mut self, path: &Path) {
        self.pending.clear();
        let text: String = self.entries.iter().map(|e| History::serialize_entry(e) + "\n").collect();
        let _ = std::fs::write(path, text);
    }

    pub fn clear(&mut self, path: &Path) {
        self.entries.clear();
        self.pending.clear();
        let _ = std::fs::write(path, "");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_search() {
        let mut h = History::default();
        h.add("https://example.com/a", "Example\tA");
        h.add("https://rust-lang.org/", "Rust");
        h.add("https://example.com/a", "Example again");
        h.add("about:home", "home");
        let text: String = h.entries.iter().map(|e| History::serialize_entry(e) + "\n").collect();
        let back = History::parse(&text);
        assert_eq!(back.entries.len(), 3);
        assert_eq!(back.entries[0].title, "Example A");
        let recent = back.recent(10);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].url, "https://example.com/a");
        assert_eq!(recent[0].title, "Example again");
        assert_eq!(back.search("rust", 5).len(), 1);
        assert!(History::parse("garbage\nx\ty\n").entries.is_empty());
        assert_eq!(back.visit_counts()["https://example.com/a"], 2);
    }

    #[test]
    fn remove_entries() {
        let dir = std::env::temp_dir().join(format!("rh-hist-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("history.txt");
        let mut h = History::default();
        h.add("https://a/", "A");
        h.add("https://b/", "B");
        h.add("https://a/", "A2");
        h.flush(&path);
        assert!(h.remove_url("https://a/", &path));
        assert!(!h.remove_url("https://zzz/", &path));
        assert_eq!(History::load(&path).entries.len(), 1);
        let day = h.entries[0].timestamp / 86400;
        h.remove_day(day, &path);
        assert!(History::load(&path).entries.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
