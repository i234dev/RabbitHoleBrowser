//! Inline formatting: line breaking, fragment placement, vertical alignment.

use crate::block::{Cb, Layouter};
use crate::float::Bfc;
use crate::text::{segment_tabs, transform, Collapser, Seg};
use crate::tree::{DecorationInfo, Fragment, FragmentKind, InlineItem, LayoutBox, LineBox};
use rabbithole_dom::NodeId;
use rabbithole_gfx::{FontId, Rect};
use rabbithole_style::{
    ComputedStyle, Direction, ListStylePosition, OverflowWrap, TextAlign, TextOverflow, VerticalAlign, Visibility, WordBreak,
};
use std::rc::Rc;

/// How a word that does not fit on the line may be broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BreakPolicy {
    /// `overflow-wrap: normal` + `word-break: normal|keep-all`: never split
    /// inside a word (it overflows).
    Never,
    /// `overflow-wrap: anywhere|break-word` / `word-break: break-word`:
    /// split only when the word does not fit on a line of its own.
    IfNeeded,
    /// `word-break: break-all`: break between any two characters.
    Anywhere,
}

fn break_policy(style: &ComputedStyle) -> BreakPolicy {
    match style.word_break {
        WordBreak::BreakAll => BreakPolicy::Anywhere,
        WordBreak::BreakWord => BreakPolicy::IfNeeded,
        _ => match style.overflow_wrap {
            OverflowWrap::Normal => BreakPolicy::Never,
            _ => BreakPolicy::IfNeeded,
        },
    }
}

/// Pseudo item index for the list marker rendered inside.
pub(crate) const MARKER_ITEM: usize = usize::MAX;

/// A measured piece of inline content.
#[derive(Debug, Clone)]
pub(crate) enum Piece {
    Word {
        text: Rc<str>,
        width: f32,
        item: usize,
        break_before: bool,
    },
    Space {
        width: f32,
        item: usize,
    },
    Open(usize),
    Close(usize),
    Atomic(usize),
    Float(usize),
    Abs(usize),
    Break,
}

/// Font metrics of a style, with the half-leading applied.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TextMetrics {
    pub asc: f32,
    pub desc: f32,
    pub above: f32,
    pub below: f32,
    pub font: FontId,
    pub size: f32,
}

pub(crate) fn item_style<'b>(items: &'b [InlineItem], container: &'b Rc<ComputedStyle>, idx: usize) -> &'b Rc<ComputedStyle> {
    if idx == MARKER_ITEM {
        return container;
    }
    match &items[idx] {
        InlineItem::Text { style, .. } | InlineItem::Open { style, .. } => style,
        _ => container,
    }
}

fn item_elem(items: &[InlineItem], idx: usize) -> Option<NodeId> {
    match items.get(idx) {
        Some(InlineItem::Text { elem, .. }) => *elem,
        Some(InlineItem::Open { node, .. }) => *node,
        _ => None,
    }
}

/// Split a word after hyphens so `foo-bar` can wrap after the dash.
fn split_hyphens(w: &str) -> Vec<(String, bool)> {
    if w.len() < 3 || !w.contains('-') {
        return vec![(w.to_string(), false)];
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = w.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        cur.push(c);
        if c == '-' && i + 1 < chars.len() && chars[i + 1].is_alphanumeric() && cur.chars().count() > 1 {
            out.push((std::mem::take(&mut cur), !out.is_empty()));
        }
    }
    if !cur.is_empty() {
        let cont = !out.is_empty();
        out.push((cur, cont));
    }
    // Mark continuations (everything after the first part).
    for (i, part) in out.iter_mut().enumerate() {
        part.1 = i > 0;
    }
    out
}

impl<'a> Layouter<'a> {
    pub(crate) fn text_metrics(&mut self, style: &ComputedStyle) -> TextMetrics {
        let font = self.font_for(style);
        let size = style.font_size;
        let m = self.ctx.fonts.metrics(font, size);
        let lh = style.line_height_px();
        let half = (lh - (m.ascent + m.descent)) / 2.0;
        TextMetrics {
            asc: m.ascent,
            desc: m.descent,
            above: m.ascent + half,
            below: m.descent + half,
            font,
            size,
        }
    }

    /// Build (and cache) the measured pieces of an inline container.
    pub(crate) fn ensure_pieces(&mut self, b: &mut LayoutBox) {
        if b.pieces.is_some() {
            return;
        }
        let pieces = self.build_pieces(&b.items, &b.style);
        b.pieces = Some(pieces);
    }

    fn build_pieces(&mut self, items: &[InlineItem], container: &Rc<ComputedStyle>) -> Vec<Piece> {
        let container_wraps = container.white_space.wraps();
        let mut col = Collapser::new();
        let mut stack: Vec<usize> = Vec::new();
        let mut pieces = Vec::new();
        let mut prev_break_opp = true;
        let mut segs = Vec::new();
        for (i, item) in items.iter().enumerate() {
            match item {
                InlineItem::Text { text, style, .. } => {
                    let font = self.font_for(style);
                    let size = style.font_size;
                    let ws = style.white_space;
                    let wraps = ws.wraps();
                    let t = transform(text, style.text_transform);
                    segs.clear();
                    segment_tabs(&t, ws, &mut col, &mut segs, style.tab_size.max(1.0) as usize);
                    let ls = style.letter_spacing;
                    for seg in segs.drain(..) {
                        match seg {
                            Seg::Word(w) => {
                                let is_space_word = w.chars().all(|c| c == ' ');
                                for (part, continuation) in split_hyphens(&w) {
                                    let width = self.measurer.width(font, size, &part) + ls * part.chars().count() as f32;
                                    let bb = wraps && (prev_break_opp || continuation);
                                    pieces.push(Piece::Word {
                                        text: Rc::from(part.as_str()),
                                        width,
                                        item: i,
                                        break_before: bb,
                                    });
                                    prev_break_opp = false;
                                }
                                prev_break_opp = is_space_word && wraps;
                            }
                            Seg::Space => {
                                let width = self.measurer.space_width(font, size) + style.word_spacing;
                                pieces.push(Piece::Space { width, item: i });
                                prev_break_opp = true;
                            }
                            Seg::Break => {
                                pieces.push(Piece::Break);
                                prev_break_opp = true;
                            }
                        }
                    }
                }
                InlineItem::Open { .. } => {
                    stack.push(i);
                    pieces.push(Piece::Open(i));
                }
                InlineItem::Close => {
                    if let Some(o) = stack.pop() {
                        pieces.push(Piece::Close(o));
                    }
                }
                InlineItem::Atomic(c) => {
                    pieces.push(Piece::Atomic(*c));
                    prev_break_opp = container_wraps;
                    // An atomic box interrupts the whitespace sequence, so a
                    // following space is not collapsed away.
                    col.prev_space = false;
                }
                InlineItem::Float(c) => pieces.push(Piece::Float(*c)),
                InlineItem::Absolute(c) => pieces.push(Piece::Abs(*c)),
                InlineItem::Break => {
                    pieces.push(Piece::Break);
                    col.prev_space = true;
                    prev_break_opp = true;
                }
            }
        }
        pieces
    }

    /// Lay out the inline content of `b`; returns the content height.
    pub(crate) fn layout_inline(&mut self, b: &mut LayoutBox, cb: &Cb, bfc: &mut Bfc) -> f32 {
        self.ensure_pieces(b);
        let mut pieces = b.pieces.take().unwrap_or_default();
        let items = std::mem::take(&mut b.items);
        let cstyle = b.style.clone();
        let mut inserted_marker = false;
        if let Some(mt) = &b.marker_text {
            if cstyle.list_style_position == ListStylePosition::Inside {
                let font = self.font_for(&cstyle);
                let text = format!("{mt} ");
                let width = self.measurer.width(font, cstyle.font_size, &text);
                pieces.insert(
                    0,
                    Piece::Word {
                        text: Rc::from(text.as_str()),
                        width,
                        item: MARKER_ITEM,
                        break_before: false,
                    },
                );
                inserted_marker = true;
            }
        }
        let content = b.content_rect;
        let strut = self.text_metrics(&cstyle);
        let indent = cstyle.text_indent.resolve_or_zero(cb.width);
        let clips = cstyle.overflow_x.clips() || cstyle.overflow_y.clips();
        let ellipsis = cstyle.text_overflow == TextOverflow::Ellipsis && clips;
        let max_lines = cstyle.line_clamp.filter(|n| *n > 0).map(|n| n as usize);
        let mut lines = Vec::new();
        let height = {
            let mut il = InlineLayout {
                lay: self,
                children: &mut b.children,
                items: &items,
                cstyle: &cstyle,
                bfc,
                content_x: content.x,
                content_y: content.y,
                content_w: cb.width,
                cb_h: cb.height,
                strut,
                lines: &mut lines,
                cur: Line::default(),
                pending_floats: Vec::new(),
                first_line: true,
                indent,
                ellipsis,
                max_lines,
                final_flush: false,
                done: false,
            };
            il.run(&pieces)
        };
        if inserted_marker {
            pieces.remove(0);
        }
        b.pieces = Some(pieces);
        b.items = items;
        b.lines = lines;
        height
    }
}

#[derive(Debug, Clone)]
enum PFKind {
    Text { text: String, item: usize },
    Box(usize),
}

#[derive(Debug, Clone)]
struct PF {
    x: f32,
    w: f32,
    kind: PFKind,
}

#[derive(Debug, Clone)]
struct OpenInline {
    item: usize,
    start_x: f32,
    started_here: bool,
}

#[derive(Debug, Clone)]
struct BgFrag {
    x: f32,
    w: f32,
    item: usize,
    left_edge: bool,
    right_edge: bool,
}

#[derive(Debug, Default)]
struct Line {
    y: f32,
    left: f32,
    right: f32,
    x: f32,
    frags: Vec<PF>,
    bg: Vec<BgFrag>,
    pending_space: Option<(f32, usize)>,
    has_content: bool,
    open: Vec<OpenInline>,
    /// Index of the first fragment of the current unbreakable run.
    break_at: Option<usize>,
}

struct InlineLayout<'l, 'a> {
    lay: &'l mut Layouter<'a>,
    children: &'l mut Vec<LayoutBox>,
    items: &'l [InlineItem],
    cstyle: &'l Rc<ComputedStyle>,
    bfc: &'l mut Bfc,
    content_x: f32,
    content_y: f32,
    content_w: f32,
    cb_h: Option<f32>,
    strut: TextMetrics,
    lines: &'l mut Vec<LineBox>,
    cur: Line,
    pending_floats: Vec<usize>,
    first_line: bool,
    indent: f32,
    /// `text-overflow: ellipsis` applies (overflow clipped).
    ellipsis: bool,
    /// `-webkit-line-clamp`.
    max_lines: Option<usize>,
    /// Set for the last `finish_line` call (no content follows).
    final_flush: bool,
    /// Line clamp reached: remaining content is dropped.
    done: bool,
}

impl<'l, 'a> InlineLayout<'l, 'a> {
    fn style(&self, idx: usize) -> &Rc<ComputedStyle> {
        item_style(self.items, self.cstyle, idx)
    }

    fn strut_h(&self) -> f32 {
        (self.strut.above + self.strut.below).max(0.0)
    }

    fn start_line(&mut self, y: f32) {
        let (l, r) = self.bfc.available(y, self.strut_h().max(1.0), self.content_x, self.content_w);
        self.cur.y = y;
        self.cur.left = l;
        self.cur.right = r;
        self.cur.x = l + if self.first_line { self.indent } else { 0.0 };
        self.cur.frags.clear();
        self.cur.bg.clear();
        self.cur.pending_space = None;
        self.cur.has_content = false;
        self.cur.break_at = None;
        let x = self.cur.x;
        for o in &mut self.cur.open {
            o.start_x = x;
            o.started_here = false;
        }
    }

    fn run(&mut self, pieces: &[Piece]) -> f32 {
        self.start_line(self.content_y);
        for p in pieces {
            if self.done {
                break;
            }
            match p {
                Piece::Word {
                    text,
                    width,
                    item,
                    break_before,
                } => self.place_word(text.clone(), *width, *item, *break_before),
                Piece::Space { width, item } => {
                    if self.cur.has_content && self.cur.pending_space.is_none() {
                        self.cur.pending_space = Some((*width, *item));
                    }
                }
                Piece::Open(i) => self.open(*i),
                Piece::Close(i) => self.close(*i),
                Piece::Atomic(c) => self.place_atomic(*c),
                Piece::Float(c) => self.place_float(*c),
                Piece::Abs(c) => {
                    let (x, y) = (self.cur.x, self.cur.y);
                    self.children[*c].static_pos = (x, y);
                }
                Piece::Break => self.finish_line(true),
            }
        }
        if !self.done {
            self.final_flush = true;
            self.finish_line(false);
        }
        // Floats still pending go at the current y.
        let pending = std::mem::take(&mut self.pending_floats);
        for c in pending {
            let y = self.cur.y;
            self.layout_float_at(c, y);
        }
        (self.cur.y - self.content_y).max(0.0)
    }

    fn open(&mut self, i: usize) {
        let style = self.style(i).clone();
        let cw = self.content_w;
        let left = style.margin.left.resolve_or_zero(cw) + style.border_width.left + style.padding.left.resolve_or_zero(cw);
        self.cur.open.push(OpenInline {
            item: i,
            start_x: self.cur.x,
            started_here: true,
        });
        self.cur.x += left;
    }

    fn close(&mut self, i: usize) {
        let pos = match self.cur.open.iter().rposition(|o| o.item == i) {
            Some(p) => p,
            None => return,
        };
        let o = self.cur.open.remove(pos);
        let style = self.style(i).clone();
        let cw = self.content_w;
        let right = style.margin.right.resolve_or_zero(cw) + style.border_width.right + style.padding.right.resolve_or_zero(cw);
        let end = self.cur.x + right;
        if needs_bg(&style) {
            self.cur.bg.push(BgFrag {
                x: o.start_x,
                w: (end - o.start_x).max(0.0),
                item: i,
                left_edge: o.started_here,
                right_edge: true,
            });
        }
        self.cur.x = end;
    }

    fn pending_w(&self) -> f32 {
        self.cur.pending_space.map(|p| p.0).unwrap_or(0.0)
    }

    fn place_word(&mut self, text: Rc<str>, width: f32, item: usize, break_before: bool) {
        let wraps = self.style(item).white_space.wraps();
        let policy = break_policy(self.style(item));
        let mut text: String = text.to_string();
        let mut width = width;
        let mut break_before = break_before;
        let mut guard = 0;
        loop {
            guard += 1;
            if self.done {
                return;
            }
            let fits = self.cur.x + self.pending_w() + width <= self.cur.right + 0.01;
            if fits || !wraps || guard > 10_000 {
                self.push_text(text, width, item, break_before);
                return;
            }
            if self.cur.has_content && policy != BreakPolicy::Anywhere {
                if break_before {
                    self.finish_line(false);
                    break_before = false;
                    continue;
                }
                if let Some(idx) = self.cur.break_at.filter(|i| *i > 0) {
                    self.carry_over(idx);
                    continue;
                }
            }
            if policy == BreakPolicy::Never {
                // The word may not be split: it overflows the line.
                self.push_text(text, width, item, break_before);
                return;
            }
            // Split the word by characters.
            let avail = self.cur.right - self.cur.x - self.pending_w();
            let (head, head_w, tail) = self.split_to_fit(&text, item, avail, !self.cur.has_content);
            if head.is_empty() {
                if self.cur.has_content {
                    self.finish_line(false);
                    continue;
                }
                // Cannot happen (split forces one char on an empty line).
                self.push_text(text, width, item, break_before);
                return;
            }
            self.push_text(head, head_w, item, break_before);
            if tail.is_empty() {
                return;
            }
            self.finish_line(false);
            let style = self.style(item).clone();
            let font = self.lay.font_for(&style);
            width = self.lay.measurer.width(font, style.font_size, &tail) + style.letter_spacing * tail.chars().count() as f32;
            text = tail;
            break_before = false;
        }
    }

    /// Longest prefix of `text` fitting in `avail`. Forces at least one
    /// character when `force`.
    fn split_to_fit(&mut self, text: &str, item: usize, avail: f32, force: bool) -> (String, f32, String) {
        let style = self.style(item).clone();
        let font = self.lay.font_for(&style);
        let size = style.font_size;
        let ls = style.letter_spacing;
        let mut best_end = 0;
        let mut best_w = 0.0;
        let mut count = 0;
        for (i, ch) in text.char_indices() {
            let end = i + ch.len_utf8();
            count += 1;
            let w = self.lay.measurer.width(font, size, &text[..end]) + ls * count as f32;
            if w <= avail + 0.01 || (force && count == 1) {
                best_end = end;
                best_w = w;
            } else {
                break;
            }
        }
        (text[..best_end].to_string(), best_w, text[best_end..].to_string())
    }

    fn consume_pending_space(&mut self) {
        if let Some((sw, sitem)) = self.cur.pending_space.take() {
            if !self.cur.has_content {
                return;
            }
            match self.cur.frags.last_mut() {
                Some(PF {
                    kind: PFKind::Text { text, item },
                    w,
                    ..
                }) if *item == sitem => {
                    text.push(' ');
                    *w += sw;
                }
                _ => {
                    let x = self.cur.x;
                    self.cur.frags.push(PF {
                        x,
                        w: sw,
                        kind: PFKind::Text {
                            text: " ".into(),
                            item: sitem,
                        },
                    });
                }
            }
            self.cur.x += sw;
        }
    }

    fn push_text(&mut self, text: String, width: f32, item: usize, break_before: bool) {
        self.consume_pending_space();
        if break_before {
            self.cur.break_at = Some(self.cur.frags.len());
        }
        let appended = if !break_before {
            match self.cur.frags.last_mut() {
                Some(PF {
                    kind: PFKind::Text { text: t, item: it },
                    w,
                    ..
                }) if *it == item => {
                    t.push_str(&text);
                    *w += width;
                    true
                }
                _ => false,
            }
        } else {
            false
        };
        if !appended {
            let x = self.cur.x;
            self.cur.frags.push(PF {
                x,
                w: width,
                kind: PFKind::Text { text, item },
            });
        }
        self.cur.x += width;
        self.cur.has_content = true;
    }

    fn place_atomic(&mut self, c: usize) {
        let avail = self.content_w;
        let cb_h = self.cb_h;
        self.lay.layout_atomic(&mut self.children[c], avail, cb_h);
        let w = self.children[c].margin_rect().w;
        let break_before = self.cstyle.white_space.wraps();
        let mut guard = 0;
        loop {
            guard += 1;
            let fits = self.cur.x + self.pending_w() + w <= self.cur.right + 0.01;
            if fits || !self.cur.has_content || !break_before || guard > 100 {
                break;
            }
            if let Some(idx) = self.cur.break_at.filter(|i| *i > 0) {
                if !break_before {
                    self.carry_over(idx);
                    continue;
                }
            }
            self.finish_line(false);
        }
        self.consume_pending_space();
        if break_before {
            self.cur.break_at = Some(self.cur.frags.len());
        }
        let x = self.cur.x;
        self.cur.frags.push(PF {
            x,
            w,
            kind: PFKind::Box(c),
        });
        self.cur.x += w;
        self.cur.has_content = true;
    }

    fn place_float(&mut self, c: usize) {
        if self.cur.has_content {
            self.pending_floats.push(c);
            return;
        }
        let y = self.cur.y;
        self.layout_float_at(c, y);
        // The current (empty) line shrinks around the new float.
        let (l, r) = self.bfc.available(y, self.strut_h().max(1.0), self.content_x, self.content_w);
        let shift = l - self.cur.left;
        self.cur.left = l;
        self.cur.right = r;
        self.cur.x += shift;
        for o in &mut self.cur.open {
            o.start_x += shift;
        }
        for f in &mut self.cur.frags {
            f.x += shift;
        }
    }

    fn layout_float_at(&mut self, c: usize, y: f32) {
        let cb = Cb {
            x: self.content_x,
            width: self.content_w,
            height: self.cb_h,
        };
        self.lay.layout_float(&mut self.children[c], &cb, y, self.bfc);
    }

    /// Move the fragments from `idx` on to a fresh line.
    fn carry_over(&mut self, idx: usize) {
        let carried = self.cur.frags.split_off(idx);
        let carried_start = carried.first().map(|f| f.x).unwrap_or(self.cur.x);
        // Trim the x of the line to the end of what stays.
        self.cur.x = self.cur.frags.last().map(|f| f.x + f.w).unwrap_or(self.cur.left);
        self.cur.pending_space = None;
        // Open inlines that started inside the carried region move with it.
        let mut moved: Vec<usize> = Vec::new();
        for (i, o) in self.cur.open.iter().enumerate() {
            if o.started_here && o.start_x >= carried_start - 0.01 {
                moved.push(i);
            }
        }
        let saved: Vec<(usize, f32)> = moved.iter().map(|&i| (i, self.cur.open[i].start_x)).collect();
        for &i in &moved {
            // Hide from finish_line's bg emission by marking as not started here
            // with a start beyond the line end.
            self.cur.open[i].start_x = f32::MAX;
        }
        self.finish_line(false);
        let dx = self.cur.x - carried_start;
        for (i, sx) in saved {
            if let Some(o) = self.cur.open.get_mut(i) {
                o.start_x = sx + dx;
                o.started_here = true;
            }
        }
        for mut f in carried {
            f.x += dx;
            self.cur.x = f.x + f.w;
            self.cur.frags.push(f);
        }
        self.cur.has_content = !self.cur.frags.is_empty();
        self.cur.break_at = Some(0);
    }

    /// Merge adjacent text fragments of the same item into one run.
    fn merge_frags(&mut self) {
        let frags = std::mem::take(&mut self.cur.frags);
        let mut out: Vec<PF> = Vec::with_capacity(frags.len());
        for f in frags {
            if let (
                Some(PF {
                    x: px,
                    w: pw,
                    kind: PFKind::Text { text: pt, item: pi },
                }),
                PFKind::Text { text, item },
            ) = (out.last_mut(), &f.kind)
            {
                if *pi == *item && (*px + *pw - f.x).abs() < 0.05 {
                    pt.push_str(text);
                    *pw += f.w;
                    continue;
                }
            }
            out.push(f);
        }
        self.cur.frags = out;
    }

    fn trim_trailing_space(&mut self) {
        if let Some(PF {
            kind: PFKind::Text { text, item },
            w,
            ..
        }) = self.cur.frags.last_mut()
        {
            if text.ends_with(' ') && text.len() > 1 {
                text.pop();
                let style = item_style(self.items, self.cstyle, *item).clone();
                let font = self.lay.font_for(&style);
                let sw = self.lay.measurer.space_width(font, style.font_size);
                *w -= sw;
                self.cur.x -= sw;
            }
        }
    }

    /// Truncate the current line so that it ends with "…" within the line
    /// box (`text-overflow: ellipsis` / line clamping).
    fn ellipsize(&mut self) {
        let cstyle = self.cstyle.clone();
        let font = self.lay.font_for(&cstyle);
        let size = cstyle.font_size;
        let ell = "\u{2026}";
        let ell_w = self.lay.measurer.width(font, size, ell);
        let limit = self.cur.right - ell_w;
        // Drop / trim fragments from the end until the text fits.
        while let Some(last) = self.cur.frags.last() {
            if last.x + last.w <= limit + 0.01 {
                break;
            }
            let (x, item, text) = match &last.kind {
                PFKind::Text { text, item } => (last.x, *item, text.clone()),
                PFKind::Box(_) => {
                    self.cur.frags.pop();
                    continue;
                }
            };
            let avail = limit - x;
            let (head, head_w, _tail) = self.split_to_fit(text.trim_end(), item, avail, false);
            if head.trim().is_empty() {
                self.cur.frags.pop();
                continue;
            }
            if let Some(PFKind::Text { text, .. }) = self.cur.frags.last_mut().map(|f| &mut f.kind) {
                *text = head;
            }
            if let Some(f) = self.cur.frags.last_mut() {
                f.w = head_w;
            }
            break;
        }
        let x = self.cur.frags.last().map(|f| f.x + f.w).unwrap_or(self.cur.left);
        self.cur.frags.push(PF {
            x,
            w: ell_w,
            kind: PFKind::Text {
                text: ell.to_string(),
                item: MARKER_ITEM,
            },
        });
        self.cur.x = x + ell_w;
        self.cur.has_content = true;
        // Inline boxes still open end here.
        for bg in &mut self.cur.bg {
            if bg.x + bg.w > self.cur.x {
                bg.w = (self.cur.x - bg.x).max(0.0);
            }
        }
    }

    /// Mirror the line's fragments for `direction: rtl` (visual order is
    /// reversed; no bidi reordering of mixed-direction text).
    fn mirror_line(&mut self) {
        let (l, r) = (self.cur.left, self.cur.right);
        let used = self.cur.x - l;
        let mirror = |x: f32, w: f32| r - (x - l) - w;
        for f in &mut self.cur.frags {
            f.x = mirror(f.x, f.w);
        }
        for bg in &mut self.cur.bg {
            bg.x = mirror(bg.x, bg.w);
        }
        for o in &mut self.cur.open {
            if o.start_x <= self.cur.x + 0.01 {
                // An open inline box spans to the line end: its mirrored
                // start is the left edge of the used area.
                o.start_x = r - used + (self.cur.x - o.start_x) - (self.cur.x - o.start_x);
                o.start_x = r - used;
            }
        }
        // Keep `cur.x` meaning "end of content" for the alignment maths.
        self.cur.x = l + used;
    }

    fn finish_line(&mut self, forced: bool) {
        if !self.cur.has_content && !forced {
            if self.cur.frags.is_empty() {
                return;
            }
        }
        self.cur.pending_space = None;
        self.trim_trailing_space();
        self.merge_frags();

        // Line clamp: this is the last allowed line and content follows.
        let more_follows = !self.final_flush;
        let clamp_hit = matches!(self.max_lines, Some(n) if self.lines.len() + 1 >= n) && more_follows;
        let overflows = self.cur.x > self.cur.right + 0.5;
        if (self.ellipsis && overflows) || clamp_hit {
            self.ellipsize();
        }
        if clamp_hit {
            self.done = true;
        }
        if self.cstyle.direction == Direction::Rtl {
            self.mirror_line();
        }

        // Vertical metrics.
        struct FM {
            above: f32,
            below: f32,
            mode: u8, // 0 baseline, 1 top, 2 bottom
            shift: f32,
            tm: Option<TextMetrics>,
        }
        let mut fms: Vec<FM> = Vec::with_capacity(self.cur.frags.len());
        let mut a = self.strut.above;
        let mut d = self.strut.below;
        let mut extra_h: f32 = 0.0;
        for f in &self.cur.frags {
            let fm = match &f.kind {
                PFKind::Text { item, .. } => {
                    let style = self.style(*item).clone();
                    let tm = self.lay.text_metrics(&style);
                    let shift = match style.vertical_align {
                        VerticalAlign::Sub => -0.25 * style.font_size,
                        VerticalAlign::Super => 0.35 * style.font_size,
                        VerticalAlign::Length(v) => v,
                        _ => 0.0,
                    };
                    FM {
                        above: tm.above + shift,
                        below: tm.below - shift,
                        mode: 0,
                        shift,
                        tm: Some(tm),
                    }
                }
                PFKind::Box(c) => {
                    let child = &self.children[*c];
                    let mr = child.margin_rect();
                    let h = mr.h;
                    let base = child.baseline.map(|bo| child.margin.top + bo).unwrap_or(h);
                    let fs = self
                        .cur
                        .open
                        .last()
                        .map(|o| self.style(o.item).font_size)
                        .unwrap_or(self.cstyle.font_size);
                    match child.style.vertical_align {
                        VerticalAlign::Top => FM {
                            above: 0.0,
                            below: 0.0,
                            mode: 1,
                            shift: 0.0,
                            tm: None,
                        },
                        VerticalAlign::Bottom => FM {
                            above: 0.0,
                            below: 0.0,
                            mode: 2,
                            shift: 0.0,
                            tm: None,
                        },
                        VerticalAlign::Middle => {
                            let above = h / 2.0 + 0.25 * fs;
                            FM { above, below: h - above, mode: 0, shift: 0.0, tm: None }
                        }
                        VerticalAlign::TextTop => {
                            let above = self.strut.asc;
                            FM { above, below: h - above, mode: 0, shift: 0.0, tm: None }
                        }
                        VerticalAlign::TextBottom => {
                            let below = self.strut.desc;
                            FM { above: h - below, below, mode: 0, shift: 0.0, tm: None }
                        }
                        VerticalAlign::Sub => FM { above: base - 0.25 * fs, below: h - base + 0.25 * fs, mode: 0, shift: 0.0, tm: None },
                        VerticalAlign::Super => FM { above: base + 0.35 * fs, below: h - base - 0.35 * fs, mode: 0, shift: 0.0, tm: None },
                        VerticalAlign::Length(v) => FM { above: base + v, below: h - base - v, mode: 0, shift: 0.0, tm: None },
                        VerticalAlign::Baseline => FM { above: base, below: h - base, mode: 0, shift: 0.0, tm: None },
                    }
                }
            };
            if fm.mode == 0 {
                a = a.max(fm.above);
                d = d.max(fm.below);
            } else if let PFKind::Box(c) = &f.kind {
                extra_h = extra_h.max(self.children[*c].margin_rect().h);
            }
            fms.push(fm);
        }
        for bg in &self.cur.bg {
            let style = self.style(bg.item).clone();
            let tm = self.lay.text_metrics(&style);
            a = a.max(tm.above);
            d = d.max(tm.below);
        }
        let h = (a + d).max(extra_h).max(0.0);
        let y = self.cur.y;
        let baseline = y + a;

        // Horizontal alignment.
        let used = self.cur.x - self.cur.left;
        let free = (self.cur.right - self.cur.left) - used;
        let rtl = self.cstyle.direction == Direction::Rtl;
        let shift_x = if rtl {
            // Content was mirrored to the right edge (start); `right` means
            // the end (left) edge.
            match self.cstyle.text_align {
                TextAlign::Right => -free.max(0.0),
                TextAlign::Center => -(free / 2.0).max(0.0),
                _ => 0.0,
            }
        } else {
            match self.cstyle.text_align {
                TextAlign::Right => free.max(0.0),
                TextAlign::Center => (free / 2.0).max(0.0),
                _ => 0.0,
            }
        };

        let mut fragments: Vec<Fragment> = Vec::with_capacity(self.cur.frags.len() + self.cur.bg.len() + self.cur.open.len());
        // Backgrounds of inline boxes still open at the line end.
        let line_end = self.cur.x;
        let mut extra_bg: Vec<BgFrag> = Vec::new();
        for idx in 0..self.cur.open.len() {
            let (start_x, item, started_here) = {
                let o = &self.cur.open[idx];
                (o.start_x, o.item, o.started_here)
            };
            if start_x > line_end + 0.01 {
                continue; // started inside a carried-over run
            }
            let style = self.style(item).clone();
            if needs_bg(&style) {
                extra_bg.push(BgFrag {
                    x: start_x,
                    w: (line_end - start_x).max(0.0),
                    item,
                    left_edge: started_here,
                    right_edge: false,
                });
            }
            self.cur.open[idx].started_here = false;
        }
        for bg in self.cur.bg.iter().chain(extra_bg.iter()) {
            let style = self.style(bg.item).clone();
            let tm = self.lay.text_metrics(&style);
            let cw = self.content_w;
            let pt = style.padding.top.resolve_or_zero(cw) + style.border_width.top;
            let pb = style.padding.bottom.resolve_or_zero(cw) + style.border_width.bottom;
            fragments.push(Fragment {
                rect: Rect::new(bg.x + shift_x, baseline - tm.asc - pt, bg.w, tm.asc + tm.desc + pt + pb),
                baseline,
                node: item_elem(self.items, bg.item),
                kind: FragmentKind::InlineBox {
                    style,
                    left_edge: bg.left_edge,
                    right_edge: bg.right_edge,
                },
            });
        }
        let frags = std::mem::take(&mut self.cur.frags);
        for (f, fm) in frags.into_iter().zip(fms.into_iter()) {
            match f.kind {
                PFKind::Text { text, item } => {
                    let tm = fm.tm.expect("text metrics");
                    let style = self.style(item).clone();
                    let bl = baseline - fm.shift;
                    let oblique = self.lay.synthetic_oblique(tm.font, &style);
                    fragments.push(Fragment {
                        rect: Rect::new(f.x + shift_x, bl - tm.asc, f.w, tm.asc + tm.desc),
                        baseline: bl,
                        node: if item == MARKER_ITEM { None } else { item_elem(self.items, item) },
                        kind: FragmentKind::Text {
                            text: Rc::from(text.as_str()),
                            font: tm.font,
                            size: tm.size,
                            color: style.color,
                            letter_spacing: style.letter_spacing,
                            decoration: style.text_decoration,
                            hidden: style.visibility == Visibility::Hidden,
                            decoration_info: DecorationInfo {
                                lines: style.text_decoration,
                                color: style.text_decoration_color,
                                style: style.text_decoration_style,
                                thickness: style.text_decoration_thickness,
                            },
                            shadows: style.text_shadow.clone(),
                            small_caps: style.font_variant_small_caps,
                            oblique,
                        },
                    });
                }
                PFKind::Box(c) => {
                    let child = &mut self.children[c];
                    let mr = child.margin_rect();
                    let top = match fm.mode {
                        1 => y,
                        2 => y + h - mr.h,
                        _ => baseline - fm.above,
                    };
                    let m = child.margin;
                    child.translate(f.x + shift_x + m.left - child.rect.x, top + m.top - child.rect.y);
                    let node = child.node;
                    let rect = child.margin_rect();
                    fragments.push(Fragment {
                        rect,
                        baseline,
                        node,
                        kind: FragmentKind::Box(c),
                    });
                }
            }
        }
        self.lines.push(LineBox {
            rect: Rect::new(self.cur.left, y, (self.cur.right - self.cur.left).max(0.0), h),
            baseline,
            fragments,
        });
        self.first_line = false;
        let new_y = y + h;
        let pending = std::mem::take(&mut self.pending_floats);
        for c in pending {
            self.layout_float_at(c, new_y);
        }
        self.start_line(new_y);
    }
}

fn needs_bg(style: &ComputedStyle) -> bool {
    !style.background_color.is_transparent()
        || style.has_visible_border()
        || style.background_image.is_some()
        || !style.background_layers.is_empty()
        || !style.box_shadow.is_empty()
        || style.outline_width > 0.0
}
