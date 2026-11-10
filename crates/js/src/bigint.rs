//! Arbitrary-precision integers for JavaScript `BigInt`.
//!
//! Sign-magnitude representation with little-endian 32-bit limbs. The
//! magnitude never has trailing (most significant) zero limbs and zero is
//! never negative, so derived equality and hashing are structural.
//! Bitwise operations and shifts follow JavaScript's infinite two's
//! complement semantics.

use std::cmp::Ordering;
use std::fmt;

#[derive(Clone, PartialEq, Eq, Hash, Default)]
pub struct BigInt {
    neg: bool,
    mag: Vec<u32>,
}

impl fmt::Debug for BigInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}n", self.to_string_radix(10))
    }
}

impl fmt::Display for BigInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string_radix(10))
    }
}

// ------------------------------------------------------------ magnitudes

fn trim(v: &mut Vec<u32>) {
    while v.last() == Some(&0) {
        v.pop();
    }
}

fn cmp_mag(a: &[u32], b: &[u32]) -> Ordering {
    if a.len() != b.len() {
        return a.len().cmp(&b.len());
    }
    for i in (0..a.len()).rev() {
        if a[i] != b[i] {
            return a[i].cmp(&b[i]);
        }
    }
    Ordering::Equal
}

fn add_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
    let (long, short) = if a.len() >= b.len() { (a, b) } else { (b, a) };
    let mut out = Vec::with_capacity(long.len() + 1);
    let mut carry = 0u64;
    for i in 0..long.len() {
        let s = long[i] as u64 + *short.get(i).unwrap_or(&0) as u64 + carry;
        out.push(s as u32);
        carry = s >> 32;
    }
    if carry != 0 {
        out.push(carry as u32);
    }
    out
}

/// a - b with |a| >= |b|.
fn sub_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(a.len());
    let mut borrow = 0i64;
    for i in 0..a.len() {
        let mut d = a[i] as i64 - *b.get(i).unwrap_or(&0) as i64 - borrow;
        if d < 0 {
            d += 1 << 32;
            borrow = 1;
        } else {
            borrow = 0;
        }
        out.push(d as u32);
    }
    trim(&mut out);
    out
}

fn mul_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    let mut out = vec![0u32; a.len() + b.len()];
    for (i, &x) in a.iter().enumerate() {
        let mut carry = 0u64;
        for (j, &y) in b.iter().enumerate() {
            let t = out[i + j] as u64 + x as u64 * y as u64 + carry;
            out[i + j] = t as u32;
            carry = t >> 32;
        }
        let mut k = i + b.len();
        while carry != 0 {
            let t = out[k] as u64 + carry;
            out[k] = t as u32;
            carry = t >> 32;
            k += 1;
        }
    }
    trim(&mut out);
    out
}

/// Divide by a single limb; returns (quotient, remainder).
fn divrem_small(a: &[u32], d: u32) -> (Vec<u32>, u32) {
    let mut q = vec![0u32; a.len()];
    let mut r = 0u64;
    for i in (0..a.len()).rev() {
        let cur = (r << 32) | a[i] as u64;
        q[i] = (cur / d as u64) as u32;
        r = cur % d as u64;
    }
    trim(&mut q);
    (q, r as u32)
}

fn shl_mag(a: &[u32], bits: usize) -> Vec<u32> {
    if a.is_empty() {
        return Vec::new();
    }
    let limbs = bits / 32;
    let sh = bits % 32;
    let mut out = vec![0u32; limbs];
    if sh == 0 {
        out.extend_from_slice(a);
    } else {
        let mut carry = 0u32;
        for &x in a {
            out.push((x << sh) | carry);
            carry = x >> (32 - sh);
        }
        if carry != 0 {
            out.push(carry);
        }
    }
    trim(&mut out);
    out
}

fn shr_mag(a: &[u32], bits: usize) -> Vec<u32> {
    let limbs = bits / 32;
    if limbs >= a.len() {
        return Vec::new();
    }
    let sh = bits % 32;
    let src = &a[limbs..];
    let mut out = Vec::with_capacity(src.len());
    if sh == 0 {
        out.extend_from_slice(src);
    } else {
        for i in 0..src.len() {
            let hi = if i + 1 < src.len() { src[i + 1] << (32 - sh) } else { 0 };
            out.push((src[i] >> sh) | hi);
        }
    }
    trim(&mut out);
    out
}

/// Long division (Knuth algorithm D, base 2^32): (quotient, remainder).
fn divrem_mag(a: &[u32], b: &[u32]) -> (Vec<u32>, Vec<u32>) {
    if cmp_mag(a, b) == Ordering::Less {
        return (Vec::new(), a.to_vec());
    }
    if b.len() == 1 {
        let (q, r) = divrem_small(a, b[0]);
        return (q, if r == 0 { Vec::new() } else { vec![r] });
    }
    // Normalize so the top limb of the divisor has its high bit set.
    let s = b[b.len() - 1].leading_zeros() as usize;
    let v = shl_mag(b, s);
    let mut u = shl_mag(a, s);
    if u.len() == a.len() {
        u.push(0);
    }
    if u.len() < a.len() + 1 {
        u.resize(a.len() + 1, 0);
    }
    let n = v.len();
    let m = u.len() - n - 1;
    let mut q = vec![0u32; m + 1];
    let base: u64 = 1 << 32;
    for j in (0..=m).rev() {
        let num = ((u[j + n] as u64) << 32) | u[j + n - 1] as u64;
        let mut qhat = num / v[n - 1] as u64;
        let mut rhat = num % v[n - 1] as u64;
        while qhat >= base || qhat * v[n - 2] as u64 > ((rhat << 32) | u[j + n - 2] as u64) {
            qhat -= 1;
            rhat += v[n - 1] as u64;
            if rhat >= base {
                break;
            }
        }
        // Multiply and subtract.
        let mut borrow: i64 = 0;
        let mut carry: u64 = 0;
        for i in 0..n {
            let p = qhat * v[i] as u64 + carry;
            carry = p >> 32;
            let t = u[i + j] as i64 - borrow - (p & 0xFFFF_FFFF) as i64;
            u[i + j] = t as u32;
            borrow = if t < 0 { 1 } else { 0 };
        }
        let t = u[j + n] as i64 - borrow - carry as i64;
        u[j + n] = t as u32;
        if t < 0 {
            // Add back.
            qhat -= 1;
            let mut c: u64 = 0;
            for i in 0..n {
                let s2 = u[i + j] as u64 + v[i] as u64 + c;
                u[i + j] = s2 as u32;
                c = s2 >> 32;
            }
            u[j + n] = u[j + n].wrapping_add(c as u32);
        }
        q[j] = qhat as u32;
    }
    trim(&mut q);
    u.truncate(n);
    let r = shr_mag(&u, s);
    (q, r)
}

// --------------------------------------------------------------- BigInt

impl BigInt {
    pub fn zero() -> BigInt {
        BigInt::default()
    }

    fn from_parts(neg: bool, mut mag: Vec<u32>) -> BigInt {
        trim(&mut mag);
        let neg = neg && !mag.is_empty();
        BigInt { neg, mag }
    }

    pub fn from_i128(v: i128) -> BigInt {
        let neg = v < 0;
        let mut m = v.unsigned_abs();
        let mut mag = Vec::new();
        while m != 0 {
            mag.push(m as u32);
            m >>= 32;
        }
        BigInt::from_parts(neg, mag)
    }

    pub fn from_u64(v: u64) -> BigInt {
        BigInt::from_i128(v as i128)
    }

    pub fn from_i64(v: i64) -> BigInt {
        BigInt::from_i128(v as i128)
    }

    /// Exact conversion of an integral finite f64 (None otherwise).
    pub fn from_f64(f: f64) -> Option<BigInt> {
        if !f.is_finite() || f.fract() != 0.0 {
            return None;
        }
        let neg = f < 0.0;
        let a = f.abs();
        if a < 1.8e19 {
            return Some(BigInt::from_parts(neg, {
                let mut m = a as u64;
                let mut v = Vec::new();
                while m != 0 {
                    v.push(m as u32);
                    m >>= 32;
                }
                v
            }));
        }
        let bits = a.to_bits();
        let exp = ((bits >> 52) & 0x7FF) as i64 - 1075;
        let mant = (bits & ((1u64 << 52) - 1)) | (1u64 << 52);
        let m = BigInt::from_u64(mant);
        let r = if exp >= 0 { m.shl(exp) } else { m.shr(-exp) };
        Some(if neg { r.neg() } else { r })
    }

    pub fn is_zero(&self) -> bool {
        self.mag.is_empty()
    }

    pub fn is_negative(&self) -> bool {
        self.neg
    }

    /// Exact i128 value if it fits.
    pub fn to_i128(&self) -> Option<i128> {
        if self.mag.len() > 4 {
            return None;
        }
        let mut m: u128 = 0;
        for (i, &l) in self.mag.iter().enumerate() {
            m |= (l as u128) << (32 * i);
        }
        if self.neg {
            if m <= i128::MAX as u128 + 1 {
                Some((m as i128).wrapping_neg())
            } else {
                None
            }
        } else if m <= i128::MAX as u128 {
            Some(m as i128)
        } else {
            None
        }
    }

    /// Low 128 bits in two's complement (BigInt.asIntN(128, x)).
    pub fn to_i128_wrapping(&self) -> i128 {
        let mut m: u128 = 0;
        for (i, &l) in self.mag.iter().take(4).enumerate() {
            m |= (l as u128) << (32 * i);
        }
        let v = m as i128;
        if self.neg {
            v.wrapping_neg()
        } else {
            v
        }
    }

    /// Nearest f64 (round half to even on the top 64 bits; good enough for
    /// Number(big) and comparisons).
    pub fn to_f64(&self) -> f64 {
        if self.mag.is_empty() {
            return 0.0;
        }
        let bits = self.bit_len();
        let v = if bits <= 64 {
            let mut m: u64 = 0;
            for (i, &l) in self.mag.iter().enumerate() {
                m |= (l as u64) << (32 * i);
            }
            m as f64
        } else {
            // Keep 64 significant bits plus a sticky bit for rounding.
            let shift = bits - 64;
            let top = shr_mag(&self.mag, shift as usize);
            let mut m: u64 = 0;
            for (i, &l) in top.iter().enumerate().take(2) {
                m |= (l as u64) << (32 * i);
            }
            let sticky = shl_mag(&top, shift as usize) != self.mag;
            let f = (m | sticky as u64) as f64;
            if shift > 1100 {
                f64::INFINITY
            } else {
                f * 2f64.powi(shift as i32)
            }
        };
        if self.neg {
            -v
        } else {
            v
        }
    }

    fn bit_len(&self) -> u64 {
        match self.mag.last() {
            None => 0,
            Some(&top) => (self.mag.len() as u64 - 1) * 32 + (32 - top.leading_zeros() as u64),
        }
    }

    pub fn neg(&self) -> BigInt {
        BigInt::from_parts(!self.neg, self.mag.clone())
    }

    pub fn add(&self, o: &BigInt) -> BigInt {
        if self.neg == o.neg {
            return BigInt::from_parts(self.neg, add_mag(&self.mag, &o.mag));
        }
        match cmp_mag(&self.mag, &o.mag) {
            Ordering::Equal => BigInt::zero(),
            Ordering::Greater => BigInt::from_parts(self.neg, sub_mag(&self.mag, &o.mag)),
            Ordering::Less => BigInt::from_parts(o.neg, sub_mag(&o.mag, &self.mag)),
        }
    }

    pub fn sub(&self, o: &BigInt) -> BigInt {
        self.add(&o.neg())
    }

    pub fn mul(&self, o: &BigInt) -> BigInt {
        BigInt::from_parts(self.neg != o.neg, mul_mag(&self.mag, &o.mag))
    }

    /// Truncating division (None on division by zero).
    pub fn div(&self, o: &BigInt) -> Option<BigInt> {
        if o.is_zero() {
            return None;
        }
        let (q, _) = divrem_mag(&self.mag, &o.mag);
        Some(BigInt::from_parts(self.neg != o.neg, q))
    }

    /// Remainder with the sign of the dividend (None on division by zero).
    pub fn rem(&self, o: &BigInt) -> Option<BigInt> {
        if o.is_zero() {
            return None;
        }
        let (_, r) = divrem_mag(&self.mag, &o.mag);
        Some(BigInt::from_parts(self.neg, r))
    }

    /// `self ** exp` (exp must be non-negative; None when the result would be
    /// absurdly large).
    pub fn pow(&self, exp: &BigInt) -> Option<BigInt> {
        if exp.neg {
            return None;
        }
        let e = exp.to_i128()?;
        if self.mag.is_empty() {
            return Some(if e == 0 { BigInt::from_i128(1) } else { BigInt::zero() });
        }
        if self.mag == [1] {
            return Some(if self.neg && e % 2 == 1 { self.clone() } else { BigInt::from_i128(1) });
        }
        if (self.bit_len() as i128).saturating_mul(e) > 1 << 30 {
            return None;
        }
        let mut result = BigInt::from_i128(1);
        let mut base = self.clone();
        let mut e = e as u64;
        while e > 0 {
            if e & 1 == 1 {
                result = result.mul(&base);
            }
            e >>= 1;
            if e > 0 {
                base = base.mul(&base);
            }
        }
        Some(result)
    }

    /// Left shift by `n` bits (negative `n` shifts right).
    pub fn shl(&self, n: i64) -> BigInt {
        if n < 0 {
            return self.shr(-n);
        }
        BigInt::from_parts(self.neg, shl_mag(&self.mag, n as usize))
    }

    /// Arithmetic (flooring) right shift by `n` bits.
    pub fn shr(&self, n: i64) -> BigInt {
        if n < 0 {
            return self.shl(-n);
        }
        if !self.neg {
            return BigInt::from_parts(false, shr_mag(&self.mag, n as usize));
        }
        // floor(-m / 2^n) = -ceil(m / 2^n)
        let q = shr_mag(&self.mag, n as usize);
        let exact = shl_mag(&q, n as usize) == self.mag;
        let q = if exact { q } else { add_mag(&q, &[1]) };
        BigInt::from_parts(true, q)
    }

    /// Two's complement limbs of length `len` (sign-extended).
    fn twos(&self, len: usize) -> Vec<u32> {
        let mut v = self.mag.clone();
        v.resize(len, 0);
        if self.neg {
            for x in v.iter_mut() {
                *x = !*x;
            }
            let mut carry = 1u64;
            for x in v.iter_mut() {
                let s = *x as u64 + carry;
                *x = s as u32;
                carry = s >> 32;
                if carry == 0 {
                    break;
                }
            }
        }
        v
    }

    fn from_twos(mut v: Vec<u32>) -> BigInt {
        let neg = v.last().map(|&t| t & 0x8000_0000 != 0).unwrap_or(false);
        if neg {
            for x in v.iter_mut() {
                *x = !*x;
            }
            let mut carry = 1u64;
            for x in v.iter_mut() {
                let s = *x as u64 + carry;
                *x = s as u32;
                carry = s >> 32;
                if carry == 0 {
                    break;
                }
            }
        }
        BigInt::from_parts(neg, v)
    }

    fn bitwise(&self, o: &BigInt, f: impl Fn(u32, u32) -> u32) -> BigInt {
        let len = self.mag.len().max(o.mag.len()) + 1;
        let a = self.twos(len);
        let b = o.twos(len);
        BigInt::from_twos(a.iter().zip(b.iter()).map(|(x, y)| f(*x, *y)).collect())
    }

    pub fn bitand(&self, o: &BigInt) -> BigInt {
        self.bitwise(o, |x, y| x & y)
    }

    pub fn bitor(&self, o: &BigInt) -> BigInt {
        self.bitwise(o, |x, y| x | y)
    }

    pub fn bitxor(&self, o: &BigInt) -> BigInt {
        self.bitwise(o, |x, y| x ^ y)
    }

    /// `~x` = `-x - 1`.
    pub fn not(&self) -> BigInt {
        self.neg().sub(&BigInt::from_i128(1))
    }

    /// `BigInt.asUintN(bits, x)`.
    pub fn as_uint_n(&self, bits: u64) -> BigInt {
        if bits == 0 {
            return BigInt::zero();
        }
        let limbs = bits.div_ceil(32) as usize;
        let mut v = self.twos(limbs.max(self.mag.len() + 1));
        v.truncate(limbs);
        let extra = (limbs as u64 * 32 - bits) as u32;
        if extra > 0 {
            if let Some(top) = v.last_mut() {
                *top &= u32::MAX >> extra;
            }
        }
        BigInt::from_parts(false, v)
    }

    /// `BigInt.asIntN(bits, x)`.
    pub fn as_int_n(&self, bits: u64) -> BigInt {
        if bits == 0 {
            return BigInt::zero();
        }
        let u = self.as_uint_n(bits);
        // Negative when bit (bits-1) is set.
        let top_bit = bits - 1;
        let limb = (top_bit / 32) as usize;
        let set = u.mag.get(limb).map(|l| l & (1 << (top_bit % 32)) != 0).unwrap_or(false);
        if set {
            u.sub(&BigInt::from_i128(1).shl(bits as i64))
        } else {
            u
        }
    }

    /// Compare with an f64 (NaN → None).
    pub fn cmp_f64(&self, f: f64) -> Option<Ordering> {
        if f.is_nan() {
            return None;
        }
        if f == f64::INFINITY {
            return Some(Ordering::Less);
        }
        if f == f64::NEG_INFINITY {
            return Some(Ordering::Greater);
        }
        let fl = f.floor();
        let fb = BigInt::from_f64(fl).unwrap_or_default();
        match self.cmp(&fb) {
            Ordering::Equal => Some(if fl < f { Ordering::Less } else { Ordering::Equal }),
            o => Some(o),
        }
    }

    pub fn eq_f64(&self, f: f64) -> bool {
        self.cmp_f64(f) == Some(Ordering::Equal)
    }

    /// Parse a StringIntegerLiteral (`""` is 0n; `0x`/`0o`/`0b` prefixes; no
    /// sign with prefixes; surrounding whitespace allowed).
    pub fn parse(s: &str) -> Option<BigInt> {
        let s = s.trim();
        let (radix, digits, neg) = if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            (16, h, false)
        } else if let Some(o) = s.strip_prefix("0o").or_else(|| s.strip_prefix("0O")) {
            (8, o, false)
        } else if let Some(b) = s.strip_prefix("0b").or_else(|| s.strip_prefix("0B")) {
            (2, b, false)
        } else if let Some(r) = s.strip_prefix('-') {
            (10, r, true)
        } else {
            (10, s.strip_prefix('+').unwrap_or(s), false)
        };
        if digits.is_empty() {
            return if radix == 10 && !neg && s.is_empty() { Some(BigInt::zero()) } else if radix == 10 && s.is_empty() { Some(BigInt::zero()) } else { None };
        }
        BigInt::parse_radix(digits, radix).map(|b| if neg { b.neg() } else { b })
    }

    /// Parse unsigned digits in `radix` (2..=36), ignoring `_` separators.
    pub fn parse_radix(digits: &str, radix: u32) -> Option<BigInt> {
        let mut mag: Vec<u32> = Vec::new();
        let mut any = false;
        for c in digits.chars() {
            if c == '_' {
                continue;
            }
            let d = c.to_digit(radix)?;
            any = true;
            // mag = mag * radix + d
            let mut carry = d as u64;
            for x in mag.iter_mut() {
                let t = *x as u64 * radix as u64 + carry;
                *x = t as u32;
                carry = t >> 32;
            }
            if carry != 0 {
                mag.push(carry as u32);
            }
        }
        if !any {
            return None;
        }
        Some(BigInt::from_parts(false, mag))
    }

    pub fn to_string_radix(&self, radix: u32) -> String {
        if self.mag.is_empty() {
            return "0".into();
        }
        // Peel off chunks of digits per division for speed.
        let (chunk, chunk_digits) = {
            let mut c: u64 = radix as u64;
            let mut n = 1;
            while c * (radix as u64) < (1u64 << 32) {
                c *= radix as u64;
                n += 1;
            }
            (c as u32, n)
        };
        let mut out: Vec<char> = Vec::new();
        let mut cur = self.mag.clone();
        while !cur.is_empty() {
            let (q, mut r) = divrem_small(&cur, chunk);
            cur = q;
            for _ in 0..chunk_digits {
                out.push(std::char::from_digit(r % radix, radix).unwrap());
                r /= radix;
                if cur.is_empty() && r == 0 {
                    break;
                }
            }
        }
        while out.len() > 1 && out.last() == Some(&'0') {
            out.pop();
        }
        if self.neg {
            out.push('-');
        }
        out.iter().rev().collect()
    }
}

impl PartialOrd for BigInt {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BigInt {
    fn cmp(&self, o: &Self) -> Ordering {
        match (self.neg, o.neg) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => cmp_mag(&self.mag, &o.mag),
            (true, true) => cmp_mag(&o.mag, &self.mag),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(s: &str) -> BigInt {
        BigInt::parse(s).unwrap()
    }

    #[test]
    fn parse_print_round_trip() {
        let big = "123456789012345678901234567890123456789012345678901234567890";
        assert_eq!(b(big).to_string_radix(10), big);
        assert_eq!(b(&format!("-{big}")).to_string_radix(10), format!("-{big}"));
        let hex = "0x00e9d3dca1328a49ad3403e4badda37a6a13610b608b5099839e1074e720f5a33b2ebd8c2ffd12c09be0015a4635aa9d2022d8f72f90ed11610c3742b0baef5b7da73d7e79aff6cdb";
        assert_eq!(format!("0x00{}", b(hex).to_string_radix(16)), hex);
        assert_eq!(b("0").to_string_radix(10), "0");
        assert_eq!(b("").to_string_radix(10), "0");
        assert!(BigInt::parse("12a").is_none());
        assert_eq!(b("255").to_string_radix(2), "11111111");
    }

    #[test]
    fn arithmetic() {
        let x = b("340282366920938463463374607431768211456"); // 2^128
        let y = b("18446744073709551616"); // 2^64
        assert_eq!(x.div(&y).unwrap(), y);
        assert_eq!(y.mul(&y), x);
        assert_eq!(x.sub(&BigInt::from_i128(1)).to_string_radix(16), "f".repeat(32));
        assert_eq!(b("-7").div(&b("2")).unwrap(), b("-3"));
        assert_eq!(b("-7").rem(&b("2")).unwrap(), b("-1"));
        assert_eq!(b("2").pow(&b("100")).unwrap().to_string_radix(10), "1267650600228229401496703205376");
        let a = b("98765432109876543210987654321098765432109876543210");
        let d = b("1234567890123456789");
        let q = a.div(&d).unwrap();
        let r = a.rem(&d).unwrap();
        assert_eq!(q.mul(&d).add(&r), a);
        assert!(r < d);
    }

    #[test]
    fn bits_and_shifts() {
        assert_eq!(b("-1").shr(10), b("-1"));
        assert_eq!(b("-5").shr(1), b("-3"));
        assert_eq!(b("1").shl(100).shr(100), b("1"));
        assert_eq!(b("-6").bitand(&b("3")), b("2"));
        assert_eq!(b("-6").bitor(&b("3")), b("-5"));
        assert_eq!(b("-6").bitxor(&b("3")), b("-7"));
        assert_eq!(b("5").not(), b("-6"));
        assert_eq!(b("-1").as_uint_n(64).to_string_radix(16), "ffffffffffffffff");
        assert_eq!(b("255").as_int_n(8), b("-1"));
        assert_eq!(b("127").as_int_n(8), b("127"));
    }

    #[test]
    fn conversions() {
        assert_eq!(BigInt::from_f64(1e30).unwrap().to_string_radix(10), "1000000000000000019884624838656");
        assert_eq!(b("1000000000000000019884624838656").to_f64(), 1e30);
        assert_eq!(BigInt::from_i128(-42).to_i128(), Some(-42));
        assert!(b("1").cmp_f64(1.5) == Some(Ordering::Less));
        assert!(b("2").eq_f64(2.0));
        assert!(BigInt::from_f64(0.5).is_none());
    }
}
