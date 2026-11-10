//! The content thread of a tab: owns the engine and the page, runs loads,
//! scripts, timers and input handling, and paints frames.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rabbithole_engine::dom::NodeId;
use rabbithole_engine::gfx::{Canvas, Color, Rect};
use rabbithole_engine::layout::{DisplayItem, DisplayList, LayoutBox};
use rabbithole_engine::net::{Request, Url};
use rabbithole_engine::script::EventInit;
use rabbithole_engine::scripting::{input_init, key_init, mouse_init, plain_init};
use rabbithole_engine::{ClickAction, ControlKind, Engine, Modifiers, Page, ScriptOutcome, Selection};

use super::frame::{Frame, Pixels};
use super::{now_ms, ContentConfig, ContentSettings, FromContent, Inbox, KeyInput, PageState, Sink, ToContent, ViewSize};
use crate::gui::chrome::{ACCENT, FIND_CURRENT, FIND_MATCH, SELECTION, TEXT};
use crate::gui::input::TextBuffer;
use crate::gui::loader::{self, LoadJob, LoadOutcome, LoadResult};
use crate::gui::pages;

/// Overscan above and below the viewport, in viewport heights.
const OVERSCAN: f32 = 0.5;
/// Pointer travel (device px) that turns a press into a drag.
const DRAG_THRESHOLD: f32 = 4.0;
const LINE_SCROLL: f32 = 40.0;

/// A text control being edited.
struct ControlEdit {
    node: NodeId,
    buf: TextBuffer,
    kind: ControlKind,
    max_len: Option<usize>,
    /// Value when editing started (`change` fires on blur if it differs).
    initial: String,
}

/// A primary/auxiliary button press on the page.
struct Press {
    button: u8,
    /// Viewport position of the press (CSS px).
    start: (f32, f32),
    /// Document point where a drag selection starts (None: no selection).
    anchor: Option<(f32, f32)>,
    dragged: bool,
    double: bool,
}

#[derive(Default)]
struct Find {
    query: String,
    matches: Vec<Rect>,
    current: usize,
}

/// What the last frame covered.
struct Painted {
    covered: Rect,
    scroll: (f32, f32),
    scale: f32,
    /// The display list depends on the scroll position (fixed / sticky).
    scroll_dependent: bool,
}

pub(super) fn run(tab_id: u64, epoch: u64, config: ContentConfig, sink: Sink, rx: Receiver<Inbox>, tx: Sender<Inbox>, busy: Arc<AtomicU64>) {
    let mut w = Worker::new(tab_id, epoch, config, sink, tx, busy);
    w.main_loop(rx);
}

struct Worker {
    tab_id: u64,
    epoch: u64,
    sink: Sink,
    tx: Sender<Inbox>,
    busy: Arc<AtomicU64>,
    config: ContentConfig,
    engine: Engine,
    page: Option<Page>,
    /// Incremented per navigation; stale load results are dropped.
    generation: u64,
    /// Load in flight: request URL and whether it adds a history entry.
    pending: Option<(Url, bool)>,
    /// The committed page waits for its script sources.
    awaiting_scripts: bool,
    /// Load phase after the commit (None: finished).
    load_phase: Option<&'static str>,
    view: ViewSize,
    active: bool,
    scroll: (f32, f32),
    selection: Option<Selection>,
    edit: Option<ControlEdit>,
    focus: Option<NodeId>,
    press: Option<Press>,
    find: Find,
    last_scroll_dispatch: Instant,
    dirty: bool,
    frame_seq: u64,
    painted: Option<Painted>,
    sent_hover: (&'static str, Option<Url>),
    sent_state: PageState,
    sent_title: String,
    sent_favicon: usize,
    quit: bool,
}

impl Worker {
    fn new(tab_id: u64, epoch: u64, config: ContentConfig, sink: Sink, tx: Sender<Inbox>, busy: Arc<AtomicU64>) -> Worker {
        let mut engine = if config.system_fonts { Engine::new() } else { Engine::new_bundled_fonts_only() };
        engine.loader = config.loader.clone();
        // Progressive loading: paint before images and scripts arrive.
        engine.loader.prefetch_images = false;
        engine.loader.defer_scripts = true;
        engine.console_to_stderr = config.console_to_stderr;
        engine.share_local_storage(config.local_storage.clone());
        engine.set_device_pixel_ratio(config.view.scale);
        let view = config.view;
        let mut w = Worker {
            tab_id,
            epoch,
            sink,
            tx,
            busy,
            config,
            engine,
            page: None,
            generation: 0,
            pending: None,
            awaiting_scripts: false,
            load_phase: None,
            view,
            active: false,
            scroll: (0.0, 0.0),
            selection: None,
            edit: None,
            focus: None,
            press: None,
            find: Find::default(),
            last_scroll_dispatch: Instant::now(),
            dirty: false,
            frame_seq: 0,
            painted: None,
            sent_hover: ("default", None),
            sent_state: PageState::default(),
            sent_title: String::new(),
            sent_favicon: 0,
            quit: false,
        };
        let settings = w.config.settings;
        w.apply_settings(settings);
        w
    }

    fn send(&self, msg: FromContent) {
        (self.sink)(self.tab_id, self.epoch, msg);
    }

    // ------------------------------------------------------------ the loop

    fn main_loop(&mut self, rx: Receiver<Inbox>) {
        loop {
            self.busy.store(0, Ordering::Relaxed);
            let first = match self.next_wakeup() {
                Some(d) => match rx.recv_timeout(d) {
                    Ok(m) => Some(m),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
                None => match rx.recv() {
                    Ok(m) => Some(m),
                    Err(_) => return,
                },
            };
            self.busy.store(now_ms(), Ordering::Relaxed);
            let mut batch: Vec<Inbox> = first.into_iter().collect();
            while let Ok(m) = rx.try_recv() {
                batch.push(m);
            }
            for m in coalesce(batch) {
                match m {
                    Inbox::Ui(msg) => self.handle(msg),
                    Inbox::Load { generation, result } => self.on_load(generation, result),
                }
                if self.quit {
                    return;
                }
            }
            self.tick_if_due();
            self.flush();
        }
    }

    /// How long to wait for messages before running timers.
    fn next_wakeup(&self) -> Option<Duration> {
        let mut wait: Option<f64> = None;
        if let Some(due) = self.page.as_ref().and_then(|p| p.next_script_due()) {
            let now = self.engine.start.elapsed().as_secs_f64() * 1000.0;
            wait = Some((due - now).max(0.0));
        }
        if self.load_phase.is_some() && !self.awaiting_scripts {
            // Watch for the end of image loading.
            wait = Some(wait.map(|w| w.min(200.0)).unwrap_or(200.0));
        }
        wait.map(|ms| Duration::from_micros((ms * 1000.0) as u64))
    }

    fn tick_if_due(&mut self) {
        let Some(page) = self.page.as_mut() else { return };
        let Some(due) = page.next_script_due() else { return };
        let now = self.engine.start.elapsed().as_secs_f64() * 1000.0;
        if due > now + 0.5 {
            return;
        }
        let out = self.engine.tick(page);
        self.apply_outcome(out);
    }

    /// Send what changed: console lines, state, title, favicon, progress
    /// and a new frame.
    fn flush(&mut self) {
        let Some(page) = self.page.as_mut() else { return };
        let lines = page.take_console();
        let favicon = page.favicon();
        let images_loading = page.images_in_flight() > 0;
        let key_listeners = page.script.host.as_ref().map(|h| h.any_listener_for("keydown") || h.any_listener_for("keyup") || h.any_listener_for("keypress")).unwrap_or(false);
        if !lines.is_empty() {
            self.send(FromContent::Console(lines));
        }
        let state = PageState { editing: self.edit.is_some(), key_listeners, focused: self.focus.is_some(), has_selection: self.selection.is_some() };
        if state != self.sent_state {
            self.sent_state = state;
            self.send(FromContent::State(state));
        }
        let fav = favicon.as_ref().map(|f| Arc::as_ptr(f) as usize).unwrap_or(0);
        if fav != self.sent_favicon {
            self.sent_favicon = fav;
            self.send(FromContent::Favicon(favicon));
        }
        if self.load_phase == Some("Loading images") && !images_loading {
            self.load_phase = None;
            self.send(FromContent::Loading(None));
        }
        if self.dirty && self.active {
            self.paint();
        }
    }

    // ------------------------------------------------------------ messages

    fn handle(&mut self, msg: ToContent) {
        match msg {
            ToContent::Navigate { request, push_history } => self.navigate(request, push_history),
            ToContent::LoadHtml { html, url, push_history } => {
                self.generation += 1;
                self.pending = None;
                self.show_html(&html, url, push_history);
            }
            ToContent::Stop => {
                if self.pending.take().is_some() || self.awaiting_scripts || self.load_phase.is_some() {
                    self.generation += 1;
                    self.awaiting_scripts = false;
                    self.load_phase = None;
                    self.send(FromContent::Loading(None));
                    self.send(FromContent::Status("Stopped".into()));
                }
            }
            ToContent::Resize(view) => self.resize(view),
            ToContent::SetActive(active) => {
                self.active = active;
                if active {
                    self.dirty = true;
                }
            }
            ToContent::Scroll { x, y } => self.on_ui_scroll(x, y),
            ToContent::PointerMove { x, y, mods } => self.pointer_move(x, y, mods),
            ToContent::PointerDown { x, y, button, mods, click_count } => self.pointer_down(x, y, button, mods, click_count),
            ToContent::PointerUp { x, y, button, mods, inside } => self.pointer_up(x, y, button, mods, inside),
            ToContent::Wheel { x, y, dx, dy } => {
                let doc = (x + self.scroll.0, y + self.scroll.1);
                let scrolled = match self.page.as_mut() {
                    Some(page) => self.engine.scroll_at(page, doc.0, doc.1, dx, dy),
                    None => false,
                };
                if scrolled {
                    self.dirty = true;
                } else {
                    self.send(FromContent::ScrollBy { dx, dy });
                }
            }
            ToContent::Key(k) => self.key(&k),
            ToContent::Text(text) => {
                if let Some(edit) = self.edit.as_mut() {
                    let max = edit.max_len;
                    edit.buf.insert(&text, max);
                    self.commit_edit();
                }
            }
            ToContent::Find(query) => {
                self.find.query = query;
                self.find.current = 0;
                self.refresh_find(true);
            }
            ToContent::FindStep(delta) => self.find_step(delta),
            ToContent::FindClose => {
                self.find = Find::default();
                self.dirty = true;
            }
            ToContent::SelectOption { node, index } => {
                self.mutate_page(|p| p.select_option(node, index));
                self.dispatch_to(node, "input", input_init(None));
                self.dispatch_to(node, "change", plain_init(false));
            }
            ToContent::CopySelection => {
                if let Some(text) = self.selection.as_ref().map(|s| s.text.clone()) {
                    self.send(FromContent::Clipboard(text));
                }
            }
            ToContent::Eval(src) => {
                let result = match self.page.as_mut() {
                    Some(page) => self.engine.eval_in_page(page, &src),
                    None => "(no page)".to_string(),
                };
                if let Some(page) = self.page.as_mut() {
                    let lines = page.take_console();
                    if !lines.is_empty() {
                        self.send(FromContent::Console(lines));
                    }
                }
                self.send(FromContent::EvalResult(result));
                self.after_script_activity();
            }
            ToContent::JavascriptUrl(url) => {
                if let Some(page) = self.page.as_mut() {
                    let _ = crate::scripting::run_javascript_url(&self.engine, page, &url);
                    self.after_script_activity();
                }
            }
            ToContent::ShowText => {
                let (u, t) = self.page.as_ref().map(|p| (p.url.to_string(), p.text())).unwrap_or_default();
                let html = pages::text_page(&u, &t);
                self.generation += 1;
                self.pending = None;
                self.show_html(&html, crate::gui::about_page("text"), true);
            }
            ToContent::Settings(s) => self.apply_settings(s),
            ToContent::Shutdown => self.quit = true,
        }
    }

    fn apply_settings(&mut self, s: ContentSettings) {
        self.config.settings = s;
        self.engine.scripting = s.javascript;
        self.engine.loader.load_scripts = s.javascript;
        self.engine.loader.load_images = s.images;
        self.engine.loader.fetcher.block_trackers = s.block_trackers;
    }

    // ---------------------------------------------------------- navigation

    fn navigate(&mut self, request: Request, push_history: bool) {
        // Tell the old page it is going away.
        if let Some(page) = self.page.as_mut() {
            if page.has_scripts() {
                for ev in ["beforeunload", "pagehide", "unload"] {
                    let _ = self.engine.dispatch_window_event(page, ev);
                }
            }
        }
        self.generation += 1;
        self.pending = Some((request.url.clone(), push_history));
        self.awaiting_scripts = false;
        self.load_phase = None;
        let job = LoadJob {
            loader: self.engine.loader.clone(),
            request,
            viewport: self.view.viewport,
            download_dir: self.config.download_dir.clone(),
            next_download_id: self.config.next_download_id.clone(),
        };
        let generation = self.generation;
        let tx = self.tx.clone();
        let sink = self.sink.clone();
        let (tab_id, epoch) = (self.tab_id, self.epoch);
        loader::spawn(
            job,
            move |result| {
                let _ = tx.send(Inbox::Load { generation, result });
            },
            move |d| sink(tab_id, epoch, FromContent::Download(d)),
        );
    }

    fn on_load(&mut self, generation: u64, result: LoadResult) {
        if generation != self.generation {
            return; // stale (stopped or superseded)
        }
        match result {
            LoadResult::Loaded(Ok(LoadOutcome::Page(loaded))) => {
                let push = self.pending.take().map(|(_, p)| p).unwrap_or(true);
                let page = self.engine.build_page_unscripted(*loaded, self.view.viewport);
                self.awaiting_scripts = true;
                self.install(page, push);
                // First paint now, before the scripts arrive and run.
                self.flush();
            }
            LoadResult::Loaded(Ok(LoadOutcome::Download)) => {
                let url = self.pending.take().map(|(u, _)| u.to_string()).unwrap_or_default();
                self.send(FromContent::Loading(None));
                self.send(FromContent::Status(format!("Downloading {url}")));
            }
            LoadResult::Loaded(Err(e)) => {
                let (url, push) = self.pending.take().unwrap_or_else(|| (crate::gui::about_url(), true));
                let html = pages::error_page(&url.to_string(), &e);
                self.show_html(&html, url, push);
            }
            LoadResult::Scripts(scripts) => {
                if !self.awaiting_scripts {
                    return;
                }
                self.awaiting_scripts = false;
                self.send(FromContent::Loading(Some("Running scripts")));
                let Some(page) = self.page.as_mut() else { return };
                page.scripts = scripts;
                self.engine.finish_page_load(page);
                let out = self.take_load_outcome();
                self.dirty = true;
                self.update_title();
                let images = self.page.as_ref().map(|p| p.images_in_flight() > 0).unwrap_or(false);
                self.load_phase = if images { Some("Loading images") } else { None };
                self.send(FromContent::Loading(self.load_phase));
                if self.scroll == (0.0, 0.0) {
                    if let Some(f) = self.page.as_ref().and_then(|p| p.url.fragment.clone()) {
                        self.scroll_to_fragment(&f);
                    }
                }
                self.apply_outcome(out);
            }
        }
    }

    /// Show generated HTML (internal and error pages).
    fn show_html(&mut self, html: &str, url: Url, push_history: bool) {
        let page = self.engine.load_html(html, url, self.view.viewport);
        self.awaiting_scripts = false;
        self.install(page, push_history);
        let out = self.take_load_outcome();
        self.update_title();
        self.send(FromContent::Loading(None));
        self.apply_outcome(out);
    }

    /// What the page's load-time scripts asked for.
    fn take_load_outcome(&mut self) -> ScriptOutcome {
        let Some(page) = self.page.as_mut() else { return ScriptOutcome::default() };
        let navigation = self.engine.take_page_navigation(page);
        let new_windows = std::mem::take(&mut page.script.new_windows);
        let dialogs = std::mem::take(&mut page.script.dialogs);
        let scroll_to = page.requested_scroll.take();
        ScriptOutcome { navigation, new_windows, dialogs, scroll_to, ..Default::default() }
    }

    /// A new document replaces the old one.
    fn install(&mut self, page: Page, push_history: bool) {
        self.selection = None;
        self.edit = None;
        self.focus = None;
        self.press = None;
        self.find.matches.clear();
        self.find.current = 0;
        self.scroll = (0.0, 0.0);
        self.painted = None;
        self.sent_hover = ("default", None);
        let title = if page.title.trim().is_empty() { page.url.to_string() } else { page.title.trim().to_string() };
        self.sent_title = title.clone();
        self.load_phase = if self.awaiting_scripts { Some("Loading scripts") } else { None };
        self.send(FromContent::Committed { url: page.url.clone(), title, push_history });
        self.send(FromContent::Hover { cursor: "default", link: None });
        let fragment = page.url.fragment.clone();
        self.page = Some(page);
        self.dirty = true;
        if let Some(f) = fragment {
            self.scroll_to_fragment(&f);
        }
        if !self.find.query.is_empty() {
            self.refresh_find(false);
        }
    }

    fn update_title(&mut self) {
        let Some(page) = self.page.as_ref() else { return };
        let t = page.doc.title().map(|t| t.trim().to_string()).unwrap_or_default();
        if !t.is_empty() && t != self.sent_title {
            self.sent_title = t.clone();
            self.send(FromContent::Title(t));
        }
    }

    // ------------------------------------------------------------- viewing

    fn resize(&mut self, view: ViewSize) {
        if view == self.view {
            return;
        }
        let scale_changed = (view.scale - self.view.scale).abs() > 1e-4;
        self.view = view;
        if scale_changed {
            self.engine.set_device_pixel_ratio(view.scale);
        }
        let Some(page) = self.page.as_mut() else { return };
        let resized = page.viewport != view.viewport;
        if resized || scale_changed {
            self.engine.relayout(page, view.viewport);
        }
        self.dirty = true;
        if resized {
            self.dispatch_window("resize");
        }
        let (x, y) = self.scroll;
        self.set_scroll(x, y);
        if !self.find.query.is_empty() {
            self.refresh_find(false);
        }
    }

    fn max_scroll(&self) -> (f32, f32) {
        let vp = self.view.viewport;
        match self.page.as_ref() {
            Some(p) => ((p.content_width() - vp.width).max(0.0), (p.content_height() - vp.height).max(0.0)),
            None => (0.0, 0.0),
        }
    }

    /// The UI scrolled the page.
    fn on_ui_scroll(&mut self, x: f32, y: f32) {
        let (mx, my) = self.max_scroll();
        let (x, y) = (x.clamp(0.0, mx), y.clamp(0.0, my));
        if (x, y) == self.scroll {
            return;
        }
        self.scroll = (x, y);
        if let Some(page) = self.page.as_mut() {
            self.engine.set_page_scroll(page, x, y);
        }
        if self.needs_repaint_at_scroll() {
            self.dirty = true;
        }
        self.dispatch_scroll_event();
    }

    /// Scroll requested by the page itself (keys, find, focus, scripts):
    /// apply it and tell the UI.
    fn set_scroll(&mut self, x: f32, y: f32) {
        let (mx, my) = self.max_scroll();
        let (x, y) = (x.clamp(0.0, mx), y.clamp(0.0, my));
        if (x, y) == self.scroll {
            return;
        }
        self.scroll = (x, y);
        if let Some(page) = self.page.as_mut() {
            self.engine.set_page_scroll(page, x, y);
        }
        self.dirty = true;
        self.send(FromContent::ScrollTo { x, y });
        self.dispatch_scroll_event();
    }

    fn dispatch_scroll_event(&mut self) {
        if self.last_scroll_dispatch.elapsed() >= Duration::from_millis(16) {
            self.last_scroll_dispatch = Instant::now();
            self.dispatch_window("scroll");
        }
    }

    /// Does the last frame still show the viewport at the current scroll?
    fn needs_repaint_at_scroll(&self) -> bool {
        let Some(p) = &self.painted else { return true };
        if p.scroll_dependent || (p.scale - self.view.scale).abs() > 1e-4 || p.scroll.0 != self.scroll.0 {
            return true;
        }
        let vh = self.view.viewport.height;
        let slack = vh * OVERSCAN * 0.25;
        let (top, bottom) = (self.scroll.1, self.scroll.1 + vh);
        let content_h = self.page.as_ref().map(|p| p.content_height()).unwrap_or(0.0);
        let c = p.covered;
        // Keep some painted margin in the scroll direction, unless the
        // frame already reaches the document's edge.
        let top_ok = top - slack >= c.y || c.y <= 0.0;
        let bottom_ok = bottom + slack <= c.bottom() || c.bottom() >= content_h - 0.5;
        let visible = top >= c.y && bottom <= c.bottom() + 0.5;
        !(top_ok && bottom_ok && visible)
    }

    fn scroll_by(&mut self, dx: f32, dy: f32) {
        let (x, y) = self.scroll;
        self.set_scroll(x + dx, y + dy);
    }

    /// Ensure a document rect is visible (vertically).
    fn scroll_into_view(&mut self, r: Rect) {
        let vh = self.view.viewport.height;
        let (sx, sy) = self.scroll;
        if r.y < sy {
            self.set_scroll(sx, r.y - 20.0);
        } else if r.bottom() > sy + vh {
            self.set_scroll(sx, r.bottom() - vh + 20.0);
        }
    }

    fn scroll_to_fragment(&mut self, fragment: &str) {
        let target = fragment.trim_start_matches('#');
        if target.is_empty() || target == "top" {
            self.set_scroll(0.0, 0.0);
            return;
        }
        if let Some(r) = self.page.as_ref().and_then(|p| p.anchor_rect(target)) {
            self.set_scroll(0.0, r.y);
        }
    }

    // ------------------------------------------------------------ painting

    fn paint(&mut self) {
        let Some(page) = self.page.as_mut() else { return };
        self.dirty = false;
        let s = self.view.scale;
        let vp = self.view.viewport;
        let (sx, sy) = self.scroll;
        let content_h = page.content_height().max(vp.height);
        let margin = vp.height * OVERSCAN;
        let oy = (sy - margin).max(0.0);
        let bottom = (sy + vp.height + margin).min(content_h.max(sy + vp.height));
        let max_h = self.view.height as f32 * (1.0 + 2.0 * OVERSCAN) + 2.0;
        let h = (((bottom - oy) * s).ceil()).clamp(1.0, max_h.min(16384.0)) as u32;
        let w = self.view.width.max(1);
        let origin = (sx, oy);

        let mut canvas = Canvas::new(w, h);
        if page.background.a > 0 {
            canvas.fill_rect(Rect::new(0.0, 0.0, w as f32, h as f32), page.background);
        }
        // The fixed layer is painted where the viewport is inside the
        // overscanned canvas.
        let mut items = std::mem::take(&mut page.display_list.items);
        let split = items.iter().position(|i| matches!(i, DisplayItem::PushFixed));
        let fixed = split.map(|i| items.split_off(i));
        let doc_list = DisplayList { items, hits: Default::default() };
        canvas.push_transform([s, 0.0, 0.0, s, 0.0, 0.0]);
        rabbithole_engine::layout::paint(&doc_list, &mut canvas, &self.engine.fonts, origin.0, origin.1);
        canvas.pop_transform();
        let mut items = doc_list.items;
        if let Some(fixed) = fixed {
            let fixed_list = DisplayList { items: fixed, hits: Default::default() };
            canvas.push_transform([s, 0.0, 0.0, s, (sx - origin.0) * s, (sy - origin.1) * s]);
            rabbithole_engine::layout::paint(&fixed_list, &mut canvas, &self.engine.fonts, 0.0, 0.0);
            canvas.pop_transform();
            items.extend(fixed_list.items);
        }
        page.display_list.items = items;

        self.paint_overlays(&mut canvas, origin);

        let page = self.page.as_ref().expect("page");
        let mut containers = Vec::new();
        collect_scroll_containers(&page.layout.root, false, &mut containers);
        let covered = Rect::new(origin.0, origin.1, w as f32 / s, h as f32 / s);
        self.frame_seq += 1;
        let frame = Frame {
            seq: self.frame_seq,
            canvas: Arc::new(Pixels::from_canvas(&mut canvas)),
            origin,
            scale: s,
            scroll: (sx, sy),
            content: (page.content_width(), page.content_height()),
            background: if page.background.a > 0 { page.background } else { Color::WHITE },
            scroll_containers: Arc::new(containers),
        };
        self.painted = Some(Painted { covered, scroll: (sx, sy), scale: s, scroll_dependent: page.layout.has_sticky_or_fixed() });
        self.send(FromContent::Frame(frame));
    }

    /// Selection, find highlights, focus ring and caret.
    fn paint_overlays(&self, canvas: &mut Canvas, origin: (f32, f32)) {
        let Some(page) = self.page.as_ref() else { return };
        let s = self.view.scale;
        let map = |r: Rect| Rect::new((r.x - origin.0) * s, (r.y - origin.1) * s, r.w * s, r.h * s);
        if let Some(sel) = &self.selection {
            for r in &sel.rects {
                canvas.fill_rect(map(*r), SELECTION);
            }
        }
        if !self.find.query.is_empty() {
            for (i, r) in self.find.matches.iter().enumerate() {
                canvas.fill_rect(map(*r), if i == self.find.current { FIND_CURRENT } else { FIND_MATCH });
            }
        }
        if let Some(n) = self.focus {
            if self.edit.as_ref().map(|e| e.node != n).unwrap_or(true) {
                if let Some(r) = page.rect_of(n).map(map) {
                    canvas.stroke_rect(Rect::new(r.x - 2.0, r.y - 2.0, r.w + 4.0, r.h + 4.0), [2.0; 4], [ACCENT; 4], [0; 4], 3.0);
                }
            }
        }
        if let Some(edit) = &self.edit {
            if let Some(screen) = page.rect_of(edit.node).map(map) {
                let rel = page.caret_x(&self.engine.fonts, edit.node, edit.buf.caret).unwrap_or(4.0);
                let cx = screen.x + rel * s;
                let pad = (3.0 * s).min(screen.h / 4.0);
                canvas.stroke_rect(screen, [1.5; 4], [ACCENT; 4], [0; 4], 2.0);
                canvas.fill_rect(Rect::new(cx, screen.y + pad, (1.0 * s).max(1.0), (screen.h - 2.0 * pad).max(2.0)), TEXT);
            }
        }
    }

    // -------------------------------------------------------------- events

    /// Dispatch an event to a node and apply the outcome. Returns
    /// (default prevented, navigated).
    fn dispatch_to(&mut self, node: NodeId, event_type: &str, init: EventInit) -> (bool, bool) {
        let Some(page) = self.page.as_mut() else { return (false, false) };
        if !page.has_scripts() {
            return (false, false);
        }
        let out = self.engine.dispatch_event(page, node, event_type, init);
        let r = (out.default_prevented, out.navigation.is_some());
        self.apply_outcome(out);
        r
    }

    fn dispatch_window(&mut self, event_type: &str) {
        let Some(page) = self.page.as_mut() else { return };
        if !page.has_scripts() {
            return;
        }
        let out = self.engine.dispatch_window_event(page, event_type);
        self.apply_outcome(out);
    }

    fn page_op<R>(&mut self, f: impl FnOnce(&Engine, &mut Page) -> (R, ScriptOutcome)) -> Option<R> {
        let page = self.page.as_mut()?;
        let (r, out) = f(&self.engine, page);
        self.apply_outcome(out);
        Some(r)
    }

    fn after_script_activity(&mut self) {
        let Some(page) = self.page.as_mut() else { return };
        let out = self.engine.tick(page);
        self.apply_outcome(out);
    }

    /// Apply what scripts asked for: repaint, navigation, new tabs,
    /// dialogs, scrolling.
    fn apply_outcome(&mut self, out: ScriptOutcome) {
        if out.relayout {
            self.dirty = true;
            if let Some(page) = self.page.as_mut() {
                let (x, y) = self.scroll;
                self.engine.set_page_scroll(page, x, y);
                // Keep an edit buffer in sync when scripts changed the value.
                if let Some(edit) = self.edit.as_mut() {
                    let now = page.control_text(edit.node);
                    if now != edit.buf.text {
                        edit.buf.set_text(&now);
                    }
                }
            }
            let (x, y) = self.scroll;
            let (mx, my) = self.max_scroll();
            if x > mx || y > my {
                self.set_scroll(x.min(mx), y.min(my));
            }
            if !self.find.query.is_empty() {
                self.refresh_find(false);
            }
            self.update_title();
        }
        for u in out.new_windows {
            self.send(FromContent::OpenTab(u));
        }
        for d in out.dialogs {
            let note = match d.kind {
                "confirm" => format!("confirm: {}  (answered OK)", d.message),
                "prompt" => format!("prompt: {}  (cancelled)", d.message),
                _ => format!("alert: {}", d.message),
            };
            self.send(FromContent::Dialog(note));
        }
        if let Some((x, y)) = out.scroll_to {
            self.set_scroll(x, y);
        }
        if let Some(nav) = out.navigation {
            self.end_edit();
            self.send(FromContent::Navigate { request: nav.request, push_history: !nav.replace });
        }
    }

    // ------------------------------------------------------------- pointer

    fn pointer_move(&mut self, x: f32, y: f32, mods: Modifiers) {
        let scale = self.view.scale;
        let doc = (x + self.scroll.0, y + self.scroll.1);
        if let Some(p) = self.press.as_mut() {
            if p.button == 0 {
                let dist = ((x - p.start.0).powi(2) + (y - p.start.1).powi(2)).sqrt() * scale;
                if dist > DRAG_THRESHOLD {
                    p.dragged = true;
                }
                if p.dragged {
                    if let (Some(anchor), Some(page)) = (p.anchor, self.page.as_ref()) {
                        let sel = page.select_between(&self.engine.fonts, anchor, doc);
                        self.selection = if sel.rects.is_empty() { None } else { Some(sel) };
                        self.dirty = true;
                        return;
                    }
                }
            }
        }
        // Hover state + pointer/mouse events from the same hit test as clicks.
        self.page_op(|engine, page| ((), engine.pointer_move(page, x, y, mods)));
        let Some(page) = self.page.as_ref() else { return };
        let node = page.pointer.hover;
        let link = node.and_then(|n| page.link_of(n));
        let mut cursor = match page.cursor_at(doc.0, doc.1) {
            "pointer" => "pointer",
            "text" => "text",
            _ => "default",
        };
        if link.is_some() {
            cursor = "pointer";
        }
        if let Some(n) = node {
            if matches!(page.control_kind(n), ControlKind::Text | ControlKind::Password | ControlKind::TextArea | ControlKind::Other) {
                cursor = "text";
            }
        }
        if (cursor, &link) != (self.sent_hover.0, &self.sent_hover.1) {
            self.sent_hover = (cursor, link.clone());
            self.send(FromContent::Hover { cursor, link });
        }
    }

    fn page_mouse_event(&mut self, node: NodeId, event_type: &str, css: (f32, f32), button: u8, m: Modifiers) -> (bool, bool) {
        let init = mouse_init(css.0, css.1, button, m.ctrl, m.shift, m.alt, m.meta);
        self.dispatch_to(node, event_type, init)
    }

    fn pointer_down(&mut self, x: f32, y: f32, button: u8, mods: Modifiers, click_count: u8) {
        let Some(page) = self.page.as_ref() else { return };
        let doc = (x + self.scroll.0, y + self.scroll.1);
        // Leaving a text control being edited: commit it (change + blur)
        // before the press moves focus elsewhere.
        let pre_control = page.node_at_viewport(x, y).and_then(|n| page.control_of(n));
        let editing = self.edit.as_ref().map(|e| e.node);
        if editing.is_some() && editing != pre_control {
            self.end_edit();
        }
        let down = self
            .page_op(|engine, page| {
                let mut d = engine.pointer_down(page, x, y, button, mods);
                let out = std::mem::take(&mut d.outcome);
                (d, out)
            })
            .unwrap_or_default();
        let node = down.target;
        let link = node.and_then(|n| self.page.as_ref().and_then(|p| p.link_of(n)));

        if button == 2 {
            let prevented = node.map(|n| self.page_mouse_event(n, "contextmenu", (x, y), 2, mods).0).unwrap_or(false);
            // The release must not produce a click.
            if let Some(page) = self.page.as_mut() {
                page.pointer.down = None;
            }
            if !prevented {
                self.send(FromContent::ContextMenu { link, has_selection: self.selection.is_some() });
            }
            return;
        }
        if button == 1 {
            // auxclick (and opening the link in a new tab) on release.
            self.press = Some(Press { button, start: (x, y), anchor: None, dragged: false, double: false });
            return;
        }
        if button != 0 {
            return;
        }
        let double = click_count >= 2;
        // Form controls react on press (editing, dropdowns); activation
        // (checkbox toggles, submit, reset) happens with the click.
        let control = node.and_then(|n| self.page.as_ref().and_then(|p| p.control_of(n)));
        if let Some(c) = control {
            let kind = self.page.as_ref().map(|p| p.control_kind(c)).unwrap_or(ControlKind::Other);
            match kind {
                ControlKind::Text | ControlKind::Password | ControlKind::TextArea | ControlKind::Other => {
                    if !down.prevented {
                        self.begin_edit(c);
                        self.place_caret_from_click(c, doc);
                    }
                    self.press = Some(Press { button, start: (x, y), anchor: None, dragged: false, double: false });
                    return;
                }
                ControlKind::Select => {
                    self.end_edit();
                    if !down.prevented {
                        self.open_select_dropdown(c);
                    }
                    self.press = Some(Press { button, start: (x, y), anchor: None, dragged: false, double: false });
                    return;
                }
                ControlKind::File => self.send(FromContent::Status("File uploads are not supported yet".into())),
                _ => {}
            }
        }
        self.end_edit();
        if self.focus.take().is_some() {
            self.dirty = true;
        }
        if double {
            let sel = self.page.as_ref().map(|p| p.select_word_at(&self.engine.fonts, doc.0, doc.1));
            self.selection = sel.filter(|s| !s.rects.is_empty());
            self.dirty = true;
        }
        let anchor = if down.prevented { None } else { Some(doc) };
        self.press = Some(Press { button, start: (x, y), anchor, dragged: false, double });
    }

    fn pointer_up(&mut self, x: f32, y: f32, button: u8, mods: Modifiers, inside: bool) {
        let Some(p) = self.press.take() else { return };
        if p.button != button {
            return;
        }
        // A drag-selection release still gets pointerup/mouseup, no click.
        if p.dragged {
            if let Some(page) = self.page.as_mut() {
                page.pointer.down = None;
            }
        } else if !p.double && self.selection.take().is_some() {
            self.dirty = true;
        }
        if !inside {
            if let Some(page) = self.page.as_mut() {
                page.pointer.down = None;
                page.state.active = None;
            }
            return;
        }
        let Some(up) = self.page_op(|engine, page| {
            let mut u = engine.pointer_up(page, x, y, button, mods);
            let out = std::mem::take(&mut u.outcome);
            (u, out)
        }) else {
            return;
        };
        match up.action {
            ClickAction::Navigate { url, new_tab } => self.follow_link(url, new_tab),
            ClickAction::Submit { form, submitter } => self.submit_form(submitter.unwrap_or(form), submitter),
            ClickAction::FocusControl(c) => match self.page.as_ref().map(|p| p.control_kind(c)) {
                Some(ControlKind::Select) => self.open_select_dropdown(c),
                Some(ControlKind::Text | ControlKind::Password | ControlKind::TextArea | ControlKind::Other) => self.begin_edit(c),
                _ => {}
            },
            ClickAction::Changed => self.dirty = true,
            ClickAction::None => {}
        }
        if p.double && button == 0 {
            if let Some(n) = up.clicked {
                self.page_mouse_event(n, "dblclick", (x, y), 0, mods);
            }
        }
    }

    fn follow_link(&mut self, link: Url, new_tab: bool) {
        if new_tab {
            self.send(FromContent::OpenTab(link));
            return;
        }
        let Some(page) = self.page.as_ref() else { return };
        let same_doc = page.url.without_fragment() == link.without_fragment();
        if let (Some(fragment), true) = (link.fragment.clone(), same_doc) {
            self.scroll_to_fragment(&fragment);
            self.send(FromContent::UrlChanged { url: link, push_history: true });
            self.dispatch_window("hashchange");
            return;
        }
        let referer = page.url.clone();
        self.send(FromContent::Navigate { request: Request::get(link).with_referer(referer), push_history: true });
    }

    fn open_select_dropdown(&mut self, select: NodeId) {
        let Some(page) = self.page.as_ref() else { return };
        let options: Vec<(String, bool, bool)> = page.select_options(select).into_iter().map(|(label, _, selected, disabled)| (label, selected, disabled)).collect();
        if options.is_empty() {
            return;
        }
        let rect = page.rect_of(select).unwrap_or_default();
        self.focus = Some(select);
        self.dirty = true;
        self.send(FromContent::SelectDropdown { node: select, options, rect });
    }

    // --------------------------------------------------------------- forms

    /// Apply a DOM mutation and restyle.
    fn mutate_page(&mut self, f: impl FnOnce(&mut Page)) {
        let Some(page) = self.page.as_mut() else { return };
        f(page);
        self.engine.restyle(page);
        let (x, y) = self.scroll;
        self.engine.set_page_scroll(page, x, y);
        self.dirty = true;
    }

    fn begin_edit(&mut self, node: NodeId) {
        let Some(page) = self.page.as_mut() else { return };
        let kind = page.control_kind(node);
        let text = page.control_text(node);
        let max_len = page.max_length(node);
        // Focus (with focus/focusin events) unless the press already did.
        let out = self.engine.focus_element(page, Some(node));
        self.edit = Some(ControlEdit { node, buf: TextBuffer::new(&text), kind, max_len, initial: text });
        self.focus = Some(node);
        self.selection = None;
        self.dirty = true;
        self.apply_outcome(out);
    }

    fn end_edit(&mut self) {
        let Some(edit) = self.edit.take() else { return };
        self.focus = None;
        self.dirty = true;
        if edit.buf.text != edit.initial {
            self.dispatch_to(edit.node, "change", plain_init(false));
        }
        // Blur (blur/focusout) unless focus already moved elsewhere.
        self.page_op(|engine, page| {
            let out = if page.state.focused == Some(edit.node) { engine.focus_element(page, None) } else { ScriptOutcome::default() };
            ((), out)
        });
    }

    /// Push the edit buffer into the DOM.
    fn commit_edit(&mut self) {
        let Some(edit) = self.edit.as_ref() else { return };
        let (node, text) = (edit.node, edit.buf.text.clone());
        if let Some(page) = self.page.as_mut() {
            page.set_control_text(node, &text);
            self.engine.restyle(page);
            let (x, y) = self.scroll;
            self.engine.set_page_scroll(page, x, y);
        }
        self.dirty = true;
        self.dispatch_to(node, "input", input_init(Some(&text)));
    }

    fn place_caret_from_click(&mut self, control: NodeId, doc: (f32, f32)) {
        let Some(page) = self.page.as_ref() else { return };
        let Some(rect) = page.rect_of(control) else { return };
        let text = page.control_text(control);
        let rel_x = doc.0 - rect.x;
        let mut best = text.len();
        let mut best_d = f32::MAX;
        for (i, _) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
            if let Some(cx) = page.caret_x(&self.engine.fonts, control, i) {
                let d = (cx - rel_x).abs();
                if d < best_d {
                    best_d = d;
                    best = i;
                }
            }
        }
        if let Some(edit) = self.edit.as_mut() {
            edit.buf.set_caret(best, false);
        }
        self.dirty = true;
    }

    /// Submit the form containing `control`.
    fn submit_form(&mut self, control: NodeId, submitter: Option<NodeId>) {
        let Some(page) = self.page.as_ref() else { return };
        let Some(form) = page.form_of(control) else {
            self.send(FromContent::Status("This control is not inside a form".into()));
            return;
        };
        let submitter = submitter.or_else(|| page.default_submitter(form));
        let (prevented, navigated) = self.dispatch_to(form, "submit", plain_init(true));
        if prevented || navigated {
            return;
        }
        let Some(page) = self.page.as_ref() else { return };
        let Some(req) = page.submit_request(form, submitter, &|_| Vec::new()) else { return };
        self.end_edit();
        self.send(FromContent::Navigate { request: req, push_history: true });
    }

    fn toggle_control_with_events(&mut self, node: NodeId) {
        self.mutate_page(|p| p.toggle_control(node));
        self.dispatch_to(node, "input", input_init(None));
        self.dispatch_to(node, "change", plain_init(false));
    }

    /// Move keyboard focus to the next/previous link or control.
    fn focus_next(&mut self, forward: bool) {
        let Some(page) = self.page.as_ref() else { return };
        let doc = &page.doc;
        let focusables: Vec<NodeId> = doc
            .descendants(rabbithole_engine::dom::Document::ROOT)
            .filter(|&n| page.styles.get(n).is_some())
            .filter(|&n| rabbithole_engine::forms::is_interactive(doc, n) || (doc.is_element(n, "a") && doc.attr(n, "href").is_some()))
            .collect();
        if focusables.is_empty() {
            return;
        }
        let current = self.edit.as_ref().map(|e| e.node).or(self.focus);
        let pos = current.and_then(|c| focusables.iter().position(|&n| n == c));
        let next = match (pos, forward) {
            (Some(p), true) => focusables[(p + 1) % focusables.len()],
            (Some(p), false) => focusables[(p + focusables.len() - 1) % focusables.len()],
            (None, true) => focusables[0],
            (None, false) => focusables[focusables.len() - 1],
        };
        let kind = page.control_kind(next);
        let rect = page.rect_of(next);
        let interactive = rabbithole_engine::forms::is_interactive(doc, next);
        self.end_edit();
        if matches!(kind, ControlKind::Text | ControlKind::Password | ControlKind::TextArea | ControlKind::Other) && interactive {
            self.begin_edit(next);
        } else {
            if let Some(page) = self.page.as_mut() {
                self.engine.set_focus(page, Some(next));
            }
            self.focus = Some(next);
        }
        if let Some(r) = rect {
            self.scroll_into_view(r);
        }
        self.dirty = true;
    }

    // ---------------------------------------------------------------- keys

    /// Send `keydown`/`keyup` to the page (focused element, else `body`).
    /// Returns true when a listener called `preventDefault()`.
    fn page_key_event(&mut self, k: &KeyInput) -> bool {
        let Some(page) = self.page.as_ref() else { return false };
        if !page.has_scripts() {
            return false;
        }
        let Some(target) = self.focus.or_else(|| page.doc.body()) else { return false };
        let m = k.mods;
        let init = key_init(&k.key, &k.code, m.ctrl, m.shift, m.alt, m.meta, k.repeat);
        let (prevented, navigated) = self.dispatch_to(target, if k.pressed { "keydown" } else { "keyup" }, init);
        prevented || navigated
    }

    fn key(&mut self, k: &KeyInput) {
        if !k.pressed {
            self.page_key_event(k);
            return;
        }
        if self.page_key_event(k) {
            return;
        }
        if self.edit.is_some() {
            self.control_key(k);
        } else {
            self.page_key(k);
        }
    }

    fn control_key(&mut self, k: &KeyInput) {
        let ctrl = k.mods.ctrl || k.mods.meta;
        let shift = k.mods.shift;
        let Some((node, kind)) = self.edit.as_ref().map(|e| (e.node, e.kind)) else { return };
        let mut changed = false;
        let mut clipboard = None;
        {
            let edit = self.edit.as_mut().expect("edit");
            match k.key.as_str() {
                "Enter" => {
                    if kind == ControlKind::TextArea && !ctrl {
                        edit.buf.insert("\n", edit.max_len);
                        changed = true;
                    } else {
                        self.submit_form(node, None);
                        return;
                    }
                }
                "Escape" => {
                    self.end_edit();
                    return;
                }
                "Tab" => {
                    self.focus_next(!shift);
                    return;
                }
                "Backspace" => {
                    edit.buf.backspace(ctrl);
                    changed = true;
                }
                "Delete" => {
                    edit.buf.delete(ctrl);
                    changed = true;
                }
                "ArrowLeft" => edit.buf.move_left(shift, ctrl),
                "ArrowRight" => edit.buf.move_right(shift, ctrl),
                "Home" => edit.buf.home(shift),
                "End" => edit.buf.end(shift),
                " " if !ctrl => {
                    edit.buf.insert(" ", edit.max_len);
                    changed = true;
                }
                key if ctrl && key.chars().count() == 1 => match key.to_lowercase().as_str() {
                    "a" => edit.buf.select_all(),
                    "c" => clipboard = edit.buf.selected_text().map(|s| s.to_string()),
                    "x" => {
                        if let Some(t) = edit.buf.selected_text().map(|s| s.to_string()) {
                            edit.buf.delete_selection();
                            changed = true;
                            clipboard = Some(t);
                        }
                    }
                    "v" => {
                        if let Some(t) = &k.clipboard {
                            edit.buf.insert(t, edit.max_len);
                            changed = true;
                        }
                    }
                    _ => {}
                },
                _ => {
                    if let Some(text) = k.text.as_ref().filter(|t| !t.chars().any(|c| c.is_control())) {
                        if !ctrl {
                            edit.buf.insert(text, edit.max_len);
                            changed = true;
                        }
                    }
                }
            }
        }
        if let Some(t) = clipboard {
            self.send(FromContent::Clipboard(t));
        }
        if changed {
            self.commit_edit();
        }
        self.dirty = true;
    }

    fn page_key(&mut self, k: &KeyInput) {
        let ctrl = k.mods.ctrl || k.mods.meta;
        let shift = k.mods.shift;
        let vh = self.view.viewport.height;
        let page_step = (vh - LINE_SCROLL).max(LINE_SCROLL);
        match k.key.as_str() {
            "ArrowDown" => self.scroll_by(0.0, LINE_SCROLL),
            "ArrowUp" => self.scroll_by(0.0, -LINE_SCROLL),
            "ArrowLeft" => self.scroll_by(-LINE_SCROLL, 0.0),
            "ArrowRight" => self.scroll_by(LINE_SCROLL, 0.0),
            "PageDown" => self.scroll_by(0.0, page_step),
            "PageUp" => self.scroll_by(0.0, -page_step),
            " " => {
                let focused_checkbox = self.focus.filter(|&n| self.page.as_ref().map(|p| matches!(p.control_kind(n), ControlKind::Checkbox | ControlKind::Radio)).unwrap_or(false));
                if let Some(n) = focused_checkbox {
                    self.toggle_control_with_events(n);
                } else {
                    self.scroll_by(0.0, if shift { -page_step } else { page_step });
                }
            }
            "Home" => self.set_scroll(0.0, 0.0),
            "End" => self.set_scroll(0.0, f32::MAX),
            "Backspace" => self.send(FromContent::History(if shift { 1 } else { -1 })),
            "Tab" => self.focus_next(!shift),
            "Enter" => {
                let Some(n) = self.focus else { return };
                let link = self.page.as_ref().and_then(|p| p.link_of(n));
                if let Some(l) = link {
                    self.follow_link(l, ctrl);
                } else if let Some(kind) = self.page.as_ref().map(|p| p.control_kind(n)) {
                    match kind {
                        ControlKind::Submit => self.submit_form(n, Some(n)),
                        ControlKind::Checkbox | ControlKind::Radio => self.toggle_control_with_events(n),
                        ControlKind::Select => self.open_select_dropdown(n),
                        _ => {}
                    }
                }
            }
            "Escape" => {
                self.selection = None;
                self.focus = None;
                self.dirty = true;
            }
            key if ctrl && key.eq_ignore_ascii_case("c") => {
                if let Some(text) = self.selection.as_ref().map(|s| s.text.clone()) {
                    self.send(FromContent::Clipboard(text));
                }
            }
            key if ctrl && key.eq_ignore_ascii_case("a") => {
                let sel = self.page.as_ref().map(|p| p.select_between(&self.engine.fonts, (0.0, 0.0), (p.content_width(), p.content_height())));
                self.selection = sel.filter(|s| !s.rects.is_empty());
                self.dirty = true;
            }
            _ => {}
        }
    }

    // ---------------------------------------------------------------- find

    fn refresh_find(&mut self, scroll: bool) {
        let matches = match self.page.as_ref() {
            Some(p) if !self.find.query.is_empty() => p.find_text(&self.engine.fonts, &self.find.query),
            _ => Vec::new(),
        };
        self.find.current = self.find.current.min(matches.len().saturating_sub(1));
        self.find.matches = matches;
        self.dirty = true;
        if scroll {
            if let Some(r) = self.find.matches.get(self.find.current).copied() {
                self.scroll_into_view(r);
            }
        }
        self.send(FromContent::FindResult { count: self.find.matches.len(), current: self.find.current });
    }

    fn find_step(&mut self, delta: isize) {
        let n = self.find.matches.len();
        if n == 0 {
            return;
        }
        self.find.current = ((self.find.current as isize + delta).rem_euclid(n as isize)) as usize;
        let r = self.find.matches[self.find.current];
        self.scroll_into_view(r);
        self.dirty = true;
        self.send(FromContent::FindResult { count: n, current: self.find.current });
    }
}

/// Drop messages superseded by a later one of the same kind right after
/// them (pointer moves, scrolls, resizes): the content thread catches up
/// with the latest state instead of replaying every step.
fn coalesce(batch: Vec<Inbox>) -> Vec<Inbox> {
    fn kind(m: &Inbox) -> u8 {
        match m {
            Inbox::Ui(ToContent::PointerMove { .. }) => 1,
            Inbox::Ui(ToContent::Scroll { .. }) => 2,
            Inbox::Ui(ToContent::Resize(_)) => 3,
            _ => 0,
        }
    }
    let mut out: Vec<Inbox> = Vec::with_capacity(batch.len());
    for m in batch {
        let k = kind(&m);
        if k != 0 && out.last().map(kind) == Some(k) {
            out.pop();
        }
        out.push(m);
    }
    out
}

#[cfg(test)]
pub(super) fn coalesce_for_tests(batch: Vec<Inbox>) -> Vec<Inbox> {
    coalesce(batch)
}

/// Scroll containers that can scroll (for the UI's wheel routing).
fn collect_scroll_containers(b: &LayoutBox, fixed: bool, out: &mut Vec<(Rect, bool)>) {
    if out.len() >= 256 {
        return;
    }
    let fixed = fixed || b.fixed;
    if b.scroll_container {
        let pr = b.padding_rect();
        if b.scroll_height > pr.h + 0.5 || b.scroll_width > pr.w + 0.5 {
            out.push((pr, fixed));
        }
    }
    for c in &b.children {
        collect_scroll_containers(c, fixed, out);
    }
}
