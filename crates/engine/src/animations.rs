//! CSS Transitions and CSS Animations.
//!
//! The style system computes the *base* style of every element. When a
//! restyle changes an animatable property of an element whose
//! `transition-*` covers it, a transition starts from the value shown
//! before the change; `animation-name` starting to match an `@keyframes`
//! rule starts an animation. While effects run, the element's entry in
//! `page.styles` is the base style with the animated values applied (an
//! *overlay*), refreshed by [`Engine::tick`]; the base style is kept here
//! so a later restyle can tell real changes from the overlay.
//! `transitionrun/start/end/cancel` and `animationstart/iteration/end/
//! cancel` events are queued and dispatched from the tick.

use std::collections::HashMap;
use std::rc::Rc;

use rabbithole_dom::NodeId;
use rabbithole_gfx::Color;
use rabbithole_script::{EventDetail, EventInit};
use rabbithole_style::{ComputedStyle, Length, TransformOp, Visibility};

use crate::scripting::ScriptOutcome;
use crate::{Engine, Page};

/// Animatable properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Prop {
    Opacity,
    Color,
    BackgroundColor,
    BorderTopColor,
    BorderRightColor,
    BorderBottomColor,
    BorderLeftColor,
    Width,
    Height,
    MinWidth,
    MaxWidth,
    MinHeight,
    MaxHeight,
    Top,
    Right,
    Bottom,
    Left,
    MarginTop,
    MarginRight,
    MarginBottom,
    MarginLeft,
    PaddingTop,
    PaddingRight,
    PaddingBottom,
    PaddingLeft,
    Transform,
    Visibility,
    LetterSpacing,
    FontSize,
    BorderRadius,
}

const ALL: &[Prop] = &[
    Prop::Opacity,
    Prop::Color,
    Prop::BackgroundColor,
    Prop::BorderTopColor,
    Prop::BorderRightColor,
    Prop::BorderBottomColor,
    Prop::BorderLeftColor,
    Prop::Width,
    Prop::Height,
    Prop::MinWidth,
    Prop::MaxWidth,
    Prop::MinHeight,
    Prop::MaxHeight,
    Prop::Top,
    Prop::Right,
    Prop::Bottom,
    Prop::Left,
    Prop::MarginTop,
    Prop::MarginRight,
    Prop::MarginBottom,
    Prop::MarginLeft,
    Prop::PaddingTop,
    Prop::PaddingRight,
    Prop::PaddingBottom,
    Prop::PaddingLeft,
    Prop::Transform,
    Prop::Visibility,
    Prop::LetterSpacing,
    Prop::FontSize,
    Prop::BorderRadius,
];

impl Prop {
    fn name(self) -> &'static str {
        match self {
            Prop::Opacity => "opacity",
            Prop::Color => "color",
            Prop::BackgroundColor => "background-color",
            Prop::BorderTopColor => "border-top-color",
            Prop::BorderRightColor => "border-right-color",
            Prop::BorderBottomColor => "border-bottom-color",
            Prop::BorderLeftColor => "border-left-color",
            Prop::Width => "width",
            Prop::Height => "height",
            Prop::MinWidth => "min-width",
            Prop::MaxWidth => "max-width",
            Prop::MinHeight => "min-height",
            Prop::MaxHeight => "max-height",
            Prop::Top => "top",
            Prop::Right => "right",
            Prop::Bottom => "bottom",
            Prop::Left => "left",
            Prop::MarginTop => "margin-top",
            Prop::MarginRight => "margin-right",
            Prop::MarginBottom => "margin-bottom",
            Prop::MarginLeft => "margin-left",
            Prop::PaddingTop => "padding-top",
            Prop::PaddingRight => "padding-right",
            Prop::PaddingBottom => "padding-bottom",
            Prop::PaddingLeft => "padding-left",
            Prop::Transform => "transform",
            Prop::Visibility => "visibility",
            Prop::LetterSpacing => "letter-spacing",
            Prop::FontSize => "font-size",
            Prop::BorderRadius => "border-radius",
        }
    }

    /// Does the `transition-property` value `name` cover this property?
    fn covered_by(self, name: &str) -> bool {
        let name = name.trim();
        if name.eq_ignore_ascii_case("all") || name.eq_ignore_ascii_case(self.name()) {
            return true;
        }
        let n = name.to_ascii_lowercase();
        let n = n.strip_prefix("-webkit-").unwrap_or(&n);
        if n == self.name() {
            return true;
        }
        use Prop::*;
        match n {
            "margin" => matches!(self, MarginTop | MarginRight | MarginBottom | MarginLeft),
            "padding" => matches!(self, PaddingTop | PaddingRight | PaddingBottom | PaddingLeft),
            "border-color" | "border" => matches!(self, BorderTopColor | BorderRightColor | BorderBottomColor | BorderLeftColor),
            "border-top" => self == BorderTopColor,
            "border-right" => self == BorderRightColor,
            "border-bottom" => self == BorderBottomColor,
            "border-left" => self == BorderLeftColor,
            "inset" => matches!(self, Top | Right | Bottom | Left),
            "background" => self == BackgroundColor,
            "font" => self == FontSize,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Val {
    F(f32),
    C(Color),
    L(Length),
    T(Vec<TransformOp>),
    V(Visibility),
}

fn get(s: &ComputedStyle, p: Prop) -> Val {
    use Prop::*;
    match p {
        Opacity => Val::F(s.opacity),
        Color => Val::C(s.color),
        BackgroundColor => Val::C(s.background_color),
        BorderTopColor => Val::C(s.border_color.top),
        BorderRightColor => Val::C(s.border_color.right),
        BorderBottomColor => Val::C(s.border_color.bottom),
        BorderLeftColor => Val::C(s.border_color.left),
        Width => Val::L(s.width),
        Height => Val::L(s.height),
        MinWidth => Val::L(s.min_width),
        MaxWidth => Val::L(s.max_width),
        MinHeight => Val::L(s.min_height),
        MaxHeight => Val::L(s.max_height),
        Top => Val::L(s.inset.top),
        Right => Val::L(s.inset.right),
        Bottom => Val::L(s.inset.bottom),
        Left => Val::L(s.inset.left),
        MarginTop => Val::L(s.margin.top),
        MarginRight => Val::L(s.margin.right),
        MarginBottom => Val::L(s.margin.bottom),
        MarginLeft => Val::L(s.margin.left),
        PaddingTop => Val::L(s.padding.top),
        PaddingRight => Val::L(s.padding.right),
        PaddingBottom => Val::L(s.padding.bottom),
        PaddingLeft => Val::L(s.padding.left),
        Transform => Val::T(s.transform.clone()),
        Visibility => Val::V(s.visibility),
        LetterSpacing => Val::F(s.letter_spacing),
        FontSize => Val::F(s.font_size),
        BorderRadius => Val::F(s.border_radius),
    }
}

fn set(s: &mut ComputedStyle, p: Prop, v: &Val) {
    use Prop::*;
    match (p, v) {
        (Opacity, Val::F(f)) => s.opacity = f.clamp(0.0, 1.0),
        (Color, Val::C(c)) => s.color = *c,
        (BackgroundColor, Val::C(c)) => s.background_color = *c,
        (BorderTopColor, Val::C(c)) => s.border_color.top = *c,
        (BorderRightColor, Val::C(c)) => s.border_color.right = *c,
        (BorderBottomColor, Val::C(c)) => s.border_color.bottom = *c,
        (BorderLeftColor, Val::C(c)) => s.border_color.left = *c,
        (Width, Val::L(l)) => s.width = *l,
        (Height, Val::L(l)) => s.height = *l,
        (MinWidth, Val::L(l)) => s.min_width = *l,
        (MaxWidth, Val::L(l)) => s.max_width = *l,
        (MinHeight, Val::L(l)) => s.min_height = *l,
        (MaxHeight, Val::L(l)) => s.max_height = *l,
        (Top, Val::L(l)) => s.inset.top = *l,
        (Right, Val::L(l)) => s.inset.right = *l,
        (Bottom, Val::L(l)) => s.inset.bottom = *l,
        (Left, Val::L(l)) => s.inset.left = *l,
        (MarginTop, Val::L(l)) => s.margin.top = *l,
        (MarginRight, Val::L(l)) => s.margin.right = *l,
        (MarginBottom, Val::L(l)) => s.margin.bottom = *l,
        (MarginLeft, Val::L(l)) => s.margin.left = *l,
        (PaddingTop, Val::L(l)) => s.padding.top = *l,
        (PaddingRight, Val::L(l)) => s.padding.right = *l,
        (PaddingBottom, Val::L(l)) => s.padding.bottom = *l,
        (PaddingLeft, Val::L(l)) => s.padding.left = *l,
        (Transform, Val::T(t)) => s.transform = t.clone(),
        (Visibility, Val::V(v)) => s.visibility = *v,
        (LetterSpacing, Val::F(f)) => s.letter_spacing = *f,
        (FontSize, Val::F(f)) => s.font_size = f.max(0.0),
        (BorderRadius, Val::F(f)) => s.border_radius = f.max(0.0),
        _ => {}
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_len(a: &Length, b: &Length, t: f32) -> Option<Length> {
    match (a, b) {
        (Length::Px(x), Length::Px(y)) => Some(Length::Px(lerp(*x, *y, t))),
        (Length::Percent(x), Length::Percent(y)) => Some(Length::Percent(lerp(*x, *y, t))),
        _ => None,
    }
}

/// Identity transform function of the same kind as `op`.
fn identity_of(op: &TransformOp) -> TransformOp {
    match op {
        TransformOp::Translate(..) => TransformOp::Translate(Length::Px(0.0), Length::Px(0.0)),
        TransformOp::Scale(..) => TransformOp::Scale(1.0, 1.0),
        TransformOp::Rotate(_) => TransformOp::Rotate(0.0),
        TransformOp::Skew(..) => TransformOp::Skew(0.0, 0.0),
        TransformOp::Matrix(_) => TransformOp::Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
    }
}

fn lerp_op(a: &TransformOp, b: &TransformOp, t: f32) -> Option<TransformOp> {
    use TransformOp::*;
    Some(match (a, b) {
        (Translate(ax, ay), Translate(bx, by)) => Translate(lerp_len(ax, bx, t)?, lerp_len(ay, by, t)?),
        (Scale(ax, ay), Scale(bx, by)) => Scale(lerp(*ax, *bx, t), lerp(*ay, *by, t)),
        (Rotate(x), Rotate(y)) => Rotate(lerp(*x, *y, t)),
        (Skew(ax, ay), Skew(bx, by)) => Skew(lerp(*ax, *bx, t), lerp(*ay, *by, t)),
        (Matrix(m), Matrix(n)) => {
            let mut o = [0.0; 6];
            for i in 0..6 {
                o[i] = lerp(m[i], n[i], t);
            }
            Matrix(o)
        }
        _ => return None,
    })
}

fn interpolate(a: &Val, b: &Val, t: f32) -> Val {
    let discrete = || if t < 0.5 { a.clone() } else { b.clone() };
    match (a, b) {
        (Val::F(x), Val::F(y)) => Val::F(lerp(*x, *y, t)),
        (Val::C(x), Val::C(y)) => {
            let ch = |p: u8, q: u8| lerp(p as f32, q as f32, t).round().clamp(0.0, 255.0) as u8;
            Val::C(Color { r: ch(x.r, y.r), g: ch(x.g, y.g), b: ch(x.b, y.b), a: ch(x.a, y.a) })
        }
        (Val::L(x), Val::L(y)) => lerp_len(x, y, t).map(Val::L).unwrap_or_else(discrete),
        (Val::T(x), Val::T(y)) => {
            let (x, y): (Vec<TransformOp>, Vec<TransformOp>) = if x.is_empty() {
                (y.iter().map(identity_of).collect(), y.clone())
            } else if y.is_empty() {
                (x.clone(), x.iter().map(identity_of).collect())
            } else {
                (x.clone(), y.clone())
            };
            if x.len() == y.len() {
                let ops: Option<Vec<TransformOp>> = x.iter().zip(y.iter()).map(|(p, q)| lerp_op(p, q, t)).collect();
                if let Some(ops) = ops {
                    return Val::T(ops);
                }
            }
            discrete()
        }
        (Val::V(x), Val::V(y)) => {
            if t <= 0.0 {
                Val::V(*x)
            } else if t >= 1.0 {
                Val::V(*y)
            } else if *x == Visibility::Visible || *y == Visibility::Visible {
                Val::V(Visibility::Visible)
            } else {
                Val::V(*y)
            }
        }
        _ => discrete(),
    }
}

// ---------------------------------------------------------------- timing

#[derive(Debug, Clone, PartialEq)]
enum Easing {
    Linear,
    Bezier(f32, f32, f32, f32),
    Steps(u32, bool),
}

fn parse_easing(s: &str) -> Easing {
    let s = s.trim().to_ascii_lowercase();
    match s.as_str() {
        "linear" => return Easing::Linear,
        "ease" => return Easing::Bezier(0.25, 0.1, 0.25, 1.0),
        "ease-in" => return Easing::Bezier(0.42, 0.0, 1.0, 1.0),
        "ease-out" => return Easing::Bezier(0.0, 0.0, 0.58, 1.0),
        "ease-in-out" => return Easing::Bezier(0.42, 0.0, 0.58, 1.0),
        "step-start" => return Easing::Steps(1, true),
        "step-end" => return Easing::Steps(1, false),
        _ => {}
    }
    let args = |prefix: &str| -> Option<Vec<String>> {
        let inner = s.strip_prefix(prefix)?.strip_prefix('(')?.strip_suffix(')')?;
        Some(inner.split(',').map(|a| a.trim().to_string()).collect())
    };
    if let Some(a) = args("cubic-bezier") {
        let n: Vec<f32> = a.iter().filter_map(|x| x.parse().ok()).collect();
        if n.len() == 4 {
            return Easing::Bezier(n[0].clamp(0.0, 1.0), n[1], n[2].clamp(0.0, 1.0), n[3]);
        }
    }
    if let Some(a) = args("steps") {
        let n = a.first().and_then(|x| x.parse::<u32>().ok()).unwrap_or(1).max(1);
        let start = a.get(1).map(|p| p == "start" || p == "jump-start").unwrap_or(false);
        return Easing::Steps(n, start);
    }
    Easing::Bezier(0.25, 0.1, 0.25, 1.0)
}

fn bezier(p1: f32, p2: f32, t: f32) -> f32 {
    // B(t) = 3(1-t)^2 t p1 + 3(1-t) t^2 p2 + t^3
    let u = 1.0 - t;
    3.0 * u * u * t * p1 + 3.0 * u * t * t * p2 + t * t * t
}

fn ease(e: &Easing, x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    match *e {
        Easing::Linear => x,
        Easing::Steps(n, start) => {
            let n = n as f32;
            let s = if start { (x * n).floor() + 1.0 } else { (x * n).floor() };
            (s / n).clamp(0.0, 1.0)
        }
        Easing::Bezier(x1, y1, x2, y2) => {
            if x <= 0.0 || x >= 1.0 {
                return x;
            }
            // Solve bezier_x(t) = x by bisection (monotonic in t).
            let (mut lo, mut hi) = (0.0f32, 1.0f32);
            let mut t = x;
            for _ in 0..30 {
                let bx = bezier(x1, x2, t);
                if (bx - x).abs() < 1e-5 {
                    break;
                }
                if bx < x {
                    lo = t;
                } else {
                    hi = t;
                }
                t = (lo + hi) / 2.0;
            }
            bezier(y1, y2, t)
        }
    }
}

// ---------------------------------------------------------------- state

#[derive(Debug, Clone)]
struct Transition {
    node: NodeId,
    prop: Prop,
    from: Val,
    to: Val,
    /// Engine time (ms) the transition was created.
    start: f64,
    delay: f64,
    duration: f64,
    easing: Easing,
    started: bool,
}

impl Transition {
    fn value_at(&self, now: f64) -> Val {
        let t = ((now - self.start - self.delay) / self.duration.max(1e-9)).clamp(0.0, 1.0) as f32;
        interpolate(&self.from, &self.to, ease(&self.easing, t))
    }
}

#[derive(Debug, Clone)]
struct Keyframe {
    offset: f32,
    values: Vec<(Prop, Val)>,
}

#[derive(Debug, Clone)]
struct Animation {
    node: NodeId,
    name: String,
    frames: Vec<Keyframe>,
    start: f64,
    delay: f64,
    duration: f64,
    /// None = infinite
    iterations: Option<f64>,
    direction: String,
    fill: String,
    easing: Easing,
    started: bool,
    iteration: i64,
    finished: bool,
}

#[derive(Debug, Clone)]
struct QueuedEvent {
    node: NodeId,
    ty: &'static str,
    name: String,
    elapsed: f32,
    transition: bool,
}

/// Running transitions and animations of one page.
#[derive(Debug, Default)]
pub struct AnimationState {
    transitions: Vec<Transition>,
    animations: Vec<Animation>,
    /// Base (un-animated) style of elements with effects.
    base: HashMap<NodeId, Rc<ComputedStyle>>,
    events: Vec<QueuedEvent>,
    /// Engine time (ms) of the next frame needed.
    next_due: Option<f64>,
}

impl AnimationState {
    /// When the page needs the next animation frame (engine ms).
    pub fn next_due(&self) -> Option<f64> {
        if self.active() {
            self.next_due.or(Some(0.0))
        } else {
            None
        }
    }

    /// Anything running (or events to deliver)?
    pub fn active(&self) -> bool {
        !self.transitions.is_empty() || self.animations.iter().any(|a| !a.finished) || !self.events.is_empty()
    }

    /// No effects and no element with a transition or animation seen.
    pub fn is_idle(&self) -> bool {
        self.transitions.is_empty() && self.animations.is_empty() && self.base.is_empty() && self.events.is_empty()
    }

    fn has_effects(&self, node: NodeId) -> bool {
        self.transitions.iter().any(|t| t.node == node) || self.animations.iter().any(|a| a.node == node)
    }
}

/// One style change seen by a restyle: (element, before, after).
pub(crate) type Change = (NodeId, Option<Rc<ComputedStyle>>, Option<Rc<ComputedStyle>>);

fn now_ms(engine: &Engine) -> f64 {
    engine.start.elapsed().as_secs_f64() * 1000.0
}

/// The transition entry (duration s, delay s, easing) covering `p`.
fn transition_for(style: &ComputedStyle, p: Prop) -> Option<(f32, f32, String)> {
    style.transitions.iter().rev().find(|t| p.covered_by(&t.property)).map(|t| (t.duration, t.delay, t.timing.clone()))
}

impl Engine {
    /// Start / cancel effects after a restyle. `changes` lists the
    /// elements whose computed style changed (incremental restyle);
    /// `full` means every style was recomputed from scratch.
    pub(crate) fn animations_after_style(&self, page: &mut Page, changes: Vec<Change>, full: bool) {
        if changes.is_empty() && !full && page.anim.base.is_empty() {
            return;
        }
        let now = now_ms(self);
        let mut touched: Vec<NodeId> = Vec::new();
        if full {
            // Overlays were dropped: take the new styles as bases, and pick
            // up animations on elements styled for the first time.
            let nodes: Vec<NodeId> = page.anim.base.keys().copied().collect();
            for n in nodes {
                match page.styles.get(n).cloned() {
                    Some(s) => {
                        page.anim.base.insert(n, s);
                    }
                    None => self.cancel_effects(page, n, false),
                }
                touched.push(n);
            }
            let count = page.styles.styles.len();
            for i in 0..count {
                let Some(Some(s)) = page.styles.styles.get(i) else { continue };
                if s.animations.is_empty() {
                    continue;
                }
                let node: NodeId = i;
                let s = s.clone();
                let base = page.anim.base.get(&node).cloned().unwrap_or(s);
                self.sync_animations(page, node, None, &base, now);
                touched.push(node);
            }
        }
        for (node, old, new) in changes {
            let Some(new) = new else {
                if page.anim.has_effects(node) {
                    self.cancel_effects(page, node, true);
                }
                continue;
            };
            let before = page.anim.base.get(&node).cloned().or(old.clone());
            // A recomputation that only dropped our overlay is not a change.
            if let Some(b) = &before {
                if Rc::ptr_eq(b, &new) || **b == *new {
                    if page.anim.has_effects(node) {
                        page.anim.base.insert(node, new);
                        touched.push(node);
                    }
                    continue;
                }
            }
            if let Some(before) = &before {
                if !new.transitions.is_empty() || page.anim.transitions.iter().any(|t| t.node == node) {
                    self.sync_transitions(page, node, before, &new, now);
                }
            }
            if !new.animations.is_empty() || page.anim.animations.iter().any(|a| a.node == node) {
                self.sync_animations(page, node, before.as_deref(), &new, now);
            }
            if page.anim.has_effects(node) {
                page.anim.base.insert(node, new);
                touched.push(node);
            } else {
                page.anim.base.remove(&node);
            }
        }
        for n in touched {
            self.apply_overlay(page, n, now);
        }
        if page.anim.active() {
            page.anim.next_due = Some(now);
        }
    }

    fn sync_transitions(&self, page: &mut Page, node: NodeId, before: &ComputedStyle, after: &ComputedStyle, now: f64) {
        for &p in ALL {
            let to = get(after, p);
            let running = page.anim.transitions.iter().position(|t| t.node == node && t.prop == p);
            let from = match running {
                Some(i) => page.anim.transitions[i].value_at(now),
                None => get(before, p),
            };
            if let Some(i) = running {
                if page.anim.transitions[i].to == to {
                    continue; // still heading to the same value
                }
            }
            let spec = transition_for(after, p).filter(|(d, _, _)| *d > 0.0);
            if let Some(i) = running {
                // Replaced or no longer transitioned: cancel.
                let t = page.anim.transitions.remove(i);
                page.anim.events.push(QueuedEvent { node, ty: "transitioncancel", name: p.name().into(), elapsed: ((now - t.start - t.delay).max(0.0) / 1000.0) as f32, transition: true });
            }
            let Some((dur, delay, timing)) = spec else { continue };
            if from == to {
                continue;
            }
            // Discrete values (auto widths, mismatched transforms) jump.
            if !interpolable(&from, &to) {
                continue;
            }
            page.anim.events.push(QueuedEvent { node, ty: "transitionrun", name: p.name().into(), elapsed: 0.0, transition: true });
            page.anim.transitions.push(Transition {
                node,
                prop: p,
                from,
                to,
                start: now,
                delay: delay as f64 * 1000.0,
                duration: dur as f64 * 1000.0,
                easing: parse_easing(&timing),
                started: false,
            });
        }
    }

    fn sync_animations(&self, page: &mut Page, node: NodeId, before: Option<&ComputedStyle>, after: &ComputedStyle, now: f64) {
        let old_names: Vec<&str> = before.map(|b| b.animations.iter().map(|a| a.name.as_str()).collect()).unwrap_or_default();
        // Cancel animations whose name went away.
        let names: Vec<String> = after.animations.iter().map(|a| a.name.clone()).collect();
        let mut i = 0;
        while i < page.anim.animations.len() {
            let a = &page.anim.animations[i];
            if a.node == node && !names.contains(&a.name) {
                let a = page.anim.animations.remove(i);
                if !a.finished {
                    page.anim.events.push(QueuedEvent { node, ty: "animationcancel", name: a.name, elapsed: 0.0, transition: false });
                }
            } else {
                i += 1;
            }
        }
        for spec in &after.animations {
            if spec.name.is_empty() || spec.name == "none" {
                continue;
            }
            let running = page.anim.animations.iter().any(|a| a.node == node && a.name == spec.name);
            if running || (old_names.contains(&spec.name.as_str()) && before.is_some() && page.anim.base.contains_key(&node)) {
                continue;
            }
            let Some(raw) = page.styles.keyframes.get(&spec.name).cloned() else { continue };
            let parent = page.doc.parent(node).and_then(|p| page.styles.get(p).cloned()).unwrap_or_else(|| Rc::new(ComputedStyle::initial()));
            let viewport = (page.viewport.width, page.viewport.height);
            let root_fs = page.styles.root_font_size;
            let mut frames: Vec<Keyframe> = Vec::new();
            for (offsets, decls) in &raw {
                let styled = rabbithole_style::cascade::apply_over(after, &parent, decls, root_fs, viewport);
                let declared: Vec<Prop> = ALL.iter().copied().filter(|p| decls.iter().any(|d| p.covered_by(&d.name))).collect();
                let values: Vec<(Prop, Val)> = declared.into_iter().map(|p| (p, get(&styled, p))).collect();
                for &o in offsets {
                    frames.push(Keyframe { offset: o.clamp(0.0, 1.0), values: values.clone() });
                }
            }
            frames.sort_by(|a, b| a.offset.partial_cmp(&b.offset).unwrap_or(std::cmp::Ordering::Equal));
            page.anim.animations.push(Animation {
                node,
                name: spec.name.clone(),
                frames,
                start: now,
                delay: spec.delay as f64 * 1000.0,
                duration: spec.duration as f64 * 1000.0,
                iterations: spec.iterations.map(|n| n as f64),
                direction: spec.direction.clone(),
                fill: spec.fill_mode.clone(),
                easing: parse_easing(&spec.timing),
                started: false,
                iteration: 0,
                finished: false,
            });
        }
    }

    fn cancel_effects(&self, page: &mut Page, node: NodeId, events: bool) {
        let a = &mut page.anim;
        let mut ev = Vec::new();
        a.transitions.retain(|t| {
            if t.node == node {
                ev.push(QueuedEvent { node, ty: "transitioncancel", name: t.prop.name().into(), elapsed: 0.0, transition: true });
                false
            } else {
                true
            }
        });
        a.animations.retain(|x| {
            if x.node == node {
                if !x.finished {
                    ev.push(QueuedEvent { node, ty: "animationcancel", name: x.name.clone(), elapsed: 0.0, transition: false });
                }
                false
            } else {
                true
            }
        });
        a.base.remove(&node);
        if events {
            a.events.extend(ev);
        }
    }

    /// Write base + animated values into `page.styles` for `node`.
    fn apply_overlay(&self, page: &mut Page, node: NodeId, now: f64) {
        let Some(base) = page.anim.base.get(&node).cloned() else { return };
        let mut style = (*base).clone();
        for t in page.anim.transitions.iter().filter(|t| t.node == node) {
            set(&mut style, t.prop, &t.value_at(now));
        }
        for a in page.anim.animations.iter().filter(|a| a.node == node) {
            for (p, v) in animation_values(a, &base, now) {
                set(&mut style, p, &v);
            }
        }
        let changed = page.styles.get(node).map(|s| **s != style).unwrap_or(true);
        if changed {
            if let Some(slot) = page.styles.styles.get_mut(node) {
                *slot = Some(Rc::new(style));
                page.incr.style_patched(node);
            }
        }
    }

    /// Advance transitions and animations; queue their events. Returns
    /// whether a style changed (relayout / repaint needed).
    pub(crate) fn advance_css_animations(&self, page: &mut Page) -> bool {
        if !page.anim.active() {
            return false;
        }
        let now = now_ms(self);
        let mut nodes: Vec<NodeId> = Vec::new();
        // Transitions: start / end.
        let mut i = 0;
        while i < page.anim.transitions.len() {
            let t = &mut page.anim.transitions[i];
            nodes.push(t.node);
            if !t.started && now >= t.start + t.delay {
                t.started = true;
                let (node, name) = (t.node, t.prop.name().to_string());
                page.anim.events.push(QueuedEvent { node, ty: "transitionstart", name, elapsed: 0.0, transition: true });
            }
            let t = &page.anim.transitions[i];
            if now >= t.start + t.delay + t.duration {
                let t = page.anim.transitions.remove(i);
                page.anim.events.push(QueuedEvent { node: t.node, ty: "transitionend", name: t.prop.name().into(), elapsed: (t.duration / 1000.0) as f32, transition: true });
            } else {
                i += 1;
            }
        }
        // Animations: start / iteration / end.
        for a in page.anim.animations.iter_mut() {
            nodes.push(a.node);
            if a.finished {
                continue;
            }
            let local = now - a.start - a.delay;
            if !a.started && local >= 0.0 {
                a.started = true;
                page.anim.events.push(QueuedEvent { node: a.node, ty: "animationstart", name: a.name.clone(), elapsed: 0.0, transition: false });
            }
            if local < 0.0 {
                continue;
            }
            let active = match a.iterations {
                Some(n) => a.duration * n,
                None => f64::INFINITY,
            };
            if local >= active {
                a.finished = true;
                page.anim.events.push(QueuedEvent { node: a.node, ty: "animationend", name: a.name.clone(), elapsed: (active / 1000.0) as f32, transition: false });
            } else if a.duration > 0.0 {
                let it = (local / a.duration).floor() as i64;
                if it > a.iteration {
                    a.iteration = it;
                    page.anim.events.push(QueuedEvent { node: a.node, ty: "animationiteration", name: a.name.clone(), elapsed: (it as f64 * a.duration / 1000.0) as f32, transition: false });
                }
            }
        }
        // Finished animations without forwards fill stop affecting style.
        page.anim.animations.retain(|a| !(a.finished && !(a.fill == "forwards" || a.fill == "both")));
        nodes.sort_unstable();
        nodes.dedup();
        let before = page.incr.generation();
        for n in &nodes {
            if !page.anim.has_effects(*n) {
                // Last effect ended: back to the base style.
                if let Some(base) = page.anim.base.remove(n) {
                    if let Some(slot) = page.styles.styles.get_mut(*n) {
                        *slot = Some(base);
                        page.incr.style_patched(*n);
                    }
                }
            } else {
                self.apply_overlay(page, *n, now);
            }
        }
        page.anim.next_due = Some(now + 16.0);
        page.incr.generation() != before
    }

    /// Deliver queued transition/animation events.
    pub(crate) fn dispatch_animation_events(&self, page: &mut Page) -> ScriptOutcome {
        let events = std::mem::take(&mut page.anim.events);
        let mut out = ScriptOutcome::default();
        for e in events {
            if !page.doc.is_connected(e.node) {
                continue;
            }
            let init = EventInit {
                bubbles: true,
                cancelable: matches!(e.ty, "transitionrun" | "transitionstart" | "transitionend" | "transitioncancel") && false,
                detail: Some(EventDetail::Animation { name: e.name, elapsed: e.elapsed, pseudo: String::new(), transition: e.transition }),
            };
            let o = self.dispatch_event(page, e.node, e.ty, init);
            out.merge(o);
        }
        out
    }
}

/// Can `a` and `b` be interpolated (not just swapped at 50%)?
fn interpolable(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::F(_), Val::F(_)) | (Val::C(_), Val::C(_)) | (Val::V(_), Val::V(_)) => true,
        (Val::L(x), Val::L(y)) => lerp_len(x, y, 0.5).is_some(),
        (Val::T(x), Val::T(y)) => {
            let (x, y): (Vec<TransformOp>, Vec<TransformOp>) = if x.is_empty() {
                (y.iter().map(identity_of).collect(), y.clone())
            } else if y.is_empty() {
                (x.clone(), x.iter().map(identity_of).collect())
            } else {
                (x.clone(), y.clone())
            };
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| lerp_op(p, q, 0.5).is_some())
        }
        _ => false,
    }
}

/// Values of animation `a` at time `now` (base values fill keyframes that
/// do not declare a property).
fn animation_values(a: &Animation, base: &ComputedStyle, now: f64) -> Vec<(Prop, Val)> {
    let local = now - a.start - a.delay;
    let active = match a.iterations {
        Some(n) => a.duration * n,
        None => f64::INFINITY,
    };
    let fill_back = a.fill == "backwards" || a.fill == "both";
    let fill_fwd = a.fill == "forwards" || a.fill == "both";
    let (iter, progress) = if local < 0.0 {
        if !fill_back {
            return Vec::new();
        }
        (0i64, 0.0f64)
    } else if local >= active {
        if !fill_fwd {
            return Vec::new();
        }
        let n = a.iterations.unwrap_or(1.0);
        let whole = n.floor();
        if n > whole {
            (whole as i64, n - whole)
        } else {
            ((whole as i64 - 1).max(0), 1.0)
        }
    } else if a.duration <= 0.0 {
        (0, 1.0)
    } else {
        ((local / a.duration).floor() as i64, (local % a.duration) / a.duration)
    };
    let reverse = match a.direction.as_str() {
        "reverse" => true,
        "alternate" => iter % 2 == 1,
        "alternate-reverse" => iter % 2 == 0,
        _ => false,
    };
    let p = if reverse { 1.0 - progress } else { progress } as f32;
    // Properties any keyframe animates.
    let mut props: Vec<Prop> = Vec::new();
    for f in &a.frames {
        for (pr, _) in &f.values {
            if !props.contains(pr) {
                props.push(*pr);
            }
        }
    }
    let mut out = Vec::new();
    for pr in props {
        // Keyframes declaring pr, with implicit 0% / 100% from the base.
        let mut ks: Vec<(f32, Val)> = a.frames.iter().filter_map(|f| f.values.iter().find(|(q, _)| *q == pr).map(|(_, v)| (f.offset, v.clone()))).collect();
        if ks.first().map(|k| k.0 > 0.0).unwrap_or(true) {
            ks.insert(0, (0.0, get(base, pr)));
        }
        if ks.last().map(|k| k.0 < 1.0).unwrap_or(true) {
            ks.push((1.0, get(base, pr)));
        }
        let mut j = 0;
        while j + 1 < ks.len() - 1 && p > ks[j + 1].0 {
            j += 1;
        }
        let (o0, v0) = &ks[j];
        let (o1, v1) = &ks[(j + 1).min(ks.len() - 1)];
        let span = (o1 - o0).max(1e-6);
        let local_t = ((p - o0) / span).clamp(0.0, 1.0);
        out.push((pr, interpolate(v0, v1, ease(&a.easing, local_t))));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_curves() {
        assert!((ease(&parse_easing("linear"), 0.3) - 0.3).abs() < 1e-6);
        let e = parse_easing("ease-in-out");
        assert!((ease(&e, 0.5) - 0.5).abs() < 1e-3);
        assert!(ease(&e, 0.1) < 0.1);
        assert_eq!(parse_easing("cubic-bezier(0.4, 0, 0.2, 1)"), Easing::Bezier(0.4, 0.0, 0.2, 1.0));
        assert_eq!(ease(&parse_easing("steps(4, end)"), 0.3), 0.25);
        assert_eq!(ease(&parse_easing("step-start"), 0.01), 1.0);
    }

    #[test]
    fn interpolation() {
        assert_eq!(interpolate(&Val::F(0.0), &Val::F(1.0), 0.25), Val::F(0.25));
        assert_eq!(interpolate(&Val::L(Length::Px(0.0)), &Val::L(Length::Px(100.0)), 0.5), Val::L(Length::Px(50.0)));
        assert_eq!(interpolate(&Val::L(Length::Auto), &Val::L(Length::Px(100.0)), 0.4), Val::L(Length::Auto));
        let t = interpolate(&Val::T(vec![]), &Val::T(vec![TransformOp::Rotate(1.0)]), 0.5);
        assert_eq!(t, Val::T(vec![TransformOp::Rotate(0.5)]));
        assert_eq!(interpolate(&Val::V(Visibility::Visible), &Val::V(Visibility::Hidden), 0.9), Val::V(Visibility::Visible));
    }
}
