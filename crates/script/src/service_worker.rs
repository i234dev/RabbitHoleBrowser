//! Service worker registrations and live worker realms, owned by an Engine.
//! Storage is in-memory and survives documents in that Engine, not shutdown.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::{Rc, Weak};

use rabbithole_dom::Document;
use rabbithole_js::{JsResult, Realm, Value, ALL};
use rabbithole_net::{Request, Url};

use crate::env::{with_env, HostEnv};
use crate::host::{arg, arg_str, state};
use crate::workers::WorkerEnv;
use crate::ScriptHost;

#[derive(Clone, Default)]
pub struct ServiceWorkerRegistry(Rc<RefCell<Store>>);

impl std::fmt::Debug for ServiceWorkerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceWorkerRegistry").finish_non_exhaustive()
    }
}

/// A reserved navigation client. The last lease removes an uncommitted client;
/// once a Window realm adopts it, normal ScriptHost teardown owns its lifetime.
#[derive(Clone)]
pub struct ServiceWorkerClient(Rc<ReservedClient>);
struct ReservedClient { registry: Weak<RefCell<Store>>, id: u64 }
impl std::fmt::Debug for ServiceWorkerClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_tuple("ServiceWorkerClient").field(&self.0.id).finish() }
}
impl Drop for ReservedClient {
    fn drop(&mut self) {
        let Some(registry) = self.registry.upgrade() else { return };
        let discard = registry.borrow().clients.get(&self.id).is_some_and(|c| !c.execution_ready);
        if discard {
            let mut s = registry.borrow_mut();
            s.clients.remove(&self.id);
            for task in s.fetches.values().filter(|task| task.client == self.id) { task.cancelled.set(true); }
            s.prune_fetches(); s.clear_unregistered();
        }
    }
}
/// A buffered top-level GET navigation response and the client reserved for it.
pub struct ServiceWorkerNavigation { pub response: rabbithole_net::Response, pub client: ServiceWorkerClient }

/// Internal worker environment identity; embedders normally only supply a registry.
#[derive(Clone)]
pub struct ServiceWorkerScope {
    registry: Weak<RefCell<Store>>,
    worker: u64,
}

#[derive(Clone, Default)]
pub(crate) struct Context {
    registry: Weak<RefCell<Store>>,
    client: Option<u64>,
    worker: Option<u64>,
}

#[derive(Default)]
struct Store {
    next: u64,
    clients: BTreeMap<u64, Client>,
    registrations: BTreeMap<u64, Registration>,
    workers: BTreeMap<u64, Worker>,
    runtimes: BTreeMap<u64, Runtime>,
    jobs: VecDeque<Job>,
    fetches: BTreeMap<u64, FetchTask>,
    cache_names: BTreeMap<String, Vec<(String, u64)>>,
    caches: BTreeMap<u64, CacheRecord>,
}
/// Cancellation data shared between host bookkeeping only, never JS Values.
#[derive(Default)]
pub(crate) struct FetchCancellation {
    cancelled: std::cell::Cell<bool>,
    reason: RefCell<Option<String>>,
}
impl FetchCancellation {
    pub(crate) fn get(&self) -> bool { self.cancelled.get() }
    pub(crate) fn set(&self, value: bool) { if value { self.cancel(None); } }
    pub(crate) fn cancel(&self, reason: Option<String>) {
        if !self.cancelled.replace(true) { *self.reason.borrow_mut() = reason; }
    }
}
struct FetchTask {
    client: u64,
    navigation: bool,
    worker: u64,
    request: Request,
    sender: Option<std::sync::mpsc::Sender<Result<crate::fetch::FetchReply, String>>>,
    cancelled: Rc<FetchCancellation>,
    dispatched: bool,
    abort_notified: bool,
    fallback: bool,
    result: Option<Result<crate::fetch::FetchReply, String>>,
    network: Option<std::sync::mpsc::Receiver<Result<rabbithole_net::Response, String>>>,
    lifetime_done: bool,
}
struct CacheRecord {
    origin: String,
    entries: Vec<(String, String)>,
}
struct Client {
    execution_ready: bool,
    url: Url,
    controller: Option<u64>,
    events: Vec<Notice>,
}
struct Registration {
    id: u64,
    scope: Url,
    script: Url,
    cache: String,
    registered: bool,
    installing: Option<u64>,
    waiting: Option<u64>,
    active: Option<u64>,
}
struct Worker {
    registration: u64,
    script: Url,
    source: String,
    state: &'static str,
    skip_waiting: bool,
    event_result: Option<bool>,
    claim: bool,
}
struct Runtime {
    host: ScriptHost,
    doc: Document,
    url: Url,
    errors: Rc<RefCell<Vec<String>>>,
    phase: Phase,
}
#[derive(Clone, Copy)]
enum Phase { Install, Installing, Installed, Activate, Activating, Active }
struct Job {
    id: u64,
    client: u64,
    script: Url,
    scope: Url,
    cache: String,
}
#[derive(Clone)]
struct Notice {
    kind: &'static str,
    id: u64,
    value: u64,
    name: String,
    message: String,
}
impl Notice {
    fn simple(kind: &'static str, id: u64, value: u64) -> Self {
        Self { kind, id, value, name: String::new(), message: String::new() }
    }
}

impl Store {
    fn id(&mut self) -> u64 { self.next += 1; self.next }
    fn prune_fetches(&mut self) {
        self.fetches.retain(|_, f| retain_fetch(f));
        let obsolete: Vec<u64> = self.runtimes.keys().filter(|id| self.workers[*id].state == "redundant" && !fetch_work(self, **id)).copied().collect();
        for id in obsolete { self.runtimes.remove(&id); }
    }
    fn clear_unregistered(&mut self) {
        let registrations: Vec<u64> = self.registrations.values().filter(|r| {
            !r.registered
                && !self.clients.values().any(|c| c.controller.is_some_and(|id| self.workers[&id].registration == r.id))
                && !self.workers.values().any(|w| w.registration == r.id && matches!(w.state, "parsed" | "installing" | "activating"))
                && !self.fetches.values().any(|f| self.workers[&f.worker].registration == r.id && retain_fetch(f))
        }).map(|r| r.id).collect();
        for registration in registrations {
            let r = self.registrations.get_mut(&registration).unwrap();
            let workers: Vec<u64> = [r.installing.take(), r.waiting.take(), r.active.take()].into_iter().flatten().collect();
            for worker in workers {
                self.set_state(worker, "redundant");
                self.runtimes.remove(&worker);
            }
        }
    }

    fn matching(&self, url: &Url) -> Option<u64> {
        let text = url.without_fragment().to_string();
        self.registrations.values().filter(|r| r.registered && r.scope.origin() == url.origin() && text.starts_with(&r.scope.to_string()))
            .max_by_key(|r| r.scope.to_string().len()).map(|r| r.id)
    }
    fn notify(&mut self, origin: &str, notice: Notice) {
        for c in self.clients.values_mut().filter(|c| c.url.origin() == origin) { c.events.push(notice.clone()); }
    }
    fn set_state(&mut self, id: u64, state: &'static str) {
        if let Some(w) = self.workers.get_mut(&id) {
            w.state = state;
            let origin = w.script.origin();
            self.notify(&origin, Notice::simple("state", id, 0));
        }
    }
}

pub(crate) fn new_context(env: &mut dyn HostEnv) -> (Context, Option<ServiceWorkerRegistry>) {
    if let Some(scope) = env.service_worker_scope() {
        return (Context { registry: scope.registry, client: None, worker: Some(scope.worker) }, None);
    }
    let registry = env.service_worker_registry().unwrap_or_default();
    let url = env.document_url();
    if let Some(client) = env.service_worker_client() {
        if client.0.registry.upgrade().is_some_and(|s| Rc::ptr_eq(&s, &registry.0)) {
            let adopted = {
                let mut s = registry.0.borrow_mut();
                s.clients.get_mut(&client.0.id).filter(|c| !c.execution_ready && c.url.without_fragment() == url.without_fragment()).map(|c| { c.execution_ready = true; c.url = url.clone(); c.events.clear(); client.0.id })
            };
            if let Some(id) = adopted { return (Context { registry: Rc::downgrade(&registry.0), client: Some(id), worker: None }, Some(registry)); }
        }
    }
    let id = {
        let mut s = registry.0.borrow_mut();
        let controller = s.matching(&url).and_then(|id| s.registrations[&id].active).filter(|id| s.workers[id].state == "activated");
        let id = s.id();
        s.clients.insert(id, Client { execution_ready: true, url, controller, events: Vec::new() });
        id
    };
    (Context { registry: Rc::downgrade(&registry.0), client: Some(id), worker: None }, Some(registry))
}
pub(crate) fn drop_context(context: &Context) {
    if let (Some(id), Some(store)) = (context.client, context.registry.upgrade()) { let mut s = store.borrow_mut(); s.clients.remove(&id); for f in s.fetches.values().filter(|f| f.client == id) { f.cancelled.set(true); } s.prune_fetches(); s.clear_unregistered(); }
}
pub(crate) fn is_worker(context: &Context) -> bool { context.worker.is_some() }
impl ServiceWorkerScope {
    pub(crate) fn registry(&self) -> Option<ServiceWorkerRegistry> { self.registry.upgrade().map(ServiceWorkerRegistry) }
}
fn context() -> Context { state().borrow().service_worker.clone() }
fn registry(realm: &mut Realm) -> JsResult<Rc<RefCell<Store>>> {
    context().registry.upgrade().ok_or_else(|| realm.throw_type_error("Service worker environment was discarded"))
}
fn exception(realm: &mut Realm, name: &str, message: &str) -> Value { crate::misc::dom_exception(realm, name, message) }

pub(crate) fn trustworthy(url: &Url) -> bool {
    let host = url.host.trim_end_matches('.');
    let loopback = host.trim_start_matches('[').trim_end_matches(']').parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback());
    url.scheme == "https" || (url.scheme == "http" && (host == "localhost" || host.ends_with(".localhost") || loopback))
}
fn validate(realm: &mut Realm, client: &Url, script: &str, scope: &str) -> JsResult<(Url, Url)> {
    let script = Url::parse_absolute(script).ok_or_else(|| realm.throw_type_error("Invalid service worker script URL"))?.without_fragment();
    let scope = Url::parse_absolute(scope).ok_or_else(|| realm.throw_type_error("Invalid service worker scope URL"))?.without_fragment();
    for u in [&script, &scope] {
        if !matches!(u.scheme.as_str(), "http" | "https") { return Err(realm.throw_type_error("Service worker URLs must use HTTP or HTTPS")); }
        let path = u.pathname().to_ascii_lowercase();
        if path.contains("%2f") || path.contains("%5c") { return Err(realm.throw_type_error("Service worker URLs cannot contain encoded path separators")); }
        if u.origin() != client.origin() { return Err(exception(realm, "SecurityError", "Service worker script, scope and client must have the same origin")); }
    }
    if !trustworthy(client) { return Err(exception(realm, "SecurityError", "Service workers require a secure context")); }
    Ok((script, scope))
}

fn register(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let script = arg_str(realm, args, 0)?;
    let scope = arg_str(realm, args, 1)?;
    let cache = arg_str(realm, args, 2)?;
    let c = context();
    let client = c.client.ok_or_else(|| exception(realm, "InvalidStateError", "Only clients can register service workers"))?;
    let store = registry(realm)?;
    let client_url = store.borrow().clients.get(&client).map(|c| c.url.clone()).ok_or_else(|| exception(realm, "InvalidStateError", "Client was discarded"))?;
    let (script, scope) = validate(realm, &client_url, &script, &scope)?;
    let mut s = store.borrow_mut();
    let id = s.id();
    s.jobs.push_back(Job { id, client, script, scope, cache });
    Ok(Value::Number(id as f64))
}
fn unregister(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let id = realm.to_number(arg(args, 0))? as u64;
    let origin = with_env(|e| e.document_url().origin());
    let store = registry(realm)?;
    let mut s = store.borrow_mut();
    let removed = match s.registrations.get_mut(&id) {
        Some(r) if r.scope.origin() == origin => std::mem::replace(&mut r.registered, false),
        _ => false,
    };
    s.clear_unregistered();
    Ok(Value::Bool(removed))
}
fn event_done(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let success = realm.to_boolean(&arg(args, 0));
    let c = context();
    let store = registry(realm)?;
    if let Some(id) = c.worker {
        if let Some(w) = store.borrow_mut().workers.get_mut(&id) { w.event_result = Some(success); }
    }
    Ok(Value::Undefined)
}
fn skip_waiting(realm: &mut Realm, _: Value, _: &[Value]) -> JsResult<Value> {
    let store = registry(realm)?;
    if let Some(id) = context().worker { if let Some(w) = store.borrow_mut().workers.get_mut(&id) { w.skip_waiting = true; } }
    Ok(Value::Undefined)
}
fn claim(realm: &mut Realm, _: Value, _: &[Value]) -> JsResult<Value> {
    let id = context().worker.ok_or_else(|| exception(realm, "InvalidStateError", "Not a service worker"))?;
    let store = registry(realm)?;
    let mut s = store.borrow_mut();
    let reg = s.workers.get(&id).map(|w| w.registration).ok_or_else(|| exception(realm, "InvalidStateError", "Worker was discarded"))?;
    if s.registrations[&reg].active != Some(id) { return Err(exception(realm, "InvalidStateError", "The service worker is not active")); }
    s.workers.get_mut(&id).unwrap().claim = true;
    claim_clients(&mut s, id);
    Ok(Value::Undefined)
}
fn claim_clients(s: &mut Store, id: u64) {
    let reg = s.workers[&id].registration;
    let clients: Vec<u64> = s.clients.iter().filter(|(_, c)| c.execution_ready && s.matching(&c.url) == Some(reg) && c.controller != Some(id)).map(|(&id, _)| id).collect();
    for client in clients {
        let c = s.clients.get_mut(&client).unwrap();
        c.controller = Some(id); c.events.push(Notice::simple("controller", id, 0));
    }
}
fn object(realm: &mut Realm, values: &[(&str, Value)]) -> Value {
    let o = realm.new_object(); realm.native_root(Value::Object(o));
    for (name, v) in values { realm.define_property(o, *name, v.clone(), ALL); }
    Value::Object(o)
}
fn id_value(id: Option<u64>) -> Value { id.map(|v| Value::Number(v as f64)).unwrap_or(Value::Null) }
fn snapshot(realm: &mut Realm, _: Value, _: &[Value]) -> JsResult<Value> {
    let store = registry(realm)?;
    let c = context();
    let s = store.borrow();
    let origin = with_env(|e| e.document_url().origin());
    let mut registrations = Vec::new();
    for r in s.registrations.values().filter(|r| r.scope.origin() == origin) {
        registrations.push(object(realm, &[("id", Value::Number(r.id as f64)), ("scope", Value::str(&r.scope.to_string())), ("script", Value::str(&r.script.to_string())), ("cache", Value::str(&r.cache)), ("registered", Value::Bool(r.registered)), ("installing", id_value(r.installing)), ("waiting", id_value(r.waiting)), ("active", id_value(r.active))]));
    }
    let mut workers = Vec::new();
    for (&id, w) in s.workers.iter().filter(|(_, w)| w.script.origin() == origin) {
        workers.push(object(realm, &[("id", Value::Number(id as f64)), ("registration", Value::Number(w.registration as f64)), ("script", Value::str(&w.script.to_string())), ("state", Value::str(w.state))]));
    }
    let controller = c.client.and_then(|id| s.clients.get(&id)).and_then(|c| c.controller);
    let ready = c.client.and_then(|id| s.clients.get(&id)).and_then(|c| s.matching(&c.url)).filter(|id| s.registrations[id].active.is_some_and(|id| s.workers[&id].state == "activated"));
    let registrations = realm.new_array_value(registrations); realm.native_root(registrations.clone());
    let workers = realm.new_array_value(workers); realm.native_root(workers.clone());
    let worker_reg = c.worker.and_then(|id| s.workers.get(&id)).map(|w| w.registration);
    Ok(object(realm, &[("registrations", registrations), ("workers", workers), ("controller", id_value(controller)), ("ready", id_value(ready)), ("worker", id_value(c.worker)), ("registration", id_value(worker_reg))]))
}
fn take_events(realm: &mut Realm, _: Value, _: &[Value]) -> JsResult<Value> {
    let store = registry(realm)?;
    let events = context().client.and_then(|id| store.borrow_mut().clients.get_mut(&id).map(|c| std::mem::take(&mut c.events))).unwrap_or_default();
    let mut out = Vec::new();
    for e in events { out.push(object(realm, &[("kind", Value::str(e.kind)), ("id", Value::Number(e.id as f64)), ("value", Value::Number(e.value as f64)), ("name", Value::str(&e.name)), ("message", Value::str(&e.message))])); }
    Ok(realm.new_array_value(out))
}
pub(crate) fn install_natives(realm: &mut Realm, n: rabbithole_js::ObjectId) {
    realm.define_method(n, "cancelFetch", 2, crate::fetch::cancel_fetch);
    realm.define_method(n, "swFetchResponse", 2, fetch_response);
    realm.define_method(n, "swFetchFailed", 1, fetch_failed);
    realm.define_method(n, "swFetchFallback", 1, fetch_fallback);
    realm.define_method(n, "swFetchDone", 1, fetch_done);
    realm.define_method(n, "cacheOperation", 4, cache_operation);
    realm.define_method(n, "swRegister", 4, register);
    realm.define_method(n, "swUnregister", 1, unregister);
    realm.define_method(n, "swSnapshot", 0, snapshot);
    realm.define_method(n, "swTakeEvents", 0, take_events);
    realm.define_method(n, "swEventDone", 1, event_done);
    realm.define_method(n, "swSkipWaiting", 0, skip_waiting);
    realm.define_method(n, "swClaim", 0, claim);
}

/// Only controlled window clients are intercepted. Worker-initiated fetches
/// deliberately bypass the registry, including fetch(event.request).
pub(crate) fn intercept_fetch(request: &Request) -> Option<(std::sync::mpsc::Receiver<Result<crate::fetch::FetchReply, String>>, Rc<FetchCancellation>)> {
    let c = context();
    if c.worker.is_some() || !request.url.is_http() || state().borrow().worker_info.is_some() { return None; }
    let registry = c.registry.upgrade()?;
    let mut s = registry.borrow_mut();
    let client = c.client?;
    let worker = s.clients.get(&client)?.controller?;
    if !matches!(s.workers.get(&worker)?.state, "activating" | "activated") { return None; }
    let id = s.id(); let (tx, rx) = std::sync::mpsc::channel();
    let token = Rc::new(FetchCancellation::default());
    s.fetches.insert(id, FetchTask { client, navigation: false, worker, request: request.clone(), sender: Some(tx), cancelled: token.clone(), dispatched: false, abort_notified: false, fallback: false, result: None, network: None, lifetime_done: false });
    Some((rx, token))
}
fn fetch_identity(realm: &mut Realm, args: &[Value]) -> JsResult<(Rc<RefCell<Store>>, u64)> {
    let id = realm.to_number(arg(args, 0))? as u64;
    let store = registry(realm)?;
    let allowed = store.borrow().fetches.get(&id).is_some_and(|f| Some(f.worker) == context().worker);
    if !allowed { return Err(realm.throw_type_error("Fetch event has expired")); }
    Ok((store, id))
}
fn fetch_response(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let (store, id) = fetch_identity(realm, args)?;
    if store.borrow().fetches[&id].cancelled.get() { return Err(exception(realm, "AbortError", "Fetch response is no longer consumed")); }
    let request = store.borrow().fetches[&id].request.clone();
    let value = arg(args, 1); let o = value.as_object().filter(|o| realm.obj(*o).class_name() == Some("Response")).ok_or_else(|| realm.throw_type_error("respondWith requires a Response"))?;
    realm.native_root(value.clone());
    let status = realm.get_str(value.clone(), "status")?; let status = realm.to_number(status)?;
    let ty = realm.get_str(value.clone(), "type")?; let ty = realm.to_rust_string(ty)?;
    if (!(200.0..600.0).contains(&status) && !(status == 0.0 && matches!(ty.as_str(), "opaque" | "opaqueredirect"))) || ty == "error" || (ty == "cors" && request.mode == Some(rabbithole_net::RequestMode::SameOrigin)) || (ty == "opaque" && request.mode != Some(rabbithole_net::RequestMode::NoCors)) || (ty == "opaqueredirect" && request.redirect != rabbithole_net::RedirectMode::Manual) {
        return Err(realm.throw_type_error("Invalid service worker response type or status"));
    }
    let status_text = realm.get_str(value.clone(), "statusText")?; let status_text = realm.to_rust_string(status_text)?;
    let url = realm.get_str(value.clone(), "url")?; let url = realm.to_rust_string(url)?;
    let url = if url.is_empty() { request.url.clone() } else { Url::parse_absolute(&url).ok_or_else(|| realm.throw_type_error("Invalid response URL"))? };
    let headers = realm.get_str(value.clone(), "headers")?;
    let mut headers = crate::fetch::headers_from_init(realm, &headers)?;
    headers.retain(|(name, _)| !matches!(name.to_ascii_lowercase().as_str(), "set-cookie" | "set-cookie2"));
    let redirected = realm.get_str(value.clone(), "redirected")?; let redirected = realm.to_boolean(&redirected);
    let body = crate::fetch::take_body(realm, &Value::Object(o))?;
    let content_type = headers.iter().find(|(k, _)| k == "content-type").map(|(_,v)| v.clone()).unwrap_or_default();
    let (content_type, charset) = rabbithole_net::parse_content_type(&content_type);
    let reply = crate::fetch::FetchReply { response: rabbithole_net::Response { url, status: status as u16, content_type, charset, headers, body }, status_text, response_type: if ty == "default" { "basic".into() } else { ty }, redirected };
    if let Some(f) = store.borrow_mut().fetches.get_mut(&id) { f.result = Some(Ok(reply)); }
    Ok(Value::Undefined)
}
fn fetch_failed(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    if let Ok((store, id)) = fetch_identity(realm, args) { let mut s = store.borrow_mut(); let task = s.fetches.get_mut(&id).unwrap(); if !task.cancelled.get() { task.result = Some(Err("Service worker respondWith rejected or returned an invalid response".into())); } }
    Ok(Value::Undefined)
}
fn fetch_fallback(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let (store, id) = fetch_identity(realm, args)?;
    store.borrow_mut().fetches.get_mut(&id).unwrap().fallback = true;
    Ok(Value::Undefined)
}
fn fetch_done(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    if let Ok((store, id)) = fetch_identity(realm, args) { store.borrow_mut().fetches.get_mut(&id).unwrap().lifetime_done = true; }
    Ok(Value::Undefined)
}
// Cancellation ends delivery, not ExtendableEvent's explicit lifetime. In
// particular an old redundant worker still owns its outstanding waitUntil work.
fn retain_fetch(task: &FetchTask) -> bool { !task.cancelled.get() || (task.dispatched && (!task.abort_notified || !task.lifetime_done)) }
fn fetch_work(s: &Store, worker: u64) -> bool { s.fetches.values().any(|f| f.worker == worker && retain_fetch(f)) }
fn worker_fetch_hook(runtime: &mut Runtime, env: &mut dyn HostEnv, name: &str, args: &[Value]) {
    let mut wenv = WorkerEnv { outer: env, doc: &mut runtime.doc, url: runtime.url.clone(), errors: runtime.errors.clone(), service_worker: None };
    runtime.host.enter(&mut wenv, |realm| {
        realm.begin_task();
        let hook = state().borrow().protos.get(name).copied();
        if let Some(hook) = hook {
            if let Err(error) = realm.call(Value::Object(hook), Value::Undefined, args) { crate::host::report_error(realm, &error, "service worker fetch notification"); }
            realm.run_jobs();
        }
    });
}
fn pump_fetches(registry: &ServiceWorkerRegistry, worker: u64, runtime: &mut Runtime, env: &mut dyn HostEnv) {
    if !matches!(runtime.phase, Phase::Active) { return; }
    // Signal abort in the worker realm before retiring its request bookkeeping.
    let aborted: Vec<(u64, String)> = registry.0.borrow().fetches.iter().filter(|(_, f)| f.worker == worker && f.cancelled.get() && f.dispatched && !f.abort_notified).map(|(&id, f)| (id, f.cancelled.reason.borrow().clone().unwrap_or_default())).collect();
    for (id, reason) in aborted {
        worker_fetch_hook(runtime, env, "__sw_fetch_abort", &[Value::Number(id as f64), Value::str(&reason)]);
        if let Some(f) = registry.0.borrow_mut().fetches.get_mut(&id) { f.abort_notified = true; }
    }
    registry.0.borrow_mut().fetches.retain(|_, f| retain_fetch(f));
    let ids: Vec<u64> = registry.0.borrow().fetches.iter().filter(|(_, f)| f.worker == worker && !f.dispatched).map(|(&id, _)| id).collect();
    for id in ids {
        let (req, client, navigation) = {
            let mut s = registry.0.borrow_mut();
            let f = s.fetches.get_mut(&id).unwrap(); f.dispatched = true;
            (f.request.clone(), f.client, f.navigation)
        };
        let mut wenv = WorkerEnv { outer: env, doc: &mut runtime.doc, url: runtime.url.clone(), errors: runtime.errors.clone(), service_worker: None };
        runtime.host.enter(&mut wenv, |realm| {
            realm.begin_task();
            // This entry originates in Rust rather than a native call scope.
            // Release temporary request roots after the dispatch checkpoint.
            let root_mark = realm.native_roots.len();
            let request = if navigation { crate::fetch::make_navigation_request(realm, &req) } else { crate::fetch::make_request(realm, &req.url.to_string(), &req.method, req.headers.clone(), req.body.clone().map(|b| b.data).unwrap_or_default()) };
            realm.native_root(request.clone());
            let o = request.as_object().unwrap();
            let mode = match req.mode { Some(rabbithole_net::RequestMode::Navigate) => "navigate", Some(rabbithole_net::RequestMode::NoCors) => "no-cors", Some(rabbithole_net::RequestMode::SameOrigin) => "same-origin", _ => "cors" };
            let credentials = match req.credentials { rabbithole_net::CredentialsMode::Omit => "omit", rabbithole_net::CredentialsMode::Include => "include", _ => "same-origin" };
            let redirect = match req.redirect { rabbithole_net::RedirectMode::Manual => "manual", rabbithole_net::RedirectMode::Error => "error", _ => "follow" };
            for (name, value) in [("mode", mode), ("credentials", credentials), ("redirect", redirect)] { realm.define_property(o, name, Value::str(value), ALL); }
            let client_id = client.to_string();
            let hook = state().borrow().protos.get("__sw_fetch").copied();
            if let Some(hook) = hook {
                if let Err(e) = realm.call(Value::Object(hook), Value::Undefined, &[Value::Number(id as f64), request, Value::str(if navigation { "" } else { &client_id }), Value::str(if navigation { &client_id } else { "" })]) {
                    crate::host::report_error(realm, &e, "service worker fetch event");
                    if let Some(f) = registry.0.borrow_mut().fetches.get_mut(&id) { f.result = Some(Err("Service worker fetch dispatch failed".into())); f.lifetime_done = true; }
                }
                realm.run_jobs();
            }
            realm.native_roots.truncate(root_mark);
        });
    }
    let ids: Vec<u64> = registry.0.borrow().fetches.iter().filter(|(_, f)| f.worker == worker).map(|(&id, _)| id).collect();
    for id in ids {
        let mut task = registry.0.borrow_mut().fetches.remove(&id).unwrap();
        if task.cancelled.get() {
            // Drop the client channel, abandoned response bytes and network
            // receiver immediately. Keep only event lifetime bookkeeping.
            task.sender = None; task.network = None; task.result = None; task.fallback = false;
            task.request.body = None; task.request.headers.clear(); task.cancelled.reason.borrow_mut().take();
            if !task.lifetime_done { registry.0.borrow_mut().fetches.insert(id, task); }
            else { worker_fetch_hook(runtime, env, "__sw_fetch_complete", &[Value::Number(id as f64)]); }
            continue;
        }
        if task.fallback && task.result.is_none() && task.network.is_none() {
            task.fallback = false;
            if let Some(rx) = env.fetch_background(task.request.clone()) { task.network = Some(rx); }
            else { task.result = Some(env.fetch(task.request.clone()).map(crate::fetch::FetchReply::from)); }
        }
        if let Some(rx) = &task.network {
            match rx.try_recv() {
                Ok(r) => { task.result = Some(r.map(crate::fetch::FetchReply::from)); task.network = None; }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => { task.result = Some(Err("Network fallback disconnected".into())); task.network = None; }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(result) = task.result.take() { if let Some(sender) = task.sender.take() { if sender.send(result).is_err() { task.cancelled.set(true); } } }
        if !task.cancelled.get() && !(task.sender.is_none() && task.lifetime_done) { registry.0.borrow_mut().fetches.insert(id, task); }
        else { worker_fetch_hook(runtime, env, "__sw_fetch_complete", &[Value::Number(id as f64)]); }
    }
}

// Serialized response records cross realms without retaining any foreign GC
// values. Cache objects keep their storage identity even after name deletion.
fn cache_operation(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let op = arg_str(realm, args, 0)?;
    let key = arg_str(realm, args, 2)?;
    let payload = arg_str(realm, args, 3)?;
    let id = realm.to_number(arg(args, 1))? as u64;
    let url = with_env(|e| e.document_url());
    if !trustworthy(&url) { return Err(exception(realm, "SecurityError", "Cache Storage requires a secure, non-opaque origin")); }
    let origin = url.origin();
    let store = registry(realm)?;
    let mut s = store.borrow_mut();
    match op.as_str() {
        "names" => {
            let names = s.cache_names.get(&origin).map(|names| names.iter().map(|(n, _)| Value::str(n)).collect()).unwrap_or_default();
            Ok(realm.new_array_value(names))
        }
        "open" => {
            if let Some(id) = s.cache_names.get(&origin).and_then(|names| names.iter().find(|(name, _)| name == &key).map(|(_, id)| *id)) { return Ok(Value::Number(id as f64)); }
            let id = s.id();
            s.caches.insert(id, CacheRecord { origin: origin.clone(), entries: Vec::new() });
            s.cache_names.entry(origin).or_default().push((key, id));
            Ok(Value::Number(id as f64))
        }
        "delete" => {
            let names = s.cache_names.entry(origin).or_default();
            let found = names.iter().position(|(name, _)| name == &key);
            if let Some(i) = found { names.remove(i); }
            Ok(Value::Bool(found.is_some()))
        }
        _ => {
            let cache = s.caches.get_mut(&id).filter(|cache| cache.origin == origin).ok_or_else(|| realm.throw_type_error("Invalid cache storage identity"))?;
            match op.as_str() {
                "entries" => {
                    let mut out = Vec::new();
                    for (key, payload) in &cache.entries { out.push(object(realm, &[("key", Value::str(key)), ("record", Value::str(payload))])); }
                    Ok(realm.new_array_value(out))
                }
                "put" => {
                    if let Some(entry) = cache.entries.iter_mut().find(|(k, _)| *k == key) { entry.1 = payload; }
                    else { cache.entries.push((key, payload)); }
                    Ok(Value::Undefined)
                }
                "remove" => {
                    let found = cache.entries.iter().position(|(k, _)| *k == key);
                    if let Some(i) = found { cache.entries.remove(i); }
                    Ok(Value::Bool(found.is_some()))
                }
                _ => Err(realm.throw_type_error("Unknown cache operation")),
            }
        }
    }
}

fn reject_job(s: &mut Store, job: &Job, name: &str, message: &str) {
    if let Some(c) = s.clients.get_mut(&job.client) {
        c.events.push(Notice { kind: "reject", id: job.id, value: 0, name: name.into(), message: message.into() });
    }
}
fn resolve_job(s: &mut Store, job: &Job, registration: u64) {
    if let Some(c) = s.clients.get_mut(&job.client) { c.events.push(Notice::simple("resolve", job.id, registration)); }
}
fn notify(host: &mut ScriptHost, env: &mut dyn HostEnv) {
    host.enter(env, |realm| {
        for name in ["__sw_notify", "__sw_notify_events"] {
            let hook = state().borrow().protos.get(name).copied();
            if let Some(f) = hook {
                if let Err(e) = realm.call(Value::Object(f), Value::Undefined, &[]) { crate::host::report_error(realm, &e, "service worker notification"); }
                realm.run_jobs();
            }
        }
    });
}
fn process_job(registry: &ServiceWorkerRegistry, env: &mut dyn HostEnv, job: Job) {
    let existing = {
        let s = registry.0.borrow();
        s.registrations.values().find(|r| r.registered && r.scope == job.scope).map(|r| r.id)
    };
    if let Some(id) = existing {
        let mut s = registry.0.borrow_mut();
        let r = &s.registrations[&id];
        if r.installing.is_some() { s.jobs.push_back(job); return; }
    }
    let mut req = Request::get(job.script.clone());
    req.follow_redirects = false;
    req.dest = rabbithole_net::Destination::Script;
    req.referer = registry.0.borrow().clients.get(&job.client).map(|c| c.url.clone());
    req.headers.push(("Service-Worker".into(), "script".into()));
    if job.cache != "all" { req.cache = rabbithole_net::cache::CacheMode::NoCache; }
    let response = match env.fetch(req) {
        Ok(r) if (200..300).contains(&r.status) && r.url.without_fragment() == job.script => r,
        _ => { reject_job(&mut registry.0.borrow_mut(), &job, "TypeError", "Service worker script could not be fetched (redirects are not allowed)"); return; }
    };
    if !crate::node::is_js_mime(&response.content_type) {
        reject_job(&mut registry.0.borrow_mut(), &job, "SecurityError", "Service worker script must have a JavaScript MIME type"); return;
    }
    let max_scope = match response.header("service-worker-allowed") {
        Some(value) => job.script.join(value),
        None => job.script.join("./"),
    };
    let valid_scope = max_scope.is_some_and(|max| max.origin() == job.script.origin() && job.scope.pathname().starts_with(&max.pathname()));
    if !valid_scope {
        reject_job(&mut registry.0.borrow_mut(), &job, "SecurityError", "Service worker scope exceeds the script's allowed scope"); return;
    }
    let source = rabbithole_net::decode_text(&response.body, Some("utf-8"));
    let registration = {
        let mut s = registry.0.borrow_mut();
        if let Some(id) = existing {
            let r = &s.registrations[&id];
            if r.waiting.or(r.active).is_some_and(|id| s.workers[&id].script == job.script && s.workers[&id].source == source) {
                s.registrations.get_mut(&id).unwrap().cache = job.cache.clone();
                resolve_job(&mut s, &job, id); return;
            }
            id
        } else {
            let id = s.id();
            s.registrations.insert(id, Registration { id, scope: job.scope.clone(), script: job.script.clone(), cache: job.cache.clone(), registered: true, installing: None, waiting: None, active: None });
            id
        }
    };
    let worker = {
        let mut s = registry.0.borrow_mut(); let id = s.id();
        s.workers.insert(id, Worker { registration, script: job.script.clone(), source: source.clone(), state: "parsed", skip_waiting: false, event_result: None, claim: false });
        id
    };
    let mut doc = Document::new();
    let errors = Rc::new(RefCell::new(Vec::new()));
    let scope = ServiceWorkerScope { registry: Rc::downgrade(&registry.0), worker };
    let mut wenv = WorkerEnv { outer: env, doc: &mut doc, url: job.script.clone(), errors: errors.clone(), service_worker: Some(scope) };
    let mut host = ScriptHost::new_worker(&mut wenv, "", false, worker as u32);
    if let Err(message) = host.run_script(&mut wenv, &source, &job.script.to_string(), false) {
        let mut s = registry.0.borrow_mut(); s.set_state(worker, "redundant");
        if s.registrations[&registration].active.is_none() && s.registrations[&registration].waiting.is_none() { s.registrations.get_mut(&registration).unwrap().registered = false; }
        reject_job(&mut s, &job, "TypeError", &message); return;
    }
    host.run_jobs(&mut wenv);
    let mut s = registry.0.borrow_mut();
    let r = s.registrations.get_mut(&registration).unwrap();
    r.installing = Some(worker); r.script = job.script.clone(); r.cache = job.cache.clone();
    s.set_state(worker, "installing");
    s.runtimes.insert(worker, Runtime { host, doc, url: job.script.clone(), errors, phase: Phase::Install });
    resolve_job(&mut s, &job, registration);
    s.notify(&job.script.origin(), Notice::simple("updatefound", registration, 0));
}
fn dispatch(runtime: &mut Runtime, env: &mut dyn HostEnv, kind: &str) {
    let mut wenv = WorkerEnv { outer: env, doc: &mut runtime.doc, url: runtime.url.clone(), errors: runtime.errors.clone(), service_worker: None };
    runtime.host.enter(&mut wenv, |realm| {
        realm.begin_task();
        let hook = state().borrow().protos.get("__sw_dispatch").copied();
        if let Some(f) = hook {
            if let Err(e) = realm.call(Value::Object(f), Value::Undefined, &[Value::str(kind)]) { crate::host::report_error(realm, &e, "service worker event"); }
            realm.run_jobs();
        }
    });
}
fn advance(registry: &ServiceWorkerRegistry, env: &mut dyn HostEnv, id: u64, runtime: &mut Runtime, now: f64) -> bool {
    if runtime.host.next_due().is_some_and(|due| due <= now) {
        let mut wenv = WorkerEnv { outer: env, doc: &mut runtime.doc, url: runtime.url.clone(), errors: runtime.errors.clone(), service_worker: None };
        runtime.host.run_timers(&mut wenv, now);
        runtime.host.run_jobs(&mut wenv);
    }
    match runtime.phase {
        Phase::Install => { dispatch(runtime, env, "install"); runtime.phase = Phase::Installing; }
        Phase::Installing => {
            let mut s = registry.0.borrow_mut();
            if let Some(ok) = s.workers.get_mut(&id).and_then(|w| w.event_result.take()) {
                let registration = s.workers[&id].registration;
                s.registrations.get_mut(&registration).unwrap().installing = None;
                if !ok {
                    s.set_state(id, "redundant");
                    if s.registrations[&registration].active.is_none() && s.registrations[&registration].waiting.is_none() { s.registrations.get_mut(&registration).unwrap().registered = false; }
                    return false;
                }
                if let Some(old) = s.registrations[&registration].waiting { s.set_state(old, "redundant"); }
                s.registrations.get_mut(&registration).unwrap().waiting = Some(id);
                s.set_state(id, "installed"); runtime.phase = Phase::Installed;
            }
        }
        Phase::Installed => {
            let mut s = registry.0.borrow_mut();
            let registration = s.workers[&id].registration;
            let old = s.registrations[&registration].active;
            let has_clients = old.is_some_and(|old| s.clients.values().any(|c| c.controller == Some(old)));
            if !has_clients || s.workers[&id].skip_waiting {
                if let Some(old) = old {
                    s.set_state(old, "redundant");
                    for c in s.clients.values_mut().filter(|c| c.controller == Some(old)) {
                        c.controller = Some(id); c.events.push(Notice::simple("controller", id, 0));
                    }
                }
                let r = s.registrations.get_mut(&registration).unwrap(); r.waiting = None; r.active = Some(id);
                s.set_state(id, "activating"); runtime.phase = Phase::Activate;
            }
        }
        Phase::Activate => { dispatch(runtime, env, "activate"); runtime.phase = Phase::Activating; }
        Phase::Activating => {
            let mut s = registry.0.borrow_mut();
            // Activation waits for settlement; unlike installation, a rejected
            // lifetime promise does not roll back an active registration.
            if s.workers.get_mut(&id).and_then(|w| w.event_result.take()).is_some() {
                s.set_state(id, "activated"); runtime.phase = Phase::Active;
                let origin = s.workers[&id].script.origin(); s.notify(&origin, Notice::simple("ready", id, 0));
            }
        }
        Phase::Active => {}
    }
    true
}

fn pump_runtimes(registry: &ServiceWorkerRegistry, env: &mut dyn HostEnv, now: f64) -> bool {
    let mut ran = false;
    registry.0.borrow_mut().prune_fetches();
        let ids: Vec<u64> = registry.0.borrow().runtimes.keys().copied().collect();
        for id in ids {
            let runtime = registry.0.borrow_mut().runtimes.remove(&id);
            let Some(mut runtime) = runtime else { continue };
            if registry.0.borrow().workers[&id].state == "redundant" && !fetch_work(&registry.0.borrow(), id) { continue; }

            ran |= !matches!(runtime.phase, Phase::Active);
            let keep = advance(registry, env, id, &mut runtime, now);
            pump_fetches(registry, id, &mut runtime, env);
            if keep && (registry.0.borrow().workers[&id].state != "redundant" || fetch_work(&registry.0.borrow(), id)) { registry.0.borrow_mut().runtimes.insert(id, runtime); }
        }
        registry.0.borrow_mut().clear_unregistered();
        ran
}
impl ServiceWorkerRegistry {
    /// The scope check is side-effect free. Installing/waiting workers do not
    /// become navigation controllers until normal lifecycle activation occurs.
    pub fn controls_navigation(&self, url: &Url) -> bool {
        let s = self.0.borrow();
        url.is_http() && s.matching(url).and_then(|id| s.registrations[&id].active).is_some_and(|id| matches!(s.workers[&id].state, "activating" | "activated"))
    }

    /// Synchronous embedding convenience over asynchronous worker/network work.
    /// Only existing active registrations are considered. No disk persistence.
    /// The caller must provide a nonblocking fetch_background implementation.
    pub fn navigate(&self, env: &mut dyn HostEnv, url: Url, timeout: std::time::Duration) -> Result<Option<ServiceWorkerNavigation>, String> {
        if !self.controls_navigation(&url) { return Ok(None); }
        let deadline = std::time::Instant::now().checked_add(timeout).ok_or("Invalid navigation timeout")?;
        let client = {
            let mut s = self.0.borrow_mut(); let id = s.id();
            s.clients.insert(id, Client { execution_ready: false, url: url.clone(), controller: None, events: Vec::new() });
            ServiceWorkerClient(Rc::new(ReservedClient { registry: Rc::downgrade(&self.0), id }))
        };
        let mut target = url;
        for _ in 0..=20 {
            let worker = {
                let mut s = self.0.borrow_mut();
                let worker = s.matching(&target).and_then(|id| s.registrations[&id].active).filter(|id| matches!(s.workers[id].state, "activating" | "activated"));
                let reserved = s.clients.get_mut(&client.0.id).unwrap(); reserved.url = target.clone(); reserved.controller = worker;
                worker
            };
            let mut request = Request::get(target.without_fragment());
            request.dest = rabbithole_net::Destination::Document;
            request.mode = Some(rabbithole_net::RequestMode::Navigate);
            request.credentials = rabbithole_net::CredentialsMode::Include;
            request.redirect = rabbithole_net::RedirectMode::Manual;
            request.follow_redirects = false;
            let mut response = if let Some(worker) = worker {
                let (tx, rx) = std::sync::mpsc::channel();
                let token = Rc::new(FetchCancellation::default());
                { let mut s = self.0.borrow_mut(); let id = s.id();
                  s.fetches.insert(id, FetchTask { client: client.0.id, navigation: true, worker, request, sender: Some(tx), cancelled: token.clone(), dispatched: false, abort_notified: false, fallback: false, result: None, network: None, lifetime_done: false }); }
                let result = self.await_navigation(env, deadline, &rx);
                if result.is_err() { token.set(true); let now = env.now_ms(); pump_runtimes(self, env, now); }
                result?.response
            } else {
                let rx = env.fetch_background(request).ok_or("Navigation requires asynchronous network support")?;
                self.await_navigation_network(env, deadline, &rx)?
            };
            if matches!(response.status, 301 | 302 | 303 | 307 | 308) {
                if let Some(location) = response.header("location") {
                    let mut next = target.join(location.trim()).filter(|u| u.is_http()).ok_or("Invalid service worker navigation redirect")?;
                    if next.fragment.is_none() { next.fragment = target.fragment.clone(); }
                    target = next; continue;
                }
            }
            if response.status == 0 { return Err("Invalid opaque navigation response".into()); }
            // A cached fallback's Response.url must never change document origin.
            response.url = target;
            return Ok(Some(ServiceWorkerNavigation { response, client }));
        }
        Err("Too many service worker navigation redirects".into())
    }
    fn await_navigation(&self, env: &mut dyn HostEnv, deadline: std::time::Instant, rx: &std::sync::mpsc::Receiver<Result<crate::fetch::FetchReply, String>>) -> Result<crate::fetch::FetchReply, String> {
        loop {
            if std::time::Instant::now() >= deadline { return Err("Service worker navigation timed out".into()); }
            let now = env.now_ms(); pump_runtimes(self, env, now);
            if std::time::Instant::now() >= deadline { return Err("Service worker navigation timed out".into()); }
            match rx.try_recv() {
                Ok(result) => return result,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return Err("Service worker navigation disconnected".into()),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    fn await_navigation_network(&self, env: &mut dyn HostEnv, deadline: std::time::Instant, rx: &std::sync::mpsc::Receiver<Result<rabbithole_net::Response, String>>) -> Result<rabbithole_net::Response, String> {
        loop {
            if std::time::Instant::now() >= deadline { return Err("Navigation network timed out".into()); }
            let now = env.now_ms(); pump_runtimes(self, env, now);
            if std::time::Instant::now() >= deadline { return Err("Service worker navigation timed out".into()); }
            match rx.try_recv() {
                Ok(result) => return result,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return Err("Navigation network disconnected".into()),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
}

impl ScriptHost {
    pub(crate) fn pump_service_workers(&mut self, env: &mut dyn HostEnv, now: f64) -> bool {
        if self.service_workers_due().is_none_or(|due| due > now) { return false; }
        let Some(registry) = self.service_workers.clone() else { return false };
        let mut ran = false;
        registry.0.borrow_mut().prune_fetches();
        let job = registry.0.borrow_mut().jobs.pop_front();
        if let Some(job) = job { process_job(&registry, env, job); ran = true; }
        // Publish register() while installing, before firing install: callers
        // can observe the actual state changes, rather than a synthetic result.
        notify(self, env);
        ran |= pump_runtimes(&registry, env, now);
        notify(self, env);
        ran
    }
    pub(crate) fn service_workers_due(&self) -> Option<f64> {
        let registry = self.service_workers.as_ref()?;
        let s = registry.0.borrow();
        let c = context_of(self);
        if !s.jobs.is_empty() || c.client.is_some_and(|id| s.clients.get(&id).is_some_and(|c| !c.events.is_empty())) { return Some(0.0); }
        if s.fetches.values().any(|f| (f.cancelled.get() && !f.abort_notified) || (!f.dispatched && matches!(s.workers[&f.worker].state, "activated" | "redundant")) || f.result.is_some() || f.fallback) { return Some(0.0); }
        let mut due: Option<f64> = if s.fetches.values().any(|f| f.network.is_some()) { Some(self.state.borrow().last_tick + 10.0) } else { None };
        for (&id, r) in &s.runtimes {
            let immediate = match r.phase {
                Phase::Install | Phase::Activate => true,
                Phase::Installing | Phase::Activating => s.workers[&id].event_result.is_some(),
                Phase::Installed => {
                    let reg = &s.registrations[&s.workers[&id].registration];
                    s.workers[&id].skip_waiting || reg.active.is_none_or(|old| !s.clients.values().any(|c| c.controller == Some(old)))
                }
                Phase::Active => false,
            };
            let d = if immediate { Some(0.0) } else { r.host.next_due() };
            if let Some(d) = d { due = Some(due.map(|old| old.min(d)).unwrap_or(d)); }
        }
        due
    }
}
fn context_of(host: &ScriptHost) -> Context { host.state.borrow().service_worker.clone() }
