//! Per-axis subgrid inputs. Parent track geometry is immutable; child sizing
//! must not maximize inherited tracks or invent implicit ones.
use std::rc::Rc;
use std::sync::Arc;

use super::{expand_tracks, size_tracks_with_space, Contribution, GridItem, GridSpace, MeasurePhase, Sizing, Track, TrackList};
use crate::block::Layouter;
use crate::flex::ItemEdges;
use crate::tree::{Formatting, LayoutBox, WmKind};
use rabbithole_style::{ComputedStyle, ContentDistribution, ItemAlign, Length, SubgridLineNames, TrackListItem};

#[derive(Clone, Debug)]
pub(crate) struct SharedTracks {
    pub(super) sizes: Vec<f32>,
    pub(super) starts: Vec<f32>,
    pub(super) names: Vec<Vec<String>>,
    pub(super) collapsed: Vec<bool>,
    pub(super) gap: f32,
    /// Accumulated ancestor edge layers at internal track boundaries. A
    /// nested subgrid must retain its parent's own gap adjustments as well.
    edge_before: Vec<f32>,
    edge_after: Vec<f32>,
    pub(super) inset_start: f32,
    pub(super) inset_end: f32,
}

#[derive(Clone, Debug)]
pub(crate) enum AxisInheritance {
    Tracks(Arc<SharedTracks>),
    /// A request along a lanes parent's stacking axis makes that child
    /// axis a stacking axis as well (Grid3 §3.2).
    Stacking,
}

impl SharedTracks {
    pub(super) fn extent(&self) -> f32 {
        self.starts.last().zip(self.sizes.last()).map_or(0., |(p,s)| p+s)
    }
    fn reversed(&self) -> Self {
        let extent=self.extent();
        let mut result=self.clone();
        result.starts=self.starts.iter().zip(&self.sizes).rev().map(|(p,s)| extent-p-s).collect();
        result.sizes.reverse();result.collapsed.reverse();result.names.reverse();
        result.edge_before=self.edge_after.iter().rev().copied().collect();
        result.edge_after=self.edge_before.iter().rev().copied().collect();
        std::mem::swap(&mut result.inset_start,&mut result.inset_end);
        result
    }
}

fn reverse(source: Option<AxisInheritance>) -> Option<AxisInheritance> {
    source.map(|source| match source {
        AxisInheritance::Tracks(tracks)=>AxisInheritance::Tracks(Arc::new(tracks.reversed())),
        AxisInheritance::Stacking=>AxisInheritance::Stacking,
    })
}

/// Express parent-space sources in an orthogonal wrapper's inner space.
fn through_wrapper(columns: Option<AxisInheritance>, rows: Option<AxisInheritance>, kind: WmKind) -> (Option<AxisInheritance>,Option<AxisInheritance>) {
    let inner_columns=if kind==WmKind::Ccw {reverse(rows)} else {rows};
    let inner_rows=if kind==WmKind::Cw {reverse(columns)} else {columns};
    (inner_columns,inner_rows)
}

pub(super) fn requests(b: &LayoutBox) -> (bool,bool) {
    if b.wm_wrapper {
        return b.children.first().map_or((false,false),|inner| {let (columns,rows)=requests(inner);(rows,columns)});
    }
    if b.formatting!=Formatting::Grid {return (false,false);}
    let requested=|list:&[TrackListItem]| matches!(list.first(),Some(TrackListItem::Subgrid{..}));
    (requested(&b.style.grid_template_columns),requested(&b.style.grid_template_rows))
}

pub(super) fn shared(b: &LayoutBox, rows: bool) -> Option<Arc<SharedTracks>> {
    match if rows {&b.grid_rows} else {&b.grid_columns} {
        Some(AxisInheritance::Tracks(tracks))=>Some(tracks.clone()),
        _=>None,
    }
}

pub(super) fn inherits_stacking(b: &LayoutBox) -> bool {
    matches!(b.grid_columns.as_ref(),Some(AxisInheritance::Stacking)) || matches!(b.grid_rows.as_ref(),Some(AxisInheritance::Stacking))
}

pub(super) fn rows(b:&LayoutBox)->Option<bool> {
    if matches!(b.grid_rows.as_ref(),Some(AxisInheritance::Tracks(_))) || matches!(b.grid_columns.as_ref(),Some(AxisInheritance::Stacking)) {Some(true)}
    else if matches!(b.grid_columns.as_ref(),Some(AxisInheritance::Tracks(_))) || matches!(b.grid_rows.as_ref(),Some(AxisInheritance::Stacking)) {Some(false)}
    else {None}
}

pub(super) fn gap(b:&LayoutBox, rows:bool, reference:f32)->f32 {
    let (value,normal)=if rows {(b.style.row_gap,b.style.row_gap_normal)} else {(b.style.column_gap,b.style.column_gap_normal)};
    if normal {
        if let Some(tracks)=shared(b,rows) {return tracks.gap;}
    }
    value.resolve_or_zero(reference)
}

/// Capture the used parent section, retaining collapsed tracks and actual
/// distributed positions. Local line numbering starts at this section.
pub(super) fn section(list:&TrackList, positions:&[f32], start:usize, end:usize, gap:f32, names:bool)->AxisInheritance {
    let start=start.min(list.tracks.len().saturating_sub(1));
    let end=end.max(start+1).min(list.tracks.len());
    let origin=positions.get(start).copied().unwrap_or(0.);
    let before=|i:usize|list.inherited.as_ref().map_or(0.,|parent|if i==0 {parent.inset_start} else {parent.edge_before[i]+(gap-parent.gap)*0.5});
    let after=|i:usize|list.inherited.as_ref().map_or(0.,|parent|if i+1==parent.sizes.len() {parent.inset_end} else {parent.edge_after[i]+(gap-parent.gap)*0.5});
    let tracks=SharedTracks {
        sizes:list.tracks[start..end].iter().map(|t|t.base).collect(),
        starts:positions[start..end].iter().map(|p|p-origin).collect(),
        names:if names {list.names[start..=end].to_vec()} else {vec![vec![];end-start+1]},
        collapsed:list.tracks[start..end].iter().map(|t|t.collapsed).collect(),
        gap,
        edge_before:(start..end).map(|i|if i==start {0.} else {before(i)}).collect(),
        edge_after:(start..end).map(|i|if i+1==end {0.} else {after(i)}).collect(),
        inset_start:if start<end {before(start)} else {0.},
        inset_end:if start<end {after(end-1)} else {0.},
    };
    AxisInheritance::Tracks(Arc::new(tracks))
}

/// Expand only as far as the inherited line count; repeats remain bounded
/// even when their specified count is huge.
fn local_names(spec:&[SubgridLineNames], count:usize)->Vec<Vec<String>> {
    let fixed:usize=spec.iter().map(|part|match part {
        SubgridLineNames::Names(_)=>1,
        SubgridLineNames::Repeat{count:Some(n),names}=>names.len().saturating_mul(*n as usize),
        _=>0,
    }).fold(0,usize::saturating_add);
    let mut result=Vec::with_capacity(count);
    for part in spec {
        match part {
            SubgridLineNames::Names(names)=>{if result.len()<count {result.push(names.clone());}},
            SubgridLineNames::Repeat{count:reps,names}=>{
                let repetitions=reps.map(|n|n as usize).unwrap_or_else(||count.saturating_sub(fixed)/names.len().max(1));
                for _ in 0..repetitions.min(count.saturating_sub(result.len())) {
                    for names in names {if result.len()<count {result.push(names.clone());}}
                }
            }
        }
    }
    result.resize_with(count,Vec::new);result
}

pub(super) fn tracks(b:&LayoutBox,rows:bool,avail:Option<f32>,gap:f32)->TrackList {
    let list=if rows {&b.style.grid_template_rows} else {&b.style.grid_template_columns};
    let Some(inherited)=shared(b,rows) else {return expand_tracks(list,avail,gap);};
    let mut names=inherited.names.clone();
    let line_count=names.len();
    if let Some(TrackListItem::Subgrid{line_names})=list.first() {
        for (line,local) in names.iter_mut().zip(local_names(line_names,line_count)) {line.extend(local);}
    }
    let tracks=inherited.sizes.iter().zip(&inherited.collapsed).map(|(&size,&collapsed)|{
        let mut track=Track::new(Sizing::Fixed(size),Sizing::Fixed(size));
        track.base=if collapsed {0.} else {size};track.limit=Some(track.base);track.collapsed=collapsed;track
    }).collect();
    TrackList{tracks,names,inherited:Some(inherited)}
}

/// Grid2 §12.1's initial opposite-axis estimate uses definite maximum track
/// sizes, floored by definite minimums. Other tracks remain indefinite.
pub(super) fn fixed_section(list:&TrackList,start:usize,end:usize,gap:f32,names:bool)->Option<AxisInheritance> {
    if list.inherited.is_some() {
        let (positions,_)=positions(list,gap,None,ContentDistribution::FlexStart);
        return Some(section(list,&positions,start,end,gap,names));
    }
    let end=end.min(list.tracks.len());let start=start.min(end);
    if start==end||!list.tracks[start..end].iter().all(|t|matches!(t.max,Sizing::Fixed(_))) {return None;}
    let tracks=list.tracks.iter().cloned().map(|mut t| {
        t.base=match (t.min,t.max) {
            (Sizing::Fixed(min),Sizing::Fixed(max))=>min.max(max),
            (_,Sizing::Fixed(max))=>max,
            _=>0.,
        };
        if t.collapsed {t.base=0.;}t.limit=Some(t.base);t
    }).collect();
    let fixed=TrackList{tracks,names:list.names.clone(),inherited:None};
    let (positions,_)=positions(&fixed,gap,None,ContentDistribution::FlexStart);
    Some(section(&fixed,&positions,start,end,gap,names))
}

/// During contribution collection the span and line names are definite, but
/// intrinsic track sizes are not. Keep that placement geometry separate from
/// used geometry; grandchildren contribute directly without sizing this axis.
pub(super) fn sizing_section(list:&TrackList,start:usize,end:usize,gap:f32,names:bool)->AxisInheritance {
    let tracks=list.tracks.iter().cloned().map(|mut t| {
        t.base=match t.min {Sizing::Fixed(v)=>v,_=>0.};
        if t.collapsed {t.base=0.;}
        t
    }).collect();
    let probe=TrackList{tracks,names:list.names.clone(),inherited:list.inherited.clone()};
    let (positions,_)=positions(&probe,gap,None,ContentDistribution::FlexStart);
    section(&probe,&positions,start,end,gap,names)
}

pub(super) fn clamp_placement(pair:Result<(i32,i32),i32>,count:usize)->Result<(i32,i32),i32> {
    let n=count.max(1).min(i32::MAX as usize) as i32;
    match pair {
        Ok((start,end))=>{
            let start=start.clamp(0,n-1);
            Ok((start,end.clamp(start+1,n)))
        }
        Err(span)=>Err(span.clamp(1,n)),
    }
}

pub(super) fn positions(list:&TrackList,gap:f32,avail:Option<f32>,align:ContentDistribution)->(Vec<f32>,f32) {
    if let Some(inherited)=&list.inherited {
        return (inherited.starts.iter().map(|p|p-inherited.inset_start).collect(),inherited.extent()-inherited.inset_start-inherited.inset_end);
    }
    super::track_positions(&list.tracks,gap,avail,align)
}

/// Match the shared grid lines, applying the subgrid's own edge layer only
/// to items touching it. A non-normal child gap adjusts internal item edges.
pub(super) fn item_edges(b:&LayoutBox,it:&mut GridItem,column_gap:f32,row_gap:f32) {
    for (rows,gap) in [(false,column_gap),(true,row_gap)] {
        let Some(tracks)=shared(b,rows) else {continue;};
        let (start,end)=if rows {it.area.row} else {it.area.col};
        let before=if start==0 {tracks.inset_start} else {tracks.edge_before[start as usize]+(gap-tracks.gap)*0.5};
        let after=if end as usize==tracks.sizes.len() {tracks.inset_end} else {tracks.edge_after[end as usize-1]+(gap-tracks.gap)*0.5};
        if rows {it.edges.margin.top+=before;it.edges.margin.bottom+=after;}
        else {it.edges.margin.left+=before;it.edges.margin.right+=after;}
    }
}

/// Shared-axis sizing and alignment are ignored, including automatic margins.
/// The subgrid's margin box must fill exactly the inherited grid area.
pub(super) fn stretch_item(b:&LayoutBox,it:&mut GridItem) {
    if shared(b,false).is_some() {
        it.justify=ItemAlign::Stretch;it.edges.auto.left=false;it.edges.auto.right=false;
    }
    if shared(b,true).is_some() {
        it.align=ItemAlign::Stretch;it.edges.auto.top=false;it.edges.auto.bottom=false;
    }
}

/// Items with identical spans affect track sizing only through the maximum
/// of each intrinsic contribution. Compact in place before walking another
/// subgrid level; hypothetical edge items must not duplicate exponentially.
pub(super) fn compact_contributions(items:&mut Vec<Contribution>) {
    items.sort_unstable_by_key(|c|(c.start,c.end));
    let mut write=0;
    for read in 0..items.len() {
        let item=items[read];
        if write>0&&items[write-1].start==item.start&&items[write-1].end==item.end {
            items[write-1].min=items[write-1].min.max(item.min);
            items[write-1].max=items[write-1].max.max(item.max);
        } else {items[write]=item;write+=1;}
    }
    items.truncate(write);
}

/// Grid2 §9 accounts for an edge with empty adjacent tracks using a
/// hypothetical item spanning those tracks. An entirely empty subgrid
/// contributes nothing. Real items touching the edge already carry its layer.
pub(super) fn empty_edge_contributions(b:&LayoutBox,rows:bool,gap:f32,items:&mut Vec<Contribution>) {
    compact_contributions(items);
    let Some(axis)=shared(b,rows) else {return;};
    let Some(first)=items.iter().map(|c|c.start).min() else {return;};
    let last=items.iter().map(|c|c.end).max().unwrap();
    let count=items.len();
    let internal=(gap-axis.gap)*0.5;
    for i in 0..count {
        let item=items[i];
        if first>0&&item.start==first {
            let delta=axis.inset_start-axis.edge_before[item.start]-internal;
            items.push(Contribution{start:0,end:item.end,min:item.min+delta,max:item.max+delta});
        }
        if last<axis.sizes.len()&&item.end==last {
            let delta=axis.inset_end-axis.edge_after[item.end-1]-internal;
            items.push(Contribution{start:item.start,end:axis.sizes.len(),min:item.min+delta,max:item.max+delta});
        }
    }
    compact_contributions(items);
}

struct Saved {
    style:Rc<ComputedStyle>,columns:Option<AxisInheritance>,rows:Option<AxisInheritance>,
}
fn invalidate(b:&mut LayoutBox) {
    b.intrinsic_cache=None;b.layout_cache=None;b.reused_layout=false;
    for child in &mut b.children {invalidate(child);}
}
fn inset(source:Option<AxisInheritance>,before:f32,after:f32)->Option<AxisInheritance> {
    source.map(|source|match source {
        AxisInheritance::Tracks(tracks)=>{let mut tracks=(*tracks).clone();tracks.inset_start+=before;tracks.inset_end+=after;AxisInheritance::Tracks(Arc::new(tracks))},
        AxisInheritance::Stacking=>AxisInheritance::Stacking,
    })
}
fn install(b:&mut LayoutBox,columns:Option<AxisInheritance>,rows:Option<AxisInheritance>,reference:f32,saved:&mut Vec<Saved>) {
    let (use_columns,use_rows)=requests(b);
    let edges=ItemEdges::new(&b.style,reference);
    let columns=if use_columns {inset(columns,edges.margin.left+b.style.padding.left.resolve_or_zero(reference)+b.style.border_width.left,edges.margin.right+b.style.padding.right.resolve_or_zero(reference)+b.style.border_width.right)} else {None};
    let rows=if use_rows {inset(rows,edges.margin.top+b.style.padding.top.resolve_or_zero(reference)+b.style.border_width.top,edges.margin.bottom+b.style.padding.bottom.resolve_or_zero(reference)+b.style.border_width.bottom)} else {None};
    saved.push(Saved{style:b.style.clone(),columns:b.grid_columns.clone(),rows:b.grid_rows.clone()});
    b.grid_columns=columns.clone();b.grid_rows=rows.clone();
    let mut style=(*b.style).clone();
    if matches!(columns.as_ref(),Some(AxisInheritance::Tracks(_))) {style.width=Length::Auto;style.min_width=Length::ZERO;style.max_width=Length::Auto;}
    if matches!(rows.as_ref(),Some(AxisInheritance::Tracks(_))) {style.height=Length::Auto;style.min_height=Length::ZERO;style.max_height=Length::Auto;}
    b.style=Rc::new(style);
    if b.wm_wrapper {
        let (columns,rows)=through_wrapper(columns,rows,b.wm_kind);
        if let Some(inner)=b.children.first_mut() {install(inner,columns,rows,reference,saved);}
    }
}
fn restore(b:&mut LayoutBox,saved:&mut impl Iterator<Item=Saved>) {
    let old=saved.next().unwrap();b.style=old.style;b.grid_columns=old.columns;b.grid_rows=old.rows;
    if b.wm_wrapper {if let Some(inner)=b.children.first_mut() {restore(inner,saved);}}
}

impl<'a> Layouter<'a> {
    pub(super) fn with_subgrid<R>(&mut self,b:&mut LayoutBox,columns:Option<AxisInheritance>,rows:Option<AxisInheritance>,reference:f32,body:impl FnOnce(&mut Self,&mut LayoutBox)->R)->R {
        let (column_request,row_request)=requests(b);
        if (!column_request||columns.is_none())&&(!row_request||rows.is_none()) {return body(self,b);}
        let mut saved=Vec::new();invalidate(b);install(b,columns,rows,reference,&mut saved);
        let result=body(self,b);
        restore(b,&mut saved.into_iter());invalidate(b);result
    }

    /// Contributions in the requested axis, in this box's local grid lines.
    /// The caller has supplied only placement geometry for that axis. Its
    /// descendants, rather than the subgrid's preferred size, size the parent.
    pub(super) fn subgrid_axis_contributions(&mut self,b:&mut LayoutBox,rows:bool,inline_size:Option<f32>,phase:MeasurePhase)->Vec<Contribution> {
        if b.wm_wrapper {
            let reverse=if rows {b.wm_kind==WmKind::Ccw} else {b.wm_kind==WmKind::Cw};
            let count=shared(b,rows).map_or(0,|axis|axis.sizes.len());
            let Some(inner)=b.children.first_mut() else {return Vec::new();};
            let inner_inline=shared(inner,false).map(|axis|(axis.extent()-axis.inset_start-axis.inset_end).max(0.));
            let mut result=self.subgrid_axis_contributions(inner,!rows,inner_inline,phase);
            if reverse {for c in &mut result {let start=c.start;c.start=count.saturating_sub(c.end);c.end=count.saturating_sub(start);}}
            return result;
        }
        if b.style.grid_lanes||inherits_stacking(b) {
            return self.lane_axis_contributions(b,rows,inline_size,phase);
        }
        let style=b.style.clone();
        let col_gap=gap(b,false,inline_size.unwrap_or(0.));
        let row_gap=gap(b,true,0.);
        let (items,mut cols,row_tracks)=self.place_items(b,&style,inline_size,None,col_gap,row_gap,0.,0.);
        let mut result=if !rows {
            self.projected_columns(b,&items,&cols,Some(&row_tracks),col_gap,row_gap,inline_size.unwrap_or(0.),phase)
        } else {
            if cols.inherited.is_none() {
                let contributions=self.projected_columns(b,&items,&cols,Some(&row_tracks),col_gap,row_gap,inline_size.unwrap_or(0.),phase);
                let space=inline_size.map(GridSpace::Definite).unwrap_or(GridSpace::MaxContent);
                size_tracks_with_space(&mut cols.tracks,&contributions,space,col_gap,false);
            }
            let (positions,_)=positions(&cols,col_gap,inline_size,style.justify_content);
            self.projected_rows(b,&items,&row_tracks,&cols,&positions,col_gap,row_gap,inline_size.unwrap_or(0.),None,phase)
        };
        empty_edge_contributions(b,rows,if rows {row_gap} else {col_gap},&mut result);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flex::ItemEdges;
    use crate::tree::BoxKind;
    use rabbithole_style::{Edges,ItemAlign};

    fn parent()->TrackList {
        let tracks=[100.,150.].into_iter().map(|size|{let mut t=Track::new(Sizing::Fixed(size),Sizing::Fixed(size));t.base=size;t.limit=Some(size);t}).collect();
        TrackList{tracks,names:vec![vec!["first".into()],vec!["middle".into()],vec!["last".into()]],inherited:None}
    }
    fn child()->LayoutBox {
        let mut style=ComputedStyle::default();
        style.grid_template_columns=vec![TrackListItem::Subgrid{line_names:vec![]}];
        style.margin=Edges::all(Length::Px(2.));style.padding=Edges::all(Length::Px(5.));style.border_width=Edges::all(3.);
        let mut b=LayoutBox::new(BoxKind::Block,None,Rc::new(style));b.formatting=Formatting::Grid;b
    }
    fn item(start:i32,end:i32)->GridItem {
        GridItem{idx:0,area:super::super::Area{col:(start,end),row:(0,1)},edges:ItemEdges::new(&ComputedStyle::default(),0.),justify:ItemAlign::Stretch,align:ItemAlign::Stretch,column_names:true,row_names:true}
    }

    #[test]
    fn inherited_positions_keep_parent_lines_and_edge_layers_separate() {
        let parent=parent();let mut b=child();let mut saved=Vec::new();
        install(&mut b,Some(section(&parent,&[0.,150.],0,2,50.,true)),None,300.,&mut saved);
        let list=tracks(&b,false,None,50.);
        assert_eq!(positions(&list,50.,None,ContentDistribution::Center).0,[-10.,140.]);
        assert_eq!(gap(&b,false,300.),50.);
        let mut first=item(0,1);let mut last=item(1,2);
        item_edges(&b,&mut first,50.,0.);item_edges(&b,&mut last,50.,0.);
        assert_eq!((first.edges.margin.left,first.edges.margin.right),(10.,0.));
        assert_eq!((last.edges.margin.left,last.edges.margin.right),(0.,10.));
    }

    #[test]
    fn an_explicit_gap_adjusts_internal_edges_without_moving_grid_lines() {
        let parent=parent();let mut b=child();let mut style=(*b.style).clone();
        style.column_gap=Length::Px(25.);style.column_gap_normal=false;b.style=Rc::new(style);
        let mut saved=Vec::new();install(&mut b,Some(section(&parent,&[0.,150.],0,2,50.,true)),None,300.,&mut saved);
        assert_eq!(gap(&b,false,300.),25.);
        let mut first=item(0,1);let mut last=item(1,2);
        item_edges(&b,&mut first,25.,0.);item_edges(&b,&mut last,25.,0.);
        assert_eq!((first.edges.margin.left,first.edges.margin.right),(10.,-12.5));
        assert_eq!((last.edges.margin.left,last.edges.margin.right),(-12.5,10.));
        assert_eq!(positions(&tracks(&b,false,None,25.),25.,None,ContentDistribution::Stretch).0,[-10.,140.]);
    }

    #[test]
    fn oversized_edges_do_not_truncate_parent_track_advance() {
        let parent=parent();let mut b=child();let mut style=(*b.style).clone();style.padding.left=Length::Px(155.);b.style=Rc::new(style);
        let mut saved=Vec::new();install(&mut b,Some(section(&parent,&[0.,150.],0,2,50.,true)),None,300.,&mut saved);
        assert_eq!(positions(&tracks(&b,false,None,50.),50.,None,ContentDistribution::FlexStart).0,[-160.,-10.]);
    }

    #[test]
    fn orthogonal_mapping_reverses_only_the_axis_with_negative_orientation() {
        let parent=parent();let source=section(&parent,&[0.,150.],0,2,50.,true);
        let (columns,rows)=through_wrapper(Some(source.clone()),None,WmKind::Cw);
        assert!(columns.is_none());
        let Some(AxisInheritance::Tracks(rows))=rows else {panic!("missing mapped rows")};
        assert_eq!(rows.sizes,[150.,100.]);assert_eq!(rows.starts,[0.,200.]);
        assert_eq!(rows.names[0],["last"]);
        let (_,rows)=through_wrapper(Some(source),None,WmKind::Reflect);
        let Some(AxisInheritance::Tracks(rows))=rows else {panic!("missing mapped rows")};
        assert_eq!(rows.sizes,[100.,150.]);assert_eq!(rows.starts,[0.,150.]);
    }

    #[test]
    fn scoped_inheritance_preserves_computed_requests_and_restores_styles() {
        let mut b=child();let original=b.style.clone();let parent=parent();let mut saved=Vec::new();
        install(&mut b,Some(section(&parent,&[0.,150.],0,2,50.,true)),None,300.,&mut saved);
        assert!(matches!(b.style.grid_template_columns[0],TrackListItem::Subgrid{..}));
        assert!(b.grid_columns.is_some());
        restore(&mut b,&mut saved.into_iter());
        assert!(Rc::ptr_eq(&original,&b.style));assert!(b.grid_columns.is_none());
        assert_eq!(local_names(&[SubgridLineNames::Repeat{count:Some(1000000),names:vec![vec!["x".into()]]}],3).len(),3);
    }

    #[test]
    fn placements_are_clamped_to_shared_lines_without_implicit_tracks() {
        assert_eq!(clamp_placement(Ok((-9,-2)),2),Ok((0,1)));
        assert_eq!(clamp_placement(Ok((1,30)),2),Ok((1,2)));
        assert_eq!(clamp_placement(Ok((40,50)),2),Ok((1,2)));
        assert_eq!(clamp_placement(Err(12),2),Err(2));
    }

    #[test]
    fn nested_sections_carry_the_ancestor_edge_layer_once() {
        let parent=parent();let mut outer=child();let mut saved=Vec::new();
        install(&mut outer,Some(section(&parent,&[0.,150.],0,2,50.,true)),None,300.,&mut saved);
        let inherited=tracks(&outer,false,None,50.);
        let (positions,_)=positions(&inherited,50.,None,ContentDistribution::Center);
        let nested=section(&inherited,&positions,0,2,50.,true);
        let mut inner=child();let mut saved=Vec::new();install(&mut inner,Some(nested),None,300.,&mut saved);
        let axis=shared(&inner,false).unwrap();assert_eq!(axis.inset_start,20.);assert_eq!(axis.inset_end,20.);
        let mut first=item(0,1);item_edges(&inner,&mut first,50.,0.);assert_eq!(first.edges.margin.left,20.);
    }

    #[test]
    fn initial_cross_axis_estimate_uses_the_definite_maximum() {
        let list=TrackList{tracks:vec![Track::new(Sizing::Fixed(10.),Sizing::Fixed(100.)),Track::new(Sizing::Auto,Sizing::Auto)],names:vec![vec![],vec![],vec![]],inherited:None};
        let Some(AxisInheritance::Tracks(axis))=fixed_section(&list,0,1,0.,true) else {panic!("missing definite maximum")};
        assert_eq!(axis.sizes,[100.]);
        assert!(fixed_section(&list,0,2,0.,true).is_none());
    }

    #[test]
    fn nested_normal_gap_keeps_ancestor_internal_adjustments() {
        let parent=parent();let mut outer=child();let mut style=(*outer.style).clone();
        style.column_gap=Length::Px(20.);style.column_gap_normal=false;outer.style=Rc::new(style);
        let mut saved=Vec::new();install(&mut outer,Some(section(&parent,&[0.,150.],0,2,50.,true)),None,300.,&mut saved);
        let list=tracks(&outer,false,None,20.);let (positions,_)=positions(&list,20.,None,ContentDistribution::Center);
        let mut inner=child();let mut saved=Vec::new();install(&mut inner,Some(section(&list,&positions,0,2,20.,true)),None,300.,&mut saved);
        assert_eq!(gap(&inner,false,300.),20.);
        let mut first=item(0,1);let mut last=item(1,2);item_edges(&inner,&mut first,20.,0.);item_edges(&inner,&mut last,20.,0.);
        assert_eq!(first.edges.margin.right,-15.);assert_eq!(last.edges.margin.left,-15.);
        let mut first=item(0,1);item_edges(&inner,&mut first,35.,0.);assert_eq!(first.edges.margin.right,-7.5);
    }

    #[test]
    fn empty_subgrids_do_not_synthesize_padding_contributions() {
        let parent=parent();let mut b=child();let mut saved=Vec::new();
        install(&mut b,Some(section(&parent,&[0.,150.],0,2,50.,true)),None,300.,&mut saved);
        let mut items=vec![];empty_edge_contributions(&b,false,50.,&mut items);assert!(items.is_empty());
    }

    #[test]
    fn hypothetical_edge_items_cover_empty_tracks_without_moving_the_real_item() {
        let parent=parent();let mut b=child();let mut saved=Vec::new();
        install(&mut b,Some(section(&parent,&[0.,150.],0,2,50.,true)),None,300.,&mut saved);
        let real=Contribution{start:1,end:2,min:30.,max:50.};let mut items=vec![real];
        empty_edge_contributions(&b,false,50.,&mut items);
        assert_eq!(items,[Contribution{start:0,end:2,min:40.,max:60.},real]);
    }

    #[test]
    fn virtual_items_with_the_same_span_keep_both_independent_maxima() {
        let mut items=vec![Contribution{start:1,end:3,min:10.,max:100.},Contribution{start:0,end:1,min:5.,max:5.},Contribution{start:1,end:3,min:20.,max:50.}];
        compact_contributions(&mut items);
        assert_eq!(items,[Contribution{start:0,end:1,min:5.,max:5.},Contribution{start:1,end:3,min:20.,max:100.}]);
    }
}
