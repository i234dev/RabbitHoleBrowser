//! Background navigation: one worker thread per request; results come back
//! through the winit event loop proxy as `UserEvent`s.

use crate::gui::downloads::unique_path;
use rabbithole_engine::net::{Request, Url};
use rabbithole_engine::{Loaded, LoadedPage, Loader, Viewport};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::event_loop::EventLoopProxy;

/// What a navigation produced.
pub enum LoadOutcome {
    Page(Box<LoadedPage>),
    /// The response became a download (events follow with its progress).
    Download,
}

/// Events sent from worker threads to the UI thread.
pub enum UserEvent {
    Loaded {
        tab_id: u64,
        generation: u64,
        request_url: Url,
        push_history: bool,
        outcome: Result<LoadOutcome, String>,
    },
    DownloadStarted {
        id: u64,
        url: String,
        filename: String,
        path: PathBuf,
        total: Option<u64>,
        cancel: Arc<AtomicBool>,
    },
    DownloadProgress {
        id: u64,
        done: u64,
        total: Option<u64>,
    },
    DownloadDone {
        id: u64,
        error: Option<String>,
    },
}

/// Parameters of one navigation.
pub struct LoadJob {
    pub loader: Loader,
    pub request: Request,
    pub viewport: Viewport,
    pub tab_id: u64,
    pub generation: u64,
    pub push_history: bool,
    pub download_dir: PathBuf,
    pub next_download_id: Arc<AtomicU64>,
}

/// Run the job on a new thread.
pub fn spawn(job: LoadJob, proxy: EventLoopProxy<UserEvent>) {
    std::thread::Builder::new()
        .name("rabbithole-load".into())
        .spawn(move || run(job, proxy))
        .map(|_| ())
        .unwrap_or_else(|e| eprintln!("rabbithole: could not spawn loader thread: {e}"));
}

fn run(job: LoadJob, proxy: EventLoopProxy<UserEvent>) {
    let request_url = job.request.url.clone();
    let result = job.loader.load_request(&job.request, job.viewport);
    let (outcome, download) = match result {
        Ok(Loaded::Page(p)) => (Ok(LoadOutcome::Page(Box::new(p))), None),
        Ok(Loaded::Download(stream)) => (Ok(LoadOutcome::Download), Some(stream)),
        Err(e) => (Err(e.to_string()), None),
    };
    let _ = proxy.send_event(UserEvent::Loaded {
        tab_id: job.tab_id,
        generation: job.generation,
        request_url: request_url.clone(),
        push_history: job.push_history,
        outcome,
    });
    let Some(stream) = download else { return };

    let id = job.next_download_id.fetch_add(1, Ordering::SeqCst);
    let filename = stream.suggested_filename();
    let path = unique_path(&job.download_dir, &filename);
    let cancel = Arc::new(AtomicBool::new(false));
    let total = stream.content_length;
    let _ = proxy.send_event(UserEvent::DownloadStarted {
        id,
        url: stream.url.to_string(),
        filename: path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or(filename),
        path: path.clone(),
        total,
        cancel: cancel.clone(),
    });
    let mut last = Instant::now();
    let mut last_done = 0u64;
    let proxy2 = proxy.clone();
    let cancel2 = cancel.clone();
    let result = stream.save_to(&path, move |done, total| {
        if cancel2.load(Ordering::SeqCst) {
            return false;
        }
        if last.elapsed() >= Duration::from_millis(100) || done - last_done >= 512 * 1024 {
            last = Instant::now();
            last_done = done;
            let _ = proxy2.send_event(UserEvent::DownloadProgress { id, done, total });
        }
        true
    });
    let error = match result {
        Ok(done) => {
            let _ = proxy.send_event(UserEvent::DownloadProgress { id, done, total: total.or(Some(done)) });
            None
        }
        Err(e) => Some(e.to_string()),
    };
    let _ = proxy.send_event(UserEvent::DownloadDone { id, error });
}
