//! Resource fetching.

use crate::url::Url;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Response {
    /// Final URL after redirects.
    pub url: Url,
    pub status: u16,
    /// Lower-cased MIME type without parameters, e.g. `text/html`.
    pub content_type: String,
    /// Charset parameter from Content-Type, lower-cased.
    pub charset: Option<String>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone)]
pub enum FetchError {
    UnsupportedScheme(String),
    Io(String),
    Http(u16),
    Timeout,
    TooLarge,
    Other(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        unimplemented!()
    }
}

impl std::error::Error for FetchError {}

#[derive(Debug, Clone)]
pub struct Fetcher {
    pub user_agent: String,
    pub timeout: Duration,
    pub max_body: usize,
    /// Extra request headers (name, value).
    pub headers: Vec<(String, String)>,
}

impl Default for Fetcher {
    fn default() -> Self {
        Fetcher {
            user_agent: "Mozilla/5.0 (compatible; RabbitHole/0.1; +https://github.com/sergiogasca/RabbitHoleBrowser)".into(),
            timeout: Duration::from_secs(15),
            max_body: 32 * 1024 * 1024,
            headers: Vec::new(),
        }
    }
}

impl Fetcher {
    /// Fetch any supported URL: http/https (redirects followed, gzip
    /// handled), file, data (base64 or percent-encoded), about.
    pub fn fetch(&self, url: &Url) -> Result<Response, FetchError> {
        unimplemented!()
    }

    /// Fetch several URLs concurrently on a small thread pool, preserving
    /// order. Errors are per-item.
    pub fn fetch_many(&self, urls: &[Url], parallelism: usize) -> Vec<Result<Response, FetchError>> {
        unimplemented!()
    }
}

pub fn fetch(url: &Url) -> Result<Response, FetchError> {
    Fetcher::default().fetch(url)
}

pub fn fetch_many(urls: &[Url], parallelism: usize) -> Vec<Result<Response, FetchError>> {
    Fetcher::default().fetch_many(urls, parallelism)
}
