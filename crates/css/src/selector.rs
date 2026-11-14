//! Selector parsing and matching (Selectors Level 3 subset, plus `:not()`,
//! `:is()`/`:where()`, `:nth-child()` and attribute operators).

use super::tokenizer::Token;
use std::sync::Arc;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::marker::PhantomData;
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
    NthChildOf(i32, i32, Vec<Selector>),
    NthLastChildOf(i32, i32, Vec<Selector>),
    NthOfType(i32, i32),
    Not(Vec<Selector>),
    Is(Vec<Selector>),
    /// Shared nesting parent: avoid exponential expansion of repeated `&`.
    Nesting(Arc<Vec<Selector>>),
    Where(Vec<Selector>),
    NthLastOfType(i32, i32),
    /// `:has(> a, b)` relative selectors.
    Has(Vec<(Combinator, Selector)>),
    Lang(String),
    Enabled,
    Disabled,
    Checked,
    /// Form-control states (HTML "Selectors" section).
    Indeterminate,
    PlaceholderShown,
    Required,
    Optional,
    ReadOnly,
    ReadWrite,
    Valid,
    Invalid,
    InRange,
    OutOfRange,
    Default,
    /// `:open` (open `<details>` / `<dialog>`).
    Open,
    /// `:modal` (an element in the document's top layer).
    Modal,
    /// `:modal`, `:popover-open`, `:fullscreen`, `:autofill`, ... states
    /// we are never in, but which are valid selectors.
    NeverState,
    /// `:scope` outside scoped contexts is `:root`.
    Scope,
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
    /// `::-webkit-slider-thumb` / `::-moz-range-thumb`.
    SliderThumb,
    /// `::-webkit-progress-value` / `::-moz-progress-bar`.
    ProgressValue,
    /// `::file-selector-button` / `::-webkit-file-upload-button`.
    FileSelectorButton,
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
    /// Bound work across the shared nesting DAG, including repeated `&`.
    pub(crate) fn fits_nesting_budget(&self, budget: &mut usize) -> bool {
        for c in &self.compounds {
            if *budget == 0 { return false; }
            *budget -= 1;
            for p in &c.pseudos {
                let valid = match p {
                    Pseudo::Nesting(list) => list.iter().all(|s| s.fits_nesting_budget(budget)),
                    Pseudo::Is(list) | Pseudo::Where(list) | Pseudo::Not(list)
                    | Pseudo::NthChildOf(_, _, list) | Pseudo::NthLastChildOf(_, _, list)
                    | Pseudo::Host(list) | Pseudo::HostContext(list) | Pseudo::Slotted(list) => list.iter().all(|s| s.fits_nesting_budget(budget)),
                    Pseudo::Has(list) => list.iter().all(|(_, s)| s.fits_nesting_budget(budget)),
                    _ => true,
                };
                if !valid { return false; }
            }
        }
        true
    }
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
            Pseudo::NthChildOf(_, _, list) | Pseudo::NthLastChildOf(_, _, list) => {
                s.1 += 1;
                if let Some(m) = list.iter().map(Selector::specificity).max() {
                    s.0 += m.0; s.1 += m.1; s.2 += m.2;
                }
            }
            Pseudo::Nesting(list) => {
                if let Some(m) = list.iter().map(Selector::specificity).max() {
                    s.0 += m.0; s.1 += m.1; s.2 += m.2;
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
    parent: Option<Arc<Vec<Selector>>>,
    implicit_parent: bool,
    toks: &'a [Token],
    pos: usize,
}

impl<'a> SelectorParser<'a> {
    pub fn new(toks: &'a [Token]) -> Self {
        SelectorParser { toks, pos: 0, parent: None, implicit_parent: false }
    }

    pub(crate) fn nested(toks: &'a [Token], parent: Arc<Vec<Selector>>) -> Self {
        Self { toks, pos: 0, parent: Some(parent), implicit_parent: true }
    }

    fn subparser(&self, toks: &'a [Token]) -> Self {
        Self { toks, pos: 0, parent: self.parent.clone(), implicit_parent: false }
    }

    fn nesting(&self) -> Pseudo {
        self.parent.as_ref().map(|p| Pseudo::Nesting(p.clone()))
            .unwrap_or_else(|| Pseudo::Where(vec![Selector { compounds: vec![Compound { pseudos: vec![Pseudo::Scope], ..Compound::default() }], combinators: vec![], pseudo_element: None }]))
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
            let start = self.pos;
            let leading = self.implicit_parent && matches!(self.peek(), Some(Token::Delim('>' | '+' | '~')));
            let mut sel = self.parse_complex()?;
            if self.implicit_parent && !leading && !self.toks[start..self.pos].contains(&Token::Delim('&')) {
                sel.compounds.insert(0, Compound { pseudos: vec![self.nesting()], ..Compound::default() });
                sel.combinators.insert(0, Combinator::Descendant);
            }
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
        let leading = self.implicit_parent && matches!(self.peek(), Some(Token::Delim('>' | '+' | '~')));
        let mut compounds = vec![if leading { Compound { pseudos: vec![self.nesting()], ..Compound::default() } } else { self.parse_compound()? }];
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
                Some(Token::Delim('&')) => {
                    c.pseudos.push(self.nesting());
                    self.pos += 1;
                }
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
                    "-webkit-slider-thumb" | "-moz-range-thumb" => Some(PseudoElement::SliderThumb),
                    "-webkit-progress-value" | "-moz-progress-bar" => Some(PseudoElement::ProgressValue),
                    "file-selector-button" | "-webkit-file-upload-button" => Some(PseudoElement::FileSelectorButton),
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
                    "indeterminate" => Pseudo::Indeterminate,
                    "placeholder-shown" | "-moz-placeholder-shown" => Pseudo::PlaceholderShown,
                    "required" => Pseudo::Required,
                    "optional" => Pseudo::Optional,
                    "read-only" | "-moz-read-only" => Pseudo::ReadOnly,
                    "read-write" | "-moz-read-write" => Pseudo::ReadWrite,
                    "valid" => Pseudo::Valid,
                    "invalid" => Pseudo::Invalid,
                    "in-range" => Pseudo::InRange,
                    "out-of-range" => Pseudo::OutOfRange,
                    "default" => Pseudo::Default,
                    "open" => Pseudo::Open,
                    "scope" => Pseudo::Scope,
                    "modal" => Pseudo::Modal,
                    "popover-open" | "fullscreen" | "-webkit-full-screen" | "picture-in-picture" | "autofill" | "-webkit-autofill" | "user-invalid"
                    | "user-valid" | "-moz-ui-invalid" | "-moz-ui-valid" | "blank" | "closed" | "playing" | "paused" | "seeking" | "buffering" | "stalled" | "muted"
                    | "volume-locked" | "target-within" | "local-link" | "current" | "past" | "future" | "-moz-focusring" => Pseudo::NeverState,
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
                        let mut sub = self.subparser(inner);
                        return Some(PseudoParse::Class(Pseudo::Slotted(sub.parse_list_forgiving())));
                    }
                    return Some(PseudoParse::Class(Pseudo::Never));
                }
                Some(PseudoParse::Class(match lname.as_str() {
                    "host" | "host-context" => {
                        let mut sub = self.subparser(inner);
                        let list = sub.parse_list_forgiving();
                        if lname == "host" {
                            Pseudo::Host(list)
                        } else {
                            Pseudo::HostContext(list)
                        }
                    }
                    "not" | "is" | "where" | "matches" | "-webkit-any" | "-moz-any" => {
                        let mut sub = self.subparser(inner);
                        // Forgiving selector list: drop invalid parts.
                        let list = sub.parse_list_forgiving();
                        match lname.as_str() {
                            "not" => Pseudo::Not(list),
                            "where" => Pseudo::Where(list),
                            _ => Pseudo::Is(list),
                        }
                    }
                    "nth-child" | "nth-last-child" | "nth-of-type" | "nth-last-of-type" => {
                        let mut depth = 0usize;
                        let mut of = None;
                        for (i, token) in inner.iter().enumerate() {
                            match token {
                                Token::Function(_) | Token::OpenParen | Token::OpenBracket => depth += 1,
                                Token::CloseParen | Token::CloseBracket => depth = depth.saturating_sub(1),
                                Token::Ident(n) if depth == 0 && n.eq_ignore_ascii_case("of") => { of = Some(i); break; }
                                _ => {}
                            }
                        }
                        if let Some(i) = of {
                            if lname != "nth-child" && lname != "nth-last-child" { return None; }
                            let (a, b) = parse_nth(&inner[..i])?;
                            let mut sub = self.subparser(&inner[i + 1..]);
                            let list = sub.parse_list()?;
                            if list.iter().any(|s| s.pseudo_element.is_some() || s.compounds.iter().any(|c| c.pseudos.iter().any(|p| matches!(p, Pseudo::Never | Pseudo::Slotted(_))))) { return None; }
                            return Some(PseudoParse::Class(if lname == "nth-child" { Pseudo::NthChildOf(a, b, list) } else { Pseudo::NthLastChildOf(a, b, list) }));
                        }
                        let (a, b) = parse_nth(inner)?;
                        match lname.as_str() {
                            "nth-child" => Pseudo::NthChild(a, b),
                            "nth-last-child" => Pseudo::NthLastChild(a, b),
                            "nth-of-type" => Pseudo::NthOfType(a, b),
                            _ => Pseudo::NthLastOfType(a, b),
                        }
                    }
                    "has" => {
                        let mut sub = self.subparser(inner);
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
    let significant: Vec<_> = toks.iter().filter(|t| **t != Token::Whitespace).collect();
    if matches!(significant.first(), Some(Token::Number(_))) && significant.len() != 1 { return None; }
    let integer = |n: f32| n.is_finite() && n.fract() == 0.0 && (n as f64) >= i32::MIN as f64 && (n as f64) <= i32::MAX as f64;
    let mut s = String::new();
    for t in toks {
        match t {
            Token::Ident(i) => s.push_str(i),
            Token::Number(n) if integer(*n) => s.push_str(&format!("{}", *n as i32)),
            Token::Dimension(n, u) if integer(*n) => s.push_str(&format!("{}{}", *n as i32, u)),
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
    let diff = i64::from(index) - i64::from(b);
    let a = i64::from(a);
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

/// Matching state for one immutable document/state pass. The selector
/// lifetime prevents pointer-key reuse after a temporary AST is dropped.
/// Drop this context before mutating the DOM or changing dynamic state.
/// Temporary selector storage cannot be dropped and reused during a pass:
/// ```compile_fail
/// use rabbithole_css::selector::{Compound, Selector, SelectorMatcher};
/// use rabbithole_css::MatchState;
/// let doc = rabbithole_dom::Document::new();
/// let state = MatchState::default();
/// let matcher = SelectorMatcher::new(&doc, &state);
/// {
///     let temporary = Selector { compounds: vec![Compound::default()], combinators: vec![], pseudo_element: None };
///     matcher.matches(&temporary, 0);
/// }
/// let next = Selector { compounds: vec![Compound::default()], combinators: vec![], pseudo_element: None };
/// matcher.matches(&next, 0);
/// ```
pub struct SelectorMatcher<'d, 's> {
    doc: &'d Document,
    state: &'d MatchState,
    nth: RefCell<HashMap<(usize, NodeId), Arc<NthRanks>>>,
    cached_nodes: Cell<usize>,
    selectors: PhantomData<Cell<&'s Selector>>,
    #[cfg(test)]
    rank_builds: Cell<usize>,
}

struct NthRanks { indices: HashMap<NodeId, i32>, count: i32 }

impl<'d, 's> SelectorMatcher<'d, 's> {
    pub fn new(doc: &'d Document, state: &'d MatchState) -> Self {
        Self { doc, state, nth: RefCell::new(HashMap::new()), cached_nodes: Cell::new(0), selectors: PhantomData,
            #[cfg(test)] rank_builds: Cell::new(0) }
    }

    pub fn matches(&self, selector: &'s Selector, node: NodeId) -> bool {
        match_from(selector, selector.compounds.len() - 1, self.doc, node, self.state, self)
    }

    fn nth_index(&self, list: &'s [Selector], node: NodeId, backwards: bool) -> Option<i32> {
        let parent = self.doc.parent(node)?;
        let key = (list.as_ptr() as usize, parent);
        let existing = self.nth.borrow().get(&key).cloned();
        let ranks = if let Some(ranks) = existing { ranks } else {
            #[cfg(test)] self.rank_builds.set(self.rank_builds.get() + 1);
            let mut indices = HashMap::new();
            let mut count = 0;
            for sibling in self.doc.child_elements(parent) {
                if list.iter().any(|selector| self.matches(selector, sibling)) {
                    count += 1; indices.insert(sibling, count);
                }
            }
            let ranks = Arc::new(NthRanks { indices, count });
            // Bound auxiliary memory on pages with many unrelated filters.
            // Exceeding this budget falls back to uncached matching, preserving
            // results; no state or pointers survive this matching pass.
            if self.cached_nodes.get() + ranks.indices.len() <= 1_000_000 && self.nth.borrow().len() < 4096 {
                self.cached_nodes.set(self.cached_nodes.get() + ranks.indices.len());
                self.nth.borrow_mut().insert(key, ranks.clone());
            }
            ranks
        };
        ranks.indices.get(&node).map(|&i| if backwards { ranks.count - i + 1 } else { i })
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
    SelectorMatcher::new(doc, state).matches(sel, node)
}

fn match_from<'s>(sel: &'s Selector, idx: usize, doc: &Document, node: NodeId, state: &MatchState, matcher: &SelectorMatcher<'_, 's>) -> bool {
    if !match_compound_cached(&sel.compounds[idx], doc, node, state, matcher) {
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
            Some((p, crossed)) => (!crossed || compound_is_host(&sel.compounds[idx - 1])) && match_from(sel, idx - 1, doc, p, state, matcher),
            None => false,
        },
        Combinator::Descendant => {
            let mut cur = element_parent(doc, node);
            while let Some((p, crossed)) = cur {
                if crossed {
                    return compound_is_host(&sel.compounds[idx - 1]) && match_from(sel, idx - 1, doc, p, state, matcher);
                }
                if match_from(sel, idx - 1, doc, p, state, matcher) {
                    return true;
                }
                cur = element_parent(doc, p);
            }
            false
        }
        Combinator::NextSibling => match prev_element_sibling(doc, node) {
            Some(s) => match_from(sel, idx - 1, doc, s, state, matcher),
            None => false,
        },
        Combinator::SubsequentSibling => {
            let mut cur = prev_element_sibling(doc, node);
            while let Some(s) = cur {
                if match_from(sel, idx - 1, doc, s, state, matcher) {
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
    match_compound_cached(c, doc, node, state, &SelectorMatcher::new(doc, state))
}

fn match_compound_cached<'s>(c: &'s Compound, doc: &Document, node: NodeId, state: &MatchState, matcher: &SelectorMatcher<'_, 's>) -> bool {
    let el = match doc.element(node) {
        Some(e) => e,
        None => return false,
    };
    if let Some(t) = &c.tag {
        // Selectors are lower-cased; SVG / XML elements keep their case
        // (`linearGradient`, `Foo`), so compare without ASCII case.
        if t != "*" && *t != el.tag && !el.tag.eq_ignore_ascii_case(t) {
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
            Pseudo::FirstChild | Pseudo::LastChild | Pseudo::OnlyChild | Pseudo::FirstOfType | Pseudo::LastOfType | Pseudo::OnlyOfType | Pseudo::NthChild(..) | Pseudo::NthLastChild(..) | Pseudo::NthChildOf(..) | Pseudo::NthLastChildOf(..) | Pseudo::NthOfType(..) | Pseudo::NthLastOfType(..)
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
            Pseudo::NthChildOf(a, b, list) | Pseudo::NthLastChildOf(a, b, list) => {
                if *a == 1 && *b <= 1 {
                    // Every positive filtered rank matches n / n+1. This
                    // common nested filter needs no sibling scan at all.
                    list.iter().any(|selector| matcher.matches(selector, node))
                } else if *a == 0 && *b <= 0 { false } else {
                    matcher.nth_index(list, node, matches!(p, Pseudo::NthLastChildOf(..)))
                        .is_some_and(|index| nth_matches(*a, *b, index))
                }
            }
            Pseudo::NthOfType(a, b) => nth_matches(*a, *b, child_index(doc, node, true, false)),
            Pseudo::Not(list) => !list.iter().any(|s| matcher.matches(s, node)),
            Pseudo::Is(list) | Pseudo::Where(list) => list.iter().any(|s| matcher.matches(s, node)),
            Pseudo::Nesting(list) => list.iter().any(|s| s.pseudo_element.is_none() && matcher.matches(s, node)),
            Pseudo::NthLastOfType(a, b) => nth_matches(*a, *b, child_index(doc, node, true, true)),
            Pseudo::Has(list) => list.iter().any(|(comb, sel)| has_matches(*comb, sel, doc, node, state, matcher)),
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
            Pseudo::Enabled | Pseudo::Disabled => {
                let disabled = match el.tag.as_str() {
                    "input" | "button" | "select" | "textarea" | "fieldset" => Some(is_disabled_control(doc, node)),
                    "optgroup" => Some(el.attr("disabled").is_some()),
                    "option" => Some(el.attr("disabled").is_some() || doc.parent(node).is_some_and(|p| doc.is_element(p, "optgroup") && doc.attr(p, "disabled").is_some())),
                    _ => None,
                };
                disabled == Some(matches!(p, Pseudo::Disabled))
            }
            // Checkboxes and radio buttons by checkedness; options by
            // selectedness.
            Pseudo::Checked => match el.tag.as_str() {
                "input" => el.attr("type").map(|t| t.eq_ignore_ascii_case("checkbox") || t.eq_ignore_ascii_case("radio")).unwrap_or(false) && doc.is_checked(node),
                "option" => el.attr("selected").is_some(),
                _ => false,
            },
            Pseudo::Indeterminate => match el.tag.as_str() {
                "input" => {
                    let t = input_type(el);
                    (t == "checkbox" && el.attr("data-rh-indeterminate").is_some()) || (t == "radio" && radio_group_unchecked(doc, node))
                }
                "progress" => parse_num(el.attr("value")).is_none(),
                _ => false,
            },
            Pseudo::PlaceholderShown => {
                matches!(el.tag.as_str(), "input" | "textarea")
                    && el.attr("placeholder").is_some()
                    && (el.tag == "textarea" || is_text_input(&input_type(el)))
                    && control_value_of(doc, node).is_empty()
            }
            Pseudo::Required => is_requirable(el) && el.attr("required").is_some(),
            Pseudo::Optional => is_requirable(el) && el.attr("required").is_none(),
            Pseudo::ReadWrite => is_read_write(doc, node),
            Pseudo::ReadOnly => !is_read_write(doc, node),
            Pseudo::Valid | Pseudo::Invalid => {
                let (candidate, invalid) = match el.tag.as_str() {
                    "form" | "fieldset" => (true, doc.descendants(node).skip(1).any(|d| is_invalid_control(doc, d))),
                    _ => (is_validation_candidate(doc, node), is_invalid_control(doc, node)),
                };
                candidate && (invalid == matches!(p, Pseudo::Invalid))
            }
            Pseudo::InRange | Pseudo::OutOfRange => match range_state(doc, node) {
                Some(out) => out == matches!(p, Pseudo::OutOfRange),
                None => false,
            },
            Pseudo::Default => match el.tag.as_str() {
                "input" => {
                    let t = input_type(el);
                    (matches!(t.as_str(), "checkbox" | "radio") && el.attr("checked").is_some()) || (matches!(t.as_str(), "submit" | "image") && is_default_button(doc, node))
                }
                "button" => is_default_button(doc, node),
                "option" => el.attr("selected").is_some(),
                _ => false,
            },
            Pseudo::Open => matches!(el.tag.as_str(), "details" | "dialog") && el.attr("open").is_some(),
            Pseudo::Modal => doc.top_layer.contains(&node),
            Pseudo::NeverState => false,
            Pseudo::Scope => doc.parent(node).map(|p| matches!(doc.node(p).data, NodeData::Document)).unwrap_or(false),
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
            Pseudo::Host(list) => doc.shadow_root(node).is_some() && (list.is_empty() || list.iter().any(|s| matcher.matches(s, node))),
            Pseudo::HostContext(list) => {
                doc.shadow_root(node).is_some() && {
                    let mut cur = Some(node);
                    let mut found = false;
                    while let Some(n) = cur {
                        if doc.element(n).map(|e| !e.tag.starts_with('#')).unwrap_or(false) && list.iter().any(|s| matcher.matches(s, n)) {
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

fn input_type(el: &rabbithole_dom::ElementData) -> String {
    el.attr("type").map(|t| t.trim().to_ascii_lowercase()).unwrap_or_else(|| "text".into())
}

const KNOWN_INPUT_TYPES: &[&str] = &[
    "hidden", "text", "search", "tel", "url", "email", "password", "date", "month", "week", "time", "datetime-local", "number", "range", "color", "checkbox", "radio", "file",
    "submit", "image", "reset", "button",
];

/// Text-entry input types (an unknown type is `text`).
fn is_text_input(t: &str) -> bool {
    matches!(t, "text" | "search" | "url" | "tel" | "email" | "password" | "number") || !KNOWN_INPUT_TYPES.contains(&t)
}

fn control_value_of(doc: &Document, node: NodeId) -> String {
    if doc.is_element(node, "textarea") {
        return doc.control_value(node).map(str::to_string).unwrap_or_else(|| doc.text_content(node));
    }
    doc.control_value(node).unwrap_or("").to_string()
}

/// Elements `required` applies to.
fn is_requirable(el: &rabbithole_dom::ElementData) -> bool {
    match el.tag.as_str() {
        "select" | "textarea" => true,
        "input" => !matches!(input_type(el).as_str(), "hidden" | "range" | "color" | "submit" | "image" | "reset" | "button"),
        _ => false,
    }
}

fn is_disabled_control(doc: &Document, node: NodeId) -> bool {
    if doc.attr(node, "disabled").is_some() {
        return true;
    }
    // Inside a disabled fieldset (but not inside its first legend).
    let mut child = node;
    let mut cur = doc.parent(node);
    while let Some(p) = cur {
        if doc.is_element(p, "fieldset") && doc.attr(p, "disabled").is_some() {
            let first_legend = doc.child_elements(p).find(|&c| doc.is_element(c, "legend"));
            if first_legend != Some(child) {
                return true;
            }
        }
        child = p;
        cur = doc.parent(p);
    }
    false
}

fn is_read_write(doc: &Document, node: NodeId) -> bool {
    let Some(el) = doc.element(node) else { return false };
    match el.tag.as_str() {
        "textarea" => el.attr("readonly").is_none() && !is_disabled_control(doc, node),
        "input" => {
            let t = input_type(el);
            let mutable = is_text_input(&t) || matches!(t.as_str(), "date" | "month" | "week" | "time" | "datetime-local");
            mutable && el.attr("readonly").is_none() && !is_disabled_control(doc, node)
        }
        _ => {
            // Editing hosts and their descendants.
            let mut cur = Some(node);
            while let Some(n) = cur {
                if let Some(v) = doc.attr(n, "contenteditable") {
                    return !v.trim().eq_ignore_ascii_case("false");
                }
                cur = doc.parent(n);
            }
            false
        }
    }
}

/// Submittable elements that take part in constraint validation.
fn is_validation_candidate(doc: &Document, node: NodeId) -> bool {
    let Some(el) = doc.element(node) else { return false };
    let ok = match el.tag.as_str() {
        "select" | "textarea" => true,
        "button" => !matches!(el.attr("type").map(|t| t.trim().to_ascii_lowercase()).as_deref(), Some("button" | "reset")),
        "input" => !matches!(input_type(el).as_str(), "hidden" | "reset" | "button"),
        _ => false,
    };
    if !ok || is_disabled_control(doc, node) {
        return false;
    }
    if matches!(el.tag.as_str(), "input" | "textarea") && el.attr("readonly").is_some() {
        return false;
    }
    let mut cur = doc.parent(node);
    while let Some(p) = cur {
        if doc.is_element(p, "datalist") {
            return false;
        }
        cur = doc.parent(p);
    }
    true
}

fn parse_num(v: Option<&str>) -> Option<f64> {
    v.and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite())
}

/// `Some(out_of_range)` for inputs with a range limitation.
fn range_state(doc: &Document, node: NodeId) -> Option<bool> {
    let el = doc.element(node)?;
    let t = input_type(el);
    if el.tag != "input" || !matches!(t.as_str(), "number" | "range") {
        return None;
    }
    let min = parse_num(el.attr("min"));
    let max = parse_num(el.attr("max"));
    if t == "number" && min.is_none() && max.is_none() {
        return None;
    }
    let Some(v) = parse_num(doc.control_value(node)) else { return Some(false) };
    Some((t == "number" && min.is_some_and(|m| v < m)) || (t == "number" && max.is_some_and(|m| v > m)))
}

fn is_invalid_control(doc: &Document, node: NodeId) -> bool {
    if !is_validation_candidate(doc, node) {
        return false;
    }
    let Some(el) = doc.element(node) else { return false };
    if el.attr("data-rh-custom-validity").is_some_and(|m| !m.is_empty()) {
        return true;
    }
    match el.tag.as_str() {
        "textarea" => el.attr("required").is_some() && control_value_of(doc, node).is_empty(),
        "select" => {
            if el.attr("required").is_none() || el.attr("multiple").is_some() {
                return el.attr("required").is_some() && !doc.descendants(node).any(|o| doc.is_element(o, "option") && doc.attr(o, "selected").is_some());
            }
            // A required single select showing its placeholder option
            // (first option, empty value) is invalid.
            let options: Vec<NodeId> = doc.descendants(node).filter(|&o| doc.is_element(o, "option")).collect();
            let shown = options.iter().copied().rev().find(|&o| doc.attr(o, "selected").is_some()).or_else(|| options.first().copied());
            let size = parse_num(el.attr("size")).unwrap_or(1.0);
            match shown {
                None => true,
                Some(o) => {
                    size <= 1.0
                        && Some(o) == options.first().copied()
                        && doc.attr(o, "value").map(|v| v.is_empty()).unwrap_or_else(|| doc.text_content(o).trim().is_empty())
                }
            }
        }
        "input" => {
            let t = input_type(el);
            let required = el.attr("required").is_some();
            if t == "checkbox" {
                return required && !doc.is_checked(node);
            }
            if t == "radio" {
                return required && radio_group_unchecked(doc, node);
            }
            let v = doc.control_value(node).unwrap_or("");
            if v.is_empty() {
                return required && !matches!(t.as_str(), "range" | "color" | "submit" | "image");
            }
            match t.as_str() {
                "email" => {
                    let check = |a: &str| {
                        let a = a.trim();
                        let (local, domain) = a.split_once('@').unwrap_or(("", ""));
                        !local.is_empty() && !domain.is_empty() && !domain.contains('@') && !domain.starts_with('.') && !domain.ends_with('.') && !a.contains(char::is_whitespace)
                    };
                    if el.attr("multiple").is_some() {
                        !v.split(',').all(check)
                    } else {
                        !check(v)
                    }
                }
                "url" => !v
                    .split_once(':')
                    .is_some_and(|(scheme, _)| scheme.starts_with(|c: char| c.is_ascii_alphabetic()) && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))),
                "number" => match parse_num(Some(v)) {
                    None => false,
                    Some(n) => parse_num(el.attr("min")).is_some_and(|m| n < m) || parse_num(el.attr("max")).is_some_and(|m| n > m),
                },
                _ => false,
            }
        }
        _ => false,
    }
}

/// A radio button whose group (same name, same form or tree) has no
/// checked button.
fn radio_group_unchecked(doc: &Document, node: NodeId) -> bool {
    if doc.is_checked(node) {
        return false;
    }
    let Some(name) = doc.attr(node, "name").filter(|n| !n.is_empty()) else { return true };
    let mut root = node;
    while let Some(p) = doc.parent(root) {
        root = p;
        if doc.is_element(p, "form") {
            break;
        }
    }
    !doc.descendants(root).any(|d| {
        doc.is_element(d, "input") && doc.attr(d, "type").is_some_and(|t| t.trim().eq_ignore_ascii_case("radio")) && doc.attr(d, "name") == Some(name) && doc.is_checked(d)
    })
}

/// The form's default button: its first submit button in tree order.
fn is_default_button(doc: &Document, node: NodeId) -> bool {
    let mut form = doc.parent(node);
    while let Some(f) = form {
        if doc.is_element(f, "form") {
            break;
        }
        form = doc.parent(f);
    }
    let Some(form) = form else { return false };
    let first = doc.descendants(form).find(|&d| match doc.element(d) {
        Some(e) if e.tag == "button" => !matches!(e.attr("type").map(|t| t.trim().to_ascii_lowercase()).as_deref(), Some("button" | "reset")),
        Some(e) if e.tag == "input" => matches!(input_type(e).as_str(), "submit" | "image"),
        _ => false,
    });
    first == Some(node)
}

/// `:has()` support: does any element related to `node` by `comb` match `sel`?
fn has_matches<'s>(comb: Combinator, sel: &'s Selector, doc: &Document, node: NodeId, _state: &MatchState, matcher: &SelectorMatcher<'_, 's>) -> bool {
    match comb {
        Combinator::Child => doc.child_elements(node).any(|c| matcher.matches(sel, c)),
        Combinator::Descendant => doc.descendants(node).skip(1).any(|d| matcher.matches(sel, d)),
        Combinator::NextSibling => next_element_sibling(doc, node).map(|s| matcher.matches(sel, s)).unwrap_or(false),
        Combinator::SubsequentSibling => {
            let mut cur = next_element_sibling(doc, node);
            while let Some(s) = cur {
                if matcher.matches(sel, s) {
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
    fn nth_of_filters_siblings_lists_complex_selectors_and_specificity() {
        let doc = rabbithole_html::parse("<ul class=parent><li class=target id=a></li><li id=gap></li><li class=target id=b></li><li class=target id=c><em></em></li></ul>");
        let a = doc.element_by_id("a").unwrap();
        let b = doc.element_by_id("b").unwrap();
        let c = doc.element_by_id("c").unwrap();
        let gap = doc.element_by_id("gap").unwrap();
        let second = parse(":nth-child(2 of .parent > .target, #absent)");
        assert_eq!(second[0].specificity(), Specificity(1, 1, 0));
        assert!(!matches(&second[0], &doc, a));
        assert!(matches(&second[0], &doc, b));
        assert!(!matches(&second[0], &doc, gap));
        assert!(matches(&parse(":nth-last-child(1 of.target)")[0], &doc, c));
        assert!(matches(&parse(":nth-child(2n+1 of :nth-child(n of .target))")[0], &doc, c));
        assert!(matches(&parse(":nth-child(1 of :has(> em))")[0], &doc, c));
        assert!(!matches(&parse(":nth-child(2 of .target)")[0], &doc, c));
    }

    #[test]
    fn nth_of_is_strict_and_formula_arithmetic_does_not_overflow() {
        for source in [":nth-child(1 of)", ":nth-child(1 of .a,)", ":nth-child(1 of > .a)",
            ":nth-child(1 of ::before)", ":nth-child(1 of .a, :unknown)",
            ":nth-of-type(1 of div)", ":nth-last-of-type(n of div)",
            ":nth-child(1 n)", ":nth-child(1.5 of div)"] {
            let tokens = crate::tokenizer::Tokenizer::tokenize(source);
            assert!(SelectorParser::new(&tokens).parse_list().is_none(), "{source}");
        }
        assert!(nth_matches(1, i32::MIN, 1));
        assert!(!nth_matches(i32::MIN, i32::MAX, 1));
    }

    #[test]
    fn nth_of_nested_ranks_are_built_once_per_filter_and_pass() {
        let mut html = String::from("<ul>");
        for _ in 0..2000 { html.push_str("<li class=item></li>"); }
        html.push_str("</ul>");
        let doc = rabbithole_html::parse(&html);
        let selectors = parse("li:nth-child(odd of :nth-child(odd of .item))");
        let state = MatchState::default();
        let matcher = SelectorMatcher::new(&doc, &state);
        let count = doc.descendants(0).filter(|&n| matcher.matches(&selectors[0], n)).count();
        assert_eq!(count, 500);
        assert_eq!(matcher.rank_builds.get(), 2);
        assert_eq!(matcher.cached_nodes.get(), 3000);
    }

    #[test]
    fn nth_of_pass_caches_do_not_survive_mutation_or_dynamic_state_changes() {
        let mut doc = rabbithole_html::parse("<ul><li id=a class=item></li><li id=b class=item></li><li id=c class=item></li></ul>");
        let selectors = parse("li:nth-child(2 of .item), li:nth-child(1 of :hover)");
        let a = doc.element_by_id("a").unwrap();
        let b = doc.element_by_id("b").unwrap();
        let c = doc.element_by_id("c").unwrap();
        {
            let state = MatchState { hovered: Some(a), ..Default::default() };
            let matcher = SelectorMatcher::new(&doc, &state);
            assert!(matcher.matches(&selectors[0], b));
            assert!(matcher.matches(&selectors[1], a));
            assert!(!matcher.matches(&selectors[1], c));
        }
        doc.set_attr(a, "class", None);
        let state = MatchState { hovered: Some(c), ..Default::default() };
        let matcher = SelectorMatcher::new(&doc, &state);
        assert!(!matcher.matches(&selectors[0], b));
        assert!(matcher.matches(&selectors[0], c));
        assert!(!matcher.matches(&selectors[1], a));
        assert!(matcher.matches(&selectors[1], c));
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
