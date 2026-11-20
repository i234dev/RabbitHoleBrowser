//! Rendering of the tree onto a gfx `Canvas`.

use crate::parse::{Built, Gradient, Kind, Node, Pattern, Spread};
use crate::style::{ClipRef, PaintSpec, Props, TextAnchor};
use crate::units::{is_identity, multiply, scale_factor, transform_point, translate, viewbox_transform, Length, ViewBox};
use rabbithole_gfx::{Canvas, Color, ColorStop, FillRule, FontContext, Paint, PathSeg, Point, Rect, StrokeStyle};

const MAX_USE_DEPTH: usize = 16;

thread_local! {
    /// Nesting of filtered painting (feImage can reach filtered content).
    static FILTER_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Render the element `id` of `tree` (feImage) through `m` into a
/// transparent `w`×`h` buffer.
pub(crate) fn render_element(tree: &Built, fonts: &FontContext, viewport: (f32, f32), id: &str, m: &rabbithole_gfx::Matrix, w: usize, h: usize) -> Option<crate::filter::Pixels> {
    let &target = tree.ids.get(id)?;
    let render = |bg: Color| {
        let mut c = Canvas::new(w as u32, h as u32);
        c.clear(bg);
        c.push_transform(*m);
        {
            let mut p = Painter::new(tree, &mut c, fonts, viewport);
            let props = Props::default();
            p.paint_node(target, &props);
        }
        c.data
    };
    let black = render(Color::BLACK);
    let white = render(Color::WHITE);
    Some(crate::filter::css::from_black_white(&black, &white, w, h))
}

pub struct Painter<'a> {
    pub tree: &'a Built,
    pub canvas: &'a mut Canvas,
    pub fonts: &'a FontContext,
    /// Root viewport size (user units) for percentage resolution.
    pub viewport: (f32, f32),
    use_depth: usize,
}

impl<'a> Painter<'a> {
    pub fn new(tree: &'a Built, canvas: &'a mut Canvas, fonts: &'a FontContext, viewport: (f32, f32)) -> Self {
        Painter { tree, canvas, fonts, viewport, use_depth: 0 }
    }

    fn diag(&self) -> f32 {
        let (w, h) = self.viewport;
        ((w * w + h * h) / 2.0).sqrt()
    }

    /// Paint the root's children (the caller set up the viewport transform).
    pub fn paint_root(&mut self) {
        let root = self.tree.root;
        let props = Props::default().inherit(&self.tree.nodes[root].spec, self.diag());
        let children = self.tree.nodes[root].children.clone();
        for c in children {
            self.paint_node(c, &props);
        }
    }

    fn paint_node(&mut self, idx: usize, parent: &Props) {
        let node: &Node = &self.tree.nodes[idx];
        if node.spec.display_none {
            return;
        }
        match node.kind {
            Kind::Defs | Kind::Gradient(_) | Kind::ClipPath { .. } | Kind::Pattern(_) | Kind::Symbol { .. } => return,
            _ => {}
        }
        let props = parent.inherit(&node.spec, self.diag());
        let pushed_transform = !is_identity(&node.transform);
        if pushed_transform {
            self.canvas.push_transform(node.transform);
        }
        let opacity = node.spec.opacity.unwrap_or(1.0);
        let layered = opacity < 1.0;
        if layered {
            let bounds = self.layer_bounds(node, &props);
            self.canvas.begin_layer(bounds);
        }
        let clipped = match &node.spec.clip_path {
            Some(ClipRef::Id(id)) => self.push_clip(id, node.bbox.unwrap_or(Rect::new(0.0, 0.0, self.viewport.0, self.viewport.1))),
            Some(ClipRef::Css(cp)) => self.push_css_clip(cp, node, &props),
            None => false,
        };

        match node.spec.filter.as_deref() {
            Some(items) if !items.is_empty() => {
                let items = items.to_vec();
                self.paint_filtered(idx, &items, &props);
            }
            _ => self.paint_content(idx, &props),
        }

        if clipped {
            self.canvas.pop_clip();
        }
        if layered {
            self.canvas.end_layer(opacity);
        }
        if pushed_transform {
            self.canvas.pop_transform();
        }
    }

    /// Paint what the element `idx` itself draws (no transform, opacity,
    /// clip or filter).
    fn paint_content(&mut self, idx: usize, props: &Props) {
        let node: &Node = &self.tree.nodes[idx];
        let props = props.clone();
        match &node.kind {
            Kind::Root | Kind::Group => {
                let children = node.children.clone();
                for c in children {
                    self.paint_node(c, &props);
                }
            }
            Kind::Svg { rect, viewbox, par, overflow_visible } => {
                let rect = *rect;
                let (viewbox, par, overflow_visible) = (*viewbox, *par, *overflow_visible);
                if rect.w <= 0.0 || rect.h <= 0.0 {
                    // nothing
                } else {
                    let children = node.children.clone();
                    self.paint_viewport(rect, viewbox, par, overflow_visible, &props, &children);
                }
            }
            Kind::Shape(path) => {
                if props.visible {
                    let path = path.clone();
                    let bbox = node.bbox.unwrap_or(Rect::new(0.0, 0.0, 1.0, 1.0));
                    self.paint_shape(&path, &props, bbox);
                }
            }
            Kind::Text(text) => {
                if props.visible {
                    let text = text.clone();
                    self.paint_text(&text, &props);
                }
            }
            Kind::Image { image, rect, par } => {
                if props.visible && rect.w > 0.0 && rect.h > 0.0 {
                    let dest = fit_image(image.width as f32, image.height as f32, *rect, *par);
                    let src = Rect::new(0.0, 0.0, image.width as f32, image.height as f32);
                    if par.slice {
                        self.canvas.push_clip(*rect);
                        self.canvas.draw_image_region(image, src, dest, 1.0);
                        self.canvas.pop_clip();
                    } else {
                        self.canvas.draw_image_region(image, src, dest, 1.0);
                    }
                }
            }
            Kind::Use { href, x, y, width, height } => {
                let (href, x, y, width, height) = (href.clone(), *x, *y, *width, *height);
                self.paint_use(&href, x, y, width, height, &props);
            }
            Kind::Defs | Kind::Gradient(_) | Kind::ClipPath { .. } | Kind::Pattern(_) | Kind::Symbol { .. } => {}
        }

    }

    /// `filter`: paint the element offscreen (over black and over white to
    /// recover its alpha), run the filter chain and draw the result.
    fn paint_filtered(&mut self, idx: usize, items: &[crate::filter::FilterItem], props: &Props) {
        use crate::filter::{FilterEnv, FilterItem};
        let tree = self.tree;
        let node = &tree.nodes[idx];
        // A missing `url()` target: the filter is ignored. Deep nesting
        // (feImage cycles) paints unfiltered.
        let missing = items.iter().any(|i| matches!(i, FilterItem::Url(id) if !tree.filters.contains_key(id)));
        let depth = FILTER_DEPTH.with(|d| d.get());
        if missing || depth > 6 {
            self.paint_content(idx, props);
            return;
        }
        let bbox = node.bbox.unwrap_or_default();
        let defs = |id: &str| tree.filters.get(id).cloned();
        let t = self.canvas.transform();
        let scale = scale_factor(&t);
        let painted = self.layer_bounds(node, props);
        let area = crate::filter::chain_bounds(items, &defs, bbox, painted, self.viewport);
        let (cw, ch) = (self.canvas.width as f32, self.canvas.height as f32);
        let dev = crate::units::transform_rect_bounds(&t, area).intersect(&Rect::new(-cw, -ch, 3.0 * cw, 3.0 * ch));
        if dev.w <= 0.0 || dev.h <= 0.0 {
            return;
        }
        let (x0, y0) = (dev.x.floor() as i32, dev.y.floor() as i32);
        let (w, h) = (((dev.x + dev.w).ceil() as i32 - x0) as usize, ((dev.y + dev.h).ceil() as i32 - y0) as usize);
        if w == 0 || h == 0 || w * h > 1 << 24 {
            return;
        }
        let m = multiply(&translate(-(x0 as f32), -(y0 as f32)), &t);
        FILTER_DEPTH.with(|d| d.set(depth + 1));
        let mut render = |bg: Color| {
            let mut c = Canvas::new(w as u32, h as u32);
            c.clear(bg);
            c.push_transform(m);
            std::mem::swap(self.canvas, &mut c);
            self.paint_content(idx, props);
            std::mem::swap(self.canvas, &mut c);
            c.data
        };
        let black = render(Color::BLACK);
        let white = render(Color::WHITE);
        let src = crate::filter::css::from_black_white(&black, &white, w, h);
        drop((black, white));
        let (fonts, viewport) = (self.fonts, self.viewport);
        let element = |id: &str, m: &rabbithole_gfx::Matrix, w: usize, h: usize| render_element(tree, fonts, viewport, id, m, w, h);
        let env = FilterEnv { m, bbox, viewport: self.viewport, element: Some(&element), fonts: Some(fonts) };
        let out = crate::filter::apply_chain(items, &defs, src, &env, scale);
        FILTER_DEPTH.with(|d| d.set(depth));
        let Some(out) = out else { return };
        let img = rabbithole_gfx::Image::new(w as u32, h as u32, crate::filter::css::to_rgba(&out));
        let inv = crate::units::invert(&t);
        if let Some(inv) = inv {
            self.canvas.push_transform(inv);
        }
        let smooth = self.canvas.image_smoothing;
        self.canvas.image_smoothing = false;
        self.canvas.draw_image(&img, Rect::new(x0 as f32, y0 as f32, w as f32, h as f32), 1.0);
        self.canvas.image_smoothing = smooth;
        if inv.is_some() {
            self.canvas.pop_transform();
        }
    }

    fn layer_bounds(&self, node: &Node, props: &Props) -> Rect {
        match node.bbox {
            Some(b) => {
                let pad = props.stroke_width.max(1.0) * 2.0 + 2.0;
                Rect::new(b.x - pad, b.y - pad, b.w + 2.0 * pad, b.h + 2.0 * pad)
            }
            None => {
                let (w, h) = self.viewport;
                Rect::new(-w, -h, w * 3.0, h * 3.0)
            }
        }
    }

    fn paint_viewport(&mut self, rect: Rect, viewbox: Option<ViewBox>, par: crate::units::AspectRatio, overflow_visible: bool, props: &Props, children: &[usize]) {
        if !overflow_visible {
            self.canvas.push_clip(rect);
        }
        let m = match viewbox {
            Some(vb) => viewbox_transform(vb, rect, par),
            None => translate(rect.x, rect.y),
        };
        self.canvas.push_transform(m);
        let saved = self.viewport;
        self.viewport = match viewbox {
            Some(vb) => (vb.w, vb.h),
            None => (rect.w, rect.h),
        };
        for &c in children {
            self.paint_node(c, props);
        }
        self.viewport = saved;
        self.canvas.pop_transform();
        if !overflow_visible {
            self.canvas.pop_clip();
        }
    }

    fn paint_use(&mut self, href: &str, x: f32, y: f32, width: Option<f32>, height: Option<f32>, props: &Props) {
        if self.use_depth >= MAX_USE_DEPTH {
            return;
        }
        let Some(&target) = self.tree.ids.get(href) else { return };
        self.use_depth += 1;
        let moved = x != 0.0 || y != 0.0;
        if moved {
            self.canvas.push_transform(translate(x, y));
        }
        let tnode = &self.tree.nodes[target];
        match &tnode.kind {
            Kind::Symbol { viewbox, par, width: sw, height: sh, overflow_visible } => {
                let (vw, vh) = self.viewport;
                let w = width.or(*sw).unwrap_or(vw);
                let h = height.or(*sh).unwrap_or(vh);
                let tprops = props.inherit(&tnode.spec, self.diag());
                let children = tnode.children.clone();
                let (viewbox, par, ov) = (*viewbox, *par, *overflow_visible);
                if !tnode.spec.display_none {
                    self.paint_viewport(Rect::new(0.0, 0.0, w, h), viewbox, par, ov, &tprops, &children);
                }
            }
            Kind::Svg { rect, viewbox, par, overflow_visible } => {
                let w = width.unwrap_or(rect.w);
                let h = height.unwrap_or(rect.h);
                let tprops = props.inherit(&tnode.spec, self.diag());
                let children = tnode.children.clone();
                let (viewbox, par, ov) = (*viewbox, *par, *overflow_visible);
                if !tnode.spec.display_none {
                    self.paint_viewport(Rect::new(rect.x, rect.y, w, h), viewbox, par, ov, &tprops, &children);
                }
            }
            _ => {
                self.paint_node(target, props);
            }
        }
        if moved {
            self.canvas.pop_transform();
        }
        self.use_depth -= 1;
    }

    /// The `<pattern>` a paint refers to, if any.
    fn pattern_of(&self, spec: &PaintSpec) -> Option<usize> {
        let PaintSpec::Url(id, _) = spec else { return None };
        let &idx = self.tree.ids.get(id)?;
        matches!(self.tree.nodes[idx].kind, Kind::Pattern(_)).then_some(idx)
    }

    /// Fill the current clip with the tiles of pattern `idx` for a shape
    /// with bounding box `bbox` (SVG 1.1 §13.3).
    fn paint_pattern(&mut self, idx: usize, bbox: Rect, opacity: f32) {
        let Kind::Pattern(pat) = &self.tree.nodes[idx].kind else { return };
        let pat: Pattern = pat.clone();
        if self.use_depth >= MAX_USE_DEPTH {
            return;
        }
        let units_bbox = pat.units_bbox.unwrap_or(true);
        let (vw, vh) = self.viewport;
        // In bbox units a bare number is a fraction of the box.
        let res = |l: Option<Length>, origin: f32, size: f32, reference: f32| -> f32 {
            match (l, units_bbox) {
                (None, true) => origin,
                (None, false) => 0.0,
                (Some(Length::Px(v)), true) => origin + v * size,
                (Some(Length::Percent(p)), true) => origin + p / 100.0 * size,
                (Some(l), false) => l.resolve(reference),
            }
        };
        let len = |l: Option<Length>, size: f32, reference: f32| -> f32 {
            match (l, units_bbox) {
                (None, _) => 0.0,
                (Some(Length::Px(v)), true) => v * size,
                (Some(Length::Percent(p)), true) => p / 100.0 * size,
                (Some(l), false) => l.resolve(reference),
            }
        };
        let tx = res(pat.x, bbox.x, bbox.w, vw);
        let ty = res(pat.y, bbox.y, bbox.h, vh);
        let tw = len(pat.width, bbox.w, vw);
        let th = len(pat.height, bbox.h, vh);
        if tw <= 0.0 || th <= 0.0 || !tw.is_finite() || !th.is_finite() {
            return;
        }
        let content = match pat.viewbox {
            Some(vb) if vb.w > 0.0 && vb.h > 0.0 => viewbox_transform(vb, Rect::new(0.0, 0.0, tw, th), pat.par.unwrap_or_default()),
            _ if pat.content_bbox.unwrap_or(false) => [bbox.w, 0.0, 0.0, bbox.h, 0.0, 0.0],
            _ => rabbithole_gfx::IDENTITY,
        };
        let pt = pat.transform.unwrap_or(rabbithole_gfx::IDENTITY);
        // The tiles covering the shape's box, in pattern space.
        let Some(inv) = crate::units::invert(&pt) else { return };
        let area = crate::units::transform_rect_bounds(&inv, bbox);
        let (i0, i1) = (((area.x - tx) / tw).floor() as i64, ((area.x + area.w - tx) / tw).ceil() as i64);
        let (j0, j1) = (((area.y - ty) / th).floor() as i64, ((area.y + area.h - ty) / th).ceil() as i64);
        if (i1 - i0).saturating_mul(j1 - j0) > 4096 {
            return;
        }
        let layered = opacity < 1.0;
        if layered {
            self.canvas.begin_layer(bbox);
        }
        let props = Props::default().inherit(&self.tree.nodes[idx].spec, self.diag());
        let children = self.tree.nodes[idx].children.clone();
        self.use_depth += 1;
        self.canvas.push_transform(pt);
        for j in j0..j1 {
            for i in i0..i1 {
                let (ox, oy) = (tx + i as f32 * tw, ty + j as f32 * th);
                self.canvas.push_transform(translate(ox, oy));
                self.canvas.push_clip(Rect::new(0.0, 0.0, tw, th));
                self.canvas.push_transform(content);
                for &c in &children {
                    self.paint_node(c, &props);
                }
                self.canvas.pop_transform();
                self.canvas.pop_clip();
                self.canvas.pop_transform();
            }
        }
        self.canvas.pop_transform();
        self.use_depth -= 1;
        if layered {
            self.canvas.end_layer(opacity);
        }
    }

    fn paint_shape(&mut self, path: &[PathSeg], props: &Props, bbox: Rect) {
        if let Some(idx) = self.pattern_of(&props.fill) {
            // Pattern fill: the tiles clipped to the shape, then the stroke.
            if !props.paint_order_stroke_first {
                self.canvas.push_clip_path(path, props.fill_rule);
                self.paint_pattern(idx, bbox, props.fill_opacity);
                self.canvas.pop_clip();
            }
            let mut rest = props.clone();
            rest.fill = PaintSpec::None;
            self.paint_shape(path, &rest, bbox);
            if props.paint_order_stroke_first {
                self.canvas.push_clip_path(path, props.fill_rule);
                self.paint_pattern(idx, bbox, props.fill_opacity);
                self.canvas.pop_clip();
            }
            return;
        }
        let fill = self.make_paint(&props.fill, props.fill_opacity, props, bbox);
        let stroke = if props.stroke_width > 0.0 { self.make_paint(&props.stroke, props.stroke_opacity, props, bbox) } else { None };
        let do_fill = |p: &mut Self| {
            if let Some(paint) = &fill {
                p.canvas.fill_path(path, props.fill_rule, paint);
            }
        };
        let do_stroke = |p: &mut Self| {
            if let Some(paint) = &stroke {
                let mut width = props.stroke_width;
                if props.non_scaling_stroke {
                    let sf = scale_factor(&p.canvas.transform());
                    if sf > 1e-6 {
                        width /= sf;
                    }
                }
                let style = StrokeStyle {
                    width,
                    cap: props.stroke_linecap,
                    join: props.stroke_linejoin,
                    miter_limit: props.stroke_miterlimit,
                    dashes: props.stroke_dasharray.clone(),
                    dash_offset: props.stroke_dashoffset,
                };
                p.canvas.stroke_path(path, &style, paint);
            }
        };
        if props.paint_order_stroke_first {
            do_stroke(self);
            do_fill(self);
        } else {
            do_fill(self);
            do_stroke(self);
        }
    }

    /// Resolve a paint spec into a gfx paint for a shape with `bbox`.
    fn make_paint(&self, spec: &PaintSpec, opacity: f32, props: &Props, bbox: Rect) -> Option<Paint> {
        match spec {
            PaintSpec::None => None,
            PaintSpec::Color(c) => Some(Paint::Solid(c.scale_alpha(opacity))),
            PaintSpec::CurrentColor => Some(Paint::Solid(props.color.scale_alpha(opacity))),
            PaintSpec::Url(id, fallback) => {
                if let Some(&idx) = self.tree.ids.get(id) {
                    match &self.tree.nodes[idx].kind {
                        Kind::Gradient(g) => return self.gradient_paint(g, opacity, bbox),
                        // Pattern strokes are not supported: no paint.
                        Kind::Pattern(_) => return None,
                        _ => {}
                    }
                }
                match fallback {
                    Some(fb) => self.make_paint(fb, opacity, props, bbox),
                    None => None,
                }
            }
        }
    }

    fn gradient_paint(&self, g: &Gradient, opacity: f32, bbox: Rect) -> Option<Paint> {
        if g.stops.is_empty() {
            return None;
        }
        let mut stops: Vec<ColorStop> = g.stops.iter().map(|s| ColorStop { offset: s.offset, color: s.color.scale_alpha(opacity) }).collect();
        if stops.len() == 1 {
            return Some(Paint::Solid(stops[0].color));
        }
        // SVG offsets lie in 0..=1 and the gradient vector is the whole
        // period: pad the ends so the painted domain is exactly 0..=1.
        for s in &mut stops {
            s.offset = s.offset.clamp(0.0, 1.0);
        }
        if stops[0].offset > 0.0 {
            stops.insert(0, ColorStop { offset: 0.0, color: stops[0].color });
        }
        if let Some(&last) = stops.last().filter(|s| s.offset < 1.0) {
            stops.push(ColorStop { offset: 1.0, color: last.color });
        }
        let units_bbox = g.units_bbox.unwrap_or(true);
        let gt = g.transform.unwrap_or(rabbithole_gfx::IDENTITY);
        // Coordinates: in bbox units a bare number is a fraction, a percent is a percent.
        let resolve = |l: Option<Length>, default: f32, reference: f32| -> f32 {
            match l {
                None => {
                    if units_bbox {
                        default
                    } else {
                        default * reference
                    }
                }
                Some(Length::Px(v)) => v,
                Some(Length::Percent(p)) => {
                    if units_bbox {
                        p / 100.0
                    } else {
                        reference * p / 100.0
                    }
                }
            }
        };
        let (vw, vh) = self.viewport;
        // Map a gradient-space point to user space.
        let to_user = |x: f32, y: f32| -> Point {
            let p = transform_point(&gt, Point { x, y });
            if units_bbox {
                Point { x: bbox.x + p.x * bbox.w, y: bbox.y + p.y * bbox.h }
            } else {
                p
            }
        };
        let spread = g.spread.unwrap_or(Spread::Pad);
        let repeat = spread != Spread::Pad;
        if g.linear {
            let x1 = resolve(g.x1, 0.0, vw);
            let y1 = resolve(g.y1, 0.0, vh);
            let x2 = resolve(g.x2, 1.0, vw);
            let y2 = resolve(g.y2, 0.0, vh);
            let mut start = to_user(x1, y1);
            let mut end = to_user(x2, y2);
            if spread == Spread::Reflect {
                // Mirror the stops over a doubled vector.
                end = Point { x: start.x + 2.0 * (end.x - start.x), y: start.y + 2.0 * (end.y - start.y) };
                stops = reflect_stops(&stops);
            }
            if (start.x - end.x).abs() < 1e-6 && (start.y - end.y).abs() < 1e-6 {
                // Degenerate: last stop color.
                return Some(Paint::Solid(stops.last().unwrap().color));
            }
            if start.x.is_nan() || end.x.is_nan() {
                start = Point { x: bbox.x, y: bbox.y };
                end = Point { x: bbox.right(), y: bbox.y };
            }
            Some(Paint::Linear { start, end, stops, repeat })
        } else {
            let cx = resolve(g.cx, 0.5, vw);
            let cy = resolve(g.cy, 0.5, vh);
            let r = resolve(g.r, 0.5, ((vw * vw + vh * vh) / 2.0).sqrt());
            let center = to_user(cx, cy);
            let (mut rx, mut ry) = if units_bbox {
                let s = scale_factor(&gt);
                (r * bbox.w * s, r * bbox.h * s)
            } else {
                let s = scale_factor(&gt);
                (r * s, r * s)
            };
            if spread == Spread::Reflect {
                rx *= 2.0;
                ry *= 2.0;
                stops = reflect_stops(&stops);
            }
            if rx <= 0.0 || ry <= 0.0 {
                return Some(Paint::Solid(stops.last().unwrap().color));
            }
            Some(Paint::Radial { center, rx, ry, stops, repeat })
        }
    }

    /// Push a CSS `clip-path` shape (CSS Masking 1 §3.1: for SVG elements
    /// `content-box` / `padding-box` act as `fill-box`, `border-box` /
    /// `margin-box` as `stroke-box`).
    fn push_css_clip(&mut self, cp: &rabbithole_style::ClipPath, node: &Node, props: &Props) -> bool {
        use rabbithole_style::{ClipPath, ShapeBox};
        let viewport = Rect::new(0.0, 0.0, self.viewport.0, self.viewport.1);
        let fill = node.bbox.unwrap_or(viewport);
        let reference = |b: ShapeBox| match b {
            ShapeBox::Content | ShapeBox::Padding | ShapeBox::Fill => fill,
            ShapeBox::View => viewport,
            ShapeBox::Border | ShapeBox::HalfBorder | ShapeBox::Margin | ShapeBox::Stroke => {
                let e = if props.stroke != PaintSpec::None { props.stroke_width.max(0.0) / 2.0 } else { 0.0 };
                Rect::new(fill.x - e, fill.y - e, fill.w + 2.0 * e, fill.h + 2.0 * e)
            }
        };
        let (path, rule) = match cp {
            ClipPath::Url(id) => return self.push_clip(id, fill),
            ClipPath::Box(b) => (rabbithole_gfx::path::rect_path(reference(*b)), FillRule::NonZero),
            ClipPath::Shape(shape, b) => rabbithole_style::shapes::basic_shape_path(shape, reference(*b)),
        };
        self.canvas.push_clip_path(&path, rule);
        true
    }

    /// Push a clip from a `<clipPath>`; returns whether a clip was pushed.
    fn push_clip(&mut self, id: &str, bbox: Rect) -> bool {
        let Some((mut path, rule, units_bbox)) = clip_outline(self.tree, id, None) else { return false };
        if units_bbox {
            let m = [bbox.w, 0.0, 0.0, bbox.h, bbox.x, bbox.y];
            path = transform_path(&path, &m);
        }
        self.canvas.push_clip_path(&path, rule);
        true
    }

    fn paint_text(&mut self, text: &crate::text::TextNode, base: &Props) {
        let diag = self.diag();
        let mut cursor_x = 0.0f32;
        let mut cursor_y = 0.0f32;
        let mut i = 0;
        let chunks = &text.chunks;
        while i < chunks.len() {
            // A positioned run: this chunk and the following ones without an explicit x.
            let mut j = i + 1;
            while j < chunks.len() && chunks[j].x.is_none() {
                j += 1;
            }
            let run = &chunks[i..j];
            let run_props: Vec<Props> = run.iter().map(|c| base.inherit(&c.spec, diag)).collect();
            let run_fonts: Vec<_> = run_props.iter().map(|p| self.fonts.select(p.font_family, p.font_weight, p.font_italic)).collect();
            let widths: Vec<f32> = run
                .iter()
                .zip(run_fonts.iter().zip(run_props.iter()))
                .map(|(c, (f, p))| self.fonts.measure(*f, p.font_size, &c.text))
                .collect();
            let total: f32 = widths.iter().sum();
            if let Some(x) = run[0].x {
                cursor_x = x;
            }
            if let Some(y) = run[0].y {
                cursor_y = y;
            }
            let anchor_shift = match run_props[0].text_anchor {
                TextAnchor::Start => 0.0,
                TextAnchor::Middle => total / 2.0,
                TextAnchor::End => total,
            };
            cursor_x -= anchor_shift;
            for (k, c) in run.iter().enumerate() {
                cursor_x += c.dx;
                cursor_y += c.dy;
                if k > 0 {
                    if let Some(y) = c.y {
                        cursor_y = y;
                    }
                }
                let p = &run_props[k];
                if p.visible && !c.text.is_empty() {
                    let color = self.text_color(p);
                    if let Some(color) = color {
                        self.canvas.draw_text_ext(self.fonts, run_fonts[k], p.font_size, &c.text, cursor_x, cursor_y, color, 0.0, false, false);
                    }
                    if let Some(stroke) = self.make_paint(&p.stroke, p.stroke_opacity, p, Rect::new(cursor_x, cursor_y - p.font_size, widths[k], p.font_size)) {
                        // Stroked text: approximate with a slightly offset second draw.
                        if let Paint::Solid(sc) = stroke {
                            if color.is_none() {
                                self.canvas.draw_text_ext(self.fonts, run_fonts[k], p.font_size, &c.text, cursor_x, cursor_y, sc, 0.0, false, false);
                            }
                        }
                    }
                }
                cursor_x += widths[k];
            }
            i = j;
        }
    }

    fn text_color(&self, p: &Props) -> Option<Color> {
        match &p.fill {
            PaintSpec::None => None,
            PaintSpec::Color(c) => Some(c.scale_alpha(p.fill_opacity)),
            PaintSpec::CurrentColor => Some(p.color.scale_alpha(p.fill_opacity)),
            PaintSpec::Url(id, fb) => {
                if let Some(&idx) = self.tree.ids.get(id) {
                    if let Kind::Gradient(g) = &self.tree.nodes[idx].kind {
                        if let Some(s) = g.stops.first() {
                            return Some(s.color.scale_alpha(p.fill_opacity));
                        }
                    }
                }
                fb.as_ref().and_then(|f| p.paint_color(f)).map(|c| c.scale_alpha(p.fill_opacity))
            }
        }
    }
}

fn reflect_stops(stops: &[ColorStop]) -> Vec<ColorStop> {
    let mut out: Vec<ColorStop> = stops.iter().map(|s| ColorStop { offset: s.offset / 2.0, color: s.color }).collect();
    for s in stops.iter().rev() {
        out.push(ColorStop { offset: 1.0 - s.offset / 2.0, color: s.color });
    }
    out
}

/// Apply a matrix to every point of a path.
pub fn transform_path(path: &[PathSeg], m: &rabbithole_gfx::Matrix) -> Vec<PathSeg> {
    let t = |p: &Point| transform_point(m, *p);
    path.iter()
        .map(|s| match s {
            PathSeg::MoveTo(p) => PathSeg::MoveTo(t(p)),
            PathSeg::LineTo(p) => PathSeg::LineTo(t(p)),
            PathSeg::QuadTo(c, p) => PathSeg::QuadTo(t(c), t(p)),
            PathSeg::CubicTo(a, b, p) => PathSeg::CubicTo(t(a), t(b), t(p)),
            PathSeg::ArcTo { rx, ry, rotation, large_arc, sweep, to } => {
                let s = scale_factor(m);
                PathSeg::ArcTo { rx: rx * s, ry: ry * s, rotation: *rotation, large_arc: *large_arc, sweep: *sweep, to: t(to) }
            }
            PathSeg::Close => PathSeg::Close,
        })
        .collect()
}

/// Destination rect for an image of (iw, ih) inside `rect` per preserveAspectRatio.
pub fn fit_image(iw: f32, ih: f32, rect: Rect, par: crate::units::AspectRatio) -> Rect {
    if iw <= 0.0 || ih <= 0.0 {
        return rect;
    }
    match par.align {
        None => rect,
        Some((ax, ay)) => {
            let sx = rect.w / iw;
            let sy = rect.h / ih;
            let s = if par.slice { sx.max(sy) } else { sx.min(sy) };
            let w = iw * s;
            let h = ih * s;
            Rect::new(rect.x + (rect.w - w) * ax, rect.y + (rect.h - h) * ay, w, h)
        }
    }
}

/// The outline of the `<clipPath>` `id`: its shapes (in its user space,
/// its own `transform` applied), the clip rule and whether the units are
/// `objectBoundingBox`. An empty clip path clips everything. `transform`
/// replaces the clip path's own `transform` attribute (CSS `transform`).
pub(crate) fn clip_outline(tree: &Built, id: &str, transform: Option<rabbithole_gfx::Matrix>) -> Option<(Vec<PathSeg>, FillRule, bool)> {
    let &idx = tree.ids.get(id)?;
    let Kind::ClipPath { units_bbox } = tree.nodes[idx].kind else { return None };
    let clip_transform = transform.unwrap_or(tree.nodes[idx].transform);
    let mut path: Vec<PathSeg> = Vec::new();
    let mut rule = FillRule::NonZero;
    for &c in &tree.nodes[idx].children {
        collect_clip_shapes(tree, c, &clip_transform, &mut path, &mut rule, 0);
    }
    if path.is_empty() {
        path = vec![PathSeg::MoveTo(Point { x: 0.0, y: 0.0 }), PathSeg::Close];
    }
    Some((path, rule, units_bbox))
}

fn collect_clip_shapes(tree: &Built, idx: usize, m: &rabbithole_gfx::Matrix, out: &mut Vec<PathSeg>, rule: &mut FillRule, depth: usize) {
    if depth > 8 {
        return;
    }
    let node = &tree.nodes[idx];
    if node.spec.display_none {
        return;
    }
    let m = multiply(m, &node.transform);
    match &node.kind {
        Kind::Shape(p) => {
            if let Some(r) = node.spec.fill_rule {
                *rule = r;
            }
            out.extend(transform_path(p, &m));
        }
        Kind::Use { href, x, y, .. } => {
            if let Some(&t) = tree.ids.get(href) {
                let m2 = multiply(&m, &translate(*x, *y));
                collect_clip_shapes(tree, t, &m2, out, rule, depth + 1);
            }
        }
        Kind::Group => {
            for &c in &node.children {
                collect_clip_shapes(tree, c, &m, out, rule, depth + 1);
            }
        }
        Kind::Text(t) => {
            // Approximate text clips by their bounding box.
            if let Some(b) = t.approx_bbox() {
                out.extend(transform_path(&crate::parse::rect_path(b, 0.0, 0.0), &m));
            }
        }
        _ => {}
    }
}
