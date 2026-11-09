//! SVG/CSS color parsing (named colors, hex, `rgb()`, `rgba()`, `hsl()`).

use rabbithole_gfx::Color;

/// A parsed `<color>` or one of the SVG paint keywords.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColorSpec {
    Color(Color),
    CurrentColor,
}

/// Parse a color string. Returns None for `none`, `url(...)` or garbage.
pub fn parse_color(s: &str) -> Option<ColorSpec> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    if lower == "currentcolor" {
        return Some(ColorSpec::CurrentColor);
    }
    if lower == "transparent" {
        return Some(ColorSpec::Color(Color::TRANSPARENT));
    }
    if let Some(hex) = lower.strip_prefix('#') {
        return parse_hex(hex).map(ColorSpec::Color);
    }
    if let Some(inner) = lower.strip_prefix("rgba(").or_else(|| lower.strip_prefix("rgb(")) {
        let inner = inner.strip_suffix(')')?;
        return parse_rgb(inner).map(ColorSpec::Color);
    }
    if let Some(inner) = lower.strip_prefix("hsla(").or_else(|| lower.strip_prefix("hsl(")) {
        let inner = inner.strip_suffix(')')?;
        return parse_hsl(inner).map(ColorSpec::Color);
    }
    named_color(&lower).map(ColorSpec::Color)
}

fn parse_hex(hex: &str) -> Option<Color> {
    let v = u32::from_str_radix(hex, 16).ok()?;
    match hex.len() {
        3 => {
            let r = ((v >> 8) & 0xf) as u8;
            let g = ((v >> 4) & 0xf) as u8;
            let b = (v & 0xf) as u8;
            Some(Color::rgb(r * 17, g * 17, b * 17))
        }
        4 => {
            let r = ((v >> 12) & 0xf) as u8;
            let g = ((v >> 8) & 0xf) as u8;
            let b = ((v >> 4) & 0xf) as u8;
            let a = (v & 0xf) as u8;
            Some(Color::rgba(r * 17, g * 17, b * 17, a * 17))
        }
        6 => Some(Color::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)),
        8 => Some(Color::rgba((v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8)),
        _ => None,
    }
}

/// Split `a, b, c` or `a b c / d` style arguments.
fn split_args(inner: &str) -> Vec<String> {
    inner
        .replace('/', " ")
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

fn channel(s: &str) -> Option<u8> {
    if let Some(p) = s.strip_suffix('%') {
        let v: f32 = p.parse().ok()?;
        return Some((v.clamp(0.0, 100.0) * 2.55).round() as u8);
    }
    let v: f32 = s.parse().ok()?;
    Some(v.clamp(0.0, 255.0).round() as u8)
}

fn alpha(s: &str) -> Option<u8> {
    if let Some(p) = s.strip_suffix('%') {
        let v: f32 = p.parse().ok()?;
        return Some((v.clamp(0.0, 100.0) * 2.55).round() as u8);
    }
    let v: f32 = s.parse().ok()?;
    Some((v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn parse_rgb(inner: &str) -> Option<Color> {
    let a = split_args(inner);
    if a.len() < 3 {
        return None;
    }
    let r = channel(&a[0])?;
    let g = channel(&a[1])?;
    let b = channel(&a[2])?;
    let al = if a.len() > 3 { alpha(&a[3])? } else { 255 };
    Some(Color::rgba(r, g, b, al))
}

fn parse_hsl(inner: &str) -> Option<Color> {
    let a = split_args(inner);
    if a.len() < 3 {
        return None;
    }
    let h: f32 = a[0].trim_end_matches("deg").parse().ok()?;
    let s: f32 = a[1].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
    let l: f32 = a[2].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
    let al = if a.len() > 3 { alpha(&a[3])? } else { 255 };
    let h = ((h % 360.0) + 360.0) % 360.0 / 360.0;
    let (r, g, b) = hsl_to_rgb(h, s.clamp(0.0, 1.0), l.clamp(0.0, 1.0));
    Some(Color::rgba((r * 255.0).round() as u8, (g * 255.0).round() as u8, (b * 255.0).round() as u8, al))
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    if s == 0.0 {
        return (l, l, l);
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let hue = |mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    (hue(h + 1.0 / 3.0), hue(h), hue(h - 1.0 / 3.0))
}

/// CSS named color lookup (lower-case name).
pub fn named_color(name: &str) -> Option<Color> {
    let idx = NAMED.binary_search_by(|(n, _)| n.cmp(&name)).ok()?;
    let v = NAMED[idx].1;
    Some(Color::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

// Sorted by name.
static NAMED: &[(&str, u32)] = &[
    ("aliceblue", 0xf0f8ff), ("antiquewhite", 0xfaebd7), ("aqua", 0x00ffff), ("aquamarine", 0x7fffd4),
    ("azure", 0xf0ffff), ("beige", 0xf5f5dc), ("bisque", 0xffe4c4), ("black", 0x000000),
    ("blanchedalmond", 0xffebcd), ("blue", 0x0000ff), ("blueviolet", 0x8a2be2), ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887), ("cadetblue", 0x5f9ea0), ("chartreuse", 0x7fff00), ("chocolate", 0xd2691e),
    ("coral", 0xff7f50), ("cornflowerblue", 0x6495ed), ("cornsilk", 0xfff8dc), ("crimson", 0xdc143c),
    ("cyan", 0x00ffff), ("darkblue", 0x00008b), ("darkcyan", 0x008b8b), ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9), ("darkgreen", 0x006400), ("darkgrey", 0xa9a9a9), ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b), ("darkolivegreen", 0x556b2f), ("darkorange", 0xff8c00), ("darkorchid", 0x9932cc),
    ("darkred", 0x8b0000), ("darksalmon", 0xe9967a), ("darkseagreen", 0x8fbc8f), ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f), ("darkslategrey", 0x2f4f4f), ("darkturquoise", 0x00ced1), ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493), ("deepskyblue", 0x00bfff), ("dimgray", 0x696969), ("dimgrey", 0x696969),
    ("dodgerblue", 0x1e90ff), ("firebrick", 0xb22222), ("floralwhite", 0xfffaf0), ("forestgreen", 0x228b22),
    ("fuchsia", 0xff00ff), ("gainsboro", 0xdcdcdc), ("ghostwhite", 0xf8f8ff), ("gold", 0xffd700),
    ("goldenrod", 0xdaa520), ("gray", 0x808080), ("green", 0x008000), ("greenyellow", 0xadff2f),
    ("grey", 0x808080), ("honeydew", 0xf0fff0), ("hotpink", 0xff69b4), ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082), ("ivory", 0xfffff0), ("khaki", 0xf0e68c), ("lavender", 0xe6e6fa),
    ("lavenderblush", 0xfff0f5), ("lawngreen", 0x7cfc00), ("lemonchiffon", 0xfffacd), ("lightblue", 0xadd8e6),
    ("lightcoral", 0xf08080), ("lightcyan", 0xe0ffff), ("lightgoldenrodyellow", 0xfafad2), ("lightgray", 0xd3d3d3),
    ("lightgreen", 0x90ee90), ("lightgrey", 0xd3d3d3), ("lightpink", 0xffb6c1), ("lightsalmon", 0xffa07a),
    ("lightseagreen", 0x20b2aa), ("lightskyblue", 0x87cefa), ("lightslategray", 0x778899), ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de), ("lightyellow", 0xffffe0), ("lime", 0x00ff00), ("limegreen", 0x32cd32),
    ("linen", 0xfaf0e6), ("magenta", 0xff00ff), ("maroon", 0x800000), ("mediumaquamarine", 0x66cdaa),
    ("mediumblue", 0x0000cd), ("mediumorchid", 0xba55d3), ("mediumpurple", 0x9370db), ("mediumseagreen", 0x3cb371),
    ("mediumslateblue", 0x7b68ee), ("mediumspringgreen", 0x00fa9a), ("mediumturquoise", 0x48d1cc), ("mediumvioletred", 0xc71585),
    ("midnightblue", 0x191970), ("mintcream", 0xf5fffa), ("mistyrose", 0xffe4e1), ("moccasin", 0xffe4b5),
    ("navajowhite", 0xffdead), ("navy", 0x000080), ("oldlace", 0xfdf5e6), ("olive", 0x808000),
    ("olivedrab", 0x6b8e23), ("orange", 0xffa500), ("orangered", 0xff4500), ("orchid", 0xda70d6),
    ("palegoldenrod", 0xeee8aa), ("palegreen", 0x98fb98), ("paleturquoise", 0xafeeee), ("palevioletred", 0xdb7093),
    ("papayawhip", 0xffefd5), ("peachpuff", 0xffdab9), ("peru", 0xcd853f), ("pink", 0xffc0cb),
    ("plum", 0xdda0dd), ("powderblue", 0xb0e0e6), ("purple", 0x800080), ("rebeccapurple", 0x663399),
    ("red", 0xff0000), ("rosybrown", 0xbc8f8f), ("royalblue", 0x4169e1), ("saddlebrown", 0x8b4513),
    ("salmon", 0xfa8072), ("sandybrown", 0xf4a460), ("seagreen", 0x2e8b57), ("seashell", 0xfff5ee),
    ("sienna", 0xa0522d), ("silver", 0xc0c0c0), ("skyblue", 0x87ceeb), ("slateblue", 0x6a5acd),
    ("slategray", 0x708090), ("slategrey", 0x708090), ("snow", 0xfffafa), ("springgreen", 0x00ff7f),
    ("steelblue", 0x4682b4), ("tan", 0xd2b48c), ("teal", 0x008080), ("thistle", 0xd8bfd8),
    ("tomato", 0xff6347), ("turquoise", 0x40e0d0), ("violet", 0xee82ee), ("wheat", 0xf5deb3),
    ("white", 0xffffff), ("whitesmoke", 0xf5f5f5), ("yellow", 0xffff00), ("yellowgreen", 0x9acd32),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_colors() {
        assert_eq!(parse_color("red"), Some(ColorSpec::Color(Color::rgb(255, 0, 0))));
        assert_eq!(parse_color("#ABC"), Some(ColorSpec::Color(Color::rgb(0xaa, 0xbb, 0xcc))));
        assert_eq!(parse_color("#11223380"), Some(ColorSpec::Color(Color::rgba(0x11, 0x22, 0x33, 0x80))));
        assert_eq!(parse_color("rgb(1, 2, 3)"), Some(ColorSpec::Color(Color::rgb(1, 2, 3))));
        assert_eq!(parse_color("rgba(1 2 3 / 50%)"), Some(ColorSpec::Color(Color::rgba(1, 2, 3, 128))));
        assert_eq!(parse_color("hsl(120, 100%, 50%)"), Some(ColorSpec::Color(Color::rgb(0, 255, 0))));
        assert_eq!(parse_color("currentColor"), Some(ColorSpec::CurrentColor));
        assert_eq!(parse_color("none"), None);
        assert_eq!(parse_color("url(#g)"), None);
        for w in NAMED.windows(2) {
            assert!(w[0].0 < w[1].0);
        }
    }
}
