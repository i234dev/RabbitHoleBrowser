//! `Intl.DisplayNames` (ECMA-402 §12).

use super::duration::{get_string_option, is_unicode_type};
use super::*;

#[derive(Debug, Clone)]
pub struct DisplayNamesOptions {
    locale: Locale,
    style: String,
    kind: String,
    fallback: String,
    language_display: Option<String>,
}

pub(super) fn ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if realm.new_target().is_undefined() {
        return realm.type_error("Constructor Intl.DisplayNames requires 'new'");
    }
    let default = realm.intrinsics.others["Intl.DisplayNames"];
    let proto = realm.proto_from_new_target_checked(default)?;
    let locale = lookup_locale(realm, arg(args, 0))?;
    let opts = match arg(args, 1).v() {
        V::Undefined => return realm.type_error("Intl.DisplayNames requires an options object with a type"),
        V::Object(id) => Some(id),
        _ => return realm.type_error("Options must be an object"),
    };
    get_string_option(realm, opts, "localeMatcher", &["lookup", "best fit"], Some("best fit"))?;
    let style = get_string_option(realm, opts, "style", &["narrow", "short", "long"], Some("long"))?.unwrap();
    let Some(kind) = get_string_option(realm, opts, "type", &["language", "region", "script", "currency", "calendar", "dateTimeField"], None)? else {
        return realm.type_error("Required option 'type' is missing");
    };
    let fallback = get_string_option(realm, opts, "fallback", &["code", "none"], Some("code"))?.unwrap();
    let language_display = get_string_option(realm, opts, "languageDisplay", &["dialect", "standard"], Some("dialect"))?.unwrap();
    let o = DisplayNamesOptions { locale, style, language_display: (kind == "language").then_some(language_display), kind, fallback };
    let obj = realm.new_object_with_proto(Some(proto));
    realm.set_host_data(obj, Box::new(o));
    Ok(Value::Object(obj))
}

/// CanonicalCodeForDisplayNames: Err(()) when the code is malformed.
fn canonical_code(kind: &str, code: &str) -> Result<String, ()> {
    match kind {
        "language" => {
            // A unicode_language_id only: no extensions or private use.
            let id = super::locale_id::parse(code).ok_or(())?;
            if !id.u_keywords.is_empty() || !id.u_attributes.is_empty() || id.t_lang.is_some() || !id.t_fields.is_empty() || !id.other.is_empty() || !id.private.is_empty() || code.contains("-t-") || code.contains("-u-") {
                return Err(());
            }
            if code.split('-').next().map(|l| l.len()) == Some(4) {
                return Err(());
            }
            super::locale_id::canonicalize(code).ok_or(())
        }
        "region" => {
            let ok = (code.len() == 2 && code.bytes().all(|b| b.is_ascii_alphabetic())) || (code.len() == 3 && code.bytes().all(|b| b.is_ascii_digit()));
            if ok { Ok(code.to_ascii_uppercase()) } else { Err(()) }
        }
        "script" => {
            if code.len() == 4 && code.bytes().all(|b| b.is_ascii_alphabetic()) {
                Ok(code[..1].to_ascii_uppercase() + &code[1..].to_ascii_lowercase())
            } else {
                Err(())
            }
        }
        "currency" => {
            if code.len() == 3 && code.bytes().all(|b| b.is_ascii_alphabetic()) {
                Ok(code.to_ascii_uppercase())
            } else {
                Err(())
            }
        }
        "calendar" => {
            if is_unicode_type(code) && !code.contains('_') {
                Ok(code.to_ascii_lowercase())
            } else {
                Err(())
            }
        }
        _ => {
            if matches!(code, "era" | "year" | "quarter" | "month" | "weekOfYear" | "weekday" | "day" | "dayPeriod" | "hour" | "minute" | "second" | "timeZoneName") {
                Ok(code.to_string())
            } else {
                Err(())
            }
        }
    }
}

fn name_for(o: &DisplayNamesOptions, code: &str) -> Option<&'static str> {
    let es = o.locale.lang == "es";
    let pick = |en: &'static str, sp: &'static str| if es { sp } else { en };
    Some(match (o.kind.as_str(), code) {
        ("region", "US") => pick("United States", "Estados Unidos"),
        ("region", "ES") => pick("Spain", "España"),
        ("region", "GB") => pick("United Kingdom", "Reino Unido"),
        ("region", "DE") => pick("Germany", "Alemania"),
        ("region", "FR") => pick("France", "Francia"),
        ("region", "IT") => pick("Italy", "Italia"),
        ("region", "PT") => pick("Portugal", "Portugal"),
        ("region", "BR") => pick("Brazil", "Brasil"),
        ("region", "MX") => pick("Mexico", "México"),
        ("region", "AR") => pick("Argentina", "Argentina"),
        ("region", "CA") => pick("Canada", "Canadá"),
        ("region", "CN") => pick("China", "China"),
        ("region", "JP") => pick("Japan", "Japón"),
        ("region", "419") => pick("Latin America", "Latinoamérica"),
        ("language", "en") => pick("English", "inglés"),
        ("language", "en-US") => pick("American English", "inglés estadounidense"),
        ("language", "en-GB") => pick("British English", "inglés británico"),
        ("language", "es") => pick("Spanish", "español"),
        ("language", "es-419") => pick("Latin American Spanish", "español latinoamericano"),
        ("language", "es-ES") => pick("European Spanish", "español de España"),
        ("language", "de") => pick("German", "alemán"),
        ("language", "fr") => pick("French", "francés"),
        ("language", "it") => pick("Italian", "italiano"),
        ("language", "pt") => pick("Portuguese", "portugués"),
        ("language", "ja") => pick("Japanese", "japonés"),
        ("language", "zh") => pick("Chinese", "chino"),
        ("language", "ru") => pick("Russian", "ruso"),
        ("language", "ar") => pick("Arabic", "árabe"),
        ("script", "Latn") => pick("Latin", "latino"),
        ("script", "Cyrl") => pick("Cyrillic", "cirílico"),
        ("script", "Arab") => pick("Arabic", "árabe"),
        ("script", "Hans") => pick("Simplified", "simplificado"),
        ("script", "Hant") => pick("Traditional", "tradicional"),
        ("currency", "USD") => pick("US Dollar", "dólar estadounidense"),
        ("currency", "EUR") => pick("Euro", "euro"),
        ("currency", "GBP") => pick("British Pound", "libra esterlina"),
        ("currency", "JPY") => pick("Japanese Yen", "yen"),
        ("calendar", "gregory") => pick("Gregorian Calendar", "calendario gregoriano"),
        ("calendar", "buddhist") => pick("Buddhist Calendar", "calendario budista"),
        ("calendar", "chinese") => pick("Chinese Calendar", "calendario chino"),
        ("calendar", "hebrew") => pick("Hebrew Calendar", "calendario hebreo"),
        ("calendar", "islamic") => pick("Hijri Calendar", "calendario hijri"),
        ("calendar", "japanese") => pick("Japanese Calendar", "calendario japonés"),
        ("calendar", "iso8601") => pick("ISO-8601 Calendar", "calendario ISO-8601"),
        ("dateTimeField", "era") => pick("era", "era"),
        ("dateTimeField", "year") => pick("year", "año"),
        ("dateTimeField", "quarter") => pick("quarter", "trimestre"),
        ("dateTimeField", "month") => pick("month", "mes"),
        ("dateTimeField", "weekOfYear") => pick("week", "semana"),
        ("dateTimeField", "weekday") => pick("day of the week", "día de la semana"),
        ("dateTimeField", "day") => pick("day", "día"),
        ("dateTimeField", "dayPeriod") => pick("AM/PM", "a. m./p. m."),
        ("dateTimeField", "hour") => pick("hour", "hora"),
        ("dateTimeField", "minute") => pick("minute", "minuto"),
        ("dateTimeField", "second") => pick("second", "segundo"),
        ("dateTimeField", "timeZoneName") => pick("time zone", "zona horaria"),
        _ => return None,
    })
}

pub(super) fn of(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o: DisplayNamesOptions = host(realm, &this, "Intl.DisplayNames.prototype.of")?;
    let code = realm.to_rust_string(arg(args, 0))?;
    let Ok(code) = canonical_code(&o.kind, &code) else {
        return realm.range_error(&format!("invalid_argument: {}", code));
    };
    match name_for(&o, &code) {
        Some(n) => Ok(Value::str(n)),
        None if o.fallback == "code" => Ok(Value::str(&code)),
        None => Ok(Value::Undefined),
    }
}

pub(super) fn resolved_options(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let o: DisplayNamesOptions = host(realm, &this, "Intl.DisplayNames.prototype.resolvedOptions")?;
    let obj = realm.new_object();
    realm.define_property(obj, "locale", Value::str(&o.locale.tag), ALL);
    realm.define_property(obj, "style", Value::str(&o.style), ALL);
    realm.define_property(obj, "type", Value::str(&o.kind), ALL);
    realm.define_property(obj, "fallback", Value::str(&o.fallback), ALL);
    if let Some(ld) = &o.language_display {
        realm.define_property(obj, "languageDisplay", Value::str(ld), ALL);
    }
    Ok(Value::Object(obj))
}
