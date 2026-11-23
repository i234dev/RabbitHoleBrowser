//! Float bookkeeping for a block formatting context.

use rabbithole_gfx::{Point, Rect};
use std::sync::Arc;
use rabbithole_style::Clear;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FloatInfo {
    /// Margin box.
    rect: Rect,
    left: bool,
    /// Flattened contours in local margin-box coordinates, shared with layout caches.
    outline: Option<Arc<[Vec<Point>]>>,
    shape_margin: f32,
}

impl FloatInfo {
    fn translated(&self, x: f32, y: f32) -> Self {
        Self { rect: Rect::new(self.rect.x+x, self.rect.y+y, self.rect.w, self.rect.h), ..self.clone() }
    }

    fn inline_edge(&self, y: f32, h: f32) -> Option<f32> {
        let Some(contours) = &self.outline else {
            return Some(if self.left { self.rect.right() } else { self.rect.x });
        };
        let top = (y - self.rect.y).max(0.0);
        let bottom = (y + h - self.rect.y).min(self.rect.h);
        let radius = self.shape_margin;
        let sign = if self.left { 1.0 } else { -1.0 };
        let mut edge = f32::NEG_INFINITY;
        for points in contours.iter() {
            for (a, b) in points.iter().zip(points.iter().cycle().skip(1)) {
                if a.y.max(b.y) + radius <= top || a.y.min(b.y) - radius >= bottom { continue; }
                // Project the segment and its circular expansion onto the line band.
                let (dx, dy) = (sign * (b.x-a.x), b.y-a.y);
                let len = dx.hypot(dy);
                if len == 0.0 { continue; }
                let (nx, ny) = if dy >= 0.0 { (dy/len, -dx/len) } else { (-dy/len, dx/len) };
                let (x0, y0) = (sign*a.x + radius*nx, a.y + radius*ny);
                if dy != 0.0 {
                    for bound in [top,bottom] {
                        let t = (bound-y0)/dy;
                        if (0.0..=1.0).contains(&t) { edge = edge.max(x0+t*dx); }
                    }
                }
                for p in [a,b] {
                    let distance = (p.y.clamp(top,bottom)-p.y).abs();
                    if distance <= radius {
                        edge = edge.max(sign*p.x + ((radius-distance)*(radius+distance)).sqrt());
                    }
                }
            }
        }
        edge.is_finite().then(|| self.rect.x + (sign*edge).clamp(0.0,self.rect.w.max(0.0)))
    }
}

/// Does a float's margin box overlap the band `[y, y+h)`? Touching edges
/// (within float rounding noise, e.g. after translating a laid out
/// subtree) do not count.
fn overlaps(r: Rect, y: f32, h: f32) -> bool {
    const EPS: f32 = 0.01;
    r.y < y + h - EPS && r.bottom() > y + EPS
}

/// Floats placed in one block formatting context (document coordinates).
#[derive(Debug, Default)]
pub(crate) struct Bfc {
    floats: Vec<FloatInfo>,
}

/// A float recorded relative to a cached layout origin.
pub(crate) type RelFloat = FloatInfo;

/// Same floats within rounding noise?
pub(crate) fn same_floats(a: &[RelFloat], b: &[RelFloat]) -> bool {
    const EPS: f32 = 0.01;
    a.len() == b.len()
        && a.iter().zip(b).all(|(p, q)| {
            p.left == q.left && p.shape_margin == q.shape_margin && p.outline == q.outline
                && (p.rect.x-q.rect.x).abs() < EPS && (p.rect.y-q.rect.y).abs() < EPS
                && (p.rect.w-q.rect.w).abs() < EPS && (p.rect.h-q.rect.h).abs() < EPS
        })
}

impl Bfc {
    pub fn new() -> Self {
        Bfc::default()
    }

    /// The floats placed so far (to undo a trial layout).
    pub fn snapshot(&self) -> Vec<FloatInfo> {
        self.floats.clone()
    }

    pub fn restore(&mut self, floats: Vec<FloatInfo>) {
        self.floats = floats;
    }

    /// Floats not entirely above `y`, relative to `origin`.
    pub fn relative_from(&self, origin: (f32, f32), y: f32) -> Vec<RelFloat> {
        self.floats
            .iter()
            .filter(|f| f.rect.bottom() > y + 0.01)
            .map(|f| f.translated(-origin.0, -origin.1))
            .collect()
    }

    /// Floats placed after the first `n`, relative to `origin`.
    pub fn relative_since(&self, n: usize, origin: (f32, f32)) -> Vec<RelFloat> {
        self.floats[n.min(self.floats.len())..]
            .iter()
            .map(|f| f.translated(-origin.0, -origin.1))
            .collect()
    }

    /// Re-add floats recorded with [`Bfc::relative_since`] at `origin`.
    pub fn extend_relative(&mut self, origin: (f32, f32), floats: &[RelFloat]) {
        self.floats.extend(floats.iter().map(|f| f.translated(origin.0, origin.1)));
    }

    /// Number of floats placed so far.
    pub fn len(&self) -> usize {
        self.floats.len()
    }

    pub fn is_empty(&self) -> bool {
        self.floats.is_empty()
    }

    /// Horizontal range free of floats for a band `[y, y+h)` inside the
    /// container `[cx, cx+cw)`.
    pub fn available(&self, y: f32, h: f32, cx: f32, cw: f32) -> (f32, f32) {
        let mut l = cx;
        let mut r = cx + cw;
        for f in &self.floats {
            if overlaps(f.rect, y, h) {
                if f.left {
                    l = l.max(f.rect.right());
                } else {
                    r = r.min(f.rect.x);
                }
            }
        }
        (l, r)
    }

    /// Only inline content wraps around the exclusion shape. Floats and new
    /// block formatting contexts continue to avoid the rectangular margin box.
    pub fn inline_available(&self, y: f32, h: f32, cx: f32, cw: f32) -> (f32, f32) {
        let (mut l, mut r) = (cx, cx + cw);
        for f in &self.floats {
            if overlaps(f.rect, y, h) {
                if let Some(edge) = f.inline_edge(y, h) {
                    if f.left { l = l.max(edge); } else { r = r.min(edge); }
                }
            }
        }
        (l, r)
    }

    pub fn set_shape(&mut self, b: &crate::tree::LayoutBox, containing_width: f32) {
        let Some(shape) = b.style.shape_outside.as_deref() else { return };
        let Some(geometry) = crate::clip_path::geometry(b, shape) else { return };
        let float = self.floats.last_mut().expect("just placed a float");
        let matrix = [1.0, 0.0, 0.0, 1.0, -float.rect.x, -float.rect.y];
        let poly = rabbithole_gfx::path::flatten(&geometry.path, &matrix, 0.1);
        float.outline = Some(poly.contours.into_iter().filter(|c| c.points.len() >= 3)
            .map(|c| c.points).collect::<Vec<_>>().into());
        float.shape_margin = b.style.shape_margin.resolve_or_zero(containing_width).max(0.0);
    }

    /// Does any float intersect the band?
    pub fn intersects(&self, y: f32, h: f32) -> bool {
        self.floats.iter().any(|f| overlaps(f.rect, y, h))
    }

    /// Next y (strictly below `y`) at which the set of active floats changes.
    pub fn next_y(&self, y: f32) -> Option<f32> {
        self.floats
            .iter()
            .map(|f| f.rect.bottom())
            .filter(|b| *b > y + 0.01)
            .fold(None, |acc: Option<f32>, b| Some(acc.map_or(b, |a| a.min(b))))
    }

    /// The margin box of the most recently placed float.
    pub fn last(&self) -> Option<Rect> {
        self.floats.last().map(|f| f.rect)
    }

    /// Forget the most recently placed float (a tentative placement).
    pub fn pop(&mut self) {
        self.floats.pop();
    }

    /// Place a float with margin-box size `w`×`h` no higher than `y0`.
    /// Returns the margin box top-left.
    pub fn place(&mut self, left: bool, w: f32, h: f32, y0: f32, cx: f32, cw: f32) -> (f32, f32) {
        // A float's top may not be above the top of earlier floats.
        let mut y = self
            .floats
            .iter()
            .map(|f| f.rect.y)
            .fold(y0, f32::max);
        let hh = h.max(1.0);
        let mut guard = 0;
        let x = loop {
            let (l, r) = self.available(y, hh, cx, cw);
            if w <= r - l + 0.01 || !self.intersects(y, hh) {
                break if left { l } else { r - w };
            }
            match self.next_y(y) {
                Some(ny) => y = ny,
                None => break if left { l } else { r - w },
            }
            guard += 1;
            if guard > 1000 {
                break if left { l } else { r - w };
            }
        };
        self.floats.push(FloatInfo {
            rect: Rect::new(x, y, w, h),
            outline: None,
            shape_margin: 0.0,
            left,
        });
        (x, y)
    }

    /// Y below which no floats of the cleared side(s) remain.
    pub fn clear_y(&self, clear: Clear) -> f32 {
        let mut y = f32::MIN;
        for f in &self.floats {
            let relevant = match clear {
                Clear::None => false,
                Clear::Left => f.left,
                Clear::Right => !f.left,
                Clear::Both => true,
            };
            if relevant {
                y = y.max(f.rect.bottom());
            }
        }
        y
    }

    /// Bottom of the lowest float.
    pub fn max_bottom(&self) -> f32 {
        self.floats.iter().map(|f| f.rect.bottom()).fold(f32::MIN, f32::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_and_avoidance() {
        let mut bfc = Bfc::new();
        let (x, y) = bfc.place(true, 100.0, 50.0, 0.0, 0.0, 400.0);
        assert_eq!((x, y), (0.0, 0.0));
        let (x, y) = bfc.place(true, 100.0, 50.0, 0.0, 0.0, 400.0);
        assert_eq!((x, y), (100.0, 0.0));
        let (x, y) = bfc.place(false, 100.0, 20.0, 10.0, 0.0, 400.0);
        assert_eq!((x, y), (300.0, 10.0));
        assert_eq!(bfc.available(0.0, 5.0, 0.0, 400.0), (200.0, 400.0));
        assert_eq!(bfc.available(15.0, 10.0, 0.0, 400.0), (200.0, 300.0));
        assert_eq!(bfc.available(60.0, 10.0, 0.0, 400.0), (0.0, 400.0));
        // Too wide to fit beside the two left floats: goes below them.
        let (x, y) = bfc.place(true, 250.0, 10.0, 0.0, 0.0, 400.0);
        assert_eq!((x, y), (0.0, 50.0));
        assert_eq!(bfc.clear_y(Clear::Left), 60.0);
        assert_eq!(bfc.clear_y(Clear::Right), 30.0);
    }
}
