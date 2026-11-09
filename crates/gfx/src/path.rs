//! Path geometry: flattening of curves and arcs into polylines, bounding
//! boxes, matrix helpers and common path builders (rounded rects, ellipses,
//! SVG `d` strings).

use crate::canvas::{Matrix, PathSeg, Point, Radii, Rect, IDENTITY};

impl Point {
    pub const fn new(x: f32, y: f32) -> Point {
        Point { x, y }
    }
}

/// Apply `m` to a point.
#[inline]
pub fn transform_point(m: &Matrix, p: Point) -> Point {
    Point {
        x: m[0] * p.x + m[2] * p.y + m[4],
        y: m[1] * p.x + m[3] * p.y + m[5],
    }
}

/// `m × n` (apply `n` first, then `m`).
pub fn multiply(m: &Matrix, n: &Matrix) -> Matrix {
    [
        m[0] * n[0] + m[2] * n[1],
        m[1] * n[0] + m[3] * n[1],
        m[0] * n[2] + m[2] * n[3],
        m[1] * n[2] + m[3] * n[3],
        m[0] * n[4] + m[2] * n[5] + m[4],
        m[1] * n[4] + m[3] * n[5] + m[5],
    ]
}

/// Inverse of an affine matrix (identity when singular).
pub fn invert(m: &Matrix) -> Matrix {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-12 {
        return IDENTITY;
    }
    let id = 1.0 / det;
    let a = m[3] * id;
    let b = -m[1] * id;
    let c = -m[2] * id;
    let d = m[0] * id;
    let e = -(a * m[4] + c * m[5]);
    let f = -(b * m[4] + d * m[5]);
    [a, b, c, d, e, f]
}

pub fn is_identity(m: &Matrix) -> bool {
    *m == IDENTITY
}

/// True when the matrix only translates and scales by positive factors
/// (axis-aligned rectangles stay axis-aligned rectangles).
pub fn is_axis_aligned(m: &Matrix) -> bool {
    m[1].abs() < 1e-6 && m[2].abs() < 1e-6 && m[0] > 0.0 && m[3] > 0.0
}

/// Approximate uniform scale factor of a matrix (geometric mean of the
/// singular values).
pub fn scale_factor(m: &Matrix) -> f32 {
    (m[0] * m[3] - m[1] * m[2]).abs().sqrt().max(1e-6)
}

/// Map an axis-aligned rect through an axis-aligned matrix.
pub fn transform_rect_aligned(m: &Matrix, r: Rect) -> Rect {
    let p = transform_point(m, Point::new(r.x, r.y));
    Rect::new(p.x, p.y, r.w * m[0], r.h * m[3])
}

/// Bounding box of a rect mapped through any matrix.
pub fn transform_rect_bounds(m: &Matrix, r: Rect) -> Rect {
    let pts = [
        transform_point(m, Point::new(r.x, r.y)),
        transform_point(m, Point::new(r.right(), r.y)),
        transform_point(m, Point::new(r.x, r.bottom())),
        transform_point(m, Point::new(r.right(), r.bottom())),
    ];
    let mut x0 = f32::MAX;
    let mut y0 = f32::MAX;
    let mut x1 = f32::MIN;
    let mut y1 = f32::MIN;
    for p in pts {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

/// Path of an axis-aligned rect (closed, clockwise in screen space).
pub fn rect_path(r: Rect) -> Vec<PathSeg> {
    vec![
        PathSeg::MoveTo(Point::new(r.x, r.y)),
        PathSeg::LineTo(Point::new(r.right(), r.y)),
        PathSeg::LineTo(Point::new(r.right(), r.bottom())),
        PathSeg::LineTo(Point::new(r.x, r.bottom())),
        PathSeg::Close,
    ]
}

/// Clamp radii so adjacent corners never overlap (CSS algorithm).
pub fn clamp_radii(rect: Rect, radii: Radii) -> Radii {
    let mut r = radii;
    for c in &mut r {
        c.0 = c.0.max(0.0);
        c.1 = c.1.max(0.0);
    }
    let mut f: f32 = 1.0;
    let sums = [
        (r[0].0 + r[1].0, rect.w),
        (r[3].0 + r[2].0, rect.w),
        (r[0].1 + r[3].1, rect.h),
        (r[1].1 + r[2].1, rect.h),
    ];
    for (sum, len) in sums {
        if sum > len && sum > 0.0 {
            f = f.min(len / sum);
        }
    }
    if f < 1.0 {
        for c in &mut r {
            c.0 *= f;
            c.1 *= f;
        }
    }
    r
}

/// Bézier approximation constant for a quarter ellipse.
const KAPPA: f32 = 0.552_284_8;

/// Build the path of a rounded rectangle (helper shared by callers).
pub fn rounded_rect_path(rect: Rect, radii: Radii) -> Vec<PathSeg> {
    let r = clamp_radii(rect, radii);
    if r.iter().all(|c| c.0 <= 0.0 || c.1 <= 0.0) {
        return rect_path(rect);
    }
    let (x0, y0, x1, y1) = (rect.x, rect.y, rect.right(), rect.bottom());
    let mut p = Vec::with_capacity(10);
    // Top-left corner start (on the top edge)
    p.push(PathSeg::MoveTo(Point::new(x0 + r[0].0, y0)));
    p.push(PathSeg::LineTo(Point::new(x1 - r[1].0, y0)));
    if r[1].0 > 0.0 && r[1].1 > 0.0 {
        p.push(PathSeg::CubicTo(
            Point::new(x1 - r[1].0 + r[1].0 * KAPPA, y0),
            Point::new(x1, y0 + r[1].1 - r[1].1 * KAPPA),
            Point::new(x1, y0 + r[1].1),
        ));
    }
    p.push(PathSeg::LineTo(Point::new(x1, y1 - r[2].1)));
    if r[2].0 > 0.0 && r[2].1 > 0.0 {
        p.push(PathSeg::CubicTo(
            Point::new(x1, y1 - r[2].1 + r[2].1 * KAPPA),
            Point::new(x1 - r[2].0 + r[2].0 * KAPPA, y1),
            Point::new(x1 - r[2].0, y1),
        ));
    }
    p.push(PathSeg::LineTo(Point::new(x0 + r[3].0, y1)));
    if r[3].0 > 0.0 && r[3].1 > 0.0 {
        p.push(PathSeg::CubicTo(
            Point::new(x0 + r[3].0 - r[3].0 * KAPPA, y1),
            Point::new(x0, y1 - r[3].1 + r[3].1 * KAPPA),
            Point::new(x0, y1 - r[3].1),
        ));
    }
    p.push(PathSeg::LineTo(Point::new(x0, y0 + r[0].1)));
    if r[0].0 > 0.0 && r[0].1 > 0.0 {
        p.push(PathSeg::CubicTo(
            Point::new(x0, y0 + r[0].1 - r[0].1 * KAPPA),
            Point::new(x0 + r[0].0 - r[0].0 * KAPPA, y0),
            Point::new(x0 + r[0].0, y0),
        ));
    }
    p.push(PathSeg::Close);
    p
}

/// Build the path of an ellipse / circle.
pub fn ellipse_path(center: Point, rx: f32, ry: f32) -> Vec<PathSeg> {
    let (cx, cy) = (center.x, center.y);
    let kx = rx * KAPPA;
    let ky = ry * KAPPA;
    vec![
        PathSeg::MoveTo(Point::new(cx + rx, cy)),
        PathSeg::CubicTo(Point::new(cx + rx, cy + ky), Point::new(cx + kx, cy + ry), Point::new(cx, cy + ry)),
        PathSeg::CubicTo(Point::new(cx - kx, cy + ry), Point::new(cx - rx, cy + ky), Point::new(cx - rx, cy)),
        PathSeg::CubicTo(Point::new(cx - rx, cy - ky), Point::new(cx - kx, cy - ry), Point::new(cx, cy - ry)),
        PathSeg::CubicTo(Point::new(cx + kx, cy - ry), Point::new(cx + rx, cy - ky), Point::new(cx + rx, cy)),
        PathSeg::Close,
    ]
}

/// A flattened path: a list of closed or open polylines.
#[derive(Debug, Clone, Default)]
pub struct Polylines {
    pub contours: Vec<Contour>,
}

#[derive(Debug, Clone, Default)]
pub struct Contour {
    pub points: Vec<Point>,
    pub closed: bool,
}

/// Flatten `path` into polylines, mapping every point through `m`.
/// `tolerance` is the maximum deviation in device pixels.
pub fn flatten(path: &[PathSeg], m: &Matrix, tolerance: f32) -> Polylines {
    let mut out = Polylines::default();
    let mut cur = Contour::default();
    let mut start = Point::default();
    let mut last = Point::default(); // user space
    let tol = tolerance.max(0.02);
    let flush = |cur: &mut Contour, out: &mut Polylines| {
        if !cur.points.is_empty() {
            out.contours.push(std::mem::take(cur));
        }
    };
    for seg in path {
        match seg {
            PathSeg::MoveTo(p) => {
                flush(&mut cur, &mut out);
                cur.points.push(transform_point(m, *p));
                start = *p;
                last = *p;
            }
            PathSeg::LineTo(p) => {
                if cur.points.is_empty() {
                    cur.points.push(transform_point(m, last));
                }
                cur.points.push(transform_point(m, *p));
                last = *p;
            }
            PathSeg::QuadTo(c, p) => {
                if cur.points.is_empty() {
                    cur.points.push(transform_point(m, last));
                }
                let p0 = transform_point(m, last);
                let p1 = transform_point(m, *c);
                let p2 = transform_point(m, *p);
                flatten_quad(p0, p1, p2, tol, &mut cur.points);
                last = *p;
            }
            PathSeg::CubicTo(c1, c2, p) => {
                if cur.points.is_empty() {
                    cur.points.push(transform_point(m, last));
                }
                let p0 = transform_point(m, last);
                let p1 = transform_point(m, *c1);
                let p2 = transform_point(m, *c2);
                let p3 = transform_point(m, *p);
                flatten_cubic(p0, p1, p2, p3, tol, &mut cur.points);
                last = *p;
            }
            PathSeg::ArcTo { rx, ry, rotation, large_arc, sweep, to } => {
                if cur.points.is_empty() {
                    cur.points.push(transform_point(m, last));
                }
                let cubics = arc_to_cubics(last, *rx, *ry, *rotation, *large_arc, *sweep, *to);
                let mut from = last;
                for (c1, c2, p) in cubics {
                    let p0 = transform_point(m, from);
                    flatten_cubic(p0, transform_point(m, c1), transform_point(m, c2), transform_point(m, p), tol, &mut cur.points);
                    from = p;
                }
                last = *to;
            }
            PathSeg::Close => {
                if !cur.points.is_empty() {
                    cur.closed = true;
                    flush(&mut cur, &mut out);
                }
                last = start;
            }
        }
    }
    flush(&mut cur, &mut out);
    out
}

fn flatten_quad(p0: Point, p1: Point, p2: Point, tol: f32, out: &mut Vec<Point>) {
    // Number of segments from the second difference magnitude.
    let ddx = p0.x - 2.0 * p1.x + p2.x;
    let ddy = p0.y - 2.0 * p1.y + p2.y;
    let dd = (ddx * ddx + ddy * ddy).sqrt();
    let n = ((dd / (8.0 * tol)).sqrt().ceil() as usize).clamp(1, 100);
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let mt = 1.0 - t;
        let x = mt * mt * p0.x + 2.0 * mt * t * p1.x + t * t * p2.x;
        let y = mt * mt * p0.y + 2.0 * mt * t * p1.y + t * t * p2.y;
        out.push(Point::new(x, y));
    }
}

fn flatten_cubic(p0: Point, p1: Point, p2: Point, p3: Point, tol: f32, out: &mut Vec<Point>) {
    let ddx1 = p0.x - 2.0 * p1.x + p2.x;
    let ddy1 = p0.y - 2.0 * p1.y + p2.y;
    let ddx2 = p1.x - 2.0 * p2.x + p3.x;
    let ddy2 = p1.y - 2.0 * p2.y + p3.y;
    let dd = (ddx1 * ddx1 + ddy1 * ddy1).sqrt().max((ddx2 * ddx2 + ddy2 * ddy2).sqrt());
    let n = ((dd * 0.75 / tol).sqrt().ceil() as usize).clamp(1, 200);
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let mt = 1.0 - t;
        let a = mt * mt * mt;
        let b = 3.0 * mt * mt * t;
        let c = 3.0 * mt * t * t;
        let d = t * t * t;
        out.push(Point::new(
            a * p0.x + b * p1.x + c * p2.x + d * p3.x,
            a * p0.y + b * p1.y + c * p2.y + d * p3.y,
        ));
    }
}

/// Convert an SVG elliptical arc to cubic Béziers (center parameterization,
/// SVG implementation notes F.6.5).
pub fn arc_to_cubics(from: Point, rx: f32, ry: f32, rotation: f32, large_arc: bool, sweep: bool, to: Point) -> Vec<(Point, Point, Point)> {
    let mut rx = rx.abs();
    let mut ry = ry.abs();
    if rx < 1e-6 || ry < 1e-6 || (from.x == to.x && from.y == to.y) {
        return vec![(from, to, to)];
    }
    let (sin_r, cos_r) = rotation.sin_cos();
    let dx2 = (from.x - to.x) / 2.0;
    let dy2 = (from.y - to.y) / 2.0;
    let x1p = cos_r * dx2 + sin_r * dy2;
    let y1p = -sin_r * dx2 + cos_r * dy2;
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = (rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p).max(0.0);
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let mut coef = if den.abs() < 1e-12 { 0.0 } else { (num / den).sqrt() };
    if large_arc == sweep {
        coef = -coef;
    }
    let cxp = coef * (rx * y1p / ry);
    let cyp = coef * -(ry * x1p / rx);
    let cx = cos_r * cxp - sin_r * cyp + (from.x + to.x) / 2.0;
    let cy = sin_r * cxp + cos_r * cyp + (from.y + to.y) / 2.0;
    let angle = |ux: f32, uy: f32, vx: f32, vy: f32| -> f32 {
        let dot = ux * vx + uy * vy;
        let len = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
        let mut a = (dot / len).clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 {
            a = -a;
        }
        a
    };
    let theta1 = angle(1.0, 0.0, (x1p - cxp) / rx, (y1p - cyp) / ry);
    let mut dtheta = angle((x1p - cxp) / rx, (y1p - cyp) / ry, (-x1p - cxp) / rx, (-y1p - cyp) / ry);
    if !sweep && dtheta > 0.0 {
        dtheta -= std::f32::consts::TAU;
    } else if sweep && dtheta < 0.0 {
        dtheta += std::f32::consts::TAU;
    }
    let segments = ((dtheta.abs() / (std::f32::consts::FRAC_PI_2)).ceil() as usize).max(1);
    let delta = dtheta / segments as f32;
    let t = 4.0 / 3.0 * (delta / 4.0).tan();
    let mut out = Vec::with_capacity(segments);
    let mut th = theta1;
    let point_at = |a: f32| -> Point {
        let (s, c) = a.sin_cos();
        Point::new(cx + rx * c * cos_r - ry * s * sin_r, cy + rx * c * sin_r + ry * s * cos_r)
    };
    let deriv_at = |a: f32| -> Point {
        let (s, c) = a.sin_cos();
        Point::new(-rx * s * cos_r - ry * c * sin_r, -rx * s * sin_r + ry * c * cos_r)
    };
    for i in 0..segments {
        let th2 = th + delta;
        let p0 = point_at(th);
        let p3 = if i == segments - 1 { to } else { point_at(th2) };
        let d0 = deriv_at(th);
        let d3 = deriv_at(th2);
        let c1 = Point::new(p0.x + t * d0.x, p0.y + t * d0.y);
        let c2 = Point::new(p3.x - t * d3.x, p3.y - t * d3.y);
        out.push((c1, c2, p3));
        th = th2;
    }
    out
}

/// Bounding box of flattened polylines.
pub fn polylines_bounds(p: &Polylines) -> Option<Rect> {
    let mut x0 = f32::MAX;
    let mut y0 = f32::MAX;
    let mut x1 = f32::MIN;
    let mut y1 = f32::MIN;
    let mut any = false;
    for c in &p.contours {
        for pt in &c.points {
            any = true;
            x0 = x0.min(pt.x);
            y0 = y0.min(pt.y);
            x1 = x1.max(pt.x);
            y1 = y1.max(pt.y);
        }
    }
    if !any {
        return None;
    }
    Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
}

/// Parse an SVG path `d` attribute into segments.
pub fn parse_svg_path(d: &str) -> Vec<PathSeg> {
    let mut p = SvgParser { s: d.as_bytes(), i: 0 };
    let mut out = Vec::new();
    let mut cur = Point::default();
    let mut start = Point::default();
    let mut last_ctrl: Option<Point> = None; // for S/T reflection
    let mut last_cmd = b' ';
    while let Some(c) = p.next_cmd() {
        let rel = c.is_ascii_lowercase();
        let cmd = c.to_ascii_uppercase();
        match cmd {
            b'M' => {
                let mut first = true;
                while let Some((x, y)) = p.pair() {
                    let pt = if rel { Point::new(cur.x + x, cur.y + y) } else { Point::new(x, y) };
                    if first {
                        out.push(PathSeg::MoveTo(pt));
                        start = pt;
                        first = false;
                    } else {
                        out.push(PathSeg::LineTo(pt));
                    }
                    cur = pt;
                }
                last_ctrl = None;
            }
            b'L' => {
                while let Some((x, y)) = p.pair() {
                    let pt = if rel { Point::new(cur.x + x, cur.y + y) } else { Point::new(x, y) };
                    out.push(PathSeg::LineTo(pt));
                    cur = pt;
                }
                last_ctrl = None;
            }
            b'H' => {
                while let Some(x) = p.number() {
                    let pt = Point::new(if rel { cur.x + x } else { x }, cur.y);
                    out.push(PathSeg::LineTo(pt));
                    cur = pt;
                }
                last_ctrl = None;
            }
            b'V' => {
                while let Some(y) = p.number() {
                    let pt = Point::new(cur.x, if rel { cur.y + y } else { y });
                    out.push(PathSeg::LineTo(pt));
                    cur = pt;
                }
                last_ctrl = None;
            }
            b'C' => {
                while let Some((x1, y1)) = p.pair() {
                    let (Some((x2, y2)), Some((x, y))) = (p.pair(), p.pair()) else { break };
                    let (c1, c2, pt) = if rel {
                        (Point::new(cur.x + x1, cur.y + y1), Point::new(cur.x + x2, cur.y + y2), Point::new(cur.x + x, cur.y + y))
                    } else {
                        (Point::new(x1, y1), Point::new(x2, y2), Point::new(x, y))
                    };
                    out.push(PathSeg::CubicTo(c1, c2, pt));
                    last_ctrl = Some(c2);
                    cur = pt;
                }
            }
            b'S' => {
                while let Some((x2, y2)) = p.pair() {
                    let Some((x, y)) = p.pair() else { break };
                    let c1 = match (last_cmd, last_ctrl) {
                        (b'C' | b'S', Some(lc)) => Point::new(2.0 * cur.x - lc.x, 2.0 * cur.y - lc.y),
                        _ => cur,
                    };
                    let (c2, pt) = if rel {
                        (Point::new(cur.x + x2, cur.y + y2), Point::new(cur.x + x, cur.y + y))
                    } else {
                        (Point::new(x2, y2), Point::new(x, y))
                    };
                    out.push(PathSeg::CubicTo(c1, c2, pt));
                    last_ctrl = Some(c2);
                    cur = pt;
                    last_cmd = b'S';
                }
            }
            b'Q' => {
                while let Some((x1, y1)) = p.pair() {
                    let Some((x, y)) = p.pair() else { break };
                    let (c1, pt) = if rel {
                        (Point::new(cur.x + x1, cur.y + y1), Point::new(cur.x + x, cur.y + y))
                    } else {
                        (Point::new(x1, y1), Point::new(x, y))
                    };
                    out.push(PathSeg::QuadTo(c1, pt));
                    last_ctrl = Some(c1);
                    cur = pt;
                }
            }
            b'T' => {
                while let Some((x, y)) = p.pair() {
                    let c1 = match (last_cmd, last_ctrl) {
                        (b'Q' | b'T', Some(lc)) => Point::new(2.0 * cur.x - lc.x, 2.0 * cur.y - lc.y),
                        _ => cur,
                    };
                    let pt = if rel { Point::new(cur.x + x, cur.y + y) } else { Point::new(x, y) };
                    out.push(PathSeg::QuadTo(c1, pt));
                    last_ctrl = Some(c1);
                    cur = pt;
                    last_cmd = b'T';
                }
            }
            b'A' => {
                while let Some(rx) = p.number() {
                    let (Some(ry), Some(rot), Some(large), Some(sweep), Some((x, y))) = (p.number(), p.number(), p.flag(), p.flag(), p.pair()) else { break };
                    let pt = if rel { Point::new(cur.x + x, cur.y + y) } else { Point::new(x, y) };
                    out.push(PathSeg::ArcTo {
                        rx,
                        ry,
                        rotation: rot.to_radians(),
                        large_arc: large,
                        sweep,
                        to: pt,
                    });
                    cur = pt;
                }
                last_ctrl = None;
            }
            b'Z' => {
                out.push(PathSeg::Close);
                cur = start;
                last_ctrl = None;
            }
            _ => break,
        }
        if !matches!(cmd, b'S' | b'T') {
            last_cmd = cmd;
        }
    }
    out
}

struct SvgParser<'a> {
    s: &'a [u8],
    i: usize,
}

impl SvgParser<'_> {
    fn skip_ws(&mut self) {
        while self.i < self.s.len() && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',') {
            self.i += 1;
        }
    }

    fn next_cmd(&mut self) -> Option<u8> {
        self.skip_ws();
        let c = *self.s.get(self.i)?;
        if c.is_ascii_alphabetic() {
            self.i += 1;
            Some(c)
        } else {
            None
        }
    }

    fn number(&mut self) -> Option<f32> {
        self.skip_ws();
        let start = self.i;
        let s = self.s;
        let mut i = self.i;
        if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
            i += 1;
        }
        let mut digits = 0;
        while i < s.len() && s[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
        if i < s.len() && s[i] == b'.' {
            i += 1;
            while i < s.len() && s[i].is_ascii_digit() {
                i += 1;
                digits += 1;
            }
        }
        if digits == 0 {
            return None;
        }
        if i < s.len() && (s[i] == b'e' || s[i] == b'E') {
            let mut j = i + 1;
            if j < s.len() && (s[j] == b'+' || s[j] == b'-') {
                j += 1;
            }
            if j < s.len() && s[j].is_ascii_digit() {
                while j < s.len() && s[j].is_ascii_digit() {
                    j += 1;
                }
                i = j;
            }
        }
        self.i = i;
        std::str::from_utf8(&s[start..i]).ok()?.parse().ok()
    }

    fn pair(&mut self) -> Option<(f32, f32)> {
        let save = self.i;
        let x = self.number()?;
        match self.number() {
            Some(y) => Some((x, y)),
            None => {
                self.i = save;
                None
            }
        }
    }

    fn flag(&mut self) -> Option<bool> {
        self.skip_ws();
        let c = *self.s.get(self.i)?;
        match c {
            b'0' => {
                self.i += 1;
                Some(false)
            }
            b'1' => {
                self.i += 1;
                Some(true)
            }
            _ => None,
        }
    }
}
