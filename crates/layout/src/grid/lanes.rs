//! Grid lanes layout (CSS Grid Layout Level 3, `display: grid-lanes`,
//! <https://drafts.csswg.org/css-grid-3/>): tracks exist in one axis only
//! (the grid axis); items flow into the track ("lane") whose running
//! position is smallest, within the `fit-tolerance` (§4.4), and stack in
//! the other (stacking) axis.
//!
//! Column lanes (the default) take their tracks from
//! `grid-template-columns` and stack items downwards; row lanes
//! (`grid-lanes-direction: row`, or only `grid-template-rows` given) take
//! them from `grid-template-rows` and stack items in the inline direction.

use super::*;

/// An in-flow item of a grid lanes container.
struct LaneItem {
    idx: usize,
    /// Number of tracks spanned.
    span: usize,
    /// Start track of a definite grid-axis placement.
    fixed: Option<usize>,
    edges: ItemEdges,
    justify: ItemAlign,
    align: ItemAlign,
}

/// Is `b` laid out in row lanes (block axis = grid axis)?
fn row_lanes(style: &ComputedStyle) -> bool {
    match style.grid_lanes_direction.rows {
        Some(r) => r,
        None => style.grid_template_columns.is_empty() && !style.grid_template_rows.is_empty(),
    }
}

/// The tie threshold of the placement algorithm (`fit-tolerance`; `normal`
/// is 1em), resolved against the grid-axis content size.
fn tolerance(style: &ComputedStyle, grid_size: Option<f32>) -> f32 {
    match style.fit_tolerance {
        None => style.font_size,
        Some(Length::Px(v)) => v.max(0.0),
        Some(l) => l.resolve_or_zero(grid_size.unwrap_or(0.0)).max(0.0),
    }
}

struct Placement {
    /// Start track per item.
    start: Vec<usize>,
    /// Offset of each item's margin box in the stacking axis.
    pos: Vec<f32>,
    /// End of the stacking range (0 without items).
    range: f32,
}

/// §4.4 placement over `n` tracks: `size(i, start)` lays item `i` out in
/// its tracks and returns its outer size in the stacking axis.
#[allow(clippy::too_many_arguments)]
fn place(items: &[LaneItem], tracks: &[Track], tol: f32, gap: f32, reverse: bool, dense: bool, mut size: impl FnMut(usize, usize) -> f32) -> Placement {
    let n = tracks.len();
    let mut running = vec![0.0f32; n];
    let mut used = vec![false; n];
    // Occupied [start, end) stacking intervals per track (dense packing).
    let mut taken: Vec<Vec<(f32, f32)>> = vec![Vec::new(); n];
    let mut cursor = 0usize;
    let mut pos = Vec::with_capacity(items.len());
    let mut item_starts = Vec::with_capacity(items.len());
    for (i, it) in items.iter().enumerate() {
        let span = it.span.min(n).max(1);
        let saved_cursor = cursor;
        let s = match it.fixed {
            Some(s) => s.min(n - span),
            None => {
                // Candidate start lines in placement order.
                let lines: Vec<usize> = if reverse { (0..=n - span).rev().collect() } else { (0..=n - span).collect() };
                let open = |l: usize| !tracks[l..l + span].iter().any(|t| t.collapsed);
                let max_pos = |l: usize| running[l..l + span].iter().cloned().fold(0.0f32, f32::max);
                let min = lines.iter().filter(|&&l| open(l)).map(|&l| max_pos(l)).fold(f32::INFINITY, f32::min);
                let possible: Vec<usize> = (0..lines.len()).filter(|&k| open(lines[k]) && max_pos(lines[k]) <= min + tol).collect();
                let k = possible.iter().copied().find(|&k| k >= cursor).or_else(|| possible.first().copied()).unwrap_or(0);
                cursor = k + span;
                lines[k]
            }
        };
        let p = running[s..s + span].iter().cloned().fold(0.0f32, f32::max);
        let sz = size(i, s).max(0.0);
        // §4.4 step 4: dense packing backfills an earlier skipped space of
        // the same total track size.
        if dense {
            let width = |l: usize| tracks[l..l + span].iter().map(|t| t.base).sum::<f32>();
            let w0 = width(s);
            let starts: Vec<usize> = match it.fixed {
                Some(_) => vec![s],
                None => (0..=n - span).filter(|&l| !tracks[l..l + span].iter().any(|t| t.collapsed)).collect(),
            };
            let mut fits_at: Vec<(f32, usize)> = Vec::new();
            for &l in &starts {
                if (width(l) - w0).abs() > 0.01 {
                    continue;
                }
                let mut cands: Vec<f32> = vec![0.0];
                for t in l..l + span {
                    cands.extend(taken[t].iter().map(|&(_, e)| e + gap));
                }
                cands.sort_by(|a, b| a.total_cmp(b));
                for y in cands {
                    let fits = (l..l + span).all(|t| y + sz + gap <= running[t] + 0.01 && taken[t].iter().all(|&(a, e)| y + 0.01 >= e + gap || y + sz + gap <= a + 0.01));
                    if fits {
                        fits_at.push((y, l));
                        break;
                    }
                }
            }
            // The highest space; among those within the tolerance of it,
            // the start-most.
            let top = fits_at.iter().map(|f| f.0).fold(f32::INFINITY, f32::min);
            let best = fits_at.iter().filter(|f| f.0 <= top + tol).min_by_key(|f| f.1).copied();
            if let Some((y, l)) = best {
                if y + 0.01 < p {
                    if l != s {
                        size(i, l);
                    }
                    for t in l..l + span {
                        taken[t].push((y, y + sz));
                    }
                    cursor = saved_cursor;
                    pos.push(y);
                    item_starts.push(l);
                    continue;
                }
            }
        }
        for t in s..s + span {
            taken[t].push((p, p + sz));
            running[t] = p + sz + gap;
            used[t] = true;
        }
        pos.push(p);
        item_starts.push(s);
    }
    let range = (0..n).filter(|&t| used[t]).map(|t| running[t] - gap).fold(0.0f32, f32::max);
    Placement { start: item_starts, pos, range }
}

/// The end of each item's stacking-axis alignment container (§6.4): the
/// next item in any of its tracks (less the gap), or the end of the
/// stacking range for the last items.
fn stacking_limits(items: &[LaneItem], pl: &Placement, gap: f32) -> Vec<f32> {
    (0..items.len())
        .map(|i| {
            let (s, e) = (pl.start[i], pl.start[i] + items[i].span);
            let mut limit = pl.range;
            for j in 0..items.len() {
                let (sj, ej) = (pl.start[j], pl.start[j] + items[j].span);
                if j != i && sj < e && s < ej && pl.pos[j] > pl.pos[i] + 0.01 {
                    limit = limit.min(pl.pos[j] - gap);
                }
            }
            limit
        })
        .collect()
}

impl<'a> Layouter<'a> {
    /// Self-alignment of an item in the stacking axis within `free` space
    /// after it: stretch (re-laid out larger), center or end; the offset.
    fn lane_stack_align(&mut self, child: &mut LayoutBox, it: &LaneItem, align: ItemAlign, free: f32, cbw: f32, vertical: bool, cb_h: Option<f32>) -> f32 {
        if free <= 0.01 {
            return 0.0;
        }
        match align {
            ItemAlign::Stretch => {
                let bb = child.style.box_sizing == BoxSizing::BorderBox;
                let (size, pb, autos) = if vertical {
                    (child.style.height, it.edges.pb_v, it.edges.auto.top || it.edges.auto.bottom)
                } else {
                    (child.style.width, it.edges.pb_h, it.edges.auto.left || it.edges.auto.right)
                };
                if autos || child.kind == BoxKind::Image || resolve_size(size, if vertical { cb_h } else { Some(cbw) }, bb, pb).is_some() {
                    return 0.0;
                }
                let inner_w = (child.rect.w - it.edges.pb_h).max(0.0);
                let inner_h = (child.rect.h - it.edges.pb_v).max(0.0);
                let lcb = Cb { x: 0.0, width: if vertical { inner_w + it.edges.pb_h + it.edges.margin_sum(Axis::Row) } else { cbw }, height: cb_h };
                let mut bfc = Bfc::new();
                let margin = child.margin;
                if vertical {
                    self.forced_content_height = Some(inner_h + free);
                    self.layout_block(child, &lcb, 0.0, &mut bfc, WidthMode::Fixed(inner_w));
                } else {
                    self.forced_content_height = Some(inner_h);
                    self.layout_block(child, &lcb, 0.0, &mut bfc, WidthMode::Fixed(inner_w + free));
                }
                self.forced_content_height = None;
                child.margin = margin;
                0.0
            }
            ItemAlign::Center => free / 2.0,
            ItemAlign::FlexEnd => free,
            _ => 0.0,
        }
    }

    /// Build the grid-axis tracks and collect the items.
    fn lanes_setup(&mut self, b: &mut LayoutBox, style: &ComputedStyle, rows: bool, avail: Option<f32>, gap: f32, cbw: f32, origin: (f32, f32)) -> (TrackList, Vec<LaneItem>) {
        let (list, auto_list) = if rows { (&style.grid_template_rows, &style.grid_auto_rows) } else { (&style.grid_template_columns, &style.grid_auto_columns) };
        let mut tl = expand_tracks(list, avail, gap);
        let n_lines = tl.tracks.len() as i32 + 1;
        let mut order: Vec<(i32, usize)> = Vec::new();
        for (i, c) in b.children.iter_mut().enumerate() {
            match c.flow {
                Flow::Absolute => c.static_pos = origin,
                _ => order.push((c.style.order, i)),
            }
        }
        order.sort_by_key(|&(o, i)| (o, i));
        let mut placements: Vec<(usize, Result<(i32, i32), i32>)> = Vec::new();
        for &(_, idx) in &order {
            let s = &b.children[idx].style;
            let (gs, ge) = if rows { (&s.grid_row_start, &s.grid_row_end) } else { (&s.grid_column_start, &s.grid_column_end) };
            let p = resolve_pair(line_spec(gs, &tl.names, n_lines, false), line_spec(ge, &tl.names, n_lines, true));
            placements.push((idx, p));
        }
        // Leading implicit tracks for lines before the explicit grid.
        let min_line = placements.iter().filter_map(|(_, p)| p.ok().map(|(s, _)| s)).min().unwrap_or(0).min(0);
        for _ in 0..-min_line {
            tl.tracks.insert(0, Track::new(Sizing::Auto, Sizing::Auto));
            tl.names.insert(0, Vec::new());
        }
        let mut max_end = tl.tracks.len() as i32;
        for (_, p) in placements.iter_mut() {
            match p {
                Ok((s, e)) => {
                    *s -= min_line;
                    *e -= min_line;
                    max_end = max_end.max(*e);
                }
                Err(k) => max_end = max_end.max(*k),
            }
        }
        if !placements.is_empty() {
            max_end = max_end.max(1);
        }
        let mut ai = 0usize;
        while (tl.tracks.len() as i32) < max_end {
            let ts = auto_list.get(ai % auto_list.len().max(1)).cloned().unwrap_or(TrackSize::Length(Length::Auto));
            let (min, max) = sizing_of(&ts, avail);
            tl.tracks.push(Track::new(min, max));
            tl.names.push(Vec::new());
            ai += 1;
        }
        // repeat(auto-fit): explicitly occupied tracks stay, then as many
        // free tracks as the auto-placed items span (§3.3.1).
        if tl.tracks.iter().any(|t| t.auto_fit) {
            let mut occupied = vec![false; tl.tracks.len()];
            let mut auto_spans = 0usize;
            for (_, p) in &placements {
                match p {
                    Ok((s, e)) => (*s..*e).for_each(|t| occupied[t as usize] = true),
                    Err(k) => auto_spans += *k as usize,
                }
            }
            for (t, track) in tl.tracks.iter_mut().enumerate() {
                if occupied[t] || !track.auto_fit {
                    continue;
                }
                if auto_spans > 0 {
                    auto_spans -= 1;
                } else {
                    track.collapsed = true;
                }
            }
        }
        let items = placements
            .into_iter()
            .map(|(idx, p)| {
                let cs = b.children[idx].style.clone();
                let (span, fixed) = match p {
                    Ok((s, e)) => ((e - s).max(1) as usize, Some(s as usize)),
                    Err(k) => (k.max(1) as usize, None),
                };
                LaneItem {
                    idx,
                    span,
                    fixed,
                    edges: ItemEdges::new(&cs, cbw),
                    justify: if cs.justify_self == ItemAlign::Auto { style.justify_items } else { cs.justify_self },
                    align: if cs.align_self == ItemAlign::Auto { style.align_items } else { cs.align_self },
                }
            })
            .collect();
        (tl, items)
    }

    /// Every position an item may take in the grid axis (§3.4): its
    /// definite placement, or each start line for an auto-placed item.
    fn lane_positions(it: &LaneItem, tracks: &[Track]) -> Vec<usize> {
        let n = tracks.len();
        let span = it.span.min(n).max(1);
        match it.fixed {
            Some(s) => vec![s.min(n - span)],
            None => (0..=n - span).filter(|&l| !tracks[l..l + span].iter().any(|t| t.collapsed)).collect(),
        }
    }

    fn lane_grid_item(it: &LaneItem, start: usize, rows: bool) -> GridItem {
        let span = (start as i32, (start + it.span) as i32);
        GridItem {
            idx: it.idx,
            area: if rows { Area { row: span, col: (0, 1) } } else { Area { row: (0, 1), col: span } },
            edges: it.edges,
            justify: it.justify,
            align: it.align,
        }
    }

    /// Column-lane track sizing: every item contributes to every position
    /// it could take.
    fn size_column_lanes(&mut self, b: &mut LayoutBox, tl: &mut TrackList, items: &[LaneItem], avail: Option<f32>, gap: f32, cbw: f32, stretch: bool, min_only: bool) {
        let mut virt = Vec::new();
        for it in items {
            for s in Self::lane_positions(it, &tl.tracks) {
                virt.push(Self::lane_grid_item(it, s, false));
            }
        }
        let mut contribs = self.column_contributions(b, &virt, cbw);
        if min_only {
            contribs.iter_mut().for_each(|c| c.max = c.min);
        }
        size_tracks(&mut tl.tracks, &contribs, avail, gap, stretch);
    }

    /// Inner (content-box) width of an item in the stacking axis of row
    /// lanes: definite, or fit-content within `avail` (`None` = the
    /// max-content size, `Some(0)` = min-content).
    fn lane_item_width(&mut self, child: &mut LayoutBox, it: &LaneItem, cbw: f32, avail: Option<f32>) -> f32 {
        let bb = child.style.box_sizing == BoxSizing::BorderBox;
        let style = child.style.clone();
        match resolve_size(style.width, Some(cbw), bb, it.edges.pb_h) {
            Some(w) => self.clamp_width(&style, w, cbw, it.edges.pb_h),
            None if child.kind == BoxKind::Image => self.replaced_size(child, Some(cbw), None, it.edges.pb_h).0,
            None => {
                let (min, max) = self.intrinsic(child);
                let room = avail.map(|a| (a - it.edges.pb_h - it.edges.margin_sum(Axis::Row)).max(0.0)).unwrap_or(f32::INFINITY);
                self.clamp_width(&style, crate::block::shrink_to_fit(min, max, room), cbw, it.edges.pb_h)
            }
        }
    }

    /// Outer height of a row-lane item laid out `inner_w` wide.
    fn lane_item_height(&mut self, child: &mut LayoutBox, it: &LaneItem, inner_w: f32, cbw: f32) -> f32 {
        let bb = child.style.box_sizing == BoxSizing::BorderBox;
        let mv = it.edges.margin_sum(Axis::Column);
        if let Some(h) = resolve_size(child.style.height, None, bb, it.edges.pb_v) {
            return h + it.edges.pb_v + mv;
        }
        let cb = Cb { x: 0.0, width: cbw, height: None };
        let mut bfc = Bfc::new();
        self.forced_content_height = None;
        self.layout_block(child, &cb, 0.0, &mut bfc, WidthMode::Fixed(inner_w));
        child.rect.h + mv
    }

    /// Row-lane track sizing; the items are `widths[i]` wide.
    fn size_row_lanes(&mut self, b: &mut LayoutBox, tl: &mut TrackList, items: &[LaneItem], widths: &[f32], avail: Option<f32>, gap: f32, cbw: f32, stretch: bool) {
        let mut contribs = Vec::new();
        for (it, &w) in items.iter().zip(widths) {
            let h = self.lane_item_height(&mut b.children[it.idx], it, w, cbw);
            for s in Self::lane_positions(it, &tl.tracks) {
                contribs.push(Contribution { start: s, end: s + it.span.min(tl.tracks.len()), min: h, max: h });
            }
        }
        size_tracks(&mut tl.tracks, &contribs, avail, gap, stretch);
    }

    /// Lay out a grid lanes container's items; returns the content height.
    pub(crate) fn layout_grid_lanes(&mut self, b: &mut LayoutBox, cb: &Cb) -> f32 {
        let style = b.style.clone();
        let rows = row_lanes(&style);
        let cbw = cb.width;
        let (cx, cy) = (b.content_rect.x, b.content_rect.y);
        let col_gap = style.column_gap.resolve_or_zero(cbw);
        let row_gap = style.row_gap.resolve_or_zero(cb.height.unwrap_or(0.0));
        let dir = style.grid_lanes_direction;
        let dense = style.grid_lanes_dense || matches!(style.grid_auto_flow, GridAutoFlow::RowDense | GridAutoFlow::ColumnDense);
        if !rows {
            let avail = Some(cbw);
            let (mut tl, items) = self.lanes_setup(b, &style, false, avail, col_gap, cbw, (cx, cy));
            let stretch = matches!(style.justify_content, ContentDistribution::Stretch | ContentDistribution::FlexStart);
            self.size_column_lanes(b, &mut tl, &items, avail, col_gap, cbw, stretch, false);
            let (col_pos, _) = track_positions(&tl.tracks, col_gap, avail, style.justify_content);
            let tol = tolerance(&style, avail);
            let tracks = tl.tracks.clone();
            let mut placed: Vec<(f32, f32)> = vec![(0.0, 0.0); items.len()];
            let pl = place(&items, &tl.tracks, tol, row_gap, dir.track_reverse, dense, |i, s| {
                let it = &items[i];
                let gi = Self::lane_grid_item(it, s, false);
                let (ax, aw) = span_size(&tracks, &col_pos, s, s + it.span);
                let child = &mut b.children[it.idx];
                let inner_w = self.grid_item_width(child, &gi, aw, cbw);
                let lcb = Cb { x: 0.0, width: aw, height: cb.height };
                let mut bfc = Bfc::new();
                self.forced_content_height = None;
                self.layout_block(child, &lcb, 0.0, &mut bfc, WidthMode::Fixed(inner_w));
                let mut margin = it.edges.margin;
                let free = aw - (child.rect.w + margin.left + margin.right);
                let dx = align_offset(it.justify, it.edges.auto.left, it.edges.auto.right, free, &mut margin.left, &mut margin.right);
                child.margin = margin;
                placed[i] = (ax + dx + it.edges.margin.left, child.rect.h + margin.top + margin.bottom);
                placed[i].1
            });
            let extent = cb.height.unwrap_or(pl.range);
            let shift = stacking_shift(style.align_content, extent - pl.range);
            let limits = stacking_limits(&items, &pl, row_gap);
            for (i, it) in items.iter().enumerate() {
                let (x, mut outer) = placed[i];
                let free = limits[i] - pl.pos[i] - outer;
                let off = self.lane_stack_align(&mut b.children[it.idx], it, it.align, free, cbw, true, cb.height);
                if it.align == ItemAlign::Stretch && free > 0.01 {
                    outer = b.children[it.idx].rect.h + b.children[it.idx].margin.top + b.children[it.idx].margin.bottom;
                }
                let mut y = pl.pos[i] + shift + off;
                if dir.fill_reverse {
                    y = extent - y - outer;
                }
                let child = &mut b.children[it.idx];
                let (nx, ny) = (cx + x, cy + y + child.margin.top);
                child.translate(nx - child.rect.x, ny - child.rect.y);
            }
            return pl.range;
        }

        // Row lanes: tracks are rows; items stack in the inline axis.
        let avail = cb.height;
        let (mut tl, items) = self.lanes_setup(b, &style, true, avail, row_gap, cbw, (cx, cy));
        let widths: Vec<f32> = items.iter().map(|it| self.lane_item_width(&mut b.children[it.idx], it, cbw, Some(cbw))).collect();
        let stretch = matches!(style.align_content, ContentDistribution::Stretch | ContentDistribution::FlexStart);
        self.size_row_lanes(b, &mut tl, &items, &widths, avail, row_gap, cbw, stretch);
        let (row_pos, rows_extent) = track_positions(&tl.tracks, row_gap, avail, style.align_content);
        let tol = tolerance(&style, avail);
        let tracks = tl.tracks.clone();
        let mut placed: Vec<(f32, f32)> = vec![(0.0, 0.0); items.len()];
        let pl = place(&items, &tl.tracks, tol, col_gap, dir.track_reverse, dense, |i, s| {
            let it = &items[i];
            let (ay, ah) = span_size(&tracks, &row_pos, s, s + it.span);
            let child = &mut b.children[it.idx];
            let bb = child.style.box_sizing == BoxSizing::BorderBox;
            let mut margin = it.edges.margin;
            let stretch = matches!(it.align, ItemAlign::Stretch | ItemAlign::Normal | ItemAlign::Auto)
                && !it.edges.auto.top
                && !it.edges.auto.bottom
                && resolve_size(child.style.height, Some(ah), bb, it.edges.pb_v).is_none()
                && child.kind != BoxKind::Image;
            let lcb = Cb { x: 0.0, width: cbw, height: Some(ah) };
            let mut bfc = Bfc::new();
            self.forced_content_height = if stretch { Some((ah - it.edges.pb_v - it.edges.margin_sum(Axis::Column)).max(0.0)) } else { None };
            self.layout_block(child, &lcb, 0.0, &mut bfc, WidthMode::Fixed(widths[i]));
            self.forced_content_height = None;
            let free = ah - (child.rect.h + margin.top + margin.bottom);
            let dy = align_offset(it.align, it.edges.auto.top, it.edges.auto.bottom, free, &mut margin.top, &mut margin.bottom);
            child.margin = margin;
            placed[i] = (ay + dy + it.edges.margin.top, child.rect.w + margin.left + margin.right);
            placed[i].1
        });
        let extent = cbw;
        let rtl = style.direction == rabbithole_style::Direction::Rtl;
        let shift = stacking_shift(style.justify_content, extent - pl.range);
        let limits = stacking_limits(&items, &pl, col_gap);
        for (i, it) in items.iter().enumerate() {
            let (y, mut outer) = placed[i];
            let free = limits[i] - pl.pos[i] - outer;
            let ah = b.children[it.idx].rect.h;
            let off = self.lane_stack_align(&mut b.children[it.idx], it, it.justify, free, cbw, false, Some(ah));
            if it.justify == ItemAlign::Stretch && free > 0.01 {
                outer = b.children[it.idx].rect.w + b.children[it.idx].margin.left + b.children[it.idx].margin.right;
            }
            let mut x = pl.pos[i] + shift + off;
            if dir.fill_reverse != rtl {
                x = extent - x - outer;
            }
            let child = &mut b.children[it.idx];
            let (nx, ny) = (cx + x + child.margin.left, cy + y);
            child.translate(nx - child.rect.x, ny - child.rect.y);
        }
        rows_extent.max(0.0)
    }

    /// Intrinsic (min-content, max-content) widths of a grid lanes container.
    pub(crate) fn grid_lanes_intrinsic(&mut self, b: &mut LayoutBox) -> (f32, f32) {
        let style = b.style.clone();
        let col_gap = crate::flex::px_or_zero(style.column_gap);
        if !row_lanes(&style) {
            let (tl, items) = self.lanes_setup(b, &style, false, None, col_gap, 0.0, (0.0, 0.0));
            let mut min_t = TrackList { tracks: tl.tracks.clone(), names: Vec::new() };
            self.size_column_lanes(b, &mut min_t, &items, None, col_gap, 0.0, false, true);
            let mut max_t = tl;
            self.size_column_lanes(b, &mut max_t, &items, None, col_gap, 0.0, false, false);
            let sum = |t: &[Track]| t.iter().filter(|t| !t.collapsed).map(|t| t.base).sum::<f32>() + col_gap * t.iter().filter(|t| !t.collapsed).count().saturating_sub(1) as f32;
            let (min, max) = (sum(&min_t.tracks), sum(&max_t.tracks));
            return (min, max.max(min));
        }
        // Row lanes: the stacking range with min-/max-content item widths.
        let row_gap = crate::flex::px_or_zero(style.row_gap);
        let fixed_h = match style.height {
            Length::Px(h) => Some(h),
            _ => None,
        };
        let mut out = [0.0f32; 2];
        for (k, avail_w) in [Some(0.0), None].into_iter().enumerate() {
            let (mut tl, items) = self.lanes_setup(b, &style, true, fixed_h, row_gap, 0.0, (0.0, 0.0));
            let widths: Vec<f32> = items.iter().map(|it| self.lane_item_width(&mut b.children[it.idx], it, 0.0, avail_w)).collect();
            self.size_row_lanes(b, &mut tl, &items, &widths, fixed_h, row_gap, 0.0, false);
            let tol = tolerance(&style, fixed_h);
            let pl = place(&items, &tl.tracks, tol, col_gap, style.grid_lanes_direction.track_reverse, false, |i, _| widths[i] + items[i].edges.pb_h + items[i].edges.margin_sum(Axis::Row));
            out[k] = pl.range;
        }
        (out[0], out[1].max(out[0]))
    }
}

/// Offset of an item in its area along one axis (auto margins first, then
/// the self-alignment value); adjusts the auto margins.
fn align_offset(align: ItemAlign, auto_start: bool, auto_end: bool, free: f32, m_start: &mut f32, m_end: &mut f32) -> f32 {
    if (auto_start || auto_end) && free > 0.0 {
        if auto_start && auto_end {
            *m_start += free / 2.0;
            *m_end += free / 2.0;
            free / 2.0
        } else if auto_start {
            *m_start += free;
            free
        } else {
            *m_end += free;
            0.0
        }
    } else {
        match align {
            ItemAlign::FlexEnd => free,
            ItemAlign::Center => free / 2.0,
            _ => 0.0,
        }
    }
}

/// Content distribution of the stacking range (§6.3): only start, center
/// and end differ.
fn stacking_shift(d: ContentDistribution, free: f32) -> f32 {
    match d {
        ContentDistribution::FlexEnd => free,
        ContentDistribution::Center | ContentDistribution::SpaceAround | ContentDistribution::SpaceEvenly => free / 2.0,
        _ => 0.0,
    }
}
