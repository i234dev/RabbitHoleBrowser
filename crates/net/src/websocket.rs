//! WebSocket client (RFC 6455) with permessage-deflate (RFC 7692) and a
//! non-blocking, poll-based API for the DOM bindings.
//!
//! ```ignore
//! let ws = WebSocket::connect(&fetcher, &url, &[], Some(&page_url), None)?;
//! // later, from the event loop:
//! for ev in ws.poll() { /* WsEvent::Open / Message / Error / Close */ }
//! ws.send_text("hi");
//! ws.close(Some(1000), "");
//! ```
//!
//! One thread per socket reads frames (and connects first); a second one
//! writes queued frames, so neither `send` nor `poll` ever blocks.

use crate::cookies::CookieContext;
use crate::fetch::{is_tracker_host, Fetcher};
use crate::h1::{self, H1Conn};
use crate::transport::{self, base64_encode, Alpn, ReadHalf, WriteHalf};
use crate::url::Url;
use std::io::{self, Read};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const CONNECTING: u8 = 0;
pub const OPEN: u8 = 1;
pub const CLOSING: u8 = 2;
pub const CLOSED: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsMessage {
    Text(String),
    Binary(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsEvent {
    /// Handshake done: negotiated subprotocol and extensions.
    Open { protocol: String, extensions: String },
    Message(WsMessage),
    /// Failure (an `error` event; a `Close` follows).
    Error(String),
    Close { code: u16, reason: String, was_clean: bool },
}

enum Cmd {
    Frame(u8, Vec<u8>),
    Close(u16, String),
    /// Reader finished: stop the writer.
    Shutdown,
}

type Waker = Arc<dyn Fn() + Send + Sync>;

struct Shared {
    state: AtomicU8,
    buffered: AtomicU64,
    protocol: Mutex<String>,
    extensions: Mutex<String>,
    waker: Option<Waker>,
    /// Write half, once connected (for shutdown on close).
    writer: Mutex<Option<WriteHalf>>,
    close_sent: AtomicU8,
}

/// A WebSocket connection. Dropping it closes the socket.
pub struct WebSocket {
    shared: Arc<Shared>,
    events: Receiver<WsEvent>,
    cmd: Sender<Cmd>,
    url: Url,
}

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// SHA-1 (only for the Sec-WebSocket-Accept check).
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, v) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

/// Non-cryptographic random bytes (mask keys, handshake nonce): SipHash
/// with a random per-process key over a counter.
pub fn random_bytes(buf: &mut [u8]) {
    use std::hash::{BuildHasher, Hasher};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    static STATE: std::sync::OnceLock<std::collections::hash_map::RandomState> = std::sync::OnceLock::new();
    let st = STATE.get_or_init(std::collections::hash_map::RandomState::new);
    for chunk in buf.chunks_mut(8) {
        let mut h = st.build_hasher();
        h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        let v = h.finish().to_le_bytes();
        chunk.copy_from_slice(&v[..chunk.len()]);
    }
}

/// Expected `Sec-WebSocket-Accept` for a key.
pub fn accept_key(key: &str) -> String {
    base64_encode(&sha1(format!("{key}{GUID}").as_bytes()))
}

/// Encode one client frame (always masked).
pub fn encode_frame(fin: bool, rsv1: bool, opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push((if fin { 0x80 } else { 0 }) | (if rsv1 { 0x40 } else { 0 }) | (opcode & 0x0f));
    let len = payload.len();
    if len < 126 {
        out.push(0x80 | len as u8);
    } else if len <= 0xffff {
        out.push(0x80 | 126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0x80 | 127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }
    out.extend_from_slice(&mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    out
}

/// A decoded frame.
#[derive(Debug)]
pub struct RawFrame {
    pub fin: bool,
    pub rsv1: bool,
    pub opcode: u8,
    pub payload: Vec<u8>,
}

/// Read one frame (unmasking if masked).
pub fn read_frame<R: Read>(r: &mut R, max: u64) -> io::Result<RawFrame> {
    let mut h = [0u8; 2];
    r.read_exact(&mut h)?;
    let fin = h[0] & 0x80 != 0;
    let rsv1 = h[0] & 0x40 != 0;
    if h[0] & 0x30 != 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "reserved bits set"));
    }
    let opcode = h[0] & 0x0f;
    let masked = h[1] & 0x80 != 0;
    let mut len = (h[1] & 0x7f) as u64;
    if len == 126 {
        let mut b = [0u8; 2];
        r.read_exact(&mut b)?;
        len = u16::from_be_bytes(b) as u64;
    } else if len == 127 {
        let mut b = [0u8; 8];
        r.read_exact(&mut b)?;
        len = u64::from_be_bytes(b);
    }
    if len > max {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut mask = [0u8; 4];
    if masked {
        r.read_exact(&mut mask)?;
    }
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload)?;
    if masked {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
    }
    Ok(RawFrame { fin, rsv1, opcode, payload })
}

/// Negotiated permessage-deflate parameters.
#[derive(Debug, Clone, Default)]
struct Deflate {
    server_no_context_takeover: bool,
}

fn parse_extensions(v: &str) -> Result<Option<Deflate>, String> {
    let mut found = None;
    for ext in v.split(',') {
        let mut parts = ext.split(';').map(|p| p.trim());
        let name = parts.next().unwrap_or("");
        if name.is_empty() {
            continue;
        }
        if name != "permessage-deflate" {
            return Err(format!("server selected an extension we did not offer: {name}"));
        }
        let mut d = Deflate::default();
        for p in parts {
            let key = p.split('=').next().unwrap_or("").trim();
            match key {
                "server_no_context_takeover" => d.server_no_context_takeover = true,
                "client_no_context_takeover" | "server_max_window_bits" => {}
                // We never compress, so a client window limit is fine.
                "client_max_window_bits" => {}
                _ => return Err(format!("unknown permessage-deflate parameter {p}")),
            }
        }
        found = Some(d);
    }
    Ok(found)
}

impl WebSocket {
    /// Start connecting to a `ws://` / `wss://` URL. Returns immediately;
    /// `Open` (or `Error` + `Close`) arrives through `poll`. `initiator`
    /// is the document URL (Origin, cookies). `waker` is called whenever
    /// an event is queued (to wake the embedder's event loop).
    pub fn connect(fetcher: &Fetcher, url: &Url, protocols: &[String], initiator: Option<&Url>, waker: Option<Waker>) -> Result<WebSocket, String> {
        let mut url = url.clone();
        url.scheme = match url.scheme.as_str() {
            "ws" | "http" => "ws".into(),
            "wss" | "https" => "wss".into(),
            s => return Err(format!("SyntaxError: The URL's scheme must be 'ws' or 'wss' (got '{s}')")),
        };
        if url.fragment.is_some() {
            return Err("SyntaxError: The URL contains a fragment identifier".into());
        }
        for (i, p) in protocols.iter().enumerate() {
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_graphic() && !b"()<>@,;:\\\"/[]?={}".contains(&b)) || protocols[..i].contains(p) {
                return Err(format!("SyntaxError: invalid subprotocol '{p}'"));
            }
        }
        let shared = Arc::new(Shared {
            state: AtomicU8::new(CONNECTING),
            buffered: AtomicU64::new(0),
            protocol: Mutex::new(String::new()),
            extensions: Mutex::new(String::new()),
            waker,
            writer: Mutex::new(None),
            close_sent: AtomicU8::new(0),
        });
        let (ev_tx, events) = channel();
        let (cmd, cmd_rx) = channel();
        let fetcher = fetcher.clone();
        let protocols = protocols.to_vec();
        let initiator = initiator.cloned();
        let sh = shared.clone();
        let u = url.clone();
        let ctl = cmd.clone();
        std::thread::Builder::new()
            .name("rh-websocket".into())
            .spawn(move || run(fetcher, u, protocols, initiator, sh, ev_tx, cmd_rx, ctl))
            .map_err(|e| e.to_string())?;
        Ok(WebSocket { shared, events, cmd, url })
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Drain pending events (never blocks).
    pub fn poll(&self) -> Vec<WsEvent> {
        self.events.try_iter().collect()
    }

    /// Wait up to `timeout` for the next event (tests, workers).
    pub fn next_event(&self, timeout: Duration) -> Option<WsEvent> {
        self.events.recv_timeout(timeout).ok()
    }

    pub fn ready_state(&self) -> u8 {
        self.shared.state.load(Ordering::SeqCst)
    }

    pub fn protocol(&self) -> String {
        self.shared.protocol.lock().unwrap().clone()
    }

    pub fn extensions(&self) -> String {
        self.shared.extensions.lock().unwrap().clone()
    }

    /// Bytes queued but not yet written to the network.
    pub fn buffered_amount(&self) -> u64 {
        self.shared.buffered.load(Ordering::SeqCst)
    }

    fn queue(&self, opcode: u8, data: Vec<u8>) -> Result<(), String> {
        match self.ready_state() {
            CONNECTING => return Err("InvalidStateError: Still in CONNECTING state".into()),
            OPEN => {}
            // Per spec, sends after close only grow bufferedAmount.
            _ => {
                self.shared.buffered.fetch_add(data.len() as u64, Ordering::SeqCst);
                return Ok(());
            }
        }
        self.shared.buffered.fetch_add(data.len() as u64, Ordering::SeqCst);
        let _ = self.cmd.send(Cmd::Frame(opcode, data));
        Ok(())
    }

    pub fn send_text(&self, s: &str) -> Result<(), String> {
        self.queue(0x1, s.as_bytes().to_vec())
    }

    pub fn send_binary(&self, b: &[u8]) -> Result<(), String> {
        self.queue(0x2, b.to_vec())
    }

    /// Start the closing handshake. `code` must be 1000 or 3000..=4999.
    pub fn close(&self, code: Option<u16>, reason: &str) -> Result<(), String> {
        if let Some(c) = code {
            if c != 1000 && !(3000..=4999).contains(&c) {
                return Err(format!("InvalidAccessError: close code {c} is not allowed"));
            }
        }
        if reason.len() > 123 {
            return Err("SyntaxError: close reason longer than 123 bytes".into());
        }
        match self.shared.state.load(Ordering::SeqCst) {
            CLOSING | CLOSED => Ok(()),
            CONNECTING => {
                // Fail the connection: the connect thread sees CLOSING.
                self.shared.state.store(CLOSING, Ordering::SeqCst);
                let _ = self.cmd.send(Cmd::Close(code.unwrap_or(1005), reason.to_string()));
                Ok(())
            }
            _ => {
                self.shared.state.store(CLOSING, Ordering::SeqCst);
                let _ = self.cmd.send(Cmd::Close(code.unwrap_or(1005), reason.to_string()));
                Ok(())
            }
        }
    }
}

impl Drop for WebSocket {
    fn drop(&mut self) {
        if self.ready_state() == OPEN {
            let _ = self.close(Some(1000), "");
        }
        let sh = self.shared.clone();
        // Give the close frame a moment, then cut the socket.
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(2));
            if let Some(w) = sh.writer.lock().unwrap().take() {
                w.shutdown();
            }
        });
    }
}

fn emit(sh: &Shared, tx: &Sender<WsEvent>, ev: WsEvent) {
    let _ = tx.send(ev);
    if let Some(w) = &sh.waker {
        w();
    }
}

fn fail(sh: &Shared, tx: &Sender<WsEvent>, msg: String) {
    sh.state.store(CLOSED, Ordering::SeqCst);
    emit(sh, tx, WsEvent::Error(msg));
    emit(sh, tx, WsEvent::Close { code: 1006, reason: String::new(), was_clean: false });
}

fn handshake(fetcher: &Fetcher, url: &Url, protocols: &[String], initiator: Option<&Url>) -> Result<(Box<dyn Read + Send>, WriteHalf, String, String, Option<Deflate>), String> {
    if fetcher.block_trackers && is_tracker_host(&url.host) {
        return Err(format!("blocked ad/tracker host: {}", url.host));
    }
    let port = url.port.or(url.default_port()).unwrap_or(80);
    let c = transport::connect(&url.scheme, &url.host, port, Alpn::Http1, fetcher.timeout).map_err(|e| e.to_string())?;
    let mut conn = H1Conn::new(c.stream, false);
    let mut key_bytes = [0u8; 16];
    random_bytes(&mut key_bytes);
    let key = base64_encode(&key_bytes);
    let http_url = {
        let mut u = url.clone();
        u.scheme = if url.scheme == "wss" { "https".into() } else { "http".into() };
        u
    };
    let mut headers: Vec<(String, String)> = vec![
        ("Upgrade".into(), "websocket".into()),
        ("Pragma".into(), "no-cache".into()),
        ("Cache-Control".into(), "no-cache".into()),
        ("User-Agent".into(), fetcher.user_agent.clone()),
    ];
    if let Some(i) = initiator.filter(|i| i.is_http()) {
        headers.push(("Origin".into(), i.origin()));
    }
    headers.push(("Sec-WebSocket-Version".into(), "13".into()));
    headers.push(("Accept-Encoding".into(), "gzip, deflate, br, zstd".into()));
    headers.push(("Accept-Language".into(), fetcher.accept_language.clone()));
    let ctx = CookieContext::new(initiator, &http_url, false, "GET");
    if let Some(jar) = &fetcher.cookies {
        if let Some(c) = jar.lock().unwrap().header_for_context(&http_url, &ctx) {
            headers.push(("Cookie".into(), c));
        }
    }
    headers.push(("Sec-WebSocket-Key".into(), key.clone()));
    headers.push(("Sec-WebSocket-Extensions".into(), "permessage-deflate".into()));
    if !protocols.is_empty() {
        headers.push(("Sec-WebSocket-Protocol".into(), protocols.join(", ")));
    }
    // write_request adds "Connection: keep-alive"; WebSocket needs Upgrade.
    let mut req_url = url.clone();
    req_url.fragment = None;
    write_upgrade(&mut conn, &req_url, &headers).map_err(|e| e.to_string())?;
    let head = h1::read_head(&mut conn.io).map_err(|e| e.to_string())?.ok_or("connection closed during handshake")?;
    if let Some(jar) = &fetcher.cookies {
        let set: Vec<String> = head.headers.iter().filter(|(k, _)| k == "set-cookie").map(|(_, v)| v.clone()).collect();
        if !set.is_empty() {
            jar.lock().unwrap().store_from_response(&http_url, &set, &ctx);
        }
    }
    if head.status != 101 {
        return Err(format!("Error during WebSocket handshake: Unexpected response code: {}", head.status));
    }
    if !head.get("upgrade").map(|v| v.eq_ignore_ascii_case("websocket")).unwrap_or(false) {
        return Err("Error during WebSocket handshake: 'Upgrade' header is missing".into());
    }
    if !head.get("connection").map(|v| v.to_ascii_lowercase().split(',').any(|t| t.trim() == "upgrade")).unwrap_or(false) {
        return Err("Error during WebSocket handshake: 'Connection' header is missing".into());
    }
    if head.get("sec-websocket-accept") != Some(accept_key(&key).as_str()) {
        return Err("Error during WebSocket handshake: Incorrect 'Sec-WebSocket-Accept' header value".into());
    }
    let protocol = head.get("sec-websocket-protocol").unwrap_or("").to_string();
    if !protocol.is_empty() && !protocols.contains(&protocol) {
        return Err("Error during WebSocket handshake: Sent non-empty 'Sec-WebSocket-Protocol' header but no response was received".into());
    }
    let extensions = head.get("sec-websocket-extensions").unwrap_or("").to_string();
    let deflate = parse_extensions(&extensions).map_err(|e| format!("Error during WebSocket handshake: {e}"))?;
    let leftover = conn.io.buffer().to_vec();
    let stream = conn.io.into_inner();
    let (r, w): (ReadHalf, WriteHalf) = stream.split().map_err(|e| e.to_string())?;
    r.set_read_timeout(None);
    w.set_write_timeout(Some(Duration::from_secs(30)));
    let reader: Box<dyn Read + Send> = Box::new(io::Cursor::new(leftover).chain(r));
    Ok((reader, w, protocol, extensions, deflate))
}

fn write_upgrade(conn: &mut H1Conn, url: &Url, headers: &[(String, String)]) -> io::Result<()> {
    use std::io::Write;
    let mut target = if url.path.is_empty() { "/".to_string() } else { url.path.clone() };
    if let Some(q) = &url.query {
        target.push('?');
        target.push_str(q);
    }
    let mut head = format!("GET {target} HTTP/1.1\r\nHost: {}\r\nConnection: Upgrade\r\n", url.host_with_port());
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let s = conn.io.get_mut();
    s.write_all(head.as_bytes())?;
    s.flush()
}

#[allow(clippy::too_many_arguments)]
fn run(fetcher: Fetcher, url: Url, protocols: Vec<String>, initiator: Option<Url>, sh: Arc<Shared>, tx: Sender<WsEvent>, cmd_rx: Receiver<Cmd>, pong_tx: Sender<Cmd>) {
    let (mut reader, writer, protocol, extensions, deflate) = match handshake(&fetcher, &url, &protocols, initiator.as_ref()) {
        Ok(x) => x,
        Err(e) => {
            fail(&sh, &tx, e);
            return;
        }
    };
    if sh.state.load(Ordering::SeqCst) == CLOSING {
        // close() during CONNECTING: fail without opening.
        writer.shutdown();
        sh.state.store(CLOSED, Ordering::SeqCst);
        emit(&sh, &tx, WsEvent::Error("WebSocket is closed before the connection is established".into()));
        emit(&sh, &tx, WsEvent::Close { code: 1006, reason: String::new(), was_clean: false });
        return;
    }
    *sh.protocol.lock().unwrap() = protocol.clone();
    *sh.extensions.lock().unwrap() = extensions.clone();
    *sh.writer.lock().unwrap() = Some(writer.clone());
    sh.state.store(OPEN, Ordering::SeqCst);
    emit(&sh, &tx, WsEvent::Open { protocol, extensions });

    // Writer thread: user frames and reader-generated control frames.
    let wsh = sh.clone();
    let w2 = writer.clone();
    std::thread::spawn(move || {
        while let Ok(cmd) = cmd_rx.recv() {
            let mut mask = [0u8; 4];
            random_bytes(&mut mask);
            match cmd {
                Cmd::Frame(op, data) => {
                    let len = data.len() as u64;
                    let ok = wsh.close_sent.load(Ordering::SeqCst) == 0 && w2.send(&encode_frame(true, false, op, &data, mask)).is_ok();
                    if op <= 0x2 {
                        let mut cur = wsh.buffered.load(Ordering::SeqCst);
                        while let Err(v) = wsh.buffered.compare_exchange(cur, cur.saturating_sub(len), Ordering::SeqCst, Ordering::SeqCst) {
                            cur = v;
                        }
                    }
                    if !ok && wsh.close_sent.load(Ordering::SeqCst) == 0 {
                        break;
                    }
                }
                Cmd::Close(code, reason) => {
                    if wsh.close_sent.swap(1, Ordering::SeqCst) == 0 {
                        let mut payload = Vec::new();
                        if code != 1005 {
                            payload.extend_from_slice(&code.to_be_bytes());
                            payload.extend_from_slice(reason.as_bytes());
                        }
                        let _ = w2.send(&encode_frame(true, false, 0x8, &payload, mask));
                    }
                }
                Cmd::Shutdown => break,
            }
        }
    });

    // Reader loop.
    let mut inflater = deflate.as_ref().map(|_| flate2::Decompress::new(false));
    let mut message: Vec<u8> = Vec::new();
    let mut msg_opcode = 0u8;
    let mut msg_compressed = false;
    let close_result: (u16, String, bool);
    loop {
        let f = match read_frame(&mut reader, 256 * 1024 * 1024) {
            Ok(f) => f,
            Err(e) => {
                let st = sh.state.load(Ordering::SeqCst);
                if st != CLOSED {
                    sh.state.store(CLOSED, Ordering::SeqCst);
                    if st != CLOSING {
                        emit(&sh, &tx, WsEvent::Error(e.to_string()));
                    }
                }
                close_result = (1006, String::new(), false);
                break;
            }
        };
        if f.rsv1 && (deflate.is_none() || f.opcode >= 0x8 || f.opcode == 0x0) {
            protocol_fail(&sh, &tx, &pong_tx, "invalid RSV1");
            close_result = (1006, String::new(), false);
            break;
        }
        match f.opcode {
            0x0 | 0x1 | 0x2 => {
                if f.opcode != 0x0 {
                    if !message.is_empty() || msg_opcode != 0 {
                        protocol_fail(&sh, &tx, &pong_tx, "new message during a fragmented one");
                        close_result = (1006, String::new(), false);
                        break;
                    }
                    msg_opcode = f.opcode;
                    msg_compressed = f.rsv1;
                } else if msg_opcode == 0 {
                    protocol_fail(&sh, &tx, &pong_tx, "unexpected continuation frame");
                    close_result = (1006, String::new(), false);
                    break;
                }
                message.extend_from_slice(&f.payload);
                if !f.fin {
                    continue;
                }
                let mut data = std::mem::take(&mut message);
                let op = msg_opcode;
                msg_opcode = 0;
                if msg_compressed {
                    match inflate(inflater.as_mut().unwrap(), &data) {
                        Ok(d) => data = d,
                        Err(e) => {
                            protocol_fail(&sh, &tx, &pong_tx, &e);
                            close_result = (1006, String::new(), false);
                            break;
                        }
                    }
                    if deflate.as_ref().map(|d| d.server_no_context_takeover).unwrap_or(false) {
                        inflater = Some(flate2::Decompress::new(false));
                    }
                }
                if sh.state.load(Ordering::SeqCst) != OPEN {
                    continue; // messages after close() are discarded
                }
                if op == 0x1 {
                    match String::from_utf8(data) {
                        Ok(s) => emit(&sh, &tx, WsEvent::Message(WsMessage::Text(s))),
                        Err(_) => {
                            let _ = pong_tx.send(Cmd::Close(1007, String::new()));
                            sh.state.store(CLOSED, Ordering::SeqCst);
                            emit(&sh, &tx, WsEvent::Error("invalid UTF-8 in text frame".into()));
                            close_result = (1006, String::new(), false);
                            break;
                        }
                    }
                } else {
                    emit(&sh, &tx, WsEvent::Message(WsMessage::Binary(data)));
                }
            }
            0x8 => {
                let (code, reason) = if f.payload.len() >= 2 {
                    (u16::from_be_bytes([f.payload[0], f.payload[1]]), String::from_utf8_lossy(&f.payload[2..]).into_owned())
                } else {
                    (1005, String::new())
                };
                // Echo the close (if we did not start it).
                let _ = pong_tx.send(Cmd::Close(if code == 1005 { 1000 } else { code }, String::new()));
                close_result = (code, reason, true);
                break;
            }
            0x9 => {
                if f.payload.len() > 125 {
                    protocol_fail(&sh, &tx, &pong_tx, "control frame too long");
                    close_result = (1006, String::new(), false);
                    break;
                }
                let _ = pong_tx.send(Cmd::Frame(0xA, f.payload));
            }
            0xA => {}
            _ => {
                protocol_fail(&sh, &tx, &pong_tx, "unknown opcode");
                close_result = (1006, String::new(), false);
                break;
            }
        }
    }
    // Let the writer flush the close frame, then drop the TCP connection.
    let _ = pong_tx.send(Cmd::Shutdown);
    std::thread::sleep(Duration::from_millis(50));
    sh.state.store(CLOSED, Ordering::SeqCst);
    writer.shutdown();
    emit(&sh, &tx, WsEvent::Close { code: close_result.0, reason: close_result.1, was_clean: close_result.2 });
}

fn protocol_fail(sh: &Shared, tx: &Sender<WsEvent>, ctl: &Sender<Cmd>, msg: &str) {
    let _ = ctl.send(Cmd::Close(1002, String::new()));
    sh.state.store(CLOSED, Ordering::SeqCst);
    emit(sh, tx, WsEvent::Error(format!("WebSocket protocol error: {msg}")));
}

fn inflate(d: &mut flate2::Decompress, data: &[u8]) -> Result<Vec<u8>, String> {
    let mut input = data.to_vec();
    input.extend_from_slice(&[0x00, 0x00, 0xff, 0xff]);
    let mut out = Vec::with_capacity(data.len() * 3);
    let mut pos = 0usize;
    loop {
        out.reserve(64 * 1024);
        let before_in = d.total_in();
        let before_out = d.total_out();
        let status = d.decompress_vec(&input[pos..], &mut out, flate2::FlushDecompress::Sync).map_err(|e| format!("inflate: {e}"))?;
        pos += (d.total_in() - before_in) as usize;
        let produced = d.total_out() - before_out;
        if pos >= input.len() && out.len() < out.capacity() {
            break;
        }
        if matches!(status, flate2::Status::StreamEnd) || (produced == 0 && d.total_in() == before_in) {
            break;
        }
        if out.len() > 256 * 1024 * 1024 {
            return Err("inflated message too large".into());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_vectors() {
        let hex = |b: [u8; 20]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(hex(sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(hex(sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(hex(sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")), "84983e441c3bd26ebaae4aa1f95129e5e54670f1");
        // RFC 6455 section 1.3 example.
        assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn frame_roundtrip() {
        for len in [0usize, 5, 125, 126, 65535, 65536] {
            let data: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let f = encode_frame(true, false, 0x2, &data, [1, 2, 3, 4]);
            let back = read_frame(&mut &f[..], u64::MAX).unwrap();
            assert!(back.fin);
            assert_eq!(back.opcode, 2);
            assert_eq!(back.payload, data);
        }
    }

    #[test]
    fn inflate_permessage() {
        // "Hello" compressed per RFC 7692 section 7.2.3.1.
        let mut d = flate2::Decompress::new(false);
        assert_eq!(inflate(&mut d, &[0xf2, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00]).unwrap(), b"Hello");
        // Context takeover: second message references the first.
        assert_eq!(inflate(&mut d, &[0xf2, 0x00, 0x11, 0x00, 0x00]).unwrap(), b"Hello");
    }
}
