//! CSS Backgrounds and Borders Level 3: background layers (colors, images,
//! gradients, sizing, positioning, repeat, origin/clip, attachment), per
//! corner radii and box shadows, emitted as display items.

use std::sync::Arc;
use crate::display_list::{DisplayItem, DisplayList};
use crate::tree::LayoutBox;
use rabbithole_gfx::{Color, ColorStop, Image, Paint, Point, Radii, Rect};
use rabbithole_style::{
    BackgroundImage, ColorInterp, BackgroundLayer, BackgroundRepeat, BackgroundSize, BoxArea, ComputedStyle, Edges, GradientDirection,
    GradientStop, Length, RadialShape, RadialSize, Shadow,
};
use std::collections::HashMap;

/// The pixels an image shows for `style`: without its EXIF orientation
/// under `image-orientation: none`.
pub(crate) fn oriented(img: &Arc<Image>, style: &ComputedStyle) -> Arc<Image> {
    match (&img.unoriented, style.image_orientation_none) {
        (Some(raw), true) => raw.clone(),
        _ => img.clone(),
    }
}

/// The `object-view-box` of an image of natural size `natural`: the shown
/// region in natural CSS px (None: the whole image).
pub(crate) fn object_view_box(style: &ComputedStyle, natural: (f32, f32)) -> Option<Rect> {
    let [t, r, b, l] = style.object_view_box?;
    let (w, h) = natural;
    let at = |v: Length, base: f32| if v.is_auto() { 0.0 } else { v.resolve_or_zero(base) };
    let (top, right, bottom, left) = (at(t, h), at(r, w), at(b, h), at(l, w));
    // An empty view box is ignored.
    let (vw, vh) = (w - left - right, h - top - bottom);
    (vw > 0.0 && vh > 0.0).then(|| Rect::new(left, top, vw, vh))
}

/// The three boxes of an element plus its radii.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BoxGeometry {
    pub border: Rect,
    pub padding: Rect,
    pub content: Rect,
    /// Outer (border-box) radii.
    pub radii: Radii,
    pub border_width: Edges<f32>,
    /// A scroll container's padding box moved by the scroll offset and
    /// grown to its scrollable overflow (for `background-attachment: local`).
    pub scrolled: Option<Rect>,
}

impl BoxGeometry {
    /// The geometry of the scrolled contents: boxes relative to the
    /// scrollable overflow area.
    fn scrolled(&self) -> Option<BoxGeometry> {
        let p = self.scrolled?;
        let bw = &self.border_width;
        let pad = Edges {
            top: self.content.y - self.padding.y,
            right: self.padding.right() - self.content.right(),
            bottom: self.padding.bottom() - self.content.bottom(),
            left: self.content.x - self.padding.x,
        };
        Some(BoxGeometry {
            border: Rect::new(p.x - bw.left, p.y - bw.top, p.w + bw.left + bw.right, p.h + bw.top + bw.bottom),
            padding: p,
            content: Rect::new(p.x + pad.left, p.y + pad.top, (p.w - pad.left - pad.right).max(0.0), (p.h - pad.top - pad.bottom).max(0.0)),
            radii: self.radii,
            border_width: self.border_width,
            scrolled: None,
        })
    }
}

/// Resolved border-box radii of a laid out box.
pub(crate) fn box_radii(b: &LayoutBox) -> Radii {
    let r = b.style.resolved_radii(b.rect.w, b.rect.h);
    // The legacy single radius applies only when no corner radius was set
    // (`25px 0` is a square corner, not a fallback).
    let zero = |l: &Length| matches!(l, Length::Px(v) | Length::Percent(v) if *v == 0.0);
    let unset = b.style.border_radii.iter().all(|(h, v)| zero(h) && zero(v));
    if unset && b.style.border_radius > 0.0 {
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
        paint_layer(list, geom, layer, images, viewport, style);
    }
}

/// CSS Images 3 §5.2 default sizing algorithm: the concrete size of an
/// object with optional natural `(width, height, ratio)`, a `specified`
/// size (each axis optional) and a default object size.
pub(crate) fn concrete_size(specified: (Option<f32>, Option<f32>), natural: (Option<f32>, Option<f32>, Option<f32>), default: (f32, f32)) -> (f32, f32) {
    let (nw, nh, ratio) = natural;
    match specified {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, match ratio {
            Some(r) => w / r,
            None => nh.unwrap_or(default.1),
        }),
        (None, Some(h)) => (match ratio {
            Some(r) => h * r,
            None => nw.unwrap_or(default.0),
        }, h),
        (None, None) => match (nw, nh, ratio) {
            (Some(w), Some(h), _) => (w, h),
            (Some(w), None, Some(r)) => (w, w / r),
            (None, Some(h), Some(r)) => (h * r, h),
            (Some(w), None, None) => (w, default.1),
            (None, Some(h), None) => (default.0, h),
            (None, None, Some(r)) => fit_ratio(r, default, false),
            (None, None, None) => default,
        },
    }
}

/// The largest (`cover`: smallest) size with aspect ratio `r` that fits
/// in (covers) `area`.
pub(crate) fn fit_ratio(r: f32, area: (f32, f32), cover: bool) -> (f32, f32) {
    if r <= 0.0 || !r.is_finite() {
        return area;
    }
    // Scale a 1px-high object of ratio r; a zero-sized axis scales
    // nothing (`contain`) or defers to the other axis (`cover`).
    let (sx, sy) = (area.0 / r, area.1);
    let s = if cover { sx.max(sy) } else { sx.min(sy) };
    (r * s, s)
}

/// The image of a background layer, resolved.
enum LayerImage<'a> {
    Raster(Arc<Image>),
    Gradient(&'a BackgroundImage),
}

fn paint_layer(list: &mut DisplayList, geom: &BoxGeometry, layer: &BackgroundLayer, images: &HashMap<String, Arc<Image>>, viewport: Rect, style: &ComputedStyle) {
    // `local`: positioned and painted in the scrolled contents, seen
    // through the scrollport (CSS Backgrounds 3 §3.6).
    if layer.local && !layer.fixed {
        if let Some(scrolled) = geom.scrolled() {
            // The scrolled contents never show under the borders.
            let port = area_rect(geom, BoxArea::PaddingBox);
            let radii = area_radii(geom, BoxArea::PaddingBox);
            list.items.push(if has_radii(&radii) { DisplayItem::PushClipRounded(port, radii) } else { DisplayItem::PushClip(port) });
            let mut inner = layer.clone();
            inner.local = false;
            if inner.clip == BoxArea::BorderBox {
                inner.clip = BoxArea::PaddingBox;
            }
            paint_layer(list, &scrolled, &inner, images, viewport, style);
            list.items.push(DisplayItem::PopClip);
            return;
        }
    }
    let pixelated = style.image_rendering_pixelated;
    let clip_rect = area_rect(geom, layer.clip);
    let clip_radii = area_radii(geom, layer.clip);
    if clip_rect.is_empty() {
        return;
    }
    let area = if layer.fixed { viewport } else { area_rect(geom, layer.origin) };
    let image = match &layer.image {
        BackgroundImage::Url(url) => match images.get(url) {
            Some(img) if !img.broken => LayerImage::Raster(oriented(img, style)),
            _ => return,
        },
        BackgroundImage::None => return,
        g => LayerImage::Gradient(g),
    };
    let natural = match &image {
        LayerImage::Raster(img) => img.natural_dimensions(),
        LayerImage::Gradient(_) => (None, None, None),
    };
    let (tw, th) = tile_size(layer.size, natural, area);
    if !(tw > 0.0 && th > 0.0 && tw.is_finite() && th.is_finite()) {
        return;
    }
    // Round (CSS Backgrounds 3 §3.4): scale the tile so a whole number of
    // tiles fits; with `auto` on the other axis, keep the aspect ratio.
    let (tw, th) = {
        let (mut nw, mut nh) = (tw, th);
        let round_x = layer.repeat.0 == BackgroundRepeat::Round && area.w > 0.0;
        let round_y = layer.repeat.1 == BackgroundRepeat::Round && area.h > 0.0;
        if round_x {
            nw = area.w / (area.w / tw).round().max(1.0);
        }
        if round_y {
            nh = area.h / (area.h / th).round().max(1.0);
        }
        let auto_h = matches!(layer.size, BackgroundSize::Auto) || matches!(layer.size, BackgroundSize::Explicit(_, Length::Auto));
        let auto_w = matches!(layer.size, BackgroundSize::Auto) || matches!(layer.size, BackgroundSize::Explicit(Length::Auto, _));
        if round_x && !round_y && auto_h {
            nh = th * nw / tw;
        } else if round_y && !round_x && auto_w {
            nw = tw * nh / th;
        }
        (nw, nh)
    };
    let px = bg_position(layer.position.0, area.w, tw);
    let py = bg_position(layer.position.1, area.h, th);
    let repeat_x = !matches!(layer.repeat.0, BackgroundRepeat::NoRepeat);
    let repeat_y = !matches!(layer.repeat.1, BackgroundRepeat::NoRepeat);
    // Tile origins and steps per axis (space spreads the tiles out).
    let axis = |repeat: BackgroundRepeat, start: f32, len: f32, size: f32, pos: f32| -> (f32, f32) {
        if repeat == BackgroundRepeat::Space {
            let count = (len / size).floor();
            if count >= 2.0 {
                return (start, (len - size) / (count - 1.0));
            }
            return (start + pos, f32::INFINITY);
        }
        (start + pos, size)
    };
    let (ox, step_x) = axis(layer.repeat.0, area.x, area.w, tw, px);
    let (oy, step_y) = axis(layer.repeat.1, area.y, area.h, th, py);
    // A `space` axis with fewer than two copies places one tile.
    let repeat_x = repeat_x && step_x.is_finite();
    let repeat_y = repeat_y && step_y.is_finite();
    let tile = Rect::new(ox, oy, tw, th);
    // The painted region: the clip, limited to the tile on axes that do
    // not repeat.
    let mut fill = clip_rect;
    if !repeat_x {
        fill.x = tile.x;
        fill.w = tile.w;
    }
    if !repeat_y {
        fill.y = tile.y;
        fill.h = tile.h;
    }
    let fill = fill.intersect(&clip_rect);
    if fill.is_empty() {
        return;
    }

    let clipped = has_radii(&clip_radii);
    if clipped {
        list.items.push(DisplayItem::PushClipRounded(clip_rect, clip_radii));
    } else {
        list.items.push(DisplayItem::PushClip(clip_rect));
    }
    match image {
        LayerImage::Raster(img) => {
            let spaced = (repeat_x && (step_x - tw).abs() > 0.01) || (repeat_y && (step_y - th).abs() > 0.01);
            if spaced {
                let xs = tile_origins(layer.repeat.0, area.x, area.w, tw, px, fill.x, fill.right());
                let ys = tile_origins(layer.repeat.1, area.y, area.h, th, py, fill.y, fill.bottom());
                if xs.len() * ys.len() <= 4096 {
                    for &y in &ys {
                        for &x in &xs {
                            let t = Rect::new(x, y, tw, th);
                            if t.intersect(&clip_rect).is_empty() {
                                continue;
                            }
                            list.items.push(DisplayItem::Image {
                                rect: t,
                                src: Rect::new(0.0, 0.0, img.width as f32, img.height as f32),
                                image: img.clone(),
                                opacity: 1.0,
                                pixelated,
                            });
                        }
                    }
                }
            } else if repeat_x || repeat_y {
                list.items.push(DisplayItem::ImageTiled {
                    image: img,
                    area: fill,
                    origin: Point { x: tile.x, y: tile.y },
                    tile: (tw, th),
                    repeat_x,
                    repeat_y,
                    opacity: 1.0,
                    pixelated,
                });
            } else {
                let shown = tile.intersect(&clip_rect);
                if !shown.is_empty() {
                    let src = Rect::new(
                        (shown.x - tile.x) / tw * img.width as f32,
                        (shown.y - tile.y) / th * img.height as f32,
                        shown.w / tw * img.width as f32,
                        shown.h / th * img.height as f32,
                    );
                    list.items.push(DisplayItem::Image { rect: shown, src, image: img, opacity: 1.0, pixelated });
                }
            }
        }
        LayerImage::Gradient(g) => {
            if let Some(paint) = gradient_paint(g, tile) {
                // One tile covering the painted region needs no tiling.
                let covers = tile.x <= fill.x + 0.01 && tile.y <= fill.y + 0.01 && tile.right() >= fill.right() - 0.01 && tile.bottom() >= fill.bottom() - 0.01;
                let paint = if covers || !(repeat_x || repeat_y) {
                    paint
                } else {
                    Paint::Tiled {
                        paint: Box::new(paint),
                        origin: Point { x: tile.x, y: tile.y },
                        size: (tw, th),
                        step: (if repeat_x { step_x } else { tw }, if repeat_y { step_y } else { th }),
                        repeat_x,
                        repeat_y,
                    }
                };
                list.items.push(DisplayItem::RectRadii { rect: fill, radii: [(0.0, 0.0); 4], paint });
            }
        }
    }
    list.items.push(DisplayItem::PopClip);
}

/// The paint of a gradient image filling `tile`.
pub(crate) fn gradient_paint(g: &BackgroundImage, tile: Rect) -> Option<Paint> {
    match g {
        BackgroundImage::LinearGradient { direction, stops, repeating, interp } => linear_paint(direction, stops, *repeating, *interp, tile),
        BackgroundImage::RadialGradient { shape, size, position, stops, repeating, interp } => radial_paint(shape, size, *position, stops, *repeating, *interp, tile),
        BackgroundImage::ConicGradient { angle, position, stops, repeating, interp } => conic_paint(*angle, *position, stops, *repeating, *interp, tile),
        _ => None,
    }
}

/// Visible tile origins for an axis of a spaced background. The other axis
/// may repeat normally, round, or place just one tile. CSS Backgrounds 3 §2.4:
/// fewer than two copies makes `space` use background-position, even when the
/// image is larger than the positioning area. Otherwise position is ignored.
fn tile_origins(
    repeat: BackgroundRepeat,
    start: f32,
    length: f32,
    size: f32,
    position: f32,
    clip_start: f32,
    clip_end: f32,
) -> Vec<f32> {
    if ![start, length, size, position, clip_start, clip_end].iter().all(|v| v.is_finite())
        || size <= 0.0 || clip_end <= clip_start
    {
        return Vec::new();
    }
    let (start, length, size, position, clip_start, clip_end) = (
        start as f64, length as f64, size as f64, position as f64,
        clip_start as f64, clip_end as f64,
    );
    let (anchor, step) = if repeat == BackgroundRepeat::Space {
        let count = (length / size).floor();
        if count < 2.0 {
            return vec![(start + position) as f32];
        }
        (start, (length - size) / (count - 1.0))
    } else if repeat == BackgroundRepeat::NoRepeat {
        return vec![(start + position) as f32];
    } else {
        (start + position, size)
    };
    if !step.is_finite() || step <= 0.0 {
        return Vec::new();
    }
    // Normalize in constant time. Repeated subtraction can stop progressing
    // for large finite CSS offsets, and must not hang display-list creation.
    let mut first = clip_start - (clip_start - anchor).rem_euclid(step);
    if first + size <= clip_start {
        first += step;
    }
    let count = ((clip_end - first) / step).ceil().max(0.0).min(10_000.0) as usize;
    (0..count).map(|index| (first + index as f64 * step) as f32).collect()
}

/// Resolve `background-size` to a tile size (CSS Backgrounds 3 §3.9):
/// the default sizing algorithm with the positioning area as the default
/// object size.
fn tile_size(size: BackgroundSize, natural: (Option<f32>, Option<f32>, Option<f32>), area: Rect) -> (f32, f32) {
    let default = (area.w, area.h);
    match size {
        BackgroundSize::Auto => concrete_size((None, None), natural, default),
        BackgroundSize::Cover | BackgroundSize::Contain => match natural.2 {
            Some(r) => fit_ratio(r, default, matches!(size, BackgroundSize::Cover)),
            None => default,
        },
        BackgroundSize::Explicit(w, h) => {
            let rw = w.resolve(area.w).map(|v| v.max(0.0));
            let rh = h.resolve(area.h).map(|v| v.max(0.0));
            concrete_size((rw, rh), natural, default)
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

/// A resolved stop with the full-precision color it was written with.
#[derive(Clone, Copy)]
struct WideStop {
    offset: f32,
    color: Color,
    wide: rabbithole_style::Wide,
}

/// Resolve gradient stops to offsets along a line of length `len` (1.0
/// = the end of the gradient line; offsets may lie outside 0..=1) per CSS
/// Images 3 §3.4.3, expanding color interpolation hints (CSS Images 4
/// §3.5.3) into intermediate stops.
pub(crate) fn resolve_stops(stops: &[GradientStop], len: f32) -> Vec<ColorStop> {
    resolve_wide_stops(stops, len, None).into_iter().map(|s| ColorStop { offset: s.offset, color: s.color }).collect()
}

/// [`resolve_stops`] keeping full-precision colors; hint transitions are
/// interpolated in `interp` (sRGB when `None`).
fn resolve_wide_stops(stops: &[GradientStop], len: f32, interp: Option<ColorInterp>) -> Vec<WideStop> {
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
    // Evenly distribute runs of unpositioned color stops between the
    // positioned stops around them (hints always have a position).
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
    let stop = |offset: f32, s: &GradientStop| WideStop { offset, color: s.color, wide: s.wide.unwrap_or_else(|| rabbithole_style::color_to_wide(s.color)) };
    let mut out: Vec<WideStop> = Vec::with_capacity(n);
    for k in 0..n {
        let offset = pos[k].unwrap_or(0.0);
        if !stops[k].hint {
            out.push(stop(offset, &stops[k]));
            continue;
        }
        // A hint between the previous and next color stops: the color
        // at fraction f between them is mix(f ^ (ln 0.5 / ln h)).
        let (Some(prev), Some(next)) = (out.last().copied(), stops.get(k + 1)) else { continue };
        let b = pos[k + 1].unwrap_or(offset);
        let next = stop(b, next);
        let a = prev.offset;
        if b - a <= 1e-6 {
            continue;
        }
        let h = ((offset - a) / (b - a)).clamp(0.0, 1.0);
        if h <= 1e-6 {
            // The transition happens at the first stop.
            out.push(WideStop { offset: a, ..next });
            continue;
        }
        if h >= 1.0 - 1e-6 {
            out.push(WideStop { offset: b, ..prev });
            continue;
        }
        if (h - 0.5).abs() < 1e-4 {
            continue;
        }
        let exp = 0.5f32.ln() / h.ln();
        const SAMPLES: usize = 16;
        for step in 1..SAMPLES {
            let f = step as f32 / SAMPLES as f32;
            let w = f.powf(exp);
            let (color, wide) = match interp {
                Some(m) => {
                    let wide = rabbithole_style::interpolate_wide(prev.wide, next.wide, w, m);
                    (rabbithole_style::wide_to_color(wide), wide)
                }
                None => {
                    let color = rabbithole_gfx::paint::lerp_premul(prev.color, next.color, w);
                    (color, rabbithole_style::color_to_wide(color))
                }
            };
            out.push(WideStop { offset: a + (b - a) * f, color, wide });
        }
    }
    out
}

/// A repeating gradient whose period is zero paints the color of its last
/// stop (CSS Images 4 §3.6 "gradient normalization").
fn degenerate_repeat(stops: &[ColorStop]) -> Option<Paint> {
    let first = stops.first()?.offset;
    let last = stops.last()?;
    (last.offset - first <= 1e-6).then(|| Paint::Solid(last.color))
}

/// Stops for a gradient interpolated in another color space, or between
/// colors outside the sRGB gamut: each segment is sampled from the
/// unclipped colors (the paint interpolates in sRGB between samples).
fn expand_interpolation(stops: &[WideStop], interp: ColorInterp) -> Vec<ColorStop> {
    // Samples at a uniform density along the gradient line, so splitting
    // a segment at an intermediate stop samples the same offsets.
    let mut out = Vec::with_capacity(stops.len() * 64);
    // Stop colors are clipped like the samples between them.
    let clip = |s: &WideStop| rabbithole_style::wide_to_color(s.wide);
    out.push(ColorStop { offset: stops[0].offset, color: clip(&stops[0]) });
    for w in stops.windows(2) {
        let (a, b) = (w[0], w[1]);
        if b.offset - a.offset <= 1e-6 || a.wide == b.wide {
            out.push(ColorStop { offset: b.offset, color: clip(&b) });
            continue;
        }
        let samples = ((b.offset - a.offset) * 256.0).ceil().clamp(4.0, 1024.0) as usize;
        for k in 1..=samples {
            let f = k as f32 / samples as f32;
            let color = if k == samples { clip(&b) } else { rabbithole_style::wide_to_color(rabbithole_style::interpolate_wide(a.wide, b.wide, f, interp)) };
            out.push(ColorStop { offset: a.offset + (b.offset - a.offset) * f, color });
        }
    }
    out
}

/// Is a full-precision color outside the sRGB gamut?
fn out_of_gamut(w: &rabbithole_style::Wide) -> bool {
    w[..3].iter().any(|v| !(-1e-4..=1.0001).contains(v))
}

/// Stops ready for a paint: at least two, average color for degenerate
/// repeating gradients.
fn prepare_stops(stops: &[GradientStop], len: f32, repeating: bool, interp: ColorInterp) -> Result<Vec<ColorStop>, Paint> {
    let mut resolved = if !interp.is_srgb() || stops.iter().any(|s| s.wide.as_ref().is_some_and(out_of_gamut)) {
        let wide = resolve_wide_stops(stops, len, Some(interp));
        if wide.len() > 1 {
            expand_interpolation(&wide, interp)
        } else {
            wide.into_iter().map(|s| ColorStop { offset: s.offset, color: s.color }).collect()
        }
    } else {
        resolve_stops(stops, len)
    };
    if resolved.is_empty() {
        return Err(Paint::Solid(Color::TRANSPARENT));
    }
    if resolved.len() == 1 {
        return Err(Paint::Solid(resolved[0].color));
    }
    if repeating {
        if let Some(p) = degenerate_repeat(&resolved) {
            return Err(p);
        }
    }
    if resolved.iter().any(|s| !s.offset.is_finite()) {
        for s in &mut resolved {
            s.offset = s.offset.clamp(-1e6, 1e6);
        }
    }
    Ok(resolved)
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
pub(crate) fn linear_paint(direction: &GradientDirection, stops: &[GradientStop], repeating: bool, interp: ColorInterp, tile: Rect) -> Option<Paint> {
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
    let resolved = match prepare_stops(stops, len, repeating, interp) {
        Ok(r) => r,
        Err(p) => return Some(p),
    };
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
    interp: ColorInterp,
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
    // A zero-size ending shape (CSS Images 3 §3.2.4): only the last
    // color shows for a plain gradient; a tiny shape approximates the rest.
    if !repeating && (rx <= 0.0 || ry <= 0.0) {
        let resolved = resolve_stops(stops, 0.0);
        return resolved.last().map(|s| Paint::Solid(s.color));
    }
    let rx = rx.max(0.01);
    let ry = ry.max(0.01);
    let resolved = match prepare_stops(stops, rx, repeating, interp) {
        Ok(r) => r,
        Err(p) => return Some(p),
    };
    Some(Paint::Radial {
        center: Point { x: cx, y: cy },
        rx,
        ry,
        stops: resolved,
        repeat: repeating,
    })
}

/// Build a conic gradient paint for `tile` (CSS Images 4 §3.3).
pub(crate) fn conic_paint(angle: f32, position: (Length, Length), stops: &[GradientStop], repeating: bool, interp: ColorInterp, tile: Rect) -> Option<Paint> {
    if stops.is_empty() {
        return None;
    }
    let cx = tile.x + position.0.resolve_or_zero(tile.w);
    let cy = tile.y + position.1.resolve_or_zero(tile.h);
    // Stop positions are fractions of a turn.
    let resolved = match prepare_stops(stops, 1.0, repeating, interp) {
        Ok(r) => r,
        Err(p) => return Some(p),
    };
    Some(Paint::Conic { center: Point { x: cx, y: cy }, angle, stops: resolved, repeat: repeating })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_distribute_evenly() {
        let stops = vec![
            GradientStop { color: Color::BLACK, position: None, hint: false, wide: None },
            GradientStop { color: Color::WHITE, position: None, hint: false, wide: None },
            GradientStop { color: Color::BLACK, position: None, hint: false, wide: None },
        ];
        let r = resolve_stops(&stops, 100.0);
        assert_eq!(r.iter().map(|s| s.offset).collect::<Vec<_>>(), vec![0.0, 0.5, 1.0]);
        let stops = vec![
            GradientStop { color: Color::BLACK, position: Some(Length::Px(20.0)), hint: false, wide: None },
            GradientStop { color: Color::WHITE, position: Some(Length::Percent(10.0)), hint: false, wide: None },
        ];
        let r = resolve_stops(&stops, 100.0);
        assert_eq!(r[1].offset, 0.2, "never decreasing");
    }

    #[test]
    fn color_hints_curve_the_transition() {
        let stops = vec![
            GradientStop { color: Color::BLACK, position: None, hint: false, wide: None },
            GradientStop { color: Color::TRANSPARENT, position: Some(Length::Percent(20.0)), hint: true, wide: None },
            GradientStop { color: Color::WHITE, position: None, hint: false, wide: None },
        ];
        let r = resolve_stops(&stops, 100.0);
        // Expanded into intermediate stops; halfway in color at the hint.
        assert!(r.len() > 3);
        let at = |o: f32| r.iter().min_by(|a, b| (a.offset - o).abs().total_cmp(&(b.offset - o).abs())).unwrap().color;
        let mid = at(0.1875).r as i32;
        assert!((100..=140).contains(&mid), "{mid}");
        // A hint at the midpoint is the plain linear transition.
        let stops = vec![stops[0].clone(), GradientStop { position: Some(Length::Percent(50.0)), ..stops[1].clone() }, stops[2].clone()];
        assert_eq!(resolve_stops(&stops, 100.0).len(), 2);
    }

    #[test]
    fn default_sizing_and_ratio_fitting() {
        assert_eq!(concrete_size((Some(10.0), None), (None, None, None), (50.0, 40.0)), (10.0, 40.0));
        assert_eq!(concrete_size((None, None), (None, Some(8.0), Some(2.0)), (50.0, 40.0)), (16.0, 8.0));
        assert_eq!(fit_ratio(2.0, (100.0, 100.0), false), (100.0, 50.0));
        assert_eq!(fit_ratio(2.0, (100.0, 100.0), true), (200.0, 100.0));
        // A zero-sized area: contain shows nothing, cover follows the other axis.
        assert_eq!(fit_ratio(2.0, (100.0, 0.0), false), (0.0, 0.0));
        assert_eq!(fit_ratio(2.0, (100.0, 0.0), true), (100.0, 50.0));
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
            GradientStop { color: Color::BLACK, position: None, hint: false, wide: None },
            GradientStop { color: Color::WHITE, position: None, hint: false, wide: None },
        ];
        match linear_paint(&GradientDirection::Angle(std::f32::consts::FRAC_PI_2), &stops, false, ColorInterp::default(), tile).unwrap() {
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
        let n = |w: f32, h: f32| (Some(w), Some(h), Some(w / h));
        assert_eq!(tile_size(BackgroundSize::Cover, n(50.0, 50.0), area), (200.0, 200.0));
        assert_eq!(tile_size(BackgroundSize::Contain, n(50.0, 50.0), area), (100.0, 100.0));
        assert_eq!(tile_size(BackgroundSize::Explicit(Length::Px(40.0), Length::Auto), n(50.0, 25.0), area), (40.0, 20.0));
        // Vector images: a natural width only, no ratio.
        assert_eq!(tile_size(BackgroundSize::Auto, (Some(8.0), None, None), area), (8.0, 100.0));
        assert_eq!(tile_size(BackgroundSize::Explicit(Length::Auto, Length::Px(32.0)), (Some(8.0), None, None), area), (8.0, 32.0));
        assert_eq!(tile_size(BackgroundSize::Auto, (None, None, Some(2.0)), Rect::new(0.0, 0.0, 100.0, 100.0)), (100.0, 50.0));
        assert_eq!(tile_size(BackgroundSize::Contain, (None, Some(8.0), None), area), (200.0, 100.0));
        assert_eq!(bg_position(Length::Percent(50.0), 200.0, 50.0), 75.0);
        assert_eq!(bg_position(Length::Px(10.0), 200.0, 50.0), 10.0);
    }
}

#[cfg(test)]
mod space_tiling_tests {
    use super::*;

    #[test]
    fn space_anchors_edges_and_ignores_position_when_two_copies_fit() {
        assert_eq!(tile_origins(BackgroundRepeat::Space, 0., 106., 32., 15., 0., 106.), [0., 37., 74.]);
        assert_eq!(tile_origins(BackgroundRepeat::Space, 35., 65., 30., 99., 0., 135.), [0., 35., 70., 105.]);
    }

    #[test]
    fn space_places_one_positioned_tile_even_when_it_is_oversized() {
        assert_eq!(tile_origins(BackgroundRepeat::Space, 0., 40., 80., -20., 0., 40.), [-20.]);
        assert_eq!(tile_origins(BackgroundRepeat::Space, 0., 100., 60., 30., -30., 130.), [30.]);
        assert_eq!(tile_origins(BackgroundRepeat::NoRepeat, 0., 70., 20., 8., 0., 70.), [8.]);
    }

    #[test]
    fn repeated_cross_axis_normalizes_large_offsets_without_subtraction_loops() {
        assert_eq!(tile_origins(BackgroundRepeat::Repeat, 0., 16., 8., 1e30, 0., 16.), [0., 8.]);
        assert_eq!(tile_origins(BackgroundRepeat::Repeat, 0., 16., 8., -1e30, 0., 16.), [0., 8.]);
    }

    #[test]
    fn tile_enumeration_is_bounded_and_invalid_geometry_emits_nothing() {
        assert_eq!(tile_origins(BackgroundRepeat::Repeat, 0., 100., 0.0001, 0., 0., 100.).len(), 10_000);
        assert!(tile_origins(BackgroundRepeat::Space, 0., 100., 0., 0., 0., 100.).is_empty());
        assert!(tile_origins(BackgroundRepeat::Space, 0., 100., f32::NAN, 0., 0., 100.).is_empty());
        assert!(tile_origins(BackgroundRepeat::Space, 0., 100., 10., 0., 100., 0.).is_empty());
    }
}
