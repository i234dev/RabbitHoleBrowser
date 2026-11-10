//! The host environment: what the engine provides to scripts (DOM access,
//! layout queries, networking, storage, navigation, UI prompts).

use rabbithole_dom::{Document, NodeId};
use rabbithole_gfx::Rect;
use rabbithole_net::{Request, Response, Url};
use rabbithole_style::ComputedStyle;
use std::cell::RefCell;
use std::rc::Rc;

/// Services the embedding engine offers to the bindings. One instance is
/// passed into every `ScriptHost` entry point.
pub trait HostEnv {
    fn doc(&mut self) -> &mut Document;
    fn document_url(&self) -> Url;
    fn base_url(&self) -> Url;
    /// DOM or style changed: the engine should restyle + relayout lazily.
    fn mark_dom_dirty(&mut self);
    /// Force restyle + layout now (before geometry queries).
    fn ensure_layout(&mut self);
    /// Border box of a node in document coordinates (after `ensure_layout`).
    fn node_rect(&mut self, node: NodeId) -> Option<Rect>;
    /// Elements painted at a viewport point, topmost first (the engine's
    /// paint-order hit test). `None` = not supported (geometric fallback).
    fn elements_from_point(&mut self, _x: f32, _y: f32) -> Option<Vec<NodeId>> {
        None
    }
    fn computed_style(&mut self, node: NodeId) -> Option<Rc<ComputedStyle>>;
    fn viewport(&self) -> (f32, f32);
    fn scroll(&self) -> (f32, f32);
    fn set_scroll(&mut self, x: f32, y: f32);
    /// Synchronous resource fetch (same cookie jar / caches as the page).
    fn fetch(&mut self, req: Request) -> Result<Response, String>;
    /// Start a fetch on a background thread; the result arrives on the
    /// returned channel. `None` (the default) means "not supported": the
    /// caller falls back to the synchronous `fetch`.
    fn fetch_background(&mut self, _req: Request) -> Option<std::sync::mpsc::Receiver<Result<Response, String>>> {
        None
    }
    /// Open a WebSocket (`new WebSocket(url, protocols)`); the bindings
    /// poll it from the event loop. `None` (default) = unsupported.
    fn open_websocket(&mut self, _url: Url, _protocols: Vec<String>) -> Option<Result<rabbithole_net::WebSocket, String>> {
        None
    }
    /// Open an EventSource (`new EventSource(url, {withCredentials})`);
    /// polled like WebSockets. `None` (default) = unsupported.
    fn open_event_source(&mut self, _url: Url, _with_credentials: bool) -> Option<Result<rabbithole_net::EventSource, String>> {
        None
    }
    fn navigate(&mut self, url: Url, replace: bool);
    /// `document.cookie` getter.
    fn cookies(&mut self) -> String;
    /// `document.cookie` setter (one `Set-Cookie`-like string).
    fn set_cookie(&mut self, value: &str);
    fn storage_get(&mut self, local: bool, key: &str) -> Option<String>;
    /// `None` removes the key.
    fn storage_set(&mut self, local: bool, key: &str, value: Option<&str>);
    fn storage_keys(&mut self, local: bool) -> Vec<String>;
    fn log(&mut self, level: &str, msg: &str);
    fn alert(&mut self, msg: &str);
    fn confirm(&mut self, msg: &str) -> bool;
    fn prompt(&mut self, msg: &str, default: &str) -> Option<String>;
    fn user_agent(&self) -> String;
    fn title_changed(&mut self, title: &str);
    /// Monotonic milliseconds (for `performance.now()` and timers).
    fn now_ms(&self) -> f64;
    fn random_bytes(&mut self, buf: &mut [u8]);
    /// `window.open` / `target=_blank`.
    fn open_window(&mut self, url: Url);
    fn focus_node(&mut self, node: Option<NodeId>);
    /// Submit a form (build the request and navigate); `submitter` is the
    /// button used, if any.
    fn submit_form(&mut self, form: NodeId, submitter: Option<NodeId>);
    /// A script set an image element's `src`/`srcset`. Return true when the
    /// engine loads it and fires `load`/`error` itself (the default fakes a
    /// `load` event).
    fn image_src_changed(&mut self, _node: NodeId) -> bool {
        false
    }
    /// (naturalWidth, naturalHeight, complete, currentSrc) of an image
    /// element, when the engine tracks it.
    fn image_state(&mut self, _node: NodeId) -> Option<(u32, u32, bool, String)> {
        None
    }
}

type EnvPtr = *mut (dyn HostEnv + 'static);

thread_local! {
    /// Stack of active environments (re-entrant entry points push/pop).
    static ENV: RefCell<Vec<EnvPtr>> = const { RefCell::new(Vec::new()) };
}

/// RAII guard that makes `env` reachable from native functions for the
/// duration of an entry point.
pub(crate) struct EnvGuard;

impl EnvGuard {
    pub(crate) fn enter(env: &mut dyn HostEnv) -> EnvGuard {
        // SAFETY: the pointer is only dereferenced through `with_env` while
        // this guard is alive, i.e. while the `&mut dyn HostEnv` borrow that
        // produced it is still held by the calling entry point. The guard is
        // dropped before that borrow ends, and natives never store the
        // reference. The lifetime is erased only to put it in a thread-local.
        let ptr: EnvPtr = unsafe { std::mem::transmute::<*mut (dyn HostEnv + '_), EnvPtr>(env as *mut dyn HostEnv) };
        ENV.with(|e| e.borrow_mut().push(ptr));
        EnvGuard
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        ENV.with(|e| {
            e.borrow_mut().pop();
        });
    }
}

/// Run `f` with the current environment. Panics when called outside an
/// entry point (a bindings bug).
pub(crate) fn with_env<R>(f: impl FnOnce(&mut dyn HostEnv) -> R) -> R {
    let ptr = ENV.with(|e| e.borrow().last().copied()).expect("script host: no active environment");
    // SAFETY: see `EnvGuard::enter`; the pointer is valid while the guard
    // that pushed it is alive, and we are inside that guard's scope. The
    // stack discipline (push on enter, pop on drop) guarantees the innermost
    // pointer belongs to the innermost live entry point. Natives run on the
    // same thread and never hold two `&mut` to the env at once because
    // `with_env` closures are not nested across engine calls.
    let env: &mut dyn HostEnv = unsafe { &mut *ptr };
    f(env)
}

pub(crate) fn env_active() -> bool {
    ENV.with(|e| !e.borrow().is_empty())
}
