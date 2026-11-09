//! Display list generation (document coordinates) with CSS 2.1 Appendix E
//! stacking order, transforms, opacity layers, fixed positioning and the
//! sticky offset applied at paint time.

use std::sync::Arc;
use crate::background::{self, box_radii, BoxGeometry};
use crate::tree::{BoxKind, DecorationInfo, Flow, FragmentKind, LayoutBox, LayoutTree};
use crate::LayoutContext;
use rabbithole_gfx::{Color, FontId, Image, Matrix, Paint, Point, Radii, Rect};
use rabbithole_style::{BorderStyle, ObjectFit, Shadow, TextDecorationStyle, Visibility};
use std::rc::Rc;

#[derive(Debug, Clone)]
pub enum DisplayItem {
    /// Solid fill with a uniform corner radius (fast path).
    Rect {
        rect: Rect,
        color: Color,
        radius: f32,
    },
    /// Fill with per-corner radii and any paint (solid / gradients).
    RectRadii {
        rect: Rect,
        radii: Radii,
        paint: Paint,
    },
    Border {
        rect: Rect,
        widths: [f32; 4],
        colors: [Color; 4],
        styles: [BorderStyle; 4],
        radii: Radii,
    },
    BoxShadow {
        rect: Rect,
        radii: Radii,
        offset: (f32, f32),
        blur: f32,
        spread: f32,
        color: Color,
        inset: bool,
    },
    Text {
        /// Left edge of the run.
        x: f32,
        /// Baseline.
        y: f32,
        font: FontId,
        size: f32,
        color: Color,
        text: String,
        letter_spacing: f32,
        shadows: Vec<Shadow>,
        small_caps: bool,
        oblique: bool,
    },
    /// An image (or the `src` sub-rectangle of it) scaled into `rect`.
    Image {
        rect: Rect,
        src: Rect,
        image: Arc<Image>,
        opacity: f32,
    },
    /// An image tiled over `area` (CSS background-repeat).
    ImageTiled {
        image: Arc<Image>,
        area: Rect,
        origin: Point,
        tile: (f32, f32),
        repeat_x: bool,
        repeat_y: bool,
        opacity: f32,
    },
    /// Horizontal/vertical line (text decorations, hr).
    Line {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        width: f32,
        color: Color,
        style: TextDecorationStyle,
    },
    PushClip(Rect),
    PushClipRounded(Rect, Radii),
    PopClip,
    PushTransform(Matrix),
    PopTransform,
    /// Offscreen layer for `opacity`; `PopLayer` composites it.
    PushLayer(Rect),
    PopLayer(f32),
    /// Items inside are in viewport coordinates (not scrolled).
    PushFixed,
    PopFixed,
}

#[derive(Debug, Clone, Default)]
pub struct DisplayList {
    pub items: Vec<DisplayItem>,
}

impl DisplayList {
    fn push(&mut self, item: DisplayItem) {
        self.items.push(item);
    }
}

pub fn build(tree: &LayoutTree, ctx: &LayoutContext) -> DisplayList {
    let mut list = DisplayList::default();
    let bounds = Rect::new(0.0, 0.0, tree.width, tree.height);
    // Canvas background: the root's background propagates to the viewport,
    // falling back to the body's.
    let root = &tree.root;
    let mut bg = root.style.background_color;
    let mut root_bg_painted = false;
    if bg.is_transparent() {
        if let Some(body) = root.children.iter().find(|c| c.is_in_flow()) {
            bg = body.style.background_color;
        }
    } else {
        root_bg_painted = true;
    }
    if !bg.is_transparent() {
        list.push(DisplayItem::Rect {
            rect: bounds,
            color: bg,
            radius: 0.0,
        });
    }
    let mut p = Painter {
        list: &mut list,
        ctx,
        bounds,
        root,
        fixed: Vec::new(),
        skip_root_bg: root_bg_painted,
        sticky_scrollport: Rect::new(ctx.page_scroll.0, ctx.page_scroll.1, ctx.viewport_width, ctx.viewport_height),
    };
    p.paint_stacking_context(root, bounds);
    // Fixed boxes form the topmost layer (viewport coordinates).
    let mut fixed = std::mem::take(&mut p.fixed);
    if !fixed.is_empty() {
        fixed.sort_by_key(|b| b.style.z_index.unwrap_or(0));
        p.list.push(DisplayItem::PushFixed);
        let vp = Rect::new(0.0, 0.0, ctx.viewport_width, ctx.viewport_height);
        for b in fixed {
            p.paint_stacking_context(b, vp);
        }
        p.list.push(DisplayItem::PopFixed);
    }
    list
}

struct Painter<'l, 't, 'a> {
    list: &'l mut DisplayList,
    ctx: &'l LayoutContext<'a>,
    bounds: Rect,
    root: &'t LayoutBox,
    /// Fixed-position boxes collected for the final layer.
    fixed: Vec<&'t LayoutBox>,
    skip_root_bg: bool,
    sticky_scrollport: Rect,
}

/// A positioned / stacking descendant deferred to its stacking context.
struct Deferred<'t> {
    z: i32,
    order: usize,
    b: &'t LayoutBox,
    /// Containing-block content rect for sticky offsets.
    cb: Rect,
}

impl<'l, 't, 'a> Painter<'l, 't, 'a> {
    fn visible(&self, r: &Rect) -> bool {
        !r.intersect(&self.bounds).is_empty() || r.is_empty()
    }

    // ------------------------------------------------------ stacking contexts

    /// Paint `root` as a stacking context (CSS 2.1 Appendix E).
    fn paint_stacking_context(&mut self, root: &'t LayoutBox, cb: Rect) {
        if root.style.visibility == Visibility::Hidden && root.children.is_empty() && root.lines.is_empty() {
            return;
        }
        // Sticky displacement, transform and opacity wrappers.
        let (sdx, sdy) = root.sticky_offset(self.sticky_scrollport, cb);
        let mut pushed_transform = false;
        if sdx != 0.0 || sdy != 0.0 {
            self.list.push(DisplayItem::PushTransform([1.0, 0.0, 0.0, 1.0, sdx, sdy]));
            pushed_transform = true;
        }
        let mut pushed_matrix = false;
        if let Some(m) = root.transform_matrix() {
            self.list.push(DisplayItem::PushTransform(m));
            pushed_matrix = true;
        }
        let opacity = root.style.opacity.clamp(0.0, 1.0);
        let layer = opacity < 1.0;
        if layer {
            if opacity <= 0.0 {
                self.pop_wrappers(pushed_matrix, pushed_transform);
                return;
            }
            self.list.push(DisplayItem::PushLayer(root.subtree_bounds()));
        }

        // Collect deferred descendants.
        let mut deferred: Vec<Deferred<'t>> = Vec::new();
        let mut order = 0usize;
        self.collect_deferred(root, root.content_rect, &mut deferred, &mut order);
        let mut negative: Vec<&Deferred<'t>> = deferred.iter().filter(|d| d.z < 0).collect();
        let mut zero: Vec<&Deferred<'t>> = deferred.iter().filter(|d| d.z == 0).collect();
        let mut positive: Vec<&Deferred<'t>> = deferred.iter().filter(|d| d.z > 0).collect();
        negative.sort_by_key(|d| (d.z, d.order));
        zero.sort_by_key(|d| d.order);
        positive.sort_by_key(|d| (d.z, d.order));

        // 1. Background and borders of the root.
        let skip_bg = self.skip_root_bg && std::ptr::eq(root, self.root);
        if root.kind != BoxKind::Text {
            self.paint_background_and_border(root, skip_bg);
        }
        // 2. Negative z-index stacking contexts.
        for d in negative {
            self.paint_stacking_context(d.b, d.cb);
        }
        // 3–5. In-flow content (block backgrounds, floats, inline content).
        self.paint_contents(root);
        // 6. Positioned descendants with z-index auto / 0, in tree order.
        for d in zero {
            self.paint_stacking_context(d.b, d.cb);
        }
        // 7. Positive z-index stacking contexts.
        for d in positive {
            self.paint_stacking_context(d.b, d.cb);
        }
        // Outline is painted above everything of the element.
        self.paint_outline(root);

        if layer {
            self.list.push(DisplayItem::PopLayer(opacity));
        }
        self.pop_wrappers(pushed_matrix, pushed_transform);
    }

    fn pop_wrappers(&mut self, matrix: bool, translate: bool) {
        if matrix {
            self.list.push(DisplayItem::PopTransform);
        }
        if translate {
            self.list.push(DisplayItem::PopTransform);
        }
    }

    /// Walk `b`'s subtree collecting positioned boxes and stacking contexts
    /// that belong to the current stacking context. Does not enter boxes
    /// that are themselves deferred.
    fn collect_deferred(&mut self, b: &'t LayoutBox, cb: Rect, out: &mut Vec<Deferred<'t>>, order: &mut usize) {
        let cb_for_children = if b.is_positioned() { b.padding_rect() } else { cb };
        for c in &b.children {
            if c.fixed && c.flow == Flow::Absolute {
                self.fixed.push(c);
                continue;
            }
            let deferred = c.is_positioned() || c.is_stacking_context() || c.flow == Flow::Absolute;
            if deferred {
                *order += 1;
                let cb_rect = if c.flow == Flow::Absolute { cb_for_children } else { b.content_rect };
                out.push(Deferred {
                    z: if c.is_stacking_context() || c.is_positioned() { c.style.z_index.unwrap_or(0) } else { 0 },
                    order: *order,
                    b: c,
                    cb: cb_rect,
                });
            } else {
                self.collect_deferred(c, cb_for_children, out, order);
            }
        }
    }

    fn is_deferred(c: &LayoutBox) -> bool {
        c.is_positioned() || c.is_stacking_context() || c.flow == Flow::Absolute
    }

    /// Paint the in-flow contents of `b` (not its own background) in tree
    /// order, skipping deferred boxes.
    fn paint_contents(&mut self, b: &'t LayoutBox) {
        if let Some(m) = &b.marker {
            if b.style.visibility != Visibility::Hidden {
                self.list.push(DisplayItem::Text {
                    x: m.x,
                    y: m.baseline,
                    font: m.font,
                    size: m.size,
                    color: m.color,
                    text: m.text.to_string(),
                    letter_spacing: 0.0,
                    shadows: Vec::new(),
                    small_caps: false,
                    oblique: false,
                });
            }
        }
        if b.kind == BoxKind::Image {
            self.paint_image_box(b);
        }
        let clip = b.clips && !b.rect.is_empty();
        if clip {
            let pr = b.padding_rect();
            let radii = background::inner_radii(box_radii(b), &b.style.border_width);
            if radii.iter().any(|(h, v)| *h > 0.0 && *v > 0.0) {
                self.list.push(DisplayItem::PushClipRounded(pr, radii));
            } else {
                self.list.push(DisplayItem::PushClip(pr));
            }
        }
        // Floats first (they sit below inline content).
        for c in b.children.iter().filter(|c| c.flow.is_float() && !Self::is_deferred(c)) {
            self.paint_float_or_atomic(c);
        }
        if b.lines.is_empty() {
            for c in b.children.iter().filter(|c| c.flow == Flow::InFlow && !Self::is_deferred(c)) {
                self.paint_block_child(c);
            }
        } else {
            self.paint_lines(b);
        }
        if clip {
            self.list.push(DisplayItem::PopClip);
        }
    }

    /// An in-flow block-level (non-positioned) child: its background then its
    /// contents, recursively.
    fn paint_block_child(&mut self, c: &'t LayoutBox) {
        if c.kind != BoxKind::Text {
            self.paint_background_and_border(c, false);
        }
        self.paint_contents(c);
        self.paint_outline(c);
    }

    /// Floats and atomic inline-level boxes paint atomically (as if they
    /// were stacking contexts with z-index 0 that don't capture positioned
    /// descendants — approximated by a full stacking context).
    fn paint_float_or_atomic(&mut self, c: &'t LayoutBox) {
        self.paint_stacking_context(c, c.content_rect);
    }

    // -------------------------------------------------------- backgrounds

    fn paint_background_and_border(&mut self, b: &LayoutBox, skip_bg: bool) {
        let style = &b.style;
        let rect = b.rect;
        if style.visibility == Visibility::Hidden || rect.is_empty() || !self.visible(&rect) {
            return;
        }
        let geom = BoxGeometry {
            border: rect,
            padding: b.padding_rect(),
            content: b.content_rect,
            radii: box_radii(b),
            border_width: style.border_width,
        };
        let vp = Rect::new(self.ctx.page_scroll.0, self.ctx.page_scroll.1, self.ctx.viewport_width, self.ctx.viewport_height);
        background::paint_box_shadows(self.list, &geom, &style.box_shadow, false);
        if !skip_bg {
            background::paint_backgrounds(self.list, &geom, style, self.ctx.background_images, vp);
        }
        background::paint_box_shadows(self.list, &geom, &style.box_shadow, true);
        if style.has_visible_border() {
            let bw = style.border_width;
            let bs = style.border_style;
            let bc = style.border_color;
            self.list.push(DisplayItem::Border {
                rect,
                widths: [bw.top, bw.right, bw.bottom, bw.left],
                colors: [bc.top, bc.right, bc.bottom, bc.left],
                styles: [bs.top, bs.right, bs.bottom, bs.left],
                radii: geom.radii,
            });
        }
    }

    fn paint_outline(&mut self, b: &LayoutBox) {
        let s = &b.style;
        if s.outline_width <= 0.0 || !s.outline_style.is_visible() || s.visibility == Visibility::Hidden || b.kind == BoxKind::Text {
            return;
        }
        let w = s.outline_width;
        let off = s.outline_offset;
        let r = Rect::new(b.rect.x - off - w, b.rect.y - off - w, b.rect.w + 2.0 * (off + w), b.rect.h + 2.0 * (off + w));
        if !self.visible(&r) {
            return;
        }
        let color = s.outline_color.unwrap_or(s.color);
        let radii = box_radii(b);
        let grown: Radii = [
            (radii[0].0 + off + w, radii[0].1 + off + w),
            (radii[1].0 + off + w, radii[1].1 + off + w),
            (radii[2].0 + off + w, radii[2].1 + off + w),
            (radii[3].0 + off + w, radii[3].1 + off + w),
        ];
        let has_radius = radii.iter().any(|(h, v)| *h > 0.0 && *v > 0.0);
        self.list.push(DisplayItem::Border {
            rect: r,
            widths: [w; 4],
            colors: [color; 4],
            styles: [s.outline_style; 4],
            radii: if has_radius { grown } else { [(0.0, 0.0); 4] },
        });
    }

    fn paint_image_box(&mut self, b: &LayoutBox) {
        let Some(img) = &b.image else { return };
        if b.style.visibility == Visibility::Hidden || !self.visible(&b.content_rect) || b.content_rect.is_empty() {
            return;
        }
        let (iw, ih) = (img.width as f32, img.height as f32);
        let dest = b.content_rect;
        let fit = b.style.object_fit;
        let (dw, dh) = match fit {
            ObjectFit::Fill => (dest.w, dest.h),
            ObjectFit::Contain | ObjectFit::ScaleDown | ObjectFit::Cover | ObjectFit::None => {
                if iw <= 0.0 || ih <= 0.0 {
                    (dest.w, dest.h)
                } else {
                    let sx = dest.w / iw;
                    let sy = dest.h / ih;
                    let s = match fit {
                        ObjectFit::Contain => sx.min(sy),
                        ObjectFit::ScaleDown => sx.min(sy).min(1.0),
                        ObjectFit::Cover => sx.max(sy),
                        _ => 1.0,
                    };
                    (iw * s, ih * s)
                }
            }
        };
        let px = b.style.object_position.0;
        let py = b.style.object_position.1;
        let ox = match px {
            rabbithole_style::Length::Percent(p) => (dest.w - dw) * p / 100.0,
            other => other.resolve_or_zero(dest.w),
        };
        let oy = match py {
            rabbithole_style::Length::Percent(p) => (dest.h - dh) * p / 100.0,
            other => other.resolve_or_zero(dest.h),
        };
        let placed = Rect::new(dest.x + ox, dest.y + oy, dw, dh);
        // Clip the placed image to the content box and compute the source
        // sub-rectangle actually shown.
        let shown = placed.intersect(&dest);
        if shown.is_empty() || dw <= 0.0 || dh <= 0.0 {
            return;
        }
        let src = Rect::new(
            (shown.x - placed.x) / dw * iw,
            (shown.y - placed.y) / dh * ih,
            shown.w / dw * iw,
            shown.h / dh * ih,
        );
        self.list.push(DisplayItem::Image {
            rect: shown,
            src,
            image: img.clone(),
            opacity: 1.0,
        });
    }

    // --------------------------------------------------------------- lines

    fn paint_lines(&mut self, b: &'t LayoutBox) {
        let rtl = b.style.direction == rabbithole_style::Direction::Rtl;
        let _ = rtl;
        for line in &b.lines {
            if !self.visible(&line.rect) && !line.rect.is_empty() && line.fragments.iter().all(|f| !self.visible(&f.rect)) {
                continue;
            }
            for f in &line.fragments {
                match &f.kind {
                    FragmentKind::InlineBox {
                        style,
                        left_edge,
                        right_edge,
                    } => {
                        if style.visibility == Visibility::Hidden {
                            continue;
                        }
                        let bw = style.border_width;
                        let widths = [bw.top, if *right_edge { bw.right } else { 0.0 }, bw.bottom, if *left_edge { bw.left } else { 0.0 }];
                        let radii = style.resolved_radii(f.rect.w, f.rect.h);
                        let geom = BoxGeometry {
                            border: f.rect,
                            padding: Rect::new(f.rect.x + widths[3], f.rect.y + widths[0], (f.rect.w - widths[1] - widths[3]).max(0.0), (f.rect.h - widths[0] - widths[2]).max(0.0)),
                            content: f.rect,
                            radii,
                            border_width: rabbithole_style::Edges { top: widths[0], right: widths[1], bottom: widths[2], left: widths[3] },
                        };
                        let vp = Rect::new(self.ctx.page_scroll.0, self.ctx.page_scroll.1, self.ctx.viewport_width, self.ctx.viewport_height);
                        background::paint_box_shadows(self.list, &geom, &style.box_shadow, false);
                        background::paint_backgrounds(self.list, &geom, style, self.ctx.background_images, vp);
                        background::paint_box_shadows(self.list, &geom, &style.box_shadow, true);
                        if style.has_visible_border() {
                            let bs = style.border_style;
                            let bc = style.border_color;
                            self.list.push(DisplayItem::Border {
                                rect: f.rect,
                                widths,
                                colors: [bc.top, bc.right, bc.bottom, bc.left],
                                styles: [bs.top, bs.right, bs.bottom, bs.left],
                                radii,
                            });
                        }
                    }
                    FragmentKind::Text {
                        text,
                        font,
                        size,
                        color,
                        letter_spacing,
                        decoration,
                        hidden,
                        decoration_info,
                        shadows,
                        small_caps,
                        oblique,
                    } => {
                        if *hidden || !self.visible(&f.rect) {
                            continue;
                        }
                        self.list.push(DisplayItem::Text {
                            x: f.rect.x,
                            y: f.baseline,
                            font: *font,
                            size: *size,
                            color: *color,
                            text: text.to_string(),
                            letter_spacing: *letter_spacing,
                            shadows: shadows.clone(),
                            small_caps: *small_caps,
                            oblique: *oblique,
                        });
                        if decoration.any() {
                            self.paint_decorations(f.rect, f.baseline, *size, *color, decoration_info);
                        }
                    }
                    FragmentKind::Box(i) => {
                        let c = &b.children[*i];
                        if !Self::is_deferred(c) {
                            self.paint_float_or_atomic(c);
                        }
                    }
                }
            }
        }
    }

    fn paint_decorations(&mut self, rect: Rect, baseline: f32, size: f32, text_color: Color, info: &DecorationInfo) {
        let thickness = info.thickness.unwrap_or((size / 16.0).max(1.0));
        let color = info.color.unwrap_or(text_color);
        let style = info.style;
        let mut line = |y: f32| {
            self.list.push(DisplayItem::Line {
                x0: rect.x,
                y0: y,
                x1: rect.right(),
                y1: y,
                width: thickness,
                color,
                style,
            });
        };
        if info.lines.underline {
            line(baseline + thickness * 1.5);
        }
        if info.lines.line_through {
            line(baseline - size * 0.3);
        }
        if info.lines.overline {
            line(rect.y + thickness / 2.0);
        }
    }
}
