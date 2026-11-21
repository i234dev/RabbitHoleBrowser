//! Glyph rasterization with vertical grid fitting.
//!
//! Text on Linux is drawn by FreeType with "slight" hinting: outlines are
//! fitted to the pixel grid vertically only. Horizontal edges (the
//! baseline, x-height, cap-height and descender zones, and the edges of
//! horizontal stems) land on whole pixels; horizontal positions are left
//! alone, so subpixel positioning keeps working. This module does the same
//! on TrueType/CFF outlines:
//!
//! 1. the outline's horizontal edges (flat segments and vertical extrema)
//!    are found and classified as tops (ink below) or bottoms (ink above);
//! 2. edges near a blue zone of the face snap to the rounded zone (keeping
//!    a rounded overshoot), a bottom and the top just above it form a stem
//!    whose width is lightly quantized (as FreeType's light hinting does),
//!    edges within a pixel of those move with them, the rest round;
//! 3. every outline point moves by the piecewise-linear interpolation of
//!    the edge moves, and the outline is rasterized with exact area
//!    coverage.

use rustybuzz::ttf_parser::{self, GlyphId, OutlineBuilder};

/// Outline command in font units (y up).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Cmd {
    Move(f32, f32),
    Line(f32, f32),
    Quad(f32, f32, f32, f32),
    Cubic(f32, f32, f32, f32, f32, f32),
    Close,
}

#[derive(Default)]
struct Collect(Vec<Cmd>);

impl OutlineBuilder for Collect {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.push(Cmd::Move(x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.push(Cmd::Line(x, y));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.push(Cmd::Quad(x1, y1, x, y));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.push(Cmd::Cubic(x1, y1, x2, y2, x, y));
    }
    fn close(&mut self) {
        self.0.push(Cmd::Close);
    }
}

/// Which side of a horizontal edge the ink is on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Side {
    /// Ink below the edge (the top of a stroke).
    Top,
    /// Ink above the edge (the bottom of a stroke).
    Bottom,
}

/// A blue zone: a reference height (font units) shared by many glyphs,
/// e.g. the x-height, with the side its edges are on.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Zone {
    pub y: f32,
    pub side: Side,
}

/// Per-face data for grid fitting.
pub(crate) struct HintFace {
    /// Parsed from `bytes` (declared first: dropped before them).
    face: ttf_parser::Face<'static>,
    /// Keeps the bytes `face` borrows alive and unmoved.
    _bytes: std::rc::Rc<Vec<u8>>,
    pub upem: f32,
    pub zones: Vec<Zone>,
}

impl HintFace {
    pub(crate) fn new(bytes: std::rc::Rc<Vec<u8>>) -> Option<HintFace> {
        // SAFETY: the slice lives in the `Rc<Vec<u8>>` heap buffer, which is
        // never mutated and outlives `face` (stored next to it, dropped
        // after it); parsing per glyph would cost more than rasterizing.
        let data: &'static [u8] = unsafe { std::slice::from_raw_parts(bytes.as_ptr(), bytes.len()) };
        let face = ttf_parser::Face::parse(data, 0).ok()?;
        let upem = face.units_per_em() as f32;
        if !(upem > 0.0) {
            return None;
        }
        let top_of = |chars: &str| -> Option<f32> {
            chars.chars().find_map(|c| face.glyph_index(c).and_then(|g| face.glyph_bounding_box(g)).map(|b| b.y_max as f32))
        };
        let bottom_of = |chars: &str| -> Option<f32> {
            chars.chars().find_map(|c| face.glyph_index(c).and_then(|g| face.glyph_bounding_box(g)).map(|b| b.y_min as f32))
        };
        let mut zones = vec![Zone { y: 0.0, side: Side::Bottom }];
        // Flat letters give the reference heights (round ones overshoot).
        let x_height = top_of("xzvwy").or_else(|| face.x_height().map(|v| v as f32).filter(|v| *v > 0.0));
        let cap = top_of("HIEFTZ").or_else(|| face.capital_height().map(|v| v as f32).filter(|v| *v > 0.0));
        let asc = top_of("lhkbd");
        let desc = bottom_of("pq");
        for (v, side) in [(x_height, Side::Top), (cap, Side::Top), (asc, Side::Top), (desc, Side::Bottom)] {
            if let Some(v) = v {
                if v.is_finite() && !zones.iter().any(|z| z.side == side && (z.y - v).abs() < upem * 0.004) {
                    zones.push(Zone { y: v, side });
                }
            }
        }
        Some(HintFace { face, _bytes: bytes, upem, zones })
    }

    /// The outline of `glyph` in font units (None: no outline).
    pub(crate) fn outline(&self, glyph: u16) -> Option<Vec<Cmd>> {
        let mut c = Collect(Vec::with_capacity(64));
        self.face.outline_glyph(GlyphId(glyph), &mut c)?;
        Some(c.0)
    }
}

/// A horizontal edge of an outline (font units).
#[derive(Clone, Copy, Debug)]
struct Edge {
    y: f32,
    side: Side,
}

/// The end points of each segment of each contour: (start, first
/// control or end, last control or start, end), closing contours.
fn contours(cmds: &[Cmd]) -> Vec<Vec<[(f32, f32); 4]>> {
    let mut out: Vec<Vec<[(f32, f32); 4]>> = Vec::new();
    let mut cur: Vec<[(f32, f32); 4]> = Vec::new();
    let mut start = (0.0, 0.0);
    let mut pen = (0.0, 0.0);
    let finish = |cur: &mut Vec<[(f32, f32); 4]>, pen: (f32, f32), start: (f32, f32), out: &mut Vec<Vec<[(f32, f32); 4]>>| {
        if !cur.is_empty() {
            if pen != start {
                cur.push([pen, start, pen, start]);
            }
            out.push(std::mem::take(cur));
        }
    };
    for c in cmds {
        match *c {
            Cmd::Move(x, y) => {
                finish(&mut cur, pen, start, &mut out);
                start = (x, y);
                pen = start;
            }
            Cmd::Line(x, y) => {
                cur.push([pen, (x, y), pen, (x, y)]);
                pen = (x, y);
            }
            Cmd::Quad(x1, y1, x, y) => {
                cur.push([pen, (x1, y1), (x1, y1), (x, y)]);
                pen = (x, y);
            }
            Cmd::Cubic(x1, y1, x2, y2, x, y) => {
                cur.push([pen, (x1, y1), (x2, y2), (x, y)]);
                pen = (x, y);
            }
            Cmd::Close => {
                finish(&mut cur, pen, start, &mut out);
                pen = start;
            }
        }
    }
    finish(&mut cur, pen, start, &mut out);
    out
}

/// Horizontal edges of an outline: on-curve points on a flat segment or at
/// a vertical extremum, merged when close, sorted bottom to top.
fn find_edges(cmds: &[Cmd], upem: f32) -> Vec<Edge> {
    let cs = contours(cmds);
    // Signed area (shoelace on the end points): TrueType outer contours run
    // clockwise (negative area) with the ink on the right; CFF ones the
    // other way.
    let mut area = 0.0f32;
    for c in &cs {
        for s in c {
            area += s[0].0 * s[3].1 - s[3].0 * s[0].1;
        }
    }
    let ink_right = area <= 0.0;
    let min_len = upem * 0.01;
    let mut raw: Vec<(f32, Side, f32)> = Vec::new();
    for c in &cs {
        let n = c.len();
        for i in 0..n {
            let s = &c[i];
            let next = &c[(i + 1) % n];
            let p = s[3];
            // Incoming and outgoing tangents at the segment's end, skipping
            // degenerate controls.
            let din = {
                let d = (p.0 - s[2].0, p.1 - s[2].1);
                if d.0 == 0.0 && d.1 == 0.0 { (p.0 - s[0].0, p.1 - s[0].1) } else { d }
            };
            let dout = {
                let d = (next[1].0 - p.0, next[1].1 - p.1);
                if d.0 == 0.0 && d.1 == 0.0 { (next[3].0 - p.0, next[3].1 - p.1) } else { d }
            };
            let flat = |d: (f32, f32)| d.0 != 0.0 && d.1.abs() <= d.0.abs() * 0.12;
            // A vertical extremum: the tangents turn (a cusp or a corner),
            // or a horizontal tangent with both neighbouring points on the
            // same side (the top of a bowl).
            let turn = (din.1 > 0.0 && dout.1 < 0.0) || (din.1 < 0.0 && dout.1 > 0.0);
            let same_side = (s[0].1 < p.1 && next[3].1 < p.1) || (s[0].1 > p.1 && next[3].1 > p.1);
            let extremum = turn || (same_side && (din.1 == 0.0 || dout.1 == 0.0));
            // Flat segments count by length: short serif steps and joins
            // make no edge unless they are extrema.
            let len_in = if flat(din) { (p.0 - s[0].0).abs() } else { 0.0 };
            let len_out = if flat(dout) { (next[3].0 - p.0).abs() } else { 0.0 };
            if !extremum && len_in.max(len_out) < min_len {
                continue;
            }
            let dx = if len_out >= len_in && flat(dout) {
                dout.0
            } else if flat(din) {
                din.0
            } else {
                din.0 + dout.0
            };
            if dx == 0.0 {
                continue;
            }
            let side = if (dx > 0.0) == ink_right { Side::Top } else { Side::Bottom };
            raw.push((p.1, side, len_in.max(len_out)));
        }
    }
    raw.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let eps = upem * 0.006;
    let mut edges: Vec<Edge> = Vec::new();
    for (y, side, _) in raw {
        if let Some(last) = edges.iter_mut().rev().take(3).find(|e| e.side == side && (y - e.y).abs() <= eps) {
            // Keep the outermost position of a merged edge.
            last.y = if side == Side::Top { last.y.max(y) } else { last.y.min(y) };
            continue;
        }
        edges.push(Edge { y, side });
    }
    edges.sort_by(|a, b| a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal));
    edges
}

/// FreeType's light quantization of a stem width (px): widths stay close
/// to their design but near-integral, at least 7/8 px.
fn stem_width(w: f32) -> f32 {
    let w = w.max(56.0 / 64.0);
    if w >= 3.0 {
        return w.round();
    }
    let i = w.floor();
    let f = w - i;
    let f = if f < 10.0 / 64.0 {
        f
    } else if f < 0.5 {
        10.0 / 64.0
    } else if f < 54.0 / 64.0 {
        54.0 / 64.0
    } else {
        f
    };
    i + f
}

/// Grid-fitting knots for an outline at `scale` px per font unit:
/// (original y, fitted y) in px (y up), sorted, non-decreasing.
pub(crate) fn fit_knots(cmds: &[Cmd], upem: f32, zones: &[Zone], scale: f32) -> Vec<(f32, f32)> {
    let edges = find_edges(cmds, upem);
    if edges.is_empty() {
        return Vec::new();
    }
    let n = edges.len();
    let mut target: Vec<Option<f32>> = vec![None; n];
    // Blue zones: edges at most 3.5% em past the reference (overshoot) or
    // 1% em short of it.
    let over = upem * 0.035;
    let under = upem * 0.01;
    for (i, e) in edges.iter().enumerate() {
        let mut best: Option<(f32, f32)> = None;
        for z in zones.iter().filter(|z| z.side == e.side) {
            let d = e.y - z.y;
            let inside = match e.side {
                Side::Top => d >= -under && d <= over,
                Side::Bottom => d <= under && d >= -over,
            };
            if inside && best.is_none_or(|(bd, _)| d.abs() < bd) {
                best = Some((d.abs(), z.y));
            }
        }
        if let Some((_, zy)) = best {
            let r = (zy * scale).round();
            target[i] = Some(r + ((e.y - zy) * scale).round());
        }
    }
    // Stems: a bottom edge and the next edge when it is a top, close enough.
    let max_stem = upem * 0.2;
    let mut i = 0;
    while i + 1 < n {
        let (b, t) = (edges[i], edges[i + 1]);
        if b.side == Side::Bottom && t.side == Side::Top && t.y - b.y <= max_stem {
            let w = stem_width((t.y - b.y) * scale);
            match (target[i], target[i + 1]) {
                (Some(_), Some(_)) => {}
                (Some(bt), None) => target[i + 1] = Some(bt + w),
                (None, Some(tt)) => target[i] = Some(tt - w),
                (None, None) => {
                    let center = (b.y + t.y) * 0.5 * scale;
                    let bt = (center - w * 0.5).round();
                    target[i] = Some(bt);
                    target[i + 1] = Some(bt + w);
                }
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    // Other edges (serifs, steps next to a stem) within a pixel of a fitted
    // edge move with it, keeping their distance; the rest round.
    let fitted: Vec<(f32, f32)> = edges.iter().zip(&target).filter_map(|(e, t)| t.map(|t| (e.y * scale, t))).collect();
    let mut knots: Vec<(f32, f32)> = Vec::with_capacity(n);
    for (e, t) in edges.iter().zip(target) {
        let o = e.y * scale;
        let t = t.unwrap_or_else(|| {
            let near = fitted.iter().min_by(|a, b| (a.0 - o).abs().partial_cmp(&(b.0 - o).abs()).unwrap_or(std::cmp::Ordering::Equal));
            match near {
                Some(&(fo, ft)) if (fo - o).abs() <= 1.0 => o + (ft - fo),
                _ => o.round(),
            }
        });
        match knots.last_mut() {
            Some(last) if o - last.0 < 1e-3 => {}
            Some(last) => {
                let t = t.max(last.1);
                knots.push((o, t));
            }
            None => knots.push((o, t)),
        }
    }
    knots
}

/// Piecewise-linear map of a y coordinate (px) through fitting knots.
#[inline]
pub(crate) fn fit_y(knots: &[(f32, f32)], y: f32) -> f32 {
    let Some(first) = knots.first() else { return y };
    if y <= first.0 {
        return y + (first.1 - first.0);
    }
    let last = knots[knots.len() - 1];
    if y >= last.0 {
        return y + (last.1 - last.0);
    }
    let mut i = 1;
    while knots[i].0 < y {
        i += 1;
    }
    let (a, b) = (knots[i - 1], knots[i]);
    a.1 + (y - a.0) * (b.1 - a.1) / (b.0 - a.0)
}

/// Rasterized coverage: `left`/`top` place the bitmap relative to the glyph
/// origin (top measured upwards from the baseline).
pub(crate) struct Raster {
    pub width: u32,
    pub height: u32,
    pub left: i32,
    pub top: i32,
    pub coverage: Vec<u8>,
}

/// Rasterize an outline at `scale` px per font unit, shifted right by
/// `shift` px, with its y coordinates (px, y up) mapped through `knots`.
pub(crate) fn rasterize(cmds: &[Cmd], scale: f32, shift: f32, knots: &[(f32, f32)]) -> Raster {
    // Device points (y up) of every command; the bounds of the control
    // points contain the curves.
    let mut pts: Vec<(f32, f32)> = Vec::with_capacity(cmds.len() * 2);
    let mut push = |x: f32, y: f32| pts.push((x * scale + shift, fit_y(knots, y * scale)));
    for c in cmds {
        match *c {
            Cmd::Move(x, y) | Cmd::Line(x, y) => push(x, y),
            Cmd::Quad(x1, y1, x, y) => {
                push(x1, y1);
                push(x, y);
            }
            Cmd::Cubic(x1, y1, x2, y2, x, y) => {
                push(x1, y1);
                push(x2, y2);
                push(x, y);
            }
            Cmd::Close => {}
        }
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in &pts {
        x0 = x0.min(p.0);
        x1 = x1.max(p.0);
        y0 = y0.min(p.1);
        y1 = y1.max(p.1);
    }
    let empty = Raster { width: 0, height: 0, left: 0, top: 0, coverage: Vec::new() };
    if pts.len() < 2 || !(x0.is_finite() && x1.is_finite() && y0.is_finite() && y1.is_finite()) || x1 <= x0 || y1 <= y0 {
        return empty;
    }
    // Snap bounds that are within float noise of a pixel boundary.
    let fl = |v: f32| if (v - v.round()).abs() < 1e-3 { v.round() } else { v.floor() };
    let ce = |v: f32| if (v - v.round()).abs() < 1e-3 { v.round() } else { v.ceil() };
    let left = fl(x0) as i32;
    let right = ce(x1) as i32;
    let bottom = fl(y0) as i32;
    let top = ce(y1) as i32;
    let w = (right - left).max(1) as usize;
    let h = (top - bottom).max(1) as usize;
    if w.saturating_mul(h) > 64 << 20 {
        return empty;
    }
    let mut acc = Accumulator::new(w, h);
    // Bitmap space: x from the left edge, y down from the top.
    let (ox, oy) = (left as f32, top as f32);
    let bm = |p: (f32, f32)| (p.0 - ox, oy - p.1);
    let mut i = 0;
    let mut start = (0.0f32, 0.0f32);
    let mut pen = start;
    let mut open = false;
    for c in cmds {
        match *c {
            Cmd::Move(..) => {
                if open {
                    acc.line(pen, start);
                }
                start = bm(pts[i]);
                pen = start;
                open = true;
                i += 1;
            }
            Cmd::Line(..) => {
                let p = bm(pts[i]);
                acc.line(pen, p);
                pen = p;
                i += 1;
            }
            Cmd::Quad(..) => {
                let (c1, p) = (bm(pts[i]), bm(pts[i + 1]));
                i += 2;
                let dd = ((pen.0 - 2.0 * c1.0 + p.0).powi(2) + (pen.1 - 2.0 * c1.1 + p.1).powi(2)).sqrt();
                let steps = ceil_pos((dd * 2.5).sqrt()).clamp(1, 64);
                let inv = 1.0 / steps as f32;
                let mut prev = pen;
                for k in 1..steps {
                    let t = k as f32 * inv;
                    let u = 1.0 - t;
                    let q = (u * u * pen.0 + 2.0 * u * t * c1.0 + t * t * p.0, u * u * pen.1 + 2.0 * u * t * c1.1 + t * t * p.1);
                    acc.line(prev, q);
                    prev = q;
                }
                acc.line(prev, p);
                pen = p;
            }
            Cmd::Cubic(..) => {
                let (c1, c2, p) = (bm(pts[i]), bm(pts[i + 1]), bm(pts[i + 2]));
                i += 3;
                let d1 = ((pen.0 - 2.0 * c1.0 + c2.0).powi(2) + (pen.1 - 2.0 * c1.1 + c2.1).powi(2)).sqrt();
                let d2 = ((c1.0 - 2.0 * c2.0 + p.0).powi(2) + (c1.1 - 2.0 * c2.1 + p.1).powi(2)).sqrt();
                let steps = ceil_pos((d1.max(d2) * 15.0).sqrt()).clamp(1, 64);
                let inv = 1.0 / steps as f32;
                let mut prev = pen;
                for k in 1..steps {
                    let t = k as f32 * inv;
                    let u = 1.0 - t;
                    let (a, b, cc, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                    let q = (a * pen.0 + b * c1.0 + cc * c2.0 + d * p.0, a * pen.1 + b * c1.1 + cc * c2.1 + d * p.1);
                    acc.line(prev, q);
                    prev = q;
                }
                acc.line(prev, p);
                pen = p;
            }
            Cmd::Close => {
                if open {
                    acc.line(pen, start);
                    pen = start;
                }
                open = false;
            }
        }
    }
    if open {
        acc.line(pen, start);
    }
    Raster { width: w as u32, height: h as u32, left, top, coverage: acc.finish() }
}

/// `v.ceil()` for non-negative `v`, without a libm call.
#[inline(always)]
fn ceil_pos(v: f32) -> usize {
    let t = v as usize;
    if (t as f32) < v { t + 1 } else { t }
}

/// Exact-area coverage accumulation (signed area per cell, then a running
/// sum along each row), as in font-rs / fontdue.
struct Accumulator {
    w: usize,
    h: usize,
    a: Vec<f32>,
}

impl Accumulator {
    fn new(w: usize, h: usize) -> Accumulator {
        Accumulator { w, h, a: vec![0.0; w * h + 4] }
    }

    fn line(&mut self, p0: (f32, f32), p1: (f32, f32)) {
        if (p0.1 - p1.1).abs() <= f32::EPSILON {
            return;
        }
        let (dir, p0, p1) = if p0.1 < p1.1 { (1.0f32, p0, p1) } else { (-1.0f32, p1, p0) };
        let dxdy = (p1.0 - p0.0) / (p1.1 - p0.1);
        let mut x = p0.0;
        let y_start = p0.1.max(0.0);
        if p0.1 < 0.0 {
            x -= p0.1 * dxdy;
        }
        let wmax = self.w as f32;
        let y_end = ceil_pos(p1.1.max(0.0)).min(self.h);
        for y in (y_start as usize)..y_end {
            let line_start = y * self.w;
            let dy = ((y + 1) as f32).min(p1.1) - (y as f32).max(p0.1);
            let xnext = x + dxdy * dy;
            let d = dy * dir;
            let (xa, xb) = if x < xnext { (x, xnext) } else { (xnext, x) };
            let (xa, xb) = (xa.clamp(0.0, wmax), xb.clamp(0.0, wmax));
            // (Non-negative: truncation is floor; libm calls are slow.)
            let xa_i = xa as usize;
            let xa_floor = xa_i as f32;
            let xb_i = ceil_pos(xb);
            let xb_ceil = xb_i as f32;
            if xb_i <= xa_i + 1 {
                let xmf = 0.5 * (x + xnext).clamp(0.0, 2.0 * wmax) - xa_floor;
                let i = line_start + xa_i;
                self.a[i] += d - d * xmf;
                self.a[i + 1] += d * xmf;
            } else {
                let s = (xb - xa).recip();
                let xa_f = xa - xa_floor;
                let a0 = 0.5 * s * (1.0 - xa_f) * (1.0 - xa_f);
                let xb_f = xb - xb_ceil + 1.0;
                let am = 0.5 * s * xb_f * xb_f;
                let i = line_start + xa_i;
                self.a[i] += d * a0;
                if xb_i == xa_i + 2 {
                    self.a[i + 1] += d * (1.0 - a0 - am);
                } else {
                    let a1 = s * (1.5 - xa_f);
                    self.a[i + 1] += d * (a1 - a0);
                    for xi in xa_i + 2..xb_i - 1 {
                        self.a[line_start + xi] += d * s;
                    }
                    let a2 = a1 + (xb_i - xa_i - 3) as f32 * s;
                    self.a[line_start + xb_i - 1] += d * (1.0 - a2 - am);
                }
                self.a[line_start + xb_i] += d * am;
            }
            x = xnext;
        }
    }

    fn finish(self) -> Vec<u8> {
        let n = self.w * self.h;
        let mut out = vec![0u8; n];
        let mut sum = 0.0f32;
        for (o, v) in out.iter_mut().zip(&self.a[..n]) {
            sum += v;
            *o = (sum.abs() * 255.9).min(255.0) as u8;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A TrueType-style (clockwise) rectangle in font units.
    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<Cmd> {
        vec![Cmd::Move(x0, y0), Cmd::Line(x0, y1), Cmd::Line(x1, y1), Cmd::Line(x1, y0), Cmd::Close]
    }

    fn rows(r: &Raster) -> Vec<Vec<u8>> {
        r.coverage.chunks(r.width as usize).map(|c| c.to_vec()).collect()
    }

    #[test]
    fn unhinted_coverage_is_exact_area() {
        // 1000 upem at 24 px: the square spans -4.8..19.2 px.
        let sq = rect(0.0, -200.0, 1000.0, 800.0);
        let r = rasterize(&sq, 0.024, 0.0, &[]);
        assert_eq!((r.left, r.top, r.width, r.height), (0, 20, 24, 25));
        let rs = rows(&r);
        assert!((rs[0][5] as i32 - 51).abs() <= 1, "top row 0.2 covered: {}", rs[0][5]);
        assert_eq!(rs[1][5], 255);
        assert!((rs[24][5] as i32 - 204).abs() <= 1, "bottom row 0.8 covered: {}", rs[24][5]);
    }

    #[test]
    fn ahem_square_snaps_to_whole_pixels() {
        let sq = rect(0.0, -200.0, 1000.0, 800.0);
        let zones = [Zone { y: 0.0, side: Side::Bottom }, Zone { y: 800.0, side: Side::Top }, Zone { y: -200.0, side: Side::Bottom }];
        for size in [16.0f32, 19.0, 24.0, 26.5, 31.0, 37.3, 50.0, 97.0] {
            let scale = size / 1000.0;
            let knots = fit_knots(&sq, 1000.0, &zones, scale);
            let r = rasterize(&sq, scale, 0.0, &knots);
            assert_eq!(r.top, (0.8 * size).round() as i32, "size {size}");
            assert_eq!(r.top - r.height as i32, -(0.2 * size).round() as i32, "size {size}");
            // Every row is fully covered: no partly covered edge rows.
            let full = rows(&r).iter().all(|row| row[0] == 255 && row[r.width as usize - 2] == 255);
            assert!(full, "size {size}: {:?}", rows(&r).iter().map(|row| row[0]).collect::<Vec<_>>());
        }
    }

    #[test]
    fn lone_edges_round_without_zones() {
        // No zones: edges round to the nearest pixel.
        let sq = rect(0.0, -200.0, 1000.0, 800.0);
        let knots = fit_knots(&sq, 1000.0, &[], 0.024);
        let r = rasterize(&sq, 0.024, 0.0, &knots);
        assert_eq!((r.top, r.height), (19, 24));
        assert!(rows(&r).iter().all(|row| row[3] == 255));
    }

    #[test]
    fn subpixel_shift_is_horizontal_only() {
        let sq = rect(0.0, -200.0, 1000.0, 800.0);
        let zones = [Zone { y: 0.0, side: Side::Bottom }, Zone { y: 800.0, side: Side::Top }, Zone { y: -200.0, side: Side::Bottom }];
        let knots = fit_knots(&sq, 1000.0, &zones, 0.024);
        let r = rasterize(&sq, 0.024, 0.25, &knots);
        assert_eq!((r.left, r.width, r.top, r.height), (0, 25, 19, 24));
        for row in rows(&r) {
            assert!((row[0] as i32 - 191).abs() <= 1, "left column 0.75 covered: {}", row[0]);
            assert_eq!(row[1], 255);
            assert!((row[24] as i32 - 64).abs() <= 1, "right column 0.25 covered: {}", row[24]);
        }
    }

    #[test]
    fn thin_stems_keep_at_least_most_of_a_pixel() {
        // A 0.4 px bar floating at 5.3..5.7 px (no zones): it becomes one
        // nearly opaque row instead of two faint ones.
        let bar = rect(0.0, 530.0, 1000.0, 570.0);
        let knots = fit_knots(&bar, 1000.0, &[], 0.01);
        let r = rasterize(&bar, 0.01, 0.0, &knots);
        assert_eq!(r.height, 1, "{:?}", rows(&r));
        assert!(r.coverage[3] >= 200, "{}", r.coverage[3]);
    }

    #[test]
    fn stems_attach_to_zones() {
        // An 'o'-like ring: outer 0..500 overshooting both zones a little,
        // counter 60..440; at 13 px the bowl strokes stay whole-ish and the
        // outer edges sit on the zones.
        let mut ring = rect(0.0, -8.0, 500.0, 508.0);
        // Counter, counter-clockwise.
        ring.extend([Cmd::Move(80.0, 60.0), Cmd::Line(420.0, 60.0), Cmd::Line(420.0, 440.0), Cmd::Line(80.0, 440.0), Cmd::Close]);
        let zones = [Zone { y: 0.0, side: Side::Bottom }, Zone { y: 500.0, side: Side::Top }];
        let knots = fit_knots(&ring, 1000.0, &zones, 0.013);
        let r = rasterize(&ring, 0.013, 0.0, &knots);
        assert_eq!((r.top, r.top - r.height as i32), (7, 0));
        let rs = rows(&r);
        // The outer edges are whole rows; the 0.9 px bowl strokes keep
        // most of a pixel.
        assert_eq!(rs[0][0], 255);
        assert_eq!(rs[r.height as usize - 1][0], 255);
        assert!(rs[0][3] >= 200 && rs[r.height as usize - 1][3] >= 200, "{rs:?}");
    }

    #[test]
    fn knots_are_monotonic() {
        let knots = [(0.0, 0.0), (1.0, 2.0), (3.0, 3.0)];
        assert_eq!(fit_y(&knots, -1.0), -1.0);
        assert_eq!(fit_y(&knots, 0.5), 1.0);
        assert_eq!(fit_y(&knots, 2.0), 2.5);
        assert_eq!(fit_y(&knots, 4.0), 4.0);
    }
}

