//! An anchor query is a length whose reference is a different layout box.
//! Keep the query until both the anchor and its containing block have boxes.
use crate::computed::{intern_math, math_node, Length, MathNode};
use crate::values::{self, LengthContext};
use rabbithole_css::Token;

#[derive(Debug, Clone, PartialEq)]
pub enum Dimension {
    Side(&'static str),
    Percentage(f32),
    Size(Option<&'static str>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub name: Option<String>,
    pub dimension: Dimension,
    pub fallback: Option<Length>,
}

impl Query {
    pub fn css(&self) -> String {
        let (function, argument) = match self.dimension {
            Dimension::Size(size) => ("anchor-size", size.unwrap_or("").to_owned()),
            Dimension::Side(side) => ("anchor", side.to_owned()),
            Dimension::Percentage(p) => ("anchor", format!("{p}%")),
        };
        let main = [self.name.as_deref().unwrap_or(""), &argument].into_iter()
            .filter(|v| !v.is_empty()).collect::<Vec<_>>().join(" ");
        let fallback = self.fallback.map(|length| match length {
            Length::Px(v) => format!("{v}px"),
            Length::Percent(p) => format!("{p}%"),
            Length::Calc(px, p) => MathNode::Leaf(px,p).to_css(),
            Length::Math(id) => math_node(id).map(|node| node.to_css()).unwrap_or_default(),
            _ => String::new(),
        });
        match fallback {
            Some(f) if main.is_empty() => format!("{function}({f})"),
            Some(f) => format!("{function}({main}, {f})"),
            None => format!("{function}({main})"),
        }
    }
}

pub fn is_function(c: &[Token]) -> bool {
    matches!(c.first(), Some(Token::Function(f)) if f.eq_ignore_ascii_case("anchor") || f.eq_ignore_ascii_case("anchor-size"))
}

pub fn property(name: &str) -> Option<bool> {
    match name {
        "top" | "right" | "bottom" | "left" | "inset" | "inset-block" | "inset-inline"
        | "inset-block-start" | "inset-block-end" | "inset-inline-start" | "inset-inline-end" => Some(true),
        "margin" | "margin-top" | "margin-right" | "margin-bottom" | "margin-left" | "margin-block"
        | "margin-inline" | "margin-block-start" | "margin-block-end" | "margin-inline-start"
        | "margin-inline-end" | "width" | "height" | "min-width" | "min-height" | "max-width"
        | "max-height" | "inline-size" | "block-size" | "min-inline-size" | "max-inline-size"
        | "min-block-size" | "max-block-size" => Some(false),
        _ => None,
    }
}

fn parse(c: &[Token], inset: bool, ctx: &LengthContext, depth: usize) -> Option<Query> {
    let [Token::Function(f), body @ .., Token::CloseParen] = c else { return None };
    let size = f.eq_ignore_ascii_case("anchor-size");
    if depth >= 64 || !(size || inset && f.eq_ignore_ascii_case("anchor")) { return None; }
    let parts = values::components(body);
    let comma = parts.iter().position(|v| matches!(*v, [Token::Comma]));
    let mut main = &parts[..comma.unwrap_or(parts.len())];
    let mut tail = match comma {
        Some(i) => match &parts[i+1..] { [v] if !main.is_empty() => Some(*v), _ => return None },
        None => None,
    };
    if size && main.len() == 1 && tail.is_none() && !matches!(main[0], [Token::Ident(_)]) {
        tail = Some(main[0]); main = &[];
    }
    let mut name = None;
    let mut dimension = None;
    for &part in main {
        if let Some(n) = super::anchor::dashed_ident(part) {
            if name.replace(n.to_owned()).is_some() { return None; }
            continue;
        }
        let dim = if let [Token::Ident(k)] = part {
            if size {
                Dimension::Size(Some(*["width","height","block","inline","self-block","self-inline"]
                    .iter().find(|v| k.eq_ignore_ascii_case(v))?))
            } else {
                Dimension::Side(*["left","right","top","bottom","start","end","self-start","self-end",
                    "inside","outside","center"].iter().find(|v| k.eq_ignore_ascii_case(v))?)
            }
        } else if !size {
            if !part.iter().any(|t| matches!(t,Token::Percentage(_))) ||
                part.iter().any(|t| matches!(t,Token::Dimension(..))) { return None; }
            let percentage = values::parse_length(part)?.resolve(ctx).resolve(100.)?;
            Dimension::Percentage(percentage)
        } else { return None };
        if dimension.replace(dim).is_some() { return None; }
    }
    let dimension = dimension.or(if size { Some(Dimension::Size(None)) } else { None })?;
    let fallback = match tail {
        None => None,
        Some(v) if has_query(v) => Some(expression(v,inset,ctx,depth+1)?),
        Some(v) => {
            let value = values::parse_length(v)?;
            if value.is_auto() { return None; }
            Some(value.resolve(ctx))
        }
    };
    Some(Query {name, dimension, fallback})
}

fn expression(c: &[Token], inset: bool, ctx: &LengthContext, depth: usize) -> Option<Length> {
    if depth >= 64 { return None; }
    let node = if is_function(c) { MathNode::Anchor(parse(c,inset,ctx,depth)?) }
        else { values::anchor_math(c,ctx,&|v| parse(v,inset,ctx,depth+1))? };
    Some(Length::Math(intern_math(node)))
}

pub fn length(c: &[Token], inset: bool, ctx: &LengthContext) -> Option<Length> {
    if matches!(c.first(),Some(Token::Function(_))) && has_query(c) {
        return expression(c,inset,ctx,0);
    }
    values::parse_length(c).map(|v| v.resolve(ctx))
}

fn has_query(tokens: &[Token]) -> bool {
    tokens.iter().any(|t| matches!(t,Token::Function(f) if f.eq_ignore_ascii_case("anchor") || f.eq_ignore_ascii_case("anchor-size")))
}

/// Canonicalize the order of the name and side while retaining relative
/// units in a specified fallback. CSS-wide substitutions are handled by
/// the ordinary declaration parser.
pub fn specified(name: &str, tokens: &[Token], serialize: &impl Fn(&[Token]) -> String) -> Option<Option<String>> {
    if name.starts_with("--") || !has_query(tokens) || tokens.iter().any(|t| matches!(t,Token::Function(f) if
        f.eq_ignore_ascii_case("var") || f.eq_ignore_ascii_case("env") || f.eq_ignore_ascii_case("attr"))) { return None; }
    let ctx = LengthContext {font_size:16.,root_font_size:16.,viewport_w:800.,viewport_h:600.,ch:0.5,ex:0.5,ic:1.,lh:19.2};
    fn component(c: &[Token], inset: bool, ctx: &LengthContext, serialize: &impl Fn(&[Token])->String) -> Option<String> {
        if !is_function(c) {
            if has_query(c) {
                let Length::Math(id) = expression(c,inset,ctx,0)? else { return None };
                // Preserve font and viewport units in the specified value.
                if c.iter().any(|t| matches!(t, Token::Dimension(_,u) if !u.eq_ignore_ascii_case("px"))) {
                    return Some(serialize(c));
                }
                return Some(math_node(id)?.to_css());
            }
            values::parse_length(c)?;
            return Some(if matches!(c,[Token::Number(n)] if *n == 0.) { "0px".into() } else { serialize(c) });
        }
        let query = parse(c,inset,ctx,0)?;
        let parts = values::components(&c[1..c.len()-1]);
        let comma = parts.iter().position(|v| matches!(*v,[Token::Comma]));
        let main = &parts[..comma.unwrap_or(parts.len())];
        let function = if matches!(query.dimension,Dimension::Size(_)) { "anchor-size" } else { "anchor" };
        let mut args = query.name.into_iter().collect::<Vec<_>>();
        let mut fallback = comma.map(|i| parts[i+1]);
        for &v in main {
            if super::anchor::dashed_ident(v).is_some() { continue; }
            match &query.dimension {
                Dimension::Size(None) => fallback = Some(v),
                Dimension::Side(side) | Dimension::Size(Some(side)) => args.push((*side).into()),
                Dimension::Percentage(_) => args.push(serialize(v)),
            }
        }
        let main = args.join(" ");
        let arg = match fallback {
            Some(v) => {
                let f = component(v,inset,ctx,serialize)?;
                if main.is_empty() { f } else { format!("{main}, {f}") }
            }
            None => main,
        };
        Some(format!("{function}({arg})"))
    }
    Some((|| {
        let inset = property(name)?;
        let parts = values::components(tokens);
        let max = match name {
            "inset" | "margin" => 4,
            "inset-block" | "inset-inline" | "margin-inline" | "margin-block" => 2,
            _ => 1,
        };
        if parts.is_empty() || parts.len()>max { return None; }
        Some(parts.iter().map(|v| component(v,inset,&ctx,serialize)).collect::<Option<Vec<_>>>()?.join(" "))
    })())
}
