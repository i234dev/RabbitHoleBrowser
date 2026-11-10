//! EventSource (server-sent events, HTML 9.2) client with a poll-based
//! API: connects on a background thread, parses the `text/event-stream`
//! format incrementally and reconnects (with `Last-Event-ID`) after the
//! retry delay when the stream drops.

use crate::cache::CacheMode;
use crate::cors::{CredentialsMode, RequestMode};
use crate::fetch::{Destination, Fetcher, Request};
use crate::url::Url;
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

pub const CONNECTING: u8 = 0;
pub const OPEN: u8 = 1;
pub const CLOSED: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SseEvent {
    Open,
    /// A dispatched event (`type` is "message" unless `event:` set it).
    Message { event: String, data: String, last_event_id: String, origin: String },
    /// Connection problem; `reconnecting` tells whether it retries.
    Error { reconnecting: bool, message: String },
}

type Waker = Arc<dyn Fn() + Send + Sync>;

struct Shared {
    state: AtomicU8,
    closed: AtomicBool,
    waker: Option<Waker>,
}

pub struct EventSource {
    shared: Arc<Shared>,
    events: Receiver<SseEvent>,
    url: Url,
    with_credentials: bool,
}

/// Incremental `text/event-stream` parser.
#[derive(Debug, Default)]
pub struct SseParser {
    line: Vec<u8>,
    last_was_cr: bool,
    data: String,
    event: String,
    pub last_event_id: String,
    /// New reconnection time requested by `retry:`.
    pub retry: Option<u64>,
    bom_checked: bool,
}

/// One dispatched event from the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEvent {
    pub event: String,
    pub data: String,
    pub id: String,
}

impl SseParser {
    pub fn new() -> SseParser {
        SseParser::default()
    }

    /// Feed bytes; returns the events completed by them.
    pub fn feed(&mut self, mut bytes: &[u8]) -> Vec<ParsedEvent> {
        let mut out = Vec::new();
        if !self.bom_checked && !bytes.is_empty() {
            if self.line.is_empty() && bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
                bytes = &bytes[3..];
            }
            self.bom_checked = true;
        }
        for &b in bytes {
            if b == b'\n' && self.last_was_cr {
                self.last_was_cr = false;
                continue;
            }
            self.last_was_cr = b == b'\r';
            if b == b'\n' || b == b'\r' {
                let line = String::from_utf8_lossy(&std::mem::take(&mut self.line)).into_owned();
                if let Some(ev) = self.process_line(&line) {
                    out.push(ev);
                }
            } else {
                self.line.push(b);
            }
        }
        out
    }

    fn process_line(&mut self, line: &str) -> Option<ParsedEvent> {
        if line.is_empty() {
            // Dispatch.
            if self.data.is_empty() {
                self.event.clear();
                return None;
            }
            let mut data = std::mem::take(&mut self.data);
            if data.ends_with('\n') {
                data.pop();
            }
            let event = std::mem::take(&mut self.event);
            return Some(ParsedEvent { event: if event.is_empty() { "message".into() } else { event }, data, id: self.last_event_id.clone() });
        }
        if line.starts_with(':') {
            return None; // comment
        }
        let (field, value) = match line.find(':') {
            Some(i) => {
                let v = &line[i + 1..];
                (&line[..i], v.strip_prefix(' ').unwrap_or(v))
            }
            None => (line, ""),
        };
        match field {
            "event" => self.event = value.to_string(),
            "data" => {
                self.data.push_str(value);
                self.data.push('\n');
            }
            "id" => {
                if !value.contains('\0') {
                    self.last_event_id = value.to_string();
                }
            }
            "retry" => {
                if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
                    self.retry = value.parse().ok();
                }
            }
            _ => {}
        }
        None
    }
}

impl EventSource {
    /// Start an EventSource for `url`. `initiator` is the document URL
    /// (origin for CORS, referrer). Returns immediately; events arrive
    /// via `poll`. `waker` is called when an event is queued.
    pub fn connect(fetcher: &Fetcher, url: &Url, with_credentials: bool, initiator: Option<&Url>, waker: Option<Waker>) -> Result<EventSource, String> {
        if !url.is_http() {
            return Err(format!("SyntaxError: unsupported EventSource URL scheme '{}'", url.scheme));
        }
        let shared = Arc::new(Shared { state: AtomicU8::new(CONNECTING), closed: AtomicBool::new(false), waker });
        let (tx, events) = channel();
        let mut f = fetcher.clone();
        // Streams can be quiet for a long time between events.
        f.timeout = f.timeout.max(Duration::from_secs(120));
        f.max_body = usize::MAX;
        let sh = shared.clone();
        let u = url.clone();
        let init = initiator.cloned();
        std::thread::Builder::new()
            .name("rh-eventsource".into())
            .spawn(move || run(f, u, with_credentials, init, sh, tx))
            .map_err(|e| e.to_string())?;
        Ok(EventSource { shared, events, url: url.clone(), with_credentials })
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn with_credentials(&self) -> bool {
        self.with_credentials
    }

    pub fn ready_state(&self) -> u8 {
        self.shared.state.load(Ordering::SeqCst)
    }

    /// Drain pending events (never blocks).
    pub fn poll(&self) -> Vec<SseEvent> {
        if self.shared.closed.load(Ordering::SeqCst) {
            // Events after close() are not delivered.
            let _ = self.events.try_iter().count();
            return Vec::new();
        }
        self.events.try_iter().collect()
    }

    pub fn next_event(&self, timeout: Duration) -> Option<SseEvent> {
        self.events.recv_timeout(timeout).ok()
    }

    pub fn close(&self) {
        self.shared.closed.store(true, Ordering::SeqCst);
        self.shared.state.store(CLOSED, Ordering::SeqCst);
    }
}

impl Drop for EventSource {
    fn drop(&mut self) {
        self.close();
    }
}

fn emit(sh: &Shared, tx: &Sender<SseEvent>, ev: SseEvent) {
    if sh.closed.load(Ordering::SeqCst) {
        return;
    }
    let _ = tx.send(ev);
    if let Some(w) = &sh.waker {
        w();
    }
}

fn run(fetcher: Fetcher, url: Url, with_credentials: bool, initiator: Option<Url>, sh: Arc<Shared>, tx: Sender<SseEvent>) {
    let mut parser = SseParser::new();
    let mut retry_ms: u64 = 3000;
    let origin = url.origin();
    let mut current = url.clone();
    loop {
        if sh.closed.load(Ordering::SeqCst) {
            return;
        }
        let mut req = Request::get(current.clone())
            .with_dest(Destination::Empty)
            .with_mode(RequestMode::Cors, if with_credentials { CredentialsMode::Include } else { CredentialsMode::SameOrigin })
            .with_cache_mode(CacheMode::NoStore)
            .with_header("Accept", "text/event-stream")
            .with_header("Cache-Control", "no-cache");
        req.referer = initiator.clone();
        if !parser.last_event_id.is_empty() {
            req = req.with_header("Last-Event-ID", &parser.last_event_id);
        }
        let reconnect = match fetcher.request_stream(&req) {
            Ok(resp) => {
                if resp.status != 200 {
                    sh.state.store(CLOSED, Ordering::SeqCst);
                    emit(&sh, &tx, SseEvent::Error { reconnecting: false, message: format!("EventSource's response has a status ({}) that is not 200", resp.status) });
                    return;
                }
                if resp.content_type != "text/event-stream" {
                    sh.state.store(CLOSED, Ordering::SeqCst);
                    emit(&sh, &tx, SseEvent::Error { reconnecting: false, message: format!("EventSource's response has a MIME type (\"{}\") that is not \"text/event-stream\"", resp.content_type) });
                    return;
                }
                current = resp.url.clone();
                if sh.closed.load(Ordering::SeqCst) {
                    return;
                }
                sh.state.store(OPEN, Ordering::SeqCst);
                emit(&sh, &tx, SseEvent::Open);
                let mut reader = resp.reader;
                let mut buf = [0u8; 16 * 1024];
                loop {
                    if sh.closed.load(Ordering::SeqCst) {
                        return;
                    }
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            for ev in parser.feed(&buf[..n]) {
                                emit(&sh, &tx, SseEvent::Message { event: ev.event, data: ev.data, last_event_id: ev.id, origin: origin.clone() });
                            }
                            if let Some(r) = parser.retry.take() {
                                retry_ms = r;
                            }
                        }
                    }
                }
                true
            }
            Err(crate::fetch::FetchError::Cors(m)) => {
                sh.state.store(CLOSED, Ordering::SeqCst);
                emit(&sh, &tx, SseEvent::Error { reconnecting: false, message: m });
                return;
            }
            Err(_) => true,
        };
        if !reconnect || sh.closed.load(Ordering::SeqCst) {
            return;
        }
        sh.state.store(CONNECTING, Ordering::SeqCst);
        emit(&sh, &tx, SseEvent::Error { reconnecting: true, message: "connection lost; reconnecting".into() });
        // Sleep in small steps so close() stops the thread promptly.
        let mut waited = 0;
        while waited < retry_ms {
            if sh.closed.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50.min(retry_ms - waited)));
            waited += 50;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser() {
        let mut p = SseParser::new();
        let evs = p.feed(b"\xef\xbb\xbfdata: hello\r\ndata:world\n\n: comment\nevent: ping\nid: 7\ndata\n\nretry: 1500\n");
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0], ParsedEvent { event: "message".into(), data: "hello\nworld".into(), id: String::new() });
        assert_eq!(evs[1], ParsedEvent { event: "ping".into(), data: String::new(), id: "7".into() });
        assert_eq!(p.retry, Some(1500));
        // Split across feeds and CR line endings.
        let mut p = SseParser::new();
        assert!(p.feed(b"data: a").is_empty());
        let evs = p.feed(b"bc\r\r");
        assert_eq!(evs[0].data, "abc");
        // No data -> no event.
        let mut p = SseParser::new();
        assert!(p.feed(b"event: x\n\n").is_empty());
        let evs = p.feed(b"data: y\n\n");
        assert_eq!(evs[0].event, "message");
    }
}
