//! A small RFC 3986 URL implementation (parse, serialize, resolve).

use std::fmt;

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

impl Url {
    /// Parse an absolute URL. Accepts input a user might type: a bare
    /// `example.com/x` gets `https://` prepended.
    pub fn parse(input: &str) -> Option<Url> {
        unimplemented!()
    }

    /// Strict absolute-URL parse (no scheme guessing).
    pub fn parse_absolute(input: &str) -> Option<Url> {
        unimplemented!()
    }

    /// Resolve `reference` (absolute, protocol-relative, path-absolute,
    /// relative, query-only or fragment-only) against `self`.
    pub fn join(&self, reference: &str) -> Option<Url> {
        unimplemented!()
    }

    pub fn is_http(&self) -> bool {
        self.scheme == "http" || self.scheme == "https"
    }

    pub fn default_port(&self) -> Option<u16> {
        match self.scheme.as_str() {
            "http" => Some(80),
            "https" => Some(443),
            _ => None,
        }
    }

    pub fn origin(&self) -> String {
        unimplemented!()
    }

    /// Same URL without the fragment.
    pub fn without_fragment(&self) -> Url {
        Url { fragment: None, ..self.clone() }
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        unimplemented!()
    }
}
