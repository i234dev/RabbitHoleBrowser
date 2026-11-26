//! Incremental style / layout / display-list pipeline.
//!
//! The DOM records its mutations ([`rabbithole_dom::MutationLog`]); the
//! dynamic selector state lives in `Page::state`. Flushing style turns both
//! into a minimal restyle ([`rabbithole_style::StyleEngine::update`]) and
//! accumulates *layout damage*: the elements whose boxes must be rebuilt.
//! Flushing layout rebuilds only those boxes (clean subtrees are moved over
//! from the previous box tree) and re-lays out only boxes whose inputs
//! changed (clean boxes with the same constraints are translated into
//! place). The display list is rebuilt lazily, only when something paints
//! or hit tests.
//!
//! `RH_VERIFY_INCREMENTAL=1` recomputes everything from scratch after each
//! incremental step and panics on any difference.

use crate::{canvas_background, media_env, rasterize_inline_svgs, resolve_url, sheet_has_dynamic_rules, stylesheet_signature, Engine, Page};
use rabbithole_dom::{Document, Mutation, MutationLog, NodeId};
use rabbithole_layout::{LayoutContext, LayoutDamage};
use rabbithole_style::{StyleEngine, StyleOptions};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// What the embedder's canvas holds, as far as damage painting knows.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum CanvasSync {
    /// Unknown: repaint everything.
    #[default]
    Unknown,
    /// The current display list painted at this scroll position.
    Current((f32, f32)),
    /// `painted_dl` painted at this scroll position.
    Stale((f32, f32)),
}

/// Counters for profiling / tests.
#[derive(Debug, Default, Clone)]
pub struct IncrementalStats {
    pub style_full: u64,
    pub style_incremental: u64,
    pub elements_restyled: u64,
    pub layout_full: u64,
    pub layout_incremental: u64,
    pub display_lists: u64,
}

/// Incremental pipeline state of one page.
#[derive(Default)]
pub struct IncrementalState {
    style: Option<StyleEngine>,
    /// Some element was styled as a query container: layout measures them
    /// for container queries and `cq*` units.
    has_containers: bool,
    /// Revision of this document view and the shared host catalog.
    fonts_seen: Option<(u64, u64)>,
    /// `@font-face` rules seen by the styles (count, hash), to load fonts
    /// for rules added after the page loaded.
    font_faces_seen: Option<u64>,
    /// Web fonts registered in the page's font context.
    web_fonts: HashSet<String>,
    /// Families of `@font-face` rules whose fonts are not loaded (no text
    /// used them yet): a restyled element using one loads them.
    unloaded_families: HashSet<String>,
    uses_unloaded_family: bool,
    known_families: HashSet<String>,
    /// (DOM epoch, visited history size, base URL) the `:visited` link set
    /// was computed for.
    pub(crate) visited_key: Option<(u64, usize, String)>,
    /// Base URL and the destination (without fragment) of each link
    /// `href` resolved against it, for the `:visited` link set.
    pub(crate) link_targets: (String, HashMap<String, Option<String>>),
    /// Lay out from scratch next time.
    layout_full: bool,
    /// Elements whose boxes must be rebuilt.
    box_dirty: HashSet<NodeId>,
    /// Layout is out of date.
    layout_dirty: bool,
    /// The display list is out of date.
    dl_dirty: bool,
    /// Rasterized inline `<svg>` roots that use filters: rasterized again
    /// when their content changes.
    filtered_svgs: HashSet<NodeId>,
    /// Inline `<svg>` roots rasterized when the page is presented.
    pub(crate) pending_svgs: Vec<crate::present::PendingSvg>,
    /// Canvases (element, 2D surface) whose drawing is taken when the
    /// page is presented.
    pub(crate) pending_canvases: Vec<(NodeId, u32)>,
    /// A canvas drew since the last script task ended.
    pub(crate) canvas_drawn: bool,
    /// Inputs of the last layout, to detect changes made behind our back.
    images_seen: HashMap<NodeId, usize>,
    bg_seen: Vec<(String, usize)>,
    scroll_seen: HashMap<NodeId, (f32, f32)>,
    viewport_seen: (f32, f32),
    /// Cached "painting depends on page scroll" for the current layout.
    scroll_dep: Option<bool>,
    /// Element border boxes looked up since the last layout change
    /// (`offsetWidth` & co. read twice cost nothing).
    rects: std::cell::RefCell<HashMap<NodeId, Option<rabbithole_gfx::Rect>>>,
    /// Border-box unions of the line fragments of every inline element
    /// (built in one pass over the layout on first use).
    inline_index: std::cell::RefCell<Option<HashMap<NodeId, rabbithole_gfx::Rect>>>,
    /// Styles of unrendered elements computed since the last style flush
    /// (`getComputedStyle` inside `display: none` subtrees).
    unrendered: std::cell::RefCell<HashMap<NodeId, Option<std::rc::Rc<rabbithole_style::ComputedStyle>>>>,
    /// [`side_sheet_signature`] at the last style flush.
    side_sheets: u64,
    /// Device pixel ratio images were last selected for.
    dpr_seen: Option<f32>,
    /// What the embedder's canvas shows (for damage repaints).
    canvas: CanvasSync,
    /// The display list the canvas shows when it is out of date.
    painted_dl: Option<rabbithole_layout::DisplayList>,
    /// A decoded video picture has changed since the last GUI paint.
    pub video_frame_pending: bool,
    pub(crate) video_timings: Vec<(NodeId, rabbithole_script::FrameTiming)>,
    pub stats: IncrementalStats,
}

impl IncrementalState {
    pub(crate) fn font_faces_seen(&self) -> Option<u64> {
        self.font_faces_seen
    }

    pub(crate) fn set_font_faces_seen(&mut self, sig: u64) {
        self.font_faces_seen = Some(sig);
    }

    /// Whether `key` is a web font not registered yet (and register it).
    pub(crate) fn register_web_font(&mut self, key: String) -> bool {
        self.web_fonts.insert(key)
    }

    pub(crate) fn known_families(&self) -> HashSet<String> {
        self.known_families.clone()
    }

    pub(crate) fn set_known_families(&mut self, families: HashSet<String>) {
        self.known_families = families;
    }

    pub(crate) fn set_unloaded_families(&mut self, families: HashSet<String>) {
        self.unloaded_families = families;
    }

    pub(crate) fn mark_layout_full(&mut self) {
        self.layout_full = true;
        self.layout_dirty = true;
    }
}

impl IncrementalState {
    /// The style engine of the current styles (after a style flush).
    pub(crate) fn style_engine(&self) -> Option<&StyleEngine> {
        self.style.as_ref()
    }

    pub(crate) fn fonts_changed(&self, fonts: &rabbithole_gfx::FontContext) -> bool {
        self.fonts_seen != Some(fonts.selection_generation())
    }

    /// Is a layout or display list flush pending?
    pub fn dirty(&self) -> bool {
        self.layout_dirty || self.dl_dirty || self.layout_full
    }

    /// Changes whenever styles, layout or the display list were updated.
    pub fn generation(&self) -> u64 {
        let s = &self.stats;
        s.style_full + s.style_incremental + s.layout_full + s.layout_incremental + s.display_lists
    }

    /// Did the device pixel ratio change since the last call (image
    /// candidates must be selected again)?
    pub(crate) fn take_dpr_change(&mut self, dpr: f32) -> bool {
        std::mem::replace(&mut self.dpr_seen, Some(dpr)) != Some(dpr)
    }

    /// `node`'s computed style was replaced outside the style engine (an
    /// animation overlay): rebuild its box.
    pub(crate) fn style_patched(&mut self, node: NodeId) {
        self.box_dirty.insert(node);
        self.layout_dirty = true;
        self.dl_dirty = true;
        self.stats.style_incremental += 1;
        self.clear_rects();
    }

    /// Restyle and lay out everything on the next flush.
    pub fn invalidate_all(&mut self) {
        self.style = None;
        self.invalidate_layout();
    }

    /// Lay out everything on the next flush.
    pub fn invalidate_layout(&mut self) {
        self.layout_full = true;
        self.layout_dirty = true;
        self.dl_dirty = true;
    }

    /// Cached [`rabbithole_layout::LayoutTree::depends_on_page_scroll`].
    pub(crate) fn depends_on_page_scroll(&mut self, layout: &rabbithole_layout::LayoutTree) -> bool {
        *self.scroll_dep.get_or_insert_with(|| layout.depends_on_page_scroll())
    }

    fn clear_rects(&self) {
        self.rects.borrow_mut().clear();
        *self.inline_index.borrow_mut() = None;
    }

    /// The rect of inline element `node` from the inline index, built with
    /// `build` when the layout changed since the last use.
    pub(crate) fn cached_inline_rect(&self, node: NodeId, build: impl FnOnce() -> HashMap<NodeId, rabbithole_gfx::Rect>) -> Option<rabbithole_gfx::Rect> {
        let mut idx = self.inline_index.borrow_mut();
        idx.get_or_insert_with(build).get(&node).copied()
    }

    /// Cached element rectangle lookup.
    pub(crate) fn cached_rect(&self, node: NodeId, f: impl FnOnce() -> Option<rabbithole_gfx::Rect>) -> Option<rabbithole_gfx::Rect> {
        if let Some(r) = self.rects.borrow().get(&node) {
            return *r;
        }
        let r = f();
        self.rects.borrow_mut().insert(node, r);
        r
    }

    /// An inner scroll container was scrolled by translation.
    pub(crate) fn scrolled(&mut self, node: NodeId, offset: (f32, f32)) {
        self.clear_rects();
        self.scroll_seen.insert(node, offset);
    }

    /// Preserve the painted list before an in-place image update. Further
    /// updates before painting must keep that first snapshot, not the last
    /// intermediate list; unknown canvases already require a full repaint.
    pub(crate) fn display_list_will_change(&mut self, old: &rabbithole_layout::DisplayList) {
        if matches!(self.canvas, CanvasSync::Current(_)) {
            self.display_list_replaced(old.clone());
        }
    }

    /// `node`'s image `old` was replaced by `new` in the page and in its
    /// box (same natural size): layout need not look at it again.
    pub(crate) fn image_replaced(&mut self, node: NodeId, old: &Arc<rabbithole_gfx::Image>, new: &Arc<rabbithole_gfx::Image>) {
        let ptr = |v: &Arc<rabbithole_gfx::Image>| Arc::as_ptr(v) as *const u8 as usize;
        if let Some(seen) = self.images_seen.get_mut(&node).filter(|s| **s == ptr(old)) {
            *seen = ptr(new);
        }
    }

    /// A new display list replaced `old`.
    pub(crate) fn display_list_replaced(&mut self, old: rabbithole_layout::DisplayList) {
        if let CanvasSync::Current(s) = self.canvas {
            self.painted_dl = Some(old);
            self.canvas = CanvasSync::Stale(s);
        }
    }

    /// The canvas the page was painted into is gone: the next damage-based
    /// paint repaints everything.
    pub(crate) fn forget_canvas(&mut self) {
        self.painted_dl = None;
        self.canvas = CanvasSync::Unknown;
    }

    pub(crate) fn mark_display_list_clean(&mut self) {
        self.dl_dirty = false;
        self.stats.display_lists += 1;
    }
}

pub(crate) fn verify_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("RH_VERIFY_INCREMENTAL").map(|v| v != "0" && !v.is_empty()).unwrap_or(false))
}

pub(crate) fn debug_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RH_DEBUG_INCREMENTAL").is_some())
}

/// Hash of the style sources kept outside the node tree: CSSOM-modified
/// sheets, adopted / constructed sheets and shadow roots.
fn side_sheet_signature(doc: &Document) -> u64 {
    use std::hash::{Hash, Hasher};
    if doc.sheet_overrides.is_empty() && doc.adopted_styles.is_empty() && doc.shadow_roots.is_empty() {
        return 0;
    }
    // Every mutable access to those maps gets a new stamp: no need to hash
    // their (possibly large) texts on every flush.
    let mut h = std::collections::hash_map::DefaultHasher::new();
    doc.sheet_overrides.stamp().hash(&mut h);
    doc.adopted_styles.stamp().hash(&mut h);
    doc.shadow_roots.len().hash(&mut h);
    h.finish() | 1
}

/// Does a mutation possibly change the document's style sheets?
fn touches_sheets(doc: &Document, m: &Mutation) -> bool {
    let is_sheet = |n: NodeId| matches!(doc.tag(n), Some("style" | "link"));
    let subtree_has_sheet = |n: NodeId| doc.element(n).is_some() && doc.descendants(n).any(is_sheet);
    match m {
        Mutation::Attr { node, .. } => is_sheet(*node),
        Mutation::Children { parent, child } => is_sheet(*parent) || subtree_has_sheet(*child),
        Mutation::Text { node } => doc.parent(*node).map(is_sheet).unwrap_or(false),
        Mutation::Node { node } => is_sheet(*node) || doc.parent(*node).map(is_sheet).unwrap_or(false),
    }
}

impl Engine {
    /// Computed style of an element that has no stored style because it
    /// is not rendered (inside a `display: none` subtree): computed along
    /// its ancestor chain. Styles must be flushed first.
    pub fn unrendered_style(&self, page: &Page, node: NodeId) -> Option<std::rc::Rc<rabbithole_style::ComputedStyle>> {
        page.doc.element(node)?;
        if let Some(s) = page.incr.unrendered.borrow().get(&node) {
            return s.clone();
        }
        let s = self.unrendered_style_uncached(page, node);
        page.incr.unrendered.borrow_mut().insert(node, s.clone());
        s
    }

    fn unrendered_style_uncached(&self, page: &Page, node: NodeId) -> Option<std::rc::Rc<rabbithole_style::ComputedStyle>> {
        let se = page.incr.style.as_ref()?;
        let base_str = page.base_url.to_string();
        let opts = StyleOptions {
            state: &page.state,
            resolve_url: &|base, raw| resolve_url(base, raw),
            document_base: &base_str,
        };
        let _font_units = rabbithole_style::values::scope_unit_fonts(Some(page.fonts.clone()), rabbithole_style::values::font_unit_skip(&page.styles.font_faces));
        se.style_chain_from(&page.doc, &opts, node, Some(&page.styles))
    }

    /// Bring `page.styles` up to date with the DOM, stylesheets, viewport
    /// and dynamic state, restyling only what changed. Layout is marked
    /// dirty (see [`Engine::flush_layout`]).
    pub fn flush_style(&self, page: &mut Page) {
        page.incr.unrendered.get_mut().clear();
        let fonts_now = page.fonts.selection_generation();
        if page.incr.fonts_seen != Some(fonts_now) {
            if debug_enabled() && page.incr.fonts_seen.is_some() {
                eprintln!("[incr] font selection changed: full restyle + layout");
            }
            page.incr.fonts_seen = Some(fonts_now);
            page.incr.style = None;
            page.incr.layout_full = true;
            page.incr.layout_dirty = true;
        }
        let log = if page.doc.is_tracking_mutations() {
            page.doc.take_mutations()
        } else {
            page.doc.track_mutations(true);
            MutationLog { entries: Vec::new(), overflow: true }
        };
        // Scripts (this document's, a parent frame's, or evaluated from
        // outside) may add/remove `<style>`/`<link>` elements, change their
        // text or insert rules (CSS-in-JS): re-collect the sheets when
        // their sources changed.
        // CSSOM edits and constructed / adopted sheets live outside the
        // node tree (not in the mutation log).
        let side = side_sheet_signature(&page.doc);
        let side_changed = std::mem::replace(&mut page.incr.side_sheets, side) != side;
        let touched = log.overflow || log.entries.iter().any(|m| touches_sheets(&page.doc, m));
        if side_changed || touched {
            // Only the sheets outside the tree changed (adopted sheets,
            // shadow roots attached): collect again without hashing every
            // `<style>` of the document first; the stored signature then
            // matches no real one, so the next tree change collects again.
            let sig = if touched { stylesheet_signature(&page.doc) } else { page.sheet_signature ^ 1 };
            if sig != page.sheet_signature {
                page.sheet_signature = sig;
                page.sheets = self.loader.collect_stylesheets(&page.doc, &page.base_url);
                page.has_dynamic_rules = page.sheets.iter().any(sheet_has_dynamic_rules);
                // Only shadow-root sheets changed (components attached or
                // adopted sheets): keep the engine, restyle those hosts.
                if !page.incr.style.as_mut().is_some_and(|se| se.set_sheets(&page.sheets)) {
                    page.incr.style = None;
                }
            }
            self.queue_link_events(page);
        }
        let env = media_env(page.viewport, self.scripting);
        self.update_visited_links(page, &log);
        let base_str = page.base_url.to_string();
        let state = page.state.clone();
        let opts = StyleOptions {
            state: &state,
            resolve_url: &|base, raw| resolve_url(base, raw),
            document_base: &base_str,
        };
        let fresh = !matches!(&page.incr.style, Some(se) if se.env_matches(&env));
        if !fresh {
            let se = page.incr.style.as_ref().expect("style engine");
            if log.is_empty() && !se_state_differs(se, &state) && !se.has_pending(&page.doc) {
                return;
            }
        }
        let _font_units = rabbithole_style::cascade::font_unit_scope(Some(page.fonts.clone()), &page.sheets, &env, &opts);
        let incr = &mut page.incr;
        let mut anim_changes: Vec<crate::animations::Change> = Vec::new();
        let mut anim_full = fresh;
        // Elements this flush started rendering (incremental restyles).
        let mut newly_styled: Vec<NodeId> = Vec::new();
        // Elements restyled (only collected while filtered SVGs exist).
        let mut restyled: Vec<NodeId> = Vec::new();
        let anim_snapshot = page.anim.snapshot(&page.styles);
        // Element styles before a full recomputation.
        let mut previous_styles: Vec<Option<std::rc::Rc<rabbithole_style::ComputedStyle>>> = Vec::new();
        if fresh {
            let mut se = StyleEngine::new(&page.sheets, &env);
            previous_styles = std::mem::replace(&mut page.styles, se.full(&page.doc, &page.sheets, &opts)).styles;
            incr.style = Some(se);
            incr.stats.style_full += 1;
            incr.layout_full = true;
        } else {
            let se = incr.style.as_mut().expect("style engine");
            let mut out = se.update(&page.doc, &mut page.styles, &log, &opts, &page.sheets);
            previous_styles = std::mem::take(&mut out.previous);
            if debug_enabled() {
                eprintln!("[incr] style: full={} recomputed={} changed={} mutations={}", out.full, out.recomputed, out.changes.len(), log.entries.len());
                if out.full || out.recomputed > 500 {
                    let d = &page.doc;
                    let desc = |n: NodeId| format!("{}{}{}", d.tag(n).unwrap_or("#"), d.attr(n, "id").map(|v| format!("#{v}")).unwrap_or_default(), d.attr(n, "class").map(|v| format!(".{}", v.split_whitespace().next().unwrap_or(""))).unwrap_or_default());
                    for m in log.entries.iter().take(8) {
                        match m {
                            rabbithole_dom::Mutation::Attr { node, name, .. } => eprintln!("[incr]   attr {name} on {}", desc(*node)),
                            rabbithole_dom::Mutation::Children { parent, child } => eprintln!("[incr]   children of {} ({})", desc(*parent), desc(*child)),
                            rabbithole_dom::Mutation::Text { node } => eprintln!("[incr]   text in {:?}", d.parent(*node).map(desc)),
                            rabbithole_dom::Mutation::Node { node } => eprintln!("[incr]   node {}", desc(*node)),
                        }
                    }
                }
            }
            anim_full = out.full;
            if !out.full && (!page.anim.is_idle() || out.changes.iter().any(|c| c.new.as_ref().map(|s| !s.transitions.is_empty() || !s.animations.is_empty()).unwrap_or(false))) {
                anim_changes = out.changes.iter().map(|c| (c.node, c.old.clone(), c.new.clone())).collect();
            }
            if out.full {
                incr.stats.style_full += 1;
                incr.layout_full = true;
            } else {
                incr.stats.style_incremental += 1;
                incr.stats.elements_restyled += out.recomputed as u64;
                if !incr.filtered_svgs.is_empty() {
                    restyled.extend(out.changes.iter().map(|c| c.node));
                }
                if !incr.unloaded_families.is_empty() {
                    let unloaded = &incr.unloaded_families;
                    incr.uses_unloaded_family |= out.changes.iter().any(|c| c.new.as_ref().is_some_and(|s| s.font_family_list.iter().any(|f| unloaded.contains(&f.trim_matches('"').to_ascii_lowercase()))));
                }
                for c in &out.changes {
                    if c.old.as_ref().is_none_or(|s| s.display.is_none()) {
                        newly_styled.push(c.node);
                    }
                    incr.box_dirty.insert(c.node);
                    let display_changed = match (&c.old, &c.new) {
                        (Some(a), Some(b)) => a.display != b.display || a.position != b.position || a.float != b.float,
                        _ => true,
                    };
                    if display_changed {
                        // Children's boxes depend on the parent's formatting
                        // context (blockification, table rows, ...).
                        incr.box_dirty.extend(page.doc.flat_child_elements(c.node));
                    }
                }
                for &n in &out.flat_changed {
                    incr.box_dirty.insert(n);
                    if page.doc.shadow_root(n).is_some() {
                        incr.box_dirty.extend(page.doc.flat_child_elements(n));
                    }
                }
            }
            damage_from_log(&page.doc, &page.styles, &log, &mut incr.box_dirty, &mut incr.layout_full);
            if verify_enabled() {
                let mut check = StyleEngine::new(&page.sheets, &env);
                let full = check.full(&page.doc, &page.sheets, &opts);
                if let Err(e) = check_styles(&page.doc, &page.styles, &full) {
                    panic!("RH_VERIFY_INCREMENTAL: {e}");
                }
            }
        }
        incr.layout_dirty = true;
        incr.dl_dirty = true;
        let uses_unloaded = std::mem::take(&mut page.incr.uses_unloaded_family);
        self.load_added_fonts(page, uses_unloaded);
        page.background = canvas_background(&page.doc, &page.styles);
        if fresh || log.overflow || anim_full {
            crate::present::prune_pending_svgs(page);
            rasterize_inline_svgs(&page.doc, &page.styles, &mut page.images, None, &mut page.incr.filtered_svgs, &mut page.incr.pending_svgs);
        } else {
            // Only newly rendered elements can be SVGs without an image.
            let mut roots: Vec<NodeId> = std::mem::take(&mut newly_styled).into_iter().filter(|&n| page.doc.is_element(n, "svg")).collect();
            // Filtered SVGs whose content changed are rasterized again.
            if !page.incr.filtered_svgs.is_empty() {
                let doc = &page.doc;
                let filtered = &page.incr.filtered_svgs;
                let mut stale: Vec<NodeId> = Vec::new();
                let mut consider = |n: NodeId| {
                    let mut top = None;
                    let mut cur = Some(n);
                    while let Some(c) = cur {
                        if doc.is_element(c, "svg") {
                            top = Some(c);
                        }
                        cur = doc.parent(c);
                    }
                    if let Some(t) = top.filter(|t| filtered.contains(t) && !stale.contains(t)) {
                        stale.push(t);
                    }
                };
                for m in &log.entries {
                    match m {
                        Mutation::Attr { node, .. } | Mutation::Node { node } | Mutation::Text { node } => consider(*node),
                        Mutation::Children { parent, .. } => consider(*parent),
                    }
                }
                for &n in &restyled {
                    consider(n);
                }
                for t in stale {
                    page.images.remove(&t);
                    if !roots.contains(&t) {
                        roots.push(t);
                    }
                }
            }
            if !roots.is_empty() {
                crate::present::prune_pending_svgs(page);
                rasterize_inline_svgs(&page.doc, &page.styles, &mut page.images, Some(&roots), &mut page.incr.filtered_svgs, &mut page.incr.pending_svgs);
            }
        }
        if anim_full {
            // A full restyle lists no changes: compare the elements with
            // transitions against their styles before it.
            let snapshotted: HashSet<NodeId> = anim_snapshot.iter().map(|c| c.0).collect();
            for (n, old) in anim_snapshot {
                let new = page.styles.get(n).cloned();
                if new.as_ref().map(|s| **s != *old).unwrap_or(true) {
                    anim_changes.push((n, Some(old), new));
                }
            }
            // Elements that only gained transitions in this restyle: their
            // after-change style decides (CSS Transitions §3).
            for (n, new) in page.styles.styles.iter().enumerate() {
                let Some(new) = new.as_ref().filter(|s| !s.transitions.is_empty()) else { continue };
                if snapshotted.contains(&n) {
                    continue;
                }
                let old = page.anim.base_style(n).or_else(|| previous_styles.get(n).cloned().flatten());
                if let Some(old) = old.filter(|o| **o != **new) {
                    anim_changes.push((n, Some(old), Some(new.clone())));
                }
            }
        }
        // Elements with animation effects whose style attribute changed to
        // values equal to the animated ones (`commitStyles`) are not
        // reported as changed: their base style is stale.
        let restyled_bases: Vec<NodeId> = if page.anim.is_idle() {
            Vec::new()
        } else {
            log.entries
                .iter()
                .filter_map(|m| match m {
                    rabbithole_dom::Mutation::Attr { node, name, .. } if name == "style" && !anim_changes.iter().any(|c| c.0 == *node) => Some(*node),
                    _ => None,
                })
                .collect()
        };
        if anim_full || !anim_changes.is_empty() {
            self.animations_after_style(page, anim_changes, anim_full);
        }
        if !restyled_bases.is_empty() && !anim_full {
            self.refresh_animation_bases(page, &restyled_bases);
        }
    }

    /// Bring `page.layout` up to date (incrementally when possible). The
    /// display list is left stale (see [`Engine::flush_display_list`]).
    /// Query containers measured by layout feed back into style: when
    /// their sizes changed, style and layout run again (a few times at
    /// most, as container queries cannot change their containers' sizes in
    /// ways that loop for long).
    pub fn flush_layout(&self, page: &mut Page) {
        for _ in 0..4 {
            self.flush_layout_once(page);
            if !page.incr.has_containers {
                return;
            }
            let found = collect_containers(&page.layout.root);
            if *page.state.containers == found {
                return;
            }
            // Changed containers restyle their subtrees (also pseudo-elements
            // such as ::first-letter, whose changes are not listed): lay
            // those boxes out again.
            let old = page.state.containers.clone();
            let changed: Vec<NodeId> = old.iter().filter(|(n, i)| found.get(n) != Some(i)).map(|(n, _)| *n).chain(found.keys().filter(|n| !old.contains_key(n)).copied()).collect();
            for n in changed {
                if page.doc.is_connected(n) {
                    page.incr.box_dirty.extend(page.doc.descendants(n).filter(|&d| page.doc.element(d).is_some()));
                    page.incr.layout_dirty = true;
                }
            }
            page.state.containers = Arc::new(found);
        }
    }

    fn flush_layout_once(&self, page: &mut Page) {
        self.flush_style(page);
        page.incr.has_containers |= rabbithole_style::take_containers_seen();
        let _font_units = rabbithole_style::values::scope_unit_fonts(Some(page.fonts.clone()), rabbithole_style::values::font_unit_skip(&page.styles.font_faces));
        let Page { fonts, doc, styles, images, background_images, viewport, scroll_offsets, page_scroll, layout, incr, .. } = page;
        // Inputs changed outside the style system: images, background
        // images, scroll offsets, viewport.
        let img_ptr = |v: &Arc<rabbithole_gfx::Image>| Arc::as_ptr(v) as *const u8 as usize;
        let images_same = images.len() == incr.images_seen.len() && images.iter().all(|(n, img)| incr.images_seen.get(n) == Some(&img_ptr(img)));
        if !images_same {
            let images_now: HashMap<NodeId, usize> = images.iter().map(|(n, img)| (*n, img_ptr(img))).collect();
            for (n, p) in &images_now {
                if incr.images_seen.get(n) != Some(p) {
                    incr.box_dirty.insert(*n);
                }
            }
            for n in incr.images_seen.keys() {
                if !images_now.contains_key(n) {
                    incr.box_dirty.insert(*n);
                }
            }
            incr.images_seen = images_now;
            incr.layout_dirty = true;
        }
        let bg_same = background_images.len() == incr.bg_seen.len() && incr.bg_seen.iter().all(|(k, p)| background_images.get(k).is_some_and(|v| img_ptr(v) == *p));
        if !bg_same {
            let mut bg_now: Vec<(String, usize)> = background_images.iter().map(|(k, v)| (k.clone(), img_ptr(v))).collect();
            bg_now.sort();
            // Background images reach layout only as generated `content:
            // url()` images; any other use (backgrounds, border images) is
            // painted from the display list.
            let changed: HashSet<&str> = bg_now
                .iter()
                .filter(|e| incr.bg_seen.binary_search(e).is_err())
                .chain(incr.bg_seen.iter().filter(|e| bg_now.binary_search(e).is_err()))
                .map(|(k, _)| k.as_str())
                .collect();
            if generated_content_uses(styles, &changed) {
                if debug_enabled() {
                    eprintln!("[incr] full layout: generated-content image changed");
                }
                incr.layout_full = true;
                incr.layout_dirty = true;
            } else {
                incr.dl_dirty = true;
            }
            incr.bg_seen = bg_now;
        }
        if *scroll_offsets != incr.scroll_seen {
            for (n, o) in scroll_offsets.iter() {
                if incr.scroll_seen.get(n) != Some(o) {
                    incr.box_dirty.insert(*n);
                }
            }
            for n in incr.scroll_seen.keys() {
                if !scroll_offsets.contains_key(n) {
                    incr.box_dirty.insert(*n);
                }
            }
            incr.scroll_seen = scroll_offsets.clone();
            incr.layout_dirty = true;
        }
        if incr.viewport_seen != (viewport.width, viewport.height) {
            incr.viewport_seen = (viewport.width, viewport.height);
            incr.layout_full = true;
            incr.layout_dirty = true;
        }
        if !incr.layout_dirty && !incr.layout_full {
            return;
        }
        let ctx = LayoutContext {
            doc,
            styles,
            fonts,
            images,
            background_images,
            viewport_width: viewport.width,
            viewport_height: viewport.height,
            scroll_offsets,
            page_scroll: *page_scroll,
        };
        let damage = LayoutDamage { full: incr.layout_full, dirty: std::mem::take(&mut incr.box_dirty) };
        if debug_enabled() {
            eprintln!("[incr] layout: full={} dirty={}", damage.full, damage.dirty.len());
        }
        if damage.full {
            incr.stats.layout_full += 1;
        } else {
            incr.stats.layout_incremental += 1;
        }
        let prev = std::mem::replace(layout, rabbithole_layout::LayoutTree::empty());
        let t = std::time::Instant::now();
        *layout = rabbithole_layout::layout_incremental(prev, &ctx, &damage);
        if debug_enabled() {
            let (h, m, n) = rabbithole_layout::take_cache_stats();
            eprintln!("[incr] layout {:?}: cache hits {h}, misses {m}, fresh {n}", t.elapsed());
        }
        if verify_enabled() && !damage.full {
            let full = rabbithole_layout::layout(&ctx);
            if let Err(e) = check_dump("layout", &full.dump(), &layout.dump()) {
                panic!("RH_VERIFY_INCREMENTAL: {e}");
            }
        }
        incr.layout_full = false;
        incr.layout_dirty = false;
        incr.dl_dirty = true;
        incr.scroll_dep = None;
        incr.clear_rects();
    }

    /// Rebuild the display list if layout or paint inputs changed.
    pub fn flush_display_list(&self, page: &mut Page) {
        self.flush_layout(page);
        if page.incr.dl_dirty {
            self.rebuild_display_list(page);
        }
    }
}

impl Engine {
    /// Repaint after the page scrolled from `old` to `page.page_scroll`,
    /// given that `canvas` holds the frame painted at `old` (scale 1):
    /// the pixels are shifted and only the exposed strips plus the fixed
    /// and sticky layers are re-rastered. Falls back to a full repaint when
    /// that is not possible. Returns the repainted regions.
    pub fn render_scrolled(&self, page: &mut Page, canvas: &mut rabbithole_gfx::Canvas, old: (f32, f32)) -> Vec<rabbithole_gfx::Rect> {
        use rabbithole_gfx::Rect;
        let new = page.page_scroll;
        let (w, h) = (canvas.width as i64, canvas.height as i64);
        let full = Rect::new(0.0, 0.0, w as f32, h as f32);
        let (dx, dy) = ((new.0 - old.0).round() as i64, (new.1 - old.1).round() as i64);
        let integral = (new.0 - old.0 - dx as f32).abs() < 1e-3 && (new.1 - old.1 - dy as f32).abs() < 1e-3;
        self.present(page);
        let synced = matches!(page.incr.canvas, CanvasSync::Current(s) | CanvasSync::Stale(s) if s == old);
        if !synced || !integral || dx.abs() >= w || dy.abs() >= h || page.incr.depends_on_page_scroll(&page.layout) {
            self.render(page, canvas, new.0, new.1);
            page.incr.painted_dl = None;
            page.incr.canvas = CanvasSync::Current(new);
            return vec![full];
        }
        // Shift the old pixels: content moves by (-dx, -dy).
        let row = w as usize;
        let data = &mut canvas.data;
        let copy_row = |data: &mut Vec<u32>, src_y: i64, dst_y: i64| {
            let (s0, d0) = (src_y as usize * row, dst_y as usize * row);
            if dx >= 0 {
                let n = (w - dx) as usize;
                data.copy_within(s0 + dx as usize..s0 + dx as usize + n, d0);
            } else {
                let n = (w + dx) as usize;
                data.copy_within(s0..s0 + n, d0 + (-dx) as usize);
            }
        };
        if dy >= 0 {
            for y in 0..h - dy {
                copy_row(data, y + dy, y);
            }
        } else {
            for y in (-dy..h).rev() {
                copy_row(data, y + dy, y);
            }
        }
        let mut damage = Vec::new();
        if dy > 0 {
            damage.push(Rect::new(0.0, (h - dy) as f32, w as f32, dy as f32));
        } else if dy < 0 {
            damage.push(Rect::new(0.0, 0.0, w as f32, (-dy) as f32));
        }
        if dx > 0 {
            damage.push(Rect::new((w - dx) as f32, 0.0, dx as f32, h as f32));
        } else if dx < 0 {
            damage.push(Rect::new(0.0, 0.0, (-dx) as f32, h as f32));
        }
        // Fixed boxes stay put while their shifted copies moved; sticky
        // boxes move by their own displacement. Inflate for shadows.
        const SLOP: f32 = 48.0;
        let inflate = |r: Rect| Rect::new(r.x - SLOP, r.y - SLOP, r.w + 2.0 * SLOP, r.h + 2.0 * SLOP);
        for r in &page.display_list.fixed_bounds {
            damage.push(inflate(*r));
            damage.push(inflate(r.translate(-dx as f32, -dy as f32)));
        }
        let vp = (page.viewport.width, page.viewport.height);
        for p in &page.display_list.sticky {
            let (ox, oy) = p.offset(Rect::new(old.0, old.1, vp.0, vp.1));
            let (nx, ny) = p.offset(Rect::new(new.0, new.1, vp.0, vp.1));
            // Where the old copy landed after the shift, and where it goes.
            damage.push(inflate(p.bounds.translate(ox - new.0, oy - new.1)));
            damage.push(inflate(p.bounds.translate(nx - new.0, ny - new.1)));
        }
        // Display list changes not painted yet.
        if let (CanvasSync::Stale(_), Some(prev)) = (page.incr.canvas, page.incr.painted_dl.as_ref()) {
            match rabbithole_layout::display_list_damage(prev, &page.display_list, new) {
                Some(d) => damage.extend(d),
                None => damage = vec![full],
            }
        }
        page.incr.painted_dl = None;
        page.incr.canvas = CanvasSync::Current(new);
        let damage: Vec<Rect> = damage.into_iter().map(|r| r.intersect(&full)).filter(|r| !r.is_empty()).collect();
        self.repaint_regions(page, canvas, &damage);
        damage
    }

    /// Repaint the page into `canvas` (scale 1) touching only what changed
    /// since the last `render_damage` / `render_scrolled` call: the
    /// rectangles where the old and new display lists differ. The first
    /// call (or after a scroll not done through `render_scrolled`) paints
    /// everything. Returns the repainted device rectangles.
    pub fn render_damage(&self, page: &mut Page, canvas: &mut rabbithole_gfx::Canvas) -> Vec<rabbithole_gfx::Rect> {
        use rabbithole_gfx::Rect;
        self.present(page);
        let scroll = page.page_scroll;
        let full = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
        let damage = match (page.incr.canvas, page.incr.painted_dl.as_ref()) {
            (CanvasSync::Current(s), _) if s == scroll => Vec::new(),
            (CanvasSync::Stale(s), Some(prev)) if s == scroll => match rabbithole_layout::display_list_damage(prev, &page.display_list, scroll) {
                Some(d) => d.into_iter().map(|r| r.intersect(&full)).filter(|r| !r.is_empty()).collect(),
                None => vec![full],
            },
            _ => vec![full],
        };
        page.incr.painted_dl = None;
        page.incr.canvas = CanvasSync::Current(scroll);
        if damage.len() == 1 && damage[0] == full {
            self.render(page, canvas, scroll.0, scroll.1);
        } else {
            self.repaint_regions(page, canvas, &damage);
        }
        damage
    }

    /// For embedders that rasterize the display list themselves (scaled
    /// or overscanned canvases): the regions, in CSS px relative to
    /// `origin` (the document point at the canvas' top-left corner), where
    /// the display list changed since [`Engine::mark_painted`] was called
    /// with the same origin. `Some(empty)`: nothing changed; `None`:
    /// unknown, repaint everything. With fixed content, pass the unchanged
    /// viewport scroll to both calls and translate the result to the canvas.
    pub fn paint_damage_since(&self, page: &mut Page, origin: (f32, f32)) -> Option<Vec<rabbithole_gfx::Rect>> {
        self.present(page);
        match (page.incr.canvas, page.incr.painted_dl.as_ref()) {
            (CanvasSync::Current(s), _) if s == origin => Some(Vec::new()),
            (CanvasSync::Stale(s), Some(prev)) if s == origin => rabbithole_layout::display_list_damage(prev, &page.display_list, origin),
            _ => None,
        }
    }

    /// The embedder's canvas now shows the current display list painted
    /// with its top-left corner at document point `origin`.
    pub fn mark_painted(&self, page: &mut Page, origin: (f32, f32)) {
        page.incr.painted_dl = None;
        page.incr.canvas = CanvasSync::Current(origin);
    }

    fn repaint_regions(&self, page: &Page, canvas: &mut rabbithole_gfx::Canvas, damage: &[rabbithole_gfx::Rect]) {
        use rabbithole_gfx::Rect;
        let scroll = page.page_scroll;
        let damage = rabbithole_layout::merge_damage(damage.to_vec());
        // Mostly everything: one full pass is cheaper than several clipped.
        let area: f32 = damage.iter().map(|r| r.w * r.h).sum();
        if area > 0.6 * canvas.width as f32 * canvas.height as f32 {
            self.render(page, canvas, scroll.0, scroll.1);
            return;
        }
        for r in &damage {
            // Snap to whole pixels so the repaint covers what it clips.
            let r = Rect::new(r.x.floor(), r.y.floor(), (r.right().ceil() - r.x.floor()).max(0.0), (r.bottom().ceil() - r.y.floor()).max(0.0));
            canvas.push_clip(r);
            canvas.fill_rect(r, rabbithole_gfx::Color::WHITE);
            if page.background.a > 0 {
                canvas.fill_rect(r, page.background);
            }
            canvas.pop_clip();
            rabbithole_layout::paint_region(&page.display_list, canvas, &page.fonts, scroll.0, scroll.1, r);
        }
    }
}

/// Some generated content (`::before`, `::after`, `::marker`) shows one of
/// the images `urls` names.
fn generated_content_uses(styles: &rabbithole_style::StyledTree, urls: &HashSet<&str>) -> bool {
    use rabbithole_style::{Content, ContentItem};
    [&styles.before, &styles.after, &styles.marker].into_iter().flatten().flatten().any(|s| match &s.content {
        Content::Items(items) => items.iter().any(|i| matches!(i, ContentItem::Url(u) if urls.contains(u.as_str()))),
        _ => false,
    })
}

fn se_state_differs(se: &StyleEngine, state: &rabbithole_css::MatchState) -> bool {
    se.state() != state
}

/// The query containers of a layout tree with their content-box sizes.
fn collect_containers(root: &rabbithole_layout::LayoutBox) -> HashMap<NodeId, rabbithole_css::container::ContainerInfo> {
    let mut out = HashMap::new();
    let mut stack = vec![root];
    while let Some(b) = stack.pop() {
        let s = &b.style;
        if s.container_inline {
            if let Some(n) = b.node {
                // Vertical subtrees are laid out transposed (logical
                // width = inline size).
                let (mut width, mut height) = if s.layout_transposed { (b.content_rect.h, b.content_rect.w) } else { (b.content_rect.w, b.content_rect.h) };
                // An inline-size container's block size cannot be queried:
                // leave it out so content growing inside it is no change.
                if !s.container_size {
                    if s.writing_mode.is_vertical() { width = 0.0 } else { height = 0.0 }
                }
                out.entry(n).or_insert_with(|| rabbithole_css::container::ContainerInfo {
                    width,
                    height,
                    both_axes: s.container_size,
                    vertical: s.writing_mode.is_vertical(),
                    names: s.container_names.as_deref().cloned().unwrap_or_default(),
                    font_size: s.font_size,
                });
            }
        }
        stack.extend(b.children.iter());
    }
    out
}

/// Layout damage implied by DOM mutations (independent of style changes):
/// changed text, child lists and attributes (replaced elements, list
/// markers, form controls...).
fn damage_from_log(doc: &Document, styles: &rabbithole_style::StyledTree, log: &MutationLog, dirty: &mut HashSet<NodeId>, full: &mut bool) {
    if log.overflow {
        *full = true;
        return;
    }
    // No style changed in this flush: attributes that act only through
    // styles cannot have moved anything. (With style changes they damage
    // as before: boxes such as anchor-positioned ones depend on others
    // without changing style themselves.)
    let style_quiet = dirty.is_empty();
    let shadow = !doc.shadow_roots.is_empty();
    // Boxes follow the flat tree: a shadow root's children are laid out in
    // its host, a host's light children in the slots they are assigned to.
    let boxed = |n: NodeId| if shadow { doc.shadow_host(n).unwrap_or(n) } else { n };
    let add_with_children = |n: NodeId, dirty: &mut HashSet<NodeId>| {
        let n = boxed(n);
        dirty.insert(n);
        if !shadow {
            dirty.extend(doc.child_elements(n));
            return;
        }
        dirty.extend(doc.flat_child_elements(n));
        if let Some(root) = doc.shadow_root(n) {
            // Light children (re)assigned: their slots hold the boxes.
            dirty.extend(doc.descendants(root).filter(|&d| doc.is_element(d, "slot")));
        }
    };
    let host_slots = |host: NodeId, dirty: &mut HashSet<NodeId>| {
        if let Some(root) = doc.shadow_root(host) {
            dirty.insert(host);
            dirty.extend(doc.descendants(root).filter(|&d| doc.is_element(d, "slot")));
        }
    };
    let mut parents_done: HashSet<NodeId> = HashSet::new();
    // Numbered list markers count their `li` siblings: an inserted or
    // removed child renumbers the ordinal markers after it (all of them in
    // reversed lists, or when the removed child's place is unknown).
    // Bullets do not depend on position.
    let ordinal = |n: NodeId| {
        styles.styles.get(n).and_then(|s| s.as_ref()).is_none_or(|s| {
            !matches!(s.list_style_type, rabbithole_style::ListStyleType::None | rabbithole_style::ListStyleType::Disc | rabbithole_style::ListStyleType::Circle | rabbithole_style::ListStyleType::Square)
        })
    };
    let mut renumbered: HashSet<NodeId> = HashSet::new();
    let mut renumbered_all: HashSet<NodeId> = HashSet::new();
    for m in &log.entries {
        match m {
            Mutation::Attr { node, name, .. } => {
                // These reach the boxes of HTML elements only through their
                // styles: when no style changed, the boxes stay as they are.
                let style_only = matches!(name.as_str(), "class" | "id" | "style" | "title" | "role" | "tabindex")
                    || (name.starts_with("data-") && !name.starts_with("data-rh-"))
                    || name.starts_with("aria-");
                if style_only && style_quiet && doc.element(*node).is_some_and(|e| e.ns == rabbithole_dom::Namespace::Html) {
                    continue;
                }
                add_with_children(*node, dirty);
                if name == "value" {
                    if let Some(p) = doc.parent(*node) {
                        add_with_children(p, dirty);
                    }
                }
                if shadow {
                    if let Some(p) = doc.parent(*node) {
                        if name == "slot" {
                            host_slots(p, dirty);
                        }
                    }
                    if name == "name" && doc.is_element(*node, "slot") {
                        if let Some(h) = doc.containing_shadow_root(*node).and_then(|r| doc.shadow_host(r)) {
                            add_with_children(h, dirty);
                        }
                    }
                }
            }
            Mutation::Children { parent, child } => {
                if doc.element(boxed(*parent)).is_none() {
                    if debug_enabled() {
                        eprintln!("[incr] full layout: child list of a non-element ({:?}) changed", doc.tag(*parent));
                    }
                    *full = true;
                    continue;
                }
                let p = boxed(*parent);
                dirty.insert(p);
                dirty.insert(*child);
                // (What depends on the parent alone, once per parent.)
                let inserted = doc.parent(*child) == Some(*parent);
                if !doc.is_element(*child, "li") {
                    // Markers count `li` elements only.
                } else if inserted && doc.attr(*parent, "reversed").is_none() {
                    // Walk the following siblings until one an earlier
                    // mutation already walked from.
                    renumbered.insert(*child);
                    let mut s = doc.next_sibling(*child);
                    while let Some(n) = s {
                        if !renumbered.insert(n) {
                            break;
                        }
                        if doc.is_element(n, "li") && ordinal(n) {
                            dirty.insert(n);
                        }
                        s = doc.next_sibling(n);
                    }
                } else if renumbered_all.insert(*parent) {
                    dirty.extend(doc.child_elements(*parent).filter(|&c| doc.is_element(c, "li") && ordinal(c)));
                }
                // Other children keep their boxes (the parent's box is
                // rebuilt around them).
                if parents_done.insert(*parent) {
                    if shadow && p == *parent {
                        // A light child of a host is laid out in its slot.
                        host_slots(p, dirty);
                    }
                }
                if shadow && doc.element(*child).is_some() && doc.descendants(*child).any(|d| doc.is_element(d, "slot")) {
                    if let Some(h) = doc.containing_shadow_root(*parent).and_then(|r| doc.shadow_host(r)) {
                        add_with_children(h, dirty);
                    }
                }
            }
            Mutation::Text { node } => {
                if let Some(p) = doc.parent(*node) {
                    dirty.insert(boxed(p));
                    if shadow {
                        host_slots(p, dirty);
                    }
                }
            }
            Mutation::Node { node } => {
                add_with_children(*node, dirty);
                if let Some(p) = doc.parent(*node) {
                    dirty.insert(boxed(p));
                    if shadow {
                        host_slots(p, dirty);
                    }
                }
            }
        }
    }
}

fn check_styles(doc: &Document, inc: &rabbithole_style::StyledTree, full: &rabbithole_style::StyledTree) -> Result<(), String> {
    for n in rabbithole_dom::Document::ROOT..doc.nodes.len() {
        if !doc.is_connected(n) || doc.element(n).is_none() {
            continue;
        }
        let pairs = [
            ("style", inc.styles.get(n).cloned().flatten(), full.styles.get(n).cloned().flatten()),
            ("::before", inc.before.get(n).cloned().flatten(), full.before.get(n).cloned().flatten()),
            ("::after", inc.after.get(n).cloned().flatten(), full.after.get(n).cloned().flatten()),
            ("::marker", inc.marker.get(n).cloned().flatten(), full.marker.get(n).cloned().flatten()),
        ];
        for (what, a, b) in pairs {
            let same = match (&a, &b) {
                (Some(a), Some(b)) => **a == **b,
                (None, None) => true,
                _ => false,
            };
            if !same {
                let path: Vec<String> = {
                    let mut v = Vec::new();
                    let mut cur = Some(n);
                    while let Some(c) = cur {
                        if let Some(e) = doc.element(c) {
                            v.push(format!("{}{}", e.tag, e.attr("class").map(|c| format!(".{}", c.replace(' ', "."))).unwrap_or_default()));
                        }
                        cur = doc.parent(c);
                    }
                    v.reverse();
                    v
                };
                let diff = match (&a, &b) {
                    (Some(a), Some(b)) => style_diff(a, b),
                    _ => format!("incremental {:?} vs full {:?}", a.is_some(), b.is_some()),
                };
                return Err(format!("{what} mismatch at node {n} ({}): {diff}", path.join(" > ")));
            }
        }
    }
    Ok(())
}

/// Field-level difference of two styles (via their Debug output).
fn style_diff(a: &rabbithole_style::ComputedStyle, b: &rabbithole_style::ComputedStyle) -> String {
    let da = format!("{a:#?}");
    let db = format!("{b:#?}");
    let mut out = String::new();
    for (la, lb) in da.lines().zip(db.lines()) {
        if la != lb {
            out.push_str(&format!("\n  incremental: {}\n  full:        {}", la.trim(), lb.trim()));
            if out.len() > 2000 {
                break;
            }
        }
    }
    out
}

/// Compare two layout dumps, tolerating float noise from translating
/// reused subtrees instead of laying them out again.
fn check_dump(what: &str, full: &str, inc: &str) -> Result<(), String> {
    if full == inc {
        return Ok(());
    }
    let (fl, il): (Vec<&str>, Vec<&str>) = (full.lines().collect(), inc.lines().collect());
    // Line 0 (document size) last: the first differing box says more.
    for i in (1..fl.len().max(il.len())).chain(std::iter::once(0)) {
        let (a, b) = (fl.get(i).copied().unwrap_or("<missing>"), il.get(i).copied().unwrap_or("<missing>"));
        if a != b && !numbers_close(a, b) {
            let from = i.saturating_sub(if std::env::var_os("RH_VERIFY_CONTEXT").is_some() { 60 } else { 6 });
            let ctx: Vec<String> = (from..(i + 3).min(fl.len())).map(|j| format!("  full {j}: {}", fl[j])).collect();
            return Err(format!("{what} mismatch at line {i}:\n  full:        {a}\n  incremental: {b}\ncontext:\n{}", ctx.join("\n")));
        }
    }
    Ok(())
}

fn display_list_dump(dl: &rabbithole_layout::DisplayList) -> String {
    let mut out = String::new();
    for it in &dl.items {
        out.push_str(&format!("{it:?}\n"));
    }
    for h in &dl.hits.entries {
        out.push_str(&format!("hit {:?} {} {:?}\n", h.rect, h.node, h.scope));
    }
    out
}

impl Engine {
    /// Compare the page's (incrementally maintained) styles, layout and
    /// display list with a from-scratch computation. Flushes first.
    pub fn check_incremental(&self, page: &mut Page) -> Result<(), String> {
        self.flush_display_list(page);
        let env = media_env(page.viewport, self.scripting);
        let base_str = page.base_url.to_string();
        let opts = StyleOptions {
            state: &page.state,
            resolve_url: &|base, raw| resolve_url(base, raw),
            document_base: &base_str,
        };
        let _font_units = rabbithole_style::cascade::font_unit_scope(Some(page.fonts.clone()), &page.sheets, &env, &opts);
        let full_styles = StyleEngine::new(&page.sheets, &env).full(&page.doc, &page.sheets, &opts);
        check_styles(&page.doc, &page.styles, &full_styles)?;
        let ctx = LayoutContext {
            doc: &page.doc,
            styles: &page.styles,
            fonts: &page.fonts,
            images: &page.images,
            background_images: &page.background_images,
            viewport_width: page.viewport.width,
            viewport_height: page.viewport.height,
            scroll_offsets: &page.scroll_offsets,
            page_scroll: page.page_scroll,
        };
        let full_layout = rabbithole_layout::layout(&ctx);
        check_dump("layout", &full_layout.dump(), &page.layout.dump())?;
        let full_dl = rabbithole_layout::build_display_list(&full_layout, &ctx);
        check_dump("display list", &display_list_dump(&full_dl), &display_list_dump(&page.display_list))
    }
}

/// Same text except for numbers that differ by at most 0.15 (dumps print
/// one decimal, so rounding flips show up as 0.1).
fn numbers_close(a: &str, b: &str) -> bool {
    fn split(s: &str) -> Vec<Result<f32, String>> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut num = false;
        for ch in s.chars() {
            let is_num = ch.is_ascii_digit() || ch == '.' || (ch == '-' && !num && cur.is_empty());
            if is_num != num && !cur.is_empty() {
                out.push(if num { cur.parse::<f32>().map_err(|_| cur.clone()) } else { Err(cur.clone()) });
                cur.clear();
            }
            num = is_num;
            cur.push(ch);
        }
        if !cur.is_empty() {
            out.push(if num { cur.parse::<f32>().map_err(|_| cur.clone()) } else { Err(cur.clone()) });
        }
        out
    }
    let (x, y) = (split(a), split(b));
    x.len() == y.len()
        && x.iter().zip(&y).all(|(p, q)| match (p, q) {
            (Ok(p), Ok(q)) => (p - q).abs() <= 0.15,
            (Err(p), Err(q)) => p == q,
            _ => false,
        })
}
