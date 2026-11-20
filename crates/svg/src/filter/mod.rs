//! SVG filter effects (Filter Effects 1): `<filter>` elements and their
//! primitives, evaluated on premultiplied RGBA buffers in device pixels,
//! plus the CSS filter functions.

pub mod css;
mod lighting;
mod noise;
pub mod ops;

pub use ops::{IRect, Pixels};

use crate::style::Specified;
use crate::units::{invert, parse_number, parse_number_list, transform_point, transform_rect_bounds, AspectRatio, Length};
use ops::{CompositeOp, EdgeMode};
use rabbithole_dom::{Document, NodeId};
use rabbithole_gfx::{Image, Matrix, Point, Rect};
use std::collections::HashMap;
use std::sync::Arc;

/// One entry of a `filter` value.
#[derive(Debug, Clone, PartialEq)]
pub enum FilterItem {
    /// `url(#id)`.
    Url(String),
    /// A CSS filter function.
    Css(rabbithole_style::FilterOp),
}

/// Parse a `filter` value: `None` when invalid, `Some(empty)` for `none`.
pub fn parse_filter_value(v: &str) -> Option<Vec<FilterItem>> {
    let t = v.trim();
    if t.eq_ignore_ascii_case("none") {
        return Some(Vec::new());
    }
    let toks = rabbithole_css::Tokenizer::tokenize(t);
    let comps = rabbithole_style::values::components(&toks);
    let ctx = rabbithole_style::values::LengthContext { font_size: 16.0, root_font_size: 16.0, viewport_w: 0.0, viewport_h: 0.0, ch: 0.5, ex: 0.5, ic: 1.0, lh: 19.2 };
    let ops = rabbithole_style::props::effects::parse_filter_list(&comps, rabbithole_gfx::Color::BLACK, &ctx)?;
    Some(
        ops.into_iter()
            .map(|op| match op {
                rabbithole_style::FilterOp::Url(id) => FilterItem::Url(id),
                other => FilterItem::Css(other),
            })
            .collect(),
    )
}

/// An input of a primitive.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// No `in`: the previous result (SourceGraphic for the first).
    Previous,
    SourceGraphic,
    SourceAlpha,
    /// Inputs without content here (`BackgroundImage`, `FillPaint`, …).
    Transparent,
    Named(String),
}

/// A transfer function of feComponentTransfer.
#[derive(Debug, Clone, PartialEq)]
pub enum Transfer {
    Identity,
    Table(Vec<f32>),
    Discrete(Vec<f32>),
    Linear(f32, f32),
    Gamma(f32, f32, f32),
}

impl Transfer {
    fn apply(&self, c: f32) -> f32 {
        match self {
            Transfer::Identity => c,
            Transfer::Table(v) => {
                let n = v.len() - 1;
                if n == 0 {
                    return v[0];
                }
                let k = ((c * n as f32).floor() as usize).min(n - 1);
                v[k] + (c - k as f32 / n as f32) * n as f32 * (v[k + 1] - v[k])
            }
            Transfer::Discrete(v) => {
                let n = v.len();
                v[((c * n as f32).floor() as usize).min(n - 1)]
            }
            Transfer::Linear(slope, intercept) => slope * c + intercept,
            Transfer::Gamma(amp, exp, off) => amp * c.powf(*exp) + off,
        }
    }
}

/// A decoded feImage picture (compared by identity).
#[derive(Debug, Clone)]
pub struct ImageRef(pub Arc<Image>);

impl PartialEq for ImageRef {
    fn eq(&self, o: &ImageRef) -> bool {
        Arc::ptr_eq(&self.0, &o.0)
    }
}

/// An SVG document shown by feImage (compared by identity).
#[derive(Debug, Clone)]
pub struct SvgRef(pub Arc<crate::SvgTree>);

impl PartialEq for SvgRef {
    fn eq(&self, o: &SvgRef) -> bool {
        Arc::ptr_eq(&self.0, &o.0)
    }
}

/// Parse an SVG `data:` URL (with an optional `#fragment`: a `<view>` id
/// or `svgView(viewBox(…))`).
fn svg_data_image(href: &str) -> Option<SvgRef> {
    let (url, frag) = match href.rfind('#') {
        Some(i) if !href[i..].contains(['<', '>', '\'']) => (&href[..i], Some(&href[i + 1..])),
        _ => (href, None),
    };
    let (meta, bytes) = crate::parse::data_url_bytes(url)?;
    if !meta.contains("svg") {
        return None;
    }
    let text = String::from_utf8_lossy(&bytes);
    let view_box = frag.and_then(|f| f.strip_prefix("svgView(viewBox(")).and_then(|r| r.split(')').next()).map(|v| parse_number_list(&v.replace(',', " ")));
    let view_id = frag.filter(|f| !f.starts_with("svgView("));
    let mut tree = crate::SvgTree::parse_view(&text, None, view_id)?;
    // feImage fits SVG images like `<image>`: the root's own
    // preserveAspectRatio does not apply.
    tree.built.par = crate::units::AspectRatio { align: None, slice: false };
    if let Some(v) = view_box.filter(|v| v.len() == 4 && v[2] > 0.0 && v[3] > 0.0) {
        tree.built.viewbox = Some(crate::units::ViewBox { x: v[0], y: v[1], w: v[2], h: v[3] });
    }
    Some(SvgRef(Arc::new(tree)))
}

/// A light source in user space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LightSpec {
    Distant { azimuth: f32, elevation: f32 },
    Point { x: f32, y: f32, z: f32 },
    Spot { x: f32, y: f32, z: f32, at: [f32; 3], exponent: f32, cone: Option<f32> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum PrimKind {
    /// Unpremultiplied sRGB colour with `flood-opacity` applied.
    Flood([f32; 4]),
    Offset(f32, f32),
    Blur(f32, f32),
    ColorMatrix(Box<[f32; 20]>),
    ComponentTransfer(Box<[Transfer; 4]>),
    Blend(u8),
    Composite(CompositeOp),
    Merge,
    Morphology { dilate: bool, rx: f32, ry: f32 },
    Tile,
    Turbulence { bx: f32, by: f32, octaves: u32, seed: f32, stitch: bool, fractal: bool },
    Displacement { scale: f32, xc: usize, yc: usize },
    Convolve { order: (usize, usize), kernel: Vec<f32>, divisor: f32, bias: f32, target: (usize, usize), edge: EdgeMode, preserve_alpha: bool },
    DropShadow { dx: f32, dy: f32, sx: f32, sy: f32, color: [f32; 4] },
    /// `href` (an element id) or a decoded image.
    Image { href: Option<String>, image: Option<ImageRef>, svg: Option<SvgRef>, par: AspectRatio },
    Lighting { surface_scale: f32, kd: f32, specular: Option<(f32, f32)>, color: [f32; 3], light: Option<LightSpec> },
    /// Passes its input through unchanged.
    PassThrough,
    /// Produces transparent black.
    Transparent,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Primitive {
    pub x: Option<Length>,
    pub y: Option<Length>,
    pub width: Option<Length>,
    pub height: Option<Length>,
    pub result: Option<String>,
    pub inputs: Vec<Input>,
    /// `color-interpolation-filters` is linearRGB.
    pub linear: bool,
    /// Uses `currentcolor` (Filter Effects 1 §tainted filter primitives).
    pub tainted: bool,
    pub kind: PrimKind,
}

/// A `<filter>` element.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterDef {
    /// `filterUnits` is objectBoundingBox (the default).
    pub units_bbox: bool,
    /// `primitiveUnits` is objectBoundingBox.
    pub prim_bbox: bool,
    pub x: Option<Length>,
    pub y: Option<Length>,
    pub width: Option<Length>,
    pub height: Option<Length>,
    pub primitives: Vec<Primitive>,
}

/// What the filter is applied in: `m` maps user space to the pixels of the
/// buffers, `bbox` is the element's bounding box (user space) and
/// `viewport` the size percentages of user-space lengths refer to.
pub struct FilterEnv<'a> {
    pub m: Matrix,
    pub bbox: Rect,
    pub viewport: (f32, f32),
    /// Renders the element `id` (feImage) in user space mapped by the
    /// given matrix into a buffer of the given size.
    pub element: Option<&'a dyn Fn(&str, &Matrix, usize, usize) -> Option<Pixels>>,
    /// Fonts for SVG images (feImage).
    pub fonts: Option<&'a rabbithole_gfx::FontContext>,
}

fn frac(l: Length) -> f32 {
    match l {
        Length::Px(v) => v,
        Length::Percent(p) => p / 100.0,
    }
}

impl FilterDef {
    /// The filter region in user space (`None`: nothing is rendered, e.g.
    /// objectBoundingBox units on an empty box).
    pub fn region(&self, bbox: Rect, viewport: (f32, f32)) -> Option<Rect> {
        let x = self.x.unwrap_or(Length::Percent(-10.0));
        let y = self.y.unwrap_or(Length::Percent(-10.0));
        let w = self.width.unwrap_or(Length::Percent(120.0));
        let h = self.height.unwrap_or(Length::Percent(120.0));
        let r = if self.units_bbox {
            if bbox.w <= 0.0 || bbox.h <= 0.0 {
                return None;
            }
            Rect::new(bbox.x + frac(x) * bbox.w, bbox.y + frac(y) * bbox.h, frac(w) * bbox.w, frac(h) * bbox.h)
        } else {
            Rect::new(x.resolve(viewport.0), y.resolve(viewport.1), w.resolve(viewport.0), h.resolve(viewport.1))
        };
        (r.w > 0.0 && r.h > 0.0).then_some(r)
    }

    /// Run the filter on `source` (premultiplied sRGB, the buffer's whole
    /// size). The result is sRGB, clipped to the filter region.
    pub fn apply(&self, source: &Pixels, env: &FilterEnv) -> Pixels {
        self.apply_tainted(source.clone(), env, false)
    }

    /// [`FilterDef::apply`] with a source that may be tainted (an earlier
    /// `drop-shadow()` used `currentcolor`).
    pub fn apply_tainted(&self, source: Pixels, env: &FilterEnv, source_tainted: bool) -> Pixels {
        let (w, h) = (source.w, source.h);
        let buf = IRect::new(0, 0, w as i32, h as i32);
        let Some(region_user) = self.region(env.bbox, env.viewport) else { return Pixels::new(w, h) };
        let region = to_irect(transform_rect_bounds(&env.m, region_user)).intersect(&buf);
        if self.primitives.is_empty() || region.is_empty() {
            return Pixels::new(w, h);
        }
        let sx = env.m[0].hypot(env.m[1]);
        let sy = env.m[2].hypot(env.m[3]);
        // Primitive lengths (user units or bbox fractions) to pixels.
        let plx = |v: f32| if self.prim_bbox { v * env.bbox.w } else { v } * sx;
        let ply = |v: f32| if self.prim_bbox { v * env.bbox.h } else { v } * sy;
        let mut src = source;
        ops::clip_to(&mut src, region);
        struct Res {
            px: Pixels,
            sub: IRect,
            linear: bool,
            tainted: bool,
        }
        // The last primitive reading each result: buffers are freed after it.
        let mut last_use: Vec<usize> = vec![0; self.primitives.len()];
        {
            let mut names: HashMap<&str, usize> = HashMap::new();
            for (i, p) in self.primitives.iter().enumerate() {
                for inp in &p.inputs {
                    let k = match inp {
                        Input::Named(n) => names.get(n.as_str()).copied().or(i.checked_sub(1)),
                        Input::Previous => i.checked_sub(1),
                        _ => None,
                    };
                    if let Some(k) = k {
                        last_use[k] = i;
                    }
                }
                if let Some(n) = &p.result {
                    names.insert(n.as_str(), i);
                }
            }
            if let Some(l) = last_use.last_mut() {
                *l = usize::MAX;
            }
        }
        let mut results: Vec<Res> = Vec::new();
        let mut names: HashMap<&str, usize> = HashMap::new();
        let mut source_alpha: Option<Pixels> = None;
        for (pi, p) in self.primitives.iter().enumerate() {
            // Resolve inputs to result indices (`None`: a standard input).
            let resolve = |i: &Input| -> Result<usize, Input> {
                match i {
                    Input::Named(n) => match names.get(n.as_str()) {
                        Some(&k) => Ok(k),
                        None => results.len().checked_sub(1).ok_or(Input::SourceGraphic),
                    },
                    Input::Previous => results.len().checked_sub(1).ok_or(Input::SourceGraphic),
                    other => Err(other.clone()),
                }
            };
            let ins: Vec<Result<usize, Input>> = p.inputs.iter().map(resolve).collect();
            // Default subregion: the union of the inputs' subregions, or
            // the filter region with standard inputs / no inputs.
            let mut default = IRect::default();
            let mut standard = ins.is_empty() || matches!(p.kind, PrimKind::Tile);
            for i in &ins {
                match i {
                    Ok(k) => default = default.union(&results[*k].sub),
                    Err(_) => standard = true,
                }
            }
            let default_user = if standard || default.is_empty() { region_user } else { from_irect(&env.m, default).unwrap_or(region_user) };
            let sub_user = self.subregion(p, default_user, env);
            let sub = if p.x.is_none() && p.y.is_none() && p.width.is_none() && p.height.is_none() && !standard && !default.is_empty() {
                default
            } else {
                to_irect(transform_rect_bounds(&env.m, sub_user))
            }
            .intersect(&region);
            let linear = p.linear;
            let tainted = p.tainted || ins.iter().any(|i| matches!(i, Ok(k) if results[*k].tainted) || source_tainted && matches!(i, Err(Input::SourceGraphic | Input::SourceAlpha)));
            let map_tainted = match ins.get(1).cloned().unwrap_or(Err(Input::SourceGraphic)) {
                Ok(k) => results[k].tainted,
                Err(Input::SourceGraphic | Input::SourceAlpha) => source_tainted,
                Err(_) => false,
            };
            let mut fetch = |i: &Result<usize, Input>| -> (Pixels, IRect) {
                let (mut px, sub, lin) = match i {
                    // A result read for the last time is moved, not copied.
                    Ok(k) if last_use[*k] == pi && ins.iter().filter(|j| matches!(j, Ok(x) if x == k)).count() == 1 => {
                        (std::mem::replace(&mut results[*k].px, Pixels { w, h, px: Vec::new() }), results[*k].sub, results[*k].linear)
                    }
                    Ok(k) => (results[*k].px.clone(), results[*k].sub, results[*k].linear),
                    Err(Input::SourceAlpha) => {
                        let a = source_alpha.get_or_insert_with(|| {
                            let mut a = src.clone();
                            for c in a.px.iter_mut() {
                                *c = [0.0, 0.0, 0.0, c[3]];
                            }
                            a
                        });
                        // (Colourless: the same in either colour space.)
                        (a.clone(), region, linear)
                    }
                    Err(Input::Transparent) => (Pixels::new(w, h), region, linear),
                    Err(_) => (src.clone(), region, false),
                };
                if lin != linear {
                    ops::convert_space(&mut px, sub, linear);
                }
                (px, sub)
            };
            let input = |k: usize| ins.get(k).cloned().unwrap_or(Err(Input::SourceGraphic));
            let mut out = match &p.kind {
                PrimKind::Flood(c) => {
                    let mut c = *c;
                    if linear {
                        for v in c.iter_mut().take(3) {
                            *v = ops::srgb_to_linear(*v);
                        }
                    }
                    Pixels { w, h, px: vec![[c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]]; w * h] }
                }
                PrimKind::Offset(dx, dy) => {
                    let (i, _) = fetch(&input(0));
                    ops::offset(&i, plx(*dx), ply(*dy))
                }
                PrimKind::Blur(bx, by) => {
                    let (mut i, _) = fetch(&input(0));
                    if *bx < 0.0 || *by < 0.0 {
                        // Negative deviations disable the primitive.
                    } else {
                        ops::blur(&mut i, sub, plx(*bx), ply(*by));
                    }
                    i
                }
                PrimKind::ColorMatrix(m) => {
                    let (mut i, _) = fetch(&input(0));
                    ops::color_matrix(&mut i, sub, m);
                    i
                }
                PrimKind::ComponentTransfer(f) => {
                    let (mut i, _) = fetch(&input(0));
                    ops::map_unpremultiplied(&mut i, sub, |c| [f[0].apply(c[0]), f[1].apply(c[1]), f[2].apply(c[2]), f[3].apply(c[3])]);
                    i
                }
                PrimKind::Blend(mode) => {
                    let (a, _) = fetch(&input(0));
                    let (b, _) = fetch(&input(1));
                    ops::blend(&a, &b, *mode, sub)
                }
                PrimKind::Composite(op) => {
                    let (a, _) = fetch(&input(0));
                    let (b, _) = fetch(&input(1));
                    ops::composite(&a, &b, *op, sub)
                }
                PrimKind::Merge => {
                    let mut acc = Pixels::new(w, h);
                    for i in &ins {
                        let (px, s) = fetch(i);
                        ops::over(&mut acc, &px, s.intersect(&sub));
                    }
                    acc
                }
                PrimKind::Morphology { dilate, rx, ry } => {
                    let (i, _) = fetch(&input(0));
                    let (rx, ry) = (plx(*rx), ply(*ry));
                    if rx < 0.0 || ry < 0.0 || (rx == 0.0 && ry == 0.0) {
                        i
                    } else {
                        // Radii in whole device pixels.
                        ops::morphology(&i, sub, rx.round().max(0.0) as usize, ry.round().max(0.0) as usize, *dilate)
                    }
                }
                PrimKind::Tile => {
                    let (i, s) = fetch(&input(0));
                    ops::tile(&i, s, sub)
                }
                PrimKind::Turbulence { bx, by, octaves, seed, stitch, fractal } => turbulence(w, h, sub, &env.m, sub_user, (*bx, *by), *octaves, *seed, *stitch, *fractal),
                PrimKind::Displacement { .. } if map_tainted => {
                    // A tainted displacement map is not used.
                    fetch(&input(0)).0
                }
                PrimKind::Displacement { scale, xc, yc } => {
                    let (i, _) = fetch(&input(0));
                    let (map, _) = fetch(&input(1));
                    ops::displace(&i, &map, sub, plx(*scale), ply(*scale), *xc, *yc)
                }
                PrimKind::Convolve { order, kernel, divisor, bias, target, edge, preserve_alpha } => {
                    // The input image spans the filter region (transparent
                    // outside its subregion): edge modes extend it there.
                    let (i, _) = fetch(&input(0));
                    ops::convolve(&i, sub, region, *order, kernel, *divisor, *bias, *target, *edge, *preserve_alpha)
                }
                PrimKind::DropShadow { dx, dy, sx: bx, sy: by, color } => {
                    let (i, _) = fetch(&input(0));
                    let mut c = *color;
                    if linear {
                        for v in c.iter_mut().take(3) {
                            *v = ops::srgb_to_linear(*v);
                        }
                    }
                    let mut sh = ops::offset(&i, plx(*dx), ply(*dy));
                    for px in sh.px.iter_mut() {
                        let a = px[3] * c[3];
                        *px = [c[0] * a, c[1] * a, c[2] * a, a];
                    }
                    if *bx >= 0.0 && *by >= 0.0 {
                        ops::blur(&mut sh, sub, plx(*bx), ply(*by));
                    }
                    ops::over(&mut sh, &i, sub);
                    sh
                }
                PrimKind::Image { href, image, svg, par } => {
                    let mut out = Pixels::new(w, h);
                    if let (Some(svg), Some(fonts)) = (svg, env.fonts) {
                        let (iw, ih) = match svg.0.intrinsic_size() {
                            (Some(a), Some(b), _) => (a, b),
                            (_, _, Some(vb)) => (vb.w, vb.h),
                            _ => (sub_user.w, sub_user.h),
                        };
                        let dest = crate::paint::fit_image(iw, ih, sub_user, *par);
                        let clip = sub.intersect(&to_irect(transform_rect_bounds(&env.m, dest)));
                        let clip = if par.slice { clip.intersect(&to_irect(transform_rect_bounds(&env.m, sub_user))) } else { clip };
                        if dest.w > 0.0 && dest.h > 0.0 && !clip.is_empty() {
                            // Only the visible part, at the buffer's resolution.
                            let m = crate::units::multiply(&crate::units::translate(-(clip.x0 as f32), -(clip.y0 as f32)), &crate::units::multiply(&env.m, &crate::units::translate(dest.x, dest.y)));
                            let (pw, ph) = (clip.width() as usize, clip.height() as usize);
                            let part = svg.0.rasterize_stretched(fonts, dest.w, dest.h, m, pw, ph);
                            for y in 0..ph {
                                let row = (clip.y0 as usize + y) * w + clip.x0 as usize;
                                out.px[row..row + pw].copy_from_slice(&part.px[y * pw..(y + 1) * pw]);
                            }
                        }
                    } else if let Some(img) = image {
                        draw_image(&mut out, &img.0, sub_user, *par, &env.m, sub);
                    } else if let (Some(id), Some(render)) = (href, env.element) {
                        if let Some(px) = render(id, &env.m, w, h) {
                            out = px;
                        }
                    }
                    if linear {
                        ops::convert_space(&mut out, sub, true);
                    }
                    out
                }
                PrimKind::Lighting { surface_scale, kd, specular, color, light } => {
                    let (i, _) = fetch(&input(0));
                    let mut c = *color;
                    if linear {
                        for v in c.iter_mut() {
                            *v = ops::srgb_to_linear(*v);
                        }
                    }
                    match light {
                        None => Pixels::new(w, h),
                        Some(l) => {
                            let map = |x: f32, y: f32| transform_point(&env.m, Point { x, y });
                            let sz = (sx * sy).sqrt();
                            let zs = |z: f32| if self.prim_bbox { z * ((env.bbox.w * env.bbox.w + env.bbox.h * env.bbox.h) / 2.0).sqrt() } else { z } * sz;
                            let pos = |x: f32, y: f32| {
                                if self.prim_bbox {
                                    map(env.bbox.x + x * env.bbox.w, env.bbox.y + y * env.bbox.h)
                                } else {
                                    map(x, y)
                                }
                            };
                            let dl = match *l {
                                LightSpec::Distant { azimuth, elevation } => lighting::Light::Distant { azimuth, elevation },
                                LightSpec::Point { x, y, z } => {
                                    let q = pos(x, y);
                                    lighting::Light::Point { x: q.x, y: q.y, z: zs(z) }
                                }
                                LightSpec::Spot { x, y, z, at, exponent, cone } => {
                                    let q = pos(x, y);
                                    let t = pos(at[0], at[1]);
                                    lighting::Light::Spot { x: q.x, y: q.y, z: zs(z), at: [t.x, t.y, zs(at[2])], exponent, cone }
                                }
                            };
                            lighting::light(&i, sub, *surface_scale, *kd, *specular, c, dl)
                        }
                    }
                }
                PrimKind::PassThrough => fetch(&input(0)).0,
                PrimKind::Transparent => Pixels::new(w, h),
            };
            ops::clip_to(&mut out, sub);
            if let Some(n) = &p.result {
                names.insert(n.as_str(), results.len());
            }
            results.push(Res { px: out, sub, linear, tainted });
            for (k, r) in results.iter_mut().enumerate() {
                if last_use[k] <= pi && !r.px.px.is_empty() {
                    r.px = Pixels { w, h, px: Vec::new() };
                }
            }
        }
        let Some(last) = results.pop() else { return Pixels::new(w, h) };
        let mut out = last.px;
        if last.linear {
            ops::convert_space(&mut out, last.sub, false);
        }
        out
    }

    /// The primitive subregion in user space; missing attributes come from
    /// `default`.
    fn subregion(&self, p: &Primitive, default: Rect, env: &FilterEnv) -> Rect {
        let b = env.bbox;
        let (vw, vh) = env.viewport;
        let px = |l: Length, horizontal: bool, offset: bool| -> f32 {
            if self.prim_bbox {
                let (o, s) = if horizontal { (b.x, b.w) } else { (b.y, b.h) };
                if offset {
                    o + frac(l) * s
                } else {
                    frac(l) * s
                }
            } else {
                l.resolve(if horizontal { vw } else { vh })
            }
        };
        Rect::new(
            p.x.map(|l| px(l, true, true)).unwrap_or(default.x),
            p.y.map(|l| px(l, false, true)).unwrap_or(default.y),
            p.width.map(|l| px(l, true, false)).unwrap_or(default.w),
            p.height.map(|l| px(l, false, false)).unwrap_or(default.h),
        )
    }
}

/// Pixel rectangle of a device rect (edges rounded to the nearest pixel).
fn to_irect(r: Rect) -> IRect {
    let c = |v: f32| v.round().clamp(-1.0e6, 1.0e6) as i32;
    IRect::new(c(r.x), c(r.y), c(r.x + r.w), c(r.y + r.h))
}

/// The user-space rect of pixel rect `r`.
fn from_irect(m: &Matrix, r: IRect) -> Option<Rect> {
    let inv = invert(m)?;
    Some(transform_rect_bounds(&inv, Rect::new(r.x0 as f32, r.y0 as f32, r.width() as f32, r.height() as f32)))
}

#[allow(clippy::too_many_arguments)]
fn turbulence(w: usize, h: usize, sub: IRect, m: &Matrix, sub_user: Rect, base: (f32, f32), octaves: u32, seed: f32, stitch: bool, fractal: bool) -> Pixels {
    let mut out = Pixels::new(w, h);
    let Some(inv) = invert(m) else { return out };
    if base.0 < 0.0 || base.1 < 0.0 {
        return out;
    }
    let gen = noise::Turbulence::new(seed);
    let tile = stitch.then_some([sub_user.x as f64, sub_user.y as f64, sub_user.w as f64, sub_user.h as f64]);
    for y in sub.y0.max(0)..sub.y1.min(h as i32) {
        for x in sub.x0.max(0)..sub.x1.min(w as i32) {
            let p = transform_point(&inv, Point { x: x as f32 + 0.5, y: y as f32 + 0.5 });
            let mut c = [0.0f32; 4];
            for (k, v) in c.iter_mut().enumerate() {
                let n = gen.turbulence(k, [p.x as f64, p.y as f64], base.0 as f64, base.1 as f64, octaves, fractal, tile);
                *v = (if fractal { (n * 255.0 + 255.0) / 2.0 } else { n * 255.0 } / 255.0).clamp(0.0, 1.0) as f32;
            }
            out.px[y as usize * w + x as usize] = [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]];
        }
    }
    out
}

/// Draw `img` into `rect` (user space, fitted with `par`), bilinear.
fn draw_image(out: &mut Pixels, img: &Image, rect: Rect, par: AspectRatio, m: &Matrix, clip: IRect) {
    let (iw, ih) = (img.width as usize, img.height as usize);
    if iw == 0 || ih == 0 {
        return;
    }
    let dest = crate::paint::fit_image(iw as f32, ih as f32, rect, par);
    let clip = if par.slice { clip.intersect(&to_irect(transform_rect_bounds(m, rect))) } else { clip };
    // Pixel → image coordinates.
    let to_img = crate::units::multiply(&[iw as f32 / dest.w, 0.0, 0.0, ih as f32 / dest.h, -dest.x * iw as f32 / dest.w, -dest.y * ih as f32 / dest.h], &match invert(m) {
        Some(i) => i,
        None => return,
    });
    let data = img.pixels();
    let get = |x: i32, y: i32| -> [f32; 4] {
        let x = x.clamp(0, iw as i32 - 1) as usize;
        let y = y.clamp(0, ih as i32 - 1) as usize;
        let o = (y * iw + x) * 4;
        let a = data[o + 3] as f32 / 255.0;
        [data[o] as f32 / 255.0 * a, data[o + 1] as f32 / 255.0 * a, data[o + 2] as f32 / 255.0 * a, a]
    };
    for y in clip.y0.max(0)..clip.y1.min(out.h as i32) {
        for x in clip.x0.max(0)..clip.x1.min(out.w as i32) {
            let p = transform_point(&to_img, Point { x: x as f32 + 0.5, y: y as f32 + 0.5 });
            if p.x < 0.0 || p.y < 0.0 || p.x > iw as f32 || p.y > ih as f32 {
                continue;
            }
            let (fx, fy) = (p.x - 0.5, p.y - 0.5);
            let (x0, y0) = (fx.floor() as i32, fy.floor() as i32);
            let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
            let (a, b, c, d) = (get(x0, y0), get(x0 + 1, y0), get(x0, y0 + 1), get(x0 + 1, y0 + 1));
            let o = &mut out.px[y as usize * out.w + x as usize];
            for k in 0..4 {
                o[k] = (a[k] * (1.0 - tx) + b[k] * tx) * (1.0 - ty) + (c[k] * (1.0 - tx) + d[k] * tx) * ty;
            }
        }
    }
}

// ------------------------------------------------------------------ parsing

/// Parse the `<filter>` element `n`. `attr` looks attributes up ignoring
/// case and `spec` gives an element's specified properties.
pub(crate) fn build_filter(doc: &Document, n: NodeId, attr: &dyn Fn(NodeId, &str) -> Option<String>, spec: &dyn Fn(NodeId) -> Specified, decode: &dyn Fn(&str) -> Option<Image>) -> FilterDef {
    // `href` inheritance of attributes and primitives.
    let mut chain = vec![n];
    let mut cur = n;
    for _ in 0..8 {
        let Some(h) = attr(cur, "href").or_else(|| attr(cur, "xlink:href")) else { break };
        let Some(id) = h.trim().strip_prefix('#') else { break };
        let Some(t) = doc.element_by_id(id) else { break };
        if !doc.tag(t).is_some_and(|g| g.eq_ignore_ascii_case("filter")) || chain.contains(&t) {
            break;
        }
        chain.push(t);
        cur = t;
    }
    let first = |name: &str| chain.iter().find_map(|&c| attr(c, name));
    let font_size = match spec(n).font_size {
        Some(Length::Px(v)) => v,
        _ => 16.0,
    };
    let len = |name: &str| first(name).and_then(|v| region_length(&v, font_size));
    let units_bbox = first("filterunits").map(|u| !u.trim().eq_ignore_ascii_case("userSpaceOnUse")).unwrap_or(true);
    let prim_bbox = first("primitiveunits").is_some_and(|u| u.trim().eq_ignore_ascii_case("objectBoundingBox"));
    let holder = chain.iter().copied().find(|&c| doc.child_elements(c).next().is_some()).unwrap_or(n);
    // color-interpolation-filters inherits from the filter's ancestors.
    let mut base_linear = true;
    {
        let mut anc = Vec::new();
        let mut c = Some(holder);
        while let Some(x) = c {
            anc.push(x);
            if doc.is_element(x, "svg") && doc.parent(x).is_none_or(|p| !doc.is_element(p, "svg") && doc.ancestor_or_self(p, |a| doc.is_element(a, "svg")).is_none()) {
                break;
            }
            c = doc.parent(x).filter(|&p| doc.element(p).is_some());
        }
        for &x in anc.iter().rev() {
            if let Some(l) = spec(x).cif_linear {
                base_linear = l;
            }
        }
    }
    let mut primitives = Vec::new();
    for c in doc.child_elements(holder) {
        let Some(tag) = doc.tag(c).map(|t| t.to_ascii_lowercase()) else { continue };
        let s = spec(c);
        let a = |name: &str| attr(c, name);
        let num = |name: &str, d: f32| a(name).and_then(|v| parse_number(&v)).unwrap_or(d);
        let pair = |name: &str, d: f32| -> (f32, f32) {
            match a(name).map(|v| parse_number_list(&v)) {
                Some(v) if v.len() == 1 => (v[0], v[0]),
                Some(v) if v.len() == 2 => (v[0], v[1]),
                Some(_) => (-1.0, -1.0),
                None => (d, d),
            }
        };
        let input = |name: &str| -> Input {
            match a(name).map(|v| v.trim().to_string()) {
                None => Input::Previous,
                Some(v) if v.is_empty() => Input::Previous,
                Some(v) => match v.as_str() {
                    "SourceGraphic" => Input::SourceGraphic,
                    "SourceAlpha" => Input::SourceAlpha,
                    "BackgroundImage" | "BackgroundAlpha" | "FillPaint" | "StrokePaint" => Input::Transparent,
                    _ => Input::Named(v),
                },
            }
        };
        let color_of = |cs: Option<crate::color::ColorSpec>, opacity: Option<f32>, default: rabbithole_gfx::Color| -> [f32; 4] {
            let col = match cs {
                Some(crate::color::ColorSpec::Color(c)) => c,
                Some(crate::color::ColorSpec::CurrentColor) => s.color.unwrap_or(rabbithole_gfx::Color::BLACK),
                None => default,
            };
            [col.r as f32 / 255.0, col.g as f32 / 255.0, col.b as f32 / 255.0, col.a as f32 / 255.0 * opacity.unwrap_or(1.0).clamp(0.0, 1.0)]
        };
        let current = |cs: &Option<crate::color::ColorSpec>| matches!(cs, Some(crate::color::ColorSpec::CurrentColor));
        let tainted = match tag.as_str() {
            "feflood" | "fedropshadow" => current(&s.flood_color),
            "fediffuselighting" | "fespecularlighting" => current(&s.lighting_color),
            _ => false,
        };
        let (kind, inputs) = match tag.as_str() {
            "feflood" => (PrimKind::Flood(color_of(s.flood_color.clone(), s.flood_opacity, rabbithole_gfx::Color::BLACK)), vec![]),
            "feoffset" => (PrimKind::Offset(num("dx", 0.0), num("dy", 0.0)), vec![input("in")]),
            "fegaussianblur" => {
                let (x, y) = pair("stddeviation", 0.0);
                (PrimKind::Blur(x, y), vec![input("in")])
            }
            "fecolormatrix" => {
                let ty = a("type").map(|t| t.trim().to_string()).unwrap_or_else(|| "matrix".into());
                let vals = a("values").map(|v| parse_number_list(&v));
                let m = match ty.as_str() {
                    "saturate" => {
                        let s = match &vals {
                            Some(v) if v.len() == 1 => v[0],
                            Some(v) if v.is_empty() && a("values").is_some_and(|t| t.trim().is_empty()) => 1.0,
                            None => 1.0,
                            _ => 1.0,
                        };
                        let s = s.max(0.0);
                        Some([
                            0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s, 0.0, 0.0,
                            0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s, 0.0, 0.0,
                            0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s, 0.0, 0.0,
                            0.0, 0.0, 0.0, 1.0, 0.0,
                        ])
                    }
                    "hueRotate" => {
                        let deg = match &vals {
                            Some(v) if v.len() == 1 => v[0],
                            _ => 0.0,
                        };
                        let (sn, cs) = deg.to_radians().sin_cos();
                        Some([
                            0.213 + cs * 0.787 - sn * 0.213, 0.715 - cs * 0.715 - sn * 0.715, 0.072 - cs * 0.072 + sn * 0.928, 0.0, 0.0,
                            0.213 - cs * 0.213 + sn * 0.143, 0.715 + cs * 0.285 + sn * 0.140, 0.072 - cs * 0.072 - sn * 0.283, 0.0, 0.0,
                            0.213 - cs * 0.213 - sn * 0.787, 0.715 - cs * 0.715 + sn * 0.715, 0.072 + cs * 0.928 + sn * 0.072, 0.0, 0.0,
                            0.0, 0.0, 0.0, 1.0, 0.0,
                        ])
                    }
                    "luminanceToAlpha" => Some([
                        0.0, 0.0, 0.0, 0.0, 0.0, //
                        0.0, 0.0, 0.0, 0.0, 0.0, //
                        0.0, 0.0, 0.0, 0.0, 0.0, //
                        0.2125, 0.7154, 0.0721, 0.0, 0.0,
                    ]),
                    "matrix" => match &vals {
                        Some(v) if v.len() == 20 => {
                            let mut m = [0.0; 20];
                            m.copy_from_slice(v);
                            Some(m)
                        }
                        _ => None,
                    },
                    _ => None,
                };
                (m.map(|m| PrimKind::ColorMatrix(Box::new(m))).unwrap_or(PrimKind::PassThrough), vec![input("in")])
            }
            "fecomponenttransfer" => {
                let mut f = [Transfer::Identity, Transfer::Identity, Transfer::Identity, Transfer::Identity];
                for fc in doc.child_elements(c) {
                    let k = match doc.tag(fc).map(|t| t.to_ascii_lowercase()).as_deref() {
                        Some("fefuncr") => 0,
                        Some("fefuncg") => 1,
                        Some("fefuncb") => 2,
                        Some("fefunca") => 3,
                        _ => continue,
                    };
                    let fa = |name: &str| attr(fc, name);
                    let fnum = |name: &str, d: f32| fa(name).and_then(|v| parse_number(&v)).unwrap_or(d);
                    let table = fa("tablevalues").map(|v| parse_number_list(&v)).unwrap_or_default();
                    f[k] = match fa("type").map(|t| t.trim().to_string()).as_deref() {
                        Some("table") if !table.is_empty() => Transfer::Table(table),
                        Some("discrete") if !table.is_empty() => Transfer::Discrete(table),
                        Some("linear") => Transfer::Linear(fnum("slope", 1.0), fnum("intercept", 0.0)),
                        Some("gamma") => Transfer::Gamma(fnum("amplitude", 1.0), fnum("exponent", 1.0), fnum("offset", 0.0)),
                        _ => Transfer::Identity,
                    };
                }
                (PrimKind::ComponentTransfer(Box::new(f)), vec![input("in")])
            }
            "feblend" => {
                let mode = a("mode").and_then(|m| rabbithole_gfx::blend_mode_index(m.trim())).filter(|&m| m <= 15).unwrap_or(0);
                (PrimKind::Blend(mode), vec![input("in"), input("in2")])
            }
            "fecomposite" => {
                let op = match a("operator").map(|o| o.trim().to_string()).as_deref() {
                    Some("in") => CompositeOp::In,
                    Some("out") => CompositeOp::Out,
                    Some("atop") => CompositeOp::Atop,
                    Some("xor") => CompositeOp::Xor,
                    Some("lighter") => CompositeOp::Lighter,
                    Some("arithmetic") => CompositeOp::Arithmetic(num("k1", 0.0), num("k2", 0.0), num("k3", 0.0), num("k4", 0.0)),
                    _ => CompositeOp::Over,
                };
                (PrimKind::Composite(op), vec![input("in"), input("in2")])
            }
            "femerge" => {
                let ins: Vec<Input> = doc
                    .child_elements(c)
                    .filter(|&m| doc.tag(m).is_some_and(|t| t.eq_ignore_ascii_case("femergenode")))
                    .map(|m| match attr(m, "in").map(|v| v.trim().to_string()) {
                        None => Input::Previous,
                        Some(v) if v.is_empty() => Input::Previous,
                        Some(v) => match v.as_str() {
                            "SourceGraphic" => Input::SourceGraphic,
                            "SourceAlpha" => Input::SourceAlpha,
                            "BackgroundImage" | "BackgroundAlpha" | "FillPaint" | "StrokePaint" => Input::Transparent,
                            _ => Input::Named(v),
                        },
                    })
                    .collect();
                (PrimKind::Merge, ins)
            }
            "femorphology" => {
                let dilate = a("operator").is_some_and(|o| o.trim() == "dilate");
                let (rx, ry) = pair("radius", 0.0);
                (PrimKind::Morphology { dilate, rx, ry }, vec![input("in")])
            }
            "fetile" => (PrimKind::Tile, vec![input("in")]),
            "feturbulence" => {
                // Negative frequencies are an error: the default (0) is used.
                let (bx, by) = match pair("basefrequency", 0.0) {
                    (x, y) if x < 0.0 || y < 0.0 => (0.0, 0.0),
                    p => p,
                };
                let octaves = a("numoctaves").and_then(|v| parse_number(&v)).unwrap_or(1.0);
                let kind = PrimKind::Turbulence {
                    bx,
                    by,
                    octaves: if octaves.fract() == 0.0 && octaves >= 0.0 { octaves.min(32.0) as u32 } else { 1 },
                    seed: num("seed", 0.0),
                    stitch: a("stitchtiles").is_some_and(|v| v.trim() == "stitch"),
                    fractal: a("type").is_some_and(|v| v.trim() == "fractalNoise"),
                };
                (kind, vec![])
            }
            "fedisplacementmap" => {
                let ch = |name: &str| match a(name).map(|v| v.trim().to_string()).as_deref() {
                    Some("R") => 0,
                    Some("G") => 1,
                    Some("B") => 2,
                    _ => 3,
                };
                (PrimKind::Displacement { scale: num("scale", 0.0), xc: ch("xchannelselector"), yc: ch("ychannelselector") }, vec![input("in"), input("in2")])
            }
            "feconvolvematrix" => (convolve_kind(&a), vec![input("in")]),
            "fedropshadow" => {
                let (sx, sy) = pair("stddeviation", 2.0);
                let color = color_of(s.flood_color.clone(), s.flood_opacity, rabbithole_gfx::Color::BLACK);
                (PrimKind::DropShadow { dx: num("dx", 2.0), dy: num("dy", 2.0), sx, sy, color }, vec![input("in")])
            }
            "feimage" => {
                let href = a("href").or_else(|| a("xlink:href")).map(|h| h.trim().to_string()).unwrap_or_default();
                let par = crate::units::parse_aspect_ratio(a("preserveaspectratio").as_deref());
                let kind = if let Some(id) = href.strip_prefix('#') {
                    PrimKind::Image { href: Some(id.to_string()), image: None, svg: None, par }
                } else if let Some(svg) = svg_data_image(&href) {
                    PrimKind::Image { href: None, image: None, svg: Some(svg), par }
                } else {
                    PrimKind::Image { href: None, image: decode(&href).map(|i| ImageRef(Arc::new(i))), svg: None, par }
                };
                (kind, vec![])
            }
            "fediffuselighting" | "fespecularlighting" => {
                let col = color_of(s.lighting_color.clone(), None, rabbithole_gfx::Color::WHITE);
                let light = doc.child_elements(c).find_map(|l| {
                    let la = |name: &str| attr(l, name).and_then(|v| parse_number(&v));
                    match doc.tag(l).map(|t| t.to_ascii_lowercase()).as_deref() {
                        Some("fedistantlight") => Some(LightSpec::Distant { azimuth: la("azimuth").unwrap_or(0.0).to_radians(), elevation: la("elevation").unwrap_or(0.0).to_radians() }),
                        Some("fepointlight") => Some(LightSpec::Point { x: la("x").unwrap_or(0.0), y: la("y").unwrap_or(0.0), z: la("z").unwrap_or(0.0) }),
                        Some("fespotlight") => Some(LightSpec::Spot {
                            x: la("x").unwrap_or(0.0),
                            y: la("y").unwrap_or(0.0),
                            z: la("z").unwrap_or(0.0),
                            at: [la("pointsatx").unwrap_or(0.0), la("pointsaty").unwrap_or(0.0), la("pointsatz").unwrap_or(0.0)],
                            exponent: la("specularexponent").unwrap_or(1.0),
                            cone: la("limitingconeangle").map(|d| d.abs().to_radians()),
                        }),
                        _ => None,
                    }
                });
                let surface_scale = num("surfacescale", 1.0);
                let kind = if tag == "fediffuselighting" {
                    PrimKind::Lighting { surface_scale, kd: num("diffuseconstant", 1.0), specular: None, color: [col[0], col[1], col[2]], light }
                } else {
                    let exp = num("specularexponent", 1.0);
                    let ks = num("specularconstant", 1.0);
                    if !(1.0..=128.0).contains(&exp) {
                        PrimKind::Transparent
                    } else {
                        PrimKind::Lighting { surface_scale, kd: 0.0, specular: Some((ks, exp)), color: [col[0], col[1], col[2]], light }
                    }
                };
                (kind, vec![input("in")])
            }
            _ => continue,
        };
        let l = |name: &str| a(name).and_then(|v| crate::units::parse_length(&v));
        primitives.push(Primitive {
            x: l("x"),
            y: l("y"),
            width: l("width"),
            height: l("height"),
            result: a("result").map(|r| r.trim().to_string()).filter(|r| !r.is_empty()),
            inputs,
            linear: s.cif_linear.unwrap_or(base_linear),
            tainted,
            kind,
        });
    }
    FilterDef { units_bbox, prim_bbox, x: len("x"), y: len("y"), width: len("width"), height: len("height"), primitives }
}

/// A filter region length: `em` against the filter's font size and
/// simple `calc()` sums (percentages and numbers both count as fractions
/// of the box in objectBoundingBox units).
fn region_length(v: &str, font_size: f32) -> Option<Length> {
    let t = v.trim();
    let lower = t.to_ascii_lowercase();
    if let Some(inner) = lower.strip_prefix("calc(").and_then(|r| r.strip_suffix(')')) {
        let (mut px, mut pct) = (0.0f32, 0.0f32);
        let mut sign = 1.0;
        for tok in inner.split_whitespace() {
            match tok {
                "+" => sign = 1.0,
                "-" => sign = -1.0,
                _ => match region_length(tok, font_size)? {
                    Length::Px(x) => px += sign * x,
                    Length::Percent(p) => pct += sign * p,
                },
            }
        }
        return Some(if pct == 0.0 {
            Length::Px(px)
        } else if px == 0.0 {
            Length::Percent(pct)
        } else {
            Length::Px(px + pct / 100.0)
        });
    }
    if let Some(n) = lower.strip_suffix("em").filter(|n| !n.ends_with('r')) {
        return parse_number(n).map(|x| Length::Px(x * font_size));
    }
    crate::units::parse_length(t)
}

fn convolve_kind(a: &dyn Fn(&str) -> Option<String>) -> PrimKind {
    let order = match a("order").map(|v| parse_number_list(&v)) {
        None => Some((3.0, 3.0)),
        Some(v) if v.len() == 1 => Some((v[0], v[0])),
        Some(v) if v.len() == 2 => Some((v[0], v[1])),
        _ => None,
    };
    let Some((ox, oy)) = order else { return PrimKind::Transparent };
    if ox < 1.0 || oy < 1.0 || ox.fract() != 0.0 || oy.fract() != 0.0 {
        return PrimKind::Transparent;
    }
    let (ox, oy) = (ox as usize, oy as usize);
    let kernel = a("kernelmatrix").map(|v| parse_number_list(&v)).unwrap_or_default();
    if kernel.len() != ox * oy {
        return PrimKind::Transparent;
    }
    let sum: f32 = kernel.iter().sum();
    let divisor = match a("divisor").and_then(|v| parse_number(&v)) {
        Some(d) if d != 0.0 => d,
        _ => {
            if sum == 0.0 {
                1.0
            } else {
                sum
            }
        }
    };
    let target = |name: &str, n: usize| -> Option<usize> {
        match a(name).and_then(|v| parse_number(&v)) {
            None => Some(n / 2),
            Some(t) if t >= 0.0 && (t as usize) < n && t.fract() == 0.0 => Some(t as usize),
            _ => None,
        }
    };
    let (Some(tx), Some(ty)) = (target("targetx", ox), target("targety", oy)) else { return PrimKind::Transparent };
    let edge = match a("edgemode").map(|v| v.trim().to_string()).as_deref() {
        Some("wrap") => EdgeMode::Wrap,
        Some("none") => EdgeMode::None,
        _ => EdgeMode::Duplicate,
    };
    let preserve_alpha = a("preservealpha").is_some_and(|v| v.trim() == "true");
    let bias = a("bias").and_then(|v| parse_number(&v)).unwrap_or(0.0);
    PrimKind::Convolve { order: (ox, oy), kernel, divisor, bias, target: (tx, ty), edge, preserve_alpha }
}

/// Run a filter chain (`url()` filters from `defs` and CSS functions) on
/// `source`. `scale` maps CSS px to buffer pixels for CSS functions.
/// Returns `None` when a `url()` reference is missing (the chain is then
/// ignored).
pub fn apply_chain(items: &[FilterItem], defs: &dyn Fn(&str) -> Option<Arc<FilterDef>>, source: Pixels, env: &FilterEnv, scale: f32) -> Option<Pixels> {
    let mut stages = Vec::with_capacity(items.len());
    for it in items {
        match it {
            FilterItem::Url(id) => stages.push(Err(defs(id)?)),
            FilterItem::Css(op) => stages.push(Ok(op)),
        }
    }
    let mut cur = source;
    let mut tainted = false;
    for s in stages {
        match s {
            Ok(op) => {
                tainted |= matches!(op, rabbithole_style::FilterOp::DropShadow { current: true, .. });
                css::apply(&mut cur, std::slice::from_ref(op), scale)
            }
            Err(def) => cur = def.apply_tainted(cur, env, tainted),
        }
    }
    Some(cur)
}

/// The user-space area a chain can paint into, from the element's
/// bounding box `bbox` and its painted bounds `painted`.
pub fn chain_bounds(items: &[FilterItem], defs: &dyn Fn(&str) -> Option<Arc<FilterDef>>, bbox: Rect, painted: Rect, viewport: (f32, f32)) -> Rect {
    let mut r = painted;
    for it in items {
        match it {
            FilterItem::Url(id) => {
                if let Some(reg) = defs(id).and_then(|d| d.region(bbox, viewport)) {
                    r = reg;
                }
            }
            FilterItem::Css(op) => {
                let e = css::spread(std::slice::from_ref(op));
                r = Rect::new(r.x - e, r.y - e, r.w + 2.0 * e, r.h + 2.0 * e);
            }
        }
    }
    r
}

#[cfg(test)]
mod tests;
