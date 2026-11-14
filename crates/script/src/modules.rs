//! Module scripts: the module loader (specifier resolution, fetching,
//! per-host caching), import maps, JSON / CSS modules and
//! `import.meta.resolve`.
//!
//! The loader runs synchronously inside an entry point: it resolves a
//! specifier against the referrer (through the import map for bare
//! specifiers), fetches with [`HostEnv::fetch`](crate::HostEnv::fetch) and
//! compiles. Failures are logged with the specifier and URL.

use crate::env::with_env;
use rabbithole_js::V;
use crate::host::{self, state};
use rabbithole_dom::{Document, NodeId};
use rabbithole_js::{JsResult, ModuleId, ObjectId, PropertyKey, Realm, Value, HIDDEN};
use rabbithole_net::Url;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// A parsed import map. Keys and addresses are kept as written and
/// resolved against the document base URL when used.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ImportMap {
    /// Top-level `imports`: (specifier key, address).
    pub imports: Vec<(String, String)>,
    /// `scopes`: (scope prefix, its specifier map).
    pub scopes: Vec<(String, Vec<(String, String)>)>,
}

impl ImportMap {
    /// Merge `other` into `self`; existing entries win (as in the HTML
    /// "merge existing and new import maps" algorithm).
    pub fn merge(&mut self, other: ImportMap) {
        for (k, v) in other.imports {
            if !self.imports.iter().any(|(e, _)| *e == k) {
                self.imports.push((k, v));
            }
        }
        for (scope, entries) in other.scopes {
            match self.scopes.iter_mut().find(|(s, _)| *s == scope) {
                Some((_, existing)) => {
                    for (k, v) in entries {
                        if !existing.iter().any(|(e, _)| *e == k) {
                            existing.push((k, v));
                        }
                    }
                }
                None => self.scopes.push((scope, entries)),
            }
        }
    }
}

/// Per-host module state (kept in [`crate::host::HostState`]).
#[derive(Default)]
pub struct ModuleState {
    /// Fetched modules by URL.
    pub cache: HashMap<String, ModuleId>,
    pub import_map: ImportMap,
    /// `<script type="importmap">` elements already merged.
    pub import_map_nodes: HashSet<NodeId>,
    /// Whether the document has been scanned for import maps at least once.
    pub scanned: bool,
}

/// Install the module loader and the `import.meta` hook on a realm.
pub(crate) fn install(realm: &mut Realm) {
    realm.set_module_loader(Box::new(load_module));
    realm.set_import_meta_hook(Box::new(decorate_import_meta));
}

// ------------------------------------------------------------- import maps

/// Parse import map JSON (`{"imports": {...}, "scopes": {...}}`) with the
/// realm's `JSON.parse`. Invalid entries (non-string addresses) are skipped.
pub(crate) fn parse_import_map(realm: &mut Realm, json: &str) -> JsResult<ImportMap> {
    let global = Value::Object(realm.global());
    let json_obj = realm.get_str(global, "JSON")?;
    let parse = realm.get_str(json_obj.clone(), "parse")?;
    let parsed = realm.call(parse, json_obj, &[Value::str(json)])?;
    let mut map = ImportMap::default();
    if !parsed.is_object() {
        return Err(realm.throw_type_error("Import map must be a JSON object"));
    }
    let imports = realm.get_str(parsed.clone(), "imports")?;
    map.imports = specifier_map(realm, &imports)?;
    let scopes = realm.get_str(parsed, "scopes")?;
    if let V::Object(s) = scopes.v() {
        for key in string_keys(realm, s) {
            let entries = realm.get_str(scopes.clone(), &key)?;
            let entries = specifier_map(realm, &entries)?;
            map.scopes.push((key, entries));
        }
    }
    Ok(map)
}

fn string_keys(realm: &Realm, obj: ObjectId) -> Vec<String> {
    realm.obj(obj).props.keys().into_iter().filter_map(|k| k.as_str().map(|s| s.to_rust_string())).collect()
}

fn specifier_map(realm: &mut Realm, v: &Value) -> JsResult<Vec<(String, String)>> {
    let V::Object(o) = v.v() else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for key in string_keys(realm, o) {
        if let V::String(addr) = (realm.get_str(v.clone(), &key)?).v() {
            out.push((key, addr.to_rust_string()));
        }
    }
    Ok(out)
}

/// Merge every `<script type="importmap">` of the document not seen yet.
pub(crate) fn collect_import_maps(realm: &mut Realm) {
    let found: Vec<(NodeId, String)> = with_env(|env| {
        let doc: &Document = env.doc();
        doc.descendants(Document::ROOT)
            .filter(|&n| doc.is_element(n, "script") && doc.attr(n, "type").map(|t| t.trim().eq_ignore_ascii_case("importmap")).unwrap_or(false))
            .map(|n| (n, doc.text_content(n)))
            .collect()
    });
    state().borrow_mut().modules.scanned = true;
    for (node, text) in found {
        if !state().borrow_mut().modules.import_map_nodes.insert(node) {
            continue;
        }
        match parse_import_map(realm, &text) {
            Ok(map) => state().borrow_mut().modules.import_map.merge(map),
            Err(e) => {
                let msg = realm.error_to_string(&e);
                host::log("error", &format!("Invalid import map: {msg}"));
            }
        }
    }
}

// -------------------------------------------------------------- resolution

/// Parse `spec` as a URL-like specifier: a relative reference starting
/// with `/`, `./` or `../` (resolved against `base`) or an absolute URL.
fn parse_url_like(spec: &str, base: &Url) -> Option<Url> {
    if spec.starts_with('/') || spec.starts_with("./") || spec.starts_with("../") {
        return base.join(spec);
    }
    let colon = spec.find(':')?;
    let scheme = &spec[..colon];
    let valid = !scheme.is_empty() && scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if valid {
        Url::parse_absolute(spec)
    } else {
        None
    }
}

/// Resolve `normalized` through one specifier map (exact keys, then the
/// longest matching prefix key ending in `/`).
fn resolve_imports_match(normalized: &str, is_url: bool, map: &[(String, String)], base: &Url) -> Option<Result<Url, String>> {
    let mut entries: Vec<(String, &String)> = map
        .iter()
        .map(|(k, v)| {
            // Keys that are URL-like are normalised to absolute URLs.
            let key = parse_url_like(k, base).map(|u| u.to_string()).unwrap_or_else(|| k.clone());
            (key, v)
        })
        .collect();
    entries.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    for (key, addr) in entries {
        if key == normalized {
            return Some(parse_url_like(addr, base).ok_or_else(|| format!("Import map address \"{addr}\" for \"{key}\" is not a valid URL")));
        }
        if key.ends_with('/') && normalized.starts_with(&key) && (!is_url || key.contains(':')) {
            if !addr.ends_with('/') {
                return Some(Err(format!("Import map address \"{addr}\" for prefix \"{key}\" must end with \"/\"")));
            }
            let after = &normalized[key.len()..];
            let resolved = parse_url_like(addr, base).and_then(|a| a.join(after));
            return Some(resolved.ok_or_else(|| format!("Failed to resolve \"{normalized}\" through import map prefix \"{key}\"")));
        }
    }
    None
}

/// HTML "resolve a module specifier": import map scopes (for the
/// referrer), top-level imports, then URL-like specifiers.
pub(crate) fn resolve_specifier(map: &ImportMap, specifier: &str, referrer: &Url, doc_base: &Url) -> Result<Url, String> {
    let as_url = parse_url_like(specifier, referrer);
    let normalized = as_url.as_ref().map(|u| u.to_string()).unwrap_or_else(|| specifier.to_string());
    let referrer_str = referrer.to_string();
    let mut scopes: Vec<(String, &Vec<(String, String)>)> = map.scopes.iter().map(|(s, e)| (parse_url_like(s, doc_base).map(|u| u.to_string()).unwrap_or_else(|| s.clone()), e)).collect();
    scopes.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    for (prefix, entries) in scopes {
        if prefix == referrer_str || (prefix.ends_with('/') && referrer_str.starts_with(&prefix)) {
            if let Some(r) = resolve_imports_match(&normalized, as_url.is_some(), entries, doc_base) {
                return r;
            }
        }
    }
    if let Some(r) = resolve_imports_match(&normalized, as_url.is_some(), &map.imports, doc_base) {
        return r;
    }
    as_url.ok_or_else(|| format!("Failed to resolve module specifier \"{specifier}\". Relative references must start with either \"/\", \"./\", or \"../\"."))
}

/// The base for resolving imports from `referrer` (a module or script URL;
/// inline scripts use the document URL, which maps to the base URL).
fn referrer_base(referrer: &str) -> Url {
    let (doc_url, base) = with_env(|env| (env.document_url(), env.base_url()));
    // The engine names inline scripts `<document URL>#inline`.
    let referrer = referrer.strip_suffix("#inline").unwrap_or(referrer);
    match Url::parse_absolute(referrer) {
        Some(u) if u.without_fragment().to_string() != doc_url.without_fragment().to_string() && (u.is_http() || !u.host.is_empty() || u.scheme == "file") => u,
        _ => base,
    }
}

fn resolve_from(referrer: &str, specifier: &str) -> Result<Url, String> {
    let base = referrer_base(referrer);
    let doc_base = with_env(|env| env.base_url());
    let st = state();
    let st = st.borrow();
    resolve_specifier(&st.modules.import_map, specifier, &base, &doc_base)
}

// ------------------------------------------------------------------ loader

/// What a fetched module is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModuleKind {
    JavaScript,
    Json,
    Css,
}

fn module_kind(url: &Url, content_type: &str) -> ModuleKind {
    let path = url.path.to_ascii_lowercase();
    if content_type.ends_with("json") || path.ends_with(".json") {
        ModuleKind::Json
    } else if content_type == "text/css" || path.ends_with(".css") {
        ModuleKind::Css
    } else {
        ModuleKind::JavaScript
    }
}

/// The realm's module loader: resolve, fetch (synchronously), compile and
/// cache by URL.
fn load_module(realm: &mut Realm, referrer: &str, specifier: &str) -> Result<ModuleId, Value> {
    if !state().borrow().modules.scanned {
        collect_import_maps(realm);
    }
    let url = match resolve_from(referrer, specifier) {
        Ok(u) => u,
        Err(msg) => {
            host::log("error", &format!("Failed to load module \"{specifier}\" imported from {referrer}: TypeError: {msg}"));
            return Err(realm.throw_type_error(&msg));
        }
    };
    let key = url.to_string();
    if let Some(id) = state().borrow().modules.cache.get(&key) {
        return Ok(*id);
    }
    // The host accounts for synchronous fetch time (EngineEnv::fetch uses
    // exclude_from_watchdog). Compilation is accounted separately below;
    // restarting the deadline here would reset the source task's CPU clock,
    // pending host credit and regexp expiration during a cross-realm loan.
    // blob: URLs (URL.createObjectURL) are served from the page's store.
    let resp = if url.scheme == "blob" {
        crate::fetch::blob_url_response(&key).ok_or_else(|| format!("no blob for {key}"))
    } else {
        with_env(|env| env.fetch(rabbithole_net::Request::get(url.clone()).with_dest(rabbithole_net::Destination::Script).with_referer(env.document_url())))
    };
    let (text, kind) = match resp {
        Ok(r) if r.status < 400 => (rabbithole_net::decode_text(&r.body, r.charset.as_deref()), module_kind(&url, &r.content_type)),
        other => {
            let reason = match other {
                Ok(r) => format!("HTTP {}", r.status),
                Err(e) => e,
            };
            let msg = if realm.is_dynamic_module_load() { format!("Failed to fetch dynamically imported module: {key}") } else { format!("Failed to load module script: {key}") };
            host::log("error", &format!("Failed to load module \"{specifier}\" ({key}) imported from {referrer}: {reason}"));
            return Err(realm.throw_type_error(&msg));
        }
    };
    let id = match kind {
        ModuleKind::JavaScript => {
            let started = std::time::Instant::now();
            let compiled = realm.compile_module(&text, &key);
            if realm.deadline.is_some() {
                rabbithole_js::exclude_from_watchdog(started.elapsed());
            }
            match compiled {
                Ok(id) => id,
                Err(e) => {
                    let msg = realm.error_to_string(&e);
                    host::log("error", &format!("Failed to compile module \"{specifier}\" ({key}): {msg}"));
                    return Err(e);
                }
            }
        },
        ModuleKind::Json => {
            let value = parse_json(realm, &text).inspect_err(|e| {
                let msg = realm.error_to_string(e);
                host::log("error", &format!("Failed to parse JSON module \"{specifier}\" ({key}): {msg}"));
            })?;
            realm.create_synthetic_module(&key, vec![("default".into(), value)])
        }
        ModuleKind::Css => {
            // CSS module scripts are a stub: the default export is an
            // empty object instead of a constructed CSSStyleSheet.
            let sheet = Value::Object(realm.new_object());
            realm.create_synthetic_module(&key, vec![("default".into(), sheet)])
        }
    };
    state().borrow_mut().modules.cache.insert(key, id);
    Ok(id)
}

fn parse_json(realm: &mut Realm, text: &str) -> JsResult<Value> {
    let global = Value::Object(realm.global());
    let json = realm.get_str(global, "JSON")?;
    let parse = realm.get_str(json.clone(), "parse")?;
    realm.call(parse, json, &[Value::str(text)])
}

// ------------------------------------------------------------- import.meta

/// Adds `import.meta.resolve(specifier)`.
fn decorate_import_meta(realm: &mut Realm, id: ModuleId, meta: ObjectId) {
    let url = Rc::new(realm.module_url(id).to_string());
    let f = realm.new_native_function_full("resolve", 1, import_meta_resolve, Some(url), Vec::new(), false);
    realm.define_property(meta, PropertyKey::str("resolve"), Value::Object(f), HIDDEN);
}

fn import_meta_resolve(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let referrer = host::native_data::<String>(realm).map(|s| s.to_string()).unwrap_or_default();
    let spec = host::arg_str(realm, args, 0)?;
    match resolve_from(&referrer, &spec) {
        Ok(u) => Ok(Value::str(&u.to_string())),
        Err(msg) => Err(realm.throw_type_error(&msg)),
    }
}

// ---------------------------------------------------------------- entry

/// Run a top-level module script: compile with its URL, link (fetching
/// dependencies), evaluate and drain jobs. Errors are logged.
pub(crate) fn run_module_script(realm: &mut Realm, source: &str, url: &str) -> Result<(), String> {
    collect_import_maps(realm);
    let r = realm.run_module(source, url);
    realm.run_jobs();
    match r {
        Ok(_) => Ok(()),
        Err(e) => {
            let msg = realm.error_to_string(&e);
            host::log("error", &format!("Uncaught {msg} ({url})"));
            Err(msg)
        }
    }
}

#[cfg(test)]
mod watchdog_credit_tests {
    use super::*;
    use crate::tests::MockEnv;
    use crate::{FrameCall, FrameReply, ScriptHost};
    use std::time::{Duration, Instant};

    // An explicit test double for host credit; no sleep/network is involved.
    // Production EngineEnv::fetch independently credits its measured request
    // duration in engine/src/scripting.rs. The large credit is numerical and
    // makes its loss/duplication distinct from compilation time.
    const HOST_CREDIT: Duration = Duration::from_secs(60);
    const EMPTY_ARGS: &str = r#"{"t":"a","id":0,"v":[],"l":0}"#;

    fn fixture(success: bool) -> (MockEnv, ScriptHost) {
        let mut env = MockEnv::new("");
        let body = if success {
            "export const result=7;".to_owned()
        } else {
            // Parse enough input to make compilation credit observable even
            // on a coarse host clock, then fail with a real syntax error.
            let prefix: String = (0..128).map(|i| format!("const fixture{i}={i};\n")).collect();
            prefix + "export const = ;"
        };
        env.fetches.insert("https://example.test/dir/fixture.js".into(), (200, "text/javascript".into(), body));
        env.fetch_watchdog_credit = HOST_CREDIT;
        let host = ScriptHost::new(&mut env);
        (env, host)
    }

    fn consume_and_check_credit(realm: &mut Realm, original_deadline: Instant, total_elapsed: Duration) {
        // Expire relative to the ORIGINAL clock origin, not current wall
        // time, so descheduling cannot suppress the consumer checkpoint.
        let expired = original_deadline - Duration::from_secs(31);
        realm.deadline = Some(expired);
        assert!(realm.check_watchdog().is_ok());
        let credit = realm.deadline.unwrap().duration_since(expired);
        assert!(credit > HOST_CREDIT, "compilation credit missing: {credit:?}");
        assert!(credit <= HOST_CREDIT + total_elapsed, "host fetch credited twice: {credit:?} > {:?}", HOST_CREDIT + total_elapsed);
        realm.set_deadline(None);
    }

    fn load_once(success: bool) {
        let (mut env, mut host) = fixture(success);
        let deadline = Instant::now() + Duration::from_secs(30);
        host.realm().set_deadline(Some(deadline));
        let before = host.realm().watchdog_state();
        let started = Instant::now();
        let result = host.enter(&mut env, |realm| {
            load_module(realm, "https://example.test/dir/page.html", "./fixture.js")
                .map(|_| ())
                .map_err(|error| realm.error_to_string(&error))
        });
        let total_elapsed = started.elapsed();
        assert_eq!(result.is_ok(), success, "{result:?}");
        // Neither CPU origin/deadline nor termination is reset by successful
        // compilation or the syntax-error path. Credit stays TLS pending.
        assert_eq!(host.realm().watchdog_state(), before);
        consume_and_check_credit(host.realm(), deadline, total_elapsed);
    }

    #[test]
    fn javascript_dependency_success_keeps_host_credit_and_the_task_clock() {
        load_once(true);
    }

    #[test]
    fn javascript_dependency_syntax_error_credits_compilation_without_reset() {
        load_once(false);
    }

    fn compile_dependency(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
        load_module(realm, "https://example.test/dir/page.html", "./fixture.js")?;
        Ok(Value::Number(7.0))
    }

    #[test]
    fn borrowed_global_call_crossing_the_module_loader_returns_credit_and_restores_its_target() {
        for success in [true, false] {
            let (mut env, mut host) = fixture(success);
            let function = host.realm().new_native_function("compileFixture", 0, compile_dependency);
            host.realm().define_global("compileFixture", Value::Object(function));
            let idle = host.realm().watchdog_state();
            let idle_budget = host.realm().task_budget;
            let mut source = Realm::new();
            let deadline = Instant::now() + Duration::from_secs(30);
            source.set_deadline(Some(deadline));
            let before = source.watchdog_state();
            let mut task = before;
            let started = Instant::now();
            // A real global-call bridge invokes a test native that enters the
            // real dependency loader synchronously. No fake module results.
            let result = host.frame_call_with_watchdog(&mut env, FrameCall::CallGlobal { name: "compileFixture".into(), args: EMPTY_ARGS.into() }, &mut task);
            let total_elapsed = started.elapsed();
            assert_eq!(matches!(result, FrameReply::Value(ref value) if value == "7"), success, "{result:?}");
            if !success { assert!(matches!(result, FrameReply::Error(_)), "{result:?}"); }
            assert_eq!(task, before);
            assert_eq!(host.realm().watchdog_state(), idle);
            assert_eq!(host.realm().task_budget, idle_budget);
            source.replace_watchdog_state(task);
            consume_and_check_credit(&mut source, deadline, total_elapsed);
        }
    }
}
