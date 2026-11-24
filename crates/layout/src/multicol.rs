//! CSS Multi-column Layout 1.
//!
//! A multi-column container lays its contents out once as a single
//! column as wide as its columns (the *flow thread*), then fragments that
//! flow into columns (CSS Fragmentation 3):
//!
//! 1. The flow is flattened into break-relevant events in document order:
//!    the starts and ends of fragmentable block containers, and
//!    unbreakable pieces (line boxes, monolithic boxes, floats).
//! 2. A column height is chosen: balanced (the smallest height for which
//!    the flow fits in the columns) or the container's available height.
//! 3. Pieces that would cross the end of a column are pushed to the start
//!    of the next one (a *strut*), honouring forced breaks, `break-*:
//!    avoid`, `orphans` and `widows`. Margins adjoining an unforced break
//!    are truncated.
//! 4. Each piece moves to its column; block containers crossing column
//!    ends get one fragment per column ([`ColPieces`]), painted as slices
//!    of the unfragmented box (`box-decoration-break: slice`) and reported
//!    by `getClientRects()`.
//!
//! `column-span: all` children split the contents into column sets laid
//! out (and balanced) separately, with the spanners between them.

use crate::block::{Cb, Layouter, WidthMode};
use crate::float::Bfc;
use crate::tree::{BoxKind, Flow, Formatting, FragmentKind, LayoutBox};
use rabbithole_gfx::Rect;
use rabbithole_style::props::multicol as mc;
use rabbithole_style::ComputedStyle;
use std::rc::Rc;

/// A fragment of a box broken across columns.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ColPiece {
    /// The fragment's border box (document coordinates).
    pub rect: Rect,
    /// Translation from the flow thread to the fragment's column.
    pub dx: f32,
    pub dy: f32,
}

/// The fragments of a box broken across columns, with its unfragmented
/// geometry in the flow thread (painting translates it into each column
/// and clips it to the fragment).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ColPieces {
    pub border: Rect,
    pub padding: Rect,
    pub content: Rect,
    pub pieces: Vec<ColPiece>,
}

impl ColPieces {
    pub(crate) fn translate(&mut self, dx: f32, dy: f32) {
        self.border = self.border.translate(dx, dy);
        self.padding = self.padding.translate(dx, dy);
        self.content = self.content.translate(dx, dy);
        for p in &mut self.pieces {
            p.rect = p.rect.translate(dx, dy);
        }
    }
}

/// Used column count and width (Multicol 1 §3.4).
pub(crate) fn used_columns(count: Option<u32>, width: Option<f32>, avail: f32, gap: f32) -> (u32, f32) {
    let avail = avail.max(0.0);
    let n = match (width, count) {
        (None, Some(n)) => n.max(1),
        (Some(w), count) => {
            let fit = ((avail + gap) / (w.max(1.0) + gap)).floor().max(1.0);
            let fit = if fit.is_finite() { fit as u32 } else { 1 };
            count.map_or(fit, |c| c.min(fit)).max(1)
        }
        (None, None) => 1,
    };
    let w = ((avail - (n as f32 - 1.0) * gap) / n as f32).max(0.0);
    (n, w)
}

/// The used `column-gap` of a multicol container (`normal` = 1em).
pub(crate) fn used_gap(style: &ComputedStyle, basis: f32) -> f32 {
    if style.column_gap_normal {
        style.font_size
    } else {
        style.column_gap.resolve_or_zero(basis).max(0.0)
    }
}

/// Intrinsic (min, max) content widths of a multicol container from those
/// of its contents (the old CSS Sizing 3 definition browsers follow).
pub(crate) fn intrinsic(style: &ComputedStyle, (mut min, mut max): (f32, f32)) -> (f32, f32) {
    if let Some(w) = style.column_width {
        min = min.min(w);
        max = max.max(w).max(min);
    }
    if let Some(n) = style.column_count {
        let gap = used_gap(style, 0.0);
        max = max * n as f32 + gap * (n as f32 - 1.0);
    }
    (min, max.max(min))
}

/// A box whose contents the fragmentation descends into (a block
/// container in the same formatting context); other boxes are monolithic.
fn fragmentable(c: &LayoutBox) -> bool {
    c.flow == Flow::InFlow
        && matches!(c.kind, BoxKind::Block | BoxKind::AnonymousBlock)
        && matches!(c.formatting, Formatting::Block | Formatting::Inline)
        && !c.clips
        && !c.wm_wrapper
        && !c.style.multicol
        && !c.style.has_transform()
        && c.style.line_clamp.is_none()
        && !(c.style.contain_size || c.style.contain_paint || c.style.contain_layout)
}

const NONE: u32 = u32::MAX;

#[derive(Debug, Clone, Copy)]
struct LineInfo {
    idx: u32,
    count: u32,
    orphans: u16,
    widows: u16,
}

#[derive(Debug, Clone, Copy)]
enum Ev {
    /// A fragmentable block container begins (its border edge).
    Start { top: f32, h: f32, edge: bool, forced: bool, avoid_before: bool, avoid_inside: bool, prev: u32, mt: f32 },
    End,
    /// An unbreakable piece (line box, monolithic box, float).
    Leaf { top: f32, bottom: f32, forced: bool, avoid_before: bool, prev: u32, mt: f32, line: Option<LineInfo> },
}

impl Ev {
    fn top(&self) -> Option<f32> {
        match *self {
            Ev::Start { top, .. } | Ev::Leaf { top, .. } => Some(top),
            Ev::End => None,
        }
    }
}

/// The order in which an inline container's lines and floats are met.
fn inline_order(b: &LayoutBox) -> Vec<(f32, bool, usize)> {
    let mut v: Vec<(f32, bool, usize)> = b.lines.iter().enumerate().map(|(i, l)| (l.rect.y, true, i)).collect();
    for (i, c) in b.children.iter().enumerate() {
        if c.flow.is_float() && !c.clamped {
            v.push((c.margin_rect().y, false, i));
        }
    }
    if v.iter().any(|e| !e.1) {
        // Floats sit at the top of the line they were met on.
        v.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    }
    v
}

#[derive(Default)]
struct Collector {
    evs: Vec<Ev>,
}

impl Collector {
    fn inline(&mut self, b: &LayoutBox) {
        let count = b.lines.len() as u32;
        let mut idx = 0;
        for (_, line, i) in inline_order(b) {
            if line {
                let r = b.lines[i].rect;
                self.evs.push(Ev::Leaf {
                    top: r.y,
                    bottom: r.bottom(),
                    forced: false,
                    avoid_before: false,
                    prev: NONE,
                    mt: 0.0,
                    line: Some(LineInfo { idx, count, orphans: b.style.orphans, widows: b.style.widows }),
                });
                idx += 1;
            } else {
                let r = b.children[i].margin_rect();
                self.evs.push(Ev::Leaf { top: r.y, bottom: r.bottom(), forced: false, avoid_before: false, prev: NONE, mt: 0.0, line: None });
            }
        }
    }

    fn contents(&mut self, b: &LayoutBox) {
        match b.formatting {
            Formatting::Inline => self.inline(b),
            _ => self.block(b),
        }
    }

    fn block(&mut self, b: &LayoutBox) {
        let mut prev = NONE;
        let (mut forced_after, mut avoid_after) = (false, false);
        for c in &b.children {
            if c.clamped {
                continue;
            }
            match c.flow {
                Flow::Absolute => {}
                Flow::FloatLeft | Flow::FloatRight => {
                    let r = c.margin_rect();
                    self.evs.push(Ev::Leaf { top: r.y, bottom: r.bottom(), forced: false, avoid_before: false, prev: NONE, mt: 0.0, line: None });
                }
                Flow::InFlow => {
                    let s = &c.style;
                    let forced = forced_after || mc::is_forced_break(s.break_before);
                    let avoid_before = avoid_after || mc::avoids_column_break(s.break_before);
                    let here = self.evs.len() as u32;
                    if fragmentable(c) {
                        let edge = s.border_width.top > 0.0 || c.content_rect.y > c.rect.y + 0.01;
                        self.evs.push(Ev::Start {
                            top: c.rect.y,
                            h: c.rect.h,
                            edge,
                            forced,
                            avoid_before,
                            avoid_inside: mc::avoids_column_break(s.break_inside),
                            prev,
                            mt: c.margin.top,
                        });
                        self.contents(c);
                        self.evs.push(Ev::End);
                    } else {
                        self.evs.push(Ev::Leaf { top: c.rect.y, bottom: c.rect.bottom(), forced, avoid_before, prev, mt: c.margin.top, line: None });
                    }
                    prev = here;
                    forced_after = mc::is_forced_break(s.break_after);
                    avoid_after = mc::avoids_column_break(s.break_after);
                }
            }
        }
    }
}

/// Where the flow's pieces go for one column height.
struct Plan {
    /// Strut before each event (moves it and what follows down the flow).
    struts: Vec<f32>,
    /// Columns holding content.
    used: usize,
    /// Smallest column height increase that would avoid a break that was
    /// taken (balancing), `INFINITY` when none would.
    shortfall: f32,
}

struct Geometry {
    y0: f32,
    colh: f32,
}

impl Geometry {
    fn col(&self, t: f32) -> usize {
        let k = ((t - self.y0) / self.colh + 1e-4).floor();
        if k > 0.0 {
            k as usize
        } else {
            0
        }
    }
    fn start(&self, k: usize) -> f32 {
        self.y0 + k as f32 * self.colh
    }
    fn end(&self, k: usize) -> f32 {
        self.start(k + 1)
    }
}

const EPS: f32 = 0.01;

fn paginate(evs: &[Ev], g: &Geometry) -> Plan {
    let n = evs.len();
    let mut struts = vec![0.0f32; n + 1];
    // (origin event, strut index, amount)
    let mut pushes: Vec<(usize, usize, f32)> = Vec::new();
    let mut offsets = vec![0.0f32; n + 1];
    let mut placed_at = vec![false; n + 1];
    let mut pos = vec![f32::NAN; n];
    let mut off = 0.0f32;
    let mut placed = false;
    let mut force_at = usize::MAX;
    let mut shortfall = f32::INFINITY;
    let mut budget = 4 * n + 64;
    let mut used = 0usize;
    let mut i = 0;
    // Push event `i` (at flow position `t`) so that it starts at `target`;
    // the strut goes before the outermost container starting right there
    // without a border or padding of its own.
    let push = |i: usize, t: f32, target: f32, off: &mut f32, struts: &mut Vec<f32>, pushes: &mut Vec<(usize, usize, f32)>| {
        let amount = target - t;
        if amount <= 0.0 {
            return;
        }
        let top = evs[i].top().unwrap_or(t);
        let mut at = i;
        while at > 0 {
            match evs[at - 1] {
                Ev::Start { top: st, edge: false, .. } if (st - top).abs() < EPS => at -= 1,
                _ => break,
            }
        }
        struts[at] += amount;
        pushes.push((i, at, amount));
        *off += amount;
    };
    while i < n {
        if budget == 0 {
            break;
        }
        budget -= 1;
        offsets[i] = off;
        placed_at[i] = placed;
        match evs[i] {
            Ev::End => {}
            Ev::Start { top, h, edge, forced, avoid_before, avoid_inside, prev, mt } => {
                let t = top + off;
                let k = g.col(t);
                if force_at == i || (forced && placed) {
                    if t > g.y0 + EPS {
                        let k = g.col(t - EPS);
                        let target = g.start(k + 1) + if force_at == i { 0.0 } else { mt.max(0.0) };
                        push(i, t, target, &mut off, &mut struts, &mut pushes);
                    }
                } else if avoid_inside && h <= g.colh + EPS && t + h > g.end(k) + EPS && t > g.start(k) + EPS {
                    shortfall = shortfall.min(t + h - g.end(k));
                    if let Some(j) = earlier_break(evs, &pos, g, i, avoid_before, prev) {
                        backtrack(j, &mut i, &mut off, &offsets, &mut placed, &placed_at, &mut struts, &mut pushes, &mut force_at);
                        continue;
                    }
                    push(i, t, g.end(k), &mut off, &mut struts, &mut pushes);
                }
                if force_at == i {
                    force_at = usize::MAX;
                }
                pos[i] = top + off;
                placed |= edge;
            }
            Ev::Leaf { top, bottom, forced, avoid_before, prev, mt, line } => {
                let t = top + off;
                let bt = bottom + off;
                let k = g.col(t);
                if force_at == i || (forced && placed) {
                    if t > g.y0 + EPS {
                        let k = g.col(t - EPS);
                        let target = g.start(k + 1) + if force_at == i { 0.0 } else { mt.max(0.0) };
                        push(i, t, target, &mut off, &mut struts, &mut pushes);
                    }
                } else if bt > g.end(k) + EPS && t > g.start(k) + EPS {
                    shortfall = shortfall.min(bt - g.end(k));
                    let earlier = earlier_break(evs, &pos, g, i, avoid_before, prev).or_else(|| line.and_then(|l| line_break(evs, &pos, g, i, k, l)));
                    if let Some(j) = earlier {
                        backtrack(j, &mut i, &mut off, &offsets, &mut placed, &placed_at, &mut struts, &mut pushes, &mut force_at);
                        continue;
                    }
                    push(i, t, g.end(k), &mut off, &mut struts, &mut pushes);
                }
                if force_at == i {
                    force_at = usize::MAX;
                }
                let (t, bt) = (top + off, bottom + off);
                pos[i] = t;
                let k = g.col(t);
                if bt > t {
                    placed = true;
                    used = used.max(k + 1);
                }
                if bt - t > g.colh + EPS {
                    // Taller than a column: it overflows its column, and
                    // what follows starts in the next one.
                    shortfall = shortfall.min(bt - t - g.colh);
                    let next = g.end(k);
                    if bt > next && i + 1 < n {
                        let amount = next - bt;
                        struts[i + 1] += amount;
                        pushes.push((i, i + 1, amount));
                        off += amount;
                    }
                }
            }
        }
        i += 1;
    }
    struts.truncate(n);
    Plan { struts, used, shortfall }
}

#[allow(clippy::too_many_arguments)]
fn backtrack(j: usize, i: &mut usize, off: &mut f32, offsets: &[f32], placed: &mut bool, placed_at: &[bool], struts: &mut [f32], pushes: &mut Vec<(usize, usize, f32)>, force_at: &mut usize) {
    pushes.retain(|&(origin, at, amount)| {
        if origin >= j {
            struts[at] -= amount;
            false
        } else {
            true
        }
    });
    *off = offsets[j];
    *placed = placed_at[j];
    *force_at = j;
    *i = j;
}

/// `break-before: avoid` (or the previous sibling's `break-after:
/// avoid`): break before the previous sibling instead, unless it starts
/// its column.
fn earlier_break(evs: &[Ev], pos: &[f32], g: &Geometry, i: usize, avoid_before: bool, prev: u32) -> Option<usize> {
    if !avoid_before || prev == NONE {
        return None;
    }
    let j = prev as usize;
    let t = pos[j];
    let _ = evs;
    (t.is_finite() && t > g.start(g.col(t)) + EPS).then_some(j)
}

/// `orphans` / `widows` (CSS Fragmentation 3 §4.4): a break before line
/// `l.idx` of a block that would leave too few lines on either side moves
/// to an earlier line when that satisfies both.
fn line_break(evs: &[Ev], pos: &[f32], g: &Geometry, i: usize, k: usize, l: LineInfo) -> Option<usize> {
    if l.idx == 0 {
        return None;
    }
    // The block's earlier lines in this column, newest first.
    let mut here: Vec<(usize, u32)> = Vec::new();
    let mut j = i;
    while j > 0 {
        j -= 1;
        match evs[j] {
            Ev::Leaf { line: Some(o), .. } => {
                if g.col(pos[j]) != k {
                    break;
                }
                here.push((j, o.idx));
                if o.idx == 0 {
                    break;
                }
            }
            Ev::Leaf { line: None, .. } => {}
            _ => break,
        }
    }
    let first = here.last().map(|e| e.1)?;
    let before = l.idx - first;
    let orphans = l.orphans.max(1) as u32;
    let widows = l.widows.max(1) as u32;
    if before < orphans {
        // Too few lines would stay: move the whole block's lines on, if
        // they started in this column below its top.
        let (j, idx) = *here.last()?;
        return (idx == 0 && pos[j] > g.start(k) + EPS).then_some(j);
    }
    let after = l.count - l.idx;
    if after < widows {
        let want = l.count.saturating_sub(widows);
        if want > first && want - first >= orphans && want < l.idx {
            return here.iter().find(|e| e.1 == want).map(|e| e.0);
        }
    }
    None
}

/// Moves the flow's pieces into their columns.
struct Mover<'p> {
    struts: &'p [f32],
    i: usize,
    off: f32,
    g: Geometry,
    step: f32,
    /// Nesting depth of positioned containers inside the flow (their
    /// absolutely positioned descendants move with them).
    positioned: u32,
    /// The translation of the first piece placed in the current container
    /// (for its list marker).
    first: Option<(f32, f32)>,
    /// Columns that received content.
    used: usize,
}

impl Mover<'_> {
    fn next(&mut self) {
        self.off += self.struts.get(self.i).copied().unwrap_or(0.0);
        self.i += 1;
    }

    /// Translation of a piece at flow position `t` (strip coordinates)
    /// into its column.
    fn shift(&mut self, top: f32) -> (f32, f32) {
        let t = top + self.off;
        let k = self.g.col(t);
        self.used = self.used.max(k + 1);
        let d = (k as f32 * self.step, self.off - k as f32 * self.g.colh);
        if self.first.is_none() {
            self.first = Some(d);
        }
        d
    }

    fn move_abs(&mut self, c: &mut LayoutBox) {
        if self.positioned > 0 && !c.fixed && c.style.position != rabbithole_style::Position::Fixed {
            let t = c.rect.y + self.off;
            let k = self.g.col(t);
            c.translate(k as f32 * self.step, self.off - k as f32 * self.g.colh);
        }
    }

    fn inline(&mut self, b: &mut LayoutBox) {
        for (_, line, i) in inline_order(b) {
            self.next();
            if line {
                let (dx, dy) = self.shift(b.lines[i].rect.y);
                let l = &mut b.lines[i];
                l.rect = l.rect.translate(dx, dy);
                l.baseline += dy;
                let mut boxes = Vec::new();
                for f in &mut l.fragments {
                    f.rect = f.rect.translate(dx, dy);
                    f.baseline += dy;
                    if let FragmentKind::Box(c) = f.kind {
                        boxes.push(c);
                    }
                }
                for c in boxes {
                    if let Some(c) = b.children.get_mut(c) {
                        c.translate(dx, dy);
                    }
                }
            } else {
                let (dx, dy) = self.shift(b.children[i].margin_rect().y);
                b.children[i].translate(dx, dy);
            }
        }
        for c in &mut b.children {
            if c.flow == Flow::Absolute {
                self.move_abs(c);
            }
        }
    }

    fn contents(&mut self, b: &mut LayoutBox) {
        match b.formatting {
            Formatting::Inline => self.inline(b),
            _ => self.block(b),
        }
        if let Some((dx, dy)) = self.first {
            if let Some(m) = &mut b.marker {
                m.x += dx;
                m.baseline += dy;
            }
        }
    }

    fn block(&mut self, b: &mut LayoutBox) {
        for c in &mut b.children {
            if c.clamped {
                continue;
            }
            match c.flow {
                Flow::Absolute => self.move_abs(c),
                Flow::FloatLeft | Flow::FloatRight => {
                    self.next();
                    let (dx, dy) = self.shift(c.margin_rect().y);
                    c.translate(dx, dy);
                }
                Flow::InFlow if fragmentable(c) => {
                    self.next();
                    let start = c.rect.y + self.off;
                    let outer_first = self.first.take();
                    let positioned = crate::block::contains_positioned(&c.style);
                    self.positioned += positioned as u32;
                    self.contents(c);
                    self.positioned -= positioned as u32;
                    self.next();
                    let end = (c.rect.bottom() + self.off).max(start);
                    self.fragment(c, start, end);
                    self.first = outer_first.or(self.first);
                }
                Flow::InFlow => {
                    self.next();
                    let (dx, dy) = self.shift(c.rect.y);
                    c.translate(dx, dy);
                }
            }
        }
    }

    /// Place a fragmentable container spanning `[start, end)` of the
    /// flow (strut-adjusted strip coordinates); its contents were moved.
    fn fragment(&mut self, c: &mut LayoutBox, start: f32, end: f32) {
        let g = &self.g;
        let k0 = g.col(start);
        let k1 = g.col((end - EPS).max(start)).max(k0);
        let growth = (end - start) - c.rect.h;
        let content = Rect::new(c.content_rect.x, c.content_rect.y + (start - c.rect.y), c.content_rect.w, (c.content_rect.h + growth).max(0.0));
        let border = Rect::new(c.rect.x, start, c.rect.w, end - start);
        if k0 == k1 {
            let (dx, dy) = (k0 as f32 * self.step, -(k0 as f32) * g.colh);
            c.rect = border.translate(dx, dy);
            c.content_rect = content.translate(dx, dy);
            if growth.abs() > EPS {
                c.layout_cache = None;
            }
            c.col_pieces = None;
            return;
        }
        self.used = self.used.max(k1 + 1);
        let bw = c.style.border_width;
        let padding = Rect::new(border.x + bw.left, border.y + bw.top, (border.w - bw.left - bw.right).max(0.0), (border.h - bw.top - bw.bottom).max(0.0));
        let mut pieces = Vec::new();
        let mut union: Option<Rect> = None;
        let mut content_union: Option<Rect> = None;
        for k in k0..=k1 {
            let lo = if k == k0 { start } else { g.start(k) };
            let hi = if k == k1 { end } else { g.end(k) };
            let (dx, dy) = (k as f32 * self.step, -(k as f32) * g.colh);
            let rect = Rect::new(border.x + dx, lo + dy, border.w, (hi - lo).max(0.0));
            union = Some(union.map_or(rect, |u| u.union(&rect)));
            let (clo, chi) = (lo.max(content.y), hi.min(content.bottom()));
            if chi >= clo {
                let cr = Rect::new(content.x + dx, clo + dy, content.w, chi - clo);
                content_union = Some(content_union.map_or(cr, |u| u.union(&cr)));
            }
            pieces.push(ColPiece { rect, dx, dy });
        }
        c.rect = union.unwrap_or(border);
        c.content_rect = content_union.unwrap_or(content);
        c.col_pieces = Some(Box::new(ColPieces { border, padding, content, pieces }));
        // Its contents are no longer where a translation of the box would
        // put them.
        c.layout_cache = None;
        c.contents_fp.set(None);
    }
}

/// The result of fragmenting one column set.
struct ColumnSet {
    /// Column height.
    height: f32,
    /// Columns holding content.
    used: usize,
}

impl Layouter<'_> {
    /// Lay out the contents of a multi-column container (`b` is a block
    /// container with `column-count` or `column-width`); returns their
    /// height. `cb` is its content box.
    pub(crate) fn layout_multicol(&mut self, b: &mut LayoutBox, cb: &Cb) -> f32 {
        let style = b.style.clone();
        let avail_w = cb.width;
        let gap = used_gap(&style, avail_w);
        let (n, w) = used_columns(style.column_count, style.column_width, avail_w, gap);
        let rtl = style.direction == rabbithole_style::Direction::Rtl;
        let x0 = if rtl { cb.x + avail_w - w } else { cb.x };
        let step = (w + gap) * if rtl { -1.0 } else { 1.0 };
        // The height available to the columns: a definite height, else the
        // maximum height.
        let pb_v = crate::block::vertical_extras(&style);
        let max_h = self.clamp_height(&style, f32::INFINITY, None, pb_v);
        let avail_h = cb.height.or(max_h.is_finite().then_some(max_h));
        let fill_auto = style.column_fill_auto && avail_h.is_some();
        let content_y = b.content_rect.y;
        let mut rules: Vec<Rect> = Vec::new();
        let spanners = b.formatting == Formatting::Block && b.children.iter().any(|c| c.flow == Flow::InFlow && c.style.column_span_all);
        let total = if !spanners {
            let saved = b.content_rect;
            b.content_rect.x = x0;
            b.content_rect.w = w;
            let mut local = Bfc::new();
            let flow_cb = Cb { x: x0, width: w, height: cb.height };
            let h = self.layout_flow_contents(b, &flow_cb, &mut local);
            let h = if local.is_empty() { h } else { h.max(local.max_bottom() - content_y) };
            b.content_rect.x = saved.x;
            b.content_rect.w = saved.w;
            let set = fragment_flow(b, content_y, h, n, if fill_auto { avail_h } else { None }, avail_h, step);
            push_rules(&mut rules, &style, x0, w, gap, step, content_y, set.height, set.used);
            set.height
        } else {
            self.layout_column_sets(b, cb, (x0, w, gap, step, n), &mut rules)
        };
        let paints_rules = style.column_rule_style.is_visible() && style.column_rule_width > 0.0;
        b.col_rules = (paints_rules && !rules.is_empty()).then(|| Box::new(rules));
        total
    }

    fn layout_flow_contents(&mut self, b: &mut LayoutBox, cb: &Cb, bfc: &mut Bfc) -> f32 {
        match b.formatting {
            Formatting::Inline => self.layout_inline(b, cb, bfc),
            _ => self.layout_block_children_of(b, cb, bfc),
        }
    }

    /// Column sets separated by `column-span: all` children (Multicol 1
    /// §6): each set is laid out and balanced on its own, spanners take
    /// the container's whole width between them.
    fn layout_column_sets(&mut self, b: &mut LayoutBox, cb: &Cb, (x0, w, gap, step, n): (f32, f32, f32, f32, u32), rules: &mut Vec<Rect>) -> f32 {
        let style = b.style.clone();
        let content_y = b.content_rect.y;
        let mut cursor = content_y;
        let children = std::mem::take(&mut b.children);
        let mut out: Vec<LayoutBox> = Vec::with_capacity(children.len());
        let mut run: Vec<LayoutBox> = Vec::new();
        let mut anon_style: Option<Rc<ComputedStyle>> = None;
        let mut finish_run = |this: &mut Self, run: &mut Vec<LayoutBox>, out: &mut Vec<LayoutBox>, cursor: &mut f32, rules: &mut Vec<Rect>| {
            if run.is_empty() {
                return;
            }
            let st = anon_style
                .get_or_insert_with(|| {
                    let mut s = ComputedStyle::inherit_from(&style);
                    s.display = rabbithole_style::Display::Block;
                    Rc::new(s)
                })
                .clone();
            let mut set = LayoutBox::new(BoxKind::AnonymousBlock, None, st);
            set.formatting = Formatting::Block;
            set.bfc_root = true;
            set.children = std::mem::take(run);
            set.rect = Rect::new(x0, *cursor, w, 0.0);
            set.content_rect = set.rect;
            let mut local = Bfc::new();
            let h = this.layout_block_children_of(&mut set, &Cb { x: x0, width: w, height: None }, &mut local);
            let h = if local.is_empty() { h } else { h.max(local.max_bottom() - *cursor) };
            let fs = fragment_flow(&mut set, *cursor, h, n, None, None, step);
            push_rules(rules, &style, x0, w, gap, step, *cursor, fs.height, fs.used);
            *cursor += fs.height;
            out.append(&mut set.children);
        };
        for mut c in children {
            if c.flow == Flow::InFlow && c.style.column_span_all {
                finish_run(self, &mut run, &mut out, &mut cursor, rules);
                let full = Cb { x: cb.x, width: cb.width, height: None };
                let mt = self.top_margins(&c, cb.width, None).resolve();
                let mut bfc = Bfc::new();
                self.layout_block(&mut c, &full, cursor + mt, &mut bfc, WidthMode::Fill);
                cursor = c.rect.bottom() + c.margin.bottom;
                out.push(c);
            } else {
                run.push(c);
            }
        }
        finish_run(self, &mut run, &mut out, &mut cursor, rules);
        b.children = out;
        (cursor - content_y).max(0.0)
    }
}

/// Choose the column height of a laid out flow (the contents of `root`,
/// starting at `y0`, `total` tall) and move its pieces into columns.
/// `fixed`: a column height to fill (`column-fill: auto`); `max`: the
/// largest column height.
fn fragment_flow(root: &mut LayoutBox, y0: f32, total: f32, n: u32, fixed: Option<f32>, max: Option<f32>, step: f32) -> ColumnSet {
    let mut collector = Collector::default();
    collector.contents(root);
    let evs = collector.evs;
    if evs.is_empty() || total <= 0.0 && fixed.is_none() {
        return ColumnSet { height: fixed.unwrap_or(0.0).max(0.0), used: (!evs.is_empty()) as usize };
    }
    let n = n.max(1) as usize;
    let floor = |h: f32| h.max(1.0);
    let (colh, plan) = match fixed {
        Some(h) => {
            let g = Geometry { y0, colh: floor(h) };
            (h.max(0.0), paginate(&evs, &g))
        }
        None => {
            // Balance: the smallest height that fits the flow in `n`
            // columns, starting from the average and growing by the least
            // amount that avoids a break taken.
            let mut h = (total / n as f32).max(0.0);
            let cap = max.unwrap_or(f32::INFINITY);
            let mut plan = paginate(&evs, &Geometry { y0, colh: floor(h.min(cap)) });
            let mut rounds = 0;
            while plan.used > n && plan.shortfall.is_finite() && h < cap && rounds < 64 {
                rounds += 1;
                h = (h + plan.shortfall.max(0.05)).min(cap);
                plan = paginate(&evs, &Geometry { y0, colh: floor(h) });
            }
            if plan.used > n && h < total && h < cap && rounds >= 64 {
                h = total.min(cap);
                plan = paginate(&evs, &Geometry { y0, colh: floor(h) });
            }
            (h.min(cap), plan)
        }
    };
    let mut mover = Mover { struts: &plan.struts, i: 0, off: 0.0, g: Geometry { y0, colh: floor(colh) }, step, positioned: 0, first: None, used: 0 };
    mover.contents(root);
    let used = mover.used.max(plan.used);
    ColumnSet { height: colh, used }
}

/// Column rules between adjacent used columns of a set (Multicol 1 §4):
/// centred in the gaps, as tall as the columns.
#[allow(clippy::too_many_arguments)]
fn push_rules(rules: &mut Vec<Rect>, style: &ComputedStyle, x0: f32, w: f32, gap: f32, step: f32, y: f32, h: f32, used: usize) {
    if !style.column_rule_style.is_visible() || style.column_rule_width <= 0.0 || h <= 0.0 {
        return;
    }
    let rw = style.column_rule_width;
    for k in 0..used.saturating_sub(1) {
        let col_x = x0 + k as f32 * step;
        let mid = if step >= 0.0 { col_x + w + gap / 2.0 } else { col_x - gap / 2.0 };
        rules.push(Rect::new(mid - rw / 2.0, y, rw, h));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn used_column_count_and_width() {
        assert_eq!(used_columns(Some(3), None, 320.0, 10.0), (3, 100.0));
        assert_eq!(used_columns(None, Some(100.0), 320.0, 10.0), (3, 100.0));
        assert_eq!(used_columns(None, Some(100.0), 330.0, 10.0), (3, 103.333336));
        assert_eq!(used_columns(Some(2), Some(100.0), 320.0, 10.0), (2, 155.0));
        // Narrower than one column: one column as wide as the space.
        assert_eq!(used_columns(None, Some(500.0), 320.0, 10.0), (1, 320.0));
    }

    fn leaf(top: f32, bottom: f32) -> Ev {
        Ev::Leaf { top, bottom, forced: false, avoid_before: false, prev: NONE, mt: 0.0, line: None }
    }

    #[test]
    fn pieces_crossing_a_column_end_move_to_the_next_column() {
        let evs = [leaf(0.0, 40.0), leaf(40.0, 80.0), leaf(80.0, 120.0)];
        let plan = paginate(&evs, &Geometry { y0: 0.0, colh: 100.0 });
        assert_eq!(plan.struts, vec![0.0, 0.0, 20.0]);
        assert_eq!(plan.used, 2);
        assert_eq!(plan.shortfall, 20.0);
    }

    #[test]
    fn widows_move_the_break_earlier() {
        let line = |idx: u32, top: f32| Ev::Leaf {
            top,
            bottom: top + 20.0,
            forced: false,
            avoid_before: false,
            prev: NONE,
            mt: 0.0,
            line: Some(LineInfo { idx, count: 5, orphans: 2, widows: 2 }),
        };
        let evs: Vec<Ev> = (0..5).map(|i| line(i, i as f32 * 20.0)).collect();
        // Four lines fit in a 90px column; the last one alone would be a
        // widow: the fourth line moves on too.
        let plan = paginate(&evs, &Geometry { y0: 0.0, colh: 90.0 });
        assert_eq!(plan.struts[3], 30.0);
        assert_eq!(plan.used, 2);
    }

    #[test]
    fn forced_breaks_start_a_new_column() {
        let mut evs = vec![leaf(0.0, 10.0), leaf(10.0, 20.0)];
        if let Ev::Leaf { forced, .. } = &mut evs[1] {
            *forced = true;
        }
        let plan = paginate(&evs, &Geometry { y0: 0.0, colh: 100.0 });
        assert_eq!(plan.struts[1], 90.0);
        assert_eq!(plan.used, 2);
    }
}
