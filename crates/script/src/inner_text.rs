//! `innerText` / `outerText` getters (HTML "the innerText getter steps"):
//! the rendered text collection steps over the element's children, with
//! the computed `display`, `visibility`, `white-space` and
//! `text-transform` of each node.
//!
//! Line layout is not consulted: white space is collapsed per inline
//! formatting context (sequences, line starts and ends at block, `<br>`
//! and preserved-newline boundaries), which is what the getter observes
//! except for soft wraps, which it ignores anyway.

use crate::env::with_env;
use crate::host::*;
use rabbithole_dom::{Document, Namespace, NodeData, NodeId};
use rabbithole_style::{ComputedStyle, Display, Float, Position, TextTransform, Visibility, WhiteSpace};
use std::rc::Rc;

/// One rendered-text item, before white space and line breaks resolve.
enum Item {
    /// A character; `true` when it is collapsible white space.
    Char(char, bool),
    /// A string item (`<br>`'s newline, a cell's tab, a row's newline).
    Str(&'static str),
    /// A required line break count (blocks: 1, `<p>`: 2).
    Breaks(u8),
    /// Start / end of an atomic inline (replaced element, inline-block):
    /// not white space, and its contents form their own lines.
    AtomicStart,
    AtomicEnd,
}

#[derive(Clone)]
struct Node {
    id: NodeId,
    kind: Kind,
}

#[derive(Clone)]
enum Kind {
    Text(String),
    Element { tag: String, ns: Namespace },
    Other,
}

fn node_info(doc: &Document, id: NodeId) -> Node {
    let kind = match &doc.node(id).data {
        NodeData::Text(t) => Kind::Text(t.clone()),
        NodeData::Element(e) => Kind::Element { tag: e.tag.clone(), ns: e.ns.clone() },
        _ => Kind::Other,
    };
    Node { id, kind }
}

/// Elements whose children never produce boxes of their own.
fn replaced(tag: &str) -> bool {
    matches!(tag, "iframe" | "input" | "textarea" | "canvas" | "video" | "audio" | "img" | "object" | "embed" | "noscript" | "frame" | "frameset" | "meter" | "progress")
}

/// SVG elements whose contents are never rendered.
fn svg_unrendered(tag: &str) -> bool {
    matches!(
        tag,
        "defs" | "symbol" | "clipPath" | "clippath" | "mask" | "pattern" | "linearGradient" | "lineargradient" | "radialGradient" | "radialgradient" | "stop" | "marker" | "filter" | "title" | "desc" | "metadata" | "style" | "script"
    )
}

struct Collector {
    doc: u32,
    items: Vec<Item>,
}

impl Collector {
    fn style(&self, id: NodeId) -> Option<Rc<ComputedStyle>> {
        with_env(|env| env.computed_style(id))
    }

    fn info(&self, id: NodeId) -> Node {
        with_doc(self.doc, |doc| node_info(doc, id))
    }

    fn children(&self, id: NodeId) -> Vec<NodeId> {
        with_doc(self.doc, |doc| doc.children(id).collect())
    }

    fn parent(&self, id: NodeId) -> Option<NodeId> {
        with_doc(self.doc, |doc| doc.parent(id))
    }

    fn tag(&self, id: NodeId) -> Option<(String, Namespace)> {
        match self.info(id).kind {
            Kind::Element { tag, ns } => Some((tag, ns)),
            _ => None,
        }
    }

    /// The rendered text collection steps for `id`'s children, appended.
    fn children_of(&mut self, id: NodeId, in_select: bool, in_option: bool) {
        for c in self.children(id) {
            self.collect(c, in_select, in_option);
        }
    }

    fn collect(&mut self, id: NodeId, in_select: bool, in_option: bool) {
        let info = self.info(id);
        match info.kind {
            Kind::Other => {}
            Kind::Text(text) => {
                let Some(parent) = self.parent(id) else { return };
                if in_select && !in_option {
                    return;
                }
                let Some((ptag, pns)) = self.tag(parent) else { return };
                if pns == Namespace::Html && (replaced(&ptag) || ptag == "template") {
                    return;
                }
                if pns == Namespace::Svg && !matches!(ptag.as_str(), "text" | "tspan" | "textPath" | "textpath" | "a") {
                    return;
                }
                let Some(style) = self.style(parent) else { return };
                // (Options render inside their select whatever their own box.)
                if style.visibility != Visibility::Visible || (style.display.is_none() && !in_option) {
                    return;
                }
                // White space directly inside table boxes has no box.
                if matches!(
                    style.display,
                    Display::Table | Display::InlineTable | Display::TableRow | Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup | Display::TableColumn | Display::TableColumnGroup
                ) && text.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c'))
                {
                    return;
                }
                let lang = self.lang(parent);
                self.text(&text, style.white_space, style.text_transform, &lang);
            }
            Kind::Element { tag, ns } => {
                if ns == Namespace::Svg && svg_unrendered(&tag) {
                    return;
                }
                if ns == Namespace::Html && tag == "template" {
                    return;
                }
                let Some(style) = self.style(id) else { return };
                let html = ns == Namespace::Html;
                // A select's options and groups are listed one per line
                // whatever boxes the control uses for them.
                let select_item = in_select && html && matches!(tag.as_str(), "option" | "optgroup");
                if style.display.is_none() && !select_item && !(in_select && in_option) {
                    return;
                }
                let (in_select, in_option) = match (html, tag.as_str()) {
                    (true, "select") => (true, false),
                    (true, "option") => (in_select, true),
                    _ => (in_select, in_option),
                };
                let visible = style.visibility == Visibility::Visible;
                if html && tag == "br" {
                    if visible {
                        self.items.push(Item::Str("\n"));
                    }
                    return;
                }
                let floated = style.float != Float::None || matches!(style.position, Position::Absolute | Position::Fixed);
                let block = !matches!(style.display, Display::Contents) && (style.display.is_block_level() || floated || (html && matches!(tag.as_str(), "option" | "optgroup")));
                let block = block && !matches!(style.display, Display::TableCell | Display::TableRow | Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup);
                let atomic = !block
                    && (matches!(style.display, Display::InlineBlock | Display::InlineFlex | Display::InlineGrid | Display::InlineTable)
                        || (html && (replaced(&tag) || tag == "select"))
                        || (ns == Namespace::Svg && tag == "svg"));
                let p = html && tag == "p";
                let block = block || p;
                let atomic = atomic && !p;
                let breaks = if p { 2 } else { 1 };
                let cell = style.display == Display::TableCell;
                let row = style.display == Display::TableRow;
                if visible {
                    if block {
                        self.items.push(Item::Breaks(breaks));
                    } else if atomic {
                        self.items.push(Item::AtomicStart);
                    } else if cell || row {
                        self.items.push(Item::Breaks(0));
                    }
                }
                // Contents of replaced elements and closed <details> (but
                // their first <summary>) are not rendered.
                let skip_children = html && replaced(&tag);
                if !skip_children {
                    if html && tag == "details" && with_doc(self.doc, |doc| doc.attr(id, "open").is_none()) {
                        let summary = self.children(id).into_iter().find(|&c| self.tag(c).is_some_and(|(t, n)| t == "summary" && n == Namespace::Html));
                        if let Some(s) = summary {
                            self.collect(s, in_select, in_option);
                        }
                    } else {
                        self.children_of(id, in_select, in_option);
                    }
                }
                if visible {
                    if block {
                        self.items.push(Item::Breaks(breaks));
                    } else if atomic {
                        self.items.push(Item::AtomicEnd);
                    } else if cell || row {
                        self.items.push(Item::Breaks(0));
                    }
                    if cell && !self.last_cell(id) {
                        self.items.push(Item::Str("\t"));
                    }
                    if row && !self.last_row(id) {
                        self.items.push(Item::Str("\n"));
                    }
                }
            }
        }
    }

    fn display(&self, id: NodeId) -> Option<Display> {
        self.tag(id)?;
        self.style(id).map(|s| s.display)
    }

    /// Is `cell` the last table-cell box of its row?
    fn last_cell(&self, cell: NodeId) -> bool {
        let mut next = with_doc(self.doc, |doc| doc.next_sibling(cell));
        while let Some(n) = next {
            if self.display(n) == Some(Display::TableCell) {
                return false;
            }
            next = with_doc(self.doc, |doc| doc.next_sibling(n));
        }
        true
    }

    /// Is `row` the last table-row box of its table?
    fn last_row(&self, row: NodeId) -> bool {
        // The enclosing table, then any later row inside it (not in a
        // nested table).
        let mut table = self.parent(row);
        while let Some(t) = table {
            if matches!(self.display(t), Some(Display::Table | Display::InlineTable)) {
                break;
            }
            table = self.parent(t);
        }
        let Some(table) = table else { return true };
        let mut found = false;
        let mut stack: Vec<NodeId> = self.children(table).into_iter().rev().collect();
        while let Some(n) = stack.pop() {
            if n == row {
                found = true;
                continue;
            }
            let d = self.display(n);
            match d {
                Some(Display::TableRow) if found => return false,
                Some(Display::TableRow) => {}
                Some(Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup) => {
                    stack.extend(self.children(n).into_iter().rev());
                }
                _ => {}
            }
        }
        true
    }

    fn lang(&self, mut id: NodeId) -> String {
        loop {
            if let Some(l) = with_doc(self.doc, |doc| doc.attr(id, "lang").map(|s| s.to_ascii_lowercase())) {
                return l;
            }
            match self.parent(id) {
                Some(p) => id = p,
                None => return String::new(),
            }
        }
    }

    fn text(&mut self, text: &str, ws: WhiteSpace, transform: TextTransform, lang: &str) {
        let text = apply_transform(text, transform, lang);
        let collapse_spaces = matches!(ws, WhiteSpace::Normal | WhiteSpace::Nowrap | WhiteSpace::PreLine);
        let collapse_newlines = matches!(ws, WhiteSpace::Normal | WhiteSpace::Nowrap);
        for c in text.chars() {
            let c = if c == '\r' { '\n' } else { c };
            match c {
                '\n' if collapse_newlines => self.items.push(Item::Char(' ', true)),
                '\n' => {
                    // A preserved segment break: spaces around it collapse
                    // (pre-line), and it ends the line.
                    self.items.push(Item::Char('\n', false));
                }
                ' ' | '\t' if collapse_spaces => self.items.push(Item::Char(' ', true)),
                _ => self.items.push(Item::Char(c, false)),
            }
        }
    }
}

fn apply_transform(text: &str, transform: TextTransform, lang: &str) -> String {
    let turkic = lang == "tr" || lang.starts_with("tr-") || lang == "az" || lang.starts_with("az-");
    match transform {
        TextTransform::None => text.to_string(),
        TextTransform::Uppercase if turkic => text.chars().map(|c| if c == 'i' { "\u{130}".to_string() } else { c.to_uppercase().collect() }).collect(),
        TextTransform::Uppercase => text.to_uppercase(),
        TextTransform::Lowercase if turkic => text.chars().map(|c| if c == 'I' { "\u{131}".to_string() } else { c.to_lowercase().collect() }).collect(),
        TextTransform::Lowercase => text.to_lowercase(),
        TextTransform::Capitalize => {
            let mut out = String::with_capacity(text.len());
            let mut word_start = true;
            for c in text.chars() {
                if word_start && c.is_alphanumeric() {
                    out.extend(c.to_uppercase());
                    word_start = false;
                } else {
                    out.push(c);
                    if c.is_whitespace() {
                        word_start = true;
                    } else if c.is_alphanumeric() {
                        word_start = false;
                    }
                }
            }
            out
        }
    }
}

/// White space and required line breaks resolved into the result.
fn assemble(items: Vec<Item>) -> String {
    // Collapsible spaces: one per run, none at line starts or ends.
    enum Out {
        S(String),
        B(u8),
    }
    let mut out: Vec<Out> = Vec::new();
    let mut cur = String::new();
    let mut line_start = true;
    let mut pending = false;
    let flush = |cur: &mut String, out: &mut Vec<Out>| {
        if !cur.is_empty() {
            out.push(Out::S(std::mem::take(cur)));
        }
    };
    for it in items {
        match it {
            Item::Char(_, true) => {
                if !line_start {
                    pending = true;
                }
            }
            Item::Char(c, false) => {
                if c == '\n' {
                    // Spaces before a preserved newline hang and collapse.
                    pending = false;
                    cur.push('\n');
                    line_start = true;
                    continue;
                }
                if pending {
                    cur.push(' ');
                    pending = false;
                }
                cur.push(c);
                line_start = false;
            }
            Item::Str(s) => {
                pending = false;
                cur.push_str(s);
                line_start = true;
            }
            Item::AtomicStart => {
                if pending {
                    cur.push(' ');
                    pending = false;
                }
                line_start = true;
            }
            Item::AtomicEnd => {
                pending = false;
                line_start = false;
            }
            Item::Breaks(n) => {
                pending = false;
                line_start = true;
                flush(&mut cur, &mut out);
                if n > 0 {
                    out.push(Out::B(n));
                }
            }
        }
    }
    flush(&mut cur, &mut out);
    // Leading and trailing break counts go; runs become that many newlines.
    let mut res = String::new();
    let mut breaks: u8 = 0;
    let mut started = false;
    for o in out {
        match o {
            Out::B(n) => breaks = breaks.max(n),
            Out::S(s) => {
                if started {
                    for _ in 0..breaks {
                        res.push('\n');
                    }
                }
                breaks = 0;
                started = true;
                res.push_str(&s);
            }
        }
    }
    res
}

/// Is `n` being rendered (connected, in the rendered document, no
/// display:none ancestor and not inside replaced content)?
fn being_rendered(n: NodeRef) -> bool {
    if n.doc != 0 {
        return false;
    }
    let mut chain = Vec::new();
    let connected = with_doc(n.doc, |doc| {
        let mut cur = Some(n.node);
        while let Some(c) = cur {
            if c == Document::ROOT {
                return true;
            }
            chain.push(c);
            cur = doc.parent(c);
        }
        false
    });
    if !connected {
        return false;
    }
    for (i, &id) in chain.iter().enumerate() {
        let Some((tag, ns)) = with_doc(n.doc, |doc| doc.element(id).map(|e| (e.tag.clone(), e.ns.clone()))) else { continue };
        let Some(style) = with_env(|env| env.computed_style(id)) else { return false };
        if style.display.is_none() {
            return false;
        }
        if i > 0 {
            if ns == Namespace::Html && (replaced(&tag) || tag == "template") {
                return false;
            }
            if ns == Namespace::Svg && svg_unrendered(&tag) {
                return false;
            }
            // An HTML child of SVG content outside <foreignObject>.
            let child_ns = with_doc(n.doc, |doc| doc.element(chain[i - 1]).map(|e| e.ns.clone()));
            if ns == Namespace::Svg && !tag.eq_ignore_ascii_case("foreignObject") && child_ns == Some(Namespace::Html) {
                return false;
            }
        }
    }
    true
}

/// The innerText getter steps.
pub(crate) fn inner_text(n: NodeRef) -> String {
    if !being_rendered(n) {
        return with_doc(n.doc, |doc| doc.text_content(n.node));
    }
    let mut c = Collector { doc: n.doc, items: Vec::new() };
    let (in_select, in_option) = with_doc(n.doc, |doc| {
        let mut sel = false;
        let mut opt = false;
        let mut cur = Some(n.node);
        while let Some(id) = cur {
            if let Some(e) = doc.element(id) {
                if e.ns == Namespace::Html && e.tag == "option" {
                    opt = true;
                }
                if e.ns == Namespace::Html && e.tag == "select" {
                    sel = true;
                }
            }
            cur = doc.parent(id);
        }
        (sel, opt)
    });
    c.children_of(n.node, in_select, in_option);
    assemble(c.items)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str, collapsible: bool) -> Vec<Item> {
        s.chars().map(|c| Item::Char(c, collapsible && c == ' ')).collect()
    }

    #[test]
    fn collapsing_and_breaks() {
        let mut items = chars(" abc  ", true);
        items.push(Item::Breaks(2));
        items.extend(chars("def", true));
        items.push(Item::Breaks(1));
        assert_eq!(assemble(items), "abc\n\ndef");
        let mut items = chars("abc ", true);
        items.push(Item::AtomicStart);
        items.push(Item::AtomicEnd);
        items.extend(chars(" def", true));
        assert_eq!(assemble(items), "abc  def");
        let mut items = chars("abc ", true);
        items.push(Item::Str("\n"));
        items.extend(chars(" def", true));
        assert_eq!(assemble(items), "abc\ndef");
    }

    #[test]
    fn text_transform() {
        assert_eq!(apply_transform("Ma\u{df}", TextTransform::Uppercase, ""), "MASS");
        assert_eq!(apply_transform("i \u{131}", TextTransform::Uppercase, "tr"), "\u{130} I");
        assert_eq!(apply_transform("hello world", TextTransform::Capitalize, ""), "Hello World");
    }
}
