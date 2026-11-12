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

fn gradient_stops(args: &[Vec<&[Token]>], current: Color, ctx: &LengthContext) -> Vec<GradientStop> {
    let mut stops = Vec::new();
    for arg in args {
        let mut col = None;
        let mut positions = Vec::new();
        for c in arg {
            if let Some(cc) = color(c, current) {
                col = Some(cc);
            } else if let Some(l) = super::len_no_auto(c, ctx) {
                positions.push(l);
            } else if let Some(a) = super::angle(c) {
                // conic stops use angles; map to percent of a turn
                positions.push(Length::Percent(a / std::f32::consts::TAU * 100.0));
            }
        }
        let Some(col) = col else { continue }; // color hints are dropped
        if positions.is_empty() {
            stops.push(GradientStop { color: col, position: None });
        } else {
            for p in positions {
                stops.push(GradientStop { color: col, position: Some(p) });
            }
        }
    }
    stops
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
            let mut stop_start = 0;
            let first = &args[0];
            if let Some(a) = first.first().and_then(|t| super::angle(t)) {
                direction = GradientDirection::Angle(a);
                stop_start = 1;
            } else if first.first().map(|t| values::is_ident(t, "to")).unwrap_or(false) {
                let (mut h, mut v) = (0i8, 0i8);
                for t in &first[1..] {
                    match values::ident(t).as_deref() {
                        Some("left") => h = -1,
                        Some("right") => h = 1,
                        Some("top") => v = -1,
                        Some("bottom") => v = 1,
                        _ => return None,
                    }
                }
                direction = GradientDirection::Corner(h, v);
                stop_start = 1;
            } else if name.starts_with("-webkit-") || name.starts_with("-moz-") {
                // Legacy prefixed syntax: a bare side keyword means "from".
                if let Some(kw) = first.first().and_then(|t| values::ident(t)) {
                    let d = match kw.as_str() {
                        "left" => Some((1, 0)),
                        "right" => Some((-1, 0)),
                        "top" => Some((0, 1)),
                        "bottom" => Some((0, -1)),
                        _ => None,
                    };
                    if let Some((h, v)) = d {
                        direction = GradientDirection::Corner(h, v);
                        stop_start = 1;
                    }
                }
            }
            let stops = gradient_stops(&args[stop_start..], current, ctx);
            if stops.is_empty() {
                return None;
            }
            Some(BackgroundImage::LinearGradient { direction, stops, repeating })
        }
        "radial" => {
            let mut shape = RadialShape::Ellipse;
            let mut size = RadialSize::FarthestCorner;
            let mut position = (Length::Percent(50.0), Length::Percent(50.0));
            let mut stop_start = 0;
            let first = &args[0];
            // A first argument without a color is the shape/size/position prelude.
            if first.iter().all(|t| color(t, current).is_none()) && !first.is_empty() {
                let mut lens: Vec<Length> = Vec::new();
                let mut i = 0;
                let mut explicit_shape = false;
                while i < first.len() {
                    let t = first[i];
                    if let Some(kw) = values::ident(t) {
                        match kw.as_str() {
                            "circle" => {
                                shape = RadialShape::Circle;
                                explicit_shape = true;
                            }
                            "ellipse" => {
                                shape = RadialShape::Ellipse;
                                explicit_shape = true;
                            }
                            "closest-side" => size = RadialSize::ClosestSide,
                            "farthest-side" => size = RadialSize::FarthestSide,
                            "closest-corner" => size = RadialSize::ClosestCorner,
                            "farthest-corner" => size = RadialSize::FarthestCorner,
                            "at" => {
                                if let Some(p) = super::parse_position(&first[i + 1..], ctx) {
                                    position = p;
                                }
                                break;
                            }
                            _ => return None,
                        }
                    } else if let Some(l) = super::len_no_auto(t, ctx) {
                        lens.push(l);
                    } else {
                        return None;
                    }
                    i += 1;
                }
                match lens.len() {
                    1 => {
                        size = RadialSize::Explicit(lens[0], lens[0]);
                        if !explicit_shape {
                            shape = RadialShape::Circle;
                        }
                    }
                    2 => size = RadialSize::Explicit(lens[0], lens[1]),
                    _ => {}
                }
                stop_start = 1;
            }
            let stops = gradient_stops(&args[stop_start..], current, ctx);
            if stops.is_empty() {
                return None;
            }
            Some(BackgroundImage::RadialGradient { shape, size, position, stops, repeating })
        }
        _ => {
            // conic: approximate as a linear gradient from the top.
            let first = &args[0];
            let stop_start = if first.iter().all(|t| color(t, current).is_none()) { 1 } else { 0 };
            let stops = gradient_stops(&args[stop_start.min(args.len())..], current, ctx);
            if stops.is_empty() {
                return None;
            }
            Some(BackgroundImage::LinearGradient { direction: GradientDirection::Angle(0.0), stops, repeating })
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

pub fn apply(name: &str, comps: &[&[Token]], style: &mut ComputedStyle, ctx: &ApplyCtx) -> bool {
    if name.contains("mask") {
        return apply_mask(name, comps, style, ctx);
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
