//! HTTP/2 client (RFC 9113): one connection per origin, multiplexed
//! streams, HPACK, flow control in both directions, SETTINGS, PING,
//! GOAWAY, RST_STREAM, CONTINUATION. Server push is disabled.
//!
//! A reader thread owns the receive side and demultiplexes frames into
//! per-stream buffers; requests are written from the calling threads
//! (header blocks under one lock so HPACK state and stream ids stay in
//! order).

pub mod frame;
pub mod hpack;
mod huffman_table;

use crate::transport::{ReadHalf, Stream, WriteHalf};
use frame::*;
use std::collections::{HashMap, VecDeque};
use std::io::{self, Read};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Our receive window per stream (what Chrome advertises).
const STREAM_WINDOW: u32 = 6 * 1024 * 1024;
/// Our connection receive window (65535 + Chrome's 15663105 increment).
const CONN_WINDOW: u32 = 15 * 1024 * 1024;
/// Our HPACK decoder table size.
const HEADER_TABLE: u32 = 65_536;
/// Our SETTINGS_MAX_HEADER_LIST_SIZE.
const MAX_HEADER_LIST: u32 = 256 * 1024;

#[derive(Debug)]
pub enum H2Error {
    /// The request was not processed (GOAWAY above its id, REFUSED_STREAM,
    /// connection lost before a response): safe to retry elsewhere.
    Retryable(String),
    /// Connection-level protocol failure: fall back to HTTP/1.1.
    Protocol(String),
    /// The server asked for HTTP/1.1 (HTTP_1_1_REQUIRED).
    Http11Required,
    /// The stream was reset by the server with this error code.
    Reset(u32),
    Timeout,
    Io(String),
}

impl std::fmt::Display for H2Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            H2Error::Retryable(m) => write!(f, "HTTP/2 request not processed: {m}"),
            H2Error::Protocol(m) => write!(f, "HTTP/2 protocol error: {m}"),
            H2Error::Http11Required => write!(f, "server requires HTTP/1.1"),
            H2Error::Reset(c) => write!(f, "HTTP/2 stream reset (code {c})"),
            H2Error::Timeout => write!(f, "request timed out"),
            H2Error::Io(m) => write!(f, "I/O error: {m}"),
        }
    }
}

/// Stream priority as an HTTP/2 weight (1..=256) plus RFC 9218 urgency.
#[derive(Debug, Clone, Copy)]
pub struct StreamPriority {
    pub weight: u16,
    pub urgency: u8,
    pub incremental: bool,
}

#[derive(Debug, Default)]
struct StreamState {
    /// Final response head: status + headers.
    head: Option<(u16, Vec<(String, String)>)>,
    data: VecDeque<Vec<u8>>,
    /// Peer finished sending (END_STREAM).
    remote_closed: bool,
    /// We finished sending.
    local_closed: bool,
    reset: Option<u32>,
    /// Bytes delivered to the reader but not yet credited back.
    unacked: u32,
    send_window: i64,
    /// Counted in `open` until both sides are closed.
    counted: bool,
}

#[derive(Debug)]
struct ConnState {
    streams: HashMap<u32, StreamState>,
    /// Streams counted against the peer's MAX_CONCURRENT_STREAMS.
    open: usize,
    max_concurrent: usize,
    peer_initial_window: i64,
    peer_max_frame: usize,
    conn_send_window: i64,
    /// GOAWAY received: last stream id the server will process.
    goaway: Option<u32>,
    /// Connection is dead (error text).
    closed: Option<String>,
    /// A connection-level protocol error (fall back to HTTP/1.1).
    protocol_error: bool,
    http11_required: bool,
    last_used: Instant,
    /// Highest stream id we have opened.
    last_opened: u32,
}

struct Writer {
    encoder: hpack::Encoder,
    next_id: u32,
}

struct Inner {
    w: WriteHalf,
    writer: Mutex<Writer>,
    st: Mutex<ConnState>,
    cv: Condvar,
    timeout: Duration,
}

impl Inner {
    fn send_frames(&self, bytes: &[u8]) -> io::Result<()> {
        self.w.send(bytes)
    }

    fn send_frame(&self, f: Frame) -> io::Result<()> {
        self.w.send(&f.to_bytes())
    }

    fn fail(&self, msg: String, protocol: bool) {
        let mut st = self.st.lock().unwrap();
        if st.closed.is_none() {
            st.closed = Some(msg);
        }
        st.protocol_error |= protocol;
        drop(st);
        self.cv.notify_all();
        self.w.shutdown();
    }

    /// Forget a stream; returns true when it was still open (the caller
    /// should send RST_STREAM CANCEL).
    fn remove_stream(st: &mut ConnState, id: u32) -> bool {
        match st.streams.remove(&id) {
            Some(s) if s.counted => {
                st.open = st.open.saturating_sub(1);
                s.reset.is_none()
            }
            _ => false,
        }
    }

    fn close_stream_if_done(st: &mut ConnState, id: u32) {
        if let Some(s) = st.streams.get_mut(&id) {
            if s.counted && ((s.remote_closed && s.local_closed) || s.reset.is_some()) {
                s.counted = false;
                st.open = st.open.saturating_sub(1);
            }
        }
    }
}

/// A shared HTTP/2 connection.
#[derive(Clone)]
pub struct H2Conn {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for H2Conn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("H2Conn")
    }
}

/// A response whose body is still arriving.
pub struct H2Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: H2Body,
}

impl H2Conn {
    /// Send the connection preface + SETTINGS and start the reader thread.
    pub fn handshake(stream: Stream, timeout: Duration) -> io::Result<H2Conn> {
        stream.set_timeouts(None, Some(timeout));
        let (r, w) = stream.split()?;
        // Idle connections are closed by the reader after this long
        // without traffic and no open streams.
        r.set_read_timeout(Some(Duration::from_secs(120)));
        let mut out = Vec::with_capacity(128);
        out.extend_from_slice(PREFACE);
        settings_frame(&[
            (SETTINGS_HEADER_TABLE_SIZE, HEADER_TABLE),
            (SETTINGS_ENABLE_PUSH, 0),
            (SETTINGS_INITIAL_WINDOW_SIZE, STREAM_WINDOW),
            (SETTINGS_MAX_HEADER_LIST_SIZE, MAX_HEADER_LIST),
        ])
        .encode(&mut out);
        window_update(0, CONN_WINDOW - 65_535).encode(&mut out);
        w.send(&out)?;
        let inner = Arc::new(Inner {
            w,
            writer: Mutex::new(Writer { encoder: hpack::Encoder::new(4096), next_id: 1 }),
            st: Mutex::new(ConnState {
                streams: HashMap::new(),
                open: 0,
                // Until the server's SETTINGS arrive (RFC: unlimited; be modest).
                max_concurrent: 100,
                peer_initial_window: 65_535,
                peer_max_frame: DEFAULT_MAX_FRAME,
                conn_send_window: 65_535,
                goaway: None,
                closed: None,
                protocol_error: false,
                http11_required: false,
                last_used: Instant::now(),
                last_opened: 0,
            }),
            cv: Condvar::new(),
            timeout,
        });
        let reader_inner = inner.clone();
        std::thread::Builder::new()
            .name("rh-h2-reader".into())
            .spawn(move || reader_loop(reader_inner, r))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        Ok(H2Conn { inner })
    }

    /// Usable for new requests (not closed, no GOAWAY, ids left).
    pub fn is_usable(&self) -> bool {
        let st = self.inner.st.lock().unwrap();
        st.closed.is_none() && st.goaway.is_none() && st.last_opened < 0x7fff_0000
    }

    /// Streams currently open.
    pub fn open_streams(&self) -> usize {
        self.inner.st.lock().unwrap().open
    }

    pub fn max_concurrent(&self) -> usize {
        self.inner.st.lock().unwrap().max_concurrent
    }

    pub fn idle_for(&self) -> Duration {
        let st = self.inner.st.lock().unwrap();
        if st.open > 0 {
            Duration::ZERO
        } else {
            st.last_used.elapsed()
        }
    }

    /// Close politely (GOAWAY) and drop the socket.
    pub fn close(&self) {
        let last = self.inner.st.lock().unwrap().last_opened;
        let _ = self.inner.send_frame(goaway(last, NO_ERROR, b""));
        self.inner.fail("closed".into(), false);
    }

    /// Issue a request. `headers` are regular (lower-cased) fields;
    /// connection-specific ones are dropped here.
    #[allow(clippy::too_many_arguments)]
    pub fn send_request(
        &self,
        method: &str,
        scheme: &str,
        authority: &str,
        path: &str,
        headers: &[(String, String)],
        body: Option<&[u8]>,
        priority: Option<StreamPriority>,
    ) -> Result<H2Response, H2Error> {
        let inner = &self.inner;
        let deadline = Instant::now() + inner.timeout;
        // 1. Wait for a free stream slot.
        {
            let mut st = inner.st.lock().unwrap();
            loop {
                if let Some(e) = &st.closed {
                    return Err(if st.http11_required { H2Error::Http11Required } else { H2Error::Retryable(e.clone()) });
                }
                if st.goaway.is_some() {
                    return Err(H2Error::Retryable("connection is going away".into()));
                }
                if st.open < st.max_concurrent.max(1) {
                    break;
                }
                let now = Instant::now();
                if now >= deadline {
                    return Err(H2Error::Timeout);
                }
                st = inner.cv.wait_timeout(st, deadline - now).unwrap().0;
            }
            st.open += 1;
            st.last_used = Instant::now();
        }
        // 2. Allocate the id and write the header block, in order.
        let mut fields: Vec<(String, String)> = Vec::with_capacity(headers.len() + 5);
        fields.push((":method".into(), method.to_string()));
        fields.push((":authority".into(), authority.to_string()));
        fields.push((":scheme".into(), scheme.to_string()));
        fields.push((":path".into(), if path.is_empty() { "/".into() } else { path.to_string() }));
        for (k, v) in headers {
            let k = k.to_ascii_lowercase();
            if matches!(k.as_str(), "connection" | "keep-alive" | "proxy-connection" | "transfer-encoding" | "upgrade" | "host" | "te") {
                continue;
            }
            fields.push((k, v.clone()));
        }
        let has_body = body.map(|b| !b.is_empty()).unwrap_or(false);
        let id;
        {
            let mut w = inner.writer.lock().unwrap();
            id = w.next_id;
            w.next_id += 2;
            let (max_frame, init_window) = {
                let mut st = inner.st.lock().unwrap();
                st.last_opened = id;
                let send_window = st.peer_initial_window;
                st.streams.insert(id, StreamState { send_window, counted: true, local_closed: !has_body, ..Default::default() });
                (st.peer_max_frame, send_window)
            };
            let _ = init_window;
            let block = w.encoder.encode(&fields);
            let mut out = Vec::with_capacity(block.len() + 32);
            let prio = priority.map(|p| (0u32, false, (p.weight.clamp(1, 256) - 1) as u8));
            header_frames(id, &block, !has_body, max_frame, prio, &mut out);
            if let Err(e) = inner.send_frames(&out) {
                drop(w);
                inner.fail(e.to_string(), false);
                return Err(H2Error::Retryable(e.to_string()));
            }
        }
        // 3. Request body, within the peer's flow-control windows.
        if let Some(data) = body.filter(|b| !b.is_empty()) {
            let mut off = 0;
            while off < data.len() {
                let n;
                {
                    let mut st = inner.st.lock().unwrap();
                    loop {
                        if let Some(e) = &st.closed {
                            return Err(H2Error::Io(e.clone()));
                        }
                        let s = st.streams.get(&id).ok_or_else(|| H2Error::Io("stream vanished".into()))?;
                        if let Some(code) = s.reset {
                            let e = reset_error(code, s.head.is_some());
                            Inner::remove_stream(&mut st, id);
                            return Err(e);
                        }
                        if s.head.is_some() && s.remote_closed {
                            // Server answered early (e.g. 413): stop sending.
                            break;
                        }
                        let avail = s.send_window.min(st.conn_send_window);
                        if avail > 0 {
                            break;
                        }
                        let now = Instant::now();
                        if now >= deadline {
                            drop(st);
                            self.cancel(id);
                            return Err(H2Error::Timeout);
                        }
                        st = inner.cv.wait_timeout(st, deadline - now).unwrap().0;
                    }
                    let s = st.streams.get(&id).unwrap();
                    if s.head.is_some() && s.remote_closed {
                        break;
                    }
                    let avail = s.send_window.min(st.conn_send_window) as usize;
                    n = avail.min(data.len() - off).min(st.peer_max_frame);
                    st.conn_send_window -= n as i64;
                    st.streams.get_mut(&id).unwrap().send_window -= n as i64;
                }
                let last = off + n == data.len();
                let f = Frame::new(DATA, if last { FLAG_END_STREAM } else { 0 }, id, data[off..off + n].to_vec());
                if let Err(e) = inner.send_frame(f) {
                    inner.fail(e.to_string(), false);
                    return Err(H2Error::Io(e.to_string()));
                }
                off += n;
            }
            let mut st = inner.st.lock().unwrap();
            if let Some(s) = st.streams.get_mut(&id) {
                s.local_closed = true;
            }
            Inner::close_stream_if_done(&mut st, id);
        }
        // 4. Wait for the response head.
        let mut st = inner.st.lock().unwrap();
        loop {
            let s = match st.streams.get(&id) {
                Some(s) => s,
                None => return Err(H2Error::Io("stream vanished".into())),
            };
            if let Some((status, headers)) = &s.head {
                let (status, headers) = (*status, headers.clone());
                drop(st);
                return Ok(H2Response { status, headers, body: H2Body { conn: self.clone(), id, done: false } });
            }
            if let Some(code) = s.reset {
                Inner::remove_stream(&mut st, id);
                return Err(reset_error(code, false));
            }
            if let Some(last) = st.goaway {
                if id > last {
                    Inner::remove_stream(&mut st, id);
                    return Err(H2Error::Retryable("GOAWAY before processing".into()));
                }
            }
            if let Some(e) = &st.closed {
                let err = if st.http11_required {
                    H2Error::Http11Required
                } else if st.protocol_error {
                    H2Error::Protocol(e.clone())
                } else {
                    H2Error::Retryable(e.clone())
                };
                Inner::remove_stream(&mut st, id);
                return Err(err);
            }
            let now = Instant::now();
            if now >= deadline {
                drop(st);
                self.cancel(id);
                return Err(H2Error::Timeout);
            }
            st = inner.cv.wait_timeout(st, deadline - now).unwrap().0;
        }
    }

    /// Abandon a stream (RST_STREAM CANCEL if still open).
    fn cancel(&self, id: u32) {
        let mut st = self.inner.st.lock().unwrap();
        let need_rst = Inner::remove_stream(&mut st, id);
        st.last_used = Instant::now();
        drop(st);
        self.inner.cv.notify_all();
        if need_rst {
            let _ = self.inner.send_frame(rst_stream(id, CANCEL));
        }
    }
}

fn reset_error(code: u32, had_head: bool) -> H2Error {
    match code {
        HTTP_1_1_REQUIRED => H2Error::Http11Required,
        REFUSED_STREAM if !had_head => H2Error::Retryable("stream refused".into()),
        c => H2Error::Reset(c),
    }
}

/// Body of an HTTP/2 response.
pub struct H2Body {
    conn: H2Conn,
    id: u32,
    done: bool,
}

impl Read for H2Body {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        let inner = &self.conn.inner;
        let mut st = inner.st.lock().unwrap();
        let deadline = Instant::now() + inner.timeout;
        loop {
            let Some(s) = st.streams.get_mut(&self.id) else {
                return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "stream closed"));
            };
            if let Some(chunk) = s.data.front_mut() {
                let n = buf.len().min(chunk.len());
                buf[..n].copy_from_slice(&chunk[..n]);
                if n == chunk.len() {
                    s.data.pop_front();
                } else {
                    chunk.drain(..n);
                }
                // Credit the window back once half of it was consumed.
                s.unacked += n as u32;
                let credit = if s.unacked >= STREAM_WINDOW / 2 && !s.remote_closed {
                    let c = s.unacked;
                    s.unacked = 0;
                    Some(c)
                } else {
                    None
                };
                drop(st);
                if let Some(c) = credit {
                    let _ = inner.send_frame(window_update(self.id, c));
                }
                return Ok(n);
            }
            if s.remote_closed {
                let need_rst = Inner::remove_stream(&mut st, self.id);
                st.last_used = Instant::now();
                self.done = true;
                drop(st);
                inner.cv.notify_all();
                if need_rst {
                    // Response complete while we were still uploading.
                    let _ = inner.send_frame(rst_stream(self.id, CANCEL));
                }
                return Ok(0);
            }
            if let Some(code) = s.reset {
                return Err(io::Error::new(io::ErrorKind::ConnectionReset, format!("HTTP/2 stream reset (code {code})")));
            }
            if let Some(e) = &st.closed {
                return Err(io::Error::new(io::ErrorKind::ConnectionAborted, e.clone()));
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "HTTP/2 body read timed out"));
            }
            st = inner.cv.wait_timeout(st, deadline - now).unwrap().0;
        }
    }
}

impl Drop for H2Body {
    fn drop(&mut self) {
        if !self.done {
            self.conn.cancel(self.id);
        }
    }
}

// --------------------------------------------------------------- reader

fn reader_loop(inner: Arc<Inner>, mut r: ReadHalf) {
    let mut decoder = hpack::Decoder::new(HEADER_TABLE as usize);
    decoder.max_list_size = MAX_HEADER_LIST as usize;
    let mut conn_unacked: u32 = 0;
    // Header block being assembled: (stream, end_stream, bytes).
    let mut pending_block: Option<(u32, bool, Vec<u8>)> = None;
    let mut last_peer_stream = 0u32;
    loop {
        let f = match Frame::read(&mut r, DEFAULT_MAX_FRAME) {
            Ok(f) => f,
            Err(FrameError::Io(e)) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => {
                let idle = inner.st.lock().unwrap().open == 0;
                if idle {
                    let last = inner.st.lock().unwrap().last_opened;
                    let _ = inner.send_frame(goaway(last, NO_ERROR, b"idle"));
                    inner.fail("idle timeout".into(), false);
                    return;
                }
                continue;
            }
            Err(FrameError::TooLarge(n)) => {
                let _ = inner.send_frame(goaway(last_peer_stream, FRAME_SIZE_ERROR, b""));
                inner.fail(format!("frame too large ({n})"), true);
                return;
            }
            Err(e) => {
                inner.fail(format!("connection lost: {e}"), false);
                return;
            }
        };
        // CONTINUATION must directly follow HEADERS of the same stream.
        if let Some((sid, _, _)) = &pending_block {
            if f.kind != CONTINUATION || f.stream != *sid {
                let _ = inner.send_frame(goaway(last_peer_stream, PROTOCOL_ERROR, b"expected CONTINUATION"));
                inner.fail("expected CONTINUATION".into(), true);
                return;
            }
        }
        let res: Result<(), (u32, String)> = (|| {
            match f.kind {
                DATA => {
                    if f.stream == 0 {
                        return Err((PROTOCOL_ERROR, "DATA on stream 0".into()));
                    }
                    let data = f.unpadded().map_err(|e| (PROTOCOL_ERROR, e.to_string()))?.to_vec();
                    conn_unacked += f.payload.len() as u32;
                    let mut st = inner.st.lock().unwrap();
                    let mut rst = None;
                    match st.streams.get_mut(&f.stream) {
                        Some(s) if s.reset.is_none() && !s.remote_closed => {
                            // Padding is flow-controlled but never delivered.
                            s.unacked += (f.payload.len() - data.len()) as u32;
                            if !data.is_empty() {
                                s.data.push_back(data);
                            }
                            if f.has(FLAG_END_STREAM) {
                                s.remote_closed = true;
                            }
                        }
                        Some(_) => {}
                        None => {
                            if f.stream > st.last_opened {
                                return Err((PROTOCOL_ERROR, "DATA on idle stream".into()));
                            }
                            rst = Some(f.stream);
                        }
                    }
                    Inner::close_stream_if_done(&mut st, f.stream);
                    drop(st);
                    inner.cv.notify_all();
                    if let Some(id) = rst {
                        let _ = inner.send_frame(rst_stream(id, STREAM_CLOSED));
                    }
                    if conn_unacked >= CONN_WINDOW / 4 {
                        let _ = inner.send_frame(window_update(0, conn_unacked));
                        conn_unacked = 0;
                    }
                }
                HEADERS => {
                    if f.stream == 0 {
                        return Err((PROTOCOL_ERROR, "HEADERS on stream 0".into()));
                    }
                    let frag = f.header_block_fragment().map_err(|e| (PROTOCOL_ERROR, e.to_string()))?.to_vec();
                    let end_stream = f.has(FLAG_END_STREAM);
                    if f.has(FLAG_END_HEADERS) {
                        handle_header_block(&inner, &mut decoder, f.stream, end_stream, &frag)?;
                    } else {
                        pending_block = Some((f.stream, end_stream, frag));
                    }
                }
                CONTINUATION => {
                    let Some((sid, end_stream, mut block)) = pending_block.take() else {
                        return Err((PROTOCOL_ERROR, "unexpected CONTINUATION".into()));
                    };
                    block.extend_from_slice(&f.payload);
                    if block.len() > MAX_HEADER_LIST as usize * 2 {
                        return Err((PROTOCOL_ERROR, "header block too large".into()));
                    }
                    if f.has(FLAG_END_HEADERS) {
                        handle_header_block(&inner, &mut decoder, sid, end_stream, &block)?;
                    } else {
                        pending_block = Some((sid, end_stream, block));
                    }
                }
                PRIORITY => {}
                RST_STREAM => {
                    if f.stream == 0 || f.payload.len() != 4 {
                        return Err((PROTOCOL_ERROR, "bad RST_STREAM".into()));
                    }
                    let code = be_u32(&f.payload);
                    let mut st = inner.st.lock().unwrap();
                    if let Some(s) = st.streams.get_mut(&f.stream) {
                        // After a complete response a reset only stops our
                        // upload; the buffered response stays readable.
                        if !s.remote_closed {
                            s.reset = Some(code);
                        }
                        s.local_closed = true;
                    }
                    Inner::close_stream_if_done(&mut st, f.stream);
                    drop(st);
                    inner.cv.notify_all();
                }
                SETTINGS => {
                    if f.stream != 0 {
                        return Err((PROTOCOL_ERROR, "SETTINGS on a stream".into()));
                    }
                    if f.has(FLAG_ACK) {
                        return Ok(());
                    }
                    let settings = parse_settings(&f.payload).map_err(|e| (FRAME_SIZE_ERROR, e.to_string()))?;
                    let mut table_size = None;
                    {
                        let mut st = inner.st.lock().unwrap();
                        for (id, v) in settings {
                            match id {
                                SETTINGS_HEADER_TABLE_SIZE => table_size = Some(v as usize),
                                SETTINGS_ENABLE_PUSH if v > 1 => return Err((PROTOCOL_ERROR, "bad ENABLE_PUSH".into())),
                                SETTINGS_MAX_CONCURRENT_STREAMS => st.max_concurrent = v as usize,
                                SETTINGS_INITIAL_WINDOW_SIZE => {
                                    if v > 0x7fff_ffff {
                                        return Err((FLOW_CONTROL_ERROR, "window too large".into()));
                                    }
                                    let delta = v as i64 - st.peer_initial_window;
                                    st.peer_initial_window = v as i64;
                                    for s in st.streams.values_mut() {
                                        s.send_window += delta;
                                    }
                                }
                                SETTINGS_MAX_FRAME_SIZE => {
                                    if !(16_384..=16_777_215).contains(&v) {
                                        return Err((PROTOCOL_ERROR, "bad MAX_FRAME_SIZE".into()));
                                    }
                                    st.peer_max_frame = v as usize;
                                }
                                _ => {}
                            }
                        }
                    }
                    if let Some(sz) = table_size {
                        inner.writer.lock().unwrap().encoder.set_peer_max_size(sz);
                    }
                    let _ = inner.send_frame(Frame::new(SETTINGS, FLAG_ACK, 0, Vec::new()));
                    inner.cv.notify_all();
                }
                PUSH_PROMISE => {
                    // We sent ENABLE_PUSH=0.
                    return Err((PROTOCOL_ERROR, "PUSH_PROMISE with push disabled".into()));
                }
                PING => {
                    if f.payload.len() != 8 {
                        return Err((FRAME_SIZE_ERROR, "bad PING".into()));
                    }
                    if !f.has(FLAG_ACK) {
                        let _ = inner.send_frame(Frame::new(PING, FLAG_ACK, 0, f.payload.clone()));
                    }
                }
                GOAWAY => {
                    if f.payload.len() < 8 {
                        return Err((FRAME_SIZE_ERROR, "bad GOAWAY".into()));
                    }
                    let last = be_u32(&f.payload) & 0x7fff_ffff;
                    let code = be_u32(&f.payload[4..]);
                    let mut st = inner.st.lock().unwrap();
                    st.goaway = Some(st.goaway.map_or(last, |g| g.min(last)));
                    if code == HTTP_1_1_REQUIRED {
                        st.http11_required = true;
                    }
                    if code != NO_ERROR && code != HTTP_1_1_REQUIRED {
                        st.protocol_error |= code == PROTOCOL_ERROR || code == COMPRESSION_ERROR;
                    }
                    let ids: Vec<u32> = st.streams.keys().copied().filter(|id| *id > last).collect();
                    for id in ids {
                        if let Some(s) = st.streams.get_mut(&id) {
                            s.reset = Some(REFUSED_STREAM);
                        }
                        Inner::close_stream_if_done(&mut st, id);
                    }
                    let done = st.open == 0;
                    drop(st);
                    inner.cv.notify_all();
                    if done {
                        inner.fail("GOAWAY".into(), false);
                        return Err((u32::MAX, "GOAWAY".into()));
                    }
                }
                WINDOW_UPDATE => {
                    if f.payload.len() != 4 {
                        return Err((FRAME_SIZE_ERROR, "bad WINDOW_UPDATE".into()));
                    }
                    let inc = (be_u32(&f.payload) & 0x7fff_ffff) as i64;
                    let mut st = inner.st.lock().unwrap();
                    if f.stream == 0 {
                        if inc == 0 {
                            return Err((PROTOCOL_ERROR, "zero WINDOW_UPDATE".into()));
                        }
                        st.conn_send_window += inc;
                        if st.conn_send_window > 0x7fff_ffff {
                            return Err((FLOW_CONTROL_ERROR, "connection window overflow".into()));
                        }
                    } else if let Some(s) = st.streams.get_mut(&f.stream) {
                        s.send_window += inc;
                    }
                    drop(st);
                    inner.cv.notify_all();
                }
                _ => {} // Unknown frame types are ignored.
            }
            if f.stream > last_peer_stream && f.stream % 2 == 1 {
                last_peer_stream = f.stream;
            }
            Ok(())
        })();
        if let Err((code, msg)) = res {
            if code != u32::MAX {
                let _ = inner.send_frame(goaway(last_peer_stream, code, msg.as_bytes()));
                inner.fail(msg, true);
            }
            return;
        }
    }
}

fn handle_header_block(inner: &Inner, decoder: &mut hpack::Decoder, id: u32, end_stream: bool, block: &[u8]) -> Result<(), (u32, String)> {
    // Always decode, even for streams we already dropped, to keep the
    // HPACK table in sync.
    let fields = decoder.decode(block).map_err(|e| (COMPRESSION_ERROR, e.to_string()))?;
    let mut st = inner.st.lock().unwrap();
    if id % 2 == 0 {
        return Err((PROTOCOL_ERROR, "HEADERS on a server-initiated stream".into()));
    }
    let Some(s) = st.streams.get_mut(&id) else {
        return Ok(());
    };
    if s.head.is_none() {
        let status = fields.iter().find(|(k, _)| k == ":status").and_then(|(_, v)| v.parse::<u16>().ok());
        let Some(status) = status else {
            drop(st);
            let _ = inner.send_frame(rst_stream(id, PROTOCOL_ERROR));
            let mut st = inner.st.lock().unwrap();
            if let Some(s) = st.streams.get_mut(&id) {
                s.reset = Some(PROTOCOL_ERROR);
            }
            Inner::close_stream_if_done(&mut st, id);
            inner.cv.notify_all();
            return Ok(());
        };
        if (100..200).contains(&status) {
            // Interim response (100 Continue, 103 Early Hints): skip.
            return Ok(());
        }
        let headers: Vec<(String, String)> = fields.into_iter().filter(|(k, _)| !k.starts_with(':')).collect();
        s.head = Some((status, headers));
    }
    // Otherwise: trailers, ignored.
    if end_stream {
        s.remote_closed = true;
    }
    Inner::close_stream_if_done(&mut st, id);
    drop(st);
    inner.cv.notify_all();
    Ok(())
}
