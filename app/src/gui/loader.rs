//! Background navigation: one worker thread per request. The document (and
//! later its deferred script sources) go back to the tab's content thread;
//! downloads report their progress to the UI.

use crate::gui::downloads::unique_path;
use rabbithole_engine::net::Request;
use rabbithole_engine::{Loaded, LoadedPage, Loader, ScriptSource, Viewport};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// Download progress, for the UI's download list.
#[derive(Debug, Clone)]
pub enum DownloadEvent {
    Started { id: u64, url: String, filename: String, path: PathBuf, total: Option<u64>, cancel: Arc<AtomicBool> },
    Progress { id: u64, done: u64, total: Option<u64> },
    Done { id: u64, error: Option<String> },
}

/// Parameters of one navigation.
pub struct LoadJob {
    pub loader: Loader,
    pub request: Request,
    pub viewport: Viewport,
    pub download_dir: PathBuf,
    pub next_download_id: Arc<AtomicU64>,
}

/// Run the job on a new thread.
pub fn spawn(job: LoadJob, deliver: impl Fn(LoadResult) + Send + 'static, downloads: impl Fn(DownloadEvent) + Send + 'static) {
    std::thread::Builder::new()
        .name("rabbithole-load".into())
        .spawn(move || run(job, deliver, downloads))
        .map(|_| ())
        .unwrap_or_else(|e| eprintln!("rabbithole: could not spawn loader thread: {e}"));
}

fn run(job: LoadJob, deliver: impl Fn(LoadResult), downloads: impl Fn(DownloadEvent)) {
    let result = job.loader.load_request(&job.request, job.viewport);
    let mut deferred: Option<(rabbithole_engine::net::Url, Vec<ScriptSource>)> = None;
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
    let Some(stream) = download else { return };

    let id = job.next_download_id.fetch_add(1, Ordering::SeqCst);
    let filename = stream.suggested_filename();
    let path = unique_path(&job.download_dir, &filename);
    let cancel = Arc::new(AtomicBool::new(false));
    let total = stream.content_length;
    downloads(DownloadEvent::Started {
        id,
        url: stream.url.to_string(),
        filename: path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or(filename),
        path: path.clone(),
        total,
        cancel: cancel.clone(),
    });
    let mut last = Instant::now();
    let mut last_done = 0u64;
    let cancel2 = cancel.clone();
    let result = stream.save_to(&path, |done, total| {
        if cancel2.load(Ordering::SeqCst) {
            return false;
        }
        if last.elapsed() >= Duration::from_millis(100) || done - last_done >= 512 * 1024 {
            last = Instant::now();
            last_done = done;
            downloads(DownloadEvent::Progress { id, done, total });
        }
        true
    });
    let error = match result {
        Ok(done) => {
            downloads(DownloadEvent::Progress { id, done, total: total.or(Some(done)) });
            None
        }
        Err(e) => Some(e.to_string()),
    };
    downloads(DownloadEvent::Done { id, error });
}
