//! Networking APIs: `fetch`, `Headers`, `Request`, `Response`,
//! `XMLHttpRequest`, `URL`, `URLSearchParams`, `FormData`,
//! `TextEncoder`/`TextDecoder`, `atob`/`btoa`, `Blob` (minimal).

use crate::env::with_env;
use crate::host::*;
use rabbithole_js::{JsResult, ObjectId, PropertyKey, Realm, Value, ALL, CONFIGURABLE, ENUMERABLE, HIDDEN};
use rabbithole_net::{Body, Request, Url};

const ACC: u8 = CONFIGURABLE | ENUMERABLE;

// ------------------------------------------------------------------ URL

pub(crate) fn url_component(u: &Url, which: &str) -> String {
    match which {
        "href" => u.to_string(),
        "protocol" => format!("{}:", u.scheme),
        "host" => u.host_with_port(),
        "hostname" => u.host.clone(),
        "port" => u.port.map(|p| p.to_string()).unwrap_or_default(),
        "pathname" => {
            if u.is_http() || u.scheme == "file" || u.path.starts_with('/') {
                u.path.clone()
            } else {
                u.path.clone()
            }
        }
        "search" => u.query.as_ref().filter(|q| !q.is_empty()).map(|q| format!("?{q}")).unwrap_or_default(),
        "hash" => u.fragment.as_ref().filter(|f| !f.is_empty()).map(|f| format!("#{f}")).unwrap_or_default(),
        "origin" => u.origin(),
        _ => String::new(),
    }
}

pub(crate) struct UrlData {
    pub url: Url,
    pub params: Option<ObjectId>,
}

fn url_data(realm: &mut Realm, this: &Value) -> JsResult<Url> {
    match this {
        Value::Object(o) => match realm.host_data::<UrlData>(*o) {
            Some(d) => Ok(d.url.clone()),
            None => Err(realm.throw_type_error("Illegal invocation")),
        },
        _ => Err(realm.throw_type_error("Illegal invocation")),
    }
}

fn set_url_data(realm: &mut Realm, this: &Value, u: Url) {
    if let Value::Object(o) = this {
        if let Some(d) = realm.host_data_mut::<UrlData>(*o) {
            d.url = u;
        }
    }
}

pub(crate) fn make_url_object(realm: &mut Realm, u: Url) -> Value {
    let p = proto("URL");
    let o = realm.new_object_with_proto(Some(p));
    realm.set_host_data(o, Box::new(UrlData { url: u, params: None }));
    realm.set_class_name(o, "URL");
    Value::Object(o)
}

fn parse_url_args(realm: &mut Realm, args: &[Value]) -> JsResult<Option<Url>> {
    let input = arg(args, 0);
    if let Some(u) = realm_url_of(realm, &input) {
        return Ok(Some(u));
    }
    let input = realm.to_rust_string(input)?;
    let base = match args.get(1) {
        None | Some(Value::Undefined) => None,
        Some(v) => match realm_url_of(realm, v) {
            Some(u) => Some(u),
            None => {
                let s = realm.to_rust_string(v.clone())?;
                match Url::parse_absolute(&s) {
                    Some(u) => Some(u),
                    None => return Ok(None),
                }
            }
        },
    };
    Ok(match base {
        Some(b) => b.join(input.trim()),
        None => Url::parse_absolute(input.trim()),
    })
}

fn realm_url_of(realm: &Realm, v: &Value) -> Option<Url> {
    match v {
        Value::Object(o) => realm.host_data::<UrlData>(*o).map(|d| d.url.clone()),
        _ => None,
    }
}

fn url_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    match parse_url_args(realm, args)? {
        Some(u) => {
            let v = make_url_object(realm, u);
            if let Value::Object(id) = &v {
                let p = realm.proto_from_new_target(proto("URL"));
                realm.obj_mut(*id).proto = Some(p);
            }
            Ok(v)
        }
        None => {
            let s = arg_str(realm, args, 0).unwrap_or_default();
            Err(realm.throw_type_error(&format!("Failed to construct 'URL': Invalid URL: {s}")))
        }
    }
}

fn url_can_parse(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(parse_url_args(realm, args)?.is_some()))
}

fn url_parse_static(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(match parse_url_args(realm, args)? {
        Some(u) => make_url_object(realm, u),
        None => Value::Null,
    })
}

fn url_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let which = native_data::<String>(realm).expect("component");
    let u = url_data(realm, &this)?;
    Ok(Value::str(&url_component(&u, &which)))
}

fn url_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let which = native_data::<String>(realm).expect("component");
    let mut u = url_data(realm, &this)?;
    let v = arg_str(realm, args, 0)?;
    match which.as_str() {
        "href" => {
            if let Some(nu) = Url::parse_absolute(&v) {
                u = nu;
            } else {
                return Err(realm.throw_type_error(&format!("Invalid URL: {v}")));
            }
        }
        "protocol" => u.scheme = v.trim_end_matches(':').to_ascii_lowercase(),
        "host" => {
            let (h, p) = match v.rsplit_once(':') {
                Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.parse::<u16>().ok()),
                _ => (v.clone(), None),
            };
            u.host = h.to_ascii_lowercase();
            u.port = p.filter(|p| Some(*p) != u.default_port());
        }
        "hostname" => u.host = v.to_ascii_lowercase(),
        "port" => u.port = v.parse::<u16>().ok().filter(|p| Some(*p) != u.default_port()),
        "pathname" => u.path = if v.starts_with('/') { v } else { format!("/{v}") },
        "search" => {
            let q = v.trim_start_matches('?');
            u.query = if q.is_empty() { None } else { Some(q.to_string()) };
        }
        "hash" => {
            let f = v.trim_start_matches('#');
            u.fragment = if f.is_empty() { None } else { Some(f.to_string()) };
        }
        _ => {}
    }
    set_url_data(realm, &this, u.clone());
    // Keep searchParams in sync.
    if which.as_str() == "search" || which.as_str() == "href" {
        if let Value::Object(o) = &this {
            if let Some(d) = realm.host_data::<UrlData>(*o) {
                if let Some(p) = d.params {
                    let pairs = parse_query(u.query.as_deref().unwrap_or(""));
                    if let Some(pd) = realm.host_data_mut::<ParamsData>(p) {
                        pd.pairs = pairs;
                    }
                }
            }
        }
    }
    Ok(Value::Undefined)
}

fn url_to_string(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let u = url_data(realm, &this)?;
    Ok(Value::str(&u.to_string()))
}

fn url_search_params(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let u = url_data(realm, &this)?;
    let Value::Object(o) = &this else { unreachable!() };
    if let Some(d) = realm.host_data::<UrlData>(*o) {
        if let Some(p) = d.params {
            return Ok(Value::Object(p));
        }
    }
    let pairs = parse_query(u.query.as_deref().unwrap_or(""));
    let p = make_params(realm, pairs, Some(*o));
    if let Some(d) = realm.host_data_mut::<UrlData>(*o) {
        d.params = Some(p);
    }
    Ok(Value::Object(p))
}

// ------------------------------------------------------------ URLSearchParams

pub(crate) struct ParamsData {
    pub pairs: Vec<(String, String)>,
    /// Owning URL object (searchParams of a URL) to write back to.
    pub owner: Option<ObjectId>,
}

fn decode_form(s: &str) -> String {
    rabbithole_net::percent_decode(&s.replace('+', " "))
}

pub(crate) fn parse_query(q: &str) -> Vec<(String, String)> {
    let q = q.trim_start_matches('?');
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (decode_form(k), decode_form(v)),
            None => (decode_form(p), String::new()),
        })
        .collect()
}

pub(crate) fn serialize_query(pairs: &[(String, String)]) -> String {
    pairs.iter().map(|(k, v)| format!("{}={}", rabbithole_net::form_encode(k), rabbithole_net::form_encode(v))).collect::<Vec<_>>().join("&")
}

fn make_params(realm: &mut Realm, pairs: Vec<(String, String)>, owner: Option<ObjectId>) -> ObjectId {
    let p = proto("URLSearchParams");
    let o = realm.new_object_with_proto(Some(p));
    realm.set_host_data(o, Box::new(ParamsData { pairs, owner }));
    realm.set_class_name(o, "URLSearchParams");
    o
}

fn params_of(realm: &mut Realm, this: &Value) -> JsResult<ObjectId> {
    match this {
        Value::Object(o) if realm.host_data::<ParamsData>(*o).is_some() => Ok(*o),
        _ => Err(realm.throw_type_error("Illegal invocation")),
    }
}

fn params_pairs(realm: &Realm, o: ObjectId) -> Vec<(String, String)> {
    realm.host_data::<ParamsData>(o).map(|d| d.pairs.clone()).unwrap_or_default()
}

fn params_update(realm: &mut Realm, o: ObjectId, pairs: Vec<(String, String)>) {
    let owner = realm.host_data::<ParamsData>(o).and_then(|d| d.owner);
    let q = serialize_query(&pairs);
    if let Some(d) = realm.host_data_mut::<ParamsData>(o) {
        d.pairs = pairs;
    }
    if let Some(u) = owner {
        if let Some(d) = realm.host_data_mut::<UrlData>(u) {
            d.url.query = if q.is_empty() { None } else { Some(q) };
        }
    }
}

fn params_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let init = arg(args, 0);
    let pairs: Vec<(String, String)> = match &init {
        Value::Undefined | Value::Null => Vec::new(),
        Value::String(s) => parse_query(&s.to_rust_string()),
        Value::Object(o) => {
            if let Some(d) = realm.host_data::<ParamsData>(*o) {
                d.pairs.clone()
            } else if realm.obj(*o).is_array() || realm.get_method(init.clone(), PropertyKey::Symbol(rabbithole_js::realm::SYM_ITERATOR))?.is_some() {
                let items = realm.iterate_to_vec(init.clone())?;
                let mut out = Vec::new();
                for it in items {
                    let pair = realm.create_list_from_array_like(it)?;
                    if pair.len() != 2 {
                        return Err(realm.throw_type_error("Failed to construct 'URLSearchParams': Sequence initializer must only contain pair elements"));
                    }
                    let k = realm.to_rust_string(pair[0].clone())?;
                    let v = realm.to_rust_string(pair[1].clone())?;
                    out.push((k, v));
                }
                out
            } else {
                let keys = realm.own_enumerable_string_keys(*o)?;
                let mut out = Vec::new();
                for k in keys {
                    let v = realm.get(init.clone(), k.clone())?;
                    let ks = k.as_str().map(|s| s.to_rust_string()).unwrap_or_default();
                    out.push((ks, realm.to_rust_string(v)?));
                }
                out
            }
        }
        other => parse_query(&realm.to_rust_string(other.clone())?),
    };
    let o = make_params(realm, pairs, None);
    let p = realm.proto_from_new_target(proto("URLSearchParams"));
    realm.obj_mut(o).proto = Some(p);
    Ok(Value::Object(o))
}

fn params_get(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = params_of(realm, &this)?;
    let k = arg_str(realm, args, 0)?;
    Ok(params_pairs(realm, o).iter().find(|(kk, _)| *kk == k).map(|(_, v)| Value::str(v)).unwrap_or(Value::Null))
}

fn params_get_all(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = params_of(realm, &this)?;
    let k = arg_str(realm, args, 0)?;
    let vals: Vec<Value> = params_pairs(realm, o).iter().filter(|(kk, _)| *kk == k).map(|(_, v)| Value::str(v)).collect();
    Ok(realm.new_array_value(vals))
}

fn params_has(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = params_of(realm, &this)?;
    let k = arg_str(realm, args, 0)?;
    let v = arg_str_opt(realm, args, 1)?;
    Ok(Value::Bool(params_pairs(realm, o).iter().any(|(kk, vv)| *kk == k && v.as_ref().map(|v| vv == v).unwrap_or(true))))
}

fn params_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = params_of(realm, &this)?;
    let k = arg_str(realm, args, 0)?;
    let v = arg_str(realm, args, 1)?;
    let mut pairs = params_pairs(realm, o);
    match pairs.iter().position(|(kk, _)| *kk == k) {
        Some(i) => {
            pairs[i].1 = v;
            let mut seen = false;
            pairs.retain(|(kk, _)| {
                if *kk == k {
                    if seen {
                        return false;
                    }
                    seen = true;
                }
                true
            });
        }
        None => pairs.push((k, v)),
    }
    params_update(realm, o, pairs);
    Ok(Value::Undefined)
}

fn params_append(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = params_of(realm, &this)?;
    let k = arg_str(realm, args, 0)?;
    let v = arg_str(realm, args, 1)?;
    let mut pairs = params_pairs(realm, o);
    pairs.push((k, v));
    params_update(realm, o, pairs);
    Ok(Value::Undefined)
}

fn params_delete(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = params_of(realm, &this)?;
    let k = arg_str(realm, args, 0)?;
    let v = arg_str_opt(realm, args, 1)?;
    let mut pairs = params_pairs(realm, o);
    pairs.retain(|(kk, vv)| !(*kk == k && v.as_ref().map(|v| vv == v).unwrap_or(true)));
    params_update(realm, o, pairs);
    Ok(Value::Undefined)
}

fn params_sort(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = params_of(realm, &this)?;
    let mut pairs = params_pairs(realm, o);
    pairs.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
    params_update(realm, o, pairs);
    Ok(Value::Undefined)
}

fn params_to_string(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = params_of(realm, &this)?;
    Ok(Value::str(&serialize_query(&params_pairs(realm, o))))
}

fn params_size(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = params_of(realm, &this)?;
    Ok(Value::Number(params_pairs(realm, o).len() as f64))
}

fn params_for_each(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = params_of(realm, &this)?;
    let f = arg(args, 0);
    for (k, v) in params_pairs(realm, o) {
        realm.call(f.clone(), arg(args, 1), &[Value::str(&v), Value::str(&k), this.clone()])?;
    }
    Ok(Value::Undefined)
}

fn params_iter(realm: &mut Realm, this: Value, kind: u8) -> JsResult<Value> {
    let o = params_of(realm, &this)?;
    let items: Vec<Value> = params_pairs(realm, o)
        .into_iter()
        .map(|(k, v)| match kind {
            0 => Value::str(&k),
            1 => Value::str(&v),
            _ => realm.new_array_value(vec![Value::str(&k), Value::str(&v)]),
        })
        .collect();
    let arr = realm.new_array_value(items);
    realm.invoke(arr, "values", &[])
}

fn params_entries(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    params_iter(realm, this, 2)
}

fn params_keys(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    params_iter(realm, this, 0)
}

fn params_values(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    params_iter(realm, this, 1)
}

// --------------------------------------------------------------- Headers

pub(crate) struct HeadersData {
    pub list: Vec<(String, String)>,
}

fn headers_of(realm: &mut Realm, this: &Value) -> JsResult<ObjectId> {
    match this {
        Value::Object(o) if realm.host_data::<HeadersData>(*o).is_some() => Ok(*o),
        _ => Err(realm.throw_type_error("Illegal invocation")),
    }
}

pub(crate) fn make_headers(realm: &mut Realm, list: Vec<(String, String)>) -> ObjectId {
    let p = proto("Headers");
    let o = realm.new_object_with_proto(Some(p));
    realm.set_host_data(o, Box::new(HeadersData { list }));
    realm.set_class_name(o, "Headers");
    o
}

/// Convert a Headers init (object, array of pairs, Headers) to a list.
pub(crate) fn headers_from_init(realm: &mut Realm, init: &Value) -> JsResult<Vec<(String, String)>> {
    match init {
        Value::Undefined | Value::Null => Ok(Vec::new()),
        Value::Object(o) => {
            if let Some(h) = realm.host_data::<HeadersData>(*o) {
                return Ok(h.list.clone());
            }
            if realm.obj(*o).is_array() {
                let items = realm.iterate_to_vec(init.clone())?;
                let mut out = Vec::new();
                for it in items {
                    let pair = realm.create_list_from_array_like(it)?;
                    if pair.len() == 2 {
                        let k = realm.to_rust_string(pair[0].clone())?.to_ascii_lowercase();
                        let v = realm.to_rust_string(pair[1].clone())?;
                        out.push((k, v));
                    }
                }
                return Ok(out);
            }
            let keys = realm.own_enumerable_string_keys(*o)?;
            let mut out = Vec::new();
            for k in keys {
                let v = realm.get(init.clone(), k.clone())?;
                let ks = k.as_str().map(|s| s.to_rust_string().to_ascii_lowercase()).unwrap_or_default();
                out.push((ks, realm.to_rust_string(v)?));
            }
            Ok(out)
        }
        _ => Ok(Vec::new()),
    }
}

fn headers_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let list = headers_from_init(realm, &arg(args, 0))?;
    Ok(Value::Object(make_headers(realm, list)))
}

fn headers_get(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = headers_of(realm, &this)?;
    let k = arg_str(realm, args, 0)?.to_ascii_lowercase();
    let vals: Vec<String> = realm.host_data::<HeadersData>(o).unwrap().list.iter().filter(|(kk, _)| *kk == k).map(|(_, v)| v.clone()).collect();
    Ok(if vals.is_empty() { Value::Null } else { Value::str(&vals.join(", ")) })
}

fn headers_has(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = headers_of(realm, &this)?;
    let k = arg_str(realm, args, 0)?.to_ascii_lowercase();
    Ok(Value::Bool(realm.host_data::<HeadersData>(o).unwrap().list.iter().any(|(kk, _)| *kk == k)))
}

fn headers_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = headers_of(realm, &this)?;
    let k = arg_str(realm, args, 0)?.to_ascii_lowercase();
    let v = arg_str(realm, args, 1)?;
    let d = realm.host_data_mut::<HeadersData>(o).unwrap();
    d.list.retain(|(kk, _)| *kk != k);
    d.list.push((k, v));
    Ok(Value::Undefined)
}

fn headers_append(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = headers_of(realm, &this)?;
    let k = arg_str(realm, args, 0)?.to_ascii_lowercase();
    let v = arg_str(realm, args, 1)?;
    realm.host_data_mut::<HeadersData>(o).unwrap().list.push((k, v));
    Ok(Value::Undefined)
}

fn headers_delete(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = headers_of(realm, &this)?;
    let k = arg_str(realm, args, 0)?.to_ascii_lowercase();
    realm.host_data_mut::<HeadersData>(o).unwrap().list.retain(|(kk, _)| *kk != k);
    Ok(Value::Undefined)
}

fn headers_for_each(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = headers_of(realm, &this)?;
    let f = arg(args, 0);
    let list = realm.host_data::<HeadersData>(o).unwrap().list.clone();
    for (k, v) in list {
        realm.call(f.clone(), arg(args, 1), &[Value::str(&v), Value::str(&k), this.clone()])?;
    }
    Ok(Value::Undefined)
}

fn headers_iter(realm: &mut Realm, this: Value, kind: u8) -> JsResult<Value> {
    let o = headers_of(realm, &this)?;
    let mut list = realm.host_data::<HeadersData>(o).unwrap().list.clone();
    list.sort_by(|a, b| a.0.cmp(&b.0));
    let items: Vec<Value> = list
        .into_iter()
        .map(|(k, v)| match kind {
            0 => Value::str(&k),
            1 => Value::str(&v),
            _ => realm.new_array_value(vec![Value::str(&k), Value::str(&v)]),
        })
        .collect();
    let arr = realm.new_array_value(items);
    realm.invoke(arr, "values", &[])
}

fn headers_entries(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    headers_iter(realm, this, 2)
}

fn headers_keys(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    headers_iter(realm, this, 0)
}

fn headers_values(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    headers_iter(realm, this, 1)
}

// --------------------------------------------------------- Request/Response

pub(crate) struct BodyData {
    pub bytes: Vec<u8>,
    pub used: bool,
}

fn body_bytes_from_value(realm: &mut Realm, v: &Value) -> JsResult<(Vec<u8>, Option<String>)> {
    match v {
        Value::Undefined | Value::Null => Ok((Vec::new(), None)),
        Value::String(s) => Ok((s.to_rust_string().into_bytes(), Some("text/plain;charset=UTF-8".into()))),
        Value::Object(o) => {
            if let Some(p) = realm.host_data::<ParamsData>(*o) {
                return Ok((serialize_query(&p.pairs).into_bytes(), Some("application/x-www-form-urlencoded;charset=UTF-8".into())));
            }
            if let Some(f) = realm.host_data::<FormDataData>(*o) {
                let fields: Vec<(String, String)> = f.fields.clone();
                let b = Body::multipart(&fields, &[]);
                return Ok((b.data, Some(b.content_type)));
            }
            if let Some(b) = realm.host_data::<BodyData>(*o) {
                return Ok((b.bytes.clone(), None));
            }
            match &realm.obj(*o).kind {
                rabbithole_js::ObjectKind::ArrayBuffer(buf) => Ok((buf.borrow().clone(), None)),
                rabbithole_js::ObjectKind::TypedArray(t) => {
                    let buf = realm.obj(t.buffer);
                    if let rabbithole_js::ObjectKind::ArrayBuffer(b) = &buf.kind {
                        let b = b.borrow();
                        let start = t.byte_offset.min(b.len());
                        let end = (t.byte_offset + t.length * t.kind.element_size()).min(b.len());
                        return Ok((b[start..end].to_vec(), None));
                    }
                    Ok((Vec::new(), None))
                }
                _ => {
                    let s = realm.to_rust_string(v.clone())?;
                    Ok((s.into_bytes(), Some("text/plain;charset=UTF-8".into())))
                }
            }
        }
        other => {
            let s = realm.to_rust_string(other.clone())?;
            Ok((s.into_bytes(), Some("text/plain;charset=UTF-8".into())))
        }
    }
}

/// Build a Response object from bytes + metadata.
pub(crate) fn make_response(realm: &mut Realm, url: &str, status: u16, status_text: &str, headers: Vec<(String, String)>, body: Vec<u8>, ty: &str) -> Value {
    let p = proto("Response");
    let o = realm.new_object_with_proto(Some(p));
    realm.set_host_data(o, Box::new(BodyData { bytes: body, used: false }));
    realm.set_class_name(o, "Response");
    let h = make_headers(realm, headers);
    realm.define_property(o, "url", Value::str(url), ACC);
    realm.define_property(o, "status", Value::Number(status as f64), ACC);
    realm.define_property(o, "ok", Value::Bool((200..300).contains(&status)), ACC);
    realm.define_property(o, "statusText", Value::str(status_text), ACC);
    realm.define_property(o, "headers", Value::Object(h), ACC);
    realm.define_property(o, "type", Value::str(ty), ACC);
    realm.define_property(o, "redirected", Value::Bool(false), ACC);
    realm.define_property(o, "bodyUsed", Value::Bool(false), ACC);
    Value::Object(o)
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "",
    }
}

fn take_body(realm: &mut Realm, this: &Value) -> JsResult<Vec<u8>> {
    let Value::Object(o) = this else {
        return Err(realm.throw_type_error("Illegal invocation"));
    };
    let Some(b) = realm.host_data_mut::<BodyData>(*o) else {
        return Err(realm.throw_type_error("Illegal invocation"));
    };
    if b.used {
        return Err(realm.throw_type_error("body stream already read"));
    }
    b.used = true;
    let bytes = b.bytes.clone();
    realm.define_property(*o, "bodyUsed", Value::Bool(true), ACC);
    Ok(bytes)
}

fn resolved(realm: &mut Realm, v: Value) -> JsResult<Value> {
    let (p, resolve, _) = realm.new_promise();
    realm.call(Value::Object(resolve), Value::Undefined, &[v])?;
    Ok(Value::Object(p))
}

fn rejected(realm: &mut Realm, kind: &str, msg: &str) -> JsResult<Value> {
    let (p, _, reject) = realm.new_promise();
    let e = realm.new_error(kind, msg);
    realm.call(Value::Object(reject), Value::Undefined, &[e])?;
    Ok(Value::Object(p))
}

fn body_text(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let bytes = match take_body(realm, &this) {
        Ok(b) => b,
        Err(e) => {
            let msg = realm.error_to_string(&e);
            return rejected(realm, "TypeError", &msg);
        }
    };
    let s = String::from_utf8_lossy(&bytes).into_owned();
    resolved(realm, Value::str(&s))
}

fn body_json(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let bytes = match take_body(realm, &this) {
        Ok(b) => b,
        Err(e) => {
            let msg = realm.error_to_string(&e);
            return rejected(realm, "TypeError", &msg);
        }
    };
    let s = String::from_utf8_lossy(&bytes).into_owned();
    let json = realm.get_str(Value::Object(realm.global()), "JSON")?;
    let parse = realm.get_str(json.clone(), "parse")?;
    match realm.call(parse, json, &[Value::str(&s)]) {
        Ok(v) => resolved(realm, v),
        Err(e) => {
            let (p, _, reject) = realm.new_promise();
            realm.call(Value::Object(reject), Value::Undefined, &[e])?;
            Ok(Value::Object(p))
        }
    }
}

pub(crate) fn make_array_buffer(realm: &mut Realm, bytes: Vec<u8>) -> Value {
    let ctor = realm.get_str(Value::Object(realm.global()), "ArrayBuffer").unwrap_or(Value::Undefined);
    if realm.is_constructor(&ctor) {
        if let Ok(Value::Object(ab)) = realm.construct(ctor, &[Value::Number(bytes.len() as f64)], Value::Undefined) {
            if let rabbithole_js::ObjectKind::ArrayBuffer(buf) = &realm.obj(ab).kind {
                *buf.borrow_mut() = bytes;
                return Value::Object(ab);
            }
        }
    }
    // Fallback: plain array of bytes.
    let vals: Vec<Value> = bytes.iter().map(|b| Value::Number(*b as f64)).collect();
    realm.new_array_value(vals)
}

fn body_array_buffer(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let bytes = match take_body(realm, &this) {
        Ok(b) => b,
        Err(e) => {
            let msg = realm.error_to_string(&e);
            return rejected(realm, "TypeError", &msg);
        }
    };
    let ab = make_array_buffer(realm, bytes);
    resolved(realm, ab)
}

fn body_blob(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let bytes = match take_body(realm, &this) {
        Ok(b) => b,
        Err(e) => {
            let msg = realm.error_to_string(&e);
            return rejected(realm, "TypeError", &msg);
        }
    };
    let ct = realm.get_str(this.clone(), "headers").ok().and_then(|h| h.as_object()).and_then(|h| realm.host_data::<HeadersData>(h).and_then(|d| d.list.iter().find(|(k, _)| k == "content-type").map(|(_, v)| v.clone()))).unwrap_or_default();
    let blob = make_blob(realm, bytes, &ct);
    resolved(realm, blob)
}

fn body_form_data(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let bytes = match take_body(realm, &this) {
        Ok(b) => b,
        Err(e) => {
            let msg = realm.error_to_string(&e);
            return rejected(realm, "TypeError", &msg);
        }
    };
    let s = String::from_utf8_lossy(&bytes).into_owned();
    let fd = make_form_data(realm, parse_query(&s));
    resolved(realm, Value::Object(fd))
}

fn response_clone(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let Value::Object(o) = &this else {
        return Err(realm.throw_type_error("Illegal invocation"));
    };
    let bytes = realm.host_data::<BodyData>(*o).map(|b| b.bytes.clone()).unwrap_or_default();
    let url = realm.get_str(this.clone(), "url")?;
    let status = realm.get_str(this.clone(), "status")?;
    let st = realm.get_str(this.clone(), "statusText")?;
    let headers = realm.get_str(this.clone(), "headers")?;
    let list = headers.as_object().and_then(|h| realm.host_data::<HeadersData>(h).map(|d| d.list.clone())).unwrap_or_default();
    let url = realm.to_rust_string(url)?;
    let status = realm.to_number(status)? as u16;
    let st = realm.to_rust_string(st)?;
    Ok(make_response(realm, &url, status, &st, list, bytes, "basic"))
}

fn response_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let (bytes, ct) = body_bytes_from_value(realm, &arg(args, 0))?;
    let init = arg(args, 1);
    let status = dict_value(realm, &init, "status")?;
    let status = if status.is_undefined() { 200 } else { realm.to_number(status)? as u16 };
    let st = dict_str(realm, &init, "statusText")?.unwrap_or_else(|| status_text(status).to_string());
    let hv = dict_value(realm, &init, "headers")?;
    let mut headers = headers_from_init(realm, &hv)?;
    if let Some(ct) = ct {
        if !headers.iter().any(|(k, _)| k == "content-type") {
            headers.push(("content-type".into(), ct));
        }
    }
    Ok(make_response(realm, "", status, &st, headers, bytes, "default"))
}

fn response_error(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(make_response(realm, "", 0, "", vec![], vec![], "error"))
}

fn response_json_static(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let json = realm.get_str(Value::Object(realm.global()), "JSON")?;
    let stringify = realm.get_str(json.clone(), "stringify")?;
    let s = realm.call(stringify, json, &[arg(args, 0)])?;
    let s = realm.to_rust_string(s)?;
    Ok(make_response(realm, "", 200, "OK", vec![("content-type".into(), "application/json".into())], s.into_bytes(), "default"))
}

/// Request object: stores url/method/headers/body as properties + host body.
pub(crate) fn make_request(realm: &mut Realm, url: &str, method: &str, headers: Vec<(String, String)>, body: Vec<u8>) -> Value {
    let p = proto("Request");
    let o = realm.new_object_with_proto(Some(p));
    realm.set_host_data(o, Box::new(BodyData { bytes: body, used: false }));
    realm.set_class_name(o, "Request");
    let h = make_headers(realm, headers);
    realm.define_property(o, "url", Value::str(url), ACC);
    realm.define_property(o, "method", Value::str(&method.to_ascii_uppercase()), ACC);
    realm.define_property(o, "headers", Value::Object(h), ACC);
    realm.define_property(o, "bodyUsed", Value::Bool(false), ACC);
    realm.define_property(o, "credentials", Value::str("same-origin"), ACC);
    realm.define_property(o, "mode", Value::str("cors"), ACC);
    realm.define_property(o, "cache", Value::str("default"), ACC);
    realm.define_property(o, "redirect", Value::str("follow"), ACC);
    realm.define_property(o, "referrer", Value::str("about:client"), ACC);
    let signal = crate::misc::new_abort_signal(realm);
    realm.define_property(o, "signal", Value::Object(signal), ACC);
    Value::Object(o)
}

/// Resolve (url, method, headers, body) from fetch/Request arguments.
fn request_parts(realm: &mut Realm, input: &Value, init: &Value) -> JsResult<(String, String, Vec<(String, String)>, Vec<u8>)> {
    let (mut url, mut method, mut headers, mut body) = match input {
        Value::Object(o) if realm.host_data::<BodyData>(*o).is_some() && realm.get_str(input.clone(), "method").map(|m| !m.is_undefined()).unwrap_or(false) => {
            let u = realm.get_str(input.clone(), "url")?;
            let m = realm.get_str(input.clone(), "method")?;
            let h = realm.get_str(input.clone(), "headers")?;
            let list = h.as_object().and_then(|h| realm.host_data::<HeadersData>(h).map(|d| d.list.clone())).unwrap_or_default();
            let b = realm.host_data::<BodyData>(*o).map(|b| b.bytes.clone()).unwrap_or_default();
            (realm.to_rust_string(u)?, realm.to_rust_string(m)?, list, b)
        }
        _ => {
            let u = match realm_url_of(realm, input) {
                Some(u) => u.to_string(),
                None => realm.to_rust_string(input.clone())?,
            };
            (u, "GET".to_string(), Vec::new(), Vec::new())
        }
    };
    if init.is_object() {
        if let Some(m) = dict_str(realm, init, "method")? {
            method = m.to_ascii_uppercase();
        }
        let hv = dict_value(realm, init, "headers")?;
        if !hv.is_undefined() {
            headers = headers_from_init(realm, &hv)?;
        }
        let bv = dict_value(realm, init, "body")?;
        if !bv.is_nullish() {
            let (b, ct) = body_bytes_from_value(realm, &bv)?;
            body = b;
            if let Some(ct) = ct {
                if !headers.iter().any(|(k, _)| k == "content-type") {
                    headers.push(("content-type".into(), ct));
                }
            }
        }
    }
    url = resolve_url(&url).map(|u| u.to_string()).unwrap_or(url);
    Ok((url, method, headers, body))
}

fn request_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let (url, method, headers, body) = request_parts(realm, &arg(args, 0), &arg(args, 1))?;
    let req = make_request(realm, &url, &method, headers, body);
    // `signal` follows init.signal, else the input Request's signal.
    let mut signal = Value::Undefined;
    if let init @ Value::Object(_) = arg(args, 1) {
        signal = realm.get_str(init, "signal")?;
    }
    if signal.is_nullish() {
        if let input @ Value::Object(_) = arg(args, 0) {
            if realm.get_str(input.clone(), "method").map(|m| !m.is_undefined()).unwrap_or(false) {
                signal = realm.get_str(input, "signal")?;
            }
        }
    }
    if let (Value::Object(o), Value::Object(_)) = (&req, &signal) {
        realm.define_property(*o, "signal", signal, ACC);
    }
    Ok(req)
}

fn request_clone(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let (url, method, headers, body) = request_parts(realm, &this, &Value::Undefined)?;
    Ok(make_request(realm, &url, &method, headers, body))
}

/// Perform a request through the environment synchronously.
pub(crate) fn do_fetch(url: &str, method: &str, headers: &[(String, String)], body: &[u8]) -> Result<rabbithole_net::Response, String> {
    let Some(u) = Url::parse_absolute(url) else {
        return Err(format!("Failed to fetch: invalid URL {url}"));
    };
    let mut req = if body.is_empty() && method == "GET" {
        Request::get(u)
    } else {
        let ct = headers.iter().find(|(k, _)| k == "content-type").map(|(_, v)| v.clone()).unwrap_or_else(|| "application/octet-stream".into());
        let mut r = Request::post(u, Body { content_type: ct, data: body.to_vec() });
        r.method = method.to_string();
        if body.is_empty() {
            r.body = None;
        }
        r
    };
    for (k, v) in headers {
        if k != "content-type" {
            req = req.with_header(k, v);
        }
    }
    let referer = with_env(|env| env.document_url());
    // Fetch-standard semantics (CORS, credentials, opaque responses) are
    // applied by the network layer for requests with a mode.
    req = req.with_referer(referer).with_dest(rabbithole_net::Destination::Empty).with_mode(rabbithole_net::RequestMode::Cors, rabbithole_net::CredentialsMode::SameOrigin);
    with_env(|env| env.fetch(req))
}

fn fetch_fn(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let (url, method, headers, body) = match request_parts(realm, &arg(args, 0), &arg(args, 1)) {
        Ok(p) => p,
        Err(e) => {
            let (p, _, reject) = realm.new_promise();
            realm.call(Value::Object(reject), Value::Undefined, &[e])?;
            return Ok(Value::Object(p));
        }
    };
    let (p, resolve, reject) = realm.new_promise();
    match do_fetch(&url, &method, &headers, &body) {
        Ok(resp) => {
            let st = status_text(resp.status).to_string();
            let hdrs = resp.headers.clone();
            let r = make_response(realm, &resp.url.to_string(), resp.status, &st, hdrs, resp.body, "basic");
            realm.call(Value::Object(resolve), Value::Undefined, &[r])?;
        }
        Err(e) => {
            let err = realm.new_error("TypeError", &format!("Failed to fetch: {e}"));
            realm.call(Value::Object(reject), Value::Undefined, &[err])?;
        }
    }
    Ok(Value::Object(p))
}

// ------------------------------------------------------------ FormData

pub(crate) struct FormDataData {
    pub fields: Vec<(String, String)>,
}

pub(crate) fn make_form_data(realm: &mut Realm, fields: Vec<(String, String)>) -> ObjectId {
    let p = proto("FormData");
    let o = realm.new_object_with_proto(Some(p));
    realm.set_host_data(o, Box::new(FormDataData { fields }));
    realm.set_class_name(o, "FormData");
    o
}

fn form_data_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let mut fields = Vec::new();
    if let Some(form) = node_of(realm, &arg(args, 0)) {
        let (f, _) = with_doc(form.doc, |doc| crate::misc::forms_data_set(doc, form.node));
        fields = f;
    }
    Ok(Value::Object(make_form_data(realm, fields)))
}

fn fd_of(realm: &mut Realm, this: &Value) -> JsResult<ObjectId> {
    match this {
        Value::Object(o) if realm.host_data::<FormDataData>(*o).is_some() => Ok(*o),
        _ => Err(realm.throw_type_error("Illegal invocation")),
    }
}

fn fd_append(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = fd_of(realm, &this)?;
    let k = arg_str(realm, args, 0)?;
    let v = arg_str(realm, args, 1)?;
    realm.host_data_mut::<FormDataData>(o).unwrap().fields.push((k, v));
    Ok(Value::Undefined)
}

fn fd_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = fd_of(realm, &this)?;
    let k = arg_str(realm, args, 0)?;
    let v = arg_str(realm, args, 1)?;
    let d = realm.host_data_mut::<FormDataData>(o).unwrap();
    d.fields.retain(|(kk, _)| *kk != k);
    d.fields.push((k, v));
    Ok(Value::Undefined)
}

fn fd_get(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = fd_of(realm, &this)?;
    let k = arg_str(realm, args, 0)?;
    Ok(realm.host_data::<FormDataData>(o).unwrap().fields.iter().find(|(kk, _)| *kk == k).map(|(_, v)| Value::str(v)).unwrap_or(Value::Null))
}

fn fd_get_all(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = fd_of(realm, &this)?;
    let k = arg_str(realm, args, 0)?;
    let vals: Vec<Value> = realm.host_data::<FormDataData>(o).unwrap().fields.iter().filter(|(kk, _)| *kk == k).map(|(_, v)| Value::str(v)).collect();
    Ok(realm.new_array_value(vals))
}

fn fd_has(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = fd_of(realm, &this)?;
    let k = arg_str(realm, args, 0)?;
    Ok(Value::Bool(realm.host_data::<FormDataData>(o).unwrap().fields.iter().any(|(kk, _)| *kk == k)))
}

fn fd_delete(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = fd_of(realm, &this)?;
    let k = arg_str(realm, args, 0)?;
    realm.host_data_mut::<FormDataData>(o).unwrap().fields.retain(|(kk, _)| *kk != k);
    Ok(Value::Undefined)
}

fn fd_iter(realm: &mut Realm, this: Value, kind: u8) -> JsResult<Value> {
    let o = fd_of(realm, &this)?;
    let items: Vec<Value> = realm
        .host_data::<FormDataData>(o)
        .unwrap()
        .fields
        .clone()
        .into_iter()
        .map(|(k, v)| match kind {
            0 => Value::str(&k),
            1 => Value::str(&v),
            _ => realm.new_array_value(vec![Value::str(&k), Value::str(&v)]),
        })
        .collect();
    let arr = realm.new_array_value(items);
    realm.invoke(arr, "values", &[])
}

fn fd_entries(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    fd_iter(realm, this, 2)
}

fn fd_keys(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    fd_iter(realm, this, 0)
}

fn fd_values(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    fd_iter(realm, this, 1)
}

fn fd_for_each(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = fd_of(realm, &this)?;
    let f = arg(args, 0);
    let fields = realm.host_data::<FormDataData>(o).unwrap().fields.clone();
    for (k, v) in fields {
        realm.call(f.clone(), arg(args, 1), &[Value::str(&v), Value::str(&k), this.clone()])?;
    }
    Ok(Value::Undefined)
}

// ---------------------------------------------------------------- Blob

pub(crate) fn make_blob(realm: &mut Realm, bytes: Vec<u8>, ty: &str) -> Value {
    let p = proto("Blob");
    let o = realm.new_object_with_proto(Some(p));
    let len = bytes.len();
    realm.set_host_data(o, Box::new(BodyData { bytes, used: false }));
    realm.set_class_name(o, "Blob");
    realm.define_property(o, "size", Value::Number(len as f64), ACC);
    realm.define_property(o, "type", Value::str(ty), ACC);
    Value::Object(o)
}

fn blob_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let parts = arg(args, 0);
    let mut bytes = Vec::new();
    if parts.is_object() {
        for p in realm.iterate_to_vec(parts)? {
            let (b, _) = body_bytes_from_value(realm, &p)?;
            bytes.extend(b);
        }
    }
    let ty = dict_str(realm, &arg(args, 1), "type")?.unwrap_or_default();
    Ok(make_blob(realm, bytes, &ty))
}

fn blob_text(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let Value::Object(o) = &this else {
        return Err(realm.throw_type_error("Illegal invocation"));
    };
    let bytes = realm.host_data::<BodyData>(*o).map(|b| b.bytes.clone()).unwrap_or_default();
    resolved(realm, Value::str(&String::from_utf8_lossy(&bytes)))
}

fn blob_array_buffer(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let Value::Object(o) = &this else {
        return Err(realm.throw_type_error("Illegal invocation"));
    };
    let bytes = realm.host_data::<BodyData>(*o).map(|b| b.bytes.clone()).unwrap_or_default();
    let ab = make_array_buffer(realm, bytes);
    resolved(realm, ab)
}

fn blob_slice(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let Value::Object(o) = &this else {
        return Err(realm.throw_type_error("Illegal invocation"));
    };
    let bytes = realm.host_data::<BodyData>(*o).map(|b| b.bytes.clone()).unwrap_or_default();
    let start = realm.relative_index(arg(args, 0), bytes.len(), 0)?;
    let end = realm.relative_index(arg(args, 1), bytes.len(), bytes.len())?;
    let ty = arg_str_opt(realm, args, 2)?.unwrap_or_default();
    Ok(make_blob(realm, bytes[start.min(end)..end].to_vec(), &ty))
}

fn url_create_object_url(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let mut buf = [0u8; 8];
    with_env(|env| env.random_bytes(&mut buf));
    let _ = realm;
    Ok(Value::str(&format!("blob:rabbithole/{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}", buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7])))
}

fn url_revoke_object_url(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Undefined)
}

// -------------------------------------------------------- TextEncoder/Decoder

fn text_encoder_ctor(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let p = realm.proto_from_new_target(proto("TextEncoder"));
    let o = realm.new_object_with_proto(Some(p));
    realm.define_property(o, "encoding", Value::str("utf-8"), ACC);
    Ok(Value::Object(o))
}

fn text_encoder_encode(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let s = arg_str_opt(realm, args, 0)?.unwrap_or_default();
    let bytes = s.into_bytes();
    let ctor = realm.get_str(Value::Object(realm.global()), "Uint8Array")?;
    if realm.is_constructor(&ctor) {
        let ab = make_array_buffer(realm, bytes);
        return realm.construct(ctor, &[ab], Value::Undefined);
    }
    let vals: Vec<Value> = bytes.iter().map(|b| Value::Number(*b as f64)).collect();
    Ok(realm.new_array_value(vals))
}

fn text_decoder_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let p = realm.proto_from_new_target(proto("TextDecoder"));
    let o = realm.new_object_with_proto(Some(p));
    let enc = arg_str_opt(realm, args, 0)?.unwrap_or_else(|| "utf-8".into()).to_ascii_lowercase();
    realm.define_property(o, "encoding", Value::str(&enc), ACC);
    realm.define_property(o, "fatal", Value::Bool(false), ACC);
    realm.define_property(o, "ignoreBOM", Value::Bool(false), ACC);
    Ok(Value::Object(o))
}

fn text_decoder_decode(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (bytes, _) = body_bytes_from_value(realm, &arg(args, 0))?;
    let enc = realm.get_str(this, "encoding")?;
    let enc = realm.to_rust_string(enc)?;
    Ok(Value::str(&rabbithole_net::decode_text(&bytes, Some(&enc))))
}

fn atob(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let s = arg_str(realm, args, 0)?;
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    match rabbithole_net::base64_decode(&cleaned) {
        Some(bytes) => Ok(Value::str(&bytes.iter().map(|b| *b as char).collect::<String>())),
        None => Err(crate::misc::dom_exception(realm, "InvalidCharacterError", "The string to be decoded is not correctly encoded.")),
    }
}

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

fn btoa(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let s = arg_str(realm, args, 0)?;
    let mut bytes = Vec::with_capacity(s.len());
    for c in s.chars() {
        if (c as u32) > 255 {
            return Err(crate::misc::dom_exception(realm, "InvalidCharacterError", "The string to be encoded contains characters outside of the Latin1 range."));
        }
        bytes.push(c as u8);
    }
    Ok(Value::str(&base64_encode(&bytes)))
}

// --------------------------------------------------------- XMLHttpRequest

pub(crate) struct XhrData {
    pub method: String,
    pub url: String,
    pub is_async: bool,
    pub headers: Vec<(String, String)>,
    pub response_headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub status: u16,
    pub status_text: String,
    pub response_url: String,
    pub ready_state: u8,
    pub sent: bool,
    pub aborted: bool,
}

fn xhr_of(realm: &mut Realm, this: &Value) -> JsResult<ObjectId> {
    match this {
        Value::Object(o) if realm.host_data::<XhrData>(*o).is_some() => Ok(*o),
        _ => Err(realm.throw_type_error("Illegal invocation")),
    }
}

fn xhr_ctor(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let p = realm.proto_from_new_target(proto("XMLHttpRequest"));
    let o = realm.new_object_with_proto(Some(p));
    realm.set_host_data(
        o,
        Box::new(XhrData {
            method: "GET".into(),
            url: String::new(),
            is_async: true,
            headers: Vec::new(),
            response_headers: Vec::new(),
            body: Vec::new(),
            status: 0,
            status_text: String::new(),
            response_url: String::new(),
            ready_state: 0,
            sent: false,
            aborted: false,
        }),
    );
    realm.set_class_name(o, "XMLHttpRequest");
    realm.define_property(o, "responseType", Value::str(""), ALL);
    realm.define_property(o, "timeout", Value::Number(0.0), ALL);
    realm.define_property(o, "withCredentials", Value::Bool(false), ALL);
    let upload = realm.new_object_with_proto(Some(proto("EventTarget")));
    realm.define_property(o, "upload", Value::Object(upload), HIDDEN);
    Ok(Value::Object(o))
}

fn xhr_open(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = xhr_of(realm, &this)?;
    let method = arg_str(realm, args, 0)?.to_ascii_uppercase();
    let url = arg_str(realm, args, 1)?;
    let url = resolve_url(&url).map(|u| u.to_string()).unwrap_or(url);
    let is_async = args.get(2).map(|v| realm.to_boolean(v)).unwrap_or(true);
    {
        let d = realm.host_data_mut::<XhrData>(o).unwrap();
        d.method = method;
        d.url = url;
        d.is_async = is_async;
        d.headers.clear();
        d.sent = false;
        d.aborted = false;
        d.ready_state = 1;
    }
    fire_xhr_event(realm, o, "readystatechange");
    Ok(Value::Undefined)
}

fn xhr_set_request_header(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = xhr_of(realm, &this)?;
    let k = arg_str(realm, args, 0)?.to_ascii_lowercase();
    let v = arg_str(realm, args, 1)?;
    realm.host_data_mut::<XhrData>(o).unwrap().headers.push((k, v));
    Ok(Value::Undefined)
}

fn fire_xhr_event(realm: &mut Realm, o: ObjectId, ty: &str) {
    crate::events::dispatch(realm, TargetKey::Object(o), ty, crate::EventInit::simple(false, false), true);
}

/// Perform the request now and fill the response fields.
fn xhr_perform(realm: &mut Realm, o: ObjectId) {
    let (url, method, headers, body) = {
        let d = realm.host_data::<XhrData>(o).unwrap();
        (d.url.clone(), d.method.clone(), d.headers.clone(), d.body.clone())
    };
    match do_fetch(&url, &method, &headers, &body) {
        Ok(resp) => {
            let d = realm.host_data_mut::<XhrData>(o).unwrap();
            d.status = resp.status;
            d.status_text = status_text(resp.status).to_string();
            d.response_headers = resp.headers;
            d.response_url = resp.url.to_string();
            d.body = resp.body;
            d.ready_state = 4;
        }
        Err(_) => {
            let d = realm.host_data_mut::<XhrData>(o).unwrap();
            d.status = 0;
            d.body = Vec::new();
            d.ready_state = 4;
            d.aborted = true;
        }
    }
}

fn xhr_finish_events(realm: &mut Realm, o: ObjectId) {
    let failed = realm.host_data::<XhrData>(o).map(|d| d.aborted).unwrap_or(true);
    fire_xhr_event(realm, o, "readystatechange");
    if failed {
        fire_xhr_event(realm, o, "error");
    } else {
        fire_xhr_event(realm, o, "load");
    }
    fire_xhr_event(realm, o, "loadend");
}

/// Job: deliver the result of an asynchronous XHR.
fn xhr_deliver(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let caps = native_captures(realm);
    if let Some(Value::Object(o)) = caps.first() {
        let o = *o;
        if realm.host_data::<XhrData>(o).map(|d| d.sent).unwrap_or(false) {
            fire_xhr_event(realm, o, "loadstart");
            xhr_perform(realm, o);
            xhr_finish_events(realm, o);
        }
    }
    Ok(Value::Undefined)
}

fn xhr_send(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = xhr_of(realm, &this)?;
    let (body, ct) = body_bytes_from_value(realm, &arg(args, 0))?;
    let is_async = {
        let d = realm.host_data_mut::<XhrData>(o).unwrap();
        if d.ready_state != 1 || d.sent {
            return Err(crate::misc::dom_exception(realm, "InvalidStateError", "The object's state must be OPENED."));
        }
        d.body = body;
        if let Some(ct) = ct {
            if !d.headers.iter().any(|(k, _)| k == "content-type") && !d.body.is_empty() {
                d.headers.push(("content-type".into(), ct));
            }
        }
        d.sent = true;
        d.is_async
    };
    if is_async {
        let f = realm.new_native_function_full("xhrDeliver", 0, xhr_deliver, None, vec![Value::Object(o)], false);
        realm.enqueue_callback(Value::Object(f), Value::Undefined, Vec::new());
    } else {
        xhr_perform(realm, o);
        xhr_finish_events(realm, o);
    }
    Ok(Value::Undefined)
}

fn xhr_abort(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = xhr_of(realm, &this)?;
    {
        let d = realm.host_data_mut::<XhrData>(o).unwrap();
        d.aborted = true;
        d.sent = false;
        d.ready_state = 0;
    }
    fire_xhr_event(realm, o, "abort");
    Ok(Value::Undefined)
}

fn xhr_get_response_header(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = xhr_of(realm, &this)?;
    let k = arg_str(realm, args, 0)?.to_ascii_lowercase();
    let vals: Vec<String> = realm.host_data::<XhrData>(o).unwrap().response_headers.iter().filter(|(kk, _)| *kk == k).map(|(_, v)| v.clone()).collect();
    Ok(if vals.is_empty() { Value::Null } else { Value::str(&vals.join(", ")) })
}

fn xhr_get_all_response_headers(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = xhr_of(realm, &this)?;
    let s: String = realm.host_data::<XhrData>(o).unwrap().response_headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
    Ok(Value::str(&s))
}

fn xhr_override_mime(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Undefined)
}

macro_rules! xhr_getter {
    ($name:ident, $body:expr) => {
        fn $name(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
            let o = xhr_of(realm, &this)?;
            let d = realm.host_data::<XhrData>(o).unwrap();
            #[allow(clippy::redundant_closure_call)]
            Ok(($body)(d))
        }
    };
}
xhr_getter!(xhr_ready_state, |d: &XhrData| Value::Number(d.ready_state as f64));
xhr_getter!(xhr_status, |d: &XhrData| Value::Number(d.status as f64));
xhr_getter!(xhr_status_text, |d: &XhrData| Value::str(&d.status_text));
xhr_getter!(xhr_response_url, |d: &XhrData| Value::str(&d.response_url));
xhr_getter!(xhr_response_text, |d: &XhrData| Value::str(&String::from_utf8_lossy(&d.body)));

fn xhr_response(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = xhr_of(realm, &this)?;
    let rt = realm.get_str(this.clone(), "responseType")?;
    let rt = realm.to_rust_string(rt)?;
    let body = realm.host_data::<XhrData>(o).unwrap().body.clone();
    match rt.as_str() {
        "json" => {
            let s = String::from_utf8_lossy(&body).into_owned();
            let json = realm.get_str(Value::Object(realm.global()), "JSON")?;
            let parse = realm.get_str(json.clone(), "parse")?;
            Ok(realm.call(parse, json, &[Value::str(&s)]).unwrap_or(Value::Null))
        }
        "arraybuffer" => Ok(make_array_buffer(realm, body)),
        "blob" => Ok(make_blob(realm, body, "")),
        "document" => Ok(Value::Null),
        _ => Ok(Value::str(&String::from_utf8_lossy(&body))),
    }
}

fn xhr_response_xml(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Null)
}

// ---------------------------------------------------------------- install

pub(crate) fn install(realm: &mut Realm) {
    // URL
    let url = define_class(realm, "URL", None, url_ctor);
    for comp in ["href", "protocol", "host", "hostname", "port", "pathname", "search", "hash", "origin"] {
        let data: std::rc::Rc<dyn std::any::Any> = std::rc::Rc::new(comp.to_string());
        let g = realm.new_native_function_full(&format!("get {comp}"), 0, url_get, Some(data.clone()), vec![], false);
        let s = if comp == "origin" { None } else { Some(Value::Object(realm.new_native_function_full(&format!("set {comp}"), 1, url_set, Some(data), vec![], false))) };
        realm.define_accessor_values(url, comp, Some(Value::Object(g)), s, ACC);
    }
    realm.define_accessor(url, "searchParams", Some(url_search_params), None, ACC);
    realm.define_method(url, "toString", 0, url_to_string);
    realm.define_method(url, "toJSON", 0, url_to_string);
    let url_ctor_v = realm.obj(url).props.get(&PropertyKey::str("constructor")).map(|p| p.value.clone()).unwrap_or(Value::Undefined);
    if let Value::Object(c) = url_ctor_v {
        realm.define_method(c, "canParse", 1, url_can_parse);
        realm.define_method(c, "parse", 1, url_parse_static);
        realm.define_method(c, "createObjectURL", 1, url_create_object_url);
        realm.define_method(c, "revokeObjectURL", 1, url_revoke_object_url);
    }
    let url_global = realm.get_str(Value::Object(realm.global()), "URL").unwrap_or(Value::Undefined);
    realm.define_global("webkitURL", url_global);
    // URLSearchParams
    let usp = define_class(realm, "URLSearchParams", None, params_ctor);
    realm.define_method(usp, "get", 1, params_get);
    realm.define_method(usp, "getAll", 1, params_get_all);
    realm.define_method(usp, "has", 1, params_has);
    realm.define_method(usp, "set", 2, params_set);
    realm.define_method(usp, "append", 2, params_append);
    realm.define_method(usp, "delete", 1, params_delete);
    realm.define_method(usp, "sort", 0, params_sort);
    realm.define_method(usp, "toString", 0, params_to_string);
    realm.define_method(usp, "forEach", 1, params_for_each);
    realm.define_method(usp, "entries", 0, params_entries);
    realm.define_method(usp, "keys", 0, params_keys);
    realm.define_method(usp, "values", 0, params_values);
    realm.define_symbol_method(usp, rabbithole_js::realm::SYM_ITERATOR, "[Symbol.iterator]", 0, params_entries);
    realm.define_accessor(usp, "size", Some(params_size), None, ACC);
    // Headers
    let h = define_class(realm, "Headers", None, headers_ctor);
    realm.define_method(h, "get", 1, headers_get);
    realm.define_method(h, "has", 1, headers_has);
    realm.define_method(h, "set", 2, headers_set);
    realm.define_method(h, "append", 2, headers_append);
    realm.define_method(h, "delete", 1, headers_delete);
    realm.define_method(h, "forEach", 1, headers_for_each);
    realm.define_method(h, "entries", 0, headers_entries);
    realm.define_method(h, "keys", 0, headers_keys);
    realm.define_method(h, "values", 0, headers_values);
    realm.define_method(h, "getSetCookie", 0, headers_values);
    realm.define_symbol_method(h, rabbithole_js::realm::SYM_ITERATOR, "[Symbol.iterator]", 0, headers_entries);
    // Response / Request
    let resp = define_class(realm, "Response", None, response_ctor);
    for (name, f) in [("text", body_text as rabbithole_js::NativeFn), ("json", body_json), ("arrayBuffer", body_array_buffer), ("blob", body_blob), ("formData", body_form_data), ("bytes", body_array_buffer)] {
        realm.define_method(resp, name, 0, f);
    }
    realm.define_method(resp, "clone", 0, response_clone);
    realm.define_property(resp, "body", Value::Null, HIDDEN);
    let resp_ctor = realm.obj(resp).props.get(&PropertyKey::str("constructor")).map(|p| p.value.clone()).unwrap_or(Value::Undefined);
    if let Value::Object(c) = resp_ctor {
        realm.define_method(c, "error", 0, response_error);
        realm.define_method(c, "json", 1, response_json_static);
    }
    let req = define_class(realm, "Request", None, request_ctor);
    for (name, f) in [("text", body_text as rabbithole_js::NativeFn), ("json", body_json), ("arrayBuffer", body_array_buffer), ("blob", body_blob), ("formData", body_form_data)] {
        realm.define_method(req, name, 0, f);
    }
    realm.define_method(req, "clone", 0, request_clone);
    let g = realm.global();
    realm.define_method(g, "fetch", 1, fetch_fn);
    // FormData
    let fd = define_class(realm, "FormData", None, form_data_ctor);
    realm.define_method(fd, "append", 2, fd_append);
    realm.define_method(fd, "set", 2, fd_set);
    realm.define_method(fd, "get", 1, fd_get);
    realm.define_method(fd, "getAll", 1, fd_get_all);
    realm.define_method(fd, "has", 1, fd_has);
    realm.define_method(fd, "delete", 1, fd_delete);
    realm.define_method(fd, "entries", 0, fd_entries);
    realm.define_method(fd, "keys", 0, fd_keys);
    realm.define_method(fd, "values", 0, fd_values);
    realm.define_method(fd, "forEach", 1, fd_for_each);
    realm.define_symbol_method(fd, rabbithole_js::realm::SYM_ITERATOR, "[Symbol.iterator]", 0, fd_entries);
    // Blob / File
    let blob = define_class(realm, "Blob", None, blob_ctor);
    realm.define_method(blob, "text", 0, blob_text);
    realm.define_method(blob, "arrayBuffer", 0, blob_array_buffer);
    realm.define_method(blob, "slice", 2, blob_slice);
    realm.define_method(blob, "stream", 0, blob_text);
    define_class(realm, "File", Some("Blob"), blob_ctor);
    define_class(realm, "FileReader", Some("EventTarget"), file_reader_ctor);
    // TextEncoder / TextDecoder
    let te = define_class(realm, "TextEncoder", None, text_encoder_ctor);
    realm.define_method(te, "encode", 1, text_encoder_encode);
    let td = define_class(realm, "TextDecoder", None, text_decoder_ctor);
    realm.define_method(td, "decode", 1, text_decoder_decode);
    realm.define_method(g, "atob", 1, atob);
    realm.define_method(g, "btoa", 1, btoa);
    // XMLHttpRequest
    let xhr = define_class(realm, "XMLHttpRequest", Some("EventTarget"), xhr_ctor);
    realm.define_method(xhr, "open", 2, xhr_open);
    realm.define_method(xhr, "send", 1, xhr_send);
    realm.define_method(xhr, "abort", 0, xhr_abort);
    realm.define_method(xhr, "setRequestHeader", 2, xhr_set_request_header);
    realm.define_method(xhr, "getResponseHeader", 1, xhr_get_response_header);
    realm.define_method(xhr, "getAllResponseHeaders", 0, xhr_get_all_response_headers);
    realm.define_method(xhr, "overrideMimeType", 1, xhr_override_mime);
    realm.define_accessor(xhr, "readyState", Some(xhr_ready_state), None, ACC);
    realm.define_accessor(xhr, "status", Some(xhr_status), None, ACC);
    realm.define_accessor(xhr, "statusText", Some(xhr_status_text), None, ACC);
    realm.define_accessor(xhr, "responseURL", Some(xhr_response_url), None, ACC);
    realm.define_accessor(xhr, "responseText", Some(xhr_response_text), None, ACC);
    realm.define_accessor(xhr, "response", Some(xhr_response), None, ACC);
    realm.define_accessor(xhr, "responseXML", Some(xhr_response_xml), None, ACC);
    for (k, v) in [("UNSENT", 0.0), ("OPENED", 1.0), ("HEADERS_RECEIVED", 2.0), ("LOADING", 3.0), ("DONE", 4.0)] {
        realm.define_property(xhr, k, Value::Number(v), 0);
    }
    crate::events::install_handler_props(realm, xhr, &["readystatechange", "load", "error", "abort", "loadend", "loadstart", "progress", "timeout"]);
    define_class(realm, "XMLHttpRequestUpload", Some("EventTarget"), illegal_ctor);
}

fn file_reader_ctor(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let p = realm.proto_from_new_target(proto("FileReader"));
    let o = realm.new_object_with_proto(Some(p));
    realm.define_property(o, "result", Value::Null, ALL);
    realm.define_property(o, "readyState", Value::Number(0.0), ALL);
    realm.define_property(o, "error", Value::Null, ALL);
    realm.define_method(o, "readAsText", 1, file_reader_read);
    realm.define_method(o, "readAsDataURL", 1, file_reader_read_data_url);
    realm.define_method(o, "readAsArrayBuffer", 1, file_reader_read_ab);
    realm.define_method(o, "abort", 0, xhr_override_mime);
    Ok(Value::Object(o))
}

fn file_reader_finish(realm: &mut Realm, this: Value, result: Value) -> JsResult<Value> {
    if let Value::Object(o) = &this {
        realm.define_property(*o, "result", result, ALL);
        realm.define_property(*o, "readyState", Value::Number(2.0), ALL);
        let f = realm.new_native_function_full("frDeliver", 0, file_reader_deliver, None, vec![this.clone()], false);
        realm.enqueue_callback(Value::Object(f), Value::Undefined, vec![]);
    }
    Ok(Value::Undefined)
}

fn file_reader_deliver(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let caps = native_captures(realm);
    if let Some(Value::Object(o)) = caps.first() {
        fire_xhr_event(realm, *o, "load");
        fire_xhr_event(realm, *o, "loadend");
    }
    Ok(Value::Undefined)
}

fn file_reader_read(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (bytes, _) = body_bytes_from_value(realm, &arg(args, 0))?;
    file_reader_finish(realm, this, Value::str(&String::from_utf8_lossy(&bytes)))
}

fn file_reader_read_data_url(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (bytes, _) = body_bytes_from_value(realm, &arg(args, 0))?;
    let ty = realm.get_str(arg(args, 0), "type").ok().map(|t| realm.to_rust_string(t).unwrap_or_default()).unwrap_or_default();
    let url = format!("data:{};base64,{}", if ty.is_empty() { "application/octet-stream".into() } else { ty }, base64_encode(&bytes));
    file_reader_finish(realm, this, Value::str(&url))
}

fn file_reader_read_ab(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (bytes, _) = body_bytes_from_value(realm, &arg(args, 0))?;
    let ab = make_array_buffer(realm, bytes);
    file_reader_finish(realm, this, ab)
}
