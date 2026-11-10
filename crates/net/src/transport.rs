//! Connections: DNS (with a small cache), TCP connect, TLS via rustls with
//! ALPN, HTTP proxies from the environment, and a thread-safe split of a
//! TLS stream into independent read/write halves (for HTTP/2 and
//! WebSocket, where one thread reads while others write).

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection};

/// Application protocols we offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Alpn {
    /// `h2`, then `http/1.1`.
    H2OrHttp1,
    /// `http/1.1` only (after an HTTP/2 failure, and for WebSocket).
    Http1,
}

fn tls_config(alpn: Alpn) -> Arc<ClientConfig> {
    static CONFIGS: OnceLock<(Arc<ClientConfig>, Arc<ClientConfig>)> = OnceLock::new();
    let (both, h1) = CONFIGS.get_or_init(|| {
        let make = |protos: Vec<Vec<u8>>| {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let mut cfg = ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .expect("TLS protocol versions")
                .with_root_certificates(roots)
                .with_no_client_auth();
            cfg.alpn_protocols = protos;
            Arc::new(cfg)
        };
        (make(vec![b"h2".to_vec(), b"http/1.1".to_vec()]), make(vec![b"http/1.1".to_vec()]))
    });
    match alpn {
        Alpn::H2OrHttp1 => both.clone(),
        Alpn::Http1 => h1.clone(),
    }
}

// ------------------------------------------------------------------ DNS

fn dns_cache() -> &'static Mutex<HashMap<(String, u16), (Vec<SocketAddr>, Instant)>> {
    static C: OnceLock<Mutex<HashMap<(String, u16), (Vec<SocketAddr>, Instant)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

const DNS_TTL: Duration = Duration::from_secs(60);

/// Resolve `host:port` (cached for a minute).
pub fn resolve(host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
    let host_clean = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host_clean.parse::<std::net::IpAddr>() {
        return Ok(vec![SocketAddr::new(ip, port)]);
    }
    let key = (host_clean.to_ascii_lowercase(), port);
    if let Some((addrs, t)) = dns_cache().lock().unwrap().get(&key) {
        if t.elapsed() < DNS_TTL {
            return Ok(addrs.clone());
        }
    }
    let addrs: Vec<SocketAddr> = (host_clean, port).to_socket_addrs()?.collect();
    if addrs.is_empty() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("no addresses for {host}")));
    }
    dns_cache().lock().unwrap().insert(key, (addrs.clone(), Instant::now()));
    Ok(addrs)
}

fn forget_dns(host: &str, port: u16) {
    dns_cache().lock().unwrap().remove(&(host.to_ascii_lowercase(), port));
}

/// Open a TCP connection, trying each address in turn (IPv4 and IPv6
/// interleaved, so a broken family does not cost the whole timeout).
pub fn tcp_connect(host: &str, port: u16, timeout: Duration) -> io::Result<TcpStream> {
    let addrs = resolve(host, port)?;
    let (v6, v4): (Vec<SocketAddr>, Vec<SocketAddr>) = addrs.iter().partition(|a| a.is_ipv6());
    let mut ordered = Vec::with_capacity(addrs.len());
    let first_v6 = addrs.first().map(|a| a.is_ipv6()).unwrap_or(false);
    let (a, b) = if first_v6 { (v6, v4) } else { (v4, v6) };
    for i in 0..a.len().max(b.len()) {
        if let Some(x) = a.get(i) {
            ordered.push(*x);
        }
        if let Some(x) = b.get(i) {
            ordered.push(*x);
        }
    }
    let per_try = if ordered.len() > 1 { timeout.min(Duration::from_secs(4)).max(Duration::from_millis(500)) } else { timeout };
    let mut last = io::Error::new(io::ErrorKind::NotFound, "no address");
    for addr in ordered {
        match TcpStream::connect_timeout(&addr, per_try) {
            Ok(s) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Err(e) => last = e,
        }
    }
    forget_dns(host, port);
    Err(last)
}

// ---------------------------------------------------------------- proxy

/// An HTTP proxy from `HTTPS_PROXY` / `HTTP_PROXY` / `ALL_PROXY`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Proxy {
    pub host: String,
    pub port: u16,
    /// `Proxy-Authorization` value for `user:pass@` proxies.
    pub auth: Option<String>,
}

fn env_any(names: &[&str]) -> Option<String> {
    names.iter().find_map(|n| std::env::var(n).ok()).filter(|v| !v.trim().is_empty())
}

/// The proxy to use for `scheme://host`, honouring `NO_PROXY`.
pub fn proxy_for(scheme: &str, host: &str) -> Option<Proxy> {
    let raw = if scheme == "https" || scheme == "wss" {
        env_any(&["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"])
    } else {
        env_any(&["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"])
    }?;
    if let Some(no) = env_any(&["NO_PROXY", "no_proxy"]) {
        let h = host.to_ascii_lowercase();
        for pat in no.split(',').map(|p| p.trim().trim_start_matches('*').to_ascii_lowercase()) {
            if pat.is_empty() {
                continue;
            }
            if pat == "*" {
                return None;
            }
            let p = pat.trim_start_matches('.');
            if h == p || h.ends_with(&format!(".{p}")) {
                return None;
            }
        }
    }
    if matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]") {
        return None;
    }
    let rest = raw.trim();
    let rest = match rest.split_once("://") {
        Some((s, r)) if s.eq_ignore_ascii_case("http") => r,
        Some(_) => return None, // only HTTP proxies (no SOCKS)
        None => rest,
    };
    let rest = rest.trim_end_matches('/');
    let (auth, hostport) = match rest.rsplit_once('@') {
        Some((a, h)) => (Some(format!("Basic {}", base64_encode(crate::url::percent_decode(a).as_bytes()))), h),
        None => (None, rest),
    };
    let (h, p) = match hostport.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => (h.to_string(), p.parse().ok()?),
        _ => (hostport.to_string(), 80),
    };
    Some(Proxy { host: h, port: p, auth })
}

/// Standard base64 with padding.
pub fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Open a CONNECT tunnel through `proxy` to `host:port`.
fn connect_tunnel(proxy: &Proxy, host: &str, port: u16, timeout: Duration) -> io::Result<TcpStream> {
    let mut s = tcp_connect(&proxy.host, proxy.port, timeout)?;
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(timeout))?;
    let mut req = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n");
    if let Some(a) = &proxy.auth {
        req.push_str(&format!("Proxy-Authorization: {a}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes())?;
    // Read the response head byte by byte (it is tiny) so no tunnelled
    // bytes are consumed.
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if s.read(&mut b)? == 0 || head.len() > 16 * 1024 {
            return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "proxy closed the CONNECT tunnel"));
        }
        head.push(b[0]);
    }
    let line = String::from_utf8_lossy(&head);
    let code = line.split_whitespace().nth(1).and_then(|c| c.parse::<u16>().ok()).unwrap_or(0);
    if !(200..300).contains(&code) {
        return Err(io::Error::new(io::ErrorKind::ConnectionRefused, format!("proxy CONNECT failed: {}", line.lines().next().unwrap_or(""))));
    }
    Ok(s)
}

// --------------------------------------------------------------- stream

/// A connected byte stream (plain TCP or TLS).
pub enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<ClientConnection, TcpStream>>),
}

impl Stream {
    pub fn tcp(&self) -> &TcpStream {
        match self {
            Stream::Plain(s) => s,
            Stream::Tls(t) => &t.sock,
        }
    }

    pub fn set_timeouts(&self, read: Option<Duration>, write: Option<Duration>) {
        let _ = self.tcp().set_read_timeout(read);
        let _ = self.tcp().set_write_timeout(write);
    }

    /// Split into independently usable read and write halves.
    pub fn split(self) -> io::Result<(ReadHalf, WriteHalf)> {
        match self {
            Stream::Plain(s) => {
                let r = s.try_clone()?;
                let shared = Arc::new(SplitShared { tls: None, wsock: Mutex::new(s) });
                Ok((ReadHalf { shared: shared.clone(), sock: r, plain: Vec::new(), pos: 0, eof: false }, WriteHalf { shared }))
            }
            Stream::Tls(t) => {
                let rustls::StreamOwned { mut conn, sock } = *t;
                conn.set_buffer_limit(None);
                let r = sock.try_clone()?;
                let shared = Arc::new(SplitShared { tls: Some(Mutex::new(conn)), wsock: Mutex::new(sock) });
                Ok((ReadHalf { shared: shared.clone(), sock: r, plain: Vec::new(), pos: 0, eof: false }, WriteHalf { shared }))
            }
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls(t) => match t.read(buf) {
                // Servers often close without close_notify; treat as EOF.
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(0),
                r => r,
            },
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Tls(t) => t.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(t) => t.flush(),
        }
    }
}

/// Result of `connect`.
pub struct Connected {
    pub stream: Stream,
    /// The server selected `h2` via ALPN.
    pub h2: bool,
    /// Requests go to an HTTP proxy in absolute form (plain http via proxy).
    pub via_proxy: bool,
}

/// Connect to `scheme://host:port`: TCP (possibly via a proxy), then TLS
/// for https/wss with the given ALPN offer.
pub fn connect(scheme: &str, host: &str, port: u16, alpn: Alpn, timeout: Duration) -> io::Result<Connected> {
    let secure = scheme == "https" || scheme == "wss";
    let proxy = proxy_for(scheme, host);
    let (tcp, via_proxy) = match (&proxy, secure) {
        (Some(p), true) => (connect_tunnel(p, host, port, timeout)?, false),
        (Some(p), false) => (tcp_connect(&p.host, p.port, timeout)?, true),
        (None, _) => (tcp_connect(host, port, timeout)?, false),
    };
    tcp.set_read_timeout(Some(timeout))?;
    tcp.set_write_timeout(Some(timeout))?;
    if !secure {
        return Ok(Connected { stream: Stream::Plain(tcp), h2: false, via_proxy });
    }
    let name_str = host.trim_start_matches('[').trim_end_matches(']').to_string();
    let name = ServerName::try_from(name_str).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    let mut conn = ClientConnection::new(tls_config(alpn), name).map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
    let mut tcp = tcp;
    while conn.is_handshaking() {
        conn.complete_io(&mut tcp).map_err(|e| io::Error::new(e.kind(), format!("TLS handshake with {host}: {e}")))?;
    }
    let h2 = conn.alpn_protocol() == Some(b"h2");
    Ok(Connected { stream: Stream::Tls(Box::new(rustls::StreamOwned::new(conn, tcp))), h2, via_proxy })
}

// ---------------------------------------------------------------- split

struct SplitShared {
    tls: Option<Mutex<ClientConnection>>,
    /// Socket used for writing; its lock also orders TLS records.
    wsock: Mutex<TcpStream>,
}

impl SplitShared {
    /// Encrypt (if TLS) and send `data`. Lock order: socket, then TLS.
    fn send(&self, data: &[u8]) -> io::Result<()> {
        let mut sock = self.wsock.lock().unwrap();
        match &self.tls {
            None => sock.write_all(data),
            Some(tls) => {
                let mut out = Vec::with_capacity(data.len() + 64);
                {
                    let mut conn = tls.lock().unwrap();
                    if !data.is_empty() {
                        conn.writer().write_all(data)?;
                    }
                    while conn.wants_write() {
                        conn.write_tls(&mut out)?;
                    }
                }
                sock.write_all(&out)
            }
        }
    }
}

/// Reading side of a split stream.
pub struct ReadHalf {
    shared: Arc<SplitShared>,
    sock: TcpStream,
    plain: Vec<u8>,
    pos: usize,
    eof: bool,
}

impl ReadHalf {
    pub fn set_read_timeout(&self, t: Option<Duration>) {
        let _ = self.sock.set_read_timeout(t);
    }
}

impl Read for ReadHalf {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.pos < self.plain.len() {
                let n = buf.len().min(self.plain.len() - self.pos);
                buf[..n].copy_from_slice(&self.plain[self.pos..self.pos + n]);
                self.pos += n;
                return Ok(n);
            }
            if self.eof {
                return Ok(0);
            }
            self.plain.clear();
            self.pos = 0;
            let mut raw = [0u8; 32 * 1024];
            let n = self.sock.read(&mut raw)?;
            let Some(tls) = &self.shared.tls else {
                if n == 0 {
                    self.eof = true;
                    return Ok(0);
                }
                self.plain.extend_from_slice(&raw[..n]);
                continue;
            };
            let mut needs_write = false;
            {
                let mut conn = tls.lock().unwrap();
                let mut slice = &raw[..n];
                if n == 0 {
                    // TCP EOF: let rustls see it.
                    let _ = conn.read_tls(&mut slice);
                }
                loop {
                    if !slice.is_empty() {
                        conn.read_tls(&mut slice)?;
                    }
                    let state = conn.process_new_packets().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
                    let avail = state.plaintext_bytes_to_read();
                    if avail > 0 {
                        let start = self.plain.len();
                        self.plain.resize(start + avail, 0);
                        conn.reader().read_exact(&mut self.plain[start..])?;
                    }
                    if state.peer_has_closed() {
                        self.eof = true;
                    }
                    if slice.is_empty() {
                        break;
                    }
                }
                needs_write = needs_write || conn.wants_write();
            }
            if needs_write {
                // e.g. a TLS 1.3 KeyUpdate response.
                self.shared.send(&[])?;
            }
            if n == 0 {
                self.eof = true;
            }
        }
    }
}

/// Writing side of a split stream; cloneable and usable from any thread.
#[derive(Clone)]
pub struct WriteHalf {
    shared: Arc<SplitShared>,
}

impl WriteHalf {
    /// Write all of `data` atomically with respect to other writers.
    pub fn send(&self, data: &[u8]) -> io::Result<()> {
        self.shared.send(data)
    }

    /// Shut down both directions of the socket (wakes a blocked reader).
    pub fn shutdown(&self) {
        if let Ok(s) = self.shared.wsock.lock() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }

    pub fn set_write_timeout(&self, t: Option<Duration>) {
        if let Ok(s) = self.shared.wsock.lock() {
            let _ = s.set_write_timeout(t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"user:pass"), "dXNlcjpwYXNz");
    }

    #[test]
    fn split_plain_roundtrip() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut b = [0u8; 5];
            s.read_exact(&mut b).unwrap();
            s.write_all(&b).unwrap();
        });
        let c = connect("http", "127.0.0.1", port, Alpn::Http1, Duration::from_secs(5)).unwrap();
        let (mut r, w) = c.stream.split().unwrap();
        w.send(b"hello").unwrap();
        let mut b = [0u8; 5];
        r.read_exact(&mut b).unwrap();
        assert_eq!(&b, b"hello");
        t.join().unwrap();
    }
}
