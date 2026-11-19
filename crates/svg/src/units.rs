//! Numbers, lengths, transforms, viewBox / preserveAspectRatio and small
//! affine-matrix helpers.

use rabbithole_gfx::{Matrix, Point, Rect, IDENTITY};

/// `viewBox="x y w h"`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Parse a plain number (SVG allows `1e3`, `.5`, `-.5`).
pub fn parse_number(s: &str) -> Option<f32> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let v: f32 = s.parse().ok()?;
    if v.is_finite() {
        Some(v.clamp(-1.0e6, 1.0e6))
    } else {
        None
    }
}

/// Parse a list of numbers separated by whitespace and/or commas.
pub fn parse_number_list(s: &str) -> Vec<f32> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut Vec<f32>| {
        if !cur.is_empty() {
            if let Some(v) = parse_number(cur) {
                out.push(v);
            }
            cur.clear();
        }
    };
    let mut prev: Option<char> = None;
    for c in s.chars() {
        match c {
            ',' | ' ' | '\t' | '\n' | '\r' => flush(&mut cur, &mut out),
            '-' | '+' if !cur.is_empty() && !matches!(prev, Some('e') | Some('E')) => {
                flush(&mut cur, &mut out);
                cur.push(c);
            }
            '.' if cur.contains('.') && !cur.contains('e') && !cur.contains('E') => {
                flush(&mut cur, &mut out);
                cur.push(c);
            }
            _ => cur.push(c),
        }
        prev = Some(c);
    }
    flush(&mut cur, &mut out);
    out
}

/// A length with its unit resolved later against a reference size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Length {
    Px(f32),
    Percent(f32),
}

impl Length {
    pub fn resolve(&self, reference: f32) -> f32 {
        match *self {
            Length::Px(v) => v,
            Length::Percent(p) => reference * p / 100.0,
        }
    }
}

/// Parse `12`, `12px`, `50%`, `1em`, `2pt`, ... (`em` = 16px).
pub fn parse_length(s: &str) -> Option<Length> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let split = s
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+' || *c == 'e' || *c == 'E'))
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    // `e` may be part of `em`: back off when the unit starts with a letter that followed an 'e'.
    let (num, unit) = {
        let (n, u) = s.split_at(split);
        if n.ends_with(['e', 'E']) && (u.is_empty() || u.starts_with('m') || u.starts_with('x')) {
            (&n[..n.len() - 1], &s[split - 1..])
        } else {
            (n, u)
        }
    };
    let v = parse_number(num)?;
    let unit = unit.trim().to_ascii_lowercase();
    Some(match unit.as_str() {
        "" | "px" => Length::Px(v),
        "%" => Length::Percent(v),
        "em" | "rem" => Length::Px(v * 16.0),
        "ex" => Length::Px(v * 8.0),
        "pt" => Length::Px(v * 4.0 / 3.0),
        "pc" => Length::Px(v * 16.0),
        "mm" => Length::Px(v * 96.0 / 25.4),
        "cm" => Length::Px(v * 96.0 / 2.54),
        "in" => Length::Px(v * 96.0),
        _ => return None,
    })
}

/// Resolve a length attribute against a reference (percentages).
pub fn length_or(s: Option<&str>, reference: f32, default: f32) -> f32 {
    s.and_then(parse_length).map(|l| l.resolve(reference)).unwrap_or(default)
}

pub fn parse_viewbox(s: &str) -> Option<ViewBox> {
    let n = parse_number_list(s);
    if n.len() != 4 || n[2] <= 0.0 || n[3] <= 0.0 {
        return None;
    }
    Some(ViewBox { x: n[0], y: n[1], w: n[2], h: n[3] })
}

/// `preserveAspectRatio`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AspectRatio {
    /// None = `none` (non-uniform scaling).
    pub align: Option<(f32, f32)>,
    pub slice: bool,
}

impl Default for AspectRatio {
    fn default() -> Self {
        AspectRatio { align: Some((0.5, 0.5)), slice: false }
    }
}

pub fn parse_aspect_ratio(s: Option<&str>) -> AspectRatio {
    let Some(s) = s else { return AspectRatio::default() };
    let mut out = AspectRatio::default();
    for tok in s.split_whitespace() {
        let t = tok.to_ascii_lowercase();
        match t.as_str() {
            "none" => out.align = None,
            "meet" => out.slice = false,
            "slice" => out.slice = true,
            "defer" => {}
            _ => {
                if let Some(rest) = t.strip_prefix('x') {
                    let (xa, ya) = rest.split_at(rest.find('y').unwrap_or(rest.len()));
                    let fx = match xa {
                        "min" => 0.0,
                        "mid" => 0.5,
                        "max" => 1.0,
                        _ => continue,
                    };
                    let fy = match ya.trim_start_matches('y') {
                        "min" => 0.0,
                        "mid" => 0.5,
                        "max" => 1.0,
                        _ => continue,
                    };
                    out.align = Some((fx, fy));
                }
            }
        }
    }
    out
}

/// Transform mapping a viewBox onto a viewport rect.
pub fn viewbox_transform(vb: ViewBox, viewport: Rect, par: AspectRatio) -> Matrix {
    let sx = viewport.w / vb.w;
    let sy = viewport.h / vb.h;
    let (sx, sy) = match par.align {
        None => (sx, sy),
        Some(_) => {
            let s = if par.slice { sx.max(sy) } else { sx.min(sy) };
            (s, s)
        }
    };
    let (ax, ay) = par.align.unwrap_or((0.0, 0.0));
    let tx = viewport.x - vb.x * sx + (viewport.w - vb.w * sx) * ax;
    let ty = viewport.y - vb.y * sy + (viewport.h - vb.h * sy) * ay;
    [sx, 0.0, 0.0, sy, tx, ty]
}

// ------------------------------------------------------------------ matrices

/// `m × n` (apply `n` first, then `m`).
pub fn multiply(m: &Matrix, n: &Matrix) -> Matrix {
    [
        m[0] * n[0] + m[2] * n[1],
        m[1] * n[0] + m[3] * n[1],
        m[0] * n[2] + m[2] * n[3],
        m[1] * n[2] + m[3] * n[3],
        m[0] * n[4] + m[2] * n[5] + m[4],
        m[1] * n[4] + m[3] * n[5] + m[5],
    ]
}

pub fn transform_point(m: &Matrix, p: Point) -> Point {
    Point { x: m[0] * p.x + m[2] * p.y + m[4], y: m[1] * p.x + m[3] * p.y + m[5] }
}

pub fn is_identity(m: &Matrix) -> bool {
    *m == IDENTITY
}

/// Average scale factor of a matrix.
pub fn scale_factor(m: &Matrix) -> f32 {
    ((m[0] * m[3] - m[1] * m[2]).abs()).sqrt()
}

/// Axis-aligned bounds of a transformed rect.
/// The inverse of an affine matrix, if invertible.
pub fn invert(m: &Matrix) -> Option<Matrix> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-12 || !det.is_finite() {
        return None;
    }
    let (a, b, c, d) = (m[3] / det, -m[1] / det, -m[2] / det, m[0] / det);
    Some([a, b, c, d, -(a * m[4] + c * m[5]), -(b * m[4] + d * m[5])])
}

pub fn transform_rect_bounds(m: &Matrix, r: Rect) -> Rect {
    let pts = [
        transform_point(m, Point { x: r.x, y: r.y }),
        transform_point(m, Point { x: r.right(), y: r.y }),
        transform_point(m, Point { x: r.x, y: r.bottom() }),
        transform_point(m, Point { x: r.right(), y: r.bottom() }),
    ];
    let mut x0 = f32::MAX;
    let mut y0 = f32::MAX;
    let mut x1 = f32::MIN;
    let mut y1 = f32::MIN;
    for p in pts {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

/// Parse an SVG `transform` attribute (list of functions) into a matrix.
pub fn parse_transform(s: &str) -> Matrix {
    let mut m = IDENTITY;
    let mut rest = s.trim();
    while !rest.is_empty() {
        let Some(open) = rest.find('(') else { break };
        let name = rest[..open].trim().trim_start_matches(',').trim().to_ascii_lowercase();
        let Some(close) = rest[open..].find(')') else { break };
        let args = parse_number_list(&rest[open + 1..open + close]);
        rest = rest[open + close + 1..].trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        let f: Matrix = match (name.as_str(), args.len()) {
            ("matrix", 6) => [args[0], args[1], args[2], args[3], args[4], args[5]],
            ("translate", 1) => [1.0, 0.0, 0.0, 1.0, args[0], 0.0],
            ("translate", 2) => [1.0, 0.0, 0.0, 1.0, args[0], args[1]],
            ("scale", 1) => [args[0], 0.0, 0.0, args[0], 0.0, 0.0],
            ("scale", 2) => [args[0], 0.0, 0.0, args[1], 0.0, 0.0],
            ("rotate", 1) => rotation(args[0].to_radians()),
            ("rotate", 3) => {
                let (cx, cy) = (args[1], args[2]);
                let r = rotation(args[0].to_radians());
                let t1 = [1.0, 0.0, 0.0, 1.0, cx, cy];
                let t2 = [1.0, 0.0, 0.0, 1.0, -cx, -cy];
                multiply(&multiply(&t1, &r), &t2)
            }
            ("skewx", 1) => [1.0, 0.0, args[0].to_radians().tan(), 1.0, 0.0, 0.0],
            ("skewy", 1) => [1.0, args[0].to_radians().tan(), 0.0, 1.0, 0.0, 0.0],
            _ => continue,
        };
        m = multiply(&m, &f);
    }
    m
}

fn rotation(a: f32) -> Matrix {
    [a.cos(), a.sin(), -a.sin(), a.cos(), 0.0, 0.0]
}

/// `translate(x, y)` matrix.
pub fn translate(x: f32, y: f32) -> Matrix {
    [1.0, 0.0, 0.0, 1.0, x, y]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_lengths() {
        assert_eq!(parse_number_list("1,2 3-4.5.5e1"), vec![1.0, 2.0, 3.0, -4.5, 0.5e1]);
        assert_eq!(parse_length("12px"), Some(Length::Px(12.0)));
        assert_eq!(parse_length("50%"), Some(Length::Percent(50.0)));
        assert_eq!(parse_length("1em"), Some(Length::Px(16.0)));
        assert_eq!(parse_length("1e2"), Some(Length::Px(100.0)));
        assert_eq!(parse_length("2in"), Some(Length::Px(192.0)));
        assert_eq!(parse_length("abc"), None);
    }

    #[test]
    fn transforms() {
        let m = parse_transform("translate(10, 20) scale(2)");
        let p = transform_point(&m, Point { x: 1.0, y: 1.0 });
        assert_eq!((p.x, p.y), (12.0, 22.0));
        let r = parse_transform("rotate(90 10 10)");
        let p = transform_point(&r, Point { x: 20.0, y: 10.0 });
        assert!((p.x - 10.0).abs() < 1e-4 && (p.y - 20.0).abs() < 1e-4, "{p:?}");
        let mtx = parse_transform("matrix(1 0 0 1 5 6)");
        assert_eq!(mtx[4], 5.0);
    }

    #[test]
    fn viewbox_math() {
        let vb = parse_viewbox("0 0 100 50").unwrap();
        let m = viewbox_transform(vb, Rect::new(0.0, 0.0, 200.0, 200.0), AspectRatio::default());
        // meet: scale 2, centred vertically: offset (200 - 100) / 2 = 50
        assert_eq!(m, [2.0, 0.0, 0.0, 2.0, 0.0, 50.0]);
        let m = viewbox_transform(vb, Rect::new(0.0, 0.0, 200.0, 200.0), parse_aspect_ratio(Some("none")));
        assert_eq!(m, [2.0, 0.0, 0.0, 4.0, 0.0, 0.0]);
        let m = viewbox_transform(vb, Rect::new(0.0, 0.0, 200.0, 200.0), parse_aspect_ratio(Some("xMinYMax slice")));
        assert_eq!(m, [4.0, 0.0, 0.0, 4.0, 0.0, 0.0]);
    }
}

/// A length of the CSS transform text the engine passes (`Npx`, `N%`,
/// `Apx+B%`, unitless numbers as px) against `reference`.
fn css_len(s: &str, reference: f32) -> Option<f32> {
    let s = s.trim();
    if let Some((a, b)) = s.split_once("px+") {
        return Some(a.trim().parse::<f32>().ok()? + b.trim().trim_end_matches('%').parse::<f32>().ok()? / 100.0 * reference);
    }
    if let Some(p) = s.strip_suffix('%') {
        return Some(p.trim().parse::<f32>().ok()? / 100.0 * reference);
    }
    match parse_length(s)? {
        Length::Px(v) => Some(v),
        Length::Percent(p) => Some(p / 100.0 * reference),
    }
}

/// An angle (`rad`, `deg`, `grad`, `turn`; bare numbers are degrees).
fn css_angle(s: &str) -> Option<f32> {
    let s = s.trim();
    for (u, f) in [("deg", std::f32::consts::PI / 180.0), ("grad", std::f32::consts::PI / 200.0), ("rad", 1.0), ("turn", std::f32::consts::TAU)] {
        if let Some(v) = s.strip_suffix(u) {
            return Some(v.trim().parse::<f32>().ok()? * f);
        }
    }
    Some(s.parse::<f32>().ok()?.to_radians())
}

/// A CSS transform list (with units) against the reference box `r`.
fn parse_css_transform(s: &str, r: Rect) -> Option<Matrix> {
    let mut m = IDENTITY;
    let mut rest = s.trim();
    while !rest.is_empty() {
        let open = rest.find('(')?;
        let name = rest[..open].trim().to_ascii_lowercase();
        let close = rest[open..].find(')')? + open;
        let args: Vec<&str> = rest[open + 1..close].split(',').map(str::trim).filter(|a| !a.is_empty()).collect();
        rest = rest[close + 1..].trim_start();
        let num = |i: usize| args.get(i).and_then(|a| a.parse::<f32>().ok());
        let f: Matrix = match (name.as_str(), args.len()) {
            ("matrix", 6) => [num(0)?, num(1)?, num(2)?, num(3)?, num(4)?, num(5)?],
            ("translate", 1) => [1.0, 0.0, 0.0, 1.0, css_len(args[0], r.w)?, 0.0],
            ("translate", 2) => [1.0, 0.0, 0.0, 1.0, css_len(args[0], r.w)?, css_len(args[1], r.h)?],
            ("translatex", 1) => [1.0, 0.0, 0.0, 1.0, css_len(args[0], r.w)?, 0.0],
            ("translatey", 1) => [1.0, 0.0, 0.0, 1.0, 0.0, css_len(args[0], r.h)?],
            ("scale", 1) => [num(0)?, 0.0, 0.0, num(0)?, 0.0, 0.0],
            ("scale", 2) => [num(0)?, 0.0, 0.0, num(1)?, 0.0, 0.0],
            ("scalex", 1) => [num(0)?, 0.0, 0.0, 1.0, 0.0, 0.0],
            ("scaley", 1) => [1.0, 0.0, 0.0, num(0)?, 0.0, 0.0],
            ("rotate", 1) => rotation(css_angle(args[0])?),
            ("skew", 1) => [1.0, 0.0, css_angle(args[0])?.tan(), 1.0, 0.0, 0.0],
            ("skew", 2) => [1.0, css_angle(args[1])?.tan(), css_angle(args[0])?.tan(), 1.0, 0.0, 0.0],
            ("skewx", 1) => [1.0, 0.0, css_angle(args[0])?.tan(), 1.0, 0.0, 0.0],
            ("skewy", 1) => [1.0, css_angle(args[0])?.tan(), 0.0, 1.0, 0.0, 0.0],
            _ => return None,
        };
        m = multiply(&m, &f);
    }
    Some(m)
}

/// A `transform-origin` against the reference box `r` (keywords, lengths
/// and percentages; a z length is ignored). `None` when invalid.
fn parse_origin(s: &str, r: Rect) -> Option<(f32, f32)> {
    #[derive(Clone, Copy, PartialEq)]
    enum V {
        H(f32),
        Vert(f32),
        Center,
        Len(&'static str),
    }
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let class = |p: &str| -> V {
        match p {
            "left" => V::H(0.0),
            "right" => V::H(1.0),
            "top" => V::Vert(0.0),
            "bottom" => V::Vert(1.0),
            "center" => V::Center,
            _ => V::Len(""),
        }
    };
    if parts.len() == 3 {
        // The z component must be a length (not a percentage).
        let z = parts[2];
        if z.ends_with('%') || css_len(z, 0.0).is_none() {
            return None;
        }
    }
    let x_of = |p: &str, v: V| -> Option<f32> {
        match v {
            V::H(f) => Some(r.x + f * r.w),
            V::Center => Some(r.x + 0.5 * r.w),
            V::Len(_) => Some(r.x + css_len(p, r.w)?),
            V::Vert(_) => None,
        }
    };
    let y_of = |p: &str, v: V| -> Option<f32> {
        match v {
            V::Vert(f) => Some(r.y + f * r.h),
            V::Center => Some(r.y + 0.5 * r.h),
            V::Len(_) => Some(r.y + css_len(p, r.h)?),
            V::H(_) => None,
        }
    };
    let a = class(parts[0]);
    if parts.len() == 1 {
        return match a {
            V::Vert(_) => Some((r.x + 0.5 * r.w, y_of(parts[0], a)?)),
            _ => Some((x_of(parts[0], a)?, r.y + 0.5 * r.h)),
        };
    }
    let b = class(parts[1]);
    // Two keywords may come in either order.
    if !matches!(a, V::Len(_)) && !matches!(b, V::Len(_)) && (matches!(a, V::Vert(_)) || matches!(b, V::H(_))) {
        return Some((x_of(parts[1], b)?, y_of(parts[0], a)?));
    }
    Some((x_of(parts[0], a)?, y_of(parts[1], b)?))
}

/// The element's transform from its CSS transform properties: the CSS
/// `transform` (else the attribute's `attr`) about `transform-origin`, both
/// against the `transform-box` (`bbox` is the fill box).
pub fn css_transform(x: &crate::parse::XformSpec, attr: Matrix, bbox: Option<Rect>) -> Matrix {
    let view = Rect::new(0.0, 0.0, x.view.0, x.view.1);
    let fill = bbox.unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0));
    let r = match x.tbox.as_deref() {
        Some("fill-box" | "content-box" | "stroke-box" | "border-box") => fill,
        _ => view,
    };
    let m = match x.css.as_deref() {
        Some(t) => parse_css_transform(t, r).unwrap_or(attr),
        None => attr,
    };
    let (ox, oy) = match x.origin.as_deref() {
        Some(o) => parse_origin(o.trim(), r).unwrap_or((r.x, r.y)),
        None => (r.x, r.y),
    };
    multiply(&multiply(&translate(ox, oy), &m), &translate(-ox, -oy))
}
