//! Screenshots on demand (viewport, full page or one element) with the
//! refs of interactive elements drawn over them (set-of-marks).

use rabbithole_engine::gfx::{Canvas, Color, Family, Rect};
use rabbithole_engine::{Engine, Page};

use crate::ax::{Ax, Kind};

const PALETTE: [Color; 6] = [
    Color::rgb(220, 38, 38),
    Color::rgb(37, 99, 235),
    Color::rgb(22, 163, 74),
    Color::rgb(217, 119, 6),
    Color::rgb(147, 51, 234),
    Color::rgb(8, 145, 178),
];

/// What to capture.
#[derive(Debug, Clone, Copy)]
pub enum Area {
    Viewport,
    Full,
    /// Document-space region.
    Region(Rect),
}

/// Render to RGBA and return (pixels, width, height, region origin in
/// document coordinates).
pub fn render(engine: &Engine, page: &Page, area: Area, scale: f32) -> (Canvas, Rect) {
    let vw = page.viewport.width;
    let region = match area {
        Area::Viewport => Rect::new(page.page_scroll.0, page.page_scroll.1, vw, page.viewport.height),
        Area::Full => Rect::new(0.0, 0.0, vw, page.content_height().clamp(1.0, 20_000.0)),
        Area::Region(r) => {
            let pad = 8.0;
            let x = (r.x - pad).max(0.0);
            let y = (r.y - pad).max(0.0);
            Rect::new(x, y, (r.w + 2.0 * pad).min(4000.0).max(1.0), (r.h + 2.0 * pad).min(8000.0).max(1.0))
        }
    };
    let pw = (region.w * scale).round().max(1.0) as u32;
    let ph = (region.h * scale).round().max(1.0) as u32;
    let mut canvas = Canvas::new(pw, ph);
    engine.render_scaled(page, &mut canvas, region.x, region.y, scale);
    (canvas, region)
}

/// Draw boxes and `e12` labels over interactive elements of frame 0.
/// Returns the legend lines of what was marked.
pub fn draw_marks(engine: &Engine, canvas: &mut Canvas, root: &Ax, region: Rect, scale: f32, page: &Page) -> Vec<String> {
    let mut items: Vec<(u32, Rect, String)> = Vec::new();
    root.visit(&mut |a, _| {
        if a.frame != 0 || a.kind != Kind::Control {
            return;
        }
        let (Some(r), Some(rect)) = (a.reference, a.rect) else { return };
        // Fixed boxes are positioned relative to the viewport.
        let rect = if a.fixed { rect.translate(page.page_scroll.0, page.page_scroll.1) } else { rect };
        if rect.w < 2.0 || rect.h < 2.0 || rect.intersect(&region).is_empty() {
            return;
        }
        items.push((r, rect, format!("{} \"{}\"", a.role, crate::ax::clip_text(&a.name, 40))));
    });
    let font = engine.fonts.select(Family::Sans, 700, false);
    let size = 11.0 * scale;
    let mut legend = Vec::new();
    for (i, (r, rect, desc)) in items.iter().enumerate() {
        let color = PALETTE[i % PALETTE.len()];
        let x = (rect.x - region.x) * scale;
        let y = (rect.y - region.y) * scale;
        let w = rect.w * scale;
        let h = rect.h * scale;
        let t = (2.0 * scale).max(1.0);
        canvas.fill_rect(Rect::new(x, y, w, t), color);
        canvas.fill_rect(Rect::new(x, y + h - t, w, t), color);
        canvas.fill_rect(Rect::new(x, y, t, h), color);
        canvas.fill_rect(Rect::new(x + w - t, y, t, h), color);
        let label = format!("e{r}");
        let tw = engine.fonts.measure(font, size, &label) + 4.0 * scale;
        let lh = size + 3.0 * scale;
        let ly = if y >= lh { y - lh } else { y };
        canvas.fill_rect(Rect::new(x, ly, tw, lh), color);
        canvas.draw_text(&engine.fonts, font, size, &label, x + 2.0 * scale, ly + size, Color::WHITE);
        legend.push(format!("@e{r} {desc}"));
    }
    legend
}

/// PNG-encode a canvas.
pub fn png(canvas: &Canvas) -> Result<Vec<u8>, String> {
    let rgba = canvas.to_rgba8();
    let mut out = std::io::Cursor::new(Vec::new());
    image::write_buffer_with_format(&mut out, &rgba, canvas.width, canvas.height, image::ExtendedColorType::Rgba8, image::ImageFormat::Png).map_err(|e| format!("PNG encoding failed: {e}"))?;
    Ok(out.into_inner())
}
