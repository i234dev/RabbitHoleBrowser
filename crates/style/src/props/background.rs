//! `background` and its longhands, including gradients.

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

/// Color stops and hints. `conic`: positions are `<angle-percentage>`s
/// (stored as percentages of a turn). Returns None when the list is
/// invalid (a hint not between two color stops, a stray token, fewer than
/// one color stop).
fn gradient_stops(args: &[Vec<&[Token]>], current: Color, ctx: &LengthContext, conic: bool) -> Option<Vec<GradientStop>> {
    let mut stops: Vec<GradientStop> = Vec::new();
    let position = |c: &[Token]| -> Option<Length> {
        if conic {
            super::angle_percentage(c).map(|a| Length::Percent(a / std::f32::consts::TAU * 100.0))
        } else {
            super::len_no_auto(c, ctx)
        }
    };
    for arg in args {
        let mut col = None;
        let mut positions = Vec::new();
        for c in arg {
            if let (None, Some(cc)) = (col, color(c, current)) {
                col = Some(cc);
            } else if let Some(l) = position(c) {
                positions.push(l);
            } else {
                return None;
            }
        }
        match col {
            Some(col) => {
                if positions.len() > 2 {
                    return None;
                }
                if positions.is_empty() {
                    stops.push(GradientStop { color: col, position: None, hint: false });
                }
                for p in positions {
                    stops.push(GradientStop { color: col, position: Some(p), hint: false });
                }
            }
            None => {
                // A color interpolation hint: one position between stops.
                if positions.len() != 1 || stops.last().is_none_or(|s| s.hint) {
                    return None;
                }
                stops.push(GradientStop { color: Color::TRANSPARENT, position: Some(positions[0]), hint: true });
            }
        }
    }
    if stops.is_empty() || stops.last().is_some_and(|s| s.hint) {
        return None;
    }
    Some(stops)
}

/// Parse a `<color-interpolation-method>` (`in <space> [<hue> hue]`) at
/// `toks[*i]`, advancing past it.
fn parse_interpolation(toks: &[&[Token]], i: &mut usize) -> Option<crate::ColorInterp> {
    if !toks.get(*i).is_some_and(|t| values::is_ident(t, "in")) {
        return None;
    }
    let mut words = vec!["in".to_string()];
    let mut j = *i + 1;
    while j < toks.len() && words.len() < 4 {
        match values::ident(toks[j]) {
            Some(w) if w != "to" && w != "at" && w != "from" => words.push(w.to_ascii_lowercase()),
            _ => break,
        }
        j += 1;
    }
    // Take the longest word run that parses (`in hsl longer hue`).
    while words.len() >= 2 {
        if let Some(m) = crate::ColorInterp::parse(&words) {
            *i += words.len();
            return Some(m);
        }
        words.pop();
    }
    None
}

/// Is the first argument a prelude (no color in it)?
fn is_prelude(first: &[&[Token]], current: Color) -> bool {
    !first.is_empty() && first.iter().all(|t| color(t, current).is_none())
        && first.first().is_some_and(|t| values::ident(t).is_some() || super::angle(t).is_some())
}

/// Parse a gradient function component.
pub fn parse_gradient(c: &[Token], current: Color, ctx: &LengthContext) -> Option<BackgroundImage> {
    let name = super::fn_name(c)?;
    let base = name.trim_start_matches("-webkit-").trim_start_matches("-moz-").trim_start_matches("-o-");
    let (kind, repeating) = match base {
        "linear-gradient" => ("linear", false),
        "repeating-linear-gradient" => ("linear", true),
        "radial-gradient" => ("radial", false),
        "repeating-radial-gradient" => ("radial", true),
        "conic-gradient" => ("conic", false),
        "repeating-conic-gradient" => ("conic", true),
        _ => return None,
    };
    let args = super::fn_args(c);
    if args.is_empty() {
        return None;
    }
    match kind {
        "linear" => {
            let mut direction = GradientDirection::Corner(0, 1);
            let mut interp = crate::ColorInterp::default();
            let mut stop_start = 0;
            let first = &args[0];
            if is_prelude(first, current) {
                stop_start = 1;
                let mut i = 0;
                let mut seen_dir = false;
                let mut seen_interp = false;
                while i < first.len() {
                    if !seen_interp {
                        if let Some(m) = parse_interpolation(first, &mut i) {
                            interp = m;
                            seen_interp = true;
                            continue;
                        }
                    }
                    if seen_dir {
                        return None;
                    }
                    seen_dir = true;
                    if let Some(a) = super::angle(first[i]) {
                        direction = GradientDirection::Angle(a);
                        i += 1;
                    } else if values::is_ident(first[i], "to") {
                        let (mut h, mut v) = (0i8, 0i8);
                        i += 1;
                        let mut n = 0;
                        while i < first.len() && n < 2 {
                            match values::ident(first[i]).as_deref() {
                                Some("left") if h == 0 => h = -1,
                                Some("right") if h == 0 => h = 1,
                                Some("top") if v == 0 => v = -1,
                                Some("bottom") if v == 0 => v = 1,
                                Some("in") => break,
                                _ => return None,
                            }
                            i += 1;
                            n += 1;
                        }
                        if n == 0 {
                            return None;
                        }
                        direction = GradientDirection::Corner(h, v);
                    } else if name.starts_with("-webkit-") || name.starts_with("-moz-") {
                        // Legacy prefixed syntax: a bare side keyword means "from".
                        let d = match values::ident(first[i]).as_deref() {
                            Some("left") => (1, 0),
                            Some("right") => (-1, 0),
                            Some("top") => (0, 1),
                            Some("bottom") => (0, -1),
                            _ => return None,
                        };
                        direction = GradientDirection::Corner(d.0, d.1);
                        i += 1;
                        // A second legacy keyword is accepted and ignored.
                        if i < first.len() && values::ident(first[i]).is_some_and(|k| matches!(k.as_str(), "left" | "right" | "top" | "bottom")) {
                            i += 1;
                        }
                    } else {
                        return None;
                    }
                }
            }
            let stops = gradient_stops(&args[stop_start..], current, ctx, false)?;
            Some(BackgroundImage::LinearGradient { direction, stops, repeating, interp })
        }
        "radial" => {
            let mut shape = RadialShape::Ellipse;
            let mut size = RadialSize::FarthestCorner;
            let mut position = (Length::Percent(50.0), Length::Percent(50.0));
            let mut interp = crate::ColorInterp::default();
            let mut stop_start = 0;
            let first = &args[0];
            // A first argument without a color is the shape/size/position prelude.
            if first.iter().all(|t| color(t, current).is_none()) && !first.is_empty() {
                let mut lens: Vec<Length> = Vec::new();
                let mut i = 0;
                let mut explicit_shape = false;
                let mut keyword_size = false;
                while i < first.len() {
                    if let Some(m) = parse_interpolation(first, &mut i) {
                        interp = m;
                        continue;
                    }
                    let t = first[i];
                    if let Some(kw) = values::ident(t) {
                        match kw.as_str() {
                            "circle" | "ellipse" if !explicit_shape => {
                                shape = if kw == "circle" { RadialShape::Circle } else { RadialShape::Ellipse };
                                explicit_shape = true;
                            }
                            "closest-side" | "farthest-side" | "closest-corner" | "farthest-corner" if !keyword_size && lens.is_empty() => {
                                keyword_size = true;
                                size = match kw.as_str() {
                                    "closest-side" => RadialSize::ClosestSide,
                                    "farthest-side" => RadialSize::FarthestSide,
                                    "closest-corner" => RadialSize::ClosestCorner,
                                    _ => RadialSize::FarthestCorner,
                                };
                            }
                            "at" => {
                                let mut j = i + 1;
                                while j < first.len() && !values::is_ident(first[j], "in") {
                                    j += 1;
                                }
                                position = super::parse_position(&first[i + 1..j], ctx)?;
                                i = j;
                                continue;
                            }
                            _ => return None,
                        }
                    } else if let Some(l) = super::len_no_auto(t, ctx) {
                        if keyword_size {
                            return None;
                        }
                        lens.push(l);
                    } else {
                        return None;
                    }
                    i += 1;
                }
                match lens.len() {
                    0 => {}
                    1 => {
                        if matches!(lens[0], Length::Percent(_)) || matches!(shape, RadialShape::Ellipse) && explicit_shape {
                            return None;
                        }
                        size = RadialSize::Explicit(lens[0], lens[0]);
                        shape = RadialShape::Circle;
                    }
                    2 => {
                        if matches!(shape, RadialShape::Circle) {
                            return None;
                        }
                        size = RadialSize::Explicit(lens[0], lens[1]);
                    }
                    _ => return None,
                }
                stop_start = 1;
            }
            let stops = gradient_stops(&args[stop_start..], current, ctx, false)?;
            Some(BackgroundImage::RadialGradient { shape, size, position, stops, repeating, interp })
        }
        _ => {
            let first = &args[0];
            let mut angle = 0.0;
            let mut position = (Length::Percent(50.0), Length::Percent(50.0));
            let mut interp = crate::ColorInterp::default();
            let mut stop_start = 0;
            if first.iter().all(|t| color(t, current).is_none()) && first.first().is_some_and(|t| values::ident(t).is_some()) {
                stop_start = 1;
                let mut i = 0;
                let (mut seen_from, mut seen_at) = (false, false);
                while i < first.len() {
                    if let Some(m) = parse_interpolation(first, &mut i) {
                        interp = m;
                        continue;
                    }
                    if values::is_ident(first[i], "from") && !seen_from {
                        seen_from = true;
                        angle = super::angle(first.get(i + 1)?)?;
                        i += 2;
                    } else if values::is_ident(first[i], "at") && !seen_at {
                        seen_at = true;
                        let mut j = i + 1;
                        while j < first.len() && !values::is_ident(first[j], "in") && !values::is_ident(first[j], "from") {
                            j += 1;
                        }
                        position = super::parse_position(&first[i + 1..j], ctx)?;
                        i = j;
                    } else {
                        return None;
                    }
                }
            }
            let stops = gradient_stops(&args[stop_start..], current, ctx, true)?;
            Some(BackgroundImage::ConicGradient { angle, position, stops, repeating, interp })
        }
    }
}

/// Parse a `<bg-image>` component: `none`, `url()`, a gradient, or
/// `image-set()`/`-webkit-image-set()` (first url).
pub fn parse_image(c: &[Token], current: Color, ctx: &ApplyCtx) -> Option<BackgroundImage> {
    if values::is_ident(c, "none") {
        return Some(BackgroundImage::None);
    }
    if let Some(u) = crate::cascade::url_of(c) {
        return Some(BackgroundImage::Url(ctx.url(&u)));
    }
    if let Some(g) = parse_gradient(c, current, ctx) {
        return Some(g);
    }
    if let Some(f) = super::fn_name(c) {
        if f.ends_with("image-set") {
            return Some(parse_image_set(c, ctx).map(BackgroundImage::Url).unwrap_or(BackgroundImage::None));
        }
        if f == "image" || f == "cross-fade" {
            for arg in super::fn_args(c) {
                for t in arg {
                    if let Some(u) = crate::cascade::url_of(t) {
                        return Some(BackgroundImage::Url(ctx.url(&u)));
                    }
                    if let [Token::String(s)] = t {
                        return Some(BackgroundImage::Url(ctx.url(s)));
                    }
                }
            }
            return Some(BackgroundImage::None);
        }
    }
    None
}

/// `image-set()`: drop candidates whose `type()` we cannot decode, then
/// take the 1x one (the smallest resolution >= 1, else the densest).
pub fn parse_image_set(c: &[Token], ctx: &ApplyCtx) -> Option<String> {
    let mut best: Option<(f32, String)> = None;
    let mut fallback: Option<(f32, String)> = None;
    for arg in super::fn_args(c) {
        let mut url = None;
        let mut res = 1.0f32;
        let mut supported = true;
        for t in arg {
            if let Some(u) = crate::cascade::url_of(t) {
                url = Some(u);
            } else if let [Token::String(s)] = t {
                url = Some(s.clone());
            } else if let [Token::Dimension(v, unit)] = t {
                res = match unit.to_ascii_lowercase().as_str() {
                    "x" | "dppx" => *v,
                    "dpi" => *v / 96.0,
                    "dpcm" => *v * 2.54 / 96.0,
                    _ => *v,
                };
            } else if super::fn_name(t).as_deref() == Some("type") {
                if let Some(Token::String(m)) = t.get(1) {
                    supported = rabbithole_gfx::image::supports_mime(m);
                }
            }
        }
        let Some(u) = url else { continue };
        if !supported {
            continue;
        }
        if res + 1e-3 >= 1.0 {
            if best.as_ref().map(|(r, _)| res < *r).unwrap_or(true) {
                best = Some((res, u));
            }
        } else if fallback.as_ref().map(|(r, _)| res > *r).unwrap_or(true) {
            fallback = Some((res, u));
        }
    }
    best.or(fallback).map(|(_, u)| ctx.url(&u))
}

fn parse_size(comps: &[&[Token]], ctx: &LengthContext) -> Option<BackgroundSize> {
    if comps.len() == 1 {
        if let Some(kw) = values::ident(comps[0]) {
            match kw.as_str() {
                "cover" => return Some(BackgroundSize::Cover),
                "contain" => return Some(BackgroundSize::Contain),
                "auto" => return Some(BackgroundSize::Auto),
                _ => return None,
            }
        }
    }
    let one = |c: &[Token]| -> Option<Length> {
        if values::is_ident(c, "auto") {
            Some(Length::Auto)
        } else {
            super::len_no_auto(c, ctx)
        }
    };
    match comps.len() {
        1 => Some(BackgroundSize::Explicit(one(comps[0])?, Length::Auto)),
        2 => Some(BackgroundSize::Explicit(one(comps[0])?, one(comps[1])?)),
        _ => None,
    }
}

fn parse_repeat(comps: &[&[Token]]) -> Option<(BackgroundRepeat, BackgroundRepeat)> {
    let one = |c: &[Token]| -> Option<BackgroundRepeat> {
        Some(match values::ident(c)?.as_str() {
            "repeat" => BackgroundRepeat::Repeat,
            "no-repeat" => BackgroundRepeat::NoRepeat,
            "space" => BackgroundRepeat::Space,
            "round" => BackgroundRepeat::Round,
            _ => return None,
        })
    };
    match comps.len() {
        1 => match values::ident(comps[0])?.as_str() {
            "repeat-x" => Some((BackgroundRepeat::Repeat, BackgroundRepeat::NoRepeat)),
            "repeat-y" => Some((BackgroundRepeat::NoRepeat, BackgroundRepeat::Repeat)),
            _ => {
                let r = one(comps[0])?;
                Some((r, r))
            }
        },
        2 => Some((one(comps[0])?, one(comps[1])?)),
        _ => None,
    }
}

fn parse_box(c: &[Token]) -> Option<BoxArea> {
    Some(match values::ident(c)?.as_str() {
        "border-box" => BoxArea::BorderBox,
        "padding-box" => BoxArea::PaddingBox,
        "content-box" => BoxArea::ContentBox,
        _ => return None,
    })
}

fn is_repeat_kw(c: &[Token]) -> bool {
    matches!(values::ident(c).as_deref(), Some("repeat" | "no-repeat" | "repeat-x" | "repeat-y" | "space" | "round"))
}

fn is_attachment_kw(c: &[Token]) -> bool {
    matches!(values::ident(c).as_deref(), Some("scroll" | "fixed" | "local"))
}

fn is_position_kw(c: &[Token]) -> bool {
    matches!(values::ident(c).as_deref(), Some("left" | "right" | "top" | "bottom" | "center"))
}

/// Parse one layer of the `background` shorthand. Returns the layer and the
/// color found in it (if any).
fn parse_layer(layer: &[&[Token]], current: Color, ctx: &ApplyCtx) -> Option<(BackgroundLayer, Option<Color>)> {
    let mut out = BackgroundLayer::default();
    let mut col = None;
    let mut boxes: Vec<BoxArea> = Vec::new();
    // Each component may appear at most once (`background: red pink` is
    // invalid as a whole).
    let (mut seen_image, mut seen_repeat, mut seen_attach, mut seen_pos) = (false, false, false, false);
    let mut i = 0;
    while i < layer.len() {
        let c = layer[i];
        if let Some(img) = parse_image(c, current, ctx) {
            if seen_image {
                return None;
            }
            seen_image = true;
            out.image = img;
            i += 1;
            continue;
        }
        if let Some(cc) = color(c, current) {
            if col.is_some() {
                return None;
            }
            col = Some(cc);
            i += 1;
            continue;
        }
        if is_repeat_kw(c) && seen_repeat {
            return None;
        }
        if is_attachment_kw(c) && seen_attach {
            return None;
        }
        if (is_position_kw(c) || super::len_no_auto(c, ctx).is_some()) && seen_pos {
            return None;
        }
        if parse_box(c).is_some() && boxes.len() >= 2 {
            return None;
        }
        if is_repeat_kw(c) {
            seen_repeat = true;
            let mut j = i + 1;
            if j < layer.len() && is_repeat_kw(layer[j]) && !matches!(values::ident(c).as_deref(), Some("repeat-x" | "repeat-y")) {
                j += 1;
            }
            match parse_repeat(&layer[i..j]) {
                Some(r) => out.repeat = r,
                None => return None,
            }
            i = j;
            continue;
        }
        if is_attachment_kw(c) {
            seen_attach = true;
            out.fixed = values::is_ident(c, "fixed");
            i += 1;
            continue;
        }
        if let Some(b) = parse_box(c) {
            boxes.push(b);
            i += 1;
            continue;
        }
        if values::is_ident(c, "text") {
            i += 1;
            continue;
        }
        if is_position_kw(c) || super::len_no_auto(c, ctx).is_some() {
            // position: up to 4 components, then optional "/ size"
            let mut j = i;
            while j < layer.len() && j - i < 4 && (is_position_kw(layer[j]) || super::len_no_auto(layer[j], ctx).is_some()) {
                j += 1;
            }
            // Only the longest prefix that parses is taken.
            let mut pos = None;
            let mut k = j;
            while k > i {
                if let Some(p) = super::parse_position(&layer[i..k], &ctx.len) {
                    pos = Some(p);
                    break;
                }
                k -= 1;
            }
            let Some(p) = pos else { return None };
            seen_pos = true;
            out.position = p;
            i = k;
            if i < layer.len() && matches!(layer[i], [Token::Delim('/')]) {
                i += 1;
                let mut j = i;
                while j < layer.len() && j - i < 2 && (values::is_ident(layer[j], "auto") || values::is_ident(layer[j], "cover") || values::is_ident(layer[j], "contain") || super::len_no_auto(layer[j], ctx).is_some()) {
                    j += 1;
                }
                match parse_size(&layer[i..j], ctx) {
                    Some(s) => out.size = s,
                    None => return None,
                }
                i = j;
            }
            continue;
        }
        return None;
    }
    match boxes.len() {
        1 => {
            out.origin = boxes[0];
            out.clip = boxes[0];
        }
        2 => {
            out.origin = boxes[0];
            out.clip = boxes[1];
        }
        _ => {}
    }
    Some((out, col))
}

fn sync_compat(style: &mut ComputedStyle) {
    style.background_image = style.background_layers.iter().find_map(|l| match &l.image {
        BackgroundImage::Url(u) => Some(u.clone()),
        _ => None,
    });
}

fn ensure_layers(style: &mut ComputedStyle, n: usize) {
    if style.background_layers.len() < n {
        style.background_layers.resize(n, BackgroundLayer::default());
    }
}

/// Apply a per-layer longhand with `values.len()` values, cycling.
fn set_each<T: Clone>(style: &mut ComputedStyle, vals: Vec<T>, f: impl Fn(&mut BackgroundLayer, T)) {
    if vals.is_empty() {
        return;
    }
    ensure_layers(style, 1);
    let n = style.background_layers.len();
    for i in 0..n {
        f(&mut style.background_layers[i], vals[i % vals.len()].clone());
    }
}

/// `mask-*` properties: the same grammar as the `background-*` ones, applied
/// to the mask layers (origin defaults to the border box). Mode and
/// compositing keywords are accepted and ignored.
fn apply_mask(name: &str, comps: &[&[Token]], style: &mut ComputedStyle, ctx: &ApplyCtx) -> bool {
    let bg_name = match name.strip_prefix("-webkit-").unwrap_or(name) {
        "mask" => "background",
        "mask-image" => "background-image",
        "mask-size" => "background-size",
        "mask-position" => "background-position",
        "mask-position-x" => "background-position-x",
        "mask-position-y" => "background-position-y",
        "mask-repeat" => "background-repeat",
        "mask-origin" => "background-origin",
        "mask-clip" => "background-clip",
        "mask-mode" | "mask-composite" | "mask-type" | "mask-border" | "mask-box-image" | "mask-border-source" | "mask-box-image-source" => return true,
        _ => return false,
    };
    const IGNORED: &[&str] = &["alpha", "luminance", "match-source", "add", "subtract", "intersect", "exclude", "source-over", "source-in", "source-out", "xor", "no-clip"];
    let kept: Vec<&[Token]> = if bg_name == "background" {
        comps.iter().copied().filter(|c| !values::ident(c).is_some_and(|k| IGNORED.contains(&k.as_str()))).collect()
    } else {
        comps.to_vec()
    };
    if kept.is_empty() {
        return true;
    }
    let had = style.mask_layers.len();
    if bg_name == "background" {
        // The mask shorthand keeps its `none` layers: their size, position
        // and repeat apply when `mask-image` sets the image later
        // (`mask: none 50%/2rem no-repeat; mask-image: url(...)`).
        let mut out = Vec::new();
        for layer in values::split_commas(&kept) {
            match parse_layer(&layer, style.color, ctx) {
                Some((l, None)) => out.push(l),
                _ => return true,
            }
        }
        style.mask_layers = out;
    } else {
        let saved_color = style.background_color;
        let saved_image = style.background_image.take();
        std::mem::swap(&mut style.background_layers, &mut style.mask_layers);
        apply(bg_name, &kept, style, ctx);
        std::mem::swap(&mut style.background_layers, &mut style.mask_layers);
        style.background_color = saved_color;
        style.background_image = saved_image;
    }
    if matches!(bg_name, "background" | "background-image") {
        // New layers are positioned against the border box, unless the
        // shorthand named a box.
        let named_box = bg_name == "background" && kept.iter().any(|c| parse_box(c).is_some());
        for (i, l) in style.mask_layers.iter_mut().enumerate() {
            if !named_box && (bg_name == "background" || i >= had) {
                l.origin = BoxArea::BorderBox;
            }
        }
    }
    if style.mask_layers.iter().any(|l| !matches!(l.image, BackgroundImage::None)) {
        style.creates_stacking_context = true;
    }
    true
}

/// 1–4 values expanded to (top, right, bottom, left).
fn four<T: Copy>(v: &[T]) -> Option<[T; 4]> {
    Some(match v.len() {
        1 => [v[0]; 4],
        2 => [v[0], v[1], v[0], v[1]],
        3 => [v[0], v[1], v[2], v[1]],
        4 => [v[0], v[1], v[2], v[3]],
        _ => return None,
    })
}

/// `border-image-slice`: [<number> | <percentage>]{1,4} && fill?
fn parse_bi_slice(comps: &[&[Token]]) -> Option<([Length; 4], bool)> {
    let mut fill = false;
    let mut vals = Vec::new();
    for (i, c) in comps.iter().enumerate() {
        if values::is_ident(c, "fill") {
            // `fill` comes first or last.
            if fill || (i != 0 && i != comps.len() - 1) {
                return None;
            }
            fill = true;
            continue;
        }
        match c {
            [Token::Number(n)] if *n >= 0.0 => vals.push(Length::Px(*n)),
            [Token::Percentage(p)] if *p >= 0.0 => vals.push(Length::Percent(*p)),
            _ => return None,
        }
    }
    Some((four(&vals)?, fill))
}

/// `border-image-width` (`auto` allowed) / `border-image-outset`.
fn parse_bi_widths(comps: &[&[Token]], ctx: &LengthContext, outset: bool) -> Option<[BorderImageWidth; 4]> {
    let mut vals = Vec::new();
    for c in comps {
        let v = if !outset && values::is_ident(c, "auto") {
            BorderImageWidth::Auto
        } else if let [Token::Number(n)] = c {
            if *n < 0.0 {
                return None;
            }
            BorderImageWidth::Number(*n)
        } else {
            let l = super::len_no_auto(c, ctx)?;
            if outset && matches!(l, Length::Percent(_)) {
                return None;
            }
            if matches!(l, Length::Px(v) if v < 0.0) || matches!(l, Length::Percent(v) if v < 0.0) {
                return None;
            }
            BorderImageWidth::Length(l)
        };
        vals.push(v);
    }
    four(&vals)
}

fn parse_bi_repeat(comps: &[&[Token]]) -> Option<(BorderImageRepeat, BorderImageRepeat)> {
    let one = |c: &[Token]| -> Option<BorderImageRepeat> {
        Some(match values::ident(c)?.as_str() {
            "stretch" => BorderImageRepeat::Stretch,
            "repeat" => BorderImageRepeat::Repeat,
            "round" => BorderImageRepeat::Round,
            "space" => BorderImageRepeat::Space,
            _ => return None,
        })
    };
    match comps {
        [a] => {
            let r = one(a)?;
            Some((r, r))
        }
        [a, b] => Some((one(a)?, one(b)?)),
        _ => None,
    }
}

/// The `border-image` shorthand: <source> || <slice> [/ <width>? [/ <outset>]?]? || <repeat>.
fn parse_border_image(comps: &[&[Token]], current: Color, ctx: &ApplyCtx) -> Option<BorderImage> {
    let mut out = BorderImage::default();
    let (mut seen_src, mut seen_slice, mut seen_repeat) = (false, false, false);
    let is_slice_tok = |c: &[Token]| matches!(c, [Token::Number(_)] | [Token::Percentage(_)]) || values::is_ident(c, "fill");
    let is_repeat_tok = |c: &[Token]| matches!(values::ident(c).as_deref(), Some("stretch" | "repeat" | "round" | "space"));
    let mut i = 0;
    while i < comps.len() {
        let c = comps[i];
        if !seen_src {
            if let Some(img) = parse_image(c, current, ctx) {
                out.source = img;
                seen_src = true;
                i += 1;
                continue;
            }
        }
        if !seen_slice && is_slice_tok(c) {
            let mut j = i;
            while j < comps.len() && is_slice_tok(comps[j]) {
                j += 1;
            }
            let (slice, fill) = parse_bi_slice(&comps[i..j])?;
            out.slice = slice;
            out.fill = fill;
            seen_slice = true;
            i = j;
            // `/ width` and `/ outset`.
            let slash = |k: usize| k < comps.len() && matches!(comps[k], [Token::Delim('/')]);
            if slash(i) {
                i += 1;
                let mut j = i;
                while j < comps.len() && !slash(j) && !is_repeat_tok(comps[j]) && parse_image(comps[j], current, ctx).is_none() {
                    j += 1;
                }
                if j > i {
                    out.width = parse_bi_widths(&comps[i..j], ctx, false)?;
                }
                i = j;
                if slash(i) {
                    i += 1;
                    let mut j = i;
                    while j < comps.len() && !is_repeat_tok(comps[j]) && parse_image(comps[j], current, ctx).is_none() {
                        j += 1;
                    }
                    if j == i {
                        return None;
                    }
                    out.outset = parse_bi_widths(&comps[i..j], ctx, true)?;
                    i = j;
                } else if comps.get(i - 1).is_some_and(|c| matches!(c, [Token::Delim('/')])) {
                    // A slash with neither width nor outset.
                    return None;
                }
            }
            continue;
        }
        if !seen_repeat && is_repeat_tok(c) {
            let mut j = i + 1;
            if j < comps.len() && is_repeat_tok(comps[j]) {
                j += 1;
            }
            out.repeat = parse_bi_repeat(&comps[i..j])?;
            seen_repeat = true;
            i = j;
            continue;
        }
        return None;
    }
    Some(out)
}

fn apply_border_image(name: &str, comps: &[&[Token]], style: &mut ComputedStyle, ctx: &ApplyCtx) {
    let current = style.color;
    let mut bi = style.border_image.as_deref().cloned().unwrap_or_default();
    match name {
        "border-image" | "-webkit-border-image" => {
            if comps.len() == 1 && values::is_ident(comps[0], "none") {
                style.border_image = None;
                return;
            }
            let Some(mut parsed) = parse_border_image(comps, current, ctx) else { return };
            if name == "-webkit-border-image" {
                // The legacy property always fills the middle and its
                // widths, when given, are the border widths.
                parsed.fill = true;
            }
            bi = parsed;
        }
        "border-image-source" => {
            if comps.len() != 1 {
                return;
            }
            match parse_image(comps[0], current, ctx) {
                Some(img) => bi.source = img,
                None => return,
            }
        }
        "border-image-slice" => match parse_bi_slice(comps) {
            Some((slice, fill)) => {
                bi.slice = slice;
                bi.fill = fill;
            }
            None => return,
        },
        "border-image-width" => match parse_bi_widths(comps, ctx, false) {
            Some(w) => bi.width = w,
            None => return,
        },
        "border-image-outset" => match parse_bi_widths(comps, ctx, true) {
            Some(w) => bi.outset = w,
            None => return,
        },
        "border-image-repeat" => match parse_bi_repeat(comps) {
            Some(r) => bi.repeat = r,
            None => return,
        },
        _ => return,
    }
    style.border_image = if bi == BorderImage::default() { None } else { Some(Box::new(bi)) };
}

pub fn apply(name: &str, comps: &[&[Token]], style: &mut ComputedStyle, ctx: &ApplyCtx) -> bool {
    if name.contains("mask") {
        return apply_mask(name, comps, style, ctx);
    }
    if name == "image-orientation" {
        // from-image | none | [<angle> || flip] (angles are legacy: treated
        // as from-image).
        if comps.len() == 1 && values::is_ident(comps[0], "none") {
            style.image_orientation_none = true;
        } else if comps.len() == 1 && values::is_ident(comps[0], "from-image") {
            style.image_orientation_none = false;
        }
        return true;
    }
    if name == "overflow-clip-margin" {
        // <visual-box> || <length [0,∞]>
        let mut area = None;
        let mut len = None;
        for c in comps {
            if let (None, Some(b)) = (area, parse_box(c)) {
                area = Some(b);
            } else if let (None, Some(px)) = (len, super::len_no_auto(c, ctx).and_then(|l| match l {
                Length::Px(v) if v >= 0.0 => Some(v),
                _ => None,
            })) {
                len = Some(px);
            } else {
                return true;
            }
        }
        if area.is_some() || len.is_some() {
            style.overflow_clip_margin = (area.unwrap_or(BoxArea::PaddingBox), len.unwrap_or(0.0));
        }
        return true;
    }
    if name.starts_with("border-image") || name == "-webkit-border-image" {
        apply_border_image(name, comps, style, ctx);
        return true;
    }
    let name = match name {
        "-webkit-background-size" => "background-size",
        "-webkit-background-clip" | "-moz-background-clip" => "background-clip",
        "-webkit-background-origin" => "background-origin",
        n => n,
    };
    let current = style.color;
    match name {
        "background" => {
            let layers = values::split_commas(comps);
            let mut out = Vec::with_capacity(layers.len());
            let mut final_color = None;
            for (k, layer) in layers.iter().enumerate() {
                let Some((l, col)) = parse_layer(layer, current, ctx) else { return true };
                if let Some(c) = col {
                    if k + 1 != layers.len() {
                        return true; // a color is only allowed in the final layer
                    }
                    final_color = Some(c);
                }
                out.push(l);
            }
            // Layers without an image are not kept (but a color-only
            // `background: red` still resets the layers).
            out.retain(|l| !matches!(l.image, BackgroundImage::None));
            style.background_layers = out;
            style.background_color = final_color.unwrap_or(Color::TRANSPARENT);
            sync_compat(style);
        }
        "background-color" => {
            if comps.len() == 1 {
                if let Some(c) = color(comps[0], current) {
                    style.background_color = c;
                }
            }
        }
        "background-image" => {
            let lists = values::split_commas(comps);
            let mut images = Vec::new();
            for l in lists {
                if l.len() != 1 {
                    return true;
                }
                match parse_image(l[0], current, ctx) {
                    Some(img) => images.push(img),
                    None => return true,
                }
            }
            // The image list defines the number of layers.
            let n = images.len();
            style.background_layers.truncate(n);
            ensure_layers(style, n);
            for (layer, img) in style.background_layers.iter_mut().zip(images) {
                layer.image = img;
            }
            sync_compat(style);
        }
        "background-size" => {
            let vals: Option<Vec<BackgroundSize>> = values::split_commas(comps).iter().map(|l| parse_size(l, ctx)).collect();
            if let Some(v) = vals {
                set_each(style, v, |l, s| l.size = s);
            }
        }
        "background-position" => {
            let vals: Option<Vec<(Length, Length)>> = values::split_commas(comps).iter().map(|l| super::parse_position(l, ctx)).collect();
            if let Some(v) = vals {
                set_each(style, v, |l, p| l.position = p);
            }
        }
        "background-position-x" | "background-position-y" => {
            let is_x = name.ends_with('x');
            let vals: Option<Vec<Length>> = values::split_commas(comps)
                .iter()
                .map(|l| {
                    if l.len() != 1 {
                        return None;
                    }
                    if let Some(kw) = values::ident(l[0]) {
                        return match kw.as_str() {
                            "left" | "top" => Some(Length::Percent(0.0)),
                            "right" | "bottom" => Some(Length::Percent(100.0)),
                            "center" => Some(Length::Percent(50.0)),
                            _ => None,
                        };
                    }
                    super::len_no_auto(l[0], ctx)
                })
                .collect();
            if let Some(v) = vals {
                set_each(style, v, |l, p| {
                    if is_x {
                        l.position.0 = p;
                    } else {
                        l.position.1 = p;
                    }
                });
            }
        }
        "background-repeat" => {
            let vals: Option<Vec<(BackgroundRepeat, BackgroundRepeat)>> = values::split_commas(comps).iter().map(|l| parse_repeat(l)).collect();
            if let Some(v) = vals {
                set_each(style, v, |l, r| l.repeat = r);
            }
        }
        "background-attachment" => {
            let vals: Option<Vec<bool>> = values::split_commas(comps)
                .iter()
                .map(|l| if l.len() == 1 && is_attachment_kw(l[0]) { Some(values::is_ident(l[0], "fixed")) } else { None })
                .collect();
            if let Some(v) = vals {
                set_each(style, v, |l, f| l.fixed = f);
            }
        }
        "background-origin" => {
            let vals: Option<Vec<BoxArea>> = values::split_commas(comps).iter().map(|l| if l.len() == 1 { parse_box(l[0]) } else { None }).collect();
            if let Some(v) = vals {
                set_each(style, v, |l, b| l.origin = b);
            }
        }
        "background-clip" => {
            let vals: Option<Vec<Option<BoxArea>>> = values::split_commas(comps)
                .iter()
                .map(|l| {
                    if l.len() != 1 {
                        return None;
                    }
                    if values::is_ident(l[0], "text") {
                        Some(None)
                    } else {
                        parse_box(l[0]).map(Some)
                    }
                })
                .collect();
            if let Some(v) = vals {
                set_each(style, v, |l, b| {
                    if let Some(b) = b {
                        l.clip = b;
                    }
                });
            }
        }
        "background-blend-mode" | "background-position-inline" | "background-position-block" => {}
        _ => return false,
    }
    true
}
