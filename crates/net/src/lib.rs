//! Networking: URL parsing/resolution, fetching over http(s)/file/data/about
//! and text decoding.
//!
//! The HTTP stack is our own: HTTP/2 (framing, HPACK, flow control) and
//! HTTP/1.1 over TCP or TLS (rustls, ALPN `h2`/`http/1.1`), connection
//! pooling and per-host limits (`client`), an RFC 9111 HTTP cache in
//! memory and on disk (`cache`), Fetch-standard CORS / referrer policy /
//! redirect semantics for scripts (`cors`), plus WebSocket (`websocket`)
//! and EventSource (`eventsource`) clients.

pub mod url;
pub mod fetch;
pub mod charset;
pub mod cookies;
pub mod cache;
pub mod client;
pub mod cors;
pub mod h1;
pub mod h2;
pub mod transport;
pub mod websocket;
pub mod eventsource;
pub mod netlog;
pub mod robots;

pub use url::{percent_decode, percent_encode_component, site_of, Url};
pub use cookies::{Cookie, CookieContext, CookieJar};
pub use fetch::{
    base64_decode, bot_user_agent, browser_headers, decode_content, default_user_agent, fetch, fetch_many, form_encode, mime_for_extension, parse_content_type, sanitize_filename, ApiResponse, Body, Destination,
    FetchError, Fetcher, HeaderProfile, NetContext, NetEvent, NetObserver, Request, Response, StreamedResponse,
};
pub use cache::{platform_cache_dir, CacheMode, HttpCache};
pub use client::Priority;
pub use cors::{CredentialsMode, RedirectMode, ReferrerPolicy, RequestMode, ResponseType};
pub use websocket::{WebSocket, WsEvent, WsMessage};
pub use eventsource::{EventSource, SseEvent};
pub use netlog::{NetLog, NetLogEntry};
pub use robots::{Robots, BOT_TOKEN};
pub use charset::{decode_text, sniff_meta_charset};
