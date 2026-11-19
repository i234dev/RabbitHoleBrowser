//! Web Animations bridge between `js/animations.js` (the timing model,
//! promises and events) and the engine (rendering).
//!
//! The script side owns the animation state. Whenever an animation's
//! effect changes (keyframes, target, progress) it records the effect in
//! a [`WebAnimRegistry`] shared with the embedder through
//! [`crate::HostEnv::web_animations`]. The embedder applies every effect
//! with a resolved progress over the element's style, at the animation
//! level of the cascade. The document timeline only advances at animation
//! frames (`run_timers`), where the script hook updates the animations and
//! then sends their events.

use std::cell::RefCell;
use std::rc::Rc;

use rabbithole_dom::NodeId;
use rabbithole_js::{JsResult, Realm, Value, V};

use crate::env::with_env;
use crate::host::*;

/// One keyframe of a script effect.
#[derive(Debug, Clone, PartialEq)]
pub struct WebAnimKeyframe {
    /// Computed offset in [0, 1].
    pub offset: f64,
    /// Easing applied to the interval starting at this keyframe.
    pub easing: String,
    /// `replace`, `add`, `accumulate` or `auto` (the effect's).
    pub composite: String,
    /// (CSS property name, specified value).
    pub decls: Vec<(String, String)>,
}

/// A keyframe effect of a script animation.
#[derive(Debug, Clone)]
pub struct WebAnimEffect {
    pub id: u64,
    pub node: NodeId,
    pub frames: Rc<Vec<WebAnimKeyframe>>,
    /// Bumped whenever `frames` or `node` change.
    pub frames_version: u64,
    /// The effect's composite operation.
    pub composite: String,
    /// Transformed (eased) iteration progress; `None` = not in effect.
    pub progress: Option<f64>,
    pub iteration: f64,
    /// Composite order (creation order of the animation).
    pub order: u64,
}

/// Effects of the script animations of one document.
#[derive(Debug, Default)]
pub struct WebAnimRegistry {
    /// Bumped on every change.
    pub generation: u64,
    /// Effects sorted by composite order.
    pub effects: Vec<WebAnimEffect>,
}

impl WebAnimRegistry {
    /// Effects in effect on `node`, lowest composite order first.
    pub fn effects_of(&self, node: NodeId) -> impl Iterator<Item = &WebAnimEffect> {
        self.effects.iter().filter(move |e| e.node == node && e.progress.is_some())
    }

    /// Any effect in effect?
    pub fn any_in_effect(&self) -> bool {
        self.effects.iter().any(|e| e.progress.is_some())
    }
}

pub type WebAnimations = Rc<RefCell<WebAnimRegistry>>;

/// A CSS animation or transition run by the embedder, as seen by its
/// `CSSAnimation` / `CSSTransition` object.
#[derive(Debug, Clone, Default)]
pub struct CssAnimationInfo {
    pub id: u64,
    pub node: NodeId,
    pub transition: bool,
    /// `animation-name`, or the transitioned property.
    pub name: String,
    /// Start time (embedder clock, ms, as [`crate::HostEnv::now_ms`]).
    pub start_time: Option<f64>,
    /// Hold time (local ms).
    pub hold_time: Option<f64>,
    pub rate: f64,
    pub delay: f64,
    pub duration: f64,
    /// `f64::INFINITY` for `infinite`.
    pub iterations: f64,
    pub direction: String,
    pub fill: String,
    /// The effect's easing (keyframe easing for CSS animations).
    pub easing: String,
    pub keyframe_easing: String,
    /// Position in `animation-name` (composite order).
    pub order: u64,
    /// It ran to its end and no longer exists.
    pub ended: bool,
    /// `@keyframes` frames: (offset, declarations).
    pub keyframes: Vec<(f64, Vec<rabbithole_css::Declaration>)>,
}

/// Per-realm state of Web Animations.
#[derive(Default)]
pub struct WebAnimState {
    /// `update(phase)`: phase 0 updates animations, phase 1 sends events.
    pub hook: Option<Value>,
    /// Animations need frames (running, pending, or events queued).
    pub active: bool,
    /// Document timeline current time (ms since the time origin).
    pub timeline: f64,
    pub registry: Option<WebAnimations>,
    pub frames_version: u64,
}

fn registry() -> WebAnimations {
    let st = state();
    let mut st = st.borrow_mut();
    st.webanim.registry.get_or_insert_with(Default::default).clone()
}

/// Tell the embedder the registry changed.
fn changed(reg: &WebAnimations) {
    reg.borrow_mut().generation += 1;
    with_env(|env| env.web_animations(reg));
}

fn num(realm: &mut Realm, v: Value) -> JsResult<f64> {
    realm.to_number(v)
}

fn get_index(realm: &mut Realm, v: &Value, i: usize) -> JsResult<Value> {
    realm.get_str(v.clone(), &i.to_string())
}

fn length(realm: &mut Realm, v: &Value) -> JsResult<usize> {
    if !v.is_object() {
        return Ok(0);
    }
    let l = realm.get_str(v.clone(), "length")?;
    Ok(realm.to_number(l)?.max(0.0).min(1e6) as usize)
}

/// The main-document element an argument names.
fn element_node(realm: &mut Realm, v: &Value) -> Option<NodeId> {
    let n = node_of(realm, v)?;
    (n.doc == 0).then_some(n.node)
}

/// `animTimeline()`: the document timeline's current time. Between
/// animation frames it stays put while animations run; with nothing
/// running it follows the clock.
fn anim_timeline(_realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    let now = with_env(|env| env.now_ms());
    let st = state();
    let mut st = st.borrow_mut();
    if !st.webanim.active {
        let t = (now - st.time_origin).max(0.0);
        // Never go back (a frame may have set a later value).
        if t > st.webanim.timeline {
            st.webanim.timeline = t;
        }
    }
    Ok(Value::Number(st.webanim.timeline))
}

/// `animSetHook(fn)`
fn anim_set_hook(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let f = arg(args, 0);
    root_value(realm, &f);
    state().borrow_mut().webanim.hook = Some(f);
    Ok(Value::Undefined)
}

/// `animSetActive(bool)`
fn anim_set_active(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let on = realm.to_boolean(&arg(args, 0));
    let st = state();
    let mut st = st.borrow_mut();
    if on && !st.webanim.active {
        // Frames start from the current time.
        let now = with_env(|env| env.now_ms());
        let t = (now - st.time_origin).max(0.0);
        if t > st.webanim.timeline && st.last_frame + 16.0 < now {
            st.webanim.timeline = t;
        }
    }
    st.webanim.active = on;
    Ok(Value::Undefined)
}

/// `animFrames(id, element, [[offset, easing, composite, prop, value, ...], ...])`
fn anim_frames(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = num(realm, arg(args, 0))? as u64;
    let node = element_node(realm, &arg(args, 1));
    let list = arg(args, 2);
    let mut frames = Vec::new();
    for i in 0..length(realm, &list)? {
        let f = get_index(realm, &list, i)?;
        let n = length(realm, &f)?;
        if n < 3 {
            continue;
        }
        let offset = { let v = get_index(realm, &f, 0)?; num(realm, v)? };
        let easing = { let v = get_index(realm, &f, 1)?; realm.to_rust_string(v)? };
        let composite = { let v = get_index(realm, &f, 2)?; realm.to_rust_string(v)? };
        let mut decls = Vec::new();
        let mut j = 3;
        while j + 1 < n {
            let p = { let v = get_index(realm, &f, j)?; realm.to_rust_string(v)? };
            let v = { let v = get_index(realm, &f, j + 1)?; realm.to_rust_string(v)? };
            decls.push((p, v));
            j += 2;
        }
        frames.push(WebAnimKeyframe { offset, easing, composite, decls });
    }
    let reg = registry();
    let version = {
        let st = state();
        let mut st = st.borrow_mut();
        st.webanim.frames_version += 1;
        st.webanim.frames_version
    };
    {
        let mut r = reg.borrow_mut();
        match (node, r.effects.iter().position(|e| e.id == id)) {
            (Some(node), Some(i)) => {
                let e = &mut r.effects[i];
                e.node = node;
                e.frames = Rc::new(frames);
                e.frames_version = version;
            }
            (Some(node), None) => {
                r.effects.push(WebAnimEffect { id, node, frames: Rc::new(frames), frames_version: version, composite: "replace".into(), progress: None, iteration: 0.0, order: id });
            }
            (None, Some(i)) => {
                r.effects.remove(i);
            }
            (None, None) => return Ok(Value::Undefined),
        }
    }
    changed(&reg);
    Ok(Value::Undefined)
}

/// `animProgress(id, progress | null, iteration, order, composite)`
fn anim_progress(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = num(realm, arg(args, 0))? as u64;
    let progress = match arg(args, 1).v() {
        V::Null | V::Undefined => None,
        v => Some(num(realm, v.to_value())?).filter(|p| p.is_finite()),
    };
    let iteration = num(realm, arg(args, 2))?;
    let order = num(realm, arg(args, 3))? as u64;
    let composite = match arg(args, 4).v() {
        V::Undefined => None,
        v => Some(realm.to_rust_string(v.to_value())?),
    };
    let reg = registry();
    {
        let mut r = reg.borrow_mut();
        let Some(i) = r.effects.iter().position(|e| e.id == id) else { return Ok(Value::Undefined) };
        let e = &mut r.effects[i];
        let comp_same = composite.as_ref().is_none_or(|c| *c == e.composite);
        if e.progress == progress && e.iteration == iteration && e.order == order && comp_same {
            return Ok(Value::Undefined);
        }
        e.progress = progress;
        e.iteration = iteration;
        if let Some(c) = composite {
            e.composite = c;
        }
        if e.order != order {
            e.order = order;
            r.effects.sort_by_key(|e| e.order);
        }
    }
    changed(&reg);
    Ok(Value::Undefined)
}

/// `animDrop(id)`: the effect no longer exists.
fn anim_drop(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = num(realm, arg(args, 0))? as u64;
    let reg = registry();
    let had = {
        let mut r = reg.borrow_mut();
        let before = r.effects.len();
        r.effects.retain(|e| e.id != id);
        before != r.effects.len()
    };
    if had {
        changed(&reg);
    }
    Ok(Value::Undefined)
}

/// `animCommit(element, order, [props])`: computed values of `props` (CSS
/// names) of `element` with the script effects of composite order up to
/// `order` applied, as `[prop, value, ...]` (`commitStyles`).
fn anim_commit(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(node) = element_node(realm, &arg(args, 0)) else { return Ok(Value::Null) };
    let order = num(realm, arg(args, 1))? as u64;
    let list = arg(args, 2);
    let mut props = Vec::new();
    for i in 0..length(realm, &list)? {
        let v = get_index(realm, &list, i)?;
        props.push(realm.to_rust_string(v)?);
    }
    let Some(style) = with_env(|env| env.web_animation_style(node, order)) else { return Ok(Value::Null) };
    let mut out = Vec::new();
    for p in props {
        let v = crate::cssom::computed_value_of_style(&style, &p);
        out.push(Value::str(&p));
        out.push(Value::str(&v));
    }
    Ok(array(realm, out))
}

/// Run the animation update and event steps of a frame at `time` (ms
/// since the time origin). Returns whether the hook ran.
pub(crate) fn frame(realm: &mut Realm, time: f64) -> bool {
    let hook = {
        let st = state();
        let mut st = st.borrow_mut();
        if time > st.webanim.timeline {
            st.webanim.timeline = time;
        }
        if !st.webanim.active {
            return false;
        }
        st.webanim.hook.clone()
    };
    let Some(hook) = hook else { return false };
    realm.begin_task();
    call_logged(realm, &hook, Value::Undefined, &[Value::Number(0.0)], "update animations");
    realm.run_jobs();
    realm.begin_task();
    call_logged(realm, &hook, Value::Undefined, &[Value::Number(1.0)], "animation events");
    true
}

fn info_object(realm: &mut Realm, info: CssAnimationInfo) -> Value {
    if info.ended {
        return object_from(realm, vec![("id", Value::Number(info.id as f64)), ("ended", Value::Bool(true))]);
    }
    let origin = state().borrow().time_origin;
    let target = wrap_main(realm, info.node);
    let opt = |v: Option<f64>| v.map(Value::Number).unwrap_or(Value::Null);
    let mut frames = Vec::new();
    for (offset, decls) in &info.keyframes {
        let mut row = vec![Value::Number(*offset), Value::str(&info.keyframe_easing)];
        let mut easing = None;
        for d in decls {
            let v = crate::cssom::tokens_to_css(&d.value);
            if d.name == "animation-timing-function" {
                easing = Some(v);
                continue;
            }
            row.push(Value::str(&d.name));
            row.push(Value::str(v.trim()));
        }
        if let Some(e) = easing {
            row[1] = Value::str(e.trim());
        }
        frames.push(array(realm, row));
    }
    let frames = array(realm, frames);
    object_from(
        realm,
        vec![
            ("id", Value::Number(info.id as f64)),
            ("target", target),
            ("transition", Value::Bool(info.transition)),
            ("name", Value::str(&info.name)),
            ("startTime", opt(info.start_time.map(|s| s - origin))),
            ("holdTime", opt(info.hold_time)),
            ("rate", Value::Number(info.rate)),
            ("delay", Value::Number(info.delay)),
            ("duration", Value::Number(info.duration)),
            ("iterations", Value::Number(info.iterations)),
            ("direction", Value::str(&info.direction)),
            ("fill", Value::str(&info.fill)),
            ("easing", Value::str(&info.easing)),
            ("order", Value::Number(info.order as f64)),
            ("keyframes", frames),
        ],
    )
}

/// `cssAnimations()`: the main document's CSS animations and transitions.
fn css_animations(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    let infos = with_env(|env| env.css_animations());
    let list: Vec<Value> = infos.into_iter().map(|i| info_object(realm, i)).collect();
    Ok(array(realm, list))
}

/// `cssAnimation(id)`: one of them, `{ended: true}` once it ran to its
/// end, or null.
fn css_animation(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = num(realm, arg(args, 0))? as u64;
    match with_env(|env| env.css_animation(id)) {
        Some(info) => Ok(info_object(realm, info)),
        None => Ok(Value::Null),
    }
}

/// `cssAnimationSet(id, startTime | null, holdTime | null, rate)`: a script
/// changed its playback (times in timeline ms).
fn css_animation_set(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = num(realm, arg(args, 0))? as u64;
    let opt = |realm: &mut Realm, v: Value| -> JsResult<Option<f64>> {
        match v.v() {
            V::Null | V::Undefined => Ok(None),
            _ => Ok(Some(realm.to_number(v)?).filter(|n| n.is_finite())),
        }
    };
    let start = opt(realm, arg(args, 1))?;
    let hold = opt(realm, arg(args, 2))?;
    let rate = num(realm, arg(args, 3))?;
    let origin = state().borrow().time_origin;
    with_env(|env| env.set_css_animation(id, start.map(|s| s + origin), hold, rate));
    Ok(Value::Undefined)
}

pub(crate) fn install(realm: &mut Realm, o: rabbithole_js::ObjectId) {
    realm.define_method(o, "cssAnimations", 0, css_animations);
    realm.define_method(o, "cssAnimation", 1, css_animation);
    realm.define_method(o, "cssAnimationSet", 4, css_animation_set);
    realm.define_method(o, "animTimeline", 0, anim_timeline);
    realm.define_method(o, "animSetHook", 1, anim_set_hook);
    realm.define_method(o, "animSetActive", 1, anim_set_active);
    realm.define_method(o, "animFrames", 3, anim_frames);
    realm.define_method(o, "animProgress", 5, anim_progress);
    realm.define_method(o, "animDrop", 1, anim_drop);
    realm.define_method(o, "animCommit", 3, anim_commit);
}
