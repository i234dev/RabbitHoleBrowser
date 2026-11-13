//! Text and font properties beyond the core set.

use super::ApplyCtx;
use crate::computed::*;
use crate::values::{self, ColorValue};
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
            } else if let [Token::String(_)] = c0 {
                style.text_overflow = TextOverflow::Ellipsis;
            }
        }
        "word-break" => {
            if let Some(kw) = values::ident(c0) {
                style.word_break = match kw.as_str() {
                    "normal" => WordBreak::Normal,
                    "break-all" => WordBreak::BreakAll,
                    "keep-all" => WordBreak::KeepAll,
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
        "-webkit-line-clamp" | "line-clamp" => {
            if values::is_ident(c0, "none") {
                style.line_clamp = None;
            } else if let Some(n) = values::number(c0) {
                style.line_clamp = if n >= 1.0 { Some(n as u32) } else { None };
                style.line_clamp_prefixed = name == "-webkit-line-clamp";
            }
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
        "text-wrap-style" => match values::ident(c0).as_deref() {
            Some("balance") => style.text_wrap_balance = true,
            Some("auto" | "stable" | "pretty") => style.text_wrap_balance = false,
            _ => return true,
        },
        "font-synthesis" | "font-synthesis-style" => {
            let kws: Vec<String> = comps.iter().filter_map(|c| values::ident(c)).collect();
            style.font_synthesis_style = if name == "font-synthesis" {
                kws.iter().any(|k| k == "style")
            } else {
                kws.iter().any(|k| k == "auto" || k == "oblique-only")
            };
        }
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
        "hyphens" | "text-rendering" | "font-stretch" | "font-kerning" | "font-feature-settings"
        | "font-optical-sizing" | "font-smoothing" | "text-size-adjust" | "text-align-last" | "text-justify" | "text-orientation"
        | "text-combine-upright" | "font-language-override" | "font-variant-numeric" | "font-variant-ligatures"
        | "font-variant-east-asian" | "font-variant-position" | "word-spacing-x"
        | "ruby-align" | "text-spacing" | "hanging-punctuation" | "initial-letter" | "text-autospace"
        | "-webkit-text-fill-color" | "-webkit-text-stroke" | "-webkit-text-stroke-width" | "-webkit-text-stroke-color"
        | "-webkit-tap-highlight-color" | "-webkit-touch-callout" | "text-decoration-skip-ink" | "text-decoration-skip" => {}
        _ => return false,
    }
    let _ = (value, parent);
    true
}
