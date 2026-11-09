//! Scanline coverage rasterizer for polygons (non-zero / even-odd) and
//! analytic coverage for axis-aligned rounded rectangles.
//!
//! Polygons are sampled on `SUBSAMPLES` sub-scanlines per pixel row; the
//! horizontal extent of every span is accumulated with exact fractional
//! coverage, so edges are smooth in both directions.

use crate::canvas::{FillRule, Radii, Rect};
use crate::path::{Contour, Polylines};

/// 8-bit coverage of a rectangular pixel region.
#[derive(Debug, Clone)]
pub struct Mask {
    pub x0: i32,
    pub y0: i32,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Mask {
    pub fn empty() -> Mask {
        Mask { x0: 0, y0: 0, width: 0, height: 0, data: Vec::new() }
    }
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
    #[inline]
    pub fn get(&self, x: i32, y: i32) -> u8 {
        let lx = x - self.x0;
        let ly = y - self.y0;
        if lx < 0 || ly < 0 || lx >= self.width as i32 || ly >= self.height as i32 {
            return 0;
        }
        self.data[ly as usize * self.width as usize + lx as usize]
    }
    pub fn rect(&self) -> Rect {
        Rect::new(self.x0 as f32, self.y0 as f32, self.width as f32, self.height as f32)
    }
}

const SUBSAMPLES: usize = 5;

struct Edge {
    x0: f32,
    y0: f32,
    y1: f32,
    dir: i32,
    /// dx/dy
    slope: f32,
}

struct Crossing {
    x: f32,
    dir: i32,
}

/// Rasterize polylines (device coordinates) into a mask limited to the
/// integer `limit` rectangle (clip ∩ canvas), as [x0, y0, x1, y1).
pub fn rasterize(poly: &Polylines, rule: FillRule, limit: (i32, i32, i32, i32)) -> Mask {
    let mut edges: Vec<Edge> = Vec::new();
    let mut miny = f32::MAX;
    let mut maxy = f32::MIN;
    let mut minx = f32::MAX;
    let mut maxx = f32::MIN;
    for c in &poly.contours {
        add_contour_edges(c, &mut edges);
        for p in &c.points {
            miny = miny.min(p.y);
            maxy = maxy.max(p.y);
            minx = minx.min(p.x);
            maxx = maxx.max(p.x);
        }
    }
    if edges.is_empty() || !miny.is_finite() || !maxy.is_finite() {
        return Mask::empty();
    }
    let (lx0, ly0, lx1, ly1) = limit;
    let x0 = (minx.floor() as i32).max(lx0);
    let y0 = (miny.floor() as i32).max(ly0);
    let x1 = (maxx.ceil() as i32).min(lx1);
    let y1 = (maxy.ceil() as i32).min(ly1);
    if x1 <= x0 || y1 <= y0 {
        return Mask::empty();
    }
    let width = (x1 - x0) as usize;
    let height = (y1 - y0) as usize;
    let mut data = vec![0u8; width * height];
    edges.sort_by(|a, b| a.y0.partial_cmp(&b.y0).unwrap_or(std::cmp::Ordering::Equal));

    // Per-row accumulation: `acc` holds coverage deltas (difference array)
    // with fractional contributions on the span ends.
    let mut acc = vec![0.0f32; width + 2];
    let mut active: Vec<usize> = Vec::new();
    let mut next_edge = 0;
    let mut crossings: Vec<Crossing> = Vec::new();
    let inc = 1.0 / SUBSAMPLES as f32;

    for row in 0..height {
        let py = (y0 + row as i32) as f32;
        acc.iter_mut().for_each(|v| *v = 0.0);
        let mut touched = false;
        for s in 0..SUBSAMPLES {
            let sy = py + (s as f32 + 0.5) * inc;
            // Activate edges starting above this sample line.
            while next_edge < edges.len() && edges[next_edge].y0 <= sy {
                active.push(next_edge);
                next_edge += 1;
            }
            // Drop finished edges.
            active.retain(|&i| edges[i].y1 > sy);
            if active.is_empty() {
                continue;
            }
            crossings.clear();
            for &i in &active {
                let e = &edges[i];
                if e.y0 <= sy && sy < e.y1 {
                    let x = e.x0 + (sy - e.y0) * e.slope;
                    crossings.push(Crossing { x, dir: e.dir });
                }
            }
            if crossings.len() < 2 {
                continue;
            }
            crossings.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
            let mut winding = 0;
            for i in 0..crossings.len() - 1 {
                winding += crossings[i].dir;
                let inside = match rule {
                    FillRule::NonZero => winding != 0,
                    FillRule::EvenOdd => winding % 2 != 0,
                };
                if !inside {
                    continue;
                }
                let xa = crossings[i].x.max(x0 as f32);
                let xb = crossings[i + 1].x.min(x1 as f32);
                if xb <= xa {
                    continue;
                }
                touched = true;
                add_span(&mut acc, xa - x0 as f32, xb - x0 as f32, inc, width);
            }
        }
        if !touched {
            continue;
        }
        // Resolve the difference array into coverage.
        let rowdata = &mut data[row * width..(row + 1) * width];
        let mut running = 0.0f32;
        for (x, px) in rowdata.iter_mut().enumerate() {
            running += acc[x];
            let c = running.clamp(0.0, 1.0);
            *px = (c * 255.0 + 0.5) as u8;
        }
    }
    Mask { x0, y0, width: width as u32, height: height as u32, data }
}

/// Add `weight` coverage over the horizontal interval [xa, xb) (local
/// coordinates) into the difference array `acc` with fractional ends.
#[inline]
fn add_span(acc: &mut [f32], xa: f32, xb: f32, weight: f32, width: usize) {
    let ia = xa.floor() as usize;
    let ib = xb.floor() as usize;
    if ia >= width {
        return;
    }
    if ia == ib {
        // Within a single pixel
        let c = (xb - xa) * weight;
        acc[ia] += c;
        acc[ia + 1] -= c;
        return;
    }
    // First partial pixel
    let ca = (ia as f32 + 1.0 - xa) * weight;
    acc[ia] += ca;
    acc[ia + 1] -= ca;
    // Full pixels ia+1 .. ib-1
    if ib > ia + 1 {
        acc[ia + 1] += weight;
        acc[ib.min(width)] -= weight;
    }
    // Last partial pixel
    if ib < width {
        let cb = (xb - ib as f32) * weight;
        acc[ib] += cb;
        acc[ib + 1] -= cb;
    }
}

fn add_contour_edges(c: &Contour, edges: &mut Vec<Edge>) {
    let n = c.points.len();
    if n < 2 {
        return;
    }
    // Polygons are always treated as closed for filling.
    for i in 0..n {
        let a = c.points[i];
        let b = c.points[(i + 1) % n];
        if (a.y - b.y).abs() < 1e-9 {
            continue;
        }
        let (p, q, dir) = if a.y < b.y { (a, b, 1) } else { (b, a, -1) };
        edges.push(Edge { x0: p.x, y0: p.y, y1: q.y, dir, slope: (q.x - p.x) / (q.y - p.y) });
    }
}

/// Coverage evaluator for an axis-aligned rounded rectangle with
/// per-corner elliptical radii. Straight runs use exact span coverage;
/// corner cells are 4×4 supersampled.
#[derive(Clone, Copy)]
pub struct RoundedBox {
    pub r: Rect,
    pub radii: Radii,
}

impl RoundedBox {
    pub fn new(r: Rect, radii: Radii) -> RoundedBox {
        RoundedBox { r, radii: crate::path::clamp_radii(r, radii) }
    }

    pub fn is_plain(&self) -> bool {
        self.radii.iter().all(|c| c.0 <= 0.0 || c.1 <= 0.0)
    }

    #[inline]
    fn inside(&self, px: f32, py: f32) -> bool {
        let r = &self.r;
        if px < r.x || py < r.y || px >= r.right() || py >= r.bottom() {
            return false;
        }
        let corners = [
            (0, r.x + self.radii[0].0, r.y + self.radii[0].1, px < r.x + self.radii[0].0 && py < r.y + self.radii[0].1),
            (1, r.right() - self.radii[1].0, r.y + self.radii[1].1, px > r.right() - self.radii[1].0 && py < r.y + self.radii[1].1),
            (2, r.right() - self.radii[2].0, r.bottom() - self.radii[2].1, px > r.right() - self.radii[2].0 && py > r.bottom() - self.radii[2].1),
            (3, r.x + self.radii[3].0, r.bottom() - self.radii[3].1, px < r.x + self.radii[3].0 && py > r.bottom() - self.radii[3].1),
        ];
        for (i, cx, cy, in_corner) in corners {
            if in_corner {
                let (rx, ry) = self.radii[i];
                if rx <= 0.0 || ry <= 0.0 {
                    return true;
                }
                let dx = (px - cx) / rx;
                let dy = (py - cy) / ry;
                return dx * dx + dy * dy <= 1.0;
            }
        }
        true
    }

    /// Does pixel (x, y) touch any corner region?
    #[inline]
    fn in_corner_cell(&self, fx: f32, fy: f32) -> bool {
        let r = &self.r;
        let rd = &self.radii;
        let tl = fx + 1.0 > r.x && fx < r.x + rd[0].0 && fy + 1.0 > r.y && fy < r.y + rd[0].1;
        let tr = fx + 1.0 > r.right() - rd[1].0 && fx < r.right() && fy + 1.0 > r.y && fy < r.y + rd[1].1;
        let br = fx + 1.0 > r.right() - rd[2].0 && fx < r.right() && fy + 1.0 > r.bottom() - rd[2].1 && fy < r.bottom();
        let bl = fx + 1.0 > r.x && fx < r.x + rd[3].0 && fy + 1.0 > r.bottom() - rd[3].1 && fy < r.bottom();
        tl || tr || br || bl
    }

    pub fn coverage(&self, x: i32, y: i32) -> f32 {
        let fx = x as f32;
        let fy = y as f32;
        if self.in_corner_cell(fx, fy) {
            let mut hits = 0;
            for sy in 0..4 {
                let py = fy + (sy as f32 + 0.5) / 4.0;
                for sx in 0..4 {
                    let px = fx + (sx as f32 + 0.5) / 4.0;
                    if self.inside(px, py) {
                        hits += 1;
                    }
                }
            }
            hits as f32 / 16.0
        } else {
            span(fx, self.r.x, self.r.right()) * span(fy, self.r.y, self.r.bottom())
        }
    }

    /// Render the coverage into a mask limited to `limit` [x0,y0,x1,y1).
    pub fn mask(&self, limit: (i32, i32, i32, i32)) -> Mask {
        let r = self.r;
        let x0 = (r.x.floor() as i32).max(limit.0);
        let y0 = (r.y.floor() as i32).max(limit.1);
        let x1 = (r.right().ceil() as i32).min(limit.2);
        let y1 = (r.bottom().ceil() as i32).min(limit.3);
        if x1 <= x0 || y1 <= y0 {
            return Mask::empty();
        }
        let w = (x1 - x0) as usize;
        let h = (y1 - y0) as usize;
        let mut data = vec![0u8; w * h];
        let rd = &self.radii;
        // Vertical extent of the corner bands.
        let top_band = r.y + rd[0].1.max(rd[1].1);
        let bottom_band = r.bottom() - rd[2].1.max(rd[3].1);
        // Horizontal extent of the corner columns.
        let left_cols = r.x + rd[0].0.max(rd[3].0);
        let right_cols = r.right() - rd[1].0.max(rd[2].0);
        for y in y0..y1 {
            let row = (y - y0) as usize * w;
            let fy = y as f32;
            let cy = span(fy, r.y, r.bottom());
            if cy <= 0.0 {
                continue;
            }
            let in_band = fy < top_band || fy + 1.0 > bottom_band;
            if !in_band {
                // Straight run: interior pixels are fully covered (× cy);
                // only the two edge columns are partial.
                let full = (cy * 255.0 + 0.5) as u8;
                let xa = (r.x.ceil() as i32).clamp(x0, x1);
                let xb = (r.right().floor() as i32).clamp(x0, x1);
                for x in x0..xa.min(x1) {
                    data[row + (x - x0) as usize] = (span(x as f32, r.x, r.right()) * cy * 255.0 + 0.5) as u8;
                }
                if xb > xa {
                    data[row + (xa - x0) as usize..row + (xb - x0) as usize].fill(full);
                }
                for x in xb.max(x0)..x1 {
                    data[row + (x - x0) as usize] = (span(x as f32, r.x, r.right()) * cy * 255.0 + 0.5) as u8;
                }
                continue;
            }
            for x in x0..x1 {
                let fx = x as f32;
                let in_cols = fx < left_cols || fx + 1.0 > right_cols;
                let c = if in_cols && self.in_corner_cell(fx, fy) {
                    self.coverage(x, y)
                } else {
                    span(fx, r.x, r.right()) * cy
                };
                data[row + (x - x0) as usize] = (c * 255.0 + 0.5) as u8;
            }
        }
        Mask { x0, y0, width: w as u32, height: h as u32, data }
    }
}

/// Coverage of the pixel span [p, p+1) by the interval [a, b), in 0..=1.
#[inline(always)]
pub fn span(p: f32, a: f32, b: f32) -> f32 {
    (b.min(p + 1.0) - a.max(p)).clamp(0.0, 1.0)
}

/// Separable box blur of an 8-bit mask, applied `passes` times (3 passes ≈
/// gaussian). `radius` is the half-width of the box in pixels.
pub fn box_blur(data: &mut [u8], width: usize, height: usize, radius: usize, passes: usize) {
    if radius == 0 || width == 0 || height == 0 {
        return;
    }
    let mut tmp = vec![0u8; width * height];
    for _ in 0..passes {
        blur_horizontal(data, &mut tmp, width, height, radius);
        blur_vertical(&tmp, data, width, height, radius);
    }
}

fn blur_horizontal(src: &[u8], dst: &mut [u8], width: usize, height: usize, radius: usize) {
    let win = 2 * radius + 1;
    for y in 0..height {
        let row = &src[y * width..(y + 1) * width];
        let out = &mut dst[y * width..(y + 1) * width];
        let mut sum: u32 = 0;
        for x in 0..radius.min(width) {
            sum += row[x] as u32;
        }
        for x in 0..width {
            let add = x + radius;
            if add < width {
                sum += row[add] as u32;
            }
            out[x] = (sum / win as u32) as u8;
            if x >= radius {
                sum -= row[x - radius] as u32;
            }
        }
    }
}

fn blur_vertical(src: &[u8], dst: &mut [u8], width: usize, height: usize, radius: usize) {
    let win = 2 * radius + 1;
    let mut sums = vec![0u32; width];
    for y in 0..radius.min(height) {
        for x in 0..width {
            sums[x] += src[y * width + x] as u32;
        }
    }
    for y in 0..height {
        let add = y + radius;
        if add < height {
            for x in 0..width {
                sums[x] += src[add * width + x] as u32;
            }
        }
        for x in 0..width {
            dst[y * width + x] = (sums[x] / win as u32) as u8;
        }
        if y >= radius {
            for x in 0..width {
                sums[x] -= src[(y - radius) * width + x] as u32;
            }
        }
    }
}

/// Box radius for 3-pass blur approximating a gaussian with `sigma`.
pub fn blur_radius_for_sigma(sigma: f32) -> usize {
    if sigma <= 0.0 {
        return 0;
    }
    // Ideal box width for 3 passes: w = sqrt(12σ²/3 + 1)
    let w = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt();
    (((w - 1.0) / 2.0).round() as usize).max(1)
}
