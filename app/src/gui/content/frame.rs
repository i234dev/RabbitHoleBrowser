//! Rasterized page frames sent from content threads to the UI.

use std::sync::Arc;

use rabbithole_engine::gfx::{Canvas, Color, Rect};

/// Device pixels (0x00RRGGBB, row-major). Unlike a `Canvas` this can
/// cross threads.
#[derive(Clone, Default)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u32>,
}

impl Pixels {
    /// Take the pixels of a painted canvas.
    pub fn from_canvas(canvas: &mut Canvas) -> Pixels {
        Pixels { width: canvas.width, height: canvas.height, data: std::mem::take(&mut canvas.data) }
    }
}

/// A painted region of the page, taller than the viewport (overscan) so
/// the UI can scroll it before the content thread repaints.
#[derive(Clone)]
pub struct Frame {
    /// Increases with every frame of a tab.
    pub seq: u64,
    /// Pixels (device px).
    pub canvas: Arc<Pixels>,
    /// Document position (CSS px) of the canvas' top-left corner.
    pub origin: (f32, f32),
    /// Device pixels per CSS pixel.
    pub scale: f32,
    /// Page scroll the frame was painted for (fixed boxes sit there).
    pub scroll: (f32, f32),
    /// Document size (CSS px).
    pub content: (f32, f32),
    /// Page background (fills what the frame does not cover).
    pub background: Color,
    /// Scroll containers (document rect, `position: fixed`): wheel events
    /// over them go to the page instead of scrolling it on the UI side.
    pub scroll_containers: Arc<Vec<(Rect, bool)>>,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame")
            .field("seq", &self.seq)
            .field("size", &(self.canvas.width, self.canvas.height))
            .field("origin", &self.origin)
            .field("scale", &self.scale)
            .field("scroll", &self.scroll)
            .field("content", &self.content)
            .finish()
    }
}

impl Frame {
    /// Document rect (CSS px) covered by the pixels.
    #[allow(dead_code)]
    pub fn covered(&self) -> Rect {
        Rect::new(self.origin.0, self.origin.1, self.canvas.width as f32 / self.scale, self.canvas.height as f32 / self.scale)
    }

    /// Paint the part of the frame visible at `scroll` into `area` of
    /// `dst` (device px): the frame shifted by the scroll difference, the
    /// page background where it has no pixels.
    pub fn present(&self, dst: &mut Canvas, area: Rect, scroll: (f32, f32), scale: f32) {
        dst.fill_rect(area, if self.background.a > 0 { Color { a: 255, ..self.background } } else { Color::WHITE });
        // A zoom change is shown at the old scale until the repaint.
        let s = if (scale - self.scale).abs() < 1e-3 { scale } else { self.scale };
        let x = area.x + ((self.origin.0 - scroll.0) * s).round();
        let y = area.y + ((self.origin.1 - scroll.1) * s).round();
        blit_clipped(dst, &self.canvas, x as i32, y as i32, area);
    }

    /// Is a wheel at viewport point `css` (CSS px) over a scroll container?
    pub fn scroll_container_at(&self, css: (f32, f32), scroll: (f32, f32)) -> bool {
        self.scroll_containers.iter().any(|(r, fixed)| if *fixed { r.contains(css.0, css.1) } else { r.contains(css.0 + scroll.0, css.1 + scroll.1) })
    }
}

/// Copy `src` to `dst` at (x, y), only inside `clip` (device px).
pub fn blit_clipped(dst: &mut Canvas, src: &Pixels, x: i32, y: i32, clip: Rect) {
    let cx0 = (clip.x.floor() as i32).max(0);
    let cy0 = (clip.y.floor() as i32).max(0);
    let cx1 = (clip.right().ceil() as i32).min(dst.width as i32);
    let cy1 = (clip.bottom().ceil() as i32).min(dst.height as i32);
    let x0 = x.max(cx0);
    let y0 = y.max(cy0);
    let x1 = (x + src.width as i32).min(cx1);
    let y1 = (y + src.height as i32).min(cy1);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let sw = src.width as usize;
    let dw = dst.width as usize;
    let n = (x1 - x0) as usize;
    for py in y0..y1 {
        let srow = (py - y) as usize * sw + (x0 - x) as usize;
        let drow = py as usize * dw + x0 as usize;
        dst.data[drow..drow + n].copy_from_slice(&src.data[srow..srow + n]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blit_respects_clip_and_offsets() {
        let mut c = Canvas::new(4, 4);
        c.clear(Color::rgb(255, 0, 0));
        let src = Pixels::from_canvas(&mut c);
        let mut dst = Canvas::new(10, 10);
        dst.clear(Color::rgb(0, 0, 0));
        blit_clipped(&mut dst, &src, -2, 3, Rect::new(0.0, 0.0, 10.0, 5.0));
        let red = Color::rgb(255, 0, 0).to_u32();
        assert_eq!(dst.data[3 * 10], red);
        assert_eq!(dst.data[3 * 10 + 1], red);
        assert_eq!(dst.data[3 * 10 + 2], 0);
        assert_eq!(dst.data[4 * 10], red);
        assert_eq!(dst.data[5 * 10], 0, "clipped below y=5");
    }

    #[test]
    fn present_shifts_by_scroll_difference() {
        let mut c = Pixels { width: 2, height: 100, data: vec![0; 200] };
        for y in 0..100u32 {
            c.data[(y * 2) as usize] = y;
            c.data[(y * 2 + 1) as usize] = y;
        }
        let f = Frame {
            seq: 1,
            canvas: Arc::new(c),
            origin: (0.0, 50.0),
            scale: 1.0,
            scroll: (0.0, 60.0),
            content: (2.0, 1000.0),
            background: Color::WHITE,
            scroll_containers: Arc::new(Vec::new()),
        };
        let mut dst = Canvas::new(2, 20);
        // Scrolled to 70: the frame row 20 (doc y=70) is at the top.
        f.present(&mut dst, Rect::new(0.0, 0.0, 2.0, 20.0), (0.0, 70.0), 1.0);
        assert_eq!(dst.data[0], 20);
        assert_eq!(dst.data[19 * 2], 39);
        // Scrolled past the covered region: background.
        f.present(&mut dst, Rect::new(0.0, 0.0, 2.0, 20.0), (0.0, 140.0), 1.0);
        assert_eq!(dst.data[0], 90);
        assert_eq!(dst.data[19 * 2], Color::WHITE.to_u32());
    }
}
