//! Bytes -> String with charset detection (BOM, HTTP header hint,
//! `<meta charset>` sniffing, UTF-8 validation, Windows-1252 fallback).

use crate::encoding::{bom_sniff, Encoding};

/// Decode `body` to text. `hint` is the charset from the HTTP header.
/// Order: byte order mark, the hint, `<meta charset>` in the first 2 KiB,
/// then UTF-8 when the bytes are valid UTF-8, else windows-1252.
pub fn decode_text(body: &[u8], hint: Option<&str>) -> String {
    decode_text_with_encoding(body, hint).0
}

/// [`decode_text`], also returning the encoding used (its name).
pub fn decode_text_with_encoding(body: &[u8], hint: Option<&str>) -> (String, &'static str) {
    if let Some((enc, n)) = bom_sniff(body) {
        return (enc.decode_without_bom(&body[n..]), enc.name());
    }
    let label = hint.map(|h| h.trim().trim_matches(|c| c == '"' || c == '\'').to_string()).or_else(|| sniff_meta_charset(&body[..body.len().min(2048)]));
    if let Some(mut enc) = label.as_deref().and_then(Encoding::for_label) {
        // A document's encoding is never UTF-16 from a label alone
        // (HTML "get an encoding" for meta: UTF-16 means UTF-8), nor
        // `replacement` for text that is plainly UTF-8.
        if hint.is_none() && (enc == Encoding::UTF_16BE || enc == Encoding::UTF_16LE) {
            enc = Encoding::UTF_8;
        }
        if enc.name() == "x-user-defined" && hint.is_none() {
            enc = Encoding::WINDOWS_1252;
        }
        return (enc.decode_without_bom(body), enc.name());
    }
    match std::str::from_utf8(body) {
        Ok(s) => (s.to_string(), "UTF-8"),
        Err(_) => (Encoding::WINDOWS_1252.decode_without_bom(body), "windows-1252"),
    }
}

/// Decode a style sheet (CSS Syntax 3 §3.2): byte order mark, then the
/// HTTP charset, then an `@charset "...";` rule at the very start (UTF-16
/// labels mean UTF-8 there), then as [`decode_text`] without a hint.
pub fn decode_css(body: &[u8], hint: Option<&str>) -> String {
    if bom_sniff(body).is_some() {
        return decode_text(body, None);
    }
    if let Some(enc) = hint.map(|h| h.trim().trim_matches(|c| c == '"' || c == '\'')).and_then(Encoding::for_label) {
        return enc.decode_without_bom(body);
    }
    const PREFIX: &[u8] = b"@charset \"";
    if body.starts_with(PREFIX) {
        let rest = &body[PREFIX.len()..body.len().min(1024)];
        if let Some(end) = rest.windows(2).position(|w| w == b"\";") {
            if let Some(mut enc) = std::str::from_utf8(&rest[..end]).ok().and_then(Encoding::for_label) {
                if enc == Encoding::UTF_16BE || enc == Encoding::UTF_16LE {
                    enc = Encoding::UTF_8;
                }
                return enc.decode_without_bom(body);
            }
        }
    }
    match std::str::from_utf8(body) {
        Ok(s) => s.to_string(),
        Err(_) => Encoding::WINDOWS_1252.decode_without_bom(body),
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
mod css_tests {
    use super::decode_css;

    #[test]
    fn css_encoding_from_http_then_at_charset() {
        // "\u{3042}" in Shift_JIS is 0x82 0xA0.
        let sjis = b"@charset \"windows-1252\"; .\x82\xa0 {}";
        assert!(decode_css(sjis, Some("Shift_JIS")).contains('\u{3042}'));
        let at = b"@charset \"shift_jis\"; .\x82\xa0 {}";
        assert!(decode_css(at, None).contains('\u{3042}'));
        // UTF-16 labels in @charset mean UTF-8.
        assert_eq!(decode_css("@charset \"UTF-16LE\"; .\u{e9} {}".as_bytes(), None), "@charset \"UTF-16LE\"; .\u{e9} {}");
    }
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
        assert_eq!(decode_text(&[0x82, 0xA0], Some("shift_jis")), "あ");
        assert_eq!(decode_text(&[0xA4, 0xA2], Some("euc-jp")), "あ");
        assert_eq!(decode_text(&[0xB0, 0xA1], Some("euc-kr")), "가");
        assert_eq!(decode_text(&[0xA4, 0x40], Some("big5")), "一");
        assert_eq!(decode_text(&[0x81, 0x30, 0x81, 0x30], Some("gb18030")), "\u{80}");
        assert_eq!(decode_text(&[0xE4, 0xF6], Some("koi8-r")), "ДЖ");
        assert_eq!(decode_text(b"\x1b$B$\"\x1b(B", Some("iso-2022-jp")), "あ");
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
