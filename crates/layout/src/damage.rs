//! Damage between two display lists: the device-space rectangles that
//! must be repainted to turn a frame of `old` into a frame of `new`.
//!
//! The lists are compared item by item; the common prefix and suffix are
//! skipped and the bounds of the differing items (in both lists) are
//! mapped through the translations / fixed layers active at their
//! position. Anything that cannot be bounded cheaply (non-translation
//! transforms around a change) asks for a full repaint.

use crate::display_list::{DisplayItem, DisplayList};
use rabbithole_gfx::Rect;
use std::sync::Arc;

/// Are two items identical for painting purposes?
fn item_eq(a: &DisplayItem, b: &DisplayItem) -> bool {
    use DisplayItem as D;
    match (a, b) {
        (D::Rect { rect: r1, color: c1, radius: q1 }, D::Rect { rect: r2, color: c2, radius: q2 }) => r1 == r2 && c1 == c2 && q1 == q2,
        (D::RectRadii { rect: r1, radii: q1, paint: p1 }, D::RectRadii { rect: r2, radii: q2, paint: p2 }) => r1 == r2 && q1 == q2 && p1 == p2,
        (D::Border { rect: r1, widths: w1, colors: c1, styles: s1, radii: q1 }, D::Border { rect: r2, widths: w2, colors: c2, styles: s2, radii: q2 }) => {
            r1 == r2 && w1 == w2 && c1 == c2 && s1 == s2 && q1 == q2
        }
        (
            D::BoxShadow { rect: r1, radii: q1, offset: o1, blur: b1, spread: s1, color: c1, inset: i1 },
            D::BoxShadow { rect: r2, radii: q2, offset: o2, blur: b2, spread: s2, color: c2, inset: i2 },
        ) => r1 == r2 && q1 == q2 && o1 == o2 && b1 == b2 && s1 == s2 && c1 == c2 && i1 == i2,
        (
            D::Text { x: x1, y: y1, font: f1, size: z1, color: c1, text: t1, letter_spacing: l1, shadows: h1, small_caps: s1, oblique: o1 },
            D::Text { x: x2, y: y2, font: f2, size: z2, color: c2, text: t2, letter_spacing: l2, shadows: h2, small_caps: s2, oblique: o2 },
        ) => x1 == x2 && y1 == y2 && f1 == f2 && z1 == z2 && c1 == c2 && t1 == t2 && l1 == l2 && h1 == h2 && s1 == s2 && o1 == o2,
        (D::Image { rect: r1, src: s1, image: i1, opacity: o1, pixelated: p1 }, D::Image { rect: r2, src: s2, image: i2, opacity: o2, pixelated: p2 }) => {
            r1 == r2 && s1 == s2 && Arc::ptr_eq(i1, i2) && o1 == o2 && p1 == p2
        }
        (
            D::ImageTiled { image: i1, area: a1, origin: g1, tile: t1, repeat_x: x1, repeat_y: y1, opacity: o1 },
            D::ImageTiled { image: i2, area: a2, origin: g2, tile: t2, repeat_x: x2, repeat_y: y2, opacity: o2 },
        ) => Arc::ptr_eq(i1, i2) && a1 == a2 && g1 == g2 && t1 == t2 && x1 == x2 && y1 == y2 && o1 == o2,
        (D::Line { x0: a0, y0: b0, x1: a1, y1: b1, width: w1, color: c1, style: s1 }, D::Line { x0: a2, y0: b2, x1: a3, y1: b3, width: w2, color: c2, style: s2 }) => {
            a0 == a2 && b0 == b2 && a1 == a3 && b1 == b3 && w1 == w2 && c1 == c2 && s1 == s2
        }
        (D::PushClip(a), D::PushClip(b)) => a == b,
        (D::PushClipRounded(a, q1), D::PushClipRounded(b, q2)) => a == b && q1 == q2,
        (D::PopClip, D::PopClip) | (D::PopTransform, D::PopTransform) | (D::PushFixed, D::PushFixed) | (D::PopFixed, D::PopFixed) => true,
        (D::PushTransform(a), D::PushTransform(b)) => a == b,
        (D::PushLayer(a), D::PushLayer(b)) => a == b,
        (D::PopLayer(a), D::PopLayer(b)) => a == b,
        _ => false,
    }
}

/// Painted bounds of a drawing item in its local coordinates.
fn item_bounds(item: &DisplayItem) -> Option<Rect> {
    use DisplayItem as D;
    Some(match item {
        D::Rect { rect, .. } | D::RectRadii { rect, .. } | D::Border { rect, .. } | D::Image { rect, .. } => *rect,
        D::BoxShadow { rect, offset, blur, spread, .. } => {
            let e = blur.abs() * 2.0 + spread.abs() + offset.0.abs().max(offset.1.abs());
            Rect::new(rect.x - e, rect.y - e, rect.w + 2.0 * e, rect.h + 2.0 * e)
        }
        D::Text { x, y, size, text, letter_spacing, shadows, .. } => {
            // Generous: no glyph is wider than 1.5em plus spacing.
            let n = text.chars().count() as f32;
            let w = n * (size * 1.5 + letter_spacing.abs());
            let mut r = Rect::new(*x - size * 0.5, *y - size * 1.5, w + size, size * 2.3);
            for s in shadows {
                let e = s.blur.abs() * 2.0;
                let sr = r.translate(s.x, s.y);
                r = r.union(&Rect::new(sr.x - e, sr.y - e, sr.w + 2.0 * e, sr.h + 2.0 * e));
            }
            r
        }
        D::ImageTiled { area, .. } => *area,
        D::Line { x0, y0, x1, y1, width, .. } => {
            let (l, t) = (x0.min(*x1), y0.min(*y1));
            Rect::new(l - width, t - width, (x0 - x1).abs() + 2.0 * width, (y0 - y1).abs() + 2.0 * width)
        }
        D::PushLayer(r) | D::PushClip(r) | D::PushClipRounded(r, _) => *r,
        _ => return None,
    })
}

/// Paint-state at a position in a list.
#[derive(Clone, Default)]
struct State {
    /// Accumulated translation per pushed transform (None = not a pure
    /// translation).
    transforms: Vec<Option<(f32, f32)>>,
    fixed: bool,
    layers: Vec<Rect>,
}

impl State {
    fn step(&mut self, item: &DisplayItem) {
        use DisplayItem as D;
        match item {
            D::PushTransform(m) => {
                let pure = (m[0] - 1.0).abs() < 1e-6 && m[1].abs() < 1e-6 && m[2].abs() < 1e-6 && (m[3] - 1.0).abs() < 1e-6;
                self.transforms.push(pure.then_some((m[4], m[5])));
            }
            D::PopTransform => {
                self.transforms.pop();
            }
            D::PushFixed => self.fixed = true,
            D::PopFixed => self.fixed = false,
            D::PushLayer(r) => self.layers.push(*r),
            D::PopLayer(_) => {
                self.layers.pop();
            }
            _ => {}
        }
    }

    /// Map local bounds to device space; None when a non-translation
    /// transform is active.
    fn to_device(&self, r: Rect, scroll: (f32, f32)) -> Option<Rect> {
        let (mut dx, mut dy) = (0.0, 0.0);
        for t in &self.transforms {
            let (x, y) = (*t)?;
            dx += x;
            dy += y;
        }
        if !self.fixed {
            dx -= scroll.0;
            dy -= scroll.1;
        }
        Some(r.translate(dx, dy))
    }
}

/// Bounds of `items[from..to]` given the state before `from`.
fn range_damage(items: &[DisplayItem], from: usize, to: usize, scroll: (f32, f32), out: &mut Vec<Rect>) -> bool {
    let mut st = State::default();
    for it in &items[..from] {
        st.step(it);
    }
    // A changed item inside a layer changes the composite of the layer.
    for l in &st.layers {
        match st.to_device(*l, scroll) {
            Some(r) => out.push(r),
            None => return false,
        }
    }
    // A transform / clip / layer / fixed scope opened or closed inside the
    // range whose partner lies outside changes unchanged items too.
    let mut depth = 0i32;
    for it in &items[from..to] {
        use DisplayItem as D;
        match it {
            D::PushTransform(_) | D::PushClip(_) | D::PushClipRounded(..) | D::PushLayer(_) | D::PushFixed => depth += 1,
            D::PopTransform | D::PopClip | D::PopLayer(_) | D::PopFixed => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return false;
    }
    for it in &items[from..to] {
        if let DisplayItem::PopLayer(_) = it {
            if let Some(l) = st.layers.last() {
                match st.to_device(*l, scroll) {
                    Some(r) => out.push(r),
                    None => return false,
                }
            }
        }
        if let DisplayItem::PushTransform(m) = it {
            let pure = (m[0] - 1.0).abs() < 1e-6 && m[1].abs() < 1e-6 && m[2].abs() < 1e-6 && (m[3] - 1.0).abs() < 1e-6;
            if !pure {
                return false;
            }
        }
        st.step(it);
        if let Some(b) = item_bounds(it) {
            match st.to_device(b, scroll) {
                Some(r) => out.push(r),
                None => return false,
            }
        }
    }
    true
}

/// Device-space rectangles to repaint to go from a frame of `old` to one of
/// `new` at the same scroll position, or `None` for "repaint everything".
pub fn damage(old: &DisplayList, new: &DisplayList, scroll: (f32, f32)) -> Option<Vec<Rect>> {
    let (a, b) = (&old.items, &new.items);
    let n = a.len().min(b.len());
    let mut pre = 0;
    while pre < n && item_eq(&a[pre], &b[pre]) {
        pre += 1;
    }
    if pre == a.len() && pre == b.len() {
        return Some(Vec::new());
    }
    let mut suf = 0;
    while suf < n - pre && item_eq(&a[a.len() - 1 - suf], &b[b.len() - 1 - suf]) {
        suf += 1;
    }
    let mut out = Vec::new();
    if !range_damage(a, pre, a.len() - suf, scroll, &mut out) || !range_damage(b, pre, b.len() - suf, scroll, &mut out) {
        return None;
    }
    // Anti-aliasing margin, then merge into a few rectangles.
    let rects: Vec<Rect> = out.into_iter().map(|r| Rect::new(r.x - 2.0, r.y - 2.0, r.w + 4.0, r.h + 4.0)).collect();
    Some(merge_rects(rects))
}

/// Merge damage rectangles into a few (every pass over the display list
/// costs a full item walk, so fewer, slightly larger regions win).
pub fn merge_rects(rects: Vec<Rect>) -> Vec<Rect> {
    let mut rects: Vec<Rect> = rects.into_iter().filter(|r| r.w > 0.0 && r.h > 0.0).collect();
    if rects.is_empty() {
        return rects;
    }
    rects.sort_by(|p, q| p.y.partial_cmp(&q.y).unwrap_or(std::cmp::Ordering::Equal));
    let mut merged: Vec<Rect> = Vec::new();
    for r in rects {
        match merged.last_mut() {
            Some(m) if !m.intersect(&r).is_empty() || (r.y <= m.bottom() + 8.0 && (m.union(&r).w * m.union(&r).h) < 1.5 * (m.w * m.h + r.w * r.h)) => {
                *m = m.union(&r);
            }
            _ => merged.push(r),
        }
    }
    let all = merged.iter().skip(1).fold(merged[0], |acc, r| acc.union(r));
    let sum: f32 = merged.iter().map(|r| r.w * r.h).sum();
    if merged.len() > 4 && all.w * all.h < 2.0 * sum || merged.len() > 16 {
        merged = vec![all];
    }
    merged
}
