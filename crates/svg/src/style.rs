//! Presentation properties: parsing from attributes / CSS and inheritance.

use crate::color::{parse_color, ColorSpec};
use crate::units::{parse_length, parse_number, parse_number_list, Length};
use rabbithole_css::Token;
use rabbithole_gfx::{Color, Family, FillRule, LineCap, LineJoin};

/// `fill` / `stroke` value.
#[derive(Debug, Clone, PartialEq)]
pub enum PaintSpec {
    None,
    Color(Color),
    CurrentColor,
    /// `url(#id)` with an optional fallback.
    Url(String, Option<Box<PaintSpec>>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAnchor {
    Start,
    Middle,
    End,
}

/// Values that are explicitly specified on one element.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Specified {
    pub fill: Option<PaintSpec>,
    pub fill_opacity: Option<f32>,
    pub fill_rule: Option<FillRule>,
    pub stroke: Option<PaintSpec>,
    pub stroke_width: Option<Length>,
    pub stroke_opacity: Option<f32>,
    pub stroke_linecap: Option<LineCap>,
    pub stroke_linejoin: Option<LineJoin>,
    pub stroke_miterlimit: Option<f32>,
    pub stroke_dasharray: Option<Vec<f32>>,
    pub stroke_dashoffset: Option<f32>,
    pub color: Option<Color>,
    pub opacity: Option<f32>,
    pub font_size: Option<Length>,
    pub font_family: Option<Family>,
    pub font_weight: Option<u16>,
    pub font_italic: Option<bool>,
    pub text_anchor: Option<TextAnchor>,
    pub visibility: Option<bool>,
    pub display_none: bool,
    pub paint_order_stroke_first: Option<bool>,
    pub non_scaling_stroke: Option<bool>,
    pub clip_path: Option<String>,
    pub stop_color: Option<ColorSpec>,
    pub stop_opacity: Option<f32>,
    pub overflow_visible: Option<bool>,
}

/// Fully resolved (inherited) properties used while painting.
#[derive(Debug, Clone, PartialEq)]
pub struct Props {
    pub fill: PaintSpec,
    pub fill_opacity: f32,
    pub fill_rule: FillRule,
    pub stroke: PaintSpec,
    pub stroke_width: f32,
    pub stroke_opacity: f32,
    pub stroke_linecap: LineCap,
    pub stroke_linejoin: LineJoin,
    pub stroke_miterlimit: f32,
    pub stroke_dasharray: Vec<f32>,
    pub stroke_dashoffset: f32,
    pub color: Color,
    pub font_size: f32,
    pub font_family: Family,
    pub font_weight: u16,
    pub font_italic: bool,
    pub text_anchor: TextAnchor,
    pub visible: bool,
    pub paint_order_stroke_first: bool,
    pub non_scaling_stroke: bool,
}

impl Default for Props {
    fn default() -> Self {
        Props {
            fill: PaintSpec::Color(Color::BLACK),
            fill_opacity: 1.0,
            fill_rule: FillRule::NonZero,
            stroke: PaintSpec::None,
            stroke_width: 1.0,
            stroke_opacity: 1.0,
            stroke_linecap: LineCap::Butt,
            stroke_linejoin: LineJoin::Miter,
            stroke_miterlimit: 4.0,
            stroke_dasharray: Vec::new(),
            stroke_dashoffset: 0.0,
            color: Color::BLACK,
            font_size: 16.0,
            font_family: Family::Sans,
            font_weight: 400,
            font_italic: false,
            text_anchor: TextAnchor::Start,
            visible: true,
            paint_order_stroke_first: false,
            non_scaling_stroke: false,
        }
    }
}

impl Props {
    /// Child properties: inherited values overridden by `spec`.
    /// `diag` is the viewport diagonal reference for percentage stroke widths.
    pub fn inherit(&self, spec: &Specified, diag: f32) -> Props {
        let mut p = self.clone();
        if let Some(v) = &spec.fill {
            p.fill = v.clone();
        }
        if let Some(v) = spec.fill_opacity {
            p.fill_opacity = v.clamp(0.0, 1.0);
        }
        if let Some(v) = spec.fill_rule {
            p.fill_rule = v;
        }
        if let Some(v) = &spec.stroke {
            p.stroke = v.clone();
        }
        if let Some(v) = spec.stroke_width {
            p.stroke_width = v.resolve(diag).max(0.0);
        }
        if let Some(v) = spec.stroke_opacity {
            p.stroke_opacity = v.clamp(0.0, 1.0);
        }
        if let Some(v) = spec.stroke_linecap {
            p.stroke_linecap = v;
        }
        if let Some(v) = spec.stroke_linejoin {
            p.stroke_linejoin = v;
        }
        if let Some(v) = spec.stroke_miterlimit {
            p.stroke_miterlimit = v.max(1.0);
        }
        if let Some(v) = &spec.stroke_dasharray {
            p.stroke_dasharray = v.clone();
        }
        if let Some(v) = spec.stroke_dashoffset {
            p.stroke_dashoffset = v;
        }
        if let Some(v) = spec.color {
            p.color = v;
        }
        if let Some(v) = spec.font_size {
            p.font_size = match v {
                Length::Px(px) => px,
                Length::Percent(pc) => self.font_size * pc / 100.0,
            }
            .max(0.0);
        }
        if let Some(v) = spec.font_family {
            p.font_family = v;
        }
        if let Some(v) = spec.font_weight {
            p.font_weight = v;
        }
        if let Some(v) = spec.font_italic {
            p.font_italic = v;
        }
        if let Some(v) = spec.text_anchor {
            p.text_anchor = v;
        }
        if let Some(v) = spec.visibility {
            p.visible = v;
        }
        if let Some(v) = spec.paint_order_stroke_first {
            p.paint_order_stroke_first = v;
        }
        if let Some(v) = spec.non_scaling_stroke {
            p.non_scaling_stroke = v;
        }
        p
    }

    /// Resolve a paint spec's color (`currentColor`).
    pub fn paint_color(&self, spec: &PaintSpec) -> Option<Color> {
        match spec {
            PaintSpec::None => None,
            PaintSpec::Color(c) => Some(*c),
            PaintSpec::CurrentColor => Some(self.color),
            PaintSpec::Url(_, fallback) => fallback.as_ref().and_then(|f| self.paint_color(f)),
        }
    }
}

/// Parse a `fill`/`stroke` value.
pub fn parse_paint(s: &str) -> Option<PaintSpec> {
    let s = s.trim();
    let lower = s.to_ascii_lowercase();
    if lower == "none" {
        return Some(PaintSpec::None);
    }
    if let Some(rest) = lower.strip_prefix("url(") {
        let close = rest.find(')')?;
        let id = rest[..close].trim().trim_matches(|c| c == '"' || c == '\'').trim_start_matches('#').to_string();
        let fallback = rest[close + 1..].trim();
        let fb = if fallback.is_empty() { None } else { parse_paint(fallback).map(Box::new) };
        return Some(PaintSpec::Url(id, fb));
    }
    match parse_color(s)? {
        ColorSpec::Color(c) => Some(PaintSpec::Color(c)),
        ColorSpec::CurrentColor => Some(PaintSpec::CurrentColor),
    }
}

fn parse_opacity(s: &str) -> Option<f32> {
    let s = s.trim();
    if let Some(p) = s.strip_suffix('%') {
        return parse_number(p).map(|v| (v / 100.0).clamp(0.0, 1.0));
    }
    parse_number(s).map(|v| v.clamp(0.0, 1.0))
}

fn parse_font_family(s: &str) -> Family {
    for fam in s.split(',') {
        let f = fam.trim().trim_matches(|c| c == '"' || c == '\'').to_ascii_lowercase();
        if f.is_empty() {
            continue;
        }
        if f == "monospace" || f.contains("mono") || f.contains("courier") || f.contains("consolas") || f.contains("menlo") {
            return Family::Mono;
        }
        if f == "serif" || f.contains("times") || f.contains("georgia") || f.contains("garamond") {
            return Family::Serif;
        }
        if f == "sans-serif" || f.contains("sans") || f.contains("arial") || f.contains("helvetica") || f.contains("roboto") || f == "system-ui" {
            return Family::Sans;
        }
    }
    Family::Sans
}

fn parse_weight(s: &str) -> Option<u16> {
    match s.trim().to_ascii_lowercase().as_str() {
        "normal" => Some(400),
        "bold" => Some(700),
        "bolder" => Some(700),
        "lighter" => Some(300),
        other => parse_number(other).map(|v| (v.clamp(1.0, 1000.0)) as u16),
    }
}

impl Specified {
    /// Apply one property (lower-cased name) with a string value.
    pub fn apply(&mut self, name: &str, value: &str) {
        let v = value.trim();
        if v.is_empty() || v.eq_ignore_ascii_case("inherit") {
            return;
        }
        match name {
            "fill" => self.fill = parse_paint(v),
            "fill-opacity" => self.fill_opacity = parse_opacity(v),
            "fill-rule" | "clip-rule" => {
                self.fill_rule = match v.to_ascii_lowercase().as_str() {
                    "evenodd" => Some(FillRule::EvenOdd),
                    "nonzero" => Some(FillRule::NonZero),
                    _ => None,
                }
            }
            "stroke" => self.stroke = parse_paint(v),
            "stroke-width" => self.stroke_width = parse_length(v),
            "stroke-opacity" => self.stroke_opacity = parse_opacity(v),
            "stroke-linecap" => {
                self.stroke_linecap = match v.to_ascii_lowercase().as_str() {
                    "butt" => Some(LineCap::Butt),
                    "round" => Some(LineCap::Round),
                    "square" => Some(LineCap::Square),
                    _ => None,
                }
            }
            "stroke-linejoin" => {
                self.stroke_linejoin = match v.to_ascii_lowercase().as_str() {
                    "miter" | "miter-clip" | "arcs" => Some(LineJoin::Miter),
                    "round" => Some(LineJoin::Round),
                    "bevel" => Some(LineJoin::Bevel),
                    _ => None,
                }
            }
            "stroke-miterlimit" => self.stroke_miterlimit = parse_number(v),
            "stroke-dasharray" => {
                if v.eq_ignore_ascii_case("none") {
                    self.stroke_dasharray = Some(Vec::new());
                } else {
                    let list: Vec<f32> = v
                        .split(|c: char| c == ',' || c.is_whitespace())
                        .filter(|t| !t.is_empty())
                        .filter_map(|t| parse_length(t).map(|l| l.resolve(100.0)))
                        .filter(|x| *x >= 0.0)
                        .collect();
                    if list.iter().sum::<f32>() > 0.0 {
                        self.stroke_dasharray = Some(list);
                    } else {
                        self.stroke_dasharray = Some(Vec::new());
                    }
                }
            }
            "stroke-dashoffset" => self.stroke_dashoffset = parse_length(v).map(|l| l.resolve(100.0)),
            "color" => {
                if let Some(ColorSpec::Color(c)) = parse_color(v) {
                    self.color = Some(c);
                }
            }
            "opacity" => self.opacity = parse_opacity(v),
            "font-size" => {
                self.font_size = match v.to_ascii_lowercase().as_str() {
                    "xx-small" => Some(Length::Px(9.0)),
                    "x-small" => Some(Length::Px(10.0)),
                    "small" => Some(Length::Px(13.0)),
                    "medium" => Some(Length::Px(16.0)),
                    "large" => Some(Length::Px(18.0)),
                    "x-large" => Some(Length::Px(24.0)),
                    "xx-large" => Some(Length::Px(32.0)),
                    _ => parse_length(v),
                }
            }
            "font-family" => self.font_family = Some(parse_font_family(v)),
            "font-weight" => self.font_weight = parse_weight(v),
            "font-style" => {
                self.font_italic = match v.to_ascii_lowercase().as_str() {
                    "italic" | "oblique" => Some(true),
                    "normal" => Some(false),
                    _ => None,
                }
            }
            "font" => {
                // Very small `font` shorthand: pick size and family.
                for tok in v.split_whitespace() {
                    if let Some(l) = parse_length(tok.split('/').next().unwrap_or(tok)) {
                        if tok.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                            self.font_size = Some(l);
                        }
                    }
                    if tok.eq_ignore_ascii_case("bold") {
                        self.font_weight = Some(700);
                    }
                    if tok.eq_ignore_ascii_case("italic") {
                        self.font_italic = Some(true);
                    }
                }
                if let Some(idx) = v.rfind(|c: char| c.is_ascii_digit()) {
                    let fam = v[idx + 1..].trim_start_matches(|c: char| c.is_alphanumeric() || c == '%' || c == '/' || c == '.');
                    if !fam.trim().is_empty() {
                        self.font_family = Some(parse_font_family(fam));
                    }
                }
            }
            "text-anchor" => {
                self.text_anchor = match v.to_ascii_lowercase().as_str() {
                    "start" => Some(TextAnchor::Start),
                    "middle" => Some(TextAnchor::Middle),
                    "end" => Some(TextAnchor::End),
                    _ => None,
                }
            }
            "visibility" => {
                self.visibility = match v.to_ascii_lowercase().as_str() {
                    "hidden" | "collapse" => Some(false),
                    "visible" => Some(true),
                    _ => None,
                }
            }
            "display" => self.display_none = v.eq_ignore_ascii_case("none"),
            "paint-order" => {
                let l = v.to_ascii_lowercase();
                self.paint_order_stroke_first = Some(l.starts_with("stroke") || (l.contains("stroke") && !l.starts_with("fill") && !l.starts_with("markers fill")));
            }
            "vector-effect" => self.non_scaling_stroke = Some(v.eq_ignore_ascii_case("non-scaling-stroke")),
            "clip-path" => {
                let l = v.trim().to_ascii_lowercase();
                if let Some(rest) = l.strip_prefix("url(") {
                    if let Some(close) = rest.find(')') {
                        let id = rest[..close].trim().trim_matches(|c| c == '"' || c == '\'').trim_start_matches('#');
                        self.clip_path = Some(id.to_string());
                    }
                } else if l == "none" {
                    self.clip_path = None;
                }
            }
            "stop-color" => self.stop_color = parse_color(v),
            "stop-opacity" => self.stop_opacity = parse_opacity(v),
            "overflow" => self.overflow_visible = Some(matches!(v.to_ascii_lowercase().as_str(), "visible" | "auto")),
            _ => {}
        }
    }
}

/// Presentation attributes we understand (lower-case, as the HTML parser
/// lower-cases attribute names).
pub const PRESENTATION_ATTRS: &[&str] = &[
    "fill", "fill-opacity", "fill-rule", "stroke", "stroke-width", "stroke-opacity", "stroke-linecap",
    "stroke-linejoin", "stroke-miterlimit", "stroke-dasharray", "stroke-dashoffset", "color", "opacity",
    "font-size", "font-family", "font-weight", "font-style", "text-anchor", "visibility", "display",
    "paint-order", "vector-effect", "clip-path", "stop-color", "stop-opacity", "overflow", "clip-rule",
];

/// Serialize CSS tokens back to text (so declaration values can reuse the
/// string parsers above).
pub fn tokens_to_string(tokens: &[Token]) -> String {
    let mut out = String::new();
    for t in tokens {
        match t {
            Token::Ident(s) => out.push_str(s),
            Token::Function(f) => {
                out.push_str(f);
                out.push('(');
            }
            Token::AtKeyword(a) => {
                out.push('@');
                out.push_str(a);
            }
            Token::Hash(h) => {
                out.push('#');
                out.push_str(h);
            }
            Token::String(s) => {
                out.push('"');
                out.push_str(s);
                out.push('"');
            }
            Token::Url(u) => {
                out.push_str("url(");
                out.push_str(u);
                out.push(')');
            }
            Token::Number(n) => out.push_str(&fmt_num(*n)),
            Token::Percentage(p) => {
                out.push_str(&fmt_num(*p));
                out.push('%');
            }
            Token::Dimension(n, u) => {
                out.push_str(&fmt_num(*n));
                out.push_str(u);
            }
            Token::Whitespace => out.push(' '),
            Token::Colon => out.push(':'),
            Token::Semicolon => out.push(';'),
            Token::Comma => out.push(','),
            Token::OpenBracket => out.push('['),
            Token::CloseBracket => out.push(']'),
            Token::OpenParen => out.push('('),
            Token::CloseParen => out.push(')'),
            Token::OpenBrace => out.push('{'),
            Token::CloseBrace => out.push('}'),
            Token::Delim(c) => out.push(*c),
            Token::BadString | Token::BadUrl | Token::Cdo | Token::Cdc => {}
        }
    }
    out
}

fn fmt_num(n: f32) -> String {
    if n.fract() == 0.0 && n.abs() < 1e9 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Parse the `points` attribute of polyline/polygon.
pub fn parse_points(s: &str) -> Vec<(f32, f32)> {
    let n = parse_number_list(s);
    n.chunks_exact(2).map(|c| (c[0], c[1])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_and_props() {
        assert_eq!(parse_paint("none"), Some(PaintSpec::None));
        assert_eq!(parse_paint("url(#g) red"), Some(PaintSpec::Url("g".into(), Some(Box::new(PaintSpec::Color(Color::rgb(255, 0, 0)))))));
        assert_eq!(parse_paint("currentColor"), Some(PaintSpec::CurrentColor));
        let mut s = Specified::default();
        s.apply("stroke-width", "2.5");
        s.apply("stroke-dasharray", "4 2");
        s.apply("fill-rule", "evenodd");
        s.apply("font-weight", "bold");
        s.apply("display", "none");
        let p = Props::default().inherit(&s, 100.0);
        assert_eq!(p.stroke_width, 2.5);
        assert_eq!(p.stroke_dasharray, vec![4.0, 2.0]);
        assert_eq!(p.fill_rule, FillRule::EvenOdd);
        assert_eq!(p.font_weight, 700);
        assert!(s.display_none);
    }

    #[test]
    fn token_round_trip() {
        let toks = rabbithole_css::Tokenizer::tokenize("url(#grad) rgb(1, 2, 3) 50% 2px");
        assert_eq!(tokens_to_string(&toks), "url(#grad) rgb(1, 2, 3) 50% 2px");
    }
}
