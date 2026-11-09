//! Bookmarks: `url<TAB>title` lines.

use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Bookmarks {
    pub items: Vec<Bookmark>,
}

fn clean(s: &str) -> String {
    s.chars().map(|c| if c == '\t' || c == '\n' || c == '\r' { ' ' } else { c }).collect()
}

impl Bookmarks {
    pub fn parse(text: &str) -> Bookmarks {
        let items = text
            .lines()
            .filter_map(|l| {
                let (url, title) = l.split_once('\t').unwrap_or((l, ""));
                let url = url.trim();
                if url.is_empty() {
                    None
                } else {
                    Some(Bookmark { url: url.to_string(), title: title.trim().to_string() })
                }
            })
            .collect();
        Bookmarks { items }
    }

    pub fn serialize(&self) -> String {
        self.items.iter().map(|b| format!("{}\t{}\n", clean(&b.url), clean(&b.title))).collect()
    }

    pub fn load(path: &Path) -> Bookmarks {
        std::fs::read_to_string(path).map(|t| Bookmarks::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        let _ = std::fs::write(path, self.serialize());
    }

    pub fn contains(&self, url: &str) -> bool {
        self.items.iter().any(|b| b.url == url)
    }

    /// Add or remove; returns true when the page is now bookmarked.
    pub fn toggle(&mut self, url: &str, title: &str) -> bool {
        if self.contains(url) {
            self.items.retain(|b| b.url != url);
            false
        } else {
            self.items.push(Bookmark { url: url.to_string(), title: title.to_string() });
            true
        }
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<&Bookmark> {
        let q = query.to_lowercase();
        self.items
            .iter()
            .filter(|b| b.url.to_lowercase().contains(&q) || b.title.to_lowercase().contains(&q))
            .take(limit)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_toggle() {
        let mut b = Bookmarks::default();
        assert!(b.toggle("https://a/", "A\tsite"));
        assert!(b.toggle("https://b/", ""));
        let back = Bookmarks::parse(&b.serialize());
        assert_eq!(back.items.len(), 2);
        assert_eq!(back.items[0].title, "A site");
        assert!(back.contains("https://b/"));
        let mut back = back;
        assert!(!back.toggle("https://a/", "x"));
        assert_eq!(back.items.len(), 1);
        assert_eq!(back.search("b/", 5).len(), 1);
    }
}
