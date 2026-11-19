//! `::first-letter` (CSS 2.1 §5.12.2, CSS Pseudo-Elements 4 §2.4).
//!
//! Applied to a finished block container: the first typographic letter of
//! its first formatted line (possibly inside a descendant block, an inline
//! element or `::before` content), with the punctuation around it, is
//! wrapped in an inline box with the pseudo-element's style — or moved into
//! a float when the pseudo-element floats.

use crate::tree::{BoxKind, Flow, Formatting, InlineItem, LayoutBox};
use rabbithole_css::PseudoElement;
use rabbithole_style::{ComputedStyle, Display, Float, Position, WhiteSpace};
use std::rc::Rc;
use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scan {
    /// No content yet: keep looking in following siblings.
    NotFound,
    /// The first line starts with something that is not a letter.
    Stop,
    Done,
}

/// Wrap the first letter of `b` (a block container) in a box styled `fl`.
pub(crate) fn apply(b: &mut LayoutBox, fl: &Rc<ComputedStyle>) {
    if fl.display == Display::None {
        return;
    }
    let origin = b.style.clone();
    scan(b, fl, &origin);
}

fn scan(b: &mut LayoutBox, fl: &Rc<ComputedStyle>, origin: &Rc<ComputedStyle>) -> Scan {
    match b.formatting {
        Formatting::Inline => scan_inline(b, fl, origin),
        Formatting::Block => {
            for c in b.children.iter_mut() {
                if !c.flow.is_in_flow() {
                    continue;
                }
                if !matches!(c.kind, BoxKind::Block | BoxKind::AnonymousBlock) || !matches!(c.formatting, Formatting::Inline | Formatting::Block) {
                    return Scan::Stop;
                }
                match scan(c, fl, origin) {
                    Scan::NotFound => continue,
                    r => {
                        if r == Scan::Done {
                            // The box now holds content of an ancestor's
                            // pseudo-element: never reuse it on its own.
                            c.counter_touch = true;
                        }
                        return r;
                    }
                }
            }
            Scan::NotFound
        }
        _ => Scan::Stop,
    }
}

/// The pseudo-element's style for text styled `text`: the properties its
/// own rules leave inherited come from the text's parent (the innermost
/// element around the letter, CSS 2.1 §5.12.2 fictional tag sequence)
/// rather than from the originating element.
fn letter_style(fl: &Rc<ComputedStyle>, origin: &Rc<ComputedStyle>, text: &Rc<ComputedStyle>) -> ComputedStyle {
    let mut s = (**fl).clone();
    if !Rc::ptr_eq(origin, text) {
        macro_rules! inherit {
            ($($f:ident),*) => {$(
                if fl.$f == origin.$f && text.$f != origin.$f {
                    s.$f = text.$f.clone();
                }
            )*};
        }
        inherit!(color, font_family, font_family_list, font_weight, font_style, font_stretch, letter_spacing, word_spacing, text_transform, visibility, text_shadow, applied_decorations, font_variant_small_caps);
        if fl.font_size == origin.font_size && text.font_size != origin.font_size {
            s.font_size = text.font_size;
            if fl.line_height == origin.line_height && fl.line_height_number == origin.line_height_number {
                s.line_height = text.line_height;
                s.line_height_number = text.line_height_number;
            }
        }
    }
    s.position = Position::Static;
    s.display = if fl.float != Float::None { Display::Block } else { Display::Inline };
    s
}

fn scan_inline(b: &mut LayoutBox, fl: &Rc<ComputedStyle>, origin: &Rc<ComputedStyle>) -> Scan {
    // Punctuation-only text before the letter (e.g. an open quote from
    // `::before`): (item, start, end).
    let mut pending: Vec<(usize, usize, usize)> = Vec::new();
    let mut found = false;
    for i in 0..b.items.len() {
        let (text, style) = match &b.items[i] {
            InlineItem::Text { text, style, .. } => (text, style),
            InlineItem::Open { .. } | InlineItem::Close | InlineItem::Float(_) | InlineItem::Absolute(_) => continue,
            InlineItem::Atomic(_) | InlineItem::Break => return Scan::Stop,
        };
        match letter_range(text, style.white_space) {
            Extent::Skip => continue,
            Extent::Stop => return Scan::Stop,
            Extent::Punct(s, e) => pending.push((i, s, e)),
            Extent::Letter(s, e) => {
                pending.push((i, s, e));
                found = true;
                break;
            }
        }
    }
    if !found {
        return if pending.is_empty() { Scan::NotFound } else { Scan::Stop };
    }
    let floated = fl.float != Float::None;
    // Splice from the last piece backwards so earlier indices stay valid.
    let mut float_items: Vec<InlineItem> = Vec::new();
    for &(i, start, end) in pending.iter().rev() {
        let InlineItem::Text { text, style, node, elem } = &b.items[i] else { unreachable!() };
        let (text, style, node, elem) = (text.clone(), style.clone(), *node, *elem);
        let text_item = |t: &str, style: &Rc<ComputedStyle>| InlineItem::Text { text: Rc::from(t), style: style.clone(), node, elem };
        let s = Rc::new(letter_style(fl, origin, &style));
        let mut repl: Vec<InlineItem> = Vec::with_capacity(5);
        if start > 0 {
            repl.push(text_item(&text[..start], &style));
        }
        if floated {
            float_items.insert(0, text_item(&text[start..end], &s));
            if pending[0].0 == i {
                repl.push(InlineItem::Float(usize::MAX));
            }
        } else {
            repl.push(InlineItem::Open { node: None, style: s.clone() });
            repl.push(text_item(&text[start..end], &s));
            repl.push(InlineItem::Close);
        }
        if end < text.len() {
            repl.push(text_item(&text[end..], &style));
        }
        b.items.splice(i..i + 1, repl);
    }
    if floated {
        let i = pending[0].0;
        let at = (i..b.items.len()).find(|&k| matches!(b.items[k], InlineItem::Float(usize::MAX))).expect("float placeholder");
        let mut s = letter_style(fl, origin, origin);
        s.display = Display::Block;
        let mut fb = LayoutBox::new(BoxKind::Block, None, Rc::new(s));
        fb.pseudo = Some(PseudoElement::FirstLetter);
        fb.flow = if fl.float == Float::Right { Flow::FloatRight } else { Flow::FloatLeft };
        fb.bfc_root = true;
        fb.formatting = Formatting::Inline;
        fb.items = float_items;
        // Atomic indices follow item order: insert the float at its
        // position among the container's children.
        let pos = b.items[..at].iter().filter(|it| matches!(it, InlineItem::Atomic(_) | InlineItem::Float(_) | InlineItem::Absolute(_))).count();
        for it in b.items.iter_mut() {
            if let InlineItem::Atomic(k) | InlineItem::Float(k) | InlineItem::Absolute(k) = it {
                if *k >= pos && *k != usize::MAX {
                    *k += 1;
                }
            }
        }
        b.children.insert(pos, fb);
        b.items[at] = InlineItem::Float(pos);
    }
    Scan::Done
}

#[derive(Debug, PartialEq, Eq)]
enum Extent {
    /// Only (collapsible) white space.
    Skip,
    /// Starts with something that cannot begin a first letter.
    Stop,
    /// Byte range of the first letter and its punctuation.
    Letter(usize, usize),
    /// Only punctuation up to the end: the letter may follow in the next
    /// text.
    Punct(usize, usize),
}

fn is_punct(c: char) -> bool {
    matches!(
        c.general_category(),
        GeneralCategory::OpenPunctuation
            | GeneralCategory::ClosePunctuation
            | GeneralCategory::InitialPunctuation
            | GeneralCategory::FinalPunctuation
            | GeneralCategory::OtherPunctuation
    )
}

fn is_mark(c: char) -> bool {
    matches!(c.general_category(), GeneralCategory::NonspacingMark | GeneralCategory::SpacingMark | GeneralCategory::EnclosingMark)
}

/// Letters of cursive scripts that join their neighbours (Arabic, Syriac,
/// N'Ko, Mongolian).
fn joins(c: char) -> bool {
    matches!(c as u32, 0x0620..=0x064A | 0x066E..=0x06D3 | 0x06FA..=0x06FF | 0x0710..=0x074F | 0x0750..=0x077F | 0x07CA..=0x07EA | 0x0860..=0x086A | 0x08A0..=0x08C9 | 0x1820..=0x1878 | 0xFB50..=0xFDFF | 0xFE70..=0xFEFC)
}

fn letter_range(text: &str, ws: WhiteSpace) -> Extent {
    let newline_breaks = !matches!(ws, WhiteSpace::Normal | WhiteSpace::Nowrap);
    // White space, including non-collapsible spaces (Zs).
    let space = |c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}') || c.general_category() == GeneralCategory::SpaceSeparator;
    let mut it = text.char_indices().peekable();
    // Leading white space.
    let mut start = None;
    while let Some(&(i, c)) = it.peek() {
        if c == '\n' && newline_breaks {
            return Extent::Stop;
        }
        if matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}' | '\u{200B}') {
            it.next();
            continue;
        }
        if space(c) {
            // Text starting with a non-collapsible space (no-break space,
            // en space, ...) has no first letter (as in browsers).
            return Extent::Stop;
        }
        start = Some(i);
        break;
    }
    let Some(start) = start else { return Extent::Skip };
    // Preceding punctuation (with spaces between it and the letter).
    let mut letter = None;
    for (i, c) in it.by_ref() {
        if is_punct(c) || (space(c) && c != '\n' && i > start) {
            continue;
        }
        if space(c) || c.is_control() {
            return Extent::Stop;
        }
        if joins(c) && text[i + c.len_utf8()..].chars().find(|&n| !is_mark(n)).is_some_and(joins) {
            // Splitting a cursive word would break its joining forms.
            return Extent::Stop;
        }
        letter = Some((i, c));
        break;
    }
    let Some((li, lc)) = letter else { return Extent::Punct(start, text.len()) };
    let mut end = li + lc.len_utf8();
    // Combining marks of the letter, then following punctuation.
    let rest = &text[end..];
    let mut in_marks = true;
    for c in rest.chars() {
        if in_marks && is_mark(c) {
            end += c.len_utf8();
            continue;
        }
        in_marks = false;
        if is_punct(c) {
            end += c.len_utf8();
        } else {
            break;
        }
    }
    Extent::Letter(start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extent_includes_surrounding_punctuation() {
        assert_eq!(letter_range("  Test", WhiteSpace::Normal), Extent::Letter(2, 3));
        assert_eq!(letter_range("(T)est", WhiteSpace::Normal), Extent::Letter(0, 3));
        assert_eq!(letter_range("\u{201C}A\u{201D}, b", WhiteSpace::Normal), Extent::Letter(0, 8));
        assert_eq!(letter_range("   ", WhiteSpace::Normal), Extent::Skip);
        assert_eq!(letter_range("\u{a0}", WhiteSpace::Normal), Extent::Stop);
        assert_eq!(letter_range("\u{2002}B", WhiteSpace::Normal), Extent::Stop);
        assert_eq!(letter_range("\u{639}\u{627}", WhiteSpace::Normal), Extent::Stop);
        assert_eq!(letter_range("\u{201C}", WhiteSpace::Normal), Extent::Punct(0, 3));
        assert_eq!(letter_range("\nx", WhiteSpace::Pre), Extent::Stop);
        // A dash is not first-letter punctuation.
        assert_eq!(letter_range("-x", WhiteSpace::Normal), Extent::Letter(0, 1));
        // Combining marks stay with their letter.
        assert_eq!(letter_range("e\u{301}t", WhiteSpace::Normal), Extent::Letter(0, 3));
    }
}
