//! Box tree types, the layout entry point, hit testing and debug dumps.

use std::sync::Arc;
use crate::LayoutContext;
use rabbithole_css::PseudoElement;
use rabbithole_dom::NodeId;
use rabbithole_gfx::{Color, FontId, Image, Matrix, Rect};
use rabbithole_style::{
    transform_matrix, AppliedDecoration, ComputedStyle, Edges, PointerEvents, Position, Shadow,
};
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxKind {
    Block,
    AnonymousBlock,
    Inline,
    InlineBlock,
    Text,
    Image,
    LineBreak,
    Table,
    TableRow,
    TableCell,
}

/// How a box participates in the flow of its parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    InFlow,
    FloatLeft,
    FloatRight,
    /// `position: absolute` / `fixed`: laid out against its containing block.
    Absolute,
}

impl Flow {
    pub fn is_float(&self) -> bool {
        matches!(self, Flow::FloatLeft | Flow::FloatRight)
    }
    pub fn is_in_flow(&self) -> bool {
        matches!(self, Flow::InFlow)
    }
}

/// How a container lays out its children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Formatting {
    /// Children are block-level boxes stacked vertically.
    Block,
    /// Children are inline content described by `items`, producing `lines`.
    Inline,
    /// Flex container (`flex.rs`).
    Flex,
    /// Grid container (`grid.rs`).
    Grid,
    /// `display: table` — children are rows / row groups / columns.
    Table,
    /// Table wrapper box (CSS 2.1 §17.4): the table plus its captions.
    TableWrapper,
    /// A table row; children are cells.
    TableRow,
    /// Leaf (text, image, line break).
    None,
}

/// A flattened piece of inline content. Inline *elements* are represented by
/// `Open`/`Close` markers; atomic boxes (inline-blocks, images, floats,
/// absolutes) live in the container's `children` and are referenced by index.
#[derive(Debug, Clone)]
pub enum InlineItem {
    Text {
        text: Rc<str>,
        style: Rc<ComputedStyle>,
        /// The text node.
        node: Option<NodeId>,
        /// Nearest inline element ancestor (or the container) for hit testing.
        elem: Option<NodeId>,
    },
    Open {
        node: Option<NodeId>,
        style: Rc<ComputedStyle>,
    },
    Close,
    Atomic(usize),
    Float(usize),
    Absolute(usize),
    Break,
}

/// Emphasis marks over (or under) each typographic character of a text
/// fragment (CSS Text Decoration 3 §3).
#[derive(Debug, Clone, PartialEq)]
pub struct Emphasis {
    pub mark: Rc<str>,
    pub color: Color,
    /// On the logical top side (else the bottom).
    pub over: bool,
}

/// The text decorations drawn over a text fragment.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DecorationInfo {
    /// The decorations in effect (outermost decorating box first).
    pub list: Vec<AppliedDecoration>,
    /// Per decoration, the horizontal extent `(x0, x1)` of its line
    /// relative to the fragment's left edge, when it differs from the
    /// fragment's (insets at the decorating box's ends).
    pub extents: Vec<Option<(f32, f32)>>,
    /// Per decoration, the decorating box's baseline relative to the
    /// fragment's (a shifted descendant keeps its ancestor's line) and
    /// the ascent and descent of its font.
    pub metrics: Vec<(f32, f32, f32)>,
    /// `font-variation-settings` of the text (variable fonts vary their
    /// decoration metrics).
    pub variations: Vec<([u8; 4], f32)>,
    /// `text-decoration-skip-ink` of the text: 0 auto, 1 none, 2 all.
    pub skip_ink: u8,
    /// Widths of the spaces starting and ending the line in this fragment
    /// (`text-decoration-skip-spaces`): not decorated.
    pub skip_spaces: (f32, f32),
}

impl DecorationInfo {
    pub fn any(&self) -> bool {
        !self.list.is_empty()
    }
}

/// Word-separator characters (CSS Text 3 §4.1.1): expanded by
/// `inter-word` justification (NBSP is a space in laid-out text).
pub fn is_word_separator(c: char) -> bool {
    matches!(c, ' ' | '\u{a0}' | '\u{1361}' | '\u{10100}' | '\u{10101}' | '\u{1039f}' | '\u{1091f}')
}

/// The extra justification space (`word_spacing` per separator) before
/// byte `idx` of `text`.
pub fn word_spacing_before(text: &str, idx: usize, word_spacing: f32) -> f32 {
    if word_spacing == 0.0 {
        return 0.0;
    }
    word_spacing * text[..idx.min(text.len())].chars().filter(|c| is_word_separator(*c)).count() as f32
}

/// A painted piece of a line.
#[derive(Debug, Clone)]
pub enum FragmentKind {
    Text {
        text: Rc<str>,
        font: FontId,
        size: f32,
        color: Color,
        letter_spacing: f32,
        /// Extra space after each word separator (justification).
        word_spacing: f32,
        /// Hide text but keep its space (`visibility: hidden`).
        hidden: bool,
        /// Text decorations.
        decoration_info: DecorationInfo,
        /// `text-shadow` list.
        shadows: Vec<Shadow>,
        /// `font-variant: small-caps` (synthesized).
        small_caps: bool,
        /// Synthetic oblique (italic requested, upright face chosen).
        oblique: bool,
        /// `text-emphasis` marks.
        emphasis: Option<Emphasis>,
    },
    /// Background / border segment of an inline element.
    InlineBox {
        style: Rc<ComputedStyle>,
        /// Whether this fragment carries the element's left / right edge.
        left_edge: bool,
        right_edge: bool,
    },
    /// Index into the container's `children`.
    Box(usize),
}

#[derive(Debug, Clone)]
pub struct Fragment {
    /// For text: the glyph box (ascent+descent tall). For inline boxes: the
    /// border box. For atomic boxes: the margin box.
    pub rect: Rect,
    /// Absolute y of the fragment's baseline.
    pub baseline: f32,
    /// Element this fragment belongs to (for hit testing).
    pub node: Option<NodeId>,
    /// `pointer-events: none` on the fragment's style (not hit-testable).
    pub inert: bool,
    pub kind: FragmentKind,
}

#[derive(Debug, Clone)]
pub struct LineBox {
    pub rect: Rect,
    pub baseline: f32,
    pub fragments: Vec<Fragment>,
}

/// A list-item marker.
#[derive(Debug, Clone)]
pub struct Marker {
    pub text: Rc<str>,
    pub font: FontId,
    pub size: f32,
    pub color: Color,
    /// Left edge of the marker text.
    pub x: f32,
    /// Absolute baseline y.
    pub baseline: f32,
    pub width: f32,
}

/// A user-scrollable box found by [`LayoutTree::scroll_container_at`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollInfo {
    pub node: NodeId,
    /// Padding box in document coordinates.
    pub viewport: Rect,
    /// Scrollable content size (≥ the padding box).
    pub content_width: f32,
    pub content_height: f32,
    /// Current offset applied.
    pub offset: (f32, f32),
}

/// `LayoutBox::flex_baselines` before the box is laid out as a flex
/// container.
pub(crate) const FLEX_BASELINE_UNSET: u32 = u32::MAX;
/// `LayoutBox::flex_baselines`: the container has no such baseline (no
/// child is ever at this index).
pub(crate) const FLEX_BASELINE_NONE: u32 = u32::MAX - 1;

#[derive(Debug)]
pub struct LayoutBox {
    pub kind: BoxKind,
    pub node: Option<NodeId>,
    pub style: Rc<ComputedStyle>,
    /// Border box in document coordinates.
    pub rect: Rect,
    /// Content box in document coordinates.
    pub content_rect: Rect,
    pub children: Vec<LayoutBox>,
    pub flow: Flow,
    pub formatting: Formatting,
    /// Inline content (when `formatting == Inline`).
    pub items: Vec<InlineItem>,
    /// Line boxes (when `formatting == Inline`).
    pub lines: Vec<LineBox>,
    /// Text of a `Text` box (also used for `alt` text of missing images).
    pub text: Option<Rc<str>>,
    /// Decoded image for `Image` boxes.
    pub image: Option<Arc<Image>>,
    /// Intrinsic (attribute) size for replaced boxes without an image.
    pub attr_size: (Option<f32>, Option<f32>),
    /// Replaced box with an intrinsic ratio but no intrinsic size (inline
    /// `<svg>` without width/height attributes).
    pub ratio_only: bool,
    pub marker: Option<Marker>,
    /// Marker text computed at build time (before positioning).
    pub marker_text: Option<Rc<str>>,
    /// Baseline offset from `rect.y` (for inline-block alignment), if known.
    pub baseline: Option<f32>,
    /// The children whose baselines are a flex container's (first, last)
    /// baselines, set by its layout (CSS Flexbox §8.5).
    /// ([`FLEX_BASELINE_UNSET`] until then, [`FLEX_BASELINE_NONE`] for
    /// none.)
    pub(crate) flex_baselines: [u32; 2],
    /// A stretched row flex item's measuring layout, kept while the box is
    /// reused: (main size, container width and height, cross size,
    /// ascent). The item is laid out again stretched anyway, so the
    /// measure need not run. (Boxed: few boxes have one.)
    #[allow(clippy::type_complexity)]
    pub(crate) flex_measure: Option<Box<(f32, f32, Option<f32>, f32, f32)>>,
    /// Memo of `flex::depends_on_definite_height` (0 unknown, 1 no,
    /// 2 yes), valid while the box is reused.
    pub(crate) pct_dependent: std::cell::Cell<u8>,
    /// Memo of the element's form-control tag (see
    /// `Layouter::control_kind`; 0 = not looked up yet).
    pub(crate) control: std::cell::Cell<u8>,
    /// Resolved margins (px) after layout.
    pub margin: Edges<f32>,
    /// Effective bottom margin after collapsing with descendants.
    pub collapsed_bottom_margin: f32,
    /// The set of adjoining margins collapsed into the bottom margin, as
    /// (largest positive, most negative) — needed to keep collapsing it with
    /// the following sibling's margins (CSS 2.1 §8.3.1).
    pub(crate) bottom_margins: (f32, f32),
    /// Static position hint for absolutely positioned boxes.
    pub static_pos: (f32, f32),
    /// Self-alignment within a static-position rectangle whose start corner
    /// is `static_pos` (CSS Align 3 §A; children of grid containers).
    pub static_align: Option<StaticAlign>,
    /// A grid container whose items' areas overlap: its items then paint
    /// atomically (as inline blocks) so later items cover earlier ones.
    pub grid_items_overlap: bool,
    pub colspan: u32,
    /// Whether this box establishes a block formatting context.
    pub bfc_root: bool,
    /// Whether the box clips its overflow (paint pushes a clip).
    pub clips: bool,
    /// `overflow: auto | scroll` — user scrollable.
    pub scroll_container: bool,
    /// Scrollable content size (padding-box relative) of clipping boxes.
    pub scroll_width: f32,
    pub scroll_height: f32,
    /// Scroll offset applied to the contents (from the context).
    pub scroll_offset: (f32, f32),
    /// `position: fixed` (painted in the fixed layer).
    pub fixed: bool,
    /// Generated content box (`::before` / `::after`); `node` is the
    /// originating element.
    pub pseudo: Option<PseudoElement>,
    /// After the clamp point of a `line-clamp` container: not painted.
    pub clamped: bool,
    pub(crate) pieces: Option<Vec<crate::inline::Piece>>,
    pub(crate) intrinsic_cache: Option<(f32, f32)>,
    /// A flex item's row-axis content sizes measured without its own
    /// width and min/max-width (see `flex_content_sizes`), cached like
    /// `intrinsic_cache`.
    pub(crate) flex_content_cache: Option<(f32, f32)>,
    /// Inputs and results of the box's last content layout, so a box kept
    /// from the previous frame can be translated instead of laid out.
    pub(crate) layout_cache: Option<crate::incremental::LayoutCache>,
    /// Memoized fingerprint of the contents' geometry relative to the
    /// unscrolled content origin (see `incremental::contents_fingerprint`).
    /// Cleared whenever the contents are laid out again or moved relative
    /// to the box.
    pub(crate) contents_fp: std::cell::Cell<Option<u64>>,
    /// Used per-axis subgrid inputs. Computed styles retain the CSS request.
    pub(crate) grid_columns: Option<crate::grid::subgrid::AxisInheritance>,
    pub(crate) grid_rows: Option<crate::grid::subgrid::AxisInheritance>,
    /// CSSOM data from final layout, in this Grid box's own logical axes.
    pub(crate) used_grid_tracks: [Option<Rc<rabbithole_style::UsedGridTrackList>>; 2],
    /// `position: relative` offset currently applied to the box.
    pub(crate) rel_applied: (f32, f32),
    /// The last layout translated the contents from the cache (descendants
    /// are as they were: relative offsets need no update below).
    pub(crate) reused_layout: bool,
    /// No absolutely positioned box is reachable through static descendants
    /// (valid while the subtree is unchanged).
    pub(crate) no_abs_below: bool,
    /// Building the box's subtree used CSS counters / quotes.
    pub(crate) counter_touch: bool,
    /// A run-in became this box's first inline box (the box is rebuilt
    /// rather than reused: the run-in depends on its siblings).
    pub(crate) run_in_host: bool,
    /// Computed style of the element this box was built for (the box's own
    /// style may differ); identifies reusable boxes.
    pub(crate) built_from: Option<Rc<ComputedStyle>>,
    /// Counter/quote state before and after building the subtree (only
    /// recorded when the document uses counters), so the box can be reused
    /// when it would see the same state.
    pub(crate) counter_io: Option<Rc<crate::build::CounterIo>>,
    /// Anonymous wrapper of a vertical writing-mode subtree (its only
    /// child, laid out in logical space; see `wm.rs`).
    pub(crate) wm_wrapper: bool,
    /// A wrapper holding a horizontal-in-vertical composition
    /// (`text-combine-upright`): squeezed into 1em.
    pub(crate) wm_tcy: bool,
    /// Logical → physical mapping of a wrapper's contents.
    pub(crate) wm_transform: Option<Matrix>,
    /// How a wrapper's contents are turned (see `wm.rs`).
    pub(crate) wm_kind: WmKind,
    /// A wrapper's physical background layers (images and gradients of the
    /// box it wraps, painted unturned).
    pub(crate) wm_background: Option<Rc<ComputedStyle>>,
    /// Ruby layout role: 1 = a base/annotation pair box, 2 = its base.
    pub(crate) ruby: u8,
}

/// Orientation of a writing-mode wrapper's contents relative to the space
/// it sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum WmKind {
    /// 90° clockwise (`vertical-rl` in horizontal).
    #[default]
    Cw,
    /// Reflection about the diagonal (`vertical-lr` in horizontal, and
    /// horizontal in `vertical-lr`).
    Reflect,
    /// 90° counter-clockwise (horizontal in `vertical-rl`).
    Ccw,
}

impl LayoutBox {
    pub(crate) fn new(kind: BoxKind, node: Option<NodeId>, style: Rc<ComputedStyle>) -> LayoutBox {
        LayoutBox {
            kind,
            node,
            style,
            rect: Rect::default(),
            content_rect: Rect::default(),
            children: Vec::new(),
            flow: Flow::InFlow,
            formatting: Formatting::None,
            items: Vec::new(),
            lines: Vec::new(),
            text: None,
            image: None,
            attr_size: (None, None),
            ratio_only: false,
            marker: None,
            marker_text: None,
            baseline: None,
            flex_baselines: [FLEX_BASELINE_UNSET; 2],
            flex_measure: None,
            pct_dependent: std::cell::Cell::new(0),
            control: std::cell::Cell::new(0),
            margin: Edges::all(0.0),
            collapsed_bottom_margin: 0.0,
            bottom_margins: (0.0, 0.0),
            static_pos: (0.0, 0.0),
            static_align: None,
            grid_items_overlap: false,
            colspan: 1,
            bfc_root: false,
            clips: false,
            scroll_container: false,
            scroll_width: 0.0,
            scroll_height: 0.0,
            scroll_offset: (0.0, 0.0),
            fixed: false,
            pseudo: None,
            clamped: false,
            pieces: None,
            intrinsic_cache: None,
            flex_content_cache: None,
            layout_cache: None,
            contents_fp: std::cell::Cell::new(None),
            grid_columns: None,
            grid_rows: None,
            used_grid_tracks: [None, None],
            rel_applied: (0.0, 0.0),
            reused_layout: false,
            no_abs_below: false,
            counter_touch: false,
            run_in_host: false,
            built_from: None,
            counter_io: None,
            wm_wrapper: false,
            wm_tcy: false,
            ruby: 0,
            wm_transform: None,
            wm_kind: WmKind::Cw,
            wm_background: None,
        }
    }

    /// Is this an inline-level box (participates in a line)?
    pub fn is_inline_level(&self) -> bool {
        self.flow.is_in_flow() && match self.kind {
            BoxKind::Image => self.style.display.is_inline_level(),
            BoxKind::Inline | BoxKind::InlineBlock | BoxKind::Text | BoxKind::LineBreak => true,
            _ => false,
        }
    }

    /// Does this box contribute to its parent's height?
    pub fn is_in_flow(&self) -> bool {
        self.flow.is_in_flow()
    }

    /// Margin box rectangle.
    pub fn margin_rect(&self) -> Rect {
        Rect::new(
            self.rect.x - self.margin.left,
            self.rect.y - self.margin.top,
            self.rect.w + self.margin.horizontal(),
            self.rect.h + self.margin.vertical(),
        )
    }

    /// The margin box a sticky box keeps within its containing block:
    /// auto margins (resolved to absorb free space) count as zero.
    pub fn sticky_margin_rect(&self) -> Rect {
        let m = &self.style.margin;
        let side = |l: &rabbithole_style::Length, used: f32| if l.is_auto() { 0.0 } else { used };
        let (t, r, b, l) = (side(&m.top, self.margin.top), side(&m.right, self.margin.right), side(&m.bottom, self.margin.bottom), side(&m.left, self.margin.left));
        Rect::new(self.rect.x - l, self.rect.y - t, self.rect.w + l + r, self.rect.h + t + b)
    }

    /// Padding box rectangle.
    pub fn padding_rect(&self) -> Rect {
        let b = &self.style.border_width;
        Rect::new(
            self.rect.x + b.left,
            self.rect.y + b.top,
            (self.rect.w - b.left - b.right).max(0.0),
            (self.rect.h - b.top - b.bottom).max(0.0),
        )
    }

    /// Axes constrained by this box's overflow clip. Unlike scrollable
    /// overflow, `clip` can leave the other axis visibly overflowing.
    pub(crate) fn overflow_clip_axes(&self) -> (bool, bool) {
        use rabbithole_style::Overflow::{Clip, Visible};
        if !self.clips {
            (false, false)
        } else if self.style.contain_paint {
            (true, true)
        } else {
            match (self.style.overflow_x, self.style.overflow_y) {
                (Clip, Visible) => (true, false),
                (Visible, Clip) => (false, true),
                _ => (true, true),
            }
        }
    }

    /// `r` (document coordinates of this box's subtree) as seen through the
    /// box's CSS transform: the bounding box of the transformed corners
    /// (scrollable overflow follows transforms, CSS Overflow 3 §2.2).
    pub fn transformed_bounds(&self, r: Rect) -> Rect {
        if !self.style.has_transform() || self.wm_transform.is_some() {
            return r;
        }
        let Some(m) = self.transform_matrix() else { return r };
        let pts = [(r.x, r.y), (r.right(), r.y), (r.x, r.bottom()), (r.right(), r.bottom())].map(|(x, y)| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]));
        let (x0, x1) = pts.iter().fold((f32::MAX, f32::MIN), |a, p| (a.0.min(p.0), a.1.max(p.0)));
        let (y0, y1) = pts.iter().fold((f32::MAX, f32::MIN), |a, p| (a.0.min(p.1), a.1.max(p.1)));
        if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
            return r;
        }
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// A scroll container that users can scroll (not `overflow: hidden`).
    pub fn user_scrollable(&self) -> bool {
        use rabbithole_style::Overflow::{Auto, Scroll};
        self.scroll_container && (matches!(self.style.overflow_x, Auto | Scroll) || matches!(self.style.overflow_y, Auto | Scroll))
    }

    fn overflow_contains(&self, x: f32, y: f32) -> bool {
        let (clip_x, clip_y) = self.overflow_clip_axes();
        (!clip_x || (x >= self.rect.x && x < self.rect.right()))
            && (!clip_y || (y >= self.rect.y && y < self.rect.bottom()))
    }

    /// Does this box establish a stacking context (CSS 2.1 Appendix E)?
    pub fn is_stacking_context(&self) -> bool {
        self.style.is_stacking_context() || self.fixed || self.wm_transform.is_some()
    }

    /// Is the box positioned (any value other than `static`)?
    pub fn is_positioned(&self) -> bool {
        self.style.position != Position::Static
    }

    /// Transform matrix of this box (document coordinates), if any.
    pub fn transform_matrix(&self) -> Option<Matrix> {
        if let Some(m) = self.wm_transform {
            // Quarter turns map the pixel grid onto itself: keep the
            // contents on whole pixels (not a squeezed composition's).
            if m[..4].iter().any(|v| *v != 0.0 && v.abs() != 1.0) {
                return Some(m);
            }
            return Some([m[0], m[1], m[2], m[3], m[4].round(), m[5].round()]);
        }
        if !self.style.has_transform() {
            return None;
        }
        let ops = self.style.transform_ops();
        if !ops.iter().all(rabbithole_style::TransformOp::is_2d) || self.style.transform3d.as_ref().is_some_and(|t| t.origin_z != 0.0) {
            return self.transform_matrix_3d().map(|m| rabbithole_gfx::mat4::flatten_2d(&m));
        }
        let r = self.transform_reference_box();
        let m = transform_matrix(&ops, r.w, r.h, self.style.font_size);
        let ox = r.x + self.style.transform_origin.0.resolve_or_zero(r.w);
        let oy = r.y + self.style.transform_origin.1.resolve_or_zero(r.h);
        // T(origin) × M × T(-origin)
        let t1 = [1.0, 0.0, 0.0, 1.0, ox, oy];
        let t2 = [1.0, 0.0, 0.0, 1.0, -ox, -oy];
        Some(rabbithole_style::multiply_matrix(rabbithole_style::multiply_matrix(t1, m), t2))
    }

    /// The transform reference box (`transform-box`; CSS boxes map
    /// `fill-box` to the content box and `stroke-box` / `view-box` to the
    /// border box).
    pub fn transform_reference_box(&self) -> Rect {
        match self.style.transform3d.as_ref().map(|t| t.transform_box) {
            Some(rabbithole_style::TransformBox::ContentBox | rabbithole_style::TransformBox::FillBox) => self.content_rect,
            _ => self.rect,
        }
    }

    /// The 3D transform of this box (document coordinates, origin
    /// applied), without any perspective from its containing block.
    /// `None` without a transform (or for writing-mode wrappers).
    pub fn transform_matrix_3d(&self) -> Option<rabbithole_gfx::mat4::Mat4> {
        use rabbithole_gfx::mat4;
        if self.wm_transform.is_some() || !self.style.has_transform() {
            return None;
        }
        let r = self.transform_reference_box();
        let m = rabbithole_style::transform_matrix_3d(&self.style.transform_ops(), r.w, r.h);
        let ox = r.x + self.style.transform_origin.0.resolve_or_zero(r.w);
        let oy = r.y + self.style.transform_origin.1.resolve_or_zero(r.h);
        let oz = self.style.transform3d.as_ref().map_or(0.0, |t| t.origin_z);
        Some(mat4::mul(&mat4::mul(&mat4::translate(ox, oy, oz), &m), &mat4::translate(-ox, -oy, -oz)))
    }

    /// The perspective this box applies to its children (`perspective`,
    /// about `perspective-origin`), document coordinates.
    pub fn child_perspective(&self) -> Option<rabbithole_gfx::mat4::Mat4> {
        use rabbithole_gfx::mat4;
        let t = self.style.transform3d.as_deref()?;
        let d = t.perspective?;
        let r = self.rect;
        let ox = r.x + t.perspective_origin.0.resolve_or_zero(r.w);
        let oy = r.y + t.perspective_origin.1.resolve_or_zero(r.h);
        Some(mat4::mul(&mat4::mul(&mat4::translate(ox, oy, 0.0), &mat4::perspective(Some(d))), &mat4::translate(-ox, -oy, 0.0)))
    }

    /// `position: sticky` offset for the box given the nearest scrollport
    /// (document coordinates) and its containing block's content rect.
    pub fn sticky_offset(&self, scrollport: Rect, cb: Rect) -> (f32, f32) {
        if self.style.position != Position::Sticky {
            return (0.0, 0.0);
        }
        sticky_offset_for(&self.style.inset, self.rect, self.sticky_margin_rect(), scrollport, cb)
    }
}

/// `position: sticky` displacement of a box with border box `br`, margin
/// box `mr` and insets `inset`, for a scrollport and containing block
/// content rect: the border box is kept `inset` inside the scrollport,
/// and the margin box within the containing block (CSS Position 3 §3.4).
pub(crate) fn sticky_offset_for(inset: &Edges<rabbithole_style::Length>, br: Rect, mr: Rect, scrollport: Rect, cb: Rect) -> (f32, f32) {
    let mut dy = 0.0;
    if let Some(top) = inset.top.resolve(scrollport.h) {
        let want = scrollport.y + top;
        if br.y < want {
            dy = want - br.y;
            // Stay within the containing block.
            let max = (cb.bottom() - mr.bottom()).max(0.0);
            dy = dy.min(max);
        }
    }
    if dy == 0.0 {
        if let Some(bottom) = inset.bottom.resolve(scrollport.h) {
            let want = scrollport.bottom() - bottom;
            if br.bottom() > want {
                dy = want - br.bottom();
                let min = (cb.y - mr.y).min(0.0);
                dy = dy.max(min);
            }
        }
    }
    let mut dx = 0.0;
    if let Some(left) = inset.left.resolve(scrollport.w) {
        let want = scrollport.x + left;
        if br.x < want {
            dx = want - br.x;
            let max = (cb.right() - mr.right()).max(0.0);
            dx = dx.min(max);
        }
    }
    if dx == 0.0 {
        if let Some(right) = inset.right.resolve(scrollport.w) {
            let want = scrollport.right() - right;
            if br.right() > want {
                dx = want - br.right();
                let min = (cb.x - mr.x).min(0.0);
                dx = dx.max(min);
            }
        }
    }
    (dx, dy)
}

/// A static-position rectangle and the alignment of an absolutely
/// positioned box with auto insets inside it: per axis, the size and the
/// fraction of the free space before the box (0 start, 0.5 center, 1 end).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StaticAlign {
    pub size: (f32, f32),
    pub factor: (f32, f32),
}

impl LayoutBox {
    /// The element of an out-of-flow box: its own, or that of the box an
    /// orthogonal writing-mode wrapper contains.
    pub(crate) fn abs_node(&self) -> Option<rabbithole_dom::NodeId> {
        self.node.or_else(|| if self.wm_wrapper { self.children.first().and_then(|c| c.node) } else { None })
    }

    /// Move this box and everything inside it.
    pub fn translate(&mut self, dx: f32, dy: f32) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        self.rect = self.rect.translate(dx, dy);
        self.content_rect = self.content_rect.translate(dx, dy);
        self.static_pos = (self.static_pos.0 + dx, self.static_pos.1 + dy);
        if let Some(m) = &mut self.wm_transform {
            // The contents stay in their logical space.
            m[4] += dx;
            m[5] += dy;
            return;
        }
        for line in &mut self.lines {
            line.rect = line.rect.translate(dx, dy);
            line.baseline += dy;
            for f in &mut line.fragments {
                f.rect = f.rect.translate(dx, dy);
                f.baseline += dy;
            }
        }
        if let Some(m) = &mut self.marker {
            m.x += dx;
            m.baseline += dy;
        }
        for c in &mut self.children {
            c.translate(dx, dy);
        }
    }

    /// Baseline of the box's first line (absolute y), searching descendants.
    pub fn first_baseline(&self) -> Option<f32> {
        if self.wm_wrapper {
            // An orthogonal flow has no baseline in its parent's axis (its
            // lines are in its own, logical, space): one is synthesized.
            return None;
        }
        if self.formatting == Formatting::Flex && self.flex_baselines[0] != FLEX_BASELINE_UNSET {
            // An item without a baseline has one synthesized from its
            // border box's bottom edge.
            return self.children.get(self.flex_baselines[0] as usize).map(|c| c.first_baseline().unwrap_or(self.synthesized_baseline_of(c)));
        }
        if let Some(l) = self.lines.first() {
            return Some(l.baseline);
        }
        self.children
            .iter()
            .filter(|c| c.is_in_flow())
            .find_map(|c| c.first_baseline())
            .or_else(|| self.synthesized_item_baseline(false))
    }

    /// The baseline synthesized from an item's border box: its bottom
    /// edge, or its middle in a vertical container (central baseline).
    fn synthesized_baseline_of(&self, c: &LayoutBox) -> f32 {
        if self.style.central_baseline() {
            c.rect.y + c.rect.h / 2.0
        } else {
            c.rect.bottom()
        }
    }

    /// A grid container whose items have no baseline synthesizes one from
    /// the first (last) item's border box (CSS Align §9.1).
    fn synthesized_item_baseline(&self, last: bool) -> Option<f32> {
        if self.formatting != Formatting::Grid {
            return None;
        }
        let mut items = self.children.iter().filter(|c| c.is_in_flow());
        let item = if last { items.last() } else { items.next() };
        // (Orthogonal items keep the margin-box fallback of the container.)
        item.filter(|c| !c.wm_wrapper).map(|c| c.rect.bottom())
    }

    /// Baseline of the last line (absolute y).
    pub fn last_baseline(&self) -> Option<f32> {
        if self.wm_wrapper {
            return None;
        }
        if self.formatting == Formatting::Flex && self.flex_baselines[1] != FLEX_BASELINE_UNSET {
            return self.children.get(self.flex_baselines[1] as usize).map(|c| c.last_baseline().unwrap_or(self.synthesized_baseline_of(c)));
        }
        if let Some(l) = self.lines.last() {
            return Some(l.baseline);
        }
        self.children
            .iter()
            .rev()
            .filter(|c| c.is_in_flow())
            .find_map(|c| c.last_baseline())
            .or_else(|| self.synthesized_item_baseline(true))
    }

    /// Visit every absolutely positioned descendant whose containing block is
    /// this box (does not enter other positioned descendants).
    pub fn for_each_abs_child<'s, F: FnMut(&'s LayoutBox)>(&'s self, f: &mut F) {
        for c in &self.children {
            if c.flow == Flow::Absolute {
                f(c);
            } else if c.style.position == Position::Static {
                c.for_each_abs_child(f);
            }
        }
    }

    /// Union of the border boxes / fragments of the subtree (ignores clips).
    pub fn subtree_bounds(&self) -> Rect {
        let mut r = self.rect;
        for line in &self.lines {
            for f in &line.fragments {
                r = r.union(&f.rect);
            }
        }
        for c in &self.children {
            r = r.union(&c.subtree_bounds());
        }
        r
    }

    fn tag<'a>(&self, ctx_doc: Option<&'a rabbithole_dom::Document>) -> String {
        match (self.node, ctx_doc) {
            (Some(n), Some(doc)) => doc.tag(n).map(|t| format!("<{t}>")).unwrap_or_else(|| "#text".into()),
            (Some(n), None) => format!("node{n}"),
            _ => String::new(),
        }
    }

    fn dump_into(&self, depth: usize, out: &mut String) {
        use std::fmt::Write;
        let pad = "  ".repeat(depth);
        let r = self.rect;
        let flow = match self.flow {
            Flow::InFlow => "",
            Flow::FloatLeft => " float:left",
            Flow::FloatRight => " float:right",
            Flow::Absolute => " abs",
        };
        let _ = write!(
            out,
            "{pad}{:?}{}{} {} [{:.1},{:.1} {:.1}x{:.1}]{}",
            self.kind,
            match self.pseudo {
                Some(PseudoElement::Before) => "::before",
                Some(PseudoElement::After) => "::after",
                Some(_) => "::pseudo",
                None => "",
            },
            self.tag(None),
            match self.formatting {
                Formatting::Inline => "(inline)",
                Formatting::Flex => "(flex)",
                Formatting::Grid => "(grid)",
                Formatting::Table => "(table)",
                Formatting::TableWrapper => "(table-wrapper)",
                Formatting::TableRow => "(row)",
                _ => "",
            },
            r.x,
            r.y,
            r.w,
            r.h,
            flow
        );
        if self.fixed {
            let _ = write!(out, " fixed");
        }
        if self.scroll_container {
            let _ = write!(out, " scroll[{:.0}x{:.0} @{:.0},{:.0}]", self.scroll_width, self.scroll_height, self.scroll_offset.0, self.scroll_offset.1);
        }
        if let Some(m) = self.transform_matrix() {
            let _ = write!(out, " transform[{:.2} {:.2} {:.2} {:.2} {:.1} {:.1}]", m[0], m[1], m[2], m[3], m[4], m[5]);
        }
        if self.style.position == Position::Sticky {
            let _ = write!(out, " sticky");
        }
        let _ = writeln!(out);
        if let Some(m) = &self.marker {
            let _ = writeln!(out, "{pad}  marker {:?} @ {:.1}", m.text, m.x);
        }
        for (i, line) in self.lines.iter().enumerate() {
            let _ = writeln!(
                out,
                "{pad}  line {i} [{:.1},{:.1} {:.1}x{:.1}] baseline {:.1}",
                line.rect.x, line.rect.y, line.rect.w, line.rect.h, line.baseline
            );
            for f in &line.fragments {
                match &f.kind {
                    FragmentKind::Text { text, .. } => {
                        let short: String = text.chars().take(40).collect();
                        let _ = writeln!(out, "{pad}    text [{:.1},{:.1} {:.1}x{:.1}] {:?}", f.rect.x, f.rect.y, f.rect.w, f.rect.h, short);
                    }
                    FragmentKind::InlineBox { .. } => {
                        let _ = writeln!(out, "{pad}    inlinebox [{:.1},{:.1} {:.1}x{:.1}]", f.rect.x, f.rect.y, f.rect.w, f.rect.h);
                    }
                    FragmentKind::Box(i) => {
                        let _ = writeln!(out, "{pad}    box #{i}");
                    }
                }
            }
        }
        for c in &self.children {
            c.dump_into(depth + 1, out);
        }
    }
}

#[derive(Debug)]
pub struct LayoutTree {
    pub root: LayoutBox,
    /// Total document width/height (max of viewport and content).
    pub width: f32,
    pub height: f32,
    /// Viewport size used for layout (for fixed boxes and sticky).
    pub viewport: (f32, f32),
    /// Box construction used CSS counters or quotes (document-order
    /// state), so boxes cannot be rebuilt piecemeal.
    pub(crate) uses_counters: bool,
}

impl LayoutTree {
    /// Final Grid template value for the element's actual box. Anonymous
    /// writing-mode wrappers do not own it; box_of follows their inner node.
    pub fn used_grid_tracks(&self, doc: &rabbithole_dom::Document, node: NodeId, rows: bool) -> Option<Rc<rabbithole_style::UsedGridTrackList>> {
        self.box_of(doc, node)?.used_grid_tracks[usize::from(rows)].clone()
    }

    /// An empty tree (placeholder before the first layout).
    pub fn empty() -> LayoutTree {
        LayoutTree {
            root: LayoutBox::new(BoxKind::Block, None, Rc::new(ComputedStyle::initial())),
            width: 0.0,
            height: 0.0,
            viewport: (0.0, 0.0),
            uses_counters: true,
        }
    }

    /// Does the tree contain sticky or fixed boxes, inner scroll offsets or
    /// fixed backgrounds (i.e. does scrolling change the display list)?
    pub fn has_sticky_or_fixed(&self) -> bool {
        fn walk(b: &LayoutBox) -> bool {
            if b.fixed
                || b.style.position == rabbithole_style::Position::Sticky
                || b.style.background_layers.iter().any(|l| l.fixed)
                || b.user_scrollable()
                || b.scroll_container && b.scroll_offset != (0.0, 0.0)
            {
                return true;
            }
            b.children.iter().any(walk)
        }
        walk(&self.root)
    }

    /// Does painting depend on the page scroll position (sticky boxes or
    /// `background-attachment: fixed`)? Fixed boxes and inner scroll
    /// containers do not: they are painted in viewport / own coordinates.
    ///
    /// Sticky boxes alone can be handled by patching the display list
    /// ([`crate::DisplayList::update_page_scroll`]); this reports only what
    /// needs a rebuild (fixed backgrounds).
    pub fn depends_on_page_scroll(&self) -> bool {
        fn walk(b: &LayoutBox) -> bool {
            if b.style.background_layers.iter().any(|l| l.fixed) {
                return true;
            }
            b.children.iter().any(walk)
        }
        walk(&self.root)
    }

    /// Border-box union of the line fragments of an element that lives in
    /// inline content (an inline element or a text node: no box of its
    /// own), in document coordinates. None when it has no fragment (not
    /// rendered, or it generates a box: see [`LayoutTree::box_of`]).
    pub fn inline_rect(&self, doc: &rabbithole_dom::Document, node: NodeId) -> Option<Rect> {
        // The nearest ancestor with a box holds the line boxes (possibly in
        // anonymous block boxes below it).
        let mut cur = doc.flat_parent(node);
        let container = loop {
            let c = cur?;
            if let Some(b) = self.box_of(doc, c) {
                break b;
            }
            cur = doc.flat_parent(c);
        };
        let stop = container.node;
        let inside = |n: NodeId| {
            let mut c = Some(n);
            while let Some(x) = c {
                if x == node {
                    return true;
                }
                if Some(x) == stop {
                    return false;
                }
                c = doc.flat_parent(x);
            }
            false
        };
        fn walk(b: &LayoutBox, inside: &dyn Fn(NodeId) -> bool, acc: &mut Option<Rect>) {
            for line in &b.lines {
                for f in &line.fragments {
                    let hit = match &f.kind {
                        FragmentKind::Box(i) => b.children.get(*i).and_then(|c| c.node).map(inside).unwrap_or(false) || f.node.map(inside).unwrap_or(false),
                        _ => f.node.map(inside).unwrap_or(false),
                    };
                    if hit && (f.rect.w > 0.0 || f.rect.h > 0.0) {
                        *acc = Some(match acc {
                            Some(r) => r.union(&f.rect),
                            None => f.rect,
                        });
                    }
                }
            }
            for c in &b.children {
                // Descend into anonymous boxes and boxes of the element's
                // own descendants (inline-blocks inside it).
                if c.node.is_none() || c.node.map(inside).unwrap_or(false) {
                    walk(c, inside, acc);
                }
            }
        }
        let mut acc = None;
        walk(container, &inside, &mut acc);
        acc
    }

    /// [`LayoutTree::inline_rect`] for every element at once: one pass over
    /// the line fragments, adding each fragment to its element and that
    /// element's inline ancestors up to the box that holds the lines.
    pub fn inline_rect_index(&self, doc: &rabbithole_dom::Document) -> std::collections::HashMap<NodeId, Rect> {
        let mut out: std::collections::HashMap<NodeId, Rect> = std::collections::HashMap::new();
        fn add(out: &mut std::collections::HashMap<NodeId, Rect>, doc: &rabbithole_dom::Document, from: NodeId, stop: Option<NodeId>, r: Rect) {
            let mut cur = Some(from);
            let mut guard = 0;
            while let Some(n) = cur {
                if Some(n) == stop || guard > 64 {
                    break;
                }
                out.entry(n).and_modify(|e| *e = e.union(&r)).or_insert(r);
                cur = doc.flat_parent(n);
                guard += 1;
            }
        }
        fn walk(b: &LayoutBox, container: Option<NodeId>, doc: &rabbithole_dom::Document, out: &mut std::collections::HashMap<NodeId, Rect>) {
            let container = b.node.or(container);
            for line in &b.lines {
                for f in &line.fragments {
                    if f.rect.w <= 0.0 && f.rect.h <= 0.0 {
                        continue;
                    }
                    let n = match &f.kind {
                        FragmentKind::Box(i) => b.children.get(*i).and_then(|c| c.node).or(f.node),
                        _ => f.node,
                    };
                    if let Some(n) = n {
                        if Some(n) != container {
                            add(out, doc, n, container, f.rect);
                        }
                    }
                }
            }
            for c in &b.children {
                walk(c, container, doc, out);
            }
        }
        walk(&self.root, None, doc, &mut out);
        out
    }

    /// The client rects of inline element `node` (CSSOM View
    /// `getClientRects()`): one rect per line box it has fragments on, each
    /// the union of the fragments of `node` and its inline descendants there.
    pub fn inline_client_rects(&self, doc: &rabbithole_dom::Document, node: NodeId) -> Vec<Rect> {
        fn within(doc: &rabbithole_dom::Document, from: NodeId, node: NodeId, stop: Option<NodeId>) -> bool {
            let mut cur = Some(from);
            let mut guard = 0;
            while let Some(n) = cur {
                if n == node {
                    return true;
                }
                if Some(n) == stop || guard > 64 {
                    return false;
                }
                cur = doc.flat_parent(n);
                guard += 1;
            }
            false
        }
        fn walk(b: &LayoutBox, container: Option<NodeId>, doc: &rabbithole_dom::Document, node: NodeId, out: &mut Vec<Rect>) {
            let container = b.node.or(container);
            for line in &b.lines {
                let mut acc: Option<Rect> = None;
                for f in &line.fragments {
                    if f.rect.w <= 0.0 && f.rect.h <= 0.0 {
                        continue;
                    }
                    let n = match &f.kind {
                        FragmentKind::Box(i) => b.children.get(*i).and_then(|c| c.node).or(f.node),
                        _ => f.node,
                    };
                    if let Some(n) = n.filter(|&n| Some(n) != container && within(doc, n, node, container)) {
                        let _ = n;
                        acc = Some(acc.map_or(f.rect, |a| a.union(&f.rect)));
                    }
                }
                out.extend(acc);
            }
            for c in &b.children {
                walk(c, container, doc, node, out);
            }
        }
        let mut out = Vec::new();
        walk(&self.root, None, doc, node, &mut out);
        out
    }

    /// The box generated by element `node`, found by following its DOM
    /// ancestors down the box tree (no full-tree scan).
    pub fn box_of(&self, doc: &rabbithole_dom::Document, node: NodeId) -> Option<&LayoutBox> {
        let path = guided_path(&self.root, doc, node)?;
        let mut b = &self.root;
        for i in path {
            b = &b.children[i];
        }
        Some(b)
    }

    /// Mutable [`LayoutTree::box_of`].
    pub fn box_of_mut(&mut self, doc: &rabbithole_dom::Document, node: NodeId) -> Option<&mut LayoutBox> {
        let path = guided_path(&self.root, doc, node)?;
        let mut b = &mut self.root;
        for i in path {
            b = &mut b.children[i];
        }
        Some(b)
    }

    /// Scroll the contents of the scroll container generated by `node` to
    /// `offset` by translating them (no layout). Returns false when the
    /// box is not a laid out scroll container.
    pub fn set_scroll_offset(&mut self, doc: &rabbithole_dom::Document, node: NodeId, offset: (f32, f32)) -> bool {
        let Some(b) = self.box_of_mut(doc, node) else { return false };
        if !b.scroll_container {
            return false;
        }
        let pr = b.padding_rect();
        let ox = offset.0.clamp(0.0, (b.scroll_width - pr.w).max(0.0));
        let oy = offset.1.clamp(0.0, (b.scroll_height - pr.h).max(0.0));
        let (dx, dy) = (b.scroll_offset.0 - ox, b.scroll_offset.1 - oy);
        crate::table::translate_contents(b, dx, dy);
        b.scroll_offset = (ox, oy);
        true
    }

    /// Deepest element box containing the point (document coordinates).
    /// Fixed boxes are tested at their layout position (scroll 0).
    pub fn node_at(&self, x: f32, y: f32) -> Option<NodeId> {
        let mut h = Hit {
            page_scroll: (0.0, 0.0),
            skip_fixed: false,
            viewport: self.viewport,
        };
        h.hit(&self.root, x, y)
    }

    /// Hit test with `(x, y)` in *viewport* coordinates: fixed boxes are
    /// tested first (they do not scroll), then the document at
    /// `(x + scroll_x, y + scroll_y)`.
    pub fn node_at_with_scroll(&self, x: f32, y: f32, page_scroll: (f32, f32)) -> Option<NodeId> {
        let mut fixed = Vec::new();
        collect_fixed(&self.root, &mut fixed);
        // Later fixed boxes / higher z-index are on top.
        fixed.sort_by_key(|b| b.style.z_index.unwrap_or(0));
        let mut h = Hit {
            page_scroll,
            skip_fixed: false,
            viewport: self.viewport,
        };
        for b in fixed.iter().rev() {
            if let Some(n) = h.hit(b, x, y) {
                return Some(n);
            }
        }
        h.skip_fixed = true;
        h.hit(&self.root, x + page_scroll.0, y + page_scroll.1)
    }

    /// Innermost user-scrollable box containing the point (document
    /// coordinates) whose content exceeds its padding box.
    pub fn scroll_container_at(&self, x: f32, y: f32) -> Option<ScrollInfo> {
        fn walk(b: &LayoutBox, x: f32, y: f32) -> Option<ScrollInfo> {
            if !b.overflow_contains(x, y) {
                return None;
            }
            for c in b.children.iter().rev() {
                if let Some(s) = walk(c, x, y) {
                    return Some(s);
                }
            }
            if b.user_scrollable() && b.rect.contains(x, y) {
                let pr = b.padding_rect();
                if b.scroll_height > pr.h + 0.5 || b.scroll_width > pr.w + 0.5 {
                    return Some(ScrollInfo {
                        node: b.node?,
                        viewport: pr,
                        content_width: b.scroll_width.max(pr.w),
                        content_height: b.scroll_height.max(pr.h),
                        offset: b.scroll_offset,
                    });
                }
            }
            None
        }
        walk(&self.root, x, y)
    }

    /// Human readable dump for debugging (`rabbithole dump --layout`).
    pub fn dump(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("document {:.0}x{:.0}\n", self.width, self.height));
        self.root.dump_into(0, &mut s);
        s
    }
}

fn collect_fixed<'a>(b: &'a LayoutBox, out: &mut Vec<&'a LayoutBox>) {
    if b.fixed {
        out.push(b);
        return;
    }
    for c in &b.children {
        collect_fixed(c, out);
    }
}

/// Invert an affine matrix (returns None when singular).
pub fn invert_matrix(m: Matrix) -> Option<Matrix> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-9 {
        return None;
    }
    let inv_det = 1.0 / det;
    let a = m[3] * inv_det;
    let b = -m[1] * inv_det;
    let c = -m[2] * inv_det;
    let d = m[0] * inv_det;
    let e = -(a * m[4] + c * m[5]);
    let f = -(b * m[4] + d * m[5]);
    Some([a, b, c, d, e, f])
}

/// Apply a matrix to a point.
pub fn map_point(m: Matrix, x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

struct Hit {
    page_scroll: (f32, f32),
    skip_fixed: bool,
    viewport: (f32, f32),
}

impl Hit {
    fn hit(&mut self, b: &LayoutBox, x: f32, y: f32) -> Option<NodeId> {
        if b.fixed && self.skip_fixed && b.flow == Flow::Absolute {
            // Tested separately in viewport coordinates.
            return None;
        }
        // Map the point into the box's local space when it is transformed.
        let (x, y) = match b.transform_matrix() {
            Some(m) => {
                let inv = invert_matrix(m)?;
                map_point(inv, x, y)
            }
            None => (x, y),
        };
        // Sticky boxes are displaced at paint time; undo that displacement.
        let (x, y) = if b.style.position == Position::Sticky {
            let scrollport = Rect::new(self.page_scroll.0, self.page_scroll.1, self.viewport.0, self.viewport.1);
            // The containing block is unknown here; use the document.
            let cb = Rect::new(f32::MIN / 4.0, f32::MIN / 4.0, f32::MAX / 2.0, f32::MAX / 2.0);
            let (dx, dy) = b.sticky_offset(scrollport, cb);
            (x - dx, y - dy)
        } else {
            (x, y)
        };
        let transparent = b.style.pointer_events == PointerEvents::None;
        // Absolutely positioned descendants are painted last → tested first.
        let mut found = None;
        b.for_each_abs_child(&mut |abs| {
            if found.is_none() {
                found = self.hit(abs, x, y);
            }
        });
        if found.is_some() {
            return found;
        }
        if !b.overflow_contains(x, y) {
            return None;
        }
        if b.formatting == Formatting::Inline {
            for line in b.lines.iter().rev() {
                for f in line.fragments.iter().rev() {
                    if !f.rect.contains(x, y) {
                        continue;
                    }
                    match &f.kind {
                        FragmentKind::Box(i) => {
                            if let Some(n) = self.hit(&b.children[*i], x, y) {
                                return Some(n);
                            }
                        }
                        _ => {
                            if f.node.is_some() && !transparent {
                                return f.node;
                            }
                        }
                    }
                }
            }
            for c in b.children.iter().rev().filter(|c| c.flow.is_float()) {
                if let Some(n) = self.hit(c, x, y) {
                    return Some(n);
                }
            }
        } else {
            for c in b.children.iter().rev().filter(|c| c.flow != Flow::Absolute) {
                if let Some(n) = self.hit(c, x, y) {
                    return Some(n);
                }
            }
        }
        if b.rect.contains(x, y) && !transparent {
            return b.node;
        }
        None
    }
}

pub fn layout_document(ctx: &LayoutContext) -> LayoutTree {
    let mut layouter = crate::block::Layouter::new(ctx);
    layouter.layout_document()
}

/// Child-index path from `root` to the box of `node`, descending only into
/// anonymous boxes and boxes of `node`'s DOM ancestors.
fn guided_path(root: &LayoutBox, doc: &rabbithole_dom::Document, node: NodeId) -> Option<Vec<usize>> {
    if node >= doc.nodes.len() {
        return None;
    }
    if root.node == Some(node) {
        return Some(Vec::new());
    }
    // Boxes follow the flat tree (shadow trees under their host, slotted
    // nodes under their slot).
    let mut ancestors = std::collections::HashSet::new();
    let mut cur = Some(node);
    while let Some(c) = cur {
        ancestors.insert(c);
        cur = doc.flat_parent(c);
    }
    fn walk(b: &LayoutBox, node: NodeId, anc: &std::collections::HashSet<NodeId>, path: &mut Vec<usize>) -> bool {
        for (i, c) in b.children.iter().enumerate() {
            if c.pseudo.is_some() {
                continue;
            }
            match c.node {
                Some(n) if n == node => {
                    path.push(i);
                    return true;
                }
                Some(n) if !anc.contains(&n) => continue,
                _ => {}
            }
            path.push(i);
            if walk(c, node, anc, path) {
                return true;
            }
            path.pop();
        }
        false
    }
    let mut path = Vec::new();
    walk(root, node, &ancestors, &mut path).then_some(path)
}
