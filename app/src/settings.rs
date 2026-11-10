//! User settings persisted as `settings.txt` (`key=value` lines).

use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Run page scripts.
    pub javascript: bool,
    /// Download and show images.
    pub images: bool,
    /// Block known ad and tracker domains.
    pub block_trackers: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { javascript: true, images: true, block_trackers: true }
    }
}

impl Settings {
    pub fn serialize(&self) -> String {
        format!("javascript={}\nimages={}\nblock_trackers={}\n", self.javascript as u8, self.images as u8, self.block_trackers as u8)
    }

    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let on = matches!(v.trim(), "1" | "true" | "on" | "yes");
            match k.trim() {
                "javascript" => s.javascript = on,
                "images" => s.images = on,
                "block_trackers" => s.block_trackers = on,
                _ => {}
            }
        }
        s
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
        let s = Settings { javascript: false, images: true, block_trackers: false };
        assert_eq!(Settings::parse(&s.serialize()), s);
        assert_eq!(Settings::parse(""), Settings::default());
        assert_eq!(Settings::parse("javascript=0\nimages=0\nbogus=1"), Settings { javascript: false, images: false, block_trackers: true });
        let dir = std::env::temp_dir().join(format!("rh-settings-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("settings.txt");
        s.save(&p);
        assert_eq!(Settings::load(&p), s);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
