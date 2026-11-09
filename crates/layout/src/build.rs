//! Box tree construction from the styled DOM.

use crate::counters::{format_counter, Counters, Quotes};
use crate::tree::{BoxKind, Flow, Formatting, InlineItem, LayoutBox};
use crate::{pseudo_style, LayoutContext};
use rabbithole_css::PseudoElement;
use rabbithole_dom::{Document, NodeData, NodeId};
use rabbithole_style::{ComputedStyle, Content, ContentItem, Display, Float, ListStyleType, Position};
use std::cell::RefCell;
use std::rc::Rc;

const REPLACED: &[&str] = &["img", "svg", "video", "canvas", "iframe", "embed", "object", "audio"];

pub(crate) struct Builder<'a> {
    ctx: &'a LayoutContext<'a>,
    counters: RefCell<Counters>,
    quotes: RefCell<Quotes>,
}

impl<'a> Builder<'a> {
    pub fn new(ctx: &'a LayoutContext<'a>) -> Self {
        Builder {
            ctx,
            counters: RefCell::new(Counters::new()),
            quotes: RefCell::new(Quotes::default()),
        }
    }

    /// Apply `counter-reset` / `counter-increment` of a style.
    fn apply_counters(&self, style: &ComputedStyle) {
        let mut c = self.counters.borrow_mut();
        for (name, v) in &style.counter_reset {
            c.reset(name, *v);
        }
        for (name, v) in &style.counter_increment {
            c.increment(name, *v);
        }
    }

    /// Build the `::before` / `::after` box of `node`, if it generates one.
    fn build_pseudo(&self, node: NodeId, which: PseudoElement, in_table_row: bool) -> Option<LayoutBox> {
        let style = pseudo_style(self.ctx.styles, node, which)?;
        if style.display == Display::None {
            return None;
        }
        let items = match &style.content {
            Content::Items(items) => items.clone(),
            _ => return None,
        };
        self.apply_counters(&style);
        let doc = self.doc();
        let mut children: Vec<LayoutBox> = Vec::new();
        let mut text = String::new();
        let flush_text = |text: &mut String, children: &mut Vec<LayoutBox>| {
            if !text.is_empty() {
                let mut t = LayoutBox::new(BoxKind::Text, Some(node), style.clone());
                t.text = Some(Rc::from(text.as_str()));
                children.push(t);
                text.clear();
            }
        };
        for item in &items {
            match item {
                ContentItem::String(s) => text.push_str(s),
                ContentItem::Attr(name) => {
                    if let Some(v) = doc.attr(node, name) {
                        text.push_str(v);
                    }
                }
                ContentItem::Counter(name, ls) => {
                    let v = self.counters.borrow().value(name);
                    text.push_str(&format_counter(v, *ls));
                }
                ContentItem::Counters(name, sep, ls) => {
                    let vals = self.counters.borrow().values(name);
                    let parts: Vec<String> = vals.iter().map(|v| format_counter(*v, *ls)).collect();
                    text.push_str(&parts.join(sep));
                }
                ContentItem::OpenQuote => text.push_str(&self.quotes.borrow_mut().open(&style.quotes)),
                ContentItem::CloseQuote => text.push_str(&self.quotes.borrow_mut().close(&style.quotes)),
                ContentItem::NoOpenQuote => self.quotes.borrow_mut().no_open(),
                ContentItem::NoCloseQuote => self.quotes.borrow_mut().no_close(),
                ContentItem::Url(url) => {
                    flush_text(&mut text, &mut children);
                    if let Some(img) = self.ctx.background_images.get(url) {
                        let mut b = LayoutBox::new(BoxKind::Image, Some(node), style.clone());
                        b.image = Some(img.clone());
                        b.bfc_root = true;
                        children.push(b);
                    }
                }
            }
        }
        flush_text(&mut text, &mut children);
        let out_of_flow = matches!(style.position, Position::Absolute | Position::Fixed);
        let floated = !out_of_flow && style.float != Float::None;
        let kind = if out_of_flow || floated {
            BoxKind::Block
        } else if style.display == Display::Inline {
            BoxKind::Inline
        } else if style.display.is_inline_level() {
            BoxKind::InlineBlock
        } else {
            BoxKind::Block
        };
        let mut b = LayoutBox::new(kind, Some(node), style.clone());
        b.pseudo = Some(which);
        b.flow = if out_of_flow {
            Flow::Absolute
        } else if floated {
            match style.float {
                Float::Right => Flow::FloatRight,
                _ => Flow::FloatLeft,
            }
        } else {
            Flow::InFlow
        };
        b.bfc_root = kind != BoxKind::Inline;
        b.clips = style.overflow_x.clips() || style.overflow_y.clips();
        let _ = in_table_row;
        if kind == BoxKind::Inline {
            b.children = children;
            b.formatting = Formatting::Inline;
        } else {
            self.finish_container(&mut b, children, false);
        }
        Some(b)
    }

    fn doc(&self) -> &'a Document {
        self.ctx.doc
    }

    /// Build the root box for the `html` element.
    pub fn build_root(&self) -> LayoutBox {
        let doc = self.doc();
        let html = doc.html_element();
        let style = html
            .and_then(|h| self.ctx.styles.get(h).cloned())
            .unwrap_or_else(|| Rc::new(ComputedStyle::initial()));
        let mut root = LayoutBox::new(BoxKind::Block, html, style);
        root.bfc_root = true;
        let children = match html {
            Some(h) => self.build_children(h, &root.style, false),
            None => Vec::new(),
        };
        self.finish_container(&mut root, children, false);
        root
    }

    /// Build the boxes for the children of `node`.
    fn build_children(&self, node: NodeId, parent_style: &Rc<ComputedStyle>, in_table_row: bool) -> Vec<LayoutBox> {
        let doc = self.doc();
        let mut out = Vec::new();
        for child in doc.children(node) {
            match &doc.node(child).data {
                NodeData::Text(t) => {
                    if t.is_empty() {
                        continue;
                    }
                    let mut b = LayoutBox::new(BoxKind::Text, Some(child), parent_style.clone());
                    b.text = Some(Rc::from(t.as_str()));
                    out.push(b);
                }
                NodeData::Element(_) => {
                    self.build_element(child, parent_style, in_table_row, &mut out);
                }
                _ => {}
            }
        }
        out
    }

    fn build_element(&self, node: NodeId, parent_style: &Rc<ComputedStyle>, in_table_row: bool, out: &mut Vec<LayoutBox>) {
        let doc = self.doc();
        let style = match self.ctx.styles.get(node) {
            Some(s) => s.clone(),
            None => return,
        };
        if style.display == Display::None {
            return;
        }
        if style.display == Display::Contents {
            out.extend(self.build_children(node, &style, in_table_row));
            return;
        }
        if matches!(style.display, Display::TableColumn | Display::TableColumnGroup) {
            return;
        }
        let tag = doc.tag(node).unwrap_or("");
        let _ = parent_style;
        self.apply_counters(&style);

        // Replaced elements.
        if REPLACED.contains(&tag) {
            if let Some(b) = self.build_replaced(node, tag, &style) {
                out.push(b);
            }
            return;
        }
        if tag == "br" {
            out.push(LayoutBox::new(BoxKind::LineBreak, Some(node), style));
            return;
        }

        let out_of_flow = matches!(style.position, Position::Absolute | Position::Fixed);
        let floated = !out_of_flow && style.float != Float::None;

        let kind = if out_of_flow || floated {
            BoxKind::Block
        } else {
            match style.display {
                Display::Inline => BoxKind::Inline,
                Display::InlineBlock | Display::InlineFlex | Display::InlineGrid | Display::InlineTable => BoxKind::InlineBlock,
                Display::TableCell => {
                    if in_table_row {
                        BoxKind::TableCell
                    } else {
                        BoxKind::InlineBlock
                    }
                }
                Display::Table => BoxKind::Table,
                Display::TableRow => BoxKind::TableRow,
                _ => BoxKind::Block,
            }
        };
        let mut b = LayoutBox::new(kind, Some(node), style.clone());
        b.flow = if out_of_flow {
            Flow::Absolute
        } else if floated {
            match style.float {
                Float::Right => Flow::FloatRight,
                _ => Flow::FloatLeft,
            }
        } else {
            Flow::InFlow
        };
        if style.display == Display::InlineTable && !out_of_flow && !floated {
            b.kind = BoxKind::InlineBlock;
        }
        b.bfc_root = out_of_flow
            || floated
            || matches!(kind, BoxKind::InlineBlock | BoxKind::TableCell | BoxKind::Table)
            || style.overflow_x.clips()
            || style.overflow_y.clips()
            || matches!(style.display, Display::FlowRoot | Display::Flex | Display::InlineFlex | Display::Grid | Display::InlineGrid);
        b.clips = (style.overflow_x.clips() || style.overflow_y.clips()) && !matches!(tag, "html" | "body");
        if let Some(cs) = doc.attr(node, "colspan").and_then(|v| v.trim().parse::<u32>().ok()) {
            b.colspan = cs.clamp(1, 100);
        }
        if style.display == Display::ListItem {
            b.marker_text = list_marker_text(doc, node, &style);
        }

        // Form controls get synthesized text content. Generated content
        // (`::before` / `::after`) wraps the element's children; counters
        // created by the children live in this element's scope.
        self.counters.borrow_mut().enter();
        let before = self.build_pseudo(node, PseudoElement::Before, kind == BoxKind::TableRow);
        let mut children = match tag {
            "input" => self.build_input(node, &style),
            "select" => self.build_select(node, &style),
            "textarea" => {
                let mut t = LayoutBox::new(BoxKind::Text, None, style.clone());
                t.text = Some(Rc::from(doc.text_content(node).as_str()));
                vec![t]
            }
            _ => self.build_children(node, &style, kind == BoxKind::TableRow),
        };
        let after = self.build_pseudo(node, PseudoElement::After, kind == BoxKind::TableRow);
        self.counters.borrow_mut().leave();
        if let Some(bf) = before {
            children.insert(0, bf);
        }
        if let Some(af) = after {
            children.push(af);
        }

        if kind == BoxKind::Inline {
            // Inline boxes keep their inline-level children as a tree; any
            // block-level child is hoisted by splitting (simplified: treated
            // as inline-block so nothing is lost).
            for c in &mut children {
                if matches!(c.kind, BoxKind::Block | BoxKind::Table) && c.flow.is_in_flow() {
                    c.kind = BoxKind::InlineBlock;
                    c.bfc_root = true;
                    // A block inside an inline normally takes the full width;
                    // emulate by marking it with the flag below.
                    c.attr_size = (Some(-1.0), None);
                }
            }
            b.children = children;
            b.formatting = Formatting::Inline;
            out.push(b);
            return;
        }

        // grid/flex hook: flex and grid containers blockify their children.
        match kind {
            BoxKind::Table => self.finish_table(&mut b, children),
            BoxKind::TableRow => self.finish_row(&mut b, children),
            _ => {
                if matches!(style.display, Display::Flex | Display::InlineFlex) {
                    self.finish_flex(&mut b, children);
                } else if matches!(style.display, Display::Grid | Display::InlineGrid) {
                    self.finish_grid(&mut b, children);
                } else {
                    self.finish_container(&mut b, children, false);
                }
            }
        }
        out.push(b);
    }

    fn build_replaced(&self, node: NodeId, tag: &str, style: &Rc<ComputedStyle>) -> Option<LayoutBox> {
        let doc = self.doc();
        let image = if tag == "img" { self.ctx.images.get(&node).cloned() } else { None };
        let attr_w = doc.attr(node, "width").and_then(parse_dimension_attr);
        let attr_h = doc.attr(node, "height").and_then(parse_dimension_attr);
        if tag == "img" && image.is_none() && attr_w.is_none() && style.width.is_auto() {
            // Missing image: render alt text inline instead.
            let alt = doc.attr(node, "alt").unwrap_or("").trim();
            if !alt.is_empty() {
                let mut t = LayoutBox::new(BoxKind::Text, Some(node), style.clone());
                t.text = Some(Rc::from(alt));
                return Some(t);
            }
        }
        let mut b = LayoutBox::new(BoxKind::Image, Some(node), style.clone());
        b.image = image;
        b.attr_size = (attr_w, attr_h);
        if tag == "iframe" || tag == "video" || tag == "object" || tag == "embed" {
            if b.attr_size.0.is_none() {
                b.attr_size.0 = Some(300.0);
            }
            if b.attr_size.1.is_none() {
                b.attr_size.1 = Some(150.0);
            }
        }
        b.bfc_root = true;
        b.flow = match style.position {
            Position::Absolute | Position::Fixed => Flow::Absolute,
            _ => match style.float {
                Float::Left => Flow::FloatLeft,
                Float::Right => Flow::FloatRight,
                Float::None => Flow::InFlow,
            },
        };
        if style.display.is_block_level() && b.flow.is_in_flow() {
            // A block-level image: wrap semantics are handled by the
            // container (it is treated as a block child).
            b.kind = BoxKind::Image;
        }
        Some(b)
    }

    fn build_input(&self, node: NodeId, style: &Rc<ComputedStyle>) -> Vec<LayoutBox> {
        let doc = self.doc();
        let ty = doc.attr(node, "type").unwrap_or("text").to_ascii_lowercase();
        let text = match ty.as_str() {
            "submit" => doc.attr(node, "value").unwrap_or("Submit").to_string(),
            "reset" => doc.attr(node, "value").unwrap_or("Reset").to_string(),
            "button" => doc.attr(node, "value").unwrap_or("").to_string(),
            "checkbox" | "radio" | "range" | "color" | "file" | "image" => String::new(),
            _ => doc
                .attr(node, "value")
                .filter(|v| !v.is_empty())
                .or_else(|| doc.attr(node, "placeholder"))
                .unwrap_or("")
                .to_string(),
        };
        if text.is_empty() {
            return Vec::new();
        }
        let mut t = LayoutBox::new(BoxKind::Text, None, style.clone());
        t.text = Some(Rc::from(text.as_str()));
        vec![t]
    }

    fn build_select(&self, node: NodeId, style: &Rc<ComputedStyle>) -> Vec<LayoutBox> {
        let doc = self.doc();
        let options: Vec<NodeId> = doc.descendants(node).filter(|&n| doc.is_element(n, "option")).collect();
        let chosen = options
            .iter()
            .copied()
            .find(|&o| doc.attr(o, "selected").is_some())
            .or_else(|| options.first().copied());
        let text = chosen
            .map(|o| doc.text_content(o).split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        let mut t = LayoutBox::new(BoxKind::Text, None, style.clone());
        t.text = Some(Rc::from(format!("{text} ▾").as_str()));
        vec![t]
    }

    /// Decide block vs inline formatting for a container and wrap inline
    /// runs in anonymous blocks when the content is mixed.
    pub(crate) fn finish_container(&self, b: &mut LayoutBox, children: Vec<LayoutBox>, blockify: bool) {
        let has_block = children
            .iter()
            .any(|c| c.flow.is_in_flow() && !c.is_inline_level());
        let has_inline = children.iter().any(|c| c.is_inline_level());
        if blockify {
            // Flex column / grid: every element child is a block; text runs
            // become anonymous blocks.
            let mut out = Vec::new();
            let mut run: Vec<LayoutBox> = Vec::new();
            for c in children {
                if c.is_inline_level() && !matches!(c.kind, BoxKind::InlineBlock | BoxKind::Image) {
                    run.push(c);
                } else {
                    self.flush_run(&mut run, &mut out, &b.style);
                    let mut c = c;
                    if c.kind == BoxKind::InlineBlock {
                        c.kind = BoxKind::Block;
                    }
                    if c.kind == BoxKind::Inline {
                        // An inline element as a flex item becomes a block.
                        let mut blk = LayoutBox::new(BoxKind::Block, c.node, c.style.clone());
                        blk.bfc_root = true;
                        let kids = std::mem::take(&mut c.children);
                        self.finish_container(&mut blk, kids, false);
                        out.push(blk);
                        continue;
                    }
                    out.push(c);
                }
            }
            self.flush_run(&mut run, &mut out, &b.style);
            b.children = out;
            b.formatting = Formatting::Block;
            return;
        }
        if has_inline && !has_block {
            b.formatting = Formatting::Inline;
            let mut items = Vec::new();
            let mut atomics = Vec::new();
            flatten_inline(children, b.node, &mut items, &mut atomics);
            b.items = items;
            b.children = atomics;
        } else if has_inline {
            let mut out = Vec::new();
            let mut run: Vec<LayoutBox> = Vec::new();
            for c in children {
                if c.is_inline_level() || (!c.flow.is_in_flow() && !run.is_empty()) {
                    run.push(c);
                } else {
                    self.flush_run(&mut run, &mut out, &b.style);
                    out.push(c);
                }
            }
            self.flush_run(&mut run, &mut out, &b.style);
            b.children = out;
            b.formatting = Formatting::Block;
        } else {
            b.children = children;
            b.formatting = Formatting::Block;
        }
    }

    /// Wrap an inline run in an anonymous block (dropping whitespace-only runs).
    fn flush_run(&self, run: &mut Vec<LayoutBox>, out: &mut Vec<LayoutBox>, parent_style: &Rc<ComputedStyle>) {
        if run.is_empty() {
            return;
        }
        let run = std::mem::take(run);
        let only_ws = run.iter().all(|c| match c.kind {
            BoxKind::Text => c.text.as_deref().map(|t| t.trim().is_empty()).unwrap_or(true),
            _ => !c.flow.is_in_flow(),
        });
        if only_ws {
            // Keep out-of-flow boxes, drop the whitespace.
            for c in run.into_iter().filter(|c| !c.flow.is_in_flow()) {
                out.push(c);
            }
            return;
        }
        let style = Rc::new(ComputedStyle::inherit_from(parent_style));
        let mut anon = LayoutBox::new(BoxKind::AnonymousBlock, None, style);
        anon.formatting = Formatting::Inline;
        let mut items = Vec::new();
        let mut atomics = Vec::new();
        flatten_inline(run, parent_style_node(out), &mut items, &mut atomics);
        anon.items = items;
        anon.children = atomics;
        out.push(anon);
    }

    // grid/flex hook
    fn finish_flex(&self, b: &mut LayoutBox, children: Vec<LayoutBox>) {
        self.finish_container(b, children, true);
        b.formatting = Formatting::Flex;
        for c in &mut b.children {
            if c.flow.is_in_flow() || c.flow.is_float() {
                // Floats do not float inside flex containers.
                c.flow = Flow::InFlow;
                c.bfc_root = true;
            }
        }
    }

    // grid/flex hook
    fn finish_grid(&self, b: &mut LayoutBox, children: Vec<LayoutBox>) {
        self.finish_container(b, children, true);
        b.formatting = Formatting::Grid;
        for c in &mut b.children {
            if c.flow.is_in_flow() || c.flow.is_float() {
                c.flow = Flow::InFlow;
                c.bfc_root = true;
            }
        }
    }

    fn finish_table(&self, b: &mut LayoutBox, children: Vec<LayoutBox>) {
        // Children: rows, row groups (blocks containing rows), captions and
        // stray content. Stray content is wrapped in an anonymous row/cell.
        let mut out = Vec::new();
        let mut stray: Vec<LayoutBox> = Vec::new();
        for c in children {
            let is_group = c.kind == BoxKind::Block
                && matches!(
                    c.style.display,
                    Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup
                );
            let is_caption = c.style.display == Display::TableCaption;
            if c.kind == BoxKind::TableRow || is_group || is_caption || !c.flow.is_in_flow() {
                self.flush_stray(&mut stray, &mut out, &b.style);
                out.push(c);
            } else if c.kind == BoxKind::Text && c.text.as_deref().map(|t| t.trim().is_empty()).unwrap_or(true) {
                // whitespace between rows
            } else {
                stray.push(c);
            }
        }
        self.flush_stray(&mut stray, &mut out, &b.style);
        b.children = out;
        b.formatting = Formatting::Table;
        b.bfc_root = true;
    }

    fn flush_stray(&self, stray: &mut Vec<LayoutBox>, out: &mut Vec<LayoutBox>, table_style: &Rc<ComputedStyle>) {
        if stray.is_empty() {
            return;
        }
        let kids = std::mem::take(stray);
        let style = Rc::new(ComputedStyle::inherit_from(table_style));
        let mut cell = LayoutBox::new(BoxKind::TableCell, None, style.clone());
        cell.bfc_root = true;
        self.finish_container(&mut cell, kids, false);
        let mut row = LayoutBox::new(BoxKind::TableRow, None, style);
        row.formatting = Formatting::TableRow;
        row.children = vec![cell];
        out.push(row);
    }

    fn finish_row(&self, b: &mut LayoutBox, children: Vec<LayoutBox>) {
        let mut out = Vec::new();
        let mut stray: Vec<LayoutBox> = Vec::new();
        for c in children {
            if c.kind == BoxKind::TableCell || !c.flow.is_in_flow() {
                if !stray.is_empty() {
                    let kids = std::mem::take(&mut stray);
                    out.push(self.anon_cell(kids, &b.style));
                }
                out.push(c);
            } else if c.kind == BoxKind::Text && c.text.as_deref().map(|t| t.trim().is_empty()).unwrap_or(true) {
            } else {
                stray.push(c);
            }
        }
        if !stray.is_empty() {
            let kids = std::mem::take(&mut stray);
            out.push(self.anon_cell(kids, &b.style));
        }
        b.children = out;
        b.formatting = Formatting::TableRow;
    }

    fn anon_cell(&self, kids: Vec<LayoutBox>, row_style: &Rc<ComputedStyle>) -> LayoutBox {
        let style = Rc::new(ComputedStyle::inherit_from(row_style));
        let mut cell = LayoutBox::new(BoxKind::TableCell, None, style);
        cell.bfc_root = true;
        self.finish_container(&mut cell, kids, false);
        cell
    }
}

fn parent_style_node(_out: &[LayoutBox]) -> Option<NodeId> {
    None
}

/// Flatten an inline subtree into `items`, moving atomic boxes to `atomics`.
fn flatten_inline(children: Vec<LayoutBox>, container_node: Option<NodeId>, items: &mut Vec<InlineItem>, atomics: &mut Vec<LayoutBox>) {
    for c in children {
        flatten_one(c, container_node, items, atomics);
    }
}

fn flatten_one(mut c: LayoutBox, elem: Option<NodeId>, items: &mut Vec<InlineItem>, atomics: &mut Vec<LayoutBox>) {
    match c.flow {
        Flow::FloatLeft | Flow::FloatRight => {
            atomics.push(c);
            items.push(InlineItem::Float(atomics.len() - 1));
            return;
        }
        Flow::Absolute => {
            atomics.push(c);
            items.push(InlineItem::Absolute(atomics.len() - 1));
            return;
        }
        Flow::InFlow => {}
    }
    match c.kind {
        BoxKind::Text => {
            if let Some(t) = c.text.take() {
                items.push(InlineItem::Text {
                    text: t,
                    style: c.style.clone(),
                    node: c.node,
                    elem,
                });
            }
        }
        BoxKind::LineBreak => items.push(InlineItem::Break),
        BoxKind::Inline => {
            let node = c.node;
            items.push(InlineItem::Open {
                node,
                style: c.style.clone(),
            });
            let kids = std::mem::take(&mut c.children);
            flatten_inline(kids, node.or(elem), items, atomics);
            items.push(InlineItem::Close);
        }
        _ => {
            // Atomic inline-level box (inline-block, image, table-as-inline...)
            atomics.push(c);
            items.push(InlineItem::Atomic(atomics.len() - 1));
        }
    }
}

fn parse_dimension_attr(v: &str) -> Option<f32> {
    let v = v.trim();
    if v.ends_with('%') {
        return None;
    }
    let num: String = v.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    num.parse::<f32>().ok().filter(|n| *n >= 0.0)
}

/// Marker text for a list item: bullet or counter (with trailing ". ").
fn list_marker_text(doc: &Document, node: NodeId, style: &ComputedStyle) -> Option<Rc<str>> {
    let t = match style.list_style_type {
        ListStyleType::None => return None,
        ListStyleType::Disc => return Some(Rc::from("•")),
        ListStyleType::Circle => return Some(Rc::from("◦")),
        ListStyleType::Square => return Some(Rc::from("▪")),
        other => other,
    };
    // Count previous list-item siblings.
    let parent = doc.parent(node);
    let (start, reversed) = parent
        .and_then(|p| doc.element(p))
        .map(|e| {
            (
                e.attr("start").and_then(|s| s.trim().parse::<i64>().ok()),
                e.attr("reversed").is_some(),
            )
        })
        .unwrap_or((None, false));
    let mut index: i64 = 0;
    let mut value_override: Option<i64> = None;
    if let Some(p) = parent {
        let mut count_total = 0i64;
        for sib in doc.child_elements(p) {
            if doc.is_element(sib, "li") {
                count_total += 1;
            }
        }
        let mut n = 0i64;
        let mut current = start.unwrap_or(if reversed { count_total } else { 1 });
        for sib in doc.child_elements(p) {
            if !doc.is_element(sib, "li") {
                continue;
            }
            if let Some(v) = doc.attr(sib, "value").and_then(|v| v.trim().parse::<i64>().ok()) {
                current = v;
            }
            n += 1;
            if sib == node {
                index = n;
                value_override = Some(current);
                break;
            }
            if reversed {
                current -= 1;
            } else {
                current += 1;
            }
        }
    }
    let number = value_override.unwrap_or(index.max(1));
    let s = match t {
        ListStyleType::Decimal => number.to_string(),
        ListStyleType::DecimalLeadingZero => format!("{number:02}"),
        ListStyleType::LowerAlpha => crate::counters::alpha(number, false),
        ListStyleType::UpperAlpha => crate::counters::alpha(number, true),
        ListStyleType::LowerRoman => crate::counters::roman(number).to_lowercase(),
        ListStyleType::UpperRoman => crate::counters::roman(number),
        _ => number.to_string(),
    };
    Some(Rc::from(format!("{s}.").as_str()))
}


