//! Box tree types, the layout entry point, hit testing and debug dumps.

use std::sync::Arc;
use crate::LayoutContext;
use rabbithole_css::PseudoElement;
use rabbithole_dom::NodeId;
use rabbithole_gfx::{Color, FontId, Image, Matrix, Rect};
use rabbithole_style::{
    transform_matrix, ComputedStyle, Edges, PointerEvents, Position, Shadow, TextDecoration, TextDecorationStyle,
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
    /// `display: table` — children are rows / row groups / captions.
    Table,
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

/// Text decoration details carried by a text fragment.
#[derive(Debug, Clone, PartialEq)]
pub struct DecorationInfo {
    pub lines: TextDecoration,
    /// `None` = the text color.
    pub color: Option<Color>,
    pub style: TextDecorationStyle,
    /// `None` = automatic (font-size / 16).
    pub thickness: Option<f32>,
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
        decoration: TextDecoration,
        /// Hide text but keep its space (`visibility: hidden`).
        hidden: bool,
        /// Decoration color/style/thickness.
        decoration_info: DecorationInfo,
        /// `text-shadow` list.
        shadows: Vec<Shadow>,
        /// `font-variant: small-caps` (synthesized).
        small_caps: bool,
        /// Synthetic oblique (italic requested, upright face chosen).
        oblique: bool,
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
    pub marker: Option<Marker>,
    /// Marker text computed at build time (before positioning).
    pub marker_text: Option<Rc<str>>,
    /// Baseline offset from `rect.y` (for inline-block alignment), if known.
    pub baseline: Option<f32>,
    /// Resolved margins (px) after layout.
    pub margin: Edges<f32>,
    /// Effective bottom margin after collapsing with descendants.
    pub collapsed_bottom_margin: f32,
    /// Static position hint for absolutely positioned boxes.
    pub static_pos: (f32, f32),
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
    pub(crate) pieces: Option<Vec<crate::inline::Piece>>,
    pub(crate) intrinsic_cache: Option<(f32, f32)>,
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
            marker: None,
            marker_text: None,
            baseline: None,
            margin: Edges::all(0.0),
            collapsed_bottom_margin: 0.0,
            static_pos: (0.0, 0.0),
            colspan: 1,
            bfc_root: false,
            clips: false,
            scroll_container: false,
            scroll_width: 0.0,
            scroll_height: 0.0,
            scroll_offset: (0.0, 0.0),
            fixed: false,
            pseudo: None,
            pieces: None,
            intrinsic_cache: None,
        }
    }

    /// Is this an inline-level box (participates in a line)?
    pub fn is_inline_level(&self) -> bool {
        matches!(
            self.kind,
            BoxKind::Inline | BoxKind::InlineBlock | BoxKind::Text | BoxKind::LineBreak | BoxKind::Image
        ) && self.flow.is_in_flow()
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

    /// Does this box establish a stacking context (CSS 2.1 Appendix E)?
    pub fn is_stacking_context(&self) -> bool {
        self.style.is_stacking_context() || self.fixed
    }

    /// Is the box positioned (any value other than `static`)?
    pub fn is_positioned(&self) -> bool {
        self.style.position != Position::Static
    }

    /// Transform matrix of this box (document coordinates), if any.
    pub fn transform_matrix(&self) -> Option<Matrix> {
        if self.style.transform.is_empty() {
            return None;
        }
        let r = self.rect;
        let m = transform_matrix(&self.style.transform, r.w, r.h, self.style.font_size);
        let ox = r.x + self.style.transform_origin.0.resolve_or_zero(r.w);
        let oy = r.y + self.style.transform_origin.1.resolve_or_zero(r.h);
        // T(origin) × M × T(-origin)
        let t1 = [1.0, 0.0, 0.0, 1.0, ox, oy];
        let t2 = [1.0, 0.0, 0.0, 1.0, -ox, -oy];
        Some(rabbithole_style::multiply_matrix(rabbithole_style::multiply_matrix(t1, m), t2))
    }

    /// `position: sticky` offset for the box given the nearest scrollport
    /// (document coordinates) and its containing block's content rect.
    pub fn sticky_offset(&self, scrollport: Rect, cb: Rect) -> (f32, f32) {
        if self.style.position != Position::Sticky {
            return (0.0, 0.0);
        }
        let s = &self.style;
        let mr = self.margin_rect();
        let mut dy = 0.0;
        if let Some(top) = s.inset.top.resolve(scrollport.h) {
            let want = scrollport.y + top;
            if mr.y < want {
                dy = want - mr.y;
                // Stay within the containing block.
                let max = (cb.bottom() - mr.h - mr.y).max(0.0);
                dy = dy.min(max);
            }
        }
        if dy == 0.0 {
            if let Some(bottom) = s.inset.bottom.resolve(scrollport.h) {
                let want = scrollport.bottom() - bottom - mr.h;
                if mr.y > want {
                    dy = want - mr.y;
                    let min = (cb.y - mr.y).min(0.0);
                    dy = dy.max(min);
                }
            }
        }
        let mut dx = 0.0;
        if let Some(left) = s.inset.left.resolve(scrollport.w) {
            let want = scrollport.x + left;
            if mr.x < want {
                dx = want - mr.x;
                let max = (cb.right() - mr.w - mr.x).max(0.0);
                dx = dx.min(max);
            }
        }
        if dx == 0.0 {
            if let Some(right) = s.inset.right.resolve(scrollport.w) {
                let want = scrollport.right() - right - mr.w;
                if mr.x > want {
                    dx = want - mr.x;
                    let min = (cb.x - mr.x).min(0.0);
                    dx = dx.max(min);
                }
            }
        }
        (dx, dy)
    }

    /// Move this box and everything inside it.
    pub fn translate(&mut self, dx: f32, dy: f32) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        self.rect = self.rect.translate(dx, dy);
        self.content_rect = self.content_rect.translate(dx, dy);
        self.static_pos = (self.static_pos.0 + dx, self.static_pos.1 + dy);
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
        if let Some(l) = self.lines.first() {
            return Some(l.baseline);
        }
        self.children
            .iter()
            .filter(|c| c.is_in_flow())
            .find_map(|c| c.first_baseline())
    }

    /// Baseline of the last line (absolute y).
    pub fn last_baseline(&self) -> Option<f32> {
        if let Some(l) = self.lines.last() {
            return Some(l.baseline);
        }
        self.children
            .iter()
            .rev()
            .filter(|c| c.is_in_flow())
            .find_map(|c| c.last_baseline())
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
}

impl LayoutTree {
    /// An empty tree (placeholder before the first layout).
    pub fn empty() -> LayoutTree {
        LayoutTree {
            root: LayoutBox::new(BoxKind::Block, None, Rc::new(ComputedStyle::initial())),
            width: 0.0,
            height: 0.0,
            viewport: (0.0, 0.0),
        }
    }

    /// Does the tree contain sticky or fixed boxes, inner scroll offsets or
    /// fixed backgrounds (i.e. does scrolling change the display list)?
    pub fn has_sticky_or_fixed(&self) -> bool {
        fn walk(b: &LayoutBox) -> bool {
            if b.fixed
                || b.style.position == rabbithole_style::Position::Sticky
                || b.style.background_layers.iter().any(|l| l.fixed)
                || b.scroll_container
            {
                return true;
            }
            b.children.iter().any(walk)
        }
        walk(&self.root)
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
            if b.clips && !b.rect.contains(x, y) {
                return None;
            }
            for c in b.children.iter().rev() {
                if let Some(s) = walk(c, x, y) {
                    return Some(s);
                }
            }
            if b.scroll_container && b.rect.contains(x, y) {
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
        if b.clips && !b.rect.contains(x, y) {
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
