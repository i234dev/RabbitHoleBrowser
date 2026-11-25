//! CSSOM: `element.style`, `getComputedStyle`, `matchMedia`, `CSS`.

use crate::element::{get_attribute, make_proxy, set_attribute};
use rabbithole_js::V;
use crate::env::with_env;
use crate::host::*;
use rabbithole_css::{parse_declaration_list, Declaration, Token};
use rabbithole_js::{JsResult, ObjectId, Realm, Value};
use rabbithole_style::{ComputedStyle, Display, Length, Position};

mod alignment;
mod grid;
mod images;
mod shapes;

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

fn paired_longhands(name: &str) -> Option<[&'static str;2]> {
    match name {
        "place-items" => Some(["align-items","justify-items"]),
        "place-content" => Some(["align-content","justify-content"]),
        "place-self" => Some(["align-self","justify-self"]),
        "position-try" => Some(["position-try-order","position-try-fallbacks"]),
        _ => None,
    }
}

fn css_wide(value: &str) -> bool { matches!(value,"inherit" | "initial" | "unset" | "revert" | "revert-layer") }

fn expand_place(name: &str, value: &str) -> Option<[(&'static str,String);2]> {
    let [align,justify] = paired_longhands(name)?;
    if css_wide(value) { return Some([(align,value.into()),(justify,value.into())]); }
    let tokens = rabbithole_css::Tokenizer::tokenize(value);
    let parts = rabbithole_style::values::components(&tokens);
    if name == "position-try" {
        let (order,list) = rabbithole_style::props::anchor::TryOrder::shorthand(&parts)?;
        return Some([(align,order.css().into()), (justify,rabbithole_style::props::anchor::TryOption::list_css(&list,false))]);
    }
    let (a,b) = rabbithole_style::props::flex::specified_alignment_values(name,&parts)?;
    Some([(align,a),(justify,b?)])
}

/// Rule declarations use the same shorthand parser as inline declarations.
pub(crate) fn cssom_expand_place(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let name = arg_str(realm,args,0)?;
    let value = arg_str(realm,args,1)?;
    let Some([(a,av),(b,bv)]) = expand_place(&name,&value) else { return Ok(Value::Null) };
    Ok(realm.new_array_value(vec![Value::str(a),Value::str(&av),Value::str(b),Value::str(&bv)]))
}

/// The longhand values of a grid shorthand (CSS-wide keywords apply to
/// every longhand).
fn expand_grid(name: &str, value: &str) -> Option<Vec<(&'static str,String)>> {
    let longhands = rabbithole_style::props::grid_css::longhands(name)?;
    if css_wide(value) { return Some(longhands.iter().map(|l| (*l, value.to_string())).collect()); }
    rabbithole_style::props::grid_css::expand(name, &rabbithole_css::Tokenizer::tokenize(value))
}

impl DeclList {
    fn push(&mut self, name: String, value: String, important: bool) {
        if let Some(longhands) = expand_place(&name,&value) {
            self.items.extend(longhands.into_iter().map(|(k,v)| (k.into(),v,important)));
        } else if let Some(longhands) = expand_grid(&name,&value) {
            for (k,v) in longhands {
                self.items.retain(|(n,_,_)| n != k);
                self.items.push((k.into(),v,important));
            }
        } else {
            self.items.push((name,value,important));
        }
    }

    /// A grid shorthand from its longhands, when they can form one.
    fn grid_shorthand(&self, prop: &str) -> Option<(String,bool)> {
        let longhands = rabbithole_style::props::grid_css::longhands(prop)?;
        let values: Vec<(&str,bool)> = longhands.iter().map(|l| self.direct(l)).collect::<Option<_>>()?;
        let important = values[0].1;
        if values.iter().any(|(_,imp)| *imp != important) { return None; }
        if values.iter().any(|(v,_)| css_wide(v)) {
            return values.iter().all(|(v,_)| *v == values[0].0).then(|| (values[0].0.to_string(),important));
        }
        let get = |name: &str| longhands.iter().position(|l| *l == name).map(|i| values[i].0.to_string());
        rabbithole_style::props::grid_css::shorthand(prop, &get).map(|v| (v,important))
    }

    fn direct(&self, prop: &str) -> Option<(&str,bool)> {
        self.items.iter().rev().filter(|(k,_,_)| k==prop)
            .find(|(_,_,imp)| *imp).or_else(|| self.items.iter().rev().find(|(k,_,_)| k==prop))
            .map(|(_,v,imp)| (v.as_str(),*imp))
    }

    fn value(&self, prop: &str) -> (String,bool) {
        if let Some((v,imp)) = self.direct(prop) {
            if keeps_source_text(prop, v) {
                if let Some(serialized) = specified_color(prop, &rabbithole_css::Tokenizer::tokenize(v)) { return (serialized,imp); }
            }
            return (v.into(),imp);
        }
        if rabbithole_style::props::grid_css::longhands(prop).is_some() {
            return self.grid_shorthand(prop).unwrap_or_default();
        }
        if let Some([a,b]) = paired_longhands(prop) {
            if let (Some((a,pa)),Some((b,pb))) = (self.direct(a),self.direct(b)) {
                if pa==pb {
                    if prop == "position-try" && a == "normal" && !css_wide(b) { return (b.into(),pa); }
                    if a==b { return (a.into(),pa); }
                    if !css_wide(a) && !css_wide(b) { return (format!("{a} {b}"),pa); }
                }
            }
        }
        (String::new(),false)
    }

    fn css_text(&self) -> String {
        let mut out = String::new();
        let mut items = self.items.iter().peekable();
        while let Some((name,value,important)) = items.next() {
            let mut property = name.as_str();
            let mut text = value.clone();
            if let Some((next,other,priority)) = items.peek() {
                let shorthand = match (property,next.as_str()) {
                    ("position-try-order","position-try-fallbacks") => Some("position-try"),
                    ("align-items","justify-items") => Some("place-items"),
                    ("align-self","justify-self") => Some("place-self"),
                    ("align-content","justify-content") => Some("place-content"),
                    _ => None,
                };
                if let Some(shorthand) = shorthand.filter(|_| important==priority && (value==other || (!css_wide(value) && !css_wide(other)))) {
                    property = shorthand;
                    if shorthand == "position-try" && value == "normal" { text = other.clone(); }
                    else if value != other { text.push(' '); text.push_str(other); }
                    items.next();
                }
            }
            if !out.is_empty() { out.push(' '); }
            out.push_str(property); out.push_str(": "); out.push_str(&text);
            if *important { out.push_str(" !important"); }
            out.push(';');
        }
        out
    }

    fn remove(&mut self, prop: &str) {
        let longhands = paired_longhands(prop);
        let grid = rabbithole_style::props::grid_css::longhands(prop);
        self.items.retain(|(k,_,_)| k != prop && !longhands.is_some_and(|pair| pair.contains(&k.as_str()))
            && !grid.is_some_and(|l| l.contains(&k.as_str())));
    }
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

fn single_color_property(name: &str) -> bool {
    matches!(name, "color" | "background-color" | "border-top-color" | "border-right-color" |
        "border-bottom-color" | "border-left-color" | "outline-color" | "text-decoration-color" |
        "caret-color" | "accent-color" | "column-rule-color")
}

fn specified_color(name: &str, tokens: &[Token]) -> Option<String> {
    use rabbithole_style::values::{components, ident, parse_color_component, ColorValue};
    let parts = components(tokens);
    let [c] = parts.as_slice() else { return None };
    if let Some(k) = ident(c) {
        if matches!(k.as_str(), "inherit" | "unset" | "initial" | "revert" | "revert-layer")
            || (k == "auto" && matches!(name, "caret-color" | "accent-color"))
            || (k == "invert" && name == "outline-color")
            || parse_color_component(c).is_some() {
            return Some(k);
        }
        return None;
    }
    // Keep expressions in the specified value until their variables, units
    // and origin color can be resolved in the element's computed style.
    if let [Token::Function(f), ..] = *c {
        if f.eq_ignore_ascii_case("color-layers") {
            let (mode, layers) = rabbithole_style::layer_parts(c)?;
            let mut items = Vec::new();
            if let Some(mode) = mode.filter(|s| s != "normal") { items.push(mode); }
            for layer in layers { items.push(specified_color(name, layer)?); }
            return Some(format!("color-layers({})", items.join(", ")));
        }
        let deferred = c.iter().any(|t| matches!(t, Token::Function(f)
            if ["var", "env", "attr", "sibling-index", "sibling-count", "random", "progress", "if"].iter().any(|k| f.eq_ignore_ascii_case(k))));
        if deferred {
            let known = ["var", "env", "attr", "rgb", "rgba", "hsl", "hsla", "hwb", "lab", "lch", "oklab", "alpha",
                "oklch", "color", "light-dark", "color-mix", "contrast-color"].iter().any(|k| f.eq_ignore_ascii_case(k));
            return known.then(|| tokens_to_css(c));
        }
    }
    if let Some(spec) = rabbithole_style::color_model::Spec::parse(c) {
        return Some(spec.to_css());
    }
    let ColorValue::Color(rgb) = parse_color_component(c)? else { return None };
    Some(color_css(rgb))
}

/// Colors computed from other colors (relative colors, mixes, ...) keep
/// their source text in the declaration block: their serialization rounds
/// nested legacy colors to bytes, which must not change the computed value.
fn keeps_source_text(name: &str, value: &str) -> bool {
    use rabbithole_style::color_model::Spec;
    if !single_color_property(name) || !value.contains('(') { return false; }
    let tokens = rabbithole_css::Tokenizer::tokenize(value);
    let parts = rabbithole_style::values::components(&tokens);
    matches!(parts.as_slice(), [c] if matches!(Spec::parse(c), Some(Spec::Relative { .. } | Spec::Mix { .. } | Spec::Alpha(..) | Spec::LightDark(..) | Spec::Contrast(..))))
}

fn grid_property(name: &str) -> bool {
    rabbithole_style::props::grid_css::is_longhand(name) || rabbithole_style::props::grid_css::longhands(name).is_some()
}

fn checked_property(name: &str) -> bool {
    matches!(name, "shape-outside" | "shape-margin" | "position-area" | "position-try" | "position-try-order" | "position-try-fallbacks" | "position-anchor" | "anchor-name" | "anchor-scope") || rabbithole_style::props::motion::property(name) || single_color_property(name) || name == "opacity" || grid_property(name) || rabbithole_style::props::flex::alignment_property(name)
}

fn specified_value(name: &str, tokens: &[Token]) -> Option<String> {
    if let Some(result) = rabbithole_style::props::anchor_length::specified(name,tokens,&tokens_to_css) {
        return result;
    }
    if matches!(name, "shape-outside" | "shape-margin") {
        if rabbithole_style::cascade::supports_declaration(name,tokens) == Some(false) { return None; }
        if name == "shape-outside" { return Some(shapes::specified(tokens,"margin-box")); }
    }
    if rabbithole_style::props::motion::property(name) {
        if rabbithole_style::cascade::supports_declaration(name,tokens) != Some(true) { return None; }
        let parts = rabbithole_style::values::components(tokens);
        if parts.len() == 1 && matches!(parts[0], [Token::Ident(_)]) { return Some(tokens_to_css(tokens)); }
        if tokens.iter().any(|t| matches!(t,Token::Function(f) if ["var","env","attr"].iter().any(|n| f.eq_ignore_ascii_case(n)))) {
            return Some(tokens_to_css(tokens));
        }
        match name {
            "offset-path" => return Some(shapes::specified(tokens,"border-box")),
            "offset-anchor" | "offset-position" => return Some(shapes::specified_position(&parts)),
            "offset-rotate" if parts.len() == 2 && matches!(parts[1],[Token::Ident(_)]) => {
                return Some(format!("{} {}", tokens_to_css(parts[1]),tokens_to_css(parts[0])));
            }
            _ => {}
        }
    }
    if single_color_property(name) { return specified_color(name, tokens); }
    if grid_property(name) {
        let parts = rabbithole_style::values::components(tokens);
        if let [c] = parts.as_slice() {
            if let Some(k) = rabbithole_style::values::ident(c).filter(|k| css_wide(k)) { return Some(k); }
        }
        if tokens.iter().any(|t| matches!(t, Token::Function(f) if ["var","env","attr"].iter().any(|n| f.eq_ignore_ascii_case(n)))) {
            return Some(tokens_to_css(tokens));
        }
        if rabbithole_style::props::grid_css::is_longhand(name) {
            return rabbithole_style::props::grid_css::specified(name, tokens);
        }
        return rabbithole_style::props::grid_css::expand(name, tokens).map(|_| tokens_to_css(tokens));
    }
    if name == "opacity" {
        let parts = rabbithole_style::values::components(tokens);
        let [c] = parts.as_slice() else { return None };
        if let Some(k) = rabbithole_style::values::ident(c).filter(|k| matches!(k.as_str(), "inherit" | "initial" | "unset" | "revert" | "revert-layer")) {
            return Some(k);
        }
        // Functions resolved later (variables, tree counting, random
        // values, progress) keep their text.
        if c.iter().any(|t| matches!(t, Token::Function(f) if !rabbithole_style::color_calc::is_math_function(f))) {
            return Some(tokens_to_css(tokens));
        }
        return rabbithole_style::color_model::alpha_specified(c);
    }
    if matches!(name, "position-area" | "position-try" | "position-try-order" | "position-try-fallbacks" | "position-anchor" | "anchor-name" | "anchor-scope") || rabbithole_style::props::flex::alignment_property(name) {
        let parts = rabbithole_style::values::components(tokens);
        if let [value] = parts.as_slice() {
            if let Some(k) = rabbithole_style::values::ident(value).filter(|k| matches!(k.as_str(), "inherit" | "initial" | "unset" | "revert" | "revert-layer")) {
                return Some(k);
            }
        }
        if !tokens.iter().any(|t| matches!(t, Token::Function(f) if f.eq_ignore_ascii_case("var") || f.eq_ignore_ascii_case("env") || f.eq_ignore_ascii_case("attr"))) {
            if name == "anchor-scope" { return rabbithole_style::props::anchor::AnchorScope::parse(&parts).map(|scope| scope.to_css()); }
            if name == "anchor-name" { return rabbithole_style::props::anchor::names(&parts).map(|names| if names.is_empty() { "none".into() } else { names.join(", ") }); }
            if name == "position-anchor" {
                let [c] = parts.as_slice() else { return None };
                return if rabbithole_style::values::is_ident(c,"auto") { Some("auto".into()) } else { rabbithole_style::props::anchor::dashed_ident(c).map(str::to_owned) };
            }
            if name == "position-try" { return rabbithole_style::props::anchor::TryOrder::shorthand(&parts).map(|(order,list)| order.shorthand_css(&list,false)); }
            if name == "position-try-order" {
                let [part] = parts.as_slice() else { return None };
                return rabbithole_style::props::anchor::TryOrder::parse(part).map(|o| o.css().into());
            }
            if name == "position-try-fallbacks" { use rabbithole_style::props::anchor::TryOption; return TryOption::parse_list(&parts).map(|v| TryOption::list_css(&v,false)); }
            if name == "position-area" { return rabbithole_style::props::anchor::PositionArea::parse(&parts).map(|a| a.to_css(false)); }
            return rabbithole_style::props::flex::specified_alignment(name, &parts);
        }
    }
    Some(tokens_to_css(tokens))
}

pub(crate) fn cssom_serialize_selector(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let selector = arg_str(realm, args, 0)?;
    let namespaces = rabbithole_css::Tokenizer::tokenize(&arg_str(realm, args, 1)?);
    Ok(Value::str(&rabbithole_css::selector::serialize_type_selectors(&selector, &namespaces)))
}

pub(crate) fn cssom_keyframe_selector(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let text = arg_str(realm, args, 0)?;
    let tokens = rabbithole_css::Tokenizer::tokenize(&text);
    let Some(keys) = rabbithole_css::keyframe_selectors(&tokens) else { return Ok(Value::Null) };
    let keys: Vec<_> = keys.iter().map(|k| {
        let percentage = format!("{}%", if k.percentage == 0.0 { 0.0 } else { k.percentage });
        match &k.range { Some(name) => format!("{name} {percentage}"), None => percentage }
    }).collect();
    Ok(Value::str(&keys.join(", ")))
}

pub(crate) fn cssom_valid_selector(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let selector = arg_str(realm, args, 0)?;
    let nested = realm.to_boolean(&arg(args, 1));
    let tokens = rabbithole_css::Tokenizer::tokenize(&selector);
    let declarations = arg_str(realm, args, 2)?;
    let declarations = rabbithole_css::Tokenizer::tokenize(&declarations);
    Ok(Value::Bool(rabbithole_css::selector::valid_rule_selector_with_namespaces(&tokens, nested, &declarations)))
}

pub(crate) fn cssom_try_name(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let tokens = rabbithole_css::Tokenizer::tokenize(&arg_str(realm, args, 0)?);
    Ok(rabbithole_css::position_try::name(&tokens).map_or(Value::Null,Value::str))
}

pub(crate) fn cssom_try_descriptors(realm: &mut Realm, _: Value, _: &[Value]) -> JsResult<Value> {
    Ok(realm.new_array_value(rabbithole_css::position_try::DESCRIPTORS.iter().map(|name| Value::str(name)).collect()))
}

pub(crate) fn cssom_namespace(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let tokens = rabbithole_css::Tokenizer::tokenize(&arg_str(realm, args, 0)?);
    Ok(match rabbithole_css::parse_namespace(&tokens) {
        Some((prefix, uri)) => realm.new_array_value(vec![Value::str(prefix), Value::str(uri)]),
        None => Value::Null,
    })
}

pub(crate) fn cssom_specified_value(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let name = arg_str(realm, args, 0)?;
    let value = arg_str(realm, args, 1)?;
    if !checked_property(&name) && !might_use_anchor(&value) { return Ok(Value::str(&value)); }
    let tokens = rabbithole_css::Tokenizer::tokenize(&value);
    Ok(specified_value(&name, &tokens).map_or(Value::Null, |s| Value::str(&s)))
}

fn parse_style(n: NodeRef) -> DeclList {
    let text = get_attribute(n, "style").unwrap_or_default();
    let decls: Vec<Declaration> = parse_declaration_list(&text);
    let mut list = DeclList { items: Vec::with_capacity(decls.len()) };
    for d in decls {
        if let Some(value) = specified_value(&d.name, &d.value) {
            // (Only color functions can keep their text: other declarations
            // are not serialized a second time on every parse.)
            let keeps = single_color_property(&d.name) && d.value.iter().any(|t| matches!(t, Token::Function(_)));
            let value = if keeps && keeps_source_text(&d.name, &tokens_to_css(&d.value)) { tokens_to_css(&d.value) } else { value };
            list.push(d.name,value,d.important);
        }
    }
    list
}

fn write_style(n: NodeRef, list: &DeclList) {
    if list.items.is_empty() {
        if get_attribute(n, "style").is_some() {
            set_attribute(n, "style", Some(""));
        }
        return;
    }
    let text = list.css_text();
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
    Ok(Value::str(&list.value(&prop).0))
}

fn might_use_anchor(value: &str) -> bool {
    value.as_bytes().windows(6).any(|w| w.eq_ignore_ascii_case(b"anchor"))
}

fn set_style_prop(n: NodeRef, prop: &str, value: &str, important: bool) {
    let value = value.trim();
    let serialized;
    let value = if !value.is_empty() && (checked_property(prop) || might_use_anchor(value)) {
        let Some(v) = specified_value(prop, &rabbithole_css::Tokenizer::tokenize(value)) else { return };
        serialized = if keeps_source_text(prop, value) { value.to_string() } else { v };
        serialized.as_str()
    } else { value };
    let mut list = parse_style(n);
    list.remove(prop);
    if !value.is_empty() {
        list.push(prop.to_string(), value.to_string(), important);
    }
    write_style(n, &list);
}

fn style_set(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let n = style_target(realm, args)?;
    let Some(key) = key_of(realm, args) else { return Ok(Value::Bool(true)) };
    let v = arg(args, 2);
    let value = if v.is_null() { String::new() } else { realm.to_rust_string(v)? };
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
    if args.len() < 2 { return Err(realm.throw_type_error("setProperty requires a name and a value")); }
    let prop = prop_name(arg_str(realm, args, 0)?.trim());
    let v = arg(args, 1);
    let value = if v.is_null() { String::new() } else { realm.to_rust_string(v)? };
    let priority = arg(args, 2);
    let prio = if priority.is_nullish() { String::new() } else { realm.to_rust_string(priority)? };
    if value.is_empty() || prio.is_empty() || prio.eq_ignore_ascii_case("important") {
        set_style_prop(n, &prop, &value, !prio.is_empty());
    }
    Ok(Value::Undefined)
}

fn style_get_property_value(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "getPropertyValue")?;
    let prop = prop_name(arg_str(realm, args, 0)?.trim());
    let list = parse_style(n);
    Ok(Value::str(&list.value(&prop).0))
}

fn style_get_property_priority(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "getPropertyPriority")?;
    let prop = prop_name(arg_str(realm, args, 0)?.trim());
    let list = parse_style(n);
    Ok(Value::str(if list.value(&prop).1 { "important" } else { "" }))
}

fn style_remove_property(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "removeProperty")?;
    let prop = prop_name(arg_str(realm, args, 0)?.trim());
    let list = parse_style(n);
    let old = list.value(&prop).0;
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

fn anchor_style(node: NodeRef, prop: &str, frame: Option<u64>, style: std::rc::Rc<ComputedStyle>) -> std::rc::Rc<ComputedStyle> {
    if (rabbithole_style::props::anchor_length::property(prop).is_some() && style.has_anchor_lengths())
        || (style.position_try_fallbacks.is_some() && rabbithole_css::position_try::DESCRIPTORS.contains(&prop)) {
        return with_env(|env| env.anchor_style(frame,node.node)).unwrap_or(style);
    }
    style
}

/// Computed value of `prop` for a node, as a CSS string.
pub(crate) fn computed_value(n: NodeRef, prop: &str) -> Option<String> {
    if n.doc & crate::host::FRAME_DOC != 0 {
        // A same-origin frame's document. Grid templates have a dedicated
        // final-layout query; the other existing frame values use style.
        let frame = (n.doc & !crate::host::FRAME_DOC) as u64;
        let s = with_env(|env| env.frame_computed_style(frame, n.node))?;
        let s = anchor_style(n,prop,Some(frame),s);
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
    let s = anchor_style(n,prop,None,style?);
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
        st.map_or(s, |style| anchor_style(n,prop,None,style))
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
    // Percentage margins and paddings resolve to their used values (of
    // grid items, against the grid area).
    let edge = match prop {
        "margin-top" => Some((true, 0)), "margin-right" => Some((true, 1)), "margin-bottom" => Some((true, 2)), "margin-left" => Some((true, 3)),
        "padding-top" => Some((false, 0)), "padding-right" => Some((false, 1)), "padding-bottom" => Some((false, 2)), "padding-left" => Some((false, 3)),
        _ => None,
    };
    if let Some((margin, i)) = edge.filter(|_| needs_box) {
        if let Some((m, p)) = with_env(|env| env.used_box_edges(n.node)) {
            return Some(px(if margin { m[i] } else { p[i] }));
        }
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
    if !value.is_auto() && !value.is_anchor_query() { return Some(length_css(value, base)); }
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
    if let Some(value) = alignment::computed(s, prop) { return value; }
    match prop {
        "grid-template-columns" => grid::computed(&s.grid_template_columns),
        "grid-template-rows" => grid::computed(&s.grid_template_rows),
        "grid-auto-columns" => grid::auto_tracks(&s.grid_auto_columns),
        "grid-auto-rows" => grid::auto_tracks(&s.grid_auto_rows),
        "grid-auto-flow" => rabbithole_style::props::grid_css::auto_flow_css(s.grid_auto_flow).into(),
        "grid-template-areas" => rabbithole_style::props::grid_css::areas_computed(&s.grid_template_areas),
        "grid-row-start" => rabbithole_style::props::grid_css::line_css(&s.grid_row_start),
        "grid-row-end" => rabbithole_style::props::grid_css::line_css(&s.grid_row_end),
        "grid-column-start" => rabbithole_style::props::grid_css::line_css(&s.grid_column_start),
        "grid-column-end" => rabbithole_style::props::grid_css::line_css(&s.grid_column_end),
        "grid-lanes-direction" => rabbithole_style::props::grid_css::lanes_direction_css(&s.grid_lanes_direction),
        "grid-lanes-pack" => if s.grid_lanes_dense { "dense" } else { "normal" }.into(),
        "fit-tolerance" | "flow-tolerance" => match s.fit_tolerance {
            None => "normal".into(),
            Some(rabbithole_style::Length::Px(v)) if v.is_infinite() => "infinite".into(),
            Some(l) => position_css(&l),
        },
        "grid-row" | "grid-column" | "grid-area" | "grid-lanes" | "grid-template" | "grid" => {
            let get = |l: &str| Some(computed_from_style(s, l, r, cb));
            rabbithole_style::props::grid_css::shorthand(prop, &get).unwrap_or_default()
        }
        "display" => display_css(s.display).to_string(),
        "column-count" => s.column_count.map_or_else(|| "auto".into(), |n| n.to_string()),
        "column-width" => s.column_width.map_or_else(|| "auto".into(), px),
        "columns" => {
            let wc = match (s.column_width, s.column_count) {
                (None, None) => "auto".into(),
                (Some(w), None) => px(w),
                (None, Some(n)) => n.to_string(),
                (Some(w), Some(n)) => format!("{} {n}", px(w)),
            };
            match s.column_height {
                Some(h) => format!("{wc} / {}", px(h)),
                None => wc,
            }
        }
        "column-height" => s.column_height.map_or_else(|| "auto".into(), px),
        "column-wrap" => ["auto", "nowrap", "wrap"][s.column_wrap.min(2) as usize].into(),
        "column-fill" => if s.column_fill_auto { "auto" } else { "balance" }.into(),
        "column-span" => if s.column_span_all { "all" } else { "none" }.into(),
        "column-rule-width" => px(s.column_rule_width),
        "column-rule-style" => rabbithole_style::props::multicol::rule_style_css(s.column_rule_style_kw).into(),
        "column-rule-color" => color_css(s.column_rule_color.unwrap_or(s.color)),
        // (Lists of gap decorations have no shorthand form.)
        "column-rule" if s.column_rule_list != 0 => String::new(),
        "column-rule" => {
            let kw = rabbithole_style::props::multicol::rule_style_css(s.column_rule_style_kw);
            let color = color_css(s.column_rule_color.unwrap_or(s.color));
            if kw == "none" { format!("{} {color}", px(s.column_rule_width)) } else { format!("{} {kw} {color}", px(s.column_rule_width)) }
        }
        "break-before" => rabbithole_style::props::multicol::break_css(s.break_before).into(),
        "break-after" => rabbithole_style::props::multicol::break_css(s.break_after).into(),
        "break-inside" => rabbithole_style::props::multicol::break_css(s.break_inside).into(),
        "orphans" => s.orphans.to_string(),
        "widows" => s.widows.to_string(),
        "column-gap" => {
            if s.column_gap_normal { "normal".into() } else { length_css(&s.column_gap, cb.w) }
        }
        "shape-outside" => shapes::computed(s.shape_outside.as_deref(), s.shape_outside_position_explicit, rabbithole_style::ShapeBox::Margin),
        "shape-margin" => position_css(&s.shape_margin),
        "offset-path" => shapes::computed(s.motion().path.as_deref(),s.motion().explicit_center,rabbithole_style::ShapeBox::Border),
        "offset-distance" => position_css(&s.motion().distance),
        "offset-rotate" => {
            let v = s.motion().rotate;
            if v.follows_path && v.degrees == 0.0 { "auto".into() }
            else { format!("{}{}deg", if v.follows_path { "auto " } else { "" }, fmt_num(v.degrees)) }
        }
        "offset-position" | "offset-anchor" => {
            use rabbithole_style::props::motion::OffsetPosition;
            match if prop == "offset-position" { s.motion().position } else { s.motion().anchor } {
                OffsetPosition::Auto => "auto".into(), OffsetPosition::Normal => "normal".into(),
                OffsetPosition::Coordinates(x,y) => format!("{} {}",position_css(&x),position_css(&y)),
            }
        }
        "position-try" => s.position_try_order.shorthand_css(s.position_try_fallbacks.as_deref().unwrap_or_default(),true),
        "position-try-order" => s.position_try_order.css().into(),
        "position-try-fallbacks" => rabbithole_style::props::anchor::TryOption::list_css(s.position_try_fallbacks.as_deref().unwrap_or_default(),true),
        "position-area" => s.position_area.to_css(true),
        "anchor-scope" => s.anchor_scope.to_css(),
        "anchor-name" => s.anchor_names.as_ref().map_or_else(|| "none".into(),|names| names.join(", ")),
        "position-anchor" => s.position_anchor.as_deref().unwrap_or("auto").into(),
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
        "list-style-type" => {
            use rabbithole_style::ListStyleType::*;
            match s.list_style_type {
                None => "none", Disc => "disc", Circle => "circle", Square => "square",
                Decimal => "decimal", DecimalLeadingZero => "decimal-leading-zero",
                LowerAlpha => "lower-alpha", UpperAlpha => "upper-alpha",
                LowerRoman => "lower-roman", UpperRoman => "upper-roman",
            }.into()
        }
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
        "background-image" => images::layers(&s.background_layers),
        "mask-image" => images::layers(&s.mask_layers),
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
        "transition-behavior" => list(s.transitions.iter().map(|t| if t.allow_discrete { "allow-discrete".into() } else { "normal".into() }).collect(), "normal"),
        "transition-duration" => list(s.transitions.iter().map(|t| secs(t.duration)).collect(), "0s"),
        "transition-delay" => list(s.transitions.iter().map(|t| secs(t.delay)).collect(), "0s"),
        "transition-timing-function" => list(s.transitions.iter().map(|t| t.timing.clone()).collect(), "ease"),
        "animation-composition" => list(s.animation_composition.as_deref().unwrap_or_default().iter().map(|op| op.css().to_string()).collect(), "replace"),
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


/// A retained computed declaration observes stylesheet edits in the same task.
fn read_computed_value(realm: &mut Realm, node: NodeRef, property: &str) -> JsResult<Value> {
    let flush = {
        let host = state();
        let host = host.borrow();
        if host.render.sheets_pending { host.factories.get("flushSheets").cloned() } else { None }
    };
    if let Some(flush) = flush { realm.call(flush, Value::Undefined, &[])?; }
    Ok(Value::str(&computed_value(node, property).unwrap_or_default()))
}

/// `domEpoch(node)`: the mutation counter of the node's document, which
/// changes whenever its tree, an attribute or text changes.
pub(crate) fn dom_epoch_native(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(n) = node_of(realm, &arg(args, 0)) else { return Ok(Value::Number(-1.0)) };
    Ok(Value::Number(crate::host::with_doc(n.doc, |doc| doc.epoch()) as f64))
}

/// `computedValue(element, property)`: one computed style value, as
/// `getComputedStyle(element).getPropertyValue(property)` (for the editing
/// algorithms, which read many).
pub(crate) fn computed_value_native(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(n) = node_of(realm, &arg(args, 0)) else {
        return Err(realm.throw_type_error("computedValue: parameter 1 is not of type 'Element'."));
    };
    let prop = arg_str(realm, args, 1)?;
    read_computed_value(realm, n, &prop)
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
    read_computed_value(realm, n, &prop)
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
    read_computed_value(realm, n, &prop)
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

/// Legacy HTML editing commands use an absolute CSS color, serialized for
/// a style declaration or converted to an opaque HTML font color.
pub(crate) fn editing_color(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let text = arg_str(realm, args, 0)?;
    let tokens = rabbithole_css::Tokenizer::tokenize(&text);
    Ok(match rabbithole_style::values::parse_color(&tokens) {
        Some(rabbithole_style::values::ColorValue::Color(c)) => Value::str(&color_css(c)),
        _ => Value::Null,
    })
}

/// A canvas color: `[r, g, b, a]` with sRGB bytes, colors outside sRGB
/// gamut mapped (CSS Color 4 §13.2); null when invalid.
pub(crate) fn canvas_color(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    use rabbithole_style::color_model::{gamut_map_srgb, Spec};
    let text = arg_str(realm, args, 0)?;
    let tokens = rabbithole_css::Tokenizer::tokenize(&text);
    let parts = rabbithole_style::values::components(&tokens);
    let [c] = parts.as_slice() else { return Ok(Value::Null) };
    let Some(spec) = Spec::parse(c) else { return Ok(Value::Null) };
    let current = rabbithole_style::color_model::Abs::from_gfx(rabbithole_gfx::Color::BLACK);
    let Some(abs) = spec.resolve(Some(&current)) else { return Ok(Value::Null) };
    let rgb = if abs.legacy { let g = abs.to_gfx(); [g.r, g.g, g.b].map(|v| f64::from(v) / 255.0) } else { gamut_map_srgb(&abs) };
    let byte = |v: f64| Value::Number((v * 255.0).round().clamp(0.0, 255.0));
    Ok(realm.new_array_value(vec![byte(rgb[0]), byte(rgb[1]), byte(rgb[2]), Value::Number(abs.alpha_value().clamp(0.0, 1.0))]))
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
