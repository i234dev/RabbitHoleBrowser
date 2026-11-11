//! The null sink: consumes audio in real time without a device.

use super::Backend;
use std::time::{Duration, Instant};

pub(crate) struct NullBackend {
    start: Instant,
    /// Frames consumed since `start`.
    consumed: u64,
    rate: u32,
}

impl NullBackend {
    pub(crate) fn new() -> NullBackend {
        NullBackend { start: Instant::now(), consumed: 0, rate: 48_000 }
    }
}

impl Backend for NullBackend {
    fn rate(&self) -> u32 {
        self.rate
    }
    fn channels(&self) -> usize {
        2
    }
    fn wait(&mut self) -> Result<usize, String> {
        loop {
            let due = (self.start.elapsed().as_secs_f64() * self.rate as f64) as u64;
            if due > self.consumed + 240 {
                // Never fall more than 100 ms behind (e.g. after a stall).
                let behind = due - self.consumed;
                if behind > self.rate as u64 / 10 {
                    self.consumed = due - self.rate as u64 / 10;
                }
                return Ok((due - self.consumed) as usize);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn write(&mut self, data: &[f32]) -> Result<(), String> {
        self.consumed += (data.len() / 2) as u64;
        Ok(())
    }
    fn latency(&self) -> f64 {
        0.0
    }
    fn name(&self) -> &'static str {
        "null"
    }
}
