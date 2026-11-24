//! CSS Transitions and CSS Animations.
//!
//! The style system computes the *base* style of every element. When a
//! restyle changes an animatable property of an element whose
//! `transition-*` covers it, a transition starts from the value shown
//! before the change; `animation-name` starting to match an `@keyframes`
//! rule starts an animation. While effects run, the element's entry in
//! `page.styles` is the base style with the animated values applied (an
//! *overlay*), refreshed by [`Engine::tick`]; the base style is kept here
//! so a later restyle can tell real changes from the overlay.
//! `transitionrun/start/end/cancel` and `animationstart/iteration/end/
//! cancel` events are queued and dispatched from the tick.

mod grid_tracks;

use rabbithole_style::props::motion::{OffsetPosition as MotionPosition, OffsetRotate as MotionRotation};

use grid_tracks::Operation as GridOperation;
use rabbithole_style::{AnimationComposition as Composite, TrackListItem};
use std::collections::HashMap;
use std::rc::Rc;

use rabbithole_dom::NodeId;
use rabbithole_gfx::Color;
use rabbithole_script::{EventDetail, EventInit};
use rabbithole_style::{AlignFlags, ContentDistribution, ItemAlign, BasicShape, ClipPath, ComputedStyle, ControlPoint, FilterOp, Length, ShapeCommand, ShapeRadius, TransformOp, Visibility};

use crate::scripting::ScriptOutcome;
use crate::{Engine, Page};

/// Animatable properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Prop {
    Opacity,
    Color,
    BackgroundColor,
    BorderTopColor,
    BorderRightColor,
    BorderBottomColor,
    BorderLeftColor,
    Width,
    Height,
    MinWidth,
    MaxWidth,
    MinHeight,
    MaxHeight,
    Top,
    Right,
    Bottom,
    Left,
    MarginTop,
    MarginRight,
    MarginBottom,
    MarginLeft,
    PaddingTop,
    PaddingRight,
    PaddingBottom,
    PaddingLeft,
    Transform,
    Translate,
    Rotate,
    Scale,
    Visibility,
    LetterSpacing,
    FontSize,
    BorderRadius,
    Filter,
    BackdropFilter,
    ClipPath,
    ShapeOutside,
    OffsetDistance,
    OffsetRotate,
    OffsetPosition,
    OffsetAnchor,
    OffsetPath,
    ShapeMargin,
    AlignContent,
    JustifyContent,
    AlignSelf,
    JustifySelf,
    AlignItems,
    JustifyItems,
    AnchorName,
    PositionAnchor,
    PositionArea,
    AnchorScope,
    GridTemplateColumns,
    GridTemplateRows,
    GridTemplateAreas,
    GridAutoColumns,
    GridAutoRows,
    GridAutoFlow,
    GridRowStart,
    GridRowEnd,
    GridColumnStart,
    GridColumnEnd,
    FitTolerance,
    ColumnCount,
    ColumnWidth,
    ColumnRuleWidth,
    ColumnRuleColor,
    ColumnRuleStyle,
    ColumnFill,
    ColumnSpan,
    ColumnHeight,
    ColumnWrap,
}

const ALL: &[Prop] = &[
    Prop::Opacity,
    Prop::Color,
    Prop::BackgroundColor,
    Prop::BorderTopColor,
    Prop::BorderRightColor,
    Prop::BorderBottomColor,
    Prop::BorderLeftColor,
    Prop::Width,
    Prop::Height,
    Prop::MinWidth,
    Prop::MaxWidth,
    Prop::MinHeight,
    Prop::MaxHeight,
    Prop::Top,
    Prop::Right,
    Prop::Bottom,
    Prop::Left,
    Prop::MarginTop,
    Prop::MarginRight,
    Prop::MarginBottom,
    Prop::MarginLeft,
    Prop::PaddingTop,
    Prop::PaddingRight,
    Prop::PaddingBottom,
    Prop::PaddingLeft,
    Prop::Transform,
    Prop::Translate,
    Prop::Rotate,
    Prop::Scale,
    Prop::Visibility,
    Prop::LetterSpacing,
    Prop::FontSize,
    Prop::BorderRadius,
    Prop::Filter,
    Prop::BackdropFilter,
    Prop::ClipPath,
    Prop::ShapeOutside,
    Prop::OffsetDistance,
    Prop::OffsetRotate,
    Prop::OffsetPosition,
    Prop::OffsetAnchor,
    Prop::OffsetPath,
    Prop::ShapeMargin,
    Prop::AlignContent,
    Prop::JustifyContent,
    Prop::AlignSelf,
    Prop::JustifySelf,
    Prop::AlignItems,
    Prop::JustifyItems,
    Prop::AnchorName,
    Prop::PositionAnchor,
    Prop::PositionArea,
    Prop::AnchorScope,
    Prop::GridTemplateColumns,
    Prop::GridTemplateRows,
    Prop::GridTemplateAreas,
    Prop::GridAutoColumns,
    Prop::GridAutoRows,
    Prop::GridAutoFlow,
    Prop::GridRowStart,
    Prop::GridRowEnd,
    Prop::GridColumnStart,
    Prop::GridColumnEnd,
    Prop::FitTolerance,
    Prop::ColumnCount,
    Prop::ColumnWidth,
    Prop::ColumnRuleWidth,
    Prop::ColumnRuleColor,
    Prop::ColumnRuleStyle,
    Prop::ColumnFill,
    Prop::ColumnSpan,
    Prop::ColumnHeight,
    Prop::ColumnWrap,
];

impl Prop {
    fn name(self) -> &'static str {
        match self {
            Prop::Opacity => "opacity",
            Prop::Color => "color",
            Prop::BackgroundColor => "background-color",
            Prop::BorderTopColor => "border-top-color",
            Prop::BorderRightColor => "border-right-color",
            Prop::BorderBottomColor => "border-bottom-color",
            Prop::BorderLeftColor => "border-left-color",
            Prop::Width => "width",
            Prop::Height => "height",
            Prop::MinWidth => "min-width",
            Prop::MaxWidth => "max-width",
            Prop::MinHeight => "min-height",
            Prop::MaxHeight => "max-height",
            Prop::Top => "top",
            Prop::Right => "right",
            Prop::Bottom => "bottom",
            Prop::Left => "left",
            Prop::MarginTop => "margin-top",
            Prop::MarginRight => "margin-right",
            Prop::MarginBottom => "margin-bottom",
            Prop::MarginLeft => "margin-left",
            Prop::PaddingTop => "padding-top",
            Prop::PaddingRight => "padding-right",
            Prop::PaddingBottom => "padding-bottom",
            Prop::PaddingLeft => "padding-left",
            Prop::Transform => "transform",
            Prop::Translate => "translate",
            Prop::Rotate => "rotate",
            Prop::Scale => "scale",
            Prop::Visibility => "visibility",
            Prop::LetterSpacing => "letter-spacing",
            Prop::FontSize => "font-size",
            Prop::BorderRadius => "border-radius",
            Prop::Filter => "filter",
            Prop::BackdropFilter => "backdrop-filter",
            Prop::ClipPath => "clip-path",
            Prop::ShapeOutside => "shape-outside",
            Prop::OffsetDistance => "offset-distance",
            Prop::OffsetRotate => "offset-rotate",
            Prop::OffsetPosition => "offset-position",
            Prop::OffsetAnchor => "offset-anchor",
            Prop::OffsetPath => "offset-path",
            Prop::ShapeMargin => "shape-margin",
            Prop::AlignContent => "align-content",
            Prop::JustifyContent => "justify-content",
            Prop::AlignSelf => "align-self",
            Prop::JustifySelf => "justify-self",
            Prop::AlignItems => "align-items",
            Prop::JustifyItems => "justify-items",
            Prop::AnchorName => "anchor-name",
            Prop::PositionAnchor => "position-anchor",
            Prop::PositionArea => "position-area",
            Prop::AnchorScope => "anchor-scope",
            Prop::GridTemplateRows => "grid-template-rows",
            Prop::GridTemplateColumns => "grid-template-columns",
            Prop::GridTemplateAreas => "grid-template-areas",
            Prop::GridAutoColumns => "grid-auto-columns",
            Prop::GridAutoRows => "grid-auto-rows",
            Prop::GridAutoFlow => "grid-auto-flow",
            Prop::GridRowStart => "grid-row-start",
            Prop::GridRowEnd => "grid-row-end",
            Prop::GridColumnStart => "grid-column-start",
            Prop::GridColumnEnd => "grid-column-end",
            Prop::FitTolerance => "fit-tolerance",
            Prop::ColumnCount => "column-count",
            Prop::ColumnWidth => "column-width",
            Prop::ColumnRuleWidth => "column-rule-width",
            Prop::ColumnRuleColor => "column-rule-color",
            Prop::ColumnRuleStyle => "column-rule-style",
            Prop::ColumnFill => "column-fill",
            Prop::ColumnSpan => "column-span",
            Prop::ColumnHeight => "column-height",
            Prop::ColumnWrap => "column-wrap",
        }
    }

    /// Does the `transition-property` value `name` cover this property?
    fn covered_by(self, name: &str) -> bool {
        let name = name.trim();
        if name.eq_ignore_ascii_case("all") || name.eq_ignore_ascii_case(self.name()) {
            return true;
        }
        let n = name.to_ascii_lowercase();
        let n = n.strip_prefix("-webkit-").unwrap_or(&n);
        if n == self.name() {
            return true;
        }
        use Prop::*;
        match n {
            "offset" => matches!(self, OffsetPath | OffsetDistance | OffsetPosition | OffsetRotate | OffsetAnchor),
            "grid-template" => matches!(self, GridTemplateRows | GridTemplateColumns | GridTemplateAreas),
            "grid" => matches!(self, GridTemplateRows | GridTemplateColumns | GridTemplateAreas | GridAutoColumns | GridAutoRows | GridAutoFlow),
            "grid-area" => matches!(self, GridRowStart | GridRowEnd | GridColumnStart | GridColumnEnd),
            "grid-row" => matches!(self, GridRowStart | GridRowEnd),
            "grid-column" => matches!(self, GridColumnStart | GridColumnEnd),
            "place-content" => matches!(self, AlignContent | JustifyContent),
            "place-items" => matches!(self, AlignItems | JustifyItems),
            "place-self" => matches!(self, AlignSelf | JustifySelf),
            "margin" => matches!(self, MarginTop | MarginRight | MarginBottom | MarginLeft),
            "padding" => matches!(self, PaddingTop | PaddingRight | PaddingBottom | PaddingLeft),
            "border-color" | "border" => matches!(self, BorderTopColor | BorderRightColor | BorderBottomColor | BorderLeftColor),
            "border-top" => self == BorderTopColor,
            "border-right" => self == BorderRightColor,
            "border-bottom" => self == BorderBottomColor,
            "border-left" => self == BorderLeftColor,
            "inset" => matches!(self, Top | Right | Bottom | Left),
            "background" => self == BackgroundColor,
            "font" => self == FontSize,
            "columns" => matches!(self, ColumnCount | ColumnWidth | ColumnHeight | ColumnWrap),
            "column-rule" => matches!(self, ColumnRuleWidth | ColumnRuleColor | ColumnRuleStyle),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Val {
    MotionPosition(MotionPosition),
    MotionRotation(MotionRotation),
    AnchorName(Option<Rc<[String]>>),
    PositionAnchor(Option<Rc<str>>),
    PositionArea(rabbithole_style::props::anchor::PositionArea),
    AnchorScope(rabbithole_style::props::anchor::AnchorScope),
    Tracks(Vec<TrackListItem>),
    /// Values that only animate discretely.
    AutoTracks(Vec<rabbithole_style::TrackSize>),
    AutoFlow(rabbithole_style::GridAutoFlow),
    Line(rabbithole_style::GridLine),
    Areas(Vec<Vec<String>>),
    /// `fit-tolerance`: `normal` (None) or a length-percentage.
    Tolerance(Option<Length>),
    Distribution(ContentDistribution, AlignFlags),
    ItemAlignment(ItemAlign, AlignFlags),
    F(f32),
    C(Color),
    /// A color that is not a legacy sRGB color (CSS Color 4 §12.1).
    Cw(rabbithole_style::color_model::Abs),
    L(Length),
    T(Vec<TransformOp>),
    V(Visibility),
    Fl(Vec<FilterOp>),
    Cp(Option<std::sync::Arc<ClipPath>>, bool),
    /// `column-count` (`None` = `auto`, discrete).
    Count(Option<u32>),
    /// `column-width` (`None` = `auto`, discrete).
    OptPx(Option<f32>),
    /// Values that only animate discretely (keyword indices).
    Keyword(u8),
}

fn get(s: &ComputedStyle, p: Prop) -> Val {
    use Prop::*;
    match p {
        Opacity => Val::F(s.opacity),
        Color => s.color_components.as_ref().filter(|c| !c.0.legacy).map_or(Val::C(s.color), |c| Val::Cw(c.0)),
        BackgroundColor => s.background_color_components.as_ref().filter(|c| !c.0.legacy).map_or(Val::C(s.background_color), |c| Val::Cw(c.0)),
        BorderTopColor => Val::C(s.border_color.top),
        BorderRightColor => Val::C(s.border_color.right),
        BorderBottomColor => Val::C(s.border_color.bottom),
        BorderLeftColor => Val::C(s.border_color.left),
        Width => Val::L(s.width),
        Height => Val::L(s.height),
        MinWidth => Val::L(s.min_width),
        MaxWidth => Val::L(s.max_width),
        MinHeight => Val::L(s.min_height),
        MaxHeight => Val::L(s.max_height),
        Top => Val::L(s.inset.top),
        Right => Val::L(s.inset.right),
        Bottom => Val::L(s.inset.bottom),
        Left => Val::L(s.inset.left),
        MarginTop => Val::L(s.margin.top),
        MarginRight => Val::L(s.margin.right),
        MarginBottom => Val::L(s.margin.bottom),
        MarginLeft => Val::L(s.margin.left),
        PaddingTop => Val::L(s.padding.top),
        PaddingRight => Val::L(s.padding.right),
        PaddingBottom => Val::L(s.padding.bottom),
        PaddingLeft => Val::L(s.padding.left),
        Transform => Val::T(s.transform.clone()),
        Translate => Val::T(s.transform3d.as_ref().and_then(|t| t.translate.clone()).into_iter().collect()),
        Rotate => Val::T(s.transform3d.as_ref().and_then(|t| t.rotate.clone()).into_iter().collect()),
        Scale => Val::T(s.transform3d.as_ref().and_then(|t| t.scale.clone()).into_iter().collect()),
        Visibility => Val::V(s.visibility),
        LetterSpacing => Val::F(s.letter_spacing),
        FontSize => Val::F(s.font_size),
        BorderRadius => Val::F(s.border_radius),
        Filter => Val::Fl(s.filters.clone()),
        BackdropFilter => Val::Fl(s.backdrop_filters.clone()),
        ClipPath => Val::Cp(s.clip_path.clone(), false),
        ShapeOutside => Val::Cp(s.shape_outside.clone(), s.shape_outside_position_explicit),
        ShapeMargin => Val::L(s.shape_margin),
        OffsetDistance => Val::L(s.motion().distance),
        OffsetRotate => Val::MotionRotation(s.motion().rotate),
        OffsetPosition => Val::MotionPosition(s.motion().position),
        OffsetAnchor => Val::MotionPosition(s.motion().anchor),
        OffsetPath => Val::Cp(s.motion().path.clone(), s.motion().explicit_center),
        AlignContent => Val::Distribution(s.align_content, s.align_content_flags),
        JustifyContent => Val::Distribution(s.justify_content, s.justify_content_flags),
        AlignSelf => Val::ItemAlignment(s.align_self, s.align_self_flags),
        JustifySelf => Val::ItemAlignment(s.justify_self, s.justify_self_flags),
        AlignItems => Val::ItemAlignment(s.align_items, s.align_items_flags),
        AnchorName => Val::AnchorName(s.anchor_names.clone()),
        PositionAnchor => Val::PositionAnchor(s.position_anchor.clone()),
        PositionArea => Val::PositionArea(s.position_area),
        AnchorScope => Val::AnchorScope(s.anchor_scope.clone()),
        GridTemplateRows => Val::Tracks(s.grid_template_rows.clone()),
        GridTemplateColumns => Val::Tracks(s.grid_template_columns.clone()),
        GridTemplateAreas => Val::Areas(s.grid_template_areas.clone()),
        GridAutoColumns => Val::AutoTracks(s.grid_auto_columns.clone()),
        GridAutoRows => Val::AutoTracks(s.grid_auto_rows.clone()),
        GridAutoFlow => Val::AutoFlow(s.grid_auto_flow),
        GridRowStart => Val::Line(s.grid_row_start.clone()),
        GridRowEnd => Val::Line(s.grid_row_end.clone()),
        GridColumnStart => Val::Line(s.grid_column_start.clone()),
        GridColumnEnd => Val::Line(s.grid_column_end.clone()),
        FitTolerance => Val::Tolerance(s.fit_tolerance),
        ColumnCount => Val::Count(s.column_count),
        ColumnWidth => Val::OptPx(s.column_width),
        ColumnRuleWidth => Val::F(s.column_rule_width),
        ColumnRuleColor => Val::C(s.column_rule_color.unwrap_or(s.color)),
        ColumnRuleStyle => Val::Keyword(s.column_rule_style_kw),
        ColumnFill => Val::Keyword(s.column_fill_auto as u8),
        ColumnSpan => Val::Keyword(s.column_span_all as u8),
        ColumnHeight => Val::OptPx(s.column_height),
        ColumnWrap => Val::Keyword(s.column_wrap),
        JustifyItems => Val::ItemAlignment(s.justify_items, s.justify_items_flags),
    }
}

fn set(s: &mut ComputedStyle, p: Prop, v: &Val) {
    use Prop::*;
    match (p, v) {
        (AnchorName, Val::AnchorName(value)) => s.anchor_names.clone_from(value),
        (PositionAnchor, Val::PositionAnchor(value)) => s.position_anchor.clone_from(value),
        (PositionArea, Val::PositionArea(value)) => s.position_area.clone_from(value),
        (AnchorScope, Val::AnchorScope(value)) => s.anchor_scope.clone_from(value),
        (GridTemplateRows, Val::Tracks(tracks)) => s.grid_template_rows.clone_from(tracks),
        (GridTemplateColumns, Val::Tracks(tracks)) => s.grid_template_columns.clone_from(tracks),
        (GridTemplateAreas, Val::Areas(a)) => s.grid_template_areas.clone_from(a),
        (GridAutoColumns, Val::AutoTracks(t)) => s.grid_auto_columns.clone_from(t),
        (GridAutoRows, Val::AutoTracks(t)) => s.grid_auto_rows.clone_from(t),
        (GridAutoFlow, Val::AutoFlow(f)) => s.grid_auto_flow = *f,
        (GridRowStart, Val::Line(l)) => s.grid_row_start.clone_from(l),
        (GridRowEnd, Val::Line(l)) => s.grid_row_end.clone_from(l),
        (GridColumnStart, Val::Line(l)) => s.grid_column_start.clone_from(l),
        (GridColumnEnd, Val::Line(l)) => s.grid_column_end.clone_from(l),
        (FitTolerance, Val::Tolerance(l)) => s.fit_tolerance = *l,
        (ColumnCount, Val::Count(n)) => {
            s.column_count = *n;
            s.multicol = s.column_count.is_some() || s.column_width.is_some();
        }
        (ColumnWidth, Val::OptPx(w)) => {
            s.column_width = w.map(|w| w.max(0.0));
            s.multicol = s.column_count.is_some() || s.column_width.is_some();
        }
        (ColumnRuleWidth, Val::F(w)) => s.column_rule_width = w.max(0.0),
        (ColumnRuleColor, Val::C(c)) => s.column_rule_color = Some(*c),
        (ColumnRuleColor, Val::Cw(c)) => s.column_rule_color = Some(c.to_gfx()),
        (ColumnRuleStyle, Val::Keyword(k)) => {
            s.column_rule_style_kw = *k;
            s.column_rule_style = match rabbithole_style::props::multicol::rule_style_css(*k) {
                "none" => rabbithole_style::BorderStyle::None,
                "hidden" => rabbithole_style::BorderStyle::Hidden,
                "dotted" => rabbithole_style::BorderStyle::Dotted,
                "dashed" => rabbithole_style::BorderStyle::Dashed,
                "double" => rabbithole_style::BorderStyle::Double,
                _ => rabbithole_style::BorderStyle::Solid,
            };
        }
        (ColumnFill, Val::Keyword(k)) => s.column_fill_auto = *k != 0,
        (ColumnSpan, Val::Keyword(k)) => s.column_span_all = *k != 0,
        (ColumnHeight, Val::OptPx(h)) => s.column_height = h.map(|h| h.max(0.0)),
        (ColumnWrap, Val::Keyword(k)) => s.column_wrap = *k,
        (AlignContent, Val::Distribution(value, flags)) => (s.align_content, s.align_content_flags) = (*value, *flags),
        (JustifyContent, Val::Distribution(value, flags)) => (s.justify_content, s.justify_content_flags) = (*value, *flags),
        (AlignSelf, Val::ItemAlignment(value, flags)) => (s.align_self, s.align_self_flags) = (*value, *flags),
        (JustifySelf, Val::ItemAlignment(value, flags)) => (s.justify_self, s.justify_self_flags) = (*value, *flags),
        (AlignItems, Val::ItemAlignment(value, flags)) => (s.align_items, s.align_items_flags) = (*value, *flags),
        (JustifyItems, Val::ItemAlignment(value, flags)) => (s.justify_items, s.justify_items_flags) = (*value, *flags),
        (Opacity, Val::F(f)) => s.opacity = f.clamp(0.0, 1.0),
        (Color, Val::C(c)) => {
            s.color = *c;
            s.color_components = None;
        }
        (BackgroundColor, Val::C(c)) => {
            s.background_color = *c;
            s.background_color_components = None;
        }
        (Color, Val::Cw(c)) => {
            s.color = c.to_gfx();
            s.color_components = Some(std::rc::Rc::new(rabbithole_style::ComputedColor(*c)));
        }
        (BackgroundColor, Val::Cw(c)) => {
            s.background_color = c.to_gfx();
            s.background_color_components = Some(std::rc::Rc::new(rabbithole_style::ComputedColor(*c)));
        }
        (BorderTopColor, Val::Cw(c)) => s.border_color.top = c.to_gfx(),
        (BorderRightColor, Val::Cw(c)) => s.border_color.right = c.to_gfx(),
        (BorderBottomColor, Val::Cw(c)) => s.border_color.bottom = c.to_gfx(),
        (BorderLeftColor, Val::Cw(c)) => s.border_color.left = c.to_gfx(),
        (BorderTopColor, Val::C(c)) => s.border_color.top = *c,
        (BorderRightColor, Val::C(c)) => s.border_color.right = *c,
        (BorderBottomColor, Val::C(c)) => s.border_color.bottom = *c,
        (BorderLeftColor, Val::C(c)) => s.border_color.left = *c,
        (Width, Val::L(l)) => s.width = *l,
        (Height, Val::L(l)) => s.height = *l,
        (MinWidth, Val::L(l)) => s.min_width = *l,
        (MaxWidth, Val::L(l)) => s.max_width = *l,
        (MinHeight, Val::L(l)) => s.min_height = *l,
        (MaxHeight, Val::L(l)) => s.max_height = *l,
        (Top, Val::L(l)) => s.inset.top = *l,
        (Right, Val::L(l)) => s.inset.right = *l,
        (Bottom, Val::L(l)) => s.inset.bottom = *l,
        (Left, Val::L(l)) => s.inset.left = *l,
        (MarginTop, Val::L(l)) => s.margin.top = *l,
        (MarginRight, Val::L(l)) => s.margin.right = *l,
        (MarginBottom, Val::L(l)) => s.margin.bottom = *l,
        (MarginLeft, Val::L(l)) => s.margin.left = *l,
        (PaddingTop, Val::L(l)) => s.padding.top = *l,
        (PaddingRight, Val::L(l)) => s.padding.right = *l,
        (PaddingBottom, Val::L(l)) => s.padding.bottom = *l,
        (PaddingLeft, Val::L(l)) => s.padding.left = *l,
        (Transform, Val::T(t)) => s.transform = t.clone(),
        (Translate | Rotate | Scale, Val::T(t)) => {
            let v = t.first().cloned();
            if v.is_some() || s.transform3d.is_some() {
                let t3 = s.transform3d_mut();
                match p {
                    Translate => t3.translate = v,
                    Rotate => t3.rotate = v,
                    _ => t3.scale = v,
                }
            }
        }
        (Visibility, Val::V(v)) => s.visibility = *v,
        (LetterSpacing, Val::F(f)) => s.letter_spacing = *f,
        (FontSize, Val::F(f)) => {
            s.font_size = f.max(0.0);
            s.resolve_spacing_pct();
        }
        (BorderRadius, Val::F(f)) => s.border_radius = f.max(0.0),
        (Filter, Val::Fl(f)) => {
            s.filters = f.clone();
            if !f.is_empty() {
                s.creates_stacking_context = true;
            }
        }
        (OffsetDistance, Val::L(v)) => s.motion_mut().distance = *v,
        (OffsetPosition, Val::MotionPosition(v)) => s.motion_mut().position = *v,
        (OffsetAnchor, Val::MotionPosition(v)) => s.motion_mut().anchor = *v,
        (OffsetRotate, Val::MotionRotation(v)) => s.motion_mut().rotate = *v,
        (OffsetPath, Val::Cp(path, explicit)) => {
            let m = s.motion_mut();
            m.path.clone_from(path);
            m.explicit_center = *explicit;
        }
        (ShapeOutside, Val::Cp(c, explicit)) => {
            s.shape_outside = c.clone();
            s.shape_outside_position_explicit = *explicit;
        }
        (ShapeMargin, Val::L(l)) => s.shape_margin = match *l {
            Length::Px(px) => Length::Px(px.max(0.0)),
            Length::Percent(pct) => Length::Percent(pct.max(0.0)),
            other => other,
        },
        (ClipPath, Val::Cp(c, _)) => {
            s.clip_path = c.clone();
            if c.is_some() {
                s.creates_stacking_context = true;
            }
        }
        (BackdropFilter, Val::Fl(f)) => {
            s.backdrop_filters = f.clone();
            s.backdrop = !f.is_empty();
            if !f.is_empty() {
                s.creates_stacking_context = true;
            }
        }
        _ => {}
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_len(a: &Length, b: &Length, t: f32) -> Option<Length> {
    match (a, b) {
        (Length::Px(x), Length::Px(y)) => Some(Length::Px(lerp(*x, *y, t))),
        (Length::Percent(x), Length::Percent(y)) => Some(Length::Percent(lerp(*x, *y, t))),
        _ => None,
    }
}

/// A length-percentage as (px, percent), for interpolating mixed units.
fn len_parts(l: &Length) -> Option<(f32, f32)> {
    match *l {
        Length::Px(v) => Some((v, 0.0)),
        Length::Percent(p) => Some((0.0, p)),
        Length::Calc(v, p) => Some((v, p)),
        _ => None,
    }
}

/// Interpolate two length-percentages (mixed units give `calc()`).
fn lerp_lp(a: &Length, b: &Length, t: f32) -> Option<Length> {
    if let Some(l) = lerp_len(a, b, t) {
        return Some(l);
    }
    let ((ax, ap), (bx, bp)) = (len_parts(a)?, len_parts(b)?);
    Some(Length::Calc(lerp(ax, bx, t), lerp(ap, bp, t)))
}

fn lerp_pair(a: &(Length, Length), b: &(Length, Length), t: f32) -> Option<(Length, Length)> {
    Some((lerp_lp(&a.0, &b.0, t)?, lerp_lp(&a.1, &b.1, t)?))
}

fn lerp_radii(a: &[(Length, Length); 4], b: &[(Length, Length); 4], t: f32) -> Option<[(Length, Length); 4]> {
    Some([lerp_pair(&a[0], &b[0], t)?, lerp_pair(&a[1], &b[1], t)?, lerp_pair(&a[2], &b[2], t)?, lerp_pair(&a[3], &b[3], t)?])
}

fn lerp_radius(a: &ShapeRadius, b: &ShapeRadius, t: f32) -> Option<ShapeRadius> {
    match (a, b) {
        (ShapeRadius::Len(x), ShapeRadius::Len(y)) => Some(ShapeRadius::Len(lerp_lp(x, y, t)?)),
        _ if a == b => Some(*a),
        _ => None,
    }
}

fn lerp_point(a: rabbithole_gfx::Point, b: rabbithole_gfx::Point, t: f32) -> rabbithole_gfx::Point {
    rabbithole_gfx::Point::new(lerp(a.x, b.x, t), lerp(a.y, b.y, t))
}

fn lerp_seg(a: &rabbithole_gfx::PathSeg, b: &rabbithole_gfx::PathSeg, t: f32) -> Option<rabbithole_gfx::PathSeg> {
    use rabbithole_gfx::PathSeg::*;
    Some(match (a, b) {
        (MoveTo(p), MoveTo(q)) => MoveTo(lerp_point(*p, *q, t)),
        (LineTo(p), LineTo(q)) => LineTo(lerp_point(*p, *q, t)),
        (QuadTo(p1, p), QuadTo(q1, q)) => QuadTo(lerp_point(*p1, *q1, t), lerp_point(*p, *q, t)),
        (CubicTo(p1, p2, p), CubicTo(q1, q2, q)) => CubicTo(lerp_point(*p1, *q1, t), lerp_point(*p2, *q2, t), lerp_point(*p, *q, t)),
        (ArcTo { rx: arx, ry: ary, rotation: ar, large_arc: al, sweep: asw, to: at }, ArcTo { rx: brx, ry: bry, rotation: br, large_arc: bl, sweep: bsw, to: bt }) => {
            // Flags are discrete (SVG paths interpolate them as booleans).
            let pick = |x: bool, y: bool| if t < 0.5 { x } else { y };
            ArcTo { rx: lerp(*arx, *brx, t), ry: lerp(*ary, *bry, t), rotation: lerp(*ar, *br, t), large_arc: pick(*al, *bl), sweep: pick(*asw, *bsw), to: lerp_point(*at, *bt, t) }
        }
        (Close, Close) => Close,
        _ => return None,
    })
}

fn lerp_control(a: &ControlPoint, b: &ControlPoint, t: f32) -> Option<ControlPoint> {
    (a.anchor == b.anchor).then_some(())?;
    Some(ControlPoint { x: lerp_lp(&a.x, &b.x, t)?, y: lerp_lp(&a.y, &b.y, t)?, anchor: a.anchor })
}

fn lerp_command(a: &ShapeCommand, b: &ShapeCommand, t: f32) -> Option<ShapeCommand> {
    use ShapeCommand::*;
    let l = |x: &Length, y: &Length| lerp_lp(x, y, t);
    Some(match (a, b) {
        (Move { by: ab, x: ax, y: ay }, Move { by: bb, x: bx, y: by }) if ab == bb => Move { by: *ab, x: l(ax, bx)?, y: l(ay, by)? },
        (Line { by: ab, x: ax, y: ay }, Line { by: bb, x: bx, y: by }) if ab == bb => Line { by: *ab, x: l(ax, bx)?, y: l(ay, by)? },
        (HLine { by: ab, x: ax }, HLine { by: bb, x: bx }) if ab == bb => HLine { by: *ab, x: l(ax, bx)? },
        (VLine { by: ab, y: ay }, VLine { by: bb, y: by }) if ab == bb => VLine { by: *ab, y: l(ay, by)? },
        (Curve { by: ab, x: ax, y: ay, c1: a1, c2: a2 }, Curve { by: bb, x: bx, y: by, c1: b1, c2: b2 }) if ab == bb => {
            let c2 = match (a2, b2) {
                (Some(p), Some(q)) => Some(lerp_control(p, q, t)?),
                (None, None) => None,
                _ => return None,
            };
            Curve { by: *ab, x: l(ax, bx)?, y: l(ay, by)?, c1: lerp_control(a1, b1, t)?, c2 }
        }
        (Smooth { by: ab, x: ax, y: ay, c: ac }, Smooth { by: bb, x: bx, y: by, c: bc }) if ab == bb => {
            let c = match (ac, bc) {
                (Some(p), Some(q)) => Some(lerp_control(p, q, t)?),
                (None, None) => None,
                _ => return None,
            };
            Smooth { by: *ab, x: l(ax, bx)?, y: l(ay, by)?, c }
        }
        (Arc { by: ab, x: ax, y: ay, rx: arx, ry: ary, cw: acw, large: al, rotation: ar }, Arc { by: bb, x: bx, y: by, rx: brx, ry: bry, cw: bcw, large: bl, rotation: br }) if ab == bb => {
            let ry = match (ary, bry) {
                (Some(p), Some(q)) => Some(l(p, q)?),
                (None, None) => None,
                _ => return None,
            };
            let pick = |x: bool, y: bool| if t < 0.5 { x } else { y };
            Arc { by: *ab, x: l(ax, bx)?, y: l(ay, by)?, rx: l(arx, brx)?, ry, cw: pick(*acw, *bcw), large: pick(*al, *bl), rotation: lerp(*ar, *br, t) }
        }
        (Close, Close) => Close,
        _ => return None,
    })
}

/// Interpolate two shapes of the same kind (CSS Shapes 1 §4); `None`
/// when they do not interpolate (the animation is then discrete).
fn lerp_shape(a: &BasicShape, b: &BasicShape, t: f32) -> Option<BasicShape> {
    use BasicShape::*;
    let l = |x: &Length, y: &Length| lerp_lp(x, y, t);
    Some(match (a, b) {
        (Inset { insets: ai, radii: ar }, Inset { insets: bi, radii: br }) => {
            Inset { insets: [l(&ai[0], &bi[0])?, l(&ai[1], &bi[1])?, l(&ai[2], &bi[2])?, l(&ai[3], &bi[3])?], radii: lerp_radii(ar, br, t)? }
        }
        (Xywh { x: ax, y: ay, w: aw, h: ah, radii: ar }, Xywh { x: bx, y: by, w: bw, h: bh, radii: br }) => {
            Xywh { x: l(ax, bx)?, y: l(ay, by)?, w: l(aw, bw)?, h: l(ah, bh)?, radii: lerp_radii(ar, br, t)? }
        }
        (Rect { edges: ae, radii: ar }, Rect { edges: be, radii: br }) => {
            let e = |i: usize| if ae[i].is_auto() && be[i].is_auto() { Some(Length::Auto) } else { l(&ae[i], &be[i]) };
            Rect { edges: [e(0)?, e(1)?, e(2)?, e(3)?], radii: lerp_radii(ar, br, t)? }
        }
        (Circle { r: ar, at: aa }, Circle { r: br, at: ba }) => Circle { r: lerp_radius(ar, br, t)?, at: lerp_pair(aa, ba, t)? },
        (Ellipse { rx: ax, ry: ay, at: aa }, Ellipse { rx: bx, ry: by, at: ba }) => {
            Ellipse { rx: lerp_radius(ax, bx, t)?, ry: lerp_radius(ay, by, t)?, at: lerp_pair(aa, ba, t)? }
        }
        (Polygon { evenodd: ae, points: ap, round: ar }, Polygon { evenodd: be, points: bp, round: br }) if ae == be && ap.len() == bp.len() => {
            let points: Option<Vec<(Length, Length)>> = ap.iter().zip(bp).map(|(p, q)| lerp_pair(p, q, t)).collect();
            Polygon { evenodd: *ae, points: points?, round: lerp(*ar, *br, t).max(0.0) }
        }
        (Path { evenodd: ae, path: ap }, Path { evenodd: be, path: bp }) if ae == be && ap.len() == bp.len() => {
            let path: Option<Vec<rabbithole_gfx::PathSeg>> = ap.iter().zip(bp).map(|(p, q)| lerp_seg(p, q, t)).collect();
            Path { evenodd: *ae, path: path? }
        }
        (Shape { evenodd: ae, from: af, cmds: ac }, Shape { evenodd: be, from: bf, cmds: bc }) if ae == be && ac.len() == bc.len() => {
            let direct: Option<Vec<ShapeCommand>> = ac.iter().zip(bc).map(|(p, q)| lerp_command(p, q, t)).collect();
            let cmds = match direct {
                Some(c) => c,
                None => {
                    // CSS Shapes 2 §4.2.1: mismatched `by` / `to` (and
                    // `hline` / `vline` against `line`) interpolate in
                    // absolute form.
                    let (x, y) = (absolute_commands(*af, ac)?, absolute_commands(*bf, bc)?);
                    let (x, y) = unify_lines(x, *af, y, *bf)?;
                    x.iter().zip(&y).map(|(p, q)| lerp_command(p, q, t)).collect::<Option<Vec<_>>>()?
                }
            };
            Shape { evenodd: *ae, from: lerp_pair(af, bf, t)?, cmds }
        }
        // `path()` interpolates with `shape()` as its equivalent shape.
        (Path { .. }, Shape { .. }) => return lerp_shape(&path_as_shape(a)?, b, t),
        (Shape { .. }, Path { .. }) => return lerp_shape(a, &path_as_shape(b)?, t),
        _ => return None,
    })
}

fn add_lp(a: Length, b: Length) -> Option<Length> {
    let ((ax, ap), (bx, bp)) = (len_parts(&a)?, len_parts(&b)?);
    Some(if ap + bp == 0.0 { Length::Px(ax + bx) } else { Length::Calc(ax + bx, ap + bp) })
}

/// `shape()` commands with every end point made absolute (`to`).
fn absolute_commands(from: (Length, Length), cmds: &[ShapeCommand]) -> Option<Vec<ShapeCommand>> {
    use ShapeCommand::*;
    let (mut cx, mut cy) = from;
    let (mut sx, mut sy) = from;
    let mut out = Vec::with_capacity(cmds.len());
    for c in cmds {
        let end = |by: bool, x: Length, y: Length| -> Option<(Length, Length)> { if by { Some((add_lp(cx, x)?, add_lp(cy, y)?)) } else { Some((x, y)) } };
        let cmd = match c {
            Move { by, x, y } => {
                (cx, cy) = end(*by, *x, *y)?;
                (sx, sy) = (cx, cy);
                Move { by: false, x: cx, y: cy }
            }
            Line { by, x, y } => {
                (cx, cy) = end(*by, *x, *y)?;
                Line { by: false, x: cx, y: cy }
            }
            HLine { by, x } => {
                cx = if *by { add_lp(cx, *x)? } else { *x };
                HLine { by: false, x: cx }
            }
            VLine { by, y } => {
                cy = if *by { add_lp(cy, *y)? } else { *y };
                VLine { by: false, y: cy }
            }
            Arc { by, x, y, rx, ry, cw, large, rotation } => {
                (cx, cy) = end(*by, *x, *y)?;
                Arc { by: false, x: cx, y: cy, rx: *rx, ry: *ry, cw: *cw, large: *large, rotation: *rotation }
            }
            Close => {
                (cx, cy) = (sx, sy);
                Close
            }
            // Control points relative to the start of a `by` segment do
            // not survive the conversion: keep curves as they are.
            Curve { by: false, .. } | Smooth { by: false, .. } => c.clone(),
            _ => return None,
        };
        out.push(cmd);
    }
    Some(out)
}

/// Make `hline` / `vline` paired with `line` (absolute commands) a `line`.
fn unify_lines(mut a: Vec<ShapeCommand>, af: (Length, Length), mut b: Vec<ShapeCommand>, bf: (Length, Length)) -> Option<(Vec<ShapeCommand>, Vec<ShapeCommand>)> {
    use ShapeCommand::*;
    fn as_lines(cmds: &[ShapeCommand], from: (Length, Length)) -> Vec<Option<ShapeCommand>> {
        // The current point before each command.
        let mut cur: Option<(Length, Length)> = Some(from);
        let mut start = cur;
        let mut out = Vec::with_capacity(cmds.len());
        for c in cmds {
            let line = match (c, cur) {
                (HLine { x, .. }, Some((_, y))) => Some(Line { by: false, x: *x, y }),
                (VLine { y, .. }, Some((x, _))) => Some(Line { by: false, x, y: *y }),
                (Line { .. }, _) => Some(c.clone()),
                _ => None,
            };
            cur = match c {
                Move { x, y, .. } => {
                    start = Some((*x, *y));
                    start
                }
                Line { x, y, .. } | Arc { x, y, .. } | Curve { x, y, .. } | Smooth { x, y, .. } => Some((*x, *y)),
                HLine { x, .. } => cur.map(|(_, y)| (*x, y)),
                VLine { y, .. } => cur.map(|(x, _)| (x, *y)),
                Close => start,
            };
            out.push(line);
        }
        out
    }
    let (la, lb) = (as_lines(&a, af), as_lines(&b, bf));
    for i in 0..a.len() {
        if std::mem::discriminant(&a[i]) != std::mem::discriminant(&b[i]) {
            a[i] = la[i].clone()?;
            b[i] = lb[i].clone()?;
        }
    }
    Some((a, b))
}

/// The `shape()` equivalent of a `path()` (absolute px commands).
fn path_as_shape(p: &BasicShape) -> Option<BasicShape> {
    use rabbithole_gfx::PathSeg;
    use rabbithole_style::ControlAnchor;
    let BasicShape::Path { evenodd, path } = p else { return None };
    let px = |v: f32| Length::Px(v);
    let ctl = |q: rabbithole_gfx::Point| ControlPoint { x: px(q.x), y: px(q.y), anchor: ControlAnchor::Origin };
    let (first, rest) = path.split_first()?;
    let PathSeg::MoveTo(f) = first else { return None };
    let cmds = rest
        .iter()
        .map(|s| match *s {
            PathSeg::MoveTo(q) => ShapeCommand::Move { by: false, x: px(q.x), y: px(q.y) },
            PathSeg::LineTo(q) => ShapeCommand::Line { by: false, x: px(q.x), y: px(q.y) },
            PathSeg::QuadTo(c, q) => ShapeCommand::Curve { by: false, x: px(q.x), y: px(q.y), c1: ctl(c), c2: None },
            PathSeg::CubicTo(c1, c2, q) => ShapeCommand::Curve { by: false, x: px(q.x), y: px(q.y), c1: ctl(c1), c2: Some(ctl(c2)) },
            PathSeg::ArcTo { rx, ry, rotation, large_arc, sweep, to } => {
                ShapeCommand::Arc { by: false, x: px(to.x), y: px(to.y), rx: px(rx), ry: Some(px(ry)), cw: sweep, large: large_arc, rotation }
            }
            PathSeg::Close => ShapeCommand::Close,
        })
        .collect();
    Some(BasicShape::Shape { evenodd: *evenodd, from: (px(f.x), px(f.y)), cmds })
}

fn lerp_clip(a: &ClipPath, b: &ClipPath, t: f32) -> Option<ClipPath> {
    match (a, b) {
        (ClipPath::Shape(x, bx), ClipPath::Shape(y, by)) if bx == by => Some(ClipPath::Shape(lerp_shape(x, y, t)?, *bx)),
        _ => None,
    }
}

/// Identity transform function of the same kind as `op`.
fn identity_of(op: &TransformOp) -> TransformOp {
    match op {
        TransformOp::Translate(..) => TransformOp::Translate(Length::Px(0.0), Length::Px(0.0)),
        TransformOp::Scale(..) => TransformOp::Scale(1.0, 1.0),
        TransformOp::Rotate(_) => TransformOp::Rotate(0.0),
        TransformOp::Skew(..) => TransformOp::Skew(0.0, 0.0),
        TransformOp::SkewX(_) => TransformOp::SkewX(0.0),
        TransformOp::SkewY(_) => TransformOp::SkewY(0.0),
        TransformOp::Matrix(_) => TransformOp::Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
        TransformOp::Translate3d(..) => TransformOp::Translate3d(Length::Px(0.0), Length::Px(0.0), 0.0),
        TransformOp::Scale3d(..) => TransformOp::Scale3d(1.0, 1.0, 1.0),
        TransformOp::Rotate3d(x, y, z, _) => TransformOp::Rotate3d(*x, *y, *z, 0.0),
        TransformOp::Perspective(_) => TransformOp::Perspective(None),
        TransformOp::Matrix3d(_) => TransformOp::Matrix3d(Box::new(rabbithole_gfx::mat4::to_css(&rabbithole_gfx::mat4::IDENTITY4))),
    }
}

/// A rotation as (normalized axis, angle); a zero axis is about z.
fn rotation_of(op: &TransformOp) -> Option<([f64; 3], f64)> {
    let (x, y, z, a) = match op {
        TransformOp::Rotate(a) => (0.0, 0.0, 1.0, *a),
        TransformOp::Rotate3d(x, y, z, a) => (*x, *y, *z, *a),
        _ => return None,
    };
    let (x, y, z) = (x as f64, y as f64, z as f64);
    let len = (x * x + y * y + z * z).sqrt();
    if len == 0.0 {
        return Some(([0.0, 0.0, 1.0], a as f64));
    }
    Some(([x / len, y / len, z / len], a as f64))
}

/// Interpolate two rotations (CSS Transforms 2 §interpolation of
/// primitives): the angle when the axes agree, else the quaternions by
/// spherical linear interpolation.
fn lerp_rotation(a: &TransformOp, b: &TransformOp, t: f32) -> Option<TransformOp> {
    let (aa, ang_a) = rotation_of(a)?;
    let (ab, ang_b) = rotation_of(b)?;
    let same = (aa[0] - ab[0]).abs() < 1e-6 && (aa[1] - ab[1]).abs() < 1e-6 && (aa[2] - ab[2]).abs() < 1e-6;
    if same || ang_a == 0.0 || ang_b == 0.0 {
        let axis = if !same && ang_a == 0.0 { ab } else { aa };
        let ang = ang_a + (ang_b - ang_a) * t as f64;
        if let (TransformOp::Rotate(_), TransformOp::Rotate(_)) = (a, b) {
            return Some(TransformOp::Rotate(ang as f32));
        }
        return Some(TransformOp::Rotate3d(axis[0] as f32, axis[1] as f32, axis[2] as f32, ang as f32));
    }
    // The quaternions of the rotation matrices (whole turns vanish), as
    // matrix interpolation would find them.
    let q = |axis: [f64; 3], ang: f64| {
        let m = rabbithole_gfx::mat4::rotate(axis[0] as f32, axis[1] as f32, axis[2] as f32, ang as f32);
        rabbithole_gfx::mat4::decompose(&m).map_or([0.0, 0.0, 0.0, 1.0], |d| d.quaternion)
    };
    let r = rabbithole_gfx::mat4::slerp(q(aa, ang_a), q(ab, ang_b), t as f64);
    let w = r[3].clamp(-1.0, 1.0);
    let ang = 2.0 * w.acos();
    let s = (1.0 - w * w).sqrt();
    let axis = if s < 1e-9 { [0.0, 0.0, 1.0] } else { [r[0] / s, r[1] / s, r[2] / s] };
    Some(TransformOp::Rotate3d(axis[0] as f32, axis[1] as f32, axis[2] as f32, ang as f32))
}

fn lerp_op(a: &TransformOp, b: &TransformOp, t: f32) -> Option<TransformOp> {
    use TransformOp::*;
    Some(match (a, b) {
        (Translate(ax, ay), Translate(bx, by)) => Translate(lerp_lp(ax, bx, t)?, lerp_lp(ay, by, t)?),
        (Scale(ax, ay), Scale(bx, by)) => Scale(lerp(*ax, *bx, t), lerp(*ay, *by, t)),
        (Rotate(x), Rotate(y)) => Rotate(lerp(*x, *y, t)),
        (Skew(ax, ay), Skew(bx, by)) => Skew(lerp(*ax, *bx, t), lerp(*ay, *by, t)),
        (SkewX(x), SkewX(y)) => SkewX(lerp(*x, *y, t)),
        (SkewY(x), SkewY(y)) => SkewY(lerp(*x, *y, t)),
        (Matrix(m), Matrix(n)) => {
            let mut o = [0.0; 6];
            for i in 0..6 {
                o[i] = lerp(m[i], n[i], t);
            }
            Matrix(o)
        }
        (Translate(ax, ay) | Translate3d(ax, ay, _), Translate(bx, by) | Translate3d(bx, by, _)) => {
            let z = |o: &TransformOp| if let Translate3d(_, _, z) = o { *z } else { 0.0 };
            Translate3d(lerp_lp(ax, bx, t)?, lerp_lp(ay, by, t)?, lerp(z(a), z(b), t))
        }
        (Scale(ax, ay) | Scale3d(ax, ay, _), Scale(bx, by) | Scale3d(bx, by, _)) => {
            let z = |o: &TransformOp| if let Scale3d(_, _, z) = o { *z } else { 1.0 };
            Scale3d(lerp(*ax, *bx, t), lerp(*ay, *by, t), lerp(z(a), z(b), t))
        }
        (Rotate(_) | Rotate3d(..), Rotate(_) | Rotate3d(..)) => return lerp_rotation(a, b, t),
        (Perspective(x), Perspective(y)) => {
            // Interpolate the inverse distances (`none` is 0).
            let inv = |d: &Option<f32>| d.map_or(0.0, |d| 1.0 / d.max(1.0));
            let v = lerp(inv(x), inv(y), t);
            Perspective((v > 0.0).then(|| 1.0 / v))
        }
        _ => return None,
    })
}

/// The initial (identity) value of a filter function.
fn filter_identity(op: &FilterOp) -> FilterOp {
    match op {
        FilterOp::Blur(_) => FilterOp::Blur(0.0),
        FilterOp::Brightness(_) => FilterOp::Brightness(1.0),
        FilterOp::Contrast(_) => FilterOp::Contrast(1.0),
        FilterOp::Grayscale(_) => FilterOp::Grayscale(0.0),
        FilterOp::Sepia(_) => FilterOp::Sepia(0.0),
        FilterOp::Saturate(_) => FilterOp::Saturate(1.0),
        FilterOp::HueRotate(_) => FilterOp::HueRotate(0.0),
        FilterOp::Invert(_) => FilterOp::Invert(0.0),
        FilterOp::Opacity(_) => FilterOp::Opacity(1.0),
        FilterOp::DropShadow { color, current, .. } => FilterOp::DropShadow { x: 0.0, y: 0.0, blur: 0.0, color: Color { a: 0, ..*color }, current: *current },
        FilterOp::Url(id) => FilterOp::Url(id.clone()),
    }
}

fn lerp_filter(a: &FilterOp, b: &FilterOp, t: f32) -> Option<FilterOp> {
    use FilterOp::*;
    Some(match (a, b) {
        (Blur(x), Blur(y)) => Blur(lerp(*x, *y, t)),
        (Brightness(x), Brightness(y)) => Brightness(lerp(*x, *y, t)),
        (Contrast(x), Contrast(y)) => Contrast(lerp(*x, *y, t)),
        (Grayscale(x), Grayscale(y)) => Grayscale(lerp(*x, *y, t)),
        (Sepia(x), Sepia(y)) => Sepia(lerp(*x, *y, t)),
        (Saturate(x), Saturate(y)) => Saturate(lerp(*x, *y, t)),
        (HueRotate(x), HueRotate(y)) => HueRotate(lerp(*x, *y, t)),
        (Invert(x), Invert(y)) => Invert(lerp(*x, *y, t)),
        (Opacity(x), Opacity(y)) => Opacity(lerp(*x, *y, t)),
        (DropShadow { x: ax, y: ay, blur: ab, color: ac, current: cur_a }, DropShadow { x: bx, y: by, blur: bb, color: bc, current: cur_b }) => {
            let ch = |p: u8, q: u8| lerp(p as f32, q as f32, t).round().clamp(0.0, 255.0) as u8;
            DropShadow { x: lerp(*ax, *bx, t), y: lerp(*ay, *by, t), blur: lerp(*ab, *bb, t), color: Color { r: ch(ac.r, bc.r), g: ch(ac.g, bc.g), b: ch(ac.b, bc.b), a: ch(ac.a, bc.a) }, current: *cur_a || *cur_b }
        }
        _ => return None,
    })
}

/// Interpolate two transform lists (CSS Transforms 2 §interpolation):
/// lists of the same functions pairwise (the shorter padded with
/// identities), anything else as decomposed 2D matrices.
fn lerp_transforms(x: &[TransformOp], y: &[TransformOp], t: f32) -> Option<Vec<TransformOp>> {
    let n = x.len().max(y.len());
    let pad = |l: &[TransformOp], other: &[TransformOp]| -> Vec<TransformOp> {
        let mut v = l.to_vec();
        v.extend(other[l.len().min(other.len())..].iter().map(identity_of));
        v
    };
    let (px, py) = (pad(x, y), pad(y, x));
    if px.len() == n && py.len() == n {
        let ops: Option<Vec<TransformOp>> = px.iter().zip(py.iter()).map(|(p, q)| lerp_op(p, q, t)).collect();
        if let Some(ops) = ops {
            return Some(ops);
        }
    }
    if !x.iter().chain(y.iter()).all(TransformOp::is_2d) {
        let ma = matrix3d_of(x)?;
        let mb = matrix3d_of(y)?;
        let m = rabbithole_gfx::mat4::interpolate(&ma, &mb, t)?;
        if rabbithole_gfx::mat4::is_2d(&m) {
            return Some(vec![TransformOp::Matrix(rabbithole_gfx::mat4::flatten_2d(&m))]);
        }
        return Some(vec![TransformOp::Matrix3d(Box::new(rabbithole_gfx::mat4::to_css(&m)))]);
    }
    let ma = matrix_of(x)?;
    let mb = matrix_of(y)?;
    Some(vec![TransformOp::Matrix(lerp_matrix(ma, mb, t))])
}

/// The 3D matrix of a transform list (None when it needs the box size).
fn matrix3d_of(ops: &[TransformOp]) -> Option<rabbithole_gfx::mat4::Mat4> {
    if ops.iter().any(|o| matches!(o, TransformOp::Translate(x, y) | TransformOp::Translate3d(x, y, _) if !matches!(x, Length::Px(_)) || !matches!(y, Length::Px(_)))) {
        return None;
    }
    Some(rabbithole_style::transform_matrix_3d(ops, 0.0, 0.0))
}

/// The 2D matrix of a transform list (None when it needs the box size).
fn matrix_of(ops: &[TransformOp]) -> Option<[f32; 6]> {
    if ops.iter().any(|o| matches!(o, TransformOp::Translate(x, y) if !matches!(x, Length::Px(_)) || !matches!(y, Length::Px(_)))) {
        return None;
    }
    Some(rabbithole_style::transform_matrix(ops, 0.0, 0.0, 0.0))
}

/// Decomposed 2D matrix: translate, scale, angle (degrees), 2x2 rest.
struct Decomposed2d {
    translate: [f64; 2],
    scale: [f64; 2],
    angle: f64,
    m: [f64; 4],
}

fn decompose_2d(m: [f32; 6]) -> Decomposed2d {
    let (mut r0x, mut r0y, mut r1x, mut r1y) = (m[0] as f64, m[1] as f64, m[2] as f64, m[3] as f64);
    let mut scale = [(r0x * r0x + r0y * r0y).sqrt(), (r1x * r1x + r1y * r1y).sqrt()];
    if r0x * r1y - r0y * r1x < 0.0 {
        if r0x < r1y {
            scale[0] = -scale[0];
        } else {
            scale[1] = -scale[1];
        }
    }
    if scale[0] != 0.0 {
        r0x /= scale[0];
        r0y /= scale[0];
    }
    if scale[1] != 0.0 {
        r1x /= scale[1];
        r1y /= scale[1];
    }
    let angle = r0y.atan2(r0x);
    if angle != 0.0 {
        let (sn, cs) = (-r0y, r0x);
        let (m11, m12, m21, m22) = (r0x, r0y, r1x, r1y);
        r0x = cs * m11 + sn * m21;
        r0y = cs * m12 + sn * m22;
        r1x = -sn * m11 + cs * m21;
        r1y = -sn * m12 + cs * m22;
    }
    Decomposed2d { translate: [m[4] as f64, m[5] as f64], scale, angle: angle.to_degrees(), m: [r0x, r0y, r1x, r1y] }
}

fn lerp_matrix(a: [f32; 6], b: [f32; 6], t: f32) -> [f32; 6] {
    let (mut da, mut db) = (decompose_2d(a), decompose_2d(b));
    if (da.scale[0] < 0.0 && db.scale[1] < 0.0) || (da.scale[1] < 0.0 && db.scale[0] < 0.0) {
        da.scale = [-da.scale[0], -da.scale[1]];
        da.angle += if da.angle < 0.0 { 180.0 } else { -180.0 };
    }
    // Don't rotate the long way around.
    if da.angle == 0.0 {
        da.angle = 360.0;
    }
    if db.angle == 0.0 {
        db.angle = 360.0;
    }
    if (da.angle - db.angle).abs() > 180.0 {
        if da.angle > db.angle {
            da.angle -= 360.0;
        } else {
            db.angle -= 360.0;
        }
    }
    let t = t as f64;
    let l = |x: f64, y: f64| x + (y - x) * t;
    let tr = [l(da.translate[0], db.translate[0]), l(da.translate[1], db.translate[1])];
    let sc = [l(da.scale[0], db.scale[0]), l(da.scale[1], db.scale[1])];
    let angle = l(da.angle, db.angle).to_radians();
    let mm = [l(da.m[0], db.m[0]), l(da.m[1], db.m[1]), l(da.m[2], db.m[2]), l(da.m[3], db.m[3])];
    // Recompose: rotate × m, then scale the rows.
    let (c, s) = (angle.cos(), angle.sin());
    let r0x = c * mm[0] + s * mm[2];
    let r0y = c * mm[1] + s * mm[3];
    let r1x = -s * mm[0] + c * mm[2];
    let r1y = -s * mm[1] + c * mm[3];
    [(r0x * sc[0]) as f32, (r0y * sc[0]) as f32, (r1x * sc[1]) as f32, (r1y * sc[1]) as f32, tr[0] as f32, tr[1] as f32]
}

/// CSS Color 4 §12.1: legacy sRGB colors interpolate in premultiplied
/// sRGB; once either is not legacy, in Oklab, and the result keeps its
/// Oklab components.
fn interpolate_colors(a: &Val, b: &Val, t: f32) -> Val {
    use rabbithole_style::color_model::{interpolate, Abs, Space};
    let abs = |v: &Val| match v {
        Val::Cw(c) => (*c, true),
        Val::C(c) => (Abs::from_gfx(*c), false),
        _ => (Abs::from_gfx(Color::TRANSPARENT), false),
    };
    if let (Val::C(p), Val::C(q)) = (a, b) {
        // Premultiplied, in byte units as engines do.
        let (pa, qa) = (f32::from(p.a) / 255.0, f32::from(q.a) / 255.0);
        let alpha = lerp(pa, qa, t).clamp(0.0, 1.0);
        let ch = |x: u8, y: u8| {
            let v = lerp(f32::from(x) * pa, f32::from(y) * qa, t);
            (if alpha > 0.0 { v / alpha } else { 0.0 }).round().clamp(0.0, 255.0) as u8
        };
        return Val::C(Color { r: ch(p.r, q.r), g: ch(p.g, q.g), b: ch(p.b, q.b), a: (alpha * 255.0).round() as u8 });
    }
    let ((x, _), (y, _)) = (abs(a), abs(b));
    let t = f64::from(t);
    let mut mixed = interpolate(&x.convert(Space::Oklab), &y.convert(Space::Oklab), t, rabbithole_style::HueMethod::Shorter);
    mixed.c[0] = mixed.c[0].clamp(0.0, 1.0);
    Val::Cw(mixed)
}

fn interpolate(a: &Val, b: &Val, t: f32) -> Val {
    let discrete = || if t < 0.5 { a.clone() } else { b.clone() };
    match (a, b) {
        (Val::Tracks(x), Val::Tracks(y)) => GridOperation::Interpolate(t).tracks(x, y).map(Val::Tracks).unwrap_or_else(discrete),
        // Finite tolerances interpolate as non-negative lengths.
        (Val::Tolerance(Some(x)), Val::Tolerance(Some(y))) if !matches!(x, Length::Px(v) if v.is_infinite()) && !matches!(y, Length::Px(v) if v.is_infinite()) => {
            match lerp_lp(x, y, t) {
                Some(Length::Px(v)) => Val::Tolerance(Some(Length::Px(v.max(0.0)))),
                Some(Length::Percent(v)) => Val::Tolerance(Some(Length::Percent(v.max(0.0)))),
                Some(l) => Val::Tolerance(Some(l)),
                None => discrete(),
            }
        }
        (Val::MotionPosition(MotionPosition::Coordinates(x,y)), Val::MotionPosition(MotionPosition::Coordinates(u,v))) =>
            lerp_pair(&(*x,*y),&(*u,*v),t).map(|(x,y)| Val::MotionPosition(MotionPosition::Coordinates(x,y))).unwrap_or_else(discrete),
        (Val::MotionRotation(a), Val::MotionRotation(b)) if a.follows_path == b.follows_path =>
            Val::MotionRotation(MotionRotation { follows_path: a.follows_path, degrees: lerp(a.degrees,b.degrees,t) }),
        (Val::F(x), Val::F(y)) => Val::F(lerp(*x, *y, t)),
        // Integers interpolate rounded to the nearest (CSS Values 4 §3.1).
        (Val::Count(Some(x)), Val::Count(Some(y))) => Val::Count(Some((lerp(*x as f32, *y as f32, t) + 0.5).floor().max(1.0) as u32)),
        (Val::OptPx(Some(x)), Val::OptPx(Some(y))) => Val::OptPx(Some(lerp(*x, *y, t).max(0.0))),
        (Val::C(_) | Val::Cw(_), Val::C(_) | Val::Cw(_)) => interpolate_colors(a, b, t),
        (Val::L(x), Val::L(y)) => lerp_lp(x, y, t).map(Val::L).unwrap_or_else(discrete),
        (Val::T(x), Val::T(y)) => lerp_transforms(x, y, t).map(Val::T).unwrap_or_else(discrete),
        (Val::Fl(x), Val::Fl(y)) => {
            // Filter Effects §8: a missing list is the identity of the other;
            // lists of the same functions interpolate pairwise.
            let (x, y): (Vec<FilterOp>, Vec<FilterOp>) = if x.is_empty() {
                (y.iter().map(filter_identity).collect(), y.clone())
            } else if y.is_empty() {
                (x.clone(), x.iter().map(filter_identity).collect())
            } else {
                (x.clone(), y.clone())
            };
            if x.len() == y.len() {
                let ops: Option<Vec<FilterOp>> = x.iter().zip(y.iter()).map(|(p, q)| lerp_filter(p, q, t)).collect();
                if let Some(ops) = ops {
                    return Val::Fl(ops);
                }
            }
            discrete()
        }
        (Val::Cp(Some(x), a_at), Val::Cp(Some(y), b_at)) => match lerp_clip(x, y, t) {
            Some(c) => Val::Cp(Some(std::sync::Arc::new(c)), *a_at || *b_at),
            None => discrete(),
        },
        (Val::V(x), Val::V(y)) => {
            if t <= 0.0 {
                Val::V(*x)
            } else if t >= 1.0 {
                Val::V(*y)
            } else if *x == Visibility::Visible || *y == Visibility::Visible {
                Val::V(Visibility::Visible)
            } else {
                Val::V(*y)
            }
        }
        _ => discrete(),
    }
}

// ---------------------------------------------------------------- timing

#[derive(Debug, Clone, PartialEq)]
enum Easing {
    Linear,
    Bezier(f32, f32, f32, f32),
    /// steps(n, position): (n, jump at start, extra jumps: -1 for
    /// `jump-none`, 1 for `jump-both`).
    Steps(u32, bool, i8),
}

fn parse_easing(s: &str) -> Easing {
    let s = s.trim().to_ascii_lowercase();
    match s.as_str() {
        "linear" => return Easing::Linear,
        "ease" => return Easing::Bezier(0.25, 0.1, 0.25, 1.0),
        "ease-in" => return Easing::Bezier(0.42, 0.0, 1.0, 1.0),
        "ease-out" => return Easing::Bezier(0.0, 0.0, 0.58, 1.0),
        "ease-in-out" => return Easing::Bezier(0.42, 0.0, 0.58, 1.0),
        "step-start" => return Easing::Steps(1, true, 0),
        "step-end" => return Easing::Steps(1, false, 0),
        _ => {}
    }
    let args = |prefix: &str| -> Option<Vec<String>> {
        let inner = s.strip_prefix(prefix)?.strip_prefix('(')?.strip_suffix(')')?;
        Some(inner.split(',').map(|a| a.trim().to_string()).collect())
    };
    if let Some(a) = args("cubic-bezier") {
        let n: Vec<f32> = a.iter().filter_map(|x| x.parse().ok()).collect();
        if n.len() == 4 {
            return Easing::Bezier(n[0].clamp(0.0, 1.0), n[1], n[2].clamp(0.0, 1.0), n[3]);
        }
    }
    if let Some(a) = args("steps") {
        let n = a.first().and_then(|x| x.parse::<u32>().ok()).unwrap_or(1).max(1);
        let pos = a.get(1).map(|p| p.as_str()).unwrap_or("end");
        return match pos {
            "start" | "jump-start" => Easing::Steps(n, true, 0),
            "jump-both" => Easing::Steps(n, true, 1),
            "jump-none" if n >= 2 => Easing::Steps(n, false, -1),
            _ => Easing::Steps(n, false, 0),
        };
    }
    if s.starts_with("linear(") {
        // linear() stop lists are approximated by a straight line.
        return Easing::Linear;
    }
    Easing::Bezier(0.25, 0.1, 0.25, 1.0)
}

fn bezier64(p1: f64, p2: f64, t: f64) -> f64 {
    let u = 1.0 - t;
    3.0 * u * u * t * p1 + 3.0 * u * t * t * p2 + t * t * t
}

fn ease(e: &Easing, x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    match *e {
        Easing::Linear => x,
        Easing::Steps(n, start, extra) => {
            let jumps = (n as i64 + extra as i64).max(1) as f32;
            let n = n as f32;
            let s = if start { (x * n).floor() + 1.0 } else { (x * n).floor() };
            (s.min(jumps) / jumps).clamp(0.0, 1.0)
        }
        Easing::Bezier(x1, y1, x2, y2) => {
            if x <= 0.0 || x >= 1.0 {
                return x;
            }
            // Solve bezier_x(t) = x (monotonic in t): Newton steps, then
            // bisection.
            let (x, x1, y1, x2, y2) = (x as f64, x1 as f64, y1 as f64, x2 as f64, y2 as f64);
            let bx = |t: f64| bezier64(x1, x2, t);
            let mut t = x;
            let mut solved = false;
            for _ in 0..8 {
                let e = bx(t) - x;
                if e.abs() < 1e-9 {
                    solved = true;
                    break;
                }
                let u = 1.0 - t;
                let d = 3.0 * u * u * x1 + 6.0 * u * t * (x2 - x1) + 3.0 * t * t * (1.0 - x2);
                if d.abs() < 1e-9 {
                    break;
                }
                t = (t - e / d).clamp(0.0, 1.0);
            }
            if !solved {
                let (mut lo, mut hi) = (0.0f64, 1.0f64);
                t = x;
                for _ in 0..60 {
                    let v = bx(t);
                    if (v - x).abs() < 1e-9 {
                        break;
                    }
                    if v < x {
                        lo = t;
                    } else {
                        hi = t;
                    }
                    t = (lo + hi) / 2.0;
                }
            }
            bezier64(y1, y2, t) as f32
        }
    }
}

// ---------------------------------------------------------------- state

/// Playback of a transition or animation, as a Web Animations animation:
/// start time (engine ms), hold time (local ms) and playback rate. CSS
/// starts it running; scripts may pause, seek, reverse or cancel it
/// through its `CSSAnimation` / `CSSTransition` object.
#[derive(Debug, Clone, PartialEq)]
struct Clock {
    start: Option<f64>,
    hold: Option<f64>,
    rate: f64,
    /// A script controls the playback: `animation-play-state` no longer
    /// applies.
    api: bool,
    /// Just created: held at 0 until the next animation frame starts it
    /// (as a pending Web Animations play).
    pending: bool,
}

impl Clock {
    fn pending() -> Clock {
        Clock { start: None, hold: Some(0.0), rate: 1.0, api: false, pending: true }
    }

    /// Local time (ms since the start), None when idle (cancelled).
    fn local(&self, now: f64) -> Option<f64> {
        self.hold.or_else(|| self.start.map(|s| (now - s) * self.rate))
    }

    fn idle(&self) -> bool {
        self.start.is_none() && self.hold.is_none()
    }
}

#[derive(Debug, Clone)]
struct Transition {
    id: u64,
    node: NodeId,
    prop: Prop,
    from: Val,
    to: Val,
    clock: Clock,
    /// Style change (restyle) that created it (composite order).
    generation: u64,
    delay: f64,
    duration: f64,
    easing: Easing,
    timing: String,
    started: bool,
}

impl Transition {
    fn value_at(&self, now: f64) -> Val {
        let local = self.clock.local(now).unwrap_or(0.0);
        let t = ((local - self.delay) / self.duration.max(1e-9)).clamp(0.0, 1.0) as f32;
        interpolate(&self.from, &self.to, ease(&self.easing, t))
    }
}

#[derive(Debug, Clone)]
struct Keyframe {
    composite: Option<Composite>,
    offset: f32,
    values: Vec<(Prop, Val)>,
}

#[derive(Debug, Clone)]
struct Animation {
    id: u64,
    node: NodeId,
    name: String,
    frames: Vec<Keyframe>,
    clock: Clock,
    /// Position in the element's `animation-name` list.
    index: usize,
    timing: String,
    delay: f64,
    duration: f64,
    /// None = infinite
    iterations: Option<f64>,
    direction: String,
    fill: String,
    easing: Easing,
    started: bool,
    iteration: i64,
    finished: bool,
}

impl Animation {
    /// Follow `animation-play-state` (unless a script took control).
    fn set_paused(&mut self, paused: bool, now: f64) {
        let c = &mut self.clock;
        if c.api {
            return;
        }
        if c.pending {
            // Paused before it started: stays held at 0.
            if paused {
                c.pending = false;
            }
            return;
        }
        match (paused, c.hold, c.start) {
            (true, None, Some(_)) => {
                c.hold = c.local(now);
                c.start = None;
            }
            (false, Some(h), None) => {
                c.start = Some(now - h / c.rate);
                c.hold = None;
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone)]
struct QueuedEvent {
    node: NodeId,
    ty: &'static str,
    name: String,
    elapsed: f32,
    transition: bool,
}

/// Running transitions and animations of one page.
#[derive(Debug, Default)]
pub struct AnimationState {
    transitions: Vec<Transition>,
    animations: Vec<Animation>,
    /// Base (un-animated) style of elements with effects.
    base: HashMap<NodeId, Rc<ComputedStyle>>,
    events: Vec<QueuedEvent>,
    /// Engine time (ms) of the next frame needed.
    next_due: Option<f64>,
    /// Elements whose computed style declares transitions: their styles
    /// are snapshotted before a restyle so a full restyle (which reports
    /// no per-element changes) can still start transitions.
    pub(crate) with_transitions: std::collections::HashSet<NodeId>,
    /// Effects of script animations (Web Animations), shared with the
    /// page's script host once a script animates something.
    pub(crate) script: Option<rabbithole_script::WebAnimations>,
    /// Registry generation last applied.
    script_gen: u64,
    /// Elements with script effects in effect (as last applied).
    script_nodes: std::collections::HashSet<NodeId>,
    /// Keyframes of script effects resolved against their element's style.
    script_frames: HashMap<u64, ScriptFrames>,
    /// Indices of the registry's effects in effect, per element, valid
    /// for registry generation `script_index_gen`.
    script_index: HashMap<NodeId, Vec<usize>>,
    script_index_gen: u64,
    /// Last id given to a transition or animation.
    next_id: u64,
    /// Style changes seen (transition generations).
    generation: u64,
    /// Transitions / animations that ran to their end and were dropped
    /// (their script objects report them finished).
    ended: std::collections::HashSet<u64>,
}

/// A script effect's keyframes as computed values.
#[derive(Debug)]
struct ScriptFrames {
    version: u64,
    /// Style the values were computed from (pointer identity).
    base: usize,
    frames: Vec<ScriptKeyframe>,
}

#[derive(Debug)]
struct ScriptKeyframe {
    offset: f32,
    easing: Easing,
    /// None = the effect's composite operation.
    composite: Option<Composite>,
    values: Vec<(Prop, Val)>,
}

impl AnimationState {
    /// When the page needs the next animation frame (engine ms).
    pub fn next_due(&self) -> Option<f64> {
        if self.active() {
            self.next_due.or(Some(0.0))
        } else {
            None
        }
    }

    /// Anything running (or events to deliver)?
    pub fn active(&self) -> bool {
        !self.transitions.is_empty() || self.animations.iter().any(|a| !a.finished) || !self.events.is_empty()
    }

    /// No effects and no element with a transition or animation seen.
    pub fn is_idle(&self) -> bool {
        self.transitions.is_empty() && self.animations.is_empty() && self.base.is_empty() && self.events.is_empty() && self.with_transitions.is_empty() && self.script_nodes.is_empty()
    }

    /// Current (base) styles of the elements with transitions, before a
    /// restyle.
    pub(crate) fn snapshot(&self, styles: &rabbithole_style::StyledTree) -> Vec<(NodeId, Rc<ComputedStyle>)> {
        self.with_transitions.iter().filter_map(|n| self.base.get(n).cloned().or_else(|| styles.get(*n).cloned()).map(|s| (*n, s))).collect()
    }

    fn has_effects(&self, node: NodeId) -> bool {
        self.transitions.iter().any(|t| t.node == node) || self.animations.iter().any(|a| a.node == node) || self.script_nodes.contains(&node)
    }
}

/// One style change seen by a restyle: (element, before, after).
pub(crate) type Change = (NodeId, Option<Rc<ComputedStyle>>, Option<Rc<ComputedStyle>>);

fn now_ms(engine: &Engine) -> f64 {
    engine.start.elapsed().as_secs_f64() * 1000.0
}

/// The transition entry (duration s, delay s, easing) covering `p`.
fn transition_for(style: &ComputedStyle, p: Prop) -> Option<&rabbithole_style::Transition> {
    style.transitions.iter().rev().find(|t| p.covered_by(&t.property))
}

impl Engine {
    /// Start / cancel effects after a restyle. `changes` lists the
    /// elements whose computed style changed (incremental restyle);
    /// `full` means every style was recomputed from scratch.
    pub(crate) fn animations_after_style(&self, page: &mut Page, changes: Vec<Change>, full: bool) {
        if changes.is_empty() && !full && page.anim.base.is_empty() {
            return;
        }
        let now = now_ms(self);
        page.anim.generation += 1;
        let mut touched: Vec<NodeId> = Vec::new();
        if full {
            // Overlays were dropped: take the new styles as bases, and pick
            // up animations on elements styled for the first time. (Changed
            // elements are handled with the changes below.)
            let changed: std::collections::HashSet<NodeId> = changes.iter().map(|c| c.0).collect();
            let nodes: Vec<NodeId> = page.anim.base.keys().copied().filter(|n| !changed.contains(n)).collect();
            for n in nodes {
                match page.styles.get(n).cloned() {
                    Some(s) => {
                        page.anim.base.insert(n, s);
                    }
                    None => self.cancel_effects(page, n, false),
                }
                touched.push(n);
            }
            let count = page.styles.styles.len();
            page.anim.with_transitions.clear();
            for i in 0..count {
                let Some(Some(s)) = page.styles.styles.get(i) else { continue };
                if !s.transitions.is_empty() {
                    page.anim.with_transitions.insert(i);
                }
                if s.animations.is_empty() || changed.contains(&i) || s.display.is_none() {
                    continue;
                }
                let node: NodeId = i;
                let s = s.clone();
                let base = page.anim.base.get(&node).cloned().unwrap_or(s);
                self.sync_animations(page, node, None, &base, now);
                // The overlay is applied over the base style.
                if page.anim.has_effects(node) {
                    page.anim.base.entry(node).or_insert(base);
                }
                touched.push(node);
            }
        }
        for (node, old, new) in changes {
            match &new {
                Some(s) if !s.transitions.is_empty() => {
                    page.anim.with_transitions.insert(node);
                }
                _ => {
                    page.anim.with_transitions.remove(&node);
                }
            }
            // `display: none` (or no box) cancels transitions and animations.
            let new = match new {
                // (Unless `display` itself transitions: allow-discrete.)
                Some(s) if !s.display.is_none() || s.transitions.iter().any(|t| t.property.trim().eq_ignore_ascii_case("display")) => s,
                _ => {
                    if page.anim.has_effects(node) {
                        self.cancel_effects(page, node, true);
                    }
                    continue;
                }
            };
            let before = page.anim.base.get(&node).cloned().or(old.clone());
            // A recomputation that only dropped our overlay is not a change.
            if let Some(b) = &before {
                if Rc::ptr_eq(b, &new) || **b == *new {
                    if page.anim.has_effects(node) {
                        page.anim.base.insert(node, new);
                        touched.push(node);
                    }
                    continue;
                }
            }
            if let Some(before) = &before {
                if !new.transitions.is_empty() || page.anim.transitions.iter().any(|t| t.node == node) {
                    self.sync_transitions(page, node, before, &new, now);
                }
            }
            if !new.animations.is_empty() || page.anim.animations.iter().any(|a| a.node == node) {
                self.sync_animations(page, node, before.as_deref(), &new, now);
            }
            if page.anim.has_effects(node) {
                page.anim.base.insert(node, new);
                touched.push(node);
            } else {
                page.anim.base.remove(&node);
            }
        }
        for n in touched {
            self.apply_overlay(page, n, now);
        }
        if page.anim.active() {
            page.anim.next_due = Some(now);
        }
    }

    fn sync_transitions(&self, page: &mut Page, node: NodeId, before: &ComputedStyle, after: &ComputedStyle, now: f64) {
        let scripted = self.script_animated(page, node);
        for &p in ALL {
            // A script animation overrides both the before- and the
            // after-change value: no transition.
            if scripted.contains(&p) {
                continue;
            }
            let running = page.anim.transitions.iter().position(|t| t.node == node && t.prop == p);
            let spec = transition_for(after, p).filter(|t| t.duration.max(0.0) + t.delay > 0.0);
            if running.is_none() && spec.is_none() { continue; }
            let to = get(after, p);
            let from = match running {
                Some(i) if !page.anim.transitions[i].clock.idle() => page.anim.transitions[i].value_at(now),
                _ => get(before, p),
            };
            if let Some(i) = running {
                if page.anim.transitions[i].to == to {
                    continue; // still heading to the same value
                }
            }
            if let Some(i) = running {
                // Replaced or no longer transitioned: cancel.
                let t = page.anim.transitions.remove(i);
                page.anim.events.push(QueuedEvent { node, ty: "transitioncancel", name: p.name().into(), elapsed: ((t.clock.local(now).unwrap_or(0.0) - t.delay).max(0.0) / 1000.0) as f32, transition: true });
            }
            let Some(spec) = spec else { continue };
            if from == to {
                continue;
            }
            // Normally only interpolable pairs start a transition.
            if !spec.allow_discrete && !interpolable(&from, &to) {
                continue;
            }
            page.anim.events.push(QueuedEvent { node, ty: "transitionrun", name: p.name().into(), elapsed: 0.0, transition: true });
            page.anim.next_id += 1;
            page.anim.transitions.push(Transition {
                id: page.anim.next_id,
                node,
                prop: p,
                from,
                to,
                clock: Clock::pending(),
                generation: page.anim.generation,
                delay: spec.delay as f64 * 1000.0,
                duration: spec.duration as f64 * 1000.0,
                easing: parse_easing(&spec.timing),
                timing: spec.timing.clone(),
                started: false,
            });
        }
    }

    fn sync_animations(&self, page: &mut Page, node: NodeId, before: Option<&ComputedStyle>, after: &ComputedStyle, now: f64) {
        let _font_units = rabbithole_style::values::scope_unit_fonts(Some(page.fonts.clone()), rabbithole_style::values::font_unit_skip(&page.styles.font_faces));
        let old_names: Vec<&str> = before.map(|b| b.animations.iter().map(|a| a.name.as_str()).collect()).unwrap_or_default();
        // Cancel animations whose name went away.
        let names: Vec<String> = after.animations.iter().map(|a| a.name.clone()).collect();
        let mut i = 0;
        while i < page.anim.animations.len() {
            let a = &page.anim.animations[i];
            if a.node == node && !names.contains(&a.name) {
                let a = page.anim.animations.remove(i);
                if !a.finished {
                    page.anim.events.push(QueuedEvent { node, ty: "animationcancel", name: a.name, elapsed: 0.0, transition: false });
                }
            } else {
                i += 1;
            }
        }
        for (index, spec) in after.animations.iter().enumerate() {
            if spec.name.is_empty() || spec.name == "none" {
                continue;
            }
            if let Some(a) = page.anim.animations.iter_mut().find(|a| a.node == node && a.name == spec.name) {
                a.set_paused(spec.paused, now);
                a.index = index;
            }
            let running = page.anim.animations.iter().any(|a| a.node == node && a.name == spec.name);
            if running || (old_names.contains(&spec.name.as_str()) && before.is_some() && page.anim.base.contains_key(&node)) {
                continue;
            }
            let Some(raw) = page.styles.keyframes.get(&spec.name).cloned() else { continue };
            let parent = page.doc.parent(node).and_then(|p| page.styles.get(p).cloned()).unwrap_or_else(|| Rc::new(ComputedStyle::initial()));
            let viewport = (page.viewport.width, page.viewport.height);
            let root_fs = page.styles.root_font_size;
            let mut frames: Vec<Keyframe> = Vec::new();
            for (offsets, decls) in &raw {
                let styled = rabbithole_style::cascade::apply_over(after, &parent, decls, root_fs, viewport);
                let declared: Vec<Prop> = ALL.iter().copied().filter(|p| decls.iter().any(|d| p.covered_by(&d.name))).collect();
                let values: Vec<(Prop, Val)> = declared.into_iter().map(|p| (p, get(&styled, p))).collect();
                for &o in offsets {
                    let composite = decls.iter().any(|d| !d.important && d.name == "animation-composition")
                        .then(|| styled.animation_composition.as_deref().and_then(|ops| ops.first().copied()))
                        .flatten();
                    frames.push(Keyframe { composite, offset: o.clamp(0.0, 1.0), values: values.clone() });
                }
            }
            frames.sort_by(|a, b| a.offset.partial_cmp(&b.offset).unwrap_or(std::cmp::Ordering::Equal));
            page.anim.next_id += 1;
            let mut clock = Clock::pending();
            if spec.paused {
                clock.pending = false;
            }
            page.anim.animations.push(Animation {
                id: page.anim.next_id,
                node,
                name: spec.name.clone(),
                frames,
                clock,
                index,
                timing: spec.timing.clone(),
                delay: spec.delay as f64 * 1000.0,
                duration: spec.duration as f64 * 1000.0,
                iterations: spec.iterations.map(|n| n as f64),
                direction: spec.direction.clone(),
                fill: spec.fill_mode.clone(),
                easing: parse_easing(&spec.timing),
                started: false,
                iteration: 0,
                finished: false,
            });
        }
    }

    fn cancel_effects(&self, page: &mut Page, node: NodeId, events: bool) {
        let now = now_ms(self);
        let a = &mut page.anim;
        let mut ev = Vec::new();
        a.transitions.retain(|t| {
            if t.node == node {
                let elapsed = ((t.clock.local(now).unwrap_or(0.0) - t.delay).max(0.0) / 1000.0) as f32;
                ev.push(QueuedEvent { node, ty: "transitioncancel", name: t.prop.name().into(), elapsed, transition: true });
                false
            } else {
                true
            }
        });
        a.animations.retain(|x| {
            if x.node == node {
                if !x.finished {
                    ev.push(QueuedEvent { node, ty: "animationcancel", name: x.name.clone(), elapsed: 0.0, transition: false });
                }
                false
            } else {
                true
            }
        });
        a.base.remove(&node);
        if events {
            a.events.extend(ev);
        }
    }

    fn css_animation_style(&self, page: &Page, node: NodeId, base: &ComputedStyle, now: f64) -> ComputedStyle {
        let mut style = base.clone();
        for t in page.anim.transitions.iter().filter(|t| t.node == node && !t.clock.idle()) {
            set(&mut style, t.prop, &t.value_at(now));
        }
        // Composite order: position in `animation-name`.
        let mut anims: Vec<&Animation> = page.anim.animations.iter().filter(|a| a.node == node).collect();
        anims.sort_by_key(|a| a.index);
        for a in anims {
            for (p, v) in animation_values(a, &style, now) {
                // `!important` declarations override animations.
                if base.important.as_ref().is_some_and(|names| names.iter().any(|n| p.covered_by(n))) {
                    continue;
                }
                set(&mut style, p, &v);
            }
        }
        style
    }

    /// Write base + animated values into `page.styles` for `node`.
    fn apply_overlay(&self, page: &mut Page, node: NodeId, now: f64) {
        let Some(base) = page.anim.base.get(&node).cloned() else { return };
        let mut style = self.css_animation_style(page, node, &base, now);
        if page.anim.script_nodes.contains(&node) {
            self.apply_script_effects(page, node, &base, &mut style, None);
        }
        let changed = page.styles.get(node).map(|s| **s != style).unwrap_or(true);
        if changed {
            page.styles.any_position_area |= style.needs_anchor_layout();
            page.styles.any_motion_path |= style.has_motion_path();
            if let Some(slot) = page.styles.styles.get_mut(node) {
                *slot = Some(Rc::new(style));
                page.incr.style_patched(node);
            }
        }
    }

    /// Apply the script effects in effect on `node` (composite order up to
    /// `upto`) over `style` (the base style with CSS effects applied).
    fn apply_script_effects(&self, page: &mut Page, node: NodeId, base: &Rc<ComputedStyle>, style: &mut ComputedStyle, upto: Option<u64>) {
        let Some(reg) = page.anim.script.clone() else { return };
        let reg = reg.borrow();
        let base_key = Rc::as_ptr(base) as usize;
        // The per-element index (linear scan when the registry changed
        // since it was built).
        let indexed: Vec<usize> = if page.anim.script_index_gen == reg.generation {
            page.anim.script_index.get(&node).cloned().unwrap_or_default()
        } else {
            reg.effects.iter().enumerate().filter(|(_, e)| e.node == node && e.progress.is_some()).map(|(i, _)| i).collect()
        };
        for e in indexed.into_iter().filter_map(|i| reg.effects.get(i)) {
            if upto.is_some_and(|o| e.order > o) {
                break;
            }
            let Some(progress) = e.progress else { continue };
            let fresh = page.anim.script_frames.get(&e.id).is_some_and(|f| f.version == e.frames_version && f.base == base_key);
            if !fresh {
                let frames = self.resolve_script_frames(page, node, base, &e.frames);
                page.anim.script_frames.insert(e.id, ScriptFrames { version: e.frames_version, base: base_key, frames });
            }
            let Some(sf) = page.anim.script_frames.get(&e.id) else { continue };
            let effect_composite = Composite::parse(&e.composite).unwrap_or(Composite::Replace);
            for (p, v) in script_effect_values(&sf.frames, progress, effect_composite, style) {
                if base.important.as_ref().is_some_and(|names| names.iter().any(|n| p.covered_by(n))) {
                    continue;
                }
                set(style, p, &v);
            }
        }
    }

    /// Computed values of script keyframes, as `@keyframes` frames are
    /// computed for CSS animations.
    fn resolve_script_frames(&self, page: &Page, node: NodeId, base: &ComputedStyle, frames: &[rabbithole_script::WebAnimKeyframe]) -> Vec<ScriptKeyframe> {
        let _font_units = rabbithole_style::values::scope_unit_fonts(Some(page.fonts.clone()), rabbithole_style::values::font_unit_skip(&page.styles.font_faces));
        let parent = page.doc.parent(node).and_then(|p| page.styles.get(p).cloned()).unwrap_or_else(|| Rc::new(ComputedStyle::initial()));
        let viewport = (page.viewport.width, page.viewport.height);
        let root_fs = page.styles.root_font_size;
        frames
            .iter()
            .map(|f| {
                let text: String = f.decls.iter().filter(|(p, _)| !p.starts_with("--")).map(|(p, v)| format!("{p}: {v};")).collect();
                let decls = rabbithole_css::parse_declaration_list(&text);
                let styled = rabbithole_style::cascade::apply_over(base, &parent, &decls, root_fs, viewport);
                let values = ALL.iter().copied().filter(|p| decls.iter().any(|d| p.covered_by(&d.name))).map(|p| (p, get(&styled, p))).collect();
                ScriptKeyframe { offset: f.offset as f32, easing: parse_easing(&f.easing), composite: Composite::parse(&f.composite), values }
            })
            .collect()
    }

    /// Apply changes of the script animations' effects (Web Animations) to
    /// the page's styles. Returns whether a style changed.
    pub(crate) fn sync_script_animations(&self, page: &mut Page) -> bool {
        let Some(reg) = page.anim.script.clone() else { return false };
        let (generation, nodes) = {
            let r = reg.borrow();
            if r.generation == page.anim.script_gen {
                return false;
            }
            let nodes: std::collections::HashSet<NodeId> = r.effects.iter().filter(|e| e.progress.is_some()).map(|e| e.node).collect();
            let mut index: HashMap<NodeId, Vec<usize>> = HashMap::new();
            for (i, e) in r.effects.iter().enumerate() {
                if e.progress.is_some() {
                    index.entry(e.node).or_default().push(i);
                }
            }
            page.anim.script_index = index;
            page.anim.script_index_gen = r.generation;
            (r.generation, nodes)
        };
        {
            let r = reg.borrow();
            page.anim.script_frames.retain(|id, _| r.effects.iter().any(|e| e.id == *id));
        }
        let connected: Vec<NodeId> = nodes.into_iter().filter(|n| page.doc.is_connected(*n)).collect();
        // Elements not styled yet (inserted since the last restyle) get
        // their effects once styled: apply this generation again then.
        let unstyled = connected.iter().any(|n| page.styles.get(*n).is_none());
        page.anim.script_gen = if unstyled { generation.wrapping_sub(1) } else { generation };
        let nodes: std::collections::HashSet<NodeId> = connected.into_iter().filter(|n| page.styles.get(*n).is_some()).collect();
        let old = std::mem::replace(&mut page.anim.script_nodes, nodes);
        let mut all: Vec<NodeId> = old.union(&page.anim.script_nodes).copied().collect();
        all.sort_unstable();
        let before = page.incr.generation();
        let now = now_ms(self);
        for n in all {
            if !page.anim.has_effects(n) {
                if let Some(base) = page.anim.base.remove(&n) {
                    if let Some(slot) = page.styles.styles.get_mut(n) {
                        *slot = Some(base);
                        page.incr.style_patched(n);
                    }
                }
                continue;
            }
            if !page.anim.base.contains_key(&n) {
                let Some(s) = page.styles.get(n).cloned() else { continue };
                page.anim.base.insert(n, s);
            }
            self.apply_overlay(page, n, now);
        }
        page.incr.generation() != before
    }

    /// Style of `node` with its CSS effects and the script effects up to
    /// composite order `order` applied (`commitStyles`).
    pub(crate) fn script_animation_style(&self, page: &mut Page, node: NodeId, order: u64) -> Option<Rc<ComputedStyle>> {
        self.sync_script_animations(page);
        let base = page.anim.base.get(&node).cloned().or_else(|| page.styles.get(node).cloned())?;
        let now = now_ms(self);
        let mut style = self.css_animation_style(page, node, &base, now);
        self.apply_script_effects(page, node, &base, &mut style, Some(order));
        Some(Rc::new(style))
    }

    /// `nodes` were restyled without a reported change: their current
    /// style (equal to the animated one) is their new base style.
    pub(crate) fn refresh_animation_bases(&self, page: &mut Page, nodes: &[NodeId]) {
        let now = now_ms(self);
        for &n in nodes {
            if !page.anim.base.contains_key(&n) {
                continue;
            }
            let Some(s) = page.styles.get(n).cloned() else { continue };
            page.anim.base.insert(n, s);
            self.apply_overlay(page, n, now);
        }
    }

    /// Properties script effects in effect animate on `node`.
    fn script_animated(&self, page: &Page, node: NodeId) -> Vec<Prop> {
        let Some(reg) = &page.anim.script else { return Vec::new() };
        let reg = reg.borrow();
        let mut out = Vec::new();
        for e in reg.effects_of(node) {
            for f in e.frames.iter() {
                for (name, _) in &f.decls {
                    for &p in ALL {
                        if !out.contains(&p) && p.covered_by(name) {
                            out.push(p);
                        }
                    }
                }
            }
        }
        out
    }

    /// Script view of a CSS transition / animation.
    fn css_info_of(&self, page: &Page, id: u64) -> Option<rabbithole_script::CssAnimationInfo> {
        use rabbithole_script::CssAnimationInfo;
        if let Some(t) = page.anim.transitions.iter().find(|t| t.id == id) {
            return Some(CssAnimationInfo {
                id,
                node: t.node,
                transition: true,
                name: t.prop.name().to_string(),
                start_time: t.clock.start,
                hold_time: t.clock.hold,
                pending: t.clock.pending,
                rate: t.clock.rate,
                delay: t.delay,
                duration: t.duration,
                iterations: 1.0,
                direction: "normal".into(),
                fill: "backwards".into(),
                easing: t.timing.clone(),
                keyframe_easing: "linear".into(),
                order: t.generation,
                ended: false,
                keyframes: Vec::new(),
            });
        }
        if let Some(a) = page.anim.animations.iter().find(|a| a.id == id) {
            let keyframes = page
                .styles
                .keyframes
                .get(&a.name)
                .map(|raw| {
                    let mut out: Vec<(f64, Vec<rabbithole_css::Declaration>)> = Vec::new();
                    for (offsets, decls) in raw {
                        for o in offsets {
                            out.push((o.clamp(0.0, 1.0) as f64, decls.clone()));
                        }
                    }
                    out.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
                    out
                })
                .unwrap_or_default();
            return Some(CssAnimationInfo {
                id,
                node: a.node,
                transition: false,
                name: a.name.clone(),
                start_time: a.clock.start,
                hold_time: a.clock.hold,
                pending: a.clock.pending,
                rate: a.clock.rate,
                delay: a.delay,
                duration: a.duration,
                iterations: a.iterations.unwrap_or(f64::INFINITY),
                direction: a.direction.clone(),
                fill: a.fill.clone(),
                easing: "linear".into(),
                keyframe_easing: a.timing.clone(),
                order: a.index as u64,
                ended: false,
                keyframes,
            });
        }
        page.anim.ended.contains(&id).then(|| CssAnimationInfo { id, ended: true, ..Default::default() })
    }

    /// The page's CSS transitions and animations (connected elements).
    pub(crate) fn css_animation_infos(&self, page: &Page) -> Vec<rabbithole_script::CssAnimationInfo> {
        let ids: Vec<u64> = page
            .anim
            .transitions
            .iter()
            .filter(|t| rendered(page, t.node))
            .map(|t| t.id)
            .chain(page.anim.animations.iter().filter(|a| rendered(page, a.node)).map(|a| a.id))
            .collect();
        ids.into_iter().filter_map(|id| self.css_info_of(page, id)).collect()
    }

    pub(crate) fn css_animation_info(&self, page: &Page, id: u64) -> Option<rabbithole_script::CssAnimationInfo> {
        self.css_info_of(page, id)
    }

    /// A script set a CSS transition / animation's playback. Returns
    /// whether a style changed.
    pub(crate) fn set_css_animation(&self, page: &mut Page, id: u64, start: Option<f64>, hold: Option<f64>, rate: f64) -> bool {
        let now = now_ms(self);
        let clock = Clock { start, hold, rate, api: true, pending: false };
        let mut node = None;
        if let Some(t) = page.anim.transitions.iter_mut().find(|t| t.id == id) {
            if !t.clock.idle() && clock.idle() {
                page.anim.events.push(QueuedEvent { node: t.node, ty: "transitioncancel", name: t.prop.name().into(), elapsed: 0.0, transition: true });
            }
            t.clock = clock;
            node = Some(t.node);
        } else if let Some(a) = page.anim.animations.iter_mut().find(|a| a.id == id) {
            if !a.clock.idle() && clock.idle() && !a.finished {
                page.anim.events.push(QueuedEvent { node: a.node, ty: "animationcancel", name: a.name.clone(), elapsed: 0.0, transition: false });
            }
            a.clock = clock;
            // Seeking back before the end runs it again.
            let active = a.iterations.map(|n| a.duration * n).unwrap_or(f64::INFINITY);
            if let Some(local) = a.clock.local(now) {
                if local - a.delay < active && (a.clock.rate >= 0.0 || local > 0.0) {
                    a.finished = false;
                }
            }
            node = Some(a.node);
        }
        let Some(node) = node else { return false };
        if !page.anim.base.contains_key(&node) {
            let Some(s) = page.styles.get(node).cloned() else { return false };
            page.anim.base.insert(node, s);
        }
        let before = page.incr.generation();
        self.apply_overlay(page, node, now);
        page.anim.next_due = Some(now);
        page.incr.generation() != before
    }

    /// Advance transitions and animations; queue their events. Returns
    /// whether a style changed (relayout / repaint needed).
    pub(crate) fn advance_css_animations(&self, page: &mut Page) -> bool {
        if !page.anim.active() {
            return false;
        }
        let now = now_ms(self);
        let mut nodes: Vec<NodeId> = Vec::new();
        // Effects created since the last frame start now.
        for c in page.anim.transitions.iter_mut().map(|t| &mut t.clock).chain(page.anim.animations.iter_mut().map(|a| &mut a.clock)) {
            if c.pending {
                c.pending = false;
                c.start = Some(now - c.hold.unwrap_or(0.0) / c.rate);
                c.hold = None;
            }
        }
        // Transitions: start / end.
        let mut i = 0;
        while i < page.anim.transitions.len() {
            let t = &mut page.anim.transitions[i];
            nodes.push(t.node);
            let Some(local) = t.clock.local(now) else {
                i += 1;
                continue;
            };
            if !t.started && local >= t.delay {
                t.started = true;
                let (node, name) = (t.node, t.prop.name().to_string());
                page.anim.events.push(QueuedEvent { node, ty: "transitionstart", name, elapsed: 0.0, transition: true });
            }
            let t = &page.anim.transitions[i];
            if local >= t.delay + t.duration && t.clock.rate > 0.0 {
                let t = page.anim.transitions.remove(i);
                page.anim.ended.insert(t.id);
                page.anim.events.push(QueuedEvent { node: t.node, ty: "transitionend", name: t.prop.name().into(), elapsed: (t.duration / 1000.0) as f32, transition: true });
            } else {
                i += 1;
            }
        }
        // Animations: start / iteration / end.
        for a in page.anim.animations.iter_mut() {
            nodes.push(a.node);
            if a.finished {
                continue;
            }
            let Some(local) = a.clock.local(now) else { continue };
            let local = local - a.delay;
            if !a.started && local >= 0.0 {
                a.started = true;
                page.anim.events.push(QueuedEvent { node: a.node, ty: "animationstart", name: a.name.clone(), elapsed: 0.0, transition: false });
            }
            if local < 0.0 {
                continue;
            }
            let active = match a.iterations {
                Some(n) => a.duration * n,
                None => f64::INFINITY,
            };
            if local >= active || (a.clock.rate < 0.0 && local + a.delay <= 0.0 && a.started) {
                a.finished = true;
                page.anim.events.push(QueuedEvent { node: a.node, ty: "animationend", name: a.name.clone(), elapsed: (active / 1000.0) as f32, transition: false });
            } else if a.duration > 0.0 {
                let it = (local / a.duration).floor() as i64;
                if it > a.iteration {
                    a.iteration = it;
                    page.anim.events.push(QueuedEvent { node: a.node, ty: "animationiteration", name: a.name.clone(), elapsed: (it as f64 * a.duration / 1000.0) as f32, transition: false });
                }
            }
        }
        // Finished animations without forwards fill stop affecting style.
        let ended = &mut page.anim.ended;
        page.anim.animations.retain(|a| {
            let keep = !(a.finished && !(a.fill == "forwards" || a.fill == "both"));
            if !keep {
                ended.insert(a.id);
            }
            keep
        });
        if ended.len() > 100_000 {
            ended.clear();
        }
        nodes.sort_unstable();
        nodes.dedup();
        let before = page.incr.generation();
        for n in &nodes {
            if !page.anim.has_effects(*n) {
                // Last effect ended: back to the base style.
                if let Some(base) = page.anim.base.remove(n) {
                    if let Some(slot) = page.styles.styles.get_mut(*n) {
                        *slot = Some(base);
                        page.incr.style_patched(*n);
                    }
                }
            } else {
                self.apply_overlay(page, *n, now);
            }
        }
        page.anim.next_due = Some(now + 16.0);
        page.incr.generation() != before
    }

    /// Deliver queued transition/animation events.
    pub(crate) fn dispatch_animation_events(&self, page: &mut Page) -> ScriptOutcome {
        let events = std::mem::take(&mut page.anim.events);
        let mut out = ScriptOutcome::default();
        for e in events {
            if !page.doc.is_connected(e.node) {
                continue;
            }
            let init = EventInit {
                bubbles: true,
                cancelable: matches!(e.ty, "transitionrun" | "transitionstart" | "transitionend" | "transitioncancel") && false,
                detail: Some(EventDetail::Animation { name: e.name, elapsed: e.elapsed, pseudo: String::new(), transition: e.transition }),
            };
            let o = self.dispatch_event(page, e.node, e.ty, init);
            out.merge(o);
        }
        out
    }
}

/// Is `node` connected and rendered (not `display: none`)?
fn rendered(page: &Page, node: NodeId) -> bool {
    page.doc.is_connected(node) && page.styles.get(node).is_some_and(|s| !s.display.is_none())
}

/// `under` composited with `v` (`add` / `accumulate`).
fn add_values(under: &Val, v: &Val) -> Val {
    match (under, v) {
        (Val::Tracks(a), Val::Tracks(b)) => GridOperation::Add.tracks(a, b).map(Val::Tracks).unwrap_or_else(|| v.clone()),
        (Val::F(a), Val::F(b)) => Val::F(a + b),
        (Val::Count(Some(a)), Val::Count(Some(b))) => Val::Count(Some(a + b)),
        (Val::OptPx(Some(a)), Val::OptPx(Some(b))) => Val::OptPx(Some(a + b)),
        (Val::L(a), Val::L(b)) => add_lp(*a, *b).map(Val::L).unwrap_or_else(|| v.clone()),
        (Val::T(a), Val::T(b)) => Val::T(a.iter().chain(b.iter()).cloned().collect()),
        (Val::C(_) | Val::Cw(_), Val::C(_) | Val::Cw(_)) => {
            let gfx = |v: &Val| match v { Val::Cw(c) => c.to_gfx(), Val::C(c) => *c, _ => Color::TRANSPARENT };
            let (a, b) = (gfx(under), gfx(v));
            let ch = |p: u8, q: u8| (p as u16 + q as u16).min(255) as u8;
            Val::C(Color { r: ch(a.r, b.r), g: ch(a.g, b.g), b: ch(a.b, b.b), a: ch(a.a, b.a) })
        }
        (Val::Fl(a), Val::Fl(b)) => Val::Fl(a.iter().chain(b.iter()).cloned().collect()),
        _ => v.clone(),
    }
}

/// Keyframe easing, extrapolating cubic Béziers linearly outside [0, 1]
/// (the timing function's output may overshoot).
fn ease_ext(e: &Easing, x: f32) -> f32 {
    if (0.0..=1.0).contains(&x) {
        return ease(e, x);
    }
    match e {
        Easing::Steps(..) => ease(e, x),
        _ => x,
    }
}

/// Values of a script keyframe effect at (eased) iteration `progress`
/// over the underlying style `under` (Web Animations §5.3).
fn script_effect_values(frames: &[ScriptKeyframe], progress: f64, composite: Composite, under: &ComputedStyle) -> Vec<(Prop, Val)> {
    let p = progress as f32;
    let mut props: Vec<Prop> = Vec::new();
    for f in frames {
        for (pr, _) in &f.values {
            if !props.contains(pr) {
                props.push(*pr);
            }
        }
    }
    let mut out = Vec::with_capacity(props.len());
    for pr in props {
        let underlying = get(under, pr);
        // Property-specific keyframes, composited with the underlying value;
        // neutral keyframes at 0 / 1 where missing.
        let mut ks: Vec<(f32, &Easing, Val)> = Vec::new();
        for f in frames {
            let Some((_, v)) = f.values.iter().find(|(q, _)| *q == pr) else { continue };
            let v = match f.composite.unwrap_or(composite) {
                Composite::Replace => v.clone(),
                Composite::Add | Composite::Accumulate => add_values(&underlying, v),
            };
            ks.push((f.offset, &f.easing, v));
        }
        if ks.is_empty() {
            continue;
        }
        if ks[0].0 != 0.0 {
            ks.insert(0, (0.0, &Easing::Linear, underlying.clone()));
        }
        if ks[ks.len() - 1].0 != 1.0 {
            ks.push((1.0, &Easing::Linear, underlying.clone()));
        }
        let n = ks.len();
        let value = if p < 0.0 && ks.iter().filter(|k| k.0 == 0.0).count() > 1 {
            ks[0].2.clone()
        } else if p >= 1.0 && ks.iter().filter(|k| k.0 == 1.0).count() > 1 {
            ks[n - 1].2.clone()
        } else if n == 1 {
            ks[0].2.clone()
        } else {
            let i = if p < 0.0 {
                0
            } else if p >= 1.0 {
                n - 2
            } else {
                ks.iter().rposition(|k| k.0 <= p && k.0 < 1.0).unwrap_or(0).min(n - 2)
            };
            let (o0, e0, v0) = &ks[i];
            let (o1, _, v1) = &ks[i + 1];
            let span = o1 - o0;
            let t = if span.abs() < 1e-9 { 0.0 } else { (p - o0) / span };
            interpolate(v0, v1, ease_ext(e0, t))
        };
        out.push((pr, value));
    }
    out
}

/// Can `a` and `b` be interpolated (not just swapped at 50%)?
fn interpolable(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::F(_), Val::F(_)) | (Val::C(_) | Val::Cw(_), Val::C(_) | Val::Cw(_)) | (Val::V(_), Val::V(_)) => true,
        (Val::Count(Some(_)), Val::Count(Some(_))) | (Val::OptPx(Some(_)), Val::OptPx(Some(_))) => true,
        (Val::MotionPosition(MotionPosition::Coordinates(x,y)), Val::MotionPosition(MotionPosition::Coordinates(u,v))) =>
            lerp_pair(&(*x,*y),&(*u,*v),0.5).is_some(),
        (Val::MotionRotation(a), Val::MotionRotation(b)) => a.follows_path == b.follows_path,
        (Val::Tracks(x), Val::Tracks(y)) => GridOperation::Interpolate(0.5).tracks(x, y).is_some(),
        (Val::Tolerance(Some(x)), Val::Tolerance(Some(y))) => lerp_lp(x, y, 0.5).is_some() && !matches!(x, Length::Px(v) if v.is_infinite()) && !matches!(y, Length::Px(v) if v.is_infinite()),
        (Val::L(x), Val::L(y)) => lerp_lp(x, y, 0.5).is_some(),
        (Val::T(x), Val::T(y)) => lerp_transforms(x, y, 0.5).is_some(),
        (Val::Cp(Some(x), _), Val::Cp(Some(y), _)) => lerp_clip(x, y, 0.5).is_some(),
        _ => false,
    }
}

/// Values of animation `a` at time `now` (base values fill keyframes that
/// do not declare a property).
fn animation_values(a: &Animation, base: &ComputedStyle, now: f64) -> Vec<(Prop, Val)> {
    let Some(local) = a.clock.local(now) else { return Vec::new() };
    let local = local - a.delay;
    let active = match a.iterations {
        Some(n) => a.duration * n,
        None => f64::INFINITY,
    };
    let fill_back = a.fill == "backwards" || a.fill == "both";
    let fill_fwd = a.fill == "forwards" || a.fill == "both";
    let (iter, progress) = if local < 0.0 {
        if !fill_back {
            return Vec::new();
        }
        (0i64, 0.0f64)
    } else if local >= active {
        if !fill_fwd {
            return Vec::new();
        }
        let n = a.iterations.unwrap_or(1.0);
        let whole = n.floor();
        if n > whole {
            (whole as i64, n - whole)
        } else {
            ((whole as i64 - 1).max(0), 1.0)
        }
    } else if a.duration <= 0.0 {
        (0, 1.0)
    } else {
        ((local / a.duration).floor() as i64, (local % a.duration) / a.duration)
    };
    let reverse = match a.direction.as_str() {
        "reverse" => true,
        "alternate" => iter % 2 == 1,
        "alternate-reverse" => iter % 2 == 0,
        _ => false,
    };
    let p = if reverse { 1.0 - progress } else { progress } as f32;
    // Properties any keyframe animates.
    let mut props: Vec<Prop> = Vec::new();
    for f in &a.frames {
        for (pr, _) in &f.values {
            if !props.contains(pr) {
                props.push(*pr);
            }
        }
    }
    let mut out = Vec::new();
    for pr in props {
        // Keyframes declaring pr, with implicit 0% / 100% from the base.
        let ops = base.animation_composition.as_deref().unwrap_or_default();
        let default = if ops.is_empty() { Composite::Replace } else { ops[a.index % ops.len()] };
        let underlying = get(base, pr);
        let mut ks: Vec<(f32, Val)> = a.frames.iter().filter_map(|f| {
            let (_, value) = f.values.iter().find(|(q, _)| *q == pr)?;
            let composed = match f.composite.unwrap_or(default) {
                Composite::Replace => value.clone(),
                Composite::Add | Composite::Accumulate => add_values(&underlying, value),
            };
            Some((f.offset, composed))
        }).collect();
        if ks.first().map(|k| k.0 > 0.0).unwrap_or(true) {
            ks.insert(0, (0.0, underlying.clone()));
        }
        if ks.last().map(|k| k.0 < 1.0).unwrap_or(true) {
            ks.push((1.0, underlying));
        }
        let mut j = 0;
        while j + 1 < ks.len() - 1 && p > ks[j + 1].0 {
            j += 1;
        }
        let (o0, v0) = &ks[j];
        let (o1, v1) = &ks[(j + 1).min(ks.len() - 1)];
        let span = (o1 - o0).max(1e-6);
        let local_t = ((p - o0) / span).clamp(0.0, 1.0);
        out.push((pr, interpolate(v0, v1, ease(&a.easing, local_t))));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_curves() {
        assert!((ease(&parse_easing("linear"), 0.3) - 0.3).abs() < 1e-6);
        let e = parse_easing("ease-in-out");
        assert!((ease(&e, 0.5) - 0.5).abs() < 1e-3);
        assert!(ease(&e, 0.1) < 0.1);
        assert_eq!(parse_easing("cubic-bezier(0.4, 0, 0.2, 1)"), Easing::Bezier(0.4, 0.0, 0.2, 1.0));
        assert_eq!(ease(&parse_easing("steps(4, end)"), 0.3), 0.25);
        assert_eq!(ease(&parse_easing("step-start"), 0.01), 1.0);
        assert_eq!(ease(&parse_easing("steps(3, jump-none)"), 0.5), 0.5);
        assert_eq!(ease(&parse_easing("steps(1, jump-both)"), 0.5), 0.5);
        // Linear-equivalent Béziers stay linear.
        assert!((ease(&parse_easing("cubic-bezier(0, 0, 0, 0)"), 0.25) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn rotations_interpolate_3d() {
        let deg = |d: f32| d.to_radians();
        // Same axis: the angle.
        let t = interpolate(&Val::T(vec![TransformOp::Rotate3d(1.0, 0.0, 0.0, 0.0)]), &Val::T(vec![TransformOp::Rotate3d(2.0, 0.0, 0.0, deg(90.0))]), 0.5);
        let Val::T(ops) = t else { panic!() };
        let TransformOp::Rotate3d(x, y, z, a) = ops[0] else { panic!("{ops:?}") };
        assert!((x - 1.0).abs() < 1e-5 && y == 0.0 && z == 0.0 && (a - deg(45.0)).abs() < 1e-5, "{ops:?}");
        // Different axes: quaternion slerp (CSS Transforms 2 example).
        let t = interpolate(&Val::T(vec![TransformOp::Rotate3d(1.0, 1.0, 0.0, deg(90.0))]), &Val::T(vec![TransformOp::Rotate3d(0.0, 1.0, 1.0, deg(180.0))]), 0.5);
        let Val::T(ops) = t else { panic!() };
        let TransformOp::Rotate3d(x, y, z, a) = ops[0] else { panic!("{ops:?}") };
        let n = (x * x + y * y + z * z).sqrt();
        assert!((x / n - 0.3365568).abs() < 1e-3 && (y / n - 0.8125199).abs() < 1e-3 && (z / n - 0.4759632).abs() < 1e-3, "{ops:?}");
        assert!((a - deg(129.094547)).abs() < 1e-3, "{ops:?}");
        // A whole turn about x is no rotation: rotateX(360deg) → rotateZ(90deg)
        // passes through rotateZ(45deg).
        let t = interpolate(&Val::T(vec![TransformOp::Rotate3d(1.0, 0.0, 0.0, deg(360.0))]), &Val::T(vec![TransformOp::Rotate(deg(90.0))]), 0.5);
        let Val::T(ops) = t else { panic!() };
        let TransformOp::Rotate3d(x, y, z, a) = ops[0] else { panic!("{ops:?}") };
        assert!(x.abs() < 1e-5 && y.abs() < 1e-5 && (z - 1.0).abs() < 1e-5 && (a - deg(45.0)).abs() < 1e-4, "{ops:?}");
    }

    #[test]
    fn matrix3d_interpolates_by_decomposition() {
        let m = |ops: &[TransformOp]| rabbithole_style::transform_matrix_3d(ops, 0.0, 0.0);
        let a = vec![TransformOp::Perspective(Some(100.0)), TransformOp::Translate3d(Length::Px(0.0), Length::Px(0.0), 10.0)];
        let b = vec![TransformOp::Scale3d(2.0, 2.0, 2.0)];
        let Val::T(ops) = interpolate(&Val::T(a), &Val::T(b), 0.0) else { panic!() };
        // Equal up to the homogeneous scale.
        let (r, e) = (m(&ops), m(&[TransformOp::Perspective(Some(100.0)), TransformOp::Translate3d(Length::Px(0.0), Length::Px(0.0), 10.0)]));
        assert!(r.iter().zip(e.iter()).all(|(p, q)| (p / r[15] - q / e[15]).abs() < 1e-4), "{r:?} {e:?}");
    }

    #[test]
    fn interpolation() {
        assert_eq!(interpolate(&Val::F(0.0), &Val::F(1.0), 0.25), Val::F(0.25));
        assert_eq!(interpolate(&Val::L(Length::Px(0.0)), &Val::L(Length::Px(100.0)), 0.5), Val::L(Length::Px(50.0)));
        assert_eq!(interpolate(&Val::L(Length::Auto), &Val::L(Length::Px(100.0)), 0.4), Val::L(Length::Auto));
        let t = interpolate(&Val::T(vec![]), &Val::T(vec![TransformOp::Rotate(1.0)]), 0.5);
        assert_eq!(t, Val::T(vec![TransformOp::Rotate(0.5)]));
        assert_eq!(interpolate(&Val::V(Visibility::Visible), &Val::V(Visibility::Hidden), 0.9), Val::V(Visibility::Visible));
        // Mixed units interpolate through calc().
        let t = interpolate(&Val::T(vec![TransformOp::Translate(Length::Px(50.0), Length::Px(0.0))]), &Val::T(vec![TransformOp::Translate(Length::Percent(50.0), Length::Px(0.0))]), 0.5);
        assert_eq!(t, Val::T(vec![TransformOp::Translate(Length::Calc(25.0, 25.0), Length::Px(0.0))]));
        // Different functions: decomposed matrices, the short way around.
        let Val::T(ops) = interpolate(&Val::T(vec![TransformOp::Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])]), &Val::T(vec![TransformOp::Rotate(std::f32::consts::FRAC_PI_2)]), 0.5) else { panic!() };
        let TransformOp::Matrix(m) = ops[0] else { panic!("{ops:?}") };
        let h = std::f32::consts::FRAC_1_SQRT_2;
        for (x, y) in m.iter().zip([h, h, -h, h, 0.0, 0.0]) {
            assert!((x - y).abs() < 1e-5, "{m:?}");
        }
        // Lists padded with identity functions.
        let t = interpolate(&Val::T(vec![TransformOp::Scale(2.0, 2.0)]), &Val::T(vec![TransformOp::Scale(4.0, 4.0), TransformOp::Rotate(1.0)]), 0.5);
        assert_eq!(t, Val::T(vec![TransformOp::Scale(3.0, 3.0), TransformOp::Rotate(0.5)]));
    }

    #[test]
    fn script_keyframe_intervals_and_composite() {
        let base = ComputedStyle::initial();
        let kf = |offset: f32, v: f32, easing: Easing, composite: Option<Composite>| ScriptKeyframe { offset, easing, composite, values: vec![(Prop::Opacity, Val::F(v))] };
        let frames = vec![kf(0.0, 0.0, Easing::Linear, None), kf(0.5, 1.0, Easing::Steps(1, false, 0), None), kf(1.0, 0.0, Easing::Linear, None)];
        let at = |p: f64, c: Composite| script_effect_values(&frames, p, c, &base)[0].1.clone();
        assert_eq!(at(0.25, Composite::Replace), Val::F(0.5));
        // steps(1) on the second interval holds its start value.
        assert_eq!(at(0.75, Composite::Replace), Val::F(1.0));
        assert_eq!(at(1.0, Composite::Replace), Val::F(0.0));
        // Extrapolated below 0 from the first interval.
        assert_eq!(at(-0.5, Composite::Replace), Val::F(-1.0));
        // add: composited with the underlying value (opacity 1).
        assert_eq!(at(0.25, Composite::Add), Val::F(1.5));
        // A missing 0% keyframe takes the underlying value.
        let single = vec![kf(1.0, 0.0, Easing::Linear, None)];
        assert_eq!(script_effect_values(&single, 0.5, Composite::Replace, &base)[0].1, Val::F(0.5));
    }
}
