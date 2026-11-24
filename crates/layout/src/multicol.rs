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
use rabbithole_style::{ComputedStyle, Overflow};

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
    c.flow == Flow::InFlow && fragmentable_box(c)
}

/// A block container whose contents can break across columns.
fn fragmentable_box(c: &LayoutBox) -> bool {
    matches!(c.kind, BoxKind::Block | BoxKind::AnonymousBlock)
        && matches!(c.formatting, Formatting::Block | Formatting::Inline)
        // (Scroll containers are monolithic; `overflow: clip` boxes are
        // not, CSS Overflow 3 §3.1.)
        && !(c.clips && [c.style.overflow_x, c.style.overflow_y].iter().any(|o| matches!(o, Overflow::Hidden | Overflow::Auto | Overflow::Scroll)))
        && !c.wm_wrapper
        && !c.style.multicol
        && !c.style.establishes_fixed_containing_block()
        // (Size containment makes a box monolithic, CSS Contain 2 §3.)
        && !c.style.contain_size
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
    Start { top: f32, h: f32, edge: bool, forced: bool, avoid_before: bool, avoid_inside: bool, prev: u32, mt: f32, solid: bool },
    /// A container ends: its border edge, whether it has a block-end
    /// border or padding, and its last child's event.
    End { bottom: f32, edge: bool, last: u32 },
    /// An unbreakable piece (line box, monolithic box, float).
    Leaf { top: f32, bottom: f32, forced: bool, avoid_before: bool, prev: u32, mt: f32, line: Option<LineInfo> },
    /// A spanner (`column-span: all`): ends a column set. `h` is its height
    /// laid out across the container, `delta` how much taller that is than
    /// in the flow.
    Span { top: f32, h: f32, mt: f32, mb: f32, delta: f32 },
}

impl Ev {
    fn top(&self) -> Option<f32> {
        match *self {
            Ev::Start { top, .. } | Ev::Leaf { top, .. } | Ev::Span { top, .. } => Some(top),
            Ev::End { .. } => None,
        }
    }
}

/// Is `c` a spanner of the multicol flow it is in (Multicol 1 §6.1)?
fn is_spanner(c: &LayoutBox) -> bool {
    c.style.column_span_all && c.flow == Flow::InFlow && !c.is_inline_level() && c.kind != BoxKind::Text
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
    /// Height changes of the spanners, in tree order.
    deltas: std::collections::VecDeque<f32>,
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
                self.float(&b.children[i]);
            }
        }
    }

    /// A fragmentable container: its start, contents and end. Breaks
    /// fall between its children only (not inside its borders and
    /// padding, CSS Fragmentation 3 §4.4).
    fn container(&mut self, c: &LayoutBox, forced: bool, avoid_before: bool, prev: u32) {
        let s = &c.style;
        let edge = s.border_width.top > 0.0 || c.content_rect.y > c.rect.y + 0.01;
        let at = self.evs.len();
        self.evs.push(Ev::Start {
            top: c.rect.y,
            h: c.rect.h,
            edge,
            forced,
            avoid_before,
            avoid_inside: mc::avoids_column_break(s.break_inside),
            prev,
            mt: c.margin.top,
            solid: false,
        });
        self.contents(c);
        let empty = self.evs.len() == at + 1;
        if empty && c.content_rect.h <= 0.01 {
            // Only borders and padding: unbreakable.
            if let Ev::Start { solid, .. } = &mut self.evs[at] {
                *solid = true;
            }
        }
        // The last child: the last event at this depth.
        let mut last = NONE;
        let mut depth = 0;
        for j in (at + 1..self.evs.len()).rev() {
            match self.evs[j] {
                Ev::End { .. } => depth += 1,
                Ev::Start { .. } if depth > 0 => {
                    depth -= 1;
                    if depth == 0 {
                        last = j as u32;
                        break;
                    }
                }
                Ev::Start { .. } | Ev::Leaf { .. } if depth == 0 => {
                    last = j as u32;
                    break;
                }
                _ => {}
            }
        }
        let bottom_edge = s.border_width.bottom > 0.0 || c.rect.bottom() > c.content_rect.bottom() + 0.01;
        self.evs.push(Ev::End { bottom: c.rect.bottom(), edge: bottom_edge, last });
    }

    /// A float: fragmented like a block when it is a block container.
    fn float(&mut self, c: &LayoutBox) {
        if fragmentable_box(c) {
            self.container(c, false, false, NONE);
        } else {
            let r = c.margin_rect();
            self.evs.push(Ev::Leaf { top: r.y, bottom: r.bottom(), forced: false, avoid_before: false, prev: NONE, mt: 0.0, line: None });
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
                Flow::FloatLeft | Flow::FloatRight => self.float(c),
                Flow::InFlow if is_spanner(c) => {
                    let delta = self.deltas.pop_front().unwrap_or(0.0);
                    self.evs.push(Ev::Span { top: c.rect.y, h: c.rect.h, mt: c.margin.top, mb: c.margin.bottom, delta });
                    prev = NONE;
                    forced_after = false;
                    avoid_after = false;
                }
                Flow::InFlow => {
                    let s = &c.style;
                    let forced = forced_after || mc::is_forced_break(s.break_before);
                    let avoid_before = avoid_after || mc::avoids_column_break(s.break_before);
                    let here = self.evs.len() as u32;
                    if fragmentable(c) {
                        self.container(c, forced, avoid_before, prev);
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

/// Where the pieces of a column set go for one column height.
struct Plan {
    /// Strut before each event (moves it and what follows down the flow).
    struts: Vec<f32>,
    /// Columns holding content.
    used: usize,
    /// Smallest column height increase that would avoid a break that was
    /// taken (balancing), `INFINITY` when none would.
    shortfall: f32,
    /// The flow offset after the set.
    off: f32,
    /// A break was taken where `break-*: avoid` asked for none.
    violated: bool,
}

/// The columns of a column set: flow position `y0` (strip coordinates
/// plus struts) starts the first column; a flow position `t` in column `k`
/// is at `t - lift - k * colh` in the document.
#[derive(Debug, Clone, Copy)]
struct Geometry {
    y0: f32,
    colh: f32,
    lift: f32,
    /// Horizontal distance between adjacent columns.
    step: f32,
    /// Columns per row (`column-wrap`; `usize::MAX` without wrapping) and
    /// the gap between rows.
    per_row: usize,
    row_gap: f32,
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
    /// Vertical translation from the flow into column `k`.
    fn dy(&self, k: usize) -> f32 {
        let row = (k / self.per_row) as f32;
        -self.lift - k as f32 * self.colh + row * (self.colh + self.row_gap)
    }
    /// Horizontal translation from the flow into column `k`.
    fn dx(&self, k: usize) -> f32 {
        (k % self.per_row) as f32 * self.step
    }
}

const EPS: f32 = 0.01;

/// Paginate events `a..b` (one column set) starting at flow offset `off0`.
fn paginate(evs: &[Ev], a: usize, b: usize, g: &Geometry, off0: f32) -> Plan {
    let n = evs.len();
    let mut struts = vec![0.0f32; n + 1];
    // (origin event, strut index, amount)
    let mut pushes: Vec<(usize, usize, f32)> = Vec::new();
    let mut offsets = vec![0.0f32; n + 1];
    let mut placed_at = vec![false; n + 1];
    let mut pos = vec![f32::NAN; n];
    let mut off = off0;
    let mut placed = false;
    let mut force_at = usize::MAX;
    let mut shortfall = f32::INFINITY;
    let mut budget = 4 * (b - a) + 64;
    let mut used = 0usize;
    let mut violated = false;
    let mut i = a;
    // Push event `i` (at flow position `t`) so that it starts at `target`;
    // the strut goes before the outermost container starting right there
    // without a border or padding of its own.
    let push = |i: usize, t: f32, target: f32, off: &mut f32, struts: &mut Vec<f32>, pushes: &mut Vec<(usize, usize, f32)>, pos: &mut Vec<f32>| {
        // (The first child of a container goes with the container's start
        // unless that starts a column.)
        let top = evs[i].top().unwrap_or(t);
        let mut at = i;
        while at > a {
            match evs[at - 1] {
                Ev::Start { top: st, .. } if !at_column_top(g, st + (t - top)) => at -= 1,
                _ => break,
            }
        }
        let start = evs[at].top().map_or(t, |st| st + (t - top));
        let amount = target - start;
        if amount <= 0.0 {
            return;
        }
        struts[at] += amount;
        pushes.push((i, at, amount));
        *off += amount;
        for p in &mut pos[at..i] {
            *p += amount;
        }
    };
    while i < b {
        if budget == 0 {
            break;
        }
        budget -= 1;
        offsets[i] = off;
        placed_at[i] = placed;
        match evs[i] {
            Ev::Span { .. } => {}
            Ev::End { bottom, edge: true, last } if last != NONE && force_at == usize::MAX => {
                // The block-end border and padding go with the last child.
                let bt = bottom + off;
                let j = last as usize;
                let tj = pos[j];
                if tj.is_finite() {
                    let k = g.col(tj);
                    if bt > g.end(k) + EPS && tj <= g.start(k) + EPS {
                        // (It cannot move: the break falls in the padding.)
                        violated = true;
                        shortfall = shortfall.min(bt - g.end(k));
                    } else if bt > g.end(k) + EPS {
                        shortfall = shortfall.min(bt - g.end(k));
                        backtrack(j, &mut i, &mut off, &offsets, &mut placed, &placed_at, &mut struts, &mut pushes, &mut force_at, &mut pos);
                        continue;
                    }
                }
            }
            Ev::End { .. } => {}
            Ev::Start { top, h, edge, forced, avoid_before, avoid_inside, prev, mt, solid } => {
                let t = top + off;
                let k = g.col(t);
                let avoid_inside = avoid_inside || solid;
                if force_at == i || (forced && placed) {
                    if t > g.y0 + EPS {
                        let k = g.col(t - EPS);
                        let target = g.start(k + 1) + if force_at == i { 0.0 } else { mt.max(0.0) };
                        push(i, t, target, &mut off, &mut struts, &mut pushes, &mut pos);
                    }
                } else if avoid_inside && h <= g.colh + EPS && t + h > g.end(k) + EPS && t > g.start(k) + EPS {
                    shortfall = shortfall.min(t + h - g.end(k));
                    if let Some(j) = earlier_break(&pos, g, avoid_before, prev) {
                        backtrack(j, &mut i, &mut off, &offsets, &mut placed, &placed_at, &mut struts, &mut pushes, &mut force_at, &mut pos);
                        continue;
                    }
                    violated |= avoid_before;
                    push(i, t, g.end(k), &mut off, &mut struts, &mut pushes, &mut pos);
                }
                if force_at == i {
                    force_at = usize::MAX;
                }
                pos[i] = top + off;
                if avoid_before && !forced && at_column_start(g, pos[i]) {
                    // A break against `avoid` fell right before it.
                    violated = true;
                    shortfall = shortfall.min(h.max(EPS));
                }
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
                        push(i, t, target, &mut off, &mut struts, &mut pushes, &mut pos);
                    }
                } else if bt > g.end(k) + EPS && t > g.start(k) + EPS {
                    shortfall = shortfall.min(bt - g.end(k));
                    let earlier = earlier_break(&pos, g, avoid_before, prev).or_else(|| line.and_then(|l| line_break(evs, &pos, g, a, i, k, l)));
                    if let Some(j) = earlier.filter(|&j| j >= a) {
                        backtrack(j, &mut i, &mut off, &offsets, &mut placed, &placed_at, &mut struts, &mut pushes, &mut force_at, &mut pos);
                        continue;
                    }
                    // (A first child that cannot take its container's start
                    // along breaks inside the container's padding.)
                    let glue = i > a && matches!(evs[i - 1], Ev::Start { edge: true, .. }) && at_column_top(g, pos[i - 1]);
                    violated |= avoid_before || glue || inside_avoid(evs, a, i);
                    push(i, t, g.end(k), &mut off, &mut struts, &mut pushes, &mut pos);
                }
                if force_at == i {
                    force_at = usize::MAX;
                }
                let (t, bt) = (top + off, bottom + off);
                pos[i] = t;
                if avoid_before && !forced && at_column_start(g, t) {
                    violated = true;
                    shortfall = shortfall.min((bt - t).max(EPS));
                }
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
                    if bt > next && i + 1 < b {
                        let amount = next - bt;
                        struts[i + 1] += amount;
                        pushes.push((i, i + 1, amount));
                        off += amount;
                    }
                }
            }
        }
        if let Ev::End { bottom, .. } = evs[i] {
            // (A container's own extent uses columns too.)
            let bt = bottom + off;
            if bt > g.y0 + EPS {
                used = used.max(g.col(bt - EPS) + 1);
            }
        }
        i += 1;
    }
    struts.truncate(n);
    Plan { struts, used, shortfall, off, violated }
}

/// Does flow position `t` start a column (or the set)?
fn at_column_top(g: &Geometry, t: f32) -> bool {
    (t - g.start(g.col(t))).abs() < EPS
}

/// Does flow position `t` start a column other than the first?
fn at_column_start(g: &Geometry, t: f32) -> bool {
    let k = g.col(t);
    k > 0 && (t - g.start(k)).abs() < EPS
}

/// Is event `i` inside a container with `break-inside: avoid`?
fn inside_avoid(evs: &[Ev], a: usize, i: usize) -> bool {
    let mut depth = 0i32;
    for e in evs[a..i].iter().rev() {
        match e {
            Ev::End { .. } => depth += 1,
            Ev::Start { avoid_inside, .. } => {
                if depth == 0 && *avoid_inside {
                    return true;
                }
                depth = (depth - 1).max(0);
            }
            _ => {}
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn backtrack(j: usize, i: &mut usize, off: &mut f32, offsets: &[f32], placed: &mut bool, placed_at: &[bool], struts: &mut [f32], pushes: &mut Vec<(usize, usize, f32)>, force_at: &mut usize, pos: &mut [f32]) {
    pushes.retain(|&(origin, at, amount)| {
        if origin >= j {
            struts[at] -= amount;
            for p in pos.iter_mut().take(j).skip(at) {
                *p -= amount;
            }
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
fn earlier_break(pos: &[f32], g: &Geometry, avoid_before: bool, prev: u32) -> Option<usize> {
    if !avoid_before || prev == NONE {
        return None;
    }
    let j = prev as usize;
    let t = pos[j];
    (t.is_finite() && t > g.start(g.col(t)) + EPS).then_some(j)
}

/// `orphans` / `widows` (CSS Fragmentation 3 §4.4): a break before line
/// `l.idx` of a block that would leave too few lines on either side moves
/// to an earlier line when that satisfies both.
fn line_break(evs: &[Ev], pos: &[f32], g: &Geometry, a: usize, i: usize, k: usize, l: LineInfo) -> Option<usize> {
    if l.idx == 0 {
        return None;
    }
    // The block's earlier lines in this column, newest first.
    let mut here: Vec<(usize, u32)> = Vec::new();
    let mut j = i;
    while j > a {
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

/// A column set of the flow: its events, column geometry, and the flow
/// range it holds.
#[derive(Debug, Clone, Copy)]
struct Set {
    first: usize,
    end: usize,
    g: Geometry,
    /// Flow position where the set's content ends (a spanner's margin
    /// edge, or infinity for the last set).
    flow_end: f32,
}

/// Moves the flow's pieces into their columns.
struct Mover<'p> {
    struts: &'p [f32],
    sets: &'p [Set],
    /// Vertical translation of each spanner (by event index).
    spans: &'p [(usize, f32)],
    i: usize,
    off: f32,
    g: Geometry,
    /// Nesting depth of positioned containers inside the flow (their
    /// absolutely positioned descendants move with them).
    positioned: u32,
    /// The translation of the first piece placed in the current container
    /// (for its list marker).
    first: Option<(f32, f32)>,
}

impl Mover<'_> {
    fn next(&mut self) {
        self.off += self.struts.get(self.i).copied().unwrap_or(0.0);
        if let Some(s) = self.sets.iter().find(|s| s.first <= self.i && self.i < s.end.max(s.first + 1)) {
            self.g = s.g;
        }
        self.i += 1;
    }

    /// Translation of a piece at flow position `t` (strip coordinates)
    /// into its column.
    fn shift(&mut self, top: f32) -> (f32, f32) {
        let t = top + self.off;
        let k = self.g.col(t);
        let d = (self.g.dx(k), self.off + self.g.dy(k));
        if self.first.is_none() {
            self.first = Some(d);
        }
        d
    }

    /// An absolutely positioned box whose containing block is inside the
    /// flow is fragmented by the columns where it lies (CSS Fragmentation
    /// 3 §4.1: no content moves past breaks inside it).
    fn move_abs(&mut self, c: &mut LayoutBox) {
        if self.positioned == 0 || c.fixed || c.style.position == rabbithole_style::Position::Fixed {
            return;
        }
        let start = c.rect.y + self.off;
        if fragmentable_box(c) && self.g.col(start) != self.g.col((c.rect.bottom() + self.off - EPS).max(start)) {
            let sets = [Set { first: 0, end: 0, g: self.g, flow_end: f32::INFINITY }];
            let mut sub = Mover { struts: &[], sets: &[], spans: &[], i: 0, off: self.off, g: self.g, positioned: 1, first: None };
            sub.contents(c);
            let end = (c.rect.bottom() + self.off).max(start);
            sub.sets = &sets;
            sub.fragment(c, start, end);
        } else {
            let k = self.g.col(start);
            c.translate(self.g.dx(k), self.off + self.g.dy(k));
        }
    }

    /// Move a monolithic box into its column; absolutely positioned
    /// descendants whose containing block is outside it follow their own
    /// containing block.
    fn move_leaf(&mut self, c: &mut LayoutBox, dx: f32, dy: f32) {
        c.translate(dx, dy);
        if self.positioned > 0 && !crate::block::contains_positioned(&c.style) {
            self.reposition_abs(c, dx, dy);
        }
    }

    fn reposition_abs(&mut self, b: &mut LayoutBox, dx: f32, dy: f32) {
        if b.no_abs_below && !b.children.iter().any(|c| c.flow == Flow::Absolute) {
            return;
        }
        for c in &mut b.children {
            if c.flow == Flow::Absolute {
                c.translate(-dx, -dy);
                self.move_abs(c);
            } else if !crate::block::contains_positioned(&c.style) {
                self.reposition_abs(c, dx, dy);
            }
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
                        self.move_leaf(c, dx, dy);
                    }
                }
            } else {
                self.float(&mut b.children[i]);
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
                Flow::FloatLeft | Flow::FloatRight => self.float(c),
                Flow::InFlow if is_spanner(c) => {
                    let at = self.i;
                    self.next();
                    if let Some(&(_, dy)) = self.spans.iter().find(|s| s.0 == at) {
                        c.translate(0.0, dy);
                    }
                }
                Flow::InFlow if fragmentable(c) => self.container(c),
                Flow::InFlow => {
                    self.next();
                    let (dx, dy) = self.shift(c.rect.y);
                    self.move_leaf(c, dx, dy);
                }
            }
        }
    }

    /// A fragmentable container: its contents, then its fragments.
    fn container(&mut self, c: &mut LayoutBox) {
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

    fn float(&mut self, c: &mut LayoutBox) {
        if fragmentable_box(c) {
            let first = self.first;
            self.container(c);
            // (A float's marker position is not the container's.)
            self.first = first.or(self.first);
        } else {
            self.next();
            let (dx, dy) = self.shift(c.margin_rect().y);
            self.move_leaf(c, dx, dy);
        }
    }

    /// Place a fragmentable container spanning `[start, end)` of the
    /// flow (strut-adjusted strip coordinates); its contents were moved.
    fn fragment(&mut self, c: &mut LayoutBox, start: f32, end: f32) {
        let growth = (end - start) - c.rect.h;
        let content = Rect::new(c.content_rect.x, c.content_rect.y + (start - c.rect.y), c.content_rect.w, (c.content_rect.h + growth).max(0.0));
        let border = Rect::new(c.rect.x, start, c.rect.w, end - start);
        // The columns (set, column) the box has a part in.
        let mut parts: Vec<(Geometry, usize, f32, f32)> = Vec::new();
        for s in self.sets {
            let lo = start.max(s.g.y0);
            let hi = end.min(s.flow_end);
            if hi < lo || (hi == lo && !(start == end && start >= s.g.y0 && start <= s.flow_end)) {
                continue;
            }
            let (k0, k1) = (s.g.col(lo), s.g.col((hi - EPS).max(lo)).max(s.g.col(lo)));
            for k in k0..=k1 {
                let plo = if k == k0 { lo } else { s.g.start(k) };
                let phi = if k == k1 { hi } else { s.g.end(k) };
                parts.push((s.g, k, plo, phi));
            }
        }
        if parts.is_empty() {
            parts.push((self.g, self.g.col(start), start, end));
        }
        if parts.len() == 1 {
            let (g, k, _, _) = parts[0];
            let (dx, dy) = (g.dx(k), g.dy(k));
            c.rect = border.translate(dx, dy);
            c.content_rect = content.translate(dx, dy);
            if growth.abs() > EPS {
                c.layout_cache = None;
            }
            c.col_pieces = None;
            return;
        }
        let bw = c.style.border_width;
        let padding = Rect::new(border.x + bw.left, border.y + bw.top, (border.w - bw.left - bw.right).max(0.0), (border.h - bw.top - bw.bottom).max(0.0));
        let mut pieces = Vec::new();
        let mut union: Option<Rect> = None;
        let mut content_union: Option<Rect> = None;
        for (g, k, lo, hi) in parts {
            let (dx, dy) = (g.dx(k), g.dy(k));
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

/// A laid out column set (a row of columns): its top, column height, and
/// how many columns hold content.
struct RowOut {
    top: f32,
    height: f32,
    used: usize,
}

/// Lay out spanners nested in the flow of `b` across the whole container
/// (`full`); returns how much taller each became, in tree order.
fn spanner_widths(lay: &mut Layouter, b: &mut LayoutBox, full: &Cb, out: &mut std::collections::VecDeque<f32>) {
    if b.formatting != Formatting::Block {
        return;
    }
    for c in &mut b.children {
        if c.clamped || c.flow != Flow::InFlow {
            continue;
        }
        if is_spanner(c) {
            let old = c.rect.h;
            let mut bfc = Bfc::new();
            let top = c.rect.y;
            lay.layout_block(c, full, top, &mut bfc, WidthMode::Fill);
            out.push_back(c.rect.h - old);
        } else if fragmentable(c) {
            spanner_widths(lay, c, full, out);
        }
    }
}

fn has_spanner(b: &LayoutBox) -> bool {
    b.formatting == Formatting::Block && b.children.iter().any(|c| c.flow == Flow::InFlow && !c.clamped && (is_spanner(c) || (fragmentable(c) && has_spanner(c))))
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
        let content_y = b.content_rect.y;
        // The flow: the contents laid out as one column.
        let saved = b.content_rect;
        b.content_rect.x = x0;
        b.content_rect.w = w;
        let mut local = Bfc::new();
        let flow_cb = Cb { x: x0, width: w, height: cb.height };
        let h = match b.formatting {
            Formatting::Inline => self.layout_inline(b, &flow_cb, &mut local),
            _ => self.layout_block_children_of(b, &flow_cb, &mut local),
        };
        let total = if local.is_empty() { h } else { h.max(local.max_bottom() - content_y) };
        b.content_rect.x = saved.x;
        b.content_rect.w = saved.w;
        let mut deltas = std::collections::VecDeque::new();
        if has_spanner(b) {
            spanner_widths(self, b, &Cb { x: cb.x, width: avail_w, height: cb.height }, &mut deltas);
        }
        // `column-wrap: auto` wraps when the columns have a height.
        let wrap = match style.column_wrap {
            1 => false,
            2 => true,
            _ => style.column_height.is_some(),
        };
        let row_gap = if style.row_gap_normal { 0.0 } else { style.row_gap.resolve_or_zero(cb.height.unwrap_or(0.0)).max(0.0) };
        let params = FlowParams { y0: content_y, total, n, fill_auto: style.column_fill_auto, avail: avail_h, step, trim: style.margin_trim, column_height: style.column_height, wrap, row_gap };
        let (rows, end) = fragment_flow(b, deltas, params);
        let mut rules = Vec::new();
        for r in &rows {
            // The rules of a last row (after the last spanner) reach the
            // container's content end (CSSWG issue 2309).
            let h = match cb.height {
                Some(ch) if (r.top + r.height - end).abs() < EPS => r.height.max(content_y + ch - r.top),
                _ => r.height,
            };
            push_rules(&mut rules, &style, x0, w, gap, step, r.top, h, r.used);
        }
        b.col_rules = (!rules.is_empty()).then(|| Box::new(rules));
        (end - content_y).max(0.0)
    }
}

struct FlowParams {
    y0: f32,
    total: f32,
    n: u32,
    fill_auto: bool,
    avail: Option<f32>,
    step: f32,
    /// The container's `margin-trim`.
    trim: u8,
    /// `column-height` (Multicol 2), whether columns wrap into rows, and
    /// the gap between rows.
    column_height: Option<f32>,
    wrap: bool,
    row_gap: f32,
}

/// Choose the column heights of a laid out flow (the contents of `root`)
/// and move its pieces into columns; returns the rows of columns.
fn fragment_flow(root: &mut LayoutBox, deltas: std::collections::VecDeque<f32>, p: FlowParams) -> (Vec<RowOut>, f32) {
    let mut collector = Collector { evs: Vec::new(), deltas };
    collector.contents(root);
    let evs = collector.evs;
    let n = p.n.max(1) as usize;
    let floor = |h: f32| h.max(1.0);
    let mut struts = vec![0.0f32; evs.len()];
    let mut sets: Vec<Set> = Vec::new();
    let mut spans: Vec<(usize, f32)> = Vec::new();
    let mut rows: Vec<RowOut> = Vec::new();
    let mut off = 0.0f32;
    // Document position where the next row (or spanner) goes, and the
    // strip position where the next set's content starts.
    let mut phys = p.y0;
    // The bottom margin of the last spanner, collapsing with the top
    // margin of a following spanner.
    let mut pending_mb = 0.0f32;
    let mut strip_start = p.y0;
    let mut a = 0;
    loop {
        let b = (a..evs.len()).find(|&i| matches!(evs[i], Ev::Span { .. })).unwrap_or(evs.len());
        let last = b == evs.len();
        // The set's extent in the strip: up to the spanner's margin edge.
        let strip_end = match evs.get(b) {
            Some(Ev::Span { top, mt, .. }) => top - mt.max(0.0),
            _ => p.y0 + p.total,
        };
        let y0 = strip_start + off;
        let total = (strip_end - strip_start).max(0.0);
        let has_content = evs[a..b].iter().any(|e| !matches!(e, Ev::End { .. }));
        if has_content {
            phys += std::mem::take(&mut pending_mb);
            // The height left for the last row of a container with a
            // height limit.
            let room = p.avail.map(|h| (p.y0 + h - phys).max(0.0));
            // (Wrapping columns are as tall as `column-height`, else as the
            // container.)
            let fixed = if p.column_height.is_some() { p.column_height } else if last && (p.fill_auto || p.wrap) { room } else { None };
            let cap = if last { room } else { None };
            let (colh, plan) = choose_height(&evs, a, b, y0, phys, total, n, fixed, cap, off);
            let colh = if fixed.is_some() { floor(colh) } else { colh };
            let per_row = if p.wrap { n } else { usize::MAX };
            let g = Geometry { y0, colh: floor(colh), lift: y0 - phys, step: p.step, per_row, row_gap: p.row_gap };
            let used = plan.used.max(1);
            let count = if p.wrap { used.div_ceil(n) } else { 1 };
            // With wrapping rows and `column-fill: balance`, the last row
            // is balanced (Multicol 2 §7).
            let split = (count > 1 && !p.fill_auto).then(|| {
                let k = (count - 1) * n;
                let mut o = off;
                let j = (a..b).find(|&i| {
                    o += plan.struts[i];
                    evs[i].top().is_some_and(|t| g.col(t + o) >= k)
                });
                (j, k)
            });
            match split {
                Some((Some(j), k)) if j > a => {
                    for i in a..j {
                        struts[i] += plan.struts[i];
                    }
                    let f = g.start(k);
                    sets.push(Set { first: a, end: j, g, flow_end: f });
                    for r in 0..count - 1 {
                        rows.push(RowOut { top: phys, height: colh, used: (used - r * n).min(n) });
                        phys += colh + p.row_gap;
                    }
                    let off_j = off + plan.struts[a..j].iter().sum::<f32>();
                    let rest = (strip_end + plan.off - f).max(0.0);
                    let (h2, plan2) = choose_height(&evs, j, b, f, phys, rest, n, None, Some(colh), off_j);
                    for i in j..b {
                        struts[i] += plan2.struts[i];
                    }
                    let g2 = Geometry { y0: f, colh: floor(h2), lift: f - phys, step: p.step, per_row, row_gap: p.row_gap };
                    off = plan2.off;
                    sets.push(Set { first: j, end: b, g: g2, flow_end: strip_end + off });
                    rows.push(RowOut { top: phys, height: h2, used: plan2.used.clamp(1, n) });
                    phys += h2;
                }
                _ => {
                    for i in a..b {
                        struts[i] += plan.struts[i];
                    }
                    off = plan.off;
                    sets.push(Set { first: a, end: b, g, flow_end: strip_end + off });
                    for r in 0..count {
                        rows.push(RowOut { top: phys, height: colh, used: if p.wrap { (used - r * n).min(n) } else { used } });
                        phys += colh + if r + 1 < count { p.row_gap } else { 0.0 };
                    }
                }
            }
        }
        let Some(Ev::Span { top, h, mut mt, mut mb, delta }) = evs.get(b).copied() else { break };
        // `margin-trim` on the container trims a spanner's margins at its
        // block start / end (CSS Box 4 §3.3).
        if p.trim & rabbithole_style::MARGIN_TRIM_BLOCK_START != 0 && spans.is_empty() && rows.iter().all(|r| r.height <= 0.0) {
            mt = 0.0;
        }
        if p.trim & rabbithole_style::MARGIN_TRIM_BLOCK_END != 0 && !evs[b + 1..].iter().any(|e| matches!(e, Ev::Leaf { .. } | Ev::Span { .. })) {
            mb = 0.0;
        }
        // The spanner, across the container below the row.
        let sp_top = phys + std::mem::take(&mut pending_mb).max(mt.max(0.0));
        spans.push((b, sp_top - top));
        phys = sp_top + h;
        pending_mb = mb.max(0.0);
        // The flow after it moves by its change of height.
        if b + 1 < evs.len() {
            struts[b + 1] += delta;
        }
        off += delta;
        strip_start = top - delta + h + mb.max(0.0);
        a = b + 1;
    }
    phys += pending_mb;
    if sets.is_empty() {
        // (Spanners only.)
        let g = Geometry { y0: p.y0, colh: 1.0, lift: 0.0, step: p.step, per_row: usize::MAX, row_gap: 0.0 };
        let mut mover = Mover { struts: &struts, sets: &[], spans: &spans, i: 0, off: 0.0, g, positioned: 0, first: None };
        mover.contents(root);
        return (rows, phys);
    }
    let g = sets[0].g;
    let mut mover = Mover { struts: &struts, sets: &sets, spans: &spans, i: 0, off: 0.0, g, positioned: 0, first: None };
    mover.contents(root);
    (rows, phys)
}

/// The column height of one column set (events `a..b`, flow from `y0`,
/// `total` tall, the row at document position `phys`), with its plan.
#[allow(clippy::too_many_arguments)]
fn choose_height(evs: &[Ev], a: usize, b: usize, y0: f32, phys: f32, total: f32, n: usize, fixed: Option<f32>, cap: Option<f32>, off: f32) -> (f32, Plan) {
    let geom = |h: f32| Geometry { y0, colh: h.max(1.0), lift: y0 - phys, step: 0.0, per_row: usize::MAX, row_gap: 0.0 };
    if let Some(h) = fixed {
        return (h.max(0.0), paginate(evs, a, b, &geom(h), off));
    }
    // Balance: the smallest height that fits the set in `n` columns,
    // starting from the average (never shorter than an unbreakable piece)
    // and growing by the least amount that avoids a break taken.
    let tallest = evs[a..b].iter().fold(0.0f32, |m, e| match *e {
        Ev::Leaf { top, bottom, .. } => m.max(bottom - top),
        Ev::Start { h, avoid_inside: true, .. } | Ev::Start { h, solid: true, .. } => m.max(h),
        _ => m,
    });
    let cap = cap.unwrap_or(f32::INFINITY);
    let mut h = (total / n as f32).max(tallest).max(0.0).min(cap);
    let mut plan = paginate(evs, a, b, &geom(h), off);
    let mut rounds = 0;
    // (A break against `break-*: avoid` is also avoided when a taller
    // column can.)
    while (plan.used > n || (plan.violated && h < total)) && plan.shortfall.is_finite() && h < cap && rounds < 64 {
        rounds += 1;
        h = (h + plan.shortfall.max(0.05)).min(cap);
        plan = paginate(evs, a, b, &geom(h), off);
    }
    if plan.used > n && h < total && h < cap && rounds >= 64 {
        h = total.min(cap);
        plan = paginate(evs, a, b, &geom(h), off);
    }
    (h, plan)
}

/// Column rules between adjacent used columns of a row (Multicol 1 §4):
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
        let plan = paginate(&evs, 0, evs.len(), &Geometry { y0: 0.0, colh: 100.0, lift: 0.0, step: 0.0, per_row: usize::MAX, row_gap: 0.0 }, 0.0);
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
        let plan = paginate(&evs, 0, evs.len(), &Geometry { y0: 0.0, colh: 90.0, lift: 0.0, step: 0.0, per_row: usize::MAX, row_gap: 0.0 }, 0.0);
        assert_eq!(plan.struts[3], 30.0);
        assert_eq!(plan.used, 2);
    }

    #[test]
    fn forced_breaks_start_a_new_column() {
        let mut evs = vec![leaf(0.0, 10.0), leaf(10.0, 20.0)];
        if let Ev::Leaf { forced, .. } = &mut evs[1] {
            *forced = true;
        }
        let plan = paginate(&evs, 0, evs.len(), &Geometry { y0: 0.0, colh: 100.0, lift: 0.0, step: 0.0, per_row: usize::MAX, row_gap: 0.0 }, 0.0);
        assert_eq!(plan.struts[1], 90.0);
        assert_eq!(plan.used, 2);
    }
}
