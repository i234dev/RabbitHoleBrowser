//! Highlight pseudo-element styles (`::selection`) for painting selected
//! text (CSS Pseudo-Elements 4 §3).
//!
//! They are computed on demand, only for the elements that own selected
//! text, so a page pays nothing for them while nothing is selected.

use crate::computed::{Color, ComputedStyle, Shadow, TextDecoration, TextDecorationStyle};
use rabbithole_dom::NodeId;
use std::collections::HashMap;
use std::rc::Rc;

/// The resolved highlight style of one originating element: `None` fields
/// were not specified by any `::selection` rule along the highlight
/// inheritance chain (or, for non-inherited properties, on the element).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HighlightStyle {
    pub color: Option<Color>,
    pub background: Option<Color>,
    pub text_fill: Option<Color>,
    pub shadows: Option<Vec<Shadow>>,
    /// `text-decoration` of the highlight: lines, color, style.
    pub decoration: Option<(TextDecoration, Option<Color>, TextDecorationStyle)>,
}

impl HighlightStyle {
    /// No `::selection` rule applies: the UA default colors are used.
    pub fn is_unstyled(&self) -> bool {
        self.color.is_none() && self.background.is_none() && self.text_fill.is_none() && self.shadows.is_none() && self.decoration.is_none()
    }
}

/// `::selection` styles by originating element. Elements absent from
/// `by_element` (or every element, when `any` is false) use the defaults.
#[derive(Debug, Clone, Default)]
pub struct SelectionStyles {
    pub any: bool,
    pub by_element: HashMap<NodeId, Rc<HighlightStyle>>,
}

impl SelectionStyles {
    pub fn get(&self, element: NodeId) -> Option<&HighlightStyle> {
        if !self.any {
            return None;
        }
        self.by_element.get(&element).map(|s| &**s).filter(|s| !s.is_unstyled())
    }
}

/// Default selection colors (the `Highlight` / `HighlightText` system
/// colors).
pub fn default_selection_colors() -> (Color, Color) {
    (crate::color_spaces::system_color("highlight").unwrap_or(Color::rgb(181, 213, 255)), crate::color_spaces::system_color("highlighttext").unwrap_or(Color::rgb(0, 0, 0)))
}

/// Resolve one element's highlight style from its matched `::selection`
/// style (`own`: style and declared property names) and its parent's
/// resolved highlight style.
pub(crate) fn resolve(own: Option<&(ComputedStyle, Vec<String>)>, parent: Option<&HighlightStyle>) -> HighlightStyle {
    let inherited = parent.cloned().unwrap_or_default();
    let Some((style, names)) = own else {
        // Only the inherited properties carry over.
        return HighlightStyle { color: inherited.color, text_fill: inherited.text_fill, shadows: inherited.shadows, background: None, decoration: None };
    };
    let has = |pred: &dyn Fn(&str) -> bool| names.iter().any(|n| pred(n) || n == "all");
    HighlightStyle {
        color: if has(&|n| n == "color") { Some(style.color) } else { inherited.color },
        background: if has(&|n| n == "background" || n == "background-color") { Some(style.background_color) } else { None },
        text_fill: if has(&|n| n == "-webkit-text-fill-color") { style.text_fill_color.or(Some(style.color)) } else { inherited.text_fill },
        shadows: if has(&|n| n == "text-shadow") { Some(style.text_shadow.clone()) } else { inherited.shadows },
        decoration: if has(&|n| n.starts_with("text-decoration")) { Some((style.text_decoration, style.text_decoration_color, style.text_decoration_style)) } else { None },
    }
}
