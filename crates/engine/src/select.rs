//! Text selection and find-in-page over laid out text fragments.

use crate::Page;
use rabbithole_dom::NodeId;
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
}

/// A selection: highlight rectangles plus the selected text.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    pub rects: Vec<Rect>,
    pub text: String,
}

fn collect_runs(b: &LayoutBox, out: &mut Vec<TextRun>) {
    for line in &b.lines {
        for f in &line.fragments {
            if let FragmentKind::Text { text, font, size, letter_spacing, hidden, .. } = &f.kind {
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
    fonts.measure(run.font, run.size, prefix) + run.letter_spacing * prefix.chars().count() as f32
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

    /// Select the text between two document points (in either order).
    pub fn select_between(&self, fonts: &FontContext, a: (f32, f32), b: (f32, f32)) -> Selection {
        let runs = self.text_runs();
        if runs.is_empty() {
            return Selection::default();
        }
        let mut start = locate(fonts, &runs, a.0, a.1);
        let mut end = locate(fonts, &runs, b.0, b.1);
        if start > end {
            std::mem::swap(&mut start, &mut end);
        }
        if start == end {
            return Selection::default();
        }
        let mut sel = Selection::default();
        let mut last_bottom: Option<f32> = None;
        for i in start.0..=end.0 {
            let run = &runs[i];
            let s = if i == start.0 { start.1 } else { 0 };
            let e = if i == end.0 { end.1 } else { run.text.len() };
            if s >= e {
                continue;
            }
            let x0 = run.rect.x + x_at(fonts, run, s);
            let x1 = run.rect.x + x_at(fonts, run, e);
            sel.rects.push(Rect::new(x0, run.rect.y, (x1 - x0).max(1.0), run.rect.h));
            // Insert a newline when the next run is on a lower line.
            if let Some(lb) = last_bottom {
                if run.rect.y >= lb - 1.0 {
                    sel.text.push('\n');
                } else if !sel.text.ends_with(' ') && !run.text[s..e].starts_with(' ') {
                    sel.text.push(' ');
                }
            }
            sel.text.push_str(&run.text[s..e]);
            last_bottom = Some(run.rect.bottom());
        }
        sel
    }

    /// Select the word under a point (double-click).
    pub fn select_word_at(&self, fonts: &FontContext, x: f32, y: f32) -> Selection {
        let runs = self.text_runs();
        if runs.is_empty() {
            return Selection::default();
        }
        let (ri, idx) = locate(fonts, &runs, x, y);
        let run = &runs[ri];
        let text = &run.text;
        let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '\'';
        let mut s = idx.min(text.len());
        while s > 0 {
            let prev = text[..s].chars().next_back().unwrap();
            if !is_word(prev) {
                break;
            }
            s -= prev.len_utf8();
        }
        let mut e = idx.min(text.len());
        for ch in text[e..].chars() {
            if !is_word(ch) {
                break;
            }
            e += ch.len_utf8();
        }
        if s >= e {
            return Selection::default();
        }
        let x0 = run.rect.x + x_at(fonts, run, s);
        let x1 = run.rect.x + x_at(fonts, run, e);
        Selection { rects: vec![Rect::new(x0, run.rect.y, x1 - x0, run.rect.h)], text: text[s..e].to_string() }
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
