//! Flexbox and box-alignment properties (plus the legacy `-webkit-box-*`).

use super::ApplyCtx;
use crate::computed::*;
use crate::values;
use rabbithole_css::Token;

fn content_distribution(kw: &str, for_align_content: bool) -> Option<ContentDistribution> {
    Some(match kw {
        "flex-start" | "flow-start" | "start" | "left" | "self-start" => ContentDistribution::FlexStart,
        "flex-end" | "flow-end" | "end" | "right" | "self-end" => ContentDistribution::FlexEnd,
        "center" => ContentDistribution::Center,
        "space-between" | "justify" => ContentDistribution::SpaceBetween,
        "space-around" => ContentDistribution::SpaceAround,
        "space-evenly" => ContentDistribution::SpaceEvenly,
        "stretch" => ContentDistribution::Stretch,
        "normal" => {
            if for_align_content {
                ContentDistribution::Stretch
            } else {
                ContentDistribution::FlexStart
            }
        }
        "baseline" => ContentDistribution::FlexStart,
        _ => return None,
    })
}

fn item_align(kw: &str, allow_auto: bool) -> Option<ItemAlign> {
    Some(match kw {
        "auto" if allow_auto => ItemAlign::Auto,
        "stretch" => ItemAlign::Stretch,
        "normal" => ItemAlign::Normal,
        "flex-start" | "flow-start" | "start" | "self-start" | "left" => ItemAlign::FlexStart,
        "flex-end" | "flow-end" | "end" | "self-end" | "right" => ItemAlign::FlexEnd,
        // (`anchor-center` without an anchor behaves as center.)
        "center" | "anchor-center" => ItemAlign::Center,
        "baseline" => ItemAlign::Baseline,
        "legacy" => ItemAlign::Normal,
        _ => return None,
    })
}

/// One box-alignment value at the start of `kws`: its main keyword, its
/// keyword details and how many keywords it used (`safe center`, `last
/// baseline`, `legacy left`, ...). None when the value is invalid.
/// `physical`: `left` / `right` are allowed (the inline axis only).
fn align_prefix(kws: &[String], physical: bool) -> Option<(&str, AlignFlags, usize)> {
    let mut f = 0;
    let mut i = 0;
    let first = kws.first()?.as_str();
    let overflow = matches!(first, "safe" | "unsafe");
    let baseline_prefix = matches!(first, "first" | "last");
    if overflow || baseline_prefix {
        if overflow { f |= if first == "safe" { AlignFlags::SAFE } else { AlignFlags::UNSAFE }; }
        if first == "last" {
            f |= AlignFlags::LAST;
        }
        i = 1;
    }
    let mut kw = kws.get(i)?.as_str();
    i += 1;
    // `legacy` combines with left/right/center (justify-items).
    if physical && kw == "legacy" && kws.get(i).is_some_and(|k| matches!(k.as_str(), "left" | "right" | "center")) {
        f |= AlignFlags::LEGACY;
        kw = kws[i].as_str();
        i += 1;
    } else if physical && matches!(kw, "left" | "right" | "center") && kws.get(i).is_some_and(|k| k == "legacy") {
        f |= AlignFlags::LEGACY;
        i += 1;
    }
    f |= match kw {
        "self-start" | "self-end" => AlignFlags::SELF,
        "left" => AlignFlags::LEFT,
        "right" => AlignFlags::RIGHT,
        "normal" => AlignFlags::NORMAL,
        "flex-start" | "flex-end" | "flow-start" | "flow-end" => AlignFlags::FLEX,
        "anchor-center" => AlignFlags::ANCHOR_CENTER,
        "baseline" => AlignFlags::BASELINE,
        "legacy" => AlignFlags::LEGACY,
        _ => 0,
    };
    let positional = matches!(kw, "center" | "start" | "end" | "self-start" | "self-end" | "flex-start" | "flex-end" | "left" | "right" | "anchor-center" | "normal" | "flow-start" | "flow-end");
    if (overflow && (!positional || f & AlignFlags::LEGACY != 0)) || (baseline_prefix && kw != "baseline") || (!physical && matches!(kw, "left" | "right")) {
        return None;
    }
    Some((kw, AlignFlags(f), i))
}

/// Identifiers of a value (None if any component is not one).
fn idents(comps: &[&[Token]]) -> Option<Vec<String>> {
    comps.iter().map(|c| values::ident(c)).collect()
}

/// Alignment shorthands use two complete longhand values. Parsing both
/// before modifying style makes an invalid second component atomic.
fn longhands(name: &str) -> Option<(&str, Option<&'static str>)> {
    Some(match name {
        "place-content" => ("align-content", Some("justify-content")),
        "place-items" => ("align-items", Some("justify-items")),
        "place-self" => ("align-self", Some("justify-self")),
        "align-content" | "justify-content" | "align-items" | "justify-items" |
        "align-self" | "justify-self" => (name, None),
        _ => return None,
    })
}

#[derive(Clone, Copy)]
struct Alignment<'a> {
    keyword: &'a str,
    flags: AlignFlags,
}

impl Alignment<'_> {
    fn allowed(self, property: &str) -> bool {
        let kw = self.keyword;
        let legacy = self.flags.has(AlignFlags::LEGACY);
        let overflow = self.flags.has(AlignFlags::SAFE) || self.flags.has(AlignFlags::UNSAFE);
        if legacy && property != "justify-items" { return false; }
        if property.ends_with("content") {
            return !(kw == "normal" && overflow)
                && !matches!(kw, "self-start" | "self-end" | "justify")
                && (kw != "baseline" || property == "align-content")
                && content_distribution(kw, property == "align-content").is_some();
        }
        item_align(kw, property.ends_with("self")).is_some()
    }

    fn specified(self) -> String {
        let prefix = if self.flags.has(AlignFlags::LEGACY) && self.keyword != "legacy" { "legacy " }
            else if self.flags.has(AlignFlags::SAFE) { "safe " }
            else if self.flags.has(AlignFlags::UNSAFE) { "unsafe " }
            else if self.flags.has(AlignFlags::LAST) { "last " }
            else { "" };
        format!("{prefix}{}", self.keyword)
    }

    fn apply(self, property: &str, style: &mut ComputedStyle, parent: &ComputedStyle) {
        let flags = self.flags;
        if let Some(value) = content_distribution(self.keyword, property == "align-content") {
            match property {
                "align-content" => { style.align_content = value; style.align_content_flags = flags; return; }
                "justify-content" => { style.justify_content = value; style.justify_content_flags = flags; return; }
                _ => {}
            }
        }
        let Some(item) = item_align(self.keyword, property.ends_with("self")) else { return };
        let pair = (item, flags);
        match property {
            "align-items" => (style.align_items, style.align_items_flags) = pair,
            "align-self" => (style.align_self, style.align_self_flags) = pair,
            "justify-self" => (style.justify_self, style.justify_self_flags) = pair,
            "justify-items" => {
                (style.justify_items, style.justify_items_flags) = if self.keyword == "legacy" {
                    if parent.justify_items_flags.has(AlignFlags::LEGACY) { (parent.justify_items, parent.justify_items_flags) }
                    else { (ItemAlign::Normal, AlignFlags(AlignFlags::NORMAL)) }
                } else { pair };
            }
            _ => {}
        }
    }
}

fn parse_alignment<'a>(kws: &'a [String], first: &str, second: Option<&str>) -> Option<(Alignment<'a>, Option<Alignment<'a>>)> {
    let (keyword, flags, used) = align_prefix(kws, first.starts_with("justify"))?;
    let a = Alignment { keyword, flags };
    if !a.allowed(first) { return None; }
    let Some(second) = second else { return (used == kws.len()).then_some((a, None)) };
    let b = if used < kws.len() {
        let (keyword, flags, n) = align_prefix(&kws[used..], true)?;
        if used + n != kws.len() { return None; }
        Alignment { keyword, flags }
    } else if keyword == "baseline" && second == "justify-content" {
        Alignment { keyword: "start", flags: AlignFlags::default() }
    } else { a };
    b.allowed(second).then_some((a, Some(b)))
}

pub fn alignment_property(name: &str) -> bool { longhands(name).is_some() }

pub fn specified_alignment_values(name: &str, comps: &[&[Token]]) -> Option<(String, Option<String>)> {
    let (first, second) = longhands(name)?;
    let kws = idents(comps)?;
    let (a, b) = parse_alignment(&kws, first, second)?;
    Some((a.specified(),b.map(Alignment::specified)))
}

pub fn specified_alignment(name: &str, comps: &[&[Token]]) -> Option<String> {
    let (a,b) = specified_alignment_values(name,comps)?;
    match b {
        Some(b) if a != b => Some(format!("{a} {b}")),
        _ => Some(a),
    }
}

fn flex_basis(c: &[Token], ctx: &ApplyCtx) -> Option<(Length, bool)> {
    // The flex-basis grammar excludes negative literal lengths/percentages.
    // Inspect the tokens, not the resolved Length: math functions may produce
    // a negative result that is instead clamped when determining the used size.
    if matches!(c, [Token::Dimension(n, _)] | [Token::Percentage(n)] | [Token::Number(n)] if *n < 0.0) {
        return None;
    }
    if let Some(kw) = values::ident(c) {
        return match kw.as_str() {
            "auto" => Some((Length::Auto, false)),
            "content" => Some((Length::Auto, true)),
            "max-content" => Some((Length::MaxContent, false)),
            "min-content" => Some((Length::MinContent, false)),
            "fit-content" => Some((Length::FitContent, false)),
            _ => None,
        };
    }
    super::len_no_auto(c, ctx).map(|length| (length, false))
}

fn gap_value(c: &[Token], ctx: &ApplyCtx) -> Option<Length> {
    if values::is_ident(c, "normal") {
        return Some(Length::ZERO);
    }
    super::len_no_auto(c, ctx)
}

pub fn apply(name: &str, comps: &[&[Token]], style: &mut ComputedStyle, parent: &ComputedStyle, ctx: &ApplyCtx) -> bool {
    let name = match name {
        "-webkit-flex" | "-ms-flex" => "flex",
        "-webkit-flex-grow" | "-ms-flex-positive" => "flex-grow",
        "-webkit-flex-shrink" | "-ms-flex-negative" => "flex-shrink",
        "-webkit-flex-basis" | "-ms-flex-preferred-size" => "flex-basis",
        "-webkit-flex-wrap" | "-ms-flex-wrap" => "flex-wrap",
        "-webkit-flex-flow" | "-ms-flex-flow" => "flex-flow",
        "-webkit-justify-content" | "-ms-flex-pack" => "justify-content",
        "-webkit-align-items" | "-ms-flex-align" => "align-items",
        "-webkit-align-self" | "-ms-flex-item-align" => "align-self",
        "-webkit-align-content" | "-ms-flex-line-pack" => "align-content",
        "-webkit-order" | "-ms-flex-order" => "order",
        "grid-gap" => "gap",
        "grid-row-gap" => "row-gap",
        "grid-column-gap" => "column-gap",
        n => n,
    };
    let c0 = comps[0];
    match name {
        "flex" => {
            if comps.len() == 1 {
                if let Some(kw) = values::ident(c0) {
                    match kw.as_str() {
                        "none" => {
                            style.flex_grow = 0.0;
                            style.flex_shrink = 0.0;
                            style.flex_basis = Length::Auto;
                            style.flex_basis_content = false;
                            return true;
                        }
                        "auto" => {
                            style.flex_grow = 1.0;
                            style.flex_shrink = 1.0;
                            style.flex_basis = Length::Auto;
                            style.flex_basis_content = false;
                            return true;
                        }
                        _ => {}
                    }
                }
            }
            let mut grow = None;
            let mut shrink = None;
            let mut basis = None;
            for c in comps {
                if let Some(n) = values::number(c) {
                    if n < 0.0 && matches!(c, [Token::Number(_)]) {
                        return true;
                    }
                    let n = n.max(0.0);
                    if grow.is_none() {
                        grow = Some(n);
                    } else if shrink.is_none() {
                        shrink = Some(n);
                    } else if basis.is_none() && n == 0.0 {
                        basis = Some((Length::ZERO, false));
                    } else {
                        return true;
                    }
                } else if let Some(b) = flex_basis(c, ctx) {
                    if basis.is_some() {
                        return true;
                    }
                    basis = Some(b);
                } else {
                    return true;
                }
            }
            if grow.is_none() && basis.is_none() {
                return true;
            }
            style.flex_grow = grow.unwrap_or(1.0);
            style.flex_shrink = shrink.unwrap_or(1.0);
            let (length, content) = basis.unwrap_or((if grow.is_some() { Length::Percent(0.0) } else { Length::Auto }, false));
            style.flex_basis = length;
            style.flex_basis_content = content;
        }
        // Negative literal factors are invalid; math functions clamp.
        "flex-grow" | "-webkit-box-flex" => {
            if let Some(n) = values::number(c0).filter(|n| *n >= 0.0 || !matches!(c0, [Token::Number(_)])) {
                style.flex_grow = n.max(0.0);
            }
        }
        "flex-shrink" => {
            if let Some(n) = values::number(c0).filter(|n| *n >= 0.0 || !matches!(c0, [Token::Number(_)])) {
                style.flex_shrink = n.max(0.0);
            }
        }
        "flex-basis" => {
            if let Some((length, content)) = flex_basis(c0, ctx) {
                style.flex_basis = length;
                style.flex_basis_content = content;
            }
        }
        "flex-wrap" => {
            // `nowrap | wrap | wrap-reverse | [balance || wrap-reverse]`.
            let Some(kws) = idents(comps) else { return true };
            let (wrap, balance) = match kws.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
                ["nowrap"] => (FlexWrap::NoWrap, false),
                ["wrap"] => (FlexWrap::Wrap, false),
                ["wrap-reverse"] => (FlexWrap::WrapReverse, false),
                ["balance"] => (FlexWrap::Wrap, true),
                ["balance", "wrap-reverse"] | ["wrap-reverse", "balance"] => (FlexWrap::WrapReverse, true),
                _ => return true,
            };
            style.flex_wrap = wrap;
            style.flex_wrap_balance = balance;
        }
        "flex-line-count" => {
            if values::is_ident(c0, "auto") {
                style.flex_line_count = None;
            } else if let Some(n) = values::number(c0).filter(|n| comps.len() == 1 && *n >= 1.0 && n.fract() == 0.0) {
                style.flex_line_count = Some(n.min(1.0e6) as u32);
            }
        }
        "flex-flow" => {
            style.flex_wrap_balance = false;
            for c in comps {
                if let Some(kw) = values::ident(c) {
                    match kw.as_str() {
                        "row" => style.flex_direction = FlexDirection::Row,
                        "row-reverse" => style.flex_direction = FlexDirection::RowReverse,
                        "column" => style.flex_direction = FlexDirection::Column,
                        "column-reverse" => style.flex_direction = FlexDirection::ColumnReverse,
                        "nowrap" => style.flex_wrap = FlexWrap::NoWrap,
                        "wrap" => style.flex_wrap = FlexWrap::Wrap,
                        "wrap-reverse" => style.flex_wrap = FlexWrap::WrapReverse,
                        "balance" => {
                            style.flex_wrap_balance = true;
                            if style.flex_wrap == FlexWrap::NoWrap {
                                style.flex_wrap = FlexWrap::Wrap;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        "align-content" | "justify-content" | "align-items" | "justify-items" | "align-self" | "justify-self"
        | "place-content" | "place-items" | "place-self" => {
            let (first, second) = longhands(name).unwrap();
            let Some(words) = idents(comps) else { return true };
            if let Some((a, b)) = parse_alignment(&words, first, second) {
                a.apply(first, style, parent);
                if let (Some(value), Some(property)) = (b, second) { value.apply(property, style, parent); }
            }
        }
        "order" => {
            if let Some(n) = values::number(c0) {
                style.order = n as i32;
            }
        }
        "gap" => {
            let vals: Vec<(Length,bool)> = comps.iter().filter_map(|c| gap_value(c, ctx).map(|value| (value, values::is_ident(c,"normal")))).collect();
            match vals.len() {
                1 => {
                    style.row_gap = vals[0].0;
                    style.column_gap = vals[0].0;
                    style.row_gap_normal = vals[0].1;
                    style.column_gap_normal = vals[0].1;
                }
                2 => {
                    style.row_gap = vals[0].0;
                    style.column_gap = vals[1].0;
                    style.row_gap_normal = vals[0].1;
                    style.column_gap_normal = vals[1].1;
                }
                _ => {}
            }
        }
        "row-gap" => {
            if let Some(v) = gap_value(c0, ctx) {
                style.row_gap = v;
                style.row_gap_normal = values::is_ident(c0,"normal");
            }
        }
        "column-gap" => {
            if let Some(v) = gap_value(c0, ctx) {
                style.column_gap = v;
                style.column_gap_normal = values::is_ident(c0,"normal");
            }
        }
        // Legacy WebKit box model.
        "-webkit-box-orient" => {
            if let Some(kw) = values::ident(c0) {
                match kw.as_str() {
                    "vertical" | "block-axis" => style.flex_direction = FlexDirection::Column,
                    "horizontal" | "inline-axis" => style.flex_direction = FlexDirection::Row,
                    _ => {}
                }
            }
        }
        "-webkit-box-pack" => {
            if let Some(kw) = values::ident(c0) {
                style.justify_content_flags = AlignFlags::default();
                style.justify_content = match kw.as_str() {
                    "start" => ContentDistribution::FlexStart,
                    "end" => ContentDistribution::FlexEnd,
                    "center" => ContentDistribution::Center,
                    "justify" => ContentDistribution::SpaceBetween,
                    _ => return true,
                };
            }
        }
        "-webkit-box-align" => {
            if let Some(kw) = values::ident(c0) {
                style.align_items_flags = AlignFlags::default();
                style.align_items = match kw.as_str() {
                    "start" => ItemAlign::FlexStart,
                    "end" => ItemAlign::FlexEnd,
                    "center" => ItemAlign::Center,
                    "baseline" => ItemAlign::Baseline,
                    "stretch" => ItemAlign::Stretch,
                    _ => return true,
                };
            }
        }
        "-webkit-box-direction" | "-webkit-box-ordinal-group" | "-webkit-box-lines" => {}
        _ => return false,
    }
    let _ = parent;
    true
}
