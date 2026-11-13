//! `Document`: lookup, creation, `write`, `cookie`, ready state, etc.

use crate::env::with_env;
use rabbithole_js::V;
use crate::host::*;
use crate::node::{self, html_collection, insert_node};
use rabbithole_dom::{Document, Namespace, NodeData, NodeId};
use rabbithole_js::{JsResult, Realm, Value, CONFIGURABLE, ENUMERABLE};

const ACC: u8 = CONFIGURABLE | ENUMERABLE;

fn this_doc(realm: &mut Realm, this: &Value) -> JsResult<u32> {
    let n = this_node(realm, this, "Document")?;
    if n.node != Document::ROOT {
        return Err(realm.throw_type_error("Illegal invocation: not a Document"));
    }
    Ok(n.doc)
}

fn d_document_element(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    // The document's element child, whatever its name (XML documents).
    let h = with_doc(d, |doc| doc.child_elements(Document::ROOT).next());
    Ok(wrap_opt(realm, d, h))
}

fn d_head(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    let h = with_doc(d, |doc| doc.head());
    Ok(wrap_opt(realm, d, h))
}

fn d_body(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    let b = with_doc(d, |doc| doc.body());
    Ok(wrap_opt(realm, d, b))
}

fn d_body_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    let Some(new) = node_of(realm, &arg(args, 0)) else { return Ok(Value::Undefined) };
    let (old, html) = with_doc(d, |doc| (doc.body(), doc.html_element()));
    if let Some(html) = html {
        if let Some(o) = old {
            node::remove_node(NodeRef { doc: d, node: o });
        }
        insert_node(realm, NodeRef { doc: d, node: html }, new, None)?;
    }
    Ok(Value::Undefined)
}

fn d_title_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    Ok(Value::str(&with_doc(d, |doc| doc.title().unwrap_or_default())))
}

fn d_title_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    let v = arg_str(realm, args, 0)?;
    with_doc(d, |doc| {
        let title = match doc.find_element("title") {
            Some(t) => t,
            None => {
                let head = doc.head().or_else(|| doc.html_element()).unwrap_or(Document::ROOT);
                let t = doc.create_element("title", vec![]);
                doc.append_child(head, t);
                t
            }
        };
        doc.clear_children(title);
        let text = doc.create_text(&v);
        doc.append_child(title, text);
    });
    if d == 0 {
        with_env(|env| env.title_changed(&v));
    }
    Ok(Value::Undefined)
}

fn d_url(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    if d & crate::host::FRAME_DOC != 0 {
        // A same-origin frame's document: the frame's URL.
        let id = (d & !crate::host::FRAME_DOC) as u64;
        if let Some(u) = with_env(|env| env.frame_url(id)) {
            return Ok(Value::str(&u));
        }
    }
    if d != 0 {
        let own = with_doc(d, |doc| doc.url.clone());
        return Ok(Value::str(&own.unwrap_or_else(|| "about:blank".into())));
    }
    Ok(Value::str(&crate::window::current_href()))
}

fn d_referrer(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::str(""))
}

fn d_domain(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let _ = this_doc(realm, &this)?;
    Ok(Value::str(&with_env(|env| env.document_url().host)))
}

fn d_cookie_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    if d != 0 {
        return Ok(Value::str(""));
    }
    Ok(Value::str(&with_env(|env| env.cookies())))
}

fn d_cookie_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    let v = arg_str(realm, args, 0)?;
    if d == 0 {
        with_env(|env| env.set_cookie(&v));
    }
    Ok(Value::Undefined)
}

fn d_ready_state(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    if d != 0 {
        return Ok(Value::str("complete"));
    }
    Ok(Value::str(state().borrow().ready_state))
}

fn d_default_view(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    if d & crate::host::FRAME_DOC != 0 {
        // A same-origin frame's document: its window proxy.
        return crate::frames::window_for(realm, Some((d & !crate::host::FRAME_DOC) as u64));
    }
    if d != 0 {
        return Ok(Value::Null);
    }
    Ok(crate::events::window_value(realm))
}

fn d_active_element(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    let focused = state().borrow().focused;
    let n = match focused {
        // Focus inside a shadow tree is seen as its host by the document.
        Some(f) if d == 0 && with_doc(0, |doc| doc.is_connected(f)) => Some(with_doc(0, |doc| crate::shadow::retarget(doc, f, Document::ROOT))),
        _ => with_doc(d, |doc| doc.body()),
    };
    Ok(wrap_opt(realm, d, n))
}

fn d_scrolling_element(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    d_document_element(realm, this, &[])
}

fn d_current_script(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let _ = this_doc(realm, &this)?;
    let cs = state().borrow().current_script;
    Ok(match cs {
        Some(n) => wrap(realm, n),
        None => Value::Null,
    })
}

fn d_get_element_by_id(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    let id = arg_str(realm, args, 0)?;
    let n = with_doc(d, |doc| doc.element_by_id(&id));
    Ok(wrap_opt(realm, d, n))
}

fn d_get_elements_by_name(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    let name = arg_str(realm, args, 0)?;
    let found: Vec<NodeRef> = with_doc(d, |doc| doc.descendants(Document::ROOT).filter(|&n| doc.attr(n, "name") == Some(name.as_str())).map(|n| NodeRef { doc: d, node: n }).collect());
    Ok(node::node_list(realm, found))
}

fn collection_by_tags(realm: &mut Realm, this: Value, tags: &[&str], extra: impl Fn(&Document, NodeId) -> bool) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    let found: Vec<NodeRef> = with_doc(d, |doc| {
        doc.descendants(Document::ROOT)
            .filter(|&n| doc.tag(n).map(|t| tags.contains(&t)).unwrap_or(false) && extra(doc, n))
            .map(|n| NodeRef { doc: d, node: n })
            .collect()
    });
    Ok(html_collection(realm, found))
}

fn d_forms(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    collection_by_tags(realm, this, &["form"], |_, _| true)
}

fn d_images(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    collection_by_tags(realm, this, &["img"], |_, _| true)
}

fn d_links(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    collection_by_tags(realm, this, &["a", "area"], |doc, n| doc.attr(n, "href").is_some())
}

fn d_anchors(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    collection_by_tags(realm, this, &["a"], |doc, n| doc.attr(n, "name").is_some())
}

fn d_scripts(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    collection_by_tags(realm, this, &["script"], |_, _| true)
}

fn d_embeds(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    collection_by_tags(realm, this, &["embed"], |_, _| true)
}

fn d_style_sheets(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    let nodes: Vec<NodeRef> = with_doc(d, |doc| {
        doc.descendants(Document::ROOT)
            .filter(|&n| doc.is_element(n, "style") || (doc.is_element(n, "link") && doc.attr(n, "rel").map(|r| r.to_ascii_lowercase().contains("stylesheet")).unwrap_or(false)))
            .map(|n| NodeRef { doc: d, node: n })
            .collect()
    });
    let mut sheets = Vec::new();
    for n in nodes {
        let w = wrap(realm, n);
        let s = realm.get_str(w, "sheet")?;
        sheets.push(s);
    }
    Ok(realm.new_array_value(sheets))
}

/// XML `Name` production (simplified to the BMP ranges that matter).
/// DOM "valid element local name".
pub(crate) fn valid_element_local_name(s: &str) -> bool {
    let mut it = s.chars();
    let Some(first) = it.next() else { return false };
    if first.is_ascii_alphabetic() {
        return !s.chars().any(|c| matches!(c, '\0' | '\t' | '\n' | '\u{c}' | '\r' | ' ' | '/' | '>'));
    }
    if !(first == ':' || first == '_' || first as u32 >= 0x80) {
        return false;
    }
    it.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ':' | '_') || c as u32 >= 0x80)
}

/// DOM "valid attribute local name".
pub(crate) fn valid_attribute_local_name(s: &str) -> bool {
    !s.is_empty() && !s.chars().any(|c| matches!(c, '\0' | '\t' | '\n' | '\u{c}' | '\r' | ' ' | '/' | '=' | '>'))
}

/// DOM "valid namespace prefix".
fn valid_namespace_prefix(s: &str) -> bool {
    !s.is_empty() && !s.chars().any(|c| matches!(c, '\0' | '\t' | '\n' | '\u{c}' | '\r' | ' ' | '/' | '>'))
}

#[allow(dead_code)]
fn is_xml_name(s: &str) -> bool {
    let start = |c: char| c.is_ascii_alphabetic() || c == '_' || c == ':' || (c as u32 >= 0xC0 && !matches!(c as u32, 0xD7 | 0xF7 | 0x37E | 0x2000..=0x200B | 0x200E..=0x206F | 0x2190..=0x2BFF | 0x2FF0..=0x3000 | 0xD800..=0xF8FF | 0xFDD0..=0xFDEF | 0xFFFE | 0xFFFF));
    let rest = |c: char| start(c) || c.is_ascii_digit() || c == '-' || c == '.' || c as u32 == 0xB7 || (0x300..=0x36F).contains(&(c as u32)) || (0x203F..=0x2040).contains(&(c as u32));
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if start(c) => chars.all(rest),
        _ => false,
    }
}

fn d_create_element(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    let tag = arg_str(realm, args, 0)?;
    if !valid_element_local_name(&tag) {
        return Err(crate::misc::dom_exception(realm, "InvalidCharacterError", &format!("Failed to execute 'createElement' on 'Document': The tag name provided ('{tag}') is not a valid name.")));
    }
    // XML documents keep the name as given (null namespace, or HTML for
    // application/xhtml+xml documents).
    let xml = with_doc(d, |doc| (!doc.is_html).then(|| doc.content_type == "application/xhtml+xml"));
    if let Some(xhtml) = xml {
        let ns = if xhtml { Namespace::Html } else { Namespace::None };
        let node = with_doc(d, |doc| doc.create_element_ns(ns, None, &tag, vec![]));
        return Ok(wrap(realm, NodeRef { doc: d, node }));
    }
    let options = arg(args, 1);
    let is_attr = dict_str(realm, &options, "is")?;
    let node = with_doc(d, |doc| {
        let mut attrs = Vec::new();
        if let Some(isv) = is_attr {
            attrs.push(("is".to_string(), isv));
        }
        doc.create_element(&tag.to_ascii_lowercase(), attrs)
    });
    // Defined custom elements are created by running their constructor.
    if let Some(def) = crate::shadow::definition_for(NodeRef { doc: d, node }) {
        let el = crate::host::upgrade_custom_element(realm, NodeRef { doc: d, node }, &def.ctor);
        crate::shadow::flush(realm);
        return Ok(el);
    }
    Ok(wrap(realm, NodeRef { doc: d, node }))
}

/// DOM "validate and extract" (DOM §1.4): a namespace and qualified name
/// → (namespace, prefix, local name), or the DOMException to throw.
pub(crate) fn validate_and_extract(ns: Option<String>, qname: &str) -> Result<(Namespace, Option<String>, String), (&'static str, String)> {
    validate_and_extract_in(ns, qname, false)
}

/// "validate and extract" for an element (`attribute` false) or an
/// attribute qualified name.
pub(crate) fn validate_and_extract_in(ns: Option<String>, qname: &str, attribute: bool) -> Result<(Namespace, Option<String>, String), (&'static str, String)> {
    const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
    const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";
    let ns = ns.filter(|s| !s.is_empty());
    let (prefix, local) = match qname.split_once(':') {
        Some((p, l)) => {
            if !valid_namespace_prefix(p) {
                return Err(("InvalidCharacterError", format!("'{qname}' is not a valid qualified name.")));
            }
            (Some(p.to_string()), l.to_string())
        }
        None => (None, qname.to_string()),
    };
    let ok = if attribute { valid_attribute_local_name(&local) } else { valid_element_local_name(&local) };
    if !ok {
        return Err(("InvalidCharacterError", format!("'{qname}' is not a valid name.")));
    }
    let ns_err = || Err(("NamespaceError", format!("'{qname}' is not a valid qualified name for this namespace.")));
    if prefix.is_some() && ns.is_none() {
        return ns_err();
    }
    if prefix.as_deref() == Some("xml") && ns.as_deref() != Some(XML_NS) {
        return ns_err();
    }
    let is_xmlns = qname == "xmlns" || prefix.as_deref() == Some("xmlns");
    if is_xmlns != (ns.as_deref() == Some(XMLNS_NS)) {
        return ns_err();
    }
    Ok((Namespace::from_uri(ns.as_deref()), prefix, local))
}

fn d_create_element_ns(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    // `DOMString? namespace`: null and undefined are no namespace.
    let ns = match args.first().map(Value::v) {
        None | Some(V::Null) | Some(V::Undefined) => None,
        Some(v) => Some(realm.to_rust_string(v.to_value())?),
    };
    let qname = arg_str(realm, args, 1)?;
    let (ns, prefix, local) = match validate_and_extract(ns, &qname) {
        Ok(v) => v,
        Err((name, msg)) => return Err(crate::misc::dom_exception(realm, name, &msg)),
    };
    let svg = ns == Namespace::Svg;
    let node = with_doc(d, |doc| doc.create_element_ns(ns, prefix, &local, vec![]));
    let v = wrap(realm, NodeRef { doc: d, node });
    if svg {
        if let V::Object(id) = v.v() {
            let class = crate::host::svg_class(&local);
            let p = proto(class);
            realm.obj_mut(id).set_proto(Some(p));
            realm.set_class_name(id, class);
        }
    }
    Ok(v)
}

fn d_create_text_node(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    let text = arg_str(realm, args, 0)?;
    let node = with_doc(d, |doc| doc.create_text(&text));
    Ok(wrap(realm, NodeRef { doc: d, node }))
}

fn d_create_cdata_section(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    if args.is_empty() {
        return Err(realm.throw_type_error("Failed to execute 'createCDATASection' on 'Document': 1 argument required, but only 0 present."));
    }
    let text = arg_str(realm, args, 0)?;
    if with_doc(d, |doc| doc.is_html) {
        return Err(crate::misc::dom_exception(realm, "NotSupportedError", "Failed to execute 'createCDATASection' on 'Document': This operation is not supported for HTML documents."));
    }
    if text.contains("]]>") {
        return Err(crate::misc::dom_exception(realm, "InvalidCharacterError", "Failed to execute 'createCDATASection' on 'Document': String cannot contain ']]>' since that is the end delimiter of a CData section."));
    }
    let node = with_doc(d, |doc| doc.create_cdata(&text));
    Ok(wrap(realm, NodeRef { doc: d, node }))
}

fn d_create_processing_instruction(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    if args.len() < 2 {
        return Err(realm.throw_type_error(&format!("Failed to execute 'createProcessingInstruction' on 'Document': 2 arguments required, but only {} present.", args.len())));
    }
    let target = arg_str(realm, args, 0)?;
    let data = arg_str(realm, args, 1)?;
    if !is_xml_name(&target) {
        return Err(crate::misc::dom_exception(realm, "InvalidCharacterError", &format!("Failed to execute 'createProcessingInstruction' on 'Document': The target provided ('{target}') is not a valid name.")));
    }
    if data.contains("?>") {
        return Err(crate::misc::dom_exception(realm, "InvalidCharacterError", &format!("Failed to execute 'createProcessingInstruction' on 'Document': The data provided ('{data}') contains '?>'.")));
    }
    let node = with_doc(d, |doc| doc.create_pi(&target, &data));
    Ok(wrap(realm, NodeRef { doc: d, node }))
}

fn d_create_comment(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    let text = arg_str_opt(realm, args, 0)?.unwrap_or_default();
    let node = with_doc(d, |doc| doc.create(NodeData::Comment(text)));
    Ok(wrap(realm, NodeRef { doc: d, node }))
}

fn d_create_document_fragment(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    let node = with_doc(d, |doc| doc.create_element("#fragment", vec![]));
    Ok(wrap(realm, NodeRef { doc: d, node }))
}

fn d_create_event(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let _ = this_doc(realm, &this)?;
    let iface = arg_str(realm, args, 0)?;
    // DOM `createEvent()`: the legacy interface names.
    let class = match iface.to_ascii_lowercase().as_str() {
        "mouseevents" | "mouseevent" => "MouseEvent",
        "keyboardevent" => "KeyboardEvent",
        "customevent" => "CustomEvent",
        "uievents" | "uievent" => "UIEvent",
        "focusevent" => "FocusEvent",
        "compositionevent" => "CompositionEvent",
        "hashchangeevent" => "HashChangeEvent",
        "messageevent" => "MessageEvent",
        "storageevent" => "StorageEvent",
        "dragevent" => "DragEvent",
        "touchevent" => "TouchEvent",
        "textevent" => "TextEvent",
        "beforeunloadevent" => "BeforeUnloadEvent",
        "devicemotionevent" => "DeviceMotionEvent",
        "deviceorientationevent" => "DeviceOrientationEvent",
        "event" | "events" | "htmlevents" | "svgevents" => "Event",
        _ => return Err(crate::misc::dom_exception(realm, "NotSupportedError", &format!("Failed to execute 'createEvent' on 'Document': The provided event type ('{iface}') is invalid."))),
    };
    let known = matches!(class, "Event" | "MouseEvent" | "KeyboardEvent" | "CustomEvent" | "UIEvent" | "FocusEvent");
    let id = crate::events::create_event(realm, if known { class } else { "Event" }, "", &crate::EventInit::simple(false, false), false);
    if !known {
        // Interfaces defined in JS: use their prototype when present.
        let g = Value::Object(realm.global());
        let ctor = realm.get_str(g, class)?;
        if ctor.is_object() {
            let p = realm.get_str(ctor, "prototype")?;
            if let V::Object(p) = p.v() {
                realm.obj_mut(id).set_proto(Some(p));
            }
        }
    }
    if let Some(d) = realm.host_data_mut::<crate::events::EventData>(id) {
        d.initialized = false;
    }
    if class == "CustomEvent" {
        realm.define_property(id, "detail", Value::Null, ACC);
    }
    Ok(Value::Object(id))
}

fn d_create_attribute(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let _ = this_doc(realm, &this)?;
    let name = arg_str(realm, args, 0)?.to_ascii_lowercase();
    let a = object_from(realm, vec![("name", Value::str(&name)), ("value", Value::str("")), ("nodeName", Value::str(&name)), ("nodeValue", Value::str("")), ("specified", Value::Bool(true))]);
    if let V::Object(o) = a.v() {
        let p = proto("Attr");
        realm.obj_mut(o).set_proto(Some(p));
    }
    Ok(a)
}

fn d_import_node(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    let Some(n) = node_of(realm, &arg(args, 0)) else {
        return Err(realm.throw_type_error("parameter 1 is not of type 'Node'"));
    };
    let deep = realm.to_boolean(&arg(args, 1));
    let copy = if n.doc == d {
        with_doc(d, |doc| doc.clone_subtree(n.node, deep))
    } else {
        // Cross-document copy.
        let tmp = if n.doc == 0 {
            with_env(|env| {
                let src = env.doc();
                let mut t = Document::new();
                let c = t.import_subtree(src, n.node);
                t.append_child(Document::ROOT, c);
                t
            })
        } else {
            with_doc(n.doc, |src| {
                let mut t = Document::new();
                let c = t.import_subtree(src, n.node);
                t.append_child(Document::ROOT, c);
                t
            })
        };
        let root = tmp.first_child(Document::ROOT).unwrap();
        let copy = with_doc(d, |doc| doc.import_subtree(&tmp, root));
        if !deep {
            with_doc(d, |doc| doc.clear_children(copy));
        }
        copy
    };
    let copy = NodeRef { doc: d, node: copy };
    crate::shadow::queue_upgrades(copy);
    crate::shadow::drain_ce_reactions(realm);
    Ok(wrap(realm, copy))
}

fn d_adopt_node(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    if let Some(n) = node_of(realm, &arg(args, 0)) {
        if n.node == Document::ROOT {
            return Err(crate::misc::dom_exception(realm, "NotSupportedError", "The node provided is a document, which may not be adopted."));
        }
        if n.doc == d {
            node::remove_node(n);
        } else {
            node::adopt(realm, n, d);
        }
    }
    Ok(arg(args, 0))
}

fn d_has_focus(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(true))
}

fn d_exec_command(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(false))
}

fn d_query_command_supported(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(false))
}

fn d_element_from_point(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    if d != 0 {
        return Ok(Value::Null);
    }
    let x = realm.to_number(arg(args, 0))? as f32;
    let y = realm.to_number(arg(args, 1))? as f32;
    // Shadow content is retargeted to its host for the document.
    let best = hit_test(x, y).map(|n| with_doc(0, |doc| crate::shadow::retarget(doc, n, Document::ROOT)));
    Ok(wrap_opt(realm, 0, best.or_else(|| with_doc(0, |doc| doc.body()))))
}

/// The topmost rendered element at the viewport point (x, y), shadow
/// trees included: the engine's paint-order hit test, or the deepest
/// element whose border box contains the point.
pub(crate) fn hit_test(x: f32, y: f32) -> Option<NodeId> {
    if let Some(hits) = with_env(|env| {
        env.ensure_layout();
        env.elements_from_point(x, y)
    }) {
        return hits.first().copied();
    }
    let (sx, sy) = with_env(|env| env.scroll());
    let (px, py) = (x + sx, y + sy);
    // Deepest element whose rect contains the point (tree order, last wins).
    let candidates: Vec<NodeId> = with_doc(0, |doc| doc.shadow_including_descendants(Document::ROOT).into_iter().filter(|&n| doc.element(n).map(|e| !e.tag.starts_with('#')).unwrap_or(false)).collect());
    let mut best: Option<NodeId> = None;
    with_env(|env| {
        env.ensure_layout();
        for n in candidates {
            if let Some(r) = env.node_rect(n) {
                if r.contains(px, py) {
                    best = Some(n);
                }
            }
        }
    });
    best
}

fn d_elements_from_point(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let v = d_element_from_point(realm, this, args)?;
    Ok(realm.new_array_value(if v.is_null() { vec![] } else { vec![v] }))
}

/// `document.write(...)` (HTML §8.4.3): markup goes to the document's
/// script-created parser at its insertion point (see `docparser`).
fn d_write(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    let mut text = String::new();
    for a in args {
        text.push_str(&realm.to_rust_string(a.clone())?);
    }
    write_html(realm, d, &text)
}

fn d_writeln(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    let mut text = String::new();
    for a in args {
        text.push_str(&realm.to_rust_string(a.clone())?);
    }
    text.push('\n');
    write_html(realm, d, &text)
}

fn write_html(realm: &mut Realm, d: u32, html: &str) -> JsResult<Value> {
    // The page's own load-time parse: the engine parsed the whole page
    // before running its scripts, so a script's writes are inserted right
    // after it (or at the end of the body for scripts that are not
    // parser-inserted).
    let (loading, current) = {
        let st = state();
        let st = st.borrow();
        (st.loading, st.current_script)
    };
    if d == 0 && loading && !crate::docparser::has_insertion_point(0) {
        if !with_doc(0, |doc| doc.is_html) {
            return Err(crate::misc::dom_exception(realm, "InvalidStateError", "Failed to execute 'write' on 'Document': Only HTML documents support write()."));
        }
        let body = with_doc(d, |doc| doc.body());
        let Some(body) = body else { return Ok(Value::Undefined) };
        let body_ref = NodeRef { doc: d, node: body };
        let nodes = node::parse_html_into(body_ref, html);
        if let Some(cs) = current {
            let (parent, next) = with_doc(0, |doc| (doc.parent(cs.node), doc.next_sibling(cs.node)));
            if let Some(p) = parent {
                for n in nodes {
                    insert_node(realm, NodeRef::main(p), n, next)?;
                }
                return Ok(Value::Undefined);
            }
        }
        for n in nodes {
            insert_node(realm, body_ref, n, None)?;
        }
        return Ok(Value::Undefined);
    }
    // Like browsers: an external script that is not parser-inserted (async
    // or injected) cannot write into a document it did not open.
    if d == 0 && !crate::docparser::has_insertion_point(0) {
        if let Some(cs) = current {
            if with_doc(0, |doc| doc.attr(cs.node, "src").is_some()) {
                crate::host::log("warn", "Failed to execute 'write' on 'Document': It isn't possible to write into a document from an asynchronously-loaded external script unless it is explicitly opened.");
                return Ok(Value::Undefined);
            }
        }
    }
    crate::docparser::write(realm, d, html)?;
    Ok(Value::Undefined)
}

/// `document.open()` (HTML §8.4.1): empty the document and start a
/// script-created parser.
fn d_open(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    // open(url, name, features): window.open.
    if args.len() >= 3 {
        let w = realm.global();
        return realm.invoke(Value::Object(w), "open", args);
    }
    crate::docparser::open(realm, d)?;
    Ok(this)
}

/// `document.close()` (HTML §8.4.2): end the script-created parser's input
/// and parse to the end.
fn d_close(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    crate::docparser::close(realm, d)?;
    Ok(Value::Undefined)
}

fn d_create_range(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    crate::misc::new_range(realm, d)
}

fn d_create_tree_walker(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let _ = this_doc(realm, &this)?;
    crate::misc::new_tree_walker(realm, args, false)
}

fn d_create_node_iterator(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let _ = this_doc(realm, &this)?;
    crate::misc::new_tree_walker(realm, args, true)
}

fn d_get_selection(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let _ = this_doc(realm, &this)?;
    crate::misc::selection_object(realm)
}

fn d_implementation(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let _ = this_doc(realm, &this)?;
    let o = realm.new_object();
    realm.define_method(o, "createHTMLDocument", 1, impl_create_html_document);
    realm.define_method(o, "createDocument", 3, impl_create_html_document);
    realm.define_method(o, "hasFeature", 2, d_has_focus);
    realm.define_method(o, "createDocumentType", 3, impl_null);
    Ok(Value::Object(o))
}

fn impl_null(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Null)
}

/// Create an auxiliary document (DOMParser / createHTMLDocument).
pub(crate) fn new_aux_document(realm: &mut Realm, html: &str) -> Value {
    new_aux_document_from(realm, rabbithole_html::parse(html))
}

/// Register a parsed document (no browsing context) and wrap it.
pub(crate) fn new_aux_document_from(realm: &mut Realm, doc: Document) -> Value {
    let key = {
        let st = state();
        let mut st = st.borrow_mut();
        st.extra_docs.push(doc);
        st.extra_docs.len() as u32
    };
    wrap(realm, NodeRef { doc: key, node: Document::ROOT })
}

fn impl_create_html_document(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let title = arg_str_opt(realm, args, 0)?.unwrap_or_default();
    let html = format!("<!doctype html><html><head><title>{}</title></head><body></body></html>", title.replace('<', "&lt;"));
    Ok(new_aux_document(realm, &html))
}

fn d_constant_str(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let v = native_data::<String>(realm).map(|s| s.to_string()).unwrap_or_default();
    Ok(Value::str(&v))
}

fn d_false(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(false))
}

fn d_null(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Null)
}

fn d_fonts(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let (p, resolve, _) = realm.new_promise();
    realm.call(Value::Object(resolve), Value::Undefined, &[Value::Undefined])?;
    let o = object_from(realm, vec![("ready", Value::Object(p)), ("status", Value::str("loaded")), ("size", Value::Number(0.0))]);
    if let V::Object(id) = o.v() {
        realm.define_method(id, "load", 1, fonts_load);
        realm.define_method(id, "check", 1, d_true);
        realm.define_method(id, "add", 1, d_null);
        realm.define_method(id, "addEventListener", 2, d_null);
        realm.define_method(id, "forEach", 1, d_null);
    }
    Ok(o)
}

fn fonts_load(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let (p, resolve, _) = realm.new_promise();
    let arr = realm.new_array_value(vec![]);
    realm.call(Value::Object(resolve), Value::Undefined, &[arr])?;
    Ok(Value::Object(p))
}

fn d_true(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(true))
}

fn d_doctype(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    let dt = with_doc(d, |doc| doc.doctype());
    Ok(wrap_opt(realm, d, dt))
}

/// `document.location`: null for documents without a browsing context
/// (DOMParser, createDocument, createHTMLDocument).
fn d_location(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    if d != 0 && d & crate::host::FRAME_DOC == 0 {
        return Ok(Value::Null);
    }
    crate::window::location_getter(realm, this, args)
}

fn d_content_type(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    Ok(Value::str(&with_doc(d, |doc| doc.content_type.clone())))
}

fn d_character_set(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    Ok(Value::str(&with_doc(d, |doc| doc.character_set.clone())))
}

fn d_compat_mode(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    let quirks = with_doc(d, |doc| doc.quirks_mode == rabbithole_dom::QuirksMode::Quirks);
    Ok(Value::str(if quirks { "BackCompat" } else { "CSS1Compat" }))
}

/// `__rhNative.createDoctype(document, name, publicId, systemId)`: a new
/// DocumentType node owned by `document` (DOMImplementation.createDocumentType).
pub(crate) fn create_doctype(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &arg(args, 0))?;
    let name = arg_str(realm, args, 1)?;
    let public_id = arg_str_opt(realm, args, 2)?.unwrap_or_default();
    let system_id = arg_str_opt(realm, args, 3)?.unwrap_or_default();
    let node = with_doc(d, |doc| doc.create(NodeData::Doctype(rabbithole_dom::DoctypeData { name, public_id, system_id })));
    Ok(wrap(realm, NodeRef { doc: d, node }))
}

/// `__rhNative.createXMLDocument()`: a new, empty XML document
/// (DOMImplementation.createDocument).
pub(crate) fn create_xml_document(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let mut doc = Document::new();
    doc.is_html = false;
    let ns = arg_str_opt(realm, args, 0)?.unwrap_or_default();
    doc.content_type = match ns.as_str() {
        "http://www.w3.org/1999/xhtml" => "application/xhtml+xml",
        "http://www.w3.org/2000/svg" => "image/svg+xml",
        _ => "application/xml",
    }
    .to_string();
    let key = {
        let st = state();
        let mut st = st.borrow_mut();
        st.extra_docs.push(doc);
        st.extra_docs.len() as u32
    };
    Ok(wrap(realm, NodeRef { doc: key, node: Document::ROOT }))
}

/// `__rhNative.validateQName(name)`: throws the DOMException of "validate"
/// for a qualified name (InvalidCharacterError / NamespaceError).
pub(crate) fn validate_qname(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let name = arg_str(realm, args, 0)?;
    // Any namespace: only the name productions are checked.
    let ns = if name == "xmlns" || name.starts_with("xmlns:") { "http://www.w3.org/2000/xmlns/" } else if name.starts_with("xml:") { "http://www.w3.org/XML/1998/namespace" } else { "urn:x" };
    match validate_and_extract(Some(ns.to_string()), &name) {
        Ok(_) => Ok(Value::Undefined),
        Err((n, msg)) => Err(crate::misc::dom_exception(realm, n, &msg)),
    }
}

pub(crate) fn install(realm: &mut Realm) {
    let d = define_class(realm, "Document", Some("Node"), doc_ctor);
    state().borrow_mut().protos.insert("HTMLDocument", d);
    let acc = |realm: &mut Realm, name: &str, g: rabbithole_js::NativeFn, s: Option<rabbithole_js::NativeFn>| realm.define_accessor(d, name, Some(g), s, ACC);
    acc(realm, "documentElement", d_document_element, None);
    acc(realm, "head", d_head, None);
    acc(realm, "body", d_body, Some(d_body_set));
    acc(realm, "title", d_title_get, Some(d_title_set));
    acc(realm, "URL", d_url, None);
    acc(realm, "documentURI", d_url, None);
    acc(realm, "referrer", d_referrer, None);
    acc(realm, "domain", d_domain, None);
    acc(realm, "cookie", d_cookie_get, Some(d_cookie_set));
    acc(realm, "readyState", d_ready_state, None);
    acc(realm, "defaultView", d_default_view, None);
    acc(realm, "activeElement", d_active_element, None);
    acc(realm, "scrollingElement", d_scrolling_element, None);
    acc(realm, "currentScript", d_current_script, None);
    acc(realm, "forms", d_forms, None);
    acc(realm, "images", d_images, None);
    acc(realm, "links", d_links, None);
    acc(realm, "anchors", d_anchors, None);
    acc(realm, "scripts", d_scripts, None);
    acc(realm, "embeds", d_embeds, None);
    acc(realm, "plugins", d_embeds, None);
    acc(realm, "styleSheets", d_style_sheets, None);
    acc(realm, "implementation", d_implementation, None);
    acc(realm, "hidden", d_false, None);
    acc(realm, "fullscreenElement", d_null, None);
    acc(realm, "pointerLockElement", d_null, None);
    acc(realm, "pictureInPictureElement", d_null, None);
    acc(realm, "fonts", d_fonts, None);
    acc(realm, "doctype", d_doctype, None);
    acc(realm, "compatMode", d_compat_mode, None);
    acc(realm, "location", d_location, None);
    for name in ["characterSet", "charset", "inputEncoding"] {
        acc(realm, name, d_character_set, None);
    }
    acc(realm, "contentType", d_content_type, None);
    for (name, val) in [("visibilityState", "visible"), ("designMode", "off"), ("dir", "")] {
        let data: std::rc::Rc<dyn std::any::Any> = std::rc::Rc::new(val.to_string());
        let g = realm.new_native_function_full(&format!("get {name}"), 0, d_constant_str, Some(data), vec![], false);
        realm.define_accessor_values(d, name, Some(Value::Object(g)), None, ACC);
    }
    let m = |realm: &mut Realm, name: &str, arity: u32, f: rabbithole_js::NativeFn| {
        realm.define_method(d, name, arity, f);
    };
    m(realm, "getElementById", 1, d_get_element_by_id);
    m(realm, "getElementsByName", 1, d_get_elements_by_name);
    m(realm, "createElement", 1, d_create_element);
    m(realm, "createElementNS", 2, d_create_element_ns);
    m(realm, "createTextNode", 1, d_create_text_node);
    m(realm, "createComment", 1, d_create_comment);
    m(realm, "createDocumentFragment", 0, d_create_document_fragment);
    m(realm, "createEvent", 1, d_create_event);
    m(realm, "createAttribute", 1, d_create_attribute);
    m(realm, "createAttributeNS", 2, d_create_attribute);
    m(realm, "createRange", 0, d_create_range);
    m(realm, "createTreeWalker", 2, d_create_tree_walker);
    m(realm, "createNodeIterator", 2, d_create_node_iterator);
    m(realm, "createCDATASection", 1, d_create_cdata_section);
    m(realm, "createProcessingInstruction", 2, d_create_processing_instruction);
    m(realm, "importNode", 2, d_import_node);
    m(realm, "adoptNode", 1, d_adopt_node);
    m(realm, "hasFocus", 0, d_has_focus);
    m(realm, "execCommand", 3, d_exec_command);
    m(realm, "queryCommandSupported", 1, d_query_command_supported);
    m(realm, "queryCommandEnabled", 1, d_query_command_supported);
    m(realm, "queryCommandState", 1, d_query_command_supported);
    m(realm, "elementFromPoint", 2, d_element_from_point);
    m(realm, "elementsFromPoint", 2, d_elements_from_point);
    m(realm, "caretRangeFromPoint", 2, d_null);
    m(realm, "write", 1, d_write);
    m(realm, "writeln", 1, d_writeln);
    m(realm, "open", 0, d_open);
    m(realm, "close", 0, d_close);
    m(realm, "getSelection", 0, d_get_selection);
    m(realm, "exitFullscreen", 0, fonts_load);
    m(realm, "exitPointerLock", 0, d_null);
    m(realm, "getAnimations", 0, impl_empty_array);
    crate::events::install_handler_props(realm, d, &["readystatechange", "DOMContentLoaded", "visibilitychange", "selectionchange", "fullscreenchange", "pointerlockchange"]);
    crate::events::install_handler_props(realm, d, crate::element::EVENT_HANDLER_NAMES);
}

fn impl_empty_array(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(realm.new_array_value(vec![]))
}

fn doc_ctor(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(new_aux_document(realm, ""))
}
