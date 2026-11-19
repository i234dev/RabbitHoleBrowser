//! Inline formatting: line breaking, fragment placement, vertical alignment.

use crate::block::{Cb, Layouter};
use crate::float::Bfc;
use crate::linebreak as lb;
use crate::text::{segment_tabs, transform_style, Collapser, Seg};
use crate::tree::{is_word_separator, DecorationInfo, Fragment, FragmentKind, InlineItem, LayoutBox, LineBox};
use rabbithole_dom::NodeId;
use rabbithole_gfx::{FontId, Rect};
use rabbithole_style::{
    ComputedStyle, Direction, ListStylePosition, OverflowWrap, TextAlign, TextOverflow, UnicodeBidi, VerticalAlign, Visibility, WordBreak,
};
use std::rc::Rc;

/// How a word that does not fit on the line may be broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BreakPolicy {
    /// `overflow-wrap: normal` + `word-break: normal|keep-all`: never split
    /// inside a word (it overflows).
    Never,
    /// `overflow-wrap: anywhere|break-word` / `word-break: break-word`:
    /// split only when the word does not fit on a line of its own.
    IfNeeded,
}

fn break_policy(style: &ComputedStyle) -> BreakPolicy {
    match style.word_break {
        WordBreak::BreakWord | WordBreak::AutoPhrase => BreakPolicy::IfNeeded,
        _ => match style.overflow_wrap {
            OverflowWrap::Normal => BreakPolicy::Never,
            _ => BreakPolicy::IfNeeded,
        },
    }
}

/// Pseudo item index for the list marker rendered inside.
pub(crate) const MARKER_ITEM: usize = usize::MAX;

/// A measured piece of inline content.
#[derive(Debug, Clone)]
pub(crate) enum Piece {
    Word {
        text: Rc<str>,
        width: f32,
        item: usize,
        break_before: bool,
    },
    Space {
        width: f32,
        item: usize,
    },
    Open(usize),
    Close(usize),
    /// An atomic inline; whether the line may break before it.
    Atomic(usize, bool),
    Float(usize),
    Abs(usize),
    Break,
    /// Extra space before the next word (`text-autospace`).
    Gap(f32),
}

/// Font metrics of a style, with the half-leading applied.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TextMetrics {
    pub asc: f32,
    pub desc: f32,
    pub above: f32,
    pub below: f32,
    pub font: FontId,
    pub size: f32,
}

pub(crate) fn item_style<'b>(items: &'b [InlineItem], container: &'b Rc<ComputedStyle>, idx: usize) -> &'b Rc<ComputedStyle> {
    if idx == MARKER_ITEM {
        return container;
    }
    match &items[idx] {
        InlineItem::Text { style, .. } | InlineItem::Open { style, .. } => style,
        _ => container,
    }
}

fn item_elem(items: &[InlineItem], idx: usize) -> Option<NodeId> {
    match items.get(idx) {
        Some(InlineItem::Text { elem, .. }) => *elem,
        Some(InlineItem::Open { node, .. }) => *node,
        _ => None,
    }
}

/// `position: relative` offsets of inline elements (CSS 2.1 §9.4.3),
/// accumulated through nested ones: per item, and per atomic child index.
/// Empty when no inline element is relatively positioned.
fn relative_offsets(items: &[InlineItem], children: usize, cbw: f32, cbh: Option<f32>) -> (Vec<(f32, f32)>, Vec<(f32, f32)>) {
    if !items.iter().any(|it| matches!(it, InlineItem::Open { style, .. } if style.position == rabbithole_style::Position::Relative)) {
        return (Vec::new(), Vec::new());
    }
    let mut out = vec![(0.0, 0.0); items.len()];
    let mut kids = vec![(0.0, 0.0); children];
    let mut stack: Vec<(f32, f32)> = vec![(0.0, 0.0)];
    for (i, it) in items.iter().enumerate() {
        let cur = *stack.last().unwrap_or(&(0.0, 0.0));
        match it {
            InlineItem::Open { style, .. } => {
                let mut o = cur;
                if style.position == rabbithole_style::Position::Relative {
                    let s = &style.inset;
                    o.0 += s.left.resolve(cbw).or_else(|| s.right.resolve(cbw).map(|v| -v)).unwrap_or(0.0);
                    o.1 += s.top.resolve_definite(cbh).or_else(|| s.bottom.resolve_definite(cbh).map(|v| -v)).unwrap_or(0.0);
                }
                out[i] = o;
                stack.push(o);
            }
            InlineItem::Close => {
                out[i] = cur;
                if stack.len() > 1 {
                    stack.pop();
                }
            }
            InlineItem::Atomic(c) | InlineItem::Float(c) | InlineItem::Absolute(c) => {
                out[i] = cur;
                if let Some(k) = kids.get_mut(*c) {
                    *k = cur;
                }
            }
            _ => out[i] = cur,
        }
    }
    (out, kids)
}

/// A decoration extent waiting for its decorating box's whole inline
/// size (see `InlineLayout::decoration_extents`); offsets relative to the
/// fragment's left edge.
#[derive(Debug, Clone)]
struct InsetFixup {
    line: usize,
    frag: usize,
    deco: usize,
    open: usize,
    lo: f32,
    hi: f32,
    w: f32,
    inset: (Option<rabbithole_style::Length>, Option<rabbithole_style::Length>),
}

/// The inline element (Open item) directly enclosing each item, when some
/// inline element insets its text decorations (`text-decoration-inset`);
/// empty otherwise.
fn has_inset(s: &ComputedStyle) -> bool {
    s.text_decoration.any() && (s.text_decoration_inset.0 != rabbithole_style::Length::ZERO || s.text_decoration_inset.1 != rabbithole_style::Length::ZERO)
}

/// Pseudo item index standing for the block container's root inline box
/// (a block decorating box insets its decorations per paragraph).
const BLOCK_BOX: usize = usize::MAX - 1;

fn open_parents(items: &[InlineItem], container: &ComputedStyle) -> Vec<Option<usize>> {
    if !has_inset(container) && !items.iter().any(|it| matches!(it, InlineItem::Open { style, .. } if has_inset(style))) {
        return Vec::new();
    }
    let mut out = vec![None; items.len()];
    let mut stack: Vec<usize> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        out[i] = stack.last().copied();
        match it {
            InlineItem::Open { .. } => stack.push(i),
            InlineItem::Close => {
                stack.pop();
            }
            _ => {}
        }
    }
    out
}

/// The emphasis marks of text in `style`, with the side resolved in the
/// line's logical space (vertical lines are laid out turned: the right
/// side is the logical top in `vertical-rl`, the bottom in `vertical-lr`
/// and `sideways-lr`).
pub(crate) fn emphasis_of(style: &ComputedStyle) -> Option<crate::tree::Emphasis> {
    let vertical = style.writing_mode.is_vertical();
    let mark = style.text_emphasis_style.mark(vertical)?;
    let over = if !vertical {
        style.text_emphasis_over
    } else if style.writing_mode == rabbithole_style::WritingMode::VerticalRl {
        style.text_emphasis_right
    } else {
        !style.text_emphasis_right
    };
    Some(crate::tree::Emphasis { mark: Rc::from(mark.as_str()), color: style.text_emphasis_color.unwrap_or(style.color), over })
}

/// Combining marks and variation selectors: they belong to the preceding
/// character, which stays the one that decides spacing and breaks.
fn attaches_to_previous(c: char) -> bool {
    matches!(
        c as u32,
        0x300..=0x36F
            | 0x0591..=0x05BD
            | 0x05BF
            | 0x05C1..=0x05C2
            | 0x05C4..=0x05C5
            | 0x05C7
            | 0x064B..=0x065F
            | 0x0670
            | 0x06D6..=0x06DC
            | 0x06DF..=0x06E4
            | 0x06E7..=0x06E8
            | 0x06EA..=0x06ED
            | 0x1AB0..=0x1AFF
            | 0x1DC0..=0x1DFF
            | 0x20D0..=0x20FF
            | 0xFE20..=0xFE2F
            | 0x180B..=0x180F
            | 0xFE00..=0xFE0F
            | 0xE0100..=0xE01EF
    )
}

/// `text-autospace` applies between `a` and `b`: an ideograph next to a
/// letter or digit of another script.
fn autospace_between(a: char, b: char) -> bool {
    fn ideo(c: char) -> bool {
        matches!(c as u32, 0x3041..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x3FFFD)
    }
    fn other(c: char) -> bool {
        c.is_alphanumeric() && (c as u32) < 0x2E80
    }
    (ideo(a) && other(b)) || (other(a) && ideo(b))
}

/// Does `hyphens: auto` hyphenate this text (English, wrapping, no
/// `line-break: anywhere` / `word-break: auto-phrase`)?
fn auto_hyphens(style: &ComputedStyle) -> bool {
    style.hyphens == 2
        && style.lang == rabbithole_style::LANG_EN
        && style.white_space.wraps()
        && style.line_break != 3
        && style.word_break != WordBreak::AutoPhrase
}

/// The transformed texts of the items, with soft hyphens inserted at the
/// hyphenation points of the English words (runs of ASCII letters, across
/// inline element boundaries) whose letters all hyphenate; `None` for the
/// items left unchanged.
fn hyphenate_items(items: &[InlineItem]) -> Vec<Option<String>> {
    let texts: Vec<Option<String>> = items
        .iter()
        .map(|it| match it {
            InlineItem::Text { text, style, .. } => Some(transform_style(text, style)),
            _ => None,
        })
        .collect();
    // Insertion points: (item, byte offset).
    let mut inserts: Vec<(usize, usize)> = Vec::new();
    // The current word: (item, byte offset) of each letter.
    let mut word: Vec<(usize, usize)> = Vec::new();
    let mut letters = String::new();
    let mut glued_before = false;
    let mut pts = Vec::new();
    let mut flush = |word: &mut Vec<(usize, usize)>, letters: &mut String, glued: bool, inserts: &mut Vec<(usize, usize)>| {
        if !glued && word.len() >= 5 && word.iter().all(|(i, _)| matches!(&items[*i], InlineItem::Text { style, .. } if auto_hyphens(style))) {
            lb::hyphenate_en(letters, &mut pts);
            inserts.extend(pts.iter().map(|&p| word[p]));
        }
        word.clear();
        letters.clear();
    };
    for (i, it) in items.iter().enumerate() {
        match it {
            InlineItem::Text { .. } => {
                let t = texts[i].as_deref().unwrap_or("");
                for (b, c) in t.char_indices() {
                    if c.is_ascii_alphabetic() {
                        word.push((i, b));
                        letters.push(c);
                    } else if c == '\u{ad}' {
                        // Words with soft hyphens are not hyphenated
                        // automatically.
                        glued_before = true;
                    } else {
                        let glued = c.is_ascii_digit() || (c as u32 >= 0x80 && c.is_alphanumeric());
                        if !word.is_empty() {
                            flush(&mut word, &mut letters, glued_before || glued, &mut inserts);
                        }
                        glued_before = glued;
                    }
                }
            }
            InlineItem::Open { .. } | InlineItem::Close | InlineItem::Absolute(_) | InlineItem::Float(_) => {}
            _ => {
                if !word.is_empty() {
                    flush(&mut word, &mut letters, glued_before, &mut inserts);
                }
                glued_before = false;
            }
        }
    }
    if !word.is_empty() {
        flush(&mut word, &mut letters, glued_before, &mut inserts);
    }
    let mut out: Vec<Option<String>> = vec![None; items.len()];
    inserts.sort_unstable();
    let mut k = 0;
    while k < inserts.len() {
        let i = inserts[k].0;
        let t = texts[i].as_deref().unwrap_or("");
        let mut s = String::with_capacity(t.len() + 8);
        let mut last = 0;
        while k < inserts.len() && inserts[k].0 == i {
            let b = inserts[k].1;
            s.push_str(&t[last..b]);
            s.push('\u{ad}');
            last = b;
            k += 1;
        }
        s.push_str(&t[last..]);
        out[i] = Some(s);
    }
    out
}

/// Characters that glue to an adjacent atomic inline (CSS Text 3 §5.1):
/// GL (but NBSP), WJ and ZWJ.
fn glues_atomic(c: char) -> bool {
    c != ' ' && matches!(lb::class(c), lb::Lb::GL | lb::Lb::WJ | lb::Lb::ZWJ)
}


/// Default-ignorable format characters (bidi controls, joiners): they
/// form no justification opportunity of their own.
fn is_default_ignorable(c: char) -> bool {
    matches!(c as u32, 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x206F | 0xFEFF | 0xAD)
}

/// Scripts whose letters join: never separated by inter-character
/// justification (CSS Text 3 §7.4).
fn is_cursive(c: char) -> bool {
    matches!(c as u32, 0x0600..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF | 0x1800..=0x18AF | 0xA840..=0xA87F | 0x10D00..=0x10D3F)
}

/// Is there a justification opportunity between the letter units `p` and
/// `c` under `text-justify` `mode` (0 auto, 1 none, 2 inter-word,
/// 3 inter-character)? `auto` also expands around ideographs.
fn justify_between(p: char, c: char, mode: u8) -> bool {
    fn ideo(c: char) -> bool {
        matches!(c as u32, 0x2E80..=0x2FFF | 0x3001..=0x303F | 0x3041..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFF01..=0xFF60 | 0x20000..=0x3FFFD)
    }
    match mode {
        1 => false,
        2 => is_word_separator(p),
        3 => !(is_cursive(p) && is_cursive(c)),
        _ => is_word_separator(p) || ((ideo(p) || ideo(c)) && !is_word_separator(c)),
    }
}

impl<'a> Layouter<'a> {
    pub(crate) fn text_metrics(&mut self, style: &ComputedStyle) -> TextMetrics {
        let font = self.font_for(style);
        let size = style.font_size;
        let m = self.ctx.fonts.metrics(font, size);
        let lh = if style.line_height_tight { m.ascent + m.descent } else { style.line_height_px() };
        let half = (lh - (m.ascent + m.descent)) / 2.0;
        TextMetrics {
            asc: m.ascent,
            desc: m.descent,
            above: m.ascent + half,
            below: m.descent + half,
            font,
            size,
        }
    }

    /// The advance of a preserved tab at `pos` (from the line start) to
    /// the next tab stop (CSS Text 3 §4.2): stops are `tab-size` apart,
    /// a number of spaces measuring the block container's space advance
    /// with its letter and word spacing; a stop closer than 0.5ch is
    /// skipped.
    pub(crate) fn tab_width(&mut self, block: &ComputedStyle, style: &ComputedStyle, pos: f32) -> f32 {
        let font = self.font_for(block);
        let space = self.measurer.space_width(font, block.font_size) + block.letter_spacing + block.word_spacing;
        let tab = if style.tab_size_length { style.tab_size } else { style.tab_size * space };
        if tab <= 0.0 {
            return 0.0;
        }
        let half_ch = 0.5 * block.font_size * rabbithole_style::values::font_units(block).0;
        let mut next = ((pos + 0.01) / tab).floor() * tab + tab;
        if next - pos < half_ch {
            next = ((pos + half_ch) / tab).ceil() * tab;
        }
        (next - pos).max(0.0)
    }

    /// Build (and cache) the measured pieces of an inline container.
    pub(crate) fn ensure_pieces(&mut self, b: &mut LayoutBox) {
        if b.pieces.is_some() {
            return;
        }
        let pieces = self.build_pieces(&b.items, &b.style);
        b.pieces = Some(pieces);
    }

    fn build_pieces(&mut self, items: &[InlineItem], container: &Rc<ComputedStyle>) -> Vec<Piece> {
        let container_wraps = container.white_space.wraps();
        let mut col = Collapser::new();
        let mut stack: Vec<usize> = Vec::new();
        let mut pieces = Vec::new();
        let mut prev_break_opp = true;
        // Last character of the previous word (breaks between ideographs
        // need no space).
        let mut prev_char: Option<char> = None;
        // The last character before the current spaces, when a space ends
        // the previous word (its break may be forbidden by what follows).
        let mut last_char: Option<char> = None;
        let mut space_opp: Option<Option<char>> = None;
        let mut points = Vec::new();
        let mut after_atomic = false;
        let mut segs = Vec::new();
        // `hyphens: auto`: the texts with soft hyphens at the hyphenation
        // points of their English words (which may span inline elements).
        let mut hyphenated = if items.iter().any(|it| matches!(it, InlineItem::Text { style, .. } if auto_hyphens(style))) {
            Some(hyphenate_items(items))
        } else {
            None
        };
        for (i, item) in items.iter().enumerate() {
            match item {
                InlineItem::Text { text, style, .. } => {
                    let font = self.font_for(style);
                    let size = style.font_size;
                    let ws = style.white_space;
                    let wraps = ws.wraps();
                    let t = match hyphenated.as_mut().and_then(|h| h[i].take()) {
                        Some(t) => t,
                        None => transform_style(text, style),
                    };
                    segs.clear();
                    let keep_shy = style.hyphens != 0;
                    segment_tabs(&t, ws, &mut col, &mut segs, keep_shy);
                    let ls = style.letter_spacing;
                    let rules = lb::Rules {
                        line_break: if style.line_break == 3 && style.wrap_inside_avoid { 0 } else { style.line_break },
                        word_break: match style.word_break {
                            rabbithole_style::WordBreak::KeepAll => 1,
                            rabbithole_style::WordBreak::AutoPhrase => 2,
                            rabbithole_style::WordBreak::BreakAll => 3,
                            _ => 0,
                        },
                        cjk: matches!(style.lang, rabbithole_style::LANG_JA | rabbithole_style::LANG_ZH),
                        break_spaces: ws == rabbithole_style::WhiteSpace::BreakSpaces,
                    };
                    let anywhere = rules.line_break == 3;

                    let full_width = style.text_transform_extra & 1 != 0;
                    for seg in segs.drain(..) {
                        // `full-width` turns the collapsed spaces into U+3000.
                        let seg = if full_width && seg == Seg::Space { Seg::Word("\u{3000}".into()) } else { seg };
                        match seg {
                            Seg::Word(w) => {
                                let w = if style.text_transform_extra & 1 != 0 && w.contains(' ') { w.replace(' ', "\u{3000}") } else { w };
                                let is_space_word = w.chars().all(|c| c == ' ' || c == '\t');

                                // A break between the previous word and this
                                // one when no space separates them
                                // (ideographs, punctuation)...
                                if wraps && !prev_break_opp {
                                    if let (Some(p), Some(c)) = (prev_char, w.chars().next()) {
                                        let something = Some(lb::Unit { c: 'a', cls: lb::Lb::AL });
                                        // `anywhere` breaks around preserved spaces too.
                                        if (anywhere && !attaches_to_previous(c)) || (p != ' ' && c != ' ' && lb::direct_break(&rules, something, rules.unit(p), rules.unit(c), w.chars().nth(1))) {
                                            prev_break_opp = true;
                                        }
                                    }
                                }
                                // ... and none at the spaces before closing
                                // punctuation or after opening punctuation
                                // (UAX #14 LB13-LB17).
                                if let (Some(before), Some(c)) = (space_opp.take(), w.chars().next()) {
                                    if prev_break_opp && !anywhere && c != ' ' && c != '\t' && !lb::break_after_spaces(&rules, before, c) {
                                        prev_break_opp = false;
                                    }
                                }
                                if std::mem::take(&mut after_atomic) && w.chars().next().is_some_and(glues_atomic) {
                                    prev_break_opp = false;
                                }
                                let prev_before = prev_char;
                                prev_char = w.chars().rev().find(|&c| !attaches_to_previous(c)).or(w.chars().last());
                                let mut parts: Vec<(String, bool)> = Vec::new();
                                if wraps && !is_space_word {
                                    lb::split_points(&rules, &w, &mut points);
                                    let mut start = 0;
                                    for &p in points.iter().chain(std::iter::once(&w.len())) {
                                        parts.push((w[start..p].to_string(), start > 0));
                                        start = p;
                                    }
                                } else {
                                    parts.push((w, false));
                                }
                                // `text-autospace`: 1/8ic between ideographs and
                                // other letters or digits.
                                let gap = if style.text_autospace && !is_space_word && !style.writing_mode.is_vertical() {
                                    style.font_size * rabbithole_style::values::font_units(style).2 / 8.0
                                } else {
                                    0.0
                                };
                                let mut gapped: Vec<(String, bool, bool)> = Vec::new();
                                let mut last = prev_before;
                                for (part, continuation) in parts {
                                    if gap > 0.0 {
                                        let mut cur = String::new();
                                        let mut first = true;
                                        for c in part.chars() {
                                            let g = last.is_some_and(|p| autospace_between(p, c));
                                            if g && !cur.is_empty() {
                                                gapped.push((std::mem::take(&mut cur), continuation || !first, false));
                                                first = false;
                                            }
                                            if g && cur.is_empty() {
                                                // Mark the gap before this sub-part.
                                                gapped.push((String::new(), false, true));
                                            }
                                            cur.push(c);
                                            if !attaches_to_previous(c) {
                                                last = Some(c);
                                            }
                                        }
                                        if !cur.is_empty() {
                                            gapped.push((cur, continuation || !first, false));
                                        }
                                    } else {
                                        gapped.push((part, continuation, false));
                                    }
                                }
                                // The parts of a word in a joining script measure
                                // as they are shaped together: by prefix widths.
                                let joined = gapped.len() > 1 && gapped.iter().any(|g| g.0.chars().any(is_cursive));
                                let mut prefix = String::new();
                                let mut prefix_w = 0.0;
                                for (part, continuation, is_gap) in gapped {
                                    if is_gap {
                                        pieces.push(Piece::Gap(gap));
                                        continue;
                                    }
                                    let width = if part == "\t" {
                                        self.tab_width(container, style, 0.0)
                                    } else if joined {
                                        prefix.push_str(&part);
                                        let pw = self.measurer.width(font, size, &prefix);
                                        let w = pw - prefix_w + ls * part.chars().count() as f32;
                                        prefix_w = pw;
                                        w
                                    } else {
                                        self.measurer.width(font, size, &part) + ls * part.chars().count() as f32
                                    };
                                    // A break opportunity before a word comes from the
                                    // preceding whitespace (owned by whatever element
                                    // produced it); `nowrap` on this word only forbids
                                    // breaks at its own spaces.
                                    let bb = prev_break_opp || (wraps && continuation);
                                    pieces.push(Piece::Word {
                                        text: Rc::from(part.as_str()),
                                        width,
                                        item: i,
                                        break_before: bb,
                                    });
                                    prev_break_opp = false;
                                }
                                prev_break_opp = is_space_word && wraps;
                                if prev_break_opp {
                                    space_opp = Some(last_char);
                                } else {
                                    last_char = prev_char;
                                }
                            }
                            Seg::Space => {
                                let width = self.measurer.space_width(font, size) + style.word_spacing;
                                pieces.push(Piece::Space { width, item: i });
                                prev_break_opp = wraps;
                                if wraps {
                                    space_opp = Some(last_char);
                                }
                                prev_char = Some(' ');
                            }
                            Seg::Opportunity => {
                                prev_break_opp = wraps;
                                prev_char = None;
                                space_opp = None;
                                last_char = None;
                            }
                            Seg::Break => {
                                pieces.push(Piece::Break);
                                prev_break_opp = true;
                                prev_char = None;
                                space_opp = None;
                                last_char = None;
                            }
                        }
                    }
                }
                InlineItem::Open { .. } => {
                    stack.push(i);
                    pieces.push(Piece::Open(i));
                }
                InlineItem::Close => {
                    if let Some(o) = stack.pop() {
                        pieces.push(Piece::Close(o));
                    }
                }
                InlineItem::Atomic(c) => {
                    // Atomic inlines break on both sides, except next to
                    // glue (GL but NBSP, WJ, ZWJ).
                    let before = container_wraps && !prev_char.is_some_and(glues_atomic);
                    prev_char = None;
                    space_opp = None;
                    last_char = None;
                    pieces.push(Piece::Atomic(*c, before));
                    prev_break_opp = container_wraps;
                    after_atomic = true;
                    // An atomic box interrupts the whitespace sequence, so a
                    // following space is not collapsed away.
                    col.prev_space = false;
                }
                InlineItem::Float(c) => pieces.push(Piece::Float(*c)),
                InlineItem::Absolute(c) => pieces.push(Piece::Abs(*c)),
                InlineItem::Break => {
                    pieces.push(Piece::Break);
                    col.prev_space = true;
                    prev_break_opp = true;
                    space_opp = None;
                    last_char = None;
                }
            }
        }
        pieces
    }

    /// Lay out the inline content of `b`; returns the content height.
    pub(crate) fn layout_inline(&mut self, b: &mut LayoutBox, cb: &Cb, bfc: &mut Bfc) -> f32 {
        self.ensure_pieces(b);
        let mut pieces = b.pieces.take().unwrap_or_default();
        let items = std::mem::take(&mut b.items);
        let cstyle = b.style.clone();
        let mut inserted_marker = false;
        if let Some(mt) = &b.marker_text {
            if cstyle.list_style_position == ListStylePosition::Inside {
                let font = self.font_for(&cstyle);
                let text = format!("{mt} ");
                let width = self.measurer.width(font, cstyle.font_size, &text);
                pieces.insert(
                    0,
                    Piece::Word {
                        text: Rc::from(text.as_str()),
                        width,
                        item: MARKER_ITEM,
                        break_before: false,
                    },
                );
                inserted_marker = true;
            }
        }
        let content = b.content_rect;
        let strut = self.text_metrics(&cstyle);
        let indent = cstyle.text_indent.resolve_or_zero(cb.width);
        let clips = cstyle.overflow_x.clips() || cstyle.overflow_y.clips();
        let ellipsis = cstyle.text_overflow != TextOverflow::Clip && clips;
        // Lines left by the `line-clamp` in progress (this box's own
        // clamp is set up by the caller).
        let outer_clamp = self.clamp.as_ref().map(|c| c.remaining);
        let max_lines = outer_clamp.filter(|n| *n != usize::MAX);
        let ellipsize_final = self.clamp.as_ref().is_some_and(|c| c.ellipsize_last) && cstyle.block_ellipsis.is_some();
        if outer_clamp == Some(0) {
            // Clamped away: no lines.
            if !crate::block::inline_content_is_empty(b) {
                if let Some(c) = self.clamp.as_mut() {
                    c.content_after = true;
                }
            }
            b.items = items;
            b.lines = Vec::new();
            if inserted_marker {
                pieces.remove(0);
            }
            b.pieces = Some(pieces);
            return 0.0;
        }
        let (rel, rel_kids) = relative_offsets(&items, b.children.len(), cb.width, cb.height);
        let parent_open = open_parents(&items, &cstyle);
        let bidi_items = items.iter().any(|it| matches!(it, InlineItem::Open { style, .. } if style.unicode_bidi != UnicodeBidi::Normal));
        // `text-wrap: balance` (CSS Text 4 §6.1.2): per group of lines
        // between forced breaks, the narrowest wrap width that keeps its
        // line count, found by bisection over trial layouts (groups of at
        // most 6 lines; not with floats, which trial layouts would place
        // repeatedly).
        let balance = cstyle.text_wrap_balance && cstyle.white_space.wraps() && !items.iter().any(|it| matches!(it, InlineItem::Float(_)));
        let mut wrap_limits: Vec<Option<f32>> = Vec::new();
        let mut passes = if balance { 14 } else { 0 };
        // Per group: (lo, hi, wanted lines, balanced).
        let mut groups: Vec<(f32, f32, usize, bool)> = Vec::new();
        let mut first_pass = true;
        loop {
        let mut lines = Vec::new();
        let group_stats;
        let clamped;
        let height = {
            let mut il = InlineLayout {
                lay: self,
                children: &mut b.children,
                items: &items,
                cstyle: &cstyle,
                bfc,
                content_x: content.x,
                content_y: content.y,
                content_w: cb.width,
                cb_h: cb.height,
                strut,
                lines: &mut lines,
                cur: Line::default(),
                pending_floats: Vec::new(),
                pending_abs: Vec::new(),
                first_line: true,
                after_forced: false,
                join_next: false,
                indent,
                ellipsis,
                max_lines,
                ellipsize_final,
                final_flush: false,
                done: false,
                bidi_items,
                rel: rel.clone(),
                rel_kids: rel_kids.clone(),
                parent_open: parent_open.clone(),
                inset_seen: std::collections::HashSet::new(),
                inset_fixups: Vec::new(),
                inset_total: std::collections::HashMap::new(),
                wrap_limits: wrap_limits.clone(),
                group: 0,
                group_stats: Vec::new(),
            };
            let h = il.run(&pieces);
            il.resolve_insets();
            group_stats = std::mem::take(&mut il.group_stats);
            clamped = il.done;
            h
        };
        if passes > 0 {
            if first_pass {
                first_pass = false;
                groups = group_stats.iter().map(|&(n, used)| (0.0, used.min(cb.width), n, (2..=6).contains(&n))).collect();
                if clamped || !groups.iter().any(|g| g.3) {
                    passes = 0;
                }
            } else {
                passes -= 1;
                for (g, st) in groups.iter_mut().enumerate().filter(|(_, st)| st.3) {
                    let limit = wrap_limits.get(g).copied().flatten().unwrap_or(f32::MAX);
                    let (n, used) = group_stats.get(g).copied().unwrap_or((usize::MAX, f32::MAX));
                    if !clamped && n <= st.2 && used <= limit + 0.01 {
                        st.1 = limit.min(st.1);
                    } else {
                        st.0 = limit;
                    }
                }
            }
            if passes > 1 && groups.iter().any(|g| g.3 && g.1 - g.0 > 0.5) {
                wrap_limits = groups.iter().map(|g| g.3.then_some(if g.1 - g.0 > 0.5 { (g.0 + g.1) / 2.0 } else { g.1 })).collect();
                continue;
            }
            let finals: Vec<Option<f32>> = groups.iter().map(|g| g.3.then_some(g.1)).collect();
            if passes > 0 && wrap_limits != finals {
                // The final layout at the narrowest widths that fit.
                wrap_limits = finals;
                passes = 0;
                continue;
            }
        }
        if inserted_marker {
            pieces.remove(0);
        }
        b.pieces = Some(pieces);
        b.items = items;
        if let (Some(n), Some(c)) = (outer_clamp, self.clamp.as_mut()) {
            if c.record {
                c.bottoms.extend(lines.iter().map(|l| l.rect.bottom()));
            }
            c.remaining = n.saturating_sub(lines.len());
            if c.remaining == 0 {
                c.bottom = lines.last().map(|l| l.rect.bottom());
                // The clamp line ended this paragraph: an ellipsis is
                // needed only if content follows elsewhere.
                c.final_hit = !clamped;
            }
        }
        b.lines = lines;
        return height;
        }
    }
}

#[derive(Debug, Clone)]
enum PFKind {
    Text { text: String, item: usize },
    Box(usize),
}

#[derive(Debug, Clone)]
struct PF {
    x: f32,
    w: f32,
    kind: PFKind,
}

#[derive(Debug, Clone)]
struct OpenInline {
    item: usize,
    start_x: f32,
    started_here: bool,
}

#[derive(Debug, Clone)]
struct BgFrag {
    x: f32,
    w: f32,
    item: usize,
    left_edge: bool,
    right_edge: bool,
}

#[derive(Debug, Default)]
struct Line {
    y: f32,
    left: f32,
    right: f32,
    x: f32,
    frags: Vec<PF>,
    bg: Vec<BgFrag>,
    pending_space: Option<(f32, usize)>,
    has_content: bool,
    open: Vec<OpenInline>,
    /// Index of the first fragment of the current unbreakable run.
    break_at: Option<usize>,
    /// The line's end for alignment (`right` may be narrower when lines
    /// are balanced).
    align_right: f32,
    /// Space added by justification.
    justified: f32,
    /// Per fragment, the justification space after each word separator
    /// (empty when none).
    frag_ws: Vec<f32>,
}

struct InlineLayout<'l, 'a> {
    lay: &'l mut Layouter<'a>,
    children: &'l mut Vec<LayoutBox>,
    items: &'l [InlineItem],
    cstyle: &'l Rc<ComputedStyle>,
    bfc: &'l mut Bfc,
    content_x: f32,
    content_y: f32,
    content_w: f32,
    cb_h: Option<f32>,
    strut: TextMetrics,
    lines: &'l mut Vec<LineBox>,
    cur: Line,
    pending_floats: Vec<usize>,
    /// Hypothetical block-level absolutes met after inline content: their
    /// static position is below the line they would have broken.
    pending_abs: Vec<usize>,
    first_line: bool,
    /// The previous line ended with a forced break.
    after_forced: bool,
    /// The previous line ended inside a word of a joining script.
    join_next: bool,
    indent: f32,
    /// `text-overflow: ellipsis` applies (overflow clipped).
    ellipsis: bool,
    /// `line-clamp`: lines that may be shown.
    max_lines: Option<usize>,
    /// Ellipsize the clamp line even when it ends the paragraph.
    ellipsize_final: bool,
    /// Set for the last `finish_line` call (no content follows).
    final_flush: bool,
    /// Line clamp reached: remaining content is dropped.
    done: bool,
    /// Some inline element has a `unicode-bidi` other than `normal`.
    bidi_items: bool,
    /// Relative offsets per item / atomic child (see [`relative_offsets`]).
    rel: Vec<(f32, f32)>,
    rel_kids: Vec<(f32, f32)>,
    /// Lines wrap at this width (`text-wrap: balance`) but align in the
    /// full one.
    wrap_limits: Vec<Option<f32>>,
    /// The current group of lines (between forced breaks).
    group: usize,
    /// Per group: line count and widest line content (trailing hanging
    /// spaces and justification excluded).
    group_stats: Vec<(usize, f32)>,
    /// See [`open_parents`].
    parent_open: Vec<Option<usize>>,
    /// Inline elements with decoration insets already met on a line.
    inset_seen: std::collections::HashSet<usize>,
    inset_fixups: Vec<InsetFixup>,
    /// Inline size of the text of each inset decorating box, all lines.
    inset_total: std::collections::HashMap<usize, f32>,
}

impl<'l, 'a> InlineLayout<'l, 'a> {
    fn style(&self, idx: usize) -> &Rc<ComputedStyle> {
        item_style(self.items, self.cstyle, idx)
    }

    /// Per applied decoration of text item `item` (a fragment at `x`, `w`
    /// wide), the line extent relative to `x` when its decorating inline
    /// box insets it (`text-decoration-inset` at the box's start and end).
    /// The extents are resolved once all lines are laid out (percentages
    /// refer to the box's whole inline size): this records them.
    fn decoration_extents(&mut self, item: usize, style: &ComputedStyle, box_ext: &std::collections::HashMap<usize, (f32, f32)>, x: f32, w: f32, frag: usize) -> Vec<Option<(f32, f32)>> {
        let list = &style.applied_decorations;
        let mut chain = Vec::new();
        let mut p = self.parent_open.get(item).copied().flatten();
        while let Some(o) = p {
            if self.style(o).text_decoration.any() {
                chain.push(o);
            }
            p = self.parent_open[o];
        }
        chain.reverse();
        if chain.len() > list.len() {
            return Vec::new();
        }
        let first = list.len() - chain.len();
        for (j, &o) in chain.iter().enumerate() {
            let d = &list[first + j];
            let Some(&(lo, hi)) = box_ext.get(&o) else { continue };
            let starts = !self.inset_seen.contains(&o);
            let ends = !self.cur.open.iter().any(|e| e.item == o);
            self.inset_fixups.push(InsetFixup {
                line: self.lines.len(),
                frag,
                deco: first + j,
                open: o,
                lo: lo - x,
                hi: hi - x,
                w,
                inset: (if starts { Some(d.inset.0) } else { None }, if ends { Some(d.inset.1) } else { None }),
            });
        }
        // The block container's own decoration (the entry before the
        // inline elements' ones).
        if first > 0 && has_inset(self.cstyle) {
            let d = &list[first - 1];
            if d.lines == self.cstyle.text_decoration && d.inset == self.cstyle.text_decoration_inset {
                if let Some(&(lo, hi)) = box_ext.get(&BLOCK_BOX) {
                    let starts = !self.inset_seen.contains(&BLOCK_BOX);
                    let ends = self.final_flush;
                    self.inset_fixups.push(InsetFixup {
                        line: self.lines.len(),
                        frag,
                        deco: first - 1,
                        open: BLOCK_BOX,
                        lo: lo - x,
                        hi: hi - x,
                        w,
                        inset: (if starts { Some(d.inset.0) } else { None }, if ends { Some(d.inset.1) } else { None }),
                    });
                }
            }
        }
        vec![None; list.len()]
    }

    /// Resolve the recorded decoration insets (see `decoration_extents`).
    fn resolve_insets(&mut self) {
        let fixups = std::mem::take(&mut self.inset_fixups);
        for fx in fixups {
            let cw = self.content_w;
            let edges = if fx.open == BLOCK_BOX {
                0.0
            } else {
                let style = self.style(fx.open).clone();
                style.padding.left.resolve_or_zero(cw) + style.padding.right.resolve_or_zero(cw) + style.border_width.left + style.border_width.right
            };
            let base = self.inset_total.get(&fx.open).copied().unwrap_or(0.0) + edges;
            let l = fx.lo + fx.inset.0.map(|v| v.resolve_or_zero(base)).unwrap_or(0.0);
            let r = fx.hi - fx.inset.1.map(|v| v.resolve_or_zero(base)).unwrap_or(0.0);
            let a = if fx.lo.abs() < 0.01 { l } else { l.max(0.0) };
            let b = if (fx.w - fx.hi).abs() < 0.01 { r } else { r.min(fx.w) };
            if let Some(FragmentKind::Text { decoration_info, .. }) = self.lines.get_mut(fx.line).and_then(|ln| ln.fragments.get_mut(fx.frag)).map(|f| &mut f.kind) {
                if let Some(e) = decoration_info.extents.get_mut(fx.deco) {
                    *e = Some((a, b));
                }
            }
        }
    }

    fn rel_of(&self, idx: usize) -> (f32, f32) {
        self.rel.get(idx).copied().unwrap_or((0.0, 0.0))
    }

    fn strut_h(&self) -> f32 {
        (self.strut.above + self.strut.below).max(0.0)
    }

    fn start_line(&mut self, y: f32) {
        let (l, r) = self.bfc.available(y, self.strut_h().max(1.0), self.content_x, self.content_w);
        self.cur.y = y;
        self.cur.left = l;
        self.cur.align_right = r;
        self.cur.right = match self.wrap_limits.get(self.group).copied().flatten() {
            Some(w) => r.min(l + w),
            None => r,
        };
        self.cur.x = l + if self.indented() { self.indent } else { 0.0 };
        self.cur.frags.clear();
        self.cur.bg.clear();
        self.cur.pending_space = None;
        self.cur.has_content = false;
        self.cur.break_at = None;
        self.cur.justified = 0.0;
        self.cur.frag_ws.clear();
        let x = self.cur.x;
        for o in &mut self.cur.open {
            o.start_x = x;
            o.started_here = false;
        }
    }

    fn run(&mut self, pieces: &[Piece]) -> f32 {
        self.start_line(self.content_y);
        for p in pieces {
            if let Piece::Float(c) | Piece::Atomic(c, _) = p {
                self.children[*c].clamped = false;
            }
        }
        for (i, p) in pieces.iter().enumerate() {
            if self.done {
                // After the clamp point: floats and atomic inlines that were
                // not placed are hidden.
                for q in &pieces[i..] {
                    if let Piece::Float(c) | Piece::Atomic(c, _) = q {
                        self.children[*c].clamped = true;
                    }
                }
                for c in std::mem::take(&mut self.pending_floats) {
                    self.children[c].clamped = true;
                }
                break;
            }
            match p {
                Piece::Word {
                    text,
                    width,
                    item,
                    break_before,
                } => self.place_word(text.clone(), *width, *item, *break_before),
                Piece::Space { width, item } => {
                    if self.cur.has_content && self.cur.pending_space.is_none() {
                        self.cur.pending_space = Some((*width, *item));
                    }
                }
                Piece::Open(i) => self.open(*i),
                Piece::Close(i) => self.close(*i),
                Piece::Atomic(c, before) => self.place_atomic(*c, *before),
                Piece::Float(c) => self.place_float(*c),
                Piece::Abs(c) => {
                    // A hypothetical block starts at its block container's
                    // edge, even when the absolute occurs after inline text.
                    let block = !self.children[*c].style.display_before_blockification.is_inline_level();
                    let x = if block {
                        self.content_x + if self.cstyle.direction == Direction::Rtl { self.content_w } else { 0.0 }
                    } else {
                        self.cur.x
                    };
                    let y = self.cur.y;
                    let (rx, ry) = self.rel_kids.get(*c).copied().unwrap_or((0.0, 0.0));
                    self.children[*c].static_pos = (x + rx, y + ry);
                    if block && self.cur.has_content {
                        self.pending_abs.push(*c);
                    }
                    self.children[*c].static_align = None;
                }
                Piece::Break => self.finish_line(true),
                Piece::Gap(w) => {
                    if self.cur.has_content {
                        self.cur.x += w;
                    }
                }
            }
        }
        if !self.done {
            self.final_flush = true;
            self.finish_line(false);
        }
        // Floats still pending go at the current y.
        let pending = std::mem::take(&mut self.pending_floats);
        for c in pending {
            let y = self.cur.y;
            self.layout_float_at(c, y);
        }
        (self.cur.y - self.content_y).max(0.0)
    }

    fn open(&mut self, i: usize) {
        let style = self.style(i).clone();
        let cw = self.content_w;
        let left = style.margin.left.resolve_or_zero(cw) + style.border_width.left + style.padding.left.resolve_or_zero(cw);
        self.cur.open.push(OpenInline {
            item: i,
            start_x: self.cur.x,
            started_here: true,
        });
        self.cur.x += left;
    }

    fn close(&mut self, i: usize) {
        let pos = match self.cur.open.iter().rposition(|o| o.item == i) {
            Some(p) => p,
            None => return,
        };
        let o = self.cur.open.remove(pos);
        let style = self.style(i).clone();
        let cw = self.content_w;
        let right = style.margin.right.resolve_or_zero(cw) + style.border_width.right + style.padding.right.resolve_or_zero(cw);
        let end = self.cur.x + right;
        if needs_bg(&style) {
            self.cur.bg.push(BgFrag {
                x: o.start_x,
                w: (end - o.start_x).max(0.0),
                item: i,
                left_edge: o.started_here,
                right_edge: true,
            });
        }
        self.cur.x = end;
    }

    /// Distance from the content box's start edge to the line's start
    /// (floats shorten lines; tab stops are measured from the content box).
    fn line_start_offset(&self) -> f32 {
        if self.cstyle.direction == Direction::Rtl {
            (self.content_x + self.content_w - self.cur.align_right).max(0.0)
        } else {
            (self.cur.left - self.content_x).max(0.0)
        }
    }

    /// Is the current line indented (`text-indent`)? The first line, also
    /// those after a forced break with `each-line`; `hanging` inverts it.
    fn indented(&self) -> bool {
        let flags = self.cstyle.text_indent_flags;
        let first = self.first_line || (flags & 1 != 0 && self.after_forced);
        first != (flags & 2 != 0)
    }

    fn pending_w(&self) -> f32 {
        self.cur.pending_space.map(|p| p.0).unwrap_or(0.0)
    }

    fn place_word(&mut self, text: Rc<str>, width: f32, item: usize, break_before: bool) {
        let wraps = self.style(item).white_space.wraps();
        let mut policy = break_policy(self.style(item));
        // `auto-phrase` falls back to ordinary breaking only inside
        // ideographic text too long for a line.
        if self.style(item).word_break == rabbithole_style::WordBreak::AutoPhrase && !text.chars().any(|c| c as u32 >= 0x2E80) {
            policy = BreakPolicy::Never;
        }
        let mut text: String = text.to_string();
        let mut width = width;
        let tab = text == "\t";
        let mut break_before = break_before;
        let mut guard = 0;
        loop {
            guard += 1;
            if self.done {
                return;
            }
            if tab {
                let pos = self.cur.x + self.pending_w() - self.cur.left + self.line_start_offset();
                let style = self.style(item).clone();
                width = self.lay.tab_width(self.cstyle, &style, pos);
            }
            // (The letter-spacing after the word is dropped at a line end.)
            let trailing_ls = if self.cstyle.line_height_tight { 0.0 } else { self.style(item).letter_spacing.max(0.0) };
            let fits = self.cur.x + self.pending_w() + width - trailing_ls <= self.cur.right + 0.01;
            // Preserved spaces at the end of a `pre-wrap` line hang: they
            // never move to the next line (CSS Text 3 §4.1.3).
            let hangs = wraps && self.style(item).white_space != rabbithole_style::WhiteSpace::BreakSpaces && self.cur.has_content && !text.is_empty() && text.chars().all(|c| c == ' ' || c == '\t');
            if fits || hangs || (!wraps && !break_before) || guard > 10_000 {
                self.push_text(text, width, item, break_before);
                return;
            }
            if self.cur.has_content {
                if break_before {
                    self.finish_line(false);
                    break_before = false;
                    continue;
                }
                if let Some(idx) = self.cur.break_at.filter(|i| *i > 0) {
                    self.carry_over(idx);
                    continue;
                }
            }
            if policy == BreakPolicy::Never || !wraps {
                // The word may not be split: it overflows the line.
                self.push_text(text, width, item, break_before);
                return;
            }
            // Split the word by characters.
            let avail = self.cur.right - self.cur.x - self.pending_w();
            let (head, head_w, tail) = self.split_to_fit(&text, item, avail, !self.cur.has_content);
            if head.is_empty() {
                if self.cur.has_content {
                    self.finish_line(false);
                    continue;
                }
                // Cannot happen (split forces one char on an empty line).
                self.push_text(text, width, item, break_before);
                return;
            }
            self.push_text(head, head_w, item, break_before);
            if tail.is_empty() {
                return;
            }
            self.finish_line(false);
            let style = self.style(item).clone();
            let font = self.lay.font_for(&style);
            width = self.lay.measurer.width(font, style.font_size, &tail) + style.letter_spacing * tail.chars().count() as f32;
            text = tail;
            break_before = false;
        }
    }

    /// Longest prefix of `text` fitting in `avail`. Forces at least one
    /// character when `force`.
    fn split_to_fit(&mut self, text: &str, item: usize, avail: f32, force: bool) -> (String, f32, String) {
        let style = self.style(item).clone();
        let font = self.lay.font_for(&style);
        let size = style.font_size;
        let ls = style.letter_spacing;
        let mut best_end = 0;
        let mut best_w = 0.0;
        let mut count = 0;
        for (i, ch) in text.char_indices() {
            let end = i + ch.len_utf8();
            count += 1;
            let w = self.lay.measurer.width(font, size, &text[..end]) + ls * count as f32;
            if w <= avail + 0.01 || (force && count == 1) {
                best_end = end;
                best_w = w;
            } else {
                break;
            }
        }
        (text[..best_end].to_string(), best_w, text[best_end..].to_string())
    }

    fn consume_pending_space(&mut self) {
        if let Some((sw, sitem)) = self.cur.pending_space.take() {
            if !self.cur.has_content {
                return;
            }
            match self.cur.frags.last_mut() {
                Some(PF {
                    kind: PFKind::Text { text, item },
                    w,
                    ..
                }) if *item == sitem => {
                    text.push(' ');
                    *w += sw;
                }
                _ => {
                    let x = self.cur.x;
                    self.cur.frags.push(PF {
                        x,
                        w: sw,
                        kind: PFKind::Text {
                            text: " ".into(),
                            item: sitem,
                        },
                    });
                }
            }
            self.cur.x += sw;
        }
    }

    fn push_text(&mut self, text: String, width: f32, item: usize, break_before: bool) {
        self.consume_pending_space();
        if break_before {
            self.cur.break_at = Some(self.cur.frags.len());
        }
        let spaced = self.style(item).word_spacing.abs() > 0.001;
        let cur_x = self.cur.x;
        let appended = if !break_before {
            match self.cur.frags.last_mut() {
                Some(PF {
                    kind: PFKind::Text { text: t, item: it },
                    w,
                    x,
                }) if *it == item && !(spaced && (t.ends_with(' ') || text.starts_with(' '))) && text != "\t" && !t.ends_with('\t') && (*x + *w - cur_x).abs() < 0.01 => {
                    t.push_str(&text);
                    *w += width;
                    true
                }
                _ => false,
            }
        } else {
            false
        };
        if !appended {
            let x = self.cur.x;
            self.cur.frags.push(PF {
                x,
                w: width,
                kind: PFKind::Text { text, item },
            });
        }
        self.cur.x += width;
        self.cur.has_content = true;
    }

    fn place_atomic(&mut self, c: usize, break_before: bool) {
        let avail = self.content_w;
        let cb_h = self.cb_h;
        self.lay.layout_atomic(&mut self.children[c], avail, cb_h);
        let w = self.children[c].margin_rect().w;
        let mut guard = 0;
        loop {
            guard += 1;
            let fits = self.cur.x + self.pending_w() + w <= self.cur.right + 0.01;
            if fits || !self.cur.has_content || guard > 100 {
                break;
            }
            if !break_before {
                // Glued to what precedes: both move to the next line from
                // the last opportunity, if any (otherwise they overflow).
                if self.cstyle.white_space.wraps() {
                    if let Some(idx) = self.cur.break_at.filter(|i| *i > 0) {
                        self.carry_over(idx);
                        continue;
                    }
                }
                break;
            }
            self.finish_line(false);
        }
        if !self.cur.has_content && !self.bfc.is_empty() {
            // The line is at least as tall as the box: it must clear the
            // floats beside that whole height (CSS 2.1 §9.5), moving down
            // when the box does not fit beside them.
            let h = self.children[c].margin_rect().h.max(self.strut_h());
            for _ in 0..64 {
                let (l, r) = self.bfc.available(self.cur.y, h, self.content_x, self.content_w);
                let unchanged = (l - self.cur.left).abs() < 0.01 && (r - self.cur.align_right).abs() < 0.01;
                if unchanged && w <= r - l + 0.01 {
                    break;
                }
                if w > r - l + 0.01 && w <= self.content_w + 0.01 && self.cur.frags.is_empty() {
                    if let Some(ny) = self.bfc.next_y(self.cur.y) {
                        self.start_line(ny);
                        continue;
                    }
                }
                if !unchanged {
                    self.narrow_line(l, r);
                }
                break;
            }
        }
        self.consume_pending_space();
        if break_before {
            self.cur.break_at = Some(self.cur.frags.len());
        }
        let x = self.cur.x;
        self.cur.frags.push(PF {
            x,
            w,
            kind: PFKind::Box(c),
        });
        self.cur.x += w;
        self.cur.has_content = true;
    }

    fn place_float(&mut self, c: usize) {
        let y = self.cur.y;
        if self.cur.has_content {
            // A float met in the middle of a line goes at the top of that
            // line when it fits beside the line's content (CSS 2.1 §9.5.1);
            // otherwise it waits for the next line.
            self.layout_float_at(c, y);
            let placed_here = self.bfc.last().map(|r| (r.y - y).abs() < 0.01).unwrap_or(false);
            let (l, r) = self.bfc.available(y, self.strut_h().max(1.0), self.content_x, self.content_w);
            let used = self.cur.x - self.cur.left;
            if !placed_here || r - l + 0.01 < used {
                self.bfc.pop();
                self.pending_floats.push(c);
                return;
            }
        } else {
            self.layout_float_at(c, y);
        }
        // The current (empty) line shrinks around the new float.
        let (l, r) = self.bfc.available(y, self.strut_h().max(1.0), self.content_x, self.content_w);
        self.narrow_line(l, r);
    }

    /// Move the current line's edges to `[l, r)` (floats beside it).
    fn narrow_line(&mut self, l: f32, r: f32) {
        let shift = l - self.cur.left;
        self.cur.left = l;
        self.cur.right = match self.wrap_limits.get(self.group).copied().flatten() {
            Some(w) => r.min(l + w),
            None => r,
        };
        self.cur.align_right = r;
        self.cur.x += shift;
        for o in &mut self.cur.open {
            o.start_x += shift;
        }
        for f in &mut self.cur.frags {
            f.x += shift;
        }
    }

    fn layout_float_at(&mut self, c: usize, y: f32) {
        let cb = Cb {
            x: self.content_x,
            width: self.content_w,
            height: self.cb_h,
        };
        self.lay.layout_float(&mut self.children[c], &cb, y, self.bfc);
    }

    /// Move the fragments from `idx` on to a fresh line.
    fn carry_over(&mut self, idx: usize) {
        let carried = self.cur.frags.split_off(idx);
        let carried_start = carried.first().map(|f| f.x).unwrap_or(self.cur.x);
        // Trim the x of the line to the end of what stays.
        self.cur.x = self.cur.frags.last().map(|f| f.x + f.w).unwrap_or(self.cur.left);
        self.cur.pending_space = None;
        // Open inlines that started inside the carried region move with it.
        let mut moved: Vec<usize> = Vec::new();
        for (i, o) in self.cur.open.iter().enumerate() {
            if o.started_here && o.start_x >= carried_start - 0.01 {
                moved.push(i);
            }
        }
        let saved: Vec<(usize, f32)> = moved.iter().map(|&i| (i, self.cur.open[i].start_x)).collect();
        for &i in &moved {
            // Hide from finish_line's bg emission by marking as not started here
            // with a start beyond the line end.
            self.cur.open[i].start_x = f32::MAX;
        }
        self.finish_line(false);
        let dx = self.cur.x - carried_start;
        for (i, sx) in saved {
            if let Some(o) = self.cur.open.get_mut(i) {
                o.start_x = sx + dx;
                o.started_here = true;
            }
        }
        for mut f in carried {
            f.x += dx;
            self.cur.x = f.x + f.w;
            self.cur.frags.push(f);
        }
        self.cur.has_content = !self.cur.frags.is_empty();
        self.cur.break_at = Some(0);
    }

    /// Merge adjacent text fragments of the same item into one run.
    fn merge_frags(&mut self) {
        let frags = std::mem::take(&mut self.cur.frags);
        let mut out: Vec<PF> = Vec::with_capacity(frags.len());
        for f in frags {
            if let (
                Some(PF {
                    x: px,
                    w: pw,
                    kind: PFKind::Text { text: pt, item: pi },
                }),
                PFKind::Text { text, item },
            ) = (out.last_mut(), &f.kind)
            {
                // With word-spacing the painter cannot place the words of
                // one run: keep the words apart.
                let spaced = (pt.ends_with(' ') || text.starts_with(' ')) && item_style(self.items, self.cstyle, *item).word_spacing.abs() > 0.001;
                // A tab is placed on its own (its advance is not the glyph's).
                let tab = text.ends_with('\t') || pt.ends_with('\t') || text.starts_with('\t');
                if *pi == *item && (*px + *pw - f.x).abs() < 0.05 && !spaced && !tab {
                    pt.push_str(text);
                    *pw += f.w;
                    continue;
                }
            }
            out.push(f);
        }
        self.cur.frags = out;
    }

    /// A word of a joining script (Arabic) broken across lines keeps its
    /// joined forms (as in browsers): the line ends with a ZWJ after a
    /// dual-joining letter and the next line starts with one.
    fn join_across_break(&mut self, forced: bool) {
        fn dual_joining(c: char) -> bool {
            let u = c as u32;
            matches!(u, 0x0620..=0x064A | 0x066E..=0x06D3 | 0x06FA..=0x06FC | 0x0750..=0x077F)
                && !matches!(u, 0x0622..=0x0625 | 0x0627 | 0x0629 | 0x062F..=0x0632 | 0x0648 | 0x0671..=0x0673 | 0x0675..=0x0677 | 0x0688..=0x0699 | 0x06C0 | 0x06C3..=0x06CB | 0x06CD | 0x06CF | 0x06D2 | 0x06D3)
        }
        if std::mem::take(&mut self.join_next) {
            let mut delta = 0.0;
            if let Some(PF { kind: PFKind::Text { text, item }, w, .. }) = self.cur.frags.first_mut() {
                if text.chars().next().is_some_and(is_cursive) {
                    text.insert(0, '\u{200D}');
                    // It is drawn on its own: measure it so.
                    let style = item_style(self.items, self.cstyle, *item).clone();
                    let font = self.lay.font_for(&style);
                    let painted = self.lay.measurer.width(font, style.font_size, text) + style.letter_spacing * (text.chars().count() - 1) as f32;
                    delta = painted - *w;
                    *w = painted;
                }
            }
            if delta != 0.0 {
                for f in self.cur.frags.iter_mut().skip(1) {
                    f.x += delta;
                }
                self.cur.x += delta;
            }
        }
        if forced || self.final_flush {
            return;
        }
        if let Some(PF { kind: PFKind::Text { text, item }, .. }) = self.cur.frags.last_mut() {
            if *item != MARKER_ITEM && text.chars().next_back().is_some_and(dual_joining) {
                text.push('\u{200D}');
                self.join_next = true;
            }
        }
    }

    /// A line broken at a soft hyphen ends with the hyphenation character
    /// (`hyphenate-character`; U+2010, or `-` when the font lacks it).
    fn show_hyphen(&mut self) {
        let Some(PF { kind: PFKind::Text { text, item }, w, .. }) = self.cur.frags.last_mut() else { return };
        if !text.ends_with('\u{ad}') {
            return;
        }
        let style = item_style(self.items, self.cstyle, *item).clone();
        let font = self.lay.font_for(&style);
        let hyphen: Rc<str> = match &style.hyphenate_character {
            Some(h) => h.clone(),
            None if self.lay.ctx.fonts.has_glyph(font, '\u{2010}') => Rc::from("\u{2010}"),
            None => Rc::from("-"),
        };
        text.pop();
        text.push_str(&hyphen);
        let hw = if hyphen.is_empty() { 0.0 } else { self.lay.measurer.width(font, style.font_size, &hyphen) + style.letter_spacing * hyphen.chars().count() as f32 };
        *w += hw;
        self.cur.x += hw;
    }

    fn trim_trailing_space(&mut self) {
        if let Some(PF {
            kind: PFKind::Text { text, item },
            w,
            ..
        }) = self.cur.frags.last_mut()
        {
            // Preserved spaces stay (they hang or overflow instead; right-
            // to-left lines still drop one, their hanging is not modelled).
            let preserved = matches!(
                item_style(self.items, self.cstyle, *item).white_space,
                rabbithole_style::WhiteSpace::PreWrap | rabbithole_style::WhiteSpace::BreakSpaces
            ) && self.cstyle.direction != Direction::Rtl;
            if text.ends_with(' ') && text.len() > 1 && !preserved {
                text.pop();
                let style = item_style(self.items, self.cstyle, *item).clone();
                let font = self.lay.font_for(&style);
                let sw = self.lay.measurer.space_width(font, style.font_size);
                *w -= sw;
                self.cur.x -= sw;
            }
        }
    }

    /// Truncate the current line to make room for its overflow marker
    /// (`text-overflow` / line clamping).
    fn ellipsize(&mut self, clamp: bool) {
        let cstyle = self.cstyle.clone();
        let font = self.lay.font_for(&cstyle);
        let size = cstyle.font_size;
        // A block ellipsis is independent of the inline overflow marker.
        let marker = if !clamp {
            match &cstyle.text_overflow {
                // Forced line breaks in the anonymous inline marker are
                // transformed to spaces; it never creates another line.
                TextOverflow::String(s) => {
                    let mut out = String::new();
                    let mut space = false;
                    for c in s.chars() {
                        if matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c' | '\u{85}' | '\u{2028}' | '\u{2029}') {
                            if !space || !cstyle.white_space.collapses() { out.push(' '); }
                            space = true;
                        } else {
                            out.push(c);
                            space = false;
                        }
                    }
                    out
                }
                _ => "\u{2026}".to_string(),
            }
        } else {
            cstyle.block_ellipsis.as_deref().unwrap_or("\u{2026}").to_string()
        };
        let ell = marker.as_str();
        let ell_w = self.lay.measurer.width(font, size, ell) + cstyle.letter_spacing * ell.chars().count() as f32;
        let limit = self.cur.right - ell_w;
        // Drop / trim fragments from the end until the text fits.
        while let Some(last) = self.cur.frags.last() {
            if last.x + last.w <= limit + 0.01 {
                break;
            }
            let (x, item, text) = match &last.kind {
                PFKind::Text { text, item } => (last.x, *item, text.clone()),
                PFKind::Box(_) => {
                    self.cur.frags.pop();
                    continue;
                }
            };
            let avail = limit - x;
            if clamp {
                // The block ellipsis goes after the last soft wrap
                // opportunity that leaves room for it (CSS Overflow 4 §4.3).
                let only = self.cur.frags.len() == 1;
                let mut fitted: Option<(String, f32)> = None;
                let t = text.trim_end();
                for (i, ch) in t.char_indices().rev() {
                    if ch != ' ' {
                        continue;
                    }
                    let head = t[..i].trim_end();
                    if head.is_empty() {
                        continue;
                    }
                    let w = self.split_to_fit(head, item, f32::MAX, false).1;
                    if w <= avail + 0.01 {
                        fitted = Some((head.to_string(), w));
                        break;
                    }
                }
                match fitted {
                    Some((head, w)) => {
                        if let Some(f) = self.cur.frags.last_mut() {
                            f.w = w;
                            if let PFKind::Text { text, .. } = &mut f.kind {
                                *text = head;
                            }
                        }
                        break;
                    }
                    None if !only => {
                        self.cur.frags.pop();
                        continue;
                    }
                    None => {}
                }
            }
            // CSS Overflow keeps the first character on the line, clipping
            // the marker itself if there is not enough room for both.
            let keep_first = !clamp && self.cur.frags.len() == 1;
            let (mut head, mut head_w, _tail) = self.split_to_fit(text.trim_end(), item, avail, keep_first);
            // No space before the ellipsis.
            if head.ends_with(' ') && !head.trim().is_empty() {
                let trimmed = head.trim_end().to_string();
                head_w = self.split_to_fit(&trimmed, item, f32::MAX, false).1;
                head = trimmed;
            }
            if head.trim().is_empty() {
                self.cur.frags.pop();
                continue;
            }
            if let Some(PFKind::Text { text, .. }) = self.cur.frags.last_mut().map(|f| &mut f.kind) {
                *text = head;
            }
            if let Some(f) = self.cur.frags.last_mut() {
                f.w = head_w;
            }
            break;
        }
        let x = self.cur.frags.last().map(|f| f.x + f.w).unwrap_or(self.cur.left);
        self.cur.frags.push(PF {
            x,
            w: ell_w,
            kind: PFKind::Text {
                text: ell.to_string(),
                item: MARKER_ITEM,
            },
        });
        self.cur.x = x + ell_w;
        self.cur.has_content = true;
        // Inline boxes still open end here.
        for bg in &mut self.cur.bg {
            if bg.x + bg.w > self.cur.x {
                bg.w = (self.cur.x - bg.x).max(0.0);
            }
        }
    }

    /// Bidi controls opening (and closing) the inline element `item`'s
    /// `unicode-bidi` embedding / override / isolate.
    fn bidi_controls(&self, item: usize) -> (Vec<char>, Vec<char>) {
        use crate::bidi::*;
        let s = self.style(item);
        let rtl = s.direction == Direction::Rtl;
        let (emb, ovr, iso) = (if rtl { RLE_CH } else { LRE_CH }, if rtl { RLO_CH } else { LRO_CH }, if rtl { RLI_CH } else { LRI_CH });
        match s.unicode_bidi {
            UnicodeBidi::Normal => (vec![], vec![]),
            UnicodeBidi::Embed => (vec![emb], vec![PDF_CH]),
            UnicodeBidi::BidiOverride => (vec![ovr], vec![PDF_CH]),
            UnicodeBidi::Isolate => (vec![iso], vec![PDI_CH]),
            UnicodeBidi::IsolateOverride => (vec![iso, ovr], vec![PDF_CH, PDI_CH]),
            UnicodeBidi::Plaintext => (vec![FSI_CH], vec![PDI_CH]),
        }
    }

    /// Inline elements (item indices, outermost first) enclosing item `idx`
    /// that have a `unicode-bidi` other than `normal`.
    fn bidi_chain(&self, idx: usize) -> Vec<usize> {
        let mut out = Vec::new();
        if idx == MARKER_ITEM || idx >= self.items.len() {
            return out;
        }
        // Walk back over the items: an unmatched Open is an ancestor.
        let mut depth = 0usize;
        for j in (0..idx).rev() {
            match &self.items[j] {
                InlineItem::Close => depth += 1,
                InlineItem::Open { style, .. } => {
                    if depth == 0 {
                        if style.unicode_bidi != UnicodeBidi::Normal {
                            out.push(j);
                        }
                    } else {
                        depth -= 1;
                    }
                }
                _ => {}
            }
        }
        out.reverse();
        out
    }

    /// Reorder the line's fragments with the Unicode Bidirectional
    /// Algorithm (UAX #9): levels from the paragraph direction, the
    /// characters and the `unicode-bidi` / `direction` of inline elements;
    /// then runs of increasing level are reversed (L2) by mirroring their
    /// positions within the span they occupy.
    fn bidi_line(&mut self) {
        use crate::bidi::*;
        let base_rtl = self.cstyle.direction == Direction::Rtl;
        let container_bidi = self.cstyle.unicode_bidi;
        let chains: Vec<Vec<usize>> = self
            .cur
            .frags
            .iter()
            .map(|f| match &f.kind {
                _ if !self.bidi_items => Vec::new(),
                PFKind::Text { item, .. } => self.bidi_chain(*item),
                PFKind::Box(c) => self.items.iter().position(|it| matches!(it, InlineItem::Atomic(a) if a == c)).map(|i| self.bidi_chain(i)).unwrap_or_default(),
            })
            .collect();
        let explicit = chains.iter().any(|c| !c.is_empty()) || matches!(container_bidi, UnicodeBidi::BidiOverride | UnicodeBidi::IsolateOverride | UnicodeBidi::Plaintext);
        let rtl_text = self.cur.frags.iter().any(|f| matches!(&f.kind, PFKind::Text { text, .. } if needs_bidi(text)));
        // Left-to-right text spread over several fragments of a right-to-
        // left line keeps its order: it needs the full algorithm.
        let ltr_runs = base_rtl
            && self.cur.frags.len() > 1
            && self.cur.frags.iter().any(|f| matches!(&f.kind, PFKind::Text { text, .. } if text.chars().any(|c| class(c) == BidiClass::L)));
        if !explicit && !rtl_text && !ltr_runs {
            if base_rtl {
                self.mirror_line();
            }
            return;
        }
        // The line's characters with the controls of the inline elements.
        let mut chars: Vec<char> = Vec::new();
        // (fragment, char offset within it) of each non-control character.
        let mut origin: Vec<Option<(usize, usize)>> = Vec::new();
        let push = |chars: &mut Vec<char>, origin: &mut Vec<Option<(usize, usize)>>, c: char, o: Option<(usize, usize)>| {
            chars.push(c);
            origin.push(o);
        };
        if matches!(container_bidi, UnicodeBidi::BidiOverride | UnicodeBidi::IsolateOverride) {
            push(&mut chars, &mut origin, if base_rtl { RLO_CH } else { LRO_CH }, None);
        }
        let mut open: Vec<usize> = Vec::new();
        for (fi, f) in self.cur.frags.iter().enumerate() {
            let chain = &chains[fi];
            let common = open.iter().zip(chain.iter()).take_while(|(a, b)| a == b).count();
            while open.len() > common {
                let it = open.pop().unwrap();
                for c in self.bidi_controls(it).1 {
                    push(&mut chars, &mut origin, c, None);
                }
            }
            for &it in &chain[common..] {
                for c in self.bidi_controls(it).0 {
                    push(&mut chars, &mut origin, c, None);
                }
                open.push(it);
            }
            match &f.kind {
                PFKind::Text { text, item } => {
                    // Overflow markers form an independent bidi paragraph,
                    // taking their base direction from the containing line.
                    // In plaintext, the content before the marker determines it.
                    let marker = *item == MARKER_ITEM;
                    if marker {
                        let rtl = if container_bidi == UnicodeBidi::Plaintext { first_strong(&chars).unwrap_or(base_rtl) } else { base_rtl };
                        push(&mut chars, &mut origin, if rtl { RLI_CH } else { LRI_CH }, None);
                    }
                    for (k, c) in text.chars().enumerate() {
                        push(&mut chars, &mut origin, c, Some((fi, k)));
                    }
                    if marker { push(&mut chars, &mut origin, PDI_CH, None); }
                }
                PFKind::Box(_) => push(&mut chars, &mut origin, '\u{FFFC}', Some((fi, 0))),
            }
        }
        let base_rtl = if container_bidi == UnicodeBidi::Plaintext { first_strong(&chars).unwrap_or(base_rtl) } else { base_rtl };
        let base = base_rtl as u8;
        let mut levels = resolve_levels(&chars, base);
        reset_line_end(&chars, &mut levels, base);
        // Per-fragment character levels.
        let mut frag_levels: Vec<Vec<u8>> = self
            .cur
            .frags
            .iter()
            .map(|f| match &f.kind {
                PFKind::Text { text, .. } => vec![base; text.chars().count()],
                PFKind::Box(_) => vec![base],
            })
            .collect();
        for (i, o) in origin.iter().enumerate() {
            if let Some((fi, k)) = o {
                frag_levels[*fi][*k] = levels[i];
            }
        }
        // An overflow marker isolated in the paragraph's own direction sits
        // where its isolate initiator does (at the paragraph level): the
        // dropped controls must not join it to a run of higher levels.
        if base_rtl && container_bidi != UnicodeBidi::Plaintext {
            for (fi, f) in self.cur.frags.iter().enumerate() {
                if matches!(&f.kind, PFKind::Text { item, text } if *item == MARKER_ITEM && !text.chars().any(|c| matches!(class(c), BidiClass::L | BidiClass::R | BidiClass::AL))) {
                    frag_levels[fi].iter_mut().for_each(|l| *l = base);
                }
            }
        }
        // Split text fragments into units of one level (leading / trailing
        // neutrals of right-to-left runs apart from the letters, which the
        // shaper orders on its own).
        let frags = std::mem::take(&mut self.cur.frags);
        let mut units: Vec<(PF, u8)> = Vec::new();
        for (fi, f) in frags.into_iter().enumerate() {
            let lv = &frag_levels[fi];
            let (text, item) = match &f.kind {
                PFKind::Text { text, item } => (text.clone(), *item),
                PFKind::Box(_) => {
                    units.push((f, lv[0]));
                    continue;
                }
            };
            let cs: Vec<(usize, char)> = text.char_indices().collect();
            // Cut points: level changes, and the edges of each odd-level
            // run's right-to-left letters.
            let mut cuts: Vec<usize> = vec![0];
            let mut i = 0;
            while i < cs.len() {
                let mut j = i;
                while j < cs.len() && lv[j] == lv[i] {
                    j += 1;
                }
                if lv[i] % 2 == 1 {
                    let strong = |k: usize| matches!(class(cs[k].1), BidiClass::R | BidiClass::AL);
                    if let (Some(a), Some(b)) = ((i..j).find(|&k| strong(k)), (i..j).rev().find(|&k| strong(k))) {
                        cuts.push(a);
                        cuts.push(b + 1);
                    }
                }
                cuts.push(j);
                i = j;
            }
            cuts.sort_unstable();
            cuts.dedup();
            let style = self.style(item).clone();
            let font = self.lay.font_for(&style);
            let ls = style.letter_spacing;
            let mut x = f.x;
            for w in cuts.windows(2) {
                let (a, b) = (w[0], w[1]);
                if a >= b {
                    continue;
                }
                let start = cs[a].0;
                let end = if b < cs.len() { cs[b].0 } else { text.len() };
                let piece = &text[start..end];
                let width = if a == 0 && b >= cs.len() {
                    f.w
                } else if b >= cs.len() {
                    (f.x + f.w - x).max(0.0)
                } else {
                    self.lay.measurer.width(font, style.font_size, piece) + ls * piece.chars().count() as f32
                };
                let level = lv[a];
                // The run's direction for the shaper: an RLO prefix shapes
                // it right to left (visual order), an LRO prefix keeps
                // right-to-left letters in logical order.
                let has_rtl = piece.chars().any(|c| matches!(class(c), BidiClass::R | BidiClass::AL));
                let shown: String = if level % 2 == 1 {
                    format!("{RLO_CH}{piece}")
                } else if has_rtl {
                    format!("{LRO_CH}{piece}")
                } else {
                    piece.to_string()
                };
                units.push((PF { x, w: width, kind: PFKind::Text { text: shown, item } }, level));
                x += width;
            }
        }
        // L2: reverse runs of level ≥ k, highest k first. A whole-line
        // reversal (right-to-left paragraphs) mirrors the line box.
        let max = units.iter().map(|u| u.1).max().unwrap_or(0);
        let mut k = max;
        while k >= 1 {
            if k == 1 && base_rtl {
                self.cur.frags = units.iter().map(|u| u.0.clone()).collect();
                self.mirror_line();
                for (u, f) in units.iter_mut().zip(self.cur.frags.iter()) {
                    u.0.x = f.x;
                }
                break;
            }
            let mut idx: Vec<usize> = (0..units.len()).collect();
            idx.sort_by(|&a, &b| units[a].0.x.total_cmp(&units[b].0.x));
            let mut i = 0;
            while i < idx.len() {
                if units[idx[i]].1 < k {
                    i += 1;
                    continue;
                }
                let mut j = i;
                while j < idx.len() && units[idx[j]].1 >= k {
                    j += 1;
                }
                let s = units[idx[i]].0.x;
                let e = units[idx[j - 1]].0.x + units[idx[j - 1]].0.w;
                for &u in &idx[i..j] {
                    let f = &mut units[u].0;
                    f.x = s + e - (f.x + f.w);
                }
                i = j;
            }
            k -= 1;
        }
        self.cur.frags = units.into_iter().map(|u| u.0).collect();
    }

    fn mirror_line(&mut self) {
        let (l, r) = (self.cur.left, self.cur.right);
        let used = self.cur.x - l;
        let mirror = |x: f32, w: f32| r - (x - l) - w;
        for f in &mut self.cur.frags {
            f.x = mirror(f.x, f.w);
        }
        for bg in &mut self.cur.bg {
            bg.x = mirror(bg.x, bg.w);
        }
        for o in &mut self.cur.open {
            if o.start_x <= self.cur.x + 0.01 {
                // An open inline box spans to the line end: its mirrored
                // start is the left edge of the used area.
                o.start_x = r - used + (self.cur.x - o.start_x) - (self.cur.x - o.start_x);
                o.start_x = r - used;
            }
        }
        // Keep `cur.x` meaning "end of content" for the alignment maths.
        self.cur.x = l + used;
    }

    /// Per fragment of the current line, the widths of the leading and
    /// trailing spaces of the line it holds (not decorated); empty when
    /// there is none or the line is right-to-left.
    fn decoration_space_skips(&mut self) -> Vec<(f32, f32)> {
        let is_space = |c: char| c == ' ' || c == '\t' || lb::is_space_separator(c) || matches!(c, '\u{2007}' | '\u{202F}');
        if self.cstyle.direction == Direction::Rtl || !self.cur.frags.iter().any(|f| matches!(&f.kind, PFKind::Text { item, .. } if *item != MARKER_ITEM && !self.style(*item).applied_decorations.is_empty())) {
            return Vec::new();
        }
        let n = self.cur.frags.len();
        let mut out = vec![(0.0f32, 0.0f32); n];
        let mut any = false;
        for from_end in [false, true] {
            let order: Vec<usize> = if from_end { (0..n).rev().collect() } else { (0..n).collect() };
            for i in order {
                let (text, item, w) = match &self.cur.frags[i].kind {
                    PFKind::Text { text, item } if *item != MARKER_ITEM => (text.clone(), *item, self.cur.frags[i].w),
                    _ => break,
                };
                let count = if from_end { text.chars().rev().take_while(|c| is_space(*c)).count() } else { text.chars().take_while(|c| is_space(*c)).count() };
                if count == 0 {
                    break;
                }
                let all = count == text.chars().count();
                let sw = if all {
                    w
                } else {
                    let st = self.style(item).clone();
                    let font = self.lay.font_for(&st);
                    let part: String = if from_end { text.chars().rev().take(count).collect() } else { text.chars().take(count).collect() };
                    (self.lay.measurer.width(font, st.font_size, &part) + st.letter_spacing * count as f32).min(w)
                };
                if from_end {
                    out[i].1 = sw;
                } else {
                    out[i].0 = sw;
                }
                any = true;
                if !all {
                    break;
                }
            }
        }
        if any {
            out
        } else {
            Vec::new()
        }
    }

    /// The alignment of the current line: `text-align-last` applies to
    /// the last line of the block and lines ending in a forced break.
    fn line_align(&self, last: bool) -> TextAlign {
        let a = self.cstyle.text_align;
        if !last {
            return a;
        }
        match self.cstyle.text_align_last {
            Some(l) => l,
            None if a == TextAlign::Justify => TextAlign::Start,
            None => a,
        }
    }

    /// Distribute the free space of the line over its justification
    /// opportunities (CSS Text 3 §7.3): after word separators, and between
    /// typographic letter units for `text-justify: inter-character` (with
    /// `auto`, around ideographs). Trailing (hanging) spaces are not
    /// expanded. Text fragments are cut at the opportunities and moved.
    fn justify(&mut self) {
        if self.cur.frags.is_empty() {
            return;
        }
        // Trailing preserved spaces hang outside the line.
        let mut trailing_w = 0.0;
        let mut trailing_n = 0;
        if let Some(PF { kind: PFKind::Text { text, item }, w, .. }) = self.cur.frags.last() {
            let n = text.chars().rev().take_while(|c| *c == ' ').count();
            if n > 0 && *item != MARKER_ITEM {
                let st = self.style(*item).clone();
                let font = self.lay.font_for(&st);
                let sw = self.lay.measurer.space_width(font, st.font_size) + st.letter_spacing;
                trailing_w = (sw * n as f32).min(*w);
                trailing_n = n;
            }
        }
        let free = self.cur.align_right - self.cur.x + trailing_w;
        if free <= 0.01 {
            return;
        }
        // Opportunities after each character (by fragment and char index).
        let nfr = self.cur.frags.len();
        // Tab stops must keep lining up: only what follows the last tab
        // is expanded.
        let after_tab = self.cur.frags.iter().rposition(|f| matches!(&f.kind, PFKind::Text { text, .. } if text.contains('\t'))).map_or(0, |i| i + 1);
        let mut opp: Vec<Vec<bool>> = Vec::with_capacity(nfr);
        let mut count = 0usize;
        // The previous letter unit: fragment, char index, char, `text-justify`.
        let mut prev: Option<(usize, usize, char, u8)> = None;
        for (fi, f) in self.cur.frags.iter().enumerate() {
            let (chars, mode): (Vec<char>, u8) = match &f.kind {
                PFKind::Text { text, item } if *item != MARKER_ITEM => (text.chars().collect(), self.style(*item).text_justify),
                PFKind::Text { text, .. } => {
                    opp.push(vec![false; text.chars().count()]);
                    prev = None;
                    continue;
                }
                // An atomic inline is one letter unit.
                PFKind::Box(_) => (vec!['\u{FFFC}'], self.cstyle.text_justify),
            };
            let mut v = vec![false; chars.len()];
            if fi < after_tab {
                opp.push(v);
                prev = None;
                continue;
            }
            let limit = if fi + 1 == nfr { chars.len() - trailing_n.min(chars.len()) } else { chars.len() };
            for (k, &c) in chars.iter().enumerate() {
                if k >= limit {
                    break;
                }
                if attaches_to_previous(c) || is_default_ignorable(c) {
                    continue;
                }
                if let Some((pf, pk, p, pmode)) = prev {
                    if justify_between(p, c, pmode) || justify_between(p, c, mode) {
                        if pf == fi {
                            v[pk] = true;
                        } else {
                            opp[pf][pk] = true;
                        }
                        count += 1;
                    }
                }
                prev = Some((fi, k, c, mode));
            }
            opp.push(v);
        }
        if count == 0 {
            return;
        }
        let extra = free / count as f32;
        // Old x of each opportunity, for moving inline box edges.
        let mut marks: Vec<f32> = Vec::with_capacity(count);
        let frags = std::mem::take(&mut self.cur.frags);
        let mut out: Vec<PF> = Vec::with_capacity(frags.len() + count);
        let mut shift = 0.0;
        // Lines without bidi reordering keep a text fragment whole when all
        // its opportunities follow word separators: the painter spaces its
        // words (`word_spacing`). Others are cut at the opportunities.
        let whole = !self.bidi_items
            && self.cstyle.direction != Direction::Rtl
            && !frags.iter().any(|f| matches!(&f.kind, PFKind::Text { text, .. } if crate::bidi::needs_bidi(text)));
        let mut frag_ws: Vec<f32> = Vec::new();
        for (fi, f) in frags.into_iter().enumerate() {
            match f.kind {
                PFKind::Text { text, item }
                    if whole && opp[fi].iter().any(|o| *o) && text.chars().zip(opp[fi].iter()).all(|(c, o)| !*o || is_word_separator(c)) =>
                {
                    let k = opp[fi].iter().filter(|o| **o).count();
                    frag_ws.resize(out.len(), 0.0);
                    frag_ws.push(extra);
                    out.push(PF { x: f.x + shift, w: f.w + extra * k as f32, kind: PFKind::Text { text, item } });
                    // (Inline box edges fall on fragment edges.)
                    marks.extend(std::iter::repeat_n(f.x + f.w, k));
                    shift += extra * k as f32;
                }
                PFKind::Text { text, item } if opp[fi].iter().any(|o| *o) => {
                    let style = self.style(item).clone();
                    let font = self.lay.font_for(&style);
                    let ls = style.letter_spacing;
                    let mut x = f.x;
                    let mut start = 0;
                    let n = opp[fi].len();
                    for (k, (bi, ch)) in text.char_indices().enumerate() {
                        let end = bi + ch.len_utf8();
                        if !opp[fi][k] && k + 1 < n {
                            continue;
                        }
                        let piece = &text[start..end];
                        let w = if end == text.len() {
                            (f.x + f.w - x).max(0.0)
                        } else {
                            self.lay.measurer.width(font, style.font_size, piece) + ls * piece.chars().count() as f32
                        };
                        out.push(PF { x: x + shift, w, kind: PFKind::Text { text: piece.to_string(), item } });
                        x += w;
                        if opp[fi][k] {
                            marks.push(x);
                            shift += extra;
                        }
                        start = end;
                    }
                }
                kind => {
                    let opened = opp[fi].iter().any(|o| *o);
                    out.push(PF { x: f.x + shift, w: f.w, kind });
                    if opened {
                        marks.push(f.x + f.w);
                        shift += extra;
                    }
                }
            }
        }
        self.cur.frags = out;
        self.cur.frag_ws = frag_ws;
        let map = |x: f32| x + extra * marks.iter().take_while(|m| **m <= x + 0.01).count() as f32;
        for bg in &mut self.cur.bg {
            let (a, b) = (map(bg.x), map(bg.x + bg.w));
            bg.x = a;
            bg.w = (b - a).max(0.0);
        }
        for o in &mut self.cur.open {
            if o.start_x <= self.cur.x + 0.01 {
                o.start_x = map(o.start_x);
            }
        }
        self.cur.x += shift;
        self.cur.justified = shift;
    }

    fn finish_line(&mut self, forced: bool) {
        if !self.cur.has_content && !forced {
            if self.cur.frags.is_empty() {
                return;
            }
        }
        // (Whether the line ends at a space, for joining scripts.)
        let mut at_space = self.cur.pending_space.take().is_some();
        at_space |= matches!(self.cur.frags.last(), Some(PF { kind: PFKind::Text { text, .. }, .. }) if text.ends_with(' '));
        self.trim_trailing_space();
        if !forced && !self.final_flush {
            self.show_hyphen();
        }
        self.join_across_break(forced || at_space);
        // No letter-spacing at the end of a line (CSS Text 3 §8.2); the
        // lines of ruby bases and annotations are not line ends.
        if let Some(PF { kind: PFKind::Text { item, .. }, w, .. }) = self.cur.frags.last_mut().filter(|_| !self.cstyle.line_height_tight) {
            let ls = item_style(self.items, self.cstyle, *item).letter_spacing;
            if ls != 0.0 && *item != MARKER_ITEM {
                *w -= ls;
                self.cur.x -= ls;
            }
        }
        self.merge_frags();

        // Line clamp: this is the last allowed line and content follows.
        let more_follows = !self.final_flush || self.ellipsize_final;
        let clamp_hit = matches!(self.max_lines, Some(n) if self.lines.len() + 1 >= n) && more_follows;
        let overflows = self.cur.x > self.cur.right + 0.5;
        if clamp_hit && self.cstyle.block_ellipsis.is_some() {
            self.ellipsize(true);
        } else if self.ellipsis && overflows {
            self.ellipsize(false);
        }
        if clamp_hit {
            self.done = true;
        }
        let align = self.line_align(forced || self.final_flush);
        if align == TextAlign::Justify && !overflows {
            self.justify();
        }
        // Complete open inline fragments in the same coordinates as closed
        // ones. Bidi mirroring must transform their full widths, before it
        // updates the cursor/open markers for the next line.
        let line_end = self.cur.x;
        for idx in 0..self.cur.open.len() {
            let (start_x, item, started_here) = {
                let o = &self.cur.open[idx];
                (o.start_x, o.item, o.started_here)
            };
            if start_x > line_end + 0.01 {
                continue;
            }
            if needs_bg(self.style(item)) {
                self.cur.bg.push(BgFrag {
                    x: start_x,
                    w: (line_end - start_x).max(0.0),
                    item,
                    left_edge: started_here,
                    right_edge: false,
                });
            }
            self.cur.open[idx].started_here = false;
        }
        self.bidi_line();

        // Vertical metrics.
        #[derive(Clone, Copy)]
        struct FM {
            above: f32,
            below: f32,
            mode: u8, // 0 baseline, 1 top, 2 bottom
            shift: f32,
            tm: Option<TextMetrics>,
        }
        let mut fms: Vec<FM> = Vec::with_capacity(self.cur.frags.len());
        let mut a = self.strut.above;
        let mut d = self.strut.below;
        let mut extra_h: f32 = 0.0;
        // (Consecutive fragments of one text item share their metrics:
        // justified lines have one per word.)
        let mut last_text: Option<(usize, FM)> = None;
        for f in &self.cur.frags {
            let fm = match &f.kind {
                PFKind::Text { item, .. } if last_text.is_some_and(|l| l.0 == *item) => last_text.unwrap().1,
                PFKind::Text { item, .. } => {
                    let style = self.style(*item).clone();
                    let tm = self.lay.text_metrics(&style);
                    let shift = match style.vertical_align {
                        VerticalAlign::Sub => -0.25 * style.font_size,
                        VerticalAlign::Super => 0.35 * style.font_size,
                        VerticalAlign::Length(v) => v,
                        _ => 0.0,
                    };
                    let (mut above, mut below) = (tm.above + shift, tm.below - shift);
                    // Emphasis marks need room beside the glyphs.
                    if let Some(e) = emphasis_of(&style) {
                        let m = self.lay.ctx.fonts.metrics(tm.font, tm.size / 2.0);
                        let mh = m.ascent + m.descent;
                        if e.over {
                            above = above.max(tm.asc + shift + mh);
                        } else {
                            below = below.max(tm.desc - shift + mh);
                        }
                    }
                    FM {
                        above,
                        below,
                        mode: 0,
                        shift,
                        tm: Some(tm),
                    }
                }
                PFKind::Box(c) => {
                    let child = &self.children[*c];
                    let mr = child.margin_rect();
                    let h = mr.h;
                    let base = child.baseline.map(|bo| child.margin.top + bo).unwrap_or(h);
                    let fs = self
                        .cur
                        .open
                        .last()
                        .map(|o| self.style(o.item).font_size)
                        .unwrap_or(self.cstyle.font_size);
                    match child.style.vertical_align {
                        VerticalAlign::Top => FM {
                            above: 0.0,
                            below: 0.0,
                            mode: 1,
                            shift: 0.0,
                            tm: None,
                        },
                        VerticalAlign::Bottom => FM {
                            above: 0.0,
                            below: 0.0,
                            mode: 2,
                            shift: 0.0,
                            tm: None,
                        },
                        VerticalAlign::Middle => {
                            let above = h / 2.0 + 0.25 * fs;
                            FM { above, below: h - above, mode: 0, shift: 0.0, tm: None }
                        }
                        VerticalAlign::TextTop => {
                            let above = self.strut.asc;
                            FM { above, below: h - above, mode: 0, shift: 0.0, tm: None }
                        }
                        VerticalAlign::TextBottom => {
                            let below = self.strut.desc;
                            FM { above: h - below, below, mode: 0, shift: 0.0, tm: None }
                        }
                        VerticalAlign::Sub => FM { above: base - 0.25 * fs, below: h - base + 0.25 * fs, mode: 0, shift: 0.0, tm: None },
                        VerticalAlign::Super => FM { above: base + 0.35 * fs, below: h - base - 0.35 * fs, mode: 0, shift: 0.0, tm: None },
                        VerticalAlign::Length(v) => FM { above: base + v, below: h - base - v, mode: 0, shift: 0.0, tm: None },
                        VerticalAlign::Baseline => FM { above: base, below: h - base, mode: 0, shift: 0.0, tm: None },
                    }
                }
            };
            if let PFKind::Text { item, .. } = &f.kind {
                last_text = Some((*item, fm));
            }
            if fm.mode == 0 {
                a = a.max(fm.above);
                d = d.max(fm.below);
            } else if let PFKind::Box(c) = &f.kind {
                extra_h = extra_h.max(self.children[*c].margin_rect().h);
            }
            fms.push(fm);
        }
        for bg in &self.cur.bg {
            let style = self.style(bg.item).clone();
            let tm = self.lay.text_metrics(&style);
            a = a.max(tm.above);
            d = d.max(tm.below);
        }
        let h = (a + d).max(extra_h).max(0.0);
        let y = self.cur.y;
        let baseline = y + a;

        // Horizontal alignment. Preserved spaces at the end of a `pre-wrap`
        // line, and other space separators (U+3000...) at the end of any
        // line but `break-spaces` ones, hang: they do not count.
        let mut hang = 0.0;
        if let Some(PF { kind: PFKind::Text { text, item }, w, .. }) = self.cur.frags.last() {
            let st = self.style(*item).clone();
            let ws = st.white_space;
            let pre_wrap = ws == rabbithole_style::WhiteSpace::PreWrap;
            if ws != rabbithole_style::WhiteSpace::BreakSpaces && *item != MARKER_ITEM {
                let hangs = |c: char| (c == ' ' && pre_wrap) || lb::is_space_separator(c);
                let n = text.chars().rev().take_while(|c| hangs(*c)).count();
                if n > 0 {
                    let font = self.lay.font_for(&st);
                    let suffix_start = text.char_indices().rev().nth(n - 1).map_or(0, |(i, _)| i);
                    let suffix = &text[suffix_start..];
                    let width = if suffix.bytes().all(|b| b == b' ') {
                        self.lay.measurer.space_width(font, st.font_size) * n as f32
                    } else {
                        self.lay.measurer.width(font, st.font_size, suffix)
                    } + st.letter_spacing * n as f32;
                    let spaces = width.min(*w);
                    // Preserved spaces hang unconditionally at a soft wrap;
                    // before a forced break (or the end of the paragraph)
                    // only what overflows. Other space separators always.
                    hang = if pre_wrap && (forced || self.final_flush) { (self.cur.x - self.cur.right).clamp(0.0, spaces) } else { spaces };
                }
            }
        }
        let used = self.cur.x - self.cur.left - hang;
        if self.group_stats.len() <= self.group {
            self.group_stats.resize(self.group + 1, (0, 0.0));
        }
        let gs = &mut self.group_stats[self.group];
        gs.0 += 1;
        gs.1 = gs.1.max(used - self.cur.justified);
        let free = (self.cur.align_right - self.cur.left) - used;
        let rtl = self.cstyle.direction == Direction::Rtl;
        let shift_x = if rtl {
            // Content was mirrored to the right edge (start); `right` means
            // the end (left) edge.
            match align {
                TextAlign::End | TextAlign::Left => -free.max(0.0),
                TextAlign::Center => -(free / 2.0).max(0.0),
                _ => 0.0,
            }
        } else {
            match align {
                TextAlign::End | TextAlign::Right => free.max(0.0),
                TextAlign::Center => (free / 2.0).max(0.0),
                _ => 0.0,
            }
        };

        let mut fragments: Vec<Fragment> = Vec::with_capacity(self.cur.frags.len() + self.cur.bg.len());
        for bg in &self.cur.bg {
            let style = self.style(bg.item).clone();
            let tm = self.lay.text_metrics(&style);
            let cw = self.content_w;
            let pt = style.padding.top.resolve_or_zero(cw) + style.border_width.top;
            let pb = style.padding.bottom.resolve_or_zero(cw) + style.border_width.bottom;
            let (rx, ry) = self.rel_of(bg.item);
            fragments.push(Fragment {
                rect: Rect::new(bg.x + shift_x + rx, baseline - tm.asc - pt + ry, bg.w, tm.asc + tm.desc + pt + pb),
                baseline: baseline + ry,
                node: item_elem(self.items, bg.item),
                inert: style.pointer_events == rabbithole_style::PointerEvents::None,
                kind: FragmentKind::InlineBox {
                    style,
                    left_edge: bg.left_edge,
                    right_edge: bg.right_edge,
                },
            });
        }
        // Extents (on this line) of the inline elements that inset their
        // decorations.
        let mut box_ext: std::collections::HashMap<usize, (f32, f32)> = std::collections::HashMap::new();
        if !self.parent_open.is_empty() {
            let block = has_inset(self.cstyle);
            for f in &self.cur.frags {
                if let PFKind::Text { item, .. } = &f.kind {
                    if block && *item != MARKER_ITEM {
                        let e = box_ext.entry(BLOCK_BOX).or_insert((f.x, f.x + f.w));
                        e.0 = e.0.min(f.x);
                        e.1 = e.1.max(f.x + f.w);
                    }
                    let mut p = self.parent_open.get(*item).copied().flatten();
                    while let Some(o) = p {
                        let e = box_ext.entry(o).or_insert((f.x, f.x + f.w));
                        e.0 = e.0.min(f.x);
                        e.1 = e.1.max(f.x + f.w);
                        p = self.parent_open[o];
                    }
                }
            }
        }
        // `text-decoration-skip-spaces: start end`: no decoration under
        // the spaces starting or ending the line (left-to-right lines).
        let skip_spaces = self.decoration_space_skips();
        let frags = std::mem::take(&mut self.cur.frags);
        for (fi, (f, fm)) in frags.into_iter().zip(fms.into_iter()).enumerate() {
            match f.kind {
                PFKind::Text { text, item } => {
                    let tm = fm.tm.expect("text metrics");
                    let style = self.style(item).clone();
                    let (rx, ry) = if item == MARKER_ITEM { (0.0, 0.0) } else { self.rel_of(item) };
                    let bl = baseline - fm.shift + ry;
                    let oblique = self.lay.synthetic_oblique(tm.font, &style);
                    fragments.push(Fragment {
                        rect: Rect::new(f.x + shift_x + rx, bl - tm.asc, f.w, tm.asc + tm.desc),
                        baseline: bl,
                        node: if item == MARKER_ITEM { None } else { item_elem(self.items, item) },
                        inert: style.pointer_events == rabbithole_style::PointerEvents::None,
                        kind: FragmentKind::Text {
                            text: if text.contains(['\t', '\u{ad}']) { Rc::from(text.replace('\t', " ").replace('\u{ad}', "").as_str()) } else { Rc::from(text.as_str()) },
                            font: tm.font,
                            size: tm.size,
                            color: style.text_fill_color.unwrap_or(style.color),
                            letter_spacing: style.letter_spacing,
                            word_spacing: self.cur.frag_ws.get(fi).copied().unwrap_or(0.0),
                            hidden: style.visibility == Visibility::Hidden,
                            decoration_info: DecorationInfo {
                                list: style.applied_decorations.clone(),
                                extents: if box_ext.is_empty() || item == MARKER_ITEM { Vec::new() } else { self.decoration_extents(item, &style, &box_ext, f.x, f.w, fragments.len()) },
                                metrics: style
                                    .applied_decorations
                                    .iter()
                                    .map(|d| {
                                        let same = d.font_size == style.font_size
                                            && d.font_family == style.font_family
                                            && d.font_weight == style.font_weight
                                            && d.font_style == style.font_style
                                            && d.font_family_list == style.font_family_list;
                                        let (a, de) = if same {
                                            (tm.asc, tm.desc)
                                        } else {
                                            let mut s = ComputedStyle::initial();
                                            s.font_size = d.font_size;
                                            s.font_family = d.font_family;
                                            s.font_family_list = d.font_family_list.clone();
                                            s.font_weight = d.font_weight;
                                            s.font_style = d.font_style;
                                            let m = self.lay.text_metrics(&s);
                                            (m.asc, m.desc)
                                        };
                                        (fm.shift - d.baseline_shift, a, de)
                                    })
                                    .collect(),
                                variations: if style.applied_decorations.is_empty() { Vec::new() } else { style.font_variation_settings.clone() },
                                skip_ink: style.text_decoration_skip_ink,
                                skip_spaces: skip_spaces.get(fi).copied().unwrap_or((0.0, 0.0)),
                            },
                            shadows: style.text_shadow.clone(),
                            // Real small capitals when the face has them (the
                            // smcp feature); otherwise synthesized, unless
                            // font-synthesis-small-caps forbids it.
                            small_caps: style.font_variant_small_caps && style.font_synthesis_small_caps && !self.lay.ctx.fonts.has_feature(tm.font, *b"smcp"),
                            oblique,
                            emphasis: if item == MARKER_ITEM { None } else { emphasis_of(&style) },
                        },
                    });
                }
                PFKind::Box(c) => {
                    let child = &mut self.children[c];
                    let mr = child.margin_rect();
                    let top = match fm.mode {
                        1 => y,
                        2 => y + h - mr.h,
                        _ => baseline - fm.above,
                    };
                    let m = child.margin;
                    let (rx, ry) = self.rel_kids.get(c).copied().unwrap_or((0.0, 0.0));
                    child.translate(f.x + shift_x + m.left + rx - child.rect.x, top + m.top + ry - child.rect.y);
                    let node = child.node;
                    let rect = child.margin_rect();
                    fragments.push(Fragment {
                        rect,
                        baseline,
                        node,
                        inert: false,
                        kind: FragmentKind::Box(c),
                    });
                }
            }
        }
        for (o, (lo, hi)) in &box_ext {
            self.inset_seen.insert(*o);
            *self.inset_total.entry(*o).or_insert(0.0) += hi - lo;
        }
        self.lines.push(LineBox {
            rect: Rect::new(self.cur.left, y, (self.cur.align_right - self.cur.left).max(0.0), h),
            baseline,
            fragments,
        });
        self.first_line = false;
        self.after_forced = forced;
        if forced {
            self.group += 1;
        }
        let new_y = y + h;
        for c in std::mem::take(&mut self.pending_abs) {
            let ry = self.rel_kids.get(c).map_or(0.0, |r| r.1);
            self.children[c].static_pos.1 = new_y + ry;
        }
        let pending = std::mem::take(&mut self.pending_floats);
        for c in pending {
            self.layout_float_at(c, new_y);
        }
        self.start_line(new_y);
    }
}

fn needs_bg(style: &ComputedStyle) -> bool {
    // Even a transparent positioned (or filtered) inline establishes a
    // containing block. Preserve its fragments so absolute descendants can
    // use its edges.
    style.position != rabbithole_style::Position::Static
        || !style.filters.is_empty()
        || !style.backdrop_filters.is_empty()
        || !style.background_color.is_transparent()
        || style.has_visible_border()
        || style.background_image.is_some()
        || !style.background_layers.is_empty()
        || !style.box_shadow.is_empty()
        || style.outline_width > 0.0
}


