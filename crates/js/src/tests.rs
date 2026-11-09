//! Engine tests driven through `Realm::eval`.

use crate::realm::Realm;
use crate::value::Value;

fn run(src: &str) -> Result<String, String> {
    let mut realm = Realm::new();
    let logs = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let l2 = logs.clone();
    realm.set_console_sink(Box::new(move |_lvl, text| l2.borrow_mut().push(text)));
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

/// Evaluate and compare the string form of the completion value.
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

/// Run with console.log capture and return the joined log lines.
fn logs(src: &str) -> String {
    let mut realm = Realm::new();
    let logs = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let l2 = logs.clone();
    realm.set_console_sink(Box::new(move |_lvl, text| l2.borrow_mut().push(text)));
    if let Err(e) = realm.eval(src, "test.js") {
        panic!("threw: {}", realm.error_to_string(&e));
    }
    realm.run_jobs();
    let out = logs.borrow().join("|");
    out
}

#[test]
fn arithmetic_and_coercion() {
    eq("1 + 2", "3");
    eq("'5' * '2'", "10");
    eq("[] + {}", "[object Object]");
    eq("null == undefined", "true");
    eq("null === undefined", "false");
    eq("NaN !== NaN", "true");
    eq("0.1 + 0.2", "0.30000000000000004");
    eq("1/0", "Infinity");
    eq("-0 === 0", "true");
    eq("Object.is(-0, 0)", "false");
    eq("1/-0", "-Infinity");
    eq("'3' - 1", "2");
    eq("'3' + 1", "31");
    eq("true + true", "2");
    eq("[1,2] + [3]", "1,23");
    eq("typeof null", "object");
    eq("typeof undefined", "undefined");
    eq("typeof function(){}", "function");
    eq("typeof Symbol()", "symbol");
    eq("typeof 10n", "bigint");
    eq("typeof undeclaredVariable", "undefined");
    eq("7 % 3", "1");
    eq("-7 % 3", "-1");
    eq("2 ** 10", "1024");
    eq("2 ** 3 ** 2", "512");
    eq("(-8) >>> 0", "4294967288");
    eq("-8 >> 1", "-4");
    eq("1 << 31", "-2147483648");
    eq("5 & 3 | 8 ^ 1", "9");
    eq("~5", "-6");
    eq("'b' > 'a'", "true");
    eq("'10' < '9'", "true");
    eq("10 < '9'", "false");
    eq("null + 1", "1");
    eq("undefined + 1", "NaN");
    eq("'abc'[1]", "b");
    eq("'abc'.length", "3");
    eq("1 == '1'", "true");
    eq("0 == ''", "true");
    eq("0 == '0'", "true");
    eq("'' == '0'", "false");
    eq("[0] == false", "true");
}


#[test]
fn strings_and_templates() {
    eq("`a${1 + 1}b${'c'}`", "a2bc");
    eq("'hello'.toUpperCase()", "HELLO");
    eq("'a,b,c'.split(',').length", "3");
    eq("'  x '.trim()", "x");
    eq("'abc'.replace('b', 'X')", "aXc");
    eq("'aaa'.replaceAll('a', 'b')", "bbb");
    eq("'x'.padStart(3, '0')", "00x");
    eq("'abc'.at(-1)", "c");
    eq("String(123)", "123");
    eq("String(null)", "null");
    eq("String(Symbol('q'))", "Symbol(q)");
    eq("'abc'.charCodeAt(1)", "98");
    eq("String.fromCharCode(72, 105)", "Hi");
    eq("'a-b-c'.split('-', 2).join('+')", "a+b");
    eq("'abc'.includes('bc')", "true");
    eq("'😀'.length", "2");
    eq("[...'😀a'].length", "2");
    eq("'abc'.slice(-2)", "bc");
    eq("'abcdef'.substring(4, 1)", "bcd");
    eq("'x'.repeat(3)", "xxx");
    eq("String.raw`a\\n${1}`", "a\\n1");
    eq("((s, ...v) => s.raw.join('|') + v.join(','))`x${1}y${2}z`", "x|y|z1,2");
    eq("'a\\tb'.length", "3");
    eq("'\\u0041\\x42'", "AB");
    eq("encodeURIComponent('a b&c/é')", "a%20b%26c%2F%C3%A9");
    eq("decodeURIComponent('%C3%A9')", "é");
    eq("'abc'.localeCompare('abd')", "-1");
    eq("'Hello World'.replace(/o/g, '0')", "Hell0 W0rld");
    eq("'2020-01-02'.replace(/(\\d+)-(\\d+)-(\\d+)/, '$3/$2/$1')", "02/01/2020");
    eq("'a1b2'.replace(/\\d/g, d => d * 2)", "a2b4");
    eq("'x'.match(/(?<n>x)/).groups.n", "x");
    eq("'a,b;c'.split(/[,;]/).join('-')", "a-b-c");
    eq("[...'a1b2'.matchAll(/\\d/g)].map(m => m[0]).join('')", "12");
    eq("/a/.test('cat')", "true");
    eq("/^a/i.test('Apple')", "true");
    eq("'abc'.search(/c/)", "2");
}

#[test]
fn variables_and_scope() {
    eq("var a = 1; { var a = 2; } a", "2");
    eq("let a = 1; { let a = 2; } a", "1");
    eq("function f() { return x; var x = 5; } f()", "undefined");
    eq("function f() { return typeof g; function g() {} } f()", "function");
    throws("let q = 1; let q = 2;", "already been declared");
    throws("{ x; let x = 1; }", "Cannot access 'x' before initialization");
    throws("const c = 1; c = 2;", "Assignment to constant variable.");
    throws("undefinedVar", "undefinedVar is not defined");
    eq("var fs = []; for (let i = 0; i < 3; i++) fs.push(() => i); fs.map(f => f()).join()", "0,1,2");
    eq("var fs = []; for (var i = 0; i < 3; i++) fs.push(() => i); fs.map(f => f()).join()", "3,3,3");
    eq("function counter() { let c = 0; return () => ++c; } const k = counter(); k(); k(); k()", "3");
    eq("(function() { 'use strict'; return this; })()", "undefined");
    eq("(function() { return this === globalThis; })()", "true");
    eq("var o = { f() { return this.v; }, v: 7 }; o.f()", "7");
    eq("var o = { f() { return () => this.v; }, v: 8 }; o.f()()", "8");
    eq("function f(a, b = a + 1, ...rest) { return [a, b, rest.length]; } f(1).join() + '|' + f(1, 5, 6, 7).join()", "1,2,0|1,5,2");
    eq("function f() { return arguments.length + ':' + arguments[1]; } f(1, 2, 3)", "3:2");
    eq("function f() { return (() => arguments[0])(); } f(9)", "9");
    eq("var x = 10; function f() { x = 20; } f(); x", "20");
    eq("globalThis.zz = 5; zz", "5");
    eq("var s = 0; for (var i = 0, j = 10; i < j; i++, j--) s += j - i; s", "30");
    eq("let n = 0; do { n++; } while (n < 5); n", "5");
    eq("let n = 0; while (true) { if (++n > 3) break; } n", "4");
    eq("let s = ''; outer: for (let i = 0; i < 3; i++) { for (let j = 0; j < 3; j++) { if (j == 1) continue outer; if (i == 2) break outer; s += i + '' + j; } } s", "0010");
    eq("var s = ''; for (var k in {a: 1, b: 2}) s += k; s", "ab");
    eq("var s = ''; for (var k in [7, 8]) s += k; s", "01");
    eq("var o = Object.create({inh: 1}); o.own = 2; var s = []; for (var k in o) s.push(k); s.join()", "own,inh");
    eq("var s = 0; for (const v of [1, 2, 3]) s += v; s", "6");
    eq("var s = ''; for (const [k, v] of Object.entries({a: 1})) s += k + v; s", "a1");
    eq("switch (2) { case 1: 'one'; break; case 2: 'two'; case 3: 'three'; break; default: 'd' }", "three");
    eq("var r; switch ('x') { default: r = 'd'; break; case 'y': r = 'y'; } r", "d");
    eq("var a = 1, b = 2; [a, b] = [b, a]; a * 10 + b", "21");
    eq("var {p, q = 5, ...rest} = {p: 1, r: 2, s: 3}; p + q + Object.keys(rest).join()", "6r,s");
    eq("var [x, , y = 3, ...z] = [1, 2, undefined, 4, 5]; [x, y, z.length].join()", "1,3,2");
    eq("var {a: {b: [c]}} = {a: {b: [42]}}; c", "42");
    eq("function f({x = 1, y} = {}) { return x + (y || 0); } f() + f({x: 5, y: 1})", "7");
    eq("var o = {}; ({a: o.p, b: o['q']} = {a: 1, b: 2}); o.p + o.q", "3");
    eq("delete globalThis.zz; typeof zz", "undefined");
    eq("var o = {a: 1}; delete o.a; 'a' in o", "false");
    eq("'x' in {x: undefined}", "true");
    eq("void 0", "undefined");
    eq("(1, 2, 3)", "3");
    eq("var a = null; a ?? 'd'", "d");
    eq("var a = 0; a ?? 'd'", "0");
    eq("var a = null; a ||= 5; a", "5");
    eq("var a = 1; a &&= 7; a", "7");
    eq("var a = null; a ??= 3; a ??= 9; a", "3");
    eq("var o = {}; o.x ??= 4; o.x", "4");
    eq("var o = {a: {b: {c: 1}}}; o?.a?.b?.c", "1");
    eq("var o = null; o?.a.b.c", "undefined");
    eq("var o = {}; o.f?.()", "undefined");
    eq("var o = {f() { return 2; }}; o.f?.()", "2");
    eq("var o = {a: null}; o.a?.[0]", "undefined");
    eq("var i = 0; var o = {}; o[i++] = i++; o[0] + ':' + i", "1:2");
    eq("var x = 5; x++ + ++x", "12");
    eq("var o = {v: 1}; o.v++; o.v--; ++o.v; o.v", "2");
    eq("var a = [1]; a[0] += 2; a[0]", "3");
    eq("label: { break label; }; 'ok'", "ok");
}

#[test]
fn functions_closures_this() {
    eq("var add = function(a, b) { return a + b; }; add(2, 3)", "5");
    eq("(function fact(n) { return n <= 1 ? 1 : n * fact(n - 1); })(5)", "120");
    eq("var f = function g() { return typeof g; }; f() + typeof g", "functionundefined");
    eq("((a, b) => a * b)(3, 4)", "12");
    eq("(x => x + 1)(1)", "2");
    eq("(() => ({a: 1}))().a", "1");
    eq("function f() {} f.name + f.length + typeof f.prototype", "f0object");
    eq("var f = function() {}; f.name", "f");
    eq("var o = {m() {}, ['c' + 1]: () => {}}; o.m.name + o.c1.name", "mc1");
    eq("(() => {}).name", "");
    eq("function f(a, b) {} f.length", "2");
    eq("function f(a, b = 1, c) {} f.length", "1");
    eq("Math.max.apply(null, [1, 5, 3])", "5");
    eq("function f() { return this.x; } f.call({x: 1}) + f.apply({x: 2}, [])", "3");
    eq("function f(a, b) { return this.x + a + b; } var g = f.bind({x: 1}, 2); g(3) + g.name.length", "13");
    eq("function F(x) { this.x = x; } var b = F.bind(null, 1); new b().x", "1");
    eq("var o = {x: 1, get y() { return this.x + 1; }, set y(v) { this.x = v; }}; o.y = 10; o.y", "11");
    eq("var o = {}; Object.defineProperty(o, 'p', {get() { return 42; }}); o.p", "42");
    eq("var f = new Function('a', 'b', 'return a + b'); f(1, 2)", "3");
    eq("eval('1 + 1')", "2");
    eq("var z = 5; eval('z + 1')", "6");
    eq("(0, eval)('typeof globalThis')", "object");
    eq("(function() { return new.target === undefined; })()", "true");
    eq("function F() { return new.target === F; } new F() instanceof F", "true");
    eq("function F() { this.a = 1; return {b: 2}; } new F().b", "2");
    eq("function F() { this.a = 1; return 5; } new F().a", "1");
    eq("function F() {} F.prototype.m = function() { return 'm'; }; new F().m()", "m");
    throws("var f = () => {}; new f()", "is not a constructor");
    throws("null()", "null is not a function");
    throws("var o = {}; o.f()", "o.f is not a function");
    throws("undefined.x", "Cannot read properties of undefined (reading 'x')");
    throws("null.x = 1", "Cannot set properties of null (setting 'x')");
    eq("function f() { return f.caller === undefined; } typeof f()", "boolean");
    eq("var fib = n => n < 2 ? n : fib(n - 1) + fib(n - 2); fib(15)", "610");
    throws("function r() { return r(); } r()", "Maximum call stack size exceeded");
    eq("try { (function r() { r(); })(); } catch (e) { e instanceof RangeError }", "true");
    eq("function f() { 'use strict'; return typeof this; } f()", "undefined");
    eq("function f() { return typeof this; } f.call(5)", "object");
    eq("function f() { 'use strict'; return typeof this; } f.call(5)", "number");
}

#[test]
fn objects_and_prototypes() {
    eq("var o = {a: 1, b: 2}; Object.keys(o).join() + Object.values(o).join()", "a,b1,2");
    eq("var o = {2: 'b', 1: 'a', x: 'c'}; Object.keys(o).join()", "1,2,x");
    eq("var p = {greet() { return 'hi ' + this.name; }}; var o = Object.create(p); o.name = 'bob'; o.greet()", "hi bob");
    eq("var o = Object.create(null); typeof o.toString", "undefined");
    eq("({}).toString()", "[object Object]");
    eq("Object.prototype.toString.call([])", "[object Array]");
    eq("Object.prototype.toString.call(null)", "[object Null]");
    eq("var o = {}; o[Symbol.toStringTag] = 'Custom'; String(o)", "[object Custom]");
    eq("JSON.stringify(Object.assign({}, {a: 1}, {b: 2}))", "{\"a\":1,\"b\":2}");
    eq("var o = {...{a: 1}, b: 2, ...null}; JSON.stringify(o)", "{\"a\":1,\"b\":2}");
    eq("var o = Object.freeze({a: 1}); o.a = 2; o.b = 3; o.a + (o.b === undefined) + Object.isFrozen(o)", "3");
    throws("'use strict'; var o = Object.freeze({a: 1}); o.a = 2;", "read only");
    eq("var o = {}; Object.defineProperty(o, 'x', {value: 1, writable: false}); o.x = 2; o.x", "1");
    throws("var o = {}; Object.defineProperty(o, 'x', {value: 1}); Object.defineProperty(o, 'x', {value: 2});", "Cannot redefine property: x");
    eq("var o = {}; Object.defineProperty(o, 'x', {value: 1, enumerable: false}); Object.keys(o).length + ':' + o.x + ':' + JSON.stringify(Object.getOwnPropertyDescriptor(o, 'x'))", "0:1:{\"value\":1,\"writable\":false,\"enumerable\":false,\"configurable\":false}");
    eq("var o = {a: 1}; o.hasOwnProperty('a') + ':' + o.hasOwnProperty('toString') + ':' + ('toString' in o)", "true:false:true");
    eq("Object.getPrototypeOf([]) === Array.prototype", "true");
    eq("var o = {}; Object.setPrototypeOf(o, Array.prototype); o instanceof Array", "true");
    eq("[] instanceof Object", "true");
    eq("Object.entries({a: 1}).flat().join()", "a,1");
    eq("Object.fromEntries([['a', 1], ['b', 2]]).b", "2");
    eq("var o = {__proto__: {inh: 1}}; o.inh", "1");
    eq("var o = {}; o.__proto__ = {z: 9}; o.z", "9");
    eq("var o = {a: 1}; var {a, ...r} = o; a + Object.keys(r).length", "1");
    eq("JSON.stringify(Object.getOwnPropertyNames({b: 1, a: 2}))", "[\"b\",\"a\"]");
    eq("var s = Symbol('k'); var o = {[s]: 1, x: 2}; Object.getOwnPropertySymbols(o).length + Object.keys(o).length", "2");
    eq("var o = {}; o[1] = 'a'; o['1'] === 'a'", "true");
    eq("var o = {toString() { return 'T'; }}; `${o}` + (o + '')", "TT");
    eq("var o = {valueOf() { return 42; }}; o + 1", "43");
    eq("var o = {[Symbol.toPrimitive](h) { return h; }}; `${o}` + (+o) + (o + '')", "stringNaNdefault");
    eq("Object.is(NaN, NaN)", "true");
    eq("var p = new Proxy({}, {get(t, k) { return k.toString().toUpperCase(); }}); p.abc", "ABC");
    eq("var p = new Proxy({a: 1}, {has() { return true; }}); 'zzz' in p", "true");
    eq("Reflect.ownKeys({a: 1, [Symbol.iterator]: 2}).length", "2");
    eq("Reflect.has({x: 1}, 'x')", "true");
    eq("Reflect.apply(Math.max, null, [1, 3, 2])", "3");
    eq("class A {} Reflect.construct(A, []) instanceof A", "true");
}

#[test]
fn classes() {
    eq("class A { constructor(x) { this.x = x; } get dbl() { return this.x * 2; } static make() { return new A(21); } } A.make().dbl", "42");
    eq("class A { m() { return 'A'; } } class B extends A { m() { return super.m() + 'B'; } } new B().m()", "AB");
    eq("class A { constructor() { this.a = 1; } } class B extends A { constructor() { super(); this.b = 2; } } var b = new B(); b.a + b.b", "3");
    eq("class A { constructor(v) { this.v = v; } } class B extends A {} new B(7).v", "7");
    eq("class A { static s = 1; static m() { return this.s; } } class B extends A {} B.m()", "1");
    eq("class A { x = 1; y = this.x + 1; #p = 3; getP() { return this.#p; } hasP(o) { return #p in o; } } var a = new A(); a.y + a.getP() + a.hasP(a) + a.hasP({})", "6");
    eq("class A { #m() { return 'pm'; } call() { return this.#m(); } } new A().call()", "pm");
    eq("class A { static #count = 0; static inc() { return ++A.#count; } } A.inc(); A.inc()", "2");
    eq("class A { static { A.ready = true; } } A.ready", "true");
    eq("class A { ['comp' + 'uted']() { return 1; } } new A().computed()", "1");
    eq("class A {} typeof A + (A.name) + (A.prototype.constructor === A)", "functionAtrue");
    throws("class A {} A()", "Class constructor A cannot be invoked without 'new'");
    throws("class A extends Object { constructor() { this.x = 1; } } new A()", "Must call super constructor");
    eq("class A { toString() { return 'custom'; } } `${new A()}`", "custom");
    eq("var A = class Named {}; A.name", "Named");
    eq("var A = class {}; A.name", "A");
    eq("class A { set v(x) { this._v = x * 2; } get v() { return this._v; } } var a = new A(); a.v = 2; a.v", "4");
    eq("class A extends Array {} var a = new A(); a.push(1); a.length + (a instanceof Array) + (a instanceof A)", "3");
    eq("class E extends Error { constructor(m) { super(m); this.name = 'E'; } } try { throw new E('boom'); } catch (e) { e.message + (e instanceof Error) + e.name }", "boomtrueE");
    eq("class A { static x = 1; static y = A.x + 1; } A.y", "2");
    eq("class A { m() { return 1; } } Object.keys(A.prototype).length", "0");
    eq("class A { constructor() { return {z: 1}; } } new A().z", "1");
    eq("class A { get [Symbol.toStringTag]() { return 'Tagged'; } } String(new A())", "[object Tagged]");
    eq("class A { static [Symbol.hasInstance](v) { return v === 1; } } 1 instanceof A", "true");
    eq("class Animal { speak() { return `${this.name} makes a sound`; } constructor(n) { this.name = n; } } class Dog extends Animal { speak() { return super.speak() + ' (woof)'; } } new Dog('Rex').speak()", "Rex makes a sound (woof)");
    eq("const o = { base() { return 'b'; } }; const d = { __proto__: o, f() { return super.base() + 'd'; } }; d.f()", "bd");
}

#[test]
fn arrays() {
    eq("[1, 2, 3].map(x => x * 2).join()", "2,4,6");
    eq("[1, 2, 3, 4].filter(x => x % 2).join()", "1,3");
    eq("[1, 2, 3].reduce((a, b) => a + b)", "6");
    eq("[1, 2, 3].reduce((a, b) => a + b, 10)", "16");
    eq("[[1, [2]], 3].flat(Infinity).join()", "1,2,3");
    eq("[3, 1, 2].sort().join()", "1,2,3");
    eq("[10, 9, 1].sort().join()", "1,10,9");
    eq("[10, 9, 1].sort((a, b) => a - b).join()", "1,9,10");
    eq("var a = [1, 2, 3, 4, 5]; a.splice(1, 2, 'x').join() + '|' + a.join()", "2,3|1,x,4,5");
    eq("[1, 2, 3].slice(-2).join()", "2,3");
    eq("[1, 2, 3].indexOf(2) + [1, 2, 3].lastIndexOf(3) + [NaN].includes(NaN)", "4");
    eq("[1, 2, 3].find(x => x > 1) + [1, 2, 3].findIndex(x => x > 1) + [1, 2, 3].findLast(x => x < 3)", "5");
    eq("Array.from({length: 3}, (_, i) => i * i).join()", "0,1,4");
    eq("Array.from('abc').join('-')", "a-b-c");
    eq("Array.of(7).length + Array(3).length + [,].length", "5");
    eq("Array.isArray([]) + ':' + Array.isArray({length: 0})", "true:false");
    eq("var a = [1, 2, 3]; a.length = 1; a.join() + a.length", "11");
    eq("var a = []; a[5] = 1; a.length", "6");
    eq("var a = [1, , 3]; a.map(x => 1).length + ':' + (1 in a) + ':' + a.join()", "3:false:1,,3");
    eq("[1, 2, 3].reverse().join()", "3,2,1");
    eq("var a = [1, 2]; a.push(3, 4); a.unshift(0); a.pop() + a.shift() + a.length", "7");
    eq("[1, [2, 3]].toString()", "1,2,3");
    eq("String([1, [2, [3]]])", "1,2,3");
    eq("[1, 2, 3].at(-1)", "3");
    eq("[...[1, 2], ...'ab', ...[3]].join('')", "12ab3");
    eq("Math.max(...[1, 5, 2])", "5");
    eq("var a = [1, 2, 3]; var [x, ...rest] = a; rest.length", "2");
    eq("[1, 2, 3].some(x => x > 2) + ':' + [1, 2, 3].every(x => x > 0)", "true:true");
    eq("[3, 1, 2].toSorted().join() + [3, 1, 2].join()", "1,2,33,1,2");
    eq("[1, 2, 3].with(0, 9).join()", "9,2,3");
    eq("var a = [1, 2, 3]; var s = 0; a.forEach((v, i, arr) => s += v * i + arr.length); s", "17");
    eq("[].concat([1], 2, [[3]]).length", "3");
    eq("var a = ['b', 'a']; a.sort(); a[0]", "a");
    eq("[5, 1, 4].sort((a, b) => b - a).join()", "5,4,1");
    eq("var a = Array.from({length: 50}, (_, i) => (i * 7919) % 101); a.sort((x, y) => x - y); a.every((v, i) => i == 0 || a[i-1] <= v)", "true");
    eq("[1, 2, 3].entries().next().value.join()", "0,1");
    eq("var it = [1, 2][Symbol.iterator](); it.next().value + it.next().value + it.next().done", "4");
    eq("[1, 2, 3].fill(0, 1).join()", "1,0,0");
    eq("[1, 2, 3].flatMap(x => [x, x]).length", "6");
    eq("var a = [1, 2, 3]; a.copyWithin(0, 1).join()", "2,3,3");
    eq("Array.prototype.map.call('ab', c => c + c).join()", "aa,bb");
    eq("var arrLike = {0: 'a', 1: 'b', length: 2}; Array.prototype.join.call(arrLike, '+')", "a+b");
    eq("var a = [1, 2, 3]; a.length = 0; a[0]", "undefined");
    eq("[0, 1].findLastIndex(x => x == 0)", "0");
    eq("JSON.stringify([1, 'a', null, undefined, () => 1, {}])", "[1,\"a\",null,null,null,{}]");
}

#[test]
fn generators() {
    eq("function* g() { yield 1; yield 2; return 3; } var it = g(); JSON.stringify([it.next(), it.next(), it.next(), it.next()])", "[{\"value\":1,\"done\":false},{\"value\":2,\"done\":false},{\"value\":3,\"done\":true},{\"done\":true}]");
    eq("function* fib() { let [a, b] = [0, 1]; for (;;) { yield a; [a, b] = [b, a + b]; } } var out = []; for (const v of fib()) { if (v > 50) break; out.push(v); } out.join()", "0,1,1,2,3,5,8,13,21,34");
    eq("function* inner() { yield 'a'; yield 'b'; return 'r'; } function* outer() { const r = yield* inner(); yield r; } [...outer()].join()", "a,b,r");
    eq("function* g() { const x = yield 1; yield x * 2; } var it = g(); it.next(); it.next(21).value", "42");
    eq("function* g() { try { yield 1; } catch (e) { yield 'caught ' + e; } } var it = g(); it.next(); it.throw('E').value", "caught E");
    eq("function* g() { yield 1; yield 2; } var it = g(); it.next(); JSON.stringify(it.return(9)) + JSON.stringify(it.next())", "{\"value\":9,\"done\":true}{\"done\":true}");
    eq("function* g() { yield 1; } var it = g(); typeof it[Symbol.iterator] + (it[Symbol.iterator]() === it)", "functiontrue");
    eq("var o = { *[Symbol.iterator]() { yield 1; yield 2; } }; [...o].join()", "1,2");
    eq("class C { *gen() { yield this.v; } constructor() { this.v = 'cv'; } } [...new C().gen()].join()", "cv");
    eq("function* g() { yield* [1, 2]; yield* 'ab'; } [...g()].join('')", "12ab");
    eq("var it = { i: 0, next() { return {value: this.i, done: this.i++ > 2}; }, [Symbol.iterator]() { return this; } }; [...it].join()", "0,1,2");
    eq("var closed = false; var it = { [Symbol.iterator]() { return { next() { return {value: 1, done: false}; }, return() { closed = true; return {}; } }; } }; for (const v of it) break; closed", "true");
    eq("var closed = false; var it = { [Symbol.iterator]() { return { next() { return {value: 1, done: false}; }, return() { closed = true; return {}; } }; } }; function f() { for (const v of it) return 'r'; } f() + closed", "rtrue");
    eq("var closed = 0; var it = { [Symbol.iterator]() { return { next() { return {value: 1, done: false}; }, return() { closed++; return {}; } }; } }; try { for (const v of it) throw 'x'; } catch (e) {} closed", "1");
    eq("var [a, b] = (function*() { yield 1; yield 2; yield 3; })(); a + b", "3");
    eq("function* g() { var x = 0; while (true) { x = yield x + 1; if (x === null) return 'end'; } } var it = g(); it.next(); it.next(5).value + it.next(null).value", "6end");
    eq("Object.prototype.toString.call((function*(){})())", "[object Generator]");
}

#[test]
fn async_and_promises() {
    assert_eq!(logs("Promise.resolve(1).then(v => console.log('then', v)); console.log('sync');"), "sync|then 1");
    assert_eq!(logs("(async () => { console.log('a'); await null; console.log('c'); })(); console.log('b');"), "a|b|c");
    assert_eq!(logs("async function f() { return 5; } f().then(v => console.log(v));"), "5");
    assert_eq!(logs("async function f() { throw new Error('bad'); } f().catch(e => console.log(e.message));"), "bad");
    assert_eq!(logs("async function f() { try { await Promise.reject('r'); } catch (e) { console.log('caught', e); } return 'done'; } f().then(console.log);"), "caught r|done");
    assert_eq!(logs("Promise.resolve().then(() => console.log(1)); (async () => { await undefined; console.log(2); })(); Promise.resolve().then(() => console.log(3));"), "1|2|3");
    assert_eq!(logs("new Promise(r => setTimeoutLike(r)).then(() => console.log('never')); function setTimeoutLike() {} console.log('x');"), "x");
    assert_eq!(logs("Promise.all([1, Promise.resolve(2), new Promise(r => r(3))]).then(v => console.log(v.join()));"), "1,2,3");
    assert_eq!(logs("Promise.race([new Promise(() => {}), Promise.resolve('fast')]).then(console.log);"), "fast");
    assert_eq!(logs("Promise.allSettled([Promise.reject('e'), 1]).then(r => console.log(r.map(x => x.status).join()));"), "rejected,fulfilled");
    assert_eq!(logs("Promise.any([Promise.reject(1), Promise.resolve(2)]).then(console.log);"), "2");
    assert_eq!(logs("Promise.reject('x').finally(() => console.log('f')).catch(e => console.log('c', e));"), "f|c x");
    assert_eq!(logs("var p = Promise.resolve(1); p.then(v => { throw 'boom'; }).then(() => console.log('no'), e => console.log('err', e));"), "err boom");
    assert_eq!(logs("var thenable = { then(res) { res('T'); } }; Promise.resolve(thenable).then(console.log);"), "T");
    assert_eq!(logs("(async () => { const v = await { then(r) { r('aw'); } }; console.log(v); })();"), "aw");
    assert_eq!(logs("async function* ag() { yield 1; yield 2; } (async () => { for await (const v of ag()) console.log(v); console.log('end'); })();"), "1|2|end");
    assert_eq!(logs("(async () => { for await (const v of [Promise.resolve('p'), 'q']) console.log(v); })();"), "p|q");
    assert_eq!(logs("async function* ag() { const x = yield 1; console.log('got', x); } const it = ag(); it.next().then(r => { console.log(r.value); return it.next('X'); }).then(r => console.log(r.done));"), "1|got X|true");
    assert_eq!(logs("const order = []; const p = new Promise(res => { order.push('exec'); res(); }); p.then(() => order.push('then')); order.push('after'); p.then(() => console.log(order.join()));"), "exec,after,then");
    assert_eq!(logs("(async function() { const a = await Promise.all([(async () => 1)(), (async () => { await null; return 2; })()]); console.log(a.join()); })();"), "1,2");
    assert_eq!(logs("queueMicrotask(() => console.log('m')); console.log('s');"), "s|m");
    assert_eq!(logs("class P extends Promise {} P.resolve(3).then(v => console.log(v, P.resolve(1) instanceof P));"), "3 true");
    assert_eq!(logs("async function f() { return await 'v'; } f().then(v => console.log(typeof f, v));"), "function v");
    assert_eq!(logs("const { promise, resolve } = Promise.withResolvers(); promise.then(console.log); resolve('wr');"), "wr");
    assert_eq!(logs("Promise.resolve(1).then(2).then(console.log);"), "1");
    assert_eq!(logs("async function a() { await b(); console.log('a2'); } async function b() { console.log('b1'); } a(); console.log('main');"), "b1|main|a2");
}

#[test]
fn exceptions_and_finally() {
    eq("try { throw new TypeError('t'); } catch (e) { e.name + ':' + e.message + ':' + (e instanceof TypeError) + (e instanceof Error) }", "TypeError:t:truetrue");
    eq("function f() { try { return 'try'; } finally { 'fin'; } } f()", "try");
    eq("function f() { try { return 'try'; } finally { return 'finally'; } } f()", "finally");
    eq("var log = []; function f() { try { log.push('t'); return 1; } finally { log.push('f'); } } f(); log.join()", "t,f");
    eq("var log = []; for (var i = 0; i < 3; i++) { try { if (i == 1) continue; if (i == 2) break; log.push(i); } finally { log.push('f' + i); } } log.join()", "0,f0,f1,f2");
    eq("var r; try { try { throw 1; } finally { r = 'fin'; } } catch (e) { r += e; } r", "fin1");
    eq("try { try { throw 'a'; } catch (e) { throw 'b'; } finally { } } catch (e) { e }", "b");
    eq("var r = ''; try { try { throw 'a'; } finally { r += 'f'; } } catch (e) { r += e; } r", "fa");
    eq("try { null.x } catch ({message}) { message.slice(0, 22) }", "Cannot read properties");
    eq("try { throw {custom: 1}; } catch (e) { e.custom }", "1");
    eq("try { throw 5; } catch { 'nobinding' }", "nobinding");
    eq("var e = new Error('m', {cause: 'c'}); e.cause", "c");
    eq("new Error('x').toString()", "Error: x");
    eq("String(new RangeError('r'))", "RangeError: r");
    eq("function named() { throw new Error('s'); } try { named(); } catch (e) { e.stack.includes('named') }", "true");
    eq("try { undefinedFn(); } catch (e) { e.constructor === ReferenceError }", "true");
    eq("try { JSON.parse('{bad'); } catch (e) { e instanceof SyntaxError }", "true");
    eq("try { eval('var ='); } catch (e) { e.name }", "SyntaxError");
    eq("var f = () => { throw 'arrow'; }; try { f(); } catch (e) { e }", "arrow");
    eq("function* g() { try { yield 1; } finally { globalThis.gf = 'ran'; } } var it = g(); it.next(); it.next(); gf", "ran");
    eq("var r = []; function f() { for (const x of [1, 2]) { try { if (x == 1) continue; return 'ret'; } finally { r.push('f' + x); } } } f() + r.join()", "retf1,f2");
    eq("var d = 0; function f() { try { throw 1; } catch (e) { try { throw 2; } catch (e2) { d = e + e2; } } } f(); d", "3");
    eq("var r = 'none'; outer: { try { break outer; } finally { r = 'fin'; } } r", "fin");
    throws("throw new Error('plain')", "Error: plain");
    throws("[].reduce((a, b) => a)", "Reduce of empty array with no initial value");
}

#[test]
fn json_math_numbers() {
    eq("JSON.stringify({a: [1, {b: 2}], c: 'x', d: undefined, e: null})", "{\"a\":[1,{\"b\":2}],\"c\":\"x\",\"e\":null}");
    eq("JSON.stringify({a: 1, b: [1, 2]}, null, 2)", "{\n  \"a\": 1,\n  \"b\": [\n    1,\n    2\n  ]\n}");
    eq("JSON.stringify('he said \"hi\"\\n')", "\"he said \\\"hi\\\"\\n\"");
    eq("JSON.stringify({toJSON() { return 'tj'; }})", "\"tj\"");
    eq("JSON.stringify({a: 1, b: 2}, ['b'])", "{\"b\":2}");
    eq("JSON.stringify({a: 1, b: 2}, (k, v) => typeof v === 'number' ? v * 2 : v)", "{\"a\":2,\"b\":4}");
    eq("JSON.parse('{\"a\":[1,2,{\"b\":true}],\"c\":null}').a[2].b", "true");
    eq("JSON.parse('\"\\\\u0041\"')", "A");
    eq("JSON.parse('[1,2]', (k, v) => typeof v === 'number' ? v + 1 : v).join()", "2,3");
    eq("JSON.parse(' 1e3 ')", "1000");
    throws("JSON.parse('')", "Unexpected end of JSON input");
    throws("JSON.parse('{\"a\":1,}')", "SyntaxError");
    throws("var o = {}; o.self = o; JSON.stringify(o)", "circular");
    throws("JSON.stringify(1n)", "BigInt");
    eq("Math.max(1, 2, 3) + Math.min(4, 5) + Math.abs(-1) + Math.floor(1.9) + Math.ceil(1.1) + Math.round(2.5) + Math.round(-2.5)", "12");
    eq("Math.sqrt(16) + Math.pow(2, 3) + Math.trunc(-1.7) + Math.sign(-3)", "10");
    eq("Math.max() + ':' + Math.min()", "-Infinity:Infinity");
    eq("Math.hypot(3, 4)", "5");
    eq("Math.round(0.49) + Math.round(-0.5) + Math.round(1.5)", "2");
    eq("typeof Math.random() === 'number' && Math.random() < 1", "true");
    eq("Math.PI.toFixed(5)", "3.14159");
    eq("(0.1).toString()", "0.1");
    eq("(1e21).toString()", "1e+21");
    eq("(123456789).toString()", "123456789");
    eq("(255).toString(16) + (255).toString(2)", "ff11111111");
    eq("(0.5).toString(2)", "0.1");
    eq("(1.005).toFixed(2)", "1.00");
    eq("(2.5).toFixed(0) + (1.45).toFixed(1) + (0).toFixed(2)", "31.40.00");
    eq("(1234.5678).toFixed(2)", "1234.57");
    eq("(1e21).toFixed(2)", "1e+21");
    eq("(123.456).toPrecision(4) + '|' + (0.000123).toPrecision(2) + '|' + (123456).toPrecision(2)", "123.5|0.00012|1.2e+5");
    eq("(12345).toExponential(2) + '|' + (0).toExponential()", "1.23e+4|0e+0");
    eq("Number('12px') + ':' + Number('') + ':' + Number(' 12 ') + ':' + Number('0x10') + ':' + Number(null) + ':' + Number([5]) + ':' + Number(true)", "NaN:0:12:16:0:5:1");
    eq("parseInt('12px') + parseInt('0x1f') + parseInt('101', 2) + parseInt('  -7 ')", "41");
    eq("parseFloat('3.14abc') + parseFloat('.5') + parseFloat('-1e2x')", "-96.36");
    eq("isNaN('abc') + ':' + isNaN('12') + ':' + Number.isNaN('abc') + ':' + isFinite('1')", "true:false:false:true");
    eq("Number.isInteger(5.0) + ':' + Number.isSafeInteger(2 ** 53) + ':' + Number.MAX_SAFE_INTEGER", "true:false:9007199254740991");
    eq("(1234567.891).toLocaleString()", "1,234,567.891");
    eq("5..toString() + 5.5.toFixed(0)", "56");
    eq("0.1 * 3", "0.30000000000000004");
    eq("1e300 * 1e300", "Infinity");
    eq("9007199254740993", "9007199254740992");
    eq("123456789012345680000 + ''", "123456789012345680000");
    eq("(-1.5e-7).toString()", "-1.5e-7");
    eq("10n ** 20n + ''", "100000000000000000000");
    eq("(5n * 3n - 1n) + ''", "14");
    eq("BigInt(10) === 10n", "true");
    eq("typeof (1n + 2n)", "bigint");
    throws("1n + 1", "Cannot mix BigInt");
    eq("1n < 2", "true");
    eq("Number(9007199254740993n)", "9007199254740992");
    eq("new Number(5) + 1", "6");
    eq("typeof new Number(5)", "object");
    eq("new Boolean(false) ? 'truthy' : 'falsy'", "truthy");
}

#[test]
fn symbols_iterators_misc() {
    eq("var s1 = Symbol('a'), s2 = Symbol('a'); (s1 === s2) + ':' + (Symbol.for('k') === Symbol.for('k')) + ':' + Symbol.keyFor(Symbol.for('k')) + ':' + s1.description + ':' + s1.toString()", "false:true:k:a:Symbol(a)");
    throws("Symbol() + ''", "Cannot convert a Symbol value to a string");
    eq("var o = {[Symbol.iterator]: function*() { yield 'i'; }}; [...o][0]", "i");
    eq("typeof Symbol.iterator + typeof Symbol.asyncIterator", "symbolsymbol");
    eq("Object.keys(globalThis).length >= 0", "true");
    eq("[1, 2, 3].keys().next().value", "0");
    eq("Array.prototype[Symbol.iterator] === Array.prototype.values", "true");
    eq("new String('ab')[1] + new String('ab').length", "b2");
    eq("Object.prototype.toString.call(function*(){})", "[object GeneratorFunction]");
    eq("Object.prototype.toString.call(async function(){})", "[object AsyncFunction]");
    eq("String(Symbol.iterator)", "Symbol(Symbol.iterator)");
    eq("var tag = (s, ...v) => s.length + ':' + v.length; tag`a${1}b${2}c`", "3:2");
    eq("var s = 'x'; var t = `${s}${s}`; t", "xx");
    eq("structuredClone({a: [1, {b: 2}]}).a[1].b", "2");
    eq("globalThis === this", "true");
    eq("var self = globalThis; self.q = 1; q", "1");
    eq("'use strict'; var f = function() { return this; }; f()", "undefined");
    eq("escape('a b') + unescape('%41')", "a%20bA");
    eq("[1,2,3].includes(2) && 'abc'.includes('b') ? 'y' : 'n'", "y");
    eq("new Array(5).fill().map((_, i) => i).join('')", "01234");
    eq("Array.prototype.slice.call({0: 'a', length: 1})[0]", "a");
    eq("(function() { return Array.prototype.slice.call(arguments, 1).join(); })(1, 2, 3)", "2,3");
    eq("var p = new Proxy(function() { return 'called'; }, { apply(t, th, a) { return t() + a.length; } }); p(1, 2)", "called2");
    eq("Reflect.getPrototypeOf(Object.create(null))", "null");
    eq("var o = {a: 1}; with_ = typeof o; with_", "object");
}

#[test]
fn small_program() {
    let src = r#"
        // A tiny expression tokenizer + recursive descent evaluator.
        function tokenize(src) {
            const tokens = [];
            const re = /\s*(?:(\d+\.?\d*)|(.))/g;
            let m;
            while ((m = re.exec(src)) !== null && m[0] !== '') {
                if (m[1] !== undefined) tokens.push({ type: 'num', value: parseFloat(m[1]) });
                else tokens.push({ type: 'op', value: m[2] });
            }
            return tokens;
        }
        class Parser {
            constructor(tokens) { this.tokens = tokens; this.pos = 0; }
            peek() { return this.tokens[this.pos]; }
            next() { return this.tokens[this.pos++]; }
            expr() {
                let v = this.term();
                while (this.peek() && (this.peek().value === '+' || this.peek().value === '-')) {
                    const op = this.next().value;
                    const r = this.term();
                    v = op === '+' ? v + r : v - r;
                }
                return v;
            }
            term() {
                let v = this.factor();
                while (this.peek() && (this.peek().value === '*' || this.peek().value === '/')) {
                    const op = this.next().value;
                    const r = this.factor();
                    v = op === '*' ? v * r : v / r;
                }
                return v;
            }
            factor() {
                const t = this.next();
                if (t.type === 'num') return t.value;
                if (t.value === '(') { const v = this.expr(); this.next(); return v; }
                if (t.value === '-') return -this.factor();
                throw new SyntaxError('unexpected ' + t.value);
            }
        }
        const results = ['1 + 2 * 3', '(1 + 2) * 3', '10 / 4 - -1', '2 * (3 + 4) * 5'].map(s => new Parser(tokenize(s)).expr());
        results.join(',')
    "#;
    eq(src, "7,9,3.5,70");
}

#[test]
fn performance_loop() {
    let start = std::time::Instant::now();
    eq("let s = 0; for (let i = 0; i < 1000000; i++) s += i; s", "499999500000");
    let elapsed = start.elapsed();
    // Debug builds are much slower; just make sure it finishes in reasonable time.
    assert!(elapsed.as_secs() < 30, "loop took {:?}", elapsed);
}

#[test]
fn gc_survives_allocation_churn() {
    let mut realm = Realm::new();
    realm.gc_threshold = 1000;
    let r = realm.eval("var keep = []; for (let i = 0; i < 20000; i++) { const o = {i, arr: [i, i + 1], s: 'x' + i}; if (i % 1000 == 0) keep.push(o); } keep.map(o => o.arr[1]).join()", "gc.js");
    match r {
        Ok(Value::String(s)) => assert_eq!(s.to_rust_string(), "1,1001,2001,3001,4001,5001,6001,7001,8001,9001,10001,11001,12001,13001,14001,15001,16001,17001,18001,19001"),
        Ok(v) => panic!("unexpected {:?}", v),
        Err(e) => panic!("{}", realm.error_to_string(&e)),
    }
    let freed = realm.gc();
    assert!(freed > 0);
    // Values are still intact after an explicit collection.
    let v = realm.eval("keep.length + ':' + keep[3].s", "gc2.js").unwrap();
    assert_eq!(realm.to_string(v).unwrap().to_rust_string(), "20:x3000");
}

#[test]
fn host_embedding() {
    let mut realm = Realm::new();
    fn add(realm: &mut Realm, _this: Value, args: &[Value]) -> Result<Value, Value> {
        let a = realm.to_number(args.first().cloned().unwrap_or(Value::Undefined))?;
        let b = realm.to_number(args.get(1).cloned().unwrap_or(Value::Undefined))?;
        Ok(Value::Number(a + b))
    }
    let f = realm.new_native_function("add", 2, add);
    realm.define_global("add", Value::Object(f));
    let v = realm.eval("add(2, 3) + add.length + add.name.length", "host.js").unwrap();
    assert_eq!(v.as_number(), Some(10.0));
    // Host object with data + accessor
    struct Counter(i32);
    let o = realm.new_object();
    realm.set_host_data(o, Box::new(Counter(41)));
    fn get_count(realm: &mut Realm, this: Value, _args: &[Value]) -> Result<Value, Value> {
        let id = this.as_object().unwrap();
        let c = realm.host_data::<Counter>(id).map(|c| c.0).unwrap_or(0);
        Ok(Value::Number(c as f64))
    }
    realm.define_accessor(o, "count", Some(get_count), None, crate::heap::CONFIGURABLE);
    let root = realm.root(Value::Object(o));
    realm.define_global("counter", root.value());
    let v = realm.eval("counter.count + 1", "host2.js").unwrap();
    assert_eq!(v.as_number(), Some(42.0));
    // Calling back into JS from the host
    let cb = realm.eval("(x => x * 3)", "cb.js").unwrap();
    let r = realm.call(cb, Value::Undefined, &[Value::Number(5.0)]).unwrap();
    assert_eq!(r.as_number(), Some(15.0));
    // Host-driven promise
    let (p, resolve, _reject) = realm.new_promise();
    realm.define_global("hostPromise", Value::Object(p));
    realm.eval("hostPromise.then(v => globalThis.got = v)", "hp.js").unwrap();
    realm.call(Value::Object(resolve), Value::Undefined, &[Value::str("resolved")]).unwrap();
    realm.run_jobs();
    let got = realm.eval("got", "hp2.js").unwrap();
    assert_eq!(realm.to_string(got).unwrap().to_rust_string(), "resolved");
}
