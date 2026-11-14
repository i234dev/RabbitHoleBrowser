//! Caller-owned history for one per-axis Grid sizing operation.
//!
//! This checkpoint does not select a new used size. Estimates, intrinsic
//! constraints and completed axis results remain different types. Results
//! cannot be looked up by placement shape or reused with another input id.
use std::rc::Rc;

use super::{Contribution, GridItem, GridSpace, TrackList};
use super::subgrid::{self, AxisInheritance, WorkBudget};
use crate::tree::WmKind;
use rabbithole_style::ContentDistribution;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GridAxis { Columns, Rows }
impl GridAxis {
    pub(super) fn index(self)->usize {match self {Self::Columns=>0,Self::Rows=>1}}
    pub(super) fn opposite(self)->Self {match self {Self::Columns=>Self::Rows,Self::Rows=>Self::Columns}}
}

/// These are the passes of Grid2 §12.1, not the steps within track sizing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MeasurePoint { Columns, Rows, AdjustColumns, AdjustRows }
impl MeasurePoint {
    fn rank(self)->u8 {match self {Self::Columns=>0,Self::Rows=>1,Self::AdjustColumns=>2,Self::AdjustRows=>3}}
    pub(super) fn axis(self)->GridAxis {match self {Self::Columns|Self::AdjustColumns=>GridAxis::Columns,_=>GridAxis::Rows}}
    pub(super) fn is_adjustment(self)->bool {matches!(self,Self::AdjustColumns|Self::AdjustRows)}
    pub(super) fn legacy_phase(self)->super::MeasurePhase {if self.is_adjustment() {super::MeasurePhase::Adjusted} else {super::MeasurePhase::Initial}}
}

/// Operation identity isolates intrinsic-min, intrinsic-max and final layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OperationKind { IntrinsicMin, IntrinsicMax, Layout }

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum AvailableSpace {
    Definite(f32),
    MinContent,
    MaxContent,
    Indefinite,
}
impl AvailableSpace {
    pub(super) fn constraint(self)->Option<GridSpace> {
        match self {
            Self::Definite(size)=>Some(GridSpace::Definite(size)),
            Self::MinContent=>Some(GridSpace::MinContent),
            Self::MaxContent=>Some(GridSpace::MaxContent),
            Self::Indefinite=>None,
        }
    }
    fn validate(self) {
        if let Self::Definite(size)=self {assert!(size.is_finite()&&size>=0.,"Invalid definite Grid measurement space");}
    }
    pub(super) fn definite(self)->Option<f32> {if let Self::Definite(value)=self {Some(value)} else {None}}
}

/// An estimate of the perpendicular span is not an intrinsic constraint.
/// Unknown tracks retain line metadata, but expose no numeric used geometry.
#[derive(Clone, Debug)]
pub(super) struct PerpendicularEstimate {
    line_names:Vec<Vec<String>>,
    definite:Option<AxisInheritance>,
}
impl PerpendicularEstimate {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn from_tracks(list:&TrackList,start:usize,end:usize,gap:f32,names:bool,available:Option<f32>,align:ContentDistribution,budget:&mut WorkBudget)->Self {
        budget.tracks(list);
        budget.tracks(list);
        let end=end.min(list.tracks.len());let start=start.min(end);
        let line_names=if names {list.names[start..=end].to_vec()} else {vec![Vec::new();end-start+1]};
        let source=(start<end).then(||subgrid::fixed_section_aligned(list,start,end,gap,names,available,align)).flatten();
        let definite=source.filter(|source|matches!(source,AxisInheritance::Tracks(axis) if axis.definite));
        Self{line_names,definite}
    }
    pub(super) fn is_infinite(&self)->bool {self.definite.is_none()}
    pub(super) fn line_names(&self)->&[Vec<String>] {&self.line_names}
    pub(super) fn definite_geometry(&self)->Option<&AxisInheritance> {self.definite.as_ref()}
    #[allow(clippy::too_many_arguments)]
    pub(super) fn from_used(completed:&CompletedAxis,start:usize,end:usize,gap:f32,names:bool,available:Option<f32>,align:ContentDistribution,budget:&mut WorkBudget)->Self {
        let list=completed.tracks();
        budget.tracks(list);budget.tracks(list);
        let end=end.min(list.tracks.len());let start=start.min(end);
        let line_names=if names {list.names[start..=end].to_vec()} else {vec![Vec::new();end-start+1]};
        let definite=if start<end {
            let (positions,_)=subgrid::positions(list,gap,available,align);
            Some(subgrid::section(list,&positions,start,end,gap,names))
        } else {None};
        Self{line_names,definite}
    }
}

/// Geometry is supplied afresh at every entry, including when track counts
/// and placement did not change. The root measurement point is never swapped;
/// an orthogonal wrapper swaps only its interpretation in the local axes.
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) struct AxisLayout {
    pub(super) align:ContentDistribution,
    pub(super) stretch:bool,
}
impl AxisLayout {
    pub(super) fn new(align:ContentDistribution,stretch:bool)->Self {Self{align,stretch}}
}

#[derive(Clone, Debug)]
pub(super) struct MeasureInputs {
    pub(super) spaces:[AvailableSpace;2],
    /// Containing-block bases remain separate from this grid's available
    /// intrinsic/used spaces. A parent area can be definite while this
    /// container is being measured under an intrinsic constraint.
    pub(super) containing:[AvailableSpace;2],
    pub(super) gaps:[f32;2],
    /// Distribution and auto-track stretching belong to the same producer as
    /// its geometry. They cannot be supplied later from a different style.
    pub(super) layouts:[AxisLayout;2],
    /// Original containing-block inline base for margins/padding. It is
    /// separate from local available spaces and is not swapped by a wrapper.
    pub(super) edge_basis:AvailableSpace,
    pub(super) columns:Option<AxisInheritance>,
    pub(super) rows:Option<AxisInheritance>,
    swapped:bool,
}
impl MeasureInputs {
    pub(super) fn new(spaces:[AvailableSpace;2],containing:[AvailableSpace;2],gaps:[f32;2],layouts:[AxisLayout;2],edge_basis:AvailableSpace,columns:Option<AxisInheritance>,rows:Option<AxisInheritance>)->Self {
        for space in spaces {space.validate();}
        for space in containing {space.validate();}
        edge_basis.validate();
        Self{spaces,containing,gaps,layouts,edge_basis,columns,rows,swapped:false}
    }
    pub(super) fn local_axis(&self,point:MeasurePoint)->GridAxis {
        if self.swapped {point.axis().opposite()} else {point.axis()}
    }
    pub(super) fn through_wrapper(&self,kind:WmKind,budget:&mut WorkBudget)->Self {
        budget.charge(subgrid::source_work(&self.columns).saturating_add(subgrid::source_work(&self.rows)).saturating_mul(2));
        let (columns,rows)=subgrid::through_wrapper(self.columns.clone(),self.rows.clone(),kind);
        Self{spaces:[self.spaces[1],self.spaces[0]],containing:[self.containing[1],self.containing[0]],gaps:[self.gaps[1],self.gaps[0]],layouts:[self.layouts[1],self.layouts[0]],edge_basis:self.edge_basis,columns,rows,swapped:!self.swapped}
    }
    /// A descendant in the same writing mode keeps the parent-point mapping;
    /// its local spaces and scope sources are supplied again, not inferred
    /// from the previous slot's track count or geometry.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn descendant(&self,spaces:[AvailableSpace;2],containing:[AvailableSpace;2],gaps:[f32;2],layouts:[AxisLayout;2],edge_basis:AvailableSpace,columns:Option<AxisInheritance>,rows:Option<AxisInheritance>)->Self {
        let mut inputs=Self::new(spaces,containing,gaps,layouts,edge_basis,columns,rows);inputs.swapped=self.swapped;inputs
    }
}

#[derive(Clone, Debug)]
pub(super) struct MeasurementId { operation:Rc<()>, sequence:u64 }
impl PartialEq for MeasurementId {
    fn eq(&self,other:&Self)->bool {Rc::ptr_eq(&self.operation,&other.operation)&&self.sequence==other.sequence}
}
impl Eq for MeasurementId {}

/// Identity follows the concrete projected item slot, including a lanes
/// item's hypothetical area. Geometry changes still require a fresh entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SlotKey {
    item:usize,
    columns:(i32,i32),
    rows:(i32,i32),
    names:(bool,bool),
    automatic:bool,
}
impl SlotKey {
    pub(super) fn of(item:&GridItem)->Self {
        Self{item:item.idx,columns:item.area.col,rows:item.area.row,names:(item.column_names,item.row_names),automatic:item.parent_lanes_auto}
    }
}

/// A true min-content probe is separate from Contribution.min, which may be
/// the smaller minimum contribution after an explicit minimum/overflow rule.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct MinContentProbe { path:Vec<usize>, value:f32 }
impl MinContentProbe {
    pub(super) fn new(path:&[usize],value:f32,budget:&mut WorkBudget)->Self {
        assert!(value.is_finite(),"Invalid Grid min-content probe");
        budget.charge(path.len().saturating_add(1));
        Self{path:path.to_vec(),value}
    }
}

/// Construction performs complete track sizing or accepts an already-used
/// inherited axis. A partly initialized/maximized list cannot be recorded as
/// an axis result merely because it has numeric base sizes.
#[derive(Clone)]
pub(super) struct CompletedAxis {
    tracks:TrackList,
    space:Option<GridSpace>,
}
impl CompletedAxis {
    pub(super) fn standalone(mut tracks:TrackList,contributions:&[Contribution],space:GridSpace,gap:f32,stretch:bool,budget:&mut WorkBudget)->Self {
        assert!(tracks.inherited.is_none(),"Standalone completion of an inherited Grid axis");
        budget.charge(tracks.tracks.len().saturating_add(contributions.len()).saturating_mul(3));
        super::size_tracks_with_space(&mut tracks.tracks,contributions,space,gap,stretch);
        Self{tracks,space:Some(space)}
    }
    pub(super) fn inherited(tracks:TrackList,budget:&mut WorkBudget)->Self {
        assert!(tracks.inherited.as_ref().is_some_and(|axis|axis.definite),"Grid placement estimate is not a used inherited axis");
        budget.charge(tracks.tracks.len());
        Self{tracks,space:None}
    }
    pub(super) fn tracks(&self)->&TrackList {&self.tracks}
    pub(super) fn into_tracks(self)->TrackList {self.tracks}
    pub(super) fn retained_copy(&self,budget:&mut WorkBudget)->Self {budget.tracks(&self.tracks);self.clone()}
    pub(super) fn definite_space(&self)->Option<f32> {if let Some(GridSpace::Definite(size))=self.space {Some(size)} else {None}}
    fn same_template(&self,tracks:&TrackList,budget:&mut WorkBudget)->bool {
        // Compare sizing inputs and line metadata, not bases/growth limits
        // produced by the solver. Charge complete lists before inspecting them.
        budget.tracks(&self.tracks);budget.tracks(tracks);
        self.tracks.inherited.is_none()&&tracks.inherited.is_none()
            && self.tracks.names==tracks.names&&self.tracks.tracks.len()==tracks.tracks.len()
            && self.tracks.tracks.iter().zip(&tracks.tracks).all(|(old,new)|
                old.min==new.min&&old.max==new.max&&old.auto_fit==new.auto_fit&&old.collapsed==new.collapsed)
    }
}

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) enum PassRole { Requested, Prerequisite }

#[derive(Clone, Debug)]
struct Entry { id:MeasurementId, point:MeasurePoint, axis:GridAxis, role:PassRole, inputs:MeasureInputs }

#[derive(Clone)]
struct ProducedAxis { axis:CompletedAxis, entry:Entry }

/// A shared axis can project descendants before its parent resolves its used
/// sizes. That result contains no track sizes. Only an explicit completion
/// provides geometry, together with the point and inputs that produced it.
pub(super) struct AxisRecord {
    entry:Entry,
    complete:Option<ProducedAxis>,
    pub(super) contributions:Vec<Contribution>,
    min_content:Vec<MinContentProbe>,
}

pub(super) struct CompletionOrigin<'a> {
    pub(super) id:&'a MeasurementId,
    pub(super) point:MeasurePoint,
    pub(super) axis:GridAxis,
    pub(super) role:PassRole,
    pub(super) inputs:&'a MeasureInputs,
}

impl AxisRecord {
    pub(super) fn measurement_point(&self)->MeasurePoint {self.entry.point}
    pub(super) fn inputs(&self)->&MeasureInputs {&self.entry.inputs}
    pub(super) fn completion(&self)->Option<&CompletedAxis> {self.complete.as_ref().map(|complete|&complete.axis)}
    pub(super) fn completion_origin(&self)->Option<CompletionOrigin<'_>> {
        self.complete.as_ref().map(|complete|CompletionOrigin{id:&complete.entry.id,point:complete.entry.point,axis:complete.entry.axis,role:complete.entry.role,inputs:&complete.entry.inputs})
    }
    /// Historical completion is usable only under its own sizing/layout
    /// conditions. Opposite-axis bases and fresh contribution maxima are not
    /// geometry identities: those still require fresh probes/measurement.
    pub(super) fn completion_matches(&self,inputs:&MeasureInputs,axis:GridAxis,tracks:&TrackList,budget:&mut WorkBudget)->Option<bool> {
        let produced=self.complete.as_ref()?;
        let old=&produced.entry.inputs;let index=axis.index();
        let template_matches=produced.axis.same_template(tracks,budget);
        Some(produced.entry.axis==axis&&old.swapped==inputs.swapped
            && old.spaces[index]==inputs.spaces[index]&&old.gaps[index]==inputs.gaps[index]
            && old.layouts[index]==inputs.layouts[index]
            && produced.axis.space==Some(inputs.spaces[index].constraint().unwrap_or(GridSpace::MaxContent))
            && template_matches)
    }
}

pub(super) struct GridOperation {
    kind:OperationKind,
    token:Rc<()>,
    next:u64,
    active:Option<Entry>,
    axes:[Option<AxisRecord>;2],
    children:Vec<(SlotKey,GridOperation)>,
}
impl GridOperation {
    pub(super) fn new(kind:OperationKind,budget:&mut WorkBudget)->Self {
        budget.charge(1);
        Self{kind,token:Rc::new(()),next:0,active:None,axes:[None,None],children:Vec::new()}
    }
    pub(super) fn child(&mut self,key:SlotKey,budget:&mut WorkBudget)->&mut Self {
        self.child_of_kind(key,self.kind,budget)
    }
    pub(super) fn child_of_kind(&mut self,key:SlotKey,kind:OperationKind,budget:&mut WorkBudget)->&mut Self {
        budget.charge(self.children.len().saturating_add(1));
        if let Some(index)=self.children.iter().position(|(old,child)|*old==key&&child.kind==kind) {return &mut self.children[index].1;}
        budget.charge(1);
        let child=Self::new(kind,budget);
        self.children.push((key,child));
        &mut self.children.last_mut().unwrap().1
    }
    pub(super) fn begin(&mut self,point:MeasurePoint,inputs:MeasureInputs,budget:&mut WorkBudget)->MeasurementId {
        let axis=inputs.local_axis(point);
        self.enter(point,axis,PassRole::Requested,inputs,budget)
    }
    /// Bootstrap only an unresolved independent axis whose initial pass is
    /// already due in the parent's order. The work occurs NOW: its producer
    /// keeps the current parent point and an explicit local axis/role.
    pub(super) fn begin_prerequisite(&mut self,point:MeasurePoint,axis:GridAxis,inputs:MeasureInputs,budget:&mut WorkBudget)->MeasurementId {
        let first=inputs.local_axis(MeasurePoint::Columns);
        let due=if axis==first {0} else {1};
        assert!(due<point.rank(),"Grid prerequisite would complete a later parent axis prematurely");
        assert_ne!(axis,inputs.local_axis(point),"Grid prerequisite cannot repeat the requested axis");
        assert!(self.axes[axis.index()].as_ref().and_then(|record|record.completion()).is_none(),"Grid prerequisite cannot re-size a completed axis");
        self.enter(point,axis,PassRole::Prerequisite,inputs,budget)
    }
    fn enter(&mut self,point:MeasurePoint,axis:GridAxis,role:PassRole,inputs:MeasureInputs,budget:&mut WorkBudget)->MeasurementId {
        if let Some(old)=&self.active {
            assert!(point.rank()>=old.point.rank(),"Grid measurement pass moved backwards");
            assert!(!point.is_adjustment()||point!=old.point||(old.role==PassRole::Prerequisite&&role==PassRole::Requested),"Grid adjustment repeated");
            assert!(!(role==PassRole::Prerequisite&&point==old.point&&old.role==PassRole::Requested),"Grid prerequisite followed the requested pass");
        }
        budget.charge(1);
        let sequence=self.next.checked_add(1).expect("Grid measurement id exhausted");
        let id=MeasurementId{operation:self.token.clone(),sequence};
        self.next=sequence;
        self.active=Some(Entry{id:id.clone(),point,axis,role,inputs});
        id
    }
    /// Every call records freshly measured inputs. Comparison uses only true
    /// min-content probes; a max-only change must not request an adjustment.
    pub(super) fn record(&mut self,id:&MeasurementId,completed:&CompletedAxis,contributions:&[Contribution],probes:&[MinContentProbe],budget:&mut WorkBudget)->bool {
        self.record_inner(id,Some(completed),false,contributions,probes,budget)
    }
    /// Hoisted descendants of an unresolved inherited axis are measured, not
    /// completed. In particular this cannot turn placement bases into sizes.
    pub(super) fn record_projection(&mut self,id:&MeasurementId,contributions:&[Contribution],probes:&[MinContentProbe],budget:&mut WorkBudget)->bool {
        self.record_inner(id,None,false,contributions,probes,budget)
    }
    /// An adjustment that observes no min-content change must not maximize
    /// again merely because a maximum changed. Retain the previous geometry
    /// with its ORIGINAL producer, while recording the fresh observation.
    pub(super) fn observe_unchanged(&mut self,id:&MeasurementId,tracks:&TrackList,contributions:&[Contribution],probes:&[MinContentProbe],budget:&mut WorkBudget) {
        assert_eq!(self.completion_matches(id,tracks,budget),Some(true),"Changed Grid own conditions cannot retain old completion");
        self.record_inner(id,None,true,contributions,probes,budget);
    }
    pub(super) fn completion_matches(&self,id:&MeasurementId,tracks:&TrackList,budget:&mut WorkBudget)->Option<bool> {
        let entry=self.active.as_ref().filter(|entry|entry.id==*id).expect("Stale Grid measurement query");
        self.axes[entry.axis.index()].as_ref()?.completion_matches(&entry.inputs,entry.axis,tracks,budget)
    }
    pub(super) fn min_content_changed(&self,id:&MeasurementId,probes:&[MinContentProbe],budget:&mut WorkBudget)->Option<bool> {
        let entry=self.active.as_ref().filter(|entry|entry.id==*id).expect("Stale Grid measurement query");
        for probe in probes {budget.charge(probe.path.len().saturating_add(1));}
        self.axes[entry.axis.index()].as_ref().map(|before|before.min_content!=probes)
    }
    #[allow(clippy::too_many_arguments)]
    fn record_inner(&mut self,id:&MeasurementId,complete:Option<&CompletedAxis>,retain:bool,contributions:&[Contribution],probes:&[MinContentProbe],budget:&mut WorkBudget)->bool {
        let entry=self.active.as_ref().filter(|entry|entry.id==*id).expect("Stale Grid measurement result");
        let axis=entry.axis.index();
        let previous=self.axes[axis].as_ref();
        if previous.is_some() {for probe in probes {budget.charge(probe.path.len().saturating_add(1));}}
        let retained=retain.then(|| {
            let previous=previous.expect("Grid unchanged observation requires an earlier measurement");
            assert!(previous.min_content==probes,"Changed Grid min-content cannot retain old completion");
            previous.complete.as_ref().expect("Grid unchanged observation requires completed geometry")
        });
        // Sources currently share Arc-backed payloads, but retaining a new
        // record still consumes the operation's input-retention allowance.
        // Charge before Entry::clone, including projections with no items.
        budget.charge(subgrid::source_work(&entry.inputs.columns).saturating_add(subgrid::source_work(&entry.inputs.rows)));
        let geometry=complete.or_else(||retained.map(|retained|&retained.axis));
        if let Some(geometry)=geometry {
            budget.tracks(&geometry.tracks);
            let producer=retained.map_or(entry,|retained|&retained.entry);
            budget.charge(subgrid::source_work(&producer.inputs.columns).saturating_add(subgrid::source_work(&producer.inputs.rows)));
        }
        budget.charge(contributions.len().saturating_mul(3));
        for probe in probes {budget.charge(probe.path.len().saturating_add(1));}
        let changed=previous.is_some_and(|before|before.min_content!=probes);
        let complete=retained.cloned().or_else(||complete.map(|axis|ProducedAxis{axis:axis.clone(),entry:entry.clone()}));
        let record=AxisRecord{entry:entry.clone(),complete,contributions:contributions.to_vec(),min_content:probes.to_vec()};
        self.axes[axis]=Some(record);
        changed
    }
    pub(super) fn result(&self,id:&MeasurementId)->Option<&AxisRecord> {
        let entry=self.active.as_ref().filter(|entry|entry.id==*id)?;
        self.axes[entry.axis.index()].as_ref().filter(|record|record.entry.id==*id)
    }
    /// History is an earlier approximation with its original producer and
    /// inputs. Only the opposite-axis phase may consume its completion;
    /// it is never presented as a fresh result of the active measurement.
    pub(super) fn opposite_history(&self,id:&MeasurementId)->Option<&AxisRecord> {
        let entry=self.active.as_ref().filter(|entry|entry.id==*id)?;
        let opposite=entry.axis.opposite().index();
        self.axes[opposite].as_ref()
    }
    pub(super) fn same_axis_history(&self,id:&MeasurementId)->Option<&AxisRecord> {
        let entry=self.active.as_ref().filter(|entry|entry.id==*id)?;
        self.axes[entry.axis.index()].as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{Sizing, Track};
    use std::panic::{catch_unwind, AssertUnwindSafe};

    fn list(sizes:&[(Sizing,Sizing)])->TrackList {
        TrackList {
            tracks:sizes.iter().map(|&(min,max)|Track::new(min,max)).collect(),
            names:(0..=sizes.len()).map(|i|vec![format!("line-{i}")]).collect(),
            inherited:None,
        }
    }

    fn source(sizes:&[f32],gap:f32)->AxisInheritance {
        let mut list=list(&sizes.iter().map(|&size|(Sizing::Fixed(size),Sizing::Fixed(size))).collect::<Vec<_>>());
        let mut position=0.;
        let positions:Vec<_>=sizes.iter().enumerate().map(|(i,&size)| {
            list.tracks[i].base=size;
            let start=position;position+=size+gap;start
        }).collect();
        subgrid::section(&list,&positions,0,sizes.len(),gap,true)
    }

    fn inputs()->MeasureInputs {
        MeasureInputs::new([AvailableSpace::MaxContent,AvailableSpace::Indefinite],[AvailableSpace::Indefinite;2],[2.,3.],[AxisLayout::new(ContentDistribution::FlexStart,false);2],AvailableSpace::Indefinite,None,None)
    }

    fn completed(contributions:&[Contribution],budget:&mut WorkBudget)->CompletedAxis {
        CompletedAxis::standalone(list(&[(Sizing::Auto,Sizing::Auto)]),contributions,GridSpace::MaxContent,0.,false,budget)
    }

    fn contribution(min:f32,max:f32)->Contribution {Contribution{start:0,end:1,min,max}}

    fn key(column:i32)->SlotKey {
        SlotKey{item:7,columns:(column,column+1),rows:(0,1),names:(true,true),automatic:true}
    }

    fn tracks(source:&Option<AxisInheritance>)->&subgrid::SharedTracks {
        let Some(AxisInheritance::Tracks(axis))=source else {panic!("Missing tracks")};axis
    }

    #[test]
    fn availability_keeps_infinite_zero_and_intrinsic_constraints_distinct() {
        assert_eq!(AvailableSpace::Indefinite.constraint(),None);
        assert_eq!(AvailableSpace::Definite(0.).constraint(),Some(GridSpace::Definite(0.)));
        assert_eq!(AvailableSpace::MinContent.constraint(),Some(GridSpace::MinContent));
        assert_eq!(AvailableSpace::MaxContent.constraint(),Some(GridSpace::MaxContent));
        assert!(catch_unwind(||MeasureInputs::new([AvailableSpace::Definite(f32::NAN),AvailableSpace::Indefinite],[AvailableSpace::Indefinite;2],[0.,0.],[AxisLayout::new(ContentDistribution::FlexStart,false);2],AvailableSpace::Indefinite,None,None)).is_err());
    }

    #[test]
    fn initial_estimate_retains_names_and_never_exposes_placement_zero_as_used_size() {
        let mut budget=WorkBudget::default();
        let list=list(&[(Sizing::Fixed(10.),Sizing::Auto)]);
        let estimate=PerpendicularEstimate::from_tracks(&list,0,1,0.,true,None,ContentDistribution::FlexStart,&mut budget);
        assert!(estimate.is_infinite());assert!(estimate.definite_geometry().is_none());
        assert_eq!(estimate.line_names(),list.names.as_slice());
        let empty=PerpendicularEstimate::from_tracks(&list,1,1,0.,true,None,ContentDistribution::FlexStart,&mut budget);
        assert!(empty.is_infinite());assert_eq!(empty.line_names(),&list.names[1..=1]);
        let zero=list_of_zero();
        let estimate=PerpendicularEstimate::from_tracks(&zero,0,1,0.,true,None,ContentDistribution::FlexStart,&mut budget);
        assert!(!estimate.is_infinite());
        let Some(AxisInheritance::Tracks(axis))=estimate.definite_geometry() else {panic!("Missing definite zero")};
        assert_eq!(axis.extent(),0.);
    }

    fn list_of_zero()->TrackList {list(&[(Sizing::Fixed(0.),Sizing::Fixed(0.))])}

    #[test]
    fn definite_perpendicular_estimate_floors_maxima_and_preserves_distribution() {
        let mut budget=WorkBudget::default();
        let list=list(&[(Sizing::Fixed(70.),Sizing::Fixed(50.)),(Sizing::Fixed(50.),Sizing::Fixed(50.))]);
        let estimate=PerpendicularEstimate::from_tracks(&list,0,2,0.,true,Some(300.),ContentDistribution::SpaceBetween,&mut budget);
        let Some(AxisInheritance::Tracks(axis))=estimate.definite_geometry() else {panic!("Missing fixed estimate")};
        assert_eq!(axis.sizes,[70.,50.]);assert_eq!(axis.starts,[0.,250.]);
    }

    #[test]
    fn wrappers_map_axes_sources_spaces_and_gaps_without_restarting_parent_point() {
        let mut budget=WorkBudget::default();
        let outer=MeasureInputs::new([AvailableSpace::MaxContent,AvailableSpace::Definite(90.)],[AvailableSpace::Definite(400.),AvailableSpace::Definite(200.)],[5.,7.],[AxisLayout::new(ContentDistribution::SpaceBetween,false),AxisLayout::new(ContentDistribution::Center,true)],AvailableSpace::Definite(300.),Some(source(&[20.,40.],5.)),Some(source(&[10.,30.],7.)));
        for kind in [WmKind::Cw,WmKind::Ccw,WmKind::Reflect] {
            let inner=outer.through_wrapper(kind,&mut budget);
            assert_eq!(inner.local_axis(MeasurePoint::Columns),GridAxis::Rows);
            assert_eq!(inner.local_axis(MeasurePoint::AdjustRows),GridAxis::Columns);
            assert_eq!(inner.spaces,[AvailableSpace::Definite(90.),AvailableSpace::MaxContent]);
            assert_eq!(inner.containing,[AvailableSpace::Definite(200.),AvailableSpace::Definite(400.)]);
            assert_eq!(inner.gaps,[7.,5.]);
            assert_eq!(inner.layouts,[outer.layouts[1],outer.layouts[0]]);
            assert_eq!(inner.edge_basis,AvailableSpace::Definite(300.));
            let columns=tracks(&inner.columns);let rows=tracks(&inner.rows);
            assert_eq!(columns.sizes,if kind==WmKind::Ccw {vec![30.,10.]} else {vec![10.,30.]});
            assert_eq!(rows.sizes,if kind==WmKind::Cw {vec![40.,20.]} else {vec![20.,40.]});
            assert_eq!(columns.names,if kind==WmKind::Ccw {vec![vec!["line-2".to_owned()],vec!["line-1".to_owned()],vec!["line-0".to_owned()]]} else {vec![vec!["line-0".to_owned()],vec!["line-1".to_owned()],vec!["line-2".to_owned()]]});
        }
        let restored=outer.through_wrapper(WmKind::Cw,&mut budget).through_wrapper(WmKind::Ccw,&mut budget);
        assert_eq!(restored.local_axis(MeasurePoint::Columns),GridAxis::Columns);
        assert_eq!(restored.spaces,outer.spaces);assert_eq!(restored.gaps,outer.gaps);
        assert_eq!(restored.containing,outer.containing);
        assert_eq!(restored.layouts,outer.layouts);
        assert_eq!(tracks(&restored.columns).sizes,tracks(&outer.columns).sizes);
        assert_eq!(tracks(&restored.rows).starts,tracks(&outer.rows).starts);
    }

    #[test]
    fn completed_axes_keep_actual_maximization_separate_from_intrinsic_minimum() {
        let mut budget=WorkBudget::default();let contributions=[contribution(10.,48.)];
        let min=CompletedAxis::standalone(list(&[(Sizing::Auto,Sizing::Auto)]),&contributions,GridSpace::MinContent,0.,false,&mut budget);
        let max=completed(&contributions,&mut budget);
        assert_eq!(min.tracks().tracks[0].base,10.);
        assert_eq!(max.tracks().tracks[0].base,48.);
        let placement=subgrid::sizing_section_aligned(&list(&[(Sizing::Auto,Sizing::Auto)]),0,1,0.,true,None,ContentDistribution::FlexStart);
        let AxisInheritance::Tracks(axis)=placement else {panic!("Missing placement")};
        let estimated=TrackList{tracks:vec![Track::new(Sizing::Fixed(0.),Sizing::Fixed(0.))],names:axis.names.clone(),inherited:Some(axis)};
        assert!(catch_unwind(AssertUnwindSafe(||CompletedAxis::inherited(estimated,&mut budget))).is_err());
    }

    #[test]
    fn equal_shape_with_new_opposite_geometry_requires_a_fresh_result() {
        let mut budget=WorkBudget::default();let contributions=[contribution(10.,40.)];
        let done=completed(&contributions,&mut budget);
        let probes=[MinContentProbe::new(&[7],10.,&mut budget)];
        let mut operation=GridOperation::new(OperationKind::Layout,&mut budget);
        let first=operation.begin(MeasurePoint::Columns,inputs(),&mut budget);
        operation.record(&first,&done,&contributions,&probes,&mut budget);
        let mut changed=inputs();changed.gaps[1]=25.;changed.rows=Some(source(&[50.,50.],25.));
        let second=operation.begin(MeasurePoint::Columns,changed,&mut budget);
        assert_ne!(first,second);assert!(operation.result(&first).is_none());assert!(operation.result(&second).is_none());
        operation.record(&second,&done,&contributions,&probes,&mut budget);
        let record=operation.result(&second).unwrap();
        assert_eq!(record.measurement_point(),MeasurePoint::Columns);
        assert_eq!(record.inputs().gaps,[2.,25.]);assert_eq!(tracks(&record.inputs().rows).starts,[0.,75.]);
    }

    #[test]
    fn only_true_min_content_changes_trigger_adjustment_with_both_fresh_contributions() {
        let mut budget=WorkBudget::default();let mut operation=GridOperation::new(OperationKind::Layout,&mut budget);
        let first=[contribution(0.,40.)];let same_min=[MinContentProbe::new(&[7],10.,&mut budget)];
        let done=completed(&first,&mut budget);
        let id=operation.begin(MeasurePoint::Columns,inputs(),&mut budget);
        assert!(!operation.record(&id,&done,&first,&same_min,&mut budget));
        let max_only=[contribution(0.,80.)];let done=completed(&max_only,&mut budget);
        let id=operation.begin(MeasurePoint::Columns,inputs(),&mut budget);
        assert!(!operation.record(&id,&done,&max_only,&same_min,&mut budget));
        // A minimum contribution can stay zero because of min-width:0 even
        // when the real min-content size changes after the opposite pass.
        let changed_min=[MinContentProbe::new(&[7],25.,&mut budget)];
        let fresh=[contribution(0.,90.)];let done=completed(&fresh,&mut budget);
        let id=operation.begin(MeasurePoint::AdjustColumns,inputs(),&mut budget);
        assert!(operation.record(&id,&done,&fresh,&changed_min,&mut budget));
        let record=operation.result(&id).unwrap();assert_eq!(record.contributions,fresh);
        let complete=record.completion().unwrap();
        assert_eq!(complete.space,Some(GridSpace::MaxContent));assert_eq!(complete.tracks().tracks[0].base,90.);
    }

    #[test]
    fn operation_and_hypothetical_slot_ids_cannot_alias() {
        let mut budget=WorkBudget::default();
        let mut parent=GridOperation::new(OperationKind::IntrinsicMin,&mut budget);
        let first=parent.child(key(0),&mut budget).begin(MeasurePoint::Columns,inputs(),&mut budget);
        let second=parent.child(key(1),&mut budget).begin(MeasurePoint::Columns,inputs(),&mut budget);
        assert_ne!(first,second);
        assert!(parent.child(key(0),&mut budget).result(&second).is_none());
        let mut max=GridOperation::new(OperationKind::IntrinsicMax,&mut budget);
        let third=max.begin(MeasurePoint::Columns,inputs(),&mut budget);
        assert_ne!(first,third);assert!(max.result(&first).is_none());
    }

    #[test]
    fn retained_copies_are_precharged_and_failure_does_not_install_a_partial_result() {
        let mut budget=WorkBudget::default();let mut operation=GridOperation::new(OperationKind::Layout,&mut budget);
        let contributions=[contribution(10.,40.)];let done=completed(&contributions,&mut budget);
        let probes=[MinContentProbe::new(&[7],10.,&mut budget)];
        let id=operation.begin(MeasurePoint::Columns,inputs(),&mut budget);
        let mut short=WorkBudget::limited_for_test(1);
        assert!(catch_unwind(AssertUnwindSafe(||operation.record(&id,&done,&contributions,&probes,&mut short))).is_err());
        assert!(operation.result(&id).is_none());
        operation.record(&id,&done,&contributions,&probes,&mut budget);
        assert_eq!(operation.result(&id).unwrap().completion().unwrap().tracks().tracks[0].base,40.);
    }

    #[test]
    fn backward_passes_and_second_adjustments_fail_before_replacing_active_results() {
        let mut budget=WorkBudget::default();let mut operation=GridOperation::new(OperationKind::Layout,&mut budget);
        let contributions=[contribution(10.,40.)];let done=completed(&contributions,&mut budget);
        let id=operation.begin(MeasurePoint::AdjustColumns,inputs(),&mut budget);
        operation.record(&id,&done,&contributions,&[],&mut budget);
        assert!(catch_unwind(AssertUnwindSafe(||operation.begin(MeasurePoint::Rows,inputs(),&mut budget))).is_err());
        assert!(catch_unwind(AssertUnwindSafe(||operation.begin(MeasurePoint::AdjustColumns,inputs(),&mut budget))).is_err());
        assert!(operation.result(&id).is_some());
        operation.begin(MeasurePoint::AdjustRows,inputs(),&mut budget);
    }

    #[test]
    fn shared_axis_projection_cannot_be_used_as_completed_geometry() {
        let mut budget=WorkBudget::default();let mut operation=GridOperation::new(OperationKind::Layout,&mut budget);
        let contributions=[contribution(10.,40.)];
        let probes=[MinContentProbe::new(&[7],10.,&mut budget)];
        let id=operation.begin(MeasurePoint::Columns,inputs(),&mut budget);
        operation.record_projection(&id,&contributions,&probes,&mut budget);
        let record=operation.result(&id).unwrap();
        assert!(record.completion().is_none());assert_eq!(record.contributions,contributions);
        let completed=completed(&contributions,&mut budget);
        let next=operation.begin(MeasurePoint::Columns,inputs(),&mut budget);
        assert!(operation.result(&next).is_none());
        operation.record(&next,&completed,&contributions,&probes,&mut budget);
        assert!(operation.result(&next).unwrap().completion().is_some());
    }

    #[test]
    fn even_an_empty_projection_precharges_its_retained_input_sources() {
        let mut budget=WorkBudget::default();let mut operation=GridOperation::new(OperationKind::Layout,&mut budget);
        let input=MeasureInputs::new([AvailableSpace::Definite(1000.),AvailableSpace::Indefinite],[AvailableSpace::Definite(1000.),AvailableSpace::Indefinite],[0.,0.],[AxisLayout::new(ContentDistribution::FlexStart,false);2],AvailableSpace::Definite(1000.),Some(source(&[10.;64],0.)),None);
        let id=operation.begin(MeasurePoint::Columns,input,&mut budget);
        let mut exhausted=WorkBudget::limited_for_test(0);
        assert!(catch_unwind(AssertUnwindSafe(||operation.record_projection(&id,&[],&[],&mut exhausted))).is_err());
        assert!(operation.result(&id).is_none());
        operation.record_projection(&id,&[],&[],&mut budget);
        assert_eq!(tracks(&operation.result(&id).unwrap().inputs().columns).sizes.len(),64);
    }

    #[test]
    fn intrinsic_min_and_max_children_have_distinct_histories_for_the_same_item_slot() {
        let mut budget=WorkBudget::default();let mut parent=GridOperation::new(OperationKind::Layout,&mut budget);
        let min=parent.child_of_kind(key(0),OperationKind::IntrinsicMin,&mut budget).begin(MeasurePoint::Columns,inputs(),&mut budget);
        let max=parent.child_of_kind(key(0),OperationKind::IntrinsicMax,&mut budget).begin(MeasurePoint::Columns,inputs(),&mut budget);
        assert_ne!(min,max);
        assert!(parent.child_of_kind(key(0),OperationKind::IntrinsicMax,&mut budget).result(&min).is_none());
    }

    #[test]
    fn opposite_history_exposes_its_old_producer_without_claiming_fresh_geometry() {
        let mut budget=WorkBudget::default();let mut operation=GridOperation::new(OperationKind::Layout,&mut budget);
        let contributions=[contribution(10.,40.)];let done=completed(&contributions,&mut budget);
        let id=operation.begin(MeasurePoint::Columns,inputs(),&mut budget);
        operation.record(&id,&done,&contributions,&[],&mut budget);
        let mut fresh=inputs();fresh.gaps[1]=25.;fresh.rows=Some(source(&[50.,50.],25.));
        let rows=operation.begin(MeasurePoint::Rows,fresh,&mut budget);
        assert!(operation.result(&id).is_none());assert!(operation.result(&rows).is_none());
        let previous=operation.opposite_history(&rows).unwrap();
        assert_eq!(previous.measurement_point(),MeasurePoint::Columns);assert_eq!(previous.inputs().gaps,[2.,3.]);
        assert_eq!(previous.completion().unwrap().tracks().tracks[0].base,40.);
    }

    #[test]
    fn same_mode_descendant_keeps_its_orthogonal_parent_point_mapping() {
        let mut budget=WorkBudget::default();
        let parent=inputs().through_wrapper(WmKind::Cw,&mut budget);
        let child=parent.descendant([AvailableSpace::Definite(75.),AvailableSpace::Indefinite],[AvailableSpace::Definite(180.),AvailableSpace::Definite(90.)],[4.,8.],[AxisLayout::new(ContentDistribution::FlexStart,false);2],AvailableSpace::Definite(180.),None,None);
        assert_eq!(child.local_axis(MeasurePoint::Columns),GridAxis::Rows);
        assert_eq!(child.spaces,[AvailableSpace::Definite(75.),AvailableSpace::Indefinite]);
        assert_eq!(child.containing,[AvailableSpace::Definite(180.),AvailableSpace::Definite(90.)]);
        assert_eq!(child.edge_basis,AvailableSpace::Definite(180.));
        assert_eq!(MeasurePoint::AdjustRows.legacy_phase(),super::super::MeasurePhase::Adjusted);
    }

    #[test]
    fn max_only_observation_preserves_old_geometry_and_its_original_producer() {
        let mut budget=WorkBudget::default();let mut operation=GridOperation::new(OperationKind::Layout,&mut budget);
        let original=[contribution(10.,40.)];let done=completed(&original,&mut budget);
        let probes=[MinContentProbe::new(&[7],10.,&mut budget)];
        let first=operation.begin(MeasurePoint::Columns,inputs(),&mut budget);
        operation.record(&first,&done,&original,&probes,&mut budget);
        let mut changed_inputs=inputs();changed_inputs.gaps[1]=25.;
        let next=operation.begin(MeasurePoint::AdjustColumns,changed_inputs,&mut budget);
        let maximum_changed=[contribution(10.,80.)];
        assert_eq!(operation.min_content_changed(&next,&probes,&mut budget),Some(false));
        operation.observe_unchanged(&next,&list(&[(Sizing::Auto,Sizing::Auto)]),&maximum_changed,&probes,&mut budget);
        let record=operation.result(&next).unwrap();
        assert_eq!(record.measurement_point(),MeasurePoint::AdjustColumns);assert_eq!(record.inputs().gaps,[2.,25.]);
        assert_eq!(record.contributions,maximum_changed);
        assert_eq!(record.completion().unwrap().tracks().tracks[0].base,40.);
        let origin=record.completion_origin().unwrap();
        assert_eq!(origin.id,&first);assert_eq!(origin.point,MeasurePoint::Columns);assert_eq!(origin.inputs.gaps,[2.,3.]);
    }

    #[test]
    fn a_changed_minimum_cannot_be_disguised_as_an_unchanged_observation() {
        let mut budget=WorkBudget::default();let mut operation=GridOperation::new(OperationKind::Layout,&mut budget);
        let original=[contribution(0.,40.)];let done=completed(&original,&mut budget);
        let first=operation.begin(MeasurePoint::Columns,inputs(),&mut budget);
        operation.record(&first,&done,&original,&[MinContentProbe::new(&[7],10.,&mut budget)],&mut budget);
        let next=operation.begin(MeasurePoint::AdjustColumns,inputs(),&mut budget);
        let probes=[MinContentProbe::new(&[7],25.,&mut budget)];
        assert_eq!(operation.min_content_changed(&next,&probes,&mut budget),Some(true));
        assert!(catch_unwind(AssertUnwindSafe(||operation.observe_unchanged(&next,&list(&[(Sizing::Auto,Sizing::Auto)]),&original,&probes,&mut budget))).is_err());
        assert!(operation.result(&next).is_none());
    }

    #[test]
    fn unchanged_observation_precharges_retained_geometry_before_installing_a_new_result() {
        let mut budget=WorkBudget::default();let mut operation=GridOperation::new(OperationKind::Layout,&mut budget);
        let original=[contribution(10.,40.)];let done=completed(&original,&mut budget);
        let probes=[MinContentProbe::new(&[7],10.,&mut budget)];
        let first=operation.begin(MeasurePoint::Columns,inputs(),&mut budget);
        operation.record(&first,&done,&original,&probes,&mut budget);
        let next=operation.begin(MeasurePoint::AdjustColumns,inputs(),&mut budget);
        let mut exhausted=WorkBudget::limited_for_test(0);
        assert!(catch_unwind(AssertUnwindSafe(||operation.observe_unchanged(&next,&list(&[(Sizing::Auto,Sizing::Auto)]),&original,&probes,&mut exhausted))).is_err());
        assert!(operation.result(&next).is_none());
        operation.observe_unchanged(&next,&list(&[(Sizing::Auto,Sizing::Auto)]),&original,&probes,&mut budget);
        let record=operation.result(&next).unwrap();
        assert_eq!(record.completion().unwrap().tracks().tracks[0].base,40.);
        assert_eq!(record.completion_origin().unwrap().point,MeasurePoint::Columns);
    }

    #[test]
    fn completion_compatibility_covers_template_inputs_but_ignores_solver_outputs() {
        let mut budget=WorkBudget::default();let mut operation=GridOperation::new(OperationKind::Layout,&mut budget);
        let original=[contribution(10.,40.)];let done=completed(&original,&mut budget);
        let probes=[MinContentProbe::new(&[7],10.,&mut budget)];
        let first=operation.begin(MeasurePoint::Columns,inputs(),&mut budget);
        operation.record(&first,&done,&original,&probes,&mut budget);
        let next=operation.begin(MeasurePoint::AdjustColumns,inputs(),&mut budget);
        let template=||list(&[(Sizing::Auto,Sizing::Auto)]);
        assert_eq!(operation.completion_matches(&next,&template(),&mut budget),Some(true));
        for case in 0..5 {
            let mut changed=template();
            match case {
                0=>changed.tracks[0].min=Sizing::Fixed(2.),
                1=>changed.tracks[0].max=Sizing::Flex(1.),
                2=>changed.tracks[0].auto_fit=true,
                3=>changed.tracks[0].collapsed=true,
                _=>changed.names[0].push("different-line".into()),
            }
            assert_eq!(operation.completion_matches(&next,&changed,&mut budget),Some(false));
        }
        let mut outputs=template();outputs.tracks[0].base=999.;outputs.tracks[0].limit=Some(1000.);outputs.tracks[0].infinitely_growable=true;
        assert_eq!(operation.completion_matches(&next,&outputs,&mut budget),Some(true));
        let mut exhausted=WorkBudget::limited_for_test(0);
        assert!(catch_unwind(AssertUnwindSafe(||operation.completion_matches(&next,&template(),&mut exhausted))).is_err());
        assert!(operation.result(&next).is_none());
    }

    #[test]
    fn unchanged_minima_cannot_retain_under_changed_own_conditions() {
        for case in 0..4 {
            let mut budget=WorkBudget::default();let mut operation=GridOperation::new(OperationKind::Layout,&mut budget);
            let original=[contribution(10.,40.)];let done=completed(&original,&mut budget);
            let probes=[MinContentProbe::new(&[7],10.,&mut budget)];
            let first=operation.begin(MeasurePoint::Columns,inputs(),&mut budget);
            operation.record(&first,&done,&original,&probes,&mut budget);
            let mut changed=inputs();
            match case {
                0=>changed.spaces[0]=AvailableSpace::Definite(0.),
                1=>changed.gaps[0]=20.,
                2=>changed.layouts[0].align=ContentDistribution::Center,
                _=>changed.layouts[0].stretch=true,
            }
            let next=operation.begin(MeasurePoint::AdjustColumns,changed,&mut budget);
            let template=list(&[(Sizing::Auto,Sizing::Auto)]);
            assert_eq!(operation.min_content_changed(&next,&probes,&mut budget),Some(false));
            assert_eq!(operation.completion_matches(&next,&template,&mut budget),Some(false));
            assert!(catch_unwind(AssertUnwindSafe(||operation.observe_unchanged(&next,&template,&original,&probes,&mut budget))).is_err());
            assert!(operation.result(&next).is_none());
            assert_eq!(operation.same_axis_history(&next).unwrap().completion_origin().unwrap().id,&first);
        }
    }
}
