//! Paint evaluation: solid colors and gradients sampled per pixel.

use crate::canvas::{Color, ColorStop, Matrix, Paint, Point};
use crate::path::{invert, transform_point};

const LUT_SIZE: usize = 1024;

/// A gradient's color table over its offset domain.
pub struct Lut {
    colors: Vec<Color>,
    lo: f32,
    /// 1 / (hi - lo); 0 for a degenerate domain.
    inv_span: f32,
}

impl Lut {
    fn new(stops: &[ColorStop], repeat: bool) -> Lut {
        let (lo, hi) = crate::canvas::gradient_domain(stops, repeat);
        let span = hi - lo;
        let inv_span = if span > 1e-9 && span.is_finite() { 1.0 / span } else { 0.0 };
        Lut { colors: build_lut_range(stops, lo, hi), lo, inv_span }
    }

    #[inline]
    fn lookup(&self, t: f32, repeat: bool) -> Color {
        let u = if t.is_nan() { 0.0 } else { (t - self.lo) * self.inv_span };
        let u = if repeat { u.rem_euclid(1.0) } else { u.clamp(0.0, 1.0) };
        let i = ((u * (LUT_SIZE - 1) as f32) + 0.5) as usize;
        self.colors[i.min(LUT_SIZE - 1)]
    }
}

/// A paint prepared for a specific drawing call: evaluates the color at a
/// device pixel.
pub enum Shader {
    Solid(Color),
    Linear {
        inv: Matrix,
        start: Point,
        /// Direction vector divided by its squared length.
        dir: Point,
        lut: Lut,
        repeat: bool,
    },
    Radial {
        inv: Matrix,
        center: Point,
        rx: f32,
        ry: f32,
        lut: Lut,
        repeat: bool,
    },
    Conic {
        inv: Matrix,
        center: Point,
        angle: f32,
        lut: Lut,
        repeat: bool,
    },
    /// A paint repeated over tiles (user space); pixels in the gaps
    /// between spaced tiles are transparent.
    Tiled {
        inv: Matrix,
        inner: Box<Shader>,
        origin: Point,
        size: (f32, f32),
        step: (f32, f32),
        repeat_x: bool,
        repeat_y: bool,
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
                Shader::Linear { inv: invert(m), start: *start, dir, lut: Lut::new(stops, *repeat), repeat: *repeat }
            }
            Paint::Radial { center, rx, ry, stops, repeat } => Shader::Radial {
                inv: invert(m),
                center: *center,
                rx: rx.max(1e-6),
                ry: ry.max(1e-6),
                lut: Lut::new(stops, *repeat),
                repeat: *repeat,
            },
            Paint::Conic { center, angle, stops, repeat } => Shader::Conic {
                inv: invert(m),
                center: *center,
                angle: *angle,
                lut: Lut::new(stops, *repeat),
                repeat: *repeat,
            },
            Paint::Tiled { paint, origin, size, step, repeat_x, repeat_y } => Shader::Tiled {
                inv: invert(m),
                inner: Box::new(Shader::new(paint, &crate::canvas::IDENTITY)),
                origin: *origin,
                size: *size,
                step: (step.0.max(1e-3), step.1.max(1e-3)),
                repeat_x: *repeat_x,
                repeat_y: *repeat_y,
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

    /// Color for a gradient parameter (linear/radial/conic shaders only).
    #[inline]
    pub fn lut_color(&self, t: f32, repeat: bool) -> Color {
        match self {
            Shader::Solid(c) => *c,
            Shader::Linear { lut, .. } | Shader::Radial { lut, .. } | Shader::Conic { lut, .. } => lut.lookup(t, repeat),
            Shader::Tiled { .. } => Color::TRANSPARENT,
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
        let d = Point::new(x as f32 + 0.5, y as f32 + 0.5);
        match self {
            Shader::Solid(c) => *c,
            Shader::Linear { inv, .. } | Shader::Radial { inv, .. } | Shader::Conic { inv, .. } | Shader::Tiled { inv, .. } => {
                self.color_at_user(transform_point(inv, d))
            }
        }
    }

    /// Color at a point in user space.
    fn color_at_user(&self, p: Point) -> Color {
        match self {
            Shader::Solid(c) => *c,
            Shader::Linear { start, dir, lut, repeat, .. } => {
                let t = (p.x - start.x) * dir.x + (p.y - start.y) * dir.y;
                lut.lookup(t, *repeat)
            }
            Shader::Radial { center, rx, ry, lut, repeat, .. } => {
                let dx = (p.x - center.x) / rx;
                let dy = (p.y - center.y) / ry;
                lut.lookup((dx * dx + dy * dy).sqrt(), *repeat)
            }
            Shader::Conic { center, angle, lut, repeat, .. } => {
                let dx = p.x - center.x;
                let dy = p.y - center.y;
                // 0 = up, clockwise (y grows downwards).
                let a = dx.atan2(-dy) - angle;
                let t = (a / std::f32::consts::TAU).rem_euclid(1.0);
                lut.lookup(t, *repeat)
            }
            Shader::Tiled { inner, origin, size, step, repeat_x, repeat_y, .. } => {
                let mut x = p.x - origin.x;
                let mut y = p.y - origin.y;
                if *repeat_x {
                    x = x.rem_euclid(step.0);
                }
                if *repeat_y {
                    y = y.rem_euclid(step.1);
                }
                if x < 0.0 || y < 0.0 || x >= size.0 || y >= size.1 {
                    return Color::TRANSPARENT;
                }
                inner.color_at_user(Point::new(origin.x + x, origin.y + y))
            }
        }
    }
}

/// Build a lookup table of colors over t ∈ [0, 1], interpolating stops in
/// premultiplied sRGB (so transparent stops do not darken).
pub fn build_lut(stops: &[ColorStop]) -> Vec<Color> {
    let clamped: Vec<ColorStop> = stops.iter().map(|s| ColorStop { offset: s.offset.clamp(0.0, 1.0), color: s.color }).collect();
    build_lut_range(&clamped, 0.0, 1.0)
}

/// Lookup table over offsets `lo..=hi`.
fn build_lut_range(stops: &[ColorStop], lo: f32, hi: f32) -> Vec<Color> {
    let mut stops: Vec<ColorStop> = stops.to_vec();
    if stops.is_empty() {
        return vec![Color::TRANSPARENT; LUT_SIZE];
    }
    // Enforce monotonic offsets (CSS: a stop earlier than the previous is
    // clamped to it).
    let mut maxo = f32::MIN;
    for s in &mut stops {
        if !s.offset.is_finite() {
            s.offset = if s.offset > 0.0 { hi } else { lo };
        }
        if s.offset < maxo {
            s.offset = maxo;
        }
        maxo = maxo.max(s.offset);
    }
    let mut lut = Vec::with_capacity(LUT_SIZE);
    // Offsets increase with i: walk the stop segments once.
    let mut seg = 0usize;
    for i in 0..LUT_SIZE {
        let t = lo + (hi - lo) * i as f32 / (LUT_SIZE - 1) as f32;
        while seg + 1 < stops.len() && stops[seg + 1].offset < t {
            seg += 1;
        }
        lut.push(sample_stops(&stops[seg.saturating_sub(1)..], t));
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
