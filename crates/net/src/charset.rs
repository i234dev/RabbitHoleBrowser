//! Bytes -> String with charset detection (BOM, HTTP header hint,
//! `<meta charset>` sniffing, UTF-8 validation, Windows-1252 fallback).

/// Decode `body` to text. `hint` is the charset from the HTTP header.
pub fn decode_text(body: &[u8], hint: Option<&str>) -> String {
    unimplemented!()
}

/// Look for `<meta charset=...>` / `http-equiv` content-type in the first
/// 2 KiB of an HTML document.
pub fn sniff_meta_charset(head: &[u8]) -> Option<String> {
    unimplemented!()
}
