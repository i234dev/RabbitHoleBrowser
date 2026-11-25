//! Presenting a page: rasters that only painting reads are made when a
//! frame is shown, not while scripts run.
//!
//! * Child frames changed by scripts are marked unpainted and rasterized
//!   by [`Engine::present`] (see `frames.rs`).
//! * Inline `<svg>` roots get a placeholder image of their raster's size
//!   while styles are flushed (layout only needs its dimensions); the
//!   pixels are drawn from the tree captured then, so they are the same
//!   as rasterizing right away.
//!
//! Painting paths that only borrow the page ([`Engine::render`]) paint a
//! copy of the display list with the pending rasters made on the fly.

use crate::{Engine, Page};
use rabbithole_dom::NodeId;
use rabbithole_gfx::{FontContext, Image};
use rabbithole_layout::{DisplayItem, LayoutBox};
use std::borrow::Cow;
use std::sync::Arc;

/// An inline `<svg>` root whose raster waits for the page to be presented.
pub(crate) struct PendingSvg {
    pub node: NodeId,
    /// What `page.images` holds for the node meanwhile: transparent pixels
    /// of the raster's size.
    pub placeholder: Arc<Image>,
    pub tree: rabbithole_svg::SvgTree,
    pub kind: SvgRaster,
}

pub(crate) enum SvgRaster {
    /// [`rabbithole_svg::SvgTree::rasterize`] at this pixel size.
    Fixed(u32, u32),
    /// [`rabbithole_svg::SvgTree::rasterize_viewport`] for a viewport of
    /// this CSS size at this scale.
    Viewport(f32, f32, f32),
}

impl PendingSvg {
    pub(crate) fn new(node: NodeId, tree: rabbithole_svg::SvgTree, kind: SvgRaster) -> PendingSvg {
        let (w, h) = match kind {
            SvgRaster::Fixed(w, h) => (w.max(1), h.max(1)),
            SvgRaster::Viewport(w, h, scale) => ((w * scale).round().max(1.0) as u32, (h * scale).round().max(1.0) as u32),
        };
        // Zeroed allocations are not touched until written.
        let placeholder = Arc::new(Image::new(w, h, vec![0; w as usize * h as usize * 4]));
        PendingSvg { node, placeholder, tree, kind }
    }

    fn raster(&self, fonts: &FontContext) -> Image {
        match self.kind {
            SvgRaster::Fixed(w, h) => self.tree.rasterize(fonts, w, h),
            SvgRaster::Viewport(w, h, scale) => self.tree.rasterize_viewport(fonts, w, h, scale),
        }
    }

    /// The page still shows this placeholder for a rendered element.
    fn shown(&self, page: &Page) -> bool {
        page.images.get(&self.node).is_some_and(|i| Arc::ptr_eq(i, &self.placeholder))
    }

    fn visible(&self, page: &Page) -> bool {
        page.styles.get(self.node).is_some() && page.doc.is_connected(self.node)
    }
}

/// Drop pending rasters whose placeholder the page no longer uses.
pub(crate) fn prune_pending_svgs(page: &mut Page) {
    let Page { images, incr, .. } = page;
    incr.pending_svgs.retain(|p| images.get(&p.node).is_some_and(|i| Arc::ptr_eq(i, &p.placeholder)));
}

impl Engine {
    /// Bring everything painting `page` reads up to date: layout, the
    /// display list, inline SVG rasters and the rasters of child frames
    /// changed since they were last painted (recursively). Embedders call
    /// this before painting a page's display list themselves;
    /// [`Engine::render_damage`] and the other incremental paths call it.
    pub fn present(&self, page: &mut Page) {
        self.flush_display_list(page);
        if !page.incr.pending_svgs.is_empty() {
            self.raster_svgs(page, |_| true);
        }
        if crate::frames::frames_unpainted(page) {
            self.raster_frames(page, false);
        }
    }

    /// Rasterize the pending inline SVGs `wanted` selects (and still
    /// shown), installing the rasters where the placeholders were.
    pub(crate) fn raster_svgs(&self, page: &mut Page, wanted: impl Fn(&PendingSvg) -> bool) {
        let pending = std::mem::take(&mut page.incr.pending_svgs);
        let mut keep = Vec::new();
        let mut swaps: Vec<(Arc<Image>, Arc<Image>)> = Vec::new();
        for p in pending {
            if !p.shown(page) {
                continue;
            }
            if !wanted(&p) || !p.visible(page) {
                keep.push(p);
                continue;
            }
            let img = Arc::new(p.raster(&page.fonts));
            page.images.insert(p.node, img.clone());
            page.incr.image_replaced(p.node, &p.placeholder, &img);
            swaps.push((p.placeholder, img));
        }
        page.incr.pending_svgs = keep;
        if swaps.is_empty() {
            return;
        }
        swap_box_images(&mut page.layout.root, &swaps);
        if contains_images(&page.display_list.items, &swaps) {
            page.incr.display_list_will_change(&page.display_list);
            swap_images(&mut page.display_list.items, &swaps);
        }
    }

    /// The display list painting `page` shows: its own, or, when rasters
    /// wait for [`Engine::present`] (the embedder did not call it), a copy
    /// showing them.
    pub(crate) fn display_list_to_paint<'a>(&self, page: &'a Page) -> Cow<'a, rabbithole_layout::DisplayList> {
        let mut swaps = self.frame_swaps(page);
        for p in &page.incr.pending_svgs {
            if p.shown(page) && p.visible(page) {
                swaps.push((p.placeholder.clone(), Arc::new(p.raster(&page.fonts))));
            }
        }
        if swaps.is_empty() {
            return Cow::Borrowed(&page.display_list);
        }
        let mut list = page.display_list.clone();
        swap_images(&mut list.items, &swaps);
        Cow::Owned(list)
    }
}

/// Replace the images `swaps` names (old, new) in `items`, nested lists
/// included.
pub(crate) fn swap_images(items: &mut [DisplayItem], swaps: &[(Arc<Image>, Arc<Image>)]) {
    for it in items {
        match it {
            DisplayItem::Image { image, .. } | DisplayItem::ImageTiled { image, .. } => {
                if let Some((_, new)) = swaps.iter().find(|(old, _)| Arc::ptr_eq(old, image)) {
                    *image = new.clone();
                }
            }
            DisplayItem::LayerMask(items) | DisplayItem::Filtered { items, .. } | DisplayItem::Blended { items, .. } | DisplayItem::Isolated { items, .. } => swap_images(items, swaps),
            DisplayItem::Transform3d(g) => swap_images(&mut g.items, swaps),
            _ => {}
        }
    }
}

fn contains_images(items: &[DisplayItem], swaps: &[(Arc<Image>, Arc<Image>)]) -> bool {
    items.iter().any(|it| match it {
        DisplayItem::Image { image, .. } | DisplayItem::ImageTiled { image, .. } => swaps.iter().any(|(old, _)| Arc::ptr_eq(old, image)),
        DisplayItem::LayerMask(items) | DisplayItem::Filtered { items, .. } | DisplayItem::Blended { items, .. } | DisplayItem::Isolated { items, .. } => contains_images(items, swaps),
        DisplayItem::Transform3d(g) => contains_images(&g.items, swaps),
        _ => false,
    })
}

/// Replace the images `swaps` names in the boxes of a layout tree.
fn swap_box_images(b: &mut LayoutBox, swaps: &[(Arc<Image>, Arc<Image>)]) {
    if let Some(image) = b.image.as_mut() {
        if let Some((_, new)) = swaps.iter().find(|(old, _)| Arc::ptr_eq(old, image)) {
            *image = new.clone();
        }
    }
    for c in b.children.iter_mut() {
        swap_box_images(c, swaps);
    }
}
