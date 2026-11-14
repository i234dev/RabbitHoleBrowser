//! Nested browsing contexts: `<iframe>` elements host child [`Page`]s with
//! their own document, styles, layout and script realm.
//!
//! * Child documents load on a background thread and are built (scripts
//!   included) on the owner's thread when they arrive ([`Engine::tick`]).
//! * A child page is laid out for the iframe's content box and painted
//!   into an image that the parent lays out and paints like any replaced
//!   element, so stacking, clipping and transforms of the parent apply.
//! * Pointer input over an iframe, wheel scrolling and key presses while
//!   it has focus are routed into the child page.
//! * Scripts see the frame tree through [`FrameRegistry`] (`parent`,
//!   `top`, `frames[i]`, `iframe.contentWindow`) and talk across realms
//!   with `postMessage`, which the engine routes after script activity.

use crate::scripting::{EngineEnv, ScriptOutcome};
use crate::{Engine, Loaded, Page, Viewport};
use rabbithole_dom::{Document, NodeId};
use rabbithole_gfx::{Canvas, Image, Rect};
use rabbithole_layout::LayoutBox;
use rabbithole_net::{Request, Url};
use rabbithole_script::EventInit;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;

/// Most frames one document hosts, and deepest nesting.
const MAX_FRAMES_PER_PAGE: usize = 16;
const MAX_DEPTH: u32 = 4;
/// Largest frame raster (device pixels per side).
const MAX_RASTER: f32 = 4096.0;

static NEXT_FRAME_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// Cross-frame messages or navigations are queued on this thread's
    /// engines (pages report a timer due now so they get delivered).
    static MAIL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

thread_local! {
    /// Frames whose realm a parent's script called into since the parent's
    /// last microtask checkpoint (see [`Engine::frame_microtasks`]).
    static TOUCHED: std::cell::RefCell<Vec<u64>> = const { std::cell::RefCell::new(Vec::new()) };
}

thread_local! {
    /// Reverse calls need the outer owning host restored before their target
    /// can run jobs. Keep this separate from the existing descendant queue,
    /// whose checkpoint still runs between consecutive classic scripts.
    static REVERSE_TOUCHED: std::cell::RefCell<Vec<u64>> = const { std::cell::RefCell::new(Vec::new()) };
}

thread_local! {
    /// Pages whose child frame is running scripts right now, outermost
    /// first. The running child is detached from its owner (see
    /// [`Engine::with_child`]), so the owner can be reached here without
    /// aliasing: `parent.document`, `top.document`, `frameElement`.
    static ANCESTORS: std::cell::RefCell<Vec<*mut Page>> = const { std::cell::RefCell::new(Vec::new()) };
}

thread_local! {
    // A temporarily detached page still has a browsing context. In particular,
    // a callback in its parent must not replace it with a new initial blank.
    static DETACHED: std::cell::RefCell<Vec<u64>> = const { std::cell::RefCell::new(Vec::new()) };
}

pub(crate) fn frame_detached(id: u64) -> bool {
    DETACHED.with(|frames| frames.borrow().contains(&id))
}

struct DetachedGuard(u64);
impl DetachedGuard {
    fn enter(id: u64) -> Self {
        DETACHED.with(|frames| frames.borrow_mut().push(id));
        Self(id)
    }
}
impl Drop for DetachedGuard {
    fn drop(&mut self) {
        DETACHED.with(|frames| {
            let mut frames = frames.borrow_mut();
            if let Some(index) = frames.iter().rposition(|id| *id == self.0) { frames.remove(index); }
        });
    }
}

/// Keeps a page on the ancestor stack while one of its child frames runs.
pub(crate) struct AncestorGuard;

impl AncestorGuard {
    pub(crate) fn push(page: &mut Page) -> AncestorGuard {
        ANCESTORS.with(|a| a.borrow_mut().push(page as *mut Page));
        AncestorGuard
    }
}

impl Drop for AncestorGuard {
    fn drop(&mut self) {
        ANCESTORS.with(|a| {
            a.borrow_mut().pop();
        });
    }
}

/// The ancestor pages of the running child, nearest first.
pub(crate) fn ancestors() -> Vec<*mut Page> {
    ANCESTORS.with(|a| a.borrow().iter().rev().copied().collect())
}

/// An ancestor page (or a frame below one, e.g. a sibling) with frame id
/// `id`.
///
/// # Safety
/// The pointers on the stack are live for the duration of the child's
/// script run: each owner is only reachable through this stack while its
/// child runs detached.
pub(crate) fn ancestor_page(id: u64) -> Option<&'static mut Page> {
    for p in ancestors() {
        // SAFETY: see above; the returned borrow is used within the
        // current host call and never stored.
        let page = unsafe { &mut *p };
        if page.frame_id == id {
            return Some(page);
        }
        if let Some(found) = find_frame_page_mut(page, id) {
            return Some(found);
        }
    }
    None
}

pub(crate) fn mail_pending() -> bool {
    MAIL.with(|m| m.get())
}

pub(crate) fn set_mail_pending() {
    MAIL.with(|m| m.set(true));
}

/// A fresh engine-wide frame id.
pub(crate) fn new_frame_id() -> u64 {
    NEXT_FRAME_ID.fetch_add(1, Ordering::Relaxed)
}

/// Pointer feedback for the embedder (see [`Page::hover_info`]).
#[derive(Debug, Clone, PartialEq)]
pub struct HoverInfo {
    /// `"default"`, `"pointer"` or `"text"`.
    pub cursor: &'static str,
    /// The link under the pointer (status bar).
    pub link: Option<Url>,
}

/// What an iframe displays.
#[derive(Debug, Clone, PartialEq)]
pub enum FrameSource {
    Blank,
    Url(Url),
    Srcdoc(String),
}

/// One `<iframe>` of a page and the document inside it.
pub struct ChildFrame {
    /// The iframe element in the owner's document.
    pub node: NodeId,
    /// Frame id (stable across navigations of the frame).
    pub id: u64,
    pub source: FrameSource,
    pub page: Option<Box<Page>>,
    pending: Option<Receiver<Result<Loaded, String>>>,
    /// The page shows a previous source; the current one must be loaded.
    stale: bool,
    /// Content box size (CSS px) the child is laid out for.
    pub size: (f32, f32),
    /// The child must be repainted into `image`.
    dirty: bool,
    image: Option<Arc<Image>>,
}

/// Child frames of a page.
#[derive(Default)]
pub struct Frames {
    pub children: Vec<ChildFrame>,
}

impl Frames {
    pub fn get(&self, node: NodeId) -> Option<&ChildFrame> {
        self.children.iter().find(|f| f.node == node)
    }
}

/// A message posted to another frame, waiting to be delivered.
#[derive(Debug, Clone)]
pub(crate) struct FrameMessage {
    pub from: u64,
    pub from_origin: String,
    pub to: u64,
    pub data: String,
    pub target_origin: String,
}

#[derive(Debug, Default, Clone)]
struct FrameEntry {
    parent: Option<u64>,
    children: Vec<u64>,
    url: String,
    /// `name` attribute of the hosting iframe (named frame access).
    name: Option<String>,
}

/// The frame tree of every page of an engine (what scripts can see of
/// other frames), plus queued cross-frame messages and navigations.
#[derive(Default)]
pub struct FrameRegistry {
    entries: HashMap<u64, FrameEntry>,
    pub(crate) outbox: Vec<FrameMessage>,
    pub(crate) navigations: Vec<(u64, Url)>,
}

impl FrameRegistry {
    pub fn parent(&self, id: u64) -> Option<u64> {
        self.entries.get(&id).and_then(|e| e.parent)
    }
    pub fn children(&self, id: u64) -> Vec<u64> {
        self.entries.get(&id).map(|e| e.children.clone()).unwrap_or_default()
    }
    pub fn exists(&self, id: u64) -> bool {
        self.entries.contains_key(&id)
    }
    pub fn url(&self, id: u64) -> Option<String> {
        self.entries.get(&id).map(|e| e.url.clone())
    }
    /// The `name` of the iframe hosting frame `id`.
    pub fn name(&self, id: u64) -> Option<String> {
        self.entries.get(&id)?.name.clone()
    }
    /// The child of frame `id` whose iframe is named `name`.
    pub fn named_child(&self, id: u64, name: &str) -> Option<u64> {
        self.entries.get(&id)?.children.iter().copied().find(|c| self.entries.get(c).and_then(|e| e.name.as_deref()) == Some(name))
    }
    fn remove_tree(&mut self, id: u64) {
        if let Some(e) = self.entries.remove(&id) {
            for c in e.children {
                self.remove_tree(c);
            }
        }
        self.outbox.retain(|m| m.to != id);
    }
}

/// Origin used for messages and `targetOrigin` checks. Documents without
/// a network origin (about:blank, srcdoc) inherit their parent's.
/// Is frame `id` `page` or one of its descendants?
fn find_frame_page(page: &Page, id: u64) -> bool {
    page.frame_id == id || page.frames.children.iter().any(|c| c.id == id || c.page.as_ref().map(|p| find_frame_page(p, id)).unwrap_or(false))
}

fn origin_of(page: &Page) -> String {
    page.frame_origin.clone().unwrap_or_else(|| page.url.origin())
}

pub(crate) fn page_origin(page: &Page) -> String {
    origin_of(page)
}

/// The page of frame `id` anywhere below `page`.
pub(crate) fn find_frame_page_mut(page: &mut Page, id: u64) -> Option<&mut Page> {
    for c in page.frames.children.iter_mut() {
        let Some(child) = c.page.as_mut() else { continue };
        if child.frame_id == id {
            c.dirty = true;
            return Some(child);
        }
        if let Some(p) = find_frame_page_mut(child, id) {
            c.dirty = true;
            return Some(p);
        }
    }
    None
}

/// Iframe size from its attributes (the default 300×150 otherwise): the
/// natural size of the frame raster, so layout sizes the box exactly as
/// it would without content.
fn attr_size(doc: &Document, node: NodeId) -> (f32, f32) {
    let parse = |v: Option<&str>| v.and_then(|s| s.trim().trim_end_matches("px").parse::<f32>().ok()).filter(|v| *v >= 0.0);
    (parse(doc.attr(node, "width")).unwrap_or(300.0), parse(doc.attr(node, "height")).unwrap_or(150.0))
}

fn find_box_mut(b: &mut LayoutBox, node: NodeId) -> Option<&mut LayoutBox> {
    if b.node == Some(node) && b.pseudo.is_none() {
        return Some(b);
    }
    b.children.iter_mut().find_map(|c| find_box_mut(c, node))
}

fn find_box(b: &LayoutBox, node: NodeId) -> Option<&LayoutBox> {
    if b.node == Some(node) && b.pseudo.is_none() {
        return Some(b);
    }
    b.children.iter().find_map(|c| find_box(c, node))
}

impl Engine {
    /// Run `f` on the page of child frame `i`, detached from `page` and
    /// with `page` on the ancestor stack (so scripts in the child can reach
    /// their parent's document). The child is put back afterwards.
    pub(crate) fn with_child<R>(&self, page: &mut Page, i: usize, f: impl FnOnce(&mut Page) -> R) -> Option<R> {
        let (id, mut child) = {
            let frame = page.frames.children.get_mut(i)?;
            (frame.id, frame.page.take()?)
        };
        let detached = DetachedGuard::enter(id);
        // Restore ownership even if host code panics, then preserve the panic.
        // The callback can remove/reorder iframe elements while the page is
        // detached, so the original vector index is not a context identity.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = AncestorGuard::push(page);
            f(&mut child)
        }));
        let retained = if let Some(frame) = page.frames.children.iter_mut().find(|frame| frame.id == id) {
            if frame.page.is_none() {
                frame.page = Some(child);
                true
            } else {
                false
            }
        } else {
            false
        };
        drop(detached);
        let r = match result {
            Ok(r) => r,
            Err(panic) => std::panic::resume_unwind(panic),
        };
        // The child's scripts may have changed this document.
        if page.script.dom_dirty && page.script.host.is_some() {
            page.script.dom_dirty = false;
            self.restyle(page);
        }
        retained.then_some(r)
    }

    /// Run `f` on the page of descendant frame `id` of `page` (detached
    /// like [`Engine::with_child`], every page between them on the
    /// ancestor stack).
    pub(crate) fn with_frame<R>(&self, page: &mut Page, id: u64, f: &mut dyn FnMut(&mut Page) -> R) -> Option<R> {
        if let Some(i) = page.frames.children.iter().position(|c| c.id == id) {
            return self.with_child(page, i, |c| f(c));
        }
        let i = page.frames.children.iter().position(|c| c.page.as_ref().map(|p| find_frame_page(p, id)).unwrap_or(false))?;
        self.with_child(page, i, |c| self.with_frame(c, id, f)).flatten()
    }

    /// Descendant requests retain their existing origin checks. The new
    /// ancestor/sibling route is deliberately restricted to inactive HTTP(S)
    /// documents with the same tuple origin, pending opaque identity support.
    pub(crate) fn call_in_frame(&self, page: &mut Page, id: u64, call: rabbithole_script::FrameCall) -> Option<rabbithole_script::FrameReply> {
        self.call_in_frame_with_deadline(page, id, call, None)
    }

    pub(crate) fn call_in_frame_with_deadline(&self, page: &mut Page, id: u64, call: rabbithole_script::FrameCall, deadline: Option<std::time::Instant>) -> Option<rabbithole_script::FrameReply> {
        let origin = page_origin(page);
        let source_http = matches!(page.url.scheme(), "http" | "https") && origin != "null" && page.url.origin() == origin;
        let reverse_allowed = source_http && matches!(&call, rabbithole_script::FrameCall::GetGlobal { .. } | rabbithole_script::FrameCall::CallGlobal { .. });
        let mut call = Some(call);
        let mut invoke = |target: &mut Page| {
            if page_origin(target) != origin { return None; }
            if target.script.host.is_none() && !target.script.scripts_ran && self.scripting {
                self.ensure_script_host(target);
            }
            let mut host = target.script.host.take()?;
            let reply = {
                let mut env = EngineEnv { engine: self, page: target };
                host.frame_call_with_deadline(&mut env, call.take()?, deadline)
            };
            target.script.host = Some(host);
            target.script.dom_dirty = true;
            Some(reply)
        };
        let descendant = find_frame_page(page, id);
        let r = if descendant {
            self.with_frame(page, id, &mut invoke).flatten()
        } else {
            // Only global reads/calls gain the reverse route. Do not broaden
            // foreign DOM methods, blank/data inheritance or frame limits.
            if !reverse_allowed {
                return None;
            }
            // `ancestors()` is nearest-first; choose the owning page that
            // actually contains the target. Sibling calls must detach it,
            // too: leaving an executing sibling inside an accessible parent
            // would allow parent traversal to alias its mutable Page.
            let owner = ancestors().into_iter().find(|p| {
                // Same lifetime invariant as `ancestor_page`; never stored.
                let p = unsafe { &**p };
                p.frame_id == id || find_frame_page(p, id)
            })?;
            let owner = unsafe { &mut *owner };
            let mut reverse = |target: &mut Page| {
                if !matches!(target.url.scheme(), "http" | "https") || target.url.origin() != origin || target.script.host.is_none() { return None; }
                invoke(target)
            };
            if owner.frame_id == id { reverse(owner) } else { self.with_frame(owner, id, &mut reverse).flatten() }
        };
        if r.is_some() {
            let remember = |t: &std::cell::RefCell<Vec<u64>>| {
                let mut t = t.borrow_mut();
                if !t.contains(&id) { t.push(id); }
            };
            if descendant { TOUCHED.with(remember); } else { REVERSE_TOUCHED.with(remember); }
        }
        r
    }

    /// Preserve descendant checkpoints between classic scripts. Reverse
    /// targets wait until the outer owner and suspended source hosts return.
    pub(crate) fn frame_microtasks(&self, page: &mut Page) {
        if !rabbithole_script::ScriptHost::frame_checkpoint_allowed() { return; }
        let mut drain = |target: &mut Page| {
            let Some(mut host) = target.script.host.take() else { return false };
            {
                let mut env = EngineEnv { engine: self, page: target };
                host.run_jobs(&mut env);
            }
            target.script.host = Some(host);
            target.script.dom_dirty = true;
            true
        };
        let ids = TOUCHED.with(|t| std::mem::take(&mut *t.borrow_mut()));
        for id in ids {
            self.with_frame(page, id, &mut drain);
        }
        if !ancestors().is_empty() || page.script.host.is_none() { return; }
        let ids = REVERSE_TOUCHED.with(|t| std::mem::take(&mut *t.borrow_mut()));
        for id in ids {
            let drained = if page.frame_id == id { drain(page) } else { self.with_frame(page, id, &mut drain).unwrap_or(false) };
            if !drained && self.frames.borrow().exists(id) {
                REVERSE_TOUCHED.with(|t| {
                    let mut t = t.borrow_mut();
                    if !t.contains(&id) { t.push(id); }
                });
            }
        }
        if REVERSE_TOUCHED.with(|t| !t.borrow().is_empty()) { set_mail_pending(); }
    }

    fn child_index(page: &Page, node: NodeId) -> Option<usize> {
        page.frames.children.iter().position(|c| c.node == node)
    }
}

impl Page {
    /// Content box (document coordinates) of the iframe hosting `frame`.
    pub fn frame_content_rect(&self, node: NodeId) -> Option<Rect> {
        find_box(&self.layout.root, node).map(|b| b.content_rect)
    }

    /// The child frame (iframe element, child page) at viewport point
    /// `(x, y)`, with the point in the child's viewport coordinates.
    fn frame_at_viewport(&self, x: f32, y: f32) -> Option<(NodeId, f32, f32)> {
        let node = self.node_at_viewport(x, y)?;
        let f = self.frames.get(node)?;
        f.page.as_ref()?;
        let r = self.frame_content_rect(node)?;
        let (dx, dy) = (x + self.page_scroll.0, y + self.page_scroll.1);
        Some((node, dx - r.x, dy - r.y))
    }

    /// Cursor and link under viewport point `(x, y)` for the embedder's
    /// pointer feedback, looking into the frame under the point (nested
    /// frames included).
    pub fn hover_info(&self, x: f32, y: f32) -> HoverInfo {
        if let Some((node, cx, cy)) = self.frame_at_viewport(x, y) {
            if let Some(child) = self.frames.get(node).and_then(|c| c.page.as_deref()) {
                return child.hover_info(cx - child.page_scroll.0, cy - child.page_scroll.1);
            }
        }
        let node = self.node_at_viewport(x, y);
        let link = node.and_then(|n| self.link_of(n));
        let mut cursor = match self.cursor_at(x + self.page_scroll.0, y + self.page_scroll.1) {
            "pointer" => "pointer",
            "text" => "text",
            _ => "default",
        };
        if link.is_some() {
            cursor = "pointer";
        }
        if let Some(c) = node.and_then(|n| self.control_of(n)) {
            if matches!(self.control_kind(c), crate::forms::ControlKind::Text | crate::forms::ControlKind::Password | crate::forms::ControlKind::TextArea) {
                cursor = "text";
            }
        }
        HoverInfo { cursor, link }
    }

    fn frame_mut(&mut self, node: NodeId) -> Option<&mut ChildFrame> {
        self.frames.children.iter_mut().find(|f| f.node == node)
    }

    /// Visit this page and every descendant frame page.
    pub fn for_each_frame_page(&self, f: &mut dyn FnMut(&Page)) {
        f(self);
        for c in &self.frames.children {
            if let Some(p) = &c.page {
                p.for_each_frame_page(f);
            }
        }
    }

    /// Any frame load still in progress (this page or below)?
    pub fn frames_loading(&self) -> bool {
        self.frames.children.iter().any(|c| c.pending.is_some() || c.page.as_ref().map(|p| p.script.frame_load_pending || p.script.frame_initial_load_pending || p.frames_loading()).unwrap_or(false))
    }

    pub(crate) fn frames_next_due(&self) -> Option<f64> {
        let mut due: Option<f64> = None;
        // Poll background loads every 25 ms (engine clock).
        let poll = self.image_state.clock.map(|origin| (std::time::Instant::now() + std::time::Duration::from_millis(25)).saturating_duration_since(origin).as_secs_f64() * 1000.0).unwrap_or(0.0);
        for c in &self.frames.children {
            let d = if c.pending.is_some() { Some(poll) } else { c.page.as_ref().and_then(|p| if p.script.frame_load_pending || p.script.frame_initial_load_pending { Some(0.0) } else { p.next_script_due() }) };
            if let Some(d) = d {
                due = Some(due.map(|x: f64| x.min(d)).unwrap_or(d));
            }
        }
        due
    }
}

impl Engine {
    /// Reconcile the page's child frames with its `<iframe>` elements and
    /// their laid-out sizes; start loads, resize and repaint children.
    /// Called after every layout of the page.
    /// Resize the viewport of child frame `id` to its iframe's current
    /// content box (the parent's layout must be up to date), so the
    /// frame's media queries see it before the next frame sync.
    pub(crate) fn update_frame_viewport(&self, page: &mut Page, id: u64) {
        let Some(i) = page.frames.children.iter().position(|c| c.id == id) else { return };
        let node = page.frames.children[i].node;
        let Some(r) = page.frame_content_rect(node) else { return };
        let s = (r.w.max(0.0), r.h.max(0.0));
        let c = &mut page.frames.children[i];
        if (s.0 - c.size.0).abs() <= 0.01 && (s.1 - c.size.1).abs() <= 0.01 {
            return;
        }
        c.size = s;
        c.dirty = true;
        if let Some(p) = c.page.as_mut() {
            let vp = Viewport { width: s.0.max(1.0), height: s.1.max(1.0) };
            if p.viewport != vp {
                p.viewport = vp;
                p.script.dom_dirty = true;
            }
        }
    }

    pub(crate) fn sync_frames(&self, page: &mut Page) {
        let iframes: Vec<NodeId> = if page.frame_depth >= MAX_DEPTH {
            Vec::new()
        } else {
            // With scripting on, `<noscript>` content is inert.
            page.doc
                .descendants(Document::ROOT)
                .filter(|&n| page.doc.is_element(n, "iframe") && !(self.scripting && crate::in_noscript(&page.doc, n)))
                .take(MAX_FRAMES_PER_PAGE)
                .collect()
        };
        if iframes.is_empty() && page.frames.children.is_empty() {
            return;
        }
        let mut reg = self.frames.borrow_mut();
        // Removed iframes drop their browsing context.
        let before = page.frames.children.len();
        page.frames.children.retain(|c| {
            let keep = iframes.contains(&c.node);
            if !keep {
                reg.remove_tree(c.id);
            }
            keep
        });
        let mut changed = before != page.frames.children.len();
        for &node in &iframes {
            let source = frame_source(page, node);
            match page.frames.children.iter_mut().find(|c| c.node == node) {
                Some(c) if c.source == source => {}
                Some(c) => {
                    // `src` changed: navigate the frame. The current
                    // document stays until the new one arrives (scripts
                    // keep seeing the initial about:blank document).
                    c.source = source;
                    c.pending = None;
                    c.stale = true;
                    if let Some(p) = c.page.as_mut() {
                        p.script.frame_load_pending = false;
                        p.script.frame_initial_load_pending = false;
                    }
                    changed = true;
                }
                None => {
                    page.frames.children.push(ChildFrame { node, id: new_frame_id(), source, page: None, pending: None, stale: false, size: attr_size(&page.doc, node), dirty: true, image: None });
                    changed = true;
                }
            }
        }
        if changed {
            // Document order.
            page.frames.children.sort_by_key(|c| iframes.iter().position(|&n| n == c.node));
        }
        let id = page.frame_id;
        let entry = reg.entries.entry(id).or_default();
        entry.url = page.url.to_string();
        entry.children = page.frames.children.iter().map(|c| c.id).collect();
        for c in &page.frames.children {
            let e = reg.entries.entry(c.id).or_default();
            e.parent = Some(id);
            e.name = page.doc.attr(c.node, "name").map(|s| s.to_string());
            if let Some(p) = &c.page {
                e.url = p.url.to_string();
            }
        }
        drop(reg);

        // Sizes from layout; start loads.
        let base_loader = {
            let mut l = self.loader.clone();
            l.defer_scripts = false;
            l.xml_documents = true;
            l.initiator = Some(page.url.clone());
            l
        };
        // A frame without a document and without `src` gets its initial
        // about:blank document now (not through a later load, which would
        // replace a document scripts already wrote into and fire a stray
        // `load`).
        let blank: Vec<NodeId> = page.frames.children.iter().filter(|c| c.page.is_none() && !frame_detached(c.id) && c.pending.is_none() && c.source == FrameSource::Blank).map(|c| c.node).collect();
        for node in blank {
            self.create_blank_frame(page, node);
        }
        for i in 0..page.frames.children.len() {
            let node = page.frames.children[i].node;
            let size = page.frame_content_rect(node).map(|r| (r.w.max(0.0), r.h.max(0.0)));
            let c = &mut page.frames.children[i];
            // Scripts of this document changed the frame's document.
            if let Some(p) = c.page.as_mut() {
                if p.script.dom_dirty || p.incr.fonts_changed(&p.fonts) {
                    p.script.dom_dirty = false;
                    self.restyle(p);
                    c.dirty = true;
                }
            }
            if let Some(s) = size {
                if (s.0 - c.size.0).abs() > 0.01 || (s.1 - c.size.1).abs() > 0.01 {
                    c.size = s;
                    c.dirty = true;
                    if let Some(p) = c.page.as_mut() {
                        let vp = Viewport { width: s.0.max(1.0), height: s.1.max(1.0) };
                        if p.viewport != vp {
                            p.viewport = vp;
                            self.restyle(p);
                        }
                    }
                }
            }
            if (c.stale || (c.page.is_none() && !frame_detached(c.id))) && c.pending.is_none() {
                c.stale = false;
                let vp = Viewport { width: c.size.0.max(1.0), height: c.size.1.max(1.0) };
                let loader = base_loader.clone();
                let (tx, rx) = std::sync::mpsc::channel();
                match c.source.clone() {
                    FrameSource::Blank => {
                        let url = Url::parse("about:blank").expect("about:blank");
                        let _ = tx.send(Ok(Loaded::Page(loader.load_html("<!doctype html><html><head></head><body></body></html>", url, vp))));
                    }
                    FrameSource::Srcdoc(html) => {
                        let url = Url::parse("about:srcdoc").unwrap_or_else(|| page.url.clone());
                        let base = page.base_url.clone();
                        std::thread::spawn(move || {
                            let mut l = loader.load_html(&html, url, vp);
                            l.base_url = base;
                            let _ = tx.send(Ok(Loaded::Page(l)));
                        });
                    }
                    FrameSource::Url(url) if url.scheme == "blob" => {
                        // Object URLs live in the creating realm's blob URL
                        // store (this page's or an ancestor's).
                        let key = url.to_string();
                        let mut resp = page.script.host.as_ref().and_then(|h| h.blob_url_response(&key));
                        if resp.is_none() {
                            for a in crate::frames::ancestors() {
                                // SAFETY: ancestor pages outlive this call (see AncestorGuard).
                                let a = unsafe { &*a };
                                resp = a.script.host.as_ref().and_then(|h| h.blob_url_response(&key));
                                if resp.is_some() {
                                    break;
                                }
                            }
                        }
                        let _ = tx.send(match resp {
                            Some(r) => Ok(Loaded::Page(loader.load_response(r, vp))),
                            None => Err("blob URL not found".to_string()),
                        });
                    }
                    FrameSource::Url(url) => {
                        let req = Request::get(url).with_referer(page.url.clone());
                        std::thread::spawn(move || {
                            let _ = tx.send(loader.load_request(&req, vp).map_err(|e| e.to_string()));
                        });
                    }
                }
                c.pending = Some(rx);
            }
        }
        self.paint_frames(page);
    }

    /// Give a connected `<iframe>` without a browsing context its initial
    /// `about:blank` document now (scripts can use `contentDocument` /
    /// `contentWindow` right after inserting it). A `src` still loads on
    /// the next sync.
    pub(crate) fn create_blank_frame(&self, page: &mut Page, node: NodeId) -> Option<u64> {
        let existing = page.frames.children.iter().position(|c| c.node == node);
        if let Some(index) = existing {
            let id = page.frames.children[index].id;
            if frame_detached(id) { return Some(id); }
        }
        if existing.is_none() && (!page.doc.is_element(node, "iframe") || !page.doc.is_connected(node) || page.frame_depth >= MAX_DEPTH || page.frames.children.len() >= MAX_FRAMES_PER_PAGE) {
            return None;
        }
        let id = existing.map(|i| page.frames.children[i].id).unwrap_or_else(new_frame_id);
        let size = page.frame_content_rect(node).map(|r| (r.w, r.h)).unwrap_or_else(|| attr_size(&page.doc, node));
        let vp = Viewport { width: size.0.max(1.0), height: size.1.max(1.0) };
        let loaded = self.loader.load_html("<!doctype html><html><head></head><body></body></html>", Url::parse("about:blank").expect("about:blank"), vp);
        let name = page.doc.attr(node, "name").map(|s| s.to_string());
        self.frame_context.replace(Some(FrameContext { id, depth: page.frame_depth + 1, name, origin: Some(origin_of(page)) }));
        let mut child = self.build_page_unscripted(loaded, vp);
        self.frame_context.replace(None);
        // Keep the initial document available synchronously and queue its
        // load. The no-src insertion steps can deliver it synchronously;
        // document.open() replaces it with the new parser's load instead.
        child.script.frame_initial_load_pending = frame_source(page, node) == FrameSource::Blank;
        {
            let mut reg = self.frames.borrow_mut();
            let e = reg.entries.entry(id).or_default();
            e.parent = Some(page.frame_id);
            e.url = child.url.to_string();
            e.name = page.doc.attr(node, "name").map(|s| s.to_string());
            let parent = reg.entries.entry(page.frame_id).or_default();
            if !parent.children.contains(&id) {
                parent.children.push(id);
            }
        }
        if let Some(i) = existing {
            // Its document is still loading: show about:blank meanwhile.
            page.frames.children[i].page = Some(Box::new(child));
            page.frames.children[i].dirty = true;
            return Some(id);
        }
        // Source Blank: a `src` attribute shows up as a change on the next
        // sync and starts the real load.
        page.frames.children.push(ChildFrame { node, id, source: FrameSource::Blank, page: Some(Box::new(child)), pending: None, stale: false, size, dirty: true, image: None });
        Some(id)
    }

    /// Restyle frame documents changed by this page's scripts (same-origin
    /// `contentDocument` access) and repaint them. Returns true when any
    /// frame changed.
    pub(crate) fn sync_dirty_frames(&self, page: &mut Page) -> bool {
        let mut any = false;
        for c in page.frames.children.iter_mut() {
            let Some(p) = c.page.as_mut() else { continue };
            let mut changed = self.sync_dirty_frames(p);
            if p.script.dom_dirty {
                p.script.dom_dirty = false;
                self.restyle(p);
                changed = true;
            }
            if changed {
                c.dirty = true;
                any = true;
            }
        }
        if any {
            self.paint_frames(page);
        }
        any
    }

    /// Repaint dirty child frames into their images and install them in
    /// the parent's layout and display list (no relayout needed: the
    /// raster's natural size is what the box was sized from).
    fn paint_frames(&self, page: &mut Page) -> bool {
        let mut need_display_list = false;
        let mut any = false;
        for i in 0..page.frames.children.len() {
            let c = &mut page.frames.children[i];
            if !c.dirty {
                continue;
            }
            let Some(child) = c.page.as_ref() else { continue };
            c.dirty = false;
            any = true;
            let natural = attr_size(&page.doc, c.node);
            let img = Arc::new(self.render_frame(child, c.size).with_natural_size(natural.0.round() as u32, natural.1.round() as u32));
            let old = c.image.replace(img.clone());
            let node = c.node;
            page.images.insert(node, img.clone());
            match find_box_mut(&mut page.layout.root, node) {
                Some(b) => {
                    let had = b.image.is_some();
                    b.image = Some(img.clone());
                    let mut patched = false;
                    // Patch the painted item in place only when the raster
                    // keeps its pixel size (its source rectangle is in raster
                    // pixels); a resized frame needs a new display list.
                    if let Some(old) = old.as_ref().filter(|o| o.width == img.width && o.height == img.height) {
                        for it in page.display_list.items.iter_mut() {
                            if let rabbithole_layout::DisplayItem::Image { image, .. } = it {
                                if Arc::ptr_eq(image, old) {
                                    *image = img.clone();
                                    patched = true;
                                }
                            }
                        }
                    }
                    if !had || !patched {
                        need_display_list = true;
                    }
                }
                None => {}
            }
        }
        if need_display_list {
            self.rebuild_display_list(page);
        }
        any
    }

    /// Paint a child page at its scroll position into an image of the
    /// frame's size (device pixels).
    fn render_frame(&self, child: &Page, size: (f32, f32)) -> Image {
        let scale = self.device_pixel_ratio.max(1.0);
        let pw = (size.0 * scale).ceil().clamp(1.0, MAX_RASTER) as u32;
        let ph = (size.1 * scale).ceil().clamp(1.0, MAX_RASTER) as u32;
        let mut canvas = Canvas::new(pw, ph);
        if child.background.a > 0 {
            canvas.fill_rect(Rect::new(0.0, 0.0, pw as f32, ph as f32), child.background);
        }
        let (sx, sy) = child.page_scroll;
        canvas.push_transform([scale, 0.0, 0.0, scale, 0.0, 0.0]);
        rabbithole_layout::paint(&child.display_list, &mut canvas, &child.fonts, sx, sy);
        canvas.pop_transform();
        Image::new(pw, ph, canvas.to_rgba8())
    }

    /// Frame work for one tick: build arrived documents, tick children,
    /// route messages (top-level page only), repaint changed frames.
    pub(crate) fn tick_frames(&self, page: &mut Page) -> ScriptOutcome {
        let mut out = ScriptOutcome::default();
        if page.frames.children.is_empty() && page.frame_depth > 0 {
            return out;
        }
        // Build at most one arrived document per tick (each runs its
        // scripts on this thread), so input is handled in between.
        let mut installed = false;
        // Scripts run while installing or ticking a frame can add or remove
        // frames: re-check the bound on every step.
        let mut i = 0;
        while i < page.frames.children.len() {
            let arrived = match page.frames.children[i].pending.as_ref().filter(|_| !installed).map(|rx| rx.try_recv()) {
                Some(Ok(r)) => Some(r),
                Some(Err(TryRecvError::Disconnected)) => Some(Err("load failed".to_string())),
                _ => None,
            };
            if let Some(result) = arrived {
                page.frames.children[i].pending = None;
                installed = true;
                let id = page.frames.children[i].id;
                self.install_frame(page, i, result, &mut out);
                let Some(current) = page.frames.children.iter().position(|c| c.id == id) else { continue };
                i = current;
            }
            let now = self.start.elapsed().as_secs_f64() * 1000.0 + 0.5;
            let run = page.frames.children[i].page.as_ref().map(|child| child.next_script_due().map(|d| d <= now).unwrap_or(false) || !child.frames.children.is_empty()).unwrap_or(false);
            if run {
                let id = page.frames.children[i].id;
                let result = self.with_child(page, i, |child| self.tick(child));
                let Some(current) = page.frames.children.iter().position(|c| c.id == id) else { continue };
                i = current;
                if let Some(o) = result {
                    let node = page.frames.children.get_mut(i).map(|c| {
                        if o.relayout {
                            c.dirty = true;
                        }
                        c.node
                    });
                    if let Some(node) = node {
                        self.frame_side_effects(page, node, o, &mut out);
                    }
                }
            }
            // The frame's document finished loading again (document.open /
            // close): the iframe fires `load` in this document.
            let reloaded = page.frames.children.get_mut(i).and_then(|c| c.page.as_mut()).map(|p| {
                let initial = std::mem::take(&mut p.script.frame_initial_load_pending);
                std::mem::take(&mut p.script.frame_load_pending) || initial
            }).unwrap_or(false);
            if reloaded && page.script.host.is_some() {
                let node = page.frames.children[i].node;
                page.frames.children[i].dirty = true;
                let o = self.dispatch_event(page, node, "load", EventInit::simple(false, false));
                out.merge(o);
                out.relayout = true;
            }
            i += 1;
        }
        if page.frame_depth == 0 {
            self.route_frame_messages(page, &mut out);
        }
        // Frame console output shows in the page's console, tagged with
        // the frame's URL.
        for c in page.frames.children.iter_mut() {
            if let Some(child) = c.page.as_mut() {
                if !child.script.console.is_empty() {
                    let url = child.url.to_string();
                    let short: String = url.chars().take(80).collect();
                    for (level, msg) in std::mem::take(&mut child.script.console) {
                        page.script.console.push((level, format!("[frame {short}] {msg}")));
                    }
                }
            }
        }
        if self.paint_frames(page) {
            out.relayout = true;
        }
        out
    }

    /// A child document arrived: build it (running its scripts) and fire
    /// `load` on the iframe.
    fn install_frame(&self, page: &mut Page, i: usize, result: Result<Loaded, String>, out: &mut ScriptOutcome) {
        let (node, id, size) = {
            let c = &page.frames.children[i];
            (c.node, c.id, c.size)
        };
        let vp = Viewport { width: size.0.max(1.0), height: size.1.max(1.0) };
        let loaded = match result {
            Ok(Loaded::Page(p)) => p,
            // Downloads and failures show an empty frame.
            _ => self.loader.load_html("<!doctype html><html><head></head><body></body></html>", Url::parse("about:blank").expect("about:blank"), vp),
        };
        let inherit_origin = matches!(loaded.url.scheme.as_str(), "about" | "data" | "javascript");
        let name = page.doc.attr(node, "name").map(|s| s.to_string());
        let ctx = FrameContext { id, depth: page.frame_depth + 1, name, origin: if inherit_origin { Some(origin_of(page)) } else { None } };
        self.frame_context.replace(Some(ctx));
        let mut child = self.build_page_unscripted(loaded, vp);
        self.frame_context.replace(None);
        if let Some(e) = self.frames.borrow_mut().entries.get_mut(&id) {
            e.url = child.url.to_string();
        }
        // The new document is executing under the same browsing-context ID.
        // Hide the previous document until commit rather than letting a
        // callback address its realm as if it were the running document.
        let old = page.frames.children[i].page.take();
        let detached = DetachedGuard::enter(id);
        let finished = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = AncestorGuard::push(page);
            self.finish_page_load(&mut child);
        }));
        let current = page.frames.children.iter().position(|c| c.id == id);
        if let Err(panic) = finished {
            if let Some(i) = current { page.frames.children[i].page = old; }
            drop(detached);
            std::panic::resume_unwind(panic);
        }
        let Some(i) = current else {
            self.frames.borrow_mut().remove_tree(id);
            return;
        };
        let nav = child.script.pending_navigation.take();
        page.frames.children[i].page = Some(Box::new(child));
        drop(detached);
        if let Some(old) = old {
            let mut reg = self.frames.borrow_mut();
            for g in &old.frames.children {
                reg.remove_tree(g.id);
            }
            drop(reg);
            // Node wrappers this realm made for the replaced document are
            // keyed by (frame, node id): they must not resolve to nodes of
            // the new document.
            if let Some(host) = page.script.host.as_mut() {
                host.forget_frame_document(id);
            }
        }
        page.frames.children[i].dirty = true;
        if let Some(nav) = nav {
            self.navigate_frame_node(page, node, nav.request.url);
        }
        // The iframe's `load` event in the parent.
        if page.script.host.is_some() {
            let o = self.dispatch_event(page, node, "load", EventInit::simple(false, false));
            out.merge(o);
        }
        out.relayout = true;
    }

    /// After a synthetic click inside child frame `i` (hosted by `node`):
    /// repaint the frame and apply its side effects.
    pub(crate) fn frame_click_done(&self, page: &mut Page, node: NodeId, _i: usize, o: ScriptOutcome, out: &mut ScriptOutcome) {
        if let Some(c) = page.frame_mut(node) {
            c.dirty = true;
        }
        self.frame_side_effects(page, node, o, out);
        if self.paint_frames(page) {
            out.relayout = true;
        }
    }

    /// Apply what a child frame's scripts asked for: navigations load in
    /// the frame, new windows and dialogs go to the embedder.
    fn frame_side_effects(&self, page: &mut Page, node: NodeId, o: ScriptOutcome, out: &mut ScriptOutcome) {
        out.new_windows.extend(o.new_windows);
        out.dialogs.extend(o.dialogs);
        if let Some(nav) = o.navigation {
            self.navigate_frame_node(page, node, nav.request.url);
        }
    }

    /// Load `url` in the frame hosted by `node`.
    pub(crate) fn navigate_frame_node(&self, page: &mut Page, node: NodeId, url: Url) {
        let Some(c) = page.frame_mut(node) else { return };
        if url.scheme == "javascript" {
            return;
        }
        // Keep the iframe's `src` as the author set it; the frame shows
        // the new document.
        let vp = Viewport { width: c.size.0.max(1.0), height: c.size.1.max(1.0) };
        let mut loader = self.loader.clone();
        loader.defer_scripts = false;
        loader.xml_documents = true;
        // The current document stays until the new one arrives.
        let referer = c.page.as_ref().map(|p| p.url.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut req = Request::get(url);
        if let Some(r) = referer {
            req = req.with_referer(r);
        }
        std::thread::spawn(move || {
            let _ = tx.send(loader.load_request(&req, vp).map_err(|e| e.to_string()));
        });
        c.pending = Some(rx);
    }

    /// Deliver queued `postMessage`s and frame navigations anywhere in the
    /// tree below (and including) the top-level `page`.
    fn route_frame_messages(&self, page: &mut Page, out: &mut ScriptOutcome) {
        for _ in 0..8 {
            let (msgs, navs) = {
                let mut reg = self.frames.borrow_mut();
                MAIL.with(|m| m.set(false));
                (std::mem::take(&mut reg.outbox), std::mem::take(&mut reg.navigations))
            };
            if msgs.is_empty() && navs.is_empty() {
                break;
            }
            for (id, url) in navs {
                navigate_in_tree(self, page, id, url);
            }
            for m in msgs {
                if let Some(o) = deliver_in_tree(self, page, &m) {
                    out.merge(o);
                }
            }
        }
    }

    // ------------------------------------------------------------ input

    /// Route a pointer move into the frame under the point (and out of
    /// the one the pointer left). Returns true when the point is over a
    /// frame (the child handled it).
    pub(crate) fn frame_pointer_move(&self, page: &mut Page, x: f32, y: f32, m: crate::Modifiers, out: &mut ScriptOutcome) -> bool {
        let hit = page.frame_at_viewport(x, y);
        let prev = page.pointer.frame_hover;
        if prev.is_some() && prev != hit.map(|h| h.0) {
            if let Some(i) = prev.and_then(|n| Self::child_index(page, n)) {
                if let Some(o) = self.with_child(page, i, |child| self.pointer_move(child, -1.0, -1.0, m)) {
                    if o.relayout {
                        if let Some(c) = page.frames.children.iter_mut().find(|c| Some(c.node) == prev) { c.dirty = true; }
                    }
                }
            }
        }
        page.pointer.frame_hover = hit.map(|h| h.0);
        let Some((node, cx, cy)) = hit else { return false };
        if let Some(i) = Self::child_index(page, node) {
            if let Some(o) = self.with_child(page, i, |child| self.pointer_move(child, cx, cy, m)) {
                if o.relayout {
                    if let Some(c) = page.frame_mut(node) { c.dirty = true; }
                }
                self.frame_side_effects(page, node, o, out);
            }
        }
        if self.paint_frames(page) {
            out.relayout = true;
        }
        true
    }

    /// A press over a frame: focus the iframe and press inside the child.
    pub(crate) fn frame_pointer_down(&self, page: &mut Page, x: f32, y: f32, button: u8, m: crate::Modifiers) -> Option<crate::pointer::PointerDown> {
        let (node, cx, cy) = page.frame_at_viewport(x, y)?;
        let mut out = ScriptOutcome::default();
        page.pointer.pointer_modality = true;
        if button == crate::pointer::BUTTON_PRIMARY {
            out.merge(self.focus_element(page, Some(node)));
        }
        page.pointer.frame_press = Some(node);
        page.pointer.down = None;
        let i = Self::child_index(page, node)?;
        let mut d = self.with_child(page, i, |child| self.pointer_down(child, cx, cy, button, m))?;
        let o = std::mem::take(&mut d.outcome);
        page.frame_mut(node)?.dirty = true;
        self.frame_side_effects(page, node, o, &mut out);
        if self.paint_frames(page) {
            out.relayout = true;
        }
        Some(crate::pointer::PointerDown { target: Some(node), prevented: true, outcome: out })
    }

    /// Release after a press that started in a frame: released in the
    /// child (activation behaviour runs there).
    pub(crate) fn frame_pointer_up(&self, page: &mut Page, x: f32, y: f32, button: u8, m: crate::Modifiers) -> Option<crate::pointer::PointerUp> {
        let node = page.pointer.frame_press.take()?;
        let r = page.frame_content_rect(node)?;
        let (cx, cy) = (x + page.page_scroll.0 - r.x, y + page.page_scroll.1 - r.y);
        let mut out = ScriptOutcome::default();
        let i = Self::child_index(page, node)?;
        let mut up = self.with_child(page, i, |child| self.pointer_up(child, cx, cy, button, m))?;
        let o = std::mem::take(&mut up.outcome);
        page.frame_mut(node)?.dirty = true;
        let action = match up.action {
            crate::ClickAction::Navigate { url, new_tab: false } => {
                self.navigate_frame_node(page, node, url);
                crate::ClickAction::Changed
            }
            crate::ClickAction::Submit { form, submitter } => {
                let req = page.frame_mut(node).and_then(|c| c.page.as_ref()).and_then(|p| p.submit_request(form, submitter, &|_| Vec::new()));
                if let Some(req) = req {
                    self.load_frame_request(page, node, req);
                }
                crate::ClickAction::Changed
            }
            a @ crate::ClickAction::Navigate { .. } => a,
            _ => crate::ClickAction::Changed,
        };
        self.frame_side_effects(page, node, o, &mut out);
        if self.paint_frames(page) {
            out.relayout = true;
        }
        Some(crate::pointer::PointerUp { target: Some(node), clicked: None, action, outcome: out })
    }

    fn load_frame_request(&self, page: &mut Page, node: NodeId, req: Request) {
        let Some(c) = page.frame_mut(node) else { return };
        let vp = Viewport { width: c.size.0.max(1.0), height: c.size.1.max(1.0) };
        let mut loader = self.loader.clone();
        loader.defer_scripts = false;
        loader.xml_documents = true;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(loader.load_request(&req, vp).map_err(|e| e.to_string()));
        });
        c.pending = Some(rx);
    }

    /// Wheel scrolling over a frame (document point of the owner):
    /// scroll the child's inner scroll container or the child page.
    pub(crate) fn frame_scroll_at(&self, page: &mut Page, x: f32, y: f32, dx: f32, dy: f32) -> Option<bool> {
        let (vx, vy) = (x - page.page_scroll.0, y - page.page_scroll.1);
        let (node, cx, cy) = page.frame_at_viewport(vx, vy)?;
        let c = page.frame_mut(node)?;
        let child = c.page.as_mut()?;
        let (sx, sy) = child.page_scroll;
        let scrolled = if self.scroll_at(child, cx + sx, cy + sy, dx, dy) {
            true
        } else {
            let max_x = (child.content_width() - child.viewport.width).max(0.0);
            let max_y = (child.content_height() - child.viewport.height).max(0.0);
            let nx = (sx + dx).clamp(0.0, max_x);
            let ny = (sy + dy).clamp(0.0, max_y);
            if (nx, ny) != (sx, sy) {
                self.set_page_scroll(child, nx, ny);
                true
            } else {
                false
            }
        };
        if scrolled {
            c.dirty = true;
            self.paint_frames(page);
        }
        Some(scrolled)
    }

    /// A key press while an iframe has focus: `keydown`/`keyup` go to the
    /// child's focused element (or body), and printable keys / Backspace
    /// edit a focused text control there. None when no frame has focus.
    pub fn frame_key_event(&self, page: &mut Page, key: &str, code: &str, pressed: bool, m: crate::Modifiers) -> Option<ScriptOutcome> {
        let node = page.state.focused?;
        let i = Self::child_index(page, node)?;
        page.frames.children[i].page.as_ref()?;
        let mut o = self.with_child(page, i, |child| match self.frame_key_event(child, key, code, pressed, m) {
            Some(o) => o,
            None => key_into_page(self, child, key, code, pressed, m),
        })?;
        if o.relayout {
            page.frame_mut(node)?.dirty = true;
        }
        let mut out = ScriptOutcome { default_prevented: o.default_prevented, ..Default::default() };
        let nav = o.navigation.take();
        out.new_windows = std::mem::take(&mut o.new_windows);
        out.dialogs = std::mem::take(&mut o.dialogs);
        if let Some(nav) = nav {
            self.navigate_frame_node(page, node, nav.request.url);
        }
        c_dirty_paint(self, page);
        out.relayout = true;
        Some(out)
    }
}

fn c_dirty_paint(engine: &Engine, page: &mut Page) {
    engine.paint_frames(page);
}

impl Engine {
    /// Keyboard input for a page without an embedder's editing machinery
    /// (frames, WebDriver-style test automation): `keydown`/`keyup` to the
    /// focused element and simple text editing.
    pub fn key_input(&self, page: &mut Page, key: &str, code: &str, pressed: bool, m: crate::Modifiers) -> ScriptOutcome {
        key_into_page(self, page, key, code, pressed, m)
    }
}

/// Keyboard input for a page without the embedder's editing machinery
/// (a frame): key events to the focused element, simple text editing.
fn key_into_page(engine: &Engine, page: &mut Page, key: &str, code: &str, pressed: bool, m: crate::Modifiers) -> ScriptOutcome {
    let target = page.state.focused.or_else(|| page.doc.body());
    let Some(target) = target else { return ScriptOutcome::default() };
    let init = crate::scripting::key_init(key, code, m.ctrl, m.shift, m.alt, m.meta, false);
    let mut out = engine.dispatch_event(page, target, if pressed { "keydown" } else { "keyup" }, init);
    if !pressed || out.default_prevented || m.ctrl || m.alt || m.meta {
        return out;
    }
    let Some(focused) = page.state.focused else { return out };
    if !crate::pointer::is_text_entry(&page.doc, focused) || page.doc.attr(focused, "contenteditable").is_some() {
        if key == "Enter" {
            if let Some(form) = page.form_of(focused) {
                let o = engine.dispatch_event(page, form, "submit", crate::scripting::plain_init(true));
                out.merge(o);
            }
        }
        return out;
    }
    let mut text = page.control_text(focused);
    let data = match key {
        "Backspace" => {
            text.pop();
            None
        }
        "Enter" if page.doc.is_element(focused, "textarea") => {
            text.push('\n');
            Some("\n".to_string())
        }
        k if k.chars().count() == 1 => {
            text.push_str(k);
            Some(k.to_string())
        }
        _ => return out,
    };
    page.set_control_text(focused, &text);
    engine.restyle(page);
    out.relayout = true;
    let o = engine.dispatch_event(page, focused, "input", crate::scripting::input_init(data.as_deref()));
    out.merge(o);
    out
}

/// Context for the page being built as a frame (consumed by
/// [`Engine::build_page_unscripted`]).
#[derive(Debug, Clone)]
pub(crate) struct FrameContext {
    pub id: u64,
    pub depth: u32,
    pub name: Option<String>,
    pub origin: Option<String>,
}

fn frame_source(page: &Page, node: NodeId) -> FrameSource {
    if let Some(s) = page.doc.attr(node, "srcdoc") {
        return FrameSource::Srcdoc(s.to_string());
    }
    let src = page.doc.attr(node, "src").map(str::trim).unwrap_or("");
    if src.is_empty() || src.eq_ignore_ascii_case("about:blank") {
        return FrameSource::Blank;
    }
    if src.len() >= 11 && src[..11].eq_ignore_ascii_case("javascript:") {
        return FrameSource::Blank;
    }
    match page.resolve(src) {
        Some(u) if matches!(u.scheme.as_str(), "http" | "https" | "data" | "file" | "about" | "blob") => FrameSource::Url(u),
        _ => FrameSource::Blank,
    }
}

fn target_origin_ok(target: &Page, m: &FrameMessage) -> bool {
    match m.target_origin.trim() {
        "*" => true,
        "/" => origin_of(target) == m.from_origin,
        o => match Url::parse(o) {
            Some(u) => u.origin() == origin_of(target),
            None => false,
        },
    }
}

/// Deliver `m` to the page with frame id `m.to` in the tree under `page`;
/// marks the frames on the way dirty when the target changed.
fn deliver_in_tree(engine: &Engine, page: &mut Page, m: &FrameMessage) -> Option<ScriptOutcome> {
    if page.frame_id == m.to {
        if !target_origin_ok(page, m) || page.script.host.is_none() {
            return Some(ScriptOutcome::default());
        }
        let mut host = page.script.host.take()?;
        {
            let mut env = EngineEnv { engine, page };
            host.realm().begin_task();
            host.deliver_message(&mut env, &m.data, &m.from_origin, m.from);
            host.run_jobs(&mut env);
            host.realm().set_deadline(None);
        }
        page.script.host = Some(host);
        return Some(engine.settle(page, false));
    }
    let ids: Vec<_> = page.frames.children.iter().map(|c| c.id).collect();
    for id in ids {
        let Some(i) = page.frames.children.iter().position(|c| c.id == id) else { continue };
        let node = page.frames.children[i].node;
        if page.frames.children[i].page.is_none() {
            continue;
        }
        let r = engine.with_child(page, i, |child| deliver_in_tree(engine, child, m)).flatten();
        let Some(i) = page.frames.children.iter().position(|c| c.id == id) else { continue };
        if let Some(o) = &r {
            if o.relayout {
                page.frames.children[i].dirty = true;
            }
        }
        if let Some(mut o) = r {
            let nav = o.navigation.take();
            if let Some(nav) = nav {
                if page.frames.children[i].page.as_ref().map(|p| p.frame_id == m.to).unwrap_or(false) {
                    engine.navigate_frame_node(page, node, nav.request.url);
                }
            }
            engine.paint_frames(page);
            o.relayout = true;
            return Some(o);
        }
    }
    None
}

fn navigate_in_tree(engine: &Engine, page: &mut Page, id: u64, url: Url) -> bool {
    if let Some(node) = page.frames.children.iter().find(|c| c.id == id).map(|c| c.node) {
        engine.navigate_frame_node(page, node, url);
        return true;
    }
    for c in page.frames.children.iter_mut() {
        if let Some(child) = c.page.as_mut() {
            if navigate_in_tree(engine, child, id, url.clone()) {
                return true;
            }
        }
    }
    false
}

impl EngineEnv<'_> {
    pub(crate) fn queue_message(&mut self, target: u64, data: String, target_origin: String) {
        let from_origin = origin_of(self.page);
        self.engine.frames.borrow_mut().outbox.push(FrameMessage { from: self.page.frame_id, from_origin, to: target, data, target_origin });
        set_mail_pending();
    }
}

#[cfg(test)]
mod ancestor_call_tests {
    use super::*;
    use rabbithole_script::{FrameCall, FrameReply};
    use std::time::{Duration, Instant};

    fn fixture() -> (Engine, Page) {
        let mut engine = Engine::new_bundled_fonts_only();
        engine.scripting = true;
        engine.loader.load_scripts = true;
        let page = engine.load_html(
            "<iframe></iframe><iframe></iframe><script>var marker='root'; var trace=[]; function value(){trace.push('sync'); Promise.resolve().then(()=>trace.push('promise'));return 42}</script>",
            Url::parse("https://frame.test/root").unwrap(),
            Viewport { width: 800.0, height: 600.0 },
        );
        assert_eq!(page.frames.children.len(), 2);
        (engine, page)
    }

    // Real Engine/ScriptHost realms, constructed from in-memory HTML at HTTP
    // URLs. This avoids network timing while preserving the real frame IDs,
    // registry, host calls and ownership machinery.
    fn replace_child(engine: &mut Engine, page: &mut Page, index: usize, url: &str, html: &str) {
        let id = page.frames.children[index].id;
        engine.frame_context.replace(Some(FrameContext { id, depth: 1, name: None, origin: None }));
        let loaded = engine.loader.load_html(html, Url::parse(url).unwrap(), page.viewport);
        let child = engine.build_page_unscripted(loaded, page.viewport);
        engine.frame_context.replace(None);
        page.frames.children[index].page = Some(Box::new(child));
        engine.frames.borrow_mut().entries.get_mut(&id).unwrap().url = url.into();
    }

    fn eval(engine: &Engine, page: &mut Page, source: &str) -> String {
        engine.eval_in_page(page, source).trim_matches(|c| c == '\'' || c == '"').to_owned()
    }

    fn finish(engine: &Engine, page: &mut Page, index: usize) {
        engine.with_child(page, index, |child| engine.finish_page_load(child)).unwrap();
        engine.frame_microtasks(page);
    }

    #[test]
    fn descendant_jobs_still_run_between_consecutive_parent_classic_scripts() {
        let mut engine = Engine::new_bundled_fonts_only();
        engine.scripting = true;
        engine.loader.load_scripts = true;
        let viewport = Viewport { width: 800.0, height: 600.0 };
        let loaded = engine.loader.load_html("<iframe></iframe><script>frames[0].queue()</script><script>var observed=frames[0].flag</script>", Url::parse("https://frame.test/root").unwrap(), viewport);
        let mut page = engine.build_page_unscripted(loaded, viewport);
        replace_child(&mut engine, &mut page, 0, "https://frame.test/child", "<script>var flag=0;function queue(){Promise.resolve().then(()=>flag=1)}</script>");
        finish(&engine, &mut page, 0);
        engine.finish_page_load(&mut page);
        assert_eq!(eval(&engine, &mut page, "observed"), "1");
    }

    #[test]
    fn inactive_http_ancestor_globals_and_calls_defer_ancestor_jobs_until_source_returns() {
        let (mut engine, mut page) = fixture();
        replace_child(&mut engine, &mut page, 0, "https://frame.test/child", "<script>var report=parent.marker+':'+parent.value()+':'+parent.trace.join(','); Promise.resolve().then(()=>{report+=':'+parent.trace.join(',')})</script>");
        finish(&engine, &mut page, 0);
        // The parent Promise must not run inside the suspended child stack,
        // nor inside that child's checkpoint while the owner is detached.
        assert_eq!(eval(&engine, &mut page, "frames[0].report"), "root:42:sync:sync");
        assert_eq!(eval(&engine, &mut page, "trace.join(',')"), "sync,promise");
    }

    #[test]
    fn active_ancestor_stays_unavailable_instead_of_aliasing_its_script_host() {
        let (mut engine, mut page) = fixture();
        replace_child(&mut engine, &mut page, 0, "https://frame.test/child", "<script>function readParent(){return typeof parent.marker}</script>");
        finish(&engine, &mut page, 0);
        assert_eq!(eval(&engine, &mut page, "frames[0].readParent()"), "undefined");
        assert_eq!(eval(&engine, &mut page, "marker"), "root");
    }

    #[test]
    fn reverse_route_rejects_cross_origin_without_running_the_getter() {
        let (mut engine, mut page) = fixture();
        eval(&engine, &mut page, "var reads=0;Object.defineProperty(window,'secret',{get(){reads++;return 1}})");
        replace_child(&mut engine, &mut page, 0, "https://other.test/child", "<script>var report=typeof parent.secret</script>");
        finish(&engine, &mut page, 0);
        assert_eq!(eval(&engine, &mut page, "reads"), "0");
    }

    #[test]
    fn sibling_call_detaches_and_restores_the_target_context() {
        let (mut engine, mut page) = fixture();
        replace_child(&mut engine, &mut page, 0, "https://frame.test/source", "");
        replace_child(&mut engine, &mut page, 1, "https://frame.test/target", "<script>var target='sibling'</script>");
        finish(&engine, &mut page, 0);
        finish(&engine, &mut page, 1);
        let id = page.frames.children[1].id;
        let result = engine.with_child(&mut page, 0, |source| engine.call_in_frame_with_deadline(source, id, FrameCall::GetGlobal { name: "target".into() }, Some(Instant::now()+Duration::from_secs(1)))).flatten();
        assert!(matches!(result, Some(FrameReply::Value(_))));
        assert_eq!(page.frames.children[1].page.as_ref().unwrap().frame_id, id);
        assert!(!frame_detached(id));
    }

    #[test]
    fn detached_context_is_restored_by_id_after_sibling_reordering() {
        let (engine, mut page) = fixture();
        let id = page.frames.children[0].id;
        let owner = page.frame_id;
        let child_id = engine.with_child(&mut page, 0, |child| {
            let parent = ancestor_page(owner).unwrap();
            let node = parent.frames.children[0].node;
            assert!(frame_detached(id));
            assert_eq!(engine.create_blank_frame(parent, node), Some(id));
            assert!(parent.frames.children[0].page.is_none());
            parent.frames.children.swap(0, 1);
            child.frame_id
        });
        assert_eq!(child_id, Some(id));
        assert_eq!(page.frames.children[1].page.as_ref().unwrap().frame_id, id);
        assert_ne!(page.frames.children[0].page.as_ref().unwrap().frame_id, id);
        assert!(!frame_detached(id));
    }

    #[test]
    fn removing_a_detached_context_does_not_replace_its_sibling() {
        let (engine, mut page) = fixture();
        let removed = page.frames.children[0].id;
        let sibling = page.frames.children[1].id;
        let owner = page.frame_id;
        let result = engine.with_child(&mut page, 0, |_| {
            ancestor_page(owner).unwrap().frames.children.remove(0);
            7
        });
        assert_eq!(result, None);
        assert_eq!(page.frames.children.len(), 1);
        assert_eq!(page.frames.children[0].page.as_ref().unwrap().frame_id, sibling);
        assert!(!frame_detached(removed));
    }

    #[test]
    fn real_ancestor_callback_can_remove_its_calling_frame_without_overwriting_a_sibling() {
        let (mut engine, mut page) = fixture();
        eval(&engine, &mut page, "function removeSource(){document.querySelector('iframe').remove();return document.body.offsetWidth}");
        let sibling = page.frames.children[1].id;
        replace_child(&mut engine, &mut page, 0, "https://frame.test/source", "<script>parent.removeSource()</script>");
        engine.with_child(&mut page, 0, |child| engine.finish_page_load(child));
        engine.settle(&mut page, false);
        assert_eq!(page.frames.children.len(), 1);
        assert_eq!(page.frames.children[0].id, sibling);
        assert_eq!(page.frames.children[0].page.as_ref().unwrap().frame_id, sibling);
    }

    #[test]
    fn expired_idle_target_deadline_does_not_override_the_live_caller_budget() {
        let (mut engine, mut page) = fixture();
        replace_child(&mut engine, &mut page, 0, "https://frame.test/source", "");
        finish(&engine, &mut page, 0);
        let id = page.frame_id;
        let expired = Instant::now() - Duration::from_secs(1);
        page.script.host.as_mut().unwrap().realm().set_deadline(Some(expired));
        let result = engine.with_child(&mut page, 0, |source| engine.call_in_frame_with_deadline(source, id, FrameCall::GetGlobal { name: "marker".into() }, Some(Instant::now()+Duration::from_secs(1)))).flatten();
        assert!(matches!(result, Some(FrameReply::Value(ref value)) if value == "\"root\""), "{result:?}");
        assert_eq!(page.script.host.as_mut().unwrap().realm().deadline, Some(expired));
        page.script.host.as_mut().unwrap().realm().set_deadline(None);
    }

    #[test]
    fn ancestor_getter_and_call_share_absolute_deadline_and_recover() {
        for getter in [true, false] {
            let (mut engine, mut page) = fixture();
            eval(&engine, &mut page, "Object.defineProperty(window,'loopGetter',{get(){for(;;){}}});function loopCall(){for(;;){}}");
            replace_child(&mut engine, &mut page, 0, "https://frame.test/source", "");
            finish(&engine, &mut page, 0);
            let id = page.frame_id;
            let call = if getter { FrameCall::GetGlobal { name: "loopGetter".into() } } else { FrameCall::CallGlobal { name: "loopCall".into(), args: r#"{"t":"a","id":0,"v":[],"l":0}"#.into() } };
            let old_budget = page.script.host.as_mut().unwrap().realm().task_budget;
            let result = engine.with_child(&mut page, 0, |source| engine.call_in_frame_with_deadline(source, id, call, Some(Instant::now()+Duration::from_millis(20)))).flatten();
            assert!(matches!(result, Some(FrameReply::Error(ref message)) if message.contains("timed out")), "{result:?}");
            let host = page.script.host.as_mut().unwrap();
            assert!(host.realm().deadline.is_none());
            assert_eq!(host.realm().task_budget, old_budget);
            assert_eq!(eval(&engine, &mut page, "marker+':alive'"), "root:alive");
        }
    }
}
