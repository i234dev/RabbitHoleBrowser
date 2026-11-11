//! Pointer (mouse) interaction shared by every embedder (GUI, CLI, MCP,
//! tests): hit testing at viewport coordinates, the DOM event sequence
//! (`pointerover`/`mouseenter`/`mousemove`/`pointerdown`/`mousedown`/focus/
//! `pointerup`/`mouseup`/`click`) and the activation behaviour that follows
//! an uncancelled click (links, submit/reset buttons, checkboxes and radios,
//! labels, `<details>`/`<summary>`).

use rabbithole_dom::{Document, NodeId};
use rabbithole_net::Url;
use rabbithole_script::{EventDetail, EventInit};

use crate::forms::{control_kind, set_attr, ControlKind};
use crate::scripting::{plain_init, ScriptOutcome};
use crate::{Engine, Page};

/// Keyboard modifiers held during a pointer event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

/// Mouse buttons (DOM `MouseEvent.button` numbering).
pub const BUTTON_PRIMARY: u8 = 0;
pub const BUTTON_AUXILIARY: u8 = 1;
pub const BUTTON_SECONDARY: u8 = 2;

/// Pointer state kept per page.
#[derive(Debug, Clone, Default)]
pub struct PointerState {
    /// Element under the pointer (target of the last move).
    pub hover: Option<NodeId>,
    /// Target and button of the press in progress.
    pub down: Option<(NodeId, u8)>,
    /// `pointerdown` was cancelled: compatibility mouse events
    /// (`mousedown`/`mouseup`) are suppressed until release.
    pub suppress_mouse: bool,
    /// The last user input was a pointer press (not the keyboard): focus
    /// it moves matches `:focus-visible` only on text-entry controls.
    /// Embedders clear it on key presses.
    pub pointer_modality: bool,
    /// Iframe under the pointer (its document got the last move).
    pub frame_hover: Option<NodeId>,
    /// Iframe the press in progress started in.
    pub frame_press: Option<NodeId>,
}

/// What the embedder should do after a click.
#[derive(Debug, Clone, PartialEq)]
pub enum ClickAction {
    None,
    /// Follow a link. `new_tab` for modifier clicks and `target=_blank`.
    Navigate { url: Url, new_tab: bool },
    /// Submit a form (the embedder dispatches `submit` and navigates).
    Submit { form: NodeId, submitter: Option<NodeId> },
    /// Focus / start editing a text control or open a `<select>` (e.g. a
    /// click on its `<label>`).
    FocusControl(NodeId),
    /// The page changed in place (checkbox toggled, details opened, form
    /// reset): repaint.
    Changed,
}

/// Result of a button press.
#[derive(Debug, Default)]
pub struct PointerDown {
    pub target: Option<NodeId>,
    /// `mousedown` (or `pointerdown`) was cancelled: no focus change, no
    /// text selection.
    pub prevented: bool,
    pub outcome: ScriptOutcome,
}

/// Result of a button release.
#[derive(Debug)]
pub struct PointerUp {
    pub target: Option<NodeId>,
    /// Element the `click` was dispatched to (None: no click).
    pub clicked: Option<NodeId>,
    pub action: ClickAction,
    pub outcome: ScriptOutcome,
}

impl ScriptOutcome {
    /// Fold the side effects of a later dispatch into this one.
    pub fn merge(&mut self, o: ScriptOutcome) {
        self.relayout |= o.relayout;
        if self.navigation.is_none() {
            self.navigation = o.navigation;
        }
        self.new_windows.extend(o.new_windows);
        self.dialogs.extend(o.dialogs);
        if o.scroll_to.is_some() {
            self.scroll_to = o.scroll_to;
        }
    }
}

fn mouse(x: f32, y: f32, button: u8, m: Modifiers, bubbles: bool, cancelable: bool) -> EventInit {
    EventInit {
        bubbles,
        cancelable,
        detail: Some(EventDetail::Mouse { x, y, button, ctrl: m.ctrl, shift: m.shift, alt: m.alt, meta: m.meta }),
    }
}

/// Inclusive ancestors of `n`, innermost first.
fn chain(doc: &Document, n: NodeId) -> Vec<NodeId> {
    let mut v = vec![n];
    let mut cur = doc.parent(n);
    while let Some(p) = cur {
        if p == Document::ROOT {
            break;
        }
        v.push(p);
        cur = doc.parent(p);
    }
    v
}

fn is_disabled_control(doc: &Document, n: NodeId) -> bool {
    matches!(doc.tag(n), Some("button" | "input" | "select" | "textarea" | "option" | "optgroup" | "fieldset")) && doc.attr(n, "disabled").is_some()
}

/// Can the element receive focus from a click?
pub fn is_focusable(doc: &Document, n: NodeId) -> bool {
    let Some(el) = doc.element(n) else { return false };
    if is_disabled_control(doc, n) {
        return false;
    }
    match el.tag.as_str() {
        "a" | "area" if el.attr("href").is_some() => true,
        "button" | "select" | "textarea" | "iframe" | "summary" => true,
        "input" => !el.attr("type").map(|t| t.eq_ignore_ascii_case("hidden")).unwrap_or(false),
        _ => el.attr("tabindex").and_then(|t| t.trim().parse::<i32>().ok()).is_some() || el.attr("contenteditable").map(|v| v != "false").unwrap_or(false),
    }
}

/// Does the element take typed text (so focusing it by any means shows a
/// focus indicator, `:focus-visible`)?
pub fn is_text_entry(doc: &Document, n: NodeId) -> bool {
    match doc.tag(n) {
        Some("textarea") => true,
        Some("input") => !matches!(
            doc.attr(n, "type").map(|t| t.to_ascii_lowercase()).as_deref(),
            Some("button" | "submit" | "reset" | "checkbox" | "radio" | "image" | "file" | "hidden" | "range" | "color")
        ),
        _ => doc.attr(n, "contenteditable").map(|v| !v.eq_ignore_ascii_case("false")).unwrap_or(false),
    }
}

/// The control a `<label>` is for (`for` attribute or first labelable
/// descendant).
pub fn labeled_control(doc: &Document, label: NodeId) -> Option<NodeId> {
    let labelable = |n: NodeId| match doc.tag(n) {
        Some("button" | "select" | "textarea" | "meter" | "output" | "progress") => true,
        Some("input") => !doc.attr(n, "type").map(|t| t.eq_ignore_ascii_case("hidden")).unwrap_or(false),
        _ => false,
    };
    if let Some(id) = doc.attr(label, "for") {
        return doc.element_by_id(id).filter(|&n| labelable(n));
    }
    doc.descendants(label).find(|&n| n != label && labelable(n))
}

fn is_javascript_url(href: &str) -> bool {
    let h = href.trim_start();
    h.len() >= 11 && h[..11].eq_ignore_ascii_case("javascript:")
}

/// Element with activation behaviour at or above `target`.
fn activation_element(doc: &Document, target: NodeId) -> Option<NodeId> {
    for n in chain(doc, target) {
        let Some(el) = doc.element(n) else { continue };
        match el.tag.as_str() {
            "a" | "area" if el.attr("href").is_some() => return Some(n),
            "button" | "label" => return Some(n),
            "input" => return Some(n),
            "summary" => {
                let parent = doc.parent(n)?;
                if doc.is_element(parent, "details") && doc.child_elements(parent).find(|&c| doc.is_element(c, "summary")) == Some(n) {
                    return Some(n);
                }
            }
            _ => {}
        }
    }
    None
}

impl Engine {
    fn dispatch_into(&self, page: &mut Page, out: &mut ScriptOutcome, target: NodeId, ty: &str, init: EventInit) -> bool {
        if !page.doc.is_connected(target) {
            return false;
        }
        let o = self.dispatch_event(page, target, ty, init);
        let prevented = o.default_prevented;
        out.merge(o);
        prevented
    }

    /// Move focus to `node` (None = blur), dispatching `blur`/`focusout`
    /// and `focus`/`focusin`. No-op when it already has focus.
    pub fn focus_element(&self, page: &mut Page, node: Option<NodeId>) -> ScriptOutcome {
        let mut out = ScriptOutcome::default();
        let old = page.state.focused;
        if old == node {
            return out;
        }
        if self.set_focus(page, node) {
            out.relayout = true;
        }
        if let Some(h) = page.script.host.as_mut() {
            h.set_focused(node);
        }
        if let Some(o) = old {
            self.dispatch_into(page, &mut out, o, "blur", EventInit::simple(false, false));
            self.dispatch_into(page, &mut out, o, "focusout", EventInit::simple(true, false));
        }
        if let Some(n) = node {
            self.dispatch_into(page, &mut out, n, "focus", EventInit::simple(false, false));
            self.dispatch_into(page, &mut out, n, "focusin", EventInit::simple(true, false));
        }
        out
    }

    /// The pointer moved to viewport point `(x, y)`: update hover state and
    /// dispatch `pointerout`/`mouseout`, `pointerleave`/`mouseleave`,
    /// `pointerover`/`mouseover`, `pointerenter`/`mouseenter` (when the
    /// target changed) and `pointermove`/`mousemove`.
    pub fn pointer_move(&self, page: &mut Page, x: f32, y: f32, m: Modifiers) -> ScriptOutcome {
        let mut out = ScriptOutcome::default();
        self.frame_pointer_move(page, x, y, m, &mut out);
        let target = page.node_at_viewport(x, y);
        let old = page.pointer.hover.filter(|&o| page.doc.is_connected(o));
        if old != target {
            let old_chain = old.map(|o| chain(&page.doc, o)).unwrap_or_default();
            let new_chain = target.map(|t| chain(&page.doc, t)).unwrap_or_default();
            if let Some(o) = old {
                self.dispatch_into(page, &mut out, o, "pointerout", mouse(x, y, 0, m, true, true));
                self.dispatch_into(page, &mut out, o, "mouseout", mouse(x, y, 0, m, true, true));
                for &n in old_chain.iter().filter(|n| !new_chain.contains(n)) {
                    self.dispatch_into(page, &mut out, n, "pointerleave", mouse(x, y, 0, m, false, false));
                    self.dispatch_into(page, &mut out, n, "mouseleave", mouse(x, y, 0, m, false, false));
                }
            }
            page.pointer.hover = target;
            if self.set_hover(page, target) {
                out.relayout = true;
            }
            if let Some(t) = target {
                self.dispatch_into(page, &mut out, t, "pointerover", mouse(x, y, 0, m, true, true));
                self.dispatch_into(page, &mut out, t, "mouseover", mouse(x, y, 0, m, true, true));
                for &n in new_chain.iter().rev().filter(|n| !old_chain.contains(n)) {
                    self.dispatch_into(page, &mut out, n, "pointerenter", mouse(x, y, 0, m, false, false));
                    self.dispatch_into(page, &mut out, n, "mouseenter", mouse(x, y, 0, m, false, false));
                }
            }
        }
        if let Some(t) = page.pointer.hover {
            self.dispatch_into(page, &mut out, t, "pointermove", mouse(x, y, 0, m, true, true));
            self.dispatch_into(page, &mut out, t, "mousemove", mouse(x, y, 0, m, true, true));
        }
        out
    }

    /// A button was pressed at viewport point `(x, y)`: `pointerdown`,
    /// `mousedown`, then (unless cancelled, primary button) focus moves to
    /// the nearest focusable ancestor of the target, or is cleared.
    pub fn pointer_down(&self, page: &mut Page, x: f32, y: f32, button: u8, m: Modifiers) -> PointerDown {
        page.pointer.frame_press = None;
        if let Some(d) = self.frame_pointer_down(page, x, y, button, m) {
            return d;
        }
        let mut out = ScriptOutcome::default();
        let Some(target) = page.node_at_viewport(x, y) else {
            page.pointer.down = None;
            return PointerDown::default();
        };
        let disabled = chain(&page.doc, target).into_iter().any(|n| is_disabled_control(&page.doc, n));
        let mut prevented = false;
        page.pointer.pointer_modality = true;
        page.pointer.suppress_mouse = false;
        if self.dispatch_into(page, &mut out, target, "pointerdown", mouse(x, y, button, m, true, true)) {
            page.pointer.suppress_mouse = true;
            prevented = true;
        }
        if !page.pointer.suppress_mouse && !disabled && self.dispatch_into(page, &mut out, target, "mousedown", mouse(x, y, button, m, true, true)) {
            prevented = true;
        }
        page.pointer.down = Some((target, button));
        if button == BUTTON_PRIMARY {
            page.state.active = Some(target);
            if page.has_dynamic_rules {
                self.restyle(page);
                out.relayout = true;
            }
            if !prevented {
                let focus = chain(&page.doc, target).into_iter().find(|&n| is_focusable(&page.doc, n));
                out.merge(self.focus_element(page, focus));
            }
        }
        PointerDown { target: Some(target), prevented, outcome: out }
    }

    /// A button was released at viewport point `(x, y)`: `pointerup`,
    /// `mouseup`, then `click` (primary) or `auxclick` (other buttons) at
    /// the common ancestor of the press and release targets, followed by
    /// the activation behaviour when not cancelled.
    pub fn pointer_up(&self, page: &mut Page, x: f32, y: f32, button: u8, m: Modifiers) -> PointerUp {
        if let Some(up) = self.frame_pointer_up(page, x, y, button, m) {
            return up;
        }
        let mut out = ScriptOutcome::default();
        let target = page.node_at_viewport(x, y);
        let down = page.pointer.down.take();
        if page.state.active.take().is_some() && page.has_dynamic_rules {
            self.restyle(page);
            out.relayout = true;
        }
        let suppress = std::mem::take(&mut page.pointer.suppress_mouse);
        if let Some(t) = target {
            let disabled = chain(&page.doc, t).into_iter().any(|n| is_disabled_control(&page.doc, n));
            self.dispatch_into(page, &mut out, t, "pointerup", mouse(x, y, button, m, true, true));
            if !suppress && !disabled {
                self.dispatch_into(page, &mut out, t, "mouseup", mouse(x, y, button, m, true, true));
            }
        }
        // The click goes to the nearest common inclusive ancestor of the
        // press and release targets.
        let clicked = match (down, target) {
            (Some((d, b)), Some(t)) if b == button && page.doc.is_connected(d) => {
                let dc = chain(&page.doc, d);
                chain(&page.doc, t).into_iter().find(|n| dc.contains(n))
            }
            _ => None,
        };
        let mut action = ClickAction::None;
        if let Some(c) = clicked {
            if button == BUTTON_PRIMARY {
                let (a, o) = self.click_element(page, c, Some((x, y)), m);
                action = a;
                out.merge(o);
            } else if button == BUTTON_AUXILIARY {
                let prevented = self.dispatch_into(page, &mut out, c, "auxclick", mouse(x, y, button, m, true, true));
                if !prevented && out.navigation.is_none() {
                    if let Some(url) = page.link_of(c) {
                        action = ClickAction::Navigate { url, new_tab: true };
                    }
                }
            }
        }
        PointerUp { target, clicked, action, outcome: out }
    }

    /// Press and release the primary button at viewport point `(x, y)`.
    pub fn click_at(&self, page: &mut Page, x: f32, y: f32, m: Modifiers) -> PointerUp {
        let mut out = self.pointer_move(page, x, y, m);
        let down = self.pointer_down(page, x, y, BUTTON_PRIMARY, m);
        out.merge(down.outcome);
        let mut up = self.pointer_up(page, x, y, BUTTON_PRIMARY, m);
        out.merge(std::mem::take(&mut up.outcome));
        up.outcome = out;
        up
    }

    /// Dispatch a `click` to `target` and run its activation behaviour if
    /// it was not cancelled. `at` = viewport coordinates of the pointer
    /// (None for synthetic clicks: the element's centre is used).
    pub fn click_element(&self, page: &mut Page, target: NodeId, at: Option<(f32, f32)>, m: Modifiers) -> (ClickAction, ScriptOutcome) {
        self.click_element_depth(page, target, at, m, 0)
    }

    fn click_element_depth(&self, page: &mut Page, target: NodeId, at: Option<(f32, f32)>, m: Modifiers, depth: u32) -> (ClickAction, ScriptOutcome) {
        let mut out = ScriptOutcome::default();
        if chain(&page.doc, target).into_iter().any(|n| is_disabled_control(&page.doc, n)) {
            return (ClickAction::None, out);
        }
        let (x, y) = at.unwrap_or_else(|| {
            page.rect_of(target)
                .map(|r| (r.x + r.w / 2.0 - page.page_scroll.0, r.y + r.h / 2.0 - page.page_scroll.1))
                .unwrap_or((0.0, 0.0))
        });
        let act = activation_element(&page.doc, target);
        let kind = act.map(|a| control_kind(&page.doc, a)).unwrap_or(ControlKind::Other);
        let is_input_or_button = act.map(|a| matches!(page.doc.tag(a), Some("input" | "button"))).unwrap_or(false);
        // Legacy pre-activation: checkboxes and radios change before the
        // click is dispatched and revert if it is cancelled.
        let mut saved: Vec<(NodeId, bool)> = Vec::new();
        if let (Some(a), true, ControlKind::Checkbox | ControlKind::Radio) = (act, is_input_or_button, kind) {
            saved.push((a, page.doc.is_checked(a)));
            if kind == ControlKind::Radio {
                let name = page.doc.attr(a, "name").map(|s| s.to_string());
                if let Some(name) = name {
                    let scope = page.form_of(a).unwrap_or(Document::ROOT);
                    for n in page.doc.descendants(scope).collect::<Vec<_>>() {
                        if n != a && control_kind(&page.doc, n) == ControlKind::Radio && page.doc.attr(n, "name") == Some(name.as_str()) {
                            saved.push((n, page.doc.is_checked(n)));
                        }
                    }
                }
            }
            page.toggle_control(a);
            self.restyle(page);
            out.relayout = true;
        }
        let prevented = self.dispatch_into(page, &mut out, target, "click", mouse(x, y, BUTTON_PRIMARY, m, true, true));
        if prevented {
            if !saved.is_empty() {
                for (n, checked) in saved {
                    page.doc.set_checked(n, Some(checked));
                }
                self.restyle(page);
                out.relayout = true;
            }
            return (ClickAction::None, out);
        }
        // A script-requested navigation from the click handler wins.
        if out.navigation.is_some() {
            return (ClickAction::None, out);
        }
        let Some(a) = act else { return (ClickAction::None, out) };
        if !page.doc.is_connected(a) {
            return (ClickAction::None, out);
        }
        let tag = page.doc.tag(a).unwrap_or("").to_string();
        let action = match tag.as_str() {
            "a" | "area" if is_javascript_url(page.doc.attr(a, "href").unwrap_or("")) => {
                // `javascript:` links run their code in the page.
                if self.scripting {
                    let href = page.doc.attr(a, "href").unwrap_or("").trim().to_string();
                    let code = rabbithole_net::url::percent_decode(&href["javascript:".len()..]);
                    let (_, o) = self.eval_with_outcome(page, &code);
                    out.merge(o);
                }
                ClickAction::Changed
            }
            "a" | "area" => match page.link_of(a) {
                Some(url) => {
                    let blank = page.doc.attr(a, "target").map(|t| t.eq_ignore_ascii_case("_blank")).unwrap_or(false);
                    ClickAction::Navigate { url, new_tab: m.ctrl || m.meta || blank }
                }
                None => ClickAction::None,
            },
            "summary" => {
                if let Some(details) = page.doc.parent(a) {
                    let open = page.doc.attr(details, "open").is_some();
                    set_attr(&mut page.doc, details, "open", if open { None } else { Some("") });
                    self.restyle(page);
                    out.relayout = true;
                    self.dispatch_into(page, &mut out, details, "toggle", EventInit::simple(false, false));
                }
                ClickAction::Changed
            }
            "label" => {
                // Clicks on interactive content inside the label are theirs.
                let inner_interactive = chain(&page.doc, target).into_iter().take_while(|&n| n != a).any(|n| {
                    matches!(page.doc.tag(n), Some("a" | "button" | "input" | "select" | "textarea"))
                });
                match labeled_control(&page.doc, a) {
                    Some(c) if !inner_interactive && c != target && depth < 2 => {
                        match control_kind(&page.doc, c) {
                            ControlKind::Text | ControlKind::Password | ControlKind::TextArea | ControlKind::Select | ControlKind::Other => {
                                out.merge(self.focus_element(page, Some(c)));
                                ClickAction::FocusControl(c)
                            }
                            _ => {
                                let (act, o) = self.click_element_depth(page, c, None, m, depth + 1);
                                out.merge(o);
                                act
                            }
                        }
                    }
                    _ => ClickAction::None,
                }
            }
            _ => match kind {
                ControlKind::Checkbox | ControlKind::Radio => {
                    let changed = saved.first().map(|&(n, was)| page.doc.is_checked(n) != was).unwrap_or(false);
                    if changed {
                        self.dispatch_into(page, &mut out, a, "input", crate::scripting::input_init(None));
                        self.dispatch_into(page, &mut out, a, "change", plain_init(false));
                    }
                    ClickAction::Changed
                }
                ControlKind::Submit => match page.form_of(a) {
                    Some(form) => ClickAction::Submit { form, submitter: Some(a) },
                    None => ClickAction::None,
                },
                ControlKind::Reset => match page.form_of(a) {
                    Some(form) => {
                        let prevented = self.dispatch_into(page, &mut out, form, "reset", plain_init(true));
                        if !prevented {
                            page.reset_form(form);
                            self.restyle(page);
                            out.relayout = true;
                        }
                        ClickAction::Changed
                    }
                    None => ClickAction::None,
                },
                _ => ClickAction::None,
            },
        };
        (action, out)
    }
}
