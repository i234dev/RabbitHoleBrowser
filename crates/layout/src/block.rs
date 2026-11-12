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
    /// Containing blocks of absolutely positioned grid children placed by
    /// grid lines (their grid area), set by the grid layout.
    pub(crate) abs_cb: HashMap<rabbithole_dom::NodeId, Rect>,
    /// The block children being laid out have a `-webkit-center` parent.
    center_blocks: bool,
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

/// A set of adjoining vertical margins being collapsed (CSS 2.1 §8.3.1):
/// the result is the largest positive margin plus the most negative one.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Margins {
    pos: f32,
    neg: f32,
}

impl Margins {
    pub fn of(m: f32) -> Margins {
        Margins::default().add(m)
    }

    pub fn from_pair((pos, neg): (f32, f32)) -> Margins {
        Margins { pos, neg }
    }

    pub fn pair(self) -> (f32, f32) {
        (self.pos, self.neg)
    }

    pub fn add(self, m: f32) -> Margins {
        Margins {
            pos: self.pos.max(m),
            neg: self.neg.min(m),
        }
    }

    pub fn join(self, o: Margins) -> Margins {
        Margins {
            pos: self.pos.max(o.pos),
            neg: self.neg.min(o.neg),
        }
    }

    pub fn resolve(self) -> f32 {
        self.pos + self.neg
    }
}

/// Does a length used for `height`/`min-height` resolve to zero or auto
/// (percentages against an indefinite containing block are auto)?
fn zero_or_auto_height(l: Length, cb_h: Option<f32>) -> bool {
    match l {
        Length::Px(v) => v == 0.0,
        Length::Percent(p) => p == 0.0 || cb_h.map(|h| h * p == 0.0).unwrap_or(true),
        Length::Calc(..) | Length::Math(_) => cb_h.map(|h| l.resolve_or_zero(h) == 0.0).unwrap_or(true),
        Length::Auto => true,
        Length::MinContent | Length::MaxContent | Length::FitContent => true,
    }
}

fn is_zero_len(l: Length) -> bool {
    match l {
        Length::Px(v) => v == 0.0,
        Length::Percent(p) => p == 0.0,
        _ => true,
    }
}

/// Would this inline content produce no line boxes (CSS 2.1 §9.4.2: lines
/// with no text, no preserved white space, no inline boxes with non-zero
/// margins/padding/borders and no other in-flow content are zero-height)?
fn inline_content_is_empty(b: &LayoutBox) -> bool {
    use crate::tree::InlineItem;
    use rabbithole_style::WhiteSpace;
    for it in &b.items {
        match it {
            InlineItem::Text { text, style, .. } => {
                let preserve = matches!(style.white_space, WhiteSpace::Pre | WhiteSpace::PreWrap | WhiteSpace::BreakSpaces);
                let preserve_newlines = style.white_space == WhiteSpace::PreLine;
                for ch in text.chars() {
                    let collapsible = matches!(ch, ' ' | '\t' | '\r' | '\x0c') && !preserve || ch == '\n' && !preserve && !preserve_newlines;
                    if !collapsible {
                        return false;
                    }
                }
            }
            InlineItem::Open { style, .. } => {
                let s = style;
                if s.border_width.left > 0.0
                    || s.border_width.right > 0.0
                    || !is_zero_len(s.padding.left)
                    || !is_zero_len(s.padding.right)
                    || !is_zero_len(s.margin.left)
                    || !is_zero_len(s.margin.right)
                {
                    return false;
                }
            }
            InlineItem::Close | InlineItem::Float(_) | InlineItem::Absolute(_) => {}
            InlineItem::Atomic(_) | InlineItem::Break => return false,
        }
    }
    true
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
            abs_cb: HashMap::new(),
            center_blocks: false,
        }
    }

    /// Form controls that show one line of text (and keep its height when
    /// empty).
    fn is_line_control(&self, n: rabbithole_dom::NodeId) -> bool {
        let doc = self.ctx.doc;
        match doc.tag(n) {
            Some("button") => true,
            Some("select") => doc.attr(n, "multiple").is_none() && doc.attr(n, "size").and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0) <= 1,
            Some("input") => !matches!(crate::build::input_type(doc, n).as_str(), "checkbox" | "radio" | "range" | "color" | "image" | "hidden"),
            _ => false,
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
        self.layout_document_with(None)
    }

    /// Lay out the document, taking unchanged boxes from `reuse`.
    pub(crate) fn layout_document_with(&mut self, reuse: Option<&crate::incremental::Reuse>) -> LayoutTree {
        let debug = crate::incremental::debug_timing();
        let t0 = std::time::Instant::now();
        let builder = Builder::new(self.ctx, reuse);
        let mut root = builder.build_root();
        let t_build = t0.elapsed();
        let uses_counters = builder.uses_counters();
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
        let t_layout = t0.elapsed();
        let mut doc_h = (root.rect.bottom() + root.collapsed_bottom_margin.max(0.0)).max(vp_h);
        let icb = Rect::new(0.0, 0.0, vp_w, doc_h);
        if root.style.position == Position::Static {
            self.layout_abs_descendants(&mut root, icb);
        }
        self.apply_relative(&mut root, vp_w, vp_h);
        let (max_r, max_b) = bounds(&root);
        doc_h = doc_h.max(max_b);
        if debug {
            let fresh = crate::incremental::BUILD_STATS.with(|c| c.replace(0));
            eprintln!("[layout] build {:?} ({fresh} fresh elements), block layout {:?}, abs+relative+bounds {:?}", t_build, t_layout - t_build, t0.elapsed() - t_layout);
        }
        LayoutTree {
            root,
            width: vp_w.max(max_r.min(vp_w * 4.0)),
            height: doc_h,
            viewport: (vp_w, vp_h),
            uses_counters,
        }
    }

    /// Top margin of a block after collapsing with its first descendants.
    pub fn effective_top_margin(&self, b: &LayoutBox, cbw: f32) -> f32 {
        self.top_margins(b, cbw, None).resolve()
    }

    /// Width of `b`'s content box for resolving its children's percentage
    /// margins ahead of layout (approximate: ignores floats and shrink-to-fit).
    fn inner_width_estimate(&self, b: &LayoutBox, cbw: f32) -> f32 {
        let inner_w = (cbw - horizontal_extras(&b.style, cbw) - b.style.margin.left.resolve_or_zero(cbw) - b.style.margin.right.resolve_or_zero(cbw)).max(0.0);
        b.style.width.resolve(cbw).unwrap_or(inner_w)
    }

    /// Definite content height of `b` for its children's percentages.
    fn inner_height_estimate(&self, b: &LayoutBox, cb_h: Option<f32>) -> Option<f32> {
        match b.style.height {
            Length::Px(v) => Some(v),
            Length::Percent(p) => cb_h.map(|h| h * p / 100.0),
            l @ (Length::Calc(..) | Length::Math(_)) => cb_h.map(|h| l.resolve_or_zero(h)),
            _ => None,
        }
    }

    /// The margins adjoining `b`'s top margin from the inside: its own top
    /// margin, its first in-flow child's (recursively, while no border,
    /// padding or BFC boundary separates them) and the margins of empty
    /// blocks collapsing through on the way (CSS 2.1 §8.3.1).
    pub(crate) fn top_margins(&self, b: &LayoutBox, cbw: f32, cb_h: Option<f32>) -> Margins {
        if self.collapses_through(b, cb_h) {
            return self.all_margins(b, cbw, cb_h);
        }
        let m = Margins::of(b.style.margin.top.resolve_or_zero(cbw));
        if !b.flow.is_in_flow() || !self.top_collapsible(b, cbw) {
            return m;
        }
        let inner_w = self.inner_width_estimate(b, cbw);
        let inner_h = self.inner_height_estimate(b, cb_h);
        m.join(self.leading_child_margins(b, inner_w, inner_h))
    }

    /// Margins of `b`'s leading in-flow children that adjoin its content
    /// top: empty blocks collapsing through, then the first non-empty one's
    /// top margin chain.
    fn leading_child_margins(&self, b: &LayoutBox, inner_w: f32, inner_h: Option<f32>) -> Margins {
        let mut m = Margins::default();
        for c in b.children.iter().filter(|c| c.is_in_flow()) {
            if !matches!(c.kind, BoxKind::Block | BoxKind::AnonymousBlock | BoxKind::Table | BoxKind::Image) {
                break;
            }
            if self.collapses_through(c, inner_h) {
                if c.style.clear != Clear::None {
                    break;
                }
                m = m.join(self.all_margins(c, inner_w, inner_h));
                continue;
            }
            m = m.join(self.top_margins(c, inner_w, inner_h));
            break;
        }
        m
    }

    /// All margins of a box whose margins collapse through it, including its
    /// (empty) descendants'.
    fn all_margins(&self, b: &LayoutBox, cbw: f32, cb_h: Option<f32>) -> Margins {
        let mut m = Margins::of(b.style.margin.top.resolve_or_zero(cbw)).add(b.style.margin.bottom.resolve_or_zero(cbw));
        if b.formatting == Formatting::Block {
            let inner_w = self.inner_width_estimate(b, cbw);
            let inner_h = self.inner_height_estimate(b, cb_h);
            for c in b.children.iter().filter(|c| c.is_in_flow()) {
                m = m.join(self.all_margins(c, inner_w, inner_h));
            }
        }
        m
    }

    /// Is `b` an empty block whose top and bottom margins are adjoining
    /// (zero/auto height, zero min-height, no borders/padding vertically,
    /// no line boxes and only such children; not a BFC root)?
    pub(crate) fn collapses_through(&self, b: &LayoutBox, cb_h: Option<f32>) -> bool {
        if !b.flow.is_in_flow() || b.bfc_root || !matches!(b.kind, BoxKind::Block | BoxKind::AnonymousBlock) {
            return false;
        }
        let s = &b.style;
        if !zero_or_auto_height(s.height, cb_h) || !zero_or_auto_height(s.min_height, cb_h) {
            return false;
        }
        if s.border_width.top > 0.0 || s.border_width.bottom > 0.0 || !is_zero_len(s.padding.top) || !is_zero_len(s.padding.bottom) {
            return false;
        }
        match b.formatting {
            Formatting::Inline => inline_content_is_empty(b),
            Formatting::Block => {
                let inner_h = self.inner_height_estimate(b, cb_h);
                b.children.iter().filter(|c| c.is_in_flow()).all(|c| self.collapses_through(c, inner_h))
            }
            _ => false,
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
        // Where the contents' origin is now (for reuse by translation): the
        // current content box (it moved together with the contents, relative
        // offsets included) plus the scroll offset applied to the contents.
        let prev_cache = b.layout_cache.take();
        let prev_origin = (b.content_rect.x + b.scroll_offset.0, b.content_rect.y + b.scroll_offset.1);
        b.rel_applied = (0.0, 0.0);
        b.scroll_offset = (0.0, 0.0);
        if b.formatting == Formatting::Table && b.style.border_collapse {
            // Collapsed borders change the table's (and cells') border widths.
            self.resolve_collapsed_borders(b);
        }
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
        if b.formatting == Formatting::Table && matches!(mode, WidthMode::Fill) && style.width.is_auto() {
            mode = WidthMode::ShrinkToFit;
        }
        let (mut content_w, ml, mr, replaced_h) = if b.formatting == Formatting::TableWrapper {
            // The wrapper is as wide as its table; auto margins center it.
            let avail = cbw - ml.unwrap_or(0.0) - mr.unwrap_or(0.0);
            let w = self.wrapped_table_width(b, avail, mode);
            let rem = cbw - w;
            let (l, r) = match (mode, ml, mr) {
                (WidthMode::Fill, None, None) if rem > 0.0 => (rem / 2.0, rem / 2.0),
                (WidthMode::Fill, None, Some(r)) => ((rem - r).max(0.0), r),
                (WidthMode::Fill, Some(l), _) => (l, rem - l),
                (_, l, r) => (l.unwrap_or(0.0), r.unwrap_or(0.0)),
            };
            (w, l, r, None)
        } else if b.kind == BoxKind::Image {
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
        // A `<textarea>` with an auto height is `rows` lines tall (2 by
        // default), whatever its value.
        if explicit_h.is_none() && b.pseudo.is_none() {
            if let Some(n) = b.node.filter(|&n| self.ctx.doc.is_element(n, "textarea")) {
                let rows = self.ctx.doc.attr(n, "rows").and_then(|r| r.trim().parse::<f32>().ok()).filter(|r| *r >= 1.0).unwrap_or(2.0);
                explicit_h = Some(rows.floor() * style.line_height_px());
            }
        }
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
        b.bottom_margins = Margins::of(mb).pair();

        let mut local_bfc = Bfc::new();
        let content_y = b.content_rect.y;
        let origin = (b.content_rect.x, content_y);
        // Floats of the parent BFC that can reach into this box (relative to
        // its content origin): the contents can be reused only next to the
        // same floats.
        let float_env = if b.bfc_root { Vec::new() } else { bfc.relative_from(origin, content_y) };
        // Table cells are adjusted after layout (vertical-align), so their
        // contents are not where the cache expects them.
        let cacheable = b.kind != BoxKind::TableCell;
        let had_cache = prev_cache.is_some();
        let reuse = prev_cache.filter(|c| {
            cacheable
                && c.inner_w == inner_cb.width
                && c.inner_h == inner_cb.height
                && c.top_collapsible == top_collapsible
                && c.bottom_collapsible == bottom_collapsible
                && crate::float::same_floats(&c.float_env, &float_env)
        });
        crate::incremental::count(reuse.is_some(), had_cache);
        b.reused_layout = reuse.is_some();
        let (content_h, float_extent) = if let Some(c) = reuse {
            // Same constraints as last time: move the old contents.
            crate::table::translate_contents(b, b.content_rect.x - prev_origin.0, content_y - prev_origin.1);
            b.collapsed_bottom_margin = c.collapsed_bottom_margin;
            b.bottom_margins = c.bottom_margins;
            // Floats inside that belong to the parent BFC are placed again.
            bfc.extend_relative(origin, &c.escaped);
            let out = (c.content_h, c.float_extent);
            b.layout_cache = Some(c);
            out
        } else {
            let floats_before = bfc.len();
            let content_h = if b.bfc_root {
                self.layout_contents(b, &inner_cb, &mut local_bfc, top_collapsible, bottom_collapsible)
            } else {
                self.layout_contents(b, &inner_cb, bfc, top_collapsible, bottom_collapsible)
            };
            let float_extent = if b.bfc_root && !local_bfc.is_empty() { Some(local_bfc.max_bottom() - content_y) } else { None };
            b.layout_cache = cacheable.then(|| crate::incremental::LayoutCache {
                inner_w: inner_cb.width,
                inner_h: inner_cb.height,
                top_collapsible,
                bottom_collapsible,
                content_h,
                collapsed_bottom_margin: b.collapsed_bottom_margin,
                bottom_margins: b.bottom_margins,
                float_extent,
                float_env,
                escaped: bfc.relative_since(floats_before, origin),
            });
            (content_h, float_extent)
        };

        // A table's (or cell's) height is a minimum: content may exceed it.
        let mut h = match explicit_h {
            Some(e) if matches!(b.formatting, Formatting::Table) || b.kind == BoxKind::TableCell => e.max(content_h),
            Some(e) => e,
            // Size containment: laid out as if empty.
            None if style.contain_size && b.kind != BoxKind::Inline => 0.0,
            None => content_h,
        };
        if explicit_h.is_none() {
            if let Some(fe) = float_extent {
                h = h.max(fe);
            }
            // Empty text fields, buttons and menus are one line tall.
            if b.pseudo.is_none() && h < 0.5 && b.node.is_some_and(|n| self.is_line_control(n)) {
                let font = self.font_for(&style);
                h = style.line_height.unwrap_or_else(|| self.ctx.fonts.metrics(font, style.font_size).height());
            }
        }
        h = self.clamp_height(&style, h, cb.height, pb_v);
        b.rect.h = h + pb_v;
        b.content_rect.h = h;
        // An `<input>` shows its one line of text centered vertically in
        // its content box, whatever its height (Material fields with
        // `height: 28px; padding: 13px 15px`, short boxes with a large
        // line-height, ...). Reused contents were centered already.
        if !b.reused_layout && b.pseudo.is_none() && (h - content_h).abs() > 0.01 {
            // Buttons and drop-down menus center their contents too (when
            // they fit).
            let center = match b.node.and_then(|n| self.ctx.doc.tag(n)) {
                Some("input") => !b.lines.is_empty(),
                Some("select") => !b.lines.is_empty() && h > content_h,
                Some("button") => h > content_h && style.display != rabbithole_style::Display::Flex && style.display != rabbithole_style::Display::Grid,
                _ => false,
            };
            if center {
                crate::table::translate_contents(b, 0.0, (h - content_h) / 2.0);
            } else if h > content_h && matches!(b.formatting, Formatting::Block | Formatting::Inline) && b.kind != BoxKind::TableCell {
                // `align-content` on a block container (CSS Align §5.4):
                // the contents as one alignment subject.
                let shift = match style.align_content {
                    rabbithole_style::ContentDistribution::Center | rabbithole_style::ContentDistribution::SpaceAround | rabbithole_style::ContentDistribution::SpaceEvenly => (h - content_h) / 2.0,
                    rabbithole_style::ContentDistribution::FlexEnd => h - content_h,
                    _ => 0.0,
                };
                if shift > 0.0 {
                    crate::table::translate_contents(b, 0.0, shift);
                }
            }
        }
        if explicit_h.is_some() || (h - content_h).abs() > 0.01 {
            // A definite or clamped height separates the last child's
            // bottom margin from ours.
            b.collapsed_bottom_margin = mb;
            b.bottom_margins = Margins::of(mb).pair();
        }

        self.place_marker(b);

        // Single-line form controls clip their contents internally but keep
        // the baseline of their text (Blink, Gecko).
        let control_baseline = b.pseudo.is_none() && b.node.is_some_and(|n| matches!(self.ctx.doc.tag(n), Some("input" | "select" | "button")));
        b.baseline = if b.clips && !control_baseline {
            None
        } else {
            b.last_baseline().map(|bl| bl - b.rect.y)
        };
        // An empty one-line control has the baseline its text would have.
        if control_baseline && b.lines.iter().all(|l| l.fragments.is_empty()) && b.node.is_some_and(|n| self.is_line_control(n)) {
            let font = self.font_for(&style);
            let m = self.ctx.fonts.metrics(font, style.font_size);
            let lh = style.line_height.unwrap_or_else(|| m.height());
            let half_leading = (lh - (m.ascent + m.descent)) / 2.0;
            let top = b.content_rect.y + (b.content_rect.h - lh) / 2.0;
            b.baseline = Some(top + half_leading + m.ascent - b.rect.y);
        }

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
            Formatting::TableWrapper => self.layout_table_wrapper(b, cb),
            Formatting::TableRow => {
                // A row outside a table: lay out cells as blocks.
                self.layout_block_children(b, cb, bfc, false, false)
            }
            Formatting::None => 0.0,
        }
    }

    /// Lay out block-level children; returns the content height.
    ///
    /// Vertical margins collapse per CSS 2.1 §8.3.1: `pending` holds the
    /// margins adjoining the current position (the previous sibling's
    /// collapsed bottom margin plus those of empty blocks in between). While
    /// `at_top`, the parent's top margin chain is still open — those margins
    /// were already applied when the parent was positioned.
    fn layout_block_children(&mut self, b: &mut LayoutBox, cb: &Cb, bfc: &mut Bfc, top_collapsible: bool, bottom_collapsible: bool) -> f32 {
        let saved = std::mem::replace(&mut self.center_blocks, b.style.text_align_webkit_center);
        let h = self.layout_block_children_inner(b, cb, bfc, top_collapsible, bottom_collapsible);
        self.center_blocks = saved;
        h
    }

    fn layout_block_children_inner(&mut self, b: &mut LayoutBox, cb: &Cb, bfc: &mut Bfc, top_collapsible: bool, bottom_collapsible: bool) -> f32 {
        let content_y = b.content_rect.y;
        let content_x = b.content_rect.x;
        let mut cursor = content_y;
        let mut pending = Margins::default();
        let mut at_top = top_collapsible;
        let mb = b.margin.bottom;
        for i in 0..b.children.len() {
            match b.children[i].flow {
                Flow::Absolute => {
                    let off = if at_top { 0.0 } else { pending.resolve() };
                    b.children[i].static_pos = (content_x, cursor + off);
                }
                Flow::FloatLeft | Flow::FloatRight => {
                    let off = if at_top { 0.0 } else { pending.resolve() };
                    self.layout_float(&mut b.children[i], cb, cursor + off, bfc);
                }
                Flow::InFlow => {
                    let empty = self.collapses_through(&b.children[i], cb.height);
                    let child_top = self.top_margins(&b.children[i], cb.width, cb.height);
                    // Hypothetical position of the border edge (no clearance).
                    let mut top = if at_top { content_y } else { cursor + pending.join(child_top).resolve() };
                    let mut clearance = false;
                    let clear = b.children[i].style.clear;
                    if clear != Clear::None {
                        let cy = bfc.clear_y(clear);
                        if cy > top + 0.001 {
                            // Clearance (CSS 2.1 §9.5.2): the border edge goes
                            // just below the floats; margins above no longer
                            // collapse with the cleared box's top margin.
                            top = cy;
                            clearance = true;
                        }
                    }
                    self.layout_block(&mut b.children[i], cb, top, bfc, WidthMode::Fill);
                    let child = &b.children[i];
                    if empty && !clearance && child.rect.h <= 0.001 {
                        // Margins collapse through the empty box.
                        if !at_top {
                            pending = pending.join(child_top);
                        }
                    } else {
                        at_top = false;
                        cursor = child.rect.bottom();
                        pending = if empty { Margins::of(child.margin.bottom) } else { Margins::from_pair(child.bottom_margins) };
                    }
                }
            }
        }
        if at_top {
            // No in-flow content separated the parent's margins.
            b.collapsed_bottom_margin = mb;
            b.bottom_margins = Margins::of(mb).pair();
            return 0.0;
        }
        if bottom_collapsible {
            let m = pending.add(mb);
            b.collapsed_bottom_margin = m.resolve();
            b.bottom_margins = m.pair();
            cursor - content_y
        } else {
            b.collapsed_bottom_margin = mb;
            b.bottom_margins = Margins::of(mb).pair();
            (cursor + pending.resolve() - content_y).max(0.0)
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
            WidthMode::Fill => match specified.or_else(|| {
                // Auto width fills the line; when min/max-width changes it,
                // the result is used as the specified width so `auto`
                // margins share the remaining space (CSS 2.1 §10.4).
                let w = (cbw - ml.unwrap_or(0.0) - mr.unwrap_or(0.0) - pb_h).max(0.0);
                let clamped = self.clamp_width(&style, w, cbw, pb_h);
                if (clamped - w).abs() > 0.01 {
                    Some(clamped)
                } else {
                    None
                }
            }) {
                None => {
                    let (ml, mr) = (ml.unwrap_or(0.0), mr.unwrap_or(0.0));
                    let w = (cbw - ml - mr - pb_h).max(0.0);
                    (w, ml, mr)
                }
                Some(w) => {
                    let rem = cbw - w - pb_h;
                    // `-webkit-center` parents center the margin box of
                    // children with fixed margins (Blink).
                    if let (Some(l), Some(r), true) = (ml, mr, self.center_blocks) {
                        let start = ((rem - l - r) / 2.0).max(0.0) + l;
                        return (w, start, rem - start);
                    }
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
                        (Some(_), Some(r)) if style.direction == rabbithole_style::Direction::Rtl => (w, rem - r, r),
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
            l @ (Length::Calc(..) | Length::Math(_)) => cb_h.map(|h| adj(l.resolve_or_zero(h).max(0.0))),
            Length::Auto | Length::MinContent | Length::MaxContent | Length::FitContent => None,
        }
    }

    fn clamp_height(&self, style: &ComputedStyle, h: f32, cb_h: Option<f32>, pb_v: f32) -> f32 {
        let adj = |v: f32| if style.box_sizing == BoxSizing::BorderBox { (v - pb_v).max(0.0) } else { v };
        let mut h = h;
        let max = match style.max_height {
            Length::Px(v) => Some(v),
            Length::Percent(p) => cb_h.map(|b| b * p / 100.0),
            l @ (Length::Calc(..) | Length::Math(_)) => cb_h.map(|b| l.resolve_or_zero(b)),
            Length::Auto | Length::MinContent | Length::MaxContent | Length::FitContent => None,
        };
        if let Some(m) = max {
            h = h.min(adj(m));
        }
        let min = match style.min_height {
            Length::Px(v) => Some(v),
            Length::Percent(p) => cb_h.map(|b| b * p / 100.0),
            l @ (Length::Calc(..) | Length::Math(_)) => cb_h.map(|b| l.resolve_or_zero(b)),
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
        // Natural size: the decoded image (in CSS px), else the `width` /
        // `height` attributes (which also give the aspect ratio before the
        // image arrives, so loading causes no layout shift).
        let (iw, ih) = match b.image.as_ref().filter(|i| !i.broken) {
            Some(img) => img.natural_size(),
            None => (b.attr_size.0.unwrap_or(0.0), b.attr_size.1.unwrap_or(0.0)),
        };
        let ratio = style.aspect_ratio.filter(|r| *r > 0.0 && r.is_finite()).or(if ih > 0.0 && iw > 0.0 { Some(iw / ih) } else { None });
        let adj_w = |v: f32| if style.box_sizing == BoxSizing::BorderBox { (v - pb_h).max(0.0) } else { v };
        // `width`/`height` attributes arrive as presentational hints in the
        // computed style; an explicit `auto` (e.g. `height: auto`) must
        // keep the ratio instead of falling back to the attribute.
        let sw = match style.width {
            Length::Px(v) => Some(adj_w(v)),
            Length::Percent(p) => cbw.map(|c| adj_w(c * p / 100.0)),
            l @ (Length::Calc(..) | Length::Math(_)) => cbw.map(|c| adj_w(l.resolve_or_zero(c))),
            Length::Auto | Length::MinContent | Length::MaxContent | Length::FitContent => None,
        };
        let sh = match style.height {
            Length::Px(v) => Some(v),
            Length::Percent(p) => cb_h.map(|c| c * p / 100.0),
            l @ (Length::Calc(..) | Length::Math(_)) => cb_h.map(|c| l.resolve_or_zero(c)),
            Length::Auto | Length::MinContent | Length::MaxContent | Length::FitContent => None,
        };
        let (iw, ih) = match (iw > 0.0, ih > 0.0, ratio) {
            (true, false, Some(r)) => (iw, iw / r),
            (false, true, Some(r)) => (ih * r, ih),
            _ => (iw, ih),
        };
        // An inline `<svg>` without width/height has only an intrinsic
        // ratio: its auto width fills the containing block (300px when that
        // is unknown, e.g. for intrinsic sizing); the height follows the
        // ratio. (Its raster's pixel size is not its CSS size.)
        let sw = match (sw, sh) {
            (None, None) if b.ratio_only => Some(cbw.map(|c| (c - pb_h).max(0.0)).unwrap_or(300.0)),
            _ => sw,
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
            l @ (Length::Calc(..) | Length::Math(_)) => cbw.map(|c| adj_w(l.resolve_or_zero(c))),
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
    /// Returns whether any such descendant exists.
    pub(crate) fn layout_abs_descendants(&mut self, b: &mut LayoutBox, cb: Rect) -> bool {
        let mut found = false;
        for i in 0..b.children.len() {
            if b.children[i].flow == Flow::Absolute {
                let area = b.children[i].node.and_then(|n| self.abs_cb.get(&n).copied());
                self.layout_absolute(&mut b.children[i], area.unwrap_or(cb));
                found = true;
            } else if b.children[i].style.position == Position::Static && !b.children[i].no_abs_below {
                // Kept boxes remember having none (their subtree is
                // unchanged), so large static subtrees are walked once.
                let f = self.layout_abs_descendants(&mut b.children[i], cb);
                b.children[i].no_abs_below = !f;
                found |= f;
            }
        }
        found
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
        // `top` and `bottom` with an auto height: the box fills the space
        // between them (CSS 2.1 §10.6.4; overlays, outlines, `inset: 0`).
        if let (Some(t), Some(bt), true) = (t, btm, style.height.is_auto()) {
            if c.kind != BoxKind::Image {
                let p = resolve_edges(&style.padding, cb.w);
                let pb_v = p.top + p.bottom + style.border_width.top + style.border_width.bottom;
                self.forced_content_height = Some((cb.h - t - bt - mt - mb - pb_v).max(0.0));
            }
        }
        let mut bfc = Bfc::new();
        self.layout_block(c, &inner_cb, cb.y, &mut bfc, mode);
        self.forced_content_height = None;
        // With both insets and a definite size, auto margins share the
        // remaining space (CSS 2.1 §10.3.7 / §10.6.4: centering with
        // `inset: 0; margin: auto`).
        let sized_w = !style.width.is_auto() || c.kind == BoxKind::Image;
        let sized_h = !style.height.is_auto() || c.kind == BoxKind::Image;
        let x = if let (Some(l), Some(r), true) = (l, r, sized_w) {
            let free = cb.w - l - r - c.rect.w;
            match (style.margin.left.is_auto(), style.margin.right.is_auto()) {
                (true, true) => cb.x + l + (free / 2.0).max(0.0),
                (true, false) => cb.x + l + free - mr,
                _ => cb.x + l + ml,
            }
        } else if let Some(l) = l {
            cb.x + l + ml
        } else if let Some(r) = r {
            cb.right() - r - mr - c.rect.w
        } else {
            c.static_pos.0 + ml
        };
        let y = if let (Some(t), Some(bt), true) = (t, btm, sized_h) {
            let free = cb.h - t - bt - c.rect.h;
            match (style.margin.top.is_auto(), style.margin.bottom.is_auto()) {
                (true, true) => cb.y + t + free / 2.0,
                (true, false) => cb.y + t + free - mb,
                _ => cb.y + t + mt,
            }
        } else if let Some(t) = t {
            cb.y + t + mt
        } else if let Some(bt) = btm {
            cb.bottom() - bt - mb - c.rect.h
        } else {
            c.static_pos.1 + mt
        };
        // The static position is where the box would have been in flow; it
        // must not move with the box (laying it out again would drift).
        let static_pos = c.static_pos;
        c.translate(x - c.rect.x, y - c.rect.y);
        c.static_pos = static_pos;
    }

    /// Apply `position: relative` offsets after layout.
    fn apply_relative(&mut self, b: &mut LayoutBox, cbw: f32, cbh: f32) {
        let (iw, ih) = (b.content_rect.w, b.content_rect.h);
        if !b.reused_layout {
            for c in &mut b.children {
                self.apply_relative(c, iw, ih);
            }
        }
        let mut want = (0.0, 0.0);
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
            want = (dx, dy);
        }
        // Boxes reused from the previous layout may carry their offset
        // already: apply only the difference.
        let (ax, ay) = b.rel_applied;
        b.translate(want.0 - ax, want.1 - ay);
        b.rel_applied = want;
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
