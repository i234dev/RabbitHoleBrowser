//! Bounded syntax for the first basic `clip-path` milestone.
//!
//! This module neither applies styles nor advertises rendering support. It
//! accepts finite px/percentage lengths and unitless zero only. Other units,
//! calculations, URLs, SVG geometry boxes and other shape functions are left
//! unsupported. Percentages remain unresolved until a reference box exists.

use crate::Token;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShapeLength {
    Px(f32),
    Percent(f32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShapeRadius {
    Length(ShapeLength),
    ClosestSide,
    FarthestSide,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapePosition {
    pub x: ShapeLength,
    pub y: ShapeLength,
}

impl ShapePosition {
    const CENTER: Self = Self { x: ShapeLength::Percent(50.0), y: ShapeLength::Percent(50.0) };
}

#[derive(Debug, Clone, PartialEq)]
pub enum BasicShape {
    Circle { radius: ShapeRadius, center: ShapePosition },
    Ellipse { x_radius: ShapeRadius, y_radius: ShapeRadius, center: ShapePosition },
    /// Offsets: top/right/bottom/left. Corners: TL/TR/BR/BL, (x,y).
    Inset { offsets: [ShapeLength; 4], corners: [(ShapeLength, ShapeLength); 4] },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CssWide {
    Inherit,
    Initial,
    Unset,
    Revert,
    RevertLayer,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClipPathValue {
    None,
    Wide(CssWide),
    /// The only supported geometry box is border-box, also the default.
    Shape(BasicShape),
}

fn ident(t: &Token, expected: &str) -> bool {
    matches!(t, Token::Ident(s) if s.eq_ignore_ascii_case(expected))
}

fn length(t: &Token) -> Option<ShapeLength> {
    match t {
        Token::Number(n) if *n == 0.0 => Some(ShapeLength::Px(0.0)),
        Token::Dimension(n, unit) if n.is_finite() && unit.eq_ignore_ascii_case("px") => Some(ShapeLength::Px(*n)),
        Token::Percentage(n) if n.is_finite() => Some(ShapeLength::Percent(*n)),
        _ => None,
    }
}

fn positive_length(t: &Token) -> Option<ShapeLength> {
    let v = length(t)?;
    let (ShapeLength::Px(n) | ShapeLength::Percent(n)) = v;
    (n >= 0.0).then_some(v)
}

fn radius(t: &Token) -> Option<ShapeRadius> {
    if ident(t, "closest-side") {
        Some(ShapeRadius::ClosestSide)
    } else if ident(t, "farthest-side") {
        Some(ShapeRadius::FarthestSide)
    } else {
        positive_length(t).map(ShapeRadius::Length)
    }
}

fn horizontal(t: &Token) -> Option<ShapeLength> {
    if ident(t, "left") { Some(ShapeLength::Percent(0.0)) }
    else if ident(t, "right") { Some(ShapeLength::Percent(100.0)) }
    else if ident(t, "center") { Some(ShapeLength::Percent(50.0)) }
    else { length(t) }
}

fn vertical(t: &Token) -> Option<ShapeLength> {
    if ident(t, "top") { Some(ShapeLength::Percent(0.0)) }
    else if ident(t, "bottom") { Some(ShapeLength::Percent(100.0)) }
    else if ident(t, "center") { Some(ShapeLength::Percent(50.0)) }
    else { length(t) }
}

fn position(tokens: &[&Token]) -> Option<ShapePosition> {
    match tokens {
        [a] if ident(a, "top") || ident(a, "bottom") => Some(ShapePosition { x: ShapeLength::Percent(50.0), y: vertical(a)? }),
        [a] => Some(ShapePosition { x: horizontal(a)?, y: ShapeLength::Percent(50.0) }),
        [a, b] => {
            // Numeric components have fixed x/y order. Keyword pairs may
            // instead be vertical/horizontal (e.g. "bottom left").
            if let (Some(x), Some(y)) = (horizontal(a), vertical(b)) {
                return Some(ShapePosition { x, y });
            }
            if matches!(a, Token::Ident(_)) && matches!(b, Token::Ident(_)) {
                Some(ShapePosition { x: horizontal(b)?, y: vertical(a)? })
            } else {
                None
            }
        }
        _ => None,
    }
}

fn four<T: Copy>(values: &[T]) -> Option<[T; 4]> {
    match values {
        [a] => Some([*a; 4]),
        [a, b] => Some([*a, *b, *a, *b]),
        [a, b, c] => Some([*a, *b, *c, *b]),
        [a, b, c, d] => Some([*a, *b, *c, *d]),
        _ => None,
    }
}

fn inset(tokens: &[&Token]) -> Option<BasicShape> {
    let round = tokens.iter().position(|t| ident(t, "round")).unwrap_or(tokens.len());
    let offsets = four(&tokens[..round].iter().map(|t| length(t)).collect::<Option<Vec<_>>>()?)?;
    let zero = ShapeLength::Px(0.0);
    let mut corners = [(zero, zero); 4];
    if round < tokens.len() {
        let radii = &tokens[round + 1..];
        let slash = radii.iter().position(|t| matches!(t, Token::Delim('/'))).unwrap_or(radii.len());
        let h = four(&radii[..slash].iter().map(|t| positive_length(t)).collect::<Option<Vec<_>>>()?)?;
        let v = if slash < radii.len() {
            four(&radii[slash + 1..].iter().map(|t| positive_length(t)).collect::<Option<Vec<_>>>()?)?
        } else { h };
        for i in 0..4 { corners[i] = (h[i], v[i]); }
    }
    Some(BasicShape::Inset { offsets, corners })
}

fn shape(name: &str, tokens: &[&Token]) -> Option<BasicShape> {
    if name.eq_ignore_ascii_case("inset") { return inset(tokens); }
    let at = tokens.iter().position(|t| ident(t, "at")).unwrap_or(tokens.len());
    let center = if at < tokens.len() { position(&tokens[at + 1..])? } else { ShapePosition::CENTER };
    let radii = &tokens[..at];
    if name.eq_ignore_ascii_case("circle") {
        let radius = match radii { [] => ShapeRadius::ClosestSide, [r] => radius(r)?, _ => return None };
        Some(BasicShape::Circle { radius, center })
    } else if name.eq_ignore_ascii_case("ellipse") {
        let (x_radius, y_radius) = match radii {
            [] => (ShapeRadius::ClosestSide, ShapeRadius::ClosestSide),
            [x, y] => (radius(x)?, radius(y)?),
            _ => return None,
        };
        Some(BasicShape::Ellipse { x_radius, y_radius, center })
    } else { None }
}

/// Parse the entire bounded value. Failure has no side effects and does not
/// mean that a successfully parsed value is implemented by the renderer.
pub fn parse_clip_path(tokens: &[Token]) -> Option<ClipPathValue> {
    // The longest supported grammar is inset(4 offsets round 4 radii / 4
    // radii), plus border-box: 17 non-whitespace tokens. Reject extensions
    // before allocating a vector proportional to an untrusted value.
    let mut significant = Vec::with_capacity(17);
    for token in tokens.iter().filter(|t| !matches!(t, Token::Whitespace)) {
        if significant.len() == 17 { return None; }
        significant.push(token);
    }
    let mut tokens = significant;
    if let [Token::Ident(s)] = tokens.as_slice() {
        return match s.to_ascii_lowercase().as_str() {
            "none" => Some(ClipPathValue::None),
            "inherit" => Some(ClipPathValue::Wide(CssWide::Inherit)),
            "initial" => Some(ClipPathValue::Wide(CssWide::Initial)),
            "unset" => Some(ClipPathValue::Wide(CssWide::Unset)),
            "revert" => Some(ClipPathValue::Wide(CssWide::Revert)),
            "revert-layer" => Some(ClipPathValue::Wide(CssWide::RevertLayer)),
            _ => None,
        };
    }
    // <basic-shape> || <geometry-box>: border-box may precede or follow,
    // but the complete remainder must be exactly one supported function.
    if tokens.first().is_some_and(|t| ident(t, "border-box")) { tokens.remove(0); }
    else if tokens.last().is_some_and(|t| ident(t, "border-box")) { tokens.pop(); }
    let (Token::Function(name), Token::CloseParen) = (*tokens.first()?, *tokens.last()?) else { return None };
    Some(ClipPathValue::Shape(shape(name, &tokens[1..tokens.len() - 1])?))
}

#[cfg(test)]
mod tests;
