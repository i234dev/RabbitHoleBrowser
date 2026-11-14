//! Stable element references (`@e12`): a node keeps its ref for as long as
//! it exists in the document, across snapshots. Refs are never reused
//! within a tab, so a ref from a previous page is recognised as such.

use std::collections::HashMap;

use rabbithole_engine::dom::NodeId;

/// What a ref pointed to when it was handed out.
#[derive(Debug, Clone)]
pub struct RefEntry {
    pub frame: usize,
    pub node: NodeId,
    pub role: String,
    pub name: String,
    /// Navigation generation of the tab when assigned.
    pub generation: u32,
}

#[derive(Debug, Default)]
pub struct RefMap {
    by_node: HashMap<(usize, NodeId), u32>,
    entries: HashMap<u32, RefEntry>,
    next: u32,
    /// Bumped on every navigation (new document).
    pub generation: u32,
    forms: HashMap<(usize, NodeId), u32>,
    form_entries: HashMap<u32, (usize, NodeId, u32)>,
    next_form: u32,
}

/// A parsed reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ref {
    Element(u32),
    Form(u32),
}

impl Ref {
    /// Parse `@e12`, `e12`, `@f1`, `f1`.
    pub fn parse(s: &str) -> Option<Ref> {
        let s = s.trim().trim_start_matches('@');
        if let Some(n) = s.strip_prefix('e') {
            return n.parse().ok().map(Ref::Element);
        }
        if let Some(n) = s.strip_prefix('f') {
            return n.parse().ok().map(Ref::Form);
        }
        None
    }
}

impl std::fmt::Display for Ref {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ref::Element(n) => write!(f, "@e{n}"),
            Ref::Form(n) => write!(f, "@f{n}"),
        }
    }
}

impl RefMap {
    /// The ref of a node, assigning a new one on first sight.
    pub fn assign(&mut self, frame: usize, node: NodeId, role: &str, name: &str) -> u32 {
        if let Some(&r) = self.by_node.get(&(frame, node)) {
            if let Some(e) = self.entries.get_mut(&r) {
                e.role = role.to_string();
                e.name = name.to_string();
            }
            return r;
        }
        self.next += 1;
        let r = self.next;
        self.by_node.insert((frame, node), r);
        self.entries.insert(r, RefEntry { frame, node, role: role.to_string(), name: name.to_string(), generation: self.generation });
        r
    }

    /// The ref of a node, if it has one.
    pub fn lookup(&self, frame: usize, node: NodeId) -> Option<u32> {
        self.by_node.get(&(frame, node)).copied()
    }

    pub fn get(&self, r: u32) -> Option<&RefEntry> {
        self.entries.get(&r)
    }

    /// Form ref (`@f1`).
    pub fn assign_form(&mut self, frame: usize, node: NodeId) -> u32 {
        if let Some(&r) = self.forms.get(&(frame, node)) {
            return r;
        }
        self.next_form += 1;
        let r = self.next_form;
        self.forms.insert((frame, node), r);
        self.form_entries.insert(r, (frame, node, self.generation));
        r
    }

    pub fn get_form(&self, r: u32) -> Option<(usize, NodeId, u32)> {
        self.form_entries.get(&r).copied()
    }

    /// A new document was loaded: old refs stop resolving (but are
    /// remembered to explain errors).
    pub fn new_document(&mut self) {
        self.generation += 1;
        self.by_node.clear();
        self.forms.clear();
        let keep = self.generation.saturating_sub(3);
        self.entries.retain(|_, e| e.generation >= keep);
        self.form_entries.retain(|_, e| e.2 >= keep);
    }

    /// Forget the refs of one frame (it was reloaded).
    pub fn drop_frame(&mut self, frame: usize) {
        self.by_node.retain(|k, _| k.0 != frame);
        self.forms.retain(|k, _| k.0 != frame);
        // A replacement document can reuse connected NodeIds in this frame.
        // Old entries must therefore stop resolving, not only lose their index.
        self.entries.retain(|_, entry| entry.frame != frame);
        self.form_entries.retain(|_, entry| entry.0 != frame);
    }

    /// Forget a node's ref (it was removed from the document).
    pub fn forget(&mut self, frame: usize, node: NodeId) {
        self.by_node.remove(&(frame, node));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_and_generational() {
        let mut m = RefMap::default();
        let a = m.assign(0, 10, "button", "Go");
        let b = m.assign(0, 11, "link", "Home");
        assert_eq!(m.assign(0, 10, "button", "Go!"), a);
        assert_ne!(a, b);
        assert_eq!(m.get(a).unwrap().name, "Go!");
        m.new_document();
        assert!(m.lookup(0, 10).is_none());
        assert_eq!(m.get(a).unwrap().generation, 0);
        let c = m.assign(0, 10, "button", "Go");
        assert!(c > b, "refs are never reused");
        assert_eq!(Ref::parse("@e12"), Some(Ref::Element(12)));
        assert_eq!(Ref::parse("f3"), Some(Ref::Form(3)));
        assert_eq!(Ref::parse("x"), None);
        assert_eq!(Ref::Element(4).to_string(), "@e4");
    }

    #[test]
    fn drop_frame_invalidates_old_element_refs_when_node_ids_are_reused() {
        let mut refs = RefMap::default();
        let neighbor = refs.assign(0, 10, "button", "neighbor");
        let old = refs.assign(1, 10, "button", "old document");
        let generation = refs.generation;
        refs.drop_frame(1);
        let new = refs.assign(1, 10, "button", "new document, reused node id");
        assert_ne!(new, old, "a frame reload must assign a fresh ref even for the same NodeId");
        assert!(refs.get(old).is_none(), "resolve_in must not recover an old entry against a connected node of the new frame document");
        assert_eq!(refs.get(new).unwrap().name, "new document, reused node id");
        assert_eq!(refs.get(neighbor).unwrap().name, "neighbor");
        assert_eq!(refs.lookup(0, 10), Some(neighbor));
        assert_eq!(refs.lookup(1, 10), Some(new));
        assert_eq!(refs.generation, generation, "other frames keep their refs");
    }

    #[test]
    fn drop_frame_invalidates_old_form_refs_when_node_ids_are_reused() {
        let mut refs = RefMap::default();
        let neighbor = refs.assign_form(0, 20);
        let old = refs.assign_form(1, 20);
        let generation = refs.generation;
        refs.drop_frame(1);
        let new = refs.assign_form(1, 20);
        assert_ne!(new, old);
        assert!(refs.get_form(old).is_none(), "a ref from the old frame document must not resolve to the replacement form");
        assert_eq!(refs.get_form(new), Some((1, 20, generation)));
        assert_eq!(refs.get_form(neighbor), Some((0, 20, generation)));
        refs.drop_frame(1);
        assert!(refs.get_form(new).is_none());
        assert_eq!(refs.assign_form(0, 20), neighbor);
        assert_eq!(refs.generation, generation);
    }
}
