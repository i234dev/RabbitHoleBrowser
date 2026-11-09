//! HTML parsing: tokenizer + a forgiving tree builder.
//!
//! The tree builder implements the parts of the WHATWG algorithm that matter
//! for rendering real pages: implied `html`/`head`/`body`, void elements,
//! implied end tags (`p`, `li`, `dt`/`dd`, `option`, table parts), scope
//! checks for end tags, raw-text elements and foreign (SVG/MathML) content.

pub mod entities;
pub mod tokenizer;

use rabbithole_dom::{Document, NodeData, NodeId};
use tokenizer::{RawKind, Token, Tokenizer};

const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr", "keygen", "command", "basefont", "bgsound", "frame",
];

/// Start tags that implicitly close an open `<p>`.
const CLOSES_P: &[&str] = &[
    "address", "article", "aside", "blockquote", "center", "details", "dialog", "dir", "div", "dl",
    "fieldset", "figcaption", "figure", "footer", "form", "h1", "h2", "h3", "h4", "h5", "h6",
    "header", "hgroup", "hr", "main", "menu", "nav", "ol", "p", "pre", "listing", "section",
    "summary", "table", "ul", "xmp", "search",
];

const HEADINGS: &[&str] = &["h1", "h2", "h3", "h4", "h5", "h6"];

/// Elements that stop the search when looking for an element "in scope".
const SCOPE_BOUNDARY: &[&str] = &[
    "html", "table", "td", "th", "caption", "applet", "marquee", "object", "template", "svg", "math",
];

const HEAD_ONLY: &[&str] = &["base", "basefont", "bgsound", "link", "meta", "title", "style", "script", "noframes", "template"];

pub fn parse(input: &str) -> Document {
    let mut builder = TreeBuilder::new();
    builder.run(input);
    builder.doc
}

struct TreeBuilder {
    doc: Document,
    stack: Vec<NodeId>,
    html: Option<NodeId>,
    head: Option<NodeId>,
    body: Option<NodeId>,
    /// Depth of foreign (svg/math) elements currently open.
    foreign_depth: usize,
    /// Pending whitespace-only text seen before body creation.
    saw_body_content: bool,
}

impl TreeBuilder {
    fn new() -> Self {
        TreeBuilder {
            doc: Document::new(),
            stack: Vec::new(),
            html: None,
            head: None,
            body: None,
            foreign_depth: 0,
            saw_body_content: false,
        }
    }

    fn current(&self) -> NodeId {
        *self.stack.last().unwrap_or(&Document::ROOT)
    }

    fn current_tag(&self) -> &str {
        self.doc.tag(self.current()).unwrap_or("")
    }

    fn ensure_html(&mut self) -> NodeId {
        if let Some(h) = self.html {
            return h;
        }
        let h = self.doc.create_element("html", vec![]);
        self.doc.append_child(Document::ROOT, h);
        self.stack.push(h);
        self.html = Some(h);
        h
    }

    fn ensure_head(&mut self) -> NodeId {
        if let Some(h) = self.head {
            return h;
        }
        let html = self.ensure_html();
        let h = self.doc.create_element("head", vec![]);
        self.doc.append_child(html, h);
        self.head = Some(h);
        h
    }

    fn ensure_body(&mut self) -> NodeId {
        if let Some(b) = self.body {
            return b;
        }
        self.ensure_head();
        let html = self.ensure_html();
        // Pop anything (head) off the stack down to html.
        while self.stack.len() > 1 {
            self.stack.pop();
        }
        let b = self.doc.create_element("body", vec![]);
        self.doc.append_child(html, b);
        self.stack.push(b);
        self.body = Some(b);
        self.saw_body_content = true;
        b
    }

    fn run(&mut self, input: &str) {
        let mut tk = Tokenizer::new(input);
        loop {
            let tok = tk.next_token();
            match tok {
                Token::Eof => break,
                Token::Doctype(_) => {}
                Token::Comment(c) => {
                    let n = self.doc.create(NodeData::Comment(c));
                    let parent = self.current();
                    self.doc.append_child(parent, n);
                }
                Token::Text(t) => self.text(t),
                Token::StartTag {
                    name,
                    attrs,
                    self_closing,
                } => {
                    let raw = self.start_tag(&name, attrs, self_closing);
                    if let Some(kind) = raw {
                        tk.set_raw(kind, &name);
                    }
                }
                Token::EndTag { name } => self.end_tag(&name),
            }
        }
        // Make sure even an empty document has html/head/body.
        self.ensure_body();
    }

    fn text(&mut self, t: String) {
        if self.body.is_none() {
            // Whitespace before body is dropped; anything else opens the body.
            if t.trim().is_empty() {
                // ...unless we're inside <head> raw elements like <title>
                let cur = self.current_tag().to_string();
                if HEAD_ONLY.contains(&cur.as_str()) {
                    self.append_text(t);
                }
                return;
            }
            let cur = self.current_tag().to_string();
            if !HEAD_ONLY.contains(&cur.as_str()) {
                self.ensure_body();
            }
        }
        self.append_text(t);
    }

    fn append_text(&mut self, t: String) {
        let parent = self.current();
        // Merge with previous text node when possible.
        if let Some(last) = self.doc.node(parent).last_child {
            if let NodeData::Text(existing) = &mut self.doc.nodes[last].data {
                existing.push_str(&t);
                return;
            }
        }
        let n = self.doc.create_text(&t);
        self.doc.append_child(parent, n);
    }

    fn insert_element(&mut self, name: &str, attrs: Vec<(String, String)>, push: bool) -> NodeId {
        let parent = self.current();
        let n = self.doc.create_element(name, attrs);
        self.doc.append_child(parent, n);
        if push {
            self.stack.push(n);
        }
        n
    }

    /// Is an element with `tag` on the stack above the nearest scope boundary?
    fn in_scope(&self, tag: &str, extra_boundaries: &[&str]) -> Option<usize> {
        for (i, &n) in self.stack.iter().enumerate().rev() {
            let t = self.doc.tag(n).unwrap_or("");
            if t == tag {
                return Some(i);
            }
            if SCOPE_BOUNDARY.contains(&t) || extra_boundaries.contains(&t) {
                return None;
            }
        }
        None
    }

    fn pop_until_including(&mut self, index: usize) {
        self.stack.truncate(index);
    }

    fn close_if_in_scope(&mut self, tag: &str, extra: &[&str]) {
        if let Some(i) = self.in_scope(tag, extra) {
            self.pop_until_including(i);
        }
    }

    fn merge_attrs(&mut self, node: NodeId, attrs: Vec<(String, String)>) {
        if let Some(el) = self.doc.element_mut(node) {
            for (k, v) in attrs {
                if !el.attrs.iter().any(|(n, _)| *n == k) {
                    el.attrs.push((k, v));
                }
            }
        }
    }

    fn start_tag(&mut self, name: &str, attrs: Vec<(String, String)>, self_closing: bool) -> Option<RawKind> {
        // Foreign content: keep nesting faithfully, honor self-closing.
        if self.foreign_depth > 0 {
            if self_closing {
                self.insert_element(name, attrs, false);
            } else {
                self.insert_element(name, attrs, true);
                if name == "svg" || name == "math" {
                    self.foreign_depth += 1;
                }
                // Any non-svg element inside svg counts as nested for depth purposes.
                if name != "svg" && name != "math" {
                    self.foreign_depth += 1;
                }
            }
            return None;
        }

        match name {
            "html" => {
                let h = self.ensure_html();
                self.merge_attrs(h, attrs);
                return None;
            }
            "head" => {
                if self.body.is_none() {
                    let h = self.ensure_head();
                    self.merge_attrs(h, attrs);
                    if !self.stack.contains(&h) {
                        self.stack.push(h);
                    }
                }
                return None;
            }
            "body" => {
                let b = self.ensure_body();
                self.merge_attrs(b, attrs);
                return None;
            }
            "frameset" => {
                self.ensure_body();
                return None;
            }
            _ => {}
        }

        if HEAD_ONLY.contains(&name) && self.body.is_none() {
            // Head content before body.
            let head = self.ensure_head();
            let on_stack = self.stack.contains(&head);
            if !on_stack {
                self.stack.push(head);
            }
            let is_void = VOID.contains(&name);
            self.insert_element(name, attrs, !is_void);
            let raw = raw_kind(name);
            if raw.is_none() && is_void {
                // nothing to pop
            }
            return raw;
        }

        // Everything else lives in the body.
        self.ensure_body();

        // Implied end tags.
        if CLOSES_P.contains(&name) {
            self.close_if_in_scope("p", &["button"]);
        }
        match name {
            "li" => self.close_list_item(&["li"], &["ul", "ol", "menu"]),
            "dt" | "dd" => self.close_list_item(&["dt", "dd"], &["dl"]),
            "option" => {
                if self.current_tag() == "option" {
                    self.stack.pop();
                }
            }
            "optgroup" => {
                if self.current_tag() == "option" {
                    self.stack.pop();
                }
                if self.current_tag() == "optgroup" {
                    self.stack.pop();
                }
            }
            "tr" => {
                self.close_table_part(&["tr", "td", "th"], &["table", "thead", "tbody", "tfoot"]);
            }
            "td" | "th" => {
                self.close_table_part(&["td", "th"], &["table", "tr"]);
            }
            "thead" | "tbody" | "tfoot" => {
                self.close_table_part(&["thead", "tbody", "tfoot", "tr", "td", "th", "caption", "colgroup"], &["table"]);
            }
            "caption" | "colgroup" => {
                self.close_table_part(&["thead", "tbody", "tfoot", "tr", "td", "th", "caption", "colgroup"], &["table"]);
            }
            "a" => {
                // Simplified adoption agency: an open <a> is closed first.
                if let Some(i) = self.stack.iter().rposition(|&n| self.doc.is_element(n, "a")) {
                    // only within the current block context
                    let blocked = self.stack[i + 1..]
                        .iter()
                        .any(|&n| SCOPE_BOUNDARY.contains(&self.doc.tag(n).unwrap_or("")));
                    if !blocked {
                        self.pop_until_including(i);
                    }
                }
            }
            "button" => self.close_if_in_scope("button", &[]),
            "form" => {}
            _ if HEADINGS.contains(&name) => {
                if HEADINGS.contains(&self.current_tag()) {
                    self.stack.pop();
                }
            }
            _ => {}
        }

        if name == "image" {
            return self.start_tag("img", attrs, self_closing);
        }

        let is_void = VOID.contains(&name);
        self.insert_element(name, attrs, !is_void);

        if name == "svg" || name == "math" {
            if self_closing {
                self.stack.pop();
            } else {
                self.foreign_depth = 1;
            }
            return None;
        }

        raw_kind(name)
    }

    fn close_list_item(&mut self, items: &[&str], boundaries: &[&str]) {
        for i in (0..self.stack.len()).rev() {
            let t = self.doc.tag(self.stack[i]).unwrap_or("").to_string();
            if items.contains(&t.as_str()) {
                self.pop_until_including(i);
                return;
            }
            if boundaries.contains(&t.as_str()) || SCOPE_BOUNDARY.contains(&t.as_str()) {
                return;
            }
            // Special (block) elements other than address/div/p stop the search.
            if is_special_block(&t) && !matches!(t.as_str(), "address" | "div" | "p") {
                return;
            }
        }
    }

    fn close_table_part(&mut self, items: &[&str], boundaries: &[&str]) {
        for i in (0..self.stack.len()).rev() {
            let t = self.doc.tag(self.stack[i]).unwrap_or("").to_string();
            if items.contains(&t.as_str()) {
                self.pop_until_including(i);
                return;
            }
            if boundaries.contains(&t.as_str()) || t == "html" {
                return;
            }
        }
    }

    fn end_tag(&mut self, name: &str) {
        if self.foreign_depth > 0 {
            if let Some(i) = self.stack.iter().rposition(|&n| self.doc.is_element(n, name)) {
                let popped = self.stack.len() - i;
                self.pop_until_including(i);
                self.foreign_depth = self.foreign_depth.saturating_sub(popped);
                // Leaving the outermost svg/math resets foreign mode.
                if !self.stack.iter().any(|&n| self.doc.is_element(n, "svg") || self.doc.is_element(n, "math")) {
                    self.foreign_depth = 0;
                }
            }
            return;
        }
        match name {
            "html" | "body" => {
                // Ignore: trailing content still goes into body.
                self.ensure_body();
            }
            "head" => {
                if let Some(h) = self.head {
                    if let Some(i) = self.stack.iter().rposition(|&n| n == h) {
                        self.pop_until_including(i);
                    }
                }
            }
            "br" => {
                // </br> is treated as <br>
                self.start_tag("br", vec![], false);
            }
            "p" => {
                if self.in_scope("p", &["button"]).is_some() {
                    self.close_if_in_scope("p", &["button"]);
                } else {
                    // Spec: insert an empty <p>. Harmless to ignore.
                }
            }
            _ if HEADINGS.contains(&name) => {
                // Any heading end tag closes the innermost open heading.
                for i in (0..self.stack.len()).rev() {
                    let t = self.doc.tag(self.stack[i]).unwrap_or("");
                    if HEADINGS.contains(&t) {
                        self.pop_until_including(i);
                        return;
                    }
                    if SCOPE_BOUNDARY.contains(&t) {
                        return;
                    }
                }
            }
            _ => {
                // Generic: find matching element within scope and pop to it.
                let extra: &[&str] = if matches!(name, "li" | "dt" | "dd") { &["ul", "ol", "dl"] } else { &[] };
                let boundary_free = matches!(name, "td" | "th" | "tr" | "table" | "caption" | "tbody" | "thead" | "tfoot" | "svg" | "math" | "object" | "template" | "marquee" | "applet");
                let found = if boundary_free {
                    self.stack.iter().rposition(|&n| self.doc.is_element(n, name))
                } else {
                    self.in_scope(name, extra)
                };
                if let Some(i) = found {
                    self.pop_until_including(i);
                }
            }
        }
    }
}

fn raw_kind(name: &str) -> Option<RawKind> {
    match name {
        "script" | "style" | "xmp" | "iframe" | "noembed" | "noframes" => Some(RawKind::RawText),
        "textarea" | "title" => Some(RawKind::RcData),
        "plaintext" => Some(RawKind::PlainText),
        _ => None,
    }
}

fn is_special_block(tag: &str) -> bool {
    matches!(
        tag,
        "address" | "article" | "aside" | "blockquote" | "body" | "center" | "details" | "dialog" | "dir" | "div" | "dl"
            | "fieldset" | "figcaption" | "figure" | "footer" | "form" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "header"
            | "hgroup" | "main" | "menu" | "nav" | "ol" | "p" | "pre" | "section" | "summary" | "table" | "ul" | "li"
            | "dd" | "dt" | "td" | "th" | "tr" | "tbody" | "thead" | "tfoot" | "caption" | "colgroup" | "html" | "head"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implied_structure() {
        let doc = parse("<title>T</title><p>Hello<p>World");
        let d = doc.dump();
        assert!(d.contains("<html>"), "{d}");
        assert!(d.contains("<head>"), "{d}");
        assert!(d.contains("<title>"), "{d}");
        assert!(d.contains("<body>"), "{d}");
        assert_eq!(doc.title().as_deref(), Some("T"));
        let body = doc.body().unwrap();
        let ps: Vec<_> = doc.child_elements(body).collect();
        assert_eq!(ps.len(), 2, "{d}");
    }

    #[test]
    fn lists_and_tables() {
        let doc = parse("<ul><li>a<li>b<ul><li>c</ul><li>d</ul><table><tr><td>1<td>2<tr><td>3</table>");
        let d = doc.dump();
        let body = doc.body().unwrap();
        let ul = doc.child_elements(body).next().unwrap();
        let lis: Vec<_> = doc.child_elements(ul).collect();
        assert_eq!(lis.len(), 3, "{d}");
        let nested_lis = doc.descendants(lis[1]).filter(|&n| doc.is_element(n, "li")).count();
        assert_eq!(nested_lis, 2, "{d}");
        let table = doc.find_element("table").unwrap();
        let trs = doc.descendants(table).filter(|&n| doc.is_element(n, "tr")).count();
        assert_eq!(trs, 2, "{d}");
        let tds = doc.descendants(table).filter(|&n| doc.is_element(n, "td")).count();
        assert_eq!(tds, 3, "{d}");
    }

    #[test]
    fn misnested_end_tags_ignored() {
        let doc = parse("<div><span>a</div></span>b");
        let body = doc.body().unwrap();
        let kids: Vec<_> = doc.children(body).collect();
        assert_eq!(kids.len(), 2); // div, text "b"
        assert!(matches!(doc.node(kids[1]).data, NodeData::Text(ref t) if t == "b"));
    }

    #[test]
    fn svg_self_closing() {
        let doc = parse("<p><svg viewBox='0 0 1 1'><path d='M0 0'/><circle r='1'/></svg>after</p>");
        let svg = doc.find_element("svg").unwrap();
        assert_eq!(doc.child_elements(svg).count(), 2);
        let p = doc.find_element("p").unwrap();
        assert!(doc.text_content(p).contains("after"));
    }
}
