//! HTTP/2 frame layer (RFC 9113 section 4 and 6).

use std::io::{self, Read};

pub const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

pub const DATA: u8 = 0x0;
pub const HEADERS: u8 = 0x1;
pub const PRIORITY: u8 = 0x2;
pub const RST_STREAM: u8 = 0x3;
pub const SETTINGS: u8 = 0x4;
pub const PUSH_PROMISE: u8 = 0x5;
pub const PING: u8 = 0x6;
pub const GOAWAY: u8 = 0x7;
pub const WINDOW_UPDATE: u8 = 0x8;
pub const CONTINUATION: u8 = 0x9;

pub const FLAG_END_STREAM: u8 = 0x1;
pub const FLAG_ACK: u8 = 0x1;
pub const FLAG_END_HEADERS: u8 = 0x4;
pub const FLAG_PADDED: u8 = 0x8;
pub const FLAG_PRIORITY: u8 = 0x20;

pub const SETTINGS_HEADER_TABLE_SIZE: u16 = 0x1;
pub const SETTINGS_ENABLE_PUSH: u16 = 0x2;
pub const SETTINGS_MAX_CONCURRENT_STREAMS: u16 = 0x3;
pub const SETTINGS_INITIAL_WINDOW_SIZE: u16 = 0x4;
pub const SETTINGS_MAX_FRAME_SIZE: u16 = 0x5;
pub const SETTINGS_MAX_HEADER_LIST_SIZE: u16 = 0x6;

pub const NO_ERROR: u32 = 0x0;
pub const PROTOCOL_ERROR: u32 = 0x1;
pub const INTERNAL_ERROR: u32 = 0x2;
pub const FLOW_CONTROL_ERROR: u32 = 0x3;
pub const STREAM_CLOSED: u32 = 0x5;
pub const FRAME_SIZE_ERROR: u32 = 0x6;
pub const REFUSED_STREAM: u32 = 0x7;
pub const CANCEL: u32 = 0x8;
pub const COMPRESSION_ERROR: u32 = 0x9;
pub const HTTP_1_1_REQUIRED: u32 = 0xd;

/// Default (and our advertised) SETTINGS_MAX_FRAME_SIZE.
pub const DEFAULT_MAX_FRAME: usize = 16_384;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: u8,
    pub flags: u8,
    pub stream: u32,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(kind: u8, flags: u8, stream: u32, payload: Vec<u8>) -> Frame {
        Frame { kind, flags, stream, payload }
    }

    pub fn has(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }

    /// Serialize header + payload.
    pub fn encode(&self, out: &mut Vec<u8>) {
        let len = self.payload.len();
        out.extend_from_slice(&[(len >> 16) as u8, (len >> 8) as u8, len as u8, self.kind, self.flags]);
        out.extend_from_slice(&(self.stream & 0x7fff_ffff).to_be_bytes());
        out.extend_from_slice(&self.payload);
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(9 + self.payload.len());
        self.encode(&mut v);
        v
    }

    /// Read one frame, rejecting payloads above `max_len`.
    pub fn read<R: Read>(r: &mut R, max_len: usize) -> Result<Frame, FrameError> {
        let mut h = [0u8; 9];
        r.read_exact(&mut h).map_err(FrameError::Io)?;
        let len = ((h[0] as usize) << 16) | ((h[1] as usize) << 8) | h[2] as usize;
        if len > max_len {
            return Err(FrameError::TooLarge(len));
        }
        let stream = u32::from_be_bytes([h[5], h[6], h[7], h[8]]) & 0x7fff_ffff;
        let mut payload = vec![0u8; len];
        r.read_exact(&mut payload).map_err(FrameError::Io)?;
        Ok(Frame { kind: h[3], flags: h[4], stream, payload })
    }

    /// For DATA / HEADERS / PUSH_PROMISE: strip padding (when PADDED).
    pub fn unpadded(&self) -> Result<&[u8], FrameError> {
        if !self.has(FLAG_PADDED) {
            return Ok(&self.payload);
        }
        let pad = *self.payload.first().ok_or(FrameError::Protocol("empty padded frame"))? as usize;
        if pad + 1 > self.payload.len() {
            return Err(FrameError::Protocol("padding exceeds payload"));
        }
        Ok(&self.payload[1..self.payload.len() - pad])
    }

    /// HEADERS payload without padding and priority fields.
    pub fn header_block_fragment(&self) -> Result<&[u8], FrameError> {
        let p = self.unpadded()?;
        if self.kind == HEADERS && self.has(FLAG_PRIORITY) {
            if p.len() < 5 {
                return Err(FrameError::Protocol("short priority fields"));
            }
            return Ok(&p[5..]);
        }
        Ok(p)
    }
}

#[derive(Debug)]
pub enum FrameError {
    Io(io::Error),
    TooLarge(usize),
    Protocol(&'static str),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Io(e) => write!(f, "{e}"),
            FrameError::TooLarge(n) => write!(f, "frame of {n} bytes exceeds limit"),
            FrameError::Protocol(m) => write!(f, "{m}"),
        }
    }
}

pub fn settings_frame(settings: &[(u16, u32)]) -> Frame {
    let mut p = Vec::with_capacity(settings.len() * 6);
    for (id, v) in settings {
        p.extend_from_slice(&id.to_be_bytes());
        p.extend_from_slice(&v.to_be_bytes());
    }
    Frame::new(SETTINGS, 0, 0, p)
}

pub fn parse_settings(payload: &[u8]) -> Result<Vec<(u16, u32)>, FrameError> {
    if payload.len() % 6 != 0 {
        return Err(FrameError::Protocol("SETTINGS length not a multiple of 6"));
    }
    Ok(payload.chunks(6).map(|c| (u16::from_be_bytes([c[0], c[1]]), u32::from_be_bytes([c[2], c[3], c[4], c[5]]))).collect())
}

pub fn window_update(stream: u32, increment: u32) -> Frame {
    Frame::new(WINDOW_UPDATE, 0, stream, (increment & 0x7fff_ffff).to_be_bytes().to_vec())
}

pub fn rst_stream(stream: u32, code: u32) -> Frame {
    Frame::new(RST_STREAM, 0, stream, code.to_be_bytes().to_vec())
}

pub fn goaway(last_stream: u32, code: u32, debug: &[u8]) -> Frame {
    let mut p = Vec::with_capacity(8 + debug.len());
    p.extend_from_slice(&(last_stream & 0x7fff_ffff).to_be_bytes());
    p.extend_from_slice(&code.to_be_bytes());
    p.extend_from_slice(debug);
    Frame::new(GOAWAY, 0, 0, p)
}

pub fn be_u32(b: &[u8]) -> u32 {
    if b.len() < 4 {
        return 0;
    }
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// Split an encoded header block into HEADERS + CONTINUATION frames.
pub fn header_frames(stream: u32, block: &[u8], end_stream: bool, max_frame: usize, priority: Option<(u32, bool, u8)>, out: &mut Vec<u8>) {
    let mut first_payload = Vec::new();
    let mut flags = 0u8;
    if end_stream {
        flags |= FLAG_END_STREAM;
    }
    if let Some((dep, exclusive, weight)) = priority {
        flags |= FLAG_PRIORITY;
        let d = (dep & 0x7fff_ffff) | if exclusive { 0x8000_0000 } else { 0 };
        first_payload.extend_from_slice(&d.to_be_bytes());
        first_payload.push(weight);
    }
    let room = max_frame - first_payload.len();
    let first_len = block.len().min(room);
    first_payload.extend_from_slice(&block[..first_len]);
    let mut rest = &block[first_len..];
    if rest.is_empty() {
        flags |= FLAG_END_HEADERS;
    }
    Frame::new(HEADERS, flags, stream, first_payload).encode(out);
    while !rest.is_empty() {
        let n = rest.len().min(max_frame);
        let f = if n == rest.len() { FLAG_END_HEADERS } else { 0 };
        Frame::new(CONTINUATION, f, stream, rest[..n].to_vec()).encode(out);
        rest = &rest[n..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let f = Frame::new(DATA, FLAG_END_STREAM, 3, b"hello".to_vec());
        let bytes = f.to_bytes();
        assert_eq!(&bytes[..9], &[0, 0, 5, 0, 1, 0, 0, 0, 3]);
        let g = Frame::read(&mut &bytes[..], 16384).unwrap();
        assert_eq!(f, g);
        assert!(matches!(Frame::read(&mut &bytes[..], 4), Err(FrameError::TooLarge(5))));
    }

    #[test]
    fn padding() {
        let f = Frame::new(DATA, FLAG_PADDED, 1, vec![2, b'a', b'b', 0, 0]);
        assert_eq!(f.unpadded().unwrap(), b"ab");
        let bad = Frame::new(DATA, FLAG_PADDED, 1, vec![9, b'a']);
        assert!(bad.unpadded().is_err());
        let h = Frame::new(HEADERS, FLAG_PRIORITY | FLAG_PADDED, 1, vec![1, 0, 0, 0, 0, 15, 0x82, 0]);
        assert_eq!(h.header_block_fragment().unwrap(), &[0x82]);
    }

    #[test]
    fn continuation_split() {
        let block: Vec<u8> = (0..40000u32).map(|i| i as u8).collect();
        let mut out = Vec::new();
        header_frames(5, &block, true, 16384, Some((0, false, 255)), &mut out);
        let mut r = &out[..];
        let a = Frame::read(&mut r, 16384).unwrap();
        assert_eq!(a.kind, HEADERS);
        assert!(a.has(FLAG_END_STREAM) && !a.has(FLAG_END_HEADERS));
        let mut got = a.header_block_fragment().unwrap().to_vec();
        loop {
            let c = Frame::read(&mut r, 16384).unwrap();
            assert_eq!(c.kind, CONTINUATION);
            got.extend_from_slice(&c.payload);
            if c.has(FLAG_END_HEADERS) {
                break;
            }
        }
        assert_eq!(got, block);
        assert!(r.is_empty());
    }
}
