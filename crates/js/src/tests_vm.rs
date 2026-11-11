//! Regression tests for the interpreter fast paths: register allocation of
//! non-captured locals, inline caches, shapes, call paths and numeric fast
//! paths. Each case is checked against plain ECMAScript semantics.

use crate::realm::Realm;

fn run(src: &str) -> Result<String, String> {
    let mut realm = Realm::new();
    realm.set_console_sink(Box::new(|_, _| {}));
    let r = realm.eval(src, "test.js");
    realm.run_jobs();
    match r {
        Ok(v) => {
            let s = realm.to_string(v).map_err(|e| realm.error_to_string(&e))?;
            Ok(s.to_rust_string())
        }
        Err(e) => Err(realm.error_to_string(&e)),
    }
}

fn eq(src: &str, expected: &str) {
    match run(src) {
        Ok(s) => assert_eq!(s, expected, "source: {}", src),
        Err(e) => panic!("{} threw: {}", src, e),
    }
}

fn throws(src: &str, contains: &str) {
    match run(src) {
        Ok(s) => panic!("{} should throw, got {}", src, s),
        Err(e) => assert!(e.contains(contains), "{}: expected error containing {:?}, got {:?}", src, contains, e),
    }
}

#[test]
fn locals_and_captures() {
    // Mixed captured / uncaptured locals in one function.
    eq("function f(a, b) { var c = a + b; var d = function() { return c * 2; }; var e = 1; e += d(); return e; } f(1, 2)", "7");
    // Captured parameter modified after closure creation.
    eq("function f(x) { var g = () => x; x = 5; return g(); } f(1)", "5");
    // Captured via nested-nested closure.
    eq("function f() { var v = 1; return function() { return function() { return v++; }; }; } var g = f()(); g(); g()", "2");
    // Shadowed names in inner functions (conservative capture still correct).
    eq("function f() { var x = 1; function g() { var x = 2; return x; } return x + g(); } f()", "3");
    // Block-scoped captures per iteration.
    eq("var fs = []; (function() { for (let i = 0; i < 3; i++) { let j = i * 2; fs.push(() => i + j); } })(); fs.map(f => f()).join()", "0,3,6");
    // Per-iteration bindings not captured (registers) keep loop semantics.
    eq("(function() { var s = 0; for (let i = 0; i < 10; i++) { let k = i; s += k; } return s; })()", "45");
    // Closure created in loop body captures block let, loop var is register.
    eq("(function() { var out = []; for (var i = 0; i < 3; i++) { let k = i; out.push(function() { return k; }); } return out.map(f => f()).join(); })()", "0,1,2");
    // TDZ for register lets.
    throws("(function() { x; let x = 1; })()", "Cannot access 'x' before initialization");
    throws("(function() { { y = 2; let y; } })()", "Cannot access 'y' before initialization");
    throws("(function() { const c = 1; c = 2; })()", "Assignment to constant variable.");
    // TDZ re-armed when a block is re-entered.
    throws("(function() { for (var i = 0; i < 2; i++) { if (i == 1) { z; } let z = i; } })()", "before initialization");
    // arguments object and captured arguments in arrows.
    eq("function f(a) { a = 9; return arguments[0]; } f(1)", "1");
    eq("function f() { var g = () => arguments.length; return g(); } f(1, 2, 3)", "3");
    // Named function expression self binding.
    eq("var f = function fact(n) { return n < 2 ? 1 : n * fact(n - 1); }; f(6)", "720");
    // Default params referencing earlier params and closures.
    eq("function f(a, b = () => a) { a = 3; return b(); } f(1)", "3");
    eq("function f(a, b = a * 2) { return a + b; } f(2)", "6");
    // Destructured params.
    eq("function f({a, b: [c, d] = [3, 4]}, ...rest) { return a + c + d + rest.length; } f({a: 1}, 0, 0)", "10");
    // Catch parameter scoping.
    eq("(function() { var e = 'outer'; try { throw 'inner'; } catch (e) { var r = e; } return r + e; })()", "innerouter");
    eq("(function() { try { throw 1; } catch ({message = 'm'}) { return message; } })()", "m");
    // Switch with lexical declarations and closure.
    eq("(function(v) { switch (v) { case 1: let a = 'one'; return (() => a)(); default: return 'd'; } })(1)", "one");
    // Class private names and class binding inside methods.
    eq("class A { #x = 1; static make() { return new A(); } get x() { return this.#x; } } A.make().x", "1");
    eq("(function() { class B { m() { return B.name; } } return new B().m(); })()", "B");
    // for-in / for-of with register heads.
    eq("(function() { var s = ''; for (const k in {a: 1, b: 2}) s += k; for (let v of [1, 2]) s += v; return s; })()", "ab12");
    eq("(function() { var fs = []; for (const v of [1, 2, 3]) fs.push(() => v); return fs.map(f => f()).join(); })()", "1,2,3");
    // Labeled continue out of a block scope.
    eq("(function() { var s = 0; outer: for (let i = 0; i < 3; i++) { let k = i; for (let j = 0; j < 3; j++) { if (j > k) continue outer; s++; } } return s; })()", "6");
    // try/finally with returns from register code.
    eq("(function() { var x = 1; try { return x; } finally { x = 2; } })()", "1");
    eq("(function() { let log = []; for (let i = 0; i < 3; i++) { try { if (i == 1) continue; log.push(i); } finally { log.push('f'); } } return log.join(''); })()", "0ff2f");
    // Deep recursion with locals.
    eq("function sum(n) { var acc = n; if (n > 0) acc += sum(n - 1); return acc; } sum(500)", "125250");
}

#[test]
fn generators_keep_registers() {
    eq("function* g() { let a = 1; var b = 2; yield a + b; a = 10; yield a + b; for (let i = 0; i < 2; i++) yield i; } [...g()].join()", "3,12,0,1");
    eq("function* g(n) { let total = 0; while (true) { const x = yield total; if (x === undefined) return total; total += x * n; } } var it = g(2); it.next(); it.next(3); it.next(4); it.next().value", "14");
    eq("async function f() { let a = 1; await null; let b = a + 1; await null; return a + b; } var r; f().then(v => r = v); r", "undefined");
    eq("var out = []; async function f() { for (let i = 0; i < 3; i++) { await null; out.push(i); } } f(); 1", "1");
    eq("function* g() { try { yield 1; yield 2; } finally { } } var it = g(); it.next(); it.return(5).value", "5");
}

#[test]
fn property_caches() {
    // Same site sees different shapes (polymorphic) and dictionary objects.
    eq("function get(o) { return o.x; } var a = [{x: 1}, {y: 2, x: 3}, {x: 5, z: 1}, Object.create({x: 7}), {}]; a.map(get).join()", "1,3,5,7,");
    // Prototype property later shadowed by own property.
    eq("function P() {} P.prototype.v = 1; var p = new P(); var r = []; for (var i = 0; i < 3; i++) { r.push(p.v); if (i == 1) p.v = 9; } r.join()", "1,1,9");
    // Prototype property changed after caching.
    eq("function P() {} P.prototype.v = 1; var p = new P(); var r = []; for (var i = 0; i < 3; i++) { r.push(p.v); P.prototype.v = 5; } r.join()", "1,5,5");
    // Prototype swapped.
    eq("var a = {v: 1}, b = {v: 2}; var o = Object.create(a); var r = []; for (var i = 0; i < 3; i++) { r.push(o.v); Object.setPrototypeOf(o, b); } r.join()", "1,2,2");
    // Deleted property falls back to prototype.
    eq("var o = Object.create({k: 'proto'}); o.k = 'own'; var r = []; for (var i = 0; i < 2; i++) { r.push(o.k); delete o.k; } r.join()", "own,proto");
    // Getter / setter on prototype through a cached site.
    eq("var proto = { get g() { return this.n * 2; }, set g(v) { this.n = v; } }; var o = Object.create(proto); var r = 0; for (var i = 0; i < 5; i++) { o.g = i; r += o.g; } r", "20");
    // Non-writable property: assignment ignored (sloppy) / throws (strict).
    eq("var o = {}; Object.defineProperty(o, 'x', {value: 1, writable: false}); for (var i = 0; i < 3; i++) o.x = i; o.x", "1");
    throws("'use strict'; var o = Object.freeze({x: 1}); for (var i = 0; i < 3; i++) o.x = i;", "read only");
    // Frozen prototype property blocks assignment on the instance.
    eq("var p = {}; Object.defineProperty(p, 'x', {value: 1, writable: false}); var o = Object.create(p); for (var i = 0; i < 3; i++) o.x = 5; o.x + ',' + Object.keys(o).length", "1,0");
    // Setter defined on prototype after transitions were cached.
    eq("function P() {} var log = []; function mk() { var o = new P(); o.a = 1; return o; } mk(); mk(); Object.defineProperty(P.prototype, 'a', {set(v) { log.push(v); }}); mk(); log.join()", "1");
    // Property order preserved by transitions and after delete.
    eq("var o = {b: 1, a: 2}; o.c = 3; delete o.a; o.a = 4; Object.keys(o).join()", "b,c,a");
    eq("var o = {}; for (var i = 0; i < 200; i++) o['k' + i] = i; delete o.k5; var s = 0; for (var k in o) s += o[k]; s + ',' + Object.keys(o).length", "19895,199");
    // Non-extensible objects reject new properties.
    eq("var o = Object.preventExtensions({a: 1}); for (var i = 0; i < 3; i++) o.b = i; o.b", "undefined");
    // Integer-like keys go to elements, not shapes.
    eq("var o = {}; o[1] = 'a'; o['2'] = 'b'; o.x = 'c'; Object.keys(o).join()", "1,2,x");
    // Accessor redefined as data property.
    eq("var o = { get v() { return 1; } }; var r = []; for (var i = 0; i < 2; i++) { r.push(o.v); Object.defineProperty(o, 'v', {value: 2}); } r.join()", "1,2");
    // Proxy at a cached site.
    eq("var t = {x: 1}; var p = new Proxy(t, { get(o, k) { return k === 'x' ? 42 : o[k]; } }); var objs = [t, p, t]; objs.map(o => o.x).join()", "1,42,1");
    // Strings, numbers and arrays at a property site.
    eq("function len(o) { return o.length; } [len('abc'), len([1, 2]), len({length: 7}), len(function(a, b) {})].join()", "3,2,7,2");
    eq("var r = []; ['a', 'bb', 'ccc'].forEach(s => r.push(s.toUpperCase())); r.join()", "A,BB,CCC");
    // Global variables through caches, including redefinition and deletion.
    eq("var g = 1; function rd() { return g; } var r = rd(); g = 2; r += rd(); globalThis.g = 3; r + rd()", "6");
    eq("globalThis.q1 = 1; function rd() { return typeof q1 === 'undefined' ? 'u' : q1; } var r = '' + rd(); delete globalThis.q1; r + rd()", "1u");
    throws("globalThis.q2 = 1; function rd() { return q2; } rd(); delete globalThis.q2; rd()", "q2 is not defined");
    eq("var r = []; function rd() { return typeof lx; } r.push(rd()); let lx = 1; r.push(rd()); r.join()", "undefined,number");
    eq("Object.defineProperty(globalThis, 'acc', {get() { return 11; }, configurable: true}); function rd() { return acc; } rd() + rd()", "22");
}

#[test]
fn numeric_fast_paths() {
    eq("-7 % 7", "0");
    eq("Object.is(-7 % 7, -0)", "true");
    eq("Object.is(-0 % 5, -0)", "true");
    eq("5 % 0", "NaN");
    eq("5.5 % 2", "1.5");
    eq("-5 % 3", "-2");
    eq("2147483647 + 1", "2147483648");
    eq("var s = 0; for (var i = 0; i < 100; i++) s = (s + i * 7) % 13; s", "5");
    eq("var a = 2147483647; a++; a", "2147483648");
    eq("var x = 0.1; x += 0.2; x", "0.30000000000000004");
    eq("1 < 2 && 'a' < 'b' && !(NaN < 1) && !(NaN >= 1)", "true");
    eq("var r = []; for (var i = 0; i < 3; i++) r.push(i < '2'); r.join()", "true,true,false");
    eq("(-1) >>> 28", "15");
    eq("var a = [1, 2, 3]; a[1.5] = 'x'; a.length + a[1.5]", "3x");
    eq("var a = []; a[-1] = 5; a.length + ',' + a[-1]", "0,5");
    eq("var a = [1, 2]; a[4] = 5; a.length + ',' + a[3]", "5,undefined");
    eq("var a = []; for (var i = 0; i < 1000; i++) a[i] = i; var s = 0; for (var i = 0; i < a.length; i++) s += a[i]; s", "499500");
    eq("var s = 'hello'; var r = ''; for (var i = 0; i < s.length; i++) r += s[i]; r", "hello");
    eq("var t = new Uint8Array(4); for (var i = 0; i < 6; i++) t[i] = i * 100; Array.from(t).join()", "0,100,200,44");
    eq("var a = [1, , 3]; [a[1], 1 in a].join()", ",false");
    eq("var o = {}; o[0] = 1; o[1] = 2; o[0] + o[1]", "3");
    eq("var b = 10n; b++; b * 2n", "22");
    eq("var x = '5'; x++; x", "6");
}

#[test]
fn calls_and_frames() {
    eq("function f(a, b, c) { return [a, b, c].join(); } f(1) + '|' + f(1, 2, 3, 4)", "1,,|1,2,3");
    eq("function f() { return arguments.length; } f() + f(1) + f(1, 2)", "3");
    eq("var o = { n: 1, f() { return this.n; } }; var g = o.f; [o.f(), (0, o.f)() === undefined || true].join()", "1,true");
    eq("function F(a) { this.a = a; } F.prototype.get = function() { return this.a; }; var s = 0; for (var i = 0; i < 100; i++) s += new F(i).get(); s", "4950");
    eq("class A { constructor(x) { this.x = x; } } class B extends A { constructor() { super(5); this.y = 1; } } var b = new B(); b.x + b.y", "6");
    eq("Math.max(1, 5, 3) + Math.min()", "Infinity");
    eq("[3, 1, 2].sort((a, b) => a - b).join()", "1,2,3");
    eq("function f(...r) { return r.length; } f.apply(null, [1, 2, 3]) + f.call(null, 1)", "4");
    eq("var bound = function(a, b) { return this.k + a + b; }.bind({k: 1}, 2); bound(3)", "6");
    eq("function outer() { return inner(); function inner() { return 'in'; } } outer()", "in");
    throws("function f() { throw new Error('boom'); } function g() { return f(); } g()", "boom");
    eq("function f() { try { return g(); } catch (e) { return 'caught ' + e; } } function g() { throw 'x'; } f()", "caught x");
    eq("var n = 0; function rec(d) { if (d == 0) return 0; return 1 + rec(d - 1); } rec(2000)", "2000");
    eq("function f() { 'use strict'; return this; } [f() === undefined, f.call(2)].join()", "true,2");
    eq("var o = {f: function() { return typeof this; }}; o.f()", "object");
}

#[test]
fn superinstructions_and_fast_paths() {
    // Constant operands with non-number left sides.
    eq("var s = 'a'; s = s + 1; s += 2; s", "a12");
    eq("var o = {valueOf() { return 4; }}; [o * 2, o - 1, o % 3, o | 1, o << 1].join()", "8,3,1,5,8");
    eq("var b = 5n; (b * 2n).toString()", "10");
    eq("var r = []; for (var i = '0'; i < 3; i++) r.push(typeof i); r.join()", "string,number,number");
    eq("var r = []; var x = {valueOf() { r.push('v'); return 1; }}; for (var k = 0; x < 2 && k < 2; k++) r.push(k); r.join()", "v,0,v,1,v");
    eq("(function() { var a = 3, b = 4; return a + b + a * b; })()", "19");
    eq("(function() { var t = 0; for (var i = 0; i < 10; i++) { t += i & 3; t ^= 1; } return t; })()", "15");
    // Completion values at script level.
    eq("var x = 1; x + 1; for (var i = 0; i < 3; i++) { i * 10; }", "20");
    // Number to string conversions.
    eq("[String(0), String(-0), String(42), String(-7), String(1e21), String(123456789012), String(2 ** 53), String(0.5)].join()", "0,0,42,-7,1e+21,123456789012,9007199254740992,0.5");
    eq("var o = {}; o[70000] = 1; o[5] = 2; Object.keys(o).join()", "5,70000");
    eq("var a = []; a[70000] = 1; a.length + ',' + a[70000] + ',' + (70000 in a) + ',' + (69999 in a)", "70001,1,true,false");
    // Constructors through the fast path.
    eq("function P(x) { this.x = x; } P.prototype.m = function() { return this.x * 2; }; new P(4).m()", "8");
    eq("function Q() { return {z: 1}; } new Q().z", "1");
    eq("function R() { this.a = 1; return 5; } new R().a", "1");
    eq("function S() {} S.prototype = 7; Object.getPrototypeOf(new S()) === Object.prototype", "true");
    eq("function T(a, b) { this.s = a + (b === undefined ? 'u' : b); } new T('x').s + new T('x', 'y', 'z').s", "xuxy");
    eq("function U() { this.n = new.target === U; } new U().n", "true");
    eq("function V() { return arguments.length; } new V(1, 2, 3) instanceof V", "true");
    eq("var calls = 0; function W() { calls++; if (calls < 3) new W(); } new W(); calls", "3");
    throws("var A = () => {}; new A()", "is not a constructor");
    throws("var o = { m() {} }; new o.m()", "is not a constructor");
    // Method calls through GetMethod.
    eq("var o = { v: 2, f() { return this.v; } }; var r = []; for (var i = 0; i < 3; i++) r.push(o.f()); r.join()", "2,2,2");
    eq("'abc'.toUpperCase() + (5).toFixed(1) + true.toString()", "ABC5.0true");
    throws("var o = {}; o.missing()", "o.missing is not a function");
    throws("var o = {x: 1}; o.x()", "o.x is not a function");
    // for-in with prototype enumerables and shadowing.
    eq("var p = {a: 1, b: 2}; var o = Object.create(p); o.b = 3; Object.defineProperty(o, 'a', {value: 9, enumerable: false}); var k = []; for (var x in o) k.push(x); k.join()", "b");
    eq("var p = {z: 1}; var o = Object.create(p); o.y = 2; var k = []; for (var x in o) k.push(x); k.join()", "y,z");
    eq("var k = []; for (var x in [5, 6]) k.push(x); for (var y in 'ab') k.push(y); k.join()", "0,1,0,1");
}

#[test]
fn realm_dropped_during_thread_teardown() {
    // A realm kept in thread-local storage is destroyed while the thread's
    // other thread-locals (string caches, the root shape...) may already be
    // gone: dropping its strings and shapes must not touch them.
    thread_local! {
        static KEEP: std::cell::RefCell<Option<Realm>> = const { std::cell::RefCell::new(None) };
    }
    let t = std::thread::spawn(|| {
        // Registered first, so destroyed after the engine's thread-locals.
        KEEP.with(|k| k.borrow_mut().take());
        let mut realm = Realm::new();
        let src = "var big = 'x'.repeat(40); var o = {}; var keys = [];\n\
                   for (var i = 0; i < 200; i++) { var k = big + i + big; o[k] = k + k; keys.push(k + '!'); }\n\
                   var s = ''; for (var j = 0; j < 2000; j++) s += 'ab' + j; o[s] = s; keys.length";
        let v = realm.eval(src, "t.js").map_err(|e| realm.error_to_string(&e)).unwrap();
        assert_eq!(realm.to_string(v).unwrap().to_rust_string(), "200");
        realm.gc();
        KEEP.with(|k| *k.borrow_mut() = Some(realm));
    });
    t.join().expect("thread with a realm exited cleanly");
    // Dropping a realm on a spawned thread normally works too.
    std::thread::spawn(|| {
        let mut realm = Realm::new();
        realm.eval("var a = []; for (var i = 0; i < 100; i++) a.push({x: 'y'.repeat(60) + i}); a.length", "t.js").unwrap();
    })
    .join()
    .unwrap();
}
