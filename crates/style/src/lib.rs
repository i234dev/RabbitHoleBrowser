//! Style resolution: the user-agent stylesheet, value parsing, the cascade
//! (origin, importance, specificity, order) and inheritance, producing a
//! `ComputedStyle` per element.

pub mod cascade;
pub mod computed;
pub mod ua;
pub mod values;

pub use computed::*;
pub use rabbithole_css::{MediaEnv, Stylesheet};

use rabbithole_dom::{Document, NodeId};
use std::rc::Rc;

/// Styles indexed by `NodeId`. Non-element nodes and `display: none`
/// subtrees have `None`.
pub struct StyledTree {
    pub styles: Vec<Option<Rc<ComputedStyle>>>,
}

impl StyledTree {
    pub fn get(&self, id: NodeId) -> Option<&Rc<ComputedStyle>> {
        self.styles.get(id).and_then(|s| s.as_ref())
    }
}

/// Resolve styles for the whole document.
///
/// `author` sheets are applied in order after the UA sheet; `style`
/// attributes and presentational attributes are handled internally.
pub fn style_tree(doc: &Document, author: &[Stylesheet], env: &MediaEnv) -> StyledTree {
    cascade::style_tree(doc, author, env)
}
