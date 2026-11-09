//! Rendering of the tree onto a gfx `Canvas`.

use crate::parse::{Built, Gradient, Kind, Node, Spread};
use crate::style::{PaintSpec, Props, TextAnchor};
use crate::units::{is_identity, multiply, scale_factor, transform_point, translate, viewbox_transform, Length, ViewBox};
use rabbithole_gfx::{Canvas, Color, ColorStop, FillRule, FontContext, Paint, PathSeg, Point, Rect, StrokeStyle};

const MAX_USE_DEPTH: usize = 16;

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
            Some(id) => self.push_clip(id, node.bbox.unwrap_or(Rect::new(0.0, 0.0, self.viewport.0, self.viewport.1))),
            None => false,
        };

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

    fn paint_shape(&mut self, path: &[PathSeg], props: &Props, bbox: Rect) {
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
                        Kind::Pattern(color) => {
                            let c = color.unwrap_or(Color::rgb(128, 128, 128));
                            return Some(Paint::Solid(c.scale_alpha(opacity)));
                        }
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

    /// Push a clip from a `<clipPath>`; returns whether a clip was pushed.
    fn push_clip(&mut self, id: &str, bbox: Rect) -> bool {
        let Some(&idx) = self.tree.ids.get(id) else { return false };
        let Kind::ClipPath { units_bbox } = self.tree.nodes[idx].kind else { return false };
        let clip_transform = self.tree.nodes[idx].transform;
        let mut path: Vec<PathSeg> = Vec::new();
        let mut rule = FillRule::NonZero;
        let children = self.tree.nodes[idx].children.clone();
        for c in children {
            self.collect_clip_shapes(c, &clip_transform, &mut path, &mut rule, 0);
        }
        if path.is_empty() {
            // An empty clip path clips everything.
            path = vec![PathSeg::MoveTo(Point { x: 0.0, y: 0.0 }), PathSeg::Close];
        }
        if units_bbox {
            let m = [bbox.w, 0.0, 0.0, bbox.h, bbox.x, bbox.y];
            path = transform_path(&path, &m);
        }
        self.canvas.push_clip_path(&path, rule);
        true
    }

    fn collect_clip_shapes(&self, idx: usize, m: &rabbithole_gfx::Matrix, out: &mut Vec<PathSeg>, rule: &mut FillRule, depth: usize) {
        if depth > 8 {
            return;
        }
        let node = &self.tree.nodes[idx];
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
                if let Some(&t) = self.tree.ids.get(href) {
                    let m2 = multiply(&m, &translate(*x, *y));
                    self.collect_clip_shapes(t, &m2, out, rule, depth + 1);
                }
            }
            Kind::Group => {
                for &c in &node.children {
                    self.collect_clip_shapes(c, &m, out, rule, depth + 1);
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
