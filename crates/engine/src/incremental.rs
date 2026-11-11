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
    /// Lay out from scratch next time.
    layout_full: bool,
    /// Elements whose boxes must be rebuilt.
    box_dirty: HashSet<NodeId>,
    /// Layout is out of date.
    layout_dirty: bool,
    /// The display list is out of date.
    dl_dirty: bool,
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
    /// [`side_sheet_signature`] at the last style flush.
    side_sheets: u64,
    /// Device pixel ratio images were last selected for.
    dpr_seen: Option<f32>,
    /// What the embedder's canvas shows (for damage repaints).
    canvas: CanvasSync,
    /// The display list the canvas shows when it is out of date.
    painted_dl: Option<rabbithole_layout::DisplayList>,
    pub stats: IncrementalStats,
}

impl IncrementalState {
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
        self.rects.borrow_mut().clear();
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
        self.rects.borrow_mut().clear();
        self.scroll_seen.insert(node, offset);
    }

    /// A new display list replaced `old`.
    pub(crate) fn display_list_replaced(&mut self, old: rabbithole_layout::DisplayList) {
        if let CanvasSync::Current(s) = self.canvas {
            self.painted_dl = Some(old);
            self.canvas = CanvasSync::Stale(s);
        }
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
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut keys: Vec<&NodeId> = doc.sheet_overrides.keys().collect();
    keys.sort();
    for k in keys {
        k.hash(&mut h);
        doc.sheet_overrides[k].hash(&mut h);
    }
    let mut keys: Vec<&NodeId> = doc.adopted_styles.keys().collect();
    keys.sort();
    for k in keys {
        k.hash(&mut h);
        doc.adopted_styles[k].hash(&mut h);
    }
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
    /// Bring `page.styles` up to date with the DOM, stylesheets, viewport
    /// and dynamic state, restyling only what changed. Layout is marked
    /// dirty (see [`Engine::flush_layout`]).
    pub fn flush_style(&self, page: &mut Page) {
        let log = if page.doc.is_tracking_mutations() {
            page.doc.take_mutations()
        } else {
            page.doc.track_mutations(true);
            MutationLog { entries: Vec::new(), overflow: true }
        };
        // Scripts may add/remove `<style>`/`<link>` elements or insert rules
        // (CSS-in-JS): re-collect the sheets when their sources changed.
        let scripted = page.script.host.is_some() || page.script.scripts_ran;
        // CSSOM edits and constructed / adopted sheets live outside the
        // node tree (not in the mutation log).
        let side = side_sheet_signature(&page.doc);
        let side_changed = std::mem::replace(&mut page.incr.side_sheets, side) != side;
        if scripted && (side_changed || log.overflow || log.entries.iter().any(|m| touches_sheets(&page.doc, m))) {
            let sig = stylesheet_signature(&page.doc);
            if sig != page.sheet_signature {
                page.sheet_signature = sig;
                page.sheets = self.loader.collect_stylesheets(&page.doc, &page.base_url);
                page.has_dynamic_rules = page.sheets.iter().any(sheet_has_dynamic_rules);
                page.incr.style = None;
            }
        }
        let env = media_env(page.viewport, self.scripting);
        let base_str = page.base_url.to_string();
        let state = page.state.clone();
        let opts = StyleOptions {
            state: &state,
            resolve_url: &|base, raw| resolve_url(base, raw),
            document_base: &base_str,
        };
        let incr = &mut page.incr;
        let fresh = !matches!(&incr.style, Some(se) if se.env_matches(&env));
        let mut anim_changes: Vec<crate::animations::Change> = Vec::new();
        let mut anim_full = fresh;
        if fresh {
            let mut se = StyleEngine::new(&page.sheets, &env);
            page.styles = se.full(&page.doc, &page.sheets, &opts);
            incr.style = Some(se);
            incr.stats.style_full += 1;
            incr.layout_full = true;
        } else {
            let se = incr.style.as_mut().expect("style engine");
            let had_changes = !log.is_empty() || se_state_differs(se, &state);
            if !had_changes {
                return;
            }
            let out = se.update(&page.doc, &mut page.styles, &log, &opts, &page.sheets);
            if debug_enabled() {
                eprintln!("[incr] style: full={} recomputed={} changed={} mutations={}", out.full, out.recomputed, out.changes.len(), log.entries.len());
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
                for c in &out.changes {
                    incr.box_dirty.insert(c.node);
                    let display_changed = match (&c.old, &c.new) {
                        (Some(a), Some(b)) => a.display != b.display || a.position != b.position || a.float != b.float,
                        _ => true,
                    };
                    if display_changed {
                        // Children's boxes depend on the parent's formatting
                        // context (blockification, table rows, ...).
                        incr.box_dirty.extend(page.doc.child_elements(c.node));
                    }
                }
            }
            damage_from_log(&page.doc, &log, &mut incr.box_dirty, &mut incr.layout_full);
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
        page.background = canvas_background(&page.doc, &page.styles);
        if fresh || log.overflow || log.entries.iter().any(|m| matches!(m, Mutation::Children { .. } | Mutation::Node { .. })) {
            rasterize_inline_svgs(&page.doc, &page.styles, &self.fonts, &mut page.images);
        }
        if anim_full || !anim_changes.is_empty() {
            self.animations_after_style(page, anim_changes, anim_full);
        }
    }

    /// Bring `page.layout` up to date (incrementally when possible). The
    /// display list is left stale (see [`Engine::flush_display_list`]).
    pub fn flush_layout(&self, page: &mut Page) {
        self.flush_style(page);
        let Page { doc, styles, images, background_images, viewport, scroll_offsets, page_scroll, layout, incr, .. } = page;
        // Inputs changed outside the style system: images, background
        // images, scroll offsets, viewport.
        let mut images_now = HashMap::with_capacity(images.len());
        for (n, img) in images.iter() {
            images_now.insert(*n, Arc::as_ptr(img) as *const u8 as usize);
        }
        if images_now != incr.images_seen {
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
        let mut bg_now: Vec<(String, usize)> = background_images.iter().map(|(k, v)| (k.clone(), Arc::as_ptr(v) as *const u8 as usize)).collect();
        bg_now.sort();
        if bg_now != incr.bg_seen {
            incr.bg_seen = bg_now;
            incr.layout_full = true;
            incr.layout_dirty = true;
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
            fonts: &self.fonts,
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
        incr.rects.borrow_mut().clear();
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
        self.flush_display_list(page);
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
        self.flush_display_list(page);
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
    /// unknown, repaint everything. Only meaningful for pages without
    /// fixed or sticky content (their position depends on the scroll).
    pub fn paint_damage_since(&self, page: &mut Page, origin: (f32, f32)) -> Option<Vec<rabbithole_gfx::Rect>> {
        self.flush_display_list(page);
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
            rabbithole_layout::paint_region(&page.display_list, canvas, &self.fonts, scroll.0, scroll.1, r);
        }
    }
}

fn se_state_differs(se: &StyleEngine, state: &rabbithole_css::MatchState) -> bool {
    se.state() != state
}

/// Layout damage implied by DOM mutations (independent of style changes):
/// changed text, child lists and attributes (replaced elements, list
/// markers, form controls...).
fn damage_from_log(doc: &Document, log: &MutationLog, dirty: &mut HashSet<NodeId>, full: &mut bool) {
    if log.overflow {
        *full = true;
        return;
    }
    let add_with_children = |n: NodeId, dirty: &mut HashSet<NodeId>| {
        dirty.insert(n);
        dirty.extend(doc.child_elements(n));
    };
    for m in &log.entries {
        match m {
            Mutation::Attr { node, name, .. } => {
                add_with_children(*node, dirty);
                if name == "value" {
                    if let Some(p) = doc.parent(*node) {
                        add_with_children(p, dirty);
                    }
                }
            }
            Mutation::Children { parent, child } => {
                if doc.element(*parent).is_none() {
                    *full = true;
                    continue;
                }
                // List markers count siblings.
                add_with_children(*parent, dirty);
                dirty.insert(*child);
            }
            Mutation::Text { node } => {
                if let Some(p) = doc.parent(*node) {
                    dirty.insert(p);
                }
            }
            Mutation::Node { node } => {
                add_with_children(*node, dirty);
                if let Some(p) = doc.parent(*node) {
                    dirty.insert(p);
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
        let full_styles = StyleEngine::new(&page.sheets, &env).full(&page.doc, &page.sheets, &opts);
        check_styles(&page.doc, &page.styles, &full_styles)?;
        let ctx = LayoutContext {
            doc: &page.doc,
            styles: &page.styles,
            fonts: &self.fonts,
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
