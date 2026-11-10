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
mod host;
mod misc;
mod modules;
mod node;
mod observers;
mod window;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_webapi;

pub use env::HostEnv;
pub use host::{NodeRef, TargetKey};

use env::{with_env, EnvGuard};
use host::{state, HostState, StateGuard};
use rabbithole_dom::{Document, NodeId};
use rabbithole_js::{Realm, Value};
use rabbithole_net::Url;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Extra data carried by a dispatched event.
#[derive(Debug, Clone, PartialEq)]
pub enum EventDetail {
    Mouse { x: f32, y: f32, button: u8, ctrl: bool, shift: bool, alt: bool, meta: bool },
    Key { key: String, code: String, ctrl: bool, shift: bool, alt: bool, meta: bool, repeat: bool },
    Input { data: Option<String> },
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

/// The scripting host of one page.
pub struct ScriptHost {
    realm: Realm,
    state: Rc<RefCell<HostState>>,
}

impl ScriptHost {
    /// Create the realm and install `window`, `document` and all globals.
    pub fn new(env: &mut dyn HostEnv) -> ScriptHost {
        let mut realm = Realm::new();
        let now = env.now_ms();
        let state = Rc::new(RefCell::new(HostState {
            wrappers: HashMap::new(),
            protos: HashMap::new(),
            listeners: HashMap::new(),
            listener_counts: HashMap::new(),
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
            ran_scripts: HashSet::new(),
            inline_handlers: HashMap::new(),
            observers: Vec::new(),
            mutation_records: Vec::new(),
            mutation_scheduled: false,
            custom_elements: HashMap::new(),
            ce_constructing: None,
            ce_upgraded: std::collections::HashSet::new(),
            location_override: None,
            history_len: 1,
            history_state: Value::Null,
            event_stack: Vec::new(),
            time_origin: now,
            console_marks: HashMap::new(),
            console_counts: HashMap::new(),
            pending_intersections: Vec::new(),
            pending_resizes: Vec::new(),
            pending_script_events: Vec::new(),
            pending_script_fetches: Vec::new(),
            deferred_natives: Vec::new(),
            modules: Default::default(),
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
        let mut host = ScriptHost { realm, state };
        {
            let _g = EnvGuard::enter(env);
            let _s = StateGuard::enter(host.state.clone());
            let realm = &mut host.realm;
            events::install(realm);
            node::install(realm);
            element::install(realm);
            document::install(realm);
            window::install(realm);
            cssom::install(realm);
            fetch::install(realm);
            misc::install(realm);
            observers::install(realm);
            modules::install(realm);
            // Web APIs written in JavaScript (DOMMatrix, ...).
            if let Err(e) = realm.eval(include_str!("prelude.js"), "rabbithole:prelude.js") {
                let msg = realm.error_to_string(&e);
                eprintln!("[rabbithole] prelude failed: {msg}");
            }
            // The document wrapper.
            let doc_obj = host::wrap_main(realm, Document::ROOT);
            if let Value::Object(d) = doc_obj {
                host.state.borrow_mut().document_obj = d;
            }
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
        if is_module {
            return self.enter(env, |realm| modules::run_module_script(realm, source, url));
        }
        self.enter(env, |realm| {
            let r = realm.eval(source, url);
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
    pub fn mark_script_ran(&mut self, node: NodeId) {
        self.state.borrow_mut().ran_scripts.insert(node);
    }

    /// Dispatch an event to a node (bubbling through its ancestors and the
    /// window). Returns whether the default was prevented and the default
    /// action the embedder should perform.
    pub fn dispatch_event(&mut self, env: &mut dyn HostEnv, target: NodeId, event_type: &str, init: EventInit) -> DispatchResult {
        self.enter(env, |realm| events::dispatch(realm, TargetKey::Node(NodeRef::main(target)), event_type, init, true))
    }

    /// Dispatch an event on `window` (`load`, `resize`, `scroll`, ...).
    pub fn dispatch_window_event(&mut self, env: &mut dyn HostEnv, event_type: &str) -> DispatchResult {
        self.enter(env, |realm| events::dispatch(realm, TargetKey::Window, event_type, EventInit::simple(false, false), true))
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
            realm.run_jobs();
            state().borrow_mut().ready_state = "complete";
            events::dispatch(realm, TargetKey::Node(NodeRef::main(Document::ROOT)), "readystatechange", EventInit::simple(false, false), true);
            events::dispatch(realm, TargetKey::Window, "load", EventInit::simple(false, false), true);
            events::dispatch(realm, TargetKey::Window, "pageshow", EventInit::simple(false, false), true);
            realm.run_jobs();
        });
    }

    /// Drain microtasks, queued callbacks and mutation observer records.
    pub fn run_jobs(&mut self, env: &mut dyn HostEnv) {
        self.enter(env, |realm| {
            for _ in 0..16 {
                realm.run_jobs();
                if observers::has_pending_mutations() {
                    observers::deliver_mutations(realm);
                }
                if !realm.has_pending_jobs() && !observers::has_pending_mutations() {
                    break;
                }
            }
            for r in realm.take_unhandled_rejections() {
                let msg = realm.error_to_string(&r);
                host::log("error", &format!("Uncaught (in promise) {msg}"));
            }
        });
    }

    /// Run timers and animation frames that are due at `now_ms`. Returns
    /// whether anything ran.
    pub fn run_timers(&mut self, env: &mut dyn HostEnv, now_ms: f64) -> bool {
        self.enter(env, |realm| {
            state().borrow_mut().last_tick = now_ms;
            let mut ran = node::run_ready_script_fetches(realm);
            // Timers due, in order of due time then id.
            loop {
                let next = {
                    let st = state();
                    let mut st = st.borrow_mut();
                    let mut due: Vec<usize> = st.timers.iter().enumerate().filter(|(_, t)| t.due <= now_ms).map(|(i, _)| i).collect();
                    if due.is_empty() {
                        None
                    } else {
                        due.sort_by(|a, b| {
                            let ta = &st.timers[*a];
                            let tb = &st.timers[*b];
                            ta.due.partial_cmp(&tb.due).unwrap_or(std::cmp::Ordering::Equal).then(ta.id.cmp(&tb.id))
                        });
                        let i = due[0];
                        let t = &mut st.timers[i];
                        let item = (t.id, t.callback.clone(), t.args.clone(), t.interval);
                        match t.interval {
                            Some(iv) => {
                                t.due = now_ms + iv;
                            }
                            None => {
                                st.timers.remove(i);
                            }
                        }
                        Some(item)
                    }
                };
                let Some((id, cb, args, interval)) = next else { break };
                ran = true;
                host::call_logged(realm, &cb, Value::Undefined, &args, "timer");
                if interval.is_none() {
                    host::unroot_value(realm, &cb);
                    for a in &args {
                        host::unroot_value(realm, a);
                    }
                }
                let _ = id;
                realm.run_jobs();
                if observers::has_pending_mutations() {
                    observers::deliver_mutations(realm);
                }
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
                    host::call_logged(realm, &r.callback, Value::Undefined, &[Value::Number(now_ms - origin)], "requestAnimationFrame");
                    host::unroot_value(realm, &r.callback);
                }
                realm.run_jobs();
                if observers::has_pending_mutations() {
                    observers::deliver_mutations(realm);
                }
            }
            ran
        })
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
        if !st.pending_script_fetches.is_empty() {
            // Poll background script downloads every ~10 ms.
            let soon = st.last_tick + 10.0;
            due = Some(due.map(|d| d.min(soon)).unwrap_or(soon));
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
