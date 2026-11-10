//! Number, Boolean and BigInt.

use super::*;
use crate::realm::SYM_TO_STRING_TAG;

pub fn register(realm: &mut Realm) {
    let np = realm.intrinsics.number_proto;
    realm.define_method(np, "toString", 1, to_string);
    realm.define_method(np, "toLocaleString", 0, to_locale_string);
    realm.define_method(np, "valueOf", 0, value_of);
    realm.define_method(np, "toFixed", 1, to_fixed);
    realm.define_method(np, "toExponential", 1, to_exponential);
    realm.define_method(np, "toPrecision", 1, to_precision);
    let ctor = define_ctor(realm, "Number", 1, number_ctor, np);
    for (name, v) in [
        ("EPSILON", f64::EPSILON),
        ("MAX_SAFE_INTEGER", 9007199254740991.0),
        ("MIN_SAFE_INTEGER", -9007199254740991.0),
        ("MAX_VALUE", f64::MAX),
        ("MIN_VALUE", 5e-324),
        ("NaN", f64::NAN),
        ("POSITIVE_INFINITY", f64::INFINITY),
        ("NEGATIVE_INFINITY", f64::NEG_INFINITY),
    ] {
        realm.define_property(ctor, name, Value::Number(v), 0);
    }
    realm.define_method(ctor, "isFinite", 1, is_finite);
    realm.define_method(ctor, "isInteger", 1, is_integer);
    realm.define_method(ctor, "isNaN", 1, is_nan);
    realm.define_method(ctor, "isSafeInteger", 1, is_safe_integer);
    realm.define_method(ctor, "parseFloat", 1, global::parse_float);
    realm.define_method(ctor, "parseInt", 2, global::parse_int);

    let bp = realm.intrinsics.boolean_proto;
    realm.define_method(bp, "toString", 0, bool_to_string);
    realm.define_method(bp, "valueOf", 0, bool_value_of);
    define_ctor(realm, "Boolean", 1, boolean_ctor, bp);

    let bip = realm.intrinsics.bigint_proto;
    realm.define_method(bip, "toString", 0, bigint_to_string);
    realm.define_method(bip, "valueOf", 0, bigint_value_of);
    realm.define_method(bip, "toLocaleString", 0, bigint_to_string);
    realm.define_property(bip, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("BigInt"), CONFIGURABLE);
    let bc = realm.new_native_function_full("BigInt", 1, bigint_fn, None, vec![], false);
    realm.define_property(bc, "prototype", Value::Object(bip), 0);
    realm.define_property(bip, "constructor", Value::Object(bc), HIDDEN);
    realm.define_global("BigInt", Value::Object(bc));
    realm.define_method(bc, "asIntN", 2, as_int_n);
    realm.define_method(bc, "asUintN", 2, as_uint_n);
}

fn this_number(realm: &mut Realm, this: &Value) -> JsResult<f64> {
    match this {
        Value::Number(n) => Ok(*n),
        Value::Object(id) => match &realm.obj(*id).kind {
            ObjectKind::Number(n) => Ok(*n),
            _ => realm.type_error("Number.prototype method called on incompatible receiver"),
        },
        _ => realm.type_error("Number.prototype method called on incompatible receiver"),
    }
}

fn number_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let n = if args.is_empty() {
        0.0
    } else {
        match realm.to_numeric(args[0].clone())? {
            Value::BigInt(b) => b.to_f64(),
            Value::Number(n) => n,
            _ => unreachable!(),
        }
    };
    if realm.is_construct_call() {
        let default = realm.intrinsics.number_proto;
        let proto = realm.proto_from_new_target(default);
        return Ok(Value::Object(realm.alloc(Object::new(Some(proto), ObjectKind::Number(n)))));
    }
    Ok(Value::Number(n))
}

fn to_string(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let n = this_number(realm, &this)?;
    let radix = match arg(args, 0) {
        Value::Undefined => 10,
        v => {
            let r = to_integer(realm.to_number(v)?);
            if !(2.0..=36.0).contains(&r) {
                return realm.range_error("toString() radix must be between 2 and 36");
            }
            r as u32
        }
    };
    Ok(Value::str(&number_to_string_radix(n, radix)))
}

fn to_locale_string(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let n = this_number(realm, &this)?;
    if !n.is_finite() {
        return Ok(Value::str(&number_to_string(n)));
    }
    // en-US style grouping with up to 3 fraction digits.
    let s = format!("{:.3}", n.abs());
    let (int, frac) = s.split_once('.').unwrap();
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    let frac = frac.trim_end_matches('0');
    let mut out = String::new();
    if n < 0.0 {
        out.push('-');
    }
    out.push_str(&grouped);
    if !frac.is_empty() {
        out.push('.');
        out.push_str(frac);
    }
    Ok(Value::str(&out))
}

fn value_of(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(Value::Number(this_number(realm, &this)?))
}

/// Exact decimal digits of |x| as (integer digits, fraction digits without trailing zeros).
fn exact_decimal(x: f64) -> (String, String) {
    let s = format!("{:.1100}", x.abs());
    let (i, f) = s.split_once('.').unwrap_or((&s, ""));
    (i.to_string(), f.trim_end_matches('0').to_string())
}

/// Round a digit string to `keep` fraction digits, half away from zero.
fn round_digits(int: &str, frac: &str, keep: usize) -> (String, String) {
    let mut digits: Vec<u8> = int.bytes().map(|b| b - b'0').collect();
    let int_len = digits.len();
    let frac_bytes: Vec<u8> = frac.bytes().map(|b| b - b'0').collect();
    for i in 0..keep {
        digits.push(*frac_bytes.get(i).unwrap_or(&0));
    }
    let round_up = frac_bytes.get(keep).map(|d| *d >= 5).unwrap_or(false);
    if round_up {
        let mut i = digits.len();
        loop {
            if i == 0 {
                digits.insert(0, 1);
                break;
            }
            i -= 1;
            if digits[i] == 9 {
                digits[i] = 0;
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    let new_int_len = digits.len() - keep;
    let _ = int_len;
    let int_s: String = digits[..new_int_len].iter().map(|d| (b'0' + d) as char).collect();
    let frac_s: String = digits[new_int_len..].iter().map(|d| (b'0' + d) as char).collect();
    (int_s, frac_s)
}

fn to_fixed(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let x = this_number(realm, &this)?;
    let f = to_integer(realm.to_number(arg(args, 0))?);
    if !(0.0..=100.0).contains(&f) {
        return realm.range_error("toFixed() digits argument must be between 0 and 100");
    }
    if !x.is_finite() || x.abs() >= 1e21 {
        return Ok(Value::str(&number_to_string(x)));
    }
    let (i, fr) = exact_decimal(x);
    let (i, fr) = round_digits(&i, &fr, f as usize);
    let mut out = String::new();
    if x < 0.0 && (i.bytes().any(|b| b != b'0') || fr.bytes().any(|b| b != b'0')) {
        out.push('-');
    }
    out.push_str(&i);
    if f > 0.0 {
        out.push('.');
        out.push_str(&fr);
    }
    Ok(Value::str(&out))
}

/// Significant digits (count) of |x| with half-away rounding: returns (digits, exponent) where value = 0.d1d2.. × 10^exp.
fn significant_digits(x: f64, count: usize) -> (String, i32) {
    let (i, fr) = exact_decimal(x);
    let all: String = format!("{}{}", i, fr);
    let first_nonzero = all.find(|c| c != '0').unwrap_or(0);
    let exp = i.len() as i32 - first_nonzero as i32;
    let digits: Vec<u8> = all[first_nonzero..].bytes().map(|b| b - b'0').collect();
    let mut kept: Vec<u8> = digits.iter().take(count).copied().collect();
    while kept.len() < count {
        kept.push(0);
    }
    let round_up = digits.get(count).map(|d| *d >= 5).unwrap_or(false);
    let mut exp = exp;
    if round_up {
        let mut k = kept.len();
        loop {
            if k == 0 {
                kept.insert(0, 1);
                kept.pop();
                exp += 1;
                break;
            }
            k -= 1;
            if kept[k] == 9 {
                kept[k] = 0;
            } else {
                kept[k] += 1;
                break;
            }
        }
    }
    (kept.iter().map(|d| (b'0' + d) as char).collect(), exp)
}

fn to_exponential(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let x = this_number(realm, &this)?;
    let fd = arg(args, 0);
    let f = to_integer(realm.to_number(fd.clone())?);
    if !x.is_finite() {
        return Ok(Value::str(&number_to_string(x)));
    }
    if !(0.0..=100.0).contains(&f) {
        return realm.range_error("toExponential() argument must be between 0 and 100");
    }
    let (digits, exp) = if x == 0.0 {
        ("0".repeat(f as usize + 1), 1)
    } else if fd.is_undefined() {
        // shortest representation
        let e = format!("{:e}", x.abs());
        let (mant, ex) = e.split_once('e').unwrap();
        let d: String = mant.chars().filter(|c| *c != '.').collect();
        (d, ex.parse::<i32>().unwrap() + 1)
    } else {
        significant_digits(x, f as usize + 1)
    };
    let e = exp - 1;
    let mut out = String::new();
    if x < 0.0 {
        out.push('-');
    }
    out.push_str(&digits[..1]);
    if digits.len() > 1 {
        out.push('.');
        out.push_str(&digits[1..]);
    }
    out.push('e');
    out.push(if e >= 0 { '+' } else { '-' });
    out.push_str(&e.abs().to_string());
    Ok(Value::str(&out))
}

fn to_precision(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let x = this_number(realm, &this)?;
    let p = arg(args, 0);
    if p.is_undefined() {
        return Ok(Value::str(&number_to_string(x)));
    }
    let p = to_integer(realm.to_number(p)?);
    if !x.is_finite() {
        return Ok(Value::str(&number_to_string(x)));
    }
    if !(1.0..=100.0).contains(&p) {
        return realm.range_error("toPrecision() argument must be between 1 and 100");
    }
    let p = p as usize;
    let (digits, e) = if x == 0.0 { ("0".repeat(p), 1) } else { significant_digits(x, p) };
    let e = e - 1;
    let mut out = String::new();
    if x < 0.0 {
        out.push('-');
    }
    if e < -6 || e >= p as i32 {
        out.push_str(&digits[..1]);
        if p > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if e >= 0 { '+' } else { '-' });
        out.push_str(&e.abs().to_string());
    } else if e == p as i32 - 1 {
        out.push_str(&digits);
    } else if e >= 0 {
        out.push_str(&digits[..(e + 1) as usize]);
        out.push('.');
        out.push_str(&digits[(e + 1) as usize..]);
    } else {
        out.push_str("0.");
        for _ in 0..(-(e + 1)) {
            out.push('0');
        }
        out.push_str(&digits);
    }
    Ok(Value::str(&out))
}

fn is_finite(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let _ = realm;
    Ok(Value::Bool(matches!(arg(args, 0), Value::Number(n) if n.is_finite())))
}
fn is_integer(_realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Number(n) if n.is_finite() && n.fract() == 0.0)))
}
fn is_nan(_realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Number(n) if n.is_nan())))
}
fn is_safe_integer(_realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Number(n) if n.is_finite() && n.fract() == 0.0 && n.abs() <= 9007199254740991.0)))
}

// ---------------------------------------------------------------- Boolean

fn boolean_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let b = arg(args, 0).truthy();
    if realm.is_construct_call() {
        let default = realm.intrinsics.boolean_proto;
        let proto = realm.proto_from_new_target(default);
        return Ok(Value::Object(realm.alloc(Object::new(Some(proto), ObjectKind::Boolean(b)))));
    }
    Ok(Value::Bool(b))
}

fn this_bool(realm: &mut Realm, this: &Value) -> JsResult<bool> {
    match this {
        Value::Bool(b) => Ok(*b),
        Value::Object(id) => match &realm.obj(*id).kind {
            ObjectKind::Boolean(b) => Ok(*b),
            _ => realm.type_error("Boolean.prototype method called on incompatible receiver"),
        },
        _ => realm.type_error("Boolean.prototype method called on incompatible receiver"),
    }
}

fn bool_to_string(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(Value::str(if this_bool(realm, &this)? { "true" } else { "false" }))
}
fn bool_value_of(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(this_bool(realm, &this)?))
}

// ----------------------------------------------------------------- BigInt

fn bigint_fn(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if realm.is_construct_call() {
        return realm.type_error("BigInt is not a constructor");
    }
    let v = realm.to_primitive(arg(args, 0), crate::interop::Hint::Number)?;
    match v {
        Value::Number(n) => {
            if n.fract() != 0.0 || !n.is_finite() {
                return realm.range_error("The number cannot be converted to a BigInt because it is not an integer");
            }
            Ok(realm.new_bigint_value(BigInt::from_f64(n).unwrap_or_default()))
        }
        Value::BigInt(_) => Ok(v),
        Value::Bool(b) => Ok(realm.new_bigint(b as i128)),
        Value::String(s) => match BigInt::parse(&s.to_rust_string()) {
            Some(b) => Ok(Value::BigInt(std::rc::Rc::new(b))),
            None => realm.new_error("SyntaxError", &format!("Cannot convert {} to a BigInt", s)).pipe_err(),
        },
        _ => realm.type_error("Cannot convert value to a BigInt"),
    }
}

trait PipeErr {
    fn pipe_err<T>(self) -> JsResult<T>;
}
impl PipeErr for Value {
    fn pipe_err<T>(self) -> JsResult<T> {
        Err(self)
    }
}

fn this_bigint(realm: &mut Realm, this: &Value) -> JsResult<BigInt> {
    match this {
        Value::BigInt(b) => Ok((**b).clone()),
        Value::Object(id) => match &realm.obj(*id).kind {
            ObjectKind::BigInt(b) => Ok((**b).clone()),
            _ => realm.type_error("BigInt.prototype method called on incompatible receiver"),
        },
        _ => realm.type_error("BigInt.prototype method called on incompatible receiver"),
    }
}

fn bigint_to_string(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let v = this_bigint(realm, &this)?;
    let radix = match arg(args, 0) {
        Value::Undefined => 10,
        r => {
            let r = to_integer(realm.to_number(r)?);
            if !(2.0..=36.0).contains(&r) {
                return realm.range_error("toString() radix must be between 2 and 36");
            }
            r as u32
        }
    };
    Ok(Value::str(&v.to_string_radix(radix)))
}

fn bigint_value_of(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let v = this_bigint(realm, &this)?;
    Ok(realm.new_bigint_value(v))
}

fn as_int_n(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let bits = realm.to_index(arg(args, 0))?;
    let v = match arg(args, 1) {
        Value::BigInt(b) => b,
        _ => return realm.type_error("Cannot convert value to a BigInt"),
    };
    Ok(realm.new_bigint_value(v.as_int_n(bits as u64)))
}

fn as_uint_n(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let bits = realm.to_index(arg(args, 0))?;
    let v = match arg(args, 1) {
        Value::BigInt(b) => b,
        _ => return realm.type_error("Cannot convert value to a BigInt"),
    };
    Ok(realm.new_bigint_value(v.as_uint_n(bits as u64)))
}
