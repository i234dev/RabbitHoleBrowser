//! Automatic handling of cookie-consent dialogs: the global mode, per-site
//! rules and the log of what was done (`about:autocookies`). Detection and
//! clicking live in `rabbithole_engine::consent`; the content threads run
//! them and report each result to the UI, which records it here.

use std::collections::BTreeMap;
use std::path::Path;

use rabbithole_engine::consent::{Choice, ConsentOutcome};

/// The global setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConsentMode {
    /// Leave consent dialogs alone (default).
    #[default]
    Off,
    Accept,
    Reject,
}

impl ConsentMode {
    pub const ALL: [ConsentMode; 3] = [ConsentMode::Off, ConsentMode::Reject, ConsentMode::Accept];

    pub fn id(self) -> &'static str {
        match self {
            ConsentMode::Off => "off",
            ConsentMode::Accept => "accept",
            ConsentMode::Reject => "reject",
        }
    }

    pub fn from_id(s: &str) -> Option<ConsentMode> {
        match s.trim() {
            "off" | "0" | "none" => Some(ConsentMode::Off),
            "accept" => Some(ConsentMode::Accept),
            "reject" => Some(ConsentMode::Reject),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ConsentMode::Off => "Off",
            ConsentMode::Accept => "Auto-accept",
            ConsentMode::Reject => "Auto-reject",
        }
    }

    /// The next mode for the cycling menu entry.
    pub fn next(self) -> ConsentMode {
        match self {
            ConsentMode::Off => ConsentMode::Reject,
            ConsentMode::Reject => ConsentMode::Accept,
            ConsentMode::Accept => ConsentMode::Off,
        }
    }
}

/// A per-site override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteRule {
    Accept,
    Reject,
    /// Never touch this site's dialogs.
    Never,
}

impl SiteRule {
    pub fn id(self) -> &'static str {
        match self {
            SiteRule::Accept => "accept",
            SiteRule::Reject => "reject",
            SiteRule::Never => "never",
        }
    }
    pub fn from_id(s: &str) -> Option<SiteRule> {
        match s.trim() {
            "accept" => Some(SiteRule::Accept),
            "reject" => Some(SiteRule::Reject),
            "never" => Some(SiteRule::Never),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            SiteRule::Accept => "Always accept",
            SiteRule::Reject => "Always reject",
            SiteRule::Never => "Never touch",
        }
    }
}

/// What the content threads need to decide per page.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConsentPolicy {
    pub mode: ConsentMode,
    /// Host → rule; a rule for `example.com` also covers its subdomains.
    pub sites: BTreeMap<String, SiteRule>,
}

impl ConsentPolicy {
    /// The rule that applies to `host` (most specific first).
    pub fn rule_for(&self, host: &str) -> Option<SiteRule> {
        let host = host.trim_start_matches("www.").to_ascii_lowercase();
        let mut h = host.as_str();
        loop {
            if let Some(r) = self.sites.get(h).or_else(|| self.sites.get(&format!("www.{h}"))) {
                return Some(*r);
            }
            match h.split_once('.') {
                Some((_, rest)) if rest.contains('.') => h = rest,
                _ => return None,
            }
        }
    }

    /// What to do with a dialog on `host` (None: leave it alone).
    pub fn choice_for(&self, host: &str) -> Option<Choice> {
        if host.is_empty() {
            return None;
        }
        match self.rule_for(host) {
            Some(SiteRule::Accept) => Some(Choice::Accept),
            Some(SiteRule::Reject) => Some(Choice::Reject),
            Some(SiteRule::Never) => None,
            None => match self.mode {
                ConsentMode::Off => None,
                ConsentMode::Accept => Some(Choice::Accept),
                ConsentMode::Reject => Some(Choice::Reject),
            },
        }
    }
}

/// A handled dialog, as reported by a content thread.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsentEvent {
    pub host: String,
    pub url: String,
    pub outcome: ConsentOutcome,
    /// The choice that was attempted.
    pub choice: Choice,
    /// Name of the last button clicked (empty: none).
    pub button: String,
    pub vendor: Option<String>,
    pub in_frame: bool,
    /// Every button clicked, in order.
    pub steps: Vec<String>,
}

impl ConsentEvent {
    /// Short description for the status bar / URL-bar tooltip.
    pub fn summary(&self) -> String {
        match self.outcome {
            ConsentOutcome::Accepted => format!("Cookies: accepted automatically (“{}”)", self.button),
            ConsentOutcome::Rejected => format!("Cookies: rejected automatically (“{}”)", self.button),
            ConsentOutcome::NoButton => match self.choice {
                Choice::Reject => "Cookies: dialog found but no reject option — left for you".to_string(),
                Choice::Accept => "Cookies: dialog found but no accept button — left for you".to_string(),
            },
            ConsentOutcome::StillShown => format!("Cookies: clicked “{}” but the dialog stayed", self.button),
        }
    }
}

/// One entry of the log.
#[derive(Debug, Clone, PartialEq)]
pub struct LogEntry {
    pub id: u64,
    /// Seconds since the epoch.
    pub time: u64,
    pub host: String,
    pub outcome: ConsentOutcome,
    pub button: String,
    pub vendor: String,
    pub url: String,
}

impl LogEntry {
    pub fn action_label(&self) -> &'static str {
        match self.outcome {
            ConsentOutcome::Accepted => "Accepted",
            ConsentOutcome::Rejected => "Rejected",
            ConsentOutcome::NoButton => "Dialog found, no matching button",
            ConsentOutcome::StillShown => "Clicked, dialog stayed",
        }
    }
}

/// Most entries kept.
pub const MAX_LOG: usize = 2000;

/// The log plus the per-site rules.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AutoCookies {
    pub log: Vec<LogEntry>,
    pub sites: BTreeMap<String, SiteRule>,
}

fn clean(s: &str) -> String {
    s.chars().map(|c| if c == '\t' || c == '\n' || c == '\r' { ' ' } else { c }).collect::<String>().trim().to_string()
}

impl AutoCookies {
    pub fn record(&mut self, e: &ConsentEvent, time: u64) -> u64 {
        let id = self.log.iter().map(|l| l.id).max().unwrap_or(0) + 1;
        self.log.push(LogEntry { id, time, host: e.host.clone(), outcome: e.outcome, button: e.button.clone(), vendor: e.vendor.clone().unwrap_or_default(), url: e.url.clone() });
        if self.log.len() > MAX_LOG {
            let drop = self.log.len() - MAX_LOG;
            self.log.drain(..drop);
        }
        id
    }

    pub fn remove(&mut self, id: u64) {
        self.log.retain(|l| l.id != id);
    }

    pub fn clear(&mut self) {
        self.log.clear();
    }

    /// Set (Some) or remove (None) a site's rule.
    pub fn set_rule(&mut self, host: &str, rule: Option<SiteRule>) -> bool {
        let host = host.trim().trim_start_matches("www.").to_ascii_lowercase();
        if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':')) {
            return false;
        }
        match rule {
            Some(r) => {
                self.sites.insert(host, r);
            }
            None => {
                self.sites.remove(&host);
            }
        }
        true
    }

    /// Entries matching `query` (host, button, action or vendor), newest
    /// first.
    pub fn search(&self, query: &str) -> Vec<&LogEntry> {
        let q = query.trim().to_lowercase();
        self.log
            .iter()
            .rev()
            .filter(|e| q.is_empty() || e.host.to_lowercase().contains(&q) || e.button.to_lowercase().contains(&q) || e.action_label().to_lowercase().contains(&q) || e.vendor.to_lowercase().contains(&q))
            .collect()
    }

    pub fn serialize_log(&self) -> String {
        self.log.iter().map(|e| format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\n", e.id, e.time, clean(&e.host), e.outcome.id(), clean(&e.button), clean(&e.vendor), clean(&e.url))).collect()
    }

    pub fn parse_log(text: &str) -> Vec<LogEntry> {
        text.lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split('\t').collect();
                if f.len() < 4 {
                    return None;
                }
                Some(LogEntry {
                    id: f[0].parse().ok()?,
                    time: f[1].parse().ok()?,
                    host: f[2].to_string(),
                    outcome: ConsentOutcome::from_id(f[3])?,
                    button: f.get(4).unwrap_or(&"").to_string(),
                    vendor: f.get(5).unwrap_or(&"").to_string(),
                    url: f.get(6).unwrap_or(&"").to_string(),
                })
            })
            .collect()
    }

    pub fn serialize_sites(&self) -> String {
        self.sites.iter().map(|(h, r)| format!("{h}\t{}\n", r.id())).collect()
    }

    pub fn parse_sites(text: &str) -> BTreeMap<String, SiteRule> {
        text.lines().filter_map(|l| l.split_once('\t')).filter_map(|(h, r)| Some((h.trim().to_string(), SiteRule::from_id(r)?))).filter(|(h, _)| !h.is_empty()).collect()
    }

    pub fn load(log: &Path, sites: &Path) -> AutoCookies {
        AutoCookies {
            log: std::fs::read_to_string(log).map(|t| AutoCookies::parse_log(&t)).unwrap_or_default(),
            sites: std::fs::read_to_string(sites).map(|t| AutoCookies::parse_sites(&t)).unwrap_or_default(),
        }
    }

    pub fn save(&self, log: &Path, sites: &Path) {
        let _ = std::fs::write(log, self.serialize_log());
        let _ = std::fs::write(sites, self.serialize_sites());
    }

    /// Hosts to show in the "per-site" section: every logged host and every
    /// host with a rule.
    pub fn hosts(&self) -> Vec<String> {
        let mut v: Vec<String> = self.sites.keys().cloned().collect();
        for e in &self.log {
            let h = e.host.trim_start_matches("www.").to_string();
            if !v.contains(&h) {
                v.push(h);
            }
        }
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(host: &str, outcome: ConsentOutcome) -> ConsentEvent {
        ConsentEvent { host: host.into(), url: format!("https://{host}/"), outcome, choice: Choice::Reject, button: "Reject all".into(), vendor: Some("OneTrust".into()), in_frame: false, steps: vec![] }
    }

    #[test]
    fn policy_rules() {
        let mut p = ConsentPolicy::default();
        assert_eq!(p.choice_for("example.com"), None);
        p.mode = ConsentMode::Reject;
        assert_eq!(p.choice_for("example.com"), Some(Choice::Reject));
        p.sites.insert("example.com".into(), SiteRule::Accept);
        p.sites.insert("news.example.org".into(), SiteRule::Never);
        assert_eq!(p.choice_for("www.example.com"), Some(Choice::Accept));
        assert_eq!(p.choice_for("shop.example.com"), Some(Choice::Accept));
        assert_eq!(p.choice_for("news.example.org"), None);
        assert_eq!(p.choice_for("example.org"), Some(Choice::Reject));
        p.mode = ConsentMode::Off;
        assert_eq!(p.choice_for("example.com"), Some(Choice::Accept));
        assert_eq!(p.choice_for("other.net"), None);
    }

    #[test]
    fn log_round_trip() {
        let mut a = AutoCookies::default();
        a.record(&ev("a.example", ConsentOutcome::Rejected), 1000);
        a.record(&ev("b.example", ConsentOutcome::NoButton), 2000);
        a.set_rule("www.c.example", Some(SiteRule::Never));
        assert!(!a.set_rule("bad host", Some(SiteRule::Accept)));
        let dir = std::env::temp_dir().join(format!("rh-autocookies-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let (l, s) = (dir.join("log.txt"), dir.join("sites.txt"));
        a.save(&l, &s);
        let b = AutoCookies::load(&l, &s);
        assert_eq!(a, b);
        assert_eq!(b.sites.get("c.example"), Some(&SiteRule::Never));
        assert_eq!(b.search("b.ex").len(), 1);
        assert_eq!(b.search("rejected").len(), 1);
        assert_eq!(b.search("").first().map(|e| e.host.as_str()), Some("b.example"));
        let mut c = b.clone();
        c.remove(1);
        assert_eq!(c.log.len(), 1);
        assert_eq!(c.hosts(), vec!["b.example".to_string(), "c.example".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
