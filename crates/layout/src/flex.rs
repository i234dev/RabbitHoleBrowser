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
use rabbithole_style::{BoxSizing, ComputedStyle, ContentDistribution, Edges, FlexDirection, FlexWrap, ItemAlign, Length, Overflow};

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
    /// Whether the cross size property is `auto` (stretchable).
    cross_auto: bool,
    /// Resolved `align-self`.
    align: ItemAlign,
    /// Distance from the margin-box top to the first baseline (row axis).
    ascent: f32,
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
    !matches!(style.overflow_x, Overflow::Visible) || !matches!(style.overflow_y, Overflow::Visible)
}

impl<'a> Layouter<'a> {
    /// Lay out the flex items of `b`; returns the content height.
    pub(crate) fn layout_flex(&mut self, b: &mut LayoutBox, cb: &Cb) -> f32 {
        let style = b.style.clone();
        let axis = match style.flex_direction {
            FlexDirection::Row | FlexDirection::RowReverse => Axis::Row,
            FlexDirection::Column | FlexDirection::ColumnReverse => Axis::Column,
        };
        let reverse_main = matches!(style.flex_direction, FlexDirection::RowReverse | FlexDirection::ColumnReverse);
        let cbw = cb.width;
        let content_x = b.content_rect.x;
        let content_y = b.content_rect.y;
        let (avail_main, avail_cross) = match axis {
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

        // §9.1 collect items in order-modified document order.
        let mut order: Vec<(i32, usize)> = Vec::new();
        for (i, c) in b.children.iter_mut().enumerate() {
            match c.flow {
                Flow::Absolute => {
                    c.static_pos = (content_x, content_y);
                }
                _ => order.push((c.style.order, i)),
            }
        }
        order.sort_by_key(|&(o, i)| (o, i));

        // §9.2 determine flex base size and hypothetical main size.
        let mut items: Vec<FlexItem> = Vec::with_capacity(order.len());
        for &(_, idx) in &order {
            let item = self.measure_flex_item(b, idx, axis, cbw, cb.height, avail_main, &style);
            items.push(item);
        }

        // §9.3 collect into flex lines.
        let mut lines: Vec<Vec<usize>> = Vec::new();
        let wraps = style.flex_wrap != FlexWrap::NoWrap;
        if !wraps || avail_main.is_none() {
            lines.push((0..items.len()).collect());
        } else {
            let avail = avail_main.unwrap_or(f32::INFINITY);
            let mut cur: Vec<usize> = Vec::new();
            let mut used = 0.0f32;
            for (i, it) in items.iter().enumerate() {
                let outer = it.hyp + it.edges.pb(axis) + it.edges.margin_sum(axis);
                let needed = if cur.is_empty() { outer } else { used + gap_main + outer };
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
        }

        // §9.7 resolve flexible lengths per line.
        for line in &lines {
            self.resolve_flexible_lengths(&mut items, line, axis, avail_main, gap_main);
        }

        // §9.4 cross size: lay out every item with its used main size.
        for it in items.iter_mut() {
            self.layout_flex_item(b, it, axis, cbw, cb.height, None);
        }

        // Line cross sizes.
        let single_definite = lines.len() == 1 && avail_cross.is_some() && !wraps;
        let mut line_cross: Vec<f32> = Vec::with_capacity(lines.len());
        let mut line_ascent: Vec<f32> = Vec::with_capacity(lines.len());
        for line in &lines {
            let mut max_asc: f32 = 0.0;
            let mut max_desc: f32 = 0.0;
            let mut max_outer: f32 = 0.0;
            for &i in line {
                let it = &items[i];
                let outer = it.outer_cross(axis);
                if it.align == ItemAlign::Baseline && axis == Axis::Row && !it.edges.auto.top && !it.edges.auto.bottom {
                    max_asc = max_asc.max(it.ascent);
                    max_desc = max_desc.max(outer - it.ascent);
                } else {
                    max_outer = max_outer.max(outer);
                }
            }
            let cross = max_outer.max(max_asc + max_desc);
            line_cross.push(if single_definite { avail_cross.unwrap_or(cross) } else { cross });
            line_ascent.push(max_asc);
        }

        // §9.6 align-content: distribute free cross space between lines.
        let n_lines = lines.len();
        let total_cross: f32 = line_cross.iter().sum::<f32>() + gap_cross * (n_lines.saturating_sub(1)) as f32;
        let (mut cross_offset, cross_spacing) = match avail_cross {
            Some(ac) if n_lines > 0 && (wraps || n_lines > 1) => {
                let free = ac - total_cross;
                if style.align_content == ContentDistribution::Stretch && free > 0.0 {
                    let add = free / n_lines as f32;
                    for lc in line_cross.iter_mut() {
                        *lc += add;
                    }
                    (0.0, 0.0)
                } else {
                    distribute(style.align_content, free, n_lines)
                }
            }
            _ => (0.0, 0.0),
        };
        let container_cross = avail_cross.unwrap_or(line_cross.iter().sum::<f32>() + gap_cross * (n_lines.saturating_sub(1)) as f32);
        if style.flex_wrap == FlexWrap::WrapReverse {
            // Lines run from the cross end.
            cross_offset = container_cross - cross_offset;
        }

        // §9.4 step 11 / §9.5 / §9.6: per-line item alignment and main-axis
        // distribution.
        let mut max_main_extent: f32 = 0.0;
        for (li, line) in lines.iter().enumerate() {
            let lc = line_cross[li];
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
            let (lead, spacing) = distribute(style.justify_content, free, line.len());
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
                    match it.align {
                        ItemAlign::Stretch | ItemAlign::Normal | ItemAlign::Auto => {
                            if it.cross_auto {
                                let inner = (lc - it.edges.pb(cross_axis) - it.margin_sum_axis(cross_axis)).max(0.0);
                                stretch_to = Some(inner);
                            }
                            0.0
                        }
                        ItemAlign::FlexStart => 0.0,
                        ItemAlign::FlexEnd => free_cross,
                        ItemAlign::Center => free_cross / 2.0,
                        ItemAlign::Baseline => {
                            if axis == Axis::Row {
                                line_ascent[li] - it.ascent
                            } else {
                                0.0
                            }
                        }
                    }
                };
                if let Some(inner) = stretch_to {
                    if (inner - it.cross_hyp).abs() > 0.01 {
                        let b_ptr: &mut LayoutBox = b;
                        self.layout_flex_item(b_ptr, it, axis, cbw, cb.height, Some(inner));
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
        for it in &items {
            let child = &mut b.children[it.idx];
            let (x, y) = match axis {
                Axis::Row => (content_x + it.main_pos + it.margin.left, content_y + it.cross_pos + it.margin.top),
                Axis::Column => (content_x + it.cross_pos + it.margin.left, content_y + it.main_pos + it.margin.top),
            };
            child.translate(x - child.rect.x, y - child.rect.y);
            child.margin = it.margin;
        }

        match axis {
            Axis::Row => line_cross.iter().sum::<f32>() + gap_cross * (n_lines.saturating_sub(1)) as f32,
            Axis::Column => avail_main.unwrap_or(max_main_extent),
        }
    }

    /// §9.2: flex base size, hypothetical main size, min/max and the
    /// cross-size-related facts of one item.
    #[allow(clippy::too_many_arguments)]
    fn measure_flex_item(&mut self, b: &mut LayoutBox, idx: usize, axis: Axis, cbw: f32, cbh: Option<f32>, avail_main: Option<f32>, container: &ComputedStyle) -> FlexItem {
        let child_style = b.children[idx].style.clone();
        let edges = ItemEdges::new(&child_style, cbw);
        let bb = child_style.box_sizing == BoxSizing::BorderBox;
        let pb_main = edges.pb(axis);
        let (size_prop, min_prop, max_prop, cross_prop) = match axis {
            Axis::Row => (child_style.width, child_style.min_width, child_style.max_width, child_style.height),
            Axis::Column => (child_style.height, child_style.min_height, child_style.max_height, child_style.width),
        };
        let specified = resolve_size(size_prop, avail_main, bb, pb_main);
        let max = resolve_size(max_prop, avail_main, bb, pb_main).unwrap_or(f32::INFINITY);

        // Flex basis.
        let basis = match child_style.flex_basis {
            Length::Auto => specified,
            l => resolve_size(l, avail_main, bb, pb_main),
        };
        // Content size along the main axis (also the automatic minimum).
        let (content_min, content_max) = self.flex_content_sizes(b, idx, axis, cbw, cbh);
        let base = basis.unwrap_or(content_max).max(0.0);

        let min = match resolve_size(min_prop, avail_main, bb, pb_main) {
            Some(m) => m,
            None if min_prop == Length::Auto => {
                // §4.5 automatic minimum size.
                if is_scroll_container(&child_style) {
                    0.0
                } else {
                    let content_based = content_min.min(max);
                    match specified {
                        Some(s) => content_based.min(s),
                        None => content_based,
                    }
                }
            }
            None => 0.0,
        };
        let max = max.max(min);
        let hyp = base.clamp(min, max);
        let align = match child_style.align_self {
            ItemAlign::Auto => container.align_items,
            a => a,
        };
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
            cross_auto,
            align,
            ascent: 0.0,
            frozen: false,
            main_pos: 0.0,
            cross_pos: 0.0,
            margin: edges.margin,
        }
    }

    /// (min-content, max-content) sizes of an item along the main axis.
    fn flex_content_sizes(&mut self, b: &mut LayoutBox, idx: usize, axis: Axis, cbw: f32, cbh: Option<f32>) -> (f32, f32) {
        let child = &mut b.children[idx];
        match axis {
            Axis::Row => {
                if child.kind == BoxKind::Image {
                    let pb = child.style.padding.left.resolve_or_zero(cbw) + child.style.padding.right.resolve_or_zero(cbw) + child.style.border_width.left + child.style.border_width.right;
                    let (w, _) = self.replaced_size(child, Some(cbw), cbh, pb);
                    (w, w)
                } else {
                    self.intrinsic(child)
                }
            }
            Axis::Column => {
                // Content height with the cross (width) size the item will get.
                let edges = ItemEdges::new(&child.style, cbw);
                let bb = child.style.box_sizing == BoxSizing::BorderBox;
                let mode = match resolve_size(child.style.width, Some(cbw), bb, edges.pb_h) {
                    Some(w) => WidthMode::Fixed(w),
                    None => WidthMode::Fixed((cbw - edges.pb_h - edges.margin_sum(Axis::Row)).max(0.0)),
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
        }
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
                    self.layout_block(child, &cb, 0.0, &mut bfc, mode);
                }
                it.cross_hyp = child.content_rect.w;
            }
        }
        self.forced_content_height = None;
        // Baseline relative to the margin-box top.
        let margin_top = child.rect.y - it.edges.margin.top;
        it.ascent = match child.first_baseline() {
            Some(bl) => bl - margin_top,
            None => child.rect.bottom() + it.edges.margin.bottom - margin_top,
        };
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
            let sum_factors: f32 = unfrozen.iter().map(|&i| if grow { items[i].grow } else { items[i].shrink }).sum();
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
        let wraps = style.flex_wrap != FlexWrap::NoWrap;
        let gap = if row { px_or_zero(style.column_gap) } else { 0.0 };
        let mut min: f32 = 0.0;
        let mut max: f32 = 0.0;
        let mut count = 0usize;
        for c in &mut b.children {
            if !c.is_in_flow() {
                continue;
            }
            let ex = crate::intrinsic::extras_px(&c.style);
            let (mut cmin, mut cmax) = self.intrinsic(c);
            if row {
                if let Length::Px(basis) = c.style.flex_basis {
                    let pb = crate::block::horizontal_extras(&c.style, 0.0);
                    let inner = if c.style.box_sizing == BoxSizing::BorderBox { (basis - pb).max(0.0) } else { basis };
                    cmax = cmax.max(inner);
                    cmin = cmin.min(inner);
                }
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
        (min, max)
    }
}

pub(crate) fn px_or_zero(l: Length) -> f32 {
    match l {
        Length::Px(v) => v,
        _ => 0.0,
    }
}
