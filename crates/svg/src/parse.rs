//! Builds the renderable tree from a DOM subtree rooted at an `<svg>`.

use crate::style::{tokens_to_string, Specified, PRESENTATION_ATTRS};
use crate::text::{Chunk, TextNode};
use crate::units::{
    length_or, multiply, parse_aspect_ratio, parse_length, parse_number, parse_transform, parse_viewbox,
    transform_rect_bounds, AspectRatio, Length, ViewBox,
};
use rabbithole_css::{matches, MediaEnv, Specificity, StyleRule};
use rabbithole_dom::{Document, NodeData, NodeId};
use rabbithole_gfx::{parse_svg_path, Color, ColorStop, Image, Matrix, PathSeg, Point, Rect, IDENTITY};
use std::collections::HashMap;

/// Gradient `spreadMethod`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spread {
    Pad,
    Reflect,
    Repeat,
}

/// A linear or radial gradient definition. Coordinates keep their
/// `Length` form; `units_bbox` selects objectBoundingBox (default).
#[derive(Debug, Clone, PartialEq)]
pub struct Gradient {
    pub linear: bool,
    pub x1: Option<Length>,
    pub y1: Option<Length>,
    pub x2: Option<Length>,
    pub y2: Option<Length>,
    pub cx: Option<Length>,
    pub cy: Option<Length>,
    pub r: Option<Length>,
    pub fx: Option<Length>,
    pub fy: Option<Length>,
    pub units_bbox: Option<bool>,
    pub transform: Option<Matrix>,
    pub spread: Option<Spread>,
    pub stops: Vec<ColorStop>,
    pub href: Option<String>,
}

impl Default for Gradient {
    fn default() -> Self {
        Gradient {
            linear: true,
            x1: None,
            y1: None,
            x2: None,
            y2: None,
            cx: None,
            cy: None,
            r: None,
            fx: None,
            fy: None,
            units_bbox: None,
            transform: None,
            spread: None,
            stops: Vec::new(),
            href: None,
        }
    }
}

/// Node kinds of the renderable tree.
#[derive(Debug, Clone)]
pub enum Kind {
    /// The outermost `<svg>` (viewport handled by `render`).
    Root,
    /// Nested `<svg>`: viewport rect in the parent's user space.
    Svg {
        rect: Rect,
        viewbox: Option<ViewBox>,
        par: AspectRatio,
        overflow_visible: bool,
    },
    Group,
    Defs,
    Symbol {
        viewbox: Option<ViewBox>,
        par: AspectRatio,
        width: Option<f32>,
        height: Option<f32>,
        overflow_visible: bool,
    },
    Use {
        href: String,
        x: f32,
        y: f32,
        width: Option<f32>,
        height: Option<f32>,
    },
    Shape(Vec<PathSeg>),
    Text(TextNode),
    Image {
        image: Image,
        rect: Rect,
        par: AspectRatio,
    },
    Gradient(Gradient),
    ClipPath {
        units_bbox: bool,
    },
    /// `<pattern>`: approximated by a flat color.
    Pattern(Option<Color>),
}

#[derive(Debug, Clone)]
pub struct Node {
    pub kind: Kind,
    pub spec: Specified,
    pub transform: Matrix,
    pub children: Vec<usize>,
    pub id: Option<String>,
    /// Bounding box in this node's own user space (children transforms
    /// applied, this node's `transform` not), when known.
    pub bbox: Option<Rect>,
}

/// Result of building.
pub struct Built {
    pub nodes: Vec<Node>,
    pub root: usize,
    pub ids: HashMap<String, usize>,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub viewbox: Option<ViewBox>,
    pub par: AspectRatio,
    pub overflow_visible: bool,
}

const MAX_DEPTH: usize = 64;

struct Builder<'a> {
    doc: &'a Document,
    style_of: &'a dyn Fn(NodeId, &str) -> Option<String>,
    rules: Vec<&'a StyleRule>,
    nodes: Vec<Node>,
    ids: HashMap<String, usize>,
    /// DOM node → tree node (so `use` can reference anything built).
    built: HashMap<NodeId, usize>,
    viewport: (f32, f32),
}

/// Build the tree for the `<svg>` element `root`.
pub fn build(doc: &Document, root: NodeId, style_of: &dyn Fn(NodeId, &str) -> Option<String>) -> Option<Built> {
    if !doc.is_element(root, "svg") {
        return None;
    }
    // Internal stylesheets inside the SVG.
    let mut sheets = Vec::new();
    for n in doc.descendants(root) {
        if doc.is_element(n, "style") {
            sheets.push(rabbithole_css::parse_stylesheet(&doc.text_content(n)));
        }
    }
    let env = MediaEnv { width: 1000.0, height: 1000.0, screen: true, scripting: true };
    let mut rules = Vec::new();
    for s in &sheets {
        s.active_rules(&env, &mut rules);
    }

    // Root viewport: width/height attributes (px), else viewBox, else 300×150.
    let width = doc.attr(root, "width").and_then(parse_length).and_then(|l| match l {
        Length::Px(v) if v > 0.0 => Some(v),
        _ => None,
    });
    let height = doc.attr(root, "height").and_then(parse_length).and_then(|l| match l {
        Length::Px(v) if v > 0.0 => Some(v),
        _ => None,
    });
    let viewbox = doc.attr(root, "viewbox").and_then(parse_viewbox);
    let par = parse_aspect_ratio(doc.attr(root, "preserveaspectratio"));
    let vw = width.or(viewbox.map(|v| v.w)).unwrap_or(300.0);
    let vh = height.or(viewbox.map(|v| v.h)).unwrap_or(150.0);

    let mut b = Builder { doc, style_of, rules, nodes: Vec::new(), ids: HashMap::new(), built: HashMap::new(), viewport: (vw, vh) };
    let root_idx = b.build_node(root, 0, true)?;
    b.resolve_gradients();
    let overflow_visible = b.nodes[root_idx].spec.overflow_visible.unwrap_or(false);
    let mut built = Built { nodes: b.nodes, root: root_idx, ids: b.ids, width, height, viewbox, par, overflow_visible };
    compute_bboxes(&mut built);
    Some(built)
}

impl<'a> Builder<'a> {
    fn attr(&self, n: NodeId, name: &str) -> Option<&'a str> {
        self.doc.attr(n, name)
    }

    fn href(&self, n: NodeId) -> Option<String> {
        self.attr(n, "href")
            .or_else(|| self.attr(n, "xlink:href"))
            .map(|h| h.trim().to_string())
    }

    /// Collect the specified presentation properties of an element.
    fn specified(&self, n: NodeId) -> Specified {
        let mut spec = Specified::default();
        // 1. presentation attributes
        if let Some(el) = self.doc.element(n) {
            for (k, v) in &el.attrs {
                if PRESENTATION_ATTRS.contains(&k.as_str()) {
                    spec.apply(k, v);
                }
            }
        }
        // 2. internal stylesheets, by specificity then order
        let mut matched: Vec<(Specificity, usize, &StyleRule)> = Vec::new();
        for (order, rule) in self.rules.iter().enumerate() {
            let mut best: Option<Specificity> = None;
            for sel in &rule.selectors {
                if sel.pseudo_element.is_some() {
                    continue;
                }
                if matches(sel, self.doc, n) {
                    let s = sel.specificity();
                    if best.map(|b| s > b).unwrap_or(true) {
                        best = Some(s);
                    }
                }
            }
            if let Some(s) = best {
                matched.push((s, order, rule));
            }
        }
        matched.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut important: Vec<(&str, String)> = Vec::new();
        for (_, _, rule) in &matched {
            for d in &rule.declarations {
                let v = tokens_to_string(&d.value);
                if d.important {
                    important.push((d.name.as_str(), v));
                } else {
                    spec.apply(&d.name, &v);
                }
            }
        }
        // 3. page CSS
        for prop in PRESENTATION_ATTRS {
            if let Some(v) = (self.style_of)(n, prop) {
                spec.apply(prop, &v);
            }
        }
        // 4. inline style attribute
        if let Some(style) = self.attr(n, "style") {
            for d in rabbithole_css::parse_declaration_list(style) {
                spec.apply(&d.name, &tokens_to_string(&d.value));
            }
        }
        for (k, v) in important {
            spec.apply(k, &v);
        }
        spec
    }

    fn push(&mut self, dom: NodeId, node: Node) -> usize {
        let idx = self.nodes.len();
        if let Some(id) = &node.id {
            self.ids.entry(id.clone()).or_insert(idx);
        }
        self.nodes.push(node);
        self.built.insert(dom, idx);
        idx
    }

    fn element_children(&self, n: NodeId) -> Vec<NodeId> {
        self.doc.child_elements(n).collect()
    }

    fn build_children(&mut self, n: NodeId, depth: usize) -> Vec<usize> {
        let mut out = Vec::new();
        for c in self.element_children(n) {
            if let Some(i) = self.build_node(c, depth + 1, false) {
                out.push(i);
            }
        }
        out
    }

    /// Build one element (and its subtree). Returns None for elements we
    /// skip entirely.
    fn build_node(&mut self, n: NodeId, depth: usize, is_root: bool) -> Option<usize> {
        if depth > MAX_DEPTH {
            return None;
        }
        if let Some(&idx) = self.built.get(&n) {
            return Some(idx);
        }
        let tag = self.doc.tag(n)?.to_string();
        let spec = self.specified(n);
        let transform = self.attr(n, "transform").map(parse_transform).unwrap_or(IDENTITY);
        let id = self.attr(n, "id").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let (vw, vh) = self.viewport;
        let diag = ((vw * vw + vh * vh) / 2.0).sqrt();
        let mk = |kind: Kind, children: Vec<usize>| Node { kind, spec: spec.clone(), transform, children, id: id.clone(), bbox: None };

        let node = match tag.as_str() {
            "svg" => {
                if is_root {
                    let children = self.build_children(n, depth);
                    mk(Kind::Root, children)
                } else {
                    let x = length_or(self.attr(n, "x"), vw, 0.0);
                    let y = length_or(self.attr(n, "y"), vh, 0.0);
                    let w = length_or(self.attr(n, "width"), vw, vw);
                    let h = length_or(self.attr(n, "height"), vh, vh);
                    let viewbox = self.attr(n, "viewbox").and_then(parse_viewbox);
                    let par = parse_aspect_ratio(self.attr(n, "preserveaspectratio"));
                    let saved = self.viewport;
                    self.viewport = (viewbox.map(|v| v.w).unwrap_or(w), viewbox.map(|v| v.h).unwrap_or(h));
                    let children = self.build_children(n, depth);
                    self.viewport = saved;
                    let overflow_visible = spec.overflow_visible.unwrap_or(false);
                    mk(Kind::Svg { rect: Rect::new(x, y, w.max(0.0), h.max(0.0)), viewbox, par, overflow_visible }, children)
                }
            }
            "g" | "a" => {
                let children = self.build_children(n, depth);
                mk(Kind::Group, children)
            }
            "switch" => {
                // First renderable child wins.
                let mut children = Vec::new();
                for c in self.element_children(n) {
                    if let Some(i) = self.build_node(c, depth + 1, false) {
                        children.push(i);
                        break;
                    }
                }
                mk(Kind::Group, children)
            }
            "defs" => {
                let children = self.build_children(n, depth);
                mk(Kind::Defs, children)
            }
            "symbol" => {
                let viewbox = self.attr(n, "viewbox").and_then(parse_viewbox);
                let par = parse_aspect_ratio(self.attr(n, "preserveaspectratio"));
                let width = self.attr(n, "width").and_then(parse_length).map(|l| l.resolve(vw));
                let height = self.attr(n, "height").and_then(parse_length).map(|l| l.resolve(vh));
                let saved = self.viewport;
                if let Some(v) = viewbox {
                    self.viewport = (v.w, v.h);
                }
                let children = self.build_children(n, depth);
                self.viewport = saved;
                let overflow_visible = spec.overflow_visible.unwrap_or(false);
                mk(Kind::Symbol { viewbox, par, width, height, overflow_visible }, children)
            }
            "use" => {
                let href = self.href(n)?;
                let href = href.trim_start_matches('#').to_string();
                if href.is_empty() {
                    return None;
                }
                let x = length_or(self.attr(n, "x"), vw, 0.0);
                let y = length_or(self.attr(n, "y"), vh, 0.0);
                let width = self.attr(n, "width").and_then(parse_length).map(|l| l.resolve(vw));
                let height = self.attr(n, "height").and_then(parse_length).map(|l| l.resolve(vh));
                // Make sure the referenced element exists in the tree even
                // when it lives outside this <svg> (sprite sheets).
                if !self.ids.contains_key(&href) {
                    if let Some(target) = self.doc.element_by_id(&href) {
                        if !self.doc.descendants(target).any(|d| d == n) {
                            let _ = self.build_node(target, depth + 1, false);
                        }
                    }
                }
                mk(Kind::Use { href, x, y, width, height }, Vec::new())
            }
            "path" => {
                let d = self.attr(n, "d").unwrap_or("");
                let path = parse_svg_path(d);
                if path.is_empty() {
                    return None;
                }
                mk(Kind::Shape(path), Vec::new())
            }
            "rect" => {
                let x = length_or(self.attr(n, "x"), vw, 0.0);
                let y = length_or(self.attr(n, "y"), vh, 0.0);
                let w = length_or(self.attr(n, "width"), vw, 0.0);
                let h = length_or(self.attr(n, "height"), vh, 0.0);
                if w <= 0.0 || h <= 0.0 {
                    return None;
                }
                let rx_attr = self.attr(n, "rx").and_then(parse_length).map(|l| l.resolve(vw));
                let ry_attr = self.attr(n, "ry").and_then(parse_length).map(|l| l.resolve(vh));
                let rx = rx_attr.or(ry_attr).unwrap_or(0.0).clamp(0.0, w / 2.0);
                let ry = ry_attr.or(rx_attr).unwrap_or(0.0).clamp(0.0, h / 2.0);
                let path = rect_path(Rect::new(x, y, w, h), rx, ry);
                mk(Kind::Shape(path), Vec::new())
            }
            "circle" => {
                let cx = length_or(self.attr(n, "cx"), vw, 0.0);
                let cy = length_or(self.attr(n, "cy"), vh, 0.0);
                let r = length_or(self.attr(n, "r"), diag, 0.0);
                if r <= 0.0 {
                    return None;
                }
                mk(Kind::Shape(ellipse(cx, cy, r, r)), Vec::new())
            }
            "ellipse" => {
                let cx = length_or(self.attr(n, "cx"), vw, 0.0);
                let cy = length_or(self.attr(n, "cy"), vh, 0.0);
                let rx_attr = self.attr(n, "rx").and_then(parse_length).map(|l| l.resolve(vw));
                let ry_attr = self.attr(n, "ry").and_then(parse_length).map(|l| l.resolve(vh));
                let rx = rx_attr.or(ry_attr).unwrap_or(0.0);
                let ry = ry_attr.or(rx_attr).unwrap_or(0.0);
                if rx <= 0.0 || ry <= 0.0 {
                    return None;
                }
                mk(Kind::Shape(ellipse(cx, cy, rx, ry)), Vec::new())
            }
            "line" => {
                let x1 = length_or(self.attr(n, "x1"), vw, 0.0);
                let y1 = length_or(self.attr(n, "y1"), vh, 0.0);
                let x2 = length_or(self.attr(n, "x2"), vw, 0.0);
                let y2 = length_or(self.attr(n, "y2"), vh, 0.0);
                let path = vec![PathSeg::MoveTo(Point { x: x1, y: y1 }), PathSeg::LineTo(Point { x: x2, y: y2 })];
                let mut node = mk(Kind::Shape(path), Vec::new());
                // Lines have no interior.
                node.spec.fill = Some(crate::style::PaintSpec::None);
                node
            }
            "polyline" | "polygon" => {
                let pts = crate::style::parse_points(self.attr(n, "points").unwrap_or(""));
                if pts.len() < 2 {
                    return None;
                }
                let mut path = Vec::with_capacity(pts.len() + 1);
                for (i, (x, y)) in pts.iter().enumerate() {
                    let p = Point { x: *x, y: *y };
                    path.push(if i == 0 { PathSeg::MoveTo(p) } else { PathSeg::LineTo(p) });
                }
                if tag == "polygon" {
                    path.push(PathSeg::Close);
                }
                mk(Kind::Shape(path), Vec::new())
            }
            "text" => {
                let text = self.build_text(n);
                mk(Kind::Text(text), Vec::new())
            }
            "image" => {
                let href = self.href(n)?;
                let image = decode_data_url(&href)?;
                let x = length_or(self.attr(n, "x"), vw, 0.0);
                let y = length_or(self.attr(n, "y"), vh, 0.0);
                let w = self.attr(n, "width").and_then(parse_length).map(|l| l.resolve(vw)).unwrap_or(image.width as f32);
                let h = self.attr(n, "height").and_then(parse_length).map(|l| l.resolve(vh)).unwrap_or(image.height as f32);
                let par = parse_aspect_ratio(self.attr(n, "preserveaspectratio"));
                mk(Kind::Image { image, rect: Rect::new(x, y, w.max(0.0), h.max(0.0)), par }, Vec::new())
            }
            "lineargradient" | "radialgradient" => {
                let linear = tag == "lineargradient";
                let g = self.build_gradient(n, linear);
                mk(Kind::Gradient(g), Vec::new())
            }
            "clippath" => {
                let units_bbox = self.attr(n, "clippathunits").map(|u| u.eq_ignore_ascii_case("objectBoundingBox")).unwrap_or(false);
                let children = self.build_children(n, depth);
                mk(Kind::ClipPath { units_bbox }, children)
            }
            "pattern" => {
                // Average-ish color: first fill we can find among descendants.
                let mut color = None;
                for d in self.doc.descendants(n).skip(1) {
                    let s = self.specified(d);
                    if let Some(crate::style::PaintSpec::Color(c)) = s.fill {
                        color = Some(c);
                        break;
                    }
                }
                mk(Kind::Pattern(color), Vec::new())
            }
            // Never rendered / unsupported.
            "title" | "desc" | "metadata" | "style" | "script" | "filter" | "mask" | "marker" | "foreignobject"
            | "font" | "font-face" | "animate" | "animatetransform" | "animatemotion" | "set" | "view" | "cursor" => {
                return None;
            }
            _ => return None,
        };
        Some(self.push(n, node))
    }

    fn build_gradient(&mut self, n: NodeId, linear: bool) -> Gradient {
        let (vw, vh) = self.viewport;
        let mut g = Gradient { linear, ..Default::default() };
        let l = |name: &str| self.attr(n, name).and_then(parse_length);
        g.x1 = l("x1");
        g.y1 = l("y1");
        g.x2 = l("x2");
        g.y2 = l("y2");
        g.cx = l("cx");
        g.cy = l("cy");
        g.r = l("r");
        g.fx = l("fx");
        g.fy = l("fy");
        let _ = (vw, vh);
        g.units_bbox = self.attr(n, "gradientunits").map(|u| !u.eq_ignore_ascii_case("userSpaceOnUse"));
        g.transform = self.attr(n, "gradienttransform").map(parse_transform);
        g.spread = self.attr(n, "spreadmethod").map(|s| match s.to_ascii_lowercase().as_str() {
            "reflect" => Spread::Reflect,
            "repeat" => Spread::Repeat,
            _ => Spread::Pad,
        });
        g.href = self.href(n).map(|h| h.trim_start_matches('#').to_string());
        // Stops
        let mut last_offset = 0.0f32;
        for c in self.element_children(n) {
            if !self.doc.is_element(c, "stop") {
                continue;
            }
            let spec = self.specified(c);
            let offset_raw = self.attr(c, "offset").unwrap_or("0").trim();
            let mut offset = if let Some(p) = offset_raw.strip_suffix('%') {
                parse_number(p).unwrap_or(0.0) / 100.0
            } else {
                parse_number(offset_raw).unwrap_or(0.0)
            };
            offset = offset.clamp(0.0, 1.0).max(last_offset);
            last_offset = offset;
            let mut color = match spec.stop_color {
                Some(crate::color::ColorSpec::Color(c)) => c,
                Some(crate::color::ColorSpec::CurrentColor) => spec.color.unwrap_or(Color::BLACK),
                None => Color::BLACK,
            };
            if let Some(o) = spec.stop_opacity {
                color = color.scale_alpha(o);
            }
            g.stops.push(ColorStop { offset, color });
        }
        g
    }

    /// Fill in gradient attributes/stops inherited through `href`.
    fn resolve_gradients(&mut self) {
        let count = self.nodes.len();
        for i in 0..count {
            let Kind::Gradient(_) = &self.nodes[i].kind else { continue };
            let mut chain: Vec<usize> = Vec::new();
            let mut cur = i;
            let mut guard = 0;
            while let Kind::Gradient(g) = &self.nodes[cur].kind {
                let Some(h) = &g.href else { break };
                let Some(&next) = self.ids.get(h) else { break };
                if chain.contains(&next) || next == i || guard > 16 {
                    break;
                }
                chain.push(next);
                cur = next;
                guard += 1;
            }
            if chain.is_empty() {
                continue;
            }
            let parents: Vec<Gradient> = chain
                .iter()
                .filter_map(|&p| match &self.nodes[p].kind {
                    Kind::Gradient(g) => Some(g.clone()),
                    _ => None,
                })
                .collect();
            if let Kind::Gradient(g) = &mut self.nodes[i].kind {
                for p in parents {
                    if g.stops.is_empty() {
                        g.stops = p.stops.clone();
                    }
                    g.units_bbox = g.units_bbox.or(p.units_bbox);
                    g.transform = g.transform.or(p.transform);
                    g.spread = g.spread.or(p.spread);
                    if g.linear == p.linear {
                        g.x1 = g.x1.or(p.x1);
                        g.y1 = g.y1.or(p.y1);
                        g.x2 = g.x2.or(p.x2);
                        g.y2 = g.y2.or(p.y2);
                        g.cx = g.cx.or(p.cx);
                        g.cy = g.cy.or(p.cy);
                        g.r = g.r.or(p.r);
                        g.fx = g.fx.or(p.fx);
                        g.fy = g.fy.or(p.fy);
                    }
                }
            }
        }
    }

    /// Flatten `<text>` with its `<tspan>` children into positioned chunks.
    fn build_text(&self, n: NodeId) -> TextNode {
        let (vw, vh) = self.viewport;
        let preserve = self.attr(n, "xml:space").map(|s| s == "preserve").unwrap_or(false);
        let mut chunks = Vec::new();
        let base = Specified::default();
        self.collect_text(n, &base, true, preserve, &mut chunks, vw, vh, 0);
        // Trim leading/trailing whitespace of the whole text.
        if !preserve {
            if let Some(first) = chunks.first_mut() {
                let t = first.text.trim_start().to_string();
                first.text = t;
            }
            if let Some(last) = chunks.last_mut() {
                let t = last.text.trim_end().to_string();
                last.text = t;
            }
            chunks.retain(|c| !c.text.is_empty() || c.x.is_some() || c.y.is_some());
        }
        TextNode { chunks }
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_text(&self, n: NodeId, inherited: &Specified, is_text_root: bool, preserve: bool, out: &mut Vec<Chunk>, vw: f32, vh: f32, depth: usize) {
        if depth > 16 {
            return;
        }
        let own = if is_text_root { Specified::default() } else { self.specified(n) };
        let spec = merge_specified(inherited, &own);
        let x = self.attr(n, "x").and_then(|v| crate::units::parse_number_list(v).first().copied());
        let y = self.attr(n, "y").and_then(|v| crate::units::parse_number_list(v).first().copied());
        let dx = self.attr(n, "dx").and_then(|v| parse_length(v)).map(|l| l.resolve(vw)).unwrap_or(0.0);
        let dy = self.attr(n, "dy").and_then(|v| parse_length(v)).map(|l| l.resolve(vh)).unwrap_or(0.0);
        let mut first = true;
        for c in self.doc.children(n) {
            match &self.doc.node(c).data {
                NodeData::Text(t) => {
                    let text = if preserve { t.replace(['\n', '\r', '\t'], " ") } else { collapse_ws(t) };
                    if text.is_empty() {
                        continue;
                    }
                    let (cx, cy, cdx, cdy) = if first { (x, y, dx, dy) } else { (None, None, 0.0, 0.0) };
                    first = false;
                    out.push(Chunk { x: cx, y: cy, dx: cdx, dy: cdy, text, spec: spec.clone() });
                }
                NodeData::Element(el) => {
                    if el.tag == "tspan" || el.tag == "a" || el.tag == "textpath" {
                        let before = out.len();
                        self.collect_text(c, &spec, false, preserve, out, vw, vh, depth + 1);
                        if first && out.len() > before {
                            // Apply the text element's own position to the first chunk.
                            let ch = &mut out[before];
                            if ch.x.is_none() {
                                ch.x = x;
                            }
                            if ch.y.is_none() {
                                ch.y = y;
                            }
                            ch.dx += dx;
                            ch.dy += dy;
                            first = false;
                        }
                    }
                }
                _ => {}
            }
        }
        if first && is_text_root {
            // Empty text element: keep nothing.
        }
    }
}

fn merge_specified(base: &Specified, own: &Specified) -> Specified {
    let mut s = base.clone();
    macro_rules! take {
        ($($f:ident),*) => { $( if own.$f.is_some() { s.$f = own.$f.clone(); } )* };
    }
    take!(
        fill, fill_opacity, fill_rule, stroke, stroke_width, stroke_opacity, stroke_linecap, stroke_linejoin,
        stroke_miterlimit, stroke_dasharray, stroke_dashoffset, color, opacity, font_size, font_family, font_weight,
        font_italic, text_anchor, visibility, paint_order_stroke_first, non_scaling_stroke, clip_path, stop_color,
        stop_opacity, overflow_visible
    );
    s.display_none = base.display_none || own.display_none;
    s
}

fn collapse_ws(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut prev_space = false;
    for c in t.chars() {
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out
}

/// Rounded-rect path with elliptical corners (rx, ry).
pub fn rect_path(r: Rect, rx: f32, ry: f32) -> Vec<PathSeg> {
    if rx <= 0.0 || ry <= 0.0 {
        return vec![
            PathSeg::MoveTo(Point { x: r.x, y: r.y }),
            PathSeg::LineTo(Point { x: r.right(), y: r.y }),
            PathSeg::LineTo(Point { x: r.right(), y: r.bottom() }),
            PathSeg::LineTo(Point { x: r.x, y: r.bottom() }),
            PathSeg::Close,
        ];
    }
    let k = 0.5522848;
    let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
    vec![
        PathSeg::MoveTo(Point { x: x0 + rx, y: y0 }),
        PathSeg::LineTo(Point { x: x1 - rx, y: y0 }),
        PathSeg::CubicTo(Point { x: x1 - rx + rx * k, y: y0 }, Point { x: x1, y: y0 + ry - ry * k }, Point { x: x1, y: y0 + ry }),
        PathSeg::LineTo(Point { x: x1, y: y1 - ry }),
        PathSeg::CubicTo(Point { x: x1, y: y1 - ry + ry * k }, Point { x: x1 - rx + rx * k, y: y1 }, Point { x: x1 - rx, y: y1 }),
        PathSeg::LineTo(Point { x: x0 + rx, y: y1 }),
        PathSeg::CubicTo(Point { x: x0 + rx - rx * k, y: y1 }, Point { x: x0, y: y1 - ry + ry * k }, Point { x: x0, y: y1 - ry }),
        PathSeg::LineTo(Point { x: x0, y: y0 + ry }),
        PathSeg::CubicTo(Point { x: x0, y: y0 + ry - ry * k }, Point { x: x0 + rx - rx * k, y: y0 }, Point { x: x0 + rx, y: y0 }),
        PathSeg::Close,
    ]
}

/// Ellipse path from four cubic Béziers.
pub fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> Vec<PathSeg> {
    let k = 0.5522848;
    vec![
        PathSeg::MoveTo(Point { x: cx + rx, y: cy }),
        PathSeg::CubicTo(Point { x: cx + rx, y: cy + ry * k }, Point { x: cx + rx * k, y: cy + ry }, Point { x: cx, y: cy + ry }),
        PathSeg::CubicTo(Point { x: cx - rx * k, y: cy + ry }, Point { x: cx - rx, y: cy + ry * k }, Point { x: cx - rx, y: cy }),
        PathSeg::CubicTo(Point { x: cx - rx, y: cy - ry * k }, Point { x: cx - rx * k, y: cy - ry }, Point { x: cx, y: cy - ry }),
        PathSeg::CubicTo(Point { x: cx + rx * k, y: cy - ry }, Point { x: cx + rx, y: cy - ry * k }, Point { x: cx + rx, y: cy }),
        PathSeg::Close,
    ]
}

/// Decode a `data:` URL image (base64 or percent-encoded).
fn decode_data_url(href: &str) -> Option<Image> {
    let rest = href.strip_prefix("data:")?;
    let comma = rest.find(',')?;
    let meta = &rest[..comma];
    let data = &rest[comma + 1..];
    let bytes = if meta.to_ascii_lowercase().contains(";base64") {
        base64_decode(data)?
    } else {
        percent_decode(data)
    };
    Image::decode(&bytes)
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 0 && i + 2 <= b.len() - 1 {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Bounding box of a path from its points (control points included).
pub fn path_bbox(path: &[PathSeg]) -> Option<Rect> {
    let mut x0 = f32::MAX;
    let mut y0 = f32::MAX;
    let mut x1 = f32::MIN;
    let mut y1 = f32::MIN;
    let mut any = false;
    let mut add = |p: &Point| {
        any = true;
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    };
    for seg in path {
        match seg {
            PathSeg::MoveTo(p) | PathSeg::LineTo(p) => add(p),
            PathSeg::QuadTo(c, p) => {
                add(c);
                add(p);
            }
            PathSeg::CubicTo(c1, c2, p) => {
                add(c1);
                add(c2);
                add(p);
            }
            PathSeg::ArcTo { to, rx, ry, .. } => {
                add(to);
                add(&Point { x: to.x - rx.abs(), y: to.y - ry.abs() });
                add(&Point { x: to.x + rx.abs(), y: to.y + ry.abs() });
            }
            PathSeg::Close => {}
        }
    }
    if any {
        Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
    } else {
        None
    }
}

fn union(a: Option<Rect>, b: Option<Rect>) -> Option<Rect> {
    match (a, b) {
        (Some(a), Some(b)) => {
            let x0 = a.x.min(b.x);
            let y0 = a.y.min(b.y);
            let x1 = a.right().max(b.right());
            let y1 = a.bottom().max(b.bottom());
            Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
        }
        (Some(a), None) => Some(a),
        (None, b) => b,
    }
}

/// Compute bounding boxes bottom-up (with `use` references resolved).
fn compute_bboxes(b: &mut Built) {
    fn bbox_of(b: &Built, idx: usize, depth: usize, memo: &mut HashMap<usize, Option<Rect>>) -> Option<Rect> {
        if depth > 32 {
            return None;
        }
        if let Some(r) = memo.get(&idx) {
            return *r;
        }
        let node = &b.nodes[idx];
        let own = match &node.kind {
            Kind::Shape(p) => path_bbox(p),
            Kind::Text(t) => t.approx_bbox(),
            Kind::Image { rect, .. } => Some(*rect),
            Kind::Svg { rect, .. } => Some(*rect),
            Kind::Use { href, x, y, width, height } => {
                let target = b.ids.get(href).copied();
                let inner = match target {
                    Some(t) => match &b.nodes[t].kind {
                        Kind::Symbol { .. } | Kind::Svg { .. } => Some(Rect::new(0.0, 0.0, width.unwrap_or(100.0), height.unwrap_or(100.0))),
                        _ => bbox_of(b, t, depth + 1, memo).map(|r| transform_rect_bounds(&b.nodes[t].transform, r)),
                    },
                    None => None,
                };
                inner.map(|r| r.translate(*x, *y))
            }
            Kind::Gradient(_) | Kind::ClipPath { .. } | Kind::Pattern(_) | Kind::Defs => None,
            Kind::Root | Kind::Group | Kind::Symbol { .. } => None,
        };
        let mut acc = own;
        if !matches!(node.kind, Kind::Defs | Kind::Gradient(_) | Kind::ClipPath { .. } | Kind::Pattern(_)) {
            for &c in &node.children {
                if matches!(b.nodes[c].kind, Kind::Defs | Kind::Gradient(_) | Kind::ClipPath { .. } | Kind::Pattern(_) | Kind::Symbol { .. }) {
                    continue;
                }
                if let Some(r) = bbox_of(b, c, depth + 1, memo) {
                    let t = transform_rect_bounds(&b.nodes[c].transform, r);
                    acc = union(acc, Some(t));
                }
            }
        }
        memo.insert(idx, acc);
        acc
    }
    let mut memo = HashMap::new();
    for i in 0..b.nodes.len() {
        let r = bbox_of(b, i, 0, &mut memo);
        b.nodes[i].bbox = r;
    }
    let _ = multiply;
}
