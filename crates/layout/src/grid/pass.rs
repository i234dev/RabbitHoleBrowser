//! Execute one parent-axis pass without completing the opposite axis.
//!
//! Traversal/hoisting is supplied by the caller. The legacy layout traversal
//! is not connected to this dispatcher in this checkpoint.
use crate::block::Layouter;
use rabbithole_style::ContentDistribution;
use super::{Contribution,GridSpace,TrackList};
use super::phases::{CompletedAxis,GridAxis,GridOperation,MeasureInputs,MeasurePoint,MeasurementId,MinContentProbe,PassRole,PerpendicularEstimate};
use super::subgrid::{self,WorkBudget};

pub(super) enum Target {
    Shared(TrackList),
    Standalone{tracks:TrackList,space:GridSpace,stretch:bool},
}
impl Target {
    pub(super) fn shared(tracks:TrackList)->Self {
        assert!(tracks.inherited.is_some(),"Shared Grid pass requires an inherited axis");Self::Shared(tracks)
    }
    pub(super) fn standalone(tracks:TrackList,space:GridSpace,stretch:bool)->Self {
        assert!(tracks.inherited.is_none(),"Standalone Grid pass cannot size inherited tracks");Self::Standalone{tracks,space,stretch}
    }
    fn tracks(&self)->&TrackList {match self {Self::Shared(tracks)|Self::Standalone{tracks,..}=>tracks}}
}

#[derive(Default)]
pub(super) struct Measurements {
    pub(super) contributions:Vec<Contribution>,
    pub(super) min_content:Vec<MinContentProbe>,
}
impl Measurements {
    pub(super) fn push(&mut self,contribution:Contribution,probe:MinContentProbe,budget:&mut WorkBudget) {
        budget.charge(4);self.contributions.push(contribution);self.min_content.push(probe);
    }
}

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) enum OppositeOrigin { CurrentInherited, Recorded{point:MeasurePoint,axis:GridAxis,role:PassRole}, InitialEstimate }

enum OppositeTracks { Used(CompletedAxis), Estimate(TrackList) }
pub(super) struct Opposite {
    tracks:OppositeTracks,
    pub(super) origin:OppositeOrigin,
    /// These are the inputs of the geometry's producer, which may precede
    /// the fresh observation. They are never relabeled as current inputs.
    pub(super) producer_inputs:MeasureInputs,
    axis:GridAxis,
    align:ContentDistribution,
}
impl Opposite {
    pub(super) fn span(&self,start:usize,end:usize,names:bool,budget:&mut WorkBudget)->PerpendicularEstimate {
        let gap=self.producer_inputs.gaps[self.axis.index()];
        match &self.tracks {
            OppositeTracks::Used(completed)=>PerpendicularEstimate::from_used(completed,start,end,gap,names,completed.definite_space(),self.align,budget),
            OppositeTracks::Estimate(tracks)=>PerpendicularEstimate::from_tracks(tracks,start,end,gap,names,self.producer_inputs.spaces[self.axis.index()].definite(),self.align,budget),
        }
    }
}

pub(super) struct Frame {
    pub(super) id:MeasurementId,
    pub(super) point:MeasurePoint,
    pub(super) axis:GridAxis,
    pub(super) role:PassRole,
    pub(super) inputs:MeasureInputs,
    pub(super) opposite:Opposite,
}

pub(super) struct PassResult {
    pub(super) id:MeasurementId,
    pub(super) measurements:Measurements,
    pub(super) completion:Option<CompletedAxis>,
    pub(super) min_content_changed:bool,
}

fn retained_inputs(inputs:&MeasureInputs,budget:&mut WorkBudget)->MeasureInputs {
    budget.charge(subgrid::source_work(&inputs.columns).saturating_add(subgrid::source_work(&inputs.rows)));
    inputs.clone()
}

impl<'a> Layouter<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn grid_axis_pass(&mut self,operation:&mut GridOperation,point:MeasurePoint,inputs:MeasureInputs,target:Target,opposite:TrackList,opposite_align:ContentDistribution,collect:impl FnOnce(&mut Self,&mut GridOperation,&Frame,&TrackList)->Measurements)->PassResult {
        self.grid_axis_pass_inner(operation,point,None,inputs,target,opposite,opposite_align,collect)
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn grid_axis_prerequisite(&mut self,operation:&mut GridOperation,point:MeasurePoint,axis:GridAxis,inputs:MeasureInputs,target:Target,opposite:TrackList,opposite_align:ContentDistribution,collect:impl FnOnce(&mut Self,&mut GridOperation,&Frame,&TrackList)->Measurements)->PassResult {
        assert!(matches!(&target,Target::Standalone{..}),"Grid prerequisite cannot size an inherited axis");
        self.grid_axis_pass_inner(operation,point,Some(axis),inputs,target,opposite,opposite_align,collect)
    }
    #[allow(clippy::too_many_arguments)]
    fn grid_axis_pass_inner(&mut self,operation:&mut GridOperation,point:MeasurePoint,prerequisite:Option<GridAxis>,inputs:MeasureInputs,target:Target,opposite:TrackList,opposite_align:ContentDistribution,collect:impl FnOnce(&mut Self,&mut GridOperation,&Frame,&TrackList)->Measurements)->PassResult {
        let axis=prerequisite.unwrap_or_else(||inputs.local_axis(point));let opposite_axis=axis.opposite();
        let role=if prerequisite.is_some() {PassRole::Prerequisite} else {PassRole::Requested};
        if let Target::Standalone{space,..}=&target {
            let available=inputs.spaces[axis.index()].constraint().unwrap_or(GridSpace::MaxContent);
            assert_eq!(*space,available,"Grid target and available space disagree");
        }
        let retained=retained_inputs(&inputs,&mut self.grid_work);
        let id=if prerequisite.is_some() {operation.begin_prerequisite(point,axis,retained,&mut self.grid_work)} else {operation.begin(point,retained,&mut self.grid_work)};
        let previous=operation.opposite_history(&id).and_then(|record|record.completion().zip(record.completion_origin()));
        let opposite=if opposite.inherited.as_ref().is_some_and(|source|source.definite) {
            Opposite{tracks:OppositeTracks::Used(CompletedAxis::inherited(opposite,&mut self.grid_work)),origin:OppositeOrigin::CurrentInherited,producer_inputs:retained_inputs(&inputs,&mut self.grid_work),axis:opposite_axis,align:opposite_align}
        } else if let Some((completed,origin))=previous {
            assert_eq!(origin.axis,opposite_axis,"Opposite Grid history changed local axes");
            Opposite{tracks:OppositeTracks::Used(completed.retained_copy(&mut self.grid_work)),origin:OppositeOrigin::Recorded{point:origin.point,axis:origin.axis,role:origin.role},producer_inputs:retained_inputs(origin.inputs,&mut self.grid_work),axis:opposite_axis,align:opposite_align}
        } else {
            Opposite{tracks:OppositeTracks::Estimate(opposite),origin:OppositeOrigin::InitialEstimate,producer_inputs:retained_inputs(&inputs,&mut self.grid_work),axis:opposite_axis,align:opposite_align}
        };
        let frame=Frame{id:id.clone(),point,axis,role,inputs,opposite};
        let measurements=collect(self,operation,&frame,target.tracks());
        let changed=operation.min_content_changed(&id,&measurements.min_content,&mut self.grid_work);
        let completion=match target {
            Target::Shared(_)=>{
                operation.record_projection(&id,&measurements.contributions,&measurements.min_content,&mut self.grid_work);None
            }
            Target::Standalone{tracks,space,stretch}=>{
                if role==PassRole::Requested&&point.is_adjustment()&&changed==Some(false)&&operation.same_axis_history(&id).and_then(|record|record.completion()).is_some() {
                    operation.observe_unchanged(&id,&measurements.contributions,&measurements.min_content,&mut self.grid_work);
                    Some(operation.result(&id).unwrap().completion().unwrap().retained_copy(&mut self.grid_work))
                } else {
                    let completed=CompletedAxis::standalone(tracks,&measurements.contributions,space,frame.inputs.gaps[axis.index()],stretch,&mut self.grid_work);
                    operation.record(&id,&completed,&measurements.contributions,&measurements.min_content,&mut self.grid_work);Some(completed)
                }
            }
        };
        PassResult{id,measurements,completion,min_content_changed:changed.unwrap_or(false)}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{Sizing,Track};
    use super::super::phases::{AvailableSpace,OperationKind};
    use super::super::subgrid::AxisInheritance;
    use crate::tree::{BoxKind,LayoutBox,WmKind};
    use rabbithole_gfx::FontContext;
    use std::collections::HashMap;
    use std::panic::{catch_unwind,AssertUnwindSafe};
    use std::rc::Rc;

    fn with_layouter(test:impl FnOnce(&mut Layouter<'_>)) {
        let doc=rabbithole_html::parse("<!doctype html><body></body>");
        let styles=rabbithole_style::style_tree(&doc,&[],&rabbithole_css::MediaEnv{width:800.,height:600.,screen:true,scripting:true});
        let fonts=FontContext::new();let images=HashMap::new();let backgrounds=HashMap::new();let scrolls=HashMap::new();
        let ctx=crate::LayoutContext{doc:&doc,styles:&styles,fonts:&fonts,images:&images,background_images:&backgrounds,scroll_offsets:&scrolls,viewport_width:800.,viewport_height:600.,page_scroll:(0.,0.)};
        test(&mut Layouter::new(&ctx));
    }
    fn list(sizes:&[(Sizing,Sizing)])->TrackList {
        TrackList{tracks:sizes.iter().map(|&(min,max)|Track::new(min,max)).collect(),names:(0..=sizes.len()).map(|i|vec![format!("line-{i}")]).collect(),inherited:None}
    }
    fn automatic(count:usize)->TrackList {list(&vec![(Sizing::Auto,Sizing::Auto);count])}
    fn shared()->TrackList {
        let original=automatic(1);
        let AxisInheritance::Tracks(source)=subgrid::sizing_section_aligned(&original,0,1,0.,true,None,ContentDistribution::FlexStart) else {panic!("missing source")};
        TrackList{tracks:vec![Track::new(Sizing::Fixed(0.),Sizing::Fixed(0.))],names:source.names.clone(),inherited:Some(source)}
    }
    fn inherited(size:f32)->TrackList {
        let mut original=list(&[(Sizing::Fixed(size),Sizing::Fixed(size))]);original.tracks[0].base=size;
        let AxisInheritance::Tracks(source)=subgrid::section(&original,&[0.],0,1,0.,true) else {panic!("missing source")};
        TrackList{tracks:original.tracks,names:source.names.clone(),inherited:Some(source)}
    }
    fn inputs(spaces:[AvailableSpace;2])->MeasureInputs {
        MeasureInputs::new(spaces,[AvailableSpace::Definite(300.),AvailableSpace::Definite(200.)],[0.,0.],AvailableSpace::Definite(300.),None,None)
    }
    fn sample(layouter:&mut Layouter<'_>,contributions:&[Contribution],minimums:&[f32])->Measurements {
        assert_eq!(contributions.len(),minimums.len());let mut result=Measurements::default();
        for (i,(&contribution,&minimum)) in contributions.iter().zip(minimums).enumerate() {
            let probe=MinContentProbe::new(&[i],minimum,&mut layouter.grid_work);
            result.push(contribution,probe,&mut layouter.grid_work);
        }
        result
    }
    fn column_contributions()->[Contribution;2] {
        [Contribution{start:0,end:1,min:100.,max:100.},Contribution{start:1,end:2,min:10.,max:48.}]
    }
    fn used_sizes(result:&PassResult)->Vec<f32> {result.completion.as_ref().unwrap().tracks().tracks.iter().map(|track|track.base).collect()}
    fn panic_text(error:&(dyn std::any::Any+Send))->&str {
        if let Some(text)=error.downcast_ref::<String>() {text}
        else if let Some(text)=error.downcast_ref::<&str>() {text}
        else {"non-text panic"}
    }

    #[test]
    fn orthogonal_rows_first_estimates_then_completes_only_the_parent_requested_axis() {
        with_layouter(|layouter| {
            let mut operation=GridOperation::new(OperationKind::Layout,&mut layouter.grid_work);
            let inputs=inputs([AvailableSpace::MaxContent,AvailableSpace::Definite(130.)]).through_wrapper(WmKind::Cw,&mut layouter.grid_work);
            let first=layouter.grid_axis_pass(&mut operation,MeasurePoint::Columns,inputs.clone(),Target::shared(shared()),automatic(2),ContentDistribution::FlexStart,|this,_,frame,_| {
                assert_eq!(frame.axis,GridAxis::Rows);assert_eq!(frame.opposite.origin,OppositeOrigin::InitialEstimate);
                assert!(frame.opposite.span(0,2,true,&mut this.grid_work).is_infinite());
                sample(this,&[Contribution{start:0,end:1,min:20.,max:20.}],&[20.])
            });
            assert!(first.completion.is_none());
            let columns=layouter.grid_axis_pass(&mut operation,MeasurePoint::Rows,inputs.clone(),Target::standalone(automatic(2),GridSpace::Definite(130.),false),inherited(20.),ContentDistribution::FlexStart,|this,_,frame,_| {
                assert_eq!(frame.axis,GridAxis::Columns);assert_eq!(frame.opposite.origin,OppositeOrigin::CurrentInherited);
                sample(this,&column_contributions(),&[100.,10.])
            });
            assert_eq!(used_sizes(&columns),[100.,30.]);
            let rows=layouter.grid_axis_pass(&mut operation,MeasurePoint::AdjustColumns,inputs.clone(),Target::shared(shared()),automatic(2),ContentDistribution::FlexStart,|this,_,frame,_| {
                assert_eq!(frame.opposite.origin,OppositeOrigin::Recorded{point:MeasurePoint::Rows,axis:GridAxis::Columns,role:PassRole::Requested});
                let geometry=frame.opposite.span(0,2,true,&mut this.grid_work);
                let Some(AxisInheritance::Tracks(axis))=geometry.definite_geometry() else {panic!("missing used columns")};
                assert_eq!(axis.extent(),130.);assert_eq!(axis.sizes,[100.,30.]);
                sample(this,&[Contribution{start:0,end:1,min:25.,max:25.}],&[25.])
            });
            assert!(rows.min_content_changed);assert!(rows.completion.is_none());
            let unchanged=layouter.grid_axis_pass(&mut operation,MeasurePoint::AdjustRows,inputs,Target::standalone(automatic(2),GridSpace::Definite(130.),false),inherited(25.),ContentDistribution::FlexStart,|this,_,_,_| {
                let mut contributions=column_contributions();contributions[1].max=100.;
                sample(this,&contributions,&[100.,10.])
            });
            assert!(!unchanged.min_content_changed);assert_eq!(used_sizes(&unchanged),[100.,30.]);
            assert_eq!(unchanged.completion.as_ref().unwrap().tracks().tracks[1].limit,Some(48.));
            assert_eq!(operation.result(&unchanged.id).unwrap().completion_origin().unwrap().point,MeasurePoint::Rows);
        });
    }

    #[test]
    fn standalone_passes_keep_min_max_and_definite_completion_constraints_distinct() {
        with_layouter(|layouter| {
            for (space,available,expected) in [(GridSpace::MinContent,AvailableSpace::MinContent,vec![100.,10.]),(GridSpace::MaxContent,AvailableSpace::MaxContent,vec![100.,48.]),(GridSpace::Definite(130.),AvailableSpace::Definite(130.),vec![100.,30.])] {
                let mut operation=GridOperation::new(OperationKind::Layout,&mut layouter.grid_work);
                let result=layouter.grid_axis_pass(&mut operation,MeasurePoint::Columns,inputs([available,AvailableSpace::Indefinite]),Target::standalone(automatic(2),space,false),automatic(1),ContentDistribution::FlexStart,|this,_,_,_|sample(this,&column_contributions(),&[100.,10.]));
                assert_eq!(used_sizes(&result),expected);
            }
        });
    }

    #[test]
    fn repeated_shape_gets_fresh_opposite_distribution_and_containing_inputs() {
        with_layouter(|layouter| {
            let mut operation=GridOperation::new(OperationKind::Layout,&mut layouter.grid_work);
            let mut previous=None;
            for (height,start) in [(300.,250.),(400.,350.)] {
                let mut input=inputs([AvailableSpace::Definite(20.),AvailableSpace::Definite(height)]);input.containing[1]=AvailableSpace::Definite(height);
                let result=layouter.grid_axis_pass(&mut operation,MeasurePoint::Columns,input,Target::standalone(list(&[(Sizing::Fixed(20.),Sizing::Fixed(20.))]),GridSpace::Definite(20.),false),list(&[(Sizing::Fixed(50.),Sizing::Fixed(50.));2]),ContentDistribution::SpaceBetween,|this,_,frame,_| {
                    if let Some(old)=&previous {assert_ne!(&frame.id,old);}
                    assert_eq!(frame.inputs.containing[1],AvailableSpace::Definite(height));
                    let geometry=frame.opposite.span(0,2,true,&mut this.grid_work);
                    let Some(AxisInheritance::Tracks(axis))=geometry.definite_geometry() else {panic!("missing fixed estimate")};
                    assert_eq!(axis.starts,[0.,start]);Measurements::default()
                });
                previous=Some(result.id);
            }
        });
    }

    #[test]
    fn inherited_opposite_uses_its_actual_geometry_instead_of_estimated_bases() {
        with_layouter(|layouter| {
            let mut operation=GridOperation::new(OperationKind::Layout,&mut layouter.grid_work);
            let result=layouter.grid_axis_pass(&mut operation,MeasurePoint::Rows,inputs([AvailableSpace::Definite(90.),AvailableSpace::MaxContent]),Target::standalone(automatic(1),GridSpace::MaxContent,false),inherited(90.),ContentDistribution::FlexStart,|this,_,frame,_| {
                assert_eq!(frame.opposite.origin,OppositeOrigin::CurrentInherited);
                let geometry=frame.opposite.span(0,1,true,&mut this.grid_work);
                let Some(AxisInheritance::Tracks(axis))=geometry.definite_geometry() else {panic!("missing inherited size")};
                assert_eq!(axis.extent(),90.);sample(this,&[Contribution{start:0,end:1,min:10.,max:10.}],&[10.])
            });
            assert_eq!(used_sizes(&result),[10.]);
        });
    }

    #[test]
    fn failure_in_a_collector_restores_scope_and_installs_no_axis_result() {
        with_layouter(|layouter| {
            let mut operation=GridOperation::new(OperationKind::Layout,&mut layouter.grid_work);
            let mut style=rabbithole_style::ComputedStyle::initial();style.width=rabbithole_style::Length::Percent(50.);
            let original=Rc::new(style);let mut node=LayoutBox::new(BoxKind::Block,None,original.clone());let mut id=None;
            let failure=catch_unwind(AssertUnwindSafe(|| {
                layouter.grid_axis_pass(&mut operation,MeasurePoint::Columns,inputs([AvailableSpace::MaxContent,AvailableSpace::Indefinite]),Target::standalone(automatic(1),GridSpace::MaxContent,false),automatic(1),ContentDistribution::FlexStart,|this,_,frame,_| {
                    id=Some(frame.id.clone());
                    let scope=super::super::estimate::Scope::new(&mut node,[AvailableSpace::Indefinite;2],AvailableSpace::Indefinite,GridAxis::Columns,OperationKind::IntrinsicMax,&mut this.grid_work);
                    scope.node.intrinsic_cache=Some((100.,100.));
                    this.grid_work.charge(usize::MAX);Measurements::default()
                })
            }));
            assert!(failure.is_err());assert!(operation.result(id.as_ref().unwrap()).is_none());
            assert!(Rc::ptr_eq(&node.style,&original));assert!(node.intrinsic_cache.is_none());
        });
    }

    #[test]
    fn a_new_virtual_slot_can_first_complete_at_the_current_parent_adjustment_point() {
        with_layouter(|layouter| {
            let mut operation=GridOperation::new(OperationKind::Layout,&mut layouter.grid_work);
            let result=layouter.grid_axis_pass(&mut operation,MeasurePoint::AdjustColumns,inputs([AvailableSpace::MaxContent,AvailableSpace::Indefinite]),Target::standalone(automatic(1),GridSpace::MaxContent,false),automatic(1),ContentDistribution::FlexStart,|this,_,_,_|sample(this,&[Contribution{start:0,end:1,min:10.,max:40.}],&[10.]));
            assert_eq!(used_sizes(&result),[40.]);
            assert_eq!(operation.result(&result.id).unwrap().completion_origin().unwrap().point,MeasurePoint::AdjustColumns);
        });
    }

    #[test]
    fn a_second_parent_pass_can_bootstrap_only_the_earlier_local_axis_in_either_orientation() {
        with_layouter(|layouter| {
            for orthogonal in [false,true] {
                let mut operation=GridOperation::new(OperationKind::Layout,&mut layouter.grid_work);
                let mut input=inputs([AvailableSpace::Definite(130.),AvailableSpace::MaxContent]);
                if orthogonal {input=input.through_wrapper(WmKind::Cw,&mut layouter.grid_work);}
                let first=input.local_axis(MeasurePoint::Columns);let second=input.local_axis(MeasurePoint::Rows);
                let prerequisite=layouter.grid_axis_prerequisite(&mut operation,MeasurePoint::Rows,first,input.clone(),Target::standalone(automatic(2),GridSpace::Definite(130.),false),shared(),ContentDistribution::FlexStart,|this,_,frame,_| {
                    assert_eq!(frame.point,MeasurePoint::Rows);assert_eq!(frame.axis,first);assert_eq!(frame.role,PassRole::Prerequisite);
                    assert!(frame.opposite.span(0,1,true,&mut this.grid_work).is_infinite());
                    sample(this,&column_contributions(),&[100.,10.])
                });
                assert_eq!(used_sizes(&prerequisite),[100.,30.]);
                let origin=operation.result(&prerequisite.id).unwrap().completion_origin().unwrap();
                assert_eq!(origin.point,MeasurePoint::Rows);assert_eq!(origin.axis,first);assert_eq!(origin.role,PassRole::Prerequisite);
                let requested=layouter.grid_axis_pass(&mut operation,MeasurePoint::Rows,input,Target::shared(shared()),automatic(2),ContentDistribution::FlexStart,|this,_,frame,_| {
                    assert_eq!(frame.point,MeasurePoint::Rows);assert_eq!(frame.axis,second);assert_eq!(frame.role,PassRole::Requested);
                    assert_eq!(frame.opposite.origin,OppositeOrigin::Recorded{point:MeasurePoint::Rows,axis:first,role:PassRole::Prerequisite});
                    let geometry=frame.opposite.span(0,2,true,&mut this.grid_work);
                    let Some(AxisInheritance::Tracks(axis))=geometry.definite_geometry() else {panic!("missing prerequisite result")};
                    assert_eq!(axis.extent(),130.);sample(this,&[Contribution{start:0,end:1,min:20.,max:20.}],&[20.])
                });
                assert!(requested.completion.is_none());
            }
        });
    }

    #[test]
    fn a_first_parent_pass_cannot_bootstrap_the_later_axis_in_either_orientation() {
        with_layouter(|layouter| {
            for orthogonal in [false,true] {
                let mut operation=GridOperation::new(OperationKind::Layout,&mut layouter.grid_work);
                let mut input=inputs([AvailableSpace::Definite(130.),AvailableSpace::MaxContent]);
                if orthogonal {input=input.through_wrapper(WmKind::Cw,&mut layouter.grid_work);}
                let later=input.local_axis(MeasurePoint::Rows);let mut collected=false;
                let failure=catch_unwind(AssertUnwindSafe(|| {
                    layouter.grid_axis_prerequisite(&mut operation,MeasurePoint::Columns,later,input,Target::standalone(automatic(1),GridSpace::MaxContent,false),automatic(1),ContentDistribution::FlexStart,|_,_,_,_| {collected=true;Measurements::default()})
                }));
                let Err(error)=failure else {panic!("premature Grid prerequisite was accepted")};
                assert!(panic_text(error.as_ref()).contains("later parent axis prematurely"));assert!(!collected);
            }
        });
    }

    #[test]
    fn a_prerequisite_cannot_be_reused_to_resize_an_already_completed_axis() {
        with_layouter(|layouter| {
            let mut operation=GridOperation::new(OperationKind::Layout,&mut layouter.grid_work);
            let input=inputs([AvailableSpace::MaxContent,AvailableSpace::MaxContent]);
            layouter.grid_axis_prerequisite(&mut operation,MeasurePoint::Rows,GridAxis::Columns,input.clone(),Target::standalone(automatic(1),GridSpace::MaxContent,false),shared(),ContentDistribution::FlexStart,|this,_,_,_|sample(this,&[Contribution{start:0,end:1,min:10.,max:40.}],&[10.]));
            let failure=catch_unwind(AssertUnwindSafe(|| {
                layouter.grid_axis_prerequisite(&mut operation,MeasurePoint::Rows,GridAxis::Columns,input,Target::standalone(automatic(1),GridSpace::MaxContent,false),shared(),ContentDistribution::FlexStart,|this,_,_,_|sample(this,&[Contribution{start:0,end:1,min:10.,max:80.}],&[10.]))
            }));
            let Err(error)=failure else {panic!("completed Grid axis was resized through prerequisite")};
            assert!(panic_text(error.as_ref()).contains("cannot re-size a completed axis"));
        });
    }
}
