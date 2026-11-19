//! Flexbox layout per CSS Flexible Box Layout Module Level 1
//! (<https://www.w3.org/TR/css-flexbox-1/>): §9 "Flex Layout Algorithm".
//!
//! Items are the container's in-flow children (text is already wrapped in
//! anonymous blocks by the builder). Each item is laid out with the normal
//! block machinery (`layout_block`) given a fixed main size, measured, and
//! translated into its final position.

use crate::block::{resolve_edges, Cb, Layouter, WidthMode};
use crate::float::Bfc;
use crate::tree::{BoxKind, Flow, LayoutBox};
use rabbithole_style::{AlignMods, AlignPos, BoxSizing, ComputedStyle, ContentDistribution, Direction, Edges, FlexDirection, FlexWrap, ItemAlign, Length, Overflow};

/// Main axis direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Axis {
    Row,
    Column,
}

/// Resolved margins / padding / border of a flex or grid item.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ItemEdges {
    pub margin: Edges<f32>,
    /// Which margins were `auto`.
    pub auto: Edges<bool>,
    pub pb_h: f32,
    pub pb_v: f32,
}

impl ItemEdges {
    pub fn new(style: &ComputedStyle, cbw: f32) -> ItemEdges {
        let pad = resolve_edges(&style.padding, cbw);
        let bw = style.border_width;
        let m = |l: Length| l.resolve(cbw);
        let (ml, mr, mt, mb) = (m(style.margin.left), m(style.margin.right), m(style.margin.top), m(style.margin.bottom));
        ItemEdges {
            margin: Edges {
                top: mt.unwrap_or(0.0),
                right: mr.unwrap_or(0.0),
                bottom: mb.unwrap_or(0.0),
                left: ml.unwrap_or(0.0),
            },
            auto: Edges {
                top: mt.is_none(),
                right: mr.is_none(),
                bottom: mb.is_none(),
                left: ml.is_none(),
            },
            pb_h: pad.left + pad.right + bw.left + bw.right,
            pb_v: pad.top + pad.bottom + bw.top + bw.bottom,
        }
    }

    pub fn pb(&self, axis: Axis) -> f32 {
        match axis {
            Axis::Row => self.pb_h,
            Axis::Column => self.pb_v,
        }
    }
    /// (start, end) margins along `axis`.
    pub fn margins(&self, axis: Axis) -> (f32, f32) {
        match axis {
            Axis::Row => (self.margin.left, self.margin.right),
            Axis::Column => (self.margin.top, self.margin.bottom),
        }
    }
    pub fn autos(&self, axis: Axis) -> (bool, bool) {
        match axis {
            Axis::Row => (self.auto.left, self.auto.right),
            Axis::Column => (self.auto.top, self.auto.bottom),
        }
    }
    pub fn margin_sum(&self, axis: Axis) -> f32 {
        let (a, b) = self.margins(axis);
        a + b
    }
}

/// Resolve a size property (`width`/`height`/`min-*`/`max-*`) to an inner
/// (content-box) size in px. `avail` is the containing block size in that
/// axis (None = indefinite: percentages behave as `auto`).
pub(crate) fn resolve_size(l: Length, avail: Option<f32>, border_box: bool, pb: f32) -> Option<f32> {
    let v = match l {
        Length::Px(v) => v,
        Length::Percent(p) => avail? * p / 100.0,
        Length::Calc(..) | Length::Math(_) => l.resolve(avail?)?,
        _ => return None,
    };
    Some(if border_box { (v - pb).max(0.0) } else { v.max(0.0) })
}

/// Distribute free space between `n` items according to a content
/// distribution keyword. Returns (leading offset, spacing between items).
pub(crate) fn distribute(kind: ContentDistribution, free: f32, n: usize) -> (f32, f32) {
    if n == 0 {
        return (0.0, 0.0);
    }
    let nf = n as f32;
    match kind {
        ContentDistribution::FlexStart | ContentDistribution::Stretch => (0.0, 0.0),
        ContentDistribution::FlexEnd => (free, 0.0),
        ContentDistribution::Center => (free / 2.0, 0.0),
        ContentDistribution::SpaceBetween => {
            if n == 1 || free < 0.0 {
                (0.0, 0.0)
            } else {
                (0.0, free / (nf - 1.0))
            }
        }
        ContentDistribution::SpaceAround => {
            if free < 0.0 {
                (free / 2.0, 0.0)
            } else {
                let s = free / nf;
                (s / 2.0, s)
            }
        }
        ContentDistribution::SpaceEvenly => {
            if free < 0.0 {
                (free / 2.0, 0.0)
            } else {
                let s = free / (nf + 1.0);
                (s, s)
            }
        }
    }
}

/// Per-container working storage of [`Layouter::layout_flex`], pooled on
/// the layouter.
#[derive(Default)]
pub(crate) struct FlexScratch {
    order: Vec<(i32, usize)>,
    items: Vec<FlexItem>,
    lines: Vec<std::ops::Range<usize>>,
    ids: Vec<usize>,
    metrics: Vec<[f32; 3]>,
}

struct FlexItem {
    idx: usize,
    edges: ItemEdges,
    grow: f32,
    shrink: f32,
    /// Flex base size (inner, content-box).
    base: f32,
    /// Hypothetical main size (base clamped by min/max).
    hyp: f32,
    min: f32,
    max: f32,
    /// Used main size (inner).
    main: f32,
    /// Hypothetical cross size (inner) after laying out with the main size.
    cross_hyp: f32,
    /// The used main size is a definite base for the contents'
    /// percentages (§9.8: the container's main size is definite, or the
    /// item's flex basis is).
    main_definite: bool,
    /// Whether the cross size property is `auto` (stretchable).
    cross_auto: bool,
    /// The item is stretched to its line (stretch alignment, auto cross
    /// size, no auto cross margins).
    stretches: bool,
    /// The measuring layout was skipped: lay out in the stretch step.
    must_relayout: bool,
    /// Resolved `align-self` (flex-relative: `start`/`end`/`self-*` are
    /// mapped to `FlexStart`/`FlexEnd`).
    align: ItemAlign,
    /// `safe` alignment: overflowing items align to the start.
    align_safe: bool,
    /// `last baseline` alignment.
    last_baseline: bool,
    /// Distance from the margin-box top to the first baseline (row axis).
    ascent: f32,
    /// Distance from the last baseline to the margin-box bottom.
    below: f32,
    frozen: bool,
    /// Final outer main / cross positions relative to the content box.
    main_pos: f32,
    cross_pos: f32,
    /// Margins after auto-margin distribution.
    margin: Edges<f32>,
}

impl FlexItem {
    fn outer_main(&self, axis: Axis) -> f32 {
        self.main + self.edges.pb(axis) + self.margin_sum_axis(axis)
    }
    fn margin_sum_axis(&self, axis: Axis) -> f32 {
        match axis {
            Axis::Row => self.margin.left + self.margin.right,
            Axis::Column => self.margin.top + self.margin.bottom,
        }
    }
    fn outer_cross(&self, axis: Axis) -> f32 {
        let cross_axis = other(axis);
        self.cross_hyp + self.edges.pb(cross_axis) + self.margin_sum_axis(cross_axis)
    }
}

fn other(axis: Axis) -> Axis {
    match axis {
        Axis::Row => Axis::Column,
        Axis::Column => Axis::Row,
    }
}

pub(crate) fn is_scroll_container(style: &ComputedStyle) -> bool {
    // Flex §4.5 and Grid §6.6 use the computed scrollable overflow values,
    // not whether the element clips its painting or currently overflows.
    // `clip`, like `visible`, preserves the content-based automatic minimum.
    let scrollable = |overflow| matches!(overflow, Overflow::Hidden | Overflow::Scroll | Overflow::Auto);
    scrollable(style.overflow_x) || scrollable(style.overflow_y)
}

impl<'a> Layouter<'a> {
    /// Lay out the flex items of `b`; returns the content height.
    pub(crate) fn layout_flex(&mut self, b: &mut LayoutBox, cb: &Cb) -> f32 {
        // The container's (min, max) content heights when its height is
        // indefinite (set by `layout_block` just before this call).
        let limits = self.flex_limits.take();
        let style = b.style.clone();
        let axis = match style.flex_direction {
            FlexDirection::Row | FlexDirection::RowReverse => Axis::Row,
            FlexDirection::Column | FlexDirection::ColumnReverse => Axis::Column,
        };
        let dir_reverse = matches!(style.flex_direction, FlexDirection::RowReverse | FlexDirection::ColumnReverse);
        // Right-to-left rows start at the right; right-to-left columns
        // have their cross start at the right.
        let rtl = style.direction == Direction::Rtl;
        let reverse_main = dir_reverse ^ (axis == Axis::Row && rtl);
        let wrap_reverse = style.flex_wrap == FlexWrap::WrapReverse;
        // `left`/`right` name physical sides only along a horizontal axis.
        let physical = match (axis, style.writing_mode) {
            (Axis::Row, wm) if !wm.is_vertical() => Some(reverse_main),
            // A column's block axis is horizontal in vertical writing modes.
            (Axis::Column, rabbithole_style::WritingMode::VerticalRl) => Some(!dir_reverse),
            (Axis::Column, rabbithole_style::WritingMode::VerticalLr | rabbithole_style::WritingMode::SidewaysLr) => Some(dir_reverse),
            _ => None,
        };
        let justify = flex_relative(style.justify_content, rabbithole_style::AlignMods::of_content(style.justify_content, style.justify_content_flags), dir_reverse, physical);
        let align_content = flex_relative(style.align_content, rabbithole_style::AlignMods::of_content(style.align_content, style.align_content_flags), wrap_reverse, None);
        let cbw = cb.width;
        let content_x = b.content_rect.x;
        let content_y = b.content_rect.y;
        let (mut avail_main, avail_cross) = match axis {
            Axis::Row => (Some(cbw), cb.height),
            Axis::Column => (cb.height, Some(cbw)),
        };
        let gap_main = match axis {
            Axis::Row => style.column_gap.resolve_or_zero(cbw),
            Axis::Column => style.row_gap.resolve_or_zero(cb.height.unwrap_or(0.0)),
        };
        let gap_cross = match axis {
            Axis::Row => style.row_gap.resolve_or_zero(cb.height.unwrap_or(0.0)),
            Axis::Column => style.column_gap.resolve_or_zero(cbw),
        };

        // Working storage, reused across containers (nested containers
        // take their own).
        let FlexScratch { mut order, mut items, mut lines, mut ids, metrics: mut line_metrics } = self.flex_scratch.pop().unwrap_or_default();
        order.clear();
        items.clear();
        lines.clear();
        ids.clear();
        line_metrics.clear();
        // §9.1 collect items in order-modified document order.
        for (i, c) in b.children.iter_mut().enumerate() {
            match c.flow {
                Flow::Absolute => {
                    c.static_pos = (content_x, content_y);
                    c.static_align = None;
                }
                _ => order.push((c.style.order, i)),
            }
        }
        order.sort_by_key(|&(o, i)| (o, i));

        // The cross size of the single line of a container with a definite
        // cross size, which stretched items take (§9.8).
        let stretch_line = if !is_multi_line(&style) { avail_cross } else { None };
        // §9.2 determine flex base size and hypothetical main size.
        items.reserve(order.len());
        for &(_, idx) in &order {
            let item = self.measure_flex_item(b, idx, axis, cbw, cb.height, avail_main, stretch_line, &style);
            items.push(item);
        }

        // §9.3 collect into flex lines: ranges of `ids` (the item indices
        // in order).
        ids.extend(0..items.len());
        let wraps = is_multi_line(&style);
        // A column container with an indefinite height breaks lines at its
        // maximum height (§9.2 step 2).
        let break_main = avail_main.or_else(|| limits.map(|(_, max)| max).filter(|m| m.is_finite()));
        if !wraps || break_main.is_none() {
            lines.push(0..items.len());
        } else {
            let outers: Vec<f32> = items.iter().map(|it| it.hyp + it.edges.pb(axis) + it.edges.margin_sum(axis)).collect();
            let mut avail = break_main.unwrap_or(f32::INFINITY);
            if style.flex_wrap_balance && outers.len() > 1 {
                avail = balanced_width(&outers, gap_main, avail, style.flex_line_count);
            }
            break_line_ranges(&outers, gap_main, avail, &mut lines);
        }

        // §9.2 step 3: an indefinite column container is as tall as its
        // largest line of hypothetical sizes, clamped by min/max-height;
        // its items then flex within that size.
        if avail_main.is_none() {
            if let Some((min, max)) = limits {
                let longest = lines
                    .iter()
                    .map(|r| &ids[r.clone()])
                    .map(|line| line.iter().map(|&i| items[i].hyp + items[i].edges.pb(axis) + items[i].edges.margin_sum(axis)).sum::<f32>() + gap_main * line.len().saturating_sub(1) as f32)
                    .fold(0.0f32, f32::max);
                let clamped = longest.clamp(min, max);
                // Shorter lines flex within the container's size too.
                if (clamped - longest).abs() > 0.01 || lines.len() > 1 {
                    avail_main = Some(clamped);
                }
            }
        }

        // §9.7 resolve flexible lengths per line.
        for r in &lines {
            self.resolve_flexible_lengths(&mut items, &ids[r.clone()], axis, avail_main, gap_main);
        }

        // §9.4 cross size: lay out every item with its used main size.
        // A stretched row item whose contents depend on a definite height
        // is laid out again with its line's height, so its measured height
        // is remembered from the previous layout instead of measuring the
        // subtree again (nested containers would otherwise be laid out
        // twice per level on every layout).
        for it in items.iter_mut() {
            let child = &b.children[it.idx];
            if axis == Axis::Row && it.stretches && child.kind != BoxKind::Image && self.definite_relayouts == 0 && depends_on_definite_height(child) {
                if let Some(&(main, w, h, cross, ascent)) = child.flex_measure.as_deref() {
                    if main == it.main && w == cbw && h == cb.height {
                        it.cross_hyp = cross;
                        it.ascent = ascent;
                        it.must_relayout = true;
                        continue;
                    }
                }
                self.layout_flex_item(b, it, axis, cbw, cb.height, None);
                let m = (it.main, cbw, cb.height, it.cross_hyp, it.ascent);
                match &mut b.children[it.idx].flex_measure {
                    Some(slot) => **slot = m,
                    slot => *slot = Some(Box::new(m)),
                }
            } else {
                self.layout_flex_item(b, it, axis, cbw, cb.height, None);
            }
        }

        // Line cross sizes.
        let single_definite = lines.len() == 1 && avail_cross.is_some() && !wraps;
        // Per line: cross size, first-baseline ascent, last-baseline
        // distance to the bottom (one allocation).
        line_metrics.reserve(lines.len());
        for r in &lines {
            let line = &ids[r.clone()];
            let mut max_asc: f32 = 0.0;
            let mut max_desc: f32 = 0.0;
            let mut max_outer: f32 = 0.0;
            // The `last baseline` group shares the last baselines.
            let (mut last_above, mut last_below) = (0.0f32, 0.0f32);
            for &i in line {
                let it = &items[i];
                let outer = it.outer_cross(axis);
                if participates_in_baseline(it, axis) {
                    if it.last_baseline {
                        last_below = last_below.max(it.below);
                        last_above = last_above.max(outer - it.below);
                    } else {
                        max_asc = max_asc.max(it.ascent);
                        max_desc = max_desc.max(outer - it.ascent);
                    }
                } else {
                    max_outer = max_outer.max(outer);
                }
            }
            let cross = max_outer.max(max_asc + max_desc).max(last_above + last_below);
            let lc = if single_definite {
                avail_cross.unwrap_or(cross)
            } else if !wraps && lines.len() == 1 {
                // §9.4 step 15: a single line is clamped by the
                // container's min/max cross size.
                match (axis, limits) {
                    (Axis::Row, Some((min, max))) => cross.clamp(min, max),
                    _ => cross,
                }
            } else {
                cross
            };
            line_metrics.push([lc, max_asc, last_below]);
        }

        // §9.6 align-content: distribute free cross space between lines.
        let n_lines = lines.len();
        let total_cross: f32 = line_metrics.iter().map(|m| m[0]).sum::<f32>() + gap_cross * (n_lines.saturating_sub(1)) as f32;
        // An indefinite row container's height is its lines' total clamped
        // by min/max-height; align-content distributes the difference.
        let avail_cross = avail_cross.or_else(|| match (axis, limits) {
            (Axis::Row, Some((min, max))) if (total_cross.clamp(min, max) - total_cross).abs() > 0.01 => Some(total_cross.clamp(min, max)),
            _ => None,
        });
        let (mut cross_offset, cross_spacing) = match avail_cross {
            Some(ac) if n_lines > 0 && (wraps || n_lines > 1) => {
                let free = ac - total_cross;
                let dist = overflow_fallback(align_content, rabbithole_style::AlignMods::of_content(style.align_content, style.align_content_flags), free, wrap_reverse);
                if dist == ContentDistribution::Stretch && free > 0.0 {
                    let add = free / n_lines as f32;
                    for m in line_metrics.iter_mut() {
                        m[0] += add;
                    }
                    (0.0, 0.0)
                } else {
                    distribute(dist, free, n_lines)
                }
            }
            _ => (0.0, 0.0),
        };
        let container_cross = avail_cross.unwrap_or(line_metrics.iter().map(|m| m[0]).sum::<f32>() + gap_cross * (n_lines.saturating_sub(1)) as f32);
        if style.flex_wrap == FlexWrap::WrapReverse {
            // Lines run from the cross end.
            cross_offset = container_cross - cross_offset;
        }

        // §9.4 step 11 / §9.5 / §9.6: per-line item alignment and main-axis
        // distribution.
        let mut max_main_extent: f32 = 0.0;
        for (li, r) in lines.iter().enumerate() {
            let line = &ids[r.clone()];
            let lc = line_metrics[li][0];
            // Main axis.
            let sum_outer: f32 = line.iter().map(|&i| items[i].outer_main(axis)).sum::<f32>() + gap_main * (line.len().saturating_sub(1)) as f32;
            let container_main = avail_main.unwrap_or(sum_outer);
            let mut free = container_main - sum_outer;
            // Auto margins absorb positive free space.
            let auto_count: usize = line
                .iter()
                .map(|&i| {
                    let (a, bb) = items[i].edges.autos(axis);
                    a as usize + bb as usize
                })
                .sum();
            if free > 0.0 && auto_count > 0 {
                let each = free / auto_count as f32;
                for &i in line {
                    let (a, bb) = items[i].edges.autos(axis);
                    match axis {
                        Axis::Row => {
                            if a {
                                items[i].margin.left += each;
                            }
                            if bb {
                                items[i].margin.right += each;
                            }
                        }
                        Axis::Column => {
                            if a {
                                items[i].margin.top += each;
                            }
                            if bb {
                                items[i].margin.bottom += each;
                            }
                        }
                    }
                }
                free = 0.0;
            }
            let (lead, spacing) = distribute(overflow_fallback(justify, rabbithole_style::AlignMods::of_content(style.justify_content, style.justify_content_flags), free, dir_reverse), free, line.len());
            let mut pos = lead;
            for (k, &i) in line.iter().enumerate() {
                if k > 0 {
                    pos += gap_main + spacing;
                }
                let outer = items[i].outer_main(axis);
                items[i].main_pos = if reverse_main { container_main - pos - outer } else { pos };
                pos += outer;
            }
            max_main_extent = max_main_extent.max(sum_outer);

            // Cross axis.
            let line_start = if style.flex_wrap == FlexWrap::WrapReverse { cross_offset - lc } else { cross_offset };
            for &i in line {
                let it = &mut items[i];
                let cross_axis = other(axis);
                let (ma, mb) = it.edges.autos(cross_axis);
                let outer = it.outer_cross(axis);
                let free_cross = lc - outer;
                let mut stretch_to: Option<f32> = None;
                let offset = if (ma || mb) && free_cross > 0.0 {
                    if ma && mb {
                        free_cross / 2.0
                    } else if ma {
                        free_cross
                    } else {
                        0.0
                    }
                } else {
                    let align = if it.align_safe && free_cross < 0.0 { ItemAlign::FlexStart } else { it.align };
                    match align {
                        ItemAlign::Stretch | ItemAlign::Normal | ItemAlign::Auto => {
                            // Auto margins keep the item from stretching.
                            if it.cross_auto && !ma && !mb {
                                let inner = (lc - it.edges.pb(cross_axis) - it.margin_sum_axis(cross_axis)).max(0.0);
                                stretch_to = Some(inner);
                            }
                            0.0
                        }
                        ItemAlign::FlexStart => 0.0,
                        ItemAlign::FlexEnd => free_cross,
                        ItemAlign::Center => free_cross / 2.0,
                        ItemAlign::Baseline => {
                            if participates_in_baseline(it, axis) {
                                if it.last_baseline {
                                    lc - line_metrics[li][2] - (outer - it.below)
                                } else {
                                    line_metrics[li][1] - it.ascent
                                }
                            } else if it.last_baseline && axis == Axis::Row {
                                // Fallback alignment of `last baseline`: end.
                                // (Column items keep the start until
                                // orthogonal baselines are supported.)
                                free_cross
                            } else {
                                0.0
                            }
                        }
                    }
                };
                if let Some(inner) = stretch_to {
                    // The stretched size is definite (§9.4 step 11): lay
                    // out again when it differs, or when the contents
                    // resolve percentages against it.
                    // A same-size relayout only for definiteness is not
                    // repeated inside another one: nested containers would
                    // otherwise be laid out exponentially often.
                    let for_definiteness = (inner - it.cross_hyp).abs() <= 0.01
                        && !it.must_relayout
                        && axis == Axis::Row
                        && self.definite_relayouts == 0
                        && depends_on_definite_height(&b.children[it.idx]);
                    if it.must_relayout || (inner - it.cross_hyp).abs() > 0.01 || for_definiteness {
                        let b_ptr: &mut LayoutBox = b;
                        self.definite_relayouts += for_definiteness as u32;
                        self.layout_flex_item(b_ptr, it, axis, cbw, cb.height, Some(inner));
                        self.definite_relayouts -= for_definiteness as u32;
                    }
                }
                it.cross_pos = line_start + offset;
            }
            if style.flex_wrap == FlexWrap::WrapReverse {
                cross_offset -= lc + gap_cross + cross_spacing;
            } else {
                cross_offset += lc + gap_cross + cross_spacing;
            }
        }

        // Final placement.
        let flip_cross = axis == Axis::Column && rtl;
        for it in items.iter_mut() {
            if flip_cross {
                it.cross_pos = container_cross - it.cross_pos - it.outer_cross(axis);
            }
            let child = &mut b.children[it.idx];
            let (x, y) = match axis {
                Axis::Row => (content_x + it.main_pos + it.margin.left, content_y + it.cross_pos + it.margin.top),
                Axis::Column => (content_x + it.cross_pos + it.margin.left, content_y + it.main_pos + it.margin.top),
            };
            child.translate(x - child.rect.x, y - child.rect.y);
            child.margin = it.margin;
        }

        // §8.5 the container's first / last baselines: those of the items
        // baseline-aligned in its first / last line, else of the line's
        // first / last item.
        // (Only the items are recorded: their baselines are looked up when
        // asked for, which most containers never are.)
        let line_baseline = |line: &[usize], last: bool| -> Option<u32> {
            let shared = line.iter().find(|&&i| participates_in_baseline(&items[i], axis) && items[i].last_baseline == last);
            let &i = shared.or(if last { line.last() } else { line.first() })?;
            Some(items[i].idx as u32)
        };
        let baselines = (lines.first().and_then(|r| line_baseline(&ids[r.clone()], false)), lines.last().and_then(|r| line_baseline(&ids[r.clone()], true)));
        let none = crate::tree::FLEX_BASELINE_NONE;
        b.flex_baselines = [baselines.0.unwrap_or(none), baselines.1.unwrap_or(none)];

        let size = match axis {
            Axis::Row => line_metrics.iter().map(|m| m[0]).sum::<f32>() + gap_cross * (n_lines.saturating_sub(1)) as f32,
            Axis::Column => avail_main.unwrap_or(max_main_extent),
        };
        self.flex_scratch.push(FlexScratch { order, items, lines, ids, metrics: line_metrics });
        size
    }

    /// §9.2: flex base size, hypothetical main size, min/max and the
    /// cross-size-related facts of one item.
    #[allow(clippy::too_many_arguments)]
    fn measure_flex_item(&mut self, b: &mut LayoutBox, idx: usize, axis: Axis, cbw: f32, cbh: Option<f32>, avail_main: Option<f32>, stretch_line: Option<f32>, container: &ComputedStyle) -> FlexItem {
        let child_style = b.children[idx].style.clone();
        let edges = ItemEdges::new(&child_style, cbw);
        let bb = child_style.box_sizing == BoxSizing::BorderBox;
        let pb_main = edges.pb(axis);
        let (size_prop, min_prop, max_prop, cross_prop) = match axis {
            Axis::Row => (child_style.width, child_style.min_width, child_style.max_width, child_style.height),
            Axis::Column => (child_style.height, child_style.min_height, child_style.max_height, child_style.width),
        };
        let specified = resolve_size(size_prop, avail_main, bb, pb_main);

        let (align, flags) = match child_style.align_self {
            ItemAlign::Auto => (container.align_items, container.align_items_flags),
            a => (a, child_style.align_self_flags),
        };
        // (Most values carry no keyword details worth decoding.)
        let mods = if flags.0 & !rabbithole_style::AlignFlags::NORMAL == 0 && !matches!(align, ItemAlign::FlexStart | ItemAlign::FlexEnd) {
            rabbithole_style::AlignMods::default()
        } else {
            rabbithole_style::AlignMods::of_item(align, flags)
        };
        let wrap_reverse = container.flex_wrap == FlexWrap::WrapReverse;
        // `start`/`end` follow the writing mode, so flip with wrap-reverse;
        // `self-start`/`self-end` also follow the item's direction in the
        // (inline) cross axis of a column container.
        let flip_self = axis == Axis::Column && child_style.direction != container.direction;
        let align = match (mods.pos, align) {
            (AlignPos::Start | AlignPos::Left | AlignPos::Right, _) => if wrap_reverse { ItemAlign::FlexEnd } else { ItemAlign::FlexStart },
            (AlignPos::End, _) => if wrap_reverse { ItemAlign::FlexStart } else { ItemAlign::FlexEnd },
            (AlignPos::SelfStart, _) => if wrap_reverse ^ flip_self { ItemAlign::FlexEnd } else { ItemAlign::FlexStart },
            (AlignPos::SelfEnd, _) => if wrap_reverse ^ flip_self { ItemAlign::FlexStart } else { ItemAlign::FlexEnd },
            (AlignPos::Flex, a) => a,
        };
        // Lay out in physical terms: wrap-reverse puts cross-start at the
        // bottom (right).
        let align = match align {
            ItemAlign::FlexStart if wrap_reverse => ItemAlign::FlexEnd,
            ItemAlign::FlexEnd if wrap_reverse => ItemAlign::FlexStart,
            a => a,
        };

        // Items with a preferred aspect ratio and a definite cross size
        // (specified, or stretched in a single-line container with a
        // definite cross size, §9.8) transfer it to the main axis.
        let cross_axis = other(axis);
        let pb_cross = edges.pb(cross_axis);
        let (avail_cross, cross_min_prop, cross_max_prop) = match axis {
            Axis::Row => (cbh, child_style.min_height, child_style.max_height),
            Axis::Column => (Some(cbw), child_style.min_width, child_style.max_width),
        };
        let ratio = if child_style.aspect_ratio.is_none() && b.children[idx].kind != BoxKind::Image { None } else { item_ratio(&b.children[idx]) };
        let is_replaced = b.children[idx].kind == BoxKind::Image;
        // The ratio relates content boxes for natural ratios and the
        // box-sizing boxes for `aspect-ratio`.
        let ratio_border_box = bb && child_style.aspect_ratio.is_some() && !child_style.aspect_ratio_auto && item_ratio_is_css(&b.children[idx]);
        let transfer = |cross: f32| -> Option<f32> {
            let r = ratio?;
            let main_per_cross = match axis {
                Axis::Row => r,
                Axis::Column => 1.0 / r,
            };
            Some(if ratio_border_box { ((cross + pb_cross) * main_per_cross - pb_main).max(0.0) } else { (cross * main_per_cross).max(0.0) })
        };
        // (Cross limits only matter for ratios and stretched sizes.)
        let (cross_min, cross_max) = if ratio.is_some() || stretch_line.is_some() {
            let lo = resolve_size(cross_min_prop, avail_cross, bb, pb_cross).unwrap_or(0.0);
            (lo, resolve_size(cross_max_prop, avail_cross, bb, pb_cross).unwrap_or(f32::INFINITY).max(lo))
        } else {
            (0.0, f32::INFINITY)
        };
        let (ma, mb) = edges.autos(cross_axis);
        let stretch_like = cross_prop.is_auto() && matches!(align, ItemAlign::Stretch | ItemAlign::Normal) && !ma && !mb;
        let stretched_cross = {
            let stretched = stretch_like;
            stretch_line.filter(|_| stretched).map(|line| (line - pb_cross - edges.margin_sum(cross_axis)).max(0.0).clamp(cross_min, cross_max))
        };
        let definite_cross = ratio.and_then(|_| resolve_size(cross_prop, avail_cross, bb, pb_cross).map(|c| c.clamp(cross_min, cross_max)).or(stretched_cross));
        let transferred = definite_cross.and_then(transfer);

        // Flex §7.2.3 / §9.2: `auto` retrieves the preferred main-size
        // property, including its intrinsic keyword; `content` does not.
        // Keep `specified` numeric for the separate automatic-minimum rule.
        let used_basis = if child_style.flex_basis == Length::Auto && !child_style.flex_basis_content {
            size_prop
        } else {
            child_style.flex_basis
        };
        let basis = resolve_size(used_basis, avail_main, bb, pb_main);
        // Content size along the main axis (also the automatic minimum).
        let (mut content_min, mut content_max) = self.flex_content_sizes(b, idx, axis, cbw, cbh, stretched_cross, stretch_like);
        if let Some(t) = transferred {
            // A replaced item's contents are its ratio; other boxes keep at
            // least their min-content contents.
            if is_replaced {
                (content_min, content_max) = (t, t);
            } else {
                content_min = content_min.max(t);
                content_max = content_min;
            }
        }
        // Intrinsic size keywords of min/max-width/height in the main axis.
        let keyword = |l: Length| match l {
            Length::MinContent => Some(content_min),
            Length::MaxContent => Some(content_max),
            Length::FitContent => Some(avail_main.map(|a| content_max.min((a - pb_main - edges.margin_sum(axis)).max(0.0)).max(content_min)).unwrap_or(content_max)),
            _ => None,
        };
        let max = resolve_size(max_prop, avail_main, bb, pb_main).or_else(|| keyword(max_prop)).unwrap_or(f32::INFINITY);
        let base = match used_basis {
            Length::MinContent => content_min,
            Length::MaxContent => content_max,
            Length::FitContent => {
                let available = avail_main.map(|space| (space - pb_main - edges.margin_sum(axis)).max(0.0));
                available.map(|space| content_max.min(space).max(content_min)).unwrap_or(content_max)
            }
            _ => basis.or(transferred).unwrap_or(content_max),
        }
        .max(0.0);

        let min = match resolve_size(min_prop, avail_main, bb, pb_main) {
            Some(m) => m,
            None if min_prop == Length::Auto => {
                // §4.5 automatic minimum size.
                if is_scroll_container(&child_style) {
                    0.0
                } else {
                    let mut content_based = content_min;
                    // Definite min/max cross sizes limit it through the ratio.
                    if ratio.is_some() {
                        if let Some(hi) = transfer(cross_max).filter(|v| v.is_finite()) {
                            content_based = content_based.min(hi);
                        }
                        if let Some(lo) = transfer(cross_min).filter(|_| cross_min > 0.0) {
                            content_based = content_based.max(lo);
                        }
                    }
                    let content_based = content_based.min(max);
                    match specified {
                        Some(s) => content_based.min(s),
                        None => content_based,
                    }
                }
            }
            None => keyword(min_prop).unwrap_or(0.0),
        };
        let max = max.max(min);
        let hyp = base.clamp(min, max);
        let cross_auto = cross_prop.is_auto() || cross_prop.is_intrinsic();
        FlexItem {
            idx,
            edges,
            grow: child_style.flex_grow.max(0.0),
            shrink: child_style.flex_shrink.max(0.0),
            base,
            hyp,
            min,
            max,
            main: hyp,
            cross_hyp: 0.0,
            stretches: stretch_like,
            must_relayout: false,
            main_definite: basis.is_some() || transferred.is_some() || !matches!(b.children[idx].formatting, crate::tree::Formatting::Block | crate::tree::Formatting::Inline),
            cross_auto,
            align,
            align_safe: mods.safe,
            last_baseline: mods.last && align == ItemAlign::Baseline,
            ascent: 0.0,
            below: 0.0,
            frozen: false,
            main_pos: 0.0,
            cross_pos: 0.0,
            margin: edges.margin,
        }
    }

    /// (min-content, max-content) sizes of an item along the main axis.
    #[allow(clippy::too_many_arguments)]
    fn flex_content_sizes(&mut self, b: &mut LayoutBox, idx: usize, axis: Axis, cbw: f32, cbh: Option<f32>, stretched_cross: Option<f32>, stretch_like: bool) -> (f32, f32) {
        let _probe = self.grid_publication.probe();
        let child = &mut b.children[idx];
        // Content suggestions and content flex bases measure the item's
        // contents, not its own preferred/min/max main size (Flex §4.5, §9.2).
        // The normal intrinsic cache includes those constraints, so isolate
        // the temporary measurement and restore it for ordinary block sizing.
        // Replaced, ratio and orthogonal-wrapper sizing retain their existing
        // measurement paths; a wrapper's logical child owns those properties.
        let (size, min, max) = match axis {
            Axis::Row => (child.style.width, child.style.min_width, child.style.max_width),
            Axis::Column => (child.style.height, child.style.min_height, child.style.max_height),
        };
        let unconstrain = child.kind != BoxKind::Image && !child.wm_wrapper && child.style.aspect_ratio.is_none()
            && (size != Length::Auto || (min != Length::Auto && min != Length::Px(0.0)) || max != Length::Auto);
        // Measured with its stretched height when that can matter.
        let forced_h = if axis == Axis::Row && child.kind != BoxKind::Image { stretched_cross.filter(|_| may_use_definite_height(child)) } else { None };
        // The unconstrained row measurement is remembered like the
        // ordinary intrinsic sizes (when those could be cached).
        if unconstrain && axis == Axis::Row && forced_h.is_none() {
            if let Some(c) = child.flex_content_cache {
                return c;
            }
        }
        let original = if unconstrain {
            let original = (child.style.clone(), child.intrinsic_cache.take());
            let mut style = (*child.style).clone();
            match axis {
                Axis::Row => { style.width = Length::Auto; style.min_width = Length::Px(0.0); style.max_width = Length::Auto; }
                Axis::Column => { style.height = Length::Auto; style.min_height = Length::Px(0.0); style.max_height = Length::Auto; }
            }
            child.style = std::rc::Rc::new(style);
            Some(original)
        } else { None };
        // An orthogonal wrapper in a row is measured without the wrapped
        // box's own logical block size (its physical width) either.
        // (A column measures the wrapped box laid out at its own inline
        // size: an intrinsic keyword there is the content measure.)
        let inner_original = if child.wm_wrapper && axis == Axis::Row {
            child.children.first_mut().filter(|r| matches!(r.style.height, Length::Px(_)) || r.style.min_height != Length::Auto || r.style.max_height != Length::Auto).map(|r| {
                let original = (r.style.clone(), r.intrinsic_cache.take(), r.layout_cache.take());
                let mut style = (*r.style).clone();
                style.height = Length::Auto;
                style.min_height = Length::Auto;
                style.max_height = Length::Auto;
                r.style = std::rc::Rc::new(style);
                (original, child.intrinsic_cache.take())
            })
        } else {
            None
        };
        let sizes = match axis {
            Axis::Row => {
                if child.kind == BoxKind::Image {
                    let pb = child.style.padding.left.resolve_or_zero(cbw) + child.style.padding.right.resolve_or_zero(cbw) + child.style.border_width.left + child.style.border_width.right;
                    // The content size ignores the item's own min/max-width.
                    let (w, _) = self.replaced_size_with(child, Some(cbw), cbh, pb, false);
                    (w, w)
                } else {
                    self.intrinsic_forced_h = forced_h;
                    let r = self.intrinsic_in(child, cbh);
                    self.intrinsic_forced_h = None;
                    r
                }
            }
            Axis::Column => {
                // Content height with the cross (width) size the item will get.
                let edges = ItemEdges::new(&child.style, cbw);
                let bb = child.style.box_sizing == BoxSizing::BorderBox;
                let mode = match resolve_size(child.style.width, Some(cbw), bb, edges.pb_h) {
                    Some(w) => WidthMode::Fixed(w),
                    None if stretch_like => WidthMode::Fixed(stretched_cross.unwrap_or(cbw - edges.pb_h - edges.margin_sum(Axis::Row)).max(0.0)),
                    None => WidthMode::ShrinkToFit,
                };
                let cb = Cb {
                    x: 0.0,
                    width: cbw,
                    height: cbh,
                };
                let mut bfc = Bfc::new();
                self.layout_block(child, &cb, 0.0, &mut bfc, mode);
                let h = child.content_rect.h;
                (h, h)
            }
        };
        if let Some(((style, icache, lcache), wcache)) = inner_original {
            if let Some(r) = child.children.first_mut() {
                r.style = style;
                r.intrinsic_cache = icache;
                r.layout_cache = lcache;
            }
            child.intrinsic_cache = wcache;
        }
        if let Some((style, cache)) = original {
            child.style = style;
            // (Set only when the measurement was cacheable.)
            let measured = std::mem::replace(&mut child.intrinsic_cache, cache);
            if axis == Axis::Row && forced_h.is_none() {
                child.flex_content_cache = measured;
            }
        }
        sizes
    }

    /// Lay out an item with its used main size; `forced_cross` stretches
    /// the cross size. Records the hypothetical cross size and baseline.
    fn layout_flex_item(&mut self, b: &mut LayoutBox, it: &mut FlexItem, axis: Axis, cbw: f32, cbh: Option<f32>, forced_cross: Option<f32>) {
        let child = &mut b.children[it.idx];
        let cb = Cb {
            x: 0.0,
            width: cbw,
            height: cbh,
        };
        let mut bfc = Bfc::new();
        match axis {
            Axis::Row => {
                self.forced_content_height = forced_cross;
                let main = it.main;
                if child.kind == BoxKind::Image {
                    // Replaced items: size via a temporary explicit width.
                    let mut st = (*child.style).clone();
                    st.width = Length::Px(main);
                    st.box_sizing = BoxSizing::ContentBox;
                    if let Some(h) = forced_cross {
                        st.height = Length::Px(h);
                    }
                    let orig = child.style.clone();
                    child.style = std::rc::Rc::new(st);
                    self.forced_content_height = None;
                    self.layout_block(child, &cb, 0.0, &mut bfc, WidthMode::Fixed(main));
                    child.style = orig;
                } else {
                    self.layout_block(child, &cb, 0.0, &mut bfc, WidthMode::Fixed(main));
                }
                it.cross_hyp = child.content_rect.h;
            }
            Axis::Column => {
                let bb = child.style.box_sizing == BoxSizing::BorderBox;
                let mode = match forced_cross {
                    Some(w) => WidthMode::Fixed(w),
                    None => match resolve_size(child.style.width, Some(cbw), bb, it.edges.pb_h) {
                        Some(w) => WidthMode::Fixed(w),
                        None => {
                            if matches!(it.align, ItemAlign::Stretch | ItemAlign::Normal | ItemAlign::Auto) && !it.edges.auto.left && !it.edges.auto.right {
                                WidthMode::Fixed((cbw - it.edges.pb_h - it.edges.margin_sum(Axis::Row)).max(0.0))
                            } else {
                                WidthMode::ShrinkToFit
                            }
                        }
                    },
                };
                if child.kind == BoxKind::Image {
                    let mut st = (*child.style).clone();
                    st.height = Length::Px(it.main);
                    st.box_sizing = BoxSizing::ContentBox;
                    if let WidthMode::Fixed(w) = mode {
                        st.width = Length::Px(w);
                    }
                    let orig = child.style.clone();
                    child.style = std::rc::Rc::new(st);
                    self.layout_block(child, &cb, 0.0, &mut bfc, WidthMode::Fill);
                    child.style = orig;
                } else {
                    self.forced_content_height = Some(it.main);
                    self.forced_height_indefinite = cbh.is_none() && !it.main_definite;
                    self.layout_block(child, &cb, 0.0, &mut bfc, mode);
                }
                it.cross_hyp = child.content_rect.w;
            }
        }
        self.forced_content_height = None;
        // Baselines relative to the margin box; an item without one has
        // a baseline synthesized from its border box's bottom edge.
        // (Vertical containers align central baselines: synthesized
        // halfway across the border box.)
        let margin_top = child.rect.y - it.edges.margin.top;
        let margin_bottom = child.rect.bottom() + it.edges.margin.bottom;
        let synthesized = if b.style.central_baseline() { child.rect.y + child.rect.h / 2.0 } else { child.rect.bottom() };
        it.ascent = child.first_baseline().unwrap_or(synthesized) - margin_top;
        if it.last_baseline {
            it.below = margin_bottom - child.last_baseline().unwrap_or(synthesized);
        }
    }

    /// §9.7 Resolving Flexible Lengths.
    fn resolve_flexible_lengths(&mut self, items: &mut [FlexItem], line: &[usize], axis: Axis, avail_main: Option<f32>, gap: f32) {
        let avail = match avail_main {
            Some(a) => a,
            None => {
                for &i in line {
                    items[i].main = items[i].hyp;
                }
                return;
            }
        };
        let gaps = gap * (line.len().saturating_sub(1)) as f32;
        let outer_fixed = |it: &FlexItem| it.edges.pb(axis) + it.edges.margin_sum(axis);
        // 1. Determine the used flex factor.
        let sum_hyp: f32 = line.iter().map(|&i| items[i].hyp + outer_fixed(&items[i])).sum::<f32>() + gaps;
        let grow = sum_hyp < avail;
        // 2. Size inflexible items.
        for &i in line {
            let it = &mut items[i];
            let factor = if grow { it.grow } else { it.shrink };
            if factor == 0.0 || (grow && it.base > it.hyp) || (!grow && it.base < it.hyp) {
                it.frozen = true;
                it.main = it.hyp;
            } else {
                it.frozen = false;
                it.main = it.base;
            }
        }
        // 3. Initial free space.
        let free_space = |items: &[FlexItem]| -> f32 {
            let used: f32 = line
                .iter()
                .map(|&i| {
                    let it = &items[i];
                    (if it.frozen { it.main } else { it.base }) + outer_fixed(it)
                })
                .sum::<f32>()
                + gaps;
            avail - used
        };
        let initial_free = free_space(items);
        // 4. Loop.
        let mut guard = 0;
        loop {
            guard += 1;
            if guard > 64 {
                break;
            }
            let unfrozen: Vec<usize> = line.iter().copied().filter(|&i| !items[i].frozen).collect();
            if unfrozen.is_empty() {
                break;
            }
            let mut sum_factors: f32 = unfrozen.iter().map(|&i| if grow { items[i].grow } else { items[i].shrink }).sum();
            if sum_factors.is_infinite() {
                // Infinite factors share the free space; finite ones get none.
                for &i in &unfrozen {
                    let it = &mut items[i];
                    let f = if grow { &mut it.grow } else { &mut it.shrink };
                    *f = if f.is_infinite() { 1.0 } else { 0.0 };
                }
                sum_factors = unfrozen.iter().map(|&i| if grow { items[i].grow } else { items[i].shrink }).sum();
            }
            let mut remaining = free_space(items);
            if sum_factors < 1.0 {
                let scaled = initial_free * sum_factors;
                if scaled.abs() < remaining.abs() {
                    remaining = scaled;
                }
            }
            if grow {
                if sum_factors > 0.0 {
                    for &i in &unfrozen {
                        let it = &mut items[i];
                        it.main = it.base + remaining * it.grow / sum_factors;
                    }
                }
            } else {
                let sum_scaled: f32 = unfrozen.iter().map(|&i| items[i].shrink * items[i].base).sum();
                if sum_scaled > 0.0 {
                    for &i in &unfrozen {
                        let it = &mut items[i];
                        let ratio = it.shrink * it.base / sum_scaled;
                        it.main = it.base - remaining.abs() * ratio;
                    }
                }
            }
            // Fix min/max violations.
            let mut total_violation = 0.0f32;
            let mut violations: Vec<(usize, i8)> = Vec::new();
            for &i in &unfrozen {
                let it = &mut items[i];
                let clamped = it.main.clamp(it.min, it.max);
                if clamped > it.main + 0.001 {
                    violations.push((i, 1));
                } else if clamped < it.main - 0.001 {
                    violations.push((i, -1));
                }
                total_violation += clamped - it.main;
                it.main = clamped;
            }
            if total_violation.abs() < 0.001 {
                for &i in &unfrozen {
                    items[i].frozen = true;
                }
            } else if total_violation > 0.0 {
                for (i, v) in violations {
                    if v > 0 {
                        items[i].frozen = true;
                    }
                }
            } else {
                for (i, v) in violations {
                    if v < 0 {
                        items[i].frozen = true;
                    }
                }
            }
        }
        for &i in line {
            let it = &mut items[i];
            it.main = it.main.max(0.0);
        }
    }

    /// Intrinsic (min-content, max-content) widths of a flex container.
    pub(crate) fn flex_intrinsic(&mut self, b: &mut LayoutBox) -> (f32, f32) {
        let style = b.style.clone();
        let row = matches!(style.flex_direction, FlexDirection::Row | FlexDirection::RowReverse);
        let wraps = is_multi_line(&style);
        if !row && wraps {
            // Column wrap: the lines' widths depend on how the items break
            // at the container's (maximum) height.
            let pb_v = crate::block::vertical_extras(&style);
            let max_h = match style.max_height {
                Length::Px(v) => Some(if style.box_sizing == BoxSizing::BorderBox { (v - pb_v).max(0.0) } else { v }),
                _ => None,
            };
            if let Some(limit) = self.intrinsic_cb_h.or(max_h) {
                self.intrinsic_cb_used |= self.intrinsic_cb_h.is_some();
                return self.column_wrap_intrinsic(b, limit);
            }
        }
        let gap = if row { px_or_zero(style.column_gap) } else { 0.0 };
        let mut min: f32 = 0.0;
        let mut max: f32 = 0.0;
        let mut count = 0usize;
        // Balanced lines with a line count: max-content is the narrowest
        // length that fits the items in that many lines.
        let balanced_count = style.flex_line_count.filter(|_| row && wraps && style.flex_wrap_balance);
        let mut contributions: Vec<f32> = Vec::new();
        for c in &mut b.children {
            if !c.is_in_flow() {
                continue;
            }
            let ex = crate::intrinsic::extras_px(&c.style);
            // Items stretched in a single line of a definite height are
            // measured with that height (it transfers through ratios and
            // resolves percentages).
            if row && !wraps {
                if let Some(h) = self.intrinsic_cb_h.filter(|_| stretches_in_cross(c, &style) && may_use_definite_height(c)) {
                    let pb_v = crate::block::vertical_extras(&c.style);
                    let m = px_or_zero(c.style.margin.top) + px_or_zero(c.style.margin.bottom);
                    let mut inner = (h - pb_v - m).max(0.0);
                    if let Some(max) = resolve_size(c.style.max_height, None, c.style.box_sizing == BoxSizing::BorderBox, pb_v) {
                        inner = inner.min(max);
                    }
                    if let Some(min) = resolve_size(c.style.min_height, None, c.style.box_sizing == BoxSizing::BorderBox, pb_v) {
                        inner = inner.max(min);
                    }
                    self.intrinsic_forced_h = Some(inner);
                }
            }
            let (mut cmin, mut cmax) = self.intrinsic(c);
            self.intrinsic_forced_h = None;
            if row {
                if let Length::Px(basis) = c.style.flex_basis {
                    let pb = crate::block::horizontal_extras(&c.style, 0.0);
                    let inner = if c.style.box_sizing == BoxSizing::BorderBox { (basis - pb).max(0.0) } else { basis };
                    cmax = cmax.max(inner);
                    // An item that cannot shrink keeps its flex basis.
                    cmin = if c.style.flex_shrink == 0.0 { cmin.max(inner) } else { cmin.min(inner) };
                }
            }
            if balanced_count.is_some() {
                contributions.push(cmax + ex);
            }
            if row {
                if wraps {
                    min = min.max(cmin + ex);
                } else {
                    min += cmin + ex;
                }
                max += cmax + ex;
            } else {
                min = min.max(cmin + ex);
                max = max.max(cmax + ex);
            }
            count += 1;
        }
        if row && count > 1 {
            let g = gap * (count - 1) as f32;
            max += g;
            if !wraps {
                min += g;
            }
        }
        if balanced_count.is_some() && contributions.len() > 1 {
            max = balanced_width(&contributions, gap, f32::INFINITY, balanced_count).max(min);
        }
        (min, max)
    }
}

impl<'a> Layouter<'a> {
    /// Intrinsic widths of a column-wrap container whose lines break at
    /// `limit` (its definite or maximum content height): the sum of its
    /// lines' widths after the items are broken into lines and flexed.
    fn column_wrap_intrinsic(&mut self, b: &mut LayoutBox, limit: f32) -> (f32, f32) {
        let style = b.style.clone();
        let gap_main = px_or_zero(style.row_gap);
        let gap_cross = px_or_zero(style.column_gap);
        let mut order: Vec<(i32, usize)> = b.children.iter().enumerate().filter(|(_, c)| c.is_in_flow()).map(|(i, c)| (c.style.order, i)).collect();
        order.sort();
        // (outer main, grow, outer min/max widths, ratio data)
        struct Col {
            main: f32,
            extra_main: f32,
            grow: f32,
            min_w: f32,
            max_w: f32,
            ratio: Option<(f32, f32)>,
        }
        let mut cols: Vec<Col> = Vec::with_capacity(order.len());
        for &(_, i) in &order {
            let c = &mut b.children[i];
            let cs = c.style.clone();
            let ex = crate::intrinsic::extras_px(&cs);
            let (cmin, cmax) = self.intrinsic(c);
            let bb = cs.box_sizing == BoxSizing::BorderBox;
            let pb_v = crate::block::vertical_extras(&cs);
            let extra_main = pb_v + px_or_zero(cs.margin.top) + px_or_zero(cs.margin.bottom);
            let px_inner = |l: Length| match l {
                Length::Px(v) => Some(if bb { (v - pb_v).max(0.0) } else { v.max(0.0) }),
                _ => None,
            };
            let basis = if cs.flex_basis == Length::Auto && !cs.flex_basis_content { cs.height } else { cs.flex_basis };
            let mut main = match px_inner(basis) {
                Some(v) => v,
                None => {
                    // Content height at the item's max-content width.
                    let cb = Cb { x: 0.0, width: cmax + ex, height: None };
                    let mut bfc = Bfc::new();
                    self.layout_block(c, &cb, 0.0, &mut bfc, WidthMode::Fixed(cmax));
                    c.content_rect.h
                }
            };
            if let Some(max) = px_inner(cs.max_height) {
                main = main.min(max);
            }
            if let Some(min) = px_inner(cs.min_height) {
                main = main.max(min);
            }
            let ratio = item_ratio(c).filter(|_| cs.width.is_auto()).map(|r| (r, crate::block::horizontal_extras(&cs, 0.0)));
            cols.push(Col { main, extra_main, grow: cs.flex_grow.max(0.0), min_w: cmin + ex, max_w: cmax + ex, ratio });
        }
        let outers: Vec<f32> = cols.iter().map(|c| c.main + c.extra_main).collect();
        let lines = break_lines(&outers, gap_main, limit);
        let (mut min, mut max) = (0.0f32, 0.0f32);
        for (li, line) in lines.iter().enumerate() {
            let used: f32 = line.iter().map(|&k| outers[k]).sum::<f32>() + gap_main * line.len().saturating_sub(1) as f32;
            let free = (limit - used).max(0.0);
            let grow: f32 = line.iter().map(|&k| cols[k].grow).sum();
            let (mut lmin, mut lmax) = (0.0f32, 0.0f32);
            for &k in line {
                let c = &cols[k];
                let main = c.main + if grow > 0.0 { free * c.grow / grow } else { 0.0 };
                let (wmin, wmax) = match c.ratio {
                    // A ratio item's width follows its flexed height.
                    Some((r, pb_h)) => {
                        let w = main * r + pb_h;
                        (w, w)
                    }
                    None => (c.min_w, c.max_w),
                };
                lmin = lmin.max(wmin);
                lmax = lmax.max(wmax);
            }
            let gap = if li > 0 { gap_cross } else { 0.0 };
            min += lmin + gap;
            max += lmax + gap;
        }
        (min, max)
    }
}

/// Whether a flex container is multi-line (`-webkit-box` does not
/// balance lines).
fn is_multi_line(style: &ComputedStyle) -> bool {
    style.flex_wrap != FlexWrap::NoWrap && !(style.webkit_box && style.flex_wrap_balance)
}

/// [`break_lines`] as ranges of item indices, into `out`.
fn break_line_ranges(outers: &[f32], gap: f32, avail: f32, out: &mut Vec<std::ops::Range<usize>>) {
    let mut start = 0;
    let mut used = 0.0f32;
    for (i, &outer) in outers.iter().enumerate() {
        let needed = if i == start { outer } else { used + gap + outer };
        if i > start && needed > avail + 0.01 {
            out.push(start..i);
            start = i;
            used = outer;
        } else {
            used = needed;
        }
    }
    out.push(start..outers.len());
}

/// Greedy line breaking of outer main sizes in `avail` (§9.3).
fn break_lines(outers: &[f32], gap: f32, avail: f32) -> Vec<Vec<usize>> {
    let mut lines: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut used = 0.0f32;
    for (i, &outer) in outers.iter().enumerate() {
        let needed = if cur.is_empty() { outer } else { used + gap + outer };
        if !cur.is_empty() && needed > avail + 0.01 {
            lines.push(std::mem::take(&mut cur));
            used = outer;
        } else {
            used = needed;
        }
        cur.push(i);
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

/// `flex-wrap: balance`: the narrowest line length that breaks the items
/// into as many lines as greedy breaking in `avail` does (at least
/// `flex-line-count`, at most one line per item).
fn balanced_width(outers: &[f32], gap: f32, avail: f32, line_count: Option<u32>) -> f32 {
    let count = |w: f32| break_lines(outers, gap, w).len();
    let target = count(avail).max(line_count.unwrap_or(1) as usize).min(outers.len());
    let widest = outers.iter().copied().fold(0.0f32, f32::max);
    let total: f32 = outers.iter().sum::<f32>() + gap * (outers.len() - 1) as f32;
    let (mut lo, mut hi) = (widest, total.min(avail).max(widest));
    if count(lo) > target {
        // Searching between the widest item and the greedy length.
        for _ in 0..40 {
            let mid = (lo + hi) / 2.0;
            if count(mid) <= target {
                hi = mid;
            } else {
                lo = mid;
            }
            if hi - lo < 0.01 {
                break;
            }
        }
        hi
    } else {
        lo
    }
}

/// Whether a box's intrinsic widths can depend on its definite height:
/// replaced and aspect-ratio boxes, flex/grid containers, and contents
/// with percentage heights.
fn may_use_definite_height(c: &LayoutBox) -> bool {
    use crate::tree::Formatting;
    // Memoized in bits 2-3 of `pct_dependent` while the box is reused.
    match c.pct_dependent.get() >> 2 {
        1 => return false,
        2 => return true,
        _ => {}
    }
    let r = c.kind == BoxKind::Image
        || c.style.aspect_ratio.is_some()
        // (A column-wrap container's width follows its height.)
        || (c.formatting == Formatting::Flex && is_multi_line(&c.style) && matches!(c.style.flex_direction, FlexDirection::Column | FlexDirection::ColumnReverse))
        || c.children.iter().any(|g| g.style.height.has_percent() || g.style.max_height.has_percent() || g.style.min_height.has_percent());
    c.pct_dependent.set((c.pct_dependent.get() & 3) | if r { 8 } else { 4 });
    r
}

/// Whether a flex item is stretched in the cross axis of `container`:
/// `align-self` stretch (or normal), an auto cross size and no auto
/// cross-axis margins (row containers).
fn stretches_in_cross(c: &LayoutBox, container: &ComputedStyle) -> bool {
    let align = if c.style.align_self == ItemAlign::Auto { container.align_items } else { c.style.align_self };
    let mods = if c.style.align_self == ItemAlign::Auto { rabbithole_style::AlignMods::of_item(container.align_items, container.align_items_flags) } else { rabbithole_style::AlignMods::of_item(c.style.align_self, c.style.align_self_flags) };
    matches!(align, ItemAlign::Stretch | ItemAlign::Normal) && mods.pos == AlignPos::Flex && c.style.height.is_auto() && !c.style.margin.top.is_auto() && !c.style.margin.bottom.is_auto()
}

/// The preferred aspect ratio (width / height) of a flex item: its
/// `aspect-ratio`, else a replaced element's natural ratio.
pub(crate) fn item_ratio(b: &LayoutBox) -> Option<f32> {
    let css = b.style.aspect_ratio.filter(|r| *r > 0.0 && r.is_finite());
    if b.style.aspect_ratio_auto && b.kind == BoxKind::Image {
        return natural_ratio(b).or(css);
    }
    css.or_else(|| natural_ratio(b))
}

/// Whether [`item_ratio`] comes from the `aspect-ratio` property.
fn item_ratio_is_css(b: &LayoutBox) -> bool {
    !(b.style.aspect_ratio_auto && b.kind == BoxKind::Image && natural_ratio(b).is_some())
}

/// A replaced element's natural aspect ratio.
fn natural_ratio(b: &LayoutBox) -> Option<f32> {
    if b.kind != BoxKind::Image {
        return None;
    }
    let (w, h) = match b.image.as_ref().filter(|i| !i.broken) {
        Some(img) if img.no_ratio => return None,
        Some(img) => img.natural_size(),
        None => (b.attr_size.0.unwrap_or(0.0), b.attr_size.1.unwrap_or(0.0)),
    };
    (w > 0.0 && h > 0.0).then(|| w / h)
}

/// Whether an item takes part in baseline alignment: `baseline` items of
/// a row container without cross-axis auto margins.
fn participates_in_baseline(it: &FlexItem, axis: Axis) -> bool {
    it.align == ItemAlign::Baseline && axis == Axis::Row && !it.edges.auto.top && !it.edges.auto.bottom
}

/// Map `start`/`end`/`left`/`right` of a content-distribution value to
/// flex-relative positions. `reversed` says whether the axis runs against
/// the writing mode (reverse direction or wrap-reverse); `physical` is
/// Some(whether main-start is at the right) for a horizontal main axis.
fn flex_relative(dist: ContentDistribution, mods: AlignMods, reversed: bool, physical: Option<bool>) -> ContentDistribution {
    let start = |at_start: bool| if at_start { ContentDistribution::FlexStart } else { ContentDistribution::FlexEnd };
    match (mods.pos, physical) {
        (AlignPos::Start | AlignPos::SelfStart, _) | (AlignPos::Left | AlignPos::Right, None) => start(!reversed),
        (AlignPos::End | AlignPos::SelfEnd, _) => start(reversed),
        (AlignPos::Left, Some(right_start)) => start(!right_start),
        (AlignPos::Right, Some(right_start)) => start(right_start),
        (AlignPos::Flex, _) => dist,
    }
}

/// The alignment used when the items overflow (negative free space):
/// `safe` aligns to the start, and the distributed values fall back to
/// `flex-start` / `safe center` (CSS Align §4.3).
fn overflow_fallback(dist: ContentDistribution, mods: AlignMods, free: f32, reversed: bool) -> ContentDistribution {
    if free >= 0.0 {
        return dist;
    }
    match dist {
        ContentDistribution::SpaceAround | ContentDistribution::SpaceEvenly | ContentDistribution::SpaceBetween | ContentDistribution::Stretch => ContentDistribution::FlexStart,
        _ if mods.safe => if reversed { ContentDistribution::FlexEnd } else { ContentDistribution::FlexStart },
        d => d,
    }
}

/// Whether laying out `b` with a definite content height (rather than an
/// auto one that comes out the same) can change its contents: children
/// sized by percentages of that height, also through nested flex and grid
/// containers (whose stretched or flexed items become definite).
fn depends_on_definite_height(b: &LayoutBox) -> bool {
    use crate::tree::Formatting;
    match b.pct_dependent.get() & 3 {
        1 => return false,
        2 => return true,
        _ => {}
    }
    let pct = |c: &LayoutBox| c.style.height.has_percent() || c.style.min_height.has_percent() || c.style.max_height.has_percent();
    let dep = if matches!(b.formatting, Formatting::Flex | Formatting::Grid) {
        // Items with percentage sizes or flex bases, and items whose
        // contents resolve percentages against them.
        b.children.iter().any(|c| {
            c.is_in_flow()
                && (pct(c) || c.style.flex_basis.has_percent()
                    || match c.formatting {
                    Formatting::Flex | Formatting::Grid => depends_on_definite_height(c),
                    Formatting::Block | Formatting::Inline => c.children.iter().any(|g| g.is_in_flow() && pct(g)),
                    _ => false,
                })
        })
    } else {
        b.children.iter().any(|c| c.is_in_flow() && (pct(c) || (matches!(c.formatting, Formatting::Flex | Formatting::Grid) && depends_on_definite_height(c))))
    };
    b.pct_dependent.set((b.pct_dependent.get() & !3) | if dep { 2 } else { 1 });
    dep
}

pub(crate) fn px_or_zero(l: Length) -> f32 {
    match l {
        Length::Px(v) => v,
        _ => 0.0,
    }
}
