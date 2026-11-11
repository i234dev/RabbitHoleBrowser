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
fn origin_of(page: &Page) -> String {
    page.frame_origin.clone().unwrap_or_else(|| page.url.origin())
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
        self.frames.children.iter().any(|c| c.pending.is_some() || c.page.as_ref().map(|p| p.frames_loading()).unwrap_or(false))
    }

    pub(crate) fn frames_next_due(&self) -> Option<f64> {
        let mut due: Option<f64> = None;
        // Poll background loads every 25 ms (engine clock).
        let poll = self.image_state.clock.map(|origin| (std::time::Instant::now() + std::time::Duration::from_millis(25)).saturating_duration_since(origin).as_secs_f64() * 1000.0).unwrap_or(0.0);
        for c in &self.frames.children {
            let d = if c.pending.is_some() { Some(poll) } else { c.page.as_ref().and_then(|p| p.next_script_due()) };
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
                    // `src` changed: navigate the frame.
                    c.source = source;
                    c.pending = None;
                    if let Some(old) = c.page.take() {
                        for g in &old.frames.children {
                            reg.remove_tree(g.id);
                        }
                    }
                    changed = true;
                }
                None => {
                    page.frames.children.push(ChildFrame { node, id: new_frame_id(), source, page: None, pending: None, size: attr_size(&page.doc, node), dirty: true, image: None });
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
            if let Some(p) = &c.page {
                e.url = p.url.to_string();
            }
        }
        drop(reg);

        // Sizes from layout; start loads.
        let base_loader = {
            let mut l = self.loader.clone();
            l.defer_scripts = false;
            l.initiator = Some(page.url.clone());
            l
        };
        for i in 0..page.frames.children.len() {
            let node = page.frames.children[i].node;
            let size = page.frame_content_rect(node).map(|r| (r.w.max(0.0), r.h.max(0.0)));
            let c = &mut page.frames.children[i];
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
            if c.page.is_none() && c.pending.is_none() {
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
                    if let Some(old) = &old {
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
        rabbithole_layout::paint(&child.display_list, &mut canvas, &self.fonts, sx, sy);
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
        for i in 0..page.frames.children.len() {
            let arrived = match page.frames.children[i].pending.as_ref().filter(|_| !installed).map(|rx| rx.try_recv()) {
                Some(Ok(r)) => Some(r),
                Some(Err(TryRecvError::Disconnected)) => Some(Err("load failed".to_string())),
                _ => None,
            };
            if let Some(result) = arrived {
                page.frames.children[i].pending = None;
                installed = true;
                self.install_frame(page, i, result, &mut out);
            }
            let c = &mut page.frames.children[i];
            if let Some(child) = c.page.as_mut() {
                let due = child.next_script_due().map(|d| d <= self.start.elapsed().as_secs_f64() * 1000.0 + 0.5).unwrap_or(false);
                if due || !child.frames.children.is_empty() {
                    let o = self.tick(child);
                    if o.relayout {
                        c.dirty = true;
                    }
                    let node = c.node;
                    self.frame_side_effects(page, node, o, &mut out);
                }
            }
        }
        if page.frame_depth == 0 {
            self.route_frame_messages(page, &mut out);
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
        self.finish_page_load(&mut child);
        let nav = child.script.pending_navigation.take();
        page.frames.children[i].page = Some(Box::new(child));
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
        let referer = c.page.as_ref().map(|p| p.url.clone());
        if let Some(old) = c.page.take() {
            let mut reg = self.frames.borrow_mut();
            for g in &old.frames.children {
                reg.remove_tree(g.id);
            }
        }
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
            if let Some(c) = prev.and_then(|n| page.frame_mut(n)) {
                if let Some(child) = c.page.as_mut() {
                    let o = self.pointer_move(child, -1.0, -1.0, m);
                    if o.relayout {
                        c.dirty = true;
                    }
                }
            }
        }
        page.pointer.frame_hover = hit.map(|h| h.0);
        let Some((node, cx, cy)) = hit else { return false };
        if let Some(c) = page.frame_mut(node) {
            if let Some(child) = c.page.as_mut() {
                let o = self.pointer_move(child, cx, cy, m);
                if o.relayout {
                    c.dirty = true;
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
        let c = page.frame_mut(node)?;
        let child = c.page.as_mut()?;
        let mut d = self.pointer_down(child, cx, cy, button, m);
        let o = std::mem::take(&mut d.outcome);
        c.dirty = true;
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
        let c = page.frame_mut(node)?;
        let child = c.page.as_mut()?;
        let mut up = self.pointer_up(child, cx, cy, button, m);
        let o = std::mem::take(&mut up.outcome);
        c.dirty = true;
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
        let c = page.frame_mut(node)?;
        let child = c.page.as_mut()?;
        let mut o = match self.frame_key_event(child, key, code, pressed, m) {
            Some(o) => o,
            None => key_into_page(self, child, key, code, pressed, m),
        };
        if o.relayout {
            c.dirty = true;
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
        Some(u) if matches!(u.scheme.as_str(), "http" | "https" | "data" | "file" | "about") => FrameSource::Url(u),
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
            host.realm().set_deadline(Some(std::time::Instant::now() + crate::scripting::task_budget()));
            host.deliver_message(&mut env, &m.data, &m.from_origin, m.from);
            host.run_jobs(&mut env);
            host.realm().set_deadline(None);
        }
        page.script.host = Some(host);
        return Some(engine.settle(page, false));
    }
    for i in 0..page.frames.children.len() {
        let node = page.frames.children[i].node;
        let r = {
            let c = &mut page.frames.children[i];
            let Some(child) = c.page.as_mut() else { continue };
            let r = deliver_in_tree(engine, child, m);
            if let Some(o) = &r {
                if o.relayout {
                    c.dirty = true;
                }
            }
            r
        };
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
