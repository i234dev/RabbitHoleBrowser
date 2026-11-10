//! An optional log of the HTTP requests made through a [`crate::Fetcher`]
//! (method, URL, type, status, size, timing and, for API/textual
//! responses, the body), including the requests still in flight. Used by
//! the agent interface to list network activity, read JSON responses and
//! wait for pending requests.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::fetch::{Destination, Request};

/// Bodies larger than this are not kept.
pub const MAX_KEPT_BODY: usize = 2 * 1024 * 1024;
/// Entries kept (oldest dropped first).
pub const MAX_ENTRIES: usize = 2000;

/// One request.
#[derive(Debug, Clone)]
pub struct NetLogEntry {
    pub id: u64,
    pub method: String,
    pub url: String,
    /// Request destination (document, script, style, image, font, empty).
    pub dest: Destination,
    /// Made by page scripts (`fetch()` / `XMLHttpRequest`).
    pub api: bool,
    pub status: Option<u16>,
    pub content_type: String,
    /// Response body size in bytes (decoded), when known.
    pub size: Option<u64>,
    pub started: Instant,
    pub duration: Option<Duration>,
    pub error: Option<String>,
    /// Kept body (API and textual responses up to [`MAX_KEPT_BODY`]).
    pub body: Option<Arc<Vec<u8>>>,
    /// Request body (form posts, JSON), truncated to 4 KiB.
    pub request_body: Option<String>,
}

impl NetLogEntry {
    pub fn finished(&self) -> bool {
        self.duration.is_some()
    }

    /// Short resource type name, as in devtools.
    pub fn kind(&self) -> &'static str {
        if self.api {
            return "fetch";
        }
        match self.dest {
            Destination::Document => "document",
            Destination::Script => "script",
            Destination::Style => "stylesheet",
            Destination::Image => "image",
            Destination::Font => "font",
            Destination::Empty => "other",
        }
    }
}

/// Shared request log (clone the `Arc` into fetchers).
#[derive(Debug, Default)]
pub struct NetLog {
    entries: Mutex<VecDeque<NetLogEntry>>,
    next: AtomicU64,
}

impl NetLog {
    pub fn new() -> Arc<NetLog> {
        Arc::new(NetLog::default())
    }

    /// Record the start of a request; returns its id.
    pub fn begin(&self, req: &Request) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let request_body = req.body.as_ref().map(|b| String::from_utf8_lossy(&b.data[..b.data.len().min(4096)]).into_owned());
        let e = NetLogEntry {
            id,
            method: req.method.to_ascii_uppercase(),
            url: req.url.without_fragment().to_string(),
            dest: req.dest,
            api: req.mode.is_some_and(|m| m != crate::RequestMode::Navigate),
            status: None,
            content_type: String::new(),
            size: None,
            started: Instant::now(),
            duration: None,
            error: None,
            body: None,
            request_body,
        };
        let mut q = self.entries.lock().unwrap();
        if q.len() >= MAX_ENTRIES {
            q.pop_front();
        }
        q.push_back(e);
        id
    }

    /// Record the end of a request.
    pub fn finish(&self, id: u64, status: u16, content_type: &str, size: Option<u64>, body: Option<&[u8]>) {
        let mut q = self.entries.lock().unwrap();
        if let Some(e) = q.iter_mut().rev().find(|e| e.id == id) {
            e.status = Some(status);
            e.content_type = content_type.to_string();
            e.size = size;
            e.duration = Some(e.started.elapsed());
            let textual = e.api
                || e.dest == Destination::Document
                || content_type.contains("json")
                || content_type.starts_with("text/")
                || content_type.contains("xml");
            let keep = textual && !matches!(e.dest, Destination::Image | Destination::Font | Destination::Script | Destination::Style);
            if let Some(b) = body.filter(|b| keep && b.len() <= MAX_KEPT_BODY) {
                e.body = Some(Arc::new(b.to_vec()));
            }
        }
    }

    /// Record a failed request.
    pub fn fail(&self, id: u64, error: &str) {
        let mut q = self.entries.lock().unwrap();
        if let Some(e) = q.iter_mut().rev().find(|e| e.id == id) {
            e.duration = Some(e.started.elapsed());
            e.error = Some(error.to_string());
        }
    }

    /// All entries, oldest first.
    pub fn entries(&self) -> Vec<NetLogEntry> {
        self.entries.lock().unwrap().iter().cloned().collect()
    }

    /// Entries with an id greater than `after`.
    pub fn since(&self, after: u64) -> Vec<NetLogEntry> {
        self.entries.lock().unwrap().iter().filter(|e| e.id > after).cloned().collect()
    }

    /// Requests not finished yet.
    pub fn in_flight(&self) -> Vec<NetLogEntry> {
        self.entries.lock().unwrap().iter().filter(|e| !e.finished()).cloned().collect()
    }

    pub fn get(&self, id: u64) -> Option<NetLogEntry> {
        self.entries.lock().unwrap().iter().find(|e| e.id == id).cloned()
    }

    /// Id of the most recent request (0 when none).
    pub fn last_id(&self) -> u64 {
        self.next.load(Ordering::Relaxed)
    }

    pub fn clear(&self) {
        self.entries.lock().unwrap().clear();
    }
}
