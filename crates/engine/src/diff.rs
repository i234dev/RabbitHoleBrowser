//! Rendering pages to frames and comparing frames pixel by pixel.
//!
//! The building block for reftests (a test page against its reference
//! page or PNG, e.g. Acid2), comparisons with screenshots from other
//! browsers and the agent interface: snapshot before and after an action
//! and ask [`changed_regions`] whether — and where — the page changed.

use crate::{Engine, EngineError, Page, Viewport};
use rabbithole_gfx::Canvas;
pub use rabbithole_gfx::diff::{DiffOptions, DiffResult, FrameRef, Region};
use std::time::{Duration, Instant};

/// An opaque RGBA8 image of (part of) a rendered page.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl Frame {
    pub fn view(&self) -> FrameRef<'_> {
        FrameRef::new(&self.rgba, self.width, self.height)
    }

    /// Decode a PNG (or any raster format `image` reads) into a frame.
    pub fn from_image_bytes(bytes: &[u8]) -> Result<Frame, String> {
        let (rgba, width, height) = rabbithole_gfx::diff::decode_rgba(bytes)?;
        Ok(Frame { rgba, width, height })
    }

    pub fn load_png(path: &std::path::Path) -> Result<Frame, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Frame::from_image_bytes(&bytes)
    }

    pub fn save_png(&self, path: &std::path::Path) -> Result<(), String> {
        let png = rabbithole_gfx::diff::encode_png(&self.rgba, self.width, self.height)?;
        std::fs::write(path, png).map_err(|e| format!("cannot write {}: {e}", path.display()))
    }

    /// The `w`×`h` sub-image at (x, y) (clipped to the frame).
    pub fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> Frame {
        let x = x.min(self.width);
        let y = y.min(self.height);
        let w = w.min(self.width - x);
        let h = h.min(self.height - y);
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for row in y..y + h {
            let s = ((row * self.width + x) * 4) as usize;
            rgba.extend_from_slice(&self.rgba[s..s + (w * 4) as usize]);
        }
        Frame { rgba, width: w, height: h }
    }
}

/// Compare two frames.
pub fn diff_frames(a: &Frame, b: &Frame, opts: &DiffOptions) -> DiffResult {
    rabbithole_gfx::diff::diff(a.view(), b.view(), opts)
}

/// Compare two frames and also produce a visual diff (differences in red
/// over a faded copy of `a`, changed regions outlined).
pub fn diff_frames_image(a: &Frame, b: &Frame, opts: &DiffOptions) -> (DiffResult, Frame) {
    let (r, rgba) = rabbithole_gfx::diff::diff_image(a.view(), b.view(), opts);
    let (width, height) = (r.width, r.height);
    (r, Frame { rgba, width, height })
}

/// Areas that differ between two frames of the same page (e.g. before and
/// after a click), in frame pixel coordinates. Empty when nothing changed.
pub fn changed_regions(before: &Frame, after: &Frame) -> Vec<Region> {
    rabbithole_gfx::diff::changed_regions(before.view(), after.view(), &DiffOptions::default())
}

impl Engine {
    /// Paint the page's viewport at its current scroll position.
    pub fn snapshot(&self, page: &Page) -> Frame {
        let (w, h) = (page.viewport.width.max(1.0) as u32, page.viewport.height.max(1.0) as u32);
        let mut canvas = Canvas::new(w, h);
        self.render(page, &mut canvas, page.page_scroll.0, page.page_scroll.1);
        Frame { rgba: canvas.to_rgba8(), width: w, height: h }
    }

    /// Let timers and image loads settle (bounded by `max_wait`), like a
    /// browser would before a screenshot.
    pub fn settle_for_snapshot(&self, page: &mut Page, max_wait: Duration) {
        let deadline = Instant::now() + max_wait;
        while Instant::now() < deadline {
            let scripts_due = page.next_script_due().map(|due| due <= self.start.elapsed().as_secs_f64() * 1000.0 + 50.0).unwrap_or(false);
            if !scripts_due && page.images_in_flight() == 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
            let _ = self.tick(page);
        }
        let _ = self.tick(page);
    }

    /// Load `url`, let it settle, scroll to its fragment (if any) and
    /// render the viewport.
    pub fn render_url(&mut self, url: &str, viewport: Viewport) -> Result<Frame, EngineError> {
        let mut page = self.load(url, viewport)?;
        self.settle_for_snapshot(&mut page, Duration::from_secs(5));
        if let Some(y) = page.url.fragment.clone().and_then(|f| page.fragment_scroll(&f)) {
            self.set_page_scroll(&mut page, 0.0, y);
        }
        Ok(self.snapshot(&page))
    }
}
