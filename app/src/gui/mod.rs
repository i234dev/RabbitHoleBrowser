//! The graphical browser: window, tabs and chrome on the UI thread; every
//! tab's page on a content thread of its own (`content`). The UI thread
//! only handles input, chrome and presenting frames, so a page can never
//! freeze the window. Painting lives in `render.rs`, input handling in
//! `interact.rs`.

pub mod bench;
pub mod chrome;
pub mod console;
pub mod content;
pub mod downloads;
pub mod input;
mod interact;
pub mod loader;
pub mod pages;
mod render;
pub mod stats;
pub mod tab;

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rabbithole_engine::gfx::{Canvas, Family, FontId, Rect};
use rabbithole_engine::net::{CookieJar, Request, Url};
use rabbithole_engine::{Engine, Viewport};
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
use content::{ContentConfig, ContentHandle, ContentSettings, FromContent, Sink, ToContent, ViewSize};
use downloads::{DownloadItem, DownloadState, Downloads};
use input::TextBuffer;
use loader::DownloadEvent;
use stats::UiStats;
use tab::Tab;

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
    /// The press went to the page (its moves and release do too).
    pub on_page: bool,
}

/// Events delivered to the UI thread's event loop.
pub enum UserEvent {
    /// A message from a tab's content thread.
    Content { tab_id: u64, epoch: u64, msg: FromContent },
}

/// A busy content thread shows the "not responding" bar after this long.
pub const HANG_AFTER: Duration = Duration::from_millis(2500);

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
    /// UI fonts and the template loader (shared caches and cookies).
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
    pub(crate) next_epoch: u64,
    pub(crate) window: Option<Rc<Window>>,
    pub(crate) surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    pub(crate) sink: Sink,
    pub(crate) canvas: Canvas,
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
    pub(crate) modifiers: ModifiersState,
    pub(crate) cursor: PhysicalPosition<f64>,
    pub(crate) cursor_icon: CursorIcon,
    pub(crate) mouse_down: Option<MouseDown>,
    pub(crate) last_click: Option<(Instant, PhysicalPosition<f64>)>,
    /// Where the next page context menu opens (physical px).
    pub(crate) context_pos: (f32, f32),
    pub(crate) clipboard: Option<arboard::Clipboard>,
    pub(crate) start_urls: Vec<String>,
    pub(crate) ui_font: FontId,
    pub(crate) ui_font_bold: FontId,
    pub(crate) anim_start: Instant,
    pub(crate) fullscreen: bool,
    pub(crate) settings: Settings,
    /// The JavaScript console pane is open.
    pub(crate) console_open: bool,
    /// Transient banners for script dialogs (`alert`, ...).
    pub(crate) toasts: Vec<(String, Instant)>,
    /// UI thread frame/stall measurements.
    pub(crate) stats: UiStats,
    /// Show the frame-time overlay (Ctrl+Shift+U).
    pub(crate) show_stats: bool,
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
        let proxy = Mutex::new(proxy);
        let sink: Sink = Arc::new(move |tab_id, epoch, msg| {
            if let Ok(p) = proxy.lock() {
                let _ = p.send_event(UserEvent::Content { tab_id, epoch, msg });
            }
        });
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
            next_epoch: 1,
            window: None,
            surface: None,
            sink,
            canvas: Canvas::new(1, 1),
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
            modifiers: ModifiersState::empty(),
            cursor: PhysicalPosition::new(0.0, 0.0),
            cursor_icon: CursorIcon::Default,
            mouse_down: None,
            last_click: None,
            context_pos: (0.0, 0.0),
            clipboard: None,
            start_urls,
            ui_font,
            ui_font_bold,
            anim_start: Instant::now(),
            fullscreen: false,
            settings,
            console_open: false,
            toasts: Vec::new(),
            stats: UiStats::new(std::env::var_os("RH_UI_STATS").is_some()),
            show_stats: false,
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
        self.view_for(self.active).viewport
    }

    /// Page area size and scale of a tab.
    pub(crate) fn view_for(&self, idx: usize) -> ViewSize {
        let area = self.chrome().page_area();
        let zoom = self.tabs.get(idx).map(|t| t.zoom).unwrap_or(1.0);
        ViewSize::new(area.w.max(1.0) as u32, area.h.max(1.0) as u32, self.ui_scale() * zoom)
    }

    /// Tell a tab's content thread about its view size when it changed.
    pub(crate) fn sync_view(&mut self, idx: usize) {
        let view = self.view_for(idx);
        let Some(tab) = self.tabs.get_mut(idx) else { return };
        if tab.view != Some(view) {
            tab.view = Some(view);
            tab.send(ToContent::Resize(view));
        }
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

    /// CSS coordinates of a window position, clamped into the page area
    /// (drags that leave the page).
    pub(crate) fn to_css_clamped(&self, pos: PhysicalPosition<f64>) -> (f32, f32) {
        let area = self.chrome().page_area();
        let s = self.page_scale();
        let x = (pos.x as f32).clamp(area.x, area.right() - 1.0);
        let y = (pos.y as f32).clamp(area.y, area.bottom() - 1.0);
        ((x - area.x) / s, (y - area.y) / s)
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

    /// Send a message to the active tab's content thread.
    pub(crate) fn send_active(&self, msg: ToContent) {
        if let Some(t) = self.tab() {
            t.send(msg);
        }
    }

    pub(crate) fn tab_index(&self, tab_id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == tab_id)
    }

    pub(crate) fn request_redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    pub(crate) fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
        self.request_redraw();
    }

    // ------------------------------------------------------- content threads

    fn content_settings(&self) -> ContentSettings {
        ContentSettings { javascript: self.settings.javascript, images: self.settings.images, block_trackers: self.settings.block_trackers }
    }

    /// Start a content thread for a tab.
    fn spawn_content(&mut self, tab_id: u64, view: ViewSize) -> ContentHandle {
        let epoch = self.next_epoch;
        self.next_epoch += 1;
        let config = ContentConfig {
            loader: self.engine.loader.clone(),
            settings: self.content_settings(),
            local_storage: self.engine.local_storage.clone(),
            download_dir: self.profile.downloads_dir(),
            next_download_id: self.next_download_id.clone(),
            view,
            system_fonts: true,
            console_to_stderr: self.engine.console_to_stderr,
        };
        ContentHandle::spawn(tab_id, epoch, config, self.sink.clone())
    }

    /// Replace a tab's content thread (it stopped responding): the old
    /// thread is abandoned and exits when its task ends.
    pub(crate) fn restart_content(&mut self, idx: usize, reason: &str) {
        let Some(tab) = self.tabs.get(idx) else { return };
        let (tab_id, url) = (tab.id, tab.current_url.clone());
        let view = self.view_for(idx);
        let handle = self.spawn_content(tab_id, view);
        let active = idx == self.active;
        let tab = &mut self.tabs[idx];
        tab.content = Some(handle);
        tab.view = Some(view);
        tab.loading = None;
        tab.load_phase = None;
        tab.hang_dismissed = None;
        tab.send(ToContent::SetActive(active));
        let url = url.unwrap_or_else(about_url);
        let html = pages::error_page(&url.to_string(), reason);
        tab.send(ToContent::LoadHtml { html, url, push_history: false });
        self.set_status(reason.to_string());
    }

    // ---------------------------------------------------------------- tabs

    /// Open a new tab (optionally in the background) and navigate it.
    pub(crate) fn new_tab(&mut self, url: Option<&str>, background: bool) -> usize {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = Tab::new(id);
        let insert_at = if self.tabs.is_empty() { 0 } else { self.active + 1 };
        // Background tabs load for the active tab's view size.
        let view = self.view_for(self.active.min(self.tabs.len().saturating_sub(1)));
        tab.content = Some(self.spawn_content(id, view));
        tab.view = Some(view);
        self.tabs.insert(insert_at, tab);
        if insert_at <= self.active && self.tabs.len() > 1 {
            self.active += 1;
        }
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
        if idx != self.active {
            if let Some(old) = self.tabs.get(self.active) {
                old.send(ToContent::SetActive(false));
            }
        }
        self.active = idx;
        self.tabs[idx].send(ToContent::SetActive(true));
        self.sync_view(idx);
        self.close_popups();
        self.ui_focus = UiFocus::Page;
        self.url_edited = false;
        self.sync_url_field();
        self.update_title();
        self.apply_cursor();
        self.request_redraw();
    }

    pub(crate) fn close_tab(&mut self, idx: usize, event_loop: &ActiveEventLoop) {
        if idx >= self.tabs.len() {
            return;
        }
        // Dropping the tab's handle shuts its content thread down.
        self.tabs.remove(idx);
        if self.tabs.is_empty() {
            self.shutdown(event_loop);
            return;
        }
        let new_active = if idx < self.active || (idx == self.active && idx >= self.tabs.len()) { self.active.saturating_sub(1) } else { self.active };
        self.active = new_active.min(self.tabs.len() - 1);
        let a = self.active;
        self.activate_tab(a);
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

    /// Navigate a tab with an arbitrary request (form submissions use
    /// POST). Internal schemes are handled here; documents load on the
    /// tab's content thread.
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
            self.tabs[idx].send(ToContent::JavascriptUrl(url));
            return;
        }
        let tab = &mut self.tabs[idx];
        tab.loading = Some(url.clone());
        tab.load_phase = None;
        tab.find.count = 0;
        tab.send(ToContent::Navigate { request: req, push_history });
        if idx == self.active {
            self.ui_focus = UiFocus::Page;
            self.url_edited = false;
            self.url_field.set_text(&url.to_string());
            self.close_popups();
        }
        self.anim_start = Instant::now();
        self.request_redraw();
    }

    /// Show generated HTML in a tab as if it were loaded from `url`.
    fn show_internal(&mut self, idx: usize, html: &str, url: &str, push_history: bool) {
        let Some(tab) = self.tabs.get_mut(idx) else { return };
        let parsed = Url::parse(url).unwrap_or_else(about_url);
        tab.loading = None;
        tab.send(ToContent::LoadHtml { html: html.to_string(), url: parsed, push_history });
    }

    fn show_about_page(&mut self, idx: usize, url: &Url, push_history: bool) {
        if url.path == "text" {
            // Built from the page on its content thread.
            if let Some(t) = self.tabs.get(idx) {
                t.send(ToContent::ShowText);
            }
            return;
        }
        let html = match url.path.as_str() {
            "history" => pages::history_page(&self.history),
            "bookmarks" => pages::bookmarks_page(&self.bookmarks),
            "downloads" => pages::downloads_page(&self.downloads.rows()),
            "version" => pages::version_page(self.engine.fonts.face_count(), &self.profile, self.cookies.lock().map(|j| j.len()).unwrap_or(0)),
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

    /// A new document was committed on a tab's content thread.
    fn on_committed(&mut self, idx: usize, url: Url, title: String, push_history: bool) {
        let host = url.host.clone();
        let url_str = url.to_string();
        let zoom = self.zoom_levels.get(&host);
        {
            let tab = &mut self.tabs[idx];
            tab.reset_page_state();
            tab.title = title.clone();
            tab.current_url = Some(url.clone());
            tab.loading = None;
            if push_history || tab.history.is_empty() {
                tab.push_history(url.clone());
            } else if let Some(h) = tab.history.get_mut(tab.index) {
                // Redirects: keep the final URL in the history entry.
                *h = url.clone();
            }
            tab.zoom = zoom;
        }
        self.sync_view(idx);
        if !url_str.starts_with("about:") || url_str == "about:home" {
            self.history.add(&url_str, &title);
            self.history.flush(&self.profile.history_path());
        }
        self.profile.save_cookies(&self.cookies);
        if idx == self.active {
            self.sync_url_field();
            self.update_title();
            self.apply_cursor();
            self.request_redraw();
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
        let Some(tab) = self.tabs.get_mut(idx) else { return };
        if !tab.is_loading() {
            return;
        }
        tab.loading = None;
        tab.load_phase = None;
        // A content thread stuck in a long task cannot read the request.
        let stuck = tab.content.as_ref().and_then(|c| c.busy_for()).map(|d| d >= Duration::from_secs(1)).unwrap_or(false);
        if stuck {
            self.restart_content(idx, "Stopped: the page was busy and has been stopped.");
        } else {
            tab.send(ToContent::Stop);
        }
        self.sync_url_field();
        self.set_status("Stopped");
    }

    /// Follow a link from the UI (context menu).
    pub(crate) fn follow_link(&mut self, link: Url, new_tab: bool) {
        if new_tab {
            self.new_tab(Some(&link.to_string()), true);
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

    /// Scroll the active page (immediately on screen: the last frame is
    /// shifted; the content thread repaints and catches up).
    pub(crate) fn set_scroll(&mut self, x: f32, y: f32) {
        let vp = self.css_viewport();
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let nx = x.clamp(0.0, tab.max_scroll_x(vp.width));
        let ny = y.clamp(0.0, tab.max_scroll(vp.height));
        if (nx, ny) != tab.scroll {
            tab.scroll = (nx, ny);
            tab.send(ToContent::Scroll { x: nx, y: ny });
            self.request_redraw();
            self.update_hover();
        }
    }

    pub(crate) fn scroll_by(&mut self, dx: f32, dy: f32) {
        let (x, y) = self.tab().map(|t| t.scroll).unwrap_or((0.0, 0.0));
        self.set_scroll(x + dx, y + dy);
    }

    pub(crate) fn set_zoom(&mut self, zoom: f32) {
        let zoom = clamp_zoom(zoom);
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        if (tab.zoom - zoom).abs() < 0.001 {
            return;
        }
        tab.zoom = zoom;
        if let Some(host) = tab.current_url.as_ref().map(|u| u.host.clone()) {
            self.zoom_levels.set(&host, zoom);
            self.zoom_levels.save(&self.profile.zoom_path());
        }
        let idx = self.active;
        self.sync_view(idx);
        self.set_status(format!("Zoom {}%", (zoom * 100.0).round()));
    }

    // ------------------------------------------------------ content events

    fn on_content(&mut self, tab_id: u64, epoch: u64, msg: FromContent) {
        // Downloads outlive the tab that started them.
        if let FromContent::Download(d) = msg {
            self.on_download(d);
            return;
        }
        let Some(idx) = self.tab_index(tab_id) else { return };
        if self.tabs[idx].content.as_ref().map(|c| c.epoch) != Some(epoch) {
            return; // from a replaced content thread
        }
        let active = idx == self.active;
        match msg {
            FromContent::Frame(frame) => {
                self.tabs[idx].frame = Some(frame);
                if active {
                    // The document may have shrunk under the scroll position.
                    let vp = self.css_viewport();
                    let tab = &mut self.tabs[idx];
                    let (x, y) = tab.scroll;
                    let (mx, my) = (tab.max_scroll_x(vp.width), tab.max_scroll(vp.height));
                    if x > mx || y > my {
                        tab.scroll = (x.min(mx), y.min(my));
                        tab.send(ToContent::Scroll { x: tab.scroll.0, y: tab.scroll.1 });
                    }
                    self.request_redraw();
                }
            }
            FromContent::Committed { url, title, push_history } => self.on_committed(idx, url, title, push_history),
            FromContent::Loading(phase) => {
                let tab = &mut self.tabs[idx];
                tab.load_phase = phase;
                if phase.is_none() {
                    tab.loading = None;
                    if active {
                        self.sync_url_field();
                    }
                }
                self.request_redraw();
            }
            FromContent::Title(t) => {
                self.tabs[idx].title = t.clone();
                if let Some(u) = self.tabs[idx].current_url.as_ref().map(|u| u.to_string()) {
                    if !u.starts_with("about:") {
                        self.history.add(&u, &t);
                    }
                }
                if active {
                    self.update_title();
                }
                self.request_redraw();
            }
            FromContent::UrlChanged { url, push_history } => {
                let tab = &mut self.tabs[idx];
                tab.current_url = Some(url.clone());
                if push_history {
                    tab.push_history(url);
                }
                if active {
                    self.sync_url_field();
                    self.request_redraw();
                }
            }
            FromContent::Favicon(f) => {
                self.tabs[idx].favicon = f;
                self.request_redraw();
            }
            FromContent::Hover { cursor, link } => {
                let tab = &mut self.tabs[idx];
                tab.cursor = cursor;
                tab.hover_link = link;
                if active {
                    self.apply_cursor();
                    self.request_redraw();
                }
            }
            FromContent::State(s) => self.tabs[idx].state = s,
            FromContent::Console(lines) => {
                self.tabs[idx].console.extend(lines);
                if self.console_open && active {
                    self.request_redraw();
                }
            }
            FromContent::EvalResult(r) => {
                let tab = &mut self.tabs[idx];
                tab.console.push("result", r);
                tab.console_scroll = 0;
                self.request_redraw();
            }
            FromContent::Navigate { request, push_history } => {
                if active && self.tabs[idx].state.editing {
                    self.ui_focus = UiFocus::Page;
                }
                self.navigate_request(idx, request, push_history);
            }
            FromContent::OpenTab(url) => {
                self.new_tab(Some(&url.to_string()), true);
            }
            FromContent::Dialog(note) => self.push_toast(note),
            FromContent::ScrollTo { x, y } => {
                // Echo it so both sides agree after concurrent scrolls (a
                // no-op for the content thread when nothing else moved).
                let tab = &mut self.tabs[idx];
                tab.scroll = (x, y);
                tab.send(ToContent::Scroll { x, y });
                if active {
                    self.request_redraw();
                }
            }
            FromContent::ScrollBy { dx, dy } => {
                if active {
                    self.scroll_by(dx, dy);
                }
            }
            FromContent::History(delta) => {
                if active {
                    self.go_history(delta);
                }
            }
            FromContent::ContextMenu { link, has_selection } => {
                if active {
                    let (x, y) = self.context_pos;
                    self.open_context_menu(link, has_selection, x, y);
                }
            }
            FromContent::SelectDropdown { node, options, rect } => {
                if active {
                    self.open_select_dropdown(node, options, rect);
                }
            }
            FromContent::Clipboard(text) => self.clipboard_set(&text),
            FromContent::Status(s) => {
                if active {
                    self.set_status(s);
                }
            }
            FromContent::FindResult { count, current } => {
                let tab = &mut self.tabs[idx];
                tab.find.count = count;
                tab.find.current = current;
                self.request_redraw();
            }
            FromContent::Download(_) => {}
        }
    }

    fn on_download(&mut self, event: DownloadEvent) {
        match event {
            DownloadEvent::Started { id, url, filename, path, total, cancel } => {
                self.downloads.items.push(DownloadItem { id, url, filename: filename.clone(), path, done: 0, total, state: DownloadState::InProgress, cancel });
                self.set_status(format!("Downloading {filename}…"));
            }
            DownloadEvent::Progress { id, done, total } => {
                if let Some(d) = self.downloads.get_mut(id) {
                    d.done = done;
                    if total.is_some() {
                        d.total = total;
                    }
                }
                self.request_redraw();
            }
            DownloadEvent::Done { id, error } => {
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
                self.profile.save_cookies(&self.cookies);
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

    /// The window cursor for the pointer position.
    pub(crate) fn apply_cursor(&mut self) {
        let over_page = self.to_css(self.cursor).is_some() && self.menu.is_none() && self.context_menu.is_none() && self.dropdown.is_none();
        let chrome_icon = self.chrome_cursor();
        let icon = if over_page && self.hang_bar().map(|(b, _, _)| !b.contains(self.cursor.x as f32, self.cursor.y as f32)).unwrap_or(true) {
            match self.tab().map(|t| t.cursor).unwrap_or("default") {
                "pointer" => CursorIcon::Pointer,
                "text" => CursorIcon::Text,
                _ => CursorIcon::Default,
            }
        } else {
            chrome_icon
        };
        if icon != self.cursor_icon {
            self.cursor_icon = icon;
            if let Some(w) = &self.window {
                w.set_cursor(icon);
            }
        }
    }

    // ------------------------------------------------------ hanging pages

    /// The active tab's content thread has been busy for a long time and
    /// the user has not chosen to wait.
    pub(crate) fn active_hanging(&self) -> bool {
        let Some(tab) = self.tab() else { return false };
        let Some(c) = &tab.content else { return false };
        match (c.busy_for(), c.busy_since()) {
            (Some(d), Some(since)) => d >= HANG_AFTER && tab.hang_dismissed != Some(since),
            _ => false,
        }
    }

    /// Geometry of the "not responding" bar: (bar, wait button, stop button).
    pub(crate) fn hang_bar(&self) -> Option<(Rect, Rect, Rect)> {
        if !self.active_hanging() {
            return None;
        }
        let area = self.chrome().page_area();
        let s = self.ui_scale();
        let h = 40.0 * s;
        let bar = Rect::new(area.x, area.y, area.w, h);
        let bw = 76.0 * s;
        let stop = Rect::new(bar.right() - bw - 10.0 * s, bar.y + 6.0 * s, bw, h - 12.0 * s);
        let wait = Rect::new(stop.x - bw - 8.0 * s, stop.y, bw, stop.h);
        Some((bar, wait, stop))
    }

    pub(crate) fn hang_wait(&mut self) {
        let idx = self.active;
        if let Some(tab) = self.tabs.get_mut(idx) {
            tab.hang_dismissed = tab.content.as_ref().and_then(|c| c.busy_since());
        }
        self.request_redraw();
    }

    pub(crate) fn hang_stop(&mut self) {
        let idx = self.active;
        self.restart_content(idx, "The page stopped responding and was stopped.");
    }

    // ------------------------------------------------------------- toasts

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
        self.request_redraw();
    }

    fn broadcast_settings(&self) {
        let s = self.content_settings();
        for t in &self.tabs {
            t.send(ToContent::Settings(s));
        }
    }

    pub(crate) fn toggle_javascript(&mut self) {
        self.settings.javascript = !self.settings.javascript;
        self.engine.scripting = self.settings.javascript;
        self.engine.loader.load_scripts = self.settings.javascript;
        self.settings.save(&self.profile.settings_path());
        self.broadcast_settings();
        self.set_status(if self.settings.javascript { "JavaScript enabled (reload to apply)" } else { "JavaScript disabled (reload to apply)" });
    }

    pub(crate) fn toggle_tracker_blocking(&mut self) {
        self.settings.block_trackers = !self.settings.block_trackers;
        self.engine.loader.fetcher.block_trackers = self.settings.block_trackers;
        self.settings.save(&self.profile.settings_path());
        self.broadcast_settings();
        self.set_status(if self.settings.block_trackers { "Bloqueo de anuncios y rastreadores activado" } else { "Bloqueo de anuncios y rastreadores desactivado" });
    }

    /// Evaluate the console input line in the active page.
    pub(crate) fn console_eval(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let src = tab.console_input.text.trim().to_string();
        if src.is_empty() {
            return;
        }
        tab.console_input.set_text("");
        tab.console.push("input", format!("> {src}"));
        tab.console_scroll = 0;
        tab.send(ToContent::Eval(src));
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
        // Content threads exit when their handles are dropped.
        self.tabs.clear();
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

    /// Wake-up deadline for animations, banners and hang detection.
    fn next_wakeup(&self) -> Option<Instant> {
        let animating = self.tabs.get(self.active).map(|t| t.is_loading()).unwrap_or(false) || self.downloads.active().next().is_some();
        let transient = self.status.as_ref().map(|(_, t)| t.elapsed() < Duration::from_secs(4)).unwrap_or(false) || !self.toasts.is_empty();
        // Watch a busy content thread so the "not responding" bar shows up.
        let busy = self.tab().and_then(|t| t.content.as_ref()).and_then(|c| c.busy_for()).is_some();
        if animating || self.show_stats {
            Some(Instant::now() + Duration::from_millis(50))
        } else if busy {
            Some(Instant::now() + Duration::from_millis(250))
        } else if transient {
            Some(Instant::now() + Duration::from_millis(500))
        } else {
            None
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
        let t = Instant::now();
        match event {
            UserEvent::Content { tab_id, epoch, msg } => self.on_content(tab_id, epoch, msg),
        }
        self.stats.record("content message", t.elapsed());
    }

    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        if matches!(cause, StartCause::ResumeTimeReached { .. }) {
            self.expire_toasts();
            self.request_redraw();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.stats.report();
        match self.next_wakeup() {
            Some(t) => event_loop.set_control_flow(ControlFlow::WaitUntil(t)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let t = Instant::now();
        let kind = match &event {
            WindowEvent::RedrawRequested => "redraw",
            WindowEvent::KeyboardInput { .. } => "key",
            WindowEvent::MouseWheel { .. } => "wheel",
            WindowEvent::CursorMoved { .. } => "mouse move",
            WindowEvent::MouseInput { .. } => "click",
            WindowEvent::Resized(_) => "resize",
            _ => "window event",
        };
        match event {
            WindowEvent::CloseRequested => self.shutdown(event_loop),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => self.request_redraw(),
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
                if let Some(tab) = self.tab_mut() {
                    if tab.hover_link.take().is_some() {
                        self.request_redraw();
                    }
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
        self.stats.record(kind, t.elapsed());
    }
}

impl App {
    pub(crate) fn toggle_fullscreen(&mut self) {
        self.fullscreen = !self.fullscreen;
        if let Some(w) = &self.window {
            w.set_fullscreen(if self.fullscreen { Some(Fullscreen::Borderless(None)) } else { None });
        }
        self.request_redraw();
    }
}

// ------------------------------------------------------------------ helpers

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
