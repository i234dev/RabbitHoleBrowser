//! Mapped arguments exotic objects (sloppy functions with simple parameter
//! lists): while index `i` is mapped, `arguments[i]` and parameter `i` are
//! the same binding. The object stores ordinary properties; for mapped
//! indices the stored value may be stale and the parameter's scope slot
//! is authoritative (reads overlay it, writes go to both).

use crate::descriptor::PropDesc;
use crate::heap::*;
use crate::interop::JsResult;
use crate::realm::Realm;
use crate::value::*;

impl Realm {
    /// The (scope, slot) argument `key` of mapped arguments object `id`
    /// aliases, if it is mapped.
    #[inline]
    pub(crate) fn mapped_arg(&self, id: ObjectId, key: &PropertyKey) -> Option<(ObjectId, u32)> {
        let ObjectKind::MappedArguments(m) = &self.obj(id).kind else { return None };
        let i = key.as_array_index()? as usize;
        match m.slots.get(i) {
            Some(&s) if s != ArgMap::UNMAPPED => Some((m.scope, s)),
            _ => None,
        }
    }

    pub(crate) fn mapped_arg_read(&self, (scope, slot): (ObjectId, u32)) -> Value {
        match &self.obj(scope).kind {
            ObjectKind::Scope(s) => s.slots.get(slot as usize).cloned().unwrap_or(Value::Undefined),
            _ => Value::Undefined,
        }
    }

    pub(crate) fn mapped_arg_write(&mut self, (scope, slot): (ObjectId, u32), v: Value) {
        if let ObjectKind::Scope(s) = &mut self.obj_mut(scope).kind {
            if let Some(d) = s.slots.get_mut(slot as usize) {
                *d = v;
            }
        }
    }

    /// Remove index `key` from the parameter map of `id`.
    pub(crate) fn unmap_arg(&mut self, id: ObjectId, key: &PropertyKey) {
        let Some(i) = key.as_array_index() else { return };
        if let ObjectKind::MappedArguments(m) = &mut self.obj_mut(id).kind {
            if let Some(s) = m.slots.get_mut(i as usize) {
                *s = ArgMap::UNMAPPED;
            }
        }
    }

    /// [[DefineOwnProperty]] of a mapped arguments object.
    pub(crate) fn mapped_args_define(&mut self, id: ObjectId, key: PropertyKey, desc: PropDesc) -> JsResult<Option<bool>> {
        let Some(m) = self.mapped_arg(id, &key) else { return Ok(None) };
        let mut d = desc.clone();
        if desc.is_data() && desc.value.is_none() && desc.writable == Some(false) {
            d.value = Some(self.mapped_arg_read(m));
        }
        if !self.ordinary_define_own_pub(id, key.clone(), d)? {
            return Ok(Some(false));
        }
        if desc.is_accessor() {
            self.unmap_arg(id, &key);
        } else {
            if let Some(v) = desc.value {
                self.mapped_arg_write(m, v);
            }
            if desc.writable == Some(false) {
                self.unmap_arg(id, &key);
            }
        }
        Ok(Some(true))
    }
}
