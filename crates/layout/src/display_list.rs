//! Display list generation (document coordinates) with CSS 2.1 Appendix E
//! stacking order, transforms, opacity layers, fixed positioning and the
//! sticky offset applied at paint time.

use std::sync::Arc;
use crate::background::{self, box_radii, BoxGeometry};
use crate::tree::{BoxKind, DecorationInfo, Flow, FragmentKind, LayoutBox, LayoutTree};
use crate::LayoutContext;
use rabbithole_dom::NodeId;
use rabbithole_gfx::{Color, FontId, Image, Matrix, Paint, Point, Radii, Rect};
use rabbithole_style::{BorderStyle, ObjectFit, PointerEvents, Shadow, TextDecorationStyle, Visibility};

#[derive(Debug, Clone)]
pub enum DisplayItem {
    /// Solid fill with a uniform corner radius (fast path).
    Rect {
        rect: Rect,
        color: Color,
        radius: f32,
    },
    /// Fill with per-corner radii and any paint (solid / gradients).
    RectRadii {
        rect: Rect,
        radii: Radii,
        paint: Paint,
    },
    Border {
        rect: Rect,
        widths: [f32; 4],
        colors: [Color; 4],
        styles: [BorderStyle; 4],
        radii: Radii,
    },
    BoxShadow {
        rect: Rect,
        radii: Radii,
        offset: (f32, f32),
        blur: f32,
        spread: f32,
        color: Color,
        inset: bool,
    },
    Text {
        /// Left edge of the run.
        x: f32,
        /// Baseline.
        y: f32,
        font: FontId,
        size: f32,
        color: Color,
        text: String,
        letter_spacing: f32,
        shadows: Vec<Shadow>,
        small_caps: bool,
        oblique: bool,
    },
    /// An image (or the `src` sub-rectangle of it) scaled into `rect`.
    Image {
        rect: Rect,
        src: Rect,
        image: Arc<Image>,
        opacity: f32,
        /// Nearest-neighbour scaling (`image-rendering: pixelated`).
        pixelated: bool,
    },
    /// An image tiled over `area` (CSS background-repeat).
    ImageTiled {
        image: Arc<Image>,
        area: Rect,
        origin: Point,
        tile: (f32, f32),
        repeat_x: bool,
        repeat_y: bool,
        opacity: f32,
    },
    /// Horizontal/vertical line (text decorations, hr).
    Line {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        width: f32,
        color: Color,
        style: TextDecorationStyle,
    },
    PushClip(Rect),
    PushClipRounded(Rect, Radii),
    PopClip,
    PushTransform(Matrix),
    PopTransform,
    /// Offscreen layer for `opacity`; `PopLayer` composites it.
    PushLayer(Rect),
    PopLayer(f32),
    /// Mask of the layer closed by the next `PopLayer` (`mask-image`):
    /// the alpha of these items scales the layer's contribution.
    LayerMask(Vec<DisplayItem>),
    /// Blend mode (`mix-blend-mode`) of the layer closed by the next
    /// `PopLayer`.
    LayerBlend(u8),
    /// `filter`: `items` (the element's painting) rendered offscreen
    /// within `bounds` and filtered before being composited.
    Filtered {
        bounds: Rect,
        filters: Vec<rabbithole_style::FilterOp>,
        items: Vec<DisplayItem>,
    },
    /// `backdrop-filter`: what is already painted inside `rect` (clipped to
    /// `radii`) is filtered in place.
    BackdropFilter {
        rect: Rect,
        radii: Radii,
        filters: Vec<rabbithole_style::FilterOp>,
    },
    /// Items inside are in viewport coordinates (not scrolled).
    PushFixed,
    PopFixed,
}

#[derive(Debug, Clone, Default)]
pub struct DisplayList {
    pub items: Vec<DisplayItem>,
    /// Hit-test regions recorded in paint order (what is painted on top is
    /// hit first), so clicks land on what the user sees.
    pub hits: HitList,
    /// Scroll-dependent translations of `position: sticky` boxes, patched in
    /// place by [`DisplayList::update_page_scroll`] instead of rebuilding.
    pub sticky: Vec<StickyPatch>,
    /// Bounds of `position: fixed` boxes (viewport coordinates): what must
    /// be repainted when the page scrolls under them.
    pub fixed_bounds: Vec<Rect>,
}

/// Where a sticky box's displacement lives in the list.
#[derive(Debug, Clone, Copy)]
pub struct StickyPatch {
    /// Index of its `PushTransform` item.
    pub item: usize,
    /// Index of its hit-test transform scope.
    pub scope: u32,
    pub margin_rect: Rect,
    pub cb: Rect,
    pub inset: rabbithole_style::Edges<rabbithole_style::Length>,
    /// Painted bounds of the sticky subtree (document coordinates, before
    /// the sticky displacement).
    pub bounds: Rect,
    /// The scrollport of an inner scroll container (the box does not
    /// follow the page scroll); `None` = the viewport.
    pub port: Option<Rect>,
}

impl StickyPatch {
    /// Displacement for a scrollport (document coordinates).
    pub fn offset(&self, port: Rect) -> (f32, f32) {
        crate::tree::sticky_offset_for(&self.inset, self.margin_rect, port, self.cb)
    }
}

impl DisplayList {
    /// Re-position sticky boxes for a new page scroll position (the rest of
    /// the list is in document coordinates and does not change).
    pub fn update_page_scroll(&mut self, page_scroll: (f32, f32), viewport: (f32, f32)) {
        let port = Rect::new(page_scroll.0, page_scroll.1, viewport.0, viewport.1);
        for p in &self.sticky {
            if p.port.is_some() {
                continue;
            }
            let (dx, dy) = p.offset(port);
            self.items[p.item] = DisplayItem::PushTransform([1.0, 0.0, 0.0, 1.0, dx, dy]);
            self.hits.scopes[p.scope as usize].scope = HitScope::Transform(Some([1.0, 0.0, 0.0, 1.0, -dx, -dy]));
        }
    }
}

/// A clip / transform / fixed-layer scope enclosing hit regions.
#[derive(Debug, Clone, Copy)]
pub enum HitScope {
    /// Points outside the rectangle (current local coordinates) miss.
    Clip(Rect),
    /// Maps parent coordinates into local ones (the *inverse* of the
    /// paint transform); `None` for a singular transform (nothing hits).
    Transform(Option<Matrix>),
    /// Contents are in viewport coordinates (`position: fixed`).
    Fixed,
}

#[derive(Debug, Clone, Copy)]
pub struct HitScopeNode {
    pub parent: Option<u32>,
    pub scope: HitScope,
}

/// A hit-testable rectangle of an element.
#[derive(Debug, Clone, Copy)]
pub struct HitEntry {
    pub rect: Rect,
    pub node: NodeId,
    pub scope: Option<u32>,
}

/// Hit regions in paint order plus their clip/transform scopes.
#[derive(Debug, Clone, Default)]
pub struct HitList {
    pub scopes: Vec<HitScopeNode>,
    pub entries: Vec<HitEntry>,
}

impl HitList {
    /// Topmost element at viewport point `(vx, vy)` with the page scrolled
    /// by `page_scroll` (document = viewport + scroll, except inside the
    /// fixed layer).
    pub fn hit_test(&self, vx: f32, vy: f32, page_scroll: (f32, f32)) -> Option<NodeId> {
        self.hit_test_all(vx, vy, page_scroll).into_iter().next()
    }

    /// All elements under the point, topmost first (like
    /// `document.elementsFromPoint`, without ancestors that were not hit).
    pub fn hit_test_all(&self, vx: f32, vy: f32, page_scroll: (f32, f32)) -> Vec<NodeId> {
        // Memoized point of each scope: None = not computed, Some(None) =
        // clipped out, Some(Some(p)) = local point.
        let mut memo: Vec<Option<Option<(f32, f32)>>> = vec![None; self.scopes.len()];
        let doc = (vx + page_scroll.0, vy + page_scroll.1);
        let mut out = Vec::new();
        for e in self.entries.iter().rev() {
            let p = match e.scope {
                None => Some(doc),
                Some(s) => self.scope_point(s, doc, (vx, vy), &mut memo),
            };
            if let Some((x, y)) = p {
                if e.rect.contains(x, y) && !out.contains(&e.node) {
                    out.push(e.node);
                }
            }
        }
        out
    }

    fn scope_point(&self, s: u32, doc: (f32, f32), view: (f32, f32), memo: &mut Vec<Option<Option<(f32, f32)>>>) -> Option<(f32, f32)> {
        if let Some(p) = memo[s as usize] {
            return p;
        }
        let node = self.scopes[s as usize];
        let base = match node.parent {
            None => Some(doc),
            Some(p) => self.scope_point(p, doc, view, memo),
        };
        let r = base.and_then(|(x, y)| match node.scope {
            HitScope::Fixed => Some(view),
            HitScope::Clip(r) => {
                if r.contains(x, y) {
                    Some((x, y))
                } else {
                    None
                }
            }
            HitScope::Transform(m) => m.map(|m| crate::tree::map_point(m, x, y)),
        });
        memo[s as usize] = Some(r);
        r
    }
}

impl DisplayList {
    fn push(&mut self, item: DisplayItem) {
        self.items.push(item);
    }
}

pub fn build(tree: &LayoutTree, ctx: &LayoutContext) -> DisplayList {
    let mut list = DisplayList::default();
    let bounds = Rect::new(0.0, 0.0, tree.width, tree.height);
    // Canvas background: the root's background propagates to the viewport,
    // falling back to the body's.
    let root = &tree.root;
    let mut bg = root.style.background_color;
    let mut root_bg_painted = false;
    if bg.is_transparent() {
        if let Some(body) = root.children.iter().find(|c| c.is_in_flow()) {
            bg = body.style.background_color;
        }
    } else {
        root_bg_painted = true;
    }
    // The root's opacity also fades the canvas background over the white
    // page backdrop (CSS Compositing 1 §4.1: the root group blends with
    // it).
    let root_opacity = root.style.opacity.clamp(0.0, 1.0);
    if root_opacity < 1.0 && !bg.is_transparent() {
        let a = bg.a as f32 / 255.0 * root_opacity;
        let mix = |c: u8| (255.0 * (1.0 - a) + c as f32 * a + 0.5) as u8;
        bg = Color::rgb(mix(bg.r), mix(bg.g), mix(bg.b));
        root_bg_painted = true;
    }
    if !bg.is_transparent() {
        list.push(DisplayItem::Rect {
            rect: bounds,
            color: bg,
            radius: 0.0,
        });
    }
    // The top layer (modal dialogs): painted after everything else, in
    // viewport coordinates, each over its `::backdrop`.
    let top_layer: Vec<(NodeId, &LayoutBox)> = ctx
        .doc
        .top_layer
        .iter()
        .filter(|&&n| ctx.doc.is_connected(n))
        .filter_map(|&n| tree.box_of(ctx.doc, n).filter(|b| b.node == Some(n)).map(|b| (n, b)))
        .collect();
    let mut p = Painter {
        list: &mut list,
        ctx,
        bounds,
        root,
        skip_root_bg: root_bg_painted,
        sticky_scrollport: Rect::new(ctx.page_scroll.0, ctx.page_scroll.1, ctx.viewport_width, ctx.viewport_height),
        nested_port: false,
        flow_cb: Rect::default(),
        hits: HitList::default(),
        scope: None,
        hoisted: top_layer.iter().map(|(_, b)| *b as *const LayoutBox as usize).collect(),
        flipped: false,
        wm_bg: None,
    };
    p.paint_stacking_context(root, bounds);
    for (node, b) in top_layer {
        p.paint_top_layer(node, b);
    }
    let hits = std::mem::take(&mut p.hits);
    list.hits = hits;
    list
}

struct Painter<'l, 't, 'a> {
    list: &'l mut DisplayList,
    ctx: &'l LayoutContext<'a>,
    bounds: Rect,
    root: &'t LayoutBox,
    skip_root_bg: bool,
    sticky_scrollport: Rect,
    /// The sticky scrollport is an inner scroll container's.
    nested_port: bool,
    /// While collecting deferred boxes: the containing block content rect
    /// of in-flow boxes inside an anonymous block (its parent's: anonymous
    /// blocks form no containing block).
    flow_cb: Rect,
    /// Hit regions recorded while painting.
    hits: HitList,
    /// Current hit scope (clip / transform / fixed layer).
    scope: Option<u32>,
    /// Boxes (by address) already taken into an outer stacking context
    /// from inside a positioned `z-index: auto` ancestor (see
    /// [`Painter::collect_hoisted`]); that ancestor skips them.
    hoisted: std::collections::HashSet<usize>,
    /// Inside an odd number of reflections (`vertical-lr` subtrees): text
    /// is flipped back locally so glyphs read sideways, not mirrored.
    flipped: bool,
    /// The box a vertical-mode wrapper turns, with the wrapper's physical
    /// rect, mapping and background layers: painted in physical space.
    wm_bg: Option<(usize, Rect, Matrix, std::rc::Rc<rabbithole_style::ComputedStyle>)>,
}

/// Painting phases of the in-flow content of a stacking context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Backgrounds,
    Floats,
    Content,
}

/// A positioned / stacking descendant deferred to its stacking context.
struct Deferred<'t> {
    z: i32,
    order: usize,
    b: &'t LayoutBox,
    /// Containing-block content rect for sticky offsets.
    cb: Rect,
    /// Overflow clips (padding boxes) of ancestors between the stacking
    /// context root and this box that apply to it.
    clips: Vec<Rect>,
}

impl<'l, 't, 'a> Painter<'l, 't, 'a> {
    fn visible(&self, r: &Rect) -> bool {
        !r.intersect(&self.bounds).is_empty() || r.is_empty()
    }

    /// Enter a hit scope; returns the scope to restore.
    fn push_scope(&mut self, scope: HitScope) -> Option<u32> {
        let saved = self.scope;
        self.hits.scopes.push(HitScopeNode { parent: saved, scope });
        self.scope = Some((self.hits.scopes.len() - 1) as u32);
        saved
    }

    /// Record a hit region for `node` in the current scope.
    fn record_hit(&mut self, rect: Rect, node: Option<NodeId>) {
        if let Some(node) = node {
            if rect.w > 0.0 && rect.h > 0.0 {
                self.hits.entries.push(HitEntry { rect, node, scope: self.scope });
            }
        }
    }

    /// Overflow clipping in local coordinates, bounded by the painting area.
    fn overflow_clip_rect(&self, b: &LayoutBox) -> Rect {
        let pr = b.padding_rect();
        let (clip_x, clip_y) = b.overflow_clip_axes();
        // The unconstrained axis needs only cover the current painting area.
        // Bounds are already mapped back through any ancestor transforms.
        Rect::new(
            if clip_x { pr.x } else { self.bounds.x },
            if clip_y { pr.y } else { self.bounds.y },
            if clip_x { pr.w } else { self.bounds.w },
            if clip_y { pr.h } else { self.bounds.h },
        )
    }

    /// Push an overflow clip (paint + hit scope); returns the scope to
    /// restore with `pop_clip`.
    fn push_clip(&mut self, b: &LayoutBox) -> Option<u32> {
        let pr = self.overflow_clip_rect(b);
        let radii = background::inner_radii(box_radii(b), &b.style.border_width);
        // CSS Overflow 3: a clip/visible pair does not round the content clip.
        if b.overflow_clip_axes() == (true, true) && radii.iter().any(|(h, v)| *h > 0.0 && *v > 0.0) {
            self.list.push(DisplayItem::PushClipRounded(pr, radii));
        } else {
            self.list.push(DisplayItem::PushClip(pr));
        }
        self.push_scope(HitScope::Clip(pr))
    }

    fn pop_clip(&mut self, saved: Option<u32>) {
        self.list.push(DisplayItem::PopClip);
        self.scope = saved;
    }

    // ------------------------------------------------------ stacking contexts

    /// Paint `root` as a stacking context (CSS 2.1 Appendix E).
    fn paint_stacking_context(&mut self, root: &'t LayoutBox, cb: Rect) {
        if root.style.visibility == Visibility::Hidden && root.children.is_empty() && root.lines.is_empty() {
            return;
        }
        let saved_scope = self.scope;
        // Culling (`visible`) compares the boxes' own (pre-transform)
        // coordinates with the painted area: inside the wrappers below, the
        // area is mapped back into the subtree's coordinates (a scaled-down
        // container shows children laid out far outside the viewport).
        let saved_bounds = self.bounds;
        // Sticky displacement, transform and opacity wrappers.
        let (sdx, sdy) = root.sticky_offset(self.sticky_scrollport, cb);
        let start = self.list.items.len();
        let sticky_start = self.list.sticky.len();
        let mut pushed_transform = false;
        let sticky = root.style.position == rabbithole_style::Position::Sticky;
        if sdx != 0.0 || sdy != 0.0 || sticky {
            // Sticky boxes always get the wrapper so scrolling can patch it.
            self.list.push(DisplayItem::PushTransform([1.0, 0.0, 0.0, 1.0, sdx, sdy]));
            self.push_scope(HitScope::Transform(Some([1.0, 0.0, 0.0, 1.0, -sdx, -sdy])));
            pushed_transform = true;
            self.bounds = Rect::new(self.bounds.x - sdx, self.bounds.y - sdy, self.bounds.w, self.bounds.h);
            if sticky {
                self.list.sticky.push(StickyPatch {
                    item: start,
                    scope: (self.hits.scopes.len() - 1) as u32,
                    margin_rect: root.margin_rect(),
                    cb,
                    inset: root.style.inset,
                    bounds: root.subtree_bounds(),
                    port: self.nested_port.then_some(self.sticky_scrollport),
                });
            }
        }
        let mut pushed_matrix = false;
        let saved_flip = self.flipped;
        let saved_wm_bg = self.wm_bg.take();
        if let (Some(m), Some(bg), Some(inner)) = (root.transform_matrix().filter(|_| root.wm_transform.is_some()), root.wm_background.clone(), root.children.first()) {
            self.wm_bg = Some((inner as *const LayoutBox as usize, root.rect, m, bg));
        }
        if let Some(m) = root.wm_transform {
            if m[0] * m[3] - m[1] * m[2] < 0.0 {
                self.flipped = !self.flipped;
            }
        }
        if let Some(m) = root.transform_matrix() {
            self.list.push(DisplayItem::PushTransform(m));
            let inverse = crate::tree::invert_matrix(m);
            self.push_scope(HitScope::Transform(inverse));
            pushed_matrix = true;
            self.bounds = match inverse {
                Some(inv) => rabbithole_gfx::path::transform_rect_bounds(&inv, self.bounds),
                // Flattened to nothing (scale(0)): nothing shows.
                None => Rect::new(self.bounds.x, self.bounds.y, 0.0, 0.0),
            };
        }
        let opacity = root.style.opacity.clamp(0.0, 1.0);
        let layer = opacity < 1.0 || root.style.mix_blend_mode != 0;
        if layer && opacity <= 0.0 {
            // Invisible but still hit-testable (transparent overlays,
            // styled file inputs, ...): record hits, drop the paint.
            self.paint_stacking_context_inner(root);
            self.list.items.truncate(start);
            self.list.sticky.truncate(sticky_start);
            self.scope = saved_scope;
            self.bounds = saved_bounds;
            self.flipped = saved_flip;
            self.wm_bg = saved_wm_bg;
            return;
        }
        if !root.style.backdrop_filters.is_empty() && root.style.visibility != Visibility::Hidden {
            let r = root.rect;
            self.list.push(DisplayItem::BackdropFilter { rect: r, radii: root.style.resolved_radii(r.w, r.h), filters: root.style.backdrop_filters.clone() });
        }
        let mask = self.mask_items(root);
        let layer = layer || mask.is_some();
        if layer {
            self.list.push(DisplayItem::PushLayer(root.subtree_bounds()));
        }
        let filter_start = (self.list.items.len(), self.list.sticky.len());
        self.paint_stacking_context_inner(root);
        // `filter`: the painted items move into a filtered group (not
        // around sticky descendants, whose items are patched in place).
        if !root.style.filters.is_empty() && self.list.sticky.len() == filter_start.1 {
            let items: Vec<DisplayItem> = self.list.items.drain(filter_start.0..).collect();
            let e = crate::filter::spread(&root.style.filters);
            let b = root.subtree_bounds();
            let bounds = Rect::new(b.x - e, b.y - e, b.w + 2.0 * e, b.h + 2.0 * e);
            self.list.push(DisplayItem::Filtered { bounds, filters: root.style.filters.clone(), items });
        }
        if let Some(items) = mask {
            self.list.push(DisplayItem::LayerMask(items));
        }
        if root.style.mix_blend_mode != 0 {
            self.list.push(DisplayItem::LayerBlend(root.style.mix_blend_mode));
        }
        if layer {
            self.list.push(DisplayItem::PopLayer(opacity));
        }
        self.pop_wrappers(pushed_matrix, pushed_transform);
        self.scope = saved_scope;
        self.bounds = saved_bounds;
        self.flipped = saved_flip;
        self.wm_bg = saved_wm_bg;
    }

    /// The mask of a box with `mask-image` layers: its mask layers painted
    /// like backgrounds (only their alpha matters). An image that has not
    /// loaded yet masks everything out, like in browsers.
    fn mask_items(&self, b: &LayoutBox) -> Option<Vec<DisplayItem>> {
        // `none` layers mask nothing; no image at all means no mask.
        if !b.style.mask_layers.iter().any(|l| !matches!(l.image, rabbithole_style::BackgroundImage::None)) || b.rect.is_empty() {
            return None;
        }
        let mut style = (*b.style).clone();
        style.background_layers = b.style.mask_layers.clone();
        style.background_color = Color::TRANSPARENT;
        style.background_image = None;
        let geom = BoxGeometry { border: b.rect, padding: b.padding_rect(), content: b.content_rect, radii: [(0.0, 0.0); 4], border_width: style.border_width };
        let vp = Rect::new(self.ctx.page_scroll.0, self.ctx.page_scroll.1, self.ctx.viewport_width, self.ctx.viewport_height);
        let mut tmp = DisplayList::default();
        background::paint_backgrounds(&mut tmp, &geom, &style, self.ctx.background_images, vp);
        Some(tmp.items)
    }

    /// The contents of a stacking context in CSS 2.1 Appendix E order
    /// (wrappers already pushed).
    fn paint_stacking_context_inner(&mut self, root: &'t LayoutBox) {

        // Collect deferred descendants. The root's own overflow clip
        // applies to them, except to absolutely positioned ones whose
        // containing block lies outside a non-positioned root.
        let mut deferred: Vec<Deferred<'t>> = Vec::new();
        let mut order = 0usize;
        let (flow_clips, abs_clips) = if root.clips && !root.rect.is_empty() && !std::ptr::eq(root, self.root) {
            let pr = self.overflow_clip_rect(root);
            let abs = if root.is_positioned() || root.transform_matrix().is_some() { vec![pr] } else { Vec::new() };
            (vec![pr], abs)
        } else {
            (Vec::new(), Vec::new())
        };
        self.collect_deferred(root, root.content_rect, &mut deferred, &mut order, &flow_clips, &abs_clips);
        let mut negative: Vec<&Deferred<'t>> = deferred.iter().filter(|d| d.z < 0).collect();
        let mut zero: Vec<&Deferred<'t>> = deferred.iter().filter(|d| d.z == 0).collect();
        let mut positive: Vec<&Deferred<'t>> = deferred.iter().filter(|d| d.z > 0).collect();
        negative.sort_by_key(|d| (d.z, d.order));
        zero.sort_by_key(|d| d.order);
        positive.sort_by_key(|d| (d.z, d.order));

        // 1. Background and borders of the root.
        let skip_bg = self.skip_root_bg && std::ptr::eq(root, self.root);
        if root.kind != BoxKind::Text {
            self.paint_background_and_border(root, skip_bg);
        }
        // 2. Negative z-index stacking contexts.
        for d in negative {
            self.paint_deferred(d);
        }
        // 3–5. In-flow content (block backgrounds, floats, inline content).
        self.paint_contents(root);
        // 6. Positioned descendants with z-index auto / 0, in tree order.
        for d in zero {
            self.paint_deferred(d);
        }
        // 7. Positive z-index stacking contexts.
        for d in positive {
            self.paint_deferred(d);
        }
        // Outline is painted above everything of the element.
        self.paint_outline(root);
    }

    /// Paint a deferred box inside the overflow clips of its ancestors.
    fn paint_deferred(&mut self, d: &Deferred<'t>) {
        let saved = self.scope;
        if d.b.fixed && d.b.flow == Flow::Absolute {
            // `position: fixed`: painted in its stacking context's order,
            // in viewport coordinates (not scrolled, not clipped by
            // scrolling ancestors).
            self.list.push(DisplayItem::PushFixed);
            self.push_scope(HitScope::Fixed);
            let vp = Rect::new(0.0, 0.0, self.ctx.viewport_width, self.ctx.viewport_height);
            self.list.fixed_bounds.push(d.b.subtree_bounds());
            self.paint_stacking_context(d.b, vp);
            self.list.push(DisplayItem::PopFixed);
            self.scope = saved;
            return;
        }
        for r in &d.clips {
            self.list.push(DisplayItem::PushClip(*r));
            self.push_scope(HitScope::Clip(*r));
        }
        // A sticky box inside a scroll container sticks to its scrollport
        // (the innermost overflow clip).
        let saved_port = (self.sticky_scrollport, self.nested_port);
        if let Some(r) = d.clips.last() {
            self.sticky_scrollport = *r;
            self.nested_port = true;
        }
        self.paint_stacking_context(d.b, d.cb);
        (self.sticky_scrollport, self.nested_port) = saved_port;
        for _ in &d.clips {
            self.list.push(DisplayItem::PopClip);
        }
        self.scope = saved;
    }

    /// A top-layer element: its `::backdrop` over the whole viewport (it
    /// takes the clicks outside the element: they target the element, as
    /// in browsers), then the element as a stacking context of its own.
    fn paint_top_layer(&mut self, node: NodeId, b: &'t LayoutBox) {
        let saved = self.scope;
        let saved_bounds = self.bounds;
        self.list.push(DisplayItem::PushFixed);
        self.push_scope(HitScope::Fixed);
        let vp = Rect::new(0.0, 0.0, self.ctx.viewport_width, self.ctx.viewport_height);
        self.bounds = vp;
        if let Some(bs) = crate::pseudo_style(self.ctx.styles, node, rabbithole_css::PseudoElement::Marker) {
            if !bs.background_color.is_transparent() && bs.visibility != Visibility::Hidden {
                self.list.push(DisplayItem::Rect { rect: vp, color: bs.background_color.scale_alpha(bs.opacity.clamp(0.0, 1.0)), radius: 0.0 });
            }
        }
        self.record_hit(vp, Some(node));
        self.list.fixed_bounds.push(vp);
        if b.fixed && b.flow == Flow::Absolute {
            self.paint_stacking_context(b, vp);
        } else {
            // Not fixed (author styles): painted where it is, still on top.
            self.list.push(DisplayItem::PopFixed);
            self.paint_stacking_context(b, saved_bounds);
            self.list.push(DisplayItem::PushFixed);
        }
        self.list.push(DisplayItem::PopFixed);
        self.bounds = saved_bounds;
        self.scope = saved;
    }

    fn pop_wrappers(&mut self, matrix: bool, translate: bool) {
        if matrix {
            self.list.push(DisplayItem::PopTransform);
        }
        if translate {
            self.list.push(DisplayItem::PopTransform);
        }
    }

    /// Walk `b`'s subtree collecting positioned boxes and stacking contexts
    /// that belong to the current stacking context. Does not enter boxes
    /// that are themselves deferred.
    fn collect_deferred(&mut self, b: &'t LayoutBox, cb: Rect, out: &mut Vec<Deferred<'t>>, order: &mut usize, flow_clips: &[Rect], abs_clips: &[Rect]) {
        let cb_for_children = if b.is_positioned() { b.padding_rect() } else { cb };
        let flow_cb = if b.kind == BoxKind::AnonymousBlock { self.flow_cb } else { b.content_rect };
        let saved_flow_cb = std::mem::replace(&mut self.flow_cb, flow_cb);
        for c in &b.children {
            if self.hoisted.contains(&(c as *const LayoutBox as usize)) {
                continue;
            }
            if c.fixed && c.flow == Flow::Absolute {
                *order += 1;
                out.push(Deferred { z: c.style.z_index.unwrap_or(0), order: *order, b: c, cb: self.bounds, clips: Vec::new() });
                continue;
            }
            let deferred = c.is_positioned() || c.is_stacking_context() || c.flow == Flow::Absolute;
            if deferred {
                *order += 1;
                let cb_rect = if c.flow == Flow::Absolute { cb_for_children } else { flow_cb };
                let clips = if c.flow == Flow::Absolute { abs_clips.to_vec() } else { flow_clips.to_vec() };
                out.push(Deferred {
                    z: if c.is_stacking_context() || c.is_positioned() { c.style.z_index.unwrap_or(0) } else { 0 },
                    order: *order,
                    b: c,
                    cb: cb_rect,
                    clips: clips.clone(),
                });
                if Self::is_pseudo_stacking(c) {
                    let mut inner = clips;
                    if c.clips && !c.rect.is_empty() {
                        inner.push(self.overflow_clip_rect(c));
                    }
                    self.collect_hoisted(c, c.padding_rect(), out, order, &inner, &inner);
                }
            } else if c.clips && !c.rect.is_empty() {
                // A non-positioned clipping box clips in-flow (relative,
                // sticky, stacking) descendants but not absolutely
                // positioned ones (their containing block is outside it).
                let mut fc = flow_clips.to_vec();
                fc.push(self.overflow_clip_rect(c));
                self.collect_deferred(c, cb_for_children, out, order, &fc, abs_clips);
            } else {
                self.collect_deferred(c, cb_for_children, out, order, flow_clips, abs_clips);
            }
            self.flow_cb = flow_cb;
        }
        self.flow_cb = saved_flow_cb;
    }

    /// A positioned box with `z-index: auto`: painted like a stacking
    /// context, but it is not one (CSS 2.1 Appendix E).
    fn is_pseudo_stacking(c: &LayoutBox) -> bool {
        c.is_positioned() && !c.is_stacking_context() && !(c.fixed && c.flow == Flow::Absolute) && c.style.position != rabbithole_style::Position::Sticky
    }

    /// Inside a positioned `z-index: auto` box `b`: collect the descendant
    /// stacking contexts with a non-zero z-index, which belong to the
    /// enclosing stacking context (a `z-index: 1` flex item inside a
    /// `position: relative` wrapper must paint, and take clicks, above a
    /// later absolutely positioned sibling of that wrapper). Descendants
    /// with z-index auto / 0 keep painting inside `b`: in tree order that
    /// is the same place.
    fn collect_hoisted(&mut self, b: &'t LayoutBox, cb: Rect, out: &mut Vec<Deferred<'t>>, order: &mut usize, flow_clips: &[Rect], abs_clips: &[Rect]) {
        let cb_for_children = if b.is_positioned() { b.padding_rect() } else { cb };
        for c in &b.children {
            if self.hoisted.contains(&(c as *const LayoutBox as usize)) {
                // Already taken by an outer stacking context.
                continue;
            }
            if c.fixed && c.flow == Flow::Absolute {
                if c.style.z_index.unwrap_or(0) != 0 {
                    *order += 1;
                    self.hoisted.insert(c as *const LayoutBox as usize);
                    out.push(Deferred { z: c.style.z_index.unwrap_or(0), order: *order, b: c, cb: self.bounds, clips: Vec::new() });
                }
                continue;
            }
            let deferred = Self::is_deferred(c);
            let clips = if c.flow == Flow::Absolute { abs_clips } else { flow_clips };
            if deferred && c.is_stacking_context() {
                let z = c.style.z_index.unwrap_or(0);
                if z != 0 {
                    *order += 1;
                    self.hoisted.insert(c as *const LayoutBox as usize);
                    let cb_rect = if c.flow == Flow::Absolute { cb_for_children } else { b.content_rect };
                    out.push(Deferred { z, order: *order, b: c, cb: cb_rect, clips: clips.to_vec() });
                }
            } else if deferred {
                if Self::is_pseudo_stacking(c) {
                    let mut inner = clips.to_vec();
                    if c.clips && !c.rect.is_empty() {
                        inner.push(self.overflow_clip_rect(c));
                    }
                    self.collect_hoisted(c, c.padding_rect(), out, order, &inner, &inner);
                }
            } else if c.clips && !c.rect.is_empty() {
                let mut fc = flow_clips.to_vec();
                fc.push(self.overflow_clip_rect(c));
                self.collect_hoisted(c, cb_for_children, out, order, &fc, abs_clips);
            } else {
                self.collect_hoisted(c, cb_for_children, out, order, flow_clips, abs_clips);
            }
        }
    }

    fn is_deferred(c: &LayoutBox) -> bool {
        c.is_positioned() || c.is_stacking_context() || c.flow == Flow::Absolute
    }

    /// Paint the in-flow contents of `b` (not its own background) in tree
    /// order, skipping deferred boxes.
    fn paint_contents(&mut self, b: &'t LayoutBox) {
        // CSS 2.1 Appendix E, steps 4-7 for the in-flow subtree: first the
        // backgrounds of all block-level descendants, then floats, then
        // inline content. (A later block's background must not cover an
        // earlier float's content, e.g. a floated toolbar of links followed
        // by a full-width sibling.)
        self.paint_phase(b, Phase::Backgrounds);
        self.paint_phase(b, Phase::Floats);
        self.paint_phase(b, Phase::Content);
    }

    /// One painting phase over `b`'s in-flow, non-deferred subtree (`b`'s
    /// own background is painted by the caller).
    fn paint_phase(&mut self, b: &'t LayoutBox, phase: Phase) {
        if phase == Phase::Content {
            if let Some(m) = &b.marker {
                if b.style.visibility != Visibility::Hidden {
                    self.list.push(DisplayItem::Text {
                        x: m.x,
                        y: m.baseline,
                        font: m.font,
                        size: m.size,
                        color: m.color,
                        text: m.text.to_string(),
                        letter_spacing: 0.0,
                        shadows: Vec::new(),
                        small_caps: false,
                        oblique: false,
                    });
                }
            }
            if b.kind == BoxKind::Image {
                self.paint_image_box(b);
            }
        }
        let clip = b.clips && !b.rect.is_empty();
        let saved_scope = if clip { Some(self.push_clip(b)) } else { None };
        if phase == Phase::Floats {
            for c in b.children.iter().filter(|c| c.flow.is_float() && !Self::is_deferred(c)) {
                self.paint_float_or_atomic(c);
            }
        }
        if b.lines.is_empty() {
            for c in b.children.iter().filter(|c| c.flow == Flow::InFlow && !Self::is_deferred(c)) {
                if phase == Phase::Backgrounds && c.kind != BoxKind::Text {
                    self.paint_background_and_border(c, false);
                }
                self.paint_phase(c, phase);
                if phase == Phase::Content {
                    self.paint_outline(c);
                }
            }
        } else if phase == Phase::Content {
            self.paint_lines(b);
        }
        if let Some(saved) = saved_scope {
            self.pop_clip(saved);
        }
    }

    /// Floats and atomic inline-level boxes paint atomically (as if they
    /// were stacking contexts with z-index 0 that don't capture positioned
    /// descendants — approximated by a full stacking context).
    fn paint_float_or_atomic(&mut self, c: &'t LayoutBox) {
        self.paint_stacking_context(c, c.content_rect);
    }

    // -------------------------------------------------------- backgrounds

    fn paint_background_and_border(&mut self, b: &LayoutBox, skip_bg: bool) {
        if b.style.visibility != Visibility::Hidden && b.style.pointer_events != PointerEvents::None {
            self.record_hit(b.rect, b.node);
        }
        let style = &b.style;
        let rect = b.rect;
        if style.visibility == Visibility::Hidden || rect.is_empty() || !self.visible(&rect) {
            return;
        }
        let mut radii = box_radii(b);
        if crate::widgets::native_rounding(b, self.ctx) {
            radii = [(2.0, 2.0); 4];
        }
        let geom = BoxGeometry {
            border: rect,
            padding: b.padding_rect(),
            content: b.content_rect,
            radii,
            border_width: style.border_width,
        };
        let vp = Rect::new(self.ctx.page_scroll.0, self.ctx.page_scroll.1, self.ctx.viewport_width, self.ctx.viewport_height);
        let widget = crate::widgets::widget(b, self.ctx);
        background::paint_box_shadows(self.list, &geom, &style.box_shadow, false);
        if let Some(crate::widgets::Widget::Replace(items)) = widget {
            // Native checkbox / radio / slider / progress / meter.
            for it in items {
                self.list.push(it);
            }
            return;
        }
        if !skip_bg {
            background::paint_backgrounds(self.list, &geom, style, self.ctx.background_images, vp);
            // A vertical box's background images, unturned.
            if let Some((ptr, prect, m, bg)) = self.wm_bg.clone() {
                if ptr == b as *const LayoutBox as usize {
                    if let Some(inv) = crate::tree::invert_matrix(m) {
                        self.list.push(DisplayItem::PushTransform(inv));
                        let bw = bg.border_width;
                        let pad = crate::block::resolve_edges(&bg.padding, prect.w);
                        let padding = Rect::new(prect.x + bw.left, prect.y + bw.top, (prect.w - bw.left - bw.right).max(0.0), (prect.h - bw.top - bw.bottom).max(0.0));
                        let content = Rect::new(padding.x + pad.left, padding.y + pad.top, (padding.w - pad.left - pad.right).max(0.0), (padding.h - pad.top - pad.bottom).max(0.0));
                        let pgeom = BoxGeometry { border: prect, padding, content, radii: bg.resolved_radii(prect.w, prect.h), border_width: bw };
                        background::paint_backgrounds(self.list, &pgeom, &bg, self.ctx.background_images, vp);
                        self.list.push(DisplayItem::PopTransform);
                    }
                }
            }
        }
        background::paint_box_shadows(self.list, &geom, &style.box_shadow, true);
        if style.has_visible_border() {
            let bw = style.border_width;
            let bs = style.border_style;
            let bc = style.border_color;
            self.list.push(DisplayItem::Border {
                rect,
                widths: [bw.top, bw.right, bw.bottom, bw.left],
                colors: [bc.top, bc.right, bc.bottom, bc.left],
                styles: [bs.top, bs.right, bs.bottom, bs.left],
                radii: geom.radii,
            });
        }
        if let Some(crate::widgets::Widget::Overlay(items)) = widget {
            for it in items {
                self.list.push(it);
            }
        }
    }

    fn paint_outline(&mut self, b: &LayoutBox) {
        let s = &b.style;
        if s.outline_width <= 0.0 || !s.outline_style.is_visible() || s.visibility == Visibility::Hidden || b.kind == BoxKind::Text {
            return;
        }
        let w = s.outline_width;
        let off = s.outline_offset;
        let r = Rect::new(b.rect.x - off - w, b.rect.y - off - w, b.rect.w + 2.0 * (off + w), b.rect.h + 2.0 * (off + w));
        if !self.visible(&r) {
            return;
        }
        let color = s.outline_color.unwrap_or(s.color);
        let radii = box_radii(b);
        let grown: Radii = [
            (radii[0].0 + off + w, radii[0].1 + off + w),
            (radii[1].0 + off + w, radii[1].1 + off + w),
            (radii[2].0 + off + w, radii[2].1 + off + w),
            (radii[3].0 + off + w, radii[3].1 + off + w),
        ];
        let has_radius = radii.iter().any(|(h, v)| *h > 0.0 && *v > 0.0);
        self.list.push(DisplayItem::Border {
            rect: r,
            widths: [w; 4],
            colors: [color; 4],
            styles: [s.outline_style; 4],
            radii: if has_radius { grown } else { [(0.0, 0.0); 4] },
        });
    }

    fn paint_image_box(&mut self, b: &LayoutBox) {
        if b.style.visibility == Visibility::Hidden || !self.visible(&b.content_rect) || b.content_rect.is_empty() {
            return;
        }
        let tag = b.node.and_then(|n| self.ctx.doc.element(n)).map(|e| e.tag.as_str()).unwrap_or("");
        let is_media = matches!(tag, "video" | "audio");
        if tag == "video" {
            // Letterbox like a real player before the poster is drawn.
            self.list.push(DisplayItem::Rect { rect: b.content_rect, color: Color::rgb(0, 0, 0), radius: 0.0 });
        }
        match &b.image {
            Some(img) if img.broken => self.paint_broken_image(b),
            Some(img) => self.paint_image_content(b, img.clone()),
            None => {}
        }
        if is_media {
            let controls = b.node.map(|n| self.ctx.doc.attr(n, "controls").is_some()).unwrap_or(false);
            if controls || tag == "audio" {
                self.paint_media_controls(b, tag == "audio");
            } else {
                // No native controls: a centred play affordance so the
                // element does not look like an empty black box.
                let playing = b.node.and_then(|n| self.ctx.doc.media_ui.get(&n)).is_some_and(|u| !u.paused);
                if !playing {
                    self.paint_big_play(b);
                }
            }
        }
    }

    /// Broken image: light frame, the broken-image icon and the alt text
    /// (clipped to the box).
    fn paint_broken_image(&mut self, b: &LayoutBox) {
        let r = b.content_rect;
        let alt = b.node.and_then(|n| self.ctx.doc.attr(n, "alt")).unwrap_or("").trim().to_string();
        if r.w >= 20.0 && r.h >= 20.0 {
            self.list.push(DisplayItem::Border {
                rect: r,
                widths: [1.0; 4],
                colors: [Color::rgb(192, 192, 192); 4],
                styles: [BorderStyle::Solid; 4],
                radii: [(0.0, 0.0); 4],
            });
        }
        self.list.push(DisplayItem::PushClip(r));
        let inset = if r.w >= 20.0 && r.h >= 20.0 { 2.0 } else { 0.0 };
        let (ix, iy) = (r.x + inset, r.y + inset);
        if r.w >= 14.0 && r.h >= 16.0 {
            self.paint_broken_icon(ix, iy);
        }
        if !alt.is_empty() {
            let st = &b.style;
            let font = self.ctx.fonts.select(st.font_family, st.font_weight, st.is_italic());
            let size = st.font_size;
            let x = if r.w >= 40.0 && r.h >= 16.0 { ix + 18.0 } else { r.x + 2.0 };
            self.list.push(DisplayItem::Text {
                x,
                y: iy + size * 0.9,
                font,
                size,
                color: st.color,
                text: alt,
                letter_spacing: 0.0,
                shadows: Vec::new(),
                small_caps: false,
                oblique: false,
            });
        }
        self.list.push(DisplayItem::PopClip);
    }

    /// 14×16 "broken picture" glyph built from rectangles.
    fn paint_broken_icon(&mut self, x: f32, y: f32) {
        let mut push = |rx: f32, ry: f32, w: f32, h: f32, c: Color| self.list.push(DisplayItem::Rect { rect: Rect::new(x + rx, y + ry, w, h), color: c, radius: 0.0 });
        let frame = Color::rgb(140, 140, 140);
        push(0.0, 0.0, 14.0, 16.0, Color::rgb(250, 250, 250));
        push(0.0, 0.0, 14.0, 1.0, frame);
        push(0.0, 15.0, 14.0, 1.0, frame);
        push(0.0, 0.0, 1.0, 16.0, frame);
        push(13.0, 0.0, 1.0, 16.0, frame);
        // Sky, hill and sun.
        push(2.0, 2.0, 10.0, 6.0, Color::rgb(168, 204, 240));
        push(2.0, 8.0, 10.0, 5.0, Color::rgb(96, 168, 80));
        push(8.0, 3.0, 2.0, 2.0, Color::rgb(240, 200, 40));
        // The tear.
        for i in 0..5 {
            let fi = i as f32;
            push(1.0 + fi * 2.4, 7.0 + if i % 2 == 0 { 0.0 } else { 1.5 }, 2.4, 1.5, Color::rgb(250, 250, 250));
        }
    }

    /// Native media controls: a bar along the bottom with play button,
    /// time, a timeline and a volume glyph.
    fn paint_media_controls(&mut self, b: &LayoutBox, audio: bool) {
        let r = b.content_rect;
        let h = if audio { r.h.min(54.0) } else { 40.0f32.min(r.h) };
        if h < 16.0 || r.w < 60.0 {
            return;
        }
        let bar = Rect::new(r.x, r.bottom() - h, r.w, h);
        let (bg, fg, track) = if audio {
            (Color::rgb(241, 243, 244), Color::rgb(32, 33, 36), Color::rgb(190, 190, 190))
        } else {
            (Color::rgba(0, 0, 0, 140), Color::rgb(255, 255, 255), Color::rgba(255, 255, 255, 110))
        };
        self.list.push(DisplayItem::Rect { rect: bar, color: bg, radius: if audio { (h / 2.0).min(27.0) } else { 0.0 } });
        let cy = bar.y + bar.h / 2.0;
        // Playback state (set by the media bindings).
        let ui = b.node.and_then(|n| self.ctx.doc.media_ui.get(&n).copied()).unwrap_or(rabbithole_dom::MediaUi { paused: true, duration: f64::NAN, volume: 1.0, ..Default::default() });
        // Play triangle / pause bars.
        let px = bar.x + 14.0;
        if ui.paused {
            self.push_triangle(px, cy, 12.0, fg);
        } else {
            self.list.push(DisplayItem::Rect { rect: Rect::new(px, cy - 6.0, 4.0, 12.0), color: fg, radius: 0.0 });
            self.list.push(DisplayItem::Rect { rect: Rect::new(px + 7.0, cy - 6.0, 4.0, 12.0), color: fg, radius: 0.0 });
        }
        let fmt = |t: f64| -> String {
            if !t.is_finite() || t < 0.0 {
                return if t.is_infinite() { "LIVE".into() } else { "0:00".into() };
            }
            let s = t.floor() as u64;
            if s >= 3600 {
                format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
            } else {
                format!("{}:{:02}", s / 60, s % 60)
            }
        };
        let label = format!("{} / {}", fmt(ui.time), fmt(ui.duration));
        // Time label.
        let font = self.ctx.fonts.select(rabbithole_gfx::Family::Sans, 400, false);
        let size = 12.0;
        self.list.push(DisplayItem::Text {
            x: px + 22.0,
            y: cy + size * 0.35,
            font,
            size,
            color: fg,
            text: label,
            letter_spacing: 0.0,
            shadows: Vec::new(),
            small_caps: false,
            oblique: false,
        });
        // Timeline and volume.
        let tl_x = px + 110.0;
        let tl_end = bar.right() - 44.0;
        if tl_end - tl_x > 20.0 {
            let w = tl_end - tl_x;
            let frac = |t: f64| if ui.duration.is_finite() && ui.duration > 0.0 { (t / ui.duration).clamp(0.0, 1.0) as f32 } else { 0.0 };
            self.list.push(DisplayItem::Rect { rect: Rect::new(tl_x, cy - 2.0, w, 4.0), color: track, radius: 2.0 });
            let buffered = frac(ui.buffered_end);
            if buffered > 0.0 {
                self.list.push(DisplayItem::Rect { rect: Rect::new(tl_x, cy - 2.0, w * buffered, 4.0), color: track, radius: 2.0 });
            }
            let played = frac(ui.time);
            if played > 0.0 {
                self.list.push(DisplayItem::Rect { rect: Rect::new(tl_x, cy - 2.0, w * played, 4.0), color: fg, radius: 2.0 });
            }
            self.list.push(DisplayItem::Rect { rect: Rect::new(tl_x + w * played - 5.0, cy - 5.0, 10.0, 10.0), color: fg, radius: 5.0 });
        }
        let vx = bar.right() - 30.0;
        if vx > px + 30.0 {
            self.list.push(DisplayItem::Rect { rect: Rect::new(vx, cy - 3.0, 4.0, 6.0), color: fg, radius: 0.0 });
            if ui.muted || ui.volume <= 0.0 {
                // Speaker with a cross.
                for i in 0..5 {
                    let fi = i as f32;
                    self.list.push(DisplayItem::Rect { rect: Rect::new(vx + 7.0 + fi, cy - 3.0 + fi * 1.5, 1.5, 1.5), color: fg, radius: 0.0 });
                    self.list.push(DisplayItem::Rect { rect: Rect::new(vx + 7.0 + fi, cy + 3.0 - fi * 1.5, 1.5, 1.5), color: fg, radius: 0.0 });
                }
            } else {
                let bars = (ui.volume * 5.0).ceil().clamp(1.0, 5.0) as i32;
                for i in 0..bars {
                    let fi = i as f32;
                    self.list.push(DisplayItem::Rect { rect: Rect::new(vx + 4.0 + fi, cy - 3.0 - fi, 1.0, 6.0 + 2.0 * fi), color: fg, radius: 0.0 });
                }
            }
        }
    }

    /// Centred round play button over a video without controls.
    fn paint_big_play(&mut self, b: &LayoutBox) {
        let r = b.content_rect;
        let d = 56.0f32.min(r.w * 0.5).min(r.h * 0.5);
        if d < 16.0 {
            return;
        }
        let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        self.list.push(DisplayItem::Rect { rect: Rect::new(cx - d / 2.0, cy - d / 2.0, d, d), color: Color::rgba(0, 0, 0, 150), radius: d / 2.0 });
        self.push_triangle(cx - d * 0.15, cy, d * 0.4, Color::rgb(255, 255, 255));
    }

    /// Right-pointing triangle with its left edge at `x`, centred on `cy`.
    fn push_triangle(&mut self, x: f32, cy: f32, size: f32, color: Color) {
        let steps = size.ceil().max(2.0) as i32;
        let w = size * 0.87;
        let sw = w / steps as f32;
        for i in 0..steps {
            let t = (i as f32 + 0.5) / steps as f32;
            // Slice height shrinks linearly towards the tip.
            let sh = size * (1.0 - t);
            self.list.push(DisplayItem::Rect { rect: Rect::new(x + i as f32 * sw, cy - sh / 2.0, sw + 0.3, sh), color, radius: 0.0 });
        }
    }

    fn paint_image_content(&mut self, b: &LayoutBox, img: Arc<Image>) {
        let img = &img;
        let (iw, ih) = (img.width as f32, img.height as f32);
        let dest = b.content_rect;
        let fit = b.style.object_fit;
        let (dw, dh) = match fit {
            ObjectFit::Fill => (dest.w, dest.h),
            ObjectFit::Contain | ObjectFit::ScaleDown | ObjectFit::Cover | ObjectFit::None => {
                if iw <= 0.0 || ih <= 0.0 {
                    (dest.w, dest.h)
                } else {
                    let sx = dest.w / iw;
                    let sy = dest.h / ih;
                    let s = match fit {
                        ObjectFit::Contain => sx.min(sy),
                        ObjectFit::ScaleDown => sx.min(sy).min(1.0),
                        ObjectFit::Cover => sx.max(sy),
                        _ => 1.0,
                    };
                    (iw * s, ih * s)
                }
            }
        };
        let px = b.style.object_position.0;
        let py = b.style.object_position.1;
        let ox = match px {
            rabbithole_style::Length::Percent(p) => (dest.w - dw) * p / 100.0,
            l @ (rabbithole_style::Length::Calc(..) | rabbithole_style::Length::Math(_)) => l.resolve_or_zero(dest.w - dw),
            other => other.resolve_or_zero(dest.w),
        };
        let oy = match py {
            rabbithole_style::Length::Percent(p) => (dest.h - dh) * p / 100.0,
            l @ (rabbithole_style::Length::Calc(..) | rabbithole_style::Length::Math(_)) => l.resolve_or_zero(dest.h - dh),
            other => other.resolve_or_zero(dest.h),
        };
        let placed = Rect::new(dest.x + ox, dest.y + oy, dw, dh);
        // Clip the placed image to the content box and compute the source
        // sub-rectangle actually shown.
        let shown = placed.intersect(&dest);
        if shown.is_empty() || dw <= 0.0 || dh <= 0.0 {
            return;
        }
        let src = Rect::new(
            (shown.x - placed.x) / dw * iw,
            (shown.y - placed.y) / dh * ih,
            shown.w / dw * iw,
            shown.h / dh * ih,
        );
        self.list.push(DisplayItem::Image {
            rect: shown,
            src,
            image: img.clone(),
            opacity: 1.0,
            pixelated: b.style.image_rendering_pixelated,
        });
    }

    // --------------------------------------------------------------- lines

    fn paint_lines(&mut self, b: &'t LayoutBox) {
        let rtl = b.style.direction == rabbithole_style::Direction::Rtl;
        let _ = rtl;
        for line in &b.lines {
            if !self.visible(&line.rect) && !line.rect.is_empty() && line.fragments.iter().all(|f| !self.visible(&f.rect)) {
                continue;
            }
            for f in &line.fragments {
                match &f.kind {
                    FragmentKind::InlineBox {
                        style,
                        left_edge,
                        right_edge,
                    } => {
                        if style.visibility == Visibility::Hidden {
                            continue;
                        }
                        if !f.inert {
                            self.record_hit(f.rect, f.node);
                        }
                        let bw = style.border_width;
                        let widths = [bw.top, if *right_edge { bw.right } else { 0.0 }, bw.bottom, if *left_edge { bw.left } else { 0.0 }];
                        let radii = style.resolved_radii(f.rect.w, f.rect.h);
                        let geom = BoxGeometry {
                            border: f.rect,
                            padding: Rect::new(f.rect.x + widths[3], f.rect.y + widths[0], (f.rect.w - widths[1] - widths[3]).max(0.0), (f.rect.h - widths[0] - widths[2]).max(0.0)),
                            content: f.rect,
                            radii,
                            border_width: rabbithole_style::Edges { top: widths[0], right: widths[1], bottom: widths[2], left: widths[3] },
                        };
                        let vp = Rect::new(self.ctx.page_scroll.0, self.ctx.page_scroll.1, self.ctx.viewport_width, self.ctx.viewport_height);
                        background::paint_box_shadows(self.list, &geom, &style.box_shadow, false);
                        background::paint_backgrounds(self.list, &geom, style, self.ctx.background_images, vp);
                        background::paint_box_shadows(self.list, &geom, &style.box_shadow, true);
                        if style.has_visible_border() {
                            let bs = style.border_style;
                            let bc = style.border_color;
                            self.list.push(DisplayItem::Border {
                                rect: f.rect,
                                widths,
                                colors: [bc.top, bc.right, bc.bottom, bc.left],
                                styles: [bs.top, bs.right, bs.bottom, bs.left],
                                radii,
                            });
                        }
                    }
                    FragmentKind::Text {
                        text,
                        font,
                        size,
                        color,
                        letter_spacing,
                        hidden,
                        decoration_info,
                        shadows,
                        small_caps,
                        oblique,
                        emphasis,
                    } => {
                        if *hidden {
                            continue;
                        }
                        if !f.inert {
                            self.record_hit(f.rect, f.node);
                        }
                        if !self.visible(&f.rect) {
                            continue;
                        }
                        if self.flipped {
                            let cy = f.rect.y + f.rect.h / 2.0;
                            self.list.push(DisplayItem::PushTransform([1.0, 0.0, 0.0, -1.0, 0.0, 2.0 * cy]));
                        }
                        // Underlines and overlines go under the text,
                        // line-throughs over it (CSS Text Decoration 3 §2.1).
                        if decoration_info.any() {
                            self.paint_decorations(f.rect, f.baseline, *font, *size, *color, decoration_info, false);
                        }
                        self.list.push(DisplayItem::Text {
                            x: f.rect.x,
                            y: f.baseline,
                            font: *font,
                            size: *size,
                            color: *color,
                            text: text.to_string(),
                            letter_spacing: *letter_spacing,
                            shadows: shadows.clone(),
                            small_caps: *small_caps,
                            oblique: *oblique,
                        });
                        if decoration_info.any() {
                            self.paint_decorations(f.rect, f.baseline, *font, *size, *color, decoration_info, true);
                        }
                        if self.flipped {
                            self.list.push(DisplayItem::PopTransform);
                        }
                        if let Some(e) = emphasis {
                            self.paint_emphasis(f.rect, *font, *size, text, *letter_spacing, e);
                        }
                    }
                    FragmentKind::Box(i) => {
                        let c = &b.children[*i];
                        if !Self::is_deferred(c) {
                            self.paint_float_or_atomic(c);
                        }
                    }
                }
            }
        }
    }

    /// Paint emphasis marks centered over (or under) each character of a
    /// text fragment, at half the font size, touching the glyph box.
    fn paint_emphasis(&mut self, rect: Rect, font: rabbithole_gfx::FontId, size: f32, text: &str, letter_spacing: f32, e: &crate::tree::Emphasis) {
        let fonts = self.ctx.fonts;
        let msize = size / 2.0;
        let mm = fonts.metrics(font, msize);
        let mw = fonts.measure(font, msize, &e.mark);
        let y = if e.over { rect.y - mm.descent } else { rect.bottom() + mm.ascent };
        let glyphs = fonts.shape(font, size, text, letter_spacing);
        let mut i = 0;
        while i < glyphs.len() {
            let g = glyphs[i];
            let mut adv = g.advance;
            let mut j = i + 1;
            while j < glyphs.len() && glyphs[j].byte_index == g.byte_index {
                adv += glyphs[j].advance;
                j += 1;
            }
            i = j;
            let Some(ch) = text.get(g.byte_index..).and_then(|t| t.chars().next()) else { continue };
            if emphasis_skips(ch) {
                continue;
            }
            let x = rect.x + g.x + (adv - letter_spacing - mw) / 2.0;
            if self.flipped {
                let cy = y - mm.ascent + (mm.ascent + mm.descent) / 2.0;
                self.list.push(DisplayItem::PushTransform([1.0, 0.0, 0.0, -1.0, 0.0, 2.0 * cy]));
            }
            self.list.push(DisplayItem::Text {
                x,
                y,
                font,
                size: msize,
                color: e.color,
                text: e.mark.to_string(),
                letter_spacing: 0.0,
                shadows: Vec::new(),
                small_caps: false,
                oblique: false,
            });
            if self.flipped {
                self.list.push(DisplayItem::PopTransform);
            }
        }
    }

    /// Paint the underlines and overlines (`through` false) or the
    /// line-throughs (`through` true) of a text fragment.
    #[allow(clippy::too_many_arguments)]
    fn paint_decorations(&mut self, rect: Rect, baseline: f32, font: rabbithole_gfx::FontId, size: f32, _text_color: Color, info: &DecorationInfo, through: bool) {
        let fm = self.ctx.fonts.decoration_metrics(font, size, &info.variations);
        let asc = baseline - rect.y;
        let desc = rect.bottom() - baseline;
        for (i, d) in info.list.iter().enumerate() {
            let (x0, x1) = info.extents.get(i).copied().flatten().map(|(a, b)| (rect.x + a, rect.x + b)).unwrap_or((rect.x, rect.right()));
            let (bo, asc, desc) = info.metrics.get(i).copied().unwrap_or((0.0, asc, desc));
            let baseline = baseline + bo;
            if x1 <= x0 {
                continue;
            }
            let auto_t = (size / 16.0).max(1.0);
            let thick = |from: Option<(f32, f32)>| -> f32 {
                match d.thickness {
                    Some(t) => t.max(0.0),
                    None if d.from_font => from.map(|m| m.1).filter(|t| *t > 0.0).unwrap_or(auto_t),
                    None => auto_t,
                }
            };
            // Decoration lines are snapped to device pixels (whole-pixel
            // thickness, at least 1px; pixel-aligned ends so the pieces of a
            // line over adjacent fragments join seamlessly), as in Blink.
            let (x0, x1) = (x0.round(), x1.round());
            let mut line = |top: f32, t: f32| {
                if t <= 0.0 || x1 <= x0 {
                    return;
                }
                let t = t.round().max(1.0);
                let top = top.round();
                self.list.push(DisplayItem::Line { x0, y0: top + t / 2.0, x1, y1: top + t / 2.0, width: t, color: d.color, style: d.style });
            };
            if through {
                if d.lines.line_through {
                    let t = thick(fm.strikeout);
                    line(baseline - size * 0.3 - t / 2.0, t);
                }
                continue;
            }
            if d.lines.underline {
                let t = thick(fm.underline);
                let pos = d.underline_position;
                let top = if pos.under {
                    baseline + desc + d.underline_offset.unwrap_or(0.0)
                } else if pos.from_font && fm.underline.is_some() {
                    fm.underline.map(|(p, _)| baseline - p).unwrap_or(baseline) + d.underline_offset.unwrap_or(0.0)
                } else if let Some(o) = d.underline_offset {
                    baseline + o
                } else {
                    baseline + t
                };
                line(top, t);
            }
            if d.lines.overline {
                let t = thick(fm.underline);
                line(baseline - asc, t);
            }
        }
    }
}

/// Characters that get no emphasis mark: separators, controls and
/// punctuation, other (format, unassigned...) characters (CSS Text
/// Decoration 3 §3.1: Unicode categories Z*, P* and C*).
fn emphasis_skips(c: char) -> bool {
    use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};
    c.is_whitespace()
        || matches!(c.general_category_group(), GeneralCategoryGroup::Punctuation | GeneralCategoryGroup::Separator | GeneralCategoryGroup::Other)
}
