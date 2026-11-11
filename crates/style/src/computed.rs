//! Computed style values. Every element gets one `ComputedStyle` with
//! absolute units (px) except percentages/auto, which layout resolves.

pub use rabbithole_gfx::{Color, Family};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Length {
    Px(f32),
    Percent(f32),
    Auto,
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
            Length::Auto | Length::MinContent | Length::MaxContent | Length::FitContent => None,
        }
    }
    pub fn resolve_or(&self, base: f32, default: f32) -> f32 {
        self.resolve(base).unwrap_or(default)
    }
    pub fn resolve_or_zero(&self, base: f32) -> f32 {
        self.resolve(base).unwrap_or(0.0)
    }
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
}

impl WhiteSpace {
    pub fn collapses(&self) -> bool {
        matches!(self, WhiteSpace::Normal | WhiteSpace::Nowrap | WhiteSpace::PreLine)
    }
    pub fn wraps(&self) -> bool {
        matches!(self, WhiteSpace::Normal | WhiteSpace::PreWrap | WhiteSpace::PreLine)
    }
    pub fn preserves_newlines(&self) -> bool {
        matches!(self, WhiteSpace::Pre | WhiteSpace::PreWrap | WhiteSpace::PreLine)
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
    pub letter_spacing: f32,
    pub word_spacing: f32,
    pub text_align: TextAlign,
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
    pub cursor_pointer: bool,

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
    pub clip_overflow_text: bool,

    // ---- text
    pub text_overflow: TextOverflow,
    pub word_break: WordBreak,
    pub overflow_wrap: OverflowWrap,
    pub text_decoration_color: Option<Color>,
    pub text_decoration_style: TextDecorationStyle,
    pub text_decoration_thickness: Option<f32>,
    pub direction: Direction,
    pub tab_size: f32,
    /// `-webkit-line-clamp`
    pub line_clamp: Option<u32>,
    /// Actual family names in order (e.g. `["Roboto", "Helvetica Neue"]`),
    /// matched against `@font-face` and system fonts before the generic
    /// `font_family` fallback.
    pub font_family_list: Vec<String>,
    pub font_variant_small_caps: bool,

    // ---- replaced elements / sizing
    pub object_fit: ObjectFit,
    pub object_position: (Length, Length),
    pub aspect_ratio: Option<f32>,
    /// `image-rendering: pixelated | crisp-edges` (nearest-neighbour
    /// scaling). Inherited.
    pub image_rendering_pixelated: bool,

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
            letter_spacing: 0.0,
            word_spacing: 0.0,
            text_align: TextAlign::Left,
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
            cursor_pointer: false,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: Length::Auto,
            flex_wrap: FlexWrap::NoWrap,
            justify_content: ContentDistribution::FlexStart,
            align_items: ItemAlign::Stretch,
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
            grid_template_areas: Vec::new(),
            grid_row_start: GridLine::Auto,
            grid_row_end: GridLine::Auto,
            grid_column_start: GridLine::Auto,
            grid_column_end: GridLine::Auto,
            justify_items: ItemAlign::Stretch,
            justify_self: ItemAlign::Auto,
            background_layers: Vec::new(),
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
            clip_overflow_text: false,
            text_overflow: TextOverflow::Clip,
            word_break: WordBreak::Normal,
            overflow_wrap: OverflowWrap::Normal,
            text_decoration_color: None,
            text_decoration_style: TextDecorationStyle::Solid,
            text_decoration_thickness: None,
            direction: Direction::Ltr,
            tab_size: 8.0,
            line_clamp: None,
            font_family_list: Vec::new(),
            font_variant_small_caps: false,
            object_fit: ObjectFit::Fill,
            object_position: (Length::Percent(50.0), Length::Percent(50.0)),
            aspect_ratio: None,
            image_rendering_pixelated: false,
            content: Content::Normal,
            counter_reset: Vec::new(),
            counter_increment: Vec::new(),
            quotes: Vec::new(),
            pointer_events: PointerEvents::Auto,
            user_select: UserSelect::Auto,
            resize: Resize::None,
            appearance: Appearance::Auto,
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
            letter_spacing: parent.letter_spacing,
            word_spacing: parent.word_spacing,
            text_align: parent.text_align,
            // Not inherited per spec, but decorations propagate to all
            // descendant inline content, which amounts to the same thing
            // for a renderer without pseudo-boxes.
            text_decoration: parent.text_decoration,
            text_transform: parent.text_transform,
            text_indent: parent.text_indent,
            white_space: parent.white_space,
            list_style_type: parent.list_style_type,
            list_style_position: parent.list_style_position,
            visibility: parent.visibility,
            cursor_pointer: parent.cursor_pointer,
            text_shadow: parent.text_shadow.clone(),
            word_break: parent.word_break,
            overflow_wrap: parent.overflow_wrap,
            direction: parent.direction,
            tab_size: parent.tab_size,
            font_family_list: parent.font_family_list.clone(),
            font_variant_small_caps: parent.font_variant_small_caps,
            image_rendering_pixelated: parent.image_rendering_pixelated,
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
            && self.letter_spacing == o.letter_spacing
            && self.word_spacing == o.word_spacing
            && self.text_align == o.text_align
            && self.text_decoration == o.text_decoration
            && self.text_transform == o.text_transform
            && self.text_indent == o.text_indent
            && self.white_space == o.white_space
            && self.list_style_type == o.list_style_type
            && self.list_style_position == o.list_style_position
            && self.visibility == o.visibility
            && self.cursor_pointer == o.cursor_pointer
            && self.text_shadow == o.text_shadow
            && self.word_break == o.word_break
            && self.overflow_wrap == o.overflow_wrap
            && self.direction == o.direction
            && self.tab_size == o.tab_size
            && self.font_family_list == o.font_family_list
            && self.font_variant_small_caps == o.font_variant_small_caps
            && self.image_rendering_pixelated == o.image_rendering_pixelated
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
