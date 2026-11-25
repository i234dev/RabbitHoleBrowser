//! Presenting a page: rasters that only painting reads are made when a
//! frame is shown, not while scripts run.
//!
//! * Child frames changed by scripts are marked unpainted and rasterized
//!   by [`Engine::present`] (see `frames.rs`).
//!
//! Painting paths that only borrow the page ([`Engine::render`]) paint a
//! copy of the display list with the pending rasters made on the fly.

use crate::{Engine, Page};
use rabbithole_gfx::Image;
use rabbithole_layout::DisplayItem;
use std::borrow::Cow;
use std::sync::Arc;

impl Engine {
    /// Bring everything painting `page` reads up to date: layout, the
    /// display list and the rasters of child frames
    /// changed since they were last painted (recursively). Embedders call
    /// this before painting a page's display list themselves;
    /// [`Engine::render_damage`] and the other incremental paths call it.
    pub fn present(&self, page: &mut Page) {
        self.flush_display_list(page);
        if crate::frames::frames_unpainted(page) {
            self.raster_frames(page, false);
        }
    }

    /// The display list painting `page` shows: its own, or, when rasters
    /// wait for [`Engine::present`] (the embedder did not call it), a copy
    /// showing them.
    pub(crate) fn display_list_to_paint<'a>(&self, page: &'a Page) -> Cow<'a, rabbithole_layout::DisplayList> {
        let swaps = self.frame_swaps(page);
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
            _ => {}
        }
    }
}
