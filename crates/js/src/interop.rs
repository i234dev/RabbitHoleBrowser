//! Abstract operations: property access with prototype chains, accessors
//! and proxies, type conversions, equality, iteration helpers.

use crate::heap::*;
use crate::realm::*;
use crate::value::*;
use std::rc::Rc;

pub type JsResult<T> = Result<T, Value>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    Default,
    Number,
    String,
}

impl Realm {
    // ------------------------------------------------------- property get

    /// Own property, synthesising array elements, string indices, etc.
    pub fn get_own_property(&self, id: ObjectId, key: &PropertyKey) -> Option<Property> {
        let obj = self.obj(id);
        match &obj.kind {
            ObjectKind::Array { elements, length } => {
                if let Some(i) = key.as_array_index() {
                    if (i as usize) < elements.len() {
                        let v = &elements[i as usize];
                        if !v.is_empty() {
                            return Some(Property::data(v.clone(), ALL));
                        }
                        return obj.props.get(key).cloned();
                    }
                } else if let PropertyKey::String(s) = key {
                    if s.eq_str("length") {
                        let frozen = obj.props.get(&crate::value::names::length()).map(|p| p.flags).unwrap_or(WRITABLE);
                        return Some(Property::data(Value::Number(*length as f64), frozen & WRITABLE));
                    }
                }
            }
            ObjectKind::String(s) => {
                if let Some(i) = key.as_array_index() {
                    if (i as usize) < s.len() {
                        return Some(Property::data(Value::String(s.slice(i as usize, i as usize + 1)), ENUMERABLE));
                    }
                } else if let PropertyKey::String(k) = key {
                    if k.eq_str("length") {
                        return Some(Property::data(Value::Number(s.len() as f64), 0));
                    }
                }
            }
            ObjectKind::TypedArray(t) => {
                // Integer-indexed exotic object: every canonical numeric key
                // is an element access; out-of-range ones have no property.
                if let Some(n) = crate::builtins::typed_array::canonical_numeric_index(key) {
                    let i = crate::builtins::typed_array::valid_integer_index(n, t.length)?;
                    return crate::builtins::typed_array_get(self, t, i).map(|v| Property::data(v, WRITABLE | ENUMERABLE | CONFIGURABLE));
                }
            }
            ObjectKind::Arguments => {}
            _ => {}
        }
        obj.props.get(key).cloned()
    }

    pub fn has_own_property(&self, id: ObjectId, key: &PropertyKey) -> bool {
        self.get_own_property(id, key).is_some()
    }

    pub fn get_prototype_of(&mut self, id: ObjectId) -> JsResult<Option<ObjectId>> {
        if let ObjectKind::Proxy { target, handler, revoked } = &self.obj(id).kind {
            let (target, handler) = (*target, *handler);
            if *revoked {
                return self.type_error("Cannot perform 'getPrototypeOf' on a proxy that has been revoked");
            }
            if let Some(trap) = self.get_method(Value::Object(handler), PropertyKey::str("getPrototypeOf"))? {
                let r = self.call(trap, Value::Object(handler), &[Value::Object(target)])?;
                return Ok(r.as_object());
            }
            return self.get_prototype_of(target);
        }
        Ok(self.obj(id).proto)
    }

    /// `[[Get]]` on any value (primitives use their wrapper prototype).
    pub fn get(&mut self, target: Value, key: PropertyKey) -> JsResult<Value> {
        match &target {
            Value::Object(id) => self.get_from_object(*id, &key, target.clone()),
            Value::String(s) => {
                if let Some(i) = key.as_array_index() {
                    if (i as usize) < s.len() {
                        return Ok(Value::String(s.slice(i as usize, i as usize + 1)));
                    }
                    return Ok(Value::Undefined);
                }
                if let PropertyKey::String(k) = &key {
                    if k.eq_str("length") {
                        return Ok(Value::Number(s.len() as f64));
                    }
                }
                let proto = self.intrinsics.string_proto;
                self.get_from_object(proto, &key, target.clone())
            }
            Value::Number(_) => {
                let p = self.intrinsics.number_proto;
                self.get_from_object(p, &key, target.clone())
            }
            Value::Bool(_) => {
                let p = self.intrinsics.boolean_proto;
                self.get_from_object(p, &key, target.clone())
            }
            Value::Symbol(_) => {
                let p = self.intrinsics.symbol_proto;
                self.get_from_object(p, &key, target.clone())
            }
            Value::BigInt(_) => {
                let p = self.intrinsics.bigint_proto;
                self.get_from_object(p, &key, target.clone())
            }
            Value::Undefined | Value::Null | Value::Empty => {
                let what = if target.is_null() { "null" } else { "undefined" };
                let k = self.key_display(&key);
                self.type_error(&format!("Cannot read properties of {} (reading '{}')", what, k))
            }
        }
    }

    pub fn key_display(&self, key: &PropertyKey) -> String {
        match key {
            PropertyKey::String(s) => s.to_rust_string(),
            PropertyKey::Symbol(s) => format!("Symbol({})", self.symbol_description(*s)),
        }
    }

    pub fn get_str(&mut self, target: Value, key: &str) -> JsResult<Value> {
        self.get(target, PropertyKey::str(key))
    }

    /// Walk the prototype chain starting at `id`, with `receiver` as `this` for getters.
    pub fn get_from_object(&mut self, id: ObjectId, key: &PropertyKey, receiver: Value) -> JsResult<Value> {
        let mut cur = id;
        let mut depth = 0;
        loop {
            let obj = self.obj(cur);
            match &obj.kind {
                ObjectKind::Proxy { .. } => return self.proxy_get(cur, key, receiver),
                ObjectKind::Array { .. } | ObjectKind::String(_) | ObjectKind::TypedArray(_) => {
                    // Exotic own properties (elements, length).
                    if let Some(p) = self.get_own_property(cur, key) {
                        if p.is_accessor() {
                            return match p.getter_cloned() {
                                Some(g) => self.call(g, receiver, &[]),
                                None => Ok(Value::Undefined),
                            };
                        }
                        return Ok(p.value);
                    }
                }
                _ => {
                    // Ordinary storage: no Property clone on the common path.
                    if let Some(p) = obj.props.get(key) {
                        if p.flags & ACCESSOR == 0 {
                            return Ok(p.value.clone());
                        }
                        return match p.getter_cloned() {
                            Some(g) => self.call(g, receiver, &[]),
                            None => Ok(Value::Undefined),
                        };
                    }
                }
            }
            match self.obj(cur).proto {
                Some(p) => cur = p,
                None => return Ok(Value::Undefined),
            }
            depth += 1;
            if depth > 10_000 {
                return self.type_error("Prototype chain too deep");
            }
        }
    }

    pub fn get_method(&mut self, target: Value, key: PropertyKey) -> JsResult<Option<Value>> {
        let v = self.get(target, key.clone())?;
        if v.is_nullish() {
            return Ok(None);
        }
        if !self.is_callable(&v) {
            let k = self.key_display(&key);
            return self.type_error(&format!("{} is not a function", k));
        }
        Ok(Some(v))
    }

    // ------------------------------------------------------- property set

    /// `[[Set]]`; returns false when the assignment silently fails (sloppy mode).
    pub fn set(&mut self, target: Value, key: PropertyKey, value: Value) -> JsResult<bool> {
        match &target {
            Value::Object(id) => self.set_on_object(*id, key, value, target.clone()),
            Value::Undefined | Value::Null | Value::Empty => {
                let what = if target.is_null() { "null" } else { "undefined" };
                let k = self.key_display(&key);
                self.type_error(&format!("Cannot set properties of {} (setting '{}')", what, k))
            }
            _ => {
                // Primitives: look for a setter on the prototype chain; otherwise ignore.
                let proto = match &target {
                    Value::String(_) => self.intrinsics.string_proto,
                    Value::Number(_) => self.intrinsics.number_proto,
                    Value::Bool(_) => self.intrinsics.boolean_proto,
                    Value::Symbol(_) => self.intrinsics.symbol_proto,
                    _ => self.intrinsics.bigint_proto,
                };
                let mut cur = Some(proto);
                while let Some(c) = cur {
                    if let Some(p) = self.obj(c).props.get(&key).cloned() {
                        if p.is_accessor() {
                            if let Some(s) = p.setter_cloned() {
                                self.call(s, target, &[value])?;
                                return Ok(true);
                            }
                        }
                        return Ok(false);
                    }
                    cur = self.obj(c).proto;
                }
                Ok(false)
            }
        }
    }

    pub fn set_str(&mut self, target: Value, key: &str, value: Value) -> JsResult<bool> {
        self.set(target, PropertyKey::str(key), value)
    }

    pub fn set_on_object(&mut self, id: ObjectId, key: PropertyKey, value: Value, receiver: Value) -> JsResult<bool> {
        // Fast path: own writable data property on an ordinary object / array element.
        {
            let obj = self.obj_mut(id);
            match &mut obj.kind {
                ObjectKind::Array { elements, length } => {
                    if let Some(i) = key.as_array_index() {
                        let i = i as usize;
                        if i < elements.len() {
                            if !elements[i].is_empty() || !obj.props.contains(&key) {
                                if obj.extensible || !elements[i].is_empty() {
                                    elements[i] = value;
                                    return Ok(true);
                                }
                            }
                        } else if i == elements.len() && obj.extensible && i < (1 << 28) && (*length as usize) <= i {
                            let frozen_len = obj.props.get(&crate::value::names::length()).map(|p| p.flags & WRITABLE == 0).unwrap_or(false);
                            if !frozen_len {
                                elements.push(value);
                                *length = (i + 1) as u32;
                                return Ok(true);
                            }
                        } else if i > elements.len() && i - elements.len() <= 4096 && obj.extensible && i < (1 << 28) {
                            let frozen_len = obj.props.get(&crate::value::names::length()).map(|p| p.flags & WRITABLE == 0).unwrap_or(false);
                            if !frozen_len {
                                // Fill holes (only if no sparse props exist in between).
                                elements.resize(i, Value::Empty);
                                elements.push(value);
                                if (*length as usize) < i + 1 {
                                    *length = (i + 1) as u32;
                                }
                                return Ok(true);
                            }
                        }
                    } else if let PropertyKey::String(s) = &key {
                        if s.eq_str("length") {
                            return self.set_array_length(id, value);
                        }
                    }
                }
                ObjectKind::TypedArray(t) => {
                    if let Some(n) = crate::builtins::typed_array::canonical_numeric_index(&key) {
                        let t = TypedArrayData { kind: t.kind, buffer: t.buffer, byte_offset: t.byte_offset, length: t.length };
                        // Conversion happens even for out-of-range indices (it may throw).
                        match crate::builtins::typed_array::valid_integer_index(n, t.length) {
                            Some(i) => crate::builtins::typed_array::ta_set_value(self, &t, i, value)?,
                            None => {
                                if matches!(t.kind, TypedArrayKind::BigInt64 | TypedArrayKind::BigUint64) {
                                    crate::builtins::typed_array::to_bigint(self, value)?;
                                } else {
                                    self.to_number(value)?;
                                }
                            }
                        }
                        return Ok(true);
                    }
                }
                ObjectKind::Proxy { .. } => {
                    return self.proxy_set(id, key, value, receiver);
                }
                _ => {}
            }
            if let Some(p) = obj.props.get_mut(&key) {
                if !p.is_accessor() {
                    if p.writable() {
                        if receiver.as_object() == Some(id) {
                            p.value = value;
                            return Ok(true);
                        }
                    } else {
                        return Ok(false);
                    }
                }
            }
        }
        // Slow path: walk the prototype chain looking for setters / non-writable.
        let mut cur = Some(id);
        while let Some(c) = cur {
            if let ObjectKind::Proxy { .. } = &self.obj(c).kind {
                if c != id {
                    return self.proxy_set(c, key, value, receiver);
                }
            }
            if let Some(p) = self.get_own_property(c, &key) {
                if p.is_accessor() {
                    return match p.setter_cloned() {
                        Some(s) => {
                            self.call(s, receiver, &[value])?;
                            Ok(true)
                        }
                        None => Ok(false),
                    };
                }
                if !p.writable() {
                    return Ok(false);
                }
                break;
            }
            cur = self.obj(c).proto;
        }
        // Define on receiver.
        let rid = match receiver {
            Value::Object(r) => r,
            _ => return Ok(false),
        };
        if let Some(existing) = self.get_own_property(rid, &key) {
            if existing.is_accessor() || !existing.writable() {
                return Ok(false);
            }
            self.obj_mut(rid).props.insert(key, Property::data(value, existing.flags));
            return Ok(true);
        }
        if !self.obj(rid).extensible {
            return Ok(false);
        }
        if let ObjectKind::Array { length, .. } = &mut self.obj_mut(rid).kind {
            if let Some(i) = key.as_array_index() {
                if i >= *length {
                    *length = i + 1;
                }
            }
        }
        self.obj_mut(rid).props.insert(key, Property::data(value, ALL));
        Ok(true)
    }

    pub fn set_array_length(&mut self, id: ObjectId, value: Value) -> JsResult<bool> {
        let n = self.to_number(value)?;
        let len = n as u32;
        if len as f64 != n {
            return self.range_error("Invalid array length");
        }
        let frozen = self.obj(id).props.get(&crate::value::names::length()).map(|p| p.flags & WRITABLE == 0).unwrap_or(false);
        if frozen {
            return Ok(false);
        }
        let obj = self.obj_mut(id);
        if let ObjectKind::Array { elements, length } = &mut obj.kind {
            if (len as usize) < elements.len() {
                elements.truncate(len as usize);
            }
            if len < *length {
                // remove sparse props >= len
                let keys: Vec<PropertyKey> = obj.props.keys().into_iter().filter(|k| k.as_array_index().map(|i| i >= len).unwrap_or(false)).collect();
                for k in keys {
                    obj.props.remove(&k);
                }
            }
            *length = len;
        }
        Ok(true)
    }

    /// Create or replace an own property; returns false if not allowed.
    pub fn define_own(&mut self, id: ObjectId, key: PropertyKey, prop: Property) -> JsResult<bool> {
        if let ObjectKind::Proxy { .. } = &self.obj(id).kind {
            return self.proxy_define(id, key, prop);
        }
        let existing = self.get_own_property(id, &key);
        if let Some(ex) = &existing {
            if !ex.configurable() {
                // Only allowed: same accessor/data kind, writable→non-writable, same value.
                if prop.is_accessor() != ex.is_accessor() {
                    return Ok(false);
                }
                if prop.configurable() || (prop.enumerable() != ex.enumerable()) {
                    return Ok(false);
                }
                if !ex.is_accessor() && !ex.writable() {
                    if prop.writable() {
                        return Ok(false);
                    }
                    if !self.same_value(&prop.value, &ex.value) {
                        return Ok(false);
                    }
                }
            }
        } else if !self.obj(id).extensible {
            return Ok(false);
        }
        // Arrays: keep dense storage in sync.
        let obj = self.obj_mut(id);
        if let ObjectKind::Array { elements, length } = &mut obj.kind {
            if let Some(i) = key.as_array_index() {
                let i_us = i as usize;
                if !prop.is_accessor() && prop.flags & ALL == ALL {
                    if i_us < elements.len() {
                        elements[i_us] = prop.value;
                        obj.props.remove(&key);
                        return Ok(true);
                    } else if i_us == elements.len() {
                        elements.push(prop.value);
                        if *length <= i {
                            *length = i + 1;
                        }
                        return Ok(true);
                    }
                }
                if i_us < elements.len() {
                    // move to sparse storage: mark hole
                    elements[i_us] = Value::Empty;
                }
                if *length <= i {
                    *length = i + 1;
                }
            } else if let PropertyKey::String(s) = &key {
                if s.eq_str("length") {
                    let new_flags = prop.flags;
                    let v = prop.value.clone();
                    let was_accessor = prop.is_accessor();
                    if was_accessor {
                        return Ok(false);
                    }
                    if !v.is_undefined() {
                        self.set_array_length(id, v)?;
                    }
                    // store the writability flag in props
                    self.obj_mut(id).props.insert(key, Property::data(Value::Undefined, new_flags & WRITABLE));
                    return Ok(true);
                }
            }
        }
        self.obj_mut(id).props.insert(key, prop);
        Ok(true)
    }

    pub fn delete_property(&mut self, id: ObjectId, key: &PropertyKey) -> JsResult<bool> {
        if let ObjectKind::Proxy { .. } = &self.obj(id).kind {
            return self.proxy_delete(id, key);
        }
        let obj = self.obj_mut(id);
        if let ObjectKind::Array { elements, .. } = &mut obj.kind {
            if let Some(i) = key.as_array_index() {
                if (i as usize) < elements.len() {
                    if i as usize == elements.len() - 1 {
                        elements.pop();
                    } else {
                        elements[i as usize] = Value::Empty;
                    }
                    obj.props.remove(key);
                    return Ok(true);
                }
            } else if let PropertyKey::String(s) = key {
                if s.eq_str("length") {
                    return Ok(false);
                }
            }
        }
        if let ObjectKind::String(s) = &obj.kind {
            if let Some(i) = key.as_array_index() {
                if (i as usize) < s.len() {
                    return Ok(false);
                }
            }
            if let PropertyKey::String(k) = key {
                if k.eq_str("length") {
                    return Ok(false);
                }
            }
        }
        match obj.props.get(key) {
            Some(p) if !p.configurable() => Ok(false),
            Some(_) => {
                obj.props.remove(key);
                Ok(true)
            }
            None => Ok(true),
        }
    }

    pub fn has_property(&mut self, id: ObjectId, key: &PropertyKey) -> JsResult<bool> {
        let mut cur = Some(id);
        while let Some(c) = cur {
            if let ObjectKind::Proxy { .. } = &self.obj(c).kind {
                return self.proxy_has(c, key);
            }
            if self.has_own_property(c, key) {
                return Ok(true);
            }
            cur = self.obj(c).proto;
        }
        Ok(false)
    }

    /// Own property keys in spec order (indices, strings, symbols).
    pub fn own_keys(&mut self, id: ObjectId) -> JsResult<Vec<PropertyKey>> {
        if let ObjectKind::Proxy { .. } = &self.obj(id).kind {
            return self.proxy_own_keys(id);
        }
        let obj = self.obj(id);
        let mut out = Vec::new();
        match &obj.kind {
            ObjectKind::Array { elements, .. } => {
                for (i, v) in elements.iter().enumerate() {
                    if !v.is_empty() {
                        out.push(PropertyKey::index(i as u32));
                    }
                }
            }
            ObjectKind::String(s) => {
                for i in 0..s.len() {
                    out.push(PropertyKey::index(i as u32));
                }
            }
            ObjectKind::TypedArray(t) => {
                for i in 0..t.length {
                    out.push(PropertyKey::index(i as u32));
                }
            }
            _ => {}
        }
        let mut rest = obj.props.keys();
        if !out.is_empty() {
            // merge indices from props (sparse) in order
            let (sparse_idx, others): (Vec<PropertyKey>, Vec<PropertyKey>) = rest.into_iter().partition(|k| k.as_array_index().is_some());
            let mut all_idx: Vec<u32> = out.iter().chain(sparse_idx.iter()).filter_map(|k| k.as_array_index()).collect();
            all_idx.sort_unstable();
            all_idx.dedup();
            out = all_idx.into_iter().map(PropertyKey::index).collect();
            rest = others;
        }
        if matches!(obj.kind, ObjectKind::Array { .. }) {
            out.push(crate::value::names::length());
            rest.retain(|k| !matches!(k, PropertyKey::String(s) if s.eq_str("length")));
        }
        if matches!(obj.kind, ObjectKind::String(_)) {
            out.push(crate::value::names::length());
        }
        // private symbols are hidden
        let private: Vec<bool> = rest.iter().map(|k| matches!(k, PropertyKey::Symbol(s) if self.is_private_symbol(*s))).collect();
        for (k, p) in rest.into_iter().zip(private) {
            if !p {
                out.push(k);
            }
        }
        Ok(out)
    }

    /// Own enumerable string keys (for `Object.keys`, for-in, spread).
    pub fn own_enumerable_string_keys(&mut self, id: ObjectId) -> JsResult<Vec<PropertyKey>> {
        let keys = self.own_keys(id)?;
        let mut out = Vec::new();
        for k in keys {
            if k.is_symbol() {
                continue;
            }
            let enumerable = if let ObjectKind::Proxy { .. } = &self.obj(id).kind {
                match self.proxy_get_own_property(id, &k)? {
                    Some(p) => p.enumerable(),
                    None => false,
                }
            } else {
                self.get_own_property(id, &k).map(|p| p.enumerable()).unwrap_or(false)
            };
            if enumerable {
                out.push(k);
            }
        }
        Ok(out)
    }

    // ---------------------------------------------------------- proxies

    fn proxy_parts(&mut self, id: ObjectId, trap: &str) -> JsResult<(ObjectId, ObjectId, Option<Value>)> {
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

    fn proxy_get(&mut self, id: ObjectId, key: &PropertyKey, receiver: Value) -> JsResult<Value> {
        let (target, handler, trap) = self.proxy_parts(id, "get")?;
        match trap {
            Some(t) => self.call(t, Value::Object(handler), &[Value::Object(target), key.to_value(), receiver]),
            None => self.get_from_object(target, key, receiver),
        }
    }

    fn proxy_set(&mut self, id: ObjectId, key: PropertyKey, value: Value, receiver: Value) -> JsResult<bool> {
        let (target, handler, trap) = self.proxy_parts(id, "set")?;
        match trap {
            Some(t) => {
                let r = self.call(t, Value::Object(handler), &[Value::Object(target), key.to_value(), value, receiver])?;
                Ok(r.truthy())
            }
            None => self.set_on_object(target, key, value, receiver),
        }
    }

    fn proxy_has(&mut self, id: ObjectId, key: &PropertyKey) -> JsResult<bool> {
        let (target, handler, trap) = self.proxy_parts(id, "has")?;
        match trap {
            Some(t) => Ok(self.call(t, Value::Object(handler), &[Value::Object(target), key.to_value()])?.truthy()),
            None => self.has_property(target, key),
        }
    }

    fn proxy_delete(&mut self, id: ObjectId, key: &PropertyKey) -> JsResult<bool> {
        let (target, handler, trap) = self.proxy_parts(id, "deleteProperty")?;
        match trap {
            Some(t) => Ok(self.call(t, Value::Object(handler), &[Value::Object(target), key.to_value()])?.truthy()),
            None => self.delete_property(target, key),
        }
    }

    fn proxy_own_keys(&mut self, id: ObjectId) -> JsResult<Vec<PropertyKey>> {
        let (target, handler, trap) = self.proxy_parts(id, "ownKeys")?;
        match trap {
            Some(t) => {
                let r = self.call(t, Value::Object(handler), &[Value::Object(target)])?;
                let list = self.create_list_from_array_like(r)?;
                let mut out = Vec::new();
                for v in list {
                    out.push(self.to_property_key(v)?);
                }
                Ok(out)
            }
            None => self.own_keys(target),
        }
    }

    pub fn proxy_get_own_property(&mut self, id: ObjectId, key: &PropertyKey) -> JsResult<Option<Property>> {
        let (target, handler, trap) = self.proxy_parts(id, "getOwnPropertyDescriptor")?;
        match trap {
            Some(t) => {
                let r = self.call(t, Value::Object(handler), &[Value::Object(target), key.to_value()])?;
                if r.is_undefined() {
                    return Ok(None);
                }
                Ok(Some(self.to_property_descriptor(r)?))
            }
            None => {
                if let ObjectKind::Proxy { .. } = &self.obj(target).kind {
                    return self.proxy_get_own_property(target, key);
                }
                Ok(self.get_own_property(target, key))
            }
        }
    }

    fn proxy_define(&mut self, id: ObjectId, key: PropertyKey, prop: Property) -> JsResult<bool> {
        let (target, handler, trap) = self.proxy_parts(id, "defineProperty")?;
        match trap {
            Some(t) => {
                let desc = self.from_property_descriptor(&prop);
                Ok(self.call(t, Value::Object(handler), &[Value::Object(target), key.to_value(), desc])?.truthy())
            }
            None => self.define_own(target, key, prop),
        }
    }

    /// Resolve the own property of any object kind including proxies.
    pub fn get_own_property_any(&mut self, id: ObjectId, key: &PropertyKey) -> JsResult<Option<Property>> {
        if let ObjectKind::Proxy { .. } = &self.obj(id).kind {
            return self.proxy_get_own_property(id, key);
        }
        Ok(self.get_own_property(id, key))
    }

    // ------------------------------------------------------ descriptors

    pub fn to_property_descriptor(&mut self, v: Value) -> JsResult<Property> {
        let id = match v {
            Value::Object(id) => id,
            _ => return self.type_error("Property description must be an object"),
        };
        let has = |r: &mut Realm, k: &str| r.has_property(id, &PropertyKey::str(k));
        let mut flags = 0u8;
        if has(self, "enumerable")? && self.get_str(Value::Object(id), "enumerable")?.truthy() {
            flags |= ENUMERABLE;
        }
        if has(self, "configurable")? && self.get_str(Value::Object(id), "configurable")?.truthy() {
            flags |= CONFIGURABLE;
        }
        let has_get = has(self, "get")?;
        let has_set = has(self, "set")?;
        if has_get || has_set {
            if has(self, "value")? || has(self, "writable")? {
                return self.type_error("Invalid property descriptor. Cannot both specify accessors and a value or writable attribute");
            }
            let g = if has_get { self.get_str(Value::Object(id), "get")? } else { Value::Undefined };
            let s = if has_set { self.get_str(Value::Object(id), "set")? } else { Value::Undefined };
            if !g.is_undefined() && !self.is_callable(&g) {
                return self.type_error("Getter must be a function");
            }
            if !s.is_undefined() && !self.is_callable(&s) {
                return self.type_error("Setter must be a function");
            }
            let g = if g.is_undefined() { None } else { Some(g) };
            let s = if s.is_undefined() { None } else { Some(s) };
            return Ok(Property::accessor(g, s, flags));
        }
        if has(self, "writable")? && self.get_str(Value::Object(id), "writable")?.truthy() {
            flags |= WRITABLE;
        }
        let value = if has(self, "value")? { self.get_str(Value::Object(id), "value")? } else { Value::Undefined };
        Ok(Property::data(value, flags))
    }

    /// Like `to_property_descriptor` but merges with an existing property
    /// for fields the descriptor leaves out (defineProperty semantics).
    pub fn to_property_descriptor_merged(&mut self, v: Value, existing: Option<&Property>) -> JsResult<Property> {
        let id = match v {
            Value::Object(id) => id,
            _ => return self.type_error("Property description must be an object"),
        };
        let has_enum = self.has_property(id, &PropertyKey::str("enumerable"))?;
        let has_conf = self.has_property(id, &PropertyKey::str("configurable"))?;
        let has_writ = self.has_property(id, &PropertyKey::str("writable"))?;
        let has_value = self.has_property(id, &PropertyKey::str("value"))?;
        let has_get = self.has_property(id, &PropertyKey::str("get"))?;
        let has_set = self.has_property(id, &PropertyKey::str("set"))?;
        let mut p = self.to_property_descriptor(v)?;
        if let Some(ex) = existing {
            if !has_enum && ex.enumerable() {
                p.flags |= ENUMERABLE;
            }
            if !has_conf && ex.configurable() {
                p.flags |= CONFIGURABLE;
            }
            if !(has_get || has_set) {
                if ex.is_accessor() && !has_value && !has_writ {
                    p.flags |= ACCESSOR;
                    p.flags &= !WRITABLE;
                    p.set_getter(ex.getter_cloned());
                    p.set_setter(ex.setter_cloned());
                } else {
                    if !has_writ && ex.writable() && !ex.is_accessor() {
                        p.flags |= WRITABLE;
                    }
                    if !has_value && !ex.is_accessor() {
                        p.value = ex.value.clone();
                    }
                }
            } else {
                if !has_get && ex.is_accessor() {
                    p.set_getter(ex.getter_cloned());
                }
                if !has_set && ex.is_accessor() {
                    p.set_setter(ex.setter_cloned());
                }
            }
        }
        Ok(p)
    }

    pub fn from_property_descriptor(&mut self, p: &Property) -> Value {
        let o = self.new_object();
        if p.is_accessor() {
            self.define_property(o, "get", p.getter_cloned().unwrap_or(Value::Undefined), ALL);
            self.define_property(o, "set", p.setter_cloned().unwrap_or(Value::Undefined), ALL);
        } else {
            self.define_property(o, "value", p.value.clone(), ALL);
            self.define_property(o, "writable", Value::Bool(p.writable()), ALL);
        }
        self.define_property(o, "enumerable", Value::Bool(p.enumerable()), ALL);
        self.define_property(o, "configurable", Value::Bool(p.configurable()), ALL);
        Value::Object(o)
    }

    // ------------------------------------------------------- conversions

    pub fn to_primitive(&mut self, v: Value, hint: Hint) -> JsResult<Value> {
        let id = match v {
            Value::Object(id) => id,
            other => return Ok(other),
        };
        if let Some(exotic) = self.get_method(Value::Object(id), PropertyKey::Symbol(SYM_TO_PRIMITIVE))? {
            let h = match hint {
                Hint::Default => "default",
                Hint::Number => "number",
                Hint::String => "string",
            };
            let r = self.call(exotic, Value::Object(id), &[Value::str(h)])?;
            if r.is_object() {
                return self.type_error("Cannot convert object to primitive value");
            }
            return Ok(r);
        }
        let order: [&str; 2] = if hint == Hint::String { ["toString", "valueOf"] } else { ["valueOf", "toString"] };
        for name in order {
            let m = self.get_str(Value::Object(id), name)?;
            if self.is_callable(&m) {
                let r = self.call(m, Value::Object(id), &[])?;
                if !r.is_object() {
                    return Ok(r);
                }
            }
        }
        self.type_error("Cannot convert object to primitive value")
    }

    pub fn to_number(&mut self, v: Value) -> JsResult<f64> {
        Ok(match v {
            Value::Undefined | Value::Empty => f64::NAN,
            Value::Null => 0.0,
            Value::Bool(b) => {
                if b {
                    1.0
                } else {
                    0.0
                }
            }
            Value::Number(n) => n,
            Value::String(s) => string_to_number(&s),
            Value::Symbol(_) => return self.type_error("Cannot convert a Symbol value to a number"),
            Value::BigInt(_) => return self.type_error("Cannot convert a BigInt value to a number"),
            Value::Object(_) => {
                let p = self.to_primitive(v, Hint::Number)?;
                return self.to_number(p);
            }
        })
    }

    /// ToNumeric: Number or BigInt.
    pub fn to_numeric(&mut self, v: Value) -> JsResult<Value> {
        let p = self.to_primitive(v, Hint::Number)?;
        if let Value::BigInt(_) = p {
            return Ok(p);
        }
        Ok(Value::Number(self.to_number(p)?))
    }

    pub fn to_string(&mut self, v: Value) -> JsResult<JsStr> {
        Ok(match v {
            Value::Undefined | Value::Empty => JsStr::new("undefined"),
            Value::Null => JsStr::new("null"),
            Value::Bool(b) => JsStr::new(if b { "true" } else { "false" }),
            Value::Number(n) if n >= 0.0 && n < 65536.0 && n == (n as u32) as f64 && !(n == 0.0 && n.is_sign_negative()) => JsStr::from_index(n as u32),
            Value::Number(n) => JsStr::new(&number_to_string(n)),
            Value::String(s) => s,
            Value::Symbol(_) => return self.type_error("Cannot convert a Symbol value to a string"),
            Value::BigInt(b) => JsStr::new(&b.to_string_radix(10)),
            Value::Object(_) => {
                let p = self.to_primitive(v, Hint::String)?;
                return self.to_string(p);
            }
        })
    }

    pub fn to_rust_string(&mut self, v: Value) -> JsResult<String> {
        Ok(self.to_string(v)?.to_rust_string())
    }

    pub fn to_boolean(&self, v: &Value) -> bool {
        v.truthy()
    }

    pub fn to_object(&mut self, v: Value) -> JsResult<ObjectId> {
        Ok(match v {
            Value::Object(id) => id,
            Value::Undefined | Value::Null | Value::Empty => return self.type_error("Cannot convert undefined or null to object"),
            Value::Bool(b) => {
                let p = self.intrinsics.boolean_proto;
                self.alloc(Object::new(Some(p), ObjectKind::Boolean(b)))
            }
            Value::Number(n) => {
                let p = self.intrinsics.number_proto;
                self.alloc(Object::new(Some(p), ObjectKind::Number(n)))
            }
            Value::String(s) => {
                let p = self.intrinsics.string_proto;
                self.alloc(Object::new(Some(p), ObjectKind::String(s)))
            }
            Value::Symbol(s) => {
                let p = self.intrinsics.symbol_proto;
                self.alloc(Object::new(Some(p), ObjectKind::Symbol(s)))
            }
            Value::BigInt(b) => {
                let p = self.intrinsics.bigint_proto;
                self.alloc(Object::new(Some(p), ObjectKind::BigInt(b)))
            }
        })
    }

    pub fn to_property_key(&mut self, v: Value) -> JsResult<PropertyKey> {
        Ok(match v {
            Value::String(s) => PropertyKey::String(s),
            Value::Symbol(s) => PropertyKey::Symbol(s),
            Value::Number(n) => {
                if n >= 0.0 && n < 4294967295.0 && n.fract() == 0.0 {
                    PropertyKey::index(n as u32)
                } else {
                    PropertyKey::String(JsStr::new(&number_to_string(n)))
                }
            }
            Value::Object(_) => {
                let p = self.to_primitive(v, Hint::String)?;
                return self.to_property_key(p);
            }
            other => PropertyKey::String(self.to_string(other)?),
        })
    }

    pub fn to_length(&mut self, v: Value) -> JsResult<usize> {
        let n = to_integer(self.to_number(v)?);
        if n <= 0.0 {
            return Ok(0);
        }
        Ok(n.min(9007199254740991.0) as usize)
    }

    pub fn to_integer_or_infinity(&mut self, v: Value) -> JsResult<f64> {
        Ok(to_integer(self.to_number(v)?))
    }

    pub fn to_index(&mut self, v: Value) -> JsResult<usize> {
        if v.is_undefined() {
            return Ok(0);
        }
        let n = to_integer(self.to_number(v)?);
        if n < 0.0 || n > 9007199254740991.0 {
            return self.range_error("Invalid index");
        }
        Ok(n as usize)
    }

    pub fn to_int32(&mut self, v: Value) -> JsResult<i32> {
        Ok(to_int32(self.to_number(v)?))
    }

    pub fn to_uint32(&mut self, v: Value) -> JsResult<u32> {
        Ok(to_uint32(self.to_number(v)?))
    }

    /// Relative index helper used by slice/splice/at: resolves negative
    /// indices against `len`, clamping to `[0, len]`.
    pub fn relative_index(&mut self, v: Value, len: usize, default: usize) -> JsResult<usize> {
        if v.is_undefined() {
            return Ok(default);
        }
        let n = to_integer(self.to_number(v)?);
        Ok(if n < 0.0 { (len as f64 + n).max(0.0) as usize } else { n.min(len as f64) as usize })
    }

    // ---------------------------------------------------------- equality

    pub fn strict_equals(&self, a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => true,
            (Value::Empty, Value::Empty) | (Value::Empty, Value::Undefined) | (Value::Undefined, Value::Empty) => true,
            (Value::Bool(x), Value::Bool(y)) => x == y,
            (Value::Number(x), Value::Number(y)) => x == y,
            (Value::String(x), Value::String(y)) => x == y,
            (Value::Symbol(x), Value::Symbol(y)) => x == y,
            (Value::BigInt(x), Value::BigInt(y)) => x == y,
            (Value::Object(x), Value::Object(y)) => x == y,
            _ => false,
        }
    }

    pub fn same_value(&self, a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => {
                if x.is_nan() && y.is_nan() {
                    return true;
                }
                if *x == 0.0 && *y == 0.0 {
                    return x.is_sign_negative() == y.is_sign_negative();
                }
                x == y
            }
            _ => self.strict_equals(a, b),
        }
    }

    pub fn same_value_zero(&self, a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) if x.is_nan() && y.is_nan() => true,
            _ => self.strict_equals(a, b),
        }
    }

    pub fn loose_equals(&mut self, a: Value, b: Value) -> JsResult<bool> {
        match (&a, &b) {
            (Value::Undefined | Value::Null | Value::Empty, Value::Undefined | Value::Null | Value::Empty) => Ok(true),
            (Value::Undefined | Value::Null | Value::Empty, _) | (_, Value::Undefined | Value::Null | Value::Empty) => Ok(false),
            (Value::Number(_), Value::String(s)) => {
                let n = string_to_number(s);
                Ok(a.as_number() == Some(n))
            }
            (Value::String(s), Value::Number(_)) => {
                let n = string_to_number(s);
                Ok(b.as_number() == Some(n))
            }
            (Value::BigInt(x), Value::Number(y)) | (Value::Number(y), Value::BigInt(x)) => Ok(x.eq_f64(*y)),
            (Value::BigInt(x), Value::String(s)) | (Value::String(s), Value::BigInt(x)) => Ok(BigInt::parse(&s.to_rust_string()).map(|b| b == **x).unwrap_or(false)),
            (Value::Bool(x), _) => {
                let n = if *x { 1.0 } else { 0.0 };
                self.loose_equals(Value::Number(n), b)
            }
            (_, Value::Bool(y)) => {
                let n = if *y { 1.0 } else { 0.0 };
                self.loose_equals(a, Value::Number(n))
            }
            (Value::Object(_), Value::Object(_)) => Ok(self.strict_equals(&a, &b)),
            (Value::Object(_), _) => {
                let p = self.to_primitive(a, Hint::Default)?;
                self.loose_equals(p, b)
            }
            (_, Value::Object(_)) => {
                let p = self.to_primitive(b, Hint::Default)?;
                self.loose_equals(a, p)
            }
            _ => Ok(self.strict_equals(&a, &b)),
        }
    }

    pub fn instance_of(&mut self, v: Value, target: Value) -> JsResult<bool> {
        if !target.is_object() {
            return self.type_error("Right-hand side of 'instanceof' is not an object");
        }
        if let Some(h) = self.get_method(target.clone(), PropertyKey::Symbol(SYM_HAS_INSTANCE))? {
            let r = self.call(h, target, &[v])?;
            return Ok(r.truthy());
        }
        if !self.is_callable(&target) {
            return self.type_error("Right-hand side of 'instanceof' is not callable");
        }
        self.ordinary_has_instance(target, v)
    }

    pub fn ordinary_has_instance(&mut self, c: Value, v: Value) -> JsResult<bool> {
        if !self.is_callable(&c) {
            return Ok(false);
        }
        let cid = c.as_object().unwrap();
        if let ObjectKind::Bound(b) = &self.obj(cid).kind {
            let t = Value::Object(b.target);
            return self.instance_of(v, t);
        }
        let mut o = match v {
            Value::Object(o) => o,
            _ => return Ok(false),
        };
        let proto = self.get_str(c, "prototype")?;
        let proto = match proto {
            Value::Object(p) => p,
            _ => return self.type_error("Function has non-object prototype in instanceof check"),
        };
        loop {
            match self.get_prototype_of(o)? {
                None => return Ok(false),
                Some(p) => {
                    if p == proto {
                        return Ok(true);
                    }
                    o = p;
                }
            }
        }
    }

    pub fn type_of(&self, v: &Value) -> &'static str {
        match v {
            Value::Object(o) => {
                if self.is_callable(v) {
                    "function"
                } else {
                    let _ = o;
                    "object"
                }
            }
            other => other.type_of(),
        }
    }

    // ---------------------------------------------------------- iteration

    /// GetIterator: returns (iterator, next method).
    pub fn get_iterator(&mut self, v: Value) -> JsResult<(Value, Value)> {
        let method = self.get_method(v.clone(), PropertyKey::Symbol(SYM_ITERATOR))?;
        let method = match method {
            Some(m) => m,
            None => {
                let d = self.describe_for_error(&v);
                return self.type_error(&format!("{} is not iterable", d));
            }
        };
        let it = self.call(method, v, &[])?;
        if !it.is_object() {
            return self.type_error("Result of the Symbol.iterator method is not an object");
        }
        let next = self.get_str(it.clone(), "next")?;
        Ok((it, next))
    }

    pub fn get_async_iterator(&mut self, v: Value) -> JsResult<(Value, Value)> {
        let method = self.get_method(v.clone(), PropertyKey::Symbol(SYM_ASYNC_ITERATOR))?;
        match method {
            Some(m) => {
                let it = self.call(m, v, &[])?;
                let next = self.get_str(it.clone(), "next")?;
                Ok((it, next))
            }
            None => {
                // Wrap a sync iterator.
                let (it, next) = self.get_iterator(v)?;
                let wrapped = crate::builtins::iterator::create_async_from_sync_iterator(self, it, next);
                let next = self.get_str(wrapped.clone(), "next")?;
                Ok((wrapped, next))
            }
        }
    }

    /// IteratorStep: Some(value) or None when done.
    pub fn iterator_step(&mut self, it: &Value, next: &Value) -> JsResult<Option<Value>> {
        let r = self.call(next.clone(), it.clone(), &[])?;
        if !r.is_object() {
            return self.type_error("Iterator result is not an object");
        }
        let done = self.get_str(r.clone(), "done")?.truthy();
        if done {
            return Ok(None);
        }
        Ok(Some(self.get_str(r, "value")?))
    }

    pub fn iterator_close(&mut self, it: &Value) -> JsResult<()> {
        let ret = self.get_method(it.clone(), PropertyKey::str("return"))?;
        if let Some(r) = ret {
            let v = self.call(r, it.clone(), &[])?;
            if !v.is_object() {
                return self.type_error("Iterator result is not an object");
            }
        }
        Ok(())
    }

    /// Collect an iterable into a Vec (fast path for plain arrays).
    pub fn iterate_to_vec(&mut self, v: Value) -> JsResult<Vec<Value>> {
        if let Value::Object(id) = &v {
            if let ObjectKind::Array { elements, length } = &self.obj(*id).kind {
                // Only when the array iterator is untouched.
                let proto_ok = self.obj(*id).proto == Some(self.intrinsics.array_proto) && !self.obj(*id).props.contains(&PropertyKey::Symbol(SYM_ITERATOR));
                if proto_ok && self.array_iterator_pristine() && *length as usize == elements.len() {
                    return Ok(elements.iter().map(|e| if e.is_empty() { Value::Undefined } else { e.clone() }).collect());
                }
            }
        }
        let (it, next) = self.get_iterator(v)?;
        let mut out = Vec::new();
        while let Some(x) = self.iterator_step(&it, &next)? {
            out.push(x);
        }
        Ok(out)
    }

    pub(crate) fn array_iterator_pristine(&self) -> bool {
        // Cheap check: Array.prototype[Symbol.iterator] is still the intrinsic function.
        match self.intrinsics.others.get("ArrayProtoValues") {
            Some(orig) => self.obj(self.intrinsics.array_proto).props.get(&PropertyKey::Symbol(SYM_ITERATOR)).map(|p| p.value.as_object() == Some(*orig)).unwrap_or(false),
            None => false,
        }
    }

    pub fn create_iter_result(&mut self, value: Value, done: bool) -> Value {
        let o = self.new_object();
        self.define_property(o, names::value(), value, ALL);
        self.define_property(o, names::done(), Value::Bool(done), ALL);
        Value::Object(o)
    }

    pub fn create_list_from_array_like(&mut self, v: Value) -> JsResult<Vec<Value>> {
        let id = match v {
            Value::Object(id) => id,
            _ => return self.type_error("CreateListFromArrayLike called on non-object"),
        };
        if let ObjectKind::Array { elements, length } = &self.obj(id).kind {
            if *length as usize == elements.len() {
                return Ok(elements.iter().map(|e| if e.is_empty() { Value::Undefined } else { e.clone() }).collect());
            }
        }
        let len = self.get_str(Value::Object(id), "length")?;
        let len = self.to_length(len)?;
        if self.no_index_properties(id) {
            // `{length: n}`: every element reads as undefined.
            return Ok(vec![Value::Undefined; len]);
        }
        let mut out = Vec::with_capacity(len);
        for i in 0..len {
            out.push(self.get(Value::Object(id), PropertyKey::index(i as u32))?);
        }
        Ok(out)
    }

    /// No object on the prototype chain of `id` (itself included) can have
    /// an array-index property: ordinary objects (and empty arrays) whose
    /// property keys are all non-index strings or symbols.
    pub fn no_index_properties(&self, id: ObjectId) -> bool {
        let mut cur = Some(id);
        let mut depth = 0;
        while let Some(c) = cur {
            let Some(o) = self.heap.try_get(c) else { return false };
            match &o.kind {
                ObjectKind::Ordinary | ObjectKind::Function(_) | ObjectKind::Native(_) | ObjectKind::Error => {}
                ObjectKind::Array { elements, .. } if elements.is_empty() => {}
                _ => return false,
            }
            for (k, _) in o.props.iter() {
                if k.as_array_index().is_some() {
                    return false;
                }
            }
            cur = o.proto;
            depth += 1;
            if depth > 64 {
                return false;
            }
        }
        true
    }

    /// Length of an array-like.
    pub fn length_of(&mut self, v: Value) -> JsResult<usize> {
        if let Value::Object(id) = &v {
            if let ObjectKind::Array { length, .. } = &self.obj(*id).kind {
                return Ok(*length as usize);
            }
        }
        let l = self.get_str(v, "length")?;
        self.to_length(l)
    }

    pub fn invoke(&mut self, target: Value, name: &str, args: &[Value]) -> JsResult<Value> {
        let f = self.get_str(target.clone(), name)?;
        if !self.is_callable(&f) {
            return self.type_error(&format!("{} is not a function", name));
        }
        self.call(f, target, args)
    }

    pub fn describe_for_error(&mut self, v: &Value) -> String {
        match v {
            Value::String(s) => format!("\"{}\"", s),
            Value::Object(id) => match &self.obj(*id).kind {
                ObjectKind::Function(c) => format!("function {}", c.code.name),
                ObjectKind::Native(n) => format!("function {}", n.name),
                ObjectKind::Array { .. } => "array".into(),
                _ => "object".into(),
            },
            Value::Undefined | Value::Empty => "undefined".into(),
            other => self.to_string(other.clone()).map(|s| s.to_rust_string()).unwrap_or_else(|_| other.type_of().into()),
        }
    }

    /// Species constructor lookup (simplified).
    pub fn species_constructor(&mut self, obj: ObjectId, default: ObjectId) -> JsResult<Value> {
        let c = self.get_str(Value::Object(obj), "constructor")?;
        if c.is_undefined() {
            return Ok(Value::Object(default));
        }
        if !c.is_object() {
            return self.type_error("object.constructor is not an object");
        }
        let s = self.get(c, PropertyKey::Symbol(SYM_SPECIES))?;
        if s.is_nullish() {
            return Ok(Value::Object(default));
        }
        if self.is_constructor(&s) {
            return Ok(s);
        }
        self.type_error("object.constructor[Symbol.species] is not a constructor")
    }

    pub fn function_name(&self, f: ObjectId) -> JsStr {
        match &self.obj(f).kind {
            ObjectKind::Function(c) => c.code.name.clone(),
            ObjectKind::Native(n) => n.name.clone(),
            _ => JsStr::empty(),
        }
    }

    pub fn new_bigint(&mut self, v: i128) -> Value {
        Value::BigInt(Rc::new(BigInt::from_i128(v)))
    }

    pub fn new_bigint_value(&mut self, v: BigInt) -> Value {
        Value::BigInt(Rc::new(v))
    }
}
