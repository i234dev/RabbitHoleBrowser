//! MIME Sniffing type parsing and Fetch Content-Type header extraction.

pub fn is_http_token(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

const WHITESPACE: &[char] = &['\t', '\n', '\r', ' '];

struct MimeType {
    essence: String,
    parameters: Vec<(String, String)>,
}

impl MimeType {
    fn parse(input: &str) -> Option<Self> {
        let input = input.trim_matches(WHITESPACE);
        let (essence, mut tail) = input.split_once(';').unwrap_or((input, ""));
        let (ty, subtype) = essence.trim_end_matches(WHITESPACE).split_once('/')?;
        if !is_http_token(ty) || !is_http_token(subtype) {
            return None;
        }
        let mut result = Self { essence: format!("{ty}/{subtype}").to_ascii_lowercase(), parameters: Vec::new() };
        while !tail.is_empty() {
            tail = tail.trim_start_matches(WHITESPACE);
            let end = tail.find([';', '=']).unwrap_or(tail.len());
            let name = &tail[..end];
            tail = &tail[end..];
            if let Some(value) = tail.strip_prefix('=') {
                let quoted = value.starts_with('"');
                let mut decoded = String::new();
                if quoted {
                    let mut chars = value[1..].chars();
                    while let Some(c) = chars.next() {
                        if c == '"' { break; }
                        decoded.push(if c == '\\' { chars.next().unwrap_or('\\') } else { c });
                    }
                    tail = chars.as_str();
                } else {
                    let end = value.find(';').unwrap_or(value.len());
                    decoded.push_str(value[..end].trim_end_matches(WHITESPACE));
                    tail = &value[end..];
                }
                if is_http_token(name) && (quoted || !decoded.is_empty())
                    && decoded.chars().all(|c| matches!(c, '\t' | ' '..='~' | '\u{80}'..='\u{ff}'))
                    && !result.parameters.iter().any(|(k, _)| k.eq_ignore_ascii_case(name))
                {
                    result.parameters.push((name.to_ascii_lowercase(), decoded));
                }
            }
            tail = tail.split_once(';').map_or("", |(_, rest)| rest);
        }
        Some(result)
    }

    fn serialize(self) -> String {
        let mut out = self.essence;
        for (key, value) in self.parameters {
            out.push(';');
            out.push_str(&key);
            out.push('=');
            if is_http_token(&value) {
                out.push_str(&value);
            } else {
                out.push('"');
                for ch in value.chars() {
                    if matches!(ch, '"' | '\\') { out.push('\\'); }
                    out.push(ch);
                }
                out.push('"');
            }
        }
        out
    }
}

/// Extract a MIME type from all Content-Type values in header-list order.
pub fn from_content_type_headers<'a>(values: impl Iterator<Item = &'a str>) -> String {
    let mut selected: Option<MimeType> = None;
    let mut charset = None;
    for value in values {
        let mut quoted = false;
        let mut escaped = false;
        for item in value.split(|c| {
            if escaped { escaped = false; }
            else if quoted && c == '\\' { escaped = true; }
            else if c == '"' { quoted = !quoted; }
            else if !quoted && c == ',' { return true; }
            false
        }) {
            let Some(mut mime) = MimeType::parse(item) else { continue };
            if mime.essence == "*/*" { continue; }
            let declared = mime.parameters.iter().find(|(k, _)| k == "charset");
            if selected.as_ref().is_none_or(|prev| prev.essence != mime.essence) {
                charset = declared.map(|(_, v)| v.clone());
            } else if declared.is_none() {
                if let Some(v) = &charset { mime.parameters.push(("charset".into(), v.clone())); }
            }
            selected = Some(mime);
        }
    }
    selected.map_or_else(String::new, MimeType::serialize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_extracts_and_serializes_valid_mime_parameters() {
        let ct = from_content_type_headers(["invalid, Text/Plain; CHARSET=UTF-8", "text/plain; P=\"a,é;B\";p=x"].into_iter());
        assert_eq!(ct, "text/plain;p=\"a,é;B\";charset=UTF-8");
        assert_eq!(from_content_type_headers(["bad, */*, Image/PNG;Empty=\"\"; Bad Name=x;Bare;Q=cAse"].into_iter()), "image/png;empty=\"\";q=cAse");
    }
}
