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
    /// [`Self::intrinsic`] of a box whose containing block has the content
    /// height `cb_h` (for percentage heights of replaced descendants).
    pub(crate) fn intrinsic_in(&mut self, b: &mut LayoutBox, cb_h: Option<f32>) -> (f32, f32) {
        let saved = std::mem::replace(&mut self.intrinsic_cb_h, cb_h);
        let saved_used = std::mem::replace(&mut self.intrinsic_cb_used, false);
        let r = self.intrinsic(b);
        self.intrinsic_cb_h = saved;
        self.intrinsic_cb_used = saved_used;
        r
    }

    /// (min-content, max-content) widths of the box's content area.
    pub(crate) fn intrinsic(&mut self, b: &mut LayoutBox) -> (f32, f32) {
        let _probe = self.grid_publication.probe();
        let forced_h = self.intrinsic_forced_h.take();
        if forced_h.is_none() {
            if let Some(c) = b.intrinsic_cache {
                return c;
            }
        }
        if b.wm_wrapper {
            let r = self.wm_wrapper_intrinsic(b);
            b.intrinsic_cache = Some(r);
            return r;
        }
        let style = b.style.clone();
        let pb = horizontal_extras(&style, 0.0);
        let adj = |v: f32| if style.box_sizing == BoxSizing::BorderBox { (v - pb).max(0.0) } else { v };
        // The box's own definite content height, against which its
        // contents resolve percentage heights (CSS Sizing §5.2.1).
        let outer_h = self.intrinsic_cb_h;
        let pb_v = crate::block::vertical_extras(&style);
        let adj_v = |v: f32| if style.box_sizing == BoxSizing::BorderBox { (v - pb_v).max(0.0) } else { v.max(0.0) };
        let own_h = forced_h.or_else(|| match style.height {
            Length::Px(v) => Some(adj_v(v)),
            l if l.has_percent() => l.resolve_definite(outer_h).map(adj_v),
            _ => None,
        });
        // Whether this result depends on the containing block's height.
        let mut reads_outer = forced_h.is_some();
        let saved_cb = std::mem::replace(&mut self.intrinsic_cb_h, own_h);
        let saved_used = std::mem::replace(&mut self.intrinsic_cb_used, false);
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
        } else if (style.contain_size || style.container_size || ((style.contain_inline_size || style.container_inline) && !style.writing_mode.is_vertical()))
            && !matches!(style.display, rabbithole_style::Display::Inline)
            && b.kind != BoxKind::Image
        {
            let w = style.contain_intrinsic_width.unwrap_or(0.0);
            (w, w)
        } else if b.kind == BoxKind::Image {
            let pct = style.height.has_percent() || style.max_height.has_percent() || style.min_height.has_percent();
            reads_outer |= pct;
            let (w, _) = match forced_h {
                // A stretched image's width follows its ratio.
                Some(h) if style.width.is_auto() => match crate::flex::item_ratio(b) {
                    Some(r) => (h * r, h),
                    None => self.replaced_size(b, None, None, pb),
                },
                _ => self.replaced_size(b, None, if pct { outer_h } else { None }, pb),
            };
            (w, w)
        } else if let (Some(ratio), true) = (style.aspect_ratio.filter(|r| *r > 0.0), style.width.is_auto() && b.kind != BoxKind::Image) {
            // A definite height (clamped by min/max-height) transfers
            // through `aspect-ratio`; the contents still set a minimum.
            // Without one, min/max-height transfer as limits.
            reads_outer |= style.height.has_percent() || style.min_height.has_percent() || style.max_height.has_percent();
            let to_w = |h: f32| if style.box_sizing == BoxSizing::BorderBox && !style.aspect_ratio_auto { ((h + pb_v) * ratio - pb).max(0.0) } else { h * ratio };
            let h_min = style.min_height.resolve_definite(outer_h).map(adj_v).unwrap_or(0.0);
            let h_max = style.max_height.resolve_definite(outer_h).map(adj_v).unwrap_or(f32::INFINITY).max(h_min);
            let (cmin, cmax) = self.intrinsic_contents(b);
            match own_h {
                Some(h) => {
                    let w = to_w(h.clamp(h_min, h_max)).max(cmin);
                    (w, w)
                }
                None => {
                    let (lo, hi) = (if h_min > 0.0 { to_w(h_min) } else { 0.0 }, if h_max.is_finite() { to_w(h_max) } else { f32::INFINITY });
                    (cmin.min(hi).max(lo), cmax.min(hi).max(lo))
                }
            }
        } else {
            self.intrinsic_contents(b)
        };
        let children_read = self.intrinsic_cb_used;
        self.intrinsic_cb_h = saved_cb;
        // Contents that read this box's height depend on the outer height
        // only through a percentage height.
        reads_outer |= children_read && style.height.has_percent();
        self.intrinsic_cb_used = saved_used || reads_outer;
        let cacheable = !reads_outer;
        result = self.finish_intrinsic(&style, result, adj);
        if cacheable {
            b.intrinsic_cache = Some(result);
        }
        result
    }

    /// (min, max) of a box's contents by formatting context.
    #[inline]
    fn intrinsic_contents(&mut self, b: &mut LayoutBox) -> (f32, f32) {
        {
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
        }
    }

    /// Apply min/max-width to intrinsic sizes.
    fn finish_intrinsic(&self, style: &rabbithole_style::ComputedStyle, mut result: (f32, f32), adj: impl Fn(f32) -> f32) -> (f32, f32) {
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
        (result.0.max(0.0), result.1.max(result.0.max(0.0)))
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
        // `hanging` indents all lines but the first; `each-line` the lines
        // after forced breaks too.
        let (each_line, hanging) = (cstyle.text_indent_flags & 1 != 0, cstyle.text_indent_flags & 2 != 0);
        let soft_indent = if hanging { indent } else { 0.0 };
        let forced_indent = if each_line != hanging { indent } else { 0.0 };
        if !hanging {
            line += indent;
            run += indent;
        }
        // Until the first word or atomic, break opportunities keep the
        // indent with the content that follows.
        let mut lead = true;
        let mut after_shy = false;
        // Hanging width at the end of the last word.
        let mut trail: f32 = 0.0;
        // Preserved `pre-wrap` spaces ending the run: they hang at a soft
        // wrap and do not count for min-content.
        let mut space_trail: f32 = 0.0;
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
                    text, width, item, break_before,
                } => {
                    let st = item_style(&b.items, &cstyle, *item);
                    // `auto-phrase` keeps its words whole in the intrinsic
                    // sizes (soft hyphens are a fallback for overflow).
                    let wraps = st.white_space.wraps() && !(after_shy && st.word_break == rabbithole_style::WordBreak::AutoPhrase);
                    after_shy = text.ends_with('\u{ad}');
                    let tab;
                    let width = if &**text == "\t" {
                        let st = item_style(&b.items, &cstyle, *item).clone();
                        tab = self.tab_width(&cstyle, &st, line + pending_space - indent);
                        &tab
                    } else {
                        width
                    };
                    if *break_before && wraps && !lead {
                        min = min.max(run - trail - space_trail);
                        run = soft_indent;
                    }
                    lead = false;
                    if st.white_space == rabbithole_style::WhiteSpace::PreWrap && text.bytes().all(|b| b == b' ' || b == b'\t') {
                        space_trail += width;
                    } else {
                        space_trail = 0.0;
                    }
                    line += pending_space + width;
                    // `overflow-wrap: anywhere` / `word-break: break-word`:
                    // every grapheme cluster may end a line (min-content).
                    // `word-break: break-all` / `line-break: anywhere`: the
                    // word's opportunities (kept whole in the pieces).
                    let rules = crate::inline::line_rules(st);
                    let cut_all = wraps && (rules.word_break == 3 || rules.line_break == 3) && text.chars().nth(1).is_some();
                    if cut_all || wraps && (st.overflow_wrap == rabbithole_style::OverflowWrap::Anywhere || st.word_break == rabbithole_style::WordBreak::BreakWord) && text.chars().nth(1).is_some() {
                        let font = self.font_for(st);
                        let mut first = true;
                        let mut start = 0;
                        let mut points = Vec::new();
                        let mut pi = 0;
                        if cut_all {
                            crate::linebreak::split_points(&rules, text, &mut points);
                        }
                        let mut it = text.char_indices().peekable();
                        while it.next().is_some() {
                            let end = it.peek().map_or(text.len(), |n| n.0);
                            if cut_all {
                                while points.get(pi).is_some_and(|p| *p < end) {
                                    pi += 1;
                                }
                                if end != text.len() && points.get(pi) != Some(&end) {
                                    continue;
                                }
                            } else if it.peek().is_some_and(|n| matches!(crate::linebreak::class(n.1), crate::linebreak::Lb::CM | crate::linebreak::Lb::ZWJ)) {
                                continue;
                            }
                            let cw = self.measurer.width(font, st.font_size, &text[start..end]) + st.letter_spacing * text[start..end].chars().count() as f32;
                            if first {
                                run += pending_space + cw;
                                first = false;
                            } else {
                                min = min.max(run);
                                run = cw;
                            }
                            start = end;
                        }
                        pending_space = 0.0;
                        trail = if cstyle.line_height_tight { 0.0 } else { st.letter_spacing.max(0.0) };
                        continue;
                    }
                    run += pending_space + width;
                    pending_space = 0.0;
                    // Other space separators ending a line hang; no
                    // letter-spacing at a line end.
                    trail = if cstyle.line_height_tight { 0.0 } else { st.letter_spacing.max(0.0) } + if text.chars().next_back().is_some_and(crate::linebreak::is_space_separator) && st.white_space != rabbithole_style::WhiteSpace::BreakSpaces {
                        let font = self.font_for(st);
                        let sep: String = text.chars().rev().take_while(|c| crate::linebreak::is_space_separator(*c)).collect();
                        (self.measurer.width(font, st.font_size, &sep) + st.letter_spacing * sep.chars().count() as f32).min(*width)
                    } else {
                        0.0
                    };
                    continue;
                }
                Piece::Space { width, item } => {
                    let wraps = item_style(&b.items, &cstyle, *item).white_space.wraps();
                    if wraps && !lead {
                        min = min.max(run - trail - space_trail);
                        run = soft_indent;
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
                Piece::Atomic(c, _) | Piece::Float(c) => {
                    let before = !matches!(p, Piece::Atomic(_, false));
                    let ex = extras_px(&b.children[*c].style);
                    let (cmin, cmax) = self.intrinsic(&mut b.children[*c]);
                    if container_wraps && before && !lead {
                        min = min.max(run - trail - space_trail);
                        run = soft_indent;
                    }
                    run += cmin + ex;
                    if container_wraps {
                        min = min.max(run);
                    }
                    line += pending_space + cmax + ex;
                    pending_space = 0.0;
                    lead = false;
                    trail = 0.0;
                    space_trail = 0.0;
                }
                Piece::Abs(_) => {}
                Piece::Gap(w) => {
                    run += w;
                    line += w;
                    trail = 0.0;
                    space_trail = 0.0;
                }
                Piece::Break => {
                    lead = false;
                    min = min.max(run - trail - space_trail);
                    run = forced_indent;
                    max = max.max(line - trail);
                    line = forced_indent;
                    pending_space = 0.0;
                    trail = 0.0;
                }
            }
        }
        min = min.max(run - trail - space_trail);
        max = max.max(line - trail);
        b.pieces = Some(pieces);
        (min, max)
    }
}
