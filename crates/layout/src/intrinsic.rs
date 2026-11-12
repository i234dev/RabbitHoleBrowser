//! Min-content / max-content width computation (for shrink-to-fit sizing).

use crate::block::{horizontal_extras, Layouter};
use crate::inline::{item_style, Piece};
use crate::tree::{BoxKind, Formatting, LayoutBox};
use rabbithole_style::{BoxSizing, Length};

fn px(l: Length) -> f32 {
    match l {
        Length::Px(v) => v,
        _ => 0.0,
    }
}

/// Horizontal margins + padding + border in px (percentages count as 0).
pub(crate) fn extras_px(style: &rabbithole_style::ComputedStyle) -> f32 {
    px(style.margin.left) + px(style.margin.right) + horizontal_extras(style, 0.0)
}

impl<'a> Layouter<'a> {
    /// (min-content, max-content) widths of the box's content area.
    pub(crate) fn intrinsic(&mut self, b: &mut LayoutBox) -> (f32, f32) {
        if let Some(c) = b.intrinsic_cache {
            return c;
        }
        if b.wm_wrapper {
            let r = self.wm_wrapper_intrinsic(b);
            b.intrinsic_cache = Some(r);
            return r;
        }
        let style = b.style.clone();
        let pb = horizontal_extras(&style, 0.0);
        let adj = |v: f32| if style.box_sizing == BoxSizing::BorderBox { (v - pb).max(0.0) } else { v };
        let mut result = if b.formatting == Formatting::Table {
            // A table is never narrower than its columns.
            let (min, max) = self.table_intrinsic(b);
            match style.width {
                Length::Px(w) => {
                    let w = adj(w).max(min);
                    (w, w)
                }
                _ => (min, max),
            }
        } else if let Length::Px(w) = style.width {
            let w = adj(w);
            (w, w)
        } else if style.contain_size && !matches!(style.display, rabbithole_style::Display::Inline) && b.kind != BoxKind::Image {
            let w = style.contain_intrinsic_width.unwrap_or(0.0);
            (w, w)
        } else if b.kind == BoxKind::Image {
            let (w, _) = self.replaced_size(b, None, None, pb);
            (w, w)
        } else {
            match b.formatting {
                Formatting::Inline => self.inline_intrinsic(b),
                Formatting::Block | Formatting::TableRow => {
                    let mut min: f32 = 0.0;
                    let mut max: f32 = 0.0;
                    let is_row = b.formatting == Formatting::TableRow;
                    for c in &mut b.children {
                        if !(c.is_in_flow() || c.flow.is_float()) {
                            continue;
                        }
                        let ex = extras_px(&c.style);
                        let (cmin, mut cmax) = self.intrinsic(c);
                        if c.style.display == rabbithole_style::Display::TableCaption {
                            // Captions wrap to the table's width: only their
                            // min-content contributes to the container.
                            cmax = cmin;
                        }
                        if is_row {
                            min += cmin + ex;
                            max += cmax + ex;
                        } else {
                            min = min.max(cmin + ex);
                            max = max.max(cmax + ex);
                        }
                    }
                    (min, max)
                }
                Formatting::Flex => self.flex_intrinsic(b),
                Formatting::Grid => self.grid_intrinsic(b),
                Formatting::Table => self.table_intrinsic(b),
                Formatting::TableWrapper => {
                    let mut min: f32 = 0.0;
                    let mut max: f32 = 0.0;
                    for c in &mut b.children {
                        if c.formatting == Formatting::Table {
                            let ex = horizontal_extras(&c.style, 0.0);
                            let (a, m) = self.intrinsic(c);
                            min = min.max(a + ex);
                            max = max.max(m + ex);
                        } else if c.is_in_flow() {
                            // Captions wrap to the table's width: only their
                            // min-content contributes (CSS 2.1 §17.4).
                            let ex = extras_px(&c.style);
                            let (a, _) = self.intrinsic(c);
                            min = min.max(a + ex);
                            max = max.max(a + ex);
                        }
                    }
                    (min, max)
                }
                Formatting::None => (0.0, 0.0),
            }
        };
        if let Length::Px(m) = style.max_width {
            let m = adj(m);
            result.0 = result.0.min(m);
            result.1 = result.1.min(m);
        }
        if let Length::Px(m) = style.min_width {
            let m = adj(m);
            result.0 = result.0.max(m);
            result.1 = result.1.max(m);
        }
        let result = (result.0.max(0.0), result.1.max(result.0.max(0.0)));
        b.intrinsic_cache = Some(result);
        result
    }

    fn inline_intrinsic(&mut self, b: &mut LayoutBox) -> (f32, f32) {
        self.ensure_pieces(b);
        let pieces = b.pieces.take().unwrap_or_default();
        let cstyle = b.style.clone();
        let container_wraps = cstyle.white_space.wraps();
        let mut min: f32 = 0.0;
        let mut max: f32 = 0.0;
        let mut line: f32 = 0.0;
        let mut run: f32 = 0.0;
        let mut pending_space: f32 = 0.0;
        // text-indent widens the first line (percentages resolve against
        // a zero base here, as in browsers).
        let indent = cstyle.text_indent.resolve_or_zero(0.0);
        line += indent;
        run += indent;
        // Until the first word or atomic, break opportunities keep the
        // indent with the content that follows.
        let mut lead = true;
        if let Some(mt) = &b.marker_text {
            if cstyle.list_style_position == rabbithole_style::ListStylePosition::Inside {
                let font = self.font_for(&cstyle);
                let w = self.measurer.width(font, cstyle.font_size, &format!("{mt} "));
                line += w;
                run += w;
            }
        }
        for p in &pieces {
            match p {
                Piece::Word {
                    width, item, break_before, ..
                } => {
                    let wraps = item_style(&b.items, &cstyle, *item).white_space.wraps();
                    if *break_before && wraps && !lead {
                        min = min.max(run);
                        run = 0.0;
                    }
                    lead = false;
                    line += pending_space + width;
                    run += pending_space + width;
                    pending_space = 0.0;
                }
                Piece::Space { width, item } => {
                    let wraps = item_style(&b.items, &cstyle, *item).white_space.wraps();
                    if wraps && !lead {
                        min = min.max(run);
                        run = 0.0;
                    }
                    if line > 0.0 && !lead {
                        pending_space = *width;
                    }
                }
                Piece::Open(i) | Piece::Close(i) => {
                    let s = item_style(&b.items, &cstyle, *i);
                    let e = if matches!(p, Piece::Open(_)) {
                        px(s.margin.left) + s.border_width.left + px(s.padding.left)
                    } else {
                        px(s.margin.right) + s.border_width.right + px(s.padding.right)
                    };
                    line += e;
                    run += e;
                }
                Piece::Atomic(c) | Piece::Float(c) => {
                    let ex = extras_px(&b.children[*c].style);
                    let (cmin, cmax) = self.intrinsic(&mut b.children[*c]);
                    if container_wraps {
                        if lead {
                            min = min.max(run + cmin + ex);
                        } else {
                            min = min.max(run);
                            min = min.max(cmin + ex);
                        }
                        run = 0.0;
                    } else {
                        run += cmin + ex;
                    }
                    line += pending_space + cmax + ex;
                    pending_space = 0.0;
                    lead = false;
                }
                Piece::Abs(_) => {}
                Piece::Break => {
                    lead = false;
                    min = min.max(run);
                    run = 0.0;
                    max = max.max(line);
                    line = 0.0;
                    pending_space = 0.0;
                }
            }
        }
        min = min.max(run);
        max = max.max(line);
        b.pieces = Some(pieces);
        (min, max)
    }
}
