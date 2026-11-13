//! Script-created parsers: `document.open()` / `write()` / `writeln()` /
//! `close()` (HTML §8.4) with a re-entrant, incremental HTML parser
//! ([`rabbithole_html::Parser`]) working on the live document.
//!
//! Written markup is inserted at the parser's insertion point and parsed
//! right away; a `</script>` pauses the parser, the script runs (its own
//! `document.write` calls nest: they insert at the insertion point just
//! after the script and are parsed before the rest), then parsing goes on.
//! External classic scripts are parser-blocking: when one is reached in a
//! nested write it becomes the pending parsing-blocking script, run by the
//! outermost invocation once the writing script returns. When the end of
//! the stream is reached, "the end" runs: deferred scripts, then
//! `DOMContentLoaded` and `load` as tasks, and a frame's `<iframe>` fires
//! `load` in its parent.
//!
//! Documents of same-origin child frames are written through their own
//! realm (scripts in them run there), see [`crate::FrameCall::Document`].

use std::cell::RefCell;
use std::rc::Rc;

use rabbithole_dom::{Document, NodeData, NodeId};
use rabbithole_html::{ParseStatus, Parser};
use rabbithole_js::{JsResult, Realm, Value};

use crate::env::with_env;
use crate::host::{mark_dirty, state, with_doc, NodeRef, FRAME_DOC};

/// The script-created parser of one document of this realm.
pub(crate) struct ActiveParser {
    parser: Parser,
    /// The parser's script nesting level.
    nesting: u32,
    /// External parser-blocking script waiting to run (a nested write
    /// reached it).
    pending_blocking: Option<NodeId>,
    /// Scripts that run when parsing finishes (`defer`, modules).
    deferred: Vec<NodeId>,
}

type Shared = Rc<RefCell<ActiveParser>>;

fn parser_of(d: u32) -> Option<Shared> {
    state().borrow().doc_parsers.get(&d).cloned()
}

fn is_html_doc(d: u32) -> bool {
    with_doc(d, |doc| doc.is_html)
}

fn check_dynamic_markup(realm: &mut Realm, d: u32, method: &str) -> JsResult<()> {
    if !is_html_doc(d) {
        return Err(crate::misc::dom_exception(realm, "InvalidStateError", &format!("Failed to execute '{method}' on 'Document': Only HTML documents support {method}().")));
    }
    if state().borrow().throw_on_dynamic_markup > 0 {
        return Err(crate::misc::dom_exception(realm, "InvalidStateError", &format!("Failed to execute '{method}' on 'Document': {method}() is not allowed while a custom element constructor runs.")));
    }
    Ok(())
}

/// Run `op` on a same-origin frame's document in that frame's realm.
/// None when that realm cannot run it (the caller parses here instead,
/// without running scripts).
fn in_frame(realm: &mut Realm, d: u32, op: crate::DocumentOp) -> Option<JsResult<()>> {
    let id = (d & !FRAME_DOC) as u64;
    match crate::frames::call_frame(id, crate::FrameCall::Document { op })? {
        crate::FrameReply::Error(msg) => {
            let (name, message) = msg.split_once(": ").unwrap_or(("InvalidStateError", msg.as_str()));
            Some(Err(crate::misc::dom_exception(realm, name, message)))
        }
        _ => Some(Ok(())),
    }
}

/// A frame realm's side of [`in_frame`]: errors as "Name: message".
pub(crate) fn foreign_op(realm: &mut Realm, op: crate::DocumentOp) -> crate::FrameReply {
    let r = match op {
        crate::DocumentOp::Open => open(realm, 0),
        crate::DocumentOp::Write(text) => write(realm, 0, &text),
        crate::DocumentOp::Close => close(realm, 0),
    };
    match r {
        Ok(()) => crate::FrameReply::Undefined,
        Err(ev) => {
            let name = realm.get_str(ev.clone(), "name").ok().map(|v| realm.to_rust_string(v).unwrap_or_default()).unwrap_or_default();
            let message = realm.get_str(ev, "message").ok().map(|v| realm.to_rust_string(v).unwrap_or_default()).unwrap_or_default();
            crate::FrameReply::Error(format!("{}: {message}", if name.is_empty() { "Error" } else { &name }))
        }
    }
}

/// `document.open()` (HTML §8.4.1 "document open steps").
pub(crate) fn open(realm: &mut Realm, d: u32) -> JsResult<()> {
    if d & FRAME_DOC != 0 {
        if let Some(r) = in_frame(realm, d, crate::DocumentOp::Open) {
            return r;
        }
    }
    check_dynamic_markup(realm, d, "open")?;
    // Called by a script the parser runs: nothing to do.
    if let Some(ap) = parser_of(d) {
        if ap.borrow().nesting > 0 {
            return Ok(());
        }
    }
    // The page's own load-time parse (the engine parses the page before
    // running its scripts): opening it is a no-op.
    if d == 0 && state().borrow().loading {
        return Ok(());
    }
    if let Some(ap) = state().borrow_mut().doc_parsers.remove(&d) {
        ap.borrow_mut().parser.abort();
    }
    let root = NodeRef { doc: d, node: Document::ROOT };
    let removed = crate::node::children_of(root);
    with_doc(d, |doc| doc.clear_children(Document::ROOT));
    crate::node::record_child_list(root, Vec::new(), removed);
    let mut parser = Parser::new(scripting(d));
    parser.set_insertion_point_at_end();
    let ap = Rc::new(RefCell::new(ActiveParser { parser, nesting: 0, pending_blocking: None, deferred: Vec::new() }));
    state().borrow_mut().doc_parsers.insert(d, ap);
    if d == 0 {
        state().borrow_mut().ready_state = "loading";
        with_env(|env| env.document_load_started());
        mark_dirty();
    }
    Ok(())
}

fn scripting(d: u32) -> bool {
    d == 0
}

/// `document.write(text)` (HTML §8.4.3 "document write steps").
pub(crate) fn write(realm: &mut Realm, d: u32, text: &str) -> JsResult<()> {
    if d & FRAME_DOC != 0 {
        if let Some(r) = in_frame(realm, d, crate::DocumentOp::Write(text.to_string())) {
            return r;
        }
    }
    check_dynamic_markup(realm, d, "write")?;
    let mut ap = parser_of(d);
    if ap.as_ref().map(|a| a.borrow().parser.insertion_point().is_none()).unwrap_or(true) {
        open(realm, d)?;
        ap = parser_of(d);
    }
    let Some(ap) = ap else { return Ok(()) };
    ap.borrow_mut().parser.write(text);
    if ap.borrow().pending_blocking.is_none() {
        pump(realm, d, &ap);
    }
    Ok(())
}

/// `document.close()` (HTML §8.4.2).
pub(crate) fn close(realm: &mut Realm, d: u32) -> JsResult<()> {
    if d & FRAME_DOC != 0 {
        if let Some(r) = in_frame(realm, d, crate::DocumentOp::Close) {
            return r;
        }
    }
    check_dynamic_markup(realm, d, "close")?;
    let Some(ap) = parser_of(d) else { return Ok(()) };
    ap.borrow_mut().parser.close();
    if ap.borrow().pending_blocking.is_none() {
        pump(realm, d, &ap);
    }
    Ok(())
}

/// Is `d` being written by a script-created parser right now (its
/// insertion point is defined)?
pub(crate) fn has_insertion_point(d: u32) -> bool {
    parser_of(d).map(|a| a.borrow().parser.insertion_point().is_some()).unwrap_or(false)
}

/// Run the parser until it needs input, or the stream ends.
fn pump(realm: &mut Realm, d: u32, ap: &Shared) {
    loop {
        let before = with_doc(d, |doc| doc.node_count());
        let status = with_doc(d, |doc| ap.borrow_mut().parser.run(doc));
        after_parse_slice(d, before);
        match status {
            ParseStatus::NeedInput => break,
            ParseStatus::Script(node) => {
                let kind = classify(d, node);
                match kind {
                    ScriptKind::Ignore => {
                        state().borrow_mut().ran_scripts.insert(node);
                    }
                    ScriptKind::Deferred => {
                        ap.borrow_mut().deferred.push(node);
                    }
                    ScriptKind::Async => crate::node::run_script_element(realm, NodeRef { doc: d, node }),
                    ScriptKind::Blocking if ap.borrow().nesting > 0 => {
                        // Run by the outermost invocation, after the
                        // script that wrote it returns.
                        ap.borrow_mut().pending_blocking = Some(node);
                        break;
                    }
                    ScriptKind::Inline | ScriptKind::Blocking => run_parser_script(realm, d, ap, node),
                }
                // A nested write left a parser-blocking script: run it now
                // (we are the outer invocation once nesting is back to 0).
                loop {
                    let pending = {
                        let mut a = ap.borrow_mut();
                        if a.nesting > 0 {
                            None
                        } else {
                            a.pending_blocking.take()
                        }
                    };
                    let Some(p) = pending else { break };
                    run_parser_script(realm, d, ap, p);
                }
                if ap.borrow().pending_blocking.is_some() {
                    break;
                }
            }
            ParseStatus::Finished => {
                finish(realm, d, ap);
                break;
            }
        }
    }
}

/// Execute a parser-inserted script with the insertion point just after
/// it and the script nesting level raised.
fn run_parser_script(realm: &mut Realm, d: u32, ap: &Shared, node: NodeId) {
    {
        let mut a = ap.borrow_mut();
        a.parser.push_insertion_point_here();
        a.nesting += 1;
    }
    crate::node::run_script_element_sync(realm, NodeRef { doc: d, node });
    let mut a = ap.borrow_mut();
    a.nesting -= 1;
    a.parser.pop_insertion_point();
}

enum ScriptKind {
    Ignore,
    Inline,
    Blocking,
    Async,
    Deferred,
}

fn classify(d: u32, node: NodeId) -> ScriptKind {
    with_doc(d, |doc| {
        let ty = doc.attr(node, "type").map(|t| t.trim().to_ascii_lowercase());
        let lang = doc.attr(node, "language").map(|t| t.trim().to_ascii_lowercase());
        let module = ty.as_deref() == Some("module");
        let classic = match (ty.as_deref(), lang.as_deref()) {
            (Some(""), _) | (None, None) | (None, Some("")) => true,
            (None, Some(l)) => crate::node::is_js_mime(&format!("text/{l}")),
            (Some(t), _) => crate::node::is_js_mime(t),
        };
        if !(module || classic) || (classic && doc.attr(node, "nomodule").is_some()) {
            return ScriptKind::Ignore;
        }
        let src = doc.attr(node, "src").is_some();
        let is_async = doc.attr(node, "async").is_some();
        if module {
            return if is_async { ScriptKind::Async } else { ScriptKind::Deferred };
        }
        if !src {
            return ScriptKind::Inline;
        }
        if is_async {
            ScriptKind::Async
        } else if doc.attr(node, "defer").is_some() {
            ScriptKind::Deferred
        } else {
            ScriptKind::Blocking
        }
    })
}

/// Nodes the parser created in the last slice: custom element reactions
/// and mutation records for the ones it inserted under existing nodes.
fn after_parse_slice(d: u32, before: usize) {
    let groups: Vec<(NodeId, Vec<NodeId>)> = with_doc(d, |doc| {
        let after = doc.node_count();
        let mut groups: Vec<(NodeId, Vec<NodeId>)> = Vec::new();
        for n in before..after {
            if matches!(doc.node(n).data, NodeData::Document) {
                continue;
            }
            let Some(p) = doc.parent(n) else { continue };
            if p >= before {
                continue;
            }
            match groups.last_mut() {
                Some((gp, kids)) if *gp == p => kids.push(n),
                _ => groups.push((p, vec![n])),
            }
        }
        groups
    });
    if groups.is_empty() {
        return;
    }
    for (p, kids) in groups {
        let added = kids.into_iter().map(|node| NodeRef { doc: d, node }).collect();
        crate::node::record_child_list(NodeRef { doc: d, node: p }, added, Vec::new());
    }
    if d == 0 {
        mark_dirty();
    }
}

/// "The end" (HTML §13.2.7) for a script-created parser.
fn finish(realm: &mut Realm, d: u32, ap: &Shared) {
    let deferred = std::mem::take(&mut ap.borrow_mut().deferred);
    {
        let st = state();
        let mut st = st.borrow_mut();
        if st.doc_parsers.get(&d).map(|a| Rc::ptr_eq(a, ap)).unwrap_or(false) {
            st.doc_parsers.remove(&d);
        }
    }
    if d != 0 {
        return;
    }
    state().borrow_mut().ready_state = "interactive";
    crate::events::dispatch(realm, crate::host::TargetKey::Node(NodeRef::main(Document::ROOT)), "readystatechange", crate::EventInit::simple(false, false), true);
    for s in deferred {
        crate::node::run_script_element_sync(realm, NodeRef { doc: d, node: s });
    }
    crate::window::queue_task(realm, "DOMContentLoaded", task_dom_content_loaded);
}

fn task_dom_content_loaded(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    crate::events::dispatch(realm, crate::host::TargetKey::Node(NodeRef::main(Document::ROOT)), "DOMContentLoaded", crate::EventInit::simple(true, false), true);
    crate::window::queue_task(realm, "load", task_load);
    Ok(Value::Undefined)
}

fn task_load(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    state().borrow_mut().ready_state = "complete";
    crate::events::dispatch(realm, crate::host::TargetKey::Node(NodeRef::main(Document::ROOT)), "readystatechange", crate::EventInit::simple(false, false), true);
    crate::events::dispatch(realm, crate::host::TargetKey::Window, "load", crate::EventInit::simple(false, false), true);
    with_env(|env| env.document_load_complete());
    Ok(Value::Undefined)
}

// ------------------------------------------------------ streaming methods

/// A streaming fragment parser (`streamHTML()`, `streamAppendHTML()`, ...)
/// writing into a live container.
struct StreamParser {
    parser: Parser,
    doc: u32,
    container: NodeId,
    before: Option<NodeId>,
    run_scripts: bool,
    closed: bool,
}

fn hierarchy_error(realm: &mut Realm, msg: &str) -> Value {
    crate::misc::dom_exception(realm, "HierarchyRequestError", msg)
}

/// `__rhNative.streamOpen(node, position, {runScripts, check})`: validate
/// the target and (unless `check`) start a streaming parser; position is
/// "html", "append", "prepend", "before", "after" or "replaceWith".
pub(crate) fn stream_open(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(n) = crate::host::node_of(realm, &crate::host::arg(args, 0)) else {
        return Err(realm.throw_type_error("streamOpen: node expected"));
    };
    let pos = crate::host::arg_str(realm, args, 1)?;
    let opts = crate::host::arg(args, 2);
    let flag = |realm: &mut Realm, k: &str| -> JsResult<bool> {
        if !opts.is_object() {
            return Ok(false);
        }
        let v = realm.get_str(opts.clone(), k)?;
        Ok(realm.to_boolean(&v))
    };
    let run_scripts = flag(realm, "runScripts")?;
    let dsd = flag(realm, "dsd")?;
    let check = flag(realm, "check")?;
    let d = n.doc;
    // The container whose children are written, and the context element.
    let positional = matches!(pos.as_str(), "before" | "after" | "replaceWith");
    let container = if positional {
        let parent = with_doc(d, |doc| doc.parent(n.node));
        let Some(p) = parent else { return Err(hierarchy_error(realm, "The node has no parent.")) };
        let ok = with_doc(d, |doc| match doc.element(p) {
            None => false,
            Some(e) if e.tag == "#fragment" => doc.template_contents.values().any(|&c| c == p),
            Some(_) => true,
        });
        if !ok {
            return Err(hierarchy_error(realm, "The node's parent is not an element, shadow root or template contents."));
        }
        p
    } else {
        // A template's markup is its content's.
        with_doc(d, |doc| match doc.element(n.node) {
            Some(e) if e.tag == "template" && e.ns == rabbithole_dom::Namespace::Html => doc.template_content(n.node),
            _ => n.node,
        })
    };
    if check {
        return Ok(Value::Undefined);
    }
    let (context, ctx_tag, ctx_ns, form) = with_doc(d, |doc| {
        // Shadow roots parse in their host's context, template contents in
        // the template's; other fragments as a <body>.
        let ctx = if let Some(h) = doc.shadow_host(container) {
            Some(h)
        } else if let Some((&t, _)) = doc.template_contents.iter().find(|(_, &c)| c == container) {
            Some(t)
        } else if doc.element(container).map(|e| !e.tag.starts_with('#')).unwrap_or(false) {
            Some(container)
        } else {
            None
        };
        match ctx {
            Some(c) => {
                let e = doc.element(c).expect("element");
                let form = doc.ancestor_or_self(c, |a| doc.element(a).map(|e| e.tag == "form" && e.ns == rabbithole_dom::Namespace::Html).unwrap_or(false));
                (c, e.tag.clone(), e.ns.clone(), form)
            }
            None => {
                let body = doc.create_element("body", vec![]);
                (body, "body".to_string(), rabbithole_dom::Namespace::Html, None)
            }
        }
    });
    let mut removed = Vec::new();
    let before = match pos.as_str() {
        "html" => {
            removed = crate::node::children_of(NodeRef { doc: d, node: container });
            with_doc(d, |doc| doc.clear_children(container));
            None
        }
        "append" => None,
        "prepend" => with_doc(d, |doc| doc.first_child(container)),
        "before" => Some(n.node),
        "after" => with_doc(d, |doc| doc.next_sibling(n.node)),
        "replaceWith" => {
            let next = with_doc(d, |doc| doc.next_sibling(n.node));
            crate::node::remove_node(n);
            next
        }
        _ => return Err(realm.throw_type_error("streamOpen: bad position")),
    };
    if !removed.is_empty() {
        crate::node::record_child_list(NodeRef { doc: d, node: container }, Vec::new(), removed);
    }
    let scripting = d == 0 || d & FRAME_DOC != 0;
    let mut parser = Parser::new_fragment(context, &ctx_tag, ctx_ns, container, before, form, scripting, dsd);
    parser.set_pause_on_scripts(run_scripts);
    let o = realm.new_object();
    realm.set_host_data(o, Box::new(RefCell::new(StreamParser { parser, doc: d, container, before, run_scripts, closed: false })));
    if d == 0 {
        mark_dirty();
    }
    Ok(Value::Object(o))
}

fn stream_data(realm: &Realm, v: &Value) -> Option<rabbithole_js::ObjectId> {
    let o = v.as_object()?;
    realm.host_data::<RefCell<StreamParser>>(o).map(|_| o)
}

/// `__rhNative.streamWrite(handle, text)`: parse more markup into the
/// container; returns the elements created.
pub(crate) fn stream_write(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(o) = stream_data(realm, &crate::host::arg(args, 0)) else {
        return Err(realm.throw_type_error("streamWrite: stream expected"));
    };
    let text = crate::host::arg_str(realm, args, 1)?;
    stream_run(realm, o, Some(&text))
}

/// `__rhNative.streamClose(handle)`: end of input; returns the elements
/// created.
pub(crate) fn stream_close(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(o) = stream_data(realm, &crate::host::arg(args, 0)) else {
        return Err(realm.throw_type_error("streamClose: stream expected"));
    };
    stream_run(realm, o, None)
}

fn stream_run(realm: &mut Realm, o: rabbithole_js::ObjectId, text: Option<&str>) -> JsResult<Value> {
    let (d, container, before, run_scripts, closed) = {
        let s = realm.host_data::<RefCell<StreamParser>>(o).expect("stream").borrow();
        (s.doc, s.container, s.before, s.run_scripts, s.closed)
    };
    if closed {
        return Ok(realm.new_array_value(Vec::new()));
    }
    // The reference node must still be a child of the container.
    if let Some(b) = before {
        if with_doc(d, |doc| doc.parent(b)) != Some(container) {
            return Err(hierarchy_error(realm, "The reference node is no longer a child of the stream's parent."));
        }
    }
    {
        let s = realm.host_data::<RefCell<StreamParser>>(o).expect("stream");
        let mut s = s.borrow_mut();
        match text {
            Some(t) => s.parser.append(t),
            None => {
                s.parser.close();
                s.closed = true;
            }
        }
    }
    let start = with_doc(d, |doc| doc.node_count());
    loop {
        let before_slice = with_doc(d, |doc| doc.node_count());
        let status = {
            let s = realm.host_data::<RefCell<StreamParser>>(o).expect("stream");
            let mut s = s.borrow_mut();
            with_doc(d, |doc| s.parser.run(doc))
        };
        after_parse_slice(d, before_slice);
        match status {
            ParseStatus::Script(node) => {
                let connected = with_doc(d, |doc| doc.is_connected(node));
                if run_scripts {
                    // The runScripts option: not "already started".
                    with_doc(d, |doc| doc.script_started.remove(&node));
                }
                if run_scripts && connected && d == 0 {
                    crate::node::run_script_element_sync(realm, NodeRef { doc: d, node });
                } else if d == 0 {
                    state().borrow_mut().ran_scripts.insert(node);
                }
            }
            _ => break,
        }
    }
    // Scripts of a stream that does not run them never run.
    let created: Vec<NodeId> = with_doc(d, |doc| (start..doc.node_count()).filter(|&n| doc.element(n).map(|e| !e.tag.starts_with('#')).unwrap_or(false)).collect());
    if run_scripts {
        let ran: Vec<NodeId> = if d == 0 { created.iter().copied().filter(|n| state().borrow().ran_scripts.contains(n)).collect() } else { Vec::new() };
        with_doc(d, |doc| {
            for &n in &created {
                if doc.tag(n) == Some("script") && !ran.contains(&n) {
                    doc.script_started.remove(&n);
                }
            }
        });
    }
    if d == 0 && !run_scripts {
        let st = state();
        let mut st = st.borrow_mut();
        with_doc(0, |doc| {
            for &n in &created {
                if doc.is_element(n, "script") {
                    st.ran_scripts.insert(n);
                }
            }
        });
    }
    let vals: Vec<Value> = created.into_iter().map(|n| crate::host::wrap(realm, NodeRef { doc: d, node: n })).collect();
    Ok(realm.new_array_value(vals))
}
