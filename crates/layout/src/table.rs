//! CSS 2.1 §17 table layout.
//!
//! The box tree (see `build.rs`) is already fixed up per §17.2.1: a table
//! box contains row groups, rows and column (group) boxes; rows contain
//! cells. Captions live in a table wrapper box around the table.
//!
//! Layout builds the cell grid (rowspan/colspan), resolves column widths
//! (automatic or fixed layout), lays out cells, computes row heights with
//! baseline alignment, applies `vertical-align` inside cells and positions
//! rows / row groups / columns. Both border models are supported: separated
//! borders with `border-spacing`, and collapsing borders (resolved per
//! §17.6.2 into per-cell half borders before layout).

use crate::block::{horizontal_extras, Cb, Layouter, WidthMode};
use crate::float::Bfc;
use crate::tree::{BoxKind, Flow, Formatting, LayoutBox};
use rabbithole_gfx::{Color, Rect};
use rabbithole_style::{BorderStyle, BoxSizing, ComputedStyle, Display, Edges, Length, VerticalAlign};
use std::rc::Rc;

pub(crate) fn is_row_group(b: &LayoutBox) -> bool {
    b.kind == BoxKind::Block
        && b.flow.is_in_flow()
        && matches!(b.style.display, Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup)
}

pub(crate) fn is_column(b: &LayoutBox) -> bool {
    b.kind == BoxKind::Block && matches!(b.style.display, Display::TableColumn | Display::TableColumnGroup)
}

pub(crate) fn is_caption(b: &LayoutBox) -> bool {
    b.flow.is_in_flow() && b.style.display == Display::TableCaption && b.kind == BoxKind::Block
}

/// Path from the table box to a row: `children[a]` or `children[a].children[b]`.
#[derive(Debug, Clone, Copy)]
struct RowPath(usize, Option<usize>);

#[derive(Debug, Clone, Copy)]
struct CellInfo {
    row_path: RowPath,
    /// Index of the cell in its row's children.
    idx: usize,
    row: usize,
    col: usize,
    rowspan: usize,
    colspan: usize,
}

#[derive(Debug, Clone, Copy)]
struct ColBox {
    /// `children[a]` or `children[a].children[b]` of the table.
    path: (usize, Option<usize>),
    first: usize,
    span: usize,
}

#[derive(Debug, Default)]
struct Grid {
    rows: Vec<RowPath>,
    /// Row group (table child index) of each row, if any.
    row_group: Vec<Option<usize>>,
    cells: Vec<CellInfo>,
    ncols: usize,
    /// Column / column-group boxes with the columns they cover.
    col_boxes: Vec<ColBox>,
    /// Specified width of each column (from `<col>` / `<colgroup>`).
    col_widths: Vec<Length>,
    /// `slots[r][c]` = index into `cells` of the cell covering the slot.
    slots: Vec<Vec<Option<usize>>>,
}

fn row_ref(table: &LayoutBox, p: RowPath) -> &LayoutBox {
    match p.1 {
        Some(j) => &table.children[p.0].children[j],
        None => &table.children[p.0],
    }
}

fn row_mut(table: &mut LayoutBox, p: RowPath) -> &mut LayoutBox {
    match p.1 {
        Some(j) => &mut table.children[p.0].children[j],
        None => &mut table.children[p.0],
    }
}

fn span_attr(doc: &rabbithole_dom::Document, b: &LayoutBox, name: &str) -> Option<u32> {
    let n = b.node?;
    if b.pseudo.is_some() {
        return None;
    }
    let v = doc.attr(n, name)?.trim();
    let digits: String = v.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<u32>().ok()
}

/// Build the slot grid of a table (HTML table model, simplified).
fn build_grid(table: &LayoutBox, doc: &rabbithole_dom::Document) -> Grid {
    let mut g = Grid::default();
    // Row order: the first header group, then the body, then the first
    // footer group (CSS 2.1 §17.2).
    let header = table.children.iter().position(|c| is_row_group(c) && c.style.display == Display::TableHeaderGroup);
    let footer = table.children.iter().position(|c| is_row_group(c) && c.style.display == Display::TableFooterGroup);
    let mut order: Vec<usize> = Vec::new();
    order.extend(header);
    order.extend((0..table.children.len()).filter(|&i| Some(i) != header && Some(i) != footer));
    order.extend(footer);
    for i in order {
        let c = &table.children[i];
        if c.kind == BoxKind::TableRow && c.flow.is_in_flow() {
            g.rows.push(RowPath(i, None));
            g.row_group.push(None);
        } else if is_row_group(c) {
            for (j, r) in c.children.iter().enumerate() {
                if r.kind == BoxKind::TableRow {
                    g.rows.push(RowPath(i, Some(j)));
                    g.row_group.push(Some(i));
                }
            }
        }
    }
    // Columns.
    let mut ncol_elems = 0usize;
    for (i, c) in table.children.iter().enumerate() {
        if !is_column(c) {
            continue;
        }
        if c.style.display == Display::TableColumnGroup {
            let start = ncol_elems;
            let cols: Vec<usize> = c.children.iter().enumerate().filter(|(_, k)| is_column(k)).map(|(j, _)| j).collect();
            if cols.is_empty() {
                let span = span_attr(doc, c, "span").unwrap_or(1).clamp(1, 1000) as usize;
                for _ in 0..span {
                    g.col_widths.push(c.style.width);
                }
                ncol_elems += span;
            } else {
                for j in cols {
                    let k = &c.children[j];
                    let span = span_attr(doc, k, "span").unwrap_or(1).clamp(1, 1000) as usize;
                    let w = if k.style.width.is_auto() { c.style.width } else { k.style.width };
                    for _ in 0..span {
                        g.col_widths.push(w);
                    }
                    g.col_boxes.push(ColBox { path: (i, Some(j)), first: ncol_elems, span });
                    ncol_elems += span;
                }
            }
            g.col_boxes.push(ColBox { path: (i, None), first: start, span: ncol_elems - start });
        } else {
            let span = span_attr(doc, c, "span").unwrap_or(1).clamp(1, 1000) as usize;
            for _ in 0..span {
                g.col_widths.push(c.style.width);
            }
            g.col_boxes.push(ColBox { path: (i, None), first: ncol_elems, span });
            ncol_elems += span;
        }
    }
    // Cells.
    let nrows = g.rows.len();
    let mut occupied: Vec<Vec<bool>> = vec![Vec::new(); nrows];
    let mut ncols = ncol_elems;
    for r in 0..nrows {
        let row = row_ref(table, g.rows[r]);
        // The rows of this row's group (rowspan cannot leave it).
        let mut group_end = r + 1;
        while group_end < nrows && g.row_group[group_end] == g.row_group[r] && g.row_group[r].is_some() {
            group_end += 1;
        }
        if g.row_group[r].is_none() {
            // Consecutive direct rows form an implicit group.
            while group_end < nrows && g.row_group[group_end].is_none() {
                group_end += 1;
            }
        }
        let mut col = 0usize;
        for (idx, cell) in row.children.iter().enumerate() {
            if cell.kind != BoxKind::TableCell {
                continue;
            }
            while occupied[r].get(col).copied().unwrap_or(false) {
                col += 1;
            }
            let colspan = (cell.colspan.max(1) as usize).min(1000);
            let rowspan = match span_attr(doc, cell, "rowspan") {
                Some(0) => group_end - r,
                Some(n) => (n as usize).min(group_end - r).max(1),
                None => 1,
            };
            for rr in r..r + rowspan {
                let o = &mut occupied[rr];
                if o.len() < col + colspan {
                    o.resize(col + colspan, false);
                }
                for slot in o.iter_mut().skip(col).take(colspan) {
                    *slot = true;
                }
            }
            g.cells.push(CellInfo { row_path: g.rows[r], idx, row: r, col, rowspan, colspan });
            col += colspan;
            ncols = ncols.max(col);
        }
    }
    g.ncols = ncols;
    g.col_widths.resize(ncols, Length::Auto);
    g.slots = vec![vec![None; ncols]; nrows];
    for (i, c) in g.cells.iter().enumerate() {
        for rr in c.row..c.row + c.rowspan {
            for cc in c.col..c.col + c.colspan {
                g.slots[rr][cc] = Some(i);
            }
        }
    }
    g
}

fn cell_ref<'t>(table: &'t LayoutBox, c: &CellInfo) -> &'t LayoutBox {
    &row_ref(table, c.row_path).children[c.idx]
}

fn cell_mut<'t>(table: &'t mut LayoutBox, c: &CellInfo) -> &'t mut LayoutBox {
    &mut row_mut(table, c.row_path).children[c.idx]
}

/// Horizontal padding + border of a cell in px (percent padding as 0).
fn cell_extras(style: &ComputedStyle) -> f32 {
    horizontal_extras(style, 0.0)
}

/// Spread `amount` over `cols` proportionally to `weights` (equally when
/// all weights are zero).
fn spread(target: &mut [f32], weights: &[f32], amount: f32) {
    if target.is_empty() || amount <= 0.0 {
        return;
    }
    let total: f32 = weights.iter().sum();
    let n = target.len() as f32;
    for (t, w) in target.iter_mut().zip(weights) {
        *t += if total > 0.0 { amount * w / total } else { amount / n };
    }
}

/// Column constraints of the automatic table layout algorithm.
#[derive(Debug, Clone, Default)]
struct Columns {
    min: Vec<f32>,
    max: Vec<f32>,
    /// Fixed (px) width, border-box.
    fixed: Vec<Option<f32>>,
    pct: Vec<Option<f32>>,
}

impl<'a> Layouter<'a> {
    fn spacing(&self, table: &LayoutBox) -> (f32, f32) {
        if table.style.border_collapse {
            (0.0, 0.0)
        } else {
            table.style.border_spacing
        }
    }

    /// Per-column min/max widths (cell border boxes).
    fn column_constraints(&mut self, table: &mut LayoutBox, g: &Grid) -> Columns {
        let n = g.ncols;
        let mut cols = Columns { min: vec![0.0; n], max: vec![0.0; n], fixed: vec![None; n], pct: vec![None; n] };
        let (hs, _) = self.spacing(table);
        for (c, w) in g.col_widths.iter().enumerate() {
            match *w {
                Length::Px(v) => {
                    cols.fixed[c] = Some(v);
                    cols.min[c] = cols.min[c].max(0.0);
                }
                Length::Percent(p) => cols.pct[c] = Some(p),
                _ => {}
            }
        }
        let mut order: Vec<usize> = (0..g.cells.len()).collect();
        order.sort_by_key(|&i| g.cells[i].colspan);
        for i in order {
            let info = g.cells[i];
            let cell = cell_mut(table, &info);
            let ex = cell_extras(&cell.style);
            let width = cell.style.width;
            let border_box = cell.style.box_sizing == BoxSizing::BorderBox;
            let (cmin, cmax) = self.intrinsic(cell);
            let (mut cmin, mut cmax) = (cmin + ex, cmax + ex);
            let fixed = match width {
                Length::Px(w) => {
                    let w = if border_box { w.max(ex) } else { w + ex };
                    cmin = cmin.max(if border_box { ex } else { 0.0 });
                    cmax = w.max(cmin);
                    Some(w)
                }
                _ => None,
            };
            let pct = match width {
                Length::Percent(p) => Some(p),
                _ => None,
            };
            let (s, e) = (info.col, info.col + info.colspan);
            if info.colspan == 1 {
                cols.min[s] = cols.min[s].max(cmin);
                cols.max[s] = cols.max[s].max(cmax);
                if let Some(f) = fixed {
                    cols.fixed[s] = Some(cols.fixed[s].unwrap_or(0.0).max(f));
                }
                if let Some(p) = pct {
                    cols.pct[s] = Some(cols.pct[s].unwrap_or(0.0).max(p));
                }
            } else {
                let gaps = (info.colspan - 1) as f32 * hs;
                let sum_min: f32 = cols.min[s..e].iter().sum::<f32>() + gaps;
                let sum_max: f32 = cols.max[s..e].iter().sum::<f32>() + gaps;
                let weights: Vec<f32> = cols.max[s..e].to_vec();
                if cmin > sum_min {
                    spread(&mut cols.min[s..e], &weights, cmin - sum_min);
                }
                if cmax > sum_max {
                    spread(&mut cols.max[s..e], &weights, cmax - sum_max);
                }
            }
        }
        for c in 0..n {
            if let Some(f) = cols.fixed[c] {
                cols.max[c] = f.max(cols.min[c]);
            }
            cols.max[c] = cols.max[c].max(cols.min[c]);
        }
        cols
    }

    /// (min-content, max-content) width of a table's content box
    /// (columns plus border spacing).
    pub(crate) fn table_intrinsic(&mut self, table: &mut LayoutBox) -> (f32, f32) {
        let g = build_grid(table, self.ctx.doc);
        if table.style.border_collapse {
            self.resolve_collapsed_borders(table);
        }
        let cols = self.column_constraints(table, &g);
        let (hs, _) = self.spacing(table);
        let spacing = if g.ncols > 0 { (g.ncols + 1) as f32 * hs } else { 0.0 };
        let min: f32 = cols.min.iter().sum::<f32>() + spacing;
        let mut max: f32 = cols.max.iter().sum::<f32>() + spacing;
        // Percentage columns ask for a table wide enough to honour them.
        let total_pct: f32 = cols.pct.iter().flatten().sum::<f32>().min(100.0);
        if total_pct > 0.0 {
            for c in 0..g.ncols {
                if let Some(p) = cols.pct[c].filter(|p| *p > 0.0) {
                    max = max.max(cols.max[c] * 100.0 / p + spacing);
                }
            }
            let rest: f32 = (0..g.ncols).filter(|&c| cols.pct[c].is_none()).map(|c| cols.max[c]).sum();
            if total_pct < 100.0 {
                max = max.max(rest * 100.0 / (100.0 - total_pct) + spacing);
            }
        }
        (min, max.max(min))
    }

    /// Distribute the available column width `w` (excluding spacing).
    fn distribute(cols: &Columns, w: f32) -> Vec<f32> {
        let n = cols.min.len();
        let sum_min: f32 = cols.min.iter().sum();
        if n == 0 {
            return Vec::new();
        }
        if w <= sum_min {
            return cols.min.clone();
        }
        let mut out = cols.min.clone();
        // Percentage columns first.
        let mut remaining = w - sum_min;
        let mut pct_total = 0.0;
        for c in 0..n {
            if let Some(p) = cols.pct[c] {
                let p = p.min(100.0 - pct_total).max(0.0);
                pct_total += p;
                let want = (w * p / 100.0).max(cols.min[c]);
                let add = (want - cols.min[c]).min(remaining).max(0.0);
                out[c] += add;
                remaining -= add;
            }
        }
        let fixed: Vec<usize> = (0..n).filter(|&c| cols.pct[c].is_none() && cols.fixed[c].is_some()).collect();
        let auto: Vec<usize> = (0..n).filter(|&c| cols.pct[c].is_none() && cols.fixed[c].is_none()).collect();
        // Grow fixed columns to their width, then auto columns to max.
        for set in [&fixed, &auto] {
            if remaining <= 0.0 {
                break;
            }
            let need: Vec<f32> = set.iter().map(|&c| (cols.max[c] - out[c]).max(0.0)).collect();
            let total: f32 = need.iter().sum();
            if total <= 0.0 {
                continue;
            }
            let give = remaining.min(total);
            for (k, &c) in set.iter().enumerate() {
                out[c] += give * need[k] / total;
            }
            remaining -= give;
        }
        // Excess: to auto columns (by max width), else fixed, else percent.
        if remaining > 0.01 {
            let targets: Vec<usize> = if !auto.is_empty() {
                auto
            } else if !fixed.is_empty() {
                fixed
            } else {
                (0..n).collect()
            };
            let weights: Vec<f32> = targets.iter().map(|&c| out[c]).collect();
            let mut add = vec![0.0; targets.len()];
            spread(&mut add, &weights, remaining);
            for (k, &c) in targets.iter().enumerate() {
                out[c] += add[k];
            }
        }
        out
    }

    /// `table-layout: fixed` column widths (CSS 2.1 §17.5.2.1).
    fn fixed_columns(&mut self, table: &mut LayoutBox, g: &Grid, w: f32) -> Vec<f32> {
        let n = g.ncols;
        let mut widths: Vec<Option<f32>> = g.col_widths.iter().map(|l| l.resolve(w)).collect();
        // Cells of the first row.
        let first: Vec<CellInfo> = g.cells.iter().filter(|c| c.row == 0).copied().collect();
        for info in first {
            let cell = cell_ref(table, &info);
            let ex = cell_extras(&cell.style);
            let Some(cw) = cell.style.width.resolve(w) else { continue };
            let cw = if cell.style.box_sizing == BoxSizing::BorderBox { cw } else { cw + ex };
            let span = info.colspan as f32;
            for c in info.col..info.col + info.colspan {
                if widths[c].is_none() {
                    widths[c] = Some(cw / span);
                }
            }
        }
        let used: f32 = widths.iter().flatten().sum();
        let free = widths.iter().filter(|x| x.is_none()).count();
        let each = if free > 0 { ((w - used) / free as f32).max(0.0) } else { 0.0 };
        let mut out: Vec<f32> = widths.iter().map(|x| x.unwrap_or(each)).collect();
        let total: f32 = out.iter().sum();
        if free == 0 && total < w && total > 0.0 {
            // All columns sized: the extra goes to them proportionally.
            let k = w / total;
            for x in &mut out {
                *x *= k;
            }
        }
        let _ = n;
        out
    }

    /// Lay out a table's rows; returns the content height. May widen the
    /// table box when its columns do not fit the given width.
    pub(crate) fn layout_table(&mut self, b: &mut LayoutBox, cb: &Cb) -> f32 {
        let g = build_grid(b, self.ctx.doc);
        let (hs, vs) = self.spacing(b);
        let n = g.ncols;
        let spacing = if n > 0 { (n + 1) as f32 * hs } else { 0.0 };
        let fixed_layout = b.style.table_layout_fixed && !b.style.width.is_auto();
        let widths = if fixed_layout {
            self.fixed_columns(b, &g, (cb.width - spacing).max(0.0))
        } else {
            let cols = self.column_constraints(b, &g);
            Self::distribute(&cols, (cb.width - spacing).max(0.0))
        };
        let table_w: f32 = widths.iter().sum::<f32>() + spacing;
        if table_w > cb.width + 0.01 {
            let extra = table_w - cb.width;
            b.rect.w += extra;
            b.content_rect.w += extra;
        }
        let content_x = b.content_rect.x;
        let content_y = b.content_rect.y;
        let mut col_x = Vec::with_capacity(n);
        let mut x = content_x + hs;
        for w in &widths {
            col_x.push(x);
            x += w + hs;
        }
        let span_w = |c: usize, span: usize| -> f32 { widths[c..c + span].iter().sum::<f32>() + (span.saturating_sub(1)) as f32 * hs };

        // Pass 1: lay out cells (at y = 0) and measure them.
        let nrows = g.rows.len();
        let mut measured: Vec<(f32, f32)> = Vec::with_capacity(g.cells.len()); // (height, baseline)
        for info in &g.cells {
            let cw = span_w(info.col, info.colspan);
            let cell = cell_mut(b, info);
            if cell.style.empty_cells_hide && !cell.style.border_collapse && cell_is_empty(cell) {
                cell.style = Rc::new(hidden_cell_style(&cell.style));
            }
            let ex = cell_extras(&cell.style);
            let ccb = Cb { x: col_x[info.col], width: cw, height: None };
            let mut bfc = Bfc::new();
            self.layout_block(cell, &ccb, 0.0, &mut bfc, WidthMode::Fixed((cw - ex).max(0.0)));
            // Cells have no margins.
            let m = cell.margin;
            if m.left != 0.0 || m.top != 0.0 {
                cell.translate(-m.left, -m.top);
            }
            cell.margin = Edges::all(0.0);
            cell.rect.w = cw;
            let baseline = cell
                .first_baseline()
                .map(|bl| bl - cell.rect.y)
                .unwrap_or_else(|| cell.content_rect.bottom() - cell.rect.y);
            measured.push((cell.rect.h, baseline));
        }

        // Row heights.
        let mut row_h = vec![0.0f32; nrows];
        let mut row_base = vec![0.0f32; nrows];
        for (r, p) in g.rows.iter().enumerate() {
            if let Length::Px(h) = row_ref(b, *p).style.height {
                row_h[r] = h.max(0.0);
            }
        }
        let baseline_aligned = |s: &ComputedStyle| !matches!(s.vertical_align, VerticalAlign::Top | VerticalAlign::Middle | VerticalAlign::Bottom);
        for (i, info) in g.cells.iter().enumerate() {
            if baseline_aligned(&cell_ref(b, info).style) {
                row_base[info.row] = row_base[info.row].max(measured[i].1);
            }
        }
        for (i, info) in g.cells.iter().enumerate() {
            if info.rowspan != 1 {
                continue;
            }
            let (h, bl) = measured[i];
            let need = if baseline_aligned(&cell_ref(b, info).style) { row_base[info.row] - bl + h } else { h };
            row_h[info.row] = row_h[info.row].max(need);
        }
        // Spanning cells: grow the last spanned row.
        for (i, info) in g.cells.iter().enumerate() {
            if info.rowspan <= 1 {
                continue;
            }
            let end = info.row + info.rowspan;
            let have: f32 = row_h[info.row..end].iter().sum::<f32>() + (info.rowspan - 1) as f32 * vs;
            let (h, _) = measured[i];
            if h > have {
                row_h[end - 1] += h - have;
            }
        }
        // A taller specified table height is shared by the rows.
        let rows_total: f32 = row_h.iter().sum::<f32>() + if nrows > 0 { (nrows + 1) as f32 * vs } else { 0.0 };
        if let Some(target) = cb.height {
            if target > rows_total + 0.01 && nrows > 0 {
                let extra = target - rows_total;
                let weights = row_h.clone();
                spread(&mut row_h, &weights, extra);
            }
        }

        // Pass 2: position rows and cells.
        let mut row_y = vec![0.0f32; nrows];
        let mut y = content_y + if nrows > 0 { vs } else { 0.0 };
        for r in 0..nrows {
            row_y[r] = y;
            y += row_h[r] + vs;
        }
        let content_h = if nrows > 0 { y - content_y } else { 0.0 };
        let grid_x = col_x.first().copied().unwrap_or(content_x);
        let grid_w: f32 = widths.iter().sum::<f32>() + (n.saturating_sub(1)) as f32 * hs;
        for (i, info) in g.cells.iter().enumerate() {
            let end = info.row + info.rowspan;
            let total_h: f32 = row_h[info.row..end].iter().sum::<f32>() + (info.rowspan - 1) as f32 * vs;
            let (h, bl) = measured[i];
            let base = row_base[info.row];
            let cell = cell_mut(b, info);
            let dy = row_y[info.row] - cell.rect.y;
            cell.translate(0.0, dy);
            let shift = match cell.style.vertical_align {
                VerticalAlign::Top => 0.0,
                VerticalAlign::Middle => (total_h - h) / 2.0,
                VerticalAlign::Bottom => total_h - h,
                _ => base - bl,
            };
            if shift.abs() > 0.001 {
                translate_contents(cell, 0.0, shift);
            }
            let extra = total_h - cell.rect.h;
            cell.rect.h = total_h;
            cell.content_rect.h = (cell.content_rect.h + extra).max(0.0);
        }
        for (r, p) in g.rows.iter().enumerate() {
            let row = row_mut(b, *p);
            row.rect = Rect::new(grid_x, row_y[r], grid_w, row_h[r]);
            row.content_rect = row.rect;
            row.margin = Edges::all(0.0);
        }
        // Row groups span their rows.
        let mut cursor = content_y + if nrows > 0 { vs } else { 0.0 };
        let mut i = 0;
        while i < b.children.len() {
            if is_row_group(&b.children[i]) {
                let rows: Vec<usize> = (0..nrows).filter(|&r| g.row_group[r] == Some(i)).collect();
                let (top, bottom) = match (rows.first(), rows.last()) {
                    (Some(&f), Some(&l)) => (row_y[f], row_y[l] + row_h[l]),
                    _ => (cursor, cursor),
                };
                cursor = bottom;
                let grp = &mut b.children[i];
                grp.rect = Rect::new(grid_x, top, grid_w, bottom - top);
                grp.content_rect = grp.rect;
                grp.margin = Edges::all(0.0);
            } else if b.children[i].kind == BoxKind::TableRow {
                cursor = b.children[i].rect.bottom();
            }
            i += 1;
        }
        // Columns (painted beneath rows and cells).
        let (gy, gh) = if nrows > 0 { (row_y[0], row_y[nrows - 1] + row_h[nrows - 1] - row_y[0]) } else { (content_y, 0.0) };
        for cbx in &g.col_boxes {
            if cbx.first >= n {
                continue;
            }
            let span = cbx.span.min(n - cbx.first).max(1);
            let r = Rect::new(col_x[cbx.first], gy, span_w(cbx.first, span), gh);
            let bx = match cbx.path.1 {
                Some(j) => &mut b.children[cbx.path.0].children[j],
                None => &mut b.children[cbx.path.0],
            };
            bx.rect = r;
            bx.content_rect = r;
        }
        // Anything else (stray out-of-flow boxes) sits at the table origin.
        for c in b.children.iter_mut() {
            if c.flow == Flow::Absolute {
                c.static_pos = (content_x, content_y);
            }
        }
        content_h
    }

    /// Lay out a table wrapper's contents: captions above/below the table.
    pub(crate) fn layout_table_wrapper(&mut self, b: &mut LayoutBox, cb: &Cb) -> f32 {
        let content_y = b.content_rect.y;
        let mut y = content_y;
        for i in 0..b.children.len() {
            let c = &mut b.children[i];
            let mut bfc = Bfc::new();
            if c.formatting == Formatting::Table {
                let ex = horizontal_extras(&c.style, cb.width);
                self.layout_block(c, cb, y, &mut bfc, WidthMode::Fixed((cb.width - ex).max(0.0)));
                y = c.rect.bottom();
            } else if c.flow.is_in_flow() {
                let mt = c.style.margin.top.resolve_or_zero(cb.width);
                self.layout_block(c, cb, y + mt, &mut bfc, WidthMode::Fill);
                y = c.rect.bottom() + c.margin.bottom;
            }
        }
        y - content_y
    }

    /// Border-box width of a wrapper's table (to size the wrapper).
    pub(crate) fn wrapped_table_width(&mut self, b: &mut LayoutBox, avail: f32, mode: WidthMode) -> f32 {
        let Some(t) = b.children.iter_mut().find(|c| c.formatting == Formatting::Table) else { return 0.0 };
        let pb = horizontal_extras(&t.style, avail);
        if let WidthMode::Fixed(w) = mode {
            return w;
        }
        let (min, max) = self.intrinsic(t);
        let border_box = t.style.box_sizing == BoxSizing::BorderBox;
        let content = match t.style.width.resolve(avail) {
            Some(w) => {
                let w = if border_box { w - pb } else { w };
                self.clamp_width(&t.style.clone(), w.max(0.0), avail, pb).max(min)
            }
            None => crate::block::shrink_to_fit(min, max, (avail - pb).max(0.0)),
        };
        content + pb
    }

    /// Resolve collapsed borders (CSS 2.1 §17.6.2): every cell gets half of
    /// each winning border (style and color of the winner), the table gets
    /// the outer halves and no padding.
    pub(crate) fn resolve_collapsed_borders(&mut self, table: &mut LayoutBox) {
        let g = build_grid(table, self.ctx.doc);
        let orig = |b: &LayoutBox| -> Rc<ComputedStyle> {
            if b.pseudo.is_none() {
                if let Some(s) = b.node.and_then(|n| self.ctx.styles.get(n)) {
                    return s.clone();
                }
            }
            b.style.clone()
        };
        let tstyle = orig(table);
        let nrows = g.rows.len();
        let ncols = g.ncols;
        if nrows == 0 || ncols == 0 {
            let mut s = (*tstyle).clone();
            s.padding = Edges::all(Length::ZERO);
            table.style = Rc::new(s);
            return;
        }
        let cell_styles: Vec<Rc<ComputedStyle>> = g.cells.iter().map(|c| orig(cell_ref(table, c))).collect();
        let row_styles: Vec<Rc<ComputedStyle>> = g.rows.iter().map(|p| orig(row_ref(table, *p))).collect();
        let group_style = |r: usize| -> Option<Rc<ComputedStyle>> { g.row_group[r].map(|i| orig(&table.children[i])) };
        // Candidate border: (width, style, color, origin rank).
        type Cand = (f32, BorderStyle, Color, u8);
        fn side(s: &ComputedStyle, k: usize, rank: u8) -> Cand {
            let (w, st, c) = match k {
                0 => (s.border_width.top, s.border_style.top, s.border_color.top),
                1 => (s.border_width.right, s.border_style.right, s.border_color.right),
                2 => (s.border_width.bottom, s.border_style.bottom, s.border_color.bottom),
                _ => (s.border_width.left, s.border_style.left, s.border_color.left),
            };
            (if st.is_visible() { w } else { 0.0 }, st, c, rank)
        }
        fn style_rank(s: BorderStyle) -> u8 {
            match s {
                BorderStyle::Double => 5,
                BorderStyle::Solid => 4,
                BorderStyle::Dashed => 3,
                BorderStyle::Dotted => 2,
                BorderStyle::None => 0,
            }
        }
        fn better(a: Cand, b: Cand) -> Cand {
            let ka = (a.0, style_rank(a.1), a.3);
            let kb = (b.0, style_rank(b.1), b.3);
            if kb.0 > ka.0 || (kb.0 == ka.0 && (kb.1 > ka.1 || (kb.1 == ka.1 && kb.2 > ka.2))) {
                b
            } else {
                a
            }
        }
        const CELL: u8 = 4;
        const ROW: u8 = 3;
        const GROUP: u8 = 2;
        const TABLE: u8 = 0;
        let mut new_cells: Vec<[Cand; 4]> = Vec::with_capacity(g.cells.len());
        for (i, c) in g.cells.iter().enumerate() {
            let s = &cell_styles[i];
            let last_row = c.row + c.rowspan - 1;
            let last_col = c.col + c.colspan - 1;
            // Top.
            let mut top = side(s, 0, CELL);
            top = better(top, side(&row_styles[c.row], 0, ROW));
            if c.row > 0 {
                if let Some(j) = g.slots[c.row - 1][c.col] {
                    top = better(top, side(&cell_styles[j], 2, CELL));
                }
                top = better(top, side(&row_styles[c.row - 1], 2, ROW));
            }
            if c.row == 0 || g.row_group[c.row - 1] != g.row_group[c.row] {
                if let Some(gs) = group_style(c.row) {
                    top = better(top, side(&gs, 0, GROUP));
                }
                if c.row > 0 {
                    if let Some(gs) = group_style(c.row - 1) {
                        top = better(top, side(&gs, 2, GROUP));
                    }
                }
            }
            if c.row == 0 {
                top = better(top, side(&tstyle, 0, TABLE));
            }
            // Bottom.
            let mut bottom = side(s, 2, CELL);
            bottom = better(bottom, side(&row_styles[last_row], 2, ROW));
            if last_row + 1 < nrows {
                if let Some(j) = g.slots[last_row + 1][c.col] {
                    bottom = better(bottom, side(&cell_styles[j], 0, CELL));
                }
                bottom = better(bottom, side(&row_styles[last_row + 1], 0, ROW));
            }
            if last_row + 1 == nrows || g.row_group[last_row + 1] != g.row_group[last_row] {
                if let Some(gs) = group_style(last_row) {
                    bottom = better(bottom, side(&gs, 2, GROUP));
                }
                if last_row + 1 < nrows {
                    if let Some(gs) = group_style(last_row + 1) {
                        bottom = better(bottom, side(&gs, 0, GROUP));
                    }
                }
            }
            if last_row + 1 == nrows {
                bottom = better(bottom, side(&tstyle, 2, TABLE));
            }
            // Left.
            let mut left = side(s, 3, CELL);
            if c.col > 0 {
                if let Some(j) = g.slots[c.row][c.col - 1] {
                    left = better(left, side(&cell_styles[j], 1, CELL));
                }
            } else {
                left = better(left, side(&row_styles[c.row], 3, ROW));
                if let Some(gs) = group_style(c.row) {
                    left = better(left, side(&gs, 3, GROUP));
                }
                left = better(left, side(&tstyle, 3, TABLE));
            }
            // Right.
            let mut right = side(s, 1, CELL);
            if last_col + 1 < ncols {
                if let Some(j) = g.slots[c.row][last_col + 1] {
                    right = better(right, side(&cell_styles[j], 3, CELL));
                }
            } else {
                right = better(right, side(&row_styles[c.row], 1, ROW));
                if let Some(gs) = group_style(c.row) {
                    right = better(right, side(&gs, 1, GROUP));
                }
                right = better(right, side(&tstyle, 1, TABLE));
            }
            new_cells.push([top, right, bottom, left]);
        }
        // Halves: the cell after a grid line (below / right) takes the
        // larger half; at the table edge the cell takes the inner half.
        let half_lo = |w: f32| (w / 2.0).floor();
        let half_hi = |w: f32| w - (w / 2.0).floor();
        let mut tb: [Cand; 4] = [(0.0, BorderStyle::None, Color::TRANSPARENT, 0); 4];
        let mut tw = [0.0f32; 4];
        for (i, c) in g.cells.iter().enumerate() {
            let [top, right, bottom, left] = new_cells[i];
            let mut s = (*cell_styles[i]).clone();
            let last_col = c.col + c.colspan - 1;
            let last_row = c.row + c.rowspan - 1;
            s.border_width = Edges { top: half_hi(top.0), right: half_lo(right.0), bottom: half_lo(bottom.0), left: half_hi(left.0) };
            s.border_style = Edges { top: top.1, right: right.1, bottom: bottom.1, left: left.1 };
            s.border_color = Edges { top: top.2, right: right.2, bottom: bottom.2, left: left.2 };
            if c.row == 0 && half_lo(top.0) >= tw[0] {
                tw[0] = half_lo(top.0);
                tb[0] = top;
            }
            if last_col + 1 == ncols && half_hi(right.0) >= tw[1] {
                tw[1] = half_hi(right.0);
                tb[1] = right;
            }
            if last_row + 1 == nrows && half_hi(bottom.0) >= tw[2] {
                tw[2] = half_hi(bottom.0);
                tb[2] = bottom;
            }
            if c.col == 0 && half_lo(left.0) >= tw[3] {
                tw[3] = half_lo(left.0);
                tb[3] = left;
            }
            cell_mut(table, c).style = Rc::new(s);
        }
        let mut ts = (*tstyle).clone();
        ts.padding = Edges::all(Length::ZERO);
        ts.border_width = Edges { top: tw[0], right: tw[1], bottom: tw[2], left: tw[3] };
        ts.border_style = Edges { top: tb[0].1, right: tb[1].1, bottom: tb[2].1, left: tb[3].1 };
        ts.border_color = Edges { top: tb[0].2, right: tb[1].2, bottom: tb[2].2, left: tb[3].2 };
        table.style = Rc::new(ts);
    }
}

/// A cell with no in-flow content (for `empty-cells: hide`).
fn cell_is_empty(cell: &LayoutBox) -> bool {
    use crate::tree::InlineItem;
    cell.children.iter().all(|c| !c.is_in_flow())
        && cell.items.iter().all(|it| match it {
            InlineItem::Text { text, .. } => text.chars().all(|ch| matches!(ch, ' ' | '\t' | '\n' | '\r' | '\x0c')),
            InlineItem::Open { .. } | InlineItem::Close | InlineItem::Float(_) | InlineItem::Absolute(_) => true,
            _ => false,
        })
}

fn hidden_cell_style(s: &ComputedStyle) -> ComputedStyle {
    let mut s = s.clone();
    s.background_color = Color::TRANSPARENT;
    s.background_layers.clear();
    s.background_image = None;
    s.border_color = Edges::all(Color::TRANSPARENT);
    s.box_shadow.clear();
    s
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
