//! Live ranges and NodeIterator positions (DOM §5.5, §6.1): boundary points
//! and iterator reference nodes that tree and character-data mutations
//! keep up to date.

use crate::{Document, Mutation, NodeData, NodeId};

/// A live range boundary point. `offset` is a child index, or a UTF-16
/// code unit offset for character data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundaryPoint {
    pub node: NodeId,
    pub offset: usize,
}

/// A NodeIterator's reference node and "pointer before reference" flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IteratorPosition {
    pub root: NodeId,
    pub node: NodeId,
    pub before: bool,
}

fn alloc<T>(slots: &mut Vec<Option<T>>, v: T) -> usize {
    match slots.iter().position(|s| s.is_none()) {
        Some(i) => {
            slots[i] = Some(v);
            i
        }
        None => {
            slots.push(Some(v));
            slots.len() - 1
        }
    }
}

fn free<T>(slots: &mut Vec<Option<T>>, id: usize) {
    if let Some(s) = slots.get_mut(id) {
        *s = None;
    }
    while matches!(slots.last(), Some(None)) {
        slots.pop();
    }
}

impl Document {
    // ------------------------------------------------------- registration

    pub fn add_live_point(&mut self, p: BoundaryPoint) -> usize {
        alloc(&mut self.live_points, p)
    }

    pub fn live_point(&self, id: usize) -> Option<BoundaryPoint> {
        self.live_points.get(id).copied().flatten()
    }

    pub fn set_live_point(&mut self, id: usize, p: BoundaryPoint) {
        if let Some(s) = self.live_points.get_mut(id) {
            *s = Some(p);
        }
    }

    pub fn remove_live_point(&mut self, id: usize) {
        free(&mut self.live_points, id);
    }

    pub fn add_live_iterator(&mut self, p: IteratorPosition) -> usize {
        alloc(&mut self.live_iterators, p)
    }

    pub fn live_iterator(&self, id: usize) -> Option<IteratorPosition> {
        self.live_iterators.get(id).copied().flatten()
    }

    pub fn set_live_iterator(&mut self, id: usize, p: IteratorPosition) {
        if let Some(s) = self.live_iterators.get_mut(id) {
            *s = Some(p);
        }
    }

    pub fn remove_live_iterator(&mut self, id: usize) {
        free(&mut self.live_iterators, id);
    }

    pub(crate) fn has_live_positions(&self) -> bool {
        !self.live_points.is_empty() || !self.live_iterators.is_empty()
    }

    /// Index of `id` among its siblings.
    pub fn index_of(&self, id: NodeId) -> usize {
        let mut i = 0;
        let mut cur = self.nodes[id].prev_sibling;
        while let Some(p) = cur {
            i += 1;
            cur = self.nodes[p].prev_sibling;
        }
        i
    }

    fn is_inclusive_ancestor(&self, a: NodeId, b: NodeId) -> bool {
        a == b || self.is_ancestor(a, b)
    }

    // ------------------------------------------------------------- hooks

    /// Removing steps for live ranges and the NodeIterator pre-removing
    /// steps; `child` is still attached to `parent`.
    pub(crate) fn live_before_remove(&mut self, parent: NodeId, child: NodeId) {
        let index = self.index_of(child);
        for i in 0..self.live_points.len() {
            let Some(mut p) = self.live_points[i] else { continue };
            if self.is_inclusive_ancestor(child, p.node) {
                p = BoundaryPoint { node: parent, offset: index };
            } else if p.node == parent && p.offset > index {
                p.offset -= 1;
            }
            self.live_points[i] = Some(p);
        }
        for i in 0..self.live_iterators.len() {
            let Some(mut it) = self.live_iterators[i] else { continue };
            if child == it.root || !self.is_inclusive_ancestor(child, it.node) {
                continue;
            }
            if it.before {
                // The first node following `child` (not inside it) that is
                // still inside the iterator's root.
                let mut next = None;
                let mut n = child;
                while n != it.root {
                    if let Some(s) = self.nodes[n].next_sibling {
                        next = Some(s);
                        break;
                    }
                    match self.nodes[n].parent {
                        Some(p) => n = p,
                        None => break,
                    }
                }
                if let Some(next) = next {
                    it.node = next;
                    self.live_iterators[i] = Some(it);
                    continue;
                }
                it.before = false;
            }
            it.node = match self.nodes[child].prev_sibling {
                None => parent,
                Some(mut s) => {
                    while let Some(l) = self.nodes[s].last_child {
                        s = l;
                    }
                    s
                }
            };
            self.live_iterators[i] = Some(it);
        }
    }

    /// Insertion steps: a node was inserted into `parent` at `index`.
    pub(crate) fn live_after_insert(&mut self, parent: NodeId, index: usize) {
        for p in self.live_points.iter_mut().flatten() {
            if p.node == parent && p.offset > index {
                p.offset += 1;
            }
        }
    }

    /// "Replace data" steps: `count` code units at `offset` of `node` were
    /// replaced by `added` code units.
    pub(crate) fn live_replace_data(&mut self, node: NodeId, offset: usize, count: usize, added: usize) {
        for p in self.live_points.iter_mut().flatten() {
            if p.node != node {
                continue;
            }
            if p.offset > offset && p.offset <= offset + count {
                p.offset = offset;
            } else if p.offset > offset + count {
                p.offset = p.offset + added - count;
            }
        }
    }

    /// Normalization steps before removing a following Text sibling.
    /// Its data has already been appended to `into`; `offset` is the
    /// UTF-16 length of the preceding text, excluding this sibling's data.
    /// Boundaries inside the sibling or immediately before it follow the
    /// data into the surviving node rather than the ordinary removal point.
    pub fn live_before_merge_text(&mut self, into: NodeId, from: NodeId, offset: usize) {
        if self.live_points.is_empty() {
            return;
        }
        let parent = self.nodes[from].parent;
        let index = self.index_of(from);
        for p in self.live_points.iter_mut().flatten() {
            if p.node == from {
                *p = BoundaryPoint { node: into, offset: offset + p.offset };
            } else if Some(p.node) == parent && p.offset == index {
                *p = BoundaryPoint { node: into, offset };
            }
        }
    }

    // ------------------------------------------------- character data

    /// Character data of a text or comment node.
    pub fn char_data(&self, id: NodeId) -> Option<&str> {
        match &self.nodes[id].data {
            NodeData::Text(t) | NodeData::Comment(t) => Some(t),
            _ => None,
        }
    }

    /// DOM "replace data": replace `count` UTF-16 code units at `offset`
    /// of a text or comment node with `data`, updating live ranges.
    /// Returns the previous value (None for other nodes). Offsets past the
    /// end are clamped.
    pub fn replace_data(&mut self, id: NodeId, offset: usize, count: usize, data: &str) -> Option<String> {
        let old = self.char_data(id)?.to_string();
        let units: Vec<u16> = old.encode_utf16().collect();
        let offset = offset.min(units.len());
        let count = count.min(units.len() - offset);
        let mut out: Vec<u16> = Vec::with_capacity(units.len() - count + data.len());
        out.extend_from_slice(&units[..offset]);
        out.extend(data.encode_utf16());
        out.extend_from_slice(&units[offset + count..]);
        let new = String::from_utf16_lossy(&out);
        if new != old {
            if let NodeData::Text(t) | NodeData::Comment(t) = &mut self.nodes[id].data {
                *t = new;
            }
            self.record(Mutation::Text { node: id });
        }
        // Replacing a span with identical data still runs the live-range
        // replacement steps. Only the rendering invalidation is elided.
        if !self.live_points.is_empty() {
            self.live_replace_data(id, offset, count, data.encode_utf16().count());
        }
        Some(old)
    }

    /// Set the whole character data of a text or comment node.
    pub fn set_char_data(&mut self, id: NodeId, data: &str) -> Option<String> {
        let len = self.char_data(id)?.encode_utf16().count();
        self.replace_data(id, 0, len, data)
    }

    /// DOM `Text.splitText`: the text after `offset` (UTF-16 code units)
    /// moves to a new text node inserted after `id`; live ranges follow.
    pub fn split_text(&mut self, id: NodeId, offset: usize) -> NodeId {
        let units: Vec<u16> = self.char_data(id).unwrap_or("").encode_utf16().collect();
        let offset = offset.min(units.len());
        let tail = String::from_utf16_lossy(&units[offset..]);
        let new = self.create_text(&tail);
        if let Some(parent) = self.nodes[id].parent {
            let next = self.nodes[id].next_sibling;
            self.insert_before(parent, new, next);
            let index = self.index_of(id);
            for p in self.live_points.iter_mut().flatten() {
                if p.node == id && p.offset > offset {
                    *p = BoundaryPoint { node: new, offset: p.offset - offset };
                } else if p.node == parent && p.offset == index + 1 {
                    p.offset += 1;
                }
            }
        }
        self.replace_data(id, offset, units.len() - offset, "");
        new
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Document, NodeId) {
        let mut d = Document::new();
        let body = d.create_element("body", vec![]);
        d.append_child(Document::ROOT, body);
        (d, body)
    }

    #[test]
    fn removal_moves_points_to_the_parent() {
        let (mut d, body) = setup();
        let p = d.create_element("p", vec![]);
        d.append_child(body, p);
        let t = d.create_text("12345");
        d.append_child(p, t);
        let start = d.add_live_point(BoundaryPoint { node: t, offset: 2 });
        let end = d.add_live_point(BoundaryPoint { node: body, offset: 1 });
        d.detach(p);
        assert_eq!(d.live_point(start), Some(BoundaryPoint { node: body, offset: 0 }));
        assert_eq!(d.live_point(end), Some(BoundaryPoint { node: body, offset: 0 }));
        d.remove_live_point(start);
        d.remove_live_point(end);
        assert!(d.live_points.is_empty());
    }

    #[test]
    fn insertion_and_split_text_shift_points() {
        let (mut d, body) = setup();
        let t1 = d.create_text("12345");
        let t2 = d.create_text("ABCDE");
        d.append_child(body, t1);
        d.append_child(body, t2);
        let a = d.add_live_point(BoundaryPoint { node: t1, offset: 2 });
        let b = d.add_live_point(BoundaryPoint { node: t1, offset: 3 });
        let c = d.add_live_point(BoundaryPoint { node: body, offset: 1 });
        let new = d.split_text(t1, 2);
        assert_eq!(d.char_data(t1), Some("12"));
        assert_eq!(d.char_data(new), Some("345"));
        assert_eq!(d.live_point(a), Some(BoundaryPoint { node: t1, offset: 2 }));
        assert_eq!(d.live_point(b), Some(BoundaryPoint { node: new, offset: 1 }));
        assert_eq!(d.live_point(c), Some(BoundaryPoint { node: body, offset: 2 }));
        // Moving t2 in front of `new`: removal then insertion.
        d.insert_before(body, t2, Some(new));
        assert_eq!(d.live_point(c), Some(BoundaryPoint { node: body, offset: 3 }));
        d.replace_data(new, 0, 1, "xyz");
        assert_eq!(d.live_point(b), Some(BoundaryPoint { node: new, offset: 0 }));
    }

    #[test]
    fn identical_character_data_still_replaces_live_boundary_offsets() {
        let (mut d, body) = setup();
        let text = d.create_text("A😀éZ");
        d.append_child(body, text);
        let inside = d.add_live_point(BoundaryPoint { node: text, offset: 2 });
        let at_end = d.add_live_point(BoundaryPoint { node: text, offset: 3 });
        let after = d.add_live_point(BoundaryPoint { node: text, offset: 5 });
        d.replace_data(text, 1, 2, "😀");
        assert_eq!(d.char_data(text), Some("A😀éZ"));
        assert_eq!(d.live_point(inside).unwrap().offset, 1);
        assert_eq!(d.live_point(at_end).unwrap().offset, 1);
        assert_eq!(d.live_point(after).unwrap().offset, 5);
        d.set_char_data(text, "A😀éZ");
        assert_eq!(d.live_point(inside).unwrap().offset, 0);
        assert_eq!(d.live_point(after).unwrap().offset, 0);
    }

    #[test]
    fn iterator_pre_removing_steps() {
        let (mut d, body) = setup();
        let kids: Vec<NodeId> = (0..4).map(|_| d.create_element("t", vec![])).collect();
        for &k in &kids {
            d.append_child(body, k);
        }
        // Pointer after the last child: removing it moves to the previous one.
        let it = d.add_live_iterator(IteratorPosition { root: body, node: kids[3], before: true });
        d.detach(kids[3]);
        assert_eq!(d.live_iterator(it), Some(IteratorPosition { root: body, node: kids[2], before: false }));
        // Pointer before a node with a following sibling: moves forward.
        d.set_live_iterator(it, IteratorPosition { root: body, node: kids[1], before: true });
        d.detach(kids[1]);
        assert_eq!(d.live_iterator(it).unwrap().node, kids[2]);
        assert!(d.live_iterator(it).unwrap().before);
    }
}
