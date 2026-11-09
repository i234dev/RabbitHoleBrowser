//! Software graphics: an RGB canvas with a scanline/coverage rasterizer,
//! font loading + glyph rasterization (via `fontdue`), and image decoding.
//! No GPU, no system toolkit; the same code paints on every platform.

pub mod canvas;
pub mod font;
pub mod image;

pub use canvas::{Canvas, Color, Rect};
pub use font::{Family, FontContext, FontId, GlyphBitmap, LineMetrics, ShapedGlyph};
pub use image::Image;
