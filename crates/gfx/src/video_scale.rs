//! Separable box / bilinear filtering of opaque decoded video. The regular
//! image painter also handles alpha, fractional edge coverage and transforms.
use crate::canvas::{Bounds, Canvas, Rect};
use crate::image::Image;

const UNIT: u32 = 256;

struct Filter {
    first: usize,
    weights: [u16; 5],
    count: usize,
}

impl Filter {
    fn new(pixel: i32, origin: f32, source: usize, scale: f32, area: bool) -> Self {
        let lo = ((pixel as f32 - origin) * scale).clamp(0.0, source as f32);
        let hi = ((pixel as f32 + 1.0 - origin) * scale).clamp(lo, source as f32);
        let mut f = Self { first: (lo as usize).min(source - 1), weights: [0; 5], count: 1 };
        if area && hi > lo {
            f.count = ((hi.ceil() as usize).min(source) - f.first).max(1);
            let mut previous = 0;
            for (i, weight) in f.weights[..f.count].iter_mut().enumerate() {
                let end = ((f.first + i + 1) as f32).min(hi);
                let total = (((end - lo) / (hi - lo)) * UNIT as f32).round() as u16;
                *weight = total - previous;
                previous = total;
            }
        } else {
            let center = ((lo + hi) * 0.5 - 0.5).clamp(0.0, (source - 1) as f32);
            f.first = center as usize;
            f.count = if f.first + 1 < source { 2 } else { 1 };
            f.weights[1] = ((center - f.first as f32) * UNIT as f32).round() as u16;
            f.weights[0] = UNIT as u16 - f.weights[1];
        }
        f
    }
}

/// `dest` has whole device-pixel edges and covers the entire source. Keep
/// eight fractional bits per axis, rounding only at the final RGB pixel.
pub(super) fn paint(canvas: &mut Canvas, image: &Image, dest: Rect, clip: Bounds) {
    let (width, height) = (image.width as usize, image.height as usize);
    let (sx, sy) = (width as f32 / dest.w, height as f32 / dest.h);
    let area = sx > 1.0 || sy > 1.0;
    let columns: Vec<_> = (clip.x0..clip.x1).map(|x| Filter::new(x, dest.x, width, sx, area)).collect();
    let left = columns[0].first;
    let last = columns.last().unwrap();
    let right = last.first + last.count;
    let mut row = vec![[0u32; 3]; right - left];
    let pixels = image.pixels();
    for y in clip.y0..clip.y1 {
        let vertical = Filter::new(y, dest.y, height, sy, area);
        row.fill([0; 3]);
        for (i, &weight) in vertical.weights[..vertical.count].iter().enumerate() {
            let base = ((vertical.first + i) * width + left) * 4;
            let scan = &pixels[base..base + row.len() * 4];
            for (out, p) in row.iter_mut().zip(scan.chunks_exact(4)) {
                for c in 0..3 {
                    out[c] += u32::from(p[c]) * u32::from(weight);
                }
            }
        }
        let base = y as usize * canvas.width as usize + clip.x0 as usize;
        for (dst, f) in canvas.data[base..base + columns.len()].iter_mut().zip(&columns) {
            let start = f.first - left;
            let mut rgb = [0u32; 3];
            for (&weight, pixel) in f.weights[..f.count].iter().zip(&row[start..]) {
                for c in 0..3 {
                    rgb[c] += pixel[c] * u32::from(weight);
                }
            }
            let [r, g, b] = rgb.map(|v| (v + UNIT * UNIT / 2) / (UNIT * UNIT));
            *dst = r << 16 | g << 8 | b;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::Color;

    #[test]
    fn opaque_video_matches_the_image_filter_at_clipped_scales() {
        let mut pixels = Vec::new();
        for i in 0..48 * 36 {
            pixels.extend([(i * 37) as u8, (i * 71) as u8, (i * 13) as u8, 255]);
        }
        let mut image = Image::new(48, 36, pixels);
        for (w, h) in [(24, 18), (16, 12), (13, 11), (43, 29), (92, 67), (96, 20), (12, 9)] {
            let render = |image: &Image| {
                let mut canvas = Canvas::new(80, 65);
                canvas.clear(Color::WHITE);
                canvas.push_clip(Rect::new(2.0, 3.0, 67.0, 50.0));
                canvas.draw_image(image, Rect::new(-2.0, 1.0, w as f32, h as f32), 1.0);
                canvas.data
            };
            image.video_frame = false;
            let reference = render(&image);
            image.video_frame = true;
            for (a, b) in render(&image).into_iter().zip(reference) {
                for shift in [0, 8, 16] {
                    assert!(((a >> shift) & 255).abs_diff((b >> shift) & 255) <= 2, "{w}x{h}");
                }
            }
        }
    }
}
