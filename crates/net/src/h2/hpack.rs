//! HPACK header compression for HTTP/2 (RFC 7541): static + dynamic
//! tables, prefix integers, string literals with optional Huffman coding.

use super::huffman_table::HUFFMAN_CODES;
use std::collections::VecDeque;
use std::sync::OnceLock;

/// RFC 7541 Appendix A.
pub(crate) const STATIC_TABLE: [(&str, &str); 61] = [
    (":authority", ""),
    (":method", "GET"),
    (":method", "POST"),
    (":path", "/"),
    (":path", "/index.html"),
    (":scheme", "http"),
    (":scheme", "https"),
    (":status", "200"),
    (":status", "204"),
    (":status", "206"),
    (":status", "304"),
    (":status", "400"),
    (":status", "404"),
    (":status", "500"),
    ("accept-charset", ""),
    ("accept-encoding", "gzip, deflate"),
    ("accept-language", ""),
    ("accept-ranges", ""),
    ("accept", ""),
    ("access-control-allow-origin", ""),
    ("age", ""),
    ("allow", ""),
    ("authorization", ""),
    ("cache-control", ""),
    ("content-disposition", ""),
    ("content-encoding", ""),
    ("content-language", ""),
    ("content-length", ""),
    ("content-location", ""),
    ("content-range", ""),
    ("content-type", ""),
    ("cookie", ""),
    ("date", ""),
    ("etag", ""),
    ("expect", ""),
    ("expires", ""),
    ("from", ""),
    ("host", ""),
    ("if-match", ""),
    ("if-modified-since", ""),
    ("if-none-match", ""),
    ("if-range", ""),
    ("if-unmodified-since", ""),
    ("last-modified", ""),
    ("link", ""),
    ("location", ""),
    ("max-forwards", ""),
    ("proxy-authenticate", ""),
    ("proxy-authorization", ""),
    ("range", ""),
    ("referer", ""),
    ("refresh", ""),
    ("retry-after", ""),
    ("server", ""),
    ("set-cookie", ""),
    ("strict-transport-security", ""),
    ("transfer-encoding", ""),
    ("user-agent", ""),
    ("vary", ""),
    ("via", ""),
    ("www-authenticate", ""),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HpackError {
    Truncated,
    IntegerOverflow,
    BadIndex(usize),
    BadHuffman,
    BadTableSizeUpdate,
    HeaderListTooLarge,
    InvalidUtf8,
}

impl std::fmt::Display for HpackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HPACK error: {self:?}")
    }
}

// ------------------------------------------------------------- integers

/// Encode `value` with an N-bit prefix; `first` holds the high bits of the
/// first byte (the representation's flag pattern).
pub(crate) fn encode_int(out: &mut Vec<u8>, value: usize, prefix_bits: u8, first: u8) {
    let max = (1usize << prefix_bits) - 1;
    if value < max {
        out.push(first | value as u8);
        return;
    }
    out.push(first | max as u8);
    let mut v = value - max;
    while v >= 128 {
        out.push((v % 128) as u8 | 0x80);
        v /= 128;
    }
    out.push(v as u8);
}

/// Decode an N-bit-prefix integer at `buf[*pos]`.
pub(crate) fn decode_int(buf: &[u8], pos: &mut usize, prefix_bits: u8) -> Result<usize, HpackError> {
    let max = (1usize << prefix_bits) - 1;
    let b = *buf.get(*pos).ok_or(HpackError::Truncated)?;
    *pos += 1;
    let mut value = (b as usize) & max;
    if value < max {
        return Ok(value);
    }
    let mut shift = 0u32;
    loop {
        let b = *buf.get(*pos).ok_or(HpackError::Truncated)?;
        *pos += 1;
        if shift > 28 {
            return Err(HpackError::IntegerOverflow);
        }
        value = value.checked_add(((b & 0x7f) as usize) << shift).ok_or(HpackError::IntegerOverflow)?;
        if b & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
}

// -------------------------------------------------------------- Huffman

/// Huffman-encode `data` (RFC 7541 Appendix B), padding with EOS bits.
pub fn huffman_encode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut acc: u64 = 0;
    let mut bits: u32 = 0;
    for &b in data {
        let (code, len) = HUFFMAN_CODES[b as usize];
        acc = (acc << len) | code as u64;
        bits += len as u32;
        while bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
        acc &= (1u64 << bits) - 1;
    }
    if bits > 0 {
        let pad = 8 - bits;
        out.push(((acc << pad) | ((1u64 << pad) - 1)) as u8);
    }
    out
}

/// Length in bytes of the Huffman encoding of `data`.
pub fn huffman_encoded_len(data: &[u8]) -> usize {
    let bits: usize = data.iter().map(|&b| HUFFMAN_CODES[b as usize].1 as usize).sum();
    bits.div_ceil(8)
}

/// Decoding tree: node i has children `[zero, one]`; a child value with the
/// high bit set is a leaf holding a symbol.
struct HuffTree {
    nodes: Vec<[u32; 2]>,
}

const LEAF: u32 = 0x8000_0000;
const EMPTY: u32 = 0;

fn huff_tree() -> &'static HuffTree {
    static TREE: OnceLock<HuffTree> = OnceLock::new();
    TREE.get_or_init(|| {
        let mut nodes = vec![[EMPTY, EMPTY]];
        for (sym, &(code, len)) in HUFFMAN_CODES.iter().enumerate() {
            let mut n = 0usize;
            for i in (0..len).rev() {
                let bit = ((code >> i) & 1) as usize;
                if i == 0 {
                    nodes[n][bit] = LEAF | sym as u32;
                } else {
                    if nodes[n][bit] == EMPTY {
                        nodes.push([EMPTY, EMPTY]);
                        nodes[n][bit] = (nodes.len() - 1) as u32;
                    }
                    n = nodes[n][bit] as usize;
                }
            }
        }
        HuffTree { nodes }
    })
}

/// Decode a Huffman-coded string. Rejects EOS in the data, padding longer
/// than 7 bits and padding that is not all ones.
pub fn huffman_decode(data: &[u8]) -> Result<Vec<u8>, HpackError> {
    let tree = huff_tree();
    let mut out = Vec::with_capacity(data.len() * 8 / 5);
    let mut node = 0usize;
    // Bits consumed since the last emitted symbol, and whether all were 1.
    let mut pending = 0u32;
    let mut all_ones = true;
    for &byte in data {
        for i in (0..8).rev() {
            let bit = ((byte >> i) & 1) as usize;
            pending += 1;
            all_ones &= bit == 1;
            let next = tree.nodes[node][bit];
            if next & LEAF != 0 {
                let sym = next & !LEAF;
                if sym == 256 {
                    return Err(HpackError::BadHuffman);
                }
                out.push(sym as u8);
                node = 0;
                pending = 0;
                all_ones = true;
            } else if next == EMPTY {
                return Err(HpackError::BadHuffman);
            } else {
                node = next as usize;
            }
        }
    }
    if pending > 7 || !all_ones {
        return Err(HpackError::BadHuffman);
    }
    Ok(out)
}

// ---------------------------------------------------------------- table

#[derive(Debug, Default)]
struct DynamicTable {
    /// Newest first.
    entries: VecDeque<(String, String)>,
    size: usize,
    max_size: usize,
}

fn entry_size(name: &str, value: &str) -> usize {
    name.len() + value.len() + 32
}

impl DynamicTable {
    fn new(max_size: usize) -> DynamicTable {
        DynamicTable { entries: VecDeque::new(), size: 0, max_size }
    }

    fn set_max_size(&mut self, max: usize) {
        self.max_size = max;
        self.evict(0);
    }

    fn evict(&mut self, incoming: usize) {
        while self.size + incoming > self.max_size {
            match self.entries.pop_back() {
                Some((n, v)) => self.size -= entry_size(&n, &v),
                None => break,
            }
        }
    }

    fn insert(&mut self, name: String, value: String) {
        let sz = entry_size(&name, &value);
        if sz > self.max_size {
            // An entry larger than the table empties it (RFC 7541 4.4).
            self.entries.clear();
            self.size = 0;
            return;
        }
        self.evict(sz);
        self.size += sz;
        self.entries.push_front((name, value));
    }

    /// 1-based HPACK index across static + dynamic tables.
    fn get(&self, index: usize) -> Option<(&str, &str)> {
        if index == 0 {
            return None;
        }
        if index <= STATIC_TABLE.len() {
            let (n, v) = STATIC_TABLE[index - 1];
            return Some((n, v));
        }
        self.entries.get(index - STATIC_TABLE.len() - 1).map(|(n, v)| (n.as_str(), v.as_str()))
    }
}

// -------------------------------------------------------------- decoder

/// Stateful HPACK decoder (one per connection, receive direction).
#[derive(Debug)]
pub struct Decoder {
    table: DynamicTable,
    /// Upper bound the peer may set via a size update (our SETTINGS value).
    pub max_allowed: usize,
    /// Maximum total decoded header list size (name + value + 32 each).
    pub max_list_size: usize,
}

impl Decoder {
    pub fn new(max_table_size: usize) -> Decoder {
        Decoder { table: DynamicTable::new(max_table_size), max_allowed: max_table_size, max_list_size: 256 * 1024 }
    }

    /// Current dynamic table size in octets (for tests).
    pub fn table_size(&self) -> usize {
        self.table.size
    }

    /// Dynamic table entries, newest first (for tests).
    pub fn table_entries(&self) -> Vec<(String, String)> {
        self.table.entries.iter().cloned().collect()
    }

    fn read_string(buf: &[u8], pos: &mut usize) -> Result<String, HpackError> {
        let huff = buf.get(*pos).ok_or(HpackError::Truncated)? & 0x80 != 0;
        let len = decode_int(buf, pos, 7)?;
        let end = pos.checked_add(len).ok_or(HpackError::Truncated)?;
        let raw = buf.get(*pos..end).ok_or(HpackError::Truncated)?;
        *pos = end;
        let bytes = if huff { huffman_decode(raw)? } else { raw.to_vec() };
        // Header values are octets; keep invalid UTF-8 lossily.
        Ok(match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
        })
    }

    /// Decode one complete header block.
    pub fn decode(&mut self, buf: &[u8]) -> Result<Vec<(String, String)>, HpackError> {
        let mut out = Vec::new();
        let mut pos = 0;
        let mut list_size = 0usize;
        let mut seen_field = false;
        while pos < buf.len() {
            let b = buf[pos];
            let (name, value) = if b & 0x80 != 0 {
                // Indexed header field.
                let idx = decode_int(buf, &mut pos, 7)?;
                let (n, v) = self.table.get(idx).ok_or(HpackError::BadIndex(idx))?;
                (n.to_string(), v.to_string())
            } else if b & 0xc0 == 0x40 {
                // Literal with incremental indexing.
                let idx = decode_int(buf, &mut pos, 6)?;
                let name = if idx == 0 {
                    Self::read_string(buf, &mut pos)?
                } else {
                    self.table.get(idx).ok_or(HpackError::BadIndex(idx))?.0.to_string()
                };
                let value = Self::read_string(buf, &mut pos)?;
                self.table.insert(name.clone(), value.clone());
                (name, value)
            } else if b & 0xe0 == 0x20 {
                // Dynamic table size update: only before the first field.
                if seen_field {
                    return Err(HpackError::BadTableSizeUpdate);
                }
                let size = decode_int(buf, &mut pos, 5)?;
                if size > self.max_allowed {
                    return Err(HpackError::BadTableSizeUpdate);
                }
                self.table.set_max_size(size);
                continue;
            } else {
                // Literal without indexing (0000) or never indexed (0001).
                let idx = decode_int(buf, &mut pos, 4)?;
                let name = if idx == 0 {
                    Self::read_string(buf, &mut pos)?
                } else {
                    self.table.get(idx).ok_or(HpackError::BadIndex(idx))?.0.to_string()
                };
                let value = Self::read_string(buf, &mut pos)?;
                (name, value)
            };
            seen_field = true;
            list_size += entry_size(&name, &value);
            if list_size > self.max_list_size {
                return Err(HpackError::HeaderListTooLarge);
            }
            out.push((name, value));
        }
        Ok(out)
    }
}

// -------------------------------------------------------------- encoder

/// Stateful HPACK encoder (one per connection, send direction).
#[derive(Debug)]
pub struct Encoder {
    table: DynamicTable,
    /// Table size change to announce at the start of the next block.
    pending_size_update: Option<usize>,
    /// Use Huffman coding when it is shorter.
    pub huffman: bool,
}

impl Default for Encoder {
    fn default() -> Self {
        Encoder::new(4096)
    }
}

impl Encoder {
    pub fn new(max_table_size: usize) -> Encoder {
        Encoder { table: DynamicTable::new(max_table_size), pending_size_update: None, huffman: true }
    }

    /// The peer changed SETTINGS_HEADER_TABLE_SIZE; we use at most 4096.
    pub fn set_peer_max_size(&mut self, peer_max: usize) {
        let size = peer_max.min(4096);
        if size != self.table.max_size {
            self.table.set_max_size(size);
            self.pending_size_update = Some(size);
        }
    }

    fn write_string(&self, out: &mut Vec<u8>, s: &str) {
        let bytes = s.as_bytes();
        if self.huffman {
            let hl = huffman_encoded_len(bytes);
            if hl < bytes.len() {
                encode_int(out, hl, 7, 0x80);
                out.extend_from_slice(&huffman_encode(bytes));
                return;
            }
        }
        encode_int(out, bytes.len(), 7, 0);
        out.extend_from_slice(bytes);
    }

    /// Find (exact match index, name-only match index).
    fn find(&self, name: &str, value: &str) -> (Option<usize>, Option<usize>) {
        let mut name_idx = None;
        for (i, (n, v)) in STATIC_TABLE.iter().enumerate() {
            if *n == name {
                if *v == value {
                    return (Some(i + 1), Some(i + 1));
                }
                name_idx.get_or_insert(i + 1);
            }
        }
        for (i, (n, v)) in self.table.entries.iter().enumerate() {
            if n == name {
                let idx = STATIC_TABLE.len() + 1 + i;
                if v == value {
                    return (Some(idx), Some(idx));
                }
                name_idx.get_or_insert(idx);
            }
        }
        (None, name_idx)
    }

    /// Encode a header list (names must already be lower-case).
    pub fn encode(&mut self, headers: &[(String, String)]) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(size) = self.pending_size_update.take() {
            encode_int(&mut out, size, 5, 0x20);
        }
        for (name, value) in headers {
            let (exact, name_idx) = self.find(name, value);
            if let Some(idx) = exact {
                encode_int(&mut out, idx, 7, 0x80);
                continue;
            }
            let sensitive = matches!(name.as_str(), "authorization" | "proxy-authorization")
                || (name == "cookie" && value.len() < 20);
            if sensitive {
                // Never indexed.
                match name_idx {
                    Some(i) => encode_int(&mut out, i, 4, 0x10),
                    None => {
                        out.push(0x10);
                        self.write_string(&mut out, name);
                    }
                }
                self.write_string(&mut out, value);
                continue;
            }
            // Values that change every request (paths, referers) would
            // only churn the table; index them only when small.
            let index_it = entry_size(name, value) <= self.table.max_size / 2 && !matches!(name.as_str(), ":path" | "content-length" | "if-none-match" | "if-modified-since");
            let (prefix, flag) = if index_it { (6, 0x40) } else { (4, 0x00) };
            match name_idx {
                Some(i) => encode_int(&mut out, i, prefix, flag),
                None => {
                    out.push(flag);
                    self.write_string(&mut out, name);
                }
            }
            self.write_string(&mut out, value);
            if index_it {
                self.table.insert(name.clone(), value.clone());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn integers_c1() {
        // C.1.1: 10 with 5-bit prefix.
        let mut out = Vec::new();
        encode_int(&mut out, 10, 5, 0);
        assert_eq!(out, vec![0x0a]);
        // C.1.2: 1337 with 5-bit prefix.
        out.clear();
        encode_int(&mut out, 1337, 5, 0);
        assert_eq!(out, vec![0x1f, 0x9a, 0x0a]);
        let mut pos = 0;
        assert_eq!(decode_int(&out, &mut pos, 5).unwrap(), 1337);
        // C.1.3: 42 at an octet boundary.
        out.clear();
        encode_int(&mut out, 42, 8, 0);
        assert_eq!(out, vec![0x2a]);
        // Overflow is rejected.
        let mut pos = 0;
        assert!(decode_int(&[0x1f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f], &mut pos, 5).is_err());
    }

    #[test]
    fn literal_c2() {
        // C.2.1 literal with indexing.
        let mut d = Decoder::new(4096);
        let r = d.decode(&hex("400a637573746f6d2d6b65790d637573746f6d2d686561646572")).unwrap();
        assert_eq!(r, h(&[("custom-key", "custom-header")]));
        assert_eq!(d.table_size(), 55);
        // C.2.2 without indexing.
        let mut d = Decoder::new(4096);
        let r = d.decode(&hex("040c2f73616d706c652f70617468")).unwrap();
        assert_eq!(r, h(&[(":path", "/sample/path")]));
        assert_eq!(d.table_size(), 0);
        // C.2.3 never indexed.
        let mut d = Decoder::new(4096);
        let r = d.decode(&hex("100870617373776f726406736563726574")).unwrap();
        assert_eq!(r, h(&[("password", "secret")]));
        // C.2.4 indexed.
        let mut d = Decoder::new(4096);
        assert_eq!(d.decode(&hex("82")).unwrap(), h(&[(":method", "GET")]));
    }

    fn requests() -> [Vec<(String, String)>; 3] {
        [
            h(&[(":method", "GET"), (":scheme", "http"), (":path", "/"), (":authority", "www.example.com")]),
            h(&[(":method", "GET"), (":scheme", "http"), (":path", "/"), (":authority", "www.example.com"), ("cache-control", "no-cache")]),
            h(&[(":method", "GET"), (":scheme", "https"), (":path", "/index.html"), (":authority", "www.example.com"), ("custom-key", "custom-value")]),
        ]
    }

    #[test]
    fn requests_c3_and_c4() {
        for (vectors, sizes) in [
            (
                ["828684410f7777772e6578616d706c652e636f6d", "828684be58086e6f2d6361636865", "828785bf400a637573746f6d2d6b65790c637573746f6d2d76616c7565"],
                [57, 110, 164],
            ),
            (
                ["828684418cf1e3c2e5f23a6ba0ab90f4ff", "828684be5886a8eb10649cbf", "828785bf408825a849e95ba97d7f8925a849e95bb8e8b4bf"],
                [57, 110, 164],
            ),
        ] {
            let mut d = Decoder::new(4096);
            for ((v, want), size) in vectors.iter().zip(requests()).zip(sizes) {
                assert_eq!(d.decode(&hex(v)).unwrap(), want);
                assert_eq!(d.table_size(), size);
            }
            let entries = d.table_entries();
            assert_eq!(entries[0], ("custom-key".to_string(), "custom-value".to_string()));
            assert_eq!(entries[2], (":authority".to_string(), "www.example.com".to_string()));
        }
    }

    #[test]
    fn encoder_matches_c4() {
        // Our encoder's choices coincide with the RFC's for these requests
        // (indexed :authority etc.) when Huffman coding is on.
        let mut e = Encoder::new(4096);
        let want = ["828684418cf1e3c2e5f23a6ba0ab90f4ff", "828684be5886a8eb10649cbf", "828785bf408825a849e95ba97d7f8925a849e95bb8e8b4bf"];
        for (req, w) in requests().iter().zip(want) {
            assert_eq!(e.encode(req), hex(w));
        }
        let mut e = Encoder::new(4096);
        e.huffman = false;
        let want = ["828684410f7777772e6578616d706c652e636f6d", "828684be58086e6f2d6361636865", "828785bf400a637573746f6d2d6b65790c637573746f6d2d76616c7565"];
        for (req, w) in requests().iter().zip(want) {
            assert_eq!(e.encode(req), hex(w));
        }
    }

    #[test]
    fn responses_c5_and_c6_with_eviction() {
        let r1 = h(&[(":status", "302"), ("cache-control", "private"), ("date", "Mon, 21 Oct 2013 20:13:21 GMT"), ("location", "https://www.example.com")]);
        let r2 = h(&[(":status", "307"), ("cache-control", "private"), ("date", "Mon, 21 Oct 2013 20:13:21 GMT"), ("location", "https://www.example.com")]);
        let r3 = h(&[
            (":status", "200"),
            ("cache-control", "private"),
            ("date", "Mon, 21 Oct 2013 20:13:22 GMT"),
            ("location", "https://www.example.com"),
            ("content-encoding", "gzip"),
            ("set-cookie", "foo=ASDJKHQKBZXOQWEOPIUAXQWEOIU; max-age=3600; version=1"),
        ]);
        let plain = [
            "4803333032580770726976617465611d4d6f6e2c203231204f637420323031332032303a31333a323120474d546e1768747470733a2f2f7777772e6578616d706c652e636f6d",
            "4803333037c1c0bf",
            "88c1611d4d6f6e2c203231204f637420323031332032303a31333a323220474d54c05a04677a69707738666f6f3d4153444a4b48514b425a584f5157454f50495541585157454f49553b206d61782d6167653d333630303b2076657273696f6e3d31",
        ];
        let huff = [
            "488264025885aec3771a4b6196d07abe941054d444a8200595040b8166e082a62d1bff6e919d29ad171863c78f0b97c8e9ae82ae43d3",
            "4883640effc1c0bf",
            "88c16196d07abe941054d444a8200595040b8166e084a62d1bffc05a839bd9ab77ad94e7821dd7f2e6c7b335dfdfcd5b3960d5af27087f3672c1ab270fb5291f9587316065c003ed4ee5b1063d5007",
        ];
        for vectors in [plain, huff] {
            let mut d = Decoder::new(256);
            assert_eq!(d.decode(&hex(vectors[0])).unwrap(), r1);
            assert_eq!(d.table_size(), 222);
            assert_eq!(d.decode(&hex(vectors[1])).unwrap(), r2);
            assert_eq!(d.table_size(), 222);
            assert_eq!(d.decode(&hex(vectors[2])).unwrap(), r3);
            assert_eq!(d.table_size(), 215);
            let e = d.table_entries();
            assert_eq!(e.len(), 3);
            assert_eq!(e[0].0, "set-cookie");
            assert_eq!(e[1], ("content-encoding".to_string(), "gzip".to_string()));
            assert_eq!(e[2], ("date".to_string(), "Mon, 21 Oct 2013 20:13:22 GMT".to_string()));
        }
    }

    #[test]
    fn huffman_roundtrip_and_errors() {
        assert_eq!(huffman_encode(b"www.example.com"), hex("f1e3c2e5f23a6ba0ab90f4ff"));
        assert_eq!(huffman_decode(&hex("f1e3c2e5f23a6ba0ab90f4ff")).unwrap(), b"www.example.com");
        let all: Vec<u8> = (0..=255u8).collect();
        assert_eq!(huffman_decode(&huffman_encode(&all)).unwrap(), all);
        // Padding of zeros is invalid; a full byte of padding is invalid.
        // '0' is the 5-bit code 00000: followed by 111 it is valid padding,
        // followed by 000 it is not.
        assert_eq!(huffman_decode(&[0x07]).unwrap(), b"0");
        assert_eq!(huffman_decode(&[0x00]).unwrap_err(), HpackError::BadHuffman);
        assert!(huffman_decode(&[0xff]).is_err());
        // EOS encoded explicitly.
        assert!(huffman_decode(&[0xff, 0xff, 0xff, 0xff]).is_err());
    }

    #[test]
    fn size_update_rules() {
        let mut d = Decoder::new(4096);
        // Update to 0 then a field: fine.
        assert_eq!(d.decode(&[0x20, 0x82]).unwrap(), h(&[(":method", "GET")]));
        // Update after a field: error.
        let mut d = Decoder::new(4096);
        assert_eq!(d.decode(&[0x82, 0x20]).unwrap_err(), HpackError::BadTableSizeUpdate);
        // Update above the allowed maximum.
        let mut d = Decoder::new(100);
        let mut buf = Vec::new();
        encode_int(&mut buf, 4096, 5, 0x20);
        assert_eq!(d.decode(&buf).unwrap_err(), HpackError::BadTableSizeUpdate);
        // Encoder announces a reduced size.
        let mut e = Encoder::new(4096);
        e.set_peer_max_size(0);
        let block = e.encode(&h(&[("x-a", "b")]));
        assert_eq!(block[0], 0x20);
        let mut d = Decoder::new(4096);
        assert_eq!(d.decode(&block).unwrap(), h(&[("x-a", "b")]));
        assert_eq!(d.table_size(), 0);
    }

    #[test]
    fn encoder_decoder_roundtrip_many() {
        let mut e = Encoder::new(4096);
        let mut d = Decoder::new(4096);
        for i in 0..200 {
            let hdrs = h(&[
                (":method", "GET"),
                (":scheme", "https"),
                (":authority", "example.org"),
                (":path", &format!("/img/{i}.png")),
                ("user-agent", "Mozilla/5.0 test"),
                ("x-counter", &i.to_string()),
                ("authorization", "Bearer abc"),
            ]);
            assert_eq!(d.decode(&e.encode(&hdrs)).unwrap(), hdrs);
        }
    }
}
