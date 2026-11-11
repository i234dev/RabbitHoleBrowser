//! Smooth scrolling: wheel notches and scroll keys animate towards a
//! target with an ease-out curve; new input while animating retargets
//! (and accumulates), so fast wheel spins stay fluid.

use std::time::{Duration, Instant};

/// Length of one animation.
pub const DURATION: Duration = Duration::from_millis(140);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollAnim {
    from: (f32, f32),
    pub target: (f32, f32),
    start: Instant,
    duration: Duration,
}

/// Ease-out cubic.
fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

impl ScrollAnim {
    pub fn new(from: (f32, f32), target: (f32, f32), now: Instant) -> ScrollAnim {
        // Long jumps (Page Down, End) take a bit longer.
        let dist = ((target.0 - from.0).powi(2) + (target.1 - from.1).powi(2)).sqrt();
        let extra = (dist / 4000.0).min(1.0) * 80.0;
        ScrollAnim { from, target, start: now, duration: DURATION + Duration::from_millis(extra as u64) }
    }

    /// Position at `now`.
    pub fn position(&self, now: Instant) -> (f32, f32) {
        let t = now.saturating_duration_since(self.start).as_secs_f32() / self.duration.as_secs_f32();
        let e = ease(t);
        (self.from.0 + (self.target.0 - self.from.0) * e, self.from.1 + (self.target.1 - self.from.1) * e)
    }

    pub fn done(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.start) >= self.duration
    }

    /// Add more scroll while animating: continue from the current
    /// position towards the accumulated target.
    pub fn retarget(&mut self, delta: (f32, f32), now: Instant) {
        let pos = self.position(now);
        let target = (self.target.0 + delta.0, self.target.1 + delta.1);
        *self = ScrollAnim::new(pos, target, now);
    }

    /// Keep the target inside the scrollable range.
    pub fn clamp_target(&mut self, max: (f32, f32)) {
        self.target = (self.target.0.clamp(0.0, max.0.max(0.0)), self.target.1.clamp(0.0, max.1.max(0.0)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eases_to_target() {
        let t0 = Instant::now();
        let a = ScrollAnim::new((0.0, 0.0), (0.0, 100.0), t0);
        assert_eq!(a.position(t0), (0.0, 0.0));
        let mid = a.position(t0 + DURATION / 2);
        assert!(mid.1 > 50.0 && mid.1 < 100.0, "ease-out is past halfway at half time: {mid:?}");
        assert_eq!(a.position(t0 + Duration::from_secs(1)), (0.0, 100.0));
        assert!(a.done(t0 + Duration::from_secs(1)));
        assert!(!a.done(t0));
    }

    #[test]
    fn retarget_accumulates() {
        let t0 = Instant::now();
        let mut a = ScrollAnim::new((0.0, 0.0), (0.0, 100.0), t0);
        a.retarget((0.0, 100.0), t0 + DURATION / 2);
        assert_eq!(a.target, (0.0, 200.0));
        let p = a.position(t0 + DURATION / 2);
        assert!(p.1 > 50.0, "continues from where it was: {p:?}");
        a.clamp_target((0.0, 150.0));
        assert_eq!(a.target, (0.0, 150.0));
    }
}
