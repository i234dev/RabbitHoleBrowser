//! Connection management: per-origin HTTP/2 connections (one shared,
//! multiplexed connection per origin, discovered via ALPN), pooled
//! HTTP/1.1 keep-alive connections with a bounded number per host, a
//! priority-ordered wait queue for those slots, and fallback to HTTP/1.1
//! on any HTTP/2 protocol failure.

use crate::h1::{self, H1Body, H1Conn};
use crate::h2::{H2Conn, H2Error, StreamPriority};
use crate::transport::{self, Alpn, Stream};
use crate::url::Url;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{self, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Request priority (Chrome's net priorities). Higher is more urgent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    Lowest = 0,
    Low = 1,
    Medium = 2,
    High = 3,
    Highest = 4,
}

impl Priority {
    pub(crate) fn h2(self, incremental: bool) -> StreamPriority {
        let (weight, urgency) = match self {
            Priority::Highest => (256, 0),
            Priority::High => (220, 1),
            Priority::Medium => (183, 2),
            Priority::Low => (147, 3),
            Priority::Lowest => (110, 4),
        };
        StreamPriority { weight, urgency, incremental }
    }
}

/// One request as it goes on the wire.
pub struct WireRequest<'a> {
    pub method: &'a str,
    pub url: &'a Url,
    /// Headers in send order (no Host / Content-Length / Connection).
    pub headers: &'a [(String, String)],
    pub body: Option<&'a [u8]>,
    pub priority: Priority,
    /// Image-like resources are delivered incrementally (RFC 9218).
    pub incremental: bool,
}

pub struct WireResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Box<dyn Read + Send>,
    /// "h2" or "http/1.1".
    pub protocol: &'static str,
}

#[derive(Debug)]
pub enum ClientError {
    Timeout,
    Io(String),
}

impl From<io::Error> for ClientError {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::TimedOut || e.kind() == io::ErrorKind::WouldBlock {
            ClientError::Timeout
        } else {
            ClientError::Io(e.to_string())
        }
    }
}

/// Counters for diagnostics and benchmarks.
#[derive(Debug, Default)]
pub struct Stats {
    pub h2_requests: AtomicU64,
    pub h1_requests: AtomicU64,
    pub connections: AtomicU64,
    pub h2_connections: AtomicU64,
    pub h2_fallbacks: AtomicU64,
    pub retries: AtomicU64,
    pub cache_hits: AtomicU64,
    pub cache_revalidated: AtomicU64,
    pub cache_misses: AtomicU64,
}

impl Stats {
    pub fn snapshot(&self) -> String {
        let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
        format!(
            "h2 requests {}, h1 requests {}, connections {} ({} h2), h2 fallbacks {}, retries {}, cache hits {}, revalidated {}, misses {}",
            g(&self.h2_requests),
            g(&self.h1_requests),
            g(&self.connections),
            g(&self.h2_connections),
            g(&self.h2_fallbacks),
            g(&self.retries),
            g(&self.cache_hits),
            g(&self.cache_revalidated),
            g(&self.cache_misses)
        )
    }
}

#[derive(Debug, Default)]
struct OriginInfo {
    /// Never offer h2 again (protocol failure / HTTP_1_1_REQUIRED).
    no_h2: bool,
    /// The server answered with HTTP/1.1 last time.
    is_h1: bool,
    /// A connection whose protocol is not yet known is being set up.
    connecting: bool,
}

// -------------------------------------------------------------- limiter

#[derive(Default)]
struct HostSlots {
    active: usize,
    /// (Reverse priority, sequence): smallest = next to run.
    waiters: BTreeSet<(u8, u64)>,
}

/// Bounded HTTP/1.1 connections per host with a priority wait queue.
struct Limiter {
    st: Mutex<(HashMap<String, HostSlots>, u64)>,
    cv: Condvar,
    per_host: usize,
}

struct SlotGuard {
    limiter: Arc<Client>,
    key: String,
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        let l = &self.limiter.limiter;
        let mut st = l.st.lock().unwrap();
        if let Some(h) = st.0.get_mut(&self.key) {
            h.active = h.active.saturating_sub(1);
            if h.active == 0 && h.waiters.is_empty() {
                st.0.remove(&self.key);
            }
        }
        drop(st);
        l.cv.notify_all();
    }
}

// --------------------------------------------------------------- client

pub struct Client {
    h2: Mutex<HashMap<String, H2Conn>>,
    idle: Mutex<HashMap<String, Vec<H1Conn>>>,
    origins: Mutex<HashMap<String, OriginInfo>>,
    origins_cv: Condvar,
    limiter: Limiter,
    /// Plain-http origins spoken to with HTTP/2 prior knowledge (tests).
    pub(crate) h2c: Mutex<HashSet<String>>,
    pub stats: Stats,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Client")
    }
}

fn origin_key(url: &Url) -> String {
    format!("{}://{}:{}", url.scheme, url.host.to_ascii_lowercase(), url.port.or(url.default_port()).unwrap_or(0))
}

const IDLE_LIMIT: Duration = Duration::from_secs(90);
const MAX_IDLE_PER_HOST: usize = 6;

impl Client {
    pub fn new(per_host: usize) -> Client {
        Client {
            h2: Mutex::new(HashMap::new()),
            idle: Mutex::new(HashMap::new()),
            origins: Mutex::new(HashMap::new()),
            origins_cv: Condvar::new(),
            limiter: Limiter { st: Mutex::new((HashMap::new(), 0)), cv: Condvar::new(), per_host: per_host.max(1) },
            h2c: Mutex::new(HashSet::new()),
            stats: Stats::default(),
        }
    }

    /// Whether `url`'s origin currently has a usable HTTP/2 connection
    /// (so many requests can be in flight at once).
    pub fn has_h2(&self, url: &Url) -> bool {
        self.h2.lock().unwrap().get(&origin_key(url)).map(|c| c.is_usable()).unwrap_or(false)
    }

    /// Drop every pooled connection.
    pub fn clear(&self) {
        for (_, c) in self.h2.lock().unwrap().drain() {
            c.close();
        }
        self.idle.lock().unwrap().clear();
        self.origins.lock().unwrap().clear();
    }

    fn acquire_slot(self: &Arc<Self>, key: &str, prio: Priority, timeout: Duration) -> Result<SlotGuard, ClientError> {
        let l = &self.limiter;
        let deadline = Instant::now() + timeout;
        let mut st = l.st.lock().unwrap();
        st.1 += 1;
        let ticket = (4 - prio as u8, st.1);
        st.0.entry(key.to_string()).or_default().waiters.insert(ticket);
        loop {
            let h = st.0.get_mut(key).unwrap();
            if h.active < l.per_host && h.waiters.iter().next() == Some(&ticket) {
                h.waiters.remove(&ticket);
                h.active += 1;
                drop(st);
                // Others may be eligible too (more free slots).
                l.cv.notify_all();
                return Ok(SlotGuard { limiter: self.clone(), key: key.to_string() });
            }
            let now = Instant::now();
            if now >= deadline {
                h.waiters.remove(&ticket);
                drop(st);
                l.cv.notify_all();
                return Err(ClientError::Timeout);
            }
            st = l.cv.wait_timeout(st, deadline - now).unwrap().0;
        }
    }

    fn take_idle(&self, key: &str) -> Option<H1Conn> {
        let mut idle = self.idle.lock().unwrap();
        let list = idle.get_mut(key)?;
        while let Some(c) = list.pop() {
            if c.idle_since.elapsed() < IDLE_LIMIT {
                return Some(c);
            }
        }
        None
    }

    fn put_idle(&self, key: &str, conn: H1Conn) {
        let mut idle = self.idle.lock().unwrap();
        let list = idle.entry(key.to_string()).or_default();
        list.retain(|c| c.idle_since.elapsed() < IDLE_LIMIT);
        if list.len() < MAX_IDLE_PER_HOST {
            list.push(conn);
        }
    }

    fn live_h2(&self, key: &str) -> Option<H2Conn> {
        let mut map = self.h2.lock().unwrap();
        match map.get(key) {
            Some(c) if c.is_usable() => Some(c.clone()),
            Some(_) => {
                map.remove(key);
                None
            }
            None => None,
        }
    }

    fn mark_no_h2(&self, key: &str) {
        self.stats.h2_fallbacks.fetch_add(1, Ordering::Relaxed);
        if let Some(c) = self.h2.lock().unwrap().remove(key) {
            c.close();
        }
        let mut o = self.origins.lock().unwrap();
        let info = o.entry(key.to_string()).or_default();
        info.no_h2 = true;
        info.is_h1 = true;
        drop(o);
        self.origins_cv.notify_all();
    }

    fn finish_discovery(&self, key: &str, is_h1: bool) {
        let mut o = self.origins.lock().unwrap();
        let info = o.entry(key.to_string()).or_default();
        info.connecting = false;
        if is_h1 {
            info.is_h1 = true;
        }
        drop(o);
        self.origins_cv.notify_all();
    }

    /// Send a request, choosing HTTP/2 or HTTP/1.1, with retries on
    /// stale connections and fallback after HTTP/2 failures.
    pub fn send(self: &Arc<Self>, req: &WireRequest, timeout: Duration) -> Result<WireResponse, ClientError> {
        let key = origin_key(req.url);
        let h2c = req.url.scheme == "http" && self.h2c.lock().unwrap().contains(&key);
        let h2_possible = req.url.scheme == "https" || h2c;
        let deadline = Instant::now() + timeout;
        let mut last_err = ClientError::Io("no attempt".into());
        for _attempt in 0..5 {
            if Instant::now() >= deadline {
                return Err(ClientError::Timeout);
            }
            let mut discovering = false;
            if h2_possible {
                if let Some(conn) = self.live_h2(&key) {
                    match self.send_h2(&conn, req) {
                        Ok(r) => return Ok(r),
                        Err(H2Error::Retryable(m)) => {
                            self.stats.retries.fetch_add(1, Ordering::Relaxed);
                            last_err = ClientError::Io(m);
                            continue;
                        }
                        Err(H2Error::Protocol(m)) => {
                            self.mark_no_h2(&key);
                            last_err = ClientError::Io(m);
                            continue;
                        }
                        Err(H2Error::Http11Required) => {
                            self.mark_no_h2(&key);
                            continue;
                        }
                        Err(H2Error::Timeout) => return Err(ClientError::Timeout),
                        Err(e) => return Err(ClientError::Io(e.to_string())),
                    }
                }
                // No HTTP/2 connection: if the origin's protocol is being
                // discovered by another request, wait for it (so all
                // requests share the one h2 connection).
                let mut o = self.origins.lock().unwrap();
                loop {
                    let info = o.entry(key.clone()).or_default();
                    if info.no_h2 || info.is_h1 {
                        break;
                    }
                    if !info.connecting {
                        info.connecting = true;
                        discovering = true;
                        break;
                    }
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(ClientError::Timeout);
                    }
                    o = self.origins_cv.wait_timeout(o, deadline - now).unwrap().0;
                    if self.h2.lock().unwrap().get(&key).map(|c| c.is_usable()).unwrap_or(false) {
                        break;
                    }
                }
                drop(o);
                if !discovering && self.live_h2(&key).is_some() {
                    continue;
                }
            }
            match self.send_h1_or_discover(req, &key, h2c, discovering, deadline) {
                Ok(Some(r)) => return Ok(r),
                Ok(None) => continue, // got an h2 connection, or stale reuse: retry
                Err(e) => return Err(e),
            }
        }
        Err(last_err)
    }

    fn send_h2(&self, conn: &H2Conn, req: &WireRequest) -> Result<WireResponse, H2Error> {
        let authority = req.url.host_with_port();
        let mut path = if req.url.path.is_empty() { "/".to_string() } else { req.url.path.clone() };
        if let Some(q) = &req.url.query {
            path.push('?');
            path.push_str(q);
        }
        let prio = req.priority.h2(req.incremental);
        let mut headers = req.headers.to_vec();
        headers.push(("priority".into(), format!("u={}{}", prio.urgency, if prio.incremental { ", i" } else { "" })));
        if let Some(b) = req.body {
            headers.push(("content-length".into(), b.len().to_string()));
        }
        let r = conn.send_request(req.method, &req.url.scheme, &authority, &path, &headers, req.body, Some(prio))?;
        self.stats.h2_requests.fetch_add(1, Ordering::Relaxed);
        Ok(WireResponse { status: r.status, headers: r.headers, body: Box::new(r.body), protocol: "h2" })
    }

    /// HTTP/1.1 path. When `discovering`, the new connection's ALPN result
    /// decides the origin's protocol; an h2 result is registered and
    /// `Ok(None)` asks the caller to retry over it.
    fn send_h1_or_discover(self: &Arc<Self>, req: &WireRequest, key: &str, h2c: bool, discovering: bool, deadline: Instant) -> Result<Option<WireResponse>, ClientError> {
        let timeout = deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(1));
        // Discovery connections do not wait for an HTTP/1.1 slot.
        let slot = if discovering {
            None
        } else {
            Some(self.acquire_slot(key, req.priority, timeout)?)
        };
        let reused = if discovering { None } else { self.take_idle(key) };
        let mut conn = match reused {
            Some(c) => c,
            None => {
                let no_h2 = self.origins.lock().unwrap().get(key).map(|o| o.no_h2).unwrap_or(false);
                let port = req.url.port.or(req.url.default_port()).unwrap_or(80);
                let alpn = if no_h2 { Alpn::Http1 } else { Alpn::H2OrHttp1 };
                let connected = transport::connect(&req.url.scheme, &req.url.host, port, alpn, timeout);
                let connected = match connected {
                    Ok(c) => c,
                    Err(e) => {
                        if discovering {
                            self.finish_discovery(key, false);
                        }
                        return Err(e.into());
                    }
                };
                self.stats.connections.fetch_add(1, Ordering::Relaxed);
                if (connected.h2 || (h2c && !no_h2)) && !connected.via_proxy {
                    let timeout_cfg = timeout.max(Duration::from_secs(1));
                    match H2Conn::handshake(connected.stream, timeout_cfg) {
                        Ok(h2) => {
                            self.stats.h2_connections.fetch_add(1, Ordering::Relaxed);
                            let old = self.h2.lock().unwrap().insert(key.to_string(), h2);
                            if let Some(old) = old {
                                old.close();
                            }
                            if discovering {
                                self.finish_discovery(key, false);
                            }
                            drop(slot);
                            return Ok(None);
                        }
                        Err(e) => {
                            if discovering {
                                self.finish_discovery(key, false);
                            }
                            self.mark_no_h2(key);
                            return Err(e.into());
                        }
                    }
                }
                if discovering {
                    self.finish_discovery(key, true);
                }
                H1Conn::new(connected.stream, connected.via_proxy)
            }
        };
        let slot = match slot {
            Some(s) => s,
            // The discovering request turned out to be HTTP/1.1: it holds
            // a slot like any other (may briefly exceed the limit by one).
            None => SlotGuard { limiter: self.clone(), key: key.to_string() }.claim(),
        };
        let was_reused = conn.reused;
        conn.io.get_ref().set_timeouts(Some(timeout), Some(timeout));
        let proxy_auth = if conn.via_proxy { transport::proxy_for(&req.url.scheme, &req.url.host).and_then(|p| p.auth) } else { None };
        let head = h1::write_request(&mut conn, req.method, req.url, req.headers, req.body, proxy_auth.as_deref()).and_then(|_| h1::read_head(&mut conn.io));
        let head = match head {
            Ok(Some(h)) => h,
            Ok(None) | Err(_) if was_reused => {
                // Stale keep-alive connection: retry on a fresh one.
                self.stats.retries.fetch_add(1, Ordering::Relaxed);
                return Ok(None);
            }
            Ok(None) => return Err(ClientError::Io("server closed the connection without a response".into())),
            Err(e) => return Err(e.into()),
        };
        self.stats.h1_requests.fetch_add(1, Ordering::Relaxed);
        let framing = h1::framing(req.method, &head);
        let reusable = h1::keep_alive(&head, framing);
        let me = self.clone();
        let k = key.to_string();
        let body = H1Body::new(
            conn,
            framing,
            reusable,
            Box::new(move |c| {
                if let Some(c) = c {
                    me.put_idle(&k, c);
                }
                drop(slot);
            }),
        );
        Ok(Some(WireResponse { status: head.status, headers: head.headers, body: Box::new(body), protocol: "http/1.1" }))
    }
}

impl SlotGuard {
    /// Count this guard as an active slot (for a connection opened
    /// outside the queue).
    fn claim(self) -> SlotGuard {
        let l = &self.limiter.limiter;
        l.st.lock().unwrap().0.entry(self.key.clone()).or_default().active += 1;
        self
    }
}

/// Plain-TCP stream (for tests that speak h2c).
#[allow(dead_code)]
pub(crate) fn plain(stream: std::net::TcpStream) -> Stream {
    Stream::Plain(stream)
}
