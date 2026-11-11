//! Byte sources for direct media files: a resource downloaded on a
//! background thread into a sparse in-memory buffer, re-requested with
//! HTTP range requests when playback seeks outside what has arrived, and
//! read through a blocking `Read + Seek` view for the demuxers.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// A response body opened at some byte offset.
pub struct FetchStream {
    pub reader: Box<dyn Read + Send>,
    /// Total resource length, when known.
    pub total: Option<u64>,
    /// Offset of the first body byte (0 unless the range was honoured).
    pub offset: u64,
    /// `Content-Type` (MIME type only).
    pub content_type: String,
}

/// Opens a URL from a byte offset (HTTP `Range: bytes=<start>-`), with the
/// page's cookies and headers. Implemented by the embedder.
pub trait ByteFetcher: Send + Sync {
    fn open(&self, url: &str, start: u64) -> Result<FetchStream, String>;
}

/// The largest resource kept in memory.
const MAX_BYTES: u64 = 1 << 30;
/// A read this far past the download position restarts the download
/// there instead of waiting.
const JUMP: u64 = 512 * 1024;

#[derive(Default)]
struct State {
    data: Vec<u8>,
    /// Downloaded byte ranges (sorted, disjoint, half-open).
    have: Vec<(u64, u64)>,
    total: Option<u64>,
    /// Where the downloader is writing now.
    dl_pos: u64,
    /// The reader wants the download to restart here.
    restart: Option<u64>,
    /// Everything was downloaded.
    complete: bool,
    /// The server ignores ranges (whole-file streaming only).
    no_ranges: bool,
    error: Option<String>,
    content_type: String,
    closed: bool,
}

pub(crate) struct Shared {
    st: Mutex<State>,
    cv: Condvar,
    /// Set to abort a blocked read (the player has a command to handle).
    pub(crate) interrupt: AtomicBool,
}

/// A downloading resource; clones share the download.
#[derive(Clone)]
pub struct RangedSource {
    shared: Arc<Shared>,
}

impl RangedSource {
    /// Start downloading `url` (blocks until the response headers arrive).
    pub fn open(fetcher: Arc<dyn ByteFetcher>, url: &str) -> Result<RangedSource, String> {
        let first = fetcher.open(url, 0)?;
        let total = first.total;
        if total.is_some_and(|t| t > MAX_BYTES) {
            return Err("media resource too large".into());
        }
        let shared = Arc::new(Shared {
            st: Mutex::new(State { total, content_type: first.content_type.clone(), data: Vec::with_capacity(total.unwrap_or(1 << 20) as usize), ..Default::default() }),
            cv: Condvar::new(),
            interrupt: AtomicBool::new(false),
        });
        let s2 = shared.clone();
        let url = url.to_string();
        std::thread::Builder::new().name("rh-media-net".into()).spawn(move || download(s2, fetcher, url, first)).map_err(|e| e.to_string())?;
        Ok(RangedSource { shared })
    }

    /// An in-memory resource (blob: / data: URLs, tests).
    pub fn from_bytes(bytes: Vec<u8>, content_type: &str) -> RangedSource {
        let n = bytes.len() as u64;
        RangedSource {
            shared: Arc::new(Shared {
                st: Mutex::new(State { have: vec![(0, n)], total: Some(n), complete: true, dl_pos: n, data: bytes, content_type: content_type.to_string(), ..Default::default() }),
                cv: Condvar::new(),
                interrupt: AtomicBool::new(false),
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.shared.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn content_type(&self) -> String {
        self.lock().content_type.clone()
    }

    pub fn total(&self) -> Option<u64> {
        self.lock().total
    }

    /// Downloaded byte ranges.
    pub fn buffered_bytes(&self) -> Vec<(u64, u64)> {
        self.lock().have.clone()
    }

    pub fn complete(&self) -> bool {
        self.lock().complete
    }

    pub fn error(&self) -> Option<String> {
        self.lock().error.clone()
    }

    /// A reader positioned at 0.
    pub fn reader(&self) -> SourceReader {
        SourceReader { src: self.clone(), pos: 0 }
    }

    /// Stop downloading (the element dropped the resource).
    pub fn close(&self) {
        self.lock().closed = true;
        self.shared.cv.notify_all();
    }

    /// Wake a blocked reader (it returns an "interrupted" error).
    pub fn interrupt(&self) {
        self.shared.interrupt.store(true, Ordering::SeqCst);
        self.shared.cv.notify_all();
    }

    pub fn clear_interrupt(&self) {
        self.shared.interrupt.store(false, Ordering::SeqCst);
    }
}

fn has(have: &[(u64, u64)], pos: u64) -> Option<u64> {
    have.iter().find(|(s, e)| *s <= pos && pos < *e).map(|(_, e)| *e)
}

fn add_range(have: &mut Vec<(u64, u64)>, s: u64, e: u64) {
    if s >= e {
        return;
    }
    have.push((s, e));
    have.sort_unstable();
    let mut out: Vec<(u64, u64)> = Vec::with_capacity(have.len());
    for &(a, b) in have.iter() {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    *have = out;
}

/// First byte not yet downloaded at or after `from` (wrapping to 0).
fn next_hole(st: &State, from: u64) -> Option<u64> {
    let total = st.total?;
    let mut p = from;
    for _ in 0..2 {
        loop {
            if p >= total {
                break;
            }
            match has(&st.have, p) {
                Some(e) => p = e,
                None => return Some(p),
            }
        }
        p = 0;
    }
    None
}

fn download(shared: Arc<Shared>, fetcher: Arc<dyn ByteFetcher>, url: String, first: FetchStream) {
    let mut stream = Some(first);
    let mut pos = 0u64;
    let mut buf = vec![0u8; 64 * 1024];
    let mut failures = 0;
    loop {
        let mut requested = 0u64;
        let mut s = match stream.take() {
            Some(s) => s,
            None => {
                let start = {
                    let st = shared.st.lock().unwrap_or_else(|e| e.into_inner());
                    if st.closed {
                        return;
                    }
                    match next_hole(&st, pos) {
                        Some(p) => p,
                        None => return,
                    }
                };
                requested = start;
                match fetcher.open(&url, start) {
                    Ok(s) => s,
                    Err(e) => {
                        failures += 1;
                        if failures > 3 {
                            let mut st = shared.st.lock().unwrap_or_else(|e| e.into_inner());
                            st.error = Some(e);
                            shared.cv.notify_all();
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(300 * failures));
                        continue;
                    }
                }
            }
        };
        pos = s.offset;
        {
            let mut st = shared.st.lock().unwrap_or_else(|e| e.into_inner());
            if s.offset != requested {
                // Asked for a range but got the whole file again.
                st.no_ranges = true;
            }
            if st.total.is_none() {
                st.total = s.total.map(|t| t + s.offset);
            }
            st.dl_pos = pos;
        }
        loop {
            let n = match s.reader.read(&mut buf) {
                Ok(0) => {
                    let mut st = shared.st.lock().unwrap_or_else(|e| e.into_inner());
                    if st.total.is_none() {
                        // Length learnt at the end of a streamed body.
                        st.total = Some(pos);
                    }
                    break;
                }
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    failures += 1;
                    if failures > 3 {
                        let mut st = shared.st.lock().unwrap_or_else(|e| e.into_inner());
                        st.error = Some(e.to_string());
                        shared.cv.notify_all();
                        return;
                    }
                    break;
                }
            };
            let mut st = shared.st.lock().unwrap_or_else(|e| e.into_inner());
            if st.closed {
                return;
            }
            let end = pos + n as u64;
            if end > MAX_BYTES {
                st.error = Some("media resource too large".into());
                shared.cv.notify_all();
                return;
            }
            if st.data.len() < end as usize {
                st.data.resize(end as usize, 0);
            }
            st.data[pos as usize..end as usize].copy_from_slice(&buf[..n]);
            add_range(&mut st.have, pos, end);
            pos = end;
            st.dl_pos = pos;
            shared.cv.notify_all();
            // A reader waiting far from here: restart there.
            if let Some(r) = st.restart.take() {
                if !st.no_ranges && has(&st.have, r).is_none() && (r < pos || r > pos + JUMP) {
                    pos = r;
                    break;
                }
            }
            // Ran into data we already have: continue at the next hole.
            if has(&st.have, pos).is_some() && !st.no_ranges {
                break;
            }
        }
        let mut st = shared.st.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(total) = st.total {
            if next_hole(&st, 0).is_none() || st.have.first().is_some_and(|&(s, e)| s == 0 && e >= total) {
                st.complete = true;
                shared.cv.notify_all();
                return;
            }
        } else {
            st.complete = true;
            shared.cv.notify_all();
            return;
        }
        if st.no_ranges && pos >= st.total.unwrap_or(0) {
            st.complete = true;
            shared.cv.notify_all();
            return;
        }
    }
}

/// Blocking `Read + Seek` over a [`RangedSource`].
pub struct SourceReader {
    src: RangedSource,
    pos: u64,
}

impl Read for SourceReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let shared = self.src.shared.clone();
        let mut st = shared.st.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(total) = st.total {
                if self.pos >= total {
                    return Ok(0);
                }
            }
            if let Some(end) = has(&st.have, self.pos) {
                let n = ((end - self.pos) as usize).min(out.len());
                out[..n].copy_from_slice(&st.data[self.pos as usize..self.pos as usize + n]);
                self.pos += n as u64;
                return Ok(n);
            }
            if st.complete && st.total.is_none_or(|t| self.pos >= t) {
                return Ok(0);
            }
            if let Some(e) = &st.error {
                return Err(io::Error::other(e.clone()));
            }
            if st.closed {
                return Err(io::Error::other("closed"));
            }
            if shared.interrupt.load(Ordering::SeqCst) {
                return Err(io::Error::other("interrupted"));
            }
            if self.pos < st.dl_pos || self.pos > st.dl_pos + JUMP {
                st.restart = Some(self.pos);
            }
            st = shared.cv.wait_timeout(st, Duration::from_millis(100)).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

impl Seek for SourceReader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let total = self.src.total();
        let p = match to {
            SeekFrom::Start(p) => p as i64,
            SeekFrom::Current(d) => self.pos as i64 + d,
            SeekFrom::End(d) => match total {
                Some(t) => t as i64 + d,
                None => return Err(io::Error::other("unknown length")),
            },
        };
        if p < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek before start"));
        }
        self.pos = p as u64;
        Ok(self.pos)
    }
}

impl symphonia::core::io::MediaSource for SourceReader {
    fn is_seekable(&self) -> bool {
        self.src.total().is_some()
    }
    fn byte_len(&self) -> Option<u64> {
        self.src.total()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serves an in-memory file honouring ranges, slowly.
    struct Fake {
        data: Arc<Vec<u8>>,
        ranges: bool,
    }

    struct Slow {
        data: Arc<Vec<u8>>,
        pos: usize,
    }

    impl Read for Slow {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = buf.len().min(4096).min(self.data.len() - self.pos);
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            std::thread::sleep(Duration::from_micros(200));
            Ok(n)
        }
    }

    impl ByteFetcher for Fake {
        fn open(&self, _url: &str, start: u64) -> Result<FetchStream, String> {
            let start = if self.ranges { start as usize } else { 0 };
            Ok(FetchStream { reader: Box::new(Slow { data: self.data.clone(), pos: start }), total: Some((self.data.len() - start) as u64), offset: start as u64, content_type: "audio/wav".into() })
        }
    }

    #[test]
    fn sequential_and_seeking_reads() {
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        let data = Arc::new(data);
        for ranges in [true, false] {
            let src = RangedSource::open(Arc::new(Fake { data: data.clone(), ranges }), "x").unwrap();
            let mut r = src.reader();
            let mut b = [0u8; 100];
            r.read_exact(&mut b).unwrap();
            assert_eq!(&b[..], &data[..100]);
            // Far seek: served by a range request (or by waiting).
            r.seek(SeekFrom::Start(2_500_000)).unwrap();
            r.read_exact(&mut b).unwrap();
            assert_eq!(&b[..], &data[2_500_000..2_500_100]);
            r.seek(SeekFrom::End(-10)).unwrap();
            let mut t = Vec::new();
            r.read_to_end(&mut t).unwrap();
            assert_eq!(&t[..], &data[data.len() - 10..]);
            // Eventually everything arrives.
            let t0 = std::time::Instant::now();
            while !src.complete() && t0.elapsed() < Duration::from_secs(30) {
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(src.complete());
            assert_eq!(src.buffered_bytes(), vec![(0, data.len() as u64)]);
        }
    }
}
