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
}

/// Tracks collapsible-space state across adjacent text items.
#[derive(Debug, Default)]
pub(crate) struct Collapser {
    /// Whether the previous emitted thing was a space (or a line start).
    pub prev_space: bool,
}

impl Collapser {
    pub fn new() -> Self {
        Collapser { prev_space: true }
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
    segment_tabs(text, ws, st, out, 8)
}

/// Like [`segment`], with the `tab-size` (in spaces) for preserved tabs.
pub(crate) fn segment_tabs(text: &str, ws: WhiteSpace, st: &mut Collapser, out: &mut Vec<Seg>, tab_size: usize) {
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut Vec<Seg>| {
        if !word.is_empty() {
            out.push(Seg::Word(std::mem::take(word)));
        }
    };
    let preserve_spaces = matches!(ws, WhiteSpace::Pre | WhiteSpace::PreWrap);
    let preserve_newlines = ws.preserves_newlines();
    for ch in text.chars() {
        match ch {
            '\u{ad}' => {}
            '\u{200b}' => {
                // Zero-width space: break opportunity without width.
                flush(&mut word, out);
                if !preserve_spaces {
                    st.prev_space = true;
                }
            }
            '\u{a0}' => {
                word.push(' ');
                st.prev_space = false;
            }
            '\r' => {}
            '\n' if preserve_newlines => {
                flush(&mut word, out);
                out.push(Seg::Break);
                st.prev_space = true;
            }
            '\t' if preserve_spaces => {
                flush(&mut word, out);
                out.push(Seg::Word(" ".repeat(tab_size.max(1))));
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
                    out.push(Seg::Space);
                    st.prev_space = true;
                }
            }
            c => {
                word.push(c);
                st.prev_space = false;
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
        let w = self.fonts.measure(font, size, text);
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
    fn transforms() {
        assert_eq!(transform("hello big world", TextTransform::Capitalize), "Hello Big World");
        assert_eq!(transform("Ab", TextTransform::Uppercase), "AB");
    }
}
