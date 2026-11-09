//! Property handlers beyond the core set in `cascade.rs`: flexbox, grid,
//! backgrounds, effects (radii/shadows/outline/transform), text, generated
//! content/counters and miscellaneous properties.

pub mod background;
pub mod content;
pub mod effects;
pub mod flex;
pub mod grid;
pub mod misc;
pub mod text;

use crate::computed::*;
use crate::values::{self, LengthContext};
use rabbithole_css::Token;
use std::ops::Deref;

/// Context for applying one declaration.
pub struct ApplyCtx<'a> {
    pub len: LengthContext,
    /// True when styling a pseudo-element (`content` is only honoured then).
    pub pseudo: bool,
    /// Base URL of the stylesheet the declaration came from.
    pub base: Option<&'a str>,
    pub document_base: &'a str,
    pub resolve: &'a dyn Fn(&str, &str) -> Option<String>,
}

impl Deref for ApplyCtx<'_> {
    type Target = LengthContext;
    fn deref(&self) -> &LengthContext {
        &self.len
    }
}

impl ApplyCtx<'_> {
    /// Resolve a `url()` value against the declaration's base.
    pub fn url(&self, raw: &str) -> String {
        let base = self.base.unwrap_or(self.document_base);
        (self.resolve)(base, raw).unwrap_or_else(|| raw.to_string())
    }
}

/// Try the extended property handlers. Returns true when `name` was
/// recognised (even if the value was invalid and ignored).
pub fn apply(name: &str, comps: &[&[Token]], value: &[Token], style: &mut ComputedStyle, parent: &ComputedStyle, ctx: &ApplyCtx) -> bool {
    flex::apply(name, comps, style, parent, ctx)
        || grid::apply(name, comps, value, style, ctx)
        || background::apply(name, comps, style, ctx)
        || effects::apply(name, comps, style, ctx)
        || text::apply(name, comps, value, style, parent, ctx)
        || content::apply(name, comps, style, ctx)
        || misc::apply(name, comps, style, ctx)
}

// ------------------------------------------------------------ shared helpers

/// A length that may be a percentage; `auto` rejected.
pub fn len_no_auto(c: &[Token], ctx: &LengthContext) -> Option<Length> {
    let l = values::parse_length(c)?;
    if l.is_auto() {
        return None;
    }
    Some(l.resolve(ctx))
}

/// A px length (percentages rejected), non-negative.
pub fn px(c: &[Token], ctx: &LengthContext) -> Option<f32> {
    match len_no_auto(c, ctx)? {
        Length::Px(v) => Some(v),
        _ => None,
    }
}

/// A px length allowing negatives (shadow offsets).
pub fn px_signed(c: &[Token], ctx: &LengthContext) -> Option<f32> {
    px(c, ctx)
}

/// Angle in radians from `deg`/`rad`/`grad`/`turn` dimensions (or `0`).
pub fn angle(c: &[Token]) -> Option<f32> {
    match c {
        [Token::Dimension(v, u)] => Some(match u.as_str() {
            "deg" => v.to_radians(),
            "rad" => *v,
            "grad" => v * std::f32::consts::PI / 200.0,
            "turn" => v * std::f32::consts::TAU,
            _ => return None,
        }),
        [Token::Number(n)] if *n == 0.0 => Some(0.0),
        _ => None,
    }
}

/// Time in seconds from `s`/`ms` dimensions.
pub fn seconds(c: &[Token]) -> Option<f32> {
    match c {
        [Token::Dimension(v, u)] => match u.as_str() {
            "s" => Some(*v),
            "ms" => Some(v / 1000.0),
            _ => None,
        },
        [Token::Number(n)] if *n == 0.0 => Some(0.0),
        _ => None,
    }
}

/// Arguments of a function component split on commas (whitespace dropped).
pub fn fn_args(c: &[Token]) -> Vec<Vec<&[Token]>> {
    if c.len() < 2 {
        return Vec::new();
    }
    let inner = if matches!(c.last(), Some(Token::CloseParen)) { &c[1..c.len() - 1] } else { &c[1..] };
    values::split_commas(&values::components(inner))
}

/// Name of a function component, lower-cased.
pub fn fn_name(c: &[Token]) -> Option<String> {
    match c.first() {
        Some(Token::Function(f)) => Some(f.to_ascii_lowercase()),
        _ => None,
    }
}

/// Parse a CSS `<position>` (1–4 values) into (x, y) lengths/percentages.
/// Edge offsets such as `right 10px` are approximated by the edge
/// percentage (the offset is dropped unless it is a percentage).
pub fn parse_position(comps: &[&[Token]], ctx: &LengthContext) -> Option<(Length, Length)> {
    #[derive(Clone, Copy, PartialEq)]
    enum Kw {
        Left,
        Right,
        Top,
        Bottom,
        Center,
    }
    enum V {
        Kw(Kw),
        Len(Length),
    }
    let mut vals = Vec::new();
    for c in comps {
        if let Some(kw) = values::ident(c) {
            let k = match kw.as_str() {
                "left" => Kw::Left,
                "right" => Kw::Right,
                "top" => Kw::Top,
                "bottom" => Kw::Bottom,
                "center" => Kw::Center,
                _ => return None,
            };
            vals.push(V::Kw(k));
        } else if let Some(l) = len_no_auto(c, ctx) {
            vals.push(V::Len(l));
        } else {
            return None;
        }
    }
    let kw_len = |k: Kw| match k {
        Kw::Left | Kw::Top => Length::Percent(0.0),
        Kw::Right | Kw::Bottom => Length::Percent(100.0),
        Kw::Center => Length::Percent(50.0),
    };
    let is_x = |k: Kw| matches!(k, Kw::Left | Kw::Right);
    let is_y = |k: Kw| matches!(k, Kw::Top | Kw::Bottom);
    match vals.len() {
        1 => Some(match vals[0] {
            V::Kw(k) if is_y(k) => (Length::Percent(50.0), kw_len(k)),
            V::Kw(k) => (kw_len(k), Length::Percent(50.0)),
            V::Len(l) => (l, Length::Percent(50.0)),
        }),
        2 => {
            let (a, b) = (&vals[0], &vals[1]);
            Some(match (a, b) {
                (V::Kw(ka), V::Kw(kb)) if is_y(*ka) || is_x(*kb) => (kw_len(*kb), kw_len(*ka)),
                (V::Kw(ka), V::Kw(kb)) => (kw_len(*ka), kw_len(*kb)),
                (V::Kw(ka), V::Len(l)) => (kw_len(*ka), *l),
                (V::Len(l), V::Kw(kb)) => (*l, kw_len(*kb)),
                (V::Len(l1), V::Len(l2)) => (*l1, *l2),
            })
        }
        3 | 4 => {
            // keyword [offset] keyword [offset]
            let mut x = Length::Percent(50.0);
            let mut y = Length::Percent(50.0);
            let mut i = 0;
            while i < vals.len() {
                let V::Kw(k) = vals[i] else { return None };
                let mut edge = kw_len(k);
                if let Some(V::Len(off)) = vals.get(i + 1) {
                    // `right 10%` → 90%; px offsets are dropped (approximation).
                    if let (Length::Percent(p), Length::Percent(o)) = (edge, *off) {
                        edge = Length::Percent(if p >= 100.0 { 100.0 - o } else { o });
                    } else if let (Length::Percent(p), Length::Px(_)) = (edge, *off) {
                        if p == 0.0 {
                            edge = *off;
                        }
                    }
                    i += 1;
                }
                if is_y(k) {
                    y = edge;
                } else {
                    x = edge;
                }
                i += 1;
            }
            Some((x, y))
        }
        _ => None,
    }
}

/// Expand 1–4 values to (top-left, top-right, bottom-right, bottom-left).
pub fn four_corners<T: Copy>(v: &[T]) -> Option<[T; 4]> {
    Some(match v.len() {
        1 => [v[0]; 4],
        2 => [v[0], v[1], v[0], v[1]],
        3 => [v[0], v[1], v[2], v[1]],
        4 => [v[0], v[1], v[2], v[3]],
        _ => return None,
    })
}
