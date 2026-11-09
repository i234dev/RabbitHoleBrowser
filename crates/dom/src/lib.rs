//! A compact arena-based DOM.
//!
//! Nodes live in a `Vec` and reference each other by index (`NodeId`), which
//! keeps the tree cheap to build and trivially `Send`.

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
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    pub const ROOT: NodeId = 0;

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
}
