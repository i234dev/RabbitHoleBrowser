//! Per-axis subgrid inputs. Parent track geometry is immutable; child sizing
//! must not maximize inherited tracks or invent implicit ones.
use std::rc::Rc;
use std::sync::Arc;
use std::cell::Cell;

use super::{expand_tracks, size_tracks_with_space, Contribution, GridGeometry, GridItem, GridSpace, MeasurePhase, Sizing, Track, TrackList};
use crate::block::Layouter;
use crate::flex::ItemEdges;
use crate::tree::{Formatting, LayoutBox, WmKind};
use rabbithole_style::{ComputedStyle, ContentDistribution, ItemAlign, Length, SubgridLineNames, TrackListItem};

/// A per-layout cap on newly introduced recursive subgrid work. Exhaustion is
/// explicit: the engine's tab panic boundary reports the failed layout. It
/// never returns a truncated contribution set as valid geometry.
pub(crate) struct WorkBudget {
    remaining:usize,
    depth:Option<Rc<Cell<usize>>>,
}
impl Default for WorkBudget {
    fn default()->Self {Self{remaining:1_000_000,depth:None}}
}
pub(super) struct DepthGuard(Rc<Cell<usize>>);
impl Drop for DepthGuard {
    fn drop(&mut self) {self.0.set(self.0.get().saturating_sub(1));}
}
impl WorkBudget {
    pub(super) fn charge(&mut self,work:usize) {
        let Some(remaining)=self.remaining.checked_sub(work) else {panic!("Subgrid layout work budget exceeded");};
        self.remaining=remaining;
    }
    fn enter(&mut self,work:usize)->DepthGuard {
        if self.depth.as_ref().is_some_and(|depth|depth.get()>=128) {panic!("Subgrid layout recursion budget exceeded");}
        self.charge(work);
        let depth=self.depth.get_or_insert_with(||Rc::new(Cell::new(0)));
        depth.set(depth.get()+1);DepthGuard(depth.clone())
    }

    /// Reserve work before helpers clone full placement track/name lists.
    pub(super) fn tracks(&mut self,list:&TrackList) {
        self.charge(list.tracks.len().saturating_mul(8));
        for names in &list.names {
            self.charge(1);
            for name in names {self.charge(name.len().saturating_add(1));}
        }
    }

    /// Each scoped measure visits the entire subtree to count it, invalidate
    /// before layout, and invalidate after restoration. Charge before growing
    /// the pending stack or installing any temporary style.
    fn subtree(&mut self,node:&LayoutBox) {
        self.charge(3);
        let mut pending=vec![node];
        while let Some(node)=pending.pop() {
            self.charge(node.children.len().saturating_mul(3));
            pending.extend(node.children.iter());
        }
    }
}

fn source_work(source:&Option<AxisInheritance>)->usize {
    match source {
        Some(AxisInheritance::Tracks(axis))=>axis.sizes.len().saturating_mul(8).saturating_add(axis.names.iter().flatten().map(|name|name.len()).fold(0,usize::saturating_add)),
        _=>1,
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SharedTracks {
    pub(super) sizes: Vec<f32>,
    pub(super) starts: Vec<f32>,
    pub(super) names: Vec<Vec<String>>,
    pub(super) collapsed: Vec<bool>,
    pub(super) gap: f32,
    /// Placement-only geometry must not resolve cyclic percentage edges.
    pub(super) definite: bool,
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

/// Fixed line-name groups establish the automatic span without materializing
/// repeats. Auto-fill then fills the resulting used span; it cannot enlarge
/// the span from which its own repetition count is determined.
pub(super) fn automatic_span(b:&LayoutBox,rows:bool)->Option<usize> {
    if b.wm_wrapper {return b.children.first().and_then(|inner|automatic_span(inner,!rows));}
    if b.formatting!=Formatting::Grid {return None;}
    let list=if rows {&b.style.grid_template_rows} else {&b.style.grid_template_columns};
    let Some(TrackListItem::Subgrid{line_names})=list.first() else {return None;};
    let lines=line_names.iter().map(|part|match part {
        SubgridLineNames::Names(_)=>1,
        SubgridLineNames::Repeat{count:Some(count),names}=>names.len().saturating_mul(*count as usize),
        SubgridLineNames::Repeat{count:None,..}=>0,
    }).fold(0,usize::saturating_add);
    Some(lines.saturating_sub(1).max(1))
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
        definite:list.inherited.as_ref().is_none_or(|parent|parent.definite),
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
#[cfg(test)]
fn fixed_section(list:&TrackList,start:usize,end:usize,gap:f32,names:bool)->Option<AxisInheritance> {
    fixed_section_aligned(list,start,end,gap,names,None,ContentDistribution::FlexStart)
}
#[allow(clippy::too_many_arguments)]
pub(super) fn fixed_section_aligned(list:&TrackList,start:usize,end:usize,gap:f32,names:bool,available:Option<f32>,align:ContentDistribution)->Option<AxisInheritance> {
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
    let available=if list.tracks.iter().all(|t|matches!(t.max,Sizing::Fixed(_))) {available} else {None};
    let (positions,_)=positions(&fixed,gap,available,align);
    Some(section(&fixed,&positions,start,end,gap,names))
}

/// During contribution collection the span and line names are definite, but
/// intrinsic track sizes are not. Keep that placement geometry separate from
/// used geometry; grandchildren contribute directly without sizing this axis.
#[cfg(test)]
fn sizing_section(list:&TrackList,start:usize,end:usize,gap:f32,names:bool)->AxisInheritance {
    sizing_section_aligned(list,start,end,gap,names,None,ContentDistribution::FlexStart)
}
#[allow(clippy::too_many_arguments)]
pub(super) fn sizing_section_aligned(list:&TrackList,start:usize,end:usize,gap:f32,names:bool,available:Option<f32>,align:ContentDistribution)->AxisInheritance {
    let tracks=list.tracks.iter().cloned().map(|mut t| {
        t.base=match t.min {Sizing::Fixed(v)=>v,_=>0.};
        if t.collapsed {t.base=0.;}
        t
    }).collect();
    let probe=TrackList{tracks,names:list.names.clone(),inherited:list.inherited.clone()};
    let available=if list.tracks.iter().all(|t|matches!((t.min,t.max),(Sizing::Fixed(a),Sizing::Fixed(b)) if a==b)) {available} else {None};
    let (positions,_)=positions(&probe,gap,available,align);
    let mut source=section(&probe,&positions,start,end,gap,names);
    if let AxisInheritance::Tracks(axis)=&mut source {
        let definite=list.inherited.as_ref().map_or_else(||list.tracks[start..end].iter().all(|t|matches!((t.min,t.max),(Sizing::Fixed(a),Sizing::Fixed(b)) if a==b)),|parent|parent.definite);
        Arc::make_mut(axis).definite=definite;
    }
    source
}

pub(super) fn scope_reference(columns:&Option<AxisInheritance>,fallback:f32)->f32 {
    match columns {Some(AxisInheritance::Tracks(axis))=>if axis.definite {axis.extent()} else {0.},_=>fallback}
}

pub(super) fn known_block_size(b:&LayoutBox,reference:f32)->Option<f32> {
    if let Some(axis)=shared(b,true) {
        if axis.definite {return Some((axis.extent()-axis.inset_start-axis.inset_end).max(0.));}
    }
    let pb=b.style.padding.top.resolve_or_zero(reference)+b.style.padding.bottom.resolve_or_zero(reference)+b.style.border_width.top+b.style.border_width.bottom;
    crate::flex::resolve_size(b.style.height,None,b.style.box_sizing==rabbithole_style::BoxSizing::BorderBox,pb)
}

/// Preserve the inherited/hypothetical margin layer while resolving the
/// child's own percentage edges against its grid area's inline size.
pub(super) fn rebase_item(item:&GridItem,style:&ComputedStyle,old_reference:f32,new_reference:f32)->GridItem {
    let old=ItemEdges::new(style,old_reference);let mut result=*item;
    result.edges=ItemEdges::new(style,new_reference);
    result.edges.margin.left+=item.edges.margin.left-old.margin.left;
    result.edges.margin.right+=item.edges.margin.right-old.margin.right;
    result.edges.margin.top+=item.edges.margin.top-old.margin.top;
    result.edges.margin.bottom+=item.edges.margin.bottom-old.margin.bottom;
    result
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
fn item_layer(tracks:&SharedTracks,start:usize,end:usize,gap:f32)->(f32,f32) {
    let before=if start==0 {tracks.inset_start} else {tracks.edge_before[start]+(gap-tracks.gap)*0.5};
    let after=if end==tracks.sizes.len() {tracks.inset_end} else {tracks.edge_after[end-1]+(gap-tracks.gap)*0.5};
    (before,after)
}
pub(super) fn item_edges(b:&LayoutBox,it:&mut GridItem,column_gap:f32,row_gap:f32) {
    for (rows,gap) in [(false,column_gap),(true,row_gap)] {
        let Some(tracks)=shared(b,rows) else {continue;};
        let (start,end)=if rows {it.area.row} else {it.area.col};
        let (before,after)=item_layer(&tracks,start as usize,end as usize,gap);
        if rows {it.edges.margin.top+=before;it.edges.margin.bottom+=after;}
        else {it.edges.margin.left+=before;it.edges.margin.right+=after;}
    }
}

/// An auto-positioned item in a lanes subgrid receives the largest possible
/// contribution at each edge in every hypothetical placement (Grid3 §3.4.1).
pub(super) fn largest_automatic_layer(b:&LayoutBox,rows:bool,gap:f32,span:usize,positions:&[usize])->Option<(f32,f32)> {
    let tracks=shared(b,rows)?;
    let mut largest:Option<(f32,f32)>=None;
    for &start in positions {
        let (before,after)=item_layer(&tracks,start,(start+span).min(tracks.sizes.len()),gap);
        largest=Some(match largest {Some((left,right))=>(left.max(before),right.max(after)),None=>(before,after)});
    }
    largest
}

/// Carry the hypothetical item's edge layer into a nested subgrid too. Its
/// geometry still follows the shared lines; only these contribution layers
/// can differ from the actual placement used during final layout.
pub(super) fn item_layer_source(mut source:AxisInheritance,item:&GridItem,style:&ComputedStyle,rows:bool,reference:f32)->AxisInheritance {
    if let AxisInheritance::Tracks(axis)=&mut source {
        let edges=ItemEdges::new(style,reference);
        let (before,after)=if rows {(item.edges.margin.top-edges.margin.top,item.edges.margin.bottom-edges.margin.bottom)} else {(item.edges.margin.left-edges.margin.left,item.edges.margin.right-edges.margin.right)};
        if axis.inset_start!=before||axis.inset_end!=after {let axis=Arc::make_mut(axis);axis.inset_start=before;axis.inset_end=after;}
    }
    source
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

/// Grid3 §3.4.1 ignores a sub-item's placement when its own subgrid has an
/// automatic position in a lanes parent. The parent already copies the
/// subgrid at every possible start; cover every local position of this span.
pub(super) fn automatic_contributions(budget:&mut WorkBudget,items:Vec<Contribution>,count:usize)->Vec<Contribution> {
    let mut out=Vec::new();
    if count==0 {return out;}
    for item in items {
        let span=item.end.saturating_sub(item.start).max(1).min(count);
        let positions=count-span+1;budget.charge(positions);
        for start in 0..positions {out.push(Contribution{start,end:start+span,min:item.min,max:item.max});}
    }
    compact_contributions(&mut out);out
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
    let resolve=|value:Length|if value.is_auto() {Length::Auto} else {Length::Px(value.resolve_or_zero(reference))};
    style.margin=rabbithole_style::Edges{left:resolve(style.margin.left),right:resolve(style.margin.right),top:resolve(style.margin.top),bottom:resolve(style.margin.bottom)};
    style.padding=rabbithole_style::Edges{left:Length::Px(style.padding.left.resolve_or_zero(reference)),right:Length::Px(style.padding.right.resolve_or_zero(reference)),top:Length::Px(style.padding.top.resolve_or_zero(reference)),bottom:Length::Px(style.padding.bottom.resolve_or_zero(reference))};
    if matches!(columns.as_ref(),Some(AxisInheritance::Tracks(_))) {style.width=Length::Auto;style.min_width=Length::ZERO;style.max_width=Length::Auto;}
    if matches!(rows.as_ref(),Some(AxisInheritance::Tracks(_))) {style.height=Length::Auto;style.min_height=Length::ZERO;style.max_height=Length::Auto;}
    b.style=Rc::new(style);
    if b.wm_wrapper {
        let (columns,rows)=through_wrapper(columns,rows,b.wm_kind);
        if let Some(inner)=b.children.first_mut() {install(inner,columns,rows,reference,saved);}
    }
}
fn restore(b:&mut LayoutBox,saved:&mut impl Iterator<Item=Saved>) {
    let Some(old)=saved.next() else {return;};b.style=old.style;b.grid_columns=old.columns;b.grid_rows=old.rows;
    if b.wm_wrapper {if let Some(inner)=b.children.first_mut() {restore(inner,saved);}}
}

struct Scope<'b> {
    node:&'b mut LayoutBox,
    saved:Vec<Saved>,
}
impl<'b> Scope<'b> {
    fn new(node:&'b mut LayoutBox,columns:Option<AxisInheritance>,rows:Option<AxisInheritance>,reference:f32)->Self {
        let mut scope=Self{node,saved:Vec::new()};
        invalidate(scope.node);
        install(scope.node,columns,rows,reference,&mut scope.saved);
        scope
    }
}
impl Drop for Scope<'_> {
    fn drop(&mut self) {
        restore(self.node,&mut self.saved.drain(..));
        invalidate(self.node);
    }
}

impl<'a> Layouter<'a> {
    pub(super) fn with_subgrid<R>(&mut self,b:&mut LayoutBox,columns:Option<AxisInheritance>,rows:Option<AxisInheritance>,reference:f32,body:impl FnOnce(&mut Self,&mut LayoutBox)->R)->R {
        let (column_request,row_request)=requests(b);
        if (!column_request||columns.is_none())&&(!row_request||rows.is_none()) {return body(self,b);}
        let reference=scope_reference(&columns,reference);
        let mut layers=1usize;let mut node=&*b;
        while node.wm_wrapper {let Some(inner)=node.children.first() else {break;};layers=layers.saturating_add(1);node=inner;}
        self.grid_work.charge(source_work(&columns).saturating_add(source_work(&rows)).saturating_mul(layers.saturating_mul(2)));
        self.grid_work.subtree(b);
        let scope=Scope::new(b,columns,rows,reference);
        body(self,&mut *scope.node)
    }

    /// Contributions in the requested axis, in this box's local grid lines.
    /// The caller has supplied only placement geometry for that axis. Its
    /// descendants, rather than the subgrid's preferred size, size the parent.
    pub(super) fn subgrid_axis_contributions(&mut self,b:&mut LayoutBox,rows:bool,inline_size:Option<f32>,phase:MeasurePhase)->Vec<Contribution> {
        let _depth=self.grid_work.enter(b.children.len().saturating_add(1));
        if b.wm_wrapper {
            let reverse=if rows {b.wm_kind==WmKind::Ccw} else {b.wm_kind==WmKind::Cw};
            let count=shared(b,rows).map_or(0,|axis|axis.sizes.len());
            let Some(inner)=b.children.first_mut() else {return Vec::new();};
            let inner_inline=shared(inner,false).filter(|axis|axis.definite).map(|axis|(axis.extent()-axis.inset_start-axis.inset_end).max(0.));
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
        let block=known_block_size(b,inline_size.unwrap_or(0.));
        let geometry=GridGeometry{inline:inline_size,block};
        let (items,mut cols,row_tracks)=self.place_items(b,&style,inline_size,block,col_gap,row_gap,0.,0.);
        let mut result=if !rows {
            self.projected_columns(b,&items,&cols,Some(&row_tracks),col_gap,row_gap,inline_size.unwrap_or(0.),geometry,phase)
        } else {
            if cols.inherited.is_none() {
                let contributions=self.projected_columns(b,&items,&cols,Some(&row_tracks),col_gap,row_gap,inline_size.unwrap_or(0.),geometry,phase);
                let space=inline_size.map(GridSpace::Definite).unwrap_or(GridSpace::MaxContent);
                size_tracks_with_space(&mut cols.tracks,&contributions,space,col_gap,false);
            }
            let (positions,_)=positions(&cols,col_gap,inline_size,style.justify_content);
            self.projected_rows(b,&items,&row_tracks,&cols,&positions,col_gap,row_gap,inline_size.unwrap_or(0.),block,phase)
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
        let mut style=ComputedStyle::initial();
        style.grid_template_columns=vec![TrackListItem::Subgrid{line_names:vec![]}];
        style.margin=Edges::all(Length::Px(2.));style.padding=Edges::all(Length::Px(5.));style.border_width=Edges::all(3.);
        let mut b=LayoutBox::new(BoxKind::Block,None,Rc::new(style));b.formatting=Formatting::Grid;b
    }
    fn item(start:i32,end:i32)->GridItem {
        GridItem{idx:0,area:super::super::Area{col:(start,end),row:(0,1)},edges:ItemEdges::new(&ComputedStyle::initial(),0.),justify:ItemAlign::Stretch,align:ItemAlign::Stretch,column_names:true,row_names:true,parent_lanes_auto:false}
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

    #[test]
    fn scoped_styles_are_restored_when_measurement_unwinds() {
        let parent=parent();let mut b=child();let original=b.style.clone();
        let source=section(&parent,&[0.,150.],0,2,50.,true);
        let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let scope=Scope::new(&mut b,Some(source),None,300.);
            assert!(scope.node.grid_columns.is_some());
            panic!("simulated measurement failure");
        }));
        assert!(result.is_err());assert!(Rc::ptr_eq(&original,&b.style));assert!(b.grid_columns.is_none());
    }

    #[test]
    fn work_budget_is_cumulative_and_failed_charges_do_not_wrap() {
        let mut budget=WorkBudget{remaining:3,depth:None};
        budget.charge(2);
        for work in [2,usize::MAX] {
            let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||budget.charge(work)));
            assert!(result.is_err());assert_eq!(budget.remaining,1);
        }
        budget.charge(1);assert_eq!(budget.remaining,0);
    }

    #[test]
    fn recursion_budget_is_restored_by_guards_and_separate_layouts_are_fresh() {
        let mut budget=WorkBudget::default();
        let guards:Vec<_>=(0..128).map(|_|budget.enter(1)).collect();
        let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||budget.enter(1)));
        assert!(result.is_err());assert_eq!(budget.depth.as_ref().unwrap().get(),128);
        drop(guards);assert_eq!(budget.depth.as_ref().unwrap().get(),0);
        drop(budget.enter(1));assert_eq!(budget.depth.as_ref().unwrap().get(),0);
        let fresh=WorkBudget::default();assert_eq!(fresh.remaining,1_000_000);assert!(fresh.depth.is_none());
    }

    #[test]
    fn automatic_parent_positions_ignore_the_subitems_local_placement() {
        let mut budget=WorkBudget::default();
        let out=automatic_contributions(&mut budget,vec![Contribution{start:1,end:2,min:40.,max:40.}],2);
        assert_eq!(out,[Contribution{start:0,end:1,min:40.,max:40.},Contribution{start:1,end:2,min:40.,max:40.}]);
    }

    #[test]
    fn automatic_spans_count_compact_line_groups_without_expanding_repeats() {
        let mut b=child();let mut style=(*b.style).clone();
        style.grid_template_columns=vec![TrackListItem::Subgrid{line_names:vec![SubgridLineNames::Names(vec!["a".into()]),SubgridLineNames::Repeat{count:Some(2),names:vec![vec!["b".into()]]}]}];
        b.style=Rc::new(style);assert_eq!(automatic_span(&b,false),Some(2));
        let mut style=(*b.style).clone();style.grid_template_columns=vec![TrackListItem::Subgrid{line_names:vec![SubgridLineNames::Repeat{count:Some(1_000_000),names:vec![vec![]]}]}];
        b.style=Rc::new(style);assert_eq!(automatic_span(&b,false),Some(999_999));
    }

    #[test]
    fn automatic_items_keep_the_largest_edge_layer_in_every_position() {
        let parent=parent();let mut b=child();let mut saved=Vec::new();
        install(&mut b,Some(section(&parent,&[0.,150.],0,2,50.,true)),None,300.,&mut saved);
        assert_eq!(largest_automatic_layer(&b,false,50.,1,&[0,1]),Some((10.,10.)));
        let list=tracks(&b,false,None,50.);let mut hypothetical=item(1,2);
        hypothetical.edges.margin.left=10.;hypothetical.edges.margin.right=10.;
        let source=sizing_section(&list,1,2,50.,true);
        let AxisInheritance::Tracks(axis)=item_layer_source(source,&hypothetical,&ComputedStyle::initial(),false,0.) else {panic!("missing source")};
        assert_eq!((axis.inset_start,axis.inset_end),(10.,10.));
    }

    #[test]
    fn placement_only_tracks_do_not_resolve_percentage_edges() {
        let list=TrackList{tracks:vec![Track::new(Sizing::Fixed(10.),Sizing::Auto)],names:vec![vec![],vec![]],inherited:None};
        let source=Some(sizing_section(&list,0,1,0.,true));assert_eq!(scope_reference(&source,300.),0.);
        let list=parent();let source=Some(sizing_section(&list,0,1,50.,true));assert_eq!(scope_reference(&source,300.),100.);
    }

    #[test]
    fn percentage_edges_are_resolved_once_in_the_area_space_and_restored() {
        let parent=parent();let mut b=child();let mut style=(*b.style).clone();
        style.margin=rabbithole_style::Edges::all(Length::ZERO);style.border_width=rabbithole_style::Edges::all(0.);style.padding=rabbithole_style::Edges::all(Length::Percent(10.));b.style=Rc::new(style);
        let original=b.style.clone();
        {
            let scope=Scope::new(&mut b,Some(section(&parent,&[0.,150.],0,1,50.,true)),None,100.);
            assert_eq!(scope.node.style.padding.left,Length::Px(10.));
            assert_eq!(shared(scope.node,false).unwrap().inset_start,10.);
        }
        assert!(Rc::ptr_eq(&original,&b.style));assert_eq!(b.style.padding.left,Length::Percent(10.));
    }

    #[test]
    fn rebasing_percentage_edges_preserves_the_inherited_margin_layer() {
        let mut style=ComputedStyle::initial();style.margin.left=Length::Percent(10.);style.padding.left=Length::Percent(10.);
        let mut item=item(0,1);item.edges=ItemEdges::new(&style,300.);item.edges.margin.left+=7.;
        let item=rebase_item(&item,&style,300.,100.);assert_eq!(item.edges.margin.left,17.);assert_eq!(item.edges.pb_h,10.);
    }

    #[test]
    fn initial_and_used_geometry_keep_definite_distributed_gaps() {
        let tracks=vec![Track::new(Sizing::Fixed(50.),Sizing::Fixed(50.));2];
        let mut list=TrackList{tracks,names:vec![vec![],vec![],vec![]],inherited:None};
        let Some(AxisInheritance::Tracks(initial))=fixed_section_aligned(&list,0,2,0.,true,Some(300.),ContentDistribution::SpaceBetween) else {panic!("missing initial geometry")};
        assert_eq!(initial.starts,[0.,250.]);assert_eq!(initial.extent(),300.);
        let AxisInheritance::Tracks(sizing)=sizing_section_aligned(&list,0,2,0.,true,Some(300.),ContentDistribution::SpaceBetween) else {panic!("missing sizing geometry")};
        assert_eq!(sizing.starts,[0.,250.]);
        for track in &mut list.tracks {track.base=50.;track.limit=Some(50.);}
        let (used,_)=positions(&list,0.,Some(300.),ContentDistribution::SpaceBetween);
        let AxisInheritance::Tracks(used)=section(&list,&used,0,2,0.,true) else {panic!("missing used geometry")};
        assert_eq!(used.starts,initial.starts);assert_eq!(used.extent(),300.);
    }

    #[test]
    fn an_indefinite_track_outside_the_span_prevents_initial_distribution() {
        let list=TrackList{tracks:vec![Track::new(Sizing::Fixed(50.),Sizing::Fixed(50.)),Track::new(Sizing::Fixed(50.),Sizing::Fixed(50.)),Track::new(Sizing::Auto,Sizing::Auto)],names:vec![vec![],vec![],vec![],vec![]],inherited:None};
        let Some(AxisInheritance::Tracks(initial))=fixed_section_aligned(&list,0,2,0.,true,Some(300.),ContentDistribution::SpaceBetween) else {panic!("missing fixed section")};
        assert_eq!(initial.starts,[0.,50.]);assert_eq!(initial.extent(),100.);
    }

    #[test]
    fn subtree_invalidations_are_charged_before_temporary_styles_are_installed() {
        let mut b=child();let original=b.style.clone();
        b.children.push(LayoutBox::new(BoxKind::Block,None,original.clone()));
        b.children.push(LayoutBox::new(BoxKind::Block,None,original.clone()));
        let mut budget=WorkBudget{remaining:8,depth:None};
        let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||budget.subtree(&b)));
        assert!(result.is_err());assert_eq!(budget.remaining,5);assert!(Rc::ptr_eq(&original,&b.style));assert!(b.grid_columns.is_none());
    }

    #[test]
    fn track_copy_work_is_reserved_before_cloning_placement_lists() {
        let list=parent();let mut budget=WorkBudget{remaining:15,depth:None};
        let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||budget.tracks(&list)));
        assert!(result.is_err());assert_eq!(budget.remaining,15);
    }
}
