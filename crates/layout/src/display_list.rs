//! Display list items (document coordinates).

use crate::tree::LayoutTree;
use crate::LayoutContext;
use rabbithole_gfx::{Color, FontId, Image, Rect};
use rabbithole_style::BorderStyle;
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
    let _ = (tree, ctx);
    unimplemented!()
}
