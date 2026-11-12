//! Live boundary points and NodeIterator positions for the JavaScript
//! `Range` / `NodeIterator` implementations (`js/dom.js`).
//!
//! The positions are stored in the node's [`Document`] (see
//! `rabbithole_dom::live`), so every tree and character-data mutation keeps
//! them up to date (DOM §5.5 "live ranges", §6.1 "pre-removing steps").
//! A JavaScript handle object owns its slot; when the collector frees the
//! handle the slot is released lazily.

use crate::host::*;
use rabbithole_js::V;
use rabbithole_dom::{BoundaryPoint, Document, IteratorPosition};
use rabbithole_js::{JsResult, Realm, Value, CONFIGURABLE, ENUMERABLE};
use std::cell::RefCell;

/// Slots of collected handles of one realm: (is iterator, doc, slot).
///
/// Per realm, not per thread: `doc` numbers are the realm's own (its main
/// document is 0, its `DOMParser` documents 1, 2, ...), and the realms of
/// a page's frames and of the next page in the tab share the thread. A
/// thread-wide list let one realm release another realm's slots, in a
/// document it doesn't have.
pub(crate) type FreedSlots = std::rc::Rc<RefCell<Vec<(bool, u32, usize)>>>;

/// A live boundary point owned by a JS handle.
struct PointHandle {
    doc: u32,
    slot: usize,
    freed: FreedSlots,
}

/// A live NodeIterator position owned by a JS handle.
struct IterHandle {
    doc: u32,
    slot: usize,
    freed: FreedSlots,
}

/// `slot` of a handle whose slot was already released.
const RELEASED: usize = usize::MAX;

impl Drop for PointHandle {
    fn drop(&mut self) {
        if self.slot == RELEASED {
            return;
        }
        if let Ok(mut f) = self.freed.try_borrow_mut() {
            f.push((false, self.doc, self.slot));
        }
    }
}

impl Drop for IterHandle {
    fn drop(&mut self) {
        if let Ok(mut f) = self.freed.try_borrow_mut() {
            f.push((true, self.doc, self.slot));
        }
    }
}

fn freed_slots() -> FreedSlots {
    state().borrow().live_freed.clone()
}

/// Release the slots of this realm's collected handles.
fn purge() {
    let freed = std::mem::take(&mut *freed_slots().borrow_mut());
    for (iter, doc, slot) in freed {
        with_doc(doc, |d| if iter { d.remove_live_iterator(slot) } else { d.remove_live_point(slot) });
    }
}

fn node_arg(realm: &mut Realm, v: &Value) -> JsResult<NodeRef> {
    match node_of(realm, v) {
        Some(n) => Ok(n),
        None => Err(realm.throw_type_error("parameter 1 is not of type 'Node'.")),
    }
}

fn offset_arg(realm: &mut Realm, v: Value) -> JsResult<usize> {
    Ok(realm.to_number(v)? as i64 as u32 as usize)
}

fn handle_obj(realm: &mut Realm, this: &Value) -> JsResult<rabbithole_js::ObjectId> {
    match this.v() {
        V::Object(o) if realm.host_data::<PointHandle>(o).is_some() || realm.host_data::<IterHandle>(o).is_some() => Ok(o),
        _ => Err(realm.throw_type_error("Illegal invocation")),
    }
}

// ---------------------------------------------------------- boundary points

/// `liveBoundary(node, offset)`: a handle whose `node` / `offset` follow
/// DOM mutations.
fn live_boundary(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    purge();
    let n = node_arg(realm, &arg(args, 0))?;
    let offset = offset_arg(realm, arg(args, 1))?;
    let slot = with_doc(n.doc, |d| d.add_live_point(BoundaryPoint { node: n.node, offset }));
    let o = realm.new_object_with_proto(Some(proto("__LiveBoundary")));
    realm.set_host_data(o, Box::new(PointHandle { doc: n.doc, slot, freed: freed_slots() }));
    Ok(Value::Object(o))
}

fn point(realm: &mut Realm, this: &Value) -> JsResult<(rabbithole_js::ObjectId, u32, usize, BoundaryPoint)> {
    let o = handle_obj(realm, this)?;
    let Some(h) = realm.host_data::<PointHandle>(o) else { return Err(realm.throw_type_error("Illegal invocation")) };
    let (doc, slot) = (h.doc, h.slot);
    let p = with_doc(doc, |d| d.live_point(slot)).unwrap_or(BoundaryPoint { node: Document::ROOT, offset: 0 });
    Ok((o, doc, slot, p))
}

fn boundary_node_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let (_, doc, _, p) = point(realm, &this)?;
    Ok(wrap(realm, NodeRef { doc, node: p.node }))
}

fn boundary_node_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, doc, slot, p) = point(realm, &this)?;
    let n = node_arg(realm, &arg(args, 0))?;
    if n.doc == doc {
        with_doc(doc, |d| d.set_live_point(slot, BoundaryPoint { node: n.node, offset: p.offset }));
    } else {
        // Moving to another document: the slot moves to its arena.
        with_doc(doc, |d| d.remove_live_point(slot));
        let new_slot = with_doc(n.doc, |d| d.add_live_point(BoundaryPoint { node: n.node, offset: p.offset }));
        if let Some(h) = realm.host_data_mut::<PointHandle>(o) {
            // (The slot just released is not queued again.)
            let freed = h.freed.clone();
            let mut old = std::mem::replace(h, PointHandle { doc: n.doc, slot: new_slot, freed });
            old.slot = RELEASED;
        }
    }
    Ok(Value::Undefined)
}

fn boundary_offset_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let (_, _, _, p) = point(realm, &this)?;
    Ok(Value::Number(p.offset as f64))
}

fn boundary_offset_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, doc, slot, p) = point(realm, &this)?;
    let offset = offset_arg(realm, arg(args, 0))?;
    with_doc(doc, |d| d.set_live_point(slot, BoundaryPoint { node: p.node, offset }));
    Ok(Value::Undefined)
}

// ------------------------------------------------------ iterator positions

/// `liveIteratorPosition(root, node, before)`: a NodeIterator position
/// (`node`, `before`) that follows the pre-removing steps.
fn live_iterator_position(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    purge();
    let root = node_arg(realm, &arg(args, 0))?;
    let n = node_arg(realm, &arg(args, 1))?;
    let before = realm.to_boolean(&arg(args, 2));
    let node = if n.doc == root.doc { n.node } else { root.node };
    let slot = with_doc(root.doc, |d| d.add_live_iterator(IteratorPosition { root: root.node, node, before }));
    let o = realm.new_object_with_proto(Some(proto("__LiveIteratorPosition")));
    realm.set_host_data(o, Box::new(IterHandle { doc: root.doc, slot, freed: freed_slots() }));
    Ok(Value::Object(o))
}

fn iter_pos(realm: &mut Realm, this: &Value) -> JsResult<(u32, usize, IteratorPosition)> {
    let o = handle_obj(realm, this)?;
    let Some(h) = realm.host_data::<IterHandle>(o) else { return Err(realm.throw_type_error("Illegal invocation")) };
    let (doc, slot) = (h.doc, h.slot);
    let p = with_doc(doc, |d| d.live_iterator(slot)).unwrap_or(IteratorPosition { root: Document::ROOT, node: Document::ROOT, before: true });
    Ok((doc, slot, p))
}

fn iter_node_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let (doc, _, p) = iter_pos(realm, &this)?;
    Ok(wrap(realm, NodeRef { doc, node: p.node }))
}

fn iter_node_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (doc, slot, p) = iter_pos(realm, &this)?;
    let n = node_arg(realm, &arg(args, 0))?;
    if n.doc == doc {
        with_doc(doc, |d| d.set_live_iterator(slot, IteratorPosition { node: n.node, ..p }));
    }
    Ok(Value::Undefined)
}

fn iter_before_get(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let (_, _, p) = iter_pos(realm, &this)?;
    Ok(Value::Bool(p.before))
}

fn iter_before_set(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let (doc, slot, p) = iter_pos(realm, &this)?;
    let before = realm.to_boolean(&arg(args, 0));
    with_doc(doc, |d| d.set_live_iterator(slot, IteratorPosition { before, ..p }));
    Ok(Value::Undefined)
}

/// Add `liveBoundary` / `liveIteratorPosition` to the `__rhNative` object.
pub(crate) fn install(realm: &mut Realm, natives: rabbithole_js::ObjectId) {
    const ACC: u8 = CONFIGURABLE | ENUMERABLE;
    let bp = realm.new_object();
    realm.define_accessor(bp, "node", Some(boundary_node_get), Some(boundary_node_set), ACC);
    realm.define_accessor(bp, "offset", Some(boundary_offset_get), Some(boundary_offset_set), ACC);
    realm.extra_roots.push(Value::Object(bp));
    state().borrow_mut().protos.insert("__LiveBoundary", bp);
    let ip = realm.new_object();
    realm.define_accessor(ip, "node", Some(iter_node_get), Some(iter_node_set), ACC);
    realm.define_accessor(ip, "before", Some(iter_before_get), Some(iter_before_set), ACC);
    realm.extra_roots.push(Value::Object(ip));
    state().borrow_mut().protos.insert("__LiveIteratorPosition", ip);
    realm.define_method(natives, "liveBoundary", 2, live_boundary);
    realm.define_method(natives, "liveIteratorPosition", 3, live_iterator_position);
}
