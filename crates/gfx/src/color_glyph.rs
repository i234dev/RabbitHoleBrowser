//! Color glyphs (emoji): COLR v0 layered glyphs with CPAL palettes
//! (Segoe UI Emoji, Twemoji COLR builds) and embedded PNG bitmaps from
//! `CBDT` (Noto Color Emoji) and `sbix` (Apple Color Emoji) strikes.

use crate::font::{FontContext, FontId};
use crate::image::Image;
use rustybuzz::ttf_parser::{self, GlyphId, Tag};
use std::rc::Rc;
use std::sync::Arc;

/// A rendered color glyph and where it goes relative to the glyph origin
/// on the baseline (px; `top` is the distance above the baseline).
#[derive(Debug)]
pub struct ColorGlyph {
    pub image: Arc<Image>,
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

fn be16(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(at)?, *d.get(at + 1)?]))
}

fn be32(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes([*d.get(at)?, *d.get(at + 1)?, *d.get(at + 2)?, *d.get(at + 3)?]))
}

/// COLR v0 layers of `glyph` as (layer glyph, RGBA) with palette 0;
/// `None` color means the text color.
fn colr_layers(face: &ttf_parser::Face, glyph: u16) -> Option<Vec<(u16, Option<[u8; 4]>)>> {
    let colr = face.raw_face().table(Tag::from_bytes(b"COLR"))?;
    let cpal = face.raw_face().table(Tag::from_bytes(b"CPAL"))?;
    let num_base = be16(colr, 2)? as usize;
    let base_off = be32(colr, 4)? as usize;
    let layer_off = be32(colr, 8)? as usize;
    // Binary search the base glyph records (6 bytes each, sorted).
    let (mut lo, mut hi) = (0usize, num_base);
    let mut found = None;
    while lo < hi {
        let mid = (lo + hi) / 2;
        let rec = base_off + mid * 6;
        let g = be16(colr, rec)?;
        match g.cmp(&glyph) {
            std::cmp::Ordering::Less => lo = mid + 1,
            std::cmp::Ordering::Greater => hi = mid,
            std::cmp::Ordering::Equal => {
                found = Some((be16(colr, rec + 2)? as usize, be16(colr, rec + 4)? as usize));
                break;
            }
        }
    }
    let (first, count) = found?;
    let num_entries = be16(cpal, 2)? as usize;
    let colors_off = be32(cpal, 8)? as usize;
    let first_color = be16(cpal, 12)? as usize; // palette 0
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let rec = layer_off + (first + i) * 4;
        let g = be16(colr, rec)?;
        let pi = be16(colr, rec + 2)?;
        let color = if pi == 0xFFFF || pi as usize >= num_entries {
            None
        } else {
            let c = colors_off + (first_color + pi as usize) * 4;
            // BGRA.
            Some([*cpal.get(c + 2)?, *cpal.get(c + 1)?, *cpal.get(c)?, *cpal.get(c + 3)?])
        };
        out.push((g, color));
    }
    Some(out)
}

impl FontContext {
    /// The color rendering of `glyph` at `size`, if its face has one
    /// (cached).
    pub fn color_glyph(&self, font: FontId, glyph: u16, size: f32) -> Option<Rc<ColorGlyph>> {
        let idx = self.resolve_index(font.0);
        if !self.has_color_tables(idx) || glyph == crate::font::ZERO_WIDTH_GLYPH {
            return None;
        }
        let key = (idx, glyph, (size * 4.0).round() as u32);
        if let Some(c) = self.color_cache.borrow().get(&key) {
            return c.clone();
        }
        let made = self.make_color_glyph(idx, glyph, size).map(Rc::new);
        self.color_cache.borrow_mut().insert(key, made.clone());
        made
    }

    fn make_color_glyph(&self, idx: usize, glyph: u16, size: f32) -> Option<ColorGlyph> {
        let data = self.face_bytes(idx)?;
        let face = ttf_parser::Face::parse(&data, 0).ok()?;
        if let Some(layers) = colr_layers(&face, glyph) {
            // Composite the layer outlines.
            let bitmaps: Vec<_> = layers.iter().map(|(g, c)| (self.rasterize(FontId(idx), *g, size), *c)).collect();
            let x0 = bitmaps.iter().filter(|(b, _)| !b.is_empty()).map(|(b, _)| b.left).min()?;
            let x1 = bitmaps.iter().filter(|(b, _)| !b.is_empty()).map(|(b, _)| b.left + b.width as i32).max()?;
            let top = bitmaps.iter().filter(|(b, _)| !b.is_empty()).map(|(b, _)| b.top).max()?;
            let bottom = bitmaps.iter().filter(|(b, _)| !b.is_empty()).map(|(b, _)| b.top - b.height as i32).min()?;
            let (w, h) = ((x1 - x0).max(1) as usize, (top - bottom).max(1) as usize);
            // Premultiplied accumulation, converted to straight alpha.
            let mut acc = vec![0f32; w * h * 4];
            for (b, c) in &bitmaps {
                let [r, g, bl, a] = c.unwrap_or([0, 0, 0, 255]);
                let a = a as f32 / 255.0;
                for y in 0..b.height as usize {
                    for x in 0..b.width as usize {
                        let cov = b.coverage[y * b.width as usize + x] as f32 / 255.0 * a;
                        if cov <= 0.0 {
                            continue;
                        }
                        let px = (b.left - x0) as usize + x;
                        let py = (top - b.top) as usize + y;
                        if px >= w || py >= h {
                            continue;
                        }
                        let o = (py * w + px) * 4;
                        for (k, v) in [r, g, bl].iter().enumerate() {
                            acc[o + k] = *v as f32 / 255.0 * cov + acc[o + k] * (1.0 - cov);
                        }
                        acc[o + 3] = cov + acc[o + 3] * (1.0 - cov);
                    }
                }
            }
            let mut rgba = vec![0u8; w * h * 4];
            for i in 0..w * h {
                let a = acc[i * 4 + 3];
                if a > 0.0 {
                    for k in 0..3 {
                        rgba[i * 4 + k] = (acc[i * 4 + k] / a * 255.0).round().clamp(0.0, 255.0) as u8;
                    }
                    rgba[i * 4 + 3] = (a * 255.0).round() as u8;
                }
            }
            let image = Arc::new(Image::new(w as u32, h as u32, rgba));
            return Some(ColorGlyph { image, left: x0 as f32, top: top as f32, width: w as f32, height: h as f32 });
        }
        // Bitmap strikes (PNG).
        let ppem = size.round().clamp(1.0, u16::MAX as f32) as u16;
        let img = face.glyph_raster_image(GlyphId(glyph), ppem)?;
        if img.format != ttf_parser::RasterImageFormat::PNG {
            return None;
        }
        let decoded = image::load_from_memory_with_format(img.data, image::ImageFormat::Png).ok()?.to_rgba8();
        let (iw, ih) = (decoded.width(), decoded.height());
        let scale = size / img.pixels_per_em.max(1) as f32;
        let image = Arc::new(Image::new(iw, ih, decoded.into_raw()));
        Some(ColorGlyph {
            image,
            left: img.x as f32 * scale,
            top: (img.y as f32 + ih as f32) * scale,
            width: iw as f32 * scale,
            height: ih as f32 * scale,
        })
    }
}
