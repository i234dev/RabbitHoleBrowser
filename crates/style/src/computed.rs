//! Computed style values. Every element gets one `ComputedStyle` with
//! absolute units (px) except percentages/auto, which layout resolves.

pub use rabbithole_gfx::{Color, Family};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Length {
    Px(f32),
    Percent(f32),
    Auto,
    /// `calc()` mixing a length and a percentage: px + percent% of the
    /// base (`calc(100% - 20px)` = `Calc(-20, 100)`).
    Calc(f32, f32),
    /// A math expression mixing percentages with lengths that is not a
    /// plain sum (`min(100%, 500px)`, `calc(100% - max(10%, 20px))`): an
    /// index into the interned [`MathNode`] table.
    Math(u32),
    /// `min-content` / `max-content` / `fit-content` sizing keywords;
    /// `resolve()` returns None for them, like `Auto`.
    MinContent,
    MaxContent,
    FitContent,
}

impl Length {
    pub const ZERO: Length = Length::Px(0.0);

    pub fn is_auto(&self) -> bool {
        matches!(self, Length::Auto)
    }
    pub fn is_intrinsic(&self) -> bool {
        matches!(self, Length::MinContent | Length::MaxContent | Length::FitContent)
    }
    /// Resolve against `base` (the containing block dimension). `Auto` → None.
    pub fn resolve(&self, base: f32) -> Option<f32> {
        match *self {
            Length::Px(v) => Some(v),
            Length::Percent(p) => Some(base * p / 100.0),
            Length::Calc(px, p) => Some(px + base * p / 100.0),
            Length::Math(id) => Some(math_eval(id, base)),
            Length::Auto | Length::MinContent | Length::MaxContent | Length::FitContent => None,
        }
    }
    /// Resolve against an optional base: percentages (and calc() with
    /// percentages) need a definite base.
    pub fn resolve_definite(&self, base: Option<f32>) -> Option<f32> {
        match *self {
            Length::Px(v) => Some(v),
            Length::Percent(_) | Length::Calc(..) | Length::Math(_) => base.and_then(|b| self.resolve(b)),
            _ => None,
        }
    }
    /// Does the value depend on its percentage base (percentage, or calc()
    /// / math function containing one)?
    pub fn has_percent(&self) -> bool {
        matches!(self, Length::Percent(_) | Length::Calc(..) | Length::Math(_))
    }
    pub fn resolve_or(&self, base: f32, default: f32) -> f32 {
        self.resolve(base).unwrap_or(default)
    }
    pub fn resolve_or_zero(&self, base: f32) -> f32 {
        self.resolve(base).unwrap_or(0.0)
    }
}

/// A computed math expression over (px, percent) terms, evaluated against
/// the percentage base at layout time.
#[derive(Debug, Clone, PartialEq)]
pub enum MathNode {
    /// px + percent% of the base.
    Leaf(f32, f32),
    Sum(Vec<MathNode>),
    Scale(Box<MathNode>, f32),
    Min(Vec<MathNode>),
    Max(Vec<MathNode>),
    /// `clamp(min, value, max)`.
    Clamp(Box<[MathNode; 3]>),
}

impl MathNode {
    pub fn eval(&self, base: f32) -> f32 {
        match self {
            MathNode::Leaf(px, p) => px + base * p / 100.0,
            MathNode::Sum(v) => v.iter().map(|n| n.eval(base)).sum(),
            MathNode::Scale(n, f) => n.eval(base) * f,
            MathNode::Min(v) => v.iter().map(|n| n.eval(base)).fold(f32::INFINITY, f32::min),
            MathNode::Max(v) => v.iter().map(|n| n.eval(base)).fold(f32::NEG_INFINITY, f32::max),
            MathNode::Clamp(a) => {
                let (lo, v, hi) = (a[0].eval(base), a[1].eval(base), a[2].eval(base));
                v.min(hi).max(lo)
            }
        }
    }
    /// Does any term depend on the base?
    pub fn has_percent(&self) -> bool {
        match self {
            MathNode::Leaf(_, p) => *p != 0.0,
            MathNode::Sum(v) | MathNode::Min(v) | MathNode::Max(v) => v.iter().any(|n| n.has_percent()),
            MathNode::Scale(n, _) => n.has_percent(),
            MathNode::Clamp(a) => a.iter().any(|n| n.has_percent()),
        }
    }
    /// CSS serialisation (`min(100%, 500px)`, `calc(100% - 20px)`).
    pub fn to_css(&self) -> String {
        fn num(v: f32) -> String {
            format!("{}", (v * 1000.0).round() / 1000.0)
        }
        fn inner(n: &MathNode) -> String {
            match n {
                MathNode::Leaf(px, p) if *p == 0.0 => format!("{}px", num(*px)),
                MathNode::Leaf(px, p) if *px == 0.0 => format!("{}%", num(*p)),
                MathNode::Leaf(px, p) if *px < 0.0 => format!("{}% - {}px", num(*p), num(-px)),
                MathNode::Leaf(px, p) => format!("{}% + {}px", num(*p), num(*px)),
                MathNode::Sum(v) => v.iter().map(inner).collect::<Vec<_>>().join(" + "),
                MathNode::Scale(n, f) => format!("{} * ({})", num(*f), inner(n)),
                other => other.to_css(),
            }
        }
        let list = |v: &[MathNode]| v.iter().map(inner).collect::<Vec<_>>().join(", ");
        match self {
            MathNode::Min(v) => format!("min({})", list(v)),
            MathNode::Max(v) => format!("max({})", list(v)),
            MathNode::Clamp(a) => format!("clamp({})", list(&a[..])),
            MathNode::Leaf(px, p) if *p == 0.0 => format!("{}px", num(*px)),
            MathNode::Leaf(px, p) if *px == 0.0 => format!("{}%", num(*p)),
            other => format!("calc({})", inner(other)),
        }
    }
}

struct MathTable {
    nodes: Vec<std::sync::Arc<MathNode>>,
    ids: std::collections::HashMap<String, u32>,
}

static MATH_TABLE: std::sync::LazyLock<std::sync::RwLock<MathTable>> = std::sync::LazyLock::new(|| std::sync::RwLock::new(MathTable { nodes: Vec::new(), ids: std::collections::HashMap::new() }));

/// Intern a math expression, returning its [`Length::Math`] id (equal
/// expressions share one).
pub fn intern_math(n: MathNode) -> u32 {
    let key = format!("{n:?}");
    if let Some(&id) = MATH_TABLE.read().unwrap_or_else(|e| e.into_inner()).ids.get(&key) {
        return id;
    }
    let mut t = MATH_TABLE.write().unwrap_or_else(|e| e.into_inner());
    if let Some(&id) = t.ids.get(&key) {
        return id;
    }
    let id = t.nodes.len() as u32;
    t.nodes.push(std::sync::Arc::new(n));
    t.ids.insert(key, id);
    id
}

/// The interned expression `id`.
pub fn math_node(id: u32) -> Option<std::sync::Arc<MathNode>> {
    MATH_TABLE.read().unwrap_or_else(|e| e.into_inner()).nodes.get(id as usize).cloned()
}

/// Evaluate the interned expression `id` against `base`.
pub fn math_eval(id: u32, base: f32) -> f32 {
    math_node(id).map(|n| n.eval(base)).unwrap_or(0.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Display {
    None,
    Block,
    Inline,
    InlineBlock,
    ListItem,
    Flex,
    InlineFlex,
    Grid,
    InlineGrid,
    Table,
    InlineTable,
    TableRow,
    TableRowGroup,
    TableHeaderGroup,
    TableFooterGroup,
    TableCell,
    TableCaption,
    TableColumn,
    TableColumnGroup,
    FlowRoot,
    Contents,
}

impl Display {
    pub fn is_none(&self) -> bool {
        matches!(self, Display::None)
    }
    /// Inline-level outer display (participates in an inline formatting context).
    pub fn is_inline_level(&self) -> bool {
        matches!(
            self,
            Display::Inline | Display::InlineBlock | Display::InlineFlex | Display::InlineGrid | Display::InlineTable | Display::TableCell
        )
    }
    pub fn is_block_level(&self) -> bool {
        !self.is_inline_level() && !self.is_none() && !matches!(self, Display::Contents | Display::TableColumn | Display::TableColumnGroup)
    }
    /// Does this box establish its own block container for children laid
    /// out as blocks (true for anything except plain `inline`)?
    pub fn is_atomic_inline(&self) -> bool {
        matches!(self, Display::InlineBlock | Display::InlineFlex | Display::InlineGrid | Display::InlineTable | Display::TableCell)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Float {
    None,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clear {
    None,
    Left,
    Right,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    /// `start` (the initial value): left in ltr, right in rtl.
    Start,
    End,
    Left,
    Right,
    Center,
    Justify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhiteSpace {
    Normal,
    Nowrap,
    Pre,
    PreWrap,
    PreLine,
    BreakSpaces,
}

impl WhiteSpace {
    pub fn collapses(&self) -> bool {
        matches!(self, WhiteSpace::Normal | WhiteSpace::Nowrap | WhiteSpace::PreLine)
    }
    pub fn wraps(&self) -> bool {
        matches!(self, WhiteSpace::Normal | WhiteSpace::PreWrap | WhiteSpace::PreLine | WhiteSpace::BreakSpaces)
    }
    pub fn preserves_newlines(&self) -> bool {
        matches!(self, WhiteSpace::Pre | WhiteSpace::PreWrap | WhiteSpace::PreLine | WhiteSpace::BreakSpaces)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontStyle {
    Normal,
    Italic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListStyleType {
    None,
    Disc,
    Circle,
    Square,
    Decimal,
    DecimalLeadingZero,
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListStylePosition {
    Inside,
    Outside,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overflow {
    Visible,
    Hidden,
    Scroll,
    Auto,
    Clip,
}

impl Overflow {
    pub fn clips(&self) -> bool {
        !matches!(self, Overflow::Visible)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VerticalAlign {
    Baseline,
    Middle,
    Top,
    Bottom,
    TextTop,
    TextBottom,
    Sub,
    Super,
    /// Offset in px above the baseline.
    Length(f32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderStyle {
    None,
    Solid,
    Dashed,
    Dotted,
    Double,
}

impl BorderStyle {
    pub fn is_visible(&self) -> bool {
        !matches!(self, BorderStyle::None)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextTransform {
    None,
    Uppercase,
    Lowercase,
    Capitalize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxSizing {
    ContentBox,
    BorderBox,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Visible,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlexDirection {
    Row,
    RowReverse,
    Column,
    ColumnReverse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextDecoration {
    pub underline: bool,
    pub overline: bool,
    pub line_through: bool,
}

impl TextDecoration {
    pub fn any(&self) -> bool {
        self.underline || self.overline || self.line_through
    }
}

/// `text-emphasis-style` (inherited).
#[derive(Debug, Clone, PartialEq, Default)]
pub enum EmphasisStyle {
    #[default]
    None,
    /// A shape: 0 = the writing mode's default (circle horizontally,
    /// sesame vertically), 1 dot, 2 circle, 3 double-circle, 4 triangle,
    /// 5 sesame.
    Shape { open: bool, shape: u8 },
    /// A `<string>` mark (its first grapheme cluster).
    Custom(String),
}

impl EmphasisStyle {
    /// The mark drawn, for a horizontal (`vertical` false) or vertical line.
    pub fn mark(&self, vertical: bool) -> Option<String> {
        match self {
            EmphasisStyle::None => None,
            EmphasisStyle::Custom(s) => {
                let mut it = s.chars();
                let first = it.next()?;
                let mut m = first.to_string();
                // Keep combining marks / variation selectors with the base.
                for c in it {
                    if matches!(c as u32, 0x300..=0x36F | 0xFE00..=0xFE0F | 0x200D | 0x3099 | 0x309A) {
                        m.push(c);
                    } else {
                        break;
                    }
                }
                Some(m)
            }
            EmphasisStyle::Shape { open, shape } => {
                let shape = if *shape == 0 { if vertical { 5 } else { 2 } } else { *shape };
                let c = match (shape, open) {
                    (1, false) => '\u{2022}',
                    (1, true) => '\u{25E6}',
                    (2, false) => '\u{25CF}',
                    (2, true) => '\u{25CB}',
                    (3, false) => '\u{25C9}',
                    (3, true) => '\u{25CE}',
                    (4, false) => '\u{25B2}',
                    (4, true) => '\u{25B3}',
                    (5, false) => '\u{FE45}',
                    _ => '\u{FE46}',
                };
                Some(c.to_string())
            }
        }
    }
}

/// `text-underline-offset` (inherited).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum UnderlineOffset {
    #[default]
    Auto,
    Px(f32),
    /// Percentage of 1em (resolved against the decorating box's font size).
    Percent(f32),
}

/// `text-underline-position` (inherited).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UnderlinePosition {
    pub from_font: bool,
    pub under: bool,
    pub left: bool,
    pub right: bool,
}

/// A text decoration in effect on an element's text (CSS Text Decoration
/// 3 §2): the element's own `text-decoration-line` and the decorations
/// propagated from its decorating ancestors, with their color, style,
/// thickness and underline placement taken from the decorating box.
#[derive(Debug, Clone, PartialEq)]
pub struct AppliedDecoration {
    pub lines: TextDecoration,
    pub color: Color,
    pub style: TextDecorationStyle,
    /// Thickness in px; `None` = `auto` / `from-font`.
    pub thickness: Option<f32>,
    pub from_font: bool,
    /// Underline offset in px; `None` = `auto`.
    pub underline_offset: Option<f32>,
    pub underline_position: UnderlinePosition,
    /// The decorating box's font (its ascent places overlines).
    pub font_size: f32,
    pub font_family: Family,
    pub font_family_list: Vec<String>,
    pub font_weight: u16,
    pub font_style: FontStyle,
    /// The decorating box's baseline shift (`vertical-align` of an inline
    /// decorating box), in px upwards.
    pub baseline_shift: f32,
    /// `text-decoration-inset` (start, end); percentages of the
    /// decorating box's inline size.
    pub inset: (Length, Length),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edges<T> {
    pub top: T,
    pub right: T,
    pub bottom: T,
    pub left: T,
}

impl<T: Copy> Edges<T> {
    pub fn all(v: T) -> Edges<T> {
        Edges { top: v, right: v, bottom: v, left: v }
    }
    pub fn as_array(&self) -> [T; 4] {
        [self.top, self.right, self.bottom, self.left]
    }
}

impl Edges<f32> {
    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }
    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}


// ------------------------------------------------------------ flexbox / grid

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlexWrap {
    NoWrap,
    Wrap,
    WrapReverse,
}

/// `justify-content` / `align-content` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentDistribution {
    FlexStart,
    FlexEnd,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
    Stretch,
}

/// `align-items` / `align-self` / `justify-items` / `justify-self` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemAlign {
    /// `auto` (align-self / justify-self only): use the container's value.
    Auto,
    /// `normal`: stretch for grid and flex items (start for replaced
    /// grid items and in the stacking axis of grid lanes).
    Normal,
    Stretch,
    FlexStart,
    FlexEnd,
    Center,
    Baseline,
}

/// One grid track size.
#[derive(Debug, Clone, PartialEq)]
pub enum TrackSize {
    /// `px` / `%` / `auto` / `min-content` / `max-content`
    Length(Length),
    /// `<n>fr`
    Fr(f32),
    /// `minmax(min, max)`
    MinMax(Box<TrackSize>, Box<TrackSize>),
    /// `fit-content(limit)`
    FitContent(Length),
    /// `calc()` / `min()` / `max()` / `clamp()` mixing percentages with
    /// lengths (`clamp(100px, 50%, calc(100% - 4rem))`): (px, percent)
    /// terms resolved against the grid container's size at layout.
    Calc(TrackCalc, Vec<(f32, f32)>),
}

/// How the terms of a [`TrackSize::Calc`] combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackCalc {
    /// One linear term (`calc()`).
    Sum,
    Min,
    Max,
    /// `clamp(min, value, max)`.
    Clamp,
}

impl TrackCalc {
    /// The track size for a container of size `base`.
    pub fn eval(self, terms: &[(f32, f32)], base: f32) -> f32 {
        let vals: Vec<f32> = terms.iter().map(|(px, pct)| px + base * pct / 100.0).collect();
        match self {
            TrackCalc::Sum => vals.first().copied().unwrap_or(0.0),
            TrackCalc::Min => vals.iter().cloned().fold(f32::INFINITY, f32::min),
            TrackCalc::Max => vals.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
            TrackCalc::Clamp if vals.len() == 3 => vals[1].min(vals[2]).max(vals[0]),
            TrackCalc::Clamp => vals.first().copied().unwrap_or(0.0),
        }
    }
}

/// A track list entry: a sized track, optionally preceded by line names,
/// or a `repeat()`.
#[derive(Debug, Clone, PartialEq)]
pub enum TrackListItem {
    Track {
        names: Vec<String>,
        size: TrackSize,
    },
    Repeat {
        /// `None` = `auto-fill`, `Some(false)` = `auto-fit` is encoded via `auto_fit`.
        count: Option<u32>,
        auto_fit: bool,
        tracks: Vec<TrackListItem>,
    },
    /// Trailing line names after the last track.
    Names(Vec<String>),
}

/// `grid-row-start` etc.
#[derive(Debug, Clone, PartialEq)]
pub enum GridLine {
    Auto,
    /// `<integer>` (negative counts from the end)
    Line(i32),
    /// `span <n>`
    Span(u32),
    /// `<custom-ident>` or `span <ident>` (name, span)
    Named(String, bool),
}

/// `grid-lanes-direction: normal | [ row | column ] [ fill-reverse || track-reverse ]?`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GridLanesDirection {
    /// `None` = `normal`; `Some(true)` = `row` (row lanes: the block
    /// axis is the grid axis), `Some(false)` = `column`.
    pub rows: Option<bool>,
    pub fill_reverse: bool,
    pub track_reverse: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridAutoFlow {
    Row,
    Column,
    RowDense,
    ColumnDense,
}

// --------------------------------------------------------------- backgrounds

#[derive(Debug, Clone, PartialEq)]
pub struct GradientStop {
    pub color: Color,
    /// Position along the gradient line (percentage of its length or px);
    /// `None` = evenly distributed.
    pub position: Option<Length>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GradientDirection {
    /// Angle in radians, 0 = to top, clockwise (CSS convention).
    Angle(f32),
    /// `to left top` etc. as (horizontal, vertical) in -1/0/1.
    Corner(i8, i8),
}

#[derive(Debug, Clone, PartialEq)]
pub enum RadialShape {
    Circle,
    Ellipse,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RadialSize {
    ClosestSide,
    FarthestSide,
    ClosestCorner,
    FarthestCorner,
    Explicit(Length, Length),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BackgroundImage {
    None,
    /// Resolved-as-written URL string (the engine resolves it against the
    /// stylesheet / document base).
    Url(String),
    LinearGradient {
        direction: GradientDirection,
        stops: Vec<GradientStop>,
        repeating: bool,
    },
    RadialGradient {
        shape: RadialShape,
        size: RadialSize,
        /// Center position (x, y) as lengths/percentages.
        position: (Length, Length),
        stops: Vec<GradientStop>,
        repeating: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BackgroundSize {
    Auto,
    Cover,
    Contain,
    /// Explicit width/height; `Auto` keeps the aspect ratio.
    Explicit(Length, Length),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundRepeat {
    Repeat,
    NoRepeat,
    Space,
    Round,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxArea {
    BorderBox,
    PaddingBox,
    ContentBox,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundLayer {
    pub image: BackgroundImage,
    pub size: BackgroundSize,
    /// Position of the image inside the positioning area (x, y); percentages
    /// use the CSS background-position formula.
    pub position: (Length, Length),
    pub repeat: (BackgroundRepeat, BackgroundRepeat),
    pub origin: BoxArea,
    pub clip: BoxArea,
    /// `background-attachment: fixed`
    pub fixed: bool,
}

impl Default for BackgroundLayer {
    fn default() -> Self {
        BackgroundLayer {
            image: BackgroundImage::None,
            size: BackgroundSize::Auto,
            position: (Length::Percent(0.0), Length::Percent(0.0)),
            repeat: (BackgroundRepeat::Repeat, BackgroundRepeat::Repeat),
            origin: BoxArea::PaddingBox,
            clip: BoxArea::BorderBox,
            fixed: false,
        }
    }
}

// ------------------------------------------------------- shadows / transforms

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shadow {
    pub x: f32,
    pub y: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: Color,
    pub inset: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TransformOp {
    Translate(Length, Length),
    Scale(f32, f32),
    /// radians
    Rotate(f32),
    /// radians (x, y)
    Skew(f32, f32),
    /// `matrix(a, b, c, d, e, f)`
    Matrix([f32; 6]),
}

// --------------------------------------------------------------- text / misc

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextOverflow {
    Clip,
    Ellipsis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordBreak {
    Normal,
    BreakAll,
    KeepAll,
    BreakWord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverflowWrap {
    Normal,
    Anywhere,
    BreakWord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectFit {
    Fill,
    Contain,
    Cover,
    None,
    ScaleDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Ltr,
    Rtl,
}

/// A `filter` / `backdrop-filter` function (Filter Effects 1 §5).
#[derive(Debug, Clone, PartialEq)]
pub enum FilterOp {
    /// Standard deviation in px.
    Blur(f32),
    Brightness(f32),
    Contrast(f32),
    Grayscale(f32),
    Sepia(f32),
    Saturate(f32),
    /// Radians.
    HueRotate(f32),
    Invert(f32),
    Opacity(f32),
    DropShadow { x: f32, y: f32, blur: f32, color: Color },
}

/// `writing-mode` (`sideways-rl` is treated as `vertical-rl`: Latin text
/// is set sideways in both).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritingMode {
    HorizontalTb,
    VerticalRl,
    VerticalLr,
    /// Lines left to right, text bottom to top.
    SidewaysLr,
}

impl WritingMode {
    pub fn is_vertical(self) -> bool {
        self != WritingMode::HorizontalTb
    }
}

/// `unicode-bidi`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnicodeBidi {
    Normal,
    Embed,
    Isolate,
    BidiOverride,
    IsolateOverride,
    Plaintext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextDecorationStyle {
    Solid,
    Double,
    Dotted,
    Dashed,
    Wavy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerEvents {
    Auto,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserSelect {
    Auto,
    None,
    Text,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resize {
    None,
    Both,
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Auto,
    None,
}

/// Items of the `content` property (for `::before` / `::after`).
#[derive(Debug, Clone, PartialEq)]
pub enum ContentItem {
    String(String),
    /// `attr(name)`
    Attr(String),
    /// `counter(name, style)`
    Counter(String, ListStyleType),
    /// `counters(name, separator, style)`
    Counters(String, String, ListStyleType),
    OpenQuote,
    CloseQuote,
    NoOpenQuote,
    NoCloseQuote,
    /// `url(...)` image
    Url(String),
}

/// `content` value.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    /// `normal` (for pseudo-elements: no box is generated)
    Normal,
    None,
    Items(Vec<ContentItem>),
}

/// Parsed `transition` entries (property, duration s, delay s, easing name).
#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    pub property: String,
    pub duration: f32,
    pub delay: f32,
    pub timing: String,
}

/// Parsed `animation` shorthand.
#[derive(Debug, Clone, PartialEq)]
pub struct Animation {
    pub name: String,
    pub duration: f32,
    pub delay: f32,
    pub timing: String,
    /// `None` = infinite
    pub iterations: Option<f32>,
    pub direction: String,
    pub fill_mode: String,
}

/// Simple affine matrix helpers for transforms: [a, b, c, d, e, f] with
/// x' = a·x + c·y + e, y' = b·x + d·y + f.
pub fn transform_matrix(ops: &[TransformOp], width: f32, height: f32, font_size: f32) -> [f32; 6] {
    let mut m = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    for op in ops {
        let o = match op {
            TransformOp::Translate(x, y) => {
                let _ = font_size;
                [1.0, 0.0, 0.0, 1.0, x.resolve_or_zero(width), y.resolve_or_zero(height)]
            }
            TransformOp::Scale(x, y) => [*x, 0.0, 0.0, *y, 0.0, 0.0],
            TransformOp::Rotate(a) => [a.cos(), a.sin(), -a.sin(), a.cos(), 0.0, 0.0],
            TransformOp::Skew(x, y) => [1.0, y.tan(), x.tan(), 1.0, 0.0, 0.0],
            TransformOp::Matrix(mm) => *mm,
        };
        m = multiply_matrix(m, o);
    }
    m
}

/// `m × n` (apply `n` first, then `m`).
pub fn multiply_matrix(m: [f32; 6], n: [f32; 6]) -> [f32; 6] {
    [
        m[0] * n[0] + m[2] * n[1],
        m[1] * n[0] + m[3] * n[1],
        m[0] * n[2] + m[2] * n[3],
        m[1] * n[2] + m[3] * n[3],
        m[0] * n[4] + m[2] * n[5] + m[4],
        m[1] * n[4] + m[3] * n[5] + m[5],
    ]
}

/// Computed values for one element.
#[derive(Debug, Clone, PartialEq)]
pub struct ComputedStyle {
    pub display: Display,
    pub position: Position,
    pub float: Float,
    pub clear: Clear,
    pub flex_direction: FlexDirection,

    pub width: Length,
    pub height: Length,
    /// `Auto` means "none" for max-*.
    pub min_width: Length,
    pub max_width: Length,
    pub min_height: Length,
    pub max_height: Length,

    pub margin: Edges<Length>,
    pub padding: Edges<Length>,
    pub border_width: Edges<f32>,
    pub border_style: Edges<BorderStyle>,
    pub border_color: Edges<Color>,
    /// Single radius for all corners, px.
    pub border_radius: f32,
    /// top/right/bottom/left offsets for positioned boxes.
    pub inset: Edges<Length>,
    pub box_sizing: BoxSizing,

    pub color: Color,
    pub background_color: Color,
    /// Resolved URL string of `background-image: url(...)`, if any.
    pub background_image: Option<String>,
    pub opacity: f32,

    pub font_family: Family,
    /// px
    pub font_size: f32,
    /// 100..=900
    pub font_weight: u16,
    pub font_style: FontStyle,
    /// px; None = `normal`
    pub line_height: Option<f32>,
    /// A unitless `line-height` (the number is inherited, not the px
    /// value: `line_height` is recomputed for each font size).
    pub line_height_number: Option<f32>,
    pub letter_spacing: f32,
    pub word_spacing: f32,
    pub text_align: TextAlign,
    /// `text-align: -webkit-center` (`<center>`, `align=center`): also
    /// centers block-level children with non-auto margins. Inherited.
    pub text_align_webkit_center: bool,
    pub text_decoration: TextDecoration,
    pub text_transform: TextTransform,
    pub text_indent: Length,
    pub white_space: WhiteSpace,
    pub vertical_align: VerticalAlign,

    pub list_style_type: ListStyleType,
    pub list_style_position: ListStylePosition,
    pub overflow_x: Overflow,
    pub overflow_y: Overflow,
    pub visibility: Visibility,
    pub z_index: Option<i32>,
    /// `cursor` keyword (one of [`CURSOR_KEYWORDS`]).
    pub cursor: &'static str,

    // ---- flexbox
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub flex_basis: Length,
    pub flex_wrap: FlexWrap,
    pub justify_content: ContentDistribution,
    pub align_items: ItemAlign,
    pub align_self: ItemAlign,
    pub align_content: ContentDistribution,
    pub order: i32,
    pub row_gap: Length,
    pub column_gap: Length,

    // ---- grid
    pub grid_template_columns: Vec<TrackListItem>,
    pub grid_template_rows: Vec<TrackListItem>,
    pub grid_auto_columns: Vec<TrackSize>,
    pub grid_auto_rows: Vec<TrackSize>,
    pub grid_auto_flow: GridAutoFlow,
    /// `display: grid-lanes` / `inline-grid-lanes` (CSS Grid 3): `display`
    /// is `Grid` / `InlineGrid` with this flag.
    pub grid_lanes: bool,
    /// `grid-lanes-direction`.
    pub grid_lanes_direction: GridLanesDirection,
    /// `grid-lanes-pack: dense`.
    pub grid_lanes_dense: bool,
    /// `fit-tolerance` (`None` = `normal`, 1em in grid lanes layout;
    /// `Some(INFINITY)` = `infinite`).
    pub fit_tolerance: Option<Length>,
    /// Rows of area names (`.` for none).
    pub grid_template_areas: Vec<Vec<String>>,
    pub grid_row_start: GridLine,
    pub grid_row_end: GridLine,
    pub grid_column_start: GridLine,
    pub grid_column_end: GridLine,
    pub justify_items: ItemAlign,
    pub justify_self: ItemAlign,

    // ---- backgrounds, borders, effects
    /// Background layers, first = topmost (CSS order). `background_color`
    /// is painted beneath all layers.
    pub background_layers: Vec<BackgroundLayer>,
    /// `mask-image` layers (with `mask-size`, `-position`, `-repeat`,
    /// `-origin`, `-clip`): the alpha of their images masks the box.
    pub mask_layers: Vec<BackgroundLayer>,
    /// Corner radii (top-left, top-right, bottom-right, bottom-left), each
    /// (horizontal, vertical). `border_radius` keeps the largest for
    /// compatibility with older code paths.
    pub border_radii: [(Length, Length); 4],
    pub box_shadow: Vec<Shadow>,
    pub text_shadow: Vec<Shadow>,
    pub transform: Vec<TransformOp>,
    pub transform_origin: (Length, Length),
    pub outline_width: f32,
    pub outline_style: BorderStyle,
    /// `None` = `invert`/currentcolor
    pub outline_color: Option<Color>,
    pub outline_offset: f32,
    /// `filter: opacity()/blur()/grayscale()` — only opacity is applied (multiplied into `opacity`).
    pub backdrop: bool,
    /// SVG presentation properties declared for the element by the page
    /// (`fill`, `stroke`, ...), as specified tokens, for the SVG renderer.
    pub svg_props: Vec<(String, Vec<rabbithole_css::Token>)>,
    /// `filter` functions (in order).
    pub filters: Vec<FilterOp>,
    /// `backdrop-filter` functions.
    pub backdrop_filters: Vec<FilterOp>,
    pub clip_overflow_text: bool,

    // ---- text
    pub text_overflow: TextOverflow,
    pub word_break: WordBreak,
    pub overflow_wrap: OverflowWrap,
    pub text_decoration_color: Option<Color>,
    pub text_decoration_style: TextDecorationStyle,
    pub text_decoration_thickness: Option<f32>,
    /// `text-decoration-thickness: from-font`.
    pub text_decoration_thickness_from_font: bool,
    /// `text-decoration-inset` (start, end; not inherited). Percentages
    /// are kept as `Length::Percent` of the decorating box's font size.
    pub text_decoration_inset: (Length, Length),
    /// `text-underline-offset` (inherited).
    pub text_underline_offset: UnderlineOffset,
    /// `text-emphasis-style` (inherited).
    pub text_emphasis_style: EmphasisStyle,
    /// `text-emphasis-color` (inherited; `None` = currentcolor).
    pub text_emphasis_color: Option<Color>,
    /// `text-emphasis-position` (inherited): `over` (else under) and
    /// `right` (else left).
    pub text_emphasis_over: bool,
    pub text_emphasis_right: bool,
    /// `ruby-position: under` (inherited).
    pub ruby_under: bool,
    /// `mix-blend-mode` (see `rabbithole_gfx::blend_mode_index`; 0 =
    /// normal).
    pub mix_blend_mode: u8,
    /// `text-wrap-style: balance` (inherited).
    pub text_wrap_balance: bool,
    /// `font-synthesis` allows a synthesized oblique (inherited).
    pub font_synthesis_style: bool,
    /// `line-break` (inherited): 0 auto/normal, 1 loose, 2 strict,
    /// 3 anywhere.
    pub line_break: u8,
    /// Layout-internal: lines are as tall as the font's ascent + descent
    /// (ruby bases and annotations).
    pub line_height_tight: bool,
    /// `text-underline-position` (inherited).
    pub text_underline_position: UnderlinePosition,
    /// Decorations painted on this element's text (inherited: computed
    /// in the cascade from the parent's and the element's own).
    pub applied_decorations: Vec<AppliedDecoration>,
    pub direction: Direction,
    /// `writing-mode` (inherited).
    pub writing_mode: WritingMode,
    /// `unicode-bidi` (not inherited).
    pub unicode_bidi: UnicodeBidi,
    /// Layout-internal: a vertical writing mode's style transposed into
    /// its logical (horizontal) layout space.
    pub layout_transposed: bool,
    pub tab_size: f32,
    /// `-webkit-line-clamp`
    pub line_clamp: Option<u32>,
    /// `content-visibility: hidden`: the contents are not rendered.
    pub content_visibility_hidden: bool,
    /// `contain`: paint / layout / size containment.
    pub contain_paint: bool,
    pub contain_layout: bool,
    pub contain_size: bool,
    /// `contain-intrinsic-width` / `-height` (px; `None` = `none`): the
    /// size a size-contained box is laid out as.
    pub contain_intrinsic_width: Option<f32>,
    pub contain_intrinsic_height: Option<f32>,
    /// `line_clamp` came from `-webkit-line-clamp` (applies only to
    /// `display: -webkit-box` with `-webkit-box-orient: vertical`).
    pub line_clamp_prefixed: bool,
    /// `display: -webkit-box` / `-webkit-inline-box` (legacy flexbox).
    pub webkit_box: bool,
    /// Actual family names in order (e.g. `["Roboto", "Helvetica Neue"]`),
    /// matched against `@font-face` and system fonts before the generic
    /// `font_family` fallback.
    pub font_family_list: Vec<String>,
    pub font_variant_small_caps: bool,
    /// `font-variation-settings` (inherited): axis tag and value.
    pub font_variation_settings: Vec<([u8; 4], f32)>,

    // ---- replaced elements / sizing
    pub object_fit: ObjectFit,
    pub object_position: (Length, Length),
    pub aspect_ratio: Option<f32>,
    /// `image-rendering: pixelated | crisp-edges` (nearest-neighbour
    /// scaling). Inherited.
    pub image_rendering_pixelated: bool,
    /// `accent-color` (`None` = auto). Inherited.
    pub accent_color: Option<Color>,
    /// `color-scheme` includes `dark` (and not `light` first): native
    /// controls use dark colors. Inherited.
    pub color_scheme_dark: bool,

    // ---- generated content & counters
    pub content: Content,
    pub counter_reset: Vec<(String, i32)>,
    pub counter_increment: Vec<(String, i32)>,
    pub quotes: Vec<(String, String)>,

    // ---- interaction / misc
    pub pointer_events: PointerEvents,
    pub user_select: UserSelect,
    pub resize: Resize,
    pub appearance: Appearance,
    /// Author (non-UA) background or border declarations apply: a text
    /// field, button or menu then loses its native look (css-ui-4
    /// "appearance-disabling properties"), even when the values equal the
    /// UA's.
    pub author_widget_style: bool,
    pub transitions: Vec<Transition>,
    pub animations: Vec<Animation>,
    pub scroll_margin_top: f32,
    /// `contain`/`isolation`/`will-change` imply a stacking context.
    pub creates_stacking_context: bool,
    /// `border-collapse: collapse` (inherited; tables).
    pub border_collapse: bool,
    /// `border-spacing` (horizontal, vertical) in px (inherited; tables).
    pub border_spacing: (f32, f32),
    /// `table-layout: fixed` (not inherited).
    pub table_layout_fixed: bool,
    /// `caption-side: bottom` (inherited).
    pub caption_side_bottom: bool,
    /// `empty-cells: hide` (inherited).
    pub empty_cells_hide: bool,
}

impl ComputedStyle {
    /// Initial values (the root element inherits from this).
    pub fn initial() -> ComputedStyle {
        ComputedStyle {
            display: Display::Inline,
            position: Position::Static,
            float: Float::None,
            clear: Clear::None,
            flex_direction: FlexDirection::Row,
            width: Length::Auto,
            height: Length::Auto,
            min_width: Length::Auto,
            max_width: Length::Auto,
            min_height: Length::Auto,
            max_height: Length::Auto,
            margin: Edges::all(Length::ZERO),
            padding: Edges::all(Length::ZERO),
            border_width: Edges::all(0.0),
            border_style: Edges::all(BorderStyle::None),
            border_color: Edges::all(Color::BLACK),
            border_radius: 0.0,
            inset: Edges::all(Length::Auto),
            box_sizing: BoxSizing::ContentBox,
            color: Color::BLACK,
            background_color: Color::TRANSPARENT,
            background_image: None,
            opacity: 1.0,
            font_family: Family::Sans,
            font_size: 16.0,
            font_weight: 400,
            font_style: FontStyle::Normal,
            line_height: None,
            line_height_number: None,
            letter_spacing: 0.0,
            word_spacing: 0.0,
            text_align: TextAlign::Start,
            text_align_webkit_center: false,
            text_decoration: TextDecoration::default(),
            text_transform: TextTransform::None,
            text_indent: Length::ZERO,
            white_space: WhiteSpace::Normal,
            vertical_align: VerticalAlign::Baseline,
            list_style_type: ListStyleType::Disc,
            list_style_position: ListStylePosition::Outside,
            overflow_x: Overflow::Visible,
            overflow_y: Overflow::Visible,
            visibility: Visibility::Visible,
            z_index: None,
            cursor: "auto",
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: Length::Auto,
            flex_wrap: FlexWrap::NoWrap,
            justify_content: ContentDistribution::FlexStart,
            align_items: ItemAlign::Normal,
            align_self: ItemAlign::Auto,
            align_content: ContentDistribution::Stretch,
            order: 0,
            row_gap: Length::ZERO,
            column_gap: Length::ZERO,
            grid_template_columns: Vec::new(),
            grid_template_rows: Vec::new(),
            grid_auto_columns: Vec::new(),
            grid_auto_rows: Vec::new(),
            grid_auto_flow: GridAutoFlow::Row,
            grid_lanes: false,
            grid_lanes_direction: GridLanesDirection::default(),
            grid_lanes_dense: false,
            fit_tolerance: None,
            grid_template_areas: Vec::new(),
            grid_row_start: GridLine::Auto,
            grid_row_end: GridLine::Auto,
            grid_column_start: GridLine::Auto,
            grid_column_end: GridLine::Auto,
            justify_items: ItemAlign::Normal,
            justify_self: ItemAlign::Auto,
            background_layers: Vec::new(),
            mask_layers: Vec::new(),
            border_radii: [(Length::ZERO, Length::ZERO); 4],
            box_shadow: Vec::new(),
            text_shadow: Vec::new(),
            transform: Vec::new(),
            transform_origin: (Length::Percent(50.0), Length::Percent(50.0)),
            outline_width: 0.0,
            outline_style: BorderStyle::None,
            outline_color: None,
            outline_offset: 0.0,
            backdrop: false,
            svg_props: Vec::new(),
            filters: Vec::new(),
            backdrop_filters: Vec::new(),
            clip_overflow_text: false,
            text_overflow: TextOverflow::Clip,
            word_break: WordBreak::Normal,
            overflow_wrap: OverflowWrap::Normal,
            text_decoration_color: None,
            text_decoration_style: TextDecorationStyle::Solid,
            text_decoration_thickness: None,
            text_decoration_thickness_from_font: false,
            text_decoration_inset: (Length::ZERO, Length::ZERO),
            text_underline_offset: UnderlineOffset::Auto,
            text_emphasis_style: EmphasisStyle::None,
            text_emphasis_color: None,
            text_emphasis_over: true,
            text_emphasis_right: true,
            ruby_under: false,
            mix_blend_mode: 0,
            text_wrap_balance: false,
            font_synthesis_style: true,
            line_break: 0,
            line_height_tight: false,
            text_underline_position: UnderlinePosition::default(),
            applied_decorations: Vec::new(),
            direction: Direction::Ltr,
            writing_mode: WritingMode::HorizontalTb,
            unicode_bidi: UnicodeBidi::Normal,
            layout_transposed: false,
            tab_size: 8.0,
            line_clamp: None,
            content_visibility_hidden: false,
            contain_paint: false,
            contain_layout: false,
            contain_size: false,
            contain_intrinsic_width: None,
            contain_intrinsic_height: None,
            line_clamp_prefixed: false,
            webkit_box: false,
            font_family_list: Vec::new(),
            font_variant_small_caps: false,
            font_variation_settings: Vec::new(),
            object_fit: ObjectFit::Fill,
            object_position: (Length::Percent(50.0), Length::Percent(50.0)),
            aspect_ratio: None,
            image_rendering_pixelated: false,
            accent_color: None,
            color_scheme_dark: false,
            content: Content::Normal,
            counter_reset: Vec::new(),
            counter_increment: Vec::new(),
            quotes: Vec::new(),
            pointer_events: PointerEvents::Auto,
            user_select: UserSelect::Auto,
            resize: Resize::None,
            appearance: Appearance::Auto,
            author_widget_style: false,
            transitions: Vec::new(),
            animations: Vec::new(),
            scroll_margin_top: 0.0,
            creates_stacking_context: false,
            border_collapse: false,
            border_spacing: (0.0, 0.0),
            table_layout_fixed: false,
            caption_side_bottom: false,
            empty_cells_hide: false,
        }
    }

    /// A fresh style for a child: inherited properties copied from
    /// `parent`, everything else at its initial value.
    pub fn inherit_from(parent: &ComputedStyle) -> ComputedStyle {
        ComputedStyle {
            color: parent.color,
            font_family: parent.font_family,
            font_size: parent.font_size,
            font_weight: parent.font_weight,
            font_style: parent.font_style,
            line_height: parent.line_height,
            line_height_number: parent.line_height_number,
            letter_spacing: parent.letter_spacing,
            word_spacing: parent.word_spacing,
            text_align: parent.text_align,
            text_align_webkit_center: parent.text_align_webkit_center,
            // `text-decoration-line` is not inherited; the decorations it
            // draws propagate through `applied_decorations`.
            applied_decorations: parent.applied_decorations.clone(),
            text_underline_offset: parent.text_underline_offset,
            text_emphasis_style: parent.text_emphasis_style.clone(),
            text_emphasis_color: parent.text_emphasis_color,
            text_emphasis_over: parent.text_emphasis_over,
            text_emphasis_right: parent.text_emphasis_right,
            ruby_under: parent.ruby_under,
            text_wrap_balance: parent.text_wrap_balance,
            font_synthesis_style: parent.font_synthesis_style,
            line_break: parent.line_break,
            line_height_tight: parent.line_height_tight,
            text_underline_position: parent.text_underline_position,
            text_transform: parent.text_transform,
            text_indent: parent.text_indent,
            white_space: parent.white_space,
            list_style_type: parent.list_style_type,
            list_style_position: parent.list_style_position,
            visibility: parent.visibility,
            cursor: parent.cursor,
            text_shadow: parent.text_shadow.clone(),
            word_break: parent.word_break,
            overflow_wrap: parent.overflow_wrap,
            direction: parent.direction,
            writing_mode: parent.writing_mode,
            tab_size: parent.tab_size,
            font_family_list: parent.font_family_list.clone(),
            font_variant_small_caps: parent.font_variant_small_caps,
            font_variation_settings: parent.font_variation_settings.clone(),
            image_rendering_pixelated: parent.image_rendering_pixelated,
            accent_color: parent.accent_color,
            color_scheme_dark: parent.color_scheme_dark,
            quotes: parent.quotes.clone(),
            pointer_events: parent.pointer_events,
            user_select: parent.user_select,
            border_collapse: parent.border_collapse,
            border_spacing: parent.border_spacing,
            caption_side_bottom: parent.caption_side_bottom,
            empty_cells_hide: parent.empty_cells_hide,
            ..ComputedStyle::initial()
        }
    }

    /// Do two styles agree on every field [`ComputedStyle::inherit_from`]
    /// copies (keep both lists in sync)? Children of an element whose
    /// inherited part did not change need no restyle for inheritance.
    pub fn inherited_eq(&self, o: &ComputedStyle) -> bool {
        self.color == o.color
            && self.font_family == o.font_family
            && self.font_size == o.font_size
            && self.font_weight == o.font_weight
            && self.font_style == o.font_style
            && self.line_height == o.line_height
            && self.line_height_number == o.line_height_number
            && self.letter_spacing == o.letter_spacing
            && self.word_spacing == o.word_spacing
            && self.text_align == o.text_align
            && self.text_align_webkit_center == o.text_align_webkit_center
            && self.applied_decorations == o.applied_decorations
            && self.text_underline_offset == o.text_underline_offset
            && self.text_emphasis_style == o.text_emphasis_style
            && self.text_emphasis_color == o.text_emphasis_color
            && self.text_emphasis_over == o.text_emphasis_over
            && self.text_emphasis_right == o.text_emphasis_right
            && self.ruby_under == o.ruby_under
            && self.text_wrap_balance == o.text_wrap_balance
            && self.font_synthesis_style == o.font_synthesis_style
            && self.line_break == o.line_break
            && self.line_height_tight == o.line_height_tight
            && self.text_underline_position == o.text_underline_position
            && self.text_transform == o.text_transform
            && self.text_indent == o.text_indent
            && self.white_space == o.white_space
            && self.list_style_type == o.list_style_type
            && self.list_style_position == o.list_style_position
            && self.visibility == o.visibility
            && self.cursor == o.cursor
            && self.text_shadow == o.text_shadow
            && self.word_break == o.word_break
            && self.overflow_wrap == o.overflow_wrap
            && self.direction == o.direction
            && self.writing_mode == o.writing_mode
            && self.tab_size == o.tab_size
            && self.font_family_list == o.font_family_list
            && self.font_variant_small_caps == o.font_variant_small_caps
            && self.font_variation_settings == o.font_variation_settings
            && self.image_rendering_pixelated == o.image_rendering_pixelated
            && self.accent_color == o.accent_color
            && self.color_scheme_dark == o.color_scheme_dark
            && self.quotes == o.quotes
            && self.pointer_events == o.pointer_events
            && self.user_select == o.user_select
            && self.border_collapse == o.border_collapse
            && self.border_spacing == o.border_spacing
    }

    /// Resolved corner radii in px: [tl, tr, br, bl] × (h, v) for a box of
    /// the given size, scaled down per CSS when they would overlap.
    pub fn resolved_radii(&self, width: f32, height: f32) -> [(f32, f32); 4] {
        let mut r = [(0.0f32, 0.0f32); 4];
        for i in 0..4 {
            let (h, v) = self.border_radii[i];
            r[i] = (h.resolve_or_zero(width).max(0.0), v.resolve_or_zero(height).max(0.0));
        }
        let mut f: f32 = 1.0;
        if width > 0.0 {
            f = f.min(width / (r[0].0 + r[1].0).max(f32::EPSILON)).min(width / (r[3].0 + r[2].0).max(f32::EPSILON));
        }
        if height > 0.0 {
            f = f.min(height / (r[0].1 + r[3].1).max(f32::EPSILON)).min(height / (r[1].1 + r[2].1).max(f32::EPSILON));
        }
        if f < 1.0 {
            for c in &mut r {
                c.0 *= f;
                c.1 *= f;
            }
        }
        r
    }

    pub fn has_transform(&self) -> bool {
        !self.transform.is_empty()
    }

    /// Does this element establish a stacking context?
    pub fn is_stacking_context(&self) -> bool {
        self.opacity < 1.0
            || self.has_transform()
            || self.creates_stacking_context
            || (matches!(self.position, Position::Absolute | Position::Relative | Position::Sticky) && self.z_index.is_some())
            || self.position == Position::Fixed
    }

    /// Line height in px: explicit value or `normal` ≈ 1.2 × font-size.
    pub fn line_height_px(&self) -> f32 {
        self.line_height.unwrap_or(self.font_size * 1.2)
    }

    pub fn is_bold(&self) -> bool {
        self.font_weight >= 600
    }

    pub fn is_italic(&self) -> bool {
        self.font_style == FontStyle::Italic
    }

    pub fn has_visible_border(&self) -> bool {
        self.border_style.as_array().iter().any(|s| s.is_visible())
            && self.border_width.as_array().iter().any(|w| *w > 0.0)
    }
}

/// The `cursor` keywords (CSS Basic User Interface 4 §5.1).
pub const CURSOR_KEYWORDS: &[&str] = &[
    "auto", "default", "none", "context-menu", "help", "pointer", "progress", "wait", "cell", "crosshair", "text",
    "vertical-text", "alias", "copy", "move", "no-drop", "not-allowed", "grab", "grabbing", "e-resize", "n-resize",
    "ne-resize", "nw-resize", "s-resize", "se-resize", "sw-resize", "w-resize", "ew-resize", "ns-resize",
    "nesw-resize", "nwse-resize", "col-resize", "row-resize", "all-scroll", "zoom-in", "zoom-out",
];

impl ComputedStyle {
    /// Does the element ask for the link (hand) cursor?
    pub fn cursor_pointer(&self) -> bool {
        self.cursor == "pointer"
    }
}
