//! Box tree + layout algorithms.

use crate::LayoutContext;
use rabbithole_dom::NodeId;
use rabbithole_gfx::Rect;
use rabbithole_style::ComputedStyle;
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxKind {
    Block,
    AnonymousBlock,
    Inline,
    InlineBlock,
    Text,
    Image,
    LineBreak,
}

#[derive(Debug)]
pub struct LayoutBox {
    pub kind: BoxKind,
    pub node: Option<NodeId>,
    pub style: Rc<ComputedStyle>,
    /// Border box in document coordinates.
    pub rect: Rect,
    pub children: Vec<LayoutBox>,
}

#[derive(Debug)]
pub struct LayoutTree {
    pub root: LayoutBox,
    /// Total document width/height (max of viewport and content).
    pub width: f32,
    pub height: f32,
}

impl LayoutTree {
    /// Deepest element box containing the point (document coordinates).
    pub fn node_at(&self, x: f32, y: f32) -> Option<NodeId> {
        let _ = (x, y);
        unimplemented!()
    }

    /// Human readable dump for debugging (`rabbithole dump --layout`).
    pub fn dump(&self) -> String {
        unimplemented!()
    }
}

pub fn layout_document(ctx: &LayoutContext) -> LayoutTree {
    let _ = ctx;
    unimplemented!()
}
