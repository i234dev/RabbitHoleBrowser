//! Parsing of CSS component values into typed values: colors, lengths
//! (including `calc()`/`min()`/`max()`/`clamp()`), keywords.

use crate::computed::Length;
use rabbithole_css::Token;
use rabbithole_gfx::Color;

/// A color value as written in a stylesheet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColorValue {
    Color(Color),
    /// `currentcolor`: resolved to the element's `color` at apply time.
    CurrentColor,
}

/// Values needed to turn font/viewport relative units into px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LengthContext {
    pub font_size: f32,
    pub root_font_size: f32,
    pub viewport_w: f32,
    pub viewport_h: f32,
}

/// A linear combination of length units, the result of evaluating a
/// `calc()` expression (or a plain length).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Lin {
    /// Dimensionless factor (only meaningful while evaluating products).
    pub num: f32,
    pub px: f32,
    pub em: f32,
    pub rem: f32,
    pub percent: f32,
    pub vw: f32,
    pub vh: f32,
    pub vmin: f32,
    pub vmax: f32,
    /// True when this is a bare number (no unit at all).
    pub is_number: bool,
}

impl Lin {
    fn add(self, o: Lin, sign: f32) -> Lin {
        Lin {
            num: self.num + sign * o.num,
            px: self.px + sign * o.px,
            em: self.em + sign * o.em,
            rem: self.rem + sign * o.rem,
            percent: self.percent + sign * o.percent,
            vw: self.vw + sign * o.vw,
            vh: self.vh + sign * o.vh,
            vmin: self.vmin + sign * o.vmin,
            vmax: self.vmax + sign * o.vmax,
            is_number: self.is_number && o.is_number,
        }
    }
    fn scale(self, f: f32) -> Lin {
        Lin {
            num: self.num * f,
            px: self.px * f,
            em: self.em * f,
            rem: self.rem * f,
            percent: self.percent * f,
            vw: self.vw * f,
            vh: self.vh * f,
            vmin: self.vmin * f,
            vmax: self.vmax * f,
            is_number: self.is_number,
        }
    }
    fn has_percent(&self) -> bool {
        self.percent != 0.0
    }
    /// Px value of all non-percentage terms.
    fn px_part(&self, ctx: &LengthContext) -> f32 {
        self.px
            + self.em * ctx.font_size
            + self.rem * ctx.root_font_size
            + self.vw * ctx.viewport_w / 100.0
            + self.vh * ctx.viewport_h / 100.0
            + self.vmin * ctx.viewport_w.min(ctx.viewport_h) / 100.0
            + self.vmax * ctx.viewport_w.max(ctx.viewport_h) / 100.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathFn {
    Min,
    Max,
    Clamp,
}

/// A length as written in a stylesheet (before unit resolution).
#[derive(Debug, Clone, PartialEq)]
pub enum LengthValue {
    Px(f32),
    Em(f32),
    Rem(f32),
    Percent(f32),
    Vw(f32),
    Vh(f32),
    Vmin(f32),
    Vmax(f32),
    Ch(f32),
    Ex(f32),
    Auto,
    /// Evaluated `calc()` expression.
    Calc(Lin),
    /// `min()`, `max()`, `clamp()` with evaluated arguments.
    Math(MathFn, Vec<Lin>),
}

impl LengthValue {
    /// A `calc()` / math function that mixes percentages with lengths, as
    /// its combinator and (px, percent) terms (None for anything else).
    pub fn mixed_terms(&self, ctx: &LengthContext) -> Option<(crate::computed::TrackCalc, Vec<(f32, f32)>)> {
        use crate::computed::TrackCalc;
        match self {
            LengthValue::Calc(lin) if lin.has_percent() && lin.px_part(ctx) != 0.0 => Some((TrackCalc::Sum, vec![(lin.px_part(ctx), lin.percent)])),
            LengthValue::Math(f, args) if args.iter().any(|a| a.has_percent()) => {
                let op = match f {
                    MathFn::Min => TrackCalc::Min,
                    MathFn::Max => TrackCalc::Max,
                    MathFn::Clamp => TrackCalc::Clamp,
                };
                Some((op, args.iter().map(|a| (a.px_part(ctx), a.percent)).collect()))
            }
            _ => None,
        }
    }

    /// Resolve to a computed `Length`.
    ///
    /// Approximations: a `calc()` mixing percentages with absolute terms
    /// resolves to its percentage part only (`calc(100% - 20px)` → `100%`),
    /// because the containing block is not known at compute time. Math
    /// functions whose arguments contain percentages resolve to their first
    /// percentage-free argument.
    pub fn resolve(&self, ctx: &LengthContext) -> Length {
        match self {
            LengthValue::Px(v) => Length::Px(*v),
            LengthValue::Em(v) => Length::Px(v * ctx.font_size),
            LengthValue::Rem(v) => Length::Px(v * ctx.root_font_size),
            LengthValue::Percent(p) => Length::Percent(*p),
            LengthValue::Vw(v) => Length::Px(v * ctx.viewport_w / 100.0),
            LengthValue::Vh(v) => Length::Px(v * ctx.viewport_h / 100.0),
            LengthValue::Vmin(v) => Length::Px(v * ctx.viewport_w.min(ctx.viewport_h) / 100.0),
            LengthValue::Vmax(v) => Length::Px(v * ctx.viewport_w.max(ctx.viewport_h) / 100.0),
            LengthValue::Ch(v) | LengthValue::Ex(v) => Length::Px(v * ctx.font_size * 0.5),
            LengthValue::Auto => Length::Auto,
            LengthValue::Calc(lin) => {
                if lin.has_percent() {
                    Length::Percent(lin.percent)
                } else {
                    Length::Px(lin.px_part(ctx))
                }
            }
            LengthValue::Math(f, args) => {
                if args.iter().any(|a| a.has_percent()) {
                    return match args.iter().find(|a| !a.has_percent()) {
                        Some(a) => Length::Px(a.px_part(ctx)),
                        None => Length::Percent(args[0].percent),
                    };
                }
                let vals: Vec<f32> = args.iter().map(|a| a.px_part(ctx)).collect();
                let v = match f {
                    MathFn::Min => vals.iter().cloned().fold(f32::INFINITY, f32::min),
                    MathFn::Max => vals.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
                    MathFn::Clamp => {
                        if vals.len() == 3 {
                            vals[1].max(vals[0]).min(vals[2])
                        } else {
                            vals[0]
                        }
                    }
                };
                Length::Px(v)
            }
        }
    }

    /// Resolve, but a bare number is also accepted as px (quirk used for a
    /// few properties and presentational attributes).
    pub fn is_auto(&self) -> bool {
        matches!(self, LengthValue::Auto)
    }
}

// --------------------------------------------------------------- components

/// Split a token list into top-level component values: each is either a
/// single token or a function token together with its arguments and
/// closing paren. Whitespace is dropped.
pub fn components(toks: &[Token]) -> Vec<&[Token]> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            Token::Whitespace => i += 1,
            Token::Function(_) | Token::OpenParen => {
                let start = i;
                let mut depth = 0i32;
                while i < toks.len() {
                    match toks[i] {
                        Token::Function(_) | Token::OpenParen => depth += 1,
                        Token::CloseParen => {
                            depth -= 1;
                            if depth == 0 {
                                i += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                out.push(&toks[start..i.min(toks.len())]);
            }
            _ => {
                out.push(&toks[i..i + 1]);
                i += 1;
            }
        }
    }
    out
}

/// Split components on top-level commas.
pub fn split_commas<'a>(comps: &[&'a [Token]]) -> Vec<Vec<&'a [Token]>> {
    let mut out = vec![Vec::new()];
    for c in comps {
        if c.len() == 1 && c[0] == Token::Comma {
            out.push(Vec::new());
        } else {
            out.last_mut().unwrap().push(*c);
        }
    }
    out
}

/// Lower-cased identifier of a single-token component.
pub fn ident(c: &[Token]) -> Option<String> {
    match c {
        [Token::Ident(s)] => Some(s.to_ascii_lowercase()),
        _ => None,
    }
}

pub fn is_ident(c: &[Token], name: &str) -> bool {
    matches!(c, [Token::Ident(s)] if s.eq_ignore_ascii_case(name))
}

pub fn number(c: &[Token]) -> Option<f32> {
    match c {
        [Token::Number(n)] => Some(*n),
        _ => None,
    }
}

/// Arguments of a function component, split on commas, whitespace removed.
fn function_args(c: &[Token]) -> Vec<Vec<&[Token]>> {
    if c.len() < 2 {
        return Vec::new();
    }
    let inner = if matches!(c.last(), Some(Token::CloseParen)) { &c[1..c.len() - 1] } else { &c[1..] };
    split_commas(&components(inner))
}

// ------------------------------------------------------------------ lengths

/// Parse a length component. Unitless numbers are only accepted as `0`
/// unless `allow_unitless` (quirks: presentational attributes).
pub fn parse_length(c: &[Token]) -> Option<LengthValue> {
    parse_length_opts(c, false)
}

pub fn parse_length_opts(c: &[Token], allow_unitless: bool) -> Option<LengthValue> {
    match c {
        [Token::Dimension(v, u)] => dimension(*v, u),
        [Token::Percentage(p)] => Some(LengthValue::Percent(*p)),
        [Token::Number(n)] => {
            if *n == 0.0 || allow_unitless {
                Some(LengthValue::Px(*n))
            } else {
                None
            }
        }
        [Token::Ident(s)] if s.eq_ignore_ascii_case("auto") => Some(LengthValue::Auto),
        [Token::Function(f), ..] => {
            let lf = f.to_ascii_lowercase();
            match lf.as_str() {
                "calc" | "-webkit-calc" | "-moz-calc" => {
                    let inner = strip_function(c);
                    let lin = eval_sum(&components(inner))?;
                    Some(LengthValue::Calc(lin))
                }
                "min" | "max" | "clamp" => {
                    let args = function_args(c);
                    let mut lins = Vec::new();
                    for a in &args {
                        lins.push(eval_sum(a)?);
                    }
                    if lins.is_empty() {
                        return None;
                    }
                    let f = match lf.as_str() {
                        "min" => MathFn::Min,
                        "max" => MathFn::Max,
                        _ => MathFn::Clamp,
                    };
                    Some(LengthValue::Math(f, lins))
                }
                "var" | "env" => None,
                _ => None,
            }
        }
        _ => None,
    }
}

fn strip_function(c: &[Token]) -> &[Token] {
    if c.len() >= 2 && matches!(c.last(), Some(Token::CloseParen)) {
        &c[1..c.len() - 1]
    } else if !c.is_empty() {
        &c[1..]
    } else {
        c
    }
}

fn dimension(v: f32, unit: &str) -> Option<LengthValue> {
    Some(match unit {
        "px" => LengthValue::Px(v),
        "em" => LengthValue::Em(v),
        "rem" => LengthValue::Rem(v),
        "vw" | "svw" | "lvw" | "dvw" => LengthValue::Vw(v),
        "vh" | "svh" | "lvh" | "dvh" => LengthValue::Vh(v),
        "vmin" => LengthValue::Vmin(v),
        "vmax" => LengthValue::Vmax(v),
        "ch" => LengthValue::Ch(v),
        "ex" => LengthValue::Ex(v),
        "pt" => LengthValue::Px(v * 4.0 / 3.0),
        "pc" => LengthValue::Px(v * 16.0),
        "cm" => LengthValue::Px(v * 96.0 / 2.54),
        "mm" => LengthValue::Px(v * 96.0 / 25.4),
        "q" => LengthValue::Px(v * 96.0 / 25.4 / 4.0),
        "in" => LengthValue::Px(v * 96.0),
        _ => return None,
    })
}

fn lin_of(lv: &LengthValue) -> Option<Lin> {
    let mut l = Lin::default();
    match lv {
        LengthValue::Px(v) => l.px = *v,
        LengthValue::Em(v) => l.em = *v,
        LengthValue::Rem(v) => l.rem = *v,
        LengthValue::Percent(v) => l.percent = *v,
        LengthValue::Vw(v) => l.vw = *v,
        LengthValue::Vh(v) => l.vh = *v,
        LengthValue::Vmin(v) => l.vmin = *v,
        LengthValue::Vmax(v) => l.vmax = *v,
        LengthValue::Ch(v) | LengthValue::Ex(v) => l.em = *v * 0.5,
        LengthValue::Calc(lin) => l = *lin,
        LengthValue::Math(_, args) => l = *args.first()?,
        LengthValue::Auto => return None,
    }
    Some(l)
}

/// sum := product (('+' | '-') product)*
fn eval_sum(comps: &[&[Token]]) -> Option<Lin> {
    let mut i = 0;
    let mut acc = eval_product(comps, &mut i)?;
    while i < comps.len() {
        let sign = match comps[i] {
            [Token::Delim('+')] => 1.0,
            [Token::Delim('-')] => -1.0,
            _ => return None,
        };
        i += 1;
        let rhs = eval_product(comps, &mut i)?;
        acc = acc.add(rhs, sign);
    }
    Some(acc)
}

/// product := unit (('*' | '/') unit)*
fn eval_product(comps: &[&[Token]], i: &mut usize) -> Option<Lin> {
    let mut acc = eval_unit(comps, i)?;
    while *i < comps.len() {
        match comps[*i] {
            [Token::Delim('*')] => {
                *i += 1;
                let rhs = eval_unit(comps, i)?;
                acc = if rhs.is_number {
                    acc.scale(rhs.num)
                } else if acc.is_number {
                    rhs.scale(acc.num)
                } else {
                    return None;
                };
            }
            [Token::Delim('/')] => {
                *i += 1;
                let rhs = eval_unit(comps, i)?;
                if !rhs.is_number || rhs.num == 0.0 {
                    return None;
                }
                acc = acc.scale(1.0 / rhs.num);
            }
            _ => break,
        }
    }
    Some(acc)
}

fn eval_unit(comps: &[&[Token]], i: &mut usize) -> Option<Lin> {
    let c = *comps.get(*i)?;
    *i += 1;
    match c {
        [Token::Number(n)] => Some(Lin { num: *n, is_number: true, ..Lin::default() }),
        [Token::OpenParen, ..] => eval_sum(&components(strip_function(c))),
        [Token::Delim('-')] => {
            // unary minus
            let inner = eval_unit(comps, i)?;
            Some(inner.scale(-1.0))
        }
        _ => {
            let lv = parse_length(c)?;
            lin_of(&lv)
        }
    }
}

// ------------------------------------------------------------------- colors

pub fn parse_color(toks: &[Token]) -> Option<ColorValue> {
    let comps = components(toks);
    if comps.len() != 1 {
        return None;
    }
    parse_color_component(comps[0])
}

pub fn parse_color_component(c: &[Token]) -> Option<ColorValue> {
    match c {
        [Token::Ident(name)] => {
            let l = name.to_ascii_lowercase();
            match l.as_str() {
                "transparent" => Some(ColorValue::Color(Color::TRANSPARENT)),
                "currentcolor" => Some(ColorValue::CurrentColor),
                _ => named_color(&l).or_else(|| crate::color_spaces::system_color(&l)).map(ColorValue::Color),
            }
        }
        [Token::Hash(h)] => hex_color(h).map(ColorValue::Color),
        [Token::Function(f), ..] => {
            let lf = f.to_ascii_lowercase();
            match lf.as_str() {
                "rgb" | "rgba" => parse_rgb(c).map(ColorValue::Color),
                "hsl" | "hsla" => parse_hsl(c).map(ColorValue::Color),
                other => crate::color_spaces::parse_function(other, c),
            }
        }
        _ => None,
    }
}

/// Parse a hex color (without '#'). Also accepts quirky values like `FFF`.
pub fn hex_color(h: &str) -> Option<Color> {
    if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let v = |s: &str| u8::from_str_radix(s, 16).ok();
    let d = |s: &str| v(s).map(|x| x * 17);
    match h.len() {
        3 => Some(Color::rgb(d(&h[0..1])?, d(&h[1..2])?, d(&h[2..3])?)),
        4 => Some(Color::rgba(d(&h[0..1])?, d(&h[1..2])?, d(&h[2..3])?, d(&h[3..4])?)),
        6 => Some(Color::rgb(v(&h[0..2])?, v(&h[2..4])?, v(&h[4..6])?)),
        8 => Some(Color::rgba(v(&h[0..2])?, v(&h[2..4])?, v(&h[4..6])?, v(&h[6..8])?)),
        _ => None,
    }
}

/// Flatten function args: supports both `a, b, c [, alpha]` and
/// `a b c [/ alpha]` syntaxes. Returns (values, alpha).
fn color_args(c: &[Token]) -> Option<(Vec<&[Token]>, Option<&[Token]>)> {
    let inner = strip_function(c);
    let comps = components(inner);
    let mut vals = Vec::new();
    let mut alpha = None;
    let mut after_slash = false;
    for comp in comps {
        match comp {
            [Token::Comma] => continue,
            [Token::Delim('/')] => {
                after_slash = true;
                continue;
            }
            _ => {}
        }
        if after_slash {
            alpha = Some(comp);
        } else if vals.len() == 3 {
            alpha = Some(comp);
        } else {
            vals.push(comp);
        }
    }
    if vals.len() != 3 {
        return None;
    }
    Some((vals, alpha))
}

fn channel(c: &[Token]) -> Option<f32> {
    match c {
        [Token::Number(n)] => Some(*n),
        [Token::Percentage(p)] => Some(p * 255.0 / 100.0),
        [Token::Ident(s)] if s.eq_ignore_ascii_case("none") => Some(0.0),
        _ => None,
    }
}

fn alpha_value(c: Option<&[Token]>) -> Option<f32> {
    match c {
        None => Some(1.0),
        Some([Token::Number(n)]) => Some(*n),
        Some([Token::Percentage(p)]) => Some(p / 100.0),
        Some([Token::Ident(s)]) if s.eq_ignore_ascii_case("none") => Some(0.0),
        _ => None,
    }
}

fn to_u8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

fn parse_rgb(c: &[Token]) -> Option<Color> {
    let (vals, alpha) = color_args(c)?;
    let r = channel(vals[0])?;
    let g = channel(vals[1])?;
    let b = channel(vals[2])?;
    let a = alpha_value(alpha)?;
    Some(Color::rgba(to_u8(r), to_u8(g), to_u8(b), to_u8(a * 255.0)))
}

fn parse_hsl(c: &[Token]) -> Option<Color> {
    let (vals, alpha) = color_args(c)?;
    // The legacy comma syntax takes percentages only (`hsla(0, 0, 0, 1)`
    // is invalid); bare numbers belong to the modern syntax (CSS Color 4).
    let legacy = components(strip_function(c)).iter().any(|comp| matches!(comp, [Token::Comma]));
    let h = match vals[0] {
        [Token::Number(n)] => *n,
        [Token::Dimension(n, u)] => match u.as_str() {
            "deg" => *n,
            "rad" => n.to_degrees(),
            "grad" => n * 0.9,
            "turn" => n * 360.0,
            _ => return None,
        },
        [Token::Ident(s)] if s.eq_ignore_ascii_case("none") => 0.0,
        _ => return None,
    };
    let pct = |c: &[Token]| -> Option<f32> {
        match c {
            [Token::Percentage(p)] => Some(p / 100.0),
            [Token::Number(n)] if !legacy => Some(n / 100.0),
            [Token::Ident(s)] if s.eq_ignore_ascii_case("none") => Some(0.0),
            _ => None,
        }
    };
    let s = pct(vals[1])?.clamp(0.0, 1.0);
    let l = pct(vals[2])?.clamp(0.0, 1.0);
    let a = alpha_value(alpha)?;
    let (r, g, b) = hsl_to_rgb(h, s, l);
    Some(Color::rgba(to_u8(r * 255.0), to_u8(g * 255.0), to_u8(b * 255.0), to_u8(a * 255.0)))
}

pub(crate) fn hsl_to_rgb_pub(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    hsl_to_rgb(h, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let h = h.rem_euclid(360.0) / 360.0;
    if s == 0.0 {
        return (l, l, l);
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    (f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0))
}

/// CSS named colors (lower-case name → RGB).
pub fn named_color(name: &str) -> Option<Color> {
    let idx = NAMED.binary_search_by(|(n, _)| n.cmp(&name)).ok()?;
    let v = NAMED[idx].1;
    Some(Color::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

// Sorted by name.
static NAMED: &[(&str, u32)] = &[
    ("aliceblue", 0xf0f8ff), ("antiquewhite", 0xfaebd7), ("aqua", 0x00ffff), ("aquamarine", 0x7fffd4),
    ("azure", 0xf0ffff), ("beige", 0xf5f5dc), ("bisque", 0xffe4c4), ("black", 0x000000),
    ("blanchedalmond", 0xffebcd), ("blue", 0x0000ff), ("blueviolet", 0x8a2be2), ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887), ("cadetblue", 0x5f9ea0), ("chartreuse", 0x7fff00), ("chocolate", 0xd2691e),
    ("coral", 0xff7f50), ("cornflowerblue", 0x6495ed), ("cornsilk", 0xfff8dc), ("crimson", 0xdc143c),
    ("cyan", 0x00ffff), ("darkblue", 0x00008b), ("darkcyan", 0x008b8b), ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9), ("darkgreen", 0x006400), ("darkgrey", 0xa9a9a9), ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b), ("darkolivegreen", 0x556b2f), ("darkorange", 0xff8c00), ("darkorchid", 0x9932cc),
    ("darkred", 0x8b0000), ("darksalmon", 0xe9967a), ("darkseagreen", 0x8fbc8f), ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f), ("darkslategrey", 0x2f4f4f), ("darkturquoise", 0x00ced1), ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493), ("deepskyblue", 0x00bfff), ("dimgray", 0x696969), ("dimgrey", 0x696969),
    ("dodgerblue", 0x1e90ff), ("firebrick", 0xb22222), ("floralwhite", 0xfffaf0), ("forestgreen", 0x228b22),
    ("fuchsia", 0xff00ff), ("gainsboro", 0xdcdcdc), ("ghostwhite", 0xf8f8ff), ("gold", 0xffd700),
    ("goldenrod", 0xdaa520), ("gray", 0x808080), ("green", 0x008000), ("greenyellow", 0xadff2f),
    ("grey", 0x808080), ("honeydew", 0xf0fff0), ("hotpink", 0xff69b4), ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082), ("ivory", 0xfffff0), ("khaki", 0xf0e68c), ("lavender", 0xe6e6fa),
    ("lavenderblush", 0xfff0f5), ("lawngreen", 0x7cfc00), ("lemonchiffon", 0xfffacd), ("lightblue", 0xadd8e6),
    ("lightcoral", 0xf08080), ("lightcyan", 0xe0ffff), ("lightgoldenrodyellow", 0xfafad2), ("lightgray", 0xd3d3d3),
    ("lightgreen", 0x90ee90), ("lightgrey", 0xd3d3d3), ("lightpink", 0xffb6c1), ("lightsalmon", 0xffa07a),
    ("lightseagreen", 0x20b2aa), ("lightskyblue", 0x87cefa), ("lightslategray", 0x778899), ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de), ("lightyellow", 0xffffe0), ("lime", 0x00ff00), ("limegreen", 0x32cd32),
    ("linen", 0xfaf0e6), ("magenta", 0xff00ff), ("maroon", 0x800000), ("mediumaquamarine", 0x66cdaa),
    ("mediumblue", 0x0000cd), ("mediumorchid", 0xba55d3), ("mediumpurple", 0x9370db), ("mediumseagreen", 0x3cb371),
    ("mediumslateblue", 0x7b68ee), ("mediumspringgreen", 0x00fa9a), ("mediumturquoise", 0x48d1cc), ("mediumvioletred", 0xc71585),
    ("midnightblue", 0x191970), ("mintcream", 0xf5fffa), ("mistyrose", 0xffe4e1), ("moccasin", 0xffe4b5),
    ("navajowhite", 0xffdead), ("navy", 0x000080), ("oldlace", 0xfdf5e6), ("olive", 0x808000),
    ("olivedrab", 0x6b8e23), ("orange", 0xffa500), ("orangered", 0xff4500), ("orchid", 0xda70d6),
    ("palegoldenrod", 0xeee8aa), ("palegreen", 0x98fb98), ("paleturquoise", 0xafeeee), ("palevioletred", 0xdb7093),
    ("papayawhip", 0xffefd5), ("peachpuff", 0xffdab9), ("peru", 0xcd853f), ("pink", 0xffc0cb),
    ("plum", 0xdda0dd), ("powderblue", 0xb0e0e6), ("purple", 0x800080), ("rebeccapurple", 0x663399),
    ("red", 0xff0000), ("rosybrown", 0xbc8f8f), ("royalblue", 0x4169e1), ("saddlebrown", 0x8b4513),
    ("salmon", 0xfa8072), ("sandybrown", 0xf4a460), ("seagreen", 0x2e8b57), ("seashell", 0xfff5ee),
    ("sienna", 0xa0522d), ("silver", 0xc0c0c0), ("skyblue", 0x87ceeb), ("slateblue", 0x6a5acd),
    ("slategray", 0x708090), ("slategrey", 0x708090), ("snow", 0xfffafa), ("springgreen", 0x00ff7f),
    ("steelblue", 0x4682b4), ("tan", 0xd2b48c), ("teal", 0x008080), ("thistle", 0xd8bfd8),
    ("tomato", 0xff6347), ("turquoise", 0x40e0d0), ("violet", 0xee82ee), ("wheat", 0xf5deb3),
    ("white", 0xffffff), ("whitesmoke", 0xf5f5f5), ("yellow", 0xffff00), ("yellowgreen", 0x9acd32),
];

#[cfg(test)]
mod tests {
    use super::*;
    use rabbithole_css::Tokenizer;

    fn ctx() -> LengthContext {
        LengthContext { font_size: 16.0, root_font_size: 16.0, viewport_w: 1000.0, viewport_h: 500.0 }
    }

    fn len(s: &str) -> Length {
        let t = Tokenizer::tokenize(s);
        let c = components(&t);
        parse_length(c[0]).expect("length").resolve(&ctx())
    }

    fn col(s: &str) -> ColorValue {
        parse_color(&Tokenizer::tokenize(s)).expect("color")
    }

    #[test]
    fn named_sorted() {
        for w in NAMED.windows(2) {
            assert!(w[0].0 < w[1].0, "{} {}", w[0].0, w[1].0);
        }
        assert_eq!(NAMED.len(), 148);
    }

    #[test]
    fn colors() {
        assert_eq!(col("red"), ColorValue::Color(Color::rgb(255, 0, 0)));
        assert_eq!(col("#abc"), ColorValue::Color(Color::rgb(0xaa, 0xbb, 0xcc)));
        assert_eq!(col("#11223380"), ColorValue::Color(Color::rgba(0x11, 0x22, 0x33, 0x80)));
        assert_eq!(col("rgb(1, 2, 3)"), ColorValue::Color(Color::rgb(1, 2, 3)));
        assert_eq!(col("rgba(1 2 3 / 50%)"), ColorValue::Color(Color::rgba(1, 2, 3, 128)));
        assert_eq!(col("rgb(100%, 0%, 0%, 0.5)"), ColorValue::Color(Color::rgba(255, 0, 0, 128)));
        assert_eq!(col("hsl(120, 100%, 50%)"), ColorValue::Color(Color::rgb(0, 255, 0)));
        assert_eq!(col("hsl(0 0% 100%)"), ColorValue::Color(Color::rgb(255, 255, 255)));
        // The legacy (comma) syntax needs percentages for saturation and
        // lightness; bare numbers are the modern syntax.
        assert!(parse_color(&Tokenizer::tokenize("hsla(0, 0, 0, 1)")).is_none());
        assert_eq!(col("hsl(0 0 100)"), ColorValue::Color(Color::rgb(255, 255, 255)));
        assert_eq!(col("currentColor"), ColorValue::CurrentColor);
        assert_eq!(col("transparent"), ColorValue::Color(Color::TRANSPARENT));
        assert!(parse_color(&Tokenizer::tokenize("notacolor")).is_none());
    }

    #[test]
    fn lengths() {
        assert_eq!(len("10px"), Length::Px(10.0));
        assert_eq!(len("2em"), Length::Px(32.0));
        assert_eq!(len("1.5rem"), Length::Px(24.0));
        assert_eq!(len("50%"), Length::Percent(50.0));
        assert_eq!(len("10vw"), Length::Px(100.0));
        assert_eq!(len("10vh"), Length::Px(50.0));
        assert_eq!(len("auto"), Length::Auto);
        assert_eq!(len("0"), Length::Px(0.0));
        assert_eq!(len("12pt"), Length::Px(16.0));
        assert_eq!(len("1in"), Length::Px(96.0));
    }

    #[test]
    fn calc() {
        assert_eq!(len("calc(10px + 2em)"), Length::Px(42.0));
        assert_eq!(len("calc(100% - 20px)"), Length::Percent(100.0));
        assert_eq!(len("calc(2 * 10px)"), Length::Px(20.0));
        assert_eq!(len("calc((10px + 10px) / 2)"), Length::Px(10.0));
        assert_eq!(len("min(10px, 2em)"), Length::Px(10.0));
        assert_eq!(len("max(10px, 2em)"), Length::Px(32.0));
        assert_eq!(len("clamp(10px, 100px, 20px)"), Length::Px(20.0));
        assert_eq!(len("min(50%, 30px)"), Length::Px(30.0));
    }
}
