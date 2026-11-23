//! CSS Motion Path longhands. The offset transform is resolved after layout.
use super::{clip_path, len_no_auto};
use crate::computed::{ClipPath, ComputedStyle, Length, ShapeBox};
use crate::values::{self, LengthContext};
use rabbithole_css::Token;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OffsetPosition {
    Auto,
    Normal,
    Coordinates(Length, Length),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OffsetRotate {
    pub follows_path: bool,
    pub degrees: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Motion {
    pub path: Option<Arc<ClipPath>>,
    pub explicit_center: bool,
    pub distance: Length,
    pub rotate: OffsetRotate,
    pub anchor: OffsetPosition,
    pub position: OffsetPosition,
}

pub static INITIAL: Motion = Motion {
    path: None, explicit_center: false, distance: Length::Px(0.0),
    rotate: OffsetRotate { follows_path: true, degrees: 0.0 },
    anchor: OffsetPosition::Auto, position: OffsetPosition::Normal,
};

impl ComputedStyle {
    pub fn motion(&self) -> &Motion { self.motion.as_deref().unwrap_or(&INITIAL) }
    pub fn motion_mut(&mut self) -> &mut Motion { self.motion.get_or_insert_with(|| Box::new(INITIAL.clone())) }
    pub fn has_motion_path(&self) -> bool { self.motion.as_ref().is_some_and(|m| m.path.is_some()) }
}

impl OffsetPosition {
    fn parse(parts: &[&[Token]], ctx: &LengthContext, normal: bool) -> Option<Self> {
        match parts {
            [p] if values::is_ident(p,"auto") => Some(Self::Auto),
            [p] if normal && values::is_ident(p,"normal") => Some(Self::Normal),
            _ => clip_path::parse_position(parts,ctx).map(|(x,y)| Self::Coordinates(x,y)),
        }
    }
}

impl OffsetRotate {
    fn parse(parts: &[&[Token]]) -> Option<Self> {
        if parts.is_empty() || parts.len() > 2 { return None; }
        let mut facing = None;
        let mut degrees = None;
        for part in parts {
            if let Some(direction) = values::ident(part) {
                let reverse = match direction.as_str() { "auto" => false, "reverse" => true, _ => return None };
                if facing.replace(reverse).is_some() { return None; }
            } else {
                // The angle grammar requires units, even for zero.
                if matches!(**part,[Token::Number(_)]) { return None; }
                let angle = super::angle(part)?.to_degrees();
                if degrees.replace(angle).is_some() { return None; }
            }
        }
        Some(Self { follows_path: facing.is_some(), degrees: degrees.unwrap_or(0.0) + if facing == Some(true) { 180.0 } else { 0.0 } })
    }
}

pub fn property(name: &str) -> bool {
    matches!(name, "offset-path" | "offset-distance" | "offset-rotate" | "offset-position" | "offset-anchor")
}

fn path(parts: &[&[Token]], ctx: &LengthContext) -> Option<Option<ClipPath>> {
    for part in parts {
        match super::fn_name(part).as_deref() {
            Some("url") => return None,
            Some("path" | "polygon" | "shape") if part.iter().any(|t| matches!(t, Token::Ident(k) if k.eq_ignore_ascii_case("evenodd") || k.eq_ignore_ascii_case("nonzero"))) => return None,
            _ => {}
        }
    }
    let path = clip_path::parse_shape_value(parts, ctx, ShapeBox::Border)?;
    match &path {
        Some(ClipPath::Url(_) | ClipPath::Box(ShapeBox::Margin | ShapeBox::HalfBorder) |
            ClipPath::Shape(_, ShapeBox::Margin | ShapeBox::HalfBorder)) => None,
        _ => Some(path),
    }
}

pub fn apply(name: &str, parts: &[&[Token]], style: &mut ComputedStyle, ctx: &LengthContext) -> bool {
    match name {
        "offset-path" => {
            if let Some(path) = path(parts,ctx) {
                let m = style.motion_mut();
                m.path = path.map(Arc::new);
                m.explicit_center = parts.iter().any(|p| p.iter().any(|t| matches!(t,Token::Ident(s) if s.eq_ignore_ascii_case("at"))));
            }
        }
        "offset-distance" => {
            if let [p] = parts {
                if let Some(v) = len_no_auto(p,ctx) { style.motion_mut().distance = v; }
            }
        }
        "offset-rotate" => {
            if let Some(v) = OffsetRotate::parse(parts) { style.motion_mut().rotate = v; }
        }
        "offset-anchor" | "offset-position" => {
            if let Some(v) = OffsetPosition::parse(parts,ctx,name == "offset-position") {
                if name == "offset-anchor" { style.motion_mut().anchor = v; } else { style.motion_mut().position = v; }
            }
        }
        _ => return false,
    }
    true
}

pub fn copy(name: &str, from: &ComputedStyle, to: &mut ComputedStyle) {
    if from.motion.is_none() && to.motion.is_none() { return; }
    let src = from.motion();
    let dest = to.motion_mut();
    match name {
        "offset-path" => { dest.path = src.path.clone(); dest.explicit_center = src.explicit_center; }
        "offset-distance" => dest.distance = src.distance,
        "offset-anchor" => dest.anchor = src.anchor,
        "offset-position" => dest.position = src.position,
        "offset-rotate" => dest.rotate = src.rotate,
        _ => unreachable!(),
    }
}
