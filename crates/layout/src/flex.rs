//! Simplified single-line row flexbox.

use crate::block::{horizontal_extras, resolve_edges, Cb, Layouter, WidthMode};
use crate::float::Bfc;
use crate::tree::{Flow, LayoutBox};
use rabbithole_style::{BoxSizing, Length};

impl<'a> Layouter<'a> {
    /// Lay out the flex items of `b` in a row; returns the content height.
    pub(crate) fn layout_flex(&mut self, b: &mut LayoutBox, cb: &Cb) -> f32 {
        let content_x = b.content_rect.x;
        let content_y = b.content_rect.y;
        let avail = cb.width;
        let n = b.children.len();
        // Base sizes.
        let mut base = vec![0.0f32; n];
        let mut minw = vec![0.0f32; n];
        let mut extras = vec![0.0f32; n];
        let mut auto = vec![false; n];
        let mut total = 0.0f32;
        for (i, c) in b.children.iter_mut().enumerate() {
            if !c.is_in_flow() {
                continue;
            }
            let m = resolve_edges(&c.style.margin, avail);
            let pb = horizontal_extras(&c.style, avail);
            extras[i] = m.left + m.right + pb;
            let (imin, imax) = self.intrinsic(c);
            let adj = |v: f32| if c.style.box_sizing == BoxSizing::BorderBox { (v - pb).max(0.0) } else { v };
            match c.style.width {
                Length::Auto => {
                    base[i] = imax;
                    auto[i] = true;
                }
                Length::Px(w) => base[i] = adj(w),
                Length::Percent(p) => base[i] = adj(avail * p / 100.0),
            }
            if let Some(mw) = c.style.max_width.resolve(avail) {
                base[i] = base[i].min(adj(mw));
            }
            if let Some(mw) = c.style.min_width.resolve(avail) {
                base[i] = base[i].max(adj(mw));
            }
            minw[i] = imin.min(base[i]);
            total += base[i] + extras[i];
        }
        let mut widths = base.clone();
        if total > avail + 0.01 {
            let deficit = total - avail;
            let sum_base: f32 = base.iter().sum();
            if sum_base > 0.0 {
                for i in 0..n {
                    let shrink = deficit * base[i] / sum_base;
                    widths[i] = (base[i] - shrink).max(minw[i]);
                }
            }
        } else {
            let growers = auto.iter().filter(|a| **a).count();
            if growers > 0 {
                let share = (avail - total) / growers as f32;
                for i in 0..n {
                    if auto[i] {
                        widths[i] += share;
                    }
                }
            }
        }
        // Place items.
        let mut x = content_x;
        let mut row_h: f32 = 0.0;
        for i in 0..n {
            let child = &mut b.children[i];
            match child.flow {
                Flow::Absolute => {
                    child.static_pos = (x, content_y);
                    continue;
                }
                Flow::FloatLeft | Flow::FloatRight => {
                    // Floats are ignored in flex; lay out as items anyway.
                    child.flow = Flow::InFlow;
                }
                Flow::InFlow => {}
            }
            let mt = child.style.margin.top.resolve_or_zero(avail);
            let icb = Cb {
                x,
                width: widths[i] + extras[i],
                height: cb.height,
            };
            let mut bfc = Bfc::new();
            self.layout_block(child, &icb, content_y + mt, &mut bfc, WidthMode::Fixed(widths[i]));
            x += widths[i] + extras[i];
            row_h = row_h.max(child.margin_rect().h);
        }
        // Stretch.
        for c in b.children.iter_mut().filter(|c| c.is_in_flow()) {
            if c.style.height.is_auto() {
                let mh = c.margin_rect().h;
                let extra = row_h - mh;
                if extra > 0.0 {
                    c.rect.h += extra;
                    c.content_rect.h += extra;
                }
            }
        }
        row_h
    }
}
