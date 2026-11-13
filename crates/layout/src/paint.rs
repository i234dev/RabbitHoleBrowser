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

/// Snap a box to device pixels like Blink does when painting boxes (edges
/// rounded to the nearest pixel), so adjacent boxes at fractional offsets
/// do not leave antialiased seams. Only without a canvas transform.
fn snap(r: Rect, canvas: &Canvas) -> Rect {
    if !rabbithole_gfx::path::is_identity(&canvas.transform()) {
        return r;
    }
    let (x0, y0) = (r.x.round(), r.y.round());
    let (x1, y1) = ((r.x + r.w).round(), (r.y + r.h).round());
    if (x1 <= x0 && r.w > 0.0) || (y1 <= y0 && r.h > 0.0) {
        return r;
    }
    Rect::new(x0, y0, x1 - x0, y1 - y0)
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
    /// Alpha mask for the layer about to be closed (`mask-image`).
    mask: Option<Vec<u8>>,
    /// Saved translation while inside the fixed layer.
    fixed_saved: Option<(f32, f32)>,
    /// Canvas offset of `position: fixed` content (the viewport origin).
    fixed_origin: (f32, f32),
    ext: bool,
    /// Under canvas transforms: the painted area mapped back into the
    /// items' coordinates (culling must not compare untransformed rects
    /// with the device area), one per pushed matrix.
    views: Vec<Rect>,
}

pub fn paint(list: &DisplayList, canvas: &mut Canvas, fonts: &FontContext, scroll_x: f32, scroll_y: f32) {
    paint_with_fixed_origin(list, canvas, fonts, scroll_x, scroll_y, (0.0, 0.0))
}

/// Like [`paint`], with `position: fixed` content (viewport coordinates)
/// painted at `fixed_origin` on the canvas — where the viewport lies in an
/// overscanned canvas.
pub fn paint_with_fixed_origin(list: &DisplayList, canvas: &mut Canvas, fonts: &FontContext, scroll_x: f32, scroll_y: f32, fixed_origin: (f32, f32)) {
    let view = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
    paint_view(list, canvas, fonts, scroll_x, scroll_y, view, fixed_origin);
}

/// Paint only the device-space `region` (clipped; items outside it are
/// skipped), e.g. the damage of a scroll or a style change.
pub fn paint_region(list: &DisplayList, canvas: &mut Canvas, fonts: &FontContext, scroll_x: f32, scroll_y: f32, region: Rect) {
    let view = region.intersect(&Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32));
    if view.is_empty() {
        return;
    }
    canvas.push_clip(view);
    paint_view(list, canvas, fonts, scroll_x, scroll_y, view, (0.0, 0.0));
    canvas.pop_clip();
}

fn paint_view(list: &DisplayList, canvas: &mut Canvas, fonts: &FontContext, scroll_x: f32, scroll_y: f32, view: Rect, fixed_origin: (f32, f32)) {
    let mut st = State {
        dx: -scroll_x,
        dy: -scroll_y,
        frames: Vec::new(),
        clips: Vec::new(),
        layers: Vec::new(),
        mask: None,
        fixed_saved: None,
        fixed_origin,
        ext: gfx_ext_available(),
        views: Vec::new(),
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
            st.views.pop();
        }
    }
    while let Some(l) = st.layers.pop() {
        if l {
            canvas.end_layer(1.0);
        }
    }
}

/// Alpha (one byte per pixel of the open layer's device bounds) of mask
/// items: they are painted over black and over white with the current
/// transform; where a pixel ends up the same on both, the mask is opaque.
fn layer_mask_alpha(items: &[DisplayItem], canvas: &Canvas, fonts: &FontContext, st: &State) -> Option<Vec<u8>> {
    let (x0, y0, w, h) = canvas.layer_bounds()?;
    if w == 0 || h == 0 {
        return Some(Vec::new());
    }
    let m = rabbithole_style::multiply_matrix([1.0, 0.0, 0.0, 1.0, -(x0 as f32), -(y0 as f32)], canvas.transform());
    // Without a canvas transform the offset goes into the painter's own
    // translation, so boxes snap to pixels exactly as on the page.
    let plain = rabbithole_gfx::path::is_identity(&canvas.transform());
    let render = |bg: Color| {
        let mut c = Canvas::new(w as u32, h as u32);
        c.clear(bg);
        let (sdx, sdy) = if plain { (st.dx - x0 as f32, st.dy - y0 as f32) } else { (st.dx, st.dy) };
        if !plain {
            c.push_transform(m);
        }
        let mut s = State { dx: sdx, dy: sdy, frames: Vec::new(), clips: Vec::new(), layers: Vec::new(), fixed_saved: None, fixed_origin: st.fixed_origin, ext: st.ext, mask: None, views: Vec::new() };
        let all = Rect::new(-1.0e7, -1.0e7, 2.0e7, 2.0e7);
        for it in items {
            paint_item(it, &mut c, fonts, &mut s, all);
        }
        c.data
    };
    let black = render(Color::BLACK);
    let white = render(Color::WHITE);
    Some(
        black
            .iter()
            .zip(white.iter())
            .map(|(b, wh)| {
                // Per channel: white - black = 255·(1 - alpha).
                let g = |v: u32| ((v >> 8) & 0xff) as i32;
                (255 - (g(*wh) - g(*b))).clamp(0, 255) as u8
            })
            .collect(),
    )
}

/// Device pixel bounds of a local rect under the canvas transform, within
/// the canvas.
fn device_bounds(canvas: &Canvas, r: Rect) -> Option<(i32, i32, usize, usize)> {
    let d = rabbithole_gfx::path::transform_rect_bounds(&canvas.transform(), r);
    let x0 = d.x.floor().max(0.0) as i32;
    let y0 = d.y.floor().max(0.0) as i32;
    let x1 = (d.x + d.w).ceil().min(canvas.width as f32) as i32;
    let y1 = (d.y + d.h).ceil().min(canvas.height as f32) as i32;
    (x1 > x0 && y1 > y0).then(|| (x0, y0, (x1 - x0) as usize, (y1 - y0) as usize))
}

/// Draw straight RGBA pixels at device position (x0, y0), clipped by the
/// canvas clips.
fn draw_device_pixels(canvas: &mut Canvas, rgba: Vec<u8>, x0: i32, y0: i32, w: usize, h: usize) {
    let img = rabbithole_gfx::Image::new(w as u32, h as u32, rgba);
    let inv = crate::tree::invert_matrix(canvas.transform());
    if let Some(inv) = inv {
        canvas.push_transform(inv);
    }
    let smooth = canvas.image_smoothing;
    canvas.image_smoothing = false;
    canvas.draw_image(&img, Rect::new(x0 as f32, y0 as f32, w as f32, h as f32), 1.0);
    canvas.image_smoothing = smooth;
    if inv.is_some() {
        canvas.pop_transform();
    }
}

fn device_scale(m: &Matrix) -> f32 {
    (m[0] * m[3] - m[1] * m[2]).abs().sqrt().max(0.01)
}

/// `filter`: render the group over black and over white offscreen,
/// recover its RGBA, filter it and composite the result.
fn paint_filtered(bounds: Rect, filters: &[rabbithole_style::FilterOp], items: &[DisplayItem], canvas: &mut Canvas, fonts: &FontContext, st: &mut State, view: Rect) {
    let local = bounds.translate(st.dx, st.dy);
    if !st.ext || local.intersect(&view).is_empty() && rabbithole_gfx::path::transform_rect_bounds(&canvas.transform(), local).intersect(&Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32)).is_empty() {
        if !st.ext {
            for it in items {
                paint_item(it, canvas, fonts, st, view);
            }
        }
        return;
    }
    let Some((x0, y0, w, h)) = device_bounds(canvas, local) else { return };
    let m = rabbithole_style::multiply_matrix([1.0, 0.0, 0.0, 1.0, -(x0 as f32), -(y0 as f32)], canvas.transform());
    // Without a canvas transform the offset goes into the painter's own
    // translation, so boxes snap to pixels exactly as on the page.
    let plain = rabbithole_gfx::path::is_identity(&canvas.transform());
    let render = |bg: Color| {
        let mut c = Canvas::new(w as u32, h as u32);
        c.clear(bg);
        let (sdx, sdy) = if plain { (st.dx - x0 as f32, st.dy - y0 as f32) } else { (st.dx, st.dy) };
        if !plain {
            c.push_transform(m);
        }
        let mut s = State { dx: sdx, dy: sdy, frames: Vec::new(), clips: Vec::new(), layers: Vec::new(), fixed_saved: None, fixed_origin: st.fixed_origin, ext: st.ext, mask: None, views: Vec::new() };
        let all = Rect::new(-1.0e7, -1.0e7, 2.0e7, 2.0e7);
        for it in items {
            paint_item(it, &mut c, fonts, &mut s, all);
        }
        c.data
    };
    let black = render(Color::BLACK);
    let white = render(Color::WHITE);
    let mut px = crate::filter::from_black_white(&black, &white, w, h);
    crate::filter::apply(&mut px, filters, device_scale(&canvas.transform()));
    draw_device_pixels(canvas, crate::filter::to_rgba(&px), x0, y0, w, h);
}

/// `backdrop-filter`: filter the canvas pixels under `rect`.
fn paint_backdrop(rect: Rect, radii: Radii, filters: &[rabbithole_style::FilterOp], canvas: &mut Canvas, st: &mut State, view: Rect) {
    let local = snap(rect.translate(st.dx, st.dy), canvas);
    if !st.ext {
        return;
    }
    let _ = view;
    let Some((x0, y0, w, h)) = device_bounds(canvas, local) else { return };
    // The filter input is the backdrop within the border box; blurs see
    // it extended by mirroring at the box edges (no bleeding in from
    // outside).
    // Mirroring repeats the box beyond its size: no need for more margin.
    let e = (crate::filter::spread(filters) * device_scale(&canvas.transform())).ceil().min(w.max(h) as f32) as i32;
    let (pw, ph) = (w + 2 * e as usize, h + 2 * e as usize);
    let mirror = |i: i32, n: usize| -> usize {
        let n = n as i32;
        let m = if i < 0 { -i - 1 } else if i >= n { 2 * n - i - 1 } else { i };
        m.clamp(0, n - 1) as usize
    };
    let cw = canvas.width as usize;
    let mut data = Vec::with_capacity(pw * ph);
    for py in 0..ph {
        let y = y0 as usize + mirror(py as i32 - e, h);
        for px in 0..pw {
            let x = x0 as usize + mirror(px as i32 - e, w);
            data.push(canvas.data[y * cw + x]);
        }
    }
    let mut px = crate::filter::from_opaque(&data, pw, ph);
    crate::filter::apply(&mut px, filters, device_scale(&canvas.transform()));
    // Keep the box's part, composited over the original backdrop (drop
    // shadows / opacity may leave it partly transparent).
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let i = (y + e as usize) * pw + x + e as usize;
            let c = px.px[i];
            let o = data[i];
            let ch = |s: u32| ((o >> s) & 0xff) as f32 / 255.0;
            let inv = 1.0 - c[3];
            for (k, s) in [16u32, 8, 0].into_iter().enumerate() {
                rgba.push(((c[k] + ch(s) * inv).clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            }
            rgba.push(255);
        }
    }
    canvas.push_clip_rounded(local, radii);
    draw_device_pixels(canvas, rgba, x0, y0, w, h);
    canvas.pop_clip();
}

fn paint_item(item: &DisplayItem, canvas: &mut Canvas, fonts: &FontContext, st: &mut State, device_view: Rect) {
    let (dx, dy) = (st.dx, st.dy);
    let view = st.views.last().copied().unwrap_or(device_view);
    match item {
        DisplayItem::PushClip(r) => {
            let r = snap(r.translate(dx, dy), canvas);
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
                let local = match crate::tree::invert_matrix(canvas.transform()) {
                    Some(inv) => rabbithole_gfx::path::transform_rect_bounds(&inv, device_view),
                    None => Rect::new(0.0, 0.0, 0.0, 0.0),
                };
                st.views.push(local);
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
            Some(Frame::Canvas) => {
                canvas.pop_transform();
                st.views.pop();
            }
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
                match st.mask.take() {
                    Some(alpha) => canvas.end_layer_masked(*opacity, &alpha),
                    None => canvas.end_layer(*opacity),
                }
            }
        }
        DisplayItem::Filtered { bounds, filters, items } => {
            paint_filtered(*bounds, filters, items, canvas, fonts, st, view);
        }
        DisplayItem::BackdropFilter { rect, radii, filters } => {
            paint_backdrop(*rect, *radii, filters, canvas, st, view);
        }
        DisplayItem::LayerMask(items) => {
            st.mask = None;
            if st.layers.last() == Some(&true) {
                st.mask = layer_mask_alpha(items, canvas, fonts, st);
            }
        }
        DisplayItem::PushFixed => {
            // Fixed items are in viewport coordinates: drop the scroll.
            st.fixed_saved = Some((st.dx, st.dy));
            st.dx = st.fixed_origin.0;
            st.dy = st.fixed_origin.1;
        }
        DisplayItem::PopFixed => {
            if let Some((x, y)) = st.fixed_saved.take() {
                st.dx = x;
                st.dy = y;
            }
        }
        DisplayItem::Rect { rect, color, radius } => {
            let r = snap(rect.translate(dx, dy), canvas);
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
            let r = snap(rect.translate(dx, dy), canvas);
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
            let r = snap(rect.translate(dx, dy), canvas);
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
            if py + size < view.y - 2.0 || py - size * 1.5 > view.bottom() + 2.0 || px > view.right() {
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
        DisplayItem::Image { rect, src, image, opacity, pixelated } => {
            let r = snap(rect.translate(dx, dy), canvas);
            if r.intersect(&view).is_empty() {
                return;
            }
            let full = src.x <= 0.0
                && src.y <= 0.0
                && (src.w - image.width as f32).abs() < 0.5
                && (src.h - image.height as f32).abs() < 0.5;
            canvas.image_smoothing = !*pixelated;
            if full || !st.ext {
                canvas.draw_image(image, r, *opacity);
            } else {
                canvas.draw_image_region(image, *src, r, *opacity);
            }
            canvas.image_smoothing = true;
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
            let a = snap(area.translate(dx, dy), canvas);
            if a.intersect(&view).is_empty() {
                return;
            }
            let o = if rabbithole_gfx::path::is_identity(&canvas.transform()) { Point { x: (origin.x + dx).round(), y: (origin.y + dy).round() } } else { Point { x: origin.x + dx, y: origin.y + dy } };
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
            if ay.max(by) + width < view.y || ay.min(by) - width > view.bottom() || ax.max(bx) < view.x || ax.min(bx) > view.right() {
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
            let gx = (px + g.x + g.dx).round() as i32 + bmp.left;
            let gy = (py - g.dy).round() as i32 - bmp.top;
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
            // A sine wave (Blink: amplitude ~1.5× the thickness, a period
            // of ~6× it), stroked as one polyline.
            let amp = (width * 1.5).max(1.0);
            let period = (width * 6.0).max(6.0);
            let mut path = vec![rabbithole_gfx::PathSeg::MoveTo(Point { x: x0, y: ay })];
            let n = (((x1 - x0) / 1.0).ceil() as usize).min(100_000);
            for i in 1..=n {
                let x = (x0 + i as f32).min(x1);
                let y = ay - amp * ((x - x0) / period * std::f32::consts::TAU).sin();
                path.push(rabbithole_gfx::PathSeg::LineTo(Point { x, y }));
            }
            let style = rabbithole_gfx::StrokeStyle { width: width.max(1.0), ..Default::default() };
            canvas.stroke_path(&path, &style, &Paint::Solid(color));
        }
    }
}

#[allow(dead_code)]
fn unused(_: &Shadow) {}
