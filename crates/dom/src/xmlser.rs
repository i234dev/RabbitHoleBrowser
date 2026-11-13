//! The XML serialization algorithm (DOM Parsing and Serialization §3.2):
//! `XMLSerializer.serializeToString()` and `innerHTML` / `outerHTML` of
//! nodes in XML documents. Namespaces are declared where needed through
//! a namespace prefix map; with "require well-formed" the serialization
//! fails (InvalidStateError in the bindings) instead of producing markup
//! that would not parse back.

use crate::{Document, Namespace, NodeData, NodeId};
use std::collections::HashMap;

pub const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
pub const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";
const HTML_NS: &str = "http://www.w3.org/1999/xhtml";

const VOID: &[&str] = &[
    "area", "base", "basefont", "bgsound", "br", "col", "embed", "frame", "hr", "img", "input", "keygen", "link", "meta", "param", "source", "track", "wbr",
];

/// Namespace URI (None: null namespace) → prefixes, in order of definition.
#[derive(Clone, Default)]
struct PrefixMap(Vec<(Option<String>, Vec<String>)>);

impl PrefixMap {
    fn list(&self, ns: Option<&str>) -> Option<&Vec<String>> {
        self.0.iter().find(|(k, _)| k.as_deref() == ns).map(|(_, v)| v)
    }

    fn add(&mut self, ns: Option<&str>, prefix: &str) {
        match self.0.iter_mut().find(|(k, _)| k.as_deref() == ns) {
            Some((_, v)) => v.push(prefix.to_string()),
            None => self.0.push((ns.map(|s| s.to_string()), vec![prefix.to_string()])),
        }
    }

    fn has(&self, ns: Option<&str>, prefix: &str) -> bool {
        self.list(ns).map(|l| l.iter().any(|p| p == prefix)).unwrap_or(false)
    }

    /// "Retrieve a preferred prefix string".
    fn preferred(&self, ns: Option<&str>, preferred: Option<&str>) -> Option<String> {
        let list = self.list(ns)?;
        for (i, p) in list.iter().enumerate() {
            if Some(p.as_str()) == preferred || i == list.len() - 1 {
                return Some(p.clone());
            }
        }
        None
    }
}

struct Ser<'a> {
    doc: &'a Document,
    wf: bool,
    prefix_index: usize,
    out: String,
}

type R = Result<(), String>;

fn is_char(c: char) -> bool {
    matches!(c as u32, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
}

fn is_name(s: &str) -> bool {
    let start = |c: char| c.is_ascii_alphabetic() || c == '_' || c == ':' || (c as u32 >= 0xC0 && !matches!(c as u32, 0xD7 | 0xF7 | 0x37E | 0x2000..=0x200B | 0x200E..=0x206F | 0x2190..=0x2BFF | 0x2FF0..=0x3000 | 0xD800..=0xF8FF | 0xFDD0..=0xFDEF | 0xFFFE | 0xFFFF));
    let rest = |c: char| start(c) || c.is_ascii_digit() || c == '-' || c == '.' || c as u32 == 0xB7 || (0x300..=0x36F).contains(&(c as u32)) || (0x203F..=0x2040).contains(&(c as u32));
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if start(c) => chars.all(rest),
        _ => false,
    }
}

impl Ser<'_> {
    fn fail(&self, msg: &str) -> R {
        Err(msg.to_string())
    }

    fn node(&mut self, n: NodeId, ns: Option<&str>, map: &mut PrefixMap) -> R {
        let doc = self.doc;
        match &doc.node(n).data {
            NodeData::Element(e) if e.tag == "#fragment" || e.tag == "#shadow-root" => {
                for c in doc.children(n) {
                    self.node(c, ns, map)?;
                }
                Ok(())
            }
            NodeData::Element(_) => self.element(n, ns, map),
            NodeData::Document => {
                if self.wf && doc.child_elements(n).next().is_none() {
                    return self.fail("the document has no document element");
                }
                for c in doc.children(n) {
                    self.node(c, ns, map)?;
                }
                Ok(())
            }
            NodeData::Comment(data) => {
                if let Some(target) = doc.pi_target(n) {
                    if self.wf && (target.contains(':') || target.eq_ignore_ascii_case("xml")) {
                        return self.fail("invalid processing instruction target");
                    }
                    if self.wf && (!data.chars().all(is_char) || data.contains("?>")) {
                        return self.fail("invalid processing instruction data");
                    }
                    self.out.push_str("<?");
                    self.out.push_str(target);
                    self.out.push(' ');
                    self.out.push_str(data);
                    self.out.push_str("?>");
                    return Ok(());
                }
                if self.wf && (!data.chars().all(is_char) || data.contains("--") || data.ends_with('-')) {
                    return self.fail("the comment is not well-formed");
                }
                self.out.push_str("<!--");
                self.out.push_str(data);
                self.out.push_str("-->");
                Ok(())
            }
            NodeData::Text(t) => {
                if doc.is_cdata(n) {
                    self.out.push_str("<![CDATA[");
                    self.out.push_str(t);
                    self.out.push_str("]]>");
                    return Ok(());
                }
                if self.wf && !t.chars().all(is_char) {
                    return self.fail("the text contains characters that are not allowed in XML");
                }
                for c in t.chars() {
                    match c {
                        '&' => self.out.push_str("&amp;"),
                        '<' => self.out.push_str("&lt;"),
                        '>' => self.out.push_str("&gt;"),
                        c => self.out.push(c),
                    }
                }
                Ok(())
            }
            NodeData::Doctype(d) => {
                if self.wf && !d.public_id.chars().all(|c| c.is_ascii_alphanumeric() || " \r\n-'()+,./:=?;!*#@$_%".contains(c)) {
                    return self.fail("invalid doctype public id");
                }
                if self.wf && d.system_id.contains('"') && d.system_id.contains('\'') {
                    return self.fail("invalid doctype system id");
                }
                self.out.push_str("<!DOCTYPE ");
                self.out.push_str(&d.name);
                if !d.public_id.is_empty() {
                    self.out.push_str(" PUBLIC \"");
                    self.out.push_str(&d.public_id);
                    self.out.push('"');
                }
                if !d.system_id.is_empty() && d.public_id.is_empty() {
                    self.out.push_str(" SYSTEM");
                }
                if !d.system_id.is_empty() {
                    self.out.push_str(" \"");
                    self.out.push_str(&d.system_id);
                    self.out.push('"');
                }
                self.out.push('>');
                Ok(())
            }
        }
    }

    /// Attributes of `n` as (namespace, prefix, local name, value).
    fn attrs(&self, n: NodeId) -> Vec<(Option<String>, Option<String>, String, String)> {
        let Some(e) = self.doc.element(n) else { return Vec::new() };
        e.attrs
            .iter()
            .map(|(k, v)| {
                let ns = self.doc.attr_namespace(n, k).map(|s| s.to_string());
                let (prefix, local) = match (&ns, k.split_once(':')) {
                    (Some(_), Some((p, l))) => (Some(p.to_string()), l.to_string()),
                    _ => (None, k.clone()),
                };
                (ns, prefix, local, v.clone())
            })
            .collect()
    }

    fn attr_value(&self, v: Option<&str>) -> Result<String, String> {
        let Some(v) = v else { return Ok(String::new()) };
        if self.wf && !v.chars().all(is_char) {
            return Err("the attribute value contains characters that are not allowed in XML".into());
        }
        let mut out = String::with_capacity(v.len());
        for c in v.chars() {
            match c {
                '&' => out.push_str("&amp;"),
                '"' => out.push_str("&quot;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                // Kept through attribute-value normalization on reparse.
                '\t' => out.push_str("&#9;"),
                '\n' => out.push_str("&#xA;"),
                '\r' => out.push_str("&#xD;"),
                c => out.push(c),
            }
        }
        Ok(out)
    }

    fn generate_prefix(&mut self, map: &mut PrefixMap, ns: Option<&str>) -> String {
        let p = format!("ns{}", self.prefix_index);
        self.prefix_index += 1;
        map.add(ns, &p);
        p
    }

    fn element(&mut self, n: NodeId, context_ns: Option<&str>, parent_map: &mut PrefixMap) -> R {
        let doc = self.doc;
        let e = doc.element(n).expect("element");
        let local = e.tag.as_str();
        if self.wf && (local.contains(':') || !is_name(local)) {
            return self.fail("the element's local name is not a valid XML name");
        }
        let el_ns: Option<&str> = e.ns.uri();
        let el_prefix = e.prefix.as_deref();
        let mut map = parent_map.clone();
        let attrs = self.attrs(n);
        // Record the namespace information.
        let mut local_prefixes: HashMap<String, Option<String>> = HashMap::new();
        let mut local_default: Option<String> = None;
        for (ans, ap, al, av) in &attrs {
            if ans.as_deref() != Some(XMLNS_NS) {
                continue;
            }
            match ap {
                None => local_default = Some(av.clone()),
                Some(_) => {
                    let prefix_def = al.clone();
                    let ns_def = if av.is_empty() { None } else { Some(av.as_str()) };
                    if ns_def == Some(XML_NS) || map.has(ns_def, &prefix_def) {
                        continue;
                    }
                    map.add(ns_def, &prefix_def);
                    local_prefixes.insert(prefix_def, ns_def.map(|s| s.to_string()));
                }
            }
        }
        let mut inherited: Option<String> = context_ns.map(|s| s.to_string());
        let mut ignore_ns_def = false;
        let qualified: String;
        let mut decl = String::new();
        if inherited.as_deref() == el_ns {
            if local_default.is_some() {
                ignore_ns_def = true;
            }
            qualified = if el_ns == Some(XML_NS) { format!("xml:{local}") } else { local.to_string() };
        } else {
            let mut prefix = el_prefix.map(|s| s.to_string());
            let mut candidate = map.preferred(el_ns, prefix.as_deref());
            if prefix.as_deref() == Some("xmlns") {
                if self.wf {
                    return self.fail("an element cannot have the xmlns prefix");
                }
                candidate = prefix.clone();
            }
            let local_default_ns = |d: &Option<String>| d.as_ref().filter(|v| v.as_str() != XML_NS).map(|v| if v.is_empty() { None } else { Some(v.clone()) });
            if let Some(c) = candidate {
                qualified = format!("{c}:{local}");
                if let Some(d) = local_default_ns(&local_default) {
                    inherited = d;
                }
            } else if let Some(p) = prefix.take() {
                let p = if local_prefixes.contains_key(&p) { self.generate_prefix(&mut map, el_ns) } else {
                    map.add(el_ns, &p);
                    p
                };
                qualified = format!("{p}:{local}");
                decl = format!(" xmlns:{p}=\"{}\"", self.attr_value(el_ns)?);
                if let Some(d) = local_default_ns(&local_default) {
                    inherited = d;
                }
            } else if local_default.is_none() || local_default.as_deref() != Some(el_ns.unwrap_or("")) {
                ignore_ns_def = true;
                qualified = local.to_string();
                inherited = el_ns.map(|s| s.to_string());
                decl = format!(" xmlns=\"{}\"", self.attr_value(el_ns)?);
            } else {
                qualified = local.to_string();
                inherited = el_ns.map(|s| s.to_string());
            }
        }
        self.out.push('<');
        self.out.push_str(&qualified);
        self.out.push_str(&decl);
        // Attributes.
        let mut seen: Vec<(Option<String>, String)> = Vec::new();
        for (ans, ap, al, av) in &attrs {
            if self.wf && seen.iter().any(|(n, l)| n == ans && l == al) {
                return self.fail("duplicate attribute");
            }
            seen.push((ans.clone(), al.clone()));
            // A namespace-less `xmlns` attribute is not a namespace
            // declaration: the element's namespace decides.
            if ans.is_none() && al == "xmlns" && !self.wf {
                continue;
            }
            let mut candidate: Option<String> = None;
            if let Some(ns) = ans.as_deref() {
                candidate = map.preferred(Some(ns), ap.as_deref());
                if ns == XMLNS_NS {
                    let redefined = ap.is_some() && local_prefixes.get(al).map(|v| v.as_deref().unwrap_or("") != av.as_str()).unwrap_or(true) && map.has(if av.is_empty() { None } else { Some(av.as_str()) }, al);
                    if av == XML_NS || (ap.is_none() && ignore_ns_def) || redefined {
                        continue;
                    }
                    if self.wf && av == XMLNS_NS {
                        return self.fail("the XMLNS namespace cannot be declared");
                    }
                    if self.wf && av.is_empty() && ap.is_some() {
                        return self.fail("a prefix cannot be undeclared");
                    }
                    if ap.as_deref() == Some("xmlns") {
                        candidate = Some("xmlns".into());
                    }
                } else if candidate.is_none() {
                    let c = self.generate_prefix(&mut map, Some(ns));
                    let v = self.attr_value(Some(ns))?;
                    self.out.push_str(&format!(" xmlns:{c}=\"{v}\""));
                    candidate = Some(c);
                }
            }
            self.out.push(' ');
            if let Some(c) = &candidate {
                self.out.push_str(c);
                self.out.push(':');
            }
            if self.wf && (al.contains(':') || !is_name(al) || (al == "xmlns" && ans.is_none())) {
                return self.fail("the attribute's local name is not a valid XML name");
            }
            let v = self.attr_value(Some(av))?;
            self.out.push_str(al);
            self.out.push_str("=\"");
            self.out.push_str(&v);
            self.out.push('"');
        }
        let html = el_ns == Some(HTML_NS);
        let content = if html && local == "template" { doc.template_contents.get(&n).copied().unwrap_or(n) } else { n };
        let empty = doc.first_child(content).is_none();
        if html && empty && VOID.contains(&local) {
            self.out.push_str(" />");
            return Ok(());
        }
        if !html && empty {
            self.out.push_str("/>");
            return Ok(());
        }
        self.out.push('>');
        for c in doc.children(content) {
            self.node(c, inherited.as_deref(), &mut map)?;
        }
        self.out.push_str("</");
        self.out.push_str(&qualified);
        self.out.push('>');
        Ok(())
    }
}

impl Document {
    /// The XML serialization of node `id` (DOM Parsing "produce an XML
    /// serialization"); `Err` when `require_well_formed` and the node
    /// cannot be serialized as well-formed XML.
    pub fn serialize_xml(&self, id: NodeId, require_well_formed: bool) -> Result<String, String> {
        let mut s = Ser { doc: self, wf: require_well_formed, prefix_index: 1, out: String::new() };
        let mut map = PrefixMap::default();
        map.add(Some(XML_NS), "xml");
        s.node(id, None, &mut map)?;
        Ok(s.out)
    }

    /// The XML serializations of `id`'s children, concatenated (`innerHTML`
    /// of an element of an XML document).
    pub fn serialize_xml_children(&self, id: NodeId, require_well_formed: bool) -> Result<String, String> {
        let id = match self.element(id) {
            Some(e) if e.tag == "template" && e.ns == Namespace::Html => self.template_contents.get(&id).copied().unwrap_or(id),
            _ => id,
        };
        let mut out = String::new();
        for c in self.children(id) {
            out.push_str(&self.serialize_xml(c, require_well_formed)?);
        }
        Ok(out)
    }
}
