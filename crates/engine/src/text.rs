//! Readable text extraction (Markdown-flavoured) for CLI and MCP use.

use rabbithole_dom::{Document, NodeData, NodeId};
use rabbithole_net::Url;
use rabbithole_style::StyledTree;

pub fn extract(doc: &Document, styles: &StyledTree, base: &Url) -> String {
    let mut w = Writer {
        doc,
        styles,
        base,
        out: String::new(),
        pre_depth: 0,
        list_stack: Vec::new(),
        pending_space: false,
        at_line_start: true,
    };
    if let Some(body) = doc.body() {
        w.walk(body);
    } else {
        w.walk(Document::ROOT);
    }
    let text = w.out;
    // Collapse 3+ blank lines into 2.
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.lines() {
        let t = line.trim_end();
        if t.is_empty() {
            blank += 1;
            if blank <= 2 {
                out.push('\n');
            }
        } else {
            blank = 0;
            out.push_str(t);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

struct Writer<'a> {
    doc: &'a Document,
    styles: &'a StyledTree,
    base: &'a Url,
    out: String,
    pre_depth: usize,
    list_stack: Vec<Option<usize>>,
    pending_space: bool,
    at_line_start: bool,
}

impl Writer<'_> {
    fn newline(&mut self) {
        if !self.at_line_start {
            self.out.push('\n');
            self.at_line_start = true;
        }
        self.pending_space = false;
    }

    fn blank_line(&mut self) {
        self.newline();
        if !self.out.ends_with("\n\n") && !self.out.is_empty() {
            self.out.push('\n');
        }
    }

    fn push_text(&mut self, s: &str) {
        if self.pre_depth > 0 {
            self.out.push_str(s);
            self.at_line_start = s.ends_with('\n');
            return;
        }
        for word in s.split_whitespace() {
            if self.pending_space && !self.at_line_start {
                self.out.push(' ');
            }
            self.out.push_str(word);
            self.at_line_start = false;
            self.pending_space = true;
        }
        if s.ends_with(char::is_whitespace) || s.starts_with(char::is_whitespace) && s.trim().is_empty() {
            self.pending_space = true;
        }
    }

    fn push_raw(&mut self, s: &str) {
        self.out.push_str(s);
        self.at_line_start = s.ends_with('\n');
        self.pending_space = false;
    }

    fn walk(&mut self, id: NodeId) {
        match &self.doc.node(id).data {
            NodeData::Text(t) => self.push_text(t),
            NodeData::Comment(_) => {}
            NodeData::Document => {
                for c in self.doc.children(id) {
                    self.walk(c);
                }
            }
            NodeData::Element(el) => {
                let tag = el.tag.clone();
                if matches!(tag.as_str(), "script" | "style" | "template" | "head" | "title" | "meta" | "link") {
                    return;
                }
                // Respect display:none (styles are None for hidden subtrees).
                let style = self.styles.get(id).cloned();
                let Some(style) = style else {
                    return;
                };
                let is_block = style.display.is_block_level();
                match tag.as_str() {
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        self.blank_line();
                        let level = tag[1..].parse::<usize>().unwrap_or(1);
                        self.push_raw(&format!("{} ", "#".repeat(level)));
                        self.walk_children(id);
                        self.blank_line();
                    }
                    "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "nav" | "aside"
                    | "blockquote" | "figure" | "figcaption" | "address" | "fieldset" | "form" | "details"
                    | "summary" | "dl" | "dd" | "dt" | "center" => {
                        if tag == "blockquote" {
                            self.blank_line();
                            self.push_raw("> ");
                        } else if is_block {
                            self.newline();
                            if matches!(tag.as_str(), "p" | "section" | "article" | "blockquote" | "figure") {
                                self.blank_line();
                            }
                        }
                        self.walk_children(id);
                        if is_block {
                            self.newline();
                            if matches!(tag.as_str(), "p" | "section" | "article" | "blockquote" | "figure") {
                                self.blank_line();
                            }
                        }
                    }
                    "br" => self.push_raw("\n"),
                    "hr" => {
                        self.blank_line();
                        self.push_raw("---\n");
                        self.blank_line();
                    }
                    "pre" => {
                        self.blank_line();
                        self.push_raw("```\n");
                        self.pre_depth += 1;
                        self.walk_children(id);
                        self.pre_depth -= 1;
                        self.newline();
                        self.push_raw("```\n");
                        self.blank_line();
                    }
                    "ul" | "ol" | "menu" => {
                        self.newline();
                        let ordered = tag == "ol";
                        let start = el.attr("start").and_then(|s| s.parse::<usize>().ok()).unwrap_or(1);
                        self.list_stack.push(if ordered { Some(start) } else { None });
                        self.walk_children(id);
                        self.list_stack.pop();
                        self.newline();
                        if self.list_stack.is_empty() {
                            self.blank_line();
                        }
                    }
                    "li" => {
                        self.newline();
                        let depth = self.list_stack.len().saturating_sub(1);
                        let indent = "  ".repeat(depth);
                        let marker = match self.list_stack.last_mut() {
                            Some(Some(n)) => {
                                let m = format!("{n}. ");
                                *n += 1;
                                m
                            }
                            _ => "- ".to_string(),
                        };
                        self.push_raw(&format!("{indent}{marker}"));
                        self.walk_children(id);
                        self.newline();
                    }
                    "a" => {
                        let href = el.attr("href").map(|h| h.to_string());
                        let start = self.out.len();
                        self.walk_children(id);
                        if let Some(href) = href {
                            let label = self.out[start..].trim().to_string();
                            if !label.is_empty() && !href.starts_with('#') && !href.starts_with("javascript:") {
                                if let Some(abs) = self.base.join(&href) {
                                    self.out.truncate(start);
                                    if self.pending_space && !self.at_line_start && !self.out.ends_with(' ') && !self.out.ends_with('\n') {
                                        self.out.push(' ');
                                    }
                                    self.out.push_str(&format!("[{label}]({abs})"));
                                    self.at_line_start = false;
                                    self.pending_space = true;
                                }
                            }
                        }
                    }
                    "img" => {
                        let alt = el.attr("alt").unwrap_or("").trim();
                        if !alt.is_empty() {
                            self.push_text(&format!("![{alt}]"));
                        }
                    }
                    "b" | "strong" => {
                        self.push_text("**");
                        self.pending_space = false;
                        self.walk_children(id);
                        self.out.push_str("**");
                        self.pending_space = true;
                    }
                    "i" | "em" => {
                        self.push_text("_");
                        self.pending_space = false;
                        self.walk_children(id);
                        self.out.push('_');
                        self.pending_space = true;
                    }
                    "code" | "kbd" | "samp" if self.pre_depth == 0 => {
                        self.push_text("`");
                        self.pending_space = false;
                        self.walk_children(id);
                        self.out.push('`');
                        self.pending_space = true;
                    }
                    "table" => {
                        self.blank_line();
                        self.walk_children(id);
                        self.blank_line();
                    }
                    "tr" => {
                        self.newline();
                        self.push_raw("| ");
                        self.walk_children(id);
                        self.newline();
                    }
                    "td" | "th" => {
                        self.walk_children(id);
                        self.push_raw(" | ");
                    }
                    "input" => {
                        if let Some(v) = el.attr("value") {
                            if el.attr("type").map(|t| t.eq_ignore_ascii_case("submit") || t.eq_ignore_ascii_case("button")).unwrap_or(false) {
                                self.push_text(&format!("[{v}]"));
                            }
                        }
                    }
                    "button" => {
                        self.push_text("[");
                        self.pending_space = false;
                        self.walk_children(id);
                        self.out.push(']');
                        self.pending_space = true;
                    }
                    _ => {
                        if is_block {
                            self.newline();
                        }
                        self.walk_children(id);
                        if is_block {
                            self.newline();
                        }
                    }
                }
            }
        }
    }

    fn walk_children(&mut self, id: NodeId) {
        for c in self.doc.children(id) {
            self.walk(c);
        }
    }
}
