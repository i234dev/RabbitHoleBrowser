//! CSSOM: `element.style`, `getComputedStyle`, `matchMedia`, `CSS`.

use crate::element::{get_attribute, make_proxy, set_attribute};
use rabbithole_js::V;
use crate::env::with_env;
use crate::host::*;
use rabbithole_css::{parse_declaration_list, Declaration, Token};
use rabbithole_js::{JsResult, ObjectId, Realm, Value};
use rabbithole_style::{ComputedStyle, Display, Length, Position};

mod grid;

/// Serialize CSS tokens back to text.
pub(crate) fn tokens_to_css(tokens: &[Token]) -> String {
    let mut out = String::new();
    for t in tokens {
        match t {
            Token::Ident(s) => out.push_str(s),
            Token::Function(f) => {
                out.push_str(f);
                out.push('(');
            }
            Token::AtKeyword(s) => {
                out.push('@');
                out.push_str(s);
            }
            Token::Hash(s) => {
                out.push('#');
                out.push_str(s);
            }
            Token::String(s) => {
                out.push('"');
                out.push_str(&s.replace('\\', "\\\\").replace('"', "\\\""));
                out.push('"');
            }
            Token::Url(u) => {
                out.push_str("url(\"");
                out.push_str(u);
                out.push_str("\")");
            }
            Token::Number(n) => out.push_str(&fmt_num(*n)),
            Token::Percentage(n) => {
                out.push_str(&fmt_num(*n));
                out.push('%');
            }
            Token::Dimension(n, u) => {
                out.push_str(&fmt_num(*n));
                out.push_str(u);
            }
            Token::Whitespace => out.push(' '),
            Token::Colon => out.push(':'),
            Token::Semicolon => out.push(';'),
            Token::Comma => out.push(','),
            Token::OpenBracket => out.push('['),
            Token::CloseBracket => out.push(']'),
            Token::OpenParen => out.push('('),
            Token::CloseParen => out.push(')'),
            Token::OpenBrace => out.push('{'),
            Token::CloseBrace => out.push('}'),
            Token::Delim(c) => out.push(*c),
            Token::BadString | Token::BadUrl => {}
            Token::Cdo => out.push_str("<!--"),
            Token::Cdc => out.push_str("-->"),
        }
    }
    out.trim().to_string()
}

fn fmt_num(n: f32) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        let s = format!("{n}");
        s
    }
}

/// `backgroundColor` → `background-color`; `cssFloat` → `float`;
/// `webkitTransition` → `-webkit-transition`.
pub(crate) fn camel_to_kebab(name: &str) -> String {
    if name == "cssFloat" || name == "styleFloat" {
        return "float".into();
    }
    if name.contains('-') {
        return name.to_string();
    }
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_uppercase() {
            out.push('-');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    if out.starts_with("webkit-") {
        out.insert(0, '-');
    }
    out
}

pub(crate) fn kebab_to_camel(name: &str) -> String {
    let name = name.trim_start_matches('-');
    let mut out = String::new();
    let mut upper = false;
    for c in name.chars() {
        if c == '-' {
            upper = true;
        } else if upper {
            out.push(c.to_ascii_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

struct DeclList {
    items: Vec<(String, String, bool)>,
}

/// CSS property names are ASCII case-insensitive, except custom
/// properties (`--fooBar`), which keep their case.
fn prop_name(s: &str) -> String {
    if s.starts_with("--") {
        s.to_string()
    } else {
        s.to_ascii_lowercase()
    }
}

fn parse_style(n: NodeRef) -> DeclList {
    let text = get_attribute(n, "style").unwrap_or_default();
    let decls: Vec<Declaration> = parse_declaration_list(&text);
    DeclList { items: decls.into_iter().map(|d| (d.name, tokens_to_css(&d.value), d.important)).collect() }
}

fn write_style(n: NodeRef, list: &DeclList) {
    if list.items.is_empty() {
        if get_attribute(n, "style").is_some() {
            set_attribute(n, "style", Some(""));
        }
        return;
    }
    let text = list.items.iter().map(|(k, v, imp)| format!("{k}: {v}{};", if *imp { " !important" } else { "" })).collect::<Vec<_>>().join(" ");
    set_attribute(n, "style", Some(&text));
}

fn style_target(realm: &mut Realm, args: &[Value]) -> JsResult<NodeRef> {
    match node_of(realm, &arg(args, 0)) {
        Some(n) => Ok(n),
        None => Err(realm.throw_type_error("Illegal invocation")),
    }
}

fn key_of(realm: &mut Realm, args: &[Value]) -> Option<String> {
    match (arg(args, 1)).v() {
        V::String(s) => Some(s.to_rust_string()),
        V::Number(n) => Some(format!("{}", n)),
        _ => {
            let _ = realm;
            None
        }
    }
}

fn style_get(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let n = style_target(realm, args)?;
    let Some(key) = key_of(realm, args) else { return Ok(Value::Undefined) };
    let target = arg(args, 0);
    match key.as_str() {
        "cssText" => return Ok(Value::str(&get_attribute(n, "style").unwrap_or_default())),
        "length" => return Ok(Value::Number(parse_style(n).items.len() as f64)),
        "parentRule" => return Ok(Value::Null),
        "setProperty" | "getPropertyValue" | "removeProperty" | "getPropertyPriority" | "item" | "toString" => {
            return realm.get_str(target, &key);
        }
        _ => {}
    }
    if let Ok(i) = key.parse::<usize>() {
        return Ok(parse_style(n).items.get(i).map(|(k, _, _)| Value::str(k)).unwrap_or(Value::Undefined));
    }
    let prop = camel_to_kebab(&key);
    let list = parse_style(n);
    Ok(Value::str(list.items.iter().rev().find(|(k, _, _)| *k == prop).map(|(_, v, _)| v.as_str()).unwrap_or("")))
}

fn set_style_prop(n: NodeRef, prop: &str, value: &str, important: bool) {
    let mut list = parse_style(n);
    let value = value.trim();
    list.items.retain(|(k, _, _)| *k != prop);
    if !value.is_empty() {
        // Expand nothing: keep as given (the cascade handles shorthands).
        list.items.push((prop.to_string(), value.to_string(), important));
    }
    write_style(n, &list);
}

fn style_set(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let n = style_target(realm, args)?;
    let Some(key) = key_of(realm, args) else { return Ok(Value::Bool(true)) };
    let v = arg(args, 2);
    let value = if v.is_nullish() { String::new() } else { realm.to_rust_string(v)? };
    if key == "cssText" {
        set_attribute(n, "style", Some(&value));
        return Ok(Value::Bool(true));
    }
    let prop = camel_to_kebab(&key);
    set_style_prop(n, &prop, &value, false);
    Ok(Value::Bool(true))
}

fn style_has(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let target = arg(args, 0);
    let V::Object(id) = target.v() else { return Ok(Value::Bool(false)); };
    let key = realm.to_property_key(arg(args, 1))?;
    if realm.has_property(id, &key)? { return Ok(Value::Bool(true)); }
    let Some(name) = key_of(realm, args) else { return Ok(Value::Bool(false)); };
    if matches!(name.as_str(), "cssText" | "length" | "parentRule") { return Ok(Value::Bool(true)); }
    if let Ok(index) = name.parse::<usize>() {
        let n = style_target(realm, args)?;
        return Ok(Value::Bool(index.to_string() == name && index < parse_style(n).items.len()));
    }
    Ok(Value::Bool(rabbithole_style::cascade::recognizes_property(&camel_to_kebab(&name))))
}

fn style_own_keys(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let n = style_target(realm, args)?;
    let keys: Vec<Value> = parse_style(n).items.iter().map(|(k, _, _)| Value::str(&kebab_to_camel(k))).collect();
    Ok(realm.new_array_value(keys))
}

fn style_set_property(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "setProperty")?;
    let prop = prop_name(arg_str(realm, args, 0)?.trim());
    let v = arg(args, 1);
    let value = if v.is_nullish() { String::new() } else { realm.to_rust_string(v)? };
    let prio = arg_str_opt(realm, args, 2)?.unwrap_or_default();
    set_style_prop(n, &prop, &value, prio.eq_ignore_ascii_case("important"));
    Ok(Value::Undefined)
}

fn style_get_property_value(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "getPropertyValue")?;
    let prop = prop_name(arg_str(realm, args, 0)?.trim());
    let list = parse_style(n);
    Ok(Value::str(list.items.iter().rev().find(|(k, _, _)| *k == prop).map(|(_, v, _)| v.as_str()).unwrap_or("")))
}

fn style_get_property_priority(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "getPropertyPriority")?;
    let prop = prop_name(arg_str(realm, args, 0)?.trim());
    let list = parse_style(n);
    Ok(Value::str(if list.items.iter().any(|(k, _, imp)| *k == prop && *imp) { "important" } else { "" }))
}

fn style_remove_property(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "removeProperty")?;
    let prop = prop_name(arg_str(realm, args, 0)?.trim());
    let list = parse_style(n);
    let old = list.items.iter().rev().find(|(k, _, _)| *k == prop).map(|(_, v, _)| v.clone()).unwrap_or_default();
    set_style_prop(n, &prop, "", false);
    Ok(Value::str(&old))
}

fn style_item(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "item")?;
    let i = realm.to_number(arg(args, 0))?.max(0.0) as usize;
    Ok(Value::str(parse_style(n).items.get(i).map(|(k, _, _)| k.as_str()).unwrap_or("")))
}

fn style_to_string(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "toString")?;
    Ok(Value::str(&get_attribute(n, "style").unwrap_or_default()))
}

/// The `element.style` proxy (cached on the wrapper).
pub(crate) fn style_object(realm: &mut Realm, wrapper: ObjectId, n: NodeRef) -> Value {
    let cached = get_hidden(realm, wrapper, "__style");
    if cached.is_object() {
        return cached;
    }
    let p = proto("CSSStyleDeclaration");
    let target = realm.new_object_with_proto(Some(p));
    realm.set_host_data(target, Box::new(n));
    realm.set_class_name(target, "CSSStyleDeclaration");
    let proxy = make_proxy(realm, target, &[("get", style_get), ("set", style_set), ("has", style_has), ("ownKeys", style_own_keys)]);
    set_hidden(realm, wrapper, "__style", proxy.clone());
    proxy
}

// ------------------------------------------------------------ computed style

fn color_css(c: rabbithole_gfx::Color) -> String {
    if c.a == 255 {
        format!("rgb({}, {}, {})", c.r, c.g, c.b)
    } else if c.a == 0 {
        "rgba(0, 0, 0, 0)".into()
    } else {
        // CSS Color 4 serializes a byte alpha with two decimal places if
        // that value round-trips to the same byte; otherwise use three.
        let alpha = f32::from(c.a) / 255.0;
        let short = (alpha * 100.0).round() / 100.0;
        let a = if (short * 255.0).round() as u8 == c.a { short } else { (alpha * 1000.0).round() / 1000.0 };
        format!("rgba({}, {}, {}, {})", c.r, c.g, c.b, a)
    }
}

fn px(v: f32) -> String {
    format!("{}px", (v * 1000.0).round() / 1000.0)
}

fn length_css(l: &Length, base: f32) -> String {
    match l {
        Length::Px(v) => px(*v),
        Length::Percent(p) => px(base * p / 100.0),
        Length::Calc(v, p) => px(v + base * p / 100.0),
        Length::Math(_) => px(l.resolve_or_zero(base)),
        Length::Auto => "auto".into(),
        Length::MinContent => "min-content".into(),
        Length::MaxContent => "max-content".into(),
        Length::FitContent => "fit-content".into(),
    }
}

fn display_css(d: Display) -> &'static str {
    match d {
        Display::None => "none",
        Display::Block => "block",
        Display::Inline => "inline",
        Display::InlineBlock => "inline-block",
        Display::ListItem => "list-item",
        Display::Flex => "flex",
        Display::InlineFlex => "inline-flex",
        Display::Grid => "grid",
        Display::InlineGrid => "inline-grid",
        Display::Table => "table",
        Display::InlineTable => "inline-table",
        Display::TableRow => "table-row",
        Display::TableRowGroup => "table-row-group",
        Display::TableHeaderGroup => "table-header-group",
        Display::TableFooterGroup => "table-footer-group",
        Display::TableCell => "table-cell",
        Display::TableCaption => "table-caption",
        Display::TableColumn => "table-column",
        Display::TableColumnGroup => "table-column-group",
        Display::FlowRoot => "flow-root",
        Display::Contents => "contents",
    }
}

/// Computed value of `prop` for a node, as a CSS string.
pub(crate) fn computed_value(n: NodeRef, prop: &str) -> Option<String> {
    if n.doc & crate::host::FRAME_DOC != 0 {
        // A same-origin frame's document. Grid templates have a dedicated
        // final-layout query; the other existing frame values use style.
        let frame = (n.doc & !crate::host::FRAME_DOC) as u64;
        let s = with_env(|env| env.frame_computed_style(frame, n.node))?;
        if let Some(rows) = grid::axis(prop).filter(|_| matches!(s.display, Display::Grid | Display::InlineGrid)) {
            if let Some(tracks) = with_env(|env| env.frame_used_grid_tracks(frame, n.node, rows)) {
                return Some(grid::used(&tracks));
            }
        }
        if let Some(value) = resolved_inset(n, &s, prop) { return Some(value); }
        return Some(computed_from_style(&s, prop, Default::default(), Default::default()));
    }
    if n.doc != 0 {
        return None;
    }
    if prop.starts_with("--") {
        let tokens = with_env(|env| env.computed_custom_property(n.node, prop))?;
        return Some(tokens_to_css(&tokens).trim().to_string());
    }
    // Style first (cheap: computed on demand when the DOM changed); the
    // layout box only when the value actually depends on it, so scripts
    // that interleave style writes and reads don't force full layouts.
    let style = with_env(|env| env.computed_style(n.node));
    let s: std::rc::Rc<ComputedStyle> = style?;
    if let Some(value) = resolved_inset(n, &s, prop) { return Some(value); }
    let pct = |l: &rabbithole_style::Length| matches!(l, rabbithole_style::Length::Percent(_) | rabbithole_style::Length::Calc(..) | rabbithole_style::Length::Math(_));
    let needs_box = match prop {
        "grid-template-columns" | "grid-template-rows" => true,
        "width" | "height" | "inline-size" | "block-size" => true,
        "transform" => !s.transform.is_empty(),
        "margin-top" => pct(&s.margin.top),
        "margin-right" => pct(&s.margin.right),
        "margin-bottom" => pct(&s.margin.bottom),
        "margin-left" => pct(&s.margin.left),
        "padding-top" => pct(&s.padding.top),
        "padding-right" => pct(&s.padding.right),
        "padding-bottom" => pct(&s.padding.bottom),
        "padding-left" => pct(&s.padding.left),
        "min-width" => pct(&s.min_width),
        "max-width" => pct(&s.max_width),
        "min-height" => pct(&s.min_height),
        "max-height" => pct(&s.max_height),
        _ => false,
    };
    let s = if needs_box {
        // Geometry needs an up-to-date layout (and the style it used).
        let (st, _) = with_env(|env| {
            env.ensure_layout();
            (env.computed_style(n.node), ())
        });
        st.unwrap_or(s)
    } else {
        s
    };
    if let Some(rows) = grid::axis(prop) {
        // Legacy internal layout (for example frameset) may use Grid boxes
        // without making the element a CSS grid container.
        let used = if matches!(s.display, Display::Grid | Display::InlineGrid) {
            with_env(|env| env.used_grid_tracks(n.node, rows))
        } else { None };
        return Some(used
            .map(|tracks| grid::used(&tracks))
            .unwrap_or_else(|| grid::computed(if rows { &s.grid_template_rows } else { &s.grid_template_columns })));
    }
    let r = if needs_box { with_env(|env| env.node_rect(n.node)).unwrap_or_default() } else { Default::default() };
    let pct_padding = [&s.padding.top, &s.padding.right, &s.padding.bottom, &s.padding.left].into_iter().any(|l| pct(l));
    let cb = if needs_box && prop != "transform" && (!matches!(prop, "width" | "height" | "inline-size" | "block-size") || pct_padding) {
        containing_block(n, s.position, false)
    } else {
        Default::default()
    };
    Some(computed_from_style(&s, prop, r, cb))
}

/// A computed value read from a style alone (no layout box).
pub(crate) fn computed_value_of_style(s: &ComputedStyle, prop: &str) -> String {
    computed_from_style(s, prop, Default::default(), Default::default())
}

fn layout_rect(n: NodeRef) -> Option<rabbithole_gfx::Rect> {
    with_env(|env| if n.doc == 0 { env.node_rect(n.node) } else { env.frame_node_rect((n.doc & !FRAME_DOC) as u64, n.node) })
}

/// CSSOM returns the computed inset for unpositioned elements and elements
/// without a layout box. Positioned boxes resolve percentages, and relative
/// and absolutely positioned boxes also expose used values for auto insets.
fn resolved_inset(n: NodeRef, s: &ComputedStyle, prop: &str) -> Option<String> {
    let (value, opposite, vertical, start) = match prop {
        "top" => (&s.inset.top, &s.inset.bottom, true, true),
        "bottom" => (&s.inset.bottom, &s.inset.top, true, false),
        "left" => (&s.inset.left, &s.inset.right, false, true),
        "right" => (&s.inset.right, &s.inset.left, false, false),
        _ => return None,
    };
    if s.position == Position::Static || matches!(s.display, Display::None | Display::Contents)
        || matches!(value, Length::Px(_)) || (value.is_auto() && s.position == Position::Sticky) {
        return Some(position_css(value));
    }
    let Some(rect) = layout_rect(n) else { return Some(position_css(value)); };
    let cb = containing_block(n, s.position, true);
    let base = if vertical { cb.h } else { cb.w };
    if !value.is_auto() { return Some(length_css(value, base)); }
    if s.position == Position::Relative {
        let offset = -opposite.resolve_or_zero(base);
        return Some(px(if offset == 0.0 { 0.0 } else { offset }));
    }
    let (origin, extent, margin_start, margin_end) = if vertical {
        (rect.y - cb.y, rect.h, s.margin.top, s.margin.bottom)
    } else {
        (rect.x - cb.x, rect.w, s.margin.left, s.margin.right)
    };
    let offset = if start { origin - margin_start.resolve_or_zero(cb.w) } else { base - origin - extent - margin_end.resolve_or_zero(cb.w) };
    Some(px(offset))
}

/// The containing block of `node` (after layout), the base of its
/// percentage margins, padding, insets and min/max sizes: the padding box
/// of the nearest positioned ancestor for absolute positioning, the
/// viewport for fixed, else the parent's content box.
fn containing_block(n: NodeRef, position: Position, sticky_inset: bool) -> rabbithole_gfx::Rect {
    let viewport = || {
        let (w, h) = with_env(|env| if n.doc == 0 { env.viewport() } else { env.frame_viewport((n.doc & !FRAME_DOC) as u64).unwrap_or_default() });
        rabbithole_gfx::Rect { x: 0.0, y: 0.0, w, h }
    };
    let out_of_flow = matches!(position, Position::Fixed | Position::Absolute);
    let mut cur = with_doc(n.doc, |doc| doc.flat_parent(n.node));
    while let Some(p) = cur {
        let is_element = with_doc(n.doc, |doc| doc.element(p).is_some());
        if !is_element {
            break;
        }
        let frame = (n.doc & !FRAME_DOC) as u64;
        let ps = with_env(|env| if n.doc == 0 { env.computed_style(p) } else { env.frame_computed_style(frame, p) });
        if let Some(ps) = ps.filter(|ps| ps.display != Display::Contents && match position {
            Position::Fixed => ps.establishes_fixed_containing_block(),
            Position::Absolute => ps.position != Position::Static || ps.establishes_fixed_containing_block(),
            Position::Sticky if sticky_inset => [ps.overflow_x, ps.overflow_y].into_iter()
                .any(|o| !matches!(o, rabbithole_style::Overflow::Visible | rabbithole_style::Overflow::Clip)),
            _ => ps.display != Display::Inline,
        }) {
            if let Some(pr) = layout_rect(NodeRef { doc: n.doc, node: p }) {
                let b = &ps.border_width;
                let (mut x, mut y, mut w, mut h) = (pr.x + b.left, pr.y + b.top, pr.w - b.left - b.right, pr.h - b.top - b.bottom);
                if !out_of_flow {
                    let pw = w;
                    let pad = &ps.padding;
                    let (pl, pt) = (pad.left.resolve_or_zero(pw), pad.top.resolve_or_zero(pw));
                    x += pl;
                    y += pt;
                    w -= pl + pad.right.resolve_or_zero(pw);
                    h -= pt + pad.bottom.resolve_or_zero(pw);
                }
                return rabbithole_gfx::Rect { x, y, w: w.max(0.0), h: h.max(0.0) };
            }
        }
        cur = with_doc(n.doc, |doc| doc.flat_parent(p));
    }
    viewport()
}

/// A computed value from the style (and the border box `r` and the
/// containing block `cb`, for values that depend on layout).
fn computed_from_style(s: &ComputedStyle, prop: &str, r: rabbithole_gfx::Rect, cb: rabbithole_gfx::Rect) -> String {
    match prop {
        "grid-template-columns" => grid::computed(&s.grid_template_columns),
        "grid-template-rows" => grid::computed(&s.grid_template_rows),
        "display" => display_css(s.display).to_string(),
        "position" => match s.position {
            Position::Static => "static",
            Position::Relative => "relative",
            Position::Absolute => "absolute",
            Position::Fixed => "fixed",
            Position::Sticky => "sticky",
        }
        .to_string(),
        "visibility" => if s.visibility == rabbithole_style::Visibility::Hidden { "hidden".into() } else { "visible".into() },
        "opacity" => format!("{}", s.opacity),
        "color" => s.color_components.as_ref().map_or_else(|| color_css(s.color), |c| c.to_css()),
        "background-color" => s.background_color_components.as_ref().map_or_else(|| color_css(s.background_color), |c| c.to_css()),
        "border-top-color" => color_css(s.border_color.top),
        "border-right-color" => color_css(s.border_color.right),
        "border-bottom-color" => color_css(s.border_color.bottom),
        "border-left-color" => color_css(s.border_color.left),
        "font-size" => px(s.font_size),
        "font-weight" => format!("{}", s.font_weight),
        "font-style" => if s.is_italic() { "italic".into() } else { "normal".into() },
        "font-family" => {
            if s.font_family_list.is_empty() {
                match s.font_family {
                    rabbithole_gfx::Family::Sans => "sans-serif".into(),
                    rabbithole_gfx::Family::Serif => "serif".into(),
                    rabbithole_gfx::Family::Mono => "monospace".into(),
                }
            } else {
                s.font_family_list.iter().map(|f| if f.contains(' ') { format!("\"{f}\"") } else { f.clone() }).collect::<Vec<_>>().join(", ")
            }
        }
        "line-height" => s.line_height.map(px).unwrap_or_else(|| "normal".into()),
        "width" => px(r.w - s.border_width.left - s.border_width.right - s.padding.left.resolve_or_zero(cb.w) - s.padding.right.resolve_or_zero(cb.w)),
        "height" => px(r.h - s.border_width.top - s.border_width.bottom - s.padding.top.resolve_or_zero(cb.w) - s.padding.bottom.resolve_or_zero(cb.w)),
        "margin-top" => length_css(&s.margin.top, cb.w),
        "margin-right" => length_css(&s.margin.right, cb.w),
        "margin-bottom" => length_css(&s.margin.bottom, cb.w),
        "margin-left" => length_css(&s.margin.left, cb.w),
        "padding-top" => length_css(&s.padding.top, cb.w),
        "padding-right" => length_css(&s.padding.right, cb.w),
        "padding-bottom" => length_css(&s.padding.bottom, cb.w),
        "padding-left" => length_css(&s.padding.left, cb.w),
        "border-top-width" => px(s.border_width.top),
        "border-right-width" => px(s.border_width.right),
        "border-bottom-width" => px(s.border_width.bottom),
        "border-left-width" => px(s.border_width.left),
        "top" => length_css(&s.inset.top, cb.h),
        "left" => length_css(&s.inset.left, cb.w),
        "right" => length_css(&s.inset.right, cb.w),
        "bottom" => length_css(&s.inset.bottom, cb.h),
        "z-index" => s.z_index.map(|z| z.to_string()).unwrap_or_else(|| "auto".into()),
        "overflow" | "overflow-x" | "overflow-y" => match s.overflow_x {
            rabbithole_style::Overflow::Visible => "visible",
            rabbithole_style::Overflow::Hidden => "hidden",
            rabbithole_style::Overflow::Scroll => "scroll",
            rabbithole_style::Overflow::Auto => "auto",
            rabbithole_style::Overflow::Clip => "clip",
        }
        .to_string(),
        "text-align" => match s.text_align {
            rabbithole_style::TextAlign::Start => "start",
            rabbithole_style::TextAlign::End => "end",
            rabbithole_style::TextAlign::Left => "left",
            rabbithole_style::TextAlign::Right => "right",
            rabbithole_style::TextAlign::Center => "center",
            rabbithole_style::TextAlign::Justify => "justify",
        }
        .to_string(),
        "white-space" => match s.white_space {
            rabbithole_style::WhiteSpace::Normal => "normal",
            rabbithole_style::WhiteSpace::Nowrap => "nowrap",
            rabbithole_style::WhiteSpace::Pre => "pre",
            rabbithole_style::WhiteSpace::PreWrap => "pre-wrap",
            rabbithole_style::WhiteSpace::BreakSpaces => "break-spaces",
            rabbithole_style::WhiteSpace::PreLine => "pre-line",
        }
        .to_string(),
        "box-sizing" => if s.box_sizing == rabbithole_style::BoxSizing::BorderBox { "border-box".into() } else { "content-box".into() },
        "float" => match s.float {
            rabbithole_style::Float::None => "none",
            rabbithole_style::Float::Left => "left",
            rabbithole_style::Float::Right => "right",
        }
        .to_string(),
        "flex-direction" => match s.flex_direction {
            rabbithole_style::FlexDirection::Row => "row",
            rabbithole_style::FlexDirection::RowReverse => "row-reverse",
            rabbithole_style::FlexDirection::Column => "column",
            rabbithole_style::FlexDirection::ColumnReverse => "column-reverse",
        }
        .to_string(),
        "pointer-events" => if s.pointer_events == rabbithole_style::PointerEvents::None { "none".into() } else { "auto".into() },
        "cursor" => s.cursor.into(),
        "transform" => {
            if s.transform.is_empty() {
                "none".into()
            } else if s.transform.iter().all(rabbithole_style::TransformOp::is_2d) {
                let m = rabbithole_style::transform_matrix(&s.transform, r.w, r.h, s.font_size);
                format!("matrix({}, {}, {}, {}, {}, {})", m[0], m[1], m[2], m[3], m[4], m[5])
            } else {
                let m = rabbithole_gfx::mat4::to_css(&rabbithole_style::transform_matrix_3d(&s.transform, r.w, r.h));
                if rabbithole_gfx::mat4::is_2d(&rabbithole_gfx::mat4::from_css(&m)) {
                    format!("matrix({}, {}, {}, {}, {}, {})", m[0], m[1], m[4], m[5], m[12], m[13])
                } else {
                    format!("matrix3d({})", m.iter().map(|v| format!("{}", v)).collect::<Vec<_>>().join(", "))
                }
            }
        }
        "transform-box" => match s.transform3d.as_ref().map(|t| t.transform_box).unwrap_or_default() {
            rabbithole_style::TransformBox::ContentBox => "content-box",
            rabbithole_style::TransformBox::BorderBox => "border-box",
            rabbithole_style::TransformBox::FillBox => "fill-box",
            rabbithole_style::TransformBox::StrokeBox => "stroke-box",
            rabbithole_style::TransformBox::ViewBox => "view-box",
        }
        .to_string(),
        "transform-style" => if s.preserve_3d() { "preserve-3d".into() } else { "flat".into() },
        "backface-visibility" => if s.backface_hidden() { "hidden".into() } else { "visible".into() },
        "perspective" => s.perspective().map_or("none".into(), |d| format!("{}px", d)),
        "translate" | "rotate" | "scale" => {
            let t = s.transform3d.as_deref();
            let op = match prop {
                "translate" => t.and_then(|t| t.translate.as_ref()),
                "rotate" => t.and_then(|t| t.rotate.as_ref()),
                _ => t.and_then(|t| t.scale.as_ref()),
            };
            let len = |l: &rabbithole_style::Length, base: f32| match l {
                rabbithole_style::Length::Percent(p) if base == 0.0 => format!("{p}%"),
                other => format!("{}px", other.resolve_or_zero(base)),
            };
            match op {
                None => "none".into(),
                Some(rabbithole_style::TransformOp::Translate3d(x, y, z)) => {
                    let (x, y) = (len(x, 0.0), len(y, 0.0));
                    if *z != 0.0 { format!("{x} {y} {z}px") } else if y != "0px" { format!("{x} {y}") } else { x }
                }
                Some(rabbithole_style::TransformOp::Rotate3d(x, y, z, a)) => {
                    let deg = a.to_degrees();
                    match (*x, *y, *z) {
                        (x, y, z) if x == 0.0 && y == 0.0 && z > 0.0 => format!("{deg}deg"),
                        (x, y, z) if y == 0.0 && z == 0.0 && x > 0.0 => format!("x {deg}deg"),
                        (x, y, z) if x == 0.0 && z == 0.0 && y > 0.0 => format!("y {deg}deg"),
                        (x, y, z) => format!("{x} {y} {z} {deg}deg"),
                    }
                }
                Some(rabbithole_style::TransformOp::Scale3d(x, y, z)) => {
                    if *z != 1.0 { format!("{x} {y} {z}") } else if x != y { format!("{x} {y}") } else { format!("{x}") }
                }
                Some(_) => "none".into(),
            }
        }
        "text-decoration" | "text-decoration-line" => {
            let mut parts = Vec::new();
            if s.text_decoration.underline {
                parts.push("underline");
            }
            if s.text_decoration.overline {
                parts.push("overline");
            }
            if s.text_decoration.line_through {
                parts.push("line-through");
            }
            if parts.is_empty() { "none".into() } else { parts.join(" ") }
        }
        "border-radius" => px(s.border_radius),
        "border-top-left-radius" => radius_css(s, 0),
        "border-top-right-radius" => radius_css(s, 1),
        "border-bottom-right-radius" => radius_css(s, 2),
        "border-bottom-left-radius" => radius_css(s, 3),
        "border-top-style" => border_style_css(s.border_style.top).into(),
        "border-right-style" => border_style_css(s.border_style.right).into(),
        "border-bottom-style" => border_style_css(s.border_style.bottom).into(),
        "border-left-style" => border_style_css(s.border_style.left).into(),
        p if p.starts_with("border-block-") || p.starts_with("border-inline-") || p.starts_with("border-start-") || p.starts_with("border-end-") => {
            match physical_border_prop(p, s.direction == rabbithole_style::Direction::Rtl) {
                Some(phys) => computed_from_style(s, &phys, r, cb),
                None => String::new(),
            }
        }
        "border-image-source" => "none".into(),
        "border-image-slice" => "100%".into(),
        "border-image-width" => "1".into(),
        "border-image-outset" => "0".into(),
        "border-image-repeat" => "stretch".into(),
        "background-attachment" => if s.background_layers.first().is_some_and(|l| l.fixed) { "fixed".into() } else { "scroll".into() },
        "background-clip" => box_area_css(s.background_layers.first().map_or(rabbithole_style::BoxArea::BorderBox, |l| l.clip)).into(),
        "background-origin" => box_area_css(s.background_layers.first().map_or(rabbithole_style::BoxArea::PaddingBox, |l| l.origin)).into(),
        "background-size" => match s.background_layers.first().map(|l| &l.size) {
            Some(rabbithole_style::BackgroundSize::Cover) => "cover".into(),
            Some(rabbithole_style::BackgroundSize::Contain) => "contain".into(),
            Some(rabbithole_style::BackgroundSize::Explicit(w, h)) => format!("{} {}", position_css(w), position_css(h)),
            _ => "auto".into(),
        },
        "background-position" => match s.background_layers.first() {
            Some(l) => format!("{} {}", position_css(&l.position.0), position_css(&l.position.1)),
            None => "0% 0%".into(),
        },
        "direction" => if s.direction == rabbithole_style::Direction::Rtl { "rtl".into() } else { "ltr".into() },
        "background-image" => s.background_image.as_ref().map(|u| format!("url(\"{u}\")")).unwrap_or_else(|| "none".into()),
        "min-width" => length_css(&s.min_width, cb.w),
        "max-width" => if s.max_width.is_auto() { "none".into() } else { length_css(&s.max_width, cb.w) },
        "min-height" => length_css(&s.min_height, cb.h),
        "max-height" => if s.max_height.is_auto() { "none".into() } else { length_css(&s.max_height, cb.h) },
        "letter-spacing" => if s.letter_spacing == 0.0 { "normal".into() } else { px(s.letter_spacing) },
        "text-transform" => match s.text_transform {
            rabbithole_style::TextTransform::None => "none",
            rabbithole_style::TextTransform::Uppercase => "uppercase",
            rabbithole_style::TextTransform::Lowercase => "lowercase",
            rabbithole_style::TextTransform::Capitalize => "capitalize",
        }
        .to_string(),
        "vertical-align" => match s.vertical_align {
            rabbithole_style::VerticalAlign::Baseline => "baseline".into(),
            rabbithole_style::VerticalAlign::Middle => "middle".into(),
            rabbithole_style::VerticalAlign::Top => "top".into(),
            rabbithole_style::VerticalAlign::Bottom => "bottom".into(),
            rabbithole_style::VerticalAlign::TextTop => "text-top".into(),
            rabbithole_style::VerticalAlign::TextBottom => "text-bottom".into(),
            rabbithole_style::VerticalAlign::Sub => "sub".into(),
            rabbithole_style::VerticalAlign::Super => "super".into(),
            rabbithole_style::VerticalAlign::Length(v) => px(v),
        },
        "user-select" => "auto".into(),
        "content" => "normal".into(),
        p if p.starts_with("transition-") || p.starts_with("animation-") => timing_css(s, p).unwrap_or_default(),
        _ => String::new(),
    }
}

fn border_style_css(b: rabbithole_style::BorderStyle) -> &'static str {
    match b {
        rabbithole_style::BorderStyle::None => "none",
        rabbithole_style::BorderStyle::Hidden => "hidden",
        rabbithole_style::BorderStyle::Solid => "solid",
        rabbithole_style::BorderStyle::Dashed => "dashed",
        rabbithole_style::BorderStyle::Dotted => "dotted",
        rabbithole_style::BorderStyle::Double => "double",
    }
}

fn box_area_css(a: rabbithole_style::BoxArea) -> &'static str {
    match a {
        rabbithole_style::BoxArea::BorderBox => "border-box",
        rabbithole_style::BoxArea::PaddingBox => "padding-box",
        rabbithole_style::BoxArea::ContentBox => "content-box",
    }
}

/// A background position component: percentages stay percentages.
fn position_css(l: &Length) -> String {
    match l {
        Length::Percent(p) => format!("{p}%"),
        Length::Calc(v, p) => rabbithole_style::MathNode::Leaf(*v, *p).to_css(),
        Length::Math(id) => rabbithole_style::math_node(*id).map(|n| n.to_css()).unwrap_or_default(),
        other => length_css(other, 0.0),
    }
}

/// Computed corner radius `i` (top-left, top-right, bottom-right,
/// bottom-left): one value when both radii are equal.
fn radius_css(s: &ComputedStyle, i: usize) -> String {
    let (h, v) = &s.border_radii[i];
    let (h, v) = (position_css(h), position_css(v));
    if h == v { h } else { format!("{h} {v}") }
}

/// The physical longhand of a logical border property, for horizontal
/// writing mode in direction `rtl` or ltr.
fn physical_border_prop(p: &str, rtl: bool) -> Option<String> {
    let (start, end) = if rtl { ("right", "left") } else { ("left", "right") };
    for (logical, physical) in [
        ("border-start-start-radius", format!("border-top-{start}-radius")),
        ("border-start-end-radius", format!("border-top-{end}-radius")),
        ("border-end-start-radius", format!("border-bottom-{start}-radius")),
        ("border-end-end-radius", format!("border-bottom-{end}-radius")),
    ] {
        if p == logical {
            return Some(physical);
        }
    }
    let rest = p.strip_prefix("border-")?;
    let (side, prop) = if let Some(x) = rest.strip_prefix("block-start-") {
        ("top", x)
    } else if let Some(x) = rest.strip_prefix("block-end-") {
        ("bottom", x)
    } else if let Some(x) = rest.strip_prefix("inline-start-") {
        (start, x)
    } else if let Some(x) = rest.strip_prefix("inline-end-") {
        (end, x)
    } else {
        return None;
    };
    matches!(prop, "color" | "style" | "width").then(|| format!("border-{side}-{prop}"))
}

/// Computed `transition-*` / `animation-*` longhands for `getComputedStyle`.
fn timing_css(s: &ComputedStyle, prop: &str) -> Option<String> {
    let secs = |v: f32| format!("{}s", (v * 1000.0).round() / 1000.0);
    let list = |items: Vec<String>, empty: &str| if items.is_empty() { empty.to_string() } else { items.join(", ") };
    Some(match prop {
        "transition-property" => list(s.transitions.iter().map(|t| t.property.clone()).collect(), "all"),
        "transition-duration" => list(s.transitions.iter().map(|t| secs(t.duration)).collect(), "0s"),
        "transition-delay" => list(s.transitions.iter().map(|t| secs(t.delay)).collect(), "0s"),
        "transition-timing-function" => list(s.transitions.iter().map(|t| t.timing.clone()).collect(), "ease"),
        "animation-name" => list(s.animations.iter().map(|a| a.name.clone()).collect(), "none"),
        "animation-duration" => list(s.animations.iter().map(|a| secs(a.duration)).collect(), "0s"),
        "animation-delay" => list(s.animations.iter().map(|a| secs(a.delay)).collect(), "0s"),
        "animation-timing-function" => list(s.animations.iter().map(|a| a.timing.clone()).collect(), "ease"),
        "animation-iteration-count" => list(s.animations.iter().map(|a| a.iterations.map(|n| format!("{n}")).unwrap_or_else(|| "infinite".into())).collect(), "1"),
        "animation-direction" => list(s.animations.iter().map(|a| a.direction.clone()).collect(), "normal"),
        "animation-fill-mode" => list(s.animations.iter().map(|a| a.fill_mode.clone()).collect(), "none"),
        "animation-play-state" => list(s.animations.iter().map(|a| if a.paused { "paused" } else { "running" }.to_string()).collect(), "running"),
        _ => return None,
    })
}


fn cs_get(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let n = style_target(realm, args)?;
    let Some(key) = key_of(realm, args) else { return Ok(Value::Undefined) };
    match key.as_str() {
        "getPropertyValue" | "getPropertyPriority" | "item" | "length" | "cssText" | "setProperty" | "removeProperty" | "toString" => {
            return realm.get_str(arg(args, 0), &key);
        }
        _ => {}
    }
    let prop = camel_to_kebab(&key);
    Ok(Value::str(&computed_value(n, &prop).unwrap_or_default()))
}

fn cs_has(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let target = arg(args, 0);
    let V::Object(id) = target.v() else { return Ok(Value::Bool(false)); };
    let key = realm.to_property_key(arg(args, 1))?;
    if realm.has_property(id, &key)? { return Ok(Value::Bool(true)); }
    Ok(Value::Bool(key_of(realm, args).is_some_and(|k| rabbithole_style::cascade::recognizes_property(&camel_to_kebab(&k)))))
}

fn cs_set(_realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(false))
}

fn cs_get_property_value(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "getPropertyValue")?;
    let prop = prop_name(arg_str(realm, args, 0)?.trim());
    Ok(Value::str(&computed_value(n, &prop).unwrap_or_default()))
}

fn cs_empty(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::str(""))
}

fn cs_zero(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Number(0.0))
}

fn get_computed_style(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(n) = node_of(realm, &arg(args, 0)) else {
        return Err(realm.throw_type_error("Failed to execute 'getComputedStyle' on 'Window': parameter 1 is not of type 'Element'."));
    };
    let p = proto("CSSStyleDeclaration");
    let target = realm.new_object_with_proto(Some(p));
    realm.set_host_data(target, Box::new(n));
    realm.set_class_name(target, "CSSStyleDeclaration");
    realm.define_method(target, "getPropertyValue", 1, cs_get_property_value);
    realm.define_method(target, "getPropertyPriority", 1, cs_empty);
    realm.define_method(target, "setProperty", 2, cs_set);
    realm.define_method(target, "removeProperty", 1, cs_empty);
    realm.define_method(target, "item", 1, cs_empty);
    realm.define_property(target, "length", Value::Number(0.0), 0);
    realm.define_property(target, "cssText", Value::str(""), 0);
    let _ = cs_zero;
    Ok(make_proxy(realm, target, &[("get", cs_get), ("set", cs_set), ("has", cs_has)]))
}

// ---------------------------------------------------------------- matchMedia

fn mql_matches(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let media = realm.get_str(this, "media")?;
    let media = realm.to_rust_string(media)?;
    let q = rabbithole_css::parse_media_query(&media);
    let (w, h) = with_env(|env| env.viewport());
    Ok(Value::Bool(q.evaluate(&rabbithole_css::MediaEnv { width: w, height: h, screen: true, scripting: true })))
}

fn mql_noop(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Undefined)
}

fn match_media(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let media = arg_str(realm, args, 0)?;
    let p = proto("MediaQueryList");
    let o = realm.new_object_with_proto(Some(p));
    realm.define_property(o, "media", Value::str(media.trim()), rabbithole_js::CONFIGURABLE | rabbithole_js::ENUMERABLE);
    realm.define_accessor(o, "matches", Some(mql_matches), None, rabbithole_js::CONFIGURABLE | rabbithole_js::ENUMERABLE);
    realm.define_property(o, "onchange", Value::Null, rabbithole_js::ALL);
    Ok(Value::Object(o))
}

fn css_supports(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if args.is_empty() { return Err(realm.throw_type_error("CSS.supports requires an argument")); }
    let first = arg_str(realm, args, 0)?;
    rabbithole_style::install_supports_validator();
    let matches = if args.len() == 1 {
        rabbithole_css::supports::evaluate_script_condition(&rabbithole_css::Tokenizer::tokenize(&first))
    } else {
        let value = arg_str(realm, args, 1)?;
        rabbithole_css::supports::evaluate_property(&first, &rabbithole_css::Tokenizer::tokenize(&value))
    };
    Ok(Value::Bool(matches))
}

fn css_escape(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let s = arg_str(realm, args, 0)?;
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c == '\0' {
            out.push('\u{fffd}');
        } else if c.is_ascii_alphanumeric() || c == '-' || c == '_' || (c as u32) >= 0x80 {
            if i == 0 && c.is_ascii_digit() {
                out.push_str(&format!("\\{:x} ", c as u32));
            } else {
                out.push(c);
            }
        } else {
            out.push('\\');
            out.push(c);
        }
    }
    Ok(Value::str(&out))
}

pub(crate) fn install(realm: &mut Realm) {
    let csd = define_class(realm, "CSSStyleDeclaration", None, illegal_ctor);
    realm.define_method(csd, "setProperty", 2, style_set_property);
    realm.define_method(csd, "getPropertyValue", 1, style_get_property_value);
    realm.define_method(csd, "getPropertyPriority", 1, style_get_property_priority);
    realm.define_method(csd, "removeProperty", 1, style_remove_property);
    realm.define_method(csd, "item", 1, style_item);
    realm.define_method(csd, "toString", 0, style_to_string);
    let mql = define_class(realm, "MediaQueryList", Some("EventTarget"), illegal_ctor);
    realm.define_method(mql, "addListener", 1, mql_add_listener);
    realm.define_method(mql, "removeListener", 1, mql_noop);
    let g = realm.global();
    realm.define_method(g, "getComputedStyle", 1, get_computed_style);
    realm.define_method(g, "matchMedia", 1, match_media);
    let css = realm.new_object();
    realm.define_method(css, "supports", 1, css_supports);
    realm.define_method(css, "escape", 1, css_escape);
    realm.define_global("CSS", Value::Object(css));
    define_class(realm, "CSSStyleSheet", None, css_style_sheet_ctor);
    define_class(realm, "StyleSheet", None, illegal_ctor);
    define_class(realm, "CSSRule", None, illegal_ctor);
}

fn mql_add_listener(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    // `addListener(fn)` ≡ addEventListener('change', fn)
    let f = arg(args, 0);
    let add = realm.get_str(this.clone(), "addEventListener")?;
    realm.call(add, this, &[Value::str("change"), f])
}

fn css_style_sheet_ctor(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let rules = realm.new_array_value(vec![]);
    let o = object_from(realm, vec![("cssRules", rules.clone()), ("rules", rules), ("href", Value::Null), ("disabled", Value::Bool(false))]);
    if let V::Object(id) = o.v() {
        realm.define_method(id, "insertRule", 2, cs_zero);
        realm.define_method(id, "deleteRule", 1, mql_noop);
        realm.define_method(id, "replaceSync", 1, mql_noop);
        realm.define_method(id, "replace", 1, mql_noop);
    }
    Ok(o)
}
