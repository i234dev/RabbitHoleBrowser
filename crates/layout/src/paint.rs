//! Display list rasterization.

use crate::display_list::DisplayList;
use rabbithole_gfx::{Canvas, FontContext};

pub fn paint(list: &DisplayList, canvas: &mut Canvas, fonts: &FontContext, scroll_x: f32, scroll_y: f32) {
    let _ = (list, canvas, fonts, scroll_x, scroll_y);
    unimplemented!()
}
