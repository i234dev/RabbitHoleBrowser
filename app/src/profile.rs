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

/// Open tabs at the end of the last run.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Session {
    pub urls: Vec<String>,
    pub active: usize,
}

impl Session {
    pub fn serialize(&self) -> String {
        let mut out = format!("active\t{}\n", self.active);
        for u in &self.urls {
            out.push_str("tab\t");
            out.push_str(u);
            out.push('\n');
        }
        out
    }

    pub fn parse(text: &str) -> Session {
        let mut s = Session::default();
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("active\t") {
                s.active = v.trim().parse().unwrap_or(0);
            } else if let Some(u) = line.strip_prefix("tab\t") {
                if !u.trim().is_empty() {
                    s.urls.push(u.trim().to_string());
                }
            }
        }
        if s.active >= s.urls.len() {
            s.active = 0;
        }
        s
    }

    pub fn load(path: &Path) -> Session {
        std::fs::read_to_string(path).map(|t| Session::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        let _ = std::fs::write(path, self.serialize());
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
    pub fn get(&self, host: &str) -> f32 {
        self.levels.get(host).copied().unwrap_or(1.0)
    }

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
        let s = Session { urls: vec!["https://a/".into(), "about:home".into()], active: 1 };
        let back = Session::parse(&s.serialize());
        assert_eq!(s, back);
        let bad = Session::parse("active\t9\ntab\thttps://x/\n");
        assert_eq!(bad.active, 0);
        assert_eq!(bad.urls.len(), 1);
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
