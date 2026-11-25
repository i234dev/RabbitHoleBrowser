//! AES-OCB (RFC 7253) with 64-, 96- or 128-bit tags and nonces of at most
//! 120 bits.
use aes::cipher::{consts::U16, BlockDecrypt, BlockEncrypt, KeyInit};
use aes::Block;
use subtle::ConstantTimeEq;

type B = [u8; 16];

fn double(s: &B) -> B {
    let mut out = [0u8; 16];
    for i in 0..16 {
        out[i] = (s[i] << 1) | if i < 15 { s[i + 1] >> 7 } else { 0 };
    }
    if s[0] & 0x80 != 0 { out[15] ^= 0x87; }
    out
}

fn xor(a: &B, b: &B) -> B {
    let mut out = *a;
    for i in 0..16 { out[i] ^= b[i]; }
    out
}

struct Ocb<C> {
    aes: C,
    star: B,
    dollar: B,
    l: Vec<B>,
}

impl<C: BlockEncrypt<BlockSize = U16> + BlockDecrypt<BlockSize = U16>> Ocb<C> {
    fn new(aes: C) -> Self {
        let mut star = Block::default();
        aes.encrypt_block(&mut star);
        let star: B = star.into();
        let dollar = double(&star);
        let mut l = vec![double(&dollar)];
        for i in 1..64 { let next = double(&l[i - 1]); l.push(next); }
        Ocb { aes, star, dollar, l }
    }
    fn enc(&self, b: &B) -> B {
        let mut block = Block::from(*b);
        self.aes.encrypt_block(&mut block);
        block.into()
    }
    fn dec(&self, b: &B) -> B {
        let mut block = Block::from(*b);
        self.aes.decrypt_block(&mut block);
        block.into()
    }
    fn hash(&self, aad: &[u8]) -> B {
        let (mut sum, mut offset) = ([0u8; 16], [0u8; 16]);
        let mut chunks = aad.chunks_exact(16);
        for (i, chunk) in (&mut chunks).enumerate() {
            offset = xor(&offset, &self.l[(i + 1).trailing_zeros() as usize]);
            sum = xor(&sum, &self.enc(&xor(chunk.try_into().unwrap(), &offset)));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            offset = xor(&offset, &self.star);
            let mut block = [0u8; 16];
            block[..rest.len()].copy_from_slice(rest);
            block[rest.len()] = 0x80;
            sum = xor(&sum, &self.enc(&xor(&block, &offset)));
        }
        sum
    }
    fn crypt(&self, nonce: &[u8], aad: &[u8], data: &[u8], tag_len: usize, decrypt: bool) -> Option<Vec<u8>> {
        let mut block = [0u8; 16];
        block[0] = (((tag_len * 8) % 128) as u8) << 1;
        block[15 - nonce.len()] |= 1;
        block[16 - nonce.len()..].copy_from_slice(nonce);
        let bottom = (block[15] & 0x3f) as usize;
        block[15] &= 0xc0;
        let ktop = self.enc(&block);
        let mut stretch = [0u8; 24];
        stretch[..16].copy_from_slice(&ktop);
        for i in 0..8 { stretch[16 + i] = ktop[i] ^ ktop[i + 1]; }
        let mut offset = [0u8; 16];
        let (bytes, bits) = (bottom / 8, bottom % 8);
        for i in 0..16 {
            offset[i] = if bits == 0 { stretch[i + bytes] } else { (stretch[i + bytes] << bits) | (stretch[i + bytes + 1] >> (8 - bits)) };
        }
        let (input, tag) = if decrypt {
            let split = data.len().checked_sub(tag_len)?;
            (&data[..split], Some(&data[split..]))
        } else {
            (data, None)
        };
        let mut checksum = [0u8; 16];
        let mut out = Vec::with_capacity(input.len() + tag_len);
        let mut chunks = input.chunks_exact(16);
        for (i, chunk) in (&mut chunks).enumerate() {
            offset = xor(&offset, &self.l[(i + 1).trailing_zeros() as usize]);
            let chunk: &B = chunk.try_into().unwrap();
            let result = if decrypt {
                let p = xor(&offset, &self.dec(&xor(chunk, &offset)));
                checksum = xor(&checksum, &p);
                p
            } else {
                checksum = xor(&checksum, chunk);
                xor(&offset, &self.enc(&xor(chunk, &offset)))
            };
            out.extend_from_slice(&result);
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            offset = xor(&offset, &self.star);
            let pad = self.enc(&offset);
            let mut last = [0u8; 16];
            for (i, &b) in rest.iter().enumerate() {
                let r = b ^ pad[i];
                out.push(r);
                last[i] = if decrypt { r } else { b };
            }
            last[rest.len()] = 0x80;
            checksum = xor(&checksum, &last);
        }
        let full = xor(&self.enc(&xor(&xor(&checksum, &offset), &self.dollar)), &self.hash(aad));
        match tag {
            Some(tag) => bool::from(full[..tag_len].ct_eq(tag)).then_some(out),
            None => {
                out.extend_from_slice(&full[..tag_len]);
                Some(out)
            }
        }
    }
}

pub(super) fn crypt(key: &[u8], nonce: &[u8], aad: &[u8], data: &[u8], tag_len: usize, decrypt: bool) -> Option<Vec<u8>> {
    if nonce.len() > 15 || !matches!(tag_len, 8 | 12 | 16) { return None; }
    match key.len() {
        16 => Ocb::new(aes::Aes128::new_from_slice(key).ok()?).crypt(nonce, aad, data, tag_len, decrypt),
        24 => Ocb::new(aes::Aes192::new_from_slice(key).ok()?).crypt(nonce, aad, data, tag_len, decrypt),
        32 => Ocb::new(aes::Aes256::new_from_slice(key).ok()?).crypt(nonce, aad, data, tag_len, decrypt),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rfc7253_sample() {
        // RFC 7253 Appendix A, first sample: empty A and P.
        let key: Vec<u8> = (0..16).collect();
        let nonce = [0xbb, 0xaa, 0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0x00];
        let out = super::crypt(&key, &nonce, &[], &[], 16, false).unwrap();
        let hex: String = out.iter().map(|b| format!("{b:02X}")).collect();
        assert_eq!(hex, "785407BFFFC8AD9EDCC5520AC9111EE6");
        let back = super::crypt(&key, &nonce, &[], &out, 16, true).unwrap();
        assert!(back.is_empty());
        // Third sample: A = P = 0001020304050607 with nonce ...01.
        let nonce = [0xbb, 0xaa, 0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0x01];
        let p: Vec<u8> = (0..8).collect();
        let out = super::crypt(&key, &nonce, &p, &p, 16, false).unwrap();
        let hex: String = out.iter().map(|b| format!("{b:02X}")).collect();
        assert_eq!(hex, "6820B3657B6F615A5725BDA0D3B4EB3A257C9AF1F8F03009");
    }
}
