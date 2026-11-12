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

/// Transpose a vertical subtree in place (styles of boxes and inline items).
fn transpose_subtree(b: &mut LayoutBox, cache: &mut Cache, mode: WritingMode) {
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
            || (!c.style.writing_mode.is_vertical() && !c.style.layout_transposed && !matches!(c.kind, BoxKind::Text | BoxKind::LineBreak | BoxKind::Inline) && !table_internal(c));
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
    ws.position = phys.position;
    ws.inset = phys.inset;
    ws.z_index = phys.z_index;
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
    ws.align_self = phys.align_self;
    ws.justify_self = phys.justify_self;
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
fn table_internal(b: &LayoutBox) -> bool {
    matches!(b.kind, BoxKind::TableRow | BoxKind::TableCell | BoxKind::Table) || matches!(b.formatting, Formatting::TableRow | Formatting::Table) || crate::table::is_row_group(b)
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
        let element = !matches!(c.kind, BoxKind::Text | BoxKind::LineBreak | BoxKind::Image) && !table_internal(c);
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
    if let Some(bs) = root.children.iter().find(|c| c.node.is_some() && c.node == body).map(|c| c.style.clone()) {
        if bs.writing_mode != root.style.writing_mode && !root.style.layout_transposed && !root.wm_wrapper {
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
        let vp_h = self.ctx.viewport_height.max(1.0);
        // The available inline size is the physical height available.
        let avail = if is_root { cb.height.unwrap_or(vp_h) } else { (cb.height.unwrap_or(vp_h) - mt - mb).max(0.0) };
        let Some(r) = w.children.first_mut() else { return };
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
        if let Some(h) = forced_block {
            let pb = r.style.padding.top.resolve_or_zero(cbw) + r.style.padding.bottom.resolve_or_zero(cbw) + r.style.border_width.top + r.style.border_width.bottom;
            self.forced_content_height = Some((h - pb).max(0.0));
        } else {
            self.forced_content_height = None;
        }
        self.layout_block(r, &lcb, 0.0, &mut bfc, lmode);
        self.forced_content_height = None;
        let lr = r.rect;
        let (pw, ph) = (lr.h, lr.w);
        let x = if is_root && rl { cb.x + cb.width - pw - mr } else { cb.x + ml };
        let y = top;
        // Logical → physical.
        let mtx: Matrix = match kind {
            WmKind::Cw => [0.0, 1.0, -1.0, 0.0, x + pw + lr.y, y - lr.x],
            WmKind::Reflect => [0.0, 1.0, 1.0, 0.0, x - lr.y, y - lr.x],
            WmKind::Ccw => [0.0, -1.0, 1.0, 0.0, x - lr.y, y + ph + lr.x],
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
        let vp_h = self.ctx.viewport_height.max(1.0);
        let Some(r) = w.children.first_mut() else { return (0.0, 0.0) };
        let lcb = Cb { x: 0.0, width: vp_h, height: None };
        let mut bfc = Bfc::new();
        self.forced_content_height = None;
        let mode = if r.style.width.is_auto() { WidthMode::ShrinkToFit } else { WidthMode::Fill };
        self.layout_block(r, &lcb, 0.0, &mut bfc, mode);
        let h = r.rect.h;
        (h, h)
    }
}

/// Map a physical rectangle into the logical space of a wrapper.
pub(crate) fn to_logical(m: Matrix, r: Rect) -> Rect {
    match crate::tree::invert_matrix(m) {
        Some(inv) => rabbithole_gfx::path::transform_rect_bounds(&inv, r),
        None => r,
    }
}
