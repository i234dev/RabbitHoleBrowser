//! JavaScript integration: the engine implements [`HostEnv`] for a page and
//! drives the [`ScriptHost`] (script execution at load, timers, events).

use crate::{Engine, Page};
use rabbithole_dom::{Document, NodeId};
use rabbithole_gfx::Rect;
use rabbithole_net::{Request, Response, Url};
use rabbithole_script::{DispatchResult, EventDetail, EventInit, HostEnv, ScriptHost};
use rabbithole_style::ComputedStyle;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// A navigation requested by a script (`location.href = …`, `form.submit()`).
#[derive(Debug, Clone)]
pub struct PendingNavigation {
    pub request: Request,
    pub replace: bool,
}

/// A dialog a script asked for (`alert`, `confirm`, `prompt`).
#[derive(Debug, Clone)]
pub struct Dialog {
    pub kind: &'static str,
    pub message: String,
}

/// How script dialogs are answered. The default (used by the GUI) accepts
/// `confirm()` and cancels `prompt()`; embedders such as the agent
/// interface set their own policy per page.
#[derive(Debug, Clone)]
pub struct DialogPolicy {
    /// Return value of `confirm()`.
    pub confirm: bool,
    /// Answer to `prompt()`: `None` cancels, `Some("")` accepts the
    /// default value, `Some(text)` answers `text`.
    pub prompt: Option<String>,
}

impl Default for DialogPolicy {
    fn default() -> Self {
        DialogPolicy { confirm: true, prompt: None }
    }
}

/// Script-related state kept on a page.
#[derive(Default)]
pub struct ScriptState {
    pub host: Option<ScriptHost>,
    /// DOM changed by scripts since the last style/layout.
    pub dom_dirty: bool,
    pub pending_navigation: Option<PendingNavigation>,
    pub new_windows: Vec<Url>,
    pub console: Vec<(String, String)>,
    pub dialogs: Vec<Dialog>,
    pub scripts_ran: bool,
    /// Page scripts not run yet (load-time scripts run in slices, see
    /// [`Engine::run_page_scripts_for`]).
    pub(crate) pending_scripts: Option<std::collections::VecDeque<crate::ScriptSource>>,
    /// Session storage for this page's origin (tab-scoped).
    pub session_storage: HashMap<String, String>,
    /// Answers given to `confirm()` / `prompt()`.
    pub dialog_policy: DialogPolicy,
    /// `<link>` elements whose `load`/`error` event was queued, with the
    /// resource URL it was for (a new `href` fires again).
    pub link_loads: HashMap<NodeId, String>,
    /// `load`/`error` events for `<link>` elements, fired on the next tick.
    pub link_events: Vec<(NodeId, &'static str)>,
}

/// Origin-scoped `localStorage`, shared by all pages of an engine and
/// optionally persisted to a file.
#[derive(Default)]
pub struct LocalStorage {
    data: HashMap<String, HashMap<String, String>>,
    path: Option<std::path::PathBuf>,
    dirty: bool,
}

impl LocalStorage {
    pub fn load(path: std::path::PathBuf) -> LocalStorage {
        let mut ls = LocalStorage { path: Some(path.clone()), ..Default::default() };
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines() {
                let mut parts = line.splitn(3, '\t');
                if let (Some(o), Some(k), Some(v)) = (parts.next(), parts.next(), parts.next()) {
                    ls.data.entry(o.to_string()).or_default().insert(unescape(k), unescape(v));
                }
            }
        }
        ls
    }

    pub fn save(&mut self) {
        let Some(path) = &self.path else { return };
        if !self.dirty {
            return;
        }
        let mut out = String::new();
        for (o, m) in &self.data {
            for (k, v) in m {
                out.push_str(&format!("{}\t{}\t{}\n", o, escape(k), escape(v)));
            }
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, out);
        self.dirty = false;
    }

    pub fn get(&self, origin: &str, key: &str) -> Option<String> {
        self.data.get(origin)?.get(key).cloned()
    }

    pub fn set(&mut self, origin: &str, key: &str, value: Option<&str>) {
        let m = self.data.entry(origin.to_string()).or_default();
        match value {
            Some(v) => {
                m.insert(key.to_string(), v.to_string());
            }
            None => {
                m.remove(key);
            }
        }
        self.dirty = true;
    }

    pub fn keys(&self, origin: &str) -> Vec<String> {
        self.data.get(origin).map(|m| m.keys().cloned().collect()).unwrap_or_default()
    }

    pub fn clear_origin(&mut self, origin: &str) {
        self.data.remove(origin);
        self.dirty = true;
    }

    /// Origins with stored data.
    pub fn origins(&self) -> Vec<String> {
        self.data.iter().filter(|(_, m)| !m.is_empty()).map(|(o, _)| o.clone()).collect()
    }

    /// Forget everything (clear browsing data).
    pub fn clear_all(&mut self) {
        self.data.clear();
        self.dirty = true;
    }
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n")
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('\\') => out.push('\\'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The engine-side implementation of [`HostEnv`] for one page.
pub struct EngineEnv<'a> {
    pub engine: &'a Engine,
    pub page: &'a mut Page,
}

impl HostEnv for EngineEnv<'_> {
    fn doc(&mut self) -> &mut Document {
        &mut self.page.doc
    }
    fn document_url(&self) -> Url {
        self.page.url.clone()
    }
    fn base_url(&self) -> Url {
        self.page.base_url.clone()
    }
    fn mark_dom_dirty(&mut self) {
        self.page.script.dom_dirty = true;
    }
    fn ensure_layout(&mut self) {
        if self.page.script.dom_dirty {
            self.page.script.dom_dirty = false;
            let t = std::time::Instant::now();
            // Style + layout only; the display list waits for paint.
            self.engine.flush_layout(self.page);
            // The browser's own work does not count against the script.
            rabbithole_script::exclude_from_watchdog(t.elapsed());
            if std::env::var_os("RH_DEBUG_SCRIPTS").is_some() {
                eprintln!("[layout] forced restyle+layout {:?}", t.elapsed());
            }
        }
    }
    fn node_rect(&mut self, node: NodeId) -> Option<Rect> {
        self.ensure_layout();
        self.page.rect_of(node)
    }
    fn elements_from_point(&mut self, x: f32, y: f32) -> Option<Vec<NodeId>> {
        self.ensure_layout();
        let t = std::time::Instant::now();
        self.engine.flush_display_list(self.page);
        rabbithole_script::exclude_from_watchdog(t.elapsed());
        if x < 0.0 || y < 0.0 || x > self.page.viewport.width || y > self.page.viewport.height {
            return Some(Vec::new());
        }
        Some(self.page.nodes_at_viewport(x, y))
    }
    fn computed_style(&mut self, node: NodeId) -> Option<Rc<ComputedStyle>> {
        if !self.page.script.dom_dirty {
            return self.page.styles.get(node).cloned().or_else(|| self.engine.unrendered_style(self.page, node));
        }
        // The DOM changed: compute just this element's chain instead of
        // restyling and re-laying out the whole document (scripts often
        // interleave style writes and reads).
        let t = std::time::Instant::now();
        let r = self.engine.computed_style_now(self.page, node);
        rabbithole_script::exclude_from_watchdog(t.elapsed());
        if std::env::var_os("RH_DEBUG_SCRIPTS").is_some() {
            eprintln!("[layout] style_chain {:?}", t.elapsed());
        }
        r
    }
    fn element_scroll(&mut self, node: NodeId) -> Option<((f32, f32), (f32, f32))> {
        self.ensure_layout();
        self.engine.element_scroll(self.page, node)
    }
    fn set_element_scroll(&mut self, node: NodeId, x: f32, y: f32) -> bool {
        self.ensure_layout();
        let t = std::time::Instant::now();
        let moved = self.engine.set_element_scroll(self.page, node, x, y);
        rabbithole_script::exclude_from_watchdog(t.elapsed());
        moved
    }
    fn document_scroll_size(&mut self) -> Option<(f32, f32)> {
        self.ensure_layout();
        let vp = self.page.viewport;
        Some((self.page.content_width().max(vp.width), self.page.content_height().max(vp.height)))
    }
    fn viewport(&self) -> (f32, f32) {
        (self.page.viewport.width, self.page.viewport.height)
    }
    fn scroll(&self) -> (f32, f32) {
        self.page.page_scroll
    }
    fn set_scroll(&mut self, x: f32, y: f32) {
        self.page.requested_scroll = Some((x.max(0.0), y.max(0.0)));
    }
    fn fetch(&mut self, req: Request) -> Result<Response, String> {
        if !req.url.is_http() && req.url.scheme != "data" && req.url.scheme != "file" {
            return Err(format!("unsupported scheme {}", req.url.scheme));
        }
        // Waiting for the network is not script time.
        let t = std::time::Instant::now();
        let r = self.engine.loader.fetcher.request(&req).map_err(|e| e.to_string());
        rabbithole_script::exclude_from_watchdog(t.elapsed());
        r
    }
    fn fetch_background(&mut self, req: Request) -> Option<std::sync::mpsc::Receiver<Result<Response, String>>> {
        if !req.url.is_http() {
            return None;
        }
        let fetcher = self.engine.loader.fetcher.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(fetcher.request(&req).map_err(|e| e.to_string()));
        });
        Some(rx)
    }
    fn media_fetcher(&mut self) -> Option<rabbithole_net::Fetcher> {
        Some(self.engine.loader.fetcher.clone())
    }
    fn open_websocket(&mut self, url: Url, protocols: Vec<String>) -> Option<Result<rabbithole_net::WebSocket, String>> {
        Some(rabbithole_net::WebSocket::connect(&self.engine.loader.fetcher, &url, &protocols, Some(&self.page.url), None))
    }
    fn open_event_source(&mut self, url: Url, with_credentials: bool) -> Option<Result<rabbithole_net::EventSource, String>> {
        Some(rabbithole_net::EventSource::connect(&self.engine.loader.fetcher, &url, with_credentials, Some(&self.page.url), None))
    }
    fn image_pixels(&mut self, node: NodeId) -> Option<(u32, u32, Vec<u8>)> {
        self.page.images.get(&node).filter(|img| !img.broken).map(|img| (img.width, img.height, img.pixels().to_vec()))
    }
    fn set_canvas_image(&mut self, node: NodeId, width: u32, height: u32, rgba: Option<Vec<u8>>) {
        match rgba {
            Some(rgba) => {
                self.page.images.insert(node, std::sync::Arc::new(rabbithole_gfx::Image::new(width, height, rgba)));
            }
            None => {
                self.page.images.remove(&node);
            }
        }
        self.page.script.dom_dirty = true;
    }
    fn font_context(&mut self) -> Option<std::rc::Rc<rabbithole_gfx::FontContext>> {
        Some(self.engine.fonts.clone())
    }
    fn navigate(&mut self, url: Url, replace: bool) {
        self.page.script.pending_navigation = Some(PendingNavigation {
            request: Request::get(url).with_referer(self.page.url.clone()),
            replace,
        });
    }
    fn submit_form(&mut self, form: NodeId, submitter: Option<NodeId>) {
        if let Some(req) = self.page.submit_request(form, submitter, &|_| Vec::new()) {
            self.page.script.pending_navigation = Some(PendingNavigation { request: req, replace: false });
        }
    }
    fn cookies(&mut self) -> String {
        match &self.engine.loader.fetcher.cookies {
            Some(jar) => jar.lock().unwrap().script_cookies(&self.page.url),
            None => String::new(),
        }
    }
    fn set_cookie(&mut self, value: &str) {
        if let Some(jar) = &self.engine.loader.fetcher.cookies {
            jar.lock().unwrap().store_from_headers(&self.page.url, &[value.to_string()]);
        }
    }
    fn storage_get(&mut self, local: bool, key: &str) -> Option<String> {
        if local {
            self.engine.local_storage.lock().unwrap().get(&self.page.url.origin(), key)
        } else {
            self.page.script.session_storage.get(key).cloned()
        }
    }
    fn storage_set(&mut self, local: bool, key: &str, value: Option<&str>) {
        if local {
            self.engine.local_storage.lock().unwrap().set(&self.page.url.origin(), key, value);
        } else {
            match value {
                Some(v) => {
                    self.page.script.session_storage.insert(key.to_string(), v.to_string());
                }
                None => {
                    self.page.script.session_storage.remove(key);
                }
            }
        }
    }
    fn storage_keys(&mut self, local: bool) -> Vec<String> {
        if local {
            self.engine.local_storage.lock().unwrap().keys(&self.page.url.origin())
        } else {
            self.page.script.session_storage.keys().cloned().collect()
        }
    }
    fn log(&mut self, level: &str, msg: &str) {
        if self.engine.console_to_stderr {
            eprintln!("[js:{level}] {msg}");
        }
        if self.page.script.console.len() < 2000 {
            self.page.script.console.push((level.to_string(), msg.to_string()));
        }
    }
    fn alert(&mut self, msg: &str) {
        self.page.script.dialogs.push(Dialog { kind: "alert", message: msg.to_string() });
    }
    fn confirm(&mut self, msg: &str) -> bool {
        self.page.script.dialogs.push(Dialog { kind: "confirm", message: msg.to_string() });
        self.page.script.dialog_policy.confirm
    }
    fn prompt(&mut self, msg: &str, default: &str) -> Option<String> {
        self.page.script.dialogs.push(Dialog { kind: "prompt", message: msg.to_string() });
        match &self.page.script.dialog_policy.prompt {
            Some(t) if t.is_empty() => Some(default.to_string()),
            other => other.clone(),
        }
    }
    fn user_agent(&self) -> String {
        self.engine.loader.fetcher.user_agent.clone()
    }
    fn title_changed(&mut self, title: &str) {
        self.page.title = title.to_string();
    }
    fn now_ms(&self) -> f64 {
        self.engine.start.elapsed().as_secs_f64() * 1000.0
    }
    fn random_bytes(&mut self, buf: &mut [u8]) {
        fill_random(buf);
    }
    fn open_window(&mut self, url: Url) {
        self.page.script.new_windows.push(url);
    }
    fn image_src_changed(&mut self, node: NodeId) -> bool {
        self.page.image_state.script_changed(node);
        self.page.script.dom_dirty = true;
        true
    }
    fn image_state(&mut self, node: NodeId) -> Option<(u32, u32, bool, String)> {
        let i = self.page.image_info(node);
        Some((i.natural_width, i.natural_height, i.complete, i.current_src))
    }
    fn frame_id(&self) -> u64 {
        self.page.frame_id
    }
    fn frame_parent(&self, id: u64) -> Option<u64> {
        self.engine.frames.borrow().parent(id)
    }
    fn frame_children(&self, id: u64) -> Vec<u64> {
        if id == self.page.frame_id {
            return self.page.frames.children.iter().map(|c| c.id).collect();
        }
        self.engine.frames.borrow().children(id)
    }
    fn frame_exists(&self, id: u64) -> bool {
        id == self.page.frame_id || self.engine.frames.borrow().exists(id)
    }
    fn frame_url(&self, id: u64) -> Option<String> {
        if id == self.page.frame_id {
            return Some(self.page.url.to_string());
        }
        self.engine.frames.borrow().url(id)
    }
    fn navigate_frame(&mut self, id: u64, url: &str) {
        if let Some(u) = self.page.resolve(url) {
            if id == self.page.frame_id {
                self.navigate(u, false);
            } else {
                self.engine.frames.borrow_mut().navigations.push((id, u));
                crate::frames::set_mail_pending();
            }
        }
    }
    fn frame_of_element(&self, node: NodeId) -> Option<u64> {
        self.page.frames.get(node).map(|c| c.id)
    }
    fn frame_document(&mut self, id: u64) -> Option<&mut Document> {
        let origin = crate::frames::page_origin(self.page);
        if let Some(anc) = crate::frames::ancestor_page(id) {
            // The parent / top document (or a sibling frame's), reached
            // while this frame runs detached from it.
            if crate::frames::page_origin(anc) != origin {
                return None;
            }
            anc.script.dom_dirty = true;
            return Some(&mut anc.doc);
        }
        let child = crate::frames::find_frame_page_mut(self.page, id)?;
        if crate::frames::page_origin(child) != origin {
            return None;
        }
        // The caller may change it: restyle the frame on the next sync.
        child.script.dom_dirty = true;
        Some(&mut child.doc)
    }
    fn frame_computed_style(&mut self, id: u64, node: NodeId) -> Option<Rc<ComputedStyle>> {
        // The frame's media queries depend on its size, which depends on
        // this document's layout.
        self.ensure_layout();
        self.engine.update_frame_viewport(self.page, id);
        let origin = crate::frames::page_origin(self.page);
        if let Some(anc) = crate::frames::ancestor_page(id) {
            if crate::frames::page_origin(anc) != origin {
                return None;
            }
            if anc.script.dom_dirty {
                return self.engine.computed_style_now(anc, node);
            }
            return anc.styles.get(node).cloned();
        }
        let child = crate::frames::find_frame_page_mut(self.page, id)?;
        if crate::frames::page_origin(child) != origin {
            return None;
        }
        if child.script.dom_dirty {
            return self.engine.computed_style_now(child, node);
        }
        child.styles.get(node).cloned().or_else(|| self.engine.unrendered_style(child, node))
    }
    fn frame_node_rect(&mut self, id: u64, node: NodeId) -> Option<Rect> {
        self.ensure_layout();
        self.engine.update_frame_viewport(self.page, id);
        let origin = crate::frames::page_origin(self.page);
        let child = crate::frames::find_frame_page_mut(self.page, id)?;
        if crate::frames::page_origin(child) != origin {
            return None;
        }
        self.engine.flush_layout(child);
        child.rect_of(node)
    }
    fn ensure_frame(&mut self, node: NodeId) -> Option<u64> {
        if let Some(c) = self.page.frames.get(node) {
            if c.page.is_some() {
                return Some(c.id);
            }
        }
        self.engine.create_blank_frame(self.page, node)
    }
    fn frame_name(&self) -> Option<String> {
        self.page.frame_name.clone()
    }
    fn frame_name_of(&self, id: u64) -> Option<String> {
        if let Some(c) = self.page.frames.children.iter().find(|c| c.id == id) {
            return self.page.doc.attr(c.node, "name").map(|s| s.to_string());
        }
        self.engine.frames.borrow().name(id)
    }
    fn frame_named_child(&self, id: u64, name: &str) -> Option<u64> {
        if id == self.page.frame_id {
            return self.page.frames.children.iter().find(|c| self.page.doc.attr(c.node, "name") == Some(name)).map(|c| c.id);
        }
        self.engine.frames.borrow().named_child(id, name)
    }
    fn frame_element(&mut self) -> Option<(u64, NodeId)> {
        // The parent is the nearest page on the ancestor stack.
        let parent = crate::frames::ancestors().into_iter().next()?;
        // SAFETY: live while this frame runs (see `frames::ancestor_page`).
        let parent = unsafe { &*parent };
        if crate::frames::page_origin(parent) != crate::frames::page_origin(self.page) {
            return None;
        }
        let node = parent.frames.children.iter().find(|c| c.id == self.page.frame_id)?.node;
        Some((parent.frame_id, node))
    }
    fn post_message(&mut self, target: u64, data: String, target_origin: String) {
        self.queue_message(target, data, target_origin);
    }
    fn frame_call(&mut self, id: u64, call: rabbithole_script::FrameCall) -> Option<rabbithole_script::FrameReply> {
        self.engine.call_in_frame(self.page, id, call)
    }
    fn frame_checkpoint(&mut self) {
        self.engine.frame_microtasks(self.page);
    }
    fn focus_node(&mut self, node: Option<NodeId>) {
        self.page.state.focused = node;
        self.page.state.focus_visible = node.map(|n| self.page.focus_should_be_visible(n)).unwrap_or(false);
    }
}

/// Fill with pseudo-random bytes (OS entropy when available, else a
/// time-seeded xorshift).
pub fn fill_random(buf: &mut [u8]) {
    #[cfg(unix)]
    {
        use std::io::Read;
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            if f.read_exact(buf).is_ok() {
                return;
            }
        }
    }
    thread_local! {
        static STATE: std::cell::Cell<u64> = std::cell::Cell::new({
            use std::hash::{BuildHasher, Hasher};
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(1));
            h.finish() | 1
        });
    }
    STATE.with(|s| {
        let mut x = s.get();
        for b in buf.iter_mut() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = (x >> 24) as u8;
        }
        s.set(x);
    });
}

/// Result of a script-driven interaction step.
#[derive(Debug, Default)]
pub struct ScriptOutcome {
    /// Styles/layout were recomputed (repaint needed).
    pub relayout: bool,
    /// A listener called `preventDefault()`.
    pub default_prevented: bool,
    /// A navigation requested by scripts.
    pub navigation: Option<PendingNavigation>,
    /// `window.open` requests.
    pub new_windows: Vec<Url>,
    /// Dialogs requested by scripts.
    pub dialogs: Vec<Dialog>,
    /// Scroll position requested by scripts.
    pub scroll_to: Option<(f32, f32)>,
}

impl Engine {
    /// Run the page's scripts after load (classic in order, deferred after,
    /// async at the end), fire DOMContentLoaded/load and settle jobs.
    pub fn run_page_scripts(&self, page: &mut Page) -> ScriptOutcome {
        if !self.begin_page_scripts(page) {
            return ScriptOutcome::default();
        }
        loop {
            if let Some(out) = self.run_page_scripts_for(page, None) {
                return out;
            }
        }
    }

    /// Start running the page's load-time scripts: create the script host
    /// and queue the scripts (classic in order, deferred after, async at
    /// the end). Returns false when there is nothing to run. Continue with
    /// [`Engine::run_page_scripts_for`].
    pub fn begin_page_scripts(&self, page: &mut Page) -> bool {
        if !self.scripting || page.script.scripts_ran {
            return false;
        }
        page.script.scripts_ran = true;
        let scripts = std::mem::take(&mut page.scripts);
        let host = {
            let mut env = EngineEnv { engine: self, page };
            ScriptHost::new(&mut env)
        };
        page.script.host = Some(host);
        let mut classic = Vec::new();
        let mut deferred = Vec::new();
        let mut asyncs = Vec::new();
        for s in scripts {
            if s.source.trim().is_empty() {
                continue;
            }
            if s.is_async && s.url.is_some() {
                asyncs.push(s);
            } else if s.defer && s.url.is_some() || s.is_module {
                deferred.push(s);
            } else {
                classic.push(s);
            }
        }
        page.script.pending_scripts = Some(classic.into_iter().chain(deferred).chain(asyncs).collect());
        true
    }

    /// Are load-time scripts still waiting to run?
    pub fn page_scripts_pending(&self, page: &Page) -> bool {
        page.script.pending_scripts.is_some()
    }

    /// Run queued load-time scripts until `slice` has passed (at least one;
    /// `None`: all of them). The event loop can paint and handle input
    /// between slices, as a parser yielding between scripts would. Returns
    /// the outcome once the last script ran and DOMContentLoaded / load
    /// fired; None while scripts remain.
    pub fn run_page_scripts_for(&self, page: &mut Page, slice: Option<std::time::Duration>) -> Option<ScriptOutcome> {
        let mut queue = page.script.pending_scripts.take()?;
        let Some(mut host) = page.script.host.take() else {
            return Some(ScriptOutcome::default());
        };
        let start = std::time::Instant::now();
        {
            let mut env = EngineEnv { engine: self, page };
            // Watchdog: each script, listener and microtask checkpoint is a
            // task with its own budget (rabbithole_script::task_budget, plus
            // 2 s per MB for scripts; forced layout and synchronous loads
            // do not count), so a runaway script is stopped without failing
            // the scripts after it.
            while let Some(s) = queue.pop_front() {
                let name = s.url.as_ref().map(|u| u.to_string()).unwrap_or_else(|| format!("{}#inline", env.page.url));
                let debug = std::env::var_os("RH_DEBUG_SCRIPTS").is_some();
                if debug {
                    eprintln!("[script] start {} ({} bytes)", name, s.source.len());
                }
                let t = std::time::Instant::now();
                host.set_current_script(Some(s.node));
                let _ = host.run_script(&mut env, &s.source, &name, s.is_module);
                host.set_current_script(None);
                host.run_jobs(&mut env);
                if debug {
                    eprintln!("[script] done  {} in {:?}", name, t.elapsed());
                }
                if slice.is_some_and(|d| start.elapsed() >= d) && !queue.is_empty() {
                    break;
                }
            }
            if queue.is_empty() {
                host.document_ready(&mut env);
                host.run_jobs(&mut env);
            }
            host.realm().set_deadline(None);
        }
        page.script.host = Some(host);
        if queue.is_empty() {
            Some(self.settle(page, false))
        } else {
            // The embedder paints what the scripts built so far (painting
            // flushes style and layout); requests made by the scripts stay
            // on the page for the final settle.
            page.script.pending_scripts = Some(queue);
            None
        }
    }

    /// Run due timers, rAF callbacks and microtasks. Call from the UI loop;
    /// `Page::next_script_due` says when the next call is needed.
    pub fn tick(&self, page: &mut Page) -> ScriptOutcome {
        // Images first: install finished background loads, fire their
        // events and advance animations.
        let mut images_changed = self.poll_images(page);
        let mut img_out = ScriptOutcome::default();
        if images_changed {
            self.relayout_only(page);
            self.shrink_oversized_images(page);
            img_out = self.fire_image_events(page);
        }
        if self.advance_animations(page) {
            images_changed = true;
        }
        // CSS transitions / animations, then their events (before
        // animation frame callbacks, as in "update the rendering").
        if self.advance_css_animations(page) {
            images_changed = true;
        }
        let anim_out = self.dispatch_animation_events(page);
        let link_out = self.fire_link_events(page);
        let mut out = self.tick_scripts(page);
        out.merge(link_out);
        out.merge(anim_out);
        let frames_out = self.tick_frames(page);
        out.merge(frames_out);
        out.relayout |= images_changed || img_out.relayout;
        if out.navigation.is_none() {
            out.navigation = img_out.navigation;
        }
        out.new_windows.extend(img_out.new_windows);
        out.dialogs.extend(img_out.dialogs);
        if out.scroll_to.is_none() {
            out.scroll_to = img_out.scroll_to;
        }
        out
    }

    fn tick_scripts(&self, page: &mut Page) -> ScriptOutcome {
        let Some(mut host) = page.script.host.take() else {
            return ScriptOutcome::default();
        };
        let now = self.start.elapsed().as_secs_f64() * 1000.0;
        {
            let mut env = EngineEnv { engine: self, page };
            // run_timers gives every callback its own watchdog budget and
            // yields after a time slice (timers still due run next tick).
            if std::env::var_os("RH_DEBUG_SCRIPTS").is_some() {
                eprintln!("[tick] run_timers start");
            }
            let ran = host.run_timers(&mut env, now);
            if std::env::var_os("RH_DEBUG_SCRIPTS").is_some() {
                eprintln!("[tick] run_timers done (ran={ran})");
            }
            if ran {
                host.run_jobs(&mut env);
            }
            host.realm().set_deadline(None);
        }
        page.script.host = Some(host);
        self.settle(page, false)
    }

    /// Queue `load` / `error` events for `<link>` elements whose resource
    /// finished (stylesheets: once fetched into the cache by the last
    /// style flush; `modulepreload` / `preload` / `prefetch`: right away).
    /// Loaders such as Vite's preload helper await a stylesheet link's
    /// `load` before importing the module that needs it.
    pub(crate) fn queue_link_events(&self, page: &mut Page) {
        if page.script.host.is_none() {
            return;
        }
        let mut roots = vec![Document::ROOT];
        roots.extend(page.doc.connected_shadow_roots());
        let mut found: Vec<(NodeId, String, bool)> = Vec::new();
        for root in roots {
            for n in page.doc.descendants(root) {
                let Some(el) = page.doc.element(n) else { continue };
                if el.tag != "link" {
                    continue;
                }
                let rel = el.attr("rel").unwrap_or("").to_ascii_lowercase();
                let sheet = rel.split_ascii_whitespace().any(|r| r == "stylesheet");
                let preload = rel.split_ascii_whitespace().any(|r| matches!(r, "modulepreload" | "preload" | "prefetch"));
                if !(sheet || preload) || el.attr("disabled").is_some() {
                    continue;
                }
                let Some(href) = el.attr("href").map(str::trim).filter(|h| !h.is_empty()) else { continue };
                let Some(url) = page.base_url.join(href) else { continue };
                let key = url.without_fragment().to_string();
                if page.script.link_loads.get(&n) == Some(&key) {
                    continue;
                }
                found.push((n, key, sheet));
            }
        }
        if found.is_empty() {
            return;
        }
        let caches = self.loader.caches.lock().unwrap();
        for (n, key, sheet) in found {
            let ev = if sheet && self.loader.load_stylesheets {
                if !caches.css.contains_key(&key) {
                    // Not fetched (yet): the next flush that collects it fires.
                    continue;
                }
                if caches.css_failed.contains(&key) {
                    "error"
                } else {
                    "load"
                }
            } else {
                "load"
            };
            page.script.link_loads.insert(n, key);
            page.script.link_events.push((n, ev));
        }
    }

    /// Fire the queued `<link>` `load` / `error` events.
    fn fire_link_events(&self, page: &mut Page) -> ScriptOutcome {
        let events = std::mem::take(&mut page.script.link_events);
        let mut out = ScriptOutcome::default();
        for (n, ev) in events {
            if !page.doc.is_connected(n) {
                continue;
            }
            let o = self.dispatch_event(page, n, ev, EventInit { bubbles: false, cancelable: false, detail: None });
            out.merge(o);
        }
        out
    }

    /// Dispatch a DOM event to scripts. Returns whether the default action
    /// was prevented (plus any relayout/navigation side effects).
    pub fn dispatch_event(&self, page: &mut Page, target: NodeId, event_type: &str, init: EventInit) -> ScriptOutcome {
        let Some(mut host) = page.script.host.take() else {
            return ScriptOutcome::default();
        };
        if event_type == "scroll" {
            // A scroll container moved: observers rooted in it (or whose
            // targets it holds) must be checked again.
            host.invalidate_rendering();
        }
        // Skip the dispatch entirely when no listener for this type exists
        // anywhere (the event could bubble to any ancestor or the window).
        // Clicks always go through so inline `onclick` attributes and
        // default-action bookkeeping (`element.click()`) work.
        if !host.any_listener_for(event_type) && !host.has_listeners(target, event_type) && !matches!(event_type, "click" | "submit") {
            page.script.host = Some(host);
            return ScriptOutcome::default();
        }
        let result: DispatchResult = {
            let mut env = EngineEnv { engine: self, page };
            host.realm().begin_task();
            let r = host.dispatch_event(&mut env, target, event_type, init);
            host.run_jobs(&mut env);
            host.realm().set_deadline(None);
            r
        };
        page.script.host = Some(host);
        let mut out = self.settle(page, result.default_prevented);
        out.default_prevented = result.default_prevented;
        out
    }

    /// Dispatch a window-level event (`resize`, `scroll`, `hashchange`, ...).
    pub fn dispatch_window_event(&self, page: &mut Page, event_type: &str) -> ScriptOutcome {
        let Some(mut host) = page.script.host.take() else {
            return ScriptOutcome::default();
        };
        {
            let mut env = EngineEnv { engine: self, page };
            host.realm().begin_task();
            host.dispatch_window_event(&mut env, event_type);
            host.run_jobs(&mut env);
            host.realm().set_deadline(None);
        }
        page.script.host = Some(host);
        self.settle(page, false)
    }

    /// Evaluate JavaScript in the page (devtools console / CLI).
    pub fn eval_in_page(&self, page: &mut Page, src: &str) -> String {
        self.eval_with_outcome(page, src).0
    }

    /// Evaluate JavaScript in the page and report its side effects
    /// (navigation, relayout, ...), e.g. for `javascript:` links.
    pub fn eval_with_outcome(&self, page: &mut Page, src: &str) -> (String, ScriptOutcome) {
        let Some(mut host) = page.script.host.take() else {
            // No scripts ran: create a host on demand.
            let h = {
                let mut env = EngineEnv { engine: self, page };
                ScriptHost::new(&mut env)
            };
            page.script.host = Some(h);
            return self.eval_with_outcome(page, src);
        };
        let out = {
            let mut env = EngineEnv { engine: self, page };
            host.realm().set_deadline(Some(std::time::Instant::now() + std::time::Duration::from_secs(10)));
            let s = host.eval_for_console(&mut env, src);
            host.run_jobs(&mut env);
            host.realm().set_deadline(None);
            s
        };
        page.script.host = Some(host);
        let outcome = self.settle(page, false);
        (out, outcome)
    }

    /// After script activity: restyle if the DOM changed and collect
    /// side effects.
    pub(crate) fn settle(&self, page: &mut Page, default_prevented: bool) -> ScriptOutcome {
        let mut out = ScriptOutcome { default_prevented, ..Default::default() };
        let host_dirty = page.script.host.as_mut().map(|h| h.take_dirty()).unwrap_or(false);
        if page.script.dom_dirty || host_dirty {
            page.script.dom_dirty = false;
            let t = std::time::Instant::now();
            if std::env::var_os("RH_DEBUG_SCRIPTS").is_some() {
                eprintln!("[settle-engine] restyle start");
            }
            self.restyle(page);
            if std::env::var_os("RH_DEBUG_SCRIPTS").is_some() {
                eprintln!("[settle-engine] restyle done in {:?}", t.elapsed());
            }
            out.relayout = true;
        } else if page.incr.dirty() {
            // Layout was flushed for a script query; finish the display list.
            self.relayout_only(page);
            out.relayout = true;
        }
        if !page.frames.children.is_empty() && self.sync_dirty_frames(page) {
            out.relayout = true;
        }
        out.navigation = page.script.pending_navigation.take();
        out.new_windows = std::mem::take(&mut page.script.new_windows);
        out.dialogs = std::mem::take(&mut page.script.dialogs);
        out.scroll_to = page.requested_scroll.take();
        if let Ok(mut ls) = self.local_storage.lock() {
            ls.save();
        }
        out
    }
}

impl Page {
    /// Earliest time (engine ms) a timer or animation frame is due.
    pub fn next_script_due(&self) -> Option<f64> {
        let scripts = self.script.host.as_ref().and_then(|h| h.next_due());
        let mut due = match (scripts, self.image_due()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        if let Some(a) = self.anim.next_due() {
            due = Some(due.map(|d| d.min(a)).unwrap_or(a));
        }
        if !self.script.link_events.is_empty() {
            due = Some(0.0);
        }
        let frames = if crate::frames::mail_pending() { Some(0.0) } else { self.frames_next_due() };
        if let Some(f) = frames {
            due = Some(due.map(|d| d.min(f)).unwrap_or(f));
        }
        due
    }

    /// Does the page have any script listeners at all (hover/click
    /// dispatch can be skipped otherwise)?
    pub fn has_script_listeners(&self, node: NodeId) -> bool {
        self.script.host.as_ref().map(|h| h.element_has_any_listener(node)).unwrap_or(false)
    }

    /// Console messages (level, text) produced so far; cleared on read.
    pub fn take_console(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.script.console)
    }

    pub fn has_scripts(&self) -> bool {
        self.script.host.is_some()
    }
}

/// Build a mouse event init for a click at document coordinates.
pub fn mouse_init(x: f32, y: f32, button: u8, ctrl: bool, shift: bool, alt: bool, meta: bool) -> EventInit {
    EventInit {
        bubbles: true,
        cancelable: true,
        detail: Some(EventDetail::Mouse { x, y, button, ctrl, shift, alt, meta }),
    }
}

/// Build a keyboard event init.
pub fn key_init(key: &str, code: &str, ctrl: bool, shift: bool, alt: bool, meta: bool, repeat: bool) -> EventInit {
    EventInit {
        bubbles: true,
        cancelable: true,
        detail: Some(EventDetail::Key { key: key.to_string(), code: code.to_string(), ctrl, shift, alt, meta, repeat }),
    }
}

/// Build an input event init.
pub fn input_init(data: Option<&str>) -> EventInit {
    EventInit { bubbles: true, cancelable: false, detail: Some(EventDetail::Input { data: data.map(|d| d.to_string()) }) }
}

/// Plain bubbling event init (change, submit, ...).
pub fn plain_init(cancelable: bool) -> EventInit {
    EventInit { bubbles: true, cancelable, detail: None }
}

#[allow(dead_code)]
fn _unused(_: &RefCell<()>) {}
