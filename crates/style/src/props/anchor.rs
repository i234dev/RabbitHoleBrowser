//! The position-area grid can be described in physical axes, in the
//! containing block's logical axes, or in the positioned box's logical axes.
use rabbithole_css::Token;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Family { Neutral, Physical, Logical, SelfLogical, Flow, SelfFlow }

#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis { First, Second, Either }

use Axis::{Either, First, Second};
use Family::{Flow, Logical, Neutral, Physical, SelfFlow, SelfLogical};

const KEYWORDS: &[(&str, Family, Axis)] = &[
    ("none", Neutral, Either), ("all", Neutral, Either), ("center", Neutral, Either),
    ("left", Physical, First), ("right", Physical, First),
    ("top", Physical, Second), ("bottom", Physical, Second),
    ("x-start", Physical, First), ("x-end", Physical, First),
    ("self-x-start", Physical, First), ("self-x-end", Physical, First),
    ("y-start", Physical, Second), ("y-end", Physical, Second),
    ("self-y-start", Physical, Second), ("self-y-end", Physical, Second),
    ("block-start", Logical, First), ("block-end", Logical, First),
    ("inline-start", Logical, Second), ("inline-end", Logical, Second),
    ("self-block-start", SelfLogical, First), ("self-block-end", SelfLogical, First),
    ("self-inline-start", SelfLogical, Second), ("self-inline-end", SelfLogical, Second),
    ("start", Flow, Either), ("end", Flow, Either),
    ("self-start", SelfFlow, Either), ("self-end", SelfFlow, Either),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Track { keyword: u8, span: bool }

impl Track {
    fn parse(tokens: &[Token]) -> Option<Self> {
        let [Token::Ident(ident)] = tokens else { return None };
        let lower = ident.to_ascii_lowercase();
        let (span, name) = lower.strip_prefix("span-").map_or((false,lower.as_str()), |v| (true,v));
        if (name == "all" && !span) || (span && matches!(name, "none" | "center")) { return None; }
        let index = KEYWORDS.iter().position(|(k,_,_)| *k == name)?;
        Some(Self { keyword: index as u8, span })
    }

    fn descriptor(self) -> (&'static str, Family, Axis) { KEYWORDS[self.keyword as usize] }

    fn serialize(self, logical_axis: Option<&str>) -> String {
        let (name,family,_) = self.descriptor();
        let span = if self.span { "span-" } else { "" };
        if let Some(axis) = logical_axis {
            if matches!(family, Logical | SelfLogical | Flow | SelfFlow) {
                let own = if matches!(family, SelfLogical | SelfFlow) { "self-" } else { "" };
                let edge = if name.ends_with("start") { "start" } else { "end" };
                return format!("{span}{own}{axis}{edge}");
            }
        }
        format!("{span}{name}")
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PositionArea { first: Track, second: Option<Track> }

impl PositionArea {
    pub fn parse(parts: &[&[Token]]) -> Option<Self> {
        let (mut a,mut b) = match parts {
            [one] => return Some(Self { first: Track::parse(one)?, second: None }),
            [a,b] => (Track::parse(a)?,Track::parse(b)?),
            _ => return None,
        };
        let (name, family, axis) = a.descriptor();
        let (other, other_family, other_axis) = b.descriptor();
        if name == "none" || other == "none" ||
            (family != Neutral && other_family != Neutral && family != other_family) ||
            (axis != Either && axis == other_axis) { return None; }
        if axis == Second || other_axis == First { std::mem::swap(&mut a,&mut b); }
        let single = if a == b { Some(a) }
            else if a.descriptor().0 == "all" && b.descriptor().2 != Either { Some(b) }
            else if b.descriptor().0 == "all" && a.descriptor().2 != Either { Some(a) }
            else { None };
        Some(single.map_or(Self { first:a, second:Some(b) }, |first| Self { first, second:None }))
    }

    pub fn to_css(self, computed: bool) -> String {
        let Some(second) = self.second else { return self.first.serialize(None) };
        if computed {
            let (a,af,_) = self.first.descriptor();
            let (b,bf,_) = second.descriptor();
            // With an omitted (full-span) axis, spell out which logical
            // axis the remaining track belongs to. With both present,
            // their order already conveys block then inline.
            if a == "all" && matches!(bf, Flow | SelfFlow) { return second.serialize(Some("inline-")); }
            if b == "all" && matches!(af, Flow | SelfFlow) { return self.first.serialize(Some("block-")); }
            let first = self.first.serialize(Some(""));
            let second = second.serialize(Some(""));
            return if first == second { first } else { format!("{first} {second}") };
        }
        format!("{} {}",self.first.serialize(None),second.serialize(None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::values::components;
    use rabbithole_css::Tokenizer;

    #[test]
    fn named_axes_are_paired_and_serialized_without_losing_their_direction() {
        let parse = |s| PositionArea::parse(&components(&Tokenizer::tokenize(s)));
        for bad in ["none none","span-center","top bottom","span-inline-end left","start self-end","all"] {
            assert!(parse(bad).is_none(),"{bad}");
        }
        for (text,specified,computed) in [
            ("top LEFT","left top","left top"),
            ("span-inline-start block-start","block-start span-inline-start","start span-start"),
            ("span-all self-end","span-all self-end","self-inline-end"),
            ("block-start span-all","block-start","block-start"),
            ("center center","center","center"),
        ] {
            let value = parse(text).expect(text);
            assert_eq!(value.to_css(false),specified);
            assert_eq!(value.to_css(true),computed);
        }
    }
}

/// One physical axis in the nine-cell position-area grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region { Before, After, SpanBefore, SpanAfter, Center, All }

impl PositionArea {
    pub fn is_none(self) -> bool { self.first.descriptor().0 == "none" }

    pub fn physical(self, containing: (crate::WritingMode,crate::Direction),
                    own: (crate::WritingMode,crate::Direction)) -> Option<[Region;2]> {
        if self.is_none() { return None; }
        let tracks = [self.first, self.second.unwrap_or(self.first)];
        let family = tracks.iter().map(|t| t.descriptor().1).find(|f| *f != Neutral).unwrap_or(Neutral);
        let (wm,dir) = if matches!(family, SelfFlow | SelfLogical) { own } else { containing };
        let vertical = wm.is_vertical();
        let mut result = [Region::All;2];
        for (slot,track) in tracks.iter().enumerate() {
            let (name,_,axis) = track.descriptor();
            if slot == 1 && self.second.is_none() && axis != Either { break; }
            let index = match axis { First=>0, Second=>1, Either=>slot };
            let horizontal = if family == Physical { index == 0 } else { (index == 0) == vertical };
            let physical = usize::from(!horizontal);
            let (wm,dir) = if name.contains("self-") { own } else { (wm,dir) };
            let block_axis = horizontal == wm.is_vertical();
            let reversed = if block_axis { wm == crate::WritingMode::VerticalRl }
                else { (dir == crate::Direction::Rtl) != (wm == crate::WritingMode::SidewaysLr) };
            let before = match name {
                "all" => { result[physical] = Region::All; continue; }
                "center" => { result[physical] = Region::Center; continue; }
                "left" | "top" => true,
                "right" | "bottom" => false,
                _ => name.ends_with("start") != reversed,
            };
            result[physical] = match (track.span,before) {
                (false,true)=>Region::Before, (false,false)=>Region::After,
                (true,true)=>Region::SpanBefore, (true,false)=>Region::SpanAfter,
            };
        }
        Some(result)
    }
}

/// A comma-separated list of dashed identifiers, retaining their case.
pub fn names(parts: &[&[Token]]) -> Option<Vec<String>> {
    if matches!(parts, [v] if crate::values::is_ident(v,"none")) { return Some(Vec::new()); }
    let groups = crate::values::split_commas(parts);
    if groups.is_empty() { return None; }
    groups.iter().map(|group| match group.as_slice() {
        [component] => dashed_ident(component).map(str::to_owned),
        _ => None,
    }).collect()
}

pub fn dashed_ident(tokens: &[Token]) -> Option<&str> {
    match tokens { [Token::Ident(s)] if s.starts_with("--") && s.len()>2 => Some(s), _=>None }
}

/// Names made local to the inclusive flat-tree subtree of an element.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AnchorScope {
    #[default]
    None,
    All,
    Names(std::rc::Rc<[String]>),
}

impl AnchorScope {
    pub fn parse(parts: &[&[Token]]) -> Option<Self> {
        if matches!(parts, [v] if crate::values::is_ident(v, "all")) {
            return Some(Self::All);
        }
        names(parts).map(|names| if names.is_empty() { Self::None } else { Self::Names(names.into()) })
    }

    pub fn includes(&self, name: &str) -> bool {
        match self {
            Self::None => false,
            Self::All => true,
            Self::Names(names) => names.iter().any(|item| item == name),
        }
    }

    pub fn to_css(&self) -> String {
        match self {
            Self::None => "none".into(),
            Self::All => "all".into(),
            Self::Names(names) => names.join(", "),
        }
    }
}

/// A fallback may name a descriptor block and then reflect its values across
/// one or more axes. Reflections are applied in the author's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flip { Block, Inline, Start, X, Y }

impl Flip {
    fn css(self) -> &'static str {
        match self { Self::Block => "flip-block", Self::Inline => "flip-inline", Self::Start => "flip-start",
            Self::X => "flip-x", Self::Y => "flip-y" }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TryOption {
    Rule { name: Option<String>, flips: Vec<Flip> },
    Area(PositionArea),
}

impl TryOption {
    pub fn parse_list(parts: &[&[Token]]) -> Option<Vec<Self>> {
        if matches!(parts, [p] if crate::values::is_ident(p,"none")) { return Some(Vec::new()); }
        if parts.is_empty() { return None; }
        crate::values::split_commas(parts).iter().map(|group| {
            if let Some(area) = PositionArea::parse(group) {
                return (!area.is_none()).then_some(Self::Area(area));
            }
            if group.is_empty() { return None; }
            let mut name = None;
            let mut flips = Vec::new();
            for (i,part) in group.iter().enumerate() {
                if let Some(ident) = dashed_ident(part) {
                    if name.is_some() || (i != 0 && i+1 != group.len()) { return None; }
                    name = Some(ident.to_owned());
                } else {
                    let token = crate::values::ident(part)?;
                    let flip = [Flip::Block, Flip::Inline, Flip::Start, Flip::X, Flip::Y].into_iter().find(|f| f.css() == token)?;
                    if flips.contains(&flip) { return None; }
                    flips.push(flip);
                }
            }
            Some(Self::Rule { name, flips })
        }).collect()
    }

    pub fn list_css(list: &[Self], computed: bool) -> String {
        if list.is_empty() { return "none".into(); }
        list.iter().map(|item| match item {
            Self::Rule { name, flips } => name.as_deref().into_iter().chain(flips.iter().map(|f| f.css())).collect::<Vec<_>>().join(" "),
            Self::Area(a) => a.to_css(computed),
        }).collect::<Vec<_>>().join(", ")
    }
}
