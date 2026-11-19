//! Automatic hyphenation (`hyphens: auto`) with Liang's algorithm and
//! the TeX patterns of American English.

use std::collections::HashMap;
use std::sync::OnceLock;

struct Patterns {
    /// Letters (with `.` for the word edges) → the digits around them.
    map: HashMap<&'static str, Vec<u8>>,
    max_len: usize,
    /// Whole words with their hyphenation points (char offsets).
    exceptions: HashMap<String, Vec<usize>>,
}

fn en_us() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| {
        let src: &'static str = include_str!("hyph-en-us.txt");
        let mut map = HashMap::new();
        let mut exceptions = HashMap::new();
        let mut max_len = 0;
        for line in src.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('%') {
                continue;
            }
            if line.contains('-') || !line.bytes().any(|b| b.is_ascii_digit()) {
                let mut word = String::new();
                let mut points = Vec::new();
                for c in line.chars() {
                    if c == '-' {
                        points.push(word.chars().count());
                    } else {
                        word.push(c);
                    }
                }
                exceptions.insert(word, points);
                continue;
            }
            // The digits of `a1b2c` → letters `abc`, values [0, 1, 2, 0].
            let mut values = vec![0u8];
            let mut letters_len = 0;
            for b in line.bytes() {
                if b.is_ascii_digit() {
                    *values.last_mut().unwrap() = b - b'0';
                } else {
                    values.push(0);
                    letters_len += 1;
                }
            }
            // Keep the letters as a slice of a leaked string (built once).
            let letters: String = line.chars().filter(|c| !c.is_ascii_digit()).collect();
            let letters: &'static str = Box::leak(letters.into_boxed_str());
            max_len = max_len.max(letters_len);
            map.insert(letters, values);
        }
        Patterns { map, max_len, exceptions }
    })
}

/// Hyphenation points of an English `word` (char offsets where a hyphen
/// may break it), keeping at least 2 letters before and 3 after a break;
/// none for words shorter than 5 letters or with non-ASCII letters.
pub(crate) fn hyphenate_en(word: &str, out: &mut Vec<usize>) {
    out.clear();
    let n = word.len();
    if n < 5 || !word.bytes().all(|b| b.is_ascii_alphabetic()) {
        return;
    }
    let p = en_us();
    let lower = word.to_ascii_lowercase();
    if let Some(e) = p.exceptions.get(&lower) {
        out.extend_from_slice(e);
        return;
    }
    let s = format!(".{lower}.");
    let mut points = vec![0u8; s.len() + 1];
    for i in 0..s.len() {
        for j in i + 1..=(i + p.max_len).min(s.len()) {
            if let Some(v) = p.map.get(&s[i..j]) {
                for (k, &d) in v.iter().enumerate() {
                    if points[i + k] < d {
                        points[i + k] = d;
                    }
                }
            }
        }
    }
    // points[k + 1] is the digit before letter k of the word.
    for k in 2..=n.saturating_sub(3) {
        if points[k + 1] % 2 == 1 {
            out.push(k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hy(w: &str) -> String {
        let mut pts = Vec::new();
        hyphenate_en(w, &mut pts);
        let mut out = String::new();
        for (i, c) in w.chars().enumerate() {
            if pts.contains(&i) {
                out.push('-');
            }
            out.push(c);
        }
        out
    }

    #[test]
    fn english_words() {
        assert_eq!(hy("example"), "ex-am-ple");
        assert_eq!(hy("hyphenation"), "hy-phen-ation");
        assert_eq!(hy("table"), "ta-ble");
        assert_eq!(hy("cat"), "cat");
    }
}
