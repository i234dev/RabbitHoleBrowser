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
}

fn js_round(x: f64) -> f64 {
    if x.is_nan() || x.is_infinite() {
        return x;
    }
    if x == 0.0 {
        return x;
    }
    if x > 0.0 && x < 0.5 {
        return 0.0;
    }
    if x < 0.0 && x >= -0.5 {
        return -0.0;
    }
    (x + 0.5).floor()
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
