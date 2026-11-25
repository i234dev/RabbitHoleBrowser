//! CSS Grid properties: track lists, areas, placement.

use super::ApplyCtx;
use crate::computed::*;
use crate::values::{self, LengthContext};
use rabbithole_css::Token;

/// Token cursor that skips whitespace.
struct Cur<'a> {
    toks: &'a [Token],
    pos: usize,
}

impl<'a> Cur<'a> {
    fn new(toks: &'a [Token]) -> Self {
        Cur { toks, pos: 0 }
    }
    fn skip_ws(&mut self) {
        while matches!(self.toks.get(self.pos), Some(Token::Whitespace)) {
            self.pos += 1;
        }
    }
    fn peek(&mut self) -> Option<&'a Token> {
        self.skip_ws();
        self.toks.get(self.pos)
    }
    fn bump(&mut self) -> Option<&'a Token> {
        self.skip_ws();
        let t = self.toks.get(self.pos);
        self.pos += 1;
        t
    }
    /// The tokens of a function call starting at the current position
    /// (function token through its matching close paren).
    fn function(&mut self) -> Option<&'a [Token]> {
        self.skip_ws();
        let start = self.pos;
        if !matches!(self.toks.get(start), Some(Token::Function(_))) {
            return None;
        }
        let mut depth = 0i32;
        let mut i = start;
        while i < self.toks.len() {
            match self.toks[i] {
                Token::Function(_) | Token::OpenParen => depth += 1,
                Token::CloseParen => {
                    depth -= 1;
                    if depth == 0 {
                        i += 1;
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        self.pos = i;
        Some(&self.toks[start..i])
    }
    fn done(&mut self) -> bool {
        self.peek().is_none()
    }
}

fn track_breadth(tok: &Token, ctx: &LengthContext) -> Option<TrackSize> {
    match tok {
        Token::Ident(i) => Some(match i.to_ascii_lowercase().as_str() {
            "auto" => TrackSize::Length(Length::Auto),
            "min-content" => TrackSize::Length(Length::MinContent),
            "max-content" => TrackSize::Length(Length::MaxContent),
            _ => return None,
        }),
        Token::Dimension(v, u) if u == "fr" => Some(TrackSize::Fr(v.max(0.0))),
        t => {
            let l = values::parse_length(std::slice::from_ref(t))?;
            if l.is_auto() {
                return None;
            }
            Some(TrackSize::Length(l.resolve(ctx)))
        }
    }
}

/// A track breadth written as one component: a token or a math function
/// (`calc()`, `min()`, `max()`, `clamp()`).
fn track_breadth_comp(c: &[Token], ctx: &LengthContext) -> Option<TrackSize> {
    match c {
        [t] => track_breadth(t, ctx),
        [Token::Function(f), ..] if matches!(f.to_ascii_lowercase().as_str(), "calc" | "-webkit-calc" | "min" | "max" | "clamp") => {
            let l = values::parse_length(c)?;
            if let Some((op, terms)) = l.mixed_terms(ctx) {
                return Some(TrackSize::Calc(op, terms));
            }
            match l.resolve(ctx) {
                Length::Auto => None,
                l => Some(TrackSize::Length(l)),
            }
        }
        _ => None,
    }
}

/// Parse one track size at the cursor (breadth, `minmax()`, `fit-content()`).
fn track_size(cur: &mut Cur, ctx: &LengthContext) -> Option<TrackSize> {
    if let Some(Token::Function(f)) = cur.peek() {
        let lf = f.to_ascii_lowercase();
        let call = cur.function()?;
        let args = super::fn_args(call);
        return match lf.as_str() {
            "minmax" if args.len() == 2 && args[0].len() == 1 && args[1].len() == 1 => {
                let a = track_breadth_comp(args[0][0], ctx)?;
                let b = track_breadth_comp(args[1][0], ctx)?;
                Some(TrackSize::MinMax(Box::new(a), Box::new(b)))
            }
            "calc" | "-webkit-calc" | "min" | "max" | "clamp" => track_breadth_comp(call, ctx),
            "fit-content" if args.len() == 1 && args[0].len() == 1 => {
                let l = super::len_no_auto(args[0][0], ctx)?;
                Some(TrackSize::FitContent(l))
            }
            _ => None,
        };
    }
    let t = cur.bump()?;
    track_breadth(t, ctx)
}

/// `[name1 name2]` line names at the cursor.
fn line_names(cur: &mut Cur) -> Option<Vec<String>> {
    if cur.peek() != Some(&Token::OpenBracket) {
        return None;
    }
    cur.bump();
    let mut names = Vec::new();
    loop {
        match cur.bump() {
            Some(Token::Ident(n)) => names.push(n.clone()),
            Some(Token::CloseBracket) | None => break,
            _ => {}
        }
    }
    Some(names)
}

/// Unlike a standalone track list, adjacent bracket groups are distinct
/// subgrid lines; name repeats cannot contain sizes or nested repeats.
fn subgrid_line_names(cur: &mut Cur) -> Option<Vec<String>> {
    if cur.bump()? != &Token::OpenBracket { return None; }
    let mut names = Vec::new();
    loop {
        match cur.bump()? {
            Token::Ident(name) if !matches!(name.to_ascii_lowercase().as_str(), "auto" | "span" | "initial" | "inherit" | "unset" | "revert" | "revert-layer" | "default") => names.push(name.clone()),
            Token::CloseBracket => return Some(names),
            _ => return None,
        }
    }
}

fn subgrid_names(cur: &mut Cur) -> Option<Vec<SubgridLineNames>> {
    let mut result = Vec::new();
    let mut auto_fill = false;
    while let Some(token) = cur.peek() {
        match token {
            Token::OpenBracket => result.push(SubgridLineNames::Names(subgrid_line_names(cur)?)),
            Token::Function(name) if name.eq_ignore_ascii_case("repeat") => {
                let call = cur.function()?;
                if call.last() != Some(&Token::CloseParen) { return None; }
                let mut args = Cur::new(&call[1..call.len()-1]);
                let count = match args.bump()? {
                    Token::Number(n) if n.is_finite() && *n >= 1. && n.fract() == 0. && *n < u32::MAX as f32 => Some(*n as u32),
                    Token::Ident(name) if name.eq_ignore_ascii_case("auto-fill") && !auto_fill => {
                        auto_fill = true;
                        None
                    }
                    _ => return None,
                };
                if args.bump()? != &Token::Comma { return None; }
                let mut names = Vec::new();
                while !args.done() { names.push(subgrid_line_names(&mut args)?); }
                if names.is_empty() { return None; }
                result.push(SubgridLineNames::Repeat { count, names });
            }
            _ => return None,
        }
    }
    Some(result)
}

/// Subgrid has its own line-name grammar, independent of lengths or layout.
pub(crate) fn valid_subgrid(toks: &[Token]) -> Option<bool> {
    let mut cur = Cur::new(toks);
    match cur.bump()? {
        Token::Ident(name) if name.eq_ignore_ascii_case("subgrid") => Some(subgrid_names(&mut cur).is_some()),
        _ => None,
    }
}

/// Parse a full track list (`grid-template-columns` value).
pub fn parse_track_list(toks: &[Token], ctx: &LengthContext) -> Option<Vec<TrackListItem>> {
    let mut cur = Cur::new(toks);
    if let Some(Token::Ident(i)) = cur.peek() {
        if i.eq_ignore_ascii_case("none") {
            return Some(Vec::new());
        }
        if i.eq_ignore_ascii_case("subgrid") {
            cur.bump();
            return Some(vec![TrackListItem::Subgrid { line_names: subgrid_names(&mut cur)? }]);
        }
        if i.eq_ignore_ascii_case("masonry") {
            return Some(Vec::new());
        }
    }
    let items = parse_items(&mut cur, ctx)?;
    if !cur.done() || !valid_intrinsic_repeat(&items) {
        return None;
    }
    Some(items)
}

/// `<fixed-size>` (Grid2 §7.2): a length-percentage in at least one bound,
/// and no flexible minimum.
fn is_fixed_size(size: &TrackSize) -> bool {
    let breadth = |s: &TrackSize| matches!(s, TrackSize::Length(Length::Px(_) | Length::Percent(_) | Length::Calc(..) | Length::Math(_)) | TrackSize::Calc(..));
    match size {
        TrackSize::MinMax(min, max) => breadth(min) || (breadth(max) && !matches!(**min, TrackSize::Fr(_))),
        other => breadth(other),
    }
}

/// An automatic repetition of intrinsic sizes (allowed in grid lanes,
/// Grid3 §3.2.1) requires every other track to have a fixed size.
fn valid_intrinsic_repeat(items: &[TrackListItem]) -> bool {
    let fixed = |list: &[TrackListItem]| list.iter().all(|t| match t {
        TrackListItem::Track { size, .. } => is_fixed_size(size),
        _ => true,
    });
    let intrinsic = items.iter().any(|it| matches!(it, TrackListItem::Repeat { count: None, tracks, .. } if !fixed(tracks)));
    !intrinsic
        || items.iter().all(|it| match it {
            TrackListItem::Track { size, .. } => is_fixed_size(size),
            TrackListItem::Repeat { count: Some(_), tracks, .. } => fixed(tracks),
            _ => true,
        })
}

fn parse_items(cur: &mut Cur, ctx: &LengthContext) -> Option<Vec<TrackListItem>> {
    let mut out = Vec::new();
    let mut pending_names: Vec<String> = Vec::new();
    while !cur.done() {
        if let Some(names) = line_names(cur) {
            pending_names.extend(names);
            continue;
        }
        if let Some(Token::Function(f)) = cur.peek() {
            if f.eq_ignore_ascii_case("repeat") {
                let call = cur.function()?;
                let inner = &call[1..call.len().saturating_sub(1)];
                // split at the first top-level comma
                let comma = inner.iter().position(|t| *t == Token::Comma)?;
                let count_toks = &inner[..comma];
                let list_toks = &inner[comma + 1..];
                let mut cc = Cur::new(count_toks);
                let (count, auto_fit) = match cc.bump()? {
                    Token::Number(n) => (Some((*n).max(1.0) as u32), false),
                    Token::Ident(i) if i.eq_ignore_ascii_case("auto-fill") => (None, false),
                    Token::Ident(i) if i.eq_ignore_ascii_case("auto-fit") => (None, true),
                    _ => return None,
                };
                let mut lc = Cur::new(list_toks);
                let tracks = parse_items(&mut lc, ctx)?;
                if !pending_names.is_empty() {
                    out.push(TrackListItem::Names(std::mem::take(&mut pending_names)));
                }
                out.push(TrackListItem::Repeat { count, auto_fit, tracks });
                continue;
            }
        }
        let size = track_size(cur, ctx)?;
        out.push(TrackListItem::Track { names: std::mem::take(&mut pending_names), size });
    }
    if !pending_names.is_empty() {
        out.push(TrackListItem::Names(pending_names));
    }
    Some(out)
}

/// Parse a space-separated list of track sizes (`grid-auto-rows`).
fn parse_track_sizes(toks: &[Token], ctx: &LengthContext) -> Option<Vec<TrackSize>> {
    let mut cur = Cur::new(toks);
    let mut out = Vec::new();
    while !cur.done() {
        out.push(track_size(&mut cur, ctx)?);
    }
    Some(out)
}

fn parse_areas(comps: &[&[Token]]) -> Option<Vec<Vec<String>>> {
    if comps.len() == 1 && values::is_ident(comps[0], "none") {
        return Some(Vec::new());
    }
    let mut rows = Vec::new();
    for c in comps {
        match c {
            [Token::String(s)] => rows.push(s.split_whitespace().map(|x| x.to_string()).collect()),
            _ => return None,
        }
    }
    Some(rows)
}

/// `<grid-line>`: auto | <integer> [name] | span <n|name> | name
fn parse_grid_line(comps: &[&[Token]]) -> Option<GridLine> {
    if comps.len() == 1 && values::is_ident(comps[0], "auto") {
        return Some(GridLine::Auto);
    }
    let mut span = false;
    let mut num: Option<i32> = None;
    let mut name: Option<String> = None;
    for c in comps {
        // Custom identifiers (area / line names) are case-sensitive; only
        // the keywords are matched case-insensitively.
        if let [Token::Ident(raw)] = c {
            match raw.to_ascii_lowercase().as_str() {
                "span" if !span => span = true,
                "auto" | "span" | "initial" | "inherit" | "unset" | "revert" | "revert-layer" | "default" => return None,
                _ if name.is_none() => name = Some(raw.clone()),
                _ => return None,
            }
        } else if let [Token::Number(n)] = c {
            if num.is_some() || !n.is_finite() || n.fract() != 0. || *n == 0. || *n < i32::MIN as f32 || *n >= i32::MAX as f32 {
                return None;
            }
            num = Some(*n as i32);
        } else if let Some(n) = matches!(c, [Token::Function(_), ..]).then(|| values::number(c)).flatten() {
            // A math function rounds to the nearest integer.
            if num.is_some() || !n.is_finite() {
                return None;
            }
            num = Some((n.round() as i32).max(if span { 1 } else { i32::MIN }));
        } else {
            return None;
        }
    }
    if span && num.is_some_and(|n| n < 0) { return None; }
    let num = if span { num.map(|n| n.max(1)) } else { num };
    Some(match (span, num, name) {
        (s, Some(n), Some(name)) => GridLine::NamedNth(name, n, s),
        (true, Some(n), None) => GridLine::Span(n as u32),
        (true, None, Some(n)) => GridLine::Named(n, true),
        (false, Some(n), None) => GridLine::Line(n),
        (false, None, Some(n)) => GridLine::Named(n, false),
        _ => return None,
    })
}

/// Split components at top-level `/` delimiters.
fn split_slash<'a>(comps: &[&'a [Token]]) -> Vec<Vec<&'a [Token]>> {
    let mut out = vec![Vec::new()];
    for c in comps {
        if matches!(c, [Token::Delim('/')]) {
            out.push(Vec::new());
        } else {
            out.last_mut().unwrap().push(*c);
        }
    }
    out
}

/// `normal | [ row | column ] [ fill-reverse || track-reverse ]?`
fn parse_lanes_direction(comps: &[&[Token]]) -> Option<GridLanesDirection> {
    let kws: Vec<String> = comps.iter().map(|c| values::ident(c)).collect::<Option<_>>()?;
    if kws.len() == 1 && kws[0] == "normal" {
        return Some(GridLanesDirection::default());
    }
    let mut d = GridLanesDirection::default();
    for (i, k) in kws.iter().enumerate() {
        match k.as_str() {
            "row" | "column" if i == 0 => d.rows = Some(k == "row"),
            "fill-reverse" if i > 0 && !d.fill_reverse => d.fill_reverse = true,
            "track-reverse" if i > 0 && !d.track_reverse => d.track_reverse = true,
            _ => return None,
        }
    }
    (!kws.is_empty()).then_some(d)
}

fn join<'a>(comps: &[&'a [Token]]) -> Vec<Token> {
    let mut v = Vec::new();
    for c in comps {
        v.extend_from_slice(c);
        v.push(Token::Whitespace);
    }
    v
}

/// `grid-template` / `grid` shorthand: `none`, `<rows> / <columns>`, or
/// the areas form `"a b" <row-size>? ... / <columns>`.
fn apply_template(comps: &[&[Token]], style: &mut ComputedStyle, ctx: &LengthContext) {
    if comps.len() == 1 && values::is_ident(comps[0], "none") {
        style.grid_template_rows.clear();
        style.grid_template_columns.clear();
        style.grid_template_areas.clear();
        return;
    }
    let parts = split_slash(comps);
    let rows_part = &parts[0];
    let has_strings = rows_part.iter().any(|c| matches!(c, [Token::String(_)]));
    if has_strings {
        let mut areas = Vec::new();
        let mut rows: Vec<TrackListItem> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        let mut i = 0;
        while i < rows_part.len() {
            match rows_part[i] {
                [Token::String(s)] => {
                    areas.push(s.split_whitespace().map(|x| x.to_string()).collect::<Vec<_>>());
                    // optional track size after the string
                    let mut size = TrackSize::Length(Length::Auto);
                    if let Some(next) = rows_part.get(i + 1) {
                        if !matches!(next, [Token::String(_)] | [Token::OpenBracket]) {
                            let mut cur = Cur::new(next);
                            if let Some(ts) = track_size(&mut cur, ctx) {
                                size = ts;
                                i += 1;
                            }
                        }
                    }
                    rows.push(TrackListItem::Track { names: std::mem::take(&mut names), size });
                }
                [Token::OpenBracket] => {
                    // names spread over components: [ a b ]
                    let mut j = i + 1;
                    while j < rows_part.len() && !matches!(rows_part[j], [Token::CloseBracket]) {
                        if let [Token::Ident(n)] = rows_part[j] {
                            names.push(n.clone());
                        }
                        j += 1;
                    }
                    i = j;
                }
                _ => {}
            }
            i += 1;
        }
        style.grid_template_areas = areas;
        style.grid_template_rows = rows;
        if let Some(cols) = parts.get(1) {
            if let Some(list) = parse_track_list(&join(cols), ctx) {
                style.grid_template_columns = list;
            }
        }
        return;
    }
    if let Some(rows) = parse_track_list(&join(rows_part), ctx) {
        style.grid_template_rows = rows;
    }
    if let Some(cols) = parts.get(1) {
        if let Some(list) = parse_track_list(&join(cols), ctx) {
            style.grid_template_columns = list;
        }
    }
}

pub fn apply(name: &str, comps: &[&[Token]], value: &[Token], style: &mut ComputedStyle, ctx: &ApplyCtx) -> bool {
    let name = match name {
        "-ms-grid-columns" => "grid-template-columns",
        "-ms-grid-rows" => "grid-template-rows",
        n => n,
    };
    match name {
        "grid-template-columns" => {
            if let Some(l) = parse_track_list(value, ctx) {
                style.grid_template_columns = l;
            }
        }
        "grid-template-rows" => {
            if let Some(l) = parse_track_list(value, ctx) {
                style.grid_template_rows = l;
            }
        }
        "grid-template-areas" => {
            if let Some(a) = parse_areas(comps) {
                style.grid_template_areas = a;
            }
        }
        "grid-template" => apply_template(comps, style, ctx),
        "grid" => {
            // `grid: auto-flow [dense] [auto-rows] / <columns>` or
            // `grid: <rows> / auto-flow [dense] [auto-columns]`, else template.
            let parts = split_slash(comps);
            let flow_in = |p: &Vec<&[Token]>| p.iter().any(|c| values::is_ident(c, "auto-flow"));
            if parts.len() == 2 && (flow_in(&parts[0]) || flow_in(&parts[1])) {
                let (flow_part, track_part, is_row) =
                    if flow_in(&parts[0]) { (&parts[0], &parts[1], true) } else { (&parts[1], &parts[0], false) };
                let dense = flow_part.iter().any(|c| values::is_ident(c, "dense"));
                style.grid_auto_flow = match (is_row, dense) {
                    (true, false) => GridAutoFlow::Row,
                    (true, true) => GridAutoFlow::RowDense,
                    (false, false) => GridAutoFlow::Column,
                    (false, true) => GridAutoFlow::ColumnDense,
                };
                let sizes: Vec<&[Token]> = flow_part
                    .iter()
                    .copied()
                    .filter(|c| !values::is_ident(c, "auto-flow") && !values::is_ident(c, "dense"))
                    .collect();
                if let Some(ts) = parse_track_sizes(&join(&sizes), ctx) {
                    if is_row {
                        style.grid_auto_rows = ts;
                    } else {
                        style.grid_auto_columns = ts;
                    }
                }
                if let Some(list) = parse_track_list(&join(track_part), ctx) {
                    if is_row {
                        style.grid_template_columns = list;
                        style.grid_template_rows.clear();
                    } else {
                        style.grid_template_rows = list;
                        style.grid_template_columns.clear();
                    }
                }
            } else {
                apply_template(comps, style, ctx);
            }
        }
        "grid-auto-columns" => {
            if let Some(ts) = parse_track_sizes(value, ctx) {
                style.grid_auto_columns = ts;
            }
        }
        "grid-auto-rows" => {
            if let Some(ts) = parse_track_sizes(value, ctx) {
                style.grid_auto_rows = ts;
            }
        }
        "grid-auto-flow" => {
            let kws: Vec<String> = comps.iter().filter_map(|c| values::ident(c)).collect();
            let dense = kws.iter().any(|k| k == "dense");
            let column = kws.iter().any(|k| k == "column");
            style.grid_auto_flow = match (column, dense) {
                (false, false) => GridAutoFlow::Row,
                (false, true) => GridAutoFlow::RowDense,
                (true, false) => GridAutoFlow::Column,
                (true, true) => GridAutoFlow::ColumnDense,
            };
        }
        "grid-lanes-direction" => {
            if let Some(d) = parse_lanes_direction(comps) {
                style.grid_lanes_direction = d;
            }
        }
        "grid-lanes-pack" => match comps {
            [c] => match values::ident(c).as_deref() {
                Some("dense") => style.grid_lanes_dense = true,
                Some("normal") => style.grid_lanes_dense = false,
                _ => {}
            },
            _ => {}
        },
        "fit-tolerance" | "flow-tolerance" => {
            if let [c] = comps {
                match values::ident(c).as_deref() {
                    Some("normal") => style.fit_tolerance = None,
                    Some("infinite") => style.fit_tolerance = Some(Length::Px(f32::INFINITY)),
                    Some(_) => {}
                    None => {
                        if let Some(l) = values::parse_length(c).map(|v| v.resolve(ctx)) {
                            let math = matches!(c, [Token::Function(_), ..]);
                            match l {
                                // Math functions clamp to the allowed range.
                                Length::Px(v) if v < 0.0 && math => style.fit_tolerance = Some(Length::Px(0.0)),
                                Length::Px(v) | Length::Percent(v) if v < 0.0 => {}
                                l => style.fit_tolerance = Some(l),
                            }
                        }
                    }
                }
            }
        }
        "grid-lanes" => {
            // The longhands of the shorthand, areas included.
            if let Some(longhands) = super::grid_css::expand("grid-lanes", value) {
                for (name, text) in longhands {
                    let toks = rabbithole_css::Tokenizer::tokenize(&text);
                    let parts = values::components(&toks);
                    if name == "grid-template-areas" {
                        style.grid_template_areas = parse_areas(&parts).unwrap_or_default();
                    } else {
                        apply(name, &parts, &toks, style, ctx);
                    }
                }
                return true;
            }
            // `grid-lanes: <direction>? <track-list>` (the track list goes
            // to the template of the grid axis).
            let mut n = 0;
            while n < comps.len() && values::ident(comps[n]).is_some_and(|k| matches!(k.as_str(), "normal" | "row" | "column" | "fill-reverse" | "track-reverse")) {
                n += 1;
            }
            let dir = if n == 0 { Some(GridLanesDirection::default()) } else { parse_lanes_direction(&comps[..n]) };
            let Some(dir) = dir else { return true };
            let rest = join(&comps[n..]);
            let list = if rest.iter().all(|t| matches!(t, Token::Whitespace)) { Some(Vec::new()) } else { parse_track_list(&rest, ctx) };
            let Some(list) = list else { return true };
            style.grid_lanes_direction = dir;
            if dir.rows == Some(true) {
                style.grid_template_rows = list;
                style.grid_template_columns.clear();
            } else {
                style.grid_template_columns = list;
                style.grid_template_rows.clear();
            }
        }
        "grid-row" | "grid-column" => {
            let parts = split_slash(comps);
            let start = match parse_grid_line(&parts[0]) {
                Some(l) => l,
                None => return true,
            };
            let end = match parts.get(1) {
                Some(p) => match parse_grid_line(p) {
                    Some(l) => l,
                    None => return true,
                },
                None => match &start {
                    GridLine::Named(n, false) => GridLine::Named(n.clone(), false),
                    _ => GridLine::Auto,
                },
            };
            if name == "grid-row" {
                style.grid_row_start = start;
                style.grid_row_end = end;
            } else {
                style.grid_column_start = start;
                style.grid_column_end = end;
            }
        }
        "grid-row-start" | "grid-row-end" | "grid-column-start" | "grid-column-end" => {
            if let Some(l) = parse_grid_line(comps) {
                match name {
                    "grid-row-start" => style.grid_row_start = l,
                    "grid-row-end" => style.grid_row_end = l,
                    "grid-column-start" => style.grid_column_start = l,
                    _ => style.grid_column_end = l,
                }
            }
        }
        "grid-area" => {
            let parts = split_slash(comps);
            let lines: Option<Vec<GridLine>> = parts.iter().map(|p| parse_grid_line(p)).collect();
            let Some(lines) = lines else { return true };
            let named_or_auto = |l: &GridLine| match l {
                GridLine::Named(n, false) => GridLine::Named(n.clone(), false),
                _ => GridLine::Auto,
            };
            let rs = lines[0].clone();
            let cs = lines.get(1).cloned().unwrap_or_else(|| named_or_auto(&rs));
            let re = lines.get(2).cloned().unwrap_or_else(|| named_or_auto(&rs));
            let ce = lines.get(3).cloned().unwrap_or_else(|| named_or_auto(&cs));
            style.grid_row_start = rs;
            style.grid_column_start = cs;
            style.grid_row_end = re;
            style.grid_column_end = ce;
        }
        "-ms-grid-row" | "-ms-grid-column" | "-ms-grid-row-span" | "-ms-grid-column-span" => {}
        _ => return false,
    }
    true
}
