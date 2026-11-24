//! Baseline self-alignment of grid items (CSS Grid 2 §10.6 and §11.5
//! step 1, CSS Box Alignment 3 §9).
//!
//! Grids are laid out in their own logical space (vertical containers are
//! transposed), so the column axis is always x and the row axis always y.
//! An item participates in a baseline-sharing group of the track it starts
//! in (first baseline) or ends in (last baseline). Items whose block flow
//! runs against the axis share a group with the opposite preference (Box
//! Alignment 3 §9.1): the group is then aligned at the end of the track.
//! Items without a baseline in an axis synthesize one from the line-under
//! edge of their border box.
use super::{fixed_span, span_size, GridItem, Contribution, Track, TrackList};
use crate::block::{Cb, Layouter, WidthMode};
use crate::float::Bfc;
use crate::tree::{FragmentKind, LayoutBox};
use rabbithole_gfx::Rect;
use rabbithole_style::{AlignFlags, ComputedStyle, ItemAlign, WritingMode};

/// An item's place in a baseline-sharing group of one axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ItemBaseline {
    /// The track whose group the item joins.
    track: usize,
    /// The group is aligned at the end of the track (last-baseline
    /// alignment, or first-baseline alignment against the block flow).
    end: bool,
    /// Distance from the margin-box edge the group aligns (start or end)
    /// to the item's baseline.
    ascent: f32,
}

/// Whether the item asks for last-baseline alignment in the axis.
fn last_preference(container: &ComputedStyle, child: &ComputedStyle, rows: bool) -> bool {
    let flags = if rows {
        if child.align_self == ItemAlign::Auto { container.align_items_flags } else { child.align_self_flags }
    } else if child.justify_self == ItemAlign::Auto {
        container.justify_items_flags
    } else {
        child.justify_self_flags
    };
    flags.has(AlignFlags::LAST)
}

/// Whether `it` is baseline-aligned in the axis.
pub(super) fn participates(it: &GridItem, rows: bool) -> bool {
    // Auto margins take precedence over self-alignment.
    let auto = if rows { it.edges.auto.top || it.edges.auto.bottom } else { it.edges.auto.left || it.edges.auto.right };
    (if rows { it.align } else { it.justify }) == ItemAlign::Baseline && !auto
}

/// Whether `child`, baseline-aligned in the column axis, is sized against
/// the intrinsic or flexible columns it spans (a percentage width): the
/// cyclic dependency excludes it from baseline alignment, and it uses its
/// fallback alignment (§10.6). (Rows are sized before such percentages
/// resolve against them: a percentage height takes part.)
fn cyclic(child: &LayoutBox, it: &GridItem, tracks: &[Track], rows: bool) -> bool {
    if rows {
        return false;
    }
    let size = child.style.width;
    if !matches!(size, rabbithole_style::Length::Percent(_) | rabbithole_style::Length::Calc(..) | rabbithole_style::Length::Math(_)) {
        return false;
    }
    let (a, b) = if rows { it.area.row } else { it.area.col };
    let (a, b) = (a.max(0) as usize, (b.max(0) as usize).min(tracks.len()));
    tracks.get(a..b).is_some_and(|t| t.iter().any(|t| t.min.is_intrinsic() || t.max.is_intrinsic() || t.is_flex()))
}

/// Whether `it` takes part in a baseline-sharing group in the axis.
pub(super) fn eligible(child: &LayoutBox, it: &GridItem, tracks: &[Track], rows: bool) -> bool {
    participates(it, rows) && !cyclic(child, it, tracks, rows)
}

/// The glyph box of the text on the line whose baseline is `baseline`.
fn baseline_glyphs(b: &LayoutBox, baseline: f32) -> Option<Rect> {
    for line in &b.lines {
        if (line.baseline - baseline).abs() < 0.01 {
            let text = line.fragments.iter().find(|f| matches!(f.kind, FragmentKind::Text { .. }));
            return Some(text.or(line.fragments.first()).map_or(line.rect, |f| f.rect));
        }
    }
    b.children.iter().filter(|c| !c.wm_wrapper && c.is_in_flow()).find_map(|c| baseline_glyphs(c, baseline))
}

/// The first (last) baseline of `b` in its own coordinate space. Vertical
/// text keeps its central baseline.
fn line_baseline(b: &LayoutBox, last: bool) -> Option<f32> {
    let baseline = if last { b.last_baseline() } else { b.first_baseline() }?;
    let style = &b.style;
    if !style.writing_mode.is_vertical() {
        return Some(baseline);
    }
    let Some(glyphs) = baseline_glyphs(b, baseline) else { return Some(baseline) };
    let center = glyphs.y + glyphs.h / 2.0;
    Some(if style.central_baseline() { center } else { baseline })
}

/// The baseline of a laid-out item in the axis, measured from the
/// margin-box edge its group aligns.
pub(super) fn measure(container: &ComputedStyle, child: &LayoutBox, it: &GridItem, rows: bool) -> ItemBaseline {
    let last = last_preference(container, &child.style, rows);
    let m = it.edges.margin;
    let r = child.rect;
    // (position along the axis, block flow along the axis)
    let (start, outer, pos, forward) = if rows {
        // An orthogonal item has no baseline in this axis: one is
        // synthesized from the line-under edge of the container's mode,
        // the logical top of a reflected (`vertical-lr`) container.
        let under = if container.writing_mode == WritingMode::VerticalLr { r.y } else { r.bottom() };
        // A vertical item whose block flow runs against its vertical
        // container's is laid out in the container's logical space, its
        // lines in reverse order: its first line is the last one there.
        let rightward = |m: WritingMode| matches!(m, WritingMode::VerticalLr | WritingMode::SidewaysLr);
        let reversed = !child.wm_wrapper && container.writing_mode.is_vertical() && child.style.writing_mode.is_vertical() && rightward(container.writing_mode) != rightward(child.style.writing_mode);
        let pos = if child.wm_wrapper { under } else { line_baseline(child, last != reversed).unwrap_or(under) };
        (r.y - m.top, r.h + m.top + m.bottom, pos, !reversed)
    } else {
        // Items parallel to the container synthesize their baseline from
        // the line-under edge of the orthogonal mode: the left edge in a
        // horizontal container (`vertical-lr`), the bottom (logical end)
        // in a vertical one (`horizontal-tb`).
        let under = if container.writing_mode.is_vertical() { r.right() } else { r.x };
        let (pos, forward) = match (child.wm_wrapper, child.wm_transform, child.children.first()) {
            (true, Some(mtx), Some(inner)) => {
                // The inner box is in its logical space; its block axis
                // maps onto x.
                (line_baseline(inner, last).map_or(under, |y| mtx[2] * y + mtx[4]), mtx[2] > 0.0)
            }
            _ => (under, true),
        };
        (r.x - m.left, r.w + m.left + m.right, pos, forward)
    };
    let end = last != !forward;
    let ascent = if end { start + outer - pos } else { pos - start };
    let (first, last_track) = if rows { it.area.row } else { it.area.col };
    let track = if last { (last_track - 1).max(0) } else { first.max(0) } as usize;
    ItemBaseline { track, end, ascent }
}

/// The largest ascent of each group, by (track, end).
fn group_maxima(list: &[ItemBaseline]) -> Vec<((usize, bool), f32)> {
    let mut groups: Vec<((usize, bool), f32)> = Vec::new();
    for bl in list {
        match groups.iter_mut().find(|g| g.0 == (bl.track, bl.end)) {
            Some(g) => g.1 = g.1.max(bl.ascent),
            None => groups.push(((bl.track, bl.end), bl.ascent)),
        }
    }
    groups
}

/// The shim of each item: the space before its baseline that aligns it
/// with the rest of its group (§11.5 step 1).
fn shims(list: &[ItemBaseline]) -> Vec<f32> {
    let groups = group_maxima(list);
    list.iter().map(|bl| groups.iter().find(|g| g.0 == (bl.track, bl.end)).map_or(0.0, |g| g.1 - bl.ascent)).collect()
}

/// Add the baseline shims of `items` to their contributions (one per item).
pub(super) fn add_shims(list: &[(usize, ItemBaseline)], out: &mut [Contribution]) {
    let only: Vec<ItemBaseline> = list.iter().map(|x| x.1).collect();
    for (&(k, _), shim) in list.iter().zip(shims(&only)) {
        if let Some(c) = out.get_mut(k) {
            c.min += shim;
            c.max += shim;
            c.minimum += shim;
        }
    }
}

impl<'a> Layouter<'a> {
    /// Add the baseline shims of the column-axis baseline-aligned items to
    /// their column contributions. Orthogonal items are laid out at their
    /// max-content width to find their baseline.
    pub(super) fn column_baseline_shims(&mut self, b: &mut LayoutBox, items: &[GridItem], out: &mut [Contribution], cbw: f32, cols: Option<&TrackList>, rows: Option<(&TrackList, f32)>) {
        if out.len() != items.len() || !items.iter().any(|it| participates(it, false)) || b.style.direction == rabbithole_style::Direction::Rtl {
            return;
        }
        if items.iter().any(|it| super::subgrid::requests(&b.children[it.idx]) != (false, false)) {
            return;
        }
        let _probe = self.grid_publication.probe_grid();
        let style = b.style.clone();
        let mut list = Vec::new();
        for (k, it) in items.iter().enumerate() {
            if !participates(it, false) || cols.is_some_and(|c| cyclic(&b.children[it.idx], it, &c.tracks, false)) {
                continue;
            }
            let child = &mut b.children[it.idx];
            let width = out[k].max;
            let height = rows.filter(|r| r.0.sized()).and_then(|(r, gap)| fixed_span(&r.tracks, it.area.row.0, it.area.row.1, gap));
            if child.wm_wrapper {
                let inner_w = self.grid_item_width(child, it, width, cbw);
                let cb = Cb { x: 0.0, width, height };
                let mut bfc = Bfc::new();
                self.forced_content_height = None;
                self.layout_block(child, &cb, 0.0, &mut bfc, WidthMode::Fixed(inner_w));
                let mut bl = measure(&style, child, it, false);
                // The contribution may round the laid-out box differently.
                if bl.end {
                    bl.ascent += width - (child.rect.w + it.edges.margin_sum(crate::flex::Axis::Row));
                }
                list.push((k, bl));
            } else {
                // A synthesized baseline needs no layout: the start border
                // edge, or the end one in a vertical container.
                let m = it.edges.margin;
                let ascent = if style.writing_mode.is_vertical() { width - m.right } else { m.left };
                let last = last_preference(&style, &child.style, false);
                let track = if last { (it.area.col.1 - 1).max(0) } else { it.area.col.0.max(0) } as usize;
                list.push((k, ItemBaseline { track, end: last, ascent: if last { width - ascent } else { ascent } }));
            }
        }
        add_shims(&list, out);
    }

    /// Align the laid-out baseline-aligned items of `b` with their groups
    /// in both axes (§10.6): first-baseline groups at the start of their
    /// track, last-baseline groups at its end.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn align_item_baselines(&mut self, b: &mut LayoutBox, items: &[GridItem], cols: (&[Track], &[f32]), rows: (&[Track], &[f32]), origin: (f32, f32), mirror: bool, axes: &[bool]) {
        let style = b.style.clone();
        for &axis_rows in axes {
            if !axis_rows && (mirror || style.direction == rabbithole_style::Direction::Rtl) {
                continue;
            }
            let tracks = if axis_rows { rows.0 } else { cols.0 };
            let members: Vec<&GridItem> = items.iter().filter(|it| eligible(&b.children[it.idx], it, tracks, axis_rows) && b.children[it.idx].flow != crate::tree::Flow::Absolute).collect();
            if members.is_empty() {
                continue;
            }
            let list: Vec<ItemBaseline> = members.iter().map(|it| measure(&style, &b.children[it.idx], it, axis_rows)).collect();
            let shim = shims(&list);
            for ((it, bl), shim) in members.iter().zip(&list).zip(shim) {
                let (tracks, pos, span, o) = if axis_rows { (rows.0, rows.1, it.area.row, origin.1) } else { (cols.0, cols.1, it.area.col, origin.0) };
                let (a, size) = span_size(tracks, pos, span.0.max(0) as usize, span.1.max(0) as usize);
                let child = &mut b.children[it.idx];
                let m = it.edges.margin;
                let (cur, outer) = if axis_rows { (child.rect.y - m.top, child.rect.h + m.top + m.bottom) } else { (child.rect.x - m.left, child.rect.w + m.left + m.right) };
                let target = if bl.end { o + a + size - shim - outer } else { o + a + shim };
                let d = target - cur;
                if axis_rows { child.translate(0.0, d) } else { child.translate(d, 0.0) }
            }
        }
    }
}

/// The items whose baselines are the grid container's first and last
/// baselines (§10.7): an item of the first (last) row aligned by its
/// first (last) baseline, else the first (last) item of that row in grid
/// order. (Indices into the container's children.)
pub(super) fn container_baselines(container: &ComputedStyle, items: &[GridItem], children: &[LayoutBox]) -> [u32; 2] {
    let none = crate::tree::FLEX_BASELINE_NONE;
    let in_flow: Vec<&GridItem> = items.iter().filter(|it| children[it.idx].is_in_flow()).collect();
    let (Some(first_row), Some(last_row)) = (in_flow.iter().map(|it| it.area.row.0).min(), in_flow.iter().map(|it| it.area.row.1).max()) else {
        return [none, none];
    };
    let pick = |last: bool| -> u32 {
        let row: Vec<&&GridItem> = in_flow.iter().filter(|it| if last { it.area.row.1 == last_row } else { it.area.row.0 == first_row }).collect();
        let aligned = row.iter().find(|it| participates(it, true) && last_preference(container, &children[it.idx].style, true) == last);
        let ordered = if last {
            row.iter().max_by_key(|it| (it.area.row.0, it.area.col.0, it.idx))
        } else {
            row.iter().min_by_key(|it| (it.area.row.0, it.area.col.0, it.idx))
        };
        aligned.or(ordered).map_or(none, |it| it.idx as u32)
    };
    [pick(false), pick(true)]
}
