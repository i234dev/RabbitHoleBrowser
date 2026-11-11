//! Media Source Extensions state shared between the `MediaSource` /
//! `SourceBuffer` bindings (which append and remove) and the player
//! (which reads coded frames): the segment parser loop and the coded
//! frame processing / removal / eviction algorithms of the MSE spec.

use crate::demux::{self, CodedFrame, ParseEvent, SegmentParser, TrackInfo, TrackKind};
use std::sync::{Arc, Mutex, MutexGuard};

/// Byte budget per SourceBuffer before coded frame eviction (as Chrome:
/// 12 MB audio, 150 MB video).
const AUDIO_QUOTA: usize = 12 << 20;
const VIDEO_QUOTA: usize = 150 << 20;

/// Coded frames of one track, in decode order.
pub struct TrackBuffer {
    pub info: TrackInfo,
    pub frames: Vec<CodedFrame>,
    /// Bytes held.
    pub bytes: usize,
    max_dur: f64,
    /// Decode timestamp of the last frame appended in this group (`None`
    /// after a discontinuity).
    last_dts: Option<f64>,
    /// Frames from this point on were appended in the current coded frame
    /// group (overlap removal skips them).
    highest_end: Option<f64>,
    need_key: bool,
}

impl TrackBuffer {
    fn new(info: TrackInfo) -> TrackBuffer {
        TrackBuffer { info, frames: Vec::new(), bytes: 0, max_dur: 0.0, last_dts: None, highest_end: None, need_key: true }
    }

    fn tolerance(&self) -> f64 {
        (self.max_dur * 1.5).max(0.05)
    }

    /// Buffered presentation ranges of this track.
    pub fn ranges(&self) -> Vec<(f64, f64)> {
        let mut spans: Vec<(f64, f64)> = self.frames.iter().map(|f| (f.pts, f.pts + if f.dur > 0.0 { f.dur } else { self.max_dur })).collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        merge(spans, self.tolerance())
    }

    /// Insert one frame, removing what it overlaps.
    fn insert(&mut self, f: CodedFrame) {
        let end = f.pts + f.dur.max(0.0);
        // Overlap removal (MSE coded frame processing step 15): frames of
        // earlier appends presented within this frame's interval go;
        // frames of the current coded frame group (before the group's
        // highest end timestamp) stay.
        let tol = 1e-6;
        let range = match self.highest_end {
            None => Some((f.pts, end)),
            Some(h) if h <= f.pts + tol => Some((h, end)),
            Some(_) => None,
        };
        let mut removed_any = false;
        if let Some((rs, re)) = range {
            // Frames are in decode order; presentation order differs from
            // it by less than a few seconds.
            let mut i = self.frames.partition_point(|g| g.dts < rs - 5.0);
            while i < self.frames.len() {
                let g = &self.frames[i];
                if g.dts > re + 5.0 {
                    break;
                }
                if g.pts >= rs - tol && g.pts < re - tol {
                    self.bytes -= g.data.len();
                    self.frames.remove(i);
                    removed_any = true;
                    continue;
                }
                i += 1;
            }
        }
        if removed_any && self.info.kind == TrackKind::Video {
            // Frames depending on removed ones go too: drop non-key
            // frames after the insertion point up to the next key frame.
            let at = self.frames.partition_point(|g| g.dts < f.dts);
            let j = at;
            while j < self.frames.len() && !self.frames[j].key {
                self.bytes -= self.frames[j].data.len();
                self.frames.remove(j);
            }
        }
        if f.dur > self.max_dur && f.dur < 10.0 {
            self.max_dur = f.dur;
        }
        let at = self.frames.partition_point(|g| g.dts <= f.dts);
        // A frame without duration: infer it from its neighbour.
        if at > 0 && self.frames[at - 1].dur <= 0.0 {
            let d = f.pts - self.frames[at - 1].pts;
            if d > 0.0 && d < 10.0 {
                self.frames[at - 1].dur = d;
            }
        }
        self.bytes += f.data.len();
        self.last_dts = Some(f.dts);
        self.highest_end = Some(self.highest_end.map_or(end, |h: f64| h.max(end)));
        self.frames.insert(at, f);
    }

    /// Remove frames presented in [start, end) (plus dependent frames up
    /// to the next key frame for video).
    fn remove(&mut self, start: f64, end: f64) {
        let mut i = 0;
        let mut removed_key_run = false;
        while i < self.frames.len() {
            let f = &self.frames[i];
            if f.pts >= start && f.pts < end {
                self.bytes -= f.data.len();
                self.frames.remove(i);
                removed_key_run = self.info.kind == TrackKind::Video;
                continue;
            }
            if removed_key_run && f.pts >= end {
                if f.key {
                    removed_key_run = false;
                } else {
                    self.bytes -= f.data.len();
                    self.frames.remove(i);
                    continue;
                }
            }
            i += 1;
        }
        self.last_dts = None;
        self.need_key = true;
    }
}

/// Merge sorted spans whose gaps are below `tol`.
pub fn merge(spans: Vec<(f64, f64)>, tol: f64) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (s, e) in spans {
        match out.last_mut() {
            Some(last) if s <= last.1 + tol => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

/// Intersection of two range lists.
pub fn intersect(a: &[(f64, f64)], b: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for &(s1, e1) in a {
        for &(s2, e2) in b {
            let s = s1.max(s2);
            let e = e1.min(e2);
            if e > s {
                out.push((s, e));
            }
        }
    }
    out.sort_by(|x, y| x.0.total_cmp(&y.0));
    out
}

pub struct SourceBufferState {
    pub mime: String,
    parser: Box<dyn SegmentParser>,
    pub tracks: Vec<TrackBuffer>,
    pub got_init: bool,
    pub timestamp_offset: f64,
    pub append_window: (f64, f64),
    pub sequence_mode: bool,
    /// Sequence mode: where the next coded frame group starts.
    group_start: Option<f64>,
    group_end: f64,
    pub removed: bool,
    /// Generation counter: bumped when frames change (players re-read).
    pub version: u64,
}

/// Result of an append.
#[derive(Debug, Default)]
pub struct AppendResult {
    /// Initialization segments seen: (tracks, duration in the segment).
    pub inits: Vec<(Vec<TrackInfo>, Option<f64>)>,
    /// New `timestampOffset` (sequence mode / changes during append).
    pub timestamp_offset: f64,
    pub frames: usize,
}

impl SourceBufferState {
    pub fn new(mime: &str) -> Option<SourceBufferState> {
        Some(SourceBufferState {
            mime: mime.to_string(),
            parser: demux::parser_for(mime)?,
            tracks: Vec::new(),
            got_init: false,
            timestamp_offset: 0.0,
            append_window: (0.0, f64::INFINITY),
            sequence_mode: false,
            group_start: None,
            group_end: 0.0,
            removed: false,
            version: 0,
        })
    }

    /// `changeType()`: a new parser for a new MIME type.
    pub fn change_type(&mut self, mime: &str) -> bool {
        match demux::parser_for(mime) {
            Some(p) => {
                self.parser = p;
                self.mime = mime.to_string();
                true
            }
            None => false,
        }
    }

    /// The segment parser loop + coded frame processing over `bytes`.
    pub fn append(&mut self, bytes: &[u8]) -> Result<AppendResult, String> {
        let events = self.parser.push(bytes)?;
        let mut res = AppendResult::default();
        for ev in events {
            match ev {
                ParseEvent::Init { tracks, duration } => {
                    if self.got_init {
                        // Later init segments: keep buffers, refresh info
                        // (codec configuration may change).
                        for t in &tracks {
                            if let Some(tb) = self.tracks.iter_mut().find(|b| b.info.id == t.id || b.info.kind == t.kind) {
                                tb.info = t.clone();
                                tb.need_key = true;
                                tb.last_dts = None;
                            } else {
                                self.tracks.push(TrackBuffer::new(t.clone()));
                            }
                        }
                    } else {
                        self.tracks = tracks.iter().filter(|t| t.kind != TrackKind::Other).cloned().map(TrackBuffer::new).collect();
                        self.got_init = true;
                    }
                    res.inits.push((tracks, duration));
                }
                ParseEvent::Frame(f) => {
                    if self.process_frame(f) {
                        res.frames += 1;
                    }
                }
            }
        }
        if res.frames > 0 {
            self.version += 1;
        }
        res.timestamp_offset = self.timestamp_offset;
        Ok(res)
    }

    fn process_frame(&mut self, mut f: CodedFrame) -> bool {
        if self.sequence_mode {
            if self.group_start.is_none() {
                self.group_start = Some(self.group_end);
                self.timestamp_offset = self.group_end - f.pts;
            }
        }
        f.pts += self.timestamp_offset;
        f.dts += self.timestamp_offset;
        let Some(tb) = self.tracks.iter_mut().find(|t| t.info.id == f.track) else {
            // Track ids may change across init segments of the same kind.
            return false;
        };
        // Discontinuity: decode time going backwards or jumping ahead.
        if let Some(last) = tb.last_dts {
            if f.dts < last - 1e-6 || f.dts > last + 2.0 * tb.max_dur.max(0.1) + 1.0 {
                for t in self.tracks.iter_mut() {
                    t.last_dts = None;
                    t.highest_end = None;
                    t.need_key = true;
                }
                if self.sequence_mode {
                    self.group_start = None;
                }
            }
        }
        let Some(tb) = self.tracks.iter_mut().find(|t| t.info.id == f.track) else { return false };
        let end = f.pts + f.dur;
        if f.pts < self.append_window.0 || end > self.append_window.1 + 1e-6 {
            tb.need_key = true;
            return false;
        }
        if tb.need_key {
            if !f.key {
                return false;
            }
            tb.need_key = false;
        }
        if tb.last_dts.is_none() {
            tb.highest_end = None;
        }
        tb.insert(f);
        if end > self.group_end {
            self.group_end = end;
        }
        true
    }

    /// `abort()` / reset parser state.
    pub fn reset_parser(&mut self) {
        self.parser.reset();
        for t in &mut self.tracks {
            t.last_dts = None;
            t.need_key = true;
        }
        if self.sequence_mode {
            self.group_start = None;
        }
    }

    pub fn set_timestamp_offset(&mut self, v: f64) {
        self.timestamp_offset = v;
        if self.sequence_mode {
            self.group_start = None;
            self.group_end = v;
        }
    }

    pub fn set_mode(&mut self, sequence: bool) {
        self.sequence_mode = sequence;
        if sequence {
            self.group_end = self.timestamp_offset.max(self.group_end);
            self.group_start = None;
        }
    }

    pub fn parsing_media_segment(&self) -> bool {
        self.parser.mid_segment()
    }

    /// Buffered ranges: the intersection over its tracks.
    pub fn buffered(&self) -> Vec<(f64, f64)> {
        let mut it = self.tracks.iter().filter(|t| t.info.kind != TrackKind::Other);
        let Some(first) = it.next() else { return Vec::new() };
        let mut r = first.ranges();
        for t in it {
            r = intersect(&r, &t.ranges());
        }
        r
    }

    /// `remove(start, end)`.
    pub fn remove(&mut self, start: f64, end: f64) {
        for t in &mut self.tracks {
            t.remove(start, end);
        }
        self.version += 1;
    }

    pub fn bytes(&self) -> usize {
        self.tracks.iter().map(|t| t.bytes).sum()
    }

    /// Coded frame eviction before appending `incoming` bytes; `now` is
    /// the playback position. Returns false when nothing more can be
    /// evicted (QuotaExceededError).
    pub fn evict(&mut self, incoming: usize, now: f64) -> bool {
        let quota = if self.tracks.iter().any(|t| t.info.kind == TrackKind::Video) { VIDEO_QUOTA } else { AUDIO_QUOTA };
        if self.bytes() + incoming <= quota {
            return true;
        }
        // Drop data before the playback position, GOP by GOP.
        let limit = now - 1.0;
        loop {
            if self.bytes() + incoming <= quota {
                self.version += 1;
                return true;
            }
            let earliest = self.tracks.iter().filter_map(|t| t.frames.iter().map(|f| f.pts).reduce(f64::min)).reduce(f64::min);
            let Some(start) = earliest else { return false };
            if start >= limit {
                break;
            }
            // Next key frame after `start` + 1 s (or the limit).
            let mut cut = (start + 1.0).min(limit);
            for t in &self.tracks {
                if t.info.kind == TrackKind::Video {
                    if let Some(k) = t.frames.iter().filter(|f| f.key && f.pts > start).map(|f| f.pts).find(|&p| p >= cut.min(limit)) {
                        cut = k.min(limit);
                    }
                }
            }
            if cut <= start {
                break;
            }
            for t in &mut self.tracks {
                t.remove(start, cut);
            }
        }
        // Then data far after the playback position, from the end.
        loop {
            if self.bytes() + incoming <= quota {
                self.version += 1;
                return true;
            }
            let latest = self.tracks.iter().filter_map(|t| t.frames.iter().map(|f| f.pts).reduce(f64::max)).reduce(f64::max);
            let Some(last) = latest else { return false };
            let start = (last - 1.0).max(now + 10.0);
            if start >= last {
                return false;
            }
            for t in &mut self.tracks {
                t.remove(start, f64::INFINITY);
            }
        }
    }
}

/// A `MediaSource`: its SourceBuffers plus duration / ended state.
#[derive(Default)]
pub struct MseInner {
    pub buffers: Vec<SourceBufferState>,
    pub duration: f64,
    pub ended: bool,
    /// Bumped on every change the player should notice.
    pub version: u64,
    /// The playback position (set by the attached player; used by eviction).
    pub position: f64,
}

/// Shared handle to a MediaSource's state.
#[derive(Clone, Default)]
pub struct MseHandle(pub Arc<Mutex<MseInner>>);

impl MseHandle {
    pub fn new() -> MseHandle {
        MseHandle(Arc::new(Mutex::new(MseInner { duration: f64::NAN, ..Default::default() })))
    }

    pub fn lock(&self) -> MutexGuard<'_, MseInner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl MseInner {
    /// Buffered ranges of the MediaSource: intersection of the active
    /// SourceBuffers' ranges (extended to the highest end once ended).
    pub fn buffered(&self) -> Vec<(f64, f64)> {
        let active: Vec<&SourceBufferState> = self.buffers.iter().filter(|b| !b.removed).collect();
        if active.is_empty() {
            return Vec::new();
        }
        let lists: Vec<Vec<(f64, f64)>> = active.iter().map(|b| b.buffered()).collect();
        let highest = lists.iter().filter_map(|l| l.last().map(|r| r.1)).fold(0.0f64, f64::max);
        let mut out = vec![(0.0, highest)];
        for mut l in lists {
            if self.ended {
                if let Some(last) = l.last_mut() {
                    last.1 = highest;
                }
            }
            out = intersect(&out, &l);
        }
        out
    }

    /// Every SourceBuffer received an initialization segment.
    pub fn all_init(&self) -> bool {
        let active: Vec<&SourceBufferState> = self.buffers.iter().filter(|b| !b.removed).collect();
        !active.is_empty() && active.iter().all(|b| b.got_init)
    }

    /// The first audio track (index of SourceBuffer, track index).
    pub fn audio_track(&self) -> Option<(usize, usize)> {
        for (i, b) in self.buffers.iter().enumerate() {
            if b.removed {
                continue;
            }
            if let Some(j) = b.tracks.iter().position(|t| t.info.kind == TrackKind::Audio) {
                return Some((i, j));
            }
        }
        None
    }

    pub fn video_track(&self) -> Option<(usize, usize)> {
        for (i, b) in self.buffers.iter().enumerate() {
            if b.removed {
                continue;
            }
            if let Some(j) = b.tracks.iter().position(|t| t.info.kind == TrackKind::Video) {
                return Some((i, j));
            }
        }
        None
    }
}

/// Is this MIME type (with codecs) supported by `MediaSource.isTypeSupported`?
/// Video codecs are demuxed (their frames buffered) even when we cannot
/// decode the picture yet: the audio plays and the video shows the poster.
pub fn is_type_supported(mime: &str) -> bool {
    let (base, codecs) = split_mime(mime);
    let audio_codecs: &[&str] = match base.as_str() {
        "audio/webm" | "video/webm" => &["opus", "vorbis"],
        "audio/mp4" | "video/mp4" => &["aac", "mp3", "opus", "flac"],
        _ => return false,
    };
    let video_ok = base.starts_with("video/");
    let video_codecs: &[&str] = if base.ends_with("webm") { &["vp9", "vp8", "av1"] } else { &["h264", "av1", "vp9"] };
    for c in &codecs {
        let name = demux::codec_from_rfc6381(c);
        let ok = audio_codecs.contains(&name) || (video_ok && video_codecs.contains(&name));
        if !ok {
            return false;
        }
    }
    true
}

/// MIME type → (lowercased type/subtype, codecs list).
pub fn split_mime(mime: &str) -> (String, Vec<String>) {
    let mut parts = mime.split(';');
    let base = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let mut codecs = Vec::new();
    for p in parts {
        let p = p.trim();
        if let Some((k, v)) = p.split_once('=') {
            if k.trim().eq_ignore_ascii_case("codecs") {
                let v = v.trim().trim_matches('"').trim_matches('\'');
                codecs.extend(v.split(',').map(|c| c.trim().to_string()).filter(|c| !c.is_empty()));
            }
        }
    }
    (base, codecs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demux::mp4::build as mb;
    use crate::demux::webm::build as wb;

    fn opus_init() -> Vec<u8> {
        wb::audio_init("A_OPUS", b"OpusHead\x01\x02\x38\x01\x80\xbb\0\0\0\0\0", 48000.0, 2, None)
    }

    fn toc() -> Vec<u8> {
        vec![(31u8 << 3) | 0b100]
    }

    #[test]
    fn webm_append_remove_and_ranges() {
        let mut sb = SourceBufferState::new("audio/webm; codecs=\"opus\"").unwrap();
        let t = toc();
        let r = sb.append(&opus_init()).unwrap();
        assert_eq!(r.inits.len(), 1);
        let blocks: Vec<(i16, &[u8])> = (0..50).map(|i| ((i * 20) as i16, &t[..])).collect();
        sb.append(&wb::cluster(0, &blocks)).unwrap();
        let b = sb.buffered();
        assert_eq!(b.len(), 1);
        assert!((b[0].1 - 1.0).abs() < 1e-6, "{b:?}");
        // A second cluster after a gap.
        sb.append(&wb::cluster(3000, &blocks)).unwrap();
        assert_eq!(sb.buffered().len(), 2);
        // Overlapping re-append replaces frames (no duplicates).
        sb.append(&wb::cluster(0, &blocks)).unwrap();
        assert_eq!(sb.tracks[0].frames.len(), 100);
        sb.remove(0.5, 3.5);
        let b = sb.buffered();
        assert_eq!(b.len(), 2, "{b:?}");
        assert!((b[0].1 - 0.5).abs() < 1e-6 && (b[1].0 - 3.5).abs() < 1e-6, "{b:?}");
        // timestampOffset shifts.
        sb.set_timestamp_offset(10.0);
        sb.append(&wb::cluster(0, &blocks)).unwrap();
        assert!(sb.buffered().iter().any(|r| (r.0 - 10.0).abs() < 1e-6));
    }

    #[test]
    fn mp4_append_and_append_window() {
        let mut sb = SourceBufferState::new("audio/mp4; codecs=\"mp4a.40.2\"").unwrap();
        sb.append(&mb::aac_init(44100, 4, 2)).unwrap();
        let samples: Vec<Vec<u8>> = (0..43).map(|_| vec![1u8; 20]).collect();
        sb.append(&mb::fragment(1, 0, &samples)).unwrap();
        let b = sb.buffered();
        assert!((b[0].1 - 43.0 * 1024.0 / 44100.0).abs() < 1e-6);
        sb.append_window = (0.0, 0.5);
        sb.remove(0.0, 100.0);
        sb.append(&mb::fragment(2, 0, &samples)).unwrap();
        assert!(sb.buffered()[0].1 <= 0.5 + 1e-9);
    }

    #[test]
    fn sequence_mode_places_groups_back_to_back() {
        let mut sb = SourceBufferState::new("audio/webm").unwrap();
        sb.set_mode(true);
        let t = toc();
        sb.append(&opus_init()).unwrap();
        let blocks: Vec<(i16, &[u8])> = (0..10).map(|i| ((i * 20) as i16, &t[..])).collect();
        sb.append(&wb::cluster(5000, &blocks)).unwrap();
        sb.reset_parser();
        sb.append(&wb::cluster(9000, &blocks)).unwrap();
        let b = sb.buffered();
        assert_eq!(b.len(), 1, "{b:?}");
        assert!(b[0].0.abs() < 1e-6 && (b[0].1 - 0.4).abs() < 1e-6, "{b:?}");
    }

    #[test]
    fn type_support() {
        assert!(is_type_supported("audio/webm; codecs=\"opus\""));
        assert!(is_type_supported("audio/mp4; codecs=\"mp4a.40.2\""));
        assert!(is_type_supported("video/webm; codecs=\"vp9\""));
        assert!(is_type_supported("video/mp4; codecs=\"avc1.4d401f, mp4a.40.2\""));
        assert!(!is_type_supported("audio/webm; codecs=\"aac\""));
        assert!(!is_type_supported("video/x-flv"));
        assert!(!is_type_supported("audio/mp4; codecs=\"ec-3\""));
    }

    #[test]
    fn eviction_drops_played_data() {
        let mut sb = SourceBufferState::new("audio/mp4").unwrap();
        sb.append(&mb::aac_init(44100, 4, 2)).unwrap();
        let big: Vec<Vec<u8>> = (0..400).map(|_| vec![0u8; 40_000]).collect();
        sb.append(&mb::fragment(1, 0, &big)).unwrap();
        assert!(sb.bytes() > AUDIO_QUOTA);
        assert!(sb.evict(1000, 8.0));
        assert!(sb.bytes() + 1000 <= AUDIO_QUOTA);
        // Data around the playback position survives.
        assert!(sb.buffered().iter().any(|r| r.0 <= 8.0 && r.1 > 8.0));
    }
}
