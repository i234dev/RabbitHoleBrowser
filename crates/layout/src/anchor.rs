//! Resolve named position-area containing blocks from the laid-out anchor
//! border boxes. The ordinary absolute-position algorithm sizes their targets.
use crate::{LayoutContext, tree::LayoutTree};
use rabbithole_dom::{Document, NodeId};
use rabbithole_gfx::{Rect, Matrix};
use rabbithole_style::{ComputedStyle, Position, props::anchor::Region};
use std::collections::HashMap;

#[derive(Clone,Copy)]
pub(crate) enum Alignment { Start, End, Center, Anchor }

impl Alignment {
    fn flip(self) -> Self {
        match self { Self::Start => Self::End, Self::End => Self::Start, value => value }
    }

    pub fn offset(self, start: f32, end: f32, center: f32, size: f32) -> f32 {
        match self {
            Self::Start => start,
            Self::End => end - size,
            Self::Center => (start + end - size) * 0.5,
            Self::Anchor => (center - size * 0.5).clamp(start,(end-size).max(start)),
        }
    }
}

#[derive(Clone,Copy)]
pub(crate) struct Placement {
    pub area: Rect,
    pub center: (f32,f32),
    pub align: [Alignment;2],
}

fn interval(region: Region, start: f32, end: f32, a: f32, b: f32) -> (f32,f32,Alignment) {
    match region {
        Region::Before => (start,a.max(start),Alignment::End),
        Region::After => (b,end.max(b),Alignment::Start),
        Region::SpanBefore => (start,b.max(start),Alignment::End),
        Region::SpanAfter => (a,end.max(a),Alignment::Start),
        Region::Center => (a,b,Alignment::Center),
        Region::All => (start,end,Alignment::Anchor),
    }
}

impl Placement {
    fn in_box_space(self, matrix: Option<Matrix>) -> Self {
        let Some(inverse) = matrix.and_then(crate::tree::invert_matrix) else { return self; };
        let axis = |x: f32,y: f32| {
            let (align,direction) = if x.abs() > y.abs() { (self.align[0],x) } else { (self.align[1],y) };
            if direction < 0. { align.flip() } else { align }
        };
        Self {
            area: rabbithole_gfx::path::transform_rect_bounds(&inverse,self.area),
            center: crate::tree::map_point(inverse,self.center.0,self.center.1),
            align: [axis(inverse[0],inverse[2]),axis(inverse[1],inverse[3])],
        }
    }
}

fn container(ctx: &LayoutContext, tree: &LayoutTree, node: NodeId, style: &ComputedStyle) -> (Rect,(rabbithole_style::WritingMode,rabbithole_style::Direction)) {
    let mut parent = ctx.doc.flat_parent(node);
    while let Some(id) = parent {
        if let Some(s) = ctx.styles.get(id) {
            let positioned = style.position == Position::Absolute && s.position != Position::Static;
            if (positioned || s.establishes_fixed_containing_block()) && !ctx.doc.top_layer.contains(&node) {
                if let Some((_,border)) = tree.box_in_document(ctx.doc,id) {
                    let bounds = Rect::new(border.x+s.border_width.left,border.y+s.border_width.top,
                        (border.w-s.border_width.left-s.border_width.right).max(0.),
                        (border.h-s.border_width.top-s.border_width.bottom).max(0.));
                    return (bounds,(s.writing_mode,s.direction));
                }
            }
        }
        parent = ctx.doc.flat_parent(id);
    }
    let root = ctx.doc.child_elements(Document::ROOT).next().and_then(|id| ctx.styles.get(id));
    (Rect::new(0.,0.,ctx.viewport_width,ctx.viewport_height),root.map_or(
        (rabbithole_style::WritingMode::HorizontalTb,rabbithole_style::Direction::Ltr), |s| (s.writing_mode,s.direction)))
}

fn name_scope(ctx: &LayoutContext, mut node: NodeId, name: &str) -> Option<NodeId> {
    let root = ctx.doc.tree_root(node);
    loop {
        if ctx.styles.get(node).is_some_and(|style| style.anchor_scope.includes(name))
            && ctx.doc.tree_root(node) == root {
            return Some(node);
        }
        node = ctx.doc.flat_parent(node)?;
    }
}

pub(crate) fn placements(ctx: &LayoutContext, tree: &LayoutTree) -> HashMap<NodeId,Placement> {
    let mut named: HashMap<(NodeId,Option<NodeId>,&str),Vec<(usize,NodeId)>> = HashMap::new();
    let mut targets = Vec::new();
    for (order,id) in ctx.doc.shadow_including_descendants(Document::ROOT).into_iter().enumerate() {
        let Some(s) = ctx.styles.get(id) else { continue };
        if let Some(names) = &s.anchor_names {
            let scope = ctx.doc.tree_root(id);
            for name in names.iter() { named.entry((scope,name_scope(ctx,id,name),name.as_str())).or_default().push((order,id)); }
        }
        if matches!(s.position,Position::Fixed | Position::Absolute) && !s.position_area.is_none() {
            targets.push((order,id,s));
        }
    }
    let mut result = HashMap::new();
    for (order,id,s) in targets {
        let bounds = |a| {
            if ctx.doc.is_shadow_including_inclusive_ancestor(id,a) { return None; }
            tree.box_in_document(ctx.doc,a).map(|(_,r)| r).or_else(|| tree.inline_rect(ctx.doc,a))
        };
        let anchor = if let Some(name) = s.position_anchor.as_deref() {
            named.get(&(ctx.doc.tree_root(id), name_scope(ctx,id,name), name)).and_then(|candidates| {
                candidates.iter().rev().find_map(|&(other_order,a)| {
                    let anchor_style = ctx.styles.get(a)?;
                    if other_order >= order && matches!(anchor_style.position, Position::Absolute | Position::Fixed) {
                        return None;
                    }
                    bounds(a)
                })
            })
        } else {
            ctx.doc.popover_invoker(id).and_then(bounds)
        };
        let Some(anchor) = anchor else { continue };
        let Some((_,coordinates)) = tree.box_and_coordinates(ctx.doc,id) else { continue };
        let (cb,writing) = container(ctx,tree,id,s);
        let Some([x,y]) = s.position_area.physical(writing,(s.writing_mode,s.direction)) else { continue };
        let (left,right,ax) = interval(x,cb.x,cb.right(),anchor.x,anchor.right());
        let (top,bottom,ay) = interval(y,cb.y,cb.bottom(),anchor.y,anchor.bottom());
        result.insert(id,Placement { area: Rect::new(left,top,right-left,bottom-top),
            center:(anchor.x+anchor.w/2.,anchor.y+anchor.h/2.),align:[ax,ay] }.in_box_space(coordinates));
    }
    result
}
