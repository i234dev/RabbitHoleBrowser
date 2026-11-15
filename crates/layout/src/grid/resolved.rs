//! Final-layout metadata for CSSOM. Measurement does not publish a result.
use std::{cell::Cell, rc::Rc};
use rabbithole_style::{TrackListItem, UsedGridTrackList};
use crate::tree::LayoutBox;
use super::{GridSpace, TrackList, subgrid};

#[derive(Default)]
pub(crate) struct Publication { depth: Option<Rc<Cell<usize>>> }
pub(crate) struct Probe { depth: Rc<Cell<usize>> }
impl Publication {
    pub(crate) fn enabled(&self) -> bool { self.depth.as_ref().is_none_or(|d| d.get() == 0) }
    pub(crate) fn probe(&mut self) -> Probe {
        let depth = self.depth.get_or_insert_with(|| Rc::new(Cell::new(0)));
        let previous = depth.get();
        depth.set(previous.checked_add(1).expect("Grid probe depth overflow"));
        Probe { depth: depth.clone() }
    }
}
impl Drop for Probe { fn drop(&mut self) { self.depth.set(self.depth.get()-1); } }

pub(super) fn capture(node: &LayoutBox, rows: bool, list: &TrackList, space: GridSpace, budget: &mut subgrid::WorkBudget) -> Option<Rc<UsedGridTrackList>> {
    if space == GridSpace::LegacyIndefinite { return None; }
    budget.tracks(list);
    let template = if rows { &node.style.grid_template_rows } else { &node.style.grid_template_columns };
    let value = if list.inherited.is_some() {
        let local = match template.first() { Some(TrackListItem::Subgrid { line_names }) => line_names.as_slice(), _ => &[] };
        UsedGridTrackList::Subgrid { line_names: subgrid::local_names(local, list.tracks.len()+1) }
    } else {
        UsedGridTrackList::Standalone {
            sizes: list.tracks.iter().map(|t| t.base).collect(),
            line_names: list.css_names.as_ref().clone(),
        }
    };
    Some(Rc::new(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_probe_suppression_restores_on_return_and_unwind() {
        let mut state = Publication::default();
        let outer = state.probe(); assert!(!state.enabled());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _inner = state.probe(); assert!(!state.enabled()); panic!("probe");
        }));
        assert!(result.is_err()); assert!(!state.enabled());
        drop(outer); assert!(state.enabled());
    }

    #[test]
    fn a_measurement_cache_cannot_publish_in_place_of_final_layout() {
        use crate::{block::{Cb, Layouter, WidthMode}, float::Bfc, LayoutContext};
        use std::collections::HashMap;
        let doc = rabbithole_html::parse("<div id=g><i></i></div>");
        let sheets = [rabbithole_css::parse_stylesheet("*{margin:0;padding:0;font-size:0;line-height:0}#g{display:grid;grid-template-columns:20px 30px;grid-template-rows:10px}")];
        let styles = rabbithole_style::style_tree(&doc, &sheets, &rabbithole_css::MediaEnv { width:100., height:100., screen:true, scripting:true });
        let fonts = rabbithole_gfx::FontContext::new(); let images = HashMap::new(); let backgrounds = HashMap::new(); let scrolls = HashMap::new();
        let ctx = LayoutContext { doc:&doc, styles:&styles, fonts:&fonts, images:&images, background_images:&backgrounds, scroll_offsets:&scrolls, viewport_width:100., viewport_height:100., page_scroll:(0.,0.) };
        let mut root = crate::build::Builder::new(&ctx, None).build_root();
        fn find(node:&mut LayoutBox, id:usize)->Option<&mut LayoutBox> {
            if node.node==Some(id) && node.kind!=crate::BoxKind::Text { return Some(node); }
            node.children.iter_mut().find_map(|child| find(child,id))
        }
        let node = find(&mut root, doc.element_by_id("g").unwrap()).unwrap();
        let mut layouter = Layouter::new(&ctx);
        let _ = layouter.intrinsic(node);
        assert!(node.used_grid_tracks.iter().all(Option::is_none));
        let cb = Cb { x:0., width:50., height:Some(10.) };
        let probe = layouter.grid_publication.probe();
        layouter.layout_block(node, &cb, 0., &mut Bfc::new(), WidthMode::Fixed(50.));
        assert!(node.used_grid_tracks.iter().all(Option::is_none));
        assert!(!node.layout_cache.as_ref().unwrap().grid_publication);
        drop(probe);
        layouter.layout_block(node, &cb, 0., &mut Bfc::new(), WidthMode::Fixed(50.));
        assert_eq!(node.used_grid_tracks[0].as_deref(), Some(&UsedGridTrackList::Standalone { sizes:vec![20.,30.], line_names:vec![vec![],vec![],vec![]] }));
        assert!(node.layout_cache.as_ref().unwrap().grid_publication);
        let completed = node.used_grid_tracks[0].as_ref().unwrap().clone();
        layouter.layout_block(node, &cb, 0., &mut Bfc::new(), WidthMode::Fixed(50.));
        assert!(Rc::ptr_eq(&completed, node.used_grid_tracks[0].as_ref().unwrap()));
    }

    #[test]
    fn legacy_indefinite_is_not_a_completed_cssom_axis() {
        let node = LayoutBox::new(crate::BoxKind::Block, None, Rc::new(rabbithole_style::ComputedStyle::initial()));
        let list = super::super::expand_tracks(&[], None, 0.);
        let mut budget = subgrid::WorkBudget::limited_for_test(0);
        assert!(capture(&node, false, &list, GridSpace::LegacyIndefinite, &mut budget).is_none());
    }
}
