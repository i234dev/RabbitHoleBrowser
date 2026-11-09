//! Display list generation (document coordinates).

use crate::tree::{BoxKind, Flow, FragmentKind, LayoutBox, LayoutTree};
use crate::LayoutContext;
use rabbithole_gfx::{Color, FontId, Image, Rect};
use rabbithole_style::{BorderStyle, Visibility};
use std::rc::Rc;

#[derive(Debug, Clone)]
pub enum DisplayItem {
    Rect {
        rect: Rect,
        color: Color,
        radius: f32,
    },
    Border {
        rect: Rect,
        widths: [f32; 4],
        colors: [Color; 4],
        styles: [BorderStyle; 4],
        radius: f32,
    },
    Text {
        /// Left edge of the run.
        x: f32,
        /// Baseline.
        y: f32,
        font: FontId,
        size: f32,
        color: Color,
        text: String,
        letter_spacing: f32,
    },
    Image {
        rect: Rect,
        image: Rc<Image>,
        opacity: f32,
    },
    /// Horizontal line (underline / strike-through / hr)
    Line {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        width: f32,
        color: Color,
    },
    PushClip(Rect),
    PopClip,
}

#[derive(Debug, Clone, Default)]
pub struct DisplayList {
    pub items: Vec<DisplayItem>,
}

pub fn build(tree: &LayoutTree, ctx: &LayoutContext) -> DisplayList {
    let mut list = DisplayList::default();
    let bounds = Rect::new(0.0, 0.0, tree.width, tree.height);
    // Canvas background: the root's background propagates to the viewport,
    // falling back to the body's.
    let root = &tree.root;
    let mut bg = root.style.background_color;
    let mut root_bg_painted = false;
    if bg.is_transparent() {
        if let Some(body) = root.children.iter().find(|c| c.is_in_flow()) {
            bg = body.style.background_color;
        }
    } else {
        root_bg_painted = true;
    }
    if !bg.is_transparent() {
        list.items.push(DisplayItem::Rect {
            rect: bounds,
            color: bg,
            radius: 0.0,
        });
    }
    let mut p = Painter {
        list: &mut list,
        ctx,
        bounds,
    };
    p.paint_box(root, 1.0, root_bg_painted);
    if root.style.position == rabbithole_style::Position::Static {
        p.paint_abs_children(root, 1.0);
    }
    list
}

struct Painter<'l, 'a> {
    list: &'l mut DisplayList,
    ctx: &'l LayoutContext<'a>,
    bounds: Rect,
}

fn scale(c: Color, opacity: f32) -> Color {
    if opacity >= 1.0 {
        c
    } else {
        c.scale_alpha(opacity)
    }
}

impl<'l, 'a> Painter<'l, 'a> {
    fn visible(&self, r: &Rect) -> bool {
        !r.intersect(&self.bounds).is_empty() || r.is_empty()
    }

    fn paint_background_and_border(&mut self, b: &LayoutBox, rect: Rect, opacity: f32, skip_bg: bool) {
        let style = &b.style;
        if style.visibility == Visibility::Hidden || rect.is_empty() {
            return;
        }
        if !skip_bg && !style.background_color.is_transparent() && self.visible(&rect) {
            self.list.items.push(DisplayItem::Rect {
                rect,
                color: scale(style.background_color, opacity),
                radius: style.border_radius,
            });
        }
        if let Some(url) = &style.background_image {
            if let Some(img) = self.ctx.background_images.get(url) {
                let padding = Rect::new(
                    rect.x + style.border_width.left,
                    rect.y + style.border_width.top,
                    (rect.w - style.border_width.left - style.border_width.right).max(0.0),
                    (rect.h - style.border_width.top - style.border_width.bottom).max(0.0),
                );
                let dest = Rect::new(padding.x, padding.y, img.width as f32, img.height as f32);
                if self.visible(&padding) && !padding.is_empty() {
                    self.list.items.push(DisplayItem::PushClip(padding));
                    self.list.items.push(DisplayItem::Image {
                        rect: dest,
                        image: img.clone(),
                        opacity,
                    });
                    self.list.items.push(DisplayItem::PopClip);
                }
            }
        }
        if style.has_visible_border() && self.visible(&rect) {
            let bw = style.border_width;
            let bs = style.border_style;
            let bc = style.border_color;
            self.list.items.push(DisplayItem::Border {
                rect,
                widths: [bw.top, bw.right, bw.bottom, bw.left],
                colors: [scale(bc.top, opacity), scale(bc.right, opacity), scale(bc.bottom, opacity), scale(bc.left, opacity)],
                styles: [bs.top, bs.right, bs.bottom, bs.left],
                radius: style.border_radius,
            });
        }
    }

    fn paint_box(&mut self, b: &LayoutBox, parent_opacity: f32, skip_bg: bool) {
        let opacity = parent_opacity * b.style.opacity.clamp(0.0, 1.0);
        if opacity <= 0.0 {
            return;
        }
        if b.kind != BoxKind::Text {
            self.paint_background_and_border(b, b.rect, opacity, skip_bg);
        }
        if let Some(m) = &b.marker {
            if b.style.visibility != Visibility::Hidden {
                self.list.items.push(DisplayItem::Text {
                    x: m.x,
                    y: m.baseline,
                    font: m.font,
                    size: m.size,
                    color: scale(m.color, opacity),
                    text: m.text.to_string(),
                    letter_spacing: 0.0,
                });
            }
        }
        if b.kind == BoxKind::Image {
            if let Some(img) = &b.image {
                if b.style.visibility != Visibility::Hidden && self.visible(&b.content_rect) {
                    self.list.items.push(DisplayItem::Image {
                        rect: b.content_rect,
                        image: img.clone(),
                        opacity,
                    });
                }
            }
        }
        let clip = b.clips && !b.rect.is_empty();
        if clip {
            self.list.items.push(DisplayItem::PushClip(b.padding_rect()));
        }
        // Floats first (they sit below inline content).
        for c in b.children.iter().filter(|c| c.flow.is_float()) {
            self.paint_box(c, opacity, false);
        }
        if b.lines.is_empty() {
            for c in b.children.iter().filter(|c| c.flow == Flow::InFlow) {
                self.paint_box(c, opacity, false);
            }
        } else {
            self.paint_lines(b, opacity);
        }
        if clip {
            self.list.items.push(DisplayItem::PopClip);
        }
        // Absolutely positioned descendants with this box as containing block.
        if b.style.position != rabbithole_style::Position::Static {
            self.paint_abs_children(b, opacity);
        }
    }

    fn paint_abs_children(&mut self, b: &LayoutBox, opacity: f32) {
        let mut abs: Vec<&LayoutBox> = Vec::new();
        b.for_each_abs_child(&mut |c| abs.push(c));
        abs.sort_by_key(|c| c.style.z_index.unwrap_or(0));
        for c in abs {
            self.paint_box(c, opacity, false);
        }
    }

    fn paint_lines(&mut self, b: &LayoutBox, opacity: f32) {
        for line in &b.lines {
            if !self.visible(&line.rect) && !line.rect.is_empty() {
                // Lines can have fragments outside their rect (overflow), so
                // only skip when the fragments are also out of bounds.
                if line.fragments.iter().all(|f| !self.visible(&f.rect)) {
                    continue;
                }
            }
            for f in &line.fragments {
                match &f.kind {
                    FragmentKind::InlineBox {
                        style,
                        left_edge,
                        right_edge,
                    } => {
                        if style.visibility == Visibility::Hidden {
                            continue;
                        }
                        if !style.background_color.is_transparent() {
                            self.list.items.push(DisplayItem::Rect {
                                rect: f.rect,
                                color: scale(style.background_color, opacity),
                                radius: style.border_radius,
                            });
                        }
                        if style.has_visible_border() {
                            let bw = style.border_width;
                            let bs = style.border_style;
                            let bc = style.border_color;
                            self.list.items.push(DisplayItem::Border {
                                rect: f.rect,
                                widths: [bw.top, if *right_edge { bw.right } else { 0.0 }, bw.bottom, if *left_edge { bw.left } else { 0.0 }],
                                colors: [scale(bc.top, opacity), scale(bc.right, opacity), scale(bc.bottom, opacity), scale(bc.left, opacity)],
                                styles: [bs.top, bs.right, bs.bottom, bs.left],
                                radius: style.border_radius,
                            });
                        }
                    }
                    FragmentKind::Text {
                        text,
                        font,
                        size,
                        color,
                        letter_spacing,
                        decoration,
                        hidden,
                    } => {
                        if *hidden || !self.visible(&f.rect) {
                            continue;
                        }
                        let color = scale(*color, opacity);
                        self.list.items.push(DisplayItem::Text {
                            x: f.rect.x,
                            y: f.baseline,
                            font: *font,
                            size: *size,
                            color,
                            text: text.to_string(),
                            letter_spacing: *letter_spacing,
                        });
                        if decoration.any() {
                            let thickness = (size / 16.0).max(1.0);
                            if decoration.underline {
                                let y = f.baseline + thickness * 1.5;
                                self.list.items.push(DisplayItem::Line { x0: f.rect.x, y0: y, x1: f.rect.right(), y1: y, width: thickness, color });
                            }
                            if decoration.line_through {
                                let y = f.baseline - size * 0.3;
                                self.list.items.push(DisplayItem::Line { x0: f.rect.x, y0: y, x1: f.rect.right(), y1: y, width: thickness, color });
                            }
                            if decoration.overline {
                                let y = f.rect.y + thickness / 2.0;
                                self.list.items.push(DisplayItem::Line { x0: f.rect.x, y0: y, x1: f.rect.right(), y1: y, width: thickness, color });
                            }
                        }
                    }
                    FragmentKind::Box(i) => {
                        self.paint_box(&b.children[*i], opacity, false);
                    }
                }
            }
        }
    }
}

