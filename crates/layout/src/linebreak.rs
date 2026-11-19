//! Line breaking opportunities (UAX #14) with the CSS Text 3 tailorings
//! of `line-break`, `word-break` and hyphens.
//!
//! Spaces are handled by white-space processing (they always separate
//! words); this module decides the opportunities between two characters
//! with no space between them ([`split_points`], [`direct_break`]) and
//! whether the spaces between two words are one ([`break_after_spaces`]).

mod table;

/// Line_Break property values.
#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lb {
    BK,
    CR,
    LF,
    NL,
    SP,
    ZW,
    WJ,
    GL,
    CM,
    ZWJ,
    OP,
    CL,
    CP,
    QU,
    NS,
    EX,
    SY,
    IS,
    PR,
    PO,
    NU,
    AL,
    HL,
    ID,
    IN,
    HY,
    BA,
    BB,
    B2,
    CB,
    CJ,
    H2,
    H3,
    JL,
    JV,
    JT,
    RI,
    EB,
    EM,
    SA,
}

use Lb::*;

/// The Line_Break class of `c` (AI, SG and XX resolved to AL).
pub(crate) fn class(c: char) -> Lb {
    let u = c as u32;
    if u < 0x80 {
        return match c {
            'a'..='z' | 'A'..='Z' | '#' | '&' | '*' | '<' | '=' | '>' | '@' | '^' | '_' | '`' | '~' => AL,
            '0'..='9' => NU,
            ' ' => SP,
            '\t' => BA,
            '\n' => LF,
            '\r' => CR,
            '\x0b' | '\x0c' => BK,
            '!' | '?' => EX,
            '"' | '\'' => QU,
            '$' | '+' | '\\' => PR,
            '%' => PO,
            '(' | '[' | '{' => OP,
            ')' | ']' => CP,
            '}' => CL,
            ',' | '.' | ':' | ';' => IS,
            '-' => HY,
            '/' => SY,
            '|' => BA,
            _ => CM,
        };
    }
    let t = table::LB_RANGES;
    let (mut lo, mut hi) = (0usize, t.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        let (a, b, cls) = t[mid];
        if u < a {
            hi = mid;
        } else if u > b {
            lo = mid + 1;
        } else {
            return cls;
        }
    }
    AL
}

/// Combining marks of the South-East Asian scripts (their SA class
/// resolves to CM, LB1).
fn sa_mark(c: char) -> bool {
    matches!(c as u32, 0x0E31 | 0x0E34..=0x0E3A | 0x0E47..=0x0E4E | 0x0EB1 | 0x0EB4..=0x0EBC | 0x0EC8..=0x0ECE | 0x102B..=0x103E | 0x1056..=0x1059 | 0x105E..=0x1060 | 0x1062..=0x1064 | 0x1067..=0x106D | 0x1071..=0x1074 | 0x1082..=0x108D | 0x108F | 0x109A..=0x109D | 0x17B4..=0x17D3 | 0x17DD | 0x1A55..=0x1A7F)
}

/// East Asian Width F, W or H (for LB30's exception on wide brackets).
fn wide(c: char) -> bool {
    matches!(c as u32, 0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xA000..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE10..=0xFE19 | 0xFE30..=0xFE6F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x20000..=0x3FFFD)
}

/// The CSS properties that tailor the breaking rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Rules {
    /// `line-break`: 0 auto / normal, 1 loose, 2 strict, 3 anywhere.
    pub line_break: u8,
    /// `word-break`: 0 normal, 1 keep-all, 2 auto-phrase, 3 break-all.
    pub word_break: u8,
    /// The content language is Chinese or Japanese (the `normal` and
    /// `loose` tailorings apply).
    pub cjk: bool,
    /// `white-space: break-spaces`.
    pub break_spaces: bool,
}

/// Initial quotation marks (Pi).
fn is_pi(c: char) -> bool {
    matches!(c, '\u{AB}' | '\u{2018}' | '\u{201B}' | '\u{201C}' | '\u{201F}' | '\u{2039}' | '\u{2E02}' | '\u{2E04}' | '\u{2E09}' | '\u{2E0C}' | '\u{2E1C}' | '\u{2E20}')
}

/// Final quotation marks (Pf).
fn is_pf(c: char) -> bool {
    matches!(c, '\u{BB}' | '\u{2019}' | '\u{201D}' | '\u{203A}' | '\u{2E03}' | '\u{2E05}' | '\u{2E0A}' | '\u{2E0D}' | '\u{2E1D}' | '\u{2E21}')
}

/// Space separators (Zs) other than U+0020 and the no-break ones.
pub(crate) fn is_space_separator(c: char) -> bool {
    matches!(c as u32, 0x1680 | 0x2000..=0x2006 | 0x2008..=0x200A | 0x205F | 0x3000)
}

/// A character with its resolved class.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Unit {
    pub c: char,
    pub cls: Lb,
}

impl Rules {
    /// The class of `c` under these rules (LB1 and the CSS tailorings).
    pub(crate) fn resolve(&self, c: char) -> Lb {
        match class(c) {
            CJ => {
                if self.line_break == 2 {
                    NS
                } else {
                    ID
                }
            }
            SA if sa_mark(c) => CM,
            SA | AL | HL | NU if self.word_break == 3 => ID,
            SA => AL,
            cls => cls,
        }
    }

    pub(crate) fn unit(&self, c: char) -> Unit {
        Unit { c, cls: self.resolve(c) }
    }
}

/// Is a letter unit of a word (keep-all suppresses breaks between them)?
fn letter(cls: Lb) -> bool {
    matches!(cls, NU | AL | HL | ID | H2 | H3 | JL | JV | JT | CJ | EB | EM)
}

/// May the line break between `a` and `b` with nothing between them?
/// `before_a`: the unit before `a` (for LB21a / hyphen rules), if any in
/// the same word; `after_b`: the character after `b`, if known.
pub(crate) fn direct_break(rules: &Rules, before_a: Option<Unit>, a: Unit, b: Unit, after_b: Option<char>) -> bool {
    let (ca, cb) = (a.cls, b.cls);
    // LB7-LB9: no break before spaces / combining marks.
    if matches!(cb, CM | ZWJ | SP) {
        return false;
    }
    if ca == ZW {
        return true;
    }
    if matches!(ca, WJ | GL) || cb == WJ {
        return false;
    }
    if rules.word_break == 2 {
        // auto-phrase: keep-all plus a break where a phrase starts (here:
        // kanji or katakana after hiragana).
        let hira = |c: char| matches!(c as u32, 0x3041..=0x309F);
        let starts_phrase = |c: char| matches!(c as u32, 0x30A1..=0x30FA | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF);
        if hira(a.c) && starts_phrase(b.c) {
            return true;
        }
    }
    // LB8a.
    if ca == ZWJ {
        return false;
    }
    // `white-space: break-spaces`: a break after every space separator.
    if rules.break_spaces && is_space_separator(a.c) {
        return true;
    }
    let loose = rules.line_break == 1 && rules.cjk;
    let normal_or_loose = rules.line_break != 2;
    // CSS Text 3 §5.3: tailorings after ideographs.
    if ca == ID {
        // Hyphens, and wave dashes in Chinese or Japanese (normal, loose).
        if normal_or_loose && (matches!(b.c, '\u{2010}' | '\u{2013}') || (rules.cjk && matches!(b.c, '\u{301C}' | '\u{30A0}'))) {
            return true;
        }
        if loose
            && (matches!(b.c, '\u{30FB}' | '\u{FF1A}' | '\u{FF1B}' | '\u{FF65}' | '\u{203C}' | '\u{2047}'..='\u{2049}' | '\u{FF01}' | '\u{FF1F}')
                || matches!(b.c, '\u{3005}' | '\u{303B}' | '\u{309D}' | '\u{309E}' | '\u{30FD}' | '\u{30FE}')
                || cb == PO)
        {
            return true;
        }
    }
    if loose && ((ca == IN && cb == IN) || (ca == PR && cb == ID)) {
        return true;
    }
    if cb == GL && !matches!(ca, BA | HY) {
        return false;
    }
    // LB13-LB17.
    if matches!(cb, CL | CP | EX | IS | SY) || ca == OP {
        return false;
    }
    if (ca == QU && cb == OP) || (matches!(ca, CL | CP) && cb == NS) || (ca == B2 && cb == B2) {
        return false;
    }
    // LB19-LB21 (LB19a: quotation marks between East Asian characters
    // allow breaks).
    if cb == QU {
        return is_pi(b.c) && wide(a.c) && after_b.is_some_and(wide);
    }
    if ca == QU {
        return is_pf(a.c) && before_a.is_some_and(|u| wide(u.c)) && wide(b.c);
    }
    if ca == CB || cb == CB {
        return true;
    }
    if matches!(cb, BA | HY | NS) || ca == BB {
        return false;
    }
    if matches!(ca, HY | BA) && before_a.is_some_and(|u| u.cls == HL) {
        return false;
    }
    // A hyphen starting a word does not end a line on its own.
    if ca == HY && before_a.is_none() {
        return false;
    }
    if (ca == SY && cb == HL) || cb == IN {
        return false;
    }
    // LB23-LB25: letters with numbers, numbers with affixes.
    match (ca, cb) {
        (AL | HL, NU) | (NU, AL | HL) => return false,
        (PR, ID | EB | EM) | (ID | EB | EM, PO) => return false,
        (PR | PO, AL | HL) | (AL | HL, PR | PO) => return false,
        (CL | CP | NU, PO | PR) | (PO | PR, OP) | (PO | PR | HY | IS | NU | SY, NU) => return false,
        // LB26-LB27: Korean syllables.
        (JL, JL | JV | H2 | H3) | (JV | H2, JV | JT) | (JT | H3, JT) => return false,
        (JL | JV | JT | H2 | H3, PO) | (PR, JL | JV | JT | H2 | H3) => return false,
        // LB28-LB30.
        (AL | HL, AL | HL) | (IS, AL | HL) => return false,
        (AL | HL | NU, OP) if !wide(b.c) => return false,
        (CP, AL | HL | NU) if !wide(a.c) => return false,
        (RI, RI) | (EB, EM) => return false,
        _ => {}
    }
    // keep-all: no break between letters.
    if rules.word_break != 0 && rules.word_break != 3 && letter(ca) && letter(cb) {
        return false;
    }
    true
}

/// May the line break at the spaces between a word ending with `a` (the
/// last character before the spaces, if any) and one starting with `b`?
pub(crate) fn break_after_spaces(rules: &Rules, a: Option<char>, b: char) -> bool {
    let cb = rules.resolve(b);
    if matches!(cb, CL | CP | EX | IS | SY | WJ) {
        return false;
    }
    let Some(a) = a else { return true };
    let ca = rules.resolve(a);
    !(ca == OP || (ca == QU && cb == OP) || (matches!(ca, CL | CP) && cb == NS) || (ca == B2 && cb == B2))
}

/// Split `word` (no spaces) at its line break opportunities; `prev`: the
/// character before the word when no space separates them. Returns the
/// byte offsets where a new part starts (empty when there is none).
pub(crate) fn split_points(rules: &Rules, word: &str, out: &mut Vec<usize>) {
    out.clear();
    if rules.word_break != 3 && rules.line_break != 3 && word.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return;
    }
    let anywhere = rules.line_break == 3;
    let mut before: Option<Unit> = None;
    let mut prev: Option<Unit> = None;
    let mut after_zwj = false;
    let mut it = word.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let mut u = rules.unit(c);
        // NBSP inside a word (white-space processing keeps it as a space).
        if c == ' ' {
            u.cls = GL;
        }
        if let Some(p) = prev {
            let brk = if anywhere {
                u.cls != CM
            } else {
                !after_zwj && direct_break(rules, before, p, u, it.peek().map(|n| n.1))
            };
            if brk {
                out.push(i);
            }
            after_zwj = u.cls == ZWJ;
            // LB9: combining marks take the class of their base.
            if matches!(u.cls, CM | ZWJ) {
                continue;
            }
            // LB30a: regional indicators pair up.
            if p.cls == RI && u.cls == RI {
                before = Some(p);
                prev = Some(Unit { c, cls: AL });
                continue;
            }
        } else if matches!(u.cls, CM | ZWJ) {
            // LB10: a mark starting the word is a letter.
            after_zwj = u.cls == ZWJ;
            u.cls = AL;
        }
        before = prev;
        prev = Some(u);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(rules: Rules, w: &str) -> Vec<String> {
        let mut pts = Vec::new();
        split_points(&rules, w, &mut pts);
        let mut out = Vec::new();
        let mut start = 0;
        for p in pts {
            out.push(w[start..p].to_string());
            start = p;
        }
        out.push(w[start..].to_string());
        out
    }

    #[test]
    fn classes() {
        assert_eq!(class('a'), AL);
        assert_eq!(class('日'), ID);
        assert_eq!(class('。'), CL);
        assert_eq!(class('「'), OP);
        assert_eq!(class('ゃ'), CJ);
        assert_eq!(class('\u{0F3A}'), OP);
        assert_eq!(class('\u{2014}'), B2);
    }

    #[test]
    fn ideographic_breaks() {
        let r = Rules::default();
        assert_eq!(split(r, "日本語"), vec!["日", "本", "語"]);
        assert_eq!(split(r, "中\u{201D}中"), vec!["中\u{201D}", "中"]);
        assert_eq!(split(r, "a\u{200D}日"), vec!["a\u{200D}日"]);
        assert_eq!(split(r, "本。「語」"), vec!["本。", "「語」"]);
        let strict = Rules { line_break: 2, ..r };
        assert_eq!(split(strict, "きゃ"), vec!["きゃ"]);
        assert_eq!(split(r, "きゃ"), vec!["き", "ゃ"]);
        let keep = Rules { word_break: 1, ..r };
        assert_eq!(split(keep, "日本語"), vec!["日本語"]);
        assert_eq!(split(r, "word"), vec!["word"]);
        let anywhere = Rules { line_break: 3, ..r };
        assert_eq!(split(anywhere, "ab"), vec!["a", "b"]);
    }

    #[test]
    fn hyphens_and_punctuation() {
        let r = Rules::default();
        assert_eq!(split(r, "foo-bar"), vec!["foo-", "bar"]);
        assert_eq!(split(r, "-foo"), vec!["-foo"]);
        assert_eq!(split(r, "a-1"), vec!["a-1"]);
        assert_eq!(split(r, "(foo)"), vec!["(foo)"]);
        assert_eq!(split(r, "and/or"), vec!["and/", "or"]);
        assert_eq!(split(r, "$100.00"), vec!["$100.00"]);
        let all = Rules { word_break: 3, ..r };
        assert_eq!(split(all, "ab,c"), vec!["a", "b,", "c"]);
    }

    #[test]
    fn spaces() {
        let r = Rules::default();
        assert!(break_after_spaces(&r, Some('a'), 'b'));
        assert!(!break_after_spaces(&r, Some('a'), '!'));
        assert!(!break_after_spaces(&r, Some('('), 'b'));
        assert!(break_after_spaces(&r, None, 'b'));
    }
}
