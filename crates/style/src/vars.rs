//! Custom-property maps (`--*` values per element).
//!
//! Elements mostly inherit their parent's map unchanged, and an element
//! that declares custom properties usually changes only a few of them. A
//! map is therefore a layer of the element's own entries over its parent's
//! (shared) map: deriving one costs O(own entries) instead of a copy of
//! every inherited property, and values are shared token lists.

use rabbithole_css::Token;
use std::collections::HashMap;
use std::rc::Rc;

/// Layers deeper than this are flattened, which bounds lookups.
const MAX_DEPTH: u16 = 24;

#[derive(Clone, Default)]
pub(crate) struct Vars {
    /// This layer's entries (`None`: the property is removed here).
    own: HashMap<Rc<str>, Option<Rc<[Token]>>>,
    parent: Option<Rc<Vars>>,
    /// Number of layers below this one.
    depth: u16,
}

impl Vars {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// An empty layer over `parent`.
    pub(crate) fn child_of(parent: &Rc<Vars>) -> Self {
        if parent.own.is_empty() && parent.parent.is_none() {
            return Self::default();
        }
        if parent.depth >= MAX_DEPTH {
            return Vars { own: parent.flatten(), parent: None, depth: 0 };
        }
        Vars { own: HashMap::new(), parent: Some(parent.clone()), depth: parent.depth + 1 }
    }

    pub(crate) fn get(&self, name: &str) -> Option<&[Token]> {
        let mut cur = self;
        loop {
            if !cur.own.is_empty() {
                if let Some(v) = cur.own.get(name) {
                    return v.as_deref();
                }
            }
            cur = cur.parent.as_deref()?;
        }
    }

    pub(crate) fn insert(&mut self, name: &str, value: Rc<[Token]>) {
        self.own.insert(Rc::from(name), Some(value));
    }

    pub(crate) fn remove(&mut self, name: &str) {
        if self.parent.as_ref().map(|p| p.get(name).is_some()).unwrap_or(false) {
            self.own.insert(Rc::from(name), None);
        } else {
            self.own.remove(name);
        }
    }

    /// Every defined property, with this layer's entries overriding.
    fn flatten(&self) -> HashMap<Rc<str>, Option<Rc<[Token]>>> {
        let mut layers = Vec::new();
        let mut cur = Some(self);
        while let Some(l) = cur {
            layers.push(l);
            cur = l.parent.as_deref();
        }
        let mut out: HashMap<Rc<str>, Option<Rc<[Token]>>> = HashMap::new();
        for l in layers.into_iter().rev() {
            for (k, v) in &l.own {
                match v {
                    Some(v) => {
                        out.insert(k.clone(), Some(v.clone()));
                    }
                    None => {
                        out.remove(k);
                    }
                }
            }
        }
        out
    }
}

impl PartialEq for Vars {
    fn eq(&self, other: &Self) -> bool {
        let same_parent = match (&self.parent, &other.parent) {
            (None, None) => true,
            (Some(a), Some(b)) => Rc::ptr_eq(a, b),
            _ => false,
        };
        if same_parent && self.own.len() == other.own.len() && self.own.iter().all(|(k, v)| other.own.get(k).map(|o| o.as_deref() == v.as_deref()).unwrap_or(false)) {
            return true;
        }
        let (a, b) = (self.flatten(), other.flatten());
        a.len() == b.len() && a.iter().all(|(k, v)| b.get(k).map(|o| o.as_deref() == v.as_deref()).unwrap_or(false))
    }
}

impl std::fmt::Debug for Vars {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.flatten().iter().filter_map(|(k, v)| v.as_ref().map(|v| (k, v)))).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn val(s: &str) -> Rc<[Token]> {
        Rc::from(vec![Token::Ident(s.to_string())])
    }

    #[test]
    fn layers_shadow_and_remove() {
        let mut root = Vars::new();
        root.insert("--a", val("1"));
        root.insert("--b", val("2"));
        let root = Rc::new(root);
        let mut child = Vars::child_of(&root);
        child.insert("--a", val("3"));
        child.remove("--b");
        assert_eq!(child.get("--a"), Some(&val("3")[..]));
        assert_eq!(child.get("--b"), None);
        assert_eq!(root.get("--b"), Some(&val("2")[..]));
        let child = Rc::new(child);
        let grand = Vars::child_of(&child);
        assert_eq!(grand.get("--a"), Some(&val("3")[..]));
        // Same contents through different layerings compare equal.
        let mut flat = Vars::new();
        flat.insert("--a", val("3"));
        assert!(grand == flat);
        flat.insert("--c", val("4"));
        assert!(grand != flat);
    }

    #[test]
    fn deep_chains_flatten() {
        let mut cur = Rc::new(Vars::new());
        for i in 0..100 {
            let mut v = Vars::child_of(&cur);
            v.insert(&format!("--v{i}"), val(&i.to_string()));
            assert!(v.depth <= MAX_DEPTH);
            cur = Rc::new(v);
        }
        for i in 0..100 {
            assert_eq!(cur.get(&format!("--v{i}")), Some(&val(&i.to_string())[..]));
        }
    }
}
