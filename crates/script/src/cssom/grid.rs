//! Grid2 §7.2.6: used tracks for boxes, computed syntax for no-box values.
use rabbithole_style::{SubgridLineNames, TrackCalc, TrackListItem, TrackSize, UsedGridTrackList};

pub(super) fn axis(property: &str) -> Option<bool> {
    match property { "grid-template-columns" => Some(false), "grid-template-rows" => Some(true), _ => None }
}

fn ident(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == '\0' { out.push('\u{fffd}'); }
        else if c.is_ascii_control() || (c.is_ascii_digit() && (i == 0 || (i == 1 && chars[0] == '-'))) {
            out.push_str(&format!("\\{:x} ", c as u32));
        } else if c == '-' && chars.len() == 1 { out.push_str("\\-"); }
        else if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c as u32 >= 0x80 { out.push(c); }
        else { out.push('\\'); out.push(c); }
    }
    out
}

fn names(line: &[String]) -> String {
    let mut seen = std::collections::HashSet::new();
    format!("[{}]", line.iter().filter(|name| seen.insert(name.as_str())).map(|name| ident(name)).collect::<Vec<_>>().join(" "))
}

pub(super) fn used(value: &UsedGridTrackList) -> String {
    match value {
        UsedGridTrackList::Standalone { sizes, line_names } => {
            if sizes.is_empty() { return "none".into(); }
            let mut out = Vec::new();
            for (i, &size) in sizes.iter().enumerate() {
                if let Some(line) = line_names.get(i).filter(|n| !n.is_empty()) { out.push(names(line)); }
                out.push(super::px(size));
            }
            if let Some(line) = line_names.get(sizes.len()).filter(|n| !n.is_empty()) { out.push(names(line)); }
            out.join(" ")
        }
        UsedGridTrackList::Subgrid { line_names } => {
            let mut out = vec!["subgrid".into()];
            out.extend(line_names.iter().map(|line| names(line)));
            out.join(" ")
        }
    }
}

fn size(value: &TrackSize) -> String {
    match value {
        TrackSize::Length(l) => super::position_css(l),
        TrackSize::Fr(f) => format!("{}fr", super::fmt_num(*f)),
        TrackSize::MinMax(min, max) => format!("minmax({}, {})", size(min), size(max)),
        TrackSize::FitContent(l) => format!("fit-content({})", super::position_css(l)),
        TrackSize::Calc(op, terms) => {
            let terms: Vec<_> = terms.iter().map(|&(px, pct)| rabbithole_style::MathNode::Leaf(px, pct).to_css()).collect();
            match op {
                TrackCalc::Sum => terms.first().cloned().unwrap_or_else(|| "0px".into()),
                _ => format!("{}({})", match op { TrackCalc::Min => "min", TrackCalc::Max => "max", _ => "clamp" }, terms.join(", ")),
            }
        }
    }
}

/// Computed `grid-auto-columns` / `grid-auto-rows`.
pub(super) fn auto_tracks(list: &[TrackSize]) -> String {
    if list.is_empty() { return "auto".into(); }
    list.iter().map(size).collect::<Vec<_>>().join(" ")
}

pub(super) fn computed(list: &[TrackListItem]) -> String {
    let mut out = Vec::new();
    let mut pending = Vec::new();
    let flush = |pending: &mut Vec<String>, out: &mut Vec<String>| {
        if !pending.is_empty() { out.push(names(pending)); pending.clear(); }
    };
    for part in list {
        match part {
            TrackListItem::Track { names: pre, size: value } => {
                pending.extend(pre.iter().cloned()); flush(&mut pending, &mut out); out.push(size(value));
            }
            TrackListItem::Names(line) => pending.extend(line.iter().cloned()),
            TrackListItem::Repeat { count, auto_fit, tracks } => {
                flush(&mut pending, &mut out);
                let count = count.map(|n| n.to_string()).unwrap_or_else(|| if *auto_fit {"auto-fit"} else {"auto-fill"}.into());
                out.push(format!("repeat({count}, {})", computed(tracks)));
            }
            TrackListItem::Subgrid { line_names } => {
                flush(&mut pending, &mut out); out.push("subgrid".into());
                for part in line_names {
                    out.push(match part {
                        SubgridLineNames::Names(line) => names(line),
                        SubgridLineNames::Repeat { count, names: lines } => format!("repeat({}, {})", count.map(|n| n.to_string()).unwrap_or_else(|| "auto-fill".into()), lines.iter().map(|line| names(line)).collect::<Vec<_>>().join(" ")),
                    });
                }
            }
        }
    }
    flush(&mut pending, &mut out);
    if out.is_empty() { "none".into() } else { out.join(" ") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rabbithole_style::Length;
    #[test]
    fn used_tracks_keep_zero_tracks_and_serialize_identifier_sets() {
        let value = UsedGridTrackList::Standalone { sizes:vec![0., 23.5], line_names:vec![vec!["1st".into(), "1st".into(), "-1x".into()], vec!["a b".into()], vec!["tail".into()]] };
        assert_eq!(used(&value), r"[\31 st -\31 x] 0px [a\ b] 23.5px [tail]");
    }
    #[test]
    fn resolved_subgrid_keeps_empty_trailing_local_name_sets() {
        assert_eq!(used(&UsedGridTrackList::Subgrid { line_names:vec![vec![],vec!["local".into()],vec![]] }), "subgrid [] [local] []");
    }
    #[test]
    fn no_box_values_keep_repeat_and_percentage_syntax() {
        let list = vec![TrackListItem::Repeat { count:None, auto_fit:true, tracks:vec![TrackListItem::Track { names:vec!["y".into()], size:TrackSize::MinMax(Box::new(TrackSize::Length(Length::Percent(20.))), Box::new(TrackSize::Fr(1.))) }] }];
        assert_eq!(computed(&list), "repeat(auto-fit, [y] minmax(20%, 1fr))");
        assert_eq!(computed(&[]), "none");
    }
}
