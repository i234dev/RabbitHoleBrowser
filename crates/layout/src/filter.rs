//! `filter` and `backdrop-filter` functions (Filter Effects 1 §5) applied
//! to premultiplied RGBA pixel buffers (components in 0..=1): shared with
//! the SVG renderer, which also evaluates `url()` filters.

pub(crate) use rabbithole_svg::filter::css::{apply, from_black_white, from_opaque, spread, to_rgba, Pixels};
use rabbithole_gfx::Rect;
use rabbithole_style::FilterOp;
use rabbithole_svg::filter::{FilterDef, FilterItem};
use std::sync::Arc;

/// The SVG `<filter>` elements a box's `filter` list references.
pub struct UrlFilters {
    /// id → (filter, the SVG document holding it).
    defs: Vec<(String, Arc<FilterDef>, Arc<rabbithole_svg::SvgTree>)>,
    /// The border box (layout coordinates): its top-left corner is the
    /// origin of the filter's user space.
    pub rect: Rect,
    /// Size percentages of user-space lengths refer to.
    pub viewport: (f32, f32),
}

impl std::fmt::Debug for UrlFilters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UrlFilters").field("ids", &self.defs.iter().map(|d| &d.0).collect::<Vec<_>>()).field("rect", &self.rect).finish()
    }
}

impl UrlFilters {
    fn def(&self, id: &str) -> Option<Arc<FilterDef>> {
        self.defs.iter().find(|d| d.0 == id).map(|d| d.1.clone())
    }

    /// The bounds (layout coordinates) the filtered output can cover, from
    /// the bounds `painted` of the unfiltered painting.
    pub fn bounds(&self, ops: &[FilterOp], painted: Rect) -> Rect {
        let items = items(ops);
        let bbox = Rect::new(0.0, 0.0, self.rect.w, self.rect.h);
        let p = painted.translate(-self.rect.x, -self.rect.y);
        rabbithole_svg::filter::chain_bounds(&items, &|id| self.def(id), bbox, p, self.viewport).translate(self.rect.x, self.rect.y)
    }

    /// Run the chain on `src`: `m` maps layout coordinates to the buffer's
    /// pixels and `scale` CSS px to pixels.
    pub fn apply(&self, ops: &[FilterOp], src: Pixels, m: &rabbithole_gfx::Matrix, scale: f32, fonts: &rabbithole_gfx::FontContext) -> Pixels {
        let items = items(ops);
        let um = rabbithole_style::multiply_matrix(*m, [1.0, 0.0, 0.0, 1.0, self.rect.x, self.rect.y]);
        let element = |id: &str, mm: &rabbithole_gfx::Matrix, w: usize, h: usize| self.defs.iter().find_map(|d| d.2.render_element(fonts, id, mm, w, h));
        let env = rabbithole_svg::filter::FilterEnv { m: um, bbox: Rect::new(0.0, 0.0, self.rect.w, self.rect.h), viewport: self.viewport, element: Some(&element), fonts: Some(fonts) };
        let (w, h) = (src.w, src.h);
        rabbithole_svg::filter::apply_chain(&items, &|id| self.def(id), src, &env, scale).unwrap_or_else(|| Pixels::new(w, h))
    }
}

fn items(ops: &[FilterOp]) -> Vec<FilterItem> {
    ops.iter()
        .map(|o| match o {
            FilterOp::Url(id) => FilterItem::Url(id.clone()),
            other => FilterItem::Css(other.clone()),
        })
        .collect()
}

/// Resolve the `url()` entries of a box's filter list: `None` without
/// any, `Some(None)` when one does not name a `<filter>` element (the
/// whole list is then ignored).
pub(crate) fn resolve_urls(b: &crate::tree::LayoutBox, ops: &[FilterOp], ctx: &crate::LayoutContext) -> Option<Option<Arc<UrlFilters>>> {
    if !ops.iter().any(|o| matches!(o, FilterOp::Url(_))) {
        return None;
    }
    let doc = ctx.doc;
    let styles = ctx.styles;
    let style_of = |n: rabbithole_dom::NodeId, prop: &str| -> Option<String> {
        let s = styles.get(n)?;
        match prop {
            "display" if s.display.is_none() => Some("none".into()),
            "color" => Some(format!("rgba({}, {}, {}, {})", s.color.r, s.color.g, s.color.b, s.color.a as f32 / 255.0)),
            _ => s.svg_props.iter().find(|(p, _)| p == prop).map(|(_, v)| rabbithole_svg::tokens_to_string(v).trim().to_string()),
        }
    };
    let mut defs: Vec<(String, Arc<FilterDef>, Arc<rabbithole_svg::SvgTree>)> = Vec::new();
    for o in ops {
        let FilterOp::Url(id) = o else { continue };
        if defs.iter().any(|d| &d.0 == id) {
            continue;
        }
        let found = (|| {
            let node = doc.element_by_id(id)?;
            if !doc.tag(node)?.eq_ignore_ascii_case("filter") {
                return None;
            }
            let svg = doc.ancestor_or_self(node, |a| doc.is_element(a, "svg"))?;
            let tree = Arc::new(rabbithole_svg::SvgTree::from_dom(doc, svg, &style_of)?);
            let def = tree.filter(id)?;
            Some((id.clone(), def, tree))
        })();
        match found {
            Some(d) => defs.push(d),
            None => return Some(None),
        }
    }
    let rect = b.rect;
    Some(Some(Arc::new(UrlFilters { defs, rect, viewport: (ctx.viewport_width, ctx.viewport_height) })))
}
