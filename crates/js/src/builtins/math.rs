//! The Math object.

use super::*;
use crate::realm::SYM_TO_STRING_TAG;
use std::cell::Cell;

thread_local! {
    static RNG: Cell<u64> = Cell::new(0);
}

pub fn register(realm: &mut Realm) {
    let m = realm.new_object();
    realm.define_global("Math", Value::Object(m));
    realm.define_property(m, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("Math"), CONFIGURABLE);
    for (name, v) in [
        ("E", std::f64::consts::E),
        ("LN10", std::f64::consts::LN_10),
        ("LN2", std::f64::consts::LN_2),
        ("LOG10E", std::f64::consts::LOG10_E),
        ("LOG2E", std::f64::consts::LOG2_E),
        ("PI", std::f64::consts::PI),
        ("SQRT1_2", std::f64::consts::FRAC_1_SQRT_2),
        ("SQRT2", std::f64::consts::SQRT_2),
    ] {
        realm.define_property(m, name, Value::Number(v), 0);
    }
    macro_rules! unary {
        ($name:expr, $f:expr) => {{
            fn func(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
                let x = realm.to_number(arg(args, 0))?;
                let f: fn(f64) -> f64 = $f;
                Ok(Value::Number(f(x)))
            }
            realm.define_method(m, $name, 1, func);
        }};
    }
    unary!("abs", f64::abs);
    unary!("acos", f64::acos);
    unary!("acosh", f64::acosh);
    unary!("asin", f64::asin);
    unary!("asinh", f64::asinh);
    unary!("atan", f64::atan);
    unary!("atanh", f64::atanh);
    unary!("cbrt", f64::cbrt);
    unary!("ceil", f64::ceil);
    unary!("cos", f64::cos);
    unary!("cosh", f64::cosh);
    unary!("exp", f64::exp);
    unary!("expm1", f64::exp_m1);
    unary!("floor", f64::floor);
    unary!("fround", |x| x as f32 as f64);
    unary!("f16round", |x| super::typed_array::f16_bits_to_f64(super::typed_array::f64_to_f16_bits(x)));
    unary!("log", f64::ln);
    unary!("log1p", f64::ln_1p);
    unary!("log10", f64::log10);
    unary!("log2", f64::log2);
    unary!("sign", |x| if x.is_nan() { f64::NAN } else if x == 0.0 { x } else { x.signum() });
    unary!("sin", f64::sin);
    unary!("sinh", f64::sinh);
    unary!("sqrt", f64::sqrt);
    unary!("tan", f64::tan);
    unary!("tanh", f64::tanh);
    unary!("trunc", f64::trunc);
    unary!("clz32", |x| (to_uint32(x).leading_zeros()) as f64);
    unary!("round", js_round);
    realm.define_method(m, "atan2", 2, atan2);
    realm.define_method(m, "pow", 2, pow);
    realm.define_method(m, "imul", 2, imul);
    realm.define_method(m, "max", 2, max);
    realm.define_method(m, "min", 2, min);
    realm.define_method(m, "hypot", 2, hypot);
    realm.define_method(m, "random", 0, random);
    unary!("f16round", f16_round);
    realm.define_method(m, "sumPrecise", 1, sum_precise);
}

/// The nearest binary16 value (ties to even), as a double.
pub(crate) fn f16_round(x: f64) -> f64 {
    if x.is_nan() || x.is_infinite() || x == 0.0 {
        return x;
    }
    let a = x.abs();
    let r = if a >= 65520.0 {
        f64::INFINITY
    } else {
        let e = ((a.to_bits() >> 52) & 0x7ff) as i32 - 1023;
        // Spacing of binary16 values around `a` (subnormals below 2^-14).
        let q = if e < -14 { 2f64.powi(-24) } else { 2f64.powi(e - 10) };
        (a / q).round_ties_even() * q
    };
    if x < 0.0 { -r } else { r }
}

/// An exact sum of doubles: a two's-complement integer counting units of
/// 2^-1074 (wide enough for 2^53 maximal doubles).
struct ExactSum {
    limbs: [u64; 36],
}

impl ExactSum {
    fn add(&mut self, d: f64) {
        let bits = d.to_bits();
        let e = ((bits >> 52) & 0x7ff) as usize;
        let frac = bits & ((1u64 << 52) - 1);
        let (m, s) = if e == 0 { (frac, 0) } else { (frac | (1 << 52), e - 1) };
        let (i, sh) = (s / 64, s % 64);
        let lo = m << sh;
        let hi = if sh == 0 { 0 } else { m >> (64 - sh) };
        if d < 0.0 {
            let (r, b0) = self.limbs[i].overflowing_sub(lo);
            self.limbs[i] = r;
            let (r, b1) = self.limbs[i + 1].overflowing_sub(hi);
            let (r, b2) = r.overflowing_sub(b0 as u64);
            self.limbs[i + 1] = r;
            let mut borrow = b1 || b2;
            let mut j = i + 2;
            while borrow && j < self.limbs.len() {
                let (r, b) = self.limbs[j].overflowing_sub(1);
                self.limbs[j] = r;
                borrow = b;
                j += 1;
            }
        } else {
            let (r, c0) = self.limbs[i].overflowing_add(lo);
            self.limbs[i] = r;
            let (r, c1) = self.limbs[i + 1].overflowing_add(hi);
            let (r, c2) = r.overflowing_add(c0 as u64);
            self.limbs[i + 1] = r;
            let mut carry = c1 || c2;
            let mut j = i + 2;
            while carry && j < self.limbs.len() {
                let (r, c) = self.limbs[j].overflowing_add(1);
                self.limbs[j] = r;
                carry = c;
                j += 1;
            }
        }
    }

    /// The sum rounded to the nearest double (ties to even).
    fn to_f64(&self) -> f64 {
        let neg = self.limbs[self.limbs.len() - 1] >> 63 == 1;
        let mut mag = self.limbs;
        if neg {
            // Two's-complement negation.
            let mut carry = true;
            for l in mag.iter_mut() {
                let (r, c) = (!*l).overflowing_add(carry as u64);
                *l = r;
                carry = c;
            }
        }
        let Some(top) = mag.iter().rposition(|&l| l != 0) else { return 0.0 };
        let bitlen = top * 64 + (64 - mag[top].leading_zeros() as usize);
        let bit = |k: usize| (mag[k / 64] >> (k % 64)) & 1;
        let r = if bitlen <= 53 {
            (mag[0] as f64) * f64::from_bits(1)
        } else {
            let shift = bitlen - 53;
            let mut m: u64 = 0;
            for k in (shift..bitlen).rev() {
                m = (m << 1) | bit(k);
            }
            let half = bit(shift - 1) == 1;
            let sticky = (0..shift - 1).any(|k| bit(k) == 1);
            let mut exp = shift as i32 - 1074;
            if half && (sticky || m & 1 == 1) {
                m += 1;
                if m == 1 << 53 {
                    m >>= 1;
                    exp += 1;
                }
            }
            let mut x = m as f64;
            while exp > 1000 {
                x *= 2f64.powi(1000);
                exp -= 1000;
            }
            x * 2f64.powi(exp)
        };
        if neg { -r } else { r }
    }
}

/// Math.sumPrecise(items): the exactly rounded sum of an iterable of numbers.
fn sum_precise(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let items = arg(args, 0);
    if items.is_nullish() {
        return realm.type_error("Math.sumPrecise requires an iterable");
    }
    let (it, next) = realm.get_iterator(items)?;
    // States: minus-zero, finite, +inf, -inf, NaN.
    let (mut minus_zero, mut pos_inf, mut neg_inf, mut nan) = (true, false, false, false);
    let mut sum = ExactSum { limbs: [0; 36] };
    let mut count: u64 = 0;
    while let Some(v) = realm.iterator_step(&it, &next)? {
        count += 1;
        let n = match v.v() {
            crate::value::V::Number(n) if count < (1u64 << 53) => n,
            _ => {
                let _ = realm.iterator_close(&it);
                return realm.type_error("Math.sumPrecise: every item must be a number");
            }
        };
        if nan {
            continue;
        }
        if n.is_nan() {
            nan = true;
        } else if n == f64::INFINITY {
            pos_inf = true;
            nan = neg_inf;
        } else if n == f64::NEG_INFINITY {
            neg_inf = true;
            nan = pos_inf;
        } else if !(n == 0.0 && n.is_sign_negative()) {
            minus_zero = false;
            if n != 0.0 {
                sum.add(n);
            }
        }
    }
    Ok(Value::Number(if nan {
        f64::NAN
    } else if pos_inf {
        f64::INFINITY
    } else if neg_inf {
        f64::NEG_INFINITY
    } else if minus_zero {
        -0.0
    } else {
        sum.to_f64()
    }))
}

fn js_round(x: f64) -> f64 {
    if x.is_nan() || x.is_infinite() || x == 0.0 {
        return x;
    }
    if x < 0.0 && x >= -0.5 {
        return -0.0;
    }
    // floor(x) + (fraction >= 0.5), without the rounding error of x + 0.5.
    let f = x.floor();
    if x - f >= 0.5 { f + 1.0 } else { f }
}

fn atan2(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let y = realm.to_number(arg(args, 0))?;
    let x = realm.to_number(arg(args, 1))?;
    Ok(Value::Number(y.atan2(x)))
}

fn pow(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let x = realm.to_number(arg(args, 0))?;
    let y = realm.to_number(arg(args, 1))?;
    Ok(Value::Number(crate::vm::js_pow(x, y)))
}

fn imul(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let a = realm.to_int32(arg(args, 0))?;
    let b = realm.to_int32(arg(args, 1))?;
    Ok(Value::Number(a.wrapping_mul(b) as f64))
}

fn max(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let mut r = f64::NEG_INFINITY;
    for a in args {
        let n = realm.to_number(a.clone())?;
        if n.is_nan() {
            r = f64::NAN;
        } else if !r.is_nan() && (n > r || (n == 0.0 && r == 0.0 && !n.is_sign_negative())) {
            r = n;
        }
    }
    Ok(Value::Number(r))
}

fn min(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let mut r = f64::INFINITY;
    for a in args {
        let n = realm.to_number(a.clone())?;
        if n.is_nan() {
            r = f64::NAN;
        } else if !r.is_nan() && (n < r || (n == 0.0 && r == 0.0 && n.is_sign_negative())) {
            r = n;
        }
    }
    Ok(Value::Number(r))
}

fn hypot(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let mut sum = 0.0;
    let mut inf = false;
    let mut nan = false;
    for a in args {
        let n = realm.to_number(a.clone())?;
        if n.is_infinite() {
            inf = true;
        } else if n.is_nan() {
            nan = true;
        } else {
            sum += n * n;
        }
    }
    Ok(Value::Number(if inf {
        f64::INFINITY
    } else if nan {
        f64::NAN
    } else {
        sum.sqrt()
    }))
}

fn random(_realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    let v = RNG.with(|r| {
        let mut s = r.get();
        if s == 0 {
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0x9E3779B97F4A7C15);
            s = t ^ 0x9E3779B97F4A7C15 | 1;
        }
        // xorshift64*
        s ^= s >> 12;
        s ^= s << 25;
        s ^= s >> 27;
        r.set(s);
        let x = s.wrapping_mul(0x2545F4914F6CDD1D);
        (x >> 11) as f64 / (1u64 << 53) as f64
    });
    Ok(Value::Number(v))
}
