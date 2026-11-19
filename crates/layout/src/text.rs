//! Text processing for inline layout: white-space handling, text-transform
//! and a width-measurement cache.

use rabbithole_gfx::{FontContext, FontId};
use rabbithole_style::{ComputedStyle, TextTransform, WhiteSpace, LANG_EL, LANG_NL, LANG_TR};
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

/// Apply `text-transform` (the case, then `full-width` and
/// `full-size-kana`), with the case tailorings of the content language.
pub(crate) fn transform_style(text: &str, style: &ComputedStyle) -> String {
    let mut out = match (style.text_transform, style.lang) {
        (TextTransform::None, _) => text.to_string(),
        (TextTransform::Uppercase, LANG_TR) => text.chars().map(|c| if c == 'i' { "\u{130}".to_string() } else { c.to_uppercase().collect() }).collect(),
        (TextTransform::Uppercase, LANG_EL) => greek_upper(text),
        (TextTransform::Lowercase, LANG_TR) => text
            .chars()
            .map(|c| match c {
                'I' => "\u{131}".to_string(),
                '\u{130}' => "i".to_string(),
                c => c.to_lowercase().collect(),
            })
            .collect(),
        (t, lang) => transform_lang(text, t, lang),
    };
    if style.text_transform_extra & 1 != 0 {
        out = out.chars().map(full_width).collect();
    }
    if style.text_transform_extra & 2 != 0 {
        out = out.chars().map(full_size_kana).collect();
    }
    out
}

#[cfg(test)]
pub(crate) fn transform(text: &str, t: TextTransform) -> String {
    transform_lang(text, t, 0)
}

fn transform_lang(text: &str, t: TextTransform, lang: u8) -> String {
    match t {
        TextTransform::None => text.to_string(),
        TextTransform::Uppercase => text.to_uppercase(),
        TextTransform::Lowercase => text.to_lowercase(),
        TextTransform::Capitalize => {
            // The first typographic letter unit of each word in titlecase
            // (punctuation before it is skipped).
            let mut out = String::with_capacity(text.len());
            let mut at_word_start = true;
            let mut chars = text.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch.is_whitespace() {
                    at_word_start = true;
                    out.push(ch);
                } else if at_word_start && ch.is_alphabetic() {
                    at_word_start = false;
                    match (lang, ch) {
                        (LANG_TR, 'i') => out.push('\u{130}'),
                        // Dutch: the IJ digraph capitalizes as a whole.
                        (LANG_NL, 'i' | 'I') if matches!(chars.peek(), Some('j' | 'J')) => {
                            chars.next();
                            out.push_str("IJ");
                        }
                        _ => titlecase(ch, &mut out),
                    }
                } else {
                    if ch.is_alphanumeric() {
                        at_word_start = false;
                    }
                    out.push(ch);
                }
            }
            out
        }
    }
}

/// The titlecase form of `c` (digraphs and ligatures differ from the
/// uppercase).
fn titlecase(c: char, out: &mut String) {
    match c {
        '\u{1C4}'..='\u{1C6}' => out.push('\u{1C5}'),
        '\u{1C7}'..='\u{1C9}' => out.push('\u{1C8}'),
        '\u{1CA}'..='\u{1CC}' => out.push('\u{1CB}'),
        '\u{1F1}'..='\u{1F3}' => out.push('\u{1F2}'),
        'ß' => out.push_str("Ss"),
        '\u{FB00}' => out.push_str("Ff"),
        '\u{FB01}' => out.push_str("Fi"),
        '\u{FB02}' => out.push_str("Fl"),
        '\u{FB03}' => out.push_str("Ffi"),
        '\u{FB04}' => out.push_str("Ffl"),
        '\u{FB05}' | '\u{FB06}' => out.push_str("St"),
        // Greek with ypogegrammeni: titlecase keeps it as prosgegrammeni.
        '\u{1F80}'..='\u{1FAF}' => out.push(char::from_u32((c as u32 & !0xF) | 0x8 | (c as u32 & 0x7)).unwrap_or(c)),
        '\u{1FB3}' | '\u{1FC3}' | '\u{1FF3}' => out.push(char::from_u32(c as u32 + 9).unwrap_or(c)),
        _ => out.extend(c.to_uppercase()),
    }
}

/// Greek uppercase (as in Greek typography): accents are dropped, a
/// dialytika stays; an accented vowel followed by ι / υ marks it with a
/// dialytika; the disjunctive ή keeps its accent.
fn greek_upper(text: &str) -> String {
    // (base uppercase, had an accent, had a dialytika)
    fn strip(c: char) -> Option<(char, bool, bool)> {
        Some(match c {
            'ά' | 'Ά' => ('Α', true, false),
            'έ' | 'Έ' => ('Ε', true, false),
            'ή' | 'Ή' => ('Η', true, false),
            'ί' | 'Ί' => ('Ι', true, false),
            'ό' | 'Ό' => ('Ο', true, false),
            'ύ' | 'Ύ' => ('Υ', true, false),
            'ώ' | 'Ώ' => ('Ω', true, false),
            'ΐ' => ('Ϊ', true, true),
            'ΰ' => ('Ϋ', true, true),
            'ϊ' | 'Ϊ' => ('Ϊ', false, true),
            'ϋ' | 'Ϋ' => ('Ϋ', false, true),
            _ => return None,
        })
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // A word that is a lone ή keeps its tonos.
        let lone = |k: usize| k >= chars.len() || !chars[k].is_alphabetic();
        if (c == 'ή' || c == 'Ή') && (i == 0 || !chars[i - 1].is_alphabetic()) && lone(i + 1) {
            out.push('Ή');
            i += 1;
            continue;
        }
        if c == '\u{301}' || c == '\u{300}' || c == '\u{342}' {
            i += 1;
            continue;
        }
        match strip(c) {
            Some((base, accent, _)) => {
                out.push(base);
                // ά ι → ΑΪ: the accent showed the vowels are apart.
                if accent && matches!(base, 'Α' | 'Ε' | 'Ο' | 'Υ') {
                    if let Some(&n) = chars.get(i + 1) {
                        if matches!(n, 'ι' | 'Ι') {
                            out.push('Ϊ');
                            i += 2;
                            continue;
                        }
                        if matches!(n, 'υ' | 'Υ') && base != 'Υ' {
                            out.push('Ϋ');
                            i += 2;
                            continue;
                        }
                    }
                }
            }
            None => out.extend(c.to_uppercase()),
        }
        i += 1;
    }
    out
}

/// `full-width`: the fullwidth form of ASCII and halfwidth characters.
fn full_width(c: char) -> char {
    let u = c as u32;
    // (Spaces become U+3000 after white space collapsing.)
    let m = match u {
        0x21..=0x7E => u - 0x21 + 0xFF01,
        0xA2 => 0xFFE0,
        0xA3 => 0xFFE1,
        0xAC => 0xFFE2,
        0xAF => 0xFFE3,
        0xA6 => 0xFFE4,
        0xA5 => 0xFFE5,
        0x20A9 => 0xFFE6,
        0xFF61 => 0x3002,
        0xFF62 => 0x300C,
        0xFF63 => 0x300D,
        0xFF64 => 0x3001,
        0xFF65..=0xFF9F => {
            const KANA: [u16; 59] = [
                0x30FB, 0x30F2, 0x30A1, 0x30A3, 0x30A5, 0x30A7, 0x30A9, 0x30E3, 0x30E5, 0x30E7, 0x30C3, 0x30FC, 0x30A2, 0x30A4, 0x30A6, 0x30A8, 0x30AA, 0x30AB, 0x30AD, 0x30AF,
                0x30B1, 0x30B3, 0x30B5, 0x30B7, 0x30B9, 0x30BB, 0x30BD, 0x30BF, 0x30C1, 0x30C4, 0x30C6, 0x30C8, 0x30CA, 0x30CB, 0x30CC, 0x30CD, 0x30CE, 0x30CF, 0x30D2, 0x30D5,
                0x30D8, 0x30DB, 0x30DE, 0x30DF, 0x30E0, 0x30E1, 0x30E2, 0x30E4, 0x30E6, 0x30E8, 0x30E9, 0x30EA, 0x30EB, 0x30EC, 0x30ED, 0x30EF, 0x30F3, 0x3099, 0x309A,
            ];
            KANA[(u - 0xFF65) as usize] as u32
        }
        0xFFE8 => 0x2502,
        0xFFE9 => 0x2190,
        0xFFEA => 0x2191,
        0xFFEB => 0x2192,
        0xFFEC => 0x2193,
        0xFFED => 0x25A0,
        0xFFEE => 0x25CB,
        _ => u,
    };
    char::from_u32(m).unwrap_or(c)
}

/// `full-size-kana`: small kana to their full-size forms.
fn full_size_kana(c: char) -> char {
    let m = match c as u32 {
        u @ (0x3041 | 0x3043 | 0x3045 | 0x3047 | 0x3049 | 0x3063 | 0x3083 | 0x3085 | 0x3087 | 0x308E) => u + 1,
        u @ (0x30A1 | 0x30A3 | 0x30A5 | 0x30A7 | 0x30A9 | 0x30C3 | 0x30E3 | 0x30E5 | 0x30E7 | 0x30EE) => u + 1,
        0x3095 => 0x304B,
        0x3096 => 0x3051,
        0x30F5 => 0x30AB,
        0x30F6 => 0x30B1,
        u @ 0x31F0..=0x31FF => [0x30AF, 0x30B7, 0x30B9, 0x30C8, 0x30CC, 0x30CF, 0x30D2, 0x30D5, 0x30D8, 0x30DB, 0x30E0, 0x30E9, 0x30EA, 0x30EB, 0x30EC, 0x30ED][(u - 0x31F0) as usize],
        u @ 0xFF67..=0xFF6B => u + 10,
        0xFF6C => 0xFF94,
        0xFF6D => 0xFF95,
        0xFF6E => 0xFF96,
        0xFF6F => 0xFF82,
        0x1B132 => 0x3053,
        0x1B150 => 0x3090,
        0x1B151 => 0x3091,
        0x1B152 => 0x3092,
        0x1B155 => 0x30B3,
        0x1B164 => 0x30F0,
        0x1B165 => 0x30F1,
        0x1B166 => 0x30F2,
        0x1B167 => 0x30F3,
        u => u,
    };
    char::from_u32(m).unwrap_or(c)
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
                if let Some(l) = word.chars().rev().find(|c| !ignorable(*c)) {
                    st.last = Some(l);
                }
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
            }
        }
    }
    if let Some(l) = word.chars().rev().find(|c| !ignorable(*c)) {
        st.last = Some(l);
    }
    flush(&mut word, out);
}

/// Caches text widths per (font, size).
pub(crate) struct Measurer<'a> {
    pub fonts: &'a FontContext,
    cache: rabbithole_gfx::fxhash::FxHashMap<(usize, u32), rabbithole_gfx::fxhash::FxHashMap<String, f32>>,
}

impl<'a> Measurer<'a> {
    pub fn new(fonts: &'a FontContext) -> Self {
        Measurer {
            fonts,
            cache: Default::default(),
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
    fn transform_tailorings() {
        let mut st = ComputedStyle::initial();
        st.text_transform = TextTransform::Uppercase;
        st.lang = LANG_TR;
        assert_eq!(transform_style("istanbul", &st), "İSTANBUL");
        st.lang = LANG_EL;
        assert_eq!(transform_style("καλημέρα", &st), "ΚΑΛΗΜΕΡΑ");
        st.lang = 0;
        st.text_transform = TextTransform::Capitalize;
        assert_eq!(transform_style("(hello ǆx", &st), "(Hello ǅx");
        st.text_transform_extra = 1;
        assert_eq!(transform_style("ab c", &st), "Ａｂ Ｃ");
    }

    #[test]
    fn transforms() {
        assert_eq!(transform("hello big world", TextTransform::Capitalize), "Hello Big World");
        assert_eq!(transform("Ab", TextTransform::Uppercase), "AB");
    }
}
