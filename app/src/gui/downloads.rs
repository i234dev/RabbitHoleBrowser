//! Download bookkeeping on the UI thread.

use crate::gui::pages::DownloadRow;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub enum DownloadState {
    InProgress,
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
    pub cancel: Arc<AtomicBool>,
}

#[derive(Debug, Default)]
pub struct Downloads {
    pub items: Vec<DownloadItem>,
}

impl Downloads {
    pub fn get_mut(&mut self, id: u64) -> Option<&mut DownloadItem> {
        self.items.iter_mut().find(|d| d.id == id)
    }

    pub fn active(&self) -> impl Iterator<Item = &DownloadItem> {
        self.items.iter().filter(|d| d.state == DownloadState::InProgress)
    }

    pub fn cancel(&mut self, id: u64) {
        if let Some(d) = self.get_mut(id) {
            d.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        }
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
                    DownloadState::Done => "Completed".into(),
                    DownloadState::Failed(e) => format!("Failed: {e}"),
                    DownloadState::Cancelled => "Cancelled".into(),
                },
                active: d.state == DownloadState::InProgress,
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
    fn status_summary() {
        let mut d = Downloads::default();
        assert!(d.status_text().is_none());
        d.items.push(DownloadItem {
            id: 1,
            url: "u".into(),
            filename: "a.zip".into(),
            path: PathBuf::from("/tmp/a.zip"),
            done: 50,
            total: Some(200),
            state: DownloadState::InProgress,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        assert_eq!(d.status_text().unwrap(), "Downloading a.zip — 25%");
        d.cancel(1);
        assert!(d.items[0].cancel.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(d.rows().len(), 1);
    }
}
