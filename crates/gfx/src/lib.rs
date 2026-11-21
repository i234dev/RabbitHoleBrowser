//! Software graphics: an RGB canvas with a scanline/coverage rasterizer,
//! font loading + glyph rasterization (via `fontdue`), and image decoding.
//! No GPU, no system toolkit; the same code paints on every platform.

pub mod avif;
pub mod canvas;
pub mod canvas_ext;
pub mod diff;
pub mod font;
pub mod fxhash;
pub mod font_ext;
mod glyph_raster;
pub mod image;
pub mod paint;
pub mod mat4;
pub mod path;
pub mod raster;
pub mod stroke;
mod shaping;
pub mod color_glyph;
pub use shaping::{DecorationMetrics, FontFeature, MetricOverrides, ShapeFeatures, LANGUAGE_TAG};

#[cfg(test)]
mod tests_v02;
#[cfg(test)]
mod tests_font_scopes;
#[cfg(test)]
mod tests_font_ranges;
#[cfg(test)]
mod tests_line_metrics;

pub use canvas::{Canvas, Color, ColorStop, FillRule, LineCap, LineJoin, Matrix, Paint, PathSeg, Point, Radii, Rect, StrokeStyle, IDENTITY};
pub use canvas_ext::{blend_mode_index, blend_pixel, ellipse_path, parse_svg_path, rounded_rect_path};
pub use font::{FaceInfo, Family, FontContext, FontId, FontLimits, GlyphBitmap, LineMetrics, ShapedGlyph};
pub use image::Image;
