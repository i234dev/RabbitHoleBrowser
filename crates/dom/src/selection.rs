//! The document's selection (Selection API §2): one range whose boundary
//! points are live points (see [`crate::live`]), plus a direction.
//!
//! The range's slots belong either to the document (a selection made by the
//! user agent, e.g. a mouse drag) or to a script `Range` object that was
//! added to the selection; scripts adopt document-owned slots when they
//! first look at a selection the user agent made. `generation` changes
//! whenever the selection gets another range (not when its range's
//! boundary points move), so a script binding can tell whether its cached
//! `Range` object is still the selection's range.

use crate::{BoundaryPoint, Document, NodeData, NodeId};
use std::cmp::Ordering;

/// The selection's range (live point slots) and direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocSelection {
    pub start: usize,
    pub end: usize,
    /// The anchor is the end (`direction` "backward").
    pub backward: bool,
    /// The slots were allocated by the document (no script handle owns them).
    pub owned: bool,
}

/// The resolved selection: boundary points in tree order plus direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionRange {
    pub start: BoundaryPoint,
    pub end: BoundaryPoint,
    pub backward: bool,
}

impl SelectionRange {
    pub fn is_collapsed(&self) -> bool {
        self.start == self.end
    }

    pub fn anchor(&self) -> BoundaryPoint {
        if self.backward {
            self.end
        } else {
            self.start
        }
    }

    pub fn focus(&self) -> BoundaryPoint {
        if self.backward {
            self.start
        } else {
            self.end
        }
    }
}

/// A range of a registered custom highlight (CSS Custom Highlight API):
/// a live `Range`'s boundary point slots, or a `StaticRange`'s points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightRange {
    Live(usize, usize),
    Static(BoundaryPoint, BoundaryPoint),
}

/// A highlight registered in `CSS.highlights`.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomHighlight {
    pub name: String,
    pub priority: i32,
    pub ranges: Vec<HighlightRange>,
}

impl Document {
    /// Outermost editable element within the nearest editing boundary.
    pub fn editing_host(&self, node: NodeId) -> Option<NodeId> {
        let mut host = None;
        let mut current = Some(node);
        while let Some(n) = current {
            if n == Document::ROOT {
                return if self.design_mode { self.html_element().or(host) } else { host };
            }
            if let Some(value) = self.attr(n, "contenteditable") {
                if value.eq_ignore_ascii_case("false") { return host; }
                if value.is_empty() || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("plaintext-only") {
                    host = Some(n);
                }
            }
            current = self.parent(n);
        }
        host
    }

    /// Focusing an editing host establishes its caret once. An explicit
    /// removeAllRanges() afterwards leaves the editor without a caret.
    pub fn focus_editing_host(&mut self, node: Option<NodeId>) {
        let Some(host) = node.filter(|&n| self.editing_host(n) == Some(n)) else { return };
        if self.selection().is_some_and(|s| self.editing_host(s.start.node) == Some(host) && self.editing_host(s.end.node) == Some(host)) { return; }
        let point = BoundaryPoint { node: host, offset: 0 };
        self.set_selection(point, point);
    }

    /// The selection's range, if the selection is not empty.
    pub fn selection(&self) -> Option<SelectionRange> {
        let s = self.selection?;
        let start = self.live_point(s.start)?;
        let end = self.live_point(s.end)?;
        Some(SelectionRange { start, end, backward: s.backward })
    }

    /// The raw selection state (slots), for script bindings.
    pub fn selection_slots(&self) -> Option<DocSelection> {
        self.selection
    }

    /// Changes whenever the selection gets another range or is emptied.
    pub fn selection_generation(&self) -> u64 {
        self.selection_generation
    }

    /// Select from `anchor` to `focus` (in either order) with a new range
    /// owned by the document (user selection).
    pub fn set_selection(&mut self, anchor: BoundaryPoint, focus: BoundaryPoint) {
        let backward = self.compare_points(focus, anchor) == Ordering::Less;
        let (start, end) = if backward { (focus, anchor) } else { (anchor, focus) };
        if let Some(s) = self.selection.filter(|s| s.owned) {
            // Reuse the slots, but it is a new range all the same.
            self.set_live_point(s.start, start);
            self.set_live_point(s.end, end);
            self.selection = Some(DocSelection { backward, ..s });
        } else {
            let start = self.add_live_point(start);
            let end = self.add_live_point(end);
            self.selection = Some(DocSelection { start, end, backward, owned: true });
        }
        self.selection_generation += 1;
    }

    /// Move the focus of the current selection, keeping its anchor (a drag
    /// or shift-click). Starts a collapsed selection when there is none.
    pub fn extend_selection(&mut self, focus: BoundaryPoint) {
        let anchor = self.selection().map(|s| s.anchor()).unwrap_or(focus);
        self.set_selection(anchor, focus);
    }

    /// Make the selection empty.
    pub fn clear_selection(&mut self) {
        if let Some(s) = self.selection.take() {
            if s.owned {
                self.remove_live_point(s.start);
                self.remove_live_point(s.end);
            }
            self.selection_generation += 1;
        }
    }

    /// Use the live point slots of a script `Range` as the selection's
    /// range (script owned). Returns the new generation.
    pub fn set_selection_slots(&mut self, start: usize, end: usize, backward: bool) -> u64 {
        if let Some(s) = self.selection.take() {
            if s.owned && s.start != start && s.start != end {
                self.remove_live_point(s.start);
            }
            if s.owned && s.end != start && s.end != end {
                self.remove_live_point(s.end);
            }
        }
        self.selection = Some(DocSelection { start, end, backward, owned: false });
        self.selection_generation += 1;
        self.selection_generation
    }

    /// Change the selection's direction without changing its range.
    pub fn set_selection_backward(&mut self, backward: bool) {
        if let Some(s) = self.selection.as_mut() {
            s.backward = backward;
        }
    }

    /// A script handle now owns the document's selection slots.
    pub fn disown_selection_slots(&mut self) {
        if let Some(s) = self.selection.as_mut() {
            s.owned = false;
        }
    }

    /// A live point slot is being released: a selection using it ends.
    pub(crate) fn selection_slot_released(&mut self, slot: usize) {
        if self.selection.is_some_and(|s| !s.owned && (s.start == slot || s.end == slot)) {
            self.selection = None;
            self.selection_generation += 1;
        }
    }

    /// Replace the registered custom highlights (in registration order).
    pub fn set_custom_highlights(&mut self, highlights: Vec<CustomHighlight>) {
        self.custom_highlights = highlights;
    }

    pub fn custom_highlights(&self) -> &[CustomHighlight] {
        &self.custom_highlights
    }

    /// The custom highlights' current ranges (valid, non-collapsed, start
    /// before end), in registration order: (name, priority, ranges).
    pub fn custom_highlight_ranges(&self) -> Vec<(&str, i32, Vec<(BoundaryPoint, BoundaryPoint)>)> {
        let valid = |p: BoundaryPoint| p.node < self.nodes.len() && p.offset <= self.node_length(p.node);
        self.custom_highlights
            .iter()
            .map(|h| {
                let ranges = h
                    .ranges
                    .iter()
                    .filter_map(|r| match *r {
                        HighlightRange::Live(a, b) => Some((self.live_point(a)?, self.live_point(b)?)),
                        HighlightRange::Static(a, b) => Some((a, b)),
                    })
                    .filter(|&(a, b)| valid(a) && valid(b) && self.tree_root(a.node) == self.tree_root(b.node) && self.compare_points(a, b) == Ordering::Less)
                    .collect();
                (h.name.as_str(), h.priority, ranges)
            })
            .collect()
    }

    /// Changes whenever what the custom highlights cover may have changed
    /// (0 without highlights).
    pub fn custom_highlights_key(&self) -> u64 {
        if self.custom_highlights.is_empty() {
            return 0;
        }
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for (name, priority, ranges) in self.custom_highlight_ranges() {
            (name, priority).hash(&mut h);
            for (a, b) in ranges {
                (a.node, a.offset, b.node, b.offset).hash(&mut h);
            }
        }
        h.finish() | 1
    }

    /// Length of a node for boundary points: UTF-16 length of character
    /// data, else the number of children.
    pub fn node_length(&self, id: NodeId) -> usize {
        match &self.nodes[id].data {
            NodeData::Text(t) | NodeData::Comment(t) => t.encode_utf16().count(),
            NodeData::Doctype(_) => 0,
            _ => self.children(id).count(),
        }
    }

    /// Ancestors of `id` from the root down to `id` itself.
    fn ancestor_path(&self, id: NodeId) -> Vec<NodeId> {
        let mut path = vec![id];
        let mut cur = id;
        while let Some(p) = self.nodes[cur].parent {
            path.push(p);
            cur = p;
        }
        path.reverse();
        path
    }

    /// Tree-order comparison of two nodes (`Equal` only for the same node).
    pub fn compare_tree_order(&self, a: NodeId, b: NodeId) -> Ordering {
        if a == b {
            return Ordering::Equal;
        }
        let pa = self.ancestor_path(a);
        let pb = self.ancestor_path(b);
        let mut i = 0;
        while i < pa.len() && i < pb.len() && pa[i] == pb[i] {
            i += 1;
        }
        if i == pa.len() {
            return Ordering::Less; // a is an ancestor of b
        }
        if i == pb.len() {
            return Ordering::Greater;
        }
        if i == 0 {
            // Different trees: order arbitrarily but consistently.
            return pa[0].cmp(&pb[0]);
        }
        self.index_of(pa[i]).cmp(&self.index_of(pb[i]))
    }

    /// DOM "position of a boundary point relative to another".
    pub fn compare_points(&self, a: BoundaryPoint, b: BoundaryPoint) -> Ordering {
        if a.node == b.node {
            return a.offset.cmp(&b.offset);
        }
        let pa = self.ancestor_path(a.node);
        let pb = self.ancestor_path(b.node);
        let mut i = 0;
        while i < pa.len() && i < pb.len() && pa[i] == pb[i] {
            i += 1;
        }
        if i == pa.len() {
            // a.node is an ancestor of b.node: compare a.offset with the
            // index of b's ancestor child of a.node.
            let child = self.index_of(pb[i]);
            return if child < a.offset { Ordering::Greater } else { Ordering::Less };
        }
        if i == pb.len() {
            let child = self.index_of(pa[i]);
            return if child < b.offset { Ordering::Less } else { Ordering::Greater };
        }
        if i == 0 {
            return pa[0].cmp(&pb[0]);
        }
        self.index_of(pa[i]).cmp(&self.index_of(pb[i]))
    }

    /// Text nodes intersecting the range from `start` to `end`, each with
    /// the selected UTF-16 range of its data, in tree order.
    pub fn text_in_range(&self, start: BoundaryPoint, end: BoundaryPoint) -> Vec<(NodeId, usize, usize)> {
        let mut out = Vec::new();
        if start == end {
            return out;
        }
        // The first node at or after the start point.
        let first = if matches!(self.nodes[start.node].data, NodeData::Text(_)) {
            Some(start.node)
        } else {
            match self.children(start.node).nth(start.offset) {
                Some(c) => Some(c),
                None => self.following_skipping_children(start.node),
            }
        };
        let mut cur = first;
        while let Some(n) = cur {
            if self.compare_points(BoundaryPoint { node: n, offset: 0 }, end) != Ordering::Less {
                break;
            }
            if let NodeData::Text(t) = &self.nodes[n].data {
                let len = t.encode_utf16().count();
                let s = if n == start.node { start.offset.min(len) } else { 0 };
                let e = if n == end.node { end.offset.min(len) } else { len };
                if s < e {
                    out.push((n, s, e));
                }
            }
            cur = self.following(n);
        }
        out
    }

    /// The next node in tree order.
    fn following(&self, n: NodeId) -> Option<NodeId> {
        if let Some(c) = self.nodes[n].first_child {
            return Some(c);
        }
        self.following_skipping_children(n)
    }

    /// The next node in tree order that is not a descendant of `n`.
    fn following_skipping_children(&self, mut n: NodeId) -> Option<NodeId> {
        loop {
            if let Some(s) = self.nodes[n].next_sibling {
                return Some(s);
            }
            n = self.nodes[n].parent?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> (Document, NodeId, NodeId, NodeId) {
        let mut d = Document::new();
        let body = d.create_element("body", vec![]);
        d.append_child(Document::ROOT, body);
        let p = d.create_element("p", vec![]);
        d.append_child(body, p);
        let t1 = d.create_text("Hello ");
        d.append_child(p, t1);
        let b = d.create_element("b", vec![]);
        d.append_child(p, b);
        let t2 = d.create_text("world");
        d.append_child(b, t2);
        (d, body, t1, t2)
    }

    #[test]
    fn set_and_order() {
        let (mut d, _, t1, t2) = doc();
        d.set_selection(BoundaryPoint { node: t2, offset: 3 }, BoundaryPoint { node: t1, offset: 2 });
        let s = d.selection().unwrap();
        assert!(s.backward);
        assert_eq!(s.start, BoundaryPoint { node: t1, offset: 2 });
        assert_eq!(s.anchor(), BoundaryPoint { node: t2, offset: 3 });
        assert_eq!(d.text_in_range(s.start, s.end), vec![(t1, 2, 6), (t2, 0, 3)]);
        let g = d.selection_generation();
        d.clear_selection();
        assert!(d.selection().is_none());
        assert!(d.live_points.is_empty());
        assert!(d.selection_generation() > g);
    }

    #[test]
    fn live_under_mutation() {
        let (mut d, body, t1, t2) = doc();
        d.set_selection(BoundaryPoint { node: t1, offset: 1 }, BoundaryPoint { node: t2, offset: 2 });
        // Removing the <b> moves the end to (p, 1).
        let b = d.parent(t2).unwrap();
        d.detach(b);
        let s = d.selection().unwrap();
        let p = d.parent(t1).unwrap();
        assert_eq!(s.end, BoundaryPoint { node: p, offset: 1 });
        assert_eq!(d.text_in_range(s.start, s.end), vec![(t1, 1, 6)]);
        let _ = body;
    }

    #[test]
    fn element_boundaries() {
        let (mut d, body, t1, t2) = doc();
        d.set_selection(BoundaryPoint { node: body, offset: 0 }, BoundaryPoint { node: body, offset: 1 });
        let s = d.selection().unwrap();
        assert_eq!(d.text_in_range(s.start, s.end), vec![(t1, 0, 6), (t2, 0, 5)]);
        assert_eq!(d.compare_points(BoundaryPoint { node: body, offset: 1 }, BoundaryPoint { node: t2, offset: 5 }), Ordering::Greater);
    }

    #[test]
    fn script_slots_released_end_the_selection() {
        let (mut d, _, t1, _) = doc();
        let a = d.add_live_point(BoundaryPoint { node: t1, offset: 0 });
        let b = d.add_live_point(BoundaryPoint { node: t1, offset: 3 });
        d.set_selection_slots(a, b, false);
        assert_eq!(d.selection().unwrap().end.offset, 3);
        d.remove_live_point(a);
        assert!(d.selection().is_none());
    }
}
