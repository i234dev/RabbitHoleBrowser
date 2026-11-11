//! Incremental WebM / Matroska byte-stream parser (MSE "webm" byte
//! stream format): EBML header + Segment Info / Tracks form the
//! initialization segment; Clusters of SimpleBlocks / BlockGroups are
//! media segments. Masters (Segment, Cluster) are entered without
//! waiting for their full size, so a cluster is parsed block by block as
//! it arrives; large elements we do not need (Cues, Tags, ...) are skipped
//! as their bytes go by.

use super::{opus_packet_duration, CodedFrame, ParseEvent, SegmentParser, TrackInfo, TrackKind};

const EBML: u32 = 0x1A45DFA3;
const SEGMENT: u32 = 0x18538067;
const INFO: u32 = 0x1549A966;
const TIMESTAMP_SCALE: u32 = 0x2AD7B1;
const DURATION: u32 = 0x4489;
const TRACKS: u32 = 0x1654AE6B;
const TRACK_ENTRY: u32 = 0xAE;
const TRACK_NUMBER: u32 = 0xD7;
const TRACK_TYPE: u32 = 0x83;
const CODEC_ID: u32 = 0x86;
const CODEC_PRIVATE: u32 = 0x63A2;
const DEFAULT_DURATION: u32 = 0x23E383;
const VIDEO: u32 = 0xE0;
const PIXEL_WIDTH: u32 = 0xB0;
const PIXEL_HEIGHT: u32 = 0xBA;
const AUDIO: u32 = 0xE1;
const SAMPLING_FREQUENCY: u32 = 0xB5;
const CHANNELS: u32 = 0x9F;
const CONTENT_ENCODINGS: u32 = 0x6D80;
const CLUSTER: u32 = 0x1F43B675;
const CLUSTER_TIMESTAMP: u32 = 0xE7;
const SIMPLE_BLOCK: u32 = 0xA3;
const BLOCK_GROUP: u32 = 0xA0;
const BLOCK: u32 = 0xA1;
const BLOCK_DURATION: u32 = 0x9B;
const REFERENCE_BLOCK: u32 = 0xFB;

pub struct WebmParser {
    buf: Vec<u8>,
    /// Bytes of an element being skipped that have not arrived yet.
    skip: u64,
    /// Nanoseconds per timestamp tick.
    timescale: u64,
    cluster_time: Option<i64>,
    tracks: Vec<TrackInfo>,
    have_init: bool,
    in_cluster: bool,
    /// Segment duration from Info, reported with the Tracks.
    duration: Option<f64>,
}

impl Default for WebmParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Read an EBML variable-length integer: (value, length). `id` keeps the
/// marker bits (element IDs); sizes drop them. All-ones size = unknown.
fn vint(b: &[u8], id: bool) -> Option<(u64, usize)> {
    let first = *b.first()?;
    if first == 0 {
        return Some((u64::MAX, 0)); // invalid
    }
    let len = first.leading_zeros() as usize + 1;
    if b.len() < len {
        return None;
    }
    let mut v: u64 = if id { first as u64 } else { (first as u64) & ((1u64 << (8 - len)) - 1) };
    let mut all_ones = v == (1u64 << (8 - len)) - 1;
    for &x in &b[1..len] {
        v = (v << 8) | x as u64;
        all_ones &= x == 0xff;
    }
    if !id && all_ones {
        return Some((u64::MAX, len));
    }
    Some((v, len))
}

/// Element header at the start of `b`: (id, data size or MAX, header length).
fn header(b: &[u8]) -> Result<Option<(u32, u64, usize)>, String> {
    let Some((id, il)) = vint(b, true) else { return Ok(None) };
    if il == 0 || il > 4 {
        return Err("invalid EBML element id".into());
    }
    let Some((size, sl)) = vint(&b[il..], false) else { return Ok(None) };
    if sl == 0 {
        return Err("invalid EBML element size".into());
    }
    Ok(Some((id as u32, size, il + sl)))
}

fn uint(b: &[u8]) -> u64 {
    b.iter().fold(0u64, |a, &x| (a << 8) | x as u64)
}

fn float(b: &[u8]) -> f64 {
    match b.len() {
        4 => f32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f64,
        8 => f64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
        _ => 0.0,
    }
}

/// Iterate the children of a fully buffered master element.
fn children(mut b: &[u8]) -> Vec<(u32, &[u8])> {
    let mut out = Vec::new();
    while let Ok(Some((id, size, hl))) = header(b) {
        if size == u64::MAX || hl as u64 + size > b.len() as u64 {
            break;
        }
        let end = hl + size as usize;
        out.push((id, &b[hl..end]));
        b = &b[end..];
    }
    out
}

fn codec_name(id: &str) -> String {
    match id {
        "A_OPUS" => "opus",
        "A_VORBIS" => "vorbis",
        "A_FLAC" => "flac",
        "A_AAC" | "A_AAC/MPEG4/LC" | "A_AAC/MPEG2/LC" => "aac",
        "A_MPEG/L3" => "mp3",
        "A_AC3" => "ac3",
        "A_EAC3" => "eac3",
        "A_PCM/INT/LIT" => "pcm",
        "V_AV1" => "av1",
        "V_VP9" => "vp9",
        "V_VP8" => "vp8",
        "V_MPEG4/ISO/AVC" => "h264",
        "V_MPEGH/ISO/HEVC" => "hevc",
        other => return other.to_string(),
    }
    .to_string()
}

fn parse_track(b: &[u8]) -> Option<TrackInfo> {
    let mut t = TrackInfo::new(0, TrackKind::Other);
    for (id, d) in children(b) {
        match id {
            TRACK_NUMBER => t.id = uint(d),
            TRACK_TYPE => {
                t.kind = match uint(d) {
                    1 => TrackKind::Video,
                    2 => TrackKind::Audio,
                    _ => TrackKind::Other,
                }
            }
            CODEC_ID => t.codec = codec_name(String::from_utf8_lossy(d).trim_end_matches('\0')),
            CODEC_PRIVATE => t.codec_private = d.to_vec(),
            DEFAULT_DURATION => t.default_duration = Some(uint(d) as f64 / 1e9),
            CONTENT_ENCODINGS => t.encrypted = true,
            VIDEO => {
                for (id, d) in children(d) {
                    match id {
                        PIXEL_WIDTH => t.width = uint(d) as u32,
                        PIXEL_HEIGHT => t.height = uint(d) as u32,
                        _ => {}
                    }
                }
            }
            AUDIO => {
                for (id, d) in children(d) {
                    match id {
                        SAMPLING_FREQUENCY => t.sample_rate = float(d) as u32,
                        CHANNELS => t.channels = uint(d) as u32,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if t.kind == TrackKind::Audio {
        if t.channels == 0 {
            t.channels = 1;
        }
        if t.sample_rate == 0 {
            t.sample_rate = 8000;
        }
    }
    Some(t)
}

impl WebmParser {
    pub fn new() -> WebmParser {
        WebmParser { buf: Vec::new(), skip: 0, timescale: 1_000_000, cluster_time: None, tracks: Vec::new(), have_init: false, in_cluster: false, duration: None }
    }

    fn ts(&self, ticks: i64) -> f64 {
        ticks as f64 * self.timescale as f64 / 1e9
    }

    /// Frames of a (Simple)Block payload.
    fn block(&self, d: &[u8], simple: bool, duration: Option<u64>, has_reference: bool, out: &mut Vec<ParseEvent>) -> Result<(), String> {
        let Some((track, tl)) = vint(d, false) else { return Err("truncated block".into()) };
        if d.len() < tl + 3 {
            return Err("truncated block".into());
        }
        let rel = i16::from_be_bytes([d[tl], d[tl + 1]]) as i64;
        let flags = d[tl + 2];
        let key = if simple { flags & 0x80 != 0 } else { !has_reference };
        let Some(info) = self.tracks.iter().find(|t| t.id == track) else { return Ok(()) };
        let Some(cluster) = self.cluster_time else { return Err("block outside a cluster".into()) };
        let payload = &d[tl + 3..];
        let laces = split_laces(payload, (flags >> 1) & 3)?;
        let mut pts = self.ts(cluster + rel);
        let total = duration.map(|t| self.ts(t as i64));
        let n = laces.len();
        for lace in laces {
            let dur = if let (Some(t), 1) = (total, n) {
                t
            } else if let Some(dd) = info.default_duration {
                dd
            } else if info.codec == "opus" {
                opus_packet_duration(lace).unwrap_or(0.02)
            } else if let Some(t) = total {
                t / n as f64
            } else {
                0.0
            };
            out.push(ParseEvent::Frame(CodedFrame { track, pts, dts: pts, dur, key: key || info.kind == TrackKind::Audio, data: lace.to_vec().into() }));
            pts += dur;
        }
        Ok(())
    }
}

/// Split a block payload by its lacing (0 none, 1 Xiph, 2 fixed, 3 EBML).
fn split_laces(p: &[u8], lacing: u8) -> Result<Vec<&[u8]>, String> {
    if lacing == 0 {
        return Ok(vec![p]);
    }
    let count = *p.first().ok_or("truncated lacing")? as usize + 1;
    let mut pos = 1usize;
    let mut sizes: Vec<usize> = Vec::with_capacity(count);
    match lacing {
        1 => {
            for _ in 0..count - 1 {
                let mut s = 0usize;
                loop {
                    let b = *p.get(pos).ok_or("truncated lacing")?;
                    pos += 1;
                    s += b as usize;
                    if b != 255 {
                        break;
                    }
                }
                sizes.push(s);
            }
        }
        2 => {
            let each = (p.len() - 1) / count;
            sizes.extend(std::iter::repeat_n(each, count - 1));
        }
        _ => {
            let (first, l) = vint(&p[pos..], false).ok_or("truncated lacing")?;
            pos += l;
            sizes.push(first as usize);
            let mut last = first as i64;
            for _ in 1..count - 1 {
                let (raw, l) = vint(&p[pos..], false).ok_or("truncated lacing")?;
                pos += l;
                let bias = (1i64 << (7 * l - 1)) - 1;
                last += raw as i64 - bias;
                if last < 0 {
                    return Err("bad EBML lacing".into());
                }
                sizes.push(last as usize);
            }
        }
    }
    let used: usize = sizes.iter().sum();
    if pos + used > p.len() {
        return Err("bad lacing sizes".into());
    }
    sizes.push(p.len() - pos - used);
    let mut out = Vec::with_capacity(count);
    for s in sizes {
        out.push(&p[pos..pos + s]);
        pos += s;
    }
    Ok(out)
}

impl SegmentParser for WebmParser {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<ParseEvent>, String> {
        let mut out = Vec::new();
        let mut input = bytes;
        if self.skip > 0 {
            let n = (self.skip as usize).min(input.len());
            self.skip -= n as u64;
            input = &input[n..];
        }
        self.buf.extend_from_slice(input);
        let mut pos = 0usize;
        loop {
            let b = &self.buf[pos..];
            let Some((id, size, hl)) = header(b)? else { break };
            match id {
                // Masters entered in place.
                SEGMENT => {
                    pos += hl;
                    self.in_cluster = false;
                    continue;
                }
                CLUSTER => {
                    pos += hl;
                    self.in_cluster = true;
                    self.cluster_time = None;
                    continue;
                }
                _ => {}
            }
            let needed = matches!(id, EBML | INFO | TRACKS | CLUSTER_TIMESTAMP | SIMPLE_BLOCK | BLOCK_GROUP);
            if size == u64::MAX {
                if needed {
                    return Err(format!("unknown-size element {id:#x}"));
                }
                // Unknown size we cannot skip: treat as a master.
                pos += hl;
                continue;
            }
            let total = hl as u64 + size;
            if !needed {
                // Skip (possibly across future appends).
                if (b.len() as u64) < total {
                    self.skip = total - b.len() as u64;
                    pos = self.buf.len();
                    break;
                }
                pos += total as usize;
                continue;
            }
            if (b.len() as u64) < total {
                break;
            }
            let d = &b[hl..total as usize];
            match id {
                EBML => {
                    // A new initialization segment starts.
                    self.in_cluster = false;
                }
                INFO => {
                    let mut duration = None;
                    for (cid, cd) in children(d) {
                        match cid {
                            TIMESTAMP_SCALE => self.timescale = uint(cd).max(1),
                            DURATION => duration = Some(float(cd)),
                            _ => {}
                        }
                    }
                    if let Some(dur) = duration {
                        let secs = dur * self.timescale as f64 / 1e9;
                        // Reported with the Tracks event.
                        self.duration = Some(secs);
                    }
                }
                TRACKS => {
                    let tracks: Vec<TrackInfo> = children(d).into_iter().filter(|(cid, _)| *cid == TRACK_ENTRY).filter_map(|(_, td)| parse_track(td)).collect();
                    self.tracks = tracks.clone();
                    self.have_init = true;
                    let duration = self.duration.take();
                    out.push(ParseEvent::Init { tracks, duration });
                }
                CLUSTER_TIMESTAMP => self.cluster_time = Some(uint(d) as i64),
                SIMPLE_BLOCK => {
                    if !self.have_init {
                        return Err("media segment before initialization segment".into());
                    }
                    let d = d.to_vec();
                    self.block(&d, true, None, false, &mut out)?;
                }
                BLOCK_GROUP => {
                    if !self.have_init {
                        return Err("media segment before initialization segment".into());
                    }
                    let mut block: Option<&[u8]> = None;
                    let mut duration = None;
                    let mut reference = false;
                    for (cid, cd) in children(d) {
                        match cid {
                            BLOCK => block = Some(cd),
                            BLOCK_DURATION => duration = Some(uint(cd)),
                            REFERENCE_BLOCK => reference = true,
                            _ => {}
                        }
                    }
                    if let Some(bd) = block {
                        let bd = bd.to_vec();
                        self.block(&bd, false, duration, reference, &mut out)?;
                    }
                }
                _ => {}
            }
            pos += total as usize;
        }
        self.buf.drain(..pos.min(self.buf.len()));
        Ok(out)
    }

    fn reset(&mut self) {
        self.buf.clear();
        self.skip = 0;
        self.in_cluster = false;
    }

    fn mid_segment(&self) -> bool {
        self.in_cluster
    }
}

/// Build WebM bytes (tests and generated fixtures).
pub mod build {
    pub fn size(n: u64) -> Vec<u8> {
        // 8-byte size form.
        let mut v = vec![0x01];
        v.extend_from_slice(&n.to_be_bytes()[1..]);
        v
    }

    pub fn el(id: u32, data: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        let idb = id.to_be_bytes();
        let skip = idb.iter().position(|&b| b != 0).unwrap_or(3);
        v.extend_from_slice(&idb[skip..]);
        v.extend(size(data.len() as u64));
        v.extend_from_slice(data);
        v
    }

    pub fn uint(id: u32, n: u64) -> Vec<u8> {
        el(id, &n.to_be_bytes())
    }

    pub fn float(id: u32, f: f64) -> Vec<u8> {
        el(id, &f.to_be_bytes())
    }

    pub fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
        parts.concat()
    }

    /// An init segment with one audio track (track number 1).
    pub fn audio_init(codec: &str, private: &[u8], rate: f64, channels: u64, duration_ms: Option<f64>) -> Vec<u8> {
        let ebml = el(super::EBML, &cat(&[el(0x4282, b"webm")]));
        let mut info = vec![uint(super::TIMESTAMP_SCALE, 1_000_000)];
        if let Some(d) = duration_ms {
            info.push(float(super::DURATION, d));
        }
        let info = el(super::INFO, &cat(&info));
        let audio = el(super::AUDIO, &cat(&[float(super::SAMPLING_FREQUENCY, rate), uint(super::CHANNELS, channels)]));
        let entry = el(super::TRACK_ENTRY, &cat(&[uint(super::TRACK_NUMBER, 1), uint(super::TRACK_TYPE, 2), el(super::CODEC_ID, codec.as_bytes()), el(super::CODEC_PRIVATE, private), audio]));
        let tracks = el(super::TRACKS, &entry);
        let mut seg = Vec::new();
        seg.extend_from_slice(&[0x18, 0x53, 0x80, 0x67, 0x01, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
        seg.extend(info);
        seg.extend(tracks);
        cat(&[ebml, seg])
    }

    /// A cluster at `time_ms` with SimpleBlocks (track 1) at the given
    /// relative times.
    pub fn cluster(time_ms: u64, blocks: &[(i16, &[u8])]) -> Vec<u8> {
        let mut parts = vec![uint(super::CLUSTER_TIMESTAMP, time_ms)];
        for (rel, data) in blocks {
            let mut b = vec![0x81];
            b.extend_from_slice(&rel.to_be_bytes());
            b.push(0x80);
            b.extend_from_slice(data);
            parts.push(el(super::SIMPLE_BLOCK, &b));
        }
        el(super::CLUSTER, &cat(&parts))
    }
}

#[cfg(test)]
mod tests {
    use super::build::*;
    use super::*;

    #[test]
    fn parses_init_and_clusters_incrementally() {
        let init = audio_init("A_OPUS", b"OpusHead\x01\x02\x38\x01\x80\xbb\0\0\0\0\0", 48000.0, 2, Some(2000.0));
        let toc = [(31u8 << 3) | 0b100];
        let c1 = cluster(0, &[(0, &toc), (20, &toc), (40, &toc)]);
        let c2 = cluster(60, &[(0, &toc), (20, &toc)]);
        let all = cat(&[init, c1, c2]);
        // Feed one byte at a time: same result as one append.
        let mut p = WebmParser::new();
        let mut events = Vec::new();
        for b in all.chunks(7) {
            events.extend(p.push(b).unwrap());
        }
        let mut p2 = WebmParser::new();
        let events2 = p2.push(&all).unwrap();
        assert_eq!(events.len(), events2.len());
        match &events[0] {
            ParseEvent::Init { tracks, duration } => {
                assert_eq!(tracks.len(), 1);
                assert_eq!(tracks[0].codec, "opus");
                assert_eq!(tracks[0].channels, 2);
                assert_eq!(tracks[0].sample_rate, 48000);
                assert!(tracks[0].codec_private.starts_with(b"OpusHead"));
                assert_eq!(*duration, Some(2.0));
            }
            e => panic!("{e:?}"),
        }
        let frames: Vec<&CodedFrame> = events.iter().filter_map(|e| if let ParseEvent::Frame(f) = e { Some(f) } else { None }).collect();
        assert_eq!(frames.len(), 5);
        assert!((frames[4].pts - 0.08).abs() < 1e-9);
        assert!((frames[0].dur - 0.02).abs() < 1e-9);
        assert!(frames.iter().all(|f| f.key));
    }

    #[test]
    fn lacing() {
        // Xiph lacing: 3 frames of sizes 2, 3, rest.
        let p = [2u8, 2, 3, 1, 1, 2, 2, 2, 3, 3, 3, 3];
        let l = split_laces(&p, 1).unwrap();
        assert_eq!(l, vec![&[1u8, 1][..], &[2, 2, 2][..], &[3, 3, 3, 3][..]]);
        // Fixed lacing: 2 frames.
        let l = split_laces(&[1, 9, 9, 8, 8], 2).unwrap();
        assert_eq!(l, vec![&[9u8, 9][..], &[8, 8][..]]);
    }
}
