//! Download bookkeeping on the UI thread.

use crate::gui::loader::DownloadControl;
use crate::gui::pages::DownloadRow;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq)]
pub enum DownloadState {
    InProgress,
    Paused,
    Done,
    Failed(String),
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct DownloadItem {
    pub id: u64,
    pub url: String,
    pub filename: String,
    pub path: PathBuf,
    pub done: u64,
    pub total: Option<u64>,
    pub state: DownloadState,
    pub control: DownloadControl,
    pub resumable: bool,
    /// (time, bytes) of the last speed sample, and the smoothed speed.
    pub sample: Option<(Instant, u64)>,
    pub speed: Option<u64>,
}

impl DownloadItem {
    pub fn new(id: u64, url: String, filename: String, path: PathBuf, total: Option<u64>, resumable: bool, control: DownloadControl) -> DownloadItem {
        DownloadItem { id, url, filename, path, done: 0, total, state: DownloadState::InProgress, control, resumable, sample: None, speed: None }
    }

    /// Record progress and update the speed estimate.
    pub fn progress(&mut self, done: u64, now: Instant) {
        if let Some((t, b)) = self.sample {
            let dt = now.duration_since(t).as_secs_f64();
            if dt >= 0.25 {
                let inst = (done.saturating_sub(b)) as f64 / dt;
                let s = match self.speed {
                    Some(old) => old as f64 * 0.6 + inst * 0.4,
                    None => inst,
                };
                self.speed = Some(s as u64);
                self.sample = Some((now, done));
            }
        } else {
            self.sample = Some((now, done));
        }
        self.done = done;
    }
}

#[derive(Debug, Default)]
pub struct Downloads {
    pub items: Vec<DownloadItem>,
}

impl Downloads {
    pub fn get_mut(&mut self, id: u64) -> Option<&mut DownloadItem> {
        self.items.iter_mut().find(|d| d.id == id)
    }

    pub fn get(&self, id: u64) -> Option<&DownloadItem> {
        self.items.iter().find(|d| d.id == id)
    }

    pub fn active(&self) -> impl Iterator<Item = &DownloadItem> {
        self.items.iter().filter(|d| d.state == DownloadState::InProgress)
    }

    pub fn cancel(&mut self, id: u64) {
        if let Some(d) = self.get_mut(id) {
            d.control.cancel.store(true, Ordering::SeqCst);
            if d.state == DownloadState::Paused {
                let _ = std::fs::remove_file(&d.path);
                d.state = DownloadState::Cancelled;
            }
        }
    }

    pub fn pause(&mut self, id: u64) {
        if let Some(d) = self.get_mut(id) {
            if d.state == DownloadState::InProgress && d.resumable {
                d.control.pause.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Remove finished, failed and cancelled entries.
    pub fn clear_finished(&mut self) {
        self.items.retain(|d| matches!(d.state, DownloadState::InProgress | DownloadState::Paused));
    }

    pub fn remove(&mut self, id: u64) {
        self.items.retain(|d| d.id != id || matches!(d.state, DownloadState::InProgress | DownloadState::Paused));
    }

    pub fn rows(&self) -> Vec<DownloadRow> {
        self.items
            .iter()
            .map(|d| DownloadRow {
                id: d.id,
                url: d.url.clone(),
                filename: d.filename.clone(),
                path: d.path.display().to_string(),
                done: d.done,
                total: d.total,
                state: match &d.state {
                    DownloadState::InProgress => "Downloading".into(),
                    DownloadState::Paused => "Paused".into(),
                    DownloadState::Done => "Completed".into(),
                    DownloadState::Failed(e) => format!("Failed: {e}"),
                    DownloadState::Cancelled => "Cancelled".into(),
                },
                active: d.state == DownloadState::InProgress,
                paused: d.state == DownloadState::Paused,
                resumable: d.resumable,
                finished: d.state == DownloadState::Done,
                failed: matches!(d.state, DownloadState::Failed(_) | DownloadState::Cancelled),
                speed: d.speed,
            })
            .collect()
    }

    /// Status-bar summary of active downloads.
    pub fn status_text(&self) -> Option<String> {
        let active: Vec<&DownloadItem> = self.active().collect();
        let first = active.first()?;
        let progress = match first.total {
            Some(t) if t > 0 => format!("{:.0}%", first.done as f64 / t as f64 * 100.0),
            _ => crate::gui::pages::human_size(first.done),
        };
        let more = if active.len() > 1 { format!(" (+{} more)", active.len() - 1) } else { String::new() };
        Some(format!("Downloading {} — {}{}", first.filename, progress, more))
    }

    /// Overall progress (0..1) of active downloads with a known size.
    pub fn overall_progress(&self) -> Option<f32> {
        let (mut done, mut total) = (0u64, 0u64);
        for d in self.active() {
            if let Some(t) = d.total.filter(|t| *t > 0) {
                done += d.done.min(t);
                total += t;
            }
        }
        (total > 0).then(|| done as f32 / total as f32)
    }
}

/// A path in `dir` for `filename` that does not exist yet (` (1)`, ` (2)`
/// suffixes before the extension).
pub fn unique_path(dir: &Path, filename: &str) -> PathBuf {
    let candidate = dir.join(filename);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match filename.rfind('.') {
        Some(i) if i > 0 => (&filename[..i], &filename[i..]),
        _ => (filename, ""),
    };
    for n in 1..10_000 {
        let p = dir.join(format!("{stem} ({n}){ext}"));
        if !p.exists() {
            return p;
        }
    }
    dir.join(format!("{stem}-{}{ext}", crate::profile::now_secs()))
}

/// Open a file or folder with the system's default application.
pub fn open_with_system(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("explorer");
        c.arg(path);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(path);
        c
    };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path);
        c
    };
    cmd.spawn().map(|_| ()).map_err(|e| format!("could not open {}: {e}", path.display()))
}

/// Show a file in its folder (selected where the platform supports it).
pub fn show_in_folder(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(path.as_os_str());
        return std::process::Command::new("explorer").arg(arg).spawn().map(|_| ()).map_err(|e| e.to_string());
    }
    #[cfg(not(target_os = "windows"))]
    {
        open_with_system(path.parent().unwrap_or(Path::new(".")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_names() {
        let dir = std::env::temp_dir().join(format!("rh-dl-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let first = unique_path(&dir, "file.tar.gz");
        assert_eq!(first.file_name().unwrap(), "file.tar.gz");
        std::fs::write(&first, b"x").unwrap();
        let second = unique_path(&dir, "file.tar.gz");
        assert_eq!(second.file_name().unwrap(), "file.tar (1).gz");
        std::fs::write(&second, b"x").unwrap();
        let third = unique_path(&dir, "file.tar.gz");
        assert_eq!(third.file_name().unwrap(), "file.tar (2).gz");
        let noext = unique_path(&dir, "README");
        assert_eq!(noext.file_name().unwrap(), "README");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_and_controls() {
        let mut d = Downloads::default();
        assert!(d.status_text().is_none());
        d.items.push(DownloadItem::new(1, "u".into(), "a.zip".into(), PathBuf::from("/tmp/a.zip"), Some(200), true, DownloadControl::default()));
        let t0 = Instant::now();
        d.items[0].progress(50, t0);
        assert_eq!(d.status_text().unwrap(), "Downloading a.zip — 25%");
        d.items[0].progress(150, t0 + std::time::Duration::from_millis(500));
        assert_eq!(d.items[0].speed, Some(200));
        assert_eq!(d.overall_progress(), Some(0.75));
        d.pause(1);
        assert!(d.items[0].control.pause.load(Ordering::SeqCst));
        d.cancel(1);
        assert!(d.items[0].control.cancel.load(Ordering::SeqCst));
        assert_eq!(d.rows().len(), 1);
        d.remove(1);
        assert_eq!(d.items.len(), 1, "active downloads stay listed");
        d.items[0].state = DownloadState::Done;
        d.clear_finished();
        assert!(d.items.is_empty());
    }
}
