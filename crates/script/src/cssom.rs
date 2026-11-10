//! CSSOM: `element.style`, `getComputedStyle`, `matchMedia`, `CSS`.

use crate::element::{get_attribute, make_proxy, set_attribute};
use crate::env::with_env;
use crate::host::*;
use rabbithole_css::{parse_declaration_list, Declaration, Token};
use rabbithole_js::{JsResult, ObjectId, Realm, Value};
use rabbithole_style::{ComputedStyle, Display, Length, Position};

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
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i == 0 && (name.starts_with("webkit") || name.starts_with("Webkit")) {
                out.push('-');
            }
            out.push('-');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    if out.starts_with("webkit-") || out.starts_with("moz-") || out.starts_with("ms-") {
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
    match arg(args, 1) {
        Value::String(s) => Some(s.to_rust_string()),
        Value::Number(n) => Some(format!("{}", n)),
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
    let _ = style_target(realm, args)?;
    Ok(Value::Bool(true))
}

fn style_own_keys(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let n = style_target(realm, args)?;
    let keys: Vec<Value> = parse_style(n).items.iter().map(|(k, _, _)| Value::str(&kebab_to_camel(k))).collect();
    Ok(realm.new_array_value(keys))
}

fn style_set_property(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "setProperty")?;
    let prop = arg_str(realm, args, 0)?.trim().to_ascii_lowercase();
    let v = arg(args, 1);
    let value = if v.is_nullish() { String::new() } else { realm.to_rust_string(v)? };
    let prio = arg_str_opt(realm, args, 2)?.unwrap_or_default();
    set_style_prop(n, &prop, &value, prio.eq_ignore_ascii_case("important"));
    Ok(Value::Undefined)
}

fn style_get_property_value(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "getPropertyValue")?;
    let prop = arg_str(realm, args, 0)?.trim().to_ascii_lowercase();
    let list = parse_style(n);
    Ok(Value::str(list.items.iter().rev().find(|(k, _, _)| *k == prop).map(|(_, v, _)| v.as_str()).unwrap_or("")))
}

fn style_get_property_priority(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "getPropertyPriority")?;
    let prop = arg_str(realm, args, 0)?.trim().to_ascii_lowercase();
    let list = parse_style(n);
    Ok(Value::str(if list.items.iter().any(|(k, _, imp)| *k == prop && *imp) { "important" } else { "" }))
}

fn style_remove_property(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "removeProperty")?;
    let prop = arg_str(realm, args, 0)?.trim().to_ascii_lowercase();
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
        format!("rgba({}, {}, {}, {})", c.r, c.g, c.b, (c.a as f32 / 255.0 * 1000.0).round() / 1000.0)
    }
}

fn px(v: f32) -> String {
    format!("{}px", (v * 1000.0).round() / 1000.0)
}

fn length_css(l: &Length, base: f32) -> String {
    match l {
        Length::Px(v) => px(*v),
        Length::Percent(p) => px(base * p / 100.0),
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
    if n.doc != 0 {
        return None;
    }
    // Style first (cheap: computed on demand when the DOM changed); the
    // layout box only when the value actually depends on it, so scripts
    // that interleave style writes and reads don't force full layouts.
    let style = with_env(|env| env.computed_style(n.node));
    let s: std::rc::Rc<ComputedStyle> = style?;
    let pct = |l: &rabbithole_style::Length| matches!(l, rabbithole_style::Length::Percent(_));
    let needs_box = match prop {
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
        "top" => pct(&s.inset.top),
        "left" => pct(&s.inset.left),
        "right" => pct(&s.inset.right),
        "bottom" => pct(&s.inset.bottom),
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
    let r = if needs_box { with_env(|env| env.node_rect(n.node)).unwrap_or_default() } else { Default::default() };
    Some(match prop {
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
        "color" => color_css(s.color),
        "background-color" => color_css(s.background_color),
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
        "width" => px(r.w - s.border_width.left - s.border_width.right - s.padding.left.resolve_or_zero(r.w) - s.padding.right.resolve_or_zero(r.w)),
        "height" => px(r.h - s.border_width.top - s.border_width.bottom - s.padding.top.resolve_or_zero(r.w) - s.padding.bottom.resolve_or_zero(r.w)),
        "margin-top" => length_css(&s.margin.top, r.w),
        "margin-right" => length_css(&s.margin.right, r.w),
        "margin-bottom" => length_css(&s.margin.bottom, r.w),
        "margin-left" => length_css(&s.margin.left, r.w),
        "padding-top" => length_css(&s.padding.top, r.w),
        "padding-right" => length_css(&s.padding.right, r.w),
        "padding-bottom" => length_css(&s.padding.bottom, r.w),
        "padding-left" => length_css(&s.padding.left, r.w),
        "border-top-width" => px(s.border_width.top),
        "border-right-width" => px(s.border_width.right),
        "border-bottom-width" => px(s.border_width.bottom),
        "border-left-width" => px(s.border_width.left),
        "top" => length_css(&s.inset.top, r.h),
        "left" => length_css(&s.inset.left, r.w),
        "right" => length_css(&s.inset.right, r.w),
        "bottom" => length_css(&s.inset.bottom, r.h),
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
        "cursor" => if s.cursor_pointer { "pointer".into() } else { "auto".into() },
        "transform" => if s.transform.is_empty() { "none".into() } else { let m = rabbithole_style::transform_matrix(&s.transform, r.w, r.h, s.font_size); format!("matrix({}, {}, {}, {}, {}, {})", m[0], m[1], m[2], m[3], m[4], m[5]) },
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
        "border-radius" | "border-top-left-radius" => px(s.border_radius),
        "direction" => if s.direction == rabbithole_style::Direction::Rtl { "rtl".into() } else { "ltr".into() },
        "background-image" => s.background_image.as_ref().map(|u| format!("url(\"{u}\")")).unwrap_or_else(|| "none".into()),
        "min-width" => length_css(&s.min_width, r.w),
        "max-width" => if s.max_width.is_auto() { "none".into() } else { length_css(&s.max_width, r.w) },
        "min-height" => length_css(&s.min_height, r.h),
        "max-height" => if s.max_height.is_auto() { "none".into() } else { length_css(&s.max_height, r.h) },
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
        "animation-name" | "transition-property" => "none".into(),
        "animation-duration" | "transition-duration" | "transition-delay" | "animation-delay" => "0s".into(),
        _ => String::new(),
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

fn cs_set(_realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(false))
}

fn cs_get_property_value(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "getPropertyValue")?;
    let prop = arg_str(realm, args, 0)?.trim().to_ascii_lowercase();
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
    Ok(make_proxy(realm, target, &[("get", cs_get), ("set", cs_set)]))
}

// ---------------------------------------------------------------- matchMedia

fn mql_matches(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let media = realm.get_str(this, "media")?;
    let media = realm.to_rust_string(media)?;
    let q = rabbithole_css::parse_media_query(&media);
    let (w, h) = with_env(|env| env.viewport());
    Ok(Value::Bool(q.evaluate(&rabbithole_css::MediaEnv { width: w, height: h, screen: true })))
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

fn css_supports(_realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let text = match args.first() {
        Some(Value::String(s)) => s.to_rust_string().to_ascii_lowercase(),
        _ => String::new(),
    };
    Ok(Value::Bool(!(text.contains("container") || text.contains("grid-template-areas: none") || text.contains("backdrop-filter"))))
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
    if let Value::Object(id) = &o {
        realm.define_method(*id, "insertRule", 2, cs_zero);
        realm.define_method(*id, "deleteRule", 1, mql_noop);
        realm.define_method(*id, "replaceSync", 1, mql_noop);
        realm.define_method(*id, "replace", 1, mql_noop);
    }
    Ok(o)
}
