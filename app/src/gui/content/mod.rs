//! Content threads: every tab's engine and page live on a thread of their
//! own for the tab's whole life, so nothing a page does (parsing, style,
//! layout, scripts, timers) can freeze the window.
//!
//! The UI thread talks to a tab through a [`ContentHandle`]: it sends
//! [`ToContent`] messages (navigation, input, scrolling, resizing, console
//! evaluation, ...) and receives [`FromContent`] messages through a sink
//! callback (rasterized [`Frame`]s, title, URL, cursor, console lines,
//! navigation requests, dialogs, ...). Nothing here depends on the window
//! system, so the protocol is tested headlessly (see `tests.rs`).
//!
//! Frames are painted taller than the viewport (an overscan margin above
//! and below), so the UI can scroll the last frame immediately while the
//! content thread is busy and catch up later.

mod frame;
mod worker;

#[cfg(test)]
mod tests;

pub use frame::Frame;
#[cfg(test)]
pub use frame::Pixels;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rabbithole_engine::dom::NodeId;
use rabbithole_engine::gfx::{Image, Rect};
use rabbithole_engine::net::{Request, Url};
use rabbithole_engine::{LocalStorage, Loader, Modifiers, Viewport};
use super::browsing_context::{BlankOrigin, WindowBinding};

use crate::gui::loader::{DownloadEvent, LoadResult};

/// Size of a tab's page area.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewSize {
    /// CSS viewport.
    pub viewport: Viewport,
    /// Device pixels per CSS pixel (HiDPI scale × zoom).
    pub scale: f32,
    /// Page area in device pixels.
    pub width: u32,
    pub height: u32,
}

impl ViewSize {
    /// View of `width × height` device pixels at `scale`.
    pub fn new(width: u32, height: u32, scale: f32) -> ViewSize {
        let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        let (width, height) = (width.max(1), height.max(1));
        ViewSize { viewport: Viewport { width: (width as f32 / scale).max(1.0), height: (height as f32 / scale).max(1.0) }, scale, width, height }
    }
}

/// Browser settings that affect pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentSettings {
    pub javascript: bool,
    pub images: bool,
    pub block_trackers: bool,
    /// `Accept-Language` header.
    pub accept_language: String,
    /// Where downloads go.
    pub download_dir: PathBuf,
    /// Ask where to save each download.
    pub ask_download: bool,
    /// Automatic handling of cookie-consent dialogs.
    pub consent: crate::autocookies::ConsentPolicy,
}

impl ContentSettings {
    pub fn new(javascript: bool, images: bool, block_trackers: bool) -> ContentSettings {
        ContentSettings {
            javascript,
            images,
            block_trackers,
            accept_language: "es-ES,es;q=0.9,en;q=0.8".into(),
            download_dir: std::env::temp_dir(),
            ask_download: false,
            consent: crate::autocookies::ConsentPolicy::default(),
        }
    }
}

/// A node of the DOM tree shown in the devtools Elements view.
#[derive(Debug, Clone, PartialEq)]
pub struct DomRow {
    pub node: NodeId,
    pub depth: usize,
    /// `<div id="a" class="b">` or `"text…"`.
    pub label: String,
    pub is_text: bool,
}

/// Details of the inspected element.
#[derive(Debug, Clone, Default)]
pub struct NodeDetails {
    pub node: Option<NodeId>,
    /// Computed style (property, value).
    pub styles: Vec<(String, String)>,
    /// Attributes.
    pub attrs: Vec<(String, String)>,
}

/// A key press or release for the page (`key` uses DOM `KeyboardEvent.key`
/// names: "a", "Enter", "ArrowDown", " ", ...).
#[derive(Debug, Clone, Default)]
pub struct KeyInput {
    pub key: String,
    pub code: String,
    /// Text the key types (None for named keys and shortcuts).
    pub text: Option<String>,
    pub mods: Modifiers,
    pub repeat: bool,
    pub pressed: bool,
    /// Clipboard contents for a paste shortcut (read on the UI thread).
    pub clipboard: Option<String>,
}

/// Messages from the UI thread to a content thread.
#[derive(Debug, Clone)]
pub enum ToContent {
    /// Load a URL (already checked by the UI: no internal schemes).
    Navigate { request: Request, push_history: bool },
    /// A real top-level proxy command; only its owning context may consume it.
    ContextNavigate { id: u64, ticket: u64, request: Request, replace: bool, blank: Option<BlankOrigin>, javascript_origin: Option<String> },
    ContextMessage { id: u64, ticket: u64, message: rabbithole_engine::script::WindowMessage },
    /// Show generated HTML (internal pages) as if loaded from `url`.
    LoadHtml { html: String, url: Url, push_history: bool },
    /// Cancel the load in progress.
    Stop,
    Resize(ViewSize),
    /// The tab became visible (frames are only painted while active).
    SetActive(bool),
    /// The page scroll position (CSS px) set by the UI.
    Scroll { x: f32, y: f32 },
    /// Pointer at viewport CSS coordinates (may be outside the page area
    /// during a drag, clamped by the UI).
    PointerMove { x: f32, y: f32, mods: Modifiers },
    PointerDown { x: f32, y: f32, button: u8, mods: Modifiers, click_count: u8 },
    /// `inside` = released over the page area.
    PointerUp { x: f32, y: f32, button: u8, mods: Modifiers, inside: bool },
    /// Wheel over a scroll container, or over a page with `wheel`
    /// listeners: a `wheel` event first; unless cancelled the container
    /// scrolls (or the page: `ScrollBy` back to the UI). `scrolled`: the UI
    /// already scrolled the page (only passive listeners: just notify).
    Wheel { x: f32, y: f32, dx: f32, dy: f32, mods: Modifiers, scrolled: bool },
    Key(KeyInput),
    /// Text typed through the IME.
    Text(String),
    Find(String),
    FindStep(isize),
    FindClose,
    SelectOption { node: NodeId, index: usize },
    CopySelection,
    /// Evaluate a console line.
    Eval(String),
    JavascriptUrl(Url),
    /// Show the page's readable text (about:text).
    ShowText,
    Settings(ContentSettings),
    /// Render the page to a PDF file.
    PrintPdf(PathBuf),
    /// Send the DOM tree (devtools Elements view).
    InspectTree,
    /// Send the details of a node (and outline it on the page).
    InspectNode(Option<NodeId>),
    /// Inspect the element at a viewport point (context menu "Inspect").
    InspectAt { x: f32, y: f32 },
    /// Save the URL to disk (save link / image as).
    SaveUrl(Url),
    Shutdown,
    /// Panic on the content thread (tests of the crash recovery).
    #[cfg(test)]
    Crash,
}

/// What the UI should know about the page to route input.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PageState {
    /// A text control is being edited (keys type into it).
    pub editing: bool,
    /// Scripts listen to key events (keys must go through the page).
    pub key_listeners: bool,
    /// An element has keyboard focus.
    pub focused: bool,
    /// Scripts listen for `wheel` (`wheel_blocking`: with a listener that
    /// may cancel it, so the UI must let the page decide before scrolling).
    pub wheel_listeners: bool,
    pub wheel_blocking: bool,
    pub has_selection: bool,
}

/// Messages from a content thread to the UI thread.
#[derive(Debug, Clone)]
pub enum FromContent {
    Frame(Frame),
    /// A new document replaced the old one (first paint follows).
    Committed { url: Url, title: String, push_history: bool },
    /// Load progress after the commit (None = finished).
    Loading(Option<&'static str>),
    Title(String),
    /// Same-document navigation (fragment links).
    UrlChanged { url: Url, push_history: bool },
    Favicon(Option<Arc<Image>>),
    Hover { cursor: &'static str, link: Option<Url> },
    State(PageState),
    Console(Vec<(String, String)>),
    EvalResult(String),
    /// The page asks to navigate (links, forms, scripts); the UI routes it
    /// (internal pages, new tabs) and answers with `ToContent::Navigate`.
    Navigate { request: Request, push_history: bool },
    OpenTab(Url),
    OpenContext(u64),
    FocusContext(u64),
    CloseContext(u64),
    Dialog(String),
    /// The page scrolled itself (scripts, find, focus, keys).
    ScrollTo { x: f32, y: f32 },
    /// A wheel or key scroll the page did not consume.
    ScrollBy { dx: f32, dy: f32 },
    /// Go back (-1) or forward (1) in the tab's history (Backspace).
    History(isize),
    ContextMenu { link: Option<Url>, image: Option<Url>, has_selection: bool },
    /// Open a `<select>` popup: (label, selected, disabled) per option;
    /// `rect` in document coordinates.
    SelectDropdown { node: NodeId, options: Vec<(String, bool, bool)>, rect: Rect },
    Clipboard(String),
    Status(String),
    FindResult { count: usize, current: usize },
    Download(DownloadEvent),
    /// A finished network request of this tab.
    Network(rabbithole_engine::net::NetEvent),
    DomTree(Vec<DomRow>),
    NodeDetails(NodeDetails),
    /// A cookie-consent dialog was handled automatically (or found and
    /// left alone).
    Consent(crate::autocookies::ConsentEvent),
    /// The content thread died from a Rust panic (message and location);
    /// it no longer answers, the UI shows a crash page in a new one.
    Crashed(String),
}

/// Callback receiving a tab's messages: (tab id, epoch, message). Called
/// on content and loader threads.
pub type Sink = Arc<dyn Fn(u64, u64, FromContent) + Send + Sync>;

/// Everything a content thread needs to start.
#[derive(Clone)]
pub struct ContentConfig {
    pub window_context: Option<WindowBinding>,
    /// Template loader (shares caches and cookies with the UI's).
    pub loader: Loader,
    pub settings: ContentSettings,
    pub local_storage: Arc<Mutex<LocalStorage>>,
    pub next_download_id: Arc<AtomicU64>,
    pub view: ViewSize,
    /// Load the system fonts (false: bundled fonts only, for tests).
    pub system_fonts: bool,
    /// Echo console output to stderr.
    pub console_to_stderr: bool,
}

/// Messages a content thread receives: from the UI or its load workers.
pub(crate) enum Inbox {
    Ui(ToContent),
    Load { generation: u64, result: LoadResult },
}

/// Monotonic clock shared by the busy indicators.
fn clock() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

pub(crate) fn now_ms() -> u64 {
    clock().elapsed().as_millis() as u64 + 1
}

/// The UI side of a tab's content thread.
pub struct ContentHandle {
    #[allow(dead_code)]
    pub tab_id: u64,
    /// Distinguishes this thread's messages from those of a replaced one.
    pub epoch: u64,
    tx: Sender<Inbox>,
    /// When the thread started its current task (`now_ms`), 0 when idle.
    busy: Arc<AtomicU64>,
    window_context: Option<WindowBinding>,
}

impl ContentHandle {
    /// Start a content thread for a tab.
    pub fn spawn(tab_id: u64, epoch: u64, config: ContentConfig, sink: Sink) -> ContentHandle {
        let (tx, rx) = channel();
        let window_context = config.window_context.clone();
        if let Some(binding) = &window_context { binding.registry.attach(binding.id, tab_id, epoch, tx.clone()); }
        let failed_sink = sink.clone();
        let busy = Arc::new(AtomicU64::new(0));
        let worker_tx = tx.clone();
        let worker_busy = busy.clone();
        let _ = clock();
        let spawned = std::thread::Builder::new()
            .name(format!("rabbithole-tab-{tab_id}"))
            // Room for deep script recursion (~12k JS frames re-entering from
            // native code); the JS stack guard throws a RangeError before the
            // reservation runs out.
            .stack_size(256 * 1024 * 1024)
            .spawn(move || worker::run(tab_id, epoch, config, sink, rx, worker_tx, worker_busy));
        if let Err(e) = spawned {
            eprintln!("rabbithole: could not spawn content thread: {e}");
            if let Some(binding) = &window_context { binding.registry.renderer_gone(binding.id, tab_id, epoch); }
            failed_sink(tab_id, epoch, FromContent::Crashed(format!("Could not start content thread: {e}")));
        }
        ContentHandle { tab_id, epoch, tx, busy, window_context }
    }

    pub fn context_id(&self) -> Option<u64> { self.window_context.as_ref().map(|binding| binding.id) }

    pub fn send(&self, msg: ToContent) {
        let _ = self.tx.send(Inbox::Ui(msg));
    }

    /// How long the thread has been busy with its current task (None when
    /// it is waiting for work).
    pub fn busy_for(&self) -> Option<Duration> {
        let since = self.busy_since()?;
        Some(Duration::from_millis(now_ms().saturating_sub(since)))
    }

    /// When the current task started (an opaque timestamp identifying the
    /// busy period), None when idle.
    pub fn busy_since(&self) -> Option<u64> {
        let since = self.busy.load(Ordering::Relaxed);
        (since != 0).then_some(since)
    }
}

impl Drop for ContentHandle {
    fn drop(&mut self) {
        // The thread exits when it next looks at its inbox (a busy one
        // finishes its current task first; its messages are then ignored).
        if let Some(binding) = &self.window_context { binding.registry.detach(binding.id, self.tab_id, self.epoch); }
        let _ = self.tx.send(Inbox::Ui(ToContent::Shutdown));
    }
}

/// The computed style properties shown in the devtools Elements view.
pub(crate) fn style_summary(s: &rabbithole_engine::style::ComputedStyle, rect: Option<Rect>) -> Vec<(String, String)> {
    use rabbithole_engine::gfx::Color;
    let color = |c: Color| if c.a == 255 { format!("rgb({}, {}, {})", c.r, c.g, c.b) } else { format!("rgba({}, {}, {}, {:.2})", c.r, c.g, c.b, c.a as f32 / 255.0) };
    // `FlexStart` → `flex-start`
    let kw = |d: &dyn std::fmt::Debug| {
        let raw = format!("{d:?}");
        let mut out = String::new();
        for (i, ch) in raw.chars().enumerate() {
            if ch.is_ascii_uppercase() && i > 0 {
                out.push('-');
            }
            out.push(ch.to_ascii_lowercase());
        }
        out
    };
    // Px(10.0) → 10px, Percent(50.0) → 50%, Auto → auto
    let len = |l: &dyn std::fmt::Debug| {
        let raw = format!("{l:?}");
        if let Some(v) = raw.strip_prefix("Px(").and_then(|r| r.strip_suffix(')')) {
            format!("{}px", v.trim_end_matches(".0"))
        } else if let Some(v) = raw.strip_prefix("Percent(").and_then(|r| r.strip_suffix(')')) {
            format!("{}%", v.trim_end_matches(".0"))
        } else {
            raw.to_ascii_lowercase()
        }
    };
    let edges = |t: String, r: String, b: String, l: String| if t == r && r == b && b == l { t } else { format!("{t} {r} {b} {l}") };
    let px = |v: f32| format!("{}px", (v * 100.0).round() / 100.0);
    let mut out: Vec<(String, String)> = Vec::new();
    let mut push = |k: &str, v: String| out.push((k.to_string(), v));
    if let Some(r) = rect {
        push("box (x, y, w × h)", format!("{}, {}, {} × {}", px(r.x), px(r.y), px(r.w), px(r.h)));
    }
    push("display", kw(&s.display));
    push("position", kw(&s.position));
    push("box-sizing", kw(&s.box_sizing));
    push("width", len(&s.width));
    push("height", len(&s.height));
    push("margin", edges(len(&s.margin.top), len(&s.margin.right), len(&s.margin.bottom), len(&s.margin.left)));
    push("padding", edges(len(&s.padding.top), len(&s.padding.right), len(&s.padding.bottom), len(&s.padding.left)));
    push("border-width", edges(px(s.border_width.top), px(s.border_width.right), px(s.border_width.bottom), px(s.border_width.left)));
    push("border-color", color(s.border_color.top));
    push("border-radius", px(s.border_radius));
    push("color", color(s.color));
    push("background-color", color(s.background_color));
    if let Some(bg) = &s.background_image {
        push("background-image", bg.chars().take(80).collect());
    }
    push("font-family", kw(&s.font_family));
    push("font-size", px(s.font_size));
    push("font-weight", s.font_weight.to_string());
    push("line-height", s.line_height.map(px).unwrap_or_else(|| "normal".into()));
    push("text-align", kw(&s.text_align));
    push("white-space", kw(&s.white_space));
    push("opacity", format!("{}", s.opacity));
    push("visibility", kw(&s.visibility));
    push("overflow", format!("{} {}", kw(&s.overflow_x), kw(&s.overflow_y)));
    push("z-index", s.z_index.map(|z| z.to_string()).unwrap_or_else(|| "auto".into()));
    push("float", kw(&s.float));
    push("flex", format!("{} {} {}", s.flex_grow, s.flex_shrink, len(&s.flex_basis)));
    push("flex-direction", kw(&s.flex_direction));
    push("justify-content", kw(&s.justify_content));
    push("align-items", kw(&s.align_items));
    push("inset", edges(len(&s.inset.top), len(&s.inset.right), len(&s.inset.bottom), len(&s.inset.left)));
    out
}
