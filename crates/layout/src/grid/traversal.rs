//! First real consumer: independent Grid containers with ordinary orthogonal
//! items. Shared axes and recursive subgrid projection remain on the legacy
//! path until their separate consumer is ready.
use std::collections::HashMap;

use super::{subgrid,GridItem,GridSpace,TrackList};
use super::estimate::ItemSpace;
use super::pass::{Frame,Measurements,Target};
use super::phases::{AvailableSpace,AxisLayout,GridAxis,GridOperation,MeasureInputs,MeasurePoint,MinContentProbe,OperationKind,PerpendicularEstimate};
use crate::block::Layouter;
use crate::tree::{Flow,LayoutBox};
use rabbithole_style::{ContentDistribution,ItemAlign};

pub(super) fn ordinary_orthogonal_grid(b:&LayoutBox)->bool {
    if subgrid::requests(b)!=(false,false) {return false;}
    let mut orthogonal=false;
    for child in b.children.iter().filter(|child|child.flow!=Flow::Absolute) {
        // IntrinsicMin's opposite-minimum transfer for replaced/ratio items
        // remains a separate gate. Reject the whole container, including a
        // supported wrapper's unsupported siblings, rather than mix paths.
        if !super::item_measure::supports_ordinary_item(child) {return false;}
        orthogonal|=child.wm_wrapper;
    }
    orthogonal
}


pub(super) fn ordinary_inputs(b:&LayoutBox,spaces:[AvailableSpace;2],gaps:[f32;2])->MeasureInputs {
    let stretch=|axis:usize,align|spaces[axis].definite().is_some()&&matches!(align,ContentDistribution::Stretch|ContentDistribution::FlexStart);
    let layouts=[AxisLayout::new(b.style.justify_content,stretch(0,b.style.justify_content)),AxisLayout::new(b.style.align_content,stretch(1,b.style.align_content))];
    // This independent container has no inherited geometry. Item percentage
    // bases below come from its actual area spans, not these container tags.
    MeasureInputs::new(spaces,[AvailableSpace::Indefinite;2],gaps,layouts,spaces[0],None,None)
}

fn area_basis(estimate:&PerpendicularEstimate)->AvailableSpace {
    match estimate.definite_geometry() {
        Some(subgrid::AxisInheritance::Tracks(axis)) if axis.definite=>AvailableSpace::Definite((axis.extent()-axis.inset_start-axis.inset_end).max(0.)),
        _=>AvailableSpace::Indefinite,
    }
}

fn independent_fixed_span(tracks:&TrackList,start:usize,end:usize,budget:&mut subgrid::WorkBudget)->bool {
    budget.charge(end.saturating_sub(start));
    start<end&&end<=tracks.tracks.len()&&tracks.tracks[start..end].iter().all(|track|
        track.collapsed||matches!((track.min,track.max),(super::Sizing::Fixed(min),super::Sizing::Fixed(max)) if min==max))
}

impl<'a> Layouter<'a> {
    pub(super) fn layout_ordinary_grid_item(&mut self,child:&mut LayoutBox,item:&mut GridItem,x:f32,y:f32,width:f32,height:f32) {
        let bases=[AvailableSpace::Definite(width),AvailableSpace::Definite(height)];
        // Finalization has real area bases. The estimate's finite inline
        // result is an item content width, never a replacement area/base.
        let space=ItemSpace::new(bases,bases,bases[0]);
        let inline=self.estimate_item(child,item,space,GridAxis::Rows,OperationKind::Layout,width).inline.content();
        // Actual final execution also freezes dimensional/edge percentages
        // against the original area axes before WM transposes its numeric Cb.
        // Keep the resulting geometry, then restore the original styles.
        let scope=super::estimate::Scope::new(child,bases,bases[0],GridAxis::Rows,OperationKind::Layout,&mut self.grid_work);
        self.layout_grid_item_with_inline(scope.node,item,x,y,width,height,inline);
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn ordinary_grid_axis_tracks(&mut self,b:&mut LayoutBox,items:&[GridItem],columns:TrackList,rows:TrackList,inputs:MeasureInputs,kind:OperationKind,old_reference:f32)->(TrackList,TrackList) {
        assert!(ordinary_orthogonal_grid(b),"Grid axis consumer requires independent ordinary orthogonal items");
        assert!(columns.inherited.is_none()&&rows.inherited.is_none(),"Inherited Grid tracks require the recursive consumer");
        let templates=[columns,rows];
        let mut operation=GridOperation::new(kind,&mut self.grid_work);
        let mut used:[Option<TrackList>;2]=[None,None];
        for point in [MeasurePoint::Columns,MeasurePoint::Rows,MeasurePoint::AdjustColumns,MeasurePoint::AdjustRows] {
            let axis=inputs.local_axis(point);let index=axis.index();
            self.grid_work.tracks(&templates[index]);self.grid_work.tracks(&templates[axis.opposite().index()]);
            let tracks=templates[index].clone();let opposite=templates[axis.opposite().index()].clone();
            let space=inputs.spaces[index].constraint().unwrap_or(GridSpace::MaxContent);
            let result=self.grid_axis_pass(&mut operation,point,inputs.clone(),Target::standalone(tracks,space,inputs.layouts[index].stretch),opposite,|this,operation,frame,tracks| {
                this.ordinary_axis_measurements(b,items,operation,frame,tracks,kind,old_reference)
            });
            // Only the requested completed axis is installed. An estimate
            // cannot enter this used-geometry array.
            used[index]=Some(result.completion.expect("ordinary axis must complete").into_tracks());
        }
        (used[0].take().unwrap(),used[1].take().unwrap())
    }

    fn ordinary_axis_measurements(&mut self,b:&mut LayoutBox,items:&[GridItem],operation:&GridOperation,frame:&Frame,tracks:&TrackList,kind:OperationKind,old_reference:f32)->Measurements {
        let axis=frame.axis;let index=axis.index();
        let previous=if operation.completion_matches(&frame.id,tracks,&mut self.grid_work)==Some(true) {
            operation.same_axis_history(&frame.id).and_then(|record|record.completion())
        } else {None};
        self.grid_work.charge(items.len().saturating_mul(6));
        let mut measured=Vec::with_capacity(items.len());
        let mut baselines:HashMap<i32,f32>=HashMap::new();
        for item in items {
            let pair=|axis|if axis==GridAxis::Columns {item.area.col} else {item.area.row};
            let (start,end)=pair(axis);let (cross_start,cross_end)=pair(axis.opposite());
            let names=if axis==GridAxis::Columns {item.column_names} else {item.row_names};
            let own=if let Some(previous)=previous {
                PerpendicularEstimate::from_used(previous,start.max(0)as usize,end.max(0)as usize,frame.inputs.gaps[index],names,previous.definite_space(),frame.inputs.layouts[index].align,&mut self.grid_work)
            } else {
                PerpendicularEstimate::from_tracks(tracks,start.max(0)as usize,end.max(0)as usize,frame.inputs.gaps[index],names,frame.inputs.spaces[index].definite(),frame.inputs.layouts[index].align,&mut self.grid_work)
            };
            let opposite=frame.opposite.span(cross_start.max(0)as usize,cross_end.max(0)as usize,if axis==GridAxis::Columns {item.row_names} else {item.column_names},&mut self.grid_work);
            let mut bases=[AvailableSpace::Indefinite;2];bases[index]=area_basis(&own);bases[axis.opposite().index()]=area_basis(&opposite);
            let constraints=bases;
            // Numeric own-axis geometry does not make a cyclic contribution
            // basis independent. This applies to auto rows as well as columns,
            // even inside a definite container. Opposite used geometry still
            // supplies the requested cross-axis measurement basis.
            if !independent_fixed_span(tracks,start.max(0)as usize,end.max(0)as usize,&mut self.grid_work) {
                bases[index]=AvailableSpace::Indefinite;
            }
            let space=ItemSpace::new(constraints,bases,bases[0]);
            let measurement=self.measure_ordinary_item(&mut b.children[item.idx],item,axis,kind,space,old_reference);
            if axis==GridAxis::Rows&&item.align==ItemAlign::Baseline {
                if let Some(baseline)=measurement.baseline {
                    if let Some(max)=baselines.get_mut(&item.area.row.0) {*max=max.max(baseline);}
                    else {self.grid_work.charge(1);baselines.insert(item.area.row.0,baseline);}
                }
            }
            measured.push((item,measurement));
        }
        let mut out=Measurements::default();
        for (item,mut measurement) in measured {
            if axis==GridAxis::Rows&&item.align==ItemAlign::Baseline {
                if let Some(baseline)=measurement.baseline {
                    let shift=baselines[&item.area.row.0]-baseline;
                    measurement.contribution.min+=shift;measurement.contribution.max+=shift;measurement.true_min_content+=shift;
                }
            }
            let probe=MinContentProbe::new(&[item.idx],measurement.true_min_content,&mut self.grid_work);
            out.push(measurement.contribution,probe,&mut self.grid_work);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{BoxKind,Formatting};
    use rabbithole_style::{ComputedStyle,TrackListItem};
    use std::rc::Rc;

    fn node(kind:BoxKind)->LayoutBox {LayoutBox::new(kind,None,Rc::new(ComputedStyle::initial()))}
    fn wrapper(inner:LayoutBox)->LayoutBox {
        let mut wrapper=node(BoxKind::Block);wrapper.wm_wrapper=true;wrapper.children.push(inner);wrapper
    }
    fn grid(children:Vec<LayoutBox>)->LayoutBox {
        let mut grid=node(BoxKind::Block);grid.formatting=Formatting::Grid;grid.children=children;grid
    }

    #[test]
    fn initial_consumer_requires_an_orthogonal_item_and_excludes_unsupported_siblings() {
        assert!(!ordinary_orthogonal_grid(&grid(vec![node(BoxKind::Block)])));
        assert!(ordinary_orthogonal_grid(&grid(vec![wrapper(node(BoxKind::Block)),node(BoxKind::Block)])));
        assert!(!ordinary_orthogonal_grid(&grid(vec![wrapper(node(BoxKind::Block)),node(BoxKind::Image)])));
        assert!(!ordinary_orthogonal_grid(&grid(vec![wrapper(node(BoxKind::Image))])));
        let mut ratio=node(BoxKind::Block);let mut style=(*ratio.style).clone();style.aspect_ratio=Some(2.);ratio.style=Rc::new(style);
        assert!(!ordinary_orthogonal_grid(&grid(vec![wrapper(node(BoxKind::Block)),wrapper(ratio)])));
    }

    #[test]
    fn inherited_axes_and_incomplete_wrapper_chains_stay_out_of_the_ordinary_consumer() {
        let mut shared=grid(vec![]);let mut style=(*shared.style).clone();
        style.grid_template_columns=vec![TrackListItem::Subgrid{line_names:vec![]}];shared.style=Rc::new(style);
        assert!(!ordinary_orthogonal_grid(&grid(vec![wrapper(shared)])));
        let mut empty=node(BoxKind::Block);empty.wm_wrapper=true;
        assert!(!ordinary_orthogonal_grid(&grid(vec![empty])));
        let mut deep=node(BoxKind::Block);
        for _ in 0..129 {deep=wrapper(deep);}
        assert!(!ordinary_orthogonal_grid(&grid(vec![deep])));
    }
}
