//! Computed box alignment retains overflow, baseline and physical positions.
use rabbithole_style::{AlignFlags, ComputedStyle, ContentDistribution, ItemAlign};

fn keyword(base: &str, flags: AlignFlags) -> String {
    let value = if flags.has(AlignFlags::BASELINE) {
        if flags.has(AlignFlags::LAST) { "last baseline" } else { "baseline" }
    } else if flags.has(AlignFlags::ANCHOR_CENTER) {
        "anchor-center"
    } else if flags.has(AlignFlags::NORMAL) {
        "normal"
    } else if flags.has(AlignFlags::LEFT) {
        "left"
    } else if flags.has(AlignFlags::RIGHT) {
        "right"
    } else if flags.has(AlignFlags::FLEX) {
        if base == "start" { "flow-start" } else { "flow-end" }
    } else if flags.has(AlignFlags::SELF) {
        if base == "start" { "self-start" } else { "self-end" }
    } else {
        base
    };
    let modifier = if flags.has(AlignFlags::LEGACY) { "legacy " }
        else if flags.has(AlignFlags::SAFE) { "safe " }
        else if flags.has(AlignFlags::UNSAFE) { "unsafe " }
        else { "" };
    format!("{modifier}{value}")
}

fn item(value: ItemAlign, flags: AlignFlags) -> String {
    keyword(match value {
        ItemAlign::Auto => "auto",
        ItemAlign::Normal => "normal",
        ItemAlign::Stretch => "stretch",
        ItemAlign::FlexStart => "start",
        ItemAlign::FlexEnd => "end",
        ItemAlign::Center => "center",
        ItemAlign::Baseline => "baseline",
    }, flags)
}

fn content(value: ContentDistribution, flags: AlignFlags) -> String {
    keyword(match value {
        ContentDistribution::FlexStart => "start",
        ContentDistribution::FlexEnd => "end",
        ContentDistribution::Center => "center",
        ContentDistribution::SpaceBetween => "space-between",
        ContentDistribution::SpaceAround => "space-around",
        ContentDistribution::SpaceEvenly => "space-evenly",
        ContentDistribution::Stretch => "stretch",
    }, flags)
}

pub(super) fn computed(s: &ComputedStyle, property: &str) -> Option<String> {
    let pair = |a: String, b: String| if a == b { a } else { format!("{a} {b}") };
    Some(match property {
        "align-content" => content(s.align_content, s.align_content_flags),
        "justify-content" => content(s.justify_content, s.justify_content_flags),
        "align-items" => item(s.align_items, s.align_items_flags),
        "justify-items" => item(s.justify_items, s.justify_items_flags),
        "align-self" => item(s.align_self, s.align_self_flags),
        "justify-self" => item(s.justify_self, s.justify_self_flags),
        "place-content" => pair(content(s.align_content, s.align_content_flags), content(s.justify_content, s.justify_content_flags)),
        "place-items" => pair(item(s.align_items, s.align_items_flags), item(s.justify_items, s.justify_items_flags)),
        "place-self" => pair(item(s.align_self, s.align_self_flags), item(s.justify_self, s.justify_self_flags)),
        _ => return None,
    })
}
