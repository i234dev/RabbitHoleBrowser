//! Interaction, animation, table and sizing properties, plus accepted
//! no-ops for properties that have no effect on this renderer.

use super::ApplyCtx;
use crate::computed::*;
use crate::values;
use rabbithole_css::Token;

const TIMING_KW: &[&str] = &["ease", "linear", "ease-in", "ease-out", "ease-in-out", "step-start", "step-end"];

fn timing_name(c: &[Token]) -> Option<String> {
    if let Some(kw) = values::ident(c) {
        if TIMING_KW.contains(&kw.as_str()) {
            return Some(kw);
        }
        return None;
    }
    match super::fn_name(c)?.as_str() {
        "cubic-bezier" => Some("cubic-bezier".into()),
        "steps" => Some("steps".into()),
        _ => None,
    }
}

fn parse_transitions(comps: &[&[Token]]) -> Vec<Transition> {
    if comps.len() == 1 && values::is_ident(comps[0], "none") {
        return Vec::new();
    }
    let mut out = Vec::new();
    for item in values::split_commas(comps) {
        let mut t = Transition { property: "all".into(), duration: 0.0, delay: 0.0, timing: "ease".into() };
        let mut times = 0;
        for c in item {
            if let Some(s) = super::seconds(c) {
                if times == 0 {
                    t.duration = s;
                } else {
                    t.delay = s;
                }
                times += 1;
            } else if let Some(tn) = timing_name(c) {
                t.timing = tn;
            } else if let Some(kw) = values::ident(c) {
                t.property = kw;
            }
        }
        out.push(t);
    }
    out
}

fn parse_animations(comps: &[&[Token]]) -> Vec<Animation> {
    if comps.len() == 1 && values::is_ident(comps[0], "none") {
        return Vec::new();
    }
    let mut out = Vec::new();
    for item in values::split_commas(comps) {
        let mut a = Animation {
            name: String::new(),
            duration: 0.0,
            delay: 0.0,
            timing: "ease".into(),
            iterations: Some(1.0),
            direction: "normal".into(),
            fill_mode: "none".into(),
        };
        let mut times = 0;
        for c in item {
            if let Some(s) = super::seconds(c) {
                if times == 0 {
                    a.duration = s;
                } else {
                    a.delay = s;
                }
                times += 1;
            } else if let Some(n) = values::number(c) {
                a.iterations = Some(n);
            } else if let Some(tn) = timing_name(c) {
                a.timing = tn;
            } else if let Some(kw) = values::ident(c) {
                match kw.as_str() {
                    "infinite" => a.iterations = None,
                    "normal" | "reverse" | "alternate" | "alternate-reverse" => a.direction = kw,
                    "none" | "forwards" | "backwards" | "both" => a.fill_mode = kw,
                    "running" | "paused" => {}
                    _ => a.name = kw,
                }
            } else if let [Token::String(s)] = c {
                a.name = s.clone();
            }
        }
        if !a.name.is_empty() || a.duration > 0.0 {
            out.push(a);
        }
    }
    out
}

/// Set a field on every transition entry from a comma-separated list,
/// creating entries as needed.
fn each_transition<T: Clone>(style: &mut ComputedStyle, vals: Vec<T>, f: impl Fn(&mut Transition, T)) {
    if vals.is_empty() {
        return;
    }
    if style.transitions.len() < vals.len() {
        style.transitions.resize(vals.len(), Transition { property: "all".into(), duration: 0.0, delay: 0.0, timing: "ease".into() });
    }
    let n = style.transitions.len();
    for i in 0..n {
        f(&mut style.transitions[i], vals[i % vals.len()].clone());
    }
}

fn each_animation<T: Clone>(style: &mut ComputedStyle, vals: Vec<T>, f: impl Fn(&mut Animation, T)) {
    if vals.is_empty() {
        return;
    }
    if style.animations.len() < vals.len() {
        style.animations.resize(
            vals.len(),
            Animation { name: String::new(), duration: 0.0, delay: 0.0, timing: "ease".into(), iterations: Some(1.0), direction: "normal".into(), fill_mode: "none".into() },
        );
    }
    let n = style.animations.len();
    for i in 0..n {
        f(&mut style.animations[i], vals[i % vals.len()].clone());
    }
}

fn single_list<'a, T>(comps: &[&'a [Token]], f: impl Fn(&'a [Token]) -> Option<T>) -> Option<Vec<T>> {
    values::split_commas(comps).iter().map(|l| if l.len() == 1 { f(l[0]) } else { None }).collect()
}

pub fn apply(name: &str, comps: &[&[Token]], style: &mut ComputedStyle, ctx: &ApplyCtx) -> bool {
    let name = match name {
        "-webkit-user-select" | "-moz-user-select" | "-ms-user-select" => "user-select",
        "-webkit-appearance" | "-moz-appearance" => "appearance",
        "-webkit-transition" | "-moz-transition" | "-o-transition" => "transition",
        "-webkit-transition-property" => "transition-property",
        "-webkit-transition-duration" => "transition-duration",
        "-webkit-transition-timing-function" => "transition-timing-function",
        "-webkit-transition-delay" => "transition-delay",
        "-webkit-animation" | "-moz-animation" => "animation",
        "-webkit-animation-name" => "animation-name",
        "-webkit-animation-duration" => "animation-duration",
        "-webkit-animation-timing-function" => "animation-timing-function",
        "-webkit-animation-delay" => "animation-delay",
        "-webkit-animation-iteration-count" => "animation-iteration-count",
        "-webkit-animation-direction" => "animation-direction",
        "-webkit-animation-fill-mode" => "animation-fill-mode",
        "-webkit-animation-play-state" => "animation-play-state",
        "-webkit-overflow-scrolling" => "overscroll-behavior",
        "-webkit-columns" | "-moz-columns" => "columns",
        "-webkit-column-count" | "-moz-column-count" => "column-count",
        n => n,
    };
    let c0 = comps[0];
    match name {
        "pointer-events" => {
            if let Some(kw) = values::ident(c0) {
                style.pointer_events = if kw == "none" { PointerEvents::None } else { PointerEvents::Auto };
            }
        }
        "user-select" => {
            if let Some(kw) = values::ident(c0) {
                style.user_select = match kw.as_str() {
                    "none" => UserSelect::None,
                    "text" => UserSelect::Text,
                    "all" => UserSelect::All,
                    _ => UserSelect::Auto,
                };
            }
        }
        "resize" => {
            if let Some(kw) = values::ident(c0) {
                style.resize = match kw.as_str() {
                    "both" => Resize::Both,
                    "horizontal" | "inline" => Resize::Horizontal,
                    "vertical" | "block" => Resize::Vertical,
                    _ => Resize::None,
                };
            }
        }
        "appearance" => {
            if let Some(kw) = values::ident(c0) {
                style.appearance = if kw == "none" { Appearance::None } else { Appearance::Auto };
            }
        }
        "transition" => style.transitions = parse_transitions(comps),
        "transition-property" => {
            if let Some(v) = single_list(comps, |c| values::ident(c)) {
                if v.len() == 1 && v[0] == "none" {
                    style.transitions.clear();
                } else {
                    each_transition(style, v, |t, p| t.property = p);
                }
            }
        }
        "transition-duration" => {
            if let Some(v) = single_list(comps, super::seconds) {
                each_transition(style, v, |t, d| t.duration = d);
            }
        }
        "transition-delay" => {
            if let Some(v) = single_list(comps, super::seconds) {
                each_transition(style, v, |t, d| t.delay = d);
            }
        }
        "transition-timing-function" => {
            if let Some(v) = single_list(comps, timing_name) {
                each_transition(style, v, |t, f| t.timing = f);
            }
        }
        "transition-behavior" => {}
        "animation" => style.animations = parse_animations(comps),
        "animation-name" => {
            let v = single_list(comps, |c| match c {
                [Token::Ident(i)] => Some(i.clone()),
                [Token::String(s)] => Some(s.clone()),
                _ => None,
            });
            if let Some(v) = v {
                if v.len() == 1 && v[0] == "none" {
                    style.animations.clear();
                } else {
                    each_animation(style, v, |a, n| a.name = n);
                }
            }
        }
        "animation-duration" => {
            if let Some(v) = single_list(comps, super::seconds) {
                each_animation(style, v, |a, d| a.duration = d);
            }
        }
        "animation-delay" => {
            if let Some(v) = single_list(comps, super::seconds) {
                each_animation(style, v, |a, d| a.delay = d);
            }
        }
        "animation-timing-function" => {
            if let Some(v) = single_list(comps, timing_name) {
                each_animation(style, v, |a, f| a.timing = f);
            }
        }
        "animation-iteration-count" => {
            let v = single_list(comps, |c| {
                if values::is_ident(c, "infinite") {
                    Some(None)
                } else {
                    values::number(c).map(Some)
                }
            });
            if let Some(v) = v {
                each_animation(style, v, |a, n| a.iterations = n);
            }
        }
        "animation-direction" => {
            if let Some(v) = single_list(comps, |c| values::ident(c)) {
                each_animation(style, v, |a, d| a.direction = d);
            }
        }
        "animation-fill-mode" => {
            if let Some(v) = single_list(comps, |c| values::ident(c)) {
                each_animation(style, v, |a, f| a.fill_mode = f);
            }
        }
        "animation-play-state" | "animation-composition" | "animation-timeline" => {}
        "scroll-margin-top" | "scroll-margin" => {
            if let Some(v) = super::px(c0, ctx) {
                style.scroll_margin_top = v;
            }
        }
        "border-collapse" => {
            if let Some(kw) = values::ident(c0) {
                style.border_collapse = kw == "collapse";
            }
        }
        "border-spacing" => {
            let vals: Vec<f32> = comps.iter().filter_map(|c| super::px(c, ctx)).collect();
            match vals.len() {
                1 => style.border_spacing = (vals[0], vals[0]),
                2 => style.border_spacing = (vals[0], vals[1]),
                _ => {}
            }
        }
        "object-fit" => {
            if let Some(kw) = values::ident(c0) {
                style.object_fit = match kw.as_str() {
                    "fill" => ObjectFit::Fill,
                    "contain" => ObjectFit::Contain,
                    "cover" => ObjectFit::Cover,
                    "none" => ObjectFit::None,
                    "scale-down" => ObjectFit::ScaleDown,
                    _ => return true,
                };
            }
        }
        "object-position" => {
            if let Some(p) = super::parse_position(comps, ctx) {
                style.object_position = p;
            }
        }
        "aspect-ratio" => {
            if values::is_ident(c0, "auto") {
                style.aspect_ratio = None;
                return true;
            }
            let nums: Vec<f32> = comps.iter().filter_map(|c| values::number(c)).collect();
            match nums.len() {
                1 if nums[0] > 0.0 => style.aspect_ratio = Some(nums[0]),
                2 if nums[1] > 0.0 => style.aspect_ratio = Some(nums[0] / nums[1]),
                _ => {}
            }
        }
        "inset-inline" | "inset-block" => {
            let vals: Vec<Length> = comps.iter().filter_map(|c| values::parse_length(c).map(|l| l.resolve(ctx))).collect();
            let (a, b) = match vals.len() {
                1 => (vals[0], vals[0]),
                2 => (vals[0], vals[1]),
                _ => return true,
            };
            if name == "inset-inline" {
                style.inset.left = a;
                style.inset.right = b;
            } else {
                style.inset.top = a;
                style.inset.bottom = b;
            }
        }
        "zoom" => {
            // Treat a zoom factor as a font-size multiplier (approximation).
            let f = match c0 {
                [Token::Number(n)] => Some(*n),
                [Token::Percentage(p)] => Some(p / 100.0),
                _ => None,
            };
            if let Some(f) = f {
                if f > 0.0 && f != 1.0 {
                    style.font_size *= f;
                }
            }
        }
        "table-layout" | "caption-side" | "empty-cells" | "list-style-image" | "overscroll-behavior" | "overscroll-behavior-x"
        | "overscroll-behavior-y" | "scroll-behavior" | "scroll-snap-type" | "scroll-snap-align" | "scroll-padding"
        | "scroll-padding-top" | "scroll-margin-bottom" | "scroll-margin-left" | "scroll-margin-right" | "scrollbar-width"
        | "scrollbar-color" | "scrollbar-gutter" | "columns" | "column-count" | "column-width" | "column-gap-x" | "column-rule"
        | "column-span" | "column-fill" | "break-inside" | "break-before" | "break-after" | "page-break-inside"
        | "page-break-before" | "page-break-after" | "box-decoration-break" | "caret-color" | "accent-color" | "color-scheme"
        | "image-rendering" | "image-orientation" | "print-color-adjust" | "-webkit-print-color-adjust" | "touch-action"
        | "-ms-touch-action" | "speak" | "orphans" | "widows" | "page" | "size" | "marks" | "quotes-x" | "all-x"
        | "content-visibility" | "contain-intrinsic-size" | "container" | "container-type" | "container-name" | "view-transition-name"
        | "anchor-name" | "position-anchor" | "field-sizing" | "forced-color-adjust" | "interpolate-size" | "math-style"
        | "-webkit-box-sizing" | "-moz-box-sizing" | "-webkit-user-drag" | "-webkit-user-modify" | "-webkit-locale"
        | "-webkit-margin-before" | "-webkit-margin-after" | "-webkit-margin-start" | "-webkit-margin-end"
        | "-webkit-padding-start" | "-webkit-padding-end" | "-webkit-border-image" | "border-image" | "border-image-source"
        | "border-image-slice" | "border-image-width" | "border-image-outset" | "border-image-repeat" | "cursor-x"
        | "nav-index" | "ime-mode" | "src" | "unicode-range" | "font-display" | "behavior" | "filter-x" | "-ms-filter"
        | "-ms-interpolation-mode" | "-ms-overflow-style" | "-ms-high-contrast-adjust" | "-moz-force-broken-image-icon" => {}
        _ => return false,
    }
    true
}
