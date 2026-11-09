//! Computed style values. Every element gets one `ComputedStyle` with
//! absolute units (px) except percentages/auto, which layout resolves.

pub use rabbithole_gfx::{Color, Family};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Length {
    Px(f32),
    Percent(f32),
    Auto,
}

impl Length {
    pub const ZERO: Length = Length::Px(0.0);

    pub fn is_auto(&self) -> bool {
        matches!(self, Length::Auto)
    }
    /// Resolve against `base` (the containing block dimension). `Auto` → None.
    pub fn resolve(&self, base: f32) -> Option<f32> {
        match *self {
            Length::Px(v) => Some(v),
            Length::Percent(p) => Some(base * p / 100.0),
            Length::Auto => None,
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
            ..ComputedStyle::initial()
        }
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
