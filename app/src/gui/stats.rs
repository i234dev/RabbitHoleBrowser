//! UI thread responsiveness measurements: how long each event handler
//! kept the thread busy (a "stall"; anything over ~16 ms drops frames and
//! long ones make the OS report the window as not responding).
//!
//! `RH_UI_STATS=1` logs a line per second with the worst stall to stderr;
//! Ctrl+Shift+U shows an overlay with the same numbers.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub struct UiStats {
    log: bool,
    /// (when, duration ms, what) of recent handler runs.
    recent: VecDeque<(Instant, f32, &'static str)>,
    /// Worst stall since start.
    pub max_ms: f32,
    pub max_kind: &'static str,
    /// Worst stall since the last report line.
    period_max: (f32, &'static str),
    period_events: u32,
    last_report: Instant,
    /// Duration of the last redraw (ms).
    pub last_frame_ms: f32,
}

impl UiStats {
    pub fn new(log: bool) -> UiStats {
        UiStats {
            log,
            recent: VecDeque::new(),
            max_ms: 0.0,
            max_kind: "",
            period_max: (0.0, ""),
            period_events: 0,
            last_report: Instant::now(),
            last_frame_ms: 0.0,
        }
    }

    /// One event handler ran for `d`.
    pub fn record(&mut self, kind: &'static str, d: Duration) {
        let ms = d.as_secs_f32() * 1000.0;
        let now = Instant::now();
        if kind == "redraw" {
            self.last_frame_ms = ms;
        }
        self.recent.push_back((now, ms, kind));
        while self.recent.front().map(|(t, _, _)| now.duration_since(*t) > Duration::from_secs(5)).unwrap_or(false) {
            self.recent.pop_front();
        }
        if ms > self.max_ms {
            self.max_ms = ms;
            self.max_kind = kind;
        }
        if ms > self.period_max.0 {
            self.period_max = (ms, kind);
        }
        self.period_events += 1;
    }

    /// Worst stall over the last 5 seconds.
    pub fn recent_max(&self) -> (f32, &'static str) {
        self.recent.iter().fold((0.0, ""), |acc, (_, ms, k)| if *ms > acc.0 { (*ms, *k) } else { acc })
    }

    /// Log a line per second (when enabled and something happened).
    pub fn report(&mut self) {
        if !self.log || self.last_report.elapsed() < Duration::from_secs(1) || self.period_events == 0 {
            return;
        }
        eprintln!(
            "[ui] {} events, worst stall {:.1} ms ({}), last frame {:.1} ms, worst since start {:.1} ms ({})",
            self.period_events, self.period_max.0, self.period_max.1, self.last_frame_ms, self.max_ms, self.max_kind
        );
        self.period_max = (0.0, "");
        self.period_events = 0;
        self.last_report = Instant::now();
    }

    /// Text for the on-screen overlay.
    pub fn overlay_text(&self) -> String {
        let (r, k) = self.recent_max();
        format!("UI frame {:.1} ms · worst stall (5 s) {:.1} ms ({}) · since start {:.1} ms ({})", self.last_frame_ms, r, k, self.max_ms, self.max_kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_worst_stall() {
        let mut s = UiStats::new(false);
        s.record("key", Duration::from_millis(3));
        s.record("redraw", Duration::from_millis(12));
        s.record("wheel", Duration::from_millis(1));
        assert_eq!(s.recent_max().1, "redraw");
        assert!((s.max_ms - 12.0).abs() < 0.5);
        assert!((s.last_frame_ms - 12.0).abs() < 0.5);
    }
}
