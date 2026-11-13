//! Inline formatting: line breaking, fragment placement, vertical alignment.

use crate::block::{Cb, Layouter};
use crate::float::Bfc;
use crate::text::{cjk_break_between, segment_tabs, split_cjk, transform, Collapser, Seg};
use crate::tree::{DecorationInfo, Fragment, FragmentKind, InlineItem, LayoutBox, LineBox};
use rabbithole_dom::NodeId;
use rabbithole_gfx::{FontId, Rect};
use rabbithole_style::{
    ComputedStyle, Direction, ListStylePosition, OverflowWrap, TextAlign, TextOverflow, UnicodeBidi, VerticalAlign, Visibility, WordBreak,
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
        WordBreak::BreakWord | WordBreak::AutoPhrase => BreakPolicy::IfNeeded,
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
    /// Extra space before the next word (`text-autospace`).
    Gap(f32),
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

/// `position: relative` offsets of inline elements (CSS 2.1 §9.4.3),
/// accumulated through nested ones: per item, and per atomic child index.
/// Empty when no inline element is relatively positioned.
fn relative_offsets(items: &[InlineItem], children: usize, cbw: f32, cbh: Option<f32>) -> (Vec<(f32, f32)>, Vec<(f32, f32)>) {
    if !items.iter().any(|it| matches!(it, InlineItem::Open { style, .. } if style.position == rabbithole_style::Position::Relative)) {
        return (Vec::new(), Vec::new());
    }
    let mut out = vec![(0.0, 0.0); items.len()];
    let mut kids = vec![(0.0, 0.0); children];
    let mut stack: Vec<(f32, f32)> = vec![(0.0, 0.0)];
    for (i, it) in items.iter().enumerate() {
        let cur = *stack.last().unwrap_or(&(0.0, 0.0));
        match it {
            InlineItem::Open { style, .. } => {
                let mut o = cur;
                if style.position == rabbithole_style::Position::Relative {
                    let s = &style.inset;
                    o.0 += s.left.resolve(cbw).or_else(|| s.right.resolve(cbw).map(|v| -v)).unwrap_or(0.0);
                    o.1 += s.top.resolve_definite(cbh).or_else(|| s.bottom.resolve_definite(cbh).map(|v| -v)).unwrap_or(0.0);
                }
                out[i] = o;
                stack.push(o);
            }
            InlineItem::Close => {
                out[i] = cur;
                if stack.len() > 1 {
                    stack.pop();
                }
            }
            InlineItem::Atomic(c) | InlineItem::Float(c) | InlineItem::Absolute(c) => {
                out[i] = cur;
                if let Some(k) = kids.get_mut(*c) {
                    *k = cur;
                }
            }
            _ => out[i] = cur,
        }
    }
    (out, kids)
}

/// A decoration extent waiting for its decorating box's whole inline
/// size (see `InlineLayout::decoration_extents`); offsets relative to the
/// fragment's left edge.
#[derive(Debug, Clone)]
struct InsetFixup {
    line: usize,
    frag: usize,
    deco: usize,
    open: usize,
    lo: f32,
    hi: f32,
    w: f32,
    inset: (Option<rabbithole_style::Length>, Option<rabbithole_style::Length>),
}

/// The inline element (Open item) directly enclosing each item, when some
/// inline element insets its text decorations (`text-decoration-inset`);
/// empty otherwise.
fn has_inset(s: &ComputedStyle) -> bool {
    s.text_decoration.any() && (s.text_decoration_inset.0 != rabbithole_style::Length::ZERO || s.text_decoration_inset.1 != rabbithole_style::Length::ZERO)
}

/// Pseudo item index standing for the block container's root inline box
/// (a block decorating box insets its decorations per paragraph).
const BLOCK_BOX: usize = usize::MAX - 1;

fn open_parents(items: &[InlineItem], container: &ComputedStyle) -> Vec<Option<usize>> {
    if !has_inset(container) && !items.iter().any(|it| matches!(it, InlineItem::Open { style, .. } if has_inset(style))) {
        return Vec::new();
    }
    let mut out = vec![None; items.len()];
    let mut stack: Vec<usize> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        out[i] = stack.last().copied();
        match it {
            InlineItem::Open { .. } => stack.push(i),
            InlineItem::Close => {
                stack.pop();
            }
            _ => {}
        }
    }
    out
}

/// The emphasis marks of text in `style`, with the side resolved in the
/// line's logical space (vertical lines are laid out turned: the right
/// side is the logical top in `vertical-rl`, the bottom in `vertical-lr`
/// and `sideways-lr`).
pub(crate) fn emphasis_of(style: &ComputedStyle) -> Option<crate::tree::Emphasis> {
    let vertical = style.writing_mode.is_vertical();
    let mark = style.text_emphasis_style.mark(vertical)?;
    let over = if !vertical {
        style.text_emphasis_over
    } else if style.writing_mode == rabbithole_style::WritingMode::VerticalRl {
        style.text_emphasis_right
    } else {
        !style.text_emphasis_right
    };
    Some(crate::tree::Emphasis { mark: Rc::from(mark.as_str()), color: style.text_emphasis_color.unwrap_or(style.color), over })
}

/// `text-autospace` applies between `a` and `b`: an ideograph next to a
/// letter or digit of another script.
fn autospace_between(a: char, b: char) -> bool {
    fn ideo(c: char) -> bool {
        matches!(c as u32, 0x3041..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x3FFFD)
    }
    fn other(c: char) -> bool {
        c.is_alphanumeric() && (c as u32) < 0x2E80 && !matches!(c as u32, 0x0590..=0x08FF)
    }
    (ideo(a) && other(b)) || (other(a) && ideo(b))
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
        let lh = if style.line_height_tight { m.ascent + m.descent } else { style.line_height_px() };
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
        // Last character of the previous word (breaks between ideographs
        // need no space).
        let mut prev_char: Option<char> = None;
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
                    let keep_all = match style.word_break {
                        rabbithole_style::WordBreak::KeepAll => 1u8,
                        rabbithole_style::WordBreak::AutoPhrase => 2,
                        _ => 0,
                    };
                    let strict = style.line_break == 2;
                    let anywhere = style.line_break == 3 && !style.wrap_inside_avoid;
                    for seg in segs.drain(..) {
                        match seg {
                            Seg::Word(w) => {
                                let is_space_word = w.chars().all(|c| c == ' ');
                                // A break between the previous word and this
                                // one (ideographs break without spaces).
                                if wraps && !prev_break_opp {
                                    if let (Some(p), Some(c)) = (prev_char, w.chars().next()) {
                                        if p != ' ' && c != ' ' && (anywhere || cjk_break_between(p, c, keep_all, strict)) {
                                            prev_break_opp = true;
                                        }
                                    }
                                }
                                let prev_before = prev_char;
                                prev_char = w.chars().rev().find(|c| !matches!(*c as u32, 0x300..=0x36F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)).or(w.chars().last());
                                let mut parts: Vec<(String, bool)> = Vec::new();
                                for (part, continuation) in split_hyphens(&w) {
                                    if wraps && !is_space_word {
                                        for (k, p) in split_cjk(&part, keep_all, strict, anywhere).into_iter().enumerate() {
                                            parts.push((p, continuation || k > 0));
                                        }
                                    } else {
                                        parts.push((part, continuation));
                                    }
                                }
                                // `text-autospace`: 1/8ic between ideographs and
                                // other letters or digits.
                                let gap = if style.text_autospace && !is_space_word && !style.writing_mode.is_vertical() {
                                    style.font_size * rabbithole_style::values::font_units(style).2 / 8.0
                                } else {
                                    0.0
                                };
                                let mut gapped: Vec<(String, bool, bool)> = Vec::new();
                                let mut last = prev_before;
                                for (part, continuation) in parts {
                                    if gap > 0.0 {
                                        let mut cur = String::new();
                                        let mut first = true;
                                        for c in part.chars() {
                                            let g = last.is_some_and(|p| autospace_between(p, c));
                                            if g && !cur.is_empty() {
                                                gapped.push((std::mem::take(&mut cur), continuation || !first, false));
                                                first = false;
                                            }
                                            if g && cur.is_empty() {
                                                // Mark the gap before this sub-part.
                                                gapped.push((String::new(), false, true));
                                            }
                                            cur.push(c);
                                            if !matches!(c as u32, 0x300..=0x36F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F) {
                                                last = Some(c);
                                            }
                                        }
                                        if !cur.is_empty() {
                                            gapped.push((cur, continuation || !first, false));
                                        }
                                    } else {
                                        gapped.push((part, continuation, false));
                                    }
                                }
                                for (part, continuation, is_gap) in gapped {
                                    if is_gap {
                                        pieces.push(Piece::Gap(gap));
                                        continue;
                                    }
                                    let width = self.measurer.width(font, size, &part) + ls * part.chars().count() as f32;
                                    // A break opportunity before a word comes from the
                                    // preceding whitespace (owned by whatever element
                                    // produced it); `nowrap` on this word only forbids
                                    // breaks at its own spaces.
                                    let bb = prev_break_opp || (wraps && continuation);
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
                                prev_break_opp = wraps;
                                prev_char = Some(' ');
                            }
                            Seg::Opportunity => {
                                prev_break_opp = wraps;
                                prev_char = None;
                            }
                            Seg::Break => {
                                pieces.push(Piece::Break);
                                prev_break_opp = true;
                                prev_char = None;
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
                    prev_char = None;
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
        let own_clamp = cstyle.line_clamp.filter(|n| *n > 0).map(|n| n as usize);
        // Lines left by an ancestor's `line-clamp`.
        let outer_clamp = if own_clamp.is_none() { self.clamp.map(|c| c.0) } else { None };
        let max_lines = own_clamp.or(outer_clamp);
        if outer_clamp == Some(0) {
            // Clamped away: no lines.
            b.items = items;
            b.lines = Vec::new();
            if inserted_marker {
                pieces.remove(0);
            }
            b.pieces = Some(pieces);
            return 0.0;
        }
        let (rel, rel_kids) = relative_offsets(&items, b.children.len(), cb.width, cb.height);
        let parent_open = open_parents(&items, &cstyle);
        let bidi_items = items.iter().any(|it| matches!(it, InlineItem::Open { style, .. } if style.unicode_bidi != UnicodeBidi::Normal));
        // `text-wrap: balance` (CSS Text 4 §6.1.2): the narrowest wrap
        // width that keeps the line count, found by bisection over trial
        // layouts (paragraphs of at most 6 lines without floats).
        let balance = cstyle.text_wrap_balance && cstyle.white_space.wraps() && !items.iter().any(|it| matches!(it, InlineItem::Float(_)));
        let mut wrap_limit: Option<f32> = None;
        let mut passes = if balance { 14 } else { 0 };
        let (mut lo, mut hi, mut want) = (0.0f32, cb.width, 0usize);
        let mut first_pass = true;
        loop {
        let mut lines = Vec::new();
        let max_used;
        let clamped;
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
                bidi_items,
                rel: rel.clone(),
                rel_kids: rel_kids.clone(),
                parent_open: parent_open.clone(),
                inset_seen: std::collections::HashSet::new(),
                inset_fixups: Vec::new(),
                inset_total: std::collections::HashMap::new(),
                wrap_limit,
                max_used: 0.0,
            };
            let h = il.run(&pieces);
            il.resolve_insets();
            max_used = il.max_used;
            clamped = il.done;
            h
        };
        if passes > 0 {
            let n = lines.len();
            if first_pass {
                first_pass = false;
                if (2..=6).contains(&n) && !clamped {
                    want = n;
                    hi = max_used.min(cb.width);
                    lo = 0.0;
                } else {
                    passes = 0;
                }
            } else {
                passes -= 1;
                let fits = !clamped && n <= want && max_used <= wrap_limit.unwrap_or(f32::MAX) + 0.01;
                if fits {
                    hi = wrap_limit.unwrap_or(hi);
                } else {
                    lo = wrap_limit.unwrap_or(lo);
                }
            }
            if passes > 1 && hi - lo > 0.5 {
                wrap_limit = Some((lo + hi) / 2.0);
                continue;
            }
            if passes > 0 && wrap_limit != Some(hi) {
                // The final layout at the narrowest width that fits.
                wrap_limit = Some(hi);
                passes = 0;
                continue;
            }
        }
        if inserted_marker {
            pieces.remove(0);
        }
        b.pieces = Some(pieces);
        b.items = items;
        if let (Some(n), Some(c)) = (outer_clamp, self.clamp.as_mut()) {
            c.0 = n.saturating_sub(lines.len());
            if c.0 == 0 {
                c.1 = lines.last().map(|l| l.rect.bottom());
            }
        }
        b.lines = lines;
        return height;
        }
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
    /// The line's end for alignment (`right` may be narrower when lines
    /// are balanced).
    align_right: f32,
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
    /// Some inline element has a `unicode-bidi` other than `normal`.
    bidi_items: bool,
    /// Relative offsets per item / atomic child (see [`relative_offsets`]).
    rel: Vec<(f32, f32)>,
    rel_kids: Vec<(f32, f32)>,
    /// Lines wrap at this width (`text-wrap: balance`) but align in the
    /// full one.
    wrap_limit: Option<f32>,
    /// Widest line content (trailing hanging spaces excluded).
    max_used: f32,
    /// See [`open_parents`].
    parent_open: Vec<Option<usize>>,
    /// Inline elements with decoration insets already met on a line.
    inset_seen: std::collections::HashSet<usize>,
    inset_fixups: Vec<InsetFixup>,
    /// Inline size of the text of each inset decorating box, all lines.
    inset_total: std::collections::HashMap<usize, f32>,
}

impl<'l, 'a> InlineLayout<'l, 'a> {
    fn style(&self, idx: usize) -> &Rc<ComputedStyle> {
        item_style(self.items, self.cstyle, idx)
    }

    /// Per applied decoration of text item `item` (a fragment at `x`, `w`
    /// wide), the line extent relative to `x` when its decorating inline
    /// box insets it (`text-decoration-inset` at the box's start and end).
    /// The extents are resolved once all lines are laid out (percentages
    /// refer to the box's whole inline size): this records them.
    fn decoration_extents(&mut self, item: usize, style: &ComputedStyle, box_ext: &std::collections::HashMap<usize, (f32, f32)>, x: f32, w: f32, frag: usize) -> Vec<Option<(f32, f32)>> {
        let list = &style.applied_decorations;
        let mut chain = Vec::new();
        let mut p = self.parent_open.get(item).copied().flatten();
        while let Some(o) = p {
            if self.style(o).text_decoration.any() {
                chain.push(o);
            }
            p = self.parent_open[o];
        }
        chain.reverse();
        if chain.len() > list.len() {
            return Vec::new();
        }
        let first = list.len() - chain.len();
        for (j, &o) in chain.iter().enumerate() {
            let d = &list[first + j];
            let Some(&(lo, hi)) = box_ext.get(&o) else { continue };
            let starts = !self.inset_seen.contains(&o);
            let ends = !self.cur.open.iter().any(|e| e.item == o);
            self.inset_fixups.push(InsetFixup {
                line: self.lines.len(),
                frag,
                deco: first + j,
                open: o,
                lo: lo - x,
                hi: hi - x,
                w,
                inset: (if starts { Some(d.inset.0) } else { None }, if ends { Some(d.inset.1) } else { None }),
            });
        }
        // The block container's own decoration (the entry before the
        // inline elements' ones).
        if first > 0 && has_inset(self.cstyle) {
            let d = &list[first - 1];
            if d.lines == self.cstyle.text_decoration && d.inset == self.cstyle.text_decoration_inset {
                if let Some(&(lo, hi)) = box_ext.get(&BLOCK_BOX) {
                    let starts = !self.inset_seen.contains(&BLOCK_BOX);
                    let ends = self.final_flush;
                    self.inset_fixups.push(InsetFixup {
                        line: self.lines.len(),
                        frag,
                        deco: first - 1,
                        open: BLOCK_BOX,
                        lo: lo - x,
                        hi: hi - x,
                        w,
                        inset: (if starts { Some(d.inset.0) } else { None }, if ends { Some(d.inset.1) } else { None }),
                    });
                }
            }
        }
        vec![None; list.len()]
    }

    /// Resolve the recorded decoration insets (see `decoration_extents`).
    fn resolve_insets(&mut self) {
        let fixups = std::mem::take(&mut self.inset_fixups);
        for fx in fixups {
            let cw = self.content_w;
            let edges = if fx.open == BLOCK_BOX {
                0.0
            } else {
                let style = self.style(fx.open).clone();
                style.padding.left.resolve_or_zero(cw) + style.padding.right.resolve_or_zero(cw) + style.border_width.left + style.border_width.right
            };
            let base = self.inset_total.get(&fx.open).copied().unwrap_or(0.0) + edges;
            let l = fx.lo + fx.inset.0.map(|v| v.resolve_or_zero(base)).unwrap_or(0.0);
            let r = fx.hi - fx.inset.1.map(|v| v.resolve_or_zero(base)).unwrap_or(0.0);
            let a = if fx.lo.abs() < 0.01 { l } else { l.max(0.0) };
            let b = if (fx.w - fx.hi).abs() < 0.01 { r } else { r.min(fx.w) };
            if let Some(FragmentKind::Text { decoration_info, .. }) = self.lines.get_mut(fx.line).and_then(|ln| ln.fragments.get_mut(fx.frag)).map(|f| &mut f.kind) {
                if let Some(e) = decoration_info.extents.get_mut(fx.deco) {
                    *e = Some((a, b));
                }
            }
        }
    }

    fn rel_of(&self, idx: usize) -> (f32, f32) {
        self.rel.get(idx).copied().unwrap_or((0.0, 0.0))
    }

    fn strut_h(&self) -> f32 {
        (self.strut.above + self.strut.below).max(0.0)
    }

    fn start_line(&mut self, y: f32) {
        let (l, r) = self.bfc.available(y, self.strut_h().max(1.0), self.content_x, self.content_w);
        self.cur.y = y;
        self.cur.left = l;
        self.cur.align_right = r;
        self.cur.right = match self.wrap_limit {
            Some(w) => r.min(l + w + if self.first_line { self.indent.max(0.0) } else { 0.0 }),
            None => r,
        };
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
                Piece::Gap(w) => {
                    if self.cur.has_content {
                        self.cur.x += w;
                    }
                }
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
            // Preserved spaces at the end of a `pre-wrap` line hang: they
            // never move to the next line (CSS Text 3 §4.1.3).
            let hangs = wraps && self.style(item).white_space != rabbithole_style::WhiteSpace::BreakSpaces && self.cur.has_content && !text.is_empty() && text.chars().all(|c| c == ' ');
            if fits || hangs || (!wraps && !break_before) || guard > 10_000 {
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
            if policy == BreakPolicy::Never || !wraps {
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
        let spaced = self.style(item).word_spacing.abs() > 0.001;
        let cur_x = self.cur.x;
        let appended = if !break_before {
            match self.cur.frags.last_mut() {
                Some(PF {
                    kind: PFKind::Text { text: t, item: it },
                    w,
                    x,
                }) if *it == item && !(spaced && (t.ends_with(' ') || text.starts_with(' '))) && (*x + *w - cur_x).abs() < 0.01 => {
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
        let y = self.cur.y;
        if self.cur.has_content {
            // A float met in the middle of a line goes at the top of that
            // line when it fits beside the line's content (CSS 2.1 §9.5.1);
            // otherwise it waits for the next line.
            self.layout_float_at(c, y);
            let placed_here = self.bfc.last().map(|r| (r.y - y).abs() < 0.01).unwrap_or(false);
            let (l, r) = self.bfc.available(y, self.strut_h().max(1.0), self.content_x, self.content_w);
            let used = self.cur.x - self.cur.left;
            if !placed_here || r - l + 0.01 < used {
                self.bfc.pop();
                self.pending_floats.push(c);
                return;
            }
        } else {
            self.layout_float_at(c, y);
        }
        // The current (empty) line shrinks around the new float.
        let (l, r) = self.bfc.available(y, self.strut_h().max(1.0), self.content_x, self.content_w);
        let shift = l - self.cur.left;
        self.cur.left = l;
        self.cur.right = match self.wrap_limit {
            Some(w) => r.min(l + w),
            None => r,
        };
        self.cur.align_right = r;
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
                // With word-spacing the painter cannot place the words of
                // one run: keep the words apart.
                let spaced = (pt.ends_with(' ') || text.starts_with(' ')) && item_style(self.items, self.cstyle, *item).word_spacing.abs() > 0.001;
                if *pi == *item && (*px + *pw - f.x).abs() < 0.05 && !spaced {
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
            // Preserved spaces stay (they hang or overflow instead; right-
            // to-left lines still drop one, their hanging is not modelled).
            let preserved = matches!(
                item_style(self.items, self.cstyle, *item).white_space,
                rabbithole_style::WhiteSpace::PreWrap | rabbithole_style::WhiteSpace::BreakSpaces
            ) && self.cstyle.direction != Direction::Rtl;
            if text.ends_with(' ') && text.len() > 1 && !preserved {
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
            let (mut head, mut head_w, _tail) = self.split_to_fit(text.trim_end(), item, avail, false);
            // No space before the ellipsis.
            if head.ends_with(' ') && !head.trim().is_empty() {
                let trimmed = head.trim_end().to_string();
                head_w = self.split_to_fit(&trimmed, item, f32::MAX, false).1;
                head = trimmed;
            }
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

    /// Bidi controls opening (and closing) the inline element `item`'s
    /// `unicode-bidi` embedding / override / isolate.
    fn bidi_controls(&self, item: usize) -> (Vec<char>, Vec<char>) {
        use crate::bidi::*;
        let s = self.style(item);
        let rtl = s.direction == Direction::Rtl;
        let (emb, ovr, iso) = (if rtl { RLE_CH } else { LRE_CH }, if rtl { RLO_CH } else { LRO_CH }, if rtl { RLI_CH } else { LRI_CH });
        match s.unicode_bidi {
            UnicodeBidi::Normal => (vec![], vec![]),
            UnicodeBidi::Embed => (vec![emb], vec![PDF_CH]),
            UnicodeBidi::BidiOverride => (vec![ovr], vec![PDF_CH]),
            UnicodeBidi::Isolate => (vec![iso], vec![PDI_CH]),
            UnicodeBidi::IsolateOverride => (vec![iso, ovr], vec![PDF_CH, PDI_CH]),
            UnicodeBidi::Plaintext => (vec![FSI_CH], vec![PDI_CH]),
        }
    }

    /// Inline elements (item indices, outermost first) enclosing item `idx`
    /// that have a `unicode-bidi` other than `normal`.
    fn bidi_chain(&self, idx: usize) -> Vec<usize> {
        let mut out = Vec::new();
        if idx == MARKER_ITEM || idx >= self.items.len() {
            return out;
        }
        // Walk back over the items: an unmatched Open is an ancestor.
        let mut depth = 0usize;
        for j in (0..idx).rev() {
            match &self.items[j] {
                InlineItem::Close => depth += 1,
                InlineItem::Open { style, .. } => {
                    if depth == 0 {
                        if style.unicode_bidi != UnicodeBidi::Normal {
                            out.push(j);
                        }
                    } else {
                        depth -= 1;
                    }
                }
                _ => {}
            }
        }
        out.reverse();
        out
    }

    /// Reorder the line's fragments with the Unicode Bidirectional
    /// Algorithm (UAX #9): levels from the paragraph direction, the
    /// characters and the `unicode-bidi` / `direction` of inline elements;
    /// then runs of increasing level are reversed (L2) by mirroring their
    /// positions within the span they occupy.
    fn bidi_line(&mut self) {
        use crate::bidi::*;
        let base_rtl = self.cstyle.direction == Direction::Rtl;
        let container_bidi = self.cstyle.unicode_bidi;
        let chains: Vec<Vec<usize>> = self
            .cur
            .frags
            .iter()
            .map(|f| match &f.kind {
                _ if !self.bidi_items => Vec::new(),
                PFKind::Text { item, .. } => self.bidi_chain(*item),
                PFKind::Box(c) => self.items.iter().position(|it| matches!(it, InlineItem::Atomic(a) if a == c)).map(|i| self.bidi_chain(i)).unwrap_or_default(),
            })
            .collect();
        let explicit = chains.iter().any(|c| !c.is_empty()) || matches!(container_bidi, UnicodeBidi::BidiOverride | UnicodeBidi::IsolateOverride | UnicodeBidi::Plaintext);
        let rtl_text = self.cur.frags.iter().any(|f| matches!(&f.kind, PFKind::Text { text, .. } if needs_bidi(text)));
        if !explicit && !rtl_text {
            if base_rtl {
                self.mirror_line();
            }
            return;
        }
        // The line's characters with the controls of the inline elements.
        let mut chars: Vec<char> = Vec::new();
        // (fragment, char offset within it) of each non-control character.
        let mut origin: Vec<Option<(usize, usize)>> = Vec::new();
        let push = |chars: &mut Vec<char>, origin: &mut Vec<Option<(usize, usize)>>, c: char, o: Option<(usize, usize)>| {
            chars.push(c);
            origin.push(o);
        };
        if matches!(container_bidi, UnicodeBidi::BidiOverride | UnicodeBidi::IsolateOverride) {
            push(&mut chars, &mut origin, if base_rtl { RLO_CH } else { LRO_CH }, None);
        }
        let mut open: Vec<usize> = Vec::new();
        for (fi, f) in self.cur.frags.iter().enumerate() {
            let chain = &chains[fi];
            let common = open.iter().zip(chain.iter()).take_while(|(a, b)| a == b).count();
            while open.len() > common {
                let it = open.pop().unwrap();
                for c in self.bidi_controls(it).1 {
                    push(&mut chars, &mut origin, c, None);
                }
            }
            for &it in &chain[common..] {
                for c in self.bidi_controls(it).0 {
                    push(&mut chars, &mut origin, c, None);
                }
                open.push(it);
            }
            match &f.kind {
                PFKind::Text { text, .. } => {
                    for (k, c) in text.chars().enumerate() {
                        push(&mut chars, &mut origin, c, Some((fi, k)));
                    }
                }
                PFKind::Box(_) => push(&mut chars, &mut origin, '\u{FFFC}', Some((fi, 0))),
            }
        }
        let base_rtl = if container_bidi == UnicodeBidi::Plaintext { first_strong(&chars).unwrap_or(base_rtl) } else { base_rtl };
        let base = base_rtl as u8;
        let mut levels = resolve_levels(&chars, base);
        reset_line_end(&chars, &mut levels, base);
        // Per-fragment character levels.
        let mut frag_levels: Vec<Vec<u8>> = self
            .cur
            .frags
            .iter()
            .map(|f| match &f.kind {
                PFKind::Text { text, .. } => vec![base; text.chars().count()],
                PFKind::Box(_) => vec![base],
            })
            .collect();
        for (i, o) in origin.iter().enumerate() {
            if let Some((fi, k)) = o {
                frag_levels[*fi][*k] = levels[i];
            }
        }
        // Split text fragments into units of one level (leading / trailing
        // neutrals of right-to-left runs apart from the letters, which the
        // shaper orders on its own).
        let frags = std::mem::take(&mut self.cur.frags);
        let mut units: Vec<(PF, u8)> = Vec::new();
        for (fi, f) in frags.into_iter().enumerate() {
            let lv = &frag_levels[fi];
            let (text, item) = match &f.kind {
                PFKind::Text { text, item } => (text.clone(), *item),
                PFKind::Box(_) => {
                    units.push((f, lv[0]));
                    continue;
                }
            };
            let cs: Vec<(usize, char)> = text.char_indices().collect();
            // Cut points: level changes, and the edges of each odd-level
            // run's right-to-left letters.
            let mut cuts: Vec<usize> = vec![0];
            let mut i = 0;
            while i < cs.len() {
                let mut j = i;
                while j < cs.len() && lv[j] == lv[i] {
                    j += 1;
                }
                if lv[i] % 2 == 1 {
                    let strong = |k: usize| matches!(class(cs[k].1), BidiClass::R | BidiClass::AL);
                    if let (Some(a), Some(b)) = ((i..j).find(|&k| strong(k)), (i..j).rev().find(|&k| strong(k))) {
                        cuts.push(a);
                        cuts.push(b + 1);
                    }
                }
                cuts.push(j);
                i = j;
            }
            cuts.sort_unstable();
            cuts.dedup();
            let style = self.style(item).clone();
            let font = self.lay.font_for(&style);
            let ls = style.letter_spacing;
            let mut x = f.x;
            for w in cuts.windows(2) {
                let (a, b) = (w[0], w[1]);
                if a >= b {
                    continue;
                }
                let start = cs[a].0;
                let end = if b < cs.len() { cs[b].0 } else { text.len() };
                let piece = &text[start..end];
                let width = if a == 0 && b >= cs.len() {
                    f.w
                } else if b >= cs.len() {
                    (f.x + f.w - x).max(0.0)
                } else {
                    self.lay.measurer.width(font, style.font_size, piece) + ls * piece.chars().count() as f32
                };
                let level = lv[a];
                // The run's direction for the shaper: an RLO prefix shapes
                // it right to left (visual order), an LRO prefix keeps
                // right-to-left letters in logical order.
                let has_rtl = piece.chars().any(|c| matches!(class(c), BidiClass::R | BidiClass::AL));
                let shown: String = if level % 2 == 1 {
                    format!("{RLO_CH}{piece}")
                } else if has_rtl {
                    format!("{LRO_CH}{piece}")
                } else {
                    piece.to_string()
                };
                units.push((PF { x, w: width, kind: PFKind::Text { text: shown, item } }, level));
                x += width;
            }
        }
        // L2: reverse runs of level ≥ k, highest k first. A whole-line
        // reversal (right-to-left paragraphs) mirrors the line box.
        let max = units.iter().map(|u| u.1).max().unwrap_or(0);
        let mut k = max;
        while k >= 1 {
            if k == 1 && base_rtl {
                self.cur.frags = units.iter().map(|u| u.0.clone()).collect();
                self.mirror_line();
                for (u, f) in units.iter_mut().zip(self.cur.frags.iter()) {
                    u.0.x = f.x;
                }
                break;
            }
            let mut idx: Vec<usize> = (0..units.len()).collect();
            idx.sort_by(|&a, &b| units[a].0.x.total_cmp(&units[b].0.x));
            let mut i = 0;
            while i < idx.len() {
                if units[idx[i]].1 < k {
                    i += 1;
                    continue;
                }
                let mut j = i;
                while j < idx.len() && units[idx[j]].1 >= k {
                    j += 1;
                }
                let s = units[idx[i]].0.x;
                let e = units[idx[j - 1]].0.x + units[idx[j - 1]].0.w;
                for &u in &idx[i..j] {
                    let f = &mut units[u].0;
                    f.x = s + e - (f.x + f.w);
                }
                i = j;
            }
            k -= 1;
        }
        self.cur.frags = units.into_iter().map(|u| u.0).collect();
    }

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
        self.bidi_line();

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
                    let (mut above, mut below) = (tm.above + shift, tm.below - shift);
                    // Emphasis marks need room beside the glyphs.
                    if let Some(e) = emphasis_of(&style) {
                        let m = self.lay.ctx.fonts.metrics(tm.font, tm.size / 2.0);
                        let mh = m.ascent + m.descent;
                        if e.over {
                            above = above.max(tm.asc + shift + mh);
                        } else {
                            below = below.max(tm.desc - shift + mh);
                        }
                    }
                    FM {
                        above,
                        below,
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

        // Horizontal alignment. Preserved spaces at the end of a `pre-wrap`
        // line hang: what overflows the line does not count.
        let mut hang = 0.0;
        let trailing = match self.cur.frags.last() {
            Some(PF { kind: PFKind::Text { text, item }, w, .. }) if text.ends_with(' ') => Some((*item, text.chars().rev().take_while(|c| *c == ' ').count(), *w)),
            _ => None,
        };
        if let Some((item, n, w)) = trailing {
            let st = self.style(item).clone();
            if st.white_space == rabbithole_style::WhiteSpace::PreWrap {
                let font = self.lay.font_for(&st);
                let sw = self.lay.measurer.space_width(font, st.font_size) + st.letter_spacing;
                let spaces = (sw * n as f32).min(w);
                // At a soft wrap they hang unconditionally; before a forced
                // break (or the end of the paragraph) only what overflows.
                hang = if forced || self.final_flush { (self.cur.x - self.cur.right).clamp(0.0, spaces) } else { spaces };
            }
        }
        let used = self.cur.x - self.cur.left - hang;
        self.max_used = self.max_used.max(used);
        let free = (self.cur.align_right - self.cur.left) - used;
        let rtl = self.cstyle.direction == Direction::Rtl;
        let shift_x = if rtl {
            // Content was mirrored to the right edge (start); `right` means
            // the end (left) edge.
            match self.cstyle.text_align {
                TextAlign::End | TextAlign::Left => -free.max(0.0),
                TextAlign::Center => -(free / 2.0).max(0.0),
                _ => 0.0,
            }
        } else {
            match self.cstyle.text_align {
                TextAlign::End | TextAlign::Right => free.max(0.0),
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
            let (rx, ry) = self.rel_of(bg.item);
            fragments.push(Fragment {
                rect: Rect::new(bg.x + shift_x + rx, baseline - tm.asc - pt + ry, bg.w, tm.asc + tm.desc + pt + pb),
                baseline: baseline + ry,
                node: item_elem(self.items, bg.item),
                inert: style.pointer_events == rabbithole_style::PointerEvents::None,
                kind: FragmentKind::InlineBox {
                    style,
                    left_edge: bg.left_edge,
                    right_edge: bg.right_edge,
                },
            });
        }
        // Extents (on this line) of the inline elements that inset their
        // decorations.
        let mut box_ext: std::collections::HashMap<usize, (f32, f32)> = std::collections::HashMap::new();
        if !self.parent_open.is_empty() {
            let block = has_inset(self.cstyle);
            for f in &self.cur.frags {
                if let PFKind::Text { item, .. } = &f.kind {
                    if block && *item != MARKER_ITEM {
                        let e = box_ext.entry(BLOCK_BOX).or_insert((f.x, f.x + f.w));
                        e.0 = e.0.min(f.x);
                        e.1 = e.1.max(f.x + f.w);
                    }
                    let mut p = self.parent_open.get(*item).copied().flatten();
                    while let Some(o) = p {
                        let e = box_ext.entry(o).or_insert((f.x, f.x + f.w));
                        e.0 = e.0.min(f.x);
                        e.1 = e.1.max(f.x + f.w);
                        p = self.parent_open[o];
                    }
                }
            }
        }
        let frags = std::mem::take(&mut self.cur.frags);
        for (f, fm) in frags.into_iter().zip(fms.into_iter()) {
            match f.kind {
                PFKind::Text { text, item } => {
                    let tm = fm.tm.expect("text metrics");
                    let style = self.style(item).clone();
                    let (rx, ry) = if item == MARKER_ITEM { (0.0, 0.0) } else { self.rel_of(item) };
                    let bl = baseline - fm.shift + ry;
                    let oblique = self.lay.synthetic_oblique(tm.font, &style);
                    fragments.push(Fragment {
                        rect: Rect::new(f.x + shift_x + rx, bl - tm.asc, f.w, tm.asc + tm.desc),
                        baseline: bl,
                        node: if item == MARKER_ITEM { None } else { item_elem(self.items, item) },
                        inert: style.pointer_events == rabbithole_style::PointerEvents::None,
                        kind: FragmentKind::Text {
                            text: Rc::from(text.as_str()),
                            font: tm.font,
                            size: tm.size,
                            color: style.color,
                            letter_spacing: style.letter_spacing,
                            hidden: style.visibility == Visibility::Hidden,
                            decoration_info: DecorationInfo {
                                list: style.applied_decorations.clone(),
                                extents: if box_ext.is_empty() || item == MARKER_ITEM { Vec::new() } else { self.decoration_extents(item, &style, &box_ext, f.x, f.w, fragments.len()) },
                                metrics: style
                                    .applied_decorations
                                    .iter()
                                    .map(|d| {
                                        let same = d.font_size == style.font_size
                                            && d.font_family == style.font_family
                                            && d.font_weight == style.font_weight
                                            && d.font_style == style.font_style
                                            && d.font_family_list == style.font_family_list;
                                        let (a, de) = if same {
                                            (tm.asc, tm.desc)
                                        } else {
                                            let mut s = ComputedStyle::initial();
                                            s.font_size = d.font_size;
                                            s.font_family = d.font_family;
                                            s.font_family_list = d.font_family_list.clone();
                                            s.font_weight = d.font_weight;
                                            s.font_style = d.font_style;
                                            let m = self.lay.text_metrics(&s);
                                            (m.asc, m.desc)
                                        };
                                        (fm.shift - d.baseline_shift, a, de)
                                    })
                                    .collect(),
                                variations: if style.applied_decorations.is_empty() { Vec::new() } else { style.font_variation_settings.clone() },
                            },
                            shadows: style.text_shadow.clone(),
                            small_caps: style.font_variant_small_caps,
                            oblique,
                            emphasis: if item == MARKER_ITEM { None } else { emphasis_of(&style) },
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
                    let (rx, ry) = self.rel_kids.get(c).copied().unwrap_or((0.0, 0.0));
                    child.translate(f.x + shift_x + m.left + rx - child.rect.x, top + m.top + ry - child.rect.y);
                    let node = child.node;
                    let rect = child.margin_rect();
                    fragments.push(Fragment {
                        rect,
                        baseline,
                        node,
                        inert: false,
                        kind: FragmentKind::Box(c),
                    });
                }
            }
        }
        for (o, (lo, hi)) in &box_ext {
            self.inset_seen.insert(*o);
            *self.inset_total.entry(*o).or_insert(0.0) += hi - lo;
        }
        self.lines.push(LineBox {
            rect: Rect::new(self.cur.left, y, (self.cur.align_right - self.cur.left).max(0.0), h),
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


