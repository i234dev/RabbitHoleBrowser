//! A small RFC 3986 URL implementation (parse, serialize, resolve).

use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Url {
    /// Lower-cased scheme without ':'
    pub scheme: String,
    /// Lower-cased host (empty for `file:`/`data:`/`about:`)
    pub host: String,
    pub port: Option<u16>,
    /// Always starts with '/' for hierarchical schemes; raw for `data:`/`about:`.
    pub path: String,
    pub query: Option<String>,
    pub fragment: Option<String>,
}

/// Schemes that always use the `//authority` form even when written without it.
const HIERARCHICAL: &[&str] = &["http", "https", "file", "ftp", "ws", "wss"];

fn is_scheme_char(c: u8, first: bool) -> bool {
    if first {
        c.is_ascii_alphabetic()
    } else {
        c.is_ascii_alphanumeric() || c == b'+' || c == b'-' || c == b'.'
    }
}

/// Split `input` at the first ':' if the prefix is a valid scheme.
fn split_scheme(input: &str) -> Option<(&str, &str)> {
    let colon = input.find(':')?;
    let scheme = &input[..colon];
    if scheme.is_empty() {
        return None;
    }
    for (i, &b) in scheme.as_bytes().iter().enumerate() {
        if !is_scheme_char(b, i == 0) {
            return None;
        }
    }
    Some((scheme, &input[colon + 1..]))
}

/// Percent-encode bytes that cannot appear verbatim in a URL path or query:
/// spaces, control characters, non-ASCII, `"`, `<`, `>`, `` ` ``, `{`, `}`,
/// `|`, `\`, `^`. Existing `%` sequences are left untouched.
pub fn percent_encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        let keep = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'.' | b'_' | b'~' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b','
                    | b';' | b'=' | b':' | b'@' | b'/' | b'?' | b'%' | b'[' | b']' | b'#'
            );
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Decode `%XX` escapes (invalid UTF-8 becomes U+FFFD). `+` is left as-is.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = &s[i + 1..i + 3];
            if let Ok(v) = u8::from_str_radix(h, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// RFC 3986 §5.2.4 remove_dot_segments.
fn remove_dot_segments(path: &str) -> String {
    let mut input = path.to_string();
    let mut output: Vec<String> = Vec::new();
    while !input.is_empty() {
        if let Some(rest) = input.strip_prefix("../") {
            input = rest.to_string();
        } else if let Some(rest) = input.strip_prefix("./") {
            input = rest.to_string();
        } else if let Some(rest) = input.strip_prefix("/./") {
            input = format!("/{rest}");
        } else if input == "/." {
            input = "/".to_string();
        } else if let Some(rest) = input.strip_prefix("/../") {
            input = format!("/{rest}");
            output.pop();
        } else if input == "/.." {
            input = "/".to_string();
            output.pop();
        } else if input == "." || input == ".." {
            input.clear();
        } else {
            // Move the first path segment (including the initial '/' if any,
            // up to but not including the next '/') to the output.
            let start = if input.starts_with('/') { 1 } else { 0 };
            let end = input[start..].find('/').map(|p| p + start).unwrap_or(input.len());
            output.push(input[..end].to_string());
            input = input[end..].to_string();
        }
    }
    output.concat()
}

/// Parse `[userinfo@]host[:port]`.
fn parse_authority(auth: &str) -> Option<(String, Option<u16>)> {
    let hostport = match auth.rfind('@') {
        Some(i) => &auth[i + 1..],
        None => auth,
    };
    let (host, port) = if let Some(rest) = hostport.strip_prefix('[') {
        let close = rest.find(']')?;
        let host = &hostport[..close + 2];
        let after = &rest[close + 1..];
        let port = match after.strip_prefix(':') {
            Some(p) if !p.is_empty() => Some(p.parse::<u16>().ok()?),
            Some(_) | None => None,
        };
        (host.to_string(), port)
    } else {
        match hostport.rfind(':') {
            Some(i) => {
                let p = &hostport[i + 1..];
                let port = if p.is_empty() { None } else { Some(p.parse::<u16>().ok()?) };
                (hostport[..i].to_string(), port)
            }
            None => (hostport.to_string(), None),
        }
    };
    if host.chars().any(|c| c.is_whitespace() || c == '/' || c == '?' || c == '#') {
        return None;
    }
    Some((host.to_ascii_lowercase(), port))
}

impl Url {
    /// Parse an absolute URL. Accepts input a user might type: a bare
    /// `example.com/x` gets `https://` prepended.
    pub fn parse(input: &str) -> Option<Url> {
        let input = input.trim();
        if input.is_empty() {
            return None;
        }
        if let Some((scheme, _)) = split_scheme(input) {
            // Guard against "localhost:8080/x" being read as scheme "localhost".
            let known = HIERARCHICAL.contains(&scheme.to_ascii_lowercase().as_str())
                || matches!(scheme.to_ascii_lowercase().as_str(), "data" | "about" | "mailto" | "javascript" | "blob" | "tel")
                || input[scheme.len() + 1..].starts_with("//");
            if known {
                if let Some(u) = Url::parse_absolute(input) {
                    return Some(u);
                }
            }
        }
        if input.starts_with('/') && std::path::Path::new(input).exists() {
            return Url::parse_absolute(&format!("file://{}", percent_encode_component(input)));
        }
        let head = input.split(['/', '?', '#']).next().unwrap_or("");
        let host_part = head.split(':').next().unwrap_or("");
        if host_part == "localhost" || host_part.starts_with("127.") {
            return Url::parse_absolute(&format!("http://{input}"));
        }
        let looks_like_host = !input.contains(char::is_whitespace)
            && !host_part.is_empty()
            && (host_part.contains('.') || input.starts_with("www."))
            && !host_part.starts_with('.')
            && !host_part.ends_with('.');
        if looks_like_host {
            return Url::parse_absolute(&format!("https://{input}"));
        }
        None
    }

    /// Strict absolute-URL parse (no scheme guessing).
    pub fn parse_absolute(input: &str) -> Option<Url> {
        let input = input.trim();
        let (scheme, rest) = split_scheme(input)?;
        let scheme = scheme.to_ascii_lowercase();
        let (rest, fragment) = match rest.find('#') {
            Some(i) => (&rest[..i], Some(percent_encode_component(&rest[i + 1..]))),
            None => (rest, None),
        };
        let hierarchical = rest.starts_with("//") || HIERARCHICAL.contains(&scheme.as_str());
        if !hierarchical {
            return Some(Url {
                scheme,
                host: String::new(),
                port: None,
                path: percent_encode_component(rest),
                query: None,
                fragment,
            });
        }
        let rest = rest.strip_prefix("//").unwrap_or(rest);
        let auth_end = rest.find(['/', '?']).unwrap_or(rest.len());
        let (host, port) = parse_authority(&rest[..auth_end])?;
        let rest = &rest[auth_end..];
        let (path, query) = match rest.find('?') {
            Some(i) => (&rest[..i], Some(percent_encode_component(&rest[i + 1..]))),
            None => (rest, None),
        };
        let path = if path.is_empty() { "/".to_string() } else { remove_dot_segments(&percent_encode_component(path)) };
        let mut url = Url {
            scheme,
            host,
            port,
            path,
            query,
            fragment,
        };
        if url.port.is_some() && url.port == url.default_port() {
            url.port = None;
        }
        Some(url)
    }

    /// Resolve `reference` (absolute, protocol-relative, path-absolute,
    /// relative, query-only or fragment-only) against `self`.
    pub fn join(&self, reference: &str) -> Option<Url> {
        let reference = reference.trim();
        if split_scheme(reference).is_some() {
            // A reference with a scheme is absolute.
            return Url::parse_absolute(reference);
        }
        let hierarchical = !self.host.is_empty() || HIERARCHICAL.contains(&self.scheme.as_str());
        if !hierarchical {
            return None;
        }
        if reference.is_empty() {
            return Some(self.clone());
        }
        if let Some(rest) = reference.strip_prefix("//") {
            return Url::parse_absolute(&format!("{}://{}", self.scheme, rest));
        }
        let (reference, fragment) = match reference.find('#') {
            Some(i) => (&reference[..i], Some(percent_encode_component(&reference[i + 1..]))),
            None => (reference, None),
        };
        if reference.is_empty() {
            // Fragment-only reference: same document.
            return Some(Url {
                fragment,
                ..self.clone()
            });
        }
        let (ref_path, query) = match reference.find('?') {
            Some(i) => (&reference[..i], Some(percent_encode_component(&reference[i + 1..]))),
            None => (reference, None),
        };
        let mut out = Url {
            scheme: self.scheme.clone(),
            host: self.host.clone(),
            port: self.port,
            path: String::new(),
            query,
            fragment,
        };
        if ref_path.is_empty() {
            out.path = self.path.clone();
            if out.query.is_none() {
                out.query = self.query.clone();
            }
        } else if ref_path.starts_with('/') {
            out.path = remove_dot_segments(&percent_encode_component(ref_path));
        } else {
            // Merge: base path up to and including the last '/'.
            let base_dir = match self.path.rfind('/') {
                Some(i) => &self.path[..=i],
                None => "/",
            };
            out.path = remove_dot_segments(&format!("{base_dir}{}", percent_encode_component(ref_path)));
        }
        if out.path.is_empty() {
            out.path = "/".to_string();
        }
        Some(out)
    }

    pub fn is_http(&self) -> bool {
        self.scheme == "http" || self.scheme == "https"
    }

    pub fn default_port(&self) -> Option<u16> {
        match self.scheme.as_str() {
            "http" => Some(80),
            "https" => Some(443),
            "ftp" => Some(21),
            "ws" => Some(80),
            "wss" => Some(443),
            _ => None,
        }
    }

    /// `host` plus `:port` when a non-default port is set.
    pub fn host_with_port(&self) -> String {
        match self.port {
            Some(p) => format!("{}:{}", self.host, p),
            None => self.host.clone(),
        }
    }

    pub fn origin(&self) -> String {
        if self.is_http() {
            format!("{}://{}", self.scheme, self.host_with_port())
        } else {
            "null".to_string()
        }
    }

    /// Same URL without the fragment.
    pub fn without_fragment(&self) -> Url {
        Url { fragment: None, ..self.clone() }
    }

    /// Filesystem path for a `file:` URL (percent-decoded).
    pub fn file_path(&self) -> Option<PathBuf> {
        if self.scheme != "file" {
            return None;
        }
        let decoded = percent_decode(&self.path);
        #[cfg(windows)]
        {
            // "/C:/Users/..." -> "C:/Users/..."
            let b = decoded.as_bytes();
            if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
                return Some(PathBuf::from(&decoded[1..]));
            }
        }
        Some(PathBuf::from(decoded))
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hierarchical = !self.host.is_empty() || HIERARCHICAL.contains(&self.scheme.as_str());
        if hierarchical {
            write!(f, "{}://{}{}", self.scheme, self.host_with_port(), self.path)?;
        } else {
            write!(f, "{}:{}", self.scheme, self.path)?;
        }
        if let Some(q) = &self.query {
            write!(f, "?{q}")?;
        }
        if let Some(fr) = &self.fragment {
            write!(f, "#{fr}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_absolute_basics() {
        let u = Url::parse_absolute("HTTP://User:pw@Example.COM:80/a/./b/../c?x=1#frag").unwrap();
        assert_eq!(u.scheme, "http");
        assert_eq!(u.host, "example.com");
        assert_eq!(u.port, None);
        assert_eq!(u.path, "/a/c");
        assert_eq!(u.query.as_deref(), Some("x=1"));
        assert_eq!(u.fragment.as_deref(), Some("frag"));
        assert_eq!(u.to_string(), "http://example.com/a/c?x=1#frag");

        let u = Url::parse_absolute("https://example.com:8443").unwrap();
        assert_eq!(u.port, Some(8443));
        assert_eq!(u.path, "/");
        assert_eq!(u.origin(), "https://example.com:8443");
        assert_eq!(u.host_with_port(), "example.com:8443");

        let u = Url::parse_absolute("http://[::1]:8080/x").unwrap();
        assert_eq!(u.host, "[::1]");
        assert_eq!(u.port, Some(8080));

        let u = Url::parse_absolute("file:///tmp/a b/ñ.html").unwrap();
        assert_eq!(u.host, "");
        assert_eq!(u.path, "/tmp/a%20b/%C3%B1.html");
        assert_eq!(u.file_path().unwrap(), PathBuf::from("/tmp/a b/ñ.html"));
        assert_eq!(u.to_string(), "file:///tmp/a%20b/%C3%B1.html");

        let u = Url::parse_absolute("data:text/plain;base64,SGVsbG8=").unwrap();
        assert_eq!(u.scheme, "data");
        assert_eq!(u.path, "text/plain;base64,SGVsbG8=");
        assert_eq!(u.to_string(), "data:text/plain;base64,SGVsbG8=");
        assert_eq!(u.origin(), "null");

        let u = Url::parse_absolute("about:blank").unwrap();
        assert_eq!(u.path, "blank");

        assert!(Url::parse_absolute("example.com").is_none());
        assert!(Url::parse_absolute("://x").is_none());
        assert!(Url::parse_absolute("1http://x").is_none());
        assert!(Url::parse_absolute("http://exa mple.com/").is_none());
        assert!(Url::parse_absolute("http://example.com:abc/").is_none());
    }

    #[test]
    fn parse_user_input() {
        assert_eq!(Url::parse("example.com/x").unwrap().to_string(), "https://example.com/x");
        assert_eq!(Url::parse("  www.example.com  ").unwrap().to_string(), "https://www.example.com/");
        assert_eq!(Url::parse("localhost:8080/x").unwrap().to_string(), "http://localhost:8080/x");
        assert_eq!(Url::parse("localhost").unwrap().to_string(), "http://localhost/");
        assert_eq!(Url::parse("http://example.com").unwrap().to_string(), "http://example.com/");
        assert_eq!(Url::parse("about:blank").unwrap().to_string(), "about:blank");
        assert!(Url::parse("hello world").is_none());
        assert!(Url::parse("").is_none());
        assert!(Url::parse("justaword").is_none());
        assert_eq!(Url::parse("/tmp").unwrap().to_string(), "file:///tmp");
    }

    #[test]
    fn rfc3986_normal_examples() {
        let base = Url::parse_absolute("http://a/b/c/d;p?q").unwrap();
        let cases = [
            ("g:h", "g:h"),
            ("g", "http://a/b/c/g"),
            ("./g", "http://a/b/c/g"),
            ("g/", "http://a/b/c/g/"),
            ("/g", "http://a/g"),
            ("//g", "http://g/"),
            ("?y", "http://a/b/c/d;p?y"),
            ("g?y", "http://a/b/c/g?y"),
            ("#s", "http://a/b/c/d;p?q#s"),
            ("g#s", "http://a/b/c/g#s"),
            ("g?y#s", "http://a/b/c/g?y#s"),
            (";x", "http://a/b/c/;x"),
            ("g;x", "http://a/b/c/g;x"),
            ("g;x?y#s", "http://a/b/c/g;x?y#s"),
            ("", "http://a/b/c/d;p?q"),
            (".", "http://a/b/c/"),
            ("./", "http://a/b/c/"),
            ("..", "http://a/b/"),
            ("../", "http://a/b/"),
            ("../g", "http://a/b/g"),
            ("../..", "http://a/"),
            ("../../", "http://a/"),
            ("../../g", "http://a/g"),
            // abnormal
            ("../../../g", "http://a/g"),
            ("../../../../g", "http://a/g"),
            ("/./g", "http://a/g"),
            ("/../g", "http://a/g"),
            ("g.", "http://a/b/c/g."),
            (".g", "http://a/b/c/.g"),
            ("g..", "http://a/b/c/g.."),
            ("..g", "http://a/b/c/..g"),
            ("./../g", "http://a/b/g"),
            ("./g/.", "http://a/b/c/g/"),
            ("g/./h", "http://a/b/c/g/h"),
            ("g/../h", "http://a/b/c/h"),
            ("g;x=1/./y", "http://a/b/c/g;x=1/y"),
            ("g;x=1/../y", "http://a/b/c/y"),
        ];
        for (reference, expected) in cases {
            let got = base.join(reference).unwrap_or_else(|| panic!("join failed for {reference:?}"));
            assert_eq!(got.to_string(), expected, "reference {reference:?}");
        }
    }

    #[test]
    fn join_edge_cases() {
        let base = Url::parse_absolute("https://example.com/dir/page.html?q=1#top").unwrap();
        assert_eq!(base.join("other.html").unwrap().to_string(), "https://example.com/dir/other.html");
        assert_eq!(base.join("//cdn.example.org/x.css").unwrap().to_string(), "https://cdn.example.org/x.css");
        assert_eq!(base.join("#sec").unwrap().to_string(), "https://example.com/dir/page.html?q=1#sec");
        assert_eq!(base.join("?a=b").unwrap().to_string(), "https://example.com/dir/page.html?a=b");
        assert_eq!(base.join("img/ñ x.png").unwrap().path, "/dir/img/%C3%B1%20x.png");
        let data = Url::parse_absolute("data:text/html,hi").unwrap();
        assert!(data.join("x.html").is_none());
        assert!(data.join("http://a/").is_some());
        let root = Url::parse_absolute("http://a").unwrap();
        assert_eq!(root.join("x").unwrap().to_string(), "http://a/x");
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("a%20b%C3%B1%zz%4"), "a bñ%zz%4");
        assert_eq!(percent_decode("plain"), "plain");
    }

    #[test]
    fn without_fragment() {
        let u = Url::parse_absolute("http://a/b#c").unwrap();
        assert_eq!(u.without_fragment().to_string(), "http://a/b");
    }
}
