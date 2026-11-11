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

#[test]
fn arguments_direct_reads() {
    // `arguments.length` / `arguments[i]` served from the frame.
    eq("function f() { return arguments.length + ':' + arguments[0] + ':' + arguments[1]; } f(7) + '|' + f(1, 2, 3) + '|' + f()", "1:7:undefined|3:1:2|0:undefined:undefined");
    eq("function f(a) { var s = 0; for (var i = 0; i < arguments.length; i++) s += arguments[i]; return s; } f(1, 2, 3, 4)", "10");
    // Babel-style default parameters.
    eq("function f() { var a = arguments.length > 0 && arguments[0] !== undefined ? arguments[0] : 5; return a; } f() + f(1) + f(undefined)", "11");
    // Arguments are not mapped to parameters (same as the object path).
    eq("function f(a, b) { a = 10; return arguments[0] + arguments[1]; } f(1, 2)", "3");
    eq("function f(a) { a = 10; var o = arguments; return arguments[0] + o[0]; } f(1)", "2");
    eq("function f(a) { 'use strict'; a = 10; return arguments[0]; } f(1)", "1");
    eq("function f(a = 0) { a = 10; return arguments[0]; } f(1)", "1");
    eq("function f(a, b) { b = 5; return arguments[1]; } String(f(1))", "undefined");
    // Non-index keys and inherited properties.
    eq("function f() { return arguments['length'] + typeof arguments[Symbol.iterator] + arguments['0']; } f('x')", "1functionx");
    eq("Object.prototype[3] = 'p'; function f() { return arguments[3]; } var r = f(1); delete Object.prototype[3]; r", "p");
    eq("function f() { return arguments[0.5] + ',' + arguments[-1]; } f(1)", "undefined,undefined");
    // Escaping uses still get a real object.
    eq("function f() { arguments[0] = 9; return arguments[0]; } f(1)", "9");
    eq("function f() { var a = arguments; return a[0] + arguments[1] + a.length; } f(1, 2)", "5");
    eq("function f() { delete arguments[0]; return String(arguments[0]); } f(1)", "undefined");
    eq("function f() { arguments.length = 0; return arguments.length; } f(1, 2)", "0");
    eq("function f() { return [].slice.call(arguments).concat(arguments.length).join(); } f(1, 2)", "1,2,2");
    eq("function f() { var r = eval('1'); return arguments[0] + r; } f(1)", "2");
    eq("function f() { return (() => arguments[0] + arguments.length)(); } f(4, 5)", "6");
    eq("function f() { var arguments = [8]; return arguments[0] + arguments.length; } f(1, 2)", "9");
    eq("function f(arguments) { return arguments[0]; } f('ab')", "a");
    eq("function f() { { let arguments = 'q'; return arguments[0]; } } f(1)", "q");
    // Constructors, generators and nested functions.
    eq("function P() { this.n = arguments.length; this.a = arguments[1]; } var p = new P(1, 'b'); p.n + p.a", "2b");
    eq("function* g() { yield arguments.length; yield arguments[1]; } var it = g(3, 4); it.next().value + it.next().value", "6");
    eq("function f() { function g() { return arguments.length; } return g(1, 2, 3) + arguments.length; } f(1)", "4");
    eq("var o = { m() { return arguments[0] * 2; } }; o.m(21)", "42");
    eq("function f() { return arguments[0](); } f(function() { return 'called'; })", "called");
}

#[test]
fn for_in_fast_path() {
    eq("var o = {a: 1, b: 2, c: 3}; var k = []; for (var x in o) k.push(x); k.join()", "a,b,c");
    eq("var o = {b: 1, 2: 1, a: 1, 1: 1}; var k = []; for (var x in o) k.push(x); k.join()", "1,2,b,a");
    // Deletion during iteration skips the deleted key.
    eq("var o = {a: 1, b: 2, c: 3}; var k = []; for (var x in o) { k.push(x); delete o.b; } k.join()", "a,c");
    eq("var o = {a: 1, b: 2, c: 3}; var k = []; for (var x in o) { k.push(x); if (x == 'a') { delete o.c; o.c = 1; } } k.join()", "a,b,c");
    // Arrays: holes, shrinking, extra properties.
    eq("var a = [1, , 3]; a.x = 1; var k = []; for (var i in a) k.push(i); k.join()", "0,2,x");
    eq("var a = [1, 2, 3]; var k = []; for (var i in a) { k.push(i); a.length = 1; } k.join()", "0");
    // Non-enumerable own and inherited enumerables.
    eq("var o = Object.defineProperty({a: 1}, 'h', {value: 1, enumerable: false}); var k = []; for (var x in o) k.push(x); k.join()", "a");
    eq("function C() { this.own = 1; } C.prototype.inh = 2; var k = []; for (var x in new C()) k.push(x); k.join()", "own,inh");
    eq("Object.prototype.zz = 1; var k = []; for (var x in {a: 1}) k.push(x); delete Object.prototype.zz; k.join()", "a,zz");
    eq("var k = []; for (var x in null) k.push(x); for (var y in undefined) k.push(y); for (var z in 5) k.push(z); k.length", "0");
    eq("var o = {a: 1}; var n = 0; for (var i = 0; i < 1000; i++) for (var x in o) n += o[x]; n", "1000");
}

#[test]
fn register_translation() {
    // Assignment chains, compound and update expressions on registers.
    eq("(function() { var a, b, c; a = b = c = 5; c += a * b; return [a, b, c].join(); })()", "5,5,30");
    eq("(function() { var i = 0, r = []; r.push(i++, i++, ++i, i--, --i); return r.join() + '|' + i; })()", "0,1,3,3,1|1");
    eq("(function() { var x = '1'; x++; var y = '5'; y -= 1; return typeof x + x + typeof y + y; })()", "number2number4");
    eq("(function() { var o = {n: 1}, a = [1, 2]; o.n++; ++o.n; o.n += 10; a[0]++; a[1] *= 5; return o.n + ',' + a.join(); })()", "13,2,10");
    eq("(function() { var k = 'n', o = {n: 2}; o[k] **= 3; o[k] ||= 9; o.m ??= 4; o.m &&= 7; return o.n + ',' + o.m; })()", "8,7");
    // Operand order with side effects and constants on either side.
    eq("(function() { var log = []; var a = {valueOf() { log.push('a'); return 1; }}, b = {valueOf() { log.push('b'); return 2; }}; var r = [a < b, 1 < b, a < 2, 3 - a, a - 3, a > b, b >= a]; return r.join() + '|' + log.join(''); })()", "true,true,true,2,-2,false,true|abbaaaabba");
    eq("(function() { var s = 'x'; return (1 + s) + (s + 1) + (1 + 2 + s) + (s + 1 + 2); })()", "1xx13xx12");
    eq("(function() { var n = NaN; var r = []; for (var i = 0; i < 1; i++) { r.push(n < 1, n >= 1, !(n < 1), 1 > n); } return r.join(); })()", "false,false,true,false");
    // Strict equality and switch.
    eq("(function(v) { switch (v) { case 1: return 'one'; case '1': return 'str'; default: return 'other'; } })('1')", "str");
    eq("(function() { var r = []; for (var i = 0; i < 6; i++) { switch (i % 3) { case 0: r.push('a'); break; case 1: r.push('b'); default: r.push('c'); } } return r.join(''); })()", "abccabcc");
    // Loops: continue, labeled continue/break, do-while, while with complex tests.
    eq("(function() { var r = []; for (var i = 0; i < 5; i++) { if (i == 2) continue; r.push(i); } return r.join(); })()", "0,1,3,4");
    eq("(function() { var r = [], i = 0; while (i < 5) { i++; if (i % 2) continue; r.push(i); } return r.join(); })()", "2,4");
    eq("(function() { var r = []; outer: for (var i = 0; i < 3; i++) { for (var j = 0; j < 3; j++) { if (j == 1) continue outer; if (i == 2) break outer; r.push(i + '' + j); } } return r.join(); })()", "00,10");
    eq("(function() { var i = 0; do { i++; } while (i < 3); return i; })()", "3");
    eq("(function() { var n = 0; for (;;) { if (++n > 4) break; } while (true) { n++; if (n > 7) break; } return n; })()", "8");
    eq("(function() { var a = [3, 1, 2], c = 0; for (var i = 0; i < a.length && a[i] != 2; i++) c++; return c; })()", "2");
    eq("(function() { var fs = []; for (let i = 0; i < 3; i++) fs.push(() => i); return fs.map(f => f()).join(); })()", "0,1,2");
    eq("(function() { var r = 0; for (let i = 0, j = 10; i < j; i++, j--) r++; return r; })()", "5");
    eq("(function() { var t = 0; for (var i = 10; i > 0; i -= 3) t += i; for (var k = 0; k <= 3; k++) t += k; for (var m = 5; m >= 5; m--) t += m; return t; })()", "33");
    // TDZ: unchecked reads only after the declaration.
    throws("(function() { switch (1) { case 0: let y = 1; break; case 1: return y; } })()", "before initialization");
    throws("(function() { for (let k of [k]) {} })()", "ReferenceError");
    throws("(function() { let a = a + 1; })()", "before initialization");
    throws("(function() { for (var i = 0; i < 2; i++) { if (i == 1) { q++; } let q = 0; } })()", "before initialization");
    eq("(function() { let a = 1; a++; a += 2; const b = a * 2; let c; return [a, b, String(c)].join(); })()", "4,8,undefined");
    eq("(function() { var r = []; for (const x of [1, 2]) { let y = x * 2; r.push(y); } for (let k in {p: 1}) r.push(k); return r.join(); })()", "2,4,p");
    throws("(function() { const c = 1; c++; })()", "Assignment to constant variable.");
    // Calls with many arguments, missing arguments and method calls.
    eq("(function() { function f(a, b, c) { return [a, b, c].join('-'); } return f(1) + '|' + f(1, 2, 3, 4) + '|' + f.call(null, 'x'); })()", "1--|1-2-3|x--");
    eq("(function() { var o = { v: 2, m(x) { return this.v * x; } }; var s = 0; for (var i = 0; i < 3; i++) s += o.m(i); return s + (5).toString() + 'ab'.charAt(1); })()", "65b");
    eq("(function() { function f() { return this; } 'use strict'; return typeof f(); })()", "object");
    eq("(function() { 'use strict'; function f() { return this; } return typeof f(); })()", "undefined");
    // Conditional and logical operators keep values.
    eq("(function() { var a = 0, b = 'x', c = null; return [a || b, a && b, b && a, c ?? 'd', a ?? 'd', a ? 1 : 2, b ? 1 : 2].join(); })()", "x,0,0,d,0,2,1");
    eq("(function() { var o = null, p = {q: {r: 5}}; return [o?.x, p?.q?.r, o?.x.y.z, p.q?.['r']].map(String).join(); })()", "undefined,5,undefined,5");
    // typeof, void, delete, in, instanceof.
    eq("(function() { var u, o = {a: 1}; return [typeof u, typeof o, void 0, delete o.a, 'a' in o, o instanceof Object].map(String).join(); })()", "undefined,object,undefined,true,false,true");
    // Exceptions inside expressions and loops keep the stack consistent.
    eq("(function() { var r = []; for (var i = 0; i < 3; i++) { try { r.push(i, i == 1 ? null.x : i); } catch (e) { r.push('E'); } } return r.join(); })()", "0,0,E,2,2");
    eq("(function() { var s = 0; for (var i = 0; i < 4; i++) { try { if (i % 2) throw i; s += 10; } catch (e) { s += e; } finally { s += 100; } } return s; })()", "424");
    // Generators resume into the right slot.
    eq("(function() { function* g() { var a = yield 1; var b = (yield a + 1) * 2; return a + b; } var it = g(); var r = [it.next().value, it.next(5).value, it.next(3).value]; return r.join(); })()", "1,6,11");
    eq("(function() { function* g() { var s = 0; for (var i = 0; i < 3; i++) s += yield i; return s; } var it = g(), r = [], x = it.next(); while (!x.done) { r.push(x.value); x = it.next(10); } return r.join() + '|' + x.value; })()", "0,1,2|30");
    // Big functions with many registers and deep expressions.
    let mut src = String::from("(function() { var t = 0;");
    for i in 0..300 {
        src.push_str(&format!(" var v{i} = {i}; t += v{i} * 2 - (v{i} % 3);"));
    }
    src.push_str(" return t; })()");
    let expect: i64 = (0..300).map(|i: i64| i * 2 - i % 3).sum();
    eq(&src, &expect.to_string());
    let deep = format!("(function() {{ var x = 1; return {}x{}; }})()", "(1 + ".repeat(200), ")".repeat(200));
    eq(&deep, "201");
}

#[test]
fn json_and_array_like_fast_paths() {
    eq("JSON.stringify({a: undefined, b: 1, c: function() {}, d: [undefined, function() {}, Symbol()], e: {}})", "{\"b\":1,\"d\":[null,null,null],\"e\":{}}");
    eq("JSON.stringify({a: undefined})", "{}");
    eq("JSON.stringify({x: undefined, y: 2}, null, 2)", "{\n  \"y\": 2\n}");
    eq("JSON.stringify([], null, 2) + JSON.stringify({}, null, 2) + JSON.stringify([[]], null, 1)", "[]{}[\n []\n]");
    eq("JSON.stringify({a: [1, {b: 2}]}, null, '--')", "{\n--\"a\": [\n----1,\n----{\n------\"b\": 2\n----}\n--]\n}");
    eq("var o = JSON.parse('{\"a\":1,\"b\":[-0,12,-34,1e3,0.5,123456789012345678],\"a\":3,\"\\\\u0041\":4,\"c\\\\nd\":5}'); [o.a, 1 / o.b[0], o.b[1], o.b[2], o.b[3], o.b[4], o.b[5], o.A, o['c\\nd'], Object.keys(o).join()].join()", "3,-Infinity,12,-34,1000,0.5,123456789012345680,4,5,a,b,A,c\nd");
    eq("var a = JSON.parse('[{\"k\":1},{\"k\":2}]'); a[0].k + a[1].k", "3");
    eq("JSON.parse('{\"__proto__\": 1}').__proto__", "1");
    eq("Array.from({length: 3}, (_, i) => i * 2).join()", "0,2,4");
    eq("Array.from({length: 2}).map(String).join()", "undefined,undefined");
    eq("Array.from({length: 2, 1: 'x'}).map(String).join()", "undefined,x");
    eq("Object.prototype[1] = 'p'; var r = Array.from({length: 2}).map(String).join(); delete Object.prototype[1]; r", "undefined,p");
    eq("Math.max.apply(null, {length: 2, 0: 4, 1: 9})", "9");
}

#[test]
fn lazy_compilation() {
    // Nested functions compile on first call and see their enclosing scopes.
    eq("(function() { var a = 1; let b = 2; const c = 3; function f(x) { return a + b + c + x; } return f(4) + f(5); })()", "21");
    eq("(function() { var n = 0; function inc() { n++; return n; } inc(); inc(); return n; })()", "2");
    eq("(function() { var fs = []; for (let i = 0; i < 3; i++) { let j = i * 2; fs.push(function() { return i + j; }); } return fs.map(f => f()).join(); })()", "0,3,6");
    eq("(function() { function outer(a) { return function mid(b) { return (c) => a + b + c; }; } return outer(1)(2)(3); })()", "6");
    eq("(function() { var x = 'outer'; function f() { var x = 'inner'; return (() => x)(); } return f() + x; })()", "innerouter");
    eq("(function() { var k = 10; function f(a = k + 1) { return a; } return f() + f(1); })()", "12");
    // Self reference, recursion and hoisting.
    eq("var fact = function f(n) { return n < 2 ? 1 : n * f(n - 1); }; fact(5)", "120");
    eq("(function() { return g(); function g() { return h() + 1; } function h() { return 41; } })()", "42");
    // `arguments`, `this` and `new.target` in nested functions.
    eq("(function() { function f() { return (() => arguments.length + arguments[0])(); } return f(5, 6); })()", "7");
    eq("(function() { var o = { v: 3, m() { return [1, 2].map(x => x * this.v).join(); } }; return o.m(); })()", "3,6");
    eq("(function() { function C() { this.t = new.target === C; this.a = (() => new.target === C)(); } var c = new C(); return c.t && c.a; })()", "true");
    // Generators, async functions and classes inside lazily compiled code.
    eq("(function() { function* g(n) { for (let i = 0; i < n; i++) yield i * i; } return [...g(4)].join(); })()", "0,1,4,9");
    eq("var r = []; (async function() { async function f(x) { await null; return x * 2; } r.push(await f(21)); })(); r", "42");
    eq("(function() { class A { #p = 7; get() { return [1].map(() => this.#p)[0]; } static make() { return new A(); } } return A.make().get(); })()", "7");
    eq("(function() { class B { constructor(x) { this.x = x; } } class D extends B { constructor() { super(5); this.y = (() => this.x + 1)(); } } return new D().y; })()", "6");
    // Strictness is inherited.
    eq("(function() { 'use strict'; function f() { return this; } return typeof f(); })()", "undefined");
    eq("(function() { function f() { return this === undefined; } return f(); })()", "false");
    throws("(function() { 'use strict'; function f() { undeclared_var_xyz = 1; } f(); })()", "ReferenceError");
    // Function metadata without compiling.
    eq("(function() { function f(a, b, c = 1, ...d) {} var g = (x, y) => x; return f.length + ',' + f.name + ',' + g.length + ',' + g.name; })()", "2,f,2,g");
    eq("(function() { var h = function() {}; return h.name; })()", "h");
    eq("typeof (function f() { return 1; }).toString()", "string");
    // Closures created from the same code before and after its first call.
    eq("(function() { var fs = []; for (var i = 0; i < 3; i++) fs.push((x => () => x)(i)); return fs[2]() + fs[0]() + fs[1](); })()", "3");
    // Compile errors of a function surface when it is first called.
    eq("(function() { var ok = 1; function never() { with ({}) {} } return ok; })()", "1");
    throws("(function() { function bad() { with ({}) {} } bad(); })()", "SyntaxError");
    // Global code and functions defined in eval / new Function.
    eq("var G = 5; function useG() { return G * 2; } useG()", "10");
    eq("new Function('a', 'b', 'function inner(x) { return x * a; } return inner(b);')(3, 4)", "12");
    eq("eval('(function(q) { return function() { return q + 1; }; })(1)()')", "2");
}
