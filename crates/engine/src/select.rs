//! Text selection and find-in-page over laid out text fragments.

use crate::Page;
use rabbithole_dom::{BoundaryPoint, NodeId};
use rabbithole_style::UserSelect;
use rabbithole_gfx::{FontContext, FontId, Rect};
use rabbithole_layout::{FragmentKind, LayoutBox};
use std::rc::Rc;

/// A laid out run of text (document coordinates).
#[derive(Debug, Clone)]
pub struct TextRun {
    pub rect: Rect,
    pub baseline: f32,
    pub node: Option<NodeId>,
    pub text: Rc<str>,
    pub font: FontId,
    pub size: f32,
    pub letter_spacing: f32,
    /// Extra space after each word separator (justified lines).
    pub word_spacing: f32,
    /// The DOM text node shown (None: generated or widget text).
    pub src: Option<NodeId>,
    /// Address of the fragment in the page's layout (source mapping).
    pub frag: usize,
}

fn collect_runs(b: &LayoutBox, out: &mut Vec<TextRun>) {
    for line in &b.lines {
        for f in &line.fragments {
            if let FragmentKind::Text { text, font, size, letter_spacing, word_spacing, hidden, src, .. } = &f.kind {
                if *hidden || text.is_empty() {
                    continue;
                }
                out.push(TextRun {
                    rect: f.rect,
                    baseline: f.baseline,
                    node: f.node,
                    text: text.clone(),
                    font: *font,
                    size: *size,
                    letter_spacing: *letter_spacing,
                    word_spacing: *word_spacing,
                    src: rabbithole_layout::tree::text_src(*src),
                    frag: f as *const rabbithole_layout::tree::Fragment as usize,
                });
            }
        }
    }
    for c in &b.children {
        collect_runs(c, out);
    }
}

/// X offset (from the run's left edge) of the character boundary at
/// byte index `idx`.
fn x_at(fonts: &FontContext, run: &TextRun, idx: usize) -> f32 {
    let prefix = &run.text[..boundary_before(&run.text, idx)];
    if prefix.is_empty() {
        return 0.0;
    }
    fonts.measure(run.font, run.size, prefix) + run.letter_spacing * prefix.chars().count() as f32 + rabbithole_layout::tree::word_spacing_before(prefix, prefix.len(), run.word_spacing)
}

/// Caret offsets can outlive an edit or refer to a different display string.
/// Clamp them to an actual UTF-8 boundary before slicing rendered text.
fn boundary_before(text: &str, idx: usize) -> usize {
    let mut idx = idx.min(text.len());
    while !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// Byte index of the character boundary nearest to x (run-relative).
fn index_at(fonts: &FontContext, run: &TextRun, x: f32) -> usize {
    if x <= 0.0 {
        return 0;
    }
    let mut best = run.text.len();
    let mut best_d = (x - run.rect.w).abs();
    let mut prev = 0.0;
    for (i, ch) in run.text.char_indices() {
        let next = x_at(fonts, run, i + ch.len_utf8());
        let mid = (prev + next) / 2.0;
        let d = (x - mid).abs();
        if x < mid {
            let d0 = (x - prev).abs();
            if d0 < best_d {
                best = i;
            }
            return best;
        }
        if d < best_d {
            best_d = d;
            best = i + ch.len_utf8();
        }
        prev = next;
    }
    best
}

/// Position in reading order: (run index, byte offset).
fn locate(fonts: &FontContext, runs: &[TextRun], x: f32, y: f32) -> (usize, usize) {
    if runs.is_empty() {
        return (0, 0);
    }
    // Prefer the run whose line contains y; otherwise the nearest line.
    let mut best = 0usize;
    let mut best_score = f32::MAX;
    for (i, r) in runs.iter().enumerate() {
        let dy = if y < r.rect.y {
            r.rect.y - y
        } else if y > r.rect.bottom() {
            y - r.rect.bottom()
        } else {
            0.0
        };
        let dx = if x < r.rect.x {
            r.rect.x - x
        } else if x > r.rect.right() {
            x - r.rect.right()
        } else {
            0.0
        };
        let score = dy * 1000.0 + dx;
        if score < best_score {
            best_score = score;
            best = i;
        }
    }
    let run = &runs[best];
    let idx = if y < run.rect.y {
        0
    } else if y > run.rect.bottom() {
        run.text.len()
    } else {
        index_at(fonts, run, x - run.rect.x)
    };
    (best, idx)
}

impl Page {
    /// All text runs in paint order.
    pub fn text_runs(&self) -> Vec<TextRun> {
        let mut out = Vec::new();
        collect_runs(&self.layout.root, &mut out);
        out
    }

    /// Case-insensitive find: highlight rects for every match (one rect
    /// per match within a run; matches spanning runs are not found).
    pub fn find_text(&self, fonts: &FontContext, query: &str) -> Vec<Rect> {
        let q = query.to_lowercase();
        if q.trim().is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for run in self.text_runs() {
            let lower = run.text.to_lowercase();
            // Byte offsets differ between `lower` and `text` for some
            // scripts; map via char indices.
            let chars: Vec<(usize, char)> = run.text.char_indices().collect();
            let lower_chars: Vec<char> = lower.chars().collect();
            let qc: Vec<char> = q.chars().collect();
            if lower_chars.len() != chars.len() || qc.is_empty() {
                continue;
            }
            let mut i = 0;
            while i + qc.len() <= lower_chars.len() {
                if lower_chars[i..i + qc.len()] == qc[..] {
                    let s = chars[i].0;
                    let e = if i + qc.len() < chars.len() { chars[i + qc.len()].0 } else { run.text.len() };
                    let x0 = run.rect.x + x_at(fonts, &run, s);
                    let x1 = run.rect.x + x_at(fonts, &run, e);
                    out.push(Rect::new(x0, run.rect.y, (x1 - x0).max(1.0), run.rect.h));
                    i += qc.len();
                } else {
                    i += 1;
                }
            }
        }
        out
    }

    /// Caret x position for a byte offset in the control's value (relative
    /// to its border box), using the control's first text run.
    pub fn caret_x(&self, fonts: &FontContext, control: NodeId, byte_idx: usize) -> Option<f32> {
        let run = self.text_runs().into_iter().find(|r| r.node == Some(control))?;
        let bx = self.rect_of(control)?.x;
        let display_idx = if crate::forms::control_kind(&self.doc, control) == crate::forms::ControlKind::Password {
            // Layout masks each source character with a bullet. Source byte
            // offsets (e.g. 1 for ASCII) cannot index that three-byte glyph.
            let value = self.doc.control_value(control).unwrap_or("");
            let count = value[..boundary_before(value, byte_idx)].chars().count();
            run.text.char_indices().nth(count).map_or(run.text.len(), |(idx, _)| idx)
        } else {
            byte_idx
        };
        Some(run.rect.x - bx + x_at(fonts, &run, display_idx))
    }
}

/// How a press starts a selection under `user-select` (CSS UI 4 §6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectStart {
    /// Nothing selectable there (`user-select: none`, no text).
    None,
    /// A caret at this point; drags extend from it.
    At(BoundaryPoint),
    /// `user-select: all`: the whole element (anchor, focus).
    All(BoundaryPoint, BoundaryPoint),
}

impl Page {
    fn user_select_of(&self, node: NodeId) -> UserSelect {
        let mut cur = Some(node);
        while let Some(n) = cur {
            if let Some(s) = self.styles.get(n) {
                return s.user_select;
            }
            cur = self.doc.flat_parent(n).or_else(|| self.doc.parent(n));
        }
        UserSelect::Auto
    }

    /// The outermost `user-select: all` element containing `node`.
    fn select_all_root(&self, node: NodeId) -> Option<NodeId> {
        let mut found = None;
        let mut cur = Some(node);
        while let Some(n) = cur {
            if self.doc.element(n).is_some() && self.styles.get(n).is_some_and(|s| s.user_select == UserSelect::All) {
                found = Some(n);
            }
            cur = self.doc.parent(n);
        }
        found
    }

    /// The nearest `user-select: contain` element containing `node` (a
    /// selection started inside it stays inside it); editable text
    /// controls contain their selection too.
    pub fn select_contain_root(&self, node: NodeId) -> Option<NodeId> {
        let mut cur = Some(node);
        while let Some(n) = cur {
            if self.doc.element(n).is_some() && self.styles.get(n).is_some_and(|s| s.user_select == UserSelect::Contain) {
                return Some(n);
            }
            cur = self.doc.parent(n);
        }
        None
    }

    /// The DOM position (text node, UTF-16 offset) of a run's byte index.
    fn run_position(&self, run: &TextRun, byte: usize) -> Option<BoundaryPoint> {
        let src = run.src?;
        let sources = rabbithole_layout::selection::fragment_sources(&self.layout.root, &self.doc, &|n| n == src);
        let offs = sources.get(&run.frag)?;
        let chars = run.text[..boundary_before(&run.text, byte)].chars().count();
        Some(BoundaryPoint { node: src, offset: offs.get(chars).copied().unwrap_or(0) as usize })
    }

    /// The caret position (DOM boundary point) nearest to a document
    /// point, over selectable text (`caretPositionFromPoint`).
    pub fn position_at(&self, fonts: &FontContext, x: f32, y: f32) -> Option<BoundaryPoint> {
        let runs: Vec<TextRun> = self.text_runs().into_iter().filter(|r| r.src.is_some_and(|t| self.user_select_of(t) != UserSelect::None)).collect();
        if runs.is_empty() {
            return None;
        }
        let (ri, idx) = locate(fonts, &runs, x, y);
        self.run_position(&runs[ri], idx)
    }

    /// Where a press at a document point starts a selection.
    pub fn selection_start_at(&self, fonts: &FontContext, x: f32, y: f32, target: Option<NodeId>) -> SelectStart {
        if let Some(t) = target {
            if self.user_select_of(t) == UserSelect::None {
                return SelectStart::None;
            }
            if let Some(root) = self.select_all_root(t) {
                let len = self.doc.node_length(root);
                return SelectStart::All(BoundaryPoint { node: root, offset: 0 }, BoundaryPoint { node: root, offset: len });
            }
        }
        match self.position_at(fonts, x, y) {
            Some(p) => match self.select_all_root(p.node) {
                Some(root) => SelectStart::All(BoundaryPoint { node: root, offset: 0 }, BoundaryPoint { node: root, offset: self.doc.node_length(root) }),
                None => SelectStart::At(p),
            },
            None => SelectStart::None,
        }
    }

    /// The word around a document point (double click): (start, end).
    pub fn word_range_at(&self, fonts: &FontContext, x: f32, y: f32) -> Option<(BoundaryPoint, BoundaryPoint)> {
        let p = self.position_at(fonts, x, y)?;
        let text: Vec<u16> = self.doc.text(p.node)?.encode_utf16().collect();
        let is_word = |u: u16| char::from_u32(u as u32).is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '\'') || (0xD800..0xE000).contains(&u);
        let mut s = p.offset.min(text.len());
        let mut e = s;
        // On a separator: select it alone when there is no word after it.
        if e < text.len() && !is_word(text[e]) && (s == 0 || !is_word(text[s - 1])) {
            return Some((BoundaryPoint { node: p.node, offset: e }, BoundaryPoint { node: p.node, offset: e + 1 }));
        }
        while s > 0 && is_word(text[s - 1]) {
            s -= 1;
        }
        while e < text.len() && is_word(text[e]) {
            e += 1;
        }
        Some((BoundaryPoint { node: p.node, offset: s }, BoundaryPoint { node: p.node, offset: e }))
    }

    /// The block (paragraph) around a document point (triple click).
    pub fn block_range_at(&self, fonts: &FontContext, x: f32, y: f32) -> Option<(BoundaryPoint, BoundaryPoint)> {
        let p = self.position_at(fonts, x, y)?;
        let mut cur = self.doc.parent(p.node);
        while let Some(n) = cur {
            let block = self.styles.get(n).is_some_and(|s| !matches!(s.display, rabbithole_style::Display::Inline | rabbithole_style::Display::Contents));
            if block {
                break;
            }
            cur = self.doc.parent(n);
        }
        let b = cur?;
        Some((BoundaryPoint { node: b, offset: 0 }, BoundaryPoint { node: b, offset: self.doc.node_length(b) }))
    }

    /// Select everything (Ctrl+A outside text controls).
    pub fn select_all(&mut self) {
        let Some(root) = self.doc.body().or_else(|| self.doc.html_element()) else { return };
        let len = self.doc.node_length(root);
        self.doc.set_selection(BoundaryPoint { node: root, offset: 0 }, BoundaryPoint { node: root, offset: len });
    }

    /// Highlight rectangles of the current selection (document coordinates).
    pub fn selection_rects(&self, fonts: &FontContext) -> Vec<Rect> {
        self.selected_runs(fonts).into_iter().map(|(r, ..)| r).collect()
    }

    /// The selected part of every selected run, in paint order: its
    /// rectangle and text.
    fn selected_runs(&self, fonts: &FontContext) -> Vec<(Rect, TextRun, usize, usize)> {
        let Some(sel) = self.doc.selection().filter(|s| !s.is_collapsed()) else { return Vec::new() };
        let texts: std::collections::HashMap<NodeId, (usize, usize)> = self.doc.text_in_range(sel.start, sel.end).into_iter().filter(|(t, _, _)| self.user_select_of(*t) != UserSelect::None).map(|(n, s, e)| (n, (s, e))).collect();
        if texts.is_empty() {
            return Vec::new();
        }
        let sources = rabbithole_layout::selection::fragment_sources(&self.layout.root, &self.doc, &|n| texts.contains_key(&n));
        let mut out = Vec::new();
        for run in self.text_runs() {
            let Some(src) = run.src else { continue };
            let Some(&(s, e)) = texts.get(&src) else { continue };
            let Some(offs) = sources.get(&run.frag) else { continue };
            let Some((b0, b1)) = rabbithole_layout::selection::selected_bytes(&run.text, offs, s as u32, e as u32) else { continue };
            let x0 = run.rect.x + x_at(fonts, &run, b0);
            let x1 = run.rect.x + x_at(fonts, &run, b1);
            out.push((Rect::new(x0, run.rect.y, (x1 - x0).max(1.0), run.rect.h), run, b0, b1));
        }
        out
    }

    /// The selection as text (what a copy puts on the clipboard): selected
    /// text in rendering order, lines joined with newlines.
    pub fn selection_text(&self, fonts: &FontContext) -> String {
        let runs = self.selected_runs(fonts);
        let mut text = String::new();
        let mut last_bottom: Option<f32> = None;
        for (_, run, b0, b1) in runs {
            let part = &run.text[b0..b1];
            if let Some(lb) = last_bottom {
                if run.rect.y >= lb - 1.0 {
                    while text.ends_with(' ') {
                        text.pop();
                    }
                    text.push('\n');
                }
            }
            text.push_str(if text.ends_with('\n') || text.is_empty() { part.trim_start_matches(' ') } else { part });
            last_bottom = Some(run.rect.bottom());
        }
        text
    }
}

impl Page {
    /// Text nodes of the body in tree order (not inside script, style,
    /// template or head content).
    fn caret_text_nodes(&self) -> Vec<NodeId> {
        let Some(root) = self.doc.body().or_else(|| self.doc.html_element()) else { return Vec::new() };
        let mut out = Vec::new();
        for n in self.doc.descendants(root) {
            if self.doc.text(n).is_some_and(|t| !t.is_empty()) {
                let hidden = self.doc.ancestor_or_self(n, |a| matches!(self.doc.tag(a), Some("script" | "style" | "template" | "head" | "title" | "noscript")));
                if hidden.is_none() {
                    out.push(n);
                }
            }
        }
        out
    }

    /// The caret position one character (or word) before or after `p`,
    /// over the document's text (keyboard selection: shift+arrows).
    pub fn moved_position(&self, p: BoundaryPoint, forward: bool, word: bool) -> BoundaryPoint {
        let texts = self.caret_text_nodes();
        if texts.is_empty() {
            return p;
        }
        let units = |n: NodeId| -> Vec<u16> { self.doc.text(n).unwrap_or("").encode_utf16().collect() };
        // Start: the text position at or after p.
        let (mut i, mut o) = match texts.iter().position(|&t| t == p.node) {
            Some(i) => (i, p.offset),
            None => match texts.iter().position(|&t| self.doc.compare_points(BoundaryPoint { node: t, offset: self.doc.node_length(t) }, p) != std::cmp::Ordering::Less) {
                Some(i) => (i, 0),
                None => (texts.len() - 1, self.doc.node_length(texts[texts.len() - 1])),
            },
        };
        let mut data = units(texts[i]);
        // One step: the unit crossed, if any.
        let mut step = |i: &mut usize, o: &mut usize, data: &mut Vec<u16>| -> Option<u16> {
            if forward {
                while *o >= data.len() {
                    if *i + 1 >= texts.len() {
                        return None;
                    }
                    *i += 1;
                    *o = 0;
                    *data = units(texts[*i]);
                }
                let u = data[*o];
                *o += 1;
                if (0xD800..0xDC00).contains(&u) && *o < data.len() {
                    *o += 1;
                }
                Some(u)
            } else {
                while *o == 0 {
                    if *i == 0 {
                        return None;
                    }
                    *i -= 1;
                    *data = units(texts[*i]);
                    *o = data.len();
                }
                *o -= 1;
                let u = data[*o];
                if (0xDC00..0xE000).contains(&u) && *o > 0 {
                    *o -= 1;
                }
                Some(u)
            }
        };
        let is_word = |u: u16| char::from_u32(u as u32).is_some_and(|c| c.is_alphanumeric() || c == '_') || (0xD800..0xE000).contains(&u);
        if !word {
            step(&mut i, &mut o, &mut data);
        } else {
            // Skip separators, then the word.
            loop {
                let (si, so, sd) = (i, o, data.clone());
                match step(&mut i, &mut o, &mut data) {
                    Some(u) if !is_word(u) => continue,
                    Some(_) => {
                        (i, o, data) = (si, so, sd);
                        break;
                    }
                    None => break,
                }
            }
            // A word ends at its block's edge.
            let block = |n: NodeId| {
                let mut cur = self.doc.parent(n);
                while let Some(e) = cur {
                    if self.styles.get(e).is_some_and(|s| !matches!(s.display, rabbithole_style::Display::Inline | rabbithole_style::Display::Contents)) {
                        return Some(e);
                    }
                    cur = self.doc.parent(e);
                }
                None
            };
            loop {
                let (si, so, sd) = (i, o, data.clone());
                match step(&mut i, &mut o, &mut data) {
                    Some(u) if is_word(u) && (i == si || block(texts[i]) == block(texts[si])) => continue,
                    Some(_) => {
                        (i, o, data) = (si, so, sd);
                        break;
                    }
                    None => break,
                }
            }
        }
        BoundaryPoint { node: texts[i], offset: o }
    }
}
