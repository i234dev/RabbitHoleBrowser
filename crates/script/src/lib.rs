//! Web platform bindings for RabbitHole's JavaScript engine: `window`,
//! `document`, the DOM, events, timers, `fetch`/XHR, storage and CSSOM.
//!
//! The embedder implements [`HostEnv`] and drives a [`ScriptHost`]:
//! run scripts, dispatch events, run timers and jobs.

mod cssom;
mod document;
mod element;
mod env;
mod events;
mod fetch;
mod frames;
mod host;
mod misc;
mod modules;
mod live;
mod media;
mod node;
mod canvas2d;
mod natives;
mod sockets;
mod observers;
mod shadow;
mod window;
mod workers;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_webapi;
#[cfg(test)]
mod tests_media;
#[cfg(test)]
mod tests_realms;

pub use env::HostEnv;
pub use host::{NodeRef, TargetKey};
pub use rabbithole_js::exclude_from_watchdog;

use env::{with_env, EnvGuard};
use rabbithole_js::V;
use host::{state, HostState, StateGuard};
use rabbithole_dom::{Document, NodeId};
use rabbithole_js::{Realm, Value};
use rabbithole_net::Url;
use std::cell::RefCell;
use crate::host::{HashMap, HashSet};
use std::rc::Rc;

/// Web APIs implemented in JavaScript, evaluated in order at startup.
const JS_APIS: &[(&str, &str)] = &[
    ("rabbithole:prelude.js", include_str!("prelude.js")),
    ("rabbithole:reflect_table.js", include_str!("js/reflect_table.js")),
    ("rabbithole:dom.js", include_str!("js/dom.js")),
    ("rabbithole:events.js", include_str!("js/events.js")),
    ("rabbithole:streams.js", include_str!("js/streams.js")),
    ("rabbithole:net.js", include_str!("js/net.js")),
    ("rabbithole:perf.js", include_str!("js/perf.js")),
    ("rabbithole:messaging.js", include_str!("js/messaging.js")),
    ("rabbithole:cssom.js", include_str!("js/cssom.js")),
    ("rabbithole:html.js", include_str!("js/html.js")),
    ("rabbithole:observers.js", include_str!("js/observers.js")),
    ("rabbithole:platform.js", include_str!("js/platform.js")),
    ("rabbithole:idb.js", include_str!("js/idb.js")),
    ("rabbithole:crypto.js", include_str!("js/crypto.js")),
    ("rabbithole:canvas.js", include_str!("js/canvas.js")),
    ("rabbithole:navigation.js", include_str!("js/navigation.js")),
    ("rabbithole:animations.js", include_str!("js/animations.js")),
    ("rabbithole:fonts.js", include_str!("js/fonts.js")),
    ("rabbithole:extras.js", include_str!("js/extras.js")),
    ("rabbithole:svg.js", include_str!("js/svg.js")),
    ("rabbithole:media.js", include_str!("js/media.js")),
    ("rabbithole:webaudio.js", include_str!("js/webaudio.js")),
    ("rabbithole:observable.js", include_str!("js/observable.js")),
];

/// Default watchdog budget of one task (a timer callback, event listener,
/// animation frame callback, message or microtask checkpoint; see
/// [`task_budget`]).
pub const TASK_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);

/// Watchdog budget of one task: [`TASK_BUDGET`], or `RH_TASK_BUDGET_MS`.
/// Time spent in forced style/layout and synchronous loads does not count
/// (see [`rabbithole_js::exclude_from_watchdog`]). A task past it is
/// stopped with an uncatchable "Script execution timed out" error; the
/// other tasks go on. Every realm (each frame has its own) applies it.
pub fn task_budget() -> std::time::Duration {
    static BUDGET: std::sync::OnceLock<std::time::Duration> = std::sync::OnceLock::new();
    *BUDGET.get_or_init(|| std::env::var("RH_TASK_BUDGET_MS").ok().and_then(|v| v.parse().ok()).map(std::time::Duration::from_millis).unwrap_or(TASK_BUDGET))
}

/// Budget of running a script of `bytes` bytes: a task budget plus 2 s per
/// MB of source (compiling a multi-MB bundle alone takes seconds), at most
/// 30 s.
pub fn script_budget(bytes: usize) -> std::time::Duration {
    (task_budget() + std::time::Duration::from_millis(2000 * bytes as u64 / (1024 * 1024))).min(std::time::Duration::from_secs(30).max(task_budget()))
}

/// How long one [`ScriptHost::run_timers`] call keeps running due timers
/// before yielding to the event loop; the timers still due run on the next
/// call (they stay due, so the embedder calls again right away).
pub const TICK_SLICE: std::time::Duration = std::time::Duration::from_millis(100);

/// Extra data carried by a dispatched event.
#[derive(Debug, Clone, PartialEq)]
pub enum EventDetail {
    Mouse { x: f32, y: f32, button: u8, ctrl: bool, shift: bool, alt: bool, meta: bool },
    Key { key: String, code: String, ctrl: bool, shift: bool, alt: bool, meta: bool, repeat: bool },
    Input { data: Option<String> },
    /// `transition*` (TransitionEvent: `name` = propertyName) or
    /// `animation*` (AnimationEvent: `name` = animationName) events.
    Animation { name: String, elapsed: f32, pseudo: String, transition: bool },
    None,
}

/// How an event is created.
#[derive(Debug, Clone, PartialEq)]
pub struct EventInit {
    pub bubbles: bool,
    pub cancelable: bool,
    pub detail: Option<EventDetail>,
}

impl EventInit {
    pub fn simple(bubbles: bool, cancelable: bool) -> EventInit {
        EventInit { bubbles, cancelable, detail: None }
    }
}

/// What the embedder should do after an uncancelled event.
#[derive(Debug, Clone, PartialEq)]
pub enum DefaultAction {
    None,
    Navigate(Url),
    /// (form, submitter)
    SubmitForm(NodeId, Option<NodeId>),
}

/// Outcome of a dispatch.
#[derive(Debug, Clone, PartialEq)]
pub struct DispatchResult {
    pub default_prevented: bool,
    /// Whether any listener ran.
    pub handled: bool,
    pub default_action: DefaultAction,
}

impl DispatchResult {
    fn nothing() -> DispatchResult {
        DispatchResult { default_prevented: false, handled: false, default_action: DefaultAction::None }
    }
}

/// A request from a same-origin ancestor's realm, run in a frame's realm
/// (see [`HostEnv::frame_call`] and [`ScriptHost::frame_call`]).
#[derive(Debug, Clone, PartialEq)]
pub enum FrameCall {
    /// Create an event of interface `iface` (`MouseEvent`, ...) with type
    /// `ty` and the init dictionary `init` (JSON) and dispatch it at `node`.
    Dispatch { node: NodeId, iface: String, ty: String, init: String },
    /// Call a no-argument method of the node: `click`, `focus`, `blur`.
    Method { node: NodeId, name: String },
    /// Read global property `name` of the frame's window.
    GetGlobal { name: String },
    /// Call the frame's global function `name` with the arguments (a
    /// structured clone, serialized).
    CallGlobal { name: String, args: String },
}

/// Result of a [`FrameCall`].
#[derive(Debug, Clone, PartialEq)]
pub enum FrameReply {
    /// `Dispatch`: whether the event was not cancelled.
    Bool(bool),
    /// A node of the frame's document.
    Node(NodeId),
    /// A function (the caller wraps it to call it through `CallGlobal`).
    Function,
    /// Any other value, as a serialized structured clone.
    Value(String),
    Undefined,
    /// An exception thrown in the frame (its message).
    Error(String),
}

/// The scripting host of one page.
pub struct ScriptHost {
    realm: Realm,
    state: Rc<RefCell<HostState>>,
    /// Dedicated / shared workers this realm started (see [`workers`]).
    workers: Vec<workers::WorkerInstance>,
}

/// Worker realms: the worker global scope (after the window APIs).
const WORKER_JS: &str = include_str!("js/worker.js");

impl ScriptHost {
    /// Create the realm and install `window`, `document` and all globals.
    pub fn new(env: &mut dyn HostEnv) -> ScriptHost {
        Self::new_inner(env, None)
    }

    /// `worker`: (name, shared, id) for a worker realm.
    pub(crate) fn new_inner(env: &mut dyn HostEnv, worker: Option<(String, bool, u32)>) -> ScriptHost {
        let mut realm = Realm::new();
        realm.set_task_budget(Some(task_budget()));
        rabbithole_wasm_js::install(&mut realm);
        let now = env.now_ms();
        let state = Rc::new(RefCell::new(HostState {
            wrappers: HashMap::default(),
            child_lists: HashMap::default(),
            protos: HashMap::default(),
            listeners: HashMap::default(),
            listener_counts: HashMap::default(),
            timers: Vec::new(),
            rafs: Vec::new(),
            next_timer_id: 1,
            last_frame: now,
            last_tick: 0.0,
            dirty: false,
            ready_state: "loading",
            loading: true,
            current_script: None,
            focused: None,
            extra_docs: Vec::new(),
            window: realm.global(),
            document_obj: realm.global(),
            root_array: realm.new_array(Vec::new()),
            ran_scripts: HashSet::default(),
            inline_handlers: HashMap::default(),
            observers: Vec::new(),
            mutation_records: Vec::new(),
            mutation_scheduled: false,
            custom_elements: HashMap::default(),
            ce_constructing: None,
            ce_upgraded: std::collections::HashSet::default(),
            ce_failed: std::collections::HashSet::default(),
            location_override: None,
            history_len: 1,
            history_state: Value::Null,
            event_stack: Vec::new(),
            time_origin: now,
            console_marks: HashMap::default(),
            console_counts: HashMap::default(),
            pending_script_events: Vec::new(),
            pending_script_fetches: Vec::new(),
            pending_fetches: Vec::new(),
            deferred_natives: Vec::new(),
            modules: Default::default(),
            shadow: Default::default(),
            render: Default::default(),
            factories: HashMap::default(),
            blob_urls: HashMap::default(),
            sockets: Default::default(),
            media: Default::default(),
            canvases: Default::default(),
            workers: Default::default(),
            worker_info: worker.clone(),
            live_freed: Default::default(),
            open_docs: HashMap::default(),
        }));
        {
            let root_array = state.borrow().root_array;
            realm.extra_roots.push(Value::Object(root_array));
        }
        // Console output goes to the environment.
        realm.set_console_sink(Box::new(|level, text| {
            if env::env_active() {
                with_env(|env| env.log(level, &text));
            } else {
                eprintln!("[console.{level}] {text}");
            }
        }));
        let mut host = ScriptHost { realm, state, workers: Vec::new() };
        {
            let _g = EnvGuard::enter(env);
            let _s = StateGuard::enter(host.state.clone());
            let realm = &mut host.realm;
            events::install(realm);
            node::install(realm);
            element::install(realm);
            document::install(realm);
            window::install(realm);
            frames::install(realm);
            cssom::install(realm);
            fetch::install(realm);
            misc::install(realm);
            observers::install(realm);
            shadow::install(realm);
            modules::install(realm);
            natives::install(realm);
            // [CEReactions]: reactions queued by a DOM method (setAttribute,
            // classList, reflected attributes...) run before it returns.
            realm.post_native = Some(shadow::run_pending_reactions);
            // Prototypes and internal objects the bindings look up by name
            // must survive GC even while nothing in JS references them
            // (e.g. the NamedNodeMap prototype when no `attributes` object
            // is alive).
            let internals: Vec<rabbithole_js::ObjectId> = host.state.borrow().protos.values().copied().collect();
            for id in internals {
                realm.extra_roots.push(Value::Object(id));
            }
            // The document wrapper (the JS-implemented APIs use `document`).
            let doc_obj = host::wrap_main(realm, Document::ROOT);
            if let V::Object(d) = doc_obj.v() {
                host.state.borrow_mut().document_obj = d;
            }
            // Web APIs written in JavaScript (DOMMatrix, CSSOM, ...).
            for (name, src) in JS_APIS {
                if let Err(e) = realm.eval(src, name) {
                    let msg = realm.error_to_string(&e);
                    eprintln!("[rabbithole] {name} failed: {msg}");
                }
            }
            if worker.is_some() {
                if let Err(e) = realm.eval(WORKER_JS, "rabbithole:worker.js") {
                    let msg = realm.error_to_string(&e);
                    eprintln!("[rabbithole] worker.js failed: {msg}");
                }
            }
            let _ = realm.eval("delete globalThis.__rhNative; delete globalThis.__rhClone", "rabbithole:cleanup");
        }
        host
    }

    fn enter<R>(&mut self, env: &mut dyn HostEnv, f: impl FnOnce(&mut Realm) -> R) -> R {
        let _g = EnvGuard::enter(env);
        let _s = StateGuard::enter(self.state.clone());
        let r = f(&mut self.realm);
        r
    }

    /// Evaluate a classic script or module. Errors are logged and returned.
    ///
    /// Modules are compiled with `url` as their URL, linked (dependencies
    /// fetched synchronously through [`HostEnv::fetch`], bare specifiers
    /// resolved through the document's import maps) and evaluated; pending
    /// jobs are drained afterwards.
    pub fn run_script(&mut self, env: &mut dyn HostEnv, source: &str, url: &str, is_module: bool) -> Result<(), String> {
        self.realm.begin_task_with(script_budget(source.len()));
        if is_module {
            return self.enter(env, |realm| modules::run_module_script(realm, source, url));
        }
        self.enter(env, |realm| {
            let r = realm.exec(source, url);
            shadow::flush(realm);
            match r {
                Ok(_) => Ok(()),
                Err(e) => {
                    let msg = realm.error_to_string(&e);
                    host::log("error", &format!("Uncaught {msg} ({url})"));
                    Err(msg)
                }
            }
        })
    }

    /// Install an import map (the JSON text of a `<script type="importmap">`).
    /// Entries already defined by earlier maps win. Keys and addresses are
    /// resolved against the document base URL when used.
    pub fn set_import_map(&mut self, json: &str) -> Result<(), String> {
        let map = match modules::parse_import_map(&mut self.realm, json) {
            Ok(m) => m,
            Err(e) => return Err(self.realm.error_to_string(&e)),
        };
        self.state.borrow_mut().modules.import_map.merge(map);
        Ok(())
    }

    /// Mark a script element as the running one (`document.currentScript`)
    /// and as already executed.
    pub fn set_current_script(&mut self, node: Option<NodeId>) {
        let mut st = self.state.borrow_mut();
        st.current_script = node.map(NodeRef::main);
        if let Some(n) = node {
            st.ran_scripts.insert(n);
        }
    }

    /// Mark script elements present at load time as executed by the engine
    /// (so inserting them again does not rerun them).
    /// The document of frame `frame_id` was replaced: drop this realm's
    /// node wrappers, listeners and inline handlers keyed by its node ids
    /// (they would otherwise resolve to unrelated nodes of the new
    /// document). Existing JS references keep pointing at nothing useful;
    /// new lookups get fresh wrappers.
    pub fn forget_frame_document(&mut self, frame_id: u64) {
        if frame_id >= host::FRAME_DOC as u64 {
            return;
        }
        let doc = host::FRAME_DOC | frame_id as u32;
        let mut st = self.state.borrow_mut();
        st.wrappers.retain(|k, _| k.doc != doc);
        st.listeners.retain(|k, _| !matches!(k, TargetKey::Node(n) if n.doc == doc));
        st.listener_counts.retain(|(k, _), _| !matches!(k, TargetKey::Node(n) if n.doc == doc));
        st.inline_handlers.retain(|(n, _), _| n.doc != doc);
    }

    pub fn mark_script_ran(&mut self, node: NodeId) {
        self.state.borrow_mut().ran_scripts.insert(node);
    }

    /// Dispatch an event to a node (bubbling through its ancestors and the
    /// window). Returns whether the default was prevented and the default
    /// action the embedder should perform.
    pub fn dispatch_event(&mut self, env: &mut dyn HostEnv, target: NodeId, event_type: &str, init: EventInit) -> DispatchResult {
        self.enter(env, |realm| {
            let r = events::dispatch(realm, TargetKey::Node(NodeRef::main(target)), event_type, init, true);
            shadow::flush(realm);
            r
        })
    }

    /// Run a request from a same-origin ancestor's realm in this realm
    /// (see [`FrameCall`]). Microtasks it queues run at the caller's next
    /// microtask checkpoint ([`HostEnv::frame_checkpoint`]).
    pub fn frame_call(&mut self, env: &mut dyn HostEnv, call: FrameCall) -> FrameReply {
        self.enter(env, |realm| {
            let r = frames::foreign(realm, call);
            shadow::flush(realm);
            r
        })
    }

    /// Something that observers depend on changed outside of scripts
    /// (e.g. an inner scroll container scrolled): run the "update the
    /// rendering" hook (IntersectionObserver / ResizeObserver) soon.
    pub fn invalidate_rendering(&mut self) {
        self.state.borrow_mut().render.dirty = true;
    }

    /// Dispatch an event on `window` (`load`, `resize`, `scroll`, ...).
    pub fn dispatch_window_event(&mut self, env: &mut dyn HostEnv, event_type: &str) -> DispatchResult {
        if matches!(event_type, "scroll" | "resize") {
            self.state.borrow_mut().render.dirty = true;
        }
        if event_type == "scroll" {
            // Viewport scrolling fires `scroll` at the Document, bubbling
            // to the window (CSSOM View §13.1): `document.addEventListener
            // ('scroll', ...)` must see it too.
            return self.enter(env, |realm| events::dispatch(realm, TargetKey::Node(NodeRef::main(Document::ROOT)), event_type, EventInit::simple(true, false), true));
        }
        self.enter(env, |realm| events::dispatch(realm, TargetKey::Window, event_type, EventInit::simple(false, false), true))
    }

    /// Deliver a message another frame posted to this window (`data` is
    /// the serialized structured clone, `origin` the sender's origin,
    /// `source` the sender's frame id).
    pub fn deliver_message(&mut self, env: &mut dyn HostEnv, data: &str, origin: &str, source: u64) {
        self.enter(env, |realm| {
            if let Err(e) = frames::deliver(realm, data, origin, source) {
                let msg = realm.error_to_string(&e);
                host::log("error", &format!("Uncaught {msg} (message event)"));
            }
        });
    }

    /// Dispatch an event on `document`.
    pub fn dispatch_document_event(&mut self, env: &mut dyn HostEnv, event_type: &str, bubbles: bool) -> DispatchResult {
        self.enter(env, |realm| events::dispatch(realm, TargetKey::Node(NodeRef::main(Document::ROOT)), event_type, EventInit::simple(bubbles, false), true))
    }

    /// Finish loading: `readyState` → interactive, `DOMContentLoaded`,
    /// then `complete` and `load`.
    pub fn document_ready(&mut self, env: &mut dyn HostEnv) {
        self.enter(env, |realm| {
            {
                let st = state();
                let mut st = st.borrow_mut();
                st.ready_state = "interactive";
                st.loading = false;
            }
            events::dispatch(realm, TargetKey::Node(NodeRef::main(Document::ROOT)), "readystatechange", EventInit::simple(false, false), true);
            events::dispatch(realm, TargetKey::Node(NodeRef::main(Document::ROOT)), "DOMContentLoaded", EventInit::simple(true, false), true);
            realm.begin_task();
            realm.run_jobs();
            state().borrow_mut().ready_state = "complete";
            events::dispatch(realm, TargetKey::Node(NodeRef::main(Document::ROOT)), "readystatechange", EventInit::simple(false, false), true);
            events::dispatch(realm, TargetKey::Window, "load", EventInit::simple(false, false), true);
            events::dispatch(realm, TargetKey::Window, "pageshow", EventInit::simple(false, false), true);
            realm.begin_task();
            realm.run_jobs();
        });
    }

    /// Drain microtasks, queued callbacks and mutation observer records.
    pub fn run_jobs(&mut self, env: &mut dyn HostEnv) {
        self.enter(env, |realm| {
            // A microtask checkpoint is a task of its own for the watchdog.
            realm.begin_task();
            for _ in 0..16 {
                if realm.timed_out() {
                    // Out of time: what is left runs at the next checkpoint.
                    break;
                }
                shadow::flush(realm);
                realm.run_jobs();
                if observers::has_pending_mutations() {
                    observers::deliver_mutations(realm);
                }
                if shadow::has_pending_reactions() || shadow::has_pending_slot_checks() {
                    shadow::drain_ce_reactions(realm);
                    shadow::check_slots(realm);
                }
                if !realm.has_pending_jobs() && !observers::has_pending_mutations() && !shadow::has_pending_reactions() && !shadow::has_pending_slot_checks() {
                    break;
                }
            }
            for r in realm.take_unhandled_rejections() {
                let msg = realm.error_to_string(&r);
                host::log("error", &format!("Uncaught (in promise) {msg}"));
            }
            frames::checkpoint();
        });
    }

    /// Run timers and animation frames that are due at `now_ms`. Returns
    /// whether anything ran.
    ///
    /// Every callback is a task with its own watchdog budget
    /// ([`TASK_BUDGET`]), followed by a microtask checkpoint with its own
    /// budget: a callback that runs out of time is stopped alone. After
    /// [`TICK_SLICE`] of timer work the call yields; timers still due stay
    /// due and run on the next call.
    pub fn run_timers(&mut self, env: &mut dyn HostEnv, now_ms: f64) -> bool {
        let own = self.enter(env, |realm| {
            let slice_end = std::time::Instant::now() + TICK_SLICE;
            state().borrow_mut().last_tick = now_ms;
            realm.begin_task();
            let mut ran = node::run_ready_script_fetches(realm);
            ran |= fetch::run_ready_fetches(realm);
            // Timers due, in order of due time then id.
            loop {
                if ran && std::time::Instant::now() >= slice_end {
                    // Yield to the event loop (input, rendering); the timers
                    // still due run on the next call.
                    break;
                }
                let next = {
                    let st = state();
                    let mut st = st.borrow_mut();
                    let mut best: Option<usize> = None;
                    for (i, t) in st.timers.iter().enumerate() {
                        if t.due > now_ms {
                            continue;
                        }
                        let better = match best {
                            None => true,
                            Some(b) => {
                                let tb = &st.timers[b];
                                t.due < tb.due || (t.due == tb.due && t.id < tb.id)
                            }
                        };
                        if better {
                            best = Some(i);
                        }
                    }
                    best.map(|i| {
                        let t = &mut st.timers[i];
                        let item = (t.callback.clone(), t.args.clone(), t.interval);
                        match t.interval {
                            Some(iv) => {
                                t.due = now_ms + iv;
                            }
                            None => {
                                st.timers.remove(i);
                            }
                        }
                        item
                    })
                };
                let Some((cb, args, interval)) = next else { break };
                ran = true;
                realm.begin_task();
                host::call_logged(realm, &cb, Value::Undefined, &args, "timer");
                realm.begin_task();
                shadow::flush(realm);
                if interval.is_none() {
                    host::unroot_value(realm, &cb);
                    for a in &args {
                        host::unroot_value(realm, a);
                    }
                }
                realm.run_jobs();
                if observers::has_pending_mutations() {
                    realm.begin_task();
                    observers::deliver_mutations(realm);
                }
                frames::checkpoint();
            }
            // Animation frames (~60 Hz).
            let frame_due = {
                let st = state();
                let st = st.borrow();
                !st.rafs.is_empty() && now_ms - st.last_frame >= 16.0
            };
            if frame_due {
                let rafs = {
                    let st = state();
                    let mut st = st.borrow_mut();
                    st.last_frame = now_ms;
                    std::mem::take(&mut st.rafs)
                };
                let origin = state().borrow().time_origin;
                for r in rafs {
                    ran = true;
                    realm.begin_task();
                    host::call_logged(realm, &r.callback, Value::Undefined, &[Value::Number(now_ms - origin)], "requestAnimationFrame");
                    realm.begin_task();
                    host::unroot_value(realm, &r.callback);
                    shadow::flush(realm);
                }
                realm.begin_task();
                realm.run_jobs();
                if observers::has_pending_mutations() {
                    realm.begin_task();
                    observers::deliver_mutations(realm);
                }
                frames::checkpoint();
            }
            // Network connections (WebSocket / EventSource).
            realm.begin_task();
            if sockets::has_connections() && sockets::poll(realm) {
                ran = true;
                realm.begin_task();
                shadow::flush(realm);
                realm.run_jobs();
            }
            // Update the rendering: Resize / Intersection observers.
            let origin = state().borrow().time_origin;
            realm.begin_task();
            if natives::update_rendering(realm, now_ms - origin) {
                ran = true;
                realm.begin_task();
                shadow::flush(realm);
                realm.run_jobs();
                if observers::has_pending_mutations() {
                    realm.begin_task();
                    observers::deliver_mutations(realm);
                }
            }
            ran
        });
        let workers = !self.workers.is_empty() || self.has_worker_work();
        own | (workers && self.pump_workers(env, now_ms))
    }

    /// Earliest time (ms, same clock as `now_ms`) at which a timer or
    /// animation frame is due, if any.
    pub fn next_due(&self) -> Option<f64> {
        let st = self.state.borrow();
        let mut due = st.timers.iter().map(|t| t.due).fold(None, |acc: Option<f64>, d| Some(acc.map(|a| a.min(d)).unwrap_or(d)));
        if !st.rafs.is_empty() {
            let frame = st.last_frame + 16.0;
            due = Some(due.map(|d| d.min(frame)).unwrap_or(frame));
        }
        if st.media_active() {
            // Media pipelines are polled by js/media.js timers; keep the
            // loop responsive.
            let soon = st.last_tick + 50.0;
            due = Some(due.map(|d| d.min(soon)).unwrap_or(soon));
        }
        if !st.sockets_idle() {
            // Poll open connections every ~16 ms.
            let soon = st.last_tick + 16.0;
            due = Some(due.map(|d| d.min(soon)).unwrap_or(soon));
        }
        if let Some(r) = natives::render_due(&st) {
            let r = r + st.time_origin;
            due = Some(due.map(|d| d.min(r)).unwrap_or(r));
        }
        if !st.workers.ops.is_empty() {
            due = Some(st.last_tick);
        }
        if !st.pending_script_fetches.is_empty() || !st.pending_fetches.is_empty() {
            // Poll background script downloads every ~10 ms.
            let soon = st.last_tick + 10.0;
            due = Some(due.map(|d| d.min(soon)).unwrap_or(soon));
        }
        drop(st);
        if self.has_worker_work() {
            let now = self.state.borrow().last_tick;
            due = Some(due.map(|d| d.min(now)).unwrap_or(now));
        }
        if let Some(w) = self.workers_next_due() {
            due = Some(due.map(|d| d.min(w)).unwrap_or(w));
        }
        due
    }

    /// Are there listeners for `event_type` on `node` (not counting ancestors)?
    pub fn has_listeners(&self, node: NodeId, event_type: &str) -> bool {
        let st = self.state.borrow();
        st.listener_counts.get(&(TargetKey::Node(NodeRef::main(node)), event_type.to_string())).map(|c| *c > 0).unwrap_or(false)
    }

    /// Does any node listen for `event_type` (anywhere), or the window?
    pub fn any_listener_for(&self, event_type: &str) -> bool {
        let st = self.state.borrow();
        st.listener_counts.iter().any(|((_, t), c)| t == event_type && *c > 0)
    }

    /// Does `node` have any listener at all?
    pub fn element_has_any_listener(&self, node: NodeId) -> bool {
        let st = self.state.borrow();
        st.listeners.get(&TargetKey::Node(NodeRef::main(node))).map(|l| !l.is_empty()).unwrap_or(false)
    }

    /// Evaluate `src` and format the result (or error) for a console UI.
    pub fn eval_for_console(&mut self, env: &mut dyn HostEnv, src: &str) -> String {
        self.enter(env, |realm| match realm.eval(src, "console") {
            Ok(v) => rabbithole_js::builtins::global::inspect(realm, &v, 0),
            Err(e) => realm.error_to_string(&e),
        })
    }

    /// Has script mutated the DOM / styles since the last `take_dirty`?
    pub fn dirty(&self) -> bool {
        self.state.borrow().dirty
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::replace(&mut self.state.borrow_mut().dirty, false)
    }

    /// Hygiene hook when the embedder removes nodes.
    pub fn node_removed(&mut self, node: NodeId) {
        let mut st = self.state.borrow_mut();
        let key = TargetKey::Node(NodeRef::main(node));
        st.listeners.remove(&key);
        st.listener_counts.retain(|(k, _), _| *k != key);
    }

    /// The currently focused element as seen by scripts.
    pub fn focused(&self) -> Option<NodeId> {
        self.state.borrow().focused
    }

    /// Tell scripts which element is focused (from the embedder's focus
    /// handling) without dispatching events.
    pub fn set_focused(&mut self, node: Option<NodeId>) {
        self.state.borrow_mut().focused = node;
    }

    /// The location `href` as scripts see it (after pushState).
    pub fn location_href(&self, env: &mut dyn HostEnv) -> String {
        match &self.state.borrow().location_override {
            Some(o) => o.clone(),
            None => env.document_url().to_string(),
        }
    }

    /// Force a garbage collection.
    pub fn gc(&mut self) {
        self.realm.gc();
    }

    /// Direct access to the realm (advanced embedders).
    pub fn realm(&mut self) -> &mut Realm {
        &mut self.realm
    }
}

impl Default for DispatchResult {
    fn default() -> Self {
        DispatchResult::nothing()
    }
}
