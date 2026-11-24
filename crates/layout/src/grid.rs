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
pub(crate) mod resolved;
// Interface checkpoint: the following private state is compiled and tested
// before the corresponding orthogonal traversal is enabled.
#[allow(dead_code)]
mod phases;
#[allow(dead_code)]
mod percentages;
#[allow(dead_code)]
mod estimate;
#[allow(dead_code)]
mod pass;
mod item_measure;
mod traversal;
pub(crate) mod subgrid;
#[cfg(test)]
mod tests;

/// Available grid space (Grid2 §12.2). Intrinsic constraints are distinct
/// states, not fabricated definite widths or modified item contributions.
#[derive(Clone, Copy, Debug, PartialEq)]
enum GridSpace {
    Definite(f32),
    MinContent,
    MaxContent,
    /// Compatibility completion for an indefinite standalone axis in the inherited/orthogonal
    /// route, whose parent-axis consumer is not implemented yet. This is
    /// the shipped indefinite algorithm, including flexible-track sizing;
    /// it is not an intrinsic constraint or a completed per-axis result.
    LegacyIndefinite,
}

/// Grid2 §12.1 permits one adjustment after the opposite axis is known.
/// This phase is carried through subgrid recursion rather than recursively
/// completing both axes before returning an initial contribution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MeasurePhase {
    Initial,
    Adjusted,
}

/// Definite container sizes used for distributed track geometry. Intrinsic
/// constraints are None, including when cbw's numeric measurement reference
/// is zero; a real definite zero remains Some(0).
#[derive(Clone, Copy)]
struct GridGeometry {
    inline:Option<f32>,
    block:Option<f32>,
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
    /// A sizing function with percentages of an indefinite size (sized as
    /// `auto` until the container's size is known).
    deferred: Option<Box<TrackSize>>,
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
            deferred: None,
        }
    }

    /// A track of `size`; percentages of an indefinite `avail` are kept to
    /// resolve once the container size is known (Grid2 §7.2.1).
    fn of(size: &TrackSize, avail: Option<f32>) -> Track {
        let (min, max) = sizing_of(size, avail);
        let mut t = Track::new(min, max);
        if avail.is_none() && track_size_has_percent(size) {
            t.deferred = Some(Box::new(size.clone()));
        }
        t
    }
    fn is_flex(&self) -> bool {
        matches!(self.max, Sizing::Flex(_))
    }
    fn limit_or_base(&self) -> f32 {
        self.limit.unwrap_or(self.base)
    }
}

/// Where the explicit grid sits among the tracks of the last placement:
/// the leading implicit tracks and the explicit line counts, which
/// absolutely positioned children resolve their lines against.
#[derive(Clone, Copy, Default)]
struct Placement {
    row_off: i32,
    col_off: i32,
    row_lines: i32,
    col_lines: i32,
}

thread_local! {
    static LAST_PLACEMENT: std::cell::Cell<Placement> = const { std::cell::Cell::new(Placement { row_off: 0, col_off: 0, row_lines: 1, col_lines: 1 }) };
}

/// Resolved placement of an item: [start, end) track indices per axis.
#[derive(Debug, Clone, Copy)]
struct Area {
    row: (i32, i32),
    col: (i32, i32),
}

/// An item's contribution to the tracks it spans.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Contribution {
    start: usize,
    end: usize,
    min: f32,
    max: f32,
    /// `min` is a content-based automatic minimum, which is zero for an
    /// item spanning flexible tracks among others (§6.6).
    auto_min: bool,
    /// The minimum contribution (§6.6), when it differs from `min`.
    minimum: f32,
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

fn track_size_has_percent(ts: &TrackSize) -> bool {
    match ts {
        TrackSize::Length(l) | TrackSize::FitContent(l) => l.has_percent(),
        TrackSize::Calc(..) => true,
        TrackSize::MinMax(a, b) => track_size_has_percent(a) || track_size_has_percent(b),
        TrackSize::Fr(_) => false,
    }
}

/// Repeat the auto-track pattern after explicitly sized tracks, and backwards
/// before the explicit grid (§7.6). The track immediately before it has index -1.
fn auto_track(sizes: &[TrackSize], index: i32, avail: Option<f32>) -> Track {
    let Some(size) = sizes.get(index.rem_euclid(sizes.len().max(1) as i32) as usize) else {
        return Track::new(Sizing::Auto, Sizing::Auto);
    };
    Track::of(size, avail)
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

#[derive(Clone)]
struct TrackList {
    tracks: Vec<Track>,
    /// Line names; `names[i]` are the names of line `i` (0-based), length = tracks + 1.
    names: Vec<Vec<String>>,
    /// Explicitly authored names, excluding names generated by named areas.
    css_names: std::rc::Rc<Vec<Vec<String>>>,
    inherited: Option<std::sync::Arc<subgrid::SharedTracks>>,
}

impl TrackList {
    /// Whether fixed track sizes are known: own tracks, or shared tracks
    /// whose geometry is definite (not a placeholder while being sized).
    fn sized(&self) -> bool {
        self.inherited.as_ref().is_none_or(|parent| parent.definite)
    }
}

/// Expand a template track list into tracks (§7.2), resolving `repeat()`.
fn expand_tracks(list: &[TrackListItem], avail: Option<f32>, gap: f32) -> TrackList {
    expand_tracks_sized(list, avail, gap, None, None)
}

/// [`expand_tracks`] for a container's own axis: with an indefinite size,
/// automatic repetitions fit a definite maximum size, else repeat as few
/// times as fill a definite minimum size (including one transferred by an
/// aspect ratio) (Grid2 §7.2.3.2).
fn expand_tracks_for(style: &ComputedStyle, rows: bool, list: &[TrackListItem], avail: Option<f32>, gap: f32) -> TrackList {
    if avail.is_some() || !list.iter().any(|it| matches!(it, TrackListItem::Repeat { count: None, .. })) {
        return expand_tracks(list, avail, gap);
    }
    let pb = |style: &ComputedStyle, rows: bool| {
        let pad = crate::block::resolve_edges(&style.padding, 0.0);
        let bw = style.border_width;
        if rows { pad.top + pad.bottom + bw.top + bw.bottom } else { pad.left + pad.right + bw.left + bw.right }
    };
    let content = |v: f32, rows: bool| if style.box_sizing == BoxSizing::BorderBox { (v - pb(style, rows)).max(0.0) } else { v };
    let (max, min, other_min) = if rows { (style.max_height, style.min_height, style.min_width) } else { (style.max_width, style.min_width, style.min_height) };
    if let Length::Px(v) = max {
        return expand_tracks(list, Some(content(v, rows)), gap);
    }
    let min = match (min, style.aspect_ratio, other_min) {
        (Length::Px(v), _, _) => Some(content(v, rows)),
        (_, Some(r), Length::Px(v)) if r > 0.0 => Some(if rows { content(v, false) / r } else { content(v, true) * r }),
        _ => None,
    };
    let Some(min) = min.filter(|&m| m > 0.0) else {
        return expand_tracks(list, None, gap);
    };
    let fitted = expand_tracks(list, Some(min), gap);
    let size = |t: &TrackList| {
        t.tracks.iter().map(|t| repeat_fixed_size_of(t)).sum::<f32>() + gap * t.tracks.len().saturating_sub(1) as f32
    };
    if size(&fitted) + 0.01 >= min {
        return fitted;
    }
    let (before, per_rep) = auto_repeat_shape(list);
    let explicit = expand_tracks_sized(list, Some(0.0), gap, None, Some(0)).tracks.len();
    let reps = (fitted.tracks.len().saturating_sub(explicit)) / per_rep.max(1);
    let _ = before;
    expand_tracks_sized(list, Some(min), gap, None, Some(reps + 1))
}

/// The fixed size of an expanded track for counting repetitions.
fn repeat_fixed_size_of(t: &Track) -> f32 {
    match (t.min, t.max) {
        (Sizing::Fixed(min), Sizing::Fixed(max)) => max.max(min),
        (_, Sizing::Fixed(v)) | (Sizing::Fixed(v), _) => v,
        _ => 0.0,
    }
}

/// The number of tracks before an automatic repetition, and in one
/// repetition of it.
fn auto_repeat_shape(list: &[TrackListItem]) -> (usize, usize) {
    let mut before = 0;
    for it in list {
        match it {
            TrackListItem::Track { .. } => before += 1,
            TrackListItem::Repeat { count: Some(n), tracks, .. } => before += (*n).max(1) as usize * tracks.iter().filter(|t| matches!(t, TrackListItem::Track { .. })).count(),
            TrackListItem::Repeat { count: None, tracks, .. } => return (before, tracks.iter().filter(|t| matches!(t, TrackListItem::Track { .. })).count()),
            _ => {}
        }
    }
    (before, 0)
}

/// Whether an automatic repetition holds a track without a fixed size
/// (Grid3 §3.2.1: allowed in grid lanes, sized from the items).
fn intrinsic_auto_repeat(list: &[TrackListItem]) -> bool {
    list.iter().any(|it| match it {
        TrackListItem::Repeat { count: None, tracks, .. } => tracks.iter().any(|t| match t {
            TrackListItem::Track { size, .. } => {
                let (min, max) = sizing_of(size, Some(0.0));
                !matches!((min, max), (Sizing::Fixed(_), _) | (_, Sizing::Fixed(_)))
            }
            _ => false,
        }),
        _ => false,
    })
}

/// [`expand_tracks`]; `sizes` are the used sizes of the tracks of a single
/// repetition, in order, which then count automatic repetitions instead of
/// the fixed sizing functions (intrinsic repetitions in grid lanes).
fn expand_tracks_sized(list: &[TrackListItem], avail: Option<f32>, gap: f32, sizes: Option<&[f32]>, reps: Option<usize>) -> TrackList {
    let forced_reps = reps;
    let mut tracks: Vec<Track> = Vec::new();
    let mut names: Vec<Vec<String>> = vec![Vec::new()];
    let push_track = |tracks: &mut Vec<Track>, names: &mut Vec<Vec<String>>, pre: &[String], size: &TrackSize, auto_fit: bool| {
        names.last_mut().unwrap().extend(pre.iter().cloned());
        let mut t = Track::of(size, avail);
        t.auto_fit = auto_fit;
        tracks.push(t);
        names.push(Vec::new());
    };
    // Size taken by non-repeat tracks (for auto-fill counts), and by one
    // automatic repetition; `next` walks the single-repetition sizes.
    let mut index = 0usize;
    let mut next = |size: &TrackSize| -> f32 {
        let v = match sizes {
            Some(sizes) => sizes.get(index).copied().unwrap_or(0.0),
            None => repeat_fixed_size(size, avail),
        };
        index += 1;
        v
    };
    let mut fixed_sum = 0.0f32;
    let mut fixed_count = 0usize;
    let mut inner_size = 0.0f32;
    for it in list {
        match it {
            TrackListItem::Track { size, .. } => {
                fixed_sum += next(size);
                fixed_count += 1;
            }
            TrackListItem::Repeat { count, tracks, .. } => {
                let n = count.map(|n| n.max(1) as usize);
                for _ in 0..n.unwrap_or(1) {
                    for t in tracks {
                        if let TrackListItem::Track { size, .. } = t {
                            let v = next(size);
                            if n.is_some() {
                                fixed_sum += v;
                                fixed_count += 1;
                            } else {
                                inner_size += v;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    for item in list {
        match item {
            // An axis without an inherited span uses the standalone fallback.
            TrackListItem::Subgrid { .. } => {},
            TrackListItem::Track { names: pre, size } => push_track(&mut tracks, &mut names, pre, size, false),
            TrackListItem::Names(n) => names.last_mut().unwrap().extend(n.iter().cloned()),
            TrackListItem::Repeat { count, auto_fit, tracks: inner } => {
                let reps = match count {
                    Some(n) => (*n).max(1) as usize,
                    None if forced_reps.is_some() => forced_reps.unwrap_or(1),
                    None => {
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
                            TrackListItem::Repeat { .. } | TrackListItem::Subgrid { .. } => {}
                        }
                    }
                }
            }
        }
    }
    TrackList { tracks, css_names: std::rc::Rc::new(names.clone()), names, inherited: None }
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

/// Count distinct lines with this name, then continue into the implicit grid
/// on the searched side (Grid2 §8.3). Multiple copies of a name on one line
/// still identify that same line once.
fn named_line(names: &[Vec<String>], name: &str, occurrence: i32) -> i32 {
    let mut left = occurrence.unsigned_abs().max(1) as i64;
    if occurrence >= 0 {
        for (i, line) in names.iter().enumerate() {
            if line.iter().any(|n| n == name) {
                left -= 1;
                if left == 0 { return i as i32; }
            }
        }
        (names.len().saturating_sub(1) as i64 + left).min(i32::MAX as i64) as i32
    } else {
        for (i, line) in names.iter().enumerate().rev() {
            if line.iter().any(|n| n == name) {
                left -= 1;
                if left == 0 { return i as i32; }
            }
        }
        (-left).max(i32::MIN as i64) as i32
    }
}

/// Resolve a named span from its opposite edge, excluding the edge itself.
/// Only implicit lines on the search-direction side have the missing name;
/// an anchor beyond the other side must not count those intervening lines.
fn named_span_line(names: &[Vec<String>], name: &str, count: i32, anchor: i32, forward: bool) -> i32 {
    let mut left = i64::from(count.max(1));
    if forward {
        for (i, line) in names.iter().enumerate() {
            if i as i64 > i64::from(anchor) && line.iter().any(|n| n == name) {
                left -= 1;
                if left == 0 { return i as i32; }
            }
        }
        (i64::from(anchor).max(names.len().saturating_sub(1) as i64) + left).min(i32::MAX as i64) as i32
    } else {
        for (i, line) in names.iter().enumerate().rev() {
            if (i as i64) < i64::from(anchor) && line.iter().any(|n| n == name) {
                left -= 1;
                if left == 0 { return i as i32; }
            }
        }
        (i64::from(anchor).min(0) - left).max(i32::MIN as i64) as i32
    }
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
                LineSpec::Line(find_line(names, name, is_end).unwrap_or_else(|| named_line(names, name, 1)))
            }
        }
        GridLine::NamedNth(name, occurrence, false) => LineSpec::Line(named_line(names, name, *occurrence)),
        // Without an opposite line a named span becomes a span of one.
        // line_specs resolves anchored named spans before placement.
        GridLine::NamedNth(_, _, true) => LineSpec::Span(1),
    }
}

fn named_span(line: &GridLine) -> Option<(&str, i32)> {
    match line {
        GridLine::Named(name, true) => Some((name.as_str(), 1)),
        GridLine::NamedNth(name, count, true) => Some((name.as_str(), *count)),
        _ => None,
    }
}

fn line_specs(start: &GridLine, end: &GridLine, names: &[Vec<String>], n_lines: i32) -> (LineSpec, LineSpec) {
    let mut a = line_spec(start, names, n_lines, false);
    let mut z = line_spec(end, names, n_lines, true);
    if let (Some((name, count)), LineSpec::Line(anchor)) = (named_span(start), z) {
        a = LineSpec::Line(named_span_line(names, name, count, anchor, false));
    }
    if let (Some((name, count)), LineSpec::Line(anchor)) = (named_span(end), a) {
        z = LineSpec::Line(named_span_line(names, name, count, anchor, true));
    }
    (a, z)
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

#[derive(Clone, Copy)]
struct GridItem {
    idx: usize,
    area: Area,
    edges: ItemEdges,
    justify: ItemAlign,
    align: ItemAlign,
    column_names: bool,
    row_names: bool,
    /// Grid3 §3.4.1: ignore sub-item placement while sizing an automatically
    /// positioned subgrid in a lanes parent.
    parent_lanes_auto: bool,
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

/// The phases of §12.5 step 3 for items spanning several tracks.
#[derive(Clone, Copy, PartialEq)]
enum SpanPhase {
    /// Base sizes of tracks with intrinsic minimums, from the items'
    /// minimum contributions (steps 3.1-3.2).
    IntrinsicMinimums,
    /// Base sizes of tracks with max-content minimums, from max-content
    /// contributions (step 3.3).
    MaxContentMinimums,
    /// Growth limits of tracks with intrinsic maximums, from minimum
    /// contributions (step 3.5).
    IntrinsicMaximums,
    /// Growth limits of tracks with max-content maximums (auto,
    /// max-content, fit-content), from max-content contributions (3.6).
    MaxContentMaximums,
}

/// §12.5.1: the increases one spanning item needs in `phase`, folded into
/// `planned` (largest per track). Returns whether any track is affected.
fn distribute_span(tracks: &[Track], c: &Contribution, gap: f32, phase: SpanPhase, planned: &mut [f32]) -> bool {
    let range = c.start..c.end;
    let max_content_max = |t: &Track| matches!(t.max, Sizing::MaxContent | Sizing::Auto | Sizing::FitContent(_));
    let affected: Vec<usize> = range
        .clone()
        .filter(|&i| {
            let t = &tracks[i];
            match phase {
                SpanPhase::IntrinsicMinimums => t.min.is_intrinsic(),
                SpanPhase::MaxContentMinimums => matches!(t.min, Sizing::MaxContent),
                SpanPhase::IntrinsicMaximums => t.max.is_intrinsic(),
                SpanPhase::MaxContentMaximums => max_content_max(t),
            }
        })
        .collect();
    if affected.is_empty() {
        return false;
    }
    let limits_phase = matches!(phase, SpanPhase::IntrinsicMaximums | SpanPhase::MaxContentMaximums);
    let size = |t: &Track| if limits_phase { t.limit.unwrap_or(t.base) } else { t.base };
    let contribution = match phase {
        SpanPhase::IntrinsicMinimums | SpanPhase::IntrinsicMaximums => c.min,
        SpanPhase::MaxContentMinimums | SpanPhase::MaxContentMaximums => c.max,
    };
    let gaps = gap * ((c.end - c.start) as f32 - 1.0);
    let mut space = contribution - range.clone().map(|i| size(&tracks[i])).sum::<f32>() - gaps;
    if space <= 0.0 {
        return true;
    }
    // Up to each track's limit.
    let cap = |t: &Track| -> f32 {
        let fit = match t.max {
            Sizing::FitContent(l) => Some(l),
            _ => None,
        };
        if limits_phase {
            match (t.limit, t.infinitely_growable, fit) {
                (Some(l), false, _) => l,
                (_, _, Some(f)) => f.max(t.base),
                _ => f32::INFINITY,
            }
        } else {
            let l = t.limit.unwrap_or(f32::INFINITY);
            fit.map_or(l, |f| l.min(f.max(t.base)))
        }
    };
    let mut incurred = vec![0.0f32; affected.len()];
    let mut open: Vec<usize> = (0..affected.len()).collect();
    for _ in 0..affected.len() + 1 {
        if space <= 0.001 || open.is_empty() {
            break;
        }
        let share = space / open.len() as f32;
        let mut next = Vec::with_capacity(open.len());
        for &k in &open {
            let t = &tracks[affected[k]];
            let room = (cap(t) - size(t) - incurred[k]).max(0.0);
            let give = share.min(room);
            incurred[k] += give;
            space -= give;
            if room - give > 0.001 {
                next.push(k);
            }
        }
        open = next;
    }
    // Beyond the limits.
    if space > 0.001 {
        let beyond: Vec<usize> = match phase {
            SpanPhase::IntrinsicMinimums => (0..affected.len()).filter(|&k| tracks[affected[k]].max.is_intrinsic()).collect(),
            SpanPhase::MaxContentMinimums => (0..affected.len()).filter(|&k| max_content_max(&tracks[affected[k]])).collect(),
            _ => (0..affected.len()).filter(|&k| {
                let t = &tracks[affected[k]];
                t.limit.is_none() || t.infinitely_growable
            }).collect(),
        };
        let beyond = if beyond.is_empty() && !limits_phase { (0..affected.len()).collect() } else { beyond };
        if !beyond.is_empty() {
            let share = space / beyond.len() as f32;
            for k in beyond {
                incurred[k] += share;
            }
        }
    }
    for (k, &i) in affected.iter().enumerate() {
        planned[i] = planned[i].max(incurred[k]);
    }
    true
}

/// §11 track sizing algorithm for one axis.
fn size_tracks(tracks: &mut [Track], contribs: &[Contribution], avail: Option<f32>, gap: f32, stretch: bool) {
    // Content-based automatic sizes, including auto block sizes, use the
    // max-content constraint. Intrinsic minimum callers must opt in below.
    size_tracks_with_space(tracks, contribs, avail.map(GridSpace::Definite).unwrap_or(GridSpace::MaxContent), gap, stretch);
}

fn size_tracks_with_space(tracks: &mut [Track], contribs: &[Contribution], space: GridSpace, gap: f32, stretch: bool) {
    resolve_intrinsic_tracks(tracks, contribs, gap);
    complete_track_sizing(tracks, contribs, space, gap, stretch);
}

/// Grid2 §12.4-12.5: retain both the resolved bases and growth limits before
/// maximizing. A staged subgrid measure must not confuse these bases with a
/// final min-content constraint or discard the limits needed by a later pass.
fn resolve_intrinsic_tracks(tracks: &mut [Track], contribs: &[Contribution], gap: f32) {
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
    // §12.5 step 3: items spanning several tracks (none flexible), in
    // groups of equal span; each phase applies the largest increase any
    // item of the group needs (§12.5.1).
    let max_span = sorted.iter().map(|c| c.end - c.start).max().unwrap_or(1);
    let mut planned = vec![0.0f32; tracks.len()];
    for span in 2..=max_span {
        let group: Vec<&Contribution> = sorted.iter().copied().filter(|c| c.end - c.start == span && c.end <= tracks.len() && !(c.start..c.end).any(|i| tracks[i].is_flex() || tracks[i].collapsed)).collect();
        if group.is_empty() {
            continue;
        }
        for phase in [SpanPhase::IntrinsicMinimums, SpanPhase::MaxContentMinimums, SpanPhase::IntrinsicMaximums, SpanPhase::MaxContentMaximums] {
            planned.iter_mut().for_each(|p| *p = 0.0);
            let mut any = false;
            for c in &group {
                any |= distribute_span(tracks, c, gap, phase, &mut planned);
            }
            if !any {
                continue;
            }
            for (i, t) in tracks.iter_mut().enumerate() {
                if planned[i] <= 0.0 {
                    continue;
                }
                match phase {
                    SpanPhase::IntrinsicMinimums | SpanPhase::MaxContentMinimums => t.base += planned[i],
                    SpanPhase::IntrinsicMaximums => {
                        if t.limit.is_none() {
                            t.infinitely_growable = true;
                        }
                        t.limit = Some(t.limit.unwrap_or(t.base) + planned[i]);
                    }
                    SpanPhase::MaxContentMaximums => t.limit = Some(t.limit.unwrap_or(t.base) + planned[i]),
                }
            }
            if matches!(phase, SpanPhase::MaxContentMinimums) {
                // Step 3.4: growth limits below their base sizes rise to them.
                for t in tracks.iter_mut() {
                    if let Some(l) = t.limit {
                        if l < t.base {
                            t.limit = Some(t.base);
                        }
                    }
                }
            }
        }
        for t in tracks.iter_mut() {
            if let Some(l) = t.limit {
                if l < t.base {
                    t.limit = Some(t.base);
                }
            }
        }
    }
    // §12.5 step 4: items spanning several tracks, some flexible, all
    // together: their contributions grow only the flexible tracks with
    // intrinsic minimums, in proportion to the flex factors (equally for
    // the part of a total below one).
    let crossing: Vec<&Contribution> = sorted.iter().copied().filter(|c| c.end - c.start > 1 && c.end <= tracks.len() && (c.start..c.end).any(|i| tracks[i].is_flex()) && !(c.start..c.end).any(|i| tracks[i].collapsed)).collect();
    if !crossing.is_empty() {
        for max_content in [false, true] {
            planned.iter_mut().for_each(|p| *p = 0.0);
            for c in &crossing {
                let affected: Vec<usize> = (c.start..c.end).filter(|&i| {
                    let t = &tracks[i];
                    t.is_flex() && if max_content { matches!(t.min, Sizing::MaxContent) } else { t.min.is_intrinsic() }
                }).collect();
                if affected.is_empty() {
                    continue;
                }
                let gaps = gap * ((c.end - c.start) as f32 - 1.0);
                let contribution = if max_content { c.max } else if c.auto_min { 0.0 } else { c.minimum.min(c.min) };
                let space = contribution - (c.start..c.end).map(|i| tracks[i].base).sum::<f32>() - gaps;
                if space <= 0.0 {
                    continue;
                }
                let flex = |i: usize| match tracks[i].max { Sizing::Flex(f) => f, _ => 0.0 };
                let total: f32 = affected.iter().map(|&i| flex(i)).sum();
                for &i in &affected {
                    let share = if total >= 1.0 {
                        space * flex(i) / total
                    } else {
                        space * flex(i) + space * (1.0 - total) / affected.len() as f32
                    };
                    planned[i] = planned[i].max(share);
                }
            }
            for (i, t) in tracks.iter_mut().enumerate() {
                if planned[i] > 0.0 {
                    t.base += planned[i];
                    if let Some(l) = t.limit {
                        if l < t.base {
                            t.limit = Some(t.base);
                        }
                    }
                }
            }
        }
    }
    for t in tracks.iter_mut() {
        t.infinitely_growable = false;
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
}

/// Complete §12.6-12.8 from the already resolved bases and growth limits.
/// Ordinary sizing runs this immediately; separating the stages does not
/// change its available-space or intrinsic-constraint semantics.
fn complete_track_sizing(tracks: &mut [Track], contribs: &[Contribution], space: GridSpace, gap: f32, stretch: bool) {
    let n_gaps = tracks.iter().filter(|t| !t.collapsed).count().saturating_sub(1) as f32;
    // §11.6 maximize tracks.
    if let GridSpace::Definite(a) = space {
        let used: f32 = tracks.iter().map(|t| t.base).sum::<f32>() + gap * n_gaps;
        let free = a - used;
        if free > 0.0 {
            let cands: Vec<usize> = (0..tracks.len()).filter(|&i| !tracks[i].collapsed && !tracks[i].is_flex()).collect();
            distribute_extra(tracks, &cands, free, false, false);
        }
    } else if space == GridSpace::MaxContent {
        for track in tracks.iter_mut().filter(|track| !track.collapsed && !track.is_flex()) {
            track.base = track.base.max(track.limit_or_base());
        }
    }
    // §11.7 expand flexible tracks.
    let flex_idx: Vec<usize> = (0..tracks.len()).filter(|&i| tracks[i].is_flex() && !tracks[i].collapsed).collect();
    if !flex_idx.is_empty() {
        let factor = |t: &Track| match t.max {
            Sizing::Flex(f) => f,
            _ => 0.0,
        };
        let fr = match space {
            GridSpace::MinContent => 0.0,
            GridSpace::Definite(a) => {
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
            GridSpace::MaxContent | GridSpace::LegacyIndefinite => {
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
    if let GridSpace::Definite(a) = space {
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
        // The fallback of space-around/space-evenly is `safe center`
        // (CSS Align 3 §5.1): overflowing tracks start at the start edge.
        Some(a) if a < sum && matches!(dist, ContentDistribution::SpaceAround | ContentDistribution::SpaceEvenly) => (0.0, 0.0),
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

/// Whether `justify-content` / `align-content` stretch `auto` tracks
/// (§12.8): `stretch` and `normal`, which behaves as `stretch`.
pub(crate) fn stretches_tracks(style: &ComputedStyle, rows: bool) -> bool {
    let (dist, flags) = if rows { (style.align_content, style.align_content_flags) } else { (style.justify_content, style.justify_content_flags) };
    dist == ContentDistribution::Stretch || (dist == ContentDistribution::FlexStart && flags.has(rabbithole_style::AlignFlags::NORMAL))
}

/// The fraction of the free space before an absolutely positioned child
/// with auto insets (0 start, 0.5 center, 1 end): its self-alignment, or the
/// container's `*-items` for `auto`. Baseline values and `normal`/`stretch`
/// fall back to start (`last baseline` to end). `reversed`: the axis runs
/// from right to left (an RTL container's inline axis); `self_reversed`:
/// likewise for the item's own direction (`self-start`/`self-end`).
fn static_align_factor(value: ItemAlign, flags: rabbithole_style::AlignFlags, items: ItemAlign, items_flags: rabbithole_style::AlignFlags, reversed: bool, self_reversed: bool) -> f32 {
    use rabbithole_style::AlignFlags as F;
    let (value, flags) = if value == ItemAlign::Auto { (items, items_flags) } else { (value, flags) };
    let reversed = if flags.has(F::SELF) { self_reversed } else { reversed };
    let end = match value {
        ItemAlign::Center => return 0.5,
        ItemAlign::FlexEnd => true,
        ItemAlign::Baseline => flags.has(F::LAST),
        _ => false,
    };
    if flags.has(F::LEFT) {
        return 0.0;
    }
    if flags.has(F::RIGHT) {
        return 1.0;
    }
    if end != reversed { 1.0 } else { 0.0 }
}

/// Whether the columns of an RTL grid are laid out mirrored: column line 1
/// at the right content edge (§7.1, the inline axis runs right to left).
/// Laid out left to right and then mirrored as a whole; grids sharing
/// columns with a subgrid keep the unmirrored geometry.
fn mirrored(b: &LayoutBox, cols: &TrackList) -> bool {
    b.style.direction == rabbithole_style::Direction::Rtl && cols.inherited.as_ref().is_none_or(|shared| shared.flipped) && !b.children.iter().any(|c| subgrid::requests(c).0)
}

/// Inline-axis self-alignment for the left-to-right pass of a grid whose
/// columns are mirrored afterwards when `mirror` (start/end, `self-*` and
/// `left`/`right` resolved to the physical side they must end up on).
fn justify_for_pass(value: ItemAlign, flags: rabbithole_style::AlignFlags, item_rtl: bool, mirror: bool) -> ItemAlign {
    use rabbithole_style::AlignFlags as F;
    if !matches!(value, ItemAlign::FlexStart | ItemAlign::FlexEnd) {
        return value;
    }
    let end = value == ItemAlign::FlexEnd;
    let right = if flags.has(F::LEFT) {
        false
    } else if flags.has(F::RIGHT) {
        true
    } else if flags.has(F::SELF) {
        end != item_rtl
    } else {
        end != mirror
    };
    if right != mirror { ItemAlign::FlexEnd } else { ItemAlign::FlexStart }
}

/// The size of the tracks [start, end) with their gaps when all of them
/// have fixed sizing functions (a definite percentage basis before sizing).
fn fixed_span(tracks: &[Track], start: i32, end: i32, gap: f32) -> Option<f32> {
    let (start, end) = (start.max(0) as usize, end.max(0) as usize);
    if start >= end || end > tracks.len() {
        return None;
    }
    let mut sum = gap * (end - start - 1) as f32;
    for t in &tracks[start..end] {
        match (t.min, t.max) {
            (Sizing::Fixed(a), Sizing::Fixed(b)) => sum += a.max(b),
            _ => return None,
        }
    }
    Some(sum)
}

/// The sum of the fixed maximum sizing functions of [start, end) and their
/// gaps, when all of them have one (Grid2 §6.6: the automatic minimum size
/// of an item in such tracks is clamped to fit its area).
fn fixed_max_span(tracks: &[Track], start: i32, end: i32, gap: f32) -> Option<f32> {
    let (start, end) = (start.max(0) as usize, end.max(0) as usize);
    if start >= end || end > tracks.len() {
        return None;
    }
    let mut sum = gap * (end - start - 1) as f32;
    for t in &tracks[start..end] {
        match t.max {
            Sizing::Fixed(v) => sum += v,
            _ => return None,
        }
    }
    Some(sum)
}

/// Definite sizes known before track sizing for one item: the clamp of its
/// automatic minimum width, and the block-size percentage basis.
#[derive(Clone, Copy, Default)]
struct ItemBases {
    min_limit: Option<f32>,
    row_basis: Option<f32>,
}

impl ItemBases {
    fn new(it: &GridItem, cols: Option<(&TrackList, f32)>, rows: Option<(&TrackList, f32)>) -> ItemBases {
        // Shared (subgrid) tracks are placeholders while they are sized.
        ItemBases {
            min_limit: cols.filter(|(c, _)| c.sized()).and_then(|(c, gap)| fixed_max_span(&c.tracks, it.area.col.0, it.area.col.1, gap)),
            row_basis: rows.filter(|(r, _)| r.sized()).and_then(|(r, gap)| fixed_span(&r.tracks, it.area.row.0, it.area.row.1, gap)),
        }
    }
}

/// A definite `min-height` / `max-height` as a content-box height
/// (lengths only).
fn definite_height_limit(style: &ComputedStyle, cbw: f32, max: bool) -> Option<f32> {
    let Length::Px(v) = (if max { style.max_height } else { style.min_height }) else { return None };
    let pad = crate::block::resolve_edges(&style.padding, cbw);
    let pb = pad.top + pad.bottom + style.border_width.top + style.border_width.bottom;
    let v = if style.box_sizing == BoxSizing::BorderBox { v - pb } else { v };
    (v > 0.0 || max).then_some(v.max(0.0))
}

/// Rows sized in an indefinite height that end up shorter than a definite
/// `min-height` are maximized/stretched/flexed in that size instead
/// (§12.6-12.8); flexible rows that make it taller than `max-height` are
/// redone in that size (§12.7.1). Returns the size used.
fn resize_rows_to_limits(tracks: &mut [Track], contribs: &[Contribution], style: &ComputedStyle, cbw: f32, gap: f32, stretch: bool) -> Option<f32> {
    let visible = tracks.iter().filter(|t| !t.collapsed).count();
    let used = tracks.iter().map(|t| t.base).sum::<f32>() + gap * visible.saturating_sub(1) as f32;
    if let Some(min) = definite_height_limit(style, cbw, false).filter(|&min| used + 0.01 < min) {
        size_tracks_with_space(tracks, contribs, GridSpace::Definite(min), gap, stretch);
        return Some(min);
    }
    if let Some(max) = definite_height_limit(style, cbw, true).filter(|&max| used > max + 0.01 && tracks.iter().any(|t| t.is_flex())) {
        size_tracks_with_space(tracks, contribs, GridSpace::Definite(max), gap, stretch);
        return Some(max);
    }
    None
}

/// Absolutely positioned descendants of `b` whose containing block is
/// outside it (no positioned box in between).
fn abs_descendants(b: &LayoutBox, out: &mut Vec<(rabbithole_dom::NodeId, std::rc::Rc<ComputedStyle>)>) {
    if b.no_abs_below {
        return;
    }
    for c in &b.children {
        if c.flow == Flow::Absolute {
            if let Some(node) = c.abs_node() {
                out.push((node, c.style.clone()));
            }
        } else if c.style.position == rabbithole_style::Position::Static && c.formatting != crate::tree::Formatting::Grid && !c.wm_wrapper {
            // A nested grid (a subgrid) resolves its own children's lines.
            abs_descendants(c, out);
        }
    }
}

/// Percentage margins and paddings of a grid item resolve against the
/// inline size of its grid area (Grid2 §6.4), once the columns are sized.
fn percent_edges(style: &ComputedStyle) -> bool {
    let pct = |l: &Length| matches!(l, Length::Percent(_) | Length::Calc(..) | Length::Math(_));
    [&style.padding, &style.margin].iter().any(|e| pct(&e.top) || pct(&e.right) || pct(&e.bottom) || pct(&e.left))
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
    /// An automatic subgrid span comes from its line-name list (Grid2 §8/§9).
    /// Explicit spans and distinct start/end lines retain their own span.
    fn item_pair(&mut self,child:&LayoutBox,rows:bool,start:LineSpec,end:LineSpec)->Result<(i32,i32),i32> {
        if matches!(start,LineSpec::Span(_))||matches!(end,LineSpec::Span(_))||matches!((start,end),(LineSpec::Line(a),LineSpec::Line(b)) if a!=b) {
            return resolve_pair(start,end);
        }
        let Some(span)=subgrid::automatic_span(child,rows) else {return resolve_pair(start,end);};
        let edge=match (start,end) {(LineSpec::Line(line),_)|(_,LineSpec::Line(line))=>line.unsigned_abs()as usize,_=>0};
        self.grid_work.charge(span.saturating_add(edge).saturating_mul(8));
        let span=span.min(i32::MAX as usize)as i32;
        match (start,end) {
            (LineSpec::Line(line),_)=>Ok((line,line.saturating_add(span))),
            (_,LineSpec::Line(line))=>Ok((line.saturating_sub(span),line)),
            _=>Err(span),
        }
    }

    /// Lay out the grid items of `b`; returns the content height.
    pub(crate) fn layout_grid(&mut self, b: &mut LayoutBox, cb: &Cb) -> f32 {
        self.grid_layouts += 1;
        if self.grid_publication.enabled() { b.used_grid_tracks = [None, None]; }
        if b.style.grid_lanes || subgrid::inherits_stacking(b) {
            return self.layout_grid_lanes(b, cb);
        }
        let style = b.style.clone();
        let cbw = cb.width;
        let content_x = b.content_rect.x;
        let content_y = b.content_rect.y;
        let col_gap = subgrid::gap(b,false,cbw);
        let row_gap = subgrid::gap(b,true,cb.height.unwrap_or(0.0));

        let (mut items, mut cols, mut rows) = self.place_items(b, &style, Some(cbw), cb.height, col_gap, row_gap, content_x, content_y);
        let placement = LAST_PLACEMENT.with(|p| p.get());

        let project=b.children.iter().any(|child|subgrid::requests(child)!=(false,false));
        let col_stretch=stretches_tracks(&style,false);
        let row_stretch=stretches_tracks(&style,true);
        let ordinary_axis_consumer=traversal::ordinary_orthogonal_grid(b);
        let mirror=mirrored(b,&cols);
        let mut row_avail=cb.height;
        let mut content_height: Option<f32> = None;
        let mut col_pos;
        if ordinary_axis_consumer {
            let spaces=[phases::AvailableSpace::Definite(cbw),cb.height.map(phases::AvailableSpace::Definite).unwrap_or(phases::AvailableSpace::Indefinite)];
            let inputs=traversal::ordinary_inputs(b,spaces,[col_gap,row_gap]);
            (cols,rows)=self.ordinary_grid_axis_tracks(b,&items,cols,rows,inputs,phases::OperationKind::Layout,cbw);
            col_pos=subgrid::positions(&cols,col_gap,Some(cbw),style.justify_content).0;
        } else {
            let mut col_contribs=Vec::new();
            let mut row_contribs=Vec::new();
            // Column sizing.
            if cols.inherited.is_none() {
                col_contribs=if project {self.projected_columns(b,&items,&cols,Some(&rows),col_gap,row_gap,cbw,GridGeometry{inline:Some(cbw),block:cb.height},MeasurePhase::Initial)} else {self.column_contributions(b,&items,cbw,Some((&cols,col_gap)),Some((&rows,row_gap)))};
                size_tracks(&mut cols.tracks, &col_contribs, Some(cbw), col_gap, col_stretch);
            }
            col_pos = subgrid::positions(&cols, col_gap, Some(cbw), style.justify_content).0;

            // Row sizing: contributions are item heights at their column width.
            if rows.inherited.is_none() {
                row_contribs=if project {self.projected_rows(b,&items,&rows,&cols,&col_pos,col_gap,row_gap,cbw,cb.height,MeasurePhase::Initial)} else {self.row_contributions(b,&items,&cols,&col_pos,col_gap,cbw,&rows,row_gap)};
                let space=subgrid::standalone_space(b,true,cb.height);
                size_tracks_with_space(&mut rows.tracks, &row_contribs, space, row_gap, row_stretch);
                // §12.6-12.8: with an indefinite height, a definite
                // min-height is the free space for maximizing, flexible and
                // stretched tracks.
                if cb.height.is_none() && space == GridSpace::MaxContent {
                    row_avail = resize_rows_to_limits(&mut rows.tracks, &row_contribs, &style, cbw, row_gap, row_stretch).or(row_avail);
                }
                // Grid2 §7.2.1: percentage rows of an indefinite height
                // sized the grid as `auto`; they then resolve against the
                // resulting content height for laying out the items.
                if cb.height.is_none() && row_avail.is_none() && rows.tracks.iter().any(|t| t.deferred.is_some()) {
                    let height = subgrid::positions(&rows, row_gap, row_avail, style.align_content).1;
                    for t in rows.tracks.iter_mut() {
                        if let Some(size) = t.deferred.take() {
                            (t.min, t.max) = sizing_of(&size, Some(height));
                        }
                        t.base = 0.0;
                        t.limit = None;
                        t.infinitely_growable = false;
                    }
                    size_tracks_with_space(&mut rows.tracks, &row_contribs, GridSpace::Definite(height), row_gap, row_stretch);
                    row_avail = Some(height);
                    content_height = Some(height);
                }
            }
            // Grid2 §12.1 steps 3-4: at most one adjustment of each axis.
            // Ordinary grids retain the existing path; subgrid descendants now
            // observe the used opposite-axis sizes instead of a recursive layout.
            if project {
                if cols.inherited.is_none() {
                    let adjusted=self.projected_columns(b,&items,&cols,Some(&rows),col_gap,row_gap,cbw,GridGeometry{inline:Some(cbw),block:cb.height},MeasurePhase::Adjusted);
                    if adjusted!=col_contribs {
                        size_tracks(&mut cols.tracks,&adjusted,Some(cbw),col_gap,col_stretch);
                        col_pos=subgrid::positions(&cols,col_gap,Some(cbw),style.justify_content).0;
                    }
                }
                if rows.inherited.is_none() {
                    let adjusted=self.projected_rows(b,&items,&rows,&cols,&col_pos,col_gap,row_gap,cbw,cb.height,MeasurePhase::Adjusted);
                    if adjusted!=row_contribs {
                        let space=subgrid::standalone_space(b,true,cb.height);
                        size_tracks_with_space(&mut rows.tracks,&adjusted,space,row_gap,row_stretch);
                    }
                }
            }
        }
        let (mut row_pos, rows_extent) = subgrid::positions(&rows, row_gap, row_avail, style.align_content);
        let rows_extent = content_height.unwrap_or(rows_extent);
        // A percentage row gap of an indefinite height is zero for sizing,
        // then resolves against the resulting height for layout (CSS Align 3 §8.1).
        let row_gap = if cb.height.is_none() && rows.inherited.is_none() && !style.row_gap_normal && matches!(style.row_gap, Length::Percent(_) | Length::Calc(..) | Length::Math(_)) {
            let used = style.row_gap.resolve_or_zero(rows_extent).max(0.0);
            if used != row_gap {
                row_pos = subgrid::positions(&rows, used, row_avail, style.align_content).0;
            }
            used
        } else {
            row_gap
        };

        // Final item layout and alignment (§10).
        for it in items.iter_mut() {
            let (ax, aw) = span_size(&cols.tracks, &col_pos, it.area.col.0 as usize, it.area.col.1 as usize);
            let (ay, ah) = span_size(&rows.tracks, &row_pos, it.area.row.0 as usize, it.area.row.1 as usize);
            if ordinary_axis_consumer||subgrid::requests(&b.children[it.idx])!=(false,false)||percent_edges(&b.children[it.idx].style) {*it=subgrid::rebase_item(it,&b.children[it.idx].style,cbw,aw);}
            let child=&mut b.children[it.idx];
            let (column_request,row_request)=subgrid::requests(child);
            if !column_request&&!row_request {
                if ordinary_axis_consumer {
                    self.layout_ordinary_grid_item(child,it,ax+content_x,ay+content_y,aw,ah);
                } else {
                    self.layout_grid_item(child,it,ax+content_x,ay+content_y,aw,ah,cbw);
                }
                if mirror {
                    // The margin box mirrored about the content box; margins stay physical.
                    let (x, w, m) = (child.rect.x, child.rect.w, child.margin);
                    child.translate(2.0 * content_x + cbw - 2.0 * x - w - m.right + m.left, 0.0);
                }
                continue;
            }
            self.grid_work.charge(1);
            let columns=column_request.then(||subgrid::section(&cols,&col_pos,it.area.col.0 as usize,it.area.col.1 as usize,col_gap,true));
            let rows=row_request.then(||subgrid::section(&rows,&row_pos,it.area.row.0 as usize,it.area.row.1 as usize,row_gap,true));
            self.with_subgrid(child,columns,rows,aw,|this,child| {
                let (justify,align,auto)=(it.justify,it.align,it.edges.auto);
                subgrid::stretch_item(child,it);
                this.layout_grid_item(child,it,ax+content_x,ay+content_y,aw,ah,cbw);
                it.justify=justify;it.align=align;it.edges.auto=auto;
            });
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

        // Absolutely positioned children: with a positioned grid container
        // the grid area of their line placement is the containing block
        // (§9.1; an auto line is the padding edge). With auto insets they
        // are aligned by their self-alignment in a static-position
        // rectangle: that area, or the content box (§9.4, CSS Align 3 §A).
        let positioned = style.position != rabbithole_style::Position::Static;
        if b.children.iter().any(|c| c.flow == Flow::Absolute) || (positioned && !b.no_abs_below) {
            let pad = crate::block::resolve_edges(&style.padding, cbw);
            let content_h = cb.height.unwrap_or(rows_extent);
            let (px0, px1) = (content_x - pad.left, content_x + cbw + pad.right);
            let (py0, py1) = (content_y - pad.top, content_y + content_h + pad.bottom);
            // A line's position as a start edge (after any gutter before it)
            // or as an end edge (before the gutter after the previous track).
            let line_x = |pos: &[f32], tracks: &[Track], i: i32, end: bool| -> f32 {
                if tracks.is_empty() {
                    return 0.0;
                }
                let i = i.clamp(0, tracks.len() as i32) as usize;
                if (end && i > 0) || i == tracks.len() {
                    pos[i - 1] + tracks[i - 1].base
                } else {
                    pos[i]
                }
            };
            let rtl = mirror;
            // The grid area of a line placement (None when all lines are auto).
            let area_of = |s: &ComputedStyle| -> Option<Rect> {
                // Lines resolve against the explicit grid, which starts after
                // the leading implicit tracks.
                let axis = |start: &rabbithole_style::GridLine, end: &rabbithole_style::GridLine, names: &[Vec<String>], off: i32, n: i32| -> (Option<i32>, Option<i32>) {
                    let names = &names[(off.max(0) as usize).min(names.len())..];
                    let (start, end) = line_specs(start, end, names, n);
                    let a = match start {
                        LineSpec::Line(l) => Some(l),
                        _ => None,
                    };
                    let e = match end {
                        LineSpec::Line(l) => Some(l),
                        LineSpec::Span(k) => a.map(|a| a + k),
                        LineSpec::Auto => None,
                    };
                    let a = match (a, start) {
                        (None, LineSpec::Span(k)) => e.map(|e| e - k),
                        (a, _) => a,
                    };
                    (a.map(|l| l + off), e.map(|l| l + off))
                };
                // Lines outside the grid are auto for absolutely positioned
                // boxes (§9.1): the padding edge.
                let n_cols = cols.tracks.len() as i32 + 1;
                let n_rows = rows.tracks.len() as i32 + 1;
                let inside = |n: i32| move |l: &i32| (0..n).contains(l);
                let (c0, c1) = axis(&s.grid_column_start, &s.grid_column_end, &cols.names, placement.col_off, placement.col_lines);
                let (r0, r1) = axis(&s.grid_row_start, &s.grid_row_end, &rows.names, placement.row_off, placement.row_lines);
                let (c0, c1, r0, r1) = (c0.filter(inside(n_cols)), c1.filter(inside(n_cols)), r0.filter(inside(n_rows)), r1.filter(inside(n_rows)));
                if c0.is_none() && c1.is_none() && r0.is_none() && r1.is_none() {
                    return None;
                }
                let (x0, x1) = if mirror {
                    let x = |l, end| content_x + cbw - line_x(&col_pos, &cols.tracks, l, end);
                    (c1.map(|l| x(l, true)).unwrap_or(px0), c0.map(|l| x(l, false)).unwrap_or(px1))
                } else {
                    let x = |l, end| content_x + line_x(&col_pos, &cols.tracks, l, end);
                    (c0.map(|l| x(l, false)).unwrap_or(px0), c1.map(|l| x(l, true)).unwrap_or(px1))
                };
                let y0 = r0.map(|l| content_y + line_x(&row_pos, &rows.tracks, l, false)).unwrap_or(py0);
                let y1 = r1.map(|l| content_y + line_x(&row_pos, &rows.tracks, l, true)).unwrap_or(py1);
                Some(Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)))
            };
            for c in b.children.iter_mut() {
                if c.flow != Flow::Absolute {
                    continue;
                }
                let factor = (
                    static_align_factor(c.style.justify_self, c.style.justify_self_flags, style.justify_items, style.justify_items_flags, rtl, c.style.direction == rabbithole_style::Direction::Rtl),
                    static_align_factor(c.style.align_self, c.style.align_self_flags, style.align_items, style.align_items_flags, false, false),
                );
                // With the grid as containing block, auto lines are the
                // padding edges (§9.1); otherwise the static position is
                // the content box's.
                let content = if positioned { (px0, py0, px1 - px0, py1 - py0) } else { (content_x, content_y, cbw, content_h) };
                let area = c.abs_node().filter(|_| positioned).and_then(|node| area_of(&c.style).map(|area| (node, area)));
                let Some((node, area)) = area else {
                    c.static_pos = (content.0, content.1);
                    c.static_align = Some(crate::tree::StaticAlign { size: (content.2, content.3), factor });
                    continue;
                };
                self.abs_cb.insert(node, area);
                // With auto insets the item is aligned in its area.
                c.static_pos = (area.x, area.y);
                c.static_align = Some(crate::tree::StaticAlign { size: (area.w, area.h), factor });
            }
            // Deeper absolutely positioned descendants whose containing block
            // is this grid are placed by their grid lines too (§9.1).
            if positioned {
                let mut found = Vec::new();
                for item in b.children.iter().filter(|c| c.flow != Flow::Absolute && c.style.position == rabbithole_style::Position::Static && c.formatting != crate::tree::Formatting::Grid && !c.wm_wrapper) {
                    abs_descendants(item, &mut found);
                }
                for (node, style) in found {
                    if let Some(area) = area_of(&style) {
                        self.abs_cb.insert(node, area);
                    }
                }
            }
        }

        if self.grid_publication.enabled() {
            b.used_grid_tracks[0] = resolved::capture(b, false, &cols, GridSpace::Definite(cbw), &mut self.grid_work);
            b.used_grid_tracks[1] = resolved::capture(b, true, &rows, subgrid::standalone_space(b, true, cb.height), &mut self.grid_work);
        }
        rows_extent.max(0.0)
    }

    /// Build tracks, resolve placements (§8) and collapse auto-fit tracks.
    #[allow(clippy::too_many_arguments)]
    fn place_items(&mut self, b: &mut LayoutBox, style: &ComputedStyle, avail_w: Option<f32>, avail_h: Option<f32>, col_gap: f32, row_gap: f32, content_x: f32, content_y: f32) -> (Vec<GridItem>, TrackList, TrackList) {
        let mut cols = subgrid::tracks(b,false,avail_w,col_gap);
        let mut rows = subgrid::tracks(b,true,avail_h,row_gap);
        let sized_cols = cols.tracks.len();
        let sized_rows = rows.tracks.len();
        // Named areas → implicit line names (§7.3).
        let areas = &style.grid_template_areas;
        if !areas.is_empty() {
            let n_area_rows = areas.len();
            let n_area_cols = areas.iter().map(|r| r.len()).max().unwrap_or(0);
            while cols.inherited.is_none() && cols.tracks.len() < n_area_cols {
                cols.tracks.push(auto_track(&style.grid_auto_columns, (cols.tracks.len() - sized_cols) as i32, avail_w));
                cols.names.push(Vec::new());
                std::rc::Rc::make_mut(&mut cols.css_names).push(Vec::new());
            }
            while rows.inherited.is_none() && rows.tracks.len() < n_area_rows {
                rows.tracks.push(auto_track(&style.grid_auto_rows, (rows.tracks.len() - sized_rows) as i32, avail_h));
                rows.names.push(Vec::new());
                std::rc::Rc::make_mut(&mut rows.css_names).push(Vec::new());
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
                    let (c0,c1)=(c.min(cols.names.len()-1),(c1+1).min(cols.names.len()-1));
                    let (r0,r1)=(r.min(rows.names.len()-1),(r1+1).min(rows.names.len()-1));
                    cols.names[c0].push(format!("{name}-start"));
                    cols.names[c1].push(format!("{name}-end"));
                    rows.names[r0].push(format!("{name}-start"));
                    rows.names[r1].push(format!("{name}-end"));
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
            let (rs, re) = line_specs(&s.grid_row_start, &s.grid_row_end, &rows.names, n_row_lines);
            let (cs, ce) = line_specs(&s.grid_column_start, &s.grid_column_end, &cols.names, n_col_lines);
            let (mut row,mut col)=(self.item_pair(&b.children[idx],true,rs,re),self.item_pair(&b.children[idx],false,cs,ce));
            if cols.inherited.is_some() {col=subgrid::clamp_placement(col,cols.tracks.len());}
            if rows.inherited.is_some() {row=subgrid::clamp_placement(row,rows.tracks.len());}
            pending.push(Pending {
                idx,
                row,
                col,
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
            std::rc::Rc::make_mut(&mut rows.css_names).insert(0, Vec::new());
        }
        for distance in 1..=col_off {
            cols.tracks.insert(0, auto_track(&style.grid_auto_columns, -distance, avail_w));
            cols.names.insert(0, Vec::new());
            std::rc::Rc::make_mut(&mut cols.css_names).insert(0, Vec::new());
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
        let mut overlap = false;
        for (k, p) in pending.iter().enumerate() {
            if let (Ok(r), Ok(c)) = (p.row, p.col) {
                overlap = overlap || !occ.fits(r.0, r.1, c.0, c.1);
                occ.mark(r.0, r.1, c.0, c.1);
                placed[k] = Some(Area { row: r, col: c });
                if column_flow {
                    n_minor = n_minor.max(r.1);
                } else {
                    n_minor = n_minor.max(c.1);
                }
            }
        }
        b.grid_items_overlap = overlap;
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
        for area in placed.iter_mut().flatten() {
            if cols.inherited.is_some() {area.col=subgrid::clamp_placement(Ok(area.col),cols.tracks.len()).unwrap();}
            if rows.inherited.is_some() {area.row=subgrid::clamp_placement(Ok(area.row),rows.tracks.len()).unwrap();}
        }
        let mut max_row = rows.tracks.len() as i32;
        let mut max_col = cols.tracks.len() as i32;
        for a in placed.iter().flatten() {
            max_row = max_row.max(a.row.1);
            max_col = max_col.max(a.col.1);
        }
        // Tracks introduced by grid-template-areas already consumed entries
        // of the forward auto-size pattern; implicit tracks continue it.
        let mut ci = cols.tracks.len() - col_off as usize - sized_cols;
        while cols.inherited.is_none() && (cols.tracks.len() as i32) < max_col {
            cols.tracks.push(auto_track(&style.grid_auto_columns, ci as i32, avail_w));
            cols.names.push(Vec::new());
            std::rc::Rc::make_mut(&mut cols.css_names).push(Vec::new());
            ci += 1;
        }
        let mut ri = rows.tracks.len() - row_off as usize - sized_rows;
        while rows.inherited.is_none() && (rows.tracks.len() as i32) < max_row {
            rows.tracks.push(auto_track(&style.grid_auto_rows, ri as i32, avail_h));
            rows.names.push(Vec::new());
            std::rc::Rc::make_mut(&mut rows.css_names).push(Vec::new());
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

        let mirror = mirrored(b, &cols);
        let mut items = Vec::with_capacity(pending.len());
        for (k, p) in pending.iter().enumerate() {
            let area = placed[k].unwrap_or(Area { row: (0, 1), col: (0, 1) });
            let cs = b.children[p.idx].style.clone();
            // `self-start`/`self-end` use the item's own start side in each
            // physical axis: its inline or block start, by writing mode.
            // (Containers with a vertical writing mode are laid out in their
            // own logical coordinates, where only the direction counts.)
            use rabbithole_style::WritingMode as Wm;
            let item_rtl = cs.direction == rabbithole_style::Direction::Rtl;
            let horizontal = style.writing_mode == Wm::HorizontalTb;
            let start_right = match cs.writing_mode {
                _ if !horizontal => item_rtl,
                Wm::HorizontalTb => item_rtl,
                Wm::VerticalRl => true,
                Wm::VerticalLr | Wm::SidewaysLr => false,
            };
            let start_bottom = match cs.writing_mode {
                _ if !horizontal => false,
                Wm::HorizontalTb => false,
                Wm::VerticalRl | Wm::VerticalLr => item_rtl,
                Wm::SidewaysLr => !item_rtl,
            };
            let justify = match cs.justify_self {
                ItemAlign::Auto => justify_for_pass(style.justify_items, style.justify_items_flags, start_right, mirror),
                a => justify_for_pass(a, cs.justify_self_flags, start_right, mirror),
            };
            let (align, align_flags) = match cs.align_self {
                ItemAlign::Auto => (style.align_items, style.align_items_flags),
                a => (a, cs.align_self_flags),
            };
            let align = if start_bottom && align_flags.has(rabbithole_style::AlignFlags::SELF) {
                match align {
                    ItemAlign::FlexStart => ItemAlign::FlexEnd,
                    ItemAlign::FlexEnd => ItemAlign::FlexStart,
                    a => a,
                }
            } else {
                align
            };
            let mut item=GridItem {
                idx: p.idx,
                area,
                edges: ItemEdges::new(&cs, avail_w.unwrap_or(0.0)),
                justify,
                align,
                column_names: true,
                row_names: true,
                parent_lanes_auto: false,
            };
            subgrid::item_edges(b,&mut item,col_gap,row_gap);
            items.push(item);
        }
        LAST_PLACEMENT.with(|p| p.set(Placement { row_off, col_off, row_lines: n_row_lines, col_lines: n_col_lines }));
        (items, cols, rows)
    }

    /// Column contributions: min-/max-content widths incl. margins/padding/border.
    fn column_contributions(&mut self,b:&mut LayoutBox,items:&[GridItem],cbw:f32,cols:Option<(&TrackList,f32)>,rows:Option<(&TrackList,f32)>)->Vec<Contribution> {
        items.iter().map(|it|self.column_contribution(&mut b.children[it.idx],it,cbw,ItemBases::new(it,cols,rows))).collect()
    }

    /// Shared-axis items are empty for this sizing pass: their descendants
    /// contribute at the corresponding parent lines (Grid2 §12.1).
    #[allow(clippy::too_many_arguments)]
    fn projected_columns(&mut self,b:&mut LayoutBox,items:&[GridItem],cols:&TrackList,rows:Option<&TrackList>,col_gap:f32,row_gap:f32,cbw:f32,geometry:GridGeometry,phase:MeasurePhase)->Vec<Contribution> {
        let _probe = self.grid_publication.probe_grid();
        let style=b.style.clone();
        let mut out=Vec::new();
        for it in items {
            let child=&mut b.children[it.idx];
            let (column_request,row_request)=subgrid::requests(child);
            if !column_request&&!row_request {
                out.push(self.column_contribution(child,it,cbw,ItemBases::new(it,Some((cols,col_gap)),rows.map(|r|(r,row_gap)))));
                continue;
            }
            self.grid_work.charge(1);
            self.grid_work.tracks(cols);
            let column_geometry=Some(subgrid::sizing_section_aligned(cols,it.area.col.0 as usize,it.area.col.1 as usize,col_gap,it.column_names,geometry.inline,style.justify_content));
            let reference=subgrid::scope_reference(&column_geometry,0.);
            let item=subgrid::rebase_item(it,&child.style,cbw,reference);let it=&item;
            let row_source=if !row_request {None} else {rows.map_or(Some(subgrid::AxisInheritance::Stacking),|rows| {
                self.grid_work.tracks(rows);
                if phase==MeasurePhase::Initial {subgrid::fixed_section_aligned(rows,it.area.row.0 as usize,it.area.row.1 as usize,row_gap,it.row_names,geometry.block,style.align_content)}
                else {let (positions,_)=subgrid::positions(rows,row_gap,geometry.block,style.align_content);Some(subgrid::section(rows,&positions,it.area.row.0 as usize,it.area.row.1 as usize,row_gap,it.row_names))}
            })};
            if column_request {
                let columns=column_geometry.unwrap();
                let columns=subgrid::item_layer_source(columns,it,&child.style,false,reference);
                let rows=if row_request {row_source} else {None};
                let mut nested=self.with_subgrid(child,Some(columns),rows,reference,|this,child|this.subgrid_axis_contributions(child,false,None,phase));
                if it.parent_lanes_auto {nested=subgrid::automatic_contributions(&mut self.grid_work,nested,(it.area.col.1-it.area.col.0)as usize);}
                for c in &mut nested {c.start+=it.area.col.0 as usize;c.end+=it.area.col.0 as usize;}
                self.grid_work.charge(nested.len());
                out.extend(nested);
            } else if row_request&&row_source.is_some() {
                let contribution=self.with_subgrid(child,None,row_source,reference,|this,child|this.column_contribution(child,it,reference,ItemBases::new(it,Some((cols,col_gap)),None)));
                out.push(contribution);
            } else {
                out.extend(self.column_contributions(b,std::slice::from_ref(it),cbw,Some((cols,col_gap)),rows.map(|r|(r,row_gap))));
            }
        }
        out
    }

    /// Single ordinary item, after any perpendicular inherited geometry has
    /// been installed. This shares the same sizing/automatic-minimum rules.
    fn column_contribution(&mut self,child:&mut LayoutBox,it:&GridItem,cbw:f32,bases:ItemBases)->Contribution {
        // Keep the ordinary collector and projected collector on one path.
        // Percentage margins and paddings are cyclic here: zero (§6.4).
        let rebased=if percent_edges(&child.style) {subgrid::rebase_item(it,&child.style,cbw,0.)} else {*it};let it=&rebased;
        let bb=child.style.box_sizing==BoxSizing::BorderBox;
        let extras=it.edges.pb_h+it.edges.margin_sum(Axis::Row);
        let pct=|l:Length|matches!(l,Length::Percent(_)|Length::Calc(..)|Length::Math(_));
        // A preferred aspect ratio transfers a block size definite against
        // fixed rows (Grid2 §6.6 / CSS Sizing 4 §5).
        let ratio_width=match (child.style.aspect_ratio,child.kind) {
            (Some(r),k) if k!=BoxKind::Image&&r>0.&&child.style.width.is_auto()&&pct(child.style.height)=>bases.row_basis.and_then(|basis|resolve_size(child.style.height,Some(basis),bb,it.edges.pb_v)).map(|h|{
                if bb {((h+it.edges.pb_v)*r-it.edges.pb_h).max(0.)} else {h*r}
            }),
            _=>None,
        };
        let (min,max)=match resolve_size(child.style.width,Some(cbw),bb,it.edges.pb_h) {
            Some(w) if !pct(child.style.width)=>(w,w),
            _ if ratio_width.is_some()=>{let w=ratio_width.unwrap_or(0.);(w,w)},
            // A percentage of the (still indefinite) grid area behaves as auto.
            // A compressible replaced item with a cyclic percentage width has
            // a zero min-content contribution (CSS Sizing 3 §5.2.2).
            _ if child.kind==BoxKind::Image=>{let cyclic=pct(child.style.width);let(w,_)=self.replaced_size(child,(!cyclic).then_some(cbw),bases.row_basis,it.edges.pb_h);(if cyclic {0.} else {w},w)},
            // Percentage heights inside resolve against fixed rows of the
            // area (Grid2 §12.3, contributions with the rows' sizes).
            _=>{let(min,max)=if bases.row_basis.is_some() {self.intrinsic_in(child,bases.row_basis)} else {self.intrinsic(child)};match child.style.width {Length::MinContent=>(min,min),Length::MaxContent=>(max,max),_=>(min,max)}}
        };
        let min=match child.style.min_width {
            Length::Px(m) if child.kind!=BoxKind::Image=>if bb {(m-it.edges.pb_h).max(0.).min(min)} else {m.min(min)},
            Length::Auto if child.kind!=BoxKind::Image&&crate::flex::is_scroll_container(&child.style)=>0.,
            // The automatic minimum fits an area of fixed maximum tracks.
            Length::Auto if child.style.width.is_auto()||pct(child.style.width)=>match bases.min_limit {Some(limit)=>min.min((limit-extras).max(0.)),None=>min},
            _=>min,
        };
        let auto_min=child.kind!=BoxKind::Image&&child.style.min_width.is_auto()&&(child.style.width.is_auto()||pct(child.style.width))&&ratio_width.is_none();
        Contribution{start:it.area.col.0.max(0)as usize,end:it.area.col.1.max(1)as usize,min:min+extras,max:max.max(min)+extras,auto_min,minimum:min+extras}
    }

    #[allow(clippy::too_many_arguments)]
    fn projected_rows(&mut self,b:&mut LayoutBox,items:&[GridItem],rows:&TrackList,cols:&TrackList,col_pos:&[f32],col_gap:f32,row_gap:f32,cbw:f32,cbh:Option<f32>,phase:MeasurePhase)->Vec<Contribution> {
        let _probe = self.grid_publication.probe_grid();
        let align=b.style.align_content;
        let ordinary:Vec<GridItem>=items.iter().copied().filter(|it|!subgrid::requests(&b.children[it.idx]).1).collect();
        let mut out=self.row_contributions(b,&ordinary,cols,col_pos,col_gap,cbw,rows,row_gap);
        for it in items {
            if !subgrid::requests(&b.children[it.idx]).1 {
                continue;
            }
            self.grid_work.charge(1);
            let (_,aw)=span_size(&cols.tracks,col_pos,it.area.col.0 as usize,it.area.col.1 as usize);
            let item=subgrid::rebase_item(it,&b.children[it.idx].style,cbw,aw);let it=&item;
            let child=&mut b.children[it.idx];
            self.grid_work.tracks(rows);
            if subgrid::requests(child).0 {self.grid_work.tracks(cols);}
            let columns=subgrid::requests(child).0.then(||subgrid::section(cols,col_pos,it.area.col.0 as usize,it.area.col.1 as usize,col_gap,it.column_names));
            let rows=subgrid::sizing_section_aligned(rows,it.area.row.0 as usize,it.area.row.1 as usize,row_gap,it.row_names,cbh,align);
            let rows=subgrid::item_layer_source(rows,it,&child.style,true,aw);
            let mut nested=self.with_subgrid(child,columns,Some(rows),aw,|this,child| {
                let mut item=*it;subgrid::stretch_item(child,&mut item);
                let inner_w=this.grid_item_width(child,&item,aw,cbw);
                this.subgrid_axis_contributions(child,true,Some(inner_w),phase)
            });
            for c in &mut nested {c.start+=it.area.row.0 as usize;c.end+=it.area.row.0 as usize;}
            self.grid_work.charge(nested.len());
            out.extend(nested);
        }
        out
    }

    /// Row contributions: the item's height when laid out at its column width.
    #[allow(clippy::too_many_arguments)]
    fn row_contributions(&mut self, b: &mut LayoutBox, items: &[GridItem], cols: &TrackList, col_pos: &[f32], col_gap: f32, cbw: f32, rows: &TrackList, row_gap: f32) -> Vec<Contribution> {
        let _probe = self.grid_publication.probe_grid();
        let mut out = Vec::with_capacity(items.len());
        // (contribution, row, baseline offset) of baseline-aligned items.
        let mut baselines: Vec<(usize, i32, f32)> = Vec::new();
        for it in items {
            let (_, aw) = span_size(&cols.tracks, col_pos, it.area.col.0 as usize, it.area.col.1 as usize);
            let item=if subgrid::requests(&b.children[it.idx])!=(false,false)||percent_edges(&b.children[it.idx].style) {subgrid::rebase_item(it,&b.children[it.idx].style,cbw,aw)} else {*it};let it=&item;
            let child = &mut b.children[it.idx];
            let bb = child.style.box_sizing == BoxSizing::BorderBox;
            let extras_v = it.edges.pb_v + it.edges.margin_sum(Axis::Column);
            let baseline = it.align == ItemAlign::Baseline;
            // Percentages of the block size resolve against the grid area
            // only when its rows have fixed sizes; otherwise they are cyclic.
            let cbh = if rows.sized() { fixed_span(&rows.tracks, it.area.row.0, it.area.row.1, row_gap) } else { None };
            let h = match resolve_size(child.style.height, cbh, bb, it.edges.pb_v) {
                Some(h) if !baseline => h + extras_v,
                _ => {
                    let columns=subgrid::requests(child).0.then(||subgrid::section(cols,col_pos,it.area.col.0 as usize,it.area.col.1 as usize,col_gap,true));
                    self.with_subgrid(child,columns,None,aw,|this,child| {
                        let inner_w = this.grid_item_width(child, it, aw, cbw);
                        let cb = Cb { x:0., width:aw, height:cbh };
                        let mut bfc = Bfc::new();
                        this.forced_content_height = None;
                        this.layout_block(child,&cb,0.,&mut bfc,WidthMode::Fixed(inner_w));
                        child.rect.h+it.edges.margin_sum(Axis::Column)
                    })
                }
            };
            if baseline {
                baselines.push((out.len(), it.area.row.0, Self::item_baseline(child)));
            }
            // The automatic minimum fits an area of fixed maximum tracks.
            let min = match fixed_max_span(&rows.tracks, it.area.row.0, it.area.row.1, row_gap).filter(|_| rows.sized()) {
                Some(limit) if child.style.height.is_auto() && child.style.min_height.is_auto() && !crate::flex::is_scroll_container(&child.style) => h.min(limit.max(extras_v)),
                _ => h,
            };
            // With an automatic preferred height, an explicit minimum height
            // is the minimum contribution (Grid2 §6.6), which items spanning
            // flexible tracks contribute.
            let minimum = match resolve_size(child.style.min_height, cbh, bb, it.edges.pb_v) {
                Some(m) if child.style.height.is_auto() && !child.style.min_height.is_auto() && !baseline => (m + extras_v).min(min),
                _ => min,
            };
            out.push(Contribution {
                start: it.area.row.0.max(0) as usize,
                end: it.area.row.1.max(1) as usize,
                min,
                max: h,
                auto_min: child.style.height.is_auto() && child.style.min_height.is_auto(),
                minimum,
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
                // Replaced items stretch only when asked to explicitly
                // (`normal` behaves as `start` for them, CSS Align 3 §6.1).
                if stretch && (child.kind != BoxKind::Image || it.justify == ItemAlign::Stretch) {
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
    fn layout_grid_item(&mut self, child: &mut LayoutBox, it: &mut GridItem, ax: f32, ay: f32, aw: f32, ah: f32, cbw: f32) {
        // A non-stretched item with an aspect ratio and a height that
        // resolves against its area takes its width from them.
        let ratio_w = match child.style.aspect_ratio {
            Some(r) if r > 0.0 && child.kind != BoxKind::Image && child.style.width.is_auto()
                && !matches!(it.justify, ItemAlign::Stretch | ItemAlign::Normal | ItemAlign::Auto) => {
                let bb = child.style.box_sizing == BoxSizing::BorderBox;
                resolve_size(child.style.height, Some(ah), bb, it.edges.pb_v).map(|h| {
                    let w = if bb { ((h + it.edges.pb_v) * r - it.edges.pb_h).max(0.0) } else { h * r };
                    self.clamp_width(&child.style.clone(), w, aw, it.edges.pb_h)
                })
            }
            _ => None,
        };
        let inner_w = match ratio_w { Some(w) => w, None => self.grid_item_width(child, it, aw, cbw) };
        self.layout_grid_item_with_inline(child,it,ax,ay,aw,ah,inner_w);
    }

    #[allow(clippy::too_many_arguments)]
    fn layout_grid_item_with_inline(&mut self,child:&mut LayoutBox,it:&mut GridItem,ax:f32,ay:f32,aw:f32,ah:f32,inner_w:f32) {
        // Intrinsic block sizes also lack a numeric resolution, but only an
        // automatic preferred size is eligible for stretch (§6.2).
        // `normal` behaves as `start` for an item whose aspect ratio
        // transfers a definite width (CSS Align 3 §6.1).
        let ratio_sized = child.style.aspect_ratio.is_some() && !child.style.width.is_auto() && it.align != ItemAlign::Stretch;
        let stretch_h = matches!(it.align, ItemAlign::Stretch | ItemAlign::Normal | ItemAlign::Auto)
            && !it.edges.auto.top
            && !it.edges.auto.bottom
            && child.style.height.is_auto()
            && child.kind != BoxKind::Image
            && !ratio_sized;
        let forced = if stretch_h { Some((ah - it.edges.pb_v - it.edges.margin_sum(Axis::Column)).max(0.0)) } else { None };
        let cb = Cb {
            x: 0.0,
            width: aw,
            height: Some(ah),
        };
        let mut bfc = Bfc::new();
        let stretch_replaced = |align: ItemAlign, auto_size: bool, start: bool, end: bool| align == ItemAlign::Stretch && auto_size && !start && !end;
        let replaced_w = child.kind == BoxKind::Image && stretch_replaced(it.justify, child.style.width.is_auto(), it.edges.auto.left, it.edges.auto.right);
        let replaced_h = child.kind == BoxKind::Image && stretch_replaced(it.align, child.style.height.is_auto(), it.edges.auto.top, it.edges.auto.bottom);
        if replaced_w || replaced_h {
            // A stretched replaced item: an unstretched axis follows the
            // aspect ratio.
            let h = replaced_h.then(|| self.clamp_height(&child.style.clone(), (ah - it.edges.pb_v - it.edges.margin_sum(Axis::Column)).max(0.0), Some(ah), it.edges.pb_v));
            self.layout_replaced_sized(child, &cb, replaced_w.then_some(inner_w), h, it.edges.pb_h, it.edges.pb_v);
        } else {
            self.forced_content_height = forced;
            self.layout_block(child, &cb, 0.0, &mut bfc, WidthMode::Fixed(inner_w));
        }
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

    /// Lay out a replaced item with temporary definite content sizes in the
    /// given axes (stretch alignment).
    pub(crate) fn layout_replaced_sized(&mut self, child: &mut LayoutBox, cb: &Cb, w: Option<f32>, h: Option<f32>, pb_h: f32, pb_v: f32) {
        let mut st = (*child.style).clone();
        let border_box = st.box_sizing == BoxSizing::BorderBox;
        if let Some(w) = w {
            st.width = Length::Px(w + if border_box { pb_h } else { 0.0 });
        }
        if let Some(h) = h {
            st.height = Length::Px(h + if border_box { pb_v } else { 0.0 });
        }
        let inner_w = w.unwrap_or(0.0);
        let orig = std::mem::replace(&mut child.style, std::rc::Rc::new(st));
        self.forced_content_height = None;
        let mut bfc = Bfc::new();
        self.layout_block(child, cb, 0.0, &mut bfc, WidthMode::Fixed(inner_w));
        child.style = orig;
    }

    /// Intrinsic (min-content, max-content) widths of a grid container.
    pub(crate) fn grid_intrinsic(&mut self, b: &mut LayoutBox) -> (f32, f32) {
        let _probe = self.grid_publication.probe_grid();
        if b.style.grid_lanes || subgrid::inherits_stacking(b) {
            return self.grid_lanes_intrinsic(b);
        }
        let style = b.style.clone();
        if let Some(axis)=subgrid::shared(b,false) {
            let width=axis.extent()-axis.inset_start-axis.inset_end;
            return (width.max(0.),width.max(0.));
        }
        let col_gap = subgrid::gap(b,false,0.);
        let block=subgrid::known_block_size(b,0.);
        let row_gap=subgrid::gap(b,true,0.);
        if traversal::ordinary_orthogonal_grid(b) {
            let row_gap=subgrid::gap(b,true,block.unwrap_or(0.));
            let mut widths=[0.;2];
            for (index,(constraint,kind)) in [(phases::AvailableSpace::MinContent,phases::OperationKind::IntrinsicMin),(phases::AvailableSpace::MaxContent,phases::OperationKind::IntrinsicMax)].into_iter().enumerate() {
                let (items,columns,rows)=self.place_items(b,&style,None,block,col_gap,row_gap,0.,0.);
                let spaces=[constraint,block.map(phases::AvailableSpace::Definite).unwrap_or(phases::AvailableSpace::Indefinite)];
                let inputs=traversal::ordinary_inputs(b,spaces,[col_gap,row_gap]);
                let (columns,_)=self.ordinary_grid_axis_tracks(b,&items,columns,rows,inputs,kind,0.);
                widths[index]=subgrid::positions(&columns,col_gap,None,style.justify_content).1;
            }
            return (widths[0],widths[1].max(widths[0]));
        }
        let (items, mut cols, rows) = self.place_items(b, &style, None, block, col_gap, row_gap, 0.0, 0.0);
        let contribs = self.projected_columns(b,&items,&cols,Some(&rows),col_gap,row_gap,0.,GridGeometry{inline:None,block},MeasurePhase::Initial);
        // A max-content minimum sizing function still needs the item's
        // maximum contribution under a min-content constraint.
        let mut min_tracks = cols.tracks.clone();
        size_tracks_with_space(&mut min_tracks, &contribs, GridSpace::MinContent, col_gap, false);
        let n_vis = min_tracks.iter().filter(|t| !t.collapsed).count();
        let gaps = col_gap * n_vis.saturating_sub(1) as f32;
        let min: f32 = min_tracks.iter().map(|t| t.base).sum::<f32>() + gaps;
        let max_space=subgrid::standalone_space(b,false,None);
        size_tracks_with_space(&mut cols.tracks, &contribs, max_space, col_gap, false);
        let max: f32 = cols.tracks.iter().map(|t| t.base).sum::<f32>() + gaps;
        (min, max.max(min))
    }
}
