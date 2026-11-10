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

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rabbithole_engine::dom::NodeId;
use rabbithole_engine::gfx::{Image, Rect};
use rabbithole_engine::net::{Request, Url};
use rabbithole_engine::{LocalStorage, Loader, Modifiers, Viewport};

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentSettings {
    pub javascript: bool,
    pub images: bool,
    pub block_trackers: bool,
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
    /// Wheel over a scroll container (the UI scrolls the page itself).
    Wheel { x: f32, y: f32, dx: f32, dy: f32 },
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
    Shutdown,
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
    Dialog(String),
    /// The page scrolled itself (scripts, find, focus, keys).
    ScrollTo { x: f32, y: f32 },
    /// A wheel or key scroll the page did not consume.
    ScrollBy { dx: f32, dy: f32 },
    /// Go back (-1) or forward (1) in the tab's history (Backspace).
    History(isize),
    ContextMenu { link: Option<Url>, has_selection: bool },
    /// Open a `<select>` popup: (label, selected, disabled) per option;
    /// `rect` in document coordinates.
    SelectDropdown { node: NodeId, options: Vec<(String, bool, bool)>, rect: Rect },
    Clipboard(String),
    Status(String),
    FindResult { count: usize, current: usize },
    Download(DownloadEvent),
}

/// Callback receiving a tab's messages: (tab id, epoch, message). Called
/// on content and loader threads.
pub type Sink = Arc<dyn Fn(u64, u64, FromContent) + Send + Sync>;

/// Everything a content thread needs to start.
#[derive(Clone)]
pub struct ContentConfig {
    /// Template loader (shares caches and cookies with the UI's).
    pub loader: Loader,
    pub settings: ContentSettings,
    pub local_storage: Arc<Mutex<LocalStorage>>,
    pub download_dir: PathBuf,
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
}

impl ContentHandle {
    /// Start a content thread for a tab.
    pub fn spawn(tab_id: u64, epoch: u64, config: ContentConfig, sink: Sink) -> ContentHandle {
        let (tx, rx) = channel();
        let busy = Arc::new(AtomicU64::new(0));
        let worker_tx = tx.clone();
        let worker_busy = busy.clone();
        let _ = clock();
        let spawned = std::thread::Builder::new()
            .name(format!("rabbithole-tab-{tab_id}"))
            .stack_size(64 * 1024 * 1024)
            .spawn(move || worker::run(tab_id, epoch, config, sink, rx, worker_tx, worker_busy));
        if let Err(e) = spawned {
            eprintln!("rabbithole: could not spawn content thread: {e}");
        }
        ContentHandle { tab_id, epoch, tx, busy }
    }

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
        let _ = self.tx.send(Inbox::Ui(ToContent::Shutdown));
    }
}
