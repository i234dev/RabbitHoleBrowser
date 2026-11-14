//! Realize a finite item width without promoting an estimate to used tracks
//! or a definite percentage basis. No legacy layout caller uses this yet.
use super::phases::AvailableSpace;
use rabbithole_style::{BoxSizing, ComputedStyle, Length};
use std::rc::Rc;
use super::{percentages,phases::{GridAxis,OperationKind},subgrid::{self,WorkBudget}};
use crate::tree::{BoxKind,LayoutBox};
use crate::block::{Cb,Layouter,WidthMode};
use crate::flex::{Axis,ItemEdges};
use crate::float::Bfc;
use rabbithole_style::ItemAlign;

/// Normalize the measurement node and its independently styled wrapper
/// chain. Local spaces/axis swap at each wrapper, while percentage edges
/// retain the containing block's original inline base. Normal descendants
/// are laid out within this node's subsequently realized content width.
pub(super) struct Scope<'b> {
    pub(super) node:&'b mut LayoutBox,
    saved:Vec<Rc<ComputedStyle>>,
}
impl<'b> Scope<'b> {
    pub(super) fn new(node:&'b mut LayoutBox,mut spaces:[AvailableSpace;2],edge_basis:AvailableSpace,mut contribution:GridAxis,kind:OperationKind,budget:&mut WorkBudget)->Self {
        let mut current=&*node;
        loop {
            budget.charge(1);
            if !current.wm_wrapper {break;}
            let Some(inner)=current.children.first() else {break;};current=inner;
        }
        budget.subtree(node);
        let mut scope=Self{node,saved:Vec::new()};
        let mut current=&mut *scope.node;
        loop {
            scope.saved.push(current.style.clone());
            let mut style=(*current.style).clone();
            percentages::normalize(&mut style,spaces,edge_basis,contribution,kind,current.kind==BoxKind::Image);
            percentages::freeze_edges(&mut style,edge_basis);
            current.style=Rc::new(style);
            if !current.wm_wrapper {break;}
            let Some(inner)=current.children.first_mut() else {break;};
            current=inner;spaces.swap(0,1);contribution=contribution.opposite();
        }
        subgrid::invalidate(scope.node);
        scope
    }
}
impl Drop for Scope<'_> {
    fn drop(&mut self) {
        let mut current=&mut *self.node;
        for original in self.saved.drain(..) {
            current.style=original;
            if !current.wm_wrapper {break;}
            let Some(inner)=current.children.first_mut() else {break;};current=inner;
        }
        subgrid::invalidate(self.node);
    }
}

#[derive(Clone,Copy,Debug)]
pub(super) struct FiniteInline {
    content:f32,
    available:AvailableSpace,
}
impl FiniteInline {
    pub(super) fn content(self)->f32 {self.content}
    pub(super) fn available(self)->AvailableSpace {self.available}
    pub(super) fn percentage_basis(self)->Option<f32> {self.available.definite()}
}

pub(super) struct RowItemEstimate {
    pub(super) inline:FiniteInline,
    pub(super) outer_block:f32,
    pub(super) baseline:f32,
}

impl<'a> Layouter<'a> {
    /// Measure an ordinary item in the row pass without completing an
    /// unresolved perpendicular Grid axis. The per-axis traversal must
    /// intercept subgrid descendants before reaching this ordinary path.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn estimate_row_item(&mut self,child:&mut LayoutBox,item:&super::GridItem,spaces:[AvailableSpace;2],edge_basis:AvailableSpace,kind:OperationKind,old_reference:f32)->RowItemEstimate {
        assert_eq!(subgrid::requests(child),(false,false),"Subgrid measurement must remain in the parent phase");
        let original=ItemEdges::new(&child.style,old_reference);
        let scope=Scope::new(child,spaces,edge_basis,GridAxis::Rows,kind,&mut self.grid_work);
        let style=scope.node.style.clone();
        let mut edges=ItemEdges::new(&style,edge_basis.definite().unwrap_or(0.));
        // The item's hypothetical inherited layer is not part of its own
        // computed margins. Preserve it separately while normalizing %.
        edges.margin.left+=item.edges.margin.left-original.margin.left;
        edges.margin.right+=item.edges.margin.right-original.margin.right;
        edges.margin.top+=item.edges.margin.top-original.margin.top;
        edges.margin.bottom+=item.edges.margin.bottom-original.margin.bottom;
        let content=self.intrinsic(scope.node);
        let stretch=style.width.is_auto()&&matches!(item.justify,ItemAlign::Stretch|ItemAlign::Normal|ItemAlign::Auto)&&!edges.auto.left&&!edges.auto.right;
        let inline=realize(&style,spaces[0],content,edges.pb_h,edges.margin_sum(Axis::Row),stretch);
        // Only execution uses a finite proxy; own cyclic preferred/min/max
        // lengths and percentage edges were normalized in the scope above.
        let proxy=inline.percentage_basis().unwrap_or_else(||(inline.content()+edges.pb_h+edges.margin_sum(Axis::Row)).max(0.));
        let cb=Cb{x:0.,width:proxy,height:spaces[1].definite()};
        let mut bfc=Bfc::new();self.forced_content_height=None;
        self.layout_block(scope.node,&cb,0.,&mut bfc,WidthMode::Fixed(inline.content()));
        RowItemEstimate{inline,outer_block:scope.node.rect.h+edges.margin_sum(Axis::Column),baseline:Self::item_baseline(scope.node)}
    }
}

/// `style` must already have the percentage policy for this measurement
/// applied. Even so, numeric resolution uses only the original definite
/// space, never the subsequently realized content width. The result sizes
/// an item for measurement; it is not a CompletedAxis.
pub(super) fn realize(style:&ComputedStyle,available:AvailableSpace,content:(f32,f32),padding_border:f32,margins:f32,stretch:bool)->FiniteInline {
    assert!(content.0.is_finite()&&content.1.is_finite()&&content.1>=content.0,"Invalid intrinsic Grid item size");
    let reference=available.definite();
    let inner=|value:f32|if style.box_sizing==BoxSizing::BorderBox {(value-padding_border).max(0.)} else {value.max(0.)};
    let room=reference.map(|size|(size-padding_border-margins).max(0.));
    let fit=|min:f32,max:f32|match available {
        AvailableSpace::MinContent=>min,
        _=>room.map_or(max,|room|crate::block::shrink_to_fit(min,max,room)),
    };
    let size=if let Some(size)=style.width.resolve_definite(reference) {inner(size)} else {
        match style.width {
            Length::MinContent=>content.0,
            Length::MaxContent=>content.1,
            Length::FitContent=>fit(content.0,content.1),
            _=>match available {
                AvailableSpace::MinContent=>content.0,
                AvailableSpace::MaxContent|AvailableSpace::Indefinite=>content.1,
                AvailableSpace::Definite(_)=>if stretch {room.unwrap()} else {fit(content.0,content.1)},
            },
        }
    };
    let bound=|value:Length|match value {
        Length::MinContent=>Some(content.0),
        Length::MaxContent=>Some(content.1),
        Length::FitContent=>Some(fit(content.0,content.1)),
        _=>value.resolve_definite(reference).map(inner),
    };
    let max=bound(style.max_width);
    let min=bound(style.min_width).unwrap_or(0.);
    let size=max.map_or(size,|max|size.min(max)).max(min).max(0.);
    assert!(size.is_finite(),"Non-finite Grid item estimate");
    FiniteInline{content:size,available}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{catch_unwind,AssertUnwindSafe};
    use std::collections::HashMap;
    use rabbithole_gfx::FontContext;
    use rabbithole_css::{parse_stylesheet,MediaEnv};

    fn find_mut(node:&mut LayoutBox,id:usize)->Option<&mut LayoutBox> {
        if node.node==Some(id)&&node.kind!=BoxKind::Text {return Some(node);}
        node.children.iter_mut().find_map(|child|find_mut(child,id))
    }
    fn find(node:&LayoutBox,id:usize)->Option<&LayoutBox> {
        if node.node==Some(id)&&node.kind!=BoxKind::Text {return Some(node);}
        node.children.iter().find_map(|child|find(child,id))
    }

    /// The expected rectangle comes from ordinary Block layout of a separate
    /// reference with an explicit used size; no Grid/Flex oracle is reused.
    fn measured_item(css:&str,inline:AvailableSpace,edge_basis:AvailableSpace)->(RowItemEstimate,(f32,f32)) {
        let doc=rabbithole_html::parse("<div id=probe>XXXX XX</div><div id=reference>XXXX XX</div>");
        let sheets=[parse_stylesheet(&format!("*{{font:10px/1 monospace}}div{{display:block;min-width:0}}{css}"))];
        let styles=rabbithole_style::style_tree(&doc,&sheets,&MediaEnv{width:800.,height:600.,screen:true,scripting:true});
        let fonts=FontContext::new();let images=HashMap::new();let backgrounds=HashMap::new();let scrolls=HashMap::new();
        let ctx=crate::LayoutContext{doc:&doc,styles:&styles,fonts:&fonts,images:&images,background_images:&backgrounds,scroll_offsets:&scrolls,viewport_width:800.,viewport_height:600.,page_scroll:(0.,0.)};
        let reference=crate::layout(&ctx);
        let reference=find(&reference.root,doc.element_by_id("reference").unwrap()).unwrap();
        let expected=(reference.content_rect.w,reference.rect.h);
        let mut root=crate::build::Builder::new(&ctx,None).build_root();
        let node=find_mut(&mut root,doc.element_by_id("probe").unwrap()).unwrap();
        let original=node.style.clone();
        let item=super::super::GridItem{idx:0,area:super::super::Area{row:(0,1),col:(0,1)},edges:ItemEdges::new(&node.style,800.),justify:ItemAlign::Stretch,align:ItemAlign::FlexStart,column_names:true,row_names:true,parent_lanes_auto:false};
        let mut layouter=Layouter::new(&ctx);
        let measured=layouter.estimate_row_item(node,&item,[inline,AvailableSpace::Indefinite],edge_basis,OperationKind::Layout,800.);
        assert!(Rc::ptr_eq(&node.style,&original));assert!(node.intrinsic_cache.is_none());
        (measured,expected)
    }

    #[test]
    fn infinite_auto_realization_is_finite_and_keeps_the_original_percentage_tag() {
        let style=ComputedStyle::initial();
        let realized=realize(&style,AvailableSpace::Indefinite,(30.,80.),0.,0.,true);
        assert_eq!(realized.content(),80.);assert_eq!(realized.available(),AvailableSpace::Indefinite);
        assert_eq!(realized.percentage_basis(),None);
        let zero=realize(&style,AvailableSpace::Definite(0.),(30.,80.),0.,0.,true);
        assert_eq!(zero.content(),0.);assert_eq!(zero.percentage_basis(),Some(0.));
    }

    #[test]
    fn preferred_keywords_and_definite_sizes_override_an_infinite_estimate() {
        for (preferred,expected) in [(Length::MinContent,30.),(Length::MaxContent,80.),(Length::FitContent,80.),(Length::Px(55.),55.)] {
            let mut style=ComputedStyle::initial();style.width=preferred;
            assert_eq!(realize(&style,AvailableSpace::Indefinite,(30.,80.),0.,0.,true).content(),expected);
        }
    }

    #[test]
    fn definite_fit_and_stretch_do_not_share_the_max_content_fallback() {
        let mut style=ComputedStyle::initial();style.width=Length::FitContent;
        assert_eq!(realize(&style,AvailableSpace::Definite(63.),(30.,80.),8.,5.,true).content(),50.);
        assert_eq!(realize(&style,AvailableSpace::MinContent,(30.,80.),8.,5.,true).content(),30.);
        style.width=Length::Auto;
        assert_eq!(realize(&style,AvailableSpace::Definite(150.),(30.,80.),8.,5.,true).content(),137.);
        assert_eq!(realize(&style,AvailableSpace::Definite(150.),(30.,80.),8.,5.,false).content(),80.);
        assert_eq!(realize(&style,AvailableSpace::MinContent,(30.,80.),8.,5.,true).content(),30.);
    }

    #[test]
    fn cyclic_min_calc_preserves_its_absolute_floor_without_using_the_finite_proxy() {
        let mut style=ComputedStyle::initial();style.width=Length::Percent(50.);style.min_width=Length::Calc(90.,20.);style.max_width=Length::Percent(25.);
        percentages::normalize(&mut style,[AvailableSpace::Indefinite,AvailableSpace::Indefinite],AvailableSpace::Indefinite,GridAxis::Rows,OperationKind::Layout,false);
        let realized=realize(&style,AvailableSpace::Indefinite,(30.,80.),0.,0.,false);
        assert_eq!(realized.content(),90.);assert_eq!(realized.percentage_basis(),None);
        // A real base keeps percentages and yields a different used width.
        let mut defined=ComputedStyle::initial();defined.width=Length::Percent(50.);
        assert_eq!(realize(&defined,AvailableSpace::Definite(200.),(30.,80.),0.,0.,false).content(),100.);
    }

    #[test]
    fn numeric_border_box_bounds_subtract_edges_once_and_minimum_wins_conflicts() {
        let mut style=ComputedStyle::initial();style.box_sizing=BoxSizing::BorderBox;
        style.width=Length::Px(60.);style.min_width=Length::Px(90.);style.max_width=Length::Px(70.);
        assert_eq!(realize(&style,AvailableSpace::Indefinite,(30.,80.),10.,4.,false).content(),80.);
    }

    #[test]
    fn intrinsic_minimum_and_maximum_keywords_do_not_disappear_in_a_numeric_proxy() {
        let mut style=ComputedStyle::initial();style.width=Length::Px(55.);style.min_width=Length::MaxContent;
        assert_eq!(realize(&style,AvailableSpace::Indefinite,(30.,80.),10.,4.,false).content(),80.);
        style.min_width=Length::Auto;style.max_width=Length::MinContent;
        assert_eq!(realize(&style,AvailableSpace::Indefinite,(30.,80.),10.,4.,false).content(),30.);
        style.width=Length::MaxContent;style.max_width=Length::FitContent;
        assert_eq!(realize(&style,AvailableSpace::Definite(64.),(30.,80.),10.,4.,false).content(),50.);
    }

    #[test]
    fn estimate_scope_restores_styles_and_invalidates_proxy_caches_on_unwind() {
        let mut style=ComputedStyle::initial();style.width=Length::Percent(50.);style.padding.left=Length::Percent(10.);
        let original=Rc::new(style);let mut node=LayoutBox::new(BoxKind::Block,None,original.clone());
        node.intrinsic_cache=Some((10.,20.));let mut budget=WorkBudget::default();
        let result=catch_unwind(AssertUnwindSafe(|| {
            let scope=Scope::new(&mut node,[AvailableSpace::Indefinite,AvailableSpace::Indefinite],AvailableSpace::Definite(300.),GridAxis::Rows,OperationKind::Layout,&mut budget);
            assert_eq!(scope.node.style.width,Length::Auto);assert_eq!(scope.node.style.padding.left,Length::Px(30.));
            scope.node.intrinsic_cache=Some((100.,100.));
            panic!("intentional estimate abort");
        }));
        assert!(result.is_err());assert!(Rc::ptr_eq(&node.style,&original));assert!(node.intrinsic_cache.is_none());
        assert_eq!(node.style.width,Length::Percent(50.));assert_eq!(node.style.padding.left,Length::Percent(10.));
    }

    #[test]
    fn estimate_scope_precharges_subtree_before_installing_any_style() {
        let original=Rc::new(ComputedStyle::initial());let mut node=LayoutBox::new(BoxKind::Block,None,original.clone());
        node.children.push(LayoutBox::new(BoxKind::Block,None,original.clone()));
        let mut budget=WorkBudget::limited_for_test(1);
        let result=catch_unwind(AssertUnwindSafe(|| {
            let _scope=Scope::new(&mut node,[AvailableSpace::Indefinite,AvailableSpace::Indefinite],AvailableSpace::Indefinite,GridAxis::Rows,OperationKind::Layout,&mut budget);
        }));
        assert!(result.is_err());assert!(Rc::ptr_eq(&node.style,&original));
    }

    #[test]
    fn estimated_item_layout_uses_a_finite_auto_width_but_not_as_its_own_percentage_base() {
        let (measured,expected)=measured_item("#probe{width:50%}#reference{width:max-content}",AvailableSpace::Indefinite,AvailableSpace::Indefinite);
        assert!((measured.inline.content()-expected.0).abs()<0.02);
        assert!((measured.outer_block-expected.1).abs()<0.02);
        assert_eq!(measured.inline.percentage_basis(),None);
    }

    #[test]
    fn estimated_item_layout_preserves_actual_percentage_width_and_independent_edge_bases() {
        let (measured,expected)=measured_item("#probe{width:50%}#reference{width:40px}",AvailableSpace::Definite(80.),AvailableSpace::Definite(80.));
        assert!((measured.inline.content()-expected.0).abs()<0.02);
        assert!((measured.outer_block-expected.1).abs()<0.02);
        assert_eq!(measured.inline.percentage_basis(),Some(80.));
        let (measured,expected)=measured_item("#probe{width:50%;padding-left:10%}#reference{width:max-content;padding-left:30px}",AvailableSpace::Indefinite,AvailableSpace::Definite(300.));
        assert!((measured.inline.content()-expected.0).abs()<0.02);
        assert!((measured.outer_block-expected.1).abs()<0.02);
    }
}
