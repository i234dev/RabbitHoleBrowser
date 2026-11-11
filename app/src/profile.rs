//! The user profile directory: cookies, history, bookmarks, session and
//! per-host zoom live here as small text files.

use rabbithole_engine::net::CookieJar;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Locations of the profile files.
#[derive(Debug, Clone)]
pub struct Profile {
    pub dir: PathBuf,
}

impl Profile {
    /// `$XDG_CONFIG_HOME/rabbithole` (or the platform equivalent), falling
    /// back to `~/.rabbithole`. `RABBITHOLE_PROFILE` overrides.
    pub fn locate() -> Profile {
        let dir = std::env::var_os("RABBITHOLE_PROFILE")
            .map(PathBuf::from)
            .or_else(|| dirs::config_dir().map(|d| d.join("rabbithole")))
            .or_else(|| dirs::home_dir().map(|h| h.join(".rabbithole")))
            .unwrap_or_else(|| PathBuf::from(".rabbithole"));
        Profile::at(&dir)
    }

    pub fn at(dir: &Path) -> Profile {
        let _ = std::fs::create_dir_all(dir);
        Profile { dir: dir.to_path_buf() }
    }

    pub fn cookies_path(&self) -> PathBuf {
        self.dir.join("cookies.txt")
    }
    pub fn history_path(&self) -> PathBuf {
        self.dir.join("history.txt")
    }
    pub fn bookmarks_path(&self) -> PathBuf {
        self.dir.join("bookmarks.txt")
    }
    pub fn session_path(&self) -> PathBuf {
        self.dir.join("session.txt")
    }
    pub fn zoom_path(&self) -> PathBuf {
        self.dir.join("zoom.txt")
    }
    pub fn settings_path(&self) -> PathBuf {
        self.dir.join("settings.txt")
    }
    /// Log of automatically handled cookie dialogs.
    pub fn autocookies_path(&self) -> PathBuf {
        self.dir.join("autocookies.txt")
    }
    /// Per-site rules for cookie dialogs.
    pub fn autocookies_sites_path(&self) -> PathBuf {
        self.dir.join("autocookies-sites.txt")
    }
    pub fn local_storage_path(&self) -> PathBuf {
        self.dir.join("localstorage.txt")
    }

    /// Load the persisted cookie jar (empty when missing).
    pub fn load_cookies(&self) -> Arc<Mutex<CookieJar>> {
        let jar = CookieJar::load(&self.cookies_path()).unwrap_or_default();
        Arc::new(Mutex::new(jar))
    }

    /// Persist the jar when it changed.
    pub fn save_cookies(&self, jar: &Arc<Mutex<CookieJar>>) {
        if let Ok(mut jar) = jar.lock() {
            if jar.dirty {
                let _ = jar.save(&self.cookies_path());
            }
        }
    }

    /// Marker present while the browser runs: found at startup, it means
    /// the last run crashed.
    pub fn running_marker(&self) -> PathBuf {
        self.dir.join("running.lock")
    }

    /// A throwaway profile for a private window (deleted on exit).
    pub fn private() -> Profile {
        let dir = std::env::temp_dir().join(format!("rabbithole-private-{}-{}", std::process::id(), now_secs()));
        Profile::at(&dir)
    }

    /// The downloads directory (`~/Downloads` or the platform default).
    pub fn downloads_dir(&self) -> PathBuf {
        let dir = dirs::download_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
            .unwrap_or_else(|| PathBuf::from("."));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }
}

// ------------------------------------------------------------------ session

/// One tab of a saved session: its back/forward list and flags.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SessionTab {
    pub history: Vec<String>,
    /// Current entry in `history`.
    pub index: usize,
    pub pinned: bool,
    pub title: String,
}

#[cfg(test)]
impl SessionTab {
    pub fn url(&self) -> Option<&str> {
        self.history.get(self.index).or(self.history.last()).map(String::as_str)
    }
}

/// Open tabs at the end of the last run (saved continuously, so a crash
/// loses at most the last few seconds).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Session {
    pub tabs: Vec<SessionTab>,
    pub active: usize,
}

fn clean_field(s: &str) -> String {
    s.chars().map(|c| if c == '\t' || c == '\n' || c == '\r' { ' ' } else { c }).collect()
}

#[cfg(test)]
impl Session {
    /// A session of plain URLs (one entry each).
    pub fn from_urls(urls: &[&str], active: usize) -> Session {
        Session { tabs: urls.iter().map(|u| SessionTab { history: vec![u.to_string()], index: 0, pinned: false, title: String::new() }).collect(), active }
    }

    /// The current URL of every tab.
    pub fn urls(&self) -> Vec<String> {
        self.tabs.iter().filter_map(|t| t.url().map(str::to_string)).collect()
    }
}

impl Session {
    /// Format: `active<TAB>n`, then per tab a `tab<TAB>index<TAB>pinned<TAB>title`
    /// line followed by its `url<TAB>...` entries. Version 1 files (`tab<TAB>url`
    /// lines) still load.
    pub fn serialize(&self) -> String {
        let mut out = format!("session\t2\nactive\t{}\n", self.active);
        for t in &self.tabs {
            out.push_str(&format!("tab\t{}\t{}\t{}\n", t.index, t.pinned as u8, clean_field(&t.title)));
            for u in &t.history {
                out.push_str("url\t");
                out.push_str(&clean_field(u));
                out.push('\n');
            }
        }
        out
    }

    pub fn parse(text: &str) -> Session {
        let mut s = Session::default();
        let v2 = text.lines().next().map(|l| l.starts_with("session\t")).unwrap_or(false);
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("active\t") {
                s.active = v.trim().parse().unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("tab\t") {
                if v2 {
                    let mut f = rest.splitn(3, '\t');
                    let index = f.next().and_then(|x| x.trim().parse().ok()).unwrap_or(0);
                    let pinned = f.next().map(|x| x.trim() == "1").unwrap_or(false);
                    let title = f.next().unwrap_or("").trim().to_string();
                    s.tabs.push(SessionTab { history: Vec::new(), index, pinned, title });
                } else if !rest.trim().is_empty() {
                    s.tabs.push(SessionTab { history: vec![rest.trim().to_string()], index: 0, pinned: false, title: String::new() });
                }
            } else if let Some(u) = line.strip_prefix("url\t") {
                if let Some(t) = s.tabs.last_mut() {
                    if !u.trim().is_empty() {
                        t.history.push(u.trim().to_string());
                    }
                }
            }
        }
        s.tabs.retain(|t| !t.history.is_empty());
        for t in &mut s.tabs {
            if t.index >= t.history.len() {
                t.index = t.history.len() - 1;
            }
        }
        if s.active >= s.tabs.len() {
            s.active = 0;
        }
        s
    }

    pub fn load(path: &Path) -> Session {
        std::fs::read_to_string(path).map(|t| Session::parse(&t)).unwrap_or_default()
    }

    /// Write atomically (temp file + rename) so a crash never leaves a
    /// half-written session.
    pub fn save(&self, path: &Path) {
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, self.serialize()).is_ok() && std::fs::rename(&tmp, path).is_err() {
            let _ = std::fs::write(path, self.serialize());
        }
    }
}

// --------------------------------------------------------------- zoom levels

/// Zoom factors per host.
#[derive(Debug, Clone, Default)]
pub struct ZoomLevels {
    pub levels: HashMap<String, f32>,
}

pub const MIN_ZOOM: f32 = 0.25;
pub const MAX_ZOOM: f32 = 5.0;

/// Clamp and round a zoom factor to a tenth.
pub fn clamp_zoom(z: f32) -> f32 {
    ((z * 10.0).round() / 10.0).clamp(MIN_ZOOM, MAX_ZOOM)
}

impl ZoomLevels {
    #[cfg(test)]
    pub fn get(&self, host: &str) -> f32 {
        self.levels.get(host).copied().unwrap_or(1.0)
    }

    #[cfg(test)]
    pub fn set(&mut self, host: &str, zoom: f32) {
        if (zoom - 1.0).abs() < 0.01 {
            self.levels.remove(host);
        } else {
            self.levels.insert(host.to_string(), clamp_zoom(zoom));
        }
    }

    pub fn serialize(&self) -> String {
        let mut entries: Vec<_> = self.levels.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        entries.iter().map(|(h, z)| format!("{h}\t{z:.2}\n")).collect()
    }

    pub fn parse(text: &str) -> ZoomLevels {
        let mut z = ZoomLevels::default();
        for line in text.lines() {
            if let Some((h, v)) = line.split_once('\t') {
                if let Ok(f) = v.trim().parse::<f32>() {
                    z.levels.insert(h.trim().to_string(), clamp_zoom(f));
                }
            }
        }
        z
    }

    pub fn load(path: &Path) -> ZoomLevels {
        std::fs::read_to_string(path).map(|t| ZoomLevels::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        let _ = std::fs::write(path, self.serialize());
    }
}

/// Seconds since the Unix epoch.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_paths() {
        let dir = std::env::temp_dir().join(format!("rh-profile-test-{}", std::process::id()));
        let p = Profile::at(&dir);
        assert!(p.cookies_path().ends_with("cookies.txt"));
        assert!(p.history_path().starts_with(&dir));
        assert!(dir.is_dir());
        let jar = p.load_cookies();
        assert!(jar.lock().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_round_trip() {
        let mut s = Session::from_urls(&["https://a/", "about:home"], 1);
        s.tabs[0].history.push("https://a/next".into());
        s.tabs[0].index = 1;
        s.tabs[1].pinned = true;
        s.tabs[1].title = "Home\tpage".into();
        let back = Session::parse(&s.serialize());
        assert_eq!(back.tabs.len(), 2);
        assert_eq!(back.tabs[0].url(), Some("https://a/next"));
        assert!(back.tabs[1].pinned);
        assert_eq!(back.tabs[1].title, "Home page");
        assert_eq!(back.active, 1);
        // Version 1 files.
        let old = Session::parse("active\t9\ntab\thttps://x/\n");
        assert_eq!(old.active, 0);
        assert_eq!(old.urls(), vec!["https://x/".to_string()]);
        let dir = std::env::temp_dir().join(format!("rh-session-test-{}", std::process::id()));
        let p = Profile::at(&dir);
        s.save(&p.session_path());
        assert_eq!(Session::load(&p.session_path()).tabs.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zoom_round_trip_and_clamp() {
        assert_eq!(clamp_zoom(0.01), 0.25);
        assert_eq!(clamp_zoom(9.0), 5.0);
        assert_eq!(clamp_zoom(1.26), 1.3);
        let mut z = ZoomLevels::default();
        z.set("example.com", 1.5);
        z.set("one.org", 1.0);
        let back = ZoomLevels::parse(&z.serialize());
        assert_eq!(back.get("example.com"), 1.5);
        assert_eq!(back.get("one.org"), 1.0);
        assert!(!back.levels.contains_key("one.org"));
    }
}
