//! Vertical writing modes (CSS Writing Modes 3) by transposition.
//!
//! A box whose `writing-mode` is vertical while its parent's is not (or
//! the root element) starts a vertical subtree. The subtree is laid out by
//! the ordinary (horizontal) algorithms in its own *logical* coordinate
//! space — the inline axis along x, the block axis along y — with every
//! style transposed (physical `height` becomes the logical width, the
//! physical side at block-start becomes the logical top, ...). An
//! anonymous wrapper box takes the subtree's place in the physical tree:
//! it is sized from the logical result (width = logical height) and maps
//! the subtree onto the page with a transform:
//!
//! - `vertical-rl`: a 90° clockwise rotation (block-start on the right);
//!   text set sideways is exactly the rotated horizontal text;
//! - `vertical-lr`: a reflection about the diagonal (block-start on the
//!   left); text and images are flipped back locally when painted.

use crate::block::{Cb, Layouter, Margins, WidthMode};
use crate::float::Bfc;
use crate::tree::{BoxKind, Flow, Formatting, InlineItem, LayoutBox, WmKind};
use rabbithole_gfx::{Matrix, Rect};
use rabbithole_style::{ComputedStyle, Display, Edges, Float, Length, Position, WritingMode};
use std::collections::HashMap;
use std::rc::Rc;

/// `style` transposed into the logical space of a `mode` subtree.
pub(crate) fn transposed_as(style: &ComputedStyle, mode: WritingMode) -> ComputedStyle {
    let mut s = style.clone();
    if style.layout_transposed || !mode.is_vertical() {
        return s;
    }
    let m = mode;
    fn map<T: Copy>(e: Edges<T>, m: WritingMode) -> Edges<T> {
        match m {
            WritingMode::VerticalRl => Edges { top: e.right, right: e.bottom, bottom: e.left, left: e.top },
            WritingMode::SidewaysLr => Edges { top: e.left, right: e.top, bottom: e.right, left: e.bottom },
            _ => Edges { top: e.left, right: e.bottom, bottom: e.right, left: e.top },
        }
    }
    s.margin = map(style.margin, m);
    s.padding = map(style.padding, m);
    s.border_width = map(style.border_width, m);
    s.border_style = map(style.border_style, m);
    s.border_color = map(style.border_color, m);
    s.inset = map(style.inset, m);
    s.width = style.height;
    s.height = style.width;
    s.min_width = style.min_height;
    s.min_height = style.min_width;
    s.max_width = style.max_height;
    s.max_height = style.max_width;
    // aspect-ratio is width / height: inverted in logical space, and a
    // definite logical height gives the logical width.
    if let Some(r) = style.aspect_ratio.filter(|r| *r > 0.0) {
        s.aspect_ratio = Some(1.0 / r);
        if let (Length::Auto, Length::Px(h)) = (s.width, s.height) {
            s.width = Length::Px(h / r);
        }
    }
    s.overflow_x = style.overflow_y;
    s.overflow_y = style.overflow_x;
    // Corners: [top-left, top-right, bottom-right, bottom-left], (h, v).
    let r = style.border_radii;
    let sw = |c: (Length, Length)| (c.1, c.0);
    s.border_radii = match m {
        WritingMode::VerticalRl => [sw(r[1]), sw(r[2]), sw(r[3]), sw(r[0])],
        WritingMode::SidewaysLr => [sw(r[3]), sw(r[0]), sw(r[1]), sw(r[2])],
        _ => [sw(r[0]), sw(r[3]), sw(r[2]), sw(r[1])],
    };
    // Upright vertical text is typeset left to right: its used direction
    // is ltr (CSS Writing Modes 3 §5.1).
    if style.text_orientation == rabbithole_style::TextOrientation::Upright && !style.writing_sideways {
        s.direction = rabbithole_style::Direction::Ltr;
    }
    s.layout_transposed = true;
    s
}

#[derive(Default)]
struct Cache {
    map: HashMap<*const ComputedStyle, Rc<ComputedStyle>>,
}

impl Cache {
    /// `s` in the logical space of a `mode` subtree.
    fn get(&mut self, s: &Rc<ComputedStyle>, mode: WritingMode) -> Rc<ComputedStyle> {
        if s.layout_transposed || !mode.is_vertical() {
            return s.clone();
        }
        self.map.entry(Rc::as_ptr(s)).or_insert_with(|| Rc::new(transposed_as(s, mode))).clone()
    }
}

/// The text runs of `text` combined upright by `text-combine-upright`
/// value `tcu` (1: all; n + 1: up to n ASCII digits in a row), as
/// `(start, end, combined)` byte ranges covering the text.
fn combined_runs(text: &str, tcu: u8) -> Vec<(usize, usize, bool)> {
    if tcu == 1 {
        return vec![(0, text.len(), true)];
    }
    let max = (tcu - 1) as usize;
    let mut out = Vec::new();
    let b = text.as_bytes();
    let (mut i, mut last) = (0, 0);
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i - start <= max {
                if start > last {
                    out.push((last, start, false));
                }
                out.push((start, i, true));
                last = i;
            }
        } else {
            i += 1;
        }
    }
    if last < b.len() {
        out.push((last, b.len(), false));
    }
    out
}

#[cfg(test)]
pub(crate) fn combined_runs_for_test(text: &str, tcu: u8) -> Vec<(usize, usize, bool)> {
    combined_runs(text, tcu)
}

/// `text-combine-upright` (CSS Writing Modes 3 §9.1): each combined text
/// run of the vertical inline formatting context `b` becomes an atomic
/// inline holding the run set horizontally (its wrapper turns it upright
/// and squeezes it into 1em).
fn combine_upright(b: &mut LayoutBox, cache: &mut Cache, mode: WritingMode) {
    let mut items = Vec::with_capacity(b.items.len() + 2);
    let mut children: Vec<LayoutBox> = Vec::with_capacity(b.children.len() + 1);
    let mut old_children = std::mem::take(&mut b.children).into_iter().map(Some).collect::<Vec<_>>();
    let mut take = |i: usize, children: &mut Vec<LayoutBox>| {
        children.push(old_children[i].take().unwrap_or_else(|| LayoutBox::new(BoxKind::Block, None, b.style.clone())));
        children.len() - 1
    };
    for it in std::mem::take(&mut b.items) {
        match it {
            InlineItem::Text { text, style, node, elem } if style.text_combine_upright != 0 && !style.writing_sideways && style.writing_mode.is_vertical() && !style.layout_transposed => {
                for (s, e, combined) in combined_runs(&text, style.text_combine_upright) {
                    let run: Rc<str> = Rc::from(&text[s..e]);
                    if !combined {
                        items.push(InlineItem::Text { text: run, style: style.clone(), node, elem });
                        continue;
                    }
                    let em = style.font_size;
                    let mut st = ComputedStyle::inherit_from(&style);
                    st.writing_mode = WritingMode::HorizontalTb;
                    st.writing_sideways = false;
                    st.text_combine_upright = 0;
                    st.display = Display::InlineBlock;
                    st.display_before_blockification = Display::InlineBlock;
                    st.width = Length::Px(em);
                    st.line_height = Some(em);
                    st.text_align = rabbithole_style::TextAlign::Center;
                    st.text_indent = Length::ZERO;
                    // One line: white space collapses (or not) as usual.
                    st.white_space = match style.white_space {
                        rabbithole_style::WhiteSpace::Normal | rabbithole_style::WhiteSpace::PreLine => rabbithole_style::WhiteSpace::Nowrap,
                        rabbithole_style::WhiteSpace::PreWrap | rabbithole_style::WhiteSpace::BreakSpaces => rabbithole_style::WhiteSpace::Pre,
                        ws => ws,
                    };
                    st.unicode_bidi = style.unicode_bidi;
                    // (The composition is decorated as one glyph of the
                    // vertical line, not as horizontal text.)
                    st.applied_decorations = Vec::new();
                    st.text_orientation = rabbithole_style::TextOrientation::Mixed;
                    let st = Rc::new(st);
                    let mut inner = LayoutBox::new(BoxKind::InlineBlock, None, st.clone());
                    inner.formatting = Formatting::Inline;
                    inner.bfc_root = true;
                    inner.items.push(InlineItem::Text { text: run, style: st, node, elem });
                    let mut w = wrap(inner, cache, false, mode);
                    w.wm_tcy = true;
                    children.push(w);
                    items.push(InlineItem::Atomic(children.len() - 1));
                }
            }
            InlineItem::Atomic(i) => items.push(InlineItem::Atomic(take(i, &mut children))),
            InlineItem::Float(i) => items.push(InlineItem::Float(take(i, &mut children))),
            InlineItem::Absolute(i) => items.push(InlineItem::Absolute(take(i, &mut children))),
            other => items.push(other),
        }
    }
    // (Children no item refers to keep their place at the end.)
    children.extend(old_children.into_iter().flatten());
    b.items = items;
    b.children = children;
}

/// Transpose a vertical subtree in place (styles of boxes and inline items).
fn transpose_subtree(b: &mut LayoutBox, cache: &mut Cache, mode: WritingMode) {
    if b.items.iter().any(|it| matches!(it, InlineItem::Text { style, .. } if style.text_combine_upright != 0)) {
        combine_upright(b, cache, mode);
    }
    b.style = cache.get(&b.style, mode);
    for it in &mut b.items {
        match it {
            InlineItem::Text { style, .. } | InlineItem::Open { style, .. } => *style = cache.get(style, mode),
            _ => {}
        }
    }
    for i in 0..b.children.len() {
        let c = &b.children[i];
        if c.wm_wrapper {
            continue;
        }
        // A horizontal block inside the vertical subtree is an orthogonal
        // flow of its own.
        // Images stay upright: they are wrapped like horizontal blocks.
        let image = c.kind == BoxKind::Image && !c.style.layout_transposed;
        let orthogonal = image
            || (!c.style.writing_mode.is_vertical() && !c.style.layout_transposed && !matches!(c.kind, BoxKind::Text | BoxKind::LineBreak | BoxKind::Inline) && !table_internal(b, c));
        if orthogonal {
            let mut c = std::mem::replace(&mut b.children[i], LayoutBox::new(BoxKind::Block, None, b.style.clone()));
            if image {
                let mut st = (*c.style).clone();
                st.writing_mode = WritingMode::HorizontalTb;
                c.style = Rc::new(st);
            }
            b.children[i] = wrap(c, cache, false, mode);
        } else {
            transpose_subtree(&mut b.children[i], cache, mode);
        }
    }
}

/// Wrap the box `r` (physical style) whose writing mode differs from its
/// parent's (`outer`, the mode of the space the wrapper lives in).
fn wrap(mut r: LayoutBox, cache: &mut Cache, is_root: bool, outer: WritingMode) -> LayoutBox {
    let phys = r.style.clone();
    // The wrapper keeps the outer role: margins, floating, positioning.
    let mut ws = ComputedStyle::inherit_from(&phys);
    ws.writing_mode = WritingMode::HorizontalTb;
    ws.display = if phys.display.is_inline_level() { Display::InlineBlock } else { Display::Block };
    ws.display_before_blockification = phys.display_before_blockification;
    ws.position = phys.position;
    ws.inset = phys.inset;
    ws.z_index = phys.z_index;
    // The legacy `clip` of an absolutely positioned box: the wrapper is
    // the positioned box now.
    ws.clip_rect = phys.clip_rect;
    ws.float = phys.float;
    ws.clear = phys.clear;
    ws.margin = phys.margin;
    ws.width = phys.width;
    ws.height = phys.height;
    ws.min_width = phys.min_width;
    ws.max_width = phys.max_width;
    ws.min_height = phys.min_height;
    ws.max_height = phys.max_height;
    ws.box_sizing = phys.box_sizing;
    ws.vertical_align = phys.vertical_align;
    ws.order = phys.order;
    ws.flex_grow = phys.flex_grow;
    ws.flex_shrink = phys.flex_shrink;
    ws.flex_basis = phys.flex_basis;
    ws.flex_basis_content = phys.flex_basis_content;
    ws.align_self = phys.align_self;
    ws.justify_self = phys.justify_self;
    ws.align_self_flags = phys.align_self_flags;
    ws.justify_self_flags = phys.justify_self_flags;
    ws.grid_row_start = phys.grid_row_start.clone();
    ws.grid_row_end = phys.grid_row_end.clone();
    ws.grid_column_start = phys.grid_column_start.clone();
    ws.grid_column_end = phys.grid_column_end.clone();
    if is_root {
        // The canvas takes the root's background (or the body's).
        ws.background_color = phys.background_color;
        if ws.background_color.is_transparent() {
            if let Some(body) = r.children.iter().find(|c| c.is_in_flow()) {
                ws.background_color = body.style.background_color;
            }
        }
    }
    if outer.is_vertical() {
        // In a vertical subtree's logical space.
        ws = transposed_as(&ws, outer);
        ws.writing_mode = outer;
    }
    let mut w = LayoutBox::new(if ws.display == Display::InlineBlock { BoxKind::InlineBlock } else { BoxKind::Block }, None, Rc::new(ws));
    w.wm_kind = match (outer, phys.writing_mode) {
        (WritingMode::HorizontalTb, WritingMode::VerticalRl) => WmKind::Cw,
        (WritingMode::VerticalRl, WritingMode::HorizontalTb) => WmKind::Ccw,
        (WritingMode::HorizontalTb, WritingMode::SidewaysLr) => WmKind::Ccw,
        (WritingMode::SidewaysLr, WritingMode::HorizontalTb) => WmKind::Cw,
        _ => WmKind::Reflect,
    };
    w.flow = r.flow;
    w.fixed = r.fixed;
    w.bfc_root = true;
    w.formatting = Formatting::Block;
    w.wm_wrapper = true;
    w.built_from = r.built_from.clone();
    // The inner box: logical style without the outer role.
    if phys.writing_mode.is_vertical() {
        transpose_subtree(&mut r, cache, phys.writing_mode);
    } else {
        prepare_children(&mut r, cache);
    }
    let mut inner = (*r.style).clone();
    // Background images and gradients are painted by the wrapper in
    // physical space (they must not turn with the contents).
    if outer == WritingMode::HorizontalTb && (phys.background_image.is_some() || !phys.background_layers.is_empty()) {
        let mut bg = (*phys).clone();
        bg.background_color = rabbithole_gfx::Color::TRANSPARENT;
        w.wm_background = Some(Rc::new(bg));
        inner.background_image = None;
        inner.background_layers = Vec::new();
    }
    inner.margin = Edges::all(Length::ZERO);
    inner.clip_rect = None;
    inner.float = Float::None;
    inner.clear = rabbithole_style::Clear::None;
    if phys.position != Position::Static {
        // Still the containing block of its positioned descendants.
        inner.position = Position::Relative;
        inner.inset = Edges::all(Length::Auto);
    }
    if inner.display.is_inline_level() {
        inner.display = match inner.display {
            Display::InlineFlex => Display::Flex,
            Display::InlineGrid => Display::Grid,
            Display::InlineTable => Display::Table,
            _ => Display::FlowRoot,
        };
    }
    r.style = Rc::new(inner);
    r.kind = if r.kind == BoxKind::InlineBlock { BoxKind::Block } else { r.kind };
    r.flow = Flow::InFlow;
    r.fixed = false;
    r.bfc_root = true;
    w.children.push(r);
    w
}

/// Table rows, cells and the table inside its wrapper must stay where
/// they are (their parents lay them out as table parts).
fn table_internal(parent: &LayoutBox, b: &LayoutBox) -> bool {
    let table = b.kind == BoxKind::Table || b.formatting == Formatting::Table;
    // (A table box outside a table wrapper box is the table itself.)
    (table && parent.formatting == Formatting::TableWrapper) || matches!(b.kind, BoxKind::TableRow | BoxKind::TableCell) || b.formatting == Formatting::TableRow || crate::table::is_row_group(b)
}

/// Find the vertical subtrees below `b` (whose mode is `mode`) and wrap them.
fn prepare_children(b: &mut LayoutBox, cache: &mut Cache) {
    for i in 0..b.children.len() {
        let c = &b.children[i];
        if c.wm_wrapper {
            continue;
        }
        let vertical = c.style.writing_mode.is_vertical() && !c.style.layout_transposed;
        // Images are not turned; table parts cannot leave their table.
        let element = !matches!(c.kind, BoxKind::Text | BoxKind::LineBreak | BoxKind::Image) && !table_internal(b, c);
        if vertical && element {
            let c = std::mem::replace(&mut b.children[i], LayoutBox::new(BoxKind::Block, None, b.style.clone()));
            b.children[i] = wrap(c, cache, false, WritingMode::HorizontalTb);
        } else if !vertical {
            prepare_children(&mut b.children[i], cache);
        }
    }
}

/// Set up the vertical subtrees of a freshly built box tree; returns the
/// (possibly wrapped) root.
pub(crate) fn prepare(root: LayoutBox, body: Option<rabbithole_dom::NodeId>) -> LayoutBox {
    let mut cache = Cache::default();
    let mut root = root;
    // The principal writing mode comes from the body when there is one
    // (CSS Writing Modes 3 §8): the root is laid out in it.
    // Layout, paint or size containment on the root or the body stops it
    // (CSS Contain 1 §3).
    let contained = |s: &ComputedStyle| s.contain_layout || s.contain_paint || s.contain_size;
    if let Some(bs) = root.children.iter().find(|c| c.node.is_some() && c.node == body).map(|c| c.style.clone()) {
        if bs.writing_mode != root.style.writing_mode && !contained(&bs) && !contained(&root.style) && !root.style.layout_transposed && !root.wm_wrapper {
            let mut s = (*root.style).clone();
            s.writing_mode = bs.writing_mode;
            root.style = Rc::new(s);
        }
    }
    if root.style.writing_mode.is_vertical() && !root.style.layout_transposed && !root.wm_wrapper {
        return wrap(root, &mut cache, true, WritingMode::HorizontalTb);
    }
    prepare_children(&mut root, &mut cache);
    root
}

impl<'a> Layouter<'a> {
    /// Lay out a vertical subtree's wrapper `w` (physical, in a horizontal
    /// parent) at `top`: the inner box in its logical space, then the
    /// wrapper sized and mapped onto the page.
    pub(crate) fn layout_wm_wrapper(&mut self, w: &mut LayoutBox, cb: &Cb, top: f32, mode: WidthMode, forced_h: Option<f32>) {
        let phys = w.style.clone();
        let cbw = cb.width;
        let m = |l: Length| l.resolve(cbw).unwrap_or(0.0);
        let (ml, mr, mt, mb) = (m(phys.margin.left), m(phys.margin.right), m(phys.margin.top), m(phys.margin.bottom));
        let is_root = w.children.first().and_then(|r| r.node).is_some_and(|n| Some(n) == self.ctx.doc.html_element());
        let kind = w.wm_kind;
        let rl = kind == WmKind::Cw;
        let Some(r) = w.children.first_mut() else { return };
        // The available inline size is the wrapper's height available;
        // without a definite one, the initial containing block's size in
        // the orthogonal box's inline axis (CSS Writing Modes 3 §7.3).
        let vp_h = icb_inline_size(self.ctx, &r.style);
        let avail = if is_root { cb.height.unwrap_or(vp_h) } else { (cb.height.unwrap_or(vp_h) - mt - mb).max(0.0) };
        let inline_auto = r.style.width.is_auto();
        let pb_inline = r.style.padding.left.resolve_or_zero(cbw) + r.style.padding.right.resolve_or_zero(cbw) + r.style.border_width.left + r.style.border_width.right;
        let lmode = match mode {
            // A physical height forced by the parent (stretched flex / grid
            // items) is the logical width.
            _ if forced_h.is_some() && !is_root => WidthMode::Fixed((forced_h.unwrap_or(0.0) - pb_inline).max(0.0)),
            // A definite physical width forced by the parent (flex / grid
            // items, cells) is the logical block size.
            WidthMode::Fixed(_) if !is_root => {
                if inline_auto {
                    WidthMode::ShrinkToFit
                } else {
                    WidthMode::Fill
                }
            }
            _ if is_root || !inline_auto => WidthMode::Fill,
            _ => WidthMode::ShrinkToFit,
        };
        let forced_block = match mode {
            WidthMode::Fixed(wv) if !is_root => Some(wv),
            _ => None,
        };
        let lcb = Cb { x: 0.0, width: avail, height: Some((cbw - ml - mr).max(0.0)) };
        let mut bfc = Bfc::new();
        let mut table_style = None;
        if let Some(h) = forced_block {
            let pb = r.style.padding.top.resolve_or_zero(cbw) + r.style.padding.bottom.resolve_or_zero(cbw) + r.style.border_width.top + r.style.border_width.bottom;
            self.forced_content_height = Some((h - pb).max(0.0));
            if matches!(r.formatting, Formatting::Table | Formatting::TableWrapper) && r.style.height != Length::Px(h) {
                // A table's height is a minimum: the size the parent
                // gives it (a flexed item) replaces it.
                let mut st = (*r.style).clone();
                st.height = Length::Px(h);
                st.box_sizing = rabbithole_style::BoxSizing::BorderBox;
                table_style = Some(std::mem::replace(&mut r.style, Rc::new(st)));
                r.layout_cache = None;
            }
        } else {
            self.forced_content_height = None;
        }
        self.layout_block(r, &lcb, 0.0, &mut bfc, lmode);
        self.forced_content_height = None;
        if let Some(st) = table_style {
            r.style = st;
            r.layout_cache = None;
        }
        let lr = r.rect;
        let (pw, ph) = (lr.h, lr.w);
        // A combined-upright run wider than its 1em box is squeezed into it.
        let squeeze = if w.wm_tcy { tcy_squeeze(r) } else { None };
        let x = if is_root && rl { cb.x + cb.width - pw - mr } else { cb.x + ml };
        let y = top;
        // Logical → physical.
        let mtx: Matrix = match kind {
            WmKind::Cw => [0.0, 1.0, -1.0, 0.0, x + pw + lr.y, y - lr.x],
            WmKind::Reflect => [0.0, 1.0, 1.0, 0.0, x - lr.y, y - lr.x],
            WmKind::Ccw => [0.0, -1.0, 1.0, 0.0, x - lr.y, y + ph + lr.x],
        };
        let mtx = match squeeze {
            Some(sq) => rabbithole_style::multiply_matrix(mtx, sq),
            None => mtx,
        };
        w.wm_transform = Some(mtx);
        w.rect = Rect::new(x, y, pw, ph);
        w.content_rect = w.rect;
        w.margin = Edges { top: mt, right: mr, bottom: mb, left: ml };
        w.collapsed_bottom_margin = mb;
        w.bottom_margins = Margins::of(mb).pair();
        w.baseline = None;
        w.fixed = phys.position == Position::Fixed;
        w.lines.clear();
    }

    /// Intrinsic widths of a vertical subtree's wrapper: its block size
    /// when laid out in the viewport height.
    pub(crate) fn wm_wrapper_intrinsic(&mut self, w: &mut LayoutBox) -> (f32, f32) {
        let _probe = self.grid_publication.probe();
        let Some(r) = w.children.first_mut() else { return (0.0, 0.0) };
        let vp_h = icb_inline_size(self.ctx, &r.style);
        let lcb = Cb { x: 0.0, width: vp_h, height: None };
        let mut bfc = Bfc::new();
        self.forced_content_height = None;
        let mode = if r.style.width.is_auto() { WidthMode::ShrinkToFit } else { WidthMode::Fill };
        self.layout_block(r, &lcb, 0.0, &mut bfc, mode);
        let h = r.rect.h;
        (h, h)
    }
}

/// The horizontal scale (as a matrix in `r`'s space) fitting the text of a
/// combined-upright box `r` into its content box, when it overflows.
fn tcy_squeeze(r: &mut LayoutBox) -> Option<Matrix> {
    let (mut x0, mut x1) = (f32::INFINITY, f32::NEG_INFINITY);
    for f in r.lines.iter().flat_map(|l| &l.fragments) {
        x0 = x0.min(f.rect.x);
        x1 = x1.max(f.rect.right());
    }
    let c = r.content_rect;
    let wid = x1 - x0;
    if !(wid > c.w + 0.01) || c.w <= 0.0 {
        return None;
    }
    // The text starts at the content box's left edge (whatever its
    // alignment and direction), then is scaled about that edge.
    let dx = c.x - x0;
    if dx != 0.0 {
        for f in r.lines.iter_mut().flat_map(|l| &mut l.fragments) {
            f.rect.x += dx;
        }
    }
    let sx = c.w / wid;
    Some([sx, 0.0, 0.0, 1.0, c.x * (1.0 - sx), 0.0])
}

/// The initial containing block's size in the inline axis of a box of
/// (physical) style `s`: the viewport height for vertical text.
fn icb_inline_size(ctx: &crate::LayoutContext, s: &ComputedStyle) -> f32 {
    if s.writing_mode.is_vertical() { ctx.viewport_height.max(1.0) } else { ctx.viewport_width.max(1.0) }
}

/// Map a physical rectangle into the logical space of a wrapper.
pub(crate) fn to_logical(m: Matrix, r: Rect) -> Rect {
    match crate::tree::invert_matrix(m) {
        Some(inv) => rabbithole_gfx::path::transform_rect_bounds(&inv, r),
        None => r,
    }
}
