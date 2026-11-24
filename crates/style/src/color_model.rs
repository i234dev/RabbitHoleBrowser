//! CSS Color 4/5 values at full precision: absolute colors in any color
//! space with missing (`none`) components, relative colors, `color-mix()`
//! with any number of colors, `light-dark()`, `contrast-color()` and
//! `alpha()`. A parsed value ([`Spec`]) keeps what its specified value
//! serializes; resolving it gives an [`Abs`] color, which serializes as a
//! computed value and paints as 8-bit sRGB.
//!
//! Conversions follow CSS Color 4 §4.4 and §12: missing components carry
//! forward to analogous components of the destination space, and a hue
//! that is powerless after a conversion becomes missing.

use crate::color_calc::{self, Calc, Ty};
use crate::color_spaces as cs;
use crate::values::components;
use rabbithole_css::Token;
use rabbithole_gfx::Color;

type V3 = [f64; 3];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    Srgb,
    SrgbLinear,
    DisplayP3,
    DisplayP3Linear,
    A98Rgb,
    ProPhotoRgb,
    Rec2020,
    XyzD50,
    XyzD65,
    Lab,
    Lch,
    Oklab,
    Oklch,
    Hsl,
    Hwb,
}

impl Space {
    /// A `<predefined-rgb>` or `<xyz-space>` of `color()`.
    pub fn predefined(name: &str) -> Option<Space> {
        Some(match name {
            "srgb" => Space::Srgb,
            "srgb-linear" => Space::SrgbLinear,
            "display-p3" => Space::DisplayP3,
            "display-p3-linear" => Space::DisplayP3Linear,
            "a98-rgb" => Space::A98Rgb,
            "prophoto-rgb" => Space::ProPhotoRgb,
            "rec2020" => Space::Rec2020,
            "xyz" | "xyz-d65" => Space::XyzD65,
            "xyz-d50" => Space::XyzD50,
            _ => return None,
        })
    }

    /// A `<color-space>` of a `<color-interpolation-method>`.
    pub fn interpolation(name: &str) -> Option<Space> {
        Space::predefined(name).or(match name {
            "lab" => Some(Space::Lab),
            "lch" => Some(Space::Lch),
            "oklab" => Some(Space::Oklab),
            "oklch" => Some(Space::Oklch),
            "hsl" => Some(Space::Hsl),
            "hwb" => Some(Space::Hwb),
            _ => None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Space::Srgb => "srgb",
            Space::SrgbLinear => "srgb-linear",
            Space::DisplayP3 => "display-p3",
            Space::DisplayP3Linear => "display-p3-linear",
            Space::A98Rgb => "a98-rgb",
            Space::ProPhotoRgb => "prophoto-rgb",
            Space::Rec2020 => "rec2020",
            Space::XyzD50 => "xyz-d50",
            Space::XyzD65 => "xyz-d65",
            Space::Lab => "lab",
            Space::Lch => "lch",
            Space::Oklab => "oklab",
            Space::Oklch => "oklch",
            Space::Hsl => "hsl",
            Space::Hwb => "hwb",
        }
    }

    pub fn hue_index(self) -> Option<usize> {
        match self {
            Space::Hsl | Space::Hwb => Some(0),
            Space::Lch | Space::Oklch => Some(2),
            _ => None,
        }
    }

    fn is_rgb_like(self) -> bool {
        !matches!(self, Space::Lab | Space::Lch | Space::Oklab | Space::Oklch | Space::Hsl | Space::Hwb)
    }
}

/// Analogous component categories (CSS Color 4 §12.2).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cat {
    Red,
    Green,
    Blue,
    Light,
    Colorful,
    Hue,
    OppA,
    OppB,
}

fn category(space: Space, i: usize) -> Option<Cat> {
    use Cat::*;
    if space.is_rgb_like() {
        return Some([Red, Green, Blue][i]);
    }
    match space {
        Space::Lab | Space::Oklab => Some([Light, OppA, OppB][i]),
        Space::Lch | Space::Oklch => Some([Light, Colorful, Hue][i]),
        Space::Hsl => Some([Hue, Colorful, Light][i]),
        Space::Hwb => (i == 0).then_some(Hue),
        _ => None,
    }
}

fn index_of(space: Space, cat: Cat) -> Option<usize> {
    (0..3).find(|&i| category(space, i) == Some(cat))
}

pub const ALPHA_MISSING: u8 = 8;

/// An absolute color: components in its space's units (hsl/hwb in
/// percent, hues in degrees), alpha in [0, 1], missing components as bits
/// (1 << channel, `ALPHA_MISSING`). `legacy` colors (named, hex, `rgb()`,
/// `hsl()`, `hwb()`) serialize as `rgb()`/`rgba()` unless something is
/// missing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Abs {
    pub space: Space,
    pub c: V3,
    pub alpha: f64,
    pub missing: u8,
    pub legacy: bool,
}

/// sRGB of HSL (saturation and lightness in percent).
pub fn hsl_rgb(h: f64, s: f64, l: f64) -> V3 {
    hsl_to_srgb(h, s, l)
}

fn hsl_to_srgb(h: f64, s: f64, l: f64) -> V3 {
    let (s, l) = (s / 100.0, l / 100.0);
    let f = |n: f64| {
        let k = (n + h / 30.0).rem_euclid(12.0);
        let a = s * l.min(1.0 - l);
        l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
    };
    [f(0.0), f(8.0), f(4.0)]
}

fn hwb_to_srgb(h: f64, w: f64, b: f64) -> V3 {
    let (w, b) = (w / 100.0, b / 100.0);
    if w + b >= 1.0 {
        let g = w / (w + b);
        return [g; 3];
    }
    hsl_to_srgb(h, 100.0, 50.0).map(|v| v * (1.0 - w - b) + w)
}

/// HSL of sRGB (CSS Color 4 sample code): hue NaN when achromatic.
fn srgb_to_hsl(c: V3) -> V3 {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let l = (min + max) / 2.0;
    let d = max - min;
    let (mut h, mut s) = (f64::NAN, 0.0);
    if d.abs() > 1e-12 {
        // Lightness within rounding of black or white has no saturation.
        s = if l.abs() < 1e-7 || (l - 1.0).abs() < 1e-7 { 0.0 } else { (max - l) / l.min(1.0 - l) };
        h = if max == c[0] {
            (c[1] - c[2]) / d + if c[1] < c[2] { 6.0 } else { 0.0 }
        } else if max == c[1] {
            (c[2] - c[0]) / d + 2.0
        } else {
            (c[0] - c[1]) / d + 4.0
        } * 60.0;
    }
    if s < 0.0 {
        h += 180.0;
        s = s.abs();
    }
    if h >= 360.0 {
        h -= 360.0;
    }
    [h, s * 100.0, l * 100.0]
}

/// The hue of sRGB as HWB uses it: out-of-gamut colors keep the hue that
/// converts back to them (no rotation for negative saturation).
fn srgb_hue(c: V3) -> f64 {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let d = max - min;
    if d.abs() <= 1e-12 {
        return f64::NAN;
    }
    let h = if max == c[0] {
        (c[1] - c[2]) / d + if c[1] < c[2] { 6.0 } else { 0.0 }
    } else if max == c[1] {
        (c[2] - c[0]) / d + 2.0
    } else {
        (c[0] - c[1]) / d + 4.0
    } * 60.0;
    h.rem_euclid(360.0)
}

fn to_polar(lab: V3) -> V3 {
    let c = (lab[1] * lab[1] + lab[2] * lab[2]).sqrt();
    let h = lab[2].atan2(lab[1]).to_degrees().rem_euclid(360.0);
    [lab[0], c, h]
}

fn from_polar(lch: V3) -> V3 {
    let r = lch[2].to_radians();
    [lch[0], lch[1] * r.cos(), lch[1] * r.sin()]
}

fn decode(v: V3) -> V3 {
    v.map(cs::gamma_decode)
}

/// Linear-light sRGB (not clipped) of components in `space`.
pub fn to_linear(space: Space, v: V3) -> V3 {
    match space {
        Space::Srgb => decode(v),
        Space::SrgbLinear => v,
        Space::Hsl => decode(hsl_to_srgb(v[0], v[1], v[2])),
        Space::Hwb => decode(hwb_to_srgb(v[0], v[1], v[2])),
        Space::Lab => cs::lab_to_linear(v[0], v[1], v[2]),
        Space::Lch => {
            let l = from_polar(v);
            cs::lab_to_linear(l[0], l[1], l[2])
        }
        Space::Oklab => cs::oklab_to_linear(v[0], v[1], v[2]),
        Space::Oklch => {
            let l = from_polar(v);
            cs::oklab_to_linear(l[0], l[1], l[2])
        }
        s => cs::predefined_linear(s.name(), v).unwrap_or([0.0; 3]),
    }
}

/// Components in `space` of a linear-light sRGB color. A polar space's
/// hue is NaN when the conversion has none.
pub fn from_linear(space: Space, lin: V3) -> V3 {
    match space {
        Space::Srgb => lin.map(cs::gamma_encode),
        Space::SrgbLinear => lin,
        Space::Hsl => srgb_to_hsl(lin.map(cs::gamma_encode)),
        Space::Hwb => {
            let rgb = lin.map(cs::gamma_encode);
            let w = rgb[0].min(rgb[1]).min(rgb[2]);
            let b = 1.0 - rgb[0].max(rgb[1]).max(rgb[2]);
            [srgb_hue(rgb), w * 100.0, b * 100.0]
        }
        Space::Lab => cs::linear_to_lab(lin),
        Space::Lch => to_polar(cs::linear_to_lab(lin)),
        Space::Oklab => cs::linear_to_oklab(lin),
        Space::Oklch => to_polar(cs::linear_to_oklab(lin)),
        s => cs::predefined_coords(s.name(), lin).unwrap_or([0.0; 3]),
    }
}

/// Whether the hue of components in `space` is powerless (achromatic
/// within the precision engines use).
fn powerless(space: Space, v: V3) -> bool {
    match space {
        Space::Hsl => v[0].is_nan() || v[1].abs() <= 1.05e-3,
        Space::Hwb => v[0].is_nan() || v[1] + v[2] >= 100.0 - 1.05e-3,
        Space::Lch => v[2].is_nan() || v[1] <= 0.0015 + 1e-7,
        Space::Oklch => v[2].is_nan() || v[1] <= 4e-6 + 1e-11,
        _ => false,
    }
}

impl Abs {
    pub fn new(space: Space, c: V3, alpha: f64) -> Abs {
        Abs { space, c, alpha, missing: 0, legacy: false }
    }

    pub fn from_gfx(c: Color) -> Abs {
        Abs {
            space: Space::Srgb,
            c: [c.r, c.g, c.b].map(|v| f64::from(v) / 255.0),
            alpha: f64::from(c.a) / 255.0,
            missing: 0,
            legacy: true,
        }
    }

    pub fn has(&self, i: usize) -> bool {
        self.missing & (1 << i) == 0
    }

    pub fn alpha_missing(&self) -> bool {
        self.missing & ALPHA_MISSING != 0
    }

    fn values(&self) -> V3 {
        [0, 1, 2].map(|i| if self.has(i) { self.c[i] } else { 0.0 })
    }

    pub fn alpha_value(&self) -> f64 {
        if self.alpha_missing() { 0.0 } else { self.alpha }
    }

    /// Linear-light sRGB, missing components as zero.
    pub fn linear(&self) -> V3 {
        to_linear(self.space, self.values())
    }

    /// The color in another space (CSS Color 4 §12.2 carrying forward of
    /// missing components; powerless hues become missing).
    pub fn convert(&self, to: Space) -> Abs {
        if to == self.space {
            return Abs { legacy: false, ..*self };
        }
        let mut carried: Vec<Cat> = Vec::new();
        for i in 0..3 {
            if !self.has(i) {
                if let Some(cat) = category(self.space, i) {
                    carried.push(cat);
                }
            }
        }
        match self.space {
            Space::Lab | Space::Oklab if !self.has(1) && !self.has(2) => carried.extend([Cat::Colorful, Cat::Hue]),
            Space::Hwb if !self.has(1) && !self.has(2) => carried.extend([Cat::Light, Cat::Colorful]),
            Space::Lch | Space::Oklch if !self.has(1) && !self.has(2) => carried.extend([Cat::OppA, Cat::OppB]),
            _ => {}
        }
        // A color with no components at all has none in any space.
        let all_missing = self.missing & 7 == 7;
        // A powerless hue is missing in the destination, and the color
        // converts as the achromatic color it is.
        let mut source = self.values();
        if let Some(h) = self.space.hue_index() {
            if self.has(h) && powerless(self.space, self.c) {
                carried.push(Cat::Hue);
                match self.space {
                    Space::Hsl | Space::Lch | Space::Oklch => source[1] = 0.0,
                    _ => {
                        let sum = source[1] + source[2];
                        if sum > 0.0 && sum < 100.0 {
                            source[1] *= 100.0 / sum;
                            source[2] *= 100.0 / sum;
                        }
                    }
                }
            }
        }
        let mut c = from_linear(to, to_linear(self.space, source));
        let mut missing = self.missing & ALPHA_MISSING;
        if all_missing || (to == Space::Hwb && carried.contains(&Cat::Light) && carried.contains(&Cat::Colorful)) {
            missing |= if all_missing { 7 } else { 0b110 };
        }
        for cat in carried {
            if let Some(j) = index_of(to, cat) {
                missing |= 1 << j;
            }
        }
        if let Some(h) = to.hue_index() {
            if powerless(to, c) {
                missing |= 1 << h;
                // Achromatic within precision: no colorfulness either.
                if matches!(to, Space::Hsl | Space::Lch | Space::Oklch) {
                    c[1] = 0.0;
                }
            }
        }
        for (i, v) in c.iter_mut().enumerate() {
            if missing & (1 << i) != 0 || v.is_nan() {
                *v = 0.0;
            }
        }
        Abs { space: to, c, alpha: self.alpha, missing, legacy: false }
    }

    /// 8-bit sRGB for painting: per-channel clipping, except that the ends
    /// of the lightness range of Lab-like spaces are white and black.
    pub fn to_gfx(&self) -> Color {
        let alpha = self.alpha_value().clamp(0.0, 1.0);
        if self.legacy || matches!(self.space, Space::Srgb | Space::Hsl | Space::Hwb) {
            let rgb = match self.space {
                Space::Srgb => self.values(),
                Space::Hsl | Space::Hwb => {
                    let v = self.values();
                    if self.space == Space::Hsl { hsl_to_srgb(v[0], v[1], v[2]) } else { hwb_to_srgb(v[0], v[1], v[2]) }
                }
                _ => self.linear().map(cs::gamma_encode),
            };
            // (Rounded to 1e-9 first so that halves reached through float
            // noise, like 0.3 + 0.2, round up.)
            let ch = |v: f64| ((if v.is_nan() { 0.0 } else { v } * 255.0 * 1e9).round() / 1e9).round().clamp(0.0, 255.0) as u8;
            return Color::rgba(ch(rgb[0]), ch(rgb[1]), ch(rgb[2]), (alpha * 255.0).round() as u8);
        }
        let lin = self.linear();
        let lin = match self.space {
            Space::Lab | Space::Lch => cs::lightness_ends(lin, self.values()[0], 100.0),
            Space::Oklab | Space::Oklch => cs::lightness_ends(lin, self.values()[0], 1.0),
            _ => lin,
        };
        cs::to_color(lin, alpha)
    }

    /// The computed-value serialization (CSS Color 4 §15).
    pub fn to_css(&self) -> String {
        if self.legacy && self.missing == 0 {
            return legacy_css(self.to_gfx());
        }
        let mut s = match self.space {
            Space::Hsl | Space::Hwb if self.missing == 0 => {
                let rgb = self.convert(Space::Srgb);
                return Abs { missing: 0, ..rgb }.to_css();
            }
            Space::Hsl | Space::Hwb | Space::Lab | Space::Lch | Space::Oklab | Space::Oklch => format!("{}(", self.space.name()),
            s => format!("color({} ", s.name()),
        };
        let percent = matches!(self.space, Space::Hsl | Space::Hwb);
        for i in 0..3 {
            if i != 0 {
                s.push(' ');
            }
            if !self.has(i) {
                s.push_str("none");
                continue;
            }
            let mut v = self.c[i];
            if Some(i) == self.space.hue_index() {
                v = normalize_hue(v);
            }
            s.push_str(&fmt(v));
            if percent && i != 0 {
                s.push('%');
            }
        }
        if self.alpha_missing() {
            s.push_str(" / none");
        } else if self.alpha != 1.0 {
            s.push_str(" / ");
            s.push_str(&fmt(self.alpha));
        }
        s.push(')');
        s
    }
}

/// A hue in [0, 360), values within rounding of 360 being 0.
fn normalize_hue(h: f64) -> f64 {
    let h = h.rem_euclid(360.0);
    if 360.0 - h < 1e-6 { 0.0 } else { h }
}

/// A number with six significant digits, as engines serialize colors.
fn fmt(v: f64) -> String {
    let v = if v.is_finite() { v } else { 0.0 };
    if v.abs() < 1e-12 {
        return "0".into();
    }
    let magnitude = v.abs().log10().floor() as i32;
    let decimals = (5 - magnitude).clamp(0, 15) as usize;
    let s = format!("{v:.decimals$}");
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { &s };
    if s == "-0" { "0".into() } else { s.to_string() }
}

/// `rgb()` / `rgba()` of an 8-bit color, alpha with two decimals when they
/// round-trip.
pub fn legacy_css(c: Color) -> String {
    if c.a == 255 {
        format!("rgb({}, {}, {})", c.r, c.g, c.b)
    } else {
        let alpha = f32::from(c.a) / 255.0;
        let short = (alpha * 100.0).round() / 100.0;
        let a = if (short * 255.0).round() as u8 == c.a { short } else { (alpha * 1000.0).round() / 1000.0 };
        format!("rgba({}, {}, {}, {})", c.r, c.g, c.b, a)
    }
}

// ------------------------------------------------------------- parsing

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Func {
    Rgb,
    Hsl,
    Hwb,
    Lab,
    Lch,
    Oklab,
    Oklch,
    Color,
}

impl Func {
    fn parse(name: &str) -> Option<Func> {
        Some(match name {
            "rgb" | "rgba" => Func::Rgb,
            "hsl" | "hsla" => Func::Hsl,
            "hwb" => Func::Hwb,
            "lab" => Func::Lab,
            "lch" => Func::Lch,
            "oklab" => Func::Oklab,
            "oklch" => Func::Oklch,
            "color" => Func::Color,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Func::Rgb => "rgb",
            Func::Hsl => "hsl",
            Func::Hwb => "hwb",
            Func::Lab => "lab",
            Func::Lch => "lch",
            Func::Oklab => "oklab",
            Func::Oklch => "oklch",
            Func::Color => "color",
        }
    }

    fn space(self) -> Space {
        match self {
            Func::Rgb | Func::Color => Space::Srgb,
            Func::Hsl => Space::Hsl,
            Func::Hwb => Space::Hwb,
            Func::Lab => Space::Lab,
            Func::Lch => Space::Lch,
            Func::Oklab => Space::Oklab,
            Func::Oklch => Space::Oklch,
        }
    }
}

/// How a channel accepts values: whether angles are allowed (hues), what
/// 100% is, and its clamping range for absolute colors.
#[derive(Clone, Copy)]
struct Channel {
    hue: bool,
    pct_ref: f64,
    /// A bare number is in percent units (hsl/hwb) or the channel's own.
    min: f64,
    max: f64,
}

const NO_CLAMP: (f64, f64) = (f64::NEG_INFINITY, f64::INFINITY);

fn channels(func: Func, space: Space) -> [Channel; 3] {
    let ch = |hue, pct_ref, (min, max): (f64, f64)| Channel { hue, pct_ref, min, max };
    let hue = ch(true, 0.0, NO_CLAMP);
    match func {
        Func::Rgb => [ch(false, 255.0, (0.0, 255.0)); 3],
        Func::Hsl => [hue, ch(false, 100.0, (0.0, f64::INFINITY)), ch(false, 100.0, NO_CLAMP)],
        Func::Hwb => [hue, ch(false, 100.0, NO_CLAMP), ch(false, 100.0, NO_CLAMP)],
        Func::Lab => [ch(false, 100.0, (0.0, 100.0)), ch(false, 125.0, NO_CLAMP), ch(false, 125.0, NO_CLAMP)],
        Func::Lch => [ch(false, 100.0, (0.0, 100.0)), ch(false, 150.0, (0.0, f64::INFINITY)), hue],
        Func::Oklab => [ch(false, 1.0, (0.0, 1.0)), ch(false, 0.4, NO_CLAMP), ch(false, 0.4, NO_CLAMP)],
        Func::Oklch => [ch(false, 1.0, (0.0, 1.0)), ch(false, 0.4, (0.0, f64::INFINITY)), hue],
        Func::Color => {
            let _ = space;
            [ch(false, 1.0, NO_CLAMP); 3]
        }
    }
}

fn keywords(func: Func, space: Space) -> [&'static str; 3] {
    match func {
        Func::Rgb => ["r", "g", "b"],
        Func::Hsl => ["h", "s", "l"],
        Func::Hwb => ["h", "w", "b"],
        Func::Lab | Func::Oklab => ["l", "a", "b"],
        Func::Lch | Func::Oklch => ["l", "c", "h"],
        Func::Color if matches!(space, Space::XyzD50 | Space::XyzD65) => ["x", "y", "z"],
        Func::Color => ["r", "g", "b"],
    }
}

/// A component as written.
#[derive(Clone, Debug, PartialEq)]
pub enum Comp {
    None,
    Num(f64),
    Pct(f64),
    /// An angle: degrees and its value and unit as written.
    Angle(f64, f64, String),
    Key(String),
    Calc(Calc),
}

impl Comp {
    fn parse(c: &[Token], hue: bool, keys: &[&str]) -> Option<Comp> {
        Some(match c {
            [Token::Ident(k)] if k.eq_ignore_ascii_case("none") => Comp::None,
            [Token::Ident(k)] if keys.iter().any(|n| k.eq_ignore_ascii_case(n)) => Comp::Key(k.to_ascii_lowercase()),
            [Token::Number(n)] => Comp::Num(f64::from(*n)),
            [Token::Percentage(p)] if !hue => Comp::Pct(f64::from(*p)),
            [Token::Dimension(n, u)] if hue => {
                let u = u.to_ascii_lowercase();
                let deg = match u.as_str() {
                    "deg" => f64::from(*n),
                    "rad" => f64::from(*n).to_degrees(),
                    "grad" => f64::from(*n) * 0.9,
                    "turn" => f64::from(*n) * 360.0,
                    _ => return None,
                };
                Comp::Angle(deg, f64::from(*n), u)
            }
            [Token::Function(f), ..] if color_calc::is_math_function(f) => {
                let calc = color_calc::parse(c, keys)?;
                let ty = calc.ty()?;
                let ok = ty == Ty::NUMBER || (ty == Ty::PERCENT && !hue) || (ty == Ty::ANGLE && hue);
                if !ok {
                    return None;
                }
                Comp::Calc(calc)
            }
            _ => return None,
        })
    }

    fn is_percent(&self) -> bool {
        match self {
            Comp::Pct(_) => true,
            Comp::Calc(c) => c.ty() == Some(Ty::PERCENT),
            _ => false,
        }
    }

    fn is_number(&self) -> bool {
        match self {
            Comp::Num(_) => true,
            Comp::Calc(c) => c.ty() == Some(Ty::NUMBER),
            _ => false,
        }
    }

    /// The value in channel units (`None` for missing). Percentages scale
    /// to `pct_ref`.
    fn value(&self, pct_ref: f64, key: &dyn Fn(&str) -> Option<Option<f64>>) -> Option<Option<f64>> {
        let lengths = |v: f64, u: &str| length_px(v, u);
        Some(match self {
            Comp::None => None,
            Comp::Num(n) => Some(*n),
            Comp::Pct(p) => Some(p / 100.0 * pct_ref),
            Comp::Angle(d, _, _) => Some(*d),
            Comp::Key(k) => key(k)?,
            Comp::Calc(c) => {
                let v = c.eval(&|k| key(k).map(|v| v.unwrap_or(0.0)), &lengths)?;
                if c.ty() == Some(Ty::PERCENT) {
                    Some(v / 100.0 * pct_ref)
                } else {
                    Some(v)
                }
            }
        })
    }

    fn specified_css(&self) -> String {
        match self {
            Comp::None => "none".into(),
            Comp::Num(n) => color_calc::fmt_number(*n),
            Comp::Pct(p) => format!("{}%", color_calc::fmt_number(*p)),
            Comp::Angle(_, v, u) => format!("{}{u}", color_calc::fmt_number(*v)),
            Comp::Key(k) => k.clone(),
            Comp::Calc(c) => c.clone().simplify().to_css(),
        }
    }
}

thread_local! {
    /// Font size (px) for `em` in color math, while styling an element.
    static FONT_SIZE: std::cell::Cell<f64> = const { std::cell::Cell::new(16.0) };
}

pub fn set_font_size(px: f32) {
    FONT_SIZE.with(|f| f.set(f64::from(px)));
}

fn length_px(v: f64, unit: &str) -> Option<f64> {
    let fs = FONT_SIZE.with(|f| f.get());
    Some(match unit {
        "em" => v * fs,
        "rem" => v * 16.0,
        "ex" => v * fs * 0.5,
        "ch" => v * fs * 0.5,
        "lh" => v * fs * 1.2,
        _ => {
            let t = [Token::Dimension(v as f32, unit.to_string())];
            match crate::values::parse_length(&t)? {
                crate::values::LengthValue::Px(p) => f64::from(p),
                _ => return None,
            }
        }
    })
}

/// A color as specified.
#[derive(Clone, Debug, PartialEq)]
pub enum Spec {
    /// `rgb()`, `hsl()`, `hwb()`, `lab()`, ... with absolute components.
    Abs { func: Func, space: Space, comps: Box<[Comp; 3]>, alpha: Option<Comp>, legacy_syntax: bool },
    /// A named color, `transparent` or a hex color.
    Fixed(Option<String>, Color),
    System(String),
    Current,
    Relative { func: Func, space: Space, origin: Box<Spec>, comps: Box<[Comp; 3]>, alpha: Option<Comp> },
    Mix { method: Option<(Space, cs::HueMethod)>, items: Vec<(Spec, Option<Comp>)> },
    LightDark(Box<Spec>, Box<Spec>),
    Contrast(Box<Spec>),
    Alpha(Box<Spec>, Comp),
}

fn strip(c: &[Token]) -> &[Token] {
    match c {
        [Token::Function(_), inner @ .., Token::CloseParen] => inner,
        [Token::Function(_), inner @ ..] => inner,
        _ => c,
    }
}

fn split_top_commas(toks: &[Token]) -> Vec<&[Token]> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, t) in toks.iter().enumerate() {
        match t {
            Token::Function(_) | Token::OpenParen => depth += 1,
            Token::CloseParen => depth -= 1,
            Token::Comma if depth == 0 => {
                out.push(&toks[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&toks[start..]);
    out
}

/// Space-separated channels with an optional `/ alpha`.
fn modern_args(inner: &[Token]) -> Option<(Vec<&[Token]>, Option<&[Token]>)> {
    let mut parts = components(inner);
    if parts.iter().any(|p| matches!(*p, [Token::Comma])) {
        return None;
    }
    let alpha = if let Some(i) = parts.iter().position(|p| matches!(*p, [Token::Delim('/')])) {
        if i + 2 != parts.len() {
            return None;
        }
        let a = parts.pop()?;
        parts.pop();
        Some(a)
    } else {
        None
    };
    Some((parts, alpha))
}

pub fn system_color_name(name: &str) -> bool {
    cs::system_color(name).is_some()
}

impl Spec {
    /// Parse one component value as a color.
    pub fn parse(c: &[Token]) -> Option<Spec> {
        match c {
            [Token::Ident(name)] => {
                let l = name.to_ascii_lowercase();
                match l.as_str() {
                    "currentcolor" => Some(Spec::Current),
                    "transparent" => Some(Spec::Fixed(Some(l), Color::TRANSPARENT)),
                    _ => {
                        if let Some(c) = crate::values::named_color(&l) {
                            Some(Spec::Fixed(Some(l), c))
                        } else if system_color_name(&l) {
                            Some(Spec::System(l))
                        } else {
                            None
                        }
                    }
                }
            }
            [Token::Hash(h)] => crate::values::hex_color(h).map(|c| Spec::Fixed(None, c)),
            [Token::Function(f), rest @ ..] => {
                let name = f.to_ascii_lowercase();
                let relative = rest.iter().find(|t| !matches!(t, Token::Whitespace))
                    .is_some_and(|t| matches!(t, Token::Ident(k) if k.eq_ignore_ascii_case("from")));
                if let Some(func) = Func::parse(&name) {
                    return if relative { Spec::parse_relative(func, strip(c)) } else { Spec::parse_absolute(func, &name, strip(c)) };
                }
                match name.as_str() {
                    "color-mix" => Spec::parse_mix(strip(c)),
                    "light-dark" => {
                        let parts = split_top_commas(strip(c));
                        let [a, b] = parts.as_slice() else { return None };
                        let one = |p: &[Token]| -> Option<Spec> {
                            let comps = components(p);
                            let [c] = comps.as_slice() else { return None };
                            Spec::parse(c)
                        };
                        Some(Spec::LightDark(Box::new(one(a)?), Box::new(one(b)?)))
                    }
                    "contrast-color" => {
                        let comps = components(strip(c));
                        let [c] = comps.as_slice() else { return None };
                        Some(Spec::Contrast(Box::new(Spec::parse(c)?)))
                    }
                    "alpha" => {
                        let (parts, a) = modern_args(strip(c))?;
                        let [from, origin] = parts.as_slice() else { return None };
                        if !crate::values::is_ident(from, "from") {
                            return None;
                        }
                        let origin = Spec::parse(origin)?;
                        let alpha = Comp::parse(a?, false, &["alpha"])?;
                        Some(Spec::Alpha(Box::new(origin), alpha))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn parse_absolute(func: Func, name: &str, inner: &[Token]) -> Option<Spec> {
        let parts = components(inner);
        let legacy_syntax = parts.iter().any(|p| matches!(*p, [Token::Comma]));
        if legacy_syntax {
            if !matches!(func, Func::Rgb | Func::Hsl) {
                return None;
            }
            if !matches!(parts.len(), 5 | 7) || parts.iter().enumerate().any(|(i, p)| (i % 2 == 1) != matches!(*p, [Token::Comma])) {
                return None;
            }
            let chans = channels(func, func.space());
            let mut comps = Vec::new();
            for (i, p) in [parts[0], parts[2], parts[4]].into_iter().enumerate() {
                let comp = Comp::parse(p, chans[i].hue, &[])?;
                if comp == Comp::None {
                    return None;
                }
                comps.push(comp);
            }
            match func {
                Func::Rgb => {
                    let pct = comps.iter().all(Comp::is_percent);
                    let num = comps.iter().all(Comp::is_number);
                    if !pct && !num {
                        return None;
                    }
                }
                _ => {
                    if !comps[1].is_percent() || !comps[2].is_percent() {
                        return None;
                    }
                }
            }
            let alpha = match parts.get(6) {
                Some(a) => {
                    let a = Comp::parse(a, false, &[])?;
                    if a == Comp::None {
                        return None;
                    }
                    Some(a)
                }
                None => None,
            };
            let comps: [Comp; 3] = comps.try_into().ok()?;
            return Some(Spec::Abs { func, space: func.space(), comps: Box::new(comps), alpha, legacy_syntax: true });
        }
        let _ = name;
        let (mut vals, a) = modern_args(inner)?;
        let mut space = func.space();
        if func == Func::Color {
            let first = vals.first()?;
            space = Space::predefined(&crate::values::ident(first)?)?;
            vals.remove(0);
        }
        if vals.len() != 3 {
            return None;
        }
        let chans = channels(func, space);
        let mut comps = Vec::new();
        for (i, v) in vals.iter().enumerate() {
            comps.push(Comp::parse(v, chans[i].hue, &[])?);
        }
        let alpha = match a {
            Some(a) => Some(Comp::parse(a, false, &[])?),
            None => None,
        };
        let comps: [Comp; 3] = comps.try_into().ok()?;
        Some(Spec::Abs { func, space, comps: Box::new(comps), alpha, legacy_syntax: false })
    }

    fn parse_relative(func: Func, inner: &[Token]) -> Option<Spec> {
        let (vals, a) = modern_args(inner)?;
        let [from, origin, rest @ ..] = vals.as_slice() else { return None };
        if !crate::values::is_ident(from, "from") {
            return None;
        }
        let origin = Spec::parse(origin)?;
        let mut rest = rest.to_vec();
        let mut space = func.space();
        if func == Func::Color {
            let first = rest.first()?;
            space = Space::predefined(&crate::values::ident(first)?)?;
            rest.remove(0);
        }
        if rest.len() != 3 {
            return None;
        }
        let names = keywords(func, space);
        let keys = [names[0], names[1], names[2], "alpha"];
        let chans = channels(func, space);
        let mut comps = Vec::new();
        for (i, v) in rest.iter().enumerate() {
            comps.push(Comp::parse(v, chans[i].hue, &keys)?);
        }
        let alpha = match a {
            Some(a) => Some(Comp::parse(a, false, &keys)?),
            None => None,
        };
        let comps: [Comp; 3] = comps.try_into().ok()?;
        Some(Spec::Relative { func, space, origin: Box::new(origin), comps: Box::new(comps), alpha })
    }

    fn parse_mix(inner: &[Token]) -> Option<Spec> {
        let mut groups = split_top_commas(inner);
        let first = components(groups.first()?);
        let method = if first.first().is_some_and(|c| crate::values::is_ident(c, "in")) {
            let words: Vec<String> = first.iter().map(|c| crate::values::ident(c)).collect::<Option<_>>()?;
            let space = Space::interpolation(words.get(1)?)?;
            let hue = match words.len() {
                2 => cs::HueMethod::Shorter,
                4 if space.hue_index().is_some() && words[3] == "hue" => match words[2].as_str() {
                    "shorter" => cs::HueMethod::Shorter,
                    "longer" => cs::HueMethod::Longer,
                    "increasing" => cs::HueMethod::Increasing,
                    "decreasing" => cs::HueMethod::Decreasing,
                    _ => return None,
                },
                _ => return None,
            };
            groups.remove(0);
            Some((space, hue))
        } else {
            None
        };
        if groups.is_empty() {
            return None;
        }
        let mut items = Vec::new();
        for g in groups {
            let comps = components(g);
            let mut color = None;
            let mut pct = None;
            for c in comps {
                let is_pct = match c {
                    [Token::Percentage(p)] => {
                        if !(0.0..=100.0).contains(p) {
                            return None;
                        }
                        true
                    }
                    [Token::Function(f), ..] if color_calc::is_math_function(f) => true,
                    _ => false,
                };
                if is_pct {
                    if pct.is_some() {
                        return None;
                    }
                    let p = match c {
                        [Token::Percentage(p)] => Comp::Pct(f64::from(*p)),
                        _ => {
                            let calc = color_calc::parse(c, &[])?;
                            if calc.ty()? != Ty::PERCENT {
                                return None;
                            }
                            Comp::Calc(calc)
                        }
                    };
                    pct = Some(p);
                } else if color.is_some() {
                    return None;
                } else {
                    color = Some(Spec::parse(c)?);
                }
            }
            items.push((color?, pct));
        }
        Some(Spec::Mix { method, items })
    }

    /// Whether resolving needs `currentcolor`.
    pub fn uses_current(&self) -> bool {
        match self {
            Spec::Current => true,
            Spec::Relative { origin, .. } => origin.uses_current(),
            Spec::Mix { items, .. } => items.iter().any(|(c, _)| c.uses_current()),
            Spec::LightDark(a, b) => a.uses_current() || b.uses_current(),
            Spec::Contrast(c) | Spec::Alpha(c, _) => c.uses_current(),
            _ => false,
        }
    }

    /// Whether this is a plain legacy color (named, hex, legacy-function
    /// without missing components): its computed value is an 8-bit color.
    pub fn is_simple_legacy(&self) -> bool {
        match self {
            Spec::Fixed(..) | Spec::System(_) => true,
            Spec::Abs { func: Func::Rgb | Func::Hsl | Func::Hwb, comps, alpha, .. } => {
                !comps.iter().any(|c| *c == Comp::None) && alpha.as_ref() != Some(&Comp::None)
            }
            _ => false,
        }
    }

    /// The absolute color, with `current` as `currentcolor`.
    pub fn resolve(&self, current: Option<&Abs>) -> Option<Abs> {
        match self {
            Spec::Fixed(_, c) => Some(Abs::from_gfx(*c)),
            Spec::System(name) => cs::system_color(name).map(Abs::from_gfx),
            Spec::Current => current.copied(),
            Spec::Abs { func, space, comps, alpha, .. } => {
                let chans = channels(*func, *space);
                let no_key = |_: &str| -> Option<Option<f64>> { None };
                let mut c = [0.0; 3];
                let mut missing = 0u8;
                for i in 0..3 {
                    match comps[i].value(chans[i].pct_ref, &no_key)? {
                        None => missing |= 1 << i,
                        Some(v) => {
                            let v = if v.is_nan() { 0.0 } else { v };
                            c[i] = if chans[i].hue { if v.is_finite() { v } else { 0.0 } } else { v.clamp(chans[i].min, chans[i].max) };
                        }
                    }
                }
                let mut a = 1.0;
                if let Some(alpha) = alpha {
                    match alpha.value(1.0, &no_key)? {
                        None => missing |= ALPHA_MISSING,
                        Some(v) => a = if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) },
                    }
                }
                if *func == Func::Rgb {
                    c = c.map(|v| v / 255.0);
                }
                let legacy = matches!(func, Func::Rgb | Func::Hsl | Func::Hwb);
                Some(Abs { space: *space, c, alpha: a, missing, legacy })
            }
            Spec::Relative { func, space, origin, comps, alpha } => {
                let origin = origin.resolve(current)?;
                let o = origin.convert(*space);
                let names = keywords(*func, *space);
                let scale = if *func == Func::Rgb { 255.0 } else { 1.0 };
                let key = |k: &str| -> Option<Option<f64>> {
                    if k == "alpha" {
                        return Some((!o.alpha_missing()).then_some(o.alpha));
                    }
                    let i = names.iter().position(|n| *n == k)?;
                    Some(o.has(i).then(|| o.c[i] * scale))
                };
                let chans = channels(*func, *space);
                let mut c = [0.0; 3];
                let mut missing = 0u8;
                for i in 0..3 {
                    let pct_ref = if *func == Func::Rgb { 255.0 } else { chans[i].pct_ref };
                    match comps[i].value(pct_ref, &key)? {
                        None => missing |= 1 << i,
                        Some(v) => {
                            let v = if v.is_nan() { 0.0 } else { v };
                            // Lightness and chroma clamp as at parse time.
                            c[i] = match func {
                                Func::Lab | Func::Lch | Func::Oklab | Func::Oklch if !chans[i].hue => v.clamp(chans[i].min, chans[i].max),
                                _ => v,
                            };
                        }
                    }
                }
                let mut a = o.alpha;
                if o.alpha_missing() {
                    missing |= ALPHA_MISSING;
                }
                if let Some(alpha) = alpha {
                    missing &= !ALPHA_MISSING;
                    match alpha.value(1.0, &key)? {
                        None => missing |= ALPHA_MISSING,
                        Some(v) => a = if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) },
                    }
                }
                if *func == Func::Rgb {
                    c = c.map(|v| v / 255.0);
                }
                Some(Abs { space: *space, c, alpha: a, missing, legacy: false })
            }
            Spec::Mix { method, items } => {
                let (space, hue) = method.unwrap_or((Space::Oklab, cs::HueMethod::Shorter));
                let mut colors = Vec::with_capacity(items.len());
                for (c, p) in items {
                    let color = c.resolve(current)?;
                    let p = match p {
                        None => None,
                        Some(p) => Some(p.value(100.0, &|_: &str| None)??),
                    };
                    colors.push((color, p));
                }
                mix(&colors, space, hue)
            }
            Spec::LightDark(light, _) => light.resolve(current),
            Spec::Contrast(c) => {
                let c = c.resolve(current)?;
                let lin = c.linear().map(|v| v.clamp(0.0, 1.0));
                let y = 0.2126 * lin[0] + 0.7152 * lin[1] + 0.0722 * lin[2];
                let (with_white, with_black) = (1.05 / (y + 0.05), (y + 0.05) / 0.05);
                let v = if with_white >= with_black { 1.0 } else { 0.0 };
                Some(Abs { space: Space::Srgb, c: [v; 3], alpha: 1.0, missing: 0, legacy: true })
            }
            Spec::Alpha(origin, comp) => {
                let o = origin.resolve(current)?;
                let mut out = Abs { legacy: false, ..o };
                if o.legacy {
                    // Legacy origins become sRGB colors at full precision.
                    out = o.convert(Space::Srgb);
                    if o.space == Space::Srgb {
                        out.c = o.c;
                    }
                }
                let key = |k: &str| -> Option<Option<f64>> { (k == "alpha").then_some((!o.alpha_missing()).then_some(o.alpha)) };
                out.missing &= !ALPHA_MISSING;
                match comp.value(1.0, &key)? {
                    None => out.missing |= ALPHA_MISSING,
                    Some(v) => out.alpha = if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) },
                }
                Some(out)
            }
        }
    }

    /// The specified-value serialization.
    pub fn to_css(&self) -> String {
        match self {
            Spec::Fixed(Some(name), _) => name.clone(),
            Spec::Fixed(None, c) => legacy_css(*c),
            Spec::System(name) => name.clone(),
            Spec::Current => "currentcolor".into(),
            Spec::Abs { func, space, comps, alpha, .. } => {
                let has_calc = comps.iter().any(|c| matches!(c, Comp::Calc(_))) || matches!(alpha, Some(Comp::Calc(_)));
                let relative_units = comps.iter().chain(alpha.iter()).any(|c| matches!(c, Comp::Calc(calc) if calc.has_relative_units()));
                let legacy = matches!(func, Func::Rgb | Func::Hsl | Func::Hwb);
                if !has_calc || (legacy && !relative_units) {
                    if let Some(abs) = self.resolve(None) {
                        return abs.to_css();
                    }
                }
                let chans = channels(*func, *space);
                let mut s = match func {
                    Func::Color => format!("color({} ", space.name()),
                    f => format!("{}(", f.name()),
                };
                for i in 0..3 {
                    if i != 0 {
                        s.push(' ');
                    }
                    let c = &comps[i];
                    match c {
                        Comp::Calc(_) => s.push_str(&c.specified_css()),
                        _ => {
                            let single = Spec::Abs {
                                func: *func,
                                space: *space,
                                comps: Box::new([c.clone(), c.clone(), c.clone()]),
                                alpha: None,
                                legacy_syntax: false,
                            };
                            match single.resolve(None) {
                                Some(abs) if abs.has(i) => {
                                    let mut v = abs.c[i];
                                    if chans[i].hue {
                                        v = normalize_hue(v);
                                    }
                                    if *func == Func::Rgb {
                                        v *= 255.0;
                                    }
                                    s.push_str(&fmt(v));
                                }
                                _ => s.push_str("none"),
                            }
                        }
                    }
                }
                if let Some(a) = alpha {
                    match a {
                        Comp::Calc(_) => {
                            s.push_str(" / ");
                            s.push_str(&a.specified_css());
                        }
                        Comp::None => s.push_str(" / none"),
                        _ => {
                            let v = a.value(1.0, &|_: &str| None).flatten().unwrap_or(1.0).clamp(0.0, 1.0);
                            if v != 1.0 {
                                s.push_str(" / ");
                                s.push_str(&fmt(v));
                            }
                        }
                    }
                }
                s.push(')');
                s
            }
            Spec::Relative { func, space, origin, comps, alpha } => {
                let mut s = format!("{}(from {} ", func.name(), origin.to_css());
                if *func == Func::Color {
                    s.push_str(space.name());
                    s.push(' ');
                }
                let parts: Vec<String> = comps.iter().map(Comp::specified_css).collect();
                s.push_str(&parts.join(" "));
                if let Some(a) = alpha {
                    s.push_str(" / ");
                    s.push_str(&a.specified_css());
                }
                s.push(')');
                s
            }
            Spec::Mix { method, items } => {
                let mut parts = Vec::new();
                if let Some((space, hue)) = method {
                    if *space != Space::Oklab || *hue != cs::HueMethod::Shorter {
                        let mut m = format!("in {}", space.name());
                        let h = match hue {
                            cs::HueMethod::Shorter => None,
                            cs::HueMethod::Longer => Some("longer"),
                            cs::HueMethod::Increasing => Some("increasing"),
                            cs::HueMethod::Decreasing => Some("decreasing"),
                        };
                        if let Some(h) = h {
                            m.push(' ');
                            m.push_str(h);
                            m.push_str(" hue");
                        }
                        parts.push(m);
                    }
                }
                let pcts = specified_percentages(items);
                for ((c, _), p) in items.iter().zip(pcts) {
                    let mut item = c.to_css();
                    if let Some(p) = p {
                        item.push(' ');
                        item.push_str(&p);
                    }
                    parts.push(item);
                }
                format!("color-mix({})", parts.join(", "))
            }
            Spec::LightDark(a, b) => format!("light-dark({}, {})", a.to_css(), b.to_css()),
            Spec::Contrast(c) => format!("contrast-color({})", c.to_css()),
            Spec::Alpha(o, c) => format!("alpha(from {} / {})", o.to_css(), c.specified_css()),
        }
    }
}

/// The percentages a `color-mix()` serializes: omitted ones are filled in
/// from the given ones, and a 50%/50% pair (or a lone 100%) is omitted.
fn specified_percentages(items: &[(Spec, Option<Comp>)]) -> Vec<Option<String>> {
    let any_calc = items.iter().any(|(_, p)| matches!(p, Some(Comp::Calc(_))));
    let given: Vec<Option<f64>> = items.iter().map(|(_, p)| match p {
        Some(Comp::Pct(v)) => Some(*v),
        _ => None,
    }).collect();
    if any_calc || given.iter().all(Option::is_none) {
        return items.iter().map(|(_, p)| p.as_ref().map(Comp::specified_css)).collect();
    }
    let sum: f64 = given.iter().flatten().sum();
    let omitted = given.iter().filter(|p| p.is_none()).count();
    let fill = if omitted == 0 { 0.0 } else { ((100.0 - sum) / omitted as f64).max(0.0) };
    let all: Vec<f64> = given.iter().map(|p| p.unwrap_or(fill)).collect();
    if (all.len() == 2 && all.iter().all(|p| (*p - 50.0).abs() < 1e-9)) || (all.len() == 1 && (all[0] - 100.0).abs() < 1e-9) {
        return vec![None; all.len()];
    }
    all.into_iter().map(|p| Some(format!("{}%", color_calc::fmt_number(p)))).collect()
}

/// `color-mix()` of colors with optional percentages (CSS Color 5 §2).
pub fn mix(colors: &[(Abs, Option<f64>)], space: Space, hue: cs::HueMethod) -> Option<Abs> {
    if colors.is_empty() {
        return None;
    }
    let given: f64 = colors.iter().filter_map(|(_, p)| *p).sum();
    let omitted = colors.iter().filter(|(_, p)| p.is_none()).count();
    let fill = if omitted == 0 { 0.0 } else { ((100.0 - given) / omitted as f64).max(0.0) };
    let weights: Vec<f64> = colors.iter().map(|(_, p)| p.unwrap_or(fill).max(0.0)).collect();
    // Percentages summing to zero mix evenly into a transparent color.
    let total: f64 = weights.iter().sum();
    let alpha_mult = if total < 100.0 { total / 100.0 } else { 1.0 };
    let converted: Vec<Abs> = colors.iter().map(|(c, _)| c.convert(space)).collect();
    let mut acc = converted[0];
    let mut acc_w = weights[0];
    for (c, w) in converted.iter().zip(&weights).skip(1) {
        let sum = acc_w + w;
        let t = if sum > 0.0 { w / sum } else { 0.5 };
        acc = interpolate(&acc, c, t, hue);
        acc_w = sum;
    }
    if !acc.alpha_missing() {
        acc.alpha *= alpha_mult;
    }
    Some(acc)
}

/// Interpolate two colors of the same space at `t` (CSS Color 4 §12.3-4):
/// missing components take the other color's, premultiplied alpha, hue
/// arcs per the hue method.
pub fn interpolate(a: &Abs, b: &Abs, t: f64, method: cs::HueMethod) -> Abs {
    let space = a.space;
    let hue_i = space.hue_index();
    let mut missing = 0u8;
    let mut va = a.c;
    let mut vb = b.c;
    for i in 0..3 {
        match (a.has(i), b.has(i)) {
            (false, false) => missing |= 1 << i,
            (false, true) => va[i] = vb[i],
            (true, false) => vb[i] = va[i],
            _ => {}
        }
    }
    let (mut aa, mut ab) = (a.alpha, b.alpha);
    match (a.alpha_missing(), b.alpha_missing()) {
        (true, true) => {
            missing |= ALPHA_MISSING;
            aa = 1.0;
            ab = 1.0;
        }
        (true, false) => aa = ab,
        (false, true) => ab = aa,
        _ => {}
    }
    let alpha = aa + (ab - aa) * t;
    let mut out = [0.0; 3];
    for i in 0..3 {
        if Some(i) == hue_i {
            continue;
        }
        let (x, y) = (va[i] * aa, vb[i] * ab);
        let v = x + (y - x) * t;
        out[i] = if alpha != 0.0 { v / alpha } else { v };
    }
    if let Some(h) = hue_i {
        let (mut h1, mut h2) = (va[h].rem_euclid(360.0), vb[h].rem_euclid(360.0));
        let d = h2 - h1;
        match method {
            cs::HueMethod::Shorter => {
                if d > 180.0 {
                    h1 += 360.0;
                } else if d < -180.0 {
                    h2 += 360.0;
                }
            }
            cs::HueMethod::Longer => {
                if 0.0 < d && d < 180.0 {
                    h1 += 360.0;
                } else if -180.0 < d && d <= 0.0 {
                    h2 += 360.0;
                }
            }
            cs::HueMethod::Increasing => {
                if d < 0.0 {
                    h2 += 360.0;
                }
            }
            cs::HueMethod::Decreasing => {
                if d > 0.0 {
                    h1 += 360.0;
                }
            }
        }
        out[h] = (h1 + (h2 - h1) * t).rem_euclid(360.0);
    }
    for i in 0..3 {
        if missing & (1 << i) != 0 {
            out[i] = 0.0;
        }
    }
    Abs { space, c: out, alpha: if missing & ALPHA_MISSING != 0 { 0.0 } else { alpha.clamp(0.0, 1.0) }, missing, legacy: false }
}

/// sRGB (gamma-encoded, in [0, 1]) of a color by the CSS gamut mapping
/// algorithm (CSS Color 4 §13.2): chroma reduction in Oklch at constant
/// lightness and hue until the clipped color is within a just-noticeable
/// difference.
pub fn gamut_map_srgb(c: &Abs) -> V3 {
    // (Missing components are zero for painting: no carrying forward.)
    let srgb = from_linear(Space::Srgb, c.linear());
    let inside = |v: &V3| v.iter().all(|x| (-1e-6..=1.0 + 1e-6).contains(x));
    if inside(&srgb) {
        return srgb.map(|v| v.clamp(0.0, 1.0));
    }
    let lch = from_linear(Space::Oklch, c.linear());
    let lch = [lch[0], lch[1], if lch[2].is_nan() { 0.0 } else { lch[2] }];
    if lch[0] >= 1.0 {
        return [1.0; 3];
    }
    if lch[0] <= 0.0 {
        return [0.0; 3];
    }
    const JND: f64 = 0.02;
    const EPSILON: f64 = 0.0001;
    let clip = |v: V3| v.map(|x| x.clamp(0.0, 1.0));
    let to_srgb = |l: V3| from_linear(Space::Srgb, to_linear(Space::Oklch, l));
    let delta_eok = |a: V3, b: V3| {
        let (x, y) = (cs::linear_to_oklab(decode(a)), cs::linear_to_oklab(decode(b)));
        ((x[0] - y[0]).powi(2) + (x[1] - y[1]).powi(2) + (x[2] - y[2]).powi(2)).sqrt()
    };
    let clipped = clip(srgb);
    if delta_eok(clipped, srgb) < JND {
        return clipped;
    }
    let (mut min, mut max) = (0.0, lch[1]);
    let mut min_in_gamut = true;
    let mut current = lch;
    let mut result = clipped;
    while max - min > EPSILON {
        let chroma = (min + max) / 2.0;
        current[1] = chroma;
        let candidate = to_srgb(current);
        if min_in_gamut && inside(&candidate) {
            min = chroma;
            continue;
        }
        let clipped = clip(candidate);
        let e = delta_eok(clipped, candidate);
        if e < JND {
            if JND - e < EPSILON {
                return clipped;
            }
            min_in_gamut = false;
            min = chroma;
        } else {
            max = chroma;
        }
        result = clipped;
    }
    result
}

/// An `<alpha-value>` (`<number> | <percentage>`, math functions included)
/// as a number, not clamped.
pub fn alpha_value(c: &[Token]) -> Option<f64> {
    match c {
        [Token::Number(n)] => Some(f64::from(*n)),
        [Token::Percentage(p)] => Some(f64::from(*p) / 100.0),
        [Token::Function(f), ..] if color_calc::is_math_function(f) => {
            let calc = color_calc::parse(c, &[])?;
            let ty = calc.ty()?;
            let v = calc.eval(&|_| None, &length_px)?;
            if ty == Ty::NUMBER {
                Some(v)
            } else if ty == Ty::PERCENT {
                Some(v / 100.0)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// The specified-value serialization of an `<alpha-value>`: percentages
/// become numbers, math functions their simplified form.
pub fn alpha_specified(c: &[Token]) -> Option<String> {
    match c {
        [Token::Number(n)] => Some(color_calc::fmt_number(f64::from(*n))),
        [Token::Percentage(p)] => Some(color_calc::fmt_number(f64::from(*p) / 100.0)),
        [Token::Function(f), ..] if color_calc::is_math_function(f) => {
            let calc = color_calc::parse(c, &[])?;
            let ty = calc.ty()?;
            (ty == Ty::NUMBER || ty == Ty::PERCENT).then(|| calc.simplify().to_css())
        }
        _ => None,
    }
}

/// Parse and resolve a color component with the thread's current color.
thread_local! {
    /// The full-precision current color, when it is not an 8-bit color.
    static CURRENT: std::cell::Cell<Option<Abs>> = const { std::cell::Cell::new(None) };
}

/// Set the full-precision current color for `currentcolor` (with the
/// 8-bit one set by `values::set_current_color`).
pub fn set_current(c: Option<Abs>) {
    CURRENT.with(|v| v.set(c));
}

pub fn resolve_tokens(c: &[Token]) -> Option<(Spec, Abs)> {
    let spec = Spec::parse(c)?;
    let current = if spec.uses_current() {
        let gfx = crate::values::current_color()?;
        Some(CURRENT.with(|v| v.get()).filter(|a| a.to_gfx() == gfx).unwrap_or_else(|| Abs::from_gfx(gfx)))
    } else {
        None
    };
    let abs = spec.resolve(current.as_ref())?;
    Some((spec, abs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rabbithole_css::Tokenizer;

    fn computed(s: &str) -> String {
        let t = Tokenizer::tokenize(s);
        let comps = components(&t);
        Spec::parse(comps[0]).unwrap().resolve(None).unwrap().to_css()
    }

    fn specified(s: &str) -> String {
        let t = Tokenizer::tokenize(s);
        let comps = components(&t);
        Spec::parse(comps[0]).unwrap().to_css()
    }

    fn nums(s: &str) -> Vec<f64> {
        s.split(|c: char| c == ' ' || c == '(' || c == ',' || c == ')').filter_map(|p| p.parse().ok()).collect()
    }

    fn close(actual: &str, expected: &str) {
        let (a, e) = (nums(actual), nums(expected));
        let strip = |s: &str| s.replace(|c: char| c.is_ascii_digit() || c == '.' || c == '-', "");
        assert_eq!(strip(actual), strip(expected), "{actual} vs {expected}");
        assert_eq!(a.len(), e.len(), "{actual} vs {expected}");
        for (x, y) in a.iter().zip(&e) {
            assert!((x - y).abs() < 0.011, "{actual} vs {expected}");
        }
    }

    #[test]
    fn relative_colors_keep_missing_components() {
        close(&computed("hsl(from hsl(none 50% 50%) h s l)"), "hsl(none 50% 50%)");
        close(&computed("hsl(from hwb(none none 25%) h s l)"), "hsl(none 100% 37.5%)");
        close(&computed("hsl(from hwb(180 none none) h s l)"), "hsl(180 none none)");
        close(&computed("lch(from hsl(180 50% none) l c h)"), "lch(none 0 none)");
        close(&computed("hsl(from lab(50 none none) h s l)"), "hsl(none none 46.63266093%)");
        close(&computed("hsl(from lch(20 0.0015 180) h s l)"), "hsl(none 0% 18.93757452%)");
        close(&computed("hsl(from hwb(180 49.999% 50%) h s l)"), "hsl(none 0% 49.999%)");
        close(&computed("hsl(from hwb(180 49.998% 50%) h s l)"), "color(srgb 0.49998 0.5 0.5)");
        close(&computed("rgb(from rebeccapurple r g b)"), "color(srgb 0.4 0.2 0.6)");
        close(&computed("rgb(from rebeccapurple calc((r / 255) * 100%) calc((g / 255) * 100%) calc((b / 255) * 100%) / calc(alpha * 100%))"), "color(srgb 0.4 0.2 0.6)");
        close(&computed("lab(from lab(200 300 400 / 500%) l a b / alpha)"), "lab(100 300 400)");
        close(&computed("rgb(from rgb(none none none) r g b)"), "color(srgb none none none)");
        close(&computed("hsl(from hsl(50 50 50) h s calc((sin(l) + 1) * 50))"), "color(srgb 0.553 0.492 0.184)");
    }

    #[test]
    fn mixes() {
        close(&computed("color-mix(in hsl longer hue, red, white)"), "color(srgb 0.625 0.875 0.875)");
        close(&computed("color-mix(in hsl, hsl(none none none), hsl(30deg 40% 80%))"), "color(srgb 0.88 0.8 0.72)");
        close(&computed("color-mix(in hsl, hsl(120deg 40% 40% / none), hsl(0deg 40% 40% / none))"), "hsl(60 40% 40% / none)");
        close(&computed("color-mix(in oklab, oklab(0.1 0.2 0.3 / .4) 12.5%, oklab(0.5 0.6 0.7 / .8) 37.5%)"), "oklab(0.44285713 0.54285717 0.6428571 / 0.35)");
        close(&computed("color-mix(in srgb, red, green, blue)"), "color(srgb 0.333333 0.16732 0.333333)");
        close(&computed("color-mix(in hsl, hwb(180 100% 25%))"), "hsl(none 0% 80%)");
        close(&computed("color-mix(in hsl, hsl(120deg 10% 20%), hsl(30deg 30% 40%))"), "color(srgb 0.33 0.36 0.24)");
    }

    #[test]
    fn specified_values() {
        assert_eq!(specified("rgb(from rgb(20%, 40%, 60%, 80%) r g b / alpha)"), "rgb(from rgba(51, 102, 153, 0.8) r g b / alpha)");
        assert_eq!(specified("rgba(from rebeccapurple r calc(g * 2) 10)"), "rgb(from rebeccapurple r calc(2 * g) 10)");
        assert_eq!(specified("color-mix(in srgb, 70% red, 50% blue)"), "color-mix(in srgb, red 70%, blue 50%)");
        assert_eq!(specified("color-mix(in oklab, oklab(0.1 0.2 0.3), 25% oklab(0.5 0.6 0.7))"), "color-mix(oklab(0.1 0.2 0.3) 75%, oklab(0.5 0.6 0.7) 25%)");
        assert_eq!(specified("color-mix(in srgb, red 50%, green, blue)"), "color-mix(in srgb, red 50%, green 25%, blue 25%)");
        assert_eq!(specified("lab(calc(50 * 3) calc(0.5 - 1) calc(1.5) / calc(-0.5 + 1))"), "lab(calc(150) calc(-0.5) calc(1.5) / calc(0.5))");
        assert_eq!(specified("lch(calc(50%) 50% 0.5)"), "lch(calc(50%) 75 0.5)");
        assert_eq!(specified("hsl(from hsl(120deg none 50% / .5) h s l)"), "hsl(from hsl(120 none 50% / 0.5) h s l)");
        assert!(Spec::parse(&Tokenizer::tokenize("rgb(from rebeccapurple r 10deg 10)")).is_none());
        assert!(Spec::parse(&Tokenizer::tokenize("color(from color(srgb 0.7 0.5 0.3) srgb x g b)")).is_none());
        assert!(Spec::parse(&Tokenizer::tokenize("color-mix(in srgb, red -10%, green 50%, blue)")).is_none());
    }
}
