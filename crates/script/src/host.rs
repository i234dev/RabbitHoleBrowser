//! Shared state of the bindings: node wrappers, prototypes, listeners,
//! timers, observers, and the helpers natives use to reach the DOM.

use crate::env::with_env;
use rabbithole_dom::{Document, NodeId};
use rabbithole_js::{JsResult, ObjectId, PropertyKey, Realm, Value, HIDDEN};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Identifies a node in the main document (`doc == 0`) or in an auxiliary
/// document created by `DOMParser` / `createHTMLDocument` (`doc >= 1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeRef {
    pub doc: u32,
    pub node: NodeId,
}

impl NodeRef {
    pub fn main(node: NodeId) -> NodeRef {
        NodeRef { doc: 0, node }
    }
}

/// Event target identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetKey {
    Window,
    Node(NodeRef),
    Object(ObjectId),
}

/// One registered event listener.
#[derive(Clone)]
#[allow(dead_code)]
pub struct Listener {
    pub ty: String,
    pub callback: Value,
    pub capture: bool,
    pub once: bool,
    pub passive: bool,
    pub signal: Option<ObjectId>,
    pub removed: Rc<std::cell::Cell<bool>>,
    /// Placeholder for the `onX` IDL handler, inserted where it was first
    /// assigned so ordering follows the spec.
    pub is_handler: bool,
}

/// A pending timer (`setTimeout` / `setInterval`).
pub struct Timer {
    pub id: u32,
    pub due: f64,
    pub callback: Value,
    pub args: Vec<Value>,
    pub interval: Option<f64>,
}

pub struct RafCallback {
    pub id: u32,
    pub callback: Value,
}

/// A queued MutationObserver record.
pub struct MutationRecord {
    pub observer: ObjectId,
    pub ty: &'static str,
    pub target: NodeRef,
    pub added: Vec<NodeRef>,
    pub removed: Vec<NodeRef>,
    pub attribute_name: Option<String>,
    pub old_value: Option<String>,
}

pub struct ObserverReg {
    pub observer: ObjectId,
    pub target: NodeRef,
    pub child_list: bool,
    pub attributes: bool,
    pub character_data: bool,
    pub subtree: bool,
    pub attribute_old_value: bool,
}

#[allow(dead_code)]
pub struct HostState {
    pub wrappers: HashMap<NodeRef, ObjectId>,
    pub protos: HashMap<&'static str, ObjectId>,
    pub listeners: HashMap<TargetKey, Vec<Listener>>,
    /// Count of listeners per (node, type) for fast existence checks.
    pub listener_counts: HashMap<(TargetKey, String), usize>,
    pub timers: Vec<Timer>,
    pub rafs: Vec<RafCallback>,
    pub next_timer_id: u32,
    pub last_frame: f64,
    pub dirty: bool,
    pub ready_state: &'static str,
    pub loading: bool,
    pub current_script: Option<NodeRef>,
    pub focused: Option<NodeId>,
    pub extra_docs: Vec<Document>,
    pub window: ObjectId,
    pub document_obj: ObjectId,
    /// JS array object holding GC roots for listener callbacks / timers.
    pub root_array: ObjectId,
    /// Scripts already executed (so dynamic insertion doesn't rerun them).
    pub ran_scripts: HashSet<NodeId>,
    pub inline_handlers: HashMap<(NodeRef, String), Value>,
    pub observers: Vec<ObserverReg>,
    pub mutation_records: Vec<MutationRecord>,
    pub mutation_scheduled: bool,
    pub custom_elements: HashMap<String, Value>,
    /// Element an `HTMLElement` super constructor must return (set while a
    /// custom element is being created or upgraded).
    pub ce_constructing: Option<NodeRef>,
    /// Custom elements whose constructor already ran.
    pub ce_upgraded: HashSet<NodeRef>,
    /// `pushState` overrides of the location.
    pub location_override: Option<String>,
    pub history_len: u32,
    pub history_state: Value,
    pub event_stack: Vec<ObjectId>,
    pub time_origin: f64,
    pub console_marks: HashMap<String, f64>,
    pub console_counts: HashMap<String, u32>,
    pub pending_intersections: Vec<(ObjectId, Vec<NodeRef>)>,
    pub pending_resizes: Vec<(ObjectId, Vec<NodeRef>)>,
    pub pending_script_events: Vec<(NodeRef, &'static str)>,
    pub deferred_natives: Vec<ObjectId>,
    /// Module cache and import map.
    pub modules: crate::modules::ModuleState,
}

thread_local! {
    static STATE: RefCell<Vec<Rc<RefCell<HostState>>>> = const { RefCell::new(Vec::new()) };
}

pub(crate) struct StateGuard;

impl StateGuard {
    pub(crate) fn enter(state: Rc<RefCell<HostState>>) -> StateGuard {
        STATE.with(|s| s.borrow_mut().push(state));
        StateGuard
    }
}

impl Drop for StateGuard {
    fn drop(&mut self) {
        STATE.with(|s| {
            s.borrow_mut().pop();
        });
    }
}

/// The active host state (inside an entry point).
pub(crate) fn state() -> Rc<RefCell<HostState>> {
    STATE.with(|s| s.borrow().last().cloned()).expect("script host: no active state")
}

/// Borrow the document a node belongs to.
pub(crate) fn with_doc<R>(doc: u32, f: impl FnOnce(&mut Document) -> R) -> R {
    if doc == 0 {
        with_env(|env| f(env.doc()))
    } else {
        let st = state();
        let mut st = st.borrow_mut();
        let d = &mut st.extra_docs[(doc - 1) as usize];
        f(d)
    }
}

/// Record that scripts changed the DOM / styles.
pub(crate) fn mark_dirty() {
    state().borrow_mut().dirty = true;
    with_env(|env| env.mark_dom_dirty());
}

pub(crate) fn proto(name: &str) -> ObjectId {
    let st = state();
    let st = st.borrow();
    match st.protos.get(name) {
        Some(p) => *p,
        None => *st.protos.get("HTMLElement").expect("HTMLElement proto"),
    }
}

/// Lower-case tag → (prototype name, class name).
pub(crate) fn element_class(tag: &str, is_svg: bool) -> (&'static str, &'static str) {
    if is_svg {
        return ("SVGElement", if tag == "svg" { "SVGSVGElement" } else { "SVGElement" });
    }
    match tag {
        "a" => ("HTMLAnchorElement", "HTMLAnchorElement"),
        "input" => ("HTMLInputElement", "HTMLInputElement"),
        "textarea" => ("HTMLTextAreaElement", "HTMLTextAreaElement"),
        "select" => ("HTMLSelectElement", "HTMLSelectElement"),
        "option" => ("HTMLOptionElement", "HTMLOptionElement"),
        "form" => ("HTMLFormElement", "HTMLFormElement"),
        "img" => ("HTMLImageElement", "HTMLImageElement"),
        "script" => ("HTMLScriptElement", "HTMLScriptElement"),
        "style" => ("HTMLStyleElement", "HTMLStyleElement"),
        "link" => ("HTMLLinkElement", "HTMLLinkElement"),
        "iframe" => ("HTMLIFrameElement", "HTMLIFrameElement"),
        "canvas" => ("HTMLCanvasElement", "HTMLCanvasElement"),
        "button" => ("HTMLButtonElement", "HTMLButtonElement"),
        "label" => ("HTMLLabelElement", "HTMLLabelElement"),
        "template" => ("HTMLTemplateElement", "HTMLTemplateElement"),
        "video" => ("HTMLMediaElement", "HTMLVideoElement"),
        "audio" => ("HTMLMediaElement", "HTMLAudioElement"),
        "div" => ("HTMLDivElement", "HTMLDivElement"),
        "span" => ("HTMLSpanElement", "HTMLSpanElement"),
        "p" => ("HTMLParagraphElement", "HTMLParagraphElement"),
        "ul" => ("HTMLUListElement", "HTMLUListElement"),
        "ol" => ("HTMLOListElement", "HTMLOListElement"),
        "li" => ("HTMLLIElement", "HTMLLIElement"),
        "body" => ("HTMLBodyElement", "HTMLBodyElement"),
        "html" => ("HTMLHtmlElement", "HTMLHtmlElement"),
        "head" => ("HTMLHeadElement", "HTMLHeadElement"),
        "table" => ("HTMLTableElement", "HTMLTableElement"),
        "tr" => ("HTMLTableRowElement", "HTMLTableRowElement"),
        "td" | "th" => ("HTMLTableCellElement", "HTMLTableCellElement"),
        "tbody" | "thead" | "tfoot" => ("HTMLTableSectionElement", "HTMLTableSectionElement"),
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => ("HTMLHeadingElement", "HTMLHeadingElement"),
        "br" => ("HTMLBRElement", "HTMLBRElement"),
        "hr" => ("HTMLHRElement", "HTMLHRElement"),
        "pre" => ("HTMLPreElement", "HTMLPreElement"),
        "meta" => ("HTMLMetaElement", "HTMLMetaElement"),
        "title" => ("HTMLTitleElement", "HTMLTitleElement"),
        "dl" => ("HTMLDListElement", "HTMLDListElement"),
        "fieldset" => ("HTMLFieldSetElement", "HTMLFieldSetElement"),
        "legend" => ("HTMLLegendElement", "HTMLLegendElement"),
        "details" => ("HTMLDetailsElement", "HTMLDetailsElement"),
        "dialog" => ("HTMLDialogElement", "HTMLDialogElement"),
        "source" => ("HTMLSourceElement", "HTMLSourceElement"),
        "picture" => ("HTMLPictureElement", "HTMLPictureElement"),
        "object" | "embed" => ("HTMLObjectElement", "HTMLObjectElement"),
        "base" => ("HTMLBaseElement", "HTMLBaseElement"),
        "map" | "area" => ("HTMLAreaElement", "HTMLAreaElement"),
        "progress" | "meter" => ("HTMLProgressElement", "HTMLProgressElement"),
        "time" => ("HTMLTimeElement", "HTMLTimeElement"),
        "data" | "output" => ("HTMLDataElement", "HTMLDataElement"),
        "slot" => ("HTMLSlotElement", "HTMLSlotElement"),
        "menu" => ("HTMLMenuElement", "HTMLMenuElement"),
        "quote" | "blockquote" | "q" => ("HTMLQuoteElement", "HTMLQuoteElement"),
        "font" => ("HTMLFontElement", "HTMLFontElement"),
        "frame" | "frameset" => ("HTMLFrameElement", "HTMLFrameElement"),
        "mod" | "ins" | "del" => ("HTMLModElement", "HTMLModElement"),
        "caption" => ("HTMLTableCaptionElement", "HTMLTableCaptionElement"),
        "col" | "colgroup" => ("HTMLTableColElement", "HTMLTableColElement"),
        "optgroup" => ("HTMLOptGroupElement", "HTMLOptGroupElement"),
        "datalist" => ("HTMLDataListElement", "HTMLDataListElement"),
        "track" => ("HTMLTrackElement", "HTMLTrackElement"),
        "param" => ("HTMLParamElement", "HTMLParamElement"),
        "marquee" => ("HTMLMarqueeElement", "HTMLMarqueeElement"),
        "nav" | "header" | "footer" | "section" | "article" | "aside" | "main" => ("HTMLElement", "HTMLElement"),
        "#fragment" => ("DocumentFragment", "DocumentFragment"),
        _ => ("HTMLElement", "HTMLElement"),
    }
}

fn is_svg_node(doc: &Document, node: NodeId) -> bool {
    doc.ancestor_or_self(node, |n| doc.is_element(n, "svg")).is_some()
}

/// Wrapper object for a node (cached).
pub(crate) fn wrap(realm: &mut Realm, n: NodeRef) -> Value {
    if let Some(id) = state().borrow().wrappers.get(&n) {
        return Value::Object(*id);
    }
    let (proto_name, class_name) = with_doc(n.doc, |doc| match &doc.node(n.node).data {
        rabbithole_dom::NodeData::Document => ("Document", "HTMLDocument"),
        rabbithole_dom::NodeData::Text(_) => ("Text", "Text"),
        rabbithole_dom::NodeData::Comment(_) => ("Comment", "Comment"),
        rabbithole_dom::NodeData::Element(e) => {
            let svg = e.tag != "#fragment" && is_svg_node(doc, n.node);
            element_class(&e.tag, svg)
        }
    });
    let p = proto(proto_name);
    let id = realm.new_object_with_proto(Some(p));
    realm.set_host_data(id, Box::new(n));
    realm.set_class_name(id, class_name);
    realm.extra_roots.push(Value::Object(id));
    state().borrow_mut().wrappers.insert(n, id);
    Value::Object(id)
}

pub(crate) fn wrap_main(realm: &mut Realm, node: NodeId) -> Value {
    wrap(realm, NodeRef::main(node))
}

pub(crate) fn wrap_opt(realm: &mut Realm, doc: u32, node: Option<NodeId>) -> Value {
    match node {
        Some(n) => wrap(realm, NodeRef { doc, node: n }),
        None => Value::Null,
    }
}

/// The object carrying host data for `v`: itself, or a Proxy's target.
pub(crate) fn host_obj(realm: &Realm, v: &Value) -> Option<ObjectId> {
    match v {
        Value::Object(id) => match &realm.obj(*id).kind {
            rabbithole_js::ObjectKind::Proxy { target, .. } => Some(*target),
            _ => Some(*id),
        },
        _ => None,
    }
}

/// The node a wrapper refers to.
pub(crate) fn node_of(realm: &Realm, v: &Value) -> Option<NodeRef> {
    let id = host_obj(realm, v)?;
    realm.host_data::<NodeRef>(id).copied()
}

pub(crate) fn this_node(realm: &mut Realm, this: &Value, what: &str) -> JsResult<NodeRef> {
    match node_of(realm, this) {
        Some(n) => Ok(n),
        None => Err(realm.throw_type_error(&format!("Illegal invocation: {what} called on a non-Node"))),
    }
}

/// Keep a JS value alive for the lifetime of the page (listener callbacks,
/// timer callbacks, observers).
pub(crate) fn root_value(realm: &mut Realm, v: &Value) {
    if !v.is_object() {
        return;
    }
    let arr = state().borrow().root_array;
    if let rabbithole_js::ObjectKind::Array { elements, length } = &mut realm.obj_mut(arr).kind {
        elements.push(v.clone());
        *length = elements.len() as u32;
    }
}

pub(crate) fn unroot_value(realm: &mut Realm, v: &Value) {
    let Value::Object(target) = v else { return };
    let arr = state().borrow().root_array;
    if let rabbithole_js::ObjectKind::Array { elements, length } = &mut realm.obj_mut(arr).kind {
        if let Some(pos) = elements.iter().rposition(|e| matches!(e, Value::Object(o) if o == target)) {
            elements.remove(pos);
        }
        *length = elements.len() as u32;
    }
}

/// Define a class: prototype (inheriting `parent`), constructor global.
pub(crate) fn define_class(realm: &mut Realm, name: &'static str, parent: Option<&str>, ctor: rabbithole_js::NativeFn) -> ObjectId {
    let parent_proto = match parent {
        Some(p) => Some(proto(p)),
        None => Some(realm.intrinsics.object_proto),
    };
    let p = realm.new_object_with_proto(parent_proto);
    let c = realm.new_native_constructor(name, 0, ctor, p);
    realm.define_global(name, Value::Object(c));
    realm.define_property(p, rabbithole_js::realm::SYM_TO_STRING_TAG, Value::str(name), rabbithole_js::CONFIGURABLE);
    state().borrow_mut().protos.insert(name, p);
    p
}

/// A constructor that refuses direct construction.
pub(crate) fn illegal_ctor(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    Err(realm.throw_type_error("Illegal constructor"))
}

/// Constructor of `HTMLElement` and the other HTML element classes: only
/// valid as the `super()` of a defined custom element class (HTML §4.13.4
/// "HTML element constructors"). Returns the element being upgraded, or a
/// fresh element of the class's registered name.
pub(crate) fn html_element_ctor(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    let nt = realm.new_target();
    let Value::Object(nt_id) = nt.clone() else {
        return Err(realm.throw_type_error("Illegal constructor"));
    };
    let tag = state()
        .borrow()
        .custom_elements
        .iter()
        .find(|(_, c)| matches!(c, Value::Object(id) if *id == nt_id))
        .map(|(t, _)| t.clone());
    let Some(tag) = tag else {
        return Err(realm.throw_type_error("Illegal constructor"));
    };
    let pending = state().borrow_mut().ce_constructing.take();
    let n = match pending {
        Some(n) => n,
        None => {
            let node = with_doc(0, |doc| doc.create_element(&tag, vec![]));
            NodeRef::main(node)
        }
    };
    state().borrow_mut().ce_upgraded.insert(n);
    let el = wrap(realm, n);
    if let (Value::Object(e), Ok(Value::Object(p))) = (&el, realm.get_str(nt, "prototype")) {
        realm.obj_mut(*e).proto = Some(p);
    }
    Ok(el)
}

/// Run a custom element's constructor for an existing node (upgrade) if it
/// has not run yet. Returns the element wrapper.
pub(crate) fn upgrade_custom_element(realm: &mut Realm, n: NodeRef, ctor: &Value) -> Value {
    if state().borrow().ce_upgraded.contains(&n) {
        return wrap(realm, n);
    }
    state().borrow_mut().ce_constructing = Some(n);
    let result = realm.construct(ctor.clone(), &[], ctor.clone());
    state().borrow_mut().ce_constructing = None;
    let el = wrap(realm, n);
    match result {
        Ok(_) => {
            state().borrow_mut().ce_upgraded.insert(n);
            // attributeChangedCallback for observed attributes already present.
            if let Ok(obs) = realm.get_str(ctor.clone(), "observedAttributes") {
                if let Ok(cb) = realm.get_str(el.clone(), "attributeChangedCallback") {
                    if realm.is_callable(&cb) {
                        let names: Vec<String> = match obs {
                            Value::Object(arr) => {
                                let len = realm.get_str(Value::Object(arr), "length").ok().and_then(|v| realm.to_number(v).ok()).unwrap_or(0.0) as usize;
                                let mut names = Vec::new();
                                for i in 0..len.min(64) {
                                    if let Ok(v) = realm.get_str(Value::Object(arr), &i.to_string()) {
                                        if let Ok(s) = realm.to_rust_string(v) {
                                            names.push(s);
                                        }
                                    }
                                }
                                names
                            }
                            _ => Vec::new(),
                        };
                        for name in names {
                            let val = with_doc(n.doc, |doc| doc.attr(n.node, &name.to_ascii_lowercase()).map(|s| s.to_string()));
                            if let Some(v) = val {
                                let _ = realm.call(cb.clone(), el.clone(), &[Value::str(&name), Value::Null, Value::str(&v)]);
                            }
                        }
                    }
                }
            }
        }
        Err(e) => {
            let msg = realm.error_to_string(&e);
            with_env(|env| env.log("error", &format!("Uncaught {msg} (custom element constructor)")));
        }
    }
    el
}

pub(crate) fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Undefined)
}

pub(crate) fn arg_str(realm: &mut Realm, args: &[Value], i: usize) -> JsResult<String> {
    let v = arg(args, i);
    realm.to_rust_string(v)
}

pub(crate) fn arg_str_opt(realm: &mut Realm, args: &[Value], i: usize) -> JsResult<Option<String>> {
    match args.get(i) {
        None | Some(Value::Undefined) => Ok(None),
        Some(v) => realm.to_rust_string(v.clone()).map(Some),
    }
}

/// Native data (captured Rust value) of the running native function.
pub(crate) fn native_data<T: 'static>(realm: &Realm) -> Option<Rc<T>> {
    let f = realm.current_native()?;
    realm.native_data::<T>(f)
}

pub(crate) fn native_captures(realm: &Realm) -> Vec<Value> {
    match realm.current_native() {
        Some(f) => realm.native_captures(f),
        None => Vec::new(),
    }
}

/// A hidden data property (string key) on an object.
pub(crate) fn get_hidden(realm: &Realm, obj: ObjectId, key: &str) -> Value {
    realm.obj(obj).props.get(&PropertyKey::str(key)).map(|p| p.value.clone()).unwrap_or(Value::Undefined)
}

pub(crate) fn set_hidden(realm: &mut Realm, obj: ObjectId, key: &str, v: Value) {
    realm.define_property(obj, key, v, HIDDEN);
}

/// Build a JS array from values.
pub(crate) fn array(realm: &mut Realm, values: Vec<Value>) -> Value {
    realm.new_array_value(values)
}

/// A plain object from (key, value) pairs.
pub(crate) fn object_from(realm: &mut Realm, pairs: Vec<(&str, Value)>) -> Value {
    let o = realm.new_object();
    for (k, v) in pairs {
        realm.define_property(o, k, v, rabbithole_js::ALL);
    }
    Value::Object(o)
}

/// Read a string-valued option from a dictionary argument.
pub(crate) fn dict_str(realm: &mut Realm, dict: &Value, key: &str) -> JsResult<Option<String>> {
    if !dict.is_object() {
        return Ok(None);
    }
    let v = realm.get_str(dict.clone(), key)?;
    if v.is_undefined() {
        return Ok(None);
    }
    realm.to_rust_string(v).map(Some)
}

pub(crate) fn dict_bool(realm: &mut Realm, dict: &Value, key: &str) -> JsResult<Option<bool>> {
    if !dict.is_object() {
        return Ok(None);
    }
    let v = realm.get_str(dict.clone(), key)?;
    if v.is_undefined() {
        return Ok(None);
    }
    Ok(Some(realm.to_boolean(&v)))
}

pub(crate) fn dict_value(realm: &mut Realm, dict: &Value, key: &str) -> JsResult<Value> {
    if !dict.is_object() {
        return Ok(Value::Undefined);
    }
    realm.get_str(dict.clone(), key)
}

/// Log through the environment (and the console sink).
pub(crate) fn log(level: &str, msg: &str) {
    with_env(|env| env.log(level, msg));
}

/// Report an uncaught exception.
pub(crate) fn report_error(realm: &mut Realm, e: &Value, context: &str) {
    let msg = realm.error_to_string(e);
    let text = if context.is_empty() { format!("Uncaught {msg}") } else { format!("Uncaught {msg} ({context})") };
    log("error", &text);
}

/// Element tag of a node (lower-case) or None for non-elements.
pub(crate) fn tag_of(n: NodeRef) -> Option<String> {
    with_doc(n.doc, |doc| doc.tag(n.node).map(|t| t.to_string()))
}

pub(crate) fn is_fragment(n: NodeRef) -> bool {
    with_doc(n.doc, |doc| doc.tag(n.node) == Some("#fragment"))
}

/// Base URL for resolving relative URLs in the main document.
pub(crate) fn base_url() -> rabbithole_net::Url {
    with_env(|env| env.base_url())
}

pub(crate) fn resolve_url(href: &str) -> Option<rabbithole_net::Url> {
    base_url().join(href.trim())
}

/// Call `f` and route an exception to the error log instead of propagating.
pub(crate) fn call_logged(realm: &mut Realm, f: &Value, this: Value, args: &[Value], context: &str) -> Value {
    match realm.call(f.clone(), this, args) {
        Ok(v) => v,
        Err(e) => {
            report_error(realm, &e, context);
            Value::Undefined
        }
    }
}
