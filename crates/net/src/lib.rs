//! Networking: URL parsing/resolution, fetching over http(s)/file/data/about
//! and text decoding. HTTP is plain `ureq` + rustls (pure Rust TLS).

pub mod url;
pub mod fetch;
pub mod charset;

pub use url::{percent_decode, percent_encode_component, Url};
pub use fetch::{base64_decode, fetch, fetch_many, mime_for_extension, parse_content_type, FetchError, Fetcher, Response};
pub use charset::{decode_text, sniff_meta_charset};
