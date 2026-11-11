//! HTML parsing: tokenizer + a forgiving tree builder.
//!
//! The tree builder implements the parts of the WHATWG algorithm that matter
//! for rendering real pages: implied `html`/`head`/`body`, void elements,
//! implied end tags (`p`, `li`, `dt`/`dd`, `option`, table parts), scope
//! checks for end tags, raw-text elements and foreign (SVG/MathML) content.

pub mod entities;
pub mod tokenizer;

use rabbithole_dom::{DoctypeData, Document, Namespace, NodeData, NodeId, QuirksMode};
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

const HEAD_ONLY: &[&str] = &["base", "basefont", "bgsound", "link", "meta", "title", "style", "script", "noframes", "template", "noscript"];

/// Split the text of a DOCTYPE token (after `<!DOCTYPE`) into name, public
/// and system identifiers. The flag is the tokenizer's "force-quirks" flag
/// (missing name, malformed identifiers).
pub fn parse_doctype(text: &str) -> (DoctypeData, bool) {
    let text = text.trim();
    let name_end = text.find(|c: char| c.is_ascii_whitespace()).unwrap_or(text.len());
    let name = text[..name_end].to_ascii_lowercase();
    let mut rest = text[name_end..].trim_start();
    let mut data = DoctypeData { name, ..Default::default() };
    let mut force_quirks = data.name.is_empty();
    // A quoted identifier at the start of `rest`.
    fn quoted(rest: &mut &str) -> Option<String> {
        let q = rest.chars().next().filter(|&c| c == '"' || c == '\'')?;
        let body = &rest[1..];
        let (value, after) = match body.find(q) {
            Some(e) => (&body[..e], &body[e + 1..]),
            None => (body, ""),
        };
        *rest = after.trim_start();
        Some(value.to_string())
    }
    let keyword = |rest: &str, kw: &str| rest.len() >= kw.len() && rest.is_char_boundary(kw.len()) && rest[..kw.len()].eq_ignore_ascii_case(kw);
    if keyword(rest, "public") {
        rest = rest[6..].trim_start();
        match quoted(&mut rest) {
            Some(p) => {
                data.public_id = p;
                if let Some(s) = quoted(&mut rest) {
                    data.system_id = s;
                }
            }
            None => force_quirks = true,
        }
    } else if keyword(rest, "system") {
        rest = rest[6..].trim_start();
        match quoted(&mut rest) {
            Some(s) => data.system_id = s,
            None => force_quirks = true,
        }
    } else if !rest.is_empty() {
        force_quirks = true;
    }
    (data, force_quirks)
}

/// The document's quirks mode for a DOCTYPE (HTML §13.2.6.4.1).
pub fn quirks_mode_for(d: &DoctypeData, force_quirks: bool) -> QuirksMode {
    const QUIRKY_PREFIXES: &[&str] = &[
        "+//silmaril//dtd html pro v0r11 19970101//", "-//as//dtd html 3.0 aswedit + extensions//",
        "-//advasoft ltd//dtd html 3.0 aswedit + extensions//", "-//ietf//dtd html 2.0 level 1//",
        "-//ietf//dtd html 2.0 level 2//", "-//ietf//dtd html 2.0 strict level 1//", "-//ietf//dtd html 2.0 strict level 2//",
        "-//ietf//dtd html 2.0 strict//", "-//ietf//dtd html 2.0//", "-//ietf//dtd html 2.1e//", "-//ietf//dtd html 3.0//",
        "-//ietf//dtd html 3.2 final//", "-//ietf//dtd html 3.2//", "-//ietf//dtd html 3//", "-//ietf//dtd html level 0//",
        "-//ietf//dtd html level 1//", "-//ietf//dtd html level 2//", "-//ietf//dtd html level 3//",
        "-//ietf//dtd html strict level 0//", "-//ietf//dtd html strict level 1//", "-//ietf//dtd html strict level 2//",
        "-//ietf//dtd html strict level 3//", "-//ietf//dtd html strict//", "-//ietf//dtd html//",
        "-//metrius//dtd metrius presentational//", "-//microsoft//dtd internet explorer 2.0 html strict//",
        "-//microsoft//dtd internet explorer 2.0 html//", "-//microsoft//dtd internet explorer 2.0 tables//",
        "-//microsoft//dtd internet explorer 3.0 html strict//", "-//microsoft//dtd internet explorer 3.0 html//",
        "-//microsoft//dtd internet explorer 3.0 tables//", "-//netscape comm. corp.//dtd html//",
        "-//netscape comm. corp.//dtd strict html//", "-//o'reilly and associates//dtd html 2.0//",
        "-//o'reilly and associates//dtd html extended 1.0//", "-//o'reilly and associates//dtd html extended relaxed 1.0//",
        "-//sq//dtd html 2.0 hotmetal + extensions//",
        "-//softquad software//dtd hotmetal pro 6.0::19990601::extensions to html 4.0//",
        "-//softquad//dtd hotmetal pro 4.0::19970916::extensions to html 4.0//", "-//spyglass//dtd html 2.0 extended//",
        "-//sun microsystems corp.//dtd hotjava html//", "-//sun microsystems corp.//dtd hotjava strict html//",
        "-//w3c//dtd html 3 1995-03-24//", "-//w3c//dtd html 3.2 draft//", "-//w3c//dtd html 3.2 final//",
        "-//w3c//dtd html 3.2//", "-//w3c//dtd html 3.2s draft//", "-//w3c//dtd html 4.0 frameset//",
        "-//w3c//dtd html 4.0 transitional//", "-//w3c//dtd html experimental 19960712//",
        "-//w3c//dtd html experimental 970421//", "-//w3c//dtd w3 html//", "-//w3o//dtd w3 html 3.0//",
        "-//webtechs//dtd mozilla html 2.0//", "-//webtechs//dtd mozilla html//",
    ];
    let public = d.public_id.to_ascii_lowercase();
    let system = d.system_id.to_ascii_lowercase();
    let html401 = public.starts_with("-//w3c//dtd html 4.01 frameset//") || public.starts_with("-//w3c//dtd html 4.01 transitional//");
    if force_quirks
        || d.name != "html"
        || matches!(public.as_str(), "-//w3o//dtd w3 html strict 3.0//en//" | "-/w3c/dtd html 4.0 transitional/en" | "html")
        || system == "http://www.ibm.com/data/dtd/v11/ibmxhtml1-transitional.dtd"
        || QUIRKY_PREFIXES.iter().any(|p| public.starts_with(p))
        || (d.system_id.is_empty() && html401)
    {
        return QuirksMode::Quirks;
    }
    if public.starts_with("-//w3c//dtd xhtml 1.0 frameset//") || public.starts_with("-//w3c//dtd xhtml 1.0 transitional//") || html401 {
        return QuirksMode::LimitedQuirks;
    }
    QuirksMode::NoQuirks
}

pub fn parse(input: &str) -> Document {
    let mut doc = parse_tree(input);
    doc.adopt_template_contents(Document::ROOT, true);
    doc
}

/// Tree construction only (template children still in the tree).
fn parse_tree(input: &str) -> Document {
    let mut builder = TreeBuilder::new();
    builder.run(input);
    builder.doc
}

/// Parse an HTML fragment as the children of a `context` element (as
/// `innerHTML` does). The returned document's `body` holds the fragment
/// nodes, in order.
pub fn parse_fragment(input: &str, context: &str) -> Document {
    let ctx = context.to_ascii_lowercase();
    let (prefix, inner_tag) = match ctx.as_str() {
        "tr" => ("<table><tbody><tr>", "tr"),
        "tbody" | "thead" | "tfoot" => ("<table><tbody>", "tbody"),
        "td" | "th" => ("<table><tbody><tr><td>", "td"),
        "table" => ("<table>", "table"),
        "colgroup" => ("<table><colgroup>", "colgroup"),
        "select" => ("<select>", "select"),
        "ul" | "ol" => ("<ul>", "ul"),
        "dl" => ("<dl>", "dl"),
        _ => ("", ""),
    };
    // Fragment parsing (innerHTML) never creates declarative shadow roots.
    let parse = |s: &str| {
        let mut d = parse_tree(s);
        d.adopt_template_contents(Document::ROOT, false);
        d
    };
    if prefix.is_empty() {
        let mut doc = parse(input);
        // Head-only elements (style/script/meta…) at the start of a
        // fragment end up in <head>; move them in front of the body content.
        if let (Some(head), Some(body)) = (doc.head(), doc.body()) {
            let head_children: Vec<NodeId> = doc.children(head).collect();
            let first_body = doc.first_child(body);
            for c in head_children {
                doc.insert_before(body, c, first_body);
            }
        }
        return doc;
    }
    let mut doc = parse(&format!("{prefix}{input}"));
    let Some(body) = doc.body() else { return doc };
    let inner = doc.find_element(inner_tag);
    let children: Vec<NodeId> = match inner {
        Some(n) => doc.children(n).collect(),
        None => Vec::new(),
    };
    doc.clear_children(body);
    for c in children {
        doc.detach(c);
        doc.append_child(body, c);
    }
    doc
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
    /// A DOCTYPE token was seen before the first element.
    saw_doctype: bool,
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
            saw_doctype: false,
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
                Token::Doctype(text) => self.doctype(&text),
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
        if !self.saw_doctype {
            self.doc.quirks_mode = QuirksMode::Quirks;
        }
    }

    /// "initial" insertion mode: a DOCTYPE before anything else becomes the
    /// document's DocumentType node and decides the quirks mode.
    fn doctype(&mut self, text: &str) {
        if self.saw_doctype || self.html.is_some() {
            return; // parse error: ignored
        }
        self.saw_doctype = true;
        let (data, force_quirks) = parse_doctype(text);
        self.doc.quirks_mode = quirks_mode_for(&data, force_quirks);
        let n = self.doc.create(NodeData::Doctype(data));
        self.doc.append_child(Document::ROOT, n);
    }

    fn text(&mut self, t: String) {
        if self.body.is_none() {
            // Whitespace before body is dropped; anything else opens the body.
            if t.trim().is_empty() {
                // ...except inside <head> ("in head" mode inserts it, and
                // hydrating frameworks count those text nodes) and its raw
                // elements like <title>.
                let cur = self.current_tag().to_string();
                if cur == "head" || HEAD_ONLY.contains(&cur.as_str()) {
                    self.append_text(t);
                }
                return;
            }
            let in_noscript = self.stack.iter().any(|&n| self.doc.element(n).map(|e| e.tag == "noscript").unwrap_or(false));
            let cur = self.current_tag().to_string();
            if !HEAD_ONLY.contains(&cur.as_str()) && !in_noscript {
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
        // Elements inside <svg>/<math> are in the SVG/MathML namespace
        // (HTML integration points switch back to HTML).
        let ns = match name {
            "svg" => Namespace::Svg,
            "math" => Namespace::MathMl,
            _ => match self.doc.element(parent) {
                Some(p) if p.ns == Namespace::Svg && !matches!(p.tag.as_str(), "foreignobject" | "foreignObject" | "desc" | "title") => Namespace::Svg,
                Some(p) if p.ns == Namespace::MathMl && !matches!(p.tag.as_str(), "mi" | "mo" | "mn" | "ms" | "mtext" | "annotation-xml") => Namespace::MathMl,
                _ => Namespace::Html,
            },
        };
        let n = if ns == Namespace::Html { self.doc.create_element(name, attrs) } else { self.doc.create_element_ns(ns, None, &name.to_ascii_lowercase(), attrs) };
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

        // Anything inside a `<noscript>` in the head stays there (browsers
        // with scripting on treat it as inert text), so tracking pixels and
        // fallbacks don't leak into the body.
        if self.body.is_none() && self.stack.iter().any(|&n| self.doc.element(n).map(|e| e.tag == "noscript").unwrap_or(false)) {
            let is_void = VOID.contains(&name);
            self.insert_element(name, attrs, !is_void && !self_closing);
            return raw_kind(name);
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
                self.close_table_part(&["tr", "caption", "colgroup"], &["table", "thead", "tbody", "tfoot"]);
            }
            "td" | "th" => {
                self.close_table_part(&["td", "th", "caption", "colgroup"], &["table", "tr"]);
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

        // Implied table wrappers ("in table" / "in table body" insertion
        // modes): rows live in a tbody, cells in a row, cols in a colgroup.
        match (name, self.current_tag()) {
            ("tr", "table") => {
                self.insert_element("tbody", Vec::new(), true);
            }
            ("td" | "th", "table") => {
                self.insert_element("tbody", Vec::new(), true);
                self.insert_element("tr", Vec::new(), true);
            }
            ("td" | "th", "tbody" | "thead" | "tfoot") => {
                self.insert_element("tr", Vec::new(), true);
            }
            ("col", "table") => {
                self.insert_element("colgroup", Vec::new(), true);
            }
            _ => {}
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

    /// Whitespace and comments inside `<head>` stay where they are (SSR
    /// hydration walks those siblings by count).
    #[test]
    fn head_keeps_whitespace_and_comments() {
        let doc = parse("<!doctype html>\n<html>\n<head>\n<meta charset=utf-8>\n<!--h--><!--[--><link rel=a><!--]--> <meta name=x> <title>T</title></head>\n<body><p>x</p></body></html>");
        let head = doc.head().unwrap();
        let kinds: Vec<String> = doc
            .children(head)
            .map(|c| match &doc.node(c).data {
                NodeData::Text(t) => format!("text({})", t.escape_debug()),
                NodeData::Comment(t) => format!("comment({t})"),
                NodeData::Element(e) => e.tag.clone(),
                _ => "?".into(),
            })
            .collect();
        assert_eq!(kinds, ["text(\\n)", "meta", "text(\\n)", "comment(h)", "comment([)", "link", "comment(])", "text( )", "meta", "text( )", "title"]);
        // Whitespace before <head> and after </head> is not kept.
        let html = doc.html_element().unwrap();
        assert!(doc.children(html).all(|c| doc.element(c).is_some()), "{}", doc.dump());
    }

    #[test]
    fn doctype_node_and_quirks_mode() {
        let doc = parse("<!DOCTYPE HTML PUBLIC \"-//W3C//DTD HTML 4.01//EN\" \"http://www.w3.org/TR/html4/strict.dtd\">\n<html><p>x");
        let dt = doc.first_child(Document::ROOT).unwrap();
        let NodeData::Doctype(d) = &doc.node(dt).data else { panic!("{}", doc.dump()) };
        assert_eq!(d.name, "html");
        assert_eq!(d.public_id, "-//W3C//DTD HTML 4.01//EN");
        assert_eq!(d.system_id, "http://www.w3.org/TR/html4/strict.dtd");
        assert_eq!(doc.doctype(), Some(dt));
        assert_eq!(doc.quirks_mode, QuirksMode::NoQuirks);
        assert!(doc.html_element().is_some());
        assert_eq!(parse("<!doctype html><p>").quirks_mode, QuirksMode::NoQuirks);
        assert_eq!(parse("<p>no doctype").quirks_mode, QuirksMode::Quirks);
        assert!(parse("<p>no doctype").doctype().is_none());
        assert_eq!(parse("<!DOCTYPE HTML PUBLIC \"-//W3C//DTD HTML 4.01 Transitional//EN\">").quirks_mode, QuirksMode::Quirks);
        assert_eq!(parse("<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Transitional//EN\" \"x\">").quirks_mode, QuirksMode::LimitedQuirks);
        // Foreign content is namespaced.
        let doc = parse("<svg><circle/><foreignObject><div></div></foreignObject></svg><math><mi>x</mi></math>");
        let circle = doc.find_element("circle").unwrap();
        assert_eq!(doc.element(circle).unwrap().ns, Namespace::Svg);
        let div = doc.find_element("div").unwrap();
        assert_eq!(doc.element(div).unwrap().ns, Namespace::Html);
        let mi = doc.find_element("mi").unwrap();
        assert_eq!(doc.element(mi).unwrap().ns, Namespace::MathMl);
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
    fn implied_tbody_tr_and_colgroup() {
        let doc = parse("<table><col><tr><td>1<td>2<tr><td>3</table><table><td>x</td></table><table><tr><td><p></tbody> </table>");
        let d = doc.dump();
        let tables: Vec<_> = doc.descendants(Document::ROOT).filter(|&n| doc.is_element(n, "table")).collect();
        let kids = |n| doc.child_elements(n).map(|c| doc.tag(c).unwrap_or("").to_string()).collect::<Vec<_>>();
        assert_eq!(kids(tables[0]), vec!["colgroup", "tbody"], "{d}");
        let tbody = doc.child_elements(tables[0]).nth(1).unwrap();
        assert_eq!(kids(tbody), vec!["tr", "tr"], "{d}");
        assert_eq!(kids(tables[1]), vec!["tbody"], "{d}");
        let tb = doc.child_elements(tables[1]).next().unwrap();
        assert_eq!(kids(tb), vec!["tr"], "{d}");
        // Acid3: whitespace after `</tbody>` stays a child of the table.
        let last = doc.children(tables[2]).last().unwrap();
        assert!(matches!(doc.node(last).data, NodeData::Text(ref t) if t == " "), "{d}");
        assert_eq!(doc.children(tables[2]).count(), 2, "{d}");
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
