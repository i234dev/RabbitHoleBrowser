//! Geometry of `<basic-shape>` values (CSS Shapes 1/2): a shape resolved
//! in its reference box as a path.

use crate::computed::{BasicShape, ControlAnchor, ControlPoint, Length, ShapeCommand, ShapeRadius};
use rabbithole_gfx::path::{clamp_radii, ellipse_path, rounded_rect_path};
use rabbithole_gfx::{FillRule, PathSeg, Point, Radii, Rect};

fn res(l: Length, base: f32) -> f32 {
    l.resolve_or_zero(base)
}

/// Corner radii of a rectangle shape `r`: percentages resolve against the
/// reference box `rb`, then overlapping radii are scaled down.
fn radii_in(radii: &[(Length, Length); 4], r: Rect, rb: Rect) -> Radii {
    let mut out = [(0.0, 0.0); 4];
    for i in 0..4 {
        out[i] = (res(radii[i].0, rb.w).max(0.0), res(radii[i].1, rb.h).max(0.0));
    }
    clamp_radii(r, out)
}

/// A rectangle shape `r` with its radii.
fn rect_shape(r: Rect, radii: &[(Length, Length); 4], rb: Rect) -> (Rect, Radii) {
    (r, radii_in(radii, r, rb))
}

/// The shapes that are rounded rectangles (`inset()`, `xywh()`, `rect()`,
/// `circle()`, `ellipse()`) as their rectangle and corner radii, resolved in
/// the reference box `rb`; `None` for the others.
pub fn basic_shape_rounded(shape: &BasicShape, rb: Rect) -> Option<(Rect, Radii)> {
    let (w, h) = (rb.w, rb.h);
    Some(match shape {
        BasicShape::Inset { insets, radii } => {
            let (mut t, mut r, mut bo, mut l) = (res(insets[0], h), res(insets[1], w), res(insets[2], h), res(insets[3], w));
            // Insets adding up to more than the box are scaled down.
            if l + r > w && l + r > 0.0 {
                let f = w / (l + r);
                l *= f;
                r *= f;
            }
            if t + bo > h && t + bo > 0.0 {
                let f = h / (t + bo);
                t *= f;
                bo *= f;
            }
            let rr = Rect::new(rb.x + l, rb.y + t, (w - l - r).max(0.0), (h - t - bo).max(0.0));
            rect_shape(rr, radii, rb)
        }
        BasicShape::Xywh { x, y, w: rw, h: rh, radii } => {
            let rr = Rect::new(rb.x + res(*x, w), rb.y + res(*y, h), res(*rw, w).max(0.0), res(*rh, h).max(0.0));
            rect_shape(rr, radii, rb)
        }
        BasicShape::Rect { edges, radii } => {
            let edge = |l: Length, base: f32, auto: f32| if l.is_auto() { auto } else { res(l, base) };
            let top = edge(edges[0], h, 0.0);
            let right = edge(edges[1], w, w);
            let bottom = edge(edges[2], h, h);
            let left = edge(edges[3], w, 0.0);
            let rr = Rect::new(rb.x + left, rb.y + top, (right - left).max(0.0), (bottom - top).max(0.0));
            rect_shape(rr, radii, rb)
        }
        BasicShape::Circle { r, at } => {
            let (cx, cy) = (res(at.0, w), res(at.1, h));
            let (dl, dr, dt, db) = (cx.abs(), (w - cx).abs(), cy.abs(), (h - cy).abs());
            let corners = [(dl, dt), (dr, dt), (dl, db), (dr, db)].map(|(a, b)| a.hypot(b));
            let radius = match r {
                ShapeRadius::Len(l) => res(*l, w.hypot(h) / std::f32::consts::SQRT_2),
                ShapeRadius::ClosestSide => dl.min(dr).min(dt).min(db),
                ShapeRadius::FarthestSide => dl.max(dr).max(dt).max(db),
                ShapeRadius::ClosestCorner => corners.into_iter().fold(f32::INFINITY, f32::min),
                ShapeRadius::FarthestCorner => corners.into_iter().fold(0.0, f32::max),
            }
            .max(0.0);
            ellipse_box(rb.x + cx, rb.y + cy, radius, radius)
        }
        BasicShape::Ellipse { rx, ry, at } => {
            let (cx, cy) = (res(at.0, w), res(at.1, h));
            let (dl, dr, dt, db) = (cx.abs(), (w - cx).abs(), cy.abs(), (h - cy).abs());
            let corners = [(dl, dt), (dr, dt), (dl, db), (dr, db)].map(|(a, b)| a.hypot(b));
            let side = |r: &ShapeRadius, a: f32, b: f32, base: f32| -> f32 {
                match r {
                    ShapeRadius::Len(l) => res(*l, base),
                    ShapeRadius::ClosestSide => a.min(b),
                    ShapeRadius::FarthestSide => a.max(b),
                    // Corner keywords: the distance to the closest /
                    // farthest corner of the box, for either radius.
                    ShapeRadius::ClosestCorner => corners.into_iter().fold(f32::INFINITY, f32::min),
                    ShapeRadius::FarthestCorner => corners.into_iter().fold(0.0, f32::max),
                }
            };
            ellipse_box(rb.x + cx, rb.y + cy, side(rx, dl, dr, w).max(0.0), side(ry, dt, db, h).max(0.0))
        }
        _ => return None,
    })
}

fn ellipse_box(cx: f32, cy: f32, rx: f32, ry: f32) -> (Rect, Radii) {
    (Rect::new(cx - rx, cy - ry, 2.0 * rx, 2.0 * ry), [(rx, ry); 4])
}

/// A basic shape resolved in the reference box `rb`.
pub fn basic_shape_path(shape: &BasicShape, rb: Rect) -> (Vec<PathSeg>, FillRule) {
    let (w, h) = (rb.w, rb.h);
    let rule = |evenodd: bool| if evenodd { FillRule::EvenOdd } else { FillRule::NonZero };
    if let Some((r, radii)) = basic_shape_rounded(shape, rb) {
        let path = if matches!(shape, BasicShape::Circle { .. } | BasicShape::Ellipse { .. }) {
            ellipse_path(Point::new(r.x + r.w / 2.0, r.y + r.h / 2.0), r.w / 2.0, r.h / 2.0)
        } else {
            rounded_rect_path(r, radii)
        };
        return (path, FillRule::NonZero);
    }
    match shape {
        BasicShape::Polygon { evenodd, points, round } => {
            let pts: Vec<Point> = points.iter().map(|(x, y)| Point::new(rb.x + res(*x, w), rb.y + res(*y, h))).collect();
            (polygon_path(&pts, *round), rule(*evenodd))
        }
        BasicShape::Path { evenodd, path } => (translate_path(path, rb.x, rb.y), rule(*evenodd)),
        BasicShape::Shape { evenodd, from, cmds } => (shape_fn_path(*from, cmds, rb), rule(*evenodd)),
        _ => (Vec::new(), FillRule::NonZero),
    }
}

/// A polygon, its corners rounded by `round` (clamped to half of the
/// adjacent edges).
fn polygon_path(pts: &[Point], round: f32) -> Vec<PathSeg> {
    let n = pts.len();
    let mut p = Vec::with_capacity(n + 2);
    if n == 0 {
        return p;
    }
    if round <= 0.0 || n < 3 {
        p.push(PathSeg::MoveTo(pts[0]));
        for q in &pts[1..] {
            p.push(PathSeg::LineTo(*q));
        }
        p.push(PathSeg::Close);
        return p;
    }
    for i in 0..n {
        let (a, v, b) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
        let (ux, uy) = (a.x - v.x, a.y - v.y);
        let (wx, wy) = (b.x - v.x, b.y - v.y);
        let (lu, lw) = (ux.hypot(uy), wx.hypot(wy));
        let corner = if lu > 0.0 && lw > 0.0 {
            let cos = ((ux * wx + uy * wy) / (lu * lw)).clamp(-1.0, 1.0);
            let theta = cos.acos();
            let t = (theta / 2.0).tan();
            if t > 1e-4 && theta < std::f32::consts::PI - 1e-4 {
                let d = (round / t).min(lu / 2.0).min(lw / 2.0);
                let r = d * t;
                let p1 = Point::new(v.x + ux / lu * d, v.y + uy / lu * d);
                let p2 = Point::new(v.x + wx / lw * d, v.y + wy / lw * d);
                // Turning direction decides the arc sweep.
                let cross = (v.x - a.x) * (b.y - v.y) - (v.y - a.y) * (b.x - v.x);
                Some((p1, p2, r, cross > 0.0))
            } else {
                None
            }
        } else {
            None
        };
        let (start, seg) = match corner {
            Some((p1, p2, r, sweep)) => (p1, Some(PathSeg::ArcTo { rx: r, ry: r, rotation: 0.0, large_arc: false, sweep, to: p2 })),
            None => (v, None),
        };
        p.push(if i == 0 { PathSeg::MoveTo(start) } else { PathSeg::LineTo(start) });
        if let Some(s) = seg {
            p.push(s);
        }
    }
    p.push(PathSeg::Close);
    p
}

fn shift(p: Point, dx: f32, dy: f32) -> Point {
    Point::new(p.x + dx, p.y + dy)
}

/// `path` translated by (dx, dy).
pub fn translate_path(path: &[PathSeg], dx: f32, dy: f32) -> Vec<PathSeg> {
    path.iter()
        .map(|s| match *s {
            PathSeg::MoveTo(p) => PathSeg::MoveTo(shift(p, dx, dy)),
            PathSeg::LineTo(p) => PathSeg::LineTo(shift(p, dx, dy)),
            PathSeg::QuadTo(a, p) => PathSeg::QuadTo(shift(a, dx, dy), shift(p, dx, dy)),
            PathSeg::CubicTo(a, b, p) => PathSeg::CubicTo(shift(a, dx, dy), shift(b, dx, dy), shift(p, dx, dy)),
            PathSeg::ArcTo { rx, ry, rotation, large_arc, sweep, to } => PathSeg::ArcTo { rx, ry, rotation, large_arc, sweep, to: shift(to, dx, dy) },
            PathSeg::Close => PathSeg::Close,
        })
        .collect()
}

/// `shape()` commands to a path (CSS Shapes 2 §4.2).
fn shape_fn_path(from: (Length, Length), cmds: &[ShapeCommand], rb: Rect) -> Vec<PathSeg> {
    let (w, h) = (rb.w, rb.h);
    let o = |x: f32, y: f32| Point::new(rb.x + x, rb.y + y);
    let pos = |x: Length, y: Length| (res(x, w), res(y, h));
    let start = pos(from.0, from.1);
    let mut path = vec![PathSeg::MoveTo(o(start.0, start.1))];
    // Current point and subpath start, box-relative.
    let mut cur = start;
    let mut sub = start;
    // Second control point of the previous curve (for `smooth`).
    let mut last_ctrl: Option<(f32, f32)> = None;
    let end = |by: bool, cur: (f32, f32), x: Length, y: Length| {
        let (px, py) = pos(x, y);
        if by { (cur.0 + px, cur.1 + py) } else { (px, py) }
    };
    let ctrl = |c: &ControlPoint, s: (f32, f32), e: (f32, f32)| {
        let (px, py) = pos(c.x, c.y);
        match c.anchor {
            ControlAnchor::Origin => (px, py),
            ControlAnchor::Start => (s.0 + px, s.1 + py),
            ControlAnchor::End => (e.0 + px, e.1 + py),
        }
    };
    for c in cmds {
        let mut ctrl_out = None;
        match c {
            ShapeCommand::Move { by, x, y } => {
                cur = end(*by, cur, *x, *y);
                sub = cur;
                path.push(PathSeg::MoveTo(o(cur.0, cur.1)));
            }
            ShapeCommand::Line { by, x, y } => {
                cur = end(*by, cur, *x, *y);
                path.push(PathSeg::LineTo(o(cur.0, cur.1)));
            }
            ShapeCommand::HLine { by, x } => {
                let v = res(*x, w);
                cur.0 = if *by { cur.0 + v } else { v };
                path.push(PathSeg::LineTo(o(cur.0, cur.1)));
            }
            ShapeCommand::VLine { by, y } => {
                let v = res(*y, h);
                cur.1 = if *by { cur.1 + v } else { v };
                path.push(PathSeg::LineTo(o(cur.0, cur.1)));
            }
            ShapeCommand::Curve { by, x, y, c1, c2 } => {
                let e = end(*by, cur, *x, *y);
                let a = ctrl(c1, cur, e);
                match c2 {
                    Some(c2) => {
                        let b = ctrl(c2, cur, e);
                        path.push(PathSeg::CubicTo(o(a.0, a.1), o(b.0, b.1), o(e.0, e.1)));
                        ctrl_out = Some(b);
                    }
                    None => {
                        path.push(PathSeg::QuadTo(o(a.0, a.1), o(e.0, e.1)));
                        ctrl_out = Some(a);
                    }
                }
                cur = e;
            }
            ShapeCommand::Smooth { by, x, y, c } => {
                let e = end(*by, cur, *x, *y);
                // Reflection of the previous control point.
                let a = match last_ctrl {
                    Some(p) => (2.0 * cur.0 - p.0, 2.0 * cur.1 - p.1),
                    None => cur,
                };
                match c {
                    Some(c) => {
                        let b = ctrl(c, cur, e);
                        path.push(PathSeg::CubicTo(o(a.0, a.1), o(b.0, b.1), o(e.0, e.1)));
                        ctrl_out = Some(b);
                    }
                    None => {
                        path.push(PathSeg::QuadTo(o(a.0, a.1), o(e.0, e.1)));
                        ctrl_out = Some(a);
                    }
                }
                cur = e;
            }
            ShapeCommand::Arc { by, x, y, rx, ry, cw, large, rotation } => {
                let e = end(*by, cur, *x, *y);
                let (erx, ery) = match ry {
                    Some(ry) => (res(*rx, w), res(*ry, h)),
                    None => {
                        let r = res(*rx, w.hypot(h) / std::f32::consts::SQRT_2);
                        (r, r)
                    }
                };
                path.push(PathSeg::ArcTo { rx: erx, ry: ery, rotation: *rotation, large_arc: *large, sweep: *cw, to: o(e.0, e.1) });
                cur = e;
            }
            ShapeCommand::Close => {
                path.push(PathSeg::Close);
                cur = sub;
            }
        }
        last_ctrl = ctrl_out;
    }
    path
}

