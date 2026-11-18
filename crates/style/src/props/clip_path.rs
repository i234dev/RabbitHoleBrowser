//! `clip-path`: `<basic-shape>` functions (CSS Shapes 1/2), reference
//! boxes and `url()` references (CSS Masking 1 §3).

use super::ApplyCtx;
use crate::computed::*;
use crate::values::{self, LengthContext};
use rabbithole_css::Token;
use std::sync::Arc;

/// Apply `clip-path` / `-webkit-clip-path`. Invalid values are ignored.
pub fn apply(comps: &[&[Token]], style: &mut ComputedStyle, ctx: &ApplyCtx) {
    if let Some(v) = parse_clip_path(comps, ctx) {
        style.clip_path = v.map(Arc::new);
        if style.clip_path.is_some() {
            // CSS Masking 1 §3.1: a value other than `none` creates a
            // stacking context.
            style.creates_stacking_context = true;
        }
    }
}

/// `Some(None)` = `none`; `None` = invalid.
pub fn parse_clip_path(comps: &[&[Token]], ctx: &LengthContext) -> Option<Option<ClipPath>> {
    match comps {
        [] => None,
        [c] if values::is_ident(c, "none") => Some(None),
        [c] if crate::cascade::url_of(c).is_some() => {
            // Only same-document references (`#id`) can be resolved; other
            // URLs (external resources) clip nothing.
            let u = crate::cascade::url_of(c)?;
            let id = u.trim().strip_prefix('#').unwrap_or("").to_string();
            Some(Some(ClipPath::Url(id)))
        }
        _ => {
            let mut shape = None;
            let mut reference = None;
            for c in comps {
                if let Some(b) = values::ident(c).and_then(|k| shape_box(&k)) {
                    if reference.replace(b).is_some() {
                        return None;
                    }
                } else if super::fn_name(c).is_some() {
                    if shape.replace(parse_basic_shape(c, ctx)?).is_some() {
                        return None;
                    }
                } else {
                    return None;
                }
            }
            Some(Some(match shape {
                Some(s) => ClipPath::Shape(s, reference.unwrap_or(ShapeBox::Border)),
                None => ClipPath::Box(reference?),
            }))
        }
    }
}

/// Legacy `clip`: `auto` (`Some(None)`) or `rect()` with comma or space
/// separated offsets, each a length or `auto`; `None` = invalid.
pub fn parse_clip_rect(comps: &[&[Token]], ctx: &LengthContext) -> Option<Option<[Length; 4]>> {
    let [c] = comps else { return None };
    if values::is_ident(c, "auto") {
        return Some(None);
    }
    if super::fn_name(c)? != "rect" {
        return None;
    }
    // Either all four offsets separated by commas or none of them.
    let comps = inner_comps(c);
    let groups = values::split_commas(&comps);
    let args: Vec<&[Token]> = match groups.as_slice() {
        [all] => all.clone(),
        [_, _, _, _] if groups.iter().all(|g| g.len() == 1) => groups.iter().map(|g| g[0]).collect(),
        _ => return None,
    };
    let [t, r, b, l] = args.as_slice() else { return None };
    let one = |a: &[Token]| -> Option<Length> {
        if values::is_ident(a, "auto") {
            return Some(Length::Auto);
        }
        Some(Length::Px(super::px(a, ctx)?))
    };
    Some(Some([one(t)?, one(r)?, one(b)?, one(l)?]))
}

fn shape_box(k: &str) -> Option<ShapeBox> {
    Some(match k {
        "margin-box" => ShapeBox::Margin,
        "border-box" => ShapeBox::Border,
        "padding-box" => ShapeBox::Padding,
        "content-box" => ShapeBox::Content,
        "fill-box" => ShapeBox::Fill,
        "stroke-box" => ShapeBox::Stroke,
        "view-box" => ShapeBox::View,
        _ => return None,
    })
}

/// Components of a function's arguments (whitespace dropped, commas kept).
fn inner_comps(c: &[Token]) -> Vec<&[Token]> {
    if c.len() < 2 {
        return Vec::new();
    }
    let inner = if matches!(c.last(), Some(Token::CloseParen)) { &c[1..c.len() - 1] } else { &c[1..] };
    values::components(inner)
}

fn lp(c: &[Token], ctx: &LengthContext) -> Option<Length> {
    super::len_no_auto(c, ctx)
}

fn non_negative(l: Length) -> Option<Length> {
    match l {
        Length::Px(v) | Length::Percent(v) if v < 0.0 => None,
        l => Some(l),
    }
}

pub fn parse_basic_shape(c: &[Token], ctx: &LengthContext) -> Option<BasicShape> {
    let name = super::fn_name(c)?;
    let comps = inner_comps(c);
    match name.as_str() {
        "inset" => {
            let (vals, radii) = split_round(&comps, ctx)?;
            let v: Option<Vec<Length>> = vals.iter().map(|c| lp(c, ctx)).collect();
            let v = v?;
            let insets = match v.len() {
                1 => [v[0]; 4],
                2 => [v[0], v[1], v[0], v[1]],
                3 => [v[0], v[1], v[2], v[1]],
                4 => [v[0], v[1], v[2], v[3]],
                _ => return None,
            };
            Some(BasicShape::Inset { insets, radii })
        }
        "xywh" => {
            let (vals, radii) = split_round(&comps, ctx)?;
            let [x, y, w, h] = vals.as_slice() else { return None };
            Some(BasicShape::Xywh { x: lp(x, ctx)?, y: lp(y, ctx)?, w: non_negative(lp(w, ctx)?)?, h: non_negative(lp(h, ctx)?)?, radii })
        }
        "rect" => {
            let (vals, radii) = split_round(&comps, ctx)?;
            if vals.len() != 4 {
                return None;
            }
            let mut edges = [Length::Auto; 4];
            for (i, c) in vals.iter().enumerate() {
                if !values::is_ident(c, "auto") {
                    edges[i] = lp(c, ctx)?;
                }
            }
            Some(BasicShape::Rect { edges, radii })
        }
        "circle" => {
            let (before, at) = split_at_kw(&comps, ctx)?;
            let r = match before.as_slice() {
                [] => ShapeRadius::ClosestSide,
                [c] => shape_radius(c, ctx)?,
                _ => return None,
            };
            Some(BasicShape::Circle { r, at })
        }
        "ellipse" => {
            let (before, at) = split_at_kw(&comps, ctx)?;
            let (rx, ry) = match before.as_slice() {
                [] => (ShapeRadius::ClosestSide, ShapeRadius::ClosestSide),
                [a, b] => (shape_radius(a, ctx)?, shape_radius(b, ctx)?),
                _ => return None,
            };
            Some(BasicShape::Ellipse { rx, ry, at })
        }
        "polygon" => {
            let groups = values::split_commas(&comps);
            let mut evenodd = false;
            let mut round = 0.0;
            let mut rest = &groups[..];
            if let Some(first) = groups.first() {
                let is_point = first.len() == 2 && first.iter().all(|c| lp(c, ctx).is_some());
                if !is_point {
                    let mut i = 0;
                    if let Some(r) = first.first().and_then(|c| fill_rule(c)) {
                        evenodd = r;
                        i = 1;
                    }
                    if first.get(i).is_some_and(|c| values::is_ident(c, "round")) {
                        round = super::px(first.get(i + 1)?, ctx).filter(|v| *v >= 0.0)?;
                        i += 2;
                    }
                    if i == 0 || i != first.len() {
                        return None;
                    }
                    rest = &groups[1..];
                }
            }
            let mut points = Vec::new();
            for g in rest {
                let [x, y] = g.as_slice() else { return None };
                points.push((lp(x, ctx)?, lp(y, ctx)?));
            }
            if points.is_empty() {
                return None;
            }
            Some(BasicShape::Polygon { evenodd, points, round })
        }
        "path" => {
            let groups = values::split_commas(&comps);
            let (evenodd, d) = match groups.as_slice() {
                [d] => (false, d),
                [r, d] => (fill_rule(r.first()?)?, d),
                _ => return None,
            };
            let [[Token::String(s)]] = d.as_slice() else { return None };
            let path = rabbithole_gfx::path::parse_svg_path(s);
            Some(BasicShape::Path { evenodd, path })
        }
        "shape" => parse_shape_fn(&comps, ctx),
        _ => None,
    }
}

fn fill_rule(c: &[Token]) -> Option<bool> {
    match values::ident(c)?.as_str() {
        "evenodd" => Some(true),
        "nonzero" => Some(false),
        _ => None,
    }
}

fn shape_radius(c: &[Token], ctx: &LengthContext) -> Option<ShapeRadius> {
    if let Some(k) = values::ident(c) {
        return Some(match k.as_str() {
            "closest-side" => ShapeRadius::ClosestSide,
            "farthest-side" => ShapeRadius::FarthestSide,
            "closest-corner" => ShapeRadius::ClosestCorner,
            "farthest-corner" => ShapeRadius::FarthestCorner,
            _ => return None,
        });
    }
    Some(ShapeRadius::Len(non_negative(lp(c, ctx)?)?))
}

/// Split `... [at <position>]`; the position defaults to the center.
fn split_at_kw<'a>(comps: &[&'a [Token]], ctx: &LengthContext) -> Option<(Vec<&'a [Token]>, (Length, Length))> {
    match comps.iter().position(|c| values::is_ident(c, "at")) {
        Some(i) => Some((comps[..i].to_vec(), parse_position(&comps[i + 1..], ctx)?)),
        None => Some((comps.to_vec(), (Length::Percent(50.0), Length::Percent(50.0)))),
    }
}

/// Split `... [round <'border-radius'>]` into the values and the radii.
fn split_round<'a>(comps: &[&'a [Token]], ctx: &LengthContext) -> Option<(Vec<&'a [Token]>, [(Length, Length); 4])> {
    let zero = [(Length::ZERO, Length::ZERO); 4];
    let Some(i) = comps.iter().position(|c| values::is_ident(c, "round")) else {
        return Some((comps.to_vec(), zero));
    };
    let r = &comps[i + 1..];
    let slash = r.iter().position(|c| matches!(c, [Token::Delim('/')]));
    let (h, v) = match slash {
        Some(j) => (&r[..j], &r[j + 1..]),
        None => (r, r),
    };
    let rad = |c: &&[Token]| lp(c, ctx).and_then(non_negative);
    let hv: Vec<Length> = h.iter().map(rad).collect::<Option<_>>()?;
    let vv: Vec<Length> = v.iter().map(rad).collect::<Option<_>>()?;
    let (h4, v4) = (super::four_corners(&hv)?, super::four_corners(&vv)?);
    let mut out = zero;
    for k in 0..4 {
        out[k] = (h4[k], v4[k]);
    }
    Some((comps[..i].to_vec(), out))
}

/// `100% - off` for an offset from the far edge.
fn from_far_edge(off: Length) -> Option<Length> {
    Some(match off {
        Length::Px(v) => Length::Calc(-v, 100.0),
        Length::Percent(p) => Length::Percent(100.0 - p),
        Length::Calc(px, p) => Length::Calc(-px, 100.0 - p),
        _ => return None,
    })
}

/// A `<position>` (1–4 values) as box-relative (x, y), edge offsets
/// included (`right 10px` = `calc(100% - 10px)`).
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
        if let Some(k) = values::ident(c) {
            vals.push(V::Kw(match k.as_str() {
                "left" | "x-start" => Kw::Left,
                "right" | "x-end" => Kw::Right,
                "top" | "y-start" => Kw::Top,
                "bottom" | "y-end" => Kw::Bottom,
                "center" => Kw::Center,
                _ => return None,
            }));
        } else {
            vals.push(V::Len(lp(c, ctx)?));
        }
    }
    let pct = |k: Kw| match k {
        Kw::Left | Kw::Top => Length::Percent(0.0),
        Kw::Right | Kw::Bottom => Length::Percent(100.0),
        Kw::Center => Length::Percent(50.0),
    };
    let is_x = |k: Kw| matches!(k, Kw::Left | Kw::Right);
    let is_y = |k: Kw| matches!(k, Kw::Top | Kw::Bottom);
    let c50 = Length::Percent(50.0);
    match vals.as_slice() {
        [V::Kw(k)] if is_y(*k) => Some((c50, pct(*k))),
        [V::Kw(k)] => Some((pct(*k), c50)),
        [V::Len(l)] => Some((*l, c50)),
        [V::Kw(a), V::Kw(b)] => {
            if is_y(*a) || is_x(*b) {
                if is_y(*b) || is_x(*a) {
                    return None;
                }
                Some((pct(*b), pct(*a)))
            } else {
                Some((pct(*a), pct(*b)))
            }
        }
        [V::Kw(a), V::Len(l)] if !is_y(*a) => Some((pct(*a), *l)),
        [V::Len(l), V::Kw(b)] if !is_x(*b) => Some((*l, pct(*b))),
        [V::Len(a), V::Len(b)] => Some((*a, *b)),
        _ if vals.len() == 3 || vals.len() == 4 => {
            // keyword [offset] keyword [offset]
            let mut pairs = Vec::new();
            let mut i = 0;
            while i < vals.len() {
                let V::Kw(k) = vals[i] else { return None };
                let mut v = pct(k);
                if let Some(V::Len(off)) = vals.get(i + 1) {
                    if k == Kw::Center {
                        return None;
                    }
                    v = if matches!(k, Kw::Right | Kw::Bottom) { from_far_edge(*off)? } else { *off };
                    i += 1;
                }
                pairs.push((k, v));
                i += 1;
            }
            let [a, b] = pairs.as_slice() else { return None };
            let (px, py) = if is_y(a.0) || is_x(b.0) { (b, a) } else { (a, b) };
            if is_y(px.0) || is_x(py.0) {
                return None;
            }
            Some((px.1, py.1))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------- shape()

fn parse_shape_fn(comps: &[&[Token]], ctx: &LengthContext) -> Option<BasicShape> {
    let groups = values::split_commas(comps);
    let (head, cmds) = groups.split_first()?;
    let mut i = 0;
    let mut evenodd = false;
    if let Some(r) = head.first().and_then(|c| fill_rule(c)) {
        evenodd = r;
        i = 1;
    }
    if !head.get(i).is_some_and(|c| values::is_ident(c, "from")) {
        return None;
    }
    let from = parse_position(&head[i + 1..], ctx)?;
    if cmds.is_empty() {
        return None;
    }
    let cmds: Vec<ShapeCommand> = cmds.iter().map(|g| shape_command(g, ctx)).collect::<Option<_>>()?;
    Some(BasicShape::Shape { evenodd, from, cmds })
}

fn to_or_by(c: Option<&&[Token]>) -> Option<bool> {
    match values::ident(c?)?.as_str() {
        "to" => Some(false),
        "by" => Some(true),
        _ => None,
    }
}

/// End point: `to <position>` or `by <coordinate-pair>`.
fn end_point(by: bool, comps: &[&[Token]], ctx: &LengthContext) -> Option<(Length, Length)> {
    if by {
        let [x, y] = comps else { return None };
        Some((lp(x, ctx)?, lp(y, ctx)?))
    } else {
        parse_position(comps, ctx)
    }
}

fn control_point(by: bool, comps: &[&[Token]], ctx: &LengthContext) -> Option<ControlPoint> {
    if let Some(f) = comps.iter().position(|c| values::is_ident(c, "from")) {
        let [x, y] = &comps[..f] else { return None };
        let [a] = &comps[f + 1..] else { return None };
        let anchor = match values::ident(a)?.as_str() {
            "start" => ControlAnchor::Start,
            "end" => ControlAnchor::End,
            "origin" => ControlAnchor::Origin,
            _ => return None,
        };
        return Some(ControlPoint { x: lp(x, ctx)?, y: lp(y, ctx)?, anchor });
    }
    if by {
        let [x, y] = comps else { return None };
        return Some(ControlPoint { x: lp(x, ctx)?, y: lp(y, ctx)?, anchor: ControlAnchor::Start });
    }
    let (x, y) = parse_position(comps, ctx)?;
    Some(ControlPoint { x, y, anchor: ControlAnchor::Origin })
}

fn shape_command(g: &[&[Token]], ctx: &LengthContext) -> Option<ShapeCommand> {
    let name = values::ident(g.first()?)?;
    if name == "close" {
        return (g.len() == 1).then_some(ShapeCommand::Close);
    }
    let by = to_or_by(g.get(1))?;
    let rest = &g[2..];
    match name.as_str() {
        "move" | "line" => {
            let (x, y) = end_point(by, rest, ctx)?;
            Some(if name == "move" { ShapeCommand::Move { by, x, y } } else { ShapeCommand::Line { by, x, y } })
        }
        "hline" | "vline" => {
            let [c] = rest else { return None };
            let v = match values::ident(c) {
                Some(k) if !by => match (name.as_str(), k.as_str()) {
                    ("hline", "left" | "x-start") | ("vline", "top" | "y-start") => Length::Percent(0.0),
                    (_, "center") => Length::Percent(50.0),
                    ("hline", "right" | "x-end") | ("vline", "bottom" | "y-end") => Length::Percent(100.0),
                    _ => return None,
                },
                _ => lp(c, ctx)?,
            };
            Some(if name == "hline" { ShapeCommand::HLine { by, x: v } } else { ShapeCommand::VLine { by, y: v } })
        }
        "curve" | "smooth" => {
            let w = rest.iter().position(|c| values::is_ident(c, "with"));
            let (pt, ctl) = match w {
                Some(w) => (&rest[..w], Some(&rest[w + 1..])),
                None => (rest, None),
            };
            let (x, y) = end_point(by, pt, ctx)?;
            if name == "curve" {
                let ctl = ctl?;
                let slash = ctl.iter().position(|c| matches!(c, [Token::Delim('/')]));
                let (c1, c2) = match slash {
                    Some(s) => (control_point(by, &ctl[..s], ctx)?, Some(control_point(by, &ctl[s + 1..], ctx)?)),
                    None => (control_point(by, ctl, ctx)?, None),
                };
                Some(ShapeCommand::Curve { by, x, y, c1, c2 })
            } else {
                let c = match ctl {
                    Some(ctl) => Some(control_point(by, ctl, ctx)?),
                    None => None,
                };
                Some(ShapeCommand::Smooth { by, x, y, c })
            }
        }
        "arc" => {
            let of = rest.iter().position(|c| values::is_ident(c, "of"))?;
            let (x, y) = end_point(by, &rest[..of], ctx)?;
            let mut j = of + 1;
            let mut radii = Vec::new();
            while let Some(l) = rest.get(j).and_then(|c| lp(c, ctx)) {
                radii.push(non_negative(l)?);
                j += 1;
            }
            let (rx, ry) = match radii.as_slice() {
                [r] => (*r, None),
                [a, b] => (*a, Some(*b)),
                _ => return None,
            };
            let (mut cw, mut large, mut rotation) = (false, false, 0.0);
            while j < rest.len() {
                match values::ident(rest[j])?.as_str() {
                    "cw" => cw = true,
                    "ccw" => cw = false,
                    "large" => large = true,
                    "small" => large = false,
                    "rotate" => {
                        j += 1;
                        rotation = super::angle(rest.get(j)?)?;
                    }
                    _ => return None,
                }
                j += 1;
            }
            Some(ShapeCommand::Arc { by, x, y, rx, ry, cw, large, rotation })
        }
        _ => None,
    }
}
