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
