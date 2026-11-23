//! CSSOM serialization of float exclusion shapes. Specified positions retain
//! their authored units. Computed positions resolve to length-percentage pairs.
use super::{position_css as length, tokens_to_css};
use rabbithole_css::Token;
use rabbithole_gfx::{PathSeg, Point};
use rabbithole_style::{BasicShape, ClipPath, ControlAnchor, ControlPoint, Length, ShapeBox, ShapeCommand, ShapeRadius};
use rabbithole_style::values::{components, ident};

fn quad(values: &[String]) -> String {
    let mut n = values.len();
    if n == 4 && values[3] == values[1] { n = 3; }
    if n == 3 && values[2] == values[0] { n = 2; }
    if n == 2 && values[1] == values[0] { n = 1; }
    values[..n].join(" ")
}

fn corner_values(x: &[String], y: &[String]) -> String {
    let x = quad(x);
    let y = quad(y);
    if x == y { x } else { format!("{x} / {y}") }
}

pub(super) fn specified_position(parts: &[&[Token]]) -> String {
    let keyword = |part: &[Token]| ident(part).unwrap_or_default();
    let mut values: Vec<String> = parts.iter().map(|p| tokens_to_css(p)).collect();
    let vertical = |s: &str| matches!(s,"top" | "bottom");
    let horizontal = |s: &str| matches!(s,"left" | "right");
    match parts {
        [a] if vertical(&keyword(a)) => values.insert(0, "center".into()),
        [_] => values.push("center".into()),
        [a,b] if vertical(&keyword(a)) || horizontal(&keyword(b)) => values.swap(0,1),
        [a,_,_,_] if vertical(&keyword(a)) => values.rotate_left(2),
        _ => {}
    }
    values.join(" ")
}

pub(super) fn specified(tokens: &[Token], default_box: &str) -> String {
    if tokens.iter().any(|t| matches!(t, Token::Function(name) if ["var","env","attr"].iter().any(|n| name.eq_ignore_ascii_case(n)))) {
        return tokens_to_css(tokens);
    }
    let mut value = None;
    let mut which = None;
    for comp in components(tokens) {
        let Some(Token::Function(name)) = comp.first() else {
            which = Some(tokens_to_css(comp));
            continue;
        };
        if !matches!(comp.last(), Some(Token::CloseParen)) { return tokens_to_css(tokens); }
        let args = components(&comp[1..comp.len()-1]);
        let name = name.to_ascii_lowercase();
        let inner = match name.as_str() {
            "circle" | "ellipse" => {
                let at = args.iter().position(|v| ident(v).as_deref() == Some("at"));
                let radii = &args[..at.unwrap_or(args.len())];
                let mut out: Vec<String> = if radii.iter().all(|r| ident(r).as_deref() == Some("closest-side")) {
                    Vec::new()
                } else {
                    radii.iter().map(|r| tokens_to_css(r)).collect()
                };
                if let Some(i) = at { out.push(format!("at {}",specified_position(&args[i+1..]))); }
                out.join(" ")
            }
            "inset" => {
                let round = args.iter().position(|v| ident(v).as_deref() == Some("round"));
                let mut out = quad(&args[..round.unwrap_or(args.len())].iter().map(|v| tokens_to_css(v)).collect::<Vec<_>>());
                if let Some(i) = round {
                    let radii = &args[i+1..];
                    let cut = radii.iter().position(|v| matches!(*v, [Token::Delim('/')]));
                    let x: Vec<_> = radii[..cut.unwrap_or(radii.len())].iter().map(|v| tokens_to_css(v)).collect();
                    let y: Vec<_> = cut.map_or_else(|| x.clone(), |j| radii[j+1..].iter().map(|v| tokens_to_css(v)).collect());
                    out.push_str(" round ");
                    out.push_str(&corner_values(&x,&y));
                }
                out
            }
            _ => { value = Some(tokens_to_css(comp)); continue; }
        };
        value = Some(format!("{name}({inner})"));
    }
    match (value, which) {
        (Some(s), Some(b)) if b != default_box => format!("{s} {b}"),
        (Some(s), _) | (None,Some(s)) => s,
        _ => tokens_to_css(tokens),
    }
}

fn pair(x: &Length, y: &Length) -> String { format!("{} {}", length(x), length(y)) }

fn radius(r: &ShapeRadius) -> String {
    match r {
        ShapeRadius::Len(v) => length(v),
        ShapeRadius::ClosestSide => "closest-side".into(),
        ShapeRadius::FarthestSide => "farthest-side".into(),
        ShapeRadius::ClosestCorner => "closest-corner".into(),
        ShapeRadius::FarthestCorner => "farthest-corner".into(),
    }
}

fn round(out: &mut String, radii: &[(Length, Length);4]) {
    if radii.iter().all(|(x,y)| *x == Length::Px(0.0) && *y == Length::Px(0.0)) { return; }
    out.push_str(" round ");
    out.push_str(&corner_values(&radii.map(|(x,_)| length(&x)),&radii.map(|(_,y)| length(&y))));
}

fn radial(name: &str, radii: &[&ShapeRadius], at: &(Length,Length), explicit: bool) -> String {
    let mut out = Vec::new();
    if !radii.iter().all(|r| **r == ShapeRadius::ClosestSide) {
        out.extend(radii.iter().map(|r| radius(r)));
    }
    if explicit { out.push(format!("at {}",pair(&at.0,&at.1))); }
    format!("{name}({})",out.join(" "))
}

fn point(p: &Point) -> String { format!("{} {}",p.x,p.y) }

fn segment(s: &PathSeg) -> String {
    match s {
        PathSeg::MoveTo(p) => format!("M {}",point(p)),
        PathSeg::LineTo(p) => format!("L {}",point(p)),
        PathSeg::QuadTo(a,b) => format!("Q {} {}",point(a),point(b)),
        PathSeg::CubicTo(a,b,c) => format!("C {} {} {}",point(a),point(b),point(c)),
        PathSeg::ArcTo{rx,ry,rotation,large_arc,sweep,to} =>
            format!("A {rx} {ry} {} {} {} {}",rotation.to_degrees(),u8::from(*large_arc),u8::from(*sweep),point(to)),
        PathSeg::Close => "Z".into(),
    }
}

fn control(c: &ControlPoint, by: bool) -> String {
    let suffix = match (c.anchor,by) {
        (ControlAnchor::Origin,false) | (ControlAnchor::Start,true) => "",
        (ControlAnchor::Origin,_) => " from origin",
        (ControlAnchor::Start,_) => " from start",
        (ControlAnchor::End,_) => " from end",
    };
    format!("{}{suffix}",pair(&c.x,&c.y))
}

fn command(cmd: &ShapeCommand) -> String {
    use ShapeCommand::*;
    let dir = |by: bool| if by { "by" } else { "to" };
    match cmd {
        Move{by,x,y} => format!("move {} {}",dir(*by),pair(x,y)),
        Line{by,x,y} => format!("line {} {}",dir(*by),pair(x,y)),
        HLine{by,x} => format!("hline {} {}",dir(*by),length(x)),
        VLine{by,y} => format!("vline {} {}",dir(*by),length(y)),
        Curve{by,x,y,c1,c2} => {
            let mut s = format!("curve {} {} with {}",dir(*by),pair(x,y),control(c1,*by));
            if let Some(c) = c2 { s.push_str(&format!(" / {}",control(c,*by))); }
            s
        }
        Smooth{by,x,y,c} => {
            let mut s = format!("smooth {} {}",dir(*by),pair(x,y));
            if let Some(c) = c { s.push_str(&format!(" with {}",control(c,*by))); }
            s
        }
        Arc{by,x,y,rx,ry,cw,large,rotation} => {
            let mut s = format!("arc {} {} of {}",dir(*by),pair(x,y),length(rx));
            if let Some(ry) = ry { s.push(' '); s.push_str(&length(ry)); }
            if *cw { s.push_str(" cw"); }
            if *large { s.push_str(" large"); }
            if *rotation != 0.0 { s.push_str(&format!(" rotate {}deg",rotation.to_degrees())); }
            s
        }
        Close => "close".into(),
    }
}

fn basic(shape: &BasicShape, explicit: bool) -> String {
    use BasicShape::*;
    match shape {
        Circle{r,at} => radial("circle",&[r],at,explicit),
        Ellipse{rx,ry,at} => radial("ellipse",&[rx,ry],at,explicit),
        Inset{insets,radii} => {
            let mut s = quad(&insets.map(|v| length(&v)));
            round(&mut s,radii);
            format!("inset({s})")
        }
        Rect{edges,radii} => {
            let mut s = edges.map(|v| length(&v)).join(" ");
            round(&mut s,radii);
            format!("rect({s})")
        }
        Xywh{x,y,w,h,radii} => {
            let mut s = format!("{} {}",pair(x,y),pair(w,h));
            round(&mut s,radii);
            format!("xywh({s})")
        }
        Polygon{evenodd,round,points} => {
            let mut prefix = Vec::new();
            if *evenodd { prefix.push("evenodd".into()); }
            if *round != 0.0 { prefix.push(format!("round {}",super::px(*round))); }
            let mut s = Vec::new();
            if !prefix.is_empty() { s.push(prefix.join(" ")); }
            s.extend(points.iter().map(|(x,y)| pair(x,y)));
            format!("polygon({})",s.join(", "))
        }
        Path{evenodd,path} => {
            let rule = if *evenodd { "evenodd, " } else { "" };
            format!("path({rule}\"{}\")",path.iter().map(segment).collect::<Vec<_>>().join(" "))
        }
        Shape{evenodd,from,cmds} => {
            let rule = if *evenodd { "evenodd " } else { "" };
            let mut s = vec![format!("{rule}from {}",pair(&from.0,&from.1))];
            s.extend(cmds.iter().map(command));
            format!("shape({})",s.join(", "))
        }
    }
}

pub(super) fn computed(value: Option<&ClipPath>, explicit: bool, default_box: ShapeBox) -> String {
    let (shape,which) = match value {
        Some(ClipPath::Shape(shape,which)) => (Some(shape),which),
        Some(ClipPath::Box(which)) => (None,which),
        _ => return "none".into(),
    };
    let name = match which {
        ShapeBox::Margin => "margin-box", ShapeBox::Border => "border-box",
        ShapeBox::Padding => "padding-box", ShapeBox::Content => "content-box",
        ShapeBox::Fill => "fill-box", ShapeBox::Stroke => "stroke-box", ShapeBox::View => "view-box",
        ShapeBox::HalfBorder => "half-border-box",
    };
    match shape {
        Some(s) if *which != default_box => format!("{} {name}",basic(s,explicit)),
        Some(s) => basic(s,explicit),
        None => name.into(),
    }
}
