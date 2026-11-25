//! Argon2d / Argon2i / Argon2id version 0x13 (RFC 9106) over BLAKE2b.

const IV: [u64; 8] = [
    0x6a09e667f3bcc908, 0xbb67ae8584caa73b, 0x3c6ef372fe94f82b, 0xa54ff53a5f1d36f1,
    0x510e527fade682d1, 0x9b05688c2b3e6c1f, 0x1f83d9abfb41bd6b, 0x5be0cd19137e2179,
];
const SIGMA: [[usize; 16]; 12] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
    [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
    [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
    [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
    [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
    [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
    [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
    [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
    [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
];

/// BLAKE2b (RFC 7693) without a key, with an output of 1..=64 bytes.
fn blake2b(out_len: usize, parts: &[&[u8]]) -> Vec<u8> {
    let mut h = IV;
    h[0] ^= 0x0101_0000 ^ out_len as u64;
    let data: Vec<u8> = parts.concat();
    let mut t: u128 = 0;
    let blocks = if data.is_empty() { 1 } else { data.len().div_ceil(128) };
    for b in 0..blocks {
        let chunk = &data[b * 128..data.len().min(b * 128 + 128)];
        let mut block = [0u8; 128];
        block[..chunk.len()].copy_from_slice(chunk);
        t += chunk.len() as u128;
        let last = b + 1 == blocks;
        let m: Vec<u64> = block.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect();
        let mut v = [0u64; 16];
        v[..8].copy_from_slice(&h);
        v[8..].copy_from_slice(&IV);
        v[12] ^= t as u64;
        v[13] ^= (t >> 64) as u64;
        if last { v[14] = !v[14]; }
        for s in &SIGMA {
            let mut g = |a: usize, b: usize, c: usize, d: usize, x: u64, y: u64| {
                v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
                v[d] = (v[d] ^ v[a]).rotate_right(32);
                v[c] = v[c].wrapping_add(v[d]);
                v[b] = (v[b] ^ v[c]).rotate_right(24);
                v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
                v[d] = (v[d] ^ v[a]).rotate_right(16);
                v[c] = v[c].wrapping_add(v[d]);
                v[b] = (v[b] ^ v[c]).rotate_right(63);
            };
            g(0, 4, 8, 12, m[s[0]], m[s[1]]);
            g(1, 5, 9, 13, m[s[2]], m[s[3]]);
            g(2, 6, 10, 14, m[s[4]], m[s[5]]);
            g(3, 7, 11, 15, m[s[6]], m[s[7]]);
            g(0, 5, 10, 15, m[s[8]], m[s[9]]);
            g(1, 6, 11, 12, m[s[10]], m[s[11]]);
            g(2, 7, 8, 13, m[s[12]], m[s[13]]);
            g(3, 4, 9, 14, m[s[14]], m[s[15]]);
        }
        for i in 0..8 { h[i] ^= v[i] ^ v[i + 8]; }
    }
    h.iter().flat_map(|w| w.to_le_bytes()).take(out_len).collect()
}

/// H' (RFC 9106 section 3.3): variable-length hash.
fn hash_long(len: usize, input: &[&[u8]]) -> Vec<u8> {
    let prefix = (len as u32).to_le_bytes();
    let mut parts = vec![&prefix[..]];
    parts.extend_from_slice(input);
    if len <= 64 { return blake2b(len, &parts); }
    let r = len.div_ceil(32) - 2;
    let mut v = blake2b(64, &parts);
    let mut out = v[..32].to_vec();
    for _ in 1..r {
        v = blake2b(64, &[&v]);
        out.extend_from_slice(&v[..32]);
    }
    out.extend(blake2b(len - 32 * r, &[&v]));
    out
}

type Block = [u64; 128];

fn gb(v: &mut Block, a: usize, b: usize, c: usize, d: usize) {
    let f = |x: u64, y: u64| x.wrapping_add(y).wrapping_add(2u64.wrapping_mul(x & 0xffff_ffff).wrapping_mul(y & 0xffff_ffff));
    v[a] = f(v[a], v[b]);
    v[d] = (v[d] ^ v[a]).rotate_right(32);
    v[c] = f(v[c], v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(24);
    v[a] = f(v[a], v[b]);
    v[d] = (v[d] ^ v[a]).rotate_right(16);
    v[c] = f(v[c], v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(63);
}

/// Permutation P over 16 words given by their indices.
fn permute(v: &mut Block, i: [usize; 16]) {
    gb(v, i[0], i[4], i[8], i[12]);
    gb(v, i[1], i[5], i[9], i[13]);
    gb(v, i[2], i[6], i[10], i[14]);
    gb(v, i[3], i[7], i[11], i[15]);
    gb(v, i[0], i[5], i[10], i[15]);
    gb(v, i[1], i[6], i[11], i[12]);
    gb(v, i[2], i[7], i[8], i[13]);
    gb(v, i[3], i[4], i[9], i[14]);
}

/// Compression function G(X, Y).
fn compress(x: &Block, y: &Block) -> Block {
    let mut r = [0u64; 128];
    for i in 0..128 { r[i] = x[i] ^ y[i]; }
    let mut z = r;
    for row in 0..8 {
        let mut idx = [0usize; 16];
        for (k, slot) in idx.iter_mut().enumerate() { *slot = row * 16 + k; }
        permute(&mut z, idx);
    }
    for col in 0..8 {
        let mut idx = [0usize; 16];
        for k in 0..8 {
            idx[2 * k] = k * 16 + 2 * col;
            idx[2 * k + 1] = k * 16 + 2 * col + 1;
        }
        permute(&mut z, idx);
    }
    for i in 0..128 { z[i] ^= r[i]; }
    z
}

fn to_block(bytes: &[u8]) -> Block {
    let mut b = [0u64; 128];
    for (i, c) in bytes.chunks_exact(8).enumerate() { b[i] = u64::from_le_bytes(c.try_into().unwrap()); }
    b
}

pub(super) struct Params<'a> {
    pub password: &'a [u8],
    pub salt: &'a [u8],
    pub secret: &'a [u8],
    pub data: &'a [u8],
    pub lanes: u32,
    pub memory: u32,
    pub passes: u32,
    pub kind: u32,
    pub tag_len: usize,
}

pub(super) fn argon2(p: &Params) -> Option<Vec<u8>> {
    let lanes = p.lanes as usize;
    if lanes == 0 || p.passes == 0 || (p.memory as usize) < 8 * lanes || p.tag_len < 4 { return None; }
    let le = |x: u32| x.to_le_bytes();
    let h0 = blake2b(64, &[
        &le(p.lanes), &le(p.tag_len as u32), &le(p.memory), &le(p.passes), &le(0x13), &le(p.kind),
        &le(p.password.len() as u32), p.password, &le(p.salt.len() as u32), p.salt,
        &le(p.secret.len() as u32), p.secret, &le(p.data.len() as u32), p.data,
    ]);
    let blocks = 4 * lanes * (p.memory as usize / (4 * lanes));
    let q = blocks / lanes;
    let seg = q / 4;
    let mut mem: Vec<Block> = Vec::new();
    mem.try_reserve_exact(blocks).ok()?;
    mem.resize(blocks, [0u64; 128]);
    for l in 0..lanes {
        for j in 0..2 {
            mem[l * q + j] = to_block(&hash_long(1024, &[&h0, &le(j as u32), &le(l as u32)]));
        }
    }
    let zero = [0u64; 128];
    for pass in 0..p.passes as usize {
        for slice in 0..4 {
            for l in 0..lanes {
                let independent = p.kind == 1 || (p.kind == 2 && pass == 0 && slice < 2);
                let mut input = [0u64; 128];
                let mut addresses = [0u64; 128];
                if independent {
                    input[0] = pass as u64;
                    input[1] = l as u64;
                    input[2] = slice as u64;
                    input[3] = blocks as u64;
                    input[4] = p.passes as u64;
                    input[5] = p.kind as u64;
                }
                let start = if pass == 0 && slice == 0 { 2 } else { 0 };
                if independent && start == 2 {
                    input[6] += 1;
                    addresses = compress(&zero, &compress(&zero, &input));
                }
                for idx in start..seg {
                    let j = slice * seg + idx;
                    let cur = l * q + j;
                    let prev = if j == 0 { l * q + q - 1 } else { cur - 1 };
                    let rand = if independent {
                        if idx % 128 == 0 {
                            input[6] += 1;
                            addresses = compress(&zero, &compress(&zero, &input));
                        }
                        addresses[idx % 128]
                    } else {
                        mem[prev][0]
                    };
                    let (j1, j2) = (rand & 0xffff_ffff, rand >> 32);
                    let ref_lane = if pass == 0 && slice == 0 { l } else { (j2 as usize) % lanes };
                    let area = if pass == 0 {
                        if ref_lane == l { slice * seg + idx - 1 } else { slice * seg - (idx == 0) as usize }
                    } else if ref_lane == l {
                        q - seg + idx - 1
                    } else {
                        q - seg - (idx == 0) as usize
                    } as u64;
                    let x = (j1 * j1) >> 32;
                    let y = (area * x) >> 32;
                    let rel = area - 1 - y;
                    let begin = if pass == 0 || slice == 3 { 0 } else { (slice + 1) * seg };
                    let ref_index = (begin + rel as usize) % q;
                    let next = compress(&mem[prev], &mem[ref_lane * q + ref_index]);
                    if pass == 0 {
                        mem[cur] = next;
                    } else {
                        for (w, n) in mem[cur].iter_mut().zip(next.iter()) { *w ^= n; }
                    }
                }
            }
        }
    }
    let mut c = mem[q - 1];
    for l in 1..lanes {
        for (w, n) in c.iter_mut().zip(mem[l * q + q - 1].iter()) { *w ^= n; }
    }
    let bytes: Vec<u8> = c.iter().flat_map(|w| w.to_le_bytes()).collect();
    Some(hash_long(p.tag_len, &[&bytes]))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }

    #[test]
    fn rfc9106_vectors() {
        let (pw, salt, secret, ad) = ([1u8; 32], [2u8; 16], [3u8; 8], [4u8; 12]);
        let run = |kind| hex(&argon2(&Params { password: &pw, salt: &salt, secret: &secret, data: &ad, lanes: 4, memory: 32, passes: 3, kind, tag_len: 32 }).unwrap());
        assert_eq!(run(0), "512b391b6f1162975371d30919734294f868e3be3984f3c1a13a4db9fabe4acb");
        assert_eq!(run(1), "c814d9d1dc7f37aa13f0d77f2494bda1c8de6b016dd388d29952a4c4672b6ce8");
        assert_eq!(run(2), "0d640df58d78766c08c037a34a8b53c9d01ef0452d75b65eb52520e96b01e659");
    }
}
