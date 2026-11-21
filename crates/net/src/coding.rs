//! HTTP content codings (`Content-Encoding`): gzip, deflate (zlib or raw),
//! br and zstd, decoded as browsers do.
//!
//! Browsers are lenient where servers are commonly sloppy, and pages rely
//! on it: bytes after the end of a brotli stream or of the last gzip member
//! are ignored, gzip members and zstd frames may be concatenated (zstd
//! skippable frames are skipped), a stream cut short yields what was
//! decoded, and every `Content-Encoding` field line counts (a CDN that
//! appends its own line stacks its coding on the origin's).

use std::io::{self, BufRead, BufReader, Read};

/// Every coding named by the response's `Content-Encoding` field lines,
/// lowercased, in the order they were applied (`identity` dropped).
pub fn content_codings(headers: &[(String, String)]) -> Vec<String> {
    headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("content-encoding"))
        .flat_map(|(_, v)| v.split(','))
        .map(|c| c.trim().to_ascii_lowercase())
        .filter(|c| !c.is_empty() && c != "identity")
        .collect()
}

/// Wrap a body reader with the decoders for its `Content-Encoding`
/// (applied in reverse order). Unknown codings pass the bytes through.
pub fn decode_content(headers: &[(String, String)], body: Box<dyn Read + Send>) -> Box<dyn Read + Send> {
    let mut r = body;
    for coding in content_codings(headers).into_iter().rev() {
        r = match coding.as_str() {
            "gzip" | "x-gzip" | "deflate" | "br" | "zstd" => Box::new(LazyDecoder { coding, inner: Some(r), dec: None }),
            _ => r,
        };
    }
    r
}

type Src = BufReader<Box<dyn Read + Send>>;

/// Defers building the decoder until the first read, so empty bodies
/// (204, HEAD, redirects) labelled with a coding read as empty.
struct LazyDecoder {
    coding: String,
    inner: Option<Box<dyn Read + Send>>,
    dec: Option<Box<dyn Read + Send>>,
}

impl Read for LazyDecoder {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.dec.is_none() {
            let Some(inner) = self.inner.take() else { return Ok(0) };
            let mut br: Src = BufReader::with_capacity(16 * 1024, inner);
            let first = {
                let b = br.fill_buf()?;
                if b.is_empty() {
                    return Ok(0);
                }
                b.to_vec()
            };
            self.dec = Some(match self.coding.as_str() {
                "gzip" | "x-gzip" => Box::new(GzipMembers { src: Some(br), dec: None, members: 0, done: false }),
                "deflate" => {
                    // RFC 9110 says zlib; some servers send raw deflate.
                    let zlib = first.len() >= 2 && first[0] & 0x0f == 8 && ((first[0] as u16) << 8 | first[1] as u16) % 31 == 0;
                    if zlib {
                        Box::new(Truncatable::new(flate2::bufread::ZlibDecoder::new(br)))
                    } else {
                        Box::new(Truncatable::new(flate2::bufread::DeflateDecoder::new(br)))
                    }
                }
                "br" => Box::new(Brotli { dec: brotli_decompressor::Decompressor::new(br, 64 * 1024), done: false }),
                "zstd" => Box::new(ZstdFrames { src: Some(br), dec: None, frames: 0, done: false }),
                _ => Box::new(br),
            });
        }
        self.dec.as_mut().unwrap().read(buf)
    }
}

/// A truncated body ends with what was decoded (browsers show a cut-off
/// page rather than nothing).
struct Truncatable<R> {
    inner: R,
    produced: bool,
    done: bool,
    truncated: bool,
}

impl<R: Read> Truncatable<R> {
    fn new(inner: R) -> Self {
        Truncatable { inner, produced: false, done: false, truncated: false }
    }
}

impl<R: Read> Read for Truncatable<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done {
            return Ok(0);
        }
        match self.inner.read(buf) {
            Ok(0) => {
                self.done = true;
                Ok(0)
            }
            Ok(n) => {
                self.produced = true;
                Ok(n)
            }
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof && self.produced => {
                self.done = true;
                self.truncated = true;
                Ok(0)
            }
            Err(e) => Err(e),
        }
    }
}

/// Brotli: the stream's own end is the body's end; anything after it is
/// ignored, and reads after the end keep returning 0.
struct Brotli {
    dec: brotli_decompressor::Decompressor<Src>,
    done: bool,
}

impl Read for Brotli {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        match self.dec.read(buf) {
            Ok(0) => {
                self.done = true;
                Ok(0)
            }
            Ok(n) => Ok(n),
            Err(e) if e.kind() == io::ErrorKind::InvalidData => Err(io::Error::new(io::ErrorKind::InvalidData, "invalid brotli data")),
            Err(e) => Err(e),
        }
    }
}

const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

/// Whether the next bytes of `src` start another gzip member / zstd frame.
/// Fills the buffer until it holds at least `n` bytes or the input ends.
fn peek(src: &mut Src, n: usize) -> io::Result<Vec<u8>> {
    let mut have = src.fill_buf()?.to_vec();
    while have.len() < n {
        // The buffer may hold a partial prefix: read more behind it.
        let mut more = vec![0u8; n - have.len()];
        let got = src.get_mut().read(&mut more)?;
        if got == 0 {
            break;
        }
        // Rebuild the reader so the peeked bytes are read first.
        have.extend_from_slice(&more[..got]);
        let consumed = src.buffer().len();
        src.consume(consumed);
        let rest = std::mem::replace(src.get_mut(), Box::new(io::empty()));
        *src = BufReader::with_capacity(16 * 1024, Box::new(io::Cursor::new(have.clone()).chain(rest)));
        have = src.fill_buf()?.to_vec();
    }
    Ok(have)
}

/// gzip: concatenated members are decoded in turn; bytes after the last
/// member that do not start another one are ignored.
struct GzipMembers {
    src: Option<Src>,
    dec: Option<Truncatable<flate2::bufread::GzDecoder<Src>>>,
    members: usize,
    done: bool,
}

impl Read for GzipMembers {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.done || buf.is_empty() {
                return Ok(0);
            }
            if self.dec.is_none() {
                let mut src = self.src.take().expect("gzip source");
                let next = peek(&mut src, 2)?;
                if self.members > 0 && !next.starts_with(&GZIP_MAGIC) {
                    // Trailing bytes (padding, garbage): the body ended.
                    self.done = true;
                    return Ok(0);
                }
                self.members += 1;
                self.dec = Some(Truncatable::new(flate2::bufread::GzDecoder::new(src)));
            }
            let dec = self.dec.as_mut().unwrap();
            match dec.read(buf) {
                Ok(0) => {
                    let dec = self.dec.take().unwrap();
                    if dec.truncated {
                        self.done = true;
                        return Ok(0);
                    }
                    self.src = Some(dec.inner.into_inner());
                }
                Ok(n) => return Ok(n),
                Err(_) if self.members > 1 && !dec.produced => {
                    // A later "member" that is not one: ignore the rest.
                    self.done = true;
                    return Ok(0);
                }
                Err(e) => return Err(e),
            }
        }
    }
}

/// zstd: frames are decoded in turn and skippable frames are skipped.
struct ZstdFrames {
    src: Option<Src>,
    dec: Option<ruzstd::decoding::StreamingDecoder<Src, ruzstd::decoding::FrameDecoder>>,
    frames: usize,
    done: bool,
}

impl Read for ZstdFrames {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.done || buf.is_empty() {
                return Ok(0);
            }
            if self.dec.is_none() {
                let mut src = self.src.take().expect("zstd source");
                let head = peek(&mut src, 8)?;
                if head.is_empty() || self.frames > 0 && !head.starts_with(&ZSTD_MAGIC) && !(head.len() >= 4 && head[0] & 0xf0 == 0x50 && head[1..4] == [0x2a, 0x4d, 0x18]) {
                    // End of input, or trailing bytes after the last frame.
                    self.done = true;
                    return Ok(0);
                }
                if head.len() >= 8 && head[0] & 0xf0 == 0x50 && head[1..4] == [0x2a, 0x4d, 0x18] {
                    // Skippable frame: magic, 4-byte length, payload.
                    let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as u64;
                    io::copy(&mut (&mut src).take(8 + len), &mut io::sink())?;
                    self.src = Some(src);
                    continue;
                }
                match ruzstd::decoding::StreamingDecoder::new(src) {
                    Ok(d) => {
                        self.frames += 1;
                        self.dec = Some(d);
                    }
                    Err(e) => return Err(io::Error::new(io::ErrorKind::InvalidData, format!("zstd: {e}"))),
                }
            }
            match self.dec.as_mut().unwrap().read(buf) {
                Ok(0) => {
                    let d = self.dec.take().unwrap();
                    self.src = Some(d.into_inner());
                }
                Ok(n) => return Ok(n),
                Err(e) => return Err(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn h(lines: &[&str]) -> Vec<(String, String)> {
        lines.iter().map(|v| ("content-encoding".to_string(), v.to_string())).collect()
    }

    /// Decode with small reads, then check further reads stay at 0.
    fn decode(headers: &[(String, String)], body: Vec<u8>) -> io::Result<Vec<u8>> {
        let mut r = decode_content(headers, Box::new(io::Cursor::new(body)));
        let mut out = Vec::new();
        let mut buf = [0u8; 7];
        loop {
            let n = r.read(&mut buf)?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        for _ in 0..3 {
            assert_eq!(r.read(&mut buf)?, 0, "read after the end");
        }
        Ok(out)
    }

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn zstd(data: &[u8]) -> Vec<u8> {
        ruzstd::encoding::compress_to_vec(data, ruzstd::encoding::CompressionLevel::Fastest)
    }

    // "hello\n" as a brotli stream (window 22, one metablock).
    const BR_HELLO: &[u8] = b"\x8f\x02\x80hello\n\x03";

    #[test]
    fn brotli_ignores_bytes_after_the_stream() {
        assert_eq!(decode(&h(&["br"]), BR_HELLO.to_vec()).unwrap(), b"hello\n");
        let mut extra = BR_HELLO.to_vec();
        extra.extend_from_slice(b"goodbye\n");
        assert_eq!(decode(&h(&["br"]), extra).unwrap(), b"hello\n");
        // Empty streams (any window size) and empty bodies.
        assert_eq!(decode(&h(&["br"]), vec![0x3b]).unwrap(), b"");
        assert_eq!(decode(&h(&["br"]), vec![0x06]).unwrap(), b"");
        assert_eq!(decode(&h(&["br"]), vec![]).unwrap(), b"");
        // Not brotli at all is still an error.
        assert!(decode(&h(&["br"]), b"<html>plain text, mislabelled</html>".to_vec()).is_err());
    }

    #[test]
    fn gzip_members_trailing_bytes_and_truncation() {
        let text = b"0123456789abcdef".repeat(300);
        let mut two = gzip(&text[..1000]);
        two.extend(gzip(&text[1000..]));
        assert_eq!(decode(&h(&["gzip"]), two.clone()).unwrap(), text);
        let mut padded = two.clone();
        padded.extend_from_slice(&[0u8; 9]);
        assert_eq!(decode(&h(&["x-gzip"]), padded).unwrap(), text);
        let mut garbage = gzip(&text);
        garbage.extend_from_slice(b"\x1fjunk");
        assert_eq!(decode(&h(&["gzip"]), garbage).unwrap(), text);
        // Cut off inside the deflate data: what was decoded is kept.
        let mut x = 1u32;
        let text: Vec<u8> = (0..20_000)
            .map(|_| {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                b'a' + (x >> 16) as u8 % 26
            })
            .collect();
        let full = gzip(&text);
        let cut = decode(&h(&["gzip"]), full[..full.len() / 2].to_vec()).unwrap();
        assert!(!cut.is_empty() && text.starts_with(&cut), "{}", cut.len());
        assert!(decode(&h(&["gzip"]), b"not gzip".to_vec()).is_err());
    }

    #[test]
    fn zstd_frames_skippable_frames_and_trailing_bytes() {
        let text = b"zstandard frames ".repeat(200);
        let mut body = zstd(&text[..1500]);
        body.extend_from_slice(&[0x5a, 0x2a, 0x4d, 0x18, 3, 0, 0, 0, 1, 2, 3]);
        body.extend(zstd(&text[1500..]));
        assert_eq!(decode(&h(&["zstd"]), body.clone()).unwrap(), text);
        body.extend_from_slice(b"\0\0trailing");
        assert_eq!(decode(&h(&["zstd"]), body).unwrap(), text);
    }

    #[test]
    fn every_content_encoding_line_counts() {
        let text = b"stacked codings ".repeat(50);
        // deflate applied first, then gzip (a second field line).
        let body = gzip(&zlib(&text));
        assert_eq!(decode(&h(&["deflate", "GZIP"]), body.clone()).unwrap(), text);
        assert_eq!(decode(&h(&["deflate, identity", " gzip "]), body).unwrap(), text);
        assert_eq!(content_codings(&h(&["identity"])), Vec::<String>::new());
        // Unknown codings pass the bytes through.
        assert_eq!(decode(&h(&["x-unknown"]), b"raw".to_vec()).unwrap(), b"raw");
    }
}
