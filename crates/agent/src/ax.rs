//! The semantic (accessibility-style) tree of a page: ARIA and implicit
//! HTML roles, accessible names (a practical subset of accname 1.2),
//! inline states, and what is actually painted (geometry from the layout
//! tree, clipping, opacity, contrast against the background).

use std::collections::HashMap;

use rabbithole_engine::dom::{Document, NodeId};
use rabbithole_engine::gfx::{Color, Rect};
use rabbithole_engine::layout::LayoutBox;
use rabbithole_engine::style::{ComputedStyle, Display, Position, Visibility};
use rabbithole_engine::{ControlKind, Page};

use crate::refs::RefMap;

/// Where an element is painted (document coordinates of its frame).
#[derive(Debug, Clone, Copy)]
pub struct Geo {
    /// Union of its boxes/fragments after overflow clipping.
    pub rect: Rect,
    /// Union before clipping.
    pub raw: Rect,
    /// Inside a `position: fixed` subtree (rect is relative to the viewport
    /// at scroll 0).
    pub fixed: bool,
}

/// Geometry of every element that generates boxes.
#[derive(Debug, Default)]
pub struct GeoMap {
    pub map: HashMap<NodeId, Geo>,
}

fn union(a: Rect, b: Rect) -> Rect {
    if a.w <= 0.0 && a.h <= 0.0 {
        return b;
    }
    if b.w <= 0.0 && b.h <= 0.0 {
        return a;
    }
    a.union(&b)
}

impl GeoMap {
    pub fn build(page: &Page) -> GeoMap {
        let mut g = GeoMap::default();
        walk_box(&page.layout.root, None, false, &mut g.map);
        // Elements without boxes of their own (an inline `<a>` whose text
        // is in a `<span>`) take the union of their descendants.
        let doc = &page.doc;
        let order: Vec<NodeId> = doc.descendants(Document::ROOT).collect();
        let mut derived: HashMap<NodeId, Geo> = HashMap::new();
        for &n in order.iter().rev() {
            let own = g.map.get(&n).copied().or_else(|| derived.get(&n).copied());
            let Some(geo) = own else { continue };
            let Some(p) = doc.parent(n) else { continue };
            if g.map.contains_key(&p) {
                continue;
            }
            let e = derived.entry(p).or_insert(geo);
            e.rect = union(e.rect, geo.rect);
            e.raw = union(e.raw, geo.raw);
            e.fixed |= geo.fixed;
        }
        for (k, v) in derived {
            g.map.entry(k).or_insert(v);
        }
        g
    }

    pub fn get(&self, n: NodeId) -> Option<&Geo> {
        self.map.get(&n)
    }

    fn add(map: &mut HashMap<NodeId, Geo>, n: NodeId, r: Rect, clip: Option<Rect>, fixed: bool) {
        let clipped = match clip {
            Some(c) => {
                let i = r.intersect(&c);
                if i.is_empty() {
                    Rect::new(r.x, r.y, 0.0, 0.0)
                } else {
                    i
                }
            }
            None => r,
        };
        let e = map.entry(n).or_insert(Geo { rect: Rect::new(clipped.x, clipped.y, 0.0, 0.0), raw: Rect::new(r.x, r.y, 0.0, 0.0), fixed });
        e.rect = union(e.rect, clipped);
        e.raw = union(e.raw, r);
        e.fixed |= fixed;
    }
}

fn walk_box(b: &LayoutBox, clip: Option<Rect>, fixed: bool, map: &mut HashMap<NodeId, Geo>) {
    let fixed = fixed || b.fixed;
    // Absolutely/fixed positioned boxes escape clips of ordinary ancestors
    // in most real layouts; we keep it simple and only reset for fixed.
    let clip = if b.fixed { None } else { clip };
    if let Some(n) = b.node {
        if b.pseudo.is_none() {
            GeoMap::add(map, n, b.rect, clip, fixed);
        }
    }
    let child_clip = if b.clips {
        Some(match clip {
            Some(c) => c.intersect(&b.rect),
            None => b.rect,
        })
    } else {
        clip
    };
    for line in &b.lines {
        for f in &line.fragments {
            if let Some(n) = f.node {
                GeoMap::add(map, n, f.rect, child_clip, fixed);
            }
        }
    }
    for c in &b.children {
        walk_box(c, child_clip, fixed, map);
    }
}

/// What kind of item a node is in the snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Landmark,
    Dialog,
    Heading,
    Control,
    Text,
    Image,
    /// A list (or a run of repeated siblings): children are its items.
    List,
    /// One item of a list.
    Item,
    Form,
    Frame,
    /// Other named groups (fieldset, details, table, figure).
    Group,
    /// A block boundary (not printed; prevents joining items on a line).
    Break,
}

/// One node of the semantic tree.
#[derive(Debug, Clone)]
pub struct Ax {
    pub frame: usize,
    /// The element (for text: the element containing it).
    pub node: NodeId,
    pub kind: Kind,
    pub role: String,
    pub name: String,
    pub level: u8,
    pub states: Vec<String>,
    pub href: Option<String>,
    pub reference: Option<u32>,
    pub children: Vec<Ax>,
    /// Painted rect (document coordinates of its frame).
    pub rect: Option<Rect>,
    pub fixed: bool,
    /// Why it is hidden (only kept when hidden content is requested).
    pub hidden: Option<&'static str>,
    /// Ordinal among text items of the same element (diff key).
    pub ordinal: u16,
}

impl Ax {
    fn new(frame: usize, node: NodeId, kind: Kind, role: &str) -> Ax {
        Ax {
            frame,
            node,
            kind,
            role: role.to_string(),
            name: String::new(),
            level: 0,
            states: Vec::new(),
            href: None,
            reference: None,
            children: Vec::new(),
            rect: None,
            fixed: false,
            hidden: None,
            ordinal: 0,
        }
    }

    /// Visit every node (pre-order).
    pub fn visit<'a>(&'a self, f: &mut dyn FnMut(&'a Ax, usize)) {
        fn go<'a>(a: &'a Ax, depth: usize, f: &mut dyn FnMut(&'a Ax, usize)) {
            f(a, depth);
            for c in &a.children {
                go(c, depth + 1, f);
            }
        }
        go(self, 0, f)
    }

    /// Plain text of the subtree (names of items joined).
    pub fn text(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        self.visit(&mut |a, _| {
            if matches!(a.kind, Kind::Text | Kind::Control | Kind::Heading | Kind::Image) && !a.name.is_empty() {
                parts.push(a.name.clone());
            }
        });
        parts.join(" ")
    }

    /// Diff/identity key.
    pub fn key(&self) -> (usize, NodeId, u16, bool) {
        (self.frame, self.node, self.ordinal, self.kind == Kind::Text)
    }
}

/// A page (frame) to build from, with the frames nested in it.
pub struct FrameView<'a> {
    pub page: &'a Page,
    /// (iframe element, frame index) of child frames.
    pub children: Vec<(NodeId, usize)>,
}

/// Options for building the tree.
#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    /// Keep hidden content (marked) instead of dropping it.
    pub include_hidden: bool,
    /// Nodes known to be invalid (failed constraint validation).
    pub invalid: Vec<(usize, NodeId)>,
    /// Overlays of frame 0 (shown as dialogs).
    pub overlays: Vec<NodeId>,
    /// Inferred main content of frame 0 when the page has no `<main>`.
    pub main: Option<NodeId>,
}

/// Statistics gathered while building.
#[derive(Debug, Default, Clone)]
pub struct BuildStats {
    pub hidden_text_chars: usize,
    pub hidden_items: usize,
}

struct Builder<'a> {
    frames: &'a [FrameView<'a>],
    refs: &'a mut RefMap,
    opts: &'a BuildOptions,
    stats: BuildStats,
    geos: Vec<GeoMap>,
}

/// Build the semantic tree of frame 0 (child frames are inlined).
pub fn build(frames: &[FrameView], refs: &mut RefMap, opts: &BuildOptions) -> (Ax, BuildStats) {
    let geos = frames.iter().map(|f| GeoMap::build(f.page)).collect();
    let mut b = Builder { frames, refs, opts, stats: BuildStats::default(), geos };
    let root = b.frame_root(0, 0);
    (root, b.stats)
}

/// Geometry helper for other modules.
pub fn geometry(page: &Page) -> GeoMap {
    GeoMap::build(page)
}

const LANDMARK_ROLES: &[&str] = &["banner", "navigation", "main", "contentinfo", "complementary", "search", "region", "form"];
const CONTROL_ROLES: &[&str] = &[
    "link", "button", "textbox", "searchbox", "checkbox", "radio", "combobox", "listbox", "slider", "spinbutton", "switch", "tab", "menuitem",
    "menuitemcheckbox", "menuitemradio", "option", "treeitem", "scrollbar",
];

/// Inherited context while walking.
#[derive(Clone, Copy)]
struct Ctx {
    opacity: f32,
    background: Color,
    has_bg_image: bool,
    /// Inside a sectioning element (article/aside/main/nav/section):
    /// header/footer are not landmarks there.
    in_section: bool,
    /// Inside an interactive element (its content is its name).
    in_control: bool,
}

fn is_block(d: Display) -> bool {
    !matches!(d, Display::Inline | Display::InlineBlock | Display::InlineFlex | Display::InlineGrid | Display::InlineTable | Display::Contents | Display::None)
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for w in s.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    out
}

/// Does the element generate boxes (has a style and is not `display:none`)?
pub fn displayed(page: &Page, n: NodeId) -> bool {
    page.styles.get(n).map(|s| s.display != Display::None).unwrap_or(false)
}

/// Truncate to `max` characters with an ellipsis.
pub fn clip_text(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out = out.trim_end().to_string();
    out.push('…');
    out
}

fn luminance(c: Color) -> f32 {
    let f = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * f(c.r) + 0.7152 * f(c.g) + 0.0722 * f(c.b)
}

/// WCAG contrast ratio between two opaque colors.
pub fn contrast(a: Color, b: Color) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

fn blend(fg: Color, bg: Color) -> Color {
    let a = fg.a as f32 / 255.0;
    let mix = |f: u8, b: u8| (f as f32 * a + b as f32 * (1.0 - a)).round() as u8;
    Color::rgb(mix(fg.r, bg.r), mix(fg.g, bg.g), mix(fg.b, bg.b))
}

/// Explicit ARIA role (first recognised token).
fn aria_role(doc: &Document, n: NodeId) -> Option<String> {
    let r = doc.attr(n, "role")?;
    r.split_whitespace().next().map(|s| s.to_ascii_lowercase())
}

/// Implicit role of an element (HTML-AAM, simplified).
fn implicit_role(doc: &Document, n: NodeId, ctx: &Ctx) -> Option<&'static str> {
    let el = doc.element(n)?;
    let t = el.tag.as_str();
    Some(match t {
        "a" | "area" if el.attr("href").is_some() => "link",
        "button" => "button",
        "summary" => "button",
        "input" => {
            let ty = el.attr("type").map(|s| s.trim().to_ascii_lowercase()).unwrap_or_default();
            match ty.as_str() {
                "checkbox" => "checkbox",
                "radio" => "radio",
                "range" => "slider",
                "number" => "spinbutton",
                "search" => "searchbox",
                "submit" | "reset" | "button" | "image" => "button",
                "hidden" => return None,
                "file" => "button",
                _ => {
                    if el.attr("list").is_some() {
                        "combobox"
                    } else {
                        "textbox"
                    }
                }
            }
        }
        "textarea" => "textbox",
        "select" => {
            let multiple = el.attr("multiple").is_some();
            let size = el.attr("size").and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(1);
            if multiple || size > 1 {
                "listbox"
            } else {
                "combobox"
            }
        }
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => "heading",
        "nav" => "navigation",
        "main" => "main",
        "aside" => "complementary",
        "search" => "search",
        "header" if !ctx.in_section => "banner",
        "footer" if !ctx.in_section => "contentinfo",
        "form" => "form",
        "section" if el.attr("aria-label").is_some() || el.attr("aria-labelledby").is_some() => "region",
        "dialog" => "dialog",
        "ul" | "ol" | "menu" => "list",
        "li" => "listitem",
        "table" => "table",
        "tr" => "row",
        "img" => {
            if el.attr("alt").map(|a| a.trim().is_empty()).unwrap_or(false) {
                return None;
            }
            "img"
        }
        "svg" if el.attr("aria-label").is_some() => "img",
        "fieldset" => "group",
        "details" => "group",
        "figure" => "figure",
        "article" => "article",
        "iframe" | "frame" => "iframe",
        "progress" => "progressbar",
        "meter" => "meter",
        "hr" => "separator",
        "option" => "option",
        _ => return None,
    })
}

fn kind_for_role(role: &str) -> Option<Kind> {
    Some(match role {
        r if LANDMARK_ROLES.contains(&r) => {
            if r == "form" {
                Kind::Form
            } else {
                Kind::Landmark
            }
        }
        "dialog" | "alertdialog" => Kind::Dialog,
        "heading" => Kind::Heading,
        r if CONTROL_ROLES.contains(&r) => Kind::Control,
        "list" | "table" | "grid" | "tablist" | "menu" | "menubar" | "tree" | "feed" | "rowgroup" => Kind::List,
        "listitem" | "row" | "article" => Kind::Item,
        "img" | "figure" => Kind::Image,
        "group" | "radiogroup" | "toolbar" | "tabpanel" | "alert" | "status" => Kind::Group,
        "iframe" => Kind::Frame,
        "progressbar" | "meter" => Kind::Control,
        _ => return None,
    })
}

/// Text of an element subtree for name-from-content (visible text, image
/// alts, nested aria-labels), skipping `display:none` and `aria-hidden`.
pub fn content_text(page: &Page, n: NodeId) -> String {
    let mut out = String::new();
    content_text_into(page, n, &mut out, 0);
    collapse_ws(&out)
}

fn content_text_into(page: &Page, n: NodeId, out: &mut String, depth: usize) {
    let doc = &page.doc;
    if depth > 40 {
        return;
    }
    for c in doc.children(n) {
        if let Some(t) = doc.text(c) {
            out.push_str(t);
            continue;
        }
        let Some(el) = doc.element(c) else { continue };
        if matches!(el.tag.as_str(), "script" | "style" | "template" | "noscript") {
            continue;
        }
        if !crate::ax::displayed(page, c) || el.attr("aria-hidden") == Some("true") {
            continue;
        }
        if let Some(l) = el.attr("aria-label").filter(|l| !l.trim().is_empty()) {
            out.push(' ');
            out.push_str(l);
            out.push(' ');
            continue;
        }
        match el.tag.as_str() {
            "img" | "area" => {
                if let Some(a) = el.attr("alt").or_else(|| el.attr("title")) {
                    out.push(' ');
                    out.push_str(a);
                    out.push(' ');
                }
            }
            "input" => {
                let v = el.attr("value").unwrap_or("");
                out.push(' ');
                out.push_str(v);
                out.push(' ');
            }
            "br" => out.push(' '),
            _ => {
                let block = page.styles.get(c).map(|s| is_block(s.display)).unwrap_or(false);
                if block {
                    out.push(' ');
                }
                content_text_into(page, c, out, depth + 1);
                if block {
                    out.push(' ');
                }
            }
        }
    }
}

fn labelled_by(page: &Page, ids: &str) -> String {
    let doc = &page.doc;
    let parts: Vec<String> = ids
        .split_whitespace()
        .filter_map(|id| doc.element_by_id(id))
        .map(|e| doc.attr(e, "aria-label").map(|s| s.to_string()).unwrap_or_else(|| collapse_ws(&doc.text_content(e))))
        .collect();
    collapse_ws(&parts.join(" "))
}

/// Text of the `<label>`s of a control (excluding the control itself).
fn label_text(page: &Page, n: NodeId) -> Option<String> {
    let doc = &page.doc;
    let mut parts = Vec::new();
    if let Some(id) = doc.attr(n, "id").filter(|i| !i.is_empty()) {
        for l in doc.descendants(Document::ROOT) {
            if doc.is_element(l, "label") && doc.attr(l, "for") == Some(id) {
                parts.push(content_text(page, l));
            }
        }
    }
    if let Some(l) = doc.ancestor_or_self(n, |a| doc.is_element(a, "label")) {
        // The label's text without the control's own value.
        let mut s = String::new();
        for c in doc.children(l) {
            if c == n {
                continue;
            }
            if let Some(t) = doc.text(c) {
                s.push_str(t);
            } else if doc.element(c).is_some() && !matches!(doc.tag(c), Some("input" | "select" | "textarea")) && crate::ax::displayed(page, c) {
                s.push(' ');
                s.push_str(&content_text(page, c));
                s.push(' ');
            }
        }
        parts.push(collapse_ws(&s));
    }
    let s = collapse_ws(&parts.join(" "));
    (!s.is_empty()).then_some(s)
}

/// Accessible name of an element.
pub fn accessible_name(page: &Page, n: NodeId, role: &str) -> String {
    let doc = &page.doc;
    let Some(el) = doc.element(n) else { return String::new() };
    if let Some(ids) = el.attr("aria-labelledby") {
        let s = labelled_by(page, ids);
        if !s.is_empty() {
            return s;
        }
    }
    if let Some(l) = el.attr("aria-label") {
        let s = collapse_ws(l);
        if !s.is_empty() {
            return s;
        }
    }
    let tag = el.tag.as_str();
    let ty = el.attr("type").map(|t| t.to_ascii_lowercase()).unwrap_or_default();
    let native = match tag {
        "input" if matches!(ty.as_str(), "submit" | "reset" | "button") => {
            let v = el.attr("value").map(collapse_ws).unwrap_or_default();
            if v.is_empty() {
                match ty.as_str() {
                    "submit" => "Submit".to_string(),
                    "reset" => "Reset".to_string(),
                    _ => String::new(),
                }
            } else {
                v
            }
        }
        "input" if ty == "image" => el.attr("alt").or(el.attr("value")).map(collapse_ws).unwrap_or_else(|| "Submit".into()),
        "input" | "select" | "textarea" => label_text(page, n).unwrap_or_default(),
        "img" | "area" => el.attr("alt").map(collapse_ws).unwrap_or_default(),
        "fieldset" => doc.child_elements(n).find(|&c| doc.is_element(c, "legend")).map(|l| content_text(page, l)).unwrap_or_default(),
        "table" => doc.child_elements(n).find(|&c| doc.is_element(c, "caption")).map(|l| content_text(page, l)).unwrap_or_default(),
        "figure" => doc.child_elements(n).find(|&c| doc.is_element(c, "figcaption")).map(|l| content_text(page, l)).unwrap_or_default(),
        "details" => doc.child_elements(n).find(|&c| doc.is_element(c, "summary")).map(|l| content_text(page, l)).unwrap_or_default(),
        "iframe" => String::new(),
        _ => {
            let from_content = matches!(
                role,
                "link" | "button" | "heading" | "tab" | "menuitem" | "menuitemcheckbox" | "menuitemradio" | "option" | "treeitem" | "checkbox" | "radio" | "switch" | "cell" | "row" | "listitem"
            ) || role == "clickable";
            if from_content {
                content_text(page, n)
            } else {
                String::new()
            }
        }
    };
    if !native.is_empty() {
        return native;
    }
    if let Some(t) = el.attr("title") {
        let s = collapse_ws(t);
        if !s.is_empty() {
            return s;
        }
    }
    if matches!(tag, "input" | "textarea") {
        if let Some(p) = el.attr("placeholder").or(el.attr("aria-placeholder")) {
            return collapse_ws(p);
        }
    }
    // Icon links/buttons: a tooltip or label on the icon inside.
    if matches!(role, "link" | "button" | "clickable") {
        let doc = &page.doc;
        if let Some(t) = doc.descendants(n).skip(1).find_map(|d| doc.attr(d, "title").or(doc.attr(d, "aria-label")).filter(|t| !t.trim().is_empty())) {
            return collapse_ws(t);
        }
    }
    // Links wrapping only an image without alt: use the file name as a
    // last resort so the agent can tell them apart.
    if role == "link" {
        if let Some(h) = el.attr("href") {
            let t = h.trim_end_matches('/');
            let last = t.rsplit('/').next().unwrap_or(t);
            let last = if last.is_empty() || last.contains(':') { h } else { last };
            return format!("(link to {})", clip_text(last, 40));
        }
    }
    String::new()
}

fn is_disabled(doc: &Document, n: NodeId) -> bool {
    if doc.attr(n, "aria-disabled") == Some("true") {
        return true;
    }
    if !matches!(doc.tag(n), Some("button" | "input" | "select" | "textarea" | "option" | "fieldset")) {
        return false;
    }
    if doc.attr(n, "disabled").is_some() {
        return true;
    }
    let mut cur = doc.parent(n);
    let mut child = n;
    while let Some(p) = cur {
        if doc.is_element(p, "fieldset") && doc.attr(p, "disabled").is_some() {
            // Controls inside the first legend stay enabled.
            let first_legend = doc.child_elements(p).find(|&c| doc.is_element(c, "legend"));
            if first_legend != Some(child) {
                return true;
            }
        }
        child = p;
        cur = doc.parent(p);
    }
    false
}

/// Inline states of a control/item.
pub fn states_of(page: &Page, n: NodeId, role: &str, invalid: bool) -> Vec<String> {
    let doc = &page.doc;
    let mut s = Vec::new();
    let attr = |a: &str| doc.attr(n, a);
    let kind = page.control_kind(n);
    let is_input = matches!(doc.tag(n), Some("input" | "select" | "textarea"));
    // Value first (most useful).
    match role {
        "textbox" | "searchbox" | "spinbutton" | "combobox" | "slider" if is_input => {
            if doc.tag(n) == Some("select") {
                let opts = page.select_options(n);
                let sel = opts.iter().find(|o| o.2).or(opts.first());
                if let Some(o) = sel {
                    s.push(format!("value=\"{}\"", esc(&clip_text(&o.0, 60))));
                }
                if opts.len() <= 6 {
                    let list: Vec<String> = opts.iter().map(|o| esc(&clip_text(&o.0, 30))).collect();
                    s.push(format!("options=[{}]", list.join("|")));
                } else {
                    s.push(format!("{} options", opts.len()));
                }
            } else {
                let v = page.control_text(n);
                if kind == ControlKind::Password {
                    if !v.is_empty() {
                        s.push(format!("value=({} chars hidden)", v.chars().count()));
                    }
                } else if !v.is_empty() {
                    s.push(format!("value=\"{}\"", esc(&clip_text(&collapse_ws(&v), 80))));
                }
            }
        }
        "listbox" if doc.tag(n) == Some("select") => {
            let opts = page.select_options(n);
            let sel: Vec<String> = opts.iter().filter(|o| o.2).map(|o| esc(&clip_text(&o.0, 30))).collect();
            s.push(format!("selected=[{}]", sel.join("|")));
            s.push(format!("{} options", opts.len()));
        }
        "progressbar" | "meter" => {
            if let Some(v) = attr("value").or(attr("aria-valuenow")) {
                s.push(format!("value={}", esc(v)));
            }
        }
        _ => {
            if let Some(v) = attr("aria-valuetext").or(attr("aria-valuenow")) {
                s.push(format!("value=\"{}\"", esc(&clip_text(v, 40))));
            }
        }
    }
    if kind == ControlKind::Password && is_input {
        s.push("password".into());
    } else if doc.tag(n) == Some("input") {
        let ty = attr("type").unwrap_or("").to_ascii_lowercase();
        if matches!(ty.as_str(), "email" | "url" | "tel" | "date" | "time" | "datetime-local" | "month" | "week" | "color" | "file") {
            s.push(format!("type={ty}"));
        }
    }
    if matches!(role, "checkbox" | "radio" | "switch" | "menuitemcheckbox" | "menuitemradio") {
        let checked = if is_input { attr("checked").is_some() } else { attr("aria-checked") == Some("true") };
        if attr("aria-checked") == Some("mixed") {
            s.push("mixed".into());
        } else if checked {
            s.push("checked".into());
        } else {
            s.push("unchecked".into());
        }
    }
    if let Some(p) = attr("aria-pressed") {
        if p == "true" {
            s.push("pressed".into());
        }
    }
    match attr("aria-expanded") {
        Some("true") => s.push("expanded".into()),
        Some("false") => s.push("collapsed".into()),
        _ => {
            if doc.tag(n) == Some("summary") {
                let open = doc.parent(n).map(|d| doc.attr(d, "open").is_some()).unwrap_or(false);
                s.push(if open { "expanded" } else { "collapsed" }.into());
            }
        }
    }
    if attr("aria-selected") == Some("true") || (doc.tag(n) == Some("option") && attr("selected").is_some()) {
        s.push("selected".into());
    }
    if attr("aria-current").is_some_and(|v| v != "false") {
        s.push("current".into());
    }
    if is_disabled(doc, n) {
        s.push("disabled".into());
    }
    if attr("required").is_some() || attr("aria-required") == Some("true") {
        s.push("required".into());
    }
    if attr("readonly").is_some() || attr("aria-readonly") == Some("true") {
        s.push("readonly".into());
    }
    if invalid || attr("aria-invalid").is_some_and(|v| v != "false") {
        s.push("invalid".into());
    }
    if page.state.focused == Some(n) {
        s.push("focused".into());
    }
    s
}

/// Escape a page string for display inside double quotes: no quotes,
/// newlines or control characters can break out of it.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push('\''),
            '\\' => out.push('/'),
            '\n' | '\r' | '\t' => out.push(' '),
            c if c.is_control() => {}
            // Bidi overrides could visually reorder the output.
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => {}
            c => out.push(c),
        }
    }
    out
}

impl<'a> Builder<'a> {
    fn frame_root(&mut self, frame: usize, depth: usize) -> Ax {
        let page = self.frames[frame].page;
        let doc = &page.doc;
        let mut root = Ax::new(frame, Document::ROOT, Kind::Group, "document");
        root.name = page.title.trim().to_string();
        let start = doc.body().or_else(|| doc.html_element()).unwrap_or(Document::ROOT);
        let bg = if page.background.a > 0 { blend(page.background, Color::WHITE) } else { Color::WHITE };
        let ctx = Ctx { opacity: 1.0, background: bg, has_bg_image: false, in_section: false, in_control: false };
        let mut out = Vec::new();
        if depth < 4 {
            self.children(frame, start, ctx, &mut out);
        }
        root.children = finish(out);
        root
    }

    fn geo(&self, frame: usize, n: NodeId) -> Option<Geo> {
        self.geos[frame].get(n).copied()
    }

    /// Why an element's own painting is invisible, if it is.
    fn hidden_reason(&self, frame: usize, n: NodeId, style: &ComputedStyle, ctx: &Ctx, text: bool) -> Option<&'static str> {
        if style.visibility == Visibility::Hidden {
            return Some("visibility:hidden");
        }
        if ctx.opacity * style.opacity < 0.05 {
            return Some("opacity:0");
        }
        let page = self.frames[frame].page;
        match self.geo(frame, n) {
            None => {
                // No box at all (e.g. display:contents or not laid out).
                if style.display != Display::Contents {
                    return Some("no box");
                }
            }
            Some(g) => {
                if g.raw.w < 1.5 && g.raw.h < 1.5 && text {
                    return Some("zero size");
                }
                if g.rect.w <= 0.5 || g.rect.h <= 0.5 {
                    if g.raw.w > 0.5 && g.raw.h > 0.5 {
                        return Some("clipped");
                    }
                    if text {
                        return Some("zero size");
                    }
                }
                let doc_w = page.content_width().max(page.viewport.width);
                if g.rect.right() <= 0.0 || g.rect.bottom() <= 0.0 || g.rect.x >= doc_w + 1.0 {
                    return Some("off-screen");
                }
            }
        }
        if text {
            if style.font_size < 3.0 {
                return Some("tiny font");
            }
            if !ctx.has_bg_image && style.background_image.is_none() {
                let fg = blend(style.color, ctx.background);
                if style.color.a < 13 || contrast(fg, ctx.background) < 1.12 {
                    return Some("same color as background");
                }
            }
        }
        None
    }

    fn children(&mut self, frame: usize, n: NodeId, ctx: Ctx, out: &mut Vec<Ax>) {
        let page = self.frames[frame].page;
        let doc = &page.doc;
        let kids: Vec<NodeId> = doc.children(n).collect();
        for c in kids {
            if let Some(t) = doc.text(c) {
                let text = collapse_ws(t);
                if text.is_empty() || ctx.in_control {
                    continue;
                }
                self.push_text(frame, n, &text, ctx, out);
                continue;
            }
            if doc.element(c).is_some() {
                self.element(frame, c, ctx, out);
            }
        }
    }

    fn push_text(&mut self, frame: usize, parent: NodeId, text: &str, ctx: Ctx, out: &mut Vec<Ax>) {
        // Bare separators ("|", "·", "/") carry no meaning for an agent.
        if text.chars().count() <= 2 && text.chars().all(|c| matches!(c, '|' | '·' | '•' | '/' | '-' | '–' | '—' | ',' | ':' | ';' | '(' | ')' | '[' | ']')) {
            return;
        }
        let page = self.frames[frame].page;
        let Some(style) = page.styles.get(parent) else { return };
        let hidden = self.hidden_reason(frame, parent, style, &ctx, true);
        if hidden.is_some() {
            self.stats.hidden_text_chars += text.len();
            if !self.opts.include_hidden {
                return;
            }
        }
        // Join with the previous text item when nothing separates them.
        if let Some(last) = out.last_mut() {
            if last.kind == Kind::Text && last.hidden == hidden {
                last.name.push(' ');
                last.name.push_str(text);
                if let (Some(a), Some(g)) = (last.rect, self.geo(frame, parent)) {
                    last.rect = Some(union(a, g.rect));
                }
                return;
            }
        }
        let mut a = Ax::new(frame, parent, Kind::Text, "text");
        a.name = text.to_string();
        a.hidden = hidden;
        if let Some(g) = self.geo(frame, parent) {
            a.rect = Some(g.rect);
            a.fixed = g.fixed;
        }
        a.ordinal = out.iter().filter(|x| x.kind == Kind::Text && x.node == parent).count() as u16;
        out.push(a);
    }

    fn element(&mut self, frame: usize, n: NodeId, ctx: Ctx, out: &mut Vec<Ax>) {
        let page = self.frames[frame].page;
        let doc = &page.doc;
        let Some(el) = doc.element(n) else { return };
        let tag = el.tag.as_str();
        if matches!(tag, "script" | "style" | "noscript" | "head" | "meta" | "link" | "title" | "base") {
            return;
        }
        // Declarative shadow roots: flatten their content into the host.
        if tag == "template" {
            if el.attr("shadowrootmode").is_some() || el.attr("shadowroot").is_some() {
                let mut tmp = Vec::new();
                self.children_unstyled(frame, n, &mut tmp);
                out.extend(tmp);
            }
            return;
        }
        if el.attr("aria-hidden") == Some("true") || el.attr("hidden").is_some() && !crate::ax::displayed(page, n) {
            if self.opts.include_hidden {
                self.stats.hidden_items += 1;
            } else {
                self.stats.hidden_text_chars += doc.text_content(n).trim().len();
                return;
            }
        }
        let Some(style) = page.styles.get(n).cloned().filter(|s| s.display != Display::None) else {
            // display:none: count what is dropped.
            let t = doc.text_content(n);
            self.stats.hidden_text_chars += t.trim().len();
            return;
        };
        let mut ctx = ctx;
        ctx.opacity *= style.opacity;
        if style.background_color.a > 128 {
            ctx.background = blend(style.background_color, ctx.background);
        }
        if style.background_image.is_some() || !style.background_layers.is_empty() {
            ctx.has_bg_image = true;
        }
        let explicit = aria_role(doc, n);
        let mut role: Option<String> = match explicit.as_deref() {
            Some("presentation" | "none") => None,
            Some(r) if kind_for_role(r).is_some() => Some(r.to_string()),
            _ => implicit_role(doc, n, &ctx).map(|s| s.to_string()),
        };
        if frame == 0 {
            let generic = role.as_deref().map(|r| kind_for_role(r).map(|k| matches!(k, Kind::Group | Kind::Landmark | Kind::Item | Kind::List)).unwrap_or(true)).unwrap_or(true);
            if generic && self.opts.overlays.contains(&n) {
                role = Some("dialog".into());
            } else if role.is_none() && self.opts.main == Some(n) {
                role = Some("main".into());
            }
        }
        // Elements scripts made clickable (click listeners / onclick).
        let role = role.or_else(|| {
            let clickable = el.attr("onclick").is_some()
                || page.script.host.as_ref().map(|h| h.has_listeners(n, "click")).unwrap_or(false);
            // Wrappers with real controls inside are containers, not buttons.
            let wraps_controls = || {
                doc.descendants(n).skip(1).any(|d| matches!(doc.tag(d), Some("a" | "button" | "input" | "select" | "textarea")))
            };
            (clickable && !ctx.in_control && matches!(tag, "div" | "span" | "li" | "td" | "img" | "svg" | "i" | "p" | "label") && !wraps_controls()).then(|| "clickable".to_string())
        });
        let block = is_block(style.display);
        let kind = role.as_deref().and_then(|r| if r == "clickable" { Some(Kind::Control) } else { kind_for_role(r) });
        let sectioning = matches!(tag, "article" | "aside" | "main" | "nav" | "section");
        let mut child_ctx = ctx;
        if sectioning {
            child_ctx.in_section = true;
        }
        // A label's text is the name of its control: do not repeat it.
        if tag == "label" && rabbithole_engine::pointer::labeled_control(doc, n).is_some_and(|c| crate::ax::displayed(page, c)) {
            child_ctx.in_control = true;
        }
        let Some(kind) = kind else {
            // Generic container: flatten, keeping block boundaries and
            // detecting runs of repeated siblings (feeds, cards).
            if block {
                out.push(Ax::new(frame, n, Kind::Break, ""));
            }
            let start = out.len();
            self.children(frame, n, child_ctx, out);
            if block {
                out.push(Ax::new(frame, n, Kind::Break, ""));
                self.maybe_list(frame, n, start, out);
            }
            return;
        };
        let role = role.unwrap_or_default();
        let invalid = self.opts.invalid.contains(&(frame, n));
        let mut a = Ax::new(frame, n, kind, &role);
        if let Some(g) = self.geo(frame, n) {
            a.rect = Some(g.rect);
            a.fixed = g.fixed || style.position == Position::Fixed;
        } else if style.position == Position::Fixed {
            a.fixed = true;
        }
        match kind {
            Kind::Control => {
                let hidden = self.hidden_reason(frame, n, &style, &ctx, false);
                if hidden.is_some() {
                    self.stats.hidden_items += 1;
                    if !self.opts.include_hidden {
                        return;
                    }
                }
                a.hidden = hidden;
                a.name = clip_text(&accessible_name(page, n, &role), 100);
                a.states = states_of(page, n, &role, invalid);
                if role == "link" {
                    a.href = page.link_of(n).map(|u| u.to_string());
                }
                // A link/button whose content is a heading: report the
                // heading level too.
                if let Some(h) = doc.descendants(n).find(|&d| matches!(doc.tag(d), Some("h1" | "h2" | "h3" | "h4" | "h5" | "h6"))) {
                    a.level = doc.tag(h).and_then(|t| t[1..].parse().ok()).unwrap_or(0);
                }
                a.reference = Some(self.refs.assign(frame, n, &role, &a.name));
                // Controls do not expose their content as separate items,
                // except selects (options are listed in the states) and
                // composite widgets that contain other controls.
                if matches!(tag, "a" | "button" | "summary") || role == "clickable" || role == "link" || role == "button" {
                    let has_inner_controls = doc.descendants(n).skip(1).any(|d| {
                        matches!(doc.tag(d), Some("input" | "select" | "textarea" | "button")) || (doc.is_element(d, "a") && doc.attr(d, "href").is_some())
                    });
                    if has_inner_controls {
                        let mut inner = Vec::new();
                        self.children(frame, n, child_ctx, &mut inner);
                        a.children = finish(inner);
                    }
                }
                out.push(a);
            }
            Kind::Heading => {
                let level = explicit
                    .is_some()
                    .then(|| el.attr("aria-level").and_then(|l| l.parse().ok()))
                    .flatten()
                    .or_else(|| tag.strip_prefix('h').and_then(|l| l.parse().ok()))
                    .unwrap_or(2u8);
                a.level = level;
                if self.hidden_reason(frame, n, &style, &ctx, true).is_some() && !self.opts.include_hidden {
                    self.stats.hidden_items += 1;
                    return;
                }
                a.name = clip_text(&accessible_name(page, n, "heading"), 160);
                // Controls inside the heading (a title link) are kept.
                let mut inner = Vec::new();
                let mut ictx = child_ctx;
                ictx.in_control = true;
                self.children(frame, n, ictx, &mut inner);
                a.children = finish(inner).into_iter().filter(|c| c.kind == Kind::Control).collect();
                out.push(Ax::new(frame, n, Kind::Break, ""));
                out.push(a);
                out.push(Ax::new(frame, n, Kind::Break, ""));
            }
            Kind::Image => {
                let name = accessible_name(page, n, &role);
                let mut inner = Vec::new();
                if tag != "img" && tag != "svg" {
                    self.children(frame, n, child_ctx, &mut inner);
                }
                if name.is_empty() && inner.is_empty() {
                    return;
                }
                if !name.is_empty() && (tag == "img" || tag == "svg") {
                    let hidden = self.hidden_reason(frame, n, &style, &ctx, false);
                    if hidden.is_some() && !self.opts.include_hidden {
                        return;
                    }
                    a.hidden = hidden;
                    a.name = clip_text(&name, 100);
                    out.push(a);
                } else {
                    // A figure: its caption/content as items.
                    out.push(Ax::new(frame, n, Kind::Break, ""));
                    out.extend(inner);
                    out.push(Ax::new(frame, n, Kind::Break, ""));
                }
            }
            Kind::Frame => {
                let child = self.frames[frame].children.iter().find(|(e, _)| *e == n).map(|(_, f)| *f);
                a.name = accessible_name(page, n, "iframe");
                if a.name.is_empty() {
                    a.name = el.attr("title").or(el.attr("name")).map(collapse_ws).unwrap_or_default();
                }
                if let Some(src) = el.attr("src") {
                    a.href = page.resolve(src).map(|u| u.to_string());
                }
                a.reference = Some(self.refs.assign(frame, n, "iframe", &a.name));
                if let Some(f) = child {
                    let root = self.frame_root(f, frame + 1);
                    if a.name.is_empty() {
                        a.name = root.name.clone();
                    }
                    a.children = root.children;
                }
                out.push(a);
            }
            _ => {
                // Containers: landmark, dialog, form, list, item, group.
                if kind == Kind::Dialog {
                    let open = tag != "dialog" || el.attr("open").is_some();
                    if !open {
                        return;
                    }
                }
                a.name = clip_text(&accessible_name(page, n, &role), 100);
                if matches!(kind, Kind::Landmark | Kind::Dialog | Kind::Form) || (kind == Kind::Group && !a.name.is_empty()) {
                    a.reference = Some(self.refs.assign(frame, n, &role, &a.name));
                }
                if kind == Kind::Group {
                    a.states = states_of(page, n, &role, invalid);
                }
                let mut inner = Vec::new();
                self.children(frame, n, child_ctx, &mut inner);
                a.children = finish(inner);
                if a.children.is_empty() && !matches!(kind, Kind::Dialog | Kind::Form) {
                    return;
                }
                if block || matches!(kind, Kind::Landmark | Kind::Dialog | Kind::List | Kind::Item | Kind::Form) {
                    out.push(Ax::new(frame, n, Kind::Break, ""));
                }
                out.push(a);
                if block || matches!(kind, Kind::Landmark | Kind::Dialog | Kind::List | Kind::Item | Kind::Form) {
                    out.push(Ax::new(frame, n, Kind::Break, ""));
                }
            }
        }
    }

    /// Content of a declarative shadow root (no computed styles): text and
    /// controls only.
    fn children_unstyled(&mut self, frame: usize, n: NodeId, out: &mut Vec<Ax>) {
        let page = self.frames[frame].page;
        let doc = &page.doc;
        for c in doc.children(n).collect::<Vec<_>>() {
            if let Some(t) = doc.text(c) {
                let t = collapse_ws(t);
                if !t.is_empty() {
                    let mut a = Ax::new(frame, n, Kind::Text, "text");
                    a.name = t;
                    a.ordinal = out.iter().filter(|x| x.kind == Kind::Text && x.node == n).count() as u16;
                    out.push(a);
                }
                continue;
            }
            let Some(el) = doc.element(c) else { continue };
            if matches!(el.tag.as_str(), "script" | "style") {
                continue;
            }
            let ctx = Ctx { opacity: 1.0, background: Color::WHITE, has_bg_image: true, in_section: true, in_control: false };
            match implicit_role(doc, c, &ctx).or(aria_role(doc, c).as_deref().and_then(|r| CONTROL_ROLES.iter().find(|x| **x == r).copied())) {
                Some(role) if kind_for_role(role) == Some(Kind::Control) || role == "heading" => {
                    let kind = if role == "heading" { Kind::Heading } else { Kind::Control };
                    let mut a = Ax::new(frame, c, kind, role);
                    a.name = clip_text(&collapse_ws(&accessible_name_unstyled(doc, c)), 100);
                    if kind == Kind::Heading {
                        a.level = el.tag[1..].parse().unwrap_or(2);
                    } else {
                        a.reference = Some(self.refs.assign(frame, c, role, &a.name));
                        a.states = states_of(page, c, role, false);
                    }
                    out.push(a);
                }
                _ => self.children_unstyled(frame, c, out),
            }
        }
    }

    /// A generic container whose children are many similar siblings
    /// (cards, feed entries): wrap their items in a List.
    fn maybe_list(&mut self, frame: usize, n: NodeId, start: usize, out: &mut Vec<Ax>) {
        let page = self.frames[frame].page;
        let doc = &page.doc;
        let kids: Vec<NodeId> = doc.child_elements(n).filter(|&c| crate::ax::displayed(page, c)).collect();
        if kids.len() < 6 {
            return;
        }
        let sig = |c: NodeId| {
            let mut classes: Vec<&str> = doc.attr(c, "class").unwrap_or("").split_whitespace().collect();
            classes.sort();
            format!("{}.{}", doc.tag(c).unwrap_or(""), classes.join("."))
        };
        let mut counts: HashMap<String, usize> = HashMap::new();
        for &c in &kids {
            *counts.entry(sig(c)).or_default() += 1;
        }
        let Some((best, count)) = counts.into_iter().max_by_key(|(_, c)| *c) else { return };
        if count < 6 || count * 10 < kids.len() * 7 {
            return;
        }
        // Group the produced items by which child they came from.
        let items: Vec<Ax> = out.drain(start..).collect();
        let mut list = Ax::new(frame, n, Kind::List, "list");
        let mut current: Option<(NodeId, Vec<Ax>)> = None;
        let mut groups: Vec<(NodeId, Vec<Ax>)> = Vec::new();
        for it in items {
            let owner = kids.iter().copied().find(|&k| k == it.node || doc.is_ancestor(k, it.node));
            match (owner, &mut current) {
                (Some(o), Some((co, v))) if *co == o => v.push(it),
                (Some(o), _) => {
                    if let Some(c) = current.take() {
                        groups.push(c);
                    }
                    current = Some((o, vec![it]));
                }
                (None, Some((_, v))) => v.push(it),
                (None, None) => {}
            }
        }
        if let Some(c) = current.take() {
            groups.push(c);
        }
        for (owner, v) in groups {
            let v = finish(v);
            if v.is_empty() {
                continue;
            }
            let mut item = Ax::new(frame, owner, Kind::Item, if sig(owner) == best { "listitem" } else { "group" });
            item.rect = self.geo(frame, owner).map(|g| g.rect);
            item.children = v;
            list.children.push(item);
        }
        if list.children.is_empty() {
            return;
        }
        list.rect = self.geo(frame, n).map(|g| g.rect);
        out.push(Ax::new(frame, n, Kind::Break, ""));
        out.push(list);
        out.push(Ax::new(frame, n, Kind::Break, ""));
    }
}

fn accessible_name_unstyled(doc: &Document, n: NodeId) -> String {
    doc.attr(n, "aria-label")
        .map(|s| s.to_string())
        .or_else(|| doc.attr(n, "value").filter(|_| doc.is_element(n, "input")).map(|s| s.to_string()))
        .or_else(|| doc.attr(n, "placeholder").map(|s| s.to_string()))
        .unwrap_or_else(|| doc.text_content(n))
}

/// Clean a child list: drop leading/trailing/double breaks.
fn finish(v: Vec<Ax>) -> Vec<Ax> {
    let v = merge_prose(v);
    let mut out: Vec<Ax> = Vec::with_capacity(v.len());
    for a in v {
        if a.kind == Kind::Break && (out.is_empty() || out.last().map(|l| l.kind == Kind::Break).unwrap_or(false)) {
            continue;
        }
        out.push(a);
    }
    while out.last().map(|l| l.kind == Kind::Break).unwrap_or(false) {
        out.pop();
    }
    out
}

fn is_inline_link(a: &Ax) -> bool {
    a.kind == Kind::Control && a.role == "link" && a.children.is_empty() && a.hidden.is_none() && a.reference.is_some()
}

/// Prose with inline links becomes one text item with `[link]@eN`
/// markers (the links stay as children for refs, marks and diffs), which
/// is much shorter than alternating text/link items.
fn merge_prose(v: Vec<Ax>) -> Vec<Ax> {
    let mut out: Vec<Ax> = Vec::with_capacity(v.len());
    let mut run: Vec<Ax> = Vec::new();
    fn flush(run: &mut Vec<Ax>, out: &mut Vec<Ax>) {
        let texts: usize = run.iter().filter(|a| a.kind == Kind::Text).map(|a| a.name.chars().count()).sum();
        let links = run.iter().filter(|a| is_inline_link(a)).count();
        let has_text = run.iter().any(|a| a.kind == Kind::Text);
        if links == 0 || !has_text || texts < 30 || run.len() < 2 {
            out.append(run);
            return;
        }
        let first = run.iter().position(|a| a.kind == Kind::Text).unwrap_or(0);
        let mut merged = run[first].clone();
        merged.children.clear();
        merged.name.clear();
        let mut rect = None;
        for a in run.drain(..) {
            if !merged.name.is_empty() && !a.name.starts_with([',', '.', ';', ':', ')', '!', '?']) {
                merged.name.push(' ');
            }
            if a.kind == Kind::Text {
                merged.name.push_str(&a.name);
            } else {
                merged.name.push_str(&format!("[{}]@e{}", a.name, a.reference.unwrap_or(0)));
                merged.children.push(a.clone());
            }
            rect = match (rect, a.rect) {
                (None, r) => r,
                (Some(x), Some(r)) => Some(union(x, r)),
                (x, None) => x,
            };
        }
        merged.rect = rect;
        out.push(merged);
    }
    for a in v {
        let inline = (a.kind == Kind::Text && a.hidden.is_none()) || is_inline_link(&a);
        if inline {
            run.push(a);
        } else {
            flush(&mut run, &mut out);
            out.push(a);
        }
    }
    flush(&mut run, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rabbithole_engine::gfx::Color;

    #[test]
    fn contrast_ratio() {
        assert!(contrast(Color::WHITE, Color::BLACK) > 20.0);
        assert!(contrast(Color::WHITE, Color::rgb(254, 254, 254)) < 1.05);
    }

    #[test]
    fn escaping() {
        assert_eq!(esc("a\"b\nc\u{202e}d"), "a'b cd");
        assert_eq!(clip_text("hello world", 6), "hello…");
    }
}
