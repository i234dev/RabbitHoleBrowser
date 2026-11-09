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
    fn computed_style(&mut self, node: NodeId) -> Option<Rc<ComputedStyle>>;
    fn viewport(&self) -> (f32, f32);
    fn scroll(&self) -> (f32, f32);
    fn set_scroll(&mut self, x: f32, y: f32);
    /// Synchronous resource fetch (same cookie jar / caches as the page).
    fn fetch(&mut self, req: Request) -> Result<Response, String>;
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
