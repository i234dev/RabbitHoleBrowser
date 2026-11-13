//! Property descriptors with absent fields, the ordinary and array
//! [[DefineOwnProperty]] (ValidateAndApplyPropertyDescriptor,
//! ArraySetLength), and the Proxy internal methods with their invariant
//! checks (ECMA-262 §10.1, §10.4.2, §10.5).

use crate::heap::*;
use crate::interop::JsResult;
use crate::realm::*;
use crate::value::V;
use crate::value::*;

/// A Property Descriptor whose fields may each be absent.
#[derive(Clone, Default, Debug)]
pub struct PropDesc {
    pub value: Option<Value>,
    pub writable: Option<bool>,
    pub get: Option<Value>,
    pub set: Option<Value>,
    pub enumerable: Option<bool>,
    pub configurable: Option<bool>,
}

impl PropDesc {
    pub fn is_accessor(&self) -> bool {
        self.get.is_some() || self.set.is_some()
    }
    pub fn is_data(&self) -> bool {
        self.value.is_some() || self.writable.is_some()
    }
    pub fn is_generic(&self) -> bool {
        !self.is_accessor() && !self.is_data()
    }
    pub fn is_empty(&self) -> bool {
        self.is_generic() && self.enumerable.is_none() && self.configurable.is_none()
    }
    /// `{[[Value]]: v}` only.
    pub fn value_only(v: Value) -> PropDesc {
        PropDesc { value: Some(v), ..Default::default() }
    }
    /// A complete descriptor equal to an existing property.
    pub fn from_property(p: &Property) -> PropDesc {
        if p.is_accessor() {
            PropDesc {
                get: Some(p.getter_cloned().unwrap_or(Value::Undefined)),
                set: Some(p.setter_cloned().unwrap_or(Value::Undefined)),
                enumerable: Some(p.enumerable()),
                configurable: Some(p.configurable()),
                ..Default::default()
            }
        } else {
            PropDesc {
                value: Some(p.value.clone()),
                writable: Some(p.writable()),
                enumerable: Some(p.enumerable()),
                configurable: Some(p.configurable()),
                ..Default::default()
            }
        }
    }
    /// The property a new definition creates (absent fields default).
    pub fn to_new_property(&self) -> Property {
        let mut flags = 0;
        if self.enumerable == Some(true) {
            flags |= ENUMERABLE;
        }
        if self.configurable == Some(true) {
            flags |= CONFIGURABLE;
        }
        if self.is_accessor() {
            let g = self.get.clone().filter(|g| !g.is_undefined());
            let s = self.set.clone().filter(|s| !s.is_undefined());
            Property::accessor(g, s, flags)
        } else {
            if self.writable == Some(true) {
                flags |= WRITABLE;
            }
            Property::data(self.value.clone().unwrap_or(Value::Undefined), flags)
        }
    }
    /// `current` updated with the fields present here (the last step of
    /// ValidateAndApplyPropertyDescriptor).
    pub fn apply_to(&self, current: &Property) -> Property {
        let enumerable = self.enumerable.unwrap_or(current.enumerable());
        let configurable = self.configurable.unwrap_or(current.configurable());
        let mut flags = 0;
        if enumerable {
            flags |= ENUMERABLE;
        }
        if configurable {
            flags |= CONFIGURABLE;
        }
        if self.is_accessor() || (current.is_accessor() && !self.is_data()) {
            let (g, s) = if current.is_accessor() { (current.getter_cloned(), current.setter_cloned()) } else { (None, None) };
            let g = match &self.get {
                Some(v) => Some(v.clone()).filter(|v| !v.is_undefined()),
                None => g,
            };
            let s = match &self.set {
                Some(v) => Some(v.clone()).filter(|v| !v.is_undefined()),
                None => s,
            };
            Property::accessor(g, s, flags)
        } else {
            let (v, w) = if current.is_accessor() { (Value::Undefined, false) } else { (current.value.clone(), current.writable()) };
            if self.writable.unwrap_or(w) {
                flags |= WRITABLE;
            }
            Property::data(self.value.clone().unwrap_or(v), flags)
        }
    }
}

impl Realm {
    /// ToPropertyDescriptor.
    pub fn to_prop_desc(&mut self, v: Value) -> JsResult<PropDesc> {
        let id = match v.v() {
            V::Object(id) => id,
            _ => return self.type_error("Property description must be an object"),
        };
        let mut d = PropDesc::default();
        let o = Value::Object(id);
        if self.has_property(id, &PropertyKey::str("enumerable"))? {
            d.enumerable = Some(self.get_str(o.clone(), "enumerable")?.truthy());
        }
        if self.has_property(id, &PropertyKey::str("configurable"))? {
            d.configurable = Some(self.get_str(o.clone(), "configurable")?.truthy());
        }
        if self.has_property(id, &crate::value::names::value())? {
            d.value = Some(self.get(o.clone(), crate::value::names::value())?);
        }
        if self.has_property(id, &PropertyKey::str("writable"))? {
            d.writable = Some(self.get_str(o.clone(), "writable")?.truthy());
        }
        if self.has_property(id, &PropertyKey::str("get"))? {
            let g = self.get_str(o.clone(), "get")?;
            if !g.is_undefined() && !self.is_callable(&g) {
                return self.type_error("Getter must be a function");
            }
            d.get = Some(g);
        }
        if self.has_property(id, &PropertyKey::str("set"))? {
            let s = self.get_str(o, "set")?;
            if !s.is_undefined() && !self.is_callable(&s) {
                return self.type_error("Setter must be a function");
            }
            d.set = Some(s);
        }
        if d.is_accessor() && d.is_data() {
            return self.type_error("Invalid property descriptor. Cannot both specify accessors and a value or writable attribute");
        }
        Ok(d)
    }

    /// FromPropertyDescriptor (only the present fields).
    pub fn from_prop_desc(&mut self, d: &PropDesc) -> Value {
        let o = self.new_object();
        if let Some(v) = &d.value {
            self.define_property(o, "value", v.clone(), ALL);
        }
        if let Some(w) = d.writable {
            self.define_property(o, "writable", Value::Bool(w), ALL);
        }
        if let Some(g) = &d.get {
            self.define_property(o, "get", g.clone(), ALL);
        }
        if let Some(s) = &d.set {
            self.define_property(o, "set", s.clone(), ALL);
        }
        if let Some(e) = d.enumerable {
            self.define_property(o, "enumerable", Value::Bool(e), ALL);
        }
        if let Some(c) = d.configurable {
            self.define_property(o, "configurable", Value::Bool(c), ALL);
        }
        Value::Object(o)
    }

    /// ValidateAndApplyPropertyDescriptor without the apply step
    /// (IsCompatiblePropertyDescriptor when `current` is given).
    pub fn desc_is_compatible(&self, extensible: bool, desc: &PropDesc, current: Option<&Property>) -> bool {
        let Some(cur) = current else { return extensible };
        if desc.is_empty() || cur.configurable() {
            return true;
        }
        if desc.configurable == Some(true) {
            return false;
        }
        if desc.enumerable.is_some_and(|e| e != cur.enumerable()) {
            return false;
        }
        if !desc.is_generic() && desc.is_accessor() != cur.is_accessor() {
            return false;
        }
        if cur.is_accessor() {
            let g = cur.getter_cloned().unwrap_or(Value::Undefined);
            let s = cur.setter_cloned().unwrap_or(Value::Undefined);
            if desc.get.as_ref().is_some_and(|d| !self.same_value(d, &g)) || desc.set.as_ref().is_some_and(|d| !self.same_value(d, &s)) {
                return false;
            }
        } else if !cur.writable() {
            if desc.writable == Some(true) {
                return false;
            }
            if desc.value.as_ref().is_some_and(|v| !self.same_value(v, &cur.value)) {
                return false;
            }
        }
        true
    }

    /// [[DefineOwnProperty]] with a possibly partial descriptor, for every
    /// kind of object.
    pub fn define_own_desc(&mut self, id: ObjectId, key: PropertyKey, desc: PropDesc) -> JsResult<bool> {
        match &self.obj(id).kind {
            ObjectKind::Proxy { .. } => return self.proxy_define_desc(id, key, desc),
            ObjectKind::ModuleNamespace => return self.namespace_define(id, key, desc),
            ObjectKind::MappedArguments(_) => {
                if let Some(r) = self.mapped_args_define(id, key.clone(), desc.clone())? {
                    return Ok(r);
                }
            }
            ObjectKind::Array { .. } => {
                if let PropertyKey::String(s) = &key {
                    if s.eq_str("length") {
                        return self.array_set_length(id, desc);
                    }
                }
                if let Some(i) = key.as_array_index() {
                    let (len, len_writable) = self.array_length_state(id);
                    if i >= len && !len_writable {
                        return Ok(false);
                    }
                    if !self.ordinary_define_own(id, key, desc)? {
                        return Ok(false);
                    }
                    if i >= len {
                        if let ObjectKind::Array { length, .. } = &mut self.obj_mut(id).kind {
                            *length = i + 1;
                        }
                    }
                    return Ok(true);
                }
            }
            ObjectKind::TypedArray(t) => {
                if let Some(n) = crate::builtins::typed_array::canonical_numeric_index(&key) {
                    let t = **t;
                    if crate::builtins::typed_array::ta_valid_index(self, &t, n).is_none() {
                        return Ok(false);
                    }
                    if crate::builtins::typed_array::ta_immutable(self, &t) {
                        // Elements of an immutable buffer are non-writable and
                        // non-configurable: only a compatible no-op succeeds.
                        let cur = self.get_own_property(id, &key);
                        return Ok(self.desc_is_compatible(false, &desc, cur.as_ref()));
                    }
                    if desc.configurable == Some(false) || desc.enumerable == Some(false) || desc.is_accessor() || desc.writable == Some(false) {
                        return Ok(false);
                    }
                    if let Some(v) = desc.value {
                        crate::builtins::typed_array::ta_set_element(self, &t, n, v)?;
                    }
                    return Ok(true);
                }
            }
            _ => {}
        }
        self.ordinary_define_own(id, key, desc)
    }

    pub(crate) fn ordinary_define_own_pub(&mut self, id: ObjectId, key: PropertyKey, desc: PropDesc) -> JsResult<bool> {
        self.ordinary_define_own(id, key, desc)
    }

    /// OrdinaryDefineOwnProperty on a non-proxy object.
    fn ordinary_define_own(&mut self, id: ObjectId, key: PropertyKey, desc: PropDesc) -> JsResult<bool> {
        let current = self.get_own_property(id, &key);
        let extensible = self.obj(id).extensible;
        if !self.desc_is_compatible(extensible, &desc, current.as_ref()) {
            return Ok(false);
        }
        let prop = match &current {
            None => desc.to_new_property(),
            Some(_) if desc.is_empty() => return Ok(true),
            Some(cur) => desc.apply_to(cur),
        };
        self.store_property(id, key, prop);
        Ok(true)
    }

    /// The array's length and whether it is writable.
    pub(crate) fn array_length_state(&self, id: ObjectId) -> (u32, bool) {
        let o = self.obj(id);
        let len = match &o.kind {
            ObjectKind::Array { length, .. } => *length,
            _ => 0,
        };
        (len, o.props.get(&crate::value::names::length()).is_none_or(|p| p.writable()))
    }

    /// Store a validated property (array elements stay in `elements` when
    /// they are plain data properties).
    pub(crate) fn store_property(&mut self, id: ObjectId, key: PropertyKey, prop: Property) {
        let obj = self.obj_mut(id);
        if let ObjectKind::Array { elements, length } = &mut obj.kind {
            if let Some(i) = key.as_array_index() {
                let i_us = i as usize;
                if !prop.is_accessor() && prop.flags & ALL == ALL {
                    if i_us < elements.len() {
                        elements[i_us] = prop.value;
                        obj.props.remove(&key);
                        return;
                    } else if i_us == elements.len() && !obj.props.contains(&key) {
                        elements.push(prop.value);
                        if *length <= i {
                            *length = i + 1;
                        }
                        return;
                    }
                }
                if i_us < elements.len() {
                    // Kept in the property map: the element becomes a hole.
                    elements[i_us] = Value::Empty;
                }
                if *length <= i {
                    *length = i + 1;
                }
            }
        }
        obj.props.insert(key, prop);
    }

    /// ArraySetLength(A, Desc).
    pub fn array_set_length(&mut self, id: ObjectId, desc: PropDesc) -> JsResult<bool> {
        let key = crate::value::names::length();
        let Some(v) = desc.value.clone() else {
            return self.ordinary_define_own(id, key, desc);
        };
        let new_len = self.to_uint32(v.clone())?;
        let number_len = self.to_number(v)?;
        if new_len as f64 != number_len {
            return self.range_error("Invalid array length");
        }
        let (old_len, old_writable) = self.array_length_state(id);
        let mut new_desc = desc;
        new_desc.value = Some(Value::Number(new_len as f64));
        let current = Property::data(Value::Number(old_len as f64), if old_writable { WRITABLE } else { 0 });
        if !self.desc_is_compatible(false, &new_desc, Some(&current)) {
            return Ok(false);
        }
        if new_len >= old_len {
            if let ObjectKind::Array { length, .. } = &mut self.obj_mut(id).kind {
                *length = new_len;
            }
            if let Some(w) = new_desc.writable {
                self.obj_mut(id).props.insert(key, Property::data(Value::Undefined, if w { WRITABLE } else { 0 }));
            }
            return Ok(true);
        }
        if !old_writable {
            return Ok(false);
        }
        let new_writable = new_desc.writable != Some(false);
        // Delete the elements from the end; a non-configurable one stops it.
        let mut stopped_at: Option<u32> = None;
        {
            let obj = self.obj_mut(id);
            let mut sparse: Vec<(u32, bool)> = obj.props.iter().filter_map(|(k, p)| k.as_array_index().filter(|&i| i >= new_len).map(|i| (i, p.configurable()))).collect();
            sparse.sort_unstable_by(|a, b| b.0.cmp(&a.0));
            for (i, configurable) in sparse {
                if !configurable {
                    stopped_at = Some(i);
                    break;
                }
                obj.props.remove(&PropertyKey::index(i));
            }
            let keep = stopped_at.map_or(new_len, |i| i + 1);
            if let ObjectKind::Array { elements, length } = &mut obj.kind {
                if (keep as usize) < elements.len() {
                    elements.truncate(keep as usize);
                }
                *length = keep;
            }
        }
        if !new_writable || stopped_at.is_some() {
            let w = new_writable && stopped_at.is_none();
            if !new_writable {
                self.obj_mut(id).props.insert(key, Property::data(Value::Undefined, if w { WRITABLE } else { 0 }));
            }
        }
        Ok(stopped_at.is_none())
    }

    // ------------------------------------------------------ generic ops

    /// IsExtensible(O).
    pub fn is_extensible_any(&mut self, id: ObjectId) -> JsResult<bool> {
        if let ObjectKind::Proxy { .. } = &self.obj(id).kind {
            let (target, handler, trap) = self.proxy_parts(id, "isExtensible")?;
            let Some(trap) = trap else { return self.is_extensible_any(target) };
            let r = self.call(trap, Value::Object(handler), &[Value::Object(target)])?.truthy();
            if r != self.is_extensible_any(target)? {
                return self.type_error("'isExtensible' on proxy: trap result does not reflect extensibility of proxy target");
            }
            return Ok(r);
        }
        Ok(self.obj(id).extensible)
    }

    /// O.[[PreventExtensions]]().
    pub fn prevent_extensions_any(&mut self, id: ObjectId) -> JsResult<bool> {
        if let ObjectKind::Proxy { .. } = &self.obj(id).kind {
            let (target, handler, trap) = self.proxy_parts(id, "preventExtensions")?;
            let Some(trap) = trap else { return self.prevent_extensions_any(target) };
            let r = self.call(trap, Value::Object(handler), &[Value::Object(target)])?.truthy();
            if r && self.is_extensible_any(target)? {
                return self.type_error("'preventExtensions' on proxy: trap returned truish but the proxy target is extensible");
            }
            return Ok(r);
        }
        self.obj_mut(id).extensible = false;
        Ok(true)
    }

    /// O.[[SetPrototypeOf]](V).
    pub fn set_prototype_of_any(&mut self, id: ObjectId, proto: Option<ObjectId>) -> JsResult<bool> {
        if let ObjectKind::Proxy { .. } = &self.obj(id).kind {
            let (target, handler, trap) = self.proxy_parts(id, "setPrototypeOf")?;
            let Some(trap) = trap else { return self.set_prototype_of_any(target, proto) };
            let pv = proto.map_or(Value::Null, Value::Object);
            if !self.call(trap, Value::Object(handler), &[Value::Object(target), pv])?.truthy() {
                return Ok(false);
            }
            if self.is_extensible_any(target)? {
                return Ok(true);
            }
            if self.get_prototype_of(target)? != proto {
                return self.type_error("'setPrototypeOf' on proxy: trap returned truish for setting a new prototype on the non-extensible proxy target");
            }
            return Ok(true);
        }
        // OrdinarySetPrototypeOf.
        if self.obj(id).proto == proto {
            return Ok(true);
        }
        if !self.obj(id).extensible {
            return Ok(false);
        }
        let mut p = proto;
        while let Some(c) = p {
            if c == id {
                return Ok(false);
            }
            if let ObjectKind::Proxy { .. } = &self.obj(c).kind {
                break;
            }
            p = self.obj(c).proto;
        }
        self.obj_mut(id).set_proto(proto);
        Ok(true)
    }

    /// DefinePropertyOrThrow.
    pub fn define_property_or_throw(&mut self, id: ObjectId, key: PropertyKey, desc: PropDesc) -> JsResult<()> {
        if !self.define_own_desc(id, key.clone(), desc)? {
            let k = self.key_display(&key);
            return self.type_error(&format!("Cannot redefine property: {}", k));
        }
        Ok(())
    }

    // ---------------------------------------------------------- proxies

    pub(crate) fn proxy_parts(&mut self, id: ObjectId, trap: &str) -> JsResult<(ObjectId, ObjectId, Option<Value>)> {
        let (target, handler) = match &self.obj(id).kind {
            ObjectKind::Proxy { target, handler, revoked } => {
                if *revoked {
                    return self.type_error(&format!("Cannot perform '{}' on a proxy that has been revoked", trap));
                }
                (*target, *handler)
            }
            _ => unreachable!(),
        };
        let t = self.get_method(Value::Object(handler), PropertyKey::str(trap))?;
        Ok((target, handler, t))
    }

    pub(crate) fn proxy_get_prototype_of(&mut self, id: ObjectId) -> JsResult<Option<ObjectId>> {
        let (target, handler, trap) = self.proxy_parts(id, "getPrototypeOf")?;
        let Some(trap) = trap else { return self.get_prototype_of(target) };
        let r = self.call(trap, Value::Object(handler), &[Value::Object(target)])?;
        let proto = match r.v() {
            V::Object(p) => Some(p),
            V::Null => None,
            _ => return self.type_error("'getPrototypeOf' on proxy: trap returned neither object nor null"),
        };
        if self.is_extensible_any(target)? {
            return Ok(proto);
        }
        if self.get_prototype_of(target)? != proto {
            return self.type_error("'getPrototypeOf' on proxy: proxy target is non-extensible but the trap did not return its actual prototype");
        }
        Ok(proto)
    }

    pub(crate) fn proxy_get(&mut self, id: ObjectId, key: &PropertyKey, receiver: Value) -> JsResult<Value> {
        let (target, handler, trap) = self.proxy_parts(id, "get")?;
        let Some(trap) = trap else { return self.get_from_object(target, key, receiver) };
        let v = self.call(trap, Value::Object(handler), &[Value::Object(target), key.to_value(), receiver])?;
        if let Some(td) = self.get_own_property_any(target, key)? {
            if !td.configurable() {
                if !td.is_accessor() && !td.writable() && !self.same_value(&v, &td.value) {
                    return self.type_error("'get' on proxy: property is a read-only and non-configurable data property on the proxy target but the proxy did not return its actual value");
                }
                if td.is_accessor() && td.getter().is_none() && !v.is_undefined() {
                    return self.type_error("'get' on proxy: property is a non-configurable accessor property on the proxy target and does not have a getter function, but the trap did not return 'undefined'");
                }
            }
        }
        Ok(v)
    }

    pub(crate) fn proxy_set(&mut self, id: ObjectId, key: PropertyKey, value: Value, receiver: Value) -> JsResult<bool> {
        let (target, handler, trap) = self.proxy_parts(id, "set")?;
        let Some(trap) = trap else { return self.set_on_object(target, key, value, receiver) };
        if !self.call(trap, Value::Object(handler), &[Value::Object(target), key.to_value(), value.clone(), receiver])?.truthy() {
            return Ok(false);
        }
        if let Some(td) = self.get_own_property_any(target, &key)? {
            if !td.configurable() {
                if !td.is_accessor() && !td.writable() && !self.same_value(&value, &td.value) {
                    return self.type_error("'set' on proxy: trap returned truish for property which exists in the proxy target as a non-configurable and non-writable data property with a different value");
                }
                if td.is_accessor() && td.setter().is_none() {
                    return self.type_error("'set' on proxy: trap returned truish for property which exists in the proxy target as a non-configurable and non-writable accessor property without a setter");
                }
            }
        }
        Ok(true)
    }

    pub(crate) fn proxy_has(&mut self, id: ObjectId, key: &PropertyKey) -> JsResult<bool> {
        let (target, handler, trap) = self.proxy_parts(id, "has")?;
        let Some(trap) = trap else { return self.has_property(target, key) };
        let r = self.call(trap, Value::Object(handler), &[Value::Object(target), key.to_value()])?.truthy();
        if !r {
            if let Some(td) = self.get_own_property_any(target, key)? {
                if !td.configurable() {
                    return self.type_error("'has' on proxy: trap returned falsish for property which exists in the proxy target as non-configurable");
                }
                if !self.is_extensible_any(target)? {
                    return self.type_error("'has' on proxy: trap returned falsish for property but the proxy target is not extensible");
                }
            }
        }
        Ok(r)
    }

    pub(crate) fn proxy_delete(&mut self, id: ObjectId, key: &PropertyKey) -> JsResult<bool> {
        let (target, handler, trap) = self.proxy_parts(id, "deleteProperty")?;
        let Some(trap) = trap else { return self.delete_property(target, key) };
        if !self.call(trap, Value::Object(handler), &[Value::Object(target), key.to_value()])?.truthy() {
            return Ok(false);
        }
        if let Some(td) = self.get_own_property_any(target, key)? {
            if !td.configurable() {
                return self.type_error("'deleteProperty' on proxy: trap returned truish for property which is non-configurable in the proxy target");
            }
            if !self.is_extensible_any(target)? {
                return self.type_error("'deleteProperty' on proxy: trap returned truish for property but the proxy target is non-extensible");
            }
        }
        Ok(true)
    }

    pub(crate) fn proxy_own_keys(&mut self, id: ObjectId) -> JsResult<Vec<PropertyKey>> {
        let (target, handler, trap) = self.proxy_parts(id, "ownKeys")?;
        let Some(trap) = trap else { return self.own_keys(target) };
        let r = self.call(trap, Value::Object(handler), &[Value::Object(target)])?;
        let Some(rid) = r.as_object() else { return self.type_error("CreateListFromArrayLike called on non-object") };
        let len = self.length_of(Value::Object(rid))?;
        if len > crate::interop::MAX_LIST_LENGTH {
            return self.range_error("Too many elements in array-like object");
        }
        let mut keys: Vec<PropertyKey> = Vec::with_capacity(len.min(1 << 16));
        for i in 0..len {
            let v = self.get(Value::Object(rid), PropertyKey::index(i as u32))?;
            let k = match v.v() {
                V::String(s) => PropertyKey::String(s.clone()),
                V::Symbol(s) => PropertyKey::Symbol(s),
                _ => return self.type_error("'ownKeys' on proxy: trap result elements must be strings or symbols"),
            };
            if keys.contains(&k) {
                let d = self.key_display(&k);
                return self.type_error(&format!("'ownKeys' on proxy: trap returned duplicate entries ('{}')", d));
            }
            keys.push(k);
        }
        let extensible = self.is_extensible_any(target)?;
        let target_keys = self.own_keys(target)?;
        let mut configurable = Vec::new();
        let mut nonconfigurable = Vec::new();
        for k in target_keys {
            match self.get_own_property_any(target, &k)? {
                Some(p) if !p.configurable() => nonconfigurable.push(k),
                _ => configurable.push(k),
            }
        }
        if extensible && nonconfigurable.is_empty() {
            return Ok(keys);
        }
        let mut unchecked: Vec<Option<&PropertyKey>> = keys.iter().map(Some).collect();
        let mut take = |k: &PropertyKey| -> bool {
            for slot in unchecked.iter_mut() {
                if *slot == Some(k) {
                    *slot = None;
                    return true;
                }
            }
            false
        };
        for k in &nonconfigurable {
            if !take(k) {
                let d = self.key_display(k);
                return self.type_error(&format!("'ownKeys' on proxy: trap result did not include '{}'", d));
            }
        }
        if extensible {
            return Ok(keys);
        }
        for k in &configurable {
            if !take(k) {
                let d = self.key_display(k);
                return self.type_error(&format!("'ownKeys' on proxy: trap result did not include '{}'", d));
            }
        }
        if unchecked.iter().any(|s| s.is_some()) {
            return self.type_error("'ownKeys' on proxy: trap returned extra keys but proxy target is non-extensible");
        }
        Ok(keys)
    }

    pub fn proxy_get_own_property(&mut self, id: ObjectId, key: &PropertyKey) -> JsResult<Option<Property>> {
        let (target, handler, trap) = self.proxy_parts(id, "getOwnPropertyDescriptor")?;
        let Some(trap) = trap else { return self.get_own_property_any(target, key) };
        let r = self.call(trap, Value::Object(handler), &[Value::Object(target), key.to_value()])?;
        if !r.is_object() && !r.is_undefined() {
            return self.type_error("'getOwnPropertyDescriptor' on proxy: trap returned neither object nor undefined");
        }
        let target_desc = self.get_own_property_any(target, key)?;
        if r.is_undefined() {
            let Some(td) = target_desc else { return Ok(None) };
            if !td.configurable() {
                return self.type_error("'getOwnPropertyDescriptor' on proxy: trap returned undefined for property which is non-configurable in the proxy target");
            }
            if !self.is_extensible_any(target)? {
                return self.type_error("'getOwnPropertyDescriptor' on proxy: trap returned undefined for property which exists in the non-extensible proxy target");
            }
            return Ok(None);
        }
        let extensible = self.is_extensible_any(target)?;
        let rd = self.to_prop_desc(r)?;
        let result = rd.to_new_property();
        if !self.desc_is_compatible(extensible, &rd, target_desc.as_ref()) {
            return self.type_error("'getOwnPropertyDescriptor' on proxy: trap returned descriptor for property that is incompatible with the existing property in the proxy target");
        }
        if !result.configurable() {
            match &target_desc {
                Some(td) if !td.configurable() => {
                    if rd.writable == Some(false) && td.writable() && !td.is_accessor() {
                        return self.type_error("'getOwnPropertyDescriptor' on proxy: trap reported non-configurable and writable for property which is non-configurable, non-writable in the proxy target");
                    }
                }
                _ => return self.type_error("'getOwnPropertyDescriptor' on proxy: trap reported non-configurability for property which is either non-existent or configurable in the proxy target"),
            }
        }
        Ok(Some(result))
    }

    pub(crate) fn proxy_define_desc(&mut self, id: ObjectId, key: PropertyKey, desc: PropDesc) -> JsResult<bool> {
        let (target, handler, trap) = self.proxy_parts(id, "defineProperty")?;
        let Some(trap) = trap else { return self.define_own_desc(target, key, desc) };
        let desc_obj = self.from_prop_desc(&desc);
        if !self.call(trap, Value::Object(handler), &[Value::Object(target), key.to_value(), desc_obj])?.truthy() {
            return Ok(false);
        }
        let target_desc = self.get_own_property_any(target, &key)?;
        let extensible = self.is_extensible_any(target)?;
        let setting_config_false = desc.configurable == Some(false);
        match &target_desc {
            None => {
                if !extensible {
                    return self.type_error("'defineProperty' on proxy: trap returned truish for adding property to the non-extensible proxy target");
                }
                if setting_config_false {
                    return self.type_error("'defineProperty' on proxy: trap returned truish for defining non-configurable property which is either non-existent or configurable in the proxy target");
                }
            }
            Some(td) => {
                if !self.desc_is_compatible(extensible, &desc, Some(td)) {
                    return self.type_error("'defineProperty' on proxy: trap returned truish for adding property that is incompatible with the existing property in the proxy target");
                }
                if setting_config_false && td.configurable() {
                    return self.type_error("'defineProperty' on proxy: trap returned truish for defining non-configurable property which is either non-existent or configurable in the proxy target");
                }
                if !td.is_accessor() && !td.configurable() && td.writable() && desc.writable == Some(false) {
                    return self.type_error("'defineProperty' on proxy: trap returned truish for defining non-configurable property which cannot be non-writable, unless there exists a corresponding non-configurable, non-writable own property of the target object");
                }
            }
        }
        Ok(true)
    }
}
