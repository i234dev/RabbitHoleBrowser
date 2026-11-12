//! Native look of form controls (`appearance: auto`), modeled on Blink's
//! refreshed controls: checkboxes, radio buttons, sliders, progress bars,
//! meters, color wells, the drop-down arrow of menus, the calendar icon of
//! date fields and the resize grip of text areas.
//!
//! Checkboxes, radios, sliders, progress bars and meters ignore author
//! backgrounds and borders while they keep their native appearance (like
//! Blink); the other controls are painted by CSS (the UA sheet gives them
//! their default look) and only get their native decorations on top.

use crate::display_list::DisplayItem;
use crate::tree::LayoutBox;
use crate::LayoutContext;
use rabbithole_dom::{Document, NodeId};
use rabbithole_gfx::{Color, Paint, Rect};
use rabbithole_style::{Appearance, BorderStyle, TextDecorationStyle};

/// Blink's default accent color.
const ACCENT: Color = Color::rgb(0, 117, 255);
const BORDER: Color = Color::rgb(118, 118, 118);
const TRACK: Color = Color::rgb(239, 239, 239);
const TRACK_BORDER: Color = Color::rgb(178, 178, 178);

/// How a control is painted.
pub(crate) enum Widget {
    /// Instead of the CSS background and border.
    Replace(Vec<DisplayItem>),
    /// After the CSS background and border.
    Overlay(Vec<DisplayItem>),
}

fn input_type(doc: &Document, node: NodeId) -> String {
    crate::build::input_type(doc, node)
}

fn disabled(doc: &Document, node: NodeId) -> bool {
    if doc.attr(node, "disabled").is_some() {
        return true;
    }
    let mut cur = doc.parent(node);
    while let Some(p) = cur {
        if doc.is_element(p, "fieldset") && doc.attr(p, "disabled").is_some() {
            return true;
        }
        cur = doc.parent(p);
    }
    false
}

/// Pick white or black, whichever reads better on `bg`.
fn on_color(bg: Color) -> Color {
    let lum = |c: u8| {
        let c = c as f32 / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let l = 0.2126 * lum(bg.r) + 0.7152 * lum(bg.g) + 0.0722 * lum(bg.b);
    // Contrast with white is (1.05) / (l + 0.05); prefer white down to 3:1.
    if 1.05 / (l + 0.05) >= 3.0 {
        Color::WHITE
    } else {
        Color::BLACK
    }
}

/// Blend `c` towards white for disabled controls.
fn faded(c: Color) -> Color {
    let mix = |v: u8| (v as f32 * 0.45 + 255.0 * 0.55).round() as u8;
    Color::rgba(mix(c.r), mix(c.g), mix(c.b), c.a)
}

fn rect(r: Rect, color: Color, radius: f32) -> DisplayItem {
    DisplayItem::Rect { rect: r, color, radius }
}

fn ring(r: Rect, width: f32, color: Color, radius: f32) -> DisplayItem {
    DisplayItem::Border { rect: r, widths: [width; 4], colors: [color; 4], styles: [BorderStyle::Solid; 4], radii: [(radius, radius); 4] }
}

fn line(x0: f32, y0: f32, x1: f32, y1: f32, width: f32, color: Color) -> DisplayItem {
    DisplayItem::Line { x0, y0, x1, y1, width, color, style: TextDecorationStyle::Solid }
}

/// A square of side `s` centered in `r`.
fn centered_square(r: Rect, s: f32) -> Rect {
    Rect::new(r.x + (r.w - s) / 2.0, r.y + (r.h - s) / 2.0, s, s)
}

/// Does `b` show the native look of a text field, button or menu (no
/// author background or border declarations, `appearance` is `auto`)? Those get Blink's 2px rounded corners.
pub(crate) fn native_rounding(b: &LayoutBox, ctx: &LayoutContext) -> bool {
    let Some(node) = b.node else { return false };
    if b.pseudo.is_some() {
        return false;
    }
    let s = &b.style;
    if s.appearance == Appearance::None || s.author_widget_style || s.border_radius > 0.0 || s.border_width.top != 1.0 || s.border_width.left != 1.0 {
        return false;
    }
    let ua_border = [BORDER, Color::rgb(79, 79, 79), Color::rgb(141, 141, 141), Color::rgba(118, 118, 118, 77)];
    if !ua_border.contains(&s.border_color.top) || s.border_color.top != s.border_color.left || s.border_style.top != BorderStyle::Solid {
        return false;
    }
    let bg = s.background_color;
    let ua_bg = [Color::WHITE, TRACK, Color::rgb(229, 229, 229), Color::rgb(245, 245, 245), Color::rgba(239, 239, 239, 77)];
    if !ua_bg.contains(&bg) || s.background_image.is_some() || !s.background_layers.is_empty() {
        return false;
    }
    match ctx.doc.tag(node) {
        Some("button" | "select" | "textarea") => true,
        Some("input") => !matches!(input_type(ctx.doc, node).as_str(), "checkbox" | "radio" | "range" | "image" | "hidden"),
        _ => false,
    }
}

/// The native widget of `b`, if it is a form control with one.
pub(crate) fn widget(b: &LayoutBox, ctx: &LayoutContext) -> Option<Widget> {
    let node = b.node?;
    if b.pseudo.is_some() {
        return None;
    }
    let doc = ctx.doc;
    let tag = doc.tag(node)?;
    if !matches!(tag, "input" | "select" | "textarea" | "progress" | "meter") {
        return None;
    }
    let style = &b.style;
    let accent = style.accent_color.filter(|c| c.a > 0).map(|c| c.with_alpha(255)).unwrap_or(ACCENT);
    let off = disabled(doc, node);
    let r = b.rect;
    if r.w <= 0.0 || r.h <= 0.0 {
        return None;
    }
    let auto = style.appearance != Appearance::None;
    match tag {
        "input" => {
            let ty = input_type(doc, node);
            match ty.as_str() {
                "checkbox" if auto => Some(Widget::Replace(checkbox(r, doc, node, accent, off))),
                "radio" if auto => Some(Widget::Replace(radio(r, doc.is_checked(node), accent, off))),
                "range" if auto => Some(Widget::Overlay(range(b, doc, node, accent, off))),
                "range" => Some(Widget::Overlay(css_thumb(b, doc, node, ctx.styles.pseudo(node, rabbithole_css::PseudoElement::Marker).map(|s| &**s), accent))),
                "color" if auto => Some(Widget::Overlay(color_well(b, doc, node))),
                "date" | "datetime-local" | "month" | "week" | "time" if auto => Some(Widget::Overlay(calendar_icon(b, &ty, style.color))),
                _ => None,
            }
        }
        "select" if auto => {
            let listbox = doc.attr(node, "multiple").is_some() || doc.attr(node, "size").and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0) > 1;
            if listbox {
                return None;
            }
            Some(Widget::Overlay(select_arrow(b, if off { faded(style.color) } else { style.color })))
        }
        "textarea" if style.resize != rabbithole_style::Resize::None => Some(Widget::Overlay(resize_grip(b))),
        "progress" if auto => Some(Widget::Replace(progress(r, doc, node, accent))),
        "progress" => Some(Widget::Overlay(css_progress(b, doc, node, ctx.styles.pseudo(node, rabbithole_css::PseudoElement::Marker).map(|s| &**s)))),
        "meter" if auto => Some(Widget::Replace(meter(r, doc, node))),
        _ => None,
    }
}

fn checkbox(r: Rect, doc: &Document, node: NodeId, accent: Color, off: bool) -> Vec<DisplayItem> {
    let checked = doc.is_checked(node);
    let mixed = doc.attr(node, "data-rh-indeterminate").is_some();
    let s = r.w.min(r.h);
    let b = centered_square(r, s);
    let radius = (s * 2.0 / 13.0).max(1.0);
    let mut out = Vec::new();
    if checked || mixed {
        let fill = if off { faded(BORDER) } else { accent };
        out.push(rect(b, fill, radius));
        let mark = on_color(fill);
        let w = (s * 0.16).max(1.5);
        if mixed {
            let h = w;
            out.push(rect(Rect::new(b.x + s * 0.23, b.y + (s - h) / 2.0, s * 0.54, h), mark, 0.0));
        } else {
            // A check mark: short stroke down to the right, long one up.
            let (x0, y0) = (b.x + s * 0.22, b.y + s * 0.52);
            let (x1, y1) = (b.x + s * 0.42, b.y + s * 0.72);
            let (x2, y2) = (b.x + s * 0.79, b.y + s * 0.30);
            out.push(line(x0, y0, x1 + w * 0.25, y1 + w * 0.25, w, mark));
            out.push(line(x1, y1, x2, y2, w, mark));
        }
    } else {
        out.push(rect(b, if off { Color::rgba(255, 255, 255, 128) } else { Color::WHITE }, radius));
        out.push(ring(b, 1.0, if off { faded(BORDER) } else { BORDER }, radius));
    }
    out
}

fn radio(r: Rect, checked: bool, accent: Color, off: bool) -> Vec<DisplayItem> {
    let s = r.w.min(r.h);
    let b = centered_square(r, s);
    let mut out = Vec::new();
    if checked {
        let c = if off { faded(BORDER) } else { accent };
        out.push(rect(b, Color::WHITE, s / 2.0));
        let w = (s * 0.1).max(1.0);
        out.push(ring(b, w, c, s / 2.0));
        let dot = s * 0.54;
        out.push(rect(centered_square(b, dot), c, dot / 2.0));
    } else {
        out.push(rect(b, if off { Color::rgba(255, 255, 255, 128) } else { Color::WHITE }, s / 2.0));
        out.push(ring(b, 1.0, if off { faded(BORDER) } else { BORDER }, s / 2.0));
    }
    out
}

fn num_attr(doc: &Document, node: NodeId, name: &str) -> Option<f32> {
    doc.attr(node, name).and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| v.is_finite())
}

fn range(b: &LayoutBox, doc: &Document, node: NodeId, accent: Color, off: bool) -> Vec<DisplayItem> {
    let min = num_attr(doc, node, "min").unwrap_or(0.0);
    let max = num_attr(doc, node, "max").unwrap_or(100.0).max(min);
    let value = doc.control_value(node).and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(min + (max - min) / 2.0);
    let t = if max > min { ((value - min) / (max - min)).clamp(0.0, 1.0) } else { 0.0 };
    let c = b.content_rect;
    let vertical = c.h > c.w * 1.5;
    let thumb = 16.0f32.min(if vertical { c.w } else { c.h }).max(8.0);
    let track_h = (thumb / 2.0).max(4.0);
    let fill = if off { faded(BORDER) } else { accent };
    let mut out = Vec::new();
    if vertical {
        let cx = c.x + c.w / 2.0;
        let track = Rect::new(cx - track_h / 2.0, c.y + thumb / 2.0 - track_h / 2.0, track_h, (c.h - thumb + track_h).max(0.0));
        out.push(rect(track, TRACK, track_h / 2.0));
        out.push(ring(track, 1.0, TRACK_BORDER, track_h / 2.0));
        let ty = c.bottom() - thumb / 2.0 - t * (c.h - thumb);
        let filled = Rect::new(track.x, ty, track.w, track.bottom() - ty);
        out.push(rect(filled, fill, track_h / 2.0));
        out.push(rect(Rect::new(cx - thumb / 2.0, ty - thumb / 2.0, thumb, thumb), fill, thumb / 2.0));
    } else {
        let cy = c.y + c.h / 2.0;
        let track = Rect::new(c.x + thumb / 2.0 - track_h / 2.0, cy - track_h / 2.0, (c.w - thumb + track_h).max(0.0), track_h);
        out.push(rect(track, TRACK, track_h / 2.0));
        out.push(ring(track, 1.0, TRACK_BORDER, track_h / 2.0));
        let tx = c.x + thumb / 2.0 + t * (c.w - thumb);
        let filled = Rect::new(track.x, track.y, tx - track.x, track.h);
        out.push(rect(filled, fill, track_h / 2.0));
        out.push(rect(Rect::new(tx - thumb / 2.0, cy - thumb / 2.0, thumb, thumb), fill, thumb / 2.0));
    }
    out
}

fn range_fraction(doc: &Document, node: NodeId) -> f32 {
    let min = num_attr(doc, node, "min").unwrap_or(0.0);
    let max = num_attr(doc, node, "max").unwrap_or(100.0).max(min);
    let value = doc.control_value(node).and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(min + (max - min) / 2.0);
    if max > min {
        ((value - min) / (max - min)).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn px_len(l: rabbithole_style::Length, of: f32) -> Option<f32> {
    l.resolve(of)
}

/// The thumb of a slider without native appearance: its
/// `::-webkit-slider-thumb` box (or a 16px accent disc), centered on the
/// track.
fn css_thumb(b: &LayoutBox, doc: &Document, node: NodeId, thumb: Option<&rabbithole_style::ComputedStyle>, accent: Color) -> Vec<DisplayItem> {
    let c = b.content_rect;
    let t = range_fraction(doc, node);
    let (w, h) = match thumb {
        Some(s) => (px_len(s.width, c.w).unwrap_or(16.0), px_len(s.height, c.h).unwrap_or(16.0)),
        None => (16.0, 16.0),
    };
    let x = c.x + t * (c.w - w).max(0.0);
    let r = Rect::new(x, c.y + (c.h - h) / 2.0, w, h);
    let Some(s) = thumb.filter(|s| s.appearance == Appearance::None) else {
        return vec![rect(centered_square(r, w.min(h)), accent, w.min(h) / 2.0)];
    };
    let radii = s.resolved_radii(w, h);
    let mut out = Vec::new();
    if s.background_color.a > 0 {
        out.push(DisplayItem::RectRadii { rect: r, radii, paint: Paint::Solid(s.background_color) });
    }
    let bw = s.border_width;
    if bw.top > 0.0 || bw.left > 0.0 {
        out.push(DisplayItem::Border {
            rect: r,
            widths: [bw.top, bw.right, bw.bottom, bw.left],
            colors: [s.border_color.top, s.border_color.right, s.border_color.bottom, s.border_color.left],
            styles: [s.border_style.top, s.border_style.right, s.border_style.bottom, s.border_style.left],
            radii,
        });
    }
    out
}

/// The value of a progress bar without native appearance: its
/// `::-webkit-progress-value` background (green by default) over the
/// element's own CSS box.
fn css_progress(b: &LayoutBox, doc: &Document, node: NodeId, value_style: Option<&rabbithole_style::ComputedStyle>) -> Vec<DisplayItem> {
    let p = b.padding_rect();
    let max = num_attr(doc, node, "max").filter(|m| *m > 0.0).unwrap_or(1.0);
    let Some(v) = num_attr(doc, node, "value").map(|v| v.clamp(0.0, max)) else { return Vec::new() };
    let mut out = Vec::new();
    if b.style.background_color.a == 0 && b.style.background_image.is_none() {
        out.push(rect(p, Color::rgb(128, 128, 128), 0.0));
    }
    let color = value_style.map(|s| s.background_color).filter(|c| c.a > 0).unwrap_or(Color::rgb(0, 128, 0));
    let w = p.w * v / max;
    if w > 0.0 {
        let radii = b.style.resolved_radii(b.rect.w, b.rect.h);
        out.push(DisplayItem::PushClipRounded(p, radii));
        let vr = value_style.map(|s| s.resolved_radii(w, p.h)).unwrap_or([(0.0, 0.0); 4]);
        out.push(DisplayItem::RectRadii { rect: Rect::new(p.x, p.y, w, p.h), radii: vr, paint: Paint::Solid(color) });
        out.push(DisplayItem::PopClip);
    }
    out
}

fn progress(r: Rect, doc: &Document, node: NodeId, accent: Color) -> Vec<DisplayItem> {
    let max = num_attr(doc, node, "max").filter(|m| *m > 0.0).unwrap_or(1.0);
    let value = num_attr(doc, node, "value").map(|v| v.clamp(0.0, max));
    let radius = (r.h / 2.0).min(4.0);
    let mut out = vec![rect(r, TRACK, radius), ring(r, 1.0, TRACK_BORDER, radius)];
    match value {
        Some(v) => {
            let w = r.w * v / max;
            if w > 0.0 {
                out.push(DisplayItem::PushClipRounded(r, [(radius, radius); 4]));
                out.push(rect(Rect::new(r.x, r.y, w, r.h), accent, 0.0));
                out.push(DisplayItem::PopClip);
            }
        }
        None => {
            // Indeterminate: a segment of the bar (a static frame of
            // Blink's animation).
            out.push(DisplayItem::PushClipRounded(r, [(radius, radius); 4]));
            out.push(rect(Rect::new(r.x + r.w * 0.1, r.y, r.w * 0.3, r.h), accent, 0.0));
            out.push(DisplayItem::PopClip);
        }
    }
    out
}

fn meter(r: Rect, doc: &Document, node: NodeId) -> Vec<DisplayItem> {
    let min = num_attr(doc, node, "min").unwrap_or(0.0);
    let max = num_attr(doc, node, "max").unwrap_or(1.0).max(min);
    let value = num_attr(doc, node, "value").unwrap_or(0.0).clamp(min, max);
    let low = num_attr(doc, node, "low").unwrap_or(min).clamp(min, max);
    let high = num_attr(doc, node, "high").unwrap_or(max).clamp(low, max);
    let optimum = num_attr(doc, node, "optimum").unwrap_or((min + max) / 2.0).clamp(min, max);
    // HTML: the optimum region, then "suboptimal" and "even less good".
    let region = |v: f32| if v < low { 0 } else if v > high { 2 } else { 1 };
    let (rv, ro) = (region(value), region(optimum));
    let color = if rv == ro {
        Color::rgb(16, 124, 16)
    } else if (rv as i32 - ro as i32).abs() == 1 {
        Color::rgb(255, 185, 0)
    } else {
        Color::rgb(216, 59, 1)
    };
    let radius = (r.h / 2.0).min(4.0);
    let mut out = vec![rect(r, TRACK, radius), ring(r, 1.0, TRACK_BORDER, radius)];
    let w = if max > min { r.w * (value - min) / (max - min) } else { 0.0 };
    if w > 0.0 {
        out.push(DisplayItem::PushClipRounded(r, [(radius, radius); 4]));
        out.push(rect(Rect::new(r.x, r.y, w, r.h), color, 0.0));
        out.push(DisplayItem::PopClip);
    }
    out
}

fn color_well(b: &LayoutBox, doc: &Document, node: NodeId) -> Vec<DisplayItem> {
    let v = doc.control_value(node).unwrap_or("#000000").trim().to_ascii_lowercase();
    let hex = v.strip_prefix('#').filter(|h| h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()));
    let color = hex
        .map(|h| {
            let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0);
            Color::rgb(p(0), p(2), p(4))
        })
        .unwrap_or(Color::BLACK);
    // The swatch fills the content box inset by Blink's 4px × 2px wrapper
    // padding, framed in grey.
    let c = b.content_rect;
    let sw = Rect::new(c.x + 2.0, c.y + 4.0, (c.w - 4.0).max(0.0), (c.h - 8.0).max(0.0));
    if sw.w <= 0.0 || sw.h <= 0.0 {
        return Vec::new();
    }
    vec![
        DisplayItem::RectRadii { rect: sw, radii: [(0.0, 0.0); 4], paint: Paint::Solid(color) },
        ring(sw, 1.0, Color::rgb(119, 119, 119), 0.0),
    ]
}

/// The chevron of a drop-down menu, at the right of its padding box.
fn select_arrow(b: &LayoutBox, color: Color) -> Vec<DisplayItem> {
    let p = b.padding_rect();
    let w = 8.0f32.min(p.w / 2.0);
    if w < 3.0 || p.h < 4.0 {
        return Vec::new();
    }
    let h = w * 0.55;
    let cx = p.right() - crate::build::SELECT_ARROW_WIDTH / 2.0;
    let cy = p.y + p.h / 2.0;
    let (x0, x1, x2) = (cx - w / 2.0, cx, cx + w / 2.0);
    let (ytop, ybot) = (cy - h / 2.0, cy + h / 2.0);
    vec![line(x0, ytop, x1, ybot, 1.6, color), line(x1, ybot, x2, ytop, 1.6, color)]
}

/// A small calendar (or clock for `time`) glyph at the right of the
/// content box.
fn calendar_icon(b: &LayoutBox, ty: &str, color: Color) -> Vec<DisplayItem> {
    let c = b.content_rect;
    let s = 12.0f32.min(c.h).min(c.w / 3.0);
    if s < 6.0 {
        return Vec::new();
    }
    let ir = Rect::new(c.right() - s - 1.0, c.y + (c.h - s) / 2.0, s, s);
    if ty == "time" {
        let (cx, cy) = (ir.x + s / 2.0, ir.y + s / 2.0);
        return vec![ring(ir, 1.2, color, s / 2.0), line(cx, cy, cx, ir.y + s * 0.22, 1.2, color), line(cx, cy, ir.x + s * 0.75, cy, 1.2, color)];
    }
    let mut out = vec![ring(ir, 1.0, color, 1.0), rect(Rect::new(ir.x, ir.y, ir.w, (s * 0.25).round()), color, 0.0)];
    // A 3×2 grid of days.
    let cell = s / 5.0;
    for row in 0..2 {
        for col in 0..3 {
            let x = ir.x + cell * (0.8 + col as f32 * 1.3);
            let y = ir.y + s * 0.42 + row as f32 * cell * 1.3;
            out.push(rect(Rect::new(x, y, cell * 0.8, cell * 0.8), color, 0.0));
        }
    }
    out
}

/// Two diagonal strokes in the bottom-right corner of a resizable box.
fn resize_grip(b: &LayoutBox) -> Vec<DisplayItem> {
    let p = b.padding_rect();
    if p.w < 16.0 || p.h < 16.0 {
        return Vec::new();
    }
    let (rx, by) = (p.right() - 2.0, p.bottom() - 2.0);
    let c = Color::rgb(130, 130, 130);
    vec![line(rx - 9.0, by, rx, by - 9.0, 1.0, c), line(rx - 5.0, by, rx, by - 5.0, 1.0, c)]
}
