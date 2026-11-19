//! A from-scratch SVG renderer for RabbitHole.
//!
//! Supports the SVG 1.1 / SVG 2 subset used by icons, logos and charts:
//! structure (`svg`, `g`, `defs`, `symbol`, `use`, `switch`, `a`), shapes,
//! `text`/`tspan`, embedded `image` data URLs, presentation attributes +
//! CSS (inline, internal `<style>`, page cascade), gradients, clip paths,
//! opacity, transforms, `viewBox`/`preserveAspectRatio`.

mod color;
mod paint;
mod parse;
mod style;
pub use style::tokens_to_string;
mod text;
mod units;

pub use color::{named_color, parse_color, ColorSpec};
pub use units::ViewBox;

use parse::Built;
use rabbithole_dom::{Document, NodeId};
use rabbithole_gfx::{Canvas, Color, FontContext, Image, Rect};

/// A parsed, renderable SVG document.
pub struct SvgTree {
    built: Built,
}

impl std::fmt::Debug for SvgTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SvgTree")
            .field("nodes", &self.built.nodes.len())
            .field("width", &self.built.width)
            .field("height", &self.built.height)
            .field("viewbox", &self.built.viewbox)
            .finish()
    }
}

impl SvgTree {
    /// Parse a standalone SVG document (XML text).
    pub fn parse(xml: &str) -> Option<SvgTree> {
        Self::parse_in(xml, None)
    }

    /// Parse a standalone SVG document shown as an image of concrete size
    /// `viewport` (percentages resolve against it when there is no viewBox).
    pub fn parse_in(xml: &str, viewport: Option<(f32, f32)>) -> Option<SvgTree> {
        Self::parse_view(xml, viewport, None)
    }

    /// Like [`SvgTree::parse_in`], showing the `<view>` element whose id is
    /// `fragment` (SVG 2 §8.11: its viewBox / preserveAspectRatio replace
    /// the root's).
    pub fn parse_view(xml: &str, viewport: Option<(f32, f32)>, fragment: Option<&str>) -> Option<SvgTree> {
        // Drop the XML prolog / DOCTYPE; the HTML parser treats the rest as
        // foreign content once the <svg> element starts.
        let start = xml.find("<svg").or_else(|| xml.find("<SVG"))?;
        let body = &xml[start..];
        let doc = rabbithole_html::parse(&format!("<!doctype html><html><body>{body}</body></html>"));
        let root = doc.find_element("svg")?;
        let view = fragment.filter(|f| !f.is_empty()).and_then(|f| {
            doc.descendants(root).into_iter().find(|&n| doc.is_element(n, "view") && doc.attr(n, "id") == Some(f))
        });
        let view_box = view.and_then(|v| doc.attr(v, "viewBox").or_else(|| doc.attr(v, "viewbox"))).and_then(units::parse_viewbox);
        let view_par = view.and_then(|v| doc.attr(v, "preserveAspectRatio").or_else(|| doc.attr(v, "preserveaspectratio")).map(|p| units::parse_aspect_ratio(Some(p))));
        let mut built = parse::build_in(&doc, root, &|_, _| None, viewport.or(view_box.map(|v| (v.w, v.h))))?;
        if let Some(vb) = view_box {
            built.viewbox = Some(vb);
        }
        if let Some(par) = view_par {
            built.par = par;
        }
        Some(SvgTree { built })
    }

    /// Build from an `<svg>` element already in a DOM (inline SVG).
    pub fn from_dom(doc: &Document, root: NodeId, style_of: &dyn Fn(NodeId, &str) -> Option<String>) -> Option<SvgTree> {
        parse::build(doc, root, style_of).map(|built| SvgTree { built })
    }

    /// `width`/`height` attributes in px (percentages → None) and `viewBox`.
    pub fn intrinsic_size(&self) -> (Option<f32>, Option<f32>, Option<ViewBox>) {
        (self.built.width, self.built.height, self.built.viewbox)
    }

    /// Size to use when laid out with the given available dimensions (CSS
    /// replaced-element sizing using the intrinsic ratio from `viewBox`).
    pub fn resolve_size(&self, width: Option<f32>, height: Option<f32>) -> (f32, f32) {
        let (iw, ih, vb) = self.intrinsic_size();
        let ratio = match (iw, ih) {
            (Some(w), Some(h)) if h > 0.0 => Some(w / h),
            _ => vb.map(|v| v.w / v.h),
        };
        match (width, height) {
            (Some(w), Some(h)) => (w, h),
            (Some(w), None) => (w, match ratio {
                Some(r) if r > 0.0 => w / r,
                _ => ih.unwrap_or(150.0),
            }),
            (None, Some(h)) => (match ratio {
                Some(r) => h * r,
                None => iw.unwrap_or(300.0),
            }, h),
            (None, None) => match (iw, ih) {
                (Some(w), Some(h)) => (w, h),
                (Some(w), None) => (w, ratio.map(|r| w / r).unwrap_or(150.0)),
                (None, Some(h)) => (ratio.map(|r| h * r).unwrap_or(300.0), h),
                (None, None) => match ratio {
                    Some(r) if r > 0.0 => (300.0, 300.0 / r),
                    _ => (300.0, 150.0),
                },
            },
        }
    }

    /// Number of renderable nodes (diagnostics).
    pub fn node_count(&self) -> usize {
        self.built.nodes.len()
    }

    /// The outline of the `<clipPath>` with this `id` (for `clip-path:
    /// url(#id)` on CSS boxes) and its clip rule.
    ///
    /// `bbox` is the referencing box: `objectBoundingBox` units map onto
    /// it and user units start at its origin.
    /// `transform` (CSS `transform` of the `<clipPath>`) replaces its
    /// `transform` attribute.
    pub fn clip_outline(&self, id: &str, bbox: Rect, transform: Option<rabbithole_gfx::Matrix>) -> Option<(Vec<rabbithole_gfx::PathSeg>, rabbithole_gfx::FillRule)> {
        let (path, rule, units_bbox) = paint::clip_outline(&self.built, id, transform)?;
        let m = if units_bbox { [bbox.w, 0.0, 0.0, bbox.h, bbox.x, bbox.y] } else { [1.0, 0.0, 0.0, 1.0, bbox.x, bbox.y] };
        Some((paint::transform_path(&path, &m), rule))
    }

    /// Draw into `canvas` filling `dest` (applies viewBox + preserveAspectRatio).
    pub fn render(&self, canvas: &mut Canvas, fonts: &FontContext, dest: Rect) {
        self.render_mode(canvas, fonts, dest, false);
    }

    /// Draw as an image whose concrete size is `dest` (SVG as a CSS image:
    /// the size replaces `width`/`height`, so `dest` is the viewport and
    /// content without a `viewBox` is not scaled).
    pub fn render_viewport(&self, canvas: &mut Canvas, fonts: &FontContext, dest: Rect) {
        self.render_mode(canvas, fonts, dest, true);
    }

    fn render_mode(&self, canvas: &mut Canvas, fonts: &FontContext, dest: Rect, viewport: bool) {
        if dest.w <= 0.0 || dest.h <= 0.0 {
            return;
        }
        let b = &self.built;
        let (vw, vh) = match b.viewbox {
            Some(v) => (v.w, v.h),
            None if viewport => (dest.w, dest.h),
            None => (b.width.unwrap_or(dest.w), b.height.unwrap_or(dest.h)),
        };
        let m = match b.viewbox {
            Some(vb) => units::viewbox_transform(vb, dest, b.par),
            None if viewport => [1.0, 0.0, 0.0, 1.0, dest.x, dest.y],
            None => {
                // No viewBox: user units are px; scale the intrinsic size to dest.
                let sx = if let Some(w) = b.width { dest.w / w } else { 1.0 };
                let sy = if let Some(h) = b.height { dest.h / h } else { 1.0 };
                [sx, 0.0, 0.0, sy, dest.x, dest.y]
            }
        };
        if !b.overflow_visible {
            canvas.push_clip(dest);
        }
        canvas.push_transform(m);
        {
            let mut painter = paint::Painter::new(b, canvas, fonts, (vw, vh));
            painter.paint_root();
        }
        canvas.pop_transform();
        if !b.overflow_visible {
            canvas.pop_clip();
        }
    }

    /// Rasterize to an RGBA image with a transparent background.
    pub fn rasterize(&self, fonts: &FontContext, width: u32, height: u32) -> Image {
        self.rasterize_mode(fonts, width, height, None)
    }

    /// Rasterize as an image of concrete size `width`×`height` (see
    /// [`SvgTree::render_viewport`]).
    /// `scale` maps the CSS size to the pixel size.
    pub fn rasterize_viewport(&self, fonts: &FontContext, width: f32, height: f32, scale: f32) -> Image {
        let pw = (width * scale).round().max(1.0) as u32;
        let ph = (height * scale).round().max(1.0) as u32;
        self.rasterize_mode(fonts, pw, ph, Some((width, height)))
    }

    /// `viewport`: render as an image of that CSS size scaled to the pixels.
    fn rasterize_mode(&self, fonts: &FontContext, width: u32, height: u32, viewport: Option<(f32, f32)>) -> Image {
        let width = width.max(1);
        let height = height.max(1);
        let dest = Rect::new(0.0, 0.0, width as f32, height as f32);
        let mut on_black = Canvas::new(width, height);
        on_black.clear(Color::BLACK);
        match viewport {
            Some((vw, vh)) => {
                on_black.push_transform([width as f32 / vw.max(1e-3), 0.0, 0.0, height as f32 / vh.max(1e-3), 0.0, 0.0]);
                self.render_mode(&mut on_black, fonts, Rect::new(0.0, 0.0, vw, vh), true);
                on_black.pop_transform();
            }
            None => self.render_mode(&mut on_black, fonts, dest, false),
        }
        let mut on_white = Canvas::new(width, height);
        on_white.clear(Color::WHITE);
        match viewport {
            Some((vw, vh)) => {
                on_white.push_transform([width as f32 / vw.max(1e-3), 0.0, 0.0, height as f32 / vh.max(1e-3), 0.0, 0.0]);
                self.render_mode(&mut on_white, fonts, Rect::new(0.0, 0.0, vw, vh), true);
                on_white.pop_transform();
            }
            None => self.render_mode(&mut on_white, fonts, dest, false),
        }
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for (pb, pw) in on_black.data.iter().zip(on_white.data.iter()) {
            let (rb, gb, bb) = ((pb >> 16) & 0xff, (pb >> 8) & 0xff, pb & 0xff);
            let (rw, gw, bw) = ((pw >> 16) & 0xff, (pw >> 8) & 0xff, pw & 0xff);
            // alpha = 1 - (white - black) / 255, averaged over channels.
            let diff = (rw as i32 - rb as i32) + (gw as i32 - gb as i32) + (bw as i32 - bb as i32);
            let alpha = (255 - (diff / 3).clamp(0, 255)) as u32;
            if alpha == 0 {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            // Un-premultiply the black-background sample.
            let un = |c: u32| ((c * 255 + alpha / 2) / alpha).min(255) as u8;
            rgba.extend_from_slice(&[un(rb), un(gb), un(bb), alpha as u8]);
        }
        Image::new(width, height, rgba)
    }
}

#[cfg(test)]
mod tests;
