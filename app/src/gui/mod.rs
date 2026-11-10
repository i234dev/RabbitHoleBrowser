//! The graphical browser: window, tabs, asynchronous navigation and the
//! glue between winit events and the engine. Painting lives in
//! `render.rs`, input handling in `interact.rs`.

pub mod chrome;
pub mod console;
pub mod downloads;
pub mod input;
mod interact;
pub mod loader;
pub mod pages;
mod render;
pub mod tab;

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rabbithole_engine::dom::NodeId;
use rabbithole_engine::gfx::{Canvas, Family, FontId, Rect};
use rabbithole_engine::net::{CookieJar, Request, Url};
use rabbithole_engine::script::EventInit;
use rabbithole_engine::scripting::{input_init, plain_init, ScriptOutcome};
use rabbithole_engine::{Engine, Page, Viewport};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{MouseButton, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, Fullscreen, Window, WindowAttributes, WindowId};

use crate::bookmarks::Bookmarks;
use crate::history::History;
use crate::profile::{clamp_zoom, Profile, Session, ZoomLevels};
use crate::settings::Settings;
use chrome::{Menu, ToolButton};
use downloads::{DownloadItem, DownloadState, Downloads};
use input::TextBuffer;
use loader::{LoadJob, LoadOutcome, UserEvent};
use tab::{ControlEdit, Tab};

/// Which text field receives keyboard input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiFocus {
    Page,
    Url,
    Find,
    Console,
}

/// A pressed mouse button awaiting release.
pub struct MouseDown {
    pub button: MouseButton,
    pub pos: PhysicalPosition<f64>,
    /// Document point under the press (if over the page).
    pub doc: Option<(f32, f32)>,
    pub link: Option<Url>,
    pub dragged: bool,
}

/// Run the GUI. `start_urls` empty → restore the last session.
pub fn run(start_urls: Vec<String>) -> Result<(), String> {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|e| format!("could not create event loop: {e}"))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let mut app = App::new(start_urls, proxy);
    event_loop.run_app(&mut app).map_err(|e| format!("event loop error: {e}"))
}

pub struct App {
    pub(crate) engine: Engine,
    pub(crate) profile: Profile,
    pub(crate) cookies: Arc<Mutex<CookieJar>>,
    pub(crate) history: History,
    pub(crate) bookmarks: Bookmarks,
    pub(crate) zoom_levels: ZoomLevels,
    pub(crate) downloads: Downloads,
    pub(crate) next_download_id: Arc<AtomicU64>,
    pub(crate) tabs: Vec<Tab>,
    pub(crate) active: usize,
    pub(crate) next_tab_id: u64,
    pub(crate) window: Option<Rc<Window>>,
    pub(crate) surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    pub(crate) proxy: EventLoopProxy<UserEvent>,
    pub(crate) canvas: Canvas,
    pub(crate) page_canvas: Canvas,
    pub(crate) page_dirty: bool,
    pub(crate) ui_focus: UiFocus,
    pub(crate) url_field: TextBuffer,
    /// The URL field shows user-typed text (not the page URL).
    pub(crate) url_edited: bool,
    pub(crate) suggestions: Vec<(String, String)>,
    pub(crate) suggestion_sel: Option<usize>,
    pub(crate) menu: Option<Menu>,
    pub(crate) context_menu: Option<Menu>,
    pub(crate) dropdown: Option<Menu>,
    pub(crate) hover_button: Option<ToolButton>,
    pub(crate) hover_tab: Option<usize>,
    pub(crate) hover_tab_close: bool,
    pub(crate) hover_plus: bool,
    pub(crate) status: Option<(String, Instant)>,
    pub(crate) hover_link: Option<Url>,
    pub(crate) modifiers: ModifiersState,
    pub(crate) cursor: PhysicalPosition<f64>,
    pub(crate) cursor_icon: CursorIcon,
    pub(crate) mouse_down: Option<MouseDown>,
    /// Element under the pointer when the left button went down.
    pub(crate) mouse_down_node: Option<NodeId>,
    pub(crate) last_click: Option<(Instant, PhysicalPosition<f64>)>,
    pub(crate) clipboard: Option<arboard::Clipboard>,
    pub(crate) start_urls: Vec<String>,
    pub(crate) ui_font: FontId,
    pub(crate) ui_font_bold: FontId,
    pub(crate) anim_start: Instant,
    pub(crate) fullscreen: bool,
    pub(crate) needs_relayout: bool,
    /// Fragment to scroll to after the next layout.
    pub(crate) pending_fragment: Option<String>,
    pub(crate) settings: Settings,
    /// The JavaScript console pane is open.
    pub(crate) console_open: bool,
    /// Transient banners for script dialogs (`alert`, ...).
    pub(crate) toasts: Vec<(String, Instant)>,
    /// Last time a `scroll` event was dispatched to scripts.
    pub(crate) last_scroll_dispatch: Instant,
    /// A page was rebuilt by scripts since the last draw (hit tests must be fresh).
    pub(crate) script_relayout: bool,
}

impl App {
    fn new(start_urls: Vec<String>, proxy: EventLoopProxy<UserEvent>) -> App {
        let profile = Profile::locate();
        let cookies = profile.load_cookies();
        let mut engine = Engine::new();
        engine.loader.fetcher.cookies = Some(cookies.clone());
        let settings = Settings::load(&profile.settings_path());
        engine.scripting = settings.javascript;
        engine.loader.load_scripts = settings.javascript;
        engine.loader.load_images = settings.images;
        engine.loader.fetcher.block_trackers = settings.block_trackers;
        engine.set_local_storage_path(profile.local_storage_path());
        let ui_font = engine.fonts.select(Family::Sans, 400, false);
        let ui_font_bold = engine.fonts.select(Family::Sans, 700, false);
        let history = History::load(&profile.history_path());
        let bookmarks = Bookmarks::load(&profile.bookmarks_path());
        let zoom_levels = ZoomLevels::load(&profile.zoom_path());
        App {
            engine,
            profile,
            cookies,
            history,
            bookmarks,
            zoom_levels,
            downloads: Downloads::default(),
            next_download_id: Arc::new(AtomicU64::new(1)),
            tabs: Vec::new(),
            active: 0,
            next_tab_id: 1,
            window: None,
            surface: None,
            proxy,
            canvas: Canvas::new(1, 1),
            page_canvas: Canvas::new(1, 1),
            page_dirty: true,
            ui_focus: UiFocus::Page,
            url_field: TextBuffer::default(),
            url_edited: false,
            suggestions: Vec::new(),
            suggestion_sel: None,
            menu: None,
            context_menu: None,
            dropdown: None,
            hover_button: None,
            hover_tab: None,
            hover_tab_close: false,
            hover_plus: false,
            status: None,
            hover_link: None,
            modifiers: ModifiersState::empty(),
            cursor: PhysicalPosition::new(0.0, 0.0),
            cursor_icon: CursorIcon::Default,
            mouse_down: None,
            last_click: None,
            clipboard: None,
            start_urls,
            ui_font,
            ui_font_bold,
            anim_start: Instant::now(),
            fullscreen: false,
            needs_relayout: false,
            pending_fragment: None,
            settings,
            console_open: false,
            toasts: Vec::new(),
            last_scroll_dispatch: Instant::now(),
            script_relayout: false,
            mouse_down_node: None,
        }
    }

    // ------------------------------------------------------------ geometry

    pub(crate) fn window_size(&self) -> PhysicalSize<u32> {
        self.window.as_ref().map(|w| w.inner_size()).unwrap_or(PhysicalSize::new(1200, 800))
    }

    /// UI scale (device pixels per logical pixel).
    pub(crate) fn ui_scale(&self) -> f32 {
        self.window.as_ref().map(|w| w.scale_factor() as f32).unwrap_or(1.0).max(0.5)
    }

    pub(crate) fn chrome(&self) -> chrome::Chrome {
        let size = self.window_size();
        chrome::Chrome {
            s: self.ui_scale(),
            width: size.width.max(1) as f32,
            height: size.height.max(1) as f32,
            find_open: self.tabs.get(self.active).map(|t| t.find.open).unwrap_or(false),
            fullscreen: self.fullscreen,
            console_open: self.console_open,
        }
    }

    /// Device pixels per CSS pixel for the active tab.
    pub(crate) fn page_scale(&self) -> f32 {
        self.ui_scale() * self.tabs.get(self.active).map(|t| t.zoom).unwrap_or(1.0)
    }

    /// CSS viewport of the active tab.
    pub(crate) fn css_viewport(&self) -> Viewport {
        let area = self.chrome().page_area();
        let s = self.page_scale();
        Viewport { width: (area.w / s).max(1.0), height: (area.h / s).max(1.0) }
    }

    /// Physical window position → CSS viewport coordinates (if over the page).
    pub(crate) fn to_css(&self, pos: PhysicalPosition<f64>) -> Option<(f32, f32)> {
        let area = self.chrome().page_area();
        let (x, y) = (pos.x as f32, pos.y as f32);
        if !area.contains(x, y) {
            return None;
        }
        let s = self.page_scale();
        Some(((x - area.x) / s, (y - area.y) / s))
    }

    /// CSS viewport coordinates → document coordinates.
    pub(crate) fn to_doc(&self, css: (f32, f32)) -> (f32, f32) {
        let scroll = self.tabs.get(self.active).map(|t| t.scroll).unwrap_or((0.0, 0.0));
        (css.0 + scroll.0, css.1 + scroll.1)
    }

    /// Document rect → physical window rect.
    pub(crate) fn doc_to_screen(&self, r: Rect) -> Rect {
        let area = self.chrome().page_area();
        let s = self.page_scale();
        let scroll = self.tabs.get(self.active).map(|t| t.scroll).unwrap_or((0.0, 0.0));
        Rect::new(area.x + (r.x - scroll.0) * s, area.y + (r.y - scroll.1) * s, r.w * s, r.h * s)
    }

    pub(crate) fn tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    pub(crate) fn tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    pub(crate) fn page(&self) -> Option<&Page> {
        self.tabs.get(self.active).and_then(|t| t.page.as_ref())
    }

    pub(crate) fn request_redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    pub(crate) fn mark_page_dirty(&mut self) {
        self.page_dirty = true;
        self.request_redraw();
    }

    pub(crate) fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
        self.request_redraw();
    }

    // ---------------------------------------------------------------- tabs

    /// Open a new tab (optionally in the background) and navigate it.
    pub(crate) fn new_tab(&mut self, url: Option<&str>, background: bool) -> usize {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let tab = Tab::new(id);
        let insert_at = if self.tabs.is_empty() { 0 } else { self.active + 1 };
        self.tabs.insert(insert_at, tab);
        if !background || self.tabs.len() == 1 {
            self.activate_tab(insert_at);
        }
        let target = url.unwrap_or("about:home").to_string();
        self.navigate_tab(insert_at, &target, true);
        if !background {
            self.focus_url_field();
            if target != "about:home" {
                self.ui_focus = UiFocus::Page;
                self.url_field.clear_selection();
            }
        }
        insert_at
    }

    pub(crate) fn activate_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        self.active = idx;
        self.close_popups();
        self.ui_focus = UiFocus::Page;
        self.url_edited = false;
        self.sync_url_field();
        self.update_title();
        self.needs_relayout = true;
        self.mark_page_dirty();
    }

    pub(crate) fn close_tab(&mut self, idx: usize, event_loop: &ActiveEventLoop) {
        if idx >= self.tabs.len() {
            return;
        }
        self.tabs.remove(idx);
        if self.tabs.is_empty() {
            self.shutdown(event_loop);
            return;
        }
        let new_active = if idx <= self.active { self.active.saturating_sub(if idx < self.active { 1 } else { 0 }) } else { self.active };
        self.activate_tab(new_active.min(self.tabs.len() - 1));
    }

    pub(crate) fn update_title(&self) {
        if let (Some(w), Some(t)) = (&self.window, self.tab()) {
            let title = if t.title.trim().is_empty() { "RabbitHole".to_string() } else { format!("{} — RabbitHole", t.title.trim()) };
            w.set_title(&title);
        }
    }

    /// Put the active tab's URL in the address bar (unless the user is typing).
    pub(crate) fn sync_url_field(&mut self) {
        if self.ui_focus == UiFocus::Url && self.url_edited {
            return;
        }
        let text = self.tab().map(|t| t.display_url()).unwrap_or_default();
        self.url_field.set_text(&text);
        self.url_edited = false;
    }

    pub(crate) fn focus_url_field(&mut self) {
        self.ui_focus = UiFocus::Url;
        self.url_field.select_all();
        self.url_edited = false;
        self.suggestions.clear();
        self.suggestion_sel = None;
        self.request_redraw();
    }

    pub(crate) fn close_popups(&mut self) {
        self.menu = None;
        self.context_menu = None;
        self.dropdown = None;
        self.suggestions.clear();
        self.suggestion_sel = None;
    }

    // ---------------------------------------------------------- navigation

    /// Turn typed text into a URL (search fallback).
    pub(crate) fn resolve_input(input: &str) -> String {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return "about:home".to_string();
        }
        if looks_like_url(trimmed) {
            trimmed.to_string()
        } else {
            format!("https://duckduckgo.com/html/?q={}", percent_encode_query(trimmed))
        }
    }

    /// Navigate a tab to a URL string (GET).
    pub(crate) fn navigate_tab(&mut self, idx: usize, url: &str, push_history: bool) {
        let Some(parsed) = Url::parse(url) else {
            let html = pages::error_page(url, "The address could not be understood.");
            self.show_internal(idx, &html, url, push_history);
            return;
        };
        let referer = self.tabs.get(idx).and_then(|t| t.current_url.clone());
        let mut req = Request::get(parsed);
        if let Some(r) = referer {
            req = req.with_referer(r);
        }
        self.navigate_request(idx, req, push_history);
    }

    /// Navigate a tab with an arbitrary request (form submissions use POST).
    pub(crate) fn navigate_request(&mut self, idx: usize, req: Request, push_history: bool) {
        if idx >= self.tabs.len() {
            return;
        }
        let url = req.url.clone();
        if url.scheme == "rabbithole" {
            self.handle_internal_action(idx, &url);
            return;
        }
        if url.scheme == "about" && url.path != "home" {
            self.show_about_page(idx, &url, push_history);
            return;
        }
        if url.scheme == "mailto" {
            self.set_status(format!("Mail links are not supported: {url}"));
            return;
        }
        if url.scheme == "javascript" {
            if let Some(page) = self.tabs.get_mut(idx).and_then(|t| t.page.as_mut()) {
                let result = crate::scripting::run_javascript_url(&self.engine, page, &url);
                let _ = result;
                self.after_script_activity(idx);
            }
            return;
        }
        // Tell the old page it is going away.
        if self.tabs.get(idx).and_then(|t| t.page.as_ref()).map(|p| p.has_scripts()).unwrap_or(false) {
            for ev in ["beforeunload", "pagehide", "unload"] {
                if let Some(page) = self.tabs.get_mut(idx).and_then(|t| t.page.as_mut()) {
                    let _ = self.engine.dispatch_window_event(page, ev);
                }
            }
        }
        let viewport = self.css_viewport();
        let tab = &mut self.tabs[idx];
        tab.generation += 1;
        tab.loading = Some(url.clone());
        tab.find.matches.clear();
        if idx == self.active {
            self.ui_focus = UiFocus::Page;
            self.url_edited = false;
            self.url_field.set_text(&url.to_string());
            self.close_popups();
        }
        let job = LoadJob {
            loader: self.engine.loader.clone(),
            request: req,
            viewport,
            tab_id: self.tabs[idx].id,
            generation: self.tabs[idx].generation,
            push_history,
            download_dir: self.profile.downloads_dir(),
            next_download_id: self.next_download_id.clone(),
        };
        loader::spawn(job, self.proxy.clone());
        self.anim_start = Instant::now();
        self.request_redraw();
    }

    /// Show generated HTML in a tab as if it were loaded from `url`.
    fn show_internal(&mut self, idx: usize, html: &str, url: &str, push_history: bool) {
        if idx >= self.tabs.len() {
            return;
        }
        let parsed = Url::parse(url).unwrap_or_else(about_url);
        let viewport = self.css_viewport();
        let page = self.engine.load_html(html, parsed.clone(), viewport);
        self.tabs[idx].generation += 1;
        self.tabs[idx].loading = None;
        self.install_page(idx, page, parsed, push_history);
    }

    fn show_about_page(&mut self, idx: usize, url: &Url, push_history: bool) {
        let html = match url.path.as_str() {
            "history" => pages::history_page(&self.history),
            "bookmarks" => pages::bookmarks_page(&self.bookmarks),
            "downloads" => pages::downloads_page(&self.downloads.rows()),
            "version" => pages::version_page(self.engine.fonts.face_count(), &self.profile, self.cookies.lock().map(|j| j.len()).unwrap_or(0)),
            "text" => {
                let (u, t) = self.tabs.get(idx).and_then(|t| t.page.as_ref()).map(|p| (p.url.to_string(), p.text())).unwrap_or_default();
                pages::text_page(&u, &t)
            }
            "blank" | "" => pages::blank_page(),
            other => pages::error_page(&format!("about:{other}"), "Unknown internal page."),
        };
        self.show_internal(idx, &html, &url.to_string(), push_history);
    }

    /// `rabbithole://...` links from the internal pages.
    fn handle_internal_action(&mut self, idx: usize, url: &Url) {
        let path = url.path.trim_start_matches('/').to_string();
        match url.host.as_str() {
            "download" => {
                if let Some(id) = path.strip_prefix("cancel/").and_then(|s| s.parse::<u64>().ok()) {
                    self.downloads.cancel(id);
                    self.set_status("Download cancelled");
                }
                self.show_about_page(idx, &about_page("downloads"), false);
            }
            "history" => {
                if path == "clear" {
                    self.history.clear(&self.profile.history_path());
                }
                self.show_about_page(idx, &about_page("history"), false);
            }
            "bookmark" => {
                if path == "remove" {
                    if let Some(q) = &url.query {
                        let target = rabbithole_engine::net::url::percent_decode(q);
                        self.bookmarks.items.retain(|b| b.url != target);
                        self.bookmarks.save(&self.profile.bookmarks_path());
                    }
                }
                self.show_about_page(idx, &about_page("bookmarks"), false);
            }
            _ => self.set_status(format!("Unknown action: {url}")),
        }
    }

    /// Finish a navigation: install the page in the tab and update state.
    fn install_page(&mut self, idx: usize, page: Page, request_url: Url, push_history: bool) {
        let host = page.url.host.clone();
        let fragment = page.url.fragment.clone();
        let title = if page.title.trim().is_empty() { page.url.to_string() } else { page.title.trim().to_string() };
        let url_str = page.url.to_string();
        {
            let tab = &mut self.tabs[idx];
            tab.reset_page_state();
            tab.title = title.clone();
            tab.current_url = Some(page.url.clone());
            tab.loading = None;
            if push_history {
                tab.push_history(page.url.clone());
            } else if tab.history.is_empty() {
                tab.push_history(page.url.clone());
            } else if let Some(h) = tab.history.get_mut(tab.index) {
                // Redirects: keep the final URL in the history entry.
                *h = page.url.clone();
            }
            tab.zoom = self.zoom_levels.get(&host);
            tab.page = Some(page);
            tab.needs_relayout = true;
        }
        let _ = request_url;
        self.drain_console(idx);
        self.history.add(&url_str, &title);
        self.history.flush(&self.profile.history_path());
        self.profile.save_cookies(&self.cookies);
        if idx == self.active {
            self.sync_url_field();
            self.update_title();
            self.needs_relayout = true;
            if let Some(f) = fragment {
                self.pending_fragment = Some(f);
            }
            self.mark_page_dirty();
        }
    }

    /// Scroll so the element with `fragment` id is at the top.
    pub(crate) fn scroll_to_fragment(&mut self, fragment: &str) {
        let target = fragment.trim_start_matches('#');
        if target.is_empty() || target == "top" {
            self.set_scroll(0.0, 0.0);
            return;
        }
        let rect = self.page().and_then(|p| p.anchor_rect(target));
        if let Some(r) = rect {
            self.set_scroll(0.0, r.y);
        }
    }

    pub(crate) fn go_history(&mut self, delta: isize) {
        let Some(tab) = self.tab() else { return };
        let target = tab.index as isize + delta;
        if target < 0 || target as usize >= tab.history.len() {
            return;
        }
        let idx = self.active;
        self.tabs[idx].index = target as usize;
        let url = self.tabs[idx].history[target as usize].to_string();
        self.navigate_tab(idx, &url, false);
    }

    pub(crate) fn reload(&mut self, bypass_cache: bool) {
        if bypass_cache {
            self.engine.loader.clear_caches();
        }
        let idx = self.active;
        if let Some(url) = self.tabs.get(idx).and_then(|t| t.current_url.clone()) {
            self.navigate_tab(idx, &url.to_string(), false);
        }
    }

    pub(crate) fn stop_loading(&mut self) {
        let idx = self.active;
        if let Some(tab) = self.tabs.get_mut(idx) {
            if tab.loading.take().is_some() {
                tab.generation += 1;
                self.sync_url_field();
                self.set_status("Stopped");
            }
        }
    }

    /// Follow a link from the active tab.
    pub(crate) fn follow_link(&mut self, link: Url, new_tab: bool) {
        if new_tab {
            self.new_tab(Some(&link.to_string()), true);
            return;
        }
        let same_doc = self.page().map(|p| p.url.without_fragment() == link.without_fragment()).unwrap_or(false);
        if let (Some(fragment), true) = (&link.fragment, same_doc) {
            let fragment = fragment.clone();
            if let Some(tab) = self.tab_mut() {
                tab.current_url = Some(link.clone());
                tab.push_history(link.clone());
            }
            self.scroll_to_fragment(&fragment);
            self.sync_url_field();
            self.dispatch_window("hashchange");
            return;
        }
        let idx = self.active;
        self.navigate_tab(idx, &link.to_string(), true);
    }

    pub(crate) fn submit_url_field(&mut self) {
        let text = match self.suggestion_sel.and_then(|i| self.suggestions.get(i)) {
            Some((url, _)) => url.clone(),
            None => App::resolve_input(&self.url_field.text),
        };
        self.ui_focus = UiFocus::Page;
        self.close_popups();
        let idx = self.active;
        self.navigate_tab(idx, &text, true);
    }

    // ------------------------------------------------------------ scrolling

    pub(crate) fn set_scroll(&mut self, x: f32, y: f32) {
        let vp = self.css_viewport();
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let nx = x.clamp(0.0, tab.max_scroll_x(vp.width));
        let ny = y.clamp(0.0, tab.max_scroll(vp.height));
        if (nx, ny) != tab.scroll {
            tab.scroll = (nx, ny);
            if let Some(page) = tab.page.as_mut() {
                self.engine.set_page_scroll(page, nx, ny);
            }
            self.mark_page_dirty();
            self.update_hover();
            if self.last_scroll_dispatch.elapsed() >= Duration::from_millis(16) {
                self.last_scroll_dispatch = Instant::now();
                self.dispatch_window("scroll");
            }
        }
    }

    pub(crate) fn scroll_by(&mut self, dx: f32, dy: f32) {
        let (x, y) = self.tab().map(|t| t.scroll).unwrap_or((0.0, 0.0));
        self.set_scroll(x + dx, y + dy);
    }

    /// Ensure a document rect is visible (vertically).
    pub(crate) fn scroll_into_view(&mut self, r: Rect) {
        let vp = self.css_viewport();
        let (sx, sy) = self.tab().map(|t| t.scroll).unwrap_or((0.0, 0.0));
        if r.y < sy {
            self.set_scroll(sx, r.y - 20.0);
        } else if r.bottom() > sy + vp.height {
            self.set_scroll(sx, r.bottom() - vp.height + 20.0);
        }
    }

    pub(crate) fn set_zoom(&mut self, zoom: f32) {
        let zoom = clamp_zoom(zoom);
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        if (tab.zoom - zoom).abs() < 0.001 {
            return;
        }
        tab.zoom = zoom;
        tab.needs_relayout = true;
        if let Some(host) = tab.current_url.as_ref().map(|u| u.host.clone()) {
            self.zoom_levels.set(&host, zoom);
            self.zoom_levels.save(&self.profile.zoom_path());
        }
        self.needs_relayout = true;
        self.set_status(format!("Zoom {}%", (zoom * 100.0).round()));
        self.mark_page_dirty();
    }

    /// Relayout the active tab for the current viewport if needed.
    pub(crate) fn relayout_if_needed(&mut self) {
        let vp = self.css_viewport();
        // srcset / resolution media queries follow HiDPI scale and zoom.
        let dpr = self.page_scale();
        if (self.engine.device_pixel_ratio - dpr).abs() > 1e-3 {
            self.engine.set_device_pixel_ratio(dpr);
        }
        let idx = self.active;
        let Some(tab) = self.tabs.get_mut(idx) else { return };
        let needs = self.needs_relayout || tab.needs_relayout;
        self.needs_relayout = false;
        tab.needs_relayout = false;
        let mut resized = false;
        if let Some(page) = tab.page.as_mut() {
            if needs || page.viewport != vp {
                resized = page.viewport != vp;
                self.engine.relayout(page, vp);
                self.page_dirty = true;
            }
        }
        if resized {
            self.dispatch_window("resize");
        }
        let Some(tab) = self.tabs.get(idx) else { return };
        let (x, y) = tab.scroll;
        let (mx, my) = (tab.max_scroll_x(vp.width), tab.max_scroll(vp.height));
        if x > mx || y > my {
            self.set_scroll(x.min(mx), y.min(my));
        }
        if let Some(f) = self.pending_fragment.take() {
            self.scroll_to_fragment(&f);
        }
        if let Some(tab) = self.tabs.get_mut(idx) {
            if tab.find.open && !tab.find.field.is_empty() {
                self.refresh_find();
            }
        }
    }

    // ---------------------------------------------------------------- forms

    /// Apply a DOM mutation on the active page and restyle.
    pub(crate) fn mutate_page(&mut self, f: impl FnOnce(&mut Page)) {
        let idx = self.active;
        let Some(tab) = self.tabs.get_mut(idx) else { return };
        if let Some(page) = tab.page.as_mut() {
            f(page);
            self.engine.restyle(page);
            let (x, y) = tab.scroll;
            self.engine.set_page_scroll(page, x, y);
        }
        self.mark_page_dirty();
    }

    /// Start editing a text control.
    pub(crate) fn begin_edit(&mut self, node: NodeId) {
        let idx = self.active;
        let Some(tab) = self.tabs.get_mut(idx) else { return };
        let Some(page) = tab.page.as_mut() else { return };
        let kind = page.control_kind(node);
        let text = page.control_text(node);
        let max_len = page.max_length(node);
        self.engine.set_focus(page, Some(node));
        tab.edit = Some(ControlEdit { node, buf: TextBuffer::new(&text), kind, max_len, initial: text.clone() });
        tab.focus = Some(node);
        tab.selection = None;
        self.ui_focus = UiFocus::Page;
        self.mark_page_dirty();
        self.dispatch_to(node, "focus", EventInit::simple(false, false));
        self.dispatch_to(node, "focusin", EventInit::simple(true, false));
    }

    pub(crate) fn end_edit(&mut self) {
        let idx = self.active;
        let Some(tab) = self.tabs.get_mut(idx) else { return };
        if let Some(edit) = tab.edit.take() {
            if let Some(page) = tab.page.as_mut() {
                self.engine.set_focus(page, None);
            }
            tab.focus = None;
            self.mark_page_dirty();
            if edit.buf.text != edit.initial {
                self.dispatch_to(edit.node, "change", plain_init(false));
            }
            self.dispatch_to(edit.node, "blur", EventInit::simple(false, false));
            self.dispatch_to(edit.node, "focusout", EventInit::simple(true, false));
        }
    }

    /// Push the edit buffer into the DOM.
    pub(crate) fn commit_edit(&mut self) {
        let idx = self.active;
        let Some(tab) = self.tabs.get_mut(idx) else { return };
        let Some(edit) = tab.edit.as_ref() else { return };
        let (node, text) = (edit.node, edit.buf.text.clone());
        if let Some(page) = tab.page.as_mut() {
            page.set_control_text(node, &text);
            self.engine.restyle(page);
            let (x, y) = tab.scroll;
            self.engine.set_page_scroll(page, x, y);
        }
        self.mark_page_dirty();
        self.dispatch_to(node, "input", input_init(Some(&text)));
    }

    /// Submit the form containing `control` (or the control itself when it
    /// is a submit button).
    pub(crate) fn submit_form(&mut self, control: NodeId, submitter: Option<NodeId>) {
        let idx = self.active;
        let Some(page) = self.tabs.get(idx).and_then(|t| t.page.as_ref()) else { return };
        let Some(form) = page.form_of(control) else {
            self.set_status("This control is not inside a form");
            return;
        };
        let submitter = submitter.or_else(|| page.default_submitter(form));
        let out = self.dispatch_to(form, "submit", plain_init(true));
        if out.default_prevented || out.navigation.is_some() {
            return;
        }
        let Some(page) = self.tabs.get(idx).and_then(|t| t.page.as_ref()) else { return };
        let Some(req) = page.submit_request(form, submitter, &|_| Vec::new()) else { return };
        self.end_edit();
        self.navigate_request(idx, req, true);
    }

    /// Toggle a checkbox/radio and tell scripts.
    pub(crate) fn toggle_control_with_events(&mut self, node: NodeId) {
        self.mutate_page(|p| p.toggle_control(node));
        self.dispatch_to(node, "input", input_init(None));
        self.dispatch_to(node, "change", plain_init(false));
    }

    /// Choose a `<select>` option and tell scripts.
    pub(crate) fn select_option_with_events(&mut self, node: NodeId, index: usize) {
        self.mutate_page(|p| p.select_option(node, index));
        self.dispatch_to(node, "input", input_init(None));
        self.dispatch_to(node, "change", plain_init(false));
    }

    // ------------------------------------------------------------ scripting

    /// Dispatch an event to a node of the active page and apply the outcome.
    /// Returns the outcome (with `default_prevented`).
    pub(crate) fn dispatch_to(&mut self, node: NodeId, event_type: &str, init: EventInit) -> ScriptOutcome {
        let idx = self.active;
        let Some(page) = self.tabs.get_mut(idx).and_then(|t| t.page.as_mut()) else { return ScriptOutcome::default() };
        if !page.has_scripts() {
            return ScriptOutcome::default();
        }
        let out = self.engine.dispatch_event(page, node, event_type, init);
        let prevented = out.default_prevented;
        let navigated = out.navigation.is_some();
        self.apply_outcome(idx, out);
        ScriptOutcome { default_prevented: prevented, navigation: if navigated { Some(dummy_navigation()) } else { None }, ..Default::default() }
    }

    /// Dispatch a window-level event (`resize`, `scroll`, `hashchange`).
    pub(crate) fn dispatch_window(&mut self, event_type: &str) {
        let idx = self.active;
        let Some(page) = self.tabs.get_mut(idx).and_then(|t| t.page.as_mut()) else { return };
        if !page.has_scripts() {
            return;
        }
        let out = self.engine.dispatch_window_event(page, event_type);
        self.apply_outcome(idx, out);
    }

    /// Apply what scripts asked for after an event/timer: relayout,
    /// navigation, new tabs, dialogs, scrolling.
    pub(crate) fn apply_outcome(&mut self, idx: usize, out: ScriptOutcome) {
        self.drain_console(idx);
        if out.relayout {
            if let Some(tab) = self.tabs.get_mut(idx) {
                let (x, y) = tab.scroll;
                if let Some(page) = tab.page.as_mut() {
                    self.engine.set_page_scroll(page, x, y);
                }
            }
            if idx == self.active {
                let vp = self.css_viewport();
                if let Some(tab) = self.tabs.get_mut(idx) {
                    let (x, y) = tab.scroll;
                    let (mx, my) = (tab.max_scroll_x(vp.width), tab.max_scroll(vp.height));
                    if x > mx || y > my {
                        tab.scroll = (x.min(mx), y.min(my));
                    }
                    // Keep an edit buffer in sync when scripts changed the value.
                    if let (Some(edit), Some(page)) = (tab.edit.as_mut(), tab.page.as_ref()) {
                        let now = page.control_text(edit.node);
                        if now != edit.buf.text {
                            edit.buf.set_text(&now);
                        }
                    }
                    if tab.find.open && !tab.find.field.is_empty() {
                        self.refresh_find();
                    }
                }
                self.script_relayout = true;
                self.mark_page_dirty();
            }
        }
        for (i, u) in out.new_windows.iter().enumerate() {
            self.new_tab(Some(&u.to_string()), true);
            let _ = i;
        }
        for d in out.dialogs {
            let note = match d.kind {
                "confirm" => format!("confirm: {}  (answered OK)", d.message),
                "prompt" => format!("prompt: {}  (cancelled)", d.message),
                _ => format!("alert: {}", d.message),
            };
            self.push_toast(note);
        }
        if let Some((x, y)) = out.scroll_to {
            if idx == self.active {
                self.set_scroll(x, y);
            } else if let Some(tab) = self.tabs.get_mut(idx) {
                tab.scroll = (x, y);
            }
        }
        if let Some(nav) = out.navigation {
            if idx == self.active {
                self.end_edit();
            }
            self.navigate_request(idx, nav.request, !nav.replace);
        }
    }

    /// Move the page's console output into the tab's log.
    pub(crate) fn drain_console(&mut self, idx: usize) {
        let Some(tab) = self.tabs.get_mut(idx) else { return };
        let Some(page) = tab.page.as_mut() else { return };
        let lines = page.take_console();
        if lines.is_empty() {
            return;
        }
        tab.console.extend(lines);
        if self.console_open && idx == self.active {
            self.request_redraw();
        }
    }

    /// After scripts ran on a tab outside the event helpers (e.g. a
    /// `javascript:` URL): restyle if needed and collect side effects.
    pub(crate) fn after_script_activity(&mut self, idx: usize) {
        let Some(page) = self.tabs.get_mut(idx).and_then(|t| t.page.as_mut()) else { return };
        let out = self.engine.tick(page);
        self.apply_outcome(idx, out);
    }

    /// Run due timers of every tab with scripts.
    pub(crate) fn tick_scripts(&mut self) {
        for idx in 0..self.tabs.len() {
            let due = self.tabs[idx].page.as_ref().and_then(|p| p.next_script_due());
            let Some(due) = due else { continue };
            let now = self.engine.start.elapsed().as_secs_f64() * 1000.0;
            if due > now + 0.5 {
                continue;
            }
            let Some(page) = self.tabs[idx].page.as_mut() else { continue };
            let out = self.engine.tick(page);
            self.apply_outcome(idx, out);
        }
    }

    /// When the next script timer is due (any tab).
    pub(crate) fn next_script_wakeup(&self) -> Option<Instant> {
        let mut best: Option<f64> = None;
        for t in &self.tabs {
            if let Some(due) = t.page.as_ref().and_then(|p| p.next_script_due()) {
                best = Some(best.map(|b: f64| b.min(due)).unwrap_or(due));
            }
        }
        let due = best?;
        let now = self.engine.start.elapsed().as_secs_f64() * 1000.0;
        let delay = (due - now).max(1.0).min(1000.0);
        Some(Instant::now() + Duration::from_millis(delay as u64))
    }

    pub(crate) fn push_toast(&mut self, text: String) {
        self.toasts.push((text, Instant::now()));
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
        self.request_redraw();
    }

    pub(crate) fn expire_toasts(&mut self) {
        let before = self.toasts.len();
        self.toasts.retain(|(_, t)| t.elapsed() < Duration::from_secs(5));
        if self.toasts.len() != before {
            self.request_redraw();
        }
    }

    pub(crate) fn toggle_console(&mut self) {
        self.console_open = !self.console_open;
        self.ui_focus = if self.console_open { UiFocus::Console } else { UiFocus::Page };
        self.needs_relayout = true;
        self.mark_page_dirty();
    }

    pub(crate) fn toggle_javascript(&mut self) {
        self.settings.javascript = !self.settings.javascript;
        self.engine.scripting = self.settings.javascript;
        self.engine.loader.load_scripts = self.settings.javascript;
        self.settings.save(&self.profile.settings_path());
        self.set_status(if self.settings.javascript { "JavaScript enabled (reload to apply)" } else { "JavaScript disabled (reload to apply)" });
    }

    pub(crate) fn toggle_tracker_blocking(&mut self) {
        self.settings.block_trackers = !self.settings.block_trackers;
        self.engine.loader.fetcher.block_trackers = self.settings.block_trackers;
        self.settings.save(&self.profile.settings_path());
        self.set_status(if self.settings.block_trackers { "Bloqueo de anuncios y rastreadores activado" } else { "Bloqueo de anuncios y rastreadores desactivado" });
    }

    /// Evaluate the console input line in the active page.
    pub(crate) fn console_eval(&mut self) {
        let idx = self.active;
        let Some(tab) = self.tabs.get_mut(idx) else { return };
        let src = tab.console_input.text.trim().to_string();
        if src.is_empty() {
            return;
        }
        tab.console_input.set_text("");
        tab.console.push("input", format!("> {src}"));
        let result = match tab.page.as_mut() {
            Some(page) => self.engine.eval_in_page(page, &src),
            None => "(no page)".to_string(),
        };
        tab.console_scroll = 0;
        self.drain_console(idx);
        if let Some(tab) = self.tabs.get_mut(idx) {
            tab.console.push("result", result);
        }
        self.after_script_activity(idx);
        self.request_redraw();
    }

    // ------------------------------------------------------------ clipboard

    pub(crate) fn clipboard_set(&mut self, text: &str) {
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        if let Some(cb) = self.clipboard.as_mut() {
            if cb.set_text(text.to_string()).is_ok() {
                self.set_status("Copied to clipboard");
                return;
            }
        }
        self.set_status("Clipboard unavailable");
    }

    pub(crate) fn clipboard_get(&mut self) -> Option<String> {
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        self.clipboard.as_mut().and_then(|cb| cb.get_text().ok())
    }

    // ------------------------------------------------------------ bookmarks

    pub(crate) fn toggle_bookmark(&mut self) {
        let Some((url, title)) = self.tab().and_then(|t| t.current_url.as_ref().map(|u| (u.to_string(), t.title.clone()))) else { return };
        let added = self.bookmarks.toggle(&url, &title);
        self.bookmarks.save(&self.profile.bookmarks_path());
        self.set_status(if added { "Bookmark added" } else { "Bookmark removed" });
    }

    pub(crate) fn is_bookmarked(&self) -> bool {
        self.tab().and_then(|t| t.current_url.as_ref()).map(|u| self.bookmarks.contains(&u.to_string())).unwrap_or(false)
    }

    // ------------------------------------------------------------- session

    fn save_session(&self) {
        let urls: Vec<String> = self.tabs.iter().filter_map(|t| t.current_url.as_ref().map(|u| u.to_string())).filter(|u| !u.starts_with("about:")).collect();
        let active = self.active.min(urls.len().saturating_sub(1));
        Session { urls, active }.save(&self.profile.session_path());
    }

    pub(crate) fn shutdown(&mut self, event_loop: &ActiveEventLoop) {
        self.save_session();
        self.history.flush(&self.profile.history_path());
        self.bookmarks.save(&self.profile.bookmarks_path());
        self.zoom_levels.save(&self.profile.zoom_path());
        self.profile.save_cookies(&self.cookies);
        event_loop.exit();
    }

    fn open_initial_tabs(&mut self) {
        let urls: Vec<String> = if self.start_urls.is_empty() {
            let session = Session::load(&self.profile.session_path());
            if session.urls.is_empty() {
                vec!["about:home".to_string()]
            } else {
                session.urls
            }
        } else {
            std::mem::take(&mut self.start_urls).into_iter().map(|u| App::resolve_input(&u)).collect()
        };
        let mut seen = HashSet::new();
        for (i, u) in urls.iter().enumerate() {
            if !seen.insert(u.clone()) {
                continue;
            }
            self.new_tab(Some(u), i > 0);
        }
        if self.tabs.is_empty() {
            self.new_tab(Some("about:home"), false);
        }
        self.activate_tab(0);
        if self.tabs[0].display_url() == "about:home" {
            self.focus_url_field();
        } else {
            self.ui_focus = UiFocus::Page;
        }
    }

    // ---------------------------------------------------------- user events

    fn on_user_event(&mut self, event: UserEvent) {
        match event {
            UserEvent::Loaded { tab_id, generation, request_url, push_history, outcome } => {
                let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) else { return };
                if self.tabs[idx].generation != generation {
                    return; // stale
                }
                match outcome {
                    Ok(LoadOutcome::Page(loaded)) => {
                        let viewport = if idx == self.active { self.css_viewport() } else { loaded.viewport };
                        let mut page = self.engine.build_page(*loaded, viewport);
                        // Redirects requested by the page itself (script or
                        // meta refresh), windows and dialogs from load time.
                        let navigation = self.engine.take_page_navigation(&mut page);
                        let new_windows = std::mem::take(&mut page.script.new_windows);
                        let dialogs = std::mem::take(&mut page.script.dialogs);
                        self.install_page(idx, page, request_url, push_history);
                        let out = rabbithole_engine::ScriptOutcome { navigation, new_windows, dialogs, ..Default::default() };
                        self.apply_outcome(idx, out);
                    }
                    Ok(LoadOutcome::Download) => {
                        self.tabs[idx].loading = None;
                        if idx == self.active {
                            self.sync_url_field();
                        }
                        self.profile.save_cookies(&self.cookies);
                        self.set_status(format!("Downloading {request_url}"));
                    }
                    Err(e) => {
                        let html = pages::error_page(&request_url.to_string(), &e);
                        let viewport = self.css_viewport();
                        let page = self.engine.load_html(&html, request_url.clone(), viewport);
                        self.install_page(idx, page, request_url, push_history);
                    }
                }
            }
            UserEvent::DownloadStarted { id, url, filename, path, total, cancel } => {
                self.downloads.items.push(DownloadItem { id, url, filename: filename.clone(), path, done: 0, total, state: DownloadState::InProgress, cancel });
                self.set_status(format!("Downloading {filename}…"));
            }
            UserEvent::DownloadProgress { id, done, total } => {
                if let Some(d) = self.downloads.get_mut(id) {
                    d.done = done;
                    if total.is_some() {
                        d.total = total;
                    }
                }
                self.request_redraw();
            }
            UserEvent::DownloadDone { id, error } => {
                let mut msg = String::new();
                if let Some(d) = self.downloads.get_mut(id) {
                    d.state = match error {
                        None => {
                            msg = format!("Download complete: {}", d.path.display());
                            DownloadState::Done
                        }
                        Some(e) if e.contains("cancelled") => {
                            msg = format!("Download cancelled: {}", d.filename);
                            DownloadState::Cancelled
                        }
                        Some(e) => {
                            msg = format!("Download failed: {e}");
                            DownloadState::Failed(e)
                        }
                    };
                }
                self.set_status(msg);
                // Refresh an open about:downloads page.
                let showing = self.tab().and_then(|t| t.current_url.as_ref()).map(|u| u.scheme == "about" && u.path == "downloads").unwrap_or(false);
                if showing {
                    let idx = self.active;
                    self.show_about_page(idx, &about_page("downloads"), false);
                }
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = WindowAttributes::default()
            .with_title("RabbitHole")
            .with_inner_size(LogicalSize::new(1200.0, 800.0))
            .with_min_inner_size(LogicalSize::new(400.0, 300.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(e) => {
                eprintln!("rabbithole: could not create window: {e}");
                event_loop.exit();
                return;
            }
        };
        let context = match softbuffer::Context::new(window.clone()) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("rabbithole: could not create graphics context: {e}");
                event_loop.exit();
                return;
            }
        };
        match softbuffer::Surface::new(&context, window.clone()) {
            Ok(s) => self.surface = Some(s),
            Err(e) => {
                eprintln!("rabbithole: could not create surface: {e}");
                event_loop.exit();
                return;
            }
        }
        window.set_ime_allowed(true);
        self.window = Some(window);
        self.open_initial_tabs();
        self.request_redraw();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        self.on_user_event(event);
    }

    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        if matches!(cause, StartCause::ResumeTimeReached { .. }) {
            self.tick_scripts();
            self.expire_toasts();
            self.request_redraw();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let animating = self.tabs.get(self.active).map(|t| t.is_loading()).unwrap_or(false) || self.downloads.active().next().is_some();
        let transient = self.status.as_ref().map(|(_, t)| t.elapsed() < Duration::from_secs(4)).unwrap_or(false) || !self.toasts.is_empty();
        let mut wake = if animating {
            Some(Instant::now() + Duration::from_millis(50))
        } else if transient {
            Some(Instant::now() + Duration::from_millis(500))
        } else {
            None
        };
        if let Some(t) = self.next_script_wakeup() {
            wake = Some(wake.map(|w| w.min(t)).unwrap_or(t));
        }
        match wake {
            Some(t) => event_loop.set_control_flow(ControlFlow::WaitUntil(t)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.shutdown(event_loop),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.needs_relayout = true;
                self.page_dirty = true;
                self.request_redraw();
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, .. } => self.handle_key(&event, event_loop),
            WindowEvent::Ime(winit::event::Ime::Commit(text)) => self.handle_text_input(&text),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = position;
                self.handle_cursor_moved();
            }
            WindowEvent::CursorLeft { .. } => {
                self.hover_button = None;
                self.hover_tab = None;
                self.hover_plus = false;
                if self.hover_link.take().is_some() {
                    self.request_redraw();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => match state {
                winit::event::ElementState::Pressed => self.handle_mouse_down(button, event_loop),
                winit::event::ElementState::Released => self.handle_mouse_up(button),
            },
            WindowEvent::MouseWheel { delta, .. } => self.handle_wheel(delta),
            WindowEvent::Focused(false) => {
                self.mouse_down = None;
            }
            _ => {}
        }
    }
}

impl App {
    pub(crate) fn toggle_fullscreen(&mut self) {
        self.fullscreen = !self.fullscreen;
        if let Some(w) = &self.window {
            w.set_fullscreen(if self.fullscreen { Some(Fullscreen::Borderless(None)) } else { None });
        }
        self.needs_relayout = true;
        self.mark_page_dirty();
    }
}

// ------------------------------------------------------------------ helpers

/// Placeholder navigation marker returned by `dispatch_to` once the real
/// request has been handed to `navigate_request`.
fn dummy_navigation() -> rabbithole_engine::PendingNavigation {
    rabbithole_engine::PendingNavigation { request: Request::get(about_url()), replace: false }
}

pub(crate) fn about_url() -> Url {
    about_page("blank")
}

pub(crate) fn about_page(name: &str) -> Url {
    Url { scheme: "about".to_string(), host: String::new(), port: None, path: name.to_string(), query: None, fragment: None }
}

/// Heuristic: does the typed text look like a URL rather than a search?
pub(crate) fn looks_like_url(s: &str) -> bool {
    if s.contains(char::is_whitespace) {
        return false;
    }
    if s.contains("://") {
        return true;
    }
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("about:") || lower.starts_with("file:") || lower.starts_with("data:") || lower.starts_with("mailto:") || lower.starts_with("rabbithole:") {
        return true;
    }
    if lower == "localhost" || lower.starts_with("localhost:") || lower.starts_with("localhost/") {
        return true;
    }
    if lower.starts_with('/') && std::path::Path::new(s).exists() {
        return true;
    }
    let host = lower.split(['/', '?', '#', ':']).next().unwrap_or("");
    if host.contains('.') && !host.starts_with('.') && !host.ends_with('.') {
        let tld = host.rsplit('.').next().unwrap_or("");
        return tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphanumeric());
    }
    false
}

/// Percent-encode a search query (space as `+`).
pub(crate) fn percent_encode_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_detection() {
        assert!(looks_like_url("example.com"));
        assert!(looks_like_url("https://example.com/x?y"));
        assert!(looks_like_url("about:home"));
        assert!(looks_like_url("localhost:8080"));
        assert!(looks_like_url("sub.example.co.uk/path"));
        assert!(!looks_like_url("rust browser engine"));
        assert!(!looks_like_url("hello"));
        assert!(!looks_like_url("what is 2.5"));
    }

    #[test]
    fn search_fallback() {
        assert_eq!(App::resolve_input("  example.org "), "example.org");
        assert_eq!(App::resolve_input("rust & ünicode"), "https://duckduckgo.com/html/?q=rust+%26+%C3%BCnicode");
        assert_eq!(App::resolve_input(""), "about:home");
    }
}
