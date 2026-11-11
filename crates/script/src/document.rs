//! `Document`: lookup, creation, `write`, `cookie`, ready state, etc.

use crate::env::with_env;
use crate::host::*;
use crate::node::{self, html_collection, insert_node};
use rabbithole_dom::{Document, NodeData, NodeId};
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
    let h = with_doc(d, |doc| doc.html_element());
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
    if d != 0 {
        return Ok(Value::str("about:blank"));
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
    if !is_xml_name(&tag) {
        return Err(crate::misc::dom_exception(realm, "InvalidCharacterError", &format!("The tag name provided ('{tag}') is not a valid name.")));
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

fn d_create_element_ns(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let d = this_doc(realm, &this)?;
    let ns = arg_str_opt(realm, args, 0)?.unwrap_or_default();
    let qname = arg_str(realm, args, 1)?;
    // DOM "validate and extract": a Name, then a QName (prefix needs a namespace).
    if !is_xml_name(&qname) {
        return Err(crate::misc::dom_exception(realm, "InvalidCharacterError", &format!("'{qname}' is not a valid name.")));
    }
    let parts: Vec<&str> = qname.split(':').collect();
    let bad_qname = parts.len() > 2 || parts.iter().any(|p| p.is_empty()) || parts.len() == 2 && !parts[1].starts_with(|c: char| c.is_alphabetic() || c == '_');
    let prefix_without_ns = parts.len() == 2 && ns.is_empty();
    if bad_qname || prefix_without_ns || parts.len() == 2 && parts[0] == "xml" && ns != "http://www.w3.org/XML/1998/namespace" {
        return Err(crate::misc::dom_exception(realm, "NamespaceError", &format!("'{qname}' is not a valid qualified name for this namespace.")));
    }
    let local = qname.rsplit(':').next().unwrap_or(&qname).to_string();
    let node = with_doc(d, |doc| {
        let tag = if ns.contains("svg") { local.clone() } else { local.to_ascii_lowercase() };
        let n = doc.create_element(&tag, vec![]);
        if ns.contains("svg") && tag != "svg" {
            // Mark as SVG so wrapping picks the SVG prototype even before insertion.
            doc.set_attr(n, "data-rh-ns", Some("svg"));
        }
        n
    });
    let v = wrap(realm, NodeRef { doc: d, node });
    if ns.contains("svg") {
        if let Value::Object(id) = &v {
            let class = crate::host::svg_class(&local);
            let p = proto(class);
            realm.obj_mut(*id).proto = Some(p);
            realm.set_class_name(*id, class);
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
    let class = match iface.to_ascii_lowercase().as_str() {
        "mouseevents" | "mouseevent" => "MouseEvent",
        "keyboardevent" | "keyevents" => "KeyboardEvent",
        "customevent" => "CustomEvent",
        "uievents" | "uievent" => "UIEvent",
        "focusevent" => "FocusEvent",
        _ => "Event",
    };
    let id = crate::events::create_event(realm, class, "", &crate::EventInit::simple(false, false), false);
    if class == "CustomEvent" {
        realm.define_property(id, "detail", Value::Null, ACC);
    }
    Ok(Value::Object(id))
}

fn d_create_attribute(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let _ = this_doc(realm, &this)?;
    let name = arg_str(realm, args, 0)?.to_ascii_lowercase();
    let a = object_from(realm, vec![("name", Value::str(&name)), ("value", Value::str("")), ("nodeName", Value::str(&name)), ("nodeValue", Value::str("")), ("specified", Value::Bool(true))]);
    if let Value::Object(o) = &a {
        let p = proto("Attr");
        realm.obj_mut(*o).proto = Some(p);
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
            let st = state();
            let st = st.borrow();
            let src = &st.extra_docs[(n.doc - 1) as usize];
            let mut t = Document::new();
            let c = t.import_subtree(src, n.node);
            t.append_child(Document::ROOT, c);
            t
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
    let _ = this_doc(realm, &this)?;
    if let Some(n) = node_of(realm, &arg(args, 0)) {
        node::remove_node(n);
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

/// `document.write(...)`: during loading, insert after the current script;
/// afterwards append to the body.
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
    let (loading, current) = {
        let st = state();
        let st = st.borrow();
        (st.loading, st.current_script)
    };
    let body = with_doc(d, |doc| doc.body());
    let Some(body) = body else { return Ok(Value::Undefined) };
    let body_ref = NodeRef { doc: d, node: body };
    let nodes = node::parse_html_into(body_ref, html);
    if loading && d == 0 {
        if let Some(cs) = current {
            let (parent, next) = with_doc(0, |doc| (doc.parent(cs.node), doc.next_sibling(cs.node)));
            if let Some(p) = parent {
                for n in nodes {
                    insert_node(realm, NodeRef::main(p), n, next)?;
                }
                return Ok(Value::Undefined);
            }
        }
    }
    for n in nodes {
        insert_node(realm, body_ref, n, None)?;
    }
    Ok(Value::Undefined)
}

fn d_open(_realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(this)
}

fn d_close(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
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
    let doc = rabbithole_html::parse(html);
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
    if let Value::Object(id) = &o {
        realm.define_method(*id, "load", 1, fonts_load);
        realm.define_method(*id, "check", 1, d_true);
        realm.define_method(*id, "add", 1, d_null);
        realm.define_method(*id, "addEventListener", 2, d_null);
        realm.define_method(*id, "forEach", 1, d_null);
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
    let _ = this_doc(realm, &this)?;
    Ok(object_from(realm, vec![("name", Value::str("html")), ("publicId", Value::str("")), ("systemId", Value::str("")), ("nodeType", Value::Number(10.0)), ("nodeName", Value::str("html"))]))
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
    acc(realm, "location", crate::window::location_getter, None);
    for (name, val) in [("characterSet", "UTF-8"), ("charset", "UTF-8"), ("inputEncoding", "UTF-8"), ("contentType", "text/html"), ("compatMode", "CSS1Compat"), ("visibilityState", "visible"), ("designMode", "off"), ("dir", "")] {
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
    m(realm, "createCDATASection", 1, d_create_text_node);
    m(realm, "createProcessingInstruction", 2, d_create_comment);
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
