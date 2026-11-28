//! Display list rasterization onto a `Canvas`.
//!
//! Pure translations (sticky offsets, `transform: translate()`) are applied
//! by the painter itself; other matrices, gradients, per-corner radii,
//! shadows and layers use the extended canvas API. When that API is not
//! available (`gfx_ext_available() == false`, during bring-up) the painter
//! degrades gracefully to the basic primitives.

use crate::display_list::{DisplayItem, DisplayList};
use rabbithole_gfx::{Canvas, Color, FontContext, FontId, Matrix, Paint, Point, Radii, Rect};
use rabbithole_style::{BorderStyle, Shadow, TextDecorationStyle};
use std::sync::OnceLock;

/// Geometry in scrolled document coordinates for an opaque video image
/// and the source-over content painted above it.
#[derive(Clone, Debug)]
pub struct VideoPlane {
    pub dest: Rect,
    pub src: Rect,
    pub clip: Rect,
    pub pixelated: bool,
    pub foreground: Option<std::sync::Arc<RasterForeground>>,
    // Underlying page colours of partially covered device pixels only.
    edges: std::sync::Arc<[(u32, u32, u32)]>,
    device_clip: Rect,
    // Difference between the first visible raster row and the logical scroll.
    raster_offset_y: f32,
}

pub fn video_plane(list: &DisplayList, target: &std::sync::Arc<rabbithole_gfx::Image>, viewport: Rect, scale: f32, fonts: &FontContext) -> Option<VideoPlane> {
    video_plane_on_background(list, target, viewport, scale, fonts, Color::WHITE)
}

pub fn video_plane_on_background(list: &DisplayList, target: &std::sync::Arc<rabbithole_gfx::Image>,
    viewport: Rect, scale: f32, fonts: &FontContext, background: Color) -> Option<VideoPlane> {
    video_plane_in_raster(list, target, viewport, scale, fonts, background, viewport.y)
}

/// Place the video on the device grid used to blit the backing canvas. Fixed
/// content still uses the logical viewport, including its fractional scroll.
pub fn video_plane_in_raster(list: &DisplayList, target: &std::sync::Arc<rabbithole_gfx::Image>,
    viewport: Rect, scale: f32, fonts: &FontContext, background: Color, canvas_y: f32) -> Option<VideoPlane> {
    use DisplayItem as D;
    macro_rules! fallback {
        ($reason:expr, $index:expr) => {{
            static TRACE: OnceLock<bool> = OnceLock::new();
            if *TRACE.get_or_init(|| std::env::var("RH_VIDEO_LAYER_TRACE").as_deref() == Ok("1")) {
                eprintln!("rh_video fallback {} {}", $index, $reason);
            }
            return None;
        }};
    }
    let mut origin = Some((0.0, 0.0));
    let mut origins = Vec::new();
    // An irregular clip still bounds later draws, but a picture inside it
    // cannot be composited with a single rectangular clip.
    if !scale.is_finite() || scale <= 0.0 { return None; }
    // The GUI blits the overscanned page at this whole-device-pixel offset.
    let blit_y = ((canvas_y - viewport.y) * scale).round();
    let raster_y = canvas_y - blit_y / scale;
    let raster_view = Rect::new(viewport.x, raster_y, viewport.w, viewport.h);
    let mut clips = vec![(raster_view, false)];
    let mut layers: Vec<Rect> = Vec::new();
    let mut plane: Option<VideoPlane> = None;
    let mut target_index = 0;
    let mut foreground_items = Vec::new();
    // The content image and rectangular clip both use snap() at unit
    // scale. At other scales they retain the original CSS coordinates.
    let image_rect = |rect: Rect| {
        if scale == 1.0 {
            snap_edges(rect.translate(-viewport.x, -canvas_y)).translate(viewport.x, canvas_y)
        } else {
            rect
        }
    };
    for (index, item) in list.items.iter().enumerate() {
        let (clip, irregular) = *clips.last()?;
        match item {
            D::PushTransform(m) => {
                origins.push(origin);
                origin = origin.filter(|_| m[..4] == [1.0, 0.0, 0.0, 1.0])
                    .map(|(x, y)| (x + m[4], y + m[5]));
            }
            D::PushFixed => {
                origins.push(origin);
                origin = origin.map(|_| (viewport.x, viewport.y));
            }
            D::PopFixed | D::PopTransform => origin = origins.pop()?,
            D::PushClip(r) | D::PushClipRounded(r, _) | D::PushClipPath { bounds: r, .. } => {
                let bounds = origin.map_or(clip, |(x, y)| {
                    let r = r.translate(x, y);
                    clip.intersect(&if matches!(item, D::PushClip(_)) { image_rect(r) } else { r })
                });
                clips.push((bounds, irregular || origin.is_none() || !matches!(item, D::PushClip(_))));
            }
            D::PopClip => { clips.pop()?; }
            D::PushLayer(r) => {
                // begin_layer saves one extra device pixel around its bounds.
                let pad = 1.0 / scale;
                let bounds = origin.map_or(clip, |(x, y)| Rect::new(r.x+x-pad, r.y+y-pad, r.w+2.0*pad, r.h+2.0*pad));
                layers.push(clip.intersect(&bounds));
            }
            D::PopLayer(_) => { layers.pop()?; }
            D::Image { rect, src, image, opacity, pixelated } if std::sync::Arc::ptr_eq(image, target) => {
                let Some((x,y)) = origin else { fallback!("transformed-video", index) };
                let dest = image_rect(rect.translate(x, y));
                let visible = dest.intersect(&clip);
                if plane.is_some() { fallback!("multiple-images", index); }
                if irregular { fallback!("nonrectangular-video-clip", index); }
                if !layers.is_empty() || *opacity != 1.0 { fallback!("video-opacity-group", index); }
                if visible.is_empty() { fallback!("outside-viewport", index); }
                let device = |v: f32, origin: f32| (v - origin) * scale;
                let left = device(visible.x, viewport.x).floor();
                let top = device(visible.y, raster_y).floor();
                let right = device(visible.right(), viewport.x).ceil();
                let bottom = device(visible.bottom(), raster_y).ceil();
                let device_clip = Rect::new(left, top, right - left, bottom - top);
                let visible = Rect::new(viewport.x + left / scale, raster_y + top / scale,
                    device_clip.w / scale, device_clip.h / scale);
                // The normal image painter treats near-full crops as full.
                let src = if src.x <= 0.0 && src.y <= 0.0 && (src.w - image.width as f32).abs() < 0.5 &&
                    (src.h - image.height as f32).abs() < 0.5 {
                    Rect::new(0.0, 0.0, image.width as f32, image.height as f32)
                } else { *src };
                if !*pixelated && (src.w > 4.0 * dest.w * scale || src.h > 4.0 * dest.h * scale) { return None; }
                target_index = index;
                plane = Some(VideoPlane { dest, src, clip: visible, pixelated: *pixelated, foreground: None, edges: Default::default(), device_clip, raster_offset_y: raster_y - viewport.y });
            }
            _ => {
                let Some(v) = &plane else { continue };
                if matches!(item, D::LayerBlend(_) | D::LayerMask(_)) {
                    if layers.last().is_some_and(|r| r.intersect(&v.clip).is_empty()) { continue; }
                    fallback!("mask-or-blend", index);
                }
                if clip.intersect(&v.clip).is_empty() { continue; }
                // The normal foreground painter replays affine transforms.
                // Include ink conservatively if a simple translated bounding
                // box is unavailable (glyph overhang, a rotated control).
                if let Some((x, y)) = origin {
                    if !matches!(item, D::Text { .. }) {
                        if let Some(bounds) = crate::damage::item_bounds(item) {
                            if bounds.translate(x, y).intersect(&clip).intersect(&v.clip).is_empty() { continue; }
                        }
                    }
                }
                match item {
                    D::Filtered { .. } => fallback!("filter", index),
                    D::Blended { .. } => fallback!("mix-blend-mode", index),
                    D::Isolated { .. } => fallback!("isolated-blend-group", index),
                    D::BackdropFilter { .. } => fallback!("backdrop-filter", index),
                    D::Transform3d(_) => fallback!("3d-group", index),
                    _ => {}
                }
                foreground_items.push(index);
            }
        }
    }
    let Some(mut plane) = plane else { fallback!("no-top-level-image", list.items.len()) };
    if !foreground_items.is_empty() {
        plane.foreground = RasterForeground::raster(list, target_index, &foreground_items, plane.clip, viewport, scale, fonts)?;
    }
    let device_dest = Rect::new((plane.dest.x - viewport.x) * scale, (plane.dest.y - raster_y) * scale,
        plane.dest.w * scale, plane.dest.h * scale);
    if [device_dest.x, device_dest.y, device_dest.right(), device_dest.bottom()].iter().any(|v| v.fract() != 0.0) {
        plane.capture_edges(list, target_index, viewport, scale, fonts, background, device_dest);
    }
    Some(plane)
}

impl VideoPlane {
    fn capture_edges(&mut self, list: &DisplayList, target: usize, viewport: Rect, scale: f32,
        fonts: &FontContext, background: Color, dest: Rect) {
        let clip = self.device_clip;
        let (w, h) = (clip.w as u32, clip.h as u32);
        if w == 0 || h == 0 { return; }
        let mut coords = Vec::new();
        for y in 0..h {
            let py = clip.y + y as f32;
            if py < dest.y || py + 1.0 > dest.bottom() {
                coords.extend((0..w).map(|x| (x, y)));
            } else {
                if clip.x < dest.x { coords.push((0, y)); }
                if clip.right() > dest.right() && (w > 1 || clip.x >= dest.x) { coords.push((w - 1, y)); }
            }
        }
        if coords.is_empty() { return; }
        // Replay the prefix before the video. The decoded image is translucent
        // only on its perimeter, so retain at most two rows and two columns.
        let mut canvas = Canvas::new(w, h);
        canvas.clear(Color::WHITE);
        if background.a != 0 { canvas.fill_rect(Rect::new(0.0, 0.0, clip.w, clip.h), background); }
        canvas.push_transform([scale, 0.0, 0.0, scale, 0.0, 0.0]);
        let fixed = (viewport.x - self.clip.x, viewport.y - self.clip.y);
        let mut state = State::nested(-self.clip.x, -self.clip.y, fixed, gfx_ext_available());
        let view = Rect::new(-1e7, -1e7, 2e7, 2e7);
        for item in &list.items[..target] { paint_item(item, &mut canvas, fonts, &mut state, view); }
        self.edges = coords.into_iter().map(|(x, y)| (x, y, canvas.data[(y * w + x) as usize])).collect();
    }

    /// Composite onto the same device grid as the cached page raster. Restore
    /// the background before blending fractional edges of the new picture.
    pub fn composite(&self, canvas: &mut Canvas, pixels: &[u8], size: (u32, u32),
        area: Rect, scroll: (f32, f32), scale: f32) -> bool {
        let dest = Rect::new(area.x + (self.dest.x - scroll.0) * scale,
            area.y + (self.dest.y - (scroll.1 + self.raster_offset_y)) * scale, self.dest.w * scale, self.dest.h * scale);
        let clip = self.device_clip.translate(area.x, area.y);
        let visible = clip.intersect(&area);
        let drawn = canvas.composite_video_with_background(pixels, size, self.src, dest, visible,
            self.pixelated, |canvas| {
                let v = visible.intersect(&Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32));
                for &(x, y, color) in self.edges.iter() {
                    let (px, py) = (clip.x + x as f32, clip.y + y as f32);
                    if px >= v.x && px < v.right() && py >= v.y && py < v.bottom() {
                        canvas.data[py as usize * canvas.width as usize + px as usize] = color;
                    }
                }
            });
        if drawn {
            if let Some(foreground) = &self.foreground { foreground.paint(canvas, clip, area); }
        }
        drawn
    }
}

/// Premultiplied ARGB8 in device pixels, cropped to the visible video.
#[derive(Debug)]
pub struct RasterForeground {
    width: usize,
    pixels: Vec<u32>,
}

/// Foreground composited over a video plane.
pub type VideoForeground = RasterForeground;

impl RasterForeground {
    fn raster(list: &DisplayList, target: usize, foreground: &[usize], clip: Rect, viewport: Rect, scale: f32, fonts: &FontContext) -> Option<Option<std::sync::Arc<Self>>> {
        use DisplayItem as D;
        let (w, h) = ((clip.w * scale).round() as u32, (clip.h * scale).round() as u32);
        if w == 0 || h == 0 { return None; }
        let render = |background| {
            let mut canvas = Canvas::new(w, h);
            canvas.clear(background);
            canvas.push_transform([scale, 0.0, 0.0, scale, 0.0, 0.0]);
            let fixed = (viewport.x - clip.x, viewport.y - clip.y);
            let mut state = State::nested(-clip.x, -clip.y, fixed, gfx_ext_available());
            // Use a conservative culling region. The device canvas itself
            // clips pixels; glyph ink may overhang its layout bounds.
            let view = Rect::new(-1e7, -1e7, 2e7, 2e7);
            let mut draws = foreground.iter().copied().peekable();
            for (i, item) in list.items.iter().enumerate() {
                let geometry = matches!(item, D::PushClip(_) | D::PushClipRounded(..) |
                    D::PushClipPath { .. } | D::PopClip | D::PushFixed | D::PopFixed |
                    D::PushTransform(_) | D::PopTransform);
                let layer = i > target && matches!(item, D::PushLayer(_) | D::PopLayer(_));
                if geometry || layer || draws.next_if_eq(&i).is_some() {
                    paint_item(item, &mut canvas, fonts, &mut state, view);
                }
            }
            canvas.data
        };
        // Source-over onto black gives the premultiplied colour. Onto white
        // it adds (1 - alpha). This also preserves antialiased glyphs.
        let (layer, any, transparent) = Self::from_backdrops(w, render(Color::BLACK), render(Color::WHITE));
        if std::env::var("RH_MEDIA_STATS").as_deref() == Ok("1") {
            eprintln!("rh_video foreground {w} {h} visible={transparent} ink={any}");
        }
        if !transparent { return None; }
        Some(any.then(|| std::sync::Arc::new(layer)))
    }

    fn from_backdrops(w: u32, mut pixels: Vec<u32>, white: Vec<u32>) -> (Self, bool, bool) {
        let mut any = false;
        let mut transparent = false;
        for (p, w) in pixels.iter_mut().zip(white) {
            let trans = ((w >> 8) & 255).saturating_sub((*p >> 8) & 255);
            let alpha = 255 - trans;
            any |= alpha != 0;
            transparent |= trans != 0;
            *p = (*p & 0xffffff) | alpha << 24;
        }
        (Self { width: w as usize, pixels }, any, transparent)
    }

    /// Restore the page foreground over a freshly drawn video, clipped to
    /// the current page area when the window is being resized.
    pub fn paint(&self, canvas: &mut Canvas, dest: Rect, area: Rect) {
        let clip = dest.intersect(&area).intersect(&Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32));
        if clip.is_empty() { return; }
        let (x0, y0) = (clip.x as usize, clip.y as usize);
        let n = clip.right() as usize - x0;
        for y in y0..clip.bottom() as usize {
            let i = (y as i32 - dest.y as i32) as usize * self.width + (x0 as i32 - dest.x as i32) as usize;
            let j = y * canvas.width as usize + x0;
            for (out, &pixel) in canvas.data[j..j+n].iter_mut().zip(&self.pixels[i..i+n]) {
                let a = pixel >> 24;
                if a == 0 { continue; }
                if a == 255 { *out = pixel & 0xffffff; continue; }
                let channel = |shift: u32| {
                    (((pixel >> shift) & 255) + (((*out >> shift) & 255) * (255 - a) + 127) / 255).min(255)
                };
                *out = (channel(16) << 16) | (channel(8) << 8) | channel(0);
            }
        }
    }
}

/// How a [`PageLayer`] moves while the UI scrolls ahead of a repaint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LayerMotion {
    /// Painted in viewport coordinates: it never moves.
    Fixed,
    /// Painted in page-canvas coordinates: it moves with the canvas.
    Scroll,
    /// A sticky box painted in page-canvas coordinates for the page scroll
    /// `at`. It moves by the change of its viewport position, which its
    /// containing block clamps.
    Sticky { patch: crate::display_list::StickyPatch, viewport: (f32, f32), at: (f32, f32) },
}

/// A source-over overlay of the page canvas: fixed or sticky content, or
/// scrolling content painted above them.
pub struct PageLayer {
    bands: Vec<(Rect, RasterForeground)>,
    pub motion: LayerMotion,
}

impl std::fmt::Debug for PageLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PageLayer").field("motion", &self.motion).field("bands", &self.bands.iter().map(|b| b.0).collect::<Vec<_>>()).finish()
    }
}

impl PageLayer {
    /// Only the occupied rows of what `paint` draws on a `w`×`h` canvas
    /// need to cross to the UI.
    pub fn from_paint(w: u32, h: u32, motion: LayerMotion, paint: impl Fn(&mut Canvas)) -> Self {
        Self::from_rows(w, 0, h, motion, paint)
    }

    /// Like [`PageLayer::from_paint`] for the `h` rows from `top` of a
    /// taller canvas (`paint` draws them at the top of its canvas).
    pub fn from_rows(w: u32, top: u32, h: u32, motion: LayerMotion, paint: impl Fn(&mut Canvas)) -> Self {
        if w == 0 || h == 0 {
            return Self { bands: Vec::new(), motion };
        }
        let render = |bg| {
            let mut canvas = Canvas::new(w, h);
            canvas.clear(bg);
            paint(&mut canvas);
            canvas.data
        };
        let (layer, _, _) = RasterForeground::from_backdrops(w, render(Color::BLACK), render(Color::WHITE));
        let width = w as usize;
        let mut bands = Vec::new();
        let mut rows = layer.pixels.chunks_exact(width).enumerate().peekable();
        while let Some((first, row)) = rows.next() {
            let occupied = |row: &[u32]| row.iter().any(|p| p >> 24 != 0);
            if !occupied(row) { continue; }
            let mut end = first + 1;
            while rows.next_if(|(_, row)| occupied(row)).is_some() { end += 1; }
            let bounds = Rect::new(0.0, (top as usize + first) as f32, w as f32, (end - first) as f32);
            let pixels = layer.pixels[first * width..end * width].to_vec();
            bands.push((bounds, RasterForeground { width, pixels }));
        }
        Self { bands, motion }
    }

    /// Viewport-fixed items, with the viewport as origin.
    pub fn fixed(items: &[DisplayItem], w: u32, h: u32, scale: f32, fonts: &FontContext) -> Self {
        Self::from_paint(w, h, LayerMotion::Fixed, |canvas| {
            canvas.push_transform([scale, 0.0, 0.0, scale, 0.0, 0.0]);
            paint_items(items, canvas, fonts, 0.0, 0.0, Rect::new(0.0, 0.0, w as f32, h as f32), (0.0, 0.0));
            canvas.pop_transform();
        })
    }

    /// Items painted like the page canvas at `origin` (see
    /// [`paint_items`]). A sticky box stuck to the viewport is painted at
    /// its exact viewport position instead, so that it keeps the same
    /// pixels however the canvas origin and the scroll change.
    #[allow(clippy::too_many_arguments)]
    pub fn scrolling(items: &[DisplayItem], w: u32, h: u32, scale: f32, fonts: &FontContext, origin: (f32, f32), fixed_origin: (f32, f32), motion: LayerMotion) -> Self {
        let shift = match motion {
            LayerMotion::Sticky { patch, viewport, at } => {
                let offset = |x: f32, y: f32| patch.offset(Rect::new(x, y, viewport.0, viewport.1));
                let (d, right, left, down, up) = (offset(at.0, at.1), offset(at.0 + 1.0, at.1), offset(at.0 - 1.0, at.1), offset(at.0, at.1 + 1.0), offset(at.0, at.1 - 1.0));
                let follows = |a: f32, b: f32| (a - b - 1.0).abs() < 1e-3;
                let snap = |o: f32, t: f32| (o - t) * scale - ((o - t) * scale).round();
                let x = if follows(right.0, d.0) || follows(d.0, left.0) { snap(origin.0, at.0) } else { 0.0 };
                let y = if follows(down.1, d.1) || follows(d.1, up.1) { snap(origin.1, at.1) } else { 0.0 };
                (x, y)
            }
            _ => (0.0, 0.0),
        };
        // Only raster the rows the run can paint, when they are known.
        let rows = crate::damage::run_bounds(items, origin).map(|b| {
            let y0 = ((b.y * scale + shift.1).floor() - 2.0).clamp(0.0, h as f32) as u32;
            let y1 = ((b.bottom() * scale + shift.1).ceil() + 2.0).clamp(0.0, h as f32) as u32;
            (y0, y1.max(y0))
        });
        let (top, bottom) = rows.unwrap_or((0, h));
        // Culling uses the canvas coordinates before scaling.
        let view = match rows {
            Some(_) => Rect::new(0.0, (top as f32 - shift.1) / scale - 2.0, w as f32, (bottom - top) as f32 / scale + 4.0),
            None => Rect::new(0.0, 0.0, w as f32, h as f32),
        };
        Self::from_rows(w, top, bottom - top, motion, |canvas| {
            canvas.push_transform([scale, 0.0, 0.0, scale, shift.0, shift.1 - top as f32]);
            paint_items(items, canvas, fonts, origin.0, origin.1, view, fixed_origin);
            canvas.pop_transform();
        })
    }

    pub fn is_empty(&self) -> bool {
        self.bands.is_empty()
    }

    /// Where the layer's top-left corner goes (device px) when the page is
    /// presented at `scroll`. `canvas` is where the page canvas goes and
    /// `painted` where it went at the frame's own scroll.
    pub fn position(&self, area: Rect, canvas: (f32, f32), painted: (f32, f32), scroll: (f32, f32), scale: f32) -> (f32, f32) {
        match self.motion {
            LayerMotion::Fixed => (area.x, area.y),
            LayerMotion::Scroll => canvas,
            LayerMotion::Sticky { patch, viewport, at } => {
                // Its viewport position, rounded like the canvas.
                let place = |t: (f32, f32)| {
                    let (dx, dy) = patch.offset(Rect::new(t.0, t.1, viewport.0, viewport.1));
                    (((dx - t.0) * scale).round(), ((dy - t.1) * scale).round())
                };
                let (now, then) = (place(scroll), place(at));
                (painted.0 + now.0 - then.0, painted.1 + now.1 - then.1)
            }
        }
    }

    /// Composite the layer with its top-left corner at `at` (whole device
    /// px), clipped to `area`.
    pub fn paint(&self, dst: &mut Canvas, area: Rect, at: (f32, f32)) {
        for (bounds, band) in &self.bands {
            band.paint(dst, bounds.translate(at.0, at.1), area);
        }
    }
}

fn style_code(s: BorderStyle) -> u8 {
    match s {
        BorderStyle::None | BorderStyle::Hidden => 0,
        BorderStyle::Solid => 0,
        BorderStyle::Dashed => 1,
        BorderStyle::Dotted => 2,
        BorderStyle::Double => 3,
    }
}

/// Whether the extended canvas / font API (gradients, radii, layers, web
/// fonts) is implemented in the linked gfx crate. Probed once.
pub(crate) fn gfx_ext_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let probe = std::panic::catch_unwind(|| {
            let mut c = Canvas::new(1, 1);
            c.push_transform(rabbithole_gfx::IDENTITY);
            c.pop_transform();
            let _ = c.map_point(Point { x: 0.0, y: 0.0 });
        });
        probe.is_ok()
    })
}

/// Whether `FontContext::select_family` & co. are implemented.
pub(crate) fn font_ext_available(fonts: &FontContext) -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let probe = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let f = fonts.select(rabbithole_gfx::Family::Sans, 400, false);
            let _ = fonts.needs_synthetic_italic(f, false);
            let _ = fonts.select_family(&[], rabbithole_gfx::Family::Sans, 400, false);
        }));
        probe.is_ok()
    })
}

fn uniform_radius(r: &Radii) -> Option<f32> {
    let (h, v) = r[0];
    if (h - v).abs() > 0.01 {
        return None;
    }
    if r.iter().all(|(a, b)| (*a - h).abs() < 0.01 && (*b - v).abs() < 0.01) {
        Some(h)
    } else {
        None
    }
}

/// Snap a box to device pixels like Blink does when painting boxes (edges
/// rounded to the nearest pixel), so adjacent boxes at fractional offsets
/// do not leave antialiased seams. Under a transform only when it maps
/// the pixel grid onto itself (mirrors, offsets).
fn snap(r: Rect, canvas: &Canvas) -> Rect {
    let m = canvas.transform();
    if !rabbithole_gfx::path::is_identity(&m) {
        // Under mirrors / offsets the pixel grid maps onto itself: snap
        // the device rect and map it back.
        if m[1] == 0.0 && m[2] == 0.0 && m[0].abs() == 1.0 && m[3].abs() == 1.0 {
            let (ax, bx) = (m[0] * r.x + m[4], m[0] * (r.x + r.w) + m[4]);
            let (ay, by) = (m[3] * r.y + m[5], m[3] * (r.y + r.h) + m[5]);
            let (x0, x1) = (ax.min(bx).round(), ax.max(bx).round());
            let (y0, y1) = (ay.min(by).round(), ay.max(by).round());
            if (x1 <= x0 && r.w > 0.0) || (y1 <= y0 && r.h > 0.0) {
                return r;
            }
            let (lx0, lx1) = ((x0 - m[4]) * m[0], (x1 - m[4]) * m[0]);
            let (ly0, ly1) = ((y0 - m[5]) * m[3], (y1 - m[5]) * m[3]);
            return Rect::new(lx0.min(lx1), ly0.min(ly1), (lx1 - lx0).abs(), (ly1 - ly0).abs());
        }
        return r;
    }
    snap_edges(r)
}

fn snap_edges(r: Rect) -> Rect {
    let (x0, y0) = (r.x.round(), r.y.round());
    let (x1, y1) = ((r.x + r.w).round(), (r.y + r.h).round());
    if (x1 <= x0 && r.w > 0.0) || (y1 <= y0 && r.h > 0.0) {
        return r;
    }
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

/// Pixels per CSS px for rendering a `w`×`h` vector image under `m`: the
/// device scale, doubled for images up to 1024 device px (the draw then
/// filters the rendering down: smoother, direction-independent edges).
fn vector_scale(m: &Matrix, w: f32, h: f32) -> f32 {
    let s = (m[0] * m[0] + m[1] * m[1]).sqrt().max((m[2] * m[2] + m[3] * m[3]).sqrt());
    if w.max(h) * s < 1024.0 { s * 2.0 } else { s }
}

fn is_translation(m: &Matrix) -> bool {
    (m[0] - 1.0).abs() < 1e-6 && m[1].abs() < 1e-6 && m[2].abs() < 1e-6 && (m[3] - 1.0).abs() < 1e-6
}

fn translate_radii(_r: Radii) -> Radii {
    _r
}

fn shift_paint(p: &Paint, dx: f32, dy: f32) -> Paint {
    match p {
        Paint::Solid(c) => Paint::Solid(*c),
        Paint::Linear { start, end, stops, repeat } => Paint::Linear {
            start: Point { x: start.x + dx, y: start.y + dy },
            end: Point { x: end.x + dx, y: end.y + dy },
            stops: stops.clone(),
            repeat: *repeat,
        },
        Paint::Radial { center, rx, ry, stops, repeat } => Paint::Radial {
            center: Point { x: center.x + dx, y: center.y + dy },
            rx: *rx,
            ry: *ry,
            stops: stops.clone(),
            repeat: *repeat,
        },
        Paint::Conic { center, angle, stops, repeat } => Paint::Conic {
            center: Point { x: center.x + dx, y: center.y + dy },
            angle: *angle,
            stops: stops.clone(),
            repeat: *repeat,
        },
        Paint::Tiled { paint, origin, size, step, repeat_x, repeat_y } => Paint::Tiled {
            paint: Box::new(shift_paint(paint, dx, dy)),
            origin: Point { x: origin.x + dx, y: origin.y + dy },
            size: *size,
            step: *step,
            repeat_x: *repeat_x,
            repeat_y: *repeat_y,
        },
    }
}

/// Average color of a paint (fallback when gradients are unavailable).
fn average_color(p: &Paint) -> Color {
    match p {
        Paint::Solid(c) => *c,
        Paint::Tiled { paint, .. } => average_color(paint),
        Paint::Linear { stops, .. } | Paint::Radial { stops, .. } | Paint::Conic { stops, .. } => {
            if stops.is_empty() {
                return Color::TRANSPARENT;
            }
            let n = stops.len() as u32;
            let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
            for s in stops {
                r += s.color.r as u32;
                g += s.color.g as u32;
                b += s.color.b as u32;
                a += s.color.a as u32;
            }
            Color::rgba((r / n) as u8, (g / n) as u8, (b / n) as u8, (a / n) as u8)
        }
    }
}

enum Frame {
    /// Pure translation handled by the painter.
    Translate(f32, f32),
    /// Matrix pushed to the canvas.
    Canvas,
}

pub(crate) struct State {
    /// Current translation (document → canvas), including scroll.
    pub(crate) dx: f32,
    pub(crate) dy: f32,
    frames: Vec<Frame>,
    clips: Vec<bool>,
    layers: Vec<bool>,
    /// Alpha mask for the layer about to be closed (`mask-image`).
    mask: Option<Vec<u8>>,
    /// Blend mode of the layer about to be closed.
    blend: u8,
    /// Saved translation while inside the fixed layer.
    fixed_saved: Option<(f32, f32)>,
    /// Canvas offset of `position: fixed` content (the viewport origin).
    pub(crate) fixed_origin: (f32, f32),
    pub(crate) ext: bool,
    /// Under canvas transforms: the painted area mapped back into the
    /// items' coordinates (culling must not compare untransformed rects
    /// with the device area), one per pushed matrix.
    views: Vec<Rect>,
}

impl State {
    /// A fresh state for painting a sub-list offscreen.
    pub(crate) fn nested(dx: f32, dy: f32, fixed_origin: (f32, f32), ext: bool) -> State {
        State { dx, dy, frames: Vec::new(), clips: Vec::new(), layers: Vec::new(), fixed_saved: None, fixed_origin, ext, mask: None, blend: 0, views: Vec::new() }
    }

}

pub fn paint(list: &DisplayList, canvas: &mut Canvas, fonts: &FontContext, scroll_x: f32, scroll_y: f32) {
    paint_with_fixed_origin(list, canvas, fonts, scroll_x, scroll_y, (0.0, 0.0))
}

/// Like [`paint`], with `position: fixed` content (viewport coordinates)
/// painted at `fixed_origin` on the canvas — where the viewport lies in an
/// overscanned canvas.
pub fn paint_with_fixed_origin(list: &DisplayList, canvas: &mut Canvas, fonts: &FontContext, scroll_x: f32, scroll_y: f32, fixed_origin: (f32, f32)) {
    let view = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
    paint_items(&list.items, canvas, fonts, scroll_x, scroll_y, view, fixed_origin);
}

/// Paint only the device-space `region` (clipped; items outside it are
/// skipped), e.g. the damage of a scroll or a style change.
pub fn paint_region(list: &DisplayList, canvas: &mut Canvas, fonts: &FontContext, scroll_x: f32, scroll_y: f32, region: Rect) {
    paint_region_with_fixed_origin(list, canvas, fonts, scroll_x, scroll_y, region, (0.0, 0.0));
}

/// Paint a damaged region of an overscanned canvas at the same viewport
/// offset as a full paint, including fixed elements overlapping the damage.
pub fn paint_region_with_fixed_origin(list: &DisplayList, canvas: &mut Canvas, fonts: &FontContext, scroll_x: f32, scroll_y: f32, region: Rect, fixed_origin: (f32, f32)) {
    paint_items_region(&list.items, canvas, fonts, scroll_x, scroll_y, region, fixed_origin);
}

/// Clip as well as cull: glyphs and other ink can extend beyond the damage.
pub fn paint_items_region(items: &[DisplayItem], canvas: &mut Canvas, fonts: &FontContext, scroll_x: f32, scroll_y: f32, region: Rect, fixed_origin: (f32, f32)) {
    let view = region.intersect(&Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32));
    if view.is_empty() {
        return;
    }
    canvas.push_clip(view);
    paint_items(items, canvas, fonts, scroll_x, scroll_y, view, fixed_origin);
    canvas.pop_clip();
}

/// Paint a balanced range of display items using the same origin and culling as a full paint.
pub fn paint_items(items: &[DisplayItem], canvas: &mut Canvas, fonts: &FontContext, scroll_x: f32, scroll_y: f32, view: Rect, fixed_origin: (f32, f32)) {
    let mut st = State {
        dx: -scroll_x,
        dy: -scroll_y,
        frames: Vec::new(),
        clips: Vec::new(),
        layers: Vec::new(),
        mask: None,
        blend: 0,
        fixed_saved: None,
        fixed_origin,
        ext: gfx_ext_available(),
        views: Vec::new(),
    };
    for item in items {
        paint_item(item, canvas, fonts, &mut st, view);
    }
    // Balance anything left open.
    while let Some(c) = st.clips.pop() {
        if c {
            canvas.pop_clip();
        }
    }
    while let Some(f) = st.frames.pop() {
        if let Frame::Canvas = f {
            canvas.pop_transform();
            st.views.pop();
        }
    }
    while let Some(l) = st.layers.pop() {
        if l {
            canvas.end_layer(1.0);
        }
    }
}

/// Alpha (one byte per pixel of the open layer's device bounds) of mask
/// items: they are painted over black and over white with the current
/// transform; where a pixel ends up the same on both, the mask is opaque.
fn layer_mask_alpha(items: &[DisplayItem], canvas: &Canvas, fonts: &FontContext, st: &State) -> Option<Vec<u8>> {
    let (x0, y0, w, h) = canvas.layer_bounds()?;
    if w == 0 || h == 0 {
        return Some(Vec::new());
    }
    let m = rabbithole_style::multiply_matrix([1.0, 0.0, 0.0, 1.0, -(x0 as f32), -(y0 as f32)], canvas.transform());
    // Without a canvas transform the offset goes into the painter's own
    // translation, so boxes snap to pixels exactly as on the page.
    let plain = rabbithole_gfx::path::is_identity(&canvas.transform());
    let render = |bg: Color| {
        let mut c = Canvas::new(w as u32, h as u32);
        c.clear(bg);
        let (sdx, sdy) = if plain { (st.dx - x0 as f32, st.dy - y0 as f32) } else { (st.dx, st.dy) };
        if !plain {
            c.push_transform(m);
        }
        let mut s = State { dx: sdx, dy: sdy, frames: Vec::new(), clips: Vec::new(), layers: Vec::new(), fixed_saved: None, fixed_origin: st.fixed_origin, ext: st.ext, mask: None, blend: 0, views: Vec::new() };
        let all = Rect::new(-1.0e7, -1.0e7, 2.0e7, 2.0e7);
        for it in items {
            paint_item(it, &mut c, fonts, &mut s, all);
        }
        c.data
    };
    let black = render(Color::BLACK);
    let white = render(Color::WHITE);
    Some(
        black
            .iter()
            .zip(white.iter())
            .map(|(b, wh)| {
                // Per channel: white - black = 255·(1 - alpha).
                let g = |v: u32| ((v >> 8) & 0xff) as i32;
                (255 - (g(*wh) - g(*b))).clamp(0, 255) as u8
            })
            .collect(),
    )
}

/// Device pixel bounds of a local rect under the canvas transform, within
/// the canvas.
fn device_bounds(canvas: &Canvas, r: Rect) -> Option<(i32, i32, usize, usize)> {
    let d = rabbithole_gfx::path::transform_rect_bounds(&canvas.transform(), r);
    let x0 = d.x.floor().max(0.0) as i32;
    let y0 = d.y.floor().max(0.0) as i32;
    let x1 = (d.x + d.w).ceil().min(canvas.width as f32) as i32;
    let y1 = (d.y + d.h).ceil().min(canvas.height as f32) as i32;
    if x1 <= x0 || y1 <= y0 { return None; }
    // The group's output cannot affect any pixel outside these bounds.
    // Keep all input pixels when it overlaps: blur can sample past the clip.
    let output = Rect::new(x0 as f32, y0 as f32, (x1 - x0) as f32, (y1 - y0) as f32);
    (!output.intersect(&canvas.clip()).is_empty()).then(|| (x0, y0, (x1 - x0) as usize, (y1 - y0) as usize))
}

/// Draw straight RGBA pixels at device position (x0, y0), clipped by the
/// canvas clips.
pub(crate) fn draw_device_pixels(canvas: &mut Canvas, rgba: Vec<u8>, x0: i32, y0: i32, w: usize, h: usize) {
    let img = rabbithole_gfx::Image::new(w as u32, h as u32, rgba);
    let inv = crate::tree::invert_matrix(canvas.transform());
    if let Some(inv) = inv {
        canvas.push_transform(inv);
    }
    let smooth = canvas.image_smoothing;
    canvas.image_smoothing = false;
    canvas.draw_image(&img, Rect::new(x0 as f32, y0 as f32, w as f32, h as f32), 1.0);
    canvas.image_smoothing = smooth;
    if inv.is_some() {
        canvas.pop_transform();
    }
}

fn device_scale(m: &Matrix) -> f32 {
    (m[0] * m[3] - m[1] * m[2]).abs().sqrt().max(0.01)
}

/// `filter`: render the group over black and over white offscreen,
/// recover its RGBA, filter it and composite the result.
#[allow(clippy::too_many_arguments)]
fn paint_filtered(bounds: Rect, filters: &[rabbithole_style::FilterOp], svg: Option<&crate::filter::UrlFilters>, items: &[DisplayItem], canvas: &mut Canvas, fonts: &FontContext, st: &mut State, view: Rect) {
    let local = bounds.translate(st.dx, st.dy);
    if !st.ext || local.intersect(&view).is_empty() && rabbithole_gfx::path::transform_rect_bounds(&canvas.transform(), local).intersect(&Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32)).is_empty() {
        if !st.ext {
            for it in items {
                paint_item(it, canvas, fonts, st, view);
            }
        }
        return;
    }
    let Some((x0, y0, w, h)) = device_bounds(canvas, local) else { return };
    let m = rabbithole_style::multiply_matrix([1.0, 0.0, 0.0, 1.0, -(x0 as f32), -(y0 as f32)], canvas.transform());
    // Without a canvas transform the offset goes into the painter's own
    // translation, so boxes snap to pixels exactly as on the page.
    let plain = rabbithole_gfx::path::is_identity(&canvas.transform());
    let render = |bg: Color| {
        let mut c = Canvas::new(w as u32, h as u32);
        c.clear(bg);
        let (sdx, sdy) = if plain { (st.dx - x0 as f32, st.dy - y0 as f32) } else { (st.dx, st.dy) };
        if !plain {
            c.push_transform(m);
        }
        let fixed_origin = if plain { (st.fixed_origin.0 - x0 as f32, st.fixed_origin.1 - y0 as f32) } else { st.fixed_origin };
        let mut s = State { dx: sdx, dy: sdy, frames: Vec::new(), clips: Vec::new(), layers: Vec::new(), fixed_saved: None, fixed_origin, ext: st.ext, mask: None, blend: 0, views: Vec::new() };
        let all = Rect::new(-1.0e7, -1.0e7, 2.0e7, 2.0e7);
        for it in items {
            paint_item(it, &mut c, fonts, &mut s, all);
        }
        c.data
    };
    let black = render(Color::BLACK);
    let white = render(Color::WHITE);
    let mut px = crate::filter::from_black_white(&black, &white, w, h);
    match svg {
        Some(u) => {
            // Layout coordinates → buffer pixels.
            let lm = rabbithole_style::multiply_matrix(m, [1.0, 0.0, 0.0, 1.0, st.dx, st.dy]);
            px = u.apply(filters, px, &lm, device_scale(&canvas.transform()), fonts);
        }
        None => crate::filter::apply(&mut px, filters, device_scale(&canvas.transform())),
    }
    draw_device_pixels(canvas, crate::filter::to_rgba(&px), x0, y0, w, h);
}

/// The RGBA (premultiplied) painting of `items` within `bounds`, at the
/// device pixels it covers: (x0, y0, w, h, pixels).
fn offscreen_rgba(bounds: Rect, items: &[DisplayItem], canvas: &Canvas, fonts: &FontContext, st: &State) -> Option<(i32, i32, usize, usize, crate::filter::Pixels)> {
    let local = bounds.translate(st.dx, st.dy);
    let (x0, y0, w, h) = device_bounds(canvas, local)?;
    let m = rabbithole_style::multiply_matrix([1.0, 0.0, 0.0, 1.0, -(x0 as f32), -(y0 as f32)], canvas.transform());
    let plain = rabbithole_gfx::path::is_identity(&canvas.transform());
    let render = |bg: Color| {
        let mut c = Canvas::new(w as u32, h as u32);
        c.clear(bg);
        let (sdx, sdy) = if plain { (st.dx - x0 as f32, st.dy - y0 as f32) } else { (st.dx, st.dy) };
        let fixed_origin = if plain { (st.fixed_origin.0 - x0 as f32, st.fixed_origin.1 - y0 as f32) } else { st.fixed_origin };
        if !plain {
            c.push_transform(m);
        }
        let mut s = State::nested(sdx, sdy, fixed_origin, st.ext);
        let all = Rect::new(-1.0e7, -1.0e7, 2.0e7, 2.0e7);
        for it in items {
            paint_item(it, &mut c, fonts, &mut s, all);
        }
        c.data
    };
    let black = render(Color::BLACK);
    let white = render(Color::WHITE);
    Some((x0, y0, w, h, crate::filter::from_black_white(&black, &white, w, h)))
}

/// Source over backdrop with a blend mode (CSS Compositing 1 §5.8):
/// `cs_p` / `a_s` the premultiplied source, `cb_p` / `a_b` the backdrop.
/// Returns the premultiplied result colour and alpha.
fn blend_over(mode: u8, cs_p: [f32; 3], a_s: f32, cb_p: [f32; 3], a_b: f32) -> ([f32; 3], f32) {
    let to = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    let pack = |c: [f32; 3], a: f32| {
        let un = |v: f32| if a > 0.0 { v / a } else { 0.0 };
        (to(un(c[0])) << 16) | (to(un(c[1])) << 8) | to(un(c[2]))
    };
    let mixed = rabbithole_gfx::blend_pixel(mode, pack(cb_p, a_b), pack(cs_p, a_s));
    let ch = |s: u32| ((mixed >> s) & 0xff) as f32 / 255.0;
    let b = [ch(16), ch(8), ch(0)];
    let mut out = [0.0f32; 3];
    for k in 0..3 {
        out[k] = cs_p[k] * (1.0 - a_b) + cb_p[k] * (1.0 - a_s) + a_s * a_b * b[k];
    }
    (out, a_s + a_b - a_s * a_b)
}

/// `mode`-blended `p` (premultiplied source, alpha × `op`) over the opaque
/// pixel `cb`, in 8-bit steps like a blended layer.
fn blend_onto_opaque(mode: u8, p: [f32; 4], op: f32, cb: u32) -> [u8; 3] {
    let a = p[3];
    let alpha = (a * op * 255.0 + 0.5) as u32;
    let un = |v: f32| ((v / a).clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    let cs = (un(p[0]) << 16) | (un(p[1]) << 8) | un(p[2]);
    let mixed = rabbithole_gfx::blend_pixel(mode, cb, cs);
    let ch = |v: u32, s: u32| (v >> s) & 0xff;
    [16u32, 8, 0].map(|s| ((ch(mixed, s) * alpha + ch(cb, s) * (255 - alpha.min(255)) + 127) / 255) as u8)
}

/// Opaque device pixels of `canvas` under (x0, y0, w, h), as colours in
/// 0..=1.
fn device_rgb(canvas: &Canvas, x: i32, y: i32) -> [f32; 3] {
    let v = canvas.data[y as usize * canvas.width as usize + x as usize];
    [((v >> 16) & 0xff) as f32 / 255.0, ((v >> 8) & 0xff) as f32 / 255.0, (v & 0xff) as f32 / 255.0]
}

/// A blended box: its painting offscreen, blended onto the canvas pixels.
fn paint_blended(bounds: Rect, mode: u8, opacity: f32, items: &[DisplayItem], canvas: &mut Canvas, fonts: &FontContext, st: &mut State, view: Rect) {
    if !st.ext {
        for it in items {
            paint_item(it, canvas, fonts, st, view);
        }
        return;
    }
    let Some((x0, y0, w, h, px)) = offscreen_rgba(bounds, items, canvas, fonts, st) else { return };
    let op = opacity.clamp(0.0, 1.0);
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let p = px.px[y * w + x];
            let a = p[3] * op;
            if a <= 0.0 {
                continue;
            }
            let cb = canvas.data[(y0 as usize + y) * canvas.width as usize + x0 as usize + x];
            let c = blend_onto_opaque(mode, p, op, cb);
            let o = (y * w + x) * 4;
            rgba[o..o + 3].copy_from_slice(&c);
            rgba[o + 3] = 255;
        }
    }
    draw_device_pixels(canvas, rgba, x0, y0, w, h);
}

/// A blended box inside an isolated group painted in lockstep over black
/// (`cb`) and white (`cw`): blended against the group's own backdrop,
/// whose alpha the two renderings give.
fn paint_blended_isolated(bounds: Rect, mode: u8, opacity: f32, items: &[DisplayItem], cb: &mut Canvas, cw: &mut Canvas, fonts: &FontContext, sb: &State) {
    let Some((x0, y0, w, h, px)) = offscreen_rgba(bounds, items, cb, fonts, sb) else { return };
    let op = opacity.clamp(0.0, 1.0);
    let mut rk = vec![0u8; w * h * 4];
    let mut rw = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let p = px.px[y * w + x];
            let a = p[3] * op;
            if a <= 0.0 {
                continue;
            }
            let (dx, dy) = (x0 + x as i32, y0 + y as i32);
            let o = (y * w + x) * 4;
            let (rawk, raww) = (cb.data[dy as usize * cb.width as usize + dx as usize], cw.data[dy as usize * cw.width as usize + dx as usize]);
            if rawk == raww {
                // Opaque group backdrop: as on the page.
                let c = blend_onto_opaque(mode, p, op, rawk);
                rk[o..o + 3].copy_from_slice(&c);
                rw[o..o + 3].copy_from_slice(&c);
                rk[o + 3] = 255;
                rw[o + 3] = 255;
                continue;
            }
            let (bk, bw) = (device_rgb(cb, dx, dy), device_rgb(cw, dx, dy));
            let ab = (1.0 - ((bw[0] - bk[0]) + (bw[1] - bk[1]) + (bw[2] - bk[2])) / 3.0).clamp(0.0, 1.0);
            let (c, ao) = blend_over(mode, [p[0] * op, p[1] * op, p[2] * op], a, bk, ab);
            for k in 0..3 {
                rk[o + k] = (c[k].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                rw[o + k] = ((c[k] + 1.0 - ao).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
            rk[o + 3] = 255;
            rw[o + 3] = 255;
        }
    }
    draw_device_pixels(cb, rk, x0, y0, w, h);
    draw_device_pixels(cw, rw, x0, y0, w, h);
}

/// An isolated group: painted offscreen over black and over white in
/// lockstep (blended layers composite against the group's own, partly
/// transparent, backdrop), then composited normally.
fn paint_isolated(bounds: Rect, items: &[DisplayItem], canvas: &mut Canvas, fonts: &FontContext, st: &mut State, view: Rect) {
    let local = bounds.translate(st.dx, st.dy);
    if !st.ext {
        for it in items {
            paint_item(it, canvas, fonts, st, view);
        }
        return;
    }
    let Some((x0, y0, w, h)) = device_bounds(canvas, local) else { return };
    let m = rabbithole_style::multiply_matrix([1.0, 0.0, 0.0, 1.0, -(x0 as f32), -(y0 as f32)], canvas.transform());
    let plain = rabbithole_gfx::path::is_identity(&canvas.transform());
    let mut cb = Canvas::new(w as u32, h as u32);
    let mut cw = Canvas::new(w as u32, h as u32);
    cb.clear(Color::BLACK);
    cw.clear(Color::WHITE);
    let (sdx, sdy) = if plain { (st.dx - x0 as f32, st.dy - y0 as f32) } else { (st.dx, st.dy) };
    if !plain {
        cb.push_transform(m);
        cw.push_transform(m);
    }
    // Fixed items are in viewport coordinates: shift their origin with
    // the offscreen origin too.
    let fixed_origin = if plain { (st.fixed_origin.0 - x0 as f32, st.fixed_origin.1 - y0 as f32) } else { st.fixed_origin };
    let mut sb = State::nested(sdx, sdy, fixed_origin, st.ext);
    let mut sw = State::nested(sdx, sdy, fixed_origin, st.ext);
    let all = Rect::new(-1.0e7, -1.0e7, 2.0e7, 2.0e7);
    for it in items {
        if let DisplayItem::Blended { bounds, mode, opacity, items } = it {
            paint_blended_isolated(*bounds, *mode, *opacity, items, &mut cb, &mut cw, fonts, &sb);
            continue;
        }
        if let DisplayItem::PopLayer(opacity) = it {
            if sb.blend != 0 && sb.layers.last() == Some(&true) && sw.layers.last() == Some(&true) {
                sb.layers.pop();
                sw.layers.pop();
                let mode = std::mem::take(&mut sb.blend);
                sw.blend = 0;
                let mask = sb.mask.take();
                sw.mask = None;
                rabbithole_gfx::Canvas::end_layer_blend_isolated(&mut cb, &mut cw, *opacity, mode, mask.as_deref());
                continue;
            }
        }
        paint_item(it, &mut cb, fonts, &mut sb, all);
        paint_item(it, &mut cw, fonts, &mut sw, all);
    }
    let px = crate::filter::from_black_white(&cb.data, &cw.data, w, h);
    draw_device_pixels(canvas, crate::filter::to_rgba(&px), x0, y0, w, h);
}

/// `backdrop-filter`: filter the canvas pixels under `rect`.
#[allow(clippy::too_many_arguments)]
fn paint_backdrop(rect: Rect, radii: Radii, filters: &[rabbithole_style::FilterOp], svg: Option<&crate::filter::UrlFilters>, canvas: &mut Canvas, fonts: &FontContext, st: &mut State, view: Rect) {
    let local = snap(rect.translate(st.dx, st.dy), canvas);
    if !st.ext {
        return;
    }
    let _ = view;
    let Some((x0, y0, w, h)) = device_bounds(canvas, local) else { return };
    // The filter input is the backdrop within the border box; blurs see
    // it extended by mirroring at the box edges (no bleeding in from
    // outside).
    // Mirroring repeats the box beyond its size: no need for more margin.
    let e = (crate::filter::spread(filters) * device_scale(&canvas.transform())).ceil().min(w.max(h) as f32) as i32;
    let (pw, ph) = (w + 2 * e as usize, h + 2 * e as usize);
    let mirror = |i: i32, n: usize| -> usize {
        let n = n as i32;
        let m = if i < 0 { -i - 1 } else if i >= n { 2 * n - i - 1 } else { i };
        m.clamp(0, n - 1) as usize
    };
    let cw = canvas.width as usize;
    let mut data = Vec::with_capacity(pw * ph);
    for py in 0..ph {
        let y = y0 as usize + mirror(py as i32 - e, h);
        for px in 0..pw {
            let x = x0 as usize + mirror(px as i32 - e, w);
            data.push(canvas.data[y * cw + x]);
        }
    }
    let mut px = crate::filter::from_opaque(&data, pw, ph);
    match svg {
        Some(u) => {
            // Layout coordinates → buffer pixels.
            let m = rabbithole_style::multiply_matrix([1.0, 0.0, 0.0, 1.0, -((x0 - e) as f32), -((y0 - e) as f32)], canvas.transform());
            let lm = rabbithole_style::multiply_matrix(m, [1.0, 0.0, 0.0, 1.0, st.dx, st.dy]);
            px = u.apply(filters, px, &lm, device_scale(&canvas.transform()), fonts);
        }
        None => crate::filter::apply(&mut px, filters, device_scale(&canvas.transform())),
    }
    // Keep the box's part, composited over the original backdrop (drop
    // shadows / opacity may leave it partly transparent).
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let i = (y + e as usize) * pw + x + e as usize;
            let c = px.px[i];
            let o = data[i];
            let ch = |s: u32| ((o >> s) & 0xff) as f32 / 255.0;
            let inv = 1.0 - c[3];
            for (k, s) in [16u32, 8, 0].into_iter().enumerate() {
                rgba.push(((c[k] + ch(s) * inv).clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            }
            rgba.push(255);
        }
    }
    canvas.push_clip_rounded(local, radii);
    draw_device_pixels(canvas, rgba, x0, y0, w, h);
    canvas.pop_clip();
}

pub(crate) fn paint_item(item: &DisplayItem, canvas: &mut Canvas, fonts: &FontContext, st: &mut State, device_view: Rect) {
    let (dx, dy) = (st.dx, st.dy);
    let view = st.views.last().copied().unwrap_or(device_view);
    match item {
        DisplayItem::PushClip(r) => {
            let r = snap(r.translate(dx, dy), canvas);
            canvas.push_clip(r);
            st.clips.push(true);
        }
        DisplayItem::PushClipRounded(r, radii) => {
            let r = r.translate(dx, dy);
            if st.ext && radii.iter().any(|(h, v)| *h > 0.0 && *v > 0.0) {
                canvas.push_clip_rounded(r, translate_radii(*radii));
            } else {
                canvas.push_clip(r);
            }
            st.clips.push(true);
        }
        DisplayItem::PushClipPath { bounds, path, evenodd, anchor, rounded } => {
            if st.ext {
                let rule = if *evenodd { rabbithole_gfx::FillRule::EvenOdd } else { rabbithole_gfx::FillRule::NonZero };
                // Follow the box when `snap` moves it to device pixels.
                let (mut dx, mut dy) = (dx, dy);
                if rabbithole_gfx::path::is_identity(&canvas.transform()) {
                    let (ax, ay) = (anchor.x + dx, anchor.y + dy);
                    dx += ax.round() - ax;
                    dy += ay.round() - ay;
                }
                if let Some((r, radii)) = rounded.filter(|(_, q)| q.iter().any(|(h, v)| *h > 0.0 && *v > 0.0)) {
                    canvas.push_clip_rounded(r.translate(dx, dy), radii);
                } else if dx == 0.0 && dy == 0.0 {
                    canvas.push_clip_path(path, rule);
                } else {
                    canvas.push_clip_path(&rabbithole_style::shapes::translate_path(path, dx, dy), rule);
                }
            } else {
                canvas.push_clip(bounds.translate(dx, dy));
            }
            st.clips.push(true);
        }
        DisplayItem::PopClip => {
            if let Some(true) = st.clips.pop() {
                canvas.pop_clip();
            }
        }
        DisplayItem::PushTransform(m) => {
            if is_translation(m) {
                st.dx += m[4];
                st.dy += m[5];
                st.frames.push(Frame::Translate(m[4], m[5]));
            } else if st.ext {
                // Compose: canvas space = translate(dx,dy) · M · doc. The
                // canvas transform applies to coordinates we already
                // translated, so conjugate M by the current translation.
                let t = [1.0, 0.0, 0.0, 1.0, dx, dy];
                let ti = [1.0, 0.0, 0.0, 1.0, -dx, -dy];
                let mm = rabbithole_style::multiply_matrix(rabbithole_style::multiply_matrix(t, *m), ti);
                canvas.push_transform(mm);
                st.frames.push(Frame::Canvas);
                let local = match crate::tree::invert_matrix(canvas.transform()) {
                    Some(inv) => rabbithole_gfx::path::transform_rect_bounds(&inv, device_view),
                    None => Rect::new(0.0, 0.0, 0.0, 0.0),
                };
                st.views.push(local);
            } else {
                // Approximate unsupported transforms by their translation.
                st.dx += m[4];
                st.dy += m[5];
                st.frames.push(Frame::Translate(m[4], m[5]));
            }
        }
        DisplayItem::PopTransform => match st.frames.pop() {
            Some(Frame::Translate(x, y)) => {
                st.dx -= x;
                st.dy -= y;
            }
            Some(Frame::Canvas) => {
                canvas.pop_transform();
                st.views.pop();
            }
            _ => {}
        },
        DisplayItem::PushLayer(bounds) => {
            if st.ext {
                canvas.begin_layer(bounds.translate(dx, dy));
                st.layers.push(true);
            } else {
                st.layers.push(false);
            }
        }
        DisplayItem::PopLayer(opacity) => {
            if let Some(true) = st.layers.pop() {
                let blend = std::mem::take(&mut st.blend);
                match (st.mask.take(), blend) {
                    (alpha, b) if b != 0 => canvas.end_layer_blend(*opacity, b, alpha.as_deref()),
                    (Some(alpha), _) => canvas.end_layer_masked(*opacity, &alpha),
                    (None, _) => canvas.end_layer(*opacity),
                }
            }
        }
        DisplayItem::Filtered { bounds, filters, items, svg } => {
            paint_filtered(*bounds, filters, svg.as_deref(), items, canvas, fonts, st, view);
        }
        DisplayItem::BackdropFilter { rect, radii, filters, svg } => {
            paint_backdrop(*rect, *radii, filters, svg.as_deref(), canvas, fonts, st, view);
        }
        DisplayItem::Blended { bounds, mode, opacity, items } => {
            paint_blended(*bounds, *mode, *opacity, items, canvas, fonts, st, view);
        }
        DisplayItem::Isolated { bounds, items } => {
            paint_isolated(*bounds, items, canvas, fonts, st, view);
        }
        DisplayItem::Transform3d(g) => {
            if st.ext {
                crate::paint3d::paint_group(g, canvas, fonts, st, device_view);
            } else {
                for it in &g.items {
                    paint_item(it, canvas, fonts, st, device_view);
                }
            }
        }
        DisplayItem::LayerBlend(mode) => {
            if st.layers.last() == Some(&true) {
                st.blend = *mode;
            }
        }
        DisplayItem::LayerMask(items) => {
            st.mask = None;
            if st.layers.last() == Some(&true) {
                st.mask = layer_mask_alpha(items, canvas, fonts, st);
            }
        }
        DisplayItem::PushFixed => {
            // Fixed items are in viewport coordinates: drop the scroll.
            st.fixed_saved = Some((st.dx, st.dy));
            st.dx = st.fixed_origin.0;
            st.dy = st.fixed_origin.1;
        }
        DisplayItem::PopFixed => {
            if let Some((x, y)) = st.fixed_saved.take() {
                st.dx = x;
                st.dy = y;
            }
        }
        DisplayItem::Rect { rect, color, radius } => {
            let r = snap(rect.translate(dx, dy), canvas);
            if r.intersect(&view).is_empty() || color.is_transparent() {
                return;
            }
            if *radius > 0.5 {
                canvas.fill_rounded_rect(r, *radius, *color);
            } else {
                canvas.fill_rect(r, *color);
            }
        }
        DisplayItem::RectRadii { rect, radii, paint } => {
            let r = snap(rect.translate(dx, dy), canvas);
            if r.intersect(&view).is_empty() {
                return;
            }
            if st.ext {
                canvas.fill_rounded_rect_radii(r, *radii, &shift_paint(paint, dx, dy));
            } else {
                let c = average_color(paint);
                match uniform_radius(radii) {
                    Some(rad) if rad > 0.5 => canvas.fill_rounded_rect(r, rad, c),
                    _ => canvas.fill_rect(r, c),
                }
            }
        }
        DisplayItem::Border {
            rect,
            widths,
            colors,
            styles,
            radii,
        } => {
            let r = snap(rect.translate(dx, dy), canvas);
            if r.intersect(&view).is_empty() {
                return;
            }
            let mut w = *widths;
            let mut c = *colors;
            for i in 0..4 {
                if styles[i] == BorderStyle::None {
                    w[i] = 0.0;
                }
                if w[i] <= 0.0 {
                    c[i] = Color::TRANSPARENT;
                }
            }
            let s = [style_code(styles[0]), style_code(styles[1]), style_code(styles[2]), style_code(styles[3])];
            match uniform_radius(radii) {
                Some(rad) => canvas.stroke_rect(r, w, c, s, rad),
                None if st.ext => canvas.stroke_rect_radii(r, w, c, s, *radii),
                None => canvas.stroke_rect(r, w, c, s, radii[0].0.min(radii[0].1)),
            }
        }
        DisplayItem::BoxShadow {
            rect,
            radii,
            offset,
            blur,
            spread,
            color,
            inset,
        } => {
            let r = rect.translate(dx, dy);
            let reach = blur * 2.0 + spread.abs() + offset.0.abs().max(offset.1.abs());
            let test = Rect::new(r.x - reach, r.y - reach, r.w + 2.0 * reach, r.h + 2.0 * reach);
            if test.intersect(&view).is_empty() {
                return;
            }
            if st.ext {
                canvas.draw_box_shadow(r, *radii, *offset, *blur, *spread, *color, *inset);
            } else if !*inset {
                // Hard shadow fallback.
                let sr = Rect::new(r.x + offset.0 - spread, r.y + offset.1 - spread, r.w + 2.0 * spread, r.h + 2.0 * spread);
                let c = color.scale_alpha(if *blur > 0.0 { 0.5 } else { 1.0 });
                match uniform_radius(radii) {
                    Some(rad) if rad > 0.5 => canvas.fill_rounded_rect(sr, rad, c),
                    _ => canvas.fill_rect(sr, c),
                }
            }
        }
        DisplayItem::Text {
            x,
            y,
            font,
            size,
            color,
            text,
            letter_spacing,
            word_spacing,
            shadows,
            small_caps,
            oblique,
        } => {
            let (px, py) = (x + dx, y + dy);
            if py + size < view.y - 2.0 || py - size * 1.5 > view.bottom() + 2.0 || px > view.right() {
                return;
            }
            if *word_spacing != 0.0 {
                // A justified run: each word is drawn at its place, moved
                // by the extra space after the separators before it.
                let glyphs = fonts.shape(*font, *size, text, *letter_spacing);
                let mut start = 0;
                let mut seps = 0usize;
                for (i, c) in text.char_indices() {
                    let end = i + c.len_utf8();
                    if crate::tree::is_word_separator(c) || end == text.len() {
                        let off = glyphs.iter().find(|g| g.byte_index >= start).map_or(0.0, |g| g.x) + *word_spacing * seps as f32;
                        let item = DisplayItem::Text {
                            x: *x + off,
                            y: *y,
                            font: *font,
                            size: *size,
                            color: *color,
                            text: text[start..end].to_string(),
                            letter_spacing: *letter_spacing,
                            word_spacing: 0.0,
                            shadows: shadows.clone(),
                            small_caps: *small_caps,
                            oblique: *oblique,
                        };
                        paint_item(&item, canvas, fonts, st, device_view);
                        start = end;
                        seps += 1;
                    }
                }
                return;
            }
            if st.ext {
                for sh in shadows.iter().rev() {
                    canvas.draw_text_shadow(fonts, *font, *size, text, px + sh.x, py + sh.y, *letter_spacing, sh.color, sh.blur);
                }
            } else {
                for sh in shadows.iter().rev() {
                    draw_text_basic(canvas, fonts, *font, *size, text, px + sh.x, py + sh.y, sh.color, *letter_spacing);
                }
            }
            if color.a == 0 {
                // Invisible glyphs (only their shadows show).
            } else if st.ext && (*small_caps || *oblique || letter_spacing.abs() >= 0.01) {
                canvas.draw_text_ext(fonts, *font, *size, text, px, py, *color, *letter_spacing, *small_caps, *oblique);
            } else {
                draw_text_basic(canvas, fonts, *font, *size, text, px, py, *color, *letter_spacing);
            }
        }
        DisplayItem::Image { rect, src, image, opacity, pixelated } => {
            let r = snap(rect.translate(dx, dy), canvas);
            if r.intersect(&view).is_empty() {
                return;
            }
            let full = src.x <= 0.0
                && src.y <= 0.0
                && (src.w - image.width as f32).abs() < 0.5
                && (src.h - image.height as f32).abs() < 0.5;
            canvas.image_smoothing = !*pixelated;
            if image.vector.is_some() && src.w > 0.0 && src.h > 0.0 {
                // Render vector images at the device size they cover.
                let m = canvas.transform();
                let whole = (rect.w / src.w * image.width as f32, rect.h / src.h * image.height as f32);
                let sized = image.for_size(whole.0, whole.1, vector_scale(&m, whole.0, whole.1));
                let (fx, fy) = (sized.width as f32 / image.width as f32, sized.height as f32 / image.height as f32);
                let src2 = Rect::new(src.x * fx, src.y * fy, src.w * fx, src.h * fy);
                if full && st.ext {
                    canvas.draw_image(&sized, r, *opacity);
                } else if st.ext {
                    canvas.draw_image_region(&sized, src2, r, *opacity);
                } else {
                    canvas.draw_image(&sized, r, *opacity);
                }
            } else if full || !st.ext {
                canvas.draw_image(image, r, *opacity);
            } else {
                canvas.draw_image_region(image, *src, r, *opacity);
            }
            canvas.image_smoothing = true;
        }
        DisplayItem::ImageTiled {
            image,
            area,
            origin,
            tile,
            repeat_x,
            repeat_y,
            opacity,
            pixelated,
        } => {
            let a = snap(area.translate(dx, dy), canvas);
            if a.intersect(&view).is_empty() {
                return;
            }
            let o = if rabbithole_gfx::path::is_identity(&canvas.transform()) { Point { x: (origin.x + dx).round(), y: (origin.y + dy).round() } } else { Point { x: origin.x + dx, y: origin.y + dy } };
            canvas.image_smoothing = !*pixelated;
            let image = if image.vector.is_some() {
                let m = canvas.transform();
                image.for_size(tile.0, tile.1, vector_scale(&m, tile.0, tile.1))
            } else {
                image.clone()
            };
            let image = &image;
            if st.ext {
                canvas.draw_image_tiled(image, a, o, *tile, *repeat_x, *repeat_y, *opacity);
            } else {
                // Manual tiling with the basic API.
                canvas.push_clip(a);
                let (tw, th) = *tile;
                if tw > 0.5 && th > 0.5 {
                    let x0 = if *repeat_x { o.x - ((o.x - a.x) / tw).ceil() * tw } else { o.x };
                    let y0 = if *repeat_y { o.y - ((o.y - a.y) / th).ceil() * th } else { o.y };
                    let mut y = y0;
                    let mut guard = 0;
                    while y < a.bottom() && guard < 10_000 {
                        let mut x = x0;
                        while x < a.right() && guard < 10_000 {
                            canvas.draw_image(image, Rect::new(x, y, tw, th), *opacity);
                            guard += 1;
                            if !*repeat_x {
                                break;
                            }
                            x += tw;
                        }
                        if !*repeat_y {
                            break;
                        }
                        y += th;
                    }
                }
                canvas.pop_clip();
            }
            canvas.image_smoothing = true;
        }
        DisplayItem::Line {
            x0,
            y0,
            x1,
            y1,
            width,
            color,
            style,
        } => {
            let (ax, ay, bx, by) = (x0 + dx, y0 + dy, x1 + dx, y1 + dy);
            if ay.max(by) + width < view.y || ay.min(by) - width > view.bottom() || ax.max(bx) < view.x || ax.min(bx) > view.right() {
                return;
            }
            draw_decoration_line(canvas, ax, ay, bx, by, *width, *color, *style);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_text_basic(canvas: &mut Canvas, fonts: &FontContext, font: FontId, size: f32, text: &str, px: f32, py: f32, color: Color, letter_spacing: f32) {
    if letter_spacing.abs() < 0.01 {
        canvas.draw_text(fonts, font, size, text, px, py, color);
    } else {
        for g in fonts.shape(font, size, text, letter_spacing) {
            let bmp = fonts.rasterize(g.font, g.glyph, size);
            if bmp.width == 0 || bmp.height == 0 {
                continue;
            }
            let gx = (px + g.x + g.dx).round() as i32 + bmp.left;
            let gy = (py - g.dy).round() as i32 - bmp.top;
            canvas.draw_glyph(&bmp, gx, gy, color);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_decoration_line(canvas: &mut Canvas, ax: f32, ay: f32, bx: f32, by: f32, width: f32, color: Color, style: TextDecorationStyle) {
    let horizontal = (ay - by).abs() < 0.01;
    if !horizontal {
        canvas.draw_line(ax, ay, bx, by, width, color);
        return;
    }
    let (x0, x1) = (ax.min(bx), ax.max(bx));
    let y = ay - width / 2.0;
    match style {
        TextDecorationStyle::Solid => canvas.fill_rect(Rect::new(x0, y, x1 - x0, width), color),
        TextDecorationStyle::Double => {
            canvas.fill_rect(Rect::new(x0, y - width, x1 - x0, width), color);
            canvas.fill_rect(Rect::new(x0, y + width, x1 - x0, width), color);
        }
        TextDecorationStyle::Dotted | TextDecorationStyle::Dashed => {
            let dash = if style == TextDecorationStyle::Dotted { width.max(1.0) } else { width.max(1.0) * 3.0 };
            let gap = dash;
            let mut x = x0;
            let mut guard = 0;
            while x < x1 && guard < 100_000 {
                let w = dash.min(x1 - x);
                canvas.fill_rect(Rect::new(x, y, w, width), color);
                x += dash + gap;
                guard += 1;
            }
        }
        TextDecorationStyle::Wavy => {
            // A sine wave (Blink: amplitude ~1.5× the thickness, a period
            // of ~6× it), stroked as one polyline.
            let amp = (width * 1.5).max(1.0);
            let period = (width * 6.0).max(6.0);
            let mut path = vec![rabbithole_gfx::PathSeg::MoveTo(Point { x: x0, y: ay })];
            let n = (((x1 - x0) / 1.0).ceil() as usize).min(100_000);
            for i in 1..=n {
                let x = (x0 + i as f32).min(x1);
                let y = ay - amp * ((x - x0) / period * std::f32::consts::TAU).sin();
                path.push(rabbithole_gfx::PathSeg::LineTo(Point { x, y }));
            }
            let style = rabbithole_gfx::StrokeStyle { width: width.max(1.0), ..Default::default() };
            canvas.stroke_path(&path, &style, &Paint::Solid(color));
        }
    }
}

#[allow(dead_code)]
fn unused(_: &Shadow) {}

#[cfg(test)]
mod video_plane_tests {
    use super::*;
    use std::sync::Arc;
    use rabbithole_gfx::Image;

    #[test]
    fn promote_an_unobstructed_clipped_video_at_the_painted_scroll() {
        let image = Arc::new(Image::new(4, 4, vec![255; 64]));
        let src = Rect::new(1.0, 0.0, 3.0, 4.0);
        let mut list = DisplayList::default();
        let rect = Rect::new(5.0, 4.0, 18.0, 16.0);
        let viewport = Rect::new(0.0, 52.0, 80.0, 50.0);
        list.items = vec![
            DisplayItem::PushFixed,
            DisplayItem::PushClip(Rect::new(5.0, 5.0, 10.0, 9.0)),
            DisplayItem::Image { rect, src, image: image.clone(), opacity: 1.0, pixelated: true },
            DisplayItem::PopClip, DisplayItem::PopFixed,
        ];
        let plane = video_plane(&list, &image, viewport, 2.0, &FontContext::new()).unwrap();
        assert_eq!(plane.dest.y, 56.0);
        assert_eq!((plane.clip.y, plane.clip.h), (57.0, 9.0));
        assert_eq!((plane.src.x, plane.src.w), (1.0, 3.0));
        assert!(plane.pixelated);
        list.items.push(DisplayItem::Rect { rect: plane.clip, color: Color::WHITE, radius: 0.0 });
        assert!(video_plane(&list, &image, viewport, 2.0, &FontContext::new()).is_none());
        list.items.pop();
        list.items.insert(0, DisplayItem::PushLayer(viewport));
        list.items.push(DisplayItem::PopLayer(0.5));
        assert!(video_plane(&list, &image, viewport, 2.0, &FontContext::new()).is_none());
    }

    #[test]
    fn fractional_video_matches_updated_content_on_the_device_grid() {
        let fonts = FontContext::new();
        let viewport = Rect::new(0.0, 8.0, 64.0, 48.0);
        let background = Color::rgb(27, 53, 91);
        let picture = |phase: u8| {
            let pixels = (0..72*48).flat_map(|i| [(i*19) as u8 ^ phase, (i*81) as u8,
                (i*13) as u8 ^ phase, 255]).collect();
            let mut image = Image::new(72, 48, pixels);
            image.video_frame = true;
            Arc::new(image)
        };
        let old = picture(0);
        let next = picture(219);
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let small = Rect::new(0.5, 0.25, 19.0, 15.0);
            let large = Rect::new(1.5, 0.25, 57.0, 41.0);
            for (pixelated, src) in [(false, small), (true, small), (false, large), (true, large)] {
                for controls in [false, true] {
                    let rect = Rect::new(3.25, 12.75, 26.5, 15.75);
                    let scene = |image: Arc<Image>| {
                        let mut list = DisplayList::default();
                        list.items.push(DisplayItem::Rect { rect: Rect::new(2.0, 9.0, 13.0, 24.0),
                            color: Color::rgb(187, 91, 29), radius: 0.0 });
                        list.items.push(DisplayItem::PushClip(Rect::new(4.25, 8.0, 38.0, 33.75)));
                        list.items.push(DisplayItem::Image { rect, src,
                            image, opacity: 1.0, pixelated });
                        if controls {
                            list.items.push(DisplayItem::Rect { rect: Rect::new(13.5, 16.0, 22.0, 7.5),
                                color: Color::rgba(170, 80, 10, 140), radius: 1.0 });
                        }
                        list.items.push(DisplayItem::PopClip);
                        list
                    };
                    let paint = |list: &DisplayList| {
                        let mut c = Canvas::new((viewport.w * scale) as u32, (viewport.h * scale) as u32);
                        c.clear(background);
                        c.push_transform([scale, 0.0, 0.0, scale, 0.0, 0.0]);
                        paint_with_fixed_origin(list, &mut c, &fonts, viewport.x, viewport.y, (0.0, 0.0));
                        c.pop_transform();
                        c
                    };
                    let first = scene(old.clone());
                    let mut actual = paint(&first);
                    let plane = video_plane_on_background(&first, &old, viewport, scale, &fonts, background).unwrap();
                    let area = Rect::new(0.0, 0.0, actual.width as f32, actual.height as f32);
                    for image in [&next, &old] {
                        let expected = paint(&scene(Arc::clone(image)));
                        assert!(plane.composite(&mut actual, image.pixels(), (72, 48), area,
                            (viewport.x, viewport.y), scale));
                        for (i, (&a,&b)) in actual.data.iter().zip(&expected.data).enumerate() {
                            for shift in [0,8,16] {
                                assert!(((a>>shift)&255u32).abs_diff((b>>shift)&255) <= if controls { 2 } else { 0 },
                                    "scale={scale} pixelated={pixelated} controls={controls} pixel={i} {a:x} {b:x}");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn scrolled_video_matches_the_overscanned_page_blit() {
        let fonts = FontContext::new();
        let picture = |value: u8| {
            let mut image = Image::new(8, 6, (0..48).flat_map(|i|
                [value ^ (i * 19) as u8, (i * 7) as u8, value, 255]).collect());
            image.video_frame = true;
            Arc::new(image)
        };
        let old = picture(0x38);
        let new = picture(0xb5);
        let scene = |picture: &Arc<Image>, fixed: bool| {
            let mut list = DisplayList::default();
            if fixed { list.items.push(DisplayItem::PushFixed); }
            list.items.push(DisplayItem::PushClip(Rect::new(2.5, 3.75, 21.5, 24.75)));
            list.items.push(DisplayItem::Image { rect: Rect::new(1.75, 4.5, 18.5, 15.25),
                src: Rect::new(0.0, 0.0, 8.0, 6.0), image: picture.clone(), opacity: 1.0, pixelated: false });
            list.items.push(DisplayItem::PopClip);
            if fixed { list.items.push(DisplayItem::PopFixed); }
            list.items.push(DisplayItem::PushFixed);
            list.items.push(DisplayItem::Rect { rect: Rect::new(4.25, 4.5, 11.0, 2.75),
                color: Color::rgba(110, 43, 92, 131), radius: 0.0 });
            list.items.push(DisplayItem::PopFixed);
            list
        };
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for phase in [0.25, 0.5, 0.75] {
                let canvas_y = 2.0 / scale;
                let viewport = Rect::new(0.0, canvas_y + (5.0 + phase) / scale, 32.0, 24.0);
                for fixed in [false, true] {
                    let render = |list: &DisplayList| {
                        let mut backing = Canvas::new((viewport.w * scale) as u32, (40.0 * scale) as u32);
                        backing.clear(Color::WHITE);
                        backing.push_transform([scale, 0.0, 0.0, scale, 0.0, 0.0]);
                        paint_with_fixed_origin(list, &mut backing, &fonts, 0.0, canvas_y, (0.0, viewport.y - canvas_y));
                        let mut view = Canvas::new(backing.width, (viewport.h * scale) as u32);
                        let top = ((viewport.y - canvas_y) * scale).round() as usize * backing.width as usize;
                        let len = view.data.len();
                        view.data.copy_from_slice(&backing.data[top..top + len]);
                        view
                    };
                    let first = scene(&old, fixed);
                    let mut actual = render(&first);
                    let expected = render(&scene(&new, fixed));
                    let plane = video_plane_in_raster(&first, &old, viewport, scale, &fonts, Color::WHITE, canvas_y).unwrap();
                    let area = Rect::new(0.0, 0.0, actual.width as f32, actual.height as f32);
                    assert!(plane.composite(&mut actual, new.pixels(), (8, 6), area, (0.0, viewport.y), scale));
                    for (i, (&a, &b)) in actual.data.iter().zip(&expected.data).enumerate() {
                        for shift in [0, 8, 16] {
                            assert!(((a >> shift) & 255u32).abs_diff((b >> shift) & 255) <= 2,
                                "scale {scale}, phase {phase}, fixed {fixed}, pixel {i}: {a:x} vs {b:x}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn translucent_controls_and_subtitles_follow_video_scroll_and_zoom() {
        let fonts = FontContext::new();
        let mut image = Image::new(16, 16, [210, 92, 31, 255].repeat(256));
        image.video_frame = true;
        let image = Arc::new(image);
        let rect = Rect::new(4.0, 12.0, 48.0, 32.0);
        let viewport = Rect::new(0.0, 8.0, 64.0, 48.0);
        let mut list = DisplayList::default();
        list.items.push(DisplayItem::Image { rect, src: Rect::new(0.0, 0.0, 16.0, 16.0), image: image.clone(), opacity: 1.0, pixelated: true });
        list.items.push(DisplayItem::PushLayer(rect));
        list.items.push(DisplayItem::Rect { rect: Rect::new(6.0, 21.0, 38.0, 19.0), color: Color::rgba(10, 40, 90, 148), radius: 2.0 });
        list.items.push(DisplayItem::Text {
            x: 7.0, y: 34.0, font: FontId(0), size: 12.0, color: Color::WHITE, text: "Play".into(),
            letter_spacing: 0.0, word_spacing: 0.0, shadows: Vec::new(), small_caps: false, oblique: false,
        });
        list.items.push(DisplayItem::PopLayer(0.8));
        // An affine transform on a control uses the normal foreground painter.
        list.items.push(DisplayItem::PushTransform([0.0, 1.0, -1.0, 0.0, 48.0, 0.0]));
        list.items.push(DisplayItem::Rect { rect: Rect::new(18.0, 7.0, 6.0, 4.0), color: Color::rgba(21, 140, 32, 96), radius: 0.0 });
        list.items.push(DisplayItem::PopTransform);
        for scale in [1.0, 1.5, 2.0] {
            let mut a = Canvas::new((64.0 * scale) as u32, (48.0 * scale) as u32);
            let mut b = Canvas::new(a.width, a.height);
            a.push_transform([scale, 0.0, 0.0, scale, 0.0, 0.0]);
            paint_with_fixed_origin(&list, &mut a, &fonts, 0.0, 8.0, (0.0, 0.0));
            let v = video_plane(&list, &image, viewport, scale, &fonts).unwrap();
            let dest = Rect::new(rect.x * scale, (rect.y - 8.0) * scale, rect.w * scale, rect.h * scale);
            assert!(b.composite_video(image.pixels(), (16, 16), v.src, dest, dest, true));
            let before = b.data.clone();
            v.foreground.unwrap().paint(&mut b, dest, dest);
            assert_ne!(before, b.data);
            for (i, (reference, overlay)) in a.data.iter().zip(&b.data).enumerate() {
                for shift in [0, 8, 16] {
                    let (r, o) = ((reference >> shift) & 255u32, (overlay >> shift) & 255u32);
                    assert!(r.abs_diff(o) <= 2, "scale {scale} pixel {i}: {reference:x} vs {overlay:x}");
                }
            }
        }
        list.items.push(DisplayItem::Filtered { bounds: rect, filters: Vec::new(), svg: None, items: Vec::new() });
        assert!(video_plane(&list, &image, viewport, 1.0, &fonts).is_none());
    }
}
