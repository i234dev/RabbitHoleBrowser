//! CSS Grid layout per CSS Grid Layout Module Level 2
//! (<https://www.w3.org/TR/css-grid-2/>): §7 explicit/implicit grid,
//! §8 placement (incl. §8.5 auto-placement), §11 track sizing (intrinsic
//! sizes, maximize, flexible tracks, stretch) and §10 alignment.

use crate::block::{Cb, Layouter, WidthMode};
use crate::flex::{distribute, resolve_size, Axis, ItemEdges};
use crate::float::Bfc;
use crate::tree::{BoxKind, Flow, LayoutBox};
use rabbithole_style::{BoxSizing, ComputedStyle, ContentDistribution, GridAutoFlow, GridLine, ItemAlign, Length, TrackListItem, TrackSize};
use std::collections::HashSet;
use rabbithole_gfx::Rect;

mod lanes;
#[cfg(test)]
mod tests;

/// Intrinsic constraints have different free space during track maximization
/// and flex expansion, even though neither provides a definite available size.
#[derive(Clone, Copy, PartialEq)]
enum SizingConstraint {
    MinContent,
    MaxContent,
}

/// A track sizing function (§11.1).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Sizing {
    Fixed(f32),
    MinContent,
    MaxContent,
    Auto,
    Flex(f32),
    FitContent(f32),
}

impl Sizing {
    fn is_intrinsic(&self) -> bool {
        matches!(self, Sizing::MinContent | Sizing::MaxContent | Sizing::Auto | Sizing::FitContent(_))
    }
}

#[derive(Debug, Clone)]
struct Track {
    min: Sizing,
    max: Sizing,
    base: f32,
    /// Growth limit; `None` = infinite.
    limit: Option<f32>,
    /// Marked by `repeat(auto-fit)`; collapsed when no item spans it.
    auto_fit: bool,
    collapsed: bool,
    infinitely_growable: bool,
}

impl Track {
    fn new(min: Sizing, max: Sizing) -> Track {
        Track {
            min,
            max,
            base: 0.0,
            limit: None,
            auto_fit: false,
            collapsed: false,
            infinitely_growable: false,
        }
    }
    fn is_flex(&self) -> bool {
        matches!(self.max, Sizing::Flex(_))
    }
    fn limit_or_base(&self) -> f32 {
        self.limit.unwrap_or(self.base)
    }
}

/// Resolved placement of an item: [start, end) track indices per axis.
#[derive(Debug, Clone, Copy)]
struct Area {
    row: (i32, i32),
    col: (i32, i32),
}

/// An item's contribution to the tracks it spans.
#[derive(Debug, Clone, Copy)]
struct Contribution {
    start: usize,
    end: usize,
    min: f32,
    max: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum LineSpec {
    Auto,
    Line(i32),
    Span(i32),
}

fn sizing_of(ts: &TrackSize, avail: Option<f32>) -> (Sizing, Sizing) {
    match ts {
        TrackSize::Length(l) => match l {
            Length::Px(v) => (Sizing::Fixed(v.max(0.0)), Sizing::Fixed(v.max(0.0))),
            Length::Percent(p) => match avail {
                Some(a) => (Sizing::Fixed((a * p / 100.0).max(0.0)), Sizing::Fixed((a * p / 100.0).max(0.0))),
                None => (Sizing::Auto, Sizing::Auto),
            },
            Length::Calc(..) | Length::Math(_) => match avail {
                Some(a) => (Sizing::Fixed(l.resolve_or_zero(a).max(0.0)), Sizing::Fixed(l.resolve_or_zero(a).max(0.0))),
                None => (Sizing::Auto, Sizing::Auto),
            },
            Length::Auto => (Sizing::Auto, Sizing::Auto),
            Length::MinContent => (Sizing::MinContent, Sizing::MinContent),
            Length::MaxContent => (Sizing::MaxContent, Sizing::MaxContent),
            Length::FitContent => (Sizing::Auto, Sizing::FitContent(f32::INFINITY)),
        },
        TrackSize::Fr(f) => (Sizing::Auto, Sizing::Flex(f.max(0.0))),
        TrackSize::MinMax(a, b) => {
            let (amin, _) = sizing_of(a, avail);
            let (_, bmax) = sizing_of(b, avail);
            let amin = if matches!(amin, Sizing::Flex(_)) { Sizing::Auto } else { amin };
            (amin, bmax)
        }
        TrackSize::Calc(op, terms) => match avail {
            Some(a) => {
                let v = op.eval(terms, a).max(0.0);
                (Sizing::Fixed(v), Sizing::Fixed(v))
            }
            // Percentages of an indefinite size behave as `auto`.
            None => (Sizing::Auto, Sizing::Auto),
        },
        TrackSize::FitContent(l) => {
            let lim = match l {
                Length::Px(v) => *v,
                Length::Percent(p) => avail.map(|a| a * p / 100.0).unwrap_or(f32::INFINITY),
                Length::Calc(..) | Length::Math(_) => avail.map(|a| l.resolve_or_zero(a)).unwrap_or(f32::INFINITY),
                _ => f32::INFINITY,
            };
            (Sizing::Auto, Sizing::FitContent(lim.max(0.0)))
        }
    }
}

/// Repeat the auto-track pattern after explicitly sized tracks, and backwards
/// before the explicit grid (§7.6). The track immediately before it has index -1.
fn auto_track(sizes: &[TrackSize], index: i32, avail: Option<f32>) -> Track {
    let Some(size) = sizes.get(index.rem_euclid(sizes.len().max(1) as i32) as usize) else {
        return Track::new(Sizing::Auto, Sizing::Auto);
    };
    let (min, max) = sizing_of(size, avail);
    Track::new(min, max)
}

/// Fixed size of a track for `repeat(auto-fill)` computations (§7.2.3.2).
fn repeat_fixed_size(ts: &TrackSize, avail: Option<f32>) -> f32 {
    let (min, max) = sizing_of(ts, avail);
    match (min, max) {
        // A minmax() maximum smaller than its minimum is floored by that
        // minimum when counting repetitions, just as when sizing the track.
        (Sizing::Fixed(min), Sizing::Fixed(max)) => max.max(min),
        (_, Sizing::Fixed(v)) | (Sizing::Fixed(v), _) => v,
        _ => 0.0,
    }
}

struct TrackList {
    tracks: Vec<Track>,
    /// Line names; `names[i]` are the names of line `i` (0-based), length = tracks + 1.
    names: Vec<Vec<String>>,
}

/// Expand a template track list into tracks (§7.2), resolving `repeat()`.
fn expand_tracks(list: &[TrackListItem], avail: Option<f32>, gap: f32) -> TrackList {
    let mut tracks: Vec<Track> = Vec::new();
    let mut names: Vec<Vec<String>> = vec![Vec::new()];
    let push_track = |tracks: &mut Vec<Track>, names: &mut Vec<Vec<String>>, pre: &[String], size: &TrackSize, auto_fit: bool| {
        names.last_mut().unwrap().extend(pre.iter().cloned());
        let (min, max) = sizing_of(size, avail);
        let mut t = Track::new(min, max);
        t.auto_fit = auto_fit;
        tracks.push(t);
        names.push(Vec::new());
    };
    // Size taken by non-repeat tracks (for auto-fill counts).
    let fixed_sum: f32 = list
        .iter()
        .map(|it| match it {
            TrackListItem::Track { size, .. } => repeat_fixed_size(size, avail),
            TrackListItem::Repeat { count: Some(n), tracks, .. } => {
                let inner: f32 = tracks
                    .iter()
                    .map(|t| match t {
                        TrackListItem::Track { size, .. } => repeat_fixed_size(size, avail),
                        _ => 0.0,
                    })
                    .sum();
                inner * *n as f32
            }
            _ => 0.0,
        })
        .sum();
    let fixed_count: usize = list
        .iter()
        .map(|it| match it {
            TrackListItem::Track { .. } => 1,
            TrackListItem::Repeat { count: Some(n), tracks, .. } => tracks.iter().filter(|t| matches!(t, TrackListItem::Track { .. })).count() * *n as usize,
            _ => 0,
        })
        .sum();
    for item in list {
        match item {
            TrackListItem::Track { names: pre, size } => push_track(&mut tracks, &mut names, pre, size, false),
            TrackListItem::Names(n) => names.last_mut().unwrap().extend(n.iter().cloned()),
            TrackListItem::Repeat { count, auto_fit, tracks: inner } => {
                let reps = match count {
                    Some(n) => (*n).max(1) as usize,
                    None => {
                        let inner_size: f32 = inner
                            .iter()
                            .map(|t| match t {
                                TrackListItem::Track { size, .. } => repeat_fixed_size(size, avail),
                                _ => 0.0,
                            })
                            .sum();
                        let inner_count = inner.iter().filter(|t| matches!(t, TrackListItem::Track { .. })).count().max(1);
                        match avail {
                            Some(a) if inner_size > 0.0 => {
                                let space = a - fixed_sum - gap * fixed_count as f32;
                                let per = inner_size + gap * inner_count as f32;
                                (((space + gap) / per).floor() as i64).max(1) as usize
                            }
                            _ => 1,
                        }
                    }
                };
                let auto = count.is_none();
                for _ in 0..reps {
                    for t in inner {
                        match t {
                            TrackListItem::Track { names: pre, size } => push_track(&mut tracks, &mut names, pre, size, auto && *auto_fit),
                            TrackListItem::Names(n) => names.last_mut().unwrap().extend(n.iter().cloned()),
                            TrackListItem::Repeat { .. } => {}
                        }
                    }
                }
            }
        }
    }
    TrackList { tracks, names }
}

/// Find a line by name (`name-start`/`name-end` first, then `name`).
fn find_line(names: &[Vec<String>], name: &str, is_end: bool) -> Option<i32> {
    let suffixed = if is_end { format!("{name}-end") } else { format!("{name}-start") };
    for (i, ns) in names.iter().enumerate() {
        if ns.iter().any(|n| *n == suffixed) {
            return Some(i as i32);
        }
    }
    for (i, ns) in names.iter().enumerate() {
        if ns.iter().any(|n| *n == name) {
            return Some(i as i32);
        }
    }
    None
}

fn line_spec(l: &GridLine, names: &[Vec<String>], n_lines: i32, is_end: bool) -> LineSpec {
    match l {
        GridLine::Auto => LineSpec::Auto,
        GridLine::Line(n) => {
            if *n > 0 {
                LineSpec::Line(n - 1)
            } else if *n < 0 {
                LineSpec::Line(n_lines + n)
            } else {
                LineSpec::Auto
            }
        }
        GridLine::Span(k) => LineSpec::Span((*k).max(1) as i32),
        GridLine::Named(name, span) => {
            if *span {
                LineSpec::Span(1)
            } else {
                match find_line(names, name, is_end) {
                    Some(i) => LineSpec::Line(i),
                    None => LineSpec::Auto,
                }
            }
        }
    }
}

/// §8.3.1: resolve a start/end pair to either a definite [start, end) or a
/// span (auto-placed).
fn resolve_pair(start: LineSpec, end: LineSpec) -> Result<(i32, i32), i32> {
    match (start, end) {
        (LineSpec::Line(s), LineSpec::Line(e)) => {
            if s == e {
                Ok((s, s + 1))
            } else if s < e {
                Ok((s, e))
            } else {
                Ok((e, s))
            }
        }
        (LineSpec::Line(s), LineSpec::Span(k)) => Ok((s, s + k)),
        (LineSpec::Line(s), LineSpec::Auto) => Ok((s, s + 1)),
        (LineSpec::Span(k), LineSpec::Line(e)) => Ok((e - k, e)),
        (LineSpec::Auto, LineSpec::Line(e)) => Ok((e - 1, e)),
        (LineSpec::Span(k), _) => Err(k),
        (LineSpec::Auto, LineSpec::Span(k)) => Err(k),
        (LineSpec::Auto, LineSpec::Auto) => Err(1),
    }
}

/// Occupancy grid for auto-placement.
struct Occupancy {
    cells: HashSet<(i32, i32)>,
}

impl Occupancy {
    fn fits(&self, r0: i32, r1: i32, c0: i32, c1: i32) -> bool {
        for r in r0..r1 {
            for c in c0..c1 {
                if self.cells.contains(&(r, c)) {
                    return false;
                }
            }
        }
        true
    }
    fn mark(&mut self, r0: i32, r1: i32, c0: i32, c1: i32) {
        for r in r0..r1 {
            for c in c0..c1 {
                self.cells.insert((r, c));
            }
        }
    }
}

struct GridItem {
    idx: usize,
    area: Area,
    edges: ItemEdges,
    justify: ItemAlign,
    align: ItemAlign,
}

/// §11.5.1 distribute `extra` space equally among `candidates` (indices),
/// respecting growth limits unless `beyond_limits`. Returns leftover.
fn distribute_extra(tracks: &mut [Track], candidates: &[usize], extra: f32, beyond_limits: bool, grow_limits: bool) -> f32 {
    let mut remaining = extra;
    let mut active: Vec<usize> = candidates.to_vec();
    let mut guard = 0;
    while remaining > 0.001 && !active.is_empty() && guard < 64 {
        guard += 1;
        let share = remaining / active.len() as f32;
        let mut next_active = Vec::new();
        for &i in &active {
            let t = &mut tracks[i];
            if grow_limits {
                let cur = t.limit.unwrap_or(t.base);
                let newv = cur + share;
                t.limit = Some(newv);
                remaining -= share;
                next_active.push(i);
            } else if beyond_limits {
                t.base += share;
                remaining -= share;
                next_active.push(i);
            } else {
                let cap = t.limit.unwrap_or(f32::INFINITY);
                let room = (cap - t.base).max(0.0);
                let give = share.min(room);
                t.base += give;
                remaining -= give;
                if room - give > 0.001 {
                    next_active.push(i);
                }
            }
        }
        active = next_active;
        if grow_limits || beyond_limits {
            break;
        }
    }
    remaining.max(0.0)
}

/// §11 track sizing algorithm for one axis.
fn size_tracks(tracks: &mut [Track], contribs: &[Contribution], avail: Option<f32>, gap: f32, stretch: bool) {
    // An indefinite automatic size (such as an auto block size) uses the
    // max-content constraint. Min-content callers must preserve that constraint
    // explicitly instead of replacing the items' max-content contributions.
    size_tracks_with_constraint(tracks, contribs, avail, gap, stretch, SizingConstraint::MaxContent);
}

fn size_tracks_with_constraint(tracks: &mut [Track], contribs: &[Contribution], avail: Option<f32>, gap: f32, stretch: bool, constraint: SizingConstraint) {
    // §11.4 initialize.
    for t in tracks.iter_mut() {
        t.base = match t.min {
            Sizing::Fixed(v) => v,
            _ => 0.0,
        };
        t.limit = match t.max {
            Sizing::Fixed(v) => Some(v),
            _ => None,
        };
        t.infinitely_growable = false;
        if t.collapsed {
            t.base = 0.0;
            t.limit = Some(0.0);
        }
    }
    // §11.5 resolve intrinsic track sizes.
    let mut sorted: Vec<&Contribution> = contribs.iter().collect();
    sorted.sort_by_key(|c| c.end - c.start);
    // Items spanning a single track.
    for c in sorted.iter().filter(|c| c.end - c.start == 1) {
        let t = &mut tracks[c.start];
        if t.collapsed {
            continue;
        }
        match t.min {
            Sizing::MinContent | Sizing::Auto => t.base = t.base.max(c.min),
            Sizing::MaxContent => t.base = t.base.max(c.max),
            _ => {}
        }
        match t.max {
            Sizing::MinContent => t.limit = Some(t.limit.unwrap_or(0.0).max(c.min)),
            Sizing::MaxContent | Sizing::Auto => t.limit = Some(t.limit.unwrap_or(0.0).max(c.max)),
            Sizing::FitContent(l) => {
                let v = c.min.max(c.max.min(l));
                t.limit = Some(t.limit.unwrap_or(0.0).max(v));
            }
            _ => {}
        }
        if let Some(l) = t.limit {
            if l < t.base {
                t.limit = Some(t.base);
            }
        }
    }
    // Items spanning multiple tracks (no flex tracks), by increasing span.
    let mut span = 2;
    let max_span = sorted.iter().map(|c| c.end - c.start).max().unwrap_or(1);
    while span <= max_span {
        for c in sorted.iter().filter(|c| c.end - c.start == span) {
            let spanned: Vec<usize> = (c.start..c.end).collect();
            if spanned.iter().any(|&i| tracks[i].is_flex()) {
                continue;
            }
            let gaps = gap * ((c.end - c.start) as f32 - 1.0);
            // Minimums.
            let sum_base: f32 = spanned.iter().map(|&i| tracks[i].base).sum();
            let extra = c.min - sum_base - gaps;
            if extra > 0.0 {
                let cands: Vec<usize> = spanned.iter().copied().filter(|&i| tracks[i].min.is_intrinsic()).collect();
                if !cands.is_empty() {
                    let left = distribute_extra(tracks, &cands, extra, false, false);
                    if left > 0.001 {
                        let cands2: Vec<usize> = cands.iter().copied().filter(|&i| tracks[i].max.is_intrinsic()).collect();
                        let cands2 = if cands2.is_empty() { cands.clone() } else { cands2 };
                        distribute_extra(tracks, &cands2, left, true, false);
                    }
                }
            }
            // Max-content minimums (tracks with max-content min sizing).
            let sum_base: f32 = spanned.iter().map(|&i| tracks[i].base).sum();
            let extra = c.max - sum_base - gaps;
            if extra > 0.0 {
                let cands: Vec<usize> = spanned.iter().copied().filter(|&i| matches!(tracks[i].min, Sizing::MaxContent)).collect();
                if !cands.is_empty() {
                    let left = distribute_extra(tracks, &cands, extra, false, false);
                    if left > 0.001 {
                        distribute_extra(tracks, &cands, left, true, false);
                    }
                }
            }
            // Growth limits.
            let sum_limit: f32 = spanned.iter().map(|&i| tracks[i].limit_or_base()).sum();
            let extra = c.max - sum_limit - gaps;
            if extra > 0.0 {
                let cands: Vec<usize> = spanned.iter().copied().filter(|&i| tracks[i].max.is_intrinsic()).collect();
                if !cands.is_empty() {
                    distribute_extra(tracks, &cands, extra, false, true);
                }
            }
            for &i in &spanned {
                let t = &mut tracks[i];
                if let Some(l) = t.limit {
                    if l < t.base {
                        t.limit = Some(t.base);
                    }
                }
            }
        }
        span += 1;
    }
    for t in tracks.iter_mut() {
        if t.limit.is_none() {
            t.limit = Some(t.base);
            t.infinitely_growable = true;
        }
        if let Sizing::FitContent(l) = t.max {
            if let Some(lim) = t.limit {
                t.limit = Some(lim.min(l.max(t.base)));
            }
        }
    }
    let n_gaps = tracks.iter().filter(|t| !t.collapsed).count().saturating_sub(1) as f32;
    // §11.6 maximize tracks.
    if let Some(a) = avail {
        let used: f32 = tracks.iter().map(|t| t.base).sum::<f32>() + gap * n_gaps;
        let free = a - used;
        if free > 0.0 {
            let cands: Vec<usize> = (0..tracks.len()).filter(|&i| !tracks[i].collapsed && !tracks[i].is_flex()).collect();
            distribute_extra(tracks, &cands, free, false, false);
        }
    } else if constraint == SizingConstraint::MaxContent {
        // Under a max-content constraint the free space is infinite: every
        // non-flexible track can reach its growth limit. A min-content
        // constraint has zero free space and keeps the intrinsic base sizes.
        for t in tracks.iter_mut().filter(|t| !t.collapsed && !t.is_flex()) {
            t.base = t.base.max(t.limit_or_base());
        }
    }
    // §11.7 expand flexible tracks.
    let flex_idx: Vec<usize> = (0..tracks.len()).filter(|&i| tracks[i].is_flex() && !tracks[i].collapsed).collect();
    if !flex_idx.is_empty() {
        let factor = |t: &Track| match t.max {
            Sizing::Flex(f) => f,
            _ => 0.0,
        };
        let fr = match avail {
            _ if constraint == SizingConstraint::MinContent => 0.0,
            Some(a) => {
                let non_flex: f32 = tracks.iter().filter(|t| !t.is_flex() && !t.collapsed).map(|t| t.base).sum();
                let mut leftover = a - non_flex - gap * n_gaps;
                let mut active: Vec<usize> = flex_idx.clone();
                let mut fr;
                let mut guard = 0;
                loop {
                    guard += 1;
                    let sum_f: f32 = active.iter().map(|&i| factor(&tracks[i])).sum();
                    let sum_f = if sum_f < 1.0 { 1.0 } else { sum_f };
                    fr = (leftover / sum_f).max(0.0);
                    let mut changed = false;
                    let mut next = Vec::new();
                    for &i in &active {
                        let t = &tracks[i];
                        if t.base > fr * factor(t) + 0.001 {
                            leftover -= t.base;
                            changed = true;
                        } else {
                            next.push(i);
                        }
                    }
                    active = next;
                    if !changed || active.is_empty() || guard > 64 {
                        break;
                    }
                }
                fr
            }
            None => {
                let mut fr: f32 = 0.0;
                for &i in &flex_idx {
                    let f = factor(&tracks[i]);
                    if f > 0.0 {
                        fr = fr.max(tracks[i].base / f);
                    }
                }
                for c in contribs {
                    let spanned: Vec<usize> = (c.start..c.end).collect();
                    let sum_f: f32 = spanned.iter().filter(|&&i| tracks[i].is_flex()).map(|&i| factor(&tracks[i])).sum();
                    if sum_f <= 0.0 {
                        continue;
                    }
                    let non_flex: f32 = spanned.iter().filter(|&&i| !tracks[i].is_flex()).map(|&i| tracks[i].base).sum();
                    let gaps = gap * ((c.end - c.start) as f32 - 1.0);
                    let space = c.max - non_flex - gaps;
                    let sum_f = if sum_f < 1.0 { 1.0 } else { sum_f };
                    fr = fr.max((space / sum_f).max(0.0));
                }
                fr
            }
        };
        for &i in &flex_idx {
            let f = factor(&tracks[i]);
            let t = &mut tracks[i];
            t.base = t.base.max(fr * f);
            t.limit = Some(t.base);
        }
    }
    // §11.8 stretch auto tracks.
    if let Some(a) = avail {
        if stretch {
            let used: f32 = tracks.iter().map(|t| t.base).sum::<f32>() + gap * n_gaps;
            let free = a - used;
            if free > 0.0 {
                let cands: Vec<usize> = (0..tracks.len()).filter(|&i| matches!(tracks[i].max, Sizing::Auto) && !tracks[i].collapsed).collect();
                if !cands.is_empty() {
                    distribute_extra(tracks, &cands, free, true, false);
                }
            }
        }
    }
    for t in tracks.iter_mut() {
        if t.collapsed {
            t.base = 0.0;
        }
    }
}

/// Track offsets (start positions) and total extent, honouring gaps,
/// collapsed tracks and content distribution.
fn track_positions(tracks: &[Track], gap: f32, avail: Option<f32>, dist: ContentDistribution) -> (Vec<f32>, f32) {
    let n_vis = tracks.iter().filter(|t| !t.collapsed).count();
    let sum: f32 = tracks.iter().map(|t| t.base).sum::<f32>() + gap * n_vis.saturating_sub(1) as f32;
    let (lead, spacing) = match avail {
        Some(a) if !matches!(dist, ContentDistribution::FlexStart | ContentDistribution::Stretch) => distribute(dist, a - sum, n_vis),
        _ => (0.0, 0.0),
    };
    let mut pos = Vec::with_capacity(tracks.len());
    let mut x = lead;
    let mut first = true;
    for t in tracks {
        if t.collapsed {
            pos.push(x);
            continue;
        }
        if !first {
            x += gap + spacing;
        }
        first = false;
        pos.push(x);
        x += t.base;
    }
    (pos, x.max(sum + lead))
}

fn span_size(tracks: &[Track], pos: &[f32], start: usize, end: usize) -> (f32, f32) {
    if tracks.is_empty() || start >= end {
        return (0.0, 0.0);
    }
    let start = start.min(tracks.len() - 1);
    let end = end.min(tracks.len());
    let x = pos[start];
    let last = end - 1;
    let right = pos[last] + tracks[last].base;
    (x, (right - x).max(0.0))
}

impl<'a> Layouter<'a> {
    /// Lay out the grid items of `b`; returns the content height.
    pub(crate) fn layout_grid(&mut self, b: &mut LayoutBox, cb: &Cb) -> f32 {
        if b.style.grid_lanes {
            return self.layout_grid_lanes(b, cb);
        }
        let style = b.style.clone();
        let cbw = cb.width;
        let content_x = b.content_rect.x;
        let content_y = b.content_rect.y;
        let col_gap = style.column_gap.resolve_or_zero(cbw);
        let row_gap = style.row_gap.resolve_or_zero(cb.height.unwrap_or(0.0));

        let (mut items, mut cols, mut rows) = self.place_items(b, &style, Some(cbw), cb.height, col_gap, row_gap, content_x, content_y);

        // Column sizing.
        let col_contribs = self.column_contributions(b, &items, cbw);
        let col_stretch = matches!(style.justify_content, ContentDistribution::Stretch | ContentDistribution::FlexStart);
        size_tracks(&mut cols.tracks, &col_contribs, Some(cbw), col_gap, col_stretch);
        let (col_pos, _) = track_positions(&cols.tracks, col_gap, Some(cbw), style.justify_content);

        // Row sizing: contributions are item heights at their column width.
        let row_contribs = self.row_contributions(b, &items, &cols.tracks, &col_pos, col_gap, cbw, cb.height);
        let row_stretch = matches!(style.align_content, ContentDistribution::Stretch | ContentDistribution::FlexStart);
        size_tracks(&mut rows.tracks, &row_contribs, cb.height, row_gap, row_stretch);
        let (row_pos, rows_extent) = track_positions(&rows.tracks, row_gap, cb.height, style.align_content);

        // Final item layout and alignment (§10).
        for it in items.iter_mut() {
            let (ax, aw) = span_size(&cols.tracks, &col_pos, it.area.col.0 as usize, it.area.col.1 as usize);
            let (ay, ah) = span_size(&rows.tracks, &row_pos, it.area.row.0 as usize, it.area.row.1 as usize);
            self.layout_grid_item(b, it, ax + content_x, ay + content_y, aw, ah, cbw);
        }
        // Baseline self-alignment (§10.6): items aligned `baseline` that
        // start in the same row line up their first baselines.
        let mut rows_seen: Vec<i32> = items.iter().filter(|it| it.align == ItemAlign::Baseline).map(|it| it.area.row.0).collect();
        rows_seen.sort_unstable();
        rows_seen.dedup();
        for row in rows_seen {
            let group: Vec<usize> = items.iter().filter(|it| it.align == ItemAlign::Baseline && it.area.row.0 == row).map(|it| it.idx).collect();
            let bls: Vec<f32> = group.iter().map(|&i| Self::item_baseline(&b.children[i])).collect();
            let max = bls.iter().cloned().fold(f32::MIN, f32::max);
            for (k, &i) in group.iter().enumerate() {
                b.children[i].translate(0.0, max - bls[k]);
            }
        }

        // Absolutely positioned children of a positioned grid container use
        // the grid area of their line placement as containing block (§9.1);
        // an auto line is the padding edge.
        if style.position != rabbithole_style::Position::Static {
            let pad = crate::block::resolve_edges(&style.padding, cbw);
            let content_h = cb.height.unwrap_or(rows_extent);
            let (px0, px1) = (content_x - pad.left, content_x + cbw + pad.right);
            let (py0, py1) = (content_y - pad.top, content_y + content_h + pad.bottom);
            let line_x = |pos: &[f32], tracks: &[Track], i: i32| -> f32 {
                if tracks.is_empty() {
                    return 0.0;
                }
                let i = i.clamp(0, tracks.len() as i32) as usize;
                if i < tracks.len() {
                    pos[i]
                } else {
                    pos[tracks.len() - 1] + tracks[tracks.len() - 1].base
                }
            };
            let n_cols = cols.tracks.len() as i32 + 1;
            let n_rows = rows.tracks.len() as i32 + 1;
            for c in b.children.iter_mut() {
                if c.flow != Flow::Absolute {
                    continue;
                }
                let Some(node) = c.node else { continue };
                let s = c.style.clone();
                let axis = |start: &rabbithole_style::GridLine, end: &rabbithole_style::GridLine, names: &[Vec<String>], n: i32| -> (Option<i32>, Option<i32>) {
                    let a = match line_spec(start, names, n, false) {
                        LineSpec::Line(l) => Some(l),
                        _ => None,
                    };
                    let e = match line_spec(end, names, n, true) {
                        LineSpec::Line(l) => Some(l),
                        LineSpec::Span(k) => a.map(|a| a + k),
                        LineSpec::Auto => None,
                    };
                    let a = match (a, line_spec(start, names, n, false)) {
                        (None, LineSpec::Span(k)) => e.map(|e| e - k),
                        (a, _) => a,
                    };
                    (a, e)
                };
                let (c0, c1) = axis(&s.grid_column_start, &s.grid_column_end, &cols.names, n_cols);
                let (r0, r1) = axis(&s.grid_row_start, &s.grid_row_end, &rows.names, n_rows);
                if c0.is_none() && c1.is_none() && r0.is_none() && r1.is_none() {
                    // The static position: as the sole item of an area
                    // whose edges are the padding edges (§9.4).
                    c.static_pos = (px0, py0);
                    continue;
                }
                let x0 = c0.map(|l| content_x + line_x(&col_pos, &cols.tracks, l)).unwrap_or(px0);
                let x1 = c1.map(|l| content_x + line_x(&col_pos, &cols.tracks, l)).unwrap_or(px1);
                let y0 = r0.map(|l| content_y + line_x(&row_pos, &rows.tracks, l)).unwrap_or(py0);
                let y1 = r1.map(|l| content_y + line_x(&row_pos, &rows.tracks, l)).unwrap_or(py1);
                self.abs_cb.insert(node, Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)));
                // With auto offsets the item sits at the start of its area.
                c.static_pos = (x0, y0);
            }
        }

        rows_extent.max(0.0)
    }

    /// Build tracks, resolve placements (§8) and collapse auto-fit tracks.
    #[allow(clippy::too_many_arguments)]
    fn place_items(&mut self, b: &mut LayoutBox, style: &ComputedStyle, avail_w: Option<f32>, avail_h: Option<f32>, col_gap: f32, row_gap: f32, content_x: f32, content_y: f32) -> (Vec<GridItem>, TrackList, TrackList) {
        let mut cols = expand_tracks(&style.grid_template_columns, avail_w, col_gap);
        let mut rows = expand_tracks(&style.grid_template_rows, avail_h, row_gap);
        let sized_cols = cols.tracks.len();
        let sized_rows = rows.tracks.len();
        // Named areas → implicit line names (§7.3).
        let areas = &style.grid_template_areas;
        if !areas.is_empty() {
            let n_area_rows = areas.len();
            let n_area_cols = areas.iter().map(|r| r.len()).max().unwrap_or(0);
            while cols.tracks.len() < n_area_cols {
                cols.tracks.push(auto_track(&style.grid_auto_columns, (cols.tracks.len() - sized_cols) as i32, avail_w));
                cols.names.push(Vec::new());
            }
            while rows.tracks.len() < n_area_rows {
                rows.tracks.push(auto_track(&style.grid_auto_rows, (rows.tracks.len() - sized_rows) as i32, avail_h));
                rows.names.push(Vec::new());
            }
            let mut seen: HashSet<String> = HashSet::new();
            for (r, row) in areas.iter().enumerate() {
                for (c, name) in row.iter().enumerate() {
                    if name == "." || name.is_empty() || !seen.insert(name.clone()) {
                        continue;
                    }
                    // Extent of this area.
                    let mut r1 = r;
                    while r1 + 1 < areas.len() && areas[r1 + 1].get(c) == Some(name) {
                        r1 += 1;
                    }
                    let mut c1 = c;
                    while c1 + 1 < row.len() && row[c1 + 1] == *name {
                        c1 += 1;
                    }
                    cols.names[c].push(format!("{name}-start"));
                    cols.names[c1 + 1].push(format!("{name}-end"));
                    rows.names[r].push(format!("{name}-start"));
                    rows.names[r1 + 1].push(format!("{name}-end"));
                }
            }
        }
        let n_col_lines = cols.tracks.len() as i32 + 1;
        let n_row_lines = rows.tracks.len() as i32 + 1;

        // Collect items (order-modified document order).
        let mut order: Vec<(i32, usize)> = Vec::new();
        for (i, c) in b.children.iter_mut().enumerate() {
            match c.flow {
                Flow::Absolute => c.static_pos = (content_x, content_y),
                _ => order.push((c.style.order, i)),
            }
        }
        order.sort_by_key(|&(o, i)| (o, i));

        // Resolve definite placements.
        struct Pending {
            idx: usize,
            row: Result<(i32, i32), i32>,
            col: Result<(i32, i32), i32>,
        }
        let mut pending: Vec<Pending> = Vec::new();
        for &(_, idx) in &order {
            let s = &b.children[idx].style;
            let rs = line_spec(&s.grid_row_start, &rows.names, n_row_lines, false);
            let re = line_spec(&s.grid_row_end, &rows.names, n_row_lines, true);
            let cs = line_spec(&s.grid_column_start, &cols.names, n_col_lines, false);
            let ce = line_spec(&s.grid_column_end, &cols.names, n_col_lines, true);
            pending.push(Pending {
                idx,
                row: resolve_pair(rs, re),
                col: resolve_pair(cs, ce),
            });
        }
        // Leading implicit tracks for negative indices.
        let mut min_row = 0i32;
        let mut min_col = 0i32;
        for p in &pending {
            if let Ok((s, _)) = p.row {
                min_row = min_row.min(s);
            }
            if let Ok((s, _)) = p.col {
                min_col = min_col.min(s);
            }
        }
        let row_off = -min_row;
        let col_off = -min_col;
        for p in pending.iter_mut() {
            if let Ok((s, e)) = p.row {
                p.row = Ok((s + row_off, e + row_off));
            }
            if let Ok((s, e)) = p.col {
                p.col = Ok((s + col_off, e + col_off));
            }
        }
        for distance in 1..=row_off {
            rows.tracks.insert(0, auto_track(&style.grid_auto_rows, -distance, avail_h));
            rows.names.insert(0, Vec::new());
        }
        for distance in 1..=col_off {
            cols.tracks.insert(0, auto_track(&style.grid_auto_columns, -distance, avail_w));
            cols.names.insert(0, Vec::new());
        }

        // §8.5 auto-placement. Work in (major, minor) coordinates: for row
        // flow major = row, minor = column.
        let column_flow = matches!(style.grid_auto_flow, GridAutoFlow::Column | GridAutoFlow::ColumnDense);
        let dense = matches!(style.grid_auto_flow, GridAutoFlow::RowDense | GridAutoFlow::ColumnDense);
        let explicit_cols = cols.tracks.len() as i32;
        let explicit_rows = rows.tracks.len() as i32;
        let (mut n_minor, _n_major) = if column_flow { (explicit_rows, explicit_cols) } else { (explicit_cols, explicit_rows) };
        let mut occ = Occupancy { cells: HashSet::new() };
        let mut placed: Vec<Option<Area>> = vec![None; pending.len()];
        // Step 1: definite in both axes.
        for (k, p) in pending.iter().enumerate() {
            if let (Ok(r), Ok(c)) = (p.row, p.col) {
                occ.mark(r.0, r.1, c.0, c.1);
                placed[k] = Some(Area { row: r, col: c });
                if column_flow {
                    n_minor = n_minor.max(r.1);
                } else {
                    n_minor = n_minor.max(c.1);
                }
            }
        }
        // Items with a span larger than the minor axis grow the explicit grid.
        for p in &pending {
            let minor_span = if column_flow {
                match p.row {
                    Err(k) => k,
                    Ok((s, e)) => e - s,
                }
            } else {
                match p.col {
                    Err(k) => k,
                    Ok((s, e)) => e - s,
                }
            };
            n_minor = n_minor.max(minor_span);
        }
        let to_rc = |major: i32, minor: i32| if column_flow { (minor, major) } else { (major, minor) };
        // Step 2: items locked to a major line.
        let mut major_cursor: std::collections::HashMap<i32, i32> = std::collections::HashMap::new();
        for (k, p) in pending.iter().enumerate() {
            if placed[k].is_some() {
                continue;
            }
            let (major, minor_span) = if column_flow {
                match (p.col, p.row) {
                    (Ok(c), Err(k)) => (c, k),
                    _ => continue,
                }
            } else {
                match (p.row, p.col) {
                    (Ok(r), Err(k)) => (r, k),
                    _ => continue,
                }
            };
            let start_minor = if dense { 0 } else { *major_cursor.get(&major.0).unwrap_or(&0) };
            let mut m = start_minor;
            loop {
                if m + minor_span > n_minor {
                    n_minor = m + minor_span;
                }
                let (r0, c0) = to_rc(major.0, m);
                let (r1, c1) = to_rc(major.1, m + minor_span);
                if occ.fits(r0, r1, c0, c1) {
                    occ.mark(r0, r1, c0, c1);
                    placed[k] = Some(Area { row: (r0, r1), col: (c0, c1) });
                    major_cursor.insert(major.0, m + minor_span);
                    break;
                }
                m += 1;
            }
        }
        // Step 3: remaining items with the auto-placement cursor.
        let mut cur_major = 0i32;
        let mut cur_minor = 0i32;
        for (k, p) in pending.iter().enumerate() {
            if placed[k].is_some() {
                continue;
            }
            let (major_spec, minor_spec) = if column_flow { (p.col, p.row) } else { (p.row, p.col) };
            match minor_spec {
                Ok((ms, me)) => {
                    // Definite minor position, auto major.
                    let major_span = match major_spec {
                        Err(k) => k,
                        Ok((s, e)) => e - s,
                    };
                    if dense {
                        cur_major = 0;
                    } else if ms < cur_minor {
                        cur_major += 1;
                    }
                    cur_minor = ms;
                    let mut mj = cur_major;
                    loop {
                        let (r0, c0) = to_rc(mj, ms);
                        let (r1, c1) = to_rc(mj + major_span, me);
                        if occ.fits(r0, r1, c0, c1) {
                            occ.mark(r0, r1, c0, c1);
                            placed[k] = Some(Area { row: (r0, r1), col: (c0, c1) });
                            cur_major = mj;
                            break;
                        }
                        mj += 1;
                    }
                }
                Err(minor_span) => {
                    let major_span = match major_spec {
                        Err(k) => k,
                        Ok((s, e)) => e - s,
                    };
                    if dense {
                        cur_major = 0;
                        cur_minor = 0;
                    }
                    let mut mj = cur_major;
                    let mut mn = cur_minor;
                    loop {
                        if mn + minor_span > n_minor {
                            mj += 1;
                            mn = 0;
                            continue;
                        }
                        let (r0, c0) = to_rc(mj, mn);
                        let (r1, c1) = to_rc(mj + major_span, mn + minor_span);
                        if occ.fits(r0, r1, c0, c1) {
                            occ.mark(r0, r1, c0, c1);
                            placed[k] = Some(Area { row: (r0, r1), col: (c0, c1) });
                            cur_major = mj;
                            cur_minor = mn + minor_span;
                            break;
                        }
                        mn += 1;
                    }
                }
            }
        }

        // Implicit tracks.
        let mut max_row = rows.tracks.len() as i32;
        let mut max_col = cols.tracks.len() as i32;
        for a in placed.iter().flatten() {
            max_row = max_row.max(a.row.1);
            max_col = max_col.max(a.col.1);
        }
        // Tracks introduced by grid-template-areas already consumed entries
        // of the forward auto-size pattern; implicit tracks continue it.
        let mut ci = cols.tracks.len() - col_off as usize - sized_cols;
        while (cols.tracks.len() as i32) < max_col {
            cols.tracks.push(auto_track(&style.grid_auto_columns, ci as i32, avail_w));
            cols.names.push(Vec::new());
            ci += 1;
        }
        let mut ri = rows.tracks.len() - row_off as usize - sized_rows;
        while (rows.tracks.len() as i32) < max_row {
            rows.tracks.push(auto_track(&style.grid_auto_rows, ri as i32, avail_h));
            rows.names.push(Vec::new());
            ri += 1;
        }
        // Collapse empty auto-fit tracks.
        let mut used_cols = vec![false; cols.tracks.len()];
        let mut used_rows = vec![false; rows.tracks.len()];
        for a in placed.iter().flatten() {
            for c in a.col.0..a.col.1 {
                if (c as usize) < used_cols.len() {
                    used_cols[c as usize] = true;
                }
            }
            for r in a.row.0..a.row.1 {
                if (r as usize) < used_rows.len() {
                    used_rows[r as usize] = true;
                }
            }
        }
        for (i, t) in cols.tracks.iter_mut().enumerate() {
            if t.auto_fit && !used_cols[i] {
                t.collapsed = true;
            }
        }
        for (i, t) in rows.tracks.iter_mut().enumerate() {
            if t.auto_fit && !used_rows[i] {
                t.collapsed = true;
            }
        }

        let mut items = Vec::with_capacity(pending.len());
        for (k, p) in pending.iter().enumerate() {
            let area = placed[k].unwrap_or(Area { row: (0, 1), col: (0, 1) });
            let cs = b.children[p.idx].style.clone();
            let justify = match cs.justify_self {
                ItemAlign::Auto => style.justify_items,
                a => a,
            };
            let align = match cs.align_self {
                ItemAlign::Auto => style.align_items,
                a => a,
            };
            items.push(GridItem {
                idx: p.idx,
                area,
                edges: ItemEdges::new(&cs, avail_w.unwrap_or(0.0)),
                justify,
                align,
            });
        }
        (items, cols, rows)
    }

    /// Column contributions: min-/max-content widths incl. margins/padding/border.
    fn column_contributions(&mut self, b: &mut LayoutBox, items: &[GridItem], cbw: f32) -> Vec<Contribution> {
        let mut out = Vec::with_capacity(items.len());
        for it in items {
            let child = &mut b.children[it.idx];
            let bb = child.style.box_sizing == BoxSizing::BorderBox;
            let extras = it.edges.pb_h + it.edges.margin_sum(Axis::Row);
            let (min, max) = match resolve_size(child.style.width, Some(cbw), bb, it.edges.pb_h) {
                Some(w) if !matches!(child.style.width, Length::Percent(_) | Length::Calc(..) | Length::Math(_)) => (w, w),
                _ => {
                    if child.kind == BoxKind::Image {
                        let (w, _) = self.replaced_size(child, Some(cbw), None, it.edges.pb_h);
                        (w, w)
                    } else {
                        let (min, max) = self.intrinsic(child);
                        // The preferred intrinsic width also constrains the
                        // contribution to the opposite intrinsic track size.
                        // A max-content item cannot shrink to min-content,
                        // and a min-content item does not contribute its
                        // unconstrained max-content size (§6.2).
                        match child.style.width {
                            Length::MinContent => (min, min),
                            Length::MaxContent => (max, max),
                            _ => (min, max),
                        }
                    }
                }
            };
            // The minimum contribution uses the automatic minimum size,
            // which is zero for scroll containers, or an explicit
            // `min-width` (§6.6): `overflow: hidden` / `min-width: 0` items
            // do not blow `1fr` tracks up to their content (carousels).
            let min = match child.style.min_width {
                Length::Px(m) if child.kind != BoxKind::Image => {
                    let m = if bb { (m - it.edges.pb_h).max(0.0) } else { m };
                    m.min(min)
                }
                Length::Auto if child.kind != BoxKind::Image && crate::flex::is_scroll_container(&child.style) => 0.0,
                _ => min,
            };
            let (min, max) = (min + extras, max.max(min) + extras);
            out.push(Contribution {
                start: it.area.col.0.max(0) as usize,
                end: it.area.col.1.max(1) as usize,
                min,
                max,
            });
        }
        out
    }

    /// Row contributions: the item's height when laid out at its column width.
    #[allow(clippy::too_many_arguments)]
    fn row_contributions(&mut self, b: &mut LayoutBox, items: &[GridItem], cols: &[Track], col_pos: &[f32], _col_gap: f32, cbw: f32, cbh: Option<f32>) -> Vec<Contribution> {
        let mut out = Vec::with_capacity(items.len());
        // (contribution, row, baseline offset) of baseline-aligned items.
        let mut baselines: Vec<(usize, i32, f32)> = Vec::new();
        for it in items {
            let (_, aw) = span_size(cols, col_pos, it.area.col.0 as usize, it.area.col.1 as usize);
            let child = &mut b.children[it.idx];
            let bb = child.style.box_sizing == BoxSizing::BorderBox;
            let extras_v = it.edges.pb_v + it.edges.margin_sum(Axis::Column);
            let baseline = it.align == ItemAlign::Baseline;
            let h = match resolve_size(child.style.height, cbh, bb, it.edges.pb_v) {
                Some(h) if !baseline => h + extras_v,
                _ => {
                    let inner_w = self.grid_item_width(child, it, aw, cbw);
                    let cb = Cb {
                        x: 0.0,
                        width: aw,
                        height: None,
                    };
                    let mut bfc = Bfc::new();
                    self.forced_content_height = None;
                    self.layout_block(child, &cb, 0.0, &mut bfc, WidthMode::Fixed(inner_w));
                    child.rect.h + it.edges.margin_sum(Axis::Column)
                }
            };
            if baseline {
                baselines.push((out.len(), it.area.row.0, Self::item_baseline(child)));
            }
            out.push(Contribution {
                start: it.area.row.0.max(0) as usize,
                end: it.area.row.1.max(1) as usize,
                min: h,
                max: h,
            });
        }
        // Baseline-aligned items of a row are shifted to share a baseline:
        // their contributions include the shift (§11.5 step 2).
        for &(k, row, bl) in &baselines {
            let max = baselines.iter().filter(|b| b.1 == row).map(|b| b.2).fold(f32::MIN, f32::max);
            out[k].min += max - bl;
            out[k].max += max - bl;
        }
        out
    }

    /// Offset of an item's first baseline from its margin-box top (its
    /// bottom margin edge when it has no line).
    fn item_baseline(child: &LayoutBox) -> f32 {
        child.first_baseline().map(|y| y - child.rect.y).unwrap_or(child.rect.h) + child.margin.top
    }

    /// Inner width of a grid item inside an area of width `aw` (§10: stretch
    /// or fit-content).
    fn grid_item_width(&mut self, child: &mut LayoutBox, it: &GridItem, aw: f32, cbw: f32) -> f32 {
        let bb = child.style.box_sizing == BoxSizing::BorderBox;
        let avail_inner = (aw - it.edges.pb_h - it.edges.margin_sum(Axis::Row)).max(0.0);
        let _ = cbw;
        match resolve_size(child.style.width, Some(aw), bb, it.edges.pb_h) {
            Some(w) => self.clamp_width(&child.style.clone(), w, aw, it.edges.pb_h),
            None if matches!(child.style.width, Length::FitContent | Length::MinContent | Length::MaxContent) && child.kind != BoxKind::Image => {
                let (min, max) = self.intrinsic(child);
                let w = match child.style.width {
                    Length::MinContent => min,
                    Length::MaxContent => max,
                    _ => crate::block::shrink_to_fit(min, max, avail_inner),
                };
                self.clamp_width(&child.style.clone(), w, aw, it.edges.pb_h)
            }
            None => {
                let stretch = matches!(it.justify, ItemAlign::Stretch | ItemAlign::Normal | ItemAlign::Auto) && !it.edges.auto.left && !it.edges.auto.right;
                if stretch && child.kind != BoxKind::Image {
                    self.clamp_width(&child.style.clone(), avail_inner, aw, it.edges.pb_h)
                } else if child.kind == BoxKind::Image {
                    let (w, _) = self.replaced_size(child, Some(aw), None, it.edges.pb_h);
                    w.min(avail_inner.max(0.0)).max(0.0)
                } else {
                    let (min, max) = self.intrinsic(child);
                    self.clamp_width(&child.style.clone(), crate::block::shrink_to_fit(min, max, avail_inner), aw, it.edges.pb_h)
                }
            }
        }
    }

    /// Final layout of one item inside its grid area (§10.3 / §10.5).
    #[allow(clippy::too_many_arguments)]
    fn layout_grid_item(&mut self, b: &mut LayoutBox, it: &mut GridItem, ax: f32, ay: f32, aw: f32, ah: f32, cbw: f32) {
        let child = &mut b.children[it.idx];
        let inner_w = self.grid_item_width(child, it, aw, cbw);
        // Intrinsic block sizes also lack a numeric resolution, but only an
        // automatic preferred size is eligible for stretch (§6.2).
        let stretch_h = matches!(it.align, ItemAlign::Stretch | ItemAlign::Normal | ItemAlign::Auto)
            && !it.edges.auto.top
            && !it.edges.auto.bottom
            && child.style.height.is_auto()
            && child.kind != BoxKind::Image;
        let forced = if stretch_h { Some((ah - it.edges.pb_v - it.edges.margin_sum(Axis::Column)).max(0.0)) } else { None };
        let cb = Cb {
            x: 0.0,
            width: aw,
            height: Some(ah),
        };
        let mut bfc = Bfc::new();
        self.forced_content_height = forced;
        self.layout_block(child, &cb, 0.0, &mut bfc, WidthMode::Fixed(inner_w));
        self.forced_content_height = None;

        // Horizontal alignment.
        let mut margin = it.edges.margin;
        let outer_w = child.rect.w + margin.left + margin.right;
        let free_w = aw - outer_w;
        let dx = if (it.edges.auto.left || it.edges.auto.right) && free_w > 0.0 {
            if it.edges.auto.left && it.edges.auto.right {
                margin.left += free_w / 2.0;
                margin.right += free_w / 2.0;
                free_w / 2.0
            } else if it.edges.auto.left {
                margin.left += free_w;
                free_w
            } else {
                margin.right += free_w;
                0.0
            }
        } else {
            match it.justify {
                ItemAlign::FlexEnd => free_w,
                ItemAlign::Center => free_w / 2.0,
                _ => 0.0,
            }
        };
        // Vertical alignment.
        let outer_h = child.rect.h + margin.top + margin.bottom;
        let free_h = ah - outer_h;
        let dy = if (it.edges.auto.top || it.edges.auto.bottom) && free_h > 0.0 {
            if it.edges.auto.top && it.edges.auto.bottom {
                margin.top += free_h / 2.0;
                margin.bottom += free_h / 2.0;
                free_h / 2.0
            } else if it.edges.auto.top {
                margin.top += free_h;
                free_h
            } else {
                margin.bottom += free_h;
                0.0
            }
        } else {
            match it.align {
                ItemAlign::FlexEnd => free_h,
                ItemAlign::Center => free_h / 2.0,
                _ => 0.0,
            }
        };
        let x = ax + dx + it.edges.margin.left;
        let y = ay + dy + it.edges.margin.top;
        child.translate(x - child.rect.x, y - child.rect.y);
        child.margin = margin;
    }

    /// Intrinsic (min-content, max-content) widths of a grid container.
    pub(crate) fn grid_intrinsic(&mut self, b: &mut LayoutBox) -> (f32, f32) {
        if b.style.grid_lanes {
            return self.grid_lanes_intrinsic(b);
        }
        let style = b.style.clone();
        let col_gap = crate::flex::px_or_zero(style.column_gap);
        let (items, mut cols, _rows) = self.place_items(b, &style, None, None, col_gap, 0.0, 0.0, 0.0);
        let contribs = self.column_contributions(b, &items, 0.0);
        // Max-content minimum track sizing functions still need each item's
        // max-content contribution under a min-content constraint (§11.5).
        let mut min_tracks = cols.tracks.clone();
        size_tracks_with_constraint(&mut min_tracks, &contribs, None, col_gap, false, SizingConstraint::MinContent);
        let n_vis = min_tracks.iter().filter(|t| !t.collapsed).count();
        let gaps = col_gap * n_vis.saturating_sub(1) as f32;
        let min: f32 = min_tracks.iter().map(|t| t.base).sum::<f32>() + gaps;
        size_tracks(&mut cols.tracks, &contribs, None, col_gap, false);
        let max: f32 = cols.tracks.iter().map(|t| t.base).sum::<f32>() + gaps;
        (min, max.max(min))
    }
}
