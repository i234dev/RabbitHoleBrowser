//! `Node`, `CharacterData`/`Text`/`Comment`, `DocumentFragment`, the
//! ParentNode/ChildNode mixins, selectors, and the shared mutation helpers.

use crate::env::with_env;
use rabbithole_js::V;
use crate::host::*;
use rabbithole_css::{selector::SelectorParser, Selector, Tokenizer};
use rabbithole_dom::{Document, NodeData, NodeId};
use rabbithole_js::{JsResult, ObjectId, Realm, Value, HIDDEN};

pub const ELEMENT_NODE: f64 = 1.0;
pub const TEXT_NODE: f64 = 3.0;
pub const COMMENT_NODE: f64 = 8.0;
pub const DOCUMENT_NODE: f64 = 9.0;
pub const DOCUMENT_TYPE_NODE: f64 = 10.0;
pub const DOCUMENT_FRAGMENT_NODE: f64 = 11.0;

// ------------------------------------------------------------------ helpers

pub(crate) fn node_type(n: NodeRef) -> f64 {
    with_doc(n.doc, |doc| match &doc.node(n.node).data {
        NodeData::Document => DOCUMENT_NODE,
        NodeData::Element(e) if e.tag == "#fragment" || e.tag == "#shadow-root" => DOCUMENT_FRAGMENT_NODE,
        NodeData::Element(_) => ELEMENT_NODE,
        NodeData::Text(_) => TEXT_NODE,
        NodeData::Comment(_) => COMMENT_NODE,
        NodeData::Doctype(_) => DOCUMENT_TYPE_NODE,
    })
}

pub(crate) fn node_name(n: NodeRef) -> String {
    with_doc(n.doc, |doc| match &doc.node(n.node).data {
        NodeData::Document => "#document".into(),
        NodeData::Element(e) if e.tag == "#fragment" || e.tag == "#shadow-root" => "#document-fragment".into(),
        NodeData::Element(e) => {
            // The qualified name, upper-cased for HTML elements in an HTML
            // document.
            let qualified = match &e.prefix {
                Some(p) => format!("{p}:{}", e.tag),
                None => e.tag.clone(),
            };
            let html = e.ns == rabbithole_dom::Namespace::Html && doc.is_html && doc.ancestor_or_self(n.node, |a| doc.is_element(a, "svg")).is_none();
            if html {
                qualified.to_ascii_uppercase()
            } else {
                qualified
            }
        }
        NodeData::Text(_) => "#text".into(),
        NodeData::Comment(_) => "#comment".into(),
        NodeData::Doctype(d) => d.name.clone(),
    })
}

/// Children as refs.
pub(crate) fn children_of(n: NodeRef) -> Vec<NodeRef> {
    with_doc(n.doc, |doc| doc.children(n.node).map(|c| NodeRef { doc: n.doc, node: c }).collect())
}

pub(crate) fn element_children_of(n: NodeRef) -> Vec<NodeRef> {
    with_doc(n.doc, |doc| doc.child_elements(n.node).map(|c| NodeRef { doc: n.doc, node: c }).collect())
}

/// Build a NodeList (array with the NodeList prototype).
pub(crate) fn node_list(realm: &mut Realm, nodes: Vec<NodeRef>) -> Value {
    let values: Vec<Value> = nodes.into_iter().map(|n| wrap(realm, n)).collect();
    let arr = realm.new_array(values);
    let p = proto("NodeList");
    realm.obj_mut(arr).set_proto(Some(p));
    Value::Object(arr)
}

pub(crate) fn html_collection(realm: &mut Realm, nodes: Vec<NodeRef>) -> Value {
    let values: Vec<Value> = nodes.iter().map(|n| wrap(realm, *n)).collect();
    let arr = realm.new_array(values.clone());
    let p = proto("HTMLCollection");
    realm.obj_mut(arr).set_proto(Some(p));
    // Named access by id / name.
    for (n, v) in nodes.iter().zip(values.iter()) {
        let names = with_doc(n.doc, |doc| {
            let mut out = Vec::new();
            if let Some(id) = doc.attr(n.node, "id") {
                out.push(id.to_string());
            }
            if let Some(name) = doc.attr(n.node, "name") {
                out.push(name.to_string());
            }
            out
        });
        for name in names {
            if !name.is_empty() && name.parse::<u32>().is_err() && !realm.obj(arr).props.contains(&rabbithole_js::PropertyKey::str(&name)) {
                realm.define_property(arr, name.as_str(), v.clone(), HIDDEN);
            }
        }
    }
    Value::Object(arr)
}

/// Parse a selector list (SyntaxError DOMException on failure).
pub(crate) fn parse_selectors(realm: &mut Realm, src: &str) -> JsResult<Vec<Selector>> {
    let toks = Tokenizer::tokenize(src);
    let mut p = SelectorParser::new(&toks);
    match p.parse_list() {
        Some(list) if !list.is_empty() => Ok(list),
        _ => Err(crate::misc::dom_exception(realm, "SyntaxError", &format!("'{src}' is not a valid selector"))),
    }
}

/// `querySelectorAll` over the descendants of `root` (exclusive).
pub(crate) fn query_all(realm: &mut Realm, root: NodeRef, selectors: &str, first_only: bool) -> JsResult<Vec<NodeRef>> {
    let list = parse_selectors(realm, selectors)?;
    Ok(with_doc(root.doc, |doc| {
        let mut out = Vec::new();
        for n in doc.descendants(root.node).skip(1) {
            if doc.element(n).is_none() {
                continue;
            }
            if list.iter().any(|s| rabbithole_css::matches(s, doc, n)) {
                out.push(NodeRef { doc: root.doc, node: n });
                if first_only {
                    break;
                }
            }
        }
        out
    }))
}

pub(crate) fn element_matches(realm: &mut Realm, n: NodeRef, selectors: &str) -> JsResult<bool> {
    let list = parse_selectors(realm, selectors)?;
    Ok(with_doc(n.doc, |doc| doc.element(n.node).is_some() && list.iter().any(|s| rabbithole_css::matches(s, doc, n.node))))
}

// ---------------------------------------------------------------- mutation

fn is_connected(n: NodeRef) -> bool {
    (n.doc == 0 || n.doc & crate::host::FRAME_DOC != 0) && with_doc(n.doc, |doc| doc.is_connected(n.node))
}

/// Adopt `child` into `target_doc` (DOM "adopt"): it is removed from its
/// parent and, when the documents differ, its subtree moves to the arena of
/// the target document. Existing wrappers (and their listeners) follow the
/// moved nodes, so script references stay valid.
pub(crate) fn adopt(realm: &mut Realm, child: NodeRef, target_doc: u32) -> NodeRef {
    if child.doc == target_doc {
        return child;
    }
    remove_node(child);
    // (Main document, script-created documents and same-origin frame
    // documents all go through `with_doc`.)
    let src_clone: Document = with_doc(child.doc, |d| d.clone_subtree_readonly(child.node));
    let copied = with_doc(target_doc, |d| d.import_subtree(&src_clone, src_clone_root(&src_clone)));
    let old: Vec<NodeId> = with_doc(child.doc, |d| d.descendants(child.node).collect());
    let new: Vec<NodeId> = with_doc(target_doc, |d| d.descendants(copied).collect());
    let moves: Vec<(NodeRef, NodeRef)> = old.into_iter().zip(new).map(|(o, n)| (NodeRef { doc: child.doc, node: o }, NodeRef { doc: target_doc, node: n })).collect();
    repoint_wrappers(realm, &moves);
    // Inserting a fragment from another document moves its children out:
    // the source fragment is left empty.
    if is_fragment(child) {
        with_doc(child.doc, |d| d.clear_children(child.node));
    }
    {
        let st = state();
        let mut st = st.borrow_mut();
        for (from, to) in &moves {
            if st.ce_upgraded.remove(from) {
                st.ce_upgraded.insert(*to);
            }
            if st.ce_failed.remove(from) {
                st.ce_failed.insert(*to);
            }
        }
    }
    NodeRef { doc: target_doc, node: copied }
}

/// Make the wrappers, listeners and inline handlers of nodes that moved to
/// another document's arena refer to their new identity.
fn repoint_wrappers(realm: &mut Realm, moves: &[(NodeRef, NodeRef)]) {
    let st = state();
    let mut st = st.borrow_mut();
    for &(from, to) in moves {
        if let Some(obj) = st.wrappers.remove(&from) {
            if let Some(h) = realm.host_data_mut::<NodeRef>(obj) {
                *h = to;
            }
            st.wrappers.insert(to, obj);
        }
        if let Some(l) = st.listeners.remove(&TargetKey::Node(from)) {
            st.listeners.insert(TargetKey::Node(to), l);
        }
        let counts: Vec<(TargetKey, String)> = st.listener_counts.keys().filter(|(k, _)| *k == TargetKey::Node(from)).cloned().collect();
        for key in counts {
            if let Some(c) = st.listener_counts.remove(&key) {
                st.listener_counts.insert((TargetKey::Node(to), key.1), c);
            }
        }
        let handlers: Vec<(NodeRef, String)> = st.inline_handlers.keys().filter(|(k, _)| *k == from).cloned().collect();
        for key in handlers {
            if let Some(h) = st.inline_handlers.remove(&key) {
                st.inline_handlers.insert((to, key.1), h);
            }
        }
    }
}

/// What kind of node `n` is, for the pre-insertion checks.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Document,
    Doctype,
    Fragment,
    Element,
    Text,
    Other,
}

fn kind_of(doc: &Document, id: NodeId) -> Kind {
    match &doc.node(id).data {
        NodeData::Document => Kind::Document,
        NodeData::Doctype(_) => Kind::Doctype,
        NodeData::Element(e) if e.tag == "#fragment" || e.tag == "#shadow-root" => Kind::Fragment,
        NodeData::Element(_) => Kind::Element,
        NodeData::Text(_) => Kind::Text,
        NodeData::Comment(_) => Kind::Other,
    }
}

/// DOM "ensure pre-insertion validity" (and the matching checks of
/// "replace" when `replacing`): `child` is the reference child (insert) or
/// the child being replaced.
pub(crate) fn ensure_insert_validity(realm: &mut Realm, parent: NodeRef, node: NodeRef, child: Option<NodeId>, replacing: bool) -> JsResult<()> {
    let parent_kind = with_doc(parent.doc, |doc| kind_of(doc, parent.node));
    if !matches!(parent_kind, Kind::Document | Kind::Fragment | Kind::Element) {
        return Err(crate::misc::dom_exception(realm, "HierarchyRequestError", "This node type does not support this method."));
    }
    let (node_kind, frag_elements, frag_text) = with_doc(node.doc, |doc| {
        let k = kind_of(doc, node.node);
        let (mut elements, mut text) = (0, false);
        if k == Kind::Fragment {
            for c in doc.children(node.node) {
                match kind_of(doc, c) {
                    Kind::Element => elements += 1,
                    Kind::Text => text = true,
                    _ => {}
                }
            }
        }
        (k, elements, text)
    });
    let err = |realm: &mut Realm, msg: &str| Err(crate::misc::dom_exception(realm, "HierarchyRequestError", msg));
    match node_kind {
        Kind::Document => return err(realm, "Nodes of type '#document' may not be inserted inside nodes."),
        Kind::Text if parent_kind == Kind::Document => return err(realm, "Nodes of type '#text' may not be inserted inside nodes of type '#document'."),
        Kind::Doctype if parent_kind != Kind::Document => return err(realm, "Nodes of type 'DocumentType' may only be inserted inside a Document."),
        _ => {}
    }
    if parent_kind != Kind::Document {
        return Ok(());
    }
    let bad = with_doc(parent.doc, |doc| {
        let kids: Vec<NodeId> = doc.children(parent.node).collect();
        let others = |k: Kind| kids.iter().any(|&c| kind_of(doc, c) == k && (!replacing || Some(c) != child));
        let pos = child.and_then(|c| kids.iter().position(|&k| k == c));
        let doctype_after = pos.map(|i| kids[i + 1..].iter().any(|&c| kind_of(doc, c) == Kind::Doctype) || (!replacing && kind_of(doc, kids[i]) == Kind::Doctype)).unwrap_or(false);
        let element_before = pos.map(|i| kids[..i].iter().any(|&c| kind_of(doc, c) == Kind::Element)).unwrap_or(false);
        match node_kind {
            Kind::Fragment => frag_elements > 1 || frag_text || (frag_elements == 1 && (others(Kind::Element) || doctype_after)),
            Kind::Element => others(Kind::Element) || doctype_after,
            Kind::Doctype => others(Kind::Doctype) || element_before || (child.is_none() && others(Kind::Element)),
            _ => false,
        }
    });
    if bad {
        return err(realm, "Only one element and one doctype are allowed as children of a Document, in that order.");
    }
    Ok(())
}

fn src_clone_root(d: &Document) -> NodeId {
    d.first_child(Document::ROOT).unwrap_or(Document::ROOT)
}

trait SubtreeClone {
    fn clone_subtree_readonly(&self, root: NodeId) -> Document;
}

impl SubtreeClone for Document {
    /// A standalone document whose root child is a copy of `root`.
    fn clone_subtree_readonly(&self, root: NodeId) -> Document {
        let mut d = Document::new();
        let copy = d.import_subtree(self, root);
        d.append_child(Document::ROOT, copy);
        d
    }
}

/// Record a childList mutation for observers watching `parent`.
pub(crate) fn record_child_list(parent: NodeRef, added: Vec<NodeRef>, removed: Vec<NodeRef>) {
    crate::shadow::on_child_list(parent, &added, &removed);
    let st = state();
    let mut st = st.borrow_mut();
    if st.observers.is_empty() {
        return;
    }
    let mut hits: Vec<ObjectId> = Vec::new();
    for reg in &st.observers {
        if !reg.child_list || reg.target.doc != parent.doc {
            continue;
        }
        let ok = reg.target.node == parent.node
            || (reg.subtree && with_doc_in_state(&st, parent.doc, |doc| doc.is_ancestor(reg.target.node, parent.node)));
        if ok && !hits.contains(&reg.observer) {
            hits.push(reg.observer);
        }
    }
    for observer in hits {
        st.mutation_records.push(MutationRecord {
            observer,
            ty: "childList",
            target: parent,
            added: added.clone(),
            removed: removed.clone(),
            attribute_name: None,
            old_value: None,
        });
    }
}

pub(crate) fn record_attribute(target: NodeRef, name: &str, old: Option<String>) {
    let st = state();
    let mut st = st.borrow_mut();
    if st.observers.is_empty() {
        return;
    }
    let mut hits: Vec<(ObjectId, bool)> = Vec::new();
    for reg in &st.observers {
        if !reg.attributes || reg.target.doc != target.doc {
            continue;
        }
        if let Some(filter) = &reg.attribute_filter {
            if !filter.iter().any(|f| f == name) {
                continue;
            }
        }
        let ok = reg.target.node == target.node
            || (reg.subtree && with_doc_in_state(&st, target.doc, |doc| doc.is_ancestor(reg.target.node, target.node)));
        if ok && !hits.iter().any(|(o, _)| *o == reg.observer) {
            hits.push((reg.observer, reg.attribute_old_value));
        }
    }
    for (observer, want_old) in hits {
        st.mutation_records.push(MutationRecord {
            observer,
            ty: "attributes",
            target,
            added: Vec::new(),
            removed: Vec::new(),
            attribute_name: Some(name.to_string()),
            old_value: if want_old { old.clone() } else { None },
        });
    }
}

pub(crate) fn record_character_data(target: NodeRef, old: Option<String>) {
    let st = state();
    let mut st = st.borrow_mut();
    if st.observers.is_empty() {
        return;
    }
    let mut hits: Vec<ObjectId> = Vec::new();
    for reg in &st.observers {
        if !reg.character_data || reg.target.doc != target.doc {
            continue;
        }
        let ok = reg.target.node == target.node
            || (reg.subtree && with_doc_in_state(&st, target.doc, |doc| doc.is_ancestor(reg.target.node, target.node)));
        if ok && !hits.contains(&reg.observer) {
            hits.push(reg.observer);
        }
    }
    for observer in hits {
        st.mutation_records.push(MutationRecord { observer, ty: "characterData", target, added: Vec::new(), removed: Vec::new(), attribute_name: None, old_value: old.clone() });
    }
}

/// Document access while the state is already borrowed (main doc via env).
fn with_doc_in_state<R>(st: &HostState, doc: u32, f: impl FnOnce(&Document) -> R) -> R {
    if doc == 0 {
        with_env(|env| f(env.doc()))
    } else if doc & crate::host::FRAME_DOC != 0 {
        with_env(|env| match env.frame_document((doc & !crate::host::FRAME_DOC) as u64) {
            Some(d) => f(d),
            None => f(&Document::new()),
        })
    } else {
        f(&st.extra_docs[(doc - 1) as usize])
    }
}

/// Insert `child` (or a fragment's children) into `parent` before `before`.
pub(crate) fn insert_node(realm: &mut Realm, parent: NodeRef, child: NodeRef, before: Option<NodeId>) -> JsResult<()> {
    insert_node_checked(realm, parent, child, before, true)
}

/// `insert_node`; `validate` runs the pre-insertion checks (callers that
/// already ran the "replace" checks pass false).
pub(crate) fn insert_node_checked(realm: &mut Realm, parent: NodeRef, child: NodeRef, before: Option<NodeId>, validate: bool) -> JsResult<()> {
    if child.doc == parent.doc && (child.node == parent.node || with_doc(parent.doc, |doc| doc.is_ancestor(child.node, parent.node))) {
        return Err(crate::misc::dom_exception(realm, "HierarchyRequestError", "The new child element contains the parent."));
    }
    if validate {
        ensure_insert_validity(realm, parent, child, before, false)?;
    }
    let child = adopt(realm, child, parent.doc);
    let inserted: Vec<NodeRef> = if is_fragment(child) {
        let kids = children_of(child);
        with_doc(parent.doc, |doc| {
            for k in &kids {
                doc.insert_before(parent.node, k.node, before);
            }
        });
        kids
    } else {
        let old_parent = with_doc(parent.doc, |doc| doc.parent(child.node));
        with_doc(parent.doc, |doc| doc.insert_before(parent.node, child.node, before));
        if let Some(op) = old_parent {
            record_child_list(NodeRef { doc: parent.doc, node: op }, Vec::new(), vec![child]);
        }
        vec![child]
    };
    record_child_list(parent, inserted.clone(), Vec::new());
    if parent.doc == 0 {
        mark_dirty();
        if is_connected(parent) {
            after_insert_connected(realm, &inserted)?;
        }
    }
    crate::shadow::flush(realm);
    Ok(())
}

/// Run newly connected scripts and custom-element reactions.
fn after_insert_connected(realm: &mut Realm, inserted: &[NodeRef]) -> JsResult<()> {
    let mut scripts = Vec::new();
    {
        let st = state();
        let st = st.borrow();
        with_env(|env| {
            let doc = env.doc();
            for n in inserted {
                for d in doc.shadow_including_descendants(n.node) {
                    if doc.is_element(d, "script") && !st.ran_scripts.contains(&d) {
                        scripts.push(d);
                    }
                }
            }
        });
    }
    // Custom element reactions (queued by record_child_list) run first:
    // upgrades and connectedCallback in tree order.
    crate::shadow::drain_ce_reactions(realm);
    for s in scripts {
        run_script_element(realm, NodeRef::main(s));
    }
    Ok(())
}

/// Execute a `<script>` element (inline or external) now.
pub(crate) fn run_script_element(realm: &mut Realm, n: NodeRef) {
    if n.doc != 0 {
        return;
    }
    {
        let st = state();
        let mut st = st.borrow_mut();
        if !st.ran_scripts.insert(n.node) {
            return;
        }
    }
    let (ty, src, inline, nomodule) = with_doc(0, |doc| {
        let ty = doc.attr(n.node, "type").map(|t| t.trim().to_ascii_lowercase()).unwrap_or_default();
        let src = doc.attr(n.node, "src").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        (ty, src, doc.text_content(n.node), doc.attr(n.node, "nomodule").is_some())
    });
    let is_module = ty == "module";
    let is_js = ty.is_empty()
        || is_module
        || matches!(ty.as_str(), "text/javascript" | "application/javascript" | "text/ecmascript" | "application/ecmascript" | "text/jscript");
    if !is_js || nomodule {
        return;
    }
    let (source, url) = match src {
        Some(s) if s.starts_with("blob:") => match crate::fetch::do_fetch(&s, "GET", &[], &[]) {
            Ok(r) => (String::from_utf8_lossy(&r.body).into_owned(), s),
            Err(_) => {
                state().borrow_mut().pending_script_events.push((n, "error"));
                schedule_script_events(realm);
                return;
            }
        },
        Some(s) => {
            let Some(url) = resolve_url(&s) else {
                state().borrow_mut().pending_script_events.push((n, "error"));
                schedule_script_events(realm);
                return;
            };
            let req = with_env(|env| rabbithole_net::Request::get(url.clone()).with_dest(rabbithole_net::Destination::Script).with_referer(env.document_url()));
            // Like browsers, download injected scripts without blocking the
            // inserting script; they run when the download completes.
            if let Some(rx) = with_env(|env| env.fetch_background(req.clone())) {
                state().borrow_mut().pending_script_fetches.push((n, url.to_string(), is_module, rx));
                return;
            }
            let resp = with_env(|env| env.fetch(req));
            match resp {
                Ok(r) if r.status < 400 => (rabbithole_net::decode_text(&r.body, r.charset.as_deref()), url.to_string()),
                _ => {
                    state().borrow_mut().pending_script_events.push((n, "error"));
                    schedule_script_events(realm);
                    return;
                }
            }
        }
        None => (inline, with_env(|env| env.document_url().to_string())),
    };
    let prev = state().borrow().current_script;
    state().borrow_mut().current_script = Some(n);
    let r = if is_module { realm.eval_module(&source, &url) } else { realm.eval(&source, &url) };
    state().borrow_mut().current_script = prev;
    if let Err(e) = r {
        report_error(realm, &e, &url);
    }
    state().borrow_mut().pending_script_events.push((n, "load"));
    schedule_script_events(realm);
}

/// Run dynamically inserted scripts whose background download finished.
/// Returns whether any ran.
pub(crate) fn run_ready_script_fetches(realm: &mut Realm) -> bool {
    let ready: Vec<(NodeRef, String, bool, Result<rabbithole_net::Response, String>)> = {
        let st = state();
        let mut st = st.borrow_mut();
        let mut ready = Vec::new();
        let mut i = 0;
        while i < st.pending_script_fetches.len() {
            match st.pending_script_fetches[i].3.try_recv() {
                Ok(r) => {
                    let (n, url, m, _) = st.pending_script_fetches.remove(i);
                    ready.push((n, url, m, r));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    let (n, url, m, _) = st.pending_script_fetches.remove(i);
                    ready.push((n, url, m, Err("download failed".into())));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => i += 1,
            }
        }
        ready
    };
    let any = !ready.is_empty();
    for (n, url, is_module, r) in ready {
        match r {
            Ok(resp) if resp.status < 400 => {
                let source = rabbithole_net::decode_text(&resp.body, resp.charset.as_deref());
                let prev = state().borrow().current_script;
                state().borrow_mut().current_script = Some(n);
                let dbg = std::env::var_os("RH_DEBUG_SCRIPTS").is_some();
                let t = std::time::Instant::now();
                if dbg {
                    eprintln!("[script] async start {url} ({} bytes)", source.len());
                }
                // A task of its own; large bundles get time to compile.
                realm.begin_task_with(crate::script_budget(source.len()));
                let r = if is_module { realm.eval_module(&source, &url) } else { realm.eval(&source, &url) };
                if dbg {
                    eprintln!("[script] async done  {url} in {:?}", t.elapsed());
                }
                state().borrow_mut().current_script = prev;
                if let Err(e) = r {
                    report_error(realm, &e, &url);
                }
                state().borrow_mut().pending_script_events.push((n, "load"));
            }
            _ => state().borrow_mut().pending_script_events.push((n, "error")),
        }
        schedule_script_events(realm);
    }
    if any {
        realm.begin_task();
        realm.run_jobs();
    }
    any
}

fn schedule_script_events(realm: &mut Realm) {
    let f = state().borrow().protos.get("__deliver_script_events").copied();
    if let Some(f) = f {
        realm.enqueue_callback(Value::Object(f), Value::Undefined, Vec::new());
    }
}

/// Job: fire pending load/error events on script elements.
pub(crate) fn deliver_script_events(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    let pending = std::mem::take(&mut state().borrow_mut().pending_script_events);
    for (n, ty) in pending {
        crate::events::dispatch(realm, TargetKey::Node(n), ty, crate::EventInit { bubbles: false, cancelable: false, detail: None }, true);
    }
    Ok(Value::Undefined)
}

/// Remove a node from its parent.
pub(crate) fn remove_node(child: NodeRef) {
    let parent = with_doc(child.doc, |doc| doc.parent(child.node));
    let Some(p) = parent else { return };
    with_doc(child.doc, |doc| doc.detach(child.node));
    record_child_list(NodeRef { doc: child.doc, node: p }, Vec::new(), vec![child]);
    if child.doc == 0 {
        mark_dirty();
    }
}

/// Replace all children with a single text node (or nothing when empty).
pub(crate) fn set_text_content(n: NodeRef, text: &str) {
    let old: Vec<NodeRef> = children_of(n);
    let added = with_doc(n.doc, |doc| {
        doc.clear_children(n.node);
        if text.is_empty() {
            None
        } else {
            let t = doc.create_text(text);
            doc.append_child(n.node, t);
            Some(NodeRef { doc: n.doc, node: t })
        }
    });
    record_child_list(n, added.into_iter().collect(), old);
    if n.doc == 0 {
        mark_dirty();
    }
}

/// Parse HTML and insert the resulting nodes as children of `n`.
pub(crate) fn set_inner_html(realm: &mut Realm, n: NodeRef, html: &str) -> JsResult<()> {
    let context = tag_of(n).unwrap_or_else(|| "div".into());
    // Raw text (script, style, ...) and RCDATA (textarea, title) elements
    // take the markup as text (HTML fragment parsing algorithm, §13.2.6.4).
    let raw = matches!(context.as_str(), "script" | "style" | "xmp" | "iframe" | "noembed" | "noframes" | "plaintext");
    let rcdata = matches!(context.as_str(), "textarea" | "title");
    if raw || rcdata {
        let text = if rcdata { rabbithole_html::entities::decode(html, false) } else { html.to_string() };
        with_doc(n.doc, |doc| {
            doc.clear_children(n.node);
            if !text.is_empty() {
                let t = doc.create_text(&text);
                doc.append_child(n.node, t);
            }
        });
        crate::host::mark_dirty();
        return Ok(());
    }
    // A template's markup is its content's.
    let n = if context == "template" { NodeRef { doc: n.doc, node: with_doc(n.doc, |doc| doc.template_content(n.node)) } } else { n };
    let frag = rabbithole_html::parse_fragment_with_scripting(html, &context, scripting_doc(n.doc));
    let frag_body = frag.body().unwrap_or(Document::ROOT);
    let old = children_of(n);
    let added = with_doc(n.doc, |doc| {
        doc.clear_children(n.node);
        let mut added = Vec::new();
        for c in frag.children(frag_body) {
            let copy = doc.import_subtree(&frag, c);
            doc.append_child(n.node, copy);
            added.push(NodeRef { doc: n.doc, node: copy });
        }
        added
    });
    // Scripts created through innerHTML never run.
    if n.doc == 0 {
        let st = state();
        let mut st = st.borrow_mut();
        with_env(|env| {
            let doc = env.doc();
            for a in &added {
                for d in doc.descendants(a.node) {
                    if doc.is_element(d, "script") {
                        st.ran_scripts.insert(d);
                    }
                }
            }
        });
    }
    record_child_list(n, added, old);
    if n.doc == 0 {
        mark_dirty();
    }
    let _ = realm;
    Ok(())
}

/// Does the document run scripts? The page and its frames do; auxiliary
/// documents (DOMParser, `createHTMLDocument`) have scripting disabled, so
/// `<noscript>` parses as markup there and as raw text here.
fn scripting_doc(doc: u32) -> bool {
    doc == 0 || doc & crate::host::FRAME_DOC != 0
}

/// Nodes produced by parsing `html` in the context of `ctx`, imported into
/// the document of `ctx` (detached, in order).
pub(crate) fn parse_html_into(ctx: NodeRef, html: &str) -> Vec<NodeRef> {
    let context = tag_of(ctx).filter(|t| t != "#fragment").unwrap_or_else(|| "div".into());
    let frag = rabbithole_html::parse_fragment_with_scripting(html, &context, scripting_doc(ctx.doc));
    let frag_body = frag.body().unwrap_or(Document::ROOT);
    with_doc(ctx.doc, |doc| {
        let mut out = Vec::new();
        for c in frag.children(frag_body) {
            let copy = doc.import_subtree(&frag, c);
            out.push(NodeRef { doc: ctx.doc, node: copy });
        }
        out
    })
}

/// Convert a `Node | string` argument to a node ref (strings become text nodes).
pub(crate) fn node_or_text(realm: &mut Realm, doc: u32, v: &Value) -> JsResult<NodeRef> {
    if let Some(n) = node_of(realm, v) {
        return Ok(n);
    }
    let s = realm.to_rust_string(v.clone())?;
    Ok(with_doc(doc, |d| NodeRef { doc, node: d.create_text(&s) }))
}

/// `innerText`-like text with newlines between block-level elements.
pub(crate) fn inner_text(n: NodeRef) -> String {
    const BLOCK: &[&str] = &["p", "div", "br", "li", "tr", "h1", "h2", "h3", "h4", "h5", "h6", "section", "article", "header", "footer", "ul", "ol", "pre", "blockquote", "table", "dd", "dt", "hr", "nav", "main", "aside", "form", "fieldset", "figure", "figcaption", "address"];
    with_doc(n.doc, |doc| {
        let mut out = String::new();
        fn walk(doc: &Document, id: NodeId, out: &mut String) {
            match &doc.node(id).data {
                NodeData::Text(t) => out.push_str(t),
                NodeData::Element(e) => {
                    if matches!(e.tag.as_str(), "script" | "style" | "template" | "noscript") {
                        return;
                    }
                    let block = BLOCK.contains(&e.tag.as_str());
                    if block && !out.is_empty() && !out.ends_with('\n') {
                        out.push('\n');
                    }
                    if e.tag == "br" {
                        out.push('\n');
                        return;
                    }
                    for c in doc.children(id) {
                        walk(doc, c, out);
                    }
                    if block && !out.ends_with('\n') {
                        out.push('\n');
                    }
                }
                _ => {
                    for c in doc.children(id) {
                        walk(doc, c, out);
                    }
                }
            }
        }
        walk(doc, n.node, &mut out);
        // Collapse runs of whitespace like rendered text would.
        let mut res = String::with_capacity(out.len());
        let mut prev_space = true;
        for ch in out.chars() {
            if ch == '\n' {
                while res.ends_with(' ') {
                    res.pop();
                }
                res.push('\n');
                prev_space = true;
            } else if ch.is_whitespace() {
                if !prev_space {
                    res.push(' ');
                    prev_space = true;
                }
            } else {
                res.push(ch);
                prev_space = false;
            }
        }
        res.trim_matches('\n').to_string()
    })
}

// ----------------------------------------------------------------- natives

fn n_node_type(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "nodeType")?;
    Ok(Value::Number(node_type(n)))
}

fn n_node_name(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "nodeName")?;
    Ok(Value::str(&node_name(n)))
}

fn n_node_value_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "nodeValue")?;
    Ok(with_doc(n.doc, |doc| match &doc.node(n.node).data {
        NodeData::Text(t) | NodeData::Comment(t) => Value::str(t),
        _ => Value::Null,
    }))
}

fn n_node_value_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "nodeValue")?;
    let v = arg_str(realm, args, 0)?;
    set_character_data(n, &v);
    Ok(Value::Undefined)
}

pub(crate) fn set_character_data(n: NodeRef, v: &str) {
    let old = with_doc(n.doc, |doc| doc.set_char_data(n.node, v));
    if old.is_some() {
        record_character_data(n, old);
        if n.doc == 0 {
            mark_dirty();
        }
    }
}

fn n_text_content_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "textContent")?;
    Ok(with_doc(n.doc, |doc| match &doc.node(n.node).data {
        NodeData::Document | NodeData::Doctype(_) => Value::Null,
        NodeData::Text(t) | NodeData::Comment(t) => Value::str(t),
        _ => Value::str(&doc.text_content(n.node)),
    }))
}

fn n_text_content_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "textContent")?;
    let v = match args.first().map(Value::v) {
        None | Some(V::Null) | Some(V::Undefined) => String::new(),
        Some(v) => realm.to_rust_string(v.to_value())?,
    };
    let (is_char, is_leaf) = with_doc(n.doc, |doc| match doc.node(n.node).data {
        NodeData::Text(_) | NodeData::Comment(_) => (true, false),
        NodeData::Document | NodeData::Doctype(_) => (false, true),
        _ => (false, false),
    });
    if is_char {
        set_character_data(n, &v);
    } else if !is_leaf {
        set_text_content(n, &v);
    }
    Ok(Value::Undefined)
}

fn n_parent_node(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "parentNode")?;
    let p = with_doc(n.doc, |doc| doc.parent(n.node));
    Ok(wrap_opt(realm, n.doc, p))
}

fn n_parent_element(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "parentElement")?;
    let p = with_doc(n.doc, |doc| doc.parent(n.node).filter(|&p| doc.element(p).is_some()));
    Ok(wrap_opt(realm, n.doc, p))
}

fn n_child_nodes(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "childNodes")?;
    let kids = children_of(n);
    Ok(node_list(realm, kids))
}

fn n_children(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "children")?;
    let kids = element_children_of(n);
    Ok(html_collection(realm, kids))
}

fn n_first_child(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "firstChild")?;
    let c = with_doc(n.doc, |doc| doc.first_child(n.node));
    Ok(wrap_opt(realm, n.doc, c))
}

fn n_last_child(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "lastChild")?;
    let c = with_doc(n.doc, |doc| doc.last_child(n.node));
    Ok(wrap_opt(realm, n.doc, c))
}

fn n_prev_sibling(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "previousSibling")?;
    let c = with_doc(n.doc, |doc| doc.prev_sibling(n.node));
    Ok(wrap_opt(realm, n.doc, c))
}

fn n_next_sibling(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "nextSibling")?;
    let c = with_doc(n.doc, |doc| doc.next_sibling(n.node));
    Ok(wrap_opt(realm, n.doc, c))
}

fn n_first_element_child(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "firstElementChild")?;
    let c = with_doc(n.doc, |doc| doc.child_elements(n.node).next());
    Ok(wrap_opt(realm, n.doc, c))
}

fn n_last_element_child(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "lastElementChild")?;
    let c = with_doc(n.doc, |doc| doc.child_elements(n.node).last());
    Ok(wrap_opt(realm, n.doc, c))
}

fn n_child_element_count(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "childElementCount")?;
    Ok(Value::Number(with_doc(n.doc, |doc| doc.child_elements(n.node).count()) as f64))
}

fn n_prev_element_sibling(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "previousElementSibling")?;
    let c = with_doc(n.doc, |doc| {
        let mut cur = doc.prev_sibling(n.node);
        while let Some(s) = cur {
            if doc.element(s).is_some() {
                return Some(s);
            }
            cur = doc.prev_sibling(s);
        }
        None
    });
    Ok(wrap_opt(realm, n.doc, c))
}

fn n_next_element_sibling(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "nextElementSibling")?;
    let c = with_doc(n.doc, |doc| {
        let mut cur = doc.next_sibling(n.node);
        while let Some(s) = cur {
            if doc.element(s).is_some() {
                return Some(s);
            }
            cur = doc.next_sibling(s);
        }
        None
    });
    Ok(wrap_opt(realm, n.doc, c))
}

fn n_owner_document(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "ownerDocument")?;
    if node_type(n) == DOCUMENT_NODE {
        return Ok(Value::Null);
    }
    Ok(wrap(realm, NodeRef { doc: n.doc, node: Document::ROOT }))
}

fn n_is_connected(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "isConnected")?;
    Ok(Value::Bool(with_doc(n.doc, |doc| doc.is_connected(n.node))))
}

fn n_base_uri(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let _ = this_node(realm, &this, "baseURI")?;
    Ok(Value::str(&base_url().to_string()))
}

fn n_append_child(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "appendChild")?;
    let child = match node_of(realm, &arg(args, 0)) {
        Some(c) => c,
        None => return Err(realm.throw_type_error("Failed to execute 'appendChild' on 'Node': parameter 1 is not of type 'Node'.")),
    };
    insert_node(realm, n, child, None)?;
    Ok(arg(args, 0))
}

fn n_insert_before(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "insertBefore")?;
    let child = match node_of(realm, &arg(args, 0)) {
        Some(c) => c,
        None => return Err(realm.throw_type_error("Failed to execute 'insertBefore' on 'Node': parameter 1 is not of type 'Node'.")),
    };
    let before = match (args.get(1)).map(Value::v) {
        None | Some(V::Null) | Some(V::Undefined) => None,
        Some(v) => match node_of(realm, &v.to_value()) {
            Some(r) => {
                if with_doc(n.doc, |doc| doc.parent(r.node)) != Some(n.node) {
                    return Err(crate::misc::dom_exception(realm, "NotFoundError", "The node before which the new node is to be inserted is not a child of this node."));
                }
                Some(r.node)
            }
            None => return Err(realm.throw_type_error("Failed to execute 'insertBefore' on 'Node': parameter 2 is not of type 'Node'.")),
        },
    };
    insert_node(realm, n, child, before)?;
    Ok(arg(args, 0))
}

fn n_remove_child(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "removeChild")?;
    let child = match node_of(realm, &arg(args, 0)) {
        Some(c) => c,
        None => return Err(realm.throw_type_error("Failed to execute 'removeChild' on 'Node': parameter 1 is not of type 'Node'.")),
    };
    if with_doc(n.doc, |doc| doc.parent(child.node)) != Some(n.node) || child.doc != n.doc {
        return Err(crate::misc::dom_exception(realm, "NotFoundError", "The node to be removed is not a child of this node."));
    }
    remove_node(child);
    Ok(arg(args, 0))
}

fn n_replace_child(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "replaceChild")?;
    let new = node_of(realm, &arg(args, 0));
    let old = node_of(realm, &arg(args, 1));
    let (Some(new), Some(old)) = (new, old) else {
        return Err(realm.throw_type_error("Failed to execute 'replaceChild' on 'Node': parameters must be Nodes."));
    };
    if with_doc(n.doc, |doc| doc.parent(old.node)) != Some(n.node) {
        return Err(crate::misc::dom_exception(realm, "NotFoundError", "The node to be replaced is not a child of this node."));
    }
    if new.doc == n.doc && (new.node == n.node || with_doc(n.doc, |doc| doc.is_ancestor(new.node, n.node))) {
        return Err(crate::misc::dom_exception(realm, "HierarchyRequestError", "The new child element contains the parent."));
    }
    ensure_insert_validity(realm, n, new, Some(old.node), true)?;
    let next = with_doc(n.doc, |doc| doc.next_sibling(old.node));
    let next = next.filter(|nx| Some(*nx) != Some(new.node));
    remove_node(old);
    insert_node_checked(realm, n, new, next, false)?;
    Ok(arg(args, 1))
}

fn n_remove(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "remove")?;
    remove_node(n);
    Ok(Value::Undefined)
}

fn insert_many(realm: &mut Realm, parent: NodeRef, args: &[Value], before: Option<NodeId>) -> JsResult<()> {
    for a in args {
        let c = node_or_text(realm, parent.doc, a)?;
        insert_node(realm, parent, c, before)?;
    }
    Ok(())
}

fn n_append(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "append")?;
    insert_many(realm, n, args, None)?;
    Ok(Value::Undefined)
}

fn n_prepend(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "prepend")?;
    let first = with_doc(n.doc, |doc| doc.first_child(n.node));
    insert_many(realm, n, args, first)?;
    Ok(Value::Undefined)
}

fn n_before(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "before")?;
    let parent = with_doc(n.doc, |doc| doc.parent(n.node));
    if let Some(p) = parent {
        insert_many(realm, NodeRef { doc: n.doc, node: p }, args, Some(n.node))?;
    }
    Ok(Value::Undefined)
}

fn n_after(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "after")?;
    let (parent, next) = with_doc(n.doc, |doc| (doc.parent(n.node), doc.next_sibling(n.node)));
    if let Some(p) = parent {
        insert_many(realm, NodeRef { doc: n.doc, node: p }, args, next)?;
    }
    Ok(Value::Undefined)
}

fn n_replace_with(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "replaceWith")?;
    let (parent, next) = with_doc(n.doc, |doc| (doc.parent(n.node), doc.next_sibling(n.node)));
    if let Some(p) = parent {
        remove_node(n);
        insert_many(realm, NodeRef { doc: n.doc, node: p }, args, next)?;
    }
    Ok(Value::Undefined)
}

fn n_replace_children(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "replaceChildren")?;
    let old = children_of(n);
    with_doc(n.doc, |doc| doc.clear_children(n.node));
    record_child_list(n, Vec::new(), old);
    insert_many(realm, n, args, None)?;
    if n.doc == 0 {
        mark_dirty();
    }
    Ok(Value::Undefined)
}

fn n_clone_node(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "cloneNode")?;
    let deep = realm.to_boolean(&arg(args, 0));
    let copy = with_doc(n.doc, |doc| doc.clone_subtree(n.node, deep));
    let copy = NodeRef { doc: n.doc, node: copy };
    crate::shadow::queue_upgrades(copy);
    crate::shadow::drain_ce_reactions(realm);
    Ok(wrap(realm, copy))
}

fn n_contains(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "contains")?;
    let other = node_of(realm, &arg(args, 0));
    Ok(Value::Bool(match other {
        Some(o) if o.doc == n.doc => o.node == n.node || with_doc(n.doc, |doc| doc.is_ancestor(n.node, o.node)),
        _ => false,
    }))
}

fn n_compare_document_position(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "compareDocumentPosition")?;
    let Some(o) = node_of(realm, &arg(args, 0)) else {
        return Err(realm.throw_type_error("parameter 1 is not of type 'Node'"));
    };
    if o == n {
        return Ok(Value::Number(0.0));
    }
    if o.doc != n.doc {
        return Ok(Value::Number(1.0 + 32.0 + 4.0)); // disconnected, implementation-specific
    }
    let r = with_doc(n.doc, |doc| {
        if doc.is_ancestor(o.node, n.node) {
            return 8.0 + 2.0; // CONTAINS | PRECEDING
        }
        if doc.is_ancestor(n.node, o.node) {
            return 16.0 + 4.0; // CONTAINED_BY | FOLLOWING
        }
        // Document order comparison.
        let mut seen_n = false;
        let mut seen_o = false;
        for d in doc.descendants(Document::ROOT) {
            if d == n.node {
                seen_n = true;
            }
            if d == o.node {
                seen_o = true;
            }
            if seen_n && !seen_o {
                return 4.0;
            }
            if seen_o && !seen_n {
                return 2.0;
            }
        }
        1.0 + 32.0 + 4.0
    });
    Ok(Value::Number(r))
}

fn n_is_equal_node(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "isEqualNode")?;
    let Some(o) = node_of(realm, &arg(args, 0)) else { return Ok(Value::Bool(false)) };
    let a = with_doc(n.doc, |doc| doc.serialize(n.node, true));
    let b = with_doc(o.doc, |doc| doc.serialize(o.node, true));
    Ok(Value::Bool(a == b))
}

fn n_is_same_node(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "isSameNode")?;
    Ok(Value::Bool(node_of(realm, &arg(args, 0)) == Some(n)))
}

fn n_has_child_nodes(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "hasChildNodes")?;
    Ok(Value::Bool(with_doc(n.doc, |doc| doc.first_child(n.node).is_some())))
}

fn n_normalize(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "normalize")?;
    with_doc(n.doc, |doc| {
        let nodes: Vec<NodeId> = doc.descendants(n.node).collect();
        for id in nodes {
            if !doc.is_text(id) {
                continue;
            }
            if doc.node(id).parent.is_none() {
                continue;
            }
            // Merge following text siblings into this one.
            loop {
                let Some(next) = doc.next_sibling(id) else { break };
                if !doc.is_text(next) {
                    break;
                }
                let t = doc.text(next).unwrap_or("").to_string();
                let cur = doc.text(id).unwrap_or("").to_string();
                doc.set_text(id, &format!("{cur}{t}"));
                doc.detach(next);
            }
            if doc.text(id).map(|t| t.is_empty()).unwrap_or(false) {
                doc.detach(id);
            }
        }
    });
    Ok(Value::Undefined)
}

fn n_get_root_node(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "getRootNode")?;
    let composed = dict_bool(realm, &arg(args, 0), "composed")?.unwrap_or(false);
    let root = with_doc(n.doc, |doc| {
        let mut cur = n.node;
        loop {
            while let Some(p) = doc.parent(cur) {
                cur = p;
            }
            match doc.shadow_host(cur) {
                Some(h) if composed => cur = h,
                _ => break,
            }
        }
        cur
    });
    Ok(wrap(realm, NodeRef { doc: n.doc, node: root }))
}

fn n_null(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Null)
}

fn n_false(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(false))
}

fn n_query_selector(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "querySelector")?;
    let s = arg_str(realm, args, 0)?;
    let found = query_all(realm, n, &s, true)?;
    Ok(match found.first() {
        Some(f) => wrap(realm, *f),
        None => Value::Null,
    })
}

fn n_query_selector_all(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "querySelectorAll")?;
    let s = arg_str(realm, args, 0)?;
    let found = query_all(realm, n, &s, false)?;
    Ok(node_list(realm, found))
}

fn n_get_elements_by_tag_name(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "getElementsByTagName")?;
    let tag = arg_str(realm, args, 0)?.to_ascii_lowercase();
    let found = with_doc(n.doc, |doc| {
        doc.descendants(n.node)
            .skip(1)
            .filter(|&d| doc.element(d).is_some() && (tag == "*" || doc.tag(d) == Some(tag.as_str())))
            .map(|d| NodeRef { doc: n.doc, node: d })
            .collect::<Vec<_>>()
    });
    Ok(html_collection(realm, found))
}

fn n_get_elements_by_class_name(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "getElementsByClassName")?;
    let classes: Vec<String> = arg_str(realm, args, 0)?.split_whitespace().map(|s| s.to_string()).collect();
    let found = with_doc(n.doc, |doc| {
        doc.descendants(n.node)
            .skip(1)
            .filter(|&d| doc.element(d).map(|e| !classes.is_empty() && classes.iter().all(|c| e.has_class(c))).unwrap_or(false))
            .map(|d| NodeRef { doc: n.doc, node: d })
            .collect::<Vec<_>>()
    });
    Ok(html_collection(realm, found))
}

// CharacterData

fn cd_data_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "data")?;
    Ok(with_doc(n.doc, |doc| match &doc.node(n.node).data {
        NodeData::Text(t) | NodeData::Comment(t) => Value::str(t),
        _ => Value::str(""),
    }))
}

fn cd_data_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "data")?;
    let v = arg_str(realm, args, 0)?;
    set_character_data(n, &v);
    Ok(Value::Undefined)
}

fn cd_length(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "length")?;
    Ok(Value::Number(with_doc(n.doc, |doc| doc.char_data(n.node).map(|t| t.encode_utf16().count()).unwrap_or(0)) as f64))
}

fn cd_append_data(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "appendData")?;
    let v = arg_str(realm, args, 0)?;
    let len = with_doc(n.doc, |doc| doc.char_data(n.node).map(|t| t.encode_utf16().count()).unwrap_or(0));
    replace_character_data(n, len, 0, &v);
    Ok(Value::Undefined)
}

/// DOM "replace data" (live ranges follow); offsets in UTF-16 code units.
pub(crate) fn replace_character_data(n: NodeRef, offset: usize, count: usize, data: &str) {
    let old = with_doc(n.doc, |doc| doc.replace_data(n.node, offset, count, data));
    if old.is_some() {
        record_character_data(n, old);
        if n.doc == 0 {
            mark_dirty();
        }
    }
}

/// `CharacterData.replaceData(offset, count, data)`.
fn cd_replace_data(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "replaceData")?;
    let off = realm.to_number(arg(args, 0))? as i64 as u32 as usize;
    let count = realm.to_number(arg(args, 1))? as i64 as u32 as usize;
    let data = arg_str(realm, args, 2)?;
    let len = with_doc(n.doc, |doc| doc.char_data(n.node).map(|t| t.encode_utf16().count()).unwrap_or(0));
    if off > len {
        return Err(crate::misc::dom_exception(realm, "IndexSizeError", &format!("The offset {off} is greater than the node's length ({len}).")));
    }
    replace_character_data(n, off, count, &data);
    Ok(Value::Undefined)
}

fn cd_substring_data(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "substringData")?;
    let off = realm.to_number(arg(args, 0))?.max(0.0) as usize;
    let count = realm.to_number(arg(args, 1))?.max(0.0) as usize;
    let cur = with_doc(n.doc, |doc| doc.char_data(n.node).unwrap_or("").to_string());
    let units: Vec<u16> = cur.encode_utf16().collect();
    let end = (off + count).min(units.len());
    let s = String::from_utf16_lossy(&units[off.min(units.len())..end]);
    Ok(Value::str(&s))
}

fn text_split(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "splitText")?;
    let off = realm.to_number(arg(args, 0))? as i64 as u32 as usize;
    let (old, len, parent) = with_doc(n.doc, |doc| {
        let t = doc.char_data(n.node).unwrap_or("").to_string();
        let len = t.encode_utf16().count();
        (t, len, doc.parent(n.node))
    });
    if off > len {
        return Err(crate::misc::dom_exception(realm, "IndexSizeError", &format!("The offset {off} is larger than the Text node's length.")));
    }
    let new = with_doc(n.doc, |doc| doc.split_text(n.node, off));
    let new = NodeRef { doc: n.doc, node: new };
    if let Some(p) = parent {
        record_child_list(NodeRef { doc: n.doc, node: p }, vec![new], Vec::new());
    }
    record_character_data(n, Some(old));
    if n.doc == 0 {
        mark_dirty();
    }
    Ok(wrap(realm, new))
}

fn text_whole(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "wholeText")?;
    let s = with_doc(n.doc, |doc| {
        let mut first = n.node;
        while let Some(p) = doc.prev_sibling(first) {
            if !doc.is_text(p) {
                break;
            }
            first = p;
        }
        let mut out = String::new();
        let mut cur = Some(first);
        while let Some(c) = cur {
            if !doc.is_text(c) {
                break;
            }
            out.push_str(doc.text(c).unwrap_or(""));
            cur = doc.next_sibling(c);
        }
        out
    });
    Ok(Value::str(&s))
}

// DocumentType

fn doctype_field(realm: &mut Realm, this: &Value, f: fn(&rabbithole_dom::DoctypeData) -> &str) -> JsResult<Value> {
    let n = this_node(realm, this, "DocumentType")?;
    Ok(Value::str(&with_doc(n.doc, |doc| match &doc.node(n.node).data {
        NodeData::Doctype(d) => f(d).to_string(),
        _ => String::new(),
    })))
}

fn doctype_name(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    doctype_field(realm, &this, |d| &d.name)
}

fn doctype_public_id(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    doctype_field(realm, &this, |d| &d.public_id)
}

fn doctype_system_id(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    doctype_field(realm, &this, |d| &d.system_id)
}

fn text_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let s = arg_str_opt(realm, args, 0)?.unwrap_or_default();
    let node = with_doc(0, |doc| doc.create_text(&s));
    Ok(wrap_main(realm, node))
}

fn comment_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let s = arg_str_opt(realm, args, 0)?.unwrap_or_default();
    let node = with_doc(0, |doc| doc.create(NodeData::Comment(s)));
    Ok(wrap_main(realm, node))
}

fn fragment_ctor(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    let node = with_doc(0, |doc| doc.create_element("#fragment", vec![]));
    Ok(wrap_main(realm, node))
}

fn fragment_get_element_by_id(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "getElementById")?;
    let id = arg_str(realm, args, 0)?;
    let found = with_doc(n.doc, |doc| doc.descendants(n.node).skip(1).find(|&d| doc.attr(d, "id") == Some(id.as_str())));
    Ok(wrap_opt(realm, n.doc, found))
}

pub(crate) fn define_hidden_method_item(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let i = realm.to_number(arg(args, 0))?;
    if i < 0.0 || i.is_nan() {
        return Ok(Value::Null);
    }
    let v = realm.get(this, rabbithole_js::PropertyKey::index(i as u32))?;
    Ok(if v.is_undefined() { Value::Null } else { v })
}

fn collection_named_item(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let name = arg_str(realm, args, 0)?;
    let v = realm.get(this, rabbithole_js::PropertyKey::str(&name))?;
    Ok(if v.is_undefined() { Value::Null } else { v })
}


/// Install Node, CharacterData, Text, Comment, DocumentFragment, NodeList,
/// HTMLCollection. Requires EventTarget to exist.
pub(crate) fn install(realm: &mut Realm) {
    let node = define_class(realm, "Node", Some("EventTarget"), illegal_ctor);
    let consts: [(&str, f64); 12] = [
        ("ELEMENT_NODE", 1.0),
        ("ATTRIBUTE_NODE", 2.0),
        ("TEXT_NODE", 3.0),
        ("CDATA_SECTION_NODE", 4.0),
        ("PROCESSING_INSTRUCTION_NODE", 7.0),
        ("COMMENT_NODE", 8.0),
        ("DOCUMENT_NODE", 9.0),
        ("DOCUMENT_TYPE_NODE", 10.0),
        ("DOCUMENT_FRAGMENT_NODE", 11.0),
        ("DOCUMENT_POSITION_DISCONNECTED", 1.0),
        ("DOCUMENT_POSITION_PRECEDING", 2.0),
        ("DOCUMENT_POSITION_FOLLOWING", 4.0),
    ];
    let ctor = realm.obj(node).props.get(&rabbithole_js::PropertyKey::str("constructor")).map(|p| p.value.clone());
    for (k, v) in consts {
        realm.define_property(node, k, Value::Number(v), 0);
        if let Some(V::Object(c)) = ctor.as_ref().map(Value::v) {
            realm.define_property(c, k, Value::Number(v), 0);
        }
    }
    if let Some(V::Object(c)) = ctor.as_ref().map(Value::v) {
        realm.define_property(c, "DOCUMENT_POSITION_CONTAINS", Value::Number(8.0), 0);
        realm.define_property(c, "DOCUMENT_POSITION_CONTAINED_BY", Value::Number(16.0), 0);
    }
    let acc = |realm: &mut Realm, name: &str, g: rabbithole_js::NativeFn| realm.define_accessor(node, name, Some(g), None, rabbithole_js::CONFIGURABLE | rabbithole_js::ENUMERABLE);
    acc(realm, "nodeType", n_node_type);
    acc(realm, "nodeName", n_node_name);
    acc(realm, "parentNode", n_parent_node);
    acc(realm, "parentElement", n_parent_element);
    acc(realm, "childNodes", n_child_nodes);
    acc(realm, "firstChild", n_first_child);
    acc(realm, "lastChild", n_last_child);
    acc(realm, "previousSibling", n_prev_sibling);
    acc(realm, "nextSibling", n_next_sibling);
    acc(realm, "ownerDocument", n_owner_document);
    acc(realm, "isConnected", n_is_connected);
    acc(realm, "baseURI", n_base_uri);
    realm.define_accessor(node, "nodeValue", Some(n_node_value_get), Some(n_node_value_set), rabbithole_js::CONFIGURABLE | rabbithole_js::ENUMERABLE);
    realm.define_accessor(node, "textContent", Some(n_text_content_get), Some(n_text_content_set), rabbithole_js::CONFIGURABLE | rabbithole_js::ENUMERABLE);
    let m = |realm: &mut Realm, name: &str, arity: u32, f: rabbithole_js::NativeFn| {
        realm.define_method(node, name, arity, f);
    };
    m(realm, "appendChild", 1, n_append_child);
    m(realm, "insertBefore", 2, n_insert_before);
    m(realm, "removeChild", 1, n_remove_child);
    m(realm, "replaceChild", 2, n_replace_child);
    m(realm, "cloneNode", 1, n_clone_node);
    m(realm, "contains", 1, n_contains);
    m(realm, "compareDocumentPosition", 1, n_compare_document_position);
    m(realm, "isEqualNode", 1, n_is_equal_node);
    m(realm, "isSameNode", 1, n_is_same_node);
    m(realm, "hasChildNodes", 0, n_has_child_nodes);
    m(realm, "normalize", 0, n_normalize);
    m(realm, "getRootNode", 0, n_get_root_node);
    m(realm, "lookupNamespaceURI", 1, n_null);
    m(realm, "lookupPrefix", 1, n_null);
    m(realm, "isDefaultNamespace", 1, n_false);

    // ParentNode / ChildNode mixins (defined on Node for simplicity).
    acc(realm, "children", n_children);
    acc(realm, "firstElementChild", n_first_element_child);
    acc(realm, "lastElementChild", n_last_element_child);
    acc(realm, "childElementCount", n_child_element_count);
    acc(realm, "previousElementSibling", n_prev_element_sibling);
    acc(realm, "nextElementSibling", n_next_element_sibling);
    m(realm, "append", 1, n_append);
    m(realm, "prepend", 1, n_prepend);
    m(realm, "before", 1, n_before);
    m(realm, "after", 1, n_after);
    m(realm, "replaceWith", 1, n_replace_with);
    m(realm, "replaceChildren", 1, n_replace_children);
    m(realm, "remove", 0, n_remove);
    m(realm, "querySelector", 1, n_query_selector);
    m(realm, "querySelectorAll", 1, n_query_selector_all);
    m(realm, "getElementsByTagName", 1, n_get_elements_by_tag_name);
    m(realm, "getElementsByTagNameNS", 2, n_get_elements_by_tag_name);
    m(realm, "getElementsByClassName", 1, n_get_elements_by_class_name);

    let cd = define_class(realm, "CharacterData", Some("Node"), illegal_ctor);
    realm.define_accessor(cd, "data", Some(cd_data_get), Some(cd_data_set), rabbithole_js::CONFIGURABLE | rabbithole_js::ENUMERABLE);
    realm.define_accessor(cd, "length", Some(cd_length), None, rabbithole_js::CONFIGURABLE | rabbithole_js::ENUMERABLE);
    realm.define_method(cd, "appendData", 1, cd_append_data);
    realm.define_method(cd, "replaceData", 3, cd_replace_data);
    realm.define_method(cd, "substringData", 2, cd_substring_data);
    let text = define_class(realm, "Text", Some("CharacterData"), text_ctor);
    realm.define_method(text, "splitText", 1, text_split);
    realm.define_accessor(text, "wholeText", Some(text_whole), None, rabbithole_js::CONFIGURABLE | rabbithole_js::ENUMERABLE);
    define_class(realm, "Comment", Some("CharacterData"), comment_ctor);
    define_class(realm, "CDATASection", Some("Text"), illegal_ctor);
    let dt = define_class(realm, "DocumentType", Some("Node"), illegal_ctor);
    realm.define_accessor(dt, "name", Some(doctype_name), None, rabbithole_js::CONFIGURABLE | rabbithole_js::ENUMERABLE);
    realm.define_accessor(dt, "publicId", Some(doctype_public_id), None, rabbithole_js::CONFIGURABLE | rabbithole_js::ENUMERABLE);
    realm.define_accessor(dt, "systemId", Some(doctype_system_id), None, rabbithole_js::CONFIGURABLE | rabbithole_js::ENUMERABLE);
    define_class(realm, "ProcessingInstruction", Some("CharacterData"), illegal_ctor);
    let frag = define_class(realm, "DocumentFragment", Some("Node"), fragment_ctor);
    realm.define_method(frag, "getElementById", 1, fragment_get_element_by_id);

    // NodeList / HTMLCollection prototypes inherit from Array.prototype so
    // `forEach`, iteration and `length` work; `item()` is added.
    let ap = realm.intrinsics.array_proto;
    let nl = realm.new_object_with_proto(Some(ap));
    realm.define_method(nl, "item", 1, define_hidden_method_item);
    realm.define_property(nl, rabbithole_js::realm::SYM_TO_STRING_TAG, Value::str("NodeList"), rabbithole_js::CONFIGURABLE);
    let nl_ctor = realm.new_native_constructor("NodeList", 0, illegal_ctor, nl);
    realm.define_global("NodeList", Value::Object(nl_ctor));
    state().borrow_mut().protos.insert("NodeList", nl);
    let hc = realm.new_object_with_proto(Some(ap));
    realm.define_method(hc, "item", 1, define_hidden_method_item);
    realm.define_method(hc, "namedItem", 1, collection_named_item);
    realm.define_property(hc, rabbithole_js::realm::SYM_TO_STRING_TAG, Value::str("HTMLCollection"), rabbithole_js::CONFIGURABLE);
    let hc_ctor = realm.new_native_constructor("HTMLCollection", 0, illegal_ctor, hc);
    realm.define_global("HTMLCollection", Value::Object(hc_ctor));
    state().borrow_mut().protos.insert("HTMLCollection", hc);

    let deliver = realm.new_native_function("deliverScriptEvents", 0, deliver_script_events);
    realm.extra_roots.push(Value::Object(deliver));
    state().borrow_mut().protos.insert("__deliver_script_events", deliver);
}
