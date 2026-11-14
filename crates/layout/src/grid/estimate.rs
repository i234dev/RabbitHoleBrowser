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
    pub(super) fn new(node:&'b mut LayoutBox,mut bases:[AvailableSpace;2],edge_basis:AvailableSpace,mut contribution:GridAxis,kind:OperationKind,budget:&mut WorkBudget)->Self {
        Self::map(node,budget,|style,box_kind,wrapper| {
            percentages::normalize(style,bases,edge_basis,contribution,kind,box_kind==BoxKind::Image);
            percentages::freeze_edges(style,edge_basis);
            if wrapper {bases.swap(0,1);contribution=contribution.opposite();}
        })
    }
    fn inline_content(node:&'b mut LayoutBox,budget:&mut WorkBudget)->Self {
        let mut axis=GridAxis::Columns;
        Self::map(node,budget,|style,_,wrapper| {
            if axis==GridAxis::Columns {
                style.width=Length::Auto;style.min_width=Length::ZERO;style.max_width=Length::Auto;
            } else {
                style.height=Length::Auto;style.min_height=Length::ZERO;style.max_height=Length::Auto;
            }
            if wrapper {axis=axis.opposite();}
        })
    }
    fn map(node:&'b mut LayoutBox,budget:&mut WorkBudget,mut prepare:impl FnMut(&mut ComputedStyle,BoxKind,bool))->Self {
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
            prepare(&mut style,current.kind,current.wm_wrapper);
            current.style=Rc::new(style);
            if !current.wm_wrapper {break;}
            let Some(inner)=current.children.first_mut() else {break;};
            current=inner;
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
    percentage_basis:Option<f32>,
}
impl FiniteInline {
    pub(super) fn content(self)->f32 {self.content}
    pub(super) fn available(self)->AvailableSpace {self.available}
    pub(super) fn percentage_basis(self)->Option<f32> {self.percentage_basis}
}

pub(super) struct RowItemEstimate {
    pub(super) inline:FiniteInline,
    pub(super) outer_block:f32,
    pub(super) baseline:f32,
}

fn vertical_edges(style:&ComputedStyle)->f32 {
    style.padding.top.resolve_or_zero(0.)+style.padding.bottom.resolve_or_zero(0.)+style.border_width.top+style.border_width.bottom
}

fn image_height_value(style:&ComputedStyle,value:Length,basis:AvailableSpace,padding_border:f32,intrinsic:f32)->Option<f32> {
    match value {
        Length::Auto=>None,
        Length::MinContent|Length::MaxContent|Length::FitContent=>Some(intrinsic),
        value=>value.resolve_definite(basis.definite()).map(|value|if style.box_sizing==BoxSizing::BorderBox {(value-padding_border).max(0.)} else {value.max(0.)}),
    }
}

fn clamp_image_height(style:&ComputedStyle,height:f32,basis:AvailableSpace,padding_border:f32,intrinsic:f32)->f32 {
    let max=image_height_value(style,style.max_height,basis,padding_border,intrinsic);
    let min=image_height_value(style,style.min_height,basis,padding_border,intrinsic).unwrap_or(0.);
    max.map_or(height,|max|height.min(max)).max(min).max(0.)
}

/// Keep explicit aspect-ratio's box-sizing convention distinct from a
/// natural image ratio, which relates content-box dimensions.
struct ImageMetrics {
    width:f32,
    height:f32,
    ratio:Option<f32>,
    border_ratio:bool,
}
impl ImageMetrics {
    fn of(node:&LayoutBox)->Self {
        let (mut width,mut height)=node.image.as_ref().filter(|image|!image.broken).map(|image|image.natural_size()).unwrap_or((node.attr_size.0.unwrap_or(0.),node.attr_size.1.unwrap_or(0.)));
        let explicit=node.style.aspect_ratio.filter(|ratio|*ratio>0.&&ratio.is_finite());
        let ratio=explicit.or_else(||(width>0.&&height>0.).then(||width/height));
        if let Some(ratio)=ratio {
            if width>0.&&height<=0. {height=width/ratio;}
            else if height>0.&&width<=0. {width=height*ratio;}
        }
        Self{width,height,ratio,border_ratio:explicit.is_some()&&node.style.box_sizing==BoxSizing::BorderBox}
    }
    fn width_from_height(&self,height:f32,pb_h:f32,pb_v:f32)->f32 {
        self.ratio.map_or(self.width,|ratio|if self.border_ratio {((height+pb_v)*ratio-pb_h).max(0.)} else {height*ratio})
    }
    fn height_from_width(&self,width:f32,pb_h:f32,pb_v:f32)->f32 {
        self.ratio.map_or(self.height,|ratio|if self.border_ratio {((width+pb_h)/ratio-pb_v).max(0.)} else {width/ratio})
    }
}

fn image_content_width(node:&LayoutBox,bases:[AvailableSpace;2])->f32 {
    let style=&node.style;let metrics=ImageMetrics::of(node);
    let pb_h=crate::block::horizontal_extras(style,0.);let pb_v=vertical_edges(style);
    let natural=if node.ratio_only {bases[0].definite().map(|width|(width-pb_h).max(0.)).unwrap_or(300.)} else {metrics.width};
    let natural_height=metrics.height_from_width(natural,pb_h,pb_v);
    let preferred=image_height_value(style,style.height,bases[1],pb_v,natural_height);
    let height=clamp_image_height(style,preferred.unwrap_or(natural_height),bases[1],pb_v,natural_height);
    if preferred.is_some()||height!=natural_height {metrics.width_from_height(height,pb_h,pb_v)} else {natural}
}

fn image_used_height(node:&LayoutBox,width:f32,bases:[AvailableSpace;2],pb_h:f32,pb_v:f32)->f32 {
    let style=&node.style;let metrics=ImageMetrics::of(node);
    let automatic=metrics.height_from_width(width,pb_h,pb_v);
    let preferred=image_height_value(style,style.height,bases[1],pb_v,automatic);
    clamp_image_height(style,preferred.unwrap_or(automatic),bases[1],pb_v,automatic)
}

impl<'a> Layouter<'a> {
    /// The content oracle excludes this box's preferred/min/max inline
    /// sizes, including the corresponding logical block properties behind
    /// an orthogonal wrapper. Bounds such as min-width:max-content must not
    /// see intrinsic(node)'s early (width,width) numeric preferred shortcut.
    fn estimated_inline_content(&mut self,child:&mut LayoutBox,bases:[AvailableSpace;2])->(f32,f32) {
        let scope=Scope::inline_content(child,&mut self.grid_work);
        if scope.node.kind==BoxKind::Image {
            let width=image_content_width(scope.node,bases);return (width,width);
        }
        if !scope.node.wm_wrapper {return self.intrinsic(scope.node);}
        // The generic wrapper intrinsic helper uses the viewport height.
        // Here a real perpendicular CB size, if present, takes precedence.
        // An actually indefinite block input keeps the existing WM fallback.
        let outer=&scope.node.style;
        let margins=outer.margin.top.resolve_or_zero(0.)+outer.margin.bottom.resolve_or_zero(0.);
        let inline=bases[1].definite().map(|height|(height-margins).max(0.)).unwrap_or_else(||self.ctx.viewport_height.max(1.));
        let block=bases[0].definite().map(|width|(width-outer.margin.left.resolve_or_zero(0.)-outer.margin.right.resolve_or_zero(0.)).max(0.));
        let Some(inner)=scope.node.children.first_mut() else {return (0.,0.);};
        let mode=if inner.style.width.is_auto() {WidthMode::ShrinkToFit} else {WidthMode::Fill};
        let cb=Cb{x:0.,width:inline,height:block};let mut bfc=Bfc::new();
        self.forced_content_height=None;
        self.layout_block(inner,&cb,0.,&mut bfc,mode);
        (inner.rect.h,inner.rect.h)
    }

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
        let content=self.estimated_inline_content(scope.node,spaces);
        let natural=scope.node.kind==BoxKind::Image||style.aspect_ratio.is_some_and(|ratio|ratio>0.);
        let stretch=style.width.is_auto()&&!edges.auto.left&&!edges.auto.right&&(item.justify==ItemAlign::Stretch||(!natural&&matches!(item.justify,ItemAlign::Normal|ItemAlign::Auto)));
        let preferred=if style.width.is_auto()&&!stretch&&scope.node.kind!=BoxKind::Image {
            style.aspect_ratio.filter(|ratio|*ratio>0.).zip(style.height.resolve_definite(spaces[1].definite())).map(|(ratio,height)|Length::Px(height*ratio)).unwrap_or(style.width)
        } else {style.width};
        let inline=realize_preferred(&style,preferred,spaces[0],spaces[0],content,edges.pb_h,edges.margin_sum(Axis::Row),stretch);
        // Only execution uses a finite proxy; own cyclic preferred/min/max
        // lengths and percentage edges were normalized in the scope above.
        let proxy=inline.percentage_basis().unwrap_or_else(||(inline.content()+edges.pb_h+edges.margin_sum(Axis::Row)).max(0.));
        if scope.node.kind==BoxKind::Image {
            // Replaced layout ignores WidthMode::Fixed. Give it the exact
            // scoped preferred width, including explicit stretch, so the
            // reported estimate and executed content width stay coherent.
            self.grid_work.charge(1);
            let height=image_used_height(scope.node,inline.content(),spaces,edges.pb_h,edges.pb_v);
            let mut execution=(*style).clone();
            // Both dimensions are already content sizes with local bounds
            // applied. Avoid the old replaced helper's independent clamp
            // and border-box conventions applying those constraints twice.
            execution.box_sizing=BoxSizing::ContentBox;
            execution.width=Length::Px(inline.content());execution.height=Length::Px(height);
            execution.min_width=Length::ZERO;execution.max_width=Length::Auto;
            execution.min_height=Length::ZERO;execution.max_height=Length::Auto;
            scope.node.style=Rc::new(execution);
        }
        let cb=Cb{x:0.,width:proxy,height:spaces[1].definite()};
        let mut bfc=Bfc::new();self.forced_content_height=None;
        self.layout_block(scope.node,&cb,0.,&mut bfc,WidthMode::Fixed(inline.content()));
        RowItemEstimate{inline,outer_block:scope.node.rect.h+edges.margin_sum(Axis::Column),baseline:Self::item_baseline(scope.node)}
    }
}

/// `style` must already have the percentage policy for this measurement
/// applied. The item's min/max-content constraint and its containing-block
/// percentage basis are separate inputs: a fixed parent track may provide
/// a real basis while an automatic item is measured under MinContent.
/// Neither input is inferred from the subsequently realized content width.
/// The result sizes an item for measurement; it is not a CompletedAxis.
pub(super) fn realize(style:&ComputedStyle,available:AvailableSpace,percentage_basis:AvailableSpace,content:(f32,f32),padding_border:f32,margins:f32,stretch:bool)->FiniteInline {
    realize_preferred(style,style.width,available,percentage_basis,content,padding_border,margins,stretch)
}
#[allow(clippy::too_many_arguments)]
fn realize_preferred(style:&ComputedStyle,preferred:Length,available:AvailableSpace,percentage_basis:AvailableSpace,content:(f32,f32),padding_border:f32,margins:f32,stretch:bool)->FiniteInline {
    assert!(content.0.is_finite()&&content.1.is_finite()&&content.1>=content.0,"Invalid intrinsic Grid item size");
    let reference=percentage_basis.definite();
    let inner=|value:f32|if style.box_sizing==BoxSizing::BorderBox {(value-padding_border).max(0.)} else {value.max(0.)};
    let room=available.definite().map(|size|(size-padding_border-margins).max(0.));
    let fit=|min:f32,max:f32|match available {
        AvailableSpace::MinContent=>min,
        _=>room.map_or(max,|room|crate::block::shrink_to_fit(min,max,room)),
    };
    let size=if let Some(size)=preferred.resolve_definite(reference) {inner(size)} else {
        match preferred {
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
    FiniteInline{content:size,available,percentage_basis:reference}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{catch_unwind,AssertUnwindSafe};
    use std::collections::HashMap;
    use std::sync::Arc;
    use rabbithole_gfx::{FontContext,Image};
    use rabbithole_css::{parse_stylesheet,MediaEnv};

    fn realize_same_basis(style:&ComputedStyle,available:AvailableSpace,content:(f32,f32),padding_border:f32,margins:f32,stretch:bool)->FiniteInline {
        super::realize(style,available,available,content,padding_border,margins,stretch)
    }

    fn find_mut(node:&mut LayoutBox,id:usize)->Option<&mut LayoutBox> {
        fn owns(node:&LayoutBox,id:usize)->bool {
            node.node==Some(id)||(node.wm_wrapper&&node.children.first().is_some_and(|inner|owns(inner,id)))
        }
        if node.wm_wrapper&&owns(node,id) {return Some(node);}
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
        measured_case("<div id=probe>XXXX XX</div><div id=reference>XXXX XX</div>",css,[inline,AvailableSpace::Indefinite],edge_basis,ItemAlign::Stretch)
    }
    fn measured_case(html:&str,css:&str,bases:[AvailableSpace;2],edge_basis:AvailableSpace,justify:ItemAlign)->(RowItemEstimate,(f32,f32)) {
        let doc=rabbithole_html::parse(html);
        let sheets=[parse_stylesheet(&format!("*{{font:10px/1 monospace}}div{{display:block;min-width:0}}{css}"))];
        let styles=rabbithole_style::style_tree(&doc,&sheets,&MediaEnv{width:800.,height:600.,screen:true,scripting:true});
        let fonts=FontContext::new();let mut images=HashMap::new();let backgrounds=HashMap::new();let scrolls=HashMap::new();
        let id=doc.element_by_id("probe").unwrap();
        if doc.tag(id)==Some("img") {images.insert(id,Arc::new(Image::new(40,20,vec![0;40*20*4])));}
        let ctx=crate::LayoutContext{doc:&doc,styles:&styles,fonts:&fonts,images:&images,background_images:&backgrounds,scroll_offsets:&scrolls,viewport_width:800.,viewport_height:600.,page_scroll:(0.,0.)};
        let reference=crate::layout(&ctx);
        let reference=find(&reference.root,doc.element_by_id("reference").unwrap()).unwrap();
        let expected=(reference.content_rect.w,reference.rect.h);
        let mut root=crate::wm::prepare(crate::build::Builder::new(&ctx,None).build_root(),doc.body());
        let node=find_mut(&mut root,doc.element_by_id("probe").unwrap()).unwrap();
        if styles.get(id).is_some_and(|style|style.writing_mode.is_vertical()) {
            assert!(node.wm_wrapper,"measure the anonymous wrapper, not its logical inner");
        }
        let original=node.style.clone();
        let item=super::super::GridItem{idx:0,area:super::super::Area{row:(0,1),col:(0,1)},edges:ItemEdges::new(&node.style,800.),justify,align:ItemAlign::FlexStart,column_names:true,row_names:true,parent_lanes_auto:false};
        let mut layouter=Layouter::new(&ctx);
        let measured=layouter.estimate_row_item(node,&item,bases,edge_basis,OperationKind::Layout,800.);
        assert!(Rc::ptr_eq(&node.style,&original));assert!(node.intrinsic_cache.is_none());
        assert!((measured.inline.content()-node.content_rect.w).abs()<0.02,"reported {} != executed {}",measured.inline.content(),node.content_rect.w);
        (measured,expected)
    }

    #[test]
    fn infinite_auto_realization_is_finite_and_keeps_the_original_percentage_tag() {
        let style=ComputedStyle::initial();
        let realized=realize_same_basis(&style,AvailableSpace::Indefinite,(30.,80.),0.,0.,true);
        assert_eq!(realized.content(),80.);assert_eq!(realized.available(),AvailableSpace::Indefinite);
        assert_eq!(realized.percentage_basis(),None);
        let zero=realize_same_basis(&style,AvailableSpace::Definite(0.),(30.,80.),0.,0.,true);
        assert_eq!(zero.content(),0.);assert_eq!(zero.percentage_basis(),Some(0.));
    }

    #[test]
    fn preferred_keywords_and_definite_sizes_override_an_infinite_estimate() {
        for (preferred,expected) in [(Length::MinContent,30.),(Length::MaxContent,80.),(Length::FitContent,80.),(Length::Px(55.),55.)] {
            let mut style=ComputedStyle::initial();style.width=preferred;
            assert_eq!(realize_same_basis(&style,AvailableSpace::Indefinite,(30.,80.),0.,0.,true).content(),expected);
        }
    }

    #[test]
    fn definite_fit_and_stretch_do_not_share_the_max_content_fallback() {
        let mut style=ComputedStyle::initial();style.width=Length::FitContent;
        assert_eq!(realize_same_basis(&style,AvailableSpace::Definite(63.),(30.,80.),8.,5.,true).content(),50.);
        assert_eq!(realize_same_basis(&style,AvailableSpace::MinContent,(30.,80.),8.,5.,true).content(),30.);
        style.width=Length::Auto;
        assert_eq!(realize_same_basis(&style,AvailableSpace::Definite(150.),(30.,80.),8.,5.,true).content(),137.);
        assert_eq!(realize_same_basis(&style,AvailableSpace::Definite(150.),(30.,80.),8.,5.,false).content(),80.);
        assert_eq!(realize_same_basis(&style,AvailableSpace::MinContent,(30.,80.),8.,5.,true).content(),30.);
    }

    #[test]
    fn cyclic_min_calc_preserves_its_absolute_floor_without_using_the_finite_proxy() {
        let mut style=ComputedStyle::initial();style.width=Length::Percent(50.);style.min_width=Length::Calc(90.,20.);style.max_width=Length::Percent(25.);
        percentages::normalize(&mut style,[AvailableSpace::Indefinite,AvailableSpace::Indefinite],AvailableSpace::Indefinite,GridAxis::Rows,OperationKind::Layout,false);
        let realized=realize_same_basis(&style,AvailableSpace::Indefinite,(30.,80.),0.,0.,false);
        assert_eq!(realized.content(),90.);assert_eq!(realized.percentage_basis(),None);
        // A real base keeps percentages and yields a different used width.
        let mut defined=ComputedStyle::initial();defined.width=Length::Percent(50.);
        assert_eq!(realize_same_basis(&defined,AvailableSpace::Definite(200.),(30.,80.),0.,0.,false).content(),100.);
    }

    #[test]
    fn numeric_border_box_bounds_subtract_edges_once_and_minimum_wins_conflicts() {
        let mut style=ComputedStyle::initial();style.box_sizing=BoxSizing::BorderBox;
        style.width=Length::Px(60.);style.min_width=Length::Px(90.);style.max_width=Length::Px(70.);
        assert_eq!(realize_same_basis(&style,AvailableSpace::Indefinite,(30.,80.),10.,4.,false).content(),80.);
    }

    #[test]
    fn intrinsic_minimum_and_maximum_keywords_do_not_disappear_in_a_numeric_proxy() {
        let mut style=ComputedStyle::initial();style.width=Length::Px(55.);style.min_width=Length::MaxContent;
        assert_eq!(realize_same_basis(&style,AvailableSpace::Indefinite,(30.,80.),10.,4.,false).content(),80.);
        style.min_width=Length::Auto;style.max_width=Length::MinContent;
        assert_eq!(realize_same_basis(&style,AvailableSpace::Indefinite,(30.,80.),10.,4.,false).content(),30.);
        style.width=Length::MaxContent;style.max_width=Length::FitContent;
        assert_eq!(realize_same_basis(&style,AvailableSpace::Definite(64.),(30.,80.),10.,4.,false).content(),50.);
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

    #[test]
    fn min_content_constraint_does_not_erase_a_definite_containing_block_percentage_base() {
        let mut style=ComputedStyle::initial();style.width=Length::Percent(50.);
        let fixed=super::realize(&style,AvailableSpace::MinContent,AvailableSpace::Definite(100.),(30.,80.),0.,0.,false);
        assert_eq!(fixed.content(),50.);assert_eq!(fixed.available(),AvailableSpace::MinContent);
        assert_eq!(fixed.percentage_basis(),Some(100.));
        style.width=Length::Auto;
        assert_eq!(super::realize(&style,AvailableSpace::MinContent,AvailableSpace::Definite(100.),(30.,80.),0.,0.,false).content(),30.);
        style.width=Length::FitContent;
        assert_eq!(super::realize(&style,AvailableSpace::MinContent,AvailableSpace::Definite(100.),(30.,80.),0.,0.,false).content(),30.);
    }

    #[test]
    fn numeric_preferred_width_does_not_replace_the_content_oracle_for_intrinsic_bounds() {
        let html="<div id=probe><span></span><span></span></div><div id=reference></div>";
        for (preferred,bound,expected_width,expected_height) in [(10.,"min-width:max-content",80.,10.),(100.,"max-width:min-content",40.,20.)] {
            let css=format!("#probe,#probe *{{font:0/0 monospace}}#probe{{width:{preferred}px;{bound}}}#probe span{{display:inline-block;width:40px;height:10px;vertical-align:top}}#reference{{width:{expected_width}px;height:{expected_height}px}}");
            let (measured,expected)=measured_case(html,&css,[AvailableSpace::Definite(160.),AvailableSpace::Indefinite],AvailableSpace::Definite(160.),ItemAlign::FlexStart);
            assert!((measured.inline.content()-expected.0).abs()<0.02);
            assert!((measured.outer_block-expected.1).abs()<0.02);
        }
    }

    #[test]
    fn percentage_normalization_uses_the_containing_basis_before_an_intrinsic_constraint_is_applied() {
        let mut style=ComputedStyle::initial();style.width=Length::Percent(50.);
        let original=Rc::new(style);let mut node=LayoutBox::new(BoxKind::Block,None,original.clone());
        let mut budget=WorkBudget::default();
        {
            let scope=Scope::new(&mut node,[AvailableSpace::Definite(100.),AvailableSpace::Indefinite],AvailableSpace::Definite(100.),GridAxis::Columns,OperationKind::IntrinsicMin,&mut budget);
            assert_eq!(scope.node.style.width,Length::Percent(50.));
            let measured=super::realize(&scope.node.style,AvailableSpace::MinContent,AvailableSpace::Definite(100.),(30.,80.),0.,0.,false);
            assert_eq!(measured.content(),50.);assert_eq!(measured.available(),AvailableSpace::MinContent);
        }
        assert!(Rc::ptr_eq(&node.style,&original));
    }

    #[test]
    fn ordinary_wrapper_content_uses_a_defined_perpendicular_space_instead_of_viewport_height() {
        let html="<div id=probe><span></span><span></span><span></span><span></span><span></span></div><div id=reference></div>";
        for mode in ["vertical-rl","vertical-lr","sideways-lr"] {
            for (block,width) in [(30.,50.),(60.,20.)] {
                let css=format!("#probe,#probe *{{font:0/0 monospace}}#probe{{writing-mode:{mode};min-height:0}}#probe span{{display:inline-block;width:10px;height:20px;vertical-align:top}}#reference{{width:{width}px;height:{block}px}}");
                let (measured,expected)=measured_case(html,&css,[AvailableSpace::Indefinite,AvailableSpace::Definite(block)],AvailableSpace::Indefinite,ItemAlign::Normal);
                assert!((measured.inline.content()-expected.0).abs()<0.02,"{mode}/{block}: {} != {}",measured.inline.content(),expected.0);
                assert!((measured.outer_block-expected.1).abs()<0.02);
            }
        }
    }

    #[test]
    fn images_and_ratio_boxes_preserve_normal_and_explicit_stretch_as_distinct_rules() {
        for (tag,extra,normal,stretch) in [("img","",(40.,20.),(100.,50.)),("div","height:20px;aspect-ratio:2",(40.,20.),(100.,20.))] {
            for (justify,(width,height)) in [(ItemAlign::Normal,normal),(ItemAlign::Stretch,stretch)] {
                let html=if tag=="img" {"<img id=probe><div id=reference></div>"} else {"<div id=probe></div><div id=reference></div>"};
                let css=format!("#probe{{{extra}}}#reference{{width:{width}px;height:{height}px}}");
                let (measured,expected)=measured_case(html,&css,[AvailableSpace::Definite(100.),AvailableSpace::Indefinite],AvailableSpace::Definite(100.),justify);
                assert!((measured.inline.content()-expected.0).abs()<0.02,"{tag}/{justify:?}: {} != {}",measured.inline.content(),expected.0);
                assert!((measured.outer_block-expected.1).abs()<0.02,"{tag}/{justify:?}: {} != {}",measured.outer_block,expected.1);
            }
        }
    }

    #[test]
    fn percentage_scope_restores_every_asymmetric_wrapper_style_on_unwind() {
        let originals:Vec<_>=(0..4).map(|i| {
            let mut style=ComputedStyle::initial();style.width=Length::Percent(50.+i as f32);style.height=Length::Percent(25.+i as f32);style.padding.left=Length::Percent(10.+i as f32);Rc::new(style)
        }).collect();
        let mut node=LayoutBox::new(BoxKind::Block,None,originals[3].clone());
        for (i,kind) in [(2,crate::tree::WmKind::Reflect),(1,crate::tree::WmKind::Ccw),(0,crate::tree::WmKind::Cw)] {
            let mut wrapper=LayoutBox::new(BoxKind::Block,None,originals[i].clone());wrapper.wm_wrapper=true;wrapper.wm_kind=kind;wrapper.children.push(node);node=wrapper;
        }
        let mut budget=WorkBudget::default();
        let result=catch_unwind(AssertUnwindSafe(|| {
            let scope=Scope::new(&mut node,[AvailableSpace::Indefinite,AvailableSpace::Definite(80.)],AvailableSpace::Definite(300.),GridAxis::Rows,OperationKind::Layout,&mut budget);
            let mut current=&mut *scope.node;
            for i in 0..4 {
                assert_eq!(current.style.padding.left,Length::Px(30.+3.*i as f32));
                if i%2==0 {assert_eq!(current.style.width,Length::Auto);} else {assert_eq!(current.style.height,Length::Auto);}
                current.intrinsic_cache=Some((100.,100.));
                if i<3 {current=current.children.first_mut().unwrap();}
            }
            panic!("intentional wrapper estimate abort");
        }));
        assert!(result.is_err());let mut current=&node;
        for (i,style) in originals.iter().enumerate() {
            assert!(Rc::ptr_eq(&current.style,style));assert!(current.intrinsic_cache.is_none());
            if i<3 {current=current.children.first().unwrap();}
        }
    }

    #[test]
    fn measurement_selection_keeps_an_anonymous_wrapper_instead_of_its_identified_inner() {
        let style=Rc::new(ComputedStyle::initial());
        let inner=LayoutBox::new(BoxKind::Block,Some(7),style.clone());
        let mut wrapper=LayoutBox::new(BoxKind::Block,None,style.clone());wrapper.wm_wrapper=true;wrapper.children.push(inner);
        let mut root=LayoutBox::new(BoxKind::Block,None,style);root.children.push(wrapper);
        let selected=find_mut(&mut root,7).unwrap();
        assert!(selected.wm_wrapper);assert!(selected.node.is_none());assert_eq!(selected.children[0].node,Some(7));
    }

    #[test]
    fn image_measurement_uses_real_block_bases_and_applies_border_box_bounds_only_once() {
        let html="<img id=probe><div id=reference></div>";
        for (source,reference,block) in [
            ("height:50%","width:80px;height:40px",AvailableSpace::Definite(80.)),
            ("box-sizing:border-box;width:10px;min-width:30px;padding:0 5px","width:20px;height:10px;padding:0 5px",AvailableSpace::Indefinite),
            ("box-sizing:border-box;height:30px;padding:5px 0","width:40px;height:20px;padding:5px 0",AvailableSpace::Indefinite),
            ("box-sizing:border-box;height:30px;padding:5px 0;aspect-ratio:2","width:60px;height:20px;padding:5px 0",AvailableSpace::Indefinite),
            ("max-height:10px","width:20px;height:10px",AvailableSpace::Indefinite),
            ("min-height:50%","width:80px;height:40px",AvailableSpace::Definite(80.)),
            ("max-height:50%","width:20px;height:10px",AvailableSpace::Definite(20.)),
        ] {
            let css=format!("#probe{{{source}}}#reference{{{reference}}}");
            let (measured,expected)=measured_case(html,&css,[AvailableSpace::Definite(100.),block],AvailableSpace::Definite(100.),ItemAlign::Normal);
            assert!((measured.inline.content()-expected.0).abs()<0.02,"{source}: {} != {}",measured.inline.content(),expected.0);
            assert!((measured.outer_block-expected.1).abs()<0.02,"{source}: {} != {}",measured.outer_block,expected.1);
        }
    }
}
