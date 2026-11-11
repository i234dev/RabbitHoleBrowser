//! Numeric literals of the text format.

/// Is `s` a digit sequence with optional single `_` separators?
fn valid_digits(s: &str, radix: u32) -> bool {
    !(s.is_empty() || s.starts_with('_') || s.ends_with('_') || s.contains("__")) && s.chars().all(|c| c == '_' || c.is_digit(radix))
}

/// Digits with optional single `_` separators between digits.
fn digits(s: &str, radix: u32) -> Option<u128> {
    if s.is_empty() || s.starts_with('_') || s.ends_with('_') || s.contains("__") {
        return None;
    }
    let mut v: u128 = 0;
    for c in s.chars() {
        if c == '_' {
            continue;
        }
        let d = c.to_digit(radix)?;
        v = v.checked_mul(radix as u128)?.checked_add(d as u128)?;
    }
    Some(v)
}

/// Unsigned integer literal (decimal or hex).
pub fn parse_uint(s: &str) -> Option<u128> {
    if let Some(h) = s.strip_prefix("0x") {
        digits(h, 16)
    } else {
        digits(s, 10)
    }
}

pub fn parse_u32(s: &str) -> Option<u32> {
    let v = parse_uint(s)?;
    u32::try_from(v).ok()
}

pub fn parse_u64(s: &str) -> Option<u64> {
    let v = parse_uint(s)?;
    u64::try_from(v).ok()
}

/// Integer literal of `bits` width: signed or unsigned range; returns the
/// two's complement bits.
pub fn parse_int(s: &str, bits: u32) -> Option<u64> {
    let (neg, body) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let v = parse_uint(body)?;
    let max_u: u128 = if bits == 64 { u64::MAX as u128 } else { (1u128 << bits) - 1 };
    let mask: u64 = if bits == 64 { u64::MAX } else { (1u64 << bits) - 1 };
    if neg {
        if v > (1u128 << (bits - 1)) {
            return None;
        }
        Some((v as u64).wrapping_neg() & mask)
    } else {
        if v > max_u {
            return None;
        }
        Some(v as u64)
    }
}

struct Fmt {
    p: u32,
    emin: i64,
    bias: i64,
    ebits: u32,
}

const F32: Fmt = Fmt { p: 24, emin: -126, bias: 127, ebits: 8 };
const F64: Fmt = Fmt { p: 53, emin: -1022, bias: 1023, ebits: 11 };

/// Round `mant * 2^exp` (plus a sticky remainder) to a float's bits
/// (without sign). None on overflow.
fn round(mant: u128, exp: i64, sticky: bool, f: &Fmt) -> Option<u64> {
    if mant == 0 {
        return Some(0);
    }
    let nbits = 128 - mant.leading_zeros() as i64;
    let e_top = exp + nbits - 1;
    let p = f.p as i64;
    let mut lsb = if e_top >= f.emin { e_top - (p - 1) } else { f.emin - (p - 1) };
    let shift = lsb - exp;
    let mut m: u128;
    if shift <= 0 {
        m = mant << (-shift) as u32;
    } else if shift >= 128 {
        // Far below half an ulp.
        m = 0;
    } else {
        m = mant >> shift as u32;
        let rem = mant & ((1u128 << shift as u32) - 1);
        let half = 1u128 << (shift as u32 - 1);
        if rem > half || (rem == half && (sticky || m & 1 == 1)) {
            m += 1;
        }
    }
    if m == 1u128 << p {
        m >>= 1;
        lsb += 1;
    }
    if m >= 1u128 << (p - 1) {
        let biased = lsb + (p - 1) + f.bias;
        if biased >= (1i64 << f.ebits) - 1 {
            return None;
        }
        Some(((biased as u64) << (p - 1)) | (m as u64 & ((1u64 << (p - 1)) - 1)))
    } else {
        Some(m as u64)
    }
}

fn parse_hex_float(s: &str, f: &Fmt) -> Option<u64> {
    let (mantissa, exp_part) = match s.find(['p', 'P']) {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    let (ip, fp) = match mantissa.find('.') {
        Some(i) => (&mantissa[..i], Some(&mantissa[i + 1..])),
        None => (mantissa, None),
    };
    if !valid_digits(ip, 16) {
        return None;
    }
    if let Some(fp) = fp {
        if !fp.is_empty() && !valid_digits(fp, 16) {
            return None;
        }
    }
    let mut mant: u128 = 0;
    let mut exp: i64 = 0;
    let mut sticky = false;
    for c in ip.chars().filter(|&c| c != '_') {
        let d = c.to_digit(16)? as u128;
        if mant >> 120 == 0 {
            mant = mant * 16 + d;
        } else {
            sticky |= d != 0;
            exp += 4;
        }
    }
    if let Some(fp) = fp {
        for c in fp.chars().filter(|&c| c != '_') {
            let d = c.to_digit(16)? as u128;
            if mant >> 120 == 0 {
                mant = mant * 16 + d;
                exp -= 4;
            } else {
                sticky |= d != 0;
            }
        }
    }
    if let Some(e) = exp_part {
        let (neg, body) = match e.as_bytes().first() {
            Some(b'-') => (true, &e[1..]),
            Some(b'+') => (false, &e[1..]),
            _ => (false, e),
        };
        let v = digits(body, 10)?;
        let v = v.min(1 << 40) as i64;
        exp += if neg { -v } else { v };
    }
    if mant == 0 {
        return Some(0);
    }
    round(mant, exp, sticky, f)
}

fn parse_dec_float(s: &str, is32: bool) -> Option<u64> {
    let (mantissa, exp_part) = match s.find(['e', 'E']) {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    let (ip, fp) = match mantissa.find('.') {
        Some(i) => (&mantissa[..i], Some(&mantissa[i + 1..])),
        None => (mantissa, None),
    };
    if !valid_digits(ip, 10) {
        return None;
    }
    let mut text = ip.replace('_', "");
    text.push('.');
    match fp {
        Some(fp) if !fp.is_empty() => {
            if !valid_digits(fp, 10) {
                return None;
            }
            text.push_str(&fp.replace('_', ""));
        }
        _ => text.push('0'),
    }
    if let Some(e) = exp_part {
        let (sign, body) = match e.as_bytes().first() {
            Some(b'-') => ("-", &e[1..]),
            Some(b'+') => ("", &e[1..]),
            _ => ("", e),
        };
        if !valid_digits(body, 10) {
            return None;
        }
        text.push('e');
        text.push_str(sign);
        text.push_str(&body.replace('_', ""));
    }
    if is32 {
        let v: f32 = text.parse().ok()?;
        if v.is_infinite() {
            return None;
        }
        Some(v.to_bits() as u64)
    } else {
        let v: f64 = text.parse().ok()?;
        if v.is_infinite() {
            return None;
        }
        Some(v.to_bits())
    }
}

fn parse_float(s: &str, f: &Fmt, is32: bool) -> Option<u64> {
    let (neg, body) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let sign_bit = if is32 { 1u64 << 31 } else { 1u64 << 63 };
    let exp_mask = ((1u64 << f.ebits) - 1) << (f.p - 1);
    let bits = if body == "inf" {
        exp_mask
    } else if body == "nan" {
        exp_mask | 1u64 << (f.p - 2)
    } else if let Some(pl) = body.strip_prefix("nan:0x") {
        let v = digits(pl, 16)?;
        if v == 0 || v >= 1u128 << (f.p - 1) {
            return None;
        }
        exp_mask | v as u64
    } else if let Some(h) = body.strip_prefix("0x") {
        parse_hex_float(h, f)?
    } else {
        if !body.starts_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
        parse_dec_float(body, is32)?
    };
    Some(if neg { bits | sign_bit } else { bits })
}

pub fn parse_f32(s: &str) -> Option<u32> {
    parse_float(s, &F32, true).map(|b| b as u32)
}

pub fn parse_f64(s: &str) -> Option<u64> {
    parse_float(s, &F64, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn floats() {
        assert_eq!(parse_f32("1.5"), Some(1.5f32.to_bits()));
        assert_eq!(parse_f32("0x1p-149"), Some(1));
        assert_eq!(parse_f32("0x1.fffffep127"), Some(f32::MAX.to_bits()));
        assert_eq!(parse_f32("0x1.ffffffp127"), None);
        assert_eq!(parse_f64("-0x1.8p1"), Some((-3.0f64).to_bits()));
        assert_eq!(parse_f32("nan:0x200000"), Some(0x7fa00000));
        assert_eq!(parse_f64("1e308"), Some(1e308f64.to_bits()));
        assert_eq!(parse_f32("1e39"), None);
        assert_eq!(parse_f32("0x1.00000100000000001p-50"), Some(0x26800001));
        assert_eq!(parse_int("-1", 32), Some(0xffff_ffff));
        assert_eq!(parse_int("4294967296", 32), None);
        assert_eq!(parse_int("0x1_0000", 32), Some(65536));
    }
}
