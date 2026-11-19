//! Border radii, shadows, outline, transforms, filters and stacking hints.

use super::ApplyCtx;
use crate::computed::*;
use crate::values::{self, ColorValue, LengthContext};
use rabbithole_css::Token;
use rabbithole_gfx::Color;

fn color(c: &[Token], current: Color) -> Option<Color> {
    match values::parse_color_component(c)? {
        ColorValue::Color(col) => Some(col),
        ColorValue::CurrentColor => Some(current),
    }
}

/// A `<filter-value-list>` (`url()` references are not supported: the
/// whole value is then ignored).
fn parse_filters(comps: &[&[Token]], current: Color, ctx: &ApplyCtx) -> Option<Vec<FilterOp>> {
    let mut out = Vec::new();
    for c in comps {
        let f = super::fn_name(c)?;
        let args = super::fn_args(c);
        let a0: Option<&[Token]> = args.first().and_then(|a| a.first()).copied();
        let amount = |default: f32| -> Option<f32> {
            match a0 {
                None => Some(default),
                Some([Token::Number(n)]) if *n >= 0.0 => Some(*n),
                Some([Token::Percentage(p)]) if *p >= 0.0 => Some(p / 100.0),
                _ => None,
            }
        };
        let op = match f.as_str() {
            "blur" => FilterOp::Blur(match a0 {
                None => 0.0,
                Some(t) => super::px(t, ctx).filter(|v| *v >= 0.0)?,
            }),
            "brightness" => FilterOp::Brightness(amount(1.0)?),
            "contrast" => FilterOp::Contrast(amount(1.0)?),
            "grayscale" => FilterOp::Grayscale(amount(1.0)?.min(1.0)),
            "sepia" => FilterOp::Sepia(amount(1.0)?.min(1.0)),
            "saturate" => FilterOp::Saturate(amount(1.0)?),
            "invert" => FilterOp::Invert(amount(1.0)?.min(1.0)),
            "opacity" => FilterOp::Opacity(amount(1.0)?.min(1.0)),
            "hue-rotate" => FilterOp::HueRotate(match a0 {
                None => 0.0,
                Some(t) => super::angle(t)?,
            }),
            "drop-shadow" => {
                let parts = args.first()?;
                let mut lens = Vec::new();
                let mut col = None;
                for p in parts {
                    if let Some(v) = super::px(p, ctx) {
                        lens.push(v);
                    } else if let Some(cc) = color(p, current) {
                        col = Some(cc);
                    } else {
                        return None;
                    }
                }
                if lens.len() < 2 || lens.len() > 3 {
                    return None;
                }
                FilterOp::DropShadow { x: lens[0], y: lens[1], blur: lens.get(2).copied().unwrap_or(0.0).max(0.0), color: col.unwrap_or(current) }
            }
            _ => return None,
        };
        out.push(op);
    }
    (!out.is_empty()).then_some(out)
}

/// Largest px radius, for the compatibility `border_radius` field.
fn max_px_radius(radii: &[(Length, Length); 4], font_size: f32) -> f32 {
    let mut m: f32 = 0.0;
    for (h, v) in radii {
        for l in [h, v] {
            let px = match l {
                Length::Px(v) => *v,
                Length::Percent(p) => p / 100.0 * font_size * 2.0,
                _ => 0.0,
            };
            m = m.max(px);
        }
    }
    m
}

/// A `<corner-shape-value>`; `superellipse(k)` maps to the nearest keyword.
fn corner_shape(c: &[Token]) -> Option<CornerShape> {
    if let Some(k) = values::ident(c) {
        return Some(match k.as_str() {
            "round" => CornerShape::Round,
            "square" => CornerShape::Square,
            "bevel" => CornerShape::Bevel,
            "notch" => CornerShape::Notch,
            "scoop" => CornerShape::Scoop,
            "squircle" => CornerShape::Squircle,
            _ => return None,
        });
    }
    if super::fn_name(c)? != "superellipse" {
        return None;
    }
    let args = super::fn_args(c);
    let [arg] = args.as_slice() else { return None };
    let [a] = arg.as_slice() else { return None };
    let k = match values::ident(a).as_deref() {
        Some("infinity") => f32::INFINITY,
        Some("-infinity") => f32::NEG_INFINITY,
        _ => values::number(a)?,
    };
    Some(match k {
        k if k >= 4.0 => CornerShape::Square,
        k if k >= 1.5 => CornerShape::Squircle,
        k if k >= 0.5 => CornerShape::Round,
        k if k > -0.5 => CornerShape::Bevel,
        k if k > -4.0 => CornerShape::Scoop,
        _ => CornerShape::Notch,
    })
}

fn radius_len(c: &[Token], ctx: &LengthContext) -> Option<Length> {
    let l = super::len_no_auto(c, ctx)?;
    Some(match l {
        Length::Px(v) => Length::Px(v.max(0.0)),
        Length::Percent(p) => Length::Percent(p.max(0.0)),
        other => other,
    })
}

/// Parse a comma separated shadow list. `allow_spread_inset` for box-shadow.
fn parse_shadows(comps: &[&[Token]], current: Color, ctx: &LengthContext, allow_spread_inset: bool) -> Option<Vec<Shadow>> {
    if comps.len() == 1 && values::is_ident(comps[0], "none") {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    for item in values::split_commas(comps) {
        let mut lens = Vec::new();
        let mut col = None;
        let mut inset = false;
        for c in item {
            if values::is_ident(c, "inset") {
                if !allow_spread_inset {
                    return None;
                }
                inset = true;
            } else if let Some(cc) = color(c, current) {
                col = Some(cc);
            } else if let Some(v) = super::px(c, ctx) {
                lens.push(v);
            } else {
                return None;
            }
        }
        let max = if allow_spread_inset { 4 } else { 3 };
        if lens.len() < 2 || lens.len() > max {
            return None;
        }
        out.push(Shadow {
            x: lens[0],
            y: lens[1],
            blur: lens.get(2).copied().unwrap_or(0.0).max(0.0),
            spread: lens.get(3).copied().unwrap_or(0.0),
            color: col.unwrap_or(current),
            inset,
        });
    }
    Some(out)
}

/// A `<number>` or `<percentage>` (as a fraction), for scales.
fn num_or_pct(c: &[Token]) -> Option<f32> {
    match c {
        [Token::Percentage(p)] => Some(p / 100.0),
        _ => values::number(c),
    }
}

fn parse_transform(comps: &[&[Token]], ctx: &LengthContext) -> Option<Vec<TransformOp>> {
    if comps.len() == 1 && values::is_ident(comps[0], "none") {
        return Some(Vec::new());
    }
    let mut ops = Vec::new();
    for c in comps {
        let name = super::fn_name(c)?;
        let args = super::fn_args(c);
        let arg = |i: usize| -> Option<&[Token]> { args.get(i).and_then(|a| if a.len() == 1 { a.first().copied() } else { None }) };
        let num = |i: usize| -> Option<f32> { arg(i).and_then(values::number) };
        let scl = |i: usize| -> Option<f32> { arg(i).and_then(num_or_pct) };
        let len = |i: usize| -> Option<Length> {
            let a = arg(i)?;
            if let Some(n) = values::number(a) {
                // Only a unitless zero is a length.
                return (n == 0.0).then_some(Length::Px(0.0));
            }
            super::len_no_auto(a, ctx)
        };
        // A length without percentages (z components).
        let zlen = |i: usize| -> Option<f32> {
            match len(i)? {
                Length::Px(v) => Some(v),
                _ => None,
            }
        };
        let ang = |i: usize| -> Option<f32> {
            let a = arg(i)?;
            // A unitless zero is an angle.
            if values::number(a) == Some(0.0) {
                return Some(0.0);
            }
            super::angle(a)
        };
        let n = args.len();
        let op = match (name.as_str(), n) {
            ("translate", 1) => TransformOp::Translate(len(0)?, Length::ZERO),
            ("translate", 2) => TransformOp::Translate(len(0)?, len(1)?),
            ("translatex", 1) => TransformOp::Translate(len(0)?, Length::ZERO),
            ("translatey", 1) => TransformOp::Translate(Length::ZERO, len(0)?),
            ("translatez", 1) => TransformOp::Translate3d(Length::ZERO, Length::ZERO, zlen(0)?),
            ("translate3d", 3) => TransformOp::Translate3d(len(0)?, len(1)?, zlen(2)?),
            ("scale", 1) => {
                let x = scl(0)?;
                TransformOp::Scale(x, x)
            }
            ("scale", 2) => TransformOp::Scale(scl(0)?, scl(1)?),
            ("scalex", 1) => TransformOp::Scale(scl(0)?, 1.0),
            ("scaley", 1) => TransformOp::Scale(1.0, scl(0)?),
            ("scalez", 1) => TransformOp::Scale3d(1.0, 1.0, scl(0)?),
            ("scale3d", 3) => TransformOp::Scale3d(scl(0)?, scl(1)?, scl(2)?),
            ("rotate" | "rotatez", 1) => TransformOp::Rotate(ang(0)?),
            ("rotatex", 1) => TransformOp::Rotate3d(1.0, 0.0, 0.0, ang(0)?),
            ("rotatey", 1) => TransformOp::Rotate3d(0.0, 1.0, 0.0, ang(0)?),
            ("rotate3d", 4) => TransformOp::Rotate3d(num(0)?, num(1)?, num(2)?, ang(3)?),
            ("perspective", 1) => {
                let a = arg(0)?;
                if values::is_ident(a, "none") {
                    TransformOp::Perspective(None)
                } else {
                    let d = zlen(0)?;
                    if d < 0.0 {
                        return None;
                    }
                    TransformOp::Perspective(Some(d))
                }
            }
            ("skew", 1) => TransformOp::Skew(ang(0)?, 0.0),
            ("skew", 2) => TransformOp::Skew(ang(0)?, ang(1)?),
            ("skewx", 1) => TransformOp::SkewX(ang(0)?),
            ("skewy", 1) => TransformOp::SkewY(ang(0)?),
            ("matrix", 6) => {
                let mut m = [0.0; 6];
                for (i, v) in m.iter_mut().enumerate() {
                    *v = num(i)?;
                }
                TransformOp::Matrix(m)
            }
            ("matrix3d", 16) => {
                let mut m = [0.0; 16];
                for (i, v) in m.iter_mut().enumerate() {
                    *v = num(i)?;
                }
                TransformOp::Matrix3d(Box::new(m))
            }
            _ => return None,
        };
        ops.push(op);
    }
    Some(ops)
}

/// The `translate` property: `none | <length-percentage> [<length-percentage> <length>?]?`.
fn parse_translate_prop(comps: &[&[Token]], ctx: &LengthContext) -> Option<Option<TransformOp>> {
    if comps.len() == 1 && values::is_ident(comps[0], "none") {
        return Some(None);
    }
    let len = |c: &[Token]| -> Option<Length> {
        if values::number(c) == Some(0.0) {
            return Some(Length::Px(0.0));
        }
        super::len_no_auto(c, ctx)
    };
    let (x, y, z) = match comps {
        [x] => (len(x)?, Length::ZERO, 0.0),
        [x, y] => (len(x)?, len(y)?, 0.0),
        [x, y, z] => match len(z)? {
            Length::Px(z) => (len(x)?, len(y)?, z),
            _ => return None,
        },
        _ => return None,
    };
    Some(Some(TransformOp::Translate3d(x, y, z)))
}

/// The `rotate` property: `none | <angle> | [x | y | z | <number>{3}] && <angle>`.
fn parse_rotate_prop(comps: &[&[Token]]) -> Option<Option<TransformOp>> {
    if comps.len() == 1 && values::is_ident(comps[0], "none") {
        return Some(None);
    }
    let mut kinds = String::new();
    let mut angle = 0.0;
    let mut axis = (0.0, 0.0, 1.0);
    let mut nums = Vec::new();
    for c in comps {
        if let Some(k) = values::ident(c) {
            axis = match k.as_str() {
                "x" => (1.0, 0.0, 0.0),
                "y" => (0.0, 1.0, 0.0),
                "z" => (0.0, 0.0, 1.0),
                _ => return None,
            };
            kinds.push('I');
        } else if let Some(n) = values::number(c) {
            nums.push(n);
            kinds.push('N');
        } else {
            angle = super::angle(c)?;
            kinds.push('A');
        }
    }
    match kinds.as_str() {
        "A" | "IA" | "AI" => {}
        "NNNA" | "ANNN" => axis = (nums[0], nums[1], nums[2]),
        _ => return None,
    }
    Some(Some(TransformOp::Rotate3d(axis.0, axis.1, axis.2, angle)))
}

/// The `scale` property: `none | [<number> | <percentage>]{1,3}`.
fn parse_scale_prop(comps: &[&[Token]]) -> Option<Option<TransformOp>> {
    if comps.len() == 1 && values::is_ident(comps[0], "none") {
        return Some(None);
    }
    let v: Option<Vec<f32>> = comps.iter().map(|c| num_or_pct(c)).collect();
    let v = v?;
    let (x, y, z) = match v.as_slice() {
        [x] => (*x, *x, 1.0),
        [x, y] => (*x, *y, 1.0),
        [x, y, z] => (*x, *y, *z),
        _ => return None,
    };
    Some(Some(TransformOp::Scale3d(x, y, z)))
}

pub fn apply(name: &str, comps: &[&[Token]], style: &mut ComputedStyle, ctx: &ApplyCtx) -> bool {
    let name = match name {
        "-webkit-border-radius" | "-moz-border-radius" => "border-radius",
        "-webkit-border-top-left-radius" | "-moz-border-radius-topleft" => "border-top-left-radius",
        "-webkit-border-top-right-radius" | "-moz-border-radius-topright" => "border-top-right-radius",
        "-webkit-border-bottom-right-radius" | "-moz-border-radius-bottomright" => "border-bottom-right-radius",
        "-webkit-border-bottom-left-radius" | "-moz-border-radius-bottomleft" => "border-bottom-left-radius",
        "-webkit-box-shadow" | "-moz-box-shadow" => "box-shadow",
        "-webkit-transform" | "-moz-transform" | "-ms-transform" | "-o-transform" => "transform",
        "-webkit-transform-origin" | "-moz-transform-origin" | "-ms-transform-origin" => "transform-origin",
        "-webkit-filter" => "filter",
        "-webkit-perspective" => "perspective",
        "-webkit-perspective-origin" => "perspective-origin",
        "-webkit-transform-style" => "transform-style",
        "-webkit-backface-visibility" => "backface-visibility",
        "-webkit-backdrop-filter" => "backdrop-filter",
        "-moz-outline" => "outline",
        n => n,
    };
    let current = style.color;
    let c0 = comps[0];
    match name {
        "border-radius" => {
            // horizontal [/ vertical]
            let slash = comps.iter().position(|c| matches!(c, [Token::Delim('/')]));
            let (h, v) = match slash {
                Some(i) => (&comps[..i], &comps[i + 1..]),
                None => (comps, comps),
            };
            let hv: Option<Vec<Length>> = h.iter().map(|c| radius_len(c, ctx)).collect();
            let vv: Option<Vec<Length>> = v.iter().map(|c| radius_len(c, ctx)).collect();
            let (Some(hv), Some(vv)) = (hv, vv) else { return true };
            let (Some(h4), Some(v4)) = (super::four_corners(&hv), super::four_corners(&vv)) else { return true };
            for i in 0..4 {
                style.border_radii[i] = (h4[i], v4[i]);
            }
            style.border_radius = max_px_radius(&style.border_radii, ctx.font_size);
        }
        "border-top-left-radius" | "border-top-right-radius" | "border-bottom-right-radius" | "border-bottom-left-radius" => {
            let vals: Option<Vec<Length>> = comps.iter().map(|c| radius_len(c, ctx)).collect();
            let Some(vals) = vals else { return true };
            let (h, v) = match vals.len() {
                1 => (vals[0], vals[0]),
                2 => (vals[0], vals[1]),
                _ => return true,
            };
            let idx = match name {
                "border-top-left-radius" => 0,
                "border-top-right-radius" => 1,
                "border-bottom-right-radius" => 2,
                _ => 3,
            };
            style.border_radii[idx] = (h, v);
            style.border_radius = max_px_radius(&style.border_radii, ctx.font_size);
        }
        "box-shadow" => {
            if let Some(s) = parse_shadows(comps, current, ctx, true) {
                style.box_shadow = s;
            }
        }
        "text-shadow" => {
            if let Some(s) = parse_shadows(comps, current, ctx, false) {
                style.text_shadow = s;
            }
        }
        "outline" => {
            let mut width = None;
            let mut ostyle = None;
            let mut col: Option<Option<Color>> = None;
            for c in comps {
                if let Some(kw) = values::ident(c) {
                    match kw.as_str() {
                        "none" | "hidden" => ostyle = Some(BorderStyle::None),
                        "auto" | "solid" | "inset" | "outset" | "groove" | "ridge" => ostyle = Some(BorderStyle::Solid),
                        "dashed" => ostyle = Some(BorderStyle::Dashed),
                        "dotted" => ostyle = Some(BorderStyle::Dotted),
                        "double" => ostyle = Some(BorderStyle::Double),
                        "invert" => col = Some(None),
                        "thin" => width = Some(1.0),
                        "medium" => width = Some(3.0),
                        "thick" => width = Some(5.0),
                        _ => {
                            if let Some(cc) = color(c, current) {
                                col = Some(Some(cc));
                            } else {
                                return true;
                            }
                        }
                    }
                } else if let Some(w) = super::px(c, ctx) {
                    width = Some(w);
                } else if let Some(cc) = color(c, current) {
                    col = Some(Some(cc));
                } else {
                    return true;
                }
            }
            style.outline_style = ostyle.unwrap_or(BorderStyle::None);
            style.outline_width = if style.outline_style.is_visible() { width.unwrap_or(3.0) } else { 0.0 };
            style.outline_color = col.unwrap_or(None);
        }
        "outline-width" => {
            let w = match values::ident(c0).as_deref() {
                Some("thin") => Some(1.0),
                Some("medium") => Some(3.0),
                Some("thick") => Some(5.0),
                Some(_) => None,
                None => super::px(c0, ctx),
            };
            if let Some(w) = w {
                style.outline_width = w;
            }
        }
        "outline-style" => {
            if let Some(kw) = values::ident(c0) {
                style.outline_style = match kw.as_str() {
                    "none" | "hidden" => BorderStyle::None,
                    "auto" | "solid" | "inset" | "outset" | "groove" | "ridge" => BorderStyle::Solid,
                    "dashed" => BorderStyle::Dashed,
                    "dotted" => BorderStyle::Dotted,
                    "double" => BorderStyle::Double,
                    _ => return true,
                };
                if style.outline_style.is_visible() && style.outline_width == 0.0 {
                    style.outline_width = 3.0;
                }
            }
        }
        "outline-color" => {
            if values::is_ident(c0, "invert") {
                style.outline_color = None;
            } else if let Some(cc) = color(c0, current) {
                style.outline_color = Some(cc);
            }
        }
        "outline-offset" => {
            if let Some(v) = super::px(c0, ctx) {
                style.outline_offset = v;
            }
        }
        "transform" => {
            if let Some(ops) = parse_transform(comps, ctx) {
                style.transform = ops;
            }
        }
        "transform-origin" => {
            // An optional trailing z length.
            let (xy, z) = match comps {
                [_, _, z] => match super::px(z, ctx) {
                    Some(z) => (&comps[..2], z),
                    None => return true,
                },
                _ => (comps, 0.0),
            };
            if let Some(p) = parse_origin(xy, ctx) {
                style.transform_origin = p;
                let t = style.transform3d_mut();
                t.origin_z = z;
                t.origin_specified = true;
            }
        }
        "perspective-origin" => {
            if let Some(p) = parse_origin(comps, ctx) {
                style.transform3d_mut().perspective_origin = p;
            }
        }
        "perspective" => {
            if values::is_ident(c0, "none") && comps.len() == 1 {
                if style.transform3d.is_some() {
                    style.transform3d_mut().perspective = None;
                }
            } else if let (Some(d), 1) = (super::px(c0, ctx), comps.len()) {
                if d >= 0.0 {
                    style.transform3d_mut().perspective = Some(d);
                }
            }
        }
        "transform-style" => match values::ident(c0).as_deref() {
            Some("preserve-3d") => style.transform3d_mut().preserve_3d = true,
            Some("flat") if style.transform3d.is_some() => style.transform3d_mut().preserve_3d = false,
            _ => {}
        },
        "backface-visibility" => match values::ident(c0).as_deref() {
            Some("hidden") => style.transform3d_mut().backface_hidden = true,
            Some("visible") if style.transform3d.is_some() => style.transform3d_mut().backface_hidden = false,
            _ => {}
        },
        "translate" => {
            if let Some(v) = parse_translate_prop(comps, ctx) {
                if v.is_some() || style.transform3d.is_some() {
                    style.transform3d_mut().translate = v;
                }
            }
        }
        "rotate" => {
            if let Some(v) = parse_rotate_prop(comps) {
                if v.is_some() || style.transform3d.is_some() {
                    style.transform3d_mut().rotate = v;
                }
            }
        }
        "scale" => {
            if let Some(v) = parse_scale_prop(comps) {
                if v.is_some() || style.transform3d.is_some() {
                    style.transform3d_mut().scale = v;
                }
            }
        }
        "filter" => {
            if values::is_ident(c0, "none") && comps.len() == 1 {
                style.filters.clear();
                return true;
            }
            let Some(ops) = parse_filters(comps, current, ctx) else { return true };
            // A lone opacity() is a plain opacity layer.
            if ops.iter().all(|o| matches!(o, FilterOp::Opacity(_))) {
                for o in &ops {
                    if let FilterOp::Opacity(v) = o {
                        style.opacity = (style.opacity * v).clamp(0.0, 1.0);
                    }
                }
                style.filters.clear();
            } else {
                style.filters = ops;
            }
            style.creates_stacking_context = true;
        }
        "backdrop-filter" => {
            if values::is_ident(c0, "none") && comps.len() == 1 {
                style.backdrop = false;
                style.backdrop_filters.clear();
                return true;
            }
            let Some(ops) = parse_filters(comps, current, ctx) else { return true };
            style.backdrop = true;
            style.backdrop_filters = ops;
            style.creates_stacking_context = true;
        }
        "isolation" => {
            if values::is_ident(c0, "isolate") {
                style.creates_stacking_context = true;
                style.isolate = true;
            } else if values::is_ident(c0, "auto") {
                style.isolate = false;
            }
        }
        "contain" | "will-change" | "mix-blend-mode" => {
            let kws: Vec<String> = comps.iter().filter_map(|c| values::ident(c)).collect();
            if name == "contain" && kws.iter().any(|k| matches!(k.as_str(), "paint" | "layout" | "strict" | "content")) {
                style.creates_stacking_context = true;
            }
            if name == "contain" {
                let has = |k: &str| kws.iter().any(|x| x == k);
                style.contain_paint = has("paint") || has("strict") || has("content");
                style.contain_layout = has("layout") || has("strict") || has("content");
                style.contain_size = has("size") || has("strict");
            }
            if name == "will-change" && kws.iter().any(|k| matches!(k.as_str(), "transform" | "opacity")) {
                style.creates_stacking_context = true;
            }
            if name == "mix-blend-mode" {
                if let [k] = kws.as_slice() {
                    if let Some(m) = rabbithole_gfx::blend_mode_index(k) {
                        style.mix_blend_mode = m;
                        style.creates_stacking_context = m != 0;
                    }
                }
            }
        }
        "clip-path" | "-webkit-clip-path" => super::clip_path::apply(comps, style, ctx),
        "border-shape" => {
            // A single shape (the two-shape form is not supported).
            match super::clip_path::parse_shape_value(comps, ctx, ShapeBox::HalfBorder) {
                Some(None) => style.border_shape = None,
                Some(Some(cp @ ClipPath::Shape(..))) => style.border_shape = Some(std::sync::Arc::new(cp)),
                _ => {}
            }
        }
        "corner-shape" => {
            let kws: Option<Vec<CornerShape>> = comps.iter().map(|c| corner_shape(c)).collect();
            if let Some(v) = kws.as_deref().and_then(super::four_corners) {
                style.corner_shape = v;
            }
        }
        "corner-top-left-shape" | "corner-top-right-shape" | "corner-bottom-right-shape" | "corner-bottom-left-shape" => {
            if let ([c], Some(i)) = (comps, ["corner-top-left-shape", "corner-top-right-shape", "corner-bottom-right-shape", "corner-bottom-left-shape"].iter().position(|n| *n == name)) {
                if let Some(k) = corner_shape(c) {
                    style.corner_shape[i] = k;
                }
            }
        }
        "clip" => {
            if let Some(v) = super::clip_path::parse_clip_rect(comps, ctx) {
                style.clip_rect = v;
            }
        }
        "transform-box" => {
            let b = match values::ident(c0).as_deref() {
                Some("content-box") => TransformBox::ContentBox,
                Some("border-box") => TransformBox::BorderBox,
                Some("fill-box") => TransformBox::FillBox,
                Some("stroke-box") => TransformBox::StrokeBox,
                Some("view-box") => TransformBox::ViewBox,
                _ => return true,
            };
            if b != TransformBox::ViewBox || style.transform3d.is_some() {
                style.transform3d_mut().transform_box = b;
            }
        }
        "box-reflect" | "-webkit-box-reflect" | "shape-outside" | "shape-margin" => {}
        _ => return false,
    }
    true
}

/// A `transform-origin` / `perspective-origin` position (one or two
/// components; keywords in either order).
fn parse_origin(comps: &[&[Token]], ctx: &LengthContext) -> Option<(Length, Length)> {
    super::parse_position(comps, ctx)
}
