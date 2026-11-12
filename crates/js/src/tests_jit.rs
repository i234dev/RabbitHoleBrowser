//! Differential tests of the baseline compiler: every program runs in a
//! fresh realm with the interpreter only and again with every function
//! compiled at its first run (threshold 0) — and with a small threshold,
//! so hot loops enter native code in the middle (loop OSR) — and the
//! completion value, exception and console output must be identical.

use crate::realm::Realm;
use std::cell::RefCell;
use std::rc::Rc;

/// Completion value or error, then the console output.
pub(crate) fn run_mode(src: &str, jit: Option<u32>) -> String {
    let mut realm = Realm::new();
    match jit {
        Some(t) => realm.set_jit(true, t),
        None => realm.set_jit(false, 0),
    }
    let out = Rc::new(RefCell::new(String::new()));
    let o = out.clone();
    realm.set_console_sink(Box::new(move |_, msg| {
        o.borrow_mut().push_str(&msg);
        o.borrow_mut().push('\n');
    }));
    let r = realm.eval(src, "jit.js");
    realm.run_jobs();
    let head = match r {
        Ok(v) => match realm.to_string(v) {
            Ok(s) => format!("ok {}", s.to_rust_string()),
            Err(e) => format!("tostring-error {}", realm.error_to_string(&e)),
        },
        Err(e) => format!("error {}", realm.error_to_string(&e)),
    };
    let console = out.borrow().clone();
    format!("{head}\n{console}")
}

/// Run `src` in every mode and require identical results.
pub(crate) fn same(src: &str) -> String {
    let vm = run_mode(src, None);
    for t in [0, 3] {
        let jit = run_mode(src, Some(t));
        assert_eq!(vm, jit, "threshold {t} differs for:\n{src}");
    }
    vm
}

fn check(src: &str, expected: &str) {
    let r = same(src);
    let head = r.lines().next().unwrap_or("");
    assert_eq!(head, format!("ok {expected}"), "source:\n{src}");
}

#[test]
fn arithmetic() {
    check("function f(a, b) { return a * b + 1 } f(6, 7)", "43");
    check("function f(a, b) { return a * b + 1 } f('6', 7)", "43");
    check("function f(a, b) { return a + b } f('x', 1) + f(1, 'y') + f(1.5, 2.25)", "x11y3.75");
    check("function f(a) { return [a / 0, -a / 0, 0 / 0, a % 3, -a % 3, a % -3, 5.5 % 2] } String(f(7))", "Infinity,-Infinity,NaN,1,-1,1,1.5");
    check("function f(x) { return 1 / (x * -1) } f(0)", "-Infinity");
    check("function f(x) { return -x } String([f(0) === 0, 1 / f(0), f(NaN), f('3')])", "true,-Infinity,NaN,-3");
    check("function f(x) { return [x & 0xff, x | 1, x ^ -1, x << 3, x >> 2, x >>> 0, ~x] } String(f(-1234.7))", "46,-1233,1233,-9872,-309,4294966062,1233");
    check("function f(x) { return [x | 0, x >>> 0, x << 0] } String(f(4294967296 * 3 + 5))", "5,5,5");
    check("function f(x) { return [x | 0, x >>> 0, ~x] } String(f(2 ** 70))", "0,0,-1");
    check("function f(x) { return [x | 0, x & 7, x >>> 1] } String(f(NaN))", "0,0,0");
    check("function f(a, b) { return a ** b } String([f(2, 10), f(-8, 1/3), f(0, 0)])", "1024,NaN,1");
    check("function f(a, b) { return [a < b, a <= b, a > b, a >= b, a == b, a != b, a === b, a !== b] } String(f(1, 2)) + String(f(NaN, NaN)) + String(f(2, 2))", "true,true,false,false,false,true,false,truefalse,false,false,false,false,true,false,truefalse,true,false,true,true,false,true,false");
    check("function f(a, b) { return [a < b, a == b, a === b] } String(f('a', 'b')) + String(f(1, '1')) + String(f(null, undefined))", "true,false,falsefalse,true,falsefalse,true,false");
    check("function f(i) { i++; i--; ++i; return i } f(41)", "42");
    check("function f(i) { i++; return i } String([f('1'), f(null), f(undefined), f(10n)])", "2,1,NaN,11");
    check("function f(s) { var n = 0; for (var i = 0; i < s.length; i++) n = (n * 31 + s.charCodeAt(i)) | 0; return n } f('hello world')", "1794106052");
    check("function f() { var x = 0.1, y = 0.2; return x + y } f()", "0.30000000000000004");
    check("function f(x) { return +x } String([f('12'), f(''), f({}), f([7])])", "12,0,NaN,7");
}

#[test]
fn nan_canonicalization() {
    // Float64Array exposes arbitrary NaN payloads; arithmetic on them must
    // never produce a bit pattern in the tag space.
    check(
        "var b = new ArrayBuffer(8), u = new Uint32Array(b), d = new Float64Array(b);
         u[0] = 1; u[1] = 0xFFF1FFFF;
         function f(x) { var y = x + 1; var z = -x; return [y !== y, z !== z, typeof y, typeof z, y * 0] }
         String(f(d[0]))",
        "true,true,number,number,NaN",
    );
}

#[test]
fn control_flow() {
    check("function f(n) { var s = 0; for (var i = 0; i < n; i++) { if (i % 3 == 0) continue; s += i; if (s > 1000) break; } return s } f(100)", "1027");
    check("function f(n) { var s = 0, i = 0; while (i < n) { s += i; i += 2 } do { s-- } while (s > 100); return s } f(50)", "100");
    check("function f(a) { var r = []; for (var i = 0; i < a.length; i++) { r.push(a[i] ? 'T' : 'F') } return r.join('') } f([0, 1, '', 'x', NaN, null, undefined, {}, [], -0, 0n, 1n])", "FTFTFFFTTFFT");
    check("function f(x) { if (x == null) return 'nullish'; if (x === undefined) return 'u'; return typeof x } String([f(null), f(undefined), f(0), f('')])", "nullish,nullish,number,string");
    check("function f(x) { return x ?? 'd' } String([f(null), f(undefined), f(0), f(false)])", "d,d,0,false");
    check("function f(x) { switch (x) { case 1: return 'one'; case 'a': return 'A'; default: return 'other' } } String([f(1), f('a'), f(2), f('1')])", "one,A,other,other");
    check("function f(n) { var s = 0; for (let i = 0; i < n; i++) { let j = i; s += (() => j)() } return s } f(10)", "45");
    check("function f() { var s = ''; for (var k in {a: 1, b: 2, c: 3}) s += k; for (var v of [4, 5]) s += v; return s } f()", "abc45");
    check("function f(n) { var i = 0; for (;;) { if (++i >= n) break } return i } f(100000)", "100000");
    check("function f(n) { var s = 0; for (var i = n; i > 0; i--) s += i; return s } f(1000)", "500500");
    check("function f(a, b) { return a !== b ? (a === 'x' ? 1 : 2) : 3 } String([f('x', 'y'), f('z', 'y'), f('y', 'y'), f(NaN, NaN), f(0, -0)])", "1,2,3,2,3");
}

#[test]
fn values_and_refcounts() {
    // Strings and BigInts in registers that are overwritten by numbers and
    // back (reference counts are handled by helpers).
    check("function f(n) { var s = 'a', t; for (var i = 0; i < n; i++) { t = s; s = i; s = t + i } return s } f(50).length", "91");
    check("function f() { var a = 1n, b = a; a = 2; b = b + 1n; return String(a) + String(b) } f()", "22");
    check("function f() { let x; try { x = y; let y = 1 } catch (e) { return e.name } } f()", "ReferenceError");
    check("function f() { const o = {x: 1}; let s = ''; for (let i = 0; i < 3; i++) { s += o.x + i } return s } f()", "123");
    check("var g = 0; function f(n) { for (var i = 0; i < n; i++) g += i; return g } f(100)", "4950");
    check("function f() { var o = {get x() { return this.y * 2 }, y: 21}; return o.x } f()", "42");
    check("function f() { var o = {valueOf() { log.push('v'); return 5 }}, log = []; var r = o + 1; return r + log.join() } f()", "6v");
    check("function f() { var a = [1, 2, 3]; a[5] = 6; var s = 0; for (var i = 0; i < a.length; i++) s += a[i] | 0; return s + ':' + a.length } f()", "12:6");
}

#[test]
fn calls_and_exceptions() {
    check("function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2) } fib(20)", "6765");
    check("function f(x) { if (x > 3) throw new Error('big ' + x); return x } function g() { var s = 0; for (var i = 0; i < 10; i++) { try { s += f(i) } catch (e) { s += 100 } } return s } g()", "606");
    check("function f() { try { return 1 } finally { log.push('f') } } var log = []; String([f(), log])", "1,f");
    check("function f(n) { try { null.x } catch (e) { return n + ':' + e.constructor.name } } f(1)", "1:TypeError");
    check("function f() { return [1, 2, 3].map(x => x * 2).reduce((a, b) => a + b) } f()", "12");
    check("class A { constructor(x) { this.x = x } get d() { return this.x * 2 } m(y) { return this.x + y } } function f() { var a = new A(4); return a.d + a.m(1) } f()", "13");
    check("function f(n) { return n == 0 ? 0 : 1 + f(n - 1) } f(5000)", "5000");
    check("function f() { return arguments.length + arguments[1] } f(1, 2, 3)", "5");
    check("function f(...r) { return r.length } f(1, 2, 3)", "3");
    check("function* g() { yield 1; yield 2 } function f() { var s = 0; for (var v of g()) s += v; return s } f()", "3");
    check("async function a() { return 1 } function f() { var r; a().then(v => { r = v }); return typeof r } f()", "undefined");
    check("function f() { return eval('1 + 1') } f()", "2");
    check("function f() { var o = {}; Object.defineProperty(o, 'x', {set(v) { this.y = v * 2 }}); o.x = 4; return o.y } f()", "8");
    check("function f() { 'use strict'; try { undefined.x = 1 } catch (e) { return e instanceof TypeError } } f()", "true");
    check("function f(n) { var e; try { (function r() { r() })() } catch (x) { e = x } return e instanceof RangeError } f()", "true");
}

#[test]
fn errors_identical() {
    // Uncaught errors: message and kind must match.
    same("function f(x) { return x.y.z } f({})");
    same("function f() { let a = b; let b = 1 } f()");
    same("function f(n) { for (var i = 0; i < n; i++) if (i == 7) throw new RangeError('at ' + i) } f(10)");
    same("function f() { return notDefined + 1 } f()");
    same("function f() { console.log('a', 1 + 1); console.log([1, 2].join('-')) } f()");
}

#[test]
fn native_code_is_freed_with_its_function() {
    let before = crate::jit::code_bytes();
    {
        let mut realm = Realm::new();
        realm.set_jit(true, 0);
        let v = realm.eval("var s = 0; for (var i = 0; i < 300; i++) s += (new Function('x', 'return x + ' + i))(1); s", "free.js").unwrap();
        assert_eq!(realm.to_string(v).unwrap().to_rust_string(), "45150");
        if crate::jit::PHASE >= 5 {
            assert!(realm.jit.stats.compiled >= 300, "functions were compiled");
        }
        realm.gc();
    }
    // Every function (and its native code) went with the realm.
    assert_eq!(crate::jit::code_bytes(), before);
}

#[test]
fn watchdog_stops_native_loop() {
    let mut realm = Realm::new();
    realm.set_jit(true, 0);
    realm.set_deadline(Some(std::time::Instant::now() + std::time::Duration::from_millis(200)));
    let r = realm.eval("function spin() { var i = 0; while (true) { i = (i + 1) | 0 } } spin()", "spin.js");
    let e = r.expect_err("the watchdog stops the loop");
    assert!(realm.error_to_string(&e).contains("timed out"), "{}", realm.error_to_string(&e));
}

#[test]
fn recursion_limit_is_the_interpreters() {
    // Calls from native code go through the run loop: the maximum depth is
    // the same with and without native code.
    same("var d = 0; function r() { d++; r() } try { r() } catch (e) { } d");
}

#[test]
fn hot_loop_enters_native_code() {
    // A loop that runs once (script body) gets compiled while it runs.
    let src = "var s = 0; for (var i = 0; i < 300000; i++) { s = (s + i * 3) % 1000003 } s";
    assert_eq!(same(src).lines().next(), Some("ok 145003"));
    let mut realm = Realm::new();
    realm.set_jit(true, 1);
    realm.eval(src, "osr.js").unwrap();
    if crate::jit::LOOP_OSR {
        assert!(realm.jit_entries() > 0, "the loop continued in native code");
    }
}
