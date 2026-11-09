//! Paint evaluation: solid colors and gradients sampled per pixel.

use crate::canvas::{Color, ColorStop, Matrix, Paint, Point};
use crate::path::{invert, transform_point};

const LUT_SIZE: usize = 1024;

/// A paint prepared for a specific drawing call: evaluates the color at a
/// device pixel.
pub enum Shader {
    Solid(Color),
    Linear {
        inv: Matrix,
        start: Point,
        /// Direction vector divided by its squared length.
        dir: Point,
        lut: Vec<Color>,
        repeat: bool,
    },
    Radial {
        inv: Matrix,
        center: Point,
        rx: f32,
        ry: f32,
        lut: Vec<Color>,
        repeat: bool,
    },
}

impl Shader {
    /// Prepare `paint` for drawing under the transform `m` (user → device).
    pub fn new(paint: &Paint, m: &Matrix) -> Shader {
        match paint {
            Paint::Solid(c) => Shader::Solid(*c),
            Paint::Linear { start, end, stops, repeat } => {
                let dx = end.x - start.x;
                let dy = end.y - start.y;
                let len2 = dx * dx + dy * dy;
                let dir = if len2 > 1e-9 { Point::new(dx / len2, dy / len2) } else { Point::new(0.0, 0.0) };
                Shader::Linear { inv: invert(m), start: *start, dir, lut: build_lut(stops), repeat: *repeat }
            }
            Paint::Radial { center, rx, ry, stops, repeat } => Shader::Radial {
                inv: invert(m),
                center: *center,
                rx: rx.max(1e-6),
                ry: ry.max(1e-6),
                lut: build_lut(stops),
                repeat: *repeat,
            },
        }
    }

    /// For linear gradients: (t at pixel x0 of row y, dt per pixel) so a
    /// row can be evaluated incrementally.
    #[inline]
    pub fn row_params(&self, x0: i32, y: i32) -> Option<(f32, f32, bool)> {
        match self {
            Shader::Linear { inv, start, dir, repeat, .. } => {
                let p0 = transform_point(inv, Point::new(x0 as f32 + 0.5, y as f32 + 0.5));
                let p1 = transform_point(inv, Point::new(x0 as f32 + 1.5, y as f32 + 0.5));
                let t0 = (p0.x - start.x) * dir.x + (p0.y - start.y) * dir.y;
                let t1 = (p1.x - start.x) * dir.x + (p1.y - start.y) * dir.y;
                Some((t0, t1 - t0, *repeat))
            }
            _ => None,
        }
    }

    /// Color for a gradient parameter (linear/radial shaders only).
    #[inline]
    pub fn lut_color(&self, t: f32, repeat: bool) -> Color {
        match self {
            Shader::Solid(c) => *c,
            Shader::Linear { lut, .. } | Shader::Radial { lut, .. } => lut_lookup(lut, t, repeat),
        }
    }

    pub fn is_solid(&self) -> Option<Color> {
        match self {
            Shader::Solid(c) => Some(*c),
            _ => None,
        }
    }

    /// Color at device pixel centre (x + 0.5, y + 0.5).
    #[inline]
    pub fn color_at(&self, x: i32, y: i32) -> Color {
        match self {
            Shader::Solid(c) => *c,
            Shader::Linear { inv, start, dir, lut, repeat } => {
                let p = transform_point(inv, Point::new(x as f32 + 0.5, y as f32 + 0.5));
                let t = (p.x - start.x) * dir.x + (p.y - start.y) * dir.y;
                lut_lookup(lut, t, *repeat)
            }
            Shader::Radial { inv, center, rx, ry, lut, repeat } => {
                let p = transform_point(inv, Point::new(x as f32 + 0.5, y as f32 + 0.5));
                let dx = (p.x - center.x) / rx;
                let dy = (p.y - center.y) / ry;
                let t = (dx * dx + dy * dy).sqrt();
                lut_lookup(lut, t, *repeat)
            }
        }
    }
}

#[inline]
fn lut_lookup(lut: &[Color], t: f32, repeat: bool) -> Color {
    let t = if t.is_nan() {
        0.0
    } else if repeat {
        t.rem_euclid(1.0)
    } else {
        t.clamp(0.0, 1.0)
    };
    let i = ((t * (LUT_SIZE - 1) as f32) + 0.5) as usize;
    lut[i.min(LUT_SIZE - 1)]
}

/// Build a lookup table of colors over t ∈ [0, 1], interpolating stops in
/// premultiplied sRGB (so transparent stops do not darken).
pub fn build_lut(stops: &[ColorStop]) -> Vec<Color> {
    let mut stops: Vec<ColorStop> = stops.to_vec();
    if stops.is_empty() {
        return vec![Color::TRANSPARENT; LUT_SIZE];
    }
    // Enforce monotonic offsets (CSS: a stop earlier than the previous is
    // clamped to it) and sort stably.
    let mut maxo = f32::MIN;
    for s in &mut stops {
        s.offset = s.offset.clamp(0.0, 1.0);
        if s.offset < maxo {
            s.offset = maxo;
        }
        maxo = maxo.max(s.offset);
    }
    let mut lut = Vec::with_capacity(LUT_SIZE);
    for i in 0..LUT_SIZE {
        let t = i as f32 / (LUT_SIZE - 1) as f32;
        lut.push(sample_stops(&stops, t));
    }
    lut
}

fn sample_stops(stops: &[ColorStop], t: f32) -> Color {
    if t <= stops[0].offset {
        return stops[0].color;
    }
    let last = stops[stops.len() - 1];
    if t >= last.offset {
        return last.color;
    }
    for w in stops.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t >= a.offset && t <= b.offset {
            let span = b.offset - a.offset;
            let f = if span <= 1e-6 { 1.0 } else { (t - a.offset) / span };
            return lerp_premul(a.color, b.color, f);
        }
    }
    last.color
}

/// Interpolate two colors in premultiplied space.
pub fn lerp_premul(a: Color, b: Color, f: f32) -> Color {
    let fa = a.a as f32 / 255.0;
    let fb = b.a as f32 / 255.0;
    let alpha = fa + (fb - fa) * f;
    if alpha <= 0.0 {
        return Color::TRANSPARENT;
    }
    let ch = |ca: u8, cb: u8| -> u8 {
        let pa = ca as f32 * fa;
        let pb = cb as f32 * fb;
        let p = pa + (pb - pa) * f;
        (p / alpha).round().clamp(0.0, 255.0) as u8
    };
    Color { r: ch(a.r, b.r), g: ch(a.g, b.g), b: ch(a.b, b.b), a: (alpha * 255.0).round() as u8 }
}
