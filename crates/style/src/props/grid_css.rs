//! Specified values of the grid properties for CSSOM (CSS Grid 2): the
//! grammar of each longhand and shorthand, their canonical serialization,
//! shorthand expansion into longhands and the shortest serialization of a
//! shorthand from its longhands.

use crate::color_calc::{self, fmt_number, Ty};
use crate::computed::{GridAutoFlow, GridLine};
use crate::values::{components, ident};
use rabbithole_css::Token;

/// The longhands of a grid shorthand, in canonical order.
pub fn longhands(name: &str) -> Option<&'static [&'static str]> {
    Some(match name {
        "grid-row" => &["grid-row-start", "grid-row-end"],
        "grid-column" => &["grid-column-start", "grid-column-end"],
        "grid-area" => &["grid-row-start", "grid-column-start", "grid-row-end", "grid-column-end"],
        "grid-template" => &["grid-template-rows", "grid-template-columns", "grid-template-areas"],
        "grid" => &["grid-template-rows", "grid-template-columns", "grid-template-areas", "grid-auto-rows", "grid-auto-columns", "grid-auto-flow"],
        "grid-lanes" => &["grid-template-columns", "grid-template-rows", "grid-template-areas", "grid-lanes-direction"],
        _ => return None,
    })
}

pub fn is_longhand(name: &str) -> bool {
    matches!(
        name,
        "grid-row-start" | "grid-row-end" | "grid-column-start" | "grid-column-end" | "grid-template-rows" | "grid-template-columns"
            | "grid-template-areas" | "grid-auto-rows" | "grid-auto-columns" | "grid-auto-flow" | "grid-lanes-direction" | "grid-lanes-pack"
            | "fit-tolerance" | "flow-tolerance"
    )
}

fn is_length_unit(u: &str) -> bool {
    matches!(
        u,
        "px" | "em" | "rem" | "ex" | "rex" | "ch" | "rch" | "ic" | "ric" | "lh" | "rlh" | "cap" | "rcap" | "vw" | "vh" | "vi" | "vb" | "vmin"
            | "vmax" | "svw" | "svh" | "svi" | "svb" | "svmin" | "svmax" | "lvw" | "lvh" | "lvi" | "lvb" | "lvmin" | "lvmax" | "dvw" | "dvh"
            | "dvi" | "dvb" | "dvmin" | "dvmax" | "cqw" | "cqh" | "cqi" | "cqb" | "cqmin" | "cqmax" | "cm" | "mm" | "q" | "in" | "pt" | "pc"
    )
}

/// `<length-percentage [0,∞]>` as specified.
fn length_percentage(c: &[Token]) -> Option<String> {
    match c {
        [Token::Number(n)] if *n == 0.0 => Some("0px".into()),
        [Token::Dimension(n, u)] => {
            let u = u.to_ascii_lowercase();
            (is_length_unit(&u) && *n >= 0.0).then(|| format!("{}{u}", fmt_number(f64::from(*n))))
        }
        [Token::Percentage(p)] => (*p >= 0.0).then(|| format!("{}%", fmt_number(f64::from(*p)))),
        [Token::Function(f), ..] if color_calc::is_math_function(f) => {
            let calc = color_calc::parse_length_percentage(c)?;
            Some(calc.simplify().to_css())
        }
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Breadth {
    Fixed,
    Flex,
    Keyword,
}

fn breadth(c: &[Token]) -> Option<(String, Breadth)> {
    if let Some(k) = ident(c) {
        return matches!(k.as_str(), "auto" | "min-content" | "max-content").then_some((k, Breadth::Keyword));
    }
    if let [Token::Dimension(n, u)] = c {
        if u.eq_ignore_ascii_case("fr") {
            return (*n >= 0.0).then(|| (format!("{}fr", fmt_number(f64::from(*n))), Breadth::Flex));
        }
    }
    length_percentage(c).map(|s| (s, Breadth::Fixed))
}

/// A `<track-size>`: its serialization and whether it is a `<fixed-size>`.
fn track_size(c: &[Token]) -> Option<(String, bool)> {
    if let [Token::Function(f), rest @ ..] = c {
        let name = f.to_ascii_lowercase();
        let inner = match rest {
            [inner @ .., Token::CloseParen] => inner,
            inner => inner,
        };
        let args: Vec<Vec<&[Token]>> = crate::values::split_commas(&components(inner));
        match name.as_str() {
            "minmax" => {
                let [a, b] = args.as_slice() else { return None };
                let ([a], [b]) = (a.as_slice(), b.as_slice()) else { return None };
                let (a, ka) = breadth(a)?;
                let (b, kb) = breadth(b)?;
                if ka == Breadth::Flex {
                    return None;
                }
                return Some((format!("minmax({a}, {b})"), ka == Breadth::Fixed || kb == Breadth::Fixed));
            }
            "fit-content" => {
                let [a] = args.as_slice() else { return None };
                let [a] = a.as_slice() else { return None };
                return Some((format!("fit-content({})", length_percentage(a)?), false));
            }
            _ => {}
        }
    }
    let (s, k) = breadth(c)?;
    Some((s, k == Breadth::Fixed))
}

/// Serialize an identifier (custom-ident) with CSS escaping.
pub fn ident_css(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == '\0' {
            out.push('\u{fffd}');
        } else if c.is_ascii_control() || (c.is_ascii_digit() && (i == 0 || (i == 1 && chars[0] == '-'))) {
            out.push_str(&format!("\\{:x} ", c as u32));
        } else if c == '-' && chars.len() == 1 {
            out.push_str("\\-");
        } else if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c as u32 >= 0x80 {
            out.push(c);
        } else {
            out.push('\\');
            out.push(c);
        }
    }
    out
}

fn custom_ident(c: &[Token], extra_excluded: &[&str]) -> Option<String> {
    let [Token::Ident(s)] = c else { return None };
    let l = s.to_ascii_lowercase();
    if matches!(l.as_str(), "initial" | "inherit" | "unset" | "revert" | "revert-layer" | "default") || extra_excluded.contains(&l.as_str()) {
        return None;
    }
    Some(s.clone())
}

/// `[ <custom-ident>* ]` starting at `comps[i]`: the names and the index
/// after the closing bracket.
fn line_names(comps: &[&[Token]], i: usize) -> Option<(Vec<String>, usize)> {
    if !matches!(comps.get(i), Some([Token::OpenBracket])) {
        return None;
    }
    let mut names = Vec::new();
    let mut j = i + 1;
    loop {
        match comps.get(j)? {
            [Token::CloseBracket] => return Some((names, j + 1)),
            c => names.push(custom_ident(c, &["span", "auto"])?),
        }
        j += 1;
    }
}

fn names_css(names: &[String]) -> String {
    format!("[{}]", names.iter().map(|n| ident_css(n)).collect::<Vec<_>>().join(" "))
}

/// A parsed track list item.
#[derive(Clone, Debug)]
enum Item {
    Names(Vec<String>),
    Size(String, bool),
    Repeat { count: String, items: Vec<Item> },
}

fn items_css(items: &[Item]) -> String {
    let mut out = Vec::new();
    for it in items {
        match it {
            Item::Names(n) if n.is_empty() => {}
            Item::Names(n) => out.push(names_css(n)),
            Item::Size(s, _) => out.push(s.clone()),
            Item::Repeat { count, items, .. } => out.push(format!("repeat({count}, {})", items_css(items))),
        }
    }
    out.join(" ")
}

/// `[ <line-names>? <track-size> ]+ <line-names>?` (inside `repeat()` or
/// an explicit list); no repeat.
fn simple_list(comps: &[&[Token]]) -> Option<Vec<Item>> {
    let mut items = Vec::new();
    let mut i = 0;
    let mut last_names = false;
    let mut sizes = 0;
    while i < comps.len() {
        if let Some((n, next)) = line_names(comps, i) {
            if last_names {
                return None;
            }
            items.push(Item::Names(n));
            last_names = true;
            i = next;
            continue;
        }
        let (s, fixed) = track_size(comps[i])?;
        items.push(Item::Size(s, fixed));
        sizes += 1;
        last_names = false;
        i += 1;
    }
    (sizes > 0).then_some(items)
}

fn positive_integer(c: &[Token]) -> Option<String> {
    match c {
        [Token::Number(n)] if n.fract() == 0.0 && *n >= 1.0 => Some(fmt_number(f64::from(*n))),
        _ => None,
    }
}

/// `none | <track-list> | <auto-track-list> | subgrid <line-name-list>?`
fn track_list(comps: &[&[Token]]) -> Option<String> {
    if let [c] = comps {
        if crate::values::is_ident(c, "none") {
            return Some("none".into());
        }
    }
    if comps.first().is_some_and(|c| crate::values::is_ident(c, "subgrid")) {
        return subgrid(&comps[1..]);
    }
    let mut items = Vec::new();
    let mut i = 0;
    let mut last_names = false;
    let mut tracks = 0;
    let mut auto_repeats = 0;
    let mut all_fixed = true;
    let mut intrinsic_repeat = false;
    while i < comps.len() {
        if let Some((n, next)) = line_names(comps, i) {
            if last_names {
                return None;
            }
            items.push(Item::Names(n));
            last_names = true;
            i = next;
            continue;
        }
        last_names = false;
        let c = comps[i];
        i += 1;
        if let [Token::Function(f), rest @ ..] = c {
            if f.eq_ignore_ascii_case("repeat") {
                let inner = match rest {
                    [inner @ .., Token::CloseParen] => inner,
                    inner => inner,
                };
                let parts = crate::values::split_commas(&components(inner));
                let [count, list] = parts.as_slice() else { return None };
                let [count] = count.as_slice() else { return None };
                let list = simple_list(list)?;
                let (count, auto) = match ident(count).as_deref() {
                    Some(k @ ("auto-fill" | "auto-fit")) => (k.to_string(), true),
                    Some(_) => return None,
                    None => (positive_integer(count)?, false),
                };
                let fixed = list.iter().all(|it| !matches!(it, Item::Size(_, false)));
                if auto {
                    // Intrinsic sizes may repeat automatically when every
                    // other track is fixed (CSS Grid 3); flexible ones not.
                    let flexible = list.iter().any(|it| matches!(it, Item::Size(s, false) if s.ends_with("fr") || s.ends_with("fr)")));
                    if flexible {
                        return None;
                    }
                    auto_repeats += 1;
                    if !fixed {
                        intrinsic_repeat = true;
                    }
                } else if !fixed {
                    all_fixed = false;
                }
                tracks += 1;
                items.push(Item::Repeat { count, items: list });
                continue;
            }
        }
        let (s, fixed) = track_size(c)?;
        all_fixed &= fixed;
        tracks += 1;
        items.push(Item::Size(s, fixed));
    }
    let _ = intrinsic_repeat;
    if tracks == 0 || auto_repeats > 1 || (auto_repeats == 1 && !all_fixed) {
        return None;
    }
    Some(items_css(&items))
}

/// `subgrid <line-name-list>?`
fn subgrid(comps: &[&[Token]]) -> Option<String> {
    let mut out = vec!["subgrid".to_string()];
    let mut i = 0;
    let mut auto_fill = false;
    while i < comps.len() {
        if let Some((n, next)) = line_names(comps, i) {
            out.push(names_css(&n));
            i = next;
            continue;
        }
        let [Token::Function(f), rest @ ..] = comps[i] else { return None };
        if !f.eq_ignore_ascii_case("repeat") {
            return None;
        }
        let inner = match rest {
            [inner @ .., Token::CloseParen] => inner,
            inner => inner,
        };
        let parts = crate::values::split_commas(&components(inner));
        let [count, list] = parts.as_slice() else { return None };
        let [count] = count.as_slice() else { return None };
        let count = if crate::values::is_ident(count, "auto-fill") {
            if auto_fill {
                return None;
            }
            auto_fill = true;
            "auto-fill".to_string()
        } else {
            positive_integer(count)?
        };
        let mut names = Vec::new();
        let mut j = 0;
        while j < list.len() {
            let (n, next) = line_names(list, j)?;
            names.push(names_css(&n));
            j = next;
        }
        if names.is_empty() {
            return None;
        }
        out.push(format!("repeat({count}, {})", names.join(" ")));
        i += 1;
    }
    Some(out.join(" "))
}

/// `<track-size>+`
fn auto_tracks(comps: &[&[Token]]) -> Option<String> {
    if comps.is_empty() {
        return None;
    }
    let sizes: Option<Vec<String>> = comps.iter().map(|c| track_size(c).map(|(s, _)| s)).collect();
    Some(sizes?.join(" "))
}

/// The cells of one `grid-template-areas` row: named cell tokens and null
/// cell tokens (`.` sequences, serialized as one `.`).
fn area_row(s: &str) -> Option<Vec<String>> {
    let mut cells = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '.' {
            while chars.peek() == Some(&'.') {
                chars.next();
            }
            cells.push(".".to_string());
        } else if c.is_alphanumeric() || c == '-' || c == '_' || c as u32 >= 0x80 {
            let mut name = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_alphanumeric() || c == '-' || c == '_' || c as u32 >= 0x80 {
                    name.push(c);
                    chars.next();
                } else {
                    break;
                }
            }
            cells.push(name);
        } else {
            return None;
        }
    }
    (!cells.is_empty()).then_some(cells)
}

/// Named areas must be rectangles.
fn areas_valid(rows: &[Vec<String>]) -> bool {
    let width = rows[0].len();
    if rows.iter().any(|r| r.len() != width) {
        return false;
    }
    let mut seen: Vec<(&str, usize, usize, usize, usize)> = Vec::new();
    for (y, row) in rows.iter().enumerate() {
        for (x, name) in row.iter().enumerate() {
            if name == "." {
                continue;
            }
            match seen.iter_mut().find(|e| e.0 == name) {
                Some(e) => {
                    e.1 = e.1.min(x);
                    e.2 = e.2.max(x);
                    e.3 = e.3.min(y);
                    e.4 = e.4.max(y);
                }
                None => seen.push((name, x, x, y, y)),
            }
        }
    }
    seen.iter().all(|&(name, x0, x1, y0, y1)| (y0..=y1).all(|y| (x0..=x1).all(|x| rows[y][x] == name)))
}

fn areas_css(rows: &[Vec<String>]) -> String {
    rows.iter().map(|r| format!("\"{}\"", r.join(" "))).collect::<Vec<_>>().join(" ")
}

fn template_areas(comps: &[&[Token]]) -> Option<String> {
    if let [c] = comps {
        if crate::values::is_ident(c, "none") {
            return Some("none".into());
        }
    }
    let mut rows = Vec::new();
    for c in comps {
        let [Token::String(s)] = c else { return None };
        rows.push(area_row(s)?);
    }
    (!rows.is_empty() && areas_valid(&rows)).then(|| areas_css(&rows))
}

fn auto_flow(comps: &[&[Token]]) -> Option<String> {
    let mut axis = None;
    let mut dense = false;
    for c in comps {
        match ident(c)?.as_str() {
            "row" | "column" if axis.is_none() => axis = Some(ident(c)?),
            "dense" if !dense => dense = true,
            _ => return None,
        }
    }
    Some(match (axis.as_deref(), dense) {
        (None, false) => return None,
        (Some(a), false) => a.to_string(),
        (Some("column"), true) => "column dense".into(),
        (_, true) => "dense".into(),
    })
}

/// A `<grid-line>` as specified: its serialization and whether it is a
/// lone `<custom-ident>`.
fn grid_line(comps: &[&[Token]]) -> Option<(String, bool)> {
    if let [c] = comps {
        if crate::values::is_ident(c, "auto") {
            return Some(("auto".into(), false));
        }
    }
    let mut span = false;
    let mut num: Option<String> = None;
    let mut name: Option<String> = None;
    for c in comps {
        if crate::values::is_ident(c, "span") {
            if span {
                return None;
            }
            span = true;
            continue;
        }
        if let Some(n) = custom_ident(c, &["span", "auto"]) {
            if name.is_some() {
                return None;
            }
            name = Some(n);
            continue;
        }
        let n = match c {
            [Token::Number(n)] if n.fract() == 0.0 => {
                if *n == 0.0 {
                    return None;
                }
                fmt_number(f64::from(*n))
            }
            [Token::Function(f), ..] if color_calc::is_math_function(f) => {
                let calc = color_calc::parse(c, &[])?;
                if calc.ty()? != Ty::NUMBER {
                    return None;
                }
                calc.simplify().to_css()
            }
            _ => return None,
        };
        if num.is_some() {
            return None;
        }
        num = Some(n);
    }
    if span {
        if let Some(n) = &num {
            if n.starts_with('-') {
                return None;
            }
        }
        let name = name.map(|n| ident_css(&n));
        let css = match (num.as_deref(), name) {
            (None, None) => return None,
            (Some("1"), Some(n)) | (None, Some(n)) => format!("span {n}"),
            (Some(k), Some(n)) => format!("span {k} {n}"),
            (Some(k), None) => format!("span {k}"),
        };
        return Some((css, false));
    }
    match (num, name) {
        (None, Some(n)) => Some((ident_css(&n), true)),
        (Some(k), Some(n)) => Some((format!("{k} {}", ident_css(&n)), false)),
        (Some(k), None) => Some((k, false)),
        (None, None) => None,
    }
}

fn split_slash<'a>(comps: &[&'a [Token]]) -> Vec<Vec<&'a [Token]>> {
    let mut out = vec![Vec::new()];
    for c in comps {
        if matches!(c, [Token::Delim('/')]) {
            out.push(Vec::new());
        } else if let Some(last) = out.last_mut() {
            last.push(*c);
        }
    }
    out
}

/// A specified longhand value; `None` when invalid.
pub fn specified(name: &str, tokens: &[Token]) -> Option<String> {
    let comps = components(tokens);
    match name {
        "grid-template-rows" | "grid-template-columns" => track_list(&comps),
        "grid-template-areas" => template_areas(&comps),
        "grid-auto-rows" | "grid-auto-columns" => auto_tracks(&comps),
        "grid-auto-flow" => auto_flow(&comps),
        "grid-row-start" | "grid-row-end" | "grid-column-start" | "grid-column-end" => grid_line(&comps).map(|(s, _)| s),
        "grid-lanes-direction" => lanes_direction(&comps),
        "grid-lanes-pack" => match comps.as_slice() {
            [c] => ident(c).filter(|k| matches!(k.as_str(), "normal" | "dense")),
            _ => None,
        },
        "fit-tolerance" | "flow-tolerance" => match comps.as_slice() {
            [c] => ident(c).filter(|k| matches!(k.as_str(), "normal" | "infinite")).or_else(|| length_percentage(c)),
            _ => None,
        },
        _ => None,
    }
}

/// `normal | [ row | column ] [ fill-reverse || track-reverse ]?`, as
/// written.
fn lanes_direction(comps: &[&[Token]]) -> Option<String> {
    let words: Vec<String> = comps.iter().map(|c| ident(c)).collect::<Option<_>>()?;
    match words.as_slice() {
        [w] if w == "normal" => return Some("normal".into()),
        [] => return None,
        _ => {}
    }
    if !matches!(words[0].as_str(), "row" | "column") {
        return None;
    }
    let mut seen = Vec::new();
    for w in &words[1..] {
        if !matches!(w.as_str(), "fill-reverse" | "track-reverse") || seen.contains(&w) {
            return None;
        }
        seen.push(w);
    }
    Some(words.join(" "))
}

const LANES_DIRECTION_WORDS: [&str; 5] = ["normal", "row", "column", "fill-reverse", "track-reverse"];

/// `grid-lanes`: <'grid-lanes-direction'>? around a group of an optional
/// area string and a track list, in either order.
fn lanes(comps: &[&[Token]]) -> Option<Vec<(&'static str, String)>> {
    if let [c] = comps {
        if crate::values::is_ident(c, "none") {
            return Some(vec![
                ("grid-template-columns", "none".into()),
                ("grid-template-rows", "none".into()),
                ("grid-template-areas", "none".into()),
                ("grid-lanes-direction", "column".into()),
            ]);
        }
    }
    let is_dir = |c: &&[Token]| ident(c).is_some_and(|k| LANES_DIRECTION_WORDS.contains(&k.as_str()));
    let lead = comps.iter().take_while(|c| is_dir(c)).count();
    let trail = comps[lead..].iter().rev().take_while(|c| is_dir(c)).count();
    if lead > 0 && trail > 0 {
        return None;
    }
    let (dir, rest) = if lead > 0 { (&comps[..lead], &comps[lead..]) } else { (&comps[comps.len() - trail..], &comps[..comps.len() - trail]) };
    let direction = if dir.is_empty() { "column".to_string() } else { lanes_direction(dir)? };
    let strings: Vec<usize> = rest.iter().enumerate().filter(|(_, c)| matches!(c, [Token::String(_)])).map(|(i, _)| i).collect();
    let (area, tracks): (Option<&str>, Vec<&[Token]>) = match strings.as_slice() {
        [] => (None, rest.to_vec()),
        [i] if *i == 0 || *i + 1 == rest.len() => {
            let [Token::String(s)] = rest[*i] else { return None };
            let mut t = rest.to_vec();
            t.remove(*i);
            (Some(s.as_str()), t)
        }
        _ => return None,
    };
    let tracks = if tracks.is_empty() { "none".to_string() } else { track_list(&tracks)? };
    if tracks == "none" && area.is_none() && dir.is_empty() {
        return None;
    }
    let rows_axis = direction.starts_with("row");
    let areas = match area {
        Some(s) => {
            let cells = area_row(s)?;
            if !areas_valid(&[cells.clone()]) {
                return None;
            }
            if rows_axis { areas_css(&cells.into_iter().map(|c| vec![c]).collect::<Vec<_>>()) } else { areas_css(&[cells]) }
        }
        None => "none".into(),
    };
    let (cols, rows) = if rows_axis { ("none".to_string(), tracks) } else { (tracks, "none".to_string()) };
    Some(vec![("grid-template-columns", cols), ("grid-template-rows", rows), ("grid-template-areas", areas), ("grid-lanes-direction", direction)])
}

/// `grid-lanes` from its longhands: the areas along the grid axis, that
/// axis's tracks and the direction.
fn lanes_shorthand(get: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let direction = get("grid-lanes-direction")?;
    let rows_axis = direction.starts_with("row");
    let tracks = get(if rows_axis { "grid-template-rows" } else { "grid-template-columns" })?;
    let areas = get("grid-template-areas")?;
    let mut parts = Vec::new();
    if areas != "none" {
        let tokens = rabbithole_css::Tokenizer::tokenize(&areas);
        let rows: Vec<Vec<String>> = components(&tokens).iter().map(|c| match c {
            [Token::String(s)] => area_row(s),
            _ => None,
        }).collect::<Option<_>>()?;
        let cells: Vec<String> = if rows_axis {
            if rows.iter().any(|r| r.len() != 1) {
                return None;
            }
            rows.into_iter().flatten().collect()
        } else {
            let [row] = rows.as_slice() else { return None };
            row.clone()
        };
        parts.push(format!("\"{}\"", cells.join(" ")));
    }
    if tracks != "none" {
        parts.push(tracks);
    }
    parts.push(direction);
    Some(parts.join(" "))
}

/// Computed `grid-lanes-direction` (`fill-reverse` first).
pub fn lanes_direction_css(d: &crate::computed::GridLanesDirection) -> String {
    let mut s = match d.rows {
        None => return "normal".into(),
        Some(true) => "row".to_string(),
        Some(false) => "column".to_string(),
    };
    if d.fill_reverse {
        s.push_str(" fill-reverse");
    }
    if d.track_reverse {
        s.push_str(" track-reverse");
    }
    s
}

/// The longhand values of a shorthand; `None` when invalid.
pub fn expand(name: &str, tokens: &[Token]) -> Option<Vec<(&'static str, String)>> {
    let comps = components(tokens);
    let parts = split_slash(&comps);
    match name {
        "grid-row" | "grid-column" => {
            if parts.len() > 2 {
                return None;
            }
            let (start, start_ident) = grid_line(&parts[0])?;
            let end = match parts.get(1) {
                Some(p) => grid_line(p)?.0,
                None if start_ident => start.clone(),
                None => "auto".into(),
            };
            let [a, b] = longhands(name)? else { return None };
            Some(vec![(*a, start), (*b, end)])
        }
        "grid-area" => {
            if parts.len() > 4 {
                return None;
            }
            let lines: Vec<(String, bool)> = parts.iter().map(|p| grid_line(p)).collect::<Option<_>>()?;
            let default = |l: &(String, bool)| if l.1 { l.0.clone() } else { "auto".to_string() };
            let rs = lines[0].clone();
            let cs = lines.get(1).cloned().unwrap_or_else(|| (default(&rs), rs.1));
            let re = lines.get(2).map(|l| l.0.clone()).unwrap_or_else(|| default(&rs));
            let ce = lines.get(3).map(|l| l.0.clone()).unwrap_or_else(|| default(&cs));
            Some(vec![("grid-row-start", rs.0), ("grid-column-start", cs.0), ("grid-row-end", re), ("grid-column-end", ce)])
        }
        "grid-lanes" => lanes(&comps),
        "grid-template" => {
            let (rows, cols, areas) = template(&comps)?;
            Some(vec![("grid-template-rows", rows), ("grid-template-columns", cols), ("grid-template-areas", areas)])
        }
        "grid" => {
            if let Some((rows, cols, areas)) = template(&comps) {
                return Some(vec![
                    ("grid-template-rows", rows),
                    ("grid-template-columns", cols),
                    ("grid-template-areas", areas),
                    ("grid-auto-rows", "auto".into()),
                    ("grid-auto-columns", "auto".into()),
                    ("grid-auto-flow", "row".into()),
                ]);
            }
            let [first, second] = parts.as_slice() else { return None };
            let flow = |p: &[&[Token]]| -> Option<(bool, usize)> {
                // `auto-flow && dense?` then the remaining components.
                let mut auto = false;
                let mut dense = false;
                let mut n = 0;
                for c in p {
                    match ident(c).as_deref() {
                        Some("auto-flow") if !auto => auto = true,
                        Some("dense") if !dense => dense = true,
                        _ => break,
                    }
                    n += 1;
                }
                auto.then_some((dense, n))
            };
            if let Some((dense, n)) = flow(first) {
                let auto_rows = if n == first.len() { "auto".to_string() } else { auto_tracks(&first[n..])? };
                let cols = track_list(second)?;
                return Some(vec![
                    ("grid-template-rows", "none".into()),
                    ("grid-template-columns", cols),
                    ("grid-template-areas", "none".into()),
                    ("grid-auto-rows", auto_rows),
                    ("grid-auto-columns", "auto".into()),
                    ("grid-auto-flow", if dense { "dense" } else { "row" }.into()),
                ]);
            }
            let (dense, n) = flow(second)?;
            let auto_cols = if n == second.len() { "auto".to_string() } else { auto_tracks(&second[n..])? };
            let rows = track_list(first)?;
            Some(vec![
                ("grid-template-rows", rows),
                ("grid-template-columns", "none".into()),
                ("grid-template-areas", "none".into()),
                ("grid-auto-rows", "auto".into()),
                ("grid-auto-columns", auto_cols),
                ("grid-auto-flow", if dense { "column dense" } else { "column" }.into()),
            ])
        }
        _ => None,
    }
}

/// `grid-template`: (rows, columns, areas).
fn template(comps: &[&[Token]]) -> Option<(String, String, String)> {
    if let [c] = comps {
        if crate::values::is_ident(c, "none") {
            return Some(("none".into(), "none".into(), "none".into()));
        }
    }
    let parts = split_slash(comps);
    let has_strings = parts[0].iter().any(|c| matches!(c, [Token::String(_)]));
    if !has_strings {
        let [rows, cols] = parts.as_slice() else { return None };
        return Some((track_list(rows)?, track_list(cols)?, "none".into()));
    }
    if parts.len() > 2 {
        return None;
    }
    // [ <line-names>? <string> <track-size>? <line-names>? ]+
    let rows_part = &parts[0];
    let mut areas = Vec::new();
    let mut out: Vec<String> = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    let mut i = 0;
    while i < rows_part.len() {
        if let Some((n, next)) = line_names(rows_part, i) {
            // Names before a string: those after the previous row merge.
            pending.extend(n);
            i = next;
            if !matches!(rows_part.get(i), Some([Token::String(_)])) {
                return None;
            }
        }
        let [Token::String(s)] = rows_part.get(i)? else { return None };
        if !pending.is_empty() {
            out.push(names_css(&pending));
            pending.clear();
        }
        areas.push(area_row(s)?);
        i += 1;
        let mut size = "auto".to_string();
        if let Some(c) = rows_part.get(i) {
            if !matches!(c, [Token::String(_)] | [Token::OpenBracket]) {
                let ([Token::Function(_), ..] | [Token::Dimension(..)] | [Token::Percentage(_)] | [Token::Number(_)] | [Token::Ident(_)]) = c else {
                    return None;
                };
                if let [Token::Function(f), ..] = c {
                    if f.eq_ignore_ascii_case("repeat") {
                        return None;
                    }
                }
                size = track_size(c)?.0;
                i += 1;
            }
        }
        out.push(size);
        if let Some((n, next)) = line_names(rows_part, i) {
            pending.extend(n);
            i = next;
            if i < rows_part.len() && !matches!(rows_part[i], [Token::String(_)] | [Token::OpenBracket]) {
                return None;
            }
        }
    }
    if !pending.is_empty() {
        out.push(names_css(&pending));
    }
    if areas.is_empty() || !areas_valid(&areas) {
        return None;
    }
    let cols = match parts.get(1) {
        Some(p) => {
            let list = simple_list(p)?;
            items_css(&list)
        }
        None => "none".into(),
    };
    Some((out.join(" "), cols, areas_css(&areas)))
}

/// The shortest serialization of a shorthand from its longhand values;
/// `None` when they cannot be represented by it.
pub fn shorthand(name: &str, get: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    // A lone <custom-ident> is a default for the omitted lines.
    let is_ident_line = |v: &str| {
        !v.contains(' ') && v != "auto" && !v.starts_with("calc(") && v.parse::<f64>().is_err() && !v.starts_with(|c: char| c.is_ascii_digit())
    };
    let default_of = |v: &str| if is_ident_line(v) { v.to_string() } else { "auto".to_string() };
    match name {
        "grid-row" | "grid-column" => {
            let [a, b] = longhands(name)? else { return None };
            let (start, end) = (get(a)?, get(b)?);
            if end == default_of(&start) { Some(start) } else { Some(format!("{start} / {end}")) }
        }
        "grid-area" => {
            let (rs, cs, re, ce) = (get("grid-row-start")?, get("grid-column-start")?, get("grid-row-end")?, get("grid-column-end")?);
            let mut parts = vec![rs.clone(), cs.clone(), re.clone(), ce.clone()];
            if ce == default_of(&cs) {
                parts.pop();
                if re == default_of(&rs) {
                    parts.pop();
                    if cs == default_of(&rs) {
                        parts.pop();
                    }
                }
            }
            Some(parts.join(" / "))
        }
        "grid-template" => template_shorthand(&get("grid-template-rows")?, &get("grid-template-columns")?, &get("grid-template-areas")?),
        "grid-lanes" => lanes_shorthand(get),
        "grid" => {
            let (rows, cols, areas) = (get("grid-template-rows")?, get("grid-template-columns")?, get("grid-template-areas")?);
            let (auto_rows, auto_cols, flow) = (get("grid-auto-rows")?, get("grid-auto-columns")?, get("grid-auto-flow")?);
            if auto_rows == "auto" && auto_cols == "auto" && flow == "row" {
                return template_shorthand(&rows, &cols, &areas);
            }
            if areas != "none" {
                return None;
            }
            let dense = flow.contains("dense");
            if matches!(flow.as_str(), "column" | "column dense") && auto_rows == "auto" && cols == "none" {
                let mut s = format!("{rows} / auto-flow");
                if dense {
                    s.push_str(" dense");
                }
                if auto_cols != "auto" {
                    s.push(' ');
                    s.push_str(&auto_cols);
                }
                return Some(s);
            }
            if matches!(flow.as_str(), "row" | "dense") && auto_cols == "auto" && rows == "none" {
                let mut s = "auto-flow".to_string();
                if dense {
                    s.push_str(" dense");
                }
                if auto_rows != "auto" {
                    s.push(' ');
                    s.push_str(&auto_rows);
                }
                s.push_str(" / ");
                s.push_str(&cols);
                return Some(s);
            }
            None
        }
        _ => None,
    }
}

fn template_shorthand(rows: &str, cols: &str, areas: &str) -> Option<String> {
    if areas == "none" {
        if rows == "none" && cols == "none" {
            return Some("none".into());
        }
        return Some(format!("{rows} / {cols}"));
    }
    // The areas form: each row's string with its size and line names.
    let area_rows: Vec<String> = {
        let mut v = Vec::new();
        let mut rest = areas;
        while let Some(start) = rest.find('"') {
            let end = rest[start + 1..].find('"')? + start + 1;
            v.push(rest[start..=end].to_string());
            rest = &rest[end + 1..];
        }
        v
    };
    if rows.contains("repeat(") || cols.contains("repeat(") || rows == "none" || rows.starts_with("subgrid") {
        return None;
    }
    let tokens = rabbithole_css::Tokenizer::tokenize(rows);
    let comps = components(&tokens);
    let mut out: Vec<String> = Vec::new();
    let mut row = 0;
    let mut i = 0;
    while i < comps.len() {
        if let Some((n, next)) = line_names(&comps, i) {
            out.push(names_css(&n));
            i = next;
            continue;
        }
        let size = track_size(comps[i])?.0;
        out.push(area_rows.get(row)?.clone());
        if size != "auto" {
            out.push(size);
        }
        row += 1;
        i += 1;
    }
    if row != area_rows.len() {
        return None;
    }
    let mut s = out.join(" ");
    if cols != "none" {
        s.push_str(" / ");
        s.push_str(cols);
    }
    Some(s)
}

/// Computed `grid-*-start` / `grid-*-end`.
pub fn line_css(l: &GridLine) -> String {
    match l {
        GridLine::Auto => "auto".into(),
        GridLine::Line(n) => n.to_string(),
        GridLine::Span(n) => format!("span {n}"),
        GridLine::Named(name, false) => ident_css(name),
        GridLine::Named(name, true) => format!("span {}", ident_css(name)),
        GridLine::NamedNth(name, n, false) => format!("{n} {}", ident_css(name)),
        GridLine::NamedNth(name, n, true) if *n == 1 => format!("span {}", ident_css(name)),
        GridLine::NamedNth(name, n, true) => format!("span {n} {}", ident_css(name)),
    }
}

pub fn auto_flow_css(f: GridAutoFlow) -> &'static str {
    match f {
        GridAutoFlow::Row => "row",
        GridAutoFlow::Column => "column",
        GridAutoFlow::RowDense => "dense",
        GridAutoFlow::ColumnDense => "column dense",
    }
}

/// Computed `grid-template-areas`.
pub fn areas_computed(rows: &[Vec<String>]) -> String {
    if rows.is_empty() {
        return "none".into();
    }
    let rows: Vec<Vec<String>> = rows.iter().map(|r| r.iter().map(|c| if c.chars().all(|ch| ch == '.') { ".".to_string() } else { c.clone() }).collect()).collect();
    areas_css(&rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rabbithole_css::Tokenizer;

    fn spec(name: &str, v: &str) -> Option<String> {
        specified(name, &Tokenizer::tokenize(v))
    }

    #[test]
    fn longhands() {
        assert_eq!(spec("grid-template-columns", "repeat(1, [] 10px [])").as_deref(), Some("repeat(1, 10px)"));
        assert_eq!(spec("grid-template-columns", "[] 150px [] 1fr []").as_deref(), Some("150px 1fr"));
        assert_eq!(spec("grid-template-columns", "calc(-0.5em + 10px)").as_deref(), Some("calc(-0.5em + 10px)"));
        assert_eq!(spec("grid-template-columns", "calc(30% + 40vw)").as_deref(), Some("calc(30% + 40vw)"));
        assert!(spec("grid-template-columns", "-10px").is_none());
        assert!(spec("grid-template-columns", "[one] 10px [two] [three]").is_none());
        assert!(spec("grid-template-columns", "auto repeat(auto-fill, auto) auto").is_none());
        assert!(spec("grid-template-columns", "repeat(auto-fill, 10px) repeat(auto-fit, 20%)").is_none());
        assert_eq!(spec("grid-template-areas", "\"first ...\"").as_deref(), Some("\"first .\""));
        assert!(spec("grid-template-areas", "\"a b\" \"b a\"").is_none());
        assert_eq!(spec("grid-auto-flow", "dense row").as_deref(), Some("dense"));
        assert_eq!(spec("grid-row-start", "i 2 SpAn").as_deref(), Some("span 2 i"));
        assert_eq!(spec("grid-row-start", "span 1 i").as_deref(), Some("span i"));
        assert_eq!(spec("grid-row-start", "+90 -a-").as_deref(), Some("90 -a-"));
        assert_eq!(spec("grid-row-start", "span min(-1, 6)").as_deref(), Some("span calc(-1)"));
    }

    fn sh(name: &str, v: &str) -> Vec<(&'static str, String)> {
        expand(name, &Tokenizer::tokenize(v)).unwrap()
    }

    #[test]
    fn shorthands() {
        let t = sh("grid", "[header-top] \"a a a\" [header-bottom] [main-top] \"b b b\" 1fr [main-bottom] / auto 1fr auto");
        assert_eq!(t[0].1, "[header-top] auto [header-bottom main-top] 1fr [main-bottom]");
        assert_eq!(t[2].1, "\"a a a\" \"b b b\"");
        let t = sh("grid", "auto-flow dense 30px / 40px");
        assert_eq!((t[3].1.as_str(), t[5].1.as_str()), ("30px", "dense"));
        let a = sh("grid-area", "a / b / c");
        assert_eq!(a[3].1, "b");
        let get = |m: Vec<(&'static str, String)>| move |k: &str| m.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
        assert_eq!(shorthand("grid-area", &get(sh("grid-area", "1 / i / auto / i"))).as_deref(), Some("1 / i"));
        assert_eq!(shorthand("grid", &get(sh("grid", "\"a\" [a] [b] \"b\""))).as_deref(), Some("\"a\" [a b] \"b\""));
        assert_eq!(shorthand("grid", &get(sh("grid", "dense auto-flow 1fr / 100px"))).as_deref(), Some("auto-flow dense 1fr / 100px"));
        assert!(expand("grid-template", &Tokenizer::tokenize("\"a\" [a] 10px")).is_none());
    }
}
