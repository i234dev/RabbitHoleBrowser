//! Character references: the full named reference table (see
//! `entity_table.rs`) with the tokenizer's longest-match rule, and numeric
//! references with the HTML replacements.

use crate::entity_table::ENTITIES;

fn exact(name: &str) -> Option<&'static str> {
    ENTITIES.binary_search_by(|(k, _)| k.as_bytes().cmp(name.as_bytes())).ok().map(|i| ENTITIES[i].1)
}

/// Value of the named reference `name` (without `&`; with or without the
/// trailing `;`).
pub fn lookup(name: &str) -> Option<&'static str> {
    if name.ends_with(';') {
        return exact(name);
    }
    exact(&format!("{name};")).or_else(|| exact(name))
}

/// Longest named reference at the start of `s` (the text after `&`):
/// returns (bytes consumed, including the `;` when part of the name;
/// value).
pub fn match_prefix(s: &str) -> Option<(usize, &'static str)> {
    let b = s.as_bytes();
    let mut run = 0;
    while run < b.len() && run < 40 && b[run].is_ascii_alphanumeric() {
        run += 1;
    }
    if run == 0 {
        return None;
    }
    if b.get(run) == Some(&b';') {
        if let Some(v) = exact(&s[..=run]) {
            return Some((run + 1, v));
        }
    }
    // Names are matched as prefixes: the longest one that exists (the
    // ones without `;` are the legacy references).
    for len in (1..=run).rev() {
        if let Some(v) = exact(&s[..len]) {
            return Some((len, v));
        }
    }
    None
}

/// Decode a numeric reference code point, applying the HTML5 replacements
/// for the Windows-1252 range and invalid code points.
pub fn numeric(cp: u32) -> char {
    const WIN1252: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ',
        '\u{9d}', 'ž', 'Ÿ',
    ];
    if (0x80..=0x9f).contains(&cp) {
        return WIN1252[(cp - 0x80) as usize];
    }
    if cp == 0 || cp > 0x10ffff || (0xd800..=0xdfff).contains(&cp) {
        return '\u{fffd}';
    }
    char::from_u32(cp).unwrap_or('\u{fffd}')
}

/// Decode character references in `input` as the tokenizer does in text
/// (`in_attribute`: the attribute-value rules, where `&name` followed by
/// `=` or an alphanumeric stays as written, so `?a=1&copy=2` survives).
pub fn decode(input: &str, in_attribute: bool) -> String {
    if !input.contains('&') {
        return input.to_string();
    }
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'&' {
            let start = i;
            while i < bytes.len() && bytes[i] != b'&' {
                i += 1;
            }
            out.push_str(&input[start..i]);
            continue;
        }
        let rest = &input[i + 1..];
        let rb = rest.as_bytes();
        if rb.first() == Some(&b'#') {
            let (hex, mut j) = match rb.get(1) {
                Some(b'x') | Some(b'X') => (true, 2),
                _ => (false, 1),
            };
            let num_start = j;
            while j < rb.len() && (if hex { rb[j].is_ascii_hexdigit() } else { rb[j].is_ascii_digit() }) {
                j += 1;
            }
            if j == num_start {
                out.push('&');
                i += 1;
                continue;
            }
            let digits = &rest[num_start..j];
            let cp = u32::from_str_radix(digits.trim_start_matches('0'), if hex { 16 } else { 10 }).unwrap_or(if digits.trim_start_matches('0').is_empty() { 0 } else { 0x110000 });
            out.push(numeric(cp));
            i += 1 + j;
            if i < bytes.len() && bytes[i] == b';' {
                i += 1;
            }
            continue;
        }
        match match_prefix(rest) {
            Some((used, v)) => {
                let semi = rest.as_bytes()[used - 1] == b';';
                if !semi && in_attribute {
                    if let Some(&c) = rb.get(used) {
                        if c == b'=' || c.is_ascii_alphanumeric() {
                            out.push('&');
                            out.push_str(&rest[..used]);
                            i += 1 + used;
                            continue;
                        }
                    }
                }
                out.push_str(v);
                i += 1 + used;
            }
            None => {
                out.push('&');
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn table_is_sorted() {
        for w in ENTITIES.windows(2) {
            assert!(w[0].0.as_bytes() < w[1].0.as_bytes(), "{} >= {}", w[0].0, w[1].0);
        }
        assert_eq!(ENTITIES.len(), 2231);
    }
    #[test]
    fn decodes() {
        assert_eq!(decode("a &amp; b &lt;c&gt; &#65;&#x42; &nbsp;x &copy &foo;", false), "a & b <c> AB \u{a0}x © &foo;");
        assert_eq!(decode("?a=1&copy=2", true), "?a=1&copy=2");
        assert_eq!(decode("?a=1&copy;=2", true), "?a=1©=2");
        assert_eq!(decode("&notit;", false), "¬it;");
        assert_eq!(decode("&notin;", false), "∉");
    }
}
