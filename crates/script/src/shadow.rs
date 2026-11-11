//! Shadow DOM (`attachShadow`, `ShadowRoot`, slots, `slotchange`) and the
//! custom element registry with its lifecycle reactions (upgrade,
//! connected / disconnected / attributeChanged callbacks).
//!
//! The shadow trees themselves live in the DOM crate (`Document::
//! attach_shadow`, flat-tree helpers) so style and layout render them.

use crate::env::with_env;
use crate::host::*;
use crate::node;
use rabbithole_dom::{Document, NodeId};
use rabbithole_js::{JsResult, Realm, Value, CONFIGURABLE, ENUMERABLE};
use crate::host::HashMap;

const ACC: u8 = CONFIGURABLE | ENUMERABLE;

/// `ShadowRootInit` options remembered per shadow root.
#[derive(Clone, Copy, Debug)]
pub struct ShadowInfo {
    pub open: bool,
    pub delegates_focus: bool,
    pub manual: bool,
    pub clonable: bool,
    pub serializable: bool,
}

/// A queued custom element reaction (run by [`drain_ce_reactions`]).
#[derive(Debug)]
pub enum CeReaction {
    Upgrade(NodeRef),
    Connected(NodeRef),
    Disconnected(NodeRef),
    Attr(NodeRef, String, Option<String>, Option<String>),
}

/// A custom element definition.
#[derive(Clone)]
pub struct CeDefinition {
    pub name: String,
    pub ctor: Value,
    pub extends: Option<String>,
    pub observed: Vec<String>,
}

#[derive(Default)]
pub struct ShadowState {
    pub info: HashMap<NodeRef, ShadowInfo>,
    pub ce_queue: Vec<CeReaction>,
    pub definitions: HashMap<String, std::rc::Rc<CeDefinition>>,
    pub waiters: HashMap<String, Vec<Value>>,
    /// Last known assigned nodes per slot (for `slotchange`).
    pub slot_cache: HashMap<NodeRef, Vec<NodeId>>,
    /// Shadow roots whose slot assignment may have changed.
    pub slots_dirty: Vec<NodeRef>,
    pub slot_check_scheduled: bool,
    /// Manually assigned nodes (`slotAssignment: "manual"`).
    pub manual: HashMap<NodeRef, Vec<NodeId>>,
}

fn sstate<R>(f: impl FnOnce(&mut ShadowState) -> R) -> R {
    let st = state();
    let mut st = st.borrow_mut();
    f(&mut st.shadow)
}

// ------------------------------------------------------------- helpers

pub(crate) fn is_shadow_root(n: NodeRef) -> bool {
    with_doc(n.doc, |doc| doc.is_shadow_root(n.node))
}

pub(crate) fn shadow_info(root: NodeRef) -> Option<ShadowInfo> {
    if let Some(i) = sstate(|s| s.info.get(&root).copied()) {
        return Some(i);
    }
    // Declarative shadow roots (parsed): open or closed per the markup.
    let open = with_doc(root.doc, |doc| doc.declarative_shadow.get(&root.node).copied())?;
    Some(ShadowInfo { open, delegates_focus: false, manual: false, clonable: false, serializable: true })
}

/// The definition that applies to element `n` (autonomous by tag, or a
/// customized built-in through `is`).
pub(crate) fn definition_for(n: NodeRef) -> Option<std::rc::Rc<CeDefinition>> {
    let (tag, is) = with_doc(n.doc, |doc| (doc.tag(n.node).map(|t| t.to_string()), doc.attr(n.node, "is").map(|t| t.to_ascii_lowercase())));
    let tag = tag?;
    sstate(|s| {
        if let Some(d) = s.definitions.get(&tag) {
            if d.extends.is_none() {
                return Some(d.clone());
            }
        }
        if let Some(is) = is {
            if let Some(d) = s.definitions.get(&is) {
                if d.extends.as_deref() == Some(tag.as_str()) {
                    return Some(d.clone());
                }
            }
        }
        None
    })
}

fn any_definitions() -> bool {
    sstate(|s| !s.definitions.is_empty())
}

fn is_connected(n: NodeRef) -> bool {
    n.doc == 0 && with_doc(0, |doc| doc.is_connected(n.node))
}

/// Elements of the shadow-including subtree rooted at `n`.
fn subtree_elements(n: NodeRef) -> Vec<NodeRef> {
    with_doc(n.doc, |doc| doc.shadow_including_descendants(n.node).into_iter().filter(|&d| doc.element(d).map(|e| !e.tag.starts_with('#')).unwrap_or(false)).map(|d| NodeRef { doc: n.doc, node: d }).collect())
}

// ------------------------------------------------- mutation notifications

/// Called for every child-list mutation (from `node::record_child_list`):
/// queue custom element reactions and slot re-assignment checks.
pub(crate) fn on_child_list(parent: NodeRef, added: &[NodeRef], removed: &[NodeRef]) {
    let defs = any_definitions();
    let has_shadow = with_doc(parent.doc, |doc| !doc.shadow_roots.is_empty());
    if !defs && !has_shadow {
        return;
    }
    let connected = is_connected(parent);
    if defs && connected {
        let mut q = Vec::new();
        for r in removed {
            for e in subtree_elements(*r) {
                if state().borrow().ce_upgraded.contains(&e) {
                    q.push(CeReaction::Disconnected(e));
                }
            }
        }
        for a in added {
            for e in subtree_elements(*a) {
                if definition_for(e).is_some() {
                    q.push(CeReaction::Connected(e));
                }
            }
        }
        if !q.is_empty() {
            sstate(|s| s.ce_queue.extend(q));
        }
    }
    if has_shadow {
        // Children of a host changed → its slots; slots added/removed inside
        // a shadow tree → that tree's slots.
        let roots: Vec<NodeId> = with_doc(parent.doc, |doc| {
            let mut v = Vec::new();
            if let Some(r) = doc.shadow_root(parent.node) {
                v.push(r);
            }
            if let Some(r) = doc.containing_shadow_root(parent.node) {
                let touches_slot = added.iter().chain(removed).any(|n| doc.descendants(n.node).any(|d| doc.is_element(d, "slot")));
                if touches_slot {
                    v.push(r);
                }
            }
            v
        });
        for r in roots {
            mark_slots_dirty(NodeRef { doc: parent.doc, node: r });
        }
    }
}

/// Called after an attribute change.
pub(crate) fn on_attribute(n: NodeRef, name: &str, old: Option<String>, new: Option<&str>) {
    if state().borrow().ce_upgraded.contains(&n) {
        // Only observed attributes get a reaction (checked again when it runs).
        let observed = definition_for(n).map(|d| d.observed.iter().any(|o| o.as_str() == name || o.eq_ignore_ascii_case(name))).unwrap_or(false);
        if observed {
            sstate(|s| s.ce_queue.push(CeReaction::Attr(n, name.to_string(), old.clone(), new.map(|v| v.to_string()))));
            rabbithole_js::request_post_native();
        }
    }
    if name == "slot" || name == "name" {
        let root = with_doc(n.doc, |doc| {
            if doc.shadow_roots.is_empty() {
                return None;
            }
            if name == "slot" {
                doc.parent(n.node).and_then(|p| doc.shadow_root(p))
            } else if doc.is_element(n.node, "slot") {
                doc.containing_shadow_root(n.node)
            } else {
                None
            }
        });
        if let Some(r) = root {
            mark_slots_dirty(NodeRef { doc: n.doc, node: r });
        }
    }
}

/// Queue upgrade reactions for defined custom elements in a fresh clone
/// (`cloneNode` / `importNode` create elements that upgrade right away).
pub(crate) fn queue_upgrades(n: NodeRef) {
    if !any_definitions() {
        return;
    }
    let mut q = Vec::new();
    for e in subtree_elements(n) {
        if definition_for(e).is_some() {
            q.push(CeReaction::Upgrade(e));
        }
    }
    sstate(|s| s.ce_queue.extend(q));
}

fn mark_slots_dirty(root: NodeRef) {
    sstate(|s| {
        if !s.slots_dirty.contains(&root) {
            s.slots_dirty.push(root);
        }
    });
}

// ------------------------------------------------------- reactions

/// Run queued custom element reactions (upgrades and callbacks).
pub(crate) fn drain_ce_reactions(realm: &mut Realm) {
    for _ in 0..64 {
        let q = sstate(|s| std::mem::take(&mut s.ce_queue));
        if q.is_empty() {
            break;
        }
        for r in q {
            match r {
                CeReaction::Upgrade(n) => {
                    if let Some(def) = definition_for(n) {
                        crate::host::upgrade_custom_element(realm, n, &def.ctor);
                    }
                }
                CeReaction::Connected(n) => {
                    if !is_connected(n) {
                        continue;
                    }
                    let Some(def) = definition_for(n) else { continue };
                    let el = crate::host::upgrade_custom_element(realm, n, &def.ctor);
                    if !state().borrow().ce_upgraded.contains(&n) {
                        continue;
                    }
                    call_callback(realm, &el, "connectedCallback", &[]);
                }
                CeReaction::Disconnected(n) => {
                    if !state().borrow().ce_upgraded.contains(&n) {
                        continue;
                    }
                    let el = wrap(realm, n);
                    call_callback(realm, &el, "disconnectedCallback", &[]);
                }
                CeReaction::Attr(n, name, old, new) => {
                    let Some(def) = definition_for(n) else { continue };
                    if !def.observed.iter().any(|o| o.as_str() == name || o.eq_ignore_ascii_case(&name)) {
                        continue;
                    }
                    let el = wrap(realm, n);
                    let v = |s: &Option<String>| s.as_deref().map(Value::str).unwrap_or(Value::Null);
                    call_callback(realm, &el, "attributeChangedCallback", &[Value::str(&name), v(&old), v(&new), Value::Null]);
                }
            }
        }
    }
}

fn call_callback(realm: &mut Realm, el: &Value, name: &str, args: &[Value]) {
    match realm.get_str(el.clone(), name) {
        Ok(cb) if realm.is_callable(&cb) => {
            call_logged(realm, &cb, el.clone(), args, name);
        }
        Ok(_) => {}
        Err(e) => report_error(realm, &e, name),
    }
}

/// Deliver pending `slotchange` events (microtask checkpoint).
pub(crate) fn check_slots(realm: &mut Realm) {
    let roots = sstate(|s| {
        s.slot_check_scheduled = false;
        std::mem::take(&mut s.slots_dirty)
    });
    let mut changed: Vec<NodeRef> = Vec::new();
    for root in roots {
        let slots: Vec<(NodeRef, Vec<NodeId>)> = with_doc(root.doc, |doc| {
            if !doc.is_shadow_root(root.node) {
                return Vec::new();
            }
            doc.descendants(root.node).filter(|&d| doc.is_element(d, "slot")).map(|s| (NodeRef { doc: root.doc, node: s }, doc.assigned_nodes(s))).collect()
        });
        for (slot, now) in slots {
            let prev = sstate(|s| s.slot_cache.insert(slot, now.clone())).unwrap_or_default();
            if prev != now {
                changed.push(slot);
            }
        }
    }
    for slot in changed {
        crate::events::dispatch(realm, TargetKey::Node(slot), "slotchange", crate::EventInit::simple(true, false), false);
    }
}

pub(crate) fn has_pending_slot_checks() -> bool {
    sstate(|s| !s.slots_dirty.is_empty())
}

pub(crate) fn has_pending_reactions() -> bool {
    sstate(|s| !s.ce_queue.is_empty())
}

// --------------------------------------------------------- attachShadow

const VALID_HOSTS: &[&str] = &["article", "aside", "blockquote", "body", "div", "footer", "h1", "h2", "h3", "h4", "h5", "h6", "header", "main", "nav", "p", "section", "span"];

fn e_attach_shadow(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "attachShadow")?;
    let init = arg(args, 0);
    let mode = dict_str(realm, &init, "mode")?;
    let open = match mode.as_deref() {
        Some("open") => true,
        Some("closed") => false,
        _ => return Err(realm.throw_type_error("Failed to execute 'attachShadow' on 'Element': The provided value is not a valid enum value of type ShadowRootMode.")),
    };
    let tag = tag_of(n).unwrap_or_default();
    if !(tag.contains('-') || VALID_HOSTS.contains(&tag.as_str())) {
        return Err(crate::misc::dom_exception(realm, "NotSupportedError", &format!("Failed to execute 'attachShadow' on 'Element': This element does not support attachShadow")));
    }
    // A declarative shadow root (`<template shadowrootmode>`) of the same
    // mode is taken over: emptied and returned.
    let declarative = with_doc(n.doc, |doc| doc.shadow_root(n.node).and_then(|r| doc.declarative_shadow.get(&r).map(|o| (r, *o))));
    if let Some((r, was_open)) = declarative {
        if was_open == open {
            with_doc(n.doc, |doc| {
                doc.declarative_shadow.remove(&r);
                doc.clear_children(r);
            });
            if n.doc == 0 {
                mark_dirty();
            }
            return Ok(wrap(realm, NodeRef { doc: n.doc, node: r }));
        }
    }
    if with_doc(n.doc, |doc| doc.shadow_root(n.node).is_some()) {
        return Err(crate::misc::dom_exception(realm, "NotSupportedError", "Failed to execute 'attachShadow' on 'Element': Shadow root cannot be created on a host which already hosts a shadow tree."));
    }
    let info = ShadowInfo {
        open,
        delegates_focus: dict_bool(realm, &init, "delegatesFocus")?.unwrap_or(false),
        manual: dict_str(realm, &init, "slotAssignment")?.as_deref() == Some("manual"),
        clonable: dict_bool(realm, &init, "clonable")?.unwrap_or(false),
        serializable: dict_bool(realm, &init, "serializable")?.unwrap_or(false),
    };
    let root = with_doc(n.doc, |doc| doc.attach_shadow(n.node));
    let root = NodeRef { doc: n.doc, node: root };
    sstate(|s| {
        s.info.insert(root, info);
    });
    if n.doc == 0 {
        mark_dirty();
    }
    Ok(wrap(realm, root))
}

fn e_shadow_root(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "shadowRoot")?;
    let root = with_doc(n.doc, |doc| doc.shadow_root(n.node));
    match root {
        Some(r) => {
            let r = NodeRef { doc: n.doc, node: r };
            if shadow_info(r).map(|i| i.open).unwrap_or(true) {
                Ok(wrap(realm, r))
            } else {
                Ok(Value::Null)
            }
        }
        None => Ok(Value::Null),
    }
}

/// `Element.assignedSlot` / `Text.assignedSlot` (null for closed trees).
fn n_assigned_slot(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let n = this_node(realm, &this, "assignedSlot")?;
    match assigned_slot(n) {
        Some(s) => {
            let root = with_doc(n.doc, |doc| doc.containing_shadow_root(s.node)).map(|r| NodeRef { doc: n.doc, node: r });
            if root.and_then(shadow_info).map(|i| i.open).unwrap_or(true) {
                Ok(wrap(realm, s))
            } else {
                Ok(Value::Null)
            }
        }
        None => Ok(Value::Null),
    }
}

/// The slot `n` is assigned to (named or manual assignment).
pub(crate) fn assigned_slot(n: NodeRef) -> Option<NodeRef> {
    let (parent_root, by_name) = with_doc(n.doc, |doc| {
        let pr = doc.parent(n.node).and_then(|p| doc.shadow_root(p));
        (pr, doc.assigned_slot(n.node))
    });
    let root = NodeRef { doc: n.doc, node: parent_root? };
    if shadow_info(root).map(|i| i.manual).unwrap_or(false) {
        let slot = sstate(|s| s.manual.iter().find(|(slot, nodes)| slot.doc == n.doc && nodes.contains(&n.node)).map(|(s, _)| *s));
        return slot.filter(|s| with_doc(n.doc, |doc| doc.containing_shadow_root(s.node)) == Some(root.node));
    }
    by_name.map(|s| NodeRef { doc: n.doc, node: s })
}

/// Assigned nodes of a slot (manual or by name).
pub(crate) fn assigned_nodes(slot: NodeRef) -> Vec<NodeRef> {
    let root = with_doc(slot.doc, |doc| doc.containing_shadow_root(slot.node));
    if let Some(r) = root {
        let r = NodeRef { doc: slot.doc, node: r };
        if shadow_info(r).map(|i| i.manual).unwrap_or(false) {
            let host = with_doc(slot.doc, |doc| doc.shadow_host(r.node));
            let list = sstate(|s| s.manual.get(&slot).cloned().unwrap_or_default());
            return with_doc(slot.doc, |doc| list.into_iter().filter(|&c| Some(c) != None && doc.parent(c) == host).map(|c| NodeRef { doc: slot.doc, node: c }).collect());
        }
    }
    with_doc(slot.doc, |doc| doc.assigned_nodes(slot.node).into_iter().map(|c| NodeRef { doc: slot.doc, node: c }).collect())
}

fn flatten_assigned(slot: NodeRef, out: &mut Vec<NodeRef>, depth: usize) {
    let assigned = assigned_nodes(slot);
    let list = if assigned.is_empty() { node::children_of(slot) } else { assigned };
    for n in list {
        let is_slot = with_doc(n.doc, |doc| doc.is_element(n.node, "slot") && doc.containing_shadow_root(n.node).is_some());
        if is_slot && depth < 32 {
            flatten_assigned(n, out, depth + 1);
        } else {
            out.push(n);
        }
    }
}

fn slot_nodes(realm: &mut Realm, this: &Value, args: &[Value], elements_only: bool) -> JsResult<Vec<NodeRef>> {
    let n = this_node(realm, this, "assignedNodes")?;
    let flatten = dict_bool(realm, &arg(args, 0), "flatten")?.unwrap_or(false);
    let in_shadow = with_doc(n.doc, |doc| doc.containing_shadow_root(n.node).is_some());
    let mut out = Vec::new();
    if flatten {
        if in_shadow {
            flatten_assigned(n, &mut out, 0);
        }
    } else {
        out = assigned_nodes(n);
    }
    if elements_only {
        out.retain(|x| with_doc(x.doc, |doc| doc.element(x.node).is_some()));
    }
    Ok(out)
}

fn slot_assigned_nodes(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let v = slot_nodes(realm, &this, args, false)?;
    let vals: Vec<Value> = v.into_iter().map(|n| wrap(realm, n)).collect();
    Ok(realm.new_array_value(vals))
}

fn slot_assigned_elements(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let v = slot_nodes(realm, &this, args, true)?;
    let vals: Vec<Value> = v.into_iter().map(|n| wrap(realm, n)).collect();
    Ok(realm.new_array_value(vals))
}

fn slot_assign(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let slot = this_node(realm, &this, "assign")?;
    let mut nodes = Vec::new();
    for a in args {
        match node_of(realm, a) {
            Some(n) => nodes.push(n.node),
            None => return Err(realm.throw_type_error("Failed to execute 'assign' on 'HTMLSlotElement': parameter is not of type 'Element' or 'Text'.")),
        }
    }
    sstate(|s| {
        for list in s.manual.values_mut() {
            list.retain(|x| !nodes.contains(x));
        }
        s.manual.insert(slot, nodes);
    });
    if let Some(r) = with_doc(slot.doc, |doc| doc.containing_shadow_root(slot.node)) {
        mark_slots_dirty(NodeRef { doc: slot.doc, node: r });
    }
    if slot.doc == 0 {
        mark_dirty();
    }
    schedule_microtask_work(realm);
    Ok(Value::Undefined)
}

// ------------------------------------------------------------ ShadowRoot

fn this_root(realm: &mut Realm, this: &Value) -> JsResult<NodeRef> {
    let n = this_node(realm, this, "ShadowRoot")?;
    if !is_shadow_root(n) {
        return Err(realm.throw_type_error("Illegal invocation: not a ShadowRoot"));
    }
    Ok(n)
}

fn sr_mode(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let r = this_root(realm, &this)?;
    Ok(Value::str(if shadow_info(r).map(|i| i.open).unwrap_or(true) { "open" } else { "closed" }))
}

fn sr_host(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let r = this_root(realm, &this)?;
    let h = with_doc(r.doc, |doc| doc.shadow_host(r.node));
    Ok(wrap_opt(realm, r.doc, h))
}

fn sr_delegates_focus(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let r = this_root(realm, &this)?;
    Ok(Value::Bool(shadow_info(r).map(|i| i.delegates_focus).unwrap_or(false)))
}

fn sr_slot_assignment(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let r = this_root(realm, &this)?;
    Ok(Value::str(if shadow_info(r).map(|i| i.manual).unwrap_or(false) { "manual" } else { "named" }))
}

fn sr_clonable(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let r = this_root(realm, &this)?;
    Ok(Value::Bool(shadow_info(r).map(|i| i.clonable).unwrap_or(false)))
}

fn sr_serializable(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let r = this_root(realm, &this)?;
    Ok(Value::Bool(shadow_info(r).map(|i| i.serializable).unwrap_or(false)))
}

fn sr_inner_html_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let r = this_root(realm, &this)?;
    Ok(Value::str(&with_doc(r.doc, |doc| doc.serialize(r.node, false))))
}

fn sr_inner_html_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let r = this_root(realm, &this)?;
    let html = arg_str(realm, args, 0)?;
    node::set_inner_html(realm, r, &html)?;
    drain_ce_reactions(realm);
    Ok(Value::Undefined)
}

fn sr_active_element(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let r = this_root(realm, &this)?;
    let focused = state().borrow().focused;
    let Some(f) = focused else { return Ok(Value::Null) };
    if r.doc != 0 {
        return Ok(Value::Null);
    }
    // Retarget the focused element against this shadow root.
    let t = with_doc(0, |doc| retarget(doc, f, r.node));
    let in_tree = with_doc(0, |doc| doc.tree_root(t) == r.node && t != r.node);
    Ok(if in_tree { wrap_main(realm, t) } else { Value::Null })
}

/// HTML's "retarget A against B".
pub(crate) fn retarget(doc: &Document, mut a: NodeId, b: NodeId) -> NodeId {
    loop {
        let root = doc.tree_root(a);
        if !doc.is_shadow_root(root) || doc.is_shadow_including_inclusive_ancestor(root, b) {
            return a;
        }
        match doc.shadow_host(root) {
            Some(h) => a = h,
            None => return a,
        }
    }
}

fn sr_get_html(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    sr_inner_html_get(realm, this, &[])
}

fn sr_style_sheets(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let r = this_root(realm, &this)?;
    let nodes: Vec<NodeRef> = with_doc(r.doc, |doc| {
        doc.descendants(r.node)
            .filter(|&n| doc.is_element(n, "style") || (doc.is_element(n, "link") && doc.attr(n, "rel").map(|v| v.to_ascii_lowercase().contains("stylesheet")).unwrap_or(false)))
            .map(|n| NodeRef { doc: r.doc, node: n })
            .collect()
    });
    let mut sheets = Vec::new();
    for n in nodes {
        let w = wrap(realm, n);
        let s = realm.get_str(w, "sheet")?;
        if !s.is_null() && !s.is_undefined() {
            sheets.push(s);
        }
    }
    Ok(realm.new_array_value(sheets))
}

fn sr_element_from_point(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let r = this_root(realm, &this)?;
    if r.doc != 0 {
        return Ok(Value::Null);
    }
    let x = realm.to_number(arg(args, 0))? as f32;
    let y = realm.to_number(arg(args, 1))? as f32;
    match crate::document::hit_test(x, y) {
        Some(h) => {
            let t = with_doc(0, |doc| retarget(doc, h, r.node));
            let inside = with_doc(0, |doc| doc.tree_root(t) == r.node);
            Ok(if inside { wrap_main(realm, t) } else { Value::Null })
        }
        None => Ok(Value::Null),
    }
}

fn sr_elements_from_point(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let v = sr_element_from_point(realm, this, args)?;
    let list = if v.is_null() { vec![] } else { vec![v] };
    Ok(realm.new_array_value(list))
}

fn sr_null(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Null)
}

fn sr_get_selection(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    let w = Value::Object(state().borrow().window);
    let f = realm.get_str(w.clone(), "getSelection")?;
    realm.call(f, w, &[])
}

fn sr_get_animations(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(realm.new_array_value(vec![]))
}

// ---------------------------------------------------- custom elements

fn ce_define(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let name = arg_str(realm, args, 0)?;
    let ctor = arg(args, 1);
    if !valid_custom_element_name(&name) {
        return Err(crate::misc::dom_exception(realm, "SyntaxError", &format!("Failed to execute 'define' on 'CustomElementRegistry': \"{name}\" is not a valid custom element name")));
    }
    if !realm.is_constructor(&ctor) {
        return Err(realm.throw_type_error("Failed to execute 'define' on 'CustomElementRegistry': The provided callback is not a constructor."));
    }
    let dup = sstate(|s| s.definitions.contains_key(&name) || s.definitions.values().any(|d| realm.strict_equals(&d.ctor, &ctor)));
    if dup {
        return Err(crate::misc::dom_exception(realm, "NotSupportedError", &format!("Failed to execute 'define' on 'CustomElementRegistry': the name \"{name}\" has already been used with this registry")));
    }
    let extends = dict_str(realm, &arg(args, 2), "extends")?.map(|e| e.to_ascii_lowercase());
    // Callbacks are looked up on the prototype at define time per spec; we
    // look them up at call time (equivalent unless the prototype is
    // mutated afterwards). observedAttributes is read once now.
    let observed_v = realm.get_str(ctor.clone(), "observedAttributes")?;
    let mut observed = Vec::new();
    if observed_v.is_object() {
        let len = realm.get_str(observed_v.clone(), "length")?;
        let len = realm.to_number(len)? as usize;
        for i in 0..len.min(1024) {
            let v = realm.get_str(observed_v.clone(), &i.to_string())?;
            observed.push(realm.to_rust_string(v)?.to_ascii_lowercase());
        }
    }
    root_value(realm, &ctor);
    let def = CeDefinition { name: name.clone(), ctor: ctor.clone(), extends: extends.clone(), observed };
    sstate(|s| {
        s.definitions.insert(name.clone(), std::rc::Rc::new(def));
    });
    // Keep the legacy map (used by the HTMLElement constructor lookup).
    state().borrow_mut().custom_elements.insert(name.clone(), ctor.clone());
    // Upgrade connected candidates in shadow-including tree order.
    let candidates: Vec<NodeRef> = with_env(|env| {
        let doc = env.doc();
        doc.shadow_including_descendants(Document::ROOT)
            .into_iter()
            .filter(|&n| match &extends {
                None => doc.tag(n) == Some(name.as_str()),
                Some(ext) => doc.tag(n) == Some(ext.as_str()) && doc.attr(n, "is").map(|v| v.eq_ignore_ascii_case(&name)).unwrap_or(false),
            })
            .map(NodeRef::main)
            .collect()
    });
    for n in candidates {
        sstate(|s| s.ce_queue.push(CeReaction::Connected(n)));
    }
    drain_ce_reactions(realm);
    // Resolve whenDefined promises.
    let waiters = sstate(|s| s.waiters.remove(&name)).unwrap_or_default();
    for w in waiters {
        realm.call(w.clone(), Value::Undefined, &[ctor.clone()])?;
        unroot_value(realm, &w);
    }
    Ok(Value::Undefined)
}

fn valid_custom_element_name(name: &str) -> bool {
    const RESERVED: &[&str] = &["annotation-xml", "color-profile", "font-face", "font-face-src", "font-face-uri", "font-face-format", "font-face-name", "missing-glyph"];
    let mut chars = name.chars();
    matches!(chars.next(), Some('a'..='z')) && name.contains('-') && !name.chars().any(|c| c.is_ascii_uppercase() || c.is_whitespace() || matches!(c, '<' | '>' | '/' | '"' | '\'' | '=')) && !RESERVED.contains(&name)
}

fn ce_get(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let name = arg_str(realm, args, 0)?;
    Ok(sstate(|s| s.definitions.get(&name).map(|d| d.ctor.clone())).unwrap_or(Value::Undefined))
}

fn ce_get_name(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let c = arg(args, 0);
    let defs: Vec<std::rc::Rc<CeDefinition>> = sstate(|s| s.definitions.values().cloned().collect());
    for d in defs {
        if realm.strict_equals(&d.ctor, &c) {
            return Ok(Value::str(&d.name));
        }
    }
    Ok(Value::Null)
}

fn ce_when_defined(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let name = arg_str(realm, args, 0)?;
    let (p, resolve, reject) = realm.new_promise();
    if !valid_custom_element_name(&name) {
        let e = crate::misc::dom_exception(realm, "SyntaxError", &format!("\"{name}\" is not a valid custom element name"));
        realm.call(Value::Object(reject), Value::Undefined, &[e])?;
        return Ok(Value::Object(p));
    }
    let existing = sstate(|s| s.definitions.get(&name).map(|d| d.ctor.clone()));
    match existing {
        Some(c) => {
            realm.call(Value::Object(resolve), Value::Undefined, &[c])?;
        }
        None => {
            root_value(realm, &Value::Object(resolve));
            sstate(|s| s.waiters.entry(name).or_default().push(Value::Object(resolve)));
        }
    }
    Ok(Value::Object(p))
}

fn ce_upgrade(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(n) = node_of(realm, &arg(args, 0)) else {
        return Err(realm.throw_type_error("Failed to execute 'upgrade' on 'CustomElementRegistry': parameter 1 is not of type 'Node'."));
    };
    queue_upgrades(n);
    drain_ce_reactions(realm);
    Ok(Value::Undefined)
}

// -------------------------------------------------------- scheduling

fn microtask_work(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    drain_ce_reactions(realm);
    check_slots(realm);
    Ok(Value::Undefined)
}

/// Make sure pending reactions / slot checks run at the next microtask.
pub(crate) fn schedule_microtask_work(realm: &mut Realm) {
    let need = sstate(|s| {
        let pending = !s.ce_queue.is_empty() || !s.slots_dirty.is_empty();
        if pending && !s.slot_check_scheduled {
            s.slot_check_scheduled = true;
            true
        } else {
            false
        }
    });
    if need {
        let f = state().borrow().protos.get("__shadow_microtask").copied();
        if let Some(f) = f {
            realm.enqueue_callback(Value::Object(f), Value::Undefined, Vec::new());
        }
    }
}

/// [`rabbithole_js::Realm::post_native`] hook: reactions queued by the DOM
/// method that just returned.
pub(crate) fn run_pending_reactions(realm: &mut Realm) {
    if has_pending_reactions() {
        drain_ce_reactions(realm);
    }
}

/// Run pending work now (entry points / after mutating natives).
pub(crate) fn flush(realm: &mut Realm) {
    if has_pending_reactions() {
        drain_ce_reactions(realm);
    }
    schedule_microtask_work(realm);
}

// --------------------------------------------------------------- install

pub(crate) fn install(realm: &mut Realm) {
    let el = proto("Element");
    realm.define_method(el, "attachShadow", 1, e_attach_shadow);
    realm.define_accessor(el, "shadowRoot", Some(e_shadow_root), None, ACC);
    realm.define_accessor(el, "assignedSlot", Some(n_assigned_slot), None, ACC);
    let text = proto("Text");
    realm.define_accessor(text, "assignedSlot", Some(n_assigned_slot), None, ACC);

    let sr = define_class(realm, "ShadowRoot", Some("DocumentFragment"), illegal_ctor);
    for (name, g) in [
        ("mode", sr_mode as rabbithole_js::NativeFn),
        ("host", sr_host),
        ("delegatesFocus", sr_delegates_focus),
        ("slotAssignment", sr_slot_assignment),
        ("clonable", sr_clonable),
        ("serializable", sr_serializable),
        ("activeElement", sr_active_element),
        ("styleSheets", sr_style_sheets),
        ("fullscreenElement", sr_null),
        ("pointerLockElement", sr_null),
        ("pictureInPictureElement", sr_null),
    ] {
        realm.define_accessor(sr, name, Some(g), None, ACC);
    }
    realm.define_accessor(sr, "innerHTML", Some(sr_inner_html_get), Some(sr_inner_html_set), ACC);
    realm.define_method(sr, "getHTML", 0, sr_get_html);
    realm.define_method(sr, "setHTMLUnsafe", 1, sr_inner_html_set);
    realm.define_method(sr, "elementFromPoint", 2, sr_element_from_point);
    realm.define_method(sr, "elementsFromPoint", 2, sr_elements_from_point);
    realm.define_method(sr, "getSelection", 0, sr_get_selection);
    realm.define_method(sr, "getAnimations", 0, sr_get_animations);
    crate::events::install_handler_props(realm, sr, &["slotchange"]);

    let slot = proto("HTMLSlotElement");
    realm.define_method(slot, "assignedNodes", 0, slot_assigned_nodes);
    realm.define_method(slot, "assignedElements", 0, slot_assigned_elements);
    realm.define_method(slot, "assign", 0, slot_assign);
    crate::element::reflect(realm, slot, "name", "name", crate::element::Reflect::Str);

    let reg_proto = define_class(realm, "CustomElementRegistry", None, illegal_ctor);
    realm.define_method(reg_proto, "define", 2, ce_define);
    realm.define_method(reg_proto, "get", 1, ce_get);
    realm.define_method(reg_proto, "getName", 1, ce_get_name);
    realm.define_method(reg_proto, "whenDefined", 1, ce_when_defined);
    realm.define_method(reg_proto, "upgrade", 1, ce_upgrade);
    let reg = realm.new_object_with_proto(Some(reg_proto));
    realm.set_class_name(reg, "CustomElementRegistry");
    realm.extra_roots.push(Value::Object(reg));
    let g = realm.global();
    realm.define_property(g, "customElements", Value::Object(reg), ACC);
    state().borrow_mut().protos.insert("__customElements", reg);

    let f = realm.new_native_function("shadowMicrotask", 0, microtask_work);
    realm.extra_roots.push(Value::Object(f));
    state().borrow_mut().protos.insert("__shadow_microtask", f);
}
