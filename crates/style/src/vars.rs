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
    /// The shared empty map (the root's inherited custom properties): a
    /// stable address lets per-element memos hit across restyles.
    pub(crate) fn empty() -> Rc<Vars> {
        thread_local! {
            static EMPTY: Rc<Vars> = Rc::new(Vars::default());
        }
        EMPTY.with(|e| e.clone())
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

fn same_own(a: &Vars, b: &Vars) -> bool {
    a.own.len() == b.own.len() && a.own.iter().all(|(k, v)| b.own.get(k).map(|o| o.as_deref() == v.as_deref()).unwrap_or(false))
}

/// The same entries layered the same way (layers compared by address
/// first): the usual case when a restyle recomputes an unchanged map.
fn same_layers(a: &Vars, b: &Vars) -> bool {
    let (mut a, mut b) = (a, b);
    loop {
        if std::ptr::eq(a, b) {
            return true;
        }
        if !same_own(a, b) {
            return false;
        }
        match (&a.parent, &b.parent) {
            (None, None) => return true,
            (Some(x), Some(y)) => {
                a = x;
                b = y;
            }
            _ => return false,
        }
    }
}

impl PartialEq for Vars {
    fn eq(&self, other: &Self) -> bool {
        if same_layers(self, other) {
            return true;
        }
        // Layers over the same map (a restyled element whose own
        // declarations changed): only the names they set can differ.
        let same_base = match (&self.parent, &other.parent) {
            (Some(x), Some(y)) => same_layers(x, y),
            (None, None) => true,
            _ => false,
        };
        if same_base {
            return self.own.keys().chain(other.own.keys()).all(|k| self.get(k) == other.get(k));
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
        let mut root = Vars::default();
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
        let mut flat = Vars::default();
        flat.insert("--a", val("3"));
        assert!(grand == flat);
        flat.insert("--c", val("4"));
        assert!(grand != flat);
    }

    #[test]
    fn layers_over_the_same_map_compare_by_their_names() {
        let mut root = Vars::default();
        root.insert("--a", val("1"));
        root.insert("--b", val("2"));
        let root = Rc::new(root);
        let layer = |f: &dyn Fn(&mut Vars)| {
            let mut v = Vars::child_of(&root);
            f(&mut v);
            v
        };
        let plain = layer(&|_| {});
        // Redeclaring an inherited value changes nothing.
        assert!(layer(&|v| v.insert("--a", val("1"))) == plain);
        assert!(layer(&|v| v.insert("--a", val("3"))) != plain);
        assert!(layer(&|v| v.remove("--b")) != plain);
        assert!(layer(&|v| v.remove("--b")) == layer(&|v| v.remove("--b")));
        assert!(layer(&|v| v.insert("--c", val("4"))) != layer(&|v| v.insert("--d", val("4"))));
        assert!(layer(&|v| v.insert("--c", val("4"))) == layer(&|v| v.insert("--c", val("4"))));
    }

    #[test]
    fn deep_chains_flatten() {
        let mut cur = Rc::new(Vars::default());
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
