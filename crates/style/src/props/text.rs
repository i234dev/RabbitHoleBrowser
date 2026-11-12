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
                        "auto" | "from-font" => thickness = Some(None),
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
            } else if let Some(t) = super::px(c0, ctx) {
                style.text_decoration_thickness = Some(t);
            } else if let [Token::Percentage(p)] = c0 {
                style.text_decoration_thickness = Some(style.font_size * p / 100.0);
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
                    "preserve" | "break-spaces" => WhiteSpace::PreWrap,
                    "preserve-breaks" => WhiteSpace::PreLine,
                    _ => return true,
                };
            }
        }
        "hyphens" | "text-rendering" | "font-stretch" | "font-kerning" | "font-feature-settings" | "font-variation-settings"
        | "font-optical-sizing" | "font-synthesis" | "font-smoothing" | "text-size-adjust" | "text-underline-offset"
        | "text-underline-position" | "text-align-last" | "text-justify" | "unicode-bidi" | "text-emphasis"
        | "text-emphasis-style" | "text-emphasis-color" | "writing-mode" | "-webkit-writing-mode" | "text-orientation"
        | "text-combine-upright" | "font-language-override" | "font-variant-numeric" | "font-variant-ligatures"
        | "font-variant-east-asian" | "font-variant-position" | "line-break" | "word-spacing-x" | "ruby-position"
        | "ruby-align" | "text-spacing" | "hanging-punctuation" | "initial-letter" | "text-autospace" | "text-wrap-style"
        | "-webkit-text-fill-color" | "-webkit-text-stroke" | "-webkit-text-stroke-width" | "-webkit-text-stroke-color"
        | "-webkit-tap-highlight-color" | "-webkit-touch-callout" | "text-decoration-skip-ink" | "text-decoration-skip" => {}
        _ => return false,
    }
    let _ = (value, parent);
    true
}
