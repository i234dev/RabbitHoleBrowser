//! Block layout: width/height resolution, margin collapsing, floats,
//! absolute/relative positioning and the document entry point.

use crate::build::Builder;
use crate::float::Bfc;
use crate::text::Measurer;
use crate::tree::{BoxKind, Flow, Formatting, LayoutBox, LayoutTree, Marker};
use crate::LayoutContext;
use rabbithole_gfx::{FontId, Rect};
use rabbithole_style::{BoxSizing, Clear, ComputedStyle, Edges, Length, Position};
use std::collections::HashMap;

/// Containing block information passed down during layout.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Cb {
    /// Left edge of the containing block's content box.
    pub x: f32,
    /// Content width.
    pub width: f32,
    /// Content height when definite (for percentage heights).
    pub height: Option<f32>,
}

/// How to resolve `width: auto`.
#[derive(Debug, Clone, Copy)]
pub(crate) enum WidthMode {
    /// Fill the containing block (normal block-level boxes).
    Fill,
    /// Shrink-to-fit (floats, inline-blocks, absolutes).
    ShrinkToFit,
    /// Use this content width (table cells, flex items).
    Fixed(f32),
}

pub(crate) struct Layouter<'a> {
    pub ctx: &'a LayoutContext<'a>,
    pub measurer: Measurer<'a>,
    /// grid/flex hook: when set, the next `layout_block` call uses this
    /// content-box height as the box's definite height (stretched flex
    /// items, grid items, definite flex main sizes in column flex).
    pub forced_content_height: Option<f32>,
    /// Whether `FontContext::select_family` & co. are implemented.
    pub font_ext: bool,
    /// Cache of family-list lookups: (families joined, weight, italic) → font.
    font_cache: HashMap<(String, u16, bool), FontId>,
}

pub(crate) fn resolve_edges(e: &Edges<Length>, base: f32) -> Edges<f32> {
    Edges {
        top: e.top.resolve_or_zero(base),
        right: e.right.resolve_or_zero(base),
        bottom: e.bottom.resolve_or_zero(base),
        left: e.left.resolve_or_zero(base),
    }
}

/// Horizontal padding + border in px (percentages against `cbw`).
pub(crate) fn horizontal_extras(style: &ComputedStyle, cbw: f32) -> f32 {
    let p = resolve_edges(&style.padding, cbw);
    p.left + p.right + style.border_width.left + style.border_width.right
}

/// Collapse two vertical margins.
pub(crate) fn collapse(a: f32, b: f32) -> f32 {
    if a >= 0.0 && b >= 0.0 {
        a.max(b)
    } else if a < 0.0 && b < 0.0 {
        a.min(b)
    } else {
        a + b
    }
}

pub(crate) fn shrink_to_fit(min: f32, max: f32, avail: f32) -> f32 {
    min.max(avail.min(max))
}

impl<'a> Layouter<'a> {
    pub fn new(ctx: &'a LayoutContext<'a>) -> Self {
        Layouter {
            ctx,
            measurer: Measurer::new(ctx.fonts),
            forced_content_height: None,
            font_ext: crate::paint::font_ext_available(ctx.fonts),
            font_cache: HashMap::new(),
        }
    }

    /// Font for a style: the first available family of `font-family`
    /// (web fonts / system fonts) or the generic fallback.
    pub fn font_for(&mut self, style: &ComputedStyle) -> FontId {
        if self.font_ext && !style.font_family_list.is_empty() {
            let key = (style.font_family_list.join(","), style.font_weight, style.is_italic());
            if let Some(f) = self.font_cache.get(&key) {
                return *f;
            }
            let f = self.ctx.fonts.select_family(&style.font_family_list, style.font_family, style.font_weight, style.is_italic());
            self.font_cache.insert(key, f);
            return f;
        }
        self.ctx.fonts.select(style.font_family, style.font_weight, style.is_italic())
    }

    /// Does `font` need a synthetic slant for an italic request?
    pub fn synthetic_oblique(&self, font: FontId, style: &ComputedStyle) -> bool {
        style.is_italic() && self.font_ext && self.ctx.fonts.needs_synthetic_italic(font, true)
    }

    pub fn layout_document(&mut self) -> LayoutTree {
        let builder = Builder::new(self.ctx);
        let mut root = builder.build_root();
        let vp_w = self.ctx.viewport_width.max(1.0);
        let vp_h = self.ctx.viewport_height.max(1.0);
        let cb = Cb {
            x: 0.0,
            width: vp_w,
            height: Some(vp_h),
        };
        let mut bfc = Bfc::new();
        let top = self.effective_top_margin(&root, cb.width);
        self.layout_block(&mut root, &cb, top, &mut bfc, WidthMode::Fill);
        let mut doc_h = (root.rect.bottom() + root.collapsed_bottom_margin.max(0.0)).max(vp_h);
        let icb = Rect::new(0.0, 0.0, vp_w, doc_h);
        if root.style.position == Position::Static {
            self.layout_abs_descendants(&mut root, icb);
        }
        self.apply_relative(&mut root, vp_w, vp_h);
        let (max_r, max_b) = bounds(&root);
        doc_h = doc_h.max(max_b);
        LayoutTree {
            root,
            width: vp_w.max(max_r.min(vp_w * 4.0)),
            height: doc_h,
            viewport: (vp_w, vp_h),
        }
    }

    /// Top margin of a block after collapsing with its first descendants.
    pub fn effective_top_margin(&self, b: &LayoutBox, cbw: f32) -> f32 {
        let mt = b.style.margin.top.resolve_or_zero(cbw);
        if !b.flow.is_in_flow() || !self.top_collapsible(b, cbw) {
            return mt;
        }
        let inner_w = (cbw - horizontal_extras(&b.style, cbw) - b.style.margin.left.resolve_or_zero(cbw) - b.style.margin.right.resolve_or_zero(cbw)).max(0.0);
        let inner_w = b.style.width.resolve(cbw).unwrap_or(inner_w);
        match b.children.iter().find(|c| c.is_in_flow()) {
            Some(first) if first.kind != BoxKind::AnonymousBlock && first.formatting != Formatting::None => {
                collapse(mt, self.effective_top_margin(first, inner_w))
            }
            _ => mt,
        }
    }

    fn top_collapsible(&self, b: &LayoutBox, cbw: f32) -> bool {
        !b.bfc_root
            && b.formatting == Formatting::Block
            && b.style.padding.top.resolve_or_zero(cbw) == 0.0
            && b.style.border_width.top == 0.0
    }

    /// Lay out `b` as a block-level box whose border-box top is at `top`.
    pub fn layout_block(&mut self, b: &mut LayoutBox, cb: &Cb, top: f32, bfc: &mut Bfc, mode: WidthMode) {
        let forced_h = self.forced_content_height.take(); // grid/flex hook
        let style = b.style.clone();
        let cbw = cb.width;
        let pad = resolve_edges(&style.padding, cbw);
        let bw = style.border_width;
        let pb_h = pad.left + pad.right + bw.left + bw.right;
        let pb_v = pad.top + pad.bottom + bw.top + bw.bottom;
        let ml = style.margin.left.resolve(cbw);
        let mr = style.margin.right.resolve(cbw);
        let mt = style.margin.top.resolve_or_zero(cbw);
        let mb = style.margin.bottom.resolve_or_zero(cbw);

        let mut mode = mode;
        if b.kind == BoxKind::Table && matches!(mode, WidthMode::Fill) && style.width.is_auto() {
            mode = WidthMode::ShrinkToFit;
        }
        let (mut content_w, ml, mr, replaced_h) = if b.kind == BoxKind::Image {
            let (w, h) = self.replaced_size(b, Some(cbw), cb.height, pb_h);
            let rem = cbw - w - pb_h;
            let (l, r) = match (ml, mr) {
                (None, None) if style.display.is_block_level() && rem > 0.0 => (rem / 2.0, rem / 2.0),
                (l, r) => (l.unwrap_or(0.0), r.unwrap_or(0.0)),
            };
            (w, l, r, Some(h))
        } else {
            let (w, l, r) = self.compute_width(b, cbw, pb_h, ml, mr, mode);
            (w, l, r, None)
        };
        b.margin = Edges {
            top: mt,
            right: mr,
            bottom: mb,
            left: ml,
        };

        let mut x = cb.x + ml;
        let mut y = top;
        if style.clear != Clear::None {
            y = y.max(bfc.clear_y(style.clear));
        }
        // A BFC root placed next to floats narrows (or moves below them).
        if b.bfc_root && b.flow.is_in_flow() && matches!(mode, WidthMode::Fill) && !bfc.is_empty() {
            let mut guard = 0;
            loop {
                let (l, r) = bfc.available(y, 1.0, cb.x, cb.width);
                if l <= cb.x && r >= cb.x + cb.width {
                    break;
                }
                let outer = content_w + pb_h + ml + mr;
                if style.width.is_auto() {
                    content_w = (r - l - ml - mr - pb_h).max(0.0);
                    x = l + ml;
                    break;
                } else if outer <= r - l + 0.01 {
                    x = l + ml;
                    break;
                }
                match bfc.next_y(y) {
                    Some(ny) => y = ny,
                    None => break,
                }
                guard += 1;
                if guard > 100 {
                    break;
                }
            }
        }

        b.rect = Rect::new(x, y, content_w + pb_h, 0.0);
        b.content_rect = Rect::new(x + bw.left + pad.left, y + bw.top + pad.top, content_w, 0.0);

        let mut explicit_h = forced_h.or(replaced_h).or_else(|| self.resolve_height(&style, cb.height, pb_v)); // grid/flex hook
        // `aspect-ratio` with an auto height derives it from the width.
        if explicit_h.is_none() && b.kind != BoxKind::Image {
            if let Some(ratio) = style.aspect_ratio.filter(|r| *r > 0.0) {
                let (w, h_extra) = if style.box_sizing == BoxSizing::BorderBox { (content_w + pb_h, pb_v) } else { (content_w, 0.0) };
                explicit_h = Some((w / ratio - h_extra).max(0.0));
            }
        }
        let inner_cb = Cb {
            x: b.content_rect.x,
            width: content_w,
            height: explicit_h,
        };
        let bottom_collapsible = explicit_h.is_none()
            && !b.bfc_root
            && b.formatting == Formatting::Block
            && pad.bottom == 0.0
            && bw.bottom == 0.0;
        let top_collapsible = self.top_collapsible(b, cbw);
        b.collapsed_bottom_margin = mb;

        let mut local_bfc = Bfc::new();
        let content_h = if b.bfc_root {
            self.layout_contents(b, &inner_cb, &mut local_bfc, top_collapsible, bottom_collapsible)
        } else {
            self.layout_contents(b, &inner_cb, bfc, top_collapsible, bottom_collapsible)
        };

        let mut h = explicit_h.unwrap_or(content_h);
        if b.bfc_root && explicit_h.is_none() && !local_bfc.is_empty() {
            h = h.max(local_bfc.max_bottom() - b.content_rect.y);
        }
        h = self.clamp_height(&style, h, cb.height, pb_v);
        b.rect.h = h + pb_v;
        b.content_rect.h = h;
        if explicit_h.is_some() {
            b.collapsed_bottom_margin = mb;
        }

        self.place_marker(b);

        b.baseline = if b.clips {
            None
        } else {
            b.last_baseline().map(|bl| bl - b.rect.y)
        };

        if style.position != Position::Static {
            let padding_box = b.padding_rect();
            self.layout_abs_descendants(b, padding_box);
        }
        b.fixed = style.position == Position::Fixed;
        self.finish_scroll_container(b);
    }

    /// Record the scrollable content size of clipping boxes and apply the
    /// scroll offset supplied by the engine.
    fn finish_scroll_container(&mut self, b: &mut LayoutBox) {
        if !b.clips {
            return;
        }
        let style = b.style.clone();
        b.scroll_container = matches!(style.overflow_x, rabbithole_style::Overflow::Auto | rabbithole_style::Overflow::Scroll)
            || matches!(style.overflow_y, rabbithole_style::Overflow::Auto | rabbithole_style::Overflow::Scroll);
        let pr = b.padding_rect();
        let mut max_r = pr.right();
        let mut max_b = pr.bottom();
        for c in &b.children {
            let r = content_bounds(c);
            max_r = max_r.max(r.right());
            max_b = max_b.max(r.bottom());
        }
        for line in &b.lines {
            for f in &line.fragments {
                max_r = max_r.max(f.rect.right());
                max_b = max_b.max(f.rect.bottom());
            }
        }
        // Padding at the end of the scrollable area (CSS Overflow 3).
        let pad_r = b.content_rect.right() - pr.right();
        let pad_b = b.content_rect.bottom() - pr.bottom();
        b.scroll_width = (max_r - pr.x + pad_r.max(0.0)).max(pr.w);
        b.scroll_height = (max_b - pr.y + pad_b.max(0.0)).max(pr.h);
        if !b.scroll_container {
            return;
        }
        let Some(node) = b.node else { return };
        let Some(&(ox, oy)) = self.ctx.scroll_offsets.get(&node) else { return };
        let max_x = (b.scroll_width - pr.w).max(0.0);
        let max_y = (b.scroll_height - pr.h).max(0.0);
        let ox = ox.clamp(0.0, max_x);
        let oy = oy.clamp(0.0, max_y);
        b.scroll_offset = (ox, oy);
        if ox != 0.0 || oy != 0.0 {
            crate::table::translate_contents(b, -ox, -oy);
        }
    }

    fn layout_contents(&mut self, b: &mut LayoutBox, cb: &Cb, bfc: &mut Bfc, top_collapsible: bool, bottom_collapsible: bool) -> f32 {
        match b.formatting {
            Formatting::Inline => self.layout_inline(b, cb, bfc),
            Formatting::Block => self.layout_block_children(b, cb, bfc, top_collapsible, bottom_collapsible),
            Formatting::Flex => self.layout_flex(b, cb), // grid/flex hook
            Formatting::Grid => self.layout_grid(b, cb), // grid/flex hook
            Formatting::Table => self.layout_table(b, cb),
            Formatting::TableRow => {
                // A row outside a table: lay out cells as blocks.
                self.layout_block_children(b, cb, bfc, false, false)
            }
            Formatting::None => 0.0,
        }
    }

    /// Lay out block-level children; returns the content height.
    fn layout_block_children(&mut self, b: &mut LayoutBox, cb: &Cb, bfc: &mut Bfc, top_collapsible: bool, bottom_collapsible: bool) -> f32 {
        let content_y = b.content_rect.y;
        let content_x = b.content_rect.x;
        let mut cursor = content_y;
        let mut prev_bottom: Option<f32> = None;
        let mut last_in_flow: Option<usize> = None;
        let mb = b.margin.bottom;
        for i in 0..b.children.len() {
            match b.children[i].flow {
                Flow::Absolute => {
                    b.children[i].static_pos = (content_x, cursor + prev_bottom.unwrap_or(0.0).max(0.0));
                }
                Flow::FloatLeft | Flow::FloatRight => {
                    let y = cursor + prev_bottom.unwrap_or(0.0).max(0.0);
                    self.layout_float(&mut b.children[i], cb, y, bfc);
                }
                Flow::InFlow => {
                    let eff_top = self.effective_top_margin(&b.children[i], cb.width);
                    let top = match prev_bottom {
                        None => {
                            if top_collapsible {
                                content_y
                            } else {
                                content_y + eff_top
                            }
                        }
                        Some(pb) => cursor + collapse(pb, eff_top),
                    };
                    self.layout_block(&mut b.children[i], cb, top, bfc, WidthMode::Fill);
                    let child = &b.children[i];
                    cursor = child.rect.bottom();
                    prev_bottom = Some(child.collapsed_bottom_margin);
                    last_in_flow = Some(i);
                }
            }
        }
        match (last_in_flow, prev_bottom) {
            (Some(_), Some(pb)) => {
                if bottom_collapsible {
                    b.collapsed_bottom_margin = collapse(mb, pb);
                    cursor - content_y
                } else {
                    b.collapsed_bottom_margin = mb;
                    (cursor + pb - content_y).max(0.0)
                }
            }
            _ => {
                b.collapsed_bottom_margin = mb;
                0.0
            }
        }
    }

    /// CSS 2.1 §10.3 width resolution for non-replaced boxes. Returns
    /// (content width, margin-left, margin-right).
    fn compute_width(&mut self, b: &mut LayoutBox, cbw: f32, pb_h: f32, ml: Option<f32>, mr: Option<f32>, mode: WidthMode) -> (f32, f32, f32) {
        let style = b.style.clone();
        let mut specified = style.width.resolve(cbw).map(|w| {
            let w = if style.box_sizing == BoxSizing::BorderBox { w - pb_h } else { w };
            self.clamp_width(&style, w.max(0.0), cbw, pb_h)
        });
        // Intrinsic sizing keywords.
        if style.width.is_intrinsic() {
            let (min, max) = self.intrinsic(b);
            let ml0 = ml.unwrap_or(0.0);
            let mr0 = mr.unwrap_or(0.0);
            let avail = (cbw - ml0 - mr0 - pb_h).max(0.0);
            let w = match style.width {
                Length::MinContent => min,
                Length::MaxContent => max,
                _ => shrink_to_fit(min, max, avail),
            };
            specified = Some(self.clamp_width(&style, w, cbw, pb_h));
        } else if style.width.is_auto() && b.kind != BoxKind::Image {
            // `aspect-ratio` with a definite height and auto width.
            if let (Some(ratio), Length::Px(h)) = (style.aspect_ratio.filter(|r| *r > 0.0), style.height) {
                let w = if style.box_sizing == BoxSizing::BorderBox { h * ratio - pb_h } else { h * ratio };
                specified = Some(self.clamp_width(&style, w.max(0.0), cbw, pb_h));
            }
        }
        match mode {
            WidthMode::Fixed(w) => (self.clamp_width(&style, w.max(0.0), cbw, pb_h), ml.unwrap_or(0.0), mr.unwrap_or(0.0)),
            WidthMode::ShrinkToFit => {
                let (ml, mr) = (ml.unwrap_or(0.0), mr.unwrap_or(0.0));
                let w = match specified {
                    Some(w) => w,
                    None => {
                        let (min, max) = self.intrinsic(b);
                        let avail = (cbw - ml - mr - pb_h).max(0.0);
                        self.clamp_width(&style, shrink_to_fit(min, max, avail), cbw, pb_h)
                    }
                };
                (w, ml, mr)
            }
            WidthMode::Fill => match specified {
                None => {
                    let (ml, mr) = (ml.unwrap_or(0.0), mr.unwrap_or(0.0));
                    let w = (cbw - ml - mr - pb_h).max(0.0);
                    let w = self.clamp_width(&style, w, cbw, pb_h);
                    (w, ml, mr)
                }
                Some(w) => {
                    let rem = cbw - w - pb_h;
                    match (ml, mr) {
                        (None, None) => {
                            if rem > 0.0 {
                                (w, rem / 2.0, rem / 2.0)
                            } else {
                                (w, 0.0, rem)
                            }
                        }
                        (None, Some(r)) => (w, (rem - r).max(0.0), r),
                        (Some(l), None) => (w, l, rem - l),
                        (Some(l), Some(_)) => (w, l, rem - l),
                    }
                }
            },
        }
    }

    pub(crate) fn clamp_width(&self, style: &ComputedStyle, w: f32, cbw: f32, pb_h: f32) -> f32 {
        let adj = |v: f32| if style.box_sizing == BoxSizing::BorderBox { (v - pb_h).max(0.0) } else { v };
        let mut w = w;
        if let Some(max) = style.max_width.resolve(cbw) {
            w = w.min(adj(max));
        }
        if let Some(min) = style.min_width.resolve(cbw) {
            w = w.max(adj(min));
        }
        w.max(0.0)
    }

    fn resolve_height(&self, style: &ComputedStyle, cb_h: Option<f32>, pb_v: f32) -> Option<f32> {
        let adj = |v: f32| if style.box_sizing == BoxSizing::BorderBox { (v - pb_v).max(0.0) } else { v };
        match style.height {
            Length::Px(v) => Some(adj(v.max(0.0))),
            Length::Percent(p) => cb_h.map(|h| adj(h * p / 100.0)),
            Length::Auto | Length::MinContent | Length::MaxContent | Length::FitContent => None,
        }
    }

    fn clamp_height(&self, style: &ComputedStyle, h: f32, cb_h: Option<f32>, pb_v: f32) -> f32 {
        let adj = |v: f32| if style.box_sizing == BoxSizing::BorderBox { (v - pb_v).max(0.0) } else { v };
        let mut h = h;
        let max = match style.max_height {
            Length::Px(v) => Some(v),
            Length::Percent(p) => cb_h.map(|b| b * p / 100.0),
            Length::Auto | Length::MinContent | Length::MaxContent | Length::FitContent => None,
        };
        if let Some(m) = max {
            h = h.min(adj(m));
        }
        let min = match style.min_height {
            Length::Px(v) => Some(v),
            Length::Percent(p) => cb_h.map(|b| b * p / 100.0),
            Length::Auto | Length::MinContent | Length::MaxContent | Length::FitContent => None,
        };
        if let Some(m) = min {
            h = h.max(adj(m));
        }
        h.max(0.0)
    }

    /// Size of a replaced box's content area. `cbw == None` means
    /// percentages are treated as auto (intrinsic sizing).
    pub(crate) fn replaced_size(&self, b: &LayoutBox, cbw: Option<f32>, cb_h: Option<f32>, pb_h: f32) -> (f32, f32) {
        let style = &b.style;
        let (iw, ih) = match &b.image {
            Some(img) => (img.width as f32, img.height as f32),
            None => (b.attr_size.0.unwrap_or(0.0), b.attr_size.1.unwrap_or(0.0)),
        };
        let ratio = if ih > 0.0 && iw > 0.0 { Some(iw / ih) } else { None };
        let adj_w = |v: f32| if style.box_sizing == BoxSizing::BorderBox { (v - pb_h).max(0.0) } else { v };
        let sw = match style.width {
            Length::Px(v) => Some(adj_w(v)),
            Length::Percent(p) => cbw.map(|c| adj_w(c * p / 100.0)),
            Length::Auto | Length::MinContent | Length::MaxContent | Length::FitContent => b.attr_size.0.filter(|_| b.image.is_none() || true),
        };
        let sh = match style.height {
            Length::Px(v) => Some(v),
            Length::Percent(p) => cb_h.map(|c| c * p / 100.0),
            Length::Auto | Length::MinContent | Length::MaxContent | Length::FitContent => b.attr_size.1,
        };
        let (mut w, mut h) = match (sw, sh) {
            (Some(w), Some(h)) => (w, h),
            (Some(w), None) => (w, ratio.map(|r| w / r).unwrap_or(ih)),
            (None, Some(h)) => (ratio.map(|r| h * r).unwrap_or(iw), h),
            (None, None) => (iw, ih),
        };
        let cbw_v = cbw.unwrap_or(0.0);
        if let Some(max) = match style.max_width {
            Length::Px(v) => Some(adj_w(v)),
            Length::Percent(p) => cbw.map(|c| adj_w(c * p / 100.0)),
            Length::Auto | Length::MinContent | Length::MaxContent | Length::FitContent => None,
        } {
            if w > max {
                if w > 0.0 {
                    h *= max / w;
                }
                w = max;
            }
        }
        if let Some(min) = style.min_width.resolve(cbw_v) {
            if w < min && cbw.is_some() {
                if w > 0.0 {
                    h *= min / w;
                }
                w = min;
            }
        }
        if let Length::Px(max_h) = style.max_height {
            if h > max_h {
                if h > 0.0 {
                    w *= max_h / h;
                }
                h = max_h;
            }
        }
        (w.max(0.0), h.max(0.0))
    }

    /// Lay out a float child and place it in the BFC.
    pub(crate) fn layout_float(&mut self, child: &mut LayoutBox, cb: &Cb, y: f32, bfc: &mut Bfc) {
        let y = if child.style.clear != Clear::None { y.max(bfc.clear_y(child.style.clear)) } else { y };
        let mut inner = Bfc::new();
        self.layout_block(child, cb, y, &mut inner, WidthMode::ShrinkToFit);
        let m = child.margin;
        let w = child.rect.w + m.horizontal();
        let h = child.rect.h + m.vertical();
        let left = child.flow == Flow::FloatLeft;
        let (fx, fy) = bfc.place(left, w, h, y, cb.x, cb.width);
        let dx = fx + m.left - child.rect.x;
        let dy = fy + m.top - child.rect.y;
        child.translate(dx, dy);
    }

    /// Lay out an atomic inline-level box (inline-block / image) at a
    /// provisional origin; the caller translates it into place.
    pub(crate) fn layout_atomic(&mut self, child: &mut LayoutBox, avail_w: f32, cb_h: Option<f32>) {
        let cb = Cb {
            x: 0.0,
            width: avail_w,
            height: cb_h,
        };
        let full_width = child.attr_size.0 == Some(-1.0) && child.kind != BoxKind::Image;
        let mode = if child.kind == BoxKind::Image {
            WidthMode::Fill
        } else if full_width {
            let m = resolve_edges(&child.style.margin, avail_w);
            WidthMode::Fixed((avail_w - m.left - m.right - horizontal_extras(&child.style, avail_w)).max(0.0))
        } else {
            WidthMode::ShrinkToFit
        };
        let mut inner = Bfc::new();
        self.layout_block(child, &cb, 0.0, &mut inner, mode);
    }

    /// Lay out the absolutely positioned descendants whose containing block
    /// is `b` (padding box `cb`).
    pub(crate) fn layout_abs_descendants(&mut self, b: &mut LayoutBox, cb: Rect) {
        for i in 0..b.children.len() {
            if b.children[i].flow == Flow::Absolute {
                self.layout_absolute(&mut b.children[i], cb);
            } else if b.children[i].style.position == Position::Static {
                self.layout_abs_descendants(&mut b.children[i], cb);
            }
        }
    }

    fn layout_absolute(&mut self, c: &mut LayoutBox, cb: Rect) {
        let cb = if c.style.position == Position::Fixed {
            Rect::new(0.0, 0.0, self.ctx.viewport_width, self.ctx.viewport_height)
        } else {
            cb
        };
        let style = c.style.clone();
        let l = style.inset.left.resolve(cb.w);
        let r = style.inset.right.resolve(cb.w);
        let t = style.inset.top.resolve(cb.h);
        let btm = style.inset.bottom.resolve(cb.h);
        let ml = style.margin.left.resolve_or_zero(cb.w);
        let mr = style.margin.right.resolve_or_zero(cb.w);
        let mt = style.margin.top.resolve_or_zero(cb.w);
        let mb = style.margin.bottom.resolve_or_zero(cb.w);
        let pb_h = horizontal_extras(&style, cb.w);
        let (mode, avail) = match (l, r, style.width.is_auto()) {
            (Some(l), Some(r), true) => (WidthMode::Fixed((cb.w - l - r - ml - mr - pb_h).max(0.0)), cb.w),
            _ => {
                let used = l.or(r).unwrap_or(0.0);
                (WidthMode::ShrinkToFit, (cb.w - used).max(0.0))
            }
        };
        let inner_cb = Cb {
            x: cb.x,
            width: avail,
            height: Some(cb.h),
        };
        let mut bfc = Bfc::new();
        self.layout_block(c, &inner_cb, cb.y, &mut bfc, mode);
        let x = if let Some(l) = l {
            cb.x + l + ml
        } else if let Some(r) = r {
            cb.right() - r - mr - c.rect.w
        } else {
            c.static_pos.0 + ml
        };
        let y = if let Some(t) = t {
            cb.y + t + mt
        } else if let Some(bt) = btm {
            cb.bottom() - bt - mb - c.rect.h
        } else {
            c.static_pos.1 + mt
        };
        c.translate(x - c.rect.x, y - c.rect.y);
    }

    /// Apply `position: relative` offsets after layout.
    fn apply_relative(&mut self, b: &mut LayoutBox, cbw: f32, cbh: f32) {
        let (iw, ih) = (b.content_rect.w, b.content_rect.h);
        for c in &mut b.children {
            self.apply_relative(c, iw, ih);
        }
        if b.style.position == Position::Relative {
            let s = &b.style;
            let dx = s
                .inset
                .left
                .resolve(cbw)
                .or_else(|| s.inset.right.resolve(cbw).map(|v| -v))
                .unwrap_or(0.0);
            let dy = s
                .inset
                .top
                .resolve(cbh)
                .or_else(|| s.inset.bottom.resolve(cbh).map(|v| -v))
                .unwrap_or(0.0);
            b.translate(dx, dy);
        }
    }

    /// Position an outside list marker next to the first line.
    fn place_marker(&mut self, b: &mut LayoutBox) {
        let text = match &b.marker_text {
            Some(t) => t.clone(),
            None => return,
        };
        if b.style.list_style_position == rabbithole_style::ListStylePosition::Inside && b.formatting == Formatting::Inline {
            return; // rendered as inline content
        }
        // `::marker` may override the marker's font and color.
        let style = b
            .node
            .and_then(|n| crate::pseudo_style(self.ctx.styles, n, rabbithole_css::PseudoElement::Marker))
            .unwrap_or_else(|| b.style.clone());
        let font = self.font_for(&style);
        let size = style.font_size;
        let width = self.measurer.width(font, size, &text);
        let baseline = b.first_baseline().unwrap_or_else(|| {
            let m = self.ctx.fonts.metrics(font, size);
            let lh = style.line_height_px();
            b.content_rect.y + (lh - (m.ascent + m.descent)) / 2.0 + m.ascent
        });
        let gap = size * 0.45;
        // Next to the first line (which may have been pushed by a float).
        let line_x = b.lines.first().map(|l| l.rect.x).unwrap_or(b.content_rect.x);
        b.marker = Some(Marker {
            text,
            font,
            size,
            color: style.color,
            x: line_x - width - gap,
            baseline,
            width,
        });
    }
}

/// Union of a box's border box, its fragments and its descendants
/// (respecting clips) — the area it occupies for scroll-size purposes.
pub(crate) fn content_bounds(b: &LayoutBox) -> Rect {
    let mut r = b.margin_rect();
    if b.flow == Flow::Absolute && b.fixed {
        return r;
    }
    if b.clips {
        return r;
    }
    for line in &b.lines {
        for f in &line.fragments {
            r = r.union(&f.rect);
        }
    }
    for c in &b.children {
        r = r.union(&content_bounds(c));
    }
    r
}

/// Max right / bottom extents of the tree (border boxes and lines).
fn bounds(b: &LayoutBox) -> (f32, f32) {
    if b.fixed && b.flow == Flow::Absolute {
        return (0.0, 0.0);
    }
    let mut r = b.rect.right();
    let mut btm = b.rect.bottom();
    if b.clips {
        return (r, btm);
    }
    for line in &b.lines {
        for f in &line.fragments {
            r = r.max(f.rect.right());
            btm = btm.max(f.rect.bottom());
        }
    }
    for c in &b.children {
        let (cr, cb) = bounds(c);
        r = r.max(cr);
        btm = btm.max(cb);
    }
    (r, btm)
}
