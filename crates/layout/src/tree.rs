//! Box tree types, the layout entry point, hit testing and debug dumps.

use crate::LayoutContext;
use rabbithole_dom::NodeId;
use rabbithole_gfx::{Color, FontId, Image, Rect};
use rabbithole_style::{ComputedStyle, Edges, TextDecoration};
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
    /// Simplified single-line row flexbox.
    FlexRow,
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
    pub image: Option<Rc<Image>>,
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
            } else if c.style.position == rabbithole_style::Position::Static {
                c.for_each_abs_child(f);
            }
        }
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
        let _ = writeln!(
            out,
            "{pad}{:?}{} {} [{:.1},{:.1} {:.1}x{:.1}]{}",
            self.kind,
            self.tag(None),
            match self.formatting {
                Formatting::Inline => "(inline)",
                Formatting::FlexRow => "(flex)",
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
}

impl LayoutTree {
    /// Deepest element box containing the point (document coordinates).
    pub fn node_at(&self, x: f32, y: f32) -> Option<NodeId> {
        hit(&self.root, x, y)
    }

    /// Human readable dump for debugging (`rabbithole dump --layout`).
    pub fn dump(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("document {:.0}x{:.0}\n", self.width, self.height));
        self.root.dump_into(0, &mut s);
        s
    }
}

fn hit(b: &LayoutBox, x: f32, y: f32) -> Option<NodeId> {
    // Absolutely positioned descendants are painted last → tested first.
    let mut found = None;
    b.for_each_abs_child(&mut |abs| {
        if found.is_none() {
            found = hit(abs, x, y);
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
                        if let Some(n) = hit(&b.children[*i], x, y) {
                            return Some(n);
                        }
                    }
                    _ => {
                        if f.node.is_some() {
                            return f.node;
                        }
                    }
                }
            }
        }
        for c in b.children.iter().rev().filter(|c| c.flow.is_float()) {
            if let Some(n) = hit(c, x, y) {
                return Some(n);
            }
        }
    } else {
        for c in b.children.iter().rev().filter(|c| c.flow != Flow::Absolute) {
            if let Some(n) = hit(c, x, y) {
                return Some(n);
            }
        }
    }
    if b.rect.contains(x, y) {
        return b.node;
    }
    None
}

pub fn layout_document(ctx: &LayoutContext) -> LayoutTree {
    let mut layouter = crate::block::Layouter::new(ctx);
    layouter.layout_document()
}
