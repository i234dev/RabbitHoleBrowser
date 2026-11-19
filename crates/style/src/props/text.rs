//! Text and font properties beyond the core set.

use super::ApplyCtx;
use crate::computed::*;
use crate::values::{self, ColorValue};
use std::rc::Rc;
use rabbithole_css::Token;

/// Family names (as written, generic keywords excluded) from a
/// comma-separated `font-family` component list.
pub fn family_names(comps: &[&[Token]]) -> Vec<String> {
    let mut out = Vec::new();
    for fam in values::split_commas(comps) {
        let mut name = String::new();
        for c in fam {
            match c {
                [Token::Ident(s)] | [Token::String(s)] => {
                    if !name.is_empty() {
                        name.push(' ');
                    }
                    name.push_str(s);
                }
                _ => {}
            }
        }
        let n = name.trim().to_string();
        if n.is_empty() {
            continue;
        }
        let lower = n.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "serif" | "sans-serif" | "monospace" | "cursive" | "fantasy" | "system-ui" | "ui-serif" | "ui-sans-serif"
                | "ui-monospace" | "ui-rounded" | "emoji" | "math" | "fangsong" | "inherit" | "initial" | "unset"
        ) {
            continue;
        }
        out.push(n);
    }
    out
}

fn decoration_style(kw: &str) -> Option<TextDecorationStyle> {
    Some(match kw {
        "solid" => TextDecorationStyle::Solid,
        "double" => TextDecorationStyle::Double,
        "dotted" => TextDecorationStyle::Dotted,
        "dashed" => TextDecorationStyle::Dashed,
        "wavy" => TextDecorationStyle::Wavy,
        _ => return None,
    })
}

fn emphasis_keyword(k: &str) -> bool {
    matches!(k, "none" | "filled" | "open" | "dot" | "circle" | "double-circle" | "triangle" | "sesame")
}

/// `text-emphasis-style`: none | [filled | open] || [dot | circle |
/// double-circle | triangle | sesame] | <string>.
fn emphasis_style(comps: &[&[Token]]) -> Option<EmphasisStyle> {
    if let [[Token::String(s)]] = comps {
        return Some(EmphasisStyle::Custom(s.to_string()));
    }
    let (mut open, mut shape): (Option<bool>, Option<u8>) = (None, None);
    for c in comps {
        match values::ident(c).as_deref() {
            Some("none") if comps.len() == 1 => return Some(EmphasisStyle::None),
            Some("filled") if open.is_none() => open = Some(false),
            Some("open") if open.is_none() => open = Some(true),
            Some(k) if shape.is_none() => {
                shape = Some(match k {
                    "dot" => 1,
                    "circle" => 2,
                    "double-circle" => 3,
                    "triangle" => 4,
                    "sesame" => 5,
                    _ => return None,
                })
            }
            _ => return None,
        }
    }
    if open.is_none() && shape.is_none() {
        return None;
    }
    Some(EmphasisStyle::Shape { open: open.unwrap_or(false), shape: shape.unwrap_or(0) })
}

pub fn apply(name: &str, comps: &[&[Token]], value: &[Token], style: &mut ComputedStyle, parent: &ComputedStyle, ctx: &ApplyCtx) -> bool {
    let name = match name {
        "word-wrap" => "overflow-wrap",
        "-webkit-text-decoration" => "text-decoration",
        "-webkit-text-decoration-color" => "text-decoration-color",
        "-webkit-text-decoration-style" => "text-decoration-style",
        "-webkit-text-decoration-line" => "text-decoration-line",
        "-webkit-font-smoothing" | "-moz-osx-font-smoothing" => "font-smoothing",
        "-webkit-text-size-adjust" | "-ms-text-size-adjust" | "-moz-text-size-adjust" => "text-size-adjust",
        "-webkit-hyphens" | "-ms-hyphens" => "hyphens",
        "-webkit-font-feature-settings" => "font-feature-settings",
        "-webkit-text-emphasis" => "text-emphasis",
        "-webkit-text-emphasis-style" => "text-emphasis-style",
        "-webkit-text-emphasis-color" => "text-emphasis-color",
        "-webkit-text-emphasis-position" => "text-emphasis-position",
        "-webkit-ruby-position" => "ruby-position",
        n => n,
    };
    let c0 = comps[0];
    match name {
        "text-overflow" => {
            if let Some(kw) = values::ident(c0) {
                style.text_overflow = match kw.as_str() {
                    "clip" => TextOverflow::Clip,
                    "ellipsis" => TextOverflow::Ellipsis,
                    _ => return true,
                };
            } else if let [Token::String(s)] = c0 {
                style.text_overflow = TextOverflow::String(s.to_string());
            }
        }
        "word-break" => {
            if let Some(kw) = values::ident(c0) {
                style.word_break = match kw.as_str() {
                    "normal" => WordBreak::Normal,
                    "break-all" => WordBreak::BreakAll,
                    "keep-all" => WordBreak::KeepAll,
                    "auto-phrase" => WordBreak::AutoPhrase,
                    "break-word" => WordBreak::BreakWord,
                    _ => return true,
                };
            }
        }
        "overflow-wrap" => {
            if let Some(kw) = values::ident(c0) {
                style.overflow_wrap = match kw.as_str() {
                    "normal" => OverflowWrap::Normal,
                    "anywhere" => OverflowWrap::Anywhere,
                    "break-word" => OverflowWrap::BreakWord,
                    _ => return true,
                };
            }
        }
        "text-decoration" => {
            // lines || style || color || thickness
            let mut dec = TextDecoration::default();
            let mut any_line = false;
            let mut dstyle = None;
            let mut dcolor: Option<Option<rabbithole_gfx::Color>> = None;
            let mut thickness: Option<Option<f32>> = None;
            let mut from_font = false;
            for c in comps {
                if let Some(kw) = values::ident(c) {
                    match kw.as_str() {
                        "underline" => {
                            dec.underline = true;
                            any_line = true;
                        }
                        "overline" => {
                            dec.overline = true;
                            any_line = true;
                        }
                        "line-through" => {
                            dec.line_through = true;
                            any_line = true;
                        }
                        "none" | "blink" => any_line = true,
                        "auto" => thickness = Some(None),
                        "from-font" => {
                            thickness = Some(None);
                            from_font = true;
                        }
                        "currentcolor" => dcolor = Some(None),
                        k => {
                            if let Some(s) = decoration_style(k) {
                                dstyle = Some(s);
                            } else if let Some(ColorValue::Color(col)) = values::parse_color_component(c) {
                                dcolor = Some(Some(col));
                            } else {
                                return true;
                            }
                        }
                    }
                } else if let Some(ColorValue::Color(col)) = values::parse_color_component(c) {
                    dcolor = Some(Some(col));
                } else if let Some(t) = super::px(c, ctx) {
                    thickness = Some(Some(t));
                } else if let [Token::Percentage(p)] = c {
                    thickness = Some(Some(style.font_size * p / 100.0));
                } else {
                    return true;
                }
            }
            if any_line || dstyle.is_some() || dcolor.is_some() || thickness.is_some() {
                style.text_decoration = dec;
                style.text_decoration_style = dstyle.unwrap_or(TextDecorationStyle::Solid);
                style.text_decoration_color = dcolor.unwrap_or(None);
                style.text_decoration_thickness = thickness.unwrap_or(None);
                style.text_decoration_thickness_from_font = from_font;
            }
        }
        "text-decoration-line" => {
            let mut dec = TextDecoration::default();
            for c in comps {
                match values::ident(c).as_deref() {
                    Some("underline") => dec.underline = true,
                    Some("overline") => dec.overline = true,
                    Some("line-through") => dec.line_through = true,
                    Some("none") | Some("blink") => {}
                    _ => return true,
                }
            }
            style.text_decoration = dec;
        }
        "text-decoration-color" => {
            if values::is_ident(c0, "currentcolor") {
                style.text_decoration_color = None;
            } else if let Some(ColorValue::Color(col)) = values::parse_color_component(c0) {
                style.text_decoration_color = Some(col);
            }
        }
        "text-decoration-style" => {
            if let Some(s) = values::ident(c0).and_then(|k| decoration_style(&k)) {
                style.text_decoration_style = s;
            }
        }
        "text-decoration-thickness" => {
            if matches!(values::ident(c0).as_deref(), Some("auto" | "from-font")) {
                style.text_decoration_thickness = None;
                style.text_decoration_thickness_from_font = values::is_ident(c0, "from-font");
            } else if let Some(t) = super::px(c0, ctx) {
                style.text_decoration_thickness = Some(t);
                style.text_decoration_thickness_from_font = false;
            } else if let [Token::Percentage(p)] = c0 {
                style.text_decoration_thickness = Some(style.font_size * p / 100.0);
                style.text_decoration_thickness_from_font = false;
            } else if let Some(l) = super::len_no_auto(c0, ctx) {
                style.text_decoration_thickness = Some(l.resolve_or_zero(style.font_size));
                style.text_decoration_thickness_from_font = false;
            }
        }
        "text-underline-offset" => {
            if values::is_ident(c0, "auto") {
                style.text_underline_offset = UnderlineOffset::Auto;
            } else if let Some(t) = super::px(c0, ctx) {
                style.text_underline_offset = UnderlineOffset::Px(t);
            } else if let [Token::Percentage(p)] = c0 {
                style.text_underline_offset = UnderlineOffset::Percent(*p);
            } else if let Some(l) = super::len_no_auto(c0, ctx) {
                // calc() mixing lengths and percentages: resolve against
                // this element's font size.
                style.text_underline_offset = UnderlineOffset::Px(l.resolve_or_zero(style.font_size));
            }
        }
        "text-underline-position" => {
            let mut pos = UnderlinePosition::default();
            for c in comps {
                match values::ident(c).as_deref() {
                    Some("auto") if comps.len() == 1 => {}
                    Some("from-font") if !pos.under => pos.from_font = true,
                    Some("under") if !pos.from_font && !pos.under => pos.under = true,
                    Some("left") if !pos.left && !pos.right => pos.left = true,
                    Some("right") if !pos.left && !pos.right => pos.right = true,
                    _ => return true,
                }
            }
            style.text_underline_position = pos;
        }
        "text-decoration-inset" => {
            if values::is_ident(c0, "auto") && comps.len() == 1 {
                // A UA-chosen trim at each end.
                let t = Length::Px(style.font_size / 8.0);
                style.text_decoration_inset = (t, t);
            } else if comps.len() <= 2 {
                let Some(a) = super::len_no_auto(c0, ctx) else { return true };
                let b = match comps.get(1) {
                    Some(c) => match super::len_no_auto(c, ctx) {
                        Some(b) => b,
                        None => return true,
                    },
                    None => a,
                };
                style.text_decoration_inset = (a, b);
            }
        }
        "writing-mode" | "-webkit-writing-mode" => {
            if let Some(kw) = values::ident(c0) {
                style.writing_mode = match kw.as_str() {
                    "horizontal-tb" | "lr" | "lr-tb" | "rl" | "rl-tb" => WritingMode::HorizontalTb,
                    "vertical-rl" | "sideways-rl" | "tb" | "tb-rl" => WritingMode::VerticalRl,
                    "vertical-lr" | "tb-lr" => WritingMode::VerticalLr,
                    "sideways-lr" => WritingMode::SidewaysLr,
                    _ => return true,
                };
            }
        }
        "unicode-bidi" => {
            if let Some(kw) = values::ident(c0) {
                style.unicode_bidi = match kw.as_str() {
                    "normal" => UnicodeBidi::Normal,
                    "embed" => UnicodeBidi::Embed,
                    "isolate" | "-webkit-isolate" => UnicodeBidi::Isolate,
                    "bidi-override" => UnicodeBidi::BidiOverride,
                    "isolate-override" | "-webkit-isolate-override" => UnicodeBidi::IsolateOverride,
                    "plaintext" | "-webkit-plaintext" => UnicodeBidi::Plaintext,
                    _ => return true,
                };
            }
        }
        "direction" => {
            if let Some(kw) = values::ident(c0) {
                style.direction = match kw.as_str() {
                    "ltr" => Direction::Ltr,
                    "rtl" => Direction::Rtl,
                    _ => return true,
                };
            }
        }
        "tab-size" => {
            if let Some(n) = values::number(c0) {
                style.tab_size = n.max(0.0);
            } else if let Some(px) = super::px(c0, ctx) {
                // length: approximate in spaces (0.5em per space)
                style.tab_size = (px / (style.font_size * 0.5).max(1.0)).max(0.0);
            }
        }
        "-webkit-line-clamp" => {
            if values::is_ident(c0, "none") {
                style.line_clamp = None;
                style.line_clamp_auto = false;
            } else if let Some(n) = values::number(c0).filter(|n| comps.len() == 1 && n.fract() == 0.0 && *n >= 1.0) {
                style.line_clamp = Some(n as u32);
                style.line_clamp_prefixed = true;
                style.line_clamp_auto = false;
                style.block_ellipsis = Some(Rc::from("\u{2026}"));
            }
        }
        // `line-clamp: none | auto | [<integer [1,∞]> || <'block-ellipsis'>]`
        // (CSS Overflow 4 §4.1; `auto` clamps by height).
        "line-clamp" => {
            if comps.len() == 1 && values::is_ident(c0, "none") {
                style.line_clamp = None;
                style.line_clamp_auto = false;
                style.block_ellipsis = Some(Rc::from("\u{2026}"));
                return true;
            }
            let mut count: Option<u32> = None;
            let mut auto = false;
            let mut ellipsis: Option<Option<Rc<str>>> = None;
            for c in comps {
                if let Some(n) = values::number(c).filter(|n| n.fract() == 0.0) {
                    if count.is_some() || n < 1.0 {
                        return true;
                    }
                    count = Some(n as u32);
                } else if let [Token::String(s)] = c {
                    if ellipsis.is_some() {
                        return true;
                    }
                    ellipsis = Some(Some(Rc::from(s.as_str())));
                } else if values::is_ident(c, "no-ellipsis") {
                    if ellipsis.is_some() {
                        return true;
                    }
                    ellipsis = Some(None);
                } else if values::is_ident(c, "auto") {
                    if !auto && count.is_none() && ellipsis.is_none() {
                        auto = true;
                    } else if ellipsis.is_none() {
                        ellipsis = Some(Some(Rc::from("\u{2026}")));
                    } else {
                        return true;
                    }
                } else {
                    return true;
                }
            }
            // `<integer> auto`: the `auto` read first was the ellipsis.
            if auto && count.is_some() {
                if ellipsis.is_some() {
                    return true;
                }
                auto = false;
                ellipsis = Some(Some(Rc::from("\u{2026}")));
            }
            if count.is_none() && !auto {
                // A block ellipsis alone: `line-clamp: "…"` has no clamp.
                return true;
            }
            style.line_clamp = Some(count.unwrap_or(u32::MAX));
            style.line_clamp_auto = auto;
            style.line_clamp_prefixed = false;
            style.block_ellipsis = ellipsis.unwrap_or_else(|| Some(Rc::from("\u{2026}")));
        }
        "font-family" => {
            style.font_family_list = family_names(comps);
            if let Some(f) = crate::cascade::family_of(comps) {
                style.font_family = f;
            }
        }
        "font-variant" | "font-variant-caps" => {
            let kws: Vec<String> = comps.iter().filter_map(|c| values::ident(c)).collect();
            if kws.iter().any(|k| k == "small-caps" || k == "all-small-caps") {
                style.font_variant_small_caps = true;
            } else if kws.iter().any(|k| k == "normal" || k == "none") {
                style.font_variant_small_caps = false;
            }
            if name == "font-variant" {
                apply_font_variant(comps, style);
            }
        }
        "font-feature-settings" | "font-kerning" | "font-variant-ligatures" | "font-variant-position" | "font-variant-numeric"
        | "font-variant-east-asian" | "font-variant-alternates" | "font-language-override" => {
            let mut f = style.font_features.as_deref().cloned().unwrap_or_default();
            let ok = match name {
                "font-language-override" => match c0 {
                    _ if comps.len() == 1 && values::is_ident(c0, "normal") => {
                        f.language = None;
                        true
                    }
                    [Token::String(t)] if comps.len() == 1 && (1..=4).contains(&t.len()) && t.is_ascii() => {
                        let mut tag = [b' '; 4];
                        tag[..t.len()].copy_from_slice(t.as_bytes());
                        f.language = Some(tag);
                        true
                    }
                    _ => false,
                },
                "font-feature-settings" => parse_feature_settings(comps).map(|v| f.settings = v).is_some(),
                "font-kerning" => match values::ident(c0).as_deref() {
                    Some("auto") if comps.len() == 1 => {
                        f.kerning = None;
                        true
                    }
                    Some(k @ ("normal" | "none")) if comps.len() == 1 => {
                        f.kerning = Some(k == "normal");
                        true
                    }
                    _ => false,
                },
                "font-variant-alternates" => match parse_alternates(comps) {
                    Some((fixed, named)) => {
                        f.alternates = fixed;
                        f.alternates_named = named;
                        f.alternates_resolved.clear();
                        true
                    }
                    None => false,
                },
                _ => {
                    let kws: Option<Vec<String>> = comps.iter().map(|c| values::ident(c)).collect();
                    match kws.and_then(|k| variant_features(name, &k)) {
                        Some(v) => {
                            match name {
                                "font-variant-ligatures" => f.ligatures = v,
                                "font-variant-position" => f.position = v,
                                "font-variant-numeric" => f.numeric = v,
                                "font-variant-east-asian" => f.east_asian = v,
                                _ => f.alternates = v,
                            }
                            true
                        }
                        None => false,
                    }
                }
            };
            if ok {
                style.font_features = if f == FontFeatureProps::default() { None } else { Some(Rc::new(f)) };
            }
        }
        "font-variation-settings" => {
            if values::is_ident(c0, "normal") && comps.len() == 1 {
                style.font_variation_settings.clear();
                return true;
            }
            let mut out = Vec::new();
            for part in values::split_commas(comps) {
                let [s, v] = part.as_slice() else { return true };
                let [Token::String(tag)] = *s else { return true };
                if tag.len() != 4 || !tag.is_ascii() {
                    return true;
                }
                let Some(n) = values::number(v) else { return true };
                let b = tag.as_bytes();
                out.push(([b[0], b[1], b[2], b[3]], n));
            }
            style.font_variation_settings = out;
        }
        "text-emphasis-style" => match emphasis_style(comps) {
            Some(s) => style.text_emphasis_style = s,
            None => return true,
        },
        "text-emphasis-color" => {
            if values::is_ident(c0, "currentcolor") {
                style.text_emphasis_color = None;
            } else if let Some(ColorValue::Color(col)) = values::parse_color_component(c0) {
                style.text_emphasis_color = Some(col);
            }
        }
        "text-emphasis" => {
            // style || color
            let mut color: Option<Option<rabbithole_gfx::Color>> = None;
            let mut rest: Vec<&[Token]> = Vec::new();
            for c in comps {
                if values::is_ident(c, "currentcolor") {
                    color = Some(None);
                } else if let (None, Some(ColorValue::Color(col))) = (values::ident(c).filter(|k| emphasis_keyword(k)), values::parse_color_component(c)) {
                    color = Some(Some(col));
                } else {
                    rest.push(c);
                }
            }
            let st = if rest.is_empty() { Some(EmphasisStyle::None) } else { emphasis_style(&rest) };
            match st {
                Some(s) => {
                    style.text_emphasis_style = s;
                    style.text_emphasis_color = color.unwrap_or(None);
                }
                None => return true,
            }
        }
        "text-emphasis-position" => {
            let (mut over, mut side) = (None, None);
            for c in comps {
                match values::ident(c).as_deref() {
                    Some("over") if over.is_none() => over = Some(true),
                    Some("under") if over.is_none() => over = Some(false),
                    Some("right") if side.is_none() => side = Some(true),
                    Some("left") if side.is_none() => side = Some(false),
                    Some("auto") if comps.len() == 1 => {
                        over = Some(true);
                        side = Some(true);
                    }
                    _ => return true,
                }
            }
            let Some(o) = over else { return true };
            style.text_emphasis_over = o;
            style.text_emphasis_right = side.unwrap_or(true);
        }
        "wrap-inside" => match values::ident(c0).as_deref() {
            Some("auto") => style.wrap_inside_avoid = false,
            Some("avoid") => style.wrap_inside_avoid = true,
            _ => return true,
        },
        "text-autospace" => {
            let kws: Vec<String> = comps.iter().filter_map(|c| values::ident(c)).collect();
            if kws.len() != comps.len() {
                return true;
            }
            style.text_autospace = !kws.iter().any(|k| k == "no-autospace");
        }
        "text-wrap-style" => match values::ident(c0).as_deref() {
            Some("balance") => style.text_wrap_balance = true,
            Some("auto" | "stable" | "pretty") => style.text_wrap_balance = false,
            _ => return true,
        },
        "font-synthesis" => {
            let kws: Vec<String> = comps.iter().filter_map(|c| values::ident(c)).collect();
            style.font_synthesis_style = kws.iter().any(|k| k == "style");
        }
        "font-synthesis-style" => match comps {
            [c] => match values::ident(c).as_deref() {
                Some("auto") => style.font_synthesis_style = true,
                Some("none") => style.font_synthesis_style = false,
                _ => return true,
            },
            _ => return true,
        },
        "line-break" | "-webkit-line-break" => {
            style.line_break = match values::ident(c0).as_deref() {
                Some("auto" | "normal") => 0,
                Some("loose") => 1,
                Some("strict") => 2,
                Some("anywhere") => 3,
                _ => return true,
            }
        }
        "ruby-position" => match values::ident(c0).as_deref() {
            Some("over") | Some("alternate") | Some("inter-character") => style.ruby_under = false,
            Some("under") => style.ruby_under = true,
            _ => return true,
        },
        "white-space-collapse" => {
            if let Some(kw) = values::ident(c0) {
                style.white_space = match kw.as_str() {
                    "collapse" => {
                        if style.white_space == WhiteSpace::Nowrap {
                            WhiteSpace::Nowrap
                        } else {
                            WhiteSpace::Normal
                        }
                    }
                    "preserve" => WhiteSpace::PreWrap,
                    "break-spaces" => WhiteSpace::BreakSpaces,
                    "preserve-breaks" => WhiteSpace::PreLine,
                    _ => return true,
                };
            }
        }
        "hyphens" | "text-rendering" | "font-stretch"
        | "font-optical-sizing" | "font-smoothing" | "text-size-adjust" | "text-align-last" | "text-justify" | "text-orientation"
        | "text-combine-upright" | "word-spacing-x"
        | "ruby-align" | "text-spacing" | "hanging-punctuation" | "initial-letter"
        | "-webkit-text-fill-color" | "-webkit-text-stroke" | "-webkit-text-stroke-width" | "-webkit-text-stroke-color"
        | "-webkit-tap-highlight-color" | "-webkit-touch-callout" | "text-decoration-skip-ink" | "text-decoration-skip" => {}
        _ => return false,
    }
    let _ = (value, parent);
    true
}

/// `font-feature-settings: normal | [<string> [<integer> | on | off]?]#`.
pub(crate) fn parse_feature_settings(comps: &[&[Token]]) -> Option<Vec<rabbithole_gfx::FontFeature>> {
    if comps.len() == 1 && values::is_ident(comps[0], "normal") {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    for item in values::split_commas(comps) {
        let (tag, value) = match item.as_slice() {
            [t] => (t, 1),
            [t, v] => (t, match v {
                [Token::Number(n)] if *n >= 0.0 && n.fract() == 0.0 => *n as u32,
                _ if values::is_ident(v, "on") => 1,
                _ if values::is_ident(v, "off") => 0,
                _ => return None,
            }),
            _ => return None,
        };
        let [Token::String(s)] = tag else { return None };
        let b = s.as_bytes();
        if b.len() != 4 || !b.iter().all(|c| (0x20..=0x7e).contains(c)) {
            return None;
        }
        out.retain(|f: &rabbithole_gfx::FontFeature| f.0 != [b[0], b[1], b[2], b[3]]);
        out.push(([b[0], b[1], b[2], b[3]], value));
    }
    Some(out)
}

/// The OpenType features of a `font-variant-*` longhand's keywords (None:
/// invalid).
fn variant_features(name: &str, kws: &[String]) -> Option<Vec<rabbithole_gfx::FontFeature>> {
    if kws.len() == 1 && kws[0] == "normal" {
        return Some(Vec::new());
    }
    let mut out: Vec<rabbithole_gfx::FontFeature> = Vec::new();
    let mut groups: Vec<u8> = Vec::new();
    let mut add = |group: u8, feats: &[(&[u8; 4], u32)]| -> bool {
        if groups.contains(&group) {
            return false;
        }
        groups.push(group);
        out.extend(feats.iter().map(|(t, v)| (**t, *v)));
        true
    };
    for k in kws {
        let ok = match (name, k.as_str()) {
            ("font-variant-ligatures", "none") if kws.len() == 1 => add(0, &[(b"liga", 0), (b"clig", 0), (b"dlig", 0), (b"hlig", 0), (b"calt", 0)]),
            ("font-variant-ligatures", "common-ligatures") => add(1, &[(b"liga", 1), (b"clig", 1)]),
            ("font-variant-ligatures", "no-common-ligatures") => add(1, &[(b"liga", 0), (b"clig", 0)]),
            ("font-variant-ligatures", "discretionary-ligatures") => add(2, &[(b"dlig", 1)]),
            ("font-variant-ligatures", "no-discretionary-ligatures") => add(2, &[(b"dlig", 0)]),
            ("font-variant-ligatures", "historical-ligatures") => add(3, &[(b"hlig", 1)]),
            ("font-variant-ligatures", "no-historical-ligatures") => add(3, &[(b"hlig", 0)]),
            ("font-variant-ligatures", "contextual") => add(4, &[(b"calt", 1)]),
            ("font-variant-ligatures", "no-contextual") => add(4, &[(b"calt", 0)]),
            ("font-variant-position", "sub") if kws.len() == 1 => add(0, &[(b"subs", 1)]),
            ("font-variant-position", "super") if kws.len() == 1 => add(0, &[(b"sups", 1)]),
            ("font-variant-numeric", "lining-nums") => add(1, &[(b"lnum", 1)]),
            ("font-variant-numeric", "oldstyle-nums") => add(1, &[(b"onum", 1)]),
            ("font-variant-numeric", "proportional-nums") => add(2, &[(b"pnum", 1)]),
            ("font-variant-numeric", "tabular-nums") => add(2, &[(b"tnum", 1)]),
            ("font-variant-numeric", "diagonal-fractions") => add(3, &[(b"frac", 1)]),
            ("font-variant-numeric", "stacked-fractions") => add(3, &[(b"afrc", 1)]),
            ("font-variant-numeric", "ordinal") => add(4, &[(b"ordn", 1)]),
            ("font-variant-numeric", "slashed-zero") => add(5, &[(b"zero", 1)]),
            ("font-variant-east-asian", "jis78") => add(1, &[(b"jp78", 1)]),
            ("font-variant-east-asian", "jis83") => add(1, &[(b"jp83", 1)]),
            ("font-variant-east-asian", "jis90") => add(1, &[(b"jp90", 1)]),
            ("font-variant-east-asian", "jis04") => add(1, &[(b"jp04", 1)]),
            ("font-variant-east-asian", "simplified") => add(1, &[(b"smpl", 1)]),
            ("font-variant-east-asian", "traditional") => add(1, &[(b"trad", 1)]),
            ("font-variant-east-asian", "full-width") => add(2, &[(b"fwid", 1)]),
            ("font-variant-east-asian", "proportional-width") => add(2, &[(b"pwid", 1)]),
            ("font-variant-east-asian", "ruby") => add(3, &[(b"ruby", 1)]),
            ("font-variant-alternates", "historical-forms") => add(1, &[(b"hist", 1)]),
            _ => false,
        };
        if !ok {
            return None;
        }
    }
    Some(out)
}

/// The `font-variant` shorthand: resets and sets every longhand.
fn apply_font_variant(comps: &[&[Token]], style: &mut ComputedStyle) {
    let Some(kws) = comps.iter().map(|c| values::ident(c)).collect::<Option<Vec<String>>>() else { return };
    let mut f = FontFeatureProps { kerning: style.font_features.as_ref().and_then(|f| f.kerning), language: style.font_features.as_ref().and_then(|f| f.language), settings: style.font_features.as_ref().map(|f| f.settings.clone()).unwrap_or_default(), ..Default::default() };
    if !(kws.len() == 1 && kws[0] == "normal") {
        if kws.len() == 1 && kws[0] == "none" {
            f.ligatures = variant_features("font-variant-ligatures", &kws).unwrap_or_default();
        } else {
            for (longhand, slot) in [
                ("font-variant-ligatures", 0),
                ("font-variant-position", 1),
                ("font-variant-numeric", 2),
                ("font-variant-east-asian", 3),
                ("font-variant-alternates", 4),
            ] {
                let mine: Vec<String> = kws.iter().filter(|k| variant_features(longhand, std::slice::from_ref(k)).is_some() && k.as_str() != "normal").cloned().collect();
                if mine.is_empty() {
                    continue;
                }
                let Some(v) = variant_features(longhand, &mine) else { return };
                match slot {
                    0 => f.ligatures = v,
                    1 => f.position = v,
                    2 => f.numeric = v,
                    3 => f.east_asian = v,
                    _ => f.alternates = v,
                }
            }
        }
    }
    style.font_features = if f == FontFeatureProps::default() { None } else { Some(Rc::new(f)) };
}

/// `font-variant-alternates: normal | [ stylistic(<name>) || historical-forms
/// || styleset(<name>#) || character-variant(<name>#) || swash(<name>) ||
/// ornaments(<name>) || annotation(<name>) ]`.
fn parse_alternates(comps: &[&[Token]]) -> Option<(Vec<rabbithole_gfx::FontFeature>, Vec<(String, Vec<String>)>)> {
    let mut fixed = Vec::new();
    let mut named: Vec<(String, Vec<String>)> = Vec::new();
    if comps.len() == 1 && values::is_ident(comps[0], "normal") {
        return Some((fixed, named));
    }
    for c in comps {
        if values::is_ident(c, "historical-forms") {
            if !fixed.is_empty() {
                return None;
            }
            fixed.push((*b"hist", 1));
            continue;
        }
        let f = super::fn_name(c)?;
        if !matches!(f.as_str(), "stylistic" | "styleset" | "character-variant" | "swash" | "ornaments" | "annotation") || named.iter().any(|(k, _)| *k == f) {
            return None;
        }
        let mut names = Vec::new();
        for arg in super::fn_args(c) {
            match arg.as_slice() {
                [[Token::Ident(n)]] => names.push(n.clone()),
                _ => return None,
            }
        }
        let multi = matches!(f.as_str(), "styleset" | "character-variant");
        if names.is_empty() || (!multi && names.len() > 1) {
            return None;
        }
        named.push((f, names));
    }
    Some((fixed, named))
}

thread_local! {
    /// The document's `@font-feature-values`: lower-cased family →
    /// (block kind, value name, numbers).
    static FEATURE_VALUES: std::cell::RefCell<Rc<std::collections::HashMap<String, Vec<(String, String, Vec<u32>)>>>> = std::cell::RefCell::new(Rc::new(std::collections::HashMap::new()));
}

/// Install the `@font-feature-values` of the document being styled.
pub fn set_feature_values(values: Rc<std::collections::HashMap<String, Vec<(String, String, Vec<u32>)>>>) {
    FEATURE_VALUES.with(|v| *v.borrow_mut() = values);
}

/// Resolve named alternates against the element's font families (the
/// first family defining the name wins).
pub(crate) fn resolve_alternates(style: &mut ComputedStyle) {
    let Some(f) = style.font_features.as_ref() else { return };
    if f.alternates_named.is_empty() && f.alternates_resolved.is_empty() {
        return;
    }
    let out = FEATURE_VALUES.with(|fv| {
        let fv = fv.borrow();
        let mut out: Vec<rabbithole_gfx::FontFeature> = Vec::new();
        for (kind, names) in &f.alternates_named {
            let block = match kind.as_str() {
                "character-variant" => "character-variant",
                k => k,
            };
            for name in names {
                let nums = style.font_family_list.iter().find_map(|fam| {
                    fv.get(&fam.to_ascii_lowercase())?.iter().rev().find(|(k, n, _)| k == block && n == name).map(|(_, _, v)| v.clone())
                });
                let Some(nums) = nums else { continue };
                let tag2 = |p: &[u8; 2], n: u32| -> Option<[u8; 4]> {
                    (1..=99).contains(&n).then(|| [p[0], p[1], b'0' + (n / 10) as u8, b'0' + (n % 10) as u8])
                };
                match kind.as_str() {
                    "stylistic" if nums.len() == 1 => out.push((*b"salt", nums[0])),
                    "styleset" => {
                        for n in nums.iter().filter(|n| **n <= 20) {
                            if let Some(t) = tag2(b"ss", *n) {
                                out.push((t, 1));
                            }
                        }
                    }
                    "character-variant" if nums.len() <= 2 => {
                        if let Some(t) = tag2(b"cv", nums[0]) {
                            out.push((t, nums.get(1).copied().unwrap_or(1)));
                        }
                    }
                    "swash" if nums.len() == 1 => {
                        out.push((*b"swsh", nums[0]));
                        out.push((*b"cswh", nums[0]));
                    }
                    "ornaments" if nums.len() == 1 => out.push((*b"ornm", nums[0])),
                    "annotation" if nums.len() == 1 => out.push((*b"nalt", nums[0])),
                    _ => {}
                }
            }
        }
        out
    });
    if out != f.alternates_resolved {
        let mut nf = (**f).clone();
        nf.alternates_resolved = out;
        style.font_features = Some(Rc::new(nf));
    }
}
