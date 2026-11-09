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
