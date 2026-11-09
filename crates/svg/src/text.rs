//! `<text>` content model: positioned chunks.

use crate::style::Specified;
use rabbithole_gfx::Rect;

/// A run of characters with its own position/style (from `<tspan>`s).
#[derive(Debug, Clone)]
pub struct Chunk {
    /// Absolute x/y when specified (starts a new anchored run).
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub dx: f32,
    pub dy: f32,
    pub text: String,
    pub spec: Specified,
}

#[derive(Debug, Clone)]
pub struct TextNode {
    pub chunks: Vec<Chunk>,
}

impl TextNode {
    /// Rough bounding box (0.6em per character) for layer bounds.
    pub fn approx_bbox(&self) -> Option<Rect> {
        let mut x = 0.0f32;
        let mut y = 0.0f32;
        let mut bbox: Option<Rect> = None;
        for c in &self.chunks {
            if let Some(cx) = c.x {
                x = cx;
            }
            if let Some(cy) = c.y {
                y = cy;
            }
            x += c.dx;
            y += c.dy;
            let size = match c.spec.font_size {
                Some(crate::units::Length::Px(v)) => v,
                _ => 16.0,
            };
            let w = c.text.chars().count() as f32 * size * 0.6;
            let r = Rect::new(x - w, y - size, w * 3.0, size * 1.5);
            bbox = Some(match bbox {
                None => r,
                Some(b) => b.union(&r),
            });
            x += w;
        }
        bbox
    }
}
