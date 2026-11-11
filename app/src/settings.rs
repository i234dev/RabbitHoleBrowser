//! User settings persisted as `settings.txt` (`key=value` lines). Unknown
//! keys are ignored and missing keys take their defaults, so the file
//! stays compatible across versions.

use std::path::Path;

/// Search engines offered in the omnibox / settings page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchEngine {
    DuckDuckGo,
    Google,
    Bing,
    Brave,
    Startpage,
}

impl SearchEngine {
    pub const ALL: [SearchEngine; 5] = [SearchEngine::DuckDuckGo, SearchEngine::Google, SearchEngine::Bing, SearchEngine::Brave, SearchEngine::Startpage];

    pub fn id(self) -> &'static str {
        match self {
            SearchEngine::DuckDuckGo => "duckduckgo",
            SearchEngine::Google => "google",
            SearchEngine::Bing => "bing",
            SearchEngine::Brave => "brave",
            SearchEngine::Startpage => "startpage",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            SearchEngine::DuckDuckGo => "DuckDuckGo",
            SearchEngine::Google => "Google",
            SearchEngine::Bing => "Bing",
            SearchEngine::Brave => "Brave Search",
            SearchEngine::Startpage => "Startpage",
        }
    }

    pub fn from_id(id: &str) -> Option<SearchEngine> {
        SearchEngine::ALL.into_iter().find(|e| e.id() == id.trim().to_ascii_lowercase())
    }

    /// Search URL for an already percent-encoded query.
    pub fn url(self, encoded_query: &str) -> String {
        match self {
            // The HTML endpoint works without JavaScript.
            SearchEngine::DuckDuckGo => format!("https://duckduckgo.com/html/?q={encoded_query}"),
            SearchEngine::Google => format!("https://www.google.com/search?q={encoded_query}"),
            SearchEngine::Bing => format!("https://www.bing.com/search?q={encoded_query}"),
            SearchEngine::Brave => format!("https://search.brave.com/search?q={encoded_query}"),
            SearchEngine::Startpage => format!("https://www.startpage.com/do/search?q={encoded_query}"),
        }
    }
}

/// Chrome colour theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeChoice {
    /// Follow the operating system.
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub fn id(self) -> &'static str {
        match self {
            ThemeChoice::System => "system",
            ThemeChoice::Light => "light",
            ThemeChoice::Dark => "dark",
        }
    }

    pub fn from_id(id: &str) -> Option<ThemeChoice> {
        match id.trim() {
            "system" => Some(ThemeChoice::System),
            "light" => Some(ThemeChoice::Light),
            "dark" => Some(ThemeChoice::Dark),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Run page scripts.
    pub javascript: bool,
    /// Download and show images.
    pub images: bool,
    /// Block known ad and tracker domains.
    pub block_trackers: bool,
    /// Do not send or store cookies in cross-site subresource requests.
    pub block_third_party_cookies: bool,
    /// Home page (`about:home` = the built-in start page).
    pub homepage: String,
    pub search_engine: SearchEngine,
    /// Restore the tabs of the last session on startup.
    pub restore_session: bool,
    /// Zoom for sites without a remembered level.
    pub default_zoom: f32,
    /// Downloads directory (empty: the platform default).
    pub download_dir: String,
    /// Ask where to save each download.
    pub ask_download: bool,
    pub theme: ThemeChoice,
    /// `Accept-Language` request header.
    pub accept_language: String,
    pub show_bookmarks_bar: bool,
    /// Animate wheel and keyboard scrolling.
    pub smooth_scrolling: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            javascript: true,
            images: true,
            block_trackers: true,
            block_third_party_cookies: false,
            homepage: "about:home".into(),
            search_engine: SearchEngine::DuckDuckGo,
            restore_session: true,
            default_zoom: 1.0,
            download_dir: String::new(),
            ask_download: false,
            theme: ThemeChoice::System,
            accept_language: "es-ES,es;q=0.9,en;q=0.8".into(),
            show_bookmarks_bar: false,
            smooth_scrolling: true,
        }
    }
}

fn clean(s: &str) -> String {
    s.chars().filter(|c| *c != '\n' && *c != '\r').collect::<String>().trim().to_string()
}

impl Settings {
    pub fn serialize(&self) -> String {
        let b = |v: bool| v as u8;
        format!(
            "javascript={}\nimages={}\nblock_trackers={}\nblock_third_party_cookies={}\nhomepage={}\nsearch_engine={}\nrestore_session={}\ndefault_zoom={:.2}\ndownload_dir={}\nask_download={}\ntheme={}\naccept_language={}\nshow_bookmarks_bar={}\nsmooth_scrolling={}\n",
            b(self.javascript),
            b(self.images),
            b(self.block_trackers),
            b(self.block_third_party_cookies),
            clean(&self.homepage),
            self.search_engine.id(),
            b(self.restore_session),
            self.default_zoom,
            clean(&self.download_dir),
            b(self.ask_download),
            self.theme.id(),
            clean(&self.accept_language),
            b(self.show_bookmarks_bar),
            b(self.smooth_scrolling),
        )
    }

    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            s.set(k.trim(), v);
        }
        s
    }

    /// Set one setting from its textual form (settings file, settings
    /// page). Returns false for unknown keys or invalid values.
    pub fn set(&mut self, key: &str, value: &str) -> bool {
        let v = value.trim();
        let on = matches!(v, "1" | "true" | "on" | "yes");
        match key {
            "javascript" => self.javascript = on,
            "images" => self.images = on,
            "block_trackers" => self.block_trackers = on,
            "block_third_party_cookies" => self.block_third_party_cookies = on,
            "homepage" => self.homepage = if v.is_empty() { "about:home".into() } else { clean(v) },
            "search_engine" => match SearchEngine::from_id(v) {
                Some(e) => self.search_engine = e,
                None => return false,
            },
            "restore_session" => self.restore_session = on,
            "default_zoom" => match v.trim_end_matches('%').parse::<f32>() {
                Ok(z) if z.is_finite() && z > 0.0 => {
                    let z = if z > 10.0 { z / 100.0 } else { z };
                    self.default_zoom = crate::profile::clamp_zoom(z);
                }
                _ => return false,
            },
            "download_dir" => self.download_dir = clean(v),
            "ask_download" => self.ask_download = on,
            "theme" => match ThemeChoice::from_id(v) {
                Some(t) => self.theme = t,
                None => return false,
            },
            "accept_language" => {
                if v.is_empty() || !v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | ',' | ';' | '=' | '.' | ' ' | '*')) {
                    return false;
                }
                self.accept_language = v.to_string();
            }
            "show_bookmarks_bar" => self.show_bookmarks_bar = on,
            "smooth_scrolling" => self.smooth_scrolling = on,
            _ => return false,
        }
        true
    }

    pub fn load(path: &Path) -> Settings {
        std::fs::read_to_string(path).map(|t| Settings::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        let _ = std::fs::write(path, self.serialize());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let s = Settings {
            javascript: false,
            images: true,
            block_trackers: false,
            block_third_party_cookies: true,
            homepage: "https://example.org/".into(),
            search_engine: SearchEngine::Google,
            restore_session: false,
            default_zoom: 1.2,
            download_dir: "/tmp/dl".into(),
            ask_download: true,
            theme: ThemeChoice::Dark,
            accept_language: "en-US,en;q=0.9".into(),
            show_bookmarks_bar: true,
            smooth_scrolling: false,
        };
        assert_eq!(Settings::parse(&s.serialize()), s);
        assert_eq!(Settings::parse(""), Settings::default());
        let p = Settings::parse("javascript=0\nimages=0\nbogus=1");
        assert!(!p.javascript && !p.images && p.block_trackers);
        let dir = std::env::temp_dir().join(format!("rh-settings-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("settings.txt");
        s.save(&path);
        assert_eq!(Settings::load(&path), s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_validates() {
        let mut s = Settings::default();
        assert!(s.set("default_zoom", "125%"));
        assert_eq!(s.default_zoom, 1.3);
        assert!(s.set("default_zoom", "0.9"));
        assert_eq!(s.default_zoom, 0.9);
        assert!(!s.set("default_zoom", "abc"));
        assert!(!s.set("search_engine", "altavista"));
        assert!(s.set("search_engine", "brave"));
        assert_eq!(s.search_engine, SearchEngine::Brave);
        assert!(!s.set("accept_language", "en\r\nX-Evil: 1"));
        assert!(s.set("homepage", ""));
        assert_eq!(s.homepage, "about:home");
        assert_eq!(SearchEngine::DuckDuckGo.url("a+b"), "https://duckduckgo.com/html/?q=a+b");
    }
}
