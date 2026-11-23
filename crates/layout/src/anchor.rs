//! Resolve named position-area containing blocks from the laid-out anchor
//! border boxes. The ordinary absolute-position algorithm sizes their targets.
use crate::{LayoutContext, tree::LayoutTree};
use rabbithole_dom::{Document, NodeId};
use rabbithole_gfx::{Rect, Matrix};
use rabbithole_style::{ComputedStyle, Position, props::anchor::Region};
use std::collections::HashMap;
use rabbithole_style::{Length, WritingMode, Direction, math_node, props::anchor_length::Dimension};

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

/// Follow the containing-block chain, including in-flow block containers
/// along which anchor positions propagate to an outside query.
fn containing_node(ctx: &LayoutContext, node: NodeId, style: &ComputedStyle) -> Option<NodeId> {
    if ctx.doc.top_layer.contains(&node) { return None; }
    let mut parent = ctx.doc.flat_parent(node);
    while let Some(id) = parent {
        if let Some(s) = ctx.styles.get(id) {
            use rabbithole_style::Display;
            let establishes = match style.position {
                Position::Absolute => s.position != Position::Static || s.establishes_fixed_containing_block(),
                Position::Fixed => s.establishes_fixed_containing_block(),
                _ => s.display.is_block_level() || s.display.is_atomic_inline() || s.display == Display::TableCell,
            };
            if establishes && !matches!(s.display,Display::None | Display::Contents) { return Some(id); }
        }
        parent = ctx.doc.flat_parent(id);
    }
    None
}

fn container(ctx: &LayoutContext, tree: &LayoutTree, parent: Option<NodeId>) -> (Rect,Writing) {
    if let Some((s, border)) = parent.and_then(|id| Some((ctx.styles.get(id)?,tree.box_in_document(ctx.doc,id)?.1))) {
        let bounds = Rect::new(border.x+s.border_width.left,border.y+s.border_width.top,
            (border.w-s.border_width.left-s.border_width.right).max(0.),
            (border.h-s.border_width.top-s.border_width.bottom).max(0.));
        return (bounds,(s.writing_mode,s.direction));
    }
    let root = ctx.doc.child_elements(Document::ROOT).next().and_then(|id| ctx.styles.get(id));
    (Rect::new(0.,0.,ctx.viewport_width,ctx.viewport_height),root.map_or(
        (WritingMode::HorizontalTb,Direction::Ltr), |s| (s.writing_mode,s.direction)))
}

fn acceptable(ctx: &LayoutContext, mut anchor: NodeId, cb: Option<NodeId>, order: usize,
              orders: &HashMap<NodeId,usize>) -> bool {
    loop {
        let Some(style) = ctx.styles.get(anchor) else { return false; };
        let parent = containing_node(ctx,anchor,style);
        if parent == cb {
            return !matches!(style.position,Position::Absolute | Position::Fixed) ||
                orders.get(&anchor).is_some_and(|i| *i < order);
        }
        let Some(next) = parent else { return false; };
        anchor = next;
    }
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

#[derive(Default)]
pub(crate) struct AnchoredLayout {
    pub areas: HashMap<NodeId, Placement>,
    pub fallback_targets: Vec<NodeId>,
    pub lengths: HashMap<NodeId, std::rc::Rc<ComputedStyle>>,
}

impl AnchoredLayout {
    pub fn is_empty(&self) -> bool { self.areas.is_empty() && self.lengths.is_empty() }

    /// Apply physical values before the writing-mode layout converts them
    /// into the local logical coordinate system.
    pub fn apply_lengths(&self, node: &mut crate::tree::LayoutBox) {
        if self.lengths.is_empty() { return; }
        if let Some(style) = node.node.and_then(|id| self.lengths.get(&id)) {
            if node.style.has_anchor_lengths() || node.style.position_try_fallbacks.is_some() { node.style = style.clone(); }
        }
        for child in &mut node.children { self.apply_lengths(child); }
    }
}

type Writing = (WritingMode,Direction);

fn reversed(horizontal: bool, (mode,dir): Writing) -> bool {
    if horizontal == mode.is_vertical() {
        mode == WritingMode::VerticalRl
    } else {
        (dir == Direction::Rtl) != (mode == WritingMode::SidewaysLr)
    }
}

/// Physical destination of the four source sides (left, right, top, bottom).
/// Composing this permutation also handles flip-start in RTL and vertical text.
#[derive(Clone, Copy)]
struct AxisMap([usize;4]);

impl AxisMap {
    fn new(flips: &[rabbithole_style::props::anchor::Flip], writing: Writing) -> Self {
        use rabbithole_style::props::anchor::Flip;
        let mut sides = [0,1,2,3];
        for flip in flips {
            for side in &mut sides {
                let horizontal = *side < 2;
                if *flip == Flip::Start {
                    *side ^= 2;
                    if reversed(true,writing) != reversed(false,writing) { *side ^= 1; }
                } else {
                    let block = horizontal == writing.0.is_vertical();
                    let reflected = match flip { Flip::X => horizontal, Flip::Y => !horizontal,
                        Flip::Block => block, Flip::Inline => !block, Flip::Start => unreachable!() };
                    if reflected { *side ^= 1; }
                }
            }
        }
        Self(sides)
    }

    fn side(self, horizontal: bool, end: bool) -> (bool,bool) {
        let side = self.0[2*usize::from(!horizontal) + usize::from(end)];
        (side < 2,side%2 == 1)
    }

    fn edges(self, edges: &mut rabbithole_style::Edges<Length>) {
        let mut v = [Length::Auto;4];
        for (to, value) in self.0.into_iter().zip([edges.left,edges.right,edges.top,edges.bottom]) { v[to] = value; }
        [edges.left,edges.right,edges.top,edges.bottom] = v;
    }

    fn regions(self, source: [Region;2]) -> [Region;2] {
        let mut regions = [Region::All;2];
        for (i,region) in source.into_iter().enumerate() {
            let (horizontal,flip) = self.side(i==0,false);
            regions[usize::from(!horizontal)] = if flip { match region {
                Region::Before => Region::After, Region::After => Region::Before,
                Region::SpanBefore => Region::SpanAfter, Region::SpanAfter => Region::SpanBefore,
                r => r,
            } } else { region };
        }
        regions
    }
}

struct QueryBox {
    axes: AxisMap,
    cb: Rect,
    writing: Writing,
    own: Writing,
}

impl QueryBox {
    fn pixels(&self, dim: &Dimension, anchor: Rect, horizontal: bool, end: bool) -> Option<f32> {
        let (destination, far_edge) = self.axes.side(horizontal,end);
        let axis = |rect: Rect| if destination { (rect.x,rect.w) } else { (rect.y,rect.h) };
        let (start,len) = axis(anchor);
        let ratio = match *dim {
            Dimension::Size(name) => {
                let width = match name {
                    Some("width") => true,
                    Some("height") => false,
                    Some("block") => self.writing.0.is_vertical(),
                    Some("inline") => !self.writing.0.is_vertical(),
                    Some("self-block") => self.own.0.is_vertical(),
                    Some("self-inline") => !self.own.0.is_vertical(),
                    _ => horizontal,
                };
                return Some(if self.axes.side(width,false).0 {anchor.w} else {anchor.h});
            }
            Dimension::Percentage(p) => if reversed(horizontal,self.writing) { 1.-p/100. } else { p/100. },
            Dimension::Side("center") => 0.5,
            Dimension::Side(side) => {
                let far = match side {
                    "left" | "right" if horizontal => side == "right",
                    "top" | "bottom" if !horizontal => side == "bottom",
                    "inside" => end,
                    "outside" => !end,
                    "start" | "end" => reversed(horizontal,self.writing) != (side == "end"),
                    "self-start" | "self-end" => reversed(horizontal,self.own) != (side == "self-end"),
                    _ => return None,
                };
                if far { 1. } else { 0. }
            }
        };
        let (origin,extent) = axis(self.cb);
        let ratio = if end != far_edge { 1.-ratio } else { ratio };
        let offset = start + len*ratio - origin;
        Some(if far_edge { extent-offset } else { offset })
    }

    fn length(&self, length: Length, horizontal: bool, end: bool, base: f32,
              lookup: &impl Fn(Option<&str>) -> Option<Rect>) -> Length {
        if !length.is_anchor_query() { return length; }
        let Length::Math(id) = length else { return length };
        let Some(node) = math_node(id) else { return Length::Auto; };
        node.eval_anchors(base, &|q| {
            lookup(q.name.as_deref()).and_then(|a| self.pixels(&q.dimension,a,horizontal,end))
                .or_else(|| self.length(q.fallback?,horizontal,end,base,lookup).resolve(base))
        }).map_or(Length::Auto,Length::Px)
    }

    fn resolve(&self, style: &ComputedStyle, lookup: &impl Fn(Option<&str>) -> Option<Rect>) -> ComputedStyle {
        let mut s = style.clone();
        for (value,horizontal,end,margin) in [
            (&mut s.inset.left,true,false,false),(&mut s.inset.right,true,true,false),
            (&mut s.inset.top,false,false,false),(&mut s.inset.bottom,false,true,false),
            (&mut s.margin.left,true,false,true),(&mut s.margin.right,true,true,true),
            (&mut s.margin.top,false,false,true),(&mut s.margin.bottom,false,true,true),
            (&mut s.width,true,false,false),(&mut s.min_width,true,false,false),(&mut s.max_width,true,false,false),
            (&mut s.height,false,false,false),(&mut s.min_height,false,false,false),(&mut s.max_height,false,false,false),
        ] {
            let width_base = if margin { !self.writing.0.is_vertical() } else { self.axes.side(horizontal,false).0 };
            *value = self.length(*value,horizontal,end,if width_base {self.cb.w} else {self.cb.h},lookup);
        }
        self.axes.edges(&mut s.inset);
        self.axes.edges(&mut s.margin);
        if !self.axes.side(true,false).0 {
            std::mem::swap(&mut s.width,&mut s.height);
            std::mem::swap(&mut s.min_width,&mut s.min_height);
            std::mem::swap(&mut s.max_width,&mut s.max_height);
            std::mem::swap(&mut s.align_self,&mut s.justify_self);
            std::mem::swap(&mut s.align_self_flags,&mut s.justify_self_flags);
        }
        s
    }
}

/// Try the next declared alternative only for boxes that overflow in the
/// actual layout. Once the list is exhausted, restore the original style.
pub(crate) fn advance_fallbacks(ctx: &LayoutContext, tree: &LayoutTree, pending: &mut Vec<NodeId>,
                                choices: &mut HashMap<NodeId, usize>) -> bool {
    let mut changed = false;
    pending.retain(|&id| {
        let Some(style) = ctx.styles.get(id) else { return false; };
        let Some(list) = style.position_try_fallbacks.as_deref() else { return false; };
        let Some((b, matrix)) = tree.box_and_coordinates(ctx.doc,id) else { return false; };
        let rect = b.margin_rect();
        let rect = matrix.map_or(rect, |m| rabbithole_gfx::path::transform_rect_bounds(&m,rect));
        let (cb,_) = container(ctx,tree,containing_node(ctx,id,style));
        if rect.x >= cb.x-0.01 && rect.y >= cb.y-0.01 && rect.right() <= cb.right()+0.01 && rect.bottom() <= cb.bottom()+0.01 {
            return false;
        }
        let start = choices.get(&id).map_or(0, |index| index+1);
        let scoped = ctx.styles.position_tries.get(&ctx.doc.tree_root(id));
        for (i, option) in list.iter().enumerate().skip(start) {
            if let rabbithole_style::props::anchor::TryOption::Rule { name: Some(name), .. } = option {
                if !scoped.is_some_and(|rules| rules.contains_key(name)) { continue; }
            }
            choices.insert(id,i);
            changed = true;
            return true;
        }
        changed |= choices.remove(&id).is_some();
        false
    });
    changed
}

pub(crate) fn placements(ctx: &LayoutContext, tree: &LayoutTree, choices: &HashMap<NodeId,usize>) -> AnchoredLayout {
    let mut named: HashMap<(NodeId,Option<NodeId>,&str),Vec<NodeId>> = HashMap::new();
    let mut targets = Vec::new();
    let mut orders = HashMap::new();
    for (order,id) in ctx.doc.shadow_including_descendants(Document::ROOT).into_iter().enumerate() {
        let Some(s) = ctx.styles.get(id) else { continue };
        if matches!(s.position, Position::Absolute | Position::Fixed) { orders.insert(id,order); }
        if let Some(names) = &s.anchor_names {
            let scope = ctx.doc.tree_root(id);
            for name in names.iter() { named.entry((scope,name_scope(ctx,id,name),name.as_str())).or_default().push(id); }
        }
        if s.needs_anchor_layout() {
            targets.push((order,id,s));
        }
    }
    let mut result = AnchoredLayout::default();
    for (order,id,base) in targets {
        let option = base.position_try_fallbacks.as_ref().and_then(|v| v.get(*choices.get(&id)?));
        if base.position_try_fallbacks.is_some() { result.fallback_targets.push(id); }
        let trial = option.and_then(|option| {
            use rabbithole_style::props::anchor::TryOption;
            match option {
                TryOption::Area(area) => {
                    let mut style = (**base).clone();
                    style.position_area = *area;
                    Some(style)
                }
                TryOption::Rule { name: None, .. } => Some((**base).clone()),
                TryOption::Rule { name: Some(name), .. } => {
                    let rules = ctx.styles.position_tries.get(&ctx.doc.tree_root(id))?;
                    let parent = ctx.doc.flat_parent(id).and_then(|n| ctx.styles.get(n)).unwrap_or(base);
                    Some(rabbithole_style::cascade::apply_over(base,parent,rules.get(name)?,
                        ctx.styles.root_font_size,(ctx.viewport_width,ctx.viewport_height)))
                }
            }
        });
        let s = trial.as_ref().unwrap_or(base);
        let cb_node = containing_node(ctx,id,s);
        let bounds = |a| {
            if ctx.doc.is_shadow_including_inclusive_ancestor(id,a) { return None; }
            tree.box_in_document(ctx.doc,a).map(|(_,r)| r).or_else(|| tree.inline_rect(ctx.doc,a))
        };
        let lookup = |name: Option<&str>| if let Some(name) = name.or(s.position_anchor.as_deref()) {
            named.get(&(ctx.doc.tree_root(id), name_scope(ctx,id,name), name)).and_then(|candidates| {
                candidates.iter().rev().find_map(|&a| {
                    acceptable(ctx,a,cb_node,order,&orders).then(|| bounds(a)).flatten()
                })
            })
        } else {
            ctx.doc.popover_invoker(id).and_then(bounds)
        };
        let (cb,writing) = container(ctx,tree,cb_node);
        let flips = match option { Some(rabbithole_style::props::anchor::TryOption::Rule { flips, .. }) => flips.as_slice(), _ => &[] };
        let axes = AxisMap::new(flips,writing);
        if trial.is_some() || s.has_anchor_lengths() {
            let geometry = QueryBox { axes, cb, writing, own:(s.writing_mode,s.direction) };
            result.lengths.insert(id,std::rc::Rc::new(geometry.resolve(s,&lookup)));
        }
        if s.position_area.is_none() { continue; }
        let Some(anchor) = lookup(None) else { continue };
        let Some((_,coordinates)) = tree.box_and_coordinates(ctx.doc,id) else { continue };
        let Some(regions) = s.position_area.physical(writing,(s.writing_mode,s.direction)) else { continue };
        let [x,y] = axes.regions(regions);
        let (left,right,ax) = interval(x,cb.x,cb.right(),anchor.x,anchor.right());
        let (top,bottom,ay) = interval(y,cb.y,cb.bottom(),anchor.y,anchor.bottom());
        result.areas.insert(id,Placement { area: Rect::new(left,top,right-left,bottom-top),
            center:(anchor.x+anchor.w/2.,anchor.y+anchor.h/2.),align:[ax,ay] }.in_box_space(coordinates));
    }
    result
}
