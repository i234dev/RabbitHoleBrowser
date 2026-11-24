//! CSS Multi-column Layout 1 and CSS Fragmentation 3 properties:
//! `columns`, `column-count`, `column-width`, `column-fill`,
//! `column-span`, `column-rule*`, `break-*` (and the legacy `page-break-*`),
//! `orphans` and `widows`.

use super::ApplyCtx;
use crate::computed::*;
use crate::values::{self, ColorValue};
use rabbithole_css::Token;

/// `break-before` / `break-after` values ([`ComputedStyle::break_before`]).
pub const BREAK_AUTO: u8 = 0;
pub const BREAK_AVOID: u8 = 1;
pub const BREAK_AVOID_COLUMN: u8 = 2;
pub const BREAK_AVOID_PAGE: u8 = 3;
pub const BREAK_COLUMN: u8 = 4;
pub const BREAK_PAGE: u8 = 5;
pub const BREAK_LEFT: u8 = 6;
pub const BREAK_RIGHT: u8 = 7;
pub const BREAK_RECTO: u8 = 8;
pub const BREAK_VERSO: u8 = 9;
pub const BREAK_ALWAYS: u8 = 10;
pub const BREAK_ALL: u8 = 11;
pub const BREAK_AVOID_REGION: u8 = 12;
pub const BREAK_REGION: u8 = 13;

const BREAK_NAMES: [&str; 14] =
    ["auto", "avoid", "avoid-column", "avoid-page", "column", "page", "left", "right", "recto", "verso", "always", "all", "avoid-region", "region"];

/// Is a `break-before` / `break-after` value a forced break in a
/// multi-column context? (Page breaks also break columns.)
pub fn is_forced_break(v: u8) -> bool {
    v >= BREAK_COLUMN && v != BREAK_AVOID_REGION && v != BREAK_REGION
}

/// Does a break value avoid column breaks?
pub fn avoids_column_break(v: u8) -> bool {
    matches!(v, BREAK_AVOID | BREAK_AVOID_COLUMN)
}

/// CSS keyword of a `break-*` value.
pub fn break_css(v: u8) -> &'static str {
    BREAK_NAMES.get(v as usize).copied().unwrap_or("auto")
}

fn break_between(kw: &str) -> Option<u8> {
    BREAK_NAMES.iter().position(|n| *n == kw).map(|i| i as u8)
}

fn break_inside(kw: &str) -> Option<u8> {
    Some(match kw {
        "auto" => BREAK_AUTO,
        "avoid" => BREAK_AVOID,
        "avoid-column" => BREAK_AVOID_COLUMN,
        "avoid-page" => BREAK_AVOID_PAGE,
        "avoid-region" => BREAK_AVOID_REGION,
        _ => return None,
    })
}

fn rule_style(kw: &str) -> Option<BorderStyle> {
    Some(match kw {
        "none" => BorderStyle::None,
        "hidden" => BorderStyle::Hidden,
        "solid" | "groove" | "ridge" | "inset" | "outset" => BorderStyle::Solid,
        "dashed" => BorderStyle::Dashed,
        "dotted" => BorderStyle::Dotted,
        "double" => BorderStyle::Double,
        _ => return None,
    })
}

/// The keyword of a `column-rule-style` as specified (the computed style
/// folds the 3D styles into `solid`).
fn rule_style_kw(kw: &str) -> Option<u8> {
    ["none", "hidden", "dotted", "dashed", "solid", "double", "groove", "ridge", "inset", "outset"]
        .iter()
        .position(|n| *n == kw)
        .map(|i| i as u8)
}

/// CSS keyword of [`ComputedStyle::column_rule_style_kw`].
pub fn rule_style_css(v: u8) -> &'static str {
    ["none", "hidden", "dotted", "dashed", "solid", "double", "groove", "ridge", "inset", "outset"].get(v as usize).copied().unwrap_or("none")
}

fn rule_width(c: &[Token], ctx: &ApplyCtx) -> Option<f32> {
    if let Some(kw) = values::ident(c) {
        return match kw.as_str() {
            "thin" => Some(1.0),
            "medium" => Some(3.0),
            "thick" => Some(5.0),
            _ => None,
        };
    }
    if matches!(c, [Token::Percentage(_)]) {
        return None;
    }
    let v = super::px(c, ctx)?;
    // (A math function's result is clamped; a negative literal is invalid.)
    let v = if matches!(c.first(), Some(Token::Function(_))) { v.max(0.0) } else { v };
    if v < 0.0 {
        return None;
    }
    // Widths snap to whole device pixels, never below 1px when not zero.
    Some(if v > 0.0 && v < 1.0 { 1.0 } else { (v + 0.01).floor() })
}

/// `column-width` (`auto` = `None`); negative lengths are invalid.
fn column_width(c: &[Token], ctx: &ApplyCtx) -> Option<Option<f32>> {
    if values::is_ident(c, "auto") {
        return Some(None);
    }
    if matches!(c, [Token::Percentage(_)]) {
        return None;
    }
    let v = super::px(c, ctx)?;
    let v = if matches!(c.first(), Some(Token::Function(_))) { v.max(0.0) } else { v };
    (v >= 0.0).then_some(Some(v))
}

/// `column-count` (`auto` = `None`); a positive integer.
fn column_count(c: &[Token]) -> Option<Option<u32>> {
    if values::is_ident(c, "auto") {
        return Some(None);
    }
    let n = match c {
        [Token::Number(n)] => *n,
        [Token::Function(_), ..] => super::math_value(c, &|t| match t {
            Token::Number(n) => Some(*n),
            _ => None,
        })?,
        _ => return None,
    };
    if c.len() == 1 && n.fract() != 0.0 {
        return None;
    }
    let n = n.round();
    (n >= 1.0).then(|| Some(n.min(1000.0) as u32))
}

/// `orphans` / `widows`: a positive integer.
fn positive_integer(c: &[Token]) -> Option<u16> {
    match c {
        [Token::Number(n)] if n.fract() == 0.0 && *n >= 1.0 => Some(n.min(u16::MAX as f32) as u16),
        [Token::Function(_), ..] => {
            let n = super::math_value(c, &|t| match t {
                Token::Number(n) => Some(*n),
                _ => None,
            })?
            .round();
            (n >= 1.0).then(|| n.min(u16::MAX as f32) as u16)
        }
        _ => None,
    }
}

fn set_multicol(style: &mut ComputedStyle) {
    style.multicol = style.column_count.is_some() || style.column_width.is_some();
}

/// Apply a multi-column / fragmentation property. Returns true when the
/// property is one of them (even if the value was invalid).
pub fn apply(name: &str, comps: &[&[Token]], style: &mut ComputedStyle, ctx: &ApplyCtx) -> bool {
    let name = match name {
        "-webkit-columns" | "-moz-columns" => "columns",
        "-webkit-column-count" | "-moz-column-count" => "column-count",
        "-webkit-column-width" | "-moz-column-width" => "column-width",
        "-webkit-column-fill" | "-moz-column-fill" => "column-fill",
        "-webkit-column-span" => "column-span",
        "-webkit-column-rule" | "-moz-column-rule" => "column-rule",
        "-webkit-column-rule-width" | "-moz-column-rule-width" => "column-rule-width",
        "-webkit-column-rule-style" | "-moz-column-rule-style" => "column-rule-style",
        "-webkit-column-rule-color" | "-moz-column-rule-color" => "column-rule-color",
        n => n,
    };
    let Some(c0) = comps.first().copied() else { return false };
    let single = comps.len() == 1;
    match name {
        "columns" => {
            // `<'column-width'> || <'column-count'>`, each at most once;
            // `auto` leaves the other one to the other keyword.
            if comps.len() > 2 {
                return true;
            }
            let (mut width, mut count): (Option<Option<f32>>, Option<Option<u32>>) = (None, None);
            let mut autos = 0;
            for c in comps {
                if values::is_ident(c, "auto") {
                    autos += 1;
                    continue;
                }
                if let Some(n) = column_count(c).filter(|_| count.is_none()) {
                    count = Some(n);
                } else if let Some(w) = column_width(c, ctx).filter(|_| width.is_none()) {
                    width = Some(w);
                } else {
                    return true;
                }
            }
            if autos > 0 && autos + width.is_some() as usize + count.is_some() as usize > 2 {
                return true;
            }
            style.column_width = width.flatten();
            style.column_count = count.flatten();
            set_multicol(style);
        }
        "column-count" if single => {
            if let Some(n) = column_count(c0) {
                style.column_count = n;
                set_multicol(style);
            }
        }
        "column-width" if single => {
            if let Some(w) = column_width(c0, ctx) {
                style.column_width = w;
                set_multicol(style);
            }
        }
        "column-fill" if single => match values::ident(c0).as_deref() {
            Some("auto") => style.column_fill_auto = true,
            Some("balance") | Some("balance-all") => style.column_fill_auto = false,
            _ => {}
        },
        "column-span" if single => match values::ident(c0).as_deref() {
            Some("all") => style.column_span_all = true,
            Some("none") => style.column_span_all = false,
            _ => {}
        },
        "column-rule-width" if single => {
            if let Some(w) = rule_width(c0, ctx) {
                style.column_rule_width = w;
            }
        }
        "column-rule-style" if single => {
            if let Some(kw) = values::ident(c0) {
                if let (Some(s), Some(k)) = (rule_style(&kw), rule_style_kw(&kw)) {
                    style.column_rule_style = s;
                    style.column_rule_style_kw = k;
                }
            }
        }
        "column-rule-color" if single => match values::parse_color_component(c0) {
            Some(ColorValue::Color(col)) => style.column_rule_color = Some(col),
            Some(ColorValue::CurrentColor) => style.column_rule_color = None,
            None => {}
        },
        "column-rule" => {
            let (mut width, mut kind, mut color): (Option<f32>, Option<(BorderStyle, u8)>, Option<Option<Color>>) = (None, None, None);
            for c in comps {
                if let Some(s) = values::ident(c).and_then(|kw| Some((rule_style(&kw)?, rule_style_kw(&kw)?))).filter(|_| kind.is_none()) {
                    kind = Some(s);
                } else if let Some(w) = rule_width(c, ctx).filter(|_| width.is_none()) {
                    width = Some(w);
                } else if let Some(v) = values::parse_color_component(c).filter(|_| color.is_none()) {
                    color = Some(match v {
                        ColorValue::Color(col) => Some(col),
                        ColorValue::CurrentColor => None,
                    });
                } else {
                    return true;
                }
            }
            let (s, k) = kind.unwrap_or((BorderStyle::None, 0));
            style.column_rule_style = s;
            style.column_rule_style_kw = k;
            style.column_rule_width = width.unwrap_or(3.0);
            style.column_rule_color = color.flatten();
        }
        "break-before" | "break-after" if single => {
            if let Some(v) = values::ident(c0).and_then(|kw| break_between(&kw)) {
                if name == "break-before" {
                    style.break_before = v;
                } else {
                    style.break_after = v;
                }
            }
        }
        "break-inside" if single => {
            if let Some(v) = values::ident(c0).and_then(|kw| break_inside(&kw)) {
                style.break_inside = v;
            }
        }
        // Legacy aliases (CSS Fragmentation 3 §3.4).
        "page-break-before" | "page-break-after" if single => {
            let v = match values::ident(c0).as_deref() {
                Some("auto") => Some(BREAK_AUTO),
                Some("always") => Some(BREAK_PAGE),
                Some("avoid") => Some(BREAK_AVOID),
                Some("left") => Some(BREAK_LEFT),
                Some("right") => Some(BREAK_RIGHT),
                _ => None,
            };
            if let Some(v) = v {
                if name == "page-break-before" {
                    style.break_before = v;
                } else {
                    style.break_after = v;
                }
            }
        }
        "page-break-inside" if single => match values::ident(c0).as_deref() {
            Some("auto") => style.break_inside = BREAK_AUTO,
            Some("avoid") => style.break_inside = BREAK_AVOID,
            _ => {}
        },
        "-webkit-column-break-before" | "-webkit-column-break-after" if single => {
            let v = match values::ident(c0).as_deref() {
                Some("auto") => Some(BREAK_AUTO),
                Some("always") => Some(BREAK_COLUMN),
                Some("avoid") => Some(BREAK_AVOID_COLUMN),
                _ => None,
            };
            if let Some(v) = v {
                if name == "-webkit-column-break-before" {
                    style.break_before = v;
                } else {
                    style.break_after = v;
                }
            }
        }
        "-webkit-column-break-inside" if single => match values::ident(c0).as_deref() {
            Some("auto") => style.break_inside = BREAK_AUTO,
            Some("avoid") => style.break_inside = BREAK_AVOID_COLUMN,
            _ => {}
        },
        "orphans" if single => {
            if let Some(n) = positive_integer(c0) {
                style.orphans = n;
            }
        }
        "widows" if single => {
            if let Some(n) = positive_integer(c0) {
                style.widows = n;
            }
        }
        "column-count" | "column-width" | "column-fill" | "column-span" | "column-rule-width" | "column-rule-style"
        | "column-rule-color" | "break-before" | "break-after" | "break-inside" | "page-break-before" | "page-break-after"
        | "page-break-inside" | "-webkit-column-break-before" | "-webkit-column-break-after" | "-webkit-column-break-inside" | "orphans" | "widows" => {}
        _ => return false,
    }
    true
}
