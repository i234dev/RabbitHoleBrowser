//! Layout and painting: builds a box tree from the styled DOM, runs block /
//! inline / simple flex layout, produces a display list and rasterizes it
//! onto a `Canvas`.

pub mod display_list;
pub mod paint;
pub mod tree;

mod block;
mod build;
mod flex;
mod float;
mod inline;
mod intrinsic;
mod table;
mod text;
#[cfg(test)]
mod tests;

pub use display_list::{DisplayItem, DisplayList};
pub use tree::{BoxKind, Flow, Formatting, Fragment, FragmentKind, InlineItem, LayoutBox, LayoutTree, LineBox, Marker};

use rabbithole_dom::{Document, NodeId};
use rabbithole_gfx::{Canvas, FontContext, Image};
use rabbithole_style::StyledTree;
use std::collections::HashMap;
use std::rc::Rc;

pub struct LayoutContext<'a> {
    pub doc: &'a Document,
    pub styles: &'a StyledTree,
    pub fonts: &'a FontContext,
    /// Decoded images keyed by the `<img>` node (and by background URL for
    /// elements with background images, via `background_images`).
    pub images: &'a HashMap<NodeId, Rc<Image>>,
    pub background_images: &'a HashMap<String, Rc<Image>>,
    pub viewport_width: f32,
    pub viewport_height: f32,
}

/// Lay out the document into absolute (document-space) coordinates.
pub fn layout(ctx: &LayoutContext) -> LayoutTree {
    tree::layout_document(ctx)
}

/// Build a display list from a laid out tree.
pub fn build_display_list(tree: &LayoutTree, ctx: &LayoutContext) -> DisplayList {
    display_list::build(tree, ctx)
}

/// Rasterize a display list. `scroll_x/scroll_y` are subtracted from every
/// item, so the canvas shows the viewport starting at that document offset.
pub fn paint(list: &DisplayList, canvas: &mut Canvas, fonts: &FontContext, scroll_x: f32, scroll_y: f32) {
    paint::paint(list, canvas, fonts, scroll_x, scroll_y)
}
