//! Stroker: converts polylines into fillable outlines (joins, caps, dashes).
//! Every piece is emitted as a consistently oriented simple polygon so the
//! union can be filled with the non-zero rule.

use crate::canvas::{LineCap, LineJoin, Point, StrokeStyle};
use crate::path::{Contour, Polylines};

/// Produce the outline polygons of stroking `poly` with `style`.
pub fn stroke_polylines(poly: &Polylines, style: &StrokeStyle) -> Polylines {
    let mut out = Polylines::default();
    let hw = (style.width / 2.0).max(0.05);
    for c in &poly.contours {
        let pieces = if style.dashes.iter().any(|d| *d > 0.0) { apply_dashes(c, &style.dashes, style.dash_offset) } else { vec![dedupe(c)] };
        for piece in pieces {
            stroke_contour(&piece, hw, style, &mut out);
        }
    }
    out
}

/// Remove consecutive duplicate points.
fn dedupe(c: &Contour) -> Contour {
    let mut points: Vec<Point> = Vec::with_capacity(c.points.len());
    for p in &c.points {
        if let Some(last) = points.last() {
            if (last.x - p.x).abs() < 1e-6 && (last.y - p.y).abs() < 1e-6 {
                continue;
            }
        }
        points.push(*p);
    }
    if c.closed && points.len() > 2 {
        let (f, l) = (points[0], points[points.len() - 1]);
        if (f.x - l.x).abs() < 1e-6 && (f.y - l.y).abs() < 1e-6 {
            points.pop();
        }
    }
    Contour { points, closed: c.closed }
}

fn apply_dashes(c: &Contour, dashes: &[f32], offset: f32) -> Vec<Contour> {
    let c = dedupe(c);
    let mut pts = c.points.clone();
    if c.closed && !pts.is_empty() {
        pts.push(pts[0]);
    }
    let pattern: Vec<f32> = dashes.iter().map(|d| d.max(0.0)).collect();
    let total: f32 = pattern.iter().sum();
    if total <= 0.0 || pts.len() < 2 {
        return vec![c];
    }
    // Advance the pattern by the offset.
    let mut idx = 0;
    let mut remaining = pattern[0];
    let mut on = true;
    let mut off = offset.rem_euclid(total);
    while off > 0.0 {
        if off >= remaining {
            off -= remaining;
            idx = (idx + 1) % pattern.len();
            remaining = pattern[idx];
            on = !on;
        } else {
            remaining -= off;
            off = 0.0;
        }
    }
    let mut out = Vec::new();
    let mut cur: Vec<Point> = Vec::new();
    if on {
        cur.push(pts[0]);
    }
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let seg_len = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        let mut pos = 0.0;
        while seg_len - pos > remaining {
            pos += remaining;
            let t = pos / seg_len;
            let p = Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
            if on {
                cur.push(p);
                out.push(Contour { points: std::mem::take(&mut cur), closed: false });
            } else {
                cur.push(p);
            }
            on = !on;
            idx = (idx + 1) % pattern.len();
            remaining = pattern[idx];
            if remaining <= 0.0 {
                // zero-length entry: skip it
                idx = (idx + 1) % pattern.len();
                remaining = pattern[idx];
                on = !on;
            }
        }
        remaining -= seg_len - pos;
        if on {
            cur.push(b);
        } else {
            cur.clear();
        }
    }
    if on && cur.len() >= 2 {
        out.push(Contour { points: cur, closed: false });
    }
    out
}

fn stroke_contour(c: &Contour, hw: f32, style: &StrokeStyle, out: &mut Polylines) {
    let pts = &c.points;
    if pts.is_empty() {
        return;
    }
    if pts.len() == 1 {
        // A dot: only round/square caps produce ink.
        match style.cap {
            LineCap::Round => out.contours.push(circle(pts[0], hw)),
            LineCap::Square => out.contours.push(square(pts[0], hw)),
            LineCap::Butt => {}
        }
        return;
    }
    let n = pts.len();
    let seg_count = if c.closed { n } else { n - 1 };
    // Segment bodies
    for i in 0..seg_count {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        let mut body = segment_quad(a, b, hw);
        if !c.closed && style.cap == LineCap::Square {
            // Extend the first/last segment by hw.
            let d = direction(a, b);
            if i == 0 {
                body = segment_quad(Point::new(a.x - d.x * hw, a.y - d.y * hw), b, hw);
            }
            if i == seg_count - 1 {
                let a2 = if i == 0 { Point::new(a.x - d.x * hw, a.y - d.y * hw) } else { a };
                body = segment_quad(a2, Point::new(b.x + d.x * hw, b.y + d.y * hw), hw);
            }
        }
        out.contours.push(body);
    }
    // Joins
    let join_range = if c.closed { 0..n } else { 1..n - 1 };
    for i in join_range {
        let prev = pts[(i + n - 1) % n];
        let cur = pts[i];
        let next = pts[(i + 1) % n];
        add_join(prev, cur, next, hw, style, out);
    }
    // Caps
    if !c.closed && style.cap == LineCap::Round {
        out.contours.push(circle(pts[0], hw));
        out.contours.push(circle(pts[n - 1], hw));
    }
}

fn direction(a: Point, b: Point) -> Point {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-9 {
        Point::new(1.0, 0.0)
    } else {
        Point::new(dx / len, dy / len)
    }
}

/// Counter-clockwise (in y-down screen space, consistently oriented) quad.
fn segment_quad(a: Point, b: Point, hw: f32) -> Contour {
    let d = direction(a, b);
    let nx = -d.y * hw;
    let ny = d.x * hw;
    Contour {
        // Positive signed area, like `orient` (caps and joins must not
        // cancel the body under the non-zero rule).
        points: vec![
            Point::new(a.x - nx, a.y - ny),
            Point::new(b.x - nx, b.y - ny),
            Point::new(b.x + nx, b.y + ny),
            Point::new(a.x + nx, a.y + ny),
        ],
        closed: true,
    }
}

fn orient(mut pts: Vec<Point>) -> Contour {
    // Make the polygon orientation match `segment_quad` (positive signed area).
    let mut area = 0.0;
    for i in 0..pts.len() {
        let p = pts[i];
        let q = pts[(i + 1) % pts.len()];
        area += p.x * q.y - q.x * p.y;
    }
    if area < 0.0 {
        pts.reverse();
    }
    Contour { points: pts, closed: true }
}

fn circle(c: Point, r: f32) -> Contour {
    let n = ((r * 2.0).ceil() as usize).clamp(8, 64);
    let mut pts = Vec::with_capacity(n);
    for i in 0..n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        pts.push(Point::new(c.x + r * a.cos(), c.y + r * a.sin()));
    }
    orient(pts)
}

fn square(c: Point, hw: f32) -> Contour {
    orient(vec![
        Point::new(c.x - hw, c.y - hw),
        Point::new(c.x + hw, c.y - hw),
        Point::new(c.x + hw, c.y + hw),
        Point::new(c.x - hw, c.y + hw),
    ])
}

fn add_join(prev: Point, cur: Point, next: Point, hw: f32, style: &StrokeStyle, out: &mut Polylines) {
    let d0 = direction(prev, cur);
    let d1 = direction(cur, next);
    let cross = d0.x * d1.y - d0.y * d1.x;
    let dot = d0.x * d1.x + d0.y * d1.y;
    if cross.abs() < 1e-6 && dot > 0.0 {
        return; // collinear, no join needed
    }
    // Outer side: the side away from the turn direction.
    let sign = if cross > 0.0 { -1.0 } else { 1.0 };
    let n0 = Point::new(-d0.y * hw * sign, d0.x * hw * sign);
    let n1 = Point::new(-d1.y * hw * sign, d1.x * hw * sign);
    let p0 = Point::new(cur.x + n0.x, cur.y + n0.y);
    let p1 = Point::new(cur.x + n1.x, cur.y + n1.y);
    match style.join {
        LineJoin::Round => out.contours.push(circle(cur, hw)),
        LineJoin::Bevel => out.contours.push(orient(vec![cur, p0, p1])),
        LineJoin::Miter => {
            // Miter length ratio = 1 / sin(theta/2) where theta is the angle between segments.
            let cos_theta = (-d0.x * d1.x - d0.y * d1.y).clamp(-1.0, 1.0);
            let half = ((1.0 - cos_theta) / 2.0).max(1e-6).sqrt(); // sin(theta/2)
            let miter_ratio = 1.0 / half;
            if miter_ratio <= style.miter_limit.max(1.0) {
                // Miter point: intersection of the two offset lines.
                let mx = (n0.x + n1.x) / 2.0;
                let my = (n0.y + n1.y) / 2.0;
                let ml = (mx * mx + my * my).sqrt().max(1e-6);
                let scale = hw * hw / (ml * ml);
                let tip = Point::new(cur.x + mx * scale, cur.y + my * scale);
                out.contours.push(orient(vec![cur, p0, tip, p1]));
            } else {
                out.contours.push(orient(vec![cur, p0, p1]));
            }
        }
    }
}
