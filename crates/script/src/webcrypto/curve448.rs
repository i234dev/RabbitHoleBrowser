//! X448 (RFC 7748) and Ed448 (RFC 8032) over GF(2^448 - 2^224 - 1).
//! Field elements use eight 56-bit limbs; scalars modulo the group order
//! use a small bit-serial reduction. Secret-dependent work runs without
//! secret-dependent branches.
use super::keccak;

const MASK: u64 = (1 << 56) - 1;

#[derive(Clone, Copy, Debug)]
struct Fe([u64; 8]);

const ZERO: Fe = Fe([0; 8]);
const ONE: Fe = Fe([1, 0, 0, 0, 0, 0, 0, 0]);

impl Fe {
    fn carry(mut t: [u128; 16]) -> Fe {
        // 2^448 = 2^224 + 1 (mod p): fold limbs 8..15 into 0..7.
        for k in (8..16).rev() {
            let v = t[k];
            t[k] = 0;
            t[k - 8] += v;
            t[k - 4] += v;
        }
        for _ in 0..2 {
            let mut c = 0u128;
            for limb in t.iter_mut().take(8) {
                *limb += c;
                c = *limb >> 56;
                *limb &= MASK as u128;
            }
            t[0] += c;
            t[4] += c;
        }
        let mut out = [0u64; 8];
        for i in 0..8 { out[i] = t[i] as u64; }
        Fe(out)
    }
    fn add(&self, b: &Fe) -> Fe {
        let mut t = [0u128; 16];
        for i in 0..8 { t[i] = (self.0[i] + b.0[i]) as u128; }
        Fe::carry(t)
    }
    fn sub(&self, b: &Fe) -> Fe {
        // a + 4p - b keeps every limb positive.
        let mut t = [0u128; 16];
        for i in 0..8 {
            let four_p = if i == 4 { 4 * (MASK - 1) } else { 4 * MASK };
            t[i] = (self.0[i] + four_p - b.0[i]) as u128;
        }
        Fe::carry(t)
    }
    fn neg(&self) -> Fe { ZERO.sub(self) }
    fn mul(&self, b: &Fe) -> Fe {
        let mut t = [0u128; 16];
        for i in 0..8 {
            for j in 0..8 { t[i + j] += self.0[i] as u128 * b.0[j] as u128; }
        }
        Fe::carry(t)
    }
    fn sqr(&self) -> Fe { self.mul(self) }
    fn mul_small(&self, k: u64) -> Fe {
        let mut t = [0u128; 16];
        for i in 0..8 { t[i] = self.0[i] as u128 * k as u128; }
        Fe::carry(t)
    }
    /// self^e for a public exponent given as little-endian 64-bit words.
    fn pow(&self, e: &[u64; 7]) -> Fe {
        let mut r = ONE;
        for i in (0..448).rev() {
            r = r.sqr();
            if (e[i / 64] >> (i % 64)) & 1 == 1 { r = r.mul(self); }
        }
        r
    }
    fn invert(&self) -> Fe {
        let mut e = P_WORDS;
        e[0] -= 2;
        self.pow(&e)
    }
    /// Fully reduced little-endian bytes.
    fn to_bytes(&self) -> [u8; 56] {
        let mut x = *self;
        for _ in 0..3 {
            let mut c = 0u64;
            for limb in x.0.iter_mut() {
                *limb += c;
                c = *limb >> 56;
                *limb &= MASK;
            }
            x.0[0] += c;
            x.0[4] += c;
        }
        // x < 2p now; subtract p once if x >= p.
        let p = [MASK, MASK, MASK, MASK, MASK - 1, MASK, MASK, MASK];
        let mut diff = [0u64; 8];
        let mut borrow = 0i128;
        for i in 0..8 {
            let v = x.0[i] as i128 - p[i] as i128 + borrow;
            borrow = v >> 56;
            diff[i] = (v as u64) & MASK;
        }
        let keep = (borrow < 0) as u64; // 1: x < p
        let m = keep.wrapping_neg();
        for i in 0..8 { x.0[i] = (x.0[i] & m) | (diff[i] & !m); }
        let mut out = [0u8; 56];
        for i in 0..8 { out[i * 7..i * 7 + 7].copy_from_slice(&x.0[i].to_le_bytes()[..7]); }
        out
    }
    fn from_bytes(b: &[u8]) -> Fe {
        let mut out = [0u64; 8];
        for i in 0..8 {
            let mut w = [0u8; 8];
            w[..7].copy_from_slice(&b[i * 7..i * 7 + 7]);
            out[i] = u64::from_le_bytes(w);
        }
        Fe(out)
    }
    fn is_zero(&self) -> bool { self.to_bytes().iter().all(|&b| b == 0) }
    fn eq(&self, b: &Fe) -> bool { self.to_bytes() == b.to_bytes() }
    fn is_odd(&self) -> bool { self.to_bytes()[0] & 1 == 1 }
    fn cswap(a: &mut Fe, b: &mut Fe, swap: u64) {
        let m = swap.wrapping_neg();
        for i in 0..8 {
            let t = m & (a.0[i] ^ b.0[i]);
            a.0[i] ^= t;
            b.0[i] ^= t;
        }
    }
    fn from_decimal(s: &str) -> Fe {
        s.bytes().fold(ZERO, |acc, c| acc.mul_small(10).add(&Fe([(c - b'0') as u64, 0, 0, 0, 0, 0, 0, 0])))
    }
}

/// p = 2^448 - 2^224 - 1 as little-endian 64-bit words.
const P_WORDS: [u64; 7] = [u64::MAX, u64::MAX, u64::MAX, 0xFFFF_FFFE_FFFF_FFFF, u64::MAX, u64::MAX, u64::MAX];

fn is_canonical(b: &[u8]) -> bool {
    // Little-endian comparison with p.
    let mut p = [0u8; 56];
    for (i, w) in P_WORDS.iter().enumerate() { p[i * 8..i * 8 + 8].copy_from_slice(&w.to_le_bytes()); }
    for i in (0..56).rev() {
        if b[i] != p[i] { return b[i] < p[i]; }
    }
    false
}

// ------------------------------------------------------------------- X448

pub(super) fn x448(scalar: &[u8], u: &[u8]) -> Option<[u8; 56]> {
    if scalar.len() != 56 || u.len() != 56 { return None; }
    let mut k = [0u8; 56];
    k.copy_from_slice(scalar);
    k[0] &= 252;
    k[55] |= 128;
    let x1 = Fe::from_bytes(u);
    let (mut x2, mut z2, mut x3, mut z3) = (ONE, ZERO, x1, ONE);
    let mut swap = 0u64;
    for t in (0..448).rev() {
        let bit = ((k[t / 8] >> (t % 8)) & 1) as u64;
        swap ^= bit;
        Fe::cswap(&mut x2, &mut x3, swap);
        Fe::cswap(&mut z2, &mut z3, swap);
        swap = bit;
        let a = x2.add(&z2);
        let aa = a.sqr();
        let b = x2.sub(&z2);
        let bb = b.sqr();
        let e = aa.sub(&bb);
        let c = x3.add(&z3);
        let d = x3.sub(&z3);
        let da = d.mul(&a);
        let cb = c.mul(&b);
        x3 = da.add(&cb).sqr();
        z3 = x1.mul(&da.sub(&cb).sqr());
        x2 = aa.mul(&bb);
        z2 = e.mul(&aa.add(&e.mul_small(39081)));
    }
    Fe::cswap(&mut x2, &mut x3, swap);
    Fe::cswap(&mut z2, &mut z3, swap);
    Some(x2.mul(&z2.invert()).to_bytes())
}

pub(super) fn x448_public(scalar: &[u8]) -> Option<[u8; 56]> {
    let mut base = [0u8; 56];
    base[0] = 5;
    x448(scalar, &base)
}

// ------------------------------------------------------------------ Ed448

#[derive(Clone, Copy)]
struct Point { x: Fe, y: Fe, z: Fe }

fn d() -> Fe { Fe([39081, 0, 0, 0, 0, 0, 0, 0]).neg() }

impl Point {
    const IDENTITY: Point = Point { x: ZERO, y: ONE, z: ONE };
    fn base() -> Point {
        Point {
            x: Fe::from_decimal("224580040295924300187604334099896036246789641632564134246125461686950415467406032909029192869357953282578032075146446173674602635247710"),
            y: Fe::from_decimal("298819210078481492676017930443930673437544040154080242095928241372331506189835876003536878655418784733982303233503462500531545062832660"),
            z: ONE,
        }
    }
    fn add(&self, q: &Point) -> Point {
        let a = self.z.mul(&q.z);
        let b = a.sqr();
        let c = self.x.mul(&q.x);
        let dd = self.y.mul(&q.y);
        let e = d().mul(&c).mul(&dd);
        let f = b.sub(&e);
        let g = b.add(&e);
        let h = self.x.add(&self.y).mul(&q.x.add(&q.y));
        Point {
            x: a.mul(&f).mul(&h.sub(&c).sub(&dd)),
            y: a.mul(&g).mul(&dd.sub(&c)),
            z: f.mul(&g),
        }
    }
    fn double(&self) -> Point {
        let b = self.x.add(&self.y).sqr();
        let c = self.x.sqr();
        let dd = self.y.sqr();
        let e = c.add(&dd);
        let h = self.z.sqr();
        let j = e.sub(&h.add(&h));
        Point { x: b.sub(&e).mul(&j), y: e.mul(&c.sub(&dd)), z: e.mul(&j) }
    }
    fn cswap(a: &mut Point, b: &mut Point, swap: u64) {
        Fe::cswap(&mut a.x, &mut b.x, swap);
        Fe::cswap(&mut a.y, &mut b.y, swap);
        Fe::cswap(&mut a.z, &mut b.z, swap);
    }
    /// [k]P for a little-endian scalar.
    fn mul(&self, k: &[u8]) -> Point {
        let (mut r0, mut r1) = (Point::IDENTITY, *self);
        for i in (0..k.len() * 8).rev() {
            let bit = ((k[i / 8] >> (i % 8)) & 1) as u64;
            Point::cswap(&mut r0, &mut r1, bit);
            r1 = r0.add(&r1);
            r0 = r0.double();
            Point::cswap(&mut r0, &mut r1, bit);
        }
        r0
    }
    fn encode(&self) -> [u8; 57] {
        let zi = self.z.invert();
        let (x, y) = (self.x.mul(&zi), self.y.mul(&zi));
        let mut out = [0u8; 57];
        out[..56].copy_from_slice(&y.to_bytes());
        out[56] = (x.is_odd() as u8) << 7;
        out
    }
    fn decode(b: &[u8]) -> Option<Point> {
        if b.len() != 57 || b[56] & 0x7f != 0 || !is_canonical(&b[..56]) { return None; }
        let sign = b[56] >> 7;
        let y = Fe::from_bytes(&b[..56]);
        let yy = y.sqr();
        let u = yy.sub(&ONE);
        let v = d().mul(&yy).sub(&ONE);
        // x = u^3 v (u^5 v^3)^((p-3)/4)
        let mut e = P_WORDS;
        e[0] -= 3;
        for i in 0..7 { e[i] = (e[i] >> 2) | if i < 6 { e[i + 1] << 62 } else { 0 }; }
        let u3v = u.sqr().mul(&u).mul(&v);
        let u5v3 = u3v.mul(&u.sqr()).mul(&v.sqr());
        let mut x = u3v.mul(&u5v3.pow(&e));
        if !v.mul(&x.sqr()).eq(&u) { return None; }
        if x.is_zero() && sign == 1 { return None; }
        if x.is_odd() != (sign == 1) { x = x.neg(); }
        Some(Point { x, y, z: ONE })
    }
    fn eq(&self, q: &Point) -> bool {
        self.x.mul(&q.z).eq(&q.x.mul(&self.z)) && self.y.mul(&q.z).eq(&q.y.mul(&self.z))
    }
}

// Scalars modulo L = 2^446 - 13818066809895115352007386748515426880336692474882178609894547503885.
const L_BYTES: [u8; 57] = [
    0xf3, 0x44, 0x58, 0xab, 0x92, 0xc2, 0x78, 0x23, 0x55, 0x8f, 0xc5, 0x8d, 0x72, 0xc2, 0x6c, 0x21,
    0x90, 0x36, 0xd6, 0xae, 0x49, 0xdb, 0x4e, 0xc4, 0xe9, 0x23, 0xca, 0x7c, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x3f, 0x00,
];

/// A little-endian number reduced modulo L, as 57 bytes.
fn reduce(n: &[u8]) -> [u8; 57] {
    let l: Vec<u32> = L_BYTES.chunks(4).map(|c| { let mut w = [0u8; 4]; w[..c.len()].copy_from_slice(c); u32::from_le_bytes(w) }).collect();
    let mut r = vec![0u32; l.len()];
    for i in (0..n.len() * 8).rev() {
        // r = 2r + bit, then subtract L if r >= L (r < 2L < 2^448 fits).
        let bit = ((n[i / 8] >> (i % 8)) & 1) as u32;
        let mut c = bit;
        for w in r.iter_mut() {
            let v = (*w as u64) << 1 | c as u64;
            *w = v as u32;
            c = (v >> 32) as u32;
        }
        let mut diff = vec![0u32; r.len()];
        let mut borrow = 0i64;
        for j in 0..r.len() {
            let v = r[j] as i64 - l[j] as i64 + borrow;
            diff[j] = v as u32;
            borrow = v >> 32;
        }
        let m = ((borrow >= 0) as u32).wrapping_neg();
        for j in 0..r.len() { r[j] = (diff[j] & m) | (r[j] & !m); }
    }
    let mut out = [0u8; 57];
    for (j, w) in r.iter().enumerate() {
        for (k, b) in w.to_le_bytes().iter().enumerate() {
            if j * 4 + k < 57 { out[j * 4 + k] = *b; }
        }
    }
    out
}

fn mul_add(a: &[u8; 57], b: &[u8; 57], c: &[u8; 57]) -> [u8; 57] {
    // a * b + c, then reduce.
    let mut t = vec![0u32; 2 * 57 + 2];
    for i in 0..57 {
        let mut carry = 0u32;
        for j in 0..57 {
            let v = t[i + j] + a[i] as u32 * b[j] as u32 + carry;
            t[i + j] = v & 0xff;
            carry = v >> 8;
        }
        t[i + 57] += carry;
    }
    let mut carry = 0u32;
    for i in 0..t.len() {
        let v = t[i] + if i < 57 { c[i] as u32 } else { 0 } + carry;
        t[i] = v & 0xff;
        carry = v >> 8;
    }
    let bytes: Vec<u8> = t.iter().map(|&v| v as u8).collect();
    reduce(&bytes)
}

fn dom4(context: &[u8]) -> Vec<u8> {
    let mut out = b"SigEd448".to_vec();
    out.push(0);
    out.push(context.len() as u8);
    out.extend_from_slice(context);
    out
}

fn expand(secret: &[u8]) -> ([u8; 57], Vec<u8>) {
    let h = keccak::shake(256, secret, 114);
    let mut s = [0u8; 57];
    s.copy_from_slice(&h[..57]);
    s[0] &= 0xfc;
    s[55] |= 0x80;
    s[56] = 0;
    (s, h[57..].to_vec())
}

pub(super) fn ed448_public(secret: &[u8]) -> Option<[u8; 57]> {
    if secret.len() != 57 { return None; }
    Some(Point::base().mul(&expand(secret).0).encode())
}

pub(super) fn ed448_sign(secret: &[u8], msg: &[u8], context: &[u8]) -> Option<Vec<u8>> {
    if secret.len() != 57 || context.len() > 255 { return None; }
    let (s, prefix) = expand(secret);
    let a = Point::base().mul(&s).encode();
    let dom = dom4(context);
    let r = reduce(&keccak::shake(256, &[&dom[..], &prefix, msg].concat(), 114));
    let big_r = Point::base().mul(&r).encode();
    let k = reduce(&keccak::shake(256, &[&dom[..], &big_r, &a, msg].concat(), 114));
    let sig_s = mul_add(&k, &s, &r);
    Some([&big_r[..], &sig_s].concat())
}

pub(super) fn ed448_verify(public: &[u8], msg: &[u8], sig: &[u8], context: &[u8]) -> bool {
    if sig.len() != 114 || context.len() > 255 { return false; }
    let (Some(a), Some(r)) = (Point::decode(public), Point::decode(&sig[..57])) else { return false };
    let s = &sig[57..];
    // S must be below L.
    if s[56] != 0 || reduce(s) != *<&[u8; 57]>::try_from(s).unwrap() { return false; }
    let k = reduce(&keccak::shake(256, &[&dom4(context)[..], &sig[..57], public, msg].concat(), 114));
    let lhs = Point::base().mul(s).double().double();
    let rhs = r.add(&a.mul(&k)).double().double();
    lhs.eq(&rhs)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn unhex(s: &str) -> Vec<u8> { (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect() }
    fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }

    #[test]
    fn rfc7748_x448() {
        let k = unhex("3d262fddf9ec8e88495266fea19a34d28882acef045104d0d1aae121700a779c984c24f8cdd78fbff44943eba368f54b29259a4f1c600ad3");
        let u = unhex("06fce640fa3487bfda5f6cf2d5263f8aad88334cbd07437f020f08f9814dc031ddbdc38c19c6da2583fa5429db94ada18aa7a7fb4ef8a086");
        assert_eq!(hex(&x448(&k, &u).unwrap()), "ce3e4ff95a60dc6697da1db1d85e6afbdf79b50a2412d7546d5f239fe14fbaadeb445fc66a01b0779d98223961111e21766282f73dd96b6f");
    }

    #[test]
    fn rfc8032_ed448_blank() {
        let sk = unhex("6c82a562cb808d10d632be89c8513ebf6c929f34ddfa8c9f63c9960ef6e348a3528c8a3fcc2f044e39a3fc5b94492f8f032e7549a20098f95b");
        let pk = ed448_public(&sk).unwrap();
        assert_eq!(hex(&pk), "5fd7449b59b461fd2ce787ec616ad46a1da1342485a70e1f8a0ea75d80e96778edf124769b46c7061bd6783df1e50f6cd1fa1abeafe8256180");
        let sig = ed448_sign(&sk, b"", b"").unwrap();
        assert_eq!(hex(&sig), "533a37f6bbe457251f023c0d88f976ae2dfb504a843e34d2074fd823d41a591f2b233f034f628281f2fd7a22ddd47d7828c59bd0a21bfd3980ff0d2028d4b18a9df63e006c5d1c2d345b925d8dc00b4104852db99ac5c7cdda8530a113a0f4dbb61149f05a7363268c71d95808ff2e652600");
        assert!(ed448_verify(&pk, b"", &sig, b""));
        assert!(!ed448_verify(&pk, b"x", &sig, b""));
    }
}
