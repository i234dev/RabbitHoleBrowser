//! CSS Scroll Anchoring (https://drafts.csswg.org/css-scroll-anchoring/)
//! for the viewport: before a layout the engine picks an anchor box in
//! the visible area of the old layout; afterwards it scrolls by the
//! distance the anchor moved, so content inserted or resized above the
//! visible area (images arriving, lazy components, late styles) does not
//! push what the reader is looking at out of place.

use crate::tree::{Flow, LayoutBox, LayoutTree};
use rabbithole_dom::{Document, NodeId};
use rabbithole_gfx::Rect;
use rabbithole_style::{ComputedStyle, Position};
use std::rc::Rc;

/// The anchor chosen in a layout, with the styles of its box and of the
/// boxes above it (for the suppression triggers).
#[derive(Debug, Clone)]
pub struct ScrollAnchor {
    pub node: NodeId,
    /// Border box in document coordinates.
    pub rect: Rect,
    chain: Vec<(NodeId, Rc<ComputedStyle>)>,
}

impl LayoutTree {
    /// The anchor node for the viewport scrolled to `scroll` (§2.2): the
    /// first box in tree order that is fully visible, descending into
    /// partially visible boxes (one without a visible candidate inside is
    /// the anchor itself). Boxes with `overflow-anchor: none` and their
    /// contents, out-of-flow and sticky boxes, and boxes in vertical
    /// writing modes are not candidates; inner scroll containers are not
    /// descended into (their contents move on their own).
    pub fn scroll_anchor(&self, scroll: (f32, f32)) -> Option<ScrollAnchor> {
        let root = &self.root;
        if root.style.overflow_anchor_none || root.wm_transform.is_some() {
            return None;
        }
        let view = Rect::new(scroll.0, scroll.1, self.viewport.0, self.viewport.1);
        let mut chain = Vec::new();
        if let Some(n) = root.node {
            chain.push((n, root.style.clone()));
        }
        let (node, rect) = find(root, view, &mut chain)?;
        Some(ScrollAnchor { node, rect, chain })
    }

    /// How far `anchor` moved in this (newer) layout, unless a suppression
    /// trigger (§2.4) applies: the anchor or a box above it changed its
    /// `position`, insets or transform (scripts moving things themselves),
    /// or it lost its box.
    pub fn scroll_anchor_shift(&self, doc: &Document, anchor: &ScrollAnchor) -> Option<f32> {
        let b = self.box_of(doc, anchor.node)?;
        if !doc.is_connected(anchor.node) || b.flow == Flow::Absolute || b.wm_transform.is_some() {
            return None;
        }
        for (n, old) in &anchor.chain {
            let new = if *n == anchor.node { &b.style } else { &self.box_of(doc, *n)?.style };
            if !Rc::ptr_eq(old, new) && (old.position != new.position || old.inset != new.inset || old.transform != new.transform) {
                return None;
            }
        }
        let dy = b.rect.y - anchor.rect.y;
        (dy.abs() >= 0.5 && dy.is_finite()).then_some(dy)
    }
}

/// Is `b` (and so its contents) excluded from anchor selection?
fn excluded(b: &LayoutBox) -> bool {
    b.style.overflow_anchor_none || b.flow == Flow::Absolute || b.style.position == Position::Sticky || b.wm_transform.is_some()
}

fn find(parent: &LayoutBox, view: Rect, chain: &mut Vec<(NodeId, Rc<ComputedStyle>)>) -> Option<(NodeId, Rect)> {
    for c in &parent.children {
        if excluded(c) {
            continue;
        }
        let r = c.rect;
        let Some(node) = c.node else {
            // Anonymous boxes (line wrappers, table parts) are transparent.
            if r.w > 0.0 && r.h > 0.0 && overlaps(r, view) {
                if let Some(found) = find(c, view, chain) {
                    return Some(found);
                }
            }
            continue;
        };
        if r.w <= 0.0 || r.h <= 0.0 || !overlaps(r, view) {
            continue;
        }
        chain.push((node, c.style.clone()));
        if r.y >= view.y && r.bottom() <= view.bottom() {
            return Some((node, r));
        }
        if !c.scroll_container {
            if let Some(found) = find(c, view, chain) {
                return Some(found);
            }
        }
        return Some((node, r));
    }
    None
}

fn overlaps(r: Rect, view: Rect) -> bool {
    r.bottom() > view.y && r.y < view.bottom() && r.right() > view.x && r.x < view.right()
}
