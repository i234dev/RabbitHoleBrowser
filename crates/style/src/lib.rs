//! Style resolution: the user-agent stylesheet, value parsing, the cascade
//! (origin, importance, specificity, order) and inheritance, producing a
//! `ComputedStyle` per element (and per `::before` / `::after` / `::marker`
//! pseudo-element), plus `@font-face` and `@keyframes` collection.

pub mod cascade;
pub mod computed;
pub mod props;
#[cfg(test)]
mod tests_props;
pub mod ua;
pub mod values;

pub use computed::*;
pub use rabbithole_css::{Declaration, MatchState, MediaEnv, PseudoElement, Stylesheet};

use rabbithole_dom::{Document, NodeId};
use std::collections::HashMap;
use std::rc::Rc;

/// One `src` entry of an `@font-face` rule.
#[derive(Debug, Clone, PartialEq)]
pub struct FontSource {
    /// Absolute URL (resolved through `StyleOptions::resolve_url`).
    pub url: String,
    /// `format("woff2")` hint, lower-cased.
    pub format: Option<String>,
}

/// A collected `@font-face` rule.
#[derive(Debug, Clone, PartialEq)]
pub struct FontFace {
    pub family: String,
    pub sources: Vec<FontSource>,
    /// Weight range (min, max); a single weight has min == max.
    pub weight: (u16, u16),
    pub italic: bool,
    /// `font-display` (`auto` when unspecified).
    pub display: String,
    /// `unicode-range` as inclusive code point ranges.
    pub unicode_range: Option<Vec<(u32, u32)>>,
}

/// Styles indexed by `NodeId`. Non-element nodes and `display: none`
/// subtrees have `None`.
pub struct StyledTree {
    pub styles: Vec<Option<Rc<ComputedStyle>>>,
    /// `::before` styles; `Some` only when a rule matched and `content` is
    /// neither `normal` nor `none`.
    pub before: Vec<Option<Rc<ComputedStyle>>>,
    /// `::after` styles (same rule as `before`).
    pub after: Vec<Option<Rc<ComputedStyle>>>,
    /// `::marker` styles; `Some` when any `::marker` rule matched.
    pub marker: Vec<Option<Rc<ComputedStyle>>>,
    pub font_faces: Vec<FontFace>,
    /// `@keyframes` by name: (offsets, declarations) per frame, raw.
    pub keyframes: HashMap<String, Vec<(Vec<f32>, Vec<Declaration>)>>,
    /// Computed font size of the root element (px).
    pub root_font_size: f32,
}

impl StyledTree {
    pub fn get(&self, id: NodeId) -> Option<&Rc<ComputedStyle>> {
        self.styles.get(id).and_then(|s| s.as_ref())
    }

    /// Style of a generated pseudo-element of `id`, if it generates a box.
    pub fn pseudo(&self, id: NodeId, which: PseudoElement) -> Option<&Rc<ComputedStyle>> {
        let v = match which {
            PseudoElement::Before => &self.before,
            PseudoElement::After => &self.after,
            PseudoElement::Marker => &self.marker,
            _ => return None,
        };
        v.get(id).and_then(|s| s.as_ref())
    }

    pub(crate) fn empty() -> StyledTree {
        StyledTree {
            styles: Vec::new(),
            before: Vec::new(),
            after: Vec::new(),
            marker: Vec::new(),
            font_faces: Vec::new(),
            keyframes: HashMap::new(),
            root_font_size: 16.0,
        }
    }
}

/// Extra inputs for style resolution.
pub struct StyleOptions<'a> {
    /// Dynamic state for `:hover` / `:focus` / `:active` / `:target`.
    pub state: &'a MatchState,
    /// `(base, raw_url) -> absolute_url`, where `base` is the stylesheet's
    /// `base_url` (or `document_base` for inline styles). Used for
    /// `url()` values in `background`, `content`, `@font-face src`, ...
    pub resolve_url: &'a dyn Fn(&str, &str) -> Option<String>,
    /// Absolute URL of the document.
    pub document_base: &'a str,
}

fn no_resolve(_base: &str, raw: &str) -> Option<String> {
    Some(raw.to_string())
}

/// Resolve styles for the whole document (no dynamic state, URLs kept as
/// written).
pub fn style_tree(doc: &Document, author: &[Stylesheet], env: &MediaEnv) -> StyledTree {
    let state = MatchState::default();
    let opts = StyleOptions {
        state: &state,
        resolve_url: &no_resolve,
        document_base: "",
    };
    cascade::style_tree_with(doc, author, env, &opts)
}

/// Resolve styles with dynamic state and URL resolution.
pub fn style_tree_with(doc: &Document, author: &[Stylesheet], env: &MediaEnv, opts: &StyleOptions) -> StyledTree {
    cascade::style_tree_with(doc, author, env, opts)
}

/// Style of one element computed on demand (see `cascade::style_chain`).
pub fn style_chain(doc: &Document, author: &[Stylesheet], env: &MediaEnv, opts: &StyleOptions, node: NodeId) -> Option<Rc<ComputedStyle>> {
    cascade::style_chain(doc, author, env, opts, node)
}
