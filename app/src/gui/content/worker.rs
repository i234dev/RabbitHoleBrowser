//! The content thread of a tab: owns the engine and the page, runs loads,
//! scripts, timers and input handling, and paints frames.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rabbithole_engine::dom::NodeId;
use rabbithole_engine::gfx::{Canvas, Color, Rect};
use rabbithole_engine::layout::LayoutBox;
use rabbithole_engine::net::{Request, Url};
use rabbithole_engine::script::EventInit;
use rabbithole_engine::scripting::{input_init, key_init, mouse_init, plain_init};
use rabbithole_engine::consent::{ConsentDriver, ConsentReport, ConsentStep};
use rabbithole_engine::{ClickAction, ControlKind, Engine, Modifiers, Page, ScriptOutcome, Selection};

use super::frame::{Frame, Pixels};
use super::{now_ms, ContentConfig, ContentSettings, DomRow, FromContent, Inbox, KeyInput, NodeDetails, PageState, Sink, ToContent, ViewSize};
use crate::gui::chrome::{ACCENT, FIND_CURRENT, FIND_MATCH, SELECTION, TEXT};
use crate::gui::input::TextBuffer;
use crate::gui::loader::{self, DownloadPrefs, LoadJob, LoadOutcome, LoadResult};
use crate::gui::pages;

/// Overscan above and below the viewport, in viewport heights.
const OVERSCAN: f32 = 0.5;
/// Load-time scripts run in slices of about this long; the frame is
/// painted and input handled between slices.
const SCRIPT_SLICE: Duration = Duration::from_millis(50);
/// Pointer travel (device px) that turns a press into a drag.
const DRAG_THRESHOLD: f32 = 4.0;
const LINE_SCROLL: f32 = 40.0;
/// Minimum interval between `scroll` events (one per frame).
const SCROLL_EVENT_INTERVAL: Duration = Duration::from_millis(16);

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

/// The last painted page pixels (without overlays), reused by the next
/// paint: scrolling shifts them and only the exposed strip is rastered;
/// page changes repaint only the damaged regions.
struct PaintCache {
    canvas: Canvas,
    origin: (f32, f32),
    scale: f32,
}

/// Paint statistics (tests, `RH_DEBUG_CONTENT`).
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct PaintStats {
    pub full: u64,
    pub partial: u64,
    /// Device pixels rastered by partial paints.
    pub partial_area: u64,
}

/// Partial (incremental) paints done by all content threads.
pub(crate) static PARTIAL_PAINTS: AtomicU64 = AtomicU64::new(0);

/// What the last frame covered.
struct Painted {
    covered: Rect,
    scroll: (f32, f32),
    scale: f32,
    /// The display list depends on the scroll position (fixed / sticky).
    scroll_dependent: bool,
}

pub(super) fn run(tab_id: u64, epoch: u64, config: ContentConfig, sink: Sink, rx: Receiver<Inbox>, tx: Sender<Inbox>, busy: Arc<AtomicU64>) {
    // A Rust panic anywhere in the page's engine (parsing, style, layout,
    // a VM bug) ends only this tab's thread: the UI is told so it can show
    // a crash page and start a fresh content thread.
    let crash_sink = sink.clone();
    let crash_busy = busy.clone();
    let r = rabbithole_js::panics::catch(move || {
        let mut w = Worker::new(tab_id, epoch, config, sink, tx, busy);
        w.main_loop(rx);
    });
    if let Err((msg, loc)) = r {
        crash_busy.store(0, Ordering::Relaxed);
        let at = if loc.is_empty() { String::new() } else { format!(" at {loc}") };
        eprintln!("rabbithole: tab {tab_id} crashed: {msg}{at}");
        crash_sink(tab_id, epoch, FromContent::Crashed(format!("{msg}{at}")));
    }
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
    /// Load-time scripts are running in slices (since when).
    running_scripts: Option<Instant>,
    /// The last turn skipped running timers to paint first.
    tick_deferred: bool,
    /// Load phase after the commit (None: finished).
    load_phase: Option<&'static str>,
    view: ViewSize,
    active: bool,
    scroll: (f32, f32),
    selection: Option<Selection>,
    edit: Option<ControlEdit>,
    /// Inside `end_edit`: don't let script focus sync restart the edit.
    ending_edit: bool,
    focus: Option<NodeId>,
    press: Option<Press>,
    find: Find,
    last_scroll_dispatch: Instant,
    /// A `scroll` event was throttled: fire it once the interval passes
    /// (the last position of a gesture must reach scroll listeners and
    /// IntersectionObservers).
    scroll_event_pending: bool,
    dirty: bool,
    frame_seq: u64,
    painted: Option<Painted>,
    cache: Option<PaintCache>,
    paint_stats: PaintStats,
    /// Node outlined for the devtools Elements view.
    inspect: Option<NodeId>,
    sent_hover: (&'static str, Option<Url>),
    sent_state: PageState,
    sent_title: String,
    sent_favicon: usize,
    /// Handles the cookie-consent dialog of the current page.
    consent: Option<ConsentDriver>,
    /// Recent successful consent clicks per host (reload-loop guard).
    consent_recent: Vec<(String, Instant)>,
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
            running_scripts: None,
            tick_deferred: false,
            load_phase: None,
            view,
            active: false,
            scroll: (0.0, 0.0),
            selection: None,
            edit: None,
            ending_edit: false,
            focus: None,
            press: None,
            find: Find::default(),
            last_scroll_dispatch: Instant::now(),
            scroll_event_pending: false,
            dirty: false,
            frame_seq: 0,
            painted: None,
            cache: None,
            paint_stats: PaintStats::default(),
            inspect: None,
            sent_hover: ("default", None),
            sent_state: PageState::default(),
            sent_title: String::new(),
            sent_favicon: 0,
            consent: None,
            consent_recent: Vec::new(),
            quit: false,
        };
        // Network log for the devtools (per tab: this thread's fetcher).
        let (sink, tab_id, epoch) = (w.sink.clone(), w.tab_id, w.epoch);
        w.engine.loader.fetcher.observer = Some(rabbithole_engine::net::NetObserver(Arc::new(move |ev: &rabbithole_engine::net::NetEvent| sink(tab_id, epoch, FromContent::Network(ev.clone())))));
        let settings = w.config.settings.clone();
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
            let timed_out = first.is_none();
            let mut batch: Vec<Inbox> = first.into_iter().collect();
            while let Ok(m) = rx.try_recv() {
                batch.push(m);
            }
            let t0 = Instant::now();
            let cpu0 = if debug_content() { rabbithole_js::realm::thread_cpu_time() } else { None };
            if self.tick_deferred {
                // Timers held back last turn (a frame went out first) run
                // before anything that arrived since.
                self.tick_deferred = false;
                self.tick_if_due(true);
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
            let t1 = Instant::now();
            self.flush_scroll_event();
            let t2 = Instant::now();
            if self.running_scripts.is_some() && t1 - t0 < SCRIPT_SLICE {
                self.continue_scripts();
            }
            // After a long stretch of event handlers or scripts, paint and
            // send a frame first: due timers run at the start of the next
            // turn, before the messages that arrived meanwhile.
            if t0.elapsed() < SCRIPT_SLICE {
                self.tick_if_due(timed_out);
            } else {
                self.tick_deferred = true;
            }
            self.consent_tick();
            let t3 = Instant::now();
            self.flush();
            if debug_content() && t0.elapsed() > Duration::from_millis(100) {
                let ms = |a: Instant, b: Instant| (b - a).as_secs_f64() * 1000.0;
                let cpu = match (cpu0, rabbithole_js::realm::thread_cpu_time()) {
                    (Some(a), Some(b)) => (b.saturating_sub(a)).as_secs_f64() * 1000.0,
                    _ => -1.0,
                };
                eprintln!(
                    "[content] busy {:.0} ms (cpu {:.0} ms): messages {:.0}, scroll event {:.0}, tick {:.0}, flush/paint {:.0}",
                    ms(t0, Instant::now()),
                    cpu,
                    ms(t0, t1),
                    ms(t1, t2),
                    ms(t2, t3),
                    ms(t3, Instant::now())
                );
            }
        }
    }

    /// How long to wait for messages before running timers.
    fn next_wakeup(&self) -> Option<Duration> {
        if self.running_scripts.is_some() || self.tick_deferred {
            return Some(Duration::ZERO);
        }
        let mut wait: Option<f64> = None;
        if let Some(due) = self.page.as_ref().and_then(|p| p.next_script_due()) {
            let now = self.engine.start.elapsed().as_secs_f64() * 1000.0;
            wait = Some((due - now).max(0.0));
        }
        if self.load_phase.is_some() && !self.awaiting_scripts {
            // Watch for the end of image loading.
            wait = Some(wait.map(|w| w.min(200.0)).unwrap_or(200.0));
        }
        if let Some(next) = self.consent.as_ref().and_then(|d| d.next).filter(|_| !self.awaiting_scripts && self.running_scripts.is_none()) {
            let left = next.saturating_duration_since(Instant::now()).as_secs_f64() * 1000.0;
            wait = Some(wait.map(|w| w.min(left)).unwrap_or(left));
        }
        if self.scroll_event_pending {
            let left = SCROLL_EVENT_INTERVAL.saturating_sub(self.last_scroll_dispatch.elapsed()).as_secs_f64() * 1000.0;
            wait = Some(wait.map(|w| w.min(left)).unwrap_or(left));
        }
        wait.map(|ms| Duration::from_micros((ms * 1000.0) as u64))
    }

    /// Run timers and image polling when due (`woken`: the wait for the
    /// due time ran out; image polls always report a time slightly ahead).
    fn tick_if_due(&mut self, woken: bool) {
        let Some(page) = self.page.as_mut() else { return };
        let Some(due) = page.next_script_due() else { return };
        let now = self.engine.start.elapsed().as_secs_f64() * 1000.0;
        if due > now + if woken { 30.0 } else { 0.5 } {
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
        let (wheel_listeners, wheel_blocking) = self.engine.wheel_listeners(page);
        if !lines.is_empty() {
            self.send(FromContent::Console(lines));
        }
        let state = PageState { editing: self.edit.is_some(), key_listeners, focused: self.focus.is_some(), has_selection: self.selection.is_some(), wheel_listeners, wheel_blocking };
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
            ToContent::Wheel { x, y, dx, dy, mods, scrolled: ui_scrolled } => {
                // Scripts see the wheel first and may cancel the scroll
                // (custom zoom / pan widgets, carousels, maps).
                let prevented = self.page_op(|engine, page| engine.wheel(page, x, y, dx, dy, mods)).unwrap_or(false);
                if prevented || ui_scrolled {
                    return;
                }
                let doc = (x + self.scroll.0, y + self.scroll.1);
                let (scrolled, container) = match self.page.as_mut() {
                    Some(page) => {
                        let container = page.layout.scroll_container_at(doc.0, doc.1).map(|i| i.node);
                        (self.engine.scroll_at(page, doc.0, doc.1, dx, dy), container)
                    }
                    None => (false, None),
                };
                if scrolled {
                    self.dirty = true;
                    // The container fires `scroll` (not bubbling); its
                    // IntersectionObservers re-check.
                    if let Some(node) = container {
                        self.dispatch_to(node, "scroll", EventInit::simple(false, false));
                        self.after_script_activity();
                    }
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
                let (result, out) = match self.page.as_mut() {
                    Some(page) => self.engine.eval_with_outcome(page, &src),
                    None => ("(no page)".to_string(), ScriptOutcome::default()),
                };
                self.apply_outcome(out);
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
            ToContent::PrintPdf(path) => self.print_pdf(path),
            ToContent::InspectTree => self.send_dom_tree(),
            ToContent::InspectNode(node) => self.inspect_node(node),
            ToContent::InspectAt { x, y } => {
                let node = self.page.as_ref().and_then(|p| p.node_at_viewport(x, y));
                self.send_dom_tree();
                self.inspect_node(node);
            }
            ToContent::SaveUrl(url) => {
                let referer = self.page.as_ref().map(|p| p.url.clone());
                let mut req = Request::get(url);
                if let Some(r) = referer {
                    req = req.with_referer(r);
                }
                let sink = self.sink.clone();
                let (tab_id, epoch) = (self.tab_id, self.epoch);
                loader::spawn_download(self.engine.loader.clone(), req, self.download_prefs(), move |d| sink(tab_id, epoch, FromContent::Download(d)));
            }
            ToContent::Shutdown => self.quit = true,
            #[cfg(test)]
            ToContent::Crash => panic!("deliberate content thread crash"),
        }
    }

    fn apply_settings(&mut self, s: ContentSettings) {
        self.engine.scripting = s.javascript;
        self.engine.loader.load_scripts = s.javascript;
        self.engine.loader.load_images = s.images;
        self.engine.loader.fetcher.block_trackers = s.block_trackers;
        self.engine.loader.fetcher.accept_language = s.accept_language.clone();
        let policy_changed = s.consent != self.config.settings.consent;
        self.config.settings = s;
        if policy_changed {
            // A new mode applies to the page on screen right away.
            let choice = self.page.as_ref().and_then(|p| self.consent_choice(p));
            match choice {
                None => self.consent = None,
                Some(c) if self.consent.as_ref().map(|d| d.choice != c || d.finished()).unwrap_or(true) => {
                    self.consent = Some(ConsentDriver::new(c, Instant::now()));
                }
                _ => {}
            }
        }
    }

    fn download_prefs(&self) -> DownloadPrefs {
        DownloadPrefs { dir: self.config.settings.download_dir.clone(), ask: self.config.settings.ask_download, next_id: self.config.next_download_id.clone() }
    }

    // ------------------------------------------------------------ devtools

    /// The DOM as indented rows (elements and non-blank text).
    fn send_dom_tree(&mut self) {
        let Some(page) = self.page.as_ref() else { return };
        let doc = &page.doc;
        let mut rows = Vec::new();
        fn walk(doc: &rabbithole_engine::dom::Document, n: NodeId, depth: usize, rows: &mut Vec<DomRow>) {
            if rows.len() >= 5000 {
                return;
            }
            if let Some(e) = doc.element(n) {
                let mut label = format!("<{}", e.tag);
                for (k, v) in e.attrs.iter().take(6) {
                    let v: String = v.chars().take(60).collect();
                    label.push_str(&format!(" {k}=\"{v}\""));
                }
                label.push('>');
                rows.push(DomRow { node: n, depth, label, is_text: false });
                if matches!(e.tag.as_str(), "script" | "style") {
                    return;
                }
                for c in doc.children(n) {
                    walk(doc, c, depth + 1, rows);
                }
            } else if let Some(t) = doc.text(n) {
                let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
                if !t.is_empty() {
                    let mut label: String = t.chars().take(80).collect();
                    if t.chars().count() > 80 {
                        label.push('…');
                    }
                    rows.push(DomRow { node: n, depth, label: format!("\"{label}\""), is_text: true });
                }
            } else {
                for c in doc.children(n) {
                    walk(doc, c, depth, rows);
                }
            }
        }
        walk(doc, rabbithole_engine::dom::Document::ROOT, 0, &mut rows);
        self.send(FromContent::DomTree(rows));
    }

    fn inspect_node(&mut self, node: Option<NodeId>) {
        // Text nodes are inspected through their parent element.
        let node = node.map(|n| if self.page.as_ref().map(|p| p.doc.element(n).is_none()).unwrap_or(false) { self.page.as_ref().and_then(|p| p.doc.parent(n)).unwrap_or(n) } else { n });
        self.inspect = node;
        self.dirty = true;
        let Some(n) = node else {
            self.send(FromContent::NodeDetails(NodeDetails::default()));
            return;
        };
        let Some(page) = self.page.as_mut() else { return };
        let rect = page.rect_of(n);
        let attrs = page.doc.element(n).map(|e| e.attrs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()).unwrap_or_default();
        let styles = self.engine.computed_style_now(page, n).map(|cs| super::style_summary(&cs, rect)).unwrap_or_default();
        if let Some(r) = rect {
            self.scroll_into_view(r);
        }
        self.send(FromContent::NodeDetails(NodeDetails { node: Some(n), styles, attrs }));
    }

    /// "Print": rasterize the page into A4 pages and write a PDF.
    fn print_pdf(&mut self, path: std::path::PathBuf) {
        let Some(page) = self.page.as_mut() else { return };
        self.engine.flush_display_list(page);
        let t = Instant::now();
        let pages = crate::gui::pdf::paginate(&page.display_list, &self.engine.fonts, page.viewport.width, page.content_height(), page.background, 2.0);
        let title = if page.title.trim().is_empty() { page.url.to_string() } else { page.title.trim().to_string() };
        let pdf = crate::gui::pdf::write_pdf(&pages, crate::gui::pdf::A4, &title);
        debug_time("print to PDF", t);
        let msg = match std::fs::write(&path, pdf) {
            Ok(()) => format!("Saved PDF ({} page{}): {}", pages.len(), if pages.len() == 1 { "" } else { "s" }, path.display()),
            Err(e) => format!("Could not save the PDF: {e}"),
        };
        self.send(FromContent::Status(msg));
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
        self.running_scripts = None;
        self.load_phase = None;
        let job = LoadJob {
            loader: self.engine.loader.clone(),
            request,
            viewport: self.view.viewport,
            downloads: self.download_prefs(),
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
                let t = Instant::now();
                let page = self.engine.build_page_unscripted(*loaded, self.view.viewport);
                debug_time("build (style + layout)", t);
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
                self.engine.start_page_load_scripts(page);
                self.running_scripts = Some(Instant::now());
                self.continue_scripts();
            }
        }
    }

    /// Run the next slice of load-time scripts; once they are all done,
    /// finish the load (outcome, title, image phase, fragment).
    fn continue_scripts(&mut self) {
        let Some(started) = self.running_scripts else { return };
        let Some(page) = self.page.as_mut() else {
            self.running_scripts = None;
            return;
        };
        if !self.engine.continue_page_load(page, Some(SCRIPT_SLICE)) {
            // Paint what the scripts built so far and handle input before
            // the next slice.
            self.dirty = true;
            return;
        }
        self.running_scripts = None;
        debug_time("scripts", started);
        if let Some(d) = self.consent.as_mut() {
            d.extend_watch(Instant::now());
        }
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
        self.running_scripts = None;
        self.edit = None;
        self.focus = None;
        self.press = None;
        self.find.matches.clear();
        self.find.current = 0;
        self.scroll = (0.0, 0.0);
        self.painted = None;
        self.inspect = None;
        self.sent_hover = ("default", None);
        let title = if page.title.trim().is_empty() { page.url.to_string() } else { page.title.trim().to_string() };
        self.sent_title = title.clone();
        self.load_phase = if self.awaiting_scripts { Some("Loading scripts") } else { None };
        self.send(FromContent::Committed { url: page.url.clone(), title, push_history });
        self.send(FromContent::Hover { cursor: "default", link: None });
        let fragment = page.url.fragment.clone();
        self.consent = self.consent_choice(&page).map(|c| ConsentDriver::new(c, Instant::now()));
        self.page = Some(page);
        self.dirty = true;
        if let Some(f) = fragment {
            self.scroll_to_fragment(&f);
        }
        if !self.find.query.is_empty() {
            self.refresh_find(false);
        }
    }

    // ------------------------------------------------------ cookie dialogs

    /// What to do with this page's consent dialog (None: leave it alone).
    fn consent_choice(&self, page: &Page) -> Option<rabbithole_engine::consent::Choice> {
        if !matches!(page.url.scheme.as_str(), "http" | "https") {
            return None;
        }
        let host = page.url.host.clone();
        // Reload-loop guard: a site that keeps showing its dialog after
        // three handled ones within a minute is left alone.
        let recent = self.consent_recent.iter().filter(|(h, t)| *h == host && t.elapsed() < Duration::from_secs(60)).count();
        if recent >= 3 {
            return None;
        }
        self.config.settings.consent.choice_for(&host)
    }

    /// Run the consent driver when due.
    fn consent_tick(&mut self) {
        if self.awaiting_scripts || self.running_scripts.is_some() {
            return;
        }
        let Some(mut d) = self.consent.take() else { return };
        let now = Instant::now();
        if d.next.map(|n| n > now).unwrap_or(true) {
            self.consent = Some(d);
            return;
        }
        let Some(page) = self.page.as_mut() else { return };
        let step = d.step(&self.engine, page, now);
        match step {
            ConsentStep::Idle => self.consent = Some(d),
            ConsentStep::Finished(r) => {
                self.consent = Some(d);
                self.report_consent(r);
            }
            ConsentStep::Clicked { action, outcome } => {
                self.dirty = true;
                let leaves = outcome.navigation.is_some() || matches!(action, ClickAction::Navigate { new_tab: false, .. } | ClickAction::Submit { .. });
                let report = if leaves { d.navigated() } else { None };
                self.consent = Some(d);
                if let Some(r) = report {
                    self.report_consent(r);
                }
                self.apply_outcome(outcome);
                match action {
                    ClickAction::Navigate { url, new_tab: false } => self.follow_link(url, false),
                    ClickAction::Submit { form, submitter } => self.submit_form(submitter.unwrap_or(form), submitter),
                    _ => {}
                }
                self.after_script_activity();
            }
        }
    }

    fn report_consent(&mut self, r: ConsentReport) {
        let Some(page) = self.page.as_ref() else { return };
        let Some(choice) = self.consent.as_ref().map(|d| d.choice) else { return };
        use rabbithole_engine::consent::ConsentOutcome;
        let host = page.url.host.clone();
        if matches!(r.outcome, ConsentOutcome::Accepted | ConsentOutcome::Rejected) {
            self.consent_recent.retain(|(_, t)| t.elapsed() < Duration::from_secs(60));
            self.consent_recent.push((host.clone(), Instant::now()));
        }
        let ev = crate::autocookies::ConsentEvent { host, url: page.url.to_string(), outcome: r.outcome, choice, button: r.button, vendor: r.vendor, in_frame: r.in_frame, steps: r.steps };
        self.send(FromContent::Consent(ev));
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
        if self.last_scroll_dispatch.elapsed() >= SCROLL_EVENT_INTERVAL {
            self.scroll_event_pending = false;
            self.last_scroll_dispatch = Instant::now();
            self.dispatch_window("scroll");
        } else {
            self.scroll_event_pending = true;
        }
    }

    /// Fire a throttled `scroll` event when its time has come.
    fn flush_scroll_event(&mut self) {
        if self.scroll_event_pending && self.last_scroll_dispatch.elapsed() >= SCROLL_EVENT_INTERVAL {
            self.dispatch_scroll_event();
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
        // An element named by the fragment wins over the special `top`.
        if let Some(y) = self.page.as_ref().and_then(|p| p.fragment_scroll(fragment)) {
            self.set_scroll(0.0, y);
        }
    }

    // ------------------------------------------------------------ painting

    fn paint(&mut self) {
        let t = Instant::now();
        self.paint_frame();
        debug_time("paint", t);
    }

    fn paint_frame(&mut self) {
        let Some(page) = self.page.as_mut() else { return };
        self.dirty = false;
        let s = self.view.scale;
        let vp = self.view.viewport;
        let (sx, sy) = self.scroll;
        let content_h = page.content_height().max(vp.height);
        let margin = vp.height * OVERSCAN;
        // The canvas origin sits on a whole device pixel, so moving it
        // shifts the old pixels exactly.
        let oy = ((sy - margin).max(0.0) * s).floor() / s;
        let bottom = (sy + vp.height + margin).min(content_h.max(sy + vp.height));
        let max_h = self.view.height as f32 * (1.0 + 2.0 * OVERSCAN) + 2.0;
        let h = (((bottom - oy) * s).ceil()).clamp(1.0, max_h.min(16384.0)) as u32;
        let w = self.view.width.max(1);
        let origin = (sx, oy);
        let fixed = page.layout.has_sticky_or_fixed();
        let background = page.background;

        // Incremental paths need a matching cached canvas, no content that
        // moves with the scroll (fixed / sticky) and scale >= 1 (region
        // culling works in canvas units).
        // `partial`: the cached pixels brought up to date; `spare`: a
        // canvas of the right size to repaint fully.
        let mut partial: Option<Canvas> = None;
        let mut spare: Option<Canvas> = None;
        if let Some(c) = self.cache.take() {
            let compatible = !fixed && s >= 1.0 && (c.scale - s).abs() < 1e-4 && c.canvas.width == w && c.canvas.height == h && c.origin.0 == origin.0;
            let shift = (origin.1 - c.origin.1) * s;
            let dy = shift.round() as i64;
            let exact = (shift - dy as f32).abs() < 0.01 && dy.unsigned_abs() < h as u64;
            let damage = if compatible && exact { self.engine.paint_damage_since(page, c.origin) } else { None };
            let mut cv = c.canvas;
            match damage {
                Some(damage) => {
                    shift_rows(&mut cv, dy);
                    // Damage (CSS px relative to the old origin) → device px now.
                    let mut regions: Vec<Rect> = damage.iter().map(|r| Rect::new(r.x * s, (r.y + c.origin.1 - origin.1) * s, r.w * s, r.h * s)).collect();
                    if dy > 0 {
                        regions.push(Rect::new(0.0, (h as i64 - dy) as f32, w as f32, dy as f32));
                    } else if dy < 0 {
                        regions.push(Rect::new(0.0, 0.0, w as f32, (-dy) as f32));
                    }
                    let full = Rect::new(0.0, 0.0, w as f32, h as f32);
                    let regions: Vec<Rect> = rabbithole_engine::layout::merge_damage(regions)
                        .into_iter()
                        .map(|r| r.intersect(&full))
                        .filter(|r| !r.is_empty())
                        .map(|r| Rect::new(r.x.floor(), r.y.floor(), r.right().ceil() - r.x.floor(), r.bottom().ceil() - r.y.floor()))
                        .collect();
                    let area: f32 = regions.iter().map(|r| r.w * r.h).sum();
                    if area <= 0.6 * w as f32 * h as f32 {
                        for r in &regions {
                            cv.push_clip(*r);
                            cv.fill_rect(*r, Color::WHITE);
                            if background.a > 0 {
                                cv.fill_rect(*r, background);
                            }
                            cv.pop_clip();
                            cv.push_transform([s, 0.0, 0.0, s, 0.0, 0.0]);
                            let css = Rect::new(r.x / s, r.y / s, r.w / s, r.h / s);
                            rabbithole_engine::layout::paint_region(&page.display_list, &mut cv, &self.engine.fonts, origin.0, origin.1, css);
                            cv.pop_transform();
                        }
                        self.paint_stats.partial += 1;
                        self.paint_stats.partial_area += area as u64;
                        PARTIAL_PAINTS.fetch_add(1, Ordering::Relaxed);
                        partial = Some(cv);
                    } else {
                        spare = Some(cv);
                    }
                }
                None => {
                    if cv.width == w && cv.height == h {
                        spare = Some(cv);
                    }
                }
            }
        }
        let mut cv = match partial {
            Some(c) => c,
            None => {
                let mut cv = spare.unwrap_or_else(|| Canvas::new(w, h));
                cv.clear(Color::WHITE);
                if background.a > 0 {
                    cv.fill_rect(Rect::new(0.0, 0.0, w as f32, h as f32), background);
                }
                // Fixed content (interleaved in stacking order) is painted
                // where the viewport is inside the overscanned canvas.
                cv.push_transform([s, 0.0, 0.0, s, 0.0, 0.0]);
                rabbithole_engine::layout::paint_with_fixed_origin(&page.display_list, &mut cv, &self.engine.fonts, origin.0, origin.1, (sx - origin.0, sy - origin.1));
                cv.pop_transform();
                self.paint_stats.full += 1;
                cv
            }
        };
        self.engine.mark_painted(page, origin);

        // The frame: the page pixels plus overlays (selection, find, caret).
        let mut frame_canvas = Canvas::new(w, h);
        frame_canvas.data.copy_from_slice(&cv.data);
        self.paint_overlays(&mut frame_canvas, origin);
        self.cache = Some(PaintCache { canvas: std::mem::replace(&mut cv, Canvas::new(1, 1)), origin, scale: s });

        let page = self.page.as_ref().expect("page");
        let mut containers = Vec::new();
        collect_scroll_containers(&page.layout.root, false, &mut containers);
        let covered = Rect::new(origin.0, origin.1, w as f32 / s, h as f32 / s);
        self.frame_seq += 1;
        let frame = Frame {
            seq: self.frame_seq,
            canvas: Arc::new(Pixels::from_canvas(&mut frame_canvas)),
            origin,
            scale: s,
            scroll: (sx, sy),
            content: (page.content_width(), page.content_height()),
            background: if page.background.a > 0 { page.background } else { Color::WHITE },
            scroll_containers: Arc::new(containers),
        };
        self.painted = Some(Painted { covered, scroll: (sx, sy), scale: s, scroll_dependent: fixed });
        self.send(FromContent::Frame(frame));
    }

    /// Selection, find highlights, focus ring and caret.
    fn paint_overlays(&self, canvas: &mut Canvas, origin: (f32, f32)) {
        let Some(page) = self.page.as_ref() else { return };
        let s = self.view.scale;
        let map = |r: Rect| Rect::new((r.x - origin.0) * s, (r.y - origin.1) * s, r.w * s, r.h * s);
        if let Some(r) = self.inspect.and_then(|n| page.rect_of(n)).map(map) {
            canvas.fill_rect(r, rabbithole_engine::gfx::Color::rgba(0x6f, 0xa8, 0xdc, 110));
            canvas.stroke_rect(r, [1.0; 4], [rabbithole_engine::gfx::Color::rgb(0x1a, 0x73, 0xe8); 4], [0; 4], 0.0);
        }
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
            return;
        }
        self.sync_script_focus();
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
                        // Scripts still see the moves of a selecting drag
                        // (drag-to-pan maps and timelines, sliders, sortable
                        // lists listen for pointermove / mousemove).
                        self.page_op(|engine, page| ((), engine.pointer_move(page, x, y, mods)));
                        return;
                    }
                }
            }
        }
        // Hover state + pointer/mouse events from the same hit test as clicks.
        self.page_op(|engine, page| ((), engine.pointer_move(page, x, y, mods)));
        let Some(page) = self.page.as_ref() else { return };
        // Cursor and link from the innermost document under the pointer
        // (iframes included).
        let rabbithole_engine::frames::HoverInfo { cursor, link } = page.hover_info(x, y);
        let _ = doc;
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
                let image = node.and_then(|n| self.page.as_ref().and_then(|p| image_of(p, n)));
                self.send(FromContent::ContextMenu { link, image, has_selection: self.selection.is_some() });
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
        self.ending_edit = true;
        if edit.buf.text != edit.initial {
            self.dispatch_to(edit.node, "change", plain_init(false));
        }
        // Blur (blur/focusout) unless focus already moved elsewhere.
        self.page_op(|engine, page| {
            let out = if page.state.focused == Some(edit.node) { engine.focus_element(page, None) } else { ScriptOutcome::default() };
            ((), out)
        });
        self.ending_edit = false;
        self.sync_script_focus();
    }

    /// Follow focus moved by scripts (`el.focus()`, a click handler that
    /// focuses the real `<input>` under a decorated field, `blur()`): a
    /// focused text control takes the keyboard like a clicked one, and the
    /// edit ends when scripts move focus away.
    fn sync_script_focus(&mut self) {
        if self.ending_edit {
            return;
        }
        let Some(page) = self.page.as_ref() else { return };
        let focused = page.state.focused;
        let editing = self.edit.as_ref().map(|e| e.node);
        if focused == editing {
            return;
        }
        let editable = focused.filter(|&n| {
            matches!(page.control_kind(n), ControlKind::Text | ControlKind::Password | ControlKind::TextArea)
                && page.doc.attr(n, "disabled").is_none()
                && page.doc.attr(n, "readonly").is_none()
        });
        if editing.is_some() {
            // Focus already left the control: commit it (change) without
            // another blur.
            self.end_edit();
            if self.edit.is_some() {
                return;
            }
        }
        let Some(page) = self.page.as_ref() else { return };
        if let Some(n) = editable.filter(|&n| page.state.focused == Some(n)) {
            let text = page.control_text(n);
            let len = text.len();
            self.edit = Some(ControlEdit { node: n, buf: TextBuffer::new(&text), kind: page.control_kind(n), max_len: page.max_length(n), initial: text });
            if let Some(e) = self.edit.as_mut() {
                e.buf.set_caret(len, false);
            }
            self.focus = Some(n);
            self.selection = None;
            self.dirty = true;
        }
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
        // Keys go into a focused iframe's document.
        let routed = self.page.as_mut().and_then(|page| self.engine.frame_key_event(page, &k.key, &k.code, k.pressed, k.mods));
        if let Some(out) = routed {
            self.apply_outcome(out);
            return k.key != "Tab";
        }
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
        // Keyboard input: focus moved from now on matches `:focus-visible`.
        if !(k.mods.ctrl || k.mods.alt || k.mods.meta) {
            if let Some(page) = self.page.as_mut() {
                page.pointer.pointer_modality = false;
            }
        }
        if self.page_key_event(k) {
            return;
        }
        // Escape closes the topmost modal dialog (after its `cancel`).
        if k.key == "Escape" && self.page_op(|engine, page| engine.cancel_modal(page).map_or((false, ScriptOutcome::default()), |o| (true, o))).unwrap_or(false) {
            self.dirty = true;
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
        self.send(FromContent::FindResult { count: self.find.matches.len(), current: self.find.current });
        if scroll {
            if let Some(r) = self.find.matches.get(self.find.current).copied() {
                self.scroll_into_view(r);
            }
        }
    }

    fn find_step(&mut self, delta: isize) {
        let n = self.find.matches.len();
        if n == 0 {
            return;
        }
        self.find.current = ((self.find.current as isize + delta).rem_euclid(n as isize)) as usize;
        let r = self.find.matches[self.find.current];
        self.send(FromContent::FindResult { count: n, current: self.find.current });
        self.scroll_into_view(r);
        self.dirty = true;
    }
}

fn debug_content() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RH_DEBUG_CONTENT").is_some())
}

/// `RH_DEBUG_CONTENT=1`: log how long content-thread steps take.
fn debug_time(what: &str, since: Instant) {
    if debug_content() {
        eprintln!("[content {:?}] {what}: {:.1} ms", std::thread::current().name().unwrap_or("?"), since.elapsed().as_secs_f64() * 1000.0);
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

/// Move the rows of a canvas up by `dy` device rows (down when negative):
/// the content of a scroll by `dy`.
fn shift_rows(c: &mut Canvas, dy: i64) {
    let (w, h) = (c.width as usize, c.height as i64);
    if dy == 0 || dy.abs() >= h {
        return;
    }
    if dy > 0 {
        let d = dy as usize * w;
        c.data.copy_within(d.., 0);
    } else {
        let d = (-dy) as usize * w;
        let n = c.data.len() - d;
        c.data.copy_within(..n, d);
    }
}

#[cfg(test)]
pub(super) fn shift_rows_for_tests(c: &mut Canvas, dy: i64) {
    shift_rows(c, dy)
}

/// The image (`<img src>`) under a node, resolved.
fn image_of(page: &Page, node: NodeId) -> Option<Url> {
    let img = page.doc.ancestor_or_self(node, |n| page.doc.is_element(n, "img"))?;
    let src = page.doc.attr(img, "src")?;
    if src.trim().is_empty() {
        return None;
    }
    page.resolve(src)
}
