//! CSS Backgrounds 3 §6: `border-image` — nine-slice drawing of an image
//! (raster, vector or gradient) over the border image area.

use crate::background::{concrete_size, gradient_paint, BoxGeometry};
use crate::display_list::{DisplayItem, DisplayList};
use rabbithole_gfx::{Image, Rect};
use rabbithole_style::{BackgroundImage, BorderImage, BorderImageRepeat, BorderImageWidth, Length};
use std::collections::HashMap;
use std::sync::Arc;

/// At most this many tiles per region (beyond it the region stretches).
const MAX_TILES: usize = 2048;

enum Source<'a> {
    Raster(Arc<Image>),
    Gradient(&'a BackgroundImage),
}

/// Paint the border image of a box. Returns false when there is nothing to
/// draw (no source, or it is not loaded): the border styles paint instead.
pub(crate) fn paint_border_image(list: &mut DisplayList, geom: &BoxGeometry, bi: &BorderImage, images: &HashMap<String, Arc<Image>>, pixelated: bool) -> bool {
    let source = match &bi.source {
        BackgroundImage::None => return false,
        BackgroundImage::Url(u) => match images.get(u) {
            Some(img) if !img.broken => Source::Raster(img.clone()),
            _ => return false,
        },
        g => Source::Gradient(g),
    };
    let bw = geom.border_width;
    let widths = [bw.top, bw.right, bw.bottom, bw.left];
    // The border image area: the border box grown by the outsets.
    let out: [f32; 4] = std::array::from_fn(|i| match bi.outset[i] {
        BorderImageWidth::Number(n) => n * widths[i],
        BorderImageWidth::Length(l) => l.resolve_or_zero(0.0).max(0.0),
        BorderImageWidth::Auto => 0.0,
    });
    let b = geom.border;
    let area = Rect::new(b.x - out[3], b.y - out[0], b.w + out[1] + out[3], b.h + out[0] + out[2]);
    if area.w <= 0.0 || area.h <= 0.0 {
        return true;
    }
    // The image's concrete size, with the area as the default object size.
    let (source, (iw, ih)) = match source {
        Source::Raster(img) => {
            let size = concrete_size((None, None), img.natural_dimensions(), (area.w, area.h));
            let img = match &img.vector {
                Some(v) => v.at_size(size.0, size.1, 1.0).unwrap_or(img),
                None => img,
            };
            (Source::Raster(img), size)
        }
        g => (g, (area.w, area.h)),
    };
    if !(iw > 0.0 && ih > 0.0) {
        return true;
    }
    // Slices in image px: top, right, bottom, left.
    let slice: [f32; 4] = std::array::from_fn(|i| {
        let along = if i % 2 == 0 { ih } else { iw };
        match bi.slice[i] {
            Length::Percent(p) => p / 100.0 * along,
            Length::Px(v) => v,
            other => other.resolve_or_zero(along),
        }
        .clamp(0.0, along)
    });
    // Border image widths, scaled down together when opposite sides overlap.
    let mut w: [f32; 4] = std::array::from_fn(|i| match bi.width[i] {
        BorderImageWidth::Number(n) => n * widths[i],
        BorderImageWidth::Length(l) => l.resolve_or_zero(if i % 2 == 0 { area.h } else { area.w }).max(0.0),
        BorderImageWidth::Auto => slice[i],
    });
    let f = (area.w / (w[1] + w[3])).min(area.h / (w[0] + w[2]));
    if f < 1.0 {
        for v in &mut w {
            *v *= f;
        }
    }
    let [st, sr, sb, sl] = slice;
    let [wt, wr, wb, wl] = w;
    let (sx0, sx1, sy0, sy1) = (sl, (iw - sr).max(sl), st, (ih - sb).max(st));
    let (dx0, dx1, dy0, dy1) = (area.x + wl, (area.right() - wr).max(area.x + wl), area.y + wt, (area.bottom() - wb).max(area.y + wt));
    let src_cols = [(0.0, sl), (sx0, sx1 - sx0), (sx1, iw - sx1)];
    let src_rows = [(0.0, st), (sy0, sy1 - sy0), (sy1, ih - sy1)];
    let dst_cols = [(area.x, wl), (dx0, dx1 - dx0), (dx1, area.right() - dx1)];
    let dst_rows = [(area.y, wt), (dy0, dy1 - dy0), (dy1, area.bottom() - dy1)];
    // Scale of the edge pieces: the side width over the slice.
    let ratio = |d: f32, s: f32| if s > 0.0 && d > 0.0 { Some(d / s) } else { None };
    let (top_s, bottom_s) = (ratio(wt, st), ratio(wb, sb));
    let (left_s, right_s) = (ratio(wl, sl), ratio(wr, sr));
    let mut draw = Drawer { list, source: &source, iw, ih, pixelated };
    for row in 0..3 {
        for col in 0..3 {
            if row == 1 && col == 1 && !bi.fill {
                continue;
            }
            let (sx, sw) = src_cols[col];
            let (sy, sh) = src_rows[row];
            let (dx, dw) = dst_cols[col];
            let (dy, dh) = dst_rows[row];
            if sw <= 0.0 || sh <= 0.0 || dw <= 0.0 || dh <= 0.0 {
                continue;
            }
            let src = Rect::new(sx, sy, sw, sh);
            let dst = Rect::new(dx, dy, dw, dh);
            if row != 1 && col != 1 {
                // Corners stretch.
                draw.piece(src, dst);
                continue;
            }
            // Tile size: edges keep their side's scale across the edge;
            // the middle takes the top/bottom and left/right scales.
            let hscale = match row {
                0 => top_s,
                2 => bottom_s,
                _ => top_s.or(bottom_s),
            };
            let vscale = match col {
                0 => left_s,
                2 => right_s,
                _ => left_s.or(right_s),
            };
            let tile_w = if col == 1 { hscale.map(|s| sw * s).unwrap_or(sw) } else { dw };
            let tile_h = if row == 1 { vscale.map(|s| sh * s).unwrap_or(sh) } else { dh };
            let xs = if col == 1 { tiles(bi.repeat.0, dx, dw, tile_w) } else { Some((vec![dx], dw)) };
            let ys = if row == 1 { tiles(bi.repeat.1, dy, dh, tile_h) } else { Some((vec![dy], dh)) };
            let (Some((xs, tw)), Some((ys, th))) = (xs, ys) else { continue };
            if xs.len() * ys.len() > MAX_TILES {
                draw.piece(src, dst);
                continue;
            }
            draw.list.items.push(DisplayItem::PushClip(dst));
            for &y in &ys {
                for &x in &xs {
                    draw.piece(src, Rect::new(x, y, tw, th));
                }
            }
            draw.list.items.push(DisplayItem::PopClip);
        }
    }
    true
}

/// Tile positions along an axis of a region (`start`, `len`) and the tile
/// length after `round`. None: nothing to draw.
fn tiles(mode: BorderImageRepeat, start: f32, len: f32, tile: f32) -> Option<(Vec<f32>, f32)> {
    if !(tile > 0.0 && len > 0.0) || !tile.is_finite() {
        return None;
    }
    match mode {
        BorderImageRepeat::Stretch => Some((vec![start], len)),
        BorderImageRepeat::Round => {
            let n = (len / tile).round().max(1.0);
            let t = len / n;
            Some(((0..n as usize).map(|k| start + k as f32 * t).collect(), t))
        }
        BorderImageRepeat::Repeat => {
            // A tile is centred in the region.
            if len / tile > MAX_TILES as f32 {
                return Some((vec![start], len));
            }
            let center = start + len / 2.0;
            let mut first = center - tile / 2.0;
            while first > start {
                first -= tile;
            }
            let mut v = Vec::new();
            let mut x = first;
            while x < start + len {
                v.push(x);
                x += tile;
            }
            Some((v, tile))
        }
        BorderImageRepeat::Space => {
            let n = (len / tile + 1e-4).floor();
            if n < 1.0 {
                return None;
            }
            if n > MAX_TILES as f32 {
                return Some((vec![start], len));
            }
            let gap = (len - n * tile) / (n + 1.0);
            Some(((0..n as usize).map(|k| start + gap + k as f32 * (tile + gap)).collect(), tile))
        }
    }
}

struct Drawer<'a, 'b> {
    list: &'a mut DisplayList,
    source: &'b Source<'b>,
    /// Concrete image size (CSS px).
    iw: f32,
    ih: f32,
    pixelated: bool,
}

impl Drawer<'_, '_> {
    /// Draw the image region `src` (CSS px of the image) into `dst`.
    fn piece(&mut self, src: Rect, dst: Rect) {
        if dst.w <= 0.0 || dst.h <= 0.0 || src.w <= 0.0 || src.h <= 0.0 {
            return;
        }
        match self.source {
            Source::Raster(img) => {
                let (fx, fy) = (img.width as f32 / self.iw, img.height as f32 / self.ih);
                self.list.items.push(DisplayItem::Image {
                    rect: dst,
                    src: Rect::new(src.x * fx, src.y * fy, src.w * fx, src.h * fy),
                    image: img.clone(),
                    opacity: 1.0,
                    pixelated: self.pixelated,
                });
            }
            Source::Gradient(g) => {
                let Some(paint) = gradient_paint(g, Rect::new(0.0, 0.0, self.iw, self.ih)) else { return };
                let (sx, sy) = (dst.w / src.w, dst.h / src.h);
                self.list.items.push(DisplayItem::PushClip(dst));
                self.list.items.push(DisplayItem::PushTransform([sx, 0.0, 0.0, sy, dst.x - src.x * sx, dst.y - src.y * sy]));
                self.list.items.push(DisplayItem::RectRadii { rect: src, radii: [(0.0, 0.0); 4], paint });
                self.list.items.push(DisplayItem::PopTransform);
                self.list.items.push(DisplayItem::PopClip);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_modes_place_tiles() {
        assert_eq!(tiles(BorderImageRepeat::Stretch, 10.0, 50.0, 7.0), Some((vec![10.0], 50.0)));
        // Round: 50 / 20 → 2 tiles of 25.
        assert_eq!(tiles(BorderImageRepeat::Round, 0.0, 50.0, 20.0), Some((vec![0.0, 25.0], 25.0)));
        // Space: 2 tiles of 20 with gaps of 10/3.
        let (v, t) = tiles(BorderImageRepeat::Space, 0.0, 50.0, 20.0).unwrap();
        assert_eq!(t, 20.0);
        assert!((v[0] - 10.0 / 3.0).abs() < 1e-4 && (v[1] - (20.0 + 20.0 / 3.0)).abs() < 1e-4);
        // Repeat: centred tile at 15..35, neighbours cover the rest.
        let (v, _) = tiles(BorderImageRepeat::Repeat, 0.0, 50.0, 20.0).unwrap();
        assert_eq!(v, vec![-5.0, 15.0, 35.0]);
        assert_eq!(tiles(BorderImageRepeat::Space, 0.0, 10.0, 20.0), None);
    }
}
