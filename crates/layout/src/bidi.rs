//! The Unicode Bidirectional Algorithm (UAX #9): explicit embeddings,
//! overrides and isolates (X1–X10), weak and neutral types (W1–W7, N0–N2),
//! implicit levels (I1–I2), line-end resets (L1) and reordering (L2).

mod table;

/// Bidi_Class values.
#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BidiClass {
    L,
    R,
    AL,
    EN,
    ES,
    ET,
    AN,
    CS,
    NSM,
    BN,
    B,
    S,
    WS,
    ON,
    LRE,
    LRO,
    RLE,
    RLO,
    PDF,
    LRI,
    RLI,
    FSI,
    PDI,
}

use BidiClass::*;

pub(crate) const LRE_CH: char = '\u{202A}';
pub(crate) const RLE_CH: char = '\u{202B}';
pub(crate) const PDF_CH: char = '\u{202C}';
pub(crate) const LRO_CH: char = '\u{202D}';
pub(crate) const RLO_CH: char = '\u{202E}';
pub(crate) const LRI_CH: char = '\u{2066}';
pub(crate) const RLI_CH: char = '\u{2067}';
pub(crate) const FSI_CH: char = '\u{2068}';
pub(crate) const PDI_CH: char = '\u{2069}';

/// The Bidi_Class of a character.
pub(crate) fn class(c: char) -> BidiClass {
    let cp = c as u32;
    if cp < 0x80 {
        // Fast path for ASCII letters.
        if c.is_ascii_alphabetic() {
            return L;
        }
    }
    let r = table::BIDI_RANGES;
    match r.binary_search_by(|&(a, b, _)| {
        if b < cp {
            std::cmp::Ordering::Less
        } else if a > cp {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    }) {
        Ok(i) => r[i].2,
        Err(_) => L,
    }
}

/// Does the text contain anything that needs the algorithm (right-to-left
/// characters or explicit formatting characters)?
pub(crate) fn needs_bidi(text: &str) -> bool {
    text.chars().any(|c| (c as u32) >= 0x590 && matches!(class(c), R | AL | AN | RLE | RLO | LRE | LRO | RLI | LRI | FSI | PDF | PDI))
}

/// P2/P3: the direction of the first strong character (skipping isolates);
/// `None` when there is none.
pub(crate) fn first_strong(chars: &[char]) -> Option<bool> {
    let mut depth = 0usize;
    for &c in chars {
        match class(c) {
            LRI | RLI | FSI => depth += 1,
            PDI => depth = depth.saturating_sub(1),
            L if depth == 0 => return Some(false),
            R | AL if depth == 0 => return Some(true),
            _ => {}
        }
    }
    None
}

fn is_removed(t: BidiClass) -> bool {
    matches!(t, RLE | LRE | RLO | LRO | PDF | BN)
}

fn is_ni(t: BidiClass) -> bool {
    matches!(t, B | S | WS | ON | LRI | RLI | FSI | PDI)
}

fn bracket(c: char) -> Option<(char, bool)> {
    // (pair, is_opening)
    Some(match c {
        '(' => (')', true),
        ')' => ('(', false),
        '[' => (']', true),
        ']' => ('[', false),
        '{' => ('}', true),
        '}' => ('{', false),
        '\u{2329}' | '\u{3008}' => ('\u{3009}', true),
        '\u{232A}' | '\u{3009}' => ('\u{3008}', false),
        '\u{300A}' => ('\u{300B}', true),
        '\u{300B}' => ('\u{300A}', false),
        '\u{FF08}' => ('\u{FF09}', true),
        '\u{FF09}' => ('\u{FF08}', false),
        '\u{FF3B}' => ('\u{FF3D}', true),
        '\u{FF3D}' => ('\u{FF3B}', false),
        _ => return None,
    })
}

/// Resolve the embedding level of every character of a paragraph with
/// base level `base` (0 or 1).
pub(crate) fn resolve_levels(chars: &[char], base: u8) -> Vec<u8> {
    let n = chars.len();
    let orig: Vec<BidiClass> = chars.iter().map(|&c| class(c)).collect();
    let mut types = orig.clone();
    let mut levels = vec![base; n];

    // Matching PDIs of isolate initiators (BD9).
    let mut matching: Vec<Option<usize>> = vec![None; n];
    let mut matched_pdi = vec![false; n];
    {
        let mut stack: Vec<usize> = Vec::new();
        for i in 0..n {
            match orig[i] {
                LRI | RLI | FSI => stack.push(i),
                PDI => {
                    if let Some(o) = stack.pop() {
                        matching[o] = Some(i);
                        matched_pdi[i] = true;
                    }
                }
                B => stack.clear(),
                _ => {}
            }
        }
    }

    // X1–X8.
    #[derive(Clone, Copy)]
    struct Entry {
        level: u8,
        over: Option<BidiClass>,
        isolate: bool,
    }
    const MAX: u8 = 125;
    let mut stack = vec![Entry { level: base, over: None, isolate: false }];
    let mut overflow_isolates = 0usize;
    let mut overflow_embeddings = 0usize;
    let mut valid_isolates = 0usize;
    for i in 0..n {
        let top = *stack.last().unwrap();
        let t = orig[i];
        match t {
            RLE | LRE | RLO | LRO => {
                let rtl = matches!(t, RLE | RLO);
                let next = if rtl { (top.level + 1) | 1 } else { (top.level + 2) & !1 };
                levels[i] = top.level;
                if next <= MAX && overflow_isolates == 0 && overflow_embeddings == 0 {
                    let over = match t {
                        RLO => Some(R),
                        LRO => Some(L),
                        _ => None,
                    };
                    stack.push(Entry { level: next, over, isolate: false });
                } else if overflow_isolates == 0 {
                    overflow_embeddings += 1;
                }
            }
            RLI | LRI | FSI => {
                levels[i] = top.level;
                if let Some(o) = top.over {
                    types[i] = o;
                }
                let rtl = match t {
                    RLI => true,
                    LRI => false,
                    _ => {
                        let end = matching[i].unwrap_or(n);
                        first_strong(&chars[i + 1..end]).unwrap_or(false)
                    }
                };
                let next = if rtl { (top.level + 1) | 1 } else { (top.level + 2) & !1 };
                if next <= MAX && overflow_isolates == 0 && overflow_embeddings == 0 {
                    valid_isolates += 1;
                    stack.push(Entry { level: next, over: None, isolate: true });
                } else {
                    overflow_isolates += 1;
                }
            }
            PDI => {
                if overflow_isolates > 0 {
                    overflow_isolates -= 1;
                } else if valid_isolates > 0 {
                    overflow_embeddings = 0;
                    while stack.len() > 1 && !stack.last().unwrap().isolate {
                        stack.pop();
                    }
                    if stack.len() > 1 {
                        stack.pop();
                    }
                    valid_isolates -= 1;
                }
                let top = *stack.last().unwrap();
                levels[i] = top.level;
                if let Some(o) = top.over {
                    types[i] = o;
                }
            }
            PDF => {
                if overflow_isolates > 0 {
                } else if overflow_embeddings > 0 {
                    overflow_embeddings -= 1;
                } else if !top.isolate && stack.len() > 1 {
                    stack.pop();
                }
                levels[i] = stack.last().unwrap().level;
            }
            B => levels[i] = base,
            BN => levels[i] = top.level,
            _ => {
                levels[i] = top.level;
                if let Some(o) = top.over {
                    types[i] = o;
                }
            }
        }
    }

    // X9/X10: level runs of the remaining characters, chained into
    // isolating run sequences.
    let kept: Vec<usize> = (0..n).filter(|&i| !is_removed(orig[i])).collect();
    let mut runs: Vec<Vec<usize>> = Vec::new();
    for &i in &kept {
        match runs.last_mut() {
            Some(r) if levels[*r.last().unwrap()] == levels[i] => r.push(i),
            _ => runs.push(vec![i]),
        }
    }
    let mut run_of = vec![usize::MAX; n];
    for (k, r) in runs.iter().enumerate() {
        for &i in r {
            run_of[i] = k;
        }
    }
    let mut sequences: Vec<Vec<usize>> = Vec::new();
    for k in 0..runs.len() {
        let first = runs[k][0];
        if orig[first] == PDI && matched_pdi[first] {
            continue; // continues a sequence
        }
        let mut seq: Vec<usize> = Vec::new();
        let mut cur = k;
        loop {
            seq.extend_from_slice(&runs[cur]);
            let last = *runs[cur].last().unwrap();
            match (orig[last], matching[last]) {
                (LRI | RLI | FSI, Some(p)) if run_of[p] != usize::MAX => cur = run_of[p],
                _ => break,
            }
        }
        sequences.push(seq);
    }

    for seq in &sequences {
        let level = levels[seq[0]];
        let first = seq[0];
        let last = *seq.last().unwrap();
        let prev_level = (0..first).rev().find(|&j| !is_removed(orig[j])).map(|j| levels[j]).unwrap_or(base);
        let next_level = if matches!(orig[last], LRI | RLI | FSI) {
            base
        } else {
            (last + 1..n).find(|&j| !is_removed(orig[j])).map(|j| levels[j]).unwrap_or(base)
        };
        let sos = if level.max(prev_level) % 2 == 1 { R } else { L };
        let eos = if level.max(next_level) % 2 == 1 { R } else { L };
        let m = seq.len();
        let mut t: Vec<BidiClass> = seq.iter().map(|&i| types[i]).collect();

        // W1.
        for k in 0..m {
            if t[k] == NSM {
                t[k] = if k == 0 {
                    sos
                } else if matches!(t[k - 1], LRI | RLI | FSI | PDI) {
                    ON
                } else {
                    t[k - 1]
                };
            }
        }
        // W2, W3.
        let mut last_strong = sos;
        for x in t.iter_mut() {
            match *x {
                L | R | AL => last_strong = *x,
                EN if last_strong == AL => *x = AN,
                _ => {}
            }
        }
        for x in t.iter_mut() {
            if *x == AL {
                *x = R;
            }
        }
        // W4.
        for k in 1..m.saturating_sub(1) {
            if t[k] == ES && t[k - 1] == EN && t[k + 1] == EN {
                t[k] = EN;
            } else if t[k] == CS && t[k - 1] == EN && t[k + 1] == EN {
                t[k] = EN;
            } else if t[k] == CS && t[k - 1] == AN && t[k + 1] == AN {
                t[k] = AN;
            }
        }
        // W5.
        let mut k = 0;
        while k < m {
            if t[k] == ET {
                let s = k;
                while k < m && t[k] == ET {
                    k += 1;
                }
                let adjacent = (s > 0 && t[s - 1] == EN) || (k < m && t[k] == EN);
                if adjacent {
                    for x in &mut t[s..k] {
                        *x = EN;
                    }
                }
            } else {
                k += 1;
            }
        }
        // W6.
        for x in t.iter_mut() {
            if matches!(*x, ES | ET | CS) {
                *x = ON;
            }
        }
        // W7.
        let mut last_strong = sos;
        for x in t.iter_mut() {
            match *x {
                L | R => last_strong = *x,
                EN if last_strong == L => *x = L,
                _ => {}
            }
        }
        // N0: bracket pairs.
        let embedding = if level % 2 == 1 { R } else { L };
        let strong_dir = |x: BidiClass| match x {
            L => Some(L),
            R | AN | EN => Some(R),
            _ => None,
        };
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        let mut open: Vec<(char, usize)> = Vec::new();
        for k in 0..m {
            if t[k] != ON {
                continue;
            }
            let c = chars[seq[k]];
            if let Some((pair, opening)) = bracket(c) {
                if opening {
                    if open.len() >= 63 {
                        break;
                    }
                    open.push((pair, k));
                } else if let Some(pos) = open.iter().rposition(|&(p, _)| p == c || (c == '\u{3009}' && p == '\u{232A}')) {
                    pairs.push((open[pos].1, k));
                    open.truncate(pos);
                }
            }
        }
        pairs.sort();
        for &(a, b) in &pairs {
            let inside: Vec<BidiClass> = t[a + 1..b].iter().filter_map(|&x| strong_dir(x)).collect();
            let new = if inside.contains(&embedding) {
                Some(embedding)
            } else if let Some(&opp) = inside.first() {
                let before = t[..a].iter().rev().find_map(|&x| strong_dir(x)).unwrap_or(sos);
                Some(if before == opp { opp } else { embedding })
            } else {
                None
            };
            if let Some(d) = new {
                t[a] = d;
                t[b] = d;
                // NSMs after a bracket take its type.
                for idx in [a, b] {
                    let mut j = idx + 1;
                    while j < m && orig[seq[j]] == NSM {
                        t[j] = d;
                        j += 1;
                    }
                }
            }
        }
        // N1, N2.
        let mut k = 0;
        while k < m {
            if is_ni(t[k]) {
                let s = k;
                while k < m && is_ni(t[k]) {
                    k += 1;
                }
                let before = if s == 0 { sos } else { strong_dir(t[s - 1]).unwrap_or(embedding) };
                let after = if k == m { eos } else { strong_dir(t[k]).unwrap_or(embedding) };
                let d = if before == after { before } else { embedding };
                for x in &mut t[s..k] {
                    *x = d;
                }
            } else {
                k += 1;
            }
        }
        // I1, I2.
        for (k, &i) in seq.iter().enumerate() {
            let lv = levels[i];
            levels[i] = if lv % 2 == 0 {
                match t[k] {
                    R => lv + 1,
                    AN | EN => lv + 2,
                    _ => lv,
                }
            } else {
                match t[k] {
                    L | EN | AN => lv + 1,
                    _ => lv,
                }
            };
        }
    }
    // Removed characters take the level of the preceding character.
    for i in 0..n {
        if is_removed(orig[i]) {
            levels[i] = if i > 0 { levels[i - 1] } else { base };
        }
    }
    levels
}

/// L1 for one line: separators and trailing whitespace go back to the
/// paragraph level. `chars`/`levels` are the line's characters.
pub(crate) fn reset_line_end(chars: &[char], levels: &mut [u8], base: u8) {
    let n = chars.len();
    let mut trailing = true;
    for i in (0..n).rev() {
        let t = class(chars[i]);
        match t {
            S | B => {
                levels[i] = base;
                trailing = true;
            }
            WS | LRI | RLI | FSI | PDI | BN | RLE | LRE | RLO | LRO | PDF if trailing => levels[i] = base,
            _ => trailing = false,
        }
        if matches!(t, S | B) {
            // Whitespace before a separator resets too.
            let mut j = i;
            while j > 0 && matches!(class(chars[j - 1]), WS | LRI | RLI | FSI | PDI | BN) {
                j -= 1;
                levels[j] = base;
            }
        }
    }
}

/// L2: the visual order (indices into `levels`) of a line.
#[cfg(test)]
pub(crate) fn visual_order(levels: &[u8]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..levels.len()).collect();
    let Some(&max) = levels.iter().max() else { return order };
    let Some(min_odd) = levels.iter().copied().filter(|l| l % 2 == 1).min() else { return order };
    let mut lv = max;
    while lv >= min_odd && lv > 0 {
        let mut i = 0;
        while i < order.len() {
            if levels[order[i]] >= lv {
                let s = i;
                while i < order.len() && levels[order[i]] >= lv {
                    i += 1;
                }
                order[s..i].reverse();
            } else {
                i += 1;
            }
        }
        lv -= 1;
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn levels(s: &str, base: u8) -> Vec<u8> {
        let chars: Vec<char> = s.chars().collect();
        resolve_levels(&chars, base)
    }

    #[test]
    fn classes() {
        assert_eq!(class('a'), L);
        assert_eq!(class('\u{5D0}'), R);
        assert_eq!(class('\u{627}'), AL);
        assert_eq!(class('1'), EN);
        assert_eq!(class(' '), WS);
        assert_eq!(class('\u{202E}'), RLO);
    }

    #[test]
    fn mixed_ltr_paragraph() {
        // "ab אב cd": the Hebrew word is level 1.
        assert_eq!(levels("ab \u{5D0}\u{5D1} cd", 0), vec![0, 0, 0, 1, 1, 0, 0, 0]);
        // Numbers after Hebrew stay at level 2 inside an RTL paragraph.
        assert_eq!(levels("\u{5D0} 12", 1), vec![1, 1, 2, 2]);
    }

    #[test]
    fn override_and_order() {
        let s: Vec<char> = "\u{202E}abc\u{202C}".chars().collect();
        let lv = resolve_levels(&s, 0);
        assert_eq!(&lv[1..4], &[1, 1, 1]);
        assert_eq!(visual_order(&[0, 1, 1, 1, 0]), vec![0, 3, 2, 1, 4]);
    }

    #[test]
    fn brackets_follow_embedding() {
        // In an RTL paragraph "a(b)" keeps the brackets with the Latin run.
        let lv = levels("\u{5D0} a(b)", 1);
        assert_eq!(lv[3], 2);
        assert_eq!(lv[5], 2);
    }
}
