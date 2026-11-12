//! CSS Backgrounds and Borders Level 3: background layers (colors, images,
//! gradients, sizing, positioning, repeat, origin/clip, attachment), per
//! corner radii and box shadows, emitted as display items.

use std::sync::Arc;
use crate::display_list::{DisplayItem, DisplayList};
use crate::tree::LayoutBox;
use rabbithole_gfx::{Color, ColorStop, Image, Paint, Point, Radii, Rect};
use rabbithole_style::{
    BackgroundImage, BackgroundLayer, BackgroundRepeat, BackgroundSize, BoxArea, ComputedStyle, Edges, GradientDirection,
    GradientStop, Length, RadialShape, RadialSize, Shadow,
};
use std::collections::HashMap;

/// The three boxes of an element plus its radii.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BoxGeometry {
    pub border: Rect,
    pub padding: Rect,
    pub content: Rect,
    /// Outer (border-box) radii.
    pub radii: Radii,
    pub border_width: Edges<f32>,
}

/// Resolved border-box radii of a laid out box.
pub(crate) fn box_radii(b: &LayoutBox) -> Radii {
    let r = b.style.resolved_radii(b.rect.w, b.rect.h);
    if r.iter().all(|(h, v)| *h <= 0.0 || *v <= 0.0) && b.style.border_radius > 0.0 {
        // Older single-value radius (kept for compatibility).
        let v = b.style.border_radius.min(b.rect.w / 2.0).min(b.rect.h / 2.0).max(0.0);
        return [(v, v); 4];
    }
    r
}

/// Radii of the padding box given the border-box radii (CSS Backgrounds 3
/// §5.2: inner radii are the outer radii minus the border widths).
pub(crate) fn inner_radii(outer: Radii, bw: &Edges<f32>) -> Radii {
    let sub = |r: f32, w: f32| (r - w).max(0.0);
    [
        (sub(outer[0].0, bw.left), sub(outer[0].1, bw.top)),
        (sub(outer[1].0, bw.right), sub(outer[1].1, bw.top)),
        (sub(outer[2].0, bw.right), sub(outer[2].1, bw.bottom)),
        (sub(outer[3].0, bw.left), sub(outer[3].1, bw.bottom)),
    ]
}

pub(crate) fn has_radii(r: &Radii) -> bool {
    r.iter().any(|(h, v)| *h > 0.0 && *v > 0.0)
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

/// Emit a solid fill, using the fast `Rect` item when the radii are uniform.
pub(crate) fn push_fill(list: &mut DisplayList, rect: Rect, radii: Radii, color: Color) {
    if color.is_transparent() || rect.is_empty() {
        return;
    }
    match uniform_radius(&radii) {
        Some(r) => list.items.push(DisplayItem::Rect { rect, color, radius: r }),
        None => list.items.push(DisplayItem::RectRadii {
            rect,
            radii,
            paint: Paint::Solid(color),
        }),
    }
}

fn area_rect(geom: &BoxGeometry, area: BoxArea) -> Rect {
    match area {
        BoxArea::BorderBox => geom.border,
        BoxArea::PaddingBox => geom.padding,
        BoxArea::ContentBox => geom.content,
    }
}

fn area_radii(geom: &BoxGeometry, area: BoxArea) -> Radii {
    match area {
        BoxArea::BorderBox => geom.radii,
        BoxArea::PaddingBox => inner_radii(geom.radii, &geom.border_width),
        BoxArea::ContentBox => {
            let pad = Edges {
                top: geom.content.y - geom.padding.y,
                right: geom.padding.right() - geom.content.right(),
                bottom: geom.padding.bottom() - geom.content.bottom(),
                left: geom.content.x - geom.padding.x,
            };
            inner_radii(inner_radii(geom.radii, &geom.border_width), &pad)
        }
    }
}

/// Paint the box shadows of one box (outer ones when `inset == false`).
/// Later shadows in the list are painted first (CSS order: first = top).
pub(crate) fn paint_box_shadows(list: &mut DisplayList, geom: &BoxGeometry, shadows: &[Shadow], inset: bool) {
    for sh in shadows.iter().rev() {
        if sh.inset != inset || sh.color.is_transparent() {
            continue;
        }
        let (rect, radii) = if inset {
            (geom.padding, inner_radii(geom.radii, &geom.border_width))
        } else {
            (geom.border, geom.radii)
        };
        list.items.push(DisplayItem::BoxShadow {
            rect,
            radii,
            offset: (sh.x, sh.y),
            blur: sh.blur.max(0.0),
            spread: sh.spread,
            color: sh.color,
            inset,
        });
    }
}

/// Paint `background-color` and all `background-image` layers.
pub(crate) fn paint_backgrounds(
    list: &mut DisplayList,
    geom: &BoxGeometry,
    style: &ComputedStyle,
    images: &HashMap<String, Arc<Image>>,
    viewport: Rect,
) {
    let layers: Vec<BackgroundLayer> = if style.background_layers.is_empty() {
        match &style.background_image {
            Some(url) => vec![BackgroundLayer {
                image: BackgroundImage::Url(url.clone()),
                ..BackgroundLayer::default()
            }],
            None => Vec::new(),
        }
    } else {
        style.background_layers.clone()
    };
    // Background color is clipped by the last layer's clip (or border-box).
    let color_clip = layers.last().map(|l| l.clip).unwrap_or(BoxArea::BorderBox);
    if !style.background_color.is_transparent() {
        push_fill(list, area_rect(geom, color_clip), area_radii(geom, color_clip), style.background_color);
    }
    // Layers: the first layer is on top, so paint from the last.
    for layer in layers.iter().rev() {
        if matches!(layer.image, BackgroundImage::None) {
            continue;
        }
        paint_layer(list, geom, layer, images, viewport);
    }
}

fn paint_layer(list: &mut DisplayList, geom: &BoxGeometry, layer: &BackgroundLayer, images: &HashMap<String, Arc<Image>>, viewport: Rect) {
    let clip_rect = area_rect(geom, layer.clip);
    let clip_radii = area_radii(geom, layer.clip);
    if clip_rect.is_empty() {
        return;
    }
    let area = if layer.fixed { viewport } else { area_rect(geom, layer.origin) };
    let image = match &layer.image {
        BackgroundImage::Url(url) => match images.get(url) {
            Some(img) => Some(img.clone()),
            None => return,
        },
        _ => None,
    };
    let intrinsic = image.as_ref().map(|i| i.natural_size());
    let (tw, th) = tile_size(layer.size, intrinsic, area);
    if tw <= 0.0 || th <= 0.0 {
        return;
    }
    // Round: adjust tile size so a whole number of tiles fits.
    let (tw, th) = {
        let mut tw = tw;
        let mut th = th;
        if layer.repeat.0 == BackgroundRepeat::Round && area.w > 0.0 {
            let n = (area.w / tw).round().max(1.0);
            tw = area.w / n;
        }
        if layer.repeat.1 == BackgroundRepeat::Round && area.h > 0.0 {
            let n = (area.h / th).round().max(1.0);
            th = area.h / n;
        }
        (tw, th)
    };
    let px = bg_position(layer.position.0, area.w, tw);
    let py = bg_position(layer.position.1, area.h, th);
    let tile = Rect::new(area.x + px, area.y + py, tw, th);
    let repeat_x = !matches!(layer.repeat.0, BackgroundRepeat::NoRepeat);
    let repeat_y = !matches!(layer.repeat.1, BackgroundRepeat::NoRepeat);

    let clipped = has_radii(&clip_radii);
    if clipped {
        list.items.push(DisplayItem::PushClipRounded(clip_rect, clip_radii));
    } else {
        list.items.push(DisplayItem::PushClip(clip_rect));
    }
    let space_x = layer.repeat.0 == BackgroundRepeat::Space;
    let space_y = layer.repeat.1 == BackgroundRepeat::Space;
    match (&layer.image, image) {
        (BackgroundImage::Url(_), Some(img)) if space_x || space_y => {
            // Tile origins along one axis: (start, count, step).
            let axis = |space: bool, repeat: bool, start: f32, len: f32, size: f32, pos: f32, clip0: f32, clip1: f32| -> Vec<f32> {
                if space {
                    let n = (len / size + 0.001).floor();
                    if n >= 2.0 {
                        let step = size + (len - n * size) / (n - 1.0);
                        // Repeat the spaced tiles over the painting area too.
                        let mut first = start;
                        while first - step + size > clip0 {
                            first -= step;
                        }
                        let mut out = Vec::new();
                        let mut x = first;
                        while x < clip1 && out.len() < 10_000 {
                            out.push(x);
                            x += step;
                        }
                        return out;
                    }
                    if n < 1.0 {
                        return Vec::new();
                    }
                    return vec![start + pos];
                }
                if repeat {
                    let mut first = start + pos;
                    while first > clip0 {
                        first -= size;
                    }
                    let mut out = Vec::new();
                    let mut x = first;
                    while x < clip1 && out.len() < 10_000 {
                        out.push(x);
                        x += size;
                    }
                    return out;
                }
                vec![start + pos]
            };
            let xs = axis(space_x, repeat_x, area.x, area.w, tw, px, clip_rect.x, clip_rect.right());
            let ys = axis(space_y, repeat_y, area.y, area.h, th, py, clip_rect.y, clip_rect.bottom());
            if xs.len() * ys.len() <= 4096 {
                for &y in &ys {
                    for &x in &xs {
                        let r = Rect::new(x, y, tw, th);
                        if r.intersect(&clip_rect).is_empty() {
                            continue;
                        }
                        list.items.push(DisplayItem::Image {
                            rect: r,
                            src: Rect::new(0.0, 0.0, img.width as f32, img.height as f32),
                            image: img.clone(),
                            opacity: 1.0,
                            pixelated: false,
                        });
                    }
                }
            }
        }
        (BackgroundImage::Url(_), Some(img)) => {
            if repeat_x || repeat_y {
                let mut x0 = tile.x;
                let mut y0 = tile.y;
                let mut w = tile.w;
                let mut h = tile.h;
                if repeat_x {
                    x0 = clip_rect.x;
                    w = clip_rect.w;
                }
                if repeat_y {
                    y0 = clip_rect.y;
                    h = clip_rect.h;
                }
                let area_r = Rect::new(x0, y0, w, h).intersect(&clip_rect);
                if !area_r.is_empty() {
                    list.items.push(DisplayItem::ImageTiled {
                        image: img,
                        area: area_r,
                        origin: Point { x: tile.x, y: tile.y },
                        tile: (tw, th),
                        repeat_x,
                        repeat_y,
                        opacity: 1.0,
                    });
                }
            } else {
                let shown = tile.intersect(&clip_rect);
                if !shown.is_empty() {
                    let src = Rect::new(
                        (shown.x - tile.x) / tw * img.width as f32,
                        (shown.y - tile.y) / th * img.height as f32,
                        shown.w / tw * img.width as f32,
                        shown.h / th * img.height as f32,
                    );
                    list.items.push(DisplayItem::Image {
                        rect: shown,
                        src,
                        image: img,
                        opacity: 1.0,
                        pixelated: false,
                    });
                }
            }
        }
        (BackgroundImage::LinearGradient { direction, stops, repeating }, _) => {
            let paint = linear_paint(direction, stops, *repeating, tile);
            let fill = gradient_fill_rect(tile, clip_rect, repeat_x, repeat_y);
            if let (Some(paint), false) = (paint, fill.is_empty()) {
                list.items.push(DisplayItem::RectRadii {
                    rect: fill,
                    radii: [(0.0, 0.0); 4],
                    paint,
                });
            }
        }
        (BackgroundImage::RadialGradient { shape, size, position, stops, repeating }, _) => {
            let paint = radial_paint(shape, size, *position, stops, *repeating, tile);
            let fill = gradient_fill_rect(tile, clip_rect, repeat_x, repeat_y);
            if let (Some(paint), false) = (paint, fill.is_empty()) {
                list.items.push(DisplayItem::RectRadii {
                    rect: fill,
                    radii: [(0.0, 0.0); 4],
                    paint,
                });
            }
        }
        _ => {}
    }
    list.items.push(DisplayItem::PopClip);
}

/// Gradients repeat seamlessly only when the tile covers the area; when
/// repeating, fill the whole clip rect (approximation), else the tile.
fn gradient_fill_rect(tile: Rect, clip: Rect, repeat_x: bool, repeat_y: bool) -> Rect {
    let mut r = tile;
    if repeat_x {
        r.x = clip.x;
        r.w = clip.w;
    }
    if repeat_y {
        r.y = clip.y;
        r.h = clip.h;
    }
    r.intersect(&clip)
}

/// Resolve `background-size` to a tile size.
fn tile_size(size: BackgroundSize, intrinsic: Option<(f32, f32)>, area: Rect) -> (f32, f32) {
    let (iw, ih) = intrinsic.unwrap_or((area.w, area.h));
    let ratio = if ih > 0.0 { iw / ih } else { 0.0 };
    match size {
        BackgroundSize::Auto => (iw, ih),
        BackgroundSize::Cover | BackgroundSize::Contain => {
            if iw <= 0.0 || ih <= 0.0 {
                return (area.w, area.h);
            }
            let sx = area.w / iw;
            let sy = area.h / ih;
            let s = if matches!(size, BackgroundSize::Cover) { sx.max(sy) } else { sx.min(sy) };
            (iw * s, ih * s)
        }
        BackgroundSize::Explicit(w, h) => {
            let rw = w.resolve(area.w);
            let rh = h.resolve(area.h);
            match (rw, rh) {
                (Some(w), Some(h)) => (w, h),
                (Some(w), None) => (w, if ratio > 0.0 { w / ratio } else { ih }),
                (None, Some(h)) => (if ratio > 0.0 { h * ratio } else { iw }, h),
                (None, None) => (iw, ih),
            }
        }
    }
}

/// `background-position` component: percentages position the image so that
/// the same percentage point of the image and the area coincide.
fn bg_position(l: Length, area: f32, tile: f32) -> f32 {
    match l {
        Length::Percent(p) => (area - tile) * p / 100.0,
        Length::Calc(..) | Length::Math(_) => l.resolve_or_zero(area - tile),
        other => other.resolve_or_zero(area),
    }
}

/// Resolve gradient stops to offsets in 0..=1 along a line of length `len`
/// (CSS Images 3 §3.4.3).
pub(crate) fn resolve_stops(stops: &[GradientStop], len: f32) -> Vec<ColorStop> {
    if stops.is_empty() {
        return Vec::new();
    }
    let n = stops.len();
    let mut pos: Vec<Option<f32>> = stops
        .iter()
        .map(|s| {
            s.position.map(|p| match p {
                Length::Percent(v) => v / 100.0,
                other => {
                    if len > 0.0 {
                        other.resolve_or_zero(len) / len
                    } else {
                        0.0
                    }
                }
            })
        })
        .collect();
    if pos[0].is_none() {
        pos[0] = Some(0.0);
    }
    if pos[n - 1].is_none() {
        pos[n - 1] = Some(1.0);
    }
    // Positions never decrease.
    let mut max = f32::MIN;
    for p in pos.iter_mut().flatten() {
        if *p < max {
            *p = max;
        }
        max = *p;
    }
    // Evenly distribute runs of unspecified positions.
    let mut i = 0;
    while i < n {
        if pos[i].is_none() {
            let start = i - 1;
            let mut j = i;
            while j < n && pos[j].is_none() {
                j += 1;
            }
            let a = pos[start].unwrap_or(0.0);
            let b = pos[j].unwrap_or(1.0);
            let count = (j - start) as f32;
            for (k, slot) in pos.iter_mut().enumerate().take(j).skip(i) {
                *slot = Some(a + (b - a) * (k - start) as f32 / count);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    stops
        .iter()
        .zip(pos)
        .map(|(s, p)| ColorStop {
            offset: p.unwrap_or(0.0),
            color: s.color,
        })
        .collect()
}

/// Angle (radians, 0 = to top, clockwise) of a gradient direction.
pub(crate) fn direction_angle(direction: &GradientDirection, w: f32, h: f32) -> f32 {
    match direction {
        GradientDirection::Angle(a) => *a,
        GradientDirection::Corner(dx, dy) => {
            let (dx, dy) = (*dx as f32, *dy as f32);
            if dx == 0.0 && dy == 0.0 {
                return std::f32::consts::PI;
            }
            if dx == 0.0 || dy == 0.0 {
                (dx).atan2(-dy)
            } else {
                // Corner: the gradient line is perpendicular to the other
                // diagonal (CSS Images 3 §3.1.1).
                (dx * h).atan2(-dy * w)
            }
        }
    }
}

/// Build a linear gradient paint for `tile`.
pub(crate) fn linear_paint(direction: &GradientDirection, stops: &[GradientStop], repeating: bool, tile: Rect) -> Option<Paint> {
    if stops.is_empty() {
        return None;
    }
    let angle = direction_angle(direction, tile.w, tile.h);
    let (sin, cos) = angle.sin_cos();
    let len = (tile.w * sin).abs() + (tile.h * cos).abs();
    let cx = tile.x + tile.w / 2.0;
    let cy = tile.y + tile.h / 2.0;
    let dx = sin * len / 2.0;
    let dy = -cos * len / 2.0;
    let mut resolved = resolve_stops(stops, len);
    if resolved.len() == 1 {
        resolved.push(resolved[0]);
    }
    Some(Paint::Linear {
        start: Point { x: cx - dx, y: cy - dy },
        end: Point { x: cx + dx, y: cy + dy },
        stops: resolved,
        repeat: repeating,
    })
}

/// Build a radial gradient paint for `tile` (CSS Images 3 §3.2).
pub(crate) fn radial_paint(
    shape: &RadialShape,
    size: &RadialSize,
    position: (Length, Length),
    stops: &[GradientStop],
    repeating: bool,
    tile: Rect,
) -> Option<Paint> {
    if stops.is_empty() {
        return None;
    }
    let cx = tile.x + position.0.resolve_or_zero(tile.w);
    let cy = tile.y + position.1.resolve_or_zero(tile.h);
    let dl = (cx - tile.x).abs();
    let dr = (tile.right() - cx).abs();
    let dt = (cy - tile.y).abs();
    let db = (tile.bottom() - cy).abs();
    let circle = matches!(shape, RadialShape::Circle);
    let (rx, ry) = match size {
        RadialSize::ClosestSide => {
            if circle {
                let r = dl.min(dr).min(dt).min(db);
                (r, r)
            } else {
                (dl.min(dr), dt.min(db))
            }
        }
        RadialSize::FarthestSide => {
            if circle {
                let r = dl.max(dr).max(dt).max(db);
                (r, r)
            } else {
                (dl.max(dr), dt.max(db))
            }
        }
        RadialSize::ClosestCorner | RadialSize::FarthestCorner => {
            let corners = [(dl, dt), (dr, dt), (dl, db), (dr, db)];
            let far = matches!(size, RadialSize::FarthestCorner);
            if circle {
                let d = corners
                    .iter()
                    .map(|(x, y)| (x * x + y * y).sqrt())
                    .fold(if far { f32::MIN } else { f32::MAX }, |a, b| if far { a.max(b) } else { a.min(b) });
                (d, d)
            } else {
                // Same aspect ratio as the corresponding *-side ellipse,
                // passing through the corner.
                let (sx, sy) = if far { (dl.max(dr), dt.max(db)) } else { (dl.min(dr), dt.min(db)) };
                let ratio = if sy > 0.0 { sx / sy } else { 1.0 };
                let (cxr, cyr) = corners
                    .iter()
                    .copied()
                    .map(|(x, y)| ((x * x + (y * ratio) * (y * ratio)).sqrt(), (x, y)))
                    .fold(
                        (if far { f32::MIN } else { f32::MAX }, (0.0, 0.0)),
                        |acc, (d, c)| if (far && d > acc.0) || (!far && d < acc.0) { (d, c) } else { acc },
                    )
                    .1;
                let rx = (cxr * cxr + (cyr * ratio) * (cyr * ratio)).sqrt();
                let ry = if ratio > 0.0 { rx / ratio } else { rx };
                (rx, ry)
            }
        }
        RadialSize::Explicit(a, b) => {
            let rx = a.resolve_or_zero(tile.w);
            let ry = if circle { rx } else { b.resolve_or_zero(tile.h) };
            (rx, ry)
        }
    };
    let rx = rx.max(0.01);
    let ry = ry.max(0.01);
    let mut resolved = resolve_stops(stops, rx);
    if resolved.len() == 1 {
        resolved.push(resolved[0]);
    }
    Some(Paint::Radial {
        center: Point { x: cx, y: cy },
        rx,
        ry,
        stops: resolved,
        repeat: repeating,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_distribute_evenly() {
        let stops = vec![
            GradientStop { color: Color::BLACK, position: None },
            GradientStop { color: Color::WHITE, position: None },
            GradientStop { color: Color::BLACK, position: None },
        ];
        let r = resolve_stops(&stops, 100.0);
        assert_eq!(r.iter().map(|s| s.offset).collect::<Vec<_>>(), vec![0.0, 0.5, 1.0]);
        let stops = vec![
            GradientStop { color: Color::BLACK, position: Some(Length::Px(20.0)) },
            GradientStop { color: Color::WHITE, position: Some(Length::Percent(10.0)) },
        ];
        let r = resolve_stops(&stops, 100.0);
        assert_eq!(r[1].offset, 0.2, "never decreasing");
    }

    #[test]
    fn angles() {
        let w = 2.0;
        let h = 1.0;
        assert!((direction_angle(&GradientDirection::Corner(0, -1), w, h)).abs() < 1e-6);
        assert!((direction_angle(&GradientDirection::Corner(1, 0), w, h) - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        let tr = direction_angle(&GradientDirection::Corner(1, -1), w, h);
        assert!((tr - (1.0f32).atan2(2.0)).abs() < 1e-6, "{tr}");
    }

    #[test]
    fn linear_geometry() {
        let tile = Rect::new(0.0, 0.0, 100.0, 50.0);
        let stops = vec![
            GradientStop { color: Color::BLACK, position: None },
            GradientStop { color: Color::WHITE, position: None },
        ];
        match linear_paint(&GradientDirection::Angle(std::f32::consts::FRAC_PI_2), &stops, false, tile).unwrap() {
            Paint::Linear { start, end, .. } => {
                assert!((start.x).abs() < 1e-3 && (end.x - 100.0).abs() < 1e-3, "{start:?} {end:?}");
                assert!((start.y - 25.0).abs() < 1e-3);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn sizes_and_positions() {
        let area = Rect::new(0.0, 0.0, 200.0, 100.0);
        assert_eq!(tile_size(BackgroundSize::Cover, Some((50.0, 50.0)), area), (200.0, 200.0));
        assert_eq!(tile_size(BackgroundSize::Contain, Some((50.0, 50.0)), area), (100.0, 100.0));
        assert_eq!(tile_size(BackgroundSize::Explicit(Length::Px(40.0), Length::Auto), Some((50.0, 25.0)), area), (40.0, 20.0));
        assert_eq!(bg_position(Length::Percent(50.0), 200.0, 50.0), 75.0);
        assert_eq!(bg_position(Length::Px(10.0), 200.0, 50.0), 10.0);
    }
}
