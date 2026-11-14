//! Read mode: the main content of a page as clean Markdown
//! (Readability-style scoring), links as refs, without navigation,
//! footers, related-content blocks or hidden text.

use std::collections::HashMap;

use rabbithole_engine::dom::{Document, NodeId};
use rabbithole_engine::style::Visibility;
use rabbithole_engine::Page;

use crate::ax::{clip_text, GeoMap};
use crate::refs::RefMap;

const POSITIVE: &[&str] = &["article", "body", "content", "entry", "hentry", "main", "page", "post", "text", "blog", "story", "prose", "markdown"];
const NEGATIVE: &[&str] = &[
    "comment", "meta", "footer", "footnote", "sidebar", "nav", "menu", "related", "share", "social", "promo", "advert", "ad-", "banner", "cookie", "newsletter",
    "subscribe", "popup", "modal", "breadcrumb", "widget", "masthead", "outbrain", "taboola", "recommend",
];

fn class_id(doc: &Document, n: NodeId) -> String {
    format!("{} {}", doc.attr(n, "class").unwrap_or(""), doc.attr(n, "id").unwrap_or("")).to_lowercase()
}

fn class_weight(doc: &Document, n: NodeId) -> f32 {
    let s = class_id(doc, n);
    if s.trim().is_empty() {
        return 0.0;
    }
    let mut w = 0.0;
    if NEGATIVE.iter().any(|k| s.contains(k)) {
        w -= 25.0;
    }
    if POSITIVE.iter().any(|k| s.contains(k)) {
        w += 25.0;
    }
    w
}

fn is_junk(doc: &Document, n: NodeId) -> bool {
    let s = class_id(doc, n);
    !s.trim().is_empty() && NEGATIVE.iter().any(|k| s.contains(k)) && !POSITIVE.iter().any(|k| s.contains(k))
}

fn visible(page: &Page, geo: &GeoMap, n: NodeId) -> bool {
    let doc = &page.doc;
    if doc.attr(n, "aria-hidden") == Some("true") {
        return false;
    }
    let Some(st) = page.styles.get(n) else { return false };
    if st.display == rabbithole_engine::style::Display::None {
        return false;
    }
    if st.visibility == Visibility::Hidden || st.opacity < 0.05 {
        return false;
    }
    match geo.get(n) {
        Some(g) => !((g.raw.w < 1.5 && g.raw.h < 1.5) || g.rect.right() <= 0.0 || g.rect.bottom() <= 0.0),
        None => true,
    }
}

fn text_len(doc: &Document, n: NodeId) -> usize {
    doc.text_content(n).split_whitespace().map(|w| w.len() + 1).sum()
}

fn link_len(doc: &Document, n: NodeId) -> usize {
    doc.descendants(n).filter(|&d| doc.is_element(d, "a")).map(|a| text_len(doc, a)).sum()
}

/// The element holding the main content.
pub fn main_content(page: &Page) -> Option<NodeId> {
    let doc = &page.doc;
    let all: Vec<NodeId> = doc.descendants(Document::ROOT).filter(|&n| doc.element(n).is_some() && crate::ax::displayed(page, n)).collect();
    // Explicit markers first.
    let explicit: Vec<NodeId> = all.iter().copied().filter(|&n| doc.is_element(n, "main") || doc.attr(n, "role") == Some("main")).collect();
    if let Some(&m) = explicit.iter().max_by_key(|&&n| text_len(doc, n)) {
        if text_len(doc, m) > 200 {
            // A single big <article> inside main is even better.
            let arts: Vec<NodeId> = doc.descendants(m).filter(|&d| doc.is_element(d, "article")).collect();
            if arts.len() == 1 && text_len(doc, arts[0]) * 2 > text_len(doc, m) {
                return Some(arts[0]);
            }
            return Some(m);
        }
    }
    let arts: Vec<NodeId> = all.iter().copied().filter(|&n| doc.is_element(n, "article")).collect();
    if arts.len() == 1 && text_len(doc, arts[0]) > 500 {
        return Some(arts[0]);
    }
    // Readability scoring.
    let mut scores: HashMap<NodeId, f32> = HashMap::new();
    for &n in &all {
        let tag = doc.tag(n).unwrap_or("");
        if !matches!(tag, "p" | "pre" | "td" | "blockquote" | "li" | "dd") && !(tag == "div" && doc.child_elements(n).next().is_none()) {
            continue;
        }
        let text = doc.text_content(n);
        let len = text.split_whitespace().map(|w| w.len() + 1).sum::<usize>();
        if len < 25 {
            continue;
        }
        let score = 1.0 + text.matches(',').count() as f32 + (len as f32 / 100.0).min(3.0);
        let mut cur = doc.parent(n);
        let mut div = 1.0;
        for _ in 0..3 {
            let Some(p) = cur else { break };
            if doc.element(p).is_none() {
                break;
            }
            let base = scores.entry(p).or_insert_with(|| {
                let t = doc.tag(p).unwrap_or("");
                let b = match t {
                    "div" | "article" | "section" | "main" => 5.0,
                    "pre" | "td" | "blockquote" => 3.0,
                    "ol" | "ul" | "dl" | "form" | "li" => -3.0,
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "th" => -5.0,
                    _ => 0.0,
                };
                b + class_weight(doc, p)
            });
            *base += score / div;
            div = if div == 1.0 { 2.0 } else { div * 1.5 };
            cur = doc.parent(p);
        }
    }
    let mut best: Option<(NodeId, f32)> = None;
    for (&n, &s) in &scores {
        let tl = text_len(doc, n).max(1);
        let ld = link_len(doc, n) as f32 / tl as f32;
        let fs = s * (1.0 - ld);
        if best.map(|(_, b)| fs > b).unwrap_or(true) {
            best = Some((n, fs));
        }
    }
    let (mut n, s) = best?;
    // Climb while the parent holds clearly more of the same content.
    while let Some(p) = doc.parent(n) {
        if doc.element(p).is_none() || doc.is_element(p, "body") {
            break;
        }
        let ps = scores.get(&p).copied().unwrap_or(0.0);
        if ps >= s * 0.8 && text_len(doc, p) < text_len(doc, n) * 2 {
            n = p;
        } else {
            break;
        }
    }
    Some(n)
}

struct Md<'a> {
    page: &'a Page,
    geo: &'a GeoMap,
    refs: &'a mut RefMap,
    frame: usize,
    links: bool,
    cites: bool,
    out: String,
}

fn esc_md(s: &str) -> String {
    s.replace(['⟦', '⟧'], "|")
}

// Normalize prose only: code spans are opaque to whitespace/punctuation cleanup.
fn clean_inline(input: &str) -> String {
    let mut out = String::new();
    let mut pending = false;
    let mut pos = 0;
    while pos < input.len() {
        let rest = &input[pos..];
        if rest.starts_with('`') {
            let count = rest.bytes().take_while(|&b| b == b'`').count();
            let delimiter = "`".repeat(count);
            if let Some(end) = rest[count..].find(&delimiter) {
                if pending && !out.is_empty() { out.push(' '); }
                let length = count + end + count;
                out.push_str(&rest[..length]);
                pos += length;
                pending = false;
                continue;
            }
            // Consume an unmatched run together (a long literal run must not
            // rescan its remaining suffix once per backtick).
            if pending && !out.is_empty() { out.push(' '); }
            out.push_str(&delimiter);
            pos += count;
            pending = false;
            continue;
        }
        let ch = rest.chars().next().unwrap();
        pos += ch.len_utf8();
        if ch.is_whitespace() { pending = !out.is_empty(); }
        else {
            if pending && !matches!(ch, ',' | '.' | ';' | ':' | ')') { out.push(' '); }
            out.push(ch);
            pending = false;
        }
    }
    out
}

fn tick_run(input: &str) -> usize {
    input.split(|ch| ch != '`').map(str::len).max().unwrap_or(0)
}

fn inline_code(input: &str) -> String {
    let input = esc_md(input);
    if input.is_empty() { return String::new(); }
    let delimiter = "`".repeat(tick_run(&input) + 1);
    let padded_spaces = input.starts_with(' ') && input.ends_with(' ') && input.chars().any(|ch| ch != ' ');
    let padding = if input.starts_with('`') || input.ends_with('`') || padded_spaces { " " } else { "" };
    format!("{delimiter}{padding}{input}{padding}{delimiter}")
}

// Keep structural indentation, fences and every byte of preformatted lines.
fn clean_blocks(input: &str) -> String {
    let mut lines = Vec::new();
    let mut fence = None;
    let mut code_span = None;
    let mut span_lines = Vec::new();
    let mut span_indent = 0;
    let mut blank = false;
    for line in input.lines() {
        let mut marker = line.trim_start();
        while let Some(rest) = marker.strip_prefix('>') { marker = rest.trim_start(); }
        let ticks = marker.bytes().take_while(|&b| b == b'`').count();
        let boundary = ticks >= 3 && marker.len() == ticks;
        if let Some(length) = fence {
            lines.push(line.to_string());
            if boundary && ticks == length { fence = None; }
            continue;
        }
        // Inline code may cross a source line. Keep such lines opaque too;
        // collapsing per-line whitespace would change code before its closer.
        let was_span = code_span.is_some();
        if !boundary || was_span {
            let mut scan = line;
            while let Some(at) = scan.find('`') {
                scan = &scan[at..];
                let run = scan.bytes().take_while(|&b| b == b'`').count();
                if code_span == Some(run) { code_span = None; }
                else if code_span.is_none() { code_span = Some(run); }
                scan = &scan[run..];
            }
        }
        if was_span || code_span.is_some() {
            if !was_span { span_indent = line.len() - line.trim_start_matches(' ').len(); }
            span_lines.push(line.to_string());
            if code_span.is_none() {
                let joined = span_lines.join("\n");
                lines.push(format!("{}{}", &joined[..span_indent], clean_inline(&joined[span_indent..])));
                span_lines.clear();
            }
            blank = false;
        } else if boundary {
            fence = Some(ticks);
            blank = false;
            lines.push(line.to_string());
        } else if line.trim().is_empty() {
            if !blank { lines.push(String::new()); }
            blank = true;
        } else {
            let indent = line.len() - line.trim_start_matches(' ').len();
            lines.push(format!("{}{}", &line[..indent], clean_inline(&line[indent..])));
            blank = false;
        }
    }
    if !span_lines.is_empty() {
        // An unmatched literal backtick is prose, not a code delimiter.
        for line in span_lines {
            let indent = line.len() - line.trim_start_matches(' ').len();
            lines.push(format!("{}{}", &line[..indent], clean_inline(&line[indent..])));
        }
    }
    lines.join("\n").trim().to_string()
}

impl Md<'_> {
    fn block_sep(&mut self) {
        let t = self.out.trim_end_matches(' ');
        let len = t.len();
        self.out.truncate(len);
        if self.out.is_empty() || self.out.ends_with("\n\n") {
            return;
        }
        if self.out.ends_with('\n') {
            self.out.push('\n');
        } else {
            self.out.push_str("\n\n");
        }
    }

    fn inline(&mut self, n: NodeId) -> String {
        let mut s = String::new();
        self.inline_into(n, &mut s);
        clean_inline(&s)
    }

    fn inline_into(&mut self, n: NodeId, s: &mut String) {
        let doc = &self.page.doc;
        for c in doc.flat_children(n) {
            if let Some(t) = doc.text(c) {
                s.push_str(&esc_md(t));
                continue;
            }
            let Some(el) = doc.element(c) else { continue };
            if !visible(self.page, self.geo, c) || matches!(el.tag.as_str(), "script" | "style" | "noscript" | "template" | "button" | "svg") {
                continue;
            }
            match el.tag.as_str() {
                "a" if el.attr("href").is_some() => {
                    if self.omit_citation(c) { continue; }
                    if self.links {
                        let text = self.inline(c);
                        if text.is_empty() { continue; }
                        let r = self.refs.assign(self.frame, c, "link", &text);
                        s.push_str(&format!(" [{text}](@e{r}) "));
                    } else {
                        // Keep the source edges until the containing prose is
                        // normalized, including whitespace-only anchors.
                        self.inline_into(c, s);
                    }
                }
                "strong" | "b" => self.marked_inline(c, "**", s),
                "em" | "i" => self.marked_inline(c, "*", s),
                "code" | "kbd" | "samp" => {
                    let t = doc.text_content(c);
                    if self.links { s.push(' '); }
                    s.push_str(&inline_code(&t));
                    if self.links { s.push(' '); }
                }
                "br" => s.push('\n'),
                "img" => {
                    if let Some(alt) = el.attr("alt").filter(|a| !a.trim().is_empty()) {
                        s.push_str(&format!(" ![{}] ", esc_md(alt.trim())));
                    }
                }
                "sup" | "sub" => {
                    if self.links { s.push_str(&self.inline(c)); }
                    else { self.inline_into(c, s); }
                }
                _ => {
                    if self.links { s.push(' '); }
                    self.inline_into(c, s);
                    if self.links { s.push(' '); }
                }
            }
        }
    }

    fn marked_inline(&mut self, node: NodeId, marker: &str, out: &mut String) {
        let mut raw = String::new();
        self.inline_into(node, &mut raw);
        let text = clean_inline(&raw);
        if text.is_empty() { if !self.links { out.push_str(&raw); } return; }
        if self.links || raw.chars().next().is_some_and(char::is_whitespace) { out.push(' '); }
        out.push_str(marker); out.push_str(&text); out.push_str(marker);
        if self.links || raw.chars().next_back().is_some_and(char::is_whitespace) { out.push(' '); }
    }

    fn omit_citation(&self, n: NodeId) -> bool {
        if self.cites { return false; }
        let doc = &self.page.doc;
        let mut parent = doc.parent(n);
        let mut in_sup = false;
        while let Some(node) = parent {
            if doc.is_element(node, "sup") { in_sup = true; break; }
            parent = doc.parent(node);
        }
        in_sup && doc.attr(n, "href").and_then(|href| self.page.resolve(href)).is_some_and(|url|
            url.fragment.is_some() && url.without_fragment() == self.page.url.without_fragment())
    }

    fn has_block_child(&self, n: NodeId) -> bool {
        let doc = &self.page.doc;
        doc.child_elements(n).any(|c| {
            matches!(
                doc.tag(c),
                Some("p" | "div" | "section" | "article" | "ul" | "ol" | "table" | "pre" | "blockquote" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "figure" | "header" | "dl" | "hr" | "main" | "aside" | "nav" | "footer" | "form" | "details")
            )
        })
    }

    fn block(&mut self, n: NodeId, list_depth: usize) {
        let doc = &self.page.doc;
        for c in doc.flat_children(n) {
            if let Some(t) = doc.text(c) {
                if !self.links { self.out.push_str(&esc_md(t)); continue; }
                let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
                if !t.is_empty() {
                    self.out.push_str(&esc_md(&t));
                    self.out.push(' ');
                }
                continue;
            }
            let Some(el) = doc.element(c) else { continue };
            let tag = el.tag.clone();
            if matches!(tag.as_str(), "script" | "style" | "noscript" | "template" | "nav" | "aside" | "footer" | "form" | "button" | "select" | "input" | "textarea" | "iframe" | "dialog" | "svg" | "menu") {
                continue;
            }
            if !visible(self.page, self.geo, c) || is_junk(doc, c) || matches!(el.attr("role"), Some("navigation" | "complementary" | "contentinfo" | "dialog" | "alertdialog")) {
                continue;
            }
            match tag.as_str() {
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    let level: usize = tag[1..].parse().unwrap_or(2);
                    let t = self.inline(c);
                    if !t.is_empty() {
                        self.block_sep();
                        self.out.push_str(&format!("{} {}", "#".repeat(level), t));
                        self.block_sep();
                    }
                }
                "p" => {
                    let t = self.inline(c);
                    if !t.is_empty() {
                        self.block_sep();
                        self.out.push_str(&t);
                        self.block_sep();
                    }
                }
                "ul" | "ol" | "dl" => {
                    self.block_sep();
                    let ordered = tag == "ol";
                    let mut i = 0;
                    for li in doc.child_elements(c).collect::<Vec<_>>() {
                        if !visible(self.page, self.geo, li) {
                            continue;
                        }
                        i += 1;
                        let marker = if ordered { format!("{i}.") } else { "-".to_string() };
                        let indent = "  ".repeat(list_depth);
                        if self.has_block_child(li) {
                            // Inline part first, nested lists after.
                            let mut head = String::new();
                            for cc in doc.flat_children(li) {
                                if matches!(doc.tag(cc), Some("ul" | "ol")) {
                                    continue;
                                }
                                if let Some(t) = doc.text(cc) {
                                    head.push_str(&esc_md(t));
                                } else if doc.element(cc).is_some() && visible(self.page, self.geo, cc) {
                                    if self.links { head.push(' '); }
                                    self.inline_one(cc, &mut head);
                                    if self.links { head.push(' '); }
                                }
                            }
                            let head = clean_inline(&head);
                            self.out.push_str(&format!("{indent}{marker} {head}\n"));
                            for cc in doc.child_elements(li).collect::<Vec<_>>() {
                                if matches!(doc.tag(cc), Some("ul" | "ol")) {
                                    let save = std::mem::take(&mut self.out);
                                    self.block_list(cc, list_depth + 1);
                                    let nested = std::mem::replace(&mut self.out, save);
                                    self.out.push_str(&nested);
                                }
                            }
                        } else {
                            let t = self.inline(li);
                            if !t.is_empty() {
                                self.out.push_str(&format!("{indent}{marker} {t}\n"));
                            }
                        }
                    }
                    self.block_sep();
                }
                "pre" => {
                    self.block_sep();
                    let t = doc.text_content(c);
                    let delimiter = "`".repeat((tick_run(&t) + 1).max(3));
                    self.out.push_str(&delimiter);
                    self.out.push('\n');
                    self.out.push_str(&esc_md(&t));
                    if !t.ends_with('\n') { self.out.push('\n'); }
                    self.out.push_str(&delimiter);
                    self.block_sep();
                }
                "blockquote" => {
                    self.block_sep();
                    let save = std::mem::take(&mut self.out);
                    self.block(c, list_depth);
                    let inner = std::mem::replace(&mut self.out, save);
                    for l in inner.trim().lines() {
                        self.out.push_str("> ");
                        self.out.push_str(l);
                        self.out.push('\n');
                    }
                    self.block_sep();
                }
                "table" => {
                    self.block_sep();
                    let rows: Vec<NodeId> = doc.descendants(c).filter(|&r| doc.is_element(r, "tr")).take(60).collect();
                    for (i, r) in rows.iter().enumerate() {
                        let cells: Vec<String> = doc
                            .child_elements(*r)
                            .filter(|&x| matches!(doc.tag(x), Some("td" | "th")))
                            .map(|x| clip_text(&self.inline(x).replace('|', "/"), 240))
                            .collect();
                        if cells.is_empty() {
                            continue;
                        }
                        self.out.push_str(&format!("| {} |\n", cells.join(" | ")));
                        if i == 0 {
                            self.out.push_str(&format!("|{}\n", " --- |".repeat(cells.len())));
                        }
                    }
                    self.block_sep();
                }
                "hr" => {
                    self.block_sep();
                    self.out.push_str("---");
                    self.block_sep();
                }
                "img" => {
                    if let Some(alt) = el.attr("alt").filter(|a| !a.trim().is_empty()) {
                        self.block_sep();
                        self.out.push_str(&format!("![{}]", esc_md(alt.trim())));
                        self.block_sep();
                    }
                }
                "figcaption" => {
                    let t = self.inline(c);
                    if !t.is_empty() {
                        self.block_sep();
                        self.out.push_str(&format!("*{t}*"));
                        self.block_sep();
                    }
                }
                "a" | "strong" | "b" | "em" | "i" | "code" | "span" | "small" | "time" | "abbr" | "mark" | "label" | "cite" | "q" | "sup" | "sub" | "br" => {
                    let mut s = String::new();
                    // Reuse the inline converter on a one-element slice.
                    let wrapper = c;
                    let tmp = {
                        let mut t = String::new();
                        self.inline_one(wrapper, &mut t);
                        t
                    };
                    s.push_str(&tmp);
                    if self.links { self.out.push_str(s.trim()); self.out.push(' '); }
                    else { self.out.push_str(&s); }
                }
                _ => {
                    // Generic containers: recurse, separating blocks.
                    let block_like = self.page.styles.get(c).map(|st| !matches!(st.display, rabbithole_engine::style::Display::Inline)).unwrap_or(true);
                    if block_like {
                        self.block_sep();
                    }
                    self.block(c, list_depth);
                    if block_like {
                        self.block_sep();
                    }
                }
            }
        }
    }

    fn block_list(&mut self, list: NodeId, depth: usize) {
        let doc = &self.page.doc;
        let ordered = doc.is_element(list, "ol");
        let mut i = 0;
        for li in doc.child_elements(list).collect::<Vec<_>>() {
            if !visible(self.page, self.geo, li) {
                continue;
            }
            i += 1;
            let marker = if ordered { format!("{i}.") } else { "-".to_string() };
            let t = self.inline(li);
            if !t.is_empty() {
                self.out.push_str(&format!("{}{marker} {t}\n", "  ".repeat(depth)));
            }
        }
    }

    /// Inline conversion of a single element (including itself).
    fn inline_one(&mut self, n: NodeId, s: &mut String) {
        let doc = &self.page.doc;
        let Some(parent) = doc.parent(n) else { return };
        // Convert just this child: temporarily walk parent's children but
        // only emit `n`.
        let _ = parent;
        let Some(el) = doc.element(n) else { return };
        match el.tag.as_str() {
            "a" if el.attr("href").is_some() => {
                if self.omit_citation(n) { return; }
                if self.links {
                    let text = self.inline(n);
                    if !text.is_empty() {
                        let r = self.refs.assign(self.frame, n, "link", &text);
                        s.push_str(&format!("[{text}](@e{r})"));
                    }
                } else { self.inline_into(n, s); }
            }
            "strong" | "b" => self.marked_inline(n, "**", s),
            "em" | "i" => self.marked_inline(n, "*", s),
            "code" => s.push_str(&inline_code(&doc.text_content(n))),
            "br" => s.push('\n'),
            _ => if self.links { s.push_str(&self.inline(n)); } else { self.inline_into(n, s); },
        }
    }
}

/// Byline / date metadata.
fn metadata(page: &Page) -> Vec<String> {
    let doc = &page.doc;
    let mut out = Vec::new();
    let meta = |name: &str| {
        doc.descendants(Document::ROOT)
            .find(|&n| doc.is_element(n, "meta") && (doc.attr(n, "name") == Some(name) || doc.attr(n, "property") == Some(name)))
            .and_then(|n| doc.attr(n, "content"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    if let Some(a) = meta("author").or_else(|| meta("article:author").filter(|a| !a.starts_with("http"))) {
        out.push(format!("By {}", clip_text(&a, 80)));
    }
    let date = meta("article:published_time").or_else(|| meta("date")).or_else(|| {
        doc.descendants(Document::ROOT).find(|&n| doc.is_element(n, "time")).and_then(|n| doc.attr(n, "datetime").map(|s| s.to_string()))
    });
    if let Some(d) = date {
        out.push(clip_text(&d, 40));
    }
    out
}

/// One element's content as Markdown.
pub fn read_subtree(page: &Page, refs: &mut RefMap, frame: usize, root: NodeId) -> String {
    read_subtree_with_options(page, refs, frame, root, true, false)
}

pub fn read_subtree_with_options(page: &Page, refs: &mut RefMap, frame: usize, root: NodeId, links: bool, cites: bool) -> String {
    let geo = GeoMap::build(page);
    let mut md = Md { page, geo: &geo, refs, frame, links, cites, out: String::new() };
    if page.doc.is_element(root, "a") && md.omit_citation(root) { return String::new(); }
    md.block(root, 0);
    clean_blocks(&md.out)
}

/// Main content as Markdown.
pub fn read(page: &Page, refs: &mut RefMap, frame: usize) -> String {
    read_with_options(page, refs, frame, true, false)
}

pub fn read_with_options(page: &Page, refs: &mut RefMap, frame: usize, links: bool, cites: bool) -> String {
    let geo = GeoMap::build(page);
    let doc = &page.doc;
    let root = main_content(page).or_else(|| doc.body()).unwrap_or(Document::ROOT);
    let mut md = Md { page, geo: &geo, refs, frame, links, cites, out: String::new() };
    md.block(root, 0);
    let body = clean_blocks(&md.out);
    let mut out = String::new();
    let title = page.title.trim();
    if !body.starts_with("# ") && !title.is_empty() {
        out.push_str(&format!("# {}\n\n", clean_inline(&esc_md(title))));
    }
    let meta = metadata(page);
    if !meta.is_empty() {
        out.push_str(&format!("_{}_\n\n", clean_inline(&esc_md(&meta.join(" · ")))));
    }
    out.push_str(&body);
    out
}

#[cfg(test)]
mod tests {
    use super::{clean_blocks,clean_inline,inline_code};
    #[test]
    fn prose_cleanup_preserves_markdown_links_and_code_contents(){
        assert_eq!(clean_inline("  A\u{a0}\u{a0} [link](@e12) , **bold** ; `x  , y` . ( tail )  "),"A [link](@e12), **bold**; `x  , y`. ( tail)");
        assert_eq!(clean_inline(&inline_code("`a  b`")),"`` `a  b` ``");
        assert_eq!(inline_code(" a "),"`  a  `");
        assert_eq!(inline_code("   "),"`   `");
        assert_eq!(inline_code(""),"");
        assert_eq!(clean_blocks("- one  , two\n  - nested   item\n\n\nend"),"- one, two\n  - nested item\n\nend");
        assert_eq!(clean_blocks("before `a  \n\n b  ` , after ."),"before `a  \n\n b  `, after.");
    }
}
