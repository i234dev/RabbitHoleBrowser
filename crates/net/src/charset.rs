//! Bytes -> String with charset detection (BOM, HTTP header hint,
//! `<meta charset>` sniffing, UTF-8 validation, Windows-1252 fallback).

/// Windows-1252 code points for bytes 0x80..=0x9F (U+FFFD for undefined).
const WIN1252_HIGH: [char; 32] = [
    '\u{20AC}', '\u{FFFD}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{FFFD}', '\u{017D}', '\u{FFFD}',
    '\u{FFFD}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{FFFD}', '\u{017E}', '\u{0178}',
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Utf8,
    Windows1252,
    Latin9,
    Utf16Le,
    Utf16Be,
}

/// Map a charset label to an encoding we can decode. `None` for unknown.
fn encoding_for_label(label: &str) -> Option<Encoding> {
    let l = label.trim().trim_matches(|c| c == '"' || c == '\'').to_ascii_lowercase();
    match l.as_str() {
        "utf-8" | "utf8" | "unicode-1-1-utf-8" => Some(Encoding::Utf8),
        "iso-8859-1" | "iso8859-1" | "iso_8859-1" | "latin1" | "latin-1" | "l1" | "windows-1252" | "cp1252"
        | "x-cp1252" | "ascii" | "us-ascii" | "iso-ir-100" | "ibm819" | "cp819" => Some(Encoding::Windows1252),
        "iso-8859-15" | "iso8859-15" | "latin9" | "latin-9" | "l9" => Some(Encoding::Latin9),
        "utf-16" | "utf-16le" | "utf16" => Some(Encoding::Utf16Le),
        "utf-16be" => Some(Encoding::Utf16Be),
        _ => None,
    }
}

fn decode_single_byte(body: &[u8], latin9: bool) -> String {
    let mut out = String::with_capacity(body.len());
    for &b in body {
        let c = if latin9 && b == 0xA4 {
            '\u{20AC}'
        } else if (0x80..=0x9F).contains(&b) && !latin9 {
            WIN1252_HIGH[(b - 0x80) as usize]
        } else {
            b as char
        };
        out.push(c);
    }
    out
}

fn decode_utf16(body: &[u8], big_endian: bool) -> String {
    let units: Vec<u16> = body
        .chunks(2)
        .map(|c| {
            let (a, b) = (c[0], *c.get(1).unwrap_or(&0));
            if big_endian {
                u16::from_be_bytes([a, b])
            } else {
                u16::from_le_bytes([a, b])
            }
        })
        .collect();
    String::from_utf16_lossy(&units)
}

fn decode_with(body: &[u8], enc: Encoding) -> String {
    match enc {
        Encoding::Utf8 => String::from_utf8_lossy(body).into_owned(),
        Encoding::Windows1252 => decode_single_byte(body, false),
        Encoding::Latin9 => decode_single_byte(body, true),
        Encoding::Utf16Le => decode_utf16(body, false),
        Encoding::Utf16Be => decode_utf16(body, true),
    }
}

/// Decode `body` to text. `hint` is the charset from the HTTP header.
pub fn decode_text(body: &[u8], hint: Option<&str>) -> String {
    // Byte order marks win over everything.
    if let Some(rest) = body.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if let Some(rest) = body.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16(rest, false);
    }
    if let Some(rest) = body.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16(rest, true);
    }
    let label = hint
        .map(|h| h.to_string())
        .or_else(|| sniff_meta_charset(&body[..body.len().min(2048)]));
    if let Some(enc) = label.as_deref().and_then(encoding_for_label) {
        return decode_with(body, enc);
    }
    match std::str::from_utf8(body) {
        Ok(s) => s.to_string(),
        Err(_) => decode_single_byte(body, false),
    }
}

/// Look for `<meta charset=...>` / `http-equiv` content-type in the first
/// 2 KiB of an HTML document.
pub fn sniff_meta_charset(head: &[u8]) -> Option<String> {
    let head = &head[..head.len().min(2048)];
    let lower: Vec<u8> = head.iter().map(|b| b.to_ascii_lowercase()).collect();
    let mut pos = 0;
    while let Some(off) = find(&lower[pos..], b"<meta") {
        let start = pos + off;
        let end = find(&lower[start..], b">").map(|e| start + e).unwrap_or(lower.len());
        let tag = &lower[start..end];
        if let Some(c) = find(tag, b"charset") {
            let mut i = c + b"charset".len();
            while i < tag.len() && tag[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < tag.len() && tag[i] == b'=' {
                i += 1;
                while i < tag.len() && (tag[i].is_ascii_whitespace() || tag[i] == b'"' || tag[i] == b'\'') {
                    i += 1;
                }
                let vstart = i;
                while i < tag.len()
                    && !tag[i].is_ascii_whitespace()
                    && !matches!(tag[i], b'"' | b'\'' | b';' | b'/' | b'>')
                {
                    i += 1;
                }
                if i > vstart {
                    return Some(String::from_utf8_lossy(&tag[vstart..i]).into_owned());
                }
            }
        }
        pos = end;
    }
    None
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_and_bom() {
        assert_eq!(decode_text("héllo".as_bytes(), None), "héllo");
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice("x".as_bytes());
        assert_eq!(decode_text(&bom, Some("iso-8859-1")), "x");
        // UTF-16LE BOM
        let mut u16le = vec![0xFF, 0xFE];
        for u in "hi".encode_utf16() {
            u16le.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_text(&u16le, None), "hi");
        let mut u16be = vec![0xFE, 0xFF];
        for u in "hi".encode_utf16() {
            u16be.extend_from_slice(&u.to_be_bytes());
        }
        assert_eq!(decode_text(&u16be, None), "hi");
    }

    #[test]
    fn latin1_fallback_and_hint() {
        let bytes = [b'c', b'a', b'f', 0xE9, b' ', 0x80];
        assert_eq!(decode_text(&bytes, None), "café €");
        assert_eq!(decode_text(&bytes, Some("ISO-8859-1")), "café €");
        assert_eq!(decode_text(&[0xA4], Some("iso-8859-15")), "€");
        assert_eq!(decode_text(&[0xA4], Some("latin1")), "¤");
        // Hint is honoured even for valid UTF-8 bytes.
        assert_eq!(decode_text("é".as_bytes(), Some("windows-1252")), "Ã©");
        // Unknown label with valid utf-8
        assert_eq!(decode_text("é".as_bytes(), Some("x-made-up")), "é");
    }

    #[test]
    fn meta_sniffing() {
        let html = b"<!doctype html><html><head><META  CHARSET = 'ISO-8859-1' ><title>x</title>";
        assert_eq!(sniff_meta_charset(html).as_deref(), Some("iso-8859-1"));
        let html = b"<meta http-equiv=\"Content-Type\" content=\"text/html; charset=windows-1252\">";
        assert_eq!(sniff_meta_charset(html).as_deref(), Some("windows-1252"));
        let html = b"<meta name=viewport content=width=device-width><meta charset=utf-8>";
        assert_eq!(sniff_meta_charset(html).as_deref(), Some("utf-8"));
        assert_eq!(sniff_meta_charset(b"<p>no meta</p>"), None);
        let doc = b"<meta charset=iso-8859-1><p>caf\xe9</p>";
        assert_eq!(decode_text(doc, None), "<meta charset=iso-8859-1><p>café</p>");
    }
}
