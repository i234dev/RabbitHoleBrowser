//! Content-attribute reflection (HTML "Reflecting content attributes in IDL
//! attributes"): the getter/setter algorithms for every reflected type and
//! the enumerated attributes' keywords and defaults, which the WebIDL
//! alone does not carry.

use crate::host::*;
use rabbithole_js::{JsResult, ObjectId, Realm, Value, CONFIGURABLE, ENUMERABLE};
use std::rc::Rc;

const ACC: u8 = CONFIGURABLE | ENUMERABLE;
const MAX_INT: i64 = 2147483647;

/// An enumerated attribute limited to only known values.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EnumSpec {
    /// (keyword, canonical keyword of its state)
    pub keywords: Vec<(&'static str, &'static str)>,
    /// Keyword of the missing value default state (`None`: no state).
    pub missing: Option<&'static str>,
    /// Keyword of the invalid value default state (`None`: no state).
    pub invalid: Option<&'static str>,
    /// `DOMString?`: no state (and setting null removes the attribute).
    pub nullable: bool,
}

impl EnumSpec {
    pub fn new(keywords: &[&'static str], missing: Option<&'static str>, invalid: Option<&'static str>) -> EnumSpec {
        EnumSpec { keywords: keywords.iter().map(|k| (*k, *k)).collect(), missing, invalid, nullable: false }
    }

    /// The canonical keyword of the state `value` (a content attribute
    /// value, or None when absent) represents; None when it has no state.
    pub fn state(&self, value: Option<&str>) -> Option<&'static str> {
        match value {
            None => self.missing,
            Some(v) => self.keywords.iter().find(|k| k.0.eq_ignore_ascii_case(v)).map(|k| k.1).or(self.invalid),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Reflect {
    Str,
    /// `[LegacyNullToEmptyString] DOMString`: null sets "".
    StrNullEmpty,
    NullableStr,
    Popover,
    Bool,
    /// `long` with its default.
    Long(i32),
    /// `long` limited to only non-negative numbers (default usually -1).
    NonNegLong(i32),
    /// `unsigned long` with its default.
    ULong(u32),
    /// `unsigned long` limited to only positive numbers (IndexSizeError on 0).
    PosULong(u32),
    /// `unsigned long` limited to only positive numbers with fallback.
    PosULongFallback(u32),
    /// `unsigned long` clamped to [min, max]: (default, min, max).
    Clamped(u32, u32, u32),
    /// `double` with its default.
    Double(f64),
    /// `double` limited to only positive numbers, with its default.
    PosDouble(f64),
    /// A URL resolved against the document base.
    Url,
    /// Like `Url`, but the document URL when absent or empty (`formAction`).
    UrlOrDocument,
    Enum(Rc<EnumSpec>),
}

pub(crate) struct AttrSpec {
    pub attr: String,
    pub kind: Reflect,
}

fn is_ascii_ws(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0c' | '\r' | ' ')
}

/// The rules for parsing integers (saturating far beyond the 32-bit
/// ranges that matter to callers).
pub(crate) fn parse_integer(s: &str) -> Option<i64> {
    let s = s.trim_start_matches(is_ascii_ws);
    let (neg, s) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let digits = s.bytes().take_while(u8::is_ascii_digit);
    let mut n: i64 = 0;
    let mut any = false;
    for d in digits {
        any = true;
        n = n.saturating_mul(10).saturating_add((d - b'0') as i64);
    }
    if !any {
        return None;
    }
    Some(if neg { -n } else { n })
}

/// The rules for parsing non-negative integers.
pub(crate) fn parse_non_negative(s: &str) -> Option<i64> {
    parse_integer(s).filter(|n| *n >= 0)
}

/// The rules for parsing floating-point number values.
pub(crate) fn parse_float(s: &str) -> Option<f64> {
    let b = s.trim_start_matches(is_ascii_ws).as_bytes();
    let mut i = 0;
    let mut text = String::new();
    match b.first() {
        Some(b'-') => {
            text.push('-');
            i += 1;
        }
        Some(b'+') => i += 1,
        _ => {}
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == int_start {
        // A fraction without integer digits (".5").
        if !(b.get(i) == Some(&b'.') && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            return None;
        }
        text.push('0');
    } else {
        text.push_str(std::str::from_utf8(&b[int_start..i]).ok()?);
    }
    if b.get(i) == Some(&b'.') && b.get(i + 1).is_some_and(u8::is_ascii_digit) {
        let start = i;
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        text.push_str(std::str::from_utf8(&b[start..i]).ok()?);
    } else if b.get(i) == Some(&b'.') {
        i += 1;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        let mut exp = String::from("e");
        match b.get(j) {
            Some(b'-') => {
                exp.push('-');
                j += 1;
            }
            Some(b'+') => j += 1,
            _ => {}
        }
        let start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > start {
            exp.push_str(std::str::from_utf8(&b[start..j]).ok()?);
            text.push_str(&exp);
        }
    }
    let v: f64 = text.parse().ok()?;
    if v.is_infinite() {
        return None;
    }
    Some(if v == 0.0 { 0.0 } else { v })
}

fn url_value(raw: Option<String>) -> Value {
    match raw {
        Some(r) => {
            let trimmed = r.trim_matches(|c: char| c <= ' ');
            Value::str(&base_url().join(trimmed).map(|u| u.to_string()).unwrap_or(r))
        }
        None => Value::str(""),
    }
}

fn reflect_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let spec = native_data::<AttrSpec>(realm).expect("attr spec");
    let n = this_node(realm, &this, &spec.attr)?;
    let raw = crate::element::get_attribute(n, &spec.attr);
    Ok(match &spec.kind {
        Reflect::Str | Reflect::StrNullEmpty => Value::str(raw.as_deref().unwrap_or("")),
        Reflect::Popover => raw.as_deref().map(rabbithole_dom::Document::popover_mode).map(Value::str).unwrap_or(Value::Null),
        Reflect::NullableStr => raw.as_deref().map(Value::str).unwrap_or(Value::Null),
        Reflect::Bool => Value::Bool(raw.is_some()),
        Reflect::Long(d) => {
            let v = raw.as_deref().and_then(parse_integer).filter(|v| (-MAX_INT - 1..=MAX_INT).contains(v));
            Value::Number(v.unwrap_or(*d as i64) as f64)
        }
        Reflect::NonNegLong(d) => {
            let v = raw.as_deref().and_then(parse_non_negative).filter(|v| *v <= MAX_INT);
            Value::Number(v.unwrap_or(*d as i64) as f64)
        }
        Reflect::ULong(d) => {
            let v = raw.as_deref().and_then(parse_non_negative).filter(|v| *v <= MAX_INT);
            Value::Number(v.unwrap_or(*d as i64) as f64)
        }
        Reflect::PosULong(d) | Reflect::PosULongFallback(d) => {
            let v = raw.as_deref().and_then(parse_non_negative).filter(|v| (1..=MAX_INT).contains(v));
            Value::Number(v.unwrap_or(*d as i64) as f64)
        }
        Reflect::Clamped(d, min, max) => Value::Number(match raw.as_deref().and_then(parse_non_negative) {
            None => *d as f64,
            Some(v) => v.clamp(*min as i64, *max as i64) as f64,
        }),
        Reflect::Double(d) => Value::Number(raw.as_deref().and_then(parse_float).unwrap_or(*d)),
        Reflect::PosDouble(d) => Value::Number(raw.as_deref().and_then(parse_float).filter(|v| *v > 0.0).unwrap_or(*d)),
        Reflect::Url => url_value(raw),
        Reflect::UrlOrDocument => match raw {
            Some(r) if !r.is_empty() => url_value(Some(r)),
            _ => Value::str(&crate::env::with_env(|env| env.document_url()).to_string()),
        },
        Reflect::Enum(e) => match e.state(raw.as_deref()) {
            Some(k) => Value::str(k),
            None if e.nullable => Value::Null,
            None => Value::str(""),
        },
    })
}

fn index_size_error(realm: &mut Realm, msg: &str) -> Value {
    crate::misc::dom_exception(realm, "IndexSizeError", msg)
}

fn reflect_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let spec = native_data::<AttrSpec>(realm).expect("attr spec");
    let n = this_node(realm, &this, &spec.attr)?;
    let v = arg(args, 0);
    let set = |s: &str| crate::element::set_attribute(n, &spec.attr, Some(s));
    match &spec.kind {
        Reflect::Bool => {
            let b = realm.to_boolean(&v);
            crate::element::set_attribute(n, &spec.attr, if b { Some("") } else { None });
        }
        Reflect::NullableStr | Reflect::Popover if v.is_nullish() => {
            if crate::element::get_attribute(n, &spec.attr).is_some() {
                crate::element::set_attribute(n, &spec.attr, None);
            }
        }
        Reflect::StrNullEmpty if matches!(v.v(), rabbithole_js::V::Null) => set(""),
        Reflect::Enum(e) if e.nullable && v.is_nullish() => {
            if crate::element::get_attribute(n, &spec.attr).is_some() {
                crate::element::set_attribute(n, &spec.attr, None);
            }
        }
        Reflect::Long(_) => {
            let x = realm.to_int32(v)?;
            set(&x.to_string());
        }
        Reflect::NonNegLong(_) => {
            let x = realm.to_int32(v)?;
            if x < 0 {
                return Err(index_size_error(realm, &format!("The value provided ({x}) is negative.")));
            }
            set(&x.to_string());
        }
        Reflect::ULong(d) | Reflect::Clamped(d, _, _) => {
            let x = realm.to_uint32(v)? as i64;
            set(&(if x <= MAX_INT { x } else { *d as i64 }).to_string());
        }
        Reflect::PosULong(d) => {
            let x = realm.to_uint32(v)? as i64;
            if x == 0 {
                return Err(index_size_error(realm, "The value provided is 0, which is an invalid size."));
            }
            set(&(if x <= MAX_INT { x } else { *d as i64 }).to_string());
        }
        Reflect::PosULongFallback(d) => {
            let x = realm.to_uint32(v)? as i64;
            set(&(if (1..=MAX_INT).contains(&x) { x } else { *d as i64 }).to_string());
        }
        Reflect::Double(_) | Reflect::PosDouble(_) => {
            let x = realm.to_number(v)?;
            if !x.is_finite() {
                return realm.type_error("The provided double value is non-finite.");
            }
            if matches!(spec.kind, Reflect::PosDouble(_)) && x <= 0.0 {
                return Ok(Value::Undefined);
            }
            set(&rabbithole_js::value::number_to_string(x));
        }
        _ => {
            let s = realm.to_rust_string(v)?;
            set(&s);
        }
    }
    Ok(Value::Undefined)
}

/// Define `prop` on `proto` as an accessor reflecting attribute `attr`.
pub(crate) fn reflect(realm: &mut Realm, proto: ObjectId, prop: &str, attr: &str, kind: Reflect) {
    let data: Rc<dyn std::any::Any> = Rc::new(AttrSpec { attr: attr.to_string(), kind });
    let g = realm.new_native_function_full(&format!("get {prop}"), 0, reflect_get, Some(data.clone()), vec![], false);
    let s = realm.new_native_function_full(&format!("set {prop}"), 1, reflect_set, Some(data), vec![], false);
    realm.define_accessor_values(proto, prop, Some(Value::Object(g)), Some(Value::Object(s)), ACC);
}

/// `__rhNative.reflect(prototype, property, attribute, kind, default)`:
/// the reflections of the generated IDL table (html.js). Kinds: s string,
/// u URL, b boolean, l long, nn non-negative long, ul unsigned long, pos
/// positive unsigned long, d double.
pub(crate) fn reflect_native(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(proto) = arg(args, 0).as_object() else { return Ok(Value::Bool(false)) };
    let prop = arg_str(realm, args, 1)?;
    let attr = arg_str(realm, args, 2)?;
    let kind = arg_str(realm, args, 3)?;
    let dflt = arg_str_opt(realm, args, 4)?;
    let int = |d: i64| dflt.as_deref().and_then(|s| s.trim().parse::<i64>().ok()).unwrap_or(d);
    let kind = match kind.as_str() {
        "s" => Reflect::Str,
        "u" => Reflect::Url,
        "b" => Reflect::Bool,
        "l" => Reflect::Long(int(0) as i32),
        "nn" => Reflect::NonNegLong(int(-1) as i32),
        "ul" => Reflect::ULong(int(0) as u32),
        "pos" => Reflect::PosULong(int(1) as u32),
        "d" => Reflect::Double(dflt.as_deref().and_then(|s| s.trim().parse::<f64>().ok()).unwrap_or(0.0)),
        _ => return Ok(Value::Bool(false)),
    };
    reflect(realm, proto, &prop, &attr, kind);
    Ok(Value::Bool(true))
}

// ---------------------------------------------------------- translate

/// `translate`: whether the element's translation mode is
/// translate-enabled — its own `translate` attribute ("yes" or "" / "no",
/// HTML elements only) or else its parent element's mode, enabled at the
/// root.
fn translate_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "translate")?;
    let enabled = with_doc(n.doc, |doc| {
        let mut cur = Some(n.node);
        while let Some(id) = cur {
            let Some(e) = doc.element(id) else { break };
            if e.ns == rabbithole_dom::Namespace::Html {
                if let Some(v) = doc.attr(id, "translate") {
                    if v.is_empty() || v.eq_ignore_ascii_case("yes") {
                        return true;
                    }
                    if v.eq_ignore_ascii_case("no") {
                        return false;
                    }
                }
            }
            cur = doc.parent(id).filter(|&p| doc.element(p).is_some());
        }
        true
    });
    Ok(Value::Bool(enabled))
}

fn translate_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "translate")?;
    let v = realm.to_boolean(&arg(args, 0));
    crate::element::set_attribute(n, "translate", Some(if v { "yes" } else { "no" }));
    Ok(Value::Undefined)
}

// ------------------------------------------------------------- nonces

thread_local! {
    /// [[CryptographicNonce]] slots of elements whose nonce was set since
    /// parsing (absent: the parsed nonce attribute's value).
    static NONCES: std::cell::RefCell<std::collections::HashMap<NodeRef, String>> = Default::default();
}

/// The nonce attribute's change steps: the slot follows the attribute.
pub(crate) fn nonce_attribute_changed(n: NodeRef, value: Option<&str>) {
    NONCES.with(|m| m.borrow_mut().insert(n, value.unwrap_or("").to_string()));
}

/// `nonce` (HTMLOrSVGElement): the element's [[CryptographicNonce]], which
/// the IDL setter changes without touching the content attribute.
fn nonce_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "nonce")?;
    let slot = NONCES.with(|m| m.borrow().get(&n).cloned());
    Ok(Value::str(&slot.or_else(|| crate::element::get_attribute(n, "nonce")).unwrap_or_default()))
}

fn nonce_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "nonce")?;
    let v = realm.to_rust_string(arg(args, 0))?;
    NONCES.with(|m| m.borrow_mut().insert(n, v));
    Ok(Value::Undefined)
}

// ------------------------------------------------- enumerated attributes

const FORM_ENCTYPES: &[&str] = &["application/x-www-form-urlencoded", "multipart/form-data", "text/plain"];
const REFERRER_POLICIES: &[&str] = &["", "no-referrer", "no-referrer-when-downgrade", "same-origin", "origin", "strict-origin", "origin-when-cross-origin", "strict-origin-when-cross-origin", "unsafe-url"];
pub(crate) const INPUT_TYPES: &[&str] = &[
    "hidden", "text", "search", "tel", "url", "email", "password", "date", "month", "week", "time", "datetime-local", "number", "range", "color", "checkbox", "radio", "file", "submit", "image", "reset", "button",
];

fn cross_origin() -> Reflect {
    Reflect::Enum(Rc::new(EnumSpec {
        keywords: vec![("anonymous", "anonymous"), ("use-credentials", "use-credentials"), ("", "anonymous")],
        missing: None,
        invalid: Some("anonymous"),
        nullable: true,
    }))
}

fn enumerated(keywords: &[&'static str], missing: Option<&'static str>, invalid: Option<&'static str>) -> Reflect {
    Reflect::Enum(Rc::new(EnumSpec::new(keywords, missing, invalid)))
}

/// The spec's reflected attributes whose type the IDL leaves open:
/// enumerated keywords and defaults, numeric limits and clamping.
pub(crate) fn install(realm: &mut Realm) {
    let protos: Vec<(String, ObjectId)> = {
        let st = state();
        let host = st.borrow();
        host.protos.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    };
    let proto = |name: &str| protos.iter().find(|p| p.0 == name).map(|p| p.1);
    for iface in ["HTMLElement", "SVGElement", "MathMLElement"] {
        if let Some(p) = proto(iface) {
            realm.define_accessor(p, "nonce", Some(nonce_get), Some(nonce_set), ACC);
        }
    }
    if let Some(p) = proto("HTMLElement") {
        realm.define_accessor(p, "translate", Some(translate_get), Some(translate_set), ACC);
    }
    let mut def = |iface: &str, prop: &str, attr: &str, kind: Reflect| {
        if let Some(p) = proto(iface) {
            reflect(realm, p, prop, attr, kind);
        }
    };
    // Global attributes.
    def("HTMLElement", "dir", "dir", enumerated(&["ltr", "rtl", "auto"], None, None));
    def("HTMLElement", "enterKeyHint", "enterkeyhint", enumerated(&["enter", "done", "go", "next", "previous", "search", "send"], None, None));
    def("HTMLElement", "inputMode", "inputmode", enumerated(&["none", "text", "tel", "url", "email", "numeric", "decimal", "search"], None, None));
    // Forms.
    def("HTMLFormElement", "autocomplete", "autocomplete", enumerated(&["on", "off"], Some("on"), Some("on")));
    def("HTMLFormElement", "enctype", "enctype", enumerated(FORM_ENCTYPES, Some(FORM_ENCTYPES[0]), Some(FORM_ENCTYPES[0])));
    def("HTMLFormElement", "encoding", "enctype", enumerated(FORM_ENCTYPES, Some(FORM_ENCTYPES[0]), Some(FORM_ENCTYPES[0])));
    def("HTMLFormElement", "method", "method", enumerated(&["get", "post", "dialog"], Some("get"), Some("get")));
    for iface in ["HTMLInputElement", "HTMLButtonElement"] {
        def(iface, "formEnctype", "formenctype", enumerated(FORM_ENCTYPES, None, Some(FORM_ENCTYPES[0])));
        def(iface, "formAction", "formaction", Reflect::UrlOrDocument);
    }
    def("HTMLInputElement", "formMethod", "formmethod", enumerated(&["get", "post"], None, Some("get")));
    def("HTMLButtonElement", "formMethod", "formmethod", enumerated(&["get", "post", "dialog"], None, Some("get")));
    def("HTMLInputElement", "src", "src", Reflect::Url);
    def("HTMLInputElement", "maxLength", "maxlength", Reflect::NonNegLong(-1));
    def("HTMLInputElement", "minLength", "minlength", Reflect::NonNegLong(-1));
    def("HTMLInputElement", "size", "size", Reflect::PosULong(20));
    def("HTMLTextAreaElement", "maxLength", "maxlength", Reflect::NonNegLong(-1));
    def("HTMLTextAreaElement", "minLength", "minlength", Reflect::NonNegLong(-1));
    def("HTMLTextAreaElement", "cols", "cols", Reflect::PosULongFallback(20));
    def("HTMLTextAreaElement", "rows", "rows", Reflect::PosULongFallback(2));
    def("HTMLSelectElement", "size", "size", Reflect::ULong(0));
    def("HTMLProgressElement", "max", "max", Reflect::PosDouble(1.0));
    // Grouping, text and tables.
    def("HTMLOListElement", "start", "start", Reflect::Long(1));
    def("HTMLLIElement", "value", "value", Reflect::Long(0));
    def("HTMLPreElement", "width", "width", Reflect::Long(0));
    def("HTMLQuoteElement", "cite", "cite", Reflect::Url);
    def("HTMLModElement", "cite", "cite", Reflect::Url);
    def("HTMLTableCellElement", "colSpan", "colspan", Reflect::Clamped(1, 1, 1000));
    def("HTMLTableCellElement", "rowSpan", "rowspan", Reflect::Clamped(1, 0, 65534));
    def("HTMLTableCellElement", "scope", "scope", enumerated(&["row", "col", "rowgroup", "colgroup"], None, None));
    def("HTMLTableColElement", "span", "span", Reflect::Clamped(1, 1, 1000));
    // Embedded content and metadata.
    for iface in ["HTMLImageElement", "HTMLMediaElement", "HTMLScriptElement", "HTMLLinkElement"] {
        def(iface, "crossOrigin", "crossorigin", cross_origin());
    }
    for iface in ["HTMLImageElement", "HTMLIFrameElement", "HTMLAnchorElement", "HTMLAreaElement", "HTMLLinkElement", "HTMLScriptElement"] {
        def(iface, "referrerPolicy", "referrerpolicy", enumerated(REFERRER_POLICIES, Some(""), Some("")));
    }
    def("HTMLImageElement", "decoding", "decoding", enumerated(&["async", "sync", "auto"], Some("auto"), Some("auto")));
    for iface in ["HTMLImageElement", "HTMLIFrameElement"] {
        def(iface, "loading", "loading", enumerated(&["lazy", "eager"], Some("eager"), Some("eager")));
    }
    for iface in ["HTMLImageElement", "HTMLLinkElement", "HTMLScriptElement"] {
        def(iface, "fetchPriority", "fetchpriority", enumerated(&["high", "low", "auto"], Some("auto"), Some("auto")));
    }
    def(
        "HTMLMediaElement",
        "preload",
        "preload",
        Reflect::Enum(Rc::new(EnumSpec {
            keywords: vec![("none", "none"), ("metadata", "metadata"), ("auto", "auto"), ("", "auto")],
            missing: Some("metadata"),
            invalid: Some("metadata"),
            nullable: false,
        })),
    );
    def("HTMLTrackElement", "kind", "kind", enumerated(&["subtitles", "captions", "descriptions", "chapters", "metadata"], Some("subtitles"), Some("metadata")));
    def(
        "HTMLLinkElement",
        "as",
        "as",
        enumerated(&["fetch", "audio", "document", "embed", "font", "image", "manifest", "object", "report", "script", "sharedworker", "style", "track", "video", "worker", "xslt"], Some(""), Some("")),
    );
    def("HTMLCanvasElement", "width", "width", Reflect::ULong(300));
    def("HTMLCanvasElement", "height", "height", Reflect::ULong(150));
    def("HTMLVideoElement", "width", "width", Reflect::ULong(0));
    def("HTMLVideoElement", "height", "height", Reflect::ULong(0));
    // ARIA attributes with token values (tentative ARIA enumerated
    // reflection, w3c/aria#2484): values are canonicalized, invalid ones
    // map to their invalid value default; an absent attribute stays null
    // as ARIAMixin's nullable DOMString requires.
    let tf_false = &[("true", "true"), ("false", "false"), ("", "false")][..];
    let tf = &[("true", "true"), ("false", "false")][..];
    let tfm = &[("true", "true"), ("false", "false"), ("mixed", "mixed")][..];
    let aria: [(&str, &str, &[(&'static str, &'static str)], Option<&'static str>); 20] = [
        ("ariaAtomic", "aria-atomic", tf_false, Some("false")),
        ("ariaAutoComplete", "aria-autocomplete", &[("inline", "inline"), ("list", "list"), ("both", "both"), ("none", "none")], Some("none")),
        ("ariaBusy", "aria-busy", tf_false, Some("false")),
        ("ariaChecked", "aria-checked", tfm, None),
        ("ariaCurrent", "aria-current", &[("page", "page"), ("step", "step"), ("location", "location"), ("date", "date"), ("time", "time"), ("true", "true"), ("false", "false"), ("", "false")], Some("true")),
        ("ariaDisabled", "aria-disabled", tf_false, Some("false")),
        ("ariaExpanded", "aria-expanded", tf, None),
        ("ariaHasPopup", "aria-haspopup", &[("true", "true"), ("false", "false"), ("menu", "menu"), ("dialog", "dialog"), ("listbox", "listbox"), ("tree", "tree"), ("grid", "grid")], Some("false")),
        ("ariaHidden", "aria-hidden", tf_false, Some("false")),
        ("ariaInvalid", "aria-invalid", &[("true", "true"), ("false", "false"), ("spelling", "spelling"), ("grammar", "grammar"), ("", "false")], Some("true")),
        ("ariaLive", "aria-live", &[("polite", "polite"), ("assertive", "assertive"), ("off", "off")], Some("off")),
        ("ariaModal", "aria-modal", tf_false, Some("false")),
        ("ariaMultiLine", "aria-multiline", tf_false, Some("false")),
        ("ariaMultiSelectable", "aria-multiselectable", tf_false, Some("false")),
        ("ariaOrientation", "aria-orientation", &[("horizontal", "horizontal"), ("vertical", "vertical")], None),
        ("ariaPressed", "aria-pressed", tfm, None),
        ("ariaReadOnly", "aria-readonly", tf_false, Some("false")),
        ("ariaRequired", "aria-required", tf_false, Some("false")),
        ("ariaSelected", "aria-selected", tf, None),
        ("ariaSort", "aria-sort", &[("ascending", "ascending"), ("descending", "descending"), ("other", "other"), ("none", "none")], Some("none")),
    ];
    for (prop, attr, keywords, invalid) in aria {
        def("Element", prop, attr, Reflect::Enum(Rc::new(EnumSpec { keywords: keywords.to_vec(), missing: None, invalid, nullable: true })));
    }
    // [LegacyNullToEmptyString] presentational attributes.
    for (iface, props) in [
        ("HTMLBodyElement", &[("text", "text"), ("link", "link"), ("vLink", "vlink"), ("aLink", "alink"), ("bgColor", "bgcolor")][..]),
        ("HTMLTableElement", &[("bgColor", "bgcolor"), ("cellPadding", "cellpadding"), ("cellSpacing", "cellspacing")][..]),
        ("HTMLTableRowElement", &[("bgColor", "bgcolor")][..]),
        ("HTMLTableCellElement", &[("bgColor", "bgcolor")][..]),
        ("HTMLImageElement", &[("border", "border")][..]),
        ("HTMLObjectElement", &[("border", "border")][..]),
        ("HTMLIFrameElement", &[("marginHeight", "marginheight"), ("marginWidth", "marginwidth")][..]),
        ("HTMLFrameElement", &[("marginHeight", "marginheight"), ("marginWidth", "marginwidth")][..]),
        ("HTMLFontElement", &[("color", "color")][..]),
    ] {
        for (prop, attr) in props {
            def(iface, prop, attr, Reflect::StrNullEmpty);
        }
    }
    // Obsolete.
    def("HTMLMarqueeElement", "behavior", "behavior", enumerated(&["scroll", "slide", "alternate"], Some("scroll"), Some("scroll")));
    def("HTMLMarqueeElement", "direction", "direction", enumerated(&["up", "right", "down", "left"], Some("left"), Some("left")));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_rules() {
        assert_eq!(parse_integer(" \t-12px"), Some(-12));
        assert_eq!(parse_integer("+7"), Some(7));
        assert_eq!(parse_integer("\u{b}7"), None);
        assert_eq!(parse_integer("-"), None);
        assert_eq!(parse_integer("-0"), Some(0));
        assert_eq!(parse_non_negative("-1"), None);
        assert_eq!(parse_integer("99999999999999999999999"), Some(i64::MAX));
    }

    #[test]
    fn float_rules() {
        assert_eq!(parse_float("1."), Some(1.0));
        assert_eq!(parse_float(".5"), Some(0.5));
        assert_eq!(parse_float("1e2"), Some(100.0));
        assert_eq!(parse_float("1E-2"), Some(0.01));
        assert_eq!(parse_float("1.e2"), Some(100.0));
        assert_eq!(parse_float("1 e2"), Some(1.0));
        assert_eq!(parse_float("1e"), Some(1.0));
        assert_eq!(parse_float("-0"), Some(0.0));
        assert_eq!(parse_float("1.8e308"), None);
        assert_eq!(parse_float("+"), None);
        assert_eq!(parse_float("\u{a0}7"), None);
        assert_eq!(parse_float("5%"), Some(5.0));
    }

    #[test]
    fn enumerated_states() {
        let e = EnumSpec::new(&["on", "off"], Some("on"), None);
        assert_eq!(e.state(None), Some("on"));
        assert_eq!(e.state(Some("OFF")), Some("off"));
        assert_eq!(e.state(Some("xoff")), None);
        // ASCII case-insensitive only: KELVIN SIGN is not "k".
        let k = EnumSpec::new(&["track"], None, Some("x"));
        assert_eq!(k.state(Some("trac\u{212a}")), Some("x"));
    }
}
