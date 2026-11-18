//! Incremental layout: keep the box tree between frames, rebuild boxes only
//! for changed elements, and translate clean boxes whose constraints did
//! not change instead of laying them out again.
//!
//! * **Box reuse.** The previous tree is dismantled lazily while the new
//!   one is built: an element that is neither dirty nor an ancestor of a
//!   dirty element (and whose style `Rc` is unchanged) takes its old box —
//!   with its whole subtree, text shaping (`pieces`) and intrinsic size
//!   caches — instead of building a new one.
//! * **Layout reuse.** Every block layout records its inputs
//!   ([`LayoutCache`]). When a reused box is laid out again with the same
//!   content width / definite height / margin-collapsing flags and no
//!   float interaction, its contents are translated to the new position
//!   instead of being laid out.

use crate::block::Layouter;
use crate::tree::{BoxKind, LayoutBox, LayoutTree};
use crate::LayoutContext;
use rabbithole_dom::NodeId;
use rabbithole_style::ComputedStyle;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

pub(crate) fn debug_timing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RH_DEBUG_INCREMENTAL").is_some())
}

/// Layout cache statistics (hits, misses, contents laid out by boxes
/// without a cache entry) since the last call.
pub fn take_cache_stats() -> (usize, usize, usize) {
    STATS.with(|s| {
        let v = s.get();
        s.set((0, 0, 0));
        v
    })
}

thread_local! {
    pub(crate) static BUILD_STATS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static STATS: std::cell::Cell<(usize, usize, usize)> = const { std::cell::Cell::new((0, 0, 0)) };
}

pub(crate) fn count(hit: bool, had_entry: bool) {
    STATS.with(|s| {
        let (h, m, n) = s.get();
        s.set(if hit { (h + 1, m, n) } else if had_entry { (h, m + 1, n) } else { (h, m, n + 1) });
    });
}

/// What changed since the previous layout.
#[derive(Debug, Default, Clone)]
pub struct LayoutDamage {
    /// Lay out from scratch.
    pub full: bool,
    /// Elements whose boxes must be rebuilt (style, children, text,
    /// attributes or image changed).
    pub dirty: HashSet<NodeId>,
}

/// Inputs and outputs of a box's last `layout_contents`.
#[derive(Debug, Clone)]
pub(crate) struct LayoutCache {
    /// A measurement cache cannot stand in for final layout and vice versa.
    pub grid_publication: bool,
    pub inner_w: f32,
    pub inner_h: Option<f32>,
    pub top_collapsible: bool,
    pub bottom_collapsible: bool,
    /// The box's content-box origin when its contents were laid out
    /// (before relative offsets / scroll offsets / later translations,
    /// which are tracked separately).
    pub content_h: f32,
    pub collapsed_bottom_margin: f32,
    /// Set-based collapsed bottom margins (most positive, most negative).
    pub bottom_margins: (f32, f32),
    /// Bottom of floats in the box's own BFC relative to the content top.
    pub float_extent: Option<f32>,
    /// Floats of the parent BFC reaching into the box, relative to its
    /// content origin (empty for BFC roots).
    pub float_env: Vec<crate::float::RelFloat>,
    /// Floats the contents placed into the parent BFC, relative to the
    /// content origin.
    pub escaped: Vec<crate::float::RelFloat>,
    /// Another `inner_h` proven to give the same contents and results (a
    /// flex item is measured without a definite height and then stretched
    /// to one; when its contents never depend on that height, both layouts
    /// are the same and both are served from this entry).
    pub alt_h: Option<Option<f32>>,
}

/// A fast non-cryptographic hasher for fingerprints (multiply-rotate).
#[derive(Default)]
struct FastHasher(u64);

impl std::hash::Hasher for FastHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(b as u64);
        }
    }
    fn write_u32(&mut self, v: u32) {
        self.write_u64(v as u64);
    }
    fn write_usize(&mut self, v: usize) {
        self.write_u64(v as u64);
    }
    fn write_u64(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

/// Geometry of everything laid out inside `b`, relative to its content
/// origin `(ox, oy)`: two layouts with the same fingerprint placed every
/// descendant box, line, fragment and marker at the same place.
pub(crate) fn contents_fingerprint(b: &LayoutBox, ox: f32, oy: f32) -> u64 {
    use std::hash::{Hash, Hasher};
    fn rect(h: &mut impl Hasher, r: &rabbithole_gfx::Rect, ox: f32, oy: f32) {
        for v in [r.x - ox, r.y - oy, r.w, r.h] {
            v.to_bits().hash(h);
        }
    }
    fn walk_contents(h: &mut impl Hasher, b: &LayoutBox, ox: f32, oy: f32) {
        b.lines.len().hash(h);
        for line in &b.lines {
            rect(h, &line.rect, ox, oy);
            (line.baseline - oy).to_bits().hash(h);
            line.fragments.len().hash(h);
            for f in &line.fragments {
                rect(h, &f.rect, ox, oy);
                (f.baseline - oy).to_bits().hash(h);
            }
        }
        if let Some(m) = &b.marker {
            (m.x - ox).to_bits().hash(h);
            (m.baseline - oy).to_bits().hash(h);
        }
        b.children.len().hash(h);
        for c in &b.children {
            rect(h, &c.rect, ox, oy);
            // A child's own contents: its memoized fingerprint (relative to
            // its unscrolled content origin), computed once per layout.
            let child_fp = |c: &LayoutBox| -> u64 {
                if let Some(v) = c.contents_fp.get() {
                    return v;
                }
                let mut ch = FastHasher::default();
                walk_contents(&mut ch, c, c.content_rect.x - c.scroll_offset.0, c.content_rect.y - c.scroll_offset.1);
                let v = ch.finish();
                c.contents_fp.set(Some(v));
                v
            };
            rect(h, &c.content_rect, ox, oy);
            (c.static_pos.0 - ox).to_bits().hash(h);
            (c.static_pos.1 - oy).to_bits().hash(h);
            if let Some(a) = c.static_align {
                for v in [a.size.0, a.size.1, a.factor.0, a.factor.1] {
                    v.to_bits().hash(h);
                }
            }
            c.scroll_width.to_bits().hash(h);
            c.scroll_height.to_bits().hash(h);
            c.collapsed_bottom_margin.to_bits().hash(h);
            c.scroll_offset.0.to_bits().hash(h);
            c.scroll_offset.1.to_bits().hash(h);
            match &c.wm_transform {
                // Contents in their own logical space: hash them as they are.
                Some(m) => {
                    for (i, v) in m.iter().enumerate() {
                        let v = if i == 4 { v - ox } else if i == 5 { v - oy } else { *v };
                        v.to_bits().hash(h);
                    }
                    walk_contents(h, c, 0.0, 0.0);
                }
                None => child_fp(c).hash(h),
            }
        }
    }
    // Measured from the box's own unscrolled content origin: the memo
    // applies (and is filled).
    let own = ox == b.content_rect.x - b.scroll_offset.0 && oy == b.content_rect.y - b.scroll_offset.1;
    if own {
        if let Some(v) = b.contents_fp.get() {
            return v;
        }
    }
    let mut h = FastHasher::default();
    walk_contents(&mut h, b, ox, oy);
    let v = h.finish();
    if own {
        b.contents_fp.set(Some(v));
    }
    v
}

/// Old boxes available for reuse while building the new tree.
pub(crate) struct Reuse {
    map: RefCell<HashMap<NodeId, LayoutBox>>,
    /// Dirty elements and all their ancestors.
    tainted: HashSet<NodeId>,
    /// The previous tree used counters: record counter state for reuse.
    pub(crate) uses_counters: bool,
}

impl Reuse {
    /// The old box of `node` if it can be reused as is; otherwise its
    /// children become available individually.
    pub(crate) fn take(&self, node: NodeId, style: &Rc<ComputedStyle>) -> Option<LayoutBox> {
        let mut map = self.map.borrow_mut();
        let old = map.remove(&node)?;
        // Vertical subtrees are rebuilt (their boxes are transposed and
        // wrapped after building).
        if !self.tainted.contains(&node) && !style.writing_mode.is_vertical() && old.built_from.as_ref().is_some_and(|s| Rc::ptr_eq(s, style)) {
            return Some(old);
        }
        dismantle(old, &mut map);
        None
    }

    /// A box taken with [`Reuse::take`] that turned out not reusable after
    /// all: make its children available.
    pub(crate) fn give_back(&self, b: LayoutBox) {
        dismantle(b, &mut self.map.borrow_mut());
    }

    /// Make the children of `node`'s old box available (the node itself is
    /// rebuilt).
    pub(crate) fn open(&self, node: NodeId) {
        let mut map = self.map.borrow_mut();
        if let Some(old) = map.remove(&node) {
            dismantle(old, &mut map);
        }
    }
}

/// Can `b` be keyed by its element for reuse?
fn reusable_kind(b: &LayoutBox) -> bool {
    b.node.is_some()
        && b.pseudo.is_none()
        && b.built_from.is_some()
        && matches!(b.kind, BoxKind::Block | BoxKind::InlineBlock | BoxKind::Image | BoxKind::Table | BoxKind::TableRow | BoxKind::TableCell)
}

fn dismantle(b: LayoutBox, map: &mut HashMap<NodeId, LayoutBox>) {
    for c in b.children {
        if reusable_kind(&c) {
            let n = c.node.expect("element box");
            map.insert(n, c);
        } else {
            dismantle(c, map);
        }
    }
}

/// Lay out the document reusing what `damage` says is still valid in
/// `prev`.
pub fn layout_incremental(prev: LayoutTree, ctx: &LayoutContext, damage: &LayoutDamage) -> LayoutTree {
    let doc = ctx.doc;
    let html = doc.html_element();
    let usable = !damage.full
        && prev.root.node.is_some()
        && prev.root.node == html
        && prev.viewport == (ctx.viewport_width.max(1.0), ctx.viewport_height.max(1.0));
    if !usable {
        return crate::layout(ctx);
    }
    if damage.dirty.is_empty() {
        return prev;
    }
    let mut tainted = HashSet::new();
    for &n in &damage.dirty {
        let mut cur = Some(n);
        while let Some(c) = cur {
            if !tainted.insert(c) {
                break;
            }
            cur = doc.box_parent(c);
        }
    }
    let mut map = HashMap::new();
    map.insert(html.expect("html"), prev.root);
    let reuse = Reuse { map: RefCell::new(map), tainted, uses_counters: prev.uses_counters };
    let mut layouter = Layouter::new(ctx);
    layouter.layout_document_with(Some(&reuse))
}
