//! Auto-wait: run the page until it is settled — no pending network
//! (documents, scripts, API calls; images, fonts and long-lived streams
//! excluded), no timer due soon, and the DOM not changing — with a cap.
//! When the cap is hit, say what was still pending.

use std::time::{Duration, Instant};

use rabbithole_engine::net::{Destination, NetLog, Url};
use rabbithole_engine::scripting::{Dialog, PendingNavigation};
use rabbithole_engine::{Engine, Page};

/// Timers due further away than this do not delay "settled".
pub const TIMER_HORIZON_MS: f64 = 600.0;
/// With only timers left, stop after the DOM has not changed for this long.
pub const QUIET_MS: u64 = 400;

/// What happened while settling.
#[derive(Debug, Default)]
pub struct Settled {
    pub navigation: Option<PendingNavigation>,
    pub new_windows: Vec<Url>,
    pub dialogs: Vec<Dialog>,
    /// The DOM changed (restyle happened) at least once.
    pub changed: bool,
    /// Cap reached: description of what was still pending.
    pub pending: Option<String>,
    pub elapsed: Duration,
}

impl Settled {
    fn merge(&mut self, o: rabbithole_engine::ScriptOutcome) {
        if o.navigation.is_some() && self.navigation.is_none() {
            self.navigation = o.navigation;
        }
        self.new_windows.extend(o.new_windows);
        self.dialogs.extend(o.dialogs);
        let _ = o.relayout;
    }
}

/// Cheap signature of the document content (structure and text).
pub fn dom_signature(page: &Page) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let doc = &page.doc;
    for n in doc.descendants(rabbithole_engine::dom::Document::ROOT) {
        n.hash(&mut h);
        if let Some(t) = doc.text(n) {
            t.hash(&mut h);
        } else if let Some(el) = doc.element(n) {
            for (k, v) in &el.attrs {
                if matches!(k.as_str(), "value" | "checked" | "disabled" | "hidden" | "aria-expanded" | "open" | "selected") {
                    k.hash(&mut h);
                    v.hash(&mut h);
                }
            }
        }
    }
    h.finish()
}

fn short_url(u: &str) -> String {
    match Url::parse(u) {
        Some(p) => {
            let mut s = p.path.clone();
            if let Some(q) = &p.query {
                s.push('?');
                s.push_str(q);
            }
            if s.chars().count() > 80 {
                s = s.chars().take(79).collect::<String>() + "…";
            }
            s
        }
        None => u.to_string(),
    }
}

/// Requests that block "settled" (not images/fonts, not long-lived).
fn blocking(log: &NetLog) -> Vec<(String, Duration)> {
    log.in_flight()
        .into_iter()
        .filter(|e| !matches!(e.dest, Destination::Image | Destination::Font))
        .map(|e| (format!("{} {}", e.method, short_url(&e.url)), e.started.elapsed()))
        .collect()
}

/// Run timers/microtasks/network completions until settled or `cap`.
pub fn settle(engine: &Engine, page: &mut Page, log: Option<&NetLog>, cap: Duration) -> Settled {
    let start = Instant::now();
    let mut s = Settled::default();
    // Side effects already queued on the page (from loading or the action).
    if let Some(n) = page.script.pending_navigation.take() {
        s.navigation = Some(n);
        s.elapsed = start.elapsed();
        return s;
    }
    s.new_windows.extend(std::mem::take(&mut page.script.new_windows));
    s.dialogs.extend(std::mem::take(&mut page.script.dialogs));
    let mut last_change = Instant::now();
    let mut sig = dom_signature(page);
    loop {
        let now = Instant::now();
        let net = log.map(blocking).unwrap_or_default();
        let now_ms = engine.start.elapsed().as_secs_f64() * 1000.0;
        let due = page.next_script_due();
        let timer_soon = due.map(|d| d - now_ms <= TIMER_HORIZON_MS).unwrap_or(false);
        if net.is_empty() && !timer_soon {
            break;
        }
        // Only timers left (polling, animations): settled once quiet.
        if net.is_empty() && now.duration_since(last_change) >= Duration::from_millis(QUIET_MS) && now.duration_since(start) >= Duration::from_millis(QUIET_MS) {
            break;
        }
        if now.duration_since(start) >= cap {
            let mut what: Vec<String> = net.iter().map(|(r, d)| format!("{r} ({:.1} s)", d.as_secs_f32())).collect();
            if what.is_empty() && timer_soon {
                what.push("timers that keep changing the page (animation or polling)".into());
            }
            s.pending = Some(what.join(", "));
            break;
        }
        // Sleep until the next timer (or a short poll for the network).
        let wait_ms = match due {
            Some(d) if d > now_ms => (d - now_ms).min(if net.is_empty() { TIMER_HORIZON_MS } else { 20.0 }),
            Some(_) => 0.0,
            None => 20.0,
        };
        let remaining = cap.saturating_sub(now.duration_since(start));
        let wait = Duration::from_micros((wait_ms * 1000.0) as u64).min(remaining);
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
        let out = engine.tick(page);
        if out.relayout {
            let s2 = dom_signature(page);
            if s2 != sig {
                sig = s2;
                last_change = Instant::now();
                s.changed = true;
            }
        }
        s.merge(out);
        if s.navigation.is_some() {
            break;
        }
    }
    s.elapsed = start.elapsed();
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use rabbithole_engine::Viewport;

    #[test]
    fn waits_for_short_timers_but_not_long_ones() {
        let mut engine = Engine::new_bundled_fonts_only();
        let html = "<p id=a>x</p><script>setTimeout(function(){document.getElementById('a').textContent='late'},120);setTimeout(function(){document.getElementById('a').textContent='never'},60000);</script>";
        let mut page = engine.load_html(html, Url::parse("https://example.test/").unwrap(), Viewport { width: 800.0, height: 600.0 });
        let s = settle(&engine, &mut page, None, Duration::from_secs(3));
        assert!(s.pending.is_none(), "{:?}", s.pending);
        assert!(s.elapsed < Duration::from_secs(2));
        assert!(page.doc.text_content(rabbithole_engine::dom::Document::ROOT).contains("late"));
    }

    #[test]
    fn interval_animation_settles_when_quiet_or_reports_pending() {
        let mut engine = Engine::new_bundled_fonts_only();
        let html = "<p id=a>0</p><script>var n=0;setInterval(function(){document.getElementById('a').textContent=String(++n)},30);</script>";
        let mut page = engine.load_html(html, Url::parse("https://example.test/").unwrap(), Viewport { width: 800.0, height: 600.0 });
        let s = settle(&engine, &mut page, None, Duration::from_millis(700));
        assert!(s.pending.as_deref().unwrap_or("").contains("timers"), "{:?}", s.pending);
    }
}
