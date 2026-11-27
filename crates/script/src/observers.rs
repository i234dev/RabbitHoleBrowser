//! `MutationObserver`, `IntersectionObserver`, `ResizeObserver`,
//! `PerformanceObserver` (stub).

use crate::host::*;
use rabbithole_js::V;
use crate::node;
use rabbithole_js::{JsResult, ObjectId, Realm, Value, ALL, CONFIGURABLE, ENUMERABLE, HIDDEN};

const ACC: u8 = CONFIGURABLE | ENUMERABLE;

fn callback_of(realm: &mut Realm, args: &[Value]) -> JsResult<Value> {
    let cb = arg(args, 0);
    if !realm.is_callable(&cb) {
        return Err(realm.throw_type_error("parameter 1 is not of type 'Function'."));
    }
    Ok(cb)
}

fn new_observer(realm: &mut Realm, class: &str, cb: Value) -> ObjectId {
    let p = realm.proto_from_new_target(proto(class));
    let o = realm.new_object_with_proto(Some(p));
    realm.set_class_name(o, class);
    set_hidden(realm, o, "__callback", cb);
    o
}

// ---------------------------------------------------------- MutationObserver

fn mo_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let cb = callback_of(realm, args)?;
    Ok(Value::Object(new_observer(realm, "MutationObserver", cb)))
}

fn mo_observe(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let V::Object(o) = this.v() else {
        return Err(realm.throw_type_error("Illegal invocation"));
    };
    let wrapper = host_obj(realm, &arg(args, 0));
    let Some((target_object, target)) = wrapper.and_then(|id| realm.host_data::<NodeRef>(id).map(|&node| (id, node))) else {
        return Err(realm.throw_type_error("parameter 1 is not of type 'Node'."));
    };
    let opts = arg(args, 1);
    let child_list = dict_bool(realm, &opts, "childList")?.unwrap_or(false);
    let attribute_old_value = dict_bool(realm, &opts, "attributeOldValue")?.unwrap_or(false);
    let attr_filter = dict_value(realm, &opts, "attributeFilter")?;
    let attributes = dict_bool(realm, &opts, "attributes")?.unwrap_or(attribute_old_value || !attr_filter.is_undefined());
    let character_data = dict_bool(realm, &opts, "characterData")?.unwrap_or(dict_bool(realm, &opts, "characterDataOldValue")?.unwrap_or(false));
    let subtree = dict_bool(realm, &opts, "subtree")?.unwrap_or(false);
    let attribute_filter = if attr_filter.is_nullish() {
        None
    } else {
        let list = realm.create_list_from_array_like(attr_filter)?;
        let mut names = Vec::with_capacity(list.len());
        for v in list {
            names.push(realm.to_rust_string(v)?);
        }
        Some(names)
    };
    if !(child_list || attributes || character_data) {
        return Err(realm.throw_type_error("The options object must set at least one of 'attributes', 'characterData', or 'childList' to true."));
    }
    let reg = ObserverReg { observer: o, target, target_object, transient_targets: Vec::new(), child_list, attributes, character_data, subtree, attribute_old_value, attribute_filter };
    let st = state();
    let mut st = st.borrow_mut();
    if let Some(old) = st.observers.iter_mut().find(|r| r.observer == o && r.target == target) {
        *old = ObserverReg { target_object: old.target_object, ..reg };
    } else {
        st.observers.push(reg);
        update_node_links(realm, &mut st, target_object);
    }
    Ok(Value::Undefined)
}

/// The node owns its observers. A private property keeps this edge visible
/// to the collector without making the observer a permanent realm root.
fn update_node_links(realm: &mut Realm, st: &mut HostState, target: ObjectId) {
    let key = *st.observer_links.get_or_insert_with(|| realm.new_private_symbol(rabbithole_js::JsStr::new("node.observers")));
    let mut observers = st.observers.iter().filter(|r| r.target_object == target).map(|r| Value::Object(r.observer));
    let links = match (observers.next(), observers.next()) {
        (None, _) => Value::Null,
        (Some(one), None) => one,
        (Some(one), Some(two)) => realm.new_array_value([one, two].into_iter().chain(observers).collect()),
    };
    realm.define_property(target, key, links, 0);
}

fn mo_disconnect(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let V::Object(o) = this.v() else {
        return Err(realm.throw_type_error("Illegal invocation"));
    };
    let st = state();
    let mut st = st.borrow_mut();
    let mut targets = Vec::new();
    st.observers.retain(|reg| {
        if reg.observer != o { return true; }
        targets.push(reg.target_object);
        false
    });
    for target in targets {
        update_node_links(realm, &mut st, target);
    }
    st.mutation_records.remove(&o);
    st.pending_observers.retain(|&id| id != o);
    Ok(Value::Undefined)
}

fn records_to_js(realm: &mut Realm, recs: Vec<MutationRecord>) -> Value {
    let mut out = Vec::new();
    for r in recs {
        let target = wrap(realm, r.target);
        let added = node::node_list(realm, r.added.clone());
        let removed = node::node_list(realm, r.removed.clone());
        let rec = object_from(
            realm,
            vec![
                ("type", Value::str(r.ty)),
                ("target", target),
                ("addedNodes", added),
                ("removedNodes", removed),
                ("attributeName", r.attribute_name.as_deref().map(Value::str).unwrap_or(Value::Null)),
                ("attributeNamespace", Value::Null),
                ("oldValue", r.old_value.as_deref().map(Value::str).unwrap_or(Value::Null)),
                ("previousSibling", Value::Null),
                ("nextSibling", Value::Null),
            ],
        );
        if let V::Object(id) = rec.v() {
            let p = proto("MutationRecord");
            realm.obj_mut(id).set_proto(Some(p));
        }
        out.push(rec);
    }
    realm.new_array_value(out)
}

fn mo_take_records(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let V::Object(o) = this.v() else {
        return Err(realm.throw_type_error("Illegal invocation"));
    };
    let recs = state().borrow_mut().mutation_records.remove(&o).unwrap_or_default();
    Ok(records_to_js(realm, recs))
}

/// Deliver queued mutation records (called from `ScriptHost::run_jobs`).
pub(crate) fn deliver_mutations(realm: &mut Realm) {
    loop {
        let pending = std::mem::take(&mut state().borrow_mut().pending_observers);
        if pending.is_empty() { break; }
        // A callback may disconnect a later observer, then run the GC.
        let notify: Vec<_> = pending.into_iter().map(|id| realm.root_object(id)).collect();
        for root in notify {
            let observer = root.object().expect("rooted observer");
            let list = {
                let st = state();
                let mut st = st.borrow_mut();
                for reg in st.observers.iter_mut().filter(|r| r.observer == observer) {
                    reg.transient_targets.clear();
                }
                st.mutation_records.remove(&observer).unwrap_or_default()
            };
            if list.is_empty() { continue; }
            let cb = get_hidden(realm, observer, "__callback");
            if !realm.is_callable(&cb) {
                continue;
            }
            let arr = records_to_js(realm, list);
            call_logged(realm, &cb, Value::Object(observer), &[arr, Value::Object(observer)], "MutationObserver callback");
        }
    }
}

pub(crate) fn has_pending_mutations() -> bool {
    !state().borrow().pending_observers.is_empty()
}

// ------------------------------------------------------ PerformanceObserver

fn po_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let cb = callback_of(realm, args)?;
    Ok(Value::Object(new_observer(realm, "PerformanceObserver", cb)))
}

fn po_take_records(realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(realm.new_array_value(vec![]))
}

fn po_noop(_realm: &mut Realm, _this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::Undefined)
}

pub(crate) fn install(realm: &mut Realm) {
    let mo = define_class(realm, "MutationObserver", None, mo_ctor);
    realm.define_method(mo, "observe", 2, mo_observe);
    realm.define_method(mo, "disconnect", 0, mo_disconnect);
    realm.define_method(mo, "takeRecords", 0, mo_take_records);
    define_class(realm, "MutationRecord", None, illegal_ctor);
    let po = define_class(realm, "PerformanceObserver", None, po_ctor);
    realm.define_method(po, "observe", 1, po_noop);
    realm.define_method(po, "disconnect", 0, po_noop);
    realm.define_method(po, "takeRecords", 0, po_take_records);
    let po_ctor_v = realm.obj(po).props.get(&rabbithole_js::PropertyKey::str("constructor")).map(|p| p.value.clone());
    if let Some(V::Object(c)) = po_ctor_v.as_ref().map(Value::v) {
        let types = realm.new_array_value(vec![]);
        realm.define_property(c, "supportedEntryTypes", types, ACC);
    }
    let _ = (ALL, HIDDEN);
}
