//! Garbage collector pause statistics (`RH_GC_STATS=1`).
//!
//! Every stop of the mutator for collector work (a whole collection or one
//! incremental slice) is recorded per thread; [`report`] prints how many
//! there were, their total, maximum and 95th percentile and a histogram.

use std::cell::RefCell;

#[derive(Default, Clone)]
pub struct GcStats {
    /// Pause lengths in microseconds.
    pub pauses_us: Vec<u32>,
    /// Completed collection cycles.
    pub cycles: u32,
    /// Microseconds spent in each phase: roots, mark, weak, sweep.
    pub phase_us: [u64; 4],
    /// Pauses taken from idle time (between tasks) rather than inside
    /// running script.
    pub idle_pauses: u32,
    pub idle_us: u64,
}

thread_local! {
    static STATS: RefCell<GcStats> = RefCell::new(GcStats::default());
}

/// Whether statistics are wanted (checked once per thread).
pub fn enabled() -> bool {
    thread_local! {
        static ON: bool = std::env::var_os("RH_GC_STATS").is_some() || std::env::var_os("RH_GC_TRACE").is_some();
    }
    ON.with(|o| *o)
}

pub(crate) fn record_pause(d: std::time::Duration, idle: bool) {
    let us = d.as_micros().min(u32::MAX as u128) as u32;
    let _ = STATS.try_with(|s| {
        let mut s = s.borrow_mut();
        s.pauses_us.push(us);
        if idle {
            s.idle_pauses += 1;
            s.idle_us += us as u64;
        }
    });
}

pub(crate) fn record_phase(phase: usize, d: std::time::Duration) {
    let _ = STATS.try_with(|s| s.borrow_mut().phase_us[phase] += d.as_micros() as u64);
}

pub(crate) fn record_cycle() {
    let _ = STATS.try_with(|s| s.borrow_mut().cycles += 1);
}

/// A copy of this thread's statistics.
pub fn snapshot() -> GcStats {
    STATS.with(|s| s.borrow().clone())
}

/// Forget this thread's statistics.
pub fn reset() {
    STATS.with(|s| *s.borrow_mut() = GcStats::default());
}

impl GcStats {
    pub fn total_us(&self) -> u64 {
        self.pauses_us.iter().map(|&p| p as u64).sum()
    }
    pub fn max_us(&self) -> u32 {
        self.pauses_us.iter().copied().max().unwrap_or(0)
    }
    /// The `q` quantile (0..=1) of the pauses.
    pub fn quantile_us(&self, q: f64) -> u32 {
        if self.pauses_us.is_empty() {
            return 0;
        }
        let mut v = self.pauses_us.clone();
        v.sort_unstable();
        let i = ((v.len() as f64 - 1.0) * q).round() as usize;
        v[i.min(v.len() - 1)]
    }
    /// One-line summary plus a histogram line.
    pub fn summary(&self) -> String {
        let buckets: [(u32, &str); 8] = [(1000, "<1ms"), (2000, "1-2"), (5000, "2-5"), (10_000, "5-10"), (20_000, "10-20"), (50_000, "20-50"), (100_000, "50-100"), (u32::MAX, ">100")];
        let mut counts = [0usize; 8];
        for &p in &self.pauses_us {
            let i = buckets.iter().position(|(lim, _)| p < *lim).unwrap_or(7);
            counts[i] += 1;
        }
        let hist: Vec<String> = buckets.iter().zip(counts.iter()).filter(|(_, c)| **c > 0).map(|((_, n), c)| format!("{n}:{c}")).collect();
        let ms = |us: u64| us as f64 / 1000.0;
        format!(
            "[gc-stats] cycles={} pauses={} total={:.1}ms max={:.2}ms p95={:.2}ms p50={:.2}ms idle={}/{:.1}ms phases(roots/mark/weak/sweep)={:.1}/{:.1}/{:.1}/{:.1}ms hist[{}]",
            self.cycles,
            self.pauses_us.len(),
            ms(self.total_us()),
            ms(self.max_us() as u64),
            ms(self.quantile_us(0.95) as u64),
            ms(self.quantile_us(0.5) as u64),
            self.idle_pauses,
            ms(self.idle_us),
            ms(self.phase_us[0]),
            ms(self.phase_us[1]),
            ms(self.phase_us[2]),
            ms(self.phase_us[3]),
            hist.join(" ")
        )
    }
}

/// Print this thread's statistics to stderr (when enabled).
pub fn report() {
    if enabled() {
        eprintln!("{}", snapshot().summary());
    }
}
