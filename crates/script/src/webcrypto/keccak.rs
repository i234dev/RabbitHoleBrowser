//! Keccak-based functions of FIPS 202, SP 800-185 and RFC 9861: SHA-3,
//! cSHAKE, KMAC, TurboSHAKE and KangarooTwelve (KT128 / KT256).

const RC: [u64; 24] = [
    0x0000000000000001, 0x0000000000008082, 0x800000000000808A, 0x8000000080008000,
    0x000000000000808B, 0x0000000080000001, 0x8000000080008081, 0x8000000000008009,
    0x000000000000008A, 0x0000000000000088, 0x0000000080008009, 0x000000008000000A,
    0x000000008000808B, 0x800000000000008B, 0x8000000000008089, 0x8000000000008003,
    0x8000000000008002, 0x8000000000000080, 0x000000000000800A, 0x800000008000000A,
    0x8000000080008081, 0x8000000000008080, 0x0000000080000001, 0x8000000080008008,
];
const RHO: [u32; 24] = [1, 3, 6, 10, 15, 21, 28, 36, 45, 55, 2, 14, 27, 41, 56, 8, 25, 43, 62, 18, 39, 61, 20, 44];
const PI: [usize; 24] = [10, 7, 11, 17, 18, 3, 5, 16, 8, 21, 24, 4, 15, 23, 19, 13, 12, 2, 20, 14, 22, 9, 6, 1];

/// Keccak-p[1600, rounds]: the last `rounds` rounds of Keccak-f[1600].
fn permute(a: &mut [u64; 25], rounds: usize) {
    for &rc in &RC[24 - rounds..] {
        let mut c = [0u64; 5];
        for x in 0..5 { c[x] = a[x] ^ a[x + 5] ^ a[x + 10] ^ a[x + 15] ^ a[x + 20]; }
        for x in 0..5 {
            let d = c[(x + 4) % 5] ^ c[(x + 1) % 5].rotate_left(1);
            for y in 0..5 { a[x + 5 * y] ^= d; }
        }
        let mut last = a[1];
        for i in 0..24 {
            let j = PI[i];
            let t = a[j];
            a[j] = last.rotate_left(RHO[i]);
            last = t;
        }
        for y in 0..5 {
            let row = [a[5 * y], a[5 * y + 1], a[5 * y + 2], a[5 * y + 3], a[5 * y + 4]];
            for x in 0..5 { a[5 * y + x] = row[x] ^ (!row[(x + 1) % 5] & row[(x + 2) % 5]); }
        }
        a[0] ^= rc;
    }
}

/// A Keccak sponge absorbing bytes with `rate` bytes per block.
#[derive(Clone)]
pub(super) struct Sponge {
    state: [u64; 25],
    rate: usize,
    rounds: usize,
    pos: usize,
}

impl Sponge {
    pub(super) fn new(rate: usize, rounds: usize) -> Sponge {
        Sponge { state: [0; 25], rate, rounds, pos: 0 }
    }
    fn xor_byte(&mut self, i: usize, b: u8) {
        self.state[i / 8] ^= (b as u64) << (8 * (i % 8));
    }
    pub(super) fn absorb(&mut self, data: &[u8]) {
        for &b in data {
            self.xor_byte(self.pos, b);
            self.pos += 1;
            if self.pos == self.rate {
                permute(&mut self.state, self.rounds);
                self.pos = 0;
            }
        }
    }
    /// Pad with the domain separation byte and squeeze `len` bytes.
    pub(super) fn finish(mut self, domain: u8, len: usize) -> Vec<u8> {
        self.xor_byte(self.pos, domain);
        self.xor_byte(self.rate - 1, 0x80);
        permute(&mut self.state, self.rounds);
        let mut out = Vec::with_capacity(len);
        let mut i = 0;
        while out.len() < len {
            if i == self.rate {
                permute(&mut self.state, self.rounds);
                i = 0;
            }
            out.push((self.state[i / 8] >> (8 * (i % 8))) as u8);
            i += 1;
        }
        out
    }
}

/// Rate in bytes of a 128- or 256-bit security level XOF.
fn xof_rate(level: u32) -> usize { if level == 128 { 168 } else { 136 } }

pub(super) fn sha3(bits: usize, data: &[u8]) -> Vec<u8> {
    let mut s = Sponge::new(200 - bits / 4, 24);
    s.absorb(data);
    s.finish(0x06, bits / 8)
}

pub(super) fn shake(level: u32, data: &[u8], len: usize) -> Vec<u8> {
    let mut s = Sponge::new(xof_rate(level), 24);
    s.absorb(data);
    s.finish(0x1f, len)
}

fn left_encode(x: u64) -> Vec<u8> {
    let bytes = x.to_be_bytes();
    let skip = bytes.iter().position(|&b| b != 0).unwrap_or(7);
    let mut out = vec![(8 - skip) as u8];
    out.extend_from_slice(&bytes[skip..]);
    out
}

fn right_encode(x: u64) -> Vec<u8> {
    let bytes = x.to_be_bytes();
    let skip = bytes.iter().position(|&b| b != 0).unwrap_or(7);
    let mut out = bytes[skip..].to_vec();
    out.push((8 - skip) as u8);
    out
}

fn encode_string(s: &[u8]) -> Vec<u8> {
    let mut out = left_encode(s.len() as u64 * 8);
    out.extend_from_slice(s);
    out
}

fn absorb_bytepad(sponge: &mut Sponge, parts: &[&[u8]]) {
    let w = sponge.rate;
    let mut total = 0;
    let head = left_encode(w as u64);
    sponge.absorb(&head);
    total += head.len();
    for part in parts {
        let enc = encode_string(part);
        sponge.absorb(&enc);
        total += enc.len();
    }
    let pad = (w - total % w) % w;
    sponge.absorb(&vec![0; pad]);
}

pub(super) fn cshake(level: u32, data: &[u8], len: usize, name: &[u8], custom: &[u8]) -> Vec<u8> {
    if name.is_empty() && custom.is_empty() { return shake(level, data, len); }
    let mut s = Sponge::new(xof_rate(level), 24);
    absorb_bytepad(&mut s, &[name, custom]);
    s.absorb(data);
    s.finish(0x04, len)
}

pub(super) fn kmac(level: u32, key: &[u8], data: &[u8], len: usize, custom: &[u8]) -> Vec<u8> {
    let mut s = Sponge::new(xof_rate(level), 24);
    absorb_bytepad(&mut s, &[b"KMAC", custom]);
    absorb_bytepad(&mut s, &[key]);
    s.absorb(data);
    s.absorb(&right_encode(len as u64 * 8));
    s.finish(0x04, len)
}

pub(super) fn turboshake(level: u32, data: &[u8], domain: u8, len: usize) -> Vec<u8> {
    let mut s = Sponge::new(xof_rate(level), 12);
    s.absorb(data);
    s.finish(domain, len)
}

fn length_encode(x: usize) -> Vec<u8> {
    let bytes = (x as u64).to_be_bytes();
    let skip = bytes.iter().position(|&b| b != 0).unwrap_or(8);
    let mut out = bytes[skip..].to_vec();
    out.push((8 - skip) as u8);
    out
}

/// KangarooTwelve (RFC 9861): KT128 over TurboSHAKE128, KT256 over
/// TurboSHAKE256, with 8192-byte chunks.
pub(super) fn kangaroo(level: u32, message: &[u8], custom: &[u8], len: usize) -> Vec<u8> {
    const CHUNK: usize = 8192;
    let suffix = length_encode(custom.len());
    let total = message.len() + custom.len() + suffix.len();
    // Byte i of S = M || C || length_encode(|C|), read in ranges.
    let feed = |sponge: &mut Sponge, from: usize, to: usize| {
        let parts: [&[u8]; 3] = [message, custom, &suffix];
        let mut offset = 0;
        for part in parts {
            let (start, end) = (from.max(offset), to.min(offset + part.len()));
            if start < end { sponge.absorb(&part[start - offset..end - offset]); }
            offset += part.len();
        }
    };
    let rate = xof_rate(level);
    let mut root = Sponge::new(rate, 12);
    if total <= CHUNK {
        feed(&mut root, 0, total);
        return root.finish(0x07, len);
    }
    feed(&mut root, 0, CHUNK);
    root.absorb(&[0x03, 0, 0, 0, 0, 0, 0, 0]);
    let cv_len = if level == 128 { 32 } else { 64 };
    let mut chunks = 0;
    let mut start = CHUNK;
    while start < total {
        let end = (start + CHUNK).min(total);
        let mut leaf = Sponge::new(rate, 12);
        feed(&mut leaf, start, end);
        root.absorb(&leaf.finish(0x0b, cv_len));
        chunks += 1;
        start = end;
    }
    root.absorb(&length_encode(chunks));
    root.absorb(&[0xff, 0xff]);
    root.finish(0x06, len)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }

    #[test]
    fn known_answers() {
        assert_eq!(hex(&sha3(256, b"")), "a7ffc6f8bf1ed76651c14756a061d662f580ff4de43b49fa82d80a4b80f8434a");
        assert_eq!(hex(&shake(128, b"", 8)), "7f9c2ba4e88f827d");
        // SP 800-185 cSHAKE128 sample #1.
        assert_eq!(hex(&cshake(128, &[0, 1, 2, 3], 32, b"", b"Email Signature")),
            "c1c36925b6409a04f1b504fcbca9d82b4017277cb5ed2b2065fc1d3814d5aaf5");
        // SP 800-185 KMAC128 sample #1.
        let key: Vec<u8> = (0x40..0x60).collect();
        assert_eq!(hex(&kmac(128, &key, &[0, 1, 2, 3], 32, b"")),
            "e5780b0d3ea6f7d3a429c5706aa43a00fadbd7d49628839e3187243f456ee14e");
        // RFC 9861 TurboSHAKE128(M=empty, D=0x1F), 32 bytes.
        assert_eq!(hex(&turboshake(128, b"", 0x1f, 32)),
            "1e415f1c5983aff2169217277d17bb538cd945a397ddec541f1ce41af2c1b74c");
        // RFC 9861 KT128(M=empty, C=empty), 32 bytes.
        assert_eq!(hex(&kangaroo(128, b"", b"", 32)),
            "1ac2d450fc3b4205d19da7bfca1b37513c0803577ac7167f06fe2ce1f0ef39e5");
        // RFC 9861 KT128(M=ptn(17^4)), 32 bytes: more than one chunk.
        let m: Vec<u8> = (0..17usize.pow(4)).map(|i| (i % 251) as u8).collect();
        assert_eq!(hex(&kangaroo(128, &m, b"", 32)),
            "8701045e22205345ff4dda05555cbb5c3af1a771c2b89baef37db43d9998b9fe");
    }
}
