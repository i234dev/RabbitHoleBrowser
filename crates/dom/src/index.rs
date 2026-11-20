//! Lookup indexes for `getElementById` and selector queries: elements by
//! the value of an attribute (`id`, `data-id`, ...) and by class.
//!
//! An index for an attribute is built the first time a query asks for it
//! and kept up to date by the attribute and node-creation APIs of
//! [`Document`]. Entries are candidates, not answers: they cover every
//! element of the arena (detached ones, template contents and shadow trees
//! too) whose attribute had the value, so a query keeps the candidates
//! inside its root and checks each against the full selector.

use crate::{Document, ElementData, NodeData, NodeId};
use std::collections::HashMap;

/// Buckets bigger than this are not tracked (a scan is as good).
const MAX_BUCKET: usize = 256;
/// Attribute names indexed at most per document.
const MAX_NAMES: usize = 32;
/// Unprocessed raw changes beyond this drop the indexes.
const MAX_DIRTY: usize = 4096;

#[derive(Debug)]
enum Bucket {
    Nodes(Vec<NodeId>),
    /// Too many elements share the value.
    Overfull,
}

impl Bucket {
    fn add(&mut self, id: NodeId, unique: bool) {
        if let Bucket::Nodes(v) = self {
            if unique && v.contains(&id) {
                return;
            }
            if v.len() >= MAX_BUCKET {
                *self = Bucket::Overfull;
            } else {
                v.push(id);
            }
        }
    }

    /// Remove `id`; true when the bucket is now empty.
    fn remove(&mut self, id: NodeId) -> bool {
        match self {
            Bucket::Nodes(v) => {
                if let Some(i) = v.iter().position(|&n| n == id) {
                    v.swap_remove(i);
                }
                v.is_empty()
            }
            Bucket::Overfull => false,
        }
    }
}

type Buckets = HashMap<String, Bucket>;

fn add_to(map: &mut Buckets, key: &str, id: NodeId, unique: bool) {
    match map.get_mut(key) {
        Some(b) => b.add(id, unique),
        None => {
            map.insert(key.to_string(), Bucket::Nodes(vec![id]));
        }
    }
}

fn remove_from(map: &mut Buckets, key: &str, id: NodeId) {
    if let Some(b) = map.get_mut(key) {
        if b.remove(id) {
            map.remove(key);
        }
    }
}

#[derive(Default, Debug)]
pub(crate) struct LookupIndex {
    /// Attribute name → value → elements.
    attrs: HashMap<String, Buckets>,
    /// Class name → elements.
    classes: Option<Buckets>,
    /// Elements changed through raw access since the last lookup.
    dirty: Vec<NodeId>,
}

/// A copied document builds its own indexes when queried.
impl Clone for LookupIndex {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl LookupIndex {
    #[inline]
    pub(crate) fn active(&self) -> bool {
        !self.attrs.is_empty() || self.classes.is_some()
    }

    /// A new element (or one whose data was replaced wholesale).
    pub(crate) fn add_element(&mut self, id: NodeId, el: &ElementData, unique: bool) {
        for (name, map) in self.attrs.iter_mut() {
            if let Some(v) = el.attr(name) {
                add_to(map, v, id, unique);
            }
        }
        if let Some(classes) = &mut self.classes {
            if let Some(c) = el.attr("class") {
                for t in c.split_ascii_whitespace() {
                    add_to(classes, t, id, unique);
                }
            }
        }
    }

    pub(crate) fn attr_changed(&mut self, id: NodeId, name: &str, old: Option<&str>, new: Option<&str>) {
        if let Some(map) = self.attrs.get_mut(name) {
            if let Some(o) = old {
                remove_from(map, o, id);
            }
            if let Some(n) = new {
                add_to(map, n, id, false);
            }
        }
        if name == "class" {
            if let Some(classes) = &mut self.classes {
                let old_tokens = old.unwrap_or("");
                let new_tokens = new.unwrap_or("");
                for t in old_tokens.split_ascii_whitespace() {
                    if !new_tokens.split_ascii_whitespace().any(|x| x == t) {
                        remove_from(classes, t, id);
                    }
                }
                for t in new_tokens.split_ascii_whitespace() {
                    if !old_tokens.split_ascii_whitespace().any(|x| x == t) {
                        add_to(classes, t, id, true);
                    }
                }
            }
        }
    }

    /// `id` may have changed in any way (raw mutable access).
    pub(crate) fn mark_dirty(&mut self, id: NodeId) {
        if self.dirty.len() >= MAX_DIRTY {
            *self = Self::default();
            return;
        }
        if self.dirty.last() != Some(&id) {
            self.dirty.push(id);
        }
    }
}

impl Document {
    fn flush_lookup_dirty(&mut self) {
        if self.lookup.dirty.is_empty() {
            return;
        }
        let dirty = std::mem::take(&mut self.lookup.dirty);
        for id in dirty {
            if let NodeData::Element(el) = &self.nodes[id].data {
                self.lookup.add_element(id, el, true);
            }
        }
    }

    /// Elements that may have attribute `name` equal to `value` (a
    /// superset, anywhere in the arena, possibly with duplicates), or
    /// `None` when too many elements share the value to be worth it.
    pub fn attr_candidates(&mut self, name: &str, value: &str) -> Option<&[NodeId]> {
        self.flush_lookup_dirty();
        if !self.lookup.attrs.contains_key(name) {
            if self.lookup.attrs.len() >= MAX_NAMES {
                return None;
            }
            let mut map = Buckets::new();
            for (id, node) in self.nodes.iter().enumerate() {
                if let NodeData::Element(el) = &node.data {
                    if let Some(v) = el.attr(name) {
                        add_to(&mut map, v, id, false);
                    }
                }
            }
            self.lookup.attrs.insert(name.to_string(), map);
        }
        match self.lookup.attrs.get(name)?.get(value) {
            None => Some(&[]),
            Some(Bucket::Nodes(v)) => Some(v),
            Some(Bucket::Overfull) => None,
        }
    }

    /// Elements that may have class `class` (see [`Document::attr_candidates`]).
    pub fn class_candidates(&mut self, class: &str) -> Option<&[NodeId]> {
        self.flush_lookup_dirty();
        if self.lookup.classes.is_none() {
            let mut map = Buckets::new();
            for (id, node) in self.nodes.iter().enumerate() {
                if let NodeData::Element(el) = &node.data {
                    if let Some(c) = el.attr("class") {
                        for t in c.split_ascii_whitespace() {
                            add_to(&mut map, t, id, true);
                        }
                    }
                }
            }
            self.lookup.classes = Some(map);
        }
        match self.lookup.classes.as_ref()?.get(class) {
            None => Some(&[]),
            Some(Bucket::Nodes(v)) => Some(v),
            Some(Bucket::Overfull) => None,
        }
    }

    /// Keep the elements of `nodes` that are descendants of `root`
    /// (exclusive; through parent links only, as [`Document::descendants`]
    /// walks) and put them in tree order without duplicates.
    pub fn retain_descendants_in_order(&self, root: NodeId, nodes: &mut Vec<NodeId>) {
        nodes.sort_unstable();
        nodes.dedup();
        nodes.retain(|&n| n < self.nodes.len() && self.element(n).is_some() && n != root && self.is_ancestor(root, n));
        if nodes.len() < 2 {
            return;
        }
        // Ancestor chains (from the top), computed once per node.
        let mut keyed: Vec<(Vec<NodeId>, NodeId)> = nodes
            .iter()
            .map(|&n| {
                let mut chain = vec![n];
                let mut cur = n;
                while let Some(p) = self.nodes[cur].parent {
                    chain.push(p);
                    cur = p;
                }
                chain.reverse();
                (chain, n)
            })
            .collect();
        keyed.sort_by(|(ca, _), (cb, _)| self.chain_order(ca, cb));
        nodes.clear();
        nodes.extend(keyed.into_iter().map(|(_, n)| n));
    }

    /// Tree order of two nodes given their ancestor chains (top first).
    fn chain_order(&self, ca: &[NodeId], cb: &[NodeId]) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        let common = ca.iter().zip(cb.iter()).take_while(|(x, y)| x == y).count();
        if common == ca.len() && common == cb.len() {
            return Ordering::Equal;
        }
        if common == ca.len() {
            return Ordering::Less; // ancestor first
        }
        if common == cb.len() {
            return Ordering::Greater;
        }
        if common == 0 {
            return ca[0].cmp(&cb[0]); // different trees
        }
        let (sa, sb) = (ca[common], cb[common]);
        let mut cur = self.nodes[sa].next_sibling;
        while let Some(s) = cur {
            if s == sb {
                return Ordering::Less;
            }
            cur = self.nodes[s].next_sibling;
        }
        Ordering::Greater
    }

    /// The first element in tree order with the given `id` (as
    /// [`Document::element_by_id`]), through the id index.
    pub fn element_by_id_indexed(&mut self, id: &str) -> Option<NodeId> {
        let Some(c) = self.attr_candidates("id", id) else { return self.element_by_id(id) };
        if c.is_empty() {
            return None;
        }
        let mut c = c.to_vec();
        c.retain(|&n| self.attr(n, "id") == Some(id));
        self.retain_descendants_in_order(Self::ROOT, &mut c);
        c.first().copied()
    }
}

#[cfg(test)]
mod tests {
    use crate::*;

    fn doc() -> (Document, NodeId, Vec<NodeId>) {
        let mut d = Document::new();
        let html = d.create_element("html", vec![]);
        d.append_child(Document::ROOT, html);
        let mut items = Vec::new();
        for i in 0..5 {
            let li = d.create_element("li", vec![("data-id".into(), i.to_string()), ("class".into(), "item".into())]);
            d.append_child(html, li);
            items.push(li);
        }
        (d, html, items)
    }

    #[test]
    fn id_index_follows_changes() {
        let (mut d, html, items) = doc();
        assert_eq!(d.element_by_id_indexed("a"), None);
        d.set_attr(items[3], "id", Some("a"));
        assert_eq!(d.element_by_id_indexed("a"), Some(items[3]));
        d.set_attr(items[1], "id", Some("a"));
        assert_eq!(d.element_by_id_indexed("a"), Some(items[1]));
        d.detach(items[1]);
        assert_eq!(d.element_by_id_indexed("a"), Some(items[3]));
        d.set_attr(items[3], "id", None);
        assert_eq!(d.element_by_id_indexed("a"), None);
        // Created after the index was built, inserted later.
        let n = d.create_element("p", vec![("id".into(), "a".into())]);
        assert_eq!(d.element_by_id_indexed("a"), None);
        d.insert_before(html, n, Some(items[0]));
        assert_eq!(d.element_by_id_indexed("a"), Some(n));
        // Raw changes.
        d.element_mut(items[4]).unwrap().attrs.push(("id".into(), "b".into()));
        assert_eq!(d.element_by_id_indexed("b"), Some(items[4]));
        // Copies index on their own.
        let copy = d.clone();
        let mut copy = copy;
        assert_eq!(copy.element_by_id_indexed("b"), Some(items[4]));
    }

    #[test]
    fn candidates_and_tree_order() {
        let (mut d, html, items) = doc();
        assert_eq!(d.attr_candidates("data-id", "2").map(|c| c.to_vec()), Some(vec![items[2]]));
        d.set_attr(items[2], "data-id", Some("9"));
        assert_eq!(d.attr_candidates("data-id", "2").map(|c| c.to_vec()), Some(vec![]));
        let mut c = d.class_candidates("item").unwrap().to_vec();
        c.reverse();
        d.retain_descendants_in_order(html, &mut c);
        assert_eq!(c, items);
        d.set_attr(items[0], "class", Some("other item"));
        d.set_attr(items[1], "class", Some("other"));
        let mut c = d.class_candidates("item").unwrap().to_vec();
        c.retain(|&n| d.element(n).unwrap().has_class("item"));
        d.retain_descendants_in_order(Document::ROOT, &mut c);
        assert_eq!(c, vec![items[0], items[2], items[3], items[4]]);
        assert_eq!(d.class_candidates("other").map(|c| c.len()), Some(2));
    }
}
