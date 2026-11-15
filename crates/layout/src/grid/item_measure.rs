//! Fresh ordinary-item measurements for the typed per-axis dispatcher.
//! Traversal owns slots, inherited-axis projection and track completion.
use crate::block::Layouter;
use crate::flex::{Axis,ItemEdges};
use crate::tree::{BoxKind,LayoutBox};
use rabbithole_style::{BoxSizing,ItemAlign,Length};
use super::{Contribution,GridItem,estimate::{ItemSpace,Scope},phases::{GridAxis,OperationKind},subgrid};

pub(super) struct OrdinaryItemMeasurement {
    pub(super) contribution:Contribution,
    /// Min-content contribution, independently of the legacy minimum
    /// contribution passed to the current track solver.
    pub(super) true_min_content:f32,
    pub(super) baseline:Option<f32>,
}

/// Initial consumer eligibility is deliberately narrower than the helper's
/// representable inputs. Reject the whole new Grid path if any sibling has
/// an inherited axis, replaced/ratio item or wrapper padding/border whose
/// physical metric is not yet represented by the helper.
pub(super) fn supports_ordinary_item(mut child:&LayoutBox)->bool {
    let wrapped=child.wm_wrapper;
    for _ in 0..128 {
        if child.kind==BoxKind::Image||child.ratio_only||child.style.aspect_ratio.is_some_and(|ratio|ratio>0.&&ratio.is_finite()) {
            return false;
        }
        if wrapped {
            let p=child.style.padding;let b=child.style.border_width;
            if [p.left,p.right,p.top,p.bottom].into_iter().any(|value|value!=Length::ZERO)
                || [b.left,b.right,b.top,b.bottom].into_iter().any(|value|value!=0.) {
                return false;
            }
        }
        if !child.wm_wrapper {return subgrid::requests(child)==(false,false);}
        let Some(inner)=child.children.first() else {return false;};child=inner;
    }
    false
}

impl<'a> Layouter<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn measure_ordinary_item(&mut self,child:&mut LayoutBox,item:&GridItem,axis:GridAxis,kind:OperationKind,space:ItemSpace,old_reference:f32)->OrdinaryItemMeasurement {
        let _probe = self.grid_publication.probe();
        assert!(supports_ordinary_item(child),"Grid ordinary adapter requires a supported non-inherited item");
        self.grid_work.charge(1);
        let (start,end)=match axis {GridAxis::Columns=>item.area.col,GridAxis::Rows=>item.area.row};
        assert!(start>=0&&end>start,"Grid measurement requires a resolved nonempty area");
        let (minimum,maximum,true_min_content,baseline)=match axis {
            GridAxis::Columns=>{
                let mut low=space;low.constraints[0]=super::phases::AvailableSpace::MinContent;
                let mut high=space;high.constraints[0]=super::phases::AvailableSpace::MaxContent;
                let min=self.estimate_item(child,item,low,axis,kind,old_reference);
                let max=self.estimate_item(child,item,high,axis,kind,old_reference);
                // Preserve the legacy minimum-contribution contract. Unlike
                // the adjustment probe, an explicit min-width:0 or scroll
                // minimum can make this smaller than min-content.
                let old_edges=ItemEdges::new(&child.style,old_reference);
                let scope=Scope::new(child,space.bases,space.edge_basis,axis,kind,&mut self.grid_work);
                let style=&scope.node.style;
                let mut edges=ItemEdges::new(style,space.edge_basis.definite().unwrap_or(0.));
                edges.margin.left+=item.edges.margin.left-old_edges.margin.left;
                edges.margin.right+=item.edges.margin.right-old_edges.margin.right;
                let extras=edges.pb_h+edges.margin_sum(Axis::Row);
                let minimum=match style.min_width {
                    Length::Px(value)=>{
                        let content=if style.box_sizing==BoxSizing::BorderBox {(value-edges.pb_h).max(0.)}else{value.max(0.)};
                        content.min(min.inline.content())+extras
                    }
                    Length::Auto if crate::flex::is_scroll_container(style)=>extras,
                    _=>min.outer_inline,
                };
                (minimum,max.outer_inline.max(min.outer_inline).max(minimum),min.outer_inline,None)
            }
            GridAxis::Rows=>{
                let measured=self.estimate_item(child,item,space,axis,kind,old_reference);
                (measured.outer_block,measured.outer_block,measured.outer_block,(item.align==ItemAlign::Baseline).then_some(measured.baseline))
            }
        };
        assert!(minimum.is_finite()&&maximum.is_finite()&&true_min_content.is_finite(),"Non-finite ordinary Grid contribution");
        OrdinaryItemMeasurement{contribution:Contribution{start:start as usize,end:end as usize,min:minimum,max:maximum},true_min_content,baseline}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::phases::AvailableSpace;
    use super::super::subgrid::WorkBudget;
    use rabbithole_gfx::FontContext;
    use std::collections::HashMap;
    use std::panic::{catch_unwind,AssertUnwindSafe};
    use std::rc::Rc;

    fn find_mut(node:&mut LayoutBox,id:usize)->Option<&mut LayoutBox> {
        fn owns(node:&LayoutBox,id:usize)->bool {node.node==Some(id)||(node.wm_wrapper&&node.children.first().is_some_and(|inner|owns(inner,id)))}
        if node.wm_wrapper&&owns(node,id) {return Some(node);}
        if node.node==Some(id)&&node.kind!=BoxKind::Text {return Some(node);}
        node.children.iter_mut().find_map(|child|find_mut(child,id))
    }
    fn assert_zero_strut(node:&LayoutBox) {
        assert_eq!(node.style.font_size,0.,"fixture font size must not enlarge its atomic boxes");
        assert_eq!(node.style.line_height,Some(0.),"fixture line height must remain zero through wrappers");
        for child in &node.children {assert_zero_strut(child);}
    }
    fn with_item(body:&str,css:&str,test:impl FnOnce(&mut Layouter<'_>,&mut LayoutBox,&GridItem)) {
        let doc=rabbithole_html::parse(&format!("<!doctype html><div id=probe>{body}</div>"));
        // The current font-shorthand parser rejects font:0/0 monospace.
        // Use supported longhands so these atomic-box geometry controls do
        // not accidentally retain the inherited 16px/normal line strut.
        let sheets=[rabbithole_css::parse_stylesheet(&format!("*{{font-size:0;line-height:0;font-family:monospace}}#probe{{min-width:0}}span{{display:inline-block;width:40px;height:10px;vertical-align:top}}{css}"))];
        let styles=rabbithole_style::style_tree(&doc,&sheets,&rabbithole_css::MediaEnv{width:800.,height:600.,screen:true,scripting:true});
        for style in styles.styles.iter().flatten() {
            assert_eq!(style.font_size,0.,"fixture zero font size must be applied before measuring");
            assert_eq!(style.line_height,Some(0.),"fixture zero line height must be applied before measuring");
        }
        let fonts=FontContext::new();let images=HashMap::new();let backgrounds=HashMap::new();let scrolls=HashMap::new();
        let ctx=crate::LayoutContext{doc:&doc,styles:&styles,fonts:&fonts,images:&images,background_images:&backgrounds,scroll_offsets:&scrolls,viewport_width:800.,viewport_height:600.,page_scroll:(0.,0.)};
        let mut root=crate::wm::prepare(crate::build::Builder::new(&ctx,None).build_root(),doc.body());
        let node=find_mut(&mut root,doc.element_by_id("probe").unwrap()).unwrap();
        assert_zero_strut(node);
        let item=GridItem{idx:0,area:super::super::Area{row:(0,1),col:(0,1)},edges:ItemEdges::new(&node.style,800.),justify:ItemAlign::Normal,align:ItemAlign::FlexStart,column_names:true,row_names:true,parent_lanes_auto:false};
        test(&mut Layouter::new(&ctx),node,&item);
    }
    fn area(inline:AvailableSpace,block:AvailableSpace)->ItemSpace {
        ItemSpace::new([inline,block],[inline,block],inline)
    }

    #[test]
    fn ordinary_minimum_and_true_min_content_are_independent() {
        with_item("<span></span><span></span>","",|layouter,node,item| {
            let original=node.style.clone();
            let value=layouter.measure_ordinary_item(node,item,GridAxis::Columns,OperationKind::Layout,area(AvailableSpace::Definite(100.),AvailableSpace::Indefinite),800.);
            assert_eq!(value.contribution,Contribution{start:0,end:1,min:0.,max:80.});
            assert_eq!(value.true_min_content,40.);assert!(value.baseline.is_none());
            assert!(Rc::ptr_eq(&node.style,&original));assert!(node.intrinsic_cache.is_none());
        });
    }

    #[test]
    fn intrinsic_bounds_do_not_reuse_a_numeric_preferred_width_as_content() {
        for (css,minimum,content,maximum) in [("#probe{width:10px;min-width:max-content}",80.,80.,80.),("#probe{width:100px;max-width:min-content}",0.,40.,40.)] {
            with_item("<span></span><span></span>",css,|layouter,node,item| {
                let value=layouter.measure_ordinary_item(node,item,GridAxis::Columns,OperationKind::Layout,area(AvailableSpace::Definite(160.),AvailableSpace::Indefinite),800.);
                assert_eq!(value.contribution.min,minimum);assert_eq!(value.true_min_content,content);assert_eq!(value.contribution.max,maximum);
            });
        }
    }

    #[test]
    fn repeated_row_measurements_use_fresh_perpendicular_geometry_on_the_same_item() {
        with_item("<span></span><span></span>","",|layouter,node,item| {
            let original=node.style.clone();
            for (width,height) in [(40.,20.),(80.,10.),(40.,20.)] {
                let value=layouter.measure_ordinary_item(node,item,GridAxis::Rows,OperationKind::Layout,area(AvailableSpace::Definite(width),AvailableSpace::Indefinite),800.);
                assert_eq!(value.contribution.min,height);assert_eq!(value.contribution.max,height);assert_eq!(value.true_min_content,height);
                assert!(Rc::ptr_eq(&node.style,&original));assert!(node.intrinsic_cache.is_none());
            }
        });
    }

    #[test]
    fn wrapper_columns_follow_fresh_block_spans_in_each_orientation() {
        for mode in ["vertical-rl","vertical-lr","sideways-lr"] {
            with_item("<span></span><span></span><span></span><span></span><span></span>",&format!("#probe{{writing-mode:{mode};min-height:0}}span{{width:10px;height:20px}}"),|layouter,node,item| {
                assert!(node.wm_wrapper);let original=node.style.clone();
                for (height,width) in [(30.,50.),(60.,20.),(30.,50.)] {
                    let space=ItemSpace::new([AvailableSpace::Indefinite,AvailableSpace::Definite(height)],[AvailableSpace::Indefinite,AvailableSpace::Definite(height)],AvailableSpace::Indefinite);
                    let value=layouter.measure_ordinary_item(node,item,GridAxis::Columns,OperationKind::Layout,space,800.);
                    assert_eq!(value.true_min_content,width,"{mode}/{height}");assert_eq!(value.contribution.max,width);
                    assert!(Rc::ptr_eq(&node.style,&original));assert!(node.intrinsic_cache.is_none());
                }
            });
        }
    }

    #[test]
    fn percentage_bases_cross_wrappers_without_becoming_the_measurement_constraint() {
        for mode in ["vertical-rl","vertical-lr","sideways-lr"] {
            with_item("",&format!("#probe{{writing-mode:{mode};width:50%;height:50%;min-height:0}}"),|layouter,node,item| {
                let space=ItemSpace::new([AvailableSpace::MinContent,AvailableSpace::Indefinite],[AvailableSpace::Definite(100.),AvailableSpace::Definite(80.)],AvailableSpace::Definite(300.));
                let columns=layouter.measure_ordinary_item(node,item,GridAxis::Columns,OperationKind::IntrinsicMin,space,800.);
                assert_eq!(columns.true_min_content,50.);assert_eq!(columns.contribution.max,50.);
                let rows=layouter.measure_ordinary_item(node,item,GridAxis::Rows,OperationKind::IntrinsicMin,space,800.);
                assert_eq!(rows.true_min_content,40.);assert_eq!(rows.contribution.max,40.);
            });
        }
    }

    #[test]
    fn unsupported_replaced_ratio_and_inherited_items_are_not_silently_collected() {
        let style=Rc::new(rabbithole_style::ComputedStyle::initial());
        let mut ordinary=LayoutBox::new(BoxKind::Block,None,style.clone());assert!(supports_ordinary_item(&ordinary));
        ordinary.ratio_only=true;assert!(!supports_ordinary_item(&ordinary));ordinary.ratio_only=false;
        let mut ratio=(*style).clone();ratio.aspect_ratio=Some(2.);ordinary.style=Rc::new(ratio);assert!(!supports_ordinary_item(&ordinary));
        let image=LayoutBox::new(BoxKind::Image,None,style.clone());assert!(!supports_ordinary_item(&image));
        let mut wrapper=LayoutBox::new(BoxKind::Block,None,style);wrapper.wm_wrapper=true;wrapper.children.push(image);assert!(!supports_ordinary_item(&wrapper));
        with_item("","#probe{display:grid;grid-template-columns:subgrid}",|_,node,_|assert!(!supports_ordinary_item(node)));
    }

    #[test]
    fn wrapper_padding_and_border_require_a_separate_physical_metric() {
        let style=Rc::new(rabbithole_style::ComputedStyle::initial());
        for padding in [Length::Px(10.),Length::Percent(10.),Length::Calc(0.,0.)] {
            let mut padded=(*style).clone();padded.padding.left=padding;
            let ordinary=LayoutBox::new(BoxKind::Block,None,Rc::new(padded));
            assert!(supports_ordinary_item(&ordinary),"ordinary content/PB metric remains supported");
            let mut wrapper=LayoutBox::new(BoxKind::Block,None,style.clone());wrapper.wm_wrapper=true;wrapper.children.push(ordinary);
            assert!(!supports_ordinary_item(&wrapper),"wrapper needs a physical PB metric for {padding:?}");
        }
        let mut bordered=(*style).clone();bordered.border_width.bottom=2.;
        let ordinary=LayoutBox::new(BoxKind::Block,None,Rc::new(bordered));assert!(supports_ordinary_item(&ordinary));
        let mut wrapper=LayoutBox::new(BoxKind::Block,None,style.clone());wrapper.wm_wrapper=true;wrapper.children.push(ordinary);assert!(!supports_ordinary_item(&wrapper));
        let mut padded=(*style).clone();padded.padding.top=Length::Px(5.);
        let mut middle=LayoutBox::new(BoxKind::Block,None,Rc::new(padded));middle.wm_wrapper=true;middle.children.push(LayoutBox::new(BoxKind::Block,None,style.clone()));
        let mut outer=LayoutBox::new(BoxKind::Block,None,style.clone());outer.wm_wrapper=true;outer.children.push(middle);assert!(!supports_ordinary_item(&outer));
        let mut zero=LayoutBox::new(BoxKind::Block,None,style.clone());zero.wm_wrapper=true;zero.children.push(LayoutBox::new(BoxKind::Block,None,style));assert!(supports_ordinary_item(&zero));
    }

    #[test]
    fn budget_failure_precedes_style_or_result_installation() {
        with_item("<span></span>","",|layouter,node,item| {
            let original=node.style.clone();node.intrinsic_cache=Some((10.,20.));
            layouter.grid_work=WorkBudget::limited_for_test(0);
            assert!(catch_unwind(AssertUnwindSafe(||layouter.measure_ordinary_item(node,item,GridAxis::Rows,OperationKind::Layout,area(AvailableSpace::Definite(40.),AvailableSpace::Indefinite),800.))).is_err());
            assert!(Rc::ptr_eq(&node.style,&original));assert_eq!(node.intrinsic_cache,Some((10.,20.)));
        });
    }
}
