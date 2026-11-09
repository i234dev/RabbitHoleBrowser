//! Table layout: automatic column widths, rows and cells.

use crate::block::{horizontal_extras, Cb, Layouter, WidthMode};
use crate::float::Bfc;
use crate::tree::{BoxKind, Flow, Formatting, LayoutBox};
use rabbithole_gfx::Rect;
use rabbithole_style::{BoxSizing, Display, Length, VerticalAlign};

fn is_row_group(b: &LayoutBox) -> bool {
    b.kind == BoxKind::Block
        && matches!(
            b.style.display,
            Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup
        )
}

/// Call `f` on every row of the table (inside or outside row groups).
fn for_each_row<F: FnMut(&mut LayoutBox)>(table: &mut LayoutBox, f: &mut F) {
    for c in &mut table.children {
        if c.kind == BoxKind::TableRow {
            f(c);
        } else if is_row_group(c) {
            for r in &mut c.children {
                if r.kind == BoxKind::TableRow {
                    f(r);
                }
            }
        }
    }
}

impl<'a> Layouter<'a> {
    /// (min, max) width constraints per column, including cell padding/border.
    pub(crate) fn column_constraints(&mut self, table: &mut LayoutBox) -> Vec<(f32, f32)> {
        let mut ncols = 0usize;
        for_each_row(table, &mut |row| {
            let n: usize = row
                .children
                .iter()
                .filter(|c| c.kind == BoxKind::TableCell)
                .map(|c| c.colspan as usize)
                .sum();
            ncols = ncols.max(n);
        });
        let mut cols = vec![(0.0f32, 0.0f32); ncols];
        let mut spans: Vec<(usize, usize, f32, f32)> = Vec::new();
        for_each_row(table, &mut |row| {
            let mut col = 0usize;
            for cell in row.children.iter_mut().filter(|c| c.kind == BoxKind::TableCell) {
                let span = cell.colspan as usize;
                let ex = horizontal_extras(&cell.style, 0.0);
                let (mut cmin, mut cmax) = self.intrinsic(cell);
                cmin += ex;
                cmax += ex;
                if let Length::Px(w) = cell.style.width {
                    let w = if cell.style.box_sizing == BoxSizing::BorderBox { w } else { w + ex };
                    cmin = cmin.max(w);
                    cmax = cmax.max(w);
                }
                if span <= 1 {
                    if col < ncols {
                        cols[col].0 = cols[col].0.max(cmin);
                        cols[col].1 = cols[col].1.max(cmax);
                    }
                } else {
                    spans.push((col, span, cmin, cmax));
                }
                col += span;
            }
        });
        for (start, span, cmin, cmax) in spans {
            let end = (start + span).min(ncols);
            if start >= end {
                continue;
            }
            let n = (end - start) as f32;
            let sum_min: f32 = cols[start..end].iter().map(|c| c.0).sum();
            let sum_max: f32 = cols[start..end].iter().map(|c| c.1).sum();
            if cmin > sum_min {
                let add = (cmin - sum_min) / n;
                for c in &mut cols[start..end] {
                    c.0 += add;
                }
            }
            if cmax > sum_max {
                let add = (cmax - sum_max) / n;
                for c in &mut cols[start..end] {
                    c.1 += add;
                }
            }
        }
        for c in &mut cols {
            c.1 = c.1.max(c.0);
        }
        cols
    }

    fn distribute(cols: &[(f32, f32)], w: f32) -> Vec<f32> {
        let sum_min: f32 = cols.iter().map(|c| c.0).sum();
        let sum_max: f32 = cols.iter().map(|c| c.1).sum();
        if cols.is_empty() {
            return Vec::new();
        }
        if w >= sum_max {
            if sum_max > 0.0 {
                cols.iter().map(|c| c.1 + (w - sum_max) * c.1 / sum_max).collect()
            } else {
                let each = w / cols.len() as f32;
                cols.iter().map(|_| each).collect()
            }
        } else if w > sum_min && sum_max > sum_min {
            let t = (w - sum_min) / (sum_max - sum_min);
            cols.iter().map(|c| c.0 + (c.1 - c.0) * t).collect()
        } else {
            cols.iter().map(|c| c.0).collect()
        }
    }

    /// Lay out a table's rows; returns the content height.
    pub(crate) fn layout_table(&mut self, b: &mut LayoutBox, cb: &Cb) -> f32 {
        let cols = self.column_constraints(b);
        let widths = Self::distribute(&cols, cb.width);
        let content_x = b.content_rect.x;
        let content_y = b.content_rect.y;
        let mut y = content_y;
        for i in 0..b.children.len() {
            let kind = b.children[i].kind;
            let flow = b.children[i].flow;
            match flow {
                Flow::Absolute => {
                    b.children[i].static_pos = (content_x, y);
                    continue;
                }
                Flow::FloatLeft | Flow::FloatRight => {
                    let mut bfc = Bfc::new();
                    self.layout_float(&mut b.children[i], cb, y, &mut bfc);
                    continue;
                }
                Flow::InFlow => {}
            }
            if kind == BoxKind::TableRow {
                let h = self.layout_row(&mut b.children[i], &widths, content_x, y);
                y += h;
            } else if is_row_group(&b.children[i]) {
                let group = &mut b.children[i];
                let y0 = y;
                for r in 0..group.children.len() {
                    if group.children[r].kind == BoxKind::TableRow {
                        let h = self.layout_row(&mut group.children[r], &widths, content_x, y);
                        y += h;
                    }
                }
                group.rect = Rect::new(content_x, y0, cb.width, y - y0);
                group.content_rect = group.rect;
            } else {
                // Caption or other block: normal block layout.
                let mut bfc = Bfc::new();
                let top = y + self.effective_top_margin(&b.children[i], cb.width);
                self.layout_block(&mut b.children[i], cb, top, &mut bfc, WidthMode::Fill);
                y = b.children[i].rect.bottom() + b.children[i].collapsed_bottom_margin.max(0.0);
            }
        }
        y - content_y
    }

    fn layout_row(&mut self, row: &mut LayoutBox, widths: &[f32], x: f32, y: f32) -> f32 {
        let mut cx = x;
        let mut col = 0usize;
        let mut row_h: f32 = 0.0;
        for cell in row.children.iter_mut() {
            if cell.kind != BoxKind::TableCell {
                if cell.flow == Flow::Absolute {
                    cell.static_pos = (cx, y);
                }
                continue;
            }
            let span = cell.colspan as usize;
            let end = (col + span).min(widths.len());
            let cw: f32 = if col < end { widths[col..end].iter().sum() } else { 0.0 };
            let ex = horizontal_extras(&cell.style, cw);
            let cb = Cb {
                x: cx,
                width: cw,
                height: None,
            };
            let mut bfc = Bfc::new();
            self.layout_block(cell, &cb, y, &mut bfc, WidthMode::Fixed((cw - ex).max(0.0)));
            // Cells ignore margins.
            let m = cell.margin;
            if m.left != 0.0 || m.top != 0.0 {
                cell.translate(-m.left, -m.top);
            }
            cell.margin = rabbithole_style::Edges::all(0.0);
            row_h = row_h.max(cell.rect.h);
            cx += cw;
            col += span;
        }
        if let Length::Px(h) = row.style.height {
            row_h = row_h.max(h);
        }
        for cell in row.children.iter_mut().filter(|c| c.kind == BoxKind::TableCell) {
            let extra = row_h - cell.rect.h;
            if extra > 0.0 {
                let dy = match cell.style.vertical_align {
                    VerticalAlign::Middle => extra / 2.0,
                    VerticalAlign::Bottom | VerticalAlign::TextBottom => extra,
                    _ => 0.0,
                };
                if dy > 0.0 {
                    translate_contents(cell, 0.0, dy);
                }
                cell.rect.h = row_h;
                cell.content_rect.h += extra;
            }
        }
        let total_w: f32 = widths.iter().sum();
        row.rect = Rect::new(x, y, total_w, row_h);
        row.content_rect = row.rect;
        row.formatting = Formatting::TableRow;
        row_h
    }
}

/// Translate the contents of a box without moving the box itself.
pub(crate) fn translate_contents(b: &mut LayoutBox, dx: f32, dy: f32) {
    for line in &mut b.lines {
        line.rect = line.rect.translate(dx, dy);
        line.baseline += dy;
        for f in &mut line.fragments {
            f.rect = f.rect.translate(dx, dy);
            f.baseline += dy;
        }
    }
    if let Some(m) = &mut b.marker {
        m.x += dx;
        m.baseline += dy;
    }
    for c in &mut b.children {
        c.translate(dx, dy);
    }
}
