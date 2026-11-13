//! Several ECMAScript realms in one agent (`$262.createRealm`).
//!
//! A [`Realm`] is the agent: heap, stacks, job queue, symbol registry. Its
//! `intrinsics`, `global`, `global_lex` and RegExp fast-path state are those
//! of the *running* realm; other realms' wait in [`Realm::realms`] and are
//! swapped in when one of their functions runs. Every function knows its
//! realm (`FunctionCode::realm` for closures, `NativeFunction::realm` for
//! built-ins): the run loop switches when it continues a frame of another
//! realm, a native call switches around the call, and each entry from Rust
//! (`Realm::run`) restores the caller's realm. With a single realm nothing
//! is ever switched (every index is 0).

use crate::bytecode::FunctionCode;
use crate::heap::*;
use crate::interop::JsResult;
use crate::realm::{Intrinsics, Realm};
use crate::value::*;

/// The per-realm part of [`Realm`] while another realm is running.
pub struct RealmRecord {
    pub intrinsics: Intrinsics,
    pub global: ObjectId,
    pub global_lex: crate::ic::GlobalLex,
    pub(crate) regexp_pristine: Option<Box<crate::builtins::regexp::Pristine>>,
}

impl Intrinsics {
    /// The intrinsics kept in fields (the others are in `others`).
    pub fn fixed(&self) -> [ObjectId; 19] {
        [
            self.object_proto,
            self.function_proto,
            self.array_proto,
            self.string_proto,
            self.number_proto,
            self.boolean_proto,
            self.symbol_proto,
            self.bigint_proto,
            self.error_proto,
            self.iterator_proto,
            self.array_iterator_proto,
            self.generator_proto,
            self.async_generator_proto,
            self.promise_proto,
            self.promise_ctor,
            self.object_ctor,
            self.array_ctor,
            self.function_ctor,
            self.eval_fn,
        ]
    }

    /// Push every intrinsic (GC roots).
    pub fn trace(&self, out: &mut Vec<ObjectId>) {
        out.extend(self.fixed());
        out.extend(self.others.values().copied());
    }
}

impl RealmRecord {
    /// GC roots of a realm that is not running.
    pub(crate) fn trace(&self, out: &mut Vec<ObjectId>) {
        self.intrinsics.trace(out);
        out.push(self.global);
        for (v, _) in self.global_lex.values() {
            if let V::Object(o) = v.v() {
                out.push(o);
            }
        }
    }
}

impl Realm {
    /// Mark freshly compiled code (and the functions in it) as the running
    /// realm's.
    #[inline]
    pub(crate) fn adopt_code(&self, code: &FunctionCode) {
        if self.current_realm != 0 {
            code.set_realm(self.current_realm);
        }
    }

    /// Number of realms in this agent.
    pub fn realm_count(&self) -> usize {
        self.realms.len().max(1)
    }

    /// Make realm `r` the running one.
    #[cold]
    #[inline(never)]
    pub fn switch_realm(&mut self, r: u16) {
        let cur = self.current_realm;
        if r == cur {
            return;
        }
        let Some(mut rec) = self.realms.get_mut(r as usize).and_then(Option::take) else {
            return;
        };
        std::mem::swap(&mut self.intrinsics, &mut rec.intrinsics);
        std::mem::swap(&mut self.global, &mut rec.global);
        std::mem::swap(&mut self.global_lex, &mut rec.global_lex);
        std::mem::swap(&mut self.regexp_pristine, &mut rec.regexp_pristine);
        self.realms[cur as usize] = Some(rec);
        self.current_realm = r;
    }

    /// Create a new realm with its own intrinsics and global object
    /// (`$262.createRealm`); returns its index. The running realm does not
    /// change.
    pub fn create_realm(&mut self) -> u16 {
        if self.realms.is_empty() {
            self.realms.push(None);
        }
        let id = self.realms.len() as u16;
        let (intrinsics, global) = Self::bootstrap_intrinsics(&mut self.heap, id);
        self.realms.push(Some(Box::new(RealmRecord { intrinsics, global, global_lex: Default::default(), regexp_pristine: None })));
        let prev = self.current_realm;
        self.switch_realm(id);
        self.install_builtins();
        self.switch_realm(prev);
        id
    }

    /// Run `f` with realm `r` running.
    pub fn in_realm<T>(&mut self, r: u16, f: impl FnOnce(&mut Realm) -> T) -> T {
        let prev = self.current_realm;
        self.switch_realm(r);
        let out = f(self);
        self.switch_realm(prev);
        out
    }

    /// Intrinsics of realm `r`.
    pub fn realm_intrinsics(&self, r: u16) -> &Intrinsics {
        if r == self.current_realm {
            return &self.intrinsics;
        }
        match self.realms.get(r as usize).and_then(Option::as_ref) {
            Some(rec) => &rec.intrinsics,
            None => &self.intrinsics,
        }
    }

    /// Global object of realm `r`.
    pub fn realm_global(&self, r: u16) -> ObjectId {
        if r == self.current_realm {
            return self.global;
        }
        match self.realms.get(r as usize).and_then(Option::as_ref) {
            Some(rec) => rec.global,
            None => self.global,
        }
    }

    /// GetFunctionRealm(f): the realm of a function object (bound functions
    /// and proxies: their target's); the running realm for other objects.
    /// A revoked proxy throws a TypeError.
    pub fn function_realm(&mut self, mut f: ObjectId) -> JsResult<u16> {
        for _ in 0..100_000 {
            match &self.obj(f).kind {
                ObjectKind::Function(c) => return Ok(c.code.realm.get()),
                ObjectKind::Native(n) => return Ok(n.realm),
                ObjectKind::Bound(b) => f = b.target,
                ObjectKind::Proxy { revoked: true, .. } => return self.type_error("Cannot perform operation on a revoked proxy"),
                ObjectKind::Proxy { target, .. } => f = *target,
                _ => return Ok(self.current_realm),
            }
        }
        Ok(self.current_realm)
    }

    /// The intrinsic of realm `r` that `id` is in the running realm (`id`
    /// itself when it is not an intrinsic or `r` is the running realm).
    pub fn same_intrinsic_in(&self, id: ObjectId, r: u16) -> ObjectId {
        if r == self.current_realm {
            return id;
        }
        let other = self.realm_intrinsics(r);
        let cur = &self.intrinsics;
        if let Some(i) = cur.fixed().iter().position(|&x| x == id) {
            return other.fixed()[i];
        }
        if let Some((k, _)) = cur.others.iter().find(|(_, &v)| v == id) {
            if let Some(&o) = other.others.get(k) {
                return o;
            }
        }
        id
    }

    /// GetPrototypeFromConstructor(new.target, default): `new.target.prototype`
    /// when it is an object, otherwise `default` (an intrinsic of the running
    /// realm) as found in new.target's realm. Abrupt completions propagate.
    pub fn proto_from_ctor(&mut self, nt: Value, default: ObjectId) -> JsResult<ObjectId> {
        let V::Object(nt) = nt.v() else { return Ok(default) };
        if let V::Object(p) = self.get(Value::Object(nt), crate::value::names::prototype())?.v() {
            return Ok(p);
        }
        if self.realms.is_empty() {
            return Ok(default);
        }
        let r = self.function_realm(nt)?;
        Ok(self.same_intrinsic_in(default, r))
    }

    /// GC roots of the realms that are not running.
    pub(crate) fn trace_realms(&self, out: &mut Vec<ObjectId>) {
        for rec in self.realms.iter().flatten() {
            rec.trace(out);
        }
    }
}
