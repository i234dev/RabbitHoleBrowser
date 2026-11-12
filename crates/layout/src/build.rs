//! Box tree construction from the styled DOM.

use crate::counters::{format_counter, Counters, Quotes};
use crate::tree::{BoxKind, Flow, Formatting, InlineItem, LayoutBox};
use crate::{pseudo_style, LayoutContext};
use rabbithole_css::PseudoElement;
use rabbithole_dom::{Document, NodeData, NodeId};
use rabbithole_style::{Color, ComputedStyle, Content, ContentItem, Display, Float, ListStyleType, Position};
use std::cell::RefCell;
use std::rc::Rc;

const REPLACED: &[&str] = &["img", "svg", "video", "canvas", "iframe", "embed", "object", "audio"];

pub(crate) struct Builder<'a> {
    ctx: &'a LayoutContext<'a>,
    counters: RefCell<Counters>,
    quotes: RefCell<Quotes>,
    /// Boxes of the previous tree that may be reused.
    reuse: Option<&'a crate::incremental::Reuse>,
    /// Counters or quotes were used (document-order state).
    used_counters: std::cell::Cell<bool>,
    /// Number of counter / quote operations so far.
    ops: std::cell::Cell<u64>,
    /// Record counter state around element subtrees (for reuse).
    snapshot: bool,
}

/// Counter and quote state around a box's subtree.
#[derive(Debug)]
pub(crate) struct CounterIo {
    before: (Counters, Quotes),
    after: (Counters, Quotes),
}

impl<'a> Builder<'a> {
    pub fn new(ctx: &'a LayoutContext<'a>, reuse: Option<&'a crate::incremental::Reuse>) -> Self {
        Builder {
            ctx,
            counters: RefCell::new(Counters::new()),
            quotes: RefCell::new(Quotes::default()),
            reuse,
            used_counters: std::cell::Cell::new(false),
            ops: std::cell::Cell::new(0),
            snapshot: reuse.map(|r| r.uses_counters).unwrap_or(false),
        }
    }

    fn counter_state(&self) -> (Counters, Quotes) {
        (self.counters.borrow().clone(), self.quotes.borrow().clone())
    }

    /// Did building use CSS counters or quotes?
    pub fn uses_counters(&self) -> bool {
        self.used_counters.get()
    }

    /// Apply `counter-reset` / `counter-increment` of a style.
    fn apply_counters(&self, style: &ComputedStyle) {
        if !style.counter_reset.is_empty() || !style.counter_increment.is_empty() {
            self.used_counters.set(true);
            self.ops.set(self.ops.get() + 1);
        }
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
        if items.iter().any(|i| !matches!(i, ContentItem::String(_) | ContentItem::Attr(_) | ContentItem::Url(_))) {
            self.used_counters.set(true);
            self.ops.set(self.ops.get() + 1);
        }
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
        if let (Some(r), Some(h)) = (self.reuse, html) {
            r.open(h);
        }
        let mut root = LayoutBox::new(BoxKind::Block, html, style);
        root.bfc_root = true;
        let children = match html {
            Some(h) => self.build_children(h, &root.style, false),
            None => Vec::new(),
        };
        let children = self.fix_misparented(children, &root.style, false);
        self.finish_container(&mut root, children, false);
        root
    }

    /// Build the boxes for the children of `node`.
    fn build_children(&self, node: NodeId, parent_style: &Rc<ComputedStyle>, in_table_row: bool) -> Vec<LayoutBox> {
        let doc = self.doc();
        let mut out = Vec::new();
        // A closed `<details>` renders only its summary: the rest sits in a
        // hidden slot of its shadow tree, so no author `display` can show it.
        let closed_details = doc.is_element(node, "details") && doc.attr(node, "open").is_none() && doc.shadow_root(node).is_none();
        let summary = if closed_details { doc.child_elements(node).find(|&c| doc.is_element(c, "summary")) } else { None };
        // Flat tree: shadow trees replace their host's children and slots
        // render their assigned nodes.
        for child in doc.flat_children(node) {
            if closed_details && Some(child) != summary {
                continue;
            }
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

    fn build_element(&self, node: NodeId, _parent_style: &Rc<ComputedStyle>, in_table_row: bool, out: &mut Vec<LayoutBox>) {
        let doc = self.doc();
        let style = match self.ctx.styles.get(node) {
            Some(s) => s.clone(),
            None => return,
        };
        if style.display == Display::None {
            return;
        }
        if style.display == Display::Contents {
            if let Some(r) = self.reuse {
                r.open(node);
            }
            out.extend(self.build_children(node, &style, in_table_row));
            return;
        }
        if matches!(style.display, Display::TableColumn | Display::TableColumnGroup) {
            // Columns only matter inside tables (widths, backgrounds); the
            // fix-up drops them elsewhere. They have no borders or padding.
            let mut b = LayoutBox::new(BoxKind::Block, Some(node), Rc::new(without_box_edges(&style)));
            b.formatting = Formatting::None;
            if style.display == Display::TableColumnGroup {
                for c in doc.child_elements(node) {
                    if let Some(cs) = self.ctx.styles.get(c) {
                        if cs.display == Display::TableColumn {
                            let mut col = LayoutBox::new(BoxKind::Block, Some(c), Rc::new(without_box_edges(cs)));
                            col.formatting = Formatting::None;
                            b.children.push(col);
                        }
                    }
                }
            }
            out.push(b);
            return;
        }
        if let Some(r) = self.reuse {
            if let Some(b) = r.take(node, &style) {
                if !b.counter_touch {
                    out.push(b);
                    return;
                }
                // The subtree uses counters: reusable when it would see the
                // same counter state as when it was built.
                let io = b.counter_io.clone();
                if let Some(io) = io.filter(|io| io.before.0 == *self.counters.borrow() && io.before.1 == *self.quotes.borrow()) {
                    *self.counters.borrow_mut() = io.after.0.clone();
                    *self.quotes.borrow_mut() = io.after.1.clone();
                    self.ops.set(self.ops.get() + 1);
                    self.used_counters.set(true);
                    out.push(b);
                    return;
                }
                r.give_back(b);
            }
        }
        let ops0 = self.ops.get();
        if crate::incremental::debug_timing() {
            crate::incremental::BUILD_STATS.with(|c| c.set(c.get() + 1));
        }
        let before = if self.snapshot { Some(self.counter_state()) } else { None };
        let len0 = out.len();
        self.build_element_inner(node, &style, in_table_row, out);
        // Remember the computed style the box was built from (its own
        // style may be adjusted: table wrappers, rows, collapsed borders).
        for b in &mut out[len0..] {
            b.built_from = Some(style.clone());
        }
        if self.ops.get() != ops0 {
            let io = before.map(|before| Rc::new(CounterIo { before, after: self.counter_state() }));
            for b in &mut out[len0..] {
                b.counter_touch = true;
                b.counter_io = io.clone();
            }
        }
    }

    /// Build a fresh box for an element (after the reuse check).
    fn build_element_inner(&self, node: NodeId, style: &Rc<ComputedStyle>, _in_table_row: bool, out: &mut Vec<LayoutBox>) {
        let doc = self.doc();
        let style = style.clone();
        let tag = doc.tag(node).unwrap_or("");
        self.apply_counters(&style);

        // Replaced elements. An `<object>` whose resource cannot be shown
        // (unknown type, failed load, plugin content) renders its fallback
        // content instead, as an ordinary element (HTML §4.8.7).
        let object_fallback = tag == "object" && !self.ctx.images.get(&node).map(|i| !i.broken).unwrap_or(false);
        if REPLACED.contains(&tag) && !object_fallback {
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
                Display::TableCell => BoxKind::TableCell,
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
        let contained = !matches!(style.display, Display::Inline | Display::Contents) && !matches!(tag, "html" | "body");
        b.clips = (style.overflow_x.clips() || style.overflow_y.clips() || (style.contain_paint && contained)) && !matches!(tag, "html" | "body");
        if contained && (style.contain_paint || style.contain_layout) {
            b.bfc_root = true;
        }
        if matches!(tag, "td" | "th") {
            if let Some(cs) = doc.attr(node, "colspan").and_then(|v| v.trim().parse::<u32>().ok()) {
                b.colspan = cs.clamp(1, 1000);
            }
        }
        if kind == BoxKind::TableRow || matches!(style.display, Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup) && b.flow.is_in_flow() {
            // Rows and row groups have no borders or padding (CSS 2.1 §17.6.1;
            // collapsed borders read them from the cascaded style).
            b.style = Rc::new(without_box_edges(&style));
        }
        if style.display == Display::ListItem {
            b.marker_text = list_marker_text(doc, node, &style);
        }

        // Form controls get synthesized text content. Generated content
        // (`::before` / `::after`) wraps the element's children; counters
        // created by the children live in this element's scope.
        self.counters.borrow_mut().enter();
        // Text fields, text areas and menus show no generated content (like
        // Blink: only buttons, checkboxes and radios render it).
        let no_generated = match tag {
            "textarea" | "select" => true,
            "input" => !matches!(input_type(doc, node).as_str(), "checkbox" | "radio"),
            _ => false,
        };
        let no_generated = no_generated || style.content_visibility_hidden;
        let before = if no_generated { None } else { self.build_pseudo(node, PseudoElement::Before, kind == BoxKind::TableRow) };
        let mut children = match tag {
            "input" => self.build_input(node, &style),
            "select" => {
                let (kids, min_w) = self.build_select(node, &style);
                if let Some(w) = min_w {
                    // Wide enough for the longest option (a menu sizes to
                    // its options, not to the selected one).
                    let mut s = (*style).clone();
                    s.min_width = rabbithole_style::Length::Px(match s.min_width {
                        rabbithole_style::Length::Px(m) => m.max(w),
                        _ => w,
                    });
                    b.style = Rc::new(s);
                }
                kids
            }
            "textarea" => {
                // The edited ("dirty") value, else the default value (the
                // text content); the placeholder when empty.
                let value = doc.control_value(node).map(str::to_string).unwrap_or_else(|| doc.text_content(node));
                let mut t;
                if value.is_empty() {
                    let ph = doc.attr(node, "placeholder").unwrap_or("");
                    t = LayoutBox::new(BoxKind::Text, None, self.placeholder_style(node, &style));
                    t.text = Some(Rc::from(if ph.is_empty() { "\u{200B}" } else { ph }));
                } else {
                    t = LayoutBox::new(BoxKind::Text, None, style.clone());
                    t.text = Some(Rc::from(value.as_str()));
                }
                vec![t]
            }
            // `content-visibility: hidden`: no contents at all.
            _ if style.content_visibility_hidden => Vec::new(),
            _ => self.build_children(node, &style, kind == BoxKind::TableRow),
        };
        let after = if no_generated { None } else { self.build_pseudo(node, PseudoElement::After, kind == BoxKind::TableRow) };
        self.counters.borrow_mut().leave();
        if let Some(bf) = before {
            children.insert(0, bf);
        }
        if let Some(af) = after {
            children.push(af);
        }
        let is_table_part = matches!(
            style.display,
            Display::Table | Display::InlineTable | Display::TableRow | Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup
        ) && !(out_of_flow || floated) || matches!(style.display, Display::Table);
        if !is_table_part {
            children = self.fix_misparented(children, &style, kind == BoxKind::Inline);
        }

        if kind == BoxKind::Inline {
            // Inline boxes keep their inline-level children as a tree. A
            // block-level child splits the inline (CSS 2.1 §9.2.1.1): the
            // inline content before and after it become separate inline
            // boxes of the same element and the block becomes a sibling in
            // the parent, which then wraps the inline runs in anonymous
            // blocks.
            let splits = children.iter().any(|c| matches!(c.kind, BoxKind::Block | BoxKind::Table) && c.flow.is_in_flow());
            if !splits {
                b.children = children;
                b.formatting = Formatting::Inline;
                out.push(b);
                return;
            }
            let mut run: Vec<LayoutBox> = Vec::new();
            let mut parts: Vec<LayoutBox> = Vec::new();
            let make_inline = |kids: Vec<LayoutBox>| {
                let mut part = LayoutBox::new(BoxKind::Inline, b.node, b.style.clone());
                part.flow = b.flow;
                part.children = kids;
                part.formatting = Formatting::Inline;
                // Several boxes for one element: never reused as one.
                part.counter_touch = true;
                part
            };
            for c in children {
                if matches!(c.kind, BoxKind::Block | BoxKind::Table) && c.flow.is_in_flow() {
                    if !run.is_empty() || parts.is_empty() {
                        parts.push(make_inline(std::mem::take(&mut run)));
                    }
                    let mut c = c;
                    c.counter_touch = true;
                    parts.push(c);
                } else {
                    run.push(c);
                }
            }
            parts.push(make_inline(run));
            out.extend(parts);
            return;
        }

        // grid/flex hook: flex and grid containers blockify their children.
        if matches!(style.display, Display::Table | Display::InlineTable) {
            let captions = self.finish_table(&mut b, children);
            out.push(self.wrap_table(b, captions));
            return;
        }
        match kind {
            BoxKind::TableRow => self.finish_row(&mut b, children),
            _ => {
                if matches!(style.display, Display::Flex | Display::InlineFlex) {
                    self.finish_flex(&mut b, children);
                } else if matches!(style.display, Display::Grid | Display::InlineGrid) {
                    self.finish_grid(&mut b, children);
                } else if crate::table::is_row_group(&b) {
                    self.finish_row_group(&mut b, children);
                } else {
                    self.finish_container(&mut b, children, false);
                    // An empty list item with an inside marker: the marker
                    // is its inline content.
                    if b.marker_text.is_some() && b.children.is_empty() && b.items.is_empty() && style.list_style_position == rabbithole_style::ListStylePosition::Inside {
                        b.formatting = Formatting::Inline;
                    }
                }
            }
        }
        out.push(b);
    }

    fn build_replaced(&self, node: NodeId, tag: &str, style: &Rc<ComputedStyle>) -> Option<LayoutBox> {
        let doc = self.doc();
        // `<video>` shows its poster image (stored under the node too).
        let image = if matches!(tag, "img" | "svg" | "input" | "video" | "canvas" | "object" | "iframe") { self.ctx.images.get(&node).cloned() } else { None };
        let attr_w = doc.attr(node, "width").and_then(parse_dimension_attr);
        let attr_h = doc.attr(node, "height").and_then(parse_dimension_attr);
        let broken = image.as_ref().map(|i| i.broken).unwrap_or(false);
        if tag == "img" && broken && attr_w.is_none() && attr_h.is_none() && style.width.is_auto() && style.height.is_auto() {
            // Failed image without a size: render the alt text inline.
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
        if tag == "img" && broken && doc.attr(node, "alt").is_none() && b.attr_size == (None, None) {
            // No alt at all: a 16×16 broken-image icon.
            b.attr_size = (Some(16.0), Some(16.0));
        }
        b.ratio_only = tag == "svg" && attr_w.is_none() && attr_h.is_none() && b.image.is_some() && (doc.attr(node, "viewBox").is_some() || doc.attr(node, "viewbox").is_some());
        let has_media = b.image.as_ref().map(|i| !i.broken).unwrap_or(false);
        if matches!(tag, "iframe" | "video" | "object" | "embed" | "canvas") && !has_media {
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

    /// Style of the placeholder text of a text field: its `::placeholder`
    /// style (carried in the marker slot for `input` / `textarea`), else
    /// the field's style.
    fn placeholder_style(&self, node: NodeId, style: &Rc<ComputedStyle>) -> Rc<ComputedStyle> {
        self.ctx.styles.pseudo(node, PseudoElement::Marker).cloned().unwrap_or_else(|| style.clone())
    }

    fn text_box(style: Rc<ComputedStyle>, text: &str) -> LayoutBox {
        let mut t = LayoutBox::new(BoxKind::Text, None, style);
        t.text = Some(Rc::from(text));
        t
    }

    fn build_input(&self, node: NodeId, style: &Rc<ComputedStyle>) -> Vec<LayoutBox> {
        let doc = self.doc();
        let ty = input_type(doc, node);
        let appearance_none = style.appearance == rabbithole_style::Appearance::None;
        let text = match ty.as_str() {
            "submit" => doc.attr(node, "value").unwrap_or("Submit").to_string(),
            "reset" => doc.attr(node, "value").unwrap_or("Reset").to_string(),
            "button" => doc.attr(node, "value").unwrap_or("").to_string(),
            "checkbox" | "radio" | "range" | "color" | "image" | "hidden" => return Vec::new(),
            "file" => {
                // "Choose File" button and the selection label.
                let mut s = ComputedStyle::inherit_from(style);
                s.display = Display::InlineBlock;
                s.white_space = rabbithole_style::WhiteSpace::Pre;
                if !appearance_none {
                    let c = Color::rgb(118, 118, 118);
                    s.border_width = rabbithole_style::Edges::all(1.0);
                    s.border_style = rabbithole_style::Edges::all(rabbithole_style::BorderStyle::Solid);
                    s.border_color = rabbithole_style::Edges::all(c);
                    s.background_color = Color::rgb(239, 239, 239);
                    s.border_radius = 2.0;
                    s.border_radii = [(rabbithole_style::Length::Px(2.0), rabbithole_style::Length::Px(2.0)); 4];
                    s.padding = rabbithole_style::Edges {
                        top: rabbithole_style::Length::Px(1.0),
                        right: rabbithole_style::Length::Px(6.0),
                        bottom: rabbithole_style::Length::Px(1.0),
                        left: rabbithole_style::Length::Px(6.0),
                    };
                }
                s.margin.right = rabbithole_style::Length::Px(4.0);
                // `::file-selector-button` (the UA sheet gives it the look
                // of a button).
                if let Some(ps) = self.ctx.styles.pseudo(node, PseudoElement::Marker) {
                    s = (**ps).clone();
                    if !matches!(s.display, Display::None | Display::InlineFlex | Display::InlineGrid) {
                        s.display = Display::InlineBlock;
                    }
                }
                if s.display == Display::None {
                    let label = if doc.attr(node, "multiple").is_some() { "No files chosen" } else { "No file chosen" };
                    return vec![Self::text_box(style.clone(), label)];
                }
                let s = Rc::new(s);
                let mut btn = LayoutBox::new(BoxKind::InlineBlock, None, s.clone());
                btn.bfc_root = true;
                let multiple = doc.attr(node, "multiple").is_some();
                self.finish_container(&mut btn, vec![Self::text_box(s, if multiple { "Choose Files" } else { "Choose File" })], false);
                let label = if multiple { "No files chosen" } else { "No file chosen" };
                return vec![btn, Self::text_box(style.clone(), label)];
            }
            "date" | "month" | "week" | "time" | "datetime-local" => {
                let v = doc.control_value(node).unwrap_or("").trim().to_string();
                format_date_value(&ty, &v)
            }
            "password" => doc.control_value(node).map(|v| "\u{2022}".repeat(v.chars().count())).unwrap_or_default(),
            _ => doc.control_value(node).unwrap_or("").to_string(),
        };
        if text.is_empty() {
            if matches!(ty.as_str(), "submit" | "reset" | "button") {
                // An empty button keeps the height of a line.
                return vec![Self::text_box(style.clone(), "\u{200B}")];
            }
            // An empty field keeps one line of height (and its baseline);
            // the placeholder is shown in its own style.
            let ph = doc.attr(node, "placeholder").map(|p| p.replace(['\n', '\r'], "")).unwrap_or_default();
            if !ph.is_empty() {
                return vec![Self::text_box(self.placeholder_style(node, style), &ph)];
            }
            return vec![Self::text_box(style.clone(), "\u{200B}")];
        }
        vec![Self::text_box(style.clone(), &text)]
    }

    /// The menu's children and, for a drop-down, the width (border box
    /// when `box-sizing: border-box`, else content box) fitting its
    /// longest option and the arrow.
    fn build_select(&self, node: NodeId, style: &Rc<ComputedStyle>) -> (Vec<LayoutBox>, Option<f32>) {
        let doc = self.doc();
        let options: Vec<NodeId> = doc.descendants(node).filter(|&n| doc.is_element(n, "option")).collect();
        let label = |o: NodeId| -> String {
            let t = doc.attr(o, "label").map(str::to_string).unwrap_or_else(|| doc.text_content(o));
            t.split_whitespace().collect::<Vec<_>>().join(" ")
        };
        let multiple = doc.attr(node, "multiple").is_some();
        let size = doc.attr(node, "size").and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0);
        if multiple || size > 1 {
            // List box: every option on its own row, selected ones
            // highlighted.
            let mut rows = Vec::new();
            for &o in &options {
                let mut s = ComputedStyle::inherit_from(style);
                s.display = Display::Block;
                s.white_space = rabbithole_style::WhiteSpace::Pre;
                s.padding = rabbithole_style::Edges {
                    top: rabbithole_style::Length::ZERO,
                    right: rabbithole_style::Length::Px(2.0),
                    bottom: rabbithole_style::Length::Px(1.0),
                    left: rabbithole_style::Length::Px(2.0),
                };
                if doc.attr(o, "selected").is_some() {
                    s.background_color = Color::rgb(206, 206, 206);
                }
                if doc.attr(o, "disabled").is_some() {
                    s.color = Color::rgba(16, 16, 16, 77);
                }
                let s = Rc::new(s);
                let mut row = LayoutBox::new(BoxKind::Block, None, s.clone());
                let text = label(o);
                self.finish_container(&mut row, vec![Self::text_box(s, if text.is_empty() { "\u{200B}" } else { &text })], false);
                rows.push(row);
            }
            if !style.width.is_auto() {
                return (rows, None);
            }
            // Wide enough for the longest option and a scroll bar.
            let font = self.ctx.fonts.select_family(&style.font_family_list, style.font_family, style.font_weight, style.is_italic());
            let widest = options.iter().map(|&o| self.ctx.fonts.measure(font, style.font_size, &label(o))).fold(0.0f32, f32::max);
            let mut w = widest.ceil() + 4.0 + 16.0;
            if style.box_sizing == rabbithole_style::BoxSizing::BorderBox {
                let px = |l: rabbithole_style::Length| if let rabbithole_style::Length::Px(v) = l { v } else { 0.0 };
                w += px(style.padding.left) + px(style.padding.right) + style.border_width.left + style.border_width.right;
            }
            return (rows, Some(w));
        }
        let chosen = options.iter().copied().rev().find(|&o| doc.attr(o, "selected").is_some()).or_else(|| options.first().copied());
        let text = chosen.map(label).unwrap_or_default();
        let t = Self::text_box(style.clone(), if text.is_empty() { "\u{200B}" } else { &text });
        if !style.width.is_auto() {
            return (vec![t], None);
        }
        let font = self.ctx.fonts.select_family(&style.font_family_list, style.font_family, style.font_weight, style.is_italic());
        let widest = options.iter().map(|&o| self.ctx.fonts.measure(font, style.font_size, &label(o))).fold(0.0f32, f32::max);
        let arrow = if style.appearance == rabbithole_style::Appearance::None { 0.0 } else { SELECT_ARROW_WIDTH };
        let mut w = widest.ceil() + arrow;
        if style.box_sizing == rabbithole_style::BoxSizing::BorderBox {
            let px = |l: rabbithole_style::Length| if let rabbithole_style::Length::Px(v) = l { v } else { 0.0 };
            w += px(style.padding.left) + px(style.padding.right) + style.border_width.left + style.border_width.right;
        }
        (vec![t], Some(w))
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
                        blk.counter_touch = c.counter_touch;
                        blk.built_from = c.built_from.take();
                        blk.counter_io = c.counter_io.take();
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
        // Items are laid out and painted in order-modified document order.
        b.children.sort_by_key(|c| c.style.order);
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
        b.children.sort_by_key(|c| c.style.order);
        for c in &mut b.children {
            if c.flow.is_in_flow() || c.flow.is_float() {
                c.flow = Flow::InFlow;
                c.bfc_root = true;
            }
        }
    }

    /// Table box fix-up (CSS 2.1 §17.2.1): rows, row groups and columns
    /// stay; runs of anything else get an anonymous row (with anonymous
    /// cells). Captions are returned for the wrapper box.
    fn finish_table(&self, b: &mut LayoutBox, children: Vec<LayoutBox>) -> Vec<LayoutBox> {
        let mut out = Vec::new();
        let mut captions = Vec::new();
        let mut stray: Vec<LayoutBox> = Vec::new();
        for c in children {
            if crate::table::is_caption(&c) {
                self.flush_stray_row(&mut stray, &mut out, &b.style);
                captions.push(c);
            } else if (c.kind == BoxKind::TableRow && c.flow.is_in_flow()) || crate::table::is_row_group(&c) || crate::table::is_column(&c) {
                self.flush_stray_row(&mut stray, &mut out, &b.style);
                out.push(c);
            } else {
                stray.push(c);
            }
        }
        self.flush_stray_row(&mut stray, &mut out, &b.style);
        b.children = out;
        b.formatting = Formatting::Table;
        b.bfc_root = true;
        captions
    }

    /// Row group fix-up: runs of non-rows get an anonymous row.
    fn finish_row_group(&self, b: &mut LayoutBox, children: Vec<LayoutBox>) {
        let mut out = Vec::new();
        let mut stray: Vec<LayoutBox> = Vec::new();
        for c in children {
            if c.kind == BoxKind::TableRow && c.flow.is_in_flow() {
                self.flush_stray_row(&mut stray, &mut out, &b.style);
                out.push(c);
            } else if crate::table::is_column(&c) {
            } else {
                stray.push(c);
            }
        }
        self.flush_stray_row(&mut stray, &mut out, &b.style);
        b.children = out;
        b.formatting = Formatting::Block;
    }

    /// Wrap a run of non-row children of a table / row group in an
    /// anonymous row (dropping white space only runs).
    fn flush_stray_row(&self, stray: &mut Vec<LayoutBox>, out: &mut Vec<LayoutBox>, parent_style: &Rc<ComputedStyle>) {
        if stray.is_empty() {
            return;
        }
        let kids = std::mem::take(stray);
        if kids.iter().all(is_whitespace_text) {
            return;
        }
        let style = Rc::new(anonymous_style(parent_style, Display::TableRow));
        let mut row = LayoutBox::new(BoxKind::TableRow, None, style);
        self.finish_row(&mut row, kids);
        out.push(row);
    }

    /// Row fix-up: runs of non-cells get an anonymous cell.
    fn finish_row(&self, b: &mut LayoutBox, children: Vec<LayoutBox>) {
        let mut out = Vec::new();
        let mut stray: Vec<LayoutBox> = Vec::new();
        for c in children {
            if c.kind == BoxKind::TableCell && c.flow.is_in_flow() {
                if !stray.is_empty() {
                    let kids = std::mem::take(&mut stray);
                    if !kids.iter().all(is_whitespace_text) {
                        out.push(self.anon_cell(kids, &b.style));
                    }
                }
                out.push(c);
            } else if crate::table::is_column(&c) {
            } else {
                stray.push(c);
            }
        }
        if !stray.is_empty() && !stray.iter().all(is_whitespace_text) {
            let kids = std::mem::take(&mut stray);
            out.push(self.anon_cell(kids, &b.style));
        }
        b.children = out;
        b.formatting = Formatting::TableRow;
    }

    fn anon_cell(&self, kids: Vec<LayoutBox>, row_style: &Rc<ComputedStyle>) -> LayoutBox {
        let style = Rc::new(anonymous_style(row_style, Display::TableCell));
        let mut cell = LayoutBox::new(BoxKind::TableCell, None, style);
        cell.bfc_root = true;
        self.finish_container(&mut cell, kids, false);
        cell
    }

    /// Generate missing table parents (CSS 2.1 §17.2.1 step 2) for table
    /// parts outside a table: consecutive cells get an anonymous row and
    /// runs of rows / row groups / captions an anonymous table (inline
    /// table inside inline boxes). Stray columns are dropped.
    pub(crate) fn fix_misparented(&self, children: Vec<LayoutBox>, parent_style: &Rc<ComputedStyle>, inline_parent: bool) -> Vec<LayoutBox> {
        let internal = |c: &LayoutBox| {
            c.flow.is_in_flow()
                && (c.kind == BoxKind::TableCell
                    || c.kind == BoxKind::TableRow
                    || crate::table::is_row_group(c)
                    || crate::table::is_caption(c)
                    || crate::table::is_column(c))
        };
        if !children.iter().any(|c| internal(c)) {
            return children;
        }
        let mut out = Vec::new();
        let mut run: Vec<LayoutBox> = Vec::new();
        let mut pending_ws: Vec<LayoutBox> = Vec::new();
        let flush = |run: &mut Vec<LayoutBox>, out: &mut Vec<LayoutBox>| {
            if run.is_empty() {
                return;
            }
            let kids = std::mem::take(run);
            let display = if inline_parent { Display::InlineTable } else { Display::Table };
            let style = Rc::new(anonymous_style(parent_style, display));
            let kind = if inline_parent { BoxKind::InlineBlock } else { BoxKind::Table };
            let mut table = LayoutBox::new(kind, None, style.clone());
            table.bfc_root = true;
            // Consecutive cells first get an anonymous row.
            let mut grouped: Vec<LayoutBox> = Vec::new();
            let mut cells: Vec<LayoutBox> = Vec::new();
            for k in kids {
                if k.kind == BoxKind::TableCell {
                    cells.push(k);
                } else {
                    if !cells.is_empty() {
                        let mut row = LayoutBox::new(BoxKind::TableRow, None, Rc::new(anonymous_style(&style, Display::TableRow)));
                        self.finish_row(&mut row, std::mem::take(&mut cells));
                        grouped.push(row);
                    }
                    if !crate::table::is_column(&k) {
                        grouped.push(k);
                    }
                }
            }
            if !cells.is_empty() {
                let mut row = LayoutBox::new(BoxKind::TableRow, None, Rc::new(anonymous_style(&style, Display::TableRow)));
                self.finish_row(&mut row, cells);
                grouped.push(row);
            }
            let captions = self.finish_table(&mut table, grouped);
            out.push(self.wrap_table(table, captions));
        };
        for c in children {
            if internal(&c) {
                // White space between table parts is dropped.
                pending_ws.clear();
                run.push(c);
            } else if is_whitespace_text(&c) && !run.is_empty() {
                pending_ws.push(c);
            } else {
                flush(&mut run, &mut out);
                out.append(&mut pending_ws);
                out.push(c);
            }
        }
        flush(&mut run, &mut out);
        out.append(&mut pending_ws);
        out
    }

    /// Put a table with captions inside a table wrapper box (CSS 2.1
    /// §17.4): the wrapper takes the table's margins and positioning, the
    /// table keeps borders, padding and backgrounds.
    fn wrap_table(&self, mut table: LayoutBox, captions: Vec<LayoutBox>) -> LayoutBox {
        if captions.is_empty() {
            return table;
        }
        let ts = table.style.clone();
        let mut ws = (*ts).clone();
        ws.border_width = rabbithole_style::Edges::all(0.0);
        ws.border_style = rabbithole_style::Edges::all(rabbithole_style::BorderStyle::None);
        ws.padding = rabbithole_style::Edges::all(rabbithole_style::Length::ZERO);
        ws.background_color = rabbithole_gfx::Color::TRANSPARENT;
        ws.background_layers.clear();
        ws.background_image = None;
        ws.box_shadow.clear();
        ws.outline_width = 0.0;
        ws.outline_style = rabbithole_style::BorderStyle::None;
        ws.width = rabbithole_style::Length::Auto;
        ws.height = rabbithole_style::Length::Auto;
        ws.min_width = rabbithole_style::Length::Auto;
        ws.max_width = rabbithole_style::Length::Auto;
        ws.min_height = rabbithole_style::Length::Auto;
        ws.max_height = rabbithole_style::Length::Auto;
        ws.box_sizing = rabbithole_style::BoxSizing::ContentBox;
        ws.display = if ts.display == Display::InlineTable { Display::InlineBlock } else { Display::Block };
        let mut inner = (*ts).clone();
        inner.margin = rabbithole_style::Edges::all(rabbithole_style::Length::ZERO);
        inner.position = Position::Static;
        inner.inset = rabbithole_style::Edges::all(rabbithole_style::Length::Auto);
        inner.float = Float::None;
        inner.clear = rabbithole_style::Clear::None;
        inner.z_index = None;
        inner.transform.clear();
        inner.opacity = 1.0;
        let mut wrapper = LayoutBox::new(if table.kind == BoxKind::InlineBlock { BoxKind::InlineBlock } else { BoxKind::Block }, table.node, Rc::new(ws));
        wrapper.flow = table.flow;
        wrapper.bfc_root = true;
        wrapper.formatting = Formatting::TableWrapper;
        table.flow = Flow::InFlow;
        table.kind = BoxKind::Table;
        table.style = Rc::new(inner);
        let (top, bottom): (Vec<LayoutBox>, Vec<LayoutBox>) = captions.into_iter().partition(|c| !c.style.caption_side_bottom);
        wrapper.children.extend(top);
        wrapper.children.push(table);
        wrapper.children.extend(bottom);
        wrapper
    }
}

fn is_whitespace_text(c: &LayoutBox) -> bool {
    c.kind == BoxKind::Text && c.text.as_deref().map(|t| t.chars().all(|ch| matches!(ch, ' ' | '\t' | '\n' | '\r' | '\x0c'))).unwrap_or(true)
}

/// Style of an anonymous table box: inherits from the parent, with the
/// given display.
fn anonymous_style(parent: &ComputedStyle, display: Display) -> ComputedStyle {
    let mut s = ComputedStyle::inherit_from(parent);
    s.display = display;
    s
}

/// Width reserved for the drop-down arrow of a `<select>` (painted by the
/// display list at the right of the content box).
pub(crate) const SELECT_ARROW_WIDTH: f32 = 20.0;

/// The `type` of an `<input>`, lower-cased (`text` when missing or
/// unknown).
pub(crate) fn input_type(doc: &Document, node: NodeId) -> String {
    const KNOWN: &[&str] = &[
        "hidden", "text", "search", "tel", "url", "email", "password", "date", "month", "week", "time", "datetime-local", "number", "range", "color", "checkbox", "radio",
        "file", "submit", "image", "reset", "button",
    ];
    let t = doc.attr(node, "type").map(|t| t.trim().to_ascii_lowercase()).unwrap_or_default();
    if KNOWN.contains(&t.as_str()) {
        t
    } else {
        "text".into()
    }
}

/// What a date/time field shows: its value in the en-US format, or the
/// format mask when empty.
fn format_date_value(ty: &str, v: &str) -> String {
    let parts: Vec<&str> = v.split(['-', 'T', ':', 'W']).collect();
    let time = |h: &str, m: &str| -> Option<String> {
        let h: u32 = h.parse().ok()?;
        let m: u32 = m.get(..2)?.parse().ok()?;
        let (h12, ampm) = match h {
            0 => (12, "AM"),
            1..=11 => (h, "AM"),
            12 => (12, "PM"),
            _ => (h - 12, "PM"),
        };
        Some(format!("{h12:02}:{m:02} {ampm}"))
    };
    let date = |y: &str, mo: &str, d: &str| -> Option<String> {
        let (y, mo, d): (u32, u32, u32) = (y.parse().ok()?, mo.parse().ok()?, d.parse().ok()?);
        Some(format!("{mo:02}/{d:02}/{y:04}"))
    };
    let shown = match ty {
        "date" if parts.len() == 3 => date(parts[0], parts[1], parts[2]),
        "time" if parts.len() >= 2 => time(parts[0], parts[1]),
        "datetime-local" if parts.len() >= 5 => date(parts[0], parts[1], parts[2]).zip(time(parts[3], parts[4])).map(|(d, t)| format!("{d}, {t}")),
        "month" if parts.len() == 2 => {
            const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
            parts[1].parse::<usize>().ok().filter(|m| (1..=12).contains(m)).map(|m| format!("{} {}", MONTHS[m - 1], parts[0]))
        }
        "week" if v.contains("-W") => {
            let (y, w) = v.split_once("-W").unwrap_or(("", ""));
            Some(format!("Week {w}, {y}"))
        }
        _ => None,
    };
    shown.unwrap_or_else(|| {
        match ty {
            "date" => "mm/dd/yyyy",
            "time" => "--:-- --",
            "datetime-local" => "mm/dd/yyyy, --:-- --",
            "month" => "--------- ----",
            _ => "Week --, ----",
        }
        .to_string()
    })
}

/// A copy of `style` without borders and padding (rows, row groups and
/// columns cannot have them).
fn without_box_edges(style: &ComputedStyle) -> ComputedStyle {
    let mut s = style.clone();
    s.border_width = rabbithole_style::Edges::all(0.0);
    s.padding = rabbithole_style::Edges::all(rabbithole_style::Length::ZERO);
    s
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


