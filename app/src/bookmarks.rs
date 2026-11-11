//! Bookmarks: `url<TAB>title<TAB>folder` lines, plus import/export in the
//! Netscape bookmark file format every browser reads and writes.

use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
    /// Folder path ("" = top level; "Work/Docs" for nested folders).
    pub folder: String,
}

impl Bookmark {
    pub fn new(url: &str, title: &str) -> Bookmark {
        Bookmark { url: url.to_string(), title: title.to_string(), folder: String::new() }
    }

    pub fn label(&self) -> &str {
        if self.title.trim().is_empty() {
            &self.url
        } else {
            self.title.trim()
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Bookmarks {
    pub items: Vec<Bookmark>,
}

fn clean(s: &str) -> String {
    s.chars().map(|c| if c == '\t' || c == '\n' || c == '\r' { ' ' } else { c }).collect()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn html_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
            _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Value of attribute `name` in a tag's source (`<A HREF="..." ...>`).
fn tag_attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let after = lower[at + name.len()..].trim_start();
        if before_ok && after.starts_with('=') {
            let vstart = tag.len() - after.len() + 1;
            let v = tag[vstart..].trim_start();
            let (val, _) = match v.chars().next() {
                Some(q @ ('"' | '\'')) => {
                    let inner = &v[1..];
                    (inner[..inner.find(q).unwrap_or(inner.len())].to_string(), ())
                }
                _ => (v[..v.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(v.len())].to_string(), ()),
            };
            return Some(html_unescape(&val));
        }
        from = at + name.len();
    }
    None
}

impl Bookmarks {
    pub fn parse(text: &str) -> Bookmarks {
        let items = text
            .lines()
            .filter_map(|l| {
                let mut f = l.splitn(3, '\t');
                let url = f.next()?.trim();
                if url.is_empty() {
                    return None;
                }
                let title = f.next().unwrap_or("").trim();
                let folder = f.next().unwrap_or("").trim();
                Some(Bookmark { url: url.to_string(), title: title.to_string(), folder: folder.to_string() })
            })
            .collect();
        Bookmarks { items }
    }

    pub fn serialize(&self) -> String {
        self.items
            .iter()
            .map(|b| if b.folder.is_empty() { format!("{}\t{}\n", clean(&b.url), clean(&b.title)) } else { format!("{}\t{}\t{}\n", clean(&b.url), clean(&b.title), clean(&b.folder)) })
            .collect()
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
            self.items.push(Bookmark::new(url, title));
            true
        }
    }

    pub fn remove(&mut self, url: &str) -> bool {
        let before = self.items.len();
        self.items.retain(|b| b.url != url);
        before != self.items.len()
    }

    /// Change a bookmark's title and folder.
    pub fn edit(&mut self, url: &str, title: &str, folder: &str) -> bool {
        match self.items.iter_mut().find(|b| b.url == url) {
            Some(b) => {
                b.title = clean(title).trim().to_string();
                b.folder = clean(folder).trim().trim_matches('/').to_string();
                true
            }
            None => false,
        }
    }

    /// Move a bookmark up (-1) or down (1) in the list.
    pub fn move_by(&mut self, url: &str, delta: isize) -> bool {
        let Some(i) = self.items.iter().position(|b| b.url == url) else { return false };
        let j = i as isize + delta;
        if j < 0 || j as usize >= self.items.len() {
            return false;
        }
        self.items.swap(i, j as usize);
        true
    }

    /// Folder names in first-use order.
    pub fn folders(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for b in &self.items {
            if !b.folder.is_empty() && !out.contains(&b.folder.as_str()) {
                out.push(&b.folder);
            }
        }
        out
    }

    #[cfg(test)]
    pub fn search(&self, query: &str, limit: usize) -> Vec<&Bookmark> {
        let q = query.to_lowercase();
        self.items
            .iter()
            .filter(|b| b.url.to_lowercase().contains(&q) || b.title.to_lowercase().contains(&q) || b.folder.to_lowercase().contains(&q))
            .take(limit)
            .collect()
    }

    /// Export in the Netscape bookmark file format (`bookmarks.html`).
    pub fn to_netscape_html(&self) -> String {
        let mut out = String::from(
            "<!DOCTYPE NETSCAPE-Bookmark-file-1>\n<!-- This is an automatically generated file.\n     It will be read and overwritten.\n     DO NOT EDIT! -->\n<META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=UTF-8\">\n<TITLE>Bookmarks</TITLE>\n<H1>Bookmarks</H1>\n<DL><p>\n",
        );
        // Group by folder path, keeping order; nested paths become nested lists.
        let mut open: Vec<String> = Vec::new();
        let mut order: Vec<&str> = vec![""];
        order.extend(self.folders());
        for folder in order {
            let parts: Vec<String> = if folder.is_empty() { Vec::new() } else { folder.split('/').map(str::to_string).collect() };
            // Close folders that are not a prefix of this one.
            let common = open.iter().zip(&parts).take_while(|(a, b)| a == b).count();
            while open.len() > common {
                open.pop();
                out.push_str(&format!("{}</DL><p>\n", "    ".repeat(open.len() + 1)));
            }
            while open.len() < parts.len() {
                let name = &parts[open.len()];
                out.push_str(&format!("{}<DT><H3>{}</H3>\n{}<DL><p>\n", "    ".repeat(open.len() + 1), html_escape(name), "    ".repeat(open.len() + 1)));
                open.push(name.clone());
            }
            let indent = "    ".repeat(open.len() + 1);
            for b in self.items.iter().filter(|b| b.folder == folder) {
                out.push_str(&format!("{indent}<DT><A HREF=\"{}\">{}</A>\n", html_escape(&b.url), html_escape(&b.title)));
            }
        }
        while !open.is_empty() {
            open.pop();
            out.push_str(&format!("{}</DL><p>\n", "    ".repeat(open.len() + 1)));
        }
        out.push_str("</DL><p>\n");
        out
    }

    /// Parse a Netscape bookmark file (exported by any browser).
    pub fn from_netscape_html(html: &str) -> Bookmarks {
        let mut items = Vec::new();
        let mut folders: Vec<String> = Vec::new();
        // A folder name waits for its <DL> to open.
        let mut pending_folder: Option<String> = None;
        let mut rest = html;
        while let Some(lt) = rest.find('<') {
            rest = &rest[lt..];
            let Some(gt) = rest.find('>') else { break };
            let tag = &rest[1..gt];
            let lower = tag.to_ascii_lowercase();
            let after = &rest[gt + 1..];
            if lower.starts_with("h3") {
                let end = after.to_ascii_lowercase().find("</h3").unwrap_or(after.len());
                pending_folder = Some(html_unescape(after[..end].trim()));
            } else if lower == "dl" || lower.starts_with("dl ") {
                if let Some(f) = pending_folder.take() {
                    folders.push(f);
                } else {
                    folders.push(String::new()); // the root list (no folder)
                }
            } else if lower == "/dl" {
                folders.pop();
            } else if lower.starts_with("a ") {
                if let Some(href) = tag_attr(tag, "href") {
                    let end = after.to_ascii_lowercase().find("</a").unwrap_or(after.len());
                    let title = html_unescape(after[..end].trim());
                    let path: Vec<&str> = folders.iter().map(String::as_str).filter(|f| !f.is_empty()).collect();
                    let href = href.trim();
                    if !href.is_empty() && !href.starts_with("place:") && !href.starts_with("javascript:") {
                        items.push(Bookmark { url: href.to_string(), title: clean(&title), folder: clean(&path.join("/")) });
                    }
                }
            }
            rest = after;
        }
        Bookmarks { items }
    }

    /// Add the bookmarks of `other` that are not present yet; returns how
    /// many were added.
    pub fn merge(&mut self, other: Bookmarks) -> usize {
        let mut n = 0;
        for b in other.items {
            if !self.contains(&b.url) {
                self.items.push(b);
                n += 1;
            }
        }
        n
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
        assert!(b.edit("https://b/", "Bee", "Work/Docs/"));
        let back = Bookmarks::parse(&b.serialize());
        assert_eq!(back.items.len(), 2);
        assert_eq!(back.items[0].title, "A site");
        assert_eq!(back.items[1].folder, "Work/Docs");
        assert!(back.contains("https://b/"));
        let mut back = back;
        assert!(back.move_by("https://b/", -1));
        assert_eq!(back.items[0].url, "https://b/");
        assert!(!back.move_by("https://b/", -1));
        assert!(!back.toggle("https://a/", "x"));
        assert_eq!(back.items.len(), 1);
        assert_eq!(back.search("docs", 5).len(), 1);
        // Old two-column files.
        assert_eq!(Bookmarks::parse("https://x/\tX\n").items[0].folder, "");
    }

    #[test]
    fn netscape_round_trip() {
        let mut b = Bookmarks::default();
        b.items.push(Bookmark::new("https://top/", "Top & <more>"));
        b.items.push(Bookmark { url: "https://w/".into(), title: "W".into(), folder: "Work".into() });
        b.items.push(Bookmark { url: "https://d/?a=1&b=2".into(), title: "D".into(), folder: "Work/Docs".into() });
        b.items.push(Bookmark { url: "https://m/".into(), title: "M".into(), folder: "Music".into() });
        let html = b.to_netscape_html();
        assert!(html.starts_with("<!DOCTYPE NETSCAPE-Bookmark-file-1>"));
        let back = Bookmarks::from_netscape_html(&html);
        assert_eq!(back, b);
    }

    #[test]
    fn netscape_import_from_other_browsers() {
        let html = r#"<!DOCTYPE NETSCAPE-Bookmark-file-1>
<META HTTP-EQUIV="Content-Type" CONTENT="text/html; charset=UTF-8">
<TITLE>Bookmarks</TITLE>
<H1>Bookmarks</H1>
<DL><p>
    <DT><H3 ADD_DATE="1" PERSONAL_TOOLBAR_FOLDER="true">Bookmarks bar</H3>
    <DL><p>
        <DT><A HREF="https://www.rust-lang.org/" ADD_DATE="1700000000" ICON="data:image/png;base64,AAAA">Rust &#8211; lang</A>
        <DT><A href='https://example.com/a?x=1&amp;y=2'>Example</A>
    </DL><p>
    <DT><A HREF="place:sort=8">Recent</A>
    <DT><A HREF="https://loose.org/">Loose</A>
</DL><p>"#;
        let b = Bookmarks::from_netscape_html(html);
        assert_eq!(b.items.len(), 3);
        assert_eq!(b.items[0].title, "Rust \u{2013} lang");
        assert_eq!(b.items[0].folder, "Bookmarks bar");
        assert_eq!(b.items[1].url, "https://example.com/a?x=1&y=2");
        assert_eq!(b.items[2].folder, "");
        let mut mine = Bookmarks::default();
        mine.toggle("https://loose.org/", "x");
        assert_eq!(mine.merge(b), 2);
    }
}
