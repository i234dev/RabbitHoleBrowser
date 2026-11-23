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
use rabbithole_dom::{BoundaryPoint, Document, DocumentWeak, IteratorPosition};
use std::rc::Rc;
use rabbithole_js::{JsResult, Realm, Value, CONFIGURABLE, ENUMERABLE};
use std::cell::RefCell;

/// Slots of collected handles of one realm: (is iterator, doc, slot).
///
/// Per realm, not per thread: `doc` numbers are the realm's own (its main
/// document is 0, its `DOMParser` documents 1, 2, ...), and the realms of
/// a page's frames and of the next page in the tab share the thread. A
/// thread-wide list let one realm release another realm's slots, in a
/// document it doesn't have.
pub(crate) type FreedSlots = std::rc::Rc<RefCell<Vec<(bool, u32, usize, Option<DocumentWeak>)>>>;

/// A live boundary point owned by a JS handle.
struct PointHandle {
    doc: u32,
    slot: usize,
    freed: FreedSlots,
    identity: Option<DocumentWeak>,
}

// Only points which actually cross a same-origin frame boundary need a
// shared owner. Both wrappers then see endpoint assignments as well as
// character-data and tree mutations recorded in the document's slot.
#[derive(Clone)]
struct SharedPoint(Rc<RefCell<PointOwner>>);

struct PointOwner {
    auxiliary: Option<crate::AuxiliaryDocument>,
    frame: u64,
    id: u64,
    handle: PointHandle,
}

impl Drop for PointOwner {
    fn drop(&mut self) {
        if let Some(arena) = &self.auxiliary {
            if let Ok(mut doc) = arena.0.try_borrow_mut() {
                doc.remove_live_point(self.handle.slot);
                self.handle.slot = RELEASED;
            }
        }
    }
}

impl SharedPoint {
    fn position(&self) -> Option<(u32,usize,BoundaryPoint)> {
        let owner = self.0.borrow();
        let h = &owner.handle;
        let here = crate::env::with_env(|env| env.frame_id());
        let doc = if let Some(arena) = &owner.auxiliary { arena.local_id() }
        else if owner.frame == here { h.doc } else {
            let target = if h.doc == 0 { owner.frame }
                else if h.doc & FRAME_DOC != 0 { u64::from(h.doc & !FRAME_DOC) }
                else { return None };
            if target == here { 0 } else if target < u64::from(FRAME_DOC) { FRAME_DOC | target as u32 } else { return None }
        };
        with_doc(doc, |d| {
            if !h.identity.as_ref()?.is_for(d) { return None; }
            Some((doc,h.slot,d.live_point(h.slot)?))
        })
    }

    fn export(realm: &mut Realm, value: Value) -> Option<Self> {
        let object = value.as_object()?;
        if let Some(shared) = realm.host_data::<Self>(object) { return Some(shared.clone()); }
        let handle = realm.host_data_mut::<PointHandle>(object)?;
        if handle.slot == RELEASED { return None; }
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT.fetch_update(std::sync::atomic::Ordering::Relaxed,std::sync::atomic::Ordering::Relaxed,|n| n.checked_add(1)).ok()?;
        let shared = Self(Rc::new(RefCell::new(PointOwner {
            auxiliary: crate::AuxiliaryDocument::lookup(handle.doc),
            frame: crate::env::with_env(|env| env.frame_id()), id,
            handle: PointHandle { doc: handle.doc, slot: handle.slot, freed: handle.freed.clone(),
                identity: Some(with_doc(handle.doc, |d| d.weak_identity())) },
        })));
        handle.slot = RELEASED;
        realm.set_host_data(object,Box::new(shared.clone()));
        Some(shared)
    }

    fn wrapper(&self, realm: &mut Realm) -> Value {
        let object = realm.new_object_with_proto(Some(proto("__LiveBoundary")));
        realm.set_host_data(object,Box::new(self.clone()));
        Value::Object(object)
    }
}

#[derive(Clone)]
pub struct SharedRange { start: SharedPoint, end: SharedPoint }

impl PartialEq for SharedRange {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.start.0,&other.start.0) && Rc::ptr_eq(&self.end.0,&other.end.0)
    }
}

impl std::fmt::Debug for SharedRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("SharedRange") }
}

impl SharedRange {
    fn key(&self) -> Value { Value::str(&self.start.0.borrow().id.to_string()) }

    pub(crate) fn export(realm: &mut Realm, range: Value) -> Option<Self> {
        let factory = state().borrow().factories.get("rangePoints")?.clone();
        let handles = realm.call(factory,Value::Undefined,std::slice::from_ref(&range)).ok()?;
        if !handles.is_object() { return None; }
        let a = realm.get_str(handles.clone(),"0").ok()?;
        let b = realm.get_str(handles,"1").ok()?;
        let shared = Self { start: SharedPoint::export(realm,a)?, end: SharedPoint::export(realm,b)? };
        let factory = state().borrow().factories.get("sharedRange")?.clone();
        realm.call(factory,Value::Undefined,&[shared.key(),range]).ok()?;
        Some(shared)
    }

    pub(crate) fn wrapper(&self, realm: &mut Realm) -> JsResult<Value> {
        let factory = state().borrow().factories.get("sharedRange").cloned();
        let Some(factory) = factory else { return Ok(Value::Undefined) };
        let a = self.start.wrapper(realm);
        let b = self.end.wrapper(realm);
        realm.call(factory,Value::Undefined,&[self.key(),Value::Null,a,b])
    }
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
            f.push((false, self.doc, self.slot, self.identity.clone()));
        }
    }
}

impl Drop for IterHandle {
    fn drop(&mut self) {
        if let Ok(mut f) = self.freed.try_borrow_mut() {
            f.push((true, self.doc, self.slot, None));
        }
    }
}

fn freed_slots() -> FreedSlots {
    state().borrow().live_freed.clone()
}

/// Release the slots of this realm's collected handles.
fn purge() {
    let freed = std::mem::take(&mut *freed_slots().borrow_mut());
    for (iter, doc, slot, identity) in freed {
        with_doc(doc, |d| {
            if identity.is_some_and(|id| !id.is_for(d)) { return; }
            if iter { d.remove_live_iterator(slot) } else { d.remove_live_point(slot) }
        });
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
        V::Object(o) if realm.host_data::<PointHandle>(o).is_some() || realm.host_data::<SharedPoint>(o).is_some() || realm.host_data::<IterHandle>(o).is_some() => Ok(o),
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
    realm.set_host_data(o, Box::new(PointHandle { doc: n.doc, slot, freed: freed_slots(), identity: None }));
    Ok(Value::Object(o))
}

fn point(realm: &mut Realm, this: &Value) -> JsResult<(rabbithole_js::ObjectId, u32, usize, BoundaryPoint)> {
    let o = handle_obj(realm, this)?;
    if let Some(shared) = realm.host_data::<SharedPoint>(o) {
        let position = shared.position();
        return position.map(|(doc,slot,p)| (o,doc,slot,p)).ok_or_else(||
            crate::misc::dom_exception(realm,"InvalidStateError","The range's document is unavailable."));
    }
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
            let mut old = std::mem::replace(h, PointHandle { doc: n.doc, slot: new_slot, freed, identity: None });
            old.slot = RELEASED;
        } else if let Some(shared) = realm.host_data::<SharedPoint>(o) {
            let mut owner = shared.0.borrow_mut();
            owner.auxiliary = crate::AuxiliaryDocument::lookup(n.doc);
            owner.frame = crate::env::with_env(|env| env.frame_id());
            owner.handle.slot = RELEASED;
            owner.handle = PointHandle { doc: n.doc, slot: new_slot, freed: freed_slots(), identity: Some(with_doc(n.doc, |d| d.weak_identity())) };
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

// ------------------------------------------------------------ selection

/// A handle object owning an existing live point slot of document `doc`.
fn adopt_point(realm: &mut Realm, doc: u32, slot: usize) -> Value {
    let o = realm.new_object_with_proto(Some(proto("__LiveBoundary")));
    realm.set_host_data(o, Box::new(PointHandle { doc, slot, freed: freed_slots(), identity: None }));
    Value::Object(o)
}

/// `selectionSync(document, knownGeneration)`: `null` when its selection
/// still has the range the caller knows, else `[generation]` (empty) or
/// `[generation, startHandle, endHandle, backward]`: handles owning the
/// selection's boundary points, for a new `Range` object.
fn selection_sync(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let doc = node_arg(realm, &arg(args, 0))?.doc;
    let known = realm.to_number(arg(args, 1))?;
    let generation = with_doc(doc, |d| d.selection_generation());
    if known == generation as f64 {
        return Ok(Value::Null);
    }
    purge();
    let (generation, sel) = with_doc(doc, |d| (d.selection_generation(), d.selection_slots()));
    let Some(sel) = sel else {
        return Ok(array(realm, vec![Value::Number(generation as f64)]));
    };
    let (start, end, generation) = if sel.owned {
        with_doc(doc, |d| d.disown_selection_slots());
        (sel.start, sel.end, generation)
    } else {
        // Slots of a Range this realm doesn't know: copy the points.
        with_doc(doc, |d| {
            let (a, b) = (d.live_point(sel.start), d.live_point(sel.end));
            let a = d.add_live_point(a.unwrap_or(BoundaryPoint { node: Document::ROOT, offset: 0 }));
            let b = d.add_live_point(b.unwrap_or(BoundaryPoint { node: Document::ROOT, offset: 0 }));
            let g = d.set_selection_slots(a, b, sel.backward);
            (a, b, g)
        })
    };
    let a = adopt_point(realm, doc, start);
    let b = adopt_point(realm, doc, end);
    Ok(array(realm, vec![Value::Number(generation as f64), a, b, Value::Bool(sel.backward)]))
}

/// `selectionSet(document, startHandle, endHandle, backward)`: make a
/// `Range`'s boundary points the document's selection (`null`: empty
/// selection). Returns the new generation.
fn selection_set(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let doc = node_arg(realm, &arg(args, 0))?.doc;
    let args = &args[1.min(args.len())..];
    let a = arg(args, 0);
    if matches!(a, Value::Null | Value::Undefined) {
        let generation = with_doc(doc, |d| {
            d.clear_selection();
            d.selection_generation()
        });
        return Ok(Value::Number(generation as f64));
    }
    let (_, da, sa, _) = point(realm, &a)?;
    let (_, db, sb, _) = point(realm, &arg(args, 1))?;
    let backward = realm.to_boolean(&arg(args, 2));
    if da != doc || db != doc {
        return Err(realm.throw_type_error("selection range of another document"));
    }
    let generation = with_doc(doc, |d| d.set_selection_slots(sa, sb, backward));
    Ok(Value::Number(generation as f64))
}

/// `selectionText()`: the selection as rendered text, or `null` when the
/// embedder has no layout.
fn selection_text(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if node_arg(realm, &arg(args, 0))?.doc != 0 {
        return Ok(Value::Null);
    }
    match crate::env::with_env(|env| env.selection_text()) {
        Some(t) => Ok(Value::str(&t)),
        None => Ok(Value::Null),
    }
}

/// `caretPositionFromPoint(x, y)`: `[textNode, offset]` nearest to a
/// viewport point, or `null`.
fn caret_position_from_point(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let x = realm.to_number(arg(args, 0))? as f32;
    let y = realm.to_number(arg(args, 1))? as f32;
    match crate::env::with_env(|env| env.caret_position(x, y)) {
        Some((node, offset)) => {
            let n = wrap(realm, NodeRef { doc: 0, node });
            Ok(array(realm, vec![n, Value::Number(offset as f64)]))
        }
        None => Ok(Value::Null),
    }
}

/// `setHighlights(flat)`: the registered custom highlights for painting,
/// flattened as `name, priority, count` followed by `count` ranges of five
/// values: `0, startHandle, _, endHandle, _` for a live `Range` or
/// `1, startNode, startOffset, endNode, endOffset` for a `StaticRange`.
fn set_highlights(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    use rabbithole_dom::{CustomHighlight, HighlightRange};
    let flat = arg(args, 0);
    let len_v = realm.get_str(flat.clone(), "length")?;
    let len = realm.to_number(len_v)?.max(0.0) as usize;
    let mut items = Vec::with_capacity(len);
    for i in 0..len {
        items.push(realm.get_str(flat.clone(), &i.to_string())?);
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 <= items.len() {
        let name = realm.to_rust_string(items[i].clone())?;
        let priority = realm.to_number(items[i + 1].clone())? as i32;
        let count = realm.to_number(items[i + 2].clone())? as usize;
        i += 3;
        let mut ranges = Vec::new();
        for _ in 0..count {
            if i + 5 > items.len() {
                break;
            }
            let kind = realm.to_number(items[i].clone())?;
            if kind == 0.0 {
                if let (Ok((_, 0, a, _)), Ok((_, 0, b, _))) = (point(realm, &items[i + 1]), point(realm, &items[i + 3])) {
                    ranges.push(HighlightRange::Live(a, b));
                }
            } else if let (Some(a), Some(b)) = (node_of(realm, &items[i + 1]), node_of(realm, &items[i + 3])) {
                if a.doc == 0 && b.doc == 0 {
                    let so = realm.to_number(items[i + 2].clone())? as usize;
                    let eo = realm.to_number(items[i + 4].clone())? as usize;
                    ranges.push(HighlightRange::Static(BoundaryPoint { node: a.node, offset: so }, BoundaryPoint { node: b.node, offset: eo }));
                }
            }
            i += 5;
        }
        out.push(CustomHighlight { name, priority, ranges });
    }
    with_doc(0, |d| d.set_custom_highlights(out));
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
    realm.define_method(natives, "selectionSync", 2, selection_sync);
    realm.define_method(natives, "selectionSet", 4, selection_set);
    realm.define_method(natives, "selectionText", 1, selection_text);
    realm.define_method(natives, "caretPositionFromPoint", 2, caret_position_from_point);
    realm.define_method(natives, "setHighlights", 1, set_highlights);
}
