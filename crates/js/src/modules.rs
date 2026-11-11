//! ECMAScript modules (ECMA-262 §16.2): module records, host loading,
//! linking, evaluation (including top-level `await`), namespace objects,
//! `import.meta` and dynamic `import()`.
//!
//! # Design
//!
//! A source module is compiled (see [`crate::compiler::compile_module`])
//! into an *environment function* whose scope is the module environment.
//! Running it once at link time initialises TDZ slots, hoists top-level
//! function declarations and returns the module *body* as an async arrow
//! closing over that scope. Import bindings are slots of the module scope
//! that hold a [`ObjectKind::ModuleBinding`] reference to the exporter's
//! slot (or a namespace object); the `GetImport` instruction dereferences
//! it on every read, so imports are live bindings with TDZ checks.
//!
//! Evaluation runs dependencies depth-first, post-order, once each. Every
//! body is async: a body without `await` settles synchronously, so the
//! evaluator checks the returned promise and only defers dependents when a
//! dependency is still pending (top-level `await`). Cycles are tolerated:
//! a module already on the evaluation stack is skipped.
//!
//! The host supplies a loader ([`Realm::set_module_loader`]) that resolves
//! a specifier against the referrer URL, fetches, compiles and caches.

use crate::builtins::promise::perform_then_native;
use crate::bytecode::FunctionCode;
use crate::compiler::{CompiledModule, DEFAULT_EXPORT_BINDING};
use crate::heap::*;
use crate::interop::JsResult;
use crate::realm::{Realm, SYM_TO_STRING_TAG};
use crate::value::*;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Index of a module record in the realm's registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleId(pub u32);

/// `[[Status]]` of a module record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleStatus {
    Unlinked,
    Linking,
    Linked,
    /// On the evaluation stack (synchronous part of `Evaluate()`).
    Evaluating,
    /// Waiting for an async dependency or for its own top-level `await`.
    EvaluatingAsync,
    Evaluated,
    /// Evaluation threw; the error is kept and rethrown to later importers.
    Errored,
}

/// What an import (or indirect export) asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportName {
    /// A named export (`"default"` for default imports).
    Name(String),
    /// The whole namespace (`import * as ns`, `export * as ns from`).
    Namespace,
}

/// `import { import_name as local_name } from "module_request"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportEntry {
    pub module_request: String,
    pub import_name: ImportName,
    pub local_name: String,
}

/// `export { local_name as export_name }` of a binding of this module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalExport {
    pub export_name: String,
    pub local_name: String,
}

/// `export { import_name as export_name } from "module_request"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndirectExport {
    pub export_name: String,
    pub module_request: String,
    pub import_name: ImportName,
}

/// Host hook resolving + fetching + compiling a module:
/// `(realm, referrer_url, specifier) -> module id`. It must return the
/// cached id for URLs it has already loaded.
pub type ModuleLoader = Box<dyn FnMut(&mut Realm, &str, &str) -> Result<ModuleId, Value>>;

/// Host hook called once per module when its `import.meta` object is
/// created (after `url` is set), e.g. to add `resolve`.
pub type ImportMetaHook = Box<dyn FnMut(&mut Realm, ModuleId, ObjectId)>;

/// A Source Text Module Record (or a synthetic one for JSON/CSS modules).
pub struct ModuleRecord {
    pub url: String,
    pub status: ModuleStatus,
    /// `[[RequestedModules]]` in source order, without duplicates.
    pub requested: Vec<String>,
    pub imports: Vec<ImportEntry>,
    pub local_exports: Vec<LocalExport>,
    pub indirect_exports: Vec<IndirectExport>,
    /// `export * from "m"` requests.
    pub star_exports: Vec<String>,
    /// Specifier → loaded module (filled while linking).
    resolved: HashMap<String, ModuleId>,
    /// Environment function (`None` for synthetic modules).
    code: Option<Rc<FunctionCode>>,
    /// Module-scope slot of each top-level binding.
    slots: HashMap<String, u16>,
    /// The module environment (a scope object).
    env: Option<ObjectId>,
    /// Body closure (async arrow), created with the environment.
    body: Option<ObjectId>,
    namespace: Option<ObjectId>,
    meta: Option<ObjectId>,
    error: Option<Value>,
    /// Settles when the module (and its dependencies) finished evaluating.
    completion: Option<ObjectId>,
    /// Async dependencies still pending before the body may run.
    pending_deps: usize,
}

impl ModuleRecord {
    fn new(url: &str) -> ModuleRecord {
        ModuleRecord {
            url: url.to_string(),
            status: ModuleStatus::Unlinked,
            requested: Vec::new(),
            imports: Vec::new(),
            local_exports: Vec::new(),
            indirect_exports: Vec::new(),
            star_exports: Vec::new(),
            resolved: HashMap::new(),
            code: None,
            slots: HashMap::new(),
            env: None,
            body: None,
            namespace: None,
            meta: None,
            error: None,
            completion: None,
            pending_deps: 0,
        }
    }

    /// Fill requests and import/export entries from the module's AST
    /// (ParseModule steps 4–10, including turning re-exported imports into
    /// indirect exports).
    fn collect_entries(&mut self, program: &crate::ast::Program) {
        use crate::ast::{ExportDecl, StmtKind};
        fn request(list: &mut Vec<String>, s: &str) {
            if !list.iter().any(|r| r == s) {
                list.push(s.to_string());
            }
        }
        for s in &program.body {
            match &s.kind {
                StmtKind::Import(d) => {
                    request(&mut self.requested, &d.source);
                    if let Some(l) = &d.default {
                        self.imports.push(ImportEntry { module_request: d.source.clone(), import_name: ImportName::Name("default".into()), local_name: l.clone() });
                    }
                    if let Some(l) = &d.namespace {
                        self.imports.push(ImportEntry { module_request: d.source.clone(), import_name: ImportName::Namespace, local_name: l.clone() });
                    }
                    for (imported, local) in &d.named {
                        self.imports.push(ImportEntry { module_request: d.source.clone(), import_name: ImportName::Name(imported.clone()), local_name: local.clone() });
                    }
                }
                StmtKind::Export(e) => match e {
                    ExportDecl::DefaultExpr(_) => self.local_exports.push(LocalExport { export_name: "default".into(), local_name: DEFAULT_EXPORT_BINDING.into() }),
                    ExportDecl::DefaultDecl(d) => {
                        let name = match &d.kind {
                            StmtKind::FunctionDecl(f) => f.name.as_ref().map(|n| n.to_string()),
                            StmtKind::ClassDecl(c) => c.name.as_ref().map(|n| n.to_string()),
                            _ => None,
                        };
                        let local_name = name.unwrap_or_else(|| DEFAULT_EXPORT_BINDING.into());
                        self.local_exports.push(LocalExport { export_name: "default".into(), local_name });
                    }
                    ExportDecl::Decl(d) => {
                        let mut names = Vec::new();
                        match &d.kind {
                            StmtKind::VarDecl(v) => {
                                for decl in &v.declarations {
                                    crate::compiler::Compiler::collect_pattern_names(&decl.target, &mut names);
                                }
                            }
                            StmtKind::FunctionDecl(f) => names.extend(f.name.as_ref().map(|n| n.to_string())),
                            StmtKind::ClassDecl(c) => names.extend(c.name.as_ref().map(|n| n.to_string())),
                            _ => {}
                        }
                        for n in names {
                            self.local_exports.push(LocalExport { export_name: n.clone(), local_name: n });
                        }
                    }
                    ExportDecl::Named { specifiers, source: Some(src) } => {
                        request(&mut self.requested, src);
                        for (local, exported) in specifiers {
                            self.indirect_exports.push(IndirectExport { export_name: exported.clone(), module_request: src.clone(), import_name: ImportName::Name(local.clone()) });
                        }
                    }
                    ExportDecl::Named { specifiers, source: None } => {
                        for (local, exported) in specifiers {
                            self.local_exports.push(LocalExport { export_name: exported.clone(), local_name: local.clone() });
                        }
                    }
                    ExportDecl::All { source, namespace } => {
                        request(&mut self.requested, source);
                        match namespace {
                            Some(ns) => self.indirect_exports.push(IndirectExport { export_name: ns.clone(), module_request: source.clone(), import_name: ImportName::Namespace }),
                            None => self.star_exports.push(source.clone()),
                        }
                    }
                },
                _ => {}
            }
        }
        // `import { a } from "m"; export { a }` re-exports m's binding
        // directly (namespace imports stay local: the slot holds the object).
        let imports = &self.imports;
        let mut indirect = Vec::new();
        self.local_exports.retain(|le| match imports.iter().find(|ie| ie.local_name == le.local_name) {
            Some(ie) if ie.import_name != ImportName::Namespace => {
                indirect.push(IndirectExport { export_name: le.export_name.clone(), module_request: ie.module_request.clone(), import_name: ie.import_name.clone() });
                false
            }
            _ => true,
        });
        self.indirect_exports.extend(indirect);
    }
}

/// All modules of a realm plus the host hooks.
#[derive(Default)]
pub struct ModuleRegistry {
    records: Vec<ModuleRecord>,
    loader: Option<ModuleLoader>,
    meta_hook: Option<ImportMetaHook>,
    /// Nesting depth of loads performed on behalf of `import()`.
    dynamic_loads: u32,
}

impl ModuleRegistry {
    /// Number of module records.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// GC roots held by module records.
    pub(crate) fn trace(&self, out: &mut Vec<Value>) {
        for r in &self.records {
            for id in [r.env, r.body, r.namespace, r.meta, r.completion].into_iter().flatten() {
                out.push(Value::Object(id));
            }
            if let Some(e) = &r.error {
                out.push(e.clone());
            }
        }
    }
}

/// Result of ResolveExport.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Resolution {
    /// A binding (by local name) in a module's environment.
    Binding(ModuleId, String),
    /// The namespace object of a module.
    Namespace(ModuleId),
}

enum Resolved {
    Found(Resolution),
    NotFound,
    Ambiguous,
}

/// First capture of the running native function.
fn capture(realm: &Realm, i: usize) -> Value {
    realm.current_native().map(|f| realm.native_captures(f)).and_then(|c| c.get(i).cloned()).unwrap_or(Value::Undefined)
}

fn captured_module(realm: &Realm) -> ModuleId {
    ModuleId(capture(realm, 0).as_number().unwrap_or(0.0) as u32)
}

fn first_arg(args: &[Value]) -> Value {
    args.first().cloned().unwrap_or(Value::Undefined)
}

impl Realm {
    fn rec(&self, id: ModuleId) -> &ModuleRecord {
        &self.modules.records[id.0 as usize]
    }

    fn rec_mut(&mut self, id: ModuleId) -> &mut ModuleRecord {
        &mut self.modules.records[id.0 as usize]
    }

    // ----------------------------------------------------------- creation

    /// Parse and compile a module. The record starts `Unlinked`.
    pub fn compile_module(&mut self, source: &str, url: &str) -> JsResult<ModuleId> {
        let program = match crate::parser::parse_program(source, true) {
            Ok(p) => p,
            Err(e) => {
                let msg = format!("{} ({}:{}:{})", e.message, url, e.line, e.column);
                return Err(self.new_error("SyntaxError", &msg));
            }
        };
        let id = ModuleId(self.modules.records.len() as u32);
        let CompiledModule { code, slots } = match crate::compiler::compile_module(&program, url, source, id.0) {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("{} ({}:{}:{})", e.message, url, e.pos.line, e.pos.column);
                return Err(self.new_error("SyntaxError", &msg));
            }
        };
        let mut rec = ModuleRecord::new(url);
        rec.code = Some(code);
        rec.slots = slots;
        rec.collect_entries(&program);
        self.modules.records.push(rec);
        Ok(id)
    }

    /// Create a synthetic module whose exports are fixed values (JSON
    /// modules export `default`; CSS module stubs likewise).
    pub fn create_synthetic_module(&mut self, url: &str, exports: Vec<(String, Value)>) -> ModuleId {
        let mut rec = ModuleRecord::new(url);
        let mut values = Vec::with_capacity(exports.len());
        for (i, (name, value)) in exports.into_iter().enumerate() {
            rec.slots.insert(name.clone(), i as u16);
            rec.local_exports.push(LocalExport { export_name: name.clone(), local_name: name });
            values.push(value);
        }
        let env = self.alloc(Object::new(None, ObjectKind::Scope(Box::new(ScopeData { slots: values, parent: None, names: None }))));
        rec.env = Some(env);
        let id = ModuleId(self.modules.records.len() as u32);
        self.modules.records.push(rec);
        id
    }

    /// Install the host module loader used for static and dynamic imports.
    pub fn set_module_loader(&mut self, loader: ModuleLoader) {
        self.modules.loader = Some(loader);
    }

    /// Install a hook that decorates `import.meta` objects.
    pub fn set_import_meta_hook(&mut self, hook: ImportMetaHook) {
        self.modules.meta_hook = Some(hook);
    }

    /// Whether the loader is currently running for a dynamic `import()`
    /// (lets hosts word fetch errors like browsers do).
    pub fn is_dynamic_module_load(&self) -> bool {
        self.modules.dynamic_loads > 0
    }

    pub fn module_url(&self, id: ModuleId) -> &str {
        &self.rec(id).url
    }

    pub fn module_status(&self, id: ModuleId) -> ModuleStatus {
        self.rec(id).status
    }

    /// The evaluation error of an `Errored` module.
    pub fn module_error(&self, id: ModuleId) -> Option<Value> {
        self.rec(id).error.clone()
    }

    /// The module's requested specifiers, in source order.
    pub fn module_requests(&self, id: ModuleId) -> &[String] {
        &self.rec(id).requested
    }

    /// Call the host loader for `specifier` imported from `referrer`.
    pub fn load_imported_module(&mut self, referrer: &str, specifier: &str) -> JsResult<ModuleId> {
        let Some(mut loader) = self.modules.loader.take() else {
            return Err(self.new_error("TypeError", &format!("Cannot import module '{specifier}': no module loader is installed")));
        };
        let r = loader(self, referrer, specifier);
        if self.modules.loader.is_none() {
            self.modules.loader = Some(loader);
        }
        let id = r?;
        if id.0 as usize >= self.modules.records.len() {
            return Err(self.new_error("TypeError", &format!("Module loader returned an invalid module for '{specifier}'")));
        }
        Ok(id)
    }

    // ------------------------------------------------------------ linking

    /// Link a module graph: load every dependency through the loader,
    /// create module environments (hoisting functions), resolve exports
    /// and bind imports.
    pub fn link_module(&mut self, id: ModuleId) -> JsResult<()> {
        let mut order = Vec::new();
        let mut visited = HashSet::new();
        self.load_graph(id, &mut visited, &mut order)?;
        let fresh: Vec<ModuleId> = order.into_iter().filter(|m| self.rec(*m).status == ModuleStatus::Unlinked).collect();
        for &m in &fresh {
            self.rec_mut(m).status = ModuleStatus::Linking;
        }
        let result = self.link_fresh(&fresh);
        let status = if result.is_ok() { ModuleStatus::Linked } else { ModuleStatus::Unlinked };
        for &m in &fresh {
            self.rec_mut(m).status = status;
        }
        result
    }

    fn link_fresh(&mut self, fresh: &[ModuleId]) -> JsResult<()> {
        for &m in fresh {
            self.init_environment(m)?;
        }
        for &m in fresh {
            self.bind_imports(m)?;
        }
        Ok(())
    }

    /// Depth-first load of every module reachable from `id`; `order` gets
    /// the post-order.
    fn load_graph(&mut self, id: ModuleId, visited: &mut HashSet<ModuleId>, order: &mut Vec<ModuleId>) -> JsResult<()> {
        if !visited.insert(id) || self.rec(id).status != ModuleStatus::Unlinked {
            return Ok(());
        }
        let url = self.rec(id).url.clone();
        for spec in self.rec(id).requested.clone() {
            let dep = match self.rec(id).resolved.get(&spec) {
                Some(d) => *d,
                None => {
                    let d = self.load_imported_module(&url, &spec)?;
                    self.rec_mut(id).resolved.insert(spec, d);
                    d
                }
            };
            self.load_graph(dep, visited, order)?;
        }
        order.push(id);
        Ok(())
    }

    /// InitializeEnvironment (environment part): run the environment
    /// function once to get the module scope and the body closure.
    fn init_environment(&mut self, id: ModuleId) -> JsResult<()> {
        if self.rec(id).env.is_some() {
            return Ok(());
        }
        let Some(code) = self.rec(id).code.clone() else { return Ok(()) };
        let body = self.run_script(code)?;
        let Value::Object(body) = body else {
            return Err(self.new_error("TypeError", "module environment did not produce a body"));
        };
        let env = match &self.obj(body).kind {
            ObjectKind::Function(c) => c.scope,
            _ => None,
        };
        let rec = self.rec_mut(id);
        rec.body = Some(body);
        rec.env = env;
        Ok(())
    }

    fn missing_export_error(&mut self, specifier: &str, name: &str, ambiguous: bool) -> Value {
        let msg = if ambiguous {
            format!("The requested module '{specifier}' contains conflicting star exports for name '{name}'")
        } else {
            format!("The requested module '{specifier}' does not provide an export named '{name}'")
        };
        self.new_error("SyntaxError", &msg)
    }

    /// Write every import binding of `id` and validate indirect exports.
    fn bind_imports(&mut self, id: ModuleId) -> JsResult<()> {
        for ie in self.rec(id).imports.clone() {
            let Some(&dep) = self.rec(id).resolved.get(&ie.module_request) else { continue };
            let value = match &ie.import_name {
                ImportName::Namespace => Value::Object(self.namespace_object(dep)?),
                ImportName::Name(n) => match self.resolve_export(dep, n, &mut Vec::new()) {
                    Resolved::Found(r) => self.resolution_value(r)?,
                    Resolved::NotFound => return Err(self.missing_export_error(&ie.module_request, n, false)),
                    Resolved::Ambiguous => return Err(self.missing_export_error(&ie.module_request, n, true)),
                },
            };
            let (env, slot) = (self.rec(id).env, self.rec(id).slots.get(&ie.local_name).copied());
            if let (Some(env), Some(slot)) = (env, slot) {
                if let ObjectKind::Scope(s) = &mut self.obj_mut(env).kind {
                    s.slots[slot as usize] = value;
                }
            }
        }
        for ie in self.rec(id).indirect_exports.clone() {
            match self.resolve_export(id, &ie.export_name, &mut Vec::new()) {
                Resolved::Found(_) => {}
                Resolved::NotFound | Resolved::Ambiguous => {
                    let name = match &ie.import_name {
                        ImportName::Name(n) => n.clone(),
                        ImportName::Namespace => "*".to_string(),
                    };
                    let ambiguous = matches!(self.resolve_export(id, &ie.export_name, &mut Vec::new()), Resolved::Ambiguous);
                    return Err(self.missing_export_error(&ie.module_request, &name, ambiguous));
                }
            }
        }
        Ok(())
    }

    /// ResolveExport (§16.2.1.6.3) with a resolve set for cycles.
    fn resolve_export(&self, id: ModuleId, name: &str, set: &mut Vec<(ModuleId, String)>) -> Resolved {
        if set.iter().any(|(m, n)| *m == id && n == name) {
            return Resolved::NotFound; // circular import request
        }
        set.push((id, name.to_string()));
        let rec = self.rec(id);
        if let Some(le) = rec.local_exports.iter().find(|e| e.export_name == name) {
            return Resolved::Found(Resolution::Binding(id, le.local_name.clone()));
        }
        if let Some(ie) = rec.indirect_exports.iter().find(|e| e.export_name == name) {
            let Some(&dep) = rec.resolved.get(&ie.module_request) else { return Resolved::NotFound };
            return match &ie.import_name {
                ImportName::Namespace => Resolved::Found(Resolution::Namespace(dep)),
                ImportName::Name(n) => self.resolve_export(dep, n, set),
            };
        }
        if name == "default" {
            return Resolved::NotFound;
        }
        let mut star: Option<Resolution> = None;
        for req in &rec.star_exports {
            let Some(&dep) = rec.resolved.get(req) else { continue };
            match self.resolve_export(dep, name, set) {
                Resolved::Ambiguous => return Resolved::Ambiguous,
                Resolved::NotFound => {}
                Resolved::Found(r) => match &star {
                    None => star = Some(r),
                    Some(s) if *s != r => return Resolved::Ambiguous,
                    Some(_) => {}
                },
            }
        }
        star.map(Resolved::Found).unwrap_or(Resolved::NotFound)
    }

    /// GetExportedNames (§16.2.1.6.2).
    fn exported_names(&self, id: ModuleId, visited: &mut Vec<ModuleId>) -> Vec<String> {
        if visited.contains(&id) {
            return Vec::new();
        }
        visited.push(id);
        let rec = self.rec(id);
        let mut names: Vec<String> = rec.local_exports.iter().map(|e| e.export_name.clone()).chain(rec.indirect_exports.iter().map(|e| e.export_name.clone())).collect();
        for req in &rec.star_exports {
            let Some(&dep) = rec.resolved.get(req) else { continue };
            for n in self.exported_names(dep, visited) {
                if n != "default" && !names.contains(&n) {
                    names.push(n);
                }
            }
        }
        names
    }

    /// The value stored in an import slot for a resolution: a binding
    /// reference (live) or a namespace object.
    fn resolution_value(&mut self, r: Resolution) -> JsResult<Value> {
        match r {
            Resolution::Namespace(m) => Ok(Value::Object(self.namespace_object(m)?)),
            Resolution::Binding(m, local) => {
                if self.rec(m).env.is_none() {
                    self.init_environment(m)?;
                }
                let (env, slot) = (self.rec(m).env, self.rec(m).slots.get(&local).copied());
                let (Some(scope), Some(slot)) = (env, slot) else {
                    let url = self.rec(m).url.clone();
                    return Err(self.new_error("ReferenceError", &format!("Module '{url}' has no binding '{local}'")));
                };
                let name = if local == DEFAULT_EXPORT_BINDING { JsStr::new("default") } else { JsStr::new(&local) };
                Ok(Value::Object(self.alloc(Object::new(None, ObjectKind::ModuleBinding { scope, slot: slot as u32, name }))))
            }
        }
    }

    /// Read an import slot: dereference a module binding reference (TDZ
    /// checked); other values (namespace objects) are returned as is.
    pub(crate) fn read_import_slot(&mut self, v: Value) -> JsResult<Value> {
        let Value::Object(id) = &v else { return Ok(v) };
        let (scope, slot, name) = match &self.obj(*id).kind {
            ObjectKind::ModuleBinding { scope, slot, name } => (*scope, *slot, name.clone()),
            _ => return Ok(v),
        };
        let target = match &self.obj(scope).kind {
            ObjectKind::Scope(s) => s.slots.get(slot as usize).cloned().unwrap_or(Value::Undefined),
            _ => Value::Undefined,
        };
        if target.is_empty() {
            return Err(self.new_error("ReferenceError", &format!("Cannot access '{}' before initialization", name)));
        }
        Ok(target)
    }

    // ---------------------------------------------------------- namespace

    /// GetModuleNamespace: the module namespace exotic object (an
    /// approximation: a non-extensible null-prototype object whose
    /// properties are enumerable getters over live bindings, sorted by
    /// name, with `Symbol.toStringTag` = "Module").
    pub fn module_namespace(&mut self, id: ModuleId) -> JsResult<Value> {
        Ok(Value::Object(self.namespace_object(id)?))
    }

    fn namespace_object(&mut self, id: ModuleId) -> JsResult<ObjectId> {
        if let Some(ns) = self.rec(id).namespace {
            return Ok(ns);
        }
        let ns = self.new_object_with_proto(None);
        self.rec_mut(id).namespace = Some(ns);
        let mut names = self.exported_names(id, &mut Vec::new());
        names.sort();
        names.dedup();
        for name in names {
            let Resolved::Found(r) = self.resolve_export(id, &name, &mut Vec::new()) else { continue };
            let target = self.resolution_value(r)?;
            let getter = self.new_native_function_full(&format!("get {name}"), 0, namespace_get, None, vec![target], false);
            self.define_accessor_values(ns, name.as_str(), Some(Value::Object(getter)), None, ENUMERABLE);
        }
        self.define_property(ns, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("Module"), 0);
        self.obj_mut(ns).extensible = false;
        Ok(ns)
    }

    // ------------------------------------------------------- import.meta

    /// The (lazily created) `import.meta` object of a module.
    pub fn import_meta(&mut self, id: ModuleId) -> ObjectId {
        if let Some(m) = self.rec(id).meta {
            return m;
        }
        let meta = self.new_object_with_proto(None);
        let url = self.rec(id).url.clone();
        self.define_property(meta, "url", Value::str(&url), ALL);
        self.rec_mut(id).meta = Some(meta);
        if let Some(mut hook) = self.modules.meta_hook.take() {
            hook(self, id, meta);
            if self.modules.meta_hook.is_none() {
                self.modules.meta_hook = Some(hook);
            }
        }
        meta
    }

    // --------------------------------------------------------- evaluation

    /// Evaluate a linked module graph. Returns a promise that settles when
    /// the module and all its dependencies finished (top-level `await`
    /// included); it is rejected with the evaluation error on failure.
    pub fn evaluate_module(&mut self, id: ModuleId) -> JsResult<Value> {
        match self.rec(id).status {
            ModuleStatus::Unlinked | ModuleStatus::Linking => {
                let url = self.rec(id).url.clone();
                return Err(self.new_error("TypeError", &format!("Module '{url}' must be linked before evaluation")));
            }
            _ => {}
        }
        // Errors are recorded on the module and its completion promise.
        let _ = self.evaluate_inner(id);
        let p = self.completion_promise(id);
        Ok(Value::Object(p))
    }

    /// The completion promise, created on demand (marked handled: hosts
    /// report module errors themselves).
    fn completion_promise(&mut self, id: ModuleId) -> ObjectId {
        if let Some(p) = self.rec(id).completion {
            return p;
        }
        let p = self.new_promise_object();
        if let ObjectKind::Promise(pd) = &mut self.obj_mut(p).kind {
            pd.handled = true;
        }
        self.rec_mut(id).completion = Some(p);
        p
    }

    fn evaluate_inner(&mut self, id: ModuleId) -> JsResult<()> {
        match self.rec(id).status {
            ModuleStatus::Evaluating | ModuleStatus::EvaluatingAsync | ModuleStatus::Evaluated => return Ok(()),
            ModuleStatus::Errored => return Err(self.rec(id).error.clone().unwrap_or(Value::Undefined)),
            _ => {}
        }
        self.rec_mut(id).status = ModuleStatus::Evaluating;
        self.completion_promise(id);
        let mut deps: Vec<ModuleId> = Vec::new();
        for spec in &self.rec(id).requested {
            if let Some(&d) = self.rec(id).resolved.get(spec) {
                if !deps.contains(&d) {
                    deps.push(d);
                }
            }
        }
        for &dep in &deps {
            if let Err(e) = self.evaluate_inner(dep) {
                self.fail_module(id, e.clone());
                return Err(e);
            }
        }
        let pending: Vec<ModuleId> = deps.into_iter().filter(|d| self.rec(*d).status == ModuleStatus::EvaluatingAsync).collect();
        if pending.is_empty() {
            return self.run_module_body(id);
        }
        let rec = self.rec_mut(id);
        rec.status = ModuleStatus::EvaluatingAsync;
        rec.pending_deps = pending.len();
        for dep in pending {
            let p = self.completion_promise(dep);
            perform_then_native(self, p, on_dependency_fulfilled, on_dependency_rejected, Value::Number(id.0 as f64));
        }
        Ok(())
    }

    /// Execute the module body (dependencies are done).
    fn run_module_body(&mut self, id: ModuleId) -> JsResult<()> {
        let Some(body) = self.rec(id).body else {
            self.finish_module(id);
            return Ok(());
        };
        let promise = match self.call(Value::Object(body), Value::Undefined, &[]) {
            Ok(Value::Object(p)) => p,
            Ok(_) => {
                self.finish_module(id);
                return Ok(());
            }
            Err(e) => {
                self.fail_module(id, e.clone());
                return Err(e);
            }
        };
        let (state, value) = match &self.obj(promise).kind {
            ObjectKind::Promise(pd) => (pd.state, pd.value.clone()),
            _ => (PromiseState::Fulfilled, Value::Undefined),
        };
        match state {
            PromiseState::Fulfilled => {
                self.finish_module(id);
                Ok(())
            }
            PromiseState::Rejected => {
                self.forget_unhandled(&value);
                self.fail_module(id, value.clone());
                Err(value)
            }
            PromiseState::Pending => {
                self.rec_mut(id).status = ModuleStatus::EvaluatingAsync;
                perform_then_native(self, promise, on_body_fulfilled, on_body_rejected, Value::Number(id.0 as f64));
                Ok(())
            }
        }
    }

    fn finish_module(&mut self, id: ModuleId) {
        self.rec_mut(id).status = ModuleStatus::Evaluated;
        let p = self.completion_promise(id);
        self.resolve_promise(p, Value::Undefined);
    }

    fn fail_module(&mut self, id: ModuleId, error: Value) {
        if self.rec(id).status == ModuleStatus::Errored {
            return;
        }
        let rec = self.rec_mut(id);
        rec.status = ModuleStatus::Errored;
        rec.error = Some(error.clone());
        let p = self.completion_promise(id);
        self.reject_promise(p, error);
    }

    /// Drop a rejection the module machinery handles from the unhandled list.
    fn forget_unhandled(&mut self, reason: &Value) {
        let pos = self.unhandled_rejections.iter().rposition(|v| match (v, reason) {
            (Value::Object(a), Value::Object(b)) => a == b,
            _ => !reason.is_object(),
        });
        if let Some(pos) = pos {
            self.unhandled_rejections.remove(pos);
        }
    }

    /// Compile, link and evaluate a top-level module (a `<script
    /// type=module>`). Link errors and synchronous evaluation errors are
    /// returned; errors after a top-level `await` are reported to the
    /// console. Returns the evaluation promise.
    pub fn run_module(&mut self, source: &str, url: &str) -> JsResult<Value> {
        let id = self.compile_module(source, url)?;
        self.link_module(id)?;
        let p = self.evaluate_module(id)?;
        if let Value::Object(pid) = &p {
            let (state, value) = match &self.obj(*pid).kind {
                ObjectKind::Promise(pd) => (pd.state, pd.value.clone()),
                _ => (PromiseState::Fulfilled, Value::Undefined),
            };
            match state {
                PromiseState::Rejected => return Err(value),
                PromiseState::Pending => perform_then_native(self, *pid, ignore_settlement, report_module_error, Value::str(url)),
                PromiseState::Fulfilled => {}
            }
        }
        Ok(p)
    }

    // ----------------------------------------------------- dynamic import

    /// `import(specifier)`: returns a promise for the namespace. Loading
    /// happens in a job so the importing code continues first.
    pub(crate) fn start_dynamic_import(&mut self, specifier: Value, referrer: &str) -> ObjectId {
        let promise = self.new_promise_object();
        match self.to_string(specifier) {
            Ok(s) => {
                let job = self.new_native_function_full("", 0, dynamic_import_job, None, vec![Value::Object(promise), Value::String(s), Value::str(referrer)], false);
                self.enqueue_callback(Value::Object(job), Value::Undefined, Vec::new());
            }
            Err(e) => self.reject_promise(promise, e),
        }
        promise
    }

    /// Load + link + evaluate for `import()`; returns the module and its
    /// completion promise.
    fn dynamic_import_load(&mut self, referrer: &str, specifier: &str) -> JsResult<(ModuleId, ObjectId)> {
        self.modules.dynamic_loads += 1;
        let r = self.load_imported_module(referrer, specifier).and_then(|id| self.link_module(id).map(|_| id));
        self.modules.dynamic_loads -= 1;
        let id = r?;
        let p = self.evaluate_module(id)?;
        Ok((id, p.as_object().unwrap_or_else(|| self.completion_promise(id))))
    }
}

// ------------------------------------------------------------- natives

/// Namespace property getter: capture 0 is a binding reference or value.
fn namespace_get(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    let target = capture(realm, 0);
    realm.read_import_slot(target)
}

fn on_dependency_fulfilled(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = captured_module(realm);
    let rec = realm.rec_mut(id);
    if rec.status != ModuleStatus::EvaluatingAsync || rec.pending_deps == 0 {
        return Ok(Value::Undefined);
    }
    rec.pending_deps -= 1;
    if rec.pending_deps == 0 {
        // Errors are recorded on the module's completion promise.
        let _ = realm.run_module_body(id);
    }
    Ok(Value::Undefined)
}

fn on_dependency_rejected(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = captured_module(realm);
    realm.fail_module(id, first_arg(args));
    Ok(Value::Undefined)
}

fn on_body_fulfilled(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = captured_module(realm);
    if realm.rec(id).status == ModuleStatus::EvaluatingAsync {
        realm.finish_module(id);
    }
    Ok(Value::Undefined)
}

fn on_body_rejected(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = captured_module(realm);
    realm.fail_module(id, first_arg(args));
    Ok(Value::Undefined)
}

fn ignore_settlement(_realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(Value::Undefined)
}

/// Report a top-level module whose async evaluation failed.
fn report_module_error(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let url = capture(realm, 0);
    let url = url.as_str().map(|s| s.to_rust_string()).unwrap_or_default();
    let msg = realm.error_to_string(&first_arg(args));
    realm.console_emit("error", format!("Uncaught (in module) {msg} ({url})"));
    Ok(Value::Undefined)
}

/// Job running a dynamic import: captures (promise, specifier, referrer).
fn dynamic_import_job(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    let Some(promise) = capture(realm, 0).as_object() else { return Ok(Value::Undefined) };
    let specifier = capture(realm, 1).as_str().map(|s| s.to_rust_string()).unwrap_or_default();
    let referrer = capture(realm, 2).as_str().map(|s| s.to_rust_string()).unwrap_or_default();
    match realm.dynamic_import_load(&referrer, &specifier) {
        Ok((id, completion)) => {
            let data = realm.new_array_value(vec![Value::Object(promise), Value::Number(id.0 as f64)]);
            perform_then_native(realm, completion, on_dynamic_fulfilled, on_dynamic_rejected, data);
        }
        Err(e) => realm.reject_promise(promise, e),
    }
    Ok(Value::Undefined)
}

/// The (promise, module id) pair captured by dynamic import reactions.
fn dynamic_data(realm: &mut Realm) -> Option<(ObjectId, ModuleId)> {
    let data = capture(realm, 0);
    let promise = realm.get(data.clone(), PropertyKey::index(0)).ok()?.as_object()?;
    let id = realm.get(data, PropertyKey::index(1)).ok()?.as_number()?;
    Some((promise, ModuleId(id as u32)))
}

fn on_dynamic_fulfilled(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    if let Some((promise, id)) = dynamic_data(realm) {
        match realm.module_namespace(id) {
            Ok(ns) => realm.resolve_promise(promise, ns),
            Err(e) => realm.reject_promise(promise, e),
        }
    }
    Ok(Value::Undefined)
}

fn on_dynamic_rejected(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if let Some((promise, _)) = dynamic_data(realm) {
        realm.reject_promise(promise, first_arg(args));
    }
    Ok(Value::Undefined)
}
