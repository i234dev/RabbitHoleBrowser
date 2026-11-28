//! Layout and painting: builds a box tree from the styled DOM, runs block /
//! inline / simple flex layout, produces a display list and rasterizes it
//! onto a `Canvas`.

pub mod display_list;
pub mod paint;
pub mod tree;
pub mod selection;

mod anchor;
mod background;
mod border_image;
mod block;
mod motion;
mod build;
pub mod clip_path;
mod counters;
mod damage;
mod flex;
mod first_letter;
mod float;
mod frameset;
mod grid;
mod incremental;
mod inline;
mod intrinsic;
mod table;
mod text;
mod linebreak;
mod multicol;
mod widgets;
mod wm;
mod filter;
mod paint3d;
mod bidi;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_flexgrid;
#[cfg(test)]
mod tests_css21;
#[cfg(test)]
mod tests_text;
#[cfg(test)]
mod tests_wm;
#[cfg(test)]
mod tests_multicol;

pub use damage::{damage as display_list_damage, video_damage as video_display_list_damage, merge_rects as merge_damage};
pub use incremental::{layout_incremental, take_cache_stats, LayoutDamage};
pub use display_list::{DisplayItem, DisplayList, StickyPatch, HitEntry, HitList, HitScope, HitScopeNode};
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
/// Does this `<object>` show an HTML document (in a nested browsing
/// context, like an `<iframe>`) rather than an image or its fallback?
pub fn object_shows_document(doc: &rabbithole_dom::Document, n: rabbithole_dom::NodeId) -> bool {
    if !doc.is_element(n, "object") {
        return false;
    }
    let Some(data) = doc.attr(n, "data").map(str::trim).filter(|d| !d.is_empty()) else { return false };
    // (Only documents named by a relative URL, of the page's own site: an
    // object pointing elsewhere keeps its fallback content, which it would
    // show anyway should the load fail.)
    let has_scheme = data.find(':').is_some_and(|i| i > 0 && data[..i].bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')));
    if has_scheme || data.starts_with("//") {
        return false;
    }
    match doc.attr(n, "type").map(|t| t.trim().to_ascii_lowercase()).filter(|t| !t.is_empty()) {
        Some(t) => t == "text/html" || t == "application/xhtml+xml",
        None => {
            let path = data.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
            [".html", ".htm", ".xht", ".xhtml"].iter().any(|e| path.ends_with(e))
        }
    }
}

pub fn layout(ctx: &LayoutContext) -> LayoutTree {
    build::reset_list_numbers();
    tree::layout_document(ctx)
}

/// Build a display list from a laid out tree.
pub fn build_display_list(tree: &LayoutTree, ctx: &LayoutContext) -> DisplayList {
    display_list::build(tree, ctx, None)
}

/// [`build_display_list`] with the highlight pseudo-element styles of the
/// selection and of the custom highlights (see
/// `StyleEngine::highlight_styles`).
pub fn build_display_list_with_highlights(tree: &LayoutTree, ctx: &LayoutContext, highlights: Option<&rabbithole_style::highlight::PaintHighlights>) -> DisplayList {
    display_list::build(tree, ctx, highlights)
}

/// Rasterize a display list. `scroll_x/scroll_y` are subtracted from every
/// item, so the canvas shows the viewport starting at that document offset.
pub fn paint(list: &DisplayList, canvas: &mut Canvas, fonts: &FontContext, scroll_x: f32, scroll_y: f32) {
    paint::paint(list, canvas, fonts, scroll_x, scroll_y)
}

/// Rasterize only `region` (device pixels) of a display list.
pub fn paint_region(list: &DisplayList, canvas: &mut Canvas, fonts: &FontContext, scroll_x: f32, scroll_y: f32, region: rabbithole_gfx::Rect) {
    paint::paint_region(list, canvas, fonts, scroll_x, scroll_y, region)
}

/// Rasterize with `position: fixed` content placed at `fixed_origin`
/// (canvas coordinates of the viewport's top-left corner).
pub fn paint_with_fixed_origin(list: &DisplayList, canvas: &mut Canvas, fonts: &FontContext, scroll_x: f32, scroll_y: f32, fixed_origin: (f32, f32)) {
    paint::paint_with_fixed_origin(list, canvas, fonts, scroll_x, scroll_y, fixed_origin)
}

/// The bounds of a box's subtree for scrollable overflow (margin boxes,
/// lines, transforms; clipped axes cut).
pub fn block_content_bounds(b: &tree::LayoutBox) -> rabbithole_gfx::Rect {
    block::content_bounds(b)
}
