//! CSS Color 4/5 beyond sRGB notations: `hwb()`, `lab()`, `lch()`,
//! `oklab()`, `oklch()`, `color()` (sRGB, linear sRGB, Display P3, A98,
//! ProPhoto, Rec.2020, XYZ), `color-mix()`, `light-dark()` and the system
//! colors. Painting uses gamut-clipped 8-bit sRGB; gradients keep
//! the unclipped linear-light sRGB of the newer notations ([`Wide`]) so
//! interpolation sees the real color and clipping happens per sample.

use crate::values::{components, ColorValue};
use rabbithole_css::Token;
use rabbithole_gfx::Color;

type Rgb = [f64; 3];

fn strip(c: &[Token]) -> &[Token] {
    match c {
        [Token::Function(_), inner @ .., Token::CloseParen] => inner,
        [Token::Function(_), inner @ ..] => inner,
        _ => c,
    }
}

/// The modern space-separated channel list permits one optional slash-alpha.
fn args(c: &[Token]) -> Option<(Vec<&[Token]>, Option<&[Token]>)> {
    let mut parts = components(strip(c));
    let alpha = if let Some(i) = parts.iter().position(|p| matches!(*p, [Token::Delim('/')])) {
        if i + 2 != parts.len() { return None; }
        let a = parts.pop()?;
        parts.pop();
        Some(a)
    } else { None };
    if parts.iter().any(|p| matches!(*p, [Token::Comma])) { return None; }
    Some((parts, alpha))
}

/// RGB and HSL also support three comma-separated channels and optional alpha.
/// Legacy syntax excludes missing channels and mixed numeric/percentage RGB.
pub(crate) fn srgb_args(c: &[Token]) -> Option<(Vec<&[Token]>, Option<&[Token]>)> {
    let p = components(strip(c));
    if !p.iter().any(|v| matches!(*v, [Token::Comma])) {
        let (v, a) = args(c)?;
        return (v.len() == 3).then_some((v, a));
    }
    if !matches!(p.len(), 5 | 7) || p.iter().enumerate().any(|(i, v)|
        (i % 2 == 1) != matches!(*v, [Token::Comma]) || crate::values::is_ident(v, "none")) {
        return None;
    }
    let v = vec![p[0], p[2], p[4]];
    if matches!(c.first(), Some(Token::Function(f)) if f.eq_ignore_ascii_case("rgb") || f.eq_ignore_ascii_case("rgba")) {
        let percentages = v.iter().any(|v| matches!(*v, [Token::Percentage(_)]));
        if percentages && v.iter().any(|v| matches!(*v, [Token::Number(_)])) { return None; }
    }
    Some((v, p.get(6).copied()))
}

pub(crate) fn gamma_encode(x: f64) -> f64 {
    let s = x.signum();
    let x = x.abs();
    s * if x <= 0.0031308 { 12.92 * x } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 }
}

pub(crate) fn gamma_decode(x: f64) -> f64 {
    let s = x.signum();
    let x = x.abs();
    s * if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
}

fn mul(m: &[f64; 9], v: Rgb) -> Rgb {
    [m[0] * v[0] + m[1] * v[1] + m[2] * v[2], m[3] * v[0] + m[4] * v[1] + m[5] * v[2], m[6] * v[0] + m[7] * v[1] + m[8] * v[2]]
}

const XYZ65_TO_LSRGB: [f64; 9] = [
    3.2409699419045226, -1.537383177570094, -0.4986107602930034, -0.9692436362808796, 1.8759675015077202, 0.04155505740717559, 0.05563007969699366,
    -0.20397695888897652, 1.0569715142428786,
];
const LSRGB_TO_XYZ65: [f64; 9] = [
    0.41239079926595934, 0.357584339383878, 0.1804807884018343, 0.21263900587151027, 0.715168678767756, 0.07219231536073371, 0.01933081871559182,
    0.11919477979462598, 0.9505321522496607,
];
const D50_TO_D65: [f64; 9] = [
    0.955473421488075, -0.02309845494876471, 0.06325924320057072, -0.0283697093338637, 1.0099953980813041, 0.021041441191917323, 0.012314014864481998,
    -0.020507649298898964, 1.330365926242124,
];
const D65_TO_D50: [f64; 9] = [
    1.0479297925449969, 0.022946870601609652, -0.05019226628920524, 0.02962780877005599, 0.9904344267538799, -0.017073799063418826, -0.009243040646204504,
    0.015055191490298152, 0.7518742814281371,
];

pub(crate) fn to_color(lin: Rgb, a: f64) -> Color {
    // (A small bias keeps halves that went through linear light rounding
    // up, as they do when computed in 8-bit sRGB.)
    let c = |v: f64| (gamma_encode(v).clamp(0.0, 1.0) * 255.0 + 1e-4).round() as u8;
    Color::rgba(c(lin[0]), c(lin[1]), c(lin[2]), (a * 255.0).round() as u8)
}

fn linear_of(c: Color) -> Rgb {
    [gamma_decode(c.r as f64 / 255.0), gamma_decode(c.g as f64 / 255.0), gamma_decode(c.b as f64 / 255.0)]
}

pub(crate) fn oklab_to_linear(l: f64, a: f64, b: f64) -> Rgb {
    let l_ = l + 0.3963377773761749 * a + 0.2158037573099136 * b;
    let m_ = l - 0.1055613458156586 * a - 0.0638541728258133 * b;
    let s_ = l - 0.0894841775298119 * a - 1.2914855480194092 * b;
    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    [
        4.0767416621 * l3 - 3.3077115913 * m3 + 0.2309699292 * s3,
        -1.2684380046 * l3 + 2.6097574011 * m3 - 0.3413193965 * s3,
        -0.0041960863 * l3 - 0.7034186147 * m3 + 1.7076147010 * s3,
    ]
}

pub(crate) fn linear_to_oklab(c: Rgb) -> Rgb {
    let l = 0.4122214708 * c[0] + 0.5363325363 * c[1] + 0.0514459929 * c[2];
    let m = 0.2119034982 * c[0] + 0.6806995451 * c[1] + 0.1073969566 * c[2];
    let s = 0.0883024619 * c[0] + 0.2817188376 * c[1] + 0.6299787005 * c[2];
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

pub(crate) fn lab_to_linear(l: f64, a: f64, b: f64) -> Rgb {
    let fy = (l + 16.0) / 116.0;
    let fx = fy + a / 500.0;
    let fz = fy - b / 200.0;
    let inv = |t: f64| if t * t * t > 216.0 / 24389.0 { t * t * t } else { (116.0 * t - 16.0) / (24389.0 / 27.0) };
    let y = if l > 8.0 { fy * fy * fy } else { l / (24389.0 / 27.0) };
    let xyz50 = [inv(fx) * 0.3457 / 0.3585, y, inv(fz) * (1.0 - 0.3457 - 0.3585) / 0.3585];
    mul(&XYZ65_TO_LSRGB, mul(&D50_TO_D65, xyz50))
}

pub(crate) fn linear_to_lab(c: Rgb) -> Rgb {
    let xyz = mul(&D65_TO_D50, mul(&LSRGB_TO_XYZ65, c));
    let white = [0.3457 / 0.3585, 1.0, (1.0 - 0.3457 - 0.3585) / 0.3585];
    let f = |t: f64| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let (fx, fy, fz) = (f(xyz[0] / white[0]), f(xyz[1] / white[1]), f(xyz[2] / white[2]));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn polar(c: f64, h: f64) -> (f64, f64) {
    let r = h.to_radians();
    (c * r.cos(), c * r.sin())
}

const P3_TO_XYZ65: [f64; 9] = [0.4865709486482162, 0.26566769316909306, 0.1982172852343625, 0.2289745640697488, 0.6917385218365064, 0.079286914093745, 0.0, 0.04511338185890264, 1.043944368900976];
const A98_TO_XYZ65: [f64; 9] = [0.5766690429101305, 0.1855582379065463, 0.1882286462349947, 0.29734497525053605, 0.6273635662554661, 0.0752914584939978, 0.02703136138641234, 0.07068885253582723, 0.9913375368376388];
const REC2020_TO_XYZ65: [f64; 9] = [0.6369580483012914, 0.14461690358620832, 0.1688809751641721, 0.2627002120112671, 0.6779980715188708, 0.05930171646986196, 0.0, 0.028072693049087428, 1.060985057710791];
const PROPHOTO_TO_XYZ50: [f64; 9] = [0.7977604896723027, 0.13518583717574031, 0.0313493495815248, 0.2880711282292934, 0.7118432178101014, 0.00008565396060525902, 0.0, 0.0, 0.8251046025104601];

// All profile matrices are fixed, nonsingular RGB/XYZ transforms. Compute
// the inverse at compile time from the same primaries used for decoding.
const fn inverse(m: [f64; 9]) -> [f64; 9] {
    let [a,b,c,d,e,f,g,h,i] = m;
    let det = a*(e*i-f*h) - b*(d*i-f*g) + c*(d*h-e*g);
    [(e*i-f*h)/det,(c*h-b*i)/det,(b*f-c*e)/det,
     (f*g-d*i)/det,(a*i-c*g)/det,(c*d-a*f)/det,
     (d*h-e*g)/det,(b*g-a*h)/det,(a*e-b*d)/det]
}

pub(crate) fn predefined_coords(space: &str, linear: Rgb) -> Option<Rgb> {
    const TO_P3: [f64; 9] = inverse(P3_TO_XYZ65);
    const TO_A98: [f64; 9] = inverse(A98_TO_XYZ65);
    const TO_REC2020: [f64; 9] = inverse(REC2020_TO_XYZ65);
    const TO_PROPHOTO: [f64; 9] = inverse(PROPHOTO_TO_XYZ50);
    let xyz = || mul(&LSRGB_TO_XYZ65,linear);
    Some(match space {
        "srgb" => linear.map(gamma_encode),
        "srgb-linear" => linear,
        "xyz" | "xyz-d65" => xyz(),
        "xyz-d50" => mul(&D65_TO_D50,xyz()),
        "display-p3" => mul(&TO_P3,xyz()).map(gamma_encode),
        "display-p3-linear" => mul(&TO_P3,xyz()),
        "a98-rgb" => mul(&TO_A98,xyz()).map(|v| v.signum()*v.abs().powf(256.0/563.0)),
        "prophoto-rgb" => mul(&TO_PROPHOTO,mul(&D65_TO_D50,xyz())).map(|v| {
            if v.abs() <= 1.0/512.0 { 16.0*v } else { v.signum()*v.abs().powf(1.0/1.8) }
        }),
        "rec2020" => mul(&TO_REC2020,xyz()).map(|v| {
            let a = 1.09929682680944;
            if v.abs() < 0.018053968510807 { 4.5*v } else { v.signum()*(a*v.abs().powf(0.45)-(a-1.0)) }
        }),
        _ => return None,
    })
}

/// `color(<space> r g b)`: linear-light sRGB of the color.
pub(crate) fn predefined_linear(space: &str, v: Rgb) -> Option<Rgb> {
    let rgb_space = |m: &[f64; 9], decode: &dyn Fn(f64) -> f64| mul(&XYZ65_TO_LSRGB, mul(m, [decode(v[0]), decode(v[1]), decode(v[2])]));
    Some(match space {
        "srgb" => [gamma_decode(v[0]), gamma_decode(v[1]), gamma_decode(v[2])],
        "srgb-linear" => v,
        "display-p3" => rgb_space(
            &P3_TO_XYZ65,
            &gamma_decode,
        ),
        "display-p3-linear" => rgb_space(
            &P3_TO_XYZ65,
            &|x: f64| x,
        ),
        "a98-rgb" => rgb_space(
            &A98_TO_XYZ65,
            &|x: f64| x.signum() * x.abs().powf(563.0 / 256.0),
        ),
        "rec2020" => rgb_space(
            &REC2020_TO_XYZ65,
            &|x: f64| {
                let (a, b) = (1.09929682680944, 0.018053968510807);
                let s = x.signum();
                let x = x.abs();
                s * if x < b * 4.5 { x / 4.5 } else { ((x + a - 1.0) / a).powf(1.0 / 0.45) }
            },
        ),
        "prophoto-rgb" => {
            let d = |x: f64| {
                let s = x.signum();
                let x = x.abs();
                s * if x <= 16.0 / 512.0 { x / 16.0 } else { x.powf(1.8) }
            };
            let xyz50 = mul(
                &PROPHOTO_TO_XYZ50,
                [d(v[0]), d(v[1]), d(v[2])],
            );
            mul(&XYZ65_TO_LSRGB, mul(&D50_TO_D65, xyz50))
        }
        "xyz" | "xyz-d65" => mul(&XYZ65_TO_LSRGB, v),
        "xyz-d50" => mul(&XYZ65_TO_LSRGB, mul(&D50_TO_D65, v)),
        _ => return None,
    })
}

/// System colors (CSS Color 4 §6.2) as rendered by Blink in light mode;
/// the deprecated ones (§6.3) are the same as their replacements.
pub fn system_color(name: &str) -> Option<Color> {
    let name = match name {
        "activeborder" | "inactiveborder" | "threeddarkshadow" | "threedshadow" | "threedlightshadow" | "windowframe" => "buttonborder",
        "activecaption" | "appworkspace" | "background" | "inactivecaption" | "infobackground" | "menu" | "scrollbar" | "window" => "canvas",
        "buttonhighlight" | "buttonshadow" | "threedface" | "threedhighlight" => "buttonface",
        "captiontext" | "infotext" | "menutext" | "windowtext" => "canvastext",
        "inactivecaptiontext" => "graytext",
        n => n,
    };
    Some(match name {
        "canvas" | "field" => Color::rgb(255, 255, 255),
        "canvastext" | "fieldtext" | "buttontext" | "highlighttext" | "selecteditemtext" | "marktext" => Color::rgb(0, 0, 0),
        "linktext" => Color::rgb(0, 0, 238),
        "visitedtext" => Color::rgb(85, 26, 139),
        "activetext" => Color::rgb(255, 0, 0),
        "buttonface" => Color::rgb(239, 239, 239),
        "buttonborder" => Color::rgb(118, 118, 118),
        "graytext" => Color::rgb(109, 109, 109),
        "highlight" | "selecteditem" => Color::rgb(181, 213, 255),
        "mark" => Color::rgb(255, 255, 0),
        "accentcolor" => Color::rgb(0, 117, 255),
        "accentcolortext" => Color::rgb(255, 255, 255),
        _ => return None,
    })
}

/// A color kept at full precision: linear-light sRGB components (not
/// clipped to the sRGB gamut) and alpha.
pub type Wide = [f32; 4];

/// Separate the optional blend mode and the ordered layers. Each layer must
/// itself be one color component; callers parse it at the appropriate stage.
pub fn layer_parts(tokens: &[Token]) -> Option<(Option<String>, Vec<&[Token]>)> {
    let mut layers = Vec::new();
    let mut mode = None;
    for (i, group) in split_top_commas(strip(tokens)).into_iter().enumerate() {
        let parts = components(group);
        let [color] = parts.as_slice() else { return None };
        if i == 0 {
            if let Some(name) = crate::values::ident(color) {
                if rabbithole_gfx::canvas_ext::blend_mode_index(&name).is_some_and(|m| m < 16) {
                    mode = Some(name);
                    continue;
                }
            }
        }
        layers.push(*color);
    }
    (!layers.is_empty()).then_some((mode, layers))
}

fn layer_color(tokens: &[Token]) -> Option<Color> {
    let (mode, layers) = layer_parts(tokens)?;
    let mode = mode.and_then(|s| rabbithole_gfx::canvas_ext::blend_mode_index(&s)).unwrap_or(0);
    let mut backdrop = Color::TRANSPARENT;
    for layer in layers.iter().rev() {
        let source = crate::values::resolve_current(crate::values::parse_color_component(layer)?)?;
        let b = f64::from(backdrop.a) / 255.0;
        let s = f64::from(source.a) / 255.0;
        let a = s + b * (1.0 - s);
        if a == 0.0 { continue; }
        let blend = Color::from_u32(rabbithole_gfx::canvas_ext::blend_pixel(mode, backdrop.to_u32(), source.to_u32()));
        let channel = |cs: u8, cb: u8, mixed: u8| {
            ((s * (1.0 - b) * f64::from(cs) + s * b * f64::from(mixed) +
                (1.0 - s) * b * f64::from(cb)) / a).round() as u8
        };
        backdrop = Color::rgba(channel(source.r, backdrop.r, blend.r), channel(source.g, backdrop.g, blend.g),
            channel(source.b, backdrop.b, blend.b), (a * 255.0).round() as u8);
    }
    Some(backdrop)
}

pub fn alpha_parts(tokens: &[Token]) -> Option<(&[Token], &[Token])> {
    let (c, a) = args(tokens)?;
    let [from, origin] = c.as_slice() else { return None; };
    crate::values::is_ident(from, "from").then_some((*origin, a?))
}

/// A color of one of the newer notations, or `None` when `c` is not one
/// (or is invalid).
pub fn parse_function(f: &str, c: &[Token]) -> Option<ColorValue> {
    if f == "color-layers" { return layer_color(c).map(ColorValue::Color); }
    let (_, abs) = crate::color_model::resolve_tokens(c)?;
    Some(ColorValue::Color(abs.to_gfx()))
}

/// The unclipped color of a component in one of the CSS Color 4/5
/// notations (`lab()`, `oklch()`, `color()`, `color-mix()`, ...); `None`
/// for the legacy sRGB notations, `currentcolor` and invalid values.
pub fn parse_wide(c: &[Token]) -> Option<Wide> {
    let [Token::Function(f), ..] = c else { return None };
    let f = f.to_ascii_lowercase();
    if matches!(f.as_str(), "rgb" | "rgba" | "hsl" | "hsla" | "hwb" | "light-dark" | "color-layers") {
        return None;
    }
    let (_, abs) = crate::color_model::resolve_tokens(c)?;
    if abs.legacy {
        return None;
    }
    let lin = abs.linear();
    Some([lin[0] as f32, lin[1] as f32, lin[2] as f32, abs.alpha_value() as f32])
}

/// A computed color kept at full precision for CSSOM, when an 8-bit sRGB
/// color would lose its coordinate system, precision or missing
/// components.
#[derive(Debug, Clone, PartialEq)]
pub struct ComputedColor(pub crate::color_model::Abs);

impl ComputedColor {
    pub fn to_css(&self) -> String {
        self.0.to_css()
    }

    pub fn parse(c: &[Token]) -> Option<std::rc::Rc<Self>> {
        let [Token::Function(_), ..] = c else { return None };
        let (spec, abs) = crate::color_model::resolve_tokens(c)?;
        if spec.is_simple_legacy() {
            return None;
        }
        Some(std::rc::Rc::new(Self(abs)))
    }
}

/// Colors outside the sRGB gamut are clipped per channel, as engines do,
/// except at the ends of the lightness range, where gamut mapping (CSS
/// Color 4 §13.2) gives white and black whatever the chroma. Lightness
/// within rounding of an end counts as that end.
pub(crate) fn lightness_ends(lin: Rgb, l: f64, max: f64) -> Rgb {
    let out = lin.iter().any(|v| !(-1e-4..=1.0001).contains(v));
    if out && l >= max * (1.0 - 1e-5) {
        [1.0; 3]
    } else if out && l <= max * 1e-5 {
        [0.0; 3]
    } else {
        lin
    }
}

/// A color in relative color syntax (CSS Color 5 §4); `None` when invalid
/// or when its origin needs an unknown `currentcolor`.
pub fn relative(_f: &str, c: &[Token]) -> Option<Color> {
    crate::color_model::resolve_tokens(c).map(|(_, abs)| abs.to_gfx())
}

/// Split function arguments at top-level commas.
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

/// `color-mix(in <space> [<hue-method> hue]?, <color> [<p>]?, <color> [<p>]?)`.
/// A `<color-interpolation-method>` (CSS Color 4 §12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InterpSpace {
    #[default]
    Srgb,
    SrgbLinear,
    DisplayP3,
    DisplayP3Linear,
    A98Rgb,
    ProPhotoRgb,
    Rec2020,
    XyzD50,
    Oklab,
    Lab,
    Xyz,
    Hsl,
    Hwb,
    Lch,
    Oklch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HueMethod {
    #[default]
    Shorter,
    Longer,
    Increasing,
    Decreasing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ColorInterp {
    pub space: InterpSpace,
    pub hue: HueMethod,
}

impl InterpSpace {
    pub fn css_name(self) -> &'static str {
        match self {
            Self::Srgb => "srgb", Self::SrgbLinear => "srgb-linear",
            Self::DisplayP3 => "display-p3", Self::DisplayP3Linear => "display-p3-linear",
            Self::A98Rgb => "a98-rgb", Self::ProPhotoRgb => "prophoto-rgb", Self::Rec2020 => "rec2020",
            Self::Xyz => "xyz-d65", Self::XyzD50 => "xyz-d50", Self::Lab => "lab", Self::Oklab => "oklab",
            Self::Lch => "lch", Self::Oklch => "oklch", Self::Hsl => "hsl", Self::Hwb => "hwb",
        }
    }
}

impl ColorInterp {
    /// Parse `in <space> [<hue-method> hue]` from identifier words.
    pub fn parse(words: &[String]) -> Option<ColorInterp> {
        if words.first().map(String::as_str) != Some("in") || words.len() < 2 {
            return None;
        }
        let space = match words[1].as_str() {
            "srgb" => InterpSpace::Srgb,
            "display-p3" => InterpSpace::DisplayP3,
            "display-p3-linear" => InterpSpace::DisplayP3Linear,
            "a98-rgb" => InterpSpace::A98Rgb,
            "prophoto-rgb" => InterpSpace::ProPhotoRgb,
            "rec2020" => InterpSpace::Rec2020,
            "srgb-linear" => InterpSpace::SrgbLinear,
            "oklab" => InterpSpace::Oklab,
            "lab" => InterpSpace::Lab,
            "xyz" | "xyz-d65" => InterpSpace::Xyz,
            "xyz-d50" => InterpSpace::XyzD50,
            "hsl" => InterpSpace::Hsl,
            "hwb" => InterpSpace::Hwb,
            "lch" => InterpSpace::Lch,
            "oklch" => InterpSpace::Oklch,
            _ => return None,
        };
        let polar = matches!(space, InterpSpace::Hsl | InterpSpace::Hwb | InterpSpace::Lch | InterpSpace::Oklch);
        let hue = match words.len() {
            2 => HueMethod::Shorter,
            4 if polar && words[3] == "hue" => match words[2].as_str() {
                "shorter" => HueMethod::Shorter,
                "longer" => HueMethod::Longer,
                "increasing" => HueMethod::Increasing,
                "decreasing" => HueMethod::Decreasing,
                _ => return None,
            },
            _ => return None,
        };
        Some(ColorInterp { space, hue })
    }

    pub fn is_srgb(&self) -> bool {
        self.space == InterpSpace::Srgb
    }
}

fn rgb_to_hsl(c: Rgb) -> (Option<f64>, f64, f64) {
    let (r, g, b) = (c[0], c[1], c[2]);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d.abs() < 1e-9 {
        return (None, 0.0, l);
    }
    let s = if l <= 0.0 || l >= 1.0 { 0.0 } else { (max - l) / l.min(1.0 - l) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } * 60.0;
    (if s.abs() < 1e-9 { None } else { Some(h) }, s, l)
}

fn hsl_to_rgb(h: f64, s: f64, l: f64) -> Rgb {
    let f = |n: f64| {
        let k = (n + h / 30.0).rem_euclid(12.0);
        let a = s * l.min(1.0 - l);
        l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
    };
    [f(0.0), f(8.0), f(4.0)]
}

/// Color components in an interpolation space: (hue, the others).
/// A missing hue (achromatic colors) is None.
fn to_interp(linear: Rgb, space: InterpSpace) -> (Option<f64>, Rgb) {
    let srgb = linear.map(gamma_encode);
    let lin = || linear;
    match space {
        InterpSpace::DisplayP3 | InterpSpace::DisplayP3Linear | InterpSpace::A98Rgb | InterpSpace::ProPhotoRgb | InterpSpace::Rec2020 | InterpSpace::XyzD50 =>
            (None, predefined_coords(space.css_name(),linear).expect("predefined interpolation profile")),
        InterpSpace::Srgb => (None, srgb),
        InterpSpace::SrgbLinear => (None, lin()),
        InterpSpace::Oklab => (None, linear_to_oklab(lin())),
        InterpSpace::Lab => (None, linear_to_lab(lin())),
        InterpSpace::Xyz => (None, mul(&LSRGB_TO_XYZ65, lin())),
        InterpSpace::Hsl => {
            let (h, s, l) = rgb_to_hsl(srgb);
            (h, [0.0, s, l])
        }
        InterpSpace::Hwb => {
            let (h, _, _) = rgb_to_hsl(srgb);
            let w = srgb[0].min(srgb[1]).min(srgb[2]);
            let b = 1.0 - srgb[0].max(srgb[1]).max(srgb[2]);
            (if w + b >= 1.0 - 1e-9 { None } else { h }, [0.0, w, b])
        }
        InterpSpace::Lch | InterpSpace::Oklch => {
            let lab = if space == InterpSpace::Lch { linear_to_lab(lin()) } else { linear_to_oklab(lin()) };
            let chroma = (lab[1] * lab[1] + lab[2] * lab[2]).sqrt();
            let eps = if space == InterpSpace::Lch { 1e-2 } else { 1e-4 };
            let h = lab[2].atan2(lab[1]).to_degrees().rem_euclid(360.0);
            (if chroma < eps { None } else { Some(h) }, [lab[0], chroma, 0.0])
        }
    }
}

fn from_interp(h: f64, v: Rgb, space: InterpSpace) -> Rgb {
    let lin = match space {
        InterpSpace::DisplayP3 | InterpSpace::DisplayP3Linear | InterpSpace::A98Rgb | InterpSpace::ProPhotoRgb | InterpSpace::Rec2020 | InterpSpace::XyzD50 =>
            predefined_linear(space.css_name(),v).expect("predefined interpolation profile"),
        InterpSpace::Srgb => [gamma_decode(v[0]), gamma_decode(v[1]), gamma_decode(v[2])],
        InterpSpace::SrgbLinear => v,
        InterpSpace::Oklab => oklab_to_linear(v[0], v[1], v[2]),
        InterpSpace::Lab => lab_to_linear(v[0], v[1], v[2]),
        InterpSpace::Xyz => mul(&XYZ65_TO_LSRGB, v),
        InterpSpace::Hsl => {
            let rgb = hsl_to_rgb(h, v[1], v[2]);
            [gamma_decode(rgb[0]), gamma_decode(rgb[1]), gamma_decode(rgb[2])]
        }
        InterpSpace::Hwb => {
            let (w, b) = (v[1], v[2]);
            let rgb = if w + b >= 1.0 {
                let g = w / (w + b);
                [g, g, g]
            } else {
                hsl_to_rgb(h, 1.0, 0.5).map(|c| c * (1.0 - w - b) + w)
            };
            [gamma_decode(rgb[0]), gamma_decode(rgb[1]), gamma_decode(rgb[2])]
        }
        InterpSpace::Lch | InterpSpace::Oklch => {
            let (x, y) = polar(v[1], h);
            if space == InterpSpace::Lch { lab_to_linear(v[0], x, y) } else { oklab_to_linear(v[0], x, y) }
        }
    };
    lin
}

/// Interpolate `a`→`b` at `t` in the given space (premultiplied alpha,
/// hue arcs per the hue method; CSS Color 4 §12.4).
pub fn interpolate(a: Color, b: Color, t: f32, m: ColorInterp) -> Color {
    let (lin, alpha) = interpolate_linear((linear_of(a), a.a as f64 / 255.0), (linear_of(b), b.a as f64 / 255.0), t as f64, m);
    to_color(lin, alpha)
}

/// [`interpolate`] between full-precision colors; the result is not
/// clipped to the sRGB gamut.
pub fn interpolate_wide(a: Wide, b: Wide, t: f32, m: ColorInterp) -> Wide {
    let w = |c: Wide| ([c[0] as f64, c[1] as f64, c[2] as f64], c[3] as f64);
    let (lin, alpha) = interpolate_linear(w(a), w(b), t as f64, m);
    [lin[0] as f32, lin[1] as f32, lin[2] as f32, alpha as f32]
}

/// The 8-bit sRGB color of a full-precision one (clipped per channel).
pub fn wide_to_color(c: Wide) -> Color {
    to_color([c[0] as f64, c[1] as f64, c[2] as f64], c[3] as f64)
}

/// The full-precision form of an 8-bit sRGB color.
pub fn color_to_wide(c: Color) -> Wide {
    let l = linear_of(c);
    [l[0] as f32, l[1] as f32, l[2] as f32, c.a as f32 / 255.0]
}

fn interpolate_linear(a: (Rgb, f64), b: (Rgb, f64), t: f64, m: ColorInterp) -> (Rgb, f64) {
    let (aa, ab) = (a.1, b.1);
    let (ha, va) = to_interp(a.0, m.space);
    let (hb, vb) = to_interp(b.0, m.space);
    let alpha = aa + (ab - aa) * t;
    let polar = matches!(m.space, InterpSpace::Hsl | InterpSpace::Hwb | InterpSpace::Lch | InterpSpace::Oklch);
    // Transparent colors are "missing" in alpha-weighted terms only.
    let mix = |x: f64, y: f64| -> f64 {
        if alpha <= 0.0 {
            x + (y - x) * t
        } else {
            (x * aa + (y * ab - x * aa) * t) / alpha
        }
    };
    let mut v = [0.0; 3];
    let first = if polar { 1 } else { 0 };
    for i in 0..3 {
        if polar && i == 0 && matches!(m.space, InterpSpace::Lch | InterpSpace::Oklch) {
            v[0] = mix(va[0], vb[0]);
            continue;
        }
        if i < first {
            continue;
        }
        v[i] = mix(va[i], vb[i]);
    }
    if matches!(m.space, InterpSpace::Lch | InterpSpace::Oklch) {
        // Components are (L, C, unused).
        v[2] = 0.0;
    }
    let h = if polar {
        match (ha, hb) {
            (None, None) => 0.0,
            (Some(h), None) | (None, Some(h)) => h,
            (Some(h1), Some(h2)) => {
                let (h1, mut h2) = (h1.rem_euclid(360.0), h2.rem_euclid(360.0));
                let d = h2 - h1;
                match m.hue {
                    HueMethod::Shorter => {
                        if d > 180.0 {
                            h2 -= 360.0;
                        } else if d < -180.0 {
                            h2 += 360.0;
                        }
                    }
                    HueMethod::Longer => {
                        if d > 0.0 && d < 180.0 {
                            h2 -= 360.0;
                        } else if d > -180.0 && d <= 0.0 {
                            h2 += 360.0;
                        }
                    }
                    HueMethod::Increasing => {
                        if d < 0.0 {
                            h2 += 360.0;
                        }
                    }
                    HueMethod::Decreasing => {
                        if d > 0.0 {
                            h2 -= 360.0;
                        }
                    }
                }
                (h1 + (h2 - h1) * t).rem_euclid(360.0)
            }
        }
    } else {
        0.0
    };
    (from_interp(h, v, m.space), alpha.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use crate::values::{parse_color, ColorValue};
    use rabbithole_css::Tokenizer;
    use rabbithole_gfx::Color;

    #[test]
    fn color_mixes_keep_their_profile_and_unclipped_channels() {
        for space in ["display-p3","display-p3-linear","a98-rgb","rec2020","prophoto-rgb","xyz-d50"] {
            let input = format!("color-mix(in {space}, color({space} 0.2 0.3 1.2), color({space} 0.6 0.7 1.4))");
            let css = super::ComputedColor::parse(&Tokenizer::tokenize(&input)).unwrap();
            assert_eq!(css.0.space.name(),space);
            for (actual,expected) in css.0.c.into_iter().zip([0.4,0.5,1.3]) {
                assert!((actual - expected).abs() < 0.0001,"{input}: {css:?}");
            }
        }
        let polar = super::ComputedColor::parse(&Tokenizer::tokenize("color-mix(in hsl, red, lime)")).unwrap();
        assert_eq!(polar.to_css(),"color(srgb 1 1 0)");
        for bad in ["color-mix(in srgb 3, red, blue)", "color-mix(in srgb, red red, blue)",
                    "color-mix(in srgb, red 20% 30%, blue)", "color-mix(in srgb, red 120%, blue)"] {
            assert!(color(bad).is_none(),"{bad}");
        }
    }

    fn color(s: &str) -> Option<Color> {
        match parse_color(&Tokenizer::tokenize(s))? {
            ColorValue::Color(c) => Some(c),
            ColorValue::CurrentColor => None,
        }
    }

    fn close(a: Color, b: Color) -> bool {
        [(a.r, b.r), (a.g, b.g), (a.b, b.b), (a.a, b.a)].iter().all(|(x, y)| x.abs_diff(*y) <= 2)
    }

    #[test]
    fn modern_notations() {
        let red = Color::rgb(255, 0, 0);
        assert!(close(color("oklch(62.8% 0.2577 29.23)").unwrap(), red));
        assert!(close(color("oklab(0.628 0.2249 0.1258)").unwrap(), red));
        assert!(close(color("lab(54.29 80.8 69.89)").unwrap(), red));
        assert!(close(color("lch(54.29 106.84 40.85)").unwrap(), red));
        assert!(close(color("hwb(0 0% 0%)").unwrap(), red));
        assert!(close(color("hwb(0 50% 50%)").unwrap(), Color::rgb(128, 128, 128)));
        assert!(close(color("color(srgb 1 0 0)").unwrap(), red));
        assert!(close(color("color(display-p3 1 0 0)").unwrap(), red), "p3 red clips to sRGB red");
        assert!(close(color("oklch(1 0 0 / 50%)").unwrap(), Color::rgba(255, 255, 255, 128)));
        assert!(close(color("color-mix(in srgb, red, blue)").unwrap(), Color::rgb(128, 0, 128)));
        assert!(close(color("color-mix(in srgb, red 25%, white)").unwrap(), Color::rgb(255, 191, 191)));
        assert!(close(color("color-mix(in oklab, white, black)").unwrap(), Color::rgb(99, 99, 99)));
        assert!(close(color("color-mix(in srgb, red, transparent)").unwrap(), Color::rgba(255, 0, 0, 128)));
        assert_eq!(color("light-dark(red, blue)"), Some(red));
        assert_eq!(color("Canvas"), Some(Color::rgb(255, 255, 255)));
        assert_eq!(color("ButtonFace"), Some(Color::rgb(239, 239, 239)));
        assert!(color("oklch(50% 0.1)").is_none());
        assert_eq!(color("contrast-color(#000)"), Some(Color::WHITE));
        assert_eq!(color("contrast-color(yellow)"), Some(Color::BLACK));
        // Relative color syntax.
        assert!(close(color("rgb(from red r g b)").unwrap(), red));
        assert!(close(color("rgb(from red b g r)").unwrap(), Color::rgb(0, 0, 255)));
        assert!(close(color("hsl(from red calc(h + 120) s l)").unwrap(), Color::rgb(0, 255, 0)));
        assert!(close(color("oklch(from red l c h / 0.5)").unwrap(), Color::rgba(255, 0, 0, 128)));
        assert!(close(color("lch(from blue l 0 h)").unwrap(), color("lch(29.57 0 0)").unwrap()));
        assert!(close(color("color(from red srgb r g b / alpha)").unwrap(), red));
        assert!(color("rgb(from currentcolor r g b)").is_none());
        crate::values::set_current_color(Some(Color::rgb(0, 128, 0)));
        assert!(close(color("lch(from currentcolor l c h)").unwrap(), Color::rgb(0, 128, 0)));
        assert!(close(color("color-mix(in srgb, currentcolor, white 0%)").unwrap(), Color::rgb(0, 128, 0)));
        crate::values::set_current_color(None);
    }

    #[test]
    fn wide_gamut_and_system_colors() {
        assert!(close(color("color(display-p3-linear 0.0383 0.2087 0.0156)").unwrap(), Color::rgb(0, 128, 0)));
        // Deprecated system colors are their replacements.
        assert_eq!(color("ActiveBorder"), color("ButtonBorder"));
        assert_eq!(color("ThreeDFace"), color("ButtonFace"));
        assert_eq!(color("InfoText"), color("CanvasText"));
        // The lightness ends are white and black whatever the chroma.
        assert_eq!(color("lch(100% 110 60)"), Some(Color::WHITE));
        assert_eq!(color("oklch(0% 110 60)"), Some(Color::BLACK));
        assert_eq!(color("oklab(99.9999% 0.15 0.15)"), color("oklab(1 0.15 0.15)"));
        // Out-of-gamut colors keep their unclipped value for interpolation.
        let toks = Tokenizer::tokenize("lch(50% 100% 0deg)");
        let w = super::parse_wide(&toks).unwrap();
        assert!(w[..3].iter().any(|v| *v < 0.0 || *v > 1.0));
        assert!(super::parse_wide(&Tokenizer::tokenize("rgb(1 2 3)")).is_none());
        // A mix in LCh of out-of-gamut colors stays out of gamut.
        let mix = super::parse_wide(&Tokenizer::tokenize("color-mix(in lch longer hue, red, blue)")).unwrap();
        let m = crate::ColorInterp { space: crate::InterpSpace::Lch, hue: crate::HueMethod::Longer };
        let red = super::parse_wide(&Tokenizer::tokenize("color(srgb 1 0 0)")).unwrap();
        let blue = super::parse_wide(&Tokenizer::tokenize("color(srgb 0 0 1)")).unwrap();
        let mid = super::interpolate_wide(red, blue, 0.5, m);
        assert!(mid.iter().zip(mix.iter()).all(|(a, b)| (a - b).abs() < 1e-4), "{mid:?} {mix:?}");
    }
}
