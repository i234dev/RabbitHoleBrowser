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
    realm.set_class_name(o, Box::leak(class.to_string().into_boxed_str()));
    set_hidden(realm, o, "__callback", cb);
    realm.extra_roots.push(Value::Object(o));
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
    let Some(target) = node_of(realm, &arg(args, 0)) else {
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
    let st = state();
    let mut st = st.borrow_mut();
    st.observers.retain(|r| !(r.observer == o && r.target == target));
    st.observers.push(ObserverReg { observer: o, target, child_list, attributes, character_data, subtree, attribute_old_value, attribute_filter });
    Ok(Value::Undefined)
}

fn mo_disconnect(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let V::Object(o) = this.v() else {
        return Err(realm.throw_type_error("Illegal invocation"));
    };
    let st = state();
    let mut st = st.borrow_mut();
    st.observers.retain(|r| r.observer != o);
    st.mutation_records.retain(|r| r.observer != o);
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
    let recs: Vec<MutationRecord> = {
        let st = state();
        let mut st = st.borrow_mut();
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut st.mutation_records).into_iter().partition(|r| r.observer == o);
        st.mutation_records = rest;
        mine
    };
    Ok(records_to_js(realm, recs))
}

/// Deliver queued mutation records (called from `ScriptHost::run_jobs`).
pub(crate) fn deliver_mutations(realm: &mut Realm) {
    loop {
        let recs = std::mem::take(&mut state().borrow_mut().mutation_records);
        state().borrow_mut().mutation_scheduled = false;
        if recs.is_empty() {
            break;
        }
        // Group by observer, preserving order.
        let mut groups: Vec<(ObjectId, Vec<MutationRecord>)> = Vec::new();
        for r in recs {
            match groups.iter_mut().find(|(o, _)| *o == r.observer) {
                Some((_, v)) => v.push(r),
                None => groups.push((r.observer, vec![r])),
            }
        }
        for (observer, list) in groups {
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
    !state().borrow().mutation_records.is_empty()
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
