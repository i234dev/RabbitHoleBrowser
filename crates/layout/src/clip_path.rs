//! `clip-path` geometry: basic shapes and reference boxes resolved to a
//! path in document coordinates (CSS Masking 1 §3, CSS Shapes 1/2).

use crate::background::{box_radii, inner_radii};
use crate::tree::LayoutBox;
use rabbithole_gfx::path::rounded_rect_path;
use rabbithole_gfx::{FillRule, PathSeg, Radii, Rect};
use rabbithole_style::{ClipPath, ShapeBox};

/// A resolved clip: the path (document coordinates) and its fill rule.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipGeometry {
    pub path: Vec<PathSeg>,
    pub rule: FillRule,
    /// The same clip as a rounded rectangle (or ellipse), when it is one:
    /// painted with the exact rounded-box coverage of borders.
    pub rounded: Option<(Rect, Radii)>,
}

/// The reference box of `b` and its corner radii.
pub(crate) fn reference_box(b: &LayoutBox, which: ShapeBox) -> (Rect, Radii) {
    let border = b.rect;
    let radii = box_radii(b);
    match which {
        ShapeBox::Border | ShapeBox::Stroke | ShapeBox::View => (border, radii),
        ShapeBox::Padding => (b.padding_rect(), inner_radii(radii, &b.style.border_width)),
        ShapeBox::HalfBorder => {
            let bw = &b.style.border_width;
            let half = rabbithole_style::Edges { top: bw.top / 2.0, right: bw.right / 2.0, bottom: bw.bottom / 2.0, left: bw.left / 2.0 };
            let r = Rect::new(border.x + half.left, border.y + half.top, (border.w - half.left - half.right).max(0.0), (border.h - half.top - half.bottom).max(0.0));
            (r, inner_radii(radii, &half))
        }
        ShapeBox::Content | ShapeBox::Fill => {
            let p = b.padding_rect();
            let pr = inner_radii(radii, &b.style.border_width);
            let c = b.content_rect;
            (c, shrink_radii(pr, p, c))
        }
        ShapeBox::Margin => {
            let m = b.margin_rect();
            (m, grow_radii(radii, border, m))
        }
    }
}

/// Radii of `inner` given the radii of the enclosing `outer`.
fn shrink_radii(r: Radii, outer: Rect, inner: Rect) -> Radii {
    let (l, t) = (inner.x - outer.x, inner.y - outer.y);
    let (rt, b) = (outer.right() - inner.right(), outer.bottom() - inner.bottom());
    let sub = |a: f32, w: f32| (a - w).max(0.0);
    [
        (sub(r[0].0, l), sub(r[0].1, t)),
        (sub(r[1].0, rt), sub(r[1].1, t)),
        (sub(r[2].0, rt), sub(r[2].1, b)),
        (sub(r[3].0, l), sub(r[3].1, b)),
    ]
}

/// Radii of the margin box: rounded corners grow by the margin.
fn grow_radii(r: Radii, inner: Rect, outer: Rect) -> Radii {
    let (l, t) = (inner.x - outer.x, inner.y - outer.y);
    let (rt, b) = (outer.right() - inner.right(), outer.bottom() - inner.bottom());
    let add = |a: f32, m: f32| if a > 0.0 { (a + m).max(0.0) } else { 0.0 };
    let corner = |c: (f32, f32), mh: f32, mv: f32| if c.0 > 0.0 && c.1 > 0.0 { (add(c.0, mh), add(c.1, mv)) } else { (0.0, 0.0) };
    [corner(r[0], l, t), corner(r[1], rt, t), corner(r[2], rt, b), corner(r[3], l, b)]
}

/// The clip geometry of `b` for its `clip-path`, or `None` when nothing is
/// clipped (`url()` references are resolved by the caller).
pub fn geometry(b: &LayoutBox, cp: &ClipPath) -> Option<ClipGeometry> {
    match cp {
        ClipPath::Box(which) => {
            let (r, radii) = reference_box(b, *which);
            Some(ClipGeometry { path: rounded_rect_path(r, radii), rule: FillRule::NonZero, rounded: Some((r, radii)) })
        }
        ClipPath::Shape(shape, which) => {
            let (r, _) = reference_box(b, *which);
            let (path, rule) = rabbithole_style::shapes::basic_shape_path(shape, r);
            // Circles and ellipses get the exact coverage of rounded boxes
            // (like `border-radius`); rectangles keep the path, whose
            // corners rasterize like the same path drawn elsewhere.
            let ellipse = matches!(shape, rabbithole_style::BasicShape::Circle { .. } | rabbithole_style::BasicShape::Ellipse { .. });
            let rounded = if ellipse { rabbithole_style::shapes::basic_shape_rounded(shape, r) } else { None };
            Some(ClipGeometry { path, rule, rounded })
        }
        ClipPath::Url(_) => None,
    }
}

/// Whether the point lies inside the flattened clip (hit testing).
pub fn contains(poly: &rabbithole_gfx::path::Polylines, rule: FillRule, x: f32, y: f32) -> bool {
    let mut winding = 0i32;
    for c in &poly.contours {
        let n = c.points.len();
        if n < 2 {
            continue;
        }
        // Contours are implicitly closed for filling.
        for i in 0..n {
            let a = c.points[i];
            let b = c.points[(i + 1) % n];
            if (a.y <= y) != (b.y <= y) {
                let t = (y - a.y) / (b.y - a.y);
                if x < a.x + t * (b.x - a.x) {
                    winding += if b.y > a.y { 1 } else { -1 };
                }
            }
        }
    }
    match rule {
        FillRule::NonZero => winding != 0,
        FillRule::EvenOdd => winding % 2 != 0,
    }
}

/// Bounds of a path's points (control points included: a cover).
pub fn path_bounds(path: &[PathSeg]) -> Rect {
    let poly = rabbithole_gfx::path::flatten(path, &rabbithole_gfx::IDENTITY, 0.5);
    rabbithole_gfx::path::polylines_bounds(&poly).unwrap_or_default()
}

/// `clip-path: url(#id)`: the outline of an SVG `<clipPath>` of the
/// document, in the user space of the border box (CSS Masking 1 §3.2).
/// `None` when the reference is invalid (nothing is clipped).
pub(crate) fn url_geometry(b: &LayoutBox, id: &str, ctx: &crate::LayoutContext) -> Option<ClipGeometry> {
    let doc = ctx.doc;
    if id.is_empty() {
        return None;
    }
    let node = doc.element_by_id(id)?;
    if !doc.tag(node)?.eq_ignore_ascii_case("clippath") {
        return None;
    }
    let svg = doc.ancestor_or_self(node, |a| doc.is_element(a, "svg"))?;
    let styles = ctx.styles;
    let style_of = |n: rabbithole_dom::NodeId, prop: &str| -> Option<String> {
        let s = styles.get(n)?;
        match prop {
            "display" if s.display.is_none() => Some("none".into()),
            _ => s.svg_props.iter().find(|(p, _)| p == prop).map(|(_, v)| rabbithole_svg::tokens_to_string(v).trim().to_string()),
        }
    };
    let tree = rabbithole_svg::SvgTree::from_dom(doc, svg, &style_of)?;
    let transform = styles
        .get(node)
        .filter(|s| s.has_transform())
        .map(|s| rabbithole_style::transform_matrix(&s.transform, 0.0, 0.0, s.font_size));
    let (path, rule) = tree.clip_outline(id, b.rect, transform)?;
    Some(ClipGeometry { path, rule, rounded: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rabbithole_gfx::Point;

    fn square(x: f32, y: f32, s: f32) -> Vec<PathSeg> {
        vec![
            PathSeg::MoveTo(Point::new(x, y)),
            PathSeg::LineTo(Point::new(x + s, y)),
            PathSeg::LineTo(Point::new(x + s, y + s)),
            PathSeg::LineTo(Point::new(x, y + s)),
            PathSeg::Close,
        ]
    }

    #[test]
    fn hit_testing_follows_the_fill_rule() {
        let mut path = square(0.0, 0.0, 100.0);
        path.extend(square(25.0, 25.0, 50.0));
        let poly = rabbithole_gfx::path::flatten(&path, &rabbithole_gfx::IDENTITY, 0.25);
        assert!(contains(&poly, FillRule::NonZero, 50.0, 50.0));
        assert!(!contains(&poly, FillRule::EvenOdd, 50.0, 50.0));
        assert!(contains(&poly, FillRule::EvenOdd, 10.0, 10.0));
        assert!(!contains(&poly, FillRule::NonZero, 150.0, 50.0));
    }

    #[test]
    fn margin_box_radii_grow_and_content_box_radii_shrink() {
        let outer = Rect::new(0.0, 0.0, 100.0, 100.0);
        let inner = Rect::new(10.0, 5.0, 80.0, 90.0);
        assert_eq!(shrink_radii([(20.0, 20.0); 4], outer, inner)[0], (10.0, 15.0));
        assert_eq!(grow_radii([(20.0, 20.0), (0.0, 0.0), (20.0, 20.0), (20.0, 20.0)], inner, outer)[0], (30.0, 25.0));
        assert_eq!(grow_radii([(20.0, 20.0), (0.0, 0.0), (20.0, 20.0), (20.0, 20.0)], inner, outer)[1], (0.0, 0.0));
    }
}
