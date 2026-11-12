//! CSS Color 4/5 beyond sRGB notations: `hwb()`, `lab()`, `lch()`,
//! `oklab()`, `oklch()`, `color()` (sRGB, linear sRGB, Display P3, A98,
//! ProPhoto, Rec.2020, XYZ), `color-mix()`, `light-dark()` and the system
//! colors. Everything is converted (and gamut-clipped) to 8-bit sRGB.

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

/// `channels / alpha` (space-separated; commas tolerated).
fn args(c: &[Token]) -> Option<(Vec<&[Token]>, Option<&[Token]>)> {
    let mut vals = Vec::new();
    let mut alpha = None;
    let mut slash = false;
    for comp in components(strip(c)) {
        match comp {
            [Token::Comma] => continue,
            [Token::Delim('/')] => {
                slash = true;
                continue;
            }
            _ => {}
        }
        if slash {
            alpha = Some(comp);
        } else {
            vals.push(comp);
        }
    }
    Some((vals, alpha))
}

/// A number, a percentage (scaled so 100% = `pct_ref`) or `none` (0).
fn num(c: &[Token], pct_ref: f64) -> Option<f64> {
    match c {
        [Token::Number(n)] => Some(*n as f64),
        [Token::Percentage(p)] => Some(*p as f64 / 100.0 * pct_ref),
        [Token::Ident(s)] if s.eq_ignore_ascii_case("none") => Some(0.0),
        _ => None,
    }
}

/// A hue in degrees.
fn hue(c: &[Token]) -> Option<f64> {
    match c {
        [Token::Number(n)] => Some(*n as f64),
        [Token::Dimension(n, u)] => {
            let n = *n as f64;
            match u.to_ascii_lowercase().as_str() {
                "deg" => Some(n),
                "rad" => Some(n.to_degrees()),
                "grad" => Some(n * 0.9),
                "turn" => Some(n * 360.0),
                _ => None,
            }
        }
        [Token::Ident(s)] if s.eq_ignore_ascii_case("none") => Some(0.0),
        _ => None,
    }
}

fn alpha(c: Option<&[Token]>) -> Option<f64> {
    match c {
        None => Some(1.0),
        Some(c) => num(c, 1.0).map(|a| a.clamp(0.0, 1.0)),
    }
}

fn gamma_encode(x: f64) -> f64 {
    let s = x.signum();
    let x = x.abs();
    s * if x <= 0.0031308 { 12.92 * x } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 }
}

fn gamma_decode(x: f64) -> f64 {
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

fn to_color(lin: Rgb, a: f64) -> Color {
    let c = |v: f64| (gamma_encode(v).clamp(0.0, 1.0) * 255.0).round() as u8;
    Color::rgba(c(lin[0]), c(lin[1]), c(lin[2]), (a * 255.0).round() as u8)
}

fn linear_of(c: Color) -> Rgb {
    [gamma_decode(c.r as f64 / 255.0), gamma_decode(c.g as f64 / 255.0), gamma_decode(c.b as f64 / 255.0)]
}

fn oklab_to_linear(l: f64, a: f64, b: f64) -> Rgb {
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

fn linear_to_oklab(c: Rgb) -> Rgb {
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

fn lab_to_linear(l: f64, a: f64, b: f64) -> Rgb {
    let fy = (l + 16.0) / 116.0;
    let fx = fy + a / 500.0;
    let fz = fy - b / 200.0;
    let inv = |t: f64| if t * t * t > 216.0 / 24389.0 { t * t * t } else { (116.0 * t - 16.0) / (24389.0 / 27.0) };
    let y = if l > 8.0 { fy * fy * fy } else { l / (24389.0 / 27.0) };
    let xyz50 = [inv(fx) * 0.3457 / 0.3585, y, inv(fz) * (1.0 - 0.3457 - 0.3585) / 0.3585];
    mul(&XYZ65_TO_LSRGB, mul(&D50_TO_D65, xyz50))
}

fn linear_to_lab(c: Rgb) -> Rgb {
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

/// `color(<space> r g b)`: linear-light sRGB of the color.
fn predefined(space: &str, v: Rgb) -> Option<Rgb> {
    let rgb_space = |m: &[f64; 9], decode: &dyn Fn(f64) -> f64| mul(&XYZ65_TO_LSRGB, mul(m, [decode(v[0]), decode(v[1]), decode(v[2])]));
    Some(match space {
        "srgb" => [gamma_decode(v[0]), gamma_decode(v[1]), gamma_decode(v[2])],
        "srgb-linear" => v,
        "display-p3" => rgb_space(
            &[0.4865709486482162, 0.26566769316909306, 0.1982172852343625, 0.2289745640697488, 0.6917385218365064, 0.079286914093745, 0.0, 0.04511338185890264, 1.043944368900976],
            &gamma_decode,
        ),
        "a98-rgb" => rgb_space(
            &[0.5766690429101305, 0.1855582379065463, 0.1882286462349947, 0.29734497525053605, 0.6273635662554661, 0.0752914584939978, 0.02703136138641234, 0.07068885253582723, 0.9913375368376388],
            &|x: f64| x.signum() * x.abs().powf(563.0 / 256.0),
        ),
        "rec2020" => rgb_space(
            &[0.6369580483012914, 0.14461690358620832, 0.1688809751641721, 0.2627002120112671, 0.6779980715188708, 0.05930171646986196, 0.0, 0.028072693049087428, 1.060985057710791],
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
                &[0.7977604896723027, 0.13518583717574031, 0.0313493495815248, 0.2880711282292934, 0.7118432178101014, 0.00008565396060525902, 0.0, 0.0, 0.8251046025104601],
                [d(v[0]), d(v[1]), d(v[2])],
            );
            mul(&XYZ65_TO_LSRGB, mul(&D50_TO_D65, xyz50))
        }
        "xyz" | "xyz-d65" => mul(&XYZ65_TO_LSRGB, v),
        "xyz-d50" => mul(&XYZ65_TO_LSRGB, mul(&D50_TO_D65, v)),
        _ => return None,
    })
}

/// System colors (CSS Color 4 §6.2) as rendered by Blink in light mode.
pub fn system_color(name: &str) -> Option<Color> {
    Some(match name {
        "canvas" | "field" | "window" | "background" | "buttonhighlight" | "threedhighlight" | "infobackground" | "menu" => Color::rgb(255, 255, 255),
        "canvastext" | "fieldtext" | "buttontext" | "windowtext" | "highlighttext" | "selecteditemtext" | "marktext" | "menutext" | "infotext" | "captiontext" => {
            Color::rgb(0, 0, 0)
        }
        "linktext" => Color::rgb(0, 0, 238),
        "visitedtext" => Color::rgb(85, 26, 139),
        "activetext" => Color::rgb(255, 0, 0),
        "buttonface" | "threedface" | "activeborder" | "inactiveborder" | "scrollbar" | "appworkspace" | "inactivecaption" => Color::rgb(239, 239, 239),
        "buttonborder" | "buttonshadow" | "threedshadow" | "threeddarkshadow" | "windowframe" | "threedlightshadow" => Color::rgb(118, 118, 118),
        "graytext" | "inactivecaptiontext" => Color::rgb(109, 109, 109),
        "highlight" | "selecteditem" => Color::rgb(181, 213, 255),
        "mark" => Color::rgb(255, 255, 0),
        "accentcolor" | "activecaption" => Color::rgb(0, 117, 255),
        "accentcolortext" => Color::rgb(255, 255, 255),
        _ => return None,
    })
}

/// A color of one of the newer notations, or `None` when `c` is not one
/// (or is invalid).
pub fn parse_function(f: &str, c: &[Token]) -> Option<ColorValue> {
    let (vals, a) = args(c)?;
    let color = match f {
        "hwb" => {
            if vals.len() != 3 {
                return None;
            }
            let h = hue(vals[0])?;
            let w = num(vals[1], 1.0).map(|v| if matches!(vals[1], [Token::Number(_)]) { v / 100.0 } else { v })?;
            let bl = num(vals[2], 1.0).map(|v| if matches!(vals[2], [Token::Number(_)]) { v / 100.0 } else { v })?;
            let (r, g, b) = if w + bl >= 1.0 {
                let gray = w / (w + bl);
                (gray, gray, gray)
            } else {
                let (r, g, b) = crate::values::hsl_to_rgb_pub(h as f32, 1.0, 0.5);
                let f = |x: f32| x as f64 * (1.0 - w - bl) + w;
                (f(r), f(g), f(b))
            };
            to_color([gamma_decode(r), gamma_decode(g), gamma_decode(b)], alpha(a)?)
        }
        "lab" | "oklab" => {
            if vals.len() != 3 {
                return None;
            }
            let ok = f == "oklab";
            let l = num(vals[0], if ok { 1.0 } else { 100.0 })?;
            let ab_ref = if ok { 0.4 } else { 125.0 };
            let (x, y) = (num(vals[1], ab_ref)?, num(vals[2], ab_ref)?);
            let l = l.clamp(0.0, if ok { 1.0 } else { 100.0 });
            let lin = if ok { oklab_to_linear(l, x, y) } else { lab_to_linear(l, x, y) };
            to_color(lin, alpha(a)?)
        }
        "lch" | "oklch" => {
            if vals.len() != 3 {
                return None;
            }
            let ok = f == "oklch";
            let l = num(vals[0], if ok { 1.0 } else { 100.0 })?;
            let ch = num(vals[1], if ok { 0.4 } else { 150.0 })?.max(0.0);
            let (x, y) = polar(ch, hue(vals[2])?);
            let l = l.clamp(0.0, if ok { 1.0 } else { 100.0 });
            let lin = if ok { oklab_to_linear(l, x, y) } else { lab_to_linear(l, x, y) };
            to_color(lin, alpha(a)?)
        }
        "color" => {
            let space = match vals.first()? {
                [Token::Ident(s)] => s.to_ascii_lowercase(),
                _ => return None,
            };
            if vals.len() != 4 {
                return None;
            }
            let xyz = space.starts_with("xyz");
            let r = if xyz { 1.0 } else { 1.0 };
            let v = [num(vals[1], r)?, num(vals[2], r)?, num(vals[3], r)?];
            to_color(predefined(&space, v)?, alpha(a)?)
        }
        "light-dark" => {
            // The light scheme is the one we render.
            let parts = split_top_commas(strip(c));
            if parts.len() != 2 {
                return None;
            }
            return crate::values::parse_color(parts[0]);
        }
        "color-mix" => return color_mix(c),
        _ => return None,
    };
    Some(ColorValue::Color(color))
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
fn color_mix(c: &[Token]) -> Option<ColorValue> {
    let parts = split_top_commas(strip(c));
    if parts.len() != 3 {
        return None;
    }
    let space: Vec<String> = components(parts[0]).iter().filter_map(|t| match t {
        [Token::Ident(s)] => Some(s.to_ascii_lowercase()),
        _ => None,
    }).collect();
    if space.first().map(String::as_str) != Some("in") || space.len() < 2 {
        return None;
    }
    let space = space[1].clone();
    let item = |toks: &[Token]| -> Option<(Color, Option<f64>)> {
        let comps: Vec<&[Token]> = components(toks).into_iter().filter(|c| !matches!(c, [Token::Whitespace])).collect();
        let mut color = None;
        let mut pct = None;
        for comp in comps {
            if let [Token::Percentage(p)] = comp {
                pct = Some(*p as f64 / 100.0);
            } else {
                match crate::values::parse_color_component(comp)? {
                    ColorValue::Color(c) => color = Some(c),
                    ColorValue::CurrentColor => return None,
                }
            }
        }
        Some((color?, pct))
    };
    let (c1, p1) = item(parts[1])?;
    let (c2, p2) = item(parts[2])?;
    let (mut p1, mut p2) = match (p1, p2) {
        (None, None) => (0.5, 0.5),
        (Some(a), None) => (a, 1.0 - a),
        (None, Some(b)) => (1.0 - b, b),
        (Some(a), Some(b)) => (a, b),
    };
    let sum = p1 + p2;
    if sum <= 0.0 {
        return None;
    }
    let alpha_mult = sum.min(1.0);
    p1 /= sum;
    p2 /= sum;
    let (a1, a2) = (c1.a as f64 / 255.0, c2.a as f64 / 255.0);
    let a = a1 * p1 + a2 * p2;
    // Interpolate premultiplied components in the mixing space.
    let to_space = |c: Color| -> Rgb {
        let lin = linear_of(c);
        match space.as_str() {
            "srgb-linear" => lin,
            "oklab" | "oklch" => linear_to_oklab(lin),
            "lab" | "lch" => linear_to_lab(lin),
            "xyz" | "xyz-d65" | "xyz-d50" => mul(&LSRGB_TO_XYZ65, lin),
            _ => [c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0],
        }
    };
    let (v1, v2) = (to_space(c1), to_space(c2));
    let mixed: Rgb = if a > 0.0 {
        [0, 1, 2].map(|i| (v1[i] * a1 * p1 + v2[i] * a2 * p2) / a)
    } else {
        [0, 1, 2].map(|i| v1[i] * p1 + v2[i] * p2)
    };
    let lin = match space.as_str() {
        "srgb-linear" => mixed,
        "oklab" | "oklch" => oklab_to_linear(mixed[0], mixed[1], mixed[2]),
        "lab" | "lch" => lab_to_linear(mixed[0], mixed[1], mixed[2]),
        "xyz" | "xyz-d65" | "xyz-d50" => mul(&XYZ65_TO_LSRGB, mixed),
        _ => [gamma_decode(mixed[0]), gamma_decode(mixed[1]), gamma_decode(mixed[2])],
    };
    Some(ColorValue::Color(to_color(lin, (a * alpha_mult).clamp(0.0, 1.0))))
}

#[cfg(test)]
mod tests {
    use crate::values::{parse_color, ColorValue};
    use rabbithole_css::Tokenizer;
    use rabbithole_gfx::Color;

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
    }
}
