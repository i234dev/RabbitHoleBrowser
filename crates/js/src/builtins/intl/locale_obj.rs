//! `Intl.Locale` (ECMA-402 §14): the constructor's UpdateLanguageId and
//! Unicode extension options, the component getters and
//! maximize/minimize with likely subtags.

use super::locale_id::{self, LocaleId};
use super::*;

/// [[InitializedLocale]] data: the canonical tag.
#[derive(Debug, Clone)]
pub struct LocaleObject {
    pub tag: String,
}

fn get_opt(realm: &mut Realm, o: ObjectId, name: &str) -> JsResult<Option<String>> {
    let v = realm.get_str(Value::Object(o), name)?;
    if v.is_undefined() {
        return Ok(None);
    }
    Ok(Some(realm.to_rust_string(v)?))
}

fn is_type_sequence(s: &str) -> bool {
    super::duration::is_unicode_type(s)
}

pub(super) fn ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if realm.new_target().is_undefined() {
        return realm.type_error("Constructor Intl.Locale requires 'new'");
    }
    let default = realm.intrinsics.others["Intl.Locale"];
    let proto = realm.proto_from_new_target_checked(default)?;
    let tag_arg = arg(args, 0);
    let tag = match tag_arg.v() {
        V::Object(id) => match realm.host_data::<LocaleObject>(id) {
            Some(l) => l.tag.clone(),
            None => realm.to_rust_string(tag_arg.clone())?,
        },
        V::String(s) => s.to_rust_string(),
        _ => return realm.type_error("First argument to Intl.Locale constructor can't be empty or missing"),
    };
    let options = match arg(args, 1).v() {
        V::Undefined => None,
        _ => Some(realm.to_object(arg(args, 1))?),
    };
    let Some(mut id) = locale_id::parse(&tag) else {
        return realm.range_error(&format!("Incorrect locale information provided: {}", tag));
    };
    // UpdateLanguageId.
    if let Some(o) = options {
        if let Some(lang) = get_opt(realm, o, "language")? {
            let l = lang.to_ascii_lowercase();
            if !(l.bytes().all(|b| b.is_ascii_alphabetic()) && matches!(l.len(), 2 | 3 | 5..=8)) {
                return realm.range_error(&format!("Invalid language: {}", lang));
            }
            id.language = l;
        }
        if let Some(script) = get_opt(realm, o, "script")? {
            let s = script.to_ascii_lowercase();
            if !(s.len() == 4 && s.bytes().all(|b| b.is_ascii_alphabetic())) {
                return realm.range_error(&format!("Invalid script: {}", script));
            }
            id.script = Some(s);
        }
        if let Some(region) = get_opt(realm, o, "region")? {
            let r = region.to_ascii_lowercase();
            if !((r.len() == 2 && r.bytes().all(|b| b.is_ascii_alphabetic())) || (r.len() == 3 && r.bytes().all(|b| b.is_ascii_digit()))) {
                return realm.range_error(&format!("Invalid region: {}", region));
            }
            id.region = Some(r);
        }
        if let Some(variants) = get_opt(realm, o, "variants")? {
            let v = variants.to_ascii_lowercase();
            let parts: Vec<String> = v.split('-').map(str::to_string).collect();
            let ok = !v.is_empty()
                && parts.iter().all(|p| p.bytes().all(|b| b.is_ascii_alphanumeric()) && ((5..=8).contains(&p.len()) || (p.len() == 4 && p.as_bytes()[0].is_ascii_digit())))
                && parts.iter().enumerate().all(|(i, p)| !parts[..i].contains(p));
            if !ok {
                return realm.range_error(&format!("Invalid variants: {}", variants));
            }
            id.variants = parts;
        }
    }
    locale_id::canonicalize_id(&mut id);
    // Unicode extension options, in order.
    if let Some(o) = options {
        let mut set: Vec<(&str, Option<String>)> = Vec::new();
        for (key, name) in [("ca", "calendar"), ("co", "collation")] {
            let v = get_opt(realm, o, name)?;
            if let Some(s) = &v {
                if !is_type_sequence(s) {
                    return realm.range_error(&format!("Invalid {}: {}", name, s));
                }
            }
            set.push((key, v));
        }
        let hc = get_opt(realm, o, "hourCycle")?;
        if let Some(h) = &hc {
            if !matches!(h.as_str(), "h11" | "h12" | "h23" | "h24") {
                return realm.range_error(&format!("Invalid hourCycle: {}", h));
            }
        }
        set.push(("hc", hc));
        let kf = get_opt(realm, o, "caseFirst")?;
        if let Some(k) = &kf {
            if !matches!(k.as_str(), "upper" | "lower" | "false") {
                return realm.range_error(&format!("Invalid caseFirst: {}", k));
            }
        }
        set.push(("kf", kf));
        let kn = realm.get_str(Value::Object(o), "numeric")?;
        let kn = if kn.is_undefined() { None } else { Some(if realm.to_boolean(&kn) { "true".to_string() } else { "false".to_string() }) };
        set.push(("kn", kn));
        let nu = get_opt(realm, o, "numberingSystem")?;
        if let Some(n) = &nu {
            if !is_type_sequence(n) {
                return realm.range_error(&format!("Invalid numberingSystem: {}", n));
            }
        }
        set.push(("nu", nu));
        for (key, v) in set {
            if let Some(v) = v {
                let v = v.to_ascii_lowercase();
                match id.u_keywords.iter_mut().find(|(k, _)| k == key) {
                    Some(kw) => kw.1 = v,
                    None => id.u_keywords.push((key.to_string(), v)),
                }
            }
        }
        locale_id::canonicalize_id(&mut id);
    }
    let obj = realm.new_object_with_proto(Some(proto));
    realm.set_host_data(obj, Box::new(LocaleObject { tag: locale_id::to_string(&id) }));
    Ok(Value::Object(obj))
}

fn this_id(realm: &mut Realm, this: &Value, what: &str) -> JsResult<LocaleId> {
    let l: LocaleObject = host(realm, this, what)?;
    Ok(locale_id::parse(&l.tag).unwrap_or_default())
}

fn keyword(id: &LocaleId, key: &str) -> Option<String> {
    id.u_keywords.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

fn opt_value(v: Option<String>) -> Value {
    match v {
        Some(s) => Value::str(&s),
        None => Value::Undefined,
    }
}

pub(super) fn to_string(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let l: LocaleObject = host(realm, &this, "Intl.Locale.prototype.toString")?;
    Ok(Value::str(&l.tag))
}
pub(super) fn base_name(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_id(realm, &this, "Intl.Locale.prototype.baseName")?;
    Ok(Value::str(&locale_id::base_name(&id)))
}
pub(super) fn language(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_id(realm, &this, "Intl.Locale.prototype.language")?;
    Ok(Value::str(&id.language))
}
pub(super) fn script(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_id(realm, &this, "Intl.Locale.prototype.script")?;
    Ok(opt_value(id.script.map(|s| {
        let mut c = s.chars();
        c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default()
    })))
}
pub(super) fn region(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_id(realm, &this, "Intl.Locale.prototype.region")?;
    Ok(opt_value(id.region.map(|r| r.to_ascii_uppercase())))
}
pub(super) fn variants(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_id(realm, &this, "Intl.Locale.prototype.variants")?;
    Ok(opt_value((!id.variants.is_empty()).then(|| id.variants.join("-"))))
}
pub(super) fn calendar(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_id(realm, &this, "Intl.Locale.prototype.calendar")?;
    Ok(opt_value(keyword(&id, "ca")))
}
pub(super) fn collation(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_id(realm, &this, "Intl.Locale.prototype.collation")?;
    Ok(opt_value(keyword(&id, "co")))
}
pub(super) fn hour_cycle(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_id(realm, &this, "Intl.Locale.prototype.hourCycle")?;
    Ok(opt_value(keyword(&id, "hc")))
}
pub(super) fn case_first(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_id(realm, &this, "Intl.Locale.prototype.caseFirst")?;
    Ok(opt_value(keyword(&id, "kf")))
}
pub(super) fn numeric(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_id(realm, &this, "Intl.Locale.prototype.numeric")?;
    Ok(Value::Bool(matches!(keyword(&id, "kn").as_deref(), Some("") | Some("true"))))
}
pub(super) fn numbering_system(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_id(realm, &this, "Intl.Locale.prototype.numberingSystem")?;
    Ok(opt_value(keyword(&id, "nu")))
}

// ---------------------------------------------------------- likely subtags

/// (language, script, region) → likely full subtags, beyond the plain
/// per-language defaults.
fn likely_special(lang: &str, script: Option<&str>, region: Option<&str>) -> Option<(&'static str, &'static str, &'static str)> {
    Some(match (lang, script, region) {
        ("en", Some("shaw"), _) => ("en", "shaw", "GB"),
        ("zh", Some("hant"), None) => ("zh", "hant", "TW"),
        ("zh", None, Some("tw" | "hk" | "mo")) => ("zh", "hant", ""),
        ("sr", Some("latn"), None) => ("sr", "latn", "RS"),
        ("sr", None, Some("me")) => ("sr", "latn", ""),
        ("pa", Some("arab"), None) => ("pa", "arab", "PK"),
        ("pa", None, Some("pk")) => ("pa", "arab", ""),
        ("und", Some("cyrl"), Some("ro")) => ("bg", "cyrl", ""),
        ("und", None, Some("at")) => ("de", "latn", ""),
        ("und", None, Some("419")) => ("es", "latn", ""),
        ("und", None, Some("ro")) => ("ro", "latn", ""),
        ("und", None, Some("ru")) => ("ru", "cyrl", ""),
        ("und", None, Some("jp")) => ("ja", "jpan", ""),
        ("und", None, Some("cn")) => ("zh", "hans", ""),
        ("und", None, Some("tw")) => ("zh", "hant", ""),
        ("und", None, Some("de")) => ("de", "latn", ""),
        ("und", None, Some("fr")) => ("fr", "latn", ""),
        ("und", None, Some("es")) => ("es", "latn", ""),
        ("und", None, Some("th")) => ("th", "thai", ""),
        _ => return None,
    })
}

fn und_script(script: &str) -> Option<(&'static str, &'static str)> {
    Some(match script {
        "latn" => ("en", "US"),
        "cyrl" => ("ru", "RU"),
        "arab" => ("ar", "EG"),
        "thai" => ("th", "TH"),
        "grek" => ("el", "GR"),
        "hebr" => ("he", "IL"),
        "armn" => ("hy", "AM"),
        "geor" => ("ka", "GE"),
        "deva" => ("hi", "IN"),
        "hans" | "hani" => ("zh", "CN"),
        "hant" => ("zh", "TW"),
        "jpan" | "kana" | "hira" => ("ja", "JP"),
        "kore" | "hang" => ("ko", "KR"),
        "shaw" => ("en", "GB"),
        _ => return None,
    })
}

/// AddLikelySubtags on the language id part.
fn maximize_id(id: &mut LocaleId) {
    if let Some((l, s, r)) = likely_special(&id.language, id.script.as_deref(), id.region.as_deref()) {
        id.language = l.to_string();
        if id.script.is_none() {
            id.script = Some(s.to_string());
        }
        if id.region.is_none() && !r.is_empty() {
            id.region = Some(r.to_ascii_lowercase());
        }
    }
    if id.language == "und" {
        let (l, r) = id.script.as_deref().and_then(und_script).unwrap_or(("en", "US"));
        id.language = l.to_string();
        if id.region.is_none() {
            id.region = Some(r.to_ascii_lowercase());
        }
    }
    let extra: &[(&str, &str, &str)] = &[("jbo", "Latn", "001"), ("hak", "Hans", "CN"), ("hsn", "Hans", "CN"), ("xtg", "Latn", "FR"), ("yi", "Hebr", "UA"), ("gss", "Grek", "GR"), ("hyw", "Armn", "AM")];
    let found = locale_id::LIKELY.iter().chain(extra.iter()).find(|(l, _, _)| *l == id.language);
    if let Some((_, s, r)) = found {
        if id.script.is_none() {
            id.script = Some(s.to_ascii_lowercase());
        }
        if id.region.is_none() {
            id.region = Some(r.to_ascii_lowercase());
        }
    }
}

fn with_extensions(id: &LocaleId, base: &LocaleId) -> String {
    let mut out = id.clone();
    out.variants = base.variants.clone();
    locale_id::to_string(&out)
}

pub(super) fn maximize(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_id(realm, &this, "Intl.Locale.prototype.maximize")?;
    let mut m = id.clone();
    maximize_id(&mut m);
    new_locale(realm, with_extensions(&m, &id))
}

pub(super) fn minimize(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_id(realm, &this, "Intl.Locale.prototype.minimize")?;
    let mut max = id.clone();
    maximize_id(&mut max);
    let key = |x: &LocaleId| (x.language.clone(), x.script.clone(), x.region.clone());
    let target = key(&max);
    let mut result = max.clone();
    for (s, r) in [(false, false), (false, true), (true, false)] {
        let mut cand = max.clone();
        if !s {
            cand.script = None;
        }
        if !r {
            cand.region = None;
        }
        let mut probe = cand.clone();
        maximize_id(&mut probe);
        if key(&probe) == target {
            result = cand;
            break;
        }
    }
    new_locale(realm, with_extensions(&result, &id))
}

fn new_locale(realm: &mut Realm, tag: String) -> JsResult<Value> {
    let proto = realm.intrinsics.others["Intl.Locale"];
    let obj = realm.new_object_with_proto(Some(proto));
    realm.set_host_data(obj, Box::new(LocaleObject { tag }));
    Ok(Value::Object(obj))
}
