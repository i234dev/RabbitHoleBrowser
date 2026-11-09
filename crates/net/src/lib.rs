//! Networking: URL parsing/resolution, fetching over http(s)/file/data/about
//! and text decoding. HTTP is plain `ureq` + rustls (pure Rust TLS).

pub mod url;
pub mod fetch;
pub mod charset;

pub use url::Url;
pub use fetch::{fetch, fetch_many, FetchError, Fetcher, Response};
pub use charset::decode_text;
