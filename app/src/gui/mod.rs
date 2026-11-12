//! The graphical browser: window, tabs and chrome on the UI thread; every
//! tab's page on a content thread of its own (`content`). The UI thread
//! only handles input, chrome and presenting frames, so a page can never
//! freeze the window. Painting lives in `render.rs`, input handling in
//! `interact.rs`, the internal pages in `pages.rs`, and the window-free
//! logic (tab strip, omnibox, smooth scrolling, PDF) in modules of their
//! own with unit tests.

pub mod bench;
pub mod chrome;
pub mod console;
pub mod content;
pub mod downloads;
pub mod input;
mod interact;
pub mod loader;
pub mod omnibox;
pub mod pages;
pub mod pdf;
mod render;
pub mod smooth;
pub mod stats;
pub mod tab;
pub mod tabstrip;

#[cfg(test)]
mod snapshots;

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::AtomicU64;
use std::sync::mpsc::Sender;
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
use crate::profile::{clamp_zoom, Profile, Session, SessionTab, ZoomLevels};
use crate::settings::{Settings, ThemeChoice};
use chrome::{Menu, ToolButton};
use crate::autocookies::{AutoCookies, ConsentEvent, ConsentPolicy, SiteRule};
use content::{ContentConfig, ContentHandle, ContentSettings, FromContent, Sink, ToContent, ViewSize};
use downloads::{DownloadItem, DownloadState, Downloads};
use input::TextBuffer;
use loader::{DownloadEvent, DownloadPrefs};
use omnibox::Suggestion;
use smooth::ScrollAnim;
use stats::UiStats;
use tab::Tab;
use tabstrip::{ClosedTab, ClosedTabs, TabDrag};

/// Which text field receives keyboard input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiFocus {
    Page,
    Url,
    Find,
    Console,
    /// The "save as" prompt bar.
    Prompt,
}

/// Devtools panels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Console,
    Elements,
    Network,
}

impl Panel {
    pub const ALL: [Panel; 3] = [Panel::Console, Panel::Elements, Panel::Network];

    pub fn label(self) -> &'static str {
        match self {
            Panel::Console => "Console",
            Panel::Elements => "Elements",
            Panel::Network => "Network",
        }
    }
}

/// A pressed mouse button awaiting release.
pub struct MouseDown {
    pub button: MouseButton,
    pub pos: PhysicalPosition<f64>,
    /// The press went to the page (its moves and release do too).
    pub on_page: bool,
}

/// "Save download as" prompt.
pub struct SavePrompt {
    pub url: String,
    pub field: TextBuffer,
    pub reply: Sender<Option<PathBuf>>,
}

/// Events delivered to the UI thread's event loop.
pub enum UserEvent {
    /// A message from a tab's content thread.
    Content { tab_id: u64, epoch: u64, msg: FromContent },
}

/// How this window was started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GuiOptions {
    /// Private window: throwaway profile, nothing persisted.
    pub private: bool,
    /// An additional window (does not restore or save the session).
    pub new_window: bool,
}

/// A busy content thread shows the "not responding" bar after this long.
pub const HANG_AFTER: Duration = Duration::from_millis(2500);
/// The session is saved at most this often (and on exit).
pub const SESSION_SAVE_EVERY: Duration = Duration::from_secs(2);

/// Run the GUI. `start_urls` empty → restore the last session (or open
/// the home page).
pub fn run(start_urls: Vec<String>, options: GuiOptions) -> Result<(), String> {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|e| format!("could not create event loop: {e}"))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let profile = if options.private {
        // A throwaway profile that starts with the user's settings and
        // bookmarks (changes made in the private window are not kept).
        let real = Profile::locate();
        let p = Profile::private();
        let _ = std::fs::copy(real.settings_path(), p.settings_path());
        let _ = std::fs::copy(real.bookmarks_path(), p.bookmarks_path());
        let _ = std::fs::copy(real.autocookies_sites_path(), p.autocookies_sites_path());
        p
    } else {
        Profile::locate()
    };
    let mut app = App::new(start_urls, options, profile, proxy_sink(proxy));
    let result = event_loop.run_app(&mut app).map_err(|e| format!("event loop error: {e}"));
    app.cleanup_private_profile();
    result
}

fn proxy_sink(proxy: EventLoopProxy<UserEvent>) -> Sink {
    let proxy = Mutex::new(proxy);
    Arc::new(move |tab_id, epoch, msg| {
        if let Ok(p) = proxy.lock() {
            let _ = p.send_event(UserEvent::Content { tab_id, epoch, msg });
        }
    })
}

pub struct App {
    /// UI fonts and the template loader (shared caches and cookies).
    pub(crate) engine: Engine,
    pub(crate) profile: Profile,
    pub(crate) options: GuiOptions,
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
    pub(crate) closed: ClosedTabs,
    pub(crate) tab_drag: Option<TabDrag>,
    pub(crate) window: Option<Rc<Window>>,
    pub(crate) surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    pub(crate) sink: Sink,
    pub(crate) canvas: Canvas,
    pub(crate) ui_focus: UiFocus,
    pub(crate) url_field: TextBuffer,
    /// The URL field shows user-typed text (not the page URL).
    pub(crate) url_edited: bool,
    pub(crate) suggestions: Vec<Suggestion>,
    pub(crate) suggestion_sel: Option<usize>,
    pub(crate) menu: Option<Menu>,
    pub(crate) context_menu: Option<Menu>,
    pub(crate) dropdown: Option<Menu>,
    pub(crate) hover_button: Option<ToolButton>,
    pub(crate) hover_tab: Option<usize>,
    pub(crate) hover_tab_close: bool,
    pub(crate) hover_plus: bool,
    /// The pointer is over the cookie-dialog indicator.
    pub(crate) hover_consent: bool,
    pub(crate) hover_bookmark: Option<usize>,
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
    pub(crate) mono_font: FontId,
    pub(crate) anim_start: Instant,
    pub(crate) fullscreen: bool,
    pub(crate) settings: Settings,
    /// The operating system prefers dark colours.
    pub(crate) os_dark: bool,
    pub(crate) dark: bool,
    /// The devtools pane is open, showing `panel`.
    pub(crate) devtools_open: bool,
    pub(crate) panel: Panel,
    /// Transient banners for script dialogs (`alert`, ...).
    pub(crate) toasts: Vec<(String, Instant)>,
    pub(crate) save_prompt: Option<SavePrompt>,
    pub(crate) scroll_anim: Option<ScrollAnim>,
    /// The session changed since it was last saved.
    pub(crate) session_dirty: bool,
    pub(crate) session_saved: Instant,
    /// The previous run did not exit cleanly.
    pub(crate) recovered_crash: bool,
    /// The next selection text from the page starts a search.
    pub(crate) pending_search: bool,
    /// Cookie dialogs handled automatically and per-site rules.
    pub(crate) autocookies: AutoCookies,
    /// UI thread frame/stall measurements.
    pub(crate) stats: UiStats,
    /// Show the frame-time overlay (Ctrl+Shift+U).
    pub(crate) show_stats: bool,
}

impl App {
    fn new(start_urls: Vec<String>, options: GuiOptions, profile: Profile, sink: Sink) -> App {
        let cookies = if options.private { Arc::new(Mutex::new(CookieJar::default())) } else { profile.load_cookies() };
        let mut engine = Engine::new();
        if options.private {
            // Memory-only HTTP cache and connections of its own.
            engine.loader.fetcher = rabbithole_engine::net::Fetcher::isolated();
        }
        engine.loader.fetcher.cookies = Some(cookies.clone());
        let settings = Settings::load(&profile.settings_path());
        if !options.private {
            engine.set_local_storage_path(profile.local_storage_path());
        }
        let ui_font = engine.fonts.select(Family::Sans, 400, false);
        let ui_font_bold = engine.fonts.select(Family::Sans, 700, false);
        let mono_font = engine.fonts.select(Family::Mono, 400, false);
        let history = if options.private { History::default() } else { History::load(&profile.history_path()) };
        let bookmarks = Bookmarks::load(&profile.bookmarks_path());
        let zoom_levels = if options.private { ZoomLevels::default() } else { ZoomLevels::load(&profile.zoom_path()) };
        let mut autocookies = AutoCookies::load(&profile.autocookies_path(), &profile.autocookies_sites_path());
        if options.private {
            // The log is not kept for private windows (rules apply).
            autocookies.log.clear();
        }
        let main_window = !options.private && !options.new_window;
        let recovered_crash = main_window && profile.running_marker().exists();
        if main_window {
            let _ = std::fs::write(profile.running_marker(), std::process::id().to_string());
        }
        let mut app = App {
            engine,
            profile,
            options,
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
            closed: ClosedTabs::default(),
            tab_drag: None,
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
            hover_consent: false,
            hover_bookmark: None,
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
            mono_font,
            anim_start: Instant::now(),
            fullscreen: false,
            settings,
            os_dark: false,
            dark: false,
            devtools_open: false,
            panel: Panel::Console,
            toasts: Vec::new(),
            save_prompt: None,
            scroll_anim: None,
            session_dirty: false,
            session_saved: Instant::now(),
            recovered_crash,
            pending_search: false,
            autocookies,
            stats: UiStats::new(std::env::var_os("RH_UI_STATS").is_some()),
            show_stats: false,
        };
        app.apply_engine_settings();
        app.apply_theme();
        app
    }

    /// Remove a private window's throwaway profile.
    fn cleanup_private_profile(&self) {
        if self.options.private {
            let _ = std::fs::remove_dir_all(&self.profile.dir);
        }
    }

    /// The main window owns the session and the crash marker.
    pub(crate) fn is_main_window(&self) -> bool {
        !self.options.private && !self.options.new_window
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
            devtools_open: self.devtools_open,
            bookmarks_bar: self.settings.show_bookmarks_bar,
            prompt_open: self.save_prompt.is_some(),
            consent_icon: self.tabs.get(self.active).map(|t| t.consent.is_some()).unwrap_or(false),
        }
    }

    pub(crate) fn pinned_flags(&self) -> Vec<bool> {
        self.tabs.iter().map(|t| t.pinned).collect()
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

    // ------------------------------------------------------------- settings

    pub(crate) fn download_dir(&self) -> PathBuf {
        let d = self.settings.download_dir.trim();
        if d.is_empty() {
            self.profile.downloads_dir()
        } else {
            let p = PathBuf::from(d);
            let _ = std::fs::create_dir_all(&p);
            p
        }
    }

    pub(crate) fn download_prefs(&self) -> DownloadPrefs {
        DownloadPrefs { dir: self.download_dir(), ask: self.settings.ask_download, next_id: self.next_download_id.clone() }
    }

    fn content_settings(&self) -> ContentSettings {
        ContentSettings {
            javascript: self.settings.javascript,
            images: self.settings.images,
            block_trackers: self.settings.block_trackers,
            accept_language: self.settings.accept_language.clone(),
            download_dir: self.download_dir(),
            ask_download: self.settings.ask_download,
            consent: ConsentPolicy { mode: self.settings.cookie_consent, sites: self.autocookies.sites.clone() },
        }
    }

    /// Push the settings into the template engine (inherited by new
    /// content threads) and the shared cookie jar.
    fn apply_engine_settings(&mut self) {
        self.engine.scripting = self.settings.javascript;
        self.engine.loader.load_scripts = self.settings.javascript;
        self.engine.loader.load_images = self.settings.images;
        self.engine.loader.fetcher.block_trackers = self.settings.block_trackers;
        self.engine.loader.fetcher.accept_language = self.settings.accept_language.clone();
        if let Ok(mut jar) = self.cookies.lock() {
            jar.block_third_party = self.settings.block_third_party_cookies;
        }
    }

    fn broadcast_settings(&self) {
        let s = self.content_settings();
        for t in &self.tabs {
            t.send(ToContent::Settings(s.clone()));
        }
    }

    /// Choose light or dark chrome (and internal pages).
    pub(crate) fn apply_theme(&mut self) {
        let dark = match self.settings.theme {
            ThemeChoice::System => self.os_dark,
            ThemeChoice::Light => false,
            ThemeChoice::Dark => true,
        };
        self.dark = dark;
        chrome::set_dark(dark);
        pages::set_dark(dark);
        if let Some(w) = &self.window {
            w.set_theme(Some(if dark { winit::window::Theme::Dark } else { winit::window::Theme::Light }));
        }
        self.request_redraw();
    }

    /// Change one setting (settings page, menus); returns false for an
    /// invalid value.
    pub(crate) fn change_setting(&mut self, key: &str, value: &str) -> bool {
        if !self.settings.set(key, value) {
            return false;
        }
        self.settings.save(&self.profile.settings_path());
        self.apply_engine_settings();
        self.broadcast_settings();
        match key {
            "theme" => {
                self.apply_theme();
                // Re-render open internal pages in the new colours.
                for i in 0..self.tabs.len() {
                    let about = self.tabs[i].current_url.clone().filter(|u| u.scheme == "about" && u.path != "home" && u.path != "text");
                    if let Some(u) = about {
                        self.show_about_page(i, &u, false);
                    }
                }
            }
            "show_bookmarks_bar" => {
                let i = self.active;
                self.sync_view(i);
            }
            _ => {}
        }
        self.request_redraw();
        true
    }

    // ------------------------------------------------------- content threads

    /// Start a content thread for a tab.
    fn spawn_content(&mut self, tab_id: u64, view: ViewSize) -> ContentHandle {
        let epoch = self.next_epoch;
        self.next_epoch += 1;
        let config = ContentConfig {
            loader: self.engine.loader.clone(),
            settings: self.content_settings(),
            local_storage: self.engine.local_storage.clone(),
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

    fn first_unpinned(&self) -> usize {
        self.tabs.iter().take_while(|t| t.pinned).count()
    }

    /// Create a tab at `insert_at` with a content thread (not navigated).
    fn insert_tab(&mut self, insert_at: usize) -> usize {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = Tab::new(id);
        let insert_at = insert_at.min(self.tabs.len());
        // Background tabs load for the active tab's view size.
        let view = self.view_for(self.active.min(self.tabs.len().saturating_sub(1)));
        tab.content = Some(self.spawn_content(id, view));
        tab.view = Some(view);
        self.tabs.insert(insert_at, tab);
        if insert_at <= self.active && self.tabs.len() > 1 {
            self.active += 1;
        }
        self.session_dirty = true;
        insert_at
    }

    /// Open a new tab (optionally in the background) and navigate it.
    /// Tabs opened from the page go right after the active tab; new empty
    /// tabs at the end.
    pub(crate) fn new_tab(&mut self, url: Option<&str>, background: bool) -> usize {
        let after_active = if self.tabs.is_empty() { 0 } else { self.active + 1 };
        let at = if url.is_none() { self.tabs.len() } else { after_active.max(self.first_unpinned()) };
        let insert_at = self.insert_tab(at);
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
            self.scroll_anim = None;
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
        if self.devtools_open && self.panel == Panel::Elements {
            self.send_active(ToContent::InspectTree);
        }
        self.session_dirty = true;
        self.request_redraw();
    }

    pub(crate) fn close_tab(&mut self, idx: usize, event_loop: &ActiveEventLoop) {
        if idx >= self.tabs.len() {
            return;
        }
        // Dropping the tab's handle shuts its content thread down.
        let tab = self.tabs.remove(idx);
        self.closed.push(ClosedTab { history: tab.history_strings(), index: tab.index, pinned: tab.pinned, title: tab.title.clone(), position: idx });
        self.session_dirty = true;
        if self.tabs.is_empty() {
            self.shutdown(event_loop);
            return;
        }
        let new_active = if idx < self.active || (idx == self.active && idx >= self.tabs.len()) { self.active.saturating_sub(1) } else { self.active };
        self.active = new_active.min(self.tabs.len() - 1);
        let a = self.active;
        self.activate_tab(a);
    }

    /// Ctrl+Shift+T: reopen the last closed tab with its history.
    pub(crate) fn reopen_closed_tab(&mut self) {
        let Some(c) = self.closed.pop() else {
            self.set_status("No recently closed tabs");
            return;
        };
        let pos = if c.pinned { c.position.min(self.first_unpinned()) } else { c.position.max(self.first_unpinned()) };
        let idx = self.insert_tab(pos);
        self.restore_tab_state(idx, &SessionTab { history: c.history, index: c.index, pinned: c.pinned, title: c.title });
        self.activate_tab(idx);
    }

    /// Give a fresh tab a saved back/forward list and load its entry.
    fn restore_tab_state(&mut self, idx: usize, st: &SessionTab) {
        let history: Vec<Url> = st.history.iter().filter_map(|u| Url::parse(u)).collect();
        let Some(tab) = self.tabs.get_mut(idx) else { return };
        tab.pinned = st.pinned;
        if !st.title.is_empty() {
            tab.title = st.title.clone();
        }
        let index = st.index.min(history.len().saturating_sub(1));
        let url = history.get(index).map(|u| u.to_string()).unwrap_or_else(|| "about:home".into());
        tab.history = history;
        tab.index = index;
        self.navigate_tab(idx, &url, false);
    }

    /// Pin or unpin a tab (pinned tabs move to the front group).
    pub(crate) fn toggle_pin(&mut self, idx: usize) {
        let Some(tab) = self.tabs.get_mut(idx) else { return };
        tab.pinned = !tab.pinned;
        let pinned = tab.pinned;
        // Pinning: last pinned slot; unpinning: first unpinned slot.
        let n_pinned = self.tabs.iter().enumerate().filter(|(i, t)| *i != idx && t.pinned).count();
        let _ = pinned;
        let to = n_pinned.min(self.tabs.len() - 1);
        self.active = tabstrip::move_item(&mut self.tabs, idx, to, self.active);
        self.session_dirty = true;
        self.request_redraw();
    }

    pub(crate) fn duplicate_tab(&mut self, idx: usize) {
        let Some(t) = self.tabs.get(idx) else { return };
        let st = SessionTab { history: t.history_strings(), index: t.index, pinned: false, title: t.title.clone() };
        let at = (idx + 1).max(self.first_unpinned());
        let new = self.insert_tab(at);
        self.restore_tab_state(new, &st);
        self.activate_tab(new);
    }

    /// Close every tab except `keep` and pinned ones.
    pub(crate) fn close_other_tabs(&mut self, keep: usize, only_right: bool, event_loop: &ActiveEventLoop) {
        let Some(keep_id) = self.tabs.get(keep).map(|t| t.id) else { return };
        let ids: Vec<u64> = self.tabs.iter().enumerate().filter(|(i, t)| t.id != keep_id && !t.pinned && (!only_right || *i > keep)).map(|(_, t)| t.id).collect();
        for id in ids {
            if let Some(i) = self.tab_index(id) {
                self.close_tab(i, event_loop);
            }
        }
        if let Some(i) = self.tab_index(keep_id) {
            self.activate_tab(i);
        }
    }

    /// Move a tab within the strip (drag and drop).
    pub(crate) fn move_tab(&mut self, from: usize, to: usize) {
        if from == to || from >= self.tabs.len() || to >= self.tabs.len() || self.tabs[from].pinned != self.tabs[to].pinned {
            return;
        }
        self.active = tabstrip::move_item(&mut self.tabs, from, to, self.active);
        self.session_dirty = true;
        self.request_redraw();
    }

    /// Open a new browser window (a separate process sharing the profile)
    /// or a private window (a throwaway profile).
    pub(crate) fn open_window(&mut self, url: Option<&str>, private: bool) {
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(e) => return self.set_status(format!("Could not open a window: {e}")),
        };
        let mut cmd = std::process::Command::new(exe);
        cmd.arg(if private { "--private" } else { "--new-window" });
        if let Some(u) = url {
            cmd.arg(u);
        }
        if let Err(e) = cmd.spawn() {
            self.set_status(format!("Could not open a window: {e}"));
        }
    }

    pub(crate) fn update_title(&self) {
        if let (Some(w), Some(t)) = (&self.window, self.tab()) {
            let suffix = if self.options.private { "RabbitHole (Private)" } else { "RabbitHole" };
            let title = if t.title.trim().is_empty() { suffix.to_string() } else { format!("{} — {suffix}", t.title.trim()) };
            w.set_title(&title);
        }
    }

    /// Put the active tab's URL in the address bar (unless the user is typing).
    pub(crate) fn sync_url_field(&mut self) {
        if self.ui_focus == UiFocus::Url && self.url_edited {
            return;
        }
        let text = self.tab().map(|t| t.display_url()).unwrap_or_default();
        let text = if text == "about:home" { String::new() } else { text };
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

    /// Turn typed text into a URL (search fallback with the configured
    /// engine).
    pub(crate) fn resolve_input(&self, input: &str) -> String {
        omnibox::resolve_input(input, self.settings.search_engine, &self.settings.homepage)
    }

    /// Navigate a tab to a URL string (GET).
    pub(crate) fn navigate_tab(&mut self, idx: usize, url: &str, push_history: bool) {
        let Some(parsed) = Url::parse(url) else {
            let html = pages::error_page(url, "The address could not be understood.");
            self.show_internal(idx, &html, url, push_history);
            return;
        };
        let referer = self.tabs.get(idx).and_then(|t| t.current_url.clone()).filter(|u| u.is_http());
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
        if url.scheme == "about" && url.path == "newtab" {
            return self.navigate_tab(idx, "about:home", push_history);
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
        tab.load_started = Some(Instant::now());
        tab.find.count = 0;
        tab.inspector.network.clear();
        tab.inspector.network_selected = None;
        tab.inspector.network_scroll = 0;
        tab.send(ToContent::Navigate { request: req, push_history });
        if idx == self.active {
            self.ui_focus = UiFocus::Page;
            self.url_edited = false;
            self.url_field.set_text(&url.to_string());
            self.close_popups();
            self.scroll_anim = None;
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

    fn site_data(&self) -> Vec<pages::SiteData> {
        let mut map: std::collections::BTreeMap<String, pages::SiteData> = std::collections::BTreeMap::new();
        if let Ok(jar) = self.cookies.lock() {
            for (host, n) in jar.by_domain() {
                let h = host.trim_start_matches('.').to_string();
                map.entry(h.clone()).or_insert(pages::SiteData { host: h, cookies: 0, zoom: None }).cookies += n;
            }
        }
        for (host, z) in &self.zoom_levels.levels {
            map.entry(host.clone()).or_insert(pages::SiteData { host: host.clone(), cookies: 0, zoom: None }).zoom = Some(*z);
        }
        map.into_values().collect()
    }

    /// Generate an internal page.
    pub(crate) fn about_html(&self, url: &Url, notice: Option<&str>) -> String {
        let q = url.query.as_deref();
        match url.path.as_str() {
            "history" => pages::history_page(&self.history, &pages::query_value(q, "q").unwrap_or_default()),
            "bookmarks" => pages::bookmarks_page(&self.bookmarks, pages::query_value(q, "edit").as_deref(), self.settings.show_bookmarks_bar, notice),
            "downloads" => pages::downloads_page(&self.downloads.rows(), &self.download_dir().display().to_string(), self.settings.ask_download),
            "autocookies" => pages::autocookies_page(&self.autocookies, self.settings.cookie_consent, &pages::query_value(q, "q").unwrap_or_default(), notice, self.options.private),
            "settings" | "preferences" => pages::settings_page(&self.settings, &self.site_data(), &self.profile.downloads_dir().display().to_string(), notice, self.options.private),
            "version" | "about" => pages::version_page(self.engine.fonts.face_count(), &self.profile, self.cookies.lock().map(|j| j.len()).unwrap_or(0)),
            "blank" | "" => pages::blank_page(),
            other => pages::error_page(&format!("about:{other}"), "Unknown internal page."),
        }
    }

    pub(crate) fn show_about_page(&mut self, idx: usize, url: &Url, push_history: bool) {
        self.show_about_page_with(idx, url, push_history, None)
    }

    fn show_about_page_with(&mut self, idx: usize, url: &Url, push_history: bool, notice: Option<&str>) {
        if url.path == "text" {
            // Built from the page on its content thread.
            if let Some(t) = self.tabs.get(idx) {
                t.send(ToContent::ShowText);
            }
            return;
        }
        let html = self.about_html(url, notice);
        self.show_internal(idx, &html, &url.to_string(), push_history);
    }

    /// `rabbithole://...` links and forms of the internal pages.
    fn handle_internal_action(&mut self, idx: usize, url: &Url) {
        let path = url.path.trim_start_matches('/').to_string();
        let q = url.query.clone();
        let qv = |k: &str| pages::query_value(q.as_deref(), k);
        let raw_q = q.as_deref().map(rabbithole_engine::net::url::percent_decode);
        match url.host.as_str() {
            "download" => {
                let (verb, id) = path.split_once('/').map(|(v, i)| (v.to_string(), i.parse::<u64>().ok())).unwrap_or((path.clone(), None));
                self.download_action(&verb, id);
                self.show_about_page(idx, &about_page("downloads"), false);
            }
            "history" => {
                let hp = self.profile.history_path();
                match path.as_str() {
                    "clear" => self.history.clear(&hp),
                    "delete" => {
                        if let Some(u) = &raw_q {
                            self.history.remove_url(u, &hp);
                        }
                    }
                    "delete-day" => {
                        if let Some(d) = raw_q.as_deref().and_then(|d| d.parse::<u64>().ok()) {
                            self.history.remove_day(d, &hp);
                        }
                    }
                    _ => {}
                }
                self.show_about_page(idx, &about_page("history"), false);
            }
            "bookmark" => {
                let bp = self.profile.bookmarks_path();
                // Another window may have changed the file.
                self.bookmarks = Bookmarks::load(&bp);
                let mut notice: Option<String> = None;
                match path.as_str() {
                    "remove" => {
                        if let Some(u) = &raw_q {
                            self.bookmarks.remove(u);
                        }
                    }
                    "up" | "down" => {
                        if let Some(u) = &raw_q {
                            self.bookmarks.move_by(u, if path == "up" { -1 } else { 1 });
                        }
                    }
                    "save" => {
                        if let Some(u) = qv("url") {
                            self.bookmarks.edit(&u, &qv("title").unwrap_or_default(), &qv("folder").unwrap_or_default());
                        }
                    }
                    "export" => {
                        let target = downloads::unique_path(&self.download_dir(), "bookmarks.html");
                        notice = Some(match std::fs::write(&target, self.bookmarks.to_netscape_html()) {
                            Ok(()) => format!("Exported {} bookmarks to {}", self.bookmarks.items.len(), target.display()),
                            Err(e) => format!("Export failed: {e}"),
                        });
                    }
                    "import" => {
                        let p = qv("path").unwrap_or_default();
                        let p = p.trim().trim_matches('"');
                        notice = Some(match std::fs::read_to_string(p) {
                            Ok(html) => {
                                let n = self.bookmarks.merge(Bookmarks::from_netscape_html(&html));
                                format!("Imported {n} new bookmarks from {p}")
                            }
                            Err(e) => format!("Could not read “{p}”: {e}"),
                        });
                    }
                    _ => {}
                }
                self.bookmarks.save(&bp);
                self.show_about_page_with(idx, &about_page("bookmarks"), false, notice.as_deref());
            }
            "settings" => {
                let key = qv("key").unwrap_or_default();
                let value = qv("value").unwrap_or_default();
                let ok = self.change_setting(&key, &value);
                let back = qv("back").unwrap_or_else(|| "settings".into());
                let notice = if ok { None } else { Some(format!("Invalid value for {key}: “{value}”")) };
                let page = if matches!(back.as_str(), "bookmarks" | "downloads" | "settings") { back } else { "settings".into() };
                self.show_about_page_with(idx, &about_page(&page), false, notice.as_deref());
                if ok {
                    self.set_status("Setting saved");
                }
            }
            "autocookies" => {
                let mut notice: Option<String> = None;
                // Another window may have changed the files.
                if !self.options.private {
                    self.autocookies = AutoCookies::load(&self.profile.autocookies_path(), &self.profile.autocookies_sites_path());
                }
                match path.as_str() {
                    "delete" => {
                        if let Some(id) = raw_q.as_deref().and_then(|v| v.parse::<u64>().ok()) {
                            self.autocookies.remove(id);
                        }
                    }
                    "clear" => {
                        self.autocookies.clear();
                        notice = Some("Log cleared".into());
                    }
                    "site" => {
                        let host = qv("host").unwrap_or_default();
                        let rule = qv("rule").unwrap_or_default();
                        let r = SiteRule::from_id(&rule);
                        if !self.autocookies.set_rule(&host, r) {
                            notice = Some(format!("Invalid site “{host}”"));
                        } else {
                            notice = Some(match r {
                                Some(r) => format!("{}: {}", host.trim(), r.label()),
                                None => format!("{}: follows the global setting", host.trim()),
                            });
                        }
                        self.broadcast_settings();
                    }
                    "mode" => {
                        let v = qv("value").unwrap_or_default();
                        if !self.change_setting("cookie_consent", &v) {
                            notice = Some(format!("Invalid mode “{v}”"));
                        }
                    }
                    _ => {}
                }
                self.save_autocookies();
                let back = match qv("q") {
                    Some(q) if !q.is_empty() => Url::parse(&format!("about:autocookies?q={}", pages::percent_encode(&q))).unwrap_or_else(|| about_page("autocookies")),
                    _ => about_page("autocookies"),
                };
                self.show_about_page_with(idx, &back, false, notice.as_deref());
            }
            "data" => {
                let msg = match path.as_str() {
                    "clear-history" => {
                        self.history.clear(&self.profile.history_path());
                        "History cleared".to_string()
                    }
                    "clear-cookies" => {
                        if let Ok(mut jar) = self.cookies.lock() {
                            jar.clear();
                        }
                        if let Ok(mut ls) = self.engine.local_storage.lock() {
                            ls.clear_all();
                            ls.save();
                        }
                        self.save_cookies();
                        "Cookies and site data cleared".to_string()
                    }
                    "clear-cache" => {
                        self.engine.loader.clear_caches();
                        if let Some(c) = &self.engine.loader.fetcher.net.cache {
                            c.clear();
                        }
                        "Cache cleared".to_string()
                    }
                    "clear-site" => {
                        let host = raw_q.clone().unwrap_or_default();
                        if let Ok(mut jar) = self.cookies.lock() {
                            jar.clear_host(&host);
                        }
                        if let Ok(mut ls) = self.engine.local_storage.lock() {
                            for o in ls.origins() {
                                if Url::parse(&o).map(|u| u.host == host).unwrap_or(false) {
                                    ls.clear_origin(&o);
                                }
                            }
                            ls.save();
                        }
                        self.zoom_levels.levels.remove(&host);
                        self.save_zoom();
                        self.save_cookies();
                        format!("Cleared data of {host}")
                    }
                    _ => String::new(),
                };
                self.show_about_page_with(idx, &about_page("settings"), false, Some(&msg));
            }
            _ => self.set_status(format!("Unknown action: {url}")),
        }
    }

    fn download_action(&mut self, verb: &str, id: Option<u64>) {
        match (verb, id) {
            ("cancel", Some(id)) => self.downloads.cancel(id),
            ("pause", Some(id)) => self.downloads.pause(id),
            ("resume", Some(id)) => self.resume_download(id),
            ("retry", Some(id)) => {
                if let Some(u) = self.downloads.get(id).and_then(|d| Url::parse(&d.url)) {
                    self.downloads.remove(id);
                    self.start_download(u);
                }
            }
            ("open", Some(id)) => {
                if let Some(p) = self.downloads.get(id).map(|d| d.path.clone()) {
                    if let Err(e) = downloads::open_with_system(&p) {
                        self.set_status(e);
                    }
                }
            }
            ("show", Some(id)) => {
                if let Some(p) = self.downloads.get(id).map(|d| d.path.clone()) {
                    if let Err(e) = downloads::show_in_folder(&p) {
                        self.set_status(e);
                    }
                }
            }
            ("remove", Some(id)) => self.downloads.remove(id),
            ("clear", _) => self.downloads.clear_finished(),
            ("open-folder", _) => {
                if let Err(e) = downloads::open_with_system(&self.download_dir()) {
                    self.set_status(e);
                }
            }
            _ => {}
        }
    }

    /// Download a URL directly (save link / image as, retry).
    pub(crate) fn start_download(&mut self, url: Url) {
        let sink = self.sink.clone();
        loader::spawn_download(self.engine.loader.clone(), Request::get(url), self.download_prefs(), move |d| sink(0, 0, FromContent::Download(d)));
    }

    fn resume_download(&mut self, id: u64) {
        let Some(d) = self.downloads.get_mut(id) else { return };
        if d.state != DownloadState::Paused {
            return;
        }
        let Some(url) = Url::parse(&d.url) else { return };
        d.control.pause.store(false, std::sync::atomic::Ordering::SeqCst);
        d.state = DownloadState::InProgress;
        d.sample = None;
        let (path, control) = (d.path.clone(), d.control.clone());
        let sink = self.sink.clone();
        loader::spawn_resume(self.engine.loader.clone(), url, path, id, control, move |e| sink(0, 0, FromContent::Download(e)));
    }

    /// A new document was committed on a tab's content thread.
    fn on_committed(&mut self, idx: usize, url: Url, title: String, push_history: bool) {
        let host = url.host.clone();
        let url_str = url.to_string();
        let zoom = self.zoom_for_host(&host);
        {
            let tab = &mut self.tabs[idx];
            tab.reset_page_state();
            // A consent click that navigated (consent pages) keeps its
            // indicator on the page it led to.
            if tab.consent.as_ref().map(|(_, t)| t.elapsed() > Duration::from_secs(4)).unwrap_or(false) {
                tab.consent = None;
            }
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
        if idx == self.active {
            self.scroll_anim = None;
        }
        self.sync_view(idx);
        if !self.options.private && !url_str.starts_with("about:") {
            self.history.add(&url_str, &title);
            self.history.flush(&self.profile.history_path());
        }
        self.save_cookies();
        self.session_dirty = true;
        if idx == self.active {
            if self.devtools_open && self.panel == Panel::Elements {
                self.send_active(ToContent::InspectTree);
            }
            self.sync_url_field();
            self.update_title();
            self.apply_cursor();
            self.request_redraw();
        }
    }

    fn zoom_for_host(&self, host: &str) -> f32 {
        self.zoom_levels.levels.get(host).copied().unwrap_or(self.settings.default_zoom)
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
        self.reload_tab(idx);
    }

    pub(crate) fn reload_tab(&mut self, idx: usize) {
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
            Some(s) => s.url.clone(),
            None => self.resolve_input(&self.url_field.text),
        };
        self.ui_focus = UiFocus::Page;
        self.close_popups();
        let idx = self.active;
        // Alt+Enter opens in a new tab.
        if self.modifiers.alt_key() {
            self.new_tab(Some(&text), false);
            return;
        }
        self.navigate_tab(idx, &text, true);
    }

    // ------------------------------------------------------------ scrolling

    /// Scroll the active page (immediately on screen: the last frame is
    /// shifted; the content thread repaints and catches up).
    pub(crate) fn set_scroll(&mut self, x: f32, y: f32) {
        self.scroll_anim = None;
        self.set_scroll_now(x, y);
    }

    fn set_scroll_now(&mut self, x: f32, y: f32) {
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

    /// Scroll by a delta right away (touchpads, drags).
    pub(crate) fn scroll_by(&mut self, dx: f32, dy: f32) {
        let (x, y) = self.scroll_target();
        self.set_scroll(x + dx, y + dy);
    }

    /// Where the page is scrolling to (the animation target, or the
    /// current position).
    fn scroll_target(&self) -> (f32, f32) {
        match &self.scroll_anim {
            Some(a) => a.target,
            None => self.tab().map(|t| t.scroll).unwrap_or((0.0, 0.0)),
        }
    }

    /// Scroll by a delta with the smooth animation (wheel notches, keys).
    pub(crate) fn scroll_by_smooth(&mut self, dx: f32, dy: f32) {
        if !self.settings.smooth_scrolling {
            return self.scroll_by(dx, dy);
        }
        let vp = self.css_viewport();
        let Some(tab) = self.tab() else { return };
        let max = (tab.max_scroll_x(vp.width), tab.max_scroll(vp.height));
        let now = Instant::now();
        let from = tab.scroll;
        match self.scroll_anim.as_mut() {
            Some(a) => a.retarget((dx, dy), now),
            None => self.scroll_anim = Some(ScrollAnim::new(from, (from.0 + dx, from.1 + dy), now)),
        }
        if let Some(a) = self.scroll_anim.as_mut() {
            a.clamp_target(max);
            if a.target == from {
                self.scroll_anim = None;
            }
        }
        self.tick_scroll();
    }

    /// Animated jump to an absolute position (Home / End).
    pub(crate) fn scroll_to_smooth(&mut self, x: f32, y: f32) {
        let (cx, cy) = self.scroll_target();
        let vp = self.css_viewport();
        let max = self.tab().map(|t| (t.max_scroll_x(vp.width), t.max_scroll(vp.height))).unwrap_or((0.0, 0.0));
        self.scroll_by_smooth(x.clamp(0.0, max.0) - cx, y.clamp(0.0, max.1) - cy);
    }

    /// Advance the scroll animation.
    pub(crate) fn tick_scroll(&mut self) {
        let Some(a) = self.scroll_anim else { return };
        let now = Instant::now();
        let (x, y) = a.position(now);
        if a.done(now) {
            self.scroll_anim = None;
        }
        self.set_scroll_now(x, y);
        self.request_redraw();
    }

    pub(crate) fn set_zoom(&mut self, zoom: f32) {
        let zoom = clamp_zoom(zoom);
        let default = self.settings.default_zoom;
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        if (tab.zoom - zoom).abs() < 0.001 {
            return;
        }
        tab.zoom = zoom;
        if let Some(host) = tab.current_url.as_ref().map(|u| u.host.clone()).filter(|h| !h.is_empty()) {
            if (zoom - default).abs() < 0.01 {
                self.zoom_levels.levels.remove(&host);
            } else {
                self.zoom_levels.levels.insert(host.clone(), zoom);
            }
            self.save_zoom();
            // Other tabs on the same site follow.
            for i in 0..self.tabs.len() {
                if i != self.active && self.tabs[i].current_url.as_ref().map(|u| u.host == host).unwrap_or(false) {
                    self.tabs[i].zoom = zoom;
                    self.sync_view(i);
                }
            }
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
                    tab.load_started = None;
                    if active {
                        self.sync_url_field();
                    }
                }
                self.request_redraw();
            }
            FromContent::Title(t) => {
                self.tabs[idx].title = t.clone();
                if let Some(u) = self.tabs[idx].current_url.as_ref().map(|u| u.to_string()) {
                    if !u.starts_with("about:") && !self.options.private {
                        self.history.add(&u, &t);
                    }
                }
                if active {
                    self.update_title();
                }
                self.session_dirty = true;
                self.request_redraw();
            }
            FromContent::UrlChanged { url, push_history } => {
                let tab = &mut self.tabs[idx];
                tab.current_url = Some(url.clone());
                if push_history {
                    tab.push_history(url);
                }
                self.session_dirty = true;
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
                if self.devtools_open && active {
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
                    self.scroll_anim = None;
                    self.request_redraw();
                }
            }
            FromContent::ScrollBy { dx, dy } => {
                if active {
                    self.scroll_by_smooth(dx, dy);
                }
            }
            FromContent::History(delta) => {
                if active {
                    self.go_history(delta);
                }
            }
            FromContent::ContextMenu { link, image, has_selection } => {
                if active {
                    let (x, y) = self.context_pos;
                    self.open_context_menu(link, image, has_selection, x, y);
                }
            }
            FromContent::SelectDropdown { node, options, rect } => {
                if active {
                    self.open_select_dropdown(node, options, rect);
                }
            }
            FromContent::Clipboard(text) => {
                if std::mem::take(&mut self.pending_search) {
                    // "Search for the selection" asked for the text.
                    let q: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !q.is_empty() {
                        let url = self.settings.search_engine.url(&omnibox::encode_query(&q));
                        self.new_tab(Some(&url), false);
                    }
                    return;
                }
                self.clipboard_set(&text)
            }
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
            FromContent::Network(ev) => {
                self.tabs[idx].log_request(ev);
                if active && self.devtools_open && self.panel == Panel::Network {
                    self.request_redraw();
                }
            }
            FromContent::DomTree(rows) => {
                self.tabs[idx].inspector.dom = rows;
                if active {
                    self.request_redraw();
                }
            }
            FromContent::NodeDetails(d) => {
                let tab = &mut self.tabs[idx];
                if let Some(n) = d.node {
                    if let Some(i) = tab.inspector.dom.iter().position(|r| r.node == n) {
                        tab.inspector.selected = Some(i);
                    }
                }
                tab.inspector.details = d;
                if active {
                    self.ensure_inspector_selection_visible();
                    self.request_redraw();
                }
            }
            FromContent::Download(_) => {}
            FromContent::Consent(ev) => self.on_consent(idx, ev),
        }
    }

    /// A content thread handled (or found) a cookie-consent dialog.
    fn on_consent(&mut self, idx: usize, ev: ConsentEvent) {
        self.autocookies.record(&ev, crate::profile::now_secs());
        self.save_autocookies();
        let summary = ev.summary();
        self.tabs[idx].consent = Some((ev, Instant::now()));
        if idx == self.active {
            self.set_status(summary);
        }
        // Refresh an open about:autocookies.
        for i in 0..self.tabs.len() {
            let showing = self.tabs[i].current_url.as_ref().map(|u| u.scheme == "about" && u.path == "autocookies").unwrap_or(false);
            if showing {
                let u = self.tabs[i].current_url.clone().unwrap();
                self.show_about_page(i, &u, false);
            }
        }
        self.request_redraw();
    }

    /// Persist the cookie-dialog log (not for private windows) and rules.
    pub(crate) fn save_autocookies(&self) {
        if self.options.private {
            let _ = std::fs::write(self.profile.autocookies_sites_path(), self.autocookies.serialize_sites());
            return;
        }
        self.autocookies.save(&self.profile.autocookies_path(), &self.profile.autocookies_sites_path());
    }

    /// Menu entry: cycle off → auto-reject → auto-accept.
    pub(crate) fn cycle_cookie_consent(&mut self) {
        let next = self.settings.cookie_consent.next();
        self.change_setting("cookie_consent", next.id());
        self.set_status(format!("Cookie dialogs: {}", next.label()));
    }

    fn on_download(&mut self, event: DownloadEvent) {
        match event {
            DownloadEvent::Ask { url, suggested, reply } => {
                let mut field = TextBuffer::new(&suggested.display().to_string());
                // Select the file name (without extension) for quick renaming.
                let text = field.text.clone();
                let name_start = text.rfind(['/', '\\']).map(|i| i + 1).unwrap_or(0);
                let name_end = text[name_start..].rfind('.').map(|i| name_start + i).filter(|e| *e > name_start).unwrap_or(text.len());
                field.set_caret(name_start, false);
                field.set_caret(name_end, true);
                if let Some(old) = self.save_prompt.replace(SavePrompt { url, field, reply }) {
                    let _ = old.reply.send(None);
                }
                self.ui_focus = UiFocus::Prompt;
                let i = self.active;
                self.sync_view(i);
                self.request_redraw();
            }
            DownloadEvent::Started { id, url, filename, path, total, resumable, control } => {
                if let Some(d) = self.downloads.get_mut(id) {
                    // A retried / resumed entry.
                    d.total = total;
                    d.state = DownloadState::InProgress;
                } else {
                    self.downloads.items.push(DownloadItem::new(id, url, filename.clone(), path, total, resumable, control));
                }
                self.set_status(format!("Downloading {filename}…"));
            }
            DownloadEvent::Progress { id, done, total } => {
                if let Some(d) = self.downloads.get_mut(id) {
                    d.progress(done, Instant::now());
                    if total.is_some() {
                        d.total = total;
                    }
                }
                self.refresh_downloads_page(false);
                self.request_redraw();
            }
            DownloadEvent::Paused { id, done } => {
                if let Some(d) = self.downloads.get_mut(id) {
                    d.done = done;
                    d.state = DownloadState::Paused;
                    d.speed = None;
                }
                self.set_status("Download paused");
                self.refresh_downloads_page(true);
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
                    d.speed = None;
                }
                self.save_cookies();
                self.set_status(msg);
                self.refresh_downloads_page(true);
            }
        }
    }

    /// Re-render an open about:downloads page (throttled for progress).
    fn refresh_downloads_page(&mut self, force: bool) {
        static LAST: Mutex<Option<Instant>> = Mutex::new(None);
        if !force {
            let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
            if last.map(|t| t.elapsed() < Duration::from_millis(500)).unwrap_or(false) {
                return;
            }
            *last = Some(Instant::now());
        }
        let showing = self.tab().and_then(|t| t.current_url.as_ref()).map(|u| u.scheme == "about" && u.path == "downloads").unwrap_or(false);
        if showing {
            let idx = self.active;
            self.show_about_page(idx, &about_page("downloads"), false);
        }
    }

    /// Answer the "save as" prompt.
    pub(crate) fn answer_save_prompt(&mut self, accept: bool) {
        let Some(p) = self.save_prompt.take() else { return };
        let answer = if accept {
            let path = PathBuf::from(p.field.text.trim());
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            Some(path)
        } else {
            None
        };
        let _ = p.reply.send(answer);
        self.ui_focus = UiFocus::Page;
        let i = self.active;
        self.sync_view(i);
        self.request_redraw();
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
        let icon = if self.tab_drag.map(|d| d.dragging).unwrap_or(false) { CursorIcon::Grabbing } else { icon };
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

    // ----------------------------------------------------------- devtools

    pub(crate) fn toggle_devtools(&mut self) {
        self.devtools_open = !self.devtools_open;
        if self.devtools_open {
            self.show_panel(self.panel);
        } else {
            self.ui_focus = UiFocus::Page;
            self.send_active(ToContent::InspectNode(None));
        }
        let i = self.active;
        self.sync_view(i);
        self.request_redraw();
    }

    pub(crate) fn show_panel(&mut self, panel: Panel) {
        self.devtools_open = true;
        self.panel = panel;
        self.ui_focus = if panel == Panel::Console { UiFocus::Console } else { UiFocus::Page };
        if panel == Panel::Elements {
            self.send_active(ToContent::InspectTree);
        } else {
            self.send_active(ToContent::InspectNode(None));
        }
        let i = self.active;
        self.sync_view(i);
        self.request_redraw();
    }

    /// Context menu "Inspect": open Elements on the node at a page point.
    pub(crate) fn inspect_at(&mut self, x: f32, y: f32) {
        self.devtools_open = true;
        self.panel = Panel::Elements;
        self.ui_focus = UiFocus::Page;
        let i = self.active;
        self.sync_view(i);
        if let Some(css) = self.to_css(PhysicalPosition::new(x as f64, y as f64)) {
            self.send_active(ToContent::InspectAt { x: css.0, y: css.1 });
        } else {
            self.send_active(ToContent::InspectTree);
        }
        self.request_redraw();
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

    pub(crate) fn toggle_javascript(&mut self) {
        let v = if self.settings.javascript { "0" } else { "1" };
        self.change_setting("javascript", v);
        self.set_status(if self.settings.javascript { "JavaScript enabled (reload to apply)" } else { "JavaScript disabled (reload to apply)" });
    }

    pub(crate) fn toggle_tracker_blocking(&mut self) {
        let v = if self.settings.block_trackers { "0" } else { "1" };
        self.change_setting("block_trackers", v);
        self.set_status(if self.settings.block_trackers { "Ad and tracker blocking on" } else { "Ad and tracker blocking off" });
    }

    pub(crate) fn toggle_bookmarks_bar(&mut self) {
        let v = if self.settings.show_bookmarks_bar { "0" } else { "1" };
        self.change_setting("show_bookmarks_bar", v);
    }

    /// Print the active page to a PDF in the downloads folder.
    pub(crate) fn print_pdf(&mut self) {
        let Some(tab) = self.tab() else { return };
        let base = tab.display_title().chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).collect::<String>();
        let base = base.trim().chars().take(60).collect::<String>();
        let base = if base.is_empty() { "page".to_string() } else { base };
        let path = downloads::unique_path(&self.download_dir(), &format!("{base}.pdf"));
        self.set_status(format!("Saving PDF to {}…", path.display()));
        self.send_active(ToContent::PrintPdf(path));
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
        // Another window may have changed the file.
        self.bookmarks = Bookmarks::load(&self.profile.bookmarks_path());
        let added = self.bookmarks.toggle(&url, &title);
        self.bookmarks.save(&self.profile.bookmarks_path());
        self.set_status(if added { "Bookmark added — edit it in about:bookmarks" } else { "Bookmark removed" });
    }

    pub(crate) fn is_bookmarked(&self) -> bool {
        self.tab().and_then(|t| t.current_url.as_ref()).map(|u| self.bookmarks.contains(&u.to_string())).unwrap_or(false)
    }

    // ------------------------------------------------------------- session

    pub(crate) fn current_session(&self) -> Session {
        let tabs: Vec<SessionTab> = self
            .tabs
            .iter()
            .filter(|t| !t.history.is_empty())
            .map(|t| SessionTab { history: t.history_strings(), index: t.index, pinned: t.pinned, title: t.title.clone() })
            .collect();
        let active_id = self.tab().map(|t| t.id);
        let active = self.tabs.iter().filter(|t| !t.history.is_empty()).position(|t| Some(t.id) == active_id).unwrap_or(0);
        Session { tabs, active }
    }

    fn save_session(&mut self) {
        self.session_dirty = false;
        self.session_saved = Instant::now();
        if !self.is_main_window() {
            return;
        }
        self.current_session().save(&self.profile.session_path());
    }

    fn save_cookies(&self) {
        if !self.options.private {
            self.profile.save_cookies(&self.cookies);
        }
    }

    fn save_zoom(&self) {
        if !self.options.private {
            self.zoom_levels.save(&self.profile.zoom_path());
        }
    }

    pub(crate) fn shutdown(&mut self, event_loop: &ActiveEventLoop) {
        self.save_session();
        if !self.options.private {
            self.history.flush(&self.profile.history_path());
            if let Ok(mut ls) = self.engine.local_storage.lock() {
                ls.save();
            }
        }
        self.save_zoom();
        self.save_cookies();
        if let Some(p) = self.save_prompt.take() {
            let _ = p.reply.send(None);
        }
        if self.is_main_window() {
            let _ = std::fs::remove_file(self.profile.running_marker());
        }
        // Content threads exit when their handles are dropped.
        self.tabs.clear();
        event_loop.exit();
    }

    fn open_initial_tabs(&mut self) {
        let explicit: Vec<String> = std::mem::take(&mut self.start_urls).into_iter().map(|u| self.resolve_input(&u)).collect();
        if !explicit.is_empty() {
            for (i, u) in explicit.iter().enumerate() {
                self.new_tab(Some(u), i > 0);
            }
        } else if self.is_main_window() && (self.settings.restore_session || self.recovered_crash) {
            let session = Session::load(&self.profile.session_path());
            for st in &session.tabs {
                let at = self.tabs.len();
                let idx = self.insert_tab(at);
                self.restore_tab_state(idx, st);
            }
            if !self.tabs.is_empty() {
                self.activate_tab(session.active.min(self.tabs.len() - 1));
            }
        }
        if self.tabs.is_empty() {
            let home = self.settings.homepage.clone();
            self.new_tab(Some(&home), false);
        }
        if self.active >= self.tabs.len() {
            self.active = 0;
        }
        let a = self.active;
        self.activate_tab(a);
        if self.tabs[a].display_url() == "about:home" {
            self.focus_url_field();
        } else {
            self.ui_focus = UiFocus::Page;
        }
        if self.recovered_crash {
            self.push_toast("RabbitHole did not close properly last time — your tabs have been restored.".into());
        }
        if self.options.private {
            self.push_toast("Private window: history, cookies and site data are forgotten when it closes.".into());
        }
    }

    /// Any tab (or download) animating?
    fn animating(&self) -> bool {
        self.tabs.iter().any(|t| t.is_loading()) || self.downloads.active().next().is_some()
    }

    /// Wake-up deadline for animations, banners and hang detection.
    fn next_wakeup(&self) -> Option<Instant> {
        let transient = self.status.as_ref().map(|(_, t)| t.elapsed() < Duration::from_secs(4)).unwrap_or(false) || !self.toasts.is_empty();
        // Watch a busy content thread so the "not responding" bar shows up.
        let busy = self.tab().and_then(|t| t.content.as_ref()).and_then(|c| c.busy_for()).is_some();
        if self.scroll_anim.is_some() {
            Some(Instant::now() + Duration::from_millis(8))
        } else if self.animating() || self.show_stats {
            Some(Instant::now() + Duration::from_millis(50))
        } else if busy {
            Some(Instant::now() + Duration::from_millis(250))
        } else if transient || self.session_dirty {
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
        let title = if self.options.private { "RabbitHole (Private)" } else { "RabbitHole" };
        let attrs = WindowAttributes::default()
            .with_title(title)
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
        self.os_dark = window.theme() == Some(winit::window::Theme::Dark);
        self.window = Some(window);
        self.apply_theme();
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
            self.tick_scroll();
            self.request_redraw();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.stats.report();
        if self.session_dirty && self.session_saved.elapsed() >= SESSION_SAVE_EVERY {
            self.save_session();
        }
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
            WindowEvent::ThemeChanged(theme) => {
                self.os_dark = theme == winit::window::Theme::Dark;
                if self.settings.theme == ThemeChoice::System {
                    self.change_setting("theme", "system");
                }
            }
            WindowEvent::RedrawRequested => {
                self.tick_scroll();
                self.redraw()
            }
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
                self.hover_bookmark = None;
                if let Some(tab) = self.tab_mut() {
                    if tab.hover_link.take().is_some() {
                        self.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => match state {
                winit::event::ElementState::Pressed => self.handle_mouse_down(button, event_loop),
                winit::event::ElementState::Released => self.handle_mouse_up(button, event_loop),
            },
            WindowEvent::MouseWheel { delta, .. } => self.handle_wheel(delta),
            WindowEvent::Focused(false) => {
                self.mouse_down = None;
                self.tab_drag = None;
            }
            WindowEvent::DroppedFile(path) => {
                let url = format!("file://{}", path.display().to_string().replace('\\', "/"));
                let url = if url.starts_with("file:///") { url } else { url.replacen("file://", "file:///", 1) };
                let idx = self.active;
                self.navigate_tab(idx, &url, true);
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
    Url { scheme: "about".to_string(), path: name.to_string(), opaque_path: true, ..Default::default() }
}
