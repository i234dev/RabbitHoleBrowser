//! Container queries (CSS Conditional 5 §6): the `@container` prelude and
//! its evaluation against the query containers found by layout.
//!
//! Sizes come from the previous layout ([`MatchState::containers`]): the
//! first style pass of a document sees no containers (queries are
//! unknown, so false) and the engine restyles once layout has measured
//! them.

use crate::selector::MatchState;
use crate::tokenizer::Token;
use rabbithole_dom::{Document, NodeId};

/// A query container measured by layout.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ContainerInfo {
    /// Content-box size in px.
    pub width: f32,
    pub height: f32,
    /// `container-type: size` (both axes) rather than `inline-size`.
    pub both_axes: bool,
    /// Vertical writing mode: the inline axis is vertical.
    pub vertical: bool,
    /// `container-name`s.
    pub names: Vec<String>,
    /// Computed `font-size` (for `em` in queries).
    pub font_size: f32,
}

impl ContainerInfo {
    /// The inline and block sizes, when queryable.
    fn inline_size(&self) -> f32 {
        if self.vertical { self.height } else { self.width }
    }

    fn block_size(&self) -> Option<f32> {
        self.both_axes.then(|| if self.vertical { self.width } else { self.height })
    }

    fn physical(&self, horizontal: bool) -> Option<f32> {
        if horizontal != self.vertical { Some(self.inline_size()) } else { self.block_size() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Feature {
    Width,
    Height,
    InlineSize,
    BlockSize,
    AspectRatio,
    Orientation,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    Lt,
    Le,
    Eq,
    Ge,
    Gt,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A length (value, unit) or a number (`unit` empty).
    Dim(f32, String),
    /// `a / b`.
    Ratio(f32),
    Ident(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    Not(Box<Condition>),
    And(Vec<Condition>),
    Or(Vec<Condition>),
    /// `feature op value` comparisons that must all hold (`(a < width < b)`
    /// is two), or a boolean test of the feature (`(width)`).
    Size(Feature, Vec<(Op, Value)>),
    /// `style(--name: value)` / `style(--name)`: a custom property of the
    /// container (whitespace-insensitive token comparison).
    Style(String, Option<Vec<Token>>),
    /// Unknown syntax (`general-enclosed`), style queries on standard
    /// properties and scroll-state queries: unknown, never true.
    Unknown,
}

/// One `@container [name] <condition>`.
#[derive(Debug, Clone, PartialEq)]
pub struct ContainerQuery {
    pub name: Option<String>,
    pub condition: Condition,
}

/// Parse an `@container` prelude; `None` when it is invalid (the rule is
/// then dropped).
pub fn parse(prelude: &[Token]) -> Option<ContainerQuery> {
    let toks: Vec<&Token> = prelude.iter().filter(|t| !matches!(t, Token::Whitespace)).collect();
    let mut i = 0;
    let mut name = None;
    if let Some(Token::Ident(n)) = toks.first() {
        let l = n.to_ascii_lowercase();
        if !matches!(l.as_str(), "not" | "and" | "or" | "none") {
            name = Some(n.clone());
            i = 1;
        } else if l == "none" {
            return None;
        }
    }
    if i == toks.len() {
        return None;
    }
    let mut p = Parser { toks: &toks, pos: i, style: false };
    let condition = p.condition()?;
    (p.pos == toks.len()).then_some(ContainerQuery { name, condition })
}

struct Parser<'a, 'b> {
    toks: &'b [&'a Token],
    pos: usize,
    /// Inside `style()`: parenthesized features are style features.
    style: bool,
}

impl<'a> Parser<'a, '_> {
    fn ident(&self) -> Option<String> {
        match self.toks.get(self.pos) {
            Some(Token::Ident(s)) => Some(s.to_ascii_lowercase()),
            _ => None,
        }
    }

    fn condition(&mut self) -> Option<Condition> {
        if self.ident().as_deref() == Some("not") {
            self.pos += 1;
            return Some(Condition::Not(Box::new(self.in_parens()?)));
        }
        let first = self.in_parens()?;
        let Some(op) = self.ident().filter(|w| w == "and" || w == "or") else { return Some(first) };
        let mut list = vec![first];
        while self.ident().as_deref() == Some(op.as_str()) {
            self.pos += 1;
            list.push(self.in_parens()?);
        }
        Some(if op == "and" { Condition::And(list) } else { Condition::Or(list) })
    }

    /// The tokens up to the parenthesis closing the one just consumed.
    fn group(&mut self) -> Option<Vec<&'a Token>> {
        let mut depth = 1;
        let start = self.pos;
        while self.pos < self.toks.len() {
            match self.toks[self.pos] {
                Token::OpenParen | Token::Function(_) => depth += 1,
                Token::CloseParen => {
                    depth -= 1;
                    if depth == 0 {
                        let inner = self.toks[start..self.pos].to_vec();
                        self.pos += 1;
                        return Some(inner);
                    }
                }
                _ => {}
            }
            self.pos += 1;
        }
        None
    }

    fn in_parens(&mut self) -> Option<Condition> {
        match self.toks.get(self.pos)? {
            Token::OpenParen => {
                self.pos += 1;
                let inner = self.group()?;
                let starts_condition = matches!(inner.first(), Some(Token::OpenParen))
                    || matches!(inner.first(), Some(Token::Ident(s)) if s.eq_ignore_ascii_case("not"));
                if starts_condition {
                    let mut sub = Parser { toks: &inner, pos: 0, style: self.style };
                    if let Some(c) = sub.condition().filter(|_| sub.pos == inner.len()) {
                        return Some(c);
                    }
                    return Some(Condition::Unknown);
                }
                if self.style {
                    return Some(style_feature(&inner).unwrap_or(Condition::Unknown));
                }
                Some(feature(&inner).unwrap_or(Condition::Unknown))
            }
            Token::Function(f) if !self.style && f.eq_ignore_ascii_case("style") => {
                // `style(<style-query>)`: one feature or a condition.
                self.pos += 1;
                let inner = self.group()?;
                if !matches!(inner.first(), Some(Token::OpenParen)) && !matches!(inner.first(), Some(Token::Ident(s)) if s.eq_ignore_ascii_case("not")) {
                    return Some(style_feature(&inner).unwrap_or(Condition::Unknown));
                }
                let mut sub = Parser { toks: &inner, pos: 0, style: true };
                Some(sub.condition().filter(|_| sub.pos == inner.len()).unwrap_or(Condition::Unknown))
            }
            Token::Function(_) => {
                // scroll-state(), ...: general-enclosed.
                self.pos += 1;
                self.group()?;
                Some(Condition::Unknown)
            }
            _ => None,
        }
    }
}

fn feature_name(s: &str) -> Option<(Feature, Option<Op>)> {
    let (prefix, base) = if let Some(b) = s.strip_prefix("min-") {
        (Some(Op::Ge), b)
    } else if let Some(b) = s.strip_prefix("max-") {
        (Some(Op::Le), b)
    } else {
        (None, s)
    };
    let f = match base {
        "width" => Feature::Width,
        "height" => Feature::Height,
        "inline-size" => Feature::InlineSize,
        "block-size" => Feature::BlockSize,
        "aspect-ratio" => Feature::AspectRatio,
        "orientation" if prefix.is_none() => Feature::Orientation,
        _ => return None,
    };
    Some((f, prefix))
}

/// A value: `<number>`, `<dimension>`, `<ratio>` or an identifier.
fn value(toks: &[&Token]) -> Option<Value> {
    match toks {
        [Token::Dimension(v, u)] => Some(Value::Dim(*v, u.to_ascii_lowercase())),
        [Token::Number(v)] => Some(Value::Dim(*v, String::new())),
        [Token::Number(a), Token::Delim('/'), Token::Number(b)] if *b != 0.0 => Some(Value::Ratio(a / b)),
        [Token::Ident(s)] => Some(Value::Ident(s.to_ascii_lowercase())),
        _ => None,
    }
}

fn op_at(toks: &[&Token], i: usize) -> Option<(Op, usize)> {
    match (toks.get(i), toks.get(i + 1)) {
        (Some(Token::Delim('<')), Some(Token::Delim('='))) => Some((Op::Le, 2)),
        (Some(Token::Delim('>')), Some(Token::Delim('='))) => Some((Op::Ge, 2)),
        (Some(Token::Delim('<')), _) => Some((Op::Lt, 1)),
        (Some(Token::Delim('>')), _) => Some((Op::Gt, 1)),
        (Some(Token::Delim('=')), _) => Some((Op::Eq, 1)),
        _ => None,
    }
}

fn flip(op: Op) -> Op {
    match op {
        Op::Lt => Op::Gt,
        Op::Le => Op::Ge,
        Op::Gt => Op::Lt,
        Op::Ge => Op::Le,
        Op::Eq => Op::Eq,
    }
}

/// A style feature: `--name: value` or `--name` (custom properties only).
fn style_feature(inner: &[&Token]) -> Option<Condition> {
    match inner {
        [Token::Ident(n)] if n.starts_with("--") => Some(Condition::Style(n.clone(), None)),
        [Token::Ident(n), Token::Colon, rest @ ..] if n.starts_with("--") => {
            Some(Condition::Style(n.clone(), Some(rest.iter().map(|t| (*t).clone()).collect())))
        }
        _ => None,
    }
}

/// `(name)`, `(name: value)` or the range forms.
fn feature(inner: &[&Token]) -> Option<Condition> {
    if let [Token::Ident(n)] = inner {
        let (f, prefix) = feature_name(&n.to_ascii_lowercase())?;
        return prefix.is_none().then_some(Condition::Size(f, Vec::new()));
    }
    if let [Token::Ident(n), Token::Colon, rest @ ..] = inner {
        let (f, prefix) = feature_name(&n.to_ascii_lowercase())?;
        return Some(Condition::Size(f, vec![(prefix.unwrap_or(Op::Eq), value(rest)?)]));
    }
    // Range: `name op value`, `value op name` or `v1 op name op v2`.
    let ops: Vec<(usize, Op, usize)> = (0..inner.len()).filter_map(|i| op_at(inner, i).map(|(o, n)| (i, o, n))).collect();
    // Skip the `=` of `<=` / `>=` counted again on its own.
    let ops: Vec<(usize, Op, usize)> = ops.iter().copied().filter(|(i, _, _)| !ops.iter().any(|(j, _, n)| *n == 2 && j + 1 == *i)).collect();
    match ops.as_slice() {
        [(i, op, n)] => {
            let (left, right) = (&inner[..*i], &inner[i + n..]);
            if let [Token::Ident(name)] = left {
                let (f, None) = feature_name(&name.to_ascii_lowercase())? else { return None };
                return Some(Condition::Size(f, vec![(*op, value(right)?)]));
            }
            if let [Token::Ident(name)] = right {
                let (f, None) = feature_name(&name.to_ascii_lowercase())? else { return None };
                return Some(Condition::Size(f, vec![(flip(*op), value(left)?)]));
            }
            None
        }
        [(i, a, n), (j, b, m)] => {
            let (left, mid, right) = (&inner[..*i], &inner[i + n..*j], &inner[j + m..]);
            let [Token::Ident(name)] = mid else { return None };
            let (f, None) = feature_name(&name.to_ascii_lowercase())? else { return None };
            let same_direction = matches!((a, b), (Op::Lt | Op::Le, Op::Lt | Op::Le) | (Op::Gt | Op::Ge, Op::Gt | Op::Ge));
            if !same_direction {
                return None;
            }
            Some(Condition::Size(f, vec![(flip(*a), value(left)?), (*b, value(right)?)]))
        }
        _ => None,
    }
}

impl Condition {
    /// (uses size features, uses style features).
    fn uses(&self) -> (bool, bool) {
        match self {
            Condition::Not(c) => c.uses(),
            Condition::And(l) | Condition::Or(l) => l.iter().fold((false, false), |a, c| {
                let b = c.uses();
                (a.0 || b.0, a.1 || b.1)
            }),
            Condition::Size(..) => (true, false),
            Condition::Style(..) => (false, true),
            Condition::Unknown => (false, false),
        }
    }

    /// Kleene evaluation against a container's size (`None`: not a size
    /// container) and custom properties: `None` is unknown.
    fn eval(&self, size: Option<&ContainerInfo>, style: Option<(&dyn StyleSource, NodeId)>, env: &crate::MediaEnv) -> Option<bool> {
        match self {
            Condition::Not(x) => x.eval(size, style, env).map(|v| !v),
            Condition::And(l) => {
                let mut out = Some(true);
                for x in l {
                    match x.eval(size, style, env) {
                        Some(false) => return Some(false),
                        None => out = None,
                        _ => {}
                    }
                }
                out
            }
            Condition::Or(l) => {
                let mut out = Some(false);
                for x in l {
                    match x.eval(size, style, env) {
                        Some(true) => return Some(true),
                        None => out = None,
                        _ => {}
                    }
                }
                out
            }
            Condition::Unknown => None,
            Condition::Style(name, want) => {
                let (src, n) = style?;
                let have = src.custom_property(n, name)?;
                Some(match (want, have) {
                    (None, have) => have.is_some(),
                    (Some(w), Some(h)) => src.same_value(w, &h),
                    (Some(_), None) => false,
                })
            }
            Condition::Size(f, cmps) => {
                let c = size?;
                let actual = match f {
                    Feature::Width => c.physical(true)?,
                    Feature::Height => c.physical(false)?,
                    Feature::InlineSize => c.inline_size(),
                    Feature::BlockSize => c.block_size()?,
                    Feature::AspectRatio => {
                        let (w, h) = (c.physical(true)?, c.physical(false)?);
                        if h <= 0.0 {
                            return None;
                        }
                        w / h
                    }
                    Feature::Orientation => {
                        let (w, h) = (c.physical(true)?, c.physical(false)?);
                        let want = match cmps.first() {
                            Some((Op::Eq, Value::Ident(o))) => o.as_str(),
                            _ => return None,
                        };
                        return match want {
                            "portrait" => Some(h >= w),
                            "landscape" => Some(w > h),
                            _ => None,
                        };
                    }
                };
                if cmps.is_empty() {
                    return Some(actual != 0.0);
                }
                for (op, v) in cmps {
                    let want = match (f, v) {
                        (Feature::AspectRatio, Value::Ratio(r)) => *r,
                        (Feature::AspectRatio, Value::Dim(n, u)) if u.is_empty() => *n,
                        (Feature::AspectRatio, _) => return None,
                        (_, Value::Dim(n, u)) => length_px(*n, u, c, env)?,
                        _ => return None,
                    };
                    let ok = match op {
                        Op::Lt => actual < want,
                        Op::Le => actual <= want,
                        Op::Eq => actual == want,
                        Op::Ge => actual >= want,
                        Op::Gt => actual > want,
                    };
                    if !ok {
                        return Some(false);
                    }
                }
                Some(true)
            }
        }
    }
}

/// A query length in px; relative units use the container's font and
/// the viewport.
fn length_px(v: f32, unit: &str, c: &ContainerInfo, env: &crate::MediaEnv) -> Option<f32> {
    Some(match unit {
        "" if v == 0.0 => 0.0,
        "px" => v,
        "em" => v * c.font_size,
        "rem" => v * 16.0,
        "ex" => v * c.font_size * 0.5,
        "ch" => v * c.font_size * 0.5,
        "vw" | "svw" | "lvw" | "dvw" => v * env.width / 100.0,
        "vh" | "svh" | "lvh" | "dvh" => v * env.height / 100.0,
        "vmin" => v * env.width.min(env.height) / 100.0,
        "vmax" => v * env.width.max(env.height) / 100.0,
        "pt" => v * 4.0 / 3.0,
        "pc" => v * 16.0,
        "in" => v * 96.0,
        "cm" => v * 96.0 / 2.54,
        "mm" => v * 96.0 / 25.4,
        "q" => v * 96.0 / 101.6,
        _ => return None,
    })
}

/// The flat-tree parent element.
fn parent_element(doc: &Document, node: NodeId) -> Option<NodeId> {
    let p = doc.flat_parent(node)?;
    doc.element(p).map(|_| p)
}

/// Custom properties and container names of the elements styled so far
/// (style queries look at ancestors of the element being styled).
pub trait StyleSource {
    /// `container-name`s of `n`; `None` when it has no style yet.
    fn container_names(&self, n: NodeId) -> Option<Vec<String>>;
    /// The computed value of custom property `name` on `n`: `Some(None)`
    /// when it is not set, `None` when `n` has no style yet.
    fn custom_property(&self, n: NodeId, name: &str) -> Option<Option<Vec<Token>>>;
    /// Do two values compute to the same value (same tokens, or the same
    /// color)?
    fn same_value(&self, a: &[Token], b: &[Token]) -> bool {
        let strip = |v: &[Token]| -> Vec<Token> { v.iter().filter(|t| !matches!(t, Token::Whitespace)).cloned().collect() };
        strip(a) == strip(b)
    }
}

/// Does every query hold for `node` (`pseudo`: the rule styles one of its
/// pseudo-elements, so the element itself may be the container)?
pub fn matches(
    queries: &[ContainerQuery],
    doc: &Document,
    node: NodeId,
    pseudo: bool,
    state: &MatchState,
    style: Option<&dyn StyleSource>,
    env: &crate::MediaEnv,
) -> bool {
    queries.iter().all(|q| query_matches(q, doc, node, pseudo, state, style, env))
}

fn query_matches(q: &ContainerQuery, doc: &Document, node: NodeId, pseudo: bool, state: &MatchState, style: Option<&dyn StyleSource>, env: &crate::MediaEnv) -> bool {
    let (needs_size, needs_style) = q.condition.uses();
    if !needs_size && !needs_style {
        return false;
    }
    let mut cur = if pseudo { Some(node) } else { parent_element(doc, node) };
    while let Some(n) = cur {
        let size = state.containers.get(&n);
        let candidate = if needs_size {
            size.is_some_and(|c| q.name.as_ref().is_none_or(|name| c.names.iter().any(|x| x == name)))
        } else {
            // Every element is a style container.
            match &q.name {
                None => true,
                Some(name) => match style.and_then(|s| s.container_names(n)) {
                    Some(names) => names.iter().any(|x| x == name),
                    None => size.is_some_and(|c| c.names.iter().any(|x| x == name)),
                },
            }
        };
        if candidate {
            return q.condition.eval(size, style.map(|s| (s, n)), env) == Some(true);
        }
        cur = parent_element(doc, n);
    }
    false
}

/// The container `cq*` units of `node` resolve against: (width, height)
/// of the nearest container for each axis, else the viewport.
pub fn unit_basis(doc: &Document, node: NodeId, state: &MatchState, env: &crate::MediaEnv) -> (f32, f32) {
    let (mut w, mut h) = (None, None);
    if !state.containers.is_empty() {
        let mut cur = parent_element(doc, node);
        while let Some(n) = cur {
            if let Some(c) = state.containers.get(&n) {
                if w.is_none() {
                    w = c.physical(true);
                }
                if h.is_none() {
                    h = c.physical(false);
                }
                if w.is_some() && h.is_some() {
                    break;
                }
            }
            cur = parent_element(doc, n);
        }
    }
    (w.unwrap_or(env.width), h.unwrap_or(env.height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokenizer::Tokenizer;

    fn eval(prelude: &str, c: &ContainerInfo) -> Option<bool> {
        let q = parse(&Tokenizer::tokenize(prelude)).expect(prelude);
        let env = crate::MediaEnv { width: 800.0, height: 600.0, screen: true, scripting: true };
        q.condition.eval(Some(c), None, &env)
    }

    #[test]
    fn parse_and_evaluate() {
        let inline = ContainerInfo { width: 300.0, height: 100.0, both_axes: false, vertical: false, names: vec!["card".into()], font_size: 10.0 };
        assert_eq!(eval("(min-width: 200px)", &inline), Some(true));
        assert_eq!(eval("(width > 300px)", &inline), Some(false));
        assert_eq!(eval("(width >= 300px)", &inline), Some(true));
        assert_eq!(eval("(200px < width <= 300px)", &inline), Some(true));
        assert_eq!(eval("(400px > width > 300px)", &inline), Some(false));
        assert_eq!(eval("(30em <= inline-size)", &inline), Some(true));
        // An inline-size container cannot answer height queries.
        assert_eq!(eval("(height > 0px)", &inline), None);
        assert_eq!(eval("not (height > 0px)", &inline), None);
        assert_eq!(eval("(width > 0px) or (height > 0px)", &inline), Some(true));
        let size = ContainerInfo { both_axes: true, ..inline.clone() };
        assert_eq!(eval("(orientation: landscape) and (aspect-ratio > 2/1)", &size), Some(true));
        assert_eq!(eval("style(--x: 1)", &size), None);
        assert_eq!(eval("style(color: red)", &size), None);
        let q = parse(&Tokenizer::tokenize("--bar not style(--bar: baz)")).unwrap();
        assert_eq!(q.condition, Condition::Not(Box::new(Condition::Style("--bar".into(), Some(vec![Token::Ident("baz".into())])))));
        let q = parse(&Tokenizer::tokenize("card (width > 1px)")).unwrap();
        assert_eq!(q.name.as_deref(), Some("card"));
        assert!(parse(&Tokenizer::tokenize("none (width > 1px)")).is_none());
        assert!(parse(&Tokenizer::tokenize("card")).is_none());
    }
}
