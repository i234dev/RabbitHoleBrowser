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
        // Drop the XML prolog / DOCTYPE; the HTML parser treats the rest as
        // foreign content once the <svg> element starts.
        let start = xml.find("<svg").or_else(|| xml.find("<SVG"))?;
        let body = &xml[start..];
        let doc = rabbithole_html::parse(&format!("<!doctype html><html><body>{body}</body></html>"));
        let root = doc.find_element("svg")?;
        Self::from_dom(&doc, root, &|_, _| None)
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

    /// Draw into `canvas` filling `dest` (applies viewBox + preserveAspectRatio).
    pub fn render(&self, canvas: &mut Canvas, fonts: &FontContext, dest: Rect) {
        if dest.w <= 0.0 || dest.h <= 0.0 {
            return;
        }
        let b = &self.built;
        let (vw, vh) = match b.viewbox {
            Some(v) => (v.w, v.h),
            None => (b.width.unwrap_or(dest.w), b.height.unwrap_or(dest.h)),
        };
        let m = match b.viewbox {
            Some(vb) => units::viewbox_transform(vb, dest, b.par),
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
        let width = width.max(1);
        let height = height.max(1);
        let dest = Rect::new(0.0, 0.0, width as f32, height as f32);
        let mut on_black = Canvas::new(width, height);
        on_black.clear(Color::BLACK);
        self.render(&mut on_black, fonts, dest);
        let mut on_white = Canvas::new(width, height);
        on_white.clear(Color::WHITE);
        self.render(&mut on_white, fonts, dest);
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
