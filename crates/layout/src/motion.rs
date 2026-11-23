//! Resolve an element's motion path in its containing block after layout.
use crate::tree::{BoxKind, LayoutBox};
use rabbithole_gfx::{path, Matrix, Point, Rect, IDENTITY};
use rabbithole_style::props::motion::OffsetPosition;
use rabbithole_style::{BasicShape, ClipPath, Length, Position};

fn point_on_path(path: &path::Polylines, distance: Length) -> Option<(Point, f32)> {
    // A move starts a new contour without adding a connecting line.
    let edges = || path.contours.iter().flat_map(|contour| {
        let closing = contour.points.last().zip(contour.points.first()).filter(|_| contour.closed);
        contour.points.windows(2).map(|w| (&w[0], &w[1])).chain(closing)
    }).filter_map(|(a,b)| {
        let dx = b.x-a.x;
        let dy = b.y-a.y;
        let length = dx.hypot(dy);
        (length > 0.0).then_some((*a,dx,dy,length))
    });
    let total: f32 = edges().map(|(_,_,_,n)| n).sum();
    let mut point = *path.contours.first()?.points.first()?;
    let mut angle = 0.0;
    if total == 0.0 { return Some((point,angle)); }
    let specified = distance.resolve_or_zero(total);
    let closed = path.contours.len() == 1 && path.contours[0].closed;
    let mut remaining = if closed { specified.rem_euclid(total) } else { specified.clamp(0.0,total) };
    for (a,dx,dy,len) in edges() {
        let t = (remaining/len).min(1.0);
        point = Point::new(a.x+t*dx,a.y+t*dy);
        angle = dy.atan2(dx);
        if remaining < len { break; }
        remaining -= len;
    }
    Some((point,angle))
}

fn at(position: OffsetPosition, reference: Rect, auto: Point, normal: Point) -> Point {
    match position {
        OffsetPosition::Coordinates(x,y) => Point::new(reference.x+x.resolve_or_zero(reference.w), reference.y+y.resolve_or_zero(reference.h)),
        OffsetPosition::Normal => normal,
        OffsetPosition::Auto => auto,
    }
}

fn resolve(b: &LayoutBox, container: Option<&LayoutBox>, viewport: Rect) -> Option<Matrix> {
    let m = b.style.motion();
    let p = m.path.as_deref()?;
    let which = match p { ClipPath::Box(k) | ClipPath::Shape(_,k) => *k, ClipPath::Url(_) => return None };
    let (rb, radii) = container.map_or((viewport, [(0.0,0.0);4]), |c| crate::clip_path::reference_box(c,which));
    let top_left = Point::new(b.rect.x,b.rect.y);
    let mut ellipse = None;
    let outline = match p {
        ClipPath::Box(_) => path::rounded_rect_path(rb,radii),
        ClipPath::Shape(shape,_) => {
            let mut rb = rb;
            let center = Point::new(rb.x+rb.w*0.5,rb.y+rb.h*0.5);
            let adjusted;
            let shape = match shape {
                BasicShape::Path { .. } => {
                    let origin = at(m.position,rb,top_left,top_left);
                    rb.x = origin.x;
                    rb.y = origin.y;
                    shape
                }
                BasicShape::Circle { .. } | BasicShape::Ellipse { .. } if !m.explicit_center && m.position != OffsetPosition::Normal => {
                    let origin = at(m.position,rb,top_left,center);
                    adjusted = {
                        let mut s = shape.clone();
                        match &mut s {
                            BasicShape::Circle { at, .. } | BasicShape::Ellipse { at, .. } =>
                                *at = (Length::Px(origin.x-rb.x),Length::Px(origin.y-rb.y)),
                            _ => unreachable!(),
                        }
                        s
                    };
                    &adjusted
                }
                other => other,
            };
            if matches!(shape,BasicShape::Circle { .. } | BasicShape::Ellipse { .. }) {
                ellipse = rabbithole_style::shapes::basic_shape_rounded(shape,rb).map(|(rect,_)| rect);
            }
            rabbithole_style::shapes::basic_shape_path(shape,rb).0
        }
        ClipPath::Url(_) => return None,
    };
    let (point,mut tangent) = point_on_path(&path::flatten(&outline,&IDENTITY,0.05),m.distance)?;
    if let Some(r) = ellipse.filter(|r| r.w > 0.0 && r.h > 0.0) {
        tangent = ((point.x-r.x-r.w*0.5)*r.h/r.w).atan2(-(point.y-r.y-r.h*0.5)*r.w/r.h);
    }
    let angle = m.rotate.degrees.to_radians() + if m.rotate.follows_path { tangent } else { 0.0 };
    let reference = b.transform_reference_box();
    let (ox,oy) = b.style.transform_origin;
    let origin = Point::new(reference.x+ox.resolve_or_zero(reference.w),reference.y+oy.resolve_or_zero(reference.h));
    let anchor = at(m.anchor,reference,origin,origin);
    let (sin,cos) = angle.sin_cos();
    let dx = origin.x-anchor.x;
    let dy = origin.y-anchor.y;
    Some([cos,sin,-sin,cos,point.x-origin.x+cos*dx-sin*dy,point.y-origin.y+sin*dx+cos*dy])
}

pub(crate) fn apply(root: &LayoutBox, viewport: Rect) {
    fn walk<'a>(b: &'a LayoutBox, viewport: Rect, flow: Option<&'a LayoutBox>, absolute: Option<&'a LayoutBox>, fixed: Option<&'a LayoutBox>) {
        let container = match b.style.position { Position::Absolute => absolute, Position::Fixed => fixed, _ => flow };
        b.motion_transform.set(if b.style.has_motion_path() { resolve(b,container,viewport) } else { None });
        let fixed = if b.style.establishes_fixed_containing_block() { Some(b) } else { fixed };
        let absolute = if b.is_positioned() || b.style.establishes_fixed_containing_block() { Some(b) } else { absolute };
        let flow = if b.node.is_some() && !matches!(b.kind, BoxKind::Inline | BoxKind::Text | BoxKind::LineBreak) { Some(b) } else { flow };
        for child in &b.children { walk(child,viewport,flow,absolute,fixed); }
    }
    walk(root,viewport,None,None,None);
}
