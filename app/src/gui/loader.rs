//! Background navigation: one worker thread per request. The document (and
//! later its deferred script sources) go back to the tab's content thread;
//! downloads report their progress to the UI and can be paused and resumed
//! (HTTP byte ranges) or saved to a location the user picks.

use crate::gui::downloads::unique_path;
use rabbithole_engine::net::{Request, StreamedResponse, Url};
use rabbithole_engine::{Loaded, LoadedPage, Loader, ScriptSource, Viewport};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What a navigation produced.
pub enum LoadOutcome {
    Page(Box<LoadedPage>),
    /// The response became a download (events follow with its progress).
    Download,
}

/// What a load worker delivers to the content thread, in this order.
pub enum LoadResult {
    /// The document (with stylesheets and fonts), a download or an error.
    Loaded(Result<LoadOutcome, String>),
    /// The page's scripts with their sources, fetched after the document
    /// was delivered (only when the loader defers scripts).
    Scripts(Vec<ScriptSource>),
}

/// Pause / cancel switches of a running download.
#[derive(Debug, Clone, Default)]
pub struct DownloadControl {
    pub cancel: Arc<AtomicBool>,
    pub pause: Arc<AtomicBool>,
}

/// Download progress, for the UI's download list.
#[derive(Debug, Clone)]
pub enum DownloadEvent {
    /// Where should this file go? The loader waits for the answer
    /// (`None` = cancel).
    Ask { url: String, suggested: PathBuf, reply: Sender<Option<PathBuf>> },
    Started { id: u64, url: String, filename: String, path: PathBuf, total: Option<u64>, resumable: bool, control: DownloadControl },
    Progress { id: u64, done: u64, total: Option<u64> },
    /// Stopped by a pause request; `done` bytes are on disk.
    Paused { id: u64, done: u64 },
    Done { id: u64, error: Option<String> },
}

/// Where downloads go.
#[derive(Debug, Clone)]
pub struct DownloadPrefs {
    pub dir: PathBuf,
    /// Ask where to save each file.
    pub ask: bool,
    pub next_id: Arc<AtomicU64>,
}

/// Parameters of one navigation.
pub struct LoadJob {
    pub loader: Loader,
    pub request: Request,
    pub viewport: Viewport,
    pub downloads: DownloadPrefs,
}

/// Run the job on a new thread.
pub fn spawn(job: LoadJob, deliver: impl Fn(LoadResult) + Send + 'static, downloads: impl Fn(DownloadEvent) + Send + Sync + 'static) {
    std::thread::Builder::new()
        .name("rabbithole-load".into())
        .spawn(move || run(job, deliver, downloads))
        .map(|_| ())
        .unwrap_or_else(|e| eprintln!("rabbithole: could not spawn loader thread: {e}"));
}

fn run(job: LoadJob, deliver: impl Fn(LoadResult), downloads: impl Fn(DownloadEvent)) {
    let result = job.loader.load_request(&job.request, job.viewport);
    let mut deferred: Option<(Url, Vec<ScriptSource>)> = None;
    let (outcome, download) = match result {
        Ok(Loaded::Page(p)) => {
            if job.loader.defer_scripts {
                deferred = Some((p.url.clone(), p.scripts.clone()));
            }
            (Ok(LoadOutcome::Page(Box::new(p))), None)
        }
        Ok(Loaded::Download(stream)) => (Ok(LoadOutcome::Download), Some(stream)),
        Err(e) => (Err(e.to_string()), None),
    };
    deliver(LoadResult::Loaded(outcome));
    if let Some((page_url, mut scripts)) = deferred {
        job.loader.fetch_script_sources(&page_url, &mut scripts);
        deliver(LoadResult::Scripts(scripts));
    }
    if let Some(stream) = download {
        let id = job.downloads.next_id.fetch_add(1, Ordering::SeqCst);
        save_stream(stream, id, None, &job.downloads, &downloads);
    }
}

/// Download `request` directly (save link / image as, retry), on a new
/// thread.
pub fn spawn_download(loader: Loader, request: Request, prefs: DownloadPrefs, events: impl Fn(DownloadEvent) + Send + Sync + 'static) -> u64 {
    let id = prefs.next_id.fetch_add(1, Ordering::SeqCst);
    let _ = std::thread::Builder::new().name("rabbithole-download".into()).spawn(move || match loader.fetcher.request_stream(&request) {
        Ok(stream) if stream.status < 400 => save_stream(stream, id, None, &prefs, &events),
        Ok(stream) => {
            events(DownloadEvent::Started {
                id,
                url: request.url.to_string(),
                filename: stream.suggested_filename(),
                path: prefs.dir.join(stream.suggested_filename()),
                total: None,
                resumable: false,
                control: DownloadControl::default(),
            });
            events(DownloadEvent::Done { id, error: Some(format!("HTTP error {}", stream.status)) });
        }
        Err(e) => {
            let name = request.url.path.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("download").to_string();
            events(DownloadEvent::Started { id, url: request.url.to_string(), filename: name.clone(), path: prefs.dir.join(name), total: None, resumable: false, control: DownloadControl::default() });
            events(DownloadEvent::Done { id, error: Some(e.to_string()) });
        }
    });
    id
}

/// Continue a paused download at `path` from its current size with a
/// `Range` request (starts over when the server ignores the range).
pub fn spawn_resume(loader: Loader, url: Url, path: PathBuf, id: u64, control: DownloadControl, events: impl Fn(DownloadEvent) + Send + Sync + 'static) {
    let _ = std::thread::Builder::new().name("rabbithole-download".into()).spawn(move || {
        let offset = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let req = Request::get(url).with_header("Range", &format!("bytes={offset}-")).with_cache_mode(rabbithole_engine::net::CacheMode::NoStore);
        match loader.fetcher.request_stream(&req) {
            Ok(stream) if stream.status == 206 || stream.status == 200 => {
                let append = stream.status == 206 && content_range_start(stream.header("content-range")) == Some(offset);
                let start = if append { offset } else { 0 };
                let total = stream.content_length.map(|l| l + start);
                write_body(stream, &path, start, total, id, &control, &events);
            }
            Ok(stream) => events(DownloadEvent::Done { id, error: Some(format!("HTTP error {}", stream.status)) }),
            Err(e) => events(DownloadEvent::Done { id, error: Some(e.to_string()) }),
        }
    });
}

/// First byte of a `Content-Range: bytes a-b/c` header.
pub fn content_range_start(h: Option<&str>) -> Option<u64> {
    let v = h?.trim().strip_prefix("bytes")?.trim();
    v.split('-').next()?.trim().parse().ok()
}

/// Can this response be resumed with a range request?
pub fn is_resumable(stream: &StreamedResponse) -> bool {
    stream.header("accept-ranges").map(|v| v.to_ascii_lowercase().contains("bytes")).unwrap_or(false) && stream.content_length.is_some() && stream.header("content-encoding").is_none_or(|e| e.trim().eq_ignore_ascii_case("identity"))
}

fn save_stream(stream: StreamedResponse, id: u64, path: Option<PathBuf>, prefs: &DownloadPrefs, events: &dyn Fn(DownloadEvent)) {
    let filename = stream.suggested_filename();
    let _ = std::fs::create_dir_all(&prefs.dir);
    let mut path = path.unwrap_or_else(|| unique_path(&prefs.dir, &filename));
    if prefs.ask {
        let (tx, rx) = std::sync::mpsc::channel();
        events(DownloadEvent::Ask { url: stream.url.to_string(), suggested: path.clone(), reply: tx });
        match rx.recv_timeout(Duration::from_secs(1800)) {
            Ok(Some(p)) => path = p,
            _ => return, // cancelled (nothing was listed yet)
        }
    }
    let control = DownloadControl::default();
    let total = stream.content_length;
    events(DownloadEvent::Started {
        id,
        url: stream.url.to_string(),
        filename: path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or(filename),
        path: path.clone(),
        total,
        resumable: is_resumable(&stream),
        control: control.clone(),
    });
    write_body(stream, &path, 0, total, id, &control, events);
}

/// Stream the body into `path` (appending after `start` bytes), with
/// progress, pause and cancel.
fn write_body(mut stream: StreamedResponse, path: &PathBuf, start: u64, total: Option<u64>, id: u64, control: &DownloadControl, events: &dyn Fn(DownloadEvent)) {
    let file = if start > 0 { std::fs::OpenOptions::new().append(true).open(path) } else { std::fs::File::create(path) };
    let mut file = match file {
        Ok(f) => f,
        Err(e) => return events(DownloadEvent::Done { id, error: Some(format!("cannot write {}: {e}", path.display())) }),
    };
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = start;
    let mut last = Instant::now();
    let mut last_done = done;
    loop {
        if control.cancel.load(Ordering::SeqCst) {
            drop(file);
            let _ = std::fs::remove_file(path);
            return events(DownloadEvent::Done { id, error: Some("cancelled".into()) });
        }
        if control.pause.load(Ordering::SeqCst) {
            let _ = file.flush();
            return events(DownloadEvent::Paused { id, done });
        }
        let n = match stream.reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                let _ = file.flush();
                return events(DownloadEvent::Done { id, error: Some(e.to_string()) });
            }
        };
        if let Err(e) = file.write_all(&buf[..n]) {
            return events(DownloadEvent::Done { id, error: Some(e.to_string()) });
        }
        done += n as u64;
        if last.elapsed() >= Duration::from_millis(100) || done - last_done >= 512 * 1024 {
            last = Instant::now();
            last_done = done;
            events(DownloadEvent::Progress { id, done, total });
        }
    }
    let _ = file.flush();
    events(DownloadEvent::Progress { id, done, total: total.or(Some(done)) });
    events(DownloadEvent::Done { id, error: None });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range() {
        assert_eq!(content_range_start(Some("bytes 100-999/1000")), Some(100));
        assert_eq!(content_range_start(Some("bytes */1000")), None);
        assert_eq!(content_range_start(None), None);
    }

    /// A local HTTP server that honours `Range`: pause after the first
    /// chunk, resume, and get the exact bytes.
    #[test]
    fn pause_and_resume_with_ranges() {
        use std::net::TcpListener;
        let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let served = body.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming().take(2) {
                let mut s = conn.unwrap();
                let mut req = Vec::new();
                let mut b = [0u8; 1];
                while !req.ends_with(b"\r\n\r\n") {
                    if s.read(&mut b).unwrap_or(0) == 0 {
                        break;
                    }
                    req.push(b[0]);
                }
                let text = String::from_utf8_lossy(&req).to_ascii_lowercase();
                let start = text.lines().find_map(|l| l.strip_prefix("range: bytes=")).and_then(|r| r.trim_end_matches('-').trim().trim_end_matches('-').parse::<usize>().ok());
                let (status, part) = match start {
                    Some(st) => (format!("206 Partial Content\r\nContent-Range: bytes {st}-{}/{}", served.len() - 1, served.len()), &served[st..]),
                    None => ("200 OK".to_string(), &served[..]),
                };
                let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n", part.len());
                let _ = s.write_all(head.as_bytes());
                // Slow first response so the pause lands mid-way.
                for chunk in part.chunks(16 * 1024) {
                    if s.write_all(chunk).is_err() {
                        break;
                    }
                    if start.is_none() {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
            }
        });
        let dir = std::env::temp_dir().join(format!("rh-dl-resume-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut loader = Loader::default();
        loader.fetcher = rabbithole_engine::net::Fetcher::isolated();
        let prefs = DownloadPrefs { dir: dir.clone(), ask: false, next_id: Arc::new(AtomicU64::new(1)) };
        let (tx, rx) = std::sync::mpsc::channel::<DownloadEvent>();
        let url = Url::parse(&format!("http://127.0.0.1:{port}/data.bin")).unwrap();
        let tx1 = tx.clone();
        spawn_download(loader.clone(), Request::get(url.clone()), prefs, move |e| {
            let _ = tx1.send(e);
        });
        let mut control = None;
        let mut path = None;
        let mut paused_at = None;
        while let Ok(e) = rx.recv_timeout(Duration::from_secs(10)) {
            match e {
                DownloadEvent::Started { control: c, path: p, resumable, .. } => {
                    assert!(resumable);
                    c.pause.store(true, Ordering::SeqCst);
                    control = Some(c);
                    path = Some(p);
                }
                DownloadEvent::Paused { done, .. } => {
                    paused_at = Some(done);
                    break;
                }
                DownloadEvent::Done { error, .. } => panic!("finished before pausing: {error:?}"),
                _ => {}
            }
        }
        let paused_at = paused_at.expect("paused");
        assert!(paused_at < body.len() as u64);
        let control = control.unwrap();
        control.pause.store(false, Ordering::SeqCst);
        let path = path.unwrap();
        spawn_resume(loader, url, path.clone(), 1, control, move |e| {
            let _ = tx.send(e);
        });
        loop {
            match rx.recv_timeout(Duration::from_secs(10)).expect("resume events") {
                DownloadEvent::Done { error, .. } => {
                    assert_eq!(error, None);
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(std::fs::read(&path).unwrap(), body);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
