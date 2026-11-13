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
    let preserve_spaces = matches!(ws, WhiteSpace::Pre | WhiteSpace::PreWrap | WhiteSpace::BreakSpaces);
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

/// Simplified line-breaking classes (UAX #14) for the characters that
/// break without spaces: ideographic scripts and their punctuation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lb {
    /// Ideographs, kana, Hangul syllables: breaks around them.
    Id,
    /// Conditional Japanese starters (small kana, prolonged sound mark):
    /// ideographic unless `line-break: strict`.
    Cj,
    /// Non-starters (iteration marks, middle dots...).
    Ns,
    /// Closing punctuation: no break before.
    Cl,
    /// Exclamation / interrogation: no break before.
    Ex,
    /// Infix separators (`,` `.` `:` `;`): no break before.
    Is,
    /// Opening punctuation: no break after.
    Op,
    /// Glue (no-break spaces, word joiner): no break around.
    Gl,
    /// Break after (ideographic space, dashes).
    Ba,
    /// Everything else (letters, digits...): no break between.
    Al,
}

fn lb_class(c: char) -> Lb {
    let u = c as u32;
    match u {
        0x00A0 | 0x2007 | 0x202F | 0x2060 | 0xFEFF | 0x200D | 0x034F => Lb::Gl,
        0x3000 | 0x2010 | 0x2012 | 0x2013 | 0x00AD => Lb::Ba,
        0x3041 | 0x3043 | 0x3045 | 0x3047 | 0x3049 | 0x3063 | 0x3083 | 0x3085 | 0x3087 | 0x308E | 0x3095 | 0x3096 | 0x30A1 | 0x30A3 | 0x30A5
        | 0x30A7 | 0x30A9 | 0x30C3 | 0x30E3 | 0x30E5 | 0x30E7 | 0x30EE | 0x30F5 | 0x30F6 | 0x30FC | 0x31F0..=0x31FF | 0xFF67..=0xFF70 => Lb::Cj,
        0x3005 | 0x303B | 0x309B..=0x309E | 0x30A0 | 0x30FB | 0x30FD | 0x30FE | 0x301C | 0x203C | 0x2047..=0x2049 | 0xFF65 | 0xFF9E | 0xFF9F => Lb::Ns,
        0x3001 | 0x3002 | 0xFF0C | 0xFF0E | 0xFF61 | 0xFF64 | 0x3009 | 0x300B | 0x300D | 0x300F | 0x3011 | 0x3015 | 0x3017 | 0x3019
        | 0x301B | 0x301E | 0x301F | 0xFF09 | 0xFF3D | 0xFF5D | 0xFF60 | 0xFF63 | 0xFE50 | 0xFE52 | 0x29 | 0x5D | 0x7D | 0x2019 | 0x201D => Lb::Cl,
        0x3008 | 0x300A | 0x300C | 0x300E | 0x3010 | 0x3014 | 0x3016 | 0x3018 | 0x301A | 0x301D | 0xFF08 | 0xFF3B | 0xFF5B | 0xFF5F | 0xFF62
        | 0x28 | 0x5B | 0x7B | 0x2018 | 0x201C => Lb::Op,
        0x21 | 0x3F | 0xFF01 | 0xFF1F => Lb::Ex,
        0x2C | 0x2E | 0x3A | 0x3B | 0xFF1A | 0xFF1B => Lb::Is,
        0x2E80..=0x2FFF | 0x3003 | 0x3004 | 0x3006 | 0x3007 | 0x3012 | 0x3013 | 0x3020..=0x303A | 0x303C..=0x303F | 0x3040..=0x309A
        | 0x309F | 0x30A2..=0x30FA | 0x30FF..=0x31EF | 0x3200..=0x4DBF | 0x4E00..=0x9FFF | 0xA000..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F | 0xFF02..=0xFF07 | 0xFF0A | 0xFF0B | 0xFF0D | 0xFF0F..=0xFF19 | 0xFF1C..=0xFF1E | 0xFF20..=0xFF3A | 0xFF3C
        | 0xFF3E..=0xFF5A | 0xFF5C | 0xFF5E | 0xFF66 | 0xFF71..=0xFF9D | 0xFFA0..=0xFFDC | 0xFFE0..=0xFFE6 | 0x20000..=0x3FFFD => Lb::Id,
        _ => Lb::Al,
    }
}

/// Is there a line break opportunity between `a` and `b` (no space
/// between them)? Only the breaks of ideographic text are found here:
/// spaces and hyphens are handled by the caller. `keep_all`: CJK text is
/// unbreakable like words (`word-break: keep-all`); `strict`: small kana
/// do not start a line (`line-break: strict`).
pub(crate) fn cjk_break_between(a: char, b: char, keep_all: bool, strict: bool) -> bool {
    let (mut ca, mut cb) = (lb_class(a), lb_class(b));
    if ca == Lb::Cj {
        ca = if strict { Lb::Ns } else { Lb::Id };
    }
    if cb == Lb::Cj {
        cb = if strict { Lb::Ns } else { Lb::Id };
    }
    if ca == Lb::Gl || cb == Lb::Gl || ca == Lb::Op {
        return false;
    }
    if matches!(cb, Lb::Cl | Lb::Ex | Lb::Is | Lb::Ns) {
        return false;
    }
    if ca == Lb::Ba {
        return cb != Lb::Ba;
    }
    if keep_all {
        // Only around ideographic punctuation.
        return matches!(ca, Lb::Cl | Lb::Ex | Lb::Is | Lb::Ns) && cb == Lb::Op;
    }
    match (ca, cb) {
        (Lb::Id, _) | (_, Lb::Id) => true,
        (Lb::Cl | Lb::Ex | Lb::Is | Lb::Ns, Lb::Op) => true,
        (Lb::Cl | Lb::Ns, Lb::Al) => lb_class(a) != Lb::Al && (a as u32) >= 0x2E80,
        _ => false,
    }
}

/// Split `w` (no spaces) at the line break opportunities of ideographic
/// text; returns the parts (one part when there is none).
pub(crate) fn split_cjk(w: &str, keep_all: bool, strict: bool, anywhere: bool) -> Vec<String> {
    if !anywhere && w.chars().all(|c| (c as u32) < 0x2E80 && (c as u32) != 0x3000) {
        return vec![w.to_string()];
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev: Option<char> = None;
    for c in w.chars() {
        if let Some(p) = prev {
            let brk = if anywhere {
                !matches!(lb_class(c), Lb::Gl) && !matches!(lb_class(p), Lb::Gl) && !is_combining(c)
            } else {
                cjk_break_between(p, c, keep_all, strict)
            };
            if brk && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        }
        cur.push(c);
        prev = Some(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn is_combining(c: char) -> bool {
    matches!(c as u32, 0x300..=0x36F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F | 0x3099 | 0x309A | 0x200C | 0x200D)
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

    #[test]
    fn ideographic_breaks() {
        // Between ideographs, not before closing punctuation or small kana
        // under `line-break: strict`, not after opening punctuation.
        assert_eq!(split_cjk("日本語", false, false, false), vec!["日", "本", "語"]);
        assert_eq!(split_cjk("本。「語」", false, false, false), vec!["本。", "「語」"]);
        assert_eq!(split_cjk("きゃ", false, true, false), vec!["きゃ"]);
        assert_eq!(split_cjk("きゃ", false, false, false), vec!["き", "ゃ"]);
        assert_eq!(split_cjk("日本語", true, false, false), vec!["日本語"]);
        assert_eq!(split_cjk("word", false, false, false), vec!["word"]);
        assert_eq!(split_cjk("ab", false, false, true), vec!["a", "b"]);
        assert!(cjk_break_between('a', '日', false, false));
        assert!(!cjk_break_between('a', 'b', false, false));
    }
}
