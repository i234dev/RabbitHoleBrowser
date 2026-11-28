//! Block layout: width/height resolution, margin collapsing, floats,
//! absolute/relative positioning and the document entry point.

use crate::build::Builder;
use crate::float::Bfc;
use crate::text::Measurer;
use crate::tree::{BoxKind, Flow, Formatting, FragmentKind, LayoutBox, LayoutTree, Marker};
use crate::LayoutContext;
use rabbithole_gfx::{FontId, Rect};
use rabbithole_style::{BoxSizing, Clear, ComputedStyle, Edges, ItemAlign, Length, Position};
use std::collections::HashMap;

// `LayoutBox::control` values.
const CONTROL_NONE: u8 = 1;
const CONTROL_TEXTAREA: u8 = 2;
const CONTROL_INPUT: u8 = 3;
const CONTROL_SELECT: u8 = 4;
const CONTROL_BUTTON: u8 = 5;

/// Containing block information passed down during layout.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Cb {
    /// Left edge of the containing block's content box.
    pub x: f32,
    /// Content width.
    pub width: f32,
    /// Content height when definite (for percentage heights).
    pub height: Option<f32>,
}

/// How to resolve `width: auto`.
#[derive(Debug, Clone, Copy)]
pub(crate) enum WidthMode {
    /// Fill the containing block (normal block-level boxes).
    Fill,
    /// Shrink-to-fit (floats, inline-blocks, absolutes).
    ShrinkToFit,
    /// Use this content width (table cells, flex items).
    Fixed(f32),
}


/// Line clamping in progress (CSS Overflow 4 §4.4: `line-clamp`).
#[derive(Debug, Clone, Default)]
pub(crate) struct ClampState {
    /// Lines that may still be shown.
    pub remaining: usize,
    /// Bottom of the last line shown, once the clamp point is reached.
    pub bottom: Option<f32>,
    /// The clamp point ended a paragraph without an ellipsis.
    pub final_hit: bool,
    /// Something follows the clamp point (it needs the block ellipsis).
    pub content_after: bool,
    /// Second pass: put the ellipsis on the clamp line even when it ends
    /// its paragraph.
    pub ellipsize_last: bool,
    /// Record line bottoms (first pass of `line-clamp: auto`).
    pub record: bool,
    pub bottoms: Vec<f32>,
}

impl ClampState {
    pub fn lines(n: usize) -> Self {
        ClampState { remaining: n, ..Default::default() }
    }

    /// The clamp point was reached: what follows is hidden.
    pub fn hit(&self) -> bool {
        self.remaining == 0
    }
}

pub(crate) struct Layouter<'a> {
    pub ctx: &'a LayoutContext<'a>,
    pub measurer: Measurer<'a>,
    /// grid/flex hook: when set, the next `layout_block` call uses this
    /// content-box height as the box's definite height (stretched flex
    /// items, grid items, definite flex main sizes in column flex).
    pub forced_content_height: Option<f32>,
    /// Flex hook: the content-box (min, max) heights of the flex container
    /// whose contents are about to be laid out with an indefinite height
    /// (single-line cross-size clamping, column main size).
    pub(crate) flex_limits: Option<(f32, f32)>,
    /// Flex hook: the `forced_content_height` of the next box is its used
    /// height but not a definite base for its children's percentages (a
    /// column item flexed in a container with an indefinite height).
    pub(crate) forced_height_indefinite: bool,
    /// Flex: same-size relayouts of stretched items (for definite
    /// percentage bases) in progress.
    pub(crate) definite_relayouts: u32,
    /// Intrinsic sizing: the definite content height of the containing
    /// block of the box being measured (percentage heights of replaced
    /// elements and aspect-ratio boxes resolve against it).
    pub(crate) intrinsic_cb_h: Option<f32>,
    /// The (min, max)-content widths of the box whose width is being
    /// computed, when its min/max-width uses intrinsic sizes.
    pub(crate) width_intrinsic: Option<(f32, f32)>,
    /// Intrinsic sizing: a measured box read `intrinsic_cb_h` (or a forced
    /// height); such results are not cached.
    pub(crate) intrinsic_cb_used: bool,
    /// Intrinsic sizing: the stretched definite height of the next box
    /// measured (a flex item in a container with a definite height).
    pub(crate) intrinsic_forced_h: Option<f32>,
    /// Floats: the height of the band beside floats that the next
    /// auto-height formatting context laid out must fit (a retry after its
    /// first layout came out taller than the space it was given).
    float_band: Option<f32>,
    /// A box is being refitted beside floats (nested layouts do not refit).
    refitting: bool,
    /// Inside the flow of a multi-column container whose columns have a
    /// known height: (flow position where its columns start, their
    /// height, their count). Nested multi-column containers break at its
    /// column ends.
    pub(crate) column_frame: Option<(f32, f32, u32)>,
    /// The space really left for the first row of nested multi-column
    /// containers (by element), and whether such a correcting layout is
    /// in progress.
    pub(crate) column_rooms: Vec<(rabbithole_dom::NodeId, f32)>,
    pub(crate) column_retry: bool,
    /// Pooled working storage of flex layout.
    pub(crate) flex_scratch: Vec<crate::flex::FlexScratch>,
    /// Some absolutely positioned box has an inline element as its
    /// containing block (computed on first use; see
    /// [`Self::needs_inline_cbs`]).
    inline_cbs_needed: Option<bool>,
    /// Whether `FontContext::select_family` & co. are implemented.
    pub font_ext: bool,
    /// Cache of family-list lookups: (families joined, weight, italic) → font.
    font_cache: HashMap<(String, u16, bool, u16), FontId>,
    /// The last `font_cache` lookup: (families, weight, italic, stretch) → font.
    last_font: Option<(Vec<String>, u16, bool, u16, FontId)>,
    /// (face, feature props by address, inter-character) → feature variant
    /// (the computed styles outlive a layout pass).
    feature_cache: HashMap<(usize, usize, bool), FontId>,
    /// Containing blocks of absolutely positioned grid children placed by
    /// grid lines (their grid area), set by the grid layout.
    pub(crate) abs_cb: HashMap<rabbithole_dom::NodeId, Rect>,
    /// The absolutely positioned boxes being laid out belong to a
    /// transformed containing block, which also contains fixed boxes.
    fixed_cb: bool,
    anchor_areas: crate::anchor::AnchoredLayout,
    /// The block children being laid out have a `-webkit-center` parent.
    center_blocks: bool,
    /// The containing block of the block children being laid out is
    /// right-to-left (over-constrained margins resolve by its direction).
    cb_rtl: bool,
    /// Line clamping of a block container in progress (`line-clamp`
    /// counts the lines of its block descendants in the same formatting
    /// context).
    pub(crate) clamp: Option<ClampState>,
    /// Bounds only the new recursive subgrid contribution/transport work.
    pub(crate) grid_work: crate::grid::subgrid::WorkBudget,
    pub(crate) grid_publication: crate::grid::resolved::Publication,
    /// Grid containers laid out, plus layouts reused from entries made
    /// while grid tracks were not published: a layout that saw neither
    /// publishes nothing, so its cache entry serves both modes.
    pub(crate) grid_layouts: u64,
}

pub(crate) fn resolve_edges(e: &Edges<Length>, base: f32) -> Edges<f32> {
    Edges {
        top: e.top.resolve_or_zero(base),
        right: e.right.resolve_or_zero(base),
        bottom: e.bottom.resolve_or_zero(base),
        left: e.left.resolve_or_zero(base),
    }
}

/// Horizontal padding + border in px (percentages against `cbw`).
pub(crate) fn horizontal_extras(style: &ComputedStyle, cbw: f32) -> f32 {
    let p = resolve_edges(&style.padding, cbw);
    p.left + p.right + style.border_width.left + style.border_width.right
}

/// Vertical padding + border in px (percentages count as 0, as in
/// intrinsic sizing).
pub(crate) fn vertical_extras(style: &ComputedStyle) -> f32 {
    let p = resolve_edges(&style.padding, 0.0);
    p.top + p.bottom + style.border_width.top + style.border_width.bottom
}

/// Whether a min/max size is an intrinsic keyword, `fit-content()` or
/// `stretch` (sizes that need more than a percentage base).
#[inline]
fn sizing_keyword(l: Length) -> bool {
    match l {
        Length::MinContent | Length::MaxContent | Length::FitContent => true,
        Length::Math(_) => rabbithole_style::is_stretch(l) || rabbithole_style::is_fit_content_fn(l),
        _ => false,
    }
}

/// The content height of a `stretch` box in a containing block of
/// content height `cb_h`: its margin box fills it.
fn stretch_height(style: &ComputedStyle, cb_h: f32, pb_v: f32) -> f32 {
    let m = style.margin.top.resolve_or_zero(0.0) + style.margin.bottom.resolve_or_zero(0.0);
    (cb_h - m - pb_v).max(0.0)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PositionedDescendants {
    Absolute,
    AbsoluteAndFixed,
    FixedOnly,
}

/// Static-position rectangle of an absolutely positioned box (inline
/// axis) and the alignment defaults of its static parent.
#[derive(Clone, Copy)]
pub(crate) struct StaticRect {
    /// The rectangle applies (a block-level box in a block container).
    block_level: bool,
    x: f32,
    w: f32,
    rtl: bool,
    justify_items: (ItemAlign, rabbithole_style::AlignMods),
}

/// Where an explicitly aligned absolutely positioned box goes in its free
/// space (0 = physical left/top, 1 = right/bottom); None for `normal`
/// and `stretch`, which keep the CSS 2.1 placement. `rtl` is the
/// direction of the alignment container, `own_rtl` the box's own (for
/// `self-*`); `inline` says whether this is the inline axis.
fn abs_align_fraction(align: ItemAlign, mods: rabbithole_style::AlignMods, rtl: bool, own_rtl: bool, inline: bool) -> Option<f32> {
    use rabbithole_style::AlignPos;
    let start = |reversed: bool| if reversed { 1.0 } else { 0.0 };
    Some(match (mods.pos, align) {
        (AlignPos::Left, _) => 0.0,
        (AlignPos::Right, _) => if inline { 1.0 } else { 0.0 },
        (AlignPos::SelfStart, _) => start(inline && own_rtl),
        (AlignPos::SelfEnd, _) => 1.0 - start(inline && own_rtl),
        (_, ItemAlign::FlexStart) => start(inline && rtl),
        (_, ItemAlign::FlexEnd) => 1.0 - start(inline && rtl),
        (_, ItemAlign::Center) => 0.5,
        // `last baseline` falls back to (safe) end without a baseline set.
        (_, ItemAlign::Baseline) if mods.last => 1.0 - start(inline && rtl),
        _ => return None,
    })
}

/// A block formatting context root next to floats: its (x, y, content
/// width), narrowed or moved below them.
#[cold]
#[inline(never)]
/// `band_h` is the box's border-box height when known before its layout:
/// it must clear the floats beside all of it.
#[allow(clippy::too_many_arguments)]
fn avoid_floats(style: &ComputedStyle, bfc: &Bfc, cb: &Cb, (mut x, mut y, mut content_w): (f32, f32, f32), pb_h: f32, ml: f32, mr: f32, band_h: f32) -> (f32, f32, f32) {
    let cbw = cb.width;
    let mut guard = 0;
    loop {
        let (l, r) = bfc.available(y, band_h.max(1.0), cb.x, cb.width);
        if l <= cb.x && r >= cb.x + cb.width {
            break;
        }
        // The used margins of a sized box absorb the rest of the
        // containing block; next to floats only its own margins count.
        let (sml, smr) = (style.margin.left.resolve(cbw).unwrap_or(0.0), style.margin.right.resolve(cbw).unwrap_or(0.0));
        let outer = content_w + pb_h + sml + smr;
        if (style.width.is_auto() || rabbithole_style::is_stretch(style.width)) && (r - l - ml - mr > 0.0 || bfc.next_y(y).is_none()) {
            // (No room at all beside the floats: further down.)
            content_w = (r - l - ml - mr - pb_h).max(0.0);
            x = l + ml;
            break;
        } else if outer <= r - l + 0.01 {
            // Auto margins share the space left beside the floats.
            let free = r - l - outer;
            x = match (style.margin.left.is_auto(), style.margin.right.is_auto()) {
                (true, true) => l + free / 2.0,
                (true, false) => l + free,
                _ => l + sml,
            };
            break;
        }
        match bfc.next_y(y) {
            Some(ny) => y = ny,
            None => break,
        }
        guard += 1;
        if guard > 100 {
            break;
        }
    }
    (x, y, content_w)
}

/// A set of adjoining vertical margins being collapsed (CSS 2.1 §8.3.1):
/// the result is the largest positive margin plus the most negative one.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Margins {
    pos: f32,
    neg: f32,
}

impl Margins {
    pub fn of(m: f32) -> Margins {
        Margins::default().add(m)
    }

    pub fn from_pair((pos, neg): (f32, f32)) -> Margins {
        Margins { pos, neg }
    }

    pub fn pair(self) -> (f32, f32) {
        (self.pos, self.neg)
    }

    pub fn add(self, m: f32) -> Margins {
        Margins {
            pos: self.pos.max(m),
            neg: self.neg.min(m),
        }
    }

    pub fn join(self, o: Margins) -> Margins {
        Margins {
            pos: self.pos.max(o.pos),
            neg: self.neg.min(o.neg),
        }
    }

    pub fn resolve(self) -> f32 {
        self.pos + self.neg
    }
}

/// Does a length used for `height`/`min-height` resolve to zero or auto
/// (percentages against an indefinite containing block are auto)?
fn zero_or_auto_height(l: Length, cb_h: Option<f32>) -> bool {
    match l {
        Length::Px(v) => v == 0.0,
        Length::Percent(p) => p == 0.0 || cb_h.map(|h| h * p == 0.0).unwrap_or(true),
        Length::Calc(..) | Length::Math(_) => cb_h.map(|h| l.resolve_or_zero(h) == 0.0).unwrap_or(true),
        Length::Auto => true,
        Length::MinContent | Length::MaxContent | Length::FitContent => true,
    }
}

/// Is the height of `b` (a child of a box whose height is definite when
/// `parent_definite`) definite, as a base for its children's percentage
/// offsets? Flex and grid items are handled by the caller.
fn height_definite(b: &LayoutBox, parent_definite: bool) -> bool {
    if b.kind == BoxKind::AnonymousBlock {
        return parent_definite;
    }
    let s = &b.style;
    match s.height {
        Length::Px(_) => true,
        Length::Auto | Length::MinContent | Length::MaxContent | Length::FitContent => {
            b.flow == Flow::Absolute && !s.inset.top.is_auto() && !s.inset.bottom.is_auto()
                || b.kind == BoxKind::Image
                || s.aspect_ratio.is_some()
                || b.wm_wrapper
        }
        _ => parent_definite,
    }
}

fn is_zero_len(l: Length) -> bool {
    match l {
        Length::Px(v) => v == 0.0,
        Length::Percent(p) => p == 0.0,
        _ => true,
    }
}

/// Would this inline content produce no line boxes (CSS 2.1 §9.4.2: lines
/// with no text, no preserved white space, no inline boxes with non-zero
/// margins/padding/borders and no other in-flow content are zero-height)?
pub(crate) fn inline_content_is_empty(b: &LayoutBox) -> bool {
    use crate::tree::InlineItem;
    use rabbithole_style::WhiteSpace;
    for it in &b.items {
        match it {
            InlineItem::Text { text, style, .. } => {
                let preserve = matches!(style.white_space, WhiteSpace::Pre | WhiteSpace::PreWrap | WhiteSpace::BreakSpaces);
                let preserve_newlines = style.white_space == WhiteSpace::PreLine;
                for ch in text.chars() {
                    let collapsible = matches!(ch, ' ' | '\t' | '\r' | '\x0c') && !preserve || ch == '\n' && !preserve && !preserve_newlines;
                    if !collapsible {
                        return false;
                    }
                }
            }
            InlineItem::Open { style, .. } => {
                let s = style;
                if s.border_width.left > 0.0
                    || s.border_width.right > 0.0
                    || !is_zero_len(s.padding.left)
                    || !is_zero_len(s.padding.right)
                    || !is_zero_len(s.margin.left)
                    || !is_zero_len(s.margin.right)
                {
                    return false;
                }
            }
            InlineItem::Close | InlineItem::Float(_) | InlineItem::Absolute(_) => {}
            InlineItem::Atomic(_) | InlineItem::Break | InlineItem::ClearBreak(_) => return false,
        }
    }
    true
}

pub(crate) fn shrink_to_fit(min: f32, max: f32, avail: f32) -> f32 {
    min.max(avail.min(max))
}

impl<'a> Layouter<'a> {
    pub fn new(ctx: &'a LayoutContext<'a>) -> Self {
        Layouter {
            ctx,
            measurer: Measurer::new(ctx.fonts),
            forced_content_height: None,
            flex_limits: None,
            forced_height_indefinite: false,
            definite_relayouts: 0,
            intrinsic_cb_h: None,
            width_intrinsic: None,
            intrinsic_cb_used: false,
            intrinsic_forced_h: None,
            float_band: None,
            refitting: false,
            column_frame: None,
            column_rooms: Vec::new(),
            column_retry: false,
            flex_scratch: Vec::new(),
            inline_cbs_needed: None,
            font_ext: crate::paint::font_ext_available(ctx.fonts),
            font_cache: HashMap::new(),
            last_font: None,
            feature_cache: HashMap::new(),
            abs_cb: HashMap::new(),
            fixed_cb: false,
            anchor_areas: Default::default(),
            center_blocks: false,
            cb_rtl: false,
            clamp: None,
            grid_work: Default::default(),
            grid_publication: Default::default(),
            grid_layouts: 0,
        }
    }

    /// Form controls that show one line of text (and keep its height when
    /// empty).
    fn is_line_control(&self, b: &LayoutBox) -> bool {
        let doc = self.ctx.doc;
        let Some(n) = b.node else { return false };
        match self.control_kind(b) {
            CONTROL_BUTTON => true,
            CONTROL_SELECT => doc.attr(n, "multiple").is_none() && doc.attr(n, "size").and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0) <= 1,
            CONTROL_INPUT => !matches!(crate::build::input_type(doc, n).as_str(), "checkbox" | "radio" | "range" | "color" | "image" | "hidden"),
            _ => false,
        }
    }

    /// Which form control the box's element is (`CONTROL_*`), memoized on
    /// the box: layout asks for every box, and the element's tag is
    /// usually not in the cache.
    #[inline]
    fn control_kind(&self, b: &LayoutBox) -> u8 {
        match b.control.get() {
            0 => {
                let k = match b.node.and_then(|n| self.ctx.doc.tag(n)) {
                    Some("textarea") => CONTROL_TEXTAREA,
                    Some("input") => CONTROL_INPUT,
                    Some("select") => CONTROL_SELECT,
                    Some("button") => CONTROL_BUTTON,
                    _ => CONTROL_NONE,
                };
                b.control.set(k);
                k
            }
            k => k,
        }
    }

    /// Font for a style: the first available family of `font-family`
    /// (web fonts / system fonts) or the generic fallback.
    pub fn font_for(&mut self, style: &ComputedStyle) -> FontId {
        let font = self.font_for_family(style);
        if !self.font_ext || style.font_features.is_none() && style.text_justify != 3 {
            return font;
        }
        let key = (font.0, style.font_features.as_ref().map_or(0, |f| std::rc::Rc::as_ptr(f) as usize), style.text_justify == 3);
        if let Some(&f) = self.feature_cache.get(&key) {
            return f;
        }
        // OpenType features (font-feature-settings, font-variant-*); text
        // justified between characters forms no ligatures (CSS Text 3 §7.3).
        let mut feats = Vec::new();
        if style.text_justify == 3 {
            feats.extend([(*b"liga", 0), (*b"clig", 0), (*b"dlig", 0), (*b"hlig", 0), (*b"calt", 0)]);
        }
        if let Some(f) = &style.font_features {
            for g in f.resolved() {
                feats.retain(|h: &rabbithole_gfx::FontFeature| h.0 != g.0);
                feats.push(g);
            }
        }
        let f = self.ctx.fonts.with_features(font, &feats);
        self.feature_cache.insert(key, f);
        f
    }

    fn font_for_family(&mut self, style: &ComputedStyle) -> FontId {
        if self.font_ext && !style.font_family_list.is_empty() {
            // Consecutive text mostly shares its font: compare with the
            // last lookup before building the cache key.
            if let Some((families, weight, italic, stretch, f)) = &self.last_font {
                if *weight == style.font_weight && *italic == style.is_italic() && *stretch == style.font_stretch && *families == style.font_family_list {
                    return *f;
                }
            }
            let key = (style.font_family_list.join(","), style.font_weight, style.is_italic(), style.font_stretch);
            let f = match self.font_cache.get(&key) {
                Some(f) => *f,
                None => {
                    let f = self.ctx.fonts.select_family_stretch(&style.font_family_list, style.font_family, style.font_weight, style.is_italic(), style.font_stretch);
                    self.font_cache.insert(key, f);
                    f
                }
            };
            self.last_font = Some((style.font_family_list.clone(), style.font_weight, style.is_italic(), style.font_stretch, f));
            return f;
        }
        self.ctx.fonts.select(style.font_family, style.font_weight, style.is_italic())
    }

    /// Does `font` need a synthetic slant for an italic request?
    pub fn synthetic_oblique(&self, font: FontId, style: &ComputedStyle) -> bool {
        style.is_italic() && style.font_synthesis_style && self.font_ext && self.ctx.fonts.needs_synthetic_italic(font, true)
    }

    pub fn layout_document(&mut self) -> LayoutTree {
        self.layout_document_with(None)
    }

    /// Lay out the document, taking unchanged boxes from `reuse`.
    pub(crate) fn layout_document_with(&mut self, reuse: Option<&crate::incremental::Reuse>) -> LayoutTree {
        let initial = self.layout_pass(reuse);
        if !self.ctx.styles.any_position_area { return initial; }
        let mut choices = HashMap::new();
        let mut areas = crate::anchor::placements(self.ctx,&initial,&choices);
        let mut pending = std::mem::take(&mut areas.fallback_targets);
        let mut best = HashMap::new();
        let mut tree = initial;
        loop {
            let available = std::mem::take(&mut areas.available_space);
            if !areas.is_empty() {
                let mut positioned = Self::new(self.ctx);
                positioned.anchor_areas = areas;
                tree = positioned.layout_pass(None);
                tree.resolved_anchor_styles = positioned.anchor_areas.lengths;
            } else if !choices.is_empty() || !tree.resolved_anchor_styles.is_empty() {
                tree = Self::new(self.ctx).layout_pass(None);
            }
            if !crate::anchor::advance_fallbacks(self.ctx,&tree,&mut pending,&mut choices,&available,&mut best) { return tree; }
            areas = crate::anchor::placements(self.ctx,&tree,&choices);
        }
    }

    fn layout_pass(&mut self, reuse: Option<&crate::incremental::Reuse>) -> LayoutTree {
        let debug = crate::incremental::debug_timing();
        let t0 = std::time::Instant::now();
        let builder = Builder::new(self.ctx, reuse);
        let mut raw_root = builder.build_root();
        self.anchor_areas.apply_lengths(&mut raw_root);
        let mut root = crate::wm::prepare(raw_root, self.ctx.doc.body());
        let t_build = t0.elapsed();
        let uses_counters = builder.uses_counters();
        let vp_w = self.ctx.viewport_width.max(1.0);
        let vp_h = self.ctx.viewport_height.max(1.0);
        let cb = Cb {
            x: 0.0,
            width: vp_w,
            height: Some(vp_h),
        };
        let mut bfc = Bfc::new();
        let top = self.effective_top_margin(&root, cb.width);
        self.layout_block(&mut root, &cb, top, &mut bfc, WidthMode::Fill);
        let t_layout = t0.elapsed();
        let mut doc_h = (root.rect.bottom() + root.collapsed_bottom_margin.max(0.0)).max(vp_h);
        let icb = Rect::new(0.0, 0.0, vp_w, doc_h);
        if root.style.position == Position::Static {
            self.layout_abs_descendants(&mut root, icb);
        }
        let root_definite = height_definite(&root, true);
        self.apply_relative(&mut root, vp_w, Some(vp_h), root_definite);
        if self.ctx.styles.any_motion_path { crate::motion::apply(&root, Rect::new(0.0, 0.0, vp_w, vp_h)); }
        let (max_r, max_b) = bounds(&root);
        doc_h = doc_h.max(max_b);
        if debug {
            let fresh = crate::incremental::BUILD_STATS.with(|c| c.replace(0));
            eprintln!("[layout] build {:?} ({fresh} fresh elements), block layout {:?}, abs+relative+bounds {:?}", t_build, t_layout - t_build, t0.elapsed() - t_layout);
        }
        LayoutTree {
            root,
            width: vp_w.max(max_r.min(vp_w * 4.0)),
            height: doc_h,
            viewport: (vp_w, vp_h),
            uses_counters,
            resolved_anchor_styles: Default::default(),
            fragmented: Default::default(),
        }
    }

    /// Top margin of a block after collapsing with its first descendants.
    pub fn effective_top_margin(&self, b: &LayoutBox, cbw: f32) -> f32 {
        self.top_margins(b, cbw, None).resolve()
    }

    /// Width of `b`'s content box for resolving its children's percentage
    /// margins ahead of layout (approximate: ignores floats and shrink-to-fit).
    fn inner_width_estimate(&self, b: &LayoutBox, cbw: f32) -> f32 {
        let inner_w = (cbw - horizontal_extras(&b.style, cbw) - b.style.margin.left.resolve_or_zero(cbw) - b.style.margin.right.resolve_or_zero(cbw)).max(0.0);
        b.style.width.resolve(cbw).unwrap_or(inner_w)
    }

    /// Definite content height of `b` for its children's percentages.
    fn inner_height_estimate(&self, b: &LayoutBox, cb_h: Option<f32>) -> Option<f32> {
        match b.style.height {
            Length::Px(v) => Some(v),
            Length::Percent(p) => cb_h.map(|h| h * p / 100.0),
            l @ (Length::Calc(..) | Length::Math(_)) => cb_h.map(|h| l.resolve_or_zero(h)),
            _ => None,
        }
    }

    /// The margins adjoining `b`'s top margin from the inside: its own top
    /// margin, its first in-flow child's (recursively, while no border,
    /// padding or BFC boundary separates them) and the margins of empty
    /// blocks collapsing through on the way (CSS 2.1 §8.3.1).
    pub(crate) fn top_margins(&self, b: &LayoutBox, cbw: f32, cb_h: Option<f32>) -> Margins {
        if self.collapses_through(b, cb_h) {
            return self.all_margins(b, cbw, cb_h);
        }
        let m = Margins::of(b.style.margin.top.resolve_or_zero(cbw));
        if !b.flow.is_in_flow() || !self.top_collapsible(b, cbw) {
            return m;
        }
        let inner_w = self.inner_width_estimate(b, cbw);
        let inner_h = self.inner_height_estimate(b, cb_h);
        m.join(self.leading_child_margins(b, inner_w, inner_h))
    }

    /// Margins of `b`'s leading in-flow children that adjoin its content
    /// top: empty blocks collapsing through, then the first non-empty one's
    /// top margin chain.
    fn leading_child_margins(&self, b: &LayoutBox, inner_w: f32, inner_h: Option<f32>) -> Margins {
        let mut m = Margins::default();
        if b.style.margin_trim & rabbithole_style::MARGIN_TRIM_BLOCK_START != 0 {
            // `margin-trim: block-start`: they are trimmed to zero.
            return m;
        }
        for c in b.children.iter().filter(|c| c.is_in_flow()) {
            if !matches!(c.kind, BoxKind::Block | BoxKind::AnonymousBlock | BoxKind::Table | BoxKind::Image) {
                break;
            }
            if self.collapses_through(c, inner_h) {
                if c.style.clear != Clear::None {
                    break;
                }
                m = m.join(self.all_margins(c, inner_w, inner_h));
                continue;
            }
            m = m.join(self.top_margins(c, inner_w, inner_h));
            break;
        }
        m
    }

    /// All margins of a box whose margins collapse through it, including its
    /// (empty) descendants'.
    fn all_margins(&self, b: &LayoutBox, cbw: f32, cb_h: Option<f32>) -> Margins {
        let mut m = Margins::of(b.style.margin.top.resolve_or_zero(cbw)).add(b.style.margin.bottom.resolve_or_zero(cbw));
        // (Trimmed children's margins are zero.)
        if b.formatting == Formatting::Block && b.style.margin_trim & (rabbithole_style::MARGIN_TRIM_BLOCK_START | rabbithole_style::MARGIN_TRIM_BLOCK_END) == 0 {
            let inner_w = self.inner_width_estimate(b, cbw);
            let inner_h = self.inner_height_estimate(b, cb_h);
            for c in b.children.iter().filter(|c| c.is_in_flow()) {
                m = m.join(self.all_margins(c, inner_w, inner_h));
            }
        }
        m
    }

    /// Is `b` an empty block whose top and bottom margins are adjoining
    /// (zero/auto height, zero min-height, no borders/padding vertically,
    /// no line boxes and only such children; not a BFC root)?
    pub(crate) fn collapses_through(&self, b: &LayoutBox, cb_h: Option<f32>) -> bool {
        if !b.flow.is_in_flow() || b.bfc_root || !matches!(b.kind, BoxKind::Block | BoxKind::AnonymousBlock) {
            return false;
        }
        let s = &b.style;
        if !zero_or_auto_height(s.height, cb_h) || !zero_or_auto_height(s.min_height, cb_h) {
            return false;
        }
        if s.border_width.top > 0.0 || s.border_width.bottom > 0.0 || !is_zero_len(s.padding.top) || !is_zero_len(s.padding.bottom) {
            return false;
        }
        match b.formatting {
            Formatting::Inline => inline_content_is_empty(b),
            Formatting::Block => {
                let inner_h = self.inner_height_estimate(b, cb_h);
                b.children.iter().filter(|c| c.is_in_flow()).all(|c| self.collapses_through(c, inner_h))
            }
            _ => false,
        }
    }

    fn top_collapsible(&self, b: &LayoutBox, cbw: f32) -> bool {
        !b.bfc_root
            && b.formatting == Formatting::Block
            && b.style.padding.top.resolve_or_zero(cbw) == 0.0
            && b.style.border_width.top == 0.0
    }

    /// Lay out `b` as a block-level box whose border-box top is at `top`.
    pub fn layout_block(&mut self, b: &mut LayoutBox, cb: &Cb, top: f32, bfc: &mut Bfc, mode: WidthMode) {
        if b.wm_wrapper {
            let _ = bfc;
            self.forced_height_indefinite = false;
            let forced = self.forced_content_height.take();
            return self.layout_wm_wrapper(b, cb, top, mode, forced);
        }
        // An independent formatting context does not take part in an
        // ancestor's line clamping.
        if b.bfc_root && self.clamp.is_some() {
            let saved = self.clamp.take();
            self.layout_block(b, cb, top, bfc, mode);
            self.clamp = saved;
            return;
        }
        let forced_h = self.forced_content_height.take(); // grid/flex hook
        let forced_indefinite = std::mem::take(&mut self.forced_height_indefinite) && forced_h.is_some();
        // Where the contents' origin is now (for reuse by translation): the
        // current content box (relative offsets included) minus the scroll
        // offset already applied to the contents. Reuse first translates
        // them back to their unscrolled origin, then applies the new offset.
        let prev_cache = b.layout_cache.take();
        let prev_origin = (b.content_rect.x - b.scroll_offset.0, b.content_rect.y - b.scroll_offset.1);
        b.rel_applied = (0.0, 0.0);
        b.scroll_offset = (0.0, 0.0);
        if b.formatting == Formatting::Table && b.style.border_collapse {
            // Collapsed borders change the table's (and cells') border widths.
            self.resolve_collapsed_borders(b);
        }
        let style = b.style.clone();
        let cbw = cb.width;
        let pad = resolve_edges(&style.padding, cbw);
        let bw = style.border_width;
        let pb_h = pad.left + pad.right + bw.left + bw.right;
        let pb_v = pad.top + pad.bottom + bw.top + bw.bottom;
        let ml = style.margin.left.resolve(cbw);
        let mr = style.margin.right.resolve(cbw);
        let mt = style.margin.top.resolve_or_zero(cbw);
        let mb = style.margin.bottom.resolve_or_zero(cbw);

        let mut mode = mode;
        if b.formatting == Formatting::Table && matches!(mode, WidthMode::Fill) && style.width.is_auto() {
            mode = WidthMode::ShrinkToFit;
        }
        let (mut content_w, ml, mr, replaced_h) = if b.formatting == Formatting::TableWrapper {
            // The wrapper is as wide as its table; auto margins center it.
            let avail = cbw - ml.unwrap_or(0.0) - mr.unwrap_or(0.0);
            let w = self.wrapped_table_width(b, avail, mode);
            let rem = cbw - w;
            let (l, r) = match (mode, ml, mr) {
                (WidthMode::Fill, None, None) if rem > 0.0 => (rem / 2.0, rem / 2.0),
                (WidthMode::Fill, None, Some(r)) => ((rem - r).max(0.0), r),
                (WidthMode::Fill, Some(l), _) => (l, rem - l),
                (_, l, r) => (l.unwrap_or(0.0), r.unwrap_or(0.0)),
            };
            (w, l, r, None)
        } else if b.kind == BoxKind::Image {
            let (w, h) = self.replaced_size(b, Some(cbw), cb.height, pb_h);
            let rem = cbw - w - pb_h;
            // Auto margins of a block-level image in normal flow share the
            // remaining space; a floated image's auto margins are zero.
            let in_flow = style.display.is_block_level() && matches!(mode, WidthMode::Fill) && !matches!(b.flow, Flow::FloatLeft | Flow::FloatRight);
            let (l, r) = match (ml, mr) {
                (None, None) if in_flow && rem > 0.0 => (rem / 2.0, rem / 2.0),
                (None, Some(r)) if in_flow => ((rem - r).max(0.0), r),
                (l, r) => (l.unwrap_or(0.0), r.unwrap_or(0.0)),
            };
            (w, l, r, Some(h))
        } else {
            // A height imposed by the parent (an absolutely positioned box
            // between `top` and `bottom`) is definite for the percentage
            // heights of the contents its shrink-to-fit width measures.
            let forced_intrinsic = forced_h.is_some() && matches!(mode, WidthMode::ShrinkToFit);
            if forced_intrinsic {
                self.intrinsic_forced_h = forced_h;
            }
            let (w, l, r) = self.compute_width(b, cbw, cb.height, pb_h, ml, mr, mode);
            if forced_intrinsic {
                self.intrinsic_forced_h = None;
            }
            (w, l, r, None)
        };
        b.margin = Edges {
            top: mt,
            right: mr,
            bottom: mb,
            left: ml,
        };

        let mut x = cb.x + ml;
        let mut y = top;
        if style.clear != Clear::None {
            y = y.max(bfc.clear_y(style.clear));
        }
        // A BFC root placed next to floats narrows (or moves below them).
        // (Block-level tables too: they are as wide as their contents.)
        let table = matches!(b.formatting, Formatting::Table | Formatting::TableWrapper) && b.kind != BoxKind::InlineBlock && style.display.is_block_level();
        // An auto-height formatting context placed beside floats is checked
        // against them again once its height is known.
        let mut recheck_floats = false;
        if b.bfc_root && b.flow.is_in_flow() && (matches!(mode, WidthMode::Fill) || table) && !bfc.is_empty() {
            let known_h = self.resolve_height(&style, cb.height, pb_v);
            recheck_floats = known_h.is_none() && forced_h.is_none() && !self.refitting;
            let band_h = known_h.map(|h| h + pb_v).or(self.float_band.take()).unwrap_or(1.0);
            if table {
                (x, y, content_w) = self.table_beside_floats(b, bfc, cb, (x, y, content_w), pb_h, band_h, mode);
            } else {
                (x, y, content_w) = avoid_floats(&style, bfc, cb, (x, y, content_w), pb_h, ml, mr, band_h);
            }
        }

        b.rect = Rect::new(x, y, content_w + pb_h, 0.0);
        b.content_rect = Rect::new(x + bw.left + pad.left, y + bw.top + pad.top, content_w, 0.0);

        // A definite `height` is clamped by `min-height`/`max-height` before
        // the contents resolve percentages against it.
        let mut explicit_h = forced_h.or(replaced_h).or_else(|| self.resolve_height(&style, cb.height, pb_v).map(|h| self.clamp_height(&style, h, cb.height, pb_v))); // grid/flex hook
        // A `<textarea>` with an auto height is `rows` lines tall (2 by
        // default), whatever its value.
        if explicit_h.is_none() && b.pseudo.is_none() && self.control_kind(b) == CONTROL_TEXTAREA {
            explicit_h = self.textarea_height(b, &style);
        }
        // `aspect-ratio` with an auto height derives it from the width.
        if explicit_h.is_none() && b.kind != BoxKind::Image {
            if let Some(ratio) = style.aspect_ratio.filter(|r| *r > 0.0) {
                let (w, h_extra) = if style.box_sizing == BoxSizing::BorderBox && !style.aspect_ratio_auto { (content_w + pb_h, pb_v) } else { (content_w, 0.0) };
                explicit_h = Some((w / ratio - h_extra).max(0.0));
            }
        }
        // The base of the contents' percentage heights. Anonymous block
        // boxes are transparent to it (CSS 2.1 §10.5).
        // (Only when a child uses it, so the box's layout cache stays keyed
        // on an indefinite height otherwise.)
        // (Without a containing-block height there is nothing to pass.)
        let anonymous = b.kind == BoxKind::AnonymousBlock
            && cb.height.is_some()
            && matches!(b.formatting, Formatting::Block | Formatting::Inline)
            && b.children.iter().any(|c| c.style.height.has_percent() || c.style.min_height.has_percent() || c.style.max_height.has_percent());
        let inner_cb = Cb {
            x: b.content_rect.x,
            width: content_w,
            height: if forced_indefinite {
                None
            } else if explicit_h.is_none() && anonymous {
                cb.height
            } else {
                explicit_h
            },
        };
        let bottom_collapsible = explicit_h.is_none()
            && !b.bfc_root
            && b.formatting == Formatting::Block
            && pad.bottom == 0.0
            && bw.bottom == 0.0;
        let top_collapsible = self.top_collapsible(b, cbw);
        b.collapsed_bottom_margin = mb;
        b.bottom_margins = Margins::of(mb).pair();

        let mut local_bfc = Bfc::new();
        let content_y = b.content_rect.y;
        let origin = (b.content_rect.x, content_y);
        // Floats of the parent BFC that can reach into this box (relative to
        // its content origin): the contents can be reused only next to the
        // same floats.
        let float_env = if b.bfc_root { Vec::new() } else { bfc.relative_from(origin, content_y) };
        // Table cells are adjusted after layout (vertical-align), so their
        // contents are not where the cache expects them.
        let flex_limits = (b.formatting == Formatting::Flex && explicit_h.is_none()).then(|| self.height_limits(&style, cb.height, pb_v));
        let cacheable = b.kind != BoxKind::TableCell && self.clamp.is_none() && self.column_frame.is_none() && b.style.line_clamp.is_none()
            // Percentage min/max heights of a flex container depend on its
            // containing block, which the cache entry does not record.
            && !(flex_limits.is_some() && (style.min_height.has_percent() || style.max_height.has_percent()));
        let had_cache = prev_cache.is_some();
        // Laid out last with only another definite height: remember what the
        // contents were, to prove below whether the height changes them.
        let alt_candidate = prev_cache
            .as_ref()
            .filter(|c| {
                cacheable
                    && c.inner_h != inner_cb.height
                    && c.alt_h != Some(inner_cb.height)
                    && (c.grid_publication || !self.grid_publication.enabled())
                    && c.inner_w == inner_cb.width
                    && c.top_collapsible == top_collapsible
                    && c.bottom_collapsible == bottom_collapsible
                    && crate::float::same_floats(&c.float_env, &float_env)
            })
            .map(|c| (c.inner_h, crate::incremental::contents_fingerprint(b, prev_origin.0, prev_origin.1), c.content_h, c.collapsed_bottom_margin, c.bottom_margins, c.float_extent, c.escaped.clone()));
        let reuse = prev_cache.filter(|c| {
            cacheable
                // A measuring layout (grid tracks not published) can use one
                // that also published them: the geometry is the same.
                && (c.grid_publication || !self.grid_publication.enabled())
                && c.inner_w == inner_cb.width
                && (c.inner_h == inner_cb.height || c.alt_h == Some(inner_cb.height))
                && c.top_collapsible == top_collapsible
                && c.bottom_collapsible == bottom_collapsible
                && crate::float::same_floats(&c.float_env, &float_env)
        });
        crate::incremental::count(reuse.is_some(), had_cache);
        b.reused_layout = reuse.is_some();
        let (content_h, float_extent) = if let Some(c) = reuse {
            if !c.grid_publication {
                self.grid_layouts += 1;
            }
            // Same constraints as last time: move the old contents.
            // The contents follow their origin: their relative geometry (and
            // its memoized fingerprint) is unchanged.
            let fp = b.contents_fp.get();
            crate::table::translate_contents(b, b.content_rect.x - prev_origin.0, content_y - prev_origin.1);
            b.contents_fp.set(fp);
            b.collapsed_bottom_margin = c.collapsed_bottom_margin;
            b.bottom_margins = c.bottom_margins;
            // The widening the contents caused (the box was sized from
            // its style again above).
            if c.grown_w != 0.0 {
                b.rect.w += c.grown_w;
                b.content_rect.w += c.grown_w;
            }
            // Floats inside that belong to the parent BFC are placed again.
            bfc.extend_relative(origin, &c.escaped);
            let out = (c.content_h, c.float_extent);
            b.layout_cache = Some(c);
            out
        } else {
            let floats_before = bfc.len();
            let grid_layouts = self.grid_layouts;
            b.contents_fp.set(None);
            self.flex_limits = flex_limits;
            let content_h = if b.bfc_root {
                self.layout_contents(b, &inner_cb, &mut local_bfc, top_collapsible, bottom_collapsible)
            } else {
                self.layout_contents(b, &inner_cb, bfc, top_collapsible, bottom_collapsible)
            };
            let float_extent = if b.bfc_root && !local_bfc.is_empty() { Some(local_bfc.max_bottom() - content_y) } else { None };
            let escaped = bfc.relative_since(floats_before, origin);
            // Same contents and results as with the previous height: either
            // height is served from this entry from now on.
            let alt_h = alt_candidate.and_then(|(h, fp, ch, cbm, bms, fe, esc)| {
                (ch == content_h
                    && cbm == b.collapsed_bottom_margin
                    && bms == b.bottom_margins
                    && fe == float_extent
                    && esc == escaped
                    && fp == crate::incremental::contents_fingerprint(b, origin.0, origin.1))
                .then_some(h)
            });
            b.layout_cache = cacheable.then(|| crate::incremental::LayoutCache {
                grid_publication: self.grid_publication.enabled() || (self.grid_layouts == grid_layouts && !self.grid_publication.in_grid_probe()),
                inner_w: inner_cb.width,
                inner_h: inner_cb.height,
                top_collapsible,
                bottom_collapsible,
                content_h,
                collapsed_bottom_margin: b.collapsed_bottom_margin,
                bottom_margins: b.bottom_margins,
                float_extent,
                float_env,
                escaped,
                alt_h,
                grown_w: b.content_rect.w - inner_cb.width,
            });
            (content_h, float_extent)
        };

        // A table's (or cell's) height is a minimum: content may exceed it.
        let mut h = match explicit_h {
            Some(e) if matches!(b.formatting, Formatting::Table) || b.kind == BoxKind::TableCell => e.max(content_h),
            Some(e) => e,
            // Size containment: laid out as if empty.
            None if (style.contain_size || style.container_size) && b.kind != BoxKind::Inline => style.contain_intrinsic_height.unwrap_or(0.0),
            None => content_h,
        };
        // The ratio-dependent automatic block size still has a content
        // minimum, unless the author requested another minimum or scrolling.
        if forced_h.is_none() && b.kind != BoxKind::Image && style.height.is_auto() && style.min_height.is_auto()
            && style.aspect_ratio.is_some_and(|ratio| ratio > 0.0)
            && !matches!(style.overflow_y, rabbithole_style::Overflow::Hidden | rabbithole_style::Overflow::Scroll | rabbithole_style::Overflow::Auto)
        {
            let minimum = if style.contain_size || style.container_size { style.contain_intrinsic_height.unwrap_or(0.0) } else { content_h };
            h = h.max(minimum);
        }
        if explicit_h.is_none() {
            if let Some(fe) = float_extent {
                h = h.max(fe);
            }
            // Empty text fields, buttons and menus are one line tall.
            if b.pseudo.is_none() && h < 0.5 && self.is_line_control(b) {
                let font = self.font_for(&style);
                h = style.line_height.unwrap_or_else(|| self.ctx.fonts.normal_line_height(font, style.font_size));
            }
            // So is an empty list item with an outside marker: the marker
            // makes a line box.
            if h < 0.5 && b.marker_text.is_some() && b.lines.is_empty() && style.list_style_position == rabbithole_style::ListStylePosition::Outside && !b.children.iter().any(|c| c.is_in_flow()) {
                let font = self.font_for(&style);
                h = style.line_height.unwrap_or_else(|| self.ctx.fonts.normal_line_height(font, style.font_size));
            }
        }
        h = self.clamp_height(&style, h, cb.height, pb_v);
        b.rect.h = h + pb_v;
        b.content_rect.h = h;
        // An `<input>` shows its one line of text centered vertically in
        // its content box, whatever its height (Material fields with
        // `height: 28px; padding: 13px 15px`, short boxes with a large
        // line-height, ...). Reused contents were centered already.
        if !b.reused_layout && b.pseudo.is_none() && (h - content_h).abs() > 0.01 {
            self.align_block_contents(b, &style, h, content_h);
        }
        if explicit_h.is_some() || (h - content_h).abs() > 0.01 {
            // A definite or clamped height separates the last child's
            // bottom margin from ours.
            b.collapsed_bottom_margin = mb;
            b.bottom_margins = Margins::of(mb).pair();
        }

        self.place_marker(b);

        // The baseline only serves inline-level (atomic) boxes; block-level
        // boxes export theirs through `first/last_baseline` when asked.
        if style.display.is_inline_level() || b.kind == BoxKind::InlineBlock {
            self.set_inline_level_baseline(b, &style);
        } else {
            b.baseline = None;
        }

        if contains_positioned(&style) {
            let padding_box = b.padding_rect();
            self.layout_abs_descendants_of(b, padding_box);
        }
        b.fixed = style.position == Position::Fixed;
        self.finish_scroll_container(b);
        if let Some(fl) = b.node.filter(|_| self.ctx.styles.any_first_line).and_then(|n| self.ctx.styles.pseudo(n, rabbithole_css::PseudoElement::FirstLine)) {
            crate::first_letter::apply_first_line_color(b, fl);
        }
        if recheck_floats && b.rect.h > 1.0 {
            self.refit_beside_floats(b, cb, bfc, mode);
        }
    }

    /// An auto-height formatting context laid out beside floats must not
    /// overlap the floats beside its whole border box (CSS 2.1 §9.5): lay
    /// it out again in the space left over its height at the same place,
    /// else below the next float edge.
    #[cold]
    #[inline(never)]
    fn refit_beside_floats(&mut self, b: &mut LayoutBox, cb: &Cb, bfc: &mut Bfc, mode: WidthMode) {
        self.refitting = true;
        self.refit_loop(b, cb, bfc, mode);
        self.refitting = false;
    }

    fn refit_loop(&mut self, b: &mut LayoutBox, cb: &Cb, bfc: &mut Bfc, mode: WidthMode) {
        for _ in 0..8 {
            let (y, h) = (b.rect.y, b.rect.h);
            let (l, r) = bfc.available(y, h, cb.x, cb.width);
            if l <= b.rect.x + 0.01 && r >= b.rect.right() - 0.01 {
                return;
            }
            let (ml, mr) = (b.margin.left.max(0.0), b.margin.right.max(0.0));
            let narrower = r - l - ml - mr;
            if narrower > 0.01 && narrower + 0.01 < b.rect.w && b.style.width.is_auto() {
                // Same place, in the space left beside its whole height.
                self.float_band = Some(h);
                self.layout_block(b, cb, y, bfc, mode);
                self.float_band = None;
                continue;
            }
            match bfc.next_y(y).filter(|ny| *ny < y + h) {
                Some(ny) => {
                    self.float_band = None;
                    self.layout_block(b, cb, ny, bfc, mode);
                }
                None => return,
            }
        }
    }

    /// Center a form control's contents in its content box, or apply
    /// `align-content` to a block container's (h: used content height).
    #[inline(never)]
    fn align_block_contents(&mut self, b: &mut LayoutBox, style: &ComputedStyle, h: f32, content_h: f32) {
        // Buttons and drop-down menus center their contents too (when
        // they fit).
        let center = match self.control_kind(b) {
            CONTROL_INPUT => !b.lines.is_empty(),
            CONTROL_SELECT => !b.lines.is_empty() && h > content_h,
            CONTROL_BUTTON => h > content_h && style.display != rabbithole_style::Display::Flex && style.display != rabbithole_style::Display::Grid,
            _ => false,
        };
        if center {
            crate::table::translate_contents(b, 0.0, (h - content_h) / 2.0);
        } else if h > content_h && matches!(b.formatting, Formatting::Block | Formatting::Inline) && b.kind != BoxKind::TableCell {
            // `align-content` on a block container (CSS Align §5.4):
            // the contents as one alignment subject.
            let shift = match style.align_content {
                rabbithole_style::ContentDistribution::Center | rabbithole_style::ContentDistribution::SpaceAround | rabbithole_style::ContentDistribution::SpaceEvenly => (h - content_h) / 2.0,
                rabbithole_style::ContentDistribution::FlexEnd => h - content_h,
                _ => 0.0,
            };
            if shift > 0.0 {
                crate::table::translate_contents(b, 0.0, shift);
            }
        }
    }

    /// The baseline of an inline-level (atomic) box, for its line.
    #[inline(never)]
    fn set_inline_level_baseline(&mut self, b: &mut LayoutBox, style: &ComputedStyle) {
        // Single-line form controls clip their contents internally but keep
        // the baseline of their text (Blink, Gecko).
        let control_baseline = b.pseudo.is_none() && matches!(self.control_kind(b), CONTROL_INPUT | CONTROL_SELECT | CONTROL_BUTTON);
        // A scroll container's baseline is synthesized from its margin box,
        // except for flex and grid containers (CSS Align §9.1).
        let content_baseline = matches!(b.formatting, Formatting::Flex | Formatting::Grid) && !style.contain_size && !style.container_size;
        b.baseline = if b.clips && !control_baseline && !content_baseline {
            None
        } else if b.ruby == 1 {
            // A ruby pair aligns by its base.
            b.children.iter().find(|c| c.ruby == 2).and_then(|c| c.last_baseline()).map(|bl| bl - b.rect.y)
        } else {
            // Inline-level flex containers and tables align by their first
            // baseline (a table's is its first row's). A grid container
            // exports its items' baselines; without one it falls back to
            // its margin box here (the synthesized item baseline only
            // serves enclosing block containers).
            match b.formatting {
                Formatting::Flex | Formatting::Table | Formatting::TableWrapper => b.first_baseline(),
                Formatting::Grid => b.children.iter().rev().filter(|c| c.is_in_flow()).find_map(|c| c.last_baseline()),
                _ => b.last_baseline(),
            }
            .map(|bl| bl - b.rect.y)
        };
        // An empty one-line control has the baseline its text would have.
        if control_baseline && b.lines.iter().all(|l| l.fragments.is_empty()) && self.is_line_control(b) {
            let font = self.font_for(style);
            let m = self.ctx.fonts.line_metrics(font, style.font_size);
            let lh = style.line_height.unwrap_or_else(|| rabbithole_gfx::font::normal_line_height(m));
            let half_leading = (lh - (m.ascent + m.descent)) / 2.0;
            let top = b.content_rect.y + (b.content_rect.h - lh) / 2.0;
            b.baseline = Some(top + half_leading + m.ascent - b.rect.y);
        }
    }

    /// The content height of a `<textarea>` with an auto height: `rows`
    /// lines (2 by default), whatever its value.
    #[cold]
    #[inline(never)]
    fn textarea_height(&mut self, b: &LayoutBox, style: &ComputedStyle) -> Option<f32> {
        let n = b.node?;
        let rows = self.ctx.doc.attr(n, "rows").and_then(|r| r.trim().parse::<f32>().ok()).filter(|r| *r >= 1.0).unwrap_or(2.0);
        Some(rows.floor() * self.used_line_height(style))
    }

    /// Record the scrollable content size of clipping boxes and apply the
    /// scroll offset supplied by the engine.
    fn finish_scroll_container(&mut self, b: &mut LayoutBox) {
        if !b.clips {
            return;
        }
        let style = b.style.clone();
        // `overflow: hidden` boxes are scroll containers too: scripts can
        // scroll them (`scrollTop`), users cannot (see `user_scrollable`).
        use rabbithole_style::Overflow::{Auto, Hidden, Scroll};
        b.scroll_container = matches!(style.overflow_x, Auto | Scroll | Hidden) || matches!(style.overflow_y, Auto | Scroll | Hidden);
        let pr = b.padding_rect();
        (b.scroll_width, b.scroll_height) = scroll_extent(b);
        if !b.scroll_container {
            return;
        }
        let Some(node) = b.node else { return };
        let Some(&(ox, oy)) = self.ctx.scroll_offsets.get(&node) else { return };
        // Scripts may scroll anywhere in the CSSOM scrolling area, which
        // can reach past the overflow measured above: keep such an offset
        // (as `LayoutTree::set_scroll_offset` does) instead of clamping it
        // by a different measure on the next layout.
        extend_to_scrolling_area(b, (ox, oy));
        let max_x = (b.scroll_width - pr.w).max(0.0);
        let max_y = (b.scroll_height - pr.h).max(0.0);
        let ox = ox.clamp(0.0, max_x);
        let oy = oy.clamp(0.0, max_y);
        b.scroll_offset = (ox, oy);
        if ox != 0.0 || oy != 0.0 {
            scroll_contents(b, -ox, -oy);
        }
    }

    fn layout_contents(&mut self, b: &mut LayoutBox, cb: &Cb, bfc: &mut Bfc, top_collapsible: bool, bottom_collapsible: bool) -> f32 {
        // Monolithic boxes (and out-of-flow ones) are not broken by the
        // columns around them: nor are multi-column containers inside.
        if self.column_frame.is_some() && !b.style.multicol && !(b.flow == crate::tree::Flow::InFlow && crate::multicol::breaks_inside(b)) {
            let frame = self.column_frame.take();
            let h = self.layout_contents(b, cb, bfc, top_collapsible, bottom_collapsible);
            self.column_frame = frame;
            return h;
        }
        if matches!(b.formatting, Formatting::Inline | Formatting::Block) && (b.style.multicol || b.style.line_clamp.is_some_and(|n| n > 0)) {
            if b.style.multicol {
                return self.layout_multicol(b, cb);
            }
            return self.layout_clamped(b, cb, bfc, top_collapsible, bottom_collapsible);
        }
        self.layout_contents_inner(b, cb, bfc, top_collapsible, bottom_collapsible)
    }

    /// Lay out a line-clamp container (CSS Overflow 4 §4.4): its lines and
    /// those of its block descendants in the same formatting context are
    /// counted; the box ends after the clamp line, with a block ellipsis
    /// when content follows, and what follows is hidden.
    fn layout_clamped(&mut self, b: &mut LayoutBox, cb: &Cb, bfc: &mut Bfc, top_collapsible: bool, bottom_collapsible: bool) -> f32 {
        let saved = self.clamp.take();
        let floats = bfc.snapshot();
        let mut max_lines = b.style.line_clamp.unwrap_or(0) as usize;
        if b.style.line_clamp_auto {
            // Clamp after the last line that fits in the (max-)height.
            let style = b.style.clone();
            let pad = resolve_edges(&style.padding, cb.width);
            let pb_v = pad.top + pad.bottom + style.border_width.top + style.border_width.bottom;
            let max = self.clamp_height(&style, f32::INFINITY, cb.height, pb_v);
            let limit = self.resolve_height(&style, cb.height, pb_v).map_or(max, |h| h.min(max));
            self.clamp = Some(ClampState { remaining: usize::MAX, record: true, ..Default::default() });
            let h = self.layout_contents_inner(b, cb, bfc, top_collapsible, bottom_collapsible);
            let state = self.clamp.take().unwrap_or_default();
            let fit = state.bottoms.iter().take_while(|&&y| y <= b.content_rect.y + limit + 0.01).count();
            if !limit.is_finite() || fit == state.bottoms.len() {
                self.clamp = saved;
                return h;
            }
            bfc.restore(floats.clone());
            max_lines = fit;
        }
        self.clamp = Some(ClampState::lines(max_lines));
        let mut h = self.layout_contents_inner(b, cb, bfc, top_collapsible, bottom_collapsible);
        let mut state = self.clamp.take().unwrap_or_default();
        if state.final_hit && state.content_after && b.style.block_ellipsis.is_some() {
            // The clamp line ended its paragraph but more content follows:
            // lay out again with the ellipsis on that line.
            bfc.restore(floats);
            self.clamp = Some(ClampState { ellipsize_last: true, ..ClampState::lines(max_lines) });
            h = self.layout_contents_inner(b, cb, bfc, top_collapsible, bottom_collapsible);
            state = self.clamp.take().unwrap_or_default();
        }
        if state.hit() && b.formatting == Formatting::Inline {
            // The box ends after the last line shown. (Block children after
            // the clamp point were left out of a block container's height,
            // which keeps the end padding and borders of the boxes holding
            // the clamp line.)
            h = state.bottom.map_or(0.0, |bottom| (bottom - b.content_rect.y).max(0.0));
            b.collapsed_bottom_margin = b.margin.bottom;
            b.bottom_margins = Margins::of(b.margin.bottom).pair();
        }
        self.clamp = saved;
        h
    }

    fn layout_contents_inner(&mut self, b: &mut LayoutBox, cb: &Cb, bfc: &mut Bfc, top_collapsible: bool, bottom_collapsible: bool) -> f32 {
        match b.formatting {
            Formatting::Inline => self.layout_inline(b, cb, bfc),
            Formatting::Block => self.layout_block_children(b, cb, bfc, top_collapsible, bottom_collapsible),
            Formatting::Flex => self.layout_flex(b, cb), // grid/flex hook
            Formatting::Grid => self.layout_grid(b, cb), // grid/flex hook
            Formatting::Table => self.layout_table(b, cb),
            Formatting::TableWrapper => self.layout_table_wrapper(b, cb),
            Formatting::TableRow => {
                // A row outside a table: lay out cells as blocks.
                self.layout_block_children(b, cb, bfc, false, false)
            }
            Formatting::None => 0.0,
        }
    }

    /// Lay out block-level children; returns the content height.
    ///
    /// Vertical margins collapse per CSS 2.1 §8.3.1: `pending` holds the
    /// margins adjoining the current position (the previous sibling's
    /// collapsed bottom margin plus those of empty blocks in between). While
    /// `at_top`, the parent's top margin chain is still open — those margins
    /// were already applied when the parent was positioned.
    /// Lay out the block-level children of a formatting context root.
    pub(crate) fn layout_block_children_of(&mut self, b: &mut LayoutBox, cb: &Cb, bfc: &mut Bfc) -> f32 {
        self.layout_block_children(b, cb, bfc, false, false)
    }

    fn layout_block_children(&mut self, b: &mut LayoutBox, cb: &Cb, bfc: &mut Bfc, top_collapsible: bool, bottom_collapsible: bool) -> f32 {
        let saved = std::mem::replace(&mut self.center_blocks, b.style.text_align_webkit_center);
        let saved_rtl = std::mem::replace(&mut self.cb_rtl, b.style.direction == rabbithole_style::Direction::Rtl);
        let h = self.layout_block_children_inner(b, cb, bfc, top_collapsible, bottom_collapsible);
        self.center_blocks = saved;
        self.cb_rtl = saved_rtl;
        h
    }

    fn layout_block_children_inner(&mut self, b: &mut LayoutBox, cb: &Cb, bfc: &mut Bfc, top_collapsible: bool, bottom_collapsible: bool) -> f32 {
        let content_y = b.content_rect.y;
        let content_x = b.content_rect.x;
        let mut cursor = content_y;
        let mut pending = Margins::default();
        let mut at_top = top_collapsible;
        let mb = b.margin.bottom;
        // `margin-trim`: the margins of the leading children (empty ones,
        // then the first non-empty one) and of the trailing ones are zero.
        let trim_start = b.style.margin_trim & rabbithole_style::MARGIN_TRIM_BLOCK_START != 0;
        let trim_end = b.style.margin_trim & rabbithole_style::MARGIN_TRIM_BLOCK_END != 0;
        let mut leading = true;
        for i in 0..b.children.len() {
            match b.children[i].flow {
                Flow::Absolute => {
                    let off = if at_top { 0.0 } else { pending.resolve() };
                    let block = !b.children[i].style.display_before_blockification.is_inline_level();
                    // (In right-to-left containers the static position is
                    // the box's right edge.)
                    let rtl = b.style.direction == rabbithole_style::Direction::Rtl;
                    let mut x = content_x + if rtl { cb.width } else { 0.0 };
                    if !block {
                        // An inline-level box would start an (empty) line
                        // beside the floats there, aligned by `text-align`.
                        let (l, r) = if bfc.is_empty() { (content_x, content_x + cb.width) } else { bfc.available(cursor + off, 1.0, content_x, cb.width) };
                        use rabbithole_style::TextAlign as T;
                        x = match b.style.text_align {
                            T::Left => l,
                            T::Right => r,
                            T::Center => (l + r) / 2.0,
                            T::End => if rtl { l } else { r },
                            T::Start | T::Justify => if rtl { r } else { l },
                        };
                    }
                    b.children[i].static_pos = (x, cursor + off);
                    b.children[i].static_align = None;
                }
                Flow::FloatLeft | Flow::FloatRight => {
                    let off = if at_top { 0.0 } else { pending.resolve() };
                    let hidden = self.clamp.as_ref().is_some_and(|c| c.hit());
                    b.children[i].clamped = hidden;
                    if hidden {
                        // Floats after the clamp point are not placed.
                        if let Some(c) = self.clamp.as_mut() {
                            c.content_after = true;
                        }
                        let mut scratch = Bfc::new();
                        self.layout_float(&mut b.children[i], cb, cursor + off, &mut scratch);
                    } else {
                        self.layout_float(&mut b.children[i], cb, cursor + off, bfc);
                    }
                }
                Flow::InFlow if self.clamp.as_ref().is_some_and(|c| c.hit()) => {
                    // After the clamp point: laid out (for scripts) but
                    // hidden, and not part of the box's height.
                    if !self.collapses_through(&b.children[i], cb.height) {
                        if let Some(c) = self.clamp.as_mut() {
                            c.content_after = true;
                        }
                    }
                    self.layout_block(&mut b.children[i], cb, cursor, bfc, WidthMode::Fill);
                    b.children[i].clamped = true;
                }
                Flow::InFlow => {
                    b.children[i].clamped = false;
                    let empty = self.collapses_through(&b.children[i], cb.height);
                    let child_top = if trim_start && leading { Margins::default() } else { self.top_margins(&b.children[i], cb.width, cb.height) };
                    // Hypothetical position of the border edge (no clearance).
                    let mut top = if at_top { content_y } else { cursor + pending.join(child_top).resolve() };
                    let mut clearance = false;
                    let clear = b.children[i].style.clear;
                    if clear != Clear::None {
                        let cy = bfc.clear_y(clear);
                        if cy > top + 0.001 {
                            // Clearance (CSS 2.1 §9.5.2): the border edge goes
                            // just below the floats; margins above no longer
                            // collapse with the cleared box's top margin.
                            top = cy;
                            clearance = true;
                        }
                    }
                    self.layout_block(&mut b.children[i], cb, top, bfc, WidthMode::Fill);
                    let child = &b.children[i];
                    if empty && !clearance && child.rect.h <= 0.001 {
                        // Margins collapse through the empty box.
                        if !at_top {
                            pending = pending.join(child_top);
                        }
                    } else {
                        at_top = false;
                        leading = false;
                        cursor = child.rect.bottom();
                        pending = if empty { Margins::of(child.margin.bottom) } else { Margins::from_pair(child.bottom_margins) };
                    }
                }
            }
        }
        if trim_end {
            pending = Margins::default();
        }
        if at_top {
            // No in-flow content separated the parent's margins.
            b.collapsed_bottom_margin = mb;
            b.bottom_margins = Margins::of(mb).pair();
            return 0.0;
        }
        if bottom_collapsible {
            let m = pending.add(mb);
            b.collapsed_bottom_margin = m.resolve();
            b.bottom_margins = m.pair();
            cursor - content_y
        } else {
            b.collapsed_bottom_margin = mb;
            b.bottom_margins = Margins::of(mb).pair();
            (cursor + pending.resolve() - content_y).max(0.0)
        }
    }

    /// CSS 2.1 §10.3 width resolution for non-replaced boxes. Returns
    /// (content width, margin-left, margin-right).
    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    fn compute_width(&mut self, b: &mut LayoutBox, cbw: f32, cb_h: Option<f32>, pb_h: f32, ml: Option<f32>, mr: Option<f32>, mode: WidthMode) -> (f32, f32, f32) {
        let intrinsic_limit = |l: Length| l.is_intrinsic() || rabbithole_style::is_fit_content_fn(l);
        if !intrinsic_limit(b.style.min_width) && !intrinsic_limit(b.style.max_width) && !rabbithole_style::is_fit_content_fn(b.style.width) {
            return self.compute_width_inner(b, cbw, cb_h, pb_h, ml, mr, mode);
        }
        self.compute_width_with_limits(b, cbw, cb_h, pb_h, ml, mr, mode)
    }

    /// [`Self::compute_width`] when min/max-width or `fit-content()` need
    /// the box's intrinsic widths.
    #[cold]
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn compute_width_with_limits(&mut self, b: &mut LayoutBox, cbw: f32, cb_h: Option<f32>, pb_h: f32, ml: Option<f32>, mr: Option<f32>, mode: WidthMode) -> (f32, f32, f32) {
        let sizes = self.intrinsic_in(b, cb_h);
        let saved = self.width_intrinsic.replace(sizes);
        let r = self.compute_width_inner(b, cbw, cb_h, pb_h, ml, mr, mode);
        self.width_intrinsic = saved;
        r
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    fn compute_width_inner(&mut self, b: &mut LayoutBox, cbw: f32, cb_h: Option<f32>, pb_h: f32, ml: Option<f32>, mr: Option<f32>, mode: WidthMode) -> (f32, f32, f32) {
        let style = b.style.clone();
        // A width imposed by the parent (flex items, cells): nothing else
        // about the width matters.
        if let WidthMode::Fixed(w) = mode {
            return (self.clamp_width(&style, w.max(0.0), cbw, pb_h), ml.unwrap_or(0.0), mr.unwrap_or(0.0));
        }
        if matches!(style.width, Length::Math(_)) {
            if let Some(r) = self.compute_sizing_keyword_width(&style, cbw, pb_h, ml, mr, mode) {
                return r;
            }
        }
        self.compute_width_general(b, &style, cbw, cb_h, pb_h, ml, mr, mode)
    }

    /// `width: stretch` and `width: fit-content(<length-percentage>)`.
    #[cold]
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn compute_sizing_keyword_width(&mut self, style: &ComputedStyle, cbw: f32, pb_h: f32, ml: Option<f32>, mr: Option<f32>, mode: WidthMode) -> Option<(f32, f32, f32)> {
        // `width: stretch`: the margin box fills the containing block
        // (auto margins count as zero).
        if rabbithole_style::is_stretch(style.width) {
            let (l, r) = (ml.unwrap_or(0.0), mr.unwrap_or(0.0));
            let w = self.clamp_width(style, (cbw - l - r - pb_h).max(0.0), cbw, pb_h);
            return Some((w, l, r));
        }
        // `width: fit-content(<length-percentage>)`.
        if let (Some(arg), Some((mn, mx))) = (rabbithole_style::fit_content_arg(style.width), self.width_intrinsic) {
            let a = arg.eval(cbw);
            let a = if style.box_sizing == BoxSizing::BorderBox { (a - pb_h).max(0.0) } else { a.max(0.0) };
            let w = self.clamp_width(style, mx.min(a.max(mn)), cbw, pb_h);
            let rem = cbw - w - pb_h;
            return Some(match (ml, mr) {
                (None, None) if matches!(mode, WidthMode::Fill) && rem > 0.0 => (w, rem / 2.0, rem / 2.0),
                (None, Some(r)) if matches!(mode, WidthMode::Fill) => (w, (rem - r).max(0.0), r),
                (l, r) => (w, l.unwrap_or(0.0), r.unwrap_or(0.0)),
            });
        }
        None
    }

    /// Width of a box that is not given one by its parent.
    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    fn compute_width_general(&mut self, b: &mut LayoutBox, style: &std::rc::Rc<ComputedStyle>, cbw: f32, cb_h: Option<f32>, pb_h: f32, ml: Option<f32>, mr: Option<f32>, mode: WidthMode) -> (f32, f32, f32) {
        let style = style.clone();
        let mut specified = style.width.resolve(cbw).map(|w| {
            let w = if style.box_sizing == BoxSizing::BorderBox { w - pb_h } else { w };
            self.clamp_width(&style, w.max(0.0), cbw, pb_h)
        });
        // Widths transferred from min/max-height through `aspect-ratio`.
        let mut ratio_limits: Option<(f32, f32)> = None;
        // Intrinsic sizing keywords.
        if style.width.is_intrinsic() {
            let (min, max) = self.intrinsic_in(b, cb_h);
            let ml0 = ml.unwrap_or(0.0);
            let mr0 = mr.unwrap_or(0.0);
            let avail = (cbw - ml0 - mr0 - pb_h).max(0.0);
            let w = match style.width {
                Length::MinContent => min,
                Length::MaxContent => max,
                _ => shrink_to_fit(min, max, avail),
            };
            specified = Some(self.clamp_width(&style, w, cbw, pb_h));
        } else if style.width.is_auto() && b.kind != BoxKind::Image {
            if let Some(ratio) = style.aspect_ratio.filter(|r| *r > 0.0) {
                let pad = resolve_edges(&style.padding, cbw);
                let pb_v = pad.top + pad.bottom + style.border_width.top + style.border_width.bottom;
                let to_w = |h: f32| if style.box_sizing == BoxSizing::BorderBox && !style.aspect_ratio_auto { ((h + pb_v) * ratio - pb_h).max(0.0) } else { h * ratio };
                let (h_min, h_max) = self.height_limits(&style, cb_h, pb_v);
                if let Some(h) = self.resolve_height(&style, cb_h, pb_v) {
                    // A definite height (clamped by min/max-height)
                    // transfers to the width; the automatic minimum width
                    // keeps the contents' min-content width.
                    let mut w = to_w(h.clamp(h_min, h_max));
                    if style.min_width.is_auto() && !crate::flex::is_scroll_container(&style) {
                        w = w.max(self.intrinsic_in(b, cb_h).0);
                    }
                    specified = Some(self.clamp_width(&style, w, cbw, pb_h));
                } else {
                    // min/max-height transfer to an automatic width.
                    ratio_limits = Some((if h_min > 0.0 { to_w(h_min) } else { 0.0 }, if h_max.is_finite() { to_w(h_max) } else { f32::INFINITY }));
                }
            }
        }
        // An auto-width block aligned by `justify-self` is fit-content
        // sized (CSS Align §6.1, Sizing §3.3).
        if specified.is_none() && matches!(mode, WidthMode::Fill) && style.width.is_auto() && !matches!(style.justify_self, ItemAlign::Auto | ItemAlign::Baseline)
            && b.kind == BoxKind::Block
            && abs_align_fraction(style.justify_self, rabbithole_style::AlignMods::of_item(style.justify_self, style.justify_self_flags), false, false, true).is_some()
        {
            let (min, max) = self.intrinsic_in(b, cb_h);
            let avail = (cbw - ml.unwrap_or(0.0) - mr.unwrap_or(0.0) - pb_h).max(0.0);
            specified = Some(self.clamp_width(&style, shrink_to_fit(min, max, avail), cbw, pb_h));
        }
        match mode {
            WidthMode::Fixed(w) => (self.clamp_width(&style, w.max(0.0), cbw, pb_h), ml.unwrap_or(0.0), mr.unwrap_or(0.0)),
            WidthMode::ShrinkToFit => {
                let (ml, mr) = (ml.unwrap_or(0.0), mr.unwrap_or(0.0));
                let w = match specified {
                    Some(w) => w,
                    None => {
                        let (min, max) = self.intrinsic_in(b, cb_h);
                        let avail = (cbw - ml - mr - pb_h).max(0.0);
                        let w = shrink_to_fit(min, max, avail);
                        let w = ratio_limits.map(|(lo, hi)| w.min(hi).max(lo)).unwrap_or(w);
                        self.clamp_width(&style, w, cbw, pb_h)
                    }
                };
                (w, ml, mr)
            }
            WidthMode::Fill => match specified.or_else(|| {
                // Auto width fills the line; when min/max-width changes it,
                // the result is used as the specified width so `auto`
                // margins share the remaining space (CSS 2.1 §10.4).
                let w = (cbw - ml.unwrap_or(0.0) - mr.unwrap_or(0.0) - pb_h).max(0.0);
                let limited = ratio_limits.map(|(lo, hi)| w.min(hi).max(lo)).unwrap_or(w);
                let clamped = self.clamp_width(&style, limited, cbw, pb_h);
                if (clamped - w).abs() > 0.01 {
                    Some(clamped)
                } else {
                    None
                }
            }) {
                None => {
                    let (ml, mr) = (ml.unwrap_or(0.0), mr.unwrap_or(0.0));
                    let w = (cbw - ml - mr - pb_h).max(0.0);
                    (w, ml, mr)
                }
                Some(w) => {
                    let rem = cbw - w - pb_h;
                    // `-webkit-center` parents center the margin box of
                    // children with fixed margins (Blink).
                    if let (Some(l), Some(r), true) = (ml, mr, self.center_blocks) {
                        let start = ((rem - l - r) / 2.0).max(0.0) + l;
                        return (w, start, rem - start);
                    }
                    match (ml, mr) {
                        (None, None) => {
                            if rem > 0.0 {
                                (w, rem / 2.0, rem / 2.0)
                            } else {
                                (w, 0.0, rem)
                            }
                        }
                        (None, Some(r)) => (w, (rem - r).max(0.0), r),
                        (Some(l), None) => (w, l, rem - l),
                        // (Baseline self-alignment does not apply to blocks.)
                        (Some(l), Some(r)) if !matches!(style.justify_self, ItemAlign::Auto | ItemAlign::Baseline) => {
                            // `justify-self` aligns a sized block in its
                            // containing block (CSS Align §6.1).
                            let own_rtl = style.direction == rabbithole_style::Direction::Rtl;
                            match abs_align_fraction(style.justify_self, rabbithole_style::AlignMods::of_item(style.justify_self, style.justify_self_flags), self.cb_rtl, own_rtl, true) {
                                Some(f) => {
                                    let left = l + (rem - l - r) * f;
                                    (w, left, rem - left)
                                }
                                None if self.cb_rtl => (w, rem - r, r),
                                None => (w, l, rem - l),
                            }
                        }
                        (Some(_), Some(r)) if self.cb_rtl => (w, rem - r, r),
                        (Some(l), Some(_)) => (w, l, rem - l),
                    }
                }
            },
        }
    }

    #[inline]
    pub(crate) fn clamp_width(&self, style: &ComputedStyle, w: f32, cbw: f32, pb_h: f32) -> f32 {
        if sizing_keyword(style.max_width) || sizing_keyword(style.min_width) {
            return self.clamp_width_keywords(style, w, cbw, pb_h);
        }
        let adj = |v: f32| if style.box_sizing == BoxSizing::BorderBox { (v - pb_h).max(0.0) } else { v };
        let mut w = w;
        if let Some(max) = style.max_width.resolve(cbw) {
            w = w.min(adj(max));
        }
        if let Some(min) = style.min_width.resolve(cbw) {
            w = w.max(adj(min));
        }
        w.max(0.0)
    }

    /// [`Self::clamp_width`] with intrinsic, `stretch` or `fit-content()`
    /// limits.
    #[cold]
    #[inline(never)]
    fn clamp_width_keywords(&self, style: &ComputedStyle, w: f32, cbw: f32, pb_h: f32) -> f32 {
        let adj = |v: f32| if style.box_sizing == BoxSizing::BorderBox { (v - pb_h).max(0.0) } else { v };
        let mut w = w;
        // Intrinsic keywords and fit-content() need the box's min/max-content
        // widths, which `compute_width` provides.
        let intrinsic = |l: Length| -> Option<f32> {
            if rabbithole_style::is_stretch(l) {
                let m = style.margin.left.resolve_or_zero(cbw) + style.margin.right.resolve_or_zero(cbw);
                return Some((cbw - m - pb_h).max(0.0));
            }
            let (mn, mx) = self.width_intrinsic?;
            match l {
                Length::MinContent => Some(mn),
                Length::MaxContent => Some(mx),
                Length::FitContent => Some(mx.min((cbw - pb_h).max(mn))),
                Length::Math(_) => rabbithole_style::fit_content_arg(l).map(|a| mx.min(adj(a.eval(cbw)).max(mn))),
                _ => None,
            }
        };
        if let Some(max) = intrinsic(style.max_width).or_else(|| style.max_width.resolve(cbw).map(adj)) {
            w = w.min(max);
        }
        if let Some(min) = intrinsic(style.min_width).or_else(|| style.min_width.resolve(cbw).map(adj)) {
            w = w.max(min);
        }
        w.max(0.0)
    }

    #[inline]
    /// Place a block-level table (or table wrapper) beside floats: at the
    /// first position where its width for the space left by the floats
    /// fits there (CSS 2.1 §9.5).
    #[allow(clippy::too_many_arguments)]
    #[cold]
    fn table_beside_floats(&mut self, b: &mut LayoutBox, bfc: &Bfc, cb: &Cb, (x, y, content_w): (f32, f32, f32), pb_h: f32, band_h: f32, mode: WidthMode) -> (f32, f32, f32) {
        let style = b.style.clone();
        let (sml, smr) = (style.margin.left.resolve(cb.width).unwrap_or(0.0), style.margin.right.resolve(cb.width).unwrap_or(0.0));
        let mut yy = y;
        for _ in 0..100 {
            let (l, r) = bfc.available(yy, band_h.max(1.0), cb.x, cb.width);
            if l <= cb.x && r >= cb.x + cb.width {
                return (x, yy, content_w);
            }
            // Only an auto width depends on the space left (percentages
            // resolve against the containing block).
            let auto = if b.formatting == Formatting::TableWrapper {
                b.children.iter().find(|c| c.formatting == Formatting::Table).is_some_and(|t| t.style.width.is_auto())
            } else {
                style.width.is_auto()
            };
            let w = if !auto {
                content_w
            } else if b.formatting == Formatting::TableWrapper {
                self.wrapped_table_width(b, (r - l - sml - smr).max(0.0), mode)
            } else {
                self.compute_width(b, r - l, cb.height, pb_h, Some(sml), Some(smr), mode).0
            };
            if w + pb_h + sml + smr <= r - l + 0.01 {
                return (l + sml, yy, w);
            }
            match bfc.next_y(yy) {
                Some(ny) => yy = ny,
                None => break,
            }
        }
        (x, y, content_w)
    }

    fn resolve_height(&self, style: &ComputedStyle, cb_h: Option<f32>, pb_v: f32) -> Option<f32> {
        let adj = |v: f32| if style.box_sizing == BoxSizing::BorderBox { (v - pb_v).max(0.0) } else { v };
        match style.height {
            Length::Px(v) => Some(adj(v.max(0.0))),
            Length::Percent(p) => cb_h.map(|h| adj(h * p / 100.0)),
            l @ Length::Math(_) if rabbithole_style::is_stretch(l) => cb_h.map(|h| stretch_height(style, h, pb_v)),
            l @ (Length::Calc(..) | Length::Math(_)) => cb_h.map(|h| adj(l.resolve_or_zero(h).max(0.0))),
            Length::Auto | Length::MinContent | Length::MaxContent | Length::FitContent => None,
        }
    }

    /// Content-box (min, max) heights (`max` is infinite for `none`).
    #[inline]
    pub(crate) fn height_limits(&self, style: &ComputedStyle, cb_h: Option<f32>, pb_v: f32) -> (f32, f32) {
        if style.min_height.is_auto() && style.max_height.is_auto() {
            return (0.0, f32::INFINITY);
        }
        self.height_limits_general(style, cb_h, pb_v)
    }

    #[inline(never)]
    fn height_limits_general(&self, style: &ComputedStyle, cb_h: Option<f32>, pb_v: f32) -> (f32, f32) {
        let adj = |v: f32| if style.box_sizing == BoxSizing::BorderBox { (v - pb_v).max(0.0) } else { v.max(0.0) };
        let limit = |l: Length| if rabbithole_style::is_stretch(l) { cb_h.map(|h| stretch_height(style, h, pb_v)) } else { l.resolve_definite(cb_h).map(adj) };
        let max = limit(style.max_height).unwrap_or(f32::INFINITY);
        let min = limit(style.min_height).unwrap_or(0.0);
        (min, max.max(min))
    }

    #[inline]
    pub(crate) fn clamp_height(&self, style: &ComputedStyle, h: f32, cb_h: Option<f32>, pb_v: f32) -> f32 {
        if matches!(style.max_height, Length::Math(_)) || matches!(style.min_height, Length::Math(_)) {
            // (`stretch` limits are math lengths.)
            let (min, max) = self.height_limits(style, cb_h, pb_v);
            return h.min(max).max(min).max(0.0);
        }
        let adj = |v: f32| if style.box_sizing == BoxSizing::BorderBox { (v - pb_v).max(0.0) } else { v };
        let mut h = h;
        let max = match style.max_height {
            Length::Px(v) => Some(v),
            Length::Percent(p) => cb_h.map(|b| b * p / 100.0),
            l @ Length::Calc(..) => cb_h.map(|b| l.resolve_or_zero(b)),
            _ => None,
        };
        if let Some(m) = max {
            h = h.min(adj(m));
        }
        let min = match style.min_height {
            Length::Px(v) => Some(v),
            Length::Percent(p) => cb_h.map(|b| b * p / 100.0),
            l @ Length::Calc(..) => cb_h.map(|b| l.resolve_or_zero(b)),
            _ => None,
        };
        if let Some(m) = min {
            h = h.max(adj(m));
        }
        h.max(0.0)
    }

    /// Size of a replaced box's content area. `cbw == None` means
    /// percentages are treated as auto (intrinsic sizing).
    pub(crate) fn replaced_size(&self, b: &LayoutBox, cbw: Option<f32>, cb_h: Option<f32>, pb_h: f32) -> (f32, f32) {
        self.replaced_size_with(b, cbw, cb_h, pb_h, true)
    }

    /// [`Self::replaced_size`]; `width_limits: false` ignores min/max-width
    /// (a flex item's content size).
    pub(crate) fn replaced_size_with(&self, b: &LayoutBox, cbw: Option<f32>, cb_h: Option<f32>, pb_h: f32, width_limits: bool) -> (f32, f32) {
        let style = &b.style;
        // Natural size: the decoded image (in CSS px), else the `width` /
        // `height` attributes (which also give the aspect ratio before the
        // image arrives, so loading causes no layout shift).
        // `object-view-box` makes the shown region the natural size.
        let natural = b.image.as_ref().filter(|i| !i.broken).map(|img| {
            let n = img.natural_size();
            crate::background::object_view_box(style, n).map_or(n, |v| (v.w, v.h))
        });
        let fallback = if let Some(text) = b.text.as_deref().filter(|_| b.image.is_none()) {
            let font = self.ctx.fonts.select_family_stretch(&style.font_family_list, style.font_family, style.font_weight, style.is_italic(), style.font_stretch);
            let width = self.ctx.fonts.shape(font, style.font_size, text, style.letter_spacing).last().map(|g| g.x + g.advance).unwrap_or(0.0);
            let lh = style.line_height.unwrap_or_else(|| self.ctx.fonts.normal_line_height(font, style.font_size));
            (width, lh)
        } else {
            (0.0, 0.0)
        };
        let (iw, ih) = natural.unwrap_or((b.attr_size.0.unwrap_or(fallback.0), b.attr_size.1.unwrap_or(fallback.1)));
        // The shape of an alternative label is not an image aspect ratio.
        // Only decoded dimensions, a pair of attributes or explicit CSS may
        // couple the axes; height:40px must not stretch the label's width.
        // Frames have no natural aspect ratio (their default 300×150 size
        // is only a fallback size).
        let frame = b.node.is_some_and(|n| matches!(self.ctx.doc.tag(n), Some("iframe" | "frame")));
        let ratio_size = if frame || b.image.as_ref().is_some_and(|i| i.no_ratio) { (0.0, 0.0) } else { natural.unwrap_or((b.attr_size.0.unwrap_or(0.0), b.attr_size.1.unwrap_or(0.0))) };
        let natural_ratio = if ratio_size.0 > 0.0 && ratio_size.1 > 0.0 { Some(ratio_size.0 / ratio_size.1) } else { None };
        let css_ratio = style.aspect_ratio.filter(|r| *r > 0.0 && r.is_finite());
        // `auto <ratio>` prefers the natural ratio.
        let ratio = if style.aspect_ratio_auto { natural_ratio.or(css_ratio) } else { css_ratio.or(natural_ratio) };
        let adj_w = |v: f32| if style.box_sizing == BoxSizing::BorderBox { (v - pb_h).max(0.0) } else { v };
        // `box-sizing: border-box` heights include the vertical padding and
        // borders too.
        let pb_v = if style.box_sizing == BoxSizing::BorderBox {
            let base = cbw.unwrap_or(0.0);
            style.padding.top.resolve_or_zero(base) + style.padding.bottom.resolve_or_zero(base) + style.border_width.top + style.border_width.bottom
        } else {
            0.0
        };
        let adj_h = |v: f32| (v - pb_v).max(0.0);
        // `width`/`height` attributes arrive as presentational hints in the
        // computed style; an explicit `auto` (e.g. `height: auto`) must
        // keep the ratio instead of falling back to the attribute.
        let sw = match style.width {
            Length::Px(v) => Some(adj_w(v)),
            Length::Percent(p) => cbw.map(|c| adj_w(c * p / 100.0)),
            l @ (Length::Calc(..) | Length::Math(_)) => cbw.map(|c| adj_w(l.resolve_or_zero(c))),
            Length::Auto | Length::MinContent | Length::MaxContent | Length::FitContent => None,
        };
        let sh = match style.height {
            Length::Px(v) => Some(adj_h(v)),
            Length::Percent(p) => cb_h.map(|c| adj_h(c * p / 100.0)),
            l @ (Length::Calc(..) | Length::Math(_)) => cb_h.map(|c| adj_h(l.resolve_or_zero(c))),
            Length::Auto | Length::MinContent | Length::MaxContent | Length::FitContent => None,
        };
        let (iw, ih) = match (iw > 0.0, ih > 0.0, ratio) {
            (true, false, Some(r)) => (iw, iw / r),
            (false, true, Some(r)) => (ih * r, ih),
            _ => (iw, ih),
        };
        // An inline `<svg>` without width/height has only an intrinsic
        // ratio: its auto width fills the containing block (300px when that
        // is unknown, e.g. for intrinsic sizing); the height follows the
        // ratio. (Its raster's pixel size is not its CSS size.)
        // Likewise an `<object>` / `<embed>` showing an SVG document whose
        // width is a percentage or omitted (100%): it fills the containing
        // block like a nested document's viewport.
        // So does an SVG image with only a ratio (no natural width or
        // height): CSS 2.1 §10.3.2's suggested used width.
        let fill_width = b.ratio_only
            || b.image.as_ref().and_then(|i| i.vector.as_ref()).is_some_and(|v| v.width.is_none())
                && b.node.and_then(|n| self.ctx.doc.tag(n)).is_some_and(|t| matches!(t, "object" | "embed"))
            || b.image.as_ref().filter(|i| !i.broken).and_then(|i| i.vector.as_ref()).is_some_and(|v| v.width.is_none() && v.height.is_none() && v.ratio.is_some());
        let sw = match (sw, sh) {
            (None, None) if fill_width => Some(cbw.map(|c| (c - pb_h).max(0.0)).unwrap_or(300.0)),
            _ => sw,
        };
        let (mut w, mut h) = match (sw, sh) {
            (Some(w), Some(h)) => (w, h),
            (Some(w), None) => (w, ratio.map(|r| w / r).unwrap_or(ih)),
            (None, Some(h)) => (ratio.map(|r| h * r).unwrap_or(iw), h),
            (None, None) => (iw, ih),
        };
        let cbw_v = cbw.unwrap_or(0.0);
        // Intrinsic keywords of min/max-width: the width transferred from
        // a definite height, else the natural width (CSS Sizing §5.1).
        let content_w = match (sh, ratio) {
            (Some(h), Some(r)) => h * r,
            _ => iw,
        };
        let h_auto = sh.is_none();
        if let Some(max) = if !width_limits { None } else { match style.max_width {
            Length::Px(v) => Some(adj_w(v)),
            Length::Percent(p) => cbw.map(|c| adj_w(c * p / 100.0)),
            l @ (Length::Calc(..) | Length::Math(_)) => cbw.map(|c| adj_w(l.resolve_or_zero(c))),
            Length::MinContent | Length::MaxContent | Length::FitContent => Some(content_w),
            Length::Auto => None,
        } } {
            if w > max {
                if w > 0.0 && (h_auto || sw.is_none()) {
                    h *= max / w;
                }
                w = max;
            }
        }
        let min_w = if !width_limits { None } else { match style.min_width {
            Length::MinContent | Length::MaxContent | Length::FitContent => Some(content_w),
            Length::Px(v) => Some(adj_w(v)),
            l => l.resolve(cbw_v).filter(|_| cbw.is_some()).map(adj_w),
        } };
        if let Some(min) = min_w {
            if w < min {
                if w > 0.0 && (h_auto || sw.is_none()) {
                    h *= min / w;
                }
                w = min;
            }
        }
        if let Some(max_h) = style.max_height.resolve_definite(cb_h).map(adj_h) {
            if h > max_h {
                if h > 0.0 {
                    w *= max_h / h;
                }
                h = max_h;
            }
        }
        // A minimum height scales an automatic width with a natural ratio,
        // within max-width.
        let real_ratio = ratio.is_some() && b.image.as_ref().is_some_and(|i| !i.broken);
        if let (Some(min_h), true) = (style.min_height.resolve_definite(cb_h).filter(|_| !matches!(style.min_height, Length::Math(_))).map(adj_h).filter(|m| *m > 0.0), real_ratio) {
            if h < min_h {
                if h > 0.0 && sw.is_none() {
                    w *= min_h / h;
                    let max_w = if width_limits { style.max_width.resolve_definite(cbw).map(adj_w) } else { None };
                    if let Some(mw) = max_w {
                        w = w.min(mw);
                    }
                }
                h = min_h;
            }
        }
        // A CSS `aspect-ratio` of a border box (`box-sizing: border-box`,
        // not `auto <ratio>`) relates the border-box sizes.
        if let (Some(r), true, false) = (css_ratio, style.box_sizing == BoxSizing::BorderBox, style.aspect_ratio_auto) {
            if pb_v > 0.0 || pb_h > 0.0 {
                if sw.is_some() && sh.is_none() {
                    h = ((w + pb_h) / r - pb_v).max(0.0);
                } else if sw.is_none() {
                    w = ((h + pb_v) * r - pb_h).max(0.0);
                }
            }
        }
        (w.max(0.0), h.max(0.0))
    }

    /// Lay out a float child and place it in the BFC.
    pub(crate) fn layout_float(&mut self, child: &mut LayoutBox, cb: &Cb, y: f32, bfc: &mut Bfc) {
        let y = if child.style.clear != Clear::None { y.max(bfc.clear_y(child.style.clear)) } else { y };
        let mut inner = Bfc::new();
        self.layout_block(child, cb, y, &mut inner, WidthMode::ShrinkToFit);
        let m = child.margin;
        let w = child.rect.w + m.horizontal();
        let h = child.rect.h + m.vertical();
        let left = child.flow == Flow::FloatLeft;
        let (fx, fy) = bfc.place(left, w, h, y, cb.x, cb.width);
        let dx = fx + m.left - child.rect.x;
        let dy = fy + m.top - child.rect.y;
        child.translate(dx, dy);
        bfc.set_shape(child, cb.width);
    }

    /// Lay out an atomic inline-level box (inline-block / image) at a
    /// provisional origin; the caller translates it into place.
    pub(crate) fn layout_atomic(&mut self, child: &mut LayoutBox, avail_w: f32, cb_h: Option<f32>) {
        let cb = Cb {
            x: 0.0,
            width: avail_w,
            height: cb_h,
        };
        let full_width = child.attr_size.0 == Some(-1.0) && child.kind != BoxKind::Image;
        let mode = if child.kind == BoxKind::Image {
            WidthMode::Fill
        } else if full_width {
            let m = resolve_edges(&child.style.margin, avail_w);
            WidthMode::Fixed((avail_w - m.left - m.right - horizontal_extras(&child.style, avail_w)).max(0.0))
        } else {
            WidthMode::ShrinkToFit
        };
        let mut inner = Bfc::new();
        self.layout_block(child, &cb, 0.0, &mut inner, mode);
    }

    /// Lay out the absolutely positioned descendants whose containing block
    /// is `b` (padding box `cb`).
    /// Returns whether any such descendant exists.
    /// Lay out the absolutely positioned (and, when transformed, fixed)
    /// descendants `b` is the containing block of.
    pub(crate) fn layout_abs_descendants_of(&mut self, b: &mut LayoutBox, cb: Rect) {
        let saved = std::mem::replace(&mut self.fixed_cb, b.style.establishes_fixed_containing_block());
        self.layout_abs_descendants(b, cb);
        self.fixed_cb = saved;
    }

    pub(crate) fn layout_abs_descendants(&mut self, b: &mut LayoutBox, cb: Rect) -> bool {
        // The common case needs no inline geometry or allocation. Preserve
        // this fact on reused subtrees, just as the recursive walk below does.
        // A positioned descendant stops the normal absolute-position walk.
        // Fixed descendants can still belong to this transformed container.
        let through_positioned = self.fixed_cb && has_contained_fixed(b);
        if b.no_abs_below && !through_positioned {
            return false;
        }
        if !through_positioned && !has_abs_descendants(b) {
            b.no_abs_below = true;
            return false;
        }
        // Walking every line of the containing block for positioned inline
        // fragments is only needed when an inline element is the
        // containing block of some absolutely positioned box.
        let inline_cbs = if self.needs_inline_cbs() { inline_containing_blocks(b) } else { HashMap::new() };
        let mut ancestors = HashMap::new();
        let mode = if through_positioned { PositionedDescendants::AbsoluteAndFixed } else { PositionedDescendants::Absolute };
        let found = self.layout_abs_descendants_with(b, cb, &inline_cbs, &mut ancestors, mode);
        b.no_abs_below = !found;
        found
    }

    /// Does any absolutely positioned or fixed box (element, `::before`,
    /// `::after`) have an inline element as its nearest positioned
    /// ancestor? Computed once per layouter from the styles.
    fn needs_inline_cbs(&mut self) -> bool {
        if let Some(v) = self.inline_cbs_needed {
            return v;
        }
        let ctx = self.ctx;
        let styles = &ctx.styles;
        let doc = ctx.doc;
        let out_of_flow = |s: &rabbithole_style::ComputedStyle| matches!(s.position, Position::Absolute | Position::Fixed);
        let inline_cb = |mut cur: Option<rabbithole_dom::NodeId>| {
            while let Some(n) = cur {
                if let Some(s) = styles.get(n) {
                    if contains_absolutes(s) {
                        return s.display == rabbithole_style::Display::Inline;
                    }
                }
                cur = doc.flat_parent(n);
            }
            false
        };
        let mut needed = false;
        for n in 0..styles.styles.len() {
            let own = styles.styles[n].as_deref().is_some_and(out_of_flow);
            let pseudo = [&styles.before, &styles.after].iter().any(|v| v.get(n).and_then(|s| s.as_deref()).is_some_and(out_of_flow));
            if (own && inline_cb(doc.flat_parent(n))) || (pseudo && inline_cb(Some(n))) {
                needed = true;
                break;
            }
        }
        self.inline_cbs_needed = Some(needed);
        needed
    }

    fn positioned_ancestor(&self, node: rabbithole_dom::NodeId, cache: &mut HashMap<rabbithole_dom::NodeId, Option<rabbithole_dom::NodeId>>) -> Option<rabbithole_dom::NodeId> {
        if let Some(&ancestor) = cache.get(&node) {
            return ancestor;
        }
        let ancestor = if self.ctx.styles.get(node).is_some_and(|s| contains_absolutes(s)) {
            Some(node)
        } else {
            self.ctx.doc.flat_parent(node).and_then(|parent| self.positioned_ancestor(parent, cache))
        };
        cache.insert(node, ancestor);
        ancestor
    }

    fn layout_abs_descendants_with(&mut self, b: &mut LayoutBox, cb: Rect, inline_cbs: &HashMap<rabbithole_dom::NodeId, Rect>, ancestors: &mut HashMap<rabbithole_dom::NodeId, Option<rabbithole_dom::NodeId>>, mode: PositionedDescendants) -> bool {
        // Into a vertical subtree's logical space.
        let logical_cbs;
        let (cb, inline_cbs) = match b.wm_transform {
            Some(m) => {
                logical_cbs = inline_cbs.iter().map(|(&n, &r)| (n, crate::wm::to_logical(m, r))).collect();
                (crate::wm::to_logical(m, cb), &logical_cbs)
            }
            None => (cb, inline_cbs),
        };
        let mut found = false;
        for i in 0..b.children.len() {
            let fixed = b.children[i].style.position == Position::Fixed;
            if b.children[i].flow == Flow::Absolute && (mode != PositionedDescendants::FixedOnly || fixed) {
                let area = b.children[i].abs_node().and_then(|n| {
                    if mode == PositionedDescendants::FixedOnly { return None; }
                    if inline_cbs.is_empty() {
                        return self.abs_cb.get(&n).copied();
                    }
                    // Inline elements have no LayoutBox after flattening.
                    // Their first/last fragments still establish the nearest
                    // positioned ancestor's containing block, including for
                    // absolutes nested inside static inline-blocks.
                    let parent = if b.children[i].pseudo.is_some() { Some(n) } else { self.ctx.doc.flat_parent(n) };
                    parent.and_then(|p| self.positioned_ancestor(p, ancestors))
                        .and_then(|a| inline_cbs.get(&a).copied())
                        .or_else(|| self.abs_cb.get(&n).copied())
                });
                let block_level = matches!(b.formatting, Formatting::Block | Formatting::Inline) && !b.children[i].style.display_before_blockification.is_inline_level();
                // (Inline-level static positions of right-to-left lines are
                // the hypothetical box's right edge too.)
                let static_right = matches!(b.formatting, Formatting::Block | Formatting::Inline) && b.style.direction == rabbithole_style::Direction::Rtl;
                // The static-position rectangle of a block-level box: the
                // parent's content box in the inline axis, its static
                // position in the block axis (CSS Position §4.1).
                let static_rect = Some(StaticRect {
                    block_level,
                    x: b.content_rect.x,
                    w: b.content_rect.w,
                    rtl: b.style.direction == rabbithole_style::Direction::Rtl,
                    justify_items: (b.style.justify_items, rabbithole_style::AlignMods::of_item(b.style.justify_items, b.style.justify_items_flags)),
                });
                self.layout_absolute(&mut b.children[i], area.unwrap_or(cb), static_right, static_rect);
                found = true;
            }
            let child = &mut b.children[i];
            if child.style.establishes_fixed_containing_block() { continue; }
            if mode != PositionedDescendants::FixedOnly && child.style.position == Position::Static &&
                (!child.no_abs_below || mode == PositionedDescendants::AbsoluteAndFixed) {
                let f = self.layout_abs_descendants_with(child, cb, inline_cbs, ancestors, mode);
                child.no_abs_below = !f;
                found |= f;
            } else if mode == PositionedDescendants::FixedOnly ||
                (mode == PositionedDescendants::AbsoluteAndFixed && child.style.position != Position::Static) {
                // Its absolute descendants have already been positioned by
                // their own containing block. Only fixed boxes escape it.
                found |= self.layout_abs_descendants_with(child, cb, inline_cbs, ancestors, PositionedDescendants::FixedOnly);
            }
        }
        found
    }

    fn layout_absolute(&mut self, c: &mut LayoutBox, cb: Rect, static_right: bool, static_rect: Option<StaticRect>) {
        let fixed_cb = std::mem::take(&mut self.fixed_cb);
        self.layout_absolute_in(c, cb, static_right, static_rect, fixed_cb);
        self.fixed_cb = fixed_cb;
        if fixed_cb && c.style.position == Position::Fixed {
            // Positioned against a transformed ancestor, not the viewport.
            c.fixed = false;
        }
    }

    fn layout_absolute_in(&mut self, c: &mut LayoutBox, cb: Rect, static_right: bool, static_rect: Option<StaticRect>, fixed_cb: bool) {
        let cb = if c.style.position == Position::Fixed && !fixed_cb {
            Rect::new(0.0, 0.0, self.ctx.viewport_width, self.ctx.viewport_height)
        } else {
            cb
        };
        // A box in the top layer is laid out as a child of the initial
        // containing block: its static position is the ICB's origin.
        let mut static_right = static_right;
        if !self.ctx.doc.top_layer.is_empty() && c.abs_node().is_some_and(|n| self.ctx.doc.top_layer.contains(&n)) {
            c.static_pos = (0.0, 0.0);
            c.static_align = None;
            static_right = false;
        }
        let style = c.style.clone();
        let anchored = if self.anchor_areas.is_empty() { None } else {
            c.abs_node().and_then(|n| self.anchor_areas.areas.get(&n)).copied()
        };
        let cb = anchored.map_or(cb,|a| a.area);
        let auto = anchored.map(|_| 0.0);
        let l = style.inset.left.resolve(cb.w).or(auto);
        let r = style.inset.right.resolve(cb.w).or(auto);
        let t = style.inset.top.resolve(cb.h).or(auto);
        let btm = style.inset.bottom.resolve(cb.h).or(auto);
        let ml = style.margin.left.resolve_or_zero(cb.w);
        let mr = style.margin.right.resolve_or_zero(cb.w);
        let mt = style.margin.top.resolve_or_zero(cb.w);
        let mb = style.margin.bottom.resolve_or_zero(cb.w);
        let pb_h = horizontal_extras(&style, cb.w);
        let p = resolve_edges(&style.padding, cb.w);
        let pb_v = p.top + p.bottom + style.border_width.top + style.border_width.bottom;
        let ratio = style.aspect_ratio.filter(|r| *r > 0.0 && c.kind != BoxKind::Image);
        let mut ratio_stretched_height = None;
        // A table's auto width is shrink-to-fit, even between two insets.
        let table = matches!(c.formatting, Formatting::Table | Formatting::TableWrapper);
        let (mut mode, mut avail) = match (l, r, style.width.is_auto()) {
            (Some(l), Some(r), true) if table || (anchored.is_some() && style.justify_self != ItemAlign::Stretch) => (WidthMode::ShrinkToFit, (cb.w - l - r).max(0.0)),
            (Some(l), Some(r), true) => (WidthMode::Fixed((cb.w - l - r - ml - mr - pb_h).max(0.0)), cb.w),
            _ => {
                // With both insets auto the box starts at its static
                // position (CSS 2.1 §10.3.7); one left of the containing
                // block (a grid area) widens the space up to its right edge.
                let before = if l.is_none() && r.is_none() && c.static_align.is_none() && !static_right { (c.static_pos.0 + ml - cb.x).min(0.0) } else { 0.0 };
                let used = l.or(r).unwrap_or(before);
                // A specified width resolves against the whole containing
                // block; only shrink-to-fit uses the space past an inset.
                // (`stretch` and other math sizes keep the inset-modified space.)
                let specified = matches!(style.width, Length::Px(_) | Length::Percent(_) | Length::Calc(..))
                    && !matches!(style.min_width, Length::Math(_)) && !matches!(style.max_width, Length::Math(_))
                    && c.kind != BoxKind::Image && ratio.is_none() && !table;
                (WidthMode::ShrinkToFit, if specified { cb.w } else { (cb.w - used).max(0.0) })
            }
        };
        if let Some(ratio) = ratio {
            // Insets do not make both automatic axes stretch independently:
            // the chosen axis determines the other through the ratio first.
            // Keep percentage lengths based on the complete containing block.
            avail = cb.w;
            if style.width.is_auto() {
                let from_height = |height: f32| {
                    if style.box_sizing == BoxSizing::BorderBox && !style.aspect_ratio_auto { ((height + pb_v) * ratio - pb_h).max(0.0) } else { height * ratio }
                };
                let definite_h = self.resolve_height(&style, Some(cb.h), pb_v);
                let stretched_h = match (t, btm, l, r) {
                    (Some(t), Some(bt), _, _) if style.height.is_auto() && (l.is_none() || r.is_none()) => Some((cb.h - t - bt - mt - mb - pb_v).max(0.0)),
                    _ => None,
                };
                if definite_h.is_none() {
                    ratio_stretched_height = stretched_h;
                }
                let (min_content, max_content) = self.intrinsic(c);
                let mut width = if let Some(height) = definite_h.or(stretched_h) {
                    let mut width = from_height(height);
                    if style.min_width.is_auto()
                        && !matches!(style.overflow_x, rabbithole_style::Overflow::Hidden | rabbithole_style::Overflow::Scroll | rabbithole_style::Overflow::Auto)
                    {
                        width = width.max(min_content);
                    }
                    width
                } else if let (Some(left), Some(right)) = (l, r) {
                    (cb.w - left - right - ml - mr - pb_h).max(0.0)
                } else {
                    shrink_to_fit(min_content, max_content, (cb.w - l.or(r).unwrap_or(0.0) - ml - mr - pb_h).max(0.0))
                };
                // Definite constraints in the opposite axis also constrain
                // an automatic width. A minimum wins over a smaller maximum.
                let adjust_h = |height: f32| if style.box_sizing == BoxSizing::BorderBox { (height - pb_v).max(0.0) } else { height.max(0.0) };
                if let Some(max) = style.max_height.resolve(cb.h) {
                    width = width.min(from_height(adjust_h(max)));
                }
                if let Some(min) = style.min_height.resolve(cb.h) {
                    width = width.max(from_height(adjust_h(min)));
                }
                mode = WidthMode::Fixed(self.clamp_width(&style, width, cb.w, pb_h));
            }
        }
        let inner_cb = Cb {
            x: cb.x,
            width: avail,
            height: Some(cb.h),
        };
        // `top` and `bottom` with an auto height: the box fills the space
        // between them (CSS 2.1 §10.6.4; overlays, outlines, `inset: 0`).
        if let Some(height) = ratio_stretched_height {
            self.forced_content_height = Some(height);
        } else if let (Some(t), Some(bt), true) = (t, btm, style.height.is_auto()) {
            if c.kind != BoxKind::Image && ratio.is_none() {
                if anchored.is_none() || style.align_self == ItemAlign::Stretch {
                    self.forced_content_height = Some((cb.h - t - bt - mt - mb - pb_v).max(0.0));
                }
            }
        }
        let mut bfc = Bfc::new();
        self.layout_block(c, &inner_cb, cb.y, &mut bfc, mode);
        self.forced_content_height = None;
        // With both insets and a definite size, auto margins share the
        // remaining space (CSS 2.1 §10.3.7 / §10.6.4: centering with
        // `inset: 0; margin: auto`).
        // (An auto width that max-width / min-width changed counts as
        // specified: CSS 2.1 §10.4 recomputes with it.)
        let clamped_w = !table && style.width.is_auto() && matches!((l, r), (Some(l), Some(r)) if (c.rect.w + ml + mr - (cb.w - l - r)).abs() > 0.01);
        let sized_w = !style.width.is_auto() || c.kind == BoxKind::Image || ratio.is_some() || clamped_w;
        let sized_h = !style.height.is_auto() || c.kind == BoxKind::Image || ratio.is_some();
        let x = if let (Some(l), Some(r), true) = (l, r, sized_w) {
            let free = cb.w - l - r - c.rect.w;
            match (style.margin.left.is_auto(), style.margin.right.is_auto()) {
                (true, true) => cb.x + l + (free / 2.0).max(0.0),
                (true, false) => cb.x + l + free - mr,
                _ => cb.x + l + ml,
            }
        } else if let Some(l) = l {
            cb.x + l + ml
        } else if let Some(r) = r {
            cb.right() - r - mr - c.rect.w
        } else if let Some(a) = c.static_align {
            c.static_pos.0 + ml + a.factor.0 * (a.size.0 - c.rect.w - ml - mr)
        } else {
            if static_right { c.static_pos.0 - mr - c.rect.w } else { c.static_pos.0 + ml }
        };
        let y = if let (Some(t), Some(bt), true) = (t, btm, sized_h) {
            let free = cb.h - t - bt - c.rect.h;
            match (style.margin.top.is_auto(), style.margin.bottom.is_auto()) {
                (true, true) => cb.y + t + free / 2.0,
                (true, false) => cb.y + t + free - mb,
                _ => cb.y + t + mt,
            }
        } else if let Some(t) = t {
            cb.y + t + mt
        } else if let Some(bt) = btm {
            cb.bottom() - bt - mb - c.rect.h
        } else if let Some(a) = c.static_align {
            c.static_pos.1 + mt + a.factor.1 * (a.size.1 - c.rect.h - mt - mb)
        } else {
            c.static_pos.1 + mt
        };
        // Box alignment (CSS Align §6, Position §4): `justify-self` /
        // `align-self` align the box in its static-position rectangle, or
        // in the inset-modified containing block when both insets are set.
        let (mut x, mut y) = (x, y);
        if let Some(area) = anchored {
            if matches!(style.justify_self,ItemAlign::Auto | ItemAlign::Normal) && !style.margin.left.is_auto() && !style.margin.right.is_auto() {
                x = area.align[0].offset(cb.x+l.unwrap_or(0.),cb.right()-r.unwrap_or(0.),area.center.0,c.rect.w+ml+mr)+ml;
            }
            if matches!(style.align_self,ItemAlign::Auto | ItemAlign::Normal) && !style.margin.top.is_auto() && !style.margin.bottom.is_auto() {
                y = area.align[1].offset(cb.y+t.unwrap_or(0.),cb.bottom()-btm.unwrap_or(0.),area.center.1,c.rect.h+mt+mb)+mt;
            }
        }
        if let Some(sr) = static_rect {
            let own_rtl = style.direction == rabbithole_style::Direction::Rtl;
            // `auto` uses the parent's `*-items` for the static position
            // only; it is `normal` in the inset-modified containing block.
            let (js_static, jm_static) = if style.justify_self == ItemAlign::Auto { sr.justify_items } else { (style.justify_self, rabbithole_style::AlignMods::of_item(style.justify_self, style.justify_self_flags)) };
            // (`align-items` does not apply to block containers.)
            let (al_static, am_static) = (style.align_self, rabbithole_style::AlignMods::of_item(style.align_self, style.align_self_flags));
            let (js, jm) = (style.justify_self, rabbithole_style::AlignMods::of_item(style.justify_self, style.justify_self_flags));
            let (al, am) = (style.align_self, rabbithole_style::AlignMods::of_item(style.align_self, style.align_self_flags));
            let outer_w = c.rect.w + ml + mr;
            let outer_h = c.rect.h + mt + mb;
            match (l, r) {
                (None, None) if sr.block_level => {
                    if let Some(f) = abs_align_fraction(js_static, jm_static, sr.rtl, own_rtl, true) {
                        x = sr.x + (sr.w - outer_w) * f + ml;
                    }
                }
                (Some(l), Some(r)) if sized_w && !style.margin.left.is_auto() && !style.margin.right.is_auto() => {
                    if let Some(f) = abs_align_fraction(js, jm, sr.rtl, own_rtl, true) {
                        x = cb.x + l + (cb.w - l - r - outer_w) * f + ml;
                    }
                }
                _ => {}
            }
            match (t, btm) {
                // A zero-height rectangle at the static position.
                (None, None) if sr.block_level => {
                    if let Some(f) = abs_align_fraction(al_static, am_static, false, false, false) {
                        y = c.static_pos.1 - outer_h * f + mt;
                    }
                }
                (Some(t), Some(bt)) if sized_h && !style.margin.top.is_auto() && !style.margin.bottom.is_auto() => {
                    if let Some(f) = abs_align_fraction(al, am, false, false, false) {
                        y = cb.y + t + (cb.h - t - bt - outer_h) * f + mt;
                    }
                }
                _ => {}
            }
        }
        // The static position is where the box would have been in flow; it
        // must not move with the box (laying it out again would drift).
        let static_pos = c.static_pos;
        c.translate(x - c.rect.x, y - c.rect.y);
        c.static_pos = static_pos;
    }

    /// Apply `position: relative` offsets after layout. `cbh` is the
    /// containing block's height when definite (percentage `top`/`bottom`
    /// against an indefinite height compute to `auto`, CSS 2.1 §9.3.2);
    /// `definite` says whether `b`'s own height is.
    /// Apply relative offsets (CSS 2.1 §9.4.3) below and at `b`. Returns
    /// whether anything moved. Clipping boxes whose contents moved measure
    /// their scrollable overflow again (relative offsets count in it).
    fn apply_relative(&mut self, b: &mut LayoutBox, cbw: f32, cbh: Option<f32>, definite: bool) -> bool {
        let (iw, ih) = (b.content_rect.w, b.content_rect.h);
        let mut moved = false;
        if !b.reused_layout {
            // Flex and grid items are sized by their container: their
            // heights (and the container's, for them) count as definite.
            let items = matches!(b.formatting, Formatting::Flex | Formatting::Grid);
            // Anonymous blocks are transparent to percentages (CSS 2.1 §10.5).
            // Table parts resolve their children's offsets against their
            // specified height.
            let table_part = matches!(b.formatting, Formatting::Table | Formatting::TableRow) || crate::table::is_row_group(b);
            let inner_h = if b.kind == BoxKind::AnonymousBlock {
                cbh
            } else if table_part {
                match b.style.height {
                    Length::Px(h) => Some(h),
                    _ => None,
                }
            } else {
                (definite || items).then_some(ih)
            };
            for i in 0..b.children.len() {
                let d = items || height_definite(&b.children[i], definite);
                moved |= self.apply_relative(&mut b.children[i], iw, inner_h, d);
            }
            if moved && b.clips {
                self.refresh_scroll_extent(b);
            }
        }
        let mut want = (0.0, 0.0);
        if b.style.position == Position::Relative {
            let s = &b.style;
            let dx = s
                .inset
                .left
                .resolve(cbw)
                .or_else(|| s.inset.right.resolve(cbw).map(|v| -v))
                .unwrap_or(0.0);
            let vertical = |l: Length| match cbh {
                Some(h) => l.resolve(h),
                None if l.has_percent() => None,
                None => l.resolve(0.0),
            };
            let dy = vertical(s.inset.top).or_else(|| vertical(s.inset.bottom).map(|v| -v)).unwrap_or(0.0);
            want = (dx, dy);
        }
        // Boxes reused from the previous layout may carry their offset
        // already: apply only the difference.
        let (ax, ay) = b.rel_applied;
        b.translate(want.0 - ax, want.1 - ay);
        b.rel_applied = want;
        moved || want != (ax, ay)
    }

    /// Measure a clipping box's scrollable overflow again after its
    /// contents moved, keeping its scroll offset within it.
    fn refresh_scroll_extent(&mut self, b: &mut LayoutBox) {
        let size = scroll_extent(b);
        if size == (b.scroll_width, b.scroll_height) {
            return;
        }
        (b.scroll_width, b.scroll_height) = size;
        if !b.scroll_container {
            return;
        }
        let Some(&want) = b.node.and_then(|n| self.ctx.scroll_offsets.get(&n)) else { return };
        // (Offsets are measured in the unscrolled geometry.)
        let now = b.scroll_offset;
        b.scroll_offset = (0.0, 0.0);
        scroll_contents(b, now.0, now.1);
        extend_to_scrolling_area(b, want);
        let pr = b.padding_rect();
        let to = (want.0.clamp(0.0, (b.scroll_width - pr.w).max(0.0)), want.1.clamp(0.0, (b.scroll_height - pr.h).max(0.0)));
        b.scroll_offset = to;
        if to != (0.0, 0.0) {
            scroll_contents(b, -to.0, -to.1);
        }
    }

    /// Position an outside list marker next to the first line.
    fn place_marker(&mut self, b: &mut LayoutBox) {
        // The marker of a previous layout must not place this one (the
        // first baseline of a box without lines is its marker's).
        b.marker = None;
        let text = match &b.marker_text {
            Some(t) => t.clone(),
            None => return,
        };
        if b.style.list_style_position == rabbithole_style::ListStylePosition::Inside && b.formatting == Formatting::Inline {
            return; // rendered as inline content
        }
        // `::marker` may override the marker's font and color.
        let style = b
            .node
            .and_then(|n| crate::pseudo_style(self.ctx.styles, n, rabbithole_css::PseudoElement::Marker))
            .unwrap_or_else(|| b.style.clone());
        let font = self.font_for(&style);
        let size = style.font_size;
        let width = self.measurer.width(font, size, &text);
        let baseline = b.first_baseline().unwrap_or_else(|| {
            let m = self.ctx.fonts.line_metrics(font, size);
            let lh = style.line_height.unwrap_or_else(|| rabbithole_gfx::font::normal_line_height(m));
            b.content_rect.y + (lh - (m.ascent + m.descent)) / 2.0 + m.ascent
        });
        let gap = size * 0.45;
        // Next to the first line (which may have been pushed by a float).
        let line_x = b.lines.first().map(|l| l.rect.x).unwrap_or(b.content_rect.x);
        b.marker = Some(Marker {
            text,
            font,
            size,
            color: style.color,
            x: line_x - width - gap,
            baseline,
            width,
        });
    }
}

/// Does an element whose inline fragments are flattened into lines contain
/// its absolutely positioned descendants? Positioned inlines do, and so do
/// filtered ones (Filter Effects §5: `filter` / `backdrop-filter` other than
/// `none` establish a containing block for absolute descendants).
pub(crate) fn contains_absolutes(style: &rabbithole_style::ComputedStyle) -> bool {
    style.position != Position::Static
        || style.establishes_fixed_containing_block()
        || (style.display == rabbithole_style::Display::Inline && (!style.filters.is_empty() || !style.backdrop_filters.is_empty()))
}

/// Is the box the containing block of its absolutely positioned
/// descendants (positioned or transformed)?
pub(crate) fn contains_positioned(style: &rabbithole_style::ComputedStyle) -> bool {
    style.position != Position::Static || style.establishes_fixed_containing_block()
}

fn has_contained_fixed(b: &LayoutBox) -> bool {
    b.children.iter().any(|child| {
        child.style.position == Position::Fixed ||
            (!child.style.establishes_fixed_containing_block() && has_contained_fixed(child))
    })
}

fn has_abs_descendants(b: &LayoutBox) -> bool {
    b.children.iter().any(|child| {
        child.flow == Flow::Absolute
            || (child.style.position == Position::Static && !child.no_abs_below && !child.style.establishes_fixed_containing_block() && has_abs_descendants(child))
    })
}

/// Positioned inlines are flattened into line fragments. Keep the first and
/// last padding edges, rather than their union: a wider intervening line must
/// not widen the absolute containing block (CSS Positioned Layout §2.1).
fn inline_containing_blocks(b: &LayoutBox) -> HashMap<rabbithole_dom::NodeId, Rect> {
    struct Edges {
        first: Rect,
        last: Rect,
        rtl: bool,
        transform: rabbithole_gfx::Matrix,
    }
    fn collect(b: &LayoutBox, transform: rabbithole_gfx::Matrix, out: &mut HashMap<rabbithole_dom::NodeId, Edges>) {
        for line in &b.lines {
            for fragment in &line.fragments {
                let (Some(node), FragmentKind::InlineBox { style, left_edge, right_edge }) = (fragment.node, &fragment.kind) else { continue };
                if !contains_absolutes(style) {
                    continue;
                }
                // Inline fragments are border boxes; the containing block
                // starts at the padding edge.
                let left = if *left_edge { style.border_width.left } else { 0.0 };
                let right = if *right_edge { style.border_width.right } else { 0.0 };
                let rect = Rect::new(
                    fragment.rect.x + left,
                    fragment.rect.y + style.border_width.top,
                    (fragment.rect.w - left - right).max(0.0),
                    (fragment.rect.h - style.border_width.top - style.border_width.bottom).max(0.0),
                );
                out.entry(node).and_modify(|edges| edges.last = rect).or_insert(Edges {
                    first: rect,
                    last: rect,
                    rtl: style.direction == rabbithole_style::Direction::Rtl,
                    transform,
                });
            }
        }
        let transform = b.wm_transform.map(|m| rabbithole_style::multiply_matrix(transform, m)).unwrap_or(transform);
        for child in &b.children {
            if child.flow != Flow::Absolute && child.style.position == Position::Static {
                collect(child, transform, out);
            }
        }
    }
    let mut fragments = HashMap::new();
    collect(b, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], &mut fragments);
    fragments.into_iter().map(|(node, edges)| {
        let (left, right) = if edges.rtl { (edges.last.x, edges.first.right()) } else { (edges.first.x, edges.last.right()) };
        let rect = Rect::new(left, edges.first.y, (right - left).max(0.0), (edges.last.bottom() - edges.first.y).max(0.0));
        (node, rabbithole_gfx::path::transform_rect_bounds(&edges.transform, rect))
    }).collect()
}

/// Union of a box's border box, its fragments and its descendants
/// (respecting clips) — the area it occupies for scroll-size purposes.
pub(crate) fn content_bounds(b: &LayoutBox) -> Rect {
    content_bounds_scoped(b, true, true)
}

/// Is `c` in flow, or an out-of-flow box whose containing block is inside
/// the scroll container being measured (`abs_in`: some box between them
/// contains absolutely positioned boxes; `fixed_in`: fixed ones)?
#[inline]
fn contained_out_of_flow(c: &LayoutBox, abs_in: bool, fixed_in: bool) -> bool {
    if c.flow != Flow::Absolute {
        return true;
    }
    if c.style.position == Position::Fixed {
        // (A box between them with a fixed containing block makes the
        // fixed box absolutely positioned against it, never `c.fixed`.)
        fixed_in
    } else {
        abs_in
    }
}

/// [`content_bounds`] of a descendant of a scroll container, leaving out
/// out-of-flow boxes whose containing block is outside the container.
fn content_bounds_scoped(b: &LayoutBox, abs_in: bool, fixed_in: bool) -> Rect {
    // A transformed box contributes its transformed border box (its auto
    // margins, turned, must not extend the overflow).
    let transformed = b.style.has_transform();
    let mut r = if transformed { b.rect } else { b.margin_rect() };
    if b.flow == Flow::Absolute && b.fixed {
        return r;
    }
    let (clip_x, clip_y) = b.overflow_clip_axes();
    if (clip_x && clip_y) || b.wm_transform.is_some() {
        return r;
    }
    for line in &b.lines {
        // Empty fragments (an empty inline, `content: ""`) add no overflow.
        for f in line.fragments.iter().filter(|f| f.rect.w > 0.0) {
            r = r.union(&f.rect);
        }
    }
    let abs_in = abs_in || contains_absolutes(&b.style);
    let fixed_in = fixed_in || b.style.establishes_fixed_containing_block();
    for c in b.children.iter().filter(|c| !c.clamped && contained_out_of_flow(c, abs_in, fixed_in)) {
        let cr = content_bounds_scoped(c, abs_in, fixed_in);
        // A transformed empty box (nothing to draw) adds no overflow.
        if cr.w <= 0.0 && cr.h <= 0.0 || c.style.has_transform() && (cr.w <= 0.0 || cr.h <= 0.0) {
            continue;
        }
        r = r.union(&cr);
    }
    let outer = b.margin_rect();
    // A clamped line-clamp container cuts its content's overflow at its
    // bottom edge (what is left after the clamp point is hidden).
    if b.style.line_clamp.is_some() && b.children.iter().any(|c| c.clamped) && r.bottom() > outer.bottom() {
        r.h = (outer.bottom() - r.y).max(0.0);
    }
    if clip_x {
        r.x = outer.x;
        r.w = outer.w;
    }
    if clip_y {
        r.y = outer.y;
        r.h = outer.h;
    }
    b.transformed_bounds(r)
}

/// The scrolling area of box `b` (CSSOM View "scrolling area"): its padding
/// box united with the border boxes and line fragments of its content (the
/// content of nested clipping boxes is clipped by them; fixed boxes do not
/// count, nor do out-of-flow boxes whose containing block is outside
/// `b`), cut at the edges the scroll origin is at (block start and
/// inline start, mirrored for `*-reverse` flex containers). Returns the
/// (width, height) — `scrollWidth` / `scrollHeight`.
pub fn scrolling_area(b: &LayoutBox) -> (f32, f32) {
    use rabbithole_style::{Direction, Display, FlexDirection, WritingMode};
    let p = b.padding_rect();
    // Contents of a scrolled box are translated by -scroll_offset.
    let (ox, oy) = b.scroll_offset;
    let mut u = (p.x, p.y, p.x + p.w, p.y + p.h);
    fn add(u: &mut (f32, f32, f32, f32), r: &Rect, dx: f32, dy: f32) {
        if r.w <= 0.0 && r.h <= 0.0 {
            return;
        }
        u.0 = u.0.min(r.x + dx);
        u.1 = u.1.min(r.y + dy);
        u.2 = u.2.max(r.x + r.w + dx);
        u.3 = u.3.max(r.y + r.h + dy);
    }
    fn walk(bx: &LayoutBox, u: &mut (f32, f32, f32, f32), dx: f32, dy: f32, columns: bool, abs_in: bool, fixed_in: bool) {
        let abs_in = abs_in || contains_absolutes(&bx.style);
        let fixed_in = fixed_in || bx.style.establishes_fixed_containing_block();
        for line in &bx.lines {
            for f in &line.fragments {
                add(u, &f.rect, dx, dy);
            }
        }
        for c in &bx.children {
            // Fixed boxes, and boxes hidden after a line-clamp point, add no
            // scrollable overflow; a clamped container cuts its content.
            if c.fixed || c.clamped || !contained_out_of_flow(c, abs_in, fixed_in) {
                continue;
            }
            // A transformed box counts with its transformed bounds.
            if c.style.has_transform() {
                let b = content_bounds(c);
                add(u, &b, dx, dy);
                continue;
            }
            // Inside a box (not the multi-column container itself), a
            // descendant broken across columns counts unbroken.
            if let Some(r) = c.unfragmented_rect().filter(|_| !columns) {
                add(u, &r, dx, dy);
                continue;
            }
            add(u, &c.rect, dx, dy);
            let clamp_cut = c.style.line_clamp.is_some() && c.children.iter().any(|x| x.clamped);
            if !c.clips && !clamp_cut {
                walk(c, u, dx, dy, columns || c.style.multicol, abs_in, fixed_in);
            }
        }
    }
    walk(b, &mut u, ox, oy, b.style.multicol, false, false);
    let s = &b.style;
    let vertical = matches!(s.writing_mode, WritingMode::VerticalRl | WritingMode::VerticalLr | WritingMode::SidewaysLr);
    let rtl = s.direction == Direction::Rtl;
    // Where the scroll origin is: the right edge / the bottom edge.
    let mut from_right = (!vertical && rtl) || s.writing_mode == WritingMode::VerticalRl;
    let mut from_bottom = vertical && rtl;
    if matches!(s.display, Display::Flex | Display::InlineFlex) {
        let reverse = matches!(s.flex_direction, FlexDirection::RowReverse | FlexDirection::ColumnReverse);
        let main_vertical = matches!(s.flex_direction, FlexDirection::Row | FlexDirection::RowReverse) == vertical;
        if reverse {
            if main_vertical {
                from_bottom = !from_bottom;
            } else {
                from_right = !from_right;
            }
        }
    }
    let w = if from_right { p.x + p.w - u.0 } else { u.2 - p.x };
    let h = if from_bottom { p.y + p.h - u.1 } else { u.3 - p.y };
    (w.max(p.w), h.max(p.h))
}


/// Grow the recorded scroll size of a scroll container to its CSSOM
/// scrolling area when `offset` reaches past it (computed only then).
pub(crate) fn extend_to_scrolling_area(b: &mut LayoutBox, offset: (f32, f32)) {
    let pr = b.padding_rect();
    if offset.0 <= b.scroll_width - pr.w && offset.1 <= b.scroll_height - pr.h {
        return;
    }
    let area = scrolling_area(b);
    b.scroll_width = b.scroll_width.max(area.0);
    b.scroll_height = b.scroll_height.max(area.1);
}

/// The scrollable overflow size of a clipping box (its padding box united
/// with its contents' bounds and end padding), measured with its contents
/// at their unscrolled position.
fn scroll_extent(b: &LayoutBox) -> (f32, f32) {
    let style = &b.style;
    let (sx, sy) = b.scroll_offset;
    let pr = b.padding_rect();
    let mut max_r = pr.right();
    let mut max_b = pr.bottom();
    // Boxes hidden after a line-clamp point add no scrollable overflow,
    // nor do out-of-flow boxes whose containing block is an ancestor
    // (CSS Overflow 3 §2.2: they are not scrolled by this box; they
    // are also positioned only after it is laid out, so their
    // geometry here would be whatever an earlier layout left).
    let abs_in = contains_absolutes(style);
    let fixed_in = style.establishes_fixed_containing_block();
    for c in b.children.iter().filter(|c| !c.clamped && contained_out_of_flow(c, abs_in, fixed_in)) {
        let r = content_bounds_scoped(c, abs_in, fixed_in);
        // An empty box adds no overflow (as in `content_bounds`), nor do
        // the auto margins of one (moved by a relative offset).
        let empty = c.rect.w <= 0.0 && c.rect.h <= 0.0 && c.children.is_empty() && c.lines.is_empty();
        if empty || r.w <= 0.0 && r.h <= 0.0 || c.style.has_transform() && (r.w <= 0.0 || r.h <= 0.0) {
            continue;
        }
        max_r = max_r.max(r.right() + sx);
        max_b = max_b.max(r.bottom() + sy);
    }
    for line in &b.lines {
        for f in &line.fragments {
            max_r = max_r.max(f.rect.right() + sx);
            max_b = max_b.max(f.rect.bottom() + sy);
        }
    }
    // Padding at the end of the scrollable area (CSS Overflow 3).
    let pad_r = b.content_rect.right() - pr.right();
    let pad_b = b.content_rect.bottom() - pr.bottom();
    ((max_r - pr.x + pad_r.max(0.0)).max(pr.w), (max_b - pr.y + pad_b.max(0.0)).max(pr.h))
}

/// Move a scroll container's contents by its scroll offset change. Out-of-
/// flow boxes whose containing block is outside the container are not
/// scrolled by it, not even at their static position (the hypothetical
/// box ignores the scroll offset: WPT css-position
/// hypothetical-box-scroll-parent), as when their containing block
/// positions them after the container's layout.
pub(crate) fn scroll_contents(b: &mut LayoutBox, dx: f32, dy: f32) {
    crate::table::translate_contents(b, dx, dy);
    fn unscroll(b: &mut LayoutBox, dx: f32, dy: f32) {
        for c in &mut b.children {
            if c.flow == Flow::Absolute {
                // (Fixed-position boxes kept their inset-given position.)
                if c.style.position != Position::Fixed {
                    c.translate(-dx, -dy);
                }
            } else if !contains_absolutes(&c.style) {
                unscroll(c, dx, dy);
            }
        }
    }
    if !contains_absolutes(&b.style) {
        unscroll(b, dx, dy);
    }
}

/// Max right / bottom extents of the tree (border boxes and lines).
fn bounds(b: &LayoutBox) -> (f32, f32) {
    if b.fixed && b.flow == Flow::Absolute {
        return (0.0, 0.0);
    }
    let mut r = b.rect.right();
    let mut btm = b.rect.bottom();
    let (clip_x, clip_y) = b.overflow_clip_axes();
    if (clip_x && clip_y) || b.wm_transform.is_some() {
        return (r, btm);
    }
    for line in &b.lines {
        for f in &line.fragments {
            if !clip_x { r = r.max(f.rect.right()); }
            if !clip_y { btm = btm.max(f.rect.bottom()); }
        }
    }
    for c in &b.children {
        let (cr, cb) = bounds(c);
        if !clip_x { r = r.max(cr); }
        if !clip_y { btm = btm.max(cb); }
    }
    (r, btm)
}
