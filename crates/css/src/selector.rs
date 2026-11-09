//! Selector parsing and matching (Selectors Level 3 subset, plus `:not()`,
//! `:is()`/`:where()`, `:nth-child()` and attribute operators).

use super::tokenizer::Token;
use rabbithole_dom::{Document, NodeData, NodeId};

#[derive(Debug, Clone, PartialEq)]
pub enum AttrOp {
    Exists,
    Equals,
    Includes,   // ~=
    DashMatch,  // |=
    Prefix,     // ^=
    Suffix,     // $=
    Substring,  // *=
}

#[derive(Debug, Clone, PartialEq)]
pub struct AttrSelector {
    pub name: String,
    pub op: AttrOp,
    pub value: String,
    pub case_insensitive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pseudo {
    Root,
    FirstChild,
    LastChild,
    OnlyChild,
    FirstOfType,
    LastOfType,
    OnlyOfType,
    Empty,
    Link,
    AnyLink,
    NthChild(i32, i32),
    NthLastChild(i32, i32),
    NthOfType(i32, i32),
    Not(Vec<Selector>),
    Is(Vec<Selector>),
    Where(Vec<Selector>),
    /// Something we never match (:hover, :focus, :visited, ::before, ...)
    Never,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Compound {
    pub tag: Option<String>,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub attrs: Vec<AttrSelector>,
    pub pseudos: Vec<Pseudo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combinator {
    Descendant,
    Child,
    NextSibling,
    SubsequentSibling,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Selector {
    /// Left-to-right: compounds[0] is leftmost. combinators[i] links
    /// compounds[i] and compounds[i+1].
    pub compounds: Vec<Compound>,
    pub combinators: Vec<Combinator>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Specificity(pub u32, pub u32, pub u32);

impl Selector {
    pub fn specificity(&self) -> Specificity {
        let mut s = Specificity(0, 0, 0);
        for c in &self.compounds {
            add_compound_specificity(c, &mut s);
        }
        s
    }
}

fn add_compound_specificity(c: &Compound, s: &mut Specificity) {
    if c.id.is_some() {
        s.0 += 1;
    }
    s.1 += c.classes.len() as u32 + c.attrs.len() as u32;
    if let Some(t) = &c.tag {
        if t != "*" {
            s.2 += 1;
        }
    }
    for p in &c.pseudos {
        match p {
            Pseudo::Not(list) | Pseudo::Is(list) => {
                if let Some(m) = list.iter().map(|x| x.specificity()).max() {
                    s.0 += m.0;
                    s.1 += m.1;
                    s.2 += m.2;
                }
            }
            Pseudo::Where(_) => {}
            Pseudo::Never => s.1 += 1,
            _ => s.1 += 1,
        }
    }
}

// ---------------------------------------------------------------- parsing

pub struct SelectorParser<'a> {
    toks: &'a [Token],
    pos: usize,
}

impl<'a> SelectorParser<'a> {
    pub fn new(toks: &'a [Token]) -> Self {
        SelectorParser { toks, pos: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.toks.get(self.pos)
    }

    fn bump(&mut self) -> Option<&Token> {
        let t = self.toks.get(self.pos);
        self.pos += 1;
        t
    }

    fn skip_ws(&mut self) -> bool {
        let mut saw = false;
        while self.peek() == Some(&Token::Whitespace) {
            self.pos += 1;
            saw = true;
        }
        saw
    }

    /// Parse a comma separated selector list. Returns None if any selector
    /// is invalid (the whole rule is then dropped, as the spec requires).
    pub fn parse_list(&mut self) -> Option<Vec<Selector>> {
        let mut list = Vec::new();
        loop {
            self.skip_ws();
            let sel = self.parse_complex()?;
            list.push(sel);
            self.skip_ws();
            match self.peek() {
                Some(Token::Comma) => {
                    self.pos += 1;
                }
                None => return Some(list),
                _ => return None,
            }
        }
    }

    fn parse_complex(&mut self) -> Option<Selector> {
        let mut compounds = vec![self.parse_compound()?];
        let mut combinators = Vec::new();
        loop {
            let saw_ws = self.skip_ws();
            let comb = match self.peek() {
                Some(Token::Delim('>')) => {
                    self.pos += 1;
                    Combinator::Child
                }
                Some(Token::Delim('+')) => {
                    self.pos += 1;
                    Combinator::NextSibling
                }
                Some(Token::Delim('~')) => {
                    self.pos += 1;
                    Combinator::SubsequentSibling
                }
                Some(Token::Comma) | Some(Token::CloseParen) | None => break,
                Some(_) if saw_ws => Combinator::Descendant,
                _ => return None,
            };
            self.skip_ws();
            let c = self.parse_compound()?;
            compounds.push(c);
            combinators.push(comb);
        }
        Some(Selector {
            compounds,
            combinators,
        })
    }

    fn parse_compound(&mut self) -> Option<Compound> {
        let mut c = Compound::default();
        let mut any = false;
        // Type / universal
        match self.peek() {
            Some(Token::Ident(name)) => {
                c.tag = Some(name.to_ascii_lowercase());
                self.pos += 1;
                any = true;
            }
            Some(Token::Delim('*')) => {
                c.tag = Some("*".into());
                self.pos += 1;
                any = true;
            }
            _ => {}
        }
        // Namespace prefix `*|` or `svg|` – accept and ignore
        if self.peek() == Some(&Token::Delim('|')) {
            self.pos += 1;
            match self.bump() {
                Some(Token::Ident(name)) => c.tag = Some(name.to_ascii_lowercase()),
                Some(Token::Delim('*')) => c.tag = Some("*".into()),
                _ => return None,
            }
        }
        loop {
            match self.peek() {
                Some(Token::Hash(id)) => {
                    c.id = Some(id.clone());
                    self.pos += 1;
                }
                Some(Token::Delim('.')) => {
                    self.pos += 1;
                    match self.bump() {
                        Some(Token::Ident(cls)) => c.classes.push(cls.clone()),
                        _ => return None,
                    }
                }
                Some(Token::OpenBracket) => {
                    self.pos += 1;
                    c.attrs.push(self.parse_attr()?);
                }
                Some(Token::Colon) => {
                    self.pos += 1;
                    let mut pseudo_element = false;
                    if self.peek() == Some(&Token::Colon) {
                        self.pos += 1;
                        pseudo_element = true;
                    }
                    let p = self.parse_pseudo(pseudo_element)?;
                    c.pseudos.push(p);
                }
                _ => break,
            }
            any = true;
        }
        if !any {
            return None;
        }
        Some(c)
    }

    fn parse_attr(&mut self) -> Option<AttrSelector> {
        self.skip_ws();
        let name = match self.bump() {
            Some(Token::Ident(n)) => n.to_ascii_lowercase(),
            _ => return None,
        };
        self.skip_ws();
        let op = match self.peek() {
            Some(Token::CloseBracket) => {
                self.pos += 1;
                return Some(AttrSelector {
                    name,
                    op: AttrOp::Exists,
                    value: String::new(),
                    case_insensitive: false,
                });
            }
            Some(Token::Delim('=')) => {
                self.pos += 1;
                AttrOp::Equals
            }
            Some(Token::Delim(ch)) => {
                let ch = *ch;
                self.pos += 1;
                if self.bump() != Some(&Token::Delim('=')) {
                    return None;
                }
                match ch {
                    '~' => AttrOp::Includes,
                    '|' => AttrOp::DashMatch,
                    '^' => AttrOp::Prefix,
                    '$' => AttrOp::Suffix,
                    '*' => AttrOp::Substring,
                    _ => return None,
                }
            }
            _ => return None,
        };
        self.skip_ws();
        let value = match self.bump() {
            Some(Token::Ident(v)) | Some(Token::String(v)) => v.clone(),
            _ => return None,
        };
        self.skip_ws();
        let mut ci = false;
        if let Some(Token::Ident(flag)) = self.peek() {
            ci = flag.eq_ignore_ascii_case("i");
            self.pos += 1;
            self.skip_ws();
        }
        if self.bump() != Some(&Token::CloseBracket) {
            return None;
        }
        Some(AttrSelector {
            name,
            op,
            value,
            case_insensitive: ci,
        })
    }

    fn parse_pseudo(&mut self, pseudo_element: bool) -> Option<Pseudo> {
        match self.bump().cloned() {
            Some(Token::Ident(name)) => {
                if pseudo_element {
                    return Some(Pseudo::Never);
                }
                Some(match name.to_ascii_lowercase().as_str() {
                    "root" => Pseudo::Root,
                    "first-child" => Pseudo::FirstChild,
                    "last-child" => Pseudo::LastChild,
                    "only-child" => Pseudo::OnlyChild,
                    "first-of-type" => Pseudo::FirstOfType,
                    "last-of-type" => Pseudo::LastOfType,
                    "only-of-type" => Pseudo::OnlyOfType,
                    "empty" => Pseudo::Empty,
                    "link" => Pseudo::Link,
                    "any-link" => Pseudo::AnyLink,
                    "before" | "after" | "first-line" | "first-letter" => Pseudo::Never,
                    _ => Pseudo::Never,
                })
            }
            Some(Token::Function(name)) => {
                let lname = name.to_ascii_lowercase();
                // Collect tokens until the matching close paren.
                let start = self.pos;
                let mut depth = 1;
                while let Some(t) = self.peek() {
                    match t {
                        Token::Function(_) | Token::OpenParen => depth += 1,
                        Token::CloseParen => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    self.pos += 1;
                }
                let inner = &self.toks[start..self.pos];
                if self.peek() == Some(&Token::CloseParen) {
                    self.pos += 1;
                }
                if pseudo_element {
                    return Some(Pseudo::Never);
                }
                Some(match lname.as_str() {
                    "not" | "is" | "where" | "matches" | "-webkit-any" | "-moz-any" => {
                        let mut sub = SelectorParser::new(inner);
                        // Forgiving selector list: drop invalid parts.
                        let list = sub.parse_list_forgiving();
                        match lname.as_str() {
                            "not" => Pseudo::Not(list),
                            "where" => Pseudo::Where(list),
                            _ => Pseudo::Is(list),
                        }
                    }
                    "nth-child" | "nth-last-child" | "nth-of-type" => {
                        let (a, b) = parse_nth(inner)?;
                        match lname.as_str() {
                            "nth-child" => Pseudo::NthChild(a, b),
                            "nth-last-child" => Pseudo::NthLastChild(a, b),
                            _ => Pseudo::NthOfType(a, b),
                        }
                    }
                    _ => Pseudo::Never,
                })
            }
            _ => None,
        }
    }

    fn parse_list_forgiving(&mut self) -> Vec<Selector> {
        let mut list = Vec::new();
        loop {
            self.skip_ws();
            if self.peek().is_none() {
                break;
            }
            match self.parse_complex() {
                Some(s) => list.push(s),
                None => {
                    // skip to next comma
                    while let Some(t) = self.peek() {
                        if *t == Token::Comma {
                            break;
                        }
                        self.pos += 1;
                    }
                }
            }
            self.skip_ws();
            if self.peek() == Some(&Token::Comma) {
                self.pos += 1;
            } else {
                break;
            }
        }
        list
    }
}

/// Parse `an+b` micro-syntax from tokens.
fn parse_nth(toks: &[Token]) -> Option<(i32, i32)> {
    let mut s = String::new();
    for t in toks {
        match t {
            Token::Ident(i) => s.push_str(i),
            Token::Number(n) => s.push_str(&format!("{}", *n as i32)),
            Token::Dimension(n, u) => s.push_str(&format!("{}{}", *n as i32, u)),
            Token::Delim(c) => s.push(*c),
            Token::Whitespace => {}
            _ => return None,
        }
    }
    let s = s.to_ascii_lowercase();
    match s.as_str() {
        "odd" => return Some((2, 1)),
        "even" => return Some((2, 0)),
        _ => {}
    }
    if let Some(npos) = s.find('n') {
        let a_str = &s[..npos];
        let a = match a_str {
            "" | "+" => 1,
            "-" => -1,
            _ => a_str.parse().ok()?,
        };
        let b_str = &s[npos + 1..];
        let b = if b_str.is_empty() { 0 } else { b_str.parse().ok()? };
        Some((a, b))
    } else {
        Some((0, s.parse().ok()?))
    }
}

fn nth_matches(a: i32, b: i32, index: i32) -> bool {
    if a == 0 {
        return index == b;
    }
    let diff = index - b;
    diff % a == 0 && diff / a >= 0
}

// --------------------------------------------------------------- matching

pub fn matches(sel: &Selector, doc: &Document, node: NodeId) -> bool {
    let n = sel.compounds.len();
    match_from(sel, n - 1, doc, node)
}

fn match_from(sel: &Selector, idx: usize, doc: &Document, node: NodeId) -> bool {
    if !match_compound(&sel.compounds[idx], doc, node) {
        return false;
    }
    if idx == 0 {
        return true;
    }
    let comb = sel.combinators[idx - 1];
    match comb {
        Combinator::Child => match element_parent(doc, node) {
            Some(p) => match_from(sel, idx - 1, doc, p),
            None => false,
        },
        Combinator::Descendant => {
            let mut cur = element_parent(doc, node);
            while let Some(p) = cur {
                if match_from(sel, idx - 1, doc, p) {
                    return true;
                }
                cur = element_parent(doc, p);
            }
            false
        }
        Combinator::NextSibling => match prev_element_sibling(doc, node) {
            Some(s) => match_from(sel, idx - 1, doc, s),
            None => false,
        },
        Combinator::SubsequentSibling => {
            let mut cur = prev_element_sibling(doc, node);
            while let Some(s) = cur {
                if match_from(sel, idx - 1, doc, s) {
                    return true;
                }
                cur = prev_element_sibling(doc, s);
            }
            false
        }
    }
}

fn element_parent(doc: &Document, node: NodeId) -> Option<NodeId> {
    let p = doc.parent(node)?;
    if doc.element(p).is_some() {
        Some(p)
    } else {
        None
    }
}

fn prev_element_sibling(doc: &Document, node: NodeId) -> Option<NodeId> {
    let mut cur = doc.node(node).prev_sibling;
    while let Some(s) = cur {
        if doc.element(s).is_some() {
            return Some(s);
        }
        cur = doc.node(s).prev_sibling;
    }
    None
}

fn next_element_sibling(doc: &Document, node: NodeId) -> Option<NodeId> {
    let mut cur = doc.node(node).next_sibling;
    while let Some(s) = cur {
        if doc.element(s).is_some() {
            return Some(s);
        }
        cur = doc.node(s).next_sibling;
    }
    None
}

fn child_index(doc: &Document, node: NodeId, of_type: bool, from_end: bool) -> i32 {
    let tag = doc.tag(node);
    let mut i = 1;
    let mut cur = if from_end { next_element_sibling(doc, node) } else { prev_element_sibling(doc, node) };
    while let Some(s) = cur {
        if !of_type || doc.tag(s) == tag {
            i += 1;
        }
        cur = if from_end { next_element_sibling(doc, s) } else { prev_element_sibling(doc, s) };
    }
    i
}

pub fn match_compound(c: &Compound, doc: &Document, node: NodeId) -> bool {
    let el = match doc.element(node) {
        Some(e) => e,
        None => return false,
    };
    if let Some(t) = &c.tag {
        if t != "*" && *t != el.tag {
            return false;
        }
    }
    if let Some(id) = &c.id {
        if el.id() != Some(id.as_str()) {
            return false;
        }
    }
    if !c.classes.is_empty() {
        let class_attr = match el.attr("class") {
            Some(v) => v,
            None => return false,
        };
        for cls in &c.classes {
            if !class_attr.split_ascii_whitespace().any(|x| x == cls) {
                return false;
            }
        }
    }
    for a in &c.attrs {
        let v = match el.attr(&a.name) {
            Some(v) => v,
            None => return false,
        };
        let (v, want) = if a.case_insensitive {
            (v.to_ascii_lowercase(), a.value.to_ascii_lowercase())
        } else {
            (v.to_string(), a.value.clone())
        };
        let ok = match a.op {
            AttrOp::Exists => true,
            AttrOp::Equals => v == want,
            AttrOp::Includes => v.split_ascii_whitespace().any(|x| x == want),
            AttrOp::DashMatch => v == want || v.starts_with(&format!("{want}-")),
            AttrOp::Prefix => !want.is_empty() && v.starts_with(&want),
            AttrOp::Suffix => !want.is_empty() && v.ends_with(&want),
            AttrOp::Substring => !want.is_empty() && v.contains(&want),
        };
        if !ok {
            return false;
        }
    }
    for p in &c.pseudos {
        let ok = match p {
            Pseudo::Root => el.tag == "html",
            Pseudo::FirstChild => prev_element_sibling(doc, node).is_none(),
            Pseudo::LastChild => next_element_sibling(doc, node).is_none(),
            Pseudo::OnlyChild => prev_element_sibling(doc, node).is_none() && next_element_sibling(doc, node).is_none(),
            Pseudo::FirstOfType => child_index(doc, node, true, false) == 1,
            Pseudo::LastOfType => child_index(doc, node, true, true) == 1,
            Pseudo::OnlyOfType => child_index(doc, node, true, false) == 1 && child_index(doc, node, true, true) == 1,
            Pseudo::Empty => doc.children(node).all(|ch| matches!(&doc.node(ch).data, NodeData::Comment(_))),
            Pseudo::Link | Pseudo::AnyLink => matches!(el.tag.as_str(), "a" | "area") && el.attr("href").is_some(),
            Pseudo::NthChild(a, b) => nth_matches(*a, *b, child_index(doc, node, false, false)),
            Pseudo::NthLastChild(a, b) => nth_matches(*a, *b, child_index(doc, node, false, true)),
            Pseudo::NthOfType(a, b) => nth_matches(*a, *b, child_index(doc, node, true, false)),
            Pseudo::Not(list) => !list.iter().any(|s| matches(s, doc, node)),
            Pseudo::Is(list) | Pseudo::Where(list) => list.iter().any(|s| matches(s, doc, node)),
            Pseudo::Never => false,
        };
        if !ok {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokenizer::Tokenizer;

    fn parse(s: &str) -> Vec<Selector> {
        let toks = Tokenizer::tokenize(s);
        SelectorParser::new(&toks).parse_list().expect("valid selector")
    }

    #[test]
    fn parse_and_specificity() {
        let sels = parse("div#main > p.intro.big a[href^='http'], *:not(.x)");
        assert_eq!(sels.len(), 2);
        assert_eq!(sels[0].specificity(), Specificity(1, 3, 3));
        assert_eq!(sels[1].specificity(), Specificity(0, 1, 0));
    }

    #[test]
    fn matching() {
        let doc = rabbithole_html::parse("<div id=main><p class='intro big'><a href='http://x'>l</a></p><p>second</p></div>");
        let a = doc.find_element("a").unwrap();
        let sel = parse("div#main > p.intro.big a[href^='http']");
        assert!(matches(&sel[0], &doc, a));
        let ps: Vec<_> = doc.descendants(0).filter(|&n| doc.is_element(n, "p")).collect();
        assert!(matches(&parse("p:first-child")[0], &doc, ps[0]));
        assert!(!matches(&parse("p:first-child")[0], &doc, ps[1]));
        assert!(matches(&parse("p:nth-child(2n)")[0], &doc, ps[1]));
        assert!(matches(&parse("p + p")[0], &doc, ps[1]));
        assert!(matches(&parse("#main p:not(.intro)")[0], &doc, ps[1]));
        assert!(!matches(&parse("p:hover")[0], &doc, ps[1]));
    }
}
