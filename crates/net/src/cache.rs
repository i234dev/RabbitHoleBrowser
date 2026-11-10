//! HTTP cache (RFC 9111) for a private (single-user) browser cache:
//! memory LRU in front of an on-disk store in the platform cache
//! directory. Honors Cache-Control (max-age, no-store, no-cache,
//! private/public, must-revalidate, immutable, stale-while-revalidate),
//! Expires, heuristic freshness from Last-Modified, validators (ETag /
//! Last-Modified) and Vary.

use crate::cookies::parse_http_date;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Parsed Cache-Control directives.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct CacheControl {
    pub max_age: Option<u64>,
    pub s_maxage: Option<u64>,
    pub no_store: bool,
    pub no_cache: bool,
    pub private: bool,
    pub public: bool,
    pub must_revalidate: bool,
    pub immutable: bool,
    pub stale_while_revalidate: Option<u64>,
    pub max_stale: Option<u64>,
    pub min_fresh: Option<u64>,
    pub only_if_cached: bool,
}

impl CacheControl {
    pub fn parse<'a>(values: impl Iterator<Item = &'a str>) -> CacheControl {
        let mut cc = CacheControl::default();
        for value in values {
            for d in value.split(',') {
                let d = d.trim();
                if d.is_empty() {
                    continue;
                }
                let (name, arg) = match d.split_once('=') {
                    Some((n, a)) => (n.trim().to_ascii_lowercase(), Some(a.trim().trim_matches('"'))),
                    None => (d.to_ascii_lowercase(), None),
                };
                let num = || arg.and_then(|a| a.parse::<u64>().ok());
                match name.as_str() {
                    // Duplicates: the most restrictive (smallest) wins.
                    "max-age" => cc.max_age = Some(num().map(|v| cc.max_age.map_or(v, |o| o.min(v))).unwrap_or(0)),
                    "s-maxage" => cc.s_maxage = num(),
                    "no-store" => cc.no_store = true,
                    "no-cache" => cc.no_cache = true,
                    "private" => cc.private = true,
                    "public" => cc.public = true,
                    "must-revalidate" | "proxy-revalidate" => cc.must_revalidate = true,
                    "immutable" => cc.immutable = true,
                    "stale-while-revalidate" => cc.stale_while_revalidate = num(),
                    "max-stale" => cc.max_stale = Some(num().unwrap_or(u64::MAX)),
                    "min-fresh" => cc.min_fresh = num(),
                    "only-if-cached" => cc.only_if_cached = true,
                    _ => {}
                }
            }
        }
        cc
    }

    pub fn from_headers(headers: &[(String, String)]) -> CacheControl {
        let mut cc = CacheControl::parse(headers.iter().filter(|(k, _)| k == "cache-control").map(|(_, v)| v.as_str()));
        // HTTP/1.0 `Pragma: no-cache` when there is no Cache-Control.
        if !headers.iter().any(|(k, _)| k == "cache-control") && headers.iter().any(|(k, v)| k == "pragma" && v.to_ascii_lowercase().contains("no-cache")) {
            cc.no_cache = true;
        }
        cc
    }
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

/// A stored response.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub url: String,
    pub status: u16,
    /// Stored response headers (lower-case names; no hop-by-hop,
    /// Set-Cookie or content coding headers: the body is decoded).
    pub headers: Vec<(String, String)>,
    pub body: Arc<Vec<u8>>,
    /// Unix seconds when the request was sent / response received.
    pub request_time: u64,
    pub response_time: u64,
    /// Request header values selected by Vary, at store time.
    pub vary: Vec<(String, String)>,
}

/// What the cache can do for a request.
#[derive(Debug, Clone)]
pub enum Lookup {
    /// Serve the stored response as is.
    Fresh(Arc<CacheEntry>),
    /// Stale: may be served while a background revalidation runs
    /// (stale-while-revalidate).
    StaleWhileRevalidate(Arc<CacheEntry>),
    /// Stale or no-cache: revalidate with these conditional headers
    /// (empty when the entry has no validators: fetch normally).
    Revalidate(Arc<CacheEntry>, Vec<(String, String)>),
    Miss,
}

impl CacheEntry {
    pub fn header(&self, name: &str) -> Option<&str> {
        header(&self.headers, name)
    }

    pub fn cache_control(&self) -> CacheControl {
        CacheControl::from_headers(&self.headers)
    }

    /// Freshness lifetime in seconds (RFC 9111 4.2.1).
    pub fn freshness_lifetime(&self) -> u64 {
        let cc = self.cache_control();
        if let Some(ma) = cc.max_age {
            return ma;
        }
        let date = self.header("date").and_then(parse_http_date).unwrap_or(self.response_time);
        if let Some(exp) = self.header("expires") {
            return match parse_http_date(exp) {
                Some(e) => e.saturating_sub(date),
                None => 0, // invalid Expires (e.g. "0") means already expired
            };
        }
        // Heuristic: 10% of the time since last modification (max 1 week).
        if heuristically_cacheable(self.status) {
            if let Some(lm) = self.header("last-modified").and_then(parse_http_date) {
                return (date.saturating_sub(lm) / 10).min(7 * 86400);
            }
        }
        0
    }

    /// Current age in seconds (RFC 9111 4.2.3).
    pub fn current_age(&self, now: u64) -> u64 {
        let date = self.header("date").and_then(parse_http_date).unwrap_or(self.response_time);
        let age_value = self.header("age").and_then(|a| a.trim().parse::<u64>().ok()).unwrap_or(0);
        let apparent_age = self.response_time.saturating_sub(date);
        let response_delay = self.response_time.saturating_sub(self.request_time);
        let corrected_age_value = age_value + response_delay;
        let corrected_initial_age = apparent_age.max(corrected_age_value);
        corrected_initial_age + now.saturating_sub(self.response_time)
    }

    /// Conditional request headers from the stored validators.
    pub fn validators(&self) -> Vec<(String, String)> {
        let mut v = Vec::new();
        if let Some(etag) = self.header("etag") {
            v.push(("If-None-Match".to_string(), etag.to_string()));
        }
        if let Some(lm) = self.header("last-modified") {
            v.push(("If-Modified-Since".to_string(), lm.to_string()));
        }
        v
    }

    fn size(&self) -> usize {
        self.body.len() + self.headers.iter().map(|(k, v)| k.len() + v.len() + 16).sum::<usize>() + self.url.len() + 128
    }

    /// Whether the request headers select this variant.
    fn vary_matches(&self, req_headers: &[(String, String)]) -> bool {
        self.vary.iter().all(|(name, stored)| {
            let cur = header(req_headers, name).map(normalize_vary_value).unwrap_or_default();
            cur == *stored
        })
    }
}

fn normalize_vary_value(v: &str) -> String {
    v.split(',').map(|p| p.trim()).collect::<Vec<_>>().join(",")
}

/// Statuses cacheable by default (heuristic freshness allowed).
fn heuristically_cacheable(status: u16) -> bool {
    matches!(status, 200 | 203 | 204 | 300 | 301 | 308 | 404 | 405 | 410 | 414 | 501)
}

/// Fetch cache mode (Request.cache).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CacheMode {
    /// Normal HTTP caching.
    #[default]
    Default,
    /// Bypass the cache entirely (no lookup, no store).
    NoStore,
    /// Fetch from the network, then update the cache.
    Reload,
    /// Always revalidate a stored response.
    NoCache,
    /// Use any stored response, however stale.
    ForceCache,
    /// Only the cache; a miss is an error.
    OnlyIfCached,
}

/// Should this response be stored? (RFC 9111 3)
pub fn storable(method: &str, status: u16, req_headers: &[(String, String)], resp_headers: &[(String, String)]) -> bool {
    if method != "GET" {
        return false;
    }
    if !(200..600).contains(&status) || status == 206 {
        return false;
    }
    let rcc = CacheControl::from_headers(req_headers);
    let cc = CacheControl::from_headers(resp_headers);
    if rcc.no_store || cc.no_store {
        return false;
    }
    if header(resp_headers, "vary").map(|v| v.split(',').any(|p| p.trim() == "*")).unwrap_or(false) {
        return false;
    }
    if header(req_headers, "authorization").is_some() && !(cc.public || cc.must_revalidate || cc.s_maxage.is_some()) {
        return false;
    }
    let explicit = cc.max_age.is_some() || header(resp_headers, "expires").is_some() || cc.public;
    let validators = header(resp_headers, "etag").is_some() || header(resp_headers, "last-modified").is_some();
    explicit || (heuristically_cacheable(status) && (validators || cc.no_cache))
}

/// Headers never stored.
fn strip_for_storage(headers: &[(String, String)]) -> Vec<(String, String)> {
    headers
        .iter()
        .filter(|(k, _)| {
            !matches!(
                k.as_str(),
                "set-cookie" | "set-cookie2" | "connection" | "keep-alive" | "transfer-encoding" | "content-encoding" | "content-length" | "proxy-authenticate" | "trailer" | "upgrade"
            )
        })
        .cloned()
        .collect()
}

// ---------------------------------------------------------------- disk

/// Platform cache directory for RabbitHole's HTTP cache:
/// `%LOCALAPPDATA%\RabbitHole\Cache\http` on Windows,
/// `~/Library/Caches/RabbitHole/http` on macOS and
/// `$XDG_CACHE_HOME/rabbithole/http` (default `~/.cache`) elsewhere.
pub fn platform_cache_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("RH_CACHE_DIR") {
        return Some(PathBuf::from(dir));
    }
    if cfg!(target_os = "windows") {
        let base = std::env::var_os("LOCALAPPDATA").or_else(|| std::env::var_os("APPDATA"))?;
        Some(PathBuf::from(base).join("RabbitHole").join("Cache").join("http"))
    } else if cfg!(target_os = "macos") {
        Some(PathBuf::from(std::env::var_os("HOME")?).join("Library/Caches/RabbitHole/http"))
    } else {
        let base = match std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
            Some(x) => PathBuf::from(x),
            None => PathBuf::from(std::env::var_os("HOME")?).join(".cache"),
        };
        Some(base.join("rabbithole").join("http"))
    }
}

fn fnv64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

const DISK_MAGIC: &str = "RHCACHE1";

struct DiskCache {
    dir: PathBuf,
    max_bytes: u64,
    /// Bytes written since the last size sweep.
    written: AtomicU64,
    sweep: Mutex<()>,
}

impl DiskCache {
    fn path_for(&self, key: &str) -> PathBuf {
        let h = format!("{:016x}", fnv64(key));
        self.dir.join(&h[..2]).join(&h[2..])
    }

    fn load(&self, key: &str) -> Option<CacheEntry> {
        let path = self.path_for(key);
        let mut f = std::fs::File::open(&path).ok()?;
        let mut data = Vec::new();
        f.read_to_end(&mut data).ok()?;
        let entry = decode_entry(&data);
        match entry {
            Some(e) if e.url == key => Some(e),
            _ => {
                let _ = std::fs::remove_file(&path);
                None
            }
        }
    }

    fn store(&self, e: &CacheEntry) {
        let path = self.path_for(&e.url);
        let Some(parent) = path.parent() else { return };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        let data = encode_entry(e);
        let ok = std::fs::File::create(&tmp).and_then(|mut f| f.write_all(&data)).is_ok();
        if !ok || std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        let w = self.written.fetch_add(data.len() as u64, Ordering::Relaxed) + data.len() as u64;
        if w > self.max_bytes / 10 {
            self.written.store(0, Ordering::Relaxed);
            self.evict();
        }
    }

    fn remove(&self, key: &str) {
        let _ = std::fs::remove_file(self.path_for(key));
    }

    /// Delete least recently written files until under 80% of the budget.
    fn evict(&self) {
        let Ok(_g) = self.sweep.try_lock() else { return };
        let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
        let Ok(dirs) = std::fs::read_dir(&self.dir) else { return };
        for d in dirs.flatten() {
            let Ok(entries) = std::fs::read_dir(d.path()) else { continue };
            for f in entries.flatten() {
                if let Ok(m) = f.metadata() {
                    if m.is_file() {
                        files.push((m.modified().unwrap_or(std::time::UNIX_EPOCH), m.len(), f.path()));
                    }
                }
            }
        }
        let total: u64 = files.iter().map(|f| f.1).sum();
        if total <= self.max_bytes {
            return;
        }
        files.sort_by_key(|f| f.0);
        let mut cur = total;
        for (_, len, path) in files {
            if cur <= self.max_bytes * 8 / 10 {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                cur -= len;
            }
        }
    }
}

fn encode_entry(e: &CacheEntry) -> Vec<u8> {
    let clean = |s: &str| s.replace(['\n', '\r'], " ");
    let mut head = String::new();
    head.push_str(DISK_MAGIC);
    head.push('\n');
    head.push_str(&clean(&e.url));
    head.push('\n');
    head.push_str(&format!("{}\n{}\n{}\n{}\n", e.status, e.request_time, e.response_time, e.vary.len()));
    for (k, v) in &e.vary {
        head.push_str(&format!("{}\t{}\n", clean(k), clean(v)));
    }
    head.push_str(&format!("{}\n", e.headers.len()));
    for (k, v) in &e.headers {
        head.push_str(&format!("{}\t{}\n", clean(k), clean(v)));
    }
    head.push_str(&format!("{}\n", e.body.len()));
    let mut out = head.into_bytes();
    out.extend_from_slice(&e.body);
    out
}

fn decode_entry(data: &[u8]) -> Option<CacheEntry> {
    let mut pos = 0usize;
    let mut line = || -> Option<String> {
        let rest = data.get(pos..)?;
        let nl = rest.iter().position(|&b| b == b'\n')?;
        let s = String::from_utf8_lossy(&rest[..nl]).into_owned();
        pos += nl + 1;
        Some(s)
    };
    if line()? != DISK_MAGIC {
        return None;
    }
    let url = line()?;
    let status = line()?.parse().ok()?;
    let request_time = line()?.parse().ok()?;
    let response_time = line()?.parse().ok()?;
    let nvary: usize = line()?.parse().ok()?;
    let mut vary = Vec::new();
    for _ in 0..nvary {
        let l = line()?;
        let (k, v) = l.split_once('\t')?;
        vary.push((k.to_string(), v.to_string()));
    }
    let nh: usize = line()?.parse().ok()?;
    let mut headers = Vec::new();
    for _ in 0..nh.min(10_000) {
        let l = line()?;
        let (k, v) = l.split_once('\t')?;
        headers.push((k.to_string(), v.to_string()));
    }
    let len: usize = line()?.parse().ok()?;
    let body = data.get(pos..pos.checked_add(len)?)?.to_vec();
    Some(CacheEntry { url, status, headers, body: Arc::new(body), request_time, response_time, vary })
}

// --------------------------------------------------------------- cache

#[derive(Default)]
struct MemCache {
    /// URL -> variants (most recent first).
    map: HashMap<String, Vec<(Arc<CacheEntry>, u64)>>,
    bytes: usize,
    tick: u64,
}

/// The HTTP cache (memory, optionally backed by disk).
pub struct HttpCache {
    mem: Mutex<MemCache>,
    mem_max: usize,
    disk: Option<DiskCache>,
    /// Largest body stored.
    pub max_entry: usize,
    /// Skip the disk for loopback hosts (tests, local dev servers).
    pub persist_loopback: bool,
}

impl std::fmt::Debug for HttpCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HttpCache(disk: {:?})", self.disk.as_ref().map(|d| &d.dir))
    }
}

impl HttpCache {
    /// Memory-only cache.
    pub fn memory(mem_max: usize) -> HttpCache {
        HttpCache { mem: Mutex::new(MemCache::default()), mem_max, disk: None, max_entry: 16 * 1024 * 1024, persist_loopback: false }
    }

    /// Memory cache backed by `dir` (created if needed) with a disk budget.
    pub fn with_disk(mem_max: usize, dir: &Path, disk_max: u64) -> HttpCache {
        let mut c = HttpCache::memory(mem_max);
        if std::fs::create_dir_all(dir).is_ok() {
            c.disk = Some(DiskCache { dir: dir.to_path_buf(), max_bytes: disk_max, written: AtomicU64::new(0), sweep: Mutex::new(()) });
        }
        c
    }

    pub fn disk_dir(&self) -> Option<&Path> {
        self.disk.as_ref().map(|d| d.dir.as_path())
    }

    fn use_disk(&self, url: &str) -> Option<&DiskCache> {
        let d = self.disk.as_ref()?;
        if !self.persist_loopback {
            let host = url.split("://").nth(1).unwrap_or("").split(['/', '?', '#']).next().unwrap_or("");
            let host = host.rsplit_once(':').map(|(h, p)| if p.chars().all(|c| c.is_ascii_digit()) { h } else { host }).unwrap_or(host);
            if matches!(host, "localhost" | "127.0.0.1" | "[::1]") {
                return None;
            }
        }
        Some(d)
    }

    /// Find a stored variant of `key` matching the request headers.
    pub fn get(&self, key: &str, req_headers: &[(String, String)]) -> Option<Arc<CacheEntry>> {
        {
            let mut mem = self.mem.lock().unwrap();
            mem.tick += 1;
            let tick = mem.tick;
            if let Some(vs) = mem.map.get_mut(key) {
                if let Some(v) = vs.iter_mut().find(|(e, _)| e.vary_matches(req_headers)) {
                    v.1 = tick;
                    return Some(v.0.clone());
                }
            }
        }
        let disk = self.use_disk(key)?;
        let e = Arc::new(disk.load(key)?);
        if !e.vary_matches(req_headers) {
            return None;
        }
        self.insert_mem(e.clone());
        Some(e)
    }

    /// Decide how to answer a GET from the cache.
    pub fn lookup(&self, key: &str, req_headers: &[(String, String)], mode: CacheMode) -> Lookup {
        if matches!(mode, CacheMode::NoStore | CacheMode::Reload) {
            return Lookup::Miss;
        }
        let Some(e) = self.get(key, req_headers) else { return Lookup::Miss };
        if matches!(mode, CacheMode::ForceCache | CacheMode::OnlyIfCached) {
            return Lookup::Fresh(e);
        }
        let rcc = CacheControl::from_headers(req_headers);
        let cc = e.cache_control();
        let now = now_secs();
        let age = e.current_age(now);
        let lifetime = e.freshness_lifetime();
        let must_validate = cc.no_cache || rcc.no_cache || mode == CacheMode::NoCache || rcc.max_age == Some(0);
        if !must_validate {
            let mut fresh_needed = age;
            if let Some(mf) = rcc.min_fresh {
                fresh_needed += mf;
            }
            let req_max_age_ok = rcc.max_age.map(|m| age <= m).unwrap_or(true);
            if lifetime > fresh_needed && req_max_age_ok {
                return Lookup::Fresh(e);
            }
            if let Some(ms) = rcc.max_stale {
                if !cc.must_revalidate && age <= lifetime.saturating_add(ms) {
                    return Lookup::Fresh(e);
                }
            }
            if let Some(swr) = cc.stale_while_revalidate {
                if !cc.must_revalidate && age < lifetime.saturating_add(swr) && !e.validators().is_empty() {
                    return Lookup::StaleWhileRevalidate(e);
                }
            }
        } else if mode == CacheMode::NoCache && cc.immutable && lifetime > age && !cc.no_cache {
            // A reload does not revalidate immutable fresh resources.
            return Lookup::Fresh(e);
        }
        let v = e.validators();
        Lookup::Revalidate(e, v)
    }

    fn insert_mem(&self, e: Arc<CacheEntry>) {
        let size = e.size();
        if size > self.mem_max / 4 {
            return;
        }
        let mut mem = self.mem.lock().unwrap();
        mem.tick += 1;
        let tick = mem.tick;
        let mut freed = 0;
        let vs = mem.map.entry(e.url.clone()).or_default();
        vs.retain(|(old, _)| {
            let same = old.vary == e.vary;
            if same {
                freed += old.size();
            }
            !same
        });
        vs.insert(0, (e, tick));
        vs.truncate(4);
        mem.bytes = mem.bytes.saturating_sub(freed) + size;
        // Evict least recently used variants.
        while mem.bytes > self.mem_max {
            let victim = mem
                .map
                .iter()
                .flat_map(|(k, vs)| vs.iter().enumerate().map(move |(i, (_, t))| (*t, k.clone(), i)))
                .min_by_key(|x| x.0);
            let Some((_, k, i)) = victim else { break };
            let vs = mem.map.get_mut(&k).unwrap();
            let (gone, _) = vs.remove(i);
            if vs.is_empty() {
                mem.map.remove(&k);
            }
            mem.bytes = mem.bytes.saturating_sub(gone.size());
        }
    }

    /// Store a complete response (body already content-decoded).
    #[allow(clippy::too_many_arguments)]
    pub fn store(&self, key: &str, method: &str, status: u16, req_headers: &[(String, String)], resp_headers: &[(String, String)], body: Vec<u8>, request_time: u64, response_time: u64) -> bool {
        if body.len() > self.max_entry || !storable(method, status, req_headers, resp_headers) {
            return false;
        }
        let vary: Vec<(String, String)> = header(resp_headers, "vary")
            .map(|v| {
                v.split(',')
                    .map(|n| n.trim().to_ascii_lowercase())
                    .filter(|n| !n.is_empty())
                    .map(|n| {
                        let val = header(req_headers, &n).map(normalize_vary_value).unwrap_or_default();
                        (n, val)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let e = Arc::new(CacheEntry { url: key.to_string(), status, headers: strip_for_storage(resp_headers), body: Arc::new(body), request_time, response_time, vary });
        if let Some(d) = self.use_disk(key) {
            d.store(&e);
        }
        self.insert_mem(e);
        true
    }

    /// A 304 arrived for `entry`: merge the new headers and refresh times.
    pub fn freshen(&self, entry: &CacheEntry, not_modified_headers: &[(String, String)], request_time: u64, response_time: u64) -> Arc<CacheEntry> {
        let mut headers = entry.headers.clone();
        for (k, v) in strip_for_storage(not_modified_headers) {
            if matches!(k.as_str(), "content-type" | "content-range") {
                continue;
            }
            headers.retain(|(hk, _)| *hk != k);
            headers.push((k, v));
        }
        // Collapse to one value per name for headers that were replaced.
        let e = Arc::new(CacheEntry { url: entry.url.clone(), status: entry.status, headers, body: entry.body.clone(), request_time, response_time, vary: entry.vary.clone() });
        if let Some(d) = self.use_disk(&e.url) {
            if !e.cache_control().no_store {
                d.store(&e);
            }
        }
        self.insert_mem(e.clone());
        e
    }

    /// Remove every stored variant of `key` (after unsafe methods).
    pub fn invalidate(&self, key: &str) {
        let mut mem = self.mem.lock().unwrap();
        if let Some(vs) = mem.map.remove(key) {
            let freed: usize = vs.iter().map(|(e, _)| e.size()).sum();
            mem.bytes = mem.bytes.saturating_sub(freed);
        }
        drop(mem);
        if let Some(d) = self.use_disk(key) {
            d.remove(key);
        }
    }

    /// Drop everything (memory and disk).
    pub fn clear(&self) {
        let mut mem = self.mem.lock().unwrap();
        mem.map.clear();
        mem.bytes = 0;
        drop(mem);
        if let Some(d) = &self.disk {
            if let Ok(dirs) = std::fs::read_dir(&d.dir) {
                for e in dirs.flatten() {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hv(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    fn http_date(t: u64) -> String {
        // Minimal RFC 1123 formatter for tests.
        let days = t / 86400;
        let secs = t % 86400;
        let (y, m, d) = civil_from_days(days as i64);
        let wd = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][(days % 7) as usize];
        let mn = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][(m - 1) as usize];
        format!("{wd}, {d:02} {mn} {y} {:02}:{:02}:{:02} GMT", secs / 3600, secs / 60 % 60, secs % 60)
    }

    fn civil_from_days(z: i64) -> (i64, u32, u32) {
        let z = z + 719468;
        let era = z.div_euclid(146097);
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        (if m <= 2 { y + 1 } else { y }, m, d)
    }

    #[test]
    fn cache_control_parsing() {
        let cc = CacheControl::parse(["max-age=60, public", "stale-while-revalidate=30, immutable", "max-age=10"].into_iter());
        assert_eq!(cc.max_age, Some(10));
        assert!(cc.public && cc.immutable);
        assert_eq!(cc.stale_while_revalidate, Some(30));
        let cc = CacheControl::parse(["no-store, No-Cache, private, must-revalidate"].into_iter());
        assert!(cc.no_store && cc.no_cache && cc.private && cc.must_revalidate);
        let cc = CacheControl::from_headers(&hv(&[("pragma", "no-cache")]));
        assert!(cc.no_cache);
    }

    #[test]
    fn freshness_and_lookup() {
        let c = HttpCache::memory(1 << 20);
        let now = now_secs();
        let k = "https://a.test/x";
        assert!(c.store(k, "GET", 200, &[], &hv(&[("cache-control", "max-age=100"), ("date", &http_date(now))]), b"x".to_vec(), now, now));
        assert!(matches!(c.lookup(k, &[], CacheMode::Default), Lookup::Fresh(_)));
        // Request max-age=0 forces revalidation; no validators -> empty list.
        match c.lookup(k, &hv(&[("cache-control", "max-age=0")]), CacheMode::Default) {
            Lookup::Revalidate(_, v) => assert!(v.is_empty()),
            other => panic!("{other:?}"),
        }
        // Old entry: stale.
        let k2 = "https://a.test/old";
        c.store(k2, "GET", 200, &[], &hv(&[("cache-control", "max-age=100"), ("etag", "\"v1\"")]), b"x".to_vec(), now - 500, now - 500);
        match c.lookup(k2, &[], CacheMode::Default) {
            Lookup::Revalidate(_, v) => assert_eq!(v, hv(&[("If-None-Match", "\"v1\"")])),
            other => panic!("{other:?}"),
        }
        // Expires in the past; Expires: 0.
        let k3 = "https://a.test/exp";
        c.store(k3, "GET", 200, &[], &hv(&[("expires", "0"), ("last-modified", &http_date(now - 1000))]), b"x".to_vec(), now, now);
        assert!(matches!(c.lookup(k3, &[], CacheMode::Default), Lookup::Revalidate(..)));
        // Heuristic freshness: 10% of 1000s = 100s.
        let k4 = "https://a.test/heur";
        c.store(k4, "GET", 200, &[], &hv(&[("date", &http_date(now)), ("last-modified", &http_date(now - 1000))]), b"x".to_vec(), now, now);
        assert!(matches!(c.lookup(k4, &[], CacheMode::Default), Lookup::Fresh(_)));
        // stale-while-revalidate window.
        let k5 = "https://a.test/swr";
        c.store(k5, "GET", 200, &[], &hv(&[("cache-control", "max-age=10, stale-while-revalidate=1000"), ("etag", "\"a\"")]), b"x".to_vec(), now - 100, now - 100);
        assert!(matches!(c.lookup(k5, &[], CacheMode::Default), Lookup::StaleWhileRevalidate(_)));
        // no-store / Vary: * / POST are never stored.
        assert!(!c.store("https://a.test/ns", "GET", 200, &[], &hv(&[("cache-control", "no-store, max-age=100")]), vec![], now, now));
        assert!(!c.store("https://a.test/v", "GET", 200, &[], &hv(&[("cache-control", "max-age=100"), ("vary", "*")]), vec![], now, now));
        assert!(!c.store("https://a.test/p", "POST", 200, &[], &hv(&[("cache-control", "max-age=100")]), vec![], now, now));
        // no-cache: stored but always revalidated.
        let k6 = "https://a.test/nc";
        assert!(c.store(k6, "GET", 200, &[], &hv(&[("cache-control", "no-cache"), ("etag", "\"z\"")]), b"x".to_vec(), now, now));
        assert!(matches!(c.lookup(k6, &[], CacheMode::Default), Lookup::Revalidate(..)));
        // Immutable: a reload (no-cache mode) still uses it while fresh.
        let k7 = "https://a.test/imm";
        c.store(k7, "GET", 200, &[], &hv(&[("cache-control", "max-age=1000, immutable")]), b"x".to_vec(), now, now);
        assert!(matches!(c.lookup(k7, &[], CacheMode::NoCache), Lookup::Fresh(_)));
        assert!(matches!(c.lookup(k, &[], CacheMode::NoCache), Lookup::Revalidate(..)));
        assert!(matches!(c.lookup(k, &[], CacheMode::Reload), Lookup::Miss));
        // Age header counts.
        let k8 = "https://a.test/aged";
        c.store(k8, "GET", 200, &[], &hv(&[("cache-control", "max-age=100"), ("age", "200")]), b"x".to_vec(), now, now);
        assert!(matches!(c.lookup(k8, &[], CacheMode::Default), Lookup::Revalidate(..)));
    }

    #[test]
    fn vary_and_freshen() {
        let c = HttpCache::memory(1 << 20);
        let now = now_secs();
        let k = "https://a.test/v";
        let resp = hv(&[("cache-control", "max-age=100"), ("vary", "Accept-Language"), ("etag", "\"1\""), ("set-cookie", "a=b")]);
        c.store(k, "GET", 200, &hv(&[("accept-language", "es")]), &resp, b"es".to_vec(), now, now);
        c.store(k, "GET", 200, &hv(&[("accept-language", "en")]), &resp, b"en".to_vec(), now, now);
        match c.lookup(k, &hv(&[("Accept-Language", "es")]), CacheMode::Default) {
            Lookup::Fresh(e) => {
                assert_eq!(*e.body, b"es");
                assert!(e.header("set-cookie").is_none());
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(c.lookup(k, &hv(&[("accept-language", "fr")]), CacheMode::Default), Lookup::Miss));
        let e = c.get(k, &hv(&[("accept-language", "en")])).unwrap();
        let f = c.freshen(&e, &hv(&[("etag", "\"2\""), ("cache-control", "max-age=5")]), now, now);
        assert_eq!(f.header("etag"), Some("\"2\""));
        assert_eq!(*f.body, b"en");
        c.invalidate(k);
        assert!(c.get(k, &hv(&[("accept-language", "en")])).is_none());
    }

    #[test]
    fn disk_roundtrip_and_eviction() {
        let dir = std::env::temp_dir().join(format!("rh-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let now = now_secs();
        let k = "https://example.org/a.css?x=1";
        {
            let c = HttpCache::with_disk(1 << 20, &dir, 1 << 20);
            assert!(c.store(k, "GET", 200, &[], &hv(&[("cache-control", "max-age=1000"), ("content-type", "text/css"), ("x-tab", "a\tb")]), b"body{}\n\x00binary".to_vec(), now, now));
        }
        // A fresh instance (new process) finds it on disk.
        let c = HttpCache::with_disk(1 << 20, &dir, 1 << 20);
        let e = c.get(k, &[]).expect("disk hit");
        assert_eq!(*e.body, b"body{}\n\x00binary");
        assert_eq!(e.header("content-type"), Some("text/css"));
        assert_eq!(e.header("x-tab"), Some("a\tb"));
        // Eviction keeps the directory under budget.
        let c = HttpCache::with_disk(1 << 10, &dir, 64 * 1024);
        for i in 0..40 {
            c.store(&format!("https://example.org/{i}"), "GET", 200, &[], &hv(&[("cache-control", "max-age=1000")]), vec![b'x'; 8 * 1024], now, now);
        }
        let mut total = 0;
        for d in std::fs::read_dir(&dir).unwrap().flatten() {
            for f in std::fs::read_dir(d.path()).unwrap().flatten() {
                total += f.metadata().unwrap().len();
            }
        }
        assert!(total <= 80 * 1024, "{total}");
        // Loopback responses are not persisted.
        let c = HttpCache::with_disk(1 << 20, &dir, 1 << 20);
        c.store("http://127.0.0.1:8080/x", "GET", 200, &[], &hv(&[("cache-control", "max-age=1000")]), b"x".to_vec(), now, now);
        let c2 = HttpCache::with_disk(1 << 20, &dir, 1 << 20);
        assert!(c2.get("http://127.0.0.1:8080/x", &[]).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
