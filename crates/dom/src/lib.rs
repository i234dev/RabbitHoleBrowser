//! A compact arena-based DOM.
//!
//! Nodes live in a `Vec` and reference each other by index (`NodeId`), which
//! keeps the tree cheap to build and trivially `Send`.

use std::collections::HashMap;
use std::fmt;

pub type NodeId = usize;

#[derive(Debug, Clone)]
pub enum NodeData {
    Document,
    Element(ElementData),
    Text(String),
    Comment(String),
}

#[derive(Debug, Clone)]
pub struct ElementData {
    /// Lower-cased local name (`div`, `a`, `img`, ...).
    pub tag: String,
    /// Attributes in source order; names lower-cased.
    pub attrs: Vec<(String, String)>,
}

impl ElementData {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn has_class(&self, class: &str) -> bool {
        self.attr("class")
            .map(|c| c.split_ascii_whitespace().any(|x| x == class))
            .unwrap_or(false)
    }

    pub fn id(&self) -> Option<&str> {
        self.attr("id")
    }
}

#[derive(Debug, Clone)]
pub struct Node {
    pub parent: Option<NodeId>,
    pub first_child: Option<NodeId>,
    pub last_child: Option<NodeId>,
    pub prev_sibling: Option<NodeId>,
    pub next_sibling: Option<NodeId>,
    pub data: NodeData,
}

#[derive(Clone)]
pub struct Document {
    pub nodes: Vec<Node>,
    /// Shadow host → its shadow root. A shadow root is an element node
    /// tagged `#shadow-root` that has no parent (it is not a child of the
    /// host); its children form the shadow tree.
    pub shadow_roots: HashMap<NodeId, NodeId>,
    /// Shadow root → its host.
    pub shadow_hosts: HashMap<NodeId, NodeId>,
    /// Texts of constructed stylesheets adopted by the document (key
    /// `Document::ROOT`) or by a shadow root (`adoptedStyleSheets`).
    pub adopted_styles: HashMap<NodeId, Vec<String>>,
    /// CSSOM-modified sheets of `<style>` / `<link>` elements:
    /// element → (element source text when modified, sheet text to render).
    /// Ignored once the element's own text changes.
    pub sheet_overrides: HashMap<NodeId, (String, String)>,
    /// Custom elements whose constructor ran (`:defined`).
    pub defined_custom: std::collections::HashSet<NodeId>,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    pub const ROOT: NodeId = 0;
    /// `adopted_styles` key of the `@font-face` rules for fonts added
    /// through `document.fonts` (CSS Font Loading).
    pub const SCRIPT_FONT_FACES: NodeId = usize::MAX;

    pub fn new() -> Self {
        Document {
            nodes: vec![Node {
                parent: None,
                first_child: None,
                last_child: None,
                prev_sibling: None,
                next_sibling: None,
                data: NodeData::Document,
            }],
            shadow_roots: HashMap::new(),
            shadow_hosts: HashMap::new(),
            adopted_styles: HashMap::new(),
            sheet_overrides: HashMap::new(),
            defined_custom: std::collections::HashSet::new(),
        }
    }

    pub fn create(&mut self, data: NodeData) -> NodeId {
        self.nodes.push(Node {
            parent: None,
            first_child: None,
            last_child: None,
            prev_sibling: None,
            next_sibling: None,
            data,
        });
        self.nodes.len() - 1
    }

    pub fn create_element(&mut self, tag: &str, attrs: Vec<(String, String)>) -> NodeId {
        self.create(NodeData::Element(ElementData {
            tag: tag.to_ascii_lowercase(),
            attrs,
        }))
    }

    pub fn create_text(&mut self, text: &str) -> NodeId {
        self.create(NodeData::Text(text.to_string()))
    }

    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        debug_assert!(self.nodes[child].parent.is_none());
        let last = self.nodes[parent].last_child;
        self.nodes[child].parent = Some(parent);
        self.nodes[child].prev_sibling = last;
        self.nodes[child].next_sibling = None;
        match last {
            Some(l) => self.nodes[l].next_sibling = Some(child),
            None => self.nodes[parent].first_child = Some(child),
        }
        self.nodes[parent].last_child = Some(child);
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id]
    }

    // ------------------------------------------------------------ mutation

    /// Detach `child` from its parent (it stays allocated; `parent` becomes
    /// None). No-op for the document node.
    pub fn detach(&mut self, child: NodeId) {
        let Some(parent) = self.nodes[child].parent else { return };
        let prev = self.nodes[child].prev_sibling;
        let next = self.nodes[child].next_sibling;
        match prev {
            Some(p) => self.nodes[p].next_sibling = next,
            None => self.nodes[parent].first_child = next,
        }
        match next {
            Some(n) => self.nodes[n].prev_sibling = prev,
            None => self.nodes[parent].last_child = prev,
        }
        let c = &mut self.nodes[child];
        c.parent = None;
        c.prev_sibling = None;
        c.next_sibling = None;
    }

    /// Insert `child` into `parent` before `reference` (append when None).
    /// The child is detached from any previous position first.
    pub fn insert_before(&mut self, parent: NodeId, child: NodeId, reference: Option<NodeId>) {
        if child == parent || self.is_ancestor(child, parent) {
            return;
        }
        self.detach(child);
        match reference {
            None => self.append_child(parent, child),
            Some(r) => {
                if self.nodes[r].parent != Some(parent) {
                    self.append_child(parent, child);
                    return;
                }
                let prev = self.nodes[r].prev_sibling;
                self.nodes[child].parent = Some(parent);
                self.nodes[child].prev_sibling = prev;
                self.nodes[child].next_sibling = Some(r);
                self.nodes[r].prev_sibling = Some(child);
                match prev {
                    Some(p) => self.nodes[p].next_sibling = Some(child),
                    None => self.nodes[parent].first_child = Some(child),
                }
            }
        }
    }

    /// Replace `old` with `new` in `old`'s parent.
    pub fn replace_child(&mut self, old: NodeId, new: NodeId) {
        let Some(parent) = self.nodes[old].parent else { return };
        let next = self.nodes[old].next_sibling;
        self.detach(old);
        self.insert_before(parent, new, next);
    }

    /// Is `a` an ancestor of `b` (strict)?
    pub fn is_ancestor(&self, a: NodeId, b: NodeId) -> bool {
        let mut cur = self.nodes[b].parent;
        while let Some(p) = cur {
            if p == a {
                return true;
            }
            cur = self.nodes[p].parent;
        }
        false
    }

    /// Deep-copy a subtree; the copy is detached.
    pub fn clone_subtree(&mut self, root: NodeId, deep: bool) -> NodeId {
        let data = self.nodes[root].data.clone();
        let copy = self.create(data);
        if deep {
            let children: Vec<NodeId> = self.children(root).collect();
            for c in children {
                let cc = self.clone_subtree(c, true);
                self.append_child(copy, cc);
            }
        }
        copy
    }

    /// Deep-copy a subtree from another document into this one; the copy
    /// is detached.
    pub fn import_subtree(&mut self, other: &Document, root: NodeId) -> NodeId {
        let copy = self.create(other.nodes[root].data.clone());
        for c in other.children(root) {
            let cc = self.import_subtree(other, c);
            self.append_child(copy, cc);
        }
        copy
    }

    /// Remove all children of `parent` (detached, not freed).
    pub fn clear_children(&mut self, parent: NodeId) {
        let children: Vec<NodeId> = self.children(parent).collect();
        for c in children {
            self.detach(c);
        }
    }

    /// Set (`Some`) or remove (`None`) an attribute on an element.
    pub fn set_attr(&mut self, id: NodeId, name: &str, value: Option<&str>) {
        let name = name.to_ascii_lowercase();
        if let Some(el) = self.element_mut(id) {
            match value {
                Some(v) => match el.attrs.iter_mut().find(|(k, _)| *k == name) {
                    Some(slot) => slot.1 = v.to_string(),
                    None => el.attrs.push((name, v.to_string())),
                },
                None => el.attrs.retain(|(k, _)| *k != name),
            }
        }
    }

    /// Replace the text of a text node.
    pub fn set_text(&mut self, id: NodeId, text: &str) {
        if let NodeData::Text(t) = &mut self.nodes[id].data {
            *t = text.to_string();
        }
    }

    /// Is the node attached to the document (reachable from the root,
    /// crossing from shadow roots to their hosts)?
    pub fn is_connected(&self, id: NodeId) -> bool {
        let mut cur = id;
        loop {
            if cur == Self::ROOT {
                return true;
            }
            match self.nodes[cur].parent {
                Some(p) => cur = p,
                None => match self.shadow_hosts.get(&cur) {
                    Some(&h) => cur = h,
                    None => return false,
                },
            }
        }
    }

    // ---------------------------------------------------------- shadow DOM

    /// Attach a shadow root to `host` (or return the existing one).
    pub fn attach_shadow(&mut self, host: NodeId) -> NodeId {
        if let Some(&r) = self.shadow_roots.get(&host) {
            return r;
        }
        let root = self.create_element("#shadow-root", vec![]);
        self.shadow_roots.insert(host, root);
        self.shadow_hosts.insert(root, host);
        root
    }

    pub fn shadow_root(&self, host: NodeId) -> Option<NodeId> {
        self.shadow_roots.get(&host).copied()
    }

    pub fn shadow_host(&self, root: NodeId) -> Option<NodeId> {
        self.shadow_hosts.get(&root).copied()
    }

    pub fn is_shadow_root(&self, id: NodeId) -> bool {
        self.shadow_hosts.contains_key(&id)
    }

    /// The root of the tree containing `id` (the document, a shadow root,
    /// or the top of a detached subtree).
    pub fn tree_root(&self, mut id: NodeId) -> NodeId {
        while let Some(p) = self.nodes[id].parent {
            id = p;
        }
        id
    }

    /// The shadow root `id` lives in, if any.
    pub fn containing_shadow_root(&self, id: NodeId) -> Option<NodeId> {
        if self.shadow_hosts.is_empty() {
            return None;
        }
        let r = self.tree_root(id);
        self.is_shadow_root(r).then_some(r)
    }

    /// Parent in the shadow-including tree: a shadow root's parent is its
    /// host.
    pub fn shadow_including_parent(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id].parent.or_else(|| self.shadow_host(id))
    }

    /// Is `a` a shadow-including inclusive ancestor of `b`?
    pub fn is_shadow_including_inclusive_ancestor(&self, a: NodeId, b: NodeId) -> bool {
        let mut cur = Some(b);
        while let Some(c) = cur {
            if c == a {
                return true;
            }
            cur = self.shadow_including_parent(c);
        }
        false
    }

    /// The slot name of a slottable (`slot` attribute; "" for text and
    /// elements without one).
    pub fn slottable_name(&self, id: NodeId) -> &str {
        self.attr(id, "slot").unwrap_or("")
    }

    fn is_slottable(&self, id: NodeId) -> bool {
        matches!(self.nodes[id].data, NodeData::Element(_) | NodeData::Text(_))
    }

    /// The first `<slot>` in `root`'s tree (tree order) named `name`.
    fn find_slot(&self, root: NodeId, name: &str) -> Option<NodeId> {
        self.descendants(root).skip(1).find(|&n| self.is_element(n, "slot") && self.attr(n, "name").unwrap_or("") == name)
    }

    /// The slot a slottable is assigned to (its parent must be a shadow
    /// host whose shadow tree has a matching slot).
    pub fn assigned_slot(&self, id: NodeId) -> Option<NodeId> {
        let parent = self.nodes[id].parent?;
        let root = self.shadow_root(parent)?;
        if !self.is_slottable(id) {
            return None;
        }
        self.find_slot(root, self.slottable_name(id))
    }

    /// Nodes assigned to `slot` (empty when it is not in a shadow tree or
    /// is not the first slot of its name).
    pub fn assigned_nodes(&self, slot: NodeId) -> Vec<NodeId> {
        let Some(root) = self.containing_shadow_root(slot) else { return Vec::new() };
        let Some(host) = self.shadow_host(root) else { return Vec::new() };
        let name = self.attr(slot, "name").unwrap_or("");
        if self.find_slot(root, name) != Some(slot) {
            return Vec::new();
        }
        self.children(host).filter(|&c| self.is_slottable(c) && self.slottable_name(c) == name).collect()
    }

    /// Children in the flat tree (what gets rendered): a shadow host's
    /// shadow-root children, a slot's assigned nodes (or its fallback
    /// children), otherwise the DOM children.
    pub fn flat_children(&self, id: NodeId) -> Vec<NodeId> {
        if !self.shadow_roots.is_empty() {
            if let Some(root) = self.shadow_root(id) {
                return self.children(root).collect();
            }
            if self.is_element(id, "slot") && self.containing_shadow_root(id).is_some() {
                let assigned = self.assigned_nodes(id);
                if !assigned.is_empty() {
                    return assigned;
                }
            }
        }
        self.children(id).collect()
    }

    /// Element children in the flat tree.
    pub fn flat_child_elements(&self, id: NodeId) -> Vec<NodeId> {
        self.flat_children(id).into_iter().filter(|&c| matches!(self.nodes[c].data, NodeData::Element(_))).collect()
    }

    /// Parent in the flat tree: the assigned slot for children of a shadow
    /// host (`None` when unassigned: not rendered), the host for top-level
    /// nodes of a shadow tree, otherwise the DOM parent.
    pub fn flat_parent(&self, id: NodeId) -> Option<NodeId> {
        let parent = self.nodes[id].parent?;
        if self.shadow_roots.is_empty() {
            return Some(parent);
        }
        if self.shadow_roots.contains_key(&parent) {
            return self.assigned_slot(id);
        }
        if let Some(h) = self.shadow_host(parent) {
            return Some(h);
        }
        Some(parent)
    }

    /// Pre-order traversal that also enters shadow trees (a host's shadow
    /// root and its subtree come right after the host).
    pub fn shadow_including_descendants(&self, root: NodeId) -> Vec<NodeId> {
        if self.shadow_roots.is_empty() {
            return self.descendants(root).collect();
        }
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            out.push(n);
            let kids: Vec<NodeId> = self.children(n).collect();
            for &k in kids.iter().rev() {
                stack.push(k);
            }
            if let Some(r) = self.shadow_root(n) {
                stack.push(r);
            }
        }
        out
    }

    /// `:defined`: not a custom element name, or an upgraded custom element.
    pub fn is_defined(&self, id: NodeId) -> bool {
        match self.tag(id) {
            Some(t) if t.contains('-') => self.defined_custom.contains(&id),
            _ => true,
        }
    }

    /// Shadow roots whose host is connected, in creation order.
    pub fn connected_shadow_roots(&self) -> Vec<NodeId> {
        let mut v: Vec<NodeId> = self.shadow_hosts.iter().filter(|(_, &h)| self.is_connected(h)).map(|(&r, _)| r).collect();
        v.sort_unstable();
        v
    }

    pub fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id].next_sibling
    }

    pub fn prev_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id].prev_sibling
    }

    pub fn first_child(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id].first_child
    }

    pub fn last_child(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id].last_child
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Serialize a subtree back to HTML (for `innerHTML`/`outerHTML`).
    pub fn serialize(&self, id: NodeId, include_self: bool) -> String {
        let mut out = String::new();
        if include_self {
            self.serialize_into(id, &mut out);
        } else {
            for c in self.children(id) {
                self.serialize_into(c, &mut out);
            }
        }
        out
    }

    fn serialize_into(&self, id: NodeId, out: &mut String) {
        const VOID: &[&str] = &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"];
        match &self.nodes[id].data {
            NodeData::Document => {
                for c in self.children(id) {
                    self.serialize_into(c, out);
                }
            }
            NodeData::Element(e) => {
                out.push('<');
                out.push_str(&e.tag);
                for (k, v) in &e.attrs {
                    out.push(' ');
                    out.push_str(k);
                    out.push_str("=\"");
                    out.push_str(&v.replace('&', "&amp;").replace('"', "&quot;"));
                    out.push('"');
                }
                out.push('>');
                if VOID.contains(&e.tag.as_str()) {
                    return;
                }
                let raw = matches!(e.tag.as_str(), "script" | "style" | "xmp" | "iframe" | "noembed" | "noframes" | "plaintext");
                for c in self.children(id) {
                    if raw {
                        if let NodeData::Text(t) = &self.nodes[c].data {
                            out.push_str(t);
                            continue;
                        }
                    }
                    self.serialize_into(c, out);
                }
                out.push_str("</");
                out.push_str(&e.tag);
                out.push('>');
            }
            NodeData::Text(t) => {
                out.push_str(&t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('\u{a0}', "&nbsp;"));
            }
            NodeData::Comment(c) => {
                out.push_str("<!--");
                out.push_str(c);
                out.push_str("-->");
            }
        }
    }

    pub fn element(&self, id: NodeId) -> Option<&ElementData> {
        match &self.nodes[id].data {
            NodeData::Element(e) => Some(e),
            _ => None,
        }
    }

    pub fn element_mut(&mut self, id: NodeId) -> Option<&mut ElementData> {
        match &mut self.nodes[id].data {
            NodeData::Element(e) => Some(e),
            _ => None,
        }
    }

    pub fn tag(&self, id: NodeId) -> Option<&str> {
        self.element(id).map(|e| e.tag.as_str())
    }

    pub fn is_element(&self, id: NodeId, tag: &str) -> bool {
        self.tag(id) == Some(tag)
    }

    pub fn is_text(&self, id: NodeId) -> bool {
        matches!(self.nodes[id].data, NodeData::Text(_))
    }

    pub fn text(&self, id: NodeId) -> Option<&str> {
        match &self.nodes[id].data {
            NodeData::Text(t) => Some(t.as_str()),
            _ => None,
        }
    }

    pub fn attr(&self, id: NodeId, name: &str) -> Option<&str> {
        self.element(id).and_then(|e| e.attr(name))
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id].parent
    }

    pub fn children(&self, id: NodeId) -> Children<'_> {
        Children {
            doc: self,
            next: self.nodes[id].first_child,
        }
    }

    pub fn child_elements(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.children(id)
            .filter(move |&c| matches!(self.nodes[c].data, NodeData::Element(_)))
    }

    /// Pre-order traversal of the subtree rooted at `root` (inclusive).
    pub fn descendants(&self, root: NodeId) -> Descendants<'_> {
        Descendants {
            doc: self,
            root,
            next: Some(root),
        }
    }

    /// Concatenated text of all text descendants.
    pub fn text_content(&self, id: NodeId) -> String {
        let mut out = String::new();
        for n in self.descendants(id) {
            if let NodeData::Text(t) = &self.nodes[n].data {
                out.push_str(t);
            }
        }
        out
    }

    /// First element in document order with the given tag.
    pub fn find_element(&self, tag: &str) -> Option<NodeId> {
        self.descendants(Self::ROOT)
            .find(|&n| self.is_element(n, tag))
    }

    /// First element with the given `id` attribute.
    pub fn element_by_id(&self, id: &str) -> Option<NodeId> {
        self.descendants(Self::ROOT)
            .find(|&n| self.element(n).map(|e| e.id() == Some(id)).unwrap_or(false))
    }

    pub fn html_element(&self) -> Option<NodeId> {
        self.child_elements(Self::ROOT)
            .find(|&n| self.is_element(n, "html"))
    }

    pub fn body(&self) -> Option<NodeId> {
        let html = self.html_element()?;
        self.child_elements(html).find(|&n| self.is_element(n, "body"))
    }

    pub fn head(&self) -> Option<NodeId> {
        let html = self.html_element()?;
        self.child_elements(html).find(|&n| self.is_element(n, "head"))
    }

    pub fn title(&self) -> Option<String> {
        self.find_element("title").map(|t| {
            self.text_content(t)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
    }

    /// Nearest ancestor-or-self that satisfies `pred`.
    pub fn ancestor_or_self(&self, mut id: NodeId, pred: impl Fn(NodeId) -> bool) -> Option<NodeId> {
        loop {
            if pred(id) {
                return Some(id);
            }
            id = self.nodes[id].parent?;
        }
    }

    pub fn dump(&self) -> String {
        let mut s = String::new();
        self.dump_into(Self::ROOT, 0, &mut s);
        s
    }

    fn dump_into(&self, id: NodeId, depth: usize, out: &mut String) {
        use std::fmt::Write;
        let pad = "  ".repeat(depth);
        match &self.nodes[id].data {
            NodeData::Document => writeln!(out, "{pad}#document").unwrap(),
            NodeData::Element(e) => {
                write!(out, "{pad}<{}", e.tag).unwrap();
                for (k, v) in &e.attrs {
                    write!(out, " {k}=\"{}\"", v.replace('"', "&quot;")).unwrap();
                }
                writeln!(out, ">").unwrap();
            }
            NodeData::Text(t) => {
                let t = t.trim();
                if !t.is_empty() {
                    let short: String = t.chars().take(60).collect();
                    writeln!(out, "{pad}\"{short}\"").unwrap();
                }
            }
            NodeData::Comment(c) => {
                let short: String = c.chars().take(40).collect();
                writeln!(out, "{pad}<!-- {short} -->").unwrap();
            }
        }
        for c in self.children(id) {
            self.dump_into(c, depth + 1, out);
        }
    }
}

impl fmt::Debug for Document {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.dump())
    }
}

pub struct Children<'a> {
    doc: &'a Document,
    next: Option<NodeId>,
}

impl Iterator for Children<'_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<NodeId> {
        let cur = self.next?;
        self.next = self.doc.nodes[cur].next_sibling;
        Some(cur)
    }
}

pub struct Descendants<'a> {
    doc: &'a Document,
    root: NodeId,
    next: Option<NodeId>,
}

impl Iterator for Descendants<'_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<NodeId> {
        let cur = self.next?;
        let nodes = &self.doc.nodes;
        if let Some(c) = nodes[cur].first_child {
            self.next = Some(c);
        } else {
            let mut n = cur;
            loop {
                if n == self.root {
                    self.next = None;
                    break;
                }
                if let Some(s) = nodes[n].next_sibling {
                    self.next = Some(s);
                    break;
                }
                match nodes[n].parent {
                    Some(p) => n = p,
                    None => {
                        self.next = None;
                        break;
                    }
                }
            }
        }
        Some(cur)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_ops() {
        let mut d = Document::new();
        let html = d.create_element("HTML", vec![]);
        d.append_child(Document::ROOT, html);
        let body = d.create_element("body", vec![("id".into(), "b".into())]);
        d.append_child(html, body);
        let t = d.create_text("hi");
        d.append_child(body, t);
        assert_eq!(d.tag(html), Some("html"));
        assert_eq!(d.body(), Some(body));
        assert_eq!(d.element_by_id("b"), Some(body));
        assert_eq!(d.text_content(html), "hi");
        assert_eq!(d.descendants(Document::ROOT).count(), 4);
    }

    #[test]
    fn mutation() {
        let mut d = Document::new();
        let html = d.create_element("html", vec![]);
        d.append_child(Document::ROOT, html);
        let a = d.create_element("a", vec![]);
        let b = d.create_element("b", vec![]);
        let c = d.create_element("c", vec![]);
        d.append_child(html, a);
        d.append_child(html, c);
        d.insert_before(html, b, Some(c));
        let tags: Vec<_> = d.children(html).map(|n| d.tag(n).unwrap().to_string()).collect();
        assert_eq!(tags, ["a", "b", "c"]);
        d.detach(b);
        assert!(!d.is_connected(b));
        let tags: Vec<_> = d.children(html).map(|n| d.tag(n).unwrap().to_string()).collect();
        assert_eq!(tags, ["a", "c"]);
        d.replace_child(c, b);
        let tags: Vec<_> = d.children(html).map(|n| d.tag(n).unwrap().to_string()).collect();
        assert_eq!(tags, ["a", "b"]);
        d.set_attr(a, "ID", Some("x"));
        assert_eq!(d.element_by_id("x"), Some(a));
        let t = d.create_text("hi & <bye>");
        d.append_child(a, t);
        let copy = d.clone_subtree(html, true);
        assert_eq!(d.serialize(copy, true), "<html><a id=\"x\">hi &amp; &lt;bye&gt;</a><b></b></html>");
        // Cycles are refused.
        d.insert_before(a, html, None);
        assert_eq!(d.parent(html), Some(Document::ROOT));
    }

    #[test]
    fn shadow_trees_and_slots() {
        let mut d = Document::new();
        let html = d.create_element("html", vec![]);
        d.append_child(Document::ROOT, html);
        let host = d.create_element("x-card", vec![]);
        d.append_child(html, host);
        let title = d.create_element("span", vec![("slot".into(), "title".into())]);
        let text = d.create_text("body text");
        let unassigned = d.create_element("i", vec![("slot".into(), "nope".into())]);
        d.append_child(host, title);
        d.append_child(host, text);
        d.append_child(host, unassigned);
        let root = d.attach_shadow(host);
        assert_eq!(d.attach_shadow(host), root);
        let header = d.create_element("header", vec![]);
        let s_title = d.create_element("slot", vec![("name".into(), "title".into())]);
        let s_default = d.create_element("slot", vec![]);
        let fallback = d.create_text("fallback");
        d.append_child(root, header);
        d.append_child(header, s_title);
        d.append_child(root, s_default);
        d.append_child(s_default, fallback);
        assert!(d.is_connected(s_title));
        assert_eq!(d.parent(root), None);
        assert_eq!(d.shadow_host(root), Some(host));
        assert_eq!(d.containing_shadow_root(s_title), Some(root));
        assert_eq!(d.assigned_slot(title), Some(s_title));
        assert_eq!(d.assigned_slot(text), Some(s_default));
        assert_eq!(d.assigned_slot(unassigned), None);
        assert_eq!(d.assigned_nodes(s_default), vec![text]);
        assert_eq!(d.flat_children(host), vec![header, s_default]);
        assert_eq!(d.flat_children(s_title), vec![title]);
        assert_eq!(d.flat_parent(title), Some(s_title));
        assert_eq!(d.flat_parent(header), Some(host));
        assert_eq!(d.flat_parent(unassigned), None);
        assert!(d.is_shadow_including_inclusive_ancestor(html, s_title));
        // With nothing assigned, a slot renders its fallback content.
        d.detach(text);
        assert_eq!(d.flat_children(s_default), vec![fallback]);
        assert_eq!(d.connected_shadow_roots(), vec![root]);
    }
}
