//! Dedicated and shared workers in their own realm.
//!
//! A worker is a separate [`ScriptHost`] (its own realm, global object,
//! timers and microtask queue) owned by the host that created it and run on
//! the same thread: [`ScriptHost::pump_workers`] starts workers, carries
//! messages both ways (structured clones serialized to JSON, MessagePorts
//! bridged by id) and runs each worker's timers, after the owner's own
//! tasks. A worker's environment ([`WorkerEnv`]) has no DOM (an empty
//! document), the worker script's URL as its base, and the owner's
//! network, storage and clock.
//!
//! Owner side (`messaging.js`): `N.workerStart(url, name, sharedKey,
//! module)` → id, `N.workerSend(id, json)`, `N.workerTerminate(id)`, and
//! a `__worker_deliver(id, json)` function the host calls with messages and
//! errors. Worker side (`worker.js`): `N.workerPost(json)`, `N.workerClose()`
//! and `__worker_receive(json)`.

use std::cell::RefCell;
use std::rc::Rc;

use rabbithole_dom::{Document, NodeId};
use rabbithole_gfx::Rect;
use rabbithole_js::{JsResult, Realm, Value};
use rabbithole_net::{Request, Response, Url};
use rabbithole_style::ComputedStyle;

use crate::env::HostEnv;
use crate::host::{arg, arg_str, state};
use crate::ScriptHost;

/// Most workers one realm may run at once.
const MAX_WORKERS: usize = 32;

/// A request from a realm to its workers.
pub(crate) enum WorkerOp {
    Start { id: u32, url: String, name: String, shared: Option<String>, module: bool },
    Send { id: u32, text: String },
    Terminate { id: u32 },
}

/// Worker bookkeeping kept in each realm's host state.
#[derive(Default)]
pub(crate) struct WorkerState {
    pub next_id: u32,
    pub ops: Vec<WorkerOp>,
    /// This realm is a worker: messages for its owner.
    pub to_owner: Vec<String>,
    /// `close()` was called in this worker.
    pub closing: bool,
}

/// A running worker.
pub(crate) struct WorkerInstance {
    id: u32,
    /// Shared workers: `url + "\n" + name`; later `SharedWorker`s with the
    /// same key connect to this instance (owner-side ids map here).
    shared: Option<String>,
    aliases: Vec<u32>,
    url: Url,
    host: Box<ScriptHost>,
    doc: Document,
    inbox: Vec<String>,
    errors: Rc<RefCell<Vec<String>>>,
}

/// The environment of a worker realm: no document of its own (an empty
/// one), the script URL as base, everything else from the owner.
pub(crate) struct WorkerEnv<'a> {
    outer: &'a mut dyn HostEnv,
    doc: &'a mut Document,
    url: Url,
    errors: Rc<RefCell<Vec<String>>>,
}

impl HostEnv for WorkerEnv<'_> {
    fn doc(&mut self) -> &mut Document {
        self.doc
    }
    fn document_url(&self) -> Url {
        self.url.clone()
    }
    fn base_url(&self) -> Url {
        self.url.clone()
    }
    fn mark_dom_dirty(&mut self) {}
    fn ensure_layout(&mut self) {}
    fn node_rect(&mut self, _node: NodeId) -> Option<Rect> {
        None
    }
    fn computed_style(&mut self, _node: NodeId) -> Option<Rc<ComputedStyle>> {
        None
    }
    fn viewport(&self) -> (f32, f32) {
        self.outer.viewport()
    }
    fn scroll(&self) -> (f32, f32) {
        (0.0, 0.0)
    }
    fn set_scroll(&mut self, _x: f32, _y: f32) {}
    fn fetch(&mut self, req: Request) -> Result<Response, String> {
        self.outer.fetch(req)
    }
    fn fetch_background(&mut self, req: Request) -> Option<std::sync::mpsc::Receiver<Result<Response, String>>> {
        self.outer.fetch_background(req)
    }
    fn open_websocket(&mut self, url: Url, protocols: Vec<String>) -> Option<Result<rabbithole_net::WebSocket, String>> {
        self.outer.open_websocket(url, protocols)
    }
    fn open_event_source(&mut self, url: Url, with_credentials: bool) -> Option<Result<rabbithole_net::EventSource, String>> {
        self.outer.open_event_source(url, with_credentials)
    }
    fn navigate(&mut self, _url: Url, _replace: bool) {}
    fn cookies(&mut self) -> String {
        String::new()
    }
    fn set_cookie(&mut self, _value: &str) {}
    fn storage_get(&mut self, _local: bool, _key: &str) -> Option<String> {
        None
    }
    fn storage_set(&mut self, _local: bool, _key: &str, _value: Option<&str>) {}
    fn storage_keys(&mut self, _local: bool) -> Vec<String> {
        Vec::new()
    }
    fn log(&mut self, level: &str, msg: &str) {
        if level == "error" && msg.starts_with("Uncaught") {
            self.errors.borrow_mut().push(msg.to_string());
        }
        let file = self.url.path.rsplit('/').next().unwrap_or("").to_string();
        self.outer.log(level, &format!("[worker {file}] {msg}"));
    }
    fn alert(&mut self, _msg: &str) {}
    fn confirm(&mut self, _msg: &str) -> bool {
        false
    }
    fn prompt(&mut self, _msg: &str, _default: &str) -> Option<String> {
        None
    }
    fn user_agent(&self) -> String {
        self.outer.user_agent()
    }
    fn title_changed(&mut self, _title: &str) {}
    fn now_ms(&self) -> f64 {
        self.outer.now_ms()
    }
    fn random_bytes(&mut self, buf: &mut [u8]) {
        self.outer.random_bytes(buf)
    }
    fn open_window(&mut self, _url: Url) {}
    fn focus_node(&mut self, _node: Option<NodeId>) {}
    fn submit_form(&mut self, _form: NodeId, _submitter: Option<NodeId>) {}
    fn font_context(&mut self) -> Option<Rc<rabbithole_gfx::FontContext>> {
        self.outer.font_context()
    }
}

// ---------------------------------------------------------------- natives

fn worker_start(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let url = arg_str(realm, args, 0)?;
    let name = arg_str(realm, args, 1)?;
    let shared = match arg(args, 2) {
        v if v.is_nullish() => None,
        v => Some(realm.to_rust_string(v)?),
    };
    let module = realm.to_boolean(&arg(args, 3));
    let st = state();
    let mut st = st.borrow_mut();
    st.workers.next_id += 1;
    let id = st.workers.next_id;
    st.workers.ops.push(WorkerOp::Start { id, url, name, shared, module });
    Ok(Value::Number(id as f64))
}

fn worker_send(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = realm.to_number(arg(args, 0))? as u32;
    let text = arg_str(realm, args, 1)?;
    state().borrow_mut().workers.ops.push(WorkerOp::Send { id, text });
    Ok(Value::Undefined)
}

fn worker_terminate(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = realm.to_number(arg(args, 0))? as u32;
    state().borrow_mut().workers.ops.push(WorkerOp::Terminate { id });
    Ok(Value::Undefined)
}

fn worker_post(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let text = arg_str(realm, args, 0)?;
    let st = state();
    let mut st = st.borrow_mut();
    // After close() nothing more is sent.
    if !st.workers.closing {
        st.workers.to_owner.push(text);
    }
    Ok(Value::Undefined)
}

fn worker_close(_realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    state().borrow_mut().workers.closing = true;
    Ok(Value::Undefined)
}

/// `N.loadScriptSync(url)` → source text or null (importScripts).
fn load_script_sync(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let url = arg_str(realm, args, 0)?;
    match crate::fetch::do_fetch(&url, "GET", &[], &[]) {
        Ok(r) if r.status < 400 => Ok(Value::str(&rabbithole_net::decode_text(&r.body, r.charset.as_deref()))),
        _ => Ok(Value::Null),
    }
}

pub(crate) fn install_natives(realm: &mut Realm, n: rabbithole_js::ObjectId) {
    realm.define_method(n, "workerStart", 4, worker_start);
    realm.define_method(n, "workerSend", 2, worker_send);
    realm.define_method(n, "workerTerminate", 1, worker_terminate);
    realm.define_method(n, "workerPost", 1, worker_post);
    realm.define_method(n, "workerClose", 0, worker_close);
    realm.define_method(n, "loadScriptSync", 1, load_script_sync);
    realm.define_method(n, "workerInfo", 0, worker_info);
}

/// Call the realm's registered JS hook `name` with `args` (logged errors).
fn call_hook(realm: &mut Realm, name: &str, args: &[Value]) {
    let Some(f) = state().borrow().protos.get(name).copied() else { return };
    if let Err(e) = realm.call(Value::Object(f), Value::Undefined, args) {
        crate::host::report_error(realm, &e, name);
    }
    realm.run_jobs();
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn error_message(message: &str, url: &str) -> String {
    format!("{{\"t\":\"error\",\"message\":{},\"filename\":{}}}", json_str(message), json_str(url))
}

impl ScriptHost {
    /// Create the realm of a worker whose script is at `url`.
    fn new_worker(env: &mut dyn HostEnv, name: &str, shared: bool, id: u32) -> ScriptHost {
        ScriptHost::new_inner(env, Some((name.to_string(), shared, id)))
    }

    /// Start requested workers, deliver messages in both directions and run
    /// the workers' timers. Returns whether anything ran.
    pub fn pump_workers(&mut self, env: &mut dyn HostEnv, now_ms: f64) -> bool {
        let mut ran = false;
        for _round in 0..8 {
            let ops = std::mem::take(&mut self.state.borrow_mut().workers.ops);
            let mut progress = !ops.is_empty();
            for op in ops {
                match op {
                    WorkerOp::Start { id, url, name, shared, module } => self.start_worker(env, id, &url, &name, shared, module),
                    WorkerOp::Send { id, text } => {
                        if let Some(w) = self.workers.iter_mut().find(|w| w.id == id || w.aliases.contains(&id)) {
                            w.inbox.push(text);
                        }
                    }
                    WorkerOp::Terminate { id } => {
                        self.workers.retain(|w| w.id != id || w.shared.is_some());
                    }
                }
            }
            // Run each worker: messages in, timers, messages out.
            let mut i = 0;
            while i < self.workers.len() {
                let mut w = self.workers.remove(i);
                let inbox = std::mem::take(&mut w.inbox);
                progress |= !inbox.is_empty();
                let mut out = Vec::new();
                let closing;
                {
                    let mut wenv = WorkerEnv { outer: &mut *env, doc: &mut w.doc, url: w.url.clone(), errors: w.errors.clone() };
                    let host = &mut w.host;
                    for text in inbox {
                        host.enter(&mut wenv, |realm| {
                            // Each delivered message is a task of the worker.
                            realm.begin_task();
                            let v = Value::str(&text);
                            call_hook(realm, "__worker_receive", &[v]);
                        });
                    }
                    if host.next_due().map(|d| d <= now_ms).unwrap_or(false) || host.has_worker_work() {
                        progress |= host.run_timers(&mut wenv, now_ms);
                    }
                    let mut st = host.state.borrow_mut();
                    out.append(&mut st.workers.to_owner);
                    closing = st.workers.closing;
                }
                for e in w.errors.borrow_mut().drain(..) {
                    let msg = e.strip_prefix("Uncaught ").unwrap_or(&e).to_string();
                    out.push(error_message(&msg, &w.url.to_string()));
                }
                progress |= !out.is_empty();
                let targets: Vec<u32> = std::iter::once(w.id).chain(w.aliases.iter().copied()).collect();
                if !out.is_empty() {
                    self.enter(env, |realm| {
                        for text in &out {
                            // Shared workers address their connections through
                            // ports; plain messages go to the creating object.
                            let id = targets[0];
                            call_hook(realm, "__worker_deliver", &[Value::Number(id as f64), Value::str(text)]);
                        }
                    });
                }
                if closing {
                    continue; // dropped
                }
                self.workers.insert(i, w);
                i += 1;
            }
            ran |= progress;
            if !progress {
                break;
            }
        }
        ran
    }

    fn start_worker(&mut self, env: &mut dyn HostEnv, id: u32, url: &str, name: &str, shared: Option<String>, module: bool) {
        if let Some(key) = &shared {
            if let Some(w) = self.workers.iter_mut().find(|w| w.shared.as_ref() == Some(key)) {
                // Another SharedWorker object for the same worker: it
                // connects through its own port (sent by messaging.js).
                w.aliases.push(id);
                return;
            }
        }
        let fail = |host: &mut ScriptHost, env: &mut dyn HostEnv, msg: String| {
            host.enter(env, |realm| call_hook(realm, "__worker_deliver", &[Value::Number(id as f64), Value::str(&error_message(&msg, url))]));
        };
        if self.workers.len() >= MAX_WORKERS {
            fail(self, env, "Too many workers".into());
            return;
        }
        let Some(u) = Url::parse_absolute(url) else {
            fail(self, env, format!("Failed to load worker script at '{url}'"));
            return;
        };
        // Same-origin (or data:/blob:) scripts only, as in browsers.
        let source = if url.starts_with("blob:") || u.scheme == "data" {
            let r = self.enter(env, |_| crate::fetch::do_fetch(url, "GET", &[], &[]));
            r.ok().map(|r| rabbithole_net::decode_text(&r.body, r.charset.as_deref()))
        } else if u.origin() != env.document_url().origin() {
            let origin = env.document_url().origin();
            fail(self, env, format!("Script at '{url}' cannot be accessed from origin '{origin}'"));
            return;
        } else {
            match env.fetch(Request::get(u.clone()).with_referer(env.document_url())) {
                Ok(r) if r.status < 400 => Some(rabbithole_net::decode_text(&r.body, r.charset.as_deref())),
                _ => None,
            }
        };
        let Some(source) = source else {
            fail(self, env, format!("Failed to load worker script at '{url}'"));
            return;
        };
        let errors = Rc::new(RefCell::new(Vec::new()));
        let mut doc = Document::new();
        let host = {
            let mut wenv = WorkerEnv { outer: &mut *env, doc: &mut doc, url: u.clone(), errors: errors.clone() };
            let mut host = Box::new(ScriptHost::new_worker(&mut wenv, name, shared.is_some(), id));
            let _ = host.run_script(&mut wenv, &source, url, module);
            host.run_jobs(&mut wenv);
            host
        };
        self.workers.push(WorkerInstance { id, shared, aliases: Vec::new(), url: u, host, doc, inbox: Vec::new(), errors });
    }

    /// Workers waiting to start or with messages to deliver.
    pub(crate) fn has_worker_work(&self) -> bool {
        !self.state.borrow().workers.ops.is_empty() || self.workers.iter().any(|w| !w.inbox.is_empty() || w.host.has_worker_work())
    }

    /// Earliest due time of the workers' timers.
    pub(crate) fn workers_next_due(&self) -> Option<f64> {
        self.workers.iter().filter_map(|w| w.host.next_due()).fold(None, |acc: Option<f64>, d| Some(acc.map(|a| a.min(d)).unwrap_or(d)))
    }
}

/// `N.workerInfo()` → `{ name, shared, id }` in a worker realm, else null.
fn worker_info(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    let info = state().borrow().worker_info.clone();
    let Some((name, shared, id)) = info else { return Ok(Value::Null) };
    let o = realm.new_object();
    realm.set_str(Value::Object(o), "name", Value::str(&name))?;
    realm.set_str(Value::Object(o), "shared", Value::Bool(shared))?;
    realm.set_str(Value::Object(o), "id", Value::Number(id as f64))?;
    Ok(Value::Object(o))
}
