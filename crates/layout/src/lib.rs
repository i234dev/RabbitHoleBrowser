//! Layout and painting: builds a box tree from the styled DOM, runs block /
//! inline / simple flex layout, produces a display list and rasterizes it
//! onto a `Canvas`.

pub mod display_list;
pub mod paint;
pub mod tree;

mod background;
mod block;
mod build;
mod counters;
mod flex;
mod float;
mod grid;
mod inline;
mod intrinsic;
mod table;
mod text;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_flexgrid;

pub use display_list::{DisplayItem, DisplayList, HitEntry, HitList, HitScope, HitScopeNode};
pub use tree::{
    BoxKind, Flow, Formatting, Fragment, FragmentKind, InlineItem, LayoutBox, LayoutTree, LineBox, Marker, ScrollInfo,
};

use std::sync::Arc;
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
    pub images: &'a HashMap<NodeId, Arc<Image>>,
    pub background_images: &'a HashMap<String, Arc<Image>>,
    pub viewport_width: f32,
    pub viewport_height: f32,
    /// Scroll offsets of scroll containers (`overflow: auto|scroll`), keyed
    /// by the element node. The engine updates them and re-runs layout.
    pub scroll_offsets: &'a HashMap<NodeId, (f32, f32)>,
    /// Current page scroll position (used for `position: sticky` and
    /// `background-attachment: fixed` at display-list time).
    pub page_scroll: (f32, f32),
}

/// Style of a pseudo-element box (`::before`, `::after`, `::marker`).
pub(crate) fn pseudo_style(styles: &StyledTree, id: NodeId, which: rabbithole_css::PseudoElement) -> Option<Rc<rabbithole_style::ComputedStyle>> {
    styles.pseudo(id, which).cloned()
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
