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
    NthLastOfType(i32, i32),
    /// `:has(> a, b)` relative selectors.
    Has(Vec<(Combinator, Selector)>),
    Lang(String),
    Enabled,
    Disabled,
    Checked,
    /// Dynamic states; matched against `MatchState` (never when unset).
    Hover,
    Active,
    /// `:focus`: the focused element only.
    Focus,
    /// `:focus-visible`: the focused element when the focus should be
    /// indicated (keyboard focus, text-entry controls).
    FocusVisible,
    /// `:focus-within`: the focused element and its ancestors.
    FocusWithin,
    Visited,
    Target,
    /// `:host` / `:host(<compound>)`: matches a shadow host (from rules of
    /// its shadow tree). An empty list is the bare `:host`.
    Host(Vec<Selector>),
    /// `:host-context(<compound>)`: a shadow host with a (shadow-including)
    /// inclusive ancestor matching the argument.
    HostContext(Vec<Selector>),
    /// `::slotted(<compound>)` on the compound that selects the slot. The
    /// cascade matches these rules against slotted elements; ordinary
    /// matching never matches them.
    Slotted(Vec<Selector>),
    /// `:defined` (custom elements once upgraded; every other element).
    Defined,
    /// Something we never match (unknown pseudo-classes/elements).
    Never,
}

/// Result of parsing one `:`/`::` suffix.
enum PseudoParse {
    Class(Pseudo),
    Element(PseudoElement),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Compound {
    pub tag: Option<String>,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub attrs: Vec<AttrSelector>,
    pub pseudos: Vec<Pseudo>,
    /// Set while parsing when a pseudo-element follows this compound; moved
    /// to `Selector::pseudo_element` (only valid on the last compound).
    pub pseudo_element: Option<PseudoElement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combinator {
    Descendant,
    Child,
    NextSibling,
    SubsequentSibling,
}

/// Pseudo-elements a selector can target. `matches()` only checks the
/// element part; the cascade applies rules with a pseudo-element to that
/// pseudo-element's style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PseudoElement {
    Before,
    After,
    Marker,
    FirstLine,
    FirstLetter,
    Placeholder,
    Selection,
    Backdrop,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Selector {
    /// Left-to-right: compounds[0] is leftmost. combinators[i] links
    /// compounds[i] and compounds[i+1].
    pub compounds: Vec<Compound>,
    pub combinators: Vec<Combinator>,
    /// `::before`, `::after`, ... attached to the rightmost compound.
    pub pseudo_element: Option<PseudoElement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Specificity(pub u32, pub u32, pub u32);

impl Selector {
    pub fn specificity(&self) -> Specificity {
        let mut s = Specificity(0, 0, 0);
        for c in &self.compounds {
            add_compound_specificity(c, &mut s);
        }
        if self.pseudo_element.is_some() {
            s.2 += 1;
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
            Pseudo::Has(list) => {
                if let Some(m) = list.iter().map(|(_, x)| x.specificity()).max() {
                    s.0 += m.0;
                    s.1 += m.1;
                    s.2 += m.2;
                }
            }
            Pseudo::Not(list) | Pseudo::Is(list) | Pseudo::Host(list) | Pseudo::HostContext(list) | Pseudo::Slotted(list) => {
                if !matches!(p, Pseudo::Not(_) | Pseudo::Is(_)) {
                    // :host / ::slotted count as a pseudo-class / -element.
                    if matches!(p, Pseudo::Slotted(_)) {
                        s.2 += 1;
                    } else {
                        s.1 += 1;
                    }
                }
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
        // A pseudo-element is only allowed on the rightmost compound.
        let n = compounds.len();
        for (i, c) in compounds.iter().enumerate() {
            if c.pseudo_element.is_some() && i != n - 1 {
                return None;
            }
        }
        let pseudo_element = compounds.last_mut().and_then(|c| c.pseudo_element.take());
        Some(Selector {
            compounds,
            combinators,
            pseudo_element,
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
                    match self.parse_pseudo(pseudo_element)? {
                        PseudoParse::Class(p) => c.pseudos.push(p),
                        PseudoParse::Element(e) => {
                            if c.pseudo_element.is_some() {
                                return None;
                            }
                            c.pseudo_element = Some(e);
                        }
                    }
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

    fn parse_pseudo(&mut self, pseudo_element: bool) -> Option<PseudoParse> {
        match self.bump().cloned() {
            Some(Token::Ident(name)) => {
                let lname = name.to_ascii_lowercase();
                let element = match lname.as_str() {
                    "before" => Some(PseudoElement::Before),
                    "after" => Some(PseudoElement::After),
                    "marker" => Some(PseudoElement::Marker),
                    "first-line" => Some(PseudoElement::FirstLine),
                    "first-letter" => Some(PseudoElement::FirstLetter),
                    "placeholder" | "-webkit-input-placeholder" | "-moz-placeholder" => Some(PseudoElement::Placeholder),
                    "selection" | "-moz-selection" => Some(PseudoElement::Selection),
                    "backdrop" => Some(PseudoElement::Backdrop),
                    _ => None,
                };
                if let Some(e) = element {
                    // `:before`/`:after` (single colon) are accepted for compatibility.
                    return Some(PseudoParse::Element(e));
                }
                if pseudo_element {
                    // Unknown pseudo-element (::-webkit-scrollbar, ...) never matches.
                    return Some(PseudoParse::Class(Pseudo::Never));
                }
                Some(PseudoParse::Class(match lname.as_str() {
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
                    "enabled" => Pseudo::Enabled,
                    "disabled" => Pseudo::Disabled,
                    "checked" => Pseudo::Checked,
                    "hover" => Pseudo::Hover,
                    "active" => Pseudo::Active,
                    "focus" => Pseudo::Focus,
                    "focus-visible" => Pseudo::FocusVisible,
                    "focus-within" => Pseudo::FocusWithin,
                    "visited" => Pseudo::Visited,
                    "target" => Pseudo::Target,
                    "host" => Pseudo::Host(Vec::new()),
                    "defined" => Pseudo::Defined,
                    _ => Pseudo::Never,
                }))
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
                    if lname == "slotted" {
                        let mut sub = SelectorParser::new(inner);
                        return Some(PseudoParse::Class(Pseudo::Slotted(sub.parse_list_forgiving())));
                    }
                    return Some(PseudoParse::Class(Pseudo::Never));
                }
                Some(PseudoParse::Class(match lname.as_str() {
                    "host" | "host-context" => {
                        let mut sub = SelectorParser::new(inner);
                        let list = sub.parse_list_forgiving();
                        if lname == "host" {
                            Pseudo::Host(list)
                        } else {
                            Pseudo::HostContext(list)
                        }
                    }
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
                    "nth-child" | "nth-last-child" | "nth-of-type" | "nth-last-of-type" => {
                        let (a, b) = parse_nth(inner)?;
                        match lname.as_str() {
                            "nth-child" => Pseudo::NthChild(a, b),
                            "nth-last-child" => Pseudo::NthLastChild(a, b),
                            "nth-of-type" => Pseudo::NthOfType(a, b),
                            _ => Pseudo::NthLastOfType(a, b),
                        }
                    }
                    "has" => {
                        let mut sub = SelectorParser::new(inner);
                        Pseudo::Has(sub.parse_relative_list())
                    }
                    "lang" => Pseudo::Lang(
                        inner
                            .iter()
                            .find_map(|t| match t {
                                Token::Ident(s) | Token::String(s) => Some(s.to_ascii_lowercase()),
                                _ => None,
                            })
                            .unwrap_or_default(),
                    ),
                    "dir" => Pseudo::Never,
                    _ => Pseudo::Never,
                }))
            }
            _ => None,
        }
    }

    /// `:has()` argument: a list of relative selectors (`> a`, `+ b`, `c`).
    fn parse_relative_list(&mut self) -> Vec<(Combinator, Selector)> {
        let mut list = Vec::new();
        loop {
            self.skip_ws();
            if self.peek().is_none() {
                break;
            }
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
                _ => Combinator::Descendant,
            };
            self.skip_ws();
            match self.parse_complex() {
                Some(s) => list.push((comb, s)),
                None => {
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

/// Dynamic document state used by `:hover`, `:active`, `:focus` and
/// `:target`. With the default (empty) state those pseudo-classes never
/// match.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MatchState {
    /// The element under the pointer; `:hover` matches it and its ancestors.
    pub hovered: Option<NodeId>,
    /// The focused element: `:focus` matches it, `:focus-within` matches
    /// it and its ancestors.
    pub focused: Option<NodeId>,
    /// The focused element also matches `:focus-visible` (it was focused
    /// from the keyboard, or it is a text-entry control).
    pub focus_visible: bool,
    /// The element being activated (mouse button down); matches it and its
    /// ancestors.
    pub active: Option<NodeId>,
    /// The id named by the URL fragment, for `:target`.
    pub target_id: Option<String>,
    /// Links (`<a>` / `<area>` with `href`) whose destination was visited:
    /// they match `:visited` instead of `:link`.
    pub visited: std::sync::Arc<std::collections::HashSet<NodeId>>,
}

impl MatchState {
    fn is_or_contains(doc: &Document, ancestor: Option<NodeId>, node: NodeId) -> bool {
        let Some(mut cur) = ancestor else { return false };
        loop {
            if cur == node {
                return true;
            }
            match doc.parent(cur) {
                Some(p) => cur = p,
                None => return false,
            }
        }
    }
}

/// Match a selector against an element without dynamic state.
pub fn matches(sel: &Selector, doc: &Document, node: NodeId) -> bool {
    matches_with(sel, doc, node, &MatchState::default())
}

/// Match a selector against an element with the given dynamic state. Only
/// the element part of the selector is considered; a pseudo-element on the
/// selector is the caller's business.
pub fn matches_with(sel: &Selector, doc: &Document, node: NodeId, state: &MatchState) -> bool {
    let n = sel.compounds.len();
    match_from(sel, n - 1, doc, node, state)
}

fn match_from(sel: &Selector, idx: usize, doc: &Document, node: NodeId, state: &MatchState) -> bool {
    if !match_compound_with(&sel.compounds[idx], doc, node, state) {
        return false;
    }
    if idx == 0 {
        return true;
    }
    let comb = sel.combinators[idx - 1];
    match comb {
        Combinator::Child => match element_parent(doc, node) {
            // Crossing from a shadow tree to its host: only `:host`
            // compounds can match the (featureless) host.
            Some((p, crossed)) => (!crossed || compound_is_host(&sel.compounds[idx - 1])) && match_from(sel, idx - 1, doc, p, state),
            None => false,
        },
        Combinator::Descendant => {
            let mut cur = element_parent(doc, node);
            while let Some((p, crossed)) = cur {
                if crossed {
                    return compound_is_host(&sel.compounds[idx - 1]) && match_from(sel, idx - 1, doc, p, state);
                }
                if match_from(sel, idx - 1, doc, p, state) {
                    return true;
                }
                cur = element_parent(doc, p);
            }
            false
        }
        Combinator::NextSibling => match prev_element_sibling(doc, node) {
            Some(s) => match_from(sel, idx - 1, doc, s, state),
            None => false,
        },
        Combinator::SubsequentSibling => {
            let mut cur = prev_element_sibling(doc, node);
            while let Some(s) = cur {
                if match_from(sel, idx - 1, doc, s, state) {
                    return true;
                }
                cur = prev_element_sibling(doc, s);
            }
            false
        }
    }
}

/// The parent element for combinator matching, and whether it is reached
/// by crossing from a shadow tree to its host. Fragments stop the walk.
fn element_parent(doc: &Document, node: NodeId) -> Option<(NodeId, bool)> {
    let p = doc.parent(node)?;
    let el = doc.element(p)?;
    if el.tag.starts_with('#') {
        return doc.shadow_host(p).map(|h| (h, true));
    }
    Some((p, false))
}

fn compound_is_host(c: &Compound) -> bool {
    c.pseudos.iter().any(|p| matches!(p, Pseudo::Host(_) | Pseudo::HostContext(_)))
}

/// Does `sel` (a rule of a shadow tree whose last compound carries
/// `::slotted(...)`) match `node`, slotted into `slot`?
pub fn matches_slotted(sel: &Selector, doc: &Document, node: NodeId, slot: NodeId, state: &MatchState) -> bool {
    let Some(last) = sel.compounds.last() else { return false };
    let Some(arg) = last.pseudos.iter().find_map(|p| match p {
        Pseudo::Slotted(l) => Some(l),
        _ => None,
    }) else {
        return false;
    };
    if !arg.iter().any(|s| matches_with(s, doc, node, state)) {
        return false;
    }
    let mut host_part = sel.clone();
    if let Some(c) = host_part.compounds.last_mut() {
        c.pseudos.retain(|p| !matches!(p, Pseudo::Slotted(_)));
    }
    host_part.pseudo_element = None;
    matches_with(&host_part, doc, slot, state)
}

/// Is this selector's subject a shadow host (`:host...` in its last
/// compound)?
pub fn is_host_selector(sel: &Selector) -> bool {
    sel.compounds.last().map(compound_is_host).unwrap_or(false)
}

/// Is this a `::slotted()` selector?
pub fn is_slotted_selector(sel: &Selector) -> bool {
    sel.compounds.last().map(|c| c.pseudos.iter().any(|p| matches!(p, Pseudo::Slotted(_)))).unwrap_or(false)
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
    match_compound_with(c, doc, node, &MatchState::default())
}

pub fn match_compound_with(c: &Compound, doc: &Document, node: NodeId, state: &MatchState) -> bool {
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
    // Structural pseudo-classes need a parent element (or fragment): the
    // root element is not a :first-child (as in Blink/WebKit/Gecko).
    let has_parent = || doc.parent(node).map(|p| doc.element(p).is_some()).unwrap_or(false);
    for p in &c.pseudos {
        let ok = match p {
            Pseudo::FirstChild | Pseudo::LastChild | Pseudo::OnlyChild | Pseudo::FirstOfType | Pseudo::LastOfType | Pseudo::OnlyOfType | Pseudo::NthChild(..) | Pseudo::NthLastChild(..) | Pseudo::NthOfType(..) | Pseudo::NthLastOfType(..)
                if !has_parent() =>
            {
                false
            }
            Pseudo::Root => doc.parent(node).map(|p| matches!(doc.node(p).data, NodeData::Document)).unwrap_or(false),
            Pseudo::FirstChild => prev_element_sibling(doc, node).is_none(),
            Pseudo::LastChild => next_element_sibling(doc, node).is_none(),
            Pseudo::OnlyChild => prev_element_sibling(doc, node).is_none() && next_element_sibling(doc, node).is_none(),
            Pseudo::FirstOfType => child_index(doc, node, true, false) == 1,
            Pseudo::LastOfType => child_index(doc, node, true, true) == 1,
            Pseudo::OnlyOfType => child_index(doc, node, true, false) == 1 && child_index(doc, node, true, true) == 1,
            // No element children and no text (empty text nodes don't count).
            Pseudo::Empty => doc.children(node).all(|ch| match &doc.node(ch).data {
                NodeData::Comment(_) => true,
                NodeData::Text(t) => t.is_empty(),
                _ => false,
            }),
            Pseudo::AnyLink => matches!(el.tag.as_str(), "a" | "area") && el.attr("href").is_some(),
            Pseudo::Link => matches!(el.tag.as_str(), "a" | "area") && el.attr("href").is_some() && !state.visited.contains(&node),
            Pseudo::NthChild(a, b) => nth_matches(*a, *b, child_index(doc, node, false, false)),
            Pseudo::NthLastChild(a, b) => nth_matches(*a, *b, child_index(doc, node, false, true)),
            Pseudo::NthOfType(a, b) => nth_matches(*a, *b, child_index(doc, node, true, false)),
            Pseudo::Not(list) => !list.iter().any(|s| matches_with(s, doc, node, state)),
            Pseudo::Is(list) | Pseudo::Where(list) => list.iter().any(|s| matches_with(s, doc, node, state)),
            Pseudo::NthLastOfType(a, b) => nth_matches(*a, *b, child_index(doc, node, true, true)),
            Pseudo::Has(list) => list.iter().any(|(comb, sel)| has_matches(*comb, sel, doc, node, state)),
            Pseudo::Lang(lang) => {
                let mut cur = Some(node);
                let mut found = false;
                while let Some(n) = cur {
                    if let Some(l) = doc.attr(n, "lang") {
                        let l = l.to_ascii_lowercase();
                        found = l == *lang || l.starts_with(&format!("{lang}-"));
                        break;
                    }
                    cur = doc.parent(n);
                }
                found
            }
            Pseudo::Enabled => {
                matches!(el.tag.as_str(), "input" | "button" | "select" | "textarea" | "option" | "fieldset")
                    && el.attr("disabled").is_none()
            }
            Pseudo::Disabled => el.attr("disabled").is_some(),
            // Checkboxes and radio buttons by checkedness; options by
            // selectedness.
            Pseudo::Checked => match el.tag.as_str() {
                "input" => el.attr("type").map(|t| t.eq_ignore_ascii_case("checkbox") || t.eq_ignore_ascii_case("radio")).unwrap_or(false) && doc.is_checked(node),
                "option" => el.attr("selected").is_some(),
                _ => false,
            },
            Pseudo::Hover => MatchState::is_or_contains(doc, state.hovered, node),
            Pseudo::Active => MatchState::is_or_contains(doc, state.active, node),
            Pseudo::Focus => state.focused == Some(node),
            Pseudo::FocusVisible => state.focus_visible && state.focused == Some(node),
            Pseudo::FocusWithin => MatchState::is_or_contains(doc, state.focused, node),
            Pseudo::Target => match (&state.target_id, el.id()) {
                (Some(t), Some(id)) => t == id,
                _ => false,
            },
            Pseudo::Visited => matches!(el.tag.as_str(), "a" | "area") && el.attr("href").is_some() && state.visited.contains(&node),
            Pseudo::Host(list) => doc.shadow_root(node).is_some() && (list.is_empty() || list.iter().any(|s| matches_with(s, doc, node, state))),
            Pseudo::HostContext(list) => {
                doc.shadow_root(node).is_some() && {
                    let mut cur = Some(node);
                    let mut found = false;
                    while let Some(n) = cur {
                        if doc.element(n).map(|e| !e.tag.starts_with('#')).unwrap_or(false) && list.iter().any(|s| matches_with(s, doc, n, state)) {
                            found = true;
                            break;
                        }
                        cur = doc.shadow_including_parent(n);
                    }
                    found
                }
            }
            Pseudo::Slotted(_) => false,
            Pseudo::Defined => doc.is_defined(node),
            Pseudo::Never => false,
        };
        if !ok {
            return false;
        }
    }
    true
}

/// `:has()` support: does any element related to `node` by `comb` match `sel`?
fn has_matches(comb: Combinator, sel: &Selector, doc: &Document, node: NodeId, state: &MatchState) -> bool {
    match comb {
        Combinator::Child => doc.child_elements(node).any(|c| matches_with(sel, doc, c, state)),
        Combinator::Descendant => doc.descendants(node).skip(1).any(|d| matches_with(sel, doc, d, state)),
        Combinator::NextSibling => next_element_sibling(doc, node).map(|s| matches_with(sel, doc, s, state)).unwrap_or(false),
        Combinator::SubsequentSibling => {
            let mut cur = next_element_sibling(doc, node);
            while let Some(s) = cur {
                if matches_with(sel, doc, s, state) {
                    return true;
                }
                cur = next_element_sibling(doc, s);
            }
            false
        }
    }
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
        let state = MatchState { hovered: Some(a), ..Default::default() };
        assert!(matches_with(&parse("p:hover")[0], &doc, ps[0], &state));
        assert!(matches_with(&parse("a:hover")[0], &doc, a, &state));
        assert!(!matches_with(&parse("p:hover")[0], &doc, ps[1], &state));
        assert!(matches_with(&parse("div:has(> p.intro)")[0], &doc, doc.find_element("div").unwrap(), &state));
        let sel = parse("p::before");
        assert_eq!(sel[0].pseudo_element, Some(PseudoElement::Before));
        assert!(matches(&sel[0], &doc, ps[0]));
    }
}
