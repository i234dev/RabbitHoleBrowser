//! Tab strip logic without a window: layout of pinned and normal tabs,
//! drag-to-reorder and the stack of recently closed tabs.

use rabbithole_engine::gfx::Rect;

/// Width of a pinned (icon-only) tab, logical px.
pub const PINNED_W: f32 = 40.0;
pub const MAX_TAB_W: f32 = 220.0;
pub const MIN_TAB_W: f32 = 48.0;
/// Pointer travel (logical px) before a press on a tab becomes a drag.
pub const DRAG_START: f32 = 6.0;
/// Closed tabs remembered for Ctrl+Shift+T.
pub const MAX_CLOSED: usize = 25;

/// Tab rects (physical px) inside `bar` for tabs with the given pinned
/// flags (pinned tabs come first), plus the "+" button rect.
pub fn layout(bar: Rect, s: f32, pinned: &[bool]) -> (Vec<Rect>, Rect) {
    let plus_w = 30.0 * s;
    let left = 4.0 * s;
    let n_pinned = pinned.iter().filter(|p| **p).count();
    let n_normal = pinned.len() - n_pinned;
    let pinned_total = n_pinned as f32 * PINNED_W * s;
    let avail = (bar.w - left - plus_w - 12.0 * s - pinned_total).max(MIN_TAB_W * s);
    let w = if n_normal == 0 { MAX_TAB_W * s } else { (avail / n_normal as f32).clamp(MIN_TAB_W * s, MAX_TAB_W * s) };
    let y = bar.y + 4.0 * s;
    let h = (bar.h - 4.0 * s).max(1.0);
    let mut x = left;
    let mut rects = Vec::with_capacity(pinned.len());
    for p in pinned {
        let tw = if *p { PINNED_W * s } else { w };
        rects.push(Rect::new(x, y, tw - 2.0 * s, h));
        x += tw;
    }
    let plus_x = rects.last().map(|r| r.right() + 4.0 * s).unwrap_or(left);
    // Too many tabs: the "+" stays visible at the right edge.
    let plus_x = plus_x.min(bar.right() - plus_w);
    (rects, Rect::new(plus_x, y + 2.0 * s, plus_w - 4.0 * s, h - 4.0 * s))
}

/// The close button of a (normal) tab.
pub fn close_rect(tab: Rect, s: f32) -> Rect {
    Rect::new(tab.right() - 24.0 * s, tab.y + (tab.h - 18.0 * s) / 2.0, 18.0 * s, 18.0 * s)
}

/// Where a tab dragged from `from` with its centre at `center_x` drops.
/// Pinned tabs stay among pinned tabs and normal tabs among normal ones.
pub fn drop_index(rects: &[Rect], pinned: &[bool], from: usize, center_x: f32) -> usize {
    let Some(&group) = pinned.get(from) else { return from };
    let mid = |i: usize| rects[i].x + rects[i].w / 2.0;
    let mut target = from;
    for i in from + 1..rects.len() {
        if pinned[i] != group || center_x <= mid(i) {
            break;
        }
        target = i;
    }
    if target != from {
        return target;
    }
    for i in (0..from).rev() {
        if pinned[i] != group || center_x >= mid(i) {
            break;
        }
        target = i;
    }
    target
}

/// Move an element; returns where an index `active` ends up.
pub fn move_item<T>(v: &mut Vec<T>, from: usize, to: usize, active: usize) -> usize {
    if from >= v.len() || to >= v.len() || from == to {
        return active;
    }
    let item = v.remove(from);
    v.insert(to, item);
    if active == from {
        to
    } else if from < active && to >= active {
        active - 1
    } else if from > active && to <= active {
        active + 1
    } else {
        active
    }
}

/// A press on a tab that may become a drag.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TabDrag {
    pub index: usize,
    /// Pointer x at the press (physical px).
    pub start_x: f32,
    /// Pointer x minus the tab's left edge at the press.
    pub grab_dx: f32,
    /// The pointer moved far enough: the tab follows it.
    pub dragging: bool,
    /// Current pointer x.
    pub x: f32,
}

impl TabDrag {
    pub fn new(index: usize, x: f32, tab: Rect) -> TabDrag {
        TabDrag { index, start_x: x, grab_dx: x - tab.x, dragging: false, x }
    }

    /// Pointer moved; returns true when the drag (re)started or moved.
    pub fn moved(&mut self, x: f32, s: f32) -> bool {
        self.x = x;
        if !self.dragging && (x - self.start_x).abs() >= DRAG_START * s {
            self.dragging = true;
        }
        self.dragging
    }

    /// Where the dragged tab is drawn (its rect shifted with the pointer,
    /// kept inside the strip).
    pub fn ghost(&self, rect: Rect, bar: Rect) -> Rect {
        let x = (self.x - self.grab_dx).clamp(bar.x, (bar.right() - rect.w).max(bar.x));
        Rect::new(x, rect.y, rect.w, rect.h)
    }
}

/// A closed tab, for Ctrl+Shift+T.
#[derive(Debug, Clone, PartialEq)]
pub struct ClosedTab {
    pub history: Vec<String>,
    pub index: usize,
    pub pinned: bool,
    pub title: String,
    /// Its position in the strip when it was closed.
    pub position: usize,
}

#[derive(Debug, Default)]
pub struct ClosedTabs {
    stack: Vec<ClosedTab>,
}

impl ClosedTabs {
    pub fn push(&mut self, t: ClosedTab) {
        if t.history.is_empty() {
            return;
        }
        self.stack.push(t);
        if self.stack.len() > MAX_CLOSED {
            self.stack.remove(0);
        }
    }

    pub fn pop(&mut self) -> Option<ClosedTab> {
        self.stack.pop()
    }

    pub fn len(&self) -> usize {
        self.stack.len()
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_pinned_first_and_narrow() {
        let bar = Rect::new(0.0, 0.0, 1000.0, 34.0);
        let (r, plus) = layout(bar, 1.0, &[true, true, false, false]);
        assert_eq!(r.len(), 4);
        assert_eq!(r[0].w, PINNED_W - 2.0);
        assert_eq!(r[1].x, 4.0 + PINNED_W);
        assert_eq!(r[2].w, MAX_TAB_W - 2.0);
        assert!(plus.x > r[3].right());
        // Many tabs shrink down to the minimum width; "+" stays visible.
        let (r, plus) = layout(bar, 1.0, &[false; 40]);
        assert_eq!(r[0].w, MIN_TAB_W - 2.0);
        assert!(plus.right() <= bar.right());
        // HiDPI scales everything.
        let (r2, _) = layout(Rect::new(0.0, 0.0, 2000.0, 68.0), 2.0, &[true, false]);
        assert_eq!(r2[0].w, (PINNED_W - 2.0) * 2.0);
    }

    #[test]
    fn dragging_reorders_within_group() {
        let bar = Rect::new(0.0, 0.0, 1000.0, 34.0);
        let pinned = [true, false, false, false];
        let (r, _) = layout(bar, 1.0, &pinned);
        // Drag tab 1 right past the middle of tab 2.
        let c = r[2].x + r[2].w * 0.75;
        assert_eq!(drop_index(&r, &pinned, 1, c), 2);
        // ... and past tab 3.
        assert_eq!(drop_index(&r, &pinned, 1, r[3].right()), 3);
        // Not far enough: stays.
        assert_eq!(drop_index(&r, &pinned, 1, r[1].x + r[1].w / 2.0 + 10.0), 1);
        // Normal tabs cannot move before pinned ones.
        assert_eq!(drop_index(&r, &pinned, 3, 0.0), 1);
        // Moving left.
        assert_eq!(drop_index(&r, &pinned, 3, r[2].x + 5.0), 2);
        // A pinned tab stays pinned.
        assert_eq!(drop_index(&r, &pinned, 0, 900.0), 0);
    }

    #[test]
    fn move_item_tracks_active() {
        let mut v = vec!['a', 'b', 'c', 'd'];
        assert_eq!(move_item(&mut v, 0, 2, 0), 2);
        assert_eq!(v, vec!['b', 'c', 'a', 'd']);
        assert_eq!(move_item(&mut v, 0, 3, 2), 1);
        assert_eq!(move_item(&mut v, 3, 0, 1), 2);
        assert_eq!(move_item(&mut v, 1, 1, 1), 1);
    }

    #[test]
    fn drag_threshold_and_ghost() {
        let bar = Rect::new(0.0, 0.0, 1000.0, 34.0);
        let tab = Rect::new(100.0, 4.0, 200.0, 30.0);
        let mut d = TabDrag::new(1, 150.0, tab);
        assert!(!d.moved(153.0, 1.0));
        assert!(d.moved(160.0, 1.0));
        assert_eq!(d.ghost(tab, bar).x, 110.0);
        d.moved(-500.0, 1.0);
        assert_eq!(d.ghost(tab, bar).x, 0.0);
    }

    #[test]
    fn closed_stack() {
        let mut c = ClosedTabs::default();
        for i in 0..30 {
            c.push(ClosedTab { history: vec![format!("https://{i}/")], index: 0, pinned: false, title: String::new(), position: 0 });
        }
        c.push(ClosedTab { history: vec![], index: 0, pinned: false, title: String::new(), position: 0 });
        assert_eq!(c.len(), MAX_CLOSED);
        assert_eq!(c.pop().unwrap().history[0], "https://29/");
    }
}
