//! Canvas v0.2 operations: per-corner radii, gradients, shadows, transforms,
//! paths, layers and image transforms.

use crate::canvas::{blend, blend_image, span_coverage, Bounds, Canvas, Color, FillRule, Layer, LineCap, Matrix, Paint, PathSeg, Point, Radii, Rect, StrokeStyle, IDENTITY};
use crate::font::{FontContext, FontId, ZERO_WIDTH_GLYPH};
use crate::image::Image;
use crate::paint::Shader;
use crate::path::{
    clamp_radii, flatten, invert, is_axis_aligned, is_identity, multiply, rect_path, scale_factor, transform_point, transform_rect_aligned,
    transform_rect_bounds, Polylines,
};
use crate::raster::{blur_radius_for_sigma, box_blur, rasterize, Mask, RoundedBox};
use crate::stroke::stroke_polylines;

pub use crate::path::{ellipse_path, parse_svg_path, rounded_rect_path};

/// Flattening tolerance in device pixels.
const TOLERANCE: f32 = 0.2;

impl Canvas {
    // ------------------------------------------------------------ transforms

    /// Push a transform that is composed with the current one (current ×
    /// m). All subsequent drawing (rects, paths, images, text, clips) is
    /// mapped through it.
    pub fn push_transform(&mut self, m: Matrix) {
        let cur = self.transform();
        self.transforms.push(multiply(&cur, &m));
    }

    pub fn pop_transform(&mut self) {
        self.transforms.pop();
    }

    /// Current transform (identity when none pushed).
    pub fn transform(&self) -> Matrix {
        self.transforms.last().copied().unwrap_or(IDENTITY)
    }

    /// Map a point through the current transform.
    pub fn map_point(&self, p: Point) -> Point {
        transform_point(&self.transform(), p)
    }

    /// Push a clip given as a rounded rect (in current user space).
    pub fn push_clip_rounded(&mut self, rect: Rect, radii: Radii) {
        let m = self.transform();
        let plain = radii.iter().all(|c| c.0 <= 0.0 || c.1 <= 0.0);
        if plain {
            return self.push_clip(rect);
        }
        if is_identity(&m) || is_axis_aligned(&m) {
            let dr = transform_rect_aligned(&m, rect);
            let dradii = scale_radii(radii, m[0], m[3]);
            let shape = RoundedBox::new(dr, dradii);
            let mask = shape.mask(self.clip_bounds().tuple());
            self.push_mask_clip(mask);
        } else {
            self.push_clip_path(&rounded_rect_path(rect, radii), FillRule::NonZero);
        }
    }

    /// Push a clip given as an arbitrary path (used by `clip-path`, SVG).
    pub fn push_clip_path(&mut self, path: &[PathSeg], rule: FillRule) {
        let m = self.transform();
        let poly = flatten(path, &m, TOLERANCE);
        let mask = rasterize(&poly, rule, self.clip_bounds().tuple());
        self.push_mask_clip(mask);
    }

    // ------------------------------------------------------------------ fills

    /// Fill a rect with per-corner radii using any paint.
    pub fn fill_rounded_rect_radii(&mut self, rect: Rect, radii: Radii, paint: &Paint) {
        if rect.is_empty() {
            return;
        }
        let m = self.transform();
        if !(is_identity(&m) || is_axis_aligned(&m)) {
            return self.fill_path(&rounded_rect_path(rect, radii), FillRule::NonZero, paint);
        }
        let dr = transform_rect_aligned(&m, rect);
        let dradii = scale_radii(radii, m[0], m[3]);
        let shape = RoundedBox::new(dr, dradii);
        let shader = Shader::new(paint, &m);
        if shape.is_plain() {
            if let Some(c) = shader.is_solid() {
                return self.fill_device_rect(dr, c);
            }
        }
        let mask = shape.mask(self.clip_bounds().tuple());
        self.fill_mask(&mask, &shader);
    }

    /// Fill an arbitrary path (anti-aliased scanline rasterization with
    /// coverage accumulation; curves flattened adaptively).
    pub fn fill_path(&mut self, path: &[PathSeg], rule: FillRule, paint: &Paint) {
        let m = self.transform();
        let poly = flatten(path, &m, TOLERANCE);
        let mask = rasterize(&poly, rule, self.clip_bounds().tuple());
        let shader = Shader::new(paint, &m);
        self.fill_mask(&mask, &shader);
    }

    /// Stroke a path (joins, caps, dashes) by converting the stroke to a
    /// fill path.
    pub fn stroke_path(&mut self, path: &[PathSeg], style: &StrokeStyle, paint: &Paint) {
        if style.width <= 0.0 {
            return;
        }
        let m = self.transform();
        let sf = scale_factor(&m);
        // Flatten and stroke in user space so the width is in user units,
        // then map the outline to device space.
        let poly = flatten(path, &IDENTITY, TOLERANCE / sf);
        let mut outline = stroke_polylines(&poly, style);
        if !is_identity(&m) {
            for c in &mut outline.contours {
                for p in &mut c.points {
                    *p = transform_point(&m, *p);
                }
            }
        }
        let mask = rasterize(&outline, FillRule::NonZero, self.clip_bounds().tuple());
        let shader = Shader::new(paint, &m);
        self.fill_mask(&mask, &shader);
    }

    /// Borders with per-side widths/colors/styles and per-corner radii.
    /// Styles: 0 solid, 1 dashed, 2 dotted, 3 double, 4 inset, 5 outset,
    /// 6 groove, 7 ridge.
    pub fn stroke_rect_radii(&mut self, rect: Rect, widths: [f32; 4], colors: [Color; 4], styles: [u8; 4], radii: Radii) {
        if rect.is_empty() || widths.iter().all(|w| *w <= 0.0) {
            return;
        }
        let widths = widths.map(|w| w.max(0.0));
        let radii = clamp_radii(rect, radii);
        let has_radius = radii.iter().any(|c| c.0 > 0.0 && c.1 > 0.0);
        let visible: Vec<usize> = (0..4).filter(|&i| widths[i] > 0.0 && colors[i].a > 0).collect();
        if visible.is_empty() {
            return;
        }
        let same_visible = visible.iter().all(|&i| {
            let f = visible[0];
            widths[i] == widths[f] && colors[i] == colors[f] && styles[i] == styles[f]
        });
        let uniform = same_visible && visible.len() == 4;
        let style0 = styles[visible[0]];

        if same_visible && matches!(style0, 1 | 2) {
            // Dashed / dotted: stroke the midline of the visible sides.
            let w = widths[visible[0]];
            let mid = rect.inset(w / 2.0, w / 2.0, w / 2.0, w / 2.0);
            let path = if uniform {
                let mid_radii = radii.map(|(h, v)| ((h - w / 2.0).max(0.0), (v - w / 2.0).max(0.0)));
                if has_radius {
                    rounded_rect_path(mid, mid_radii)
                } else {
                    rect_path(mid)
                }
            } else {
                let mut p = Vec::new();
                let corners = [
                    (Point::new(mid.x, mid.y), Point::new(mid.right(), mid.y)),
                    (Point::new(mid.right(), mid.y), Point::new(mid.right(), mid.bottom())),
                    (Point::new(mid.right(), mid.bottom()), Point::new(mid.x, mid.bottom())),
                    (Point::new(mid.x, mid.bottom()), Point::new(mid.x, mid.y)),
                ];
                for &i in &visible {
                    p.push(PathSeg::MoveTo(corners[i].0));
                    p.push(PathSeg::LineTo(corners[i].1));
                }
                p
            };
            let style = if style0 == 1 {
                StrokeStyle { width: w, dashes: vec![3.0 * w, 1.5 * w], ..Default::default() }
            } else {
                StrokeStyle { width: w, cap: LineCap::Round, dashes: vec![0.01, 2.0 * w], ..Default::default() }
            };
            return self.stroke_path(&path, &style, &Paint::Solid(colors[visible[0]]));
        }
        if uniform && style0 == 3 {
            // Double: outer and inner thirds.
            let w = widths[0];
            let t = (w / 3.0).max(1.0);
            self.fill_ring(rect, radii, [t; 4], colors);
            let inner_rect = rect.inset(w - t, w - t, w - t, w - t);
            let inner_radii = radii.map(|(h, v)| ((h - (w - t)).max(0.0), (v - (w - t)).max(0.0)));
            self.fill_ring(inner_rect, inner_radii, [t; 4], colors);
            return;
        }
        // 3-D styles: derive per-side colors.
        let mut side_colors = colors;
        let mut groove_inner: Option<[Color; 4]> = None;
        for i in 0..4 {
            match styles[i] {
                4 => side_colors[i] = shade_3d(colors[i], i, true),
                5 => side_colors[i] = shade_3d(colors[i], i, false),
                6 | 7 => {
                    let inset = styles[i] == 6;
                    side_colors[i] = shade_3d(colors[i], i, inset);
                    let inner = groove_inner.get_or_insert(colors);
                    inner[i] = shade_3d(colors[i], i, !inset);
                }
                _ => {}
            }
        }
        if let Some(inner_colors) = groove_inner {
            let half = widths.map(|w| w / 2.0);
            self.fill_ring(rect, radii, half, side_colors);
            let inner_rect = rect.inset(half[0], half[1], half[2], half[3]);
            let inner_radii = [
                ((radii[0].0 - half[3]).max(0.0), (radii[0].1 - half[0]).max(0.0)),
                ((radii[1].0 - half[1]).max(0.0), (radii[1].1 - half[0]).max(0.0)),
                ((radii[2].0 - half[1]).max(0.0), (radii[2].1 - half[2]).max(0.0)),
                ((radii[3].0 - half[3]).max(0.0), (radii[3].1 - half[2]).max(0.0)),
            ];
            self.fill_ring(inner_rect, inner_radii, half, inner_colors);
            return;
        }
        // Solid (and non-uniform dashed/dotted drawn solid).
        let same_color = visible.iter().all(|&i| side_colors[i] == side_colors[visible[0]]);
        if !has_radius && same_color && (is_identity(&self.transform()) || is_axis_aligned(&self.transform())) {
            // Fast path: four bands.
            let [wt, wr, wb, wl] = widths;
            let c = side_colors[visible[0]];
            if wt > 0.0 {
                self.fill_rect(Rect::new(rect.x, rect.y, rect.w, wt), c);
            }
            if wb > 0.0 {
                self.fill_rect(Rect::new(rect.x, rect.bottom() - wb, rect.w, wb), c);
            }
            if wl > 0.0 {
                self.fill_rect(Rect::new(rect.x, rect.y + wt, wl, rect.h - wt - wb), c);
            }
            if wr > 0.0 {
                self.fill_rect(Rect::new(rect.right() - wr, rect.y + wt, wr, rect.h - wt - wb), c);
            }
            return;
        }
        self.fill_ring(rect, radii, widths, side_colors);
    }

    /// Paint the ring between a rounded rect and its inset by `widths`,
    /// coloring each pixel by the side it belongs to (mitred corners).
    fn fill_ring(&mut self, rect: Rect, radii: Radii, widths: [f32; 4], colors: [Color; 4]) {
        let m = self.transform();
        if !(is_identity(&m) || is_axis_aligned(&m)) {
            // Rotated: even-odd fill of outer + inner path with the first visible color.
            let mut path = rounded_rect_path(rect, radii);
            let inner = rect.inset(widths[0], widths[1], widths[2], widths[3]);
            if !inner.is_empty() {
                path.extend(rounded_rect_path(inner, inner_radii(radii, widths)));
            }
            let color = (0..4).find(|&i| widths[i] > 0.0).map(|i| colors[i]).unwrap_or(colors[0]);
            return self.fill_path(&path, FillRule::EvenOdd, &Paint::Solid(color));
        }
        let dr = transform_rect_aligned(&m, rect);
        let sx = m[0];
        let sy = m[3];
        let dw = [widths[0] * sy, widths[1] * sx, widths[2] * sy, widths[3] * sx];
        let dradii = scale_radii(radii, sx, sy);
        let outer = RoundedBox::new(dr, dradii);
        let inner_rect = dr.inset(dw[0], dw[1], dw[2], dw[3]);
        let inner = if inner_rect.is_empty() { None } else { Some(RoundedBox::new(inner_rect, inner_radii(dradii, dw))) };
        let b = self.bounds(&dr);
        if b.is_empty() {
            return;
        }
        let mask = self.current_mask();
        let w = self.width as usize;
        let [wt, wr, wb, wl] = dw;
        for y in b.y0..b.y1 {
            let row = y as usize * w;
            let py = y as f32 + 0.5;
            for x in b.x0..b.x1 {
                let cov_outer = outer.coverage(x, y);
                if cov_outer <= 0.0 {
                    continue;
                }
                let cov_inner = inner.map(|i| i.coverage(x, y)).unwrap_or(0.0);
                let cov = (cov_outer - cov_inner).clamp(0.0, 1.0);
                if cov <= 0.001 {
                    continue;
                }
                let px = x as f32 + 0.5;
                let side = if py < dr.y + wt && wt > 0.0 {
                    if wl > 0.0 && px < dr.x + wl && (px - dr.x) / wl < (py - dr.y) / wt {
                        3
                    } else if wr > 0.0 && px >= dr.right() - wr && (dr.right() - px) / wr < (py - dr.y) / wt {
                        1
                    } else {
                        0
                    }
                } else if py >= dr.bottom() - wb && wb > 0.0 {
                    // Pixel centres exactly on a bottom corner's diagonal
                    // belong to the side border (as in reference renderings).
                    if wl > 0.0 && px < dr.x + wl && (px - dr.x) / wl <= (dr.bottom() - py) / wb {
                        3
                    } else if wr > 0.0 && px >= dr.right() - wr && (dr.right() - px) / wr <= (dr.bottom() - py) / wb {
                        1
                    } else {
                        2
                    }
                } else if px < dr.x + wl {
                    3
                } else if px >= dr.right() - wr {
                    1
                } else if wl > 0.0 && px - dr.x < dr.right() - px {
                    3
                } else if wr > 0.0 {
                    1
                } else if wt > 0.0 && py - dr.y < dr.bottom() - py {
                    0
                } else {
                    2
                };
                let color = colors[side];
                let mut alpha = (color.a as f32 * cov + 0.5) as u32;
                if let Some(mk) = &mask {
                    alpha = (alpha * mk.get(x, y) as u32 + 127) / 255;
                }
                if alpha > 0 {
                    let idx = row + x as usize;
                    self.data[idx] = blend(self.data[idx], color, alpha);
                }
            }
        }
    }

    // ---------------------------------------------------------------- shadows

    /// CSS box-shadow: a rounded rect (border box) with offset, gaussian
    /// blur (sigma = blur / 2), spread and color; `inset` draws the shadow
    /// inside the box instead. The box itself is not painted.
    pub fn draw_box_shadow(&mut self, rect: Rect, radii: Radii, offset: (f32, f32), blur: f32, spread: f32, color: Color, inset: bool) {
        if color.a == 0 || rect.is_empty() {
            return;
        }
        let m = self.transform();
        let sf = scale_factor(&m);
        let (dr, dradii, off, blur, spread) = if is_identity(&m) || is_axis_aligned(&m) {
            (transform_rect_aligned(&m, rect), scale_radii(radii, m[0], m[3]), (offset.0 * m[0], offset.1 * m[3]), blur * sf, spread * sf)
        } else {
            (transform_rect_bounds(&m, rect), scale_radii(radii, sf, sf), (offset.0 * sf, offset.1 * sf), blur * sf, spread * sf)
        };
        let radii = clamp_radii(dr, dradii);
        let sigma = (blur / 2.0).max(0.0);
        let blur_r = blur_radius_for_sigma(sigma);
        let pad = (blur_r * 3) as f32 + 1.0;
        let clipb = self.clip_bounds();
        if clipb.is_empty() {
            return;
        }
        let box_shape = RoundedBox::new(dr, radii);
        if !inset {
            let srect = Rect::new(dr.x + off.0 - spread, dr.y + off.1 - spread, (dr.w + 2.0 * spread).max(0.0), (dr.h + 2.0 * spread).max(0.0));
            if srect.is_empty() {
                return;
            }
            let sradii = radii.map(|(h, v)| ((h + spread).max(0.0), (v + spread).max(0.0)));
            let shadow_shape = RoundedBox::new(srect, sradii);
            // Region: shadow rect padded by the blur, limited to clip ± pad.
            let region = Rect::new(srect.x - pad, srect.y - pad, srect.w + 2.0 * pad, srect.h + 2.0 * pad);
            let limit = Rect::new(clipb.x0 as f32 - pad, clipb.y0 as f32 - pad, (clipb.x1 - clipb.x0) as f32 + 2.0 * pad, (clipb.y1 - clipb.y0) as f32 + 2.0 * pad);
            let region = region.intersect(&limit);
            if region.is_empty() {
                return;
            }
            let mut mask = shape_mask(&shadow_shape, region);
            box_blur(&mut mask.data, mask.width as usize, mask.height as usize, blur_r, 3);
            // Knock out the element's own border box.
            knock_out(&mut mask, &box_shape);
            self.fill_mask(&mask, &Shader::Solid(color));
        } else {
            let region = dr;
            let limit = Rect::new(clipb.x0 as f32 - pad, clipb.y0 as f32 - pad, (clipb.x1 - clipb.x0) as f32 + 2.0 * pad, (clipb.y1 - clipb.y0) as f32 + 2.0 * pad);
            let region = region.intersect(&limit);
            if region.is_empty() {
                return;
            }
            // Mask = 1 - coverage(inner box), computed over a padded area so
            // the blur has room, then clipped to the box.
            let padded = Rect::new(region.x - pad, region.y - pad, region.w + 2.0 * pad, region.h + 2.0 * pad);
            let irect = Rect::new(dr.x + off.0 + spread, dr.y + off.1 + spread, (dr.w - 2.0 * spread).max(0.0), (dr.h - 2.0 * spread).max(0.0));
            let iradii = radii.map(|(h, v)| ((h - spread).max(0.0), (v - spread).max(0.0)));
            let inner_shape = if irect.is_empty() { None } else { Some(RoundedBox::new(irect, iradii)) };
            let mut mask = match &inner_shape {
                Some(s) => shape_mask(s, padded),
                None => Mask { x0: padded.x.floor() as i32, y0: padded.y.floor() as i32, width: padded.w.ceil() as u32, height: padded.h.ceil() as u32, data: vec![0; (padded.w.ceil() * padded.h.ceil()) as usize] },
            };
            for v in &mut mask.data {
                *v = 255 - *v;
            }
            box_blur(&mut mask.data, mask.width as usize, mask.height as usize, blur_r, 3);
            // Keep only inside the box.
            for y in 0..mask.height as i32 {
                for x in 0..mask.width as i32 {
                    let i = y as usize * mask.width as usize + x as usize;
                    if mask.data[i] == 0 {
                        continue;
                    }
                    let c = box_shape.coverage(mask.x0 + x, mask.y0 + y);
                    mask.data[i] = (mask.data[i] as f32 * c + 0.5) as u8;
                }
            }
            self.fill_mask(&mask, &Shader::Solid(color));
        }
    }

    /// Text with an optional blur (text-shadow). `blur == 0` is a plain
    /// offset draw.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_text_shadow(&mut self, fonts: &FontContext, font: FontId, size: f32, text: &str, x: f32, baseline_y: f32, letter_spacing: f32, color: Color, blur: f32) {
        if blur <= 0.5 {
            self.draw_text_ext(fonts, font, size, text, x, baseline_y, color, letter_spacing, false, false);
            return;
        }
        let m = self.transform();
        let sf = scale_factor(&m);
        let origin = transform_point(&m, Point::new(x, baseline_y));
        let dsize = size * sf;
        let glyphs = fonts.shape(font, dsize, text, letter_spacing * sf);
        let sigma = blur * sf / 2.0;
        let blur_r = blur_radius_for_sigma(sigma);
        let pad = (blur_r * 3) as i32 + 1;
        // Bounds of the text run.
        let metrics = fonts.metrics(font, dsize);
        let width = glyphs.last().map(|g| g.x + g.advance).unwrap_or(0.0);
        let x0 = origin.x.floor() as i32 - pad;
        let y0 = (origin.y - metrics.ascent).floor() as i32 - pad - 2;
        let x1 = (origin.x + width).ceil() as i32 + pad + 2;
        let y1 = (origin.y + metrics.descent).ceil() as i32 + pad + 2;
        let clipb = self.clip_bounds();
        let x0 = x0.max(clipb.x0 - pad);
        let y0 = y0.max(clipb.y0 - pad);
        let x1 = x1.min(clipb.x1 + pad);
        let y1 = y1.min(clipb.y1 + pad);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let w = (x1 - x0) as usize;
        let h = (y1 - y0) as usize;
        let mut mask = Mask { x0, y0, width: w as u32, height: h as u32, data: vec![0; w * h] };
        let baseline = origin.y.round() as i32;
        for g in &glyphs {
            if g.glyph == ZERO_WIDTH_GLYPH {
                continue;
            }
            let bm = fonts.rasterize(g.font, g.glyph, dsize);
            if bm.is_empty() {
                continue;
            }
            let gx = (origin.x + g.x + g.dx).round() as i32 + bm.left;
            let gy = baseline - (g.dy.round() as i32) - bm.top;
            for by in 0..bm.height as i32 {
                let my = gy + by - y0;
                if my < 0 || my >= h as i32 {
                    continue;
                }
                for bx in 0..bm.width as i32 {
                    let mx = gx + bx - x0;
                    if mx < 0 || mx >= w as i32 {
                        continue;
                    }
                    let c = bm.coverage[(by * bm.width as i32 + bx) as usize];
                    let i = my as usize * w + mx as usize;
                    mask.data[i] = mask.data[i].max(c);
                }
            }
        }
        box_blur(&mut mask.data, w, h, blur_r, 3);
        self.fill_mask(&mask, &Shader::Solid(color));
    }

    /// Like `draw_text` but with letter spacing, small-caps synthesis
    /// (lowercase letters drawn as scaled capitals) and synthetic oblique
    /// (sheared glyphs) for faces lacking italic. Returns the advance in
    /// user units.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_text_ext(&mut self, fonts: &FontContext, font: FontId, size: f32, text: &str, x: f32, baseline_y: f32, color: Color, letter_spacing: f32, small_caps: bool, oblique: bool) -> f32 {
        if text.is_empty() || size <= 0.0 {
            return 0.0;
        }
        let m = self.transform();
        let aligned = is_identity(&m) || is_axis_aligned(&m);
        let sf = if aligned { m[3] } else { scale_factor(&m) };
        let dsize = size * sf;
        let origin = transform_point(&m, Point::new(x, baseline_y));
        let mut pen = 0.0f32; // device-space advance along the baseline
        let small_scale = 0.8;

        // Split the text into runs with their own size (small caps).
        let mut runs: Vec<(String, f32)> = Vec::new();
        if small_caps {
            for ch in text.chars() {
                let (c, s) = if ch.is_lowercase() { (ch.to_uppercase().collect::<String>(), small_scale) } else { (ch.to_string(), 1.0) };
                match runs.last_mut() {
                    Some((t, rs)) if *rs == s => t.push_str(&c),
                    _ => runs.push((c, s)),
                }
            }
        } else {
            runs.push((text.to_string(), 1.0));
        }
        let shear = if oblique { (12.0f32).to_radians().tan() } else { 0.0 };
        // Rotation part of the transform (device units per device unit),
        // normalised so that glyph bitmaps rendered at `dsize` map 1:1.
        let rot: Matrix = if aligned { IDENTITY } else { [m[0] / sf, m[1] / sf, m[2] / sf, m[3] / sf, 0.0, 0.0] };
        for (run, scale) in &runs {
            let rsize = dsize * scale;
            let glyphs = fonts.shape(font, rsize, run, letter_spacing * sf);
            for g in &glyphs {
                if g.glyph != ZERO_WIDTH_GLYPH {
                    if aligned && shear == 0.0 {
                        if let Some(cg) = fonts.color_glyph(g.font, g.glyph, rsize) {
                            // Emoji: draw the color image in device space.
                            let gx = origin.x + pen + g.x + g.dx;
                            let r = Rect::new(gx + cg.left, origin.y - g.dy - cg.top, cg.width, cg.height);
                            self.transforms.push(IDENTITY);
                            self.draw_image(&cg.image, r, color.a as f32 / 255.0);
                            self.transforms.pop();
                            continue;
                        }
                    }
                    if aligned {
                        let gx = origin.x + pen + g.x + g.dx;
                        let frac = gx.fract();
                        let quarter = ((frac * 4.0).round() as u8) % 4;
                        let bm = fonts.rasterize_subpixel(g.font, g.glyph, rsize, quarter);
                        if !bm.is_empty() {
                            let base_x = gx.floor() as i32 + bm.left;
                            let base_y = (origin.y - g.dy).round() as i32 - bm.top;
                            if shear == 0.0 {
                                self.draw_glyph(&bm, base_x, base_y, color);
                            } else {
                                // Shear around the baseline: x += (baseline - y) * shear
                                let mm = [1.0, 0.0, -shear, 1.0, base_x as f32 + shear * (origin.y.round() - base_y as f32), base_y as f32];
                                self.blit_mask_affine(&bm.coverage, bm.width, bm.height, &mm, color);
                            }
                        }
                    } else {
                        let bm = fonts.rasterize(g.font, g.glyph, rsize);
                        if !bm.is_empty() {
                            // Glyph origin on the baseline in device space.
                            let along = Point::new(pen + g.x + g.dx, 0.0);
                            let o = Point::new(origin.x + rot[0] * along.x, origin.y + rot[1] * along.x);
                            let local = [1.0, 0.0, -shear, 1.0, bm.left as f32 + shear * bm.top as f32, -(bm.top as f32)];
                            let mm = multiply(&[rot[0], rot[1], rot[2], rot[3], o.x, o.y], &local);
                            self.blit_mask_affine(&bm.coverage, bm.width, bm.height, &mm, color);
                        }
                    }
                }
            }
            pen += glyphs.last().map(|g| g.x + g.advance).unwrap_or(0.0);
        }
        pen / sf.max(1e-6)
    }

    /// Blit an 8-bit coverage mask through an affine matrix that maps mask
    /// pixel coordinates to device coordinates (bilinear resampling).
    pub(crate) fn blit_mask_affine(&mut self, cov: &[u8], w: u32, h: u32, m: &Matrix, color: Color) {
        if w == 0 || h == 0 || color.a == 0 {
            return;
        }
        let dev = transform_rect_bounds(m, Rect::new(0.0, 0.0, w as f32, h as f32));
        let b = self.bounds(&Rect::new(dev.x - 1.0, dev.y - 1.0, dev.w + 2.0, dev.h + 2.0));
        if b.is_empty() {
            return;
        }
        let inv = invert(m);
        let mask = self.current_mask();
        let cw = self.width as usize;
        let a = color.a as u32;
        for py in b.y0..b.y1 {
            let drow = py as usize * cw;
            for px in b.x0..b.x1 {
                let p = transform_point(&inv, Point::new(px as f32 + 0.5, py as f32 + 0.5));
                let c = sample_mask_bilinear(cov, w, h, p.x - 0.5, p.y - 0.5);
                if c == 0 {
                    continue;
                }
                let mut alpha = (c * a + 127) / 255;
                if let Some(mk) = &mask {
                    alpha = (alpha * mk.get(px, py) as u32 + 127) / 255;
                }
                if alpha > 0 {
                    let idx = drow + px as usize;
                    self.data[idx] = blend(self.data[idx], color, alpha);
                }
            }
        }
    }

    // ----------------------------------------------------------------- images

    /// Draw the `src` sub-rectangle of an image into `dest` (user space),
    /// honoring the current transform, clip and opacity.
    pub fn draw_image_region(&mut self, image: &Image, src: Rect, dest: Rect, opacity: f32) {
        if dest.is_empty() || src.is_empty() || image.width == 0 || image.height == 0 || opacity <= 0.0 {
            return;
        }
        let op = opacity.clamp(0.0, 1.0);
        let m = self.transform();
        let mask = self.current_mask();
        let w = self.width as usize;
        if is_identity(&m) || is_axis_aligned(&m) {
            let d = transform_rect_aligned(&m, dest);
            let b = self.bounds(&d);
            if b.is_empty() {
                return;
            }
            let sx = src.w / d.w;
            let sy = src.h / d.h;
            let downscale = sx > 1.0 || sy > 1.0;
            for py in b.y0..b.y1 {
                let drow = py as usize * w;
                let v0 = src.y + (py as f32 - d.y) * sy;
                let v1 = src.y + (py as f32 + 1.0 - d.y) * sy;
                let cy = span_coverage(py as f32, d.y, d.bottom());
                for px in b.x0..b.x1 {
                    let u0 = src.x + (px as f32 - d.x) * sx;
                    let u1 = src.x + (px as f32 + 1.0 - d.x) * sx;
                    let [r, g, bl, a] = if !self.image_smoothing {
                        crate::canvas::sample_nearest(image, (u0 + u1) * 0.5, (v0 + v1) * 0.5)
                    } else if downscale {
                        crate::canvas::sample_box(image, u0.max(src.x), v0.max(src.y), u1.min(src.right()), v1.min(src.bottom()))
                    } else {
                        crate::canvas::sample_bilinear(image, (u0 + u1) * 0.5 - 0.5, (v0 + v1) * 0.5 - 0.5)
                    };
                    let cx = span_coverage(px as f32, d.x, d.right());
                    let mut alpha = (a * op * cx * cy + 0.5) as u32;
                    if let Some(mk) = &mask {
                        alpha = (alpha * mk.get(px, py) as u32 + 127) / 255;
                    }
                    if alpha == 0 {
                        continue;
                    }
                    let idx = drow + px as usize;
                    self.data[idx] = blend_image(self.data[idx], [r, g, bl], alpha);
                }
            }
            return;
        }
        // General transform: inverse-map each device pixel.
        let dev = transform_rect_bounds(&m, dest);
        let b = self.bounds(&dev);
        if b.is_empty() {
            return;
        }
        let inv = invert(&m);
        let sx = src.w / dest.w;
        let sy = src.h / dest.h;
        for py in b.y0..b.y1 {
            let drow = py as usize * w;
            for px in b.x0..b.x1 {
                // 2×2 supersampling for edge anti-aliasing.
                let mut acc = [0.0f32; 4];
                let mut hits = 0;
                for (ox, oy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                    let p = transform_point(&inv, Point::new(px as f32 + ox, py as f32 + oy));
                    if !dest.contains(p.x, p.y) {
                        continue;
                    }
                    let u = src.x + (p.x - dest.x) * sx;
                    let v = src.y + (p.y - dest.y) * sy;
                    let s = if self.image_smoothing { crate::canvas::sample_bilinear(image, u - 0.5, v - 0.5) } else { crate::canvas::sample_nearest(image, u, v) };
                    for i in 0..3 {
                        acc[i] += s[i] * s[3];
                    }
                    acc[3] += s[3];
                    hits += 1;
                }
                if hits == 0 || acc[3] <= 0.0 {
                    continue;
                }
                let r = acc[0] / acc[3];
                let g = acc[1] / acc[3];
                let bl = acc[2] / acc[3];
                let a = acc[3] / 4.0;
                let mut alpha = (a * op + 0.5) as u32;
                if let Some(mk) = &mask {
                    alpha = (alpha * mk.get(px, py) as u32 + 127) / 255;
                }
                if alpha == 0 {
                    continue;
                }
                let idx = drow + px as usize;
                self.data[idx] = blend_image(self.data[idx], [r, g, bl], alpha);
            }
        }
    }

    /// Tile an image across `area` starting at `origin` with the tile size
    /// `tile` (CSS background-repeat). `repeat_x/y` false = single tile.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_image_tiled(&mut self, image: &Image, area: Rect, origin: Point, tile: (f32, f32), repeat_x: bool, repeat_y: bool, opacity: f32) {
        if area.is_empty() || tile.0 < 0.5 || tile.1 < 0.5 {
            return;
        }
        let src = Rect::new(0.0, 0.0, image.width as f32, image.height as f32);
        self.push_clip(area);
        let mut start_x = origin.x;
        let mut start_y = origin.y;
        let (mut end_x, mut end_y) = (origin.x + tile.0, origin.y + tile.1);
        if repeat_x {
            start_x -= ((start_x - area.x) / tile.0).ceil().max(0.0) * tile.0;
            end_x = area.right();
        }
        if repeat_y {
            start_y -= ((start_y - area.y) / tile.1).ceil().max(0.0) * tile.1;
            end_y = area.bottom();
        }
        let max_tiles = 4096;
        let mut count = 0;
        let mut y = start_y;
        while y < end_y && count < max_tiles {
            let mut x = start_x;
            while x < end_x && count < max_tiles {
                let d = Rect::new(x, y, tile.0, tile.1);
                if d.intersect(&area).is_empty() {
                    x += tile.0;
                    continue;
                }
                self.draw_image_region(image, src, d, opacity);
                count += 1;
                x += tile.0;
            }
            y += tile.1;
        }
        self.pop_clip();
    }

    // ------------------------------------------------------------------ layers

    /// Begin an offscreen layer; everything drawn until `end_layer` is
    /// composited back with `opacity` (CSS `opacity` on a subtree).
    pub fn begin_layer(&mut self, bounds: Rect) {
        let m = self.transform();
        let dev = transform_rect_bounds(&m, bounds);
        let b = self.bounds(&Rect::new(dev.x - 1.0, dev.y - 1.0, dev.w + 2.0, dev.h + 2.0));
        let (x0, y0, w, h) = if b.is_empty() { (0, 0, 0, 0) } else { (b.x0, b.y0, (b.x1 - b.x0) as usize, (b.y1 - b.y0) as usize) };
        let mut saved = Vec::with_capacity(w * h);
        let cw = self.width as usize;
        for y in 0..h {
            let row = (y0 as usize + y) * cw + x0 as usize;
            saved.extend_from_slice(&self.data[row..row + w]);
        }
        self.layers.push(Layer { x0, y0, w, h, saved });
    }

    /// Device bounds (x0, y0, w, h) of the innermost open layer.
    pub fn layer_bounds(&self) -> Option<(i32, i32, usize, usize)> {
        self.layers.last().map(|l| (l.x0, l.y0, l.w, l.h))
    }

    /// End a layer, compositing each pixel with `opacity` × `alpha` (one
    /// byte per pixel of the layer bounds, CSS `mask-image`).
    pub fn end_layer_masked(&mut self, opacity: f32, alpha: &[u8]) {
        let Some(layer) = self.layers.pop() else { return };
        if alpha.len() != layer.w * layer.h {
            return;
        }
        let op = opacity.clamp(0.0, 1.0);
        let cw = self.width as usize;
        for y in 0..layer.h {
            let row = (layer.y0 as usize + y) * cw + layer.x0 as usize;
            for x in 0..layer.w {
                let drawn = self.data[row + x];
                let bg = layer.saved[y * layer.w + x];
                if drawn != bg {
                    let a = (alpha[y * layer.w + x] as f32 * op + 0.5) as u32;
                    self.data[row + x] = blend(bg, Color::from_u32(drawn), a);
                }
            }
        }
    }

    pub fn end_layer(&mut self, opacity: f32) {
        let Some(layer) = self.layers.pop() else { return };
        let op = opacity.clamp(0.0, 1.0);
        if op >= 1.0 {
            return;
        }
        let cw = self.width as usize;
        let a = (op * 255.0 + 0.5) as u32;
        for y in 0..layer.h {
            let row = (layer.y0 as usize + y) * cw + layer.x0 as usize;
            for x in 0..layer.w {
                let drawn = self.data[row + x];
                let bg = layer.saved[y * layer.w + x];
                if drawn != bg {
                    self.data[row + x] = blend(bg, Color::from_u32(drawn), a);
                }
            }
        }
    }

    /// Gaussian blur of a region in place (used for `filter: blur()` and
    /// `backdrop-filter`).
    pub fn blur_region(&mut self, rect: Rect, sigma: f32) {
        let m = self.transform();
        let dev = transform_rect_bounds(&m, rect);
        let b = self.bounds(&dev);
        if b.is_empty() || sigma <= 0.0 {
            return;
        }
        let r = blur_radius_for_sigma(sigma * scale_factor(&m));
        if r == 0 {
            return;
        }
        let w = (b.x1 - b.x0) as usize;
        let h = (b.y1 - b.y0) as usize;
        let cw = self.width as usize;
        let mut ch = [vec![0u8; w * h], vec![0u8; w * h], vec![0u8; w * h]];
        for y in 0..h {
            for x in 0..w {
                let p = self.data[(b.y0 as usize + y) * cw + b.x0 as usize + x];
                ch[0][y * w + x] = (p >> 16) as u8;
                ch[1][y * w + x] = (p >> 8) as u8;
                ch[2][y * w + x] = p as u8;
            }
        }
        for c in &mut ch {
            box_blur(c, w, h, r, 3);
        }
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                self.data[(b.y0 as usize + y) * cw + b.x0 as usize + x] = ((ch[0][i] as u32) << 16) | ((ch[1][i] as u32) << 8) | ch[2][i] as u32;
            }
        }
    }
}

/// Render a rounded box's coverage over `region` (device space).
fn shape_mask(shape: &RoundedBox, region: Rect) -> Mask {
    let x0 = region.x.floor() as i32;
    let y0 = region.y.floor() as i32;
    let x1 = region.right().ceil() as i32;
    let y1 = region.bottom().ceil() as i32;
    let w = (x1 - x0).max(0) as usize;
    let h = (y1 - y0).max(0) as usize;
    let mut data = vec![0u8; w * h];
    let r = shape.r;
    for y in y0..y1 {
        let cy = span_coverage(y as f32, r.y, r.bottom());
        if cy <= 0.0 {
            continue;
        }
        let row = (y - y0) as usize * w;
        for x in x0..x1 {
            if (x as f32 + 1.0) <= r.x || (x as f32) >= r.right() {
                continue;
            }
            data[row + (x - x0) as usize] = (shape.coverage(x, y) * 255.0 + 0.5) as u8;
        }
    }
    Mask { x0, y0, width: w as u32, height: h as u32, data }
}

/// Remove the coverage of `shape` from `mask` (mask *= 1 - shape).
fn knock_out(mask: &mut Mask, shape: &RoundedBox) {
    let r = shape.r;
    let x0 = (r.x.floor() as i32).max(mask.x0);
    let y0 = (r.y.floor() as i32).max(mask.y0);
    let x1 = (r.right().ceil() as i32).min(mask.x0 + mask.width as i32);
    let y1 = (r.bottom().ceil() as i32).min(mask.y0 + mask.height as i32);
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y - mask.y0) as usize * mask.width as usize + (x - mask.x0) as usize;
            if mask.data[i] == 0 {
                continue;
            }
            let c = shape.coverage(x, y);
            if c > 0.0 {
                mask.data[i] = (mask.data[i] as f32 * (1.0 - c) + 0.5) as u8;
            }
        }
    }
}

fn scale_radii(radii: Radii, sx: f32, sy: f32) -> Radii {
    radii.map(|(h, v)| (h * sx, v * sy))
}

/// Radii of the inner edge of a border ring.
fn inner_radii(radii: Radii, widths: [f32; 4]) -> Radii {
    let [wt, wr, wb, wl] = widths;
    [
        ((radii[0].0 - wl).max(0.0), (radii[0].1 - wt).max(0.0)),
        ((radii[1].0 - wr).max(0.0), (radii[1].1 - wt).max(0.0)),
        ((radii[2].0 - wr).max(0.0), (radii[2].1 - wb).max(0.0)),
        ((radii[3].0 - wl).max(0.0), (radii[3].1 - wb).max(0.0)),
    ]
}

/// Lighten/darken a border color for the 3-D styles. Sides: 0 top, 1
/// right, 2 bottom, 3 left. `inset` → top/left dark, bottom/right light.
fn shade_3d(c: Color, side: usize, inset: bool) -> Color {
    let dark = matches!(side, 0 | 3) == inset;
    let f = |v: u8| -> u8 {
        if dark {
            (v as f32 * 0.55) as u8
        } else {
            (v as f32 + (255.0 - v as f32) * 0.45) as u8
        }
    };
    Color { r: f(c.r), g: f(c.g), b: f(c.b), a: c.a }
}

/// Bilinear sample of an 8-bit mask at (u, v); outside → 0.
#[inline]
fn sample_mask_bilinear(cov: &[u8], w: u32, h: u32, u: f32, v: f32) -> u32 {
    if u <= -1.0 || v <= -1.0 || u >= w as f32 || v >= h as f32 {
        return 0;
    }
    let x0 = u.floor() as i32;
    let y0 = v.floor() as i32;
    let fx = u - x0 as f32;
    let fy = v - y0 as f32;
    let get = |x: i32, y: i32| -> f32 {
        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
            0.0
        } else {
            cov[y as usize * w as usize + x as usize] as f32
        }
    };
    let top = get(x0, y0) * (1.0 - fx) + get(x0 + 1, y0) * fx;
    let bot = get(x0, y0 + 1) * (1.0 - fx) + get(x0 + 1, y0 + 1) * fx;
    (top * (1.0 - fy) + bot * fy + 0.5) as u32
}

/// Public helper so callers can flatten+stroke without painting (used by
/// SVG hit testing later).
pub fn stroke_outline(path: &[PathSeg], style: &StrokeStyle) -> Polylines {
    let poly = flatten(path, &IDENTITY, TOLERANCE);
    stroke_polylines(&poly, style)
}

#[allow(dead_code)]
fn _bounds_unused(_: Bounds) {}
