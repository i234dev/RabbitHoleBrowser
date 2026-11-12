//! A namespace-aware XML parser for XML documents (`application/xml`,
//! XHTML, SVG documents, `DOMParser` with XML types). Well-formedness
//! errors produce a document whose root is a `parsererror` element, as
//! browsers do.

use rabbithole_dom::{DoctypeData, Document, Namespace, NodeData, NodeId};

const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

struct Parser<'a> {
    s: &'a [u8],
    src: &'a str,
    pos: usize,
    doc: Document,
    /// Namespace scopes: (prefix, uri) declarations per open element.
    scopes: Vec<Vec<(String, String)>>,
}

type R<T> = Result<T, String>;

fn is_name_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == ':' || (c as u32) >= 0xC0
}

fn is_name_char(c: char) -> bool {
    is_name_start(c) || c.is_ascii_digit() || c == '-' || c == '.' || c as u32 == 0xB7
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    fn starts(&self, t: &str) -> bool {
        self.s[self.pos..].starts_with(t.as_bytes())
    }

    fn err<T>(&self, msg: &str) -> R<T> {
        let line = self.src[..self.pos.min(self.src.len())].matches('\n').count() + 1;
        Err(format!("error on line {line}: {msg}"))
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn name(&mut self) -> R<String> {
        let rest = &self.src[self.pos..];
        let mut end = 0;
        for (i, c) in rest.char_indices() {
            let ok = if i == 0 { is_name_start(c) } else { is_name_char(c) };
            if !ok {
                break;
            }
            end = i + c.len_utf8();
        }
        if end == 0 {
            return self.err("expected a name");
        }
        self.pos += end;
        Ok(rest[..end].to_string())
    }

    fn expect(&mut self, t: &str) -> R<()> {
        if self.starts(t) {
            self.pos += t.len();
            Ok(())
        } else {
            self.err(&format!("expected '{t}'"))
        }
    }

    /// Text with entity and character references expanded.
    fn decode(&self, raw: &str) -> R<String> {
        if !raw.contains('&') {
            return Ok(raw.to_string());
        }
        let mut out = String::with_capacity(raw.len());
        let mut rest = raw;
        while let Some(i) = rest.find('&') {
            out.push_str(&rest[..i]);
            let after = &rest[i + 1..];
            let Some(j) = after.find(';') else { return self.err("unterminated entity reference") };
            let ent = &after[..j];
            let c = match ent {
                "lt" => "<".to_string(),
                "gt" => ">".to_string(),
                "amp" => "&".to_string(),
                "apos" => "'".to_string(),
                "quot" => "\"".to_string(),
                _ if ent.starts_with("#x") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32).map(|c| c.to_string()).ok_or("bad character reference".to_string())?,
                _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32).map(|c| c.to_string()).ok_or("bad character reference".to_string())?,
                // XHTML documents commonly use HTML named references.
                _ => match crate::entities::lookup(ent) {
                    Some(v) => v.to_string(),
                    None => return self.err(&format!("undefined entity '{ent}'")),
                },
            };
            out.push_str(&c);
            rest = &after[j + 1..];
        }
        out.push_str(rest);
        Ok(out)
    }

    fn lookup_ns(&self, prefix: &str) -> Option<String> {
        if prefix == "xml" {
            return Some(XML_NS.into());
        }
        if prefix == "xmlns" {
            return Some(XMLNS_NS.into());
        }
        for scope in self.scopes.iter().rev() {
            for (p, u) in scope.iter().rev() {
                if p == prefix {
                    return if u.is_empty() { None } else { Some(u.clone()) };
                }
            }
        }
        None
    }

    fn append_text(&mut self, parent: NodeId, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.doc.last_child(parent) {
            if let NodeData::Text(t) = &mut self.doc.node_mut(last).data {
                t.push_str(text);
                return;
            }
        }
        let t = self.doc.create_text(text);
        self.doc.append_child(parent, t);
    }

    fn misc(&mut self, parent: NodeId) -> R<bool> {
        if self.starts("<!--") {
            self.pos += 4;
            let Some(end) = self.src[self.pos..].find("-->") else { return self.err("unterminated comment") };
            let c = self.src[self.pos..self.pos + end].to_string();
            self.pos += end + 3;
            let n = self.doc.create(NodeData::Comment(c));
            self.doc.append_child(parent, n);
            return Ok(true);
        }
        if self.starts("<?") {
            // Processing instructions (and the XML declaration) are skipped.
            let Some(end) = self.src[self.pos..].find("?>") else { return self.err("unterminated processing instruction") };
            self.pos += end + 2;
            return Ok(true);
        }
        Ok(false)
    }

    fn doctype(&mut self) -> R<()> {
        self.pos += "<!DOCTYPE".len();
        self.skip_ws();
        let name = self.name()?;
        self.skip_ws();
        let mut public_id = String::new();
        let mut system_id = String::new();
        let quoted = |p: &mut Parser| -> R<String> {
            p.skip_ws();
            let q = p.peek().ok_or("unexpected end")?;
            if q != b'"' && q != b'\'' {
                return p.err("expected a quoted literal");
            }
            p.pos += 1;
            let rest = &p.src[p.pos..];
            let Some(end) = rest.find(q as char) else { return p.err("unterminated literal") };
            let v = rest[..end].to_string();
            p.pos += end + 1;
            Ok(v)
        };
        if self.starts("PUBLIC") {
            self.pos += 6;
            public_id = quoted(self)?;
            self.skip_ws();
            if matches!(self.peek(), Some(b'"' | b'\'')) {
                system_id = quoted(self)?;
            }
        } else if self.starts("SYSTEM") {
            self.pos += 6;
            system_id = quoted(self)?;
        }
        self.skip_ws();
        if self.peek() == Some(b'[') {
            // Internal subset: skipped (bracket matching, ignoring quotes).
            let mut depth = 0;
            while let Some(b) = self.peek() {
                self.pos += 1;
                match b {
                    b'[' => depth += 1,
                    b']' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            self.skip_ws();
        }
        self.expect(">")?;
        let n = self.doc.create(NodeData::Doctype(DoctypeData { name, public_id, system_id }));
        self.doc.append_child(Document::ROOT, n);
        Ok(())
    }

    fn element(&mut self, parent: NodeId) -> R<()> {
        self.expect("<")?;
        let qname = self.name()?;
        let mut raw_attrs: Vec<(String, String)> = Vec::new();
        loop {
            let before = self.pos;
            self.skip_ws();
            match self.peek() {
                Some(b'>') | Some(b'/') => break,
                None => return self.err("unexpected end of input in a start tag"),
                _ => {}
            }
            if self.pos == before {
                return self.err("expected whitespace between attributes");
            }
            let an = self.name()?;
            self.skip_ws();
            self.expect("=")?;
            self.skip_ws();
            let q = self.peek().ok_or("unexpected end")?;
            if q != b'"' && q != b'\'' {
                return self.err("attribute values must be quoted");
            }
            self.pos += 1;
            let rest = &self.src[self.pos..];
            let Some(end) = rest.find(q as char) else { return self.err("unterminated attribute value") };
            let raw = &rest[..end];
            if raw.contains('<') {
                return self.err("'<' in an attribute value");
            }
            let v = self.decode(raw)?.replace(['\t', '\n', '\r'], " ");
            self.pos += end + 1;
            if raw_attrs.iter().any(|(n, _)| *n == an) {
                return self.err(&format!("duplicate attribute '{an}'"));
            }
            raw_attrs.push((an, v));
        }
        // Namespace declarations of this element.
        let mut scope = Vec::new();
        for (n, v) in &raw_attrs {
            if n == "xmlns" {
                scope.push((String::new(), v.clone()));
            } else if let Some(p) = n.strip_prefix("xmlns:") {
                scope.push((p.to_string(), v.clone()));
            }
        }
        self.scopes.push(scope);
        let (prefix, local) = match qname.split_once(':') {
            Some((p, l)) => (Some(p.to_string()), l.to_string()),
            None => (None, qname.clone()),
        };
        let ns_uri = self.lookup_ns(prefix.as_deref().unwrap_or(""));
        if prefix.is_some() && ns_uri.is_none() {
            return self.err(&format!("namespace prefix '{}' is not defined", prefix.unwrap_or_default()));
        }
        let ns = Namespace::from_uri(ns_uri.as_deref());
        // Attribute namespaces: prefixed names resolve through the scopes
        // (unprefixed attributes have no namespace); xmlns declarations
        // are in the XMLNS namespace.
        let mut attr_ns = Vec::new();
        for (n, _) in &raw_attrs {
            if n == "xmlns" || n.starts_with("xmlns:") {
                attr_ns.push((n.clone(), XMLNS_NS.to_string()));
            } else if let Some((p, _)) = n.split_once(':') {
                match self.lookup_ns(p) {
                    Some(u) => attr_ns.push((n.clone(), u)),
                    None => return self.err(&format!("namespace prefix '{p}' is not defined")),
                }
            }
        }
        let el = self.doc.create_element_ns(ns, prefix, &local, raw_attrs);
        for (n, u) in attr_ns {
            self.doc.set_attr_namespace(el, &n, Some(&u));
        }
        self.doc.append_child(parent, el);
        if self.starts("/>") {
            self.pos += 2;
            self.scopes.pop();
            return Ok(());
        }
        self.expect(">")?;
        self.content(el)?;
        self.expect("</")?;
        let end = self.name()?;
        if end != qname {
            return self.err(&format!("mismatched end tag: expected '{qname}', found '{end}'"));
        }
        self.skip_ws();
        self.expect(">")?;
        self.scopes.pop();
        Ok(())
    }

    fn content(&mut self, parent: NodeId) -> R<()> {
        loop {
            if self.pos >= self.s.len() {
                return self.err("unexpected end of input");
            }
            if self.starts("</") {
                return Ok(());
            }
            if self.starts("<![CDATA[") {
                self.pos += 9;
                let Some(end) = self.src[self.pos..].find("]]>") else { return self.err("unterminated CDATA section") };
                let t = self.src[self.pos..self.pos + end].to_string();
                self.pos += end + 3;
                self.append_text(parent, &t);
                continue;
            }
            if self.misc(parent)? {
                continue;
            }
            if self.peek() == Some(b'<') {
                self.element(parent)?;
                continue;
            }
            let rest = &self.src[self.pos..];
            let end = rest.find('<').unwrap_or(rest.len());
            let raw = &rest[..end];
            if raw.contains("]]>") {
                return self.err("']]>' in text");
            }
            let text = self.decode(raw)?;
            self.pos += end;
            self.append_text(parent, &text);
        }
    }

    fn document(&mut self) -> R<()> {
        // Byte order mark.
        if self.starts("\u{feff}") {
            self.pos += 3;
        }
        let mut seen_root = false;
        loop {
            self.skip_ws();
            if self.pos >= self.s.len() {
                break;
            }
            if self.misc(Document::ROOT)? {
                continue;
            }
            if self.starts("<!DOCTYPE") {
                if seen_root {
                    return self.err("DOCTYPE after the root element");
                }
                self.doctype()?;
                continue;
            }
            if self.peek() == Some(b'<') && !seen_root {
                self.element(Document::ROOT)?;
                seen_root = true;
                continue;
            }
            return self.err("content outside the root element");
        }
        if !seen_root {
            return self.err("no root element");
        }
        Ok(())
    }
}

/// Parse an XML document. On a well-formedness error the document holds a
/// `parsererror` element (in the Mozilla parsererror namespace) with the
/// message, as `DOMParser` does.
pub fn parse_xml(input: &str) -> Document {
    match try_parse_xml(input) {
        Ok(d) => d,
        Err(msg) => parser_error_document(&msg),
    }
}

/// Parse an XML document, or the well-formedness error message.
pub fn try_parse_xml(input: &str) -> Result<Document, String> {
    // End-of-line handling (XML 1.0 §2.11): CR LF and lone CR become LF.
    let normalized;
    let input = if input.contains('\r') {
        normalized = input.replace("\r\n", "\n").replace('\r', "\n");
        normalized.as_str()
    } else {
        input
    };
    let mut p = Parser { s: input.as_bytes(), src: input, pos: 0, doc: Document::new(), scopes: Vec::new() };
    p.doc.is_html = false;
    p.doc.content_type = "application/xml".to_string();
    p.document()?;
    Ok(p.doc)
}

/// The document returned for malformed XML.
pub fn parser_error_document(msg: &str) -> Document {
    let mut doc = Document::new();
    doc.is_html = false;
    doc.content_type = "application/xml".to_string();
    let el = doc.create_element_ns(Namespace::Other("http://www.mozilla.org/newlayout/xml/parsererror.xml".into()), None, "parsererror", vec![]);
    let t = doc.create_text(&format!("XML Parsing Error: {msg}"));
    doc.append_child(el, t);
    doc.append_child(Document::ROOT, el);
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_and_text() {
        let d = try_parse_xml("<?xml version='1.0'?><!DOCTYPE html><html xmlns='http://www.w3.org/1999/xhtml'><body><p a='1&amp;2'>x &lt; y<![CDATA[<z>]]></p><svg:svg xmlns:svg='http://www.w3.org/2000/svg'/></body></html>").unwrap();
        let p = d.descendants(Document::ROOT).find(|&n| d.tag(n) == Some("p")).unwrap();
        assert_eq!(d.element(p).unwrap().ns, Namespace::Html);
        assert_eq!(d.attr(p, "a"), Some("1&2"));
        assert_eq!(d.text_content(p), "x < y<z>");
        let svg = d.descendants(Document::ROOT).find(|&n| d.tag(n) == Some("svg")).unwrap();
        assert_eq!(d.element(svg).unwrap().ns, Namespace::Svg);
        assert!(d.doctype().is_some());
    }

    #[test]
    fn errors() {
        assert!(try_parse_xml("<a><b></a>").is_err());
        assert!(try_parse_xml("<a x=1/>").is_err());
        assert!(try_parse_xml("").is_err());
        assert!(try_parse_xml("<a/><b/>").is_err());
        let d = parse_xml("<a>");
        let root = d.first_child(Document::ROOT).unwrap();
        assert_eq!(d.tag(root), Some("parsererror"));
    }
}
