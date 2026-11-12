//! Native backing of the Canvas 2D API (`js/canvas.js` keeps the context
//! state and records paths; drawing happens here on RGBA surfaces with the
//! anti-aliased rasterizer, stroker and font engine of `rabbithole_gfx`).
//!
//! Surfaces are straight (non-premultiplied) RGBA like `ImageData`;
//! compositing implements the Porter-Duff operators and separable blend
//! modes of Compositing and Blending Level 1.

use crate::env::with_env;
use crate::host::*;
use rabbithole_gfx::path::{flatten, Polylines};
use rabbithole_gfx::raster::{box_blur, rasterize, Mask};
use rabbithole_gfx::{FillRule, FontContext, LineCap, LineJoin, PathSeg, Point, StrokeStyle};
use rabbithole_js::{JsResult, Realm, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub struct Surface {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    clips: HashMap<u32, Rc<Vec<u8>>>,
    next_clip: u32,
}

#[derive(Default)]
pub struct CanvasStore {
    surfaces: HashMap<u32, Surface>,
    next: u32,
}

thread_local! {
    static FALLBACK_FONTS: RefCell<Option<Rc<FontContext>>> = const { RefCell::new(None) };
}

fn fonts() -> Rc<FontContext> {
    if let Some(f) = with_env(|env| env.font_context()) {
        return f;
    }
    FALLBACK_FONTS.with(|f| f.borrow_mut().get_or_insert_with(|| Rc::new(FontContext::new())).clone())
}

fn store<R>(f: impl FnOnce(&mut CanvasStore) -> R) -> R {
    let st = state();
    let mut st = st.borrow_mut();
    f(&mut st.canvases)
}

fn dnum(realm: &mut Realm, v: &Value, k: &str) -> JsResult<f64> {
    let x = dict_value(realm, v, k)?;
    realm.to_number(x)
}

fn num(realm: &mut Realm, args: &[Value], i: usize) -> JsResult<f64> {
    realm.to_number(arg(args, i))
}

fn bytes(realm: &mut Realm, v: &Value) -> JsResult<Vec<u8>> {
    Ok(crate::fetch::body_bytes_from_value(realm, v)?.0)
}

/// Numbers of a Float64Array / array argument.
fn floats(realm: &mut Realm, v: &Value) -> JsResult<Vec<f64>> {
    let b = bytes(realm, v)?;
    Ok(b.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect())
}

// ------------------------------------------------------------------ paths

/// Decode recorded path ops: 0 move(x,y) 1 line(x,y) 2 quad(4) 3 cubic(6) 4 close.
fn decode_path(f: &[f64]) -> Vec<PathSeg> {
    let mut out = Vec::new();
    let mut i = 0;
    let p = |j: usize| Point { x: f[j] as f32, y: f[j + 1] as f32 };
    while i < f.len() {
        match f[i] as u32 {
            0 if i + 2 < f.len() => {
                out.push(PathSeg::MoveTo(p(i + 1)));
                i += 3;
            }
            1 if i + 2 < f.len() => {
                out.push(PathSeg::LineTo(p(i + 1)));
                i += 3;
            }
            2 if i + 4 < f.len() => {
                out.push(PathSeg::QuadTo(p(i + 1), p(i + 3)));
                i += 5;
            }
            3 if i + 6 < f.len() => {
                out.push(PathSeg::CubicTo(p(i + 1), p(i + 3), p(i + 5)));
                i += 7;
            }
            4 => {
                out.push(PathSeg::Close);
                i += 1;
            }
            _ => break,
        }
    }
    out
}

fn fill_polys(segs: &[PathSeg]) -> Polylines {
    let mut poly = flatten(segs, &rabbithole_gfx::IDENTITY, 0.2);
    // Fills close every subpath.
    for c in &mut poly.contours {
        c.closed = true;
    }
    poly
}

// ------------------------------------------------------------------ paint

/// How source colour is computed per device pixel.
enum Src {
    Solid([f32; 4]),
    Linear { inv: [f64; 6], x0: f64, y0: f64, x1: f64, y1: f64, stops: Vec<(f64, [f32; 4])> },
    Radial { inv: [f64; 6], x0: f64, y0: f64, r0: f64, x1: f64, y1: f64, r1: f64, stops: Vec<(f64, [f32; 4])> },
    Conic { inv: [f64; 6], x: f64, y: f64, angle: f64, stops: Vec<(f64, [f32; 4])> },
    Pattern { inv: [f64; 6], w: u32, h: u32, rgba: Rc<Vec<u8>>, rx: bool, ry: bool },
}

fn invert(m: [f64; 6]) -> [f64; 6] {
    let det = m[0] * m[3] - m[1] * m[2];
    if det == 0.0 {
        return [0.0; 6];
    }
    [m[3] / det, -m[1] / det, -m[2] / det, m[0] / det, (m[2] * m[5] - m[3] * m[4]) / det, (m[1] * m[4] - m[0] * m[5]) / det]
}

fn apply(m: &[f64; 6], x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

fn stop_color(stops: &[(f64, [f32; 4])], t: f64) -> [f32; 4] {
    if stops.is_empty() {
        return [0.0; 4];
    }
    if t <= stops[0].0 {
        return stops[0].1;
    }
    for w in stops.windows(2) {
        let (o0, c0) = w[0];
        let (o1, c1) = w[1];
        if t <= o1 {
            let f = if o1 == o0 { 1.0 } else { ((t - o0) / (o1 - o0)) as f32 };
            // Interpolate in premultiplied space.
            let a = c0[3] + (c1[3] - c0[3]) * f;
            if a <= 0.0 {
                return [0.0; 4];
            }
            let mut out = [0.0; 4];
            for k in 0..3 {
                out[k] = (c0[k] * c0[3] + (c1[k] * c1[3] - c0[k] * c0[3]) * f) / a;
            }
            out[3] = a;
            return out;
        }
    }
    stops[stops.len() - 1].1
}

impl Src {
    fn at(&self, x: i32, y: i32) -> [f32; 4] {
        let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
        match self {
            Src::Solid(c) => *c,
            Src::Linear { inv, x0, y0, x1, y1, stops } => {
                let (ux, uy) = apply(inv, px, py);
                let (dx, dy) = (x1 - x0, y1 - y0);
                let len = dx * dx + dy * dy;
                let t = if len == 0.0 { 0.0 } else { ((ux - x0) * dx + (uy - y0) * dy) / len };
                stop_color(stops, t)
            }
            Src::Radial { inv, x0, y0, r0, x1, y1, r1, stops } => {
                let (ux, uy) = apply(inv, px, py);
                // Solve for the largest t with r(t) >= 0 whose circle contains the point.
                let (cdx, cdy, dr) = (x1 - x0, y1 - y0, r1 - r0);
                let (pdx, pdy) = (ux - x0, uy - y0);
                let a = cdx * cdx + cdy * cdy - dr * dr;
                let b = pdx * cdx + pdy * cdy + r0 * dr;
                let c = pdx * pdx + pdy * pdy - r0 * r0;
                let t = if a.abs() < 1e-9 {
                    if b == 0.0 {
                        return [0.0; 4];
                    }
                    c / (2.0 * b)
                } else {
                    let disc = b * b - a * c;
                    if disc < 0.0 {
                        return [0.0; 4];
                    }
                    let s = disc.sqrt();
                    let t1 = (b + s) / a;
                    let t2 = (b - s) / a;
                    if r0 + t1 * dr >= 0.0 {
                        t1.max(if r0 + t2 * dr >= 0.0 { t2 } else { t1 })
                    } else if r0 + t2 * dr >= 0.0 {
                        t2
                    } else {
                        return [0.0; 4];
                    }
                };
                stop_color(stops, t)
            }
            Src::Conic { inv, x, y, angle, stops } => {
                let (ux, uy) = apply(inv, px, py);
                let mut t = ((uy - y).atan2(ux - x) - angle) / (2.0 * std::f64::consts::PI);
                t -= t.floor();
                stop_color(stops, t)
            }
            Src::Pattern { inv, w, h, rgba, rx, ry } => {
                let (ux, uy) = apply(inv, px, py);
                let (mut ix, mut iy) = (ux.floor() as i64, uy.floor() as i64);
                let (w, h) = (*w as i64, *h as i64);
                if *rx {
                    ix = ix.rem_euclid(w);
                }
                if *ry {
                    iy = iy.rem_euclid(h);
                }
                if ix < 0 || iy < 0 || ix >= w || iy >= h {
                    return [0.0; 4];
                }
                let i = ((iy * w + ix) * 4) as usize;
                [rgba[i] as f32 / 255.0, rgba[i + 1] as f32 / 255.0, rgba[i + 2] as f32 / 255.0, rgba[i + 3] as f32 / 255.0]
            }
        }
    }
}

fn read_stops(realm: &mut Realm, v: &Value) -> JsResult<Vec<(f64, [f32; 4])>> {
    let f = floats(realm, v)?;
    Ok(f.chunks_exact(5).map(|c| (c[0], [c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0, c[4] as f32])).collect())
}

/// Paint descriptor from JS: { kind, ... } (see canvas.js `paintOf`).
fn read_paint(realm: &mut Realm, p: &Value, matrix: [f64; 6]) -> JsResult<Src> {
    let kind = dict_str(realm, p, "kind")?.unwrap_or_default();
    let n = |realm: &mut Realm, k: &str| -> JsResult<f64> {
        let v = dict_value(realm, p, k)?;
        realm.to_number(v)
    };
    let inv = invert(matrix);
    Ok(match kind.as_str() {
        "linear" => Src::Linear { inv, x0: n(realm, "x0")?, y0: n(realm, "y0")?, x1: n(realm, "x1")?, y1: n(realm, "y1")?, stops: { let s = dict_value(realm, p, "stops")?; read_stops(realm, &s)? } },
        "radial" => Src::Radial { inv, x0: n(realm, "x0")?, y0: n(realm, "y0")?, r0: n(realm, "r0")?, x1: n(realm, "x1")?, y1: n(realm, "y1")?, r1: n(realm, "r1")?, stops: { let s = dict_value(realm, p, "stops")?; read_stops(realm, &s)? } },
        "conic" => Src::Conic { inv, x: n(realm, "x0")?, y: n(realm, "y0")?, angle: n(realm, "angle")?, stops: { let s = dict_value(realm, p, "stops")?; read_stops(realm, &s)? } },
        "pattern" => {
            let w = n(realm, "width")? as u32;
            let h = n(realm, "height")? as u32;
            let data = dict_value(realm, p, "data")?;
            let rgba = bytes(realm, &data)?;
            let rep = dict_str(realm, p, "repetition")?.unwrap_or_else(|| "repeat".into());
            // Pattern space = user space × pattern transform.
            let pm = dict_value(realm, p, "matrix")?;
            let pm = if pm.is_object() { let f = floats(realm, &pm)?; [f[0], f[1], f[2], f[3], f[4], f[5]] } else { [1.0, 0.0, 0.0, 1.0, 0.0, 0.0] };
            let full = [matrix[0] * pm[0] + matrix[2] * pm[1], matrix[1] * pm[0] + matrix[3] * pm[1], matrix[0] * pm[2] + matrix[2] * pm[3], matrix[1] * pm[2] + matrix[3] * pm[3], matrix[0] * pm[4] + matrix[2] * pm[5] + matrix[4], matrix[1] * pm[4] + matrix[3] * pm[5] + matrix[5]];
            if rgba.len() != (w * h * 4) as usize || w == 0 || h == 0 {
                Src::Solid([0.0; 4])
            } else {
                Src::Pattern { inv: invert(full), w, h, rgba: Rc::new(rgba), rx: rep == "repeat" || rep == "repeat-x", ry: rep == "repeat" || rep == "repeat-y" }
            }
        }
        _ => {
            let c = dict_value(realm, p, "color")?;
            let f = floats(realm, &c)?;
            Src::Solid([(f.first().copied().unwrap_or(0.0) / 255.0) as f32, (f.get(1).copied().unwrap_or(0.0) / 255.0) as f32, (f.get(2).copied().unwrap_or(0.0) / 255.0) as f32, f.get(3).copied().unwrap_or(1.0) as f32])
        }
    })
}

// ------------------------------------------------------------ compositing

/// Composite one pixel: source colour `s` (straight) with coverage-scaled
/// alpha onto destination `d` (straight u8).
fn composite(d: &mut [u8], s: [f32; 4], cov: f32, op: &str) {
    let sa = (s[3] * cov).clamp(0.0, 1.0);
    let da = d[3] as f32 / 255.0;
    let dc = [d[0] as f32 / 255.0, d[1] as f32 / 255.0, d[2] as f32 / 255.0];
    // Porter-Duff factors (Fa for source, Fb for destination).
    let (fa, fb, blend): (f32, f32, Option<fn(f32, f32) -> f32>) = match op {
        "source-over" => (1.0, 1.0 - sa, None),
        "source-in" => (da, 0.0, None),
        "source-out" => (1.0 - da, 0.0, None),
        "source-atop" => (da, 1.0 - sa, None),
        "destination-over" => (1.0 - da, 1.0, None),
        "destination-in" => (0.0, sa, None),
        "destination-out" => (0.0, 1.0 - sa, None),
        "destination-atop" => (1.0 - da, sa, None),
        "copy" => (1.0, 0.0, None),
        "xor" => (1.0 - da, 1.0 - sa, None),
        "lighter" => (1.0, 1.0, None),
        "multiply" => (1.0, 1.0 - sa, Some(|cb, cs| cb * cs)),
        "screen" => (1.0, 1.0 - sa, Some(|cb, cs| cb + cs - cb * cs)),
        "overlay" => (1.0, 1.0 - sa, Some(|cb, cs| if cb <= 0.5 { 2.0 * cb * cs } else { 1.0 - 2.0 * (1.0 - cb) * (1.0 - cs) })),
        "darken" => (1.0, 1.0 - sa, Some(|cb: f32, cs: f32| cb.min(cs))),
        "lighten" => (1.0, 1.0 - sa, Some(|cb: f32, cs: f32| cb.max(cs))),
        "color-dodge" => (1.0, 1.0 - sa, Some(|cb, cs| if cb == 0.0 { 0.0 } else if cs >= 1.0 { 1.0 } else { (cb / (1.0 - cs)).min(1.0) })),
        "color-burn" => (1.0, 1.0 - sa, Some(|cb, cs| if cb >= 1.0 { 1.0 } else if cs <= 0.0 { 0.0 } else { 1.0 - ((1.0 - cb) / cs).min(1.0) })),
        "hard-light" => (1.0, 1.0 - sa, Some(|cb, cs| if cs <= 0.5 { 2.0 * cb * cs } else { 1.0 - 2.0 * (1.0 - cb) * (1.0 - cs) })),
        "soft-light" => (1.0, 1.0 - sa, Some(|cb: f32, cs: f32| {
            if cs <= 0.5 {
                cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb)
            } else {
                let d = if cb <= 0.25 { ((16.0 * cb - 12.0) * cb + 4.0) * cb } else { cb.sqrt() };
                cb + (2.0 * cs - 1.0) * (d - cb)
            }
        })),
        "difference" => (1.0, 1.0 - sa, Some(|cb: f32, cs: f32| (cb - cs).abs())),
        "exclusion" => (1.0, 1.0 - sa, Some(|cb, cs| cb + cs - 2.0 * cb * cs)),
        // Non-separable modes approximated by normal blending.
        _ => (1.0, 1.0 - sa, None),
    };
    let mut cs = [s[0], s[1], s[2]];
    if let Some(f) = blend {
        // Mix the blended colour by the backdrop alpha (B(Cb, Cs) where backdrop exists).
        for k in 0..3 {
            cs[k] = (1.0 - da) * s[k] + da * f(dc[k], s[k]);
        }
    }
    let oa = (sa * fa + da * fb).min(1.0);
    if oa <= 0.0 {
        d.copy_from_slice(&[0, 0, 0, 0]);
        return;
    }
    for k in 0..3 {
        let premul = cs[k] * sa * fa + dc[k] * da * fb;
        d[k] = ((premul / oa).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    }
    d[3] = (oa * 255.0 + 0.5) as u8;
}

/// Ops that affect pixels outside the source shape (where coverage is 0).
fn unbounded(op: &str) -> bool {
    matches!(op, "source-in" | "source-out" | "destination-in" | "destination-atop" | "copy")
}

struct DrawParams {
    alpha: f32,
    op: String,
    clip: Option<Rc<Vec<u8>>>,
    shadow: Option<([f32; 4], f64, f64, f64)>,
}

fn read_params(realm: &mut Realm, s: &Surface, v: &Value) -> JsResult<DrawParams> {
    let alpha = dict_value(realm, v, "alpha")?;
    let alpha = if alpha.is_undefined() { 1.0 } else { realm.to_number(alpha)? as f32 };
    let op = dict_str(realm, v, "op")?.unwrap_or_else(|| "source-over".into());
    let clip = dict_value(realm, v, "clip")?;
    let clip = if clip.is_nullish() { None } else { let id = realm.to_number(clip)? as u32; s.clips.get(&id).cloned() };
    let sc = dict_value(realm, v, "shadowColor")?;
    let shadow = if sc.is_object() {
        let c = floats(realm, &sc)?;
        let blur = dnum(realm, v, "shadowBlur")?;
        let ox = dnum(realm, v, "shadowOffsetX")?;
        let oy = dnum(realm, v, "shadowOffsetY")?;
        if c.len() >= 4 && c[3] > 0.0 && (blur > 0.0 || ox != 0.0 || oy != 0.0) {
            Some(([(c[0] / 255.0) as f32, (c[1] / 255.0) as f32, (c[2] / 255.0) as f32, c[3] as f32], blur, ox, oy))
        } else {
            None
        }
    } else {
        None
    };
    Ok(DrawParams { alpha, op, clip, shadow })
}

/// Paint `src` through a coverage mask onto the surface.
fn paint_mask(s: &mut Surface, mask: &Mask, src: &Src, p: &DrawParams) {
    let w = s.width as i32;
    let h = s.height as i32;
    if let Some((color, blur, ox, oy)) = p.shadow {
        // Shadow: the mask offset and blurred, in the shadow colour.
        let pad = (blur * 1.5).ceil() as i32 + 2;
        let mw = mask.width as i32 + 2 * pad;
        let mh = mask.height as i32 + 2 * pad;
        if mw > 0 && mh > 0 && (mw as i64) * (mh as i64) < 64_000_000 {
            let mut data = vec![0u8; (mw * mh) as usize];
            for y in 0..mask.height as i32 {
                for x in 0..mask.width as i32 {
                    data[((y + pad) * mw + x + pad) as usize] = mask.data[(y * mask.width as i32 + x) as usize];
                }
            }
            let r = rabbithole_gfx::raster::blur_radius_for_sigma((blur / 2.0) as f32);
            box_blur(&mut data, mw as usize, mh as usize, r, 3);
            let sh = Mask { x0: mask.x0 - pad + ox.round() as i32, y0: mask.y0 - pad + oy.round() as i32, width: mw as u32, height: mh as u32, data };
            let sp = DrawParams { alpha: p.alpha, op: p.op.clone(), clip: p.clip.clone(), shadow: None };
            paint_mask(s, &sh, &Src::Solid(color), &sp);
        }
    }
    let ub = unbounded(&p.op);
    let (x0, y0, x1, y1) = if ub { (0, 0, w, h) } else { (mask.x0.max(0), mask.y0.max(0), (mask.x0 + mask.width as i32).min(w), (mask.y0 + mask.height as i32).min(h)) };
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y * w + x) as usize;
            if let Some(c) = &p.clip {
                if c[i] == 0 {
                    continue;
                }
            }
            let mut cov = mask.get(x, y) as f32 / 255.0;
            if let Some(c) = &p.clip {
                cov *= c[i] as f32 / 255.0;
            }
            if cov <= 0.0 && !ub {
                continue;
            }
            let c = if cov > 0.0 { src.at(x, y) } else { [0.0; 4] };
            let mut c = c;
            c[3] *= p.alpha;
            composite(&mut s.rgba[i * 4..i * 4 + 4], c, cov, &p.op);
        }
    }
}

fn limit(s: &Surface) -> (i32, i32, i32, i32) {
    (0, 0, s.width as i32, s.height as i32)
}

// ------------------------------------------------------------- natives

fn canvas_new(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let w = num(realm, args, 0)?.max(0.0) as u32;
    let h = num(realm, args, 1)?.max(0.0) as u32;
    let id = store(|st| {
        st.next += 1;
        let id = st.next;
        st.surfaces.insert(id, Surface { width: w, height: h, rgba: vec![0; (w as usize) * (h as usize) * 4], clips: HashMap::new(), next_clip: 0 });
        id
    });
    Ok(Value::Number(id as f64))
}

fn with_surface<R>(realm: &mut Realm, id: &Value, f: impl FnOnce(&mut Realm, &mut Surface) -> JsResult<R>) -> JsResult<R> {
    let id = realm.to_number(id.clone())? as u32;
    let surface = store(|st| st.surfaces.remove(&id));
    let Some(mut s) = surface else { return Err(realm.throw_type_error("canvas surface is gone")) };
    let r = f(realm, &mut s);
    store(|st| st.surfaces.insert(id, s));
    r
}

fn canvas_resize(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let w = num(realm, args, 1)?.max(0.0) as u32;
    let h = num(realm, args, 2)?.max(0.0) as u32;
    with_surface(realm, &arg(args, 0), |_, s| {
        s.width = w;
        s.height = h;
        s.rgba = vec![0; (w as usize) * (h as usize) * 4];
        s.clips.clear();
        Ok(Value::Undefined)
    })
}

/// `canvasFreeClip(id, clipId)`: drop a clip mask no state refers to.
fn canvas_free_clip(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let clip = num(realm, args, 1)? as u32;
    with_surface(realm, &arg(args, 0), |_, s| {
        s.clips.remove(&clip);
        Ok(Value::Undefined)
    })
}

fn canvas_free(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = num(realm, args, 0)? as u32;
    store(|st| st.surfaces.remove(&id));
    Ok(Value::Undefined)
}

fn read_matrix(realm: &mut Realm, v: &Value) -> JsResult<[f64; 6]> {
    let f = floats(realm, v)?;
    Ok(if f.len() >= 6 { [f[0], f[1], f[2], f[3], f[4], f[5]] } else { [1.0, 0.0, 0.0, 1.0, 0.0, 0.0] })
}

/// `canvasFill(id, path, rule, paint, matrix, params)`
fn canvas_fill(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let f = floats(realm, &arg(args, 1))?;
    let even = arg_str(realm, args, 2)? == "evenodd";
    let m = read_matrix(realm, &arg(args, 4))?;
    let src = read_paint(realm, &arg(args, 3), m)?;
    with_surface(realm, &arg(args, 0), |realm, s| {
        let p = read_params(realm, s, &arg(args, 5))?;
        let segs = decode_path(&f);
        let mask = rasterize(&fill_polys(&segs), if even { FillRule::EvenOdd } else { FillRule::NonZero }, limit(s));
        paint_mask(s, &mask, &src, &p);
        Ok(Value::Undefined)
    })
}

fn read_stroke(realm: &mut Realm, v: &Value) -> JsResult<StrokeStyle> {
    let width = dnum(realm, v, "width")? as f32;
    let cap = match dict_str(realm, v, "cap")?.as_deref() {
        Some("round") => LineCap::Round,
        Some("square") => LineCap::Square,
        _ => LineCap::Butt,
    };
    let join = match dict_str(realm, v, "join")?.as_deref() {
        Some("round") => LineJoin::Round,
        Some("bevel") => LineJoin::Bevel,
        _ => LineJoin::Miter,
    };
    let miter_limit = dnum(realm, v, "miter")? as f32;
    let dashes = dict_value(realm, v, "dashes")?;
    let dashes: Vec<f32> = if dashes.is_object() { floats(realm, &dashes)?.into_iter().map(|d| d as f32).collect() } else { Vec::new() };
    let dash_offset = dnum(realm, v, "dashOffset")? as f32;
    Ok(StrokeStyle { width, cap, join, miter_limit, dashes, dash_offset })
}

/// `canvasStroke(id, path, stroke, paint, matrix, params)`: the path is in
/// device space; the stroke geometry is computed in user space through the
/// inverse transform so line widths follow the transform.
fn canvas_stroke(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let f = floats(realm, &arg(args, 1))?;
    let style = read_stroke(realm, &arg(args, 2))?;
    let m = read_matrix(realm, &arg(args, 4))?;
    let src = read_paint(realm, &arg(args, 3), m)?;
    with_surface(realm, &arg(args, 0), |realm, s| {
        let p = read_params(realm, s, &arg(args, 5))?;
        let segs = decode_path(&f);
        let inv = invert(m);
        let mut user = Vec::with_capacity(segs.len());
        let map = |pt: &Point, mm: &[f64; 6]| {
            let (x, y) = apply(mm, pt.x as f64, pt.y as f64);
            Point { x: x as f32, y: y as f32 }
        };
        for sg in &segs {
            user.push(match sg {
                PathSeg::MoveTo(a) => PathSeg::MoveTo(map(a, &inv)),
                PathSeg::LineTo(a) => PathSeg::LineTo(map(a, &inv)),
                PathSeg::QuadTo(a, b) => PathSeg::QuadTo(map(a, &inv), map(b, &inv)),
                PathSeg::CubicTo(a, b, c) => PathSeg::CubicTo(map(a, &inv), map(b, &inv), map(c, &inv)),
                other => other.clone(),
            });
        }
        let outline = rabbithole_gfx::canvas_ext::stroke_outline(&user, &style);
        let mut dev = Polylines::default();
        for c in outline.contours {
            let pts = c.points.iter().map(|pt| map(pt, &m)).collect();
            dev.contours.push(rabbithole_gfx::path::Contour { points: pts, closed: true });
        }
        let mask = rasterize(&dev, FillRule::NonZero, limit(s));
        paint_mask(s, &mask, &src, &p);
        Ok(Value::Undefined)
    })
}

/// `canvasClip(id, path, rule, previousClipId|null)` → clip id.
fn canvas_clip(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let f = floats(realm, &arg(args, 1))?;
    let even = arg_str(realm, args, 2)? == "evenodd";
    let prev = arg(args, 3);
    let prev = if prev.is_nullish() { None } else { Some(realm.to_number(prev)? as u32) };
    with_surface(realm, &arg(args, 0), |_, s| {
        let mask = rasterize(&fill_polys(&decode_path(&f)), if even { FillRule::EvenOdd } else { FillRule::NonZero }, limit(s));
        let (w, h) = (s.width as i32, s.height as i32);
        let mut full = vec![0u8; (w * h) as usize];
        let prev = prev.and_then(|id| s.clips.get(&id).cloned());
        for y in mask.y0.max(0)..(mask.y0 + mask.height as i32).min(h) {
            for x in mask.x0.max(0)..(mask.x0 + mask.width as i32).min(w) {
                let i = (y * w + x) as usize;
                let mut v = mask.get(x, y);
                if let Some(p) = &prev {
                    v = ((v as u32 * p[i] as u32) / 255) as u8;
                }
                full[i] = v;
            }
        }
        s.next_clip += 1;
        let id = s.next_clip;
        s.clips.insert(id, Rc::new(full));
        Ok(Value::Number(id as f64))
    })
}

/// `canvasClearRect(id, path, params)`: clear to transparent black within
/// the (device-space) path, honouring the clip.
fn canvas_clear(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let f = floats(realm, &arg(args, 1))?;
    with_surface(realm, &arg(args, 0), |realm, s| {
        let p = read_params(realm, s, &arg(args, 2))?;
        let mask = rasterize(&fill_polys(&decode_path(&f)), FillRule::NonZero, limit(s));
        let w = s.width as i32;
        for y in mask.y0.max(0)..(mask.y0 + mask.height as i32).min(s.height as i32) {
            for x in mask.x0.max(0)..(mask.x0 + mask.width as i32).min(w) {
                let i = (y * w + x) as usize;
                let mut cov = mask.get(x, y) as f32 / 255.0;
                if let Some(c) = &p.clip {
                    cov *= c[i] as f32 / 255.0;
                }
                if cov <= 0.0 {
                    continue;
                }
                let px = &mut s.rgba[i * 4..i * 4 + 4];
                px[3] = (px[3] as f32 * (1.0 - cov)) as u8;
                if px[3] == 0 {
                    px.copy_from_slice(&[0, 0, 0, 0]);
                }
            }
        }
        Ok(Value::Undefined)
    })
}

/// `canvasGetImageData(id, x, y, w, h)` → ArrayBuffer (RGBA).
fn canvas_get(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let (sx, sy) = (num(realm, args, 1)? as i64, num(realm, args, 2)? as i64);
    let (sw, sh) = (num(realm, args, 3)? as i64, num(realm, args, 4)? as i64);
    let out = with_surface(realm, &arg(args, 0), |_, s| {
        let mut out = vec![0u8; (sw.max(0) * sh.max(0) * 4) as usize];
        for y in 0..sh {
            let yy = sy + y;
            if yy < 0 || yy >= s.height as i64 {
                continue;
            }
            for x in 0..sw {
                let xx = sx + x;
                if xx < 0 || xx >= s.width as i64 {
                    continue;
                }
                let si = ((yy * s.width as i64 + xx) * 4) as usize;
                let di = ((y * sw + x) * 4) as usize;
                out[di..di + 4].copy_from_slice(&s.rgba[si..si + 4]);
            }
        }
        Ok(out)
    })?;
    Ok(crate::fetch::make_array_buffer(realm, out))
}

/// `canvasPutImageData(id, data, w, h, dx, dy, dirtyX, dirtyY, dirtyW, dirtyH)`
fn canvas_put(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let data = bytes(realm, &arg(args, 1))?;
    let v: Vec<i64> = (2..10).map(|i| num(realm, args, i).map(|n| n as i64)).collect::<JsResult<_>>()?;
    let (iw, ih, dx, dy, rx, ry, rw, rh) = (v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7]);
    with_surface(realm, &arg(args, 0), |_, s| {
        for y in ry.max(0)..(ry + rh).min(ih) {
            let yy = dy + y;
            if yy < 0 || yy >= s.height as i64 {
                continue;
            }
            for x in rx.max(0)..(rx + rw).min(iw) {
                let xx = dx + x;
                if xx < 0 || xx >= s.width as i64 {
                    continue;
                }
                let si = ((y * iw + x) * 4) as usize;
                let di = ((yy * s.width as i64 + xx) * 4) as usize;
                if si + 4 <= data.len() {
                    s.rgba[di..di + 4].copy_from_slice(&data[si..si + 4]);
                }
            }
        }
        Ok(Value::Undefined)
    })
}

/// `canvasDrawImage(id, srcData, srcW, srcH, [sx, sy, sw, sh], [dx, dy, dw,
/// dh], matrix, params, smoothing)`.
fn canvas_draw_image(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let data = bytes(realm, &arg(args, 1))?;
    let (iw, ih) = (num(realm, args, 2)? as i64, num(realm, args, 3)? as i64);
    let sr = floats(realm, &arg(args, 4))?;
    let dr = floats(realm, &arg(args, 5))?;
    let m = read_matrix(realm, &arg(args, 6))?;
    let smooth = realm.to_boolean(&arg(args, 8));
    if sr.len() < 4 || dr.len() < 4 || iw <= 0 || ih <= 0 || data.len() < (iw * ih * 4) as usize {
        return Ok(Value::Undefined);
    }
    with_surface(realm, &arg(args, 0), |realm, s| {
        let p = read_params(realm, s, &arg(args, 7))?;
        let (dx, dy, dw, dh) = (dr[0], dr[1], dr[2], dr[3]);
        let corners = [(dx, dy), (dx + dw, dy), (dx + dw, dy + dh), (dx, dy + dh)];
        let pts: Vec<Point> = corners.iter().map(|(x, y)| { let (a, b) = apply(&m, *x, *y); Point { x: a as f32, y: b as f32 } }).collect();
        let poly = Polylines { contours: vec![rabbithole_gfx::path::Contour { points: pts, closed: true }] };
        let mask = rasterize(&poly, FillRule::NonZero, limit(s));
        let inv = invert(m);
        let img = Rc::new(data);
        let (sx, sy, sw, sh) = (sr[0], sr[1], sr[2], sr[3]);
        // Map device → user → image space.
        let full = {
            // image coord = (user - d) * s/d + s0
            let ax = sw / dw;
            let ay = sh / dh;
            [inv[0] * ax, inv[1] * ay, inv[2] * ax, inv[3] * ay, (inv[4] - dx) * ax + sx, (inv[5] - dy) * ay + sy]
        };
        let src = ImageSrc { inv: full, w: iw, h: ih, rgba: img, smooth, bounds: (sx, sy, sx + sw, sy + sh) };
        let w = s.width as i32;
        for y in mask.y0.max(0)..(mask.y0 + mask.height as i32).min(s.height as i32) {
            for x in mask.x0.max(0)..(mask.x0 + mask.width as i32).min(w) {
                let i = (y * w + x) as usize;
                let mut cov = mask.get(x, y) as f32 / 255.0;
                if let Some(c) = &p.clip {
                    cov *= c[i] as f32 / 255.0;
                }
                if cov <= 0.0 {
                    continue;
                }
                let mut c = src.at(x, y);
                c[3] *= p.alpha;
                composite(&mut s.rgba[i * 4..i * 4 + 4], c, cov, &p.op);
            }
        }
        Ok(Value::Undefined)
    })
}

struct ImageSrc {
    inv: [f64; 6],
    w: i64,
    h: i64,
    rgba: Rc<Vec<u8>>,
    smooth: bool,
    bounds: (f64, f64, f64, f64),
}

impl ImageSrc {
    fn texel(&self, x: i64, y: i64) -> [f32; 4] {
        let (x0, y0, x1, y1) = self.bounds;
        let x = x.clamp((x0.floor() as i64).max(0), (x1.ceil() as i64 - 1).min(self.w - 1));
        let y = y.clamp((y0.floor() as i64).max(0), (y1.ceil() as i64 - 1).min(self.h - 1));
        let i = ((y * self.w + x) * 4) as usize;
        let r = &self.rgba;
        [r[i] as f32 / 255.0, r[i + 1] as f32 / 255.0, r[i + 2] as f32 / 255.0, r[i + 3] as f32 / 255.0]
    }
    fn at(&self, x: i32, y: i32) -> [f32; 4] {
        let (u, v) = apply(&self.inv, x as f64 + 0.5, y as f64 + 0.5);
        if !self.smooth {
            return self.texel(u.floor() as i64, v.floor() as i64);
        }
        // Bilinear in premultiplied space.
        let (fu, fv) = (u - 0.5, v - 0.5);
        let (x0, y0) = (fu.floor() as i64, fv.floor() as i64);
        let (tx, ty) = ((fu - x0 as f64) as f32, (fv - y0 as f64) as f32);
        let mut acc = [0.0f32; 4];
        for (dx, dy, wgt) in [(0, 0, (1.0 - tx) * (1.0 - ty)), (1, 0, tx * (1.0 - ty)), (0, 1, (1.0 - tx) * ty), (1, 1, tx * ty)] {
            let t = self.texel(x0 + dx, y0 + dy);
            for k in 0..3 {
                acc[k] += t[k] * t[3] * wgt;
            }
            acc[3] += t[3] * wgt;
        }
        if acc[3] > 0.0 {
            for k in 0..3 {
                acc[k] /= acc[3];
            }
        }
        acc
    }
}

/// Parse a CSS `font` shorthand: (size px, family, weight, italic).
fn parse_font(font: &str) -> (f32, rabbithole_gfx::Family, u16, bool) {
    let mut size = 10.0;
    let mut weight = 400;
    let mut italic = false;
    let mut family = rabbithole_gfx::Family::Sans;
    let lower = font.to_ascii_lowercase();
    let mut rest = lower.as_str();
    for tok in lower.split_whitespace() {
        match tok {
            "italic" | "oblique" => italic = true,
            "bold" | "bolder" => weight = 700,
            "lighter" => weight = 300,
            t if t.len() == 3 && t.chars().all(|c| c.is_ascii_digit()) => weight = t.parse().unwrap_or(400),
            t if t.contains("px") || t.contains("pt") || t.contains("em") => {
                let sz = t.split('/').next().unwrap_or(t);
                let n: f32 = sz.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '%').parse().unwrap_or(10.0);
                size = if sz.ends_with("pt") { n * 4.0 / 3.0 } else if sz.ends_with("rem") || sz.ends_with("em") { n * 16.0 } else { n };
                if let Some(i) = lower.find(tok) {
                    rest = &lower[i + tok.len()..];
                }
                break;
            }
            _ => {}
        }
    }
    for fam in rest.split(',') {
        let f = fam.trim().trim_matches(|c| c == '"' || c == '\'');
        if f.contains("mono") || f.contains("courier") || f.contains("consol") {
            family = rabbithole_gfx::Family::Mono;
            break;
        }
        if f == "serif" || f.contains("times") || f.contains("georgia") || (f.contains("serif") && !f.contains("sans")) {
            family = rabbithole_gfx::Family::Serif;
            break;
        }
        if !f.is_empty() {
            break;
        }
    }
    (size.max(0.0), family, weight, italic)
}

/// `canvasMeasureText(text, font)` → { width, ascent, descent, fontAscent, fontDescent }.
fn canvas_measure(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let text = arg_str(realm, args, 0)?;
    let font = arg_str(realm, args, 1)?;
    let (size, family, weight, italic) = parse_font(&font);
    let fc = fonts();
    let fid = fc.select(family, weight, italic);
    let glyphs = fc.shape(fid, size, &text, 0.0);
    let width = glyphs.last().map(|g| g.x + g.advance).unwrap_or(0.0);
    let lm = fc.metrics(fid, size);
    let (mut asc, mut desc) = (0.0f32, 0.0f32);
    for g in &glyphs {
        let b = fc.rasterize(g.font, g.glyph, size);
        asc = asc.max(b.top as f32);
        desc = desc.max(b.height as f32 - b.top as f32);
    }
    Ok(object_from(realm, vec![("width", Value::Number(width as f64)), ("ascent", Value::Number(asc as f64)), ("descent", Value::Number(desc as f64)), ("fontAscent", Value::Number(lm.ascent as f64)), ("fontDescent", Value::Number(lm.descent as f64))]))
}

/// `canvasFillText(id, text, x, y, font, paint, matrix, params, strokeWidth)`
/// (x, y = user-space position of the alphabetic baseline start).
fn canvas_fill_text(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let text = arg_str(realm, args, 1)?;
    let (x, y) = (num(realm, args, 2)?, num(realm, args, 3)?);
    let font = arg_str(realm, args, 4)?;
    let m = read_matrix(realm, &arg(args, 6))?;
    let src = read_paint(realm, &arg(args, 5), m)?;
    let stroke_w = num(realm, args, 8).unwrap_or(0.0);
    let (size, family, weight, italic) = parse_font(&font);
    let scale = (m[0] * m[3] - m[1] * m[2]).abs().sqrt().max(0.01);
    let dev_size = (size as f64 * scale) as f32;
    let fc = fonts();
    let fid = fc.select(family, weight, italic);
    let glyphs = fc.shape(fid, dev_size, &text, 0.0);
    with_surface(realm, &arg(args, 0), |realm, s| {
        let p = read_params(realm, s, &arg(args, 7))?;
        let (ox, oy) = apply(&m, x, y);
        // Glyphs are rasterized axis-aligned at the transformed size and
        // positioned along the transformed baseline.
        let dir = ((m[0] / scale), (m[1] / scale));
        let w = s.width as i32;
        let h = s.height as i32;
        let mut mask_data: HashMap<(i32, i32), u8> = HashMap::new();
        let mut bb = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for g in &glyphs {
            let bm = fc.rasterize(g.font, g.glyph, dev_size);
            let gx = ox + dir.0 * (g.x + g.dx) as f64;
            let gy = oy + dir.1 * (g.x + g.dx) as f64 - g.dy as f64;
            let left = gx.round() as i32 + bm.left;
            let top = gy.round() as i32 - bm.top;
            for yy in 0..bm.height as i32 {
                for xx in 0..bm.width as i32 {
                    let cov = bm.coverage[(yy * bm.width as i32 + xx) as usize];
                    if cov == 0 {
                        continue;
                    }
                    let (px, py) = (left + xx, top + yy);
                    if px < 0 || py < 0 || px >= w || py >= h {
                        continue;
                    }
                    let e = mask_data.entry((px, py)).or_insert(0);
                    *e = (*e).max(cov);
                    bb = (bb.0.min(px), bb.1.min(py), bb.2.max(px), bb.3.max(py));
                }
            }
        }
        if mask_data.is_empty() {
            return Ok(Value::Undefined);
        }
        let (mw, mh) = ((bb.2 - bb.0 + 1) as u32, (bb.3 - bb.1 + 1) as u32);
        let mut data = vec![0u8; (mw * mh) as usize];
        for ((px, py), c) in mask_data {
            data[((py - bb.1) as u32 * mw + (px - bb.0) as u32) as usize] = c;
        }
        if stroke_w > 0.0 {
            // strokeText: outline = dilated coverage minus the glyph interior.
            let r = ((stroke_w * scale) / 2.0).ceil().max(1.0) as i32;
            let mut out = vec![0u8; data.len()];
            for yy in 0..mh as i32 {
                for xx in 0..mw as i32 {
                    let mut mx = 0u8;
                    for ddy in -r..=r {
                        for ddx in -r..=r {
                            let (sx, sy) = (xx + ddx, yy + ddy);
                            if sx >= 0 && sy >= 0 && sx < mw as i32 && sy < mh as i32 {
                                mx = mx.max(data[(sy as u32 * mw + sx as u32) as usize]);
                            }
                        }
                    }
                    let inner = data[(yy as u32 * mw + xx as u32) as usize];
                    out[(yy as u32 * mw + xx as u32) as usize] = mx.saturating_sub(inner / 2);
                }
            }
            data = out;
        }
        let mask = Mask { x0: bb.0, y0: bb.1, width: mw, height: mh, data };
        paint_mask(s, &mask, &src, &p);
        Ok(Value::Undefined)
    })
}

/// `canvasPresent(id, canvasElement)`: hand the bitmap to the engine.
fn canvas_present(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(n) = node_of(realm, &arg(args, 1)) else { return Ok(Value::Undefined) };
    if n.doc != 0 {
        return Ok(Value::Undefined);
    }
    with_surface(realm, &arg(args, 0), |_, s| {
        let (w, h) = (s.width, s.height);
        let rgba = if w > 0 && h > 0 { Some(s.rgba.clone()) } else { None };
        with_env(|env| env.set_canvas_image(n.node, w, h, rgba));
        Ok(Value::Undefined)
    })
}

/// `canvasEncode(id, type, quality)` → ArrayBuffer of PNG / JPEG bytes.
fn canvas_encode(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let ty = arg_str(realm, args, 1).unwrap_or_default();
    let quality = num(realm, args, 2).ok().filter(|q| (0.0..=1.0).contains(q)).unwrap_or(0.92);
    let out = with_surface(realm, &arg(args, 0), |_, s| Ok(rabbithole_gfx::image::encode(s.width, s.height, &s.rgba, &ty, quality as f32)))?;
    match out {
        Some(b) => Ok(crate::fetch::make_array_buffer(realm, b)),
        None => Ok(Value::Null),
    }
}

/// `decodeImage(bytes)` → { width, height, data } or null (createImageBitmap(blob)).
fn decode_image(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let data = bytes(realm, &arg(args, 0))?;
    match rabbithole_gfx::Image::decode(&data) {
        Some(img) => {
            let (w, h) = (img.width, img.height);
            let px = img.pixels().to_vec();
            let ab = crate::fetch::make_array_buffer(realm, px);
            Ok(object_from(realm, vec![("width", Value::Number(w as f64)), ("height", Value::Number(h as f64)), ("data", ab)]))
        }
        None => Ok(Value::Null),
    }
}

pub(crate) fn install(realm: &mut Realm, o: rabbithole_js::ObjectId) {
    realm.define_method(o, "canvasNew", 2, canvas_new);
    realm.define_method(o, "canvasResize", 3, canvas_resize);
    realm.define_method(o, "canvasFree", 1, canvas_free);
    realm.define_method(o, "canvasFill", 6, canvas_fill);
    realm.define_method(o, "canvasStroke", 6, canvas_stroke);
    realm.define_method(o, "canvasClip", 4, canvas_clip);
    realm.define_method(o, "canvasFreeClip", 2, canvas_free_clip);
    realm.define_method(o, "canvasClear", 3, canvas_clear);
    realm.define_method(o, "canvasGetImageData", 5, canvas_get);
    realm.define_method(o, "canvasPutImageData", 10, canvas_put);
    realm.define_method(o, "canvasDrawImage", 9, canvas_draw_image);
    realm.define_method(o, "canvasMeasureText", 2, canvas_measure);
    realm.define_method(o, "canvasFillText", 9, canvas_fill_text);
    realm.define_method(o, "canvasPresent", 2, canvas_present);
    realm.define_method(o, "canvasEncode", 3, canvas_encode);
    realm.define_method(o, "decodeImage", 1, decode_image);
}
