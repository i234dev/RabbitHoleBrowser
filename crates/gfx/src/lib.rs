//! Software graphics: an RGB canvas with a scanline/coverage rasterizer,
//! font loading + glyph rasterization (via `fontdue`), and image decoding.
//! No GPU, no system toolkit; the same code paints on every platform.

pub mod avif;
pub mod canvas;
pub mod canvas_ext;
pub mod diff;
pub mod font;
pub mod font_ext;
pub mod image;
pub mod paint;
pub mod path;
pub mod raster;
pub mod stroke;
mod shaping;
pub mod color_glyph;
pub use shaping::{DecorationMetrics, ShapeFeatures};

#[cfg(test)]
mod tests_v02;

pub use canvas::{Canvas, Color, ColorStop, FillRule, LineCap, LineJoin, Matrix, Paint, PathSeg, Point, Radii, Rect, StrokeStyle, IDENTITY};
pub use canvas_ext::{ellipse_path, parse_svg_path, rounded_rect_path};
pub use font::{FaceInfo, Family, FontContext, FontId, GlyphBitmap, LineMetrics, ShapedGlyph};
pub use image::Image;
