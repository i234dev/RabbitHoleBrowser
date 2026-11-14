//! Per-axis subgrid inputs. Parent track geometry is immutable; child sizing
//! must not maximize inherited tracks or invent implicit ones.
use std::rc::Rc;
use std::sync::Arc;

use super::{expand_tracks, GridItem, Sizing, Track, TrackList};
use crate::block::Layouter;
use crate::flex::ItemEdges;
use crate::tree::{Formatting, LayoutBox, WmKind};
use rabbithole_style::{ComputedStyle, ContentDistribution, Length, SubgridLineNames, TrackListItem};

#[derive(Clone, Debug)]
pub(crate) struct SharedTracks {
    pub(super) sizes: Vec<f32>,
    pub(super) starts: Vec<f32>,
    pub(super) names: Vec<Vec<String>>,
    pub(super) collapsed: Vec<bool>,
    pub(super) gap: f32,
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
    let tracks=SharedTracks {
        sizes:list.tracks[start..end].iter().map(|t|t.base).collect(),
        starts:positions[start..end].iter().map(|p|p-origin).collect(),
        names:if names {list.names[start..=end].to_vec()} else {vec![vec![];end-start+1]},
        collapsed:list.tracks[start..end].iter().map(|t|t.collapsed).collect(),
        gap,inset_start:0.,inset_end:0.,
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

/// Fixed tracks can be supplied before measuring the perpendicular axis.
/// Intrinsic/flexible tracks must wait for their own contribution pass.
pub(super) fn fixed_section(list:&TrackList,start:usize,end:usize,gap:f32,names:bool)->Option<AxisInheritance> {
    if list.inherited.is_some() {
        let (positions,_)=positions(list,gap,None,ContentDistribution::FlexStart);
        return Some(section(list,&positions,start,end,gap,names));
    }
    if !list.tracks.iter().all(|t|matches!((t.min,t.max),(Sizing::Fixed(a),Sizing::Fixed(b)) if a==b)) {return None;}
    let tracks=list.tracks.iter().cloned().map(|mut t|{if let Sizing::Fixed(size)=t.min {t.base=if t.collapsed {0.} else {size};t.limit=Some(t.base);}t}).collect();
    let fixed=TrackList{tracks,names:list.names.clone(),inherited:None};
    let (positions,_)=positions(&fixed,gap,None,ContentDistribution::FlexStart);
    Some(section(&fixed,&positions,start,end,gap,names))
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
        let before=if start==0 {tracks.inset_start} else {(gap-tracks.gap)*0.5};
        let after=if end as usize==tracks.sizes.len() {tracks.inset_end} else {(gap-tracks.gap)*0.5};
        if rows {it.edges.margin.top+=before;it.edges.margin.bottom+=after;}
        else {it.edges.margin.left+=before;it.edges.margin.right+=after;}
    }
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
        GridItem{idx:0,area:super::super::Area{col:(start,end),row:(0,1)},edges:ItemEdges::new(&ComputedStyle::default(),0.),justify:ItemAlign::Stretch,align:ItemAlign::Stretch}
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
}
