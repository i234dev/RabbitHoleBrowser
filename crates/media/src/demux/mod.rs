//! Incremental segment parsers for Media Source Extensions (WebM /
//! Matroska and fragmented MP4 byte streams): bytes go in as they are
//! appended, track descriptions and coded frames come out.

pub mod mp4;
pub mod webm;

use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    Audio,
    Video,
    Other,
}

/// A track described by an initialization segment.
#[derive(Clone, Debug)]
pub struct TrackInfo {
    /// Track number / track ID in the container.
    pub id: u64,
    pub kind: TrackKind,
    /// Normalized codec name: "opus", "vorbis", "aac", "mp3", "flac",
    /// "ac3", "eac3", "av1", "vp9", "vp8", "h264", "hevc" or the raw
    /// container codec id when unknown.
    pub codec: String,
    /// Codec configuration: OpusHead, AudioSpecificConfig, Vorbis /
    /// FLAC headers, avcC / av1C / vpcC payloads.
    pub codec_private: Vec<u8>,
    pub sample_rate: u32,
    pub channels: u32,
    pub width: u32,
    pub height: u32,
    /// Seconds, when the container gives a default frame duration.
    pub default_duration: Option<f64>,
    /// Encrypted (EME) content we cannot decode.
    pub encrypted: bool,
}

impl TrackInfo {
    pub fn new(id: u64, kind: TrackKind) -> TrackInfo {
        TrackInfo { id, kind, codec: String::new(), codec_private: Vec::new(), sample_rate: 0, channels: 0, width: 0, height: 0, default_duration: None, encrypted: false }
    }
}

/// One coded frame (an access unit).
#[derive(Clone, Debug)]
pub struct CodedFrame {
    pub track: u64,
    /// Presentation / decode timestamps and duration in seconds.
    pub pts: f64,
    pub dts: f64,
    pub dur: f64,
    pub key: bool,
    pub data: Arc<[u8]>,
}

#[derive(Debug)]
pub enum ParseEvent {
    /// An initialization segment: the tracks and the duration it states.
    Init { tracks: Vec<TrackInfo>, duration: Option<f64> },
    Frame(CodedFrame),
}

/// A byte-stream parser fed by `appendBuffer`.
pub trait SegmentParser: Send {
    /// Parse what can be parsed from `bytes` (plus what was left over by
    /// previous calls). Returns the events; an error is a parse error
    /// (the append fails with a decode error).
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<ParseEvent>, String>;
    /// Drop any partially parsed data (`abort()` / `resetParserState`).
    fn reset(&mut self);
    /// Is the parser in the middle of a media segment?
    fn mid_segment(&self) -> bool;
}

/// The parser for a MIME type (`audio/webm`, `video/mp4`, ...).
pub fn parser_for(mime: &str) -> Option<Box<dyn SegmentParser>> {
    let m = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match m.as_str() {
        "audio/webm" | "video/webm" | "audio/x-matroska" | "video/x-matroska" => Some(Box::new(webm::WebmParser::new())),
        "audio/mp4" | "video/mp4" | "audio/x-m4a" => Some(Box::new(mp4::Mp4Parser::new())),
        _ => None,
    }
}

/// Duration of an Opus packet in seconds (RFC 6716 §3.1), for frames
/// whose container gives none.
pub fn opus_packet_duration(packet: &[u8]) -> Option<f64> {
    let toc = *packet.first()?;
    let config = toc >> 3;
    let frame_ms: f64 = match config {
        0..=11 => [10.0, 20.0, 40.0, 60.0][(config % 4) as usize],
        12..=15 => [10.0, 20.0][(config % 2) as usize],
        _ => [2.5, 5.0, 10.0, 20.0][(config % 4) as usize],
    };
    let frames = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => (*packet.get(1)? & 0x3f) as u32,
    };
    Some(frame_ms * frames as f64 / 1000.0)
}

/// Codec names for an RFC 6381 `codecs` parameter entry.
pub fn codec_from_rfc6381(c: &str) -> &'static str {
    let c = c.trim().trim_matches('"').to_ascii_lowercase();
    let head = c.split('.').next().unwrap_or("");
    match head {
        "opus" => "opus",
        "vorbis" => "vorbis",
        "flac" | "flac " => "flac",
        "mp4a" => {
            // mp4a.40.x = AAC, mp4a.69 / mp4a.6b / mp4a.40.34 = MP3
            if c == "mp4a.69" || c == "mp4a.6b" || c == "mp4a.40.34" {
                "mp3"
            } else if c.starts_with("mp4a.a5") || c.starts_with("mp4a.a6") {
                "ac3"
            } else {
                "aac"
            }
        }
        "mp3" => "mp3",
        "ac-3" => "ac3",
        "ec-3" => "eac3",
        "av01" => "av1",
        "vp09" | "vp9" => "vp9",
        "vp08" | "vp8" => "vp8",
        "avc1" | "avc3" => "h264",
        "hev1" | "hvc1" => "hevc",
        "pcm" | "1" => "pcm",
        _ => "",
    }
}
