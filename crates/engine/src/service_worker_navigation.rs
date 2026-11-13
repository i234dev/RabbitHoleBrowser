//! Controlled top-level GET navigation. Realms and reserved clients stay on the
//! Engine thread; embedders may send only the owned response to a loader thread.
use crate::{Engine, EngineError, Page, Viewport};
use rabbithole_dom::{Document, NodeId};
use rabbithole_gfx::Rect;
use rabbithole_net::{Request, Response, Url};
use rabbithole_script::HostEnv;
use rabbithole_style::ComputedStyle;
use std::rc::Rc;

/// No provisional Page or ScriptHost is created: existing worker realms only
/// need network, clocks and their own empty worker documents while navigating.
struct NavigationEnv<'a> {
    engine: &'a Engine,
    doc: Document,
    url: Url,
    viewport: Viewport,
    console: Vec<(String, String)>,
}
impl HostEnv for NavigationEnv<'_> {
    fn service_worker_registry(&self) -> Option<rabbithole_script::ServiceWorkerRegistry> { Some(self.engine.service_workers.clone()) }
    fn doc(&mut self) -> &mut Document { &mut self.doc }
    fn document_url(&self) -> Url { self.url.clone() }
    fn base_url(&self) -> Url { self.url.clone() }
    fn mark_dom_dirty(&mut self) {}
    fn ensure_layout(&mut self) {}
    fn node_rect(&mut self, _: NodeId) -> Option<Rect> { None }
    fn computed_style(&mut self, _: NodeId) -> Option<Rc<ComputedStyle>> { None }
    fn viewport(&self) -> (f32, f32) { (self.viewport.width, self.viewport.height) }
    fn scroll(&self) -> (f32, f32) { (0.0, 0.0) }
    fn set_scroll(&mut self, _: f32, _: f32) {}
    fn fetch(&mut self, request: Request) -> Result<Response, String> { self.engine.loader.fetcher.request(&request).map_err(|e| e.to_string()) }
    fn fetch_background(&mut self, request: Request) -> Option<std::sync::mpsc::Receiver<Result<Response, String>>> {
        if !request.url.is_http() { return None; }
        let fetcher = self.engine.loader.fetcher.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || { let _ = tx.send(fetcher.request(&request).map_err(|e| e.to_string())); });
        Some(rx)
    }
    fn font_context(&mut self) -> Option<Rc<rabbithole_gfx::FontContext>> { Some(self.engine.fonts.clone()) }
    fn navigate(&mut self, _: Url, _: bool) {}
    fn cookies(&mut self) -> String { String::new() }
    fn set_cookie(&mut self, _: &str) {}
    fn storage_get(&mut self, _: bool, _: &str) -> Option<String> { None }
    fn storage_set(&mut self, _: bool, _: &str, _: Option<&str>) {}
    fn storage_keys(&mut self, _: bool) -> Vec<String> { Vec::new() }
    fn log(&mut self, level: &str, message: &str) {
        if self.engine.console_to_stderr { eprintln!("[js:{level}] {message}"); }
        self.console.push((level.into(), message.into()));
    }
    fn alert(&mut self, _: &str) {}
    fn confirm(&mut self, _: &str) -> bool { false }
    fn prompt(&mut self, _: &str, _: &str) -> Option<String> { None }
    fn user_agent(&self) -> String { self.engine.loader.fetcher.user_agent.clone() }
    fn title_changed(&mut self, _: &str) {}
    fn now_ms(&self) -> f64 { self.engine.start.elapsed().as_secs_f64() * 1000.0 }
    fn random_bytes(&mut self, bytes: &mut [u8]) { crate::scripting::fill_random(bytes); }
    fn open_window(&mut self, _: Url) {}
    fn focus_node(&mut self, _: Option<NodeId>) {}
    fn submit_form(&mut self, _: NodeId, _: Option<NodeId>) {}
}

pub(crate) fn load(engine: &mut Engine, url: Url, viewport: Viewport) -> Result<Option<Page>, EngineError> {
    if engine.frame_context.borrow().is_some() || !engine.scripting || !engine.service_workers.controls_navigation(&url) { return Ok(None); }
    let (navigation, console) = {
        let mut env = NavigationEnv { engine, doc: Document::new(), url: url.clone(), viewport, console: Vec::new() };
        let navigation = engine.service_workers.navigate(&mut env, url, engine.loader.fetcher.timeout).map_err(EngineError::Fetch)?;
        (navigation, env.console)
    };
    let Some(navigation) = navigation else { return Ok(None) };
    let loaded = engine.loader.load_response(navigation.response, viewport);
    let mut page = engine.build_page_unscripted(loaded, viewport);
    page.script.service_worker_client = Some(navigation.client);
    page.script.console.extend(console);
    engine.finish_page_load(&mut page);
    // A controlled Window is execution-ready even when its HTML has no script.
    // Adopt its reserved client before another worker can call clients.claim().
    engine.ensure_script_host(&mut page);
    Ok(Some(page))
}

/// A nonblocking navigation operation local to the Engine's owning thread.
pub struct PendingServiceWorkerNavigation {
    inner: rabbithole_script::ServiceWorkerNavigationHandle,
    url: Url,
    viewport: Viewport,
    console: Vec<(String,String)>,
}
impl PendingServiceWorkerNavigation {
    pub fn take_console(&mut self) -> Vec<(String,String)> { std::mem::take(&mut self.console) }
}
impl Engine {
    /// Returns None for unsupported methods, inactive scopes or disabled scripting.
    pub fn begin_service_worker_navigation(&self, request: Request, viewport: Viewport) -> Result<Option<PendingServiceWorkerNavigation>, EngineError> {
        if self.frame_context.borrow().is_some() || !self.scripting { return Ok(None); }
        let url = request.url.clone();
        self.service_workers.begin_navigation(request, self.loader.fetcher.timeout).map(|operation| operation.map(|inner| PendingServiceWorkerNavigation { inner, url, viewport, console: Vec::new() })).map_err(EngineError::Fetch)
    }
    /// Pumps existing worker realms once, without waiting for network or timers.
    pub fn poll_service_worker_navigation(&self, navigation: &mut PendingServiceWorkerNavigation) -> std::task::Poll<Result<rabbithole_script::ServiceWorkerNavigation, EngineError>> {
        if !self.scripting { return std::task::Poll::Ready(Err(EngineError::Fetch("Scripting was disabled during navigation".into()))); }
        let mut env = NavigationEnv { engine: self, doc: Document::new(), url: navigation.url.clone(), viewport: navigation.viewport, console: Vec::new() };
        let result = navigation.inner.poll(&mut env).map(|result| result.map_err(EngineError::Fetch));
        navigation.console.extend(env.console);
        result
    }
    /// Bind only after the loader's matching generation becomes the real page.
    pub fn bind_service_worker_navigation_client(&self, page: &mut Page, client: rabbithole_script::ServiceWorkerClient) {
        page.script.service_worker_client = Some(client);
        // The committed document owns its real client before deferred external
        // scripts arrive. Keep this realm when starting those scripts later.
        if page.script.host.is_none() {
            let host = { let mut env = crate::scripting::EngineEnv { engine: self, page }; rabbithole_script::ScriptHost::new(&mut env) };
            page.script.host = Some(host);
        }
    }
}
