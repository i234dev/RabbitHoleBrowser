//! The playback pipeline behind one media element: a worker thread
//! demuxes and decodes (direct files through Symphonia's format readers
//! over a downloading [`RangedSource`], or coded frames buffered by Media
//! Source Extensions) and feeds an [`AudioSink`]. The media clock is the
//! audio clock when there is audio, a wall clock otherwise. The element
//! reads a [`Status`] snapshot and sends commands.

use crate::audio::AudioSink;
use crate::decode::{Decoder, Pcm};
use crate::demux::TrackInfo;
use crate::mse::MseHandle;
use crate::source::{ByteFetcher, RangedSource};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use symphonia::core::codecs::audio::AudioCodecParameters;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Time;

/// Where the media data comes from.
pub enum PlayerSource {
    /// A URL fetched with range requests.
    Url { url: String, fetcher: Arc<dyn ByteFetcher> },
    /// Bytes already in memory (blob: / data: URLs).
    Bytes { data: Vec<u8>, mime: String },
    /// A MediaSource.
    Mse(MseHandle),
}

/// `MediaError` codes.
pub const ERR_ABORTED: u16 = 1;
pub const ERR_NETWORK: u16 = 2;
pub const ERR_DECODE: u16 = 3;
pub const ERR_SRC_NOT_SUPPORTED: u16 = 4;

/// What the element sees of the pipeline.
#[derive(Clone, Debug)]
pub struct Status {
    /// Current playback position (seconds).
    pub time: f64,
    /// NaN until known; +inf for unbounded streams.
    pub duration: f64,
    /// HTMLMediaElement readyState the data supports (0..=4).
    pub ready: u8,
    pub has_audio: bool,
    pub has_video: bool,
    pub video_width: u32,
    pub video_height: u32,
    pub audio_codec: String,
    pub video_codec: String,
    pub buffered: Vec<(f64, f64)>,
    pub seekable: Vec<(f64, f64)>,
    /// A seek is in progress.
    pub seeking: bool,
    /// Playback reached the end.
    pub ended: bool,
    /// The resource is fully downloaded (or nothing more is being fetched).
    pub idle: bool,
    pub error: Option<(u16, String)>,
    /// Bumped whenever a seek completes.
    pub seek_serial: u64,
    /// Total audio frames decoded and queued.
    pub decoded_frames: u64,
}

impl Default for Status {
    fn default() -> Self {
        Status {
            time: 0.0,
            duration: f64::NAN,
            ready: 0,
            has_audio: false,
            has_video: false,
            video_width: 0,
            video_height: 0,
            audio_codec: String::new(),
            video_codec: String::new(),
            buffered: Vec::new(),
            seekable: Vec::new(),
            seeking: false,
            ended: false,
            idle: false,
            error: None,
            seek_serial: 0,
            decoded_frames: 0,
        }
    }
}

struct Control {
    playing: bool,
    rate: f64,
    seek: Option<f64>,
    /// Wall clock (no audio): media time at `wall_at`.
    wall_time: f64,
    wall_at: Instant,
    /// The audio sink drives the clock.
    audio_clock: bool,
}

struct Shared {
    status: Mutex<Status>,
    ctl: Mutex<Control>,
    cv: Condvar,
    sink: AudioSink,
    stop: AtomicBool,
    source: Mutex<Option<RangedSource>>,
}

impl Shared {
    fn status(&self) -> MutexGuard<'_, Status> {
        self.status.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn ctl(&self) -> MutexGuard<'_, Control> {
        self.ctl.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn wake(&self) {
        self.cv.notify_all();
        if let Some(s) = self.source.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            s.interrupt();
        }
    }
    /// Sleep until woken or `ms` passed.
    fn nap(&self, ms: u64) {
        let g = self.ctl();
        let _ = self.cv.wait_timeout(g, Duration::from_millis(ms));
    }
    fn fail(&self, code: u16, msg: String) {
        let mut st = self.status();
        if st.error.is_none() {
            st.error = Some((code, msg));
        }
    }

    /// The media clock.
    fn time(&self) -> f64 {
        let c = self.ctl();
        if c.audio_clock {
            self.sink.clock()
        } else if c.playing {
            c.wall_time + c.wall_at.elapsed().as_secs_f64() * c.rate
        } else {
            c.wall_time
        }
    }
}

/// One media element's pipeline. Dropping it stops the worker.
pub struct Player {
    shared: Arc<Shared>,
}

impl Player {
    pub fn new(source: PlayerSource) -> Player {
        let shared = Arc::new(Shared {
            status: Mutex::new(Status::default()),
            ctl: Mutex::new(Control { playing: false, rate: 1.0, seek: None, wall_time: 0.0, wall_at: Instant::now(), audio_clock: false }),
            cv: Condvar::new(),
            sink: AudioSink::new(),
            stop: AtomicBool::new(false),
            source: Mutex::new(None),
        });
        let s2 = shared.clone();
        let spawned = std::thread::Builder::new().name("rh-media".into()).spawn(move || {
            match source {
                PlayerSource::Url { url, fetcher } => match RangedSource::open(fetcher, &url) {
                    Ok(src) => run_file(&s2, src, &url),
                    Err(e) => s2.fail(ERR_NETWORK, e),
                },
                PlayerSource::Bytes { data, mime } => run_file(&s2, RangedSource::from_bytes(data, &mime), ""),
                PlayerSource::Mse(h) => run_mse(&s2, h),
            }
            if let Some(src) = s2.source.lock().unwrap_or_else(|e| e.into_inner()).take() {
                src.close();
            }
        });
        if let Err(e) = spawned {
            shared.fail(ERR_DECODE, e.to_string());
        }
        Player { shared }
    }

    /// Current status (with the clock read now).
    pub fn status(&self) -> Status {
        let time = self.shared.time();
        let mut st = self.shared.status().clone();
        st.time = if st.duration.is_finite() { time.min(st.duration).max(0.0) } else { time.max(0.0) };
        if st.seeking {
            st.time = self.shared.ctl().seek.unwrap_or(st.time);
        }
        st
    }

    pub fn play(&self) {
        let mut c = self.shared.ctl();
        if !c.playing {
            c.playing = true;
            c.wall_at = Instant::now();
        }
        drop(c);
        self.shared.sink.set_playing(true);
        self.shared.status().ended = false;
        self.shared.cv.notify_all();
    }

    pub fn pause(&self) {
        let now = self.shared.time();
        let mut c = self.shared.ctl();
        if c.playing {
            c.playing = false;
            c.wall_time = now;
            c.wall_at = Instant::now();
        }
        drop(c);
        self.shared.sink.set_playing(false);
    }

    /// Seek to `t` seconds; [`Status::seeking`] stays true until data at
    /// the new position is ready.
    pub fn seek(&self, t: f64) {
        let t = t.max(0.0);
        {
            let mut c = self.shared.ctl();
            c.seek = Some(t);
            c.wall_time = t;
            c.wall_at = Instant::now();
        }
        self.shared.sink.flush(t);
        {
            let mut st = self.shared.status();
            st.seeking = true;
            st.ended = false;
        }
        self.shared.wake();
    }

    pub fn set_volume(&self, v: f64) {
        self.shared.sink.set_volume(v as f32);
    }

    pub fn set_muted(&self, m: bool) {
        self.shared.sink.set_muted(m);
    }

    pub fn set_rate(&self, r: f64) {
        let now = self.shared.time();
        let mut c = self.shared.ctl();
        c.wall_time = now;
        c.wall_at = Instant::now();
        c.rate = r;
        drop(c);
        self.shared.sink.set_speed(r);
    }

    /// Audio frames the mixer consumed from this player.
    pub fn frames_played(&self) -> u64 {
        self.shared.sink.frames_mixed()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.sink.set_playing(false);
        self.shared.wake();
    }
}

/// How much decoded audio to keep queued.
const AHEAD: f64 = 1.5;

fn secs(t: Time) -> f64 {
    t.as_secs_f64()
}

/// Codec parameters for an MSE track.
fn mse_audio_params(t: &TrackInfo) -> Option<AudioCodecParameters> {
    use symphonia::core::audio::Channels;
    use symphonia::core::codecs::audio::well_known::*;
    let codec = match t.codec.as_str() {
        "opus" => CODEC_ID_OPUS,
        "aac" => CODEC_ID_AAC,
        "mp3" => CODEC_ID_MP3,
        _ => return None,
    };
    let mut p = AudioCodecParameters::new();
    p.for_codec(codec);
    if t.sample_rate > 0 {
        p.with_sample_rate(t.sample_rate);
    }
    if t.channels > 0 {
        let ch = match t.channels {
            1 => Channels::Positioned(symphonia::core::audio::Position::FRONT_CENTER),
            2 => Channels::Positioned(symphonia::core::audio::Position::FRONT_LEFT | symphonia::core::audio::Position::FRONT_RIGHT),
            n => Channels::Discrete(n as u16),
        };
        p.with_channels(ch);
    }
    if !t.codec_private.is_empty() {
        p.with_extra_data(t.codec_private.clone().into_boxed_slice());
    }
    Some(p)
}

/// Queue decoded audio, trimming what precedes `from`.
fn push_pcm(shared: &Shared, pts: f64, mut pcm: Pcm, from: f64) -> bool {
    if pcm.frames() == 0 || pcm.rate == 0 {
        return false;
    }
    let mut pts = pts;
    if pts < from {
        let skip = ((from - pts) * pcm.rate as f64).round() as usize;
        if skip >= pcm.frames() {
            return false;
        }
        pcm.skip_frames(skip);
        pts += skip as f64 / pcm.rate as f64;
    }
    shared.sink.push(pts, pcm.rate, pcm.channels, &pcm.samples);
    shared.status().decoded_frames += pcm.frames() as u64;
    true
}

// ------------------------------------------------------------ direct files

fn run_file(shared: &Arc<Shared>, src: RangedSource, url: &str) {
    *shared.source.lock().unwrap_or_else(|e| e.into_inner()) = Some(src.clone());
    let mut hint = Hint::new();
    let ct = src.content_type();
    if !ct.is_empty() {
        hint.mime_type(&ct);
    }
    if let Some(ext) = url.split(['?', '#']).next().and_then(|p| p.rsplit('/').next()).and_then(|f| f.rsplit_once('.')).map(|(_, e)| e.to_ascii_lowercase()) {
        hint.with_extension(&ext);
    }
    let mss = MediaSourceStream::new(Box::new(src.reader()), MediaSourceStreamOptions::default());
    let mut format: Box<dyn FormatReader> = match symphonia::default::get_probe().probe(&hint, mss, FormatOptions::default(), MetadataOptions::default()) {
        Ok(f) => f,
        Err(e) => {
            let code = if src.error().is_some() { ERR_NETWORK } else { ERR_SRC_NOT_SUPPORTED };
            shared.fail(code, format!("unsupported media: {e}"));
            return;
        }
    };
    let tracks = format.tracks().to_vec();
    let audio = tracks.iter().find(|t| t.codec_params.as_ref().and_then(|p| p.audio()).is_some_and(|a| Decoder::new(a).is_ok()));
    let video = tracks.iter().find(|t| matches!(t.codec_params, Some(CodecParameters::Video(_))));
    let mut decoder = audio.and_then(|t| t.codec_params.as_ref()?.audio()).and_then(|a| Decoder::new(a).ok());
    let audio_id = audio.map(|t| t.id);
    let time_base = audio.and_then(|t| t.time_base);
    // Duration.
    let mut duration = f64::NAN;
    for t in audio.iter().chain(video.iter()) {
        if let (Some(tb), Some(d)) = (t.time_base, t.duration) {
            if let Some(time) = tb.calc_duration(d) {
                duration = secs(time);
                break;
            }
        }
        if let (Some(n), Some(rate)) = (t.num_frames, t.codec_params.as_ref().and_then(|p| p.audio()).and_then(|a| a.sample_rate)) {
            duration = n as f64 / rate as f64;
            break;
        }
    }
    if decoder.is_none() && video.is_none() {
        let codec = tracks.iter().filter_map(|t| t.codec_params.as_ref()?.audio()).map(crate::decode::codec_name).next().unwrap_or("none");
        shared.fail(ERR_SRC_NOT_SUPPORTED, format!("no supported audio or video track (audio codec: {codec})"));
        return;
    }
    {
        let mut st = shared.status();
        st.duration = duration;
        st.has_audio = decoder.is_some();
        st.has_video = video.is_some();
        if let Some(CodecParameters::Video(v)) = video.and_then(|t| t.codec_params.as_ref()) {
            st.video_width = v.width.unwrap_or(0) as u32;
            st.video_height = v.height.unwrap_or(0) as u32;
            st.video_codec = format!("{:?}", v.codec);
        }
        if let Some(a) = audio.and_then(|t| t.codec_params.as_ref()?.audio()) {
            st.audio_codec = crate::decode::codec_name(a).to_string();
        }
        st.ready = 1;
        if duration.is_finite() {
            st.seekable = vec![(0.0, duration)];
        }
    }
    shared.ctl().audio_clock = decoder.is_some();
    let mut eof = false;
    let mut trim_from = 0.0f64;
    let mut pushed_since_seek = false;
    let mut failures = 0;
    loop {
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
        // Seek.
        let seek = shared.ctl().seek.take();
        if let Some(t) = seek {
            src.clear_interrupt();
            let to = Time::try_from_secs_f64(t).unwrap_or(Time::ZERO);
            match format.seek(SeekMode::Accurate, SeekTo::Time { time: to, track_id: audio_id }) {
                Ok(_) => {}
                Err(e) => {
                    if t > 0.0 {
                        // Unseekable: restart from the beginning.
                        let _ = format.seek(SeekMode::Coarse, SeekTo::Time { time: Time::ZERO, track_id: audio_id });
                    }
                    let _ = e;
                }
            }
            if let Some(d) = decoder.as_mut() {
                d.reset(t == 0.0);
            }
            trim_from = t;
            eof = false;
            pushed_since_seek = false;
            if decoder.is_none() {
                finish_seek(shared);
            }
        }
        update_file_status(shared, &src, duration, eof);
        if decoder.is_none() {
            // Video only (picture not decoded): the wall clock runs.
            if shared.ctl().playing && duration.is_finite() && shared.time() >= duration {
                end_playback(shared, duration);
            }
            shared.nap(50);
            continue;
        }
        if eof {
            if shared.sink.drained() && shared.ctl().playing {
                end_playback(shared, duration);
            }
            shared.nap(30);
            continue;
        }
        if shared.sink.buffered_secs() > AHEAD && pushed_since_seek {
            shared.nap(20);
            continue;
        }
        match format.next_packet() {
            Ok(Some(p)) => {
                failures = 0;
                if Some(p.track_id) != audio_id {
                    continue;
                }
                let pts = time_base.and_then(|tb| tb.calc_time(p.pts)).map(secs).unwrap_or(0.0);
                let dur = time_base.and_then(|tb| tb.calc_duration(p.dur)).map(secs).unwrap_or(0.0);
                if pts + dur < trim_from - 0.5 {
                    continue;
                }
                let Some(dec) = decoder.as_mut() else { continue };
                match dec.decode(&p.data) {
                    Ok(pcm) => {
                        if push_pcm(shared, pts, pcm, trim_from) && !pushed_since_seek {
                            pushed_since_seek = true;
                            if shared.status().seeking {
                                finish_seek(shared);
                            }
                        }
                    }
                    Err(_) => {
                        failures += 1;
                        if failures > 50 {
                            shared.fail(ERR_DECODE, "too many decode errors".into());
                            return;
                        }
                    }
                }
            }
            Ok(None) => {
                eof = true;
                shared.sink.set_eos(true);
                if shared.status().seeking {
                    finish_seek(shared);
                }
            }
            Err(symphonia::core::errors::Error::ResetRequired) => {
                if let Some(a) = audio.and_then(|t| t.codec_params.as_ref()?.audio()) {
                    decoder = Decoder::new(a).ok();
                }
            }
            Err(symphonia::core::errors::Error::IoError(e)) => {
                if shared.stop.load(Ordering::SeqCst) {
                    return;
                }
                if e.to_string() == "interrupted" {
                    src.clear_interrupt();
                    continue;
                }
                if e.kind() == std::io::ErrorKind::UnexpectedEof {
                    eof = true;
                    shared.sink.set_eos(true);
                    if shared.status().seeking {
                        finish_seek(shared);
                    }
                    continue;
                }
                if let Some(err) = src.error() {
                    shared.fail(ERR_NETWORK, err);
                    return;
                }
                eof = true;
                shared.sink.set_eos(true);
            }
            Err(e) => {
                failures += 1;
                if failures > 20 || shared.status().decoded_frames == 0 {
                    shared.fail(ERR_DECODE, e.to_string());
                    return;
                }
            }
        }
    }
}

fn finish_seek(shared: &Shared) {
    let mut st = shared.status();
    st.seeking = false;
    st.seek_serial += 1;
}

fn end_playback(shared: &Shared, duration: f64) {
    let end = if duration.is_finite() { duration } else { shared.time() };
    {
        let mut c = shared.ctl();
        c.playing = false;
        c.wall_time = end;
        c.wall_at = Instant::now();
    }
    shared.sink.set_playing(false);
    shared.status().ended = true;
}

fn update_file_status(shared: &Shared, src: &RangedSource, duration: f64, eof: bool) {
    let buffered_audio = shared.sink.buffered_secs();
    let complete = src.complete();
    let total = src.total();
    let has_audio = shared.status().has_audio;
    let mut st = shared.status();
    st.idle = complete;
    if let Some(e) = src.error() {
        if st.error.is_none() && st.decoded_frames == 0 {
            st.error = Some((ERR_NETWORK, e));
        }
    }
    // Byte ranges → time ranges (proportional estimate).
    if let (Some(total), true) = (total, duration.is_finite()) {
        if total > 0 {
            let r: Vec<(f64, f64)> = src.buffered_bytes().iter().map(|&(s, e)| (s as f64 / total as f64 * duration, e as f64 / total as f64 * duration)).collect();
            st.buffered = crate::mse::merge(r, 0.0);
        }
    }
    if st.seeking {
        st.ready = st.ready.min(1).max(1);
        return;
    }
    st.ready = if !has_audio {
        4
    } else if eof || complete && buffered_audio > 0.2 || buffered_audio >= 1.0 {
        4
    } else if buffered_audio > 0.25 {
        3
    } else if st.decoded_frames > 0 {
        2
    } else {
        1
    };
}

// ------------------------------------------------------------------ MSE

fn run_mse(shared: &Arc<Shared>, mse: MseHandle) {
    let mut decoder: Option<(u64, String, Decoder)> = None;
    // Next media time to decode from.
    let mut cursor = 0.0f64;
    let mut pushed_since_seek = false;
    let mut ended_reported = false;
    let mut last_wall = Instant::now();
    loop {
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
        let seek = shared.ctl().seek.take();
        if let Some(t) = seek {
            cursor = t;
            pushed_since_seek = false;
            ended_reported = false;
            if let Some((_, _, d)) = decoder.as_mut() {
                d.reset(t == 0.0);
            }
        }
        let now = shared.time();
        // Status from the MediaSource.
        let (ready, has_audio, next_frame, ended_stream, duration) = {
            let mut m = mse.lock();
            m.position = now;
            let buffered = m.buffered();
            let duration = m.duration;
            let ended = m.ended;
            let all_init = m.all_init();
            let audio = m.audio_track();
            let video = m.video_track();
            let mut st = shared.status();
            st.duration = duration;
            st.buffered = buffered.clone();
            st.seekable = if duration.is_finite() { vec![(0.0, duration)] } else if let (Some(f), Some(l)) = (buffered.first(), buffered.last()) { vec![(f.0, l.1)] } else { Vec::new() };
            st.idle = ended;
            if let Some((i, j)) = audio {
                let t = &m.buffers[i].tracks[j].info;
                st.has_audio = true;
                st.audio_codec = t.codec.clone();
            }
            if let Some((i, j)) = video {
                let t = &m.buffers[i].tracks[j].info;
                st.has_video = true;
                st.video_codec = t.codec.clone();
                st.video_width = t.width;
                st.video_height = t.height;
            }
            let pos = if st.seeking { cursor } else { now };
            let in_range = |a: f64, b: f64| buffered.iter().any(|r| r.0 <= a + 1e-3 && r.1 >= b - 1e-3);
            let end_ok = |x: f64| ended && duration.is_finite() && x >= duration;
            let ready = if !all_init {
                0
            } else if !in_range(pos, pos) && !(ended && buffered.last().is_some_and(|r| pos >= r.1 - 0.05)) {
                1
            } else if in_range(pos, pos + 2.0) || end_ok(pos + 2.0) || ended {
                4
            } else if in_range(pos, pos + 0.15) {
                3
            } else {
                2
            };
            st.ready = if st.seeking && ready < 2 { 1 } else { ready };
            drop(st);
            // The next audio frame at the cursor.
            let next = audio.map(|(i, j)| {
                let tb = &m.buffers[i].tracks[j];
                let idx = tb.frames.partition_point(|f| f.pts + f.dur.max(1e-4) <= cursor + 1e-6);
                (tb.info.clone(), tb.frames.get(idx).cloned())
            });
            (ready, audio.is_some(), next, ended, duration)
        };
        shared.ctl().audio_clock = has_audio && decoder.is_some();
        if !has_audio {
            // Video only: a wall clock that stops when data runs out.
            let dt = last_wall.elapsed().as_secs_f64();
            last_wall = Instant::now();
            let mut c = shared.ctl();
            if c.playing && ready >= 3 {
                c.wall_time = shared_clamp(c.wall_time + dt * c.rate, duration);
            }
            c.wall_at = Instant::now();
            let playing = c.playing;
            let t = c.wall_time;
            drop(c);
            if shared.status().seeking && ready >= 2 {
                finish_seek(shared);
            }
            if playing && ended_stream && duration.is_finite() && t >= duration - 1e-3 && !ended_reported {
                ended_reported = true;
                end_playback(shared, duration);
            }
            shared.nap(15);
            continue;
        }
        last_wall = Instant::now();
        let Some((info, frame)) = next_frame else {
            shared.nap(20);
            continue;
        };
        // (Re)create the decoder for this track's configuration.
        let key = (info.id, info.codec.clone());
        if decoder.as_ref().is_none_or(|(id, c, _)| (*id, c.clone()) != key) {
            match mse_audio_params(&info).map(|p| Decoder::new(&p)) {
                Some(Ok(d)) => decoder = Some((info.id, info.codec.clone(), d)),
                _ => {
                    shared.fail(ERR_SRC_NOT_SUPPORTED, format!("unsupported audio codec {}", info.codec));
                    shared.nap(200);
                    continue;
                }
            }
            shared.ctl().audio_clock = true;
        }
        if pushed_since_seek && shared.sink.buffered_secs() > AHEAD {
            shared.nap(20);
            continue;
        }
        match frame {
            Some(f) if f.pts <= cursor + 0.1 => {
                let Some((_, _, dec)) = decoder.as_mut() else { continue };
                cursor = f.pts + f.dur.max(1e-4);
                if let Ok(pcm) = dec.decode(&f.data) {
                    let from = if pushed_since_seek { 0.0 } else { shared.ctl().wall_time };
                    if push_pcm(shared, f.pts, pcm, from) && !pushed_since_seek {
                        pushed_since_seek = true;
                    }
                }
                if pushed_since_seek && shared.status().seeking && shared.sink.buffered_secs() > 0.1 {
                    finish_seek(shared);
                }
            }
            _ => {
                // No data at the cursor yet (or the stream ended).
                if ended_stream {
                    shared.sink.set_eos(true);
                    if shared.status().seeking {
                        finish_seek(shared);
                    }
                    if shared.sink.drained() && shared.ctl().playing && !ended_reported {
                        ended_reported = true;
                        end_playback(shared, duration);
                    }
                } else {
                    shared.sink.set_eos(false);
                }
                shared.nap(20);
            }
        }
    }
}

fn shared_clamp(t: f64, duration: f64) -> f64 {
    if duration.is_finite() {
        t.min(duration)
    } else {
        t
    }
}

/// Codecs this build decodes (for `canPlayType` / `isTypeSupported`).
pub fn can_decode_audio(codec: &str) -> bool {
    matches!(codec, "opus" | "aac" | "mp3" | "vorbis" | "flac" | "pcm" | "alac")
}

#[cfg(test)]
mod tests;
