//! Smaller platform objects: `DOMException`, `DOMParser`, `XMLSerializer`,
//! `Range`, `TreeWalker`/`NodeIterator`, `Selection`, `AbortController`,
//! `customElements`, `Notification`, form data sets.

use crate::host::*;
use rabbithole_js::V;
use crate::node;
use rabbithole_dom::{Document, NodeData, NodeId};
use rabbithole_js::{JsResult, ObjectId, PropertyKey, Realm, Value, ALL, CONFIGURABLE, ENUMERABLE, HIDDEN};

const ACC: u8 = CONFIGURABLE | ENUMERABLE;

// ----------------------------------------------------------- DOMException

fn dom_exception_code(name: &str) -> u32 {
    match name {
        "IndexSizeError" => 1,
        "DOMStringSizeError" => 2,
        "HierarchyRequestError" => 3,
        "WrongDocumentError" => 4,
        "InvalidCharacterError" => 5,
        "NoDataAllowedError" => 6,
        "NoModificationAllowedError" => 7,
        "NotFoundError" => 8,
        "NotSupportedError" => 9,
        "InUseAttributeError" => 10,
        "InvalidStateError" => 11,
        "SyntaxError" => 12,
        "InvalidModificationError" => 13,
        "NamespaceError" => 14,
        "InvalidAccessError" => 15,
        "ValidationError" => 16,
        "TypeMismatchError" => 17,
        "SecurityError" => 18,
        "NetworkError" => 19,
        "AbortError" => 20,
        "URLMismatchError" => 21,
        "QuotaExceededError" => 22,
        "TimeoutError" => 23,
        "InvalidNodeTypeError" => 24,
        "DataCloneError" => 25,
        _ => 0,
    }
}

/// Build a `DOMException` value (to be returned as `Err`).
pub(crate) fn dom_exception(realm: &mut Realm, name: &str, message: &str) -> Value {
    let p = proto("DOMException");
    let o = realm.new_object_with_proto(Some(p));
    realm.set_class_name(o, "DOMException");
    realm.define_property(o, "name", Value::str(name), HIDDEN);
    realm.define_property(o, "message", Value::str(message), HIDDEN);
    realm.define_property(o, "code", Value::Number(dom_exception_code(name) as f64), HIDDEN);
    let stack = realm.build_stack_string(name, message);
    realm.define_property(o, "stack", Value::str(&stack), HIDDEN);
    Value::Object(o)
}

fn dom_exception_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let message = arg_str_opt(realm, args, 0)?.unwrap_or_default();
    let name = arg_str_opt(realm, args, 1)?.unwrap_or_else(|| "Error".into());
    let v = dom_exception(realm, &name, &message);
    if let V::Object(id) = v.v() {
        let p = realm.proto_from_new_target(proto("DOMException"));
        realm.obj_mut(id).set_proto(Some(p));
    }
    Ok(v)
}

fn dom_exception_to_string(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let name = realm.get_str(this.clone(), "name")?;
    let msg = realm.get_str(this, "message")?;
    let name = realm.to_rust_string(name)?;
    let msg = realm.to_rust_string(msg)?;
    Ok(Value::str(&if msg.is_empty() { name } else { format!("{name}: {msg}") }))
}

// --------------------------------------------------------------- DOMParser

fn dom_parser_ctor(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let p = realm.proto_from_new_target(proto("DOMParser"));
    let o = realm.new_object_with_proto(Some(p));
    Ok(Value::Object(o))
}

fn dom_parser_parse(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let src = arg_str(realm, args, 0)?;
    let ty = arg_str(realm, args, 1)?;
    // The new document's URL is the parser's associated document's.
    let url = Some(crate::window::current_href());
    match ty.as_str() {
        "text/html" => {
            let mut doc = rabbithole_html::parse(&src);
            doc.url = url;
            Ok(crate::document::new_aux_document_from(realm, doc))
        }
        "text/xml" | "application/xml" | "application/xhtml+xml" | "image/svg+xml" => {
            let mut doc = rabbithole_html::xml::parse_xml(&src);
            doc.content_type = ty.clone();
            doc.url = url;
            // An XML document with the Document interface (not XMLDocument).
            Ok(crate::document::new_aux_document_from(realm, doc))
        }
        _ => Err(realm.throw_type_error(&format!("Failed to execute 'parseFromString' on 'DOMParser': The provided value '{ty}' is not a valid enum value of type DOMParserSupportedType."))),
    }
}

fn xml_serializer_ctor(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let p = realm.proto_from_new_target(proto("XMLSerializer"));
    Ok(Value::Object(realm.new_object_with_proto(Some(p))))
}

fn xml_serializer_serialize(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(n) = node_of(realm, &arg(args, 0)) else {
        return Err(realm.throw_type_error("parameter 1 is not of type 'Node'"));
    };
    // XMLSerializer does not require well-formedness.
    Ok(Value::str(&with_doc(n.doc, |doc| doc.serialize_xml(n.node, false)).unwrap_or_default()))
}

// ------------------------------------------------------------------- Range

struct RangeData {
    doc: u32,
    start: Option<(NodeId, usize)>,
    end: Option<(NodeId, usize)>,
}

fn range_data(realm: &mut Realm, this: &Value) -> JsResult<ObjectId> {
    match this.v() {
        V::Object(o) if realm.host_data::<RangeData>(o).is_some() => Ok(o),
        _ => Err(realm.throw_type_error("Illegal invocation")),
    }
}

pub(crate) fn new_range(realm: &mut Realm, doc: u32) -> JsResult<Value> {
    let p = proto("Range");
    let o = realm.new_object_with_proto(Some(p));
    realm.set_host_data(o, Box::new(RangeData { doc, start: None, end: None }));
    realm.set_class_name(o, "Range");
    Ok(Value::Object(o))
}

fn range_ctor(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    new_range(realm, 0)
}

fn range_set_start(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = range_data(realm, &this)?;
    let n = node_of(realm, &arg(args, 0));
    let off = realm.to_number(arg(args, 1)).unwrap_or(0.0).max(0.0) as usize;
    if let Some(n) = n {
        realm.host_data_mut::<RangeData>(o).unwrap().start = Some((n.node, off));
    }
    Ok(Value::Undefined)
}

fn range_set_end(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = range_data(realm, &this)?;
    let n = node_of(realm, &arg(args, 0));
    let off = realm.to_number(arg(args, 1)).unwrap_or(0.0).max(0.0) as usize;
    if let Some(n) = n {
        realm.host_data_mut::<RangeData>(o).unwrap().end = Some((n.node, off));
    }
    Ok(Value::Undefined)
}

fn range_select_node_contents(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = range_data(realm, &this)?;
    if let Some(n) = node_of(realm, &arg(args, 0)) {
        let count = with_doc(n.doc, |doc| doc.children(n.node).count());
        let d = realm.host_data_mut::<RangeData>(o).unwrap();
        d.start = Some((n.node, 0));
        d.end = Some((n.node, count));
    }
    Ok(Value::Undefined)
}

fn range_select_node(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = range_data(realm, &this)?;
    if let Some(n) = node_of(realm, &arg(args, 0)) {
        let (parent, idx) = with_doc(n.doc, |doc| {
            let p = doc.parent(n.node);
            let idx = p.map(|p| doc.children(p).position(|c| c == n.node).unwrap_or(0)).unwrap_or(0);
            (p, idx)
        });
        if let Some(p) = parent {
            let d = realm.host_data_mut::<RangeData>(o).unwrap();
            d.start = Some((p, idx));
            d.end = Some((p, idx + 1));
        }
    }
    Ok(Value::Undefined)
}

fn range_nodes(realm: &Realm, o: ObjectId) -> (u32, Vec<NodeId>) {
    let d = realm.host_data::<RangeData>(o).unwrap();
    let doc = d.doc;
    let (Some((sn, so)), Some((en, eo))) = (d.start, d.end) else { return (doc, Vec::new()) };
    if sn == en {
        return (doc, with_doc(doc, |d| d.children(sn).skip(so).take(eo.saturating_sub(so)).collect()));
    }
    (doc, with_doc(doc, |d| d.children(sn).skip(so).collect()))
}

fn range_clone_contents(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = range_data(realm, &this)?;
    let (doc, nodes) = range_nodes(realm, o);
    let frag = with_doc(doc, |d| {
        let f = d.create_element("#fragment", vec![]);
        for n in nodes {
            let c = d.clone_subtree(n, true);
            d.append_child(f, c);
        }
        f
    });
    Ok(wrap(realm, NodeRef { doc, node: frag }))
}

fn range_extract_contents(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = range_data(realm, &this)?;
    let (doc, nodes) = range_nodes(realm, o);
    let frag = with_doc(doc, |d| {
        let f = d.create_element("#fragment", vec![]);
        for n in nodes {
            d.detach(n);
            d.append_child(f, n);
        }
        f
    });
    if doc == 0 {
        mark_dirty();
    }
    Ok(wrap(realm, NodeRef { doc, node: frag }))
}

fn range_delete_contents(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = range_data(realm, &this)?;
    let (doc, nodes) = range_nodes(realm, o);
    for n in nodes {
        node::remove_node(NodeRef { doc, node: n });
    }
    Ok(Value::Undefined)
}

fn range_to_string(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = range_data(realm, &this)?;
    let d = realm.host_data::<RangeData>(o).unwrap();
    let doc = d.doc;
    let s = match (d.start, d.end) {
        (Some((sn, so)), Some((en, eo))) if sn == en => with_doc(doc, |d| match &d.node(sn).data {
            NodeData::Text(t) => {
                let units: Vec<u16> = t.encode_utf16().collect();
                String::from_utf16_lossy(&units[so.min(units.len())..eo.min(units.len()).max(so.min(units.len()))])
            }
            _ => d.children(sn).skip(so).take(eo.saturating_sub(so)).map(|c| d.text_content(c)).collect::<String>(),
        }),
        (Some((sn, _)), _) => with_doc(doc, |d| d.text_content(sn)),
        _ => String::new(),
    };
    Ok(Value::str(&s))
}

fn range_get_bounding_client_rect(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = range_data(realm, &this)?;
    let d = realm.host_data::<RangeData>(o).unwrap();
    let doc = d.doc;
    let start = d.start.map(|(n, _)| n);
    let r = match start {
        Some(n) => crate::element::node_rect(NodeRef { doc, node: n }),
        None => rabbithole_gfx::Rect::default(),
    };
    Ok(crate::element::dom_rect(realm, r.x, r.y, r.w, r.h))
}

fn range_get_client_rects(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let r = range_get_bounding_client_rect(realm, this, &[])?;
    Ok(realm.new_array_value(vec![r]))
}

fn range_create_contextual_fragment(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = range_data(realm, &this)?;
    let d = realm.host_data::<RangeData>(o).unwrap();
    let doc = d.doc;
    let start = d.start.map(|(n, _)| n);
    if args.is_empty() {
        return Err(realm.throw_type_error("Failed to execute 'createContextualFragment' on 'Range': 1 argument required, but only 0 present."));
    }
    let html = arg_str(realm, args, 0)?;
    // The context: the start node if it is an element, else its parent
    // element; none (or an HTML document's <html>) parses as <body>.
    let ctx = with_doc(doc, |d| {
        let el = start.and_then(|s| if d.element(s).map(|e| !e.tag.starts_with('#')).unwrap_or(false) { Some(s) } else { d.parent(s).filter(|&p| d.element(p).map(|e| !e.tag.starts_with('#')).unwrap_or(false)) });
        match el {
            Some(e) if !(d.is_html && d.element(e).map(|x| x.tag == "html" && x.ns == rabbithole_dom::Namespace::Html).unwrap_or(false)) => e,
            _ => d.create_element("body", vec![]),
        }
    });
    let nodes = match node::try_parse_html_into(NodeRef { doc, node: ctx }, &html) {
        Ok(n) => n,
        Err(msg) => return Err(dom_exception(realm, "SyntaxError", &format!("Failed to execute 'createContextualFragment' on 'Range': The provided markup is invalid XML ({msg})."))),
    };
    let frag = with_doc(doc, |d| {
        let f = d.create_element("#fragment", vec![]);
        // XML fragments: HTML <html>, <head> and <body> are replaced by
        // their children (the HTML fragment parser never makes them).
        fn strip(d: &mut Document, n: NodeId, f: NodeId) {
            let special = !d.is_html && d.element(n).map(|e| e.ns == rabbithole_dom::Namespace::Html && matches!(e.tag.as_str(), "html" | "head" | "body")).unwrap_or(false);
            if special {
                let kids: Vec<NodeId> = d.children(n).collect();
                for k in kids {
                    d.detach(k);
                    strip(d, k, f);
                }
            } else {
                d.append_child(f, n);
            }
        }
        for n in nodes {
            strip(d, n.node, f);
        }
        // Its scripts run when inserted (not "already started").
        let scripts: Vec<NodeId> = d.descendants(f).filter(|&s| d.tag(s) == Some("script")).collect();
        for s in scripts {
            d.script_started.remove(&s);
        }
        f
    });
    Ok(wrap(realm, NodeRef { doc, node: frag }))
}

fn range_collapsed(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = range_data(realm, &this)?;
    let d = realm.host_data::<RangeData>(o).unwrap();
    Ok(Value::Bool(d.start == d.end))
}

fn range_start_container(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = range_data(realm, &this)?;
    let d = realm.host_data::<RangeData>(o).unwrap();
    let (doc, n) = (d.doc, d.start.map(|(n, _)| n));
    Ok(wrap_opt(realm, doc, n))
}

fn range_end_container(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = range_data(realm, &this)?;
    let d = realm.host_data::<RangeData>(o).unwrap();
    let (doc, n) = (d.doc, d.end.map(|(n, _)| n));
    Ok(wrap_opt(realm, doc, n))
}

fn range_start_offset(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = range_data(realm, &this)?;
    Ok(Value::Number(realm.host_data::<RangeData>(o).unwrap().start.map(|(_, o)| o).unwrap_or(0) as f64))
}

fn range_end_offset(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = range_data(realm, &this)?;
    Ok(Value::Number(realm.host_data::<RangeData>(o).unwrap().end.map(|(_, o)| o).unwrap_or(0) as f64))
}

fn range_noop(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Undefined)
}

fn range_insert_node(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = range_data(realm, &this)?;
    let d = realm.host_data::<RangeData>(o).unwrap();
    let doc = d.doc;
    let Some((sn, so)) = d.start else { return Ok(Value::Undefined) };
    let Some(n) = node_of(realm, &arg(args, 0)) else { return Ok(Value::Undefined) };
    let before = with_doc(doc, |d| d.children(sn).nth(so));
    node::insert_node(realm, NodeRef { doc, node: sn }, n, before)?;
    Ok(Value::Undefined)
}

// ------------------------------------------------------- TreeWalker/Iterator

struct WalkerData {
    doc: u32,
    root: NodeId,
    /// TreeWalker: currentNode. NodeIterator: the reference node.
    current: NodeId,
    what: u32,
    filter: Value,
    /// NodeIterator "pointer before reference" flag.
    before: bool,
}

fn walker_data(realm: &mut Realm, this: &Value) -> JsResult<ObjectId> {
    match this.v() {
        V::Object(o) if realm.host_data::<WalkerData>(o).is_some() => Ok(o),
        _ => Err(realm.throw_type_error("Illegal invocation")),
    }
}

pub(crate) fn new_tree_walker(realm: &mut Realm, args: &[Value], iterator: bool) -> JsResult<Value> {
    let Some(root) = node_of(realm, &arg(args, 0)) else {
        return Err(realm.throw_type_error("parameter 1 is not of type 'Node'"));
    };
    let what = match (args.get(1)).map(Value::v) {
        None | Some(V::Undefined) => 0xFFFF_FFFF,
        Some(v) => realm.to_number(v.to_value())? as u32,
    };
    let filter = arg(args, 2);
    root_value(realm, &filter);
    let p = proto(if iterator { "NodeIterator" } else { "TreeWalker" });
    let o = realm.new_object_with_proto(Some(p));
    realm.set_host_data(o, Box::new(WalkerData { doc: root.doc, root: root.node, current: root.node, what, filter, before: true }));
    let rv = wrap(realm, root);
    realm.define_property(o, "root", rv, ACC);
    realm.define_property(o, "whatToShow", Value::Number(what as f64), ACC);
    realm.define_property(o, "filter", filter_value(realm, o), ACC);
    Ok(Value::Object(o))
}

fn filter_value(realm: &Realm, o: ObjectId) -> Value {
    realm.host_data::<WalkerData>(o).map(|d| d.filter.clone()).unwrap_or(Value::Null)
}

/// FILTER_ACCEPT=1, REJECT=2, SKIP=3
fn walker_accept(realm: &mut Realm, o: ObjectId, node: NodeId) -> JsResult<u32> {
    let (doc, what, filter) = {
        let d = realm.host_data::<WalkerData>(o).unwrap();
        (d.doc, d.what, d.filter.clone())
    };
    let ty = node::node_type(NodeRef { doc, node }) as u32;
    let bit = 1u32 << (ty - 1);
    if what & bit == 0 {
        return Ok(3);
    }
    if filter.is_nullish() {
        return Ok(1);
    }
    let w = wrap(realm, NodeRef { doc, node });
    let r = if realm.is_callable(&filter) {
        realm.call(filter, Value::Undefined, &[w])?
    } else {
        let f = realm.get_str(filter.clone(), "acceptNode")?;
        if realm.is_callable(&f) {
            realm.call(f, filter, &[w])?
        } else {
            Value::Number(1.0)
        }
    };
    Ok(realm.to_number(r)? as u32)
}

/// Next node in document order within root (exclusive of ancestors above root).
fn doc_order_next(doc: &Document, root: NodeId, cur: NodeId, skip_children: bool) -> Option<NodeId> {
    if !skip_children {
        if let Some(c) = doc.first_child(cur) {
            return Some(c);
        }
    }
    let mut n = cur;
    loop {
        if n == root {
            return None;
        }
        if let Some(s) = doc.next_sibling(n) {
            return Some(s);
        }
        n = doc.parent(n)?;
    }
}

fn doc_order_prev(doc: &Document, root: NodeId, cur: NodeId) -> Option<NodeId> {
    if cur == root {
        return None;
    }
    match doc.prev_sibling(cur) {
        Some(mut s) => {
            while let Some(l) = doc.last_child(s) {
                s = l;
            }
            Some(s)
        }
        None => doc.parent(cur),
    }
}

fn walker_next_node(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = walker_data(realm, &this)?;
    let (doc, root, mut cur) = {
        let d = realm.host_data::<WalkerData>(o).unwrap();
        (d.doc, d.root, d.current)
    };
    let mut skip = false;
    loop {
        let next = with_doc(doc, |d| doc_order_next(d, root, cur, skip));
        let Some(n) = next else { return Ok(Value::Null) };
        cur = n;
        let r = walker_accept(realm, o, n)?;
        skip = r == 2;
        if r == 1 {
            realm.host_data_mut::<WalkerData>(o).unwrap().current = n;
            return Ok(wrap(realm, NodeRef { doc, node: n }));
        }
    }
}

/// NodeIterator.nextNode / previousNode (DOM §6.1 "traverse"): the
/// reference node itself is a candidate when the pointer is on the other
/// side of it, and FILTER_REJECT does not skip descendants.
fn iterator_traverse(realm: &mut Realm, this: Value, forward: bool) -> JsResult<Value> {
    let o = walker_data(realm, &this)?;
    let (doc, root, mut node, mut before) = {
        let d = realm.host_data::<WalkerData>(o).unwrap();
        (d.doc, d.root, d.current, d.before)
    };
    loop {
        if forward {
            if before {
                before = false;
            } else {
                match with_doc(doc, |d| doc_order_next(d, root, node, false)) {
                    Some(n) => node = n,
                    None => return Ok(Value::Null),
                }
            }
        } else if !before {
            before = true;
        } else {
            match with_doc(doc, |d| doc_order_prev(d, root, node)) {
                Some(n) => node = n,
                None => return Ok(Value::Null),
            }
        }
        if walker_accept(realm, o, node)? == 1 {
            let d = realm.host_data_mut::<WalkerData>(o).unwrap();
            d.current = node;
            d.before = before;
            return Ok(wrap(realm, NodeRef { doc, node }));
        }
    }
}

fn iterator_next_node(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    iterator_traverse(realm, this, true)
}

fn iterator_previous_node(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    iterator_traverse(realm, this, false)
}

fn walker_previous_node(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = walker_data(realm, &this)?;
    let (doc, root, mut cur) = {
        let d = realm.host_data::<WalkerData>(o).unwrap();
        (d.doc, d.root, d.current)
    };
    loop {
        let prev = with_doc(doc, |d| doc_order_prev(d, root, cur));
        let Some(n) = prev else { return Ok(Value::Null) };
        cur = n;
        if walker_accept(realm, o, n)? == 1 {
            realm.host_data_mut::<WalkerData>(o).unwrap().current = n;
            return Ok(wrap(realm, NodeRef { doc, node: n }));
        }
    }
}

fn walker_step_sibling(realm: &mut Realm, this: Value, forward: bool) -> JsResult<Value> {
    let o = walker_data(realm, &this)?;
    let (doc, root, mut cur) = {
        let d = realm.host_data::<WalkerData>(o).unwrap();
        (d.doc, d.root, d.current)
    };
    if cur == root {
        return Ok(Value::Null);
    }
    loop {
        let s = with_doc(doc, |d| if forward { d.next_sibling(cur) } else { d.prev_sibling(cur) });
        let Some(s) = s else { return Ok(Value::Null) };
        cur = s;
        if walker_accept(realm, o, s)? == 1 {
            realm.host_data_mut::<WalkerData>(o).unwrap().current = s;
            return Ok(wrap(realm, NodeRef { doc, node: s }));
        }
    }
}

fn walker_next_sibling(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    walker_step_sibling(realm, this, true)
}

fn walker_previous_sibling(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    walker_step_sibling(realm, this, false)
}

fn walker_parent_node(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = walker_data(realm, &this)?;
    let (doc, root, mut cur) = {
        let d = realm.host_data::<WalkerData>(o).unwrap();
        (d.doc, d.root, d.current)
    };
    loop {
        if cur == root {
            return Ok(Value::Null);
        }
        let p = with_doc(doc, |d| d.parent(cur));
        let Some(p) = p else { return Ok(Value::Null) };
        cur = p;
        if walker_accept(realm, o, p)? == 1 {
            realm.host_data_mut::<WalkerData>(o).unwrap().current = p;
            return Ok(wrap(realm, NodeRef { doc, node: p }));
        }
    }
}

fn walker_first_child(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = walker_data(realm, &this)?;
    let (doc, cur) = {
        let d = realm.host_data::<WalkerData>(o).unwrap();
        (d.doc, d.current)
    };
    let mut c = with_doc(doc, |d| d.first_child(cur));
    while let Some(n) = c {
        if walker_accept(realm, o, n)? == 1 {
            realm.host_data_mut::<WalkerData>(o).unwrap().current = n;
            return Ok(wrap(realm, NodeRef { doc, node: n }));
        }
        c = with_doc(doc, |d| d.next_sibling(n));
    }
    Ok(Value::Null)
}

fn walker_last_child(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = walker_data(realm, &this)?;
    let (doc, cur) = {
        let d = realm.host_data::<WalkerData>(o).unwrap();
        (d.doc, d.current)
    };
    let mut c = with_doc(doc, |d| d.last_child(cur));
    while let Some(n) = c {
        if walker_accept(realm, o, n)? == 1 {
            realm.host_data_mut::<WalkerData>(o).unwrap().current = n;
            return Ok(wrap(realm, NodeRef { doc, node: n }));
        }
        c = with_doc(doc, |d| d.prev_sibling(n));
    }
    Ok(Value::Null)
}

fn walker_current_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = walker_data(realm, &this)?;
    let d = realm.host_data::<WalkerData>(o).unwrap();
    let (doc, cur) = (d.doc, d.current);
    Ok(wrap(realm, NodeRef { doc, node: cur }))
}

fn walker_current_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = walker_data(realm, &this)?;
    if let Some(n) = node_of(realm, &arg(args, 0)) {
        realm.host_data_mut::<WalkerData>(o).unwrap().current = n.node;
    }
    Ok(Value::Undefined)
}

// ---------------------------------------------------------------- Selection

fn sel_to_string(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::str(""))
}

fn sel_noop(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Undefined)
}

fn sel_get_range(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    new_range(realm, 0)
}

pub(crate) fn selection_object(realm: &mut Realm) -> JsResult<Value> {
    let o = object_from(realm, vec![("anchorNode", Value::Null), ("focusNode", Value::Null), ("anchorOffset", Value::Number(0.0)), ("focusOffset", Value::Number(0.0)), ("isCollapsed", Value::Bool(true)), ("rangeCount", Value::Number(0.0)), ("type", Value::str("None"))]);
    if let V::Object(id) = o.v() {
        realm.set_class_name(id, "Selection");
        realm.define_method(id, "toString", 0, sel_to_string);
        realm.define_method(id, "getRangeAt", 1, sel_get_range);
        realm.define_method(id, "addRange", 1, sel_noop);
        realm.define_method(id, "removeAllRanges", 0, sel_noop);
        realm.define_method(id, "removeRange", 1, sel_noop);
        realm.define_method(id, "collapse", 2, sel_noop);
        realm.define_method(id, "collapseToStart", 0, sel_noop);
        realm.define_method(id, "collapseToEnd", 0, sel_noop);
        realm.define_method(id, "selectAllChildren", 1, sel_noop);
        realm.define_method(id, "extend", 2, sel_noop);
        realm.define_method(id, "setBaseAndExtent", 4, sel_noop);
        realm.define_method(id, "empty", 0, sel_noop);
        realm.define_method(id, "containsNode", 2, sel_false);
    }
    Ok(o)
}

fn sel_false(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(false))
}

// ---------------------------------------------------- AbortController/Signal

fn abort_controller_ctor(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let p = realm.proto_from_new_target(proto("AbortController"));
    let o = realm.new_object_with_proto(Some(p));
    let sig = new_abort_signal(realm);
    realm.define_property(o, "signal", Value::Object(sig), ACC);
    Ok(Value::Object(o))
}

pub(crate) fn new_abort_signal(realm: &mut Realm) -> ObjectId {
    let p = proto("AbortSignal");
    let s = realm.new_object_with_proto(Some(p));
    realm.set_class_name(s, "AbortSignal");
    set_hidden(realm, s, "__aborted", Value::Bool(false));
    set_hidden(realm, s, "__reason", Value::Undefined);
    s
}

fn signal_aborted(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let V::Object(o) = this.v() else { return Ok(Value::Bool(false)) };
    Ok(Value::Bool(get_hidden(realm, o, "__aborted").truthy()))
}

fn signal_reason(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let V::Object(o) = this.v() else { return Ok(Value::Undefined) };
    Ok(get_hidden(realm, o, "__reason"))
}

fn signal_throw_if_aborted(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let V::Object(o) = this.v() else { return Ok(Value::Undefined) };
    if get_hidden(realm, o, "__aborted").truthy() {
        return Err(get_hidden(realm, o, "__reason"));
    }
    Ok(Value::Undefined)
}

pub(crate) fn abort_signal(realm: &mut Realm, sig: ObjectId, reason: Option<Value>) {
    if get_hidden(realm, sig, "__aborted").truthy() {
        return;
    }
    let reason = match reason {
        Some(r) if !r.is_undefined() => r,
        _ => dom_exception(realm, "AbortError", "signal is aborted without reason"),
    };
    set_hidden(realm, sig, "__aborted", Value::Bool(true));
    set_hidden(realm, sig, "__reason", reason);
    // Remove listeners registered with this signal.
    let list = get_hidden(realm, sig, "__listeners");
    if let V::Object(a) = list.v() {
        let recs: Vec<Value> = realm.obj(a).array_elements().cloned().unwrap_or_default();
        for r in recs {
            let ty = realm.get_str(r.clone(), "type").ok().map(|t| realm.to_rust_string(t).unwrap_or_default()).unwrap_or_default();
            let cb = realm.get_str(r.clone(), "callback").unwrap_or(Value::Undefined);
            let capture = realm.get_str(r, "capture").map(|c| realm.to_boolean(&c)).unwrap_or(false);
            let keys: Vec<TargetKey> = state().borrow().listeners.keys().copied().collect();
            for k in keys {
                crate::events::remove_listener(realm, k, &ty, &cb, capture);
            }
        }
    }
    crate::events::dispatch(realm, TargetKey::Object(sig), "abort", crate::EventInit::simple(false, false), true);
}

fn abort_controller_abort(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let sig = realm.get_str(this, "signal")?;
    if let V::Object(s) = sig.v() {
        abort_signal(realm, s, Some(arg(args, 0)));
    }
    Ok(Value::Undefined)
}

fn abort_signal_abort_static(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let s = new_abort_signal(realm);
    abort_signal(realm, s, Some(arg(args, 0)));
    Ok(Value::Object(s))
}

fn abort_signal_timeout_static(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let s = new_abort_signal(realm);
    let ms = realm.to_number(arg(args, 0)).unwrap_or(0.0);
    let f = realm.new_native_function_full("abortTimeout", 0, abort_timeout_fire, None, vec![Value::Object(s)], false);
    let set_timeout = realm.get_str(Value::Object(realm.global()), "setTimeout")?;
    realm.call(set_timeout, Value::Undefined, &[Value::Object(f), Value::Number(ms)])?;
    Ok(Value::Object(s))
}

fn abort_timeout_fire(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let caps = native_captures(realm);
    if let Some(V::Object(s)) = caps.first().map(Value::v) {
        let e = dom_exception(realm, "TimeoutError", "signal timed out");
        abort_signal(realm, s, Some(e));
    }
    Ok(Value::Undefined)
}

fn abort_signal_any(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Object(new_abort_signal(realm)))
}

// ----------------------------------------------------------- Notification

fn notification_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let p = realm.proto_from_new_target(proto("Notification"));
    let o = realm.new_object_with_proto(Some(p));
    let title = arg_str_opt(realm, args, 0)?.unwrap_or_default();
    realm.define_property(o, "title", Value::str(&title), ACC);
    realm.define_method(o, "close", 0, sel_noop);
    Ok(Value::Object(o))
}

fn notification_request_permission(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let (p, resolve, _) = realm.new_promise();
    realm.call(Value::Object(resolve), Value::Undefined, &[Value::str("denied")])?;
    Ok(Value::Object(p))
}

// ------------------------------------------------------------ form data set

/// Simplified form data set (name/value pairs) for `new FormData(form)`.
pub(crate) fn forms_data_set(doc: &Document, form: NodeId) -> (Vec<(String, String)>, Vec<()>) {
    let mut fields = Vec::new();
    for n in doc.descendants(form).skip(1) {
        let Some(el) = doc.element(n) else { continue };
        if el.attr("disabled").is_some() {
            continue;
        }
        let name = el.attr("name").unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }
        match el.tag.as_str() {
            "input" => {
                let ty = el.attr("type").map(|t| t.to_ascii_lowercase()).unwrap_or_else(|| "text".into());
                match ty.as_str() {
                    "checkbox" | "radio" => {
                        if doc.is_checked(n) {
                            fields.push((name, el.attr("value").unwrap_or("on").to_string()));
                        }
                    }
                    "submit" | "button" | "reset" | "image" | "file" => {}
                    _ => fields.push((name, doc.control_value(n).unwrap_or("").to_string())),
                }
            }
            "textarea" => fields.push((name, doc.control_value(n).map(|v| v.to_string()).unwrap_or_else(|| doc.text_content(n)))),
            "select" => {
                let multiple = el.attr("multiple").is_some();
                let mut first: Option<String> = None;
                let mut any = false;
                for o in doc.descendants(n).skip(1).filter(|&o| doc.is_element(o, "option")) {
                    let v = doc.attr(o, "value").map(|v| v.to_string()).unwrap_or_else(|| doc.text_content(o).trim().to_string());
                    if first.is_none() {
                        first = Some(v.clone());
                    }
                    if doc.attr(o, "selected").is_some() {
                        fields.push((name.clone(), v));
                        any = true;
                        if !multiple {
                            break;
                        }
                    }
                }
                if !any && !multiple {
                    if let Some(f) = first {
                        fields.push((name, f));
                    }
                }
            }
            _ => {}
        }
    }
    (fields, Vec::new())
}

// ------------------------------------------------------------------ install

pub(crate) fn install(realm: &mut Realm) {
    let de = define_class(realm, "DOMException", None, dom_exception_ctor);
    let error_proto = realm.intrinsics.error_proto;
    realm.obj_mut(de).set_proto(Some(error_proto));
    realm.define_method(de, "toString", 0, dom_exception_to_string);
    for (k, v) in [("INDEX_SIZE_ERR", 1.0), ("HIERARCHY_REQUEST_ERR", 3.0), ("NOT_FOUND_ERR", 8.0), ("NOT_SUPPORTED_ERR", 9.0), ("INVALID_STATE_ERR", 11.0), ("SYNTAX_ERR", 12.0), ("ABORT_ERR", 20.0)] {
        realm.define_property(de, k, Value::Number(v), 0);
    }
    let dp = define_class(realm, "DOMParser", None, dom_parser_ctor);
    realm.define_method(dp, "parseFromString", 2, dom_parser_parse);
    let xs = define_class(realm, "XMLSerializer", None, xml_serializer_ctor);
    realm.define_method(xs, "serializeToString", 1, xml_serializer_serialize);

    let range = define_class(realm, "Range", None, range_ctor);
    realm.define_method(range, "setStart", 2, range_set_start);
    realm.define_method(range, "setEnd", 2, range_set_end);
    realm.define_method(range, "setStartBefore", 1, range_set_start);
    realm.define_method(range, "setStartAfter", 1, range_set_start);
    realm.define_method(range, "setEndBefore", 1, range_set_end);
    realm.define_method(range, "setEndAfter", 1, range_set_end);
    realm.define_method(range, "selectNode", 1, range_select_node);
    realm.define_method(range, "selectNodeContents", 1, range_select_node_contents);
    realm.define_method(range, "cloneContents", 0, range_clone_contents);
    realm.define_method(range, "extractContents", 0, range_extract_contents);
    realm.define_method(range, "deleteContents", 0, range_delete_contents);
    realm.define_method(range, "insertNode", 1, range_insert_node);
    realm.define_method(range, "collapse", 1, range_noop);
    realm.define_method(range, "detach", 0, range_noop);
    realm.define_method(range, "cloneRange", 0, range_ctor);
    realm.define_method(range, "toString", 0, range_to_string);
    realm.define_method(range, "getBoundingClientRect", 0, range_get_bounding_client_rect);
    realm.define_method(range, "getClientRects", 0, range_get_client_rects);
    realm.define_method(range, "createContextualFragment", 1, range_create_contextual_fragment);
    realm.define_accessor(range, "collapsed", Some(range_collapsed), None, ACC);
    realm.define_accessor(range, "startContainer", Some(range_start_container), None, ACC);
    realm.define_accessor(range, "endContainer", Some(range_end_container), None, ACC);
    realm.define_accessor(range, "commonAncestorContainer", Some(range_start_container), None, ACC);
    realm.define_accessor(range, "startOffset", Some(range_start_offset), None, ACC);
    realm.define_accessor(range, "endOffset", Some(range_end_offset), None, ACC);

    for name in ["TreeWalker", "NodeIterator"] {
        let w = define_class(realm, name, None, illegal_ctor);
        if name == "TreeWalker" {
            realm.define_method(w, "nextNode", 0, walker_next_node);
            realm.define_method(w, "previousNode", 0, walker_previous_node);
        } else {
            realm.define_method(w, "nextNode", 0, iterator_next_node);
            realm.define_method(w, "previousNode", 0, iterator_previous_node);
        }
        if name == "TreeWalker" {
            realm.define_method(w, "nextSibling", 0, walker_next_sibling);
            realm.define_method(w, "previousSibling", 0, walker_previous_sibling);
            realm.define_method(w, "parentNode", 0, walker_parent_node);
            realm.define_method(w, "firstChild", 0, walker_first_child);
            realm.define_method(w, "lastChild", 0, walker_last_child);
            realm.define_accessor(w, "currentNode", Some(walker_current_get), Some(walker_current_set), ACC);
        } else {
            realm.define_method(w, "detach", 0, range_noop);
        }
    }
    let nf = realm.new_object();
    for (k, v) in [("FILTER_ACCEPT", 1.0), ("FILTER_REJECT", 2.0), ("FILTER_SKIP", 3.0), ("SHOW_ALL", 4294967295.0), ("SHOW_ELEMENT", 1.0), ("SHOW_TEXT", 4.0), ("SHOW_COMMENT", 128.0), ("SHOW_DOCUMENT", 256.0), ("SHOW_DOCUMENT_FRAGMENT", 1024.0)] {
        realm.define_property(nf, k, Value::Number(v), 0);
    }
    realm.define_global("NodeFilter", Value::Object(nf));
    define_class(realm, "Selection", None, illegal_ctor);

    let ac = define_class(realm, "AbortController", None, abort_controller_ctor);
    realm.define_method(ac, "abort", 1, abort_controller_abort);
    let asig = define_class(realm, "AbortSignal", Some("EventTarget"), illegal_ctor);
    realm.define_accessor(asig, "aborted", Some(signal_aborted), None, ACC);
    realm.define_accessor(asig, "reason", Some(signal_reason), None, ACC);
    realm.define_method(asig, "throwIfAborted", 0, signal_throw_if_aborted);
    crate::events::install_handler_props(realm, asig, &["abort"]);
    let asig_ctor = realm.obj(asig).props.get(&PropertyKey::str("constructor")).map(|p| p.value.clone());
    if let Some(V::Object(c)) = asig_ctor.as_ref().map(Value::v) {
        realm.define_method(c, "abort", 1, abort_signal_abort_static);
        realm.define_method(c, "timeout", 1, abort_signal_timeout_static);
        realm.define_method(c, "any", 1, abort_signal_any);
    }


    let notif = define_class(realm, "Notification", Some("EventTarget"), notification_ctor);
    let notif_ctor = realm.obj(notif).props.get(&PropertyKey::str("constructor")).map(|p| p.value.clone());
    if let Some(V::Object(c)) = notif_ctor.as_ref().map(Value::v) {
        realm.define_property(c, "permission", Value::str("denied"), ACC);
        realm.define_method(c, "requestPermission", 0, notification_request_permission);
    }
    let _ = ALL;
}
