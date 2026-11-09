//! Display list rasterization onto a `Canvas`.
//!
//! Pure translations (sticky offsets, `transform: translate()`) are applied
//! by the painter itself; other matrices, gradients, per-corner radii,
//! shadows and layers use the extended canvas API. When that API is not
//! available (`gfx_ext_available() == false`, during bring-up) the painter
//! degrades gracefully to the basic primitives.

use crate::display_list::{DisplayItem, DisplayList};
use rabbithole_gfx::{Canvas, Color, FontContext, FontId, Matrix, Paint, Point, Radii, Rect};
use rabbithole_style::{BorderStyle, Shadow, TextDecorationStyle};
use std::sync::OnceLock;

fn style_code(s: BorderStyle) -> u8 {
    match s {
        BorderStyle::None => 0,
        BorderStyle::Solid => 0,
        BorderStyle::Dashed => 1,
        BorderStyle::Dotted => 2,
        BorderStyle::Double => 3,
    }
}

/// Whether the extended canvas / font API (gradients, radii, layers, web
/// fonts) is implemented in the linked gfx crate. Probed once.
pub(crate) fn gfx_ext_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let probe = std::panic::catch_unwind(|| {
            let mut c = Canvas::new(1, 1);
            c.push_transform(rabbithole_gfx::IDENTITY);
            c.pop_transform();
            let _ = c.map_point(Point { x: 0.0, y: 0.0 });
        });
        probe.is_ok()
    })
}

/// Whether `FontContext::select_family` & co. are implemented.
pub(crate) fn font_ext_available(fonts: &FontContext) -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let probe = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let f = fonts.select(rabbithole_gfx::Family::Sans, 400, false);
            let _ = fonts.needs_synthetic_italic(f, false);
            let _ = fonts.select_family(&[], rabbithole_gfx::Family::Sans, 400, false);
        }));
        probe.is_ok()
    })
}

fn uniform_radius(r: &Radii) -> Option<f32> {
    let (h, v) = r[0];
    if (h - v).abs() > 0.01 {
        return None;
    }
    if r.iter().all(|(a, b)| (*a - h).abs() < 0.01 && (*b - v).abs() < 0.01) {
        Some(h)
    } else {
        None
    }
}

fn is_translation(m: &Matrix) -> bool {
    (m[0] - 1.0).abs() < 1e-6 && m[1].abs() < 1e-6 && m[2].abs() < 1e-6 && (m[3] - 1.0).abs() < 1e-6
}

fn translate_radii(_r: Radii) -> Radii {
    _r
}

fn shift_paint(p: &Paint, dx: f32, dy: f32) -> Paint {
    match p {
        Paint::Solid(c) => Paint::Solid(*c),
        Paint::Linear { start, end, stops, repeat } => Paint::Linear {
            start: Point { x: start.x + dx, y: start.y + dy },
            end: Point { x: end.x + dx, y: end.y + dy },
            stops: stops.clone(),
            repeat: *repeat,
        },
        Paint::Radial { center, rx, ry, stops, repeat } => Paint::Radial {
            center: Point { x: center.x + dx, y: center.y + dy },
            rx: *rx,
            ry: *ry,
            stops: stops.clone(),
            repeat: *repeat,
        },
    }
}

/// Average color of a paint (fallback when gradients are unavailable).
fn average_color(p: &Paint) -> Color {
    match p {
        Paint::Solid(c) => *c,
        Paint::Linear { stops, .. } | Paint::Radial { stops, .. } => {
            if stops.is_empty() {
                return Color::TRANSPARENT;
            }
            let n = stops.len() as u32;
            let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
            for s in stops {
                r += s.color.r as u32;
                g += s.color.g as u32;
                b += s.color.b as u32;
                a += s.color.a as u32;
            }
            Color::rgba((r / n) as u8, (g / n) as u8, (b / n) as u8, (a / n) as u8)
        }
    }
}

enum Frame {
    /// Pure translation handled by the painter.
    Translate(f32, f32),
    /// Matrix pushed to the canvas.
    Canvas,
}

struct State {
    /// Current translation (document → canvas), including scroll.
    dx: f32,
    dy: f32,
    frames: Vec<Frame>,
    clips: Vec<bool>,
    layers: Vec<bool>,
    /// Saved translation while inside the fixed layer.
    fixed_saved: Option<(f32, f32)>,
    ext: bool,
}

pub fn paint(list: &DisplayList, canvas: &mut Canvas, fonts: &FontContext, scroll_x: f32, scroll_y: f32) {
    let view = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
    let mut st = State {
        dx: -scroll_x,
        dy: -scroll_y,
        frames: Vec::new(),
        clips: Vec::new(),
        layers: Vec::new(),
        fixed_saved: None,
        ext: gfx_ext_available(),
    };
    for item in &list.items {
        paint_item(item, canvas, fonts, &mut st, view);
    }
    // Balance anything left open.
    while let Some(c) = st.clips.pop() {
        if c {
            canvas.pop_clip();
        }
    }
    while let Some(f) = st.frames.pop() {
        if let Frame::Canvas = f {
            canvas.pop_transform();
        }
    }
    while let Some(l) = st.layers.pop() {
        if l {
            canvas.end_layer(1.0);
        }
    }
}

fn paint_item(item: &DisplayItem, canvas: &mut Canvas, fonts: &FontContext, st: &mut State, view: Rect) {
    let (dx, dy) = (st.dx, st.dy);
    match item {
        DisplayItem::PushClip(r) => {
            let r = r.translate(dx, dy);
            canvas.push_clip(r);
            st.clips.push(true);
        }
        DisplayItem::PushClipRounded(r, radii) => {
            let r = r.translate(dx, dy);
            if st.ext && radii.iter().any(|(h, v)| *h > 0.0 && *v > 0.0) {
                canvas.push_clip_rounded(r, translate_radii(*radii));
            } else {
                canvas.push_clip(r);
            }
            st.clips.push(true);
        }
        DisplayItem::PopClip => {
            if let Some(true) = st.clips.pop() {
                canvas.pop_clip();
            }
        }
        DisplayItem::PushTransform(m) => {
            if is_translation(m) {
                st.dx += m[4];
                st.dy += m[5];
                st.frames.push(Frame::Translate(m[4], m[5]));
            } else if st.ext {
                // Compose: canvas space = translate(dx,dy) · M · doc. The
                // canvas transform applies to coordinates we already
                // translated, so conjugate M by the current translation.
                let t = [1.0, 0.0, 0.0, 1.0, dx, dy];
                let ti = [1.0, 0.0, 0.0, 1.0, -dx, -dy];
                let mm = rabbithole_style::multiply_matrix(rabbithole_style::multiply_matrix(t, *m), ti);
                canvas.push_transform(mm);
                st.frames.push(Frame::Canvas);
            } else {
                // Approximate unsupported transforms by their translation.
                st.dx += m[4];
                st.dy += m[5];
                st.frames.push(Frame::Translate(m[4], m[5]));
            }
        }
        DisplayItem::PopTransform => match st.frames.pop() {
            Some(Frame::Translate(x, y)) => {
                st.dx -= x;
                st.dy -= y;
            }
            Some(Frame::Canvas) => canvas.pop_transform(),
            _ => {}
        },
        DisplayItem::PushLayer(bounds) => {
            if st.ext {
                canvas.begin_layer(bounds.translate(dx, dy));
                st.layers.push(true);
            } else {
                st.layers.push(false);
            }
        }
        DisplayItem::PopLayer(opacity) => {
            if let Some(true) = st.layers.pop() {
                canvas.end_layer(*opacity);
            }
        }
        DisplayItem::PushFixed => {
            // Fixed items are in viewport coordinates: drop the scroll.
            st.fixed_saved = Some((st.dx, st.dy));
            st.dx = 0.0;
            st.dy = 0.0;
        }
        DisplayItem::PopFixed => {
            if let Some((x, y)) = st.fixed_saved.take() {
                st.dx = x;
                st.dy = y;
            }
        }
        DisplayItem::Rect { rect, color, radius } => {
            let r = rect.translate(dx, dy);
            if r.intersect(&view).is_empty() || color.is_transparent() {
                return;
            }
            if *radius > 0.5 {
                canvas.fill_rounded_rect(r, *radius, *color);
            } else {
                canvas.fill_rect(r, *color);
            }
        }
        DisplayItem::RectRadii { rect, radii, paint } => {
            let r = rect.translate(dx, dy);
            if r.intersect(&view).is_empty() {
                return;
            }
            if st.ext {
                canvas.fill_rounded_rect_radii(r, *radii, &shift_paint(paint, dx, dy));
            } else {
                let c = average_color(paint);
                match uniform_radius(radii) {
                    Some(rad) if rad > 0.5 => canvas.fill_rounded_rect(r, rad, c),
                    _ => canvas.fill_rect(r, c),
                }
            }
        }
        DisplayItem::Border {
            rect,
            widths,
            colors,
            styles,
            radii,
        } => {
            let r = rect.translate(dx, dy);
            if r.intersect(&view).is_empty() {
                return;
            }
            let mut w = *widths;
            let mut c = *colors;
            for i in 0..4 {
                if styles[i] == BorderStyle::None {
                    w[i] = 0.0;
                }
                if w[i] <= 0.0 {
                    c[i] = Color::TRANSPARENT;
                }
            }
            let s = [style_code(styles[0]), style_code(styles[1]), style_code(styles[2]), style_code(styles[3])];
            match uniform_radius(radii) {
                Some(rad) => canvas.stroke_rect(r, w, c, s, rad),
                None if st.ext => canvas.stroke_rect_radii(r, w, c, s, *radii),
                None => canvas.stroke_rect(r, w, c, s, radii[0].0.min(radii[0].1)),
            }
        }
        DisplayItem::BoxShadow {
            rect,
            radii,
            offset,
            blur,
            spread,
            color,
            inset,
        } => {
            let r = rect.translate(dx, dy);
            let reach = blur * 2.0 + spread.abs() + offset.0.abs().max(offset.1.abs());
            let test = Rect::new(r.x - reach, r.y - reach, r.w + 2.0 * reach, r.h + 2.0 * reach);
            if test.intersect(&view).is_empty() {
                return;
            }
            if st.ext {
                canvas.draw_box_shadow(r, *radii, *offset, *blur, *spread, *color, *inset);
            } else if !*inset {
                // Hard shadow fallback.
                let sr = Rect::new(r.x + offset.0 - spread, r.y + offset.1 - spread, r.w + 2.0 * spread, r.h + 2.0 * spread);
                let c = color.scale_alpha(if *blur > 0.0 { 0.5 } else { 1.0 });
                match uniform_radius(radii) {
                    Some(rad) if rad > 0.5 => canvas.fill_rounded_rect(sr, rad, c),
                    _ => canvas.fill_rect(sr, c),
                }
            }
        }
        DisplayItem::Text {
            x,
            y,
            font,
            size,
            color,
            text,
            letter_spacing,
            shadows,
            small_caps,
            oblique,
        } => {
            let (px, py) = (x + dx, y + dy);
            if py + size < -2.0 || py - size * 1.5 > view.h + 2.0 || px > view.w {
                return;
            }
            if st.ext {
                for sh in shadows.iter().rev() {
                    canvas.draw_text_shadow(fonts, *font, *size, text, px + sh.x, py + sh.y, *letter_spacing, sh.color, sh.blur);
                }
            } else {
                for sh in shadows.iter().rev() {
                    draw_text_basic(canvas, fonts, *font, *size, text, px + sh.x, py + sh.y, sh.color, *letter_spacing);
                }
            }
            if st.ext && (*small_caps || *oblique || letter_spacing.abs() >= 0.01) {
                canvas.draw_text_ext(fonts, *font, *size, text, px, py, *color, *letter_spacing, *small_caps, *oblique);
            } else {
                draw_text_basic(canvas, fonts, *font, *size, text, px, py, *color, *letter_spacing);
            }
        }
        DisplayItem::Image { rect, src, image, opacity } => {
            let r = rect.translate(dx, dy);
            if r.intersect(&view).is_empty() {
                return;
            }
            let full = src.x <= 0.0
                && src.y <= 0.0
                && (src.w - image.width as f32).abs() < 0.5
                && (src.h - image.height as f32).abs() < 0.5;
            if full || !st.ext {
                canvas.draw_image(image, r, *opacity);
            } else {
                canvas.draw_image_region(image, *src, r, *opacity);
            }
        }
        DisplayItem::ImageTiled {
            image,
            area,
            origin,
            tile,
            repeat_x,
            repeat_y,
            opacity,
        } => {
            let a = area.translate(dx, dy);
            if a.intersect(&view).is_empty() {
                return;
            }
            let o = Point { x: origin.x + dx, y: origin.y + dy };
            if st.ext {
                canvas.draw_image_tiled(image, a, o, *tile, *repeat_x, *repeat_y, *opacity);
            } else {
                // Manual tiling with the basic API.
                canvas.push_clip(a);
                let (tw, th) = *tile;
                if tw > 0.5 && th > 0.5 {
                    let x0 = if *repeat_x { o.x - ((o.x - a.x) / tw).ceil() * tw } else { o.x };
                    let y0 = if *repeat_y { o.y - ((o.y - a.y) / th).ceil() * th } else { o.y };
                    let mut y = y0;
                    let mut guard = 0;
                    while y < a.bottom() && guard < 10_000 {
                        let mut x = x0;
                        while x < a.right() && guard < 10_000 {
                            canvas.draw_image(image, Rect::new(x, y, tw, th), *opacity);
                            guard += 1;
                            if !*repeat_x {
                                break;
                            }
                            x += tw;
                        }
                        if !*repeat_y {
                            break;
                        }
                        y += th;
                    }
                }
                canvas.pop_clip();
            }
        }
        DisplayItem::Line {
            x0,
            y0,
            x1,
            y1,
            width,
            color,
            style,
        } => {
            let (ax, ay, bx, by) = (x0 + dx, y0 + dy, x1 + dx, y1 + dy);
            if ay.max(by) + width < 0.0 || ay.min(by) - width > view.h || ax.max(bx) < 0.0 || ax.min(bx) > view.w {
                return;
            }
            draw_decoration_line(canvas, ax, ay, bx, by, *width, *color, *style);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_text_basic(canvas: &mut Canvas, fonts: &FontContext, font: FontId, size: f32, text: &str, px: f32, py: f32, color: Color, letter_spacing: f32) {
    if letter_spacing.abs() < 0.01 {
        canvas.draw_text(fonts, font, size, text, px, py, color);
    } else {
        for g in fonts.shape(font, size, text, letter_spacing) {
            let bmp = fonts.rasterize(g.font, g.glyph, size);
            if bmp.width == 0 || bmp.height == 0 {
                continue;
            }
            let gx = (px + g.x).round() as i32 + bmp.left;
            let gy = py.round() as i32 - bmp.top;
            canvas.draw_glyph(&bmp, gx, gy, color);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_decoration_line(canvas: &mut Canvas, ax: f32, ay: f32, bx: f32, by: f32, width: f32, color: Color, style: TextDecorationStyle) {
    let horizontal = (ay - by).abs() < 0.01;
    if !horizontal {
        canvas.draw_line(ax, ay, bx, by, width, color);
        return;
    }
    let (x0, x1) = (ax.min(bx), ax.max(bx));
    let y = ay - width / 2.0;
    match style {
        TextDecorationStyle::Solid => canvas.fill_rect(Rect::new(x0, y, x1 - x0, width), color),
        TextDecorationStyle::Double => {
            canvas.fill_rect(Rect::new(x0, y - width, x1 - x0, width), color);
            canvas.fill_rect(Rect::new(x0, y + width, x1 - x0, width), color);
        }
        TextDecorationStyle::Dotted | TextDecorationStyle::Dashed => {
            let dash = if style == TextDecorationStyle::Dotted { width.max(1.0) } else { width.max(1.0) * 3.0 };
            let gap = dash;
            let mut x = x0;
            let mut guard = 0;
            while x < x1 && guard < 100_000 {
                let w = dash.min(x1 - x);
                canvas.fill_rect(Rect::new(x, y, w, width), color);
                x += dash + gap;
                guard += 1;
            }
        }
        TextDecorationStyle::Wavy => {
            // Zig-zag approximation of a wave.
            let amp = (width * 1.5).max(1.0);
            let step = (width * 2.0).max(2.0);
            let mut x = x0;
            let mut up = true;
            let mut guard = 0;
            while x < x1 && guard < 100_000 {
                let nx = (x + step).min(x1);
                let (ya, yb) = if up { (ay + amp, ay - amp) } else { (ay - amp, ay + amp) };
                canvas.draw_line(x, ya, nx, yb, width, color);
                x = nx;
                up = !up;
                guard += 1;
            }
        }
    }
}

#[allow(dead_code)]
fn unused(_: &Shadow) {}
