//! Overlays (modals, cookie banners) found with real hit testing, and the
//! "is this element actually clickable here?" check used by actions.

use rabbithole_engine::dom::{Document, NodeId};
use rabbithole_engine::gfx::Rect;
use rabbithole_engine::style::Position;
use rabbithole_engine::Page;

use crate::ax::GeoMap;

/// A fixed/raised box drawn over the page.
#[derive(Debug, Clone)]
pub struct Overlay {
    pub node: NodeId,
    /// Fraction of viewport sample points where it is the topmost thing.
    pub covers: f32,
    pub cookie: bool,
    pub dialog: bool,
}

impl Overlay {
    pub fn note(&self) -> String {
        if self.covers >= 0.3 {
            format!("covers the page ({}% of the viewport)", (self.covers * 100.0).round())
        } else if self.cookie {
            "cookie banner (dismiss: click ref=\"cookies\" rejects)".to_string()
        } else {
            format!("overlay over {}% of the viewport", (self.covers * 100.0).round())
        }
    }
}

const COOKIE_WORDS: &[&str] = &["cookie", "consent", "gdpr", "rgpd", "privacidad", "datenschutz", "privacy"];

/// Does the text look like a cookie/consent banner?
pub fn is_cookie_text(t: &str) -> bool {
    let t = t.to_lowercase();
    COOKIE_WORDS.iter().any(|w| t.contains(w))
}

fn is_dialog(doc: &Document, n: NodeId) -> bool {
    matches!(doc.attr(n, "role"), Some("dialog" | "alertdialog")) || doc.attr(n, "aria-modal") == Some("true") || (doc.is_element(n, "dialog") && doc.attr(n, "open").is_some())
}

/// Viewport-coordinate rect of an element (fixed boxes are laid out
/// relative to the viewport).
pub fn viewport_rect(page: &Page, geo: &GeoMap, n: NodeId) -> Option<Rect> {
    let g = geo.get(n)?;
    Some(if g.fixed { g.rect } else { g.rect.translate(-page.page_scroll.0, -page.page_scroll.1) })
}

/// Find overlays of the main frame.
pub fn detect(page: &Page, geo: &GeoMap) -> Vec<Overlay> {
    let doc = &page.doc;
    let vw = page.viewport.width;
    let vh = page.viewport.height;
    let mut cands: Vec<NodeId> = Vec::new();
    for n in doc.descendants(Document::ROOT) {
        let Some(st) = page.styles.get(n) else { continue };
        if doc.element(n).is_none() {
            continue;
        }
        let raised = st.position == Position::Fixed || (st.position == Position::Absolute && st.z_index.unwrap_or(0) >= 100) || (st.position == Position::Sticky && false);
        if !(raised || is_dialog(doc, n)) {
            continue;
        }
        if cands.iter().any(|&c| doc.is_ancestor(c, n)) {
            continue;
        }
        let Some(r) = viewport_rect(page, geo, n) else { continue };
        let vis = r.intersect(&Rect::new(0.0, 0.0, vw, vh));
        if vis.is_empty() {
            continue;
        }
        cands.push(n);
    }
    if cands.is_empty() {
        return Vec::new();
    }
    // Sample a grid of viewport points.
    let mut hits = vec![0usize; cands.len()];
    let (gx, gy) = (8, 8);
    for i in 0..gx {
        for j in 0..gy {
            let x = (i as f32 + 0.5) * vw / gx as f32;
            let y = (j as f32 + 0.5) * vh / gy as f32;
            if let Some(h) = page.node_at_viewport(x, y) {
                if let Some(k) = cands.iter().position(|&c| c == h || doc.is_ancestor(c, h)) {
                    hits[k] += 1;
                }
            }
        }
    }
    let total = (gx * gy) as f32;
    let mut out = Vec::new();
    for (k, &n) in cands.iter().enumerate() {
        let covers = hits[k] as f32 / total;
        let text = doc.text_content(n);
        let cookie = is_cookie_text(&text) && text.len() < 4000;
        let dialog = is_dialog(doc, n);
        let r = viewport_rect(page, geo, n).unwrap_or_default();
        let area = r.w * r.h / (vw * vh);
        // A fixed header/toolbar is not an overlay; a banner with cookie
        // text, a dialog or something covering a big part of the view is.
        if cookie || dialog && covers > 0.0 || covers >= 0.25 || (area >= 0.3 && covers >= 0.15) {
            out.push(Overlay { node: n, covers, cookie, dialog });
        }
    }
    out
}

/// Result of checking an element before clicking it.
#[derive(Debug)]
pub enum Reach {
    /// The element (or a descendant) is topmost at this viewport point.
    Point(f32, f32),
    /// Only an ancestor is hit there (dispatch to the element directly).
    Ancestor(f32, f32),
    /// Something else is on top.
    Covered(NodeId),
    /// Not painted (no box / zero size).
    Invisible,
}

/// Can the element be reached by the pointer? Scrolls nothing; the caller
/// scrolls it into view first.
pub fn reach(page: &Page, geo: &GeoMap, target: NodeId) -> Reach {
    let doc = &page.doc;
    let Some(r) = viewport_rect(page, geo, target) else { return Reach::Invisible };
    if r.w < 1.0 || r.h < 1.0 {
        return Reach::Invisible;
    }
    let pts = [(0.5, 0.5), (0.25, 0.5), (0.75, 0.5), (0.5, 0.25), (0.5, 0.75), (0.1, 0.5), (0.9, 0.5)];
    let mut ancestor_pt = None;
    let mut cover = None;
    for (fx, fy) in pts {
        let x = r.x + r.w * fx;
        let y = r.y + r.h * fy;
        if x < 0.0 || y < 0.0 || x >= page.viewport.width || y >= page.viewport.height {
            continue;
        }
        match page.node_at_viewport(x, y) {
            Some(h) if h == target || doc.is_ancestor(target, h) => return Reach::Point(x, y),
            Some(h) if doc.is_ancestor(h, target) => {
                ancestor_pt.get_or_insert((x, y));
            }
            Some(h) => {
                cover.get_or_insert(h);
            }
            None => {}
        }
    }
    if let Some((x, y)) = ancestor_pt {
        // An ancestor without pointer-events:none above us still lets
        // the click through when nothing unrelated covers it.
        if cover.is_none() {
            return Reach::Ancestor(x, y);
        }
    }
    match cover {
        Some(c) => Reach::Covered(cover_root(page, c)),
        None => Reach::Ancestor(r.x + r.w / 2.0, r.y + r.h / 2.0),
    }
}

/// The outermost overlay-like ancestor of a covering node.
pub fn cover_root(page: &Page, n: NodeId) -> NodeId {
    let doc = &page.doc;
    let mut best = n;
    let mut cur = Some(n);
    while let Some(c) = cur {
        if c == Document::ROOT || doc.is_element(c, "body") || doc.is_element(c, "html") {
            break;
        }
        if let Some(st) = page.styles.get(c) {
            if st.position == Position::Fixed || st.position == Position::Absolute && st.z_index.unwrap_or(0) > 0 || is_dialog(doc, c) {
                best = c;
            }
        }
        cur = doc.parent(c);
    }
    best
}
