//! Audio output: one mixer thread per process feeding the platform's
//! audio device, mixing every playing [`AudioSink`] (one per media element
//! or audio context).
//!
//! Backends: WASAPI (Windows, shared mode, event driven), PulseAudio /
//! PipeWire (through `libpulse-simple`, loaded at run time) and ALSA
//! (`libasound`, loaded at run time) on Linux and the BSDs, CoreAudio
//! (AudioQueue) on macOS, and a "null" sink that consumes audio in real
//! time without a device (`RH_AUDIO=null`, and the fallback when no device
//! can be opened, so media clocks still advance).
//!
//! `RH_AUDIO_DUMP=<file.wav>` additionally writes everything mixed while a
//! sink plays to a WAV file (used to verify decoding on machines without a
//! sound card); [`stats`] counts and checksums the mixed samples.

mod null;
#[cfg(all(unix, not(target_os = "macos")))]
mod pulse;
#[cfg(all(unix, not(target_os = "macos")))]
mod alsa;
#[cfg(windows)]
mod wasapi;
#[cfg(target_os = "macos")]
mod coreaudio;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

/// A platform audio device the mixer writes to.
pub(crate) trait Backend: Send {
    /// Device sample rate (Hz).
    fn rate(&self) -> u32;
    /// Device channel count (the mixer produces stereo; extra channels are
    /// silent, a mono device gets the downmix).
    fn channels(&self) -> usize;
    /// Block until the device can take more audio; returns how many frames
    /// it can take now (at least 1).
    fn wait(&mut self) -> Result<usize, String>;
    /// Write interleaved frames (`data.len() / channels()` frames, never
    /// more than the last `wait` allowed).
    fn write(&mut self, data: &[f32]) -> Result<(), String>;
    /// Audio written but not yet heard, in seconds.
    fn latency(&self) -> f64;
    /// Short backend name ("wasapi", "pulse", "alsa", "coreaudio", "null").
    fn name(&self) -> &'static str;
}

/// Open the platform device: the backend chosen by `RH_AUDIO`
/// (`null`, `pulse`, `alsa`, `wasapi`, `coreaudio`) or the first that works.
fn open_backend() -> Result<Box<dyn Backend>, String> {
    let want = std::env::var("RH_AUDIO").unwrap_or_default().to_ascii_lowercase();
    if want == "null" || want == "none" {
        return Ok(Box::new(null::NullBackend::new()));
    }
    let mut errors: Vec<String> = Vec::new();
    #[cfg(windows)]
    {
        if want.is_empty() || want == "wasapi" {
            match wasapi::WasapiBackend::open() {
                Ok(b) => return Ok(Box::new(b)),
                Err(e) => errors.push(format!("wasapi: {e}")),
            }
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if want.is_empty() || want == "pulse" || want == "pipewire" {
            match pulse::PulseBackend::open() {
                Ok(b) => return Ok(Box::new(b)),
                Err(e) => errors.push(format!("pulse: {e}")),
            }
        }
        if want.is_empty() || want == "alsa" {
            match alsa::AlsaBackend::open() {
                Ok(b) => return Ok(Box::new(b)),
                Err(e) => errors.push(format!("alsa: {e}")),
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        if want.is_empty() || want == "coreaudio" {
            match coreaudio::CoreAudioBackend::open() {
                Ok(b) => return Ok(Box::new(b)),
                Err(e) => errors.push(format!("coreaudio: {e}")),
            }
        }
    }
    if errors.is_empty() {
        errors.push(format!("unknown RH_AUDIO backend {want:?}"));
    }
    Err(errors.join("; "))
}

// ------------------------------------------------------------------ sinks

/// One source of audio for the mixer (a media element's audio track).
/// Decoded PCM is pushed with its media timestamp; the mixer resamples it
/// to the device rate at the playback rate and reports the media time
/// being heard ([`AudioSink::clock`]). Dropping the sink removes it.
pub struct AudioSink {
    shared: Arc<SinkShared>,
}

struct SinkShared {
    st: Mutex<SinkState>,
}

struct SinkState {
    /// Queued stereo frames at `rate`, contiguous in media time.
    queue: VecDeque<[f32; 2]>,
    /// Media time of `queue[0]`.
    queue_start: f64,
    /// Fractional read position into `queue`.
    pos: f64,
    /// Sample rate of the queued frames (0 = nothing queued yet).
    rate: u32,
    playing: bool,
    volume: f32,
    muted: bool,
    speed: f64,
    /// Gain applied at the end of the last mixed buffer (ramps to the
    /// target to avoid clicks).
    last_gain: f32,
    /// No more audio will be pushed (end of stream).
    eos: bool,
    /// Media time heard now (mixed time minus device latency).
    clock: f64,
    /// Lower bound of `clock` (the last flush / seek target).
    floor: f64,
    /// The mixer ran out of queued audio while playing (and not at EOS).
    underrun: bool,
    /// Total frames mixed from this sink (diagnostics).
    frames_mixed: u64,
}

impl Default for AudioSink {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioSink {
    /// A new, paused sink at media time 0, registered with the process
    /// mixer (which starts on first use).
    pub fn new() -> AudioSink {
        let shared = Arc::new(SinkShared {
            st: Mutex::new(SinkState {
                queue: VecDeque::new(),
                queue_start: 0.0,
                pos: 0.0,
                rate: 0,
                playing: false,
                volume: 1.0,
                muted: false,
                speed: 1.0,
                last_gain: 1.0,
                eos: false,
                clock: 0.0,
                floor: 0.0,
                underrun: false,
                frames_mixed: 0,
            }),
        });
        mixer().add(&shared);
        AudioSink { shared }
    }

    fn st(&self) -> std::sync::MutexGuard<'_, SinkState> {
        self.shared.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queue decoded audio: `data` is interleaved with `channels` channels
    /// at `rate` Hz, starting at media time `pts` (seconds). Audio before
    /// the end of what is queued is dropped; a gap is filled with silence.
    pub fn push(&self, pts: f64, rate: u32, channels: usize, data: &[f32]) {
        if channels == 0 || rate == 0 || data.is_empty() {
            return;
        }
        let mut frames: Vec<[f32; 2]> = to_stereo(data, channels);
        let mut st = self.st();
        if st.rate != 0 && st.rate != rate {
            // A rate change mid-stream: convert to the queue's rate.
            frames = resample_linear(&frames, rate, st.rate);
        } else if st.rate == 0 {
            st.rate = rate;
        }
        let qrate = st.rate as f64;
        // Media time right after the queued audio.
        let end = st.queue_start + st.queue.len() as f64 / qrate;
        let delta = pts - end;
        if delta < -0.5 / qrate {
            // Overlap: drop what is already queued / played.
            let drop = ((-delta) * qrate).round() as usize;
            if drop >= frames.len() {
                return;
            }
            frames.drain(..drop);
        } else if delta > 0.5 / qrate && delta < 2.0 {
            // A gap: fill with silence so time stays contiguous.
            let n = (delta * qrate).round() as usize;
            st.queue.extend(std::iter::repeat_n([0.0f32; 2], n));
        } else if delta >= 2.0 {
            // A big jump (discontinuity): restart the queue at `pts` once
            // what is queued has played.
            if st.queue.is_empty() {
                st.queue_start = pts;
                st.pos = 0.0;
            } else {
                let n = (2.0 * qrate) as usize;
                st.queue.extend(std::iter::repeat_n([0.0f32; 2], n));
            }
        }
        st.queue.extend(frames);
        st.underrun = false;
    }

    /// Seconds of audio queued ahead of the read position.
    pub fn buffered_secs(&self) -> f64 {
        let st = self.st();
        if st.rate == 0 {
            return 0.0;
        }
        ((st.queue.len() as f64 - st.pos).max(0.0)) / st.rate as f64
    }

    /// Media time at the end of the queued audio.
    pub fn queued_end(&self) -> f64 {
        let st = self.st();
        if st.rate == 0 {
            return st.floor;
        }
        st.queue_start + st.queue.len() as f64 / st.rate as f64
    }

    /// Drop everything queued and restart the clock at `time` (seek).
    pub fn flush(&self, time: f64) {
        let mut st = self.st();
        st.queue.clear();
        st.pos = 0.0;
        st.queue_start = time;
        st.clock = time;
        st.floor = time;
        st.eos = false;
        st.underrun = false;
        st.rate = 0;
    }

    pub fn set_playing(&self, playing: bool) {
        let mut st = self.st();
        st.playing = playing;
        if !playing {
            st.underrun = false;
        }
    }

    pub fn set_volume(&self, volume: f32) {
        self.st().volume = volume.clamp(0.0, 1.0);
    }

    pub fn set_muted(&self, muted: bool) {
        self.st().muted = muted;
    }

    /// Playback rate (resampled: pitch follows the rate).
    pub fn set_speed(&self, speed: f64) {
        self.st().speed = if speed.is_finite() { speed.clamp(0.0625, 16.0) } else { 1.0 };
    }

    /// No more audio will be pushed: the sink drains then reports
    /// [`drained`](Self::drained).
    pub fn set_eos(&self, eos: bool) {
        self.st().eos = eos;
    }

    /// Media time being heard now.
    pub fn clock(&self) -> f64 {
        self.st().clock
    }

    /// End of stream reached and everything was mixed.
    pub fn drained(&self) -> bool {
        let st = self.st();
        st.eos && (st.queue.is_empty() || st.pos >= st.queue.len() as f64 - 1.0)
    }

    /// Ran dry while playing (waiting for data).
    pub fn underrun(&self) -> bool {
        self.st().underrun
    }

    /// Frames mixed from this sink so far.
    pub fn frames_mixed(&self) -> u64 {
        self.st().frames_mixed
    }
}

impl Drop for AudioSink {
    fn drop(&mut self) {
        mixer().remove(&self.shared);
    }
}

/// Interleaved `channels`-channel samples → stereo frames (mono is
/// duplicated, more channels are downmixed: L/R plus centre and
/// surrounds at -3 dB).
pub(crate) fn to_stereo(data: &[f32], channels: usize) -> Vec<[f32; 2]> {
    let n = data.len() / channels;
    let mut out = Vec::with_capacity(n);
    match channels {
        1 => out.extend(data.iter().map(|&s| [s, s])),
        2 => out.extend(data.chunks_exact(2).map(|c| [c[0], c[1]])),
        _ => {
            const K: f32 = std::f32::consts::FRAC_1_SQRT_2;
            for c in data.chunks_exact(channels) {
                // Common layouts: L R C LFE SL SR [BL BR] (WAV / AAC order
                // differs for some, close enough for a downmix).
                let (l, r) = (c[0], c[1]);
                let centre = if channels >= 3 { c[2] } else { 0.0 };
                let mut sl = 0.0;
                let mut sr = 0.0;
                if channels >= 6 {
                    sl = c[4];
                    sr = c[5];
                }
                if channels >= 8 {
                    sl += c[6];
                    sr += c[7];
                }
                let norm = 1.0 / (1.0 + K + if channels >= 6 { K } else { 0.0 });
                out.push([(l + K * centre + K * sl) * norm, (r + K * centre + K * sr) * norm]);
            }
        }
    }
    out
}

/// Simple linear-interpolation sample-rate conversion.
pub(crate) fn resample_linear(frames: &[[f32; 2]], from: u32, to: u32) -> Vec<[f32; 2]> {
    if from == to || frames.is_empty() {
        return frames.to_vec();
    }
    let step = from as f64 / to as f64;
    let n = ((frames.len() as f64) / step).floor() as usize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let p = i as f64 * step;
        let k = p.floor() as usize;
        let f = (p - k as f64) as f32;
        let a = frames[k.min(frames.len() - 1)];
        let b = frames[(k + 1).min(frames.len() - 1)];
        out.push([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f]);
    }
    out
}

// ------------------------------------------------------------------ mixer

struct Mixer {
    inner: Mutex<MixerInner>,
    cv: std::sync::Condvar,
}

#[derive(Default)]
struct MixerInner {
    sinks: Vec<Weak<SinkShared>>,
    running: bool,
    backend_name: &'static str,
    device_error: Option<String>,
    rate: u32,
}

fn mixer() -> &'static Mixer {
    static M: OnceLock<Mixer> = OnceLock::new();
    M.get_or_init(|| Mixer { inner: Mutex::new(MixerInner::default()), cv: std::sync::Condvar::new() })
}

impl Mixer {
    fn lock(&self) -> std::sync::MutexGuard<'_, MixerInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn add(&'static self, s: &Arc<SinkShared>) {
        let mut m = self.lock();
        m.sinks.retain(|w| w.strong_count() > 0);
        m.sinks.push(Arc::downgrade(s));
        if !m.running {
            m.running = true;
            drop(m);
            let spawned = std::thread::Builder::new().name("rh-audio".into()).spawn(move || self.run());
            if spawned.is_err() {
                self.lock().running = false;
            }
        } else {
            self.cv.notify_all();
        }
    }

    fn remove(&self, s: &Arc<SinkShared>) {
        let mut m = self.lock();
        m.sinks.retain(|w| w.strong_count() > 0 && !std::ptr::eq(w.as_ptr(), Arc::as_ptr(s)));
    }

    fn live_sinks(&self) -> Vec<Arc<SinkShared>> {
        let mut m = self.lock();
        m.sinks.retain(|w| w.strong_count() > 0);
        m.sinks.iter().filter_map(|w| w.upgrade()).collect()
    }

    /// The mixer thread: open the device, mix until no sink is left for a
    /// while, then close the device and exit (restarted on demand).
    fn run(&'static self) {
        let mut backend: Box<dyn Backend> = match open_backend() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("[audio] no audio output device ({e}); playing silently");
                let mut m = self.lock();
                m.device_error = Some(e);
                drop(m);
                Box::new(null::NullBackend::new())
            }
        };
        {
            let mut m = self.lock();
            m.backend_name = backend.name();
            m.rate = backend.rate();
        }
        if std::env::var("RH_AUDIO_DEBUG").is_ok() {
            eprintln!("[audio] output: {} {} Hz {} ch", backend.name(), backend.rate(), backend.channels());
        }
        let mut dump = std::env::var("RH_AUDIO_DUMP").ok().filter(|p| !p.is_empty()).and_then(|p| WavDump::create(&p, backend.rate()));
        let mut idle_since: Option<Instant> = None;
        let mut mix: Vec<[f32; 2]> = Vec::new();
        let mut out: Vec<f32> = Vec::new();
        loop {
            let frames = match backend.wait() {
                Ok(n) => n.clamp(1, 16384),
                Err(e) => {
                    eprintln!("[audio] device error ({e}); reopening");
                    std::thread::sleep(Duration::from_millis(200));
                    backend = open_backend().unwrap_or_else(|_| Box::new(null::NullBackend::new()));
                    let mut m = self.lock();
                    m.backend_name = backend.name();
                    m.rate = backend.rate();
                    continue;
                }
            };
            let sinks = self.live_sinks();
            if sinks.is_empty() {
                let since = *idle_since.get_or_insert_with(Instant::now);
                if since.elapsed() > Duration::from_secs(10) {
                    let mut m = self.lock();
                    if m.sinks.iter().all(|w| w.strong_count() == 0) {
                        m.running = false;
                        return;
                    }
                }
            } else {
                idle_since = None;
            }
            mix.clear();
            mix.resize(frames, [0.0; 2]);
            let rate = backend.rate();
            let latency = backend.latency();
            let mut any_playing = false;
            for s in &sinks {
                let mut st = s.st.lock().unwrap_or_else(|e| e.into_inner());
                any_playing |= st.playing;
                mix_sink(&mut st, &mut mix, rate, latency);
            }
            let ch = backend.channels().max(1);
            out.clear();
            out.reserve(frames * ch);
            for f in &mix {
                let l = soft_clip(f[0]);
                let r = soft_clip(f[1]);
                match ch {
                    1 => out.push((l + r) * 0.5),
                    _ => {
                        out.push(l);
                        out.push(r);
                        out.extend(std::iter::repeat_n(0.0, ch - 2));
                    }
                }
            }
            if any_playing {
                record_stats(&mix);
                if let Some(d) = dump.as_mut() {
                    d.write(&mix);
                }
            }
            if let Err(e) = backend.write(&out) {
                eprintln!("[audio] write failed ({e}); reopening");
                backend = open_backend().unwrap_or_else(|_| Box::new(null::NullBackend::new()));
                let mut m = self.lock();
                m.backend_name = backend.name();
                m.rate = backend.rate();
            }
        }
    }
}

fn soft_clip(x: f32) -> f32 {
    if x.abs() <= 0.95 {
        x
    } else {
        x.signum() * (0.95 + 0.05 * ((x.abs() - 0.95) / 0.05).tanh())
    }
}

/// Mix one sink into `out` (stereo at `dev_rate`).
fn mix_sink(st: &mut SinkState, out: &mut [[f32; 2]], dev_rate: u32, latency: f64) {
    if !st.playing || st.rate == 0 {
        if st.playing && !st.eos && st.queue.is_empty() {
            st.underrun = true;
        }
        return;
    }
    let target_gain = if st.muted { 0.0 } else { st.volume };
    let step = st.rate as f64 * st.speed / dev_rate as f64;
    let n = out.len();
    let mut produced = 0usize;
    let len = st.queue.len();
    let unity = (step - 1.0).abs() < 1e-9;
    for (i, o) in out.iter_mut().enumerate() {
        let k = st.pos as usize;
        if k >= len || (!unity && k + 1 >= len && !st.eos) {
            break;
        }
        let a = st.queue[k];
        let s = if unity || k + 1 >= len {
            a
        } else {
            let b = st.queue[k + 1];
            let f = (st.pos - k as f64) as f32;
            [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f]
        };
        let t = i as f32 / n as f32;
        let g = st.last_gain + (target_gain - st.last_gain) * t;
        o[0] += s[0] * g;
        o[1] += s[1] * g;
        st.pos += step;
        produced += 1;
    }
    st.last_gain = target_gain;
    st.frames_mixed += produced as u64;
    if produced < n && !st.eos {
        st.underrun = true;
    } else if produced > 0 {
        st.underrun = false;
    }
    // Drop consumed frames.
    let consumed = (st.pos.floor() as usize).min(st.queue.len());
    if consumed > 0 {
        st.queue.drain(..consumed);
        st.pos -= consumed as f64;
        st.queue_start += consumed as f64 / st.rate as f64;
    }
    let mixed_time = st.queue_start + st.pos / st.rate as f64;
    let heard = mixed_time - latency * st.speed;
    st.clock = heard.max(st.floor).max(st.clock.min(mixed_time));
}

// ------------------------------------------------------------ diagnostics

static STAT_FRAMES: AtomicU64 = AtomicU64::new(0);
static STAT_NONSILENT: AtomicU64 = AtomicU64::new(0);
static STAT_HASH: AtomicU64 = AtomicU64::new(0xcbf2_9ce4_8422_2325);
static STAT_PEAK: AtomicU64 = AtomicU64::new(0);

fn record_stats(mix: &[[f32; 2]]) {
    let mut h = STAT_HASH.load(Ordering::Relaxed);
    let mut nonsilent = 0u64;
    let mut peak = f32::from_bits(STAT_PEAK.load(Ordering::Relaxed) as u32);
    for f in mix {
        for s in f {
            let q = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            if q.unsigned_abs() > 16 {
                nonsilent += 1;
            }
            peak = peak.max(s.abs());
            for b in q.to_le_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    STAT_HASH.store(h, Ordering::Relaxed);
    STAT_PEAK.store(peak.to_bits() as u64, Ordering::Relaxed);
    STAT_FRAMES.fetch_add(mix.len() as u64, Ordering::Relaxed);
    STAT_NONSILENT.fetch_add(nonsilent, Ordering::Relaxed);
}

/// Process-wide output statistics (only buffers mixed while some sink
/// was playing are counted).
#[derive(Clone, Debug, Default)]
pub struct AudioStats {
    /// Output backend in use ("" before the mixer first ran).
    pub backend: &'static str,
    /// Device sample rate.
    pub rate: u32,
    /// Why the platform device could not be opened (the null sink is used).
    pub device_error: Option<String>,
    /// Frames mixed while playing.
    pub frames: u64,
    /// Samples above the noise floor (|s| > 16/32768).
    pub nonsilent_samples: u64,
    /// Peak absolute sample value.
    pub peak: f32,
    /// FNV-1a hash of the mixed samples quantized to 16 bits.
    pub checksum: u64,
}

pub fn stats() -> AudioStats {
    let m = mixer().lock();
    AudioStats {
        backend: m.backend_name,
        rate: m.rate,
        device_error: m.device_error.clone(),
        frames: STAT_FRAMES.load(Ordering::Relaxed),
        nonsilent_samples: STAT_NONSILENT.load(Ordering::Relaxed),
        peak: f32::from_bits(STAT_PEAK.load(Ordering::Relaxed) as u32),
        checksum: STAT_HASH.load(Ordering::Relaxed),
    }
}

/// `RH_AUDIO_DUMP`: a 16-bit stereo WAV of the mixed output.
struct WavDump {
    file: std::fs::File,
    data_bytes: u32,
    last_header: Instant,
}

impl WavDump {
    fn create(path: &str, rate: u32) -> Option<WavDump> {
        use std::io::Write;
        let mut file = std::fs::File::create(path).ok()?;
        file.write_all(&crate::wav::header(rate, 2, 0)).ok()?;
        Some(WavDump { file, data_bytes: 0, last_header: Instant::now() })
    }

    fn write(&mut self, mix: &[[f32; 2]]) {
        use std::io::{Seek, SeekFrom, Write};
        let mut bytes = Vec::with_capacity(mix.len() * 4);
        for f in mix {
            for s in f {
                bytes.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
            }
        }
        if self.file.write_all(&bytes).is_err() {
            return;
        }
        self.data_bytes = self.data_bytes.saturating_add(bytes.len() as u32);
        if self.last_header.elapsed() > Duration::from_millis(500) {
            self.last_header = Instant::now();
            let _ = self.fix_header();
        }
        let _ = self.file.seek(SeekFrom::End(0));
    }

    fn fix_header(&mut self) -> std::io::Result<()> {
        use std::io::{Seek, SeekFrom, Write};
        self.file.seek(SeekFrom::Start(4))?;
        self.file.write_all(&(36 + self.data_bytes).to_le_bytes())?;
        self.file.seek(SeekFrom::Start(40))?;
        self.file.write_all(&self.data_bytes.to_le_bytes())?;
        self.file.flush()
    }
}

impl Drop for WavDump {
    fn drop(&mut self) {
        let _ = self.fix_header();
    }
}

#[cfg(test)]
mod tests;
