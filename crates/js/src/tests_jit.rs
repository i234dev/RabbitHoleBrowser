//! Differential tests of the baseline compiler: every program runs in a
//! fresh realm with the interpreter only and again with every function
//! compiled at its first run (threshold 0) — and with a small threshold,
//! so hot loops enter native code in the middle (loop OSR) — and the
//! completion value, exception and console output must be identical.

use crate::realm::Realm;
use crate::{JsResult, Value};
use std::cell::{Cell, RefCell};
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
fn frame_switches_reload_slots_and_constants() {
    // Distinct callees have distinct constant pools. Deep recursion moves
    // the value stack, and each caller resumes at several different pcs.
    check(r#"
        function left(n) { return "left:" + n; }
        function right(n) { return "right:" + n; }
        function recurse(n) {
            if (n === 0) return 1;
            var first = left(n), value = recurse(n - 1), last = right(n);
            if (first !== "left:" + n || last !== "right:" + n) throw "frame";
            return value + 1;
        }
        recurse(2000)
    "#, "2001");
    // Native callbacks re-enter the engine; throws/finally resume callers
    // at non-adjacent instructions rather than the ordinary call return.
    check(r#"
        function leaf(n) { if (n === 2) throw "two"; return n * 3; }
        function outer() {
            var log = [];
            for (var i = 0; i < 4; i++) {
                try { log.push([i].map(leaf)[0]); }
                catch (e) { log.push(e); }
                finally { log.push("finally:" + i); }
            }
            return log.join(",");
        }
        outer()
    "#, "0,finally:0,3,finally:1,two,finally:2,9,finally:3");
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
    // Compile and enter this same loop before starting the deadline. Measuring
    // only cumulative entries could pass on a previous, unrelated native call.
    realm.eval("function spin(stop) { var i = 0; while (i !== stop) { i = (i + 1) | 0 } return i; } spin(1)", "spin-prepare.js").unwrap();
    let before = realm.jit_entries();
    realm.set_deadline(Some(std::time::Instant::now() + std::time::Duration::from_millis(200)));
    let r = realm.eval("spin(Infinity)", "spin.js");
    let error = realm.root(r.expect_err("the watchdog stops the loop"));
    let entries = realm.jit_entries();
    let aborts = realm.watchdog_abort_count();
    realm.set_deadline(None);
    let message = realm.error_to_string(&error.value());
    assert!(message.contains("timed out"), "{message}");
    assert!(aborts > 0, "the watchdog really aborted execution");
    if rabbithole_jit::supported() {
        assert!(entries > before, "the interrupted invocation must enter native code, not just its warmup");
    } else { assert_eq!(entries, 0); }
    eprintln!("native-canary watchdog target={}-{} override_enabled={} entries_before={before} entries_after={entries} watchdog_aborts={aborts}", std::env::consts::OS, std::env::consts::ARCH, realm.jit_enabled());
}

// Unlike general differential checks, these record entry at the native host
// callback and after its reentrant JS call. Windows uses only the existing
// validation override; Options::on remains blocked there.
#[derive(Default)]
struct CallbackGcCanary {
    initial_entries: u64,
    entries_at_callback: Cell<u64>,
    entries_after_reentry: Cell<u64>,
    collections: Cell<usize>,
}

fn canary_collect(realm: &mut Realm, _: Value, _: &[Value]) -> JsResult<Value> {
    let stats = realm.native_data::<CallbackGcCanary>(realm.current_native().unwrap()).unwrap();
    realm.gc();
    stats.collections.set(stats.collections.get() + 1);
    Ok(Value::Undefined)
}

fn canary_reenter_and_collect(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let stats = realm.native_data::<CallbackGcCanary>(realm.current_native().unwrap()).unwrap();
    stats.entries_at_callback.set(realm.jit_entries() - stats.initial_entries);
    let callback = args[0].clone();
    let held = args[1].clone();
    let result = realm.call(callback, Value::Undefined, &[held])?;
    // This host is GC-safe: its arguments are rooted by call_native, and its
    // newly returned value needs a root until this extra collection finishes.
    realm.native_root(result.clone());
    stats.entries_after_reentry.set(realm.jit_entries() - stats.initial_entries);
    realm.gc();
    stats.collections.set(stats.collections.get() + 1);
    Ok(result)
}

#[test]
fn native_override_callback_reentry_keeps_values_alive_through_gc() {
    const SOURCE: &str = r#"
        function callback(kept) {
            var sum = 0;
            for (var i = 0; i < 2048; i++) {
                var trash = { index: i, values: [i, i + 1] };
                sum += trash.index;
            }
            __canaryCollect();
            if (kept.marker !== 'alive' || kept.nested.value !== 7) throw 'callback lost its argument';
            return { kept: kept, sum: sum };
        }
        function outer() {
            var held = { marker: 'alive', nested: { value: 7 } };
            for (var i = 0; i < 2048; i++) { var trash = [i, { value: i }]; }
            var result = __canaryReenter(callback, held);
            __canaryCollect();
            return result.kept === held && result.sum === 2096128 && held.nested.value === 7;
        }
        outer()
    "#;
    for stress in [crate::realm::GC_STRESS_FULL, crate::realm::GC_STRESS_INCREMENTAL] {
        for requested in [false, true] {
            let mut realm = Realm::new();
            realm.set_jit(requested, 0);
            realm.gc_enabled = true;
            realm.gcx.no_gc = false;
            realm.gcx.stress = stress;
            realm.gcx.incremental = stress == crate::realm::GC_STRESS_INCREMENTAL;
            // Realm bootstrap observes the ambient selector before set_jit.
            // Measure only this explicit override, not those earlier entries.
            let initial_entries = realm.jit_entries();
            let stats = Rc::new(CallbackGcCanary { initial_entries, ..Default::default() });
            let global = realm.global();
            for (name, arity, function) in [
                ("__canaryCollect", 0, canary_collect as crate::NativeFn),
                ("__canaryReenter", 2, canary_reenter_and_collect as crate::NativeFn),
            ] {
                let native = realm.new_native_function_full(name, arity, function, Some(stats.clone()), Vec::new(), false);
                realm.define_property(global, name, Value::Object(native), crate::HIDDEN);
                realm.mark_gc_safe(global, name);
            }
            realm.set_deadline(Some(std::time::Instant::now() + std::time::Duration::from_secs(30)));
            let outcome = realm.eval(SOURCE, "native-callback-gc-canary.js");
            realm.set_deadline(None);
            let result = outcome.unwrap();
            assert_eq!(result, Value::Bool(true), "requested={requested} stress={stress}");
            assert_eq!(stats.collections.get(), 3, "three explicit collections actually ran");
            if requested && rabbithole_jit::supported() {
                assert!(realm.jit_enabled());
                assert!(stats.entries_at_callback.get() > 0, "outer entered native before its host callback");
                assert!(stats.entries_after_reentry.get() > stats.entries_at_callback.get(), "the reentrant callback itself entered native");
            } else {
                assert!(!realm.jit_enabled());
                assert_eq!(realm.jit_entries(), initial_entries);
                assert_eq!(stats.entries_at_callback.get(), 0);
                assert_eq!(stats.entries_after_reentry.get(), 0);
            }
            assert!(realm.frames.is_empty() && realm.vstack.is_empty() && realm.new_target_stack.is_empty());
            assert_eq!(realm.native_depth, 0);
            eprintln!("native-canary callback-gc target={}-{} requested_override={requested} enabled={} stress={stress} initial_entries={initial_entries} entries_at_callback={} entries_after_reentry={} measured_entries={} explicit_collections={}", std::env::consts::OS, std::env::consts::ARCH, realm.jit_enabled(), stats.entries_at_callback.get(), stats.entries_after_reentry.get(), realm.jit_entries() - initial_entries, stats.collections.get());
        }
    }
}


struct ActiveFrameCanary {
    expected_native: bool,
    getter_depth: Cell<u32>,
    gc_depth: Cell<u32>,
    getter_calls: Cell<usize>,
    collections: Cell<usize>,
}

fn active_frame_collect(realm: &mut Realm, _: Value, _: &[Value]) -> JsResult<Value> {
    let stats = realm.native_data::<ActiveFrameCanary>(realm.current_native().unwrap()).unwrap();
    let before = realm.jit.live_entries;
    stats.gc_depth.set(before);
    assert_eq!(before > 0, stats.expected_native, "GC hook must retain the caller's generated frame");
    // Prove collection happened, rather than counting a request that could be
    // deferred: this fresh object has no GC root, and alloc does not collect.
    let unreachable = realm.new_object();
    let freed = realm.gc();
    assert!(freed > 0 && realm.heap.try_get(unreachable).is_none(), "the explicit GC must reclaim its unrooted probe");
    assert_eq!(realm.jit.live_entries, before);
    stats.collections.set(stats.collections.get() + 1);
    Ok(Value::Undefined)
}

fn active_frame_getter(realm: &mut Realm, this: Value, _: &[Value]) -> JsResult<Value> {
    let stats = realm.native_data::<ActiveFrameCanary>(realm.current_native().unwrap()).unwrap();
    let before = realm.jit.live_entries;
    stats.getter_depth.set(before);
    // This observation occurs BEFORE reentry can create another native entry.
    assert_eq!(before > 0, stats.expected_native, "the property helper must still have its generated caller");
    let held = realm.get(this, crate::PropertyKey::str("kept"))?;
    realm.native_root(held.clone());
    let callback = realm.get(Value::Object(realm.global()), crate::PropertyKey::str("activeFrameCallback"))?;
    let result = realm.call(callback, Value::Undefined, &[held])?;
    realm.native_root(result.clone());
    assert_eq!(realm.jit.live_entries, before, "nested entry must restore dynamic depth");
    stats.getter_calls.set(stats.getter_calls.get() + 1);
    Ok(result)
}

#[test]
fn native_property_getter_keeps_generated_frame_live_during_gc() {
    const SOURCE: &str = r#"
        function activeFrameCallback(held) {
            var allocated = [];
            for (var j = 0; j < 128; j++) allocated.push({value: j});
            __activeFrameCollect();
            if (held.marker !== 'alive' || allocated[127].value !== 127) throw 'getter callback lost roots';
            return held;
        }
        var receiver = {kept: {marker: 'alive', nested: {value: 7}}};
        Object.defineProperty(receiver, 'value', {get: __activeFrameGetter});
        function readProperty(o) {
            var total = 0;
            for (var i = 0; i < 3; i++) {
                var held = o.value;
                if (held !== o.kept || held.marker !== 'alive') throw 'getter lost identity';
                total += held.nested.value;
            }
            return total;
        }
        readProperty(receiver) === 21
    "#;
    for stress in [crate::realm::GC_STRESS_FULL, crate::realm::GC_STRESS_INCREMENTAL] {
        for requested in [false, true] {
            let mut realm = Realm::new();
            realm.set_jit(requested, 0);
            realm.gc_enabled = true;
            realm.gcx.no_gc = false;
            realm.gcx.stress = stress;
            realm.gcx.incremental = stress == crate::realm::GC_STRESS_INCREMENTAL;
            let initial_entries = realm.jit_entries();
            let stats = Rc::new(ActiveFrameCanary {
                expected_native: requested && rabbithole_jit::supported(),
                getter_depth: Cell::new(0), gc_depth: Cell::new(0),
                getter_calls: Cell::new(0), collections: Cell::new(0),
            });
            let global = realm.global();
            for (name, function) in [
                ("__activeFrameGetter", active_frame_getter as crate::NativeFn),
                ("__activeFrameCollect", active_frame_collect as crate::NativeFn),
            ] {
                let native = realm.new_native_function_full(name, 0, function, Some(stats.clone()), Vec::new(), false);
                realm.define_property(global, name, Value::Object(native), crate::HIDDEN);
                realm.mark_gc_safe(global, name);
            }
            realm.set_deadline(Some(std::time::Instant::now() + std::time::Duration::from_secs(30)));
            let outcome = realm.eval(SOURCE, "native-live-frame-gc-canary.js");
            realm.set_deadline(None);
            assert_eq!(outcome.unwrap(), Value::Bool(true));
            assert_eq!(stats.getter_calls.get(), 3, "exercise both initial lookup and cached getter");
            assert_eq!(stats.collections.get(), 3, "all three GC probes were really reclaimed");
            assert_eq!(realm.jit.live_entries, 0);
            if !stats.expected_native { assert_eq!(realm.jit_entries(), initial_entries); }
            assert!(realm.frames.is_empty() && realm.vstack.is_empty() && realm.new_target_stack.is_empty());
            assert!(realm.native_roots.is_empty());
            assert_eq!(realm.native_depth, 0);
            assert_eq!(realm.gc_block, 0);
            eprintln!("native-canary live-frame target={}-{} requested_override={requested} enabled={} stress={stress} initial_entries={initial_entries} measured_entries={} getter_calls={} verified_collections={} getter_live_depth={} gc_live_depth={} live_entries_after={}", std::env::consts::OS, std::env::consts::ARCH, realm.jit_enabled(), realm.jit_entries() - initial_entries, stats.getter_calls.get(), stats.collections.get(), stats.getter_depth.get(), stats.gc_depth.get(), realm.jit.live_entries);
        }
    }
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
    if rabbithole_jit::supported() && crate::jit::LOOP_OSR {
        assert!(realm.jit_entries() > 0, "the loop continued in native code");
    } else if !rabbithole_jit::supported() {
        assert!(!realm.jit.on, "unsupported architectures stay interpreted");
        assert_eq!(realm.jit_entries(), 0);
    }
}

#[test]
fn inline_caches_and_scopes() {
    // Property caches (own, prototype, deep prototype, getters installed
    // later, frozen objects, dictionaries, proxies), closure variables,
    // TDZ, loose equality and typeof, compared over the whole value mix.
    same(r#"var vals = [undefined, null, 0, -0, 1, NaN, "", "0", "1", "a", true, false, {}, [], [1], Symbol.iterator, 10n, function(){}, {valueOf(){ return 1; }}, {toString(){ return "a"; }}];
var out = [];
function eq(a, b) { return a == b; }
function ne(a, b) { return a != b; }
function eqn(a) { return a == null; }
function nen(a) { return a != undefined; }
function eqs(a) { return a == "a"; }
function ty(a) { return typeof a; }
for (var k = 0; k < 3; k++)
for (var i = 0; i < vals.length; i++) {
  out.push(eqn(vals[i]), nen(vals[i]), eqs(vals[i]), ty(vals[i]));
  for (var j = 0; j < vals.length; j++) { try { out.push(eq(vals[i], vals[j]), ne(vals[i], vals[j])); } catch (e) { out.push("E" + e.name); } }
}
var o = {x: 1}; var p = Object.create(o); p.y = 2; var q = Object.create(p);
function g(z) { return z.x + (z.y || 0); }
for (var i = 0; i < 50; i++) out.push(g(o), g(p), g(q));
o.x = "s"; out.push(g(q)); Object.defineProperty(o, "x", { get() { return 7; } }); out.push(g(q), g(p), g(o));
function setx(z, v) { z.x = v; return z.x; }
var w = {x: 0}; for (var i = 0; i < 20; i++) out.push(setx(w, "v" + i));
Object.freeze(w); out.push(setx(w, 5)); var dd = {}; for (var i = 0; i < 200; i++) dd["k" + i] = i; out.push(setx(dd, 9));
function cl() { let a = 1; const f = () => a++; let b; function h() { return b; } b = 3; return [f(), f(), h(), a]; }
for (var i = 0; i < 5; i++) out.push(cl().join());
function tdz() { try { return (() => { const r = u; let u = 1; return r; })(); } catch (e) { return e.name; } }
out.push(tdz(), tdz());
class A { constructor() { this.v = 1; } m() { return this.v; } }
class B extends A { constructor() { super(); this.w = 2; } m() { return super.m() + this.w; } }
for (var i = 0; i < 10; i++) out.push(new B().m(), new A().m());
var px = new Proxy({}, { get(t, k) { return "P" + String(k); } });
function gp(z) { return z.x; }
out.push(gp(px), gp({x: 3}), gp(px), gp(Object.create({x: 4})));
out.join(",")
"#);
    check("function f() { let a = 1; const g = () => { a = a + 'x'; return a }; g(); return g() } f()", "1xx");
    check("function f(o) { return o.k } var d = {}; for (var i = 0; i < 300; i++) d['p' + i] = i; d.k = 'dict'; String([f({k: 1}), f(d), f(Object.create(d))])", "1,dict,dict");
}

#[test]
fn array_elements() {
    // In-range elements, holes (and getters on the prototype), typed
    // arrays, strings, frozen arrays, out-of-range / fractional / negative
    // / huge / NaN indices, appends.
    same(r#"var out = [];
function get(a, i) { return a[i]; }
function set(a, i, v) { a[i] = v; return a[i]; }
var keys = [0, 1, 2, 3, -1, -0, 1.5, NaN, Infinity, 2 ** 53, "1", "x", 4294967295, 1e300];
var arrs = [[1, "two", 3n], [, 5, , 7], new Int8Array([1, 2, 3]), "abc", {0: "o", 1: "p"}, Object.freeze([9, 8, 7]), (function () { var x = [1, 2]; x.p = 1; return x; })()];
for (var r = 0; r < 4; r++)
  for (var a = 0; a < arrs.length; a++)
    for (var k = 0; k < keys.length; k++) {
      try { out.push(String(get(arrs[a], keys[k]))); } catch (e) { out.push("E" + e.name); }
      try { out.push(String(set(arrs[a], keys[k], r % 2 ? "s" + r : r))); } catch (e) { out.push("E" + e.name); }
    }
var proto = [10, 11]; var sub = Object.create(proto);
out.push(get(sub, 0), get(sub, 1));
Object.defineProperty(Array.prototype, 5, { get() { return "proto5"; }, configurable: true });
var holey = [1, , 3, , , ]; out.push(get(holey, 1), get(holey, 5), get([0, 1, 2, 3, 4], 5));
delete Array.prototype[5];
var big = []; for (var i = 0; i < 1000; i++) set(big, i, "v" + i); out.push(big.length, get(big, 999));
var ss = 0; for (var i = 0; i < 2000; i++) ss += get(big, i % 1000).length; out.push(ss);
out.join(",")
"#);
}

#[test]
fn calls_with_wide_static_operands_and_return_slots() {
    // Exercise call operands beyond the low byte, rather than only
    // the low byte used by small hand-written functions.
    let names: Vec<_> = (0..300).map(|i| format!("p{i}")).collect();
    let args = (0..300).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    check(&format!("function leaf({}){{return p299;}} function outer(){{return leaf({args});}} outer()", names.join(",")), "299");
    let mut source = String::from("function leaf(x){return x;}function many(){let s=0;");
    for i in 0..300 {
        source.push_str(&format!("s+=leaf({i});"));
    }
    source.push_str("return s;}many()");
    check(&source, "44850");
    // Return a counted value held beyond the low-byte range, while frames
    // are repeatedly pushed/popped and the caller's slots can move.
    let locals = (0..300).map(|i| format!("let v{i}='v{i}';")).collect::<String>();
    check(&format!("function f(){{{locals}return v299;}}function g(){{let out='';for(let i=0;i<20;i++)out=f();return out;}}g()"), "v299");
}

#[test]
fn return_status_switches_to_interpreter_entries_and_constructor_errors() {
    // A loop makes native eligible even when called by interpreted code.
    // Direct eval keeps bridge interpreted, so returns cross both boundaries.
    check(r#"
function native(x) { for (var i = 0; i < 2; i++) x++; return x; }
function bridge(x) { eval(''); return native(x); }
function outer(x) { for (var i = 0; i < 2; i++) x = bridge(x); return x; }
var out = [bridge(1), outer(2), [1, 2, 3].map(native).join(':')];
class A { constructor() { this.x = 'base'; } }
class B extends A {
  constructor() { super(); for (var i = 0; i < 2; i++) {} return 1; }
}
try { new B(); } catch (e) { out.push(e.name); }
function counted(x) { for (var i = 0; i < 2; i++) {} return 'v:' + x; }
for (var j = 0; j < 100; j++) out[4] = counted(bridge(j));
out.join('|')
"#, "3|6|3:4:5|TypeError|v:101");
}
