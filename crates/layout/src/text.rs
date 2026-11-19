//! Text processing for inline layout: white-space handling, text-transform
//! and a width-measurement cache.

use rabbithole_gfx::{FontContext, FontId};
use rabbithole_style::{TextTransform, WhiteSpace};
use std::collections::HashMap;

/// A unit of processed text.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Seg {
    /// A run of non-whitespace characters (may be a preserved space in
    /// `pre` modes).
    Word(String),
    /// A collapsible space (break opportunity).
    Space,
    /// A forced line break (`\n` in pre / pre-wrap / pre-line).
    Break,
    /// A break opportunity without width (U+200B, `<wbr>`).
    Opportunity,
}

/// Tracks collapsible-space state across adjacent text items.
#[derive(Debug, Default)]
pub(crate) struct Collapser {
    /// Whether the previous emitted thing was a space (or a line start).
    pub prev_space: bool,
    /// The last character other than white space and default ignorables
    /// (for the segment break transformation).
    pub last: Option<char>,
}

impl Collapser {
    pub fn new() -> Self {
        Collapser { prev_space: true, last: None }
    }
}

/// Default ignorable code points skipped by the segment break
/// transformation (U+200B is significant).
fn ignorable(c: char) -> bool {
    matches!(c as u32, 0xAD | 0x34F | 0x61C | 0x115F | 0x1160 | 0x17B4 | 0x17B5 | 0x180B..=0x180F | 0x200C..=0x200F | 0x202A..=0x202E | 0x2060..=0x206F | 0x3164 | 0xFE00..=0xFE0F | 0xFEFF | 0xFFA0 | 0xE0000..=0xE0FFF)
}

/// East Asian Width F, W or H, Hangul excluded.
fn ea_wide_not_hangul(c: char) -> bool {
    let u = c as u32;
    let hangul = matches!(u, 0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F | 0xAC00..=0xD7FF | 0xFFA0..=0xFFDC);
    !hangul
        && matches!(u, 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xA000..=0xA4CF | 0xF900..=0xFAFF | 0xFE10..=0xFE19 | 0xFE30..=0xFE6F | 0xFF00..=0xFFDC | 0xFFE0..=0xFFEE | 0x1F200..=0x1F2FF | 0x20000..=0x3FFFD)
}

/// Is a collapsible segment break between `prev` and `next` removed
/// (CSS Text 3 §4.1.2)? Next to a zero-width space, or between two East
/// Asian wide characters (not Hangul).
fn segment_break_removed(prev: Option<char>, next: Option<char>) -> bool {
    match (prev, next) {
        (Some('\u{200b}'), _) | (_, Some('\u{200b}')) => true,
        (Some(a), Some(b)) => ea_wide_not_hangul(a) && ea_wide_not_hangul(b),
        _ => false,
    }
}

pub(crate) fn transform(text: &str, t: TextTransform) -> String {
    match t {
        TextTransform::None => text.to_string(),
        TextTransform::Uppercase => text.to_uppercase(),
        TextTransform::Lowercase => text.to_lowercase(),
        TextTransform::Capitalize => {
            let mut out = String::with_capacity(text.len());
            let mut at_word_start = true;
            for ch in text.chars() {
                if ch.is_whitespace() {
                    at_word_start = true;
                    out.push(ch);
                } else if at_word_start {
                    out.extend(ch.to_uppercase());
                    at_word_start = false;
                } else {
                    out.push(ch);
                }
            }
            out
        }
    }
}

fn is_collapsible_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c')
}

/// Split `text` into segments according to `white-space`.
#[cfg(test)]
pub(crate) fn segment(text: &str, ws: WhiteSpace, st: &mut Collapser, out: &mut Vec<Seg>) {
    segment_tabs(text, ws, st, out, false)
}

/// Like [`segment`], with the `tab-size` (in spaces) for preserved tabs.
/// Soft hyphens (U+00AD) stay in the words when `keep_shy` (`hyphens`
/// other than `none`): they are break opportunities, shown as a hyphen
/// only at the end of a line.
pub(crate) fn segment_tabs(text: &str, ws: WhiteSpace, st: &mut Collapser, out: &mut Vec<Seg>, keep_shy: bool) {
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut Vec<Seg>| {
        if !word.is_empty() {
            out.push(Seg::Word(std::mem::take(word)));
        }
    };
    let preserve_spaces = matches!(ws, WhiteSpace::Pre | WhiteSpace::PreWrap | WhiteSpace::BreakSpaces);
    let preserve_newlines = ws.preserves_newlines();
    for (i, ch) in text.char_indices() {
        match ch {
            '\u{ad}' => {
                if keep_shy {
                    word.push('\u{ad}');
                }
            }
            '\u{200b}' => {
                // Zero-width space: break opportunity without width.
                flush(&mut word, out);
                out.push(Seg::Opportunity);
                if !preserve_spaces {
                    st.prev_space = true;
                }
                st.last = Some(ch);
            }
            '\u{a0}' => {
                word.push(' ');
                st.prev_space = false;
                st.last = Some(ch);
            }
            '\r' => {}
            '\n' if preserve_newlines => {
                flush(&mut word, out);
                out.push(Seg::Break);
                st.prev_space = true;
            }
            '\t' if preserve_spaces => {
                // A preserved tab: its width (up to the next tab stop) is
                // known only when it is placed on a line.
                flush(&mut word, out);
                out.push(Seg::Word("\t".into()));
                st.prev_space = false;
            }
            c if preserve_spaces && c == ' ' => {
                flush(&mut word, out);
                out.push(Seg::Word(" ".into()));
                st.prev_space = false;
            }
            c if is_collapsible_ws(c) => {
                flush(&mut word, out);
                if !st.prev_space {
                    // A white space run with a segment break may vanish.
                    let rest = &text[i..];
                    let run = rest.find(|c: char| !is_collapsible_ws(c)).unwrap_or(rest.len());
                    let removed = !preserve_newlines
                        && rest[..run].contains('\n')
                        && segment_break_removed(st.last, rest[run..].chars().find(|c| !ignorable(*c)));
                    if !removed {
                        out.push(Seg::Space);
                    }
                    st.prev_space = true;
                }
            }
            c => {
                word.push(c);
                st.prev_space = false;
                if !ignorable(c) {
                    st.last = Some(c);
                }
            }
        }
    }
    flush(&mut word, out);
}

/// Caches text widths per (font, size).
pub(crate) struct Measurer<'a> {
    pub fonts: &'a FontContext,
    cache: HashMap<(usize, u32), HashMap<String, f32>>,
}

impl<'a> Measurer<'a> {
    pub fn new(fonts: &'a FontContext) -> Self {
        Measurer {
            fonts,
            cache: HashMap::new(),
        }
    }

    pub fn width(&mut self, font: FontId, size: f32, text: &str) -> f32 {
        let key = (font.0, size.to_bits());
        let per_font = self.cache.entry(key).or_default();
        if let Some(w) = per_font.get(text) {
            return *w;
        }
        // Soft hyphens are invisible (the hyphen shown at a line end is
        // measured on its own).
        let w = if text.contains('\u{ad}') { self.fonts.measure(font, size, &text.replace('\u{ad}', "")) } else { self.fonts.measure(font, size, text) };
        if text.len() <= 64 {
            per_font.insert(text.to_string(), w);
        }
        w
    }

    pub fn space_width(&mut self, font: FontId, size: f32) -> f32 {
        self.width(font, size, " ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_whitespace() {
        let mut st = Collapser::new();
        let mut out = Vec::new();
        segment("  hello \n  world  ", WhiteSpace::Normal, &mut st, &mut out);
        assert_eq!(
            out,
            vec![Seg::Word("hello".into()), Seg::Space, Seg::Word("world".into()), Seg::Space]
        );
        assert!(st.prev_space);
    }

    #[test]
    fn preserves_pre() {
        let mut st = Collapser::new();
        let mut out = Vec::new();
        segment("a  b\nc", WhiteSpace::Pre, &mut st, &mut out);
        assert_eq!(
            out,
            vec![
                Seg::Word("a".into()),
                Seg::Word(" ".into()),
                Seg::Word(" ".into()),
                Seg::Word("b".into()),
                Seg::Break,
                Seg::Word("c".into())
            ]
        );
    }

    #[test]
    fn segment_breaks_between_wide_characters_vanish() {
        let mut st = Collapser::new();
        let mut out = Vec::new();
        segment("中\n文 a\nb", WhiteSpace::Normal, &mut st, &mut out);
        assert_eq!(out, vec![Seg::Word("中".into()), Seg::Word("文".into()), Seg::Space, Seg::Word("a".into()), Seg::Space, Seg::Word("b".into())]);
    }

    #[test]
    fn transforms() {
        assert_eq!(transform("hello big world", TextTransform::Capitalize), "Hello Big World");
        assert_eq!(transform("Ab", TextTransform::Uppercase), "AB");
    }
}
