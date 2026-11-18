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

/// One box-alignment value at the start of `kws`: its main keyword, its
/// modifiers and how many keywords it used (`safe center`, `last
/// baseline`, `legacy left`, ...). None when the value is invalid.
fn align_prefix(kws: &[String]) -> Option<(&str, AlignMods, usize)> {
    let mut mods = AlignMods::default();
    let mut i = 0;
    let first = kws.first()?.as_str();
    let overflow = matches!(first, "safe" | "unsafe");
    let baseline_prefix = matches!(first, "first" | "last");
    if overflow || baseline_prefix {
        mods.safe = first == "safe";
        mods.last = first == "last";
        i = 1;
    }
    let mut kw = kws.get(i)?.as_str();
    i += 1;
    // `legacy` combines with left/right/center (justify-items).
    if kw == "legacy" && kws.get(i).is_some_and(|k| matches!(k.as_str(), "left" | "right" | "center")) {
        kw = kws[i].as_str();
        i += 1;
    } else if matches!(kw, "left" | "right" | "center") && kws.get(i).is_some_and(|k| k == "legacy") {
        i += 1;
    }
    mods.pos = match kw {
        "start" => AlignPos::Start,
        "end" => AlignPos::End,
        "self-start" => AlignPos::SelfStart,
        "self-end" => AlignPos::SelfEnd,
        "left" => AlignPos::Left,
        "right" => AlignPos::Right,
        _ => AlignPos::Flex,
    };
    let positional = matches!(kw, "center" | "start" | "end" | "self-start" | "self-end" | "flex-start" | "flex-end" | "left" | "right");
    if (overflow && !positional) || (baseline_prefix && kw != "baseline") {
        return None;
    }
    Some((kw, mods, i))
}

/// Identifiers of a value (None if any component is not one).
fn idents(comps: &[&[Token]]) -> Option<Vec<String>> {
    comps.iter().map(|c| values::ident(c)).collect()
}

/// A whole value of an item-alignment property.
fn item_value(comps: &[&[Token]], allow_auto: bool) -> Option<(ItemAlign, AlignMods)> {
    let kws = idents(comps)?;
    let (kw, mods, used) = align_prefix(&kws)?;
    (used == kws.len()).then_some(())?;
    Some((item_align(kw, allow_auto)?, mods))
}

/// A whole value of a content-distribution property.
fn content_value(comps: &[&[Token]], for_align_content: bool) -> Option<(ContentDistribution, AlignMods)> {
    let kws = idents(comps)?;
    let (kw, mods, used) = align_prefix(&kws)?;
    (used == kws.len()).then_some(())?;
    Some((content_distribution(kw, for_align_content)?, mods))
}

/// The two values of a `place-*` shorthand (the second defaults to the
/// first).
fn place_values(comps: &[&[Token]]) -> Option<((String, AlignMods), Option<(String, AlignMods)>)> {
    let kws = idents(comps)?;
    let (a, am, used) = align_prefix(&kws)?;
    let first = (a.to_string(), am);
    if used == kws.len() {
        return Some((first, None));
    }
    let (b, bm, used2) = align_prefix(&kws[used..])?;
    (used + used2 == kws.len()).then_some(())?;
    Some((first, Some((b.to_string(), bm))))
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
            if let Some((length, content)) = flex_basis(c0, ctx) {
                style.flex_basis = length;
                style.flex_basis_content = content;
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
            if let Some((v, m)) = content_value(comps, false) {
                style.justify_content = v;
                style.justify_content_mods = m;
            }
        }
        "align-content" => {
            if let Some((v, m)) = content_value(comps, true) {
                style.align_content = v;
                style.align_content_mods = m;
            }
        }
        "align-items" => {
            if let Some((v, m)) = item_value(comps, false) {
                style.align_items = v;
                style.align_items_mods = m;
            }
        }
        "align-self" => {
            if let Some((v, m)) = item_value(comps, true) {
                style.align_self = v;
                style.align_self_mods = m;
            }
        }
        "justify-items" => {
            if let Some((v, m)) = item_value(comps, false) {
                style.justify_items = v;
                style.justify_items_mods = m;
            }
        }
        "justify-self" => {
            if let Some((v, m)) = item_value(comps, true) {
                style.justify_self = v;
                style.justify_self_mods = m;
            }
        }
        "place-items" => {
            if let Some(((a, am), second)) = place_values(comps) {
                let (b, bm) = second.unwrap_or_else(|| (a.clone(), am));
                if let (Some(av), Some(bv)) = (item_align(&a, false), item_align(&b, false)) {
                    style.align_items = av;
                    style.align_items_mods = am;
                    style.justify_items = bv;
                    style.justify_items_mods = bm;
                }
            }
        }
        "place-self" => {
            if let Some(((a, am), second)) = place_values(comps) {
                let (b, bm) = second.unwrap_or_else(|| (a.clone(), am));
                if let (Some(av), Some(bv)) = (item_align(&a, true), item_align(&b, true)) {
                    style.align_self = av;
                    style.align_self_mods = am;
                    style.justify_self = bv;
                    style.justify_self_mods = bm;
                }
            }
        }
        "place-content" => {
            if let Some(((a, am), second)) = place_values(comps) {
                // A single baseline value sets justify-content to `start`.
                let (b, bm) = second.unwrap_or_else(|| if a == "baseline" { ("start".to_string(), AlignMods { pos: AlignPos::Start, ..AlignMods::default() }) } else { (a.clone(), am) });
                if let (Some(av), Some(bv)) = (content_distribution(&a, true), content_distribution(&b, false)) {
                    style.align_content = av;
                    style.align_content_mods = am;
                    style.justify_content = bv;
                    style.justify_content_mods = bm;
                }
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
                style.justify_content_mods = AlignMods::default();
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
                style.align_items_mods = AlignMods::default();
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
