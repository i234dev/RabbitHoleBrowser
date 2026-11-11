//! Incremental fragmented-MP4 byte-stream parser (MSE "mp4" byte stream
//! format, ISO/IEC 14496-12): `moov` is the initialization segment,
//! `moof` + `mdat` pairs are media segments.

use super::{CodedFrame, ParseEvent, SegmentParser, TrackInfo, TrackKind};

#[derive(Clone, Default)]
struct TrackState {
    id: u32,
    timescale: u32,
    /// `trex` defaults.
    def_duration: u32,
    def_size: u32,
    def_flags: u32,
    /// Edit-list start offset (media time units) subtracted from times.
    edit_offset: i64,
    kind: Option<TrackKind>,
}

/// A sample located by a `moof`, waiting for its `mdat` bytes.
struct PendingSample {
    track: u32,
    /// Absolute stream offset of the sample data.
    offset: u64,
    size: u32,
    dts: i64,
    cto: i64,
    dur: u32,
    key: bool,
}

pub struct Mp4Parser {
    buf: Vec<u8>,
    /// Absolute stream offset of `buf[0]`.
    base: u64,
    tracks: Vec<TrackState>,
    pending: Vec<PendingSample>,
    have_init: bool,
    skip: u64,
}

impl Default for Mp4Parser {
    fn default() -> Self {
        Self::new()
    }
}

fn be16(b: &[u8], i: usize) -> u16 {
    u16::from_be_bytes([b[i], b[i + 1]])
}
fn be32(b: &[u8], i: usize) -> u32 {
    u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}
fn be64(b: &[u8], i: usize) -> u64 {
    u64::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3], b[i + 4], b[i + 5], b[i + 6], b[i + 7]])
}

/// Box header at `b`: (type, total size or None if unknown/to end, header length).
fn box_header(b: &[u8]) -> Option<([u8; 4], Option<u64>, usize)> {
    if b.len() < 8 {
        return None;
    }
    let size = be32(b, 0) as u64;
    let ty = [b[4], b[5], b[6], b[7]];
    match size {
        1 => {
            if b.len() < 16 {
                return None;
            }
            Some((ty, Some(be64(b, 8)), 16))
        }
        0 => Some((ty, None, 8)),
        s => Some((ty, Some(s), 8)),
    }
}

/// Children of a fully buffered container box payload.
fn boxes(mut b: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    while let Some((ty, size, hl)) = box_header(b) {
        let size = size.unwrap_or(b.len() as u64);
        if size < hl as u64 || size > b.len() as u64 {
            break;
        }
        out.push((ty, &b[hl..size as usize]));
        b = &b[size as usize..];
    }
    out
}

fn find<'a>(b: &'a [u8], ty: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(b).into_iter().find(|(t, _)| t == ty).map(|(_, d)| d)
}

/// MPEG-4 descriptor length (up to 4 bytes of 7 bits).
fn desc_len(b: &[u8], pos: &mut usize) -> Option<usize> {
    let mut n = 0usize;
    for _ in 0..4 {
        let x = *b.get(*pos)?;
        *pos += 1;
        n = (n << 7) | (x & 0x7f) as usize;
        if x & 0x80 == 0 {
            break;
        }
    }
    Some(n)
}

/// `esds` → (objectTypeIndication, DecoderSpecificInfo).
fn parse_esds(b: &[u8]) -> Option<(u8, Vec<u8>)> {
    let mut pos = 4; // version + flags
    if *b.get(pos)? != 3 {
        return None;
    }
    pos += 1;
    desc_len(b, &mut pos)?;
    pos += 2; // ES_ID
    let flags = *b.get(pos)?;
    pos += 1;
    if flags & 0x80 != 0 {
        pos += 2;
    }
    if flags & 0x40 != 0 {
        pos += 1 + *b.get(pos)? as usize;
    }
    if flags & 0x20 != 0 {
        pos += 2;
    }
    if *b.get(pos)? != 4 {
        return None;
    }
    pos += 1;
    let dcd_len = desc_len(b, &mut pos)?;
    let dcd_end = pos + dcd_len;
    let oti = *b.get(pos)?;
    pos += 13;
    let mut dsi = Vec::new();
    if pos < dcd_end && *b.get(pos)? == 5 {
        pos += 1;
        let n = desc_len(b, &mut pos)?;
        dsi = b.get(pos..pos + n)?.to_vec();
    }
    Some((oti, dsi))
}

/// `dOps` (big endian) → an OpusHead (little endian, as in Ogg).
fn dops_to_opus_head(d: &[u8]) -> Option<Vec<u8>> {
    if d.len() < 11 {
        return None;
    }
    let mut h = b"OpusHead".to_vec();
    h.push(1);
    h.push(d[1]);
    h.extend_from_slice(&be16(d, 2).to_le_bytes());
    h.extend_from_slice(&be32(d, 4).to_le_bytes());
    h.extend_from_slice(&(be16(d, 8) as i16).to_le_bytes());
    h.push(d[10]);
    if d[10] != 0 {
        h.extend_from_slice(d.get(11..)?);
    }
    Some(h)
}

/// The sample entry of `stsd` → track info fields.
fn parse_sample_entry(t: &mut TrackInfo, stsd: &[u8]) {
    if stsd.len() < 8 {
        return;
    }
    let entries = &stsd[8..];
    let Some((ty, size, hl)) = box_header(entries) else { return };
    let size = size.unwrap_or(entries.len() as u64).min(entries.len() as u64) as usize;
    let body = &entries[hl.min(size)..size];
    let mut fourcc = ty;
    if &ty == b"encv" || &ty == b"enca" {
        t.encrypted = true;
    }
    let audio = matches!(&ty, b"mp4a" | b"Opus" | b"fLaC" | b"ac-3" | b"ec-3" | b"enca" | b".mp3");
    // Skip the SampleEntry / Audio- / VisualSampleEntry fixed fields.
    let child_start = if audio {
        if body.len() >= 28 {
            t.channels = be16(body, 16) as u32;
            t.sample_rate = be32(body, 24) >> 16;
        }
        28
    } else {
        if body.len() >= 78 {
            t.width = be16(body, 24) as u32;
            t.height = be16(body, 26) as u32;
        }
        78
    };
    let kids = if body.len() > child_start { boxes(&body[child_start..]) } else { Vec::new() };
    if t.encrypted {
        if let Some((_, sinf)) = kids.iter().find(|(k, _)| k == b"sinf") {
            if let Some(frma) = find(sinf, b"frma") {
                if frma.len() >= 4 {
                    fourcc = [frma[0], frma[1], frma[2], frma[3]];
                }
            }
        }
    }
    t.codec = match &fourcc {
        b"mp4a" => "aac",
        b"Opus" => "opus",
        b"fLaC" => "flac",
        b".mp3" => "mp3",
        b"ac-3" => "ac3",
        b"ec-3" => "eac3",
        b"avc1" | b"avc3" => "h264",
        b"hev1" | b"hvc1" => "hevc",
        b"vp09" => "vp9",
        b"vp08" => "vp8",
        b"av01" => "av1",
        _ => "",
    }
    .to_string();
    if t.codec.is_empty() {
        t.codec = String::from_utf8_lossy(&fourcc).into_owned();
    }
    for (k, d) in kids {
        match &k {
            b"esds" => {
                if let Some((oti, dsi)) = parse_esds(d) {
                    match oti {
                        0x69 | 0x6B => t.codec = "mp3".into(),
                        0xA5 => t.codec = "ac3".into(),
                        0xA6 => t.codec = "eac3".into(),
                        _ => {}
                    }
                    t.codec_private = dsi;
                }
            }
            b"dOps" => {
                if let Some(h) = dops_to_opus_head(d) {
                    if d.len() > 1 {
                        t.channels = d[1] as u32;
                    }
                    t.codec_private = h;
                }
            }
            b"dfLa" => t.codec_private = d.get(4..).unwrap_or(&[]).to_vec(),
            b"avcC" | b"hvcC" | b"av1C" | b"vpcC" => t.codec_private = d.to_vec(),
            _ => {}
        }
    }
    // AAC: the real rate / channels come from the AudioSpecificConfig.
    if t.codec == "aac" && t.codec_private.len() >= 2 {
        let asc = &t.codec_private;
        let freq_idx = ((asc[0] & 0x07) << 1) | (asc[1] >> 7);
        const RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];
        if let Some(&r) = RATES.get(freq_idx as usize) {
            t.sample_rate = r;
        }
        let ch = (asc[1] >> 3) & 0x0f;
        if (1..=7).contains(&ch) {
            t.channels = if ch == 7 { 8 } else { ch as u32 };
        }
    }
}

impl Mp4Parser {
    pub fn new() -> Mp4Parser {
        Mp4Parser { buf: Vec::new(), base: 0, tracks: Vec::new(), pending: Vec::new(), have_init: false, skip: 0 }
    }

    fn parse_moov(&mut self, moov: &[u8]) -> ParseEvent {
        let mut infos = Vec::new();
        let mut states = Vec::new();
        let mut duration = None;
        let mut movie_timescale = 1000u32;
        if let Some(mvhd) = find(moov, b"mvhd") {
            if !mvhd.is_empty() {
                let (ts, dur) = if mvhd[0] == 1 && mvhd.len() >= 32 { (be32(mvhd, 20), be64(mvhd, 24)) } else if mvhd.len() >= 20 { (be32(mvhd, 12), be32(mvhd, 16) as u64) } else { (1000, 0) };
                movie_timescale = ts.max(1);
                if dur != 0 && dur != u64::MAX && dur != u32::MAX as u64 {
                    duration = Some(dur as f64 / movie_timescale as f64);
                }
            }
        }
        if let Some(mvex) = find(moov, b"mvex") {
            if let Some(mehd) = find(mvex, b"mehd") {
                if !mehd.is_empty() {
                    let d = if mehd[0] == 1 && mehd.len() >= 12 { be64(mehd, 4) } else if mehd.len() >= 8 { be32(mehd, 4) as u64 } else { 0 };
                    if d > 0 {
                        duration = Some(d as f64 / movie_timescale as f64);
                    }
                }
            }
        }
        for (ty, trak) in boxes(moov) {
            if &ty != b"trak" {
                continue;
            }
            let mut st = TrackState::default();
            if let Some(tkhd) = find(trak, b"tkhd") {
                if !tkhd.is_empty() {
                    st.id = if tkhd[0] == 1 && tkhd.len() >= 24 { be32(tkhd, 20) } else if tkhd.len() >= 16 { be32(tkhd, 12) } else { 0 };
                }
            }
            let Some(mdia) = find(trak, b"mdia") else { continue };
            if let Some(mdhd) = find(mdia, b"mdhd") {
                if !mdhd.is_empty() {
                    st.timescale = if mdhd[0] == 1 && mdhd.len() >= 24 { be32(mdhd, 20) } else if mdhd.len() >= 16 { be32(mdhd, 12) } else { 0 };
                }
            }
            st.timescale = st.timescale.max(1);
            let kind = match find(mdia, b"hdlr").and_then(|h| h.get(8..12)) {
                Some(b"soun") => TrackKind::Audio,
                Some(b"vide") => TrackKind::Video,
                _ => TrackKind::Other,
            };
            st.kind = Some(kind);
            // Edit list: a single entry with a positive media_time shifts
            // presentation (AAC / Opus priming).
            if let Some(elst) = find(trak, b"edts").and_then(|e| find(e, b"elst")) {
                if elst.len() >= 8 {
                    let v = elst[0];
                    let count = be32(elst, 4);
                    let mut pos = 8;
                    for _ in 0..count {
                        let (seg_dur, media_time) = if v == 1 {
                            if elst.len() < pos + 20 {
                                break;
                            }
                            (be64(elst, pos), be64(elst, pos + 8) as i64)
                        } else {
                            if elst.len() < pos + 12 {
                                break;
                            }
                            (be32(elst, pos) as u64, be32(elst, pos + 4) as i32 as i64)
                        };
                        pos += if v == 1 { 20 } else { 12 };
                        if media_time >= 0 {
                            st.edit_offset = media_time;
                            break;
                        }
                        let _ = seg_dur;
                    }
                }
            }
            let mut info = TrackInfo::new(st.id as u64, kind);
            if let Some(stsd) = find(mdia, b"minf").and_then(|m| find(m, b"stbl")).and_then(|s| find(s, b"stsd")) {
                parse_sample_entry(&mut info, stsd);
            }
            states.push(st);
            infos.push(info);
        }
        if let Some(mvex) = find(moov, b"mvex") {
            for (ty, trex) in boxes(mvex) {
                if &ty == b"trex" && trex.len() >= 24 {
                    let id = be32(trex, 4);
                    if let Some(st) = states.iter_mut().find(|s| s.id == id) {
                        st.def_duration = be32(trex, 12);
                        st.def_size = be32(trex, 16);
                        st.def_flags = be32(trex, 20);
                    }
                }
            }
        }
        for (info, st) in infos.iter_mut().zip(&states) {
            if st.def_duration > 0 {
                info.default_duration = Some(st.def_duration as f64 / st.timescale as f64);
            }
        }
        self.tracks = states;
        self.have_init = true;
        ParseEvent::Init { tracks: infos, duration }
    }

    fn parse_moof(&mut self, moof: &[u8], moof_offset: u64) -> Result<(), String> {
        for (ty, traf) in boxes(moof) {
            if &ty != b"traf" {
                continue;
            }
            let tfhd = find(traf, b"tfhd").ok_or("traf without tfhd")?;
            if tfhd.len() < 8 {
                return Err("bad tfhd".into());
            }
            let tf_flags = be32(tfhd, 0) & 0xff_ffff;
            let track_id = be32(tfhd, 4);
            let Some(st) = self.tracks.iter().find(|t| t.id == track_id).cloned() else { continue };
            let mut p = 8;
            let mut base = moof_offset;
            if tf_flags & 0x1 != 0 {
                // An absolute base offset is relative to the start of the
                // byte stream in MSE; treat like moof-relative streams do.
                base = be64(tfhd, p);
                p += 8;
            }
            if tf_flags & 0x2 != 0 {
                p += 4;
            }
            let mut def_dur = st.def_duration;
            let mut def_size = st.def_size;
            let mut def_flags = st.def_flags;
            if tf_flags & 0x8 != 0 {
                def_dur = be32(tfhd, p);
                p += 4;
            }
            if tf_flags & 0x10 != 0 {
                def_size = be32(tfhd, p);
                p += 4;
            }
            if tf_flags & 0x20 != 0 {
                def_flags = be32(tfhd, p);
            }
            let mut dts: i64 = match find(traf, b"tfdt") {
                Some(t) if !t.is_empty() && t[0] == 1 && t.len() >= 12 => be64(t, 4) as i64,
                Some(t) if t.len() >= 8 => be32(t, 4) as i64,
                _ => 0,
            };
            let mut data_end = base;
            for (ty, trun) in boxes(traf) {
                if &ty != b"trun" || trun.len() < 8 {
                    continue;
                }
                let version = trun[0];
                let flags = be32(trun, 0) & 0xff_ffff;
                let count = be32(trun, 4) as usize;
                let mut q = 8;
                let mut offset = data_end;
                if flags & 0x1 != 0 {
                    offset = (base as i64 + be32(trun, q) as i32 as i64) as u64;
                    q += 4;
                }
                let mut first_flags = None;
                if flags & 0x4 != 0 {
                    first_flags = Some(be32(trun, q));
                    q += 4;
                }
                let per = [0x100, 0x200, 0x400, 0x800].iter().filter(|f| flags & **f != 0).count() * 4;
                if trun.len() < q + per * count {
                    return Err("truncated trun".into());
                }
                for i in 0..count {
                    let mut dur = def_dur;
                    let mut size = def_size;
                    let mut sflags = if i == 0 { first_flags.unwrap_or(def_flags) } else { def_flags };
                    let mut cto = 0i64;
                    if flags & 0x100 != 0 {
                        dur = be32(trun, q);
                        q += 4;
                    }
                    if flags & 0x200 != 0 {
                        size = be32(trun, q);
                        q += 4;
                    }
                    if flags & 0x400 != 0 {
                        sflags = be32(trun, q);
                        q += 4;
                    }
                    if flags & 0x800 != 0 {
                        let raw = be32(trun, q);
                        cto = if version == 0 { raw as i64 } else { raw as i32 as i64 };
                        q += 4;
                    }
                    let key = st.kind == Some(TrackKind::Audio) || sflags & 0x0001_0000 == 0;
                    self.pending.push(PendingSample { track: track_id, offset, size, dts, cto, dur, key });
                    offset += size as u64;
                    dts += dur as i64;
                }
                data_end = offset;
            }
        }
        Ok(())
    }

    /// Emit pending samples whose bytes are inside `buf`.
    fn take_samples(&mut self, out: &mut Vec<ParseEvent>, upto: u64) {
        let base = self.base;
        let mut keep = Vec::new();
        for s in std::mem::take(&mut self.pending) {
            let end = s.offset + s.size as u64;
            if s.offset >= base && end <= upto {
                let st = self.tracks.iter().find(|t| t.id == s.track).cloned().unwrap_or_default();
                let ts = st.timescale.max(1) as f64;
                let a = (s.offset - base) as usize;
                let data: std::sync::Arc<[u8]> = self.buf[a..a + s.size as usize].to_vec().into();
                let dts = (s.dts - st.edit_offset) as f64 / ts;
                let pts = (s.dts + s.cto - st.edit_offset) as f64 / ts;
                out.push(ParseEvent::Frame(CodedFrame { track: s.track as u64, pts, dts, dur: s.dur as f64 / ts, key: s.key, data }));
            } else if end > upto {
                keep.push(s);
            }
        }
        self.pending = keep;
    }
}

impl SegmentParser for Mp4Parser {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<ParseEvent>, String> {
        let mut out = Vec::new();
        let mut input = bytes;
        if self.skip > 0 {
            let n = (self.skip as usize).min(input.len());
            self.skip -= n as u64;
            self.base += n as u64;
            input = &input[n..];
        }
        self.buf.extend_from_slice(input);
        let mut pos = 0usize;
        loop {
            let b = &self.buf[pos..];
            let Some((ty, size, hl)) = box_header(b) else { break };
            let size = match size {
                Some(s) if s >= hl as u64 => s,
                Some(_) => return Err("invalid box size".into()),
                None => {
                    if &ty == b"mdat" {
                        // Runs to the end of the stream: take what is here.
                        b.len() as u64
                    } else {
                        return Err("box of unknown size".into());
                    }
                }
            };
            let wanted = matches!(&ty, b"moov" | b"moof" | b"mdat");
            if !wanted {
                if (b.len() as u64) < size {
                    self.skip = size - b.len() as u64;
                    pos = self.buf.len();
                    self.base += 0;
                    break;
                }
                pos += size as usize;
                continue;
            }
            if (b.len() as u64) < size {
                if &ty == b"mdat" && !self.pending.is_empty() {
                    // Emit the samples already complete inside the mdat.
                    let upto = self.base + self.buf.len() as u64;
                    if pos > 0 {
                        self.buf.drain(..pos);
                        self.base += pos as u64;
                        pos = 0;
                    }
                    self.take_samples(&mut out, upto);
                    let _ = pos;
                }
                break;
            }
            let abs = self.base + pos as u64;
            match &ty {
                b"moov" => {
                    let moov = self.buf[pos + hl..pos + size as usize].to_vec();
                    out.push(self.parse_moov(&moov));
                }
                b"moof" => {
                    if !self.have_init {
                        return Err("media segment before initialization segment".into());
                    }
                    let moof = self.buf[pos + hl..pos + size as usize].to_vec();
                    self.parse_moof(&moof, abs)?;
                }
                _ => {
                    // mdat: emit the samples it holds.
                    if pos > 0 {
                        self.buf.drain(..pos);
                        self.base += pos as u64;
                        pos = 0;
                    }
                    let upto = self.base + size;
                    self.take_samples(&mut out, upto);
                    self.pending.clear();
                }
            }
            pos += size as usize;
        }
        if pos > 0 {
            let pos = pos.min(self.buf.len());
            self.buf.drain(..pos);
            self.base += pos as u64;
        }
        Ok(out)
    }

    fn reset(&mut self) {
        self.base += self.buf.len() as u64 + self.skip;
        self.buf.clear();
        self.pending.clear();
        self.skip = 0;
    }

    fn mid_segment(&self) -> bool {
        !self.pending.is_empty()
    }
}

/// Build fragmented MP4 bytes (tests).
pub mod build {
    pub fn bx(ty: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(ty);
        v.extend_from_slice(body);
        v
    }

    pub fn full(ty: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
        let mut b = vec![version];
        b.extend_from_slice(&flags.to_be_bytes()[1..]);
        b.extend_from_slice(body);
        bx(ty, &b)
    }

    /// An init segment with one AAC-LC audio track (id 1).
    pub fn aac_init(timescale: u32, rate_idx: u8, channels: u8) -> Vec<u8> {
        let asc = [(2 << 3) | (rate_idx >> 1), ((rate_idx & 1) << 7) | (channels << 3)];
        let mut esds = vec![3u8, 25, 0, 1, 0, 4, 17, 0x40, 0x15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 2];
        esds.extend_from_slice(&asc);
        esds.extend_from_slice(&[6, 1, 2]);
        let esds = full(b"esds", 0, 0, &esds);
        let mut mp4a = vec![0u8; 28];
        mp4a[7] = 1; // data reference index
        mp4a[17] = channels;
        mp4a[19] = 16;
        mp4a[24..28].copy_from_slice(&(44100u32 << 16).to_be_bytes());
        mp4a.extend(esds);
        let mut stsd = 1u32.to_be_bytes().to_vec();
        stsd.extend(bx(b"mp4a", &mp4a));
        let stsd = full(b"stsd", 0, 0, &stsd);
        let stbl = bx(b"stbl", &stsd);
        let minf = bx(b"minf", &stbl);
        let mut hdlr = vec![0u8; 4];
        hdlr.extend_from_slice(b"soun");
        hdlr.extend_from_slice(&[0u8; 13]);
        let hdlr = full(b"hdlr", 0, 0, &hdlr);
        let mut mdhd = vec![0u8; 8];
        mdhd.extend_from_slice(&timescale.to_be_bytes());
        mdhd.extend_from_slice(&[0u8; 8]);
        let mdhd = full(b"mdhd", 0, 0, &mdhd);
        let mdia = bx(b"mdia", &[mdhd, hdlr, minf].concat());
        let mut tkhd = vec![0u8; 8];
        tkhd.extend_from_slice(&1u32.to_be_bytes());
        tkhd.extend_from_slice(&[0u8; 68]);
        let tkhd = full(b"tkhd", 0, 3, &tkhd);
        let trak = bx(b"trak", &[tkhd, mdia].concat());
        let mut trex = 1u32.to_be_bytes().to_vec();
        trex.extend_from_slice(&1u32.to_be_bytes());
        trex.extend_from_slice(&1024u32.to_be_bytes());
        trex.extend_from_slice(&0u32.to_be_bytes());
        trex.extend_from_slice(&0u32.to_be_bytes());
        let mvex = bx(b"mvex", &full(b"trex", 0, 0, &trex));
        let mut mvhd = vec![0u8; 8];
        mvhd.extend_from_slice(&1000u32.to_be_bytes());
        mvhd.extend_from_slice(&[0u8; 84]);
        let mvhd = full(b"mvhd", 0, 0, &mvhd);
        [bx(b"ftyp", b"iso6\0\0\0\0iso6dash"), bx(b"moov", &[mvhd, trak, mvex].concat())].concat()
    }

    /// moof + mdat with `samples` (sizes) of 1024 ticks each starting at `dts`.
    pub fn fragment(seq: u32, dts: u64, samples: &[Vec<u8>]) -> Vec<u8> {
        let mfhd = full(b"mfhd", 0, 0, &seq.to_be_bytes());
        let tfhd = full(b"tfhd", 0, 0x20000, &1u32.to_be_bytes());
        let tfdt = full(b"tfdt", 1, 0, &dts.to_be_bytes());
        let mut trun = (samples.len() as u32).to_be_bytes().to_vec();
        trun.extend_from_slice(&0u32.to_be_bytes()); // data offset, patched below
        for s in samples {
            trun.extend_from_slice(&(s.len() as u32).to_be_bytes());
        }
        let trun_len = trun.len();
        let trun = full(b"trun", 0, 0x201, &trun);
        let traf = bx(b"traf", &[tfhd, tfdt, trun].concat());
        let mut moof = bx(b"moof", &[mfhd, traf].concat());
        // data offset = moof size + mdat header.
        let off = (moof.len() + 8) as u32;
        let at = moof.len() - trun_len + 4;
        moof[at..at + 4].copy_from_slice(&off.to_be_bytes());
        let mdat = bx(b"mdat", &samples.concat());
        [moof, mdat].concat()
    }
}

#[cfg(test)]
mod tests {
    use super::build::*;
    use super::*;

    #[test]
    fn parses_aac_init_and_fragments() {
        let init = aac_init(44100, 4, 2);
        let s1: Vec<Vec<u8>> = (0..3).map(|i| vec![i as u8; 10 + i]).collect();
        let s2: Vec<Vec<u8>> = (0..2).map(|i| vec![0x40 + i as u8; 7]).collect();
        let all = [init, fragment(1, 0, &s1), fragment(2, 3072, &s2)].concat();
        for chunk in [all.len(), 5, 1] {
            let mut p = Mp4Parser::new();
            let mut ev = Vec::new();
            for c in all.chunks(chunk) {
                ev.extend(p.push(c).unwrap());
            }
            match &ev[0] {
                ParseEvent::Init { tracks, .. } => {
                    assert_eq!(tracks[0].codec, "aac");
                    assert_eq!(tracks[0].sample_rate, 44100);
                    assert_eq!(tracks[0].channels, 2);
                    assert_eq!(tracks[0].kind, TrackKind::Audio);
                    assert_eq!(tracks[0].codec_private.len(), 2);
                }
                e => panic!("{e:?}"),
            }
            let frames: Vec<&CodedFrame> = ev.iter().filter_map(|e| if let ParseEvent::Frame(f) = e { Some(f) } else { None }).collect();
            assert_eq!(frames.len(), 5, "chunk {chunk}");
            assert_eq!(&frames[1].data[..], &s1[1][..]);
            assert_eq!(&frames[4].data[..], &s2[1][..]);
            assert!((frames[3].pts - 3072.0 / 44100.0).abs() < 1e-9);
            assert!((frames[0].dur - 1024.0 / 44100.0).abs() < 1e-9);
        }
    }

    #[test]
    fn dops_conversion() {
        let d = [0u8, 2, 0x01, 0x38, 0, 0, 0xbb, 0x80, 0, 0, 0];
        let h = dops_to_opus_head(&d).unwrap();
        assert_eq!(&h[..8], b"OpusHead");
        assert_eq!(h[9], 2);
        assert_eq!(u16::from_le_bytes([h[10], h[11]]), 312);
        assert_eq!(u32::from_le_bytes([h[12], h[13], h[14], h[15]]), 48000);
    }
}
