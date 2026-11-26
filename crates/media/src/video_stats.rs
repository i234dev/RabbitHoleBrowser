//! Opt-in wall-clock timings for decoded pictures. Frame ids are process-wide
//! so a trace can follow a picture across the decoder and content threads.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

pub(crate) fn clock() -> Option<Instant> {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    ENABLED.get_or_init(|| std::env::var("RH_MEDIA_STATS").as_deref() == Ok("1")).then(Instant::now)
}

#[derive(Debug)]
struct Picture {
    id: u64,
    painted: AtomicBool,
    composited: AtomicBool,
}

impl Drop for Picture {
    fn drop(&mut self) {
        if !*self.painted.get_mut() {
            eprintln!("rh_video drop {}", self.id);
        }
    }
}

#[derive(Clone, Debug)]
pub struct FrameTiming {
    picture: Arc<Picture>,
    start: Instant,
    conversion: Duration,
}

impl FrameTiming {
    pub(crate) fn converted(start: Option<Instant>) -> Option<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        start.map(|start| Self {
            conversion: start.elapsed(),
            start,
            picture: Arc::new(Picture { id: NEXT.fetch_add(1, Ordering::Relaxed), painted: AtomicBool::new(false), composited: AtomicBool::new(false) }),
        })
    }

    pub(crate) fn conversion(&self) -> Duration {
        self.conversion
    }

    pub(crate) fn decoded(&mut self, start: Instant, codec: &str, pts: f64, cost: Duration) {
        self.start = start;
        eprintln!("rh_video decode {} {codec} {pts:.6} {:.3} {:.3}", self.picture.id, ms(cost), ms(self.conversion));
    }

    /// First successful submission of this picture to the window surface.
    pub fn composited(&self, path: &'static str) {
        static EPOCH: OnceLock<Instant> = OnceLock::new();
        if !self.picture.composited.swap(true, Ordering::Relaxed) {
            self.picture.painted.store(true, Ordering::Relaxed);
            let epoch = EPOCH.get_or_init(Instant::now);
            eprintln!("rh_video composite {} {:.3} {:.3} {path}", self.picture.id, ms(self.start.elapsed()), ms(epoch.elapsed()));
        }
    }

    /// The content raster containing this update is ready for the embedder.
    pub fn painted(self, start: Instant, end: Instant) {
        static EPOCH: OnceLock<Instant> = OnceLock::new();
        self.picture.painted.store(true, Ordering::Relaxed);
        let epoch = EPOCH.get_or_init(|| end);
        eprintln!("rh_video paint {} {:.3} {:.3} {:.3}", self.picture.id, ms(end.duration_since(start)),
                  ms(end.duration_since(self.start)), ms(end.duration_since(*epoch)));
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
