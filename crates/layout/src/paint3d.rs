//! Painting of 3D-transformed boxes (CSS Transforms 2).
//!
//! A [`Group3d`] is painted offscreen in its own (document) coordinates,
//! then mapped onto the canvas through the projective map its 4×4 matrix
//! gives the z = 0 plane, sampling the offscreen pixels per device pixel
//! (perspective-correct). A group whose used `transform-style` is
//! `preserve-3d` is a 3D rendering context: it and its nested groups are
//! separate planes, each with its accumulated matrix, composited with a
//! depth buffer so that planes in front cover the ones behind whatever
//! their painting order (intersecting planes cut each other per pixel;
//! coplanar planes keep their painting order).

use crate::display_list::{DisplayItem, Group3d};
use crate::filter::Pixels;
use crate::paint::{draw_device_pixels, paint_item, State};
use rabbithole_gfx::mat4::{self, Mat4};
use rabbithole_gfx::{Canvas, Color, FontContext, Matrix, Rect};

/// One plane of a 3D rendering context.
struct Plane<'a> {
    /// Items coordinates → the coordinates the context is flattened into.
    m: Mat4,
    items: Vec<&'a DisplayItem>,
    backface_hidden: bool,
}

/// Split a group into planes: its own items (with the clips and
/// translations open around nested planes replayed before them) and, when
/// it preserves 3D, one plane per nested group, recursively.
fn collect<'a>(g: &'a Group3d, parent: &Mat4, prefix: &[&'a DisplayItem], out: &mut Vec<Plane<'a>>) {
    let m = mat4::mul(parent, &g.matrix);
    let idx = out.len();
    out.push(Plane { m, items: prefix.to_vec(), backface_hidden: g.backface_hidden });
    let mut open: Vec<&'a DisplayItem> = prefix.to_vec();
    for it in &g.items {
        match it {
            DisplayItem::Transform3d(c) if g.preserve => collect(c, &m, &open, out),
            _ => {
                out[idx].items.push(it);
                match it {
                    DisplayItem::PushClip(_) | DisplayItem::PushClipRounded(..) | DisplayItem::PushClipPath { .. } | DisplayItem::PushTransform(_) | DisplayItem::PushFixed => open.push(it),
                    DisplayItem::PopClip | DisplayItem::PopTransform | DisplayItem::PopFixed => {
                        open.pop();
                    }
                    _ => {}
                }
            }
        }
    }
}

fn mul2(m: &Matrix, n: &Matrix) -> Matrix {
    rabbithole_style::multiply_matrix(*m, *n)
}

/// Bounds of a 2D rect's image under a 4×4 matrix (`None` when part of it
/// is behind the viewer).
fn project_rect(m: &Mat4, r: Rect) -> Option<Rect> {
    let mut x0 = f32::MAX;
    let mut y0 = f32::MAX;
    let mut x1 = f32::MIN;
    let mut y1 = f32::MIN;
    for (x, y) in [(r.x, r.y), (r.right(), r.y), (r.x, r.bottom()), (r.right(), r.bottom())] {
        let p = mat4::map_homogeneous(m, x as f64, y as f64);
        if p[3] <= 1e-6 {
            return None;
        }
        let (px, py) = ((p[0] / p[3]) as f32, (p[1] / p[3]) as f32);
        x0 = x0.min(px);
        y0 = y0.min(py);
        x1 = x1.max(px);
        y1 = y1.max(py);
    }
    (x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
}

/// Painted extent of items (their coordinates), generous.
fn extent<'a>(items: impl Iterator<Item = &'a DisplayItem>) -> Option<Rect> {
    let mut stack: Vec<Matrix> = vec![rabbithole_gfx::IDENTITY];
    let mut out: Option<Rect> = None;
    let mut add = |r: Rect, m: &Matrix, out: &mut Option<Rect>| {
        let d = rabbithole_gfx::path::transform_rect_bounds(m, r);
        *out = Some(match out {
            Some(o) => o.union(&d),
            None => d,
        });
    };
    for it in items {
        let m = *stack.last().unwrap();
        match it {
            DisplayItem::PushTransform(t) => stack.push(mul2(&m, t)),
            DisplayItem::PopTransform => {
                if stack.len() > 1 {
                    stack.pop();
                }
            }
            DisplayItem::Transform3d(g) => {
                if let Some(e) = extent(g.items.iter()) {
                    match project_rect(&g.matrix, e) {
                        Some(r) => add(r, &m, &mut out),
                        // Partly behind the viewer: unbounded.
                        None => add(Rect::new(-1.0e6, -1.0e6, 2.0e6, 2.0e6), &rabbithole_gfx::IDENTITY, &mut out),
                    }
                }
            }
            DisplayItem::PushClip(_) | DisplayItem::PushClipRounded(..) | DisplayItem::PushClipPath { .. } | DisplayItem::PushLayer(_) => {}
            _ => {
                if let Some(b) = crate::damage::item_bounds(it) {
                    add(b, &m, &mut out);
                }
            }
        }
    }
    out
}

/// A plane's painting, premultiplied, covering `[x0, x0 + w / scale) ×
/// [y0, y0 + h / scale)` of its (canvas-local) coordinates.
struct Texture {
    px: Pixels,
    x0: f32,
    y0: f32,
    scale: f32,
}

/// Largest local → device magnification of `d` over a rect's corners.
fn magnification(d: &Mat4, r: Rect) -> f32 {
    let mut s: f32 = 0.0;
    for (x, y) in [(r.x, r.y), (r.right(), r.y), (r.x, r.bottom()), (r.right(), r.bottom()), (r.x + r.w / 2.0, r.y + r.h / 2.0)] {
        let (x, y) = (x as f64, y as f64);
        let p = mat4::map_homogeneous(d, x, y);
        let px = mat4::map_homogeneous(d, x + 1.0, y);
        let py = mat4::map_homogeneous(d, x, y + 1.0);
        if p[3] <= 1e-6 || px[3] <= 1e-6 || py[3] <= 1e-6 {
            continue;
        }
        let (bx, by) = (p[0] / p[3], p[1] / p[3]);
        let lx = ((px[0] / px[3] - bx).powi(2) + (px[1] / px[3] - by).powi(2)).sqrt();
        let ly = ((py[0] / py[3] - bx).powi(2) + (py[1] / py[3] - by).powi(2)).sqrt();
        if lx.is_finite() && ly.is_finite() {
            s = s.max(lx as f32).max(ly as f32);
        }
    }
    s
}

/// The part of the plane (local coordinates) that can show in the device
/// `region`, when it is bounded.
fn visible_local(d: &Mat4, region: Rect) -> Option<Rect> {
    let h = mat4::plane_homography(d);
    let hi = mat4::invert3(&h)?;
    let mut x0 = f64::MAX;
    let mut y0 = f64::MAX;
    let mut x1 = f64::MIN;
    let mut y1 = f64::MIN;
    for (x, y) in [(region.x, region.y), (region.right(), region.y), (region.x, region.bottom()), (region.right(), region.bottom())] {
        let (x, y) = (x as f64, y as f64);
        let u = hi[0] * x + hi[1] * y + hi[2];
        let v = hi[3] * x + hi[4] * y + hi[5];
        let w = hi[6] * x + hi[7] * y + hi[8];
        if w.abs() < 1e-12 {
            return None;
        }
        let (lx, ly) = (u / w, v / w);
        // The corner must be in front of the viewer on the plane.
        if h[6] * lx + h[7] * ly + h[8] <= 1e-9 {
            return None;
        }
        x0 = x0.min(lx);
        y0 = y0.min(ly);
        x1 = x1.max(lx);
        y1 = y1.max(ly);
    }
    let r = Rect::new(x0 as f32, y0 as f32, (x1 - x0) as f32, (y1 - y0) as f32);
    (r.x.is_finite() && r.y.is_finite() && r.w.is_finite() && r.h.is_finite()).then_some(r)
}

const MAX_TEXELS: f32 = 16.0e6;

fn render_plane(items: &[&DisplayItem], d: &Mat4, region: Rect, fonts: &FontContext, st: &State) -> Option<Texture> {
    let e = extent(items.iter().copied())?.translate(st.dx, st.dy);
    let local = match visible_local(d, region) {
        // A margin keeps the sampling at the region's edges smooth.
        Some(v) => e.intersect(&Rect::new(v.x - 2.0, v.y - 2.0, v.w + 4.0, v.h + 4.0)),
        None => e,
    };
    if local.w <= 0.0 || local.h <= 0.0 {
        return None;
    }
    let (x0, y0) = (local.x.floor(), local.y.floor());
    let (w, h) = ((local.right().ceil() - x0).max(1.0), (local.bottom().ceil() - y0).max(1.0));
    let mut s = magnification(d, Rect::new(x0, y0, w, h)).clamp(1.0, 4.0);
    if s < 1.05 {
        s = 1.0;
    }
    while w * h * s * s > MAX_TEXELS && s > 0.05 {
        s *= 0.5;
    }
    let (tw, th) = ((w * s).ceil() as usize, (h * s).ceil() as usize);
    if tw == 0 || th == 0 || (tw * th) as f32 > MAX_TEXELS * 1.01 {
        return None;
    }
    let plain = s == 1.0;
    let render = |bg: Color| {
        let mut c = Canvas::new(tw as u32, th as u32);
        c.clear(bg);
        let (sdx, sdy) = if plain { (st.dx - x0, st.dy - y0) } else { (st.dx, st.dy) };
        let fixed_origin = if plain { (st.fixed_origin.0 - x0, st.fixed_origin.1 - y0) } else { st.fixed_origin };
        if !plain {
            c.push_transform([s, 0.0, 0.0, s, -x0 * s, -y0 * s]);
        }
        let mut sub = State::nested(sdx, sdy, fixed_origin, st.ext);
        let all = Rect::new(-1.0e7, -1.0e7, 2.0e7, 2.0e7);
        for it in items {
            paint_item(it, &mut c, fonts, &mut sub, all);
        }
        c.data
    };
    let black = render(Color::BLACK);
    let white = render(Color::WHITE);
    let px = crate::filter::from_black_white(&black, &white, tw, th);
    Some(Texture { px, x0, y0, scale: s })
}

/// Device-space accumulation of planes with a depth buffer.
struct Compositor {
    x0: i32,
    y0: i32,
    w: usize,
    h: usize,
    px: Vec<[f32; 4]>,
    z: Vec<f32>,
    drawn: bool,
}

/// Depths closer than this (px) are coplanar: painting order decides.
const COPLANAR: f32 = 0.05;

impl Compositor {
    fn new(region: Rect) -> Option<Compositor> {
        let x0 = region.x.floor().max(0.0) as i32;
        let y0 = region.y.floor().max(0.0) as i32;
        let x1 = region.right().ceil() as i32;
        let y1 = region.bottom().ceil() as i32;
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
        Some(Compositor { x0, y0, w, h, px: vec![[0.0; 4]; w * h], z: vec![f32::NEG_INFINITY; w * h], drawn: false })
    }

    /// Device bounds (pixels, within the compositor) of the texture's
    /// image under `d`, clipped to the half space in front of the viewer.
    fn bounds(&self, t: &Texture, d: &Mat4) -> Option<(i32, i32, i32, i32)> {
        let (tw, th) = (t.px.w as f32 / t.scale, t.px.h as f32 / t.scale);
        let corners = [(t.x0, t.y0), (t.x0 + tw, t.y0), (t.x0 + tw, t.y0 + th), (t.x0, t.y0 + th)];
        let pts: Vec<[f64; 4]> = corners.iter().map(|&(x, y)| mat4::map_homogeneous(d, x as f64, y as f64)).collect();
        // Clip the quad against w = eps (Sutherland–Hodgman).
        const EPS: f64 = 1e-4;
        let mut poly: Vec<[f64; 4]> = Vec::with_capacity(8);
        for i in 0..4 {
            let a = pts[i];
            let b = pts[(i + 1) % 4];
            let (ia, ib) = (a[3] > EPS, b[3] > EPS);
            if ia {
                poly.push(a);
            }
            if ia != ib {
                let k = (EPS - a[3]) / (b[3] - a[3]);
                poly.push([a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k, a[2] + (b[2] - a[2]) * k, EPS]);
            }
        }
        if poly.is_empty() {
            return None;
        }
        let mut x0 = f64::MAX;
        let mut y0 = f64::MAX;
        let mut x1 = f64::MIN;
        let mut y1 = f64::MIN;
        for p in &poly {
            let (x, y) = (p[0] / p[3], p[1] / p[3]);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        let cx0 = (x0.floor() as i64).max(self.x0 as i64) as i32;
        let cy0 = (y0.floor() as i64).max(self.y0 as i64) as i32;
        let cx1 = (x1.ceil() as i64).min(self.x0 as i64 + self.w as i64) as i32;
        let cy1 = (y1.ceil() as i64).min(self.y0 as i64 + self.h as i64) as i32;
        (cx1 > cx0 && cy1 > cy0).then_some((cx0, cy0, cx1, cy1))
    }

    /// Map the texture through `d` (texture coordinates → device), testing
    /// depth: in front of what is there it goes over, behind it under.
    fn draw(&mut self, t: &Texture, d: &Mat4) {
        let Some((bx0, by0, bx1, by1)) = self.bounds(t, d) else { return };
        let h = mat4::plane_homography(d);
        let Some(hi) = mat4::invert3(&h) else { return };
        let zr = [d[8], d[9], d[11]];
        let (tw, th) = (t.px.w as i64, t.px.h as i64);
        let s = t.scale as f64;
        let texel = |x: i64, y: i64| -> [f32; 4] {
            if x < 0 || y < 0 || x >= tw || y >= th {
                [0.0; 4]
            } else {
                t.px.px[(y * tw + x) as usize]
            }
        };
        for y in by0..by1 {
            let yc = y as f64 + 0.5;
            for x in bx0..bx1 {
                let xc = x as f64 + 0.5;
                let u = hi[0] * xc + hi[1] * yc + hi[2];
                let v = hi[3] * xc + hi[4] * yc + hi[5];
                let w = hi[6] * xc + hi[7] * yc + hi[8];
                if w == 0.0 {
                    continue;
                }
                let (lx, ly) = (u / w, v / w);
                let fw = h[6] * lx + h[7] * ly + h[8];
                if fw <= 1e-9 {
                    continue;
                }
                // Quantized: equal matrices composed in different orders
                // sample the same texels.
                let fx = (((lx - t.x0 as f64) * s - 0.5) * 1024.0).round() / 1024.0;
                let fy = (((ly - t.y0 as f64) * s - 0.5) * 1024.0).round() / 1024.0;
                if fx < -1.0 || fy < -1.0 || fx > tw as f64 || fy > th as f64 {
                    continue;
                }
                let (ix, iy) = (fx.floor(), fy.floor());
                let (ax, ay) = ((fx - ix) as f32, (fy - iy) as f32);
                let (ix, iy) = (ix as i64, iy as i64);
                let c00 = texel(ix, iy);
                let c10 = texel(ix + 1, iy);
                let c01 = texel(ix, iy + 1);
                let c11 = texel(ix + 1, iy + 1);
                let mut c = [0.0f32; 4];
                for k in 0..4 {
                    let top = c00[k] + (c10[k] - c00[k]) * ax;
                    let bot = c01[k] + (c11[k] - c01[k]) * ax;
                    c[k] = top + (bot - top) * ay;
                }
                if c[3] <= 1.0 / 512.0 {
                    continue;
                }
                let z = ((zr[0] * lx + zr[1] * ly + zr[2]) / fw) as f32;
                let i = (y - self.y0) as usize * self.w + (x - self.x0) as usize;
                let dst = &mut self.px[i];
                let old_z = self.z[i];
                if z + COPLANAR < old_z {
                    // Behind what is there: under it.
                    let k = 1.0 - dst[3];
                    for j in 0..4 {
                        dst[j] += c[j] * k;
                    }
                } else {
                    let k = 1.0 - c[3];
                    for j in 0..4 {
                        dst[j] = c[j] + dst[j] * k;
                    }
                    self.z[i] = z.max(old_z);
                }
                self.drawn = true;
            }
        }
    }

    fn finish(self, canvas: &mut Canvas) {
        if !self.drawn {
            return;
        }
        let px = Pixels { w: self.w, h: self.h, px: self.px };
        draw_device_pixels(canvas, crate::filter::to_rgba(&px), self.x0, self.y0, self.w, self.h);
    }
}

/// Paint a 3D group onto the canvas.
pub(crate) fn paint_group(g: &Group3d, canvas: &mut Canvas, fonts: &FontContext, st: &State, device_view: Rect) {
    let region = canvas.clip().intersect(&device_view).intersect(&Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32));
    if region.is_empty() {
        return;
    }
    let mut planes = Vec::new();
    collect(g, &mat4::IDENTITY4, &[], &mut planes);
    // Canvas-local coordinates (items + the painter's translation) to
    // device: the canvas matrix after the plane's, conjugated by the
    // translation.
    let c = mat4::from_2d(&canvas.transform());
    let t = mat4::translate(st.dx, st.dy, 0.0);
    let ti = mat4::translate(-st.dx, -st.dy, 0.0);
    let Some(mut comp) = Compositor::new(region) else { return };
    for p in &planes {
        if p.backface_hidden && mat4::backface_showing(&p.m) {
            continue;
        }
        let d = mat4::mul(&c, &mat4::mul(&t, &mat4::mul(&p.m, &ti)));
        if mat4::invert3(&mat4::plane_homography(&d)).is_none() {
            // Seen edge-on: nothing shows.
            continue;
        }
        if let Some(tex) = render_plane(&p.items, &d, region, fonts, st) {
            comp.draw(&tex, &d);
        }
    }
    comp.finish(canvas);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tex(w: usize, h: usize, c: [f32; 4]) -> Texture {
        Texture { px: Pixels { w, h, px: vec![c; w * h] }, x0: 0.0, y0: 0.0, scale: 1.0 }
    }

    #[test]
    fn identity_copies_pixels() {
        let mut comp = Compositor::new(Rect::new(0.0, 0.0, 4.0, 4.0)).unwrap();
        comp.draw(&tex(2, 2, [1.0, 0.0, 0.0, 1.0]), &mat4::IDENTITY4);
        assert_eq!(comp.px[0], [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(comp.px[5], [1.0, 0.0, 0.0, 1.0]);
        // Bilinear sampling at the edge mixes with transparency.
        assert!(comp.px[2][3] < 0.01);
    }

    #[test]
    fn depth_decides_over_painting_order() {
        let mut comp = Compositor::new(Rect::new(0.0, 0.0, 4.0, 4.0)).unwrap();
        // Front (z = 10) painted first, back (z = -10) second.
        comp.draw(&tex(4, 4, [1.0, 0.0, 0.0, 1.0]), &mat4::translate(0.0, 0.0, 10.0));
        comp.draw(&tex(4, 4, [0.0, 0.0, 1.0, 1.0]), &mat4::translate(0.0, 0.0, -10.0));
        assert_eq!(comp.px[5], [1.0, 0.0, 0.0, 1.0]);
        // Coplanar: the later one wins.
        comp.draw(&tex(4, 4, [0.0, 1.0, 0.0, 1.0]), &mat4::translate(0.0, 0.0, 10.0));
        assert_eq!(comp.px[5], [0.0, 1.0, 0.0, 1.0]);
    }

    #[test]
    fn intersecting_planes_cut_each_other() {
        // A plane tilted about the y axis through x = 2 crosses z = 0.
        let mut comp = Compositor::new(Rect::new(0.0, 0.0, 4.0, 1.0)).unwrap();
        comp.draw(&tex(4, 1, [0.0, 0.0, 1.0, 1.0]), &mat4::IDENTITY4);
        let tilt = mat4::mul(&mat4::translate(2.0, 0.0, 0.0), &mat4::mul(&mat4::rotate(0.0, 1.0, 0.0, 0.5), &mat4::translate(-2.0, 0.0, 0.0)));
        comp.draw(&tex(8, 1, [1.0, 0.0, 0.0, 1.0]), &tilt);
        // rotateY(+0.5) brings the left side (x < 2) towards the viewer.
        assert_eq!(comp.px[1][0], 1.0);
        assert_eq!(comp.px[3][2], 1.0);
    }

    #[test]
    fn behind_the_viewer_is_not_drawn() {
        let mut comp = Compositor::new(Rect::new(0.0, 0.0, 4.0, 4.0)).unwrap();
        let m = mat4::mul(&mat4::perspective(Some(10.0)), &mat4::translate(0.0, 0.0, 20.0));
        comp.draw(&tex(4, 4, [1.0, 0.0, 0.0, 1.0]), &m);
        assert!(!comp.drawn);
    }
}
