//! Flexbox and box-alignment properties (plus the legacy `-webkit-box-*`).

use super::ApplyCtx;
use crate::computed::*;
use crate::values;
use rabbithole_css::Token;

fn content_distribution(kw: &str, for_align_content: bool) -> Option<ContentDistribution> {
    Some(match kw {
        "flex-start" | "start" | "left" | "self-start" => ContentDistribution::FlexStart,
        "flex-end" | "end" | "right" | "self-end" => ContentDistribution::FlexEnd,
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
        "baseline" | "first" | "last" => ContentDistribution::FlexStart,
        _ => return None,
    })
}

fn item_align(kw: &str, allow_auto: bool) -> Option<ItemAlign> {
    Some(match kw {
        "auto" if allow_auto => ItemAlign::Auto,
        "stretch" => ItemAlign::Stretch,
        "normal" => ItemAlign::Normal,
        "flex-start" | "start" | "self-start" | "left" => ItemAlign::FlexStart,
        "flex-end" | "end" | "self-end" | "right" => ItemAlign::FlexEnd,
        "center" => ItemAlign::Center,
        "baseline" | "first" | "last" => ItemAlign::Baseline,
        "legacy" => ItemAlign::Normal,
        _ => return None,
    })
}

/// Keywords in a value, skipping `safe`/`unsafe`/`first`/`last` modifiers.
fn keywords(comps: &[&[Token]]) -> Vec<String> {
    comps
        .iter()
        .filter_map(|c| values::ident(c))
        .filter(|k| !matches!(k.as_str(), "safe" | "unsafe"))
        .collect()
}

fn flex_basis(c: &[Token], ctx: &ApplyCtx) -> Option<Length> {
    // The flex-basis grammar excludes negative literal lengths/percentages.
    // Inspect the tokens, not the resolved Length: math functions may produce
    // a negative result that is instead clamped when determining the used size.
    if matches!(c, [Token::Dimension(n, _)] | [Token::Percentage(n)] | [Token::Number(n)] if *n < 0.0) {
        return None;
    }
    if let Some(kw) = values::ident(c) {
        return match kw.as_str() {
            "auto" | "content" => Some(Length::Auto),
            "max-content" => Some(Length::MaxContent),
            "min-content" => Some(Length::MinContent),
            "fit-content" => Some(Length::FitContent),
            _ => None,
        };
    }
    super::len_no_auto(c, ctx)
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
                            return true;
                        }
                        "auto" => {
                            style.flex_grow = 1.0;
                            style.flex_shrink = 1.0;
                            style.flex_basis = Length::Auto;
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
                    if grow.is_none() {
                        grow = Some(n);
                    } else if shrink.is_none() {
                        shrink = Some(n);
                    } else if basis.is_none() && n == 0.0 {
                        basis = Some(Length::ZERO);
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
            style.flex_basis = basis.unwrap_or(if grow.is_some() { Length::Percent(0.0) } else { Length::Auto });
        }
        "flex-grow" | "-webkit-box-flex" => {
            if let Some(n) = values::number(c0) {
                style.flex_grow = n.max(0.0);
            }
        }
        "flex-shrink" => {
            if let Some(n) = values::number(c0) {
                style.flex_shrink = n.max(0.0);
            }
        }
        "flex-basis" => {
            if let Some(b) = flex_basis(c0, ctx) {
                style.flex_basis = b;
            }
        }
        "flex-wrap" => {
            if let Some(kw) = values::ident(c0) {
                style.flex_wrap = match kw.as_str() {
                    "nowrap" => FlexWrap::NoWrap,
                    "wrap" => FlexWrap::Wrap,
                    "wrap-reverse" => FlexWrap::WrapReverse,
                    _ => return true,
                };
            }
        }
        "flex-flow" => {
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
                        _ => {}
                    }
                }
            }
        }
        "justify-content" => {
            if let Some(kw) = keywords(comps).first() {
                if let Some(v) = content_distribution(kw, false) {
                    style.justify_content = v;
                }
            }
        }
        "align-content" => {
            if let Some(kw) = keywords(comps).first() {
                if let Some(v) = content_distribution(kw, true) {
                    style.align_content = v;
                }
            }
        }
        "align-items" => {
            if let Some(kw) = keywords(comps).first() {
                if let Some(v) = item_align(kw, false) {
                    style.align_items = v;
                }
            }
        }
        "align-self" => {
            if let Some(kw) = keywords(comps).first() {
                if let Some(v) = item_align(kw, true) {
                    style.align_self = v;
                }
            }
        }
        "justify-items" => {
            if let Some(kw) = keywords(comps).first() {
                if let Some(v) = item_align(kw, false) {
                    style.justify_items = v;
                }
            }
        }
        "justify-self" => {
            if let Some(kw) = keywords(comps).first() {
                if let Some(v) = item_align(kw, true) {
                    style.justify_self = v;
                }
            }
        }
        "place-items" => {
            let kws = keywords(comps);
            if let Some(a) = kws.first().and_then(|k| item_align(k, false)) {
                style.align_items = a;
                style.justify_items = kws.get(1).and_then(|k| item_align(k, false)).unwrap_or(a);
            }
        }
        "place-self" => {
            let kws = keywords(comps);
            if let Some(a) = kws.first().and_then(|k| item_align(k, true)) {
                style.align_self = a;
                style.justify_self = kws.get(1).and_then(|k| item_align(k, true)).unwrap_or(a);
            }
        }
        "place-content" => {
            let kws = keywords(comps);
            if let Some(a) = kws.first().and_then(|k| content_distribution(k, true)) {
                style.align_content = a;
                style.justify_content = kws.get(1).and_then(|k| content_distribution(k, false)).unwrap_or(a);
            }
        }
        "order" => {
            if let Some(n) = values::number(c0) {
                style.order = n as i32;
            }
        }
        "gap" => {
            let vals: Vec<Length> = comps.iter().filter_map(|c| gap_value(c, ctx)).collect();
            match vals.len() {
                1 => {
                    style.row_gap = vals[0];
                    style.column_gap = vals[0];
                }
                2 => {
                    style.row_gap = vals[0];
                    style.column_gap = vals[1];
                }
                _ => {}
            }
        }
        "row-gap" => {
            if let Some(v) = gap_value(c0, ctx) {
                style.row_gap = v;
            }
        }
        "column-gap" => {
            if let Some(v) = gap_value(c0, ctx) {
                style.column_gap = v;
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
