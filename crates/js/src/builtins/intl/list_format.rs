//! `Intl.ListFormat` (ECMA-402 §13) with CLDR list patterns.

use super::duration::{get_options_object, get_string_option};
use super::*;

#[derive(Debug, Clone)]
pub struct ListOptions {
    pub(super) locale: Locale,
    pub(super) kind: String,
    pub(super) style: String,
}

/// List patterns: (pair, start, middle, end) separators placed between
/// elements (`{0}<sep>{1}`).
struct Patterns {
    two: &'static str,
    start: &'static str,
    middle: &'static str,
    end: &'static str,
}

fn patterns(lang: &str, kind: &str, style: &str) -> Patterns {
    let p = |two, start, middle, end| Patterns { two, start, middle, end };
    match (lang, kind, style) {
        (_, "unit", "narrow") => match lang {
            "es" | "en" | "de" | "fr" | "it" | "pt" => p(" ", " ", " ", " "),
            _ => p(" ", " ", " ", " "),
        },
        ("en", "conjunction", "long") => p(" and ", ", ", ", ", ", and "),
        ("en", "conjunction", "short") => p(" & ", ", ", ", ", ", & "),
        ("en", "conjunction", _) => p(", ", ", ", ", ", ", "),
        ("en", "disjunction", _) => p(" or ", ", ", ", ", ", or "),
        ("en", "unit", _) => p(", ", ", ", ", ", ", "),
        ("es", "conjunction", _) => p(" y ", ", ", ", ", " y "),
        ("es", "disjunction", _) => p(" o ", ", ", ", ", " o "),
        ("es", "unit", "long") => p(" y ", ", ", ", ", " y "),
        ("es", "unit", _) => p(" y ", ", ", ", ", ", "),
        ("de", "disjunction", _) => p(" oder ", ", ", ", ", " oder "),
        ("de", "unit", _) => p(", ", ", ", ", ", " und "),
        ("de", _, _) => p(" und ", ", ", ", ", " und "),
        ("fr", "disjunction", _) => p(" ou ", ", ", ", ", " ou "),
        ("fr", "unit", _) => p(" et ", ", ", ", ", " et "),
        ("fr", _, _) => p(" et ", ", ", ", ", " et "),
        ("it", "disjunction", _) => p(" o ", ", ", ", ", " o "),
        ("it", _, _) => p(" e ", ", ", ", ", " e "),
        ("pt", "disjunction", _) => p(" ou ", ", ", ", ", " ou "),
        ("pt", _, _) => p(" e ", ", ", ", ", " e "),
        (_, "disjunction", _) => p(" or ", ", ", ", ", ", or "),
        (_, "unit", _) => p(", ", ", ", ", ", ", "),
        _ => p(" and ", ", ", ", ", ", and "),
    }
}

/// CreatePartsFromList: element / literal parts.
pub(super) fn list_parts(items: &[String], o: &ListOptions) -> Vec<(&'static str, String)> {
    let p = patterns(&o.locale.lang, &o.kind, &o.style);
    let n = items.len();
    let mut out = Vec::new();
    for (i, it) in items.iter().enumerate() {
        if i > 0 {
            let sep = if n == 2 {
                p.two
            } else if i == n - 1 {
                p.end
            } else if i == 1 {
                p.start
            } else {
                p.middle
            };
            out.push(("literal", sep.to_string()));
        }
        out.push(("element", it.clone()));
    }
    out
}

pub fn format_list(items: &[String], o: &ListOptions) -> String {
    list_parts(items, o).into_iter().map(|(_, s)| s).collect()
}

pub(super) fn ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if realm.new_target().is_undefined() {
        return realm.type_error("Constructor Intl.ListFormat requires 'new'");
    }
    let locale = lookup_locale(realm, arg(args, 0))?;
    let opts = get_options_object(realm, arg(args, 1))?;
    get_string_option(realm, opts, "localeMatcher", &["lookup", "best fit"], Some("best fit"))?;
    let kind = get_string_option(realm, opts, "type", &["conjunction", "disjunction", "unit"], Some("conjunction"))?.unwrap();
    let style = get_string_option(realm, opts, "style", &["long", "short", "narrow"], Some("long"))?.unwrap();
    Ok(Value::Object(new_intl_object(realm, "Intl.ListFormat", ListOptions { locale, kind, style })?))
}

/// StringListFromIterable: TypeError (closing the iterator) on a
/// non-String element.
fn string_list(realm: &mut Realm, v: Value) -> JsResult<Vec<String>> {
    if v.is_undefined() {
        return Ok(Vec::new());
    }
    let (it, next) = realm.get_iterator(v)?;
    let mut out = Vec::new();
    while let Some(item) = realm.iterator_step(&it, &next)? {
        match item.v() {
            V::String(s) => out.push(s.to_rust_string()),
            _ => {
                let e = realm.new_error("TypeError", "Iterable yielded a non-String value");
                let _ = realm.iterator_close(&it);
                return Err(e);
            }
        }
    }
    Ok(out)
}

pub(super) fn format(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o: ListOptions = host(realm, &this, "Intl.ListFormat.prototype.format")?;
    let items = string_list(realm, arg(args, 0))?;
    Ok(Value::str(&format_list(&items, &o)))
}

pub(super) fn format_to_parts(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o: ListOptions = host(realm, &this, "Intl.ListFormat.prototype.formatToParts")?;
    let items = string_list(realm, arg(args, 0))?;
    let mut out = Vec::new();
    for (t, v) in list_parts(&items, &o) {
        let obj = realm.new_object();
        realm.define_property(obj, "type", Value::str(t), ALL);
        realm.define_property(obj, "value", Value::str(&v), ALL);
        out.push(Value::Object(obj));
    }
    Ok(realm.new_array_value(out))
}

pub(super) fn resolved_options(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let o: ListOptions = host(realm, &this, "Intl.ListFormat.prototype.resolvedOptions")?;
    let obj = realm.new_object();
    realm.define_property(obj, "locale", Value::str(&o.locale.tag), ALL);
    realm.define_property(obj, "type", Value::str(&o.kind), ALL);
    realm.define_property(obj, "style", Value::str(&o.style), ALL);
    Ok(Value::Object(obj))
}
