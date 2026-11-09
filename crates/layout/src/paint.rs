//! Display list rasterization.

use crate::display_list::{DisplayItem, DisplayList};
use rabbithole_gfx::{Canvas, FontContext, Rect};
use rabbithole_style::BorderStyle;

fn style_code(s: BorderStyle) -> u8 {
    match s {
        BorderStyle::None => 0,
        BorderStyle::Solid => 0,
        BorderStyle::Dashed => 1,
        BorderStyle::Dotted => 2,
        BorderStyle::Double => 3,
    }
}

pub fn paint(list: &DisplayList, canvas: &mut Canvas, fonts: &FontContext, scroll_x: f32, scroll_y: f32) {
    let view = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
    let (dx, dy) = (-scroll_x, -scroll_y);
    // Clip depth tracking so culled PushClips still balance their PopClips.
    let mut skipped_depth: Vec<bool> = Vec::new();
    for item in &list.items {
        match item {
            DisplayItem::PushClip(r) => {
                let r = r.translate(dx, dy);
                canvas.push_clip(r);
                skipped_depth.push(false);
            }
            DisplayItem::PopClip => {
                if skipped_depth.pop().is_some() {
                    canvas.pop_clip();
                }
            }
            DisplayItem::Rect { rect, color, radius } => {
                let r = rect.translate(dx, dy);
                if r.intersect(&view).is_empty() || color.is_transparent() {
                    continue;
                }
                if *radius > 0.5 {
                    canvas.fill_rounded_rect(r, *radius, *color);
                } else {
                    canvas.fill_rect(r, *color);
                }
            }
            DisplayItem::Border {
                rect,
                widths,
                colors,
                styles,
                radius,
            } => {
                let r = rect.translate(dx, dy);
                if r.intersect(&view).is_empty() {
                    continue;
                }
                let mut w = *widths;
                let mut c = *colors;
                for i in 0..4 {
                    if styles[i] == BorderStyle::None {
                        w[i] = 0.0;
                    }
                    if w[i] <= 0.0 {
                        c[i] = rabbithole_gfx::Color::TRANSPARENT;
                    }
                }
                let s = [style_code(styles[0]), style_code(styles[1]), style_code(styles[2]), style_code(styles[3])];
                canvas.stroke_rect(r, w, c, s, *radius);
            }
            DisplayItem::Text {
                x,
                y,
                font,
                size,
                color,
                text,
                letter_spacing,
            } => {
                let (px, py) = (x + dx, y + dy);
                // Quick vertical cull.
                if py + size < -2.0 || py - size * 1.5 > view.h + 2.0 || px > view.w {
                    continue;
                }
                if letter_spacing.abs() < 0.01 {
                    canvas.draw_text(fonts, *font, *size, text, px, py, *color);
                } else {
                    for g in fonts.shape(*font, *size, text, *letter_spacing) {
                        let bmp = fonts.rasterize(g.font, g.glyph, *size);
                        if bmp.width == 0 || bmp.height == 0 {
                            continue;
                        }
                        let gx = (px + g.x).round() as i32 + bmp.left;
                        let gy = py.round() as i32 - bmp.top;
                        canvas.draw_glyph(&bmp, gx, gy, *color);
                    }
                }
            }
            DisplayItem::Image { rect, image, opacity } => {
                let r = rect.translate(dx, dy);
                if r.intersect(&view).is_empty() {
                    continue;
                }
                canvas.draw_image(image, r, *opacity);
            }
            DisplayItem::Line {
                x0,
                y0,
                x1,
                y1,
                width,
                color,
            } => {
                let (ax, ay, bx, by) = (x0 + dx, y0 + dy, x1 + dx, y1 + dy);
                if ay.max(by) + width < 0.0 || ay.min(by) - width > view.h || ax.max(bx) < 0.0 || ax.min(bx) > view.w {
                    continue;
                }
                if (ay - by).abs() < 0.01 {
                    canvas.fill_rect(Rect::new(ax.min(bx), ay - width / 2.0, (bx - ax).abs(), *width), *color);
                } else {
                    canvas.draw_line(ax, ay, bx, by, *width, *color);
                }
            }
        }
    }
    while skipped_depth.pop().is_some() {
        canvas.pop_clip();
    }
}
