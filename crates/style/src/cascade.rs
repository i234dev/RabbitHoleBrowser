//! The cascade.
use crate::{MediaEnv, StyledTree, Stylesheet};
use rabbithole_dom::Document;

pub fn style_tree(doc: &Document, author: &[Stylesheet], env: &MediaEnv) -> StyledTree {
    let _ = (doc, author, env);
    unimplemented!()
}
