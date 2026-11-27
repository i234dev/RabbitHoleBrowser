//! `Element`, `HTMLElement` and the tag-specific subclasses.

use crate::env::with_env;
use rabbithole_js::V;
use crate::host::*;
use crate::node::{self, children_of, insert_node, remove_node, set_inner_html, set_text_content};
use rabbithole_dom::{ControlDirection, Document, NodeData, NodeId};
use rabbithole_js::{JsResult, ObjectId, PropertyKey, Realm, Value, ALL, CONFIGURABLE, ENUMERABLE};

const ACC: u8 = CONFIGURABLE | ENUMERABLE;

// ------------------------------------------------------- attribute reflection

pub(crate) use crate::reflect::{reflect, Reflect};

/// Set/remove an attribute with mutation records and dirtiness.
/// The attribute name to use on `n`: lower-cased for HTML elements in
/// HTML documents (DOM "get an attribute by name").
fn attr_name_for(n: NodeRef, name: &str) -> std::borrow::Cow<'_, str> {
    // (Already lower case: the same name either way, without a lookup.)
    if !name.bytes().any(|b| b.is_ascii_uppercase()) {
        return std::borrow::Cow::Borrowed(name);
    }
    let lower = with_doc(n.doc, |doc| doc.is_html && doc.element(n.node).map(|e| e.ns == rabbithole_dom::Namespace::Html).unwrap_or(true));
    if lower {
        std::borrow::Cow::Owned(name.to_ascii_lowercase())
    } else {
        std::borrow::Cow::Borrowed(name)
    }
}

pub(crate) fn set_attribute(realm: &mut Realm, n: NodeRef, name: &str, value: Option<&str>) {
    let name = attr_name_for(n, name);
    let old = with_doc(n.doc, |doc| {
        let old = doc.attr(n.node, &name).map(|s| s.to_string());
        doc.set_attr(n.node, &name, value);
        old
    });
    crate::shadow::on_attribute(n, &name, old.clone(), value);
    crate::media::note_attribute(n, &name);
    node::record_attribute(realm, n, &name, old);
    if n.doc == 0 {
        mark_dirty();
    }
}

pub(crate) fn get_attribute(n: NodeRef, name: &str) -> Option<String> {
    let name = attr_name_for(n, name);
    with_doc(n.doc, |doc| doc.attr(n.node, &name).map(|s| s.to_string()))
}

// ------------------------------------------------------------------ Element

fn e_tag_name(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "tagName")?;
    Ok(Value::str(&node::node_name(n)))
}

fn e_local_name(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "localName")?;
    Ok(Value::str(&tag_of(n).unwrap_or_default()))
}

fn e_namespace_uri(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "namespaceURI")?;
    let ns = with_doc(n.doc, |doc| doc.element(n.node)?.ns.uri().map(|s| s.to_string()));
    Ok(match ns {
        Some(u) => Value::str(&u),
        None => Value::Null,
    })
}

fn e_prefix(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "prefix")?;
    Ok(match with_doc(n.doc, |doc| doc.element(n.node).and_then(|e| e.prefix.clone())) {
        Some(p) => Value::str(&p),
        None => Value::Null,
    })
}

fn e_get_attribute(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "getAttribute")?;
    let name = arg_str(realm, args, 0)?;
    Ok(match get_attribute(n, &name) {
        Some(v) => Value::str(&v),
        None => Value::Null,
    })
}

/// `DOMString? namespace` argument (empty string is null).
fn ns_arg(realm: &mut Realm, args: &[Value], i: usize) -> JsResult<Option<String>> {
    Ok(match args.get(i).map(Value::v) {
        None | Some(V::Null) | Some(V::Undefined) => None,
        Some(v) => Some(realm.to_rust_string(v.to_value())?).filter(|s| !s.is_empty()),
    })
}

/// The stored name of the attribute of `n` with namespace `ns` and local
/// name `local` (DOM "get an attribute by namespace and local name").
pub(crate) fn find_attr_ns(n: NodeRef, ns: Option<&str>, local: &str) -> Option<(String, String)> {
    with_doc(n.doc, |doc| {
        let e = doc.element(n.node)?;
        e.attrs.iter().find_map(|(k, v)| {
            let attr_ns = doc.attr_namespace(n.node, k);
            let attr_local = if attr_ns.is_some() { k.split_once(':').map(|x| x.1).unwrap_or(k) } else { k.as_str() };
            (attr_ns == ns && attr_local == local).then(|| (k.clone(), v.clone()))
        })
    })
}

fn e_get_attribute_ns(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "getAttributeNS")?;
    let ns = ns_arg(realm, args, 0)?;
    let local = arg_str(realm, args, 1)?;
    Ok(match find_attr_ns(n, ns.as_deref(), &local) {
        Some((_, v)) => Value::str(&v),
        None => Value::Null,
    })
}

fn e_set_attribute(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "setAttribute")?;
    let name = arg_str(realm, args, 0)?;
    let value = arg_str(realm, args, 1)?;
    if !crate::document::valid_attribute_local_name(&name) {
        return Err(crate::misc::dom_exception(realm, "InvalidCharacterError", &format!("Failed to execute 'setAttribute' on 'Element': '{name}' is not a valid attribute name.")));
    }
    set_attribute(realm, n, &name, Some(&value));
    Ok(Value::Undefined)
}

fn e_set_attribute_ns(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "setAttributeNS")?;
    let ns = ns_arg(realm, args, 0)?;
    let qname = arg_str(realm, args, 1)?;
    let local = match crate::document::validate_and_extract_in(ns.clone(), &qname, true) {
        Ok((_, _, local)) => local,
        Err((err, msg)) => return Err(crate::misc::dom_exception(realm, err, &format!("Failed to execute 'setAttributeNS' on 'Element': {msg}"))),
    };
    let value = arg_str(realm, args, 2)?;
    // An existing attribute with this namespace and local name keeps its
    // qualified name; otherwise the attribute is added under `qname`.
    let name = match find_attr_ns(n, ns.as_deref(), &local) {
        Some((k, _)) => k,
        None => qname.clone(),
    };
    let old = with_doc(n.doc, |doc| {
        let old = doc.attr(n.node, &name).map(|s| s.to_string());
        let lower = doc.is_html;
        // Stored as given (no lower-casing for namespaced attributes).
        doc.is_html = false;
        doc.set_attr(n.node, &name, Some(&value));
        doc.is_html = lower;
        doc.set_attr_namespace(n.node, &name, ns.as_deref());
        old
    });
    crate::shadow::on_attribute(n, &name, old.clone(), Some(&value));
    node::record_attribute(realm, n, &name, old);
    if n.doc == 0 {
        mark_dirty();
    }
    Ok(Value::Undefined)
}

fn e_remove_attribute(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "removeAttribute")?;
    let name = arg_str(realm, args, 0)?;
    if get_attribute(n, &name).is_some() {
        set_attribute(realm, n, &name, None);
    }
    Ok(Value::Undefined)
}

fn e_has_attribute(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "hasAttribute")?;
    let name = arg_str(realm, args, 0)?;
    Ok(Value::Bool(get_attribute(n, &name).is_some()))
}

fn e_has_attributes(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "hasAttributes")?;
    Ok(Value::Bool(with_doc(n.doc, |doc| doc.element(n.node).map(|e| !e.attrs.is_empty()).unwrap_or(false))))
}

fn e_get_attribute_names(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "getAttributeNames")?;
    let names: Vec<Value> = with_doc(n.doc, |doc| doc.element(n.node).map(|e| e.attrs.iter().map(|(k, _)| Value::str(k)).collect()).unwrap_or_default());
    Ok(array(realm, names))
}

fn e_toggle_attribute(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "toggleAttribute")?;
    let name = arg_str(realm, args, 0)?;
    if !crate::document::valid_attribute_local_name(&name) {
        return Err(crate::misc::dom_exception(realm, "InvalidCharacterError", &format!("Failed to execute 'toggleAttribute' on 'Element': '{name}' is not a valid attribute name.")));
    }
    let has = get_attribute(n, &name).is_some();
    let force = args.get(1).filter(|v| !v.is_undefined()).map(|v| realm.to_boolean(v));
    let want = force.unwrap_or(!has);
    if want && !has {
        set_attribute(realm, n, &name, Some(""));
    } else if !want && has {
        set_attribute(realm, n, &name, None);
    }
    Ok(Value::Bool(want))
}

fn e_attributes(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "attributes")?;
    let attrs: Vec<(String, String)> = with_doc(n.doc, |doc| doc.element(n.node).map(|e| e.attrs.clone()).unwrap_or_default());
    let mut items = Vec::new();
    for (k, v) in &attrs {
        let a = object_from(realm, vec![("name", Value::str(k)), ("localName", Value::str(k)), ("value", Value::str(v)), ("nodeName", Value::str(k)), ("nodeValue", Value::str(v)), ("specified", Value::Bool(true))]);
        if let V::Object(o) = a.v() {
            let p = proto("Attr");
            realm.obj_mut(o).set_proto(Some(p));
            let owner = wrap(realm, n);
            realm.define_property(o, "ownerElement", owner, CONFIGURABLE);
        }
        items.push(a);
    }
    let arr = realm.new_array(items.clone());
    let p = proto("NamedNodeMap");
    realm.obj_mut(arr).set_proto(Some(p));
    for ((k, _), v) in attrs.iter().zip(items) {
        if !realm.obj(arr).props.contains(&PropertyKey::str(k)) {
            realm.define_property(arr, k.as_str(), v, rabbithole_js::HIDDEN);
        }
    }
    Ok(Value::Object(arr))
}

fn nnm_get_named_item(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let name = arg_str(realm, args, 0)?.to_ascii_lowercase();
    let v = realm.get(this, PropertyKey::str(&name))?;
    Ok(if v.is_undefined() { Value::Null } else { v })
}

fn e_id_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "id")?;
    Ok(Value::str(&get_attribute(n, "id").unwrap_or_default()))
}

fn e_class_name_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "className")?;
    let svg = with_doc(n.doc, |doc| doc.ancestor_or_self(n.node, |a| doc.is_element(a, "svg")).is_some());
    let v = get_attribute(n, "class").unwrap_or_default();
    if svg {
        // SVGAnimatedString
        return Ok(object_from(realm, vec![("baseVal", Value::str(&v)), ("animVal", Value::str(&v))]));
    }
    Ok(Value::str(&v))
}

fn e_class_name_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "className")?;
    let v = arg_str(realm, args, 0)?;
    set_attribute(realm, n, "class", Some(&v));
    Ok(Value::Undefined)
}

// DOMTokenList --------------------------------------------------------------

pub(crate) struct TokenListRef {
    pub node: NodeRef,
    pub attr: &'static str,
}

/// The ordered set parser over ASCII whitespace.
fn tokens_of(n: NodeRef, attr: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in get_attribute(n, attr).unwrap_or_default().split(|c: char| matches!(c, '\t' | '\n' | '\u{c}' | '\r' | ' ')).filter(|t| !t.is_empty()) {
        if !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    out
}

fn token_list_ref(realm: &mut Realm, this: &Value) -> JsResult<TokenListRef> {
    match this.v() {
        V::Object(o) => match realm.host_data::<TokenListRef>(o) {
            Some(r) => Ok(TokenListRef { node: r.node, attr: r.attr }),
            None => Err(realm.throw_type_error("Illegal invocation")),
        },
        _ => Err(realm.throw_type_error("Illegal invocation")),
    }
}

/// DOMTokenList "update steps": no attribute is created for an empty set.
fn write_tokens(realm: &mut Realm, r: &TokenListRef, tokens: &[String]) {
    if tokens.is_empty() && get_attribute(r.node, r.attr).is_none() {
        return;
    }
    set_attribute(realm, r.node, r.attr, Some(&tokens.join(" ")));
}

fn check_token(realm: &mut Realm, t: &str) -> JsResult<()> {
    if t.is_empty() {
        return Err(crate::misc::dom_exception(realm, "SyntaxError", "The token provided must not be empty."));
    }
    if t.contains(|c: char| matches!(c, '\t' | '\n' | '\u{c}' | '\r' | ' ')) {
        return Err(crate::misc::dom_exception(realm, "InvalidCharacterError", "The token provided contains HTML space characters, which are not valid in tokens."));
    }
    Ok(())
}

fn tl_add(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let r = token_list_ref(realm, &this)?;
    let mut new = Vec::with_capacity(args.len());
    for a in args {
        let t = realm.to_rust_string(a.clone())?;
        check_token(realm, &t)?;
        new.push(t);
    }
    let mut tokens = tokens_of(r.node, r.attr);
    for t in new {
        if !tokens.contains(&t) {
            tokens.push(t);
        }
    }
    write_tokens(realm, &r, &tokens);
    Ok(Value::Undefined)
}

fn tl_remove(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let r = token_list_ref(realm, &this)?;
    let mut gone = Vec::with_capacity(args.len());
    for a in args {
        let t = realm.to_rust_string(a.clone())?;
        check_token(realm, &t)?;
        gone.push(t);
    }
    let mut tokens = tokens_of(r.node, r.attr);
    tokens.retain(|x| !gone.contains(x));
    write_tokens(realm, &r, &tokens);
    Ok(Value::Undefined)
}

fn tl_toggle(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let r = token_list_ref(realm, &this)?;
    let t = arg_str(realm, args, 0)?;
    check_token(realm, &t)?;
    let mut tokens = tokens_of(r.node, r.attr);
    let has = tokens.contains(&t);
    let force = args.get(1).filter(|v| !v.is_undefined()).map(|v| realm.to_boolean(v));
    let want = force.unwrap_or(!has);
    if want && !has {
        tokens.push(t);
        write_tokens(realm, &r, &tokens);
    } else if !want && has {
        tokens.retain(|x| *x != t);
        write_tokens(realm, &r, &tokens);
    }
    Ok(Value::Bool(want))
}

fn tl_contains(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let r = token_list_ref(realm, &this)?;
    let t = arg_str(realm, args, 0)?;
    Ok(Value::Bool(tokens_of(r.node, r.attr).contains(&t)))
}

fn tl_replace(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let r = token_list_ref(realm, &this)?;
    let old = arg_str(realm, args, 0)?;
    let new = arg_str(realm, args, 1)?;
    check_token(realm, &old)?;
    check_token(realm, &new)?;
    let mut tokens = tokens_of(r.node, r.attr);
    let Some(pos) = tokens.iter().position(|x| *x == old) else { return Ok(Value::Bool(false)) };
    tokens[pos] = new.clone();
    let mut seen = Vec::new();
    tokens.retain(|t| {
        if seen.contains(t) {
            false
        } else {
            seen.push(t.clone());
            true
        }
    });
    write_tokens(realm, &r, &tokens);
    Ok(Value::Bool(true))
}

fn tl_item(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let r = token_list_ref(realm, &this)?;
    // `unsigned long`: ToUint32 (so -1 is 4294967295).
    let n = realm.to_number(arg(args, 0))?;
    let i = if n.is_finite() { (n.trunc().rem_euclid(4294967296.0)) as u64 } else { 0 };
    let tokens = tokens_of(r.node, r.attr);
    Ok(tokens.get(i as usize).map(|t| Value::str(t)).unwrap_or(Value::Null))
}

fn tl_length(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let r = token_list_ref(realm, &this)?;
    Ok(Value::Number(tokens_of(r.node, r.attr).len() as f64))
}

fn tl_value_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let r = token_list_ref(realm, &this)?;
    Ok(Value::str(&get_attribute(r.node, r.attr).unwrap_or_default()))
}

fn tl_value_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let r = token_list_ref(realm, &this)?;
    let v = arg_str(realm, args, 0)?;
    set_attribute(realm, r.node, r.attr, Some(&v));
    Ok(Value::Undefined)
}

fn tl_to_string(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    tl_value_get(realm, this, &[])
}

fn tl_for_each(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let r = token_list_ref(realm, &this)?;
    let f = arg(args, 0);
    let this_arg = arg(args, 1);
    for (i, t) in tokens_of(r.node, r.attr).iter().enumerate() {
        realm.call(f.clone(), this_arg.clone(), &[Value::str(t), Value::Number(i as f64), this.clone()])?;
    }
    Ok(Value::Undefined)
}

fn tl_iterator(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let r = token_list_ref(realm, &this)?;
    let vals: Vec<Value> = tokens_of(r.node, r.attr).iter().map(|t| Value::str(t)).collect();
    let arr = realm.new_array_value(vals);
    realm.invoke(arr, "values", &[])
}

fn tl_supports(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let r = token_list_ref(realm, &this)?;
    if r.attr == "class" {
        return Err(realm.throw_type_error("Failed to execute 'supports' on 'DOMTokenList': DOMTokenList has no supported tokens."));
    }
    Ok(Value::Bool(true))
}

/// The (cached) DOMTokenList for an attribute of a node.
pub(crate) fn token_list(realm: &mut Realm, wrapper: ObjectId, n: NodeRef, attr: &'static str) -> Value {
    let key = format!("__tokens_{attr}");
    let cached = get_hidden(realm, wrapper, &key);
    if cached.is_object() {
        return cached;
    }
    let p = proto("DOMTokenList");
    let o = realm.new_object_with_proto(Some(p));
    // DOMTokenList supports indexed properties: `list[i]` is the i-th
    // token of the current attribute value (read-only, live).
    realm.set_indexed_host_data(o, Box::new(TokenListRef { node: n, attr }));
    realm.set_class_name(o, "DOMTokenList");
    set_hidden(realm, wrapper, &key, Value::Object(o));
    Value::Object(o)
}

fn tl_index(data: &dyn std::any::Any, i: u32) -> Option<Value> {
    let r = data.downcast_ref::<TokenListRef>()?;
    tokens_of(r.node, r.attr).get(i as usize).map(|t| Value::str(t))
}

fn tl_count(data: &dyn std::any::Any) -> u32 {
    data.downcast_ref::<TokenListRef>().map(|r| tokens_of(r.node, r.attr).len() as u32).unwrap_or(0)
}

fn e_class_list(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "classList")?;
    let V::Object(w) = this.v() else { unreachable!() };
    Ok(token_list(realm, w, n, "class"))
}

fn e_rel_list(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "relList")?;
    let V::Object(w) = this.v() else { unreachable!() };
    Ok(token_list(realm, w, n, "rel"))
}

fn e_part(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "part")?;
    let V::Object(w) = this.v() else { unreachable!() };
    Ok(token_list(realm, w, n, "part"))
}

// `[PutForwards=value]`: assigning a string to `classList` / `relList` /
// `part` sets the attribute (and must not throw in strict code).
fn e_class_list_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    put_forward(realm, this, args, "classList", "class")
}

fn e_rel_list_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    put_forward(realm, this, args, "relList", "rel")
}

fn e_part_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    put_forward(realm, this, args, "part", "part")
}

fn put_forward(realm: &mut Realm, this: Value, args: &[Value], what: &str, attr: &str) -> JsResult<Value> {
    let n = this_node(realm, &this, what)?;
    let v = arg_str(realm, args, 0)?;
    set_attribute(realm, n, attr, Some(&v));
    Ok(Value::Undefined)
}

// dataset -------------------------------------------------------------------

fn camel_to_data(name: &str) -> String {
    let mut out = String::from("data-");
    for c in name.chars() {
        if c.is_ascii_uppercase() {
            out.push('-');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The DOMStringMap name of a `data-*` attribute: "-" followed by an ASCII
/// lowercase letter becomes that letter uppercased; None for attributes
/// that have no name (not `data-*`, or with ASCII uppercase letters).
fn data_to_camel(attr: &str) -> Option<String> {
    let rest = attr.strip_prefix("data-")?;
    if rest.bytes().any(|b| b.is_ascii_uppercase()) {
        return None;
    }
    let mut out = String::new();
    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '-' && chars.peek().is_some_and(|n| n.is_ascii_lowercase()) {
            out.push(chars.next().unwrap().to_ascii_uppercase());
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// The value of the `data-*` attribute whose DOMStringMap name is `name`.
fn dataset_lookup(n: NodeRef, name: &str) -> Option<String> {
    with_doc(n.doc, |doc| {
        doc.element(n.node)?.attrs.iter().find(|(k, _)| data_to_camel(k).as_deref() == Some(name)).map(|(_, v)| v.clone())
    })
}

fn proxy_target_node(realm: &mut Realm, args: &[Value]) -> JsResult<NodeRef> {
    match node_of(realm, &arg(args, 0)) {
        Some(n) => Ok(n),
        None => Err(realm.throw_type_error("Illegal invocation")),
    }
}

fn dataset_get(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let n = proxy_target_node(realm, args)?;
    let key = arg(args, 1);
    if let V::String(k) = key.v() {
        if let Some(v) = dataset_lookup(n, &k.to_rust_string()) {
            return Ok(Value::str(&v));
        }
    }
    // Not a named property: the ordinary lookup (Object.prototype members
    // such as hasOwnProperty, symbols).
    let key = realm.to_property_key(key)?;
    realm.get(arg(args, 0), key)
}

fn dataset_set(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let n = proxy_target_node(realm, args)?;
    let k = realm.to_rust_string(arg(args, 1))?;
    if k.as_bytes().windows(2).any(|w| w[0] == b'-' && w[1].is_ascii_lowercase()) {
        return Err(crate::misc::dom_exception(realm, "SyntaxError", &format!("Failed to set a named property on 'DOMStringMap': '{k}' is not a valid property name.")));
    }
    let v = realm.to_rust_string(arg(args, 2))?;
    let attr = camel_to_data(&k);
    if !crate::document::valid_attribute_local_name(&attr) {
        return Err(crate::misc::dom_exception(realm, "InvalidCharacterError", &format!("Failed to set a named property on 'DOMStringMap': '{attr}' is not a valid attribute name.")));
    }
    set_attribute(realm, n, &attr, Some(&v));
    Ok(Value::Bool(true))
}

fn dataset_has(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let n = proxy_target_node(realm, args)?;
    let k = realm.to_rust_string(arg(args, 1))?;
    Ok(Value::Bool(dataset_lookup(n, &k).is_some()))
}

fn dataset_delete(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let n = proxy_target_node(realm, args)?;
    let k = realm.to_rust_string(arg(args, 1))?;
    set_attribute(realm, n, &camel_to_data(&k), None);
    Ok(Value::Bool(true))
}

fn dataset_own_keys(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let n = proxy_target_node(realm, args)?;
    let keys: Vec<Value> = with_doc(n.doc, |doc| {
        doc.element(n.node)
            .map(|e| e.attrs.iter().filter_map(|(k, _)| data_to_camel(k)).map(|k| Value::str(&k)).collect())
            .unwrap_or_default()
    });
    Ok(array(realm, keys))
}

fn dataset_gopd(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let n = proxy_target_node(realm, args)?;
    let k = realm.to_rust_string(arg(args, 1))?;
    match dataset_lookup(n, &k) {
        Some(v) => Ok(object_from(realm, vec![("value", Value::str(&v)), ("writable", Value::Bool(true)), ("enumerable", Value::Bool(true)), ("configurable", Value::Bool(true))])),
        None => Ok(Value::Undefined),
    }
}

/// Create a Proxy whose traps are native functions; `target` carries host data.
pub(crate) fn make_proxy(realm: &mut Realm, target: ObjectId, traps: &[(&str, rabbithole_js::NativeFn)]) -> Value {
    let handler = realm.new_object();
    for (name, f) in traps {
        realm.define_method(handler, name, 3, *f);
    }
    let id = realm.alloc(rabbithole_js::heap::Object::new(None, rabbithole_js::ObjectKind::Proxy { target, handler, revoked: false }));
    Value::Object(id)
}

fn e_dataset(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "dataset")?;
    let V::Object(w) = this.v() else { unreachable!() };
    let cached = get_hidden(realm, w, "__dataset");
    if cached.is_object() {
        return Ok(cached);
    }
    let target = realm.new_object();
    realm.set_host_data(target, Box::new(n));
    realm.set_class_name(target, "DOMStringMap");
    let p = make_proxy(realm, target, &[("get", dataset_get), ("set", dataset_set), ("has", dataset_has), ("deleteProperty", dataset_delete), ("ownKeys", dataset_own_keys), ("getOwnPropertyDescriptor", dataset_gopd)]);
    set_hidden(realm, w, "__dataset", p.clone());
    Ok(p)
}

// innerHTML & co --------------------------------------------------------------

/// The fragment serializing algorithm: HTML serialization in HTML
/// documents, well-formed XML serialization otherwise (InvalidStateError
/// when the subtree cannot be serialized as XML).
pub(crate) fn fragment_serialize(realm: &mut Realm, n: NodeRef, include_self: bool) -> JsResult<Value> {
    let r = with_doc(n.doc, |doc| {
        if doc.is_html {
            Ok(doc.serialize(n.node, include_self))
        } else if include_self {
            doc.serialize_xml(n.node, true)
        } else {
            doc.serialize_xml_children(n.node, true)
        }
    });
    match r {
        Ok(s) => Ok(Value::str(&s)),
        Err(msg) => Err(crate::misc::dom_exception(realm, "InvalidStateError", &format!("Failed to serialize the node: {msg}."))),
    }
}

/// `getHTML({serializableShadowRoots, shadowRoots})` on an element or a
/// shadow root: the HTML fragment serialization of its children with the
/// chosen shadow roots as declarative `<template shadowrootmode>`.
pub(crate) fn get_html(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(n) = node_of(realm, &this) else { return Err(realm.throw_type_error("Illegal invocation")) };
    let opts = arg(args, 0);
    let mut serializable = false;
    let mut listed: Vec<NodeRef> = Vec::new();
    if opts.is_object() {
        let v = realm.get_str(opts.clone(), "serializableShadowRoots")?;
        serializable = realm.to_boolean(&v);
        let roots = realm.get_str(opts, "shadowRoots")?;
        if roots.is_object() {
            let len = realm.get_str(roots.clone(), "length")?;
            let len = realm.to_number(len)? as usize;
            for i in 0..len {
                let r = realm.get_str(roots.clone(), &i.to_string())?;
                match node_of(realm, &r) {
                    Some(rn) if crate::shadow::is_shadow_root(rn) => listed.push(rn),
                    _ => return Err(realm.throw_type_error("Failed to execute 'getHTML': shadowRoots must contain ShadowRoot objects.")),
                }
            }
        }
    }
    if !with_doc(n.doc, |doc| doc.is_html) {
        return fragment_serialize(realm, n, false);
    }
    let s = with_doc(n.doc, |doc| {
        if !serializable && listed.is_empty() {
            return doc.serialize_with_shadows(n.node, false, None);
        }
        let choose = |host: NodeId| {
            let root = doc.shadow_root(host)?;
            let info = doc.shadow_info.get(&root)?;
            if !(serializable && info.serializable) && !listed.contains(&NodeRef { doc: n.doc, node: root }) {
                return None;
            }
            let mut open = format!("<template shadowrootmode=\"{}\"", if info.open { "open" } else { "closed" });
            if info.delegates_focus {
                open.push_str(" shadowrootdelegatesfocus=\"\"");
            }
            if info.serializable {
                open.push_str(" shadowrootserializable=\"\"");
            }
            if info.clonable {
                open.push_str(" shadowrootclonable=\"\"");
            }
            open.push('>');
            Some((root, open))
        };
        doc.serialize_with_shadows(n.node, false, Some(&choose))
    });
    Ok(Value::str(&s))
}

/// A `[LegacyNullToEmptyString] DOMString` argument.
pub(crate) fn arg_null_to_empty(realm: &mut Realm, args: &[Value], i: usize) -> JsResult<String> {
    match args.get(i).map(Value::v) {
        Some(V::Null) => Ok(String::new()),
        Some(v) => realm.to_rust_string(v.to_value()),
        None => Ok("undefined".to_string()),
    }
}

fn e_inner_html_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "innerHTML")?;
    fragment_serialize(realm, n, false)
}

fn e_inner_html_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "innerHTML")?;
    let html = arg_null_to_empty(realm, args, 0)?;
    set_inner_html(realm, n, &html)?;
    Ok(Value::Undefined)
}

fn e_outer_html_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "outerHTML")?;
    fragment_serialize(realm, n, true)
}

fn e_outer_html_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "outerHTML")?;
    let html = arg_null_to_empty(realm, args, 0)?;
    let parent = with_doc(n.doc, |doc| doc.parent(n.node));
    let Some(p) = parent else { return Ok(Value::Undefined) };
    if p == rabbithole_dom::Document::ROOT {
        return Err(crate::misc::dom_exception(realm, "NoModificationAllowedError", "Failed to set the 'outerHTML' property on 'Element': This element's parent is of type '#document', which is not an element node."));
    }
    let pref = NodeRef { doc: n.doc, node: p };
    // A fragment parent parses in the context of a <body>.
    let is_fragment = with_doc(n.doc, |doc| doc.element(p).map(|e| e.tag == "#fragment").unwrap_or(false));
    let nodes = if is_fragment {
        let body = with_doc(n.doc, |doc| doc.create_element("body", vec![]));
        node::parse_html_into(NodeRef { doc: n.doc, node: body }, &html)
    } else {
        node::parse_html_into(pref, &html)
    };
    let next = with_doc(n.doc, |doc| doc.next_sibling(n.node));
    remove_node(realm, n);
    for nn in nodes {
        insert_node(realm, pref, nn, next)?;
    }
    Ok(Value::Undefined)
}

fn e_inner_text_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "innerText")?;
    Ok(Value::str(&crate::inner_text::inner_text(n)))
}

/// `a.text`: the descendant text content.
fn a_text_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "text")?;
    Ok(Value::str(&with_doc(n.doc, |doc| doc.text_content(n.node))))
}

/// The rendered text fragment of `text`: text runs and a `<br>` for each
/// line break (CR LF, CR or LF), as new detached nodes.
fn rendered_text_fragment(doc: u32, text: &str) -> Vec<NodeRef> {
    with_doc(doc, |d| {
        let mut out = Vec::new();
        let mut run = String::new();
        let mut chars = text.chars().peekable();
        let flush = |run: &mut String, d: &mut Document, out: &mut Vec<NodeRef>| {
            if !run.is_empty() {
                let t = d.create_text(run);
                out.push(NodeRef { doc, node: t });
                run.clear();
            }
        };
        while let Some(c) = chars.next() {
            if c == '\r' || c == '\n' {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                flush(&mut run, d, &mut out);
                let br = d.create_element("br", vec![]);
                out.push(NodeRef { doc, node: br });
            } else {
                run.push(c);
            }
        }
        flush(&mut run, d, &mut out);
        out
    })
}

fn e_inner_text_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "innerText")?;
    let text = arg_null_to_empty(realm, args, 0)?;
    let old = children_of(n);
    let added = rendered_text_fragment(n.doc, &text);
    with_doc(n.doc, |doc| {
        doc.clear_children(n.node);
        for a in &added {
            doc.append_child(n.node, a.node);
        }
    });
    node::record_child_list(realm, n, added, old);
    if n.doc == 0 {
        mark_dirty();
    }
    Ok(Value::Undefined)
}

/// Merge `text` with its next sibling when both are Text nodes.
fn merge_with_next_text(realm: &mut Realm, n: NodeRef) {
    let next = with_doc(n.doc, |doc| {
        let next = doc.next_sibling(n.node)?;
        match (&doc.node(n.node).data, &doc.node(next).data) {
            (NodeData::Text(a), NodeData::Text(b)) if !doc.is_cdata(n.node) && !doc.is_cdata(next) => Some((next, format!("{a}{b}"))),
            _ => None,
        }
    });
    if let Some((next, data)) = next {
        node::set_character_data(realm, n, &data);
        remove_node(realm, NodeRef { doc: n.doc, node: next });
    }
}

fn e_outer_text_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "outerText")?;
    let text = arg_null_to_empty(realm, args, 0)?;
    let parent = with_doc(n.doc, |doc| doc.parent(n.node));
    let Some(p) = parent else {
        return Err(crate::misc::dom_exception(realm, "NoModificationAllowedError", "Failed to set the 'outerText' property on 'HTMLElement': The element has no parent."));
    };
    let (next, previous) = with_doc(n.doc, |doc| (doc.next_sibling(n.node), doc.prev_sibling(n.node)));
    let mut nodes = rendered_text_fragment(n.doc, &text);
    if nodes.is_empty() {
        nodes.push(with_doc(n.doc, |d| NodeRef { doc: n.doc, node: d.create_text("") }));
    }
    let pref = NodeRef { doc: n.doc, node: p };
    for nn in nodes {
        insert_node(realm, pref, nn, Some(n.node))?;
    }
    remove_node(realm, n);
    if let Some(next) = next {
        if let Some(prev_of_next) = with_doc(n.doc, |doc| doc.prev_sibling(next)) {
            merge_with_next_text(realm, NodeRef { doc: n.doc, node: prev_of_next });
        }
    }
    if let Some(previous) = previous {
        merge_with_next_text(realm, NodeRef { doc: n.doc, node: previous });
    }
    Ok(Value::Undefined)
}

fn e_insert_adjacent_html(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "insertAdjacentHTML")?;
    let pos = arg_str(realm, args, 0)?.to_ascii_lowercase();
    let html = arg_str(realm, args, 1)?;
    // The context: the parent for beforebegin / afterend (which needs an
    // element-like parent), else the element; <html> parses as <body>.
    let context = match pos.as_str() {
        "beforebegin" | "afterend" => {
            let parent = with_doc(n.doc, |doc| doc.parent(n.node));
            match parent {
                Some(p) if p != Document::ROOT => NodeRef { doc: n.doc, node: p },
                _ => return Err(crate::misc::dom_exception(realm, "NoModificationAllowedError", "Failed to execute 'insertAdjacentHTML' on 'Element': The element has no parent.")),
            }
        }
        "afterbegin" | "beforeend" => n,
        _ => return Err(crate::misc::dom_exception(realm, "SyntaxError", "The value provided is not one of 'beforeBegin', 'afterBegin', 'beforeEnd', or 'afterEnd'.")),
    };
    let use_body = with_doc(n.doc, |doc| match doc.element(context.node) {
        Some(e) => e.tag.starts_with('#') || (doc.is_html && e.tag == "html" && e.ns == rabbithole_dom::Namespace::Html),
        None => true,
    });
    let context = if use_body { NodeRef { doc: n.doc, node: with_doc(n.doc, |doc| doc.create_element("body", vec![])) } } else { context };
    let nodes = match node::try_parse_html_into(context, &html) {
        Ok(n) => n,
        Err(msg) => return Err(crate::misc::dom_exception(realm, "SyntaxError", &format!("Failed to execute 'insertAdjacentHTML' on 'Element': The provided markup is invalid XML ({msg})."))),
    };
    insert_adjacent(realm, n, &pos, nodes)
}

fn insert_adjacent(realm: &mut Realm, n: NodeRef, pos: &str, nodes: Vec<NodeRef>) -> JsResult<Value> {
    let (parent, next, first) = with_doc(n.doc, |doc| (doc.parent(n.node), doc.next_sibling(n.node), doc.first_child(n.node)));
    match pos {
        "beforebegin" => {
            if let Some(p) = parent {
                for nn in nodes {
                    insert_node(realm, NodeRef { doc: n.doc, node: p }, nn, Some(n.node))?;
                }
            }
        }
        "afterbegin" => {
            for nn in nodes {
                insert_node(realm, n, nn, first)?;
            }
        }
        "beforeend" => {
            for nn in nodes {
                insert_node(realm, n, nn, None)?;
            }
        }
        "afterend" => {
            if let Some(p) = parent {
                for nn in nodes {
                    insert_node(realm, NodeRef { doc: n.doc, node: p }, nn, next)?;
                }
            }
        }
        _ => return Err(crate::misc::dom_exception(realm, "SyntaxError", "The value provided is not one of 'beforeBegin', 'afterBegin', 'beforeEnd', or 'afterEnd'.")),
    }
    Ok(Value::Undefined)
}

fn e_insert_adjacent_element(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "insertAdjacentElement")?;
    let pos = arg_str(realm, args, 0)?.to_ascii_lowercase();
    let Some(el) = node_of(realm, &arg(args, 1)) else {
        return Err(realm.throw_type_error("parameter 2 is not of type 'Element'"));
    };
    insert_adjacent(realm, n, &pos, vec![el])?;
    Ok(arg(args, 1))
}

fn e_insert_adjacent_text(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "insertAdjacentText")?;
    let pos = arg_str(realm, args, 0)?.to_ascii_lowercase();
    let text = arg_str(realm, args, 1)?;
    let t = with_doc(n.doc, |doc| NodeRef { doc: n.doc, node: doc.create_text(&text) });
    insert_adjacent(realm, n, &pos, vec![t])
}

fn e_matches(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "matches")?;
    let s = arg_str(realm, args, 0)?;
    Ok(Value::Bool(node::element_matches(realm, n, &s)?))
}

fn e_closest(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "closest")?;
    let s = arg_str(realm, args, 0)?;
    let list = node::parse_selectors(realm, &s)?;
    let found = node::with_selector_scope(n, |doc, state| {
        let matcher = rabbithole_css::selector::SelectorMatcher::new(doc, state);
        let mut cur = Some(n.node);
        while let Some(c) = cur {
            match doc.element(c) {
                // Inclusive ancestor *elements* only: a fragment or shadow
                // root ends the walk.
                Some(e) if e.tag == "#fragment" || e.tag == "#shadow-root" => break,
                Some(_) if list.iter().any(|sel| matcher.matches(sel, c)) => return Some(c),
                _ => {}
            }
            cur = doc.parent(c);
        }
        None
    });
    Ok(wrap_opt(realm, n.doc, found))
}

// geometry ---------------------------------------------------------------------

pub(crate) fn node_rect(n: NodeRef) -> rabbithole_gfx::Rect {
    if n.doc & crate::host::FRAME_DOC != 0 {
        let frame = (n.doc & !crate::host::FRAME_DOC) as u64;
        return with_env(|env| env.frame_node_rect(frame, n.node)).unwrap_or_default();
    }
    if n.doc != 0 {
        return rabbithole_gfx::Rect::default();
    }
    with_env(|env| {
        env.ensure_layout();
        env.node_rect(n.node).unwrap_or_default()
    })
}

pub(crate) fn dom_rect(realm: &mut Realm, x: f32, y: f32, w: f32, h: f32) -> Value {
    let factory = state().borrow().factories.get("rect").cloned();
    if let Some(f) = factory {
        if let Ok(v) = realm.call(f, Value::Undefined, &[Value::Number(x as f64), Value::Number(y as f64), Value::Number(w as f64), Value::Number(h as f64)]) {
            return v;
        }
    }
    let o = object_from(realm, vec![
        ("x", Value::Number(x as f64)),
        ("y", Value::Number(y as f64)),
        ("width", Value::Number(w as f64)),
        ("height", Value::Number(h as f64)),
        ("top", Value::Number(y.min(y + h) as f64)),
        ("left", Value::Number(x.min(x + w) as f64)),
        ("right", Value::Number(x.max(x + w) as f64)),
        ("bottom", Value::Number(y.max(y + h) as f64)),
    ]);
    if let V::Object(id) = o.v() {
        let p = proto("DOMRect");
        realm.obj_mut(id).set_proto(Some(p));
    }
    o
}

fn dom_rect_to_json(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = realm.new_object();
    for k in ["x", "y", "width", "height", "top", "left", "right", "bottom"] {
        let v = realm.get_str(this.clone(), k)?;
        realm.define_property(o, k, v, ALL);
    }
    Ok(Value::Object(o))
}

fn dom_rect_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let x = realm.to_number(arg(args, 0)).unwrap_or(0.0);
    let y = realm.to_number(arg(args, 1)).unwrap_or(0.0);
    let w = realm.to_number(arg(args, 2)).unwrap_or(0.0);
    let h = realm.to_number(arg(args, 3)).unwrap_or(0.0);
    Ok(dom_rect(realm, x as f32, y as f32, w as f32, h as f32))
}

fn e_get_bounding_client_rect(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "getBoundingClientRect")?;
    let r = if n.doc == 0 {
        with_env(|env| {
            env.ensure_layout();
            env.node_client_rect(n.node)
        })
    } else if n.doc & crate::host::FRAME_DOC != 0 {
        let frame = (n.doc & !crate::host::FRAME_DOC) as u64;
        with_env(|env| env.frame_node_rect(frame, n.node))
    } else {
        None
    };
    // A boxless element returns the origin, including on a scrolled page.
    // Applying the viewport offset to a default document rect would instead
    // expose (-scrollX, -scrollY) for display:none and detached elements.
    let Some(r) = r else { return Ok(dom_rect(realm, 0.0, 0.0, 0.0, 0.0)); };
    let (sx, sy) = with_env(|env| env.scroll());
    Ok(dom_rect(realm, r.x - sx, r.y - sy, r.w, r.h))
}

fn e_get_client_rects(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "getClientRects")?;
    // No box (display:none, detached): an empty list. An inline element has
    // one rect per line, plus the blocks split out of it.
    let boxes = if n.doc == 0 {
        with_env(|env| {
            env.ensure_layout();
            env.node_client_rects(n.node)
        })
    } else {
        Vec::new()
    };
    let (sx, sy) = with_env(|env| env.scroll());
    let rects = boxes.into_iter().map(|r| dom_rect(realm, r.x - sx, r.y - sy, r.w, r.h)).collect();
    rect_list(realm, rects)
}

/// A `DOMRectList` of the given rects.
pub(crate) fn rect_list(realm: &mut Realm, rects: Vec<Value>) -> JsResult<Value> {
    let arr = realm.new_array_value(rects);
    let factory = state().borrow().factories.get("rectList").cloned();
    match factory {
        Some(f) => realm.call(f, Value::Undefined, &[arr]),
        None => Ok(arr),
    }
}

fn e_offset_width(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "offsetWidth")?;
    Ok(Value::Number(node_rect(n).w.round() as f64))
}

fn e_offset_height(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "offsetHeight")?;
    Ok(Value::Number(node_rect(n).h.round() as f64))
}

/// `offsetParent` (CSSOM View §7): the nearest ancestor that is a
/// containing block for positioned descendants (positioned, or with a
/// transform), a table cell / table for a static element, or the body.
/// None for the root, the body, fixed elements and elements without a box.
fn offset_parent_of(n: NodeRef) -> Option<NodeId> {
    if n.doc != 0 {
        // Script-created / frame documents: no styles here, the body.
        return with_doc(n.doc, |doc| {
            let mut cur = doc.parent(n.node);
            while let Some(p) = cur {
                if doc.is_element(p, "body") {
                    return Some(p);
                }
                cur = doc.parent(p);
            }
            None
        });
    }
    use rabbithole_style::Position;
    let style = |id: NodeId| with_env(|env| env.computed_style(id));
    let me = style(n.node)?;
    if me.display.is_none() || me.position == Position::Fixed {
        return None;
    }
    let (is_root_or_body, mut cur) = with_doc(0, |doc| (doc.is_element(n.node, "html") || doc.is_element(n.node, "body"), doc.flat_parent(n.node)));
    if is_root_or_body {
        return None;
    }
    let positioned = me.position != Position::Static;
    while let Some(p) = cur {
        let (tag, next) = with_doc(0, |doc| (doc.tag(p).map(|t| t.to_ascii_lowercase()), doc.flat_parent(p)));
        let Some(tag) = tag else {
            cur = next;
            continue;
        };
        if tag == "body" {
            return Some(p);
        }
        if let Some(s) = style(p) {
            if s.position != Position::Static || s.has_transform() {
                return Some(p);
            }
        }
        if !positioned && matches!(tag.as_str(), "td" | "th" | "table") {
            return Some(p);
        }
        cur = next;
    }
    None
}

/// `offsetLeft` / `offsetTop` origin: the offsetParent's padding edge, or
/// the document origin for no offsetParent and for a static body (as in
/// browsers: an element in the body is offset from the page, body margin
/// included).
fn offset_origin(n: NodeRef) -> (f32, f32) {
    let Some(p) = offset_parent_of(n) else { return (0.0, 0.0) };
    let pr = NodeRef { doc: n.doc, node: p };
    if n.doc != 0 {
        let r = node_rect(pr);
        return (r.x, r.y);
    }
    let style = with_env(|env| env.computed_style(p));
    let is_body = with_doc(0, |doc| doc.is_element(p, "body"));
    match style {
        Some(s) if is_body && s.position == rabbithole_style::Position::Static => (0.0, 0.0),
        Some(s) => {
            let r = node_rect(pr);
            (r.x + s.border_width.left, r.y + s.border_width.top)
        }
        None => (0.0, 0.0),
    }
}

fn is_body(n: NodeRef) -> bool {
    with_doc(n.doc, |doc| doc.is_element(n.node, "body"))
}

fn e_offset_top(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "offsetTop")?;
    if is_body(n) {
        return Ok(Value::Number(0.0));
    }
    let r = node_rect(n);
    let (_, oy) = offset_origin(n);
    Ok(Value::Number((r.y - oy).round() as f64))
}

fn e_offset_left(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "offsetLeft")?;
    if is_body(n) {
        return Ok(Value::Number(0.0));
    }
    let r = node_rect(n);
    let (ox, _) = offset_origin(n);
    Ok(Value::Number((r.x - ox).round() as f64))
}

fn e_offset_parent(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "offsetParent")?;
    let p = offset_parent_of(n);
    Ok(wrap_opt(realm, n.doc, p))
}

fn client_size(n: NodeRef) -> (f32, f32) {
    let r = node_rect(n);
    let (bw, bh) = if n.doc == 0 {
        with_env(|env| {
            env.computed_style(n.node)
                .map(|s| (s.border_width.left + s.border_width.right, s.border_width.top + s.border_width.bottom))
                .unwrap_or((0.0, 0.0))
        })
    } else {
        (0.0, 0.0)
    };
    ((r.w - bw).max(0.0), (r.h - bh).max(0.0))
}

fn is_root_scroller(n: NodeRef) -> bool {
    n.doc == 0 && with_doc(0, |doc| doc.is_element(n.node, "html") || doc.is_element(n.node, "body"))
}

/// The element whose `scrollTop`/`scrollLeft` are the document's scroll
/// position (CSSOM View `scrollingElement`): `<html>` in standards mode,
/// `<body>` in quirks mode.
fn is_document_scroller(n: NodeRef) -> bool {
    n.doc == 0
        && with_doc(0, |doc| {
            if doc.quirks_mode == rabbithole_dom::QuirksMode::Quirks {
                doc.is_element(n.node, "body")
            } else {
                doc.is_element(n.node, "html")
            }
        })
}

fn e_client_width(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "clientWidth")?;
    if is_root_scroller(n) {
        return Ok(Value::Number(with_env(|env| env.viewport().0).round() as f64));
    }
    Ok(Value::Number(client_size(n).0.round() as f64))
}

fn e_client_height(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "clientHeight")?;
    if is_root_scroller(n) {
        return Ok(Value::Number(with_env(|env| env.viewport().1).round() as f64));
    }
    Ok(Value::Number(client_size(n).1.round() as f64))
}

fn e_zero(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Number(0.0))
}

/// Scroll size of an element: the document's for `<html>` (infinite
/// scroll checks `scrollTop + clientHeight >= scrollHeight`), the
/// scrollable overflow for scroll containers, else the padding box.
fn scroll_size(n: NodeRef) -> (f32, f32) {
    if n.doc == 0 {
        let is_html = with_doc(0, |doc| doc.is_element(n.node, "html"));
        let size = with_env(|env| if is_html { env.document_scroll_size() } else { env.element_scroll(n.node).map(|(_, s)| s) });
        if let Some(s) = size {
            return s;
        }
    }
    let r = node_rect(n);
    (r.w, r.h)
}

/// Scroll offset of an element's scroll container, from layout (what the
/// user scrolled with the wheel too).
fn element_scroll_offset(n: NodeRef) -> Option<(f32, f32)> {
    if n.doc != 0 {
        return None;
    }
    with_env(|env| env.element_scroll(n.node)).map(|(o, _)| o)
}

/// Scroll an element; a moved container re-runs observers (its `scroll`
/// event follows from the embedder's next frame in browsers; observers
/// must see the new position).
fn set_element_scroll(realm: &mut Realm, this: &Value, n: NodeRef, x: Option<f64>, y: Option<f64>) {
    let cur = element_scroll_offset(n);
    let moved = match cur {
        Some((cx, cy)) => with_env(|env| env.set_element_scroll(n.node, x.map(|v| v as f32).unwrap_or(cx), y.map(|v| v as f32).unwrap_or(cy))),
        None => false,
    };
    if moved {
        state().borrow_mut().render.dirty = true;
    } else if cur.is_none() {
        // No box (or no layout support): remember the values.
        if let V::Object(w) = this.v() {
            if let Some(x) = x {
                set_hidden(realm, w, "__scrollLeft", Value::Number(x.max(0.0)));
            }
            if let Some(y) = y {
                set_hidden(realm, w, "__scrollTop", Value::Number(y.max(0.0)));
            }
        }
    }
}

fn e_scroll_width(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "scrollWidth")?;
    Ok(Value::Number(scroll_size(n).0.round() as f64))
}

fn e_scroll_height(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "scrollHeight")?;
    Ok(Value::Number(scroll_size(n).1.round() as f64))
}

fn e_scroll_top_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "scrollTop")?;
    if is_document_scroller(n) {
        return Ok(Value::Number(with_env(|env| env.scroll().1) as f64));
    }
    if let Some((_, y)) = element_scroll_offset(n) {
        return Ok(Value::Number(y as f64));
    }
    let V::Object(w) = this.v() else { unreachable!() };
    let v = get_hidden(realm, w, "__scrollTop");
    Ok(if v.is_undefined() { Value::Number(0.0) } else { v })
}

fn e_scroll_top_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "scrollTop")?;
    let v = realm.to_number(arg(args, 0))?;
    if is_document_scroller(n) {
        with_env(|env| {
            let (x, _) = env.scroll();
            env.set_scroll(x, v.max(0.0) as f32);
        });
        return Ok(Value::Undefined);
    }
    set_element_scroll(realm, &this, n, None, Some(v));
    Ok(Value::Undefined)
}

fn e_scroll_left_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "scrollLeft")?;
    if is_document_scroller(n) {
        return Ok(Value::Number(with_env(|env| env.scroll().0) as f64));
    }
    if let Some((x, _)) = element_scroll_offset(n) {
        return Ok(Value::Number(x as f64));
    }
    let V::Object(w) = this.v() else { unreachable!() };
    let v = get_hidden(realm, w, "__scrollLeft");
    Ok(if v.is_undefined() { Value::Number(0.0) } else { v })
}

fn e_scroll_left_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "scrollLeft")?;
    let v = realm.to_number(arg(args, 0))?;
    if is_document_scroller(n) {
        with_env(|env| {
            let (_, y) = env.scroll();
            env.set_scroll(v.max(0.0) as f32, y);
        });
        return Ok(Value::Undefined);
    }
    set_element_scroll(realm, &this, n, Some(v), None);
    Ok(Value::Undefined)
}

fn e_scroll_into_view(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "scrollIntoView")?;
    // ScrollIntoViewOptions: block "start" / inline "nearest" by default;
    // `false` is block "end".
    let (mut block, mut inline) = ("start".to_string(), "nearest".to_string());
    match args.first().map(Value::v) {
        Some(V::Bool(false)) => block = "end".into(),
        Some(v) if v.is_object() => {
            let o = v.to_value();
            for (key, slot) in [("block", &mut block), ("inline", &mut inline)] {
                if let Some(s) = dict_str(realm, &o, key)? {
                    if !matches!(s.as_str(), "start" | "center" | "end" | "nearest") {
                        return Err(realm.throw_type_error(&format!("Failed to execute 'scrollIntoView' on 'Element': The provided value '{s}' is not a valid enum value of type ScrollLogicalPosition.")));
                    }
                    *slot = s;
                }
            }
        }
        _ => {}
    }
    if n.doc != 0 || !with_doc(0, |doc| doc.is_connected(n.node)) {
        return Ok(Value::Undefined);
    }
    // The new offset along one axis of a scrollport [lo, hi) for a target
    // [t0, t1) (all in the same coordinates), from the current offset.
    fn aligned(cur: f32, lo: f32, hi: f32, t0: f32, t1: f32, how: &str, flip: bool) -> f32 {
        let how = match (how, flip) {
            ("start", true) => "end",
            ("end", true) => "start",
            (h, _) => h,
        };
        match how {
            "start" => cur + (t0 - lo),
            "end" => cur + (t1 - hi),
            "center" => cur + ((t0 + t1) / 2.0 - (lo + hi) / 2.0),
            _ => {
                // nearest: no change when the target is inside; otherwise
                // align the edge it overflows on (the start for a target
                // larger than the scrollport).
                if (t0 >= lo && t1 <= hi) || (t0 < lo && t1 > hi) {
                    cur
                } else if (t0 < lo) == (t1 - t0 <= hi - lo) {
                    cur + (t0 - lo)
                } else {
                    cur + (t1 - hi)
                }
            }
        }
    }
    // Scrolling boxes from the innermost out, then the viewport.
    let mut cur = with_doc(0, |doc| doc.flat_parent(n.node));
    while let Some(p) = cur {
        if p == rabbithole_dom::Document::ROOT {
            break;
        }
        let style = with_env(|env| env.computed_style(p));
        let is_root = with_doc(0, |doc| doc.is_element(p, "html") || doc.is_element(p, "body"));
        if let Some(s) = style.filter(|s| !is_root && (matches!(s.overflow_x, rabbithole_style::Overflow::Scroll | rabbithole_style::Overflow::Auto | rabbithole_style::Overflow::Hidden) || matches!(s.overflow_y, rabbithole_style::Overflow::Scroll | rabbithole_style::Overflow::Auto | rabbithole_style::Overflow::Hidden))) {
            // A sticky box between the target and this scroller is stuck
            // in view (its layout rect does not show that): leave it.
            let stuck = {
                let mut c = Some(n.node);
                let mut found = false;
                while let Some(x) = c {
                    if x == p {
                        break;
                    }
                    if with_env(|env| env.computed_style(x)).map(|s| s.position == rabbithole_style::Position::Sticky).unwrap_or(false) {
                        found = true;
                        break;
                    }
                    c = with_doc(0, |doc| doc.flat_parent(x));
                }
                found
            };
            if stuck {
                cur = with_doc(0, |doc| doc.flat_parent(p));
                continue;
            }
            if let Some(((sx, sy), _)) = with_env(|env| env.element_scroll(p)) {
                let t = node_rect(n);
                let pr = node_rect(NodeRef::main(p));
                let (bl, bt) = (s.border_width.left, s.border_width.top);
                let (cw, ch) = client_size(NodeRef::main(p));
                let (lo_x, lo_y) = (pr.x + bl, pr.y + bt);
                let vertical = !matches!(s.writing_mode, rabbithole_style::WritingMode::HorizontalTb);
                let rtl = s.direction == rabbithole_style::Direction::Rtl;
                let (bx, by) = if vertical { (&block, &inline) } else { (&inline, &block) };
                let flip_x = if vertical { s.writing_mode == rabbithole_style::WritingMode::VerticalRl } else { rtl };
                let flip_y = vertical && (rtl != (s.writing_mode == rabbithole_style::WritingMode::SidewaysLr));
                let nx = aligned(sx, lo_x, lo_x + cw, t.x, t.x + t.w, bx, flip_x);
                let ny = aligned(sy, lo_y, lo_y + ch, t.y, t.y + t.h, by, flip_y);
                with_env(|env| env.set_element_scroll(p, nx.max(0.0), ny.max(0.0)));
            }
        }
        cur = with_doc(0, |doc| doc.flat_parent(p));
    }
    let t = node_rect(n);
    let mut c = Some(n.node);
    while let Some(x) = c {
        if with_env(|env| env.computed_style(x)).map(|s| s.position == rabbithole_style::Position::Sticky || s.position == rabbithole_style::Position::Fixed).unwrap_or(false) {
            // Stuck / fixed in the viewport already.
            return Ok(Value::Undefined);
        }
        c = with_doc(0, |doc| doc.flat_parent(x));
    }
    with_env(|env| {
        let (vw, vh) = env.viewport();
        let (sx, sy) = env.scroll();
        let nx = aligned(sx, sx, sx + vw, t.x, t.x + t.w, &inline, false);
        let ny = aligned(sy, sy, sy + vh, t.y, t.y + t.h, &block, false);
        env.set_scroll(nx.max(0.0), ny.max(0.0));
    });
    Ok(Value::Undefined)
}

fn scroll_args(realm: &mut Realm, args: &[Value]) -> JsResult<(Option<f64>, Option<f64>)> {
    match args.first() {
        Some(v) if v.is_object() => {
            let x = realm.get_str(v.clone(), "left")?;
            let y = realm.get_str(v.clone(), "top")?;
            let x = if x.is_undefined() { None } else { Some(realm.to_number(x)?) };
            let y = if y.is_undefined() { None } else { Some(realm.to_number(y)?) };
            Ok((x, y))
        }
        Some(_) => {
            let x = realm.to_number(arg(args, 0))?;
            let y = realm.to_number(arg(args, 1))?;
            Ok((Some(x), Some(y)))
        }
        None => Ok((None, None)),
    }
}

fn e_scroll_to(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "scrollTo")?;
    let (x, y) = scroll_args(realm, args)?;
    if is_root_scroller(n) {
        with_env(|env| {
            let (cx, cy) = env.scroll();
            env.set_scroll(x.map(|v| v as f32).unwrap_or(cx), y.map(|v| v as f32).unwrap_or(cy));
        });
    } else {
        set_element_scroll(realm, &this, n, x, y);
    }
    Ok(Value::Undefined)
}

fn e_scroll_by(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "scrollBy")?;
    let (x, y) = scroll_args(realm, args)?;
    if is_root_scroller(n) {
        with_env(|env| {
            let (cx, cy) = env.scroll();
            env.set_scroll(cx + x.unwrap_or(0.0) as f32, cy + y.unwrap_or(0.0) as f32);
        });
    } else if let Some((cx, cy)) = element_scroll_offset(n) {
        set_element_scroll(realm, &this, n, Some(cx as f64 + x.unwrap_or(0.0)), Some(cy as f64 + y.unwrap_or(0.0)));
    }
    Ok(Value::Undefined)
}

pub(crate) fn e_focus(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "focus")?;
    if forward_to_frame(n, "focus") {
        return Ok(Value::Undefined);
    }
    if n.doc != 0 {
        return Ok(Value::Undefined);
    }
    let prev = state().borrow().focused;
    if prev == Some(n.node) {
        return Ok(Value::Undefined);
    }
    state().borrow_mut().focused = Some(n.node);
    with_env(|env| env.focus_node(Some(n.node)));
    if let Some(p) = prev {
        crate::events::dispatch(realm, TargetKey::Node(NodeRef::main(p)), "blur", crate::EventInit::simple(false, false), true);
        crate::events::dispatch(realm, TargetKey::Node(NodeRef::main(p)), "focusout", crate::EventInit::simple(true, false), true);
    }
    crate::events::dispatch(realm, TargetKey::Node(n), "focus", crate::EventInit::simple(false, false), true);
    crate::events::dispatch(realm, TargetKey::Node(n), "focusin", crate::EventInit::simple(true, false), true);
    Ok(Value::Undefined)
}

fn e_blur(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "blur")?;
    if forward_to_frame(n, "blur") {
        return Ok(Value::Undefined);
    }
    if state().borrow().focused != Some(n.node) {
        return Ok(Value::Undefined);
    }
    state().borrow_mut().focused = None;
    with_env(|env| env.focus_node(None));
    crate::events::dispatch(realm, TargetKey::Node(n), "blur", crate::EventInit::simple(false, false), true);
    crate::events::dispatch(realm, TargetKey::Node(n), "focusout", crate::EventInit::simple(true, false), true);
    Ok(Value::Undefined)
}

/// `click()` / `focus()` / `blur()` on a node of a same-origin child
/// frame run in the frame's realm (its listeners and default actions).
fn forward_to_frame(n: NodeRef, method: &str) -> bool {
    if n.doc & crate::host::FRAME_DOC == 0 {
        return false;
    }
    let frame = (n.doc & !crate::host::FRAME_DOC) as u64;
    match crate::frames::call_frame(frame, crate::FrameCall::Method { node: n.node, name: method.to_string() }) {
        Some(crate::FrameReply::Error(m)) => {
            crate::host::log("error", &format!("Uncaught {m}"));
            true
        }
        Some(_) => true,
        None => false,
    }
}

/// `element.click()`: dispatch a synthetic click and run the default action.
fn e_click(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "click")?;
    if forward_to_frame(n, "click") {
        return Ok(Value::Undefined);
    }
    crate::events::synthetic_click(realm, n);
    Ok(Value::Undefined)
}

fn e_hidden_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "hidden")?;
    Ok(Value::Bool(get_attribute(n, "hidden").is_some()))
}

fn e_hidden_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "hidden")?;
    let b = realm.to_boolean(&arg(args, 0));
    set_attribute(realm, n, "hidden", if b { Some("") } else { None });
    Ok(Value::Undefined)
}

fn e_tab_index_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "tabIndex")?;
    Ok(Value::Number(with_doc(n.doc, |doc| doc.tab_index(n.node)) as f64))
}

fn e_tab_index_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "tabIndex")?;
    let v = realm.to_int32(arg(args, 0))?;
    set_attribute(realm, n, "tabindex", Some(&v.to_string()));
    Ok(Value::Undefined)
}

/// The `contenteditable` state of an attribute value: Some(true / false)
/// for a valid keyword, None for `inherit` (missing or invalid).
fn content_editable_state(v: Option<&str>) -> Option<(bool, &'static str)> {
    match v?.to_ascii_lowercase().as_str() {
        "" | "true" => Some((true, "true")),
        "plaintext-only" => Some((true, "plaintext-only")),
        "false" => Some((false, "false")),
        _ => None,
    }
}

fn e_content_editable_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "contentEditable")?;
    let v = get_attribute(n, "contenteditable");
    Ok(Value::str(content_editable_state(v.as_deref()).map(|(_, k)| k).unwrap_or("inherit")))
}

/// `contentEditable = "true" | "false" | "plaintext-only" | "inherit"`
/// (ASCII case-insensitive; anything else is a SyntaxError).
fn e_content_editable_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "contentEditable")?;
    let v = arg_str(realm, args, 0)?.to_ascii_lowercase();
    match v.as_str() {
        "inherit" => set_attribute(realm, n, "contenteditable", None),
        "true" | "false" | "plaintext-only" => set_attribute(realm, n, "contenteditable", Some(&v)),
        _ => return Err(crate::misc::dom_exception(realm, "SyntaxError", &format!("Failed to set the 'contentEditable' property on 'HTMLElement': The value provided ('{v}') is not one of 'true', 'false', 'plaintext-only', or 'inherit'."))),
    }
    Ok(Value::Undefined)
}

/// Editable when the nearest ancestor-or-self with a valid
/// `contenteditable` value says so.
fn e_is_content_editable(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "isContentEditable")?;
    let editable = with_doc(n.doc, |doc| doc.editing_host(n.node).is_some());
    Ok(Value::Bool(editable))
}

fn e_animate(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let _ = this_node(realm, &this, "animate")?;
    let (p, resolve, _) = realm.new_promise();
    let anim = object_from(realm, vec![("finished", Value::Object(p)), ("playState", Value::str("finished")), ("currentTime", Value::Number(0.0)), ("playbackRate", Value::Number(1.0))]);
    if let V::Object(a) = anim.v() {
        realm.define_method(a, "cancel", 0, e_noop);
        realm.define_method(a, "finish", 0, e_noop);
        realm.define_method(a, "pause", 0, e_noop);
        realm.define_method(a, "play", 0, e_noop);
        realm.define_method(a, "reverse", 0, e_noop);
        realm.define_method(a, "addEventListener", 2, e_noop);
    }
    realm.call(Value::Object(resolve), Value::Undefined, &[anim.clone()])?;
    Ok(anim)
}

fn e_noop(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Undefined)
}

fn e_get_animations(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(array(realm, vec![]))
}

fn e_style(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "style")?;
    let V::Object(w) = this.v() else { unreachable!() };
    Ok(crate::cssom::style_object(realm, w, n))
}

fn e_style_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "style")?;
    let v = arg_str(realm, args, 0)?;
    set_attribute(realm, n, "style", Some(&v));
    Ok(Value::Undefined)
}

// -------------------------------------------------------------- subclasses

/// The element's `href` parsed against the base URL with the document's
/// encoding for the query (HTML "parse a URL").
fn anchor_url(n: NodeRef) -> Option<rabbithole_net::Url> {
    let href = get_attribute(n, "href")?;
    let enc = with_doc(n.doc, |doc| rabbithole_net::encoding::Encoding::for_label(&doc.character_set)).unwrap_or(rabbithole_net::encoding::Encoding::UTF_8);
    rabbithole_net::url::parse_with_encoding(href.trim(), Some(&base_url()), enc)
}

fn a_href_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "href")?;
    Ok(Value::str(&anchor_url(n).map(|u| u.to_string()).or_else(|| get_attribute(n, "href")).unwrap_or_default()))
}

fn a_href_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "href")?;
    let v = arg_str(realm, args, 0)?;
    set_attribute(realm, n, "href", Some(&v));
    Ok(Value::Undefined)
}

fn url_part(realm: &mut Realm, this: &Value, which: &str) -> JsResult<Value> {
    let n = this_node(realm, this, which)?;
    let Some(u) = anchor_url(n) else { return Ok(Value::str("")) };
    Ok(Value::str(&crate::fetch::url_component(&u, which)))
}

macro_rules! url_getter {
    ($name:ident, $part:expr) => {
        fn $name(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
            url_part(realm, &this, $part)
        }
    };
}
url_getter!(a_protocol, "protocol");
url_getter!(a_host, "host");
url_getter!(a_hostname, "hostname");
url_getter!(a_port, "port");
url_getter!(a_pathname, "pathname");
url_getter!(a_search, "search");
url_getter!(a_hash, "hash");
url_getter!(a_origin, "origin");

fn a_to_string(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    a_href_get(realm, this, &[])
}

// input ---------------------------------------------------------------------

/// Does `value` of this `<input>` use the "value" mode (a dirty value
/// separate from the `value` attribute)? Other modes reflect the attribute
/// (HTML §4.10.5.4).
fn input_value_mode_is_value(n: NodeRef) -> bool {
    let ty = get_attribute(n, "type").map(|t| t.to_ascii_lowercase()).unwrap_or_default();
    !matches!(ty.as_str(), "hidden" | "submit" | "image" | "reset" | "button" | "checkbox" | "radio" | "file")
}

/// The current value of a text-like control (dirty value, else default).
pub(crate) fn control_value(n: NodeRef) -> Option<String> {
    with_doc(n.doc, |doc| doc.control_value(n.node).map(|s| s.to_string()))
}

fn input_value_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "value")?;
    let tag = tag_of(n).unwrap_or_default();
    Ok(Value::str(&match tag.as_str() {
        "textarea" => control_value(n).unwrap_or_else(|| with_doc(n.doc, |doc| doc.text_content(n.node))),
        "select" => select_value(n),
        "option" => option_value(n),
        "button" => get_attribute(n, "value").unwrap_or_default(),
        "input" if input_value_mode_is_value(n) => control_value(n).unwrap_or_default(),
        _ => {
            let ty = get_attribute(n, "type").map(|t| t.to_ascii_lowercase()).unwrap_or_default();
            let v = get_attribute(n, "value").unwrap_or_default();
            if matches!(ty.as_str(), "checkbox" | "radio") && v.is_empty() {
                "on".into()
            } else {
                v
            }
        }
    }))
}

fn input_value_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "value")?;
    let v = match args.first().map(Value::v) {
        None | Some(V::Null) | Some(V::Undefined) => String::new(),
        Some(v) => realm.to_rust_string(v.to_value())?,
    };
    let tag = tag_of(n).unwrap_or_default();
    if tag == "select" {
        set_select_value(n, &v);
    } else if tag == "textarea" || (tag == "input" && input_value_mode_is_value(n)) {
        // "value" mode: set the dirty value; the attribute is the default.
        with_doc(n.doc, |doc| {
            let old = if tag == "textarea" { doc.control_value(n.node).map(str::to_string).unwrap_or_else(|| doc.text_content(n.node)) } else { doc.control_value(n.node).unwrap_or("").to_string() };
            doc.set_control_value(n.node, Some(&v));
            if old != v {
                let end = v.encode_utf16().count();
                doc.set_control_selection(n.node, end, end, ControlDirection::None);
            }
        });
        if n.doc == 0 {
            mark_dirty();
        }
    } else {
        set_attribute(realm, n, "value", Some(&v));
    }
    Ok(Value::Undefined)
}

fn input_default_value_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "defaultValue")?;
    let tag = tag_of(n).unwrap_or_default();
    Ok(Value::str(&if tag == "textarea" {
        with_doc(n.doc, |doc| doc.text_content(n.node))
    } else {
        get_attribute(n, "value").unwrap_or_default()
    }))
}

/// `input.defaultValue = v` sets the `value` content attribute; the
/// current value follows it only while the control is not dirty (React
/// writes `defaultValue` while restoring an uncontrolled input after an
/// `input` event: the typed text must survive).
fn input_default_value_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "defaultValue")?;
    let v = arg_str(realm, args, 0)?;
    set_attribute(realm, n, "value", Some(&v));
    Ok(Value::Undefined)
}

fn input_checked_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "checked")?;
    Ok(Value::Bool(with_doc(n.doc, |doc| doc.is_checked(n.node))))
}

/// Set the checkedness of a control (its "dirty checkedness"; the
/// `checked` attribute stays the default).
pub(crate) fn set_checkedness(n: NodeRef, checked: Option<bool>) {
    with_doc(n.doc, |doc| doc.set_checked(n.node, checked));
    if n.doc == 0 {
        mark_dirty();
    }
}

fn input_checked_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "checked")?;
    let b = realm.to_boolean(&arg(args, 0));
    if b && get_attribute(n, "type").map(|t| t.eq_ignore_ascii_case("radio")).unwrap_or(false) {
        // Uncheck the rest of the group.
        if let Some(name) = get_attribute(n, "name") {
            let form = with_doc(n.doc, |doc| doc.ancestor_or_self(n.node, |a| doc.is_element(a, "form")).unwrap_or(Document::ROOT));
            let group: Vec<NodeId> = with_doc(n.doc, |doc| {
                doc.descendants(form)
                    .filter(|&d| d != n.node && doc.is_element(d, "input") && doc.attr(d, "name") == Some(name.as_str()) && doc.attr(d, "type").map(|t| t.eq_ignore_ascii_case("radio")).unwrap_or(false))
                    .collect()
            });
            for g in group {
                set_checkedness(NodeRef { doc: n.doc, node: g }, Some(false));
            }
        }
    }
    set_checkedness(n, Some(b));
    Ok(Value::Undefined)
}

fn input_type_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "type")?;
    let tag = tag_of(n).unwrap_or_default();
    Ok(Value::str(&match tag.as_str() {
        // Enumerated, limited to known values: unknown types are text
        // (input) and submit (button) controls.
        "input" => {
            let t = get_attribute(n, "type").unwrap_or_default();
            crate::reflect::INPUT_TYPES.iter().find(|k| k.eq_ignore_ascii_case(&t)).unwrap_or(&"text").to_string()
        }
        "button" => {
            let t = get_attribute(n, "type").unwrap_or_default();
            ["submit", "reset", "button"].iter().find(|k| k.eq_ignore_ascii_case(&t)).unwrap_or(&"submit").to_string()
        }
        "select" => if get_attribute(n, "multiple").is_some() { "select-multiple".into() } else { "select-one".into() },
        "textarea" => "textarea".into(),
        _ => get_attribute(n, "type").unwrap_or_default(),
    }))
}

fn input_type_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "type")?;
    let v = arg_str(realm, args, 0)?;
    set_attribute(realm, n, "type", Some(&v));
    Ok(Value::Undefined)
}

fn control_form(n: NodeRef) -> Option<NodeId> {
    with_doc(n.doc, |doc| {
        if let Some(id) = doc.attr(n.node, "form") {
            if let Some(f) = doc.element_by_id(id) {
                return Some(f);
            }
        }
        doc.ancestor_or_self(n.node, |a| doc.is_element(a, "form"))
    })
}

fn input_form(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "form")?;
    let f = control_form(n);
    Ok(wrap_opt(realm, n.doc, f))
}

fn input_labels(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "labels")?;
    let id = get_attribute(n, "id");
    let labels: Vec<NodeRef> = with_doc(n.doc, |doc| {
        doc.descendants(Document::ROOT)
            .filter(|&d| doc.is_element(d, "label") && ((id.is_some() && doc.attr(d, "for") == id.as_deref()) || doc.is_ancestor(d, n.node)))
            .map(|d| NodeRef { doc: n.doc, node: d })
            .collect()
    });
    Ok(node::node_list(realm, labels))
}

fn input_select(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "select")?;
    if control_selection_applies(n) {
        set_control_selection(n, 0, usize::MAX, ControlDirection::None);
    }
    Ok(Value::Undefined)
}

fn input_files(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let _ = this_node(realm, &this, "files")?;
    let arr = realm.new_array(vec![]);
    realm.define_method(arr, "item", 1, node::define_hidden_method_item);
    Ok(Value::Object(arr))
}

fn input_validity(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "validity")?;
    let missing = get_attribute(n, "required").is_some() && control_value(n).map(|v| v.is_empty()).unwrap_or(true);
    Ok(object_from(realm, vec![("valid", Value::Bool(!missing)), ("valueMissing", Value::Bool(missing)), ("typeMismatch", Value::Bool(false)), ("patternMismatch", Value::Bool(false)), ("tooLong", Value::Bool(false)), ("tooShort", Value::Bool(false)), ("rangeUnderflow", Value::Bool(false)), ("rangeOverflow", Value::Bool(false)), ("stepMismatch", Value::Bool(false)), ("badInput", Value::Bool(false)), ("customError", Value::Bool(false))]))
}

fn input_check_validity(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "checkValidity")?;
    let missing = get_attribute(n, "required").is_some() && control_value(n).map(|v| v.is_empty()).unwrap_or(true);
    Ok(Value::Bool(!missing))
}

fn control_selection_applies(n: NodeRef) -> bool {
    if tag_of(n).as_deref() == Some("textarea") { return true; }
    let ty = get_attribute(n, "type").unwrap_or_default().to_ascii_lowercase();
    // Invalid input types fall back to the text state.
    !matches!(ty.as_str(), "hidden" | "email" | "date" | "month" | "week" | "time" | "datetime-local" | "number" | "range" | "color" | "checkbox" | "radio" | "file" | "submit" | "image" | "reset" | "button")
}

fn set_control_selection(n: NodeRef, start: usize, end: usize, direction: ControlDirection) {
    with_doc(n.doc, |doc| doc.set_control_selection(n.node, start, end, direction));
    if n.doc == 0 { mark_dirty(); }
}

fn selection_offset(realm: &mut Realm, value: Value) -> JsResult<usize> {
    let number = realm.to_number(value)?;
    Ok(if number.is_finite() { number.trunc().rem_euclid(4294967296.0) as usize } else { 0 })
}

fn selection_direction(value: &str) -> ControlDirection {
    match value { "forward" => ControlDirection::Forward, "backward" => ControlDirection::Backward, _ => ControlDirection::None }
}

fn require_control_selection(realm: &mut Realm, n: NodeRef) -> JsResult<()> {
    if control_selection_applies(n) { return Ok(()); }
    Err(crate::misc::dom_exception(realm, "InvalidStateError", "The input type does not support text selection."))
}

fn input_selection_start(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "selectionStart")?;
    if !control_selection_applies(n) { return Ok(Value::Null); }
    Ok(Value::Number(with_doc(n.doc, |doc| doc.control_selection(n.node).start) as f64))
}

fn input_selection_end(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "selectionEnd")?;
    if !control_selection_applies(n) { return Ok(Value::Null); }
    Ok(Value::Number(with_doc(n.doc, |doc| doc.control_selection(n.node).end) as f64))
}

fn input_selection_direction(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "selectionDirection")?;
    if !control_selection_applies(n) { return Ok(Value::Null); }
    let direction = with_doc(n.doc, |doc| doc.control_selection(n.node).direction);
    Ok(Value::str(match direction { ControlDirection::Forward => "forward", ControlDirection::Backward => "backward", ControlDirection::None => "none" }))
}

fn input_selection_start_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "selectionStart")?;
    require_control_selection(realm, n)?;
    let start = selection_offset(realm, arg(args, 0))?;
    let old = with_doc(n.doc, |doc| doc.control_selection(n.node));
    set_control_selection(n, start, old.end.max(start), old.direction);
    Ok(Value::Undefined)
}

fn input_selection_end_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "selectionEnd")?;
    require_control_selection(realm, n)?;
    let end = selection_offset(realm, arg(args, 0))?;
    let old = with_doc(n.doc, |doc| doc.control_selection(n.node));
    set_control_selection(n, old.start, end, old.direction);
    Ok(Value::Undefined)
}

fn input_selection_direction_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "selectionDirection")?;
    require_control_selection(realm, n)?;
    let direction = selection_direction(&arg_str(realm, args, 0)?);
    let old = with_doc(n.doc, |doc| doc.control_selection(n.node));
    set_control_selection(n, old.start, old.end, direction);
    Ok(Value::Undefined)
}

fn input_set_selection_range(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "setSelectionRange")?;
    require_control_selection(realm, n)?;
    let start = selection_offset(realm, arg(args, 0))?;
    let end = selection_offset(realm, arg(args, 1))?;
    let direction = if args.len() > 2 { selection_direction(&arg_str(realm, args, 2)?) } else { ControlDirection::None };
    set_control_selection(n, start, end, direction);
    Ok(Value::Undefined)
}

fn input_set_custom_validity(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let _ = this_node(realm, &this, "setCustomValidity")?;
    Ok(Value::Undefined)
}

// select / option ---------------------------------------------------------------

fn options_of(n: NodeRef) -> Vec<NodeId> {
    with_doc(n.doc, |doc| doc.select_options(n.node))
}

fn option_value(n: NodeRef) -> String {
    get_attribute(n, "value").unwrap_or_else(|| with_doc(n.doc, |doc| doc.text_content(n.node).trim().to_string()))
}

fn select_value(n: NodeRef) -> String {
    let option = with_doc(n.doc, |doc| doc.select_options(n.node).into_iter().find(|&o| doc.is_option_selected(o)));
    option.map(|o| option_value(NodeRef { doc: n.doc, node: o })).unwrap_or_default()
}

fn set_select_value(n: NodeRef, v: &str) {
    let index = options_of(n).iter().position(|&o| option_value(NodeRef { doc: n.doc, node: o }) == v);
    with_doc(n.doc, |doc| doc.set_select_index(n.node, index));
    if n.doc == 0 { mark_dirty(); }
}

fn select_options(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "options")?;
    let opts: Vec<NodeRef> = options_of(n).into_iter().map(|o| NodeRef { doc: n.doc, node: o }).collect();
    Ok(node::html_collection(realm, opts))
}

fn select_selected_options(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "selectedOptions")?;
    let opts: Vec<NodeRef> = options_of(n).into_iter().map(|o| NodeRef { doc: n.doc, node: o }).filter(|o| with_doc(o.doc, |doc| doc.is_option_selected(o.node))).collect();
    Ok(node::html_collection(realm, opts))
}

fn select_selected_index_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "selectedIndex")?;
    let index = with_doc(n.doc, |doc| doc.select_options(n.node).iter().position(|&o| doc.is_option_selected(o)));
    Ok(Value::Number(index.map(|i| i as f64).unwrap_or(-1.0)))
}

fn select_selected_index_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "selectedIndex")?;
    let value = realm.to_number(arg(args, 0))?;
    // Web IDL long conversion, including NaN/infinity -> 0 and modulo 2^32.
    let i = if value.is_finite() { value.trunc().rem_euclid(4294967296.0) as u32 as i32 } else { 0 };
    with_doc(n.doc, |doc| doc.set_select_index(n.node, usize::try_from(i).ok()));
    if n.doc == 0 { mark_dirty(); }
    Ok(Value::Undefined)
}

fn select_length(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "length")?;
    Ok(Value::Number(options_of(n).len() as f64))
}

fn select_add(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "add")?;
    let Some(opt) = node_of(realm, &arg(args, 0)) else { return Ok(Value::Undefined) };
    let before = match (args.get(1)).map(Value::v) {
        Some(V::Number(i)) => options_of(n).get(i as usize).copied(),
        Some(v) => node_of(realm, &v.to_value()).map(|r| r.node),
        None => None,
    };
    insert_node(realm, n, opt, before)?;
    Ok(Value::Undefined)
}

fn select_remove(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "remove")?;
    match args.first() {
        Some(v) if !v.is_undefined() => {
            let i = realm.to_number(v.clone())?;
            if let Some(o) = options_of(n).get(i as usize).copied() {
                remove_node(realm, NodeRef { doc: n.doc, node: o });
            }
        }
        _ => remove_node(realm, n),
    }
    Ok(Value::Undefined)
}

fn select_item(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "item")?;
    let i = realm.to_number(arg(args, 0))?;
    let o = options_of(n).get(i.max(0.0) as usize).copied();
    Ok(wrap_opt(realm, n.doc, o))
}

fn option_text_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "text")?;
    Ok(Value::str(with_doc(n.doc, |doc| doc.text_content(n.node)).split_whitespace().collect::<Vec<_>>().join(" ").as_str()))
}

fn option_text_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "text")?;
    let v = arg_str(realm, args, 0)?;
    set_text_content(realm, n, &v);
    Ok(Value::Undefined)
}

/// `option.label`: the label content attribute when present, else the
/// option's text; setting writes the attribute.
fn option_label_get(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "label")?;
    // (The label attribute in no namespace, not a namespaced one.)
    match find_attr_ns(n, None, "label").map(|(_, v)| v) {
        Some(l) => Ok(Value::str(&l)),
        None => option_text_get(realm, this, args),
    }
}

fn option_label_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "label")?;
    let v = arg_str(realm, args, 0)?;
    set_attribute(realm, n, "label", Some(&v));
    Ok(Value::Undefined)
}

fn option_value_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "value")?;
    Ok(Value::str(&option_value(n)))
}

fn option_selected_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "selected")?;
    Ok(Value::Bool(with_doc(n.doc, |doc| doc.is_option_selected(n.node))))
}

fn option_selected_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "selected")?;
    let selected = realm.to_boolean(&arg(args, 0));
    with_doc(n.doc, |doc| doc.set_option_selected(n.node, selected));
    if n.doc == 0 { mark_dirty(); }
    Ok(Value::Undefined)
}

fn option_default_selected_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "defaultSelected")?;
    Ok(Value::Bool(get_attribute(n, "selected").is_some()))
}

fn option_default_selected_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "defaultSelected")?;
    let selected = realm.to_boolean(&arg(args, 0));
    set_attribute(realm, n, "selected", if selected { Some("") } else { None });
    Ok(Value::Undefined)
}

fn option_index(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "index")?;
    let sel = with_doc(n.doc, |doc| doc.ancestor_or_self(n.node, |a| doc.is_element(a, "select")));
    let idx = sel.and_then(|s| options_of(NodeRef { doc: n.doc, node: s }).iter().position(|o| *o == n.node)).unwrap_or(0);
    Ok(Value::Number(idx as f64))
}

fn option_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let text = arg_str_opt(realm, args, 0)?.unwrap_or_default();
    let value = arg_str_opt(realm, args, 1)?;
    let default_selected = args.get(2).map(|v| realm.to_boolean(v)).unwrap_or(false);
    let selected = args.get(3).map(|v| realm.to_boolean(v)).unwrap_or(false);
    let node = with_doc(0, |doc| {
        let mut attrs = Vec::new();
        if let Some(v) = &value {
            attrs.push(("value".to_string(), v.clone()));
        }
        if default_selected {
            attrs.push(("selected".to_string(), String::new()));
        }
        let o = doc.create_element("option", attrs);
        doc.initialize_option_selected(o, selected);
        if !text.is_empty() {
            let t = doc.create_text(&text);
            doc.append_child(o, t);
        }
        o
    });
    Ok(wrap_main(realm, node))
}

// form ------------------------------------------------------------------------

fn form_elements(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "elements")?;
    let els: Vec<NodeRef> = with_doc(n.doc, |doc| {
        doc.descendants(n.node)
            .skip(1)
            .filter(|&d| matches!(doc.tag(d), Some("input" | "select" | "textarea" | "button" | "fieldset" | "output" | "object")))
            .map(|d| NodeRef { doc: n.doc, node: d })
            .collect()
    });
    Ok(node::html_collection(realm, els))
}

fn form_length(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "length")?;
    let c = with_doc(n.doc, |doc| doc.descendants(n.node).skip(1).filter(|&d| matches!(doc.tag(d), Some("input" | "select" | "textarea" | "button"))).count());
    Ok(Value::Number(c as f64))
}

fn form_submit(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "submit")?;
    if n.doc == 0 {
        with_env(|env| env.submit_form(n.node, None));
    }
    Ok(Value::Undefined)
}

fn form_request_submit(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "requestSubmit")?;
    let submitter = node_of(realm, &arg(args, 0)).map(|s| s.node);
    let r = crate::events::dispatch(realm, TargetKey::Node(n), "submit", crate::EventInit::simple(true, true), false);
    if !r.default_prevented && n.doc == 0 {
        with_env(|env| env.submit_form(n.node, submitter));
    }
    Ok(Value::Undefined)
}

fn form_reset(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "reset")?;
    let r = crate::events::dispatch(realm, TargetKey::Node(n), "reset", crate::EventInit::simple(true, true), false);
    if r.default_prevented {
        return Ok(Value::Undefined);
    }
    let controls: Vec<NodeId> = with_doc(n.doc, |doc| doc.descendants(n.node).skip(1).filter(|&d| matches!(doc.tag(d), Some("input" | "select" | "textarea"))).collect());
    for c in controls {
        let cr = NodeRef { doc: n.doc, node: c };
        let tag = tag_of(cr).unwrap_or_default();
        match tag.as_str() {
            "select" => {
                with_doc(cr.doc, |doc| doc.reset_select(cr.node));
                if cr.doc == 0 { mark_dirty(); }
            }
            _ => {
                let ty = get_attribute(cr, "type").map(|t| t.to_ascii_lowercase()).unwrap_or_default();
                if matches!(ty.as_str(), "checkbox" | "radio") {
                    // Back to the default checkedness.
                    set_checkedness(cr, None);
                } else {
                    // Back to the default value.
                    with_doc(cr.doc, |doc| doc.set_control_value(cr.node, None));
                    if cr.doc == 0 {
                        mark_dirty();
                    }
                }
            }
        }
    }
    Ok(Value::Undefined)
}

fn form_check_validity(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "checkValidity")?;
    let ok = with_doc(n.doc, |doc| {
        doc.descendants(n.node).all(|d| {
            !(doc.is_element(d, "input") && doc.attr(d, "required").is_some() && doc.control_value(d).map(|v| v.is_empty()).unwrap_or(true))
        })
    });
    Ok(Value::Bool(ok))
}

fn form_action_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "action")?;
    let a = get_attribute(n, "action").unwrap_or_default();
    if a.trim().is_empty() {
        return Ok(Value::str(&with_env(|env| env.document_url().to_string())));
    }
    Ok(Value::str(&resolve_url(&a).map(|u| u.to_string()).unwrap_or(a)))
}

// image ---------------------------------------------------------------------

fn img_engine_state(n: NodeRef) -> Option<(u32, u32, bool, String)> {
    if n.doc != 0 {
        return None;
    }
    with_env(|env| env.image_state(n.node))
}

fn img_natural_width(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "naturalWidth")?;
    if let Some((w, _, _, _)) = img_engine_state(n) {
        return Ok(Value::Number(w as f64));
    }
    Ok(Value::Number(node_rect(n).w.round() as f64))
}

fn img_natural_height(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "naturalHeight")?;
    if let Some((_, h, _, _)) = img_engine_state(n) {
        return Ok(Value::Number(h as f64));
    }
    Ok(Value::Number(node_rect(n).h.round() as f64))
}

fn img_complete(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "complete")?;
    Ok(Value::Bool(img_engine_state(n).map(|s| s.2).unwrap_or(true)))
}

fn img_current_src(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "currentSrc")?;
    match img_engine_state(n) {
        Some((_, _, _, src)) if !src.is_empty() => Ok(Value::str(&src)),
        // No current request: "" for an absent or empty src.
        _ if get_attribute(n, "src").is_none_or(|s| s.is_empty()) => Ok(Value::str("")),
        _ => img_src_get(realm, this, args),
    }
}

/// `srcset`/`sizes` setters: reflect, then let the engine reselect.
fn img_srcset_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "srcset")?;
    let v = arg_str(realm, args, 0)?;
    set_attribute(realm, n, "srcset", Some(&v));
    if n.doc == 0 {
        with_env(|env| env.image_src_changed(n.node));
    }
    Ok(Value::Undefined)
}

fn img_srcset_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "srcset")?;
    Ok(Value::str(&get_attribute(n, "srcset").unwrap_or_default()))
}

fn img_decode(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let (p, resolve, _) = realm.new_promise();
    realm.call(Value::Object(resolve), Value::Undefined, &[Value::Undefined])?;
    Ok(Value::Object(p))
}

fn image_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let w = args.first().filter(|v| !v.is_undefined()).map(|v| realm.to_number(v.clone())).transpose()?;
    let h = args.get(1).filter(|v| !v.is_undefined()).map(|v| realm.to_number(v.clone())).transpose()?;
    let node = with_doc(0, |doc| {
        let mut attrs = Vec::new();
        if let Some(w) = w {
            attrs.push(("width".to_string(), format!("{}", w as i64)));
        }
        if let Some(h) = h {
            attrs.push(("height".to_string(), format!("{}", h as i64)));
        }
        doc.create_element("img", attrs)
    });
    Ok(wrap_main(realm, node))
}

/// Setting `img.src` from script fires `load` asynchronously (we cannot
/// fetch asynchronously; the image appears on the next layout).
fn img_src_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "src")?;
    let v = arg_str(realm, args, 0)?;
    set_attribute(realm, n, "src", Some(&v));
    // The engine fetches the image and fires `load`/`error` when done.
    if n.doc == 0 && with_env(|env| env.image_src_changed(n.node)) {
        return Ok(Value::Undefined);
    }
    state().borrow_mut().pending_script_events.push((n, "load"));
    let f = state().borrow().protos.get("__deliver_script_events").copied();
    if let Some(f) = f {
        realm.enqueue_callback(Value::Object(f), Value::Undefined, Vec::new());
    }
    Ok(Value::Undefined)
}

fn img_src_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "src")?;
    // A reflected URL: absent is "", anything else parses against the base
    // ("" being the document's base URL itself).
    let Some(raw) = get_attribute(n, "src") else { return Ok(Value::str("")) };
    Ok(Value::str(&base_url().join(raw.trim_matches(|c: char| c <= ' ')).map(|u| u.to_string()).unwrap_or(raw)))
}

// script / style / link / template / canvas / media ---------------------------------

fn script_text_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "text")?;
    Ok(Value::str(&with_doc(n.doc, |doc| doc.text_content(n.node))))
}

fn script_text_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "text")?;
    let v = arg_str(realm, args, 0)?;
    set_text_content(realm, n, &v);
    Ok(Value::Undefined)
}

fn style_sheet(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "sheet")?;
    let href = get_attribute(n, "href");
    let rules = realm.new_array_value(vec![]);
    let o = object_from(realm, vec![("href", href.map(|h| Value::str(&h)).unwrap_or(Value::Null)), ("cssRules", rules.clone()), ("rules", rules), ("disabled", Value::Bool(false)), ("type", Value::str("text/css"))]);
    if let V::Object(id) = o.v() {
        realm.define_method(id, "insertRule", 2, sheet_insert_rule);
        realm.define_method(id, "deleteRule", 1, e_noop);
        realm.define_method(id, "addRule", 3, sheet_insert_rule);
        realm.define_method(id, "removeRule", 1, e_noop);
        let owner = wrap(realm, n);
        realm.define_property(id, "ownerNode", owner, CONFIGURABLE);
    }
    Ok(o)
}

/// `sheet.insertRule(text)`: append the rule text to the owning `<style>`.
fn sheet_insert_rule(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let owner = realm.get_str(this, "ownerNode")?;
    let rule = arg_str(realm, args, 0)?;
    if let Some(n) = node_of(realm, &owner) {
        if tag_of(n).as_deref() == Some("style") {
            let cur = with_doc(n.doc, |doc| doc.text_content(n.node));
            set_text_content(realm, n, &format!("{cur}\n{rule}"));
        }
    }
    Ok(Value::Number(0.0))
}

fn template_content(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "content")?;
    let frag = with_doc(n.doc, |doc| doc.template_content(n.node));
    Ok(wrap(realm, NodeRef { doc: n.doc, node: frag }))
}

/// `canvas.getContext()` returns null: there is no 2D canvas yet.
fn canvas_get_context(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Null)
}

fn canvas_to_data_url(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::str("data:,"))
}

/// Queue media events (fired asynchronously, not bubbling).
fn queue_media_events(realm: &mut Realm, n: NodeRef, events: &[&'static str]) {
    for e in events {
        state().borrow_mut().pending_script_events.push((n, e));
    }
    let f = state().borrow().protos.get("__deliver_script_events").copied();
    if let Some(f) = f {
        realm.enqueue_callback(Value::Object(f), Value::Undefined, Vec::new());
    }
}

fn media_set_state(realm: &mut Realm, this: &Value, key: &str, v: Value) {
    if let V::Object(o) = this.v() {
        realm.define_property(o, key, v, rabbithole_js::HIDDEN | ENUMERABLE);
    }
}

/// `play()`: playback state flips to playing (`play`/`playing` events) and
/// the returned promise resolves. There is no audio/video output; the
/// element shows its poster and native controls.
fn media_play(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let was_paused = realm.get_str(this.clone(), "paused").map(|v| v.truthy()).unwrap_or(true);
    media_set_state(realm, &this, "paused", Value::Bool(false));
    if was_paused {
        if let Some(n) = node_of(realm, &this) {
            queue_media_events(realm, n, &["play", "playing"]);
        }
    }
    let (p, resolve, _) = realm.new_promise();
    realm.call(Value::Object(resolve), Value::Undefined, &[Value::Undefined])?;
    Ok(Value::Object(p))
}

fn media_pause(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let was_paused = realm.get_str(this.clone(), "paused").map(|v| v.truthy()).unwrap_or(true);
    media_set_state(realm, &this, "paused", Value::Bool(true));
    if !was_paused {
        if let Some(n) = node_of(realm, &this) {
            queue_media_events(realm, n, &["pause"]);
        }
    }
    Ok(Value::Undefined)
}

fn media_load(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    media_set_state(realm, &this, "paused", Value::Bool(true));
    if let Some(n) = node_of(realm, &this) {
        queue_media_events(realm, n, &["emptied", "loadstart"]);
    }
    Ok(Value::Undefined)
}

/// No audio/video codecs are available for playback: `""` for everything
/// (scripts then fall back to posters / static content).
fn media_can_play_type(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::str(""))
}

fn iframe_content_window(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "contentWindow")?;
    let id = with_env(|env| if n.doc == 0 { env.ensure_frame(n.node) }
        else if n.doc & FRAME_DOC != 0 { env.ensure_frame_in((n.doc & !FRAME_DOC) as u64, n.node) }
        else { None });
    crate::frames::window_for(realm, id)
}

/// The frame's document, when it is same-origin (null otherwise).
fn iframe_content_document(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "contentDocument")?;
    let Some(id) = with_env(|env| if n.doc == 0 { env.ensure_frame(n.node) }
        else if n.doc & FRAME_DOC != 0 { env.ensure_frame_in((n.doc & !FRAME_DOC) as u64, n.node) }
        else { None }) else { return Ok(Value::Null) };
    Ok(crate::frames::frame_document_value(realm, id))
}

fn label_control(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "control")?;
    let target = match get_attribute(n, "for") {
        Some(id) => with_doc(n.doc, |doc| doc.element_by_id(&id)),
        None => with_doc(n.doc, |doc| doc.descendants(n.node).skip(1).find(|&d| matches!(doc.tag(d), Some("input" | "select" | "textarea" | "button")))),
    };
    Ok(wrap_opt(realm, n.doc, target))
}

fn svg_get_bbox(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "getBBox")?;
    let r = node_rect(n);
    Ok(dom_rect(realm, 0.0, 0.0, r.w, r.h))
}

fn svg_owner(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "ownerSVGElement")?;
    let o = with_doc(n.doc, |doc| doc.ancestor_or_self(n.node, |a| a != n.node && doc.is_element(a, "svg")));
    Ok(wrap_opt(realm, n.doc, o))
}

pub(crate) const EVENT_HANDLER_NAMES: &[&str] = &[
    "click", "dblclick", "mousedown", "mouseup", "mousemove", "mouseover", "mouseout", "mouseenter", "mouseleave", "contextmenu", "wheel",
    "keydown", "keyup", "keypress", "input", "change", "submit", "reset", "focus", "blur", "focusin", "focusout", "load", "error", "abort",
    "scroll", "resize", "touchstart", "touchend", "touchmove", "touchcancel", "pointerdown", "pointerup", "pointermove", "pointerover",
    "pointerout", "pointerenter", "pointerleave", "pointercancel", "drag", "dragstart", "dragend", "dragover", "dragenter", "dragleave", "drop",
    "animationstart", "animationend", "animationiteration", "transitionend", "transitionstart", "select", "invalid", "toggle", "play", "pause",
    "ended", "canplay", "loadeddata", "loadedmetadata", "timeupdate", "volumechange", "cut", "copy", "paste", "beforeinput", "compositionstart",
    "compositionend", "auxclick", "search",
];

pub(crate) fn install(realm: &mut Realm) {
    let el = define_class(realm, "Element", Some("Node"), illegal_ctor);
    crate::node::install_parent_node(realm, el);
    crate::node::install_get_elements_by(realm, el);
    crate::node::install_child_node(realm, el);
    crate::node::install_non_document_type_child_node(realm, el);
    let acc = |realm: &mut Realm, name: &str, g: rabbithole_js::NativeFn, s: Option<rabbithole_js::NativeFn>| realm.define_accessor(el, name, Some(g), s, ACC);
    acc(realm, "tagName", e_tag_name, None);
    acc(realm, "localName", e_local_name, None);
    acc(realm, "namespaceURI", e_namespace_uri, None);
    acc(realm, "prefix", e_prefix, None);
    acc(realm, "id", e_id_get, Some(id_set));
    acc(realm, "className", e_class_name_get, Some(e_class_name_set));
    acc(realm, "classList", e_class_list, Some(e_class_list_set));
    acc(realm, "part", e_part, Some(e_part_set));
    acc(realm, "attributes", e_attributes, None);
    acc(realm, "innerHTML", e_inner_html_get, Some(e_inner_html_set));
    acc(realm, "outerHTML", e_outer_html_get, Some(e_outer_html_set));
    acc(realm, "clientWidth", e_client_width, None);
    acc(realm, "clientHeight", e_client_height, None);
    acc(realm, "clientTop", e_zero, None);
    acc(realm, "clientLeft", e_zero, None);
    acc(realm, "scrollWidth", e_scroll_width, None);
    acc(realm, "scrollHeight", e_scroll_height, None);
    acc(realm, "scrollTop", e_scroll_top_get, Some(e_scroll_top_set));
    acc(realm, "scrollLeft", e_scroll_left_get, Some(e_scroll_left_set));
    reflect(realm, el, "slot", "slot", Reflect::Str);
    let m = |realm: &mut Realm, name: &str, arity: u32, f: rabbithole_js::NativeFn| {
        realm.define_method(el, name, arity, f);
    };
    m(realm, "getAttribute", 1, e_get_attribute);
    m(realm, "getAttributeNS", 2, e_get_attribute_ns);
    m(realm, "setAttribute", 2, e_set_attribute);
    m(realm, "setAttributeNS", 3, e_set_attribute_ns);
    m(realm, "removeAttribute", 1, e_remove_attribute);
    m(realm, "removeAttributeNS", 2, e_remove_attribute_ns);
    m(realm, "hasAttribute", 1, e_has_attribute);
    m(realm, "hasAttributeNS", 2, e_has_attribute_ns);
    m(realm, "hasAttributes", 0, e_has_attributes);
    m(realm, "getAttributeNames", 0, e_get_attribute_names);
    m(realm, "toggleAttribute", 1, e_toggle_attribute);
    m(realm, "getAttributeNode", 1, e_get_attribute_node);
    m(realm, "setAttributeNode", 1, e_set_attribute_node);
    m(realm, "matches", 1, e_matches);
    m(realm, "webkitMatchesSelector", 1, e_matches);
    m(realm, "msMatchesSelector", 1, e_matches);
    m(realm, "closest", 1, e_closest);
    m(realm, "insertAdjacentHTML", 2, e_insert_adjacent_html);
    m(realm, "getHTML", 0, get_html);
    m(realm, "insertAdjacentElement", 2, e_insert_adjacent_element);
    m(realm, "insertAdjacentText", 2, e_insert_adjacent_text);
    m(realm, "getBoundingClientRect", 0, e_get_bounding_client_rect);
    m(realm, "getClientRects", 0, e_get_client_rects);
    m(realm, "scrollIntoView", 1, e_scroll_into_view);
    m(realm, "scrollIntoViewIfNeeded", 1, e_scroll_into_view);
    m(realm, "scrollTo", 2, e_scroll_to);
    m(realm, "scroll", 2, e_scroll_to);
    m(realm, "scrollBy", 2, e_scroll_by);
    m(realm, "animate", 2, e_animate);
    m(realm, "getAnimations", 0, e_get_animations);
    m(realm, "requestFullscreen", 0, e_rejected_promise);
    m(realm, "setPointerCapture", 1, e_noop);
    m(realm, "releasePointerCapture", 1, e_noop);
    m(realm, "hasPointerCapture", 1, node_false);
    m(realm, "checkVisibility", 0, node_true);
    for (prop, attr) in [
        ("role", "role"),
        ("ariaAtomic", "aria-atomic"),
        ("ariaAutoComplete", "aria-autocomplete"),
        ("ariaBrailleLabel", "aria-braillelabel"),
        ("ariaBrailleRoleDescription", "aria-brailleroledescription"),
        ("ariaBusy", "aria-busy"),
        ("ariaChecked", "aria-checked"),
        ("ariaColCount", "aria-colcount"),
        ("ariaColIndex", "aria-colindex"),
        ("ariaColIndexText", "aria-colindextext"),
        ("ariaColSpan", "aria-colspan"),
        ("ariaCurrent", "aria-current"),
        ("ariaDescription", "aria-description"),
        ("ariaDisabled", "aria-disabled"),
        ("ariaExpanded", "aria-expanded"),
        ("ariaHasPopup", "aria-haspopup"),
        ("ariaHidden", "aria-hidden"),
        ("ariaInvalid", "aria-invalid"),
        ("ariaKeyShortcuts", "aria-keyshortcuts"),
        ("ariaLabel", "aria-label"),
        ("ariaLevel", "aria-level"),
        ("ariaLive", "aria-live"),
        ("ariaModal", "aria-modal"),
        ("ariaMultiLine", "aria-multiline"),
        ("ariaMultiSelectable", "aria-multiselectable"),
        ("ariaOrientation", "aria-orientation"),
        ("ariaPlaceholder", "aria-placeholder"),
        ("ariaPosInSet", "aria-posinset"),
        ("ariaPressed", "aria-pressed"),
        ("ariaReadOnly", "aria-readonly"),
        ("ariaRelevant", "aria-relevant"),
        ("ariaRequired", "aria-required"),
        ("ariaRoleDescription", "aria-roledescription"),
        ("ariaRowCount", "aria-rowcount"),
        ("ariaRowIndex", "aria-rowindex"),
        ("ariaRowIndexText", "aria-rowindextext"),
        ("ariaRowSpan", "aria-rowspan"),
        ("ariaSelected", "aria-selected"),
        ("ariaSetSize", "aria-setsize"),
        ("ariaSort", "aria-sort"),
        ("ariaValueMax", "aria-valuemax"),
        ("ariaValueMin", "aria-valuemin"),
        ("ariaValueNow", "aria-valuenow"),
        ("ariaValueText", "aria-valuetext"),
    ] {
        reflect(realm, el, prop, attr, Reflect::NullableStr);
    }

    let html = define_class(realm, "HTMLElement", Some("Element"), crate::host::html_element_ctor);
    let hacc = |realm: &mut Realm, name: &str, g: rabbithole_js::NativeFn, s: Option<rabbithole_js::NativeFn>| realm.define_accessor(html, name, Some(g), s, ACC);
    hacc(realm, "innerText", e_inner_text_get, Some(e_inner_text_set));
    hacc(realm, "outerText", e_inner_text_get, Some(e_outer_text_set));
    hacc(realm, "dataset", e_dataset, None);
    hacc(realm, "style", e_style, Some(e_style_set));
    hacc(realm, "hidden", e_hidden_get, Some(e_hidden_set));
    hacc(realm, "tabIndex", e_tab_index_get, Some(e_tab_index_set));
    hacc(realm, "contentEditable", e_content_editable_get, Some(e_content_editable_set));
    hacc(realm, "isContentEditable", e_is_content_editable, None);
    hacc(realm, "offsetWidth", e_offset_width, None);
    hacc(realm, "offsetHeight", e_offset_height, None);
    hacc(realm, "offsetTop", e_offset_top, None);
    hacc(realm, "offsetLeft", e_offset_left, None);
    hacc(realm, "offsetParent", e_offset_parent, None);
    for (prop, attr) in [("title", "title"), ("lang", "lang"), ("accessKey", "accesskey"), ("autocapitalize", "autocapitalize")] {
        reflect(realm, html, prop, attr, Reflect::Str);
    }
    reflect(realm, html, "popover", "popover", Reflect::Popover);
    reflect(realm, html, "draggable", "draggable", Reflect::Bool);
    reflect(realm, html, "spellcheck", "spellcheck", Reflect::Bool);
    reflect(realm, html, "inert", "inert", Reflect::Bool);
    reflect(realm, html, "translate", "translate", Reflect::Bool);
    realm.define_method(html, "focus", 0, e_focus);
    realm.define_method(html, "blur", 0, e_blur);
    realm.define_method(html, "click", 0, e_click);
    // Dispatches like the host's own clicks (see `dispatchEvent`).
    realm.mark_gc_safe(html, "click");
    realm.define_method(html, "showPopover", 0, e_noop);
    realm.define_method(html, "hidePopover", 0, e_noop);
    realm.define_method(html, "togglePopover", 0, e_noop);

    for name in [
        "HTMLDivElement", "HTMLSpanElement", "HTMLParagraphElement", "HTMLUListElement", "HTMLOListElement", "HTMLLIElement", "HTMLBodyElement", "HTMLHtmlElement", "HTMLHeadElement", "HTMLTableElement", "HTMLTableRowElement", "HTMLTableCellElement", "HTMLTableSectionElement", "HTMLHeadingElement", "HTMLBRElement", "HTMLHRElement", "HTMLPreElement", "HTMLMetaElement", "HTMLTitleElement", "HTMLDListElement", "HTMLFieldSetElement", "HTMLLegendElement", "HTMLDetailsElement", "HTMLDialogElement", "HTMLSourceElement", "HTMLPictureElement", "HTMLObjectElement", "HTMLBaseElement", "HTMLAreaElement", "HTMLProgressElement", "HTMLTimeElement", "HTMLDataElement", "HTMLSlotElement", "HTMLMenuElement", "HTMLQuoteElement", "HTMLFontElement", "HTMLFrameElement", "HTMLModElement", "HTMLTableCaptionElement", "HTMLTableColElement", "HTMLOptGroupElement", "HTMLDataListElement", "HTMLTrackElement", "HTMLParamElement", "HTMLMarqueeElement", "HTMLUnknownElement",
        "HTMLEmbedElement", "HTMLFrameSetElement", "HTMLMapElement", "HTMLMeterElement", "HTMLOutputElement", "HTMLDirectoryElement", "HTMLSelectedContentElement",
    ] {
        define_class(realm, name, Some("HTMLElement"), crate::host::html_element_ctor);
    }
    // Reflected attributes of common elements.
    let meta = proto("HTMLMetaElement");
    reflect(realm, meta, "name", "name", Reflect::Str);
    reflect(realm, meta, "content", "content", Reflect::Str);
    reflect(realm, meta, "httpEquiv", "http-equiv", Reflect::Str);
    reflect(realm, meta, "media", "media", Reflect::Str);
    let base = proto("HTMLBaseElement");
    reflect(realm, base, "href", "href", Reflect::Url);
    reflect(realm, base, "target", "target", Reflect::Str);
    let object = proto("HTMLObjectElement");
    reflect(realm, object, "data", "data", Reflect::Url);
    reflect(realm, object, "type", "type", Reflect::Str);
    reflect(realm, object, "name", "name", Reflect::Str);
    reflect(realm, object, "useMap", "usemap", Reflect::Str);
    let source = proto("HTMLSourceElement");
    reflect(realm, source, "src", "src", Reflect::Url);
    reflect(realm, source, "srcset", "srcset", Reflect::Str);
    reflect(realm, source, "sizes", "sizes", Reflect::Str);
    reflect(realm, source, "media", "media", Reflect::Str);
    reflect(realm, source, "type", "type", Reflect::Str);
    let time = proto("HTMLTimeElement");
    reflect(realm, time, "dateTime", "datetime", Reflect::Str);
    let data = proto("HTMLDataElement");
    reflect(realm, data, "value", "value", Reflect::Str);
    let cell = proto("HTMLTableCellElement");
    reflect(realm, cell, "headers", "headers", Reflect::Str);
    let ol = proto("HTMLOListElement");
    reflect(realm, ol, "reversed", "reversed", Reflect::Bool);
    let quote = proto("HTMLQuoteElement");
    reflect(realm, quote, "cite", "cite", Reflect::Url);
    let details = proto("HTMLDetailsElement");
    reflect(realm, details, "open", "open", Reflect::Bool);
    let dialog = proto("HTMLDialogElement");
    reflect(realm, dialog, "open", "open", Reflect::Bool);
    realm.define_method(dialog, "show", 0, dialog_show);
    realm.define_method(dialog, "showModal", 0, dialog_show_modal);
    realm.define_method(dialog, "close", 0, dialog_close);
    let tsec = proto("HTMLTableSectionElement");
    realm.define_method(tsec, "insertRow", 1, table_insert_row);
    let table = proto("HTMLTableElement");
    realm.define_method(table, "insertRow", 1, table_insert_row);
    realm.define_accessor(table, "rows", Some(table_rows), None, ACC);
    realm.define_accessor(table, "tBodies", Some(table_bodies), None, ACC);
    let trow = proto("HTMLTableRowElement");
    realm.define_method(trow, "insertCell", 1, row_insert_cell);
    realm.define_accessor(trow, "cells", Some(n_children_alias), None, ACC);
    realm.define_accessor(trow, "rowIndex", Some(row_index), None, ACC);
    let base = proto("HTMLBaseElement");
    reflect(realm, base, "href", "href", Reflect::Url);
    let source = proto("HTMLSourceElement");
    reflect(realm, source, "src", "src", Reflect::Url);
    reflect(realm, source, "srcset", "srcset", Reflect::Str);
    reflect(realm, source, "type", "type", Reflect::Str);
    reflect(realm, source, "media", "media", Reflect::Str);
    let time = proto("HTMLTimeElement");
    reflect(realm, time, "dateTime", "datetime", Reflect::Str);
    let progress = proto("HTMLProgressElement");
    reflect(realm, progress, "value", "value", Reflect::Double(0.0));
    let area = proto("HTMLAreaElement");
    realm.define_accessor(area, "href", Some(a_href_get), Some(a_href_set), ACC);

    // --- HTMLAnchorElement
    let a = define_class(realm, "HTMLAnchorElement", Some("HTMLElement"), crate::host::html_element_ctor);
    realm.define_accessor(a, "href", Some(a_href_get), Some(a_href_set), ACC);
    for (name, g) in [("protocol", a_protocol as rabbithole_js::NativeFn), ("host", a_host), ("hostname", a_hostname), ("port", a_port), ("pathname", a_pathname), ("search", a_search), ("hash", a_hash), ("origin", a_origin)] {
        realm.define_accessor(a, name, Some(g), None, ACC);
    }
    realm.define_method(a, "toString", 0, a_to_string);
    for (p, at) in [("target", "target"), ("rel", "rel"), ("download", "download"), ("hreflang", "hreflang"), ("type", "type"), ("ping", "ping"), ("name", "name")] {
        reflect(realm, a, p, at, Reflect::Str);
    }
    realm.define_accessor(a, "relList", Some(e_rel_list), Some(e_rel_list_set), ACC);
    realm.define_accessor(a, "text", Some(a_text_get), Some(n_text_set_alias), ACC);

    // --- HTMLInputElement
    let input = define_class(realm, "HTMLInputElement", Some("HTMLElement"), crate::host::html_element_ctor);
    realm.define_accessor(input, "popoverTargetElement", Some(popover_target_get), Some(popover_target_set), ACC);
    realm.define_accessor(input, "value", Some(input_value_get), Some(input_value_set), ACC);
    realm.define_accessor(input, "defaultValue", Some(input_default_value_get), Some(input_default_value_set), ACC);
    realm.define_accessor(input, "checked", Some(input_checked_get), Some(input_checked_set), ACC);
    reflect(realm, input, "defaultChecked", "checked", Reflect::Bool);
    realm.define_accessor(input, "type", Some(input_type_get), Some(input_type_set), ACC);
    realm.define_accessor(input, "form", Some(input_form), None, ACC);
    realm.define_accessor(input, "labels", Some(input_labels), None, ACC);
    realm.define_accessor(input, "files", Some(input_files), None, ACC);
    realm.define_accessor(input, "validity", Some(input_validity), None, ACC);
    realm.define_accessor(input, "validationMessage", Some(empty_string), None, ACC);
    realm.define_accessor(input, "willValidate", Some(node_true), None, ACC);
    realm.define_accessor(input, "selectionStart", Some(input_selection_start), Some(input_selection_start_set), ACC);
    realm.define_accessor(input, "selectionEnd", Some(input_selection_end), Some(input_selection_end_set), ACC);
    realm.define_accessor(input, "selectionDirection", Some(input_selection_direction), Some(input_selection_direction_set), ACC);
    realm.define_accessor(input, "valueAsNumber", Some(input_value_as_number), Some(input_value_set), ACC);
    realm.define_accessor(input, "list", Some(node_null), None, ACC);
    realm.define_method(input, "select", 0, input_select);
    realm.define_method(input, "setSelectionRange", 2, input_set_selection_range);
    realm.define_method(input, "setRangeText", 1, e_noop);
    realm.define_method(input, "checkValidity", 0, input_check_validity);
    realm.define_method(input, "reportValidity", 0, input_check_validity);
    realm.define_method(input, "setCustomValidity", 1, input_set_custom_validity);
    realm.define_method(input, "stepUp", 0, e_noop);
    realm.define_method(input, "stepDown", 0, e_noop);
    realm.define_method(input, "showPicker", 0, e_noop);
    for (p, at) in [("name", "name"), ("placeholder", "placeholder"), ("min", "min"), ("max", "max"), ("step", "step"), ("pattern", "pattern"), ("accept", "accept"), ("autocomplete", "autocomplete"), ("alt", "alt"), ("dirName", "dirname")] {
        reflect(realm, input, p, at, Reflect::Str);
    }
    for (p, at) in [("disabled", "disabled"), ("required", "required"), ("readOnly", "readonly"), ("multiple", "multiple"), ("autofocus", "autofocus"), ("indeterminate", "data-rh-indeterminate")] {
        reflect(realm, input, p, at, Reflect::Bool);
    }

    // --- HTMLTextAreaElement
    let ta = define_class(realm, "HTMLTextAreaElement", Some("HTMLElement"), crate::host::html_element_ctor);
    realm.define_accessor(ta, "value", Some(input_value_get), Some(input_value_set), ACC);
    realm.define_accessor(ta, "defaultValue", Some(input_default_value_get), Some(n_text_set_alias), ACC);
    realm.define_accessor(ta, "type", Some(input_type_get), None, ACC);
    realm.define_accessor(ta, "form", Some(input_form), None, ACC);
    realm.define_accessor(ta, "labels", Some(input_labels), None, ACC);
    realm.define_accessor(ta, "validity", Some(input_validity), None, ACC);
    realm.define_accessor(ta, "selectionStart", Some(input_selection_start), Some(input_selection_start_set), ACC);
    realm.define_accessor(ta, "selectionEnd", Some(input_selection_end), Some(input_selection_end_set), ACC);
    realm.define_accessor(ta, "selectionDirection", Some(input_selection_direction), Some(input_selection_direction_set), ACC);
    realm.define_accessor(ta, "textLength", Some(ta_text_length), None, ACC);
    realm.define_method(ta, "select", 0, input_select);
    realm.define_method(ta, "setSelectionRange", 2, input_set_selection_range);
    realm.define_method(ta, "checkValidity", 0, input_check_validity);
    realm.define_method(ta, "reportValidity", 0, input_check_validity);
    realm.define_method(ta, "setCustomValidity", 1, input_set_custom_validity);
    for (p, at) in [("name", "name"), ("placeholder", "placeholder"), ("wrap", "wrap"), ("autocomplete", "autocomplete")] {
        reflect(realm, ta, p, at, Reflect::Str);
    }
    for (p, at) in [("disabled", "disabled"), ("required", "required"), ("readOnly", "readonly"), ("autofocus", "autofocus")] {
        reflect(realm, ta, p, at, Reflect::Bool);
    }

    // --- HTMLSelectElement / HTMLOptionElement
    let sel = define_class(realm, "HTMLSelectElement", Some("HTMLElement"), crate::host::html_element_ctor);
    realm.define_accessor(sel, "value", Some(input_value_get), Some(input_value_set), ACC);
    realm.define_accessor(sel, "type", Some(input_type_get), None, ACC);
    realm.define_accessor(sel, "options", Some(select_options), None, ACC);
    realm.define_accessor(sel, "selectedOptions", Some(select_selected_options), None, ACC);
    realm.define_accessor(sel, "selectedIndex", Some(select_selected_index_get), Some(select_selected_index_set), ACC);
    realm.define_accessor(sel, "length", Some(select_length), None, ACC);
    realm.define_accessor(sel, "form", Some(input_form), None, ACC);
    realm.define_accessor(sel, "labels", Some(input_labels), None, ACC);
    realm.define_accessor(sel, "validity", Some(input_validity), None, ACC);
    realm.define_method(sel, "add", 2, select_add);
    realm.define_method(sel, "remove", 1, select_remove);
    realm.define_method(sel, "item", 1, select_item);
    realm.define_method(sel, "namedItem", 1, select_item);
    realm.define_method(sel, "checkValidity", 0, node_true);
    realm.define_method(sel, "reportValidity", 0, node_true);
    realm.define_method(sel, "setCustomValidity", 1, e_noop);
    for (p, at) in [("name", "name"), ("autocomplete", "autocomplete")] {
        reflect(realm, sel, p, at, Reflect::Str);
    }
    for (p, at) in [("disabled", "disabled"), ("required", "required"), ("multiple", "multiple"), ("autofocus", "autofocus")] {
        reflect(realm, sel, p, at, Reflect::Bool);
    }
    let opt = define_class(realm, "HTMLOptionElement", Some("HTMLElement"), option_ctor);
    realm.define_accessor(opt, "value", Some(option_value_get), Some(input_value_set), ACC);
    realm.define_accessor(opt, "text", Some(option_text_get), Some(option_text_set), ACC);
    realm.define_accessor(opt, "label", Some(option_label_get), Some(option_label_set), ACC);
    realm.define_accessor(opt, "selected", Some(option_selected_get), Some(option_selected_set), ACC);
    realm.define_accessor(opt, "defaultSelected", Some(option_default_selected_get), Some(option_default_selected_set), ACC);
    realm.define_accessor(opt, "index", Some(option_index), None, ACC);
    realm.define_accessor(opt, "form", Some(input_form), None, ACC);
    reflect(realm, opt, "disabled", "disabled", Reflect::Bool);
    let opt_ctor = realm.obj(opt).props.get(&PropertyKey::str("constructor")).map(|p| p.value.clone()).unwrap_or(Value::Undefined);
    realm.define_global("Option", opt_ctor);

    // --- HTMLFormElement
    let form = define_class(realm, "HTMLFormElement", Some("HTMLElement"), crate::host::html_element_ctor);
    realm.define_accessor(form, "elements", Some(form_elements), None, ACC);
    realm.define_accessor(form, "length", Some(form_length), None, ACC);
    realm.define_accessor(form, "action", Some(form_action_get), Some(form_action_set), ACC);
    realm.define_method(form, "submit", 0, form_submit);
    realm.define_method(form, "requestSubmit", 1, form_request_submit);
    realm.define_method(form, "reset", 0, form_reset);
    realm.define_method(form, "checkValidity", 0, form_check_validity);
    realm.define_method(form, "reportValidity", 0, form_check_validity);
    for (p, at) in [("name", "name"), ("target", "target"), ("acceptCharset", "accept-charset")] {
        reflect(realm, form, p, at, Reflect::Str);
    }
    reflect(realm, form, "noValidate", "novalidate", Reflect::Bool);

    // --- HTMLImageElement
    let img = define_class(realm, "HTMLImageElement", Some("HTMLElement"), image_ctor);
    realm.define_accessor(img, "src", Some(img_src_get), Some(img_src_set), ACC);
    realm.define_accessor(img, "currentSrc", Some(img_current_src), None, ACC);
    realm.define_accessor(img, "srcset", Some(img_srcset_get), Some(img_srcset_set), ACC);
    realm.define_accessor(img, "naturalWidth", Some(img_natural_width), None, ACC);
    realm.define_accessor(img, "naturalHeight", Some(img_natural_height), None, ACC);
    realm.define_accessor(img, "complete", Some(img_complete), None, ACC);
    realm.define_accessor(img, "width", Some(e_offset_width), Some(img_width_set), ACC);
    realm.define_accessor(img, "height", Some(e_offset_height), Some(img_height_set), ACC);
    realm.define_method(img, "decode", 0, img_decode);
    for (p, at) in [("alt", "alt"), ("sizes", "sizes"), ("useMap", "usemap")] {
        reflect(realm, img, p, at, Reflect::Str);
    }
    reflect(realm, img, "isMap", "ismap", Reflect::Bool);
    let img_ctor = realm.obj(img).props.get(&PropertyKey::str("constructor")).map(|p| p.value.clone()).unwrap_or(Value::Undefined);
    realm.define_global("Image", img_ctor);

    // --- HTMLScriptElement
    let script = define_class(realm, "HTMLScriptElement", Some("HTMLElement"), crate::host::html_element_ctor);
    realm.define_accessor(script, "text", Some(script_text_get), Some(script_text_set), ACC);
    for (p, at) in [("type", "type"), ("charset", "charset"), ("integrity", "integrity")] {
        reflect(realm, script, p, at, Reflect::Str);
    }
    reflect(realm, script, "src", "src", Reflect::Url);
    for (p, at) in [("async", "async"), ("defer", "defer"), ("noModule", "nomodule")] {
        reflect(realm, script, p, at, Reflect::Bool);
    }
    // --- HTMLStyleElement / HTMLLinkElement
    let style = define_class(realm, "HTMLStyleElement", Some("HTMLElement"), crate::host::html_element_ctor);
    realm.define_accessor(style, "sheet", Some(style_sheet), None, ACC);
    reflect(realm, style, "media", "media", Reflect::Str);
    reflect(realm, style, "disabled", "disabled", Reflect::Bool);
    let link = define_class(realm, "HTMLLinkElement", Some("HTMLElement"), crate::host::html_element_ctor);
    realm.define_accessor(link, "sheet", Some(style_sheet), None, ACC);
    realm.define_accessor(link, "relList", Some(e_rel_list), Some(e_rel_list_set), ACC);
    reflect(realm, link, "href", "href", Reflect::Url);
    for (p, at) in [("rel", "rel"), ("type", "type"), ("media", "media"), ("integrity", "integrity"), ("hreflang", "hreflang"), ("imageSrcset", "imagesrcset")] {
        // (`sizes` is a DOMTokenList, reflected from the table in html.js.)
        reflect(realm, link, p, at, Reflect::Str);
    }
    reflect(realm, link, "disabled", "disabled", Reflect::Bool);
    // --- HTMLIFrameElement
    let iframe = define_class(realm, "HTMLIFrameElement", Some("HTMLElement"), crate::host::html_element_ctor);
    realm.define_accessor(iframe, "contentWindow", Some(iframe_content_window), None, ACC);
    realm.define_accessor(iframe, "contentDocument", Some(iframe_content_document), None, ACC);
    reflect(realm, iframe, "src", "src", Reflect::Url);
    // (`sandbox` is a DOMTokenList, reflected from the table in html.js.)
    for (p, at) in [("name", "name"), ("allow", "allow"), ("srcdoc", "srcdoc"), ("width", "width"), ("height", "height"), ("title", "title")] {
        reflect(realm, iframe, p, at, Reflect::Str);
    }
    reflect(realm, iframe, "allowFullscreen", "allowfullscreen", Reflect::Bool);
    let frame = { state().borrow().protos.get("HTMLFrameElement").copied() };
    if let Some(frame) = frame {
        realm.define_accessor(frame, "contentWindow", Some(iframe_content_window), None, ACC);
        realm.define_accessor(frame, "contentDocument", Some(iframe_content_document), None, ACC);
        reflect(realm, frame, "src", "src", Reflect::Url);
        for (property, attribute) in [("name","name"),("scrolling","scrolling"),("frameBorder","frameborder")] {
            reflect(realm, frame, property, attribute, Reflect::Str);
        }
        reflect(realm, frame, "noResize", "noresize", Reflect::Bool);
    }
    let frameset = { state().borrow().protos.get("HTMLFrameSetElement").copied() };
    if let Some(frameset) = frameset {
        reflect(realm, frameset, "rows", "rows", Reflect::Str);
        reflect(realm, frameset, "cols", "cols", Reflect::Str);
    }
    // --- HTMLCanvasElement
    let canvas = define_class(realm, "HTMLCanvasElement", Some("HTMLElement"), crate::host::html_element_ctor);
    realm.define_method(canvas, "getContext", 1, canvas_get_context);
    realm.define_method(canvas, "toDataURL", 0, canvas_to_data_url);
    realm.define_method(canvas, "toBlob", 1, e_noop);
    realm.define_method(canvas, "captureStream", 0, node_null);
    // --- HTMLButtonElement
    let button = define_class(realm, "HTMLButtonElement", Some("HTMLElement"), crate::host::html_element_ctor);
    realm.define_accessor(button, "popoverTargetElement", Some(popover_target_get), Some(popover_target_set), ACC);
    realm.define_accessor(button, "type", Some(input_type_get), Some(input_type_set), ACC);
    reflect(realm, button, "value", "value", Reflect::Str);
    realm.define_accessor(button, "form", Some(input_form), None, ACC);
    realm.define_accessor(button, "labels", Some(input_labels), None, ACC);
    realm.define_accessor(button, "validity", Some(input_validity), None, ACC);
    realm.define_method(button, "checkValidity", 0, node_true);
    realm.define_method(button, "reportValidity", 0, node_true);
    realm.define_method(button, "setCustomValidity", 1, e_noop);
    for (p, at) in [("name", "name"), ("formTarget", "formtarget")] {
        reflect(realm, button, p, at, Reflect::Str);
    }
    for (p, at) in [("disabled", "disabled"), ("autofocus", "autofocus"), ("formNoValidate", "formnovalidate")] {
        reflect(realm, button, p, at, Reflect::Bool);
    }
    // --- HTMLLabelElement
    let label = define_class(realm, "HTMLLabelElement", Some("HTMLElement"), crate::host::html_element_ctor);
    realm.define_accessor(label, "control", Some(label_control), None, ACC);
    realm.define_accessor(label, "form", Some(input_form), None, ACC);
    reflect(realm, label, "htmlFor", "for", Reflect::Str);
    // --- HTMLTemplateElement
    let template = define_class(realm, "HTMLTemplateElement", Some("HTMLElement"), crate::host::html_element_ctor);
    realm.define_accessor(template, "content", Some(template_content), None, ACC);
    // --- HTMLMediaElement
    let media = define_class(realm, "HTMLMediaElement", Some("HTMLElement"), crate::host::html_element_ctor);
    realm.define_method(media, "play", 0, media_play);
    realm.define_method(media, "pause", 0, media_pause);
    realm.define_method(media, "load", 0, media_load);
    realm.define_method(media, "fastSeek", 1, e_noop);
    realm.define_method(media, "canPlayType", 1, media_can_play_type);
    realm.define_method(media, "addTextTrack", 1, node_null);
    reflect(realm, media, "src", "src", Reflect::Url);
    reflect(realm, media, "currentSrc", "src", Reflect::Url);
    reflect(realm, media, "poster", "poster", Reflect::Url);
    for (p, at) in [("autoplay", "autoplay"), ("controls", "controls"), ("loop", "loop"), ("muted", "muted"), ("playsInline", "playsinline")] {
        reflect(realm, media, p, at, Reflect::Bool);
    }
    for (p, v) in [("paused", Value::Bool(true)), ("ended", Value::Bool(false)), ("duration", Value::Number(f64::NAN)), ("currentTime", Value::Number(0.0)), ("volume", Value::Number(1.0)), ("playbackRate", Value::Number(1.0)), ("readyState", Value::Number(0.0)), ("networkState", Value::Number(0.0)), ("seeking", Value::Bool(false)), ("videoWidth", Value::Number(0.0)), ("videoHeight", Value::Number(0.0)), ("error", Value::Null)] {
        realm.define_property(media, p, v, rabbithole_js::HIDDEN | ENUMERABLE);
    }
    let _ = media;
    define_class(realm, "HTMLVideoElement", Some("HTMLMediaElement"), crate::host::html_element_ctor);
    define_class(realm, "HTMLAudioElement", Some("HTMLMediaElement"), crate::host::html_element_ctor);
    // --- SVGElement
    // MathML elements (MathML Core): GlobalEventHandlers (html.js),
    // HTMLOrSVGElement and ElementCSSInlineStyle.
    let mathml = define_class(realm, "MathMLElement", Some("Element"), illegal_ctor);
    realm.define_accessor(mathml, "tabIndex", Some(e_tab_index_get), Some(e_tab_index_set), ACC);
    realm.define_accessor(mathml, "style", Some(e_style), Some(e_style_set), ACC);
    realm.define_accessor(mathml, "dataset", Some(e_dataset), None, ACC);
    realm.define_method(mathml, "focus", 0, e_focus);
    realm.define_method(mathml, "blur", 0, e_blur);
    // MathML <a> (MathML Core): href, target and HyperlinkElementUtils.
    let ma = define_class(realm, "MathMLAnchorElement", Some("MathMLElement"), illegal_ctor);
    realm.define_accessor(ma, "href", Some(a_href_get), Some(a_href_set), ACC);
    for (name, g) in [("protocol", a_protocol as rabbithole_js::NativeFn), ("host", a_host), ("hostname", a_hostname), ("port", a_port), ("pathname", a_pathname), ("search", a_search), ("hash", a_hash), ("origin", a_origin)] {
        realm.define_accessor(ma, name, Some(g), None, ACC);
    }
    reflect(realm, ma, "target", "target", Reflect::Str);
    reflect(realm, ma, "rel", "rel", Reflect::Str);
    realm.define_accessor(ma, "relList", Some(e_rel_list), Some(e_rel_list_set), ACC);
    realm.define_method(ma, "toString", 0, a_to_string);
    let svg = define_class(realm, "SVGElement", Some("Element"), illegal_ctor);
    // GlobalEventHandlers: on HTML, SVG and MathML elements (not Element).
    for p in [html, mathml, svg] {
        crate::events::install_handler_props(realm, p, EVENT_HANDLER_NAMES);
    }
    realm.define_accessor(svg, "tabIndex", Some(e_tab_index_get), Some(e_tab_index_set), ACC);
    realm.define_accessor(svg, "ownerSVGElement", Some(svg_owner), None, ACC);
    realm.define_accessor(svg, "style", Some(e_style), Some(e_style_set), ACC);
    realm.define_accessor(svg, "dataset", Some(e_dataset), None, ACC);
    realm.define_method(svg, "getBBox", 0, svg_get_bbox);
    realm.define_method(svg, "getScreenCTM", 0, node_null);
    realm.define_method(svg, "getCTM", 0, node_null);
    realm.define_method(svg, "focus", 0, e_focus);
    realm.define_method(svg, "blur", 0, e_blur);
    // SVG element interfaces (SVG 2, CSS Masking, Filter Effects).
    for (name, parent) in [
        ("SVGGraphicsElement", "SVGElement"),
        ("SVGGeometryElement", "SVGGraphicsElement"),
        ("SVGPathElement", "SVGGeometryElement"),
        ("SVGRectElement", "SVGGeometryElement"),
        ("SVGCircleElement", "SVGGeometryElement"),
        ("SVGEllipseElement", "SVGGeometryElement"),
        ("SVGLineElement", "SVGGeometryElement"),
        ("SVGPolylineElement", "SVGGeometryElement"),
        ("SVGPolygonElement", "SVGGeometryElement"),
        ("SVGGElement", "SVGGraphicsElement"),
        ("SVGDefsElement", "SVGGraphicsElement"),
        ("SVGUseElement", "SVGGraphicsElement"),
        ("SVGImageElement", "SVGGraphicsElement"),
        ("SVGForeignObjectElement", "SVGGraphicsElement"),
        ("SVGSwitchElement", "SVGGraphicsElement"),
        ("SVGAElement", "SVGGraphicsElement"),
        ("SVGTextContentElement", "SVGGraphicsElement"),
        ("SVGTextPositioningElement", "SVGTextContentElement"),
        ("SVGTextElement", "SVGTextPositioningElement"),
        ("SVGTSpanElement", "SVGTextPositioningElement"),
        ("SVGTextPathElement", "SVGTextContentElement"),
        ("SVGGradientElement", "SVGElement"),
        ("SVGLinearGradientElement", "SVGGradientElement"),
        ("SVGRadialGradientElement", "SVGGradientElement"),
        ("SVGStopElement", "SVGElement"),
        ("SVGDescElement", "SVGElement"),
        ("SVGTitleElement", "SVGElement"),
        ("SVGMetadataElement", "SVGElement"),
        ("SVGMarkerElement", "SVGElement"),
        ("SVGPatternElement", "SVGElement"),
        ("SVGStyleElement", "SVGElement"),
        ("SVGScriptElement", "SVGElement"),
        ("SVGSymbolElement", "SVGElement"),
        ("SVGViewElement", "SVGElement"),
        ("SVGClipPathElement", "SVGElement"),
        ("SVGMaskElement", "SVGElement"),
        ("SVGFilterElement", "SVGElement"),
    ] {
        define_class(realm, name, Some(parent), illegal_ctor);
    }
    let svg_svg = define_class(realm, "SVGSVGElement", Some("SVGGraphicsElement"), illegal_ctor);
    realm.define_method(svg_svg, "createSVGPoint", 0, svg_create_point);
    realm.define_method(svg_svg, "createSVGMatrix", 0, svg_create_point);
    realm.define_method(svg_svg, "getBBox", 0, svg_get_bbox);
    state().borrow_mut().protos.insert("SVGElement", svg);

    // Misc support classes.
    let tl = define_class(realm, "DOMTokenList", None, illegal_ctor);
    rabbithole_js::heap::set_indexed_host_hook(rabbithole_js::heap::IndexedHostHook { get: tl_index, len: tl_count });
    realm.define_method(tl, "add", 1, tl_add);
    realm.define_method(tl, "remove", 1, tl_remove);
    realm.define_method(tl, "toggle", 1, tl_toggle);
    realm.define_method(tl, "contains", 1, tl_contains);
    realm.define_method(tl, "replace", 2, tl_replace);
    realm.define_method(tl, "item", 1, tl_item);
    realm.define_method(tl, "supports", 1, tl_supports);
    realm.define_method(tl, "forEach", 1, tl_for_each);
    realm.define_method(tl, "toString", 0, tl_to_string);
    realm.define_method(tl, "entries", 0, tl_iterator);
    realm.define_method(tl, "values", 0, tl_iterator);
    realm.define_method(tl, "keys", 0, tl_iterator);
    realm.define_symbol_method(tl, rabbithole_js::realm::SYM_ITERATOR, "[Symbol.iterator]", 0, tl_iterator);
    realm.define_accessor(tl, "length", Some(tl_length), None, ACC);
    realm.define_accessor(tl, "value", Some(tl_value_get), Some(tl_value_set), ACC);
    let nnm = realm.new_object_with_proto(Some(realm.intrinsics.array_proto));
    realm.define_method(nnm, "getNamedItem", 1, nnm_get_named_item);
    realm.define_method(nnm, "item", 1, node::define_hidden_method_item);
    state().borrow_mut().protos.insert("NamedNodeMap", nnm);
    define_class(realm, "Attr", Some("Node"), illegal_ctor);
    // DOMRect / DOMRectReadOnly are defined in prelude.js; this native
    // class is only the fallback before it runs.
    let rect = define_class(realm, "DOMRect", None, dom_rect_ctor);
    realm.define_method(rect, "toJSON", 0, dom_rect_to_json);
}

/// Opening a dialog closes auto and hint popovers above its highest
/// popover ancestor. Dismiss through the normal event path, outside the
/// document borrow, because beforetoggle can synchronously modify the DOM.
fn dismiss_popovers_for_dialog(realm: &mut Realm, dialog: NodeRef) {
    let popovers: Vec<_> = with_doc(dialog.doc, |doc| {
        doc.light_dismiss_popovers().rev().take_while(|&popover| {
            !std::iter::successors(doc.flat_parent(dialog.node), |&n| doc.flat_parent(n)).any(|n| n == popover)
        }).collect()
    });
    for node in popovers {
        if with_doc(dialog.doc, |doc| doc.popover_open(node)) {
            crate::events::popover_action(realm, NodeRef { doc: dialog.doc, node }, true);
        }
    }
}

fn dialog_show(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "show")?;
    if with_doc(n.doc, |doc| doc.attr(n.node, "open").is_some()) {
        if with_doc(n.doc, |doc| doc.is_modal(n.node)) {
            return Err(crate::misc::dom_exception(realm, "InvalidStateError", "The dialog is already open as a modal dialog."));
        }
        return Ok(Value::Undefined);
    }
    set_attribute(realm, n, "open", Some(""));
    dismiss_popovers_for_dialog(realm, n);
    Ok(Value::Undefined)
}

/// `showModal()`: open the dialog in the top layer (painted above the
/// page over its `::backdrop`, `:modal`).
fn dialog_show_modal(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "showModal")?;
    let (open, modal, connected) = with_doc(n.doc, |doc| (doc.attr(n.node, "open").is_some(), doc.is_modal(n.node), doc.is_connected(n.node)));
    if with_doc(n.doc, |doc| doc.popover_open(n.node)) {
        return Err(crate::misc::dom_exception(realm, "InvalidStateError", "The dialog is showing as a popover."));
    }
    if open && modal {
        return Ok(Value::Undefined);
    }
    if open {
        return Err(crate::misc::dom_exception(realm, "InvalidStateError", "The dialog is already open as a non-modal dialog."));
    }
    if !connected {
        return Err(crate::misc::dom_exception(realm, "InvalidStateError", "The dialog is not connected."));
    }
    with_doc(n.doc, |doc| doc.top_layer.push(n.node));
    set_attribute(realm, n, "open", Some(""));
    dismiss_popovers_for_dialog(realm, n);
    Ok(Value::Undefined)
}

fn dialog_close(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "close")?;
    if with_doc(n.doc, |doc| doc.attr(n.node, "open").is_none()) {
        return Ok(Value::Undefined);
    }
    with_doc(n.doc, |doc| doc.top_layer.retain(|&t| t != n.node));
    set_attribute(realm, n, "open", None);
    crate::events::dispatch(realm, TargetKey::Node(n), "close", crate::EventInit::simple(false, false), true);
    Ok(Value::Undefined)
}

fn table_insert_row(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "insertRow")?;
    let idx = args.first().filter(|v| !v.is_undefined()).map(|v| realm.to_number(v.clone())).transpose()?.unwrap_or(-1.0);
    let tag = tag_of(n).unwrap_or_default();
    let (container, rows) = with_doc(n.doc, |doc| {
        let container = if tag == "table" {
            doc.child_elements(n.node).find(|&c| doc.is_element(c, "tbody")).unwrap_or(n.node)
        } else {
            n.node
        };
        let rows: Vec<NodeId> = doc.child_elements(container).filter(|&c| doc.is_element(c, "tr")).collect();
        (container, rows)
    });
    let tr = with_doc(n.doc, |doc| doc.create_element("tr", vec![]));
    let before = if idx < 0.0 || idx as usize >= rows.len() { None } else { Some(rows[idx as usize]) };
    insert_node(realm, NodeRef { doc: n.doc, node: container }, NodeRef { doc: n.doc, node: tr }, before)?;
    Ok(wrap(realm, NodeRef { doc: n.doc, node: tr }))
}

fn row_insert_cell(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "insertCell")?;
    let idx = args.first().filter(|v| !v.is_undefined()).map(|v| realm.to_number(v.clone())).transpose()?.unwrap_or(-1.0);
    let cells: Vec<NodeId> = with_doc(n.doc, |doc| doc.child_elements(n.node).collect());
    let td = with_doc(n.doc, |doc| doc.create_element("td", vec![]));
    let before = if idx < 0.0 || idx as usize >= cells.len() { None } else { Some(cells[idx as usize]) };
    insert_node(realm, n, NodeRef { doc: n.doc, node: td }, before)?;
    Ok(wrap(realm, NodeRef { doc: n.doc, node: td }))
}

fn table_rows(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "rows")?;
    let rows: Vec<NodeRef> = with_doc(n.doc, |doc| doc.descendants(n.node).skip(1).filter(|&d| doc.is_element(d, "tr")).map(|d| NodeRef { doc: n.doc, node: d }).collect());
    Ok(node::html_collection(realm, rows))
}

fn table_bodies(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "tBodies")?;
    let rows: Vec<NodeRef> = with_doc(n.doc, |doc| doc.child_elements(n.node).filter(|&d| doc.is_element(d, "tbody")).map(|d| NodeRef { doc: n.doc, node: d }).collect());
    Ok(node::html_collection(realm, rows))
}

fn n_children_alias(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "cells")?;
    let kids = node::element_children_of(n);
    Ok(node::html_collection(realm, kids))
}

fn row_index(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "rowIndex")?;
    let idx = with_doc(n.doc, |doc| {
        let table = doc.ancestor_or_self(n.node, |a| doc.is_element(a, "table"));
        table.and_then(|t| doc.descendants(t).filter(|&d| doc.is_element(d, "tr")).position(|d| d == n.node)).map(|i| i as f64).unwrap_or(-1.0)
    });
    Ok(Value::Number(idx))
}

fn svg_create_point(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let o = object_from(realm, vec![("x", Value::Number(0.0)), ("y", Value::Number(0.0)), ("a", Value::Number(1.0)), ("b", Value::Number(0.0)), ("c", Value::Number(0.0)), ("d", Value::Number(1.0)), ("e", Value::Number(0.0)), ("f", Value::Number(0.0))]);
    if let V::Object(id) = o.v() {
        realm.define_method(id, "matrixTransform", 1, svg_identity);
        realm.define_method(id, "inverse", 0, svg_identity);
        realm.define_method(id, "multiply", 1, svg_identity);
        realm.define_method(id, "translate", 2, svg_identity);
        realm.define_method(id, "scale", 1, svg_identity);
    }
    Ok(o)
}

fn svg_identity(_realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(this)
}

fn id_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "id")?;
    let v = arg_str(realm, args, 0)?;
    set_attribute(realm, n, "id", Some(&v));
    Ok(Value::Undefined)
}


fn node_null(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Null)
}

fn node_false(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(false))
}

fn node_true(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(true))
}

fn empty_string(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::str(""))
}

fn n_text_set_alias(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "text")?;
    let v = arg_str(realm, args, 0)?;
    set_text_content(realm, n, &v);
    Ok(Value::Undefined)
}

fn e_rejected_promise(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let (p, _, reject) = realm.new_promise();
    let err = realm.new_error("TypeError", "Not supported");
    realm.call(Value::Object(reject), Value::Undefined, &[err])?;
    Ok(Value::Object(p))
}

fn e_remove_attribute_ns(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "removeAttributeNS")?;
    let ns = ns_arg(realm, args, 0)?;
    let local = arg_str(realm, args, 1)?;
    if let Some((k, _)) = find_attr_ns(n, ns.as_deref(), &local) {
        let old = with_doc(n.doc, |doc| {
            let old = doc.attr(n.node, &k).map(|s| s.to_string());
            let lower = doc.is_html;
            doc.is_html = false;
            doc.set_attr(n.node, &k, None);
            doc.is_html = lower;
            old
        });
        crate::shadow::on_attribute(n, &k, old.clone(), None);
        node::record_attribute(realm, n, &k, old);
        if n.doc == 0 {
            mark_dirty();
        }
    }
    Ok(Value::Undefined)
}

fn e_has_attribute_ns(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "hasAttributeNS")?;
    let ns = ns_arg(realm, args, 0)?;
    let local = arg_str(realm, args, 1)?;
    Ok(Value::Bool(find_attr_ns(n, ns.as_deref(), &local).is_some()))
}

fn popover_target_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "popoverTargetElement")?;
    let target = with_doc(n.doc, |doc| doc.popover_target(n.node));
    Ok(target.map(|node| wrap(realm, NodeRef { doc: n.doc, node })).unwrap_or(Value::Null))
}

fn popover_target_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "popoverTargetElement")?;
    let value = arg(args, 0);
    if value.is_nullish() {
        set_attribute(realm, n, "popovertarget", None);
    } else {
        let target = node_of(realm, &value).filter(|t| with_doc(t.doc, |doc| doc.element(t.node).is_some()))
            .ok_or_else(|| realm.throw_type_error("Expected an Element"))?;
        set_attribute(realm, n, "popovertarget", Some(""));
        if n.doc == target.doc { with_doc(n.doc, |doc| doc.explicit_popover_targets.insert(n.node, target.node)); }
    }
    Ok(Value::Undefined)
}

pub(crate) fn popover_state(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &arg(args, 0), "popover")?;
    if let Some(value) = args.get(1) {
        let open = realm.to_boolean(value);
        let source = args.get(2).and_then(|v| node_of(realm, v)).filter(|s| s.doc == n.doc).map(|s| s.node);
        with_doc(n.doc, |doc| {
            if !open || doc.is_connected(n.node) && !doc.is_modal(n.node)
                && doc.element(n.node).is_some_and(|el| el.ns == rabbithole_dom::Namespace::Html && el.attr("popover").is_some()) {
                doc.set_popover_open_from(n.node, open, source);
            }
        });
        if n.doc == 0 { mark_dirty(); }
    }
    Ok(Value::Bool(with_doc(n.doc, |doc| doc.popover_open(n.node))))
}

pub(crate) fn open_popovers(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &arg(args, 0), "popover")?;
    let nodes: Vec<_> = with_doc(n.doc, |doc| doc.top_layer.iter().copied().filter(|&id| doc.popover_open(id)).collect());
    let values = nodes.into_iter().map(|node| wrap(realm, NodeRef { doc: n.doc, node })).collect();
    Ok(realm.new_array_value(values))
}

/// `__rhNative.attrNamespace(element, storedName)`: the attribute's
/// namespace URI or null.
pub(crate) fn attr_namespace_native(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(n) = node_of(realm, &arg(args, 0)) else { return Ok(Value::Null) };
    let name = arg_str(realm, args, 1)?;
    Ok(with_doc(n.doc, |doc| doc.attr_namespace(n.node, &name).map(Value::str)).unwrap_or(Value::Null))
}

fn e_get_attribute_node(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "getAttributeNode")?;
    let name = arg_str(realm, args, 0)?.to_ascii_lowercase();
    match get_attribute(n, &name) {
        Some(v) => {
            let a = object_from(realm, vec![("name", Value::str(&name)), ("value", Value::str(&v)), ("nodeName", Value::str(&name)), ("nodeValue", Value::str(&v)), ("specified", Value::Bool(true))]);
            if let V::Object(o) = a.v() {
                let p = proto("Attr");
                realm.obj_mut(o).set_proto(Some(p));
            }
            Ok(a)
        }
        None => Ok(Value::Null),
    }
}

fn e_set_attribute_node(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "setAttributeNode")?;
    let a = arg(args, 0);
    let name = realm.get_str(a.clone(), "name")?;
    let value = realm.get_str(a, "value")?;
    let name = realm.to_rust_string(name)?;
    let value = realm.to_rust_string(value)?;
    set_attribute(realm, n, &name, Some(&value));
    Ok(Value::Null)
}

fn input_value_as_number(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let v = input_value_get(realm, this, &[])?;
    let s = realm.to_rust_string(v)?;
    Ok(Value::Number(s.trim().parse::<f64>().unwrap_or(f64::NAN)))
}

fn ta_text_length(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let v = input_value_get(realm, this, &[])?;
    let s = realm.to_rust_string(v)?;
    Ok(Value::Number(s.encode_utf16().count() as f64))
}

fn form_action_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "action")?;
    let v = arg_str(realm, args, 0)?;
    set_attribute(realm, n, "action", Some(&v));
    Ok(Value::Undefined)
}

fn img_width_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "width")?;
    // unsigned long: out-of-range values set the default, 0.
    let v = realm.to_uint32(arg(args, 0))?;
    set_attribute(realm, n, "width", Some(&(if v > 2147483647 { 0 } else { v }).to_string()));
    Ok(Value::Undefined)
}

fn img_height_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "height")?;
    // unsigned long: out-of-range values set the default, 0.
    let v = realm.to_uint32(arg(args, 0))?;
    set_attribute(realm, n, "height", Some(&(if v > 2147483647 { 0 } else { v }).to_string()));
    Ok(Value::Undefined)
}

/// Keep `NodeData` import used (serialization helpers use it indirectly).
#[allow(dead_code)]
fn _unused(_: NodeData) {}
