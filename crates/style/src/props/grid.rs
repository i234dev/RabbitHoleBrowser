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

/// Parse one track size at the cursor (breadth, `minmax()`, `fit-content()`).
fn track_size(cur: &mut Cur, ctx: &LengthContext) -> Option<TrackSize> {
    if let Some(Token::Function(f)) = cur.peek() {
        let lf = f.to_ascii_lowercase();
        let call = cur.function()?;
        let args = super::fn_args(call);
        return match lf.as_str() {
            "minmax" if args.len() == 2 && args[0].len() == 1 && args[1].len() == 1 => {
                let a = track_breadth(&args[0][0][0], ctx)?;
                let b = track_breadth(&args[1][0][0], ctx)?;
                Some(TrackSize::MinMax(Box::new(a), Box::new(b)))
            }
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

/// Parse a full track list (`grid-template-columns` value).
pub fn parse_track_list(toks: &[Token], ctx: &LengthContext) -> Option<Vec<TrackListItem>> {
    let mut cur = Cur::new(toks);
    if let Some(Token::Ident(i)) = cur.peek() {
        if i.eq_ignore_ascii_case("none") {
            return Some(Vec::new());
        }
        if i.eq_ignore_ascii_case("subgrid") || i.eq_ignore_ascii_case("masonry") {
            return Some(Vec::new());
        }
    }
    let items = parse_items(&mut cur, ctx)?;
    if !cur.done() {
        return None;
    }
    Some(items)
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
    let mut span = false;
    let mut num: Option<i32> = None;
    let mut name: Option<String> = None;
    for c in comps {
        if let Some(kw) = values::ident(c) {
            match kw.as_str() {
                "auto" => return Some(GridLine::Auto),
                "span" => span = true,
                _ => name = Some(kw),
            }
        } else if let Some(n) = values::number(c) {
            num = Some(n as i32);
        } else {
            return None;
        }
    }
    Some(match (span, num, name) {
        (true, Some(n), _) => GridLine::Span(n.max(1) as u32),
        (true, None, Some(n)) => GridLine::Named(n, true),
        (false, Some(n), _) => GridLine::Line(n),
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
