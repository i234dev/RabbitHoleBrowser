//! Float bookkeeping for a block formatting context.

use rabbithole_gfx::Rect;
use rabbithole_style::Clear;

#[derive(Debug, Clone, Copy)]
struct FloatInfo {
    /// Margin box.
    rect: Rect,
    left: bool,
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

/// A float relative to some origin: (x, y, w, h, left).
pub(crate) type RelFloat = (f32, f32, f32, f32, bool);

/// Same floats within rounding noise?
pub(crate) fn same_floats(a: &[RelFloat], b: &[RelFloat]) -> bool {
    const EPS: f32 = 0.01;
    a.len() == b.len()
        && a.iter().zip(b).all(|(p, q)| {
            p.4 == q.4 && (p.0 - q.0).abs() < EPS && (p.1 - q.1).abs() < EPS && (p.2 - q.2).abs() < EPS && (p.3 - q.3).abs() < EPS
        })
}

impl Bfc {
    pub fn new() -> Self {
        Bfc::default()
    }

    /// Floats not entirely above `y`, relative to `origin`.
    pub fn relative_from(&self, origin: (f32, f32), y: f32) -> Vec<RelFloat> {
        self.floats
            .iter()
            .filter(|f| f.rect.bottom() > y + 0.01)
            .map(|f| (f.rect.x - origin.0, f.rect.y - origin.1, f.rect.w, f.rect.h, f.left))
            .collect()
    }

    /// Floats placed after the first `n`, relative to `origin`.
    pub fn relative_since(&self, n: usize, origin: (f32, f32)) -> Vec<RelFloat> {
        self.floats[n.min(self.floats.len())..]
            .iter()
            .map(|f| (f.rect.x - origin.0, f.rect.y - origin.1, f.rect.w, f.rect.h, f.left))
            .collect()
    }

    /// Re-add floats recorded with [`Bfc::relative_since`] at `origin`.
    pub fn extend_relative(&mut self, origin: (f32, f32), floats: &[RelFloat]) {
        for &(x, y, w, h, left) in floats {
            self.floats.push(FloatInfo { rect: Rect::new(x + origin.0, y + origin.1, w, h), left });
        }
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
