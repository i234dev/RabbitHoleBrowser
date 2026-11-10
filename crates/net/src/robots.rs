//! `robots.txt` (RFC 9309): parsing, matching and a per-origin cache.
//!
//! Used when a [`crate::Fetcher`] has `respect_robots` set (the bot header
//! profile of the agent interfaces): top-level navigations to paths a site
//! disallows for `RabbitHoleBot` (or `*`) fail with
//! [`crate::FetchError::Robots`].

use std::collections::HashMap;
use std::sync::Mutex;

/// Product token matched against `User-agent:` lines.
pub const BOT_TOKEN: &str = "RabbitHoleBot";

/// One `allow` / `disallow` rule.
#[derive(Debug, Clone, PartialEq)]
struct Rule {
    allow: bool,
    pattern: String,
}

/// Parsed rules of one robots.txt for our user agent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Robots {
    rules: Vec<Rule>,
}

impl Robots {
    /// Parse a robots.txt body, keeping the groups for `agent` (case
    /// insensitive product token) or else the `*` groups.
    pub fn parse(text: &str, agent: &str) -> Robots {
        let agent = agent.to_ascii_lowercase();
        // Groups: (agents, rules).
        let mut groups: Vec<(Vec<String>, Vec<Rule>)> = Vec::new();
        let mut in_agents = false;
        for raw in text.lines() {
            let line = raw.split('#').next().unwrap_or("").trim();
            let Some((key, value)) = line.split_once(':') else { continue };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            match key.as_str() {
                "user-agent" => {
                    if !in_agents {
                        groups.push((Vec::new(), Vec::new()));
                        in_agents = true;
                    }
                    if let Some(g) = groups.last_mut() {
                        g.0.push(value.to_ascii_lowercase());
                    }
                }
                "allow" | "disallow" => {
                    in_agents = false;
                    if let Some(g) = groups.last_mut() {
                        // An empty `Disallow:` allows everything.
                        if !value.is_empty() {
                            g.1.push(Rule { allow: key == "allow", pattern: value.to_string() });
                        }
                    }
                }
                _ => in_agents = false,
            }
        }
        let is_ours = |a: &Vec<String>| a.iter().any(|x| x != "*" && !x.is_empty() && agent.starts_with(x.as_str()));
        if groups.iter().any(|(a, _)| is_ours(a)) {
            return Robots { rules: groups.iter().filter(|(a, _)| is_ours(a)).flat_map(|(_, r)| r.clone()).collect() };
        }
        Robots { rules: groups.iter().filter(|(a, _)| a.iter().any(|x| x == "*")).flat_map(|(_, r)| r.clone()).collect() }
    }

    /// Is `path` (path plus optional `?query`) allowed? The longest
    /// matching rule wins; ties go to `allow`.
    pub fn allows(&self, path: &str) -> bool {
        let mut best: Option<(usize, bool)> = None;
        for r in &self.rules {
            if pattern_matches(&r.pattern, path) {
                let len = r.pattern.len();
                match best {
                    Some((l, a)) if l > len || (l == len && a) => {}
                    _ => best = Some((len, r.allow)),
                }
            }
        }
        best.map(|(_, a)| a).unwrap_or(true)
    }
}

/// Match a robots pattern (`*` wildcard, `$` end anchor) against a path.
fn pattern_matches(pattern: &str, path: &str) -> bool {
    let anchored = pattern.ends_with('$');
    let pat = if anchored { &pattern[..pattern.len() - 1] } else { pattern };
    let parts: Vec<&str> = pat.split('*').collect();
    let mut pos = 0usize;
    let last = parts.len() - 1;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            if !path.starts_with(part) {
                return false;
            }
            pos = part.len();
            continue;
        }
        if part.is_empty() {
            continue;
        }
        if i == last && anchored {
            return path.len() >= pos + part.len() && path.ends_with(part);
        }
        match path[pos..].find(part) {
            Some(k) => pos += k + part.len(),
            None => return false,
        }
    }
    if anchored {
        // Pattern ended with `*$` (anything) or must match exactly.
        return (last > 0 && parts[last].is_empty()) || pos == path.len();
    }
    true
}

/// Parsed robots.txt per origin (an empty rule set when the site has none).
#[derive(Debug, Default)]
pub struct RobotsCache {
    map: Mutex<HashMap<String, Robots>>,
}

impl RobotsCache {
    pub fn get(&self, origin: &str) -> Option<Robots> {
        self.map.lock().unwrap().get(origin).cloned()
    }
    pub fn put(&self, origin: &str, robots: Robots) {
        self.map.lock().unwrap().insert(origin.to_string(), robots);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TXT: &str = "User-agent: *\nDisallow: /private\nAllow: /private/ok\n\nUser-agent: OtherBot\nDisallow: /\n";

    #[test]
    fn star_group_and_longest_match() {
        let r = Robots::parse(TXT, BOT_TOKEN);
        assert!(r.allows("/"));
        assert!(!r.allows("/private/x"));
        assert!(r.allows("/private/ok/y"));
        let o = Robots::parse(TXT, "OtherBot");
        assert!(!o.allows("/anything"));
    }

    #[test]
    fn specific_group_wins() {
        let t = "User-agent: *\nDisallow: /\n\nUser-agent: RabbitHoleBot\nUser-agent: x\nDisallow: /nope\n";
        let r = Robots::parse(t, BOT_TOKEN);
        assert!(r.allows("/yes"));
        assert!(!r.allows("/nope/1"));
    }

    #[test]
    fn wildcards_and_anchor() {
        let r = Robots::parse("User-agent: *\nDisallow: /*.pdf$\nDisallow: /s*q=\nDisallow:\n", BOT_TOKEN);
        assert!(!r.allows("/a/b.pdf"));
        assert!(r.allows("/a/b.pdf?x"));
        assert!(!r.allows("/search?q=1"));
        assert!(r.allows("/other"));
        assert!(Robots::parse("", BOT_TOKEN).allows("/x"));
    }
}
