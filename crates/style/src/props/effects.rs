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

fn parse_transform(comps: &[&[Token]], ctx: &LengthContext) -> Option<Vec<TransformOp>> {
    if comps.len() == 1 && values::is_ident(comps[0], "none") {
        return Some(Vec::new());
    }
    let mut ops = Vec::new();
    for c in comps {
        let name = super::fn_name(c)?;
        let args = super::fn_args(c);
        let arg = |i: usize| -> Option<&[Token]> { args.get(i).and_then(|a| a.first().copied()) };
        let num = |i: usize| -> Option<f32> { arg(i).and_then(values::number) };
        let len = |i: usize| -> Option<Length> {
            let a = arg(i)?;
            if let Some(n) = values::number(a) {
                return Some(Length::Px(n));
            }
            super::len_no_auto(a, ctx)
        };
        let ang = |i: usize| -> Option<f32> { arg(i).and_then(super::angle) };
        let op = match name.as_str() {
            "translate" => TransformOp::Translate(len(0)?, len(1).unwrap_or(Length::ZERO)),
            "translatex" => TransformOp::Translate(len(0)?, Length::ZERO),
            "translatey" => TransformOp::Translate(Length::ZERO, len(0)?),
            "translatez" => TransformOp::Translate(Length::ZERO, Length::ZERO),
            "translate3d" => TransformOp::Translate(len(0)?, len(1)?),
            "scale" => {
                let x = num(0)?;
                TransformOp::Scale(x, num(1).unwrap_or(x))
            }
            "scalex" => TransformOp::Scale(num(0)?, 1.0),
            "scaley" => TransformOp::Scale(1.0, num(0)?),
            "scalez" => TransformOp::Scale(1.0, 1.0),
            "scale3d" => TransformOp::Scale(num(0)?, num(1)?),
            "rotate" | "rotatez" => TransformOp::Rotate(ang(0)?),
            "rotatex" | "rotatey" | "rotate3d" | "perspective" => TransformOp::Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
            "skew" => TransformOp::Skew(ang(0)?, ang(1).unwrap_or(0.0)),
            "skewx" => TransformOp::Skew(ang(0)?, 0.0),
            "skewy" => TransformOp::Skew(0.0, ang(0)?),
            "matrix" => {
                if args.len() != 6 {
                    return None;
                }
                let mut m = [0.0; 6];
                for (i, v) in m.iter_mut().enumerate() {
                    *v = num(i)?;
                }
                TransformOp::Matrix(m)
            }
            "matrix3d" => {
                if args.len() != 16 {
                    return None;
                }
                TransformOp::Matrix([num(0)?, num(1)?, num(4)?, num(5)?, num(12)?, num(13)?])
            }
            _ => return None,
        };
        ops.push(op);
    }
    Some(ops)
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
            // Drop a trailing z offset.
            let take = comps.iter().take_while(|c| !(comps.len() >= 3 && std::ptr::eq(*c, comps.last().unwrap()) && super::px(c, ctx).is_some() && comps.len() == 3)).count();
            if let Some(p) = super::parse_position(&comps[..take.max(1).min(comps.len())], ctx) {
                style.transform_origin = p;
            } else if let Some(p) = super::parse_position(comps, ctx) {
                style.transform_origin = p;
            }
        }
        "translate" => {
            let x = super::len_no_auto(c0, ctx).or_else(|| values::number(c0).map(Length::Px));
            if let Some(x) = x {
                let y = comps.get(1).and_then(|c| super::len_no_auto(c, ctx)).unwrap_or(Length::ZERO);
                style.transform.insert(0, TransformOp::Translate(x, y));
            }
        }
        "scale" => {
            if let Some(x) = values::number(c0) {
                let y = comps.get(1).and_then(|c| values::number(c)).unwrap_or(x);
                style.transform.insert(0, TransformOp::Scale(x, y));
            }
        }
        "rotate" => {
            if let Some(a) = comps.iter().find_map(|c| super::angle(c)) {
                style.transform.insert(0, TransformOp::Rotate(a));
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
        "clip-path" | "clip" | "perspective" | "perspective-origin"
        | "backface-visibility" | "-webkit-backface-visibility" | "transform-style" | "-webkit-transform-style" | "transform-box"
        | "box-reflect" | "-webkit-box-reflect" | "shape-outside" | "shape-margin" => {}
        _ => return false,
    }
    true
}
