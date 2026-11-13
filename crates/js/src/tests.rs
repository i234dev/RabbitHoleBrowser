//! Engine tests driven through `Realm::eval`.

use crate::realm::Realm;
use crate::value::V;
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
    // A string pattern is compiled to a RegExp and its @@match invoked.
    eq("'a/k=xyz/b'.match('/k=([^/]+)')[1] + '|' + 'abc'.match('b').index + '|' + 'abc'.match('z') + '|' + 'aXbX'.match('X', 'g')[0]", "xyz|1|null|X");
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
    // The value of a short-circuited logical assignment to a computed
    // member is the existing value (not the key).
    eq("var o = {k: 5}, k = 'k'; [o[k] ??= 7, o[k] ||= 8, o[k] &&= 0, o[k]].join()", "5,5,0,0");
    eq("var s = Symbol('a'), o = {}; var a = (o[s] ??= {n: 1}); var b = (o[s] ??= {n: 2}); [a === b, b.n].join()", "true,1");
    eq("var o = {k: 0}; [o['k'] &&= 9, o['k'] ||= 3].join()", "0,3");
    // Function.prototype.toString returns the source text (libraries
    // detect native ES classes with /\bclass\b/.test(fn.toString())).
    eq("String(class A extends Object { m() {} })", "class A extends Object { m() {} }");
    eq("(function f(a, b) { return a + b; }).toString()", "function f(a, b) { return a + b; }");
    eq("/\\bclass\\b/.test(class {}.toString())", "true");
    eq("function g(x) {\n  return x;\n}\ng.toString()", "function g(x) {\n  return x;\n}");
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
    match r.as_ref().map(Value::v) {
        Ok(V::String(s)) => assert_eq!(s.to_rust_string(), "1,1001,2001,3001,4001,5001,6001,7001,8001,9001,10001,11001,12001,13001,14001,15001,16001,17001,18001,19001"),
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

// ------------------------------------------------------------------ Date

#[test]
fn date_basics() {
    eq("new Date(Date.UTC(2026, 10, 9)).toISOString()", "2026-11-09T00:00:00.000Z");
    eq("Date.UTC(2026, 10, 9, 12, 30, 15, 250)", "1794227415250");
    eq("new Date('2026-11-09T12:30:15.250Z').getTime()", "1794227415250");
    eq("new Date('2026-11-09').getTime()", "1794182400000");
    eq("new Date(0).toISOString()", "1970-01-01T00:00:00.000Z");
    eq("new Date(-1).toISOString()", "1969-12-31T23:59:59.999Z");
    eq("new Date(8.64e15).toISOString()", "+275760-09-13T00:00:00.000Z");
    eq("isNaN(new Date(8.64e15 + 1).getTime())", "true");
    eq("new Date(NaN) + ''", "Invalid Date");
    eq("String(new Date('garbage'))", "Invalid Date");
    throws("new Date(NaN).toISOString()", "RangeError");
    eq("new Date(NaN).toJSON()", "null");
    eq("typeof Date()", "string");
    eq("Date.prototype.toUTCString.call(new Date(Date.UTC(1994, 10, 6, 8, 49, 37)))", "Sun, 06 Nov 1994 08:49:37 GMT");
    eq("new Date(Date.UTC(2024, 1, 29)).getUTCDate()", "29");
    eq("new Date(Date.UTC(2023, 1, 29)).getUTCMonth()", "2"); // overflow into March
    eq("new Date(Date.UTC(2026, 0, 1)).getUTCDay()", "4"); // Thursday
    eq("new Date(Date.UTC(-1, 0, 1)).getUTCFullYear()", "-1");
    eq("new Date(Date.UTC(99, 0, 1)).getUTCFullYear()", "1999");
    eq("new Date(2026, 0, 15, 12).getHours()", "12");
    eq("new Date(2026, 0, 15, 12).getFullYear() + '-' + new Date(2026, 0, 15, 12).getMonth() + '-' + new Date(2026, 0, 15, 12).getDate()", "2026-0-15");
    eq("new Date(2026, 5, 15).getTimezoneOffset() === -new Date(2026, 5, 15).getTimezoneOffset() * -1", "true");
    // Local round trip through setters.
    eq("var d = new Date(2026, 10, 9, 10, 20, 30); d.setMonth(0); d.getMonth()", "0");
    eq("var d = new Date(2026, 10, 9, 10, 20, 30); d.setDate(40); d.getMonth() + '/' + d.getDate()", "11/10");
    eq("var d = new Date(2026, 10, 9); d.setFullYear(2000, 1, 29); d.getDate()", "29");
    eq("var d = new Date(0); d.setUTCHours(25); d.toISOString()", "1970-01-02T01:00:00.000Z");
    eq("var d = new Date(NaN); d.setFullYear(2020); d.getFullYear()", "2020");
    eq("var d = new Date(2026, 0, 1); d.setYear(99); d.getFullYear()", "1999");
    eq("new Date(2026, 0, 1).getYear()", "126");
    eq("var d = new Date(0); d.setTime(1e12); d.getTime()", "1000000000000");
    eq("new Date(1e12).valueOf()", "1000000000000");
    eq("new Date(new Date(5)).getTime()", "5");
    eq("new Date('1995-12-17T03:24:00Z') - new Date('1995-12-17T03:23:00Z')", "60000");
    eq("`${new Date(0)}`.startsWith('Thu Jan 01 1970') || `${new Date(0)}`.startsWith('Wed Dec 31 1969')", "true");
    eq("new Date(0)[Symbol.toPrimitive]('number')", "0");
    eq("typeof new Date(0)[Symbol.toPrimitive]('default')", "string");
    eq("new Date(0) + 1 === new Date(0).toString() + '1'", "true");
    eq("Object.prototype.toString.call(new Date())", "[object Date]");
    eq("JSON.stringify({d: new Date(Date.UTC(2026, 0, 2))})", "{\"d\":\"2026-01-02T00:00:00.000Z\"}");
    eq("new Date(Date.UTC(2026, 10, 9, 15, 4, 5)).toISOString().length", "24");
    eq("/^\\d{2}:\\d{2}:\\d{2} GMT[+-]\\d{4} \\(.+\\)$/.test(new Date(0).toTimeString())", "true");
    eq("/^(Sun|Mon|Tue|Wed|Thu|Fri|Sat) (Jan|Dec) \\d{2} 19(69|70)$/.test(new Date(0).toDateString())", "true");
    // Distance from the TZif-derived offset to toString's GMT offset must agree.
    eq("(function(){ var d = new Date(2026, 6, 1); var m = /GMT([+-])(\\d{2})(\\d{2})/.exec(d.toString()); var off = (m[1] === '-' ? -1 : 1) * (+m[2] * 60 + +m[3]); return off === -d.getTimezoneOffset(); })()", "true");
}

#[test]
fn date_parsing_forms() {
    eq("Date.parse('2026')", "1767225600000");
    eq("Date.parse('2026-11')", "1793491200000");
    eq("Date.parse('2026-11-09T10:00:00+01:00')", "1794214800000");
    eq("Date.parse('2026-11-09T10:00:00.5Z')", "1794218400500");
    eq("Date.parse('2026-11-09T24:00:00Z')", "1794268800000");
    eq("isNaN(Date.parse('2026-13-01'))", "true");
    eq("isNaN(Date.parse('2026-11-09T25:00:00Z'))", "true");
    eq("Date.parse('Sun, 06 Nov 1994 08:49:37 GMT')", "784111777000");
    eq("Date.parse('Sunday, 06-Nov-94 08:49:37 GMT')", "784111777000");
    eq("Date.parse('Sun Nov  6 08:49:37 1994 UTC')", "784111777000");
    eq("Date.parse('Sun Nov 06 1994 08:49:37 GMT+0000 (Coordinated Universal Time)')", "784111777000");
    eq("Date.parse('Nov 6, 1994 08:49:37 GMT')", "784111777000");
    eq("Date.parse('6 Nov 1994 08:49:37 +0100')", "784108177000");
    eq("Date.parse('11/6/1994 8:49:37 AM GMT')", "784111777000");
    eq("Date.parse('11/6/1994 8:49:37 PM GMT')", "784154977000");
    eq("Date.parse('1994/11/06 08:49:37 GMT')", "784111777000");
    eq("Date.parse('November 6, 1994 UTC')", "784080000000");
    eq("Date.parse('6 November 1994 00:00 Z')", "784080000000");
    eq("Date.parse('Thu, 01 Jan 1970 00:00:00 GMT')", "0");
    eq("Date.parse('1970-01-01T00:00:00.000+00:00')", "0");
    eq("Date.parse('1970-01-01 00:00:00Z')", "0");
    eq("Date.parse('2024-02-29T12:00Z') - Date.parse('2024-02-28T12:00Z')", "86400000");
    // Local forms round-trip through the local getters.
    eq("new Date('11/9/2026').getDate()", "9");
    eq("new Date('11/9/2026 10:30').getHours()", "10");
    eq("new Date('Nov 9, 2026').getMonth()", "10");
    eq("new Date('2026-11-09T10:00:00').getHours()", "10");
    eq("new Date(Date.parse(new Date(2026, 10, 9, 10, 20, 30).toString())).getTime() === new Date(2026, 10, 9, 10, 20, 30).getTime()", "true");
    eq("new Date(new Date(1e12).toUTCString()).getTime()", "1000000000000");
    eq("new Date(new Date(1e12).toISOString()).getTime()", "1000000000000");
}

#[test]
fn date_time_zone_matches_tzif() {
    // The engine's TZif reader and the libc-free `date` helper must agree
    // with each other across DST (only meaningful outside UTC).
    let out = run("(function(){ var jan = new Date(2026, 0, 15).getTimezoneOffset(); var jul = new Date(2026, 6, 15).getTimezoneOffset(); return jan + ',' + jul; })()").unwrap();
    let parts: Vec<i64> = out.split(',').map(|p| p.parse().unwrap()).collect();
    let tz = crate::builtins::date::tz();
    let jan_off = tz.offset_at(crate::builtins::date::utc_from_local(crate::builtins::date::make_date(crate::builtins::date::make_day(2026.0, 0.0, 15.0), 0.0)) as i64 / 1000).0;
    assert_eq!(parts[0], -(jan_off as i64) / 60);
    if tz.name == "Europe/Madrid" {
        assert_eq!(parts, vec![-60, -120]);
        // DST starts 2026-03-29 at 02:00 local (01:00 UTC).
        eq("new Date(Date.UTC(2026, 2, 29, 0, 59)).getTimezoneOffset()", "-60");
        eq("new Date(Date.UTC(2026, 2, 29, 1, 0)).getTimezoneOffset()", "-120");
        eq("new Date(2026, 2, 29, 2, 30).getHours()", "3"); // skipped hour
        eq("new Date(2026, 6, 1).toString().includes('GMT+0200 (Central European Summer Time)')", "true");
        eq("new Date(2026, 0, 1).toString().includes('GMT+0100 (Central European Standard Time)')", "true");
    }
}

#[test]
fn date_locale_formatting() {
    eq("new Date(Date.UTC(2026, 10, 9, 10, 0, 0)).toLocaleString('en-US', {timeZone: 'UTC'})", "11/9/2026, 10:00:00 AM");
    eq("new Date(Date.UTC(2026, 10, 9, 22, 5, 9)).toLocaleTimeString('en-US', {timeZone: 'UTC'})", "10:05:09 PM");
    eq("new Date(Date.UTC(2026, 10, 9)).toLocaleDateString('en-US', {timeZone: 'UTC'})", "11/9/2026");
    eq("new Date(Date.UTC(2026, 10, 9)).toLocaleDateString('en-US', {year: 'numeric', month: 'long', day: 'numeric', timeZone: 'UTC'})", "November 9, 2026");
    eq("new Date(Date.UTC(2026, 10, 9)).toLocaleDateString('en-US', {weekday: 'long', year: 'numeric', month: 'long', day: 'numeric', timeZone: 'UTC'})", "Monday, November 9, 2026");
    eq("new Date(Date.UTC(2026, 10, 9)).toLocaleDateString('en-US', {month: 'short', day: 'numeric', timeZone: 'UTC'})", "Nov 9");
    eq("new Date(Date.UTC(2026, 10, 9)).toLocaleDateString('en-GB', {timeZone: 'UTC'})", "09/11/2026");
    eq("new Date(Date.UTC(2026, 10, 9, 10)).toLocaleString('es-ES', {timeZone: 'UTC'})", "9/11/2026, 10:00:00");
    eq("new Date(Date.UTC(2026, 10, 9)).toLocaleDateString('es', {weekday: 'long', day: 'numeric', month: 'long', year: 'numeric', timeZone: 'UTC'})", "lunes, 9 de noviembre de 2026");
    eq("new Date(Date.UTC(2026, 10, 9)).toLocaleDateString('de-DE', {timeZone: 'UTC'})", "9.11.2026");
    eq("new Date(Date.UTC(2026, 10, 9)).toLocaleDateString('de', {day: 'numeric', month: 'long', year: 'numeric', timeZone: 'UTC'})", "9. November 2026");
    eq("new Date(Date.UTC(2026, 10, 9, 15, 7)).toLocaleTimeString('en-US', {hour: '2-digit', minute: '2-digit', timeZone: 'UTC'})", "03:07 PM");
    eq("new Date(Date.UTC(2026, 10, 9, 15, 7)).toLocaleTimeString('en-US', {hour12: false, timeZone: 'UTC'})", "15:07:00");
    eq("new Date(Date.UTC(2026, 10, 9, 15, 7)).toLocaleString('en-US', {dateStyle: 'full', timeStyle: 'short', timeZone: 'UTC'})", "Monday, November 9, 2026 at 3:07 PM");
    eq("new Date(Date.UTC(2026, 10, 9)).toLocaleDateString('en-US', {dateStyle: 'medium', timeZone: 'UTC'})", "Nov 9, 2026");
    eq("new Date(Date.UTC(2026, 10, 9)).toLocaleDateString('ja-JP', {timeZone: 'UTC'})", "2026/11/9");
    eq("new Date(NaN).toLocaleString()", "Invalid Date");
    eq("new Intl.DateTimeFormat('en-US', {timeZone: 'UTC'}).format(new Date(Date.UTC(2026, 10, 9)))", "11/9/2026");
    eq("var f = new Intl.DateTimeFormat('en-US', {month: 'long', timeZone: 'UTC'}).format; [new Date(Date.UTC(2026, 0, 1))].map(f)[0]", "January");
    eq("new Intl.DateTimeFormat('en-US', {timeZone: 'UTC'}).resolvedOptions().timeZone", "UTC");
    eq("new Intl.DateTimeFormat('en-US', {timeZone: 'UTC', year: 'numeric', month: '2-digit'}).formatToParts(new Date(Date.UTC(2026, 10, 9))).map(p => p.type).join()", "month,literal,year");
    throws("new Date().toLocaleString('en-US', {timeZone: 'Mars/Olympus'})", "RangeError");
    if std::path::Path::new("/usr/share/zoneinfo/America/New_York").exists() {
        eq("new Date(Date.UTC(2026, 6, 4, 16, 0)).toLocaleString('en-US', {timeZone: 'America/New_York'})", "7/4/2026, 12:00:00 PM");
        eq("new Date(Date.UTC(2026, 0, 4, 16, 0)).toLocaleString('en-US', {timeZone: 'America/New_York', timeZoneName: 'short'})", "1/4/2026, 11:00:00 AM EST");
        eq("new Date(Date.UTC(2026, 0, 4, 16, 0)).toLocaleTimeString('ja-JP', {timeZone: 'Asia/Tokyo'})", "1:00:00");
        eq("new Intl.DateTimeFormat('en-US', {timeZone: 'Europe/Madrid'}).resolvedOptions().timeZone", "Europe/Madrid");
    }
}

// ----------------------------------------------------------- Collections

#[test]
fn maps_and_sets() {
    eq("var m = new Map([[1, 'a'], [2, 'b']]); m.set(3, 'c'); [m.size, m.get(2), m.has(4), m.delete(1), m.size].join()", "3,b,false,true,2");
    eq("var m = new Map(); m.set(NaN, 1); m.set(-0, 2); [m.get(NaN), m.get(0), m.has(+0), [...m.keys()][1] === 0 && 1 / [...m.keys()][1] === Infinity].join()", "1,2,true,true");
    eq("var m = new Map([['b', 1], ['a', 2]]); m.delete('b'); m.set('b', 3); [...m.keys()].join()", "a,b");
    eq("[...new Map([[1, 2]]).entries()][0].join()", "1,2");
    eq("var m = new Map([[1, 2], [3, 4]]); var out = []; m.forEach(function(v, k, mm){ out.push(k + ':' + v + ':' + (mm === m)); if (k === 1) { m.delete(3); m.set(5, 6); } }); out.join('|')", "1:2:true|5:6:true");
    eq("var m = new Map([[1, 1], [2, 2]]); var it = m.keys(); it.next(); m.clear(); m.set(9, 9); JSON.stringify([it.next(), it.next()])", "[{\"value\":9,\"done\":false},{\"done\":true}]");
    eq("Object.prototype.toString.call(new Map()) + Object.prototype.toString.call(new Set())", "[object Map][object Set]");
    eq("Map.prototype[Symbol.iterator] === Map.prototype.entries && Set.prototype[Symbol.iterator] === Set.prototype.values && Set.prototype.keys === Set.prototype.values", "true");
    throws("Map()", "TypeError");
    throws("new Map([1])", "TypeError");
    eq("var s = new Set([1, 2, 2, 3, NaN, NaN]); [s.size, s.has(NaN), s.add(4) === s, s.delete(1), [...s].join()].join('|')", "4|true|true|true|2,3,NaN,4");
    eq("[...new Set('hello')].join('')", "helo");
    eq("var out = []; var s = new Set([1, 2]); s.forEach(function(v, v2, ss){ out.push(v === v2 && ss === s); if (v === 1) s.add(3); }); out.join()", "true,true,true");
    eq("[...new Set([1, 2, 3]).union(new Set([3, 4]))].join()", "1,2,3,4");
    eq("[...new Set([1, 2, 3]).intersection(new Set([2, 3, 4]))].join()", "2,3");
    eq("[...new Set([1, 2, 3]).difference(new Set([2]))].join()", "1,3");
    eq("[...new Set([1, 2, 3]).symmetricDifference(new Set([3, 4]))].join()", "1,2,4");
    eq("[new Set([1]).isSubsetOf(new Set([1, 2])), new Set([1, 2]).isSupersetOf(new Set([2])), new Set([1]).isDisjointFrom(new Set([2])), new Set([1]).isDisjointFrom(new Set([1]))].join()", "true,true,true,false");
    eq("[...new Set([1, 2]).union(new Map([[2, 'x'], [3, 'y']]))].join()", "1,2,3");
    throws("new Set([1]).union([1, 2])", "TypeError");
    eq("var g = Map.groupBy([1, 2, 3, 4], n => n % 2 ? 'odd' : 'even'); g.get('odd').join() + '|' + g.get('even').join()", "1,3|2,4");
    eq("var k = {}; var wm = new WeakMap([[k, 1]]); [wm.get(k), wm.has({}), wm.set(k, 2) === wm, wm.get(k), wm.delete(k), wm.has(k)].join()", "1,false,true,2,true,false");
    throws("new WeakMap().set(1, 1)", "TypeError");
    throws("new WeakSet().add('x')", "TypeError");
    eq("var o = {}; var ws = new WeakSet([o]); [ws.has(o), ws.has({}), ws.delete(o), ws.has(o)].join()", "true,false,true,false");
    eq("new WeakMap().set(Symbol('s'), 1).size", "undefined");
    eq("class MyMap extends Map { constructor(it) { super(it); this.tag = 'x'; } } var mm = new MyMap([[1, 2]]); [mm instanceof Map, mm.get(1), mm.tag].join()", "true,2,x");
}

// ---------------------------------------------------------- Typed arrays

#[test]
fn typed_arrays() {
    eq("new Uint8Array(4).length", "4");
    eq("new Uint8Array([1, 2, 300]).join()", "1,2,44");
    eq("new Uint8ClampedArray([-5, 300, 1.5, 2.5, NaN]).join()", "0,255,2,2,0");
    eq("new Int8Array([200]).join() + ',' + new Int16Array([70000])[0] + ',' + new Uint32Array([-1])[0]", "-56,4464,4294967295");
    eq("Float32Array.of(1.1)[0].toFixed(5)", "1.10000");
    eq("new Float64Array([0.1, 0.2]).reduce((a, b) => a + b)", "0.30000000000000004");
    eq("var b = new ArrayBuffer(8); var v = new DataView(b); v.setUint16(0, 258); v.setUint16(2, 258, true); [new Uint8Array(b).join(), v.getUint16(0), v.getUint16(0, true), v.getInt8(0)].join('|')", "1,2,2,1,0,0,0,0|258|513|1");
    eq("var b = new ArrayBuffer(8); new DataView(b).setFloat64(0, Math.PI); new DataView(b).getFloat64(0) === Math.PI", "true");
    eq("var b = new ArrayBuffer(8); new DataView(b).setBigInt64(0, -2n); new DataView(b).getBigUint64(0)", "18446744073709551614");
    throws("new DataView(new ArrayBuffer(4)).getUint32(1)", "RangeError");
    throws("new DataView(new ArrayBuffer(4), 5)", "RangeError");
    eq("var buf = new ArrayBuffer(16); var a = new Int32Array(buf, 4, 2); a[0] = 7; a[1] = 8; [a.byteOffset, a.byteLength, a.length, new Int32Array(buf)[1], a.buffer === buf].join()", "4,8,2,7,true");
    throws("new Int32Array(new ArrayBuffer(7))", "RangeError");
    throws("new Int32Array(new ArrayBuffer(8), 2)", "RangeError");
    eq("var a = new Uint8Array([1, 2, 3, 4]); var s = a.subarray(1, 3); s[0] = 9; [a.join(), s.join(), s.buffer === a.buffer, s.byteOffset].join('|')", "1,9,3,4|9,3|true|1");
    eq("var a = new Uint8Array([1, 2, 3, 4]); var c = a.slice(1, 3); c[0] = 9; a.join() + '|' + c.join()", "1,2,3,4|9,3");
    eq("var a = new Uint8Array(6); a.set([1, 2, 3]); a.set(new Uint8Array([7, 8]), 3); a.join()", "1,2,3,7,8,0");
    eq("var a = new Uint8Array([1, 2, 3, 4, 5]); a.set(a.subarray(0, 3), 2); a.join()", "1,2,1,2,3");
    throws("new Uint8Array(2).set([1, 2, 3])", "RangeError");
    eq("new Float64Array([3, NaN, -0, 0, -1, 10]).sort().join()", "-1,0,0,3,10,NaN");
    eq("var s = new Float64Array([3, NaN, -0, 0]).sort(); 1 / s[0] + ',' + 1 / s[1]", "-Infinity,Infinity");
    eq("new Int16Array([5, 1, 4]).sort((a, b) => b - a).join()", "5,4,1");
    eq("new Uint8Array([3, 1, 2]).toSorted().join() + '|' + new Uint8Array([3, 1, 2]).toReversed().join() + '|' + new Uint8Array([3, 1, 2]).with(0, 9).join()", "1,2,3|2,1,3|9,1,2");
    eq("new Uint8Array([1, 2, 3]).map(x => x * 2).join() + '|' + new Uint8Array([1, 2, 3]).filter(x => x > 1).join()", "2,4,6|2,3");
    eq("var a = new Uint8Array([1, 2, 3]); [a.at(-1), a.indexOf(2), a.includes(3), a.lastIndexOf(1), a.find(x => x > 1), a.findIndex(x => x > 1), a.findLast(x => x < 3), a.findLastIndex(x => x < 3), a.every(x => x > 0), a.some(x => x > 2)].join()", "3,1,true,0,2,1,2,1,true,true");
    eq("[...new Uint8Array([1, 2]).entries()].map(e => e.join(':')).join() + '|' + [...new Uint8Array([5, 6]).keys()].join() + '|' + [...new Uint8Array([5, 6])].join()", "0:1,1:2|0,1|5,6");
    eq("new Uint8Array([1, 2, 3, 4, 5]).copyWithin(0, 3).join()", "4,5,3,4,5");
    eq("new Uint8Array(4).fill(7, 1, 3).join()", "0,7,7,0");
    eq("new Uint8Array([1, 2, 3]).reverse().join()", "3,2,1");
    eq("Uint8Array.from([1, 2, 3], x => x * 2).join() + '|' + Uint8Array.from(new Set([4, 5])).join() + '|' + Int8Array.of(1, -1).join()", "2,4,6|4,5|1,-1");
    eq("var a = new Uint8Array(2); a[5] = 1; a[-1] = 2; a['1'] = 3; [a.length, a[5], a[-1], a[1], Object.keys(a).join(), 'x' in a, '-1' in a, '1' in a].join('|')", "2|||3|0,1|false|false|true");
    eq("Object.prototype.toString.call(new Uint8Array()) + ',' + Uint8Array.BYTES_PER_ELEMENT + ',' + Float64Array.prototype.BYTES_PER_ELEMENT + ',' + new Uint8Array()[Symbol.toStringTag]", "[object Uint8Array],1,8,Uint8Array");
    eq("Object.getPrototypeOf(Uint8Array) === Object.getPrototypeOf(Int32Array) && Object.getPrototypeOf(Uint8Array.prototype) === Object.getPrototypeOf(Int32Array.prototype)", "true");
    eq("new BigInt64Array([1n, -1n]).join() + '|' + new BigUint64Array([-1n])[0]", "1,-1|18446744073709551615");
    throws("new BigInt64Array([1])", "TypeError");
    throws("new Uint8Array([1n])", "TypeError");
    eq("ArrayBuffer.isView(new Uint8Array()) + ',' + ArrayBuffer.isView(new DataView(new ArrayBuffer(1))) + ',' + ArrayBuffer.isView([])", "true,true,false");
    eq("var b = new ArrayBuffer(4); new Uint8Array(b).set([1, 2, 3, 4]); new Uint8Array(b.slice(1, 3)).join() + ',' + b.slice(-1).byteLength", "2,3,1");
    eq("new Uint8Array(new Uint16Array([258, 3])).join()", "2,3");
    eq("JSON.stringify(new Uint8Array([1, 2]))", "{\"0\":1,\"1\":2}");
    eq("Array.from(new Uint8Array([1, 2])).concat([3]).join()", "1,2,3");
    eq("new Uint8Array([1, 2]).toString() + '|' + new Uint8Array([1000]).toLocaleString('en-US')", "1,2|232");
    eq("class U extends Uint8Array {} var u = new U(2); [u instanceof Uint8Array, u.map(x => x) instanceof U, u.length].join()", "true,true,2");
    eq("new SharedArrayBuffer(4).byteLength", "4");
    eq("var a = new Int32Array(2); [Atomics.add(a, 0, 5), Atomics.load(a, 0), Atomics.sub(a, 0, 2), Atomics.exchange(a, 0, 9), Atomics.compareExchange(a, 0, 9, 1), Atomics.load(a, 0), Atomics.store(a, 1, 7), Atomics.or(a, 1, 8), Atomics.and(a, 1, 12), Atomics.xor(a, 1, 1), a[1], Atomics.isLockFree(4), Atomics.notify(a, 0)].join()", "0,5,5,3,9,1,7,7,15,12,13,true,0");
    throws("Atomics.add(new Float32Array(1), 0, 1)", "TypeError");
}

#[test]
fn resizable_and_detached_buffers() {
    eq("var b = new ArrayBuffer(4, { maxByteLength: 16 }); var a = new Uint8Array(b); var f = new Uint8Array(b, 0, 4); b.resize(8); var l1 = a.length; b.resize(2); [b.resizable, b.maxByteLength, l1, a.length, f.length, f.byteOffset, f[0]].join()", "true,16,8,2,0,0,");
    eq("var b = new ArrayBuffer(8); var a = new Uint8Array(b); a[0] = 5; var c = b.transfer(); [b.detached, b.byteLength, a.length, a[0], c.byteLength, new Uint8Array(c)[0]].join()", "true,0,0,,8,5");
    throws("var b = new ArrayBuffer(8); b.transfer(); new Uint8Array(b).fill(1)", "TypeError");
    throws("var b = new ArrayBuffer(8); var d = new DataView(b); b.transfer(); d.getInt8(0)", "TypeError");
    eq("var b = new ArrayBuffer(8).transferToImmutable(); var a = new Uint8Array(b); a[0] = 1; [b.immutable, a[0]].join()", "true,0");
    throws("new ArrayBuffer(9007199254740991)", "RangeError");
    eq("var s = new SharedArrayBuffer(2, { maxByteLength: 4 }); s.grow(4); [s.growable, s.byteLength].join()", "true,4");
    eq("new Uint8Array([72, 105, 33]).toBase64() + '|' + Uint8Array.fromBase64('SGkh').join() + '|' + new Uint8Array([255, 1]).toHex() + '|' + Uint8Array.fromHex('ff01').join()", "SGkh|72,105,33|ff01|255,1");
    eq("var t = new Uint8Array(2); var r = t.setFromBase64('SGkh'); [r.read, r.written, t.join()].join()", "0,0,0,0");
    eq("[Math.f16round(1.337), new Float16Array([65520, 1e-8])].join()", "1.3369140625,Infinity,0");
    eq("var b = new ArrayBuffer(8); var a = new Uint8Array(b); b.transfer(); [...(function*(){ try { for (var x of a) yield x; } catch (e) { yield e.name; } })()].join()", "TypeError");
}

// ------------------------------------------------------------------ Intl

#[test]
fn intl_number_format() {
    eq("(1234567.891).toLocaleString('en-US')", "1,234,567.891");
    eq("(1234567.891).toLocaleString('es-ES')", "1.234.567,891");
    eq("(1234).toLocaleString('es-ES') + '|' + (12345).toLocaleString('es-ES')", "1234|12.345");
    eq("(1234567.891).toLocaleString('de-DE')", "1.234.567,891");
    eq("(0.5).toLocaleString('en-US', {style: 'percent'})", "50%");
    eq("(0.256).toLocaleString('en-US', {style: 'percent', maximumFractionDigits: 1})", "25.6%");
    eq("(1234.5).toLocaleString('en-US', {style: 'currency', currency: 'USD'})", "$1,234.50");
    eq("(1234.5).toLocaleString('es-ES', {style: 'currency', currency: 'EUR'})", "1234,50\u{a0}€");
    eq("(12345.5).toLocaleString('de-DE', {style: 'currency', currency: 'EUR'})", "12.345,50\u{a0}€");
    eq("(1234.5).toLocaleString('en-US', {style: 'currency', currency: 'EUR'})", "€1,234.50");
    eq("(5000).toLocaleString('ja-JP', {style: 'currency', currency: 'JPY'})", "¥5,000");
    eq("(1234.5).toLocaleString('en-US', {style: 'currency', currency: 'USD', currencyDisplay: 'code'})", "USD\u{a0}1,234.50");
    eq("(1).toLocaleString('en-US', {style: 'currency', currency: 'USD', currencyDisplay: 'name'}) + '|' + (2).toLocaleString('en-US', {style: 'currency', currency: 'USD', currencyDisplay: 'name'})", "1.00 US dollar|2.00 US dollars");
    eq("new Intl.NumberFormat('en-US', {minimumFractionDigits: 2}).format(3)", "3.00");
    eq("new Intl.NumberFormat('en-US', {maximumFractionDigits: 0}).format(2.5) + ',' + new Intl.NumberFormat('en-US', {maximumFractionDigits: 0}).format(3.5) + ',' + new Intl.NumberFormat('en-US', {maximumFractionDigits: 2}).format(1.005)", "3,4,1.01");
    eq("new Intl.NumberFormat('en-US', {notation: 'compact'}).format(1234) + ',' + new Intl.NumberFormat('en-US', {notation: 'compact'}).format(12345) + ',' + new Intl.NumberFormat('en-US', {notation: 'compact'}).format(1500000) + ',' + new Intl.NumberFormat('en-US', {notation: 'compact'}).format(999)", "1.2K,12K,1.5M,999");
    eq("new Intl.NumberFormat('en-US', {notation: 'compact', compactDisplay: 'long'}).format(2500000)", "2.5 million");
    eq("new Intl.NumberFormat('en-US', {minimumIntegerDigits: 3}).format(7)", "007");
    eq("new Intl.NumberFormat('en-US', {useGrouping: false}).format(1234567)", "1234567");
    eq("new Intl.NumberFormat('en-US', {signDisplay: 'always'}).format(5) + ',' + new Intl.NumberFormat('en-US', {signDisplay: 'exceptZero'}).format(0) + ',' + new Intl.NumberFormat('en-US').format(-5)", "+5,0,-5");
    eq("new Intl.NumberFormat('en-US', {maximumSignificantDigits: 3}).format(123456) + ',' + new Intl.NumberFormat('en-US', {maximumSignificantDigits: 3}).format(0.00012345)", "123,000,0.000123");
    eq("new Intl.NumberFormat('en-US', {style: 'unit', unit: 'kilometer'}).format(12) + '|' + new Intl.NumberFormat('en-US', {style: 'unit', unit: 'kilometer', unitDisplay: 'long'}).format(12)", "12 km|12 kilometers");
    eq("new Intl.NumberFormat('en-US').format(Infinity) + ',' + new Intl.NumberFormat('en-US').format(NaN) + ',' + new Intl.NumberFormat('en-US').format(12345678901234567890n)", "∞,NaN,12,345,678,901,234,567,890");
    eq("new Intl.NumberFormat('en-US').formatToParts(-1234.5).map(p => p.type + ':' + p.value).join('|')", "minusSign:-|integer:1|group:,|integer:234|decimal:.|fraction:5");
    eq("var f = new Intl.NumberFormat('en-US').format; [1000, 2000].map(f).join('|')", "1,000|2,000");
    eq("new Intl.NumberFormat('es-ES').resolvedOptions().locale + ',' + new Intl.NumberFormat('en-US', {style: 'currency', currency: 'usd'}).resolvedOptions().currency", "es-ES,USD");
    throws("new Intl.NumberFormat('en-US', {style: 'currency'})", "TypeError");
    throws("new Intl.NumberFormat('en-US', {maximumFractionDigits: 101})", "RangeError");
    eq("(1234.5).toLocaleString() === (1234.5).toLocaleString('en-US')", "true");
    eq("(255).toLocaleString(undefined, {minimumFractionDigits: 1})", "255.0");
    eq("[1234.5, 6].toLocaleString('en-US')", "1,234.5,6");
    eq("(12n).toLocaleString('en-US')", "12");
}

#[test]
fn intl_collation_plural_relative_list() {
    eq("['b', 'a', 'C', 'á'].sort(new Intl.Collator('en').compare).join()", "a,á,b,C");
    eq("['z', 'a', 'Z', 'A'].sort((x, y) => x.localeCompare(y)).join()", "a,A,z,Z");
    eq("'a'.localeCompare('A', 'en', {sensitivity: 'base'}) + ',' + 'a'.localeCompare('á', 'en', {sensitivity: 'base'}) + ',' + 'a'.localeCompare('á', 'en', {sensitivity: 'accent'}) + ',' + 'a'.localeCompare('b')", "0,0,-1,-1");
    eq("['item10', 'item2', 'item1'].sort(new Intl.Collator('en', {numeric: true}).compare).join()", "item1,item2,item10");
    eq("['item10', 'item2'].sort(new Intl.Collator('en').compare).join()", "item10,item2");
    eq("new Intl.Collator('de', {sensitivity: 'base'}).compare('ä', 'a')", "0");
    eq("new Intl.Collator().resolvedOptions().sensitivity", "variant");
    eq("var pr = new Intl.PluralRules('en-US'); [pr.select(0), pr.select(1), pr.select(2), pr.select(1.5)].join()", "other,one,other,other");
    eq("var pr = new Intl.PluralRules('en-US', {type: 'ordinal'}); [1, 2, 3, 4, 11, 12, 13, 21, 22, 23, 101].map(n => pr.select(n)).join()", "one,two,few,other,other,other,other,one,two,few,one");
    eq("new Intl.PluralRules('fr').select(0) + ',' + new Intl.PluralRules('ru').select(5) + ',' + new Intl.PluralRules('ja').select(1)", "one,many,other");
    eq("var rtf = new Intl.RelativeTimeFormat('en'); [rtf.format(3, 'day'), rtf.format(-1, 'day'), rtf.format(1, 'hour'), rtf.format(-2.5, 'hours')].join('|')", "in 3 days|1 day ago|in 1 hour|2.5 hours ago");
    eq("var rtf = new Intl.RelativeTimeFormat('en', {numeric: 'auto'}); [rtf.format(-1, 'day'), rtf.format(1, 'day'), rtf.format(0, 'day'), rtf.format(-1, 'week'), rtf.format(0, 'second'), rtf.format(2, 'day')].join('|')", "yesterday|tomorrow|today|last week|now|in 2 days");
    eq("var rtf = new Intl.RelativeTimeFormat('es', {numeric: 'auto'}); [rtf.format(-3, 'day'), rtf.format(1, 'month'), rtf.format(-1, 'day')].join('|')", "hace 3 días|dentro de 1 mes|ayer");
    eq("new Intl.RelativeTimeFormat('en', {style: 'short'}).format(-5, 'minute')", "5 min. ago");
    throws("new Intl.RelativeTimeFormat('en').format(1, 'fortnight')", "RangeError");
    eq("new Intl.ListFormat('en').format(['a', 'b', 'c']) + '|' + new Intl.ListFormat('en').format(['a', 'b']) + '|' + new Intl.ListFormat('en', {type: 'disjunction'}).format(['a', 'b', 'c']) + '|' + new Intl.ListFormat('en', {type: 'unit'}).format(['a', 'b', 'c']) + '|' + new Intl.ListFormat('en').format(['solo']) + '|' + new Intl.ListFormat('en').format([])", "a, b, and c|a and b|a, b, or c|a, b, c|solo|");
    eq("new Intl.ListFormat('es').format(['a', 'b', 'c']) + '|' + new Intl.ListFormat('es', {type: 'disjunction'}).format(['a', 'b'])", "a, b y c|a o b");
    eq("new Intl.ListFormat('en').formatToParts(['a', 'b']).map(p => p.type).join()", "element,literal,element");
    eq("Intl.getCanonicalLocales(['EN-us', 'es_es', 'zh-hant-tw']).join()", "en-US,es-ES,zh-Hant-TW");
    throws("Intl.getCanonicalLocales('en-US-')", "RangeError");
    eq("new Intl.Locale('en-US').language + ',' + new Intl.Locale('en-US').region + ',' + String(new Intl.Locale('es'))", "en,US,es");
    eq("new Intl.DisplayNames(['en'], {type: 'region'}).of('ES') + ',' + new Intl.DisplayNames('es', {type: 'language'}).of('en')", "Spain,inglés");
    eq("[...new Intl.Segmenter('en', {granularity: 'word'}).segment('Hello, world')].filter(s => s.isWordLike).map(s => s.segment).join()", "Hello,world");
    eq("Intl.supportedValuesOf('calendar').join() + ',' + Intl.NumberFormat.supportedLocalesOf(['en-US', 'de']).join()", "gregory,en-US,de");
    eq("Object.prototype.toString.call(Intl) + Object.prototype.toString.call(new Intl.NumberFormat())", "[object Intl][object Intl.NumberFormat]");
}

// ---------------------------------------------------------------- Extras

#[test]
fn es2023_surface_probe() {
    let probes = [
        "Array.prototype.at", "Array.prototype.flatMap", "Array.prototype.copyWithin", "Array.prototype.findLast", "Array.prototype.findLastIndex",
        "Array.prototype.toSorted", "Array.prototype.toReversed", "Array.prototype.toSpliced", "Array.prototype.with", "Array.from", "Array.of",
        "Array.fromAsync", "String.prototype.at", "String.prototype.normalize", "String.prototype.isWellFormed", "String.prototype.toWellFormed",
        "String.prototype.localeCompare", "String.prototype.replaceAll", "String.prototype.matchAll", "String.prototype.padStart", "String.prototype.trimStart",
        "String.prototype.anchor", "String.prototype.substr", "String.raw", "String.fromCodePoint", "Object.groupBy", "Object.fromEntries", "Object.hasOwn",
        "Object.entries", "Object.getOwnPropertyDescriptors", "Object.prototype.__defineGetter__", "Object.prototype.__lookupGetter__",
        "Promise.try", "Promise.allSettled", "Promise.any", "Promise.withResolvers", "Promise.prototype.finally", "Function.prototype[Symbol.hasInstance]",
        "Error.captureStackTrace", "Error.prototype.toString", "AggregateError", "WeakRef", "WeakRef.prototype.deref", "FinalizationRegistry",
        "FinalizationRegistry.prototype.register", "Atomics.add", "Atomics.load", "Atomics.store", "Atomics.wait", "escape", "unescape", "structuredClone",
        "queueMicrotask", "Number.isSafeInteger", "Number.parseFloat", "Number.parseInt", "Math.fround", "Math.clz32", "Math.imul", "Math.hypot", "Math.cbrt",
        "Math.expm1", "Math.log1p", "Math.sign", "Math.trunc", "Math.sinh", "Math.cosh", "Math.tanh", "Math.asinh", "Math.acosh", "Math.atanh", "Math.log2",
        "Math.log10", "RegExp.prototype.compile", "RegExp.prototype[Symbol.matchAll]", "Date.prototype.getYear", "Date.prototype.toGMTString",
        "Date.prototype[Symbol.toPrimitive]", "Symbol.asyncIterator", "Symbol('d').description === 'd' && Symbol", "BigInt.asIntN", "Map.groupBy", "Set.prototype.union",
        "Set.prototype.isSubsetOf", "WeakMap.prototype.set", "WeakSet.prototype.add", "ArrayBuffer.isView", "ArrayBuffer.prototype.slice", "DataView.prototype.getFloat64",
        "Uint8Array.from", "Uint8Array.prototype.subarray", "Uint8Array.prototype.toSorted", "Uint8Array.prototype.with", "Float64Array.prototype.set",
        "Intl.DateTimeFormat", "Intl.NumberFormat", "Intl.Collator", "Intl.PluralRules", "Intl.RelativeTimeFormat", "Intl.ListFormat", "Intl.Locale",
        "Intl.getCanonicalLocales", "Reflect.ownKeys", "Proxy.revocable", "JSON.stringify", "globalThis.Intl", "Date.now", "Date.parse", "Date.UTC",
        "Date.prototype.toLocaleDateString", "Number.prototype.toLocaleString", "BigInt.prototype.toLocaleString", "Array.prototype.toLocaleString",
    ];
    let mut realm = Realm::new();
    let mut missing = Vec::new();
    for p in probes {
        let src = format!("(function(){{ try {{ var v = {}; return typeof v === 'function' || typeof v === 'object' || typeof v === 'symbol'; }} catch (e) {{ return false; }} }})()", p);
        match realm.eval(&src, "probe.js").as_ref().map(Value::v) {
            Ok(V::Bool(true)) => {}
            _ => missing.push(p),
        }
    }
    assert!(missing.is_empty(), "missing builtins: {:?}", missing);
}

#[test]
fn extras_behaviour() {
    eq("var o = {}; var w = new WeakRef(o); [w.deref() === o, Object.prototype.toString.call(w), w instanceof WeakRef].join()", "true,[object WeakRef],true");
    throws("new WeakRef(1)", "TypeError");
    throws("WeakRef({})", "TypeError");
    eq("var fr = new FinalizationRegistry(() => {}); [fr.register({}, 'h'), fr.unregister({})].join()", ",false");
    throws("new FinalizationRegistry(1)", "TypeError");
    throws("new FinalizationRegistry(() => {}).register(1, 2)", "TypeError");
    eq("var log = []; Promise.try(() => 1).then(v => log.push('ok:' + v)); Promise.try(() => { throw new Error('x'); }).catch(e => log.push('err:' + e.message)); Promise.try((a, b) => a + b, 2, 3).then(v => log.push('sum:' + v)); Promise.try(() => Promise.resolve(9)).then(v => log.push('nested:' + v)); 'queued'", "queued");
    assert_eq!(logs("var log = []; Promise.try(() => 1).then(v => log.push('ok:' + v)); Promise.try(() => { throw new Error('x'); }).catch(e => log.push('err:' + e.message)); Promise.try((a, b) => a + b, 2, 3).then(v => log.push('sum:' + v)); Promise.try(() => Promise.resolve(9)).then(v => log.push('nested:' + v)); Promise.resolve().then(() => {}).then(() => {}).then(() => console.log(log.join('|')));"), "ok:1|err:x|sum:5|nested:9");
    eq("/(\\d+)-(\\d+)/.exec('a 12-34 b'); [RegExp.$1, RegExp.$2, RegExp.$3, RegExp.lastMatch, RegExp['$&'], RegExp.input, RegExp.leftContext, RegExp.rightContext, RegExp.lastParen].join('|')", "12|34||12-34|12-34|a 12-34 b|a | b|34");
    // Atomics.wait needs a shared buffer, and the main thread cannot block.
    throws("Atomics.wait(new Int32Array(1), 0, 0, 0)", "TypeError");
    throws("Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 0)", "TypeError");
    eq("var a = new Int32Array(new SharedArrayBuffer(8)); var r = Atomics.waitAsync(a, 0, 1); var s = Atomics.waitAsync(a, 0, 0, 0); var t = Atomics.waitAsync(a, 0, 0); [r.async, r.value, s.async, s.value, t.async, Atomics.notify(a, 0)].join()", "false,not-equal,false,timed-out,true,1");
    eq("Error.prototype.toString.call({name: 'X', message: 'm'})", "X: m");
    eq("new Date(2026, 0, 1).toLocaleDateString('en-US') === new Intl.DateTimeFormat('en-US').format(new Date(2026, 0, 1))", "true");
}

// ------------------------------------------------------------------ modules

mod modules {
    use crate::modules::{ModuleId, ModuleStatus};
    use crate::realm::Realm;
    use crate::value::Value;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    /// Resolve `spec` against `referrer` for the in-memory `mem:/` scheme.
    fn resolve(referrer: &str, spec: &str) -> Option<String> {
        if spec.starts_with("mem:/") {
            return Some(spec.to_string());
        }
        let rest = spec.strip_prefix("./")?;
        let dir = match referrer.rfind('/') {
            Some(i) if referrer.starts_with("mem:/") => &referrer[..=i],
            _ => "mem:/",
        };
        Some(format!("{dir}{rest}"))
    }

    /// A realm with an in-memory module loader over `files` (name → source,
    /// served at `mem:/<name>`) and console capture.
    fn realm_with(files: &[(&str, &str)]) -> (Realm, Rc<RefCell<Vec<String>>>) {
        let mut realm = Realm::new();
        let logs = Rc::new(RefCell::new(Vec::<String>::new()));
        let l2 = logs.clone();
        realm.set_console_sink(Box::new(move |_lvl, text| l2.borrow_mut().push(text)));
        let sources: HashMap<String, String> = files.iter().map(|(n, s)| (format!("mem:/{n}"), s.to_string())).collect();
        let cache: Rc<RefCell<HashMap<String, ModuleId>>> = Rc::default();
        realm.set_module_loader(Box::new(move |realm, referrer, spec| {
            let Some(url) = resolve(referrer, spec) else {
                return Err(realm.new_error("TypeError", &format!("Failed to resolve module specifier \"{spec}\"")));
            };
            if let Some(id) = cache.borrow().get(&url) {
                return Ok(*id);
            }
            let Some(src) = sources.get(&url) else {
                return Err(realm.new_error("TypeError", &format!("Failed to fetch dynamically imported module: {url}")));
            };
            let id = realm.compile_module(src, &url)?;
            cache.borrow_mut().insert(url, id);
            Ok(id)
        }));
        (realm, logs)
    }

    /// Load `entry` through the loader, link, evaluate and drain jobs.
    /// Returns the console lines joined by `|`, or the error text.
    fn run_entry(files: &[(&str, &str)], entry: &str) -> Result<String, String> {
        let (mut realm, logs) = realm_with(files);
        let r = run_in(&mut realm, entry);
        realm.run_jobs();
        let out = logs.borrow().join("|");
        r.map(|_| out)
    }

    fn run_in(realm: &mut Realm, entry: &str) -> Result<ModuleId, String> {
        let id = realm.load_imported_module("mem:/", &format!("./{entry}")).map_err(|e| realm.error_to_string(&e))?;
        realm.link_module(id).map_err(|e| realm.error_to_string(&e))?;
        let p = realm.evaluate_module(id).map_err(|e| realm.error_to_string(&e))?;
        realm.run_jobs();
        if let Some(e) = realm.module_error(id) {
            return Err(realm.error_to_string(&e));
        }
        assert!(p.is_object());
        Ok(id)
    }

    fn ok(files: &[(&str, &str)], entry: &str) -> String {
        match run_entry(files, entry) {
            Ok(s) => s,
            Err(e) => panic!("module {entry} failed: {e}"),
        }
    }

    fn err(files: &[(&str, &str)], entry: &str) -> String {
        match run_entry(files, entry) {
            Ok(s) => panic!("module {entry} should fail, logged {s:?}"),
            Err(e) => e,
        }
    }

    #[test]
    fn named_default_and_namespace_imports() {
        let files = [
            ("a.js", "export const x = 1; export default function f() { return 'def'; } export function g() { return 'g'; } export let y = 2, z = 3;"),
            ("main.js", "import f, { x, g as h, y, z } from './a.js'; import * as ns from './a.js';\nconsole.log(x, f(), h(), y + z, ns.x, ns.default === f, typeof ns.g);"),
            ("anon.js", "export default class {}"),
            ("expr.js", "export default 40 + 2;"),
            ("fn.js", "export default function () { return 'anon'; }"),
            ("alias.js", "const v = 'aliased'; export { v as default, v as other };"),
            ("main2.js", "import C from './anon.js'; import n from './expr.js'; import fn from './fn.js'; import al, { other } from './alias.js';\nconsole.log(new C().constructor.name, n, fn(), fn.name, al, other);"),
        ];
        assert_eq!(ok(&files, "main.js"), "1 def g 5 1 true function");
        assert_eq!(ok(&files, "main2.js"), "default 42 anon default aliased aliased");
    }

    #[test]
    fn live_bindings_are_read_only() {
        let files = [
            ("counter.js", "export let count = 0; export function inc() { count++; }"),
            ("main.js", "import { count, inc } from './counter.js'; import * as c from './counter.js';\nconsole.log(count); inc(); inc(); console.log(count, c.count);\ntry { count = 5; } catch (e) { console.log(e.constructor.name, e.message); }"),
        ];
        assert_eq!(ok(&files, "main.js"), "0|2 2|TypeError Assignment to constant variable.");
    }

    #[test]
    fn re_exports_and_star_conflicts() {
        let files = [
            ("a.js", "export const a = 'a'; export const dup = 'a-dup'; export default 'a-default';"),
            ("b.js", "export const b = 'b'; export const dup = 'b-dup';"),
            ("re.js", "export * from './a.js'; export * from './b.js'; export { a as renamed } from './a.js'; export * as nsB from './b.js'; import { b } from './b.js'; export { b as default };"),
            ("main.js", "import * as r from './re.js'; import def, { a, renamed, nsB } from './re.js';\nconsole.log(a, renamed, nsB.b, def, Object.keys(r).join(','), 'dup' in r);"),
            ("conflict.js", "import { dup } from './re.js'; console.log(dup);"),
            ("missing.js", "import { nope } from './a.js';"),
            ("bad-reexport.js", "export { nope } from './b.js';"),
        ];
        assert_eq!(ok(&files, "main.js"), "a a b b a,b,default,nsB,renamed false");
        assert!(err(&files, "conflict.js").contains("SyntaxError: The requested module './re.js' contains conflicting star exports for name 'dup'"));
        assert!(err(&files, "missing.js").contains("SyntaxError: The requested module './a.js' does not provide an export named 'nope'"));
        assert!(err(&files, "bad-reexport.js").contains("The requested module './b.js' does not provide an export named 'nope'"));
    }

    #[test]
    fn cycles_hoist_functions_and_report_tdz() {
        let files = [
            ("even.js", "import { isOdd } from './odd.js'; export function isEven(n) { return n === 0 ? true : isOdd(n - 1); } console.log('even', isEven(3));"),
            ("odd.js", "import { isEven } from './even.js'; export function isOdd(n) { return n === 0 ? false : isEven(n - 1); } console.log('odd', isEven(4));"),
            ("a.js", "import { b } from './b.js'; export let a = 1; console.log('a', b);"),
            ("b.js", "import { a } from './a.js'; export let b = 2; console.log('b', a);"),
        ];
        assert_eq!(ok(&files, "even.js"), "odd true|even false");
        let (mut realm, logs) = realm_with(&files);
        let e = run_in(&mut realm, "a.js").unwrap_err();
        assert!(e.contains("ReferenceError: Cannot access 'a' before initialization"), "{e}");
        assert!(logs.borrow().is_empty());
        let b = realm.load_imported_module("mem:/", "./b.js").unwrap();
        assert_eq!(realm.module_status(b), ModuleStatus::Errored);
    }

    #[test]
    fn top_level_await_orders_dependents() {
        let files = [
            ("slow.js", "console.log('slow start'); export let v = 1; await Promise.resolve(); await null; v = 2; console.log('slow end');"),
            ("fast.js", "console.log('fast');"),
            ("main.js", "import { v } from './slow.js'; import './fast.js'; console.log('main', v); const r = await Promise.resolve(5); console.log('after', r);"),
            ("boom.js", "await null; throw new Error('late');"),
            ("uses-boom.js", "import './boom.js'; console.log('never');"),
        ];
        assert_eq!(ok(&files, "main.js"), "slow start|fast|slow end|main 2|after 5");
        assert!(err(&files, "uses-boom.js").contains("Error: late"));
    }

    #[test]
    fn dynamic_import_resolves_and_rejects() {
        let files = [
            ("a.js", "console.log('a runs'); export const x = 1;"),
            ("main.js", "import('./a.js').then(ns => console.log('ok', ns.x, ns[Symbol.toStringTag]));\nimport('./missing.js').catch(e => console.log('err', e.constructor.name, e.message));\nconsole.log('sync');"),
        ];
        let out = ok(&files, "main.js");
        let lines: Vec<&str> = out.split('|').collect();
        assert_eq!(lines[0], "sync");
        assert!(lines.contains(&"a runs") && lines.contains(&"ok 1 Module"), "{out}");
        assert!(lines.contains(&"err TypeError Failed to fetch dynamically imported module: mem:/missing.js"), "{out}");

        // From a classic script: the referrer is the script URL.
        let (mut realm, logs) = realm_with(&files);
        realm.eval("import('./a.js').then(ns => console.log('classic', ns.x))", "mem:/classic.js").unwrap();
        realm.run_jobs();
        assert_eq!(logs.borrow().join("|"), "a runs|classic 1");
    }

    #[test]
    fn import_meta_and_module_scope() {
        let files = [
            ("dir/main.js", "console.log(import.meta.url, Object.getPrototypeOf(import.meta) === null, import.meta === import.meta);"),
            ("scope.js", "var secret = 1; let s2 = 2; function fn() { return this; }\nconsole.log(this === undefined, typeof globalThis.secret, typeof globalThis.fn, typeof globalThis.s2, fn() === undefined);\ntry { undeclared = 1; } catch (e) { console.log(e.constructor.name); }\nconsole.log((() => this)() === undefined, typeof arguments === 'undefined');"),
        ];
        assert_eq!(ok(&files, "dir/main.js"), "mem:/dir/main.js true true");
        assert_eq!(ok(&files, "scope.js"), "true undefined undefined undefined true|ReferenceError|true true");
    }

    #[test]
    fn namespace_object_shape() {
        let files = [
            ("a.js", "export const z = 1, a = 2, m = 3; export default 4;"),
            ("main.js", "import * as ns from './a.js';\nconsole.log(Object.prototype.toString.call(ns), Object.isExtensible(ns), Object.getPrototypeOf(ns), ns[Symbol.toStringTag], Object.keys(ns).join(','));\ntry { ns.a = 9; } catch (e) { console.log(e.constructor.name); }\ntry { ns.extra = 1; } catch (e) { console.log(e.constructor.name); }\nconsole.log(ns.a, 'extra' in ns, JSON.stringify(ns));"),
        ];
        assert_eq!(ok(&files, "main.js"), "[object Module] false null Module a,default,m,z|TypeError|TypeError|2 false {\"a\":2,\"default\":4,\"m\":3,\"z\":1}");
    }

    #[test]
    fn webpack_style_chunks() {
        let files = [
            (
                "wp-runtime-6b0851141170f516.js",
                "const installed = {};\nexport const __webpack_require__ = { C(chunk) { for (const id of chunk.__rspack_esm_ids) installed[id] = true; Object.assign(installed, chunk.__webpack_modules__); globalThis.loaded = Object.keys(installed).join(','); } };",
            ),
            ("code-menu-1a2b.js", "export const __rspack_esm_ids=[\"cm\"];export const __webpack_modules__={X1(){}};"),
            (
                "entry.js",
                "export const __rspack_esm_ids=[\"e3\"];export const __webpack_modules__={DZ8(){}};import{__webpack_require__ as e}from\"./wp-runtime-6b0851141170f516.js\";import*as o from\"./code-menu-1a2b.js\";e.C(o);import*as self from\"./entry.js\";e.C(self);console.log(globalThis.loaded);",
            ),
        ];
        assert_eq!(ok(&files, "entry.js"), "cm,X1,e3,DZ8");
    }

    #[test]
    fn synthetic_modules_and_eval_module() {
        let (mut realm, logs) = realm_with(&[("main.js", "import data from './data.json'; console.log(data.k);")]);
        let obj = realm.new_object();
        realm.define_property(obj, "k", Value::str("v"), crate::heap::ALL);
        let json = realm.create_synthetic_module("mem:/data.json", vec![("default".into(), Value::Object(obj))]);
        let main = realm.load_imported_module("mem:/", "./main.js").unwrap();
        // Pre-resolve the JSON request to the synthetic module through a fresh loader.
        realm.set_module_loader(Box::new(move |_realm, _referrer, _spec| Ok(json)));
        realm.link_module(main).unwrap();
        realm.evaluate_module(main).unwrap();
        realm.run_jobs();
        assert_eq!(logs.borrow().join("|"), "v");
        // `eval_module` runs a module through the installed loader.
        let r = realm.eval_module("import d from './whatever.json'; console.log('again', d.k); export {};", "mem:/inline.js");
        assert!(r.is_ok());
        realm.run_jobs();
        assert_eq!(logs.borrow().join("|"), "v|again v");
        // Static syntax errors are SyntaxErrors with the URL.
        let e = realm.compile_module("export const = 1;", "mem:/bad.js").unwrap_err();
        assert!(realm.error_to_string(&e).contains("mem:/bad.js"));
    }
}

#[test]
fn object_rest_with_computed_keys() {
    let mut realm = crate::Realm::new();
    let v = realm
        .eval(
            r#"
            const k = 'b', o = { a: 1, b: 2, c: 3, [Symbol.iterator]: 4 };
            const { [k]: x, a, ...rest } = o;
            let y, others;
            ({ ['c']: y, ...others } = o);
            function f({ [k + '']: z, ...r }) { return z + Object.keys(r).join(''); }
            [x, a, Object.keys(rest).join(','), y, Object.keys(others).join(','), f(o)].join('|')
            "#,
            "t.js",
        )
        .unwrap();
    assert_eq!(realm.to_rust_string(v).unwrap(), "2|1|c|3|a,b|2ac");
}

#[test]
fn object_rest_computed_in_arrow_block() {
    let mut realm = crate::Realm::new();
    let v = realm
        .eval(
            r#"
            const B = { sources: [1], a: 1, b: 2 };
            const f = (e, _) => { let { [_]: r, ...n } = e; return n; };
            const o = { x: 1, ...((e, _) => { let { [_]: r, ...n } = e; return n; })(B, "sources") };
            Object.keys(f(B, "sources")).join(',') + '|' + Object.keys(o).join(',')
            "#,
            "t.js",
        )
        .unwrap();
    assert_eq!(realm.to_rust_string(v).unwrap(), "a,b|x,a,b");
}

#[test]
fn switch_case_let_and_shadowed_fn_name() {
    let mut realm = crate::Realm::new();
    let v = realm
        .eval(
            r#"
            function m(e, t, n, l, u) {
              var m;
              switch (n.kind) {
                case "scalar": e.push('s:' + t); break;
                case "enum":
                  let d = t * 2;
                  if (n.open) e.push('o:' + d);
                  else if ([1, 2, d].some(x => x === d)) e.push('v:' + d);
                  break;
                case "list":
                  m = d => d + 1;
                  e.push('l:' + m(t));
                  break;
              }
              return e;
            }
            [m([], 1, { kind: 'scalar' }).join(), m([], 2, { kind: 'enum' }).join(), m([], 3, { kind: 'enum', open: true }).join(), m([], 4, { kind: 'list' }).join()].join('|')
            "#,
            "t.js",
        )
        .unwrap();
    assert_eq!(realm.to_rust_string(v).unwrap(), "s:1|v:4|o:6|l:5");
}

#[test]
fn bigint_arbitrary_precision() {
    let mut realm = crate::Realm::new();
    let v = realm
        .eval(
            r#"
            const h = BigInt("0x00e9d3dca1328a49ad3403e4badda37a6a13610b608b5099839e1074e720f5a33b2ebd8c2ffd12c09be0015a4635aa9d2022d8f72f90ed11610c3742b0baef5b7da73d7e79aff6cdb");
            const f = 2n ** 200n;
            [
              (f / 3n).toString(),
              (f % 1000007n).toString(),
              (-f >> 190n).toString(),
              (h & 0xffffn).toString(16),
              BigInt.asIntN(64, 2n ** 63n).toString(),
              BigInt.asUintN(64, -1n).toString(16),
              (f > 1e60).toString(), (f == 2 ** 200).toString(), typeof (12345678901234567890123n * 10n),
              Number(2n ** 70n) === 2 ** 70,
            ].join('|')
            "#,
            "t.js",
        )
        .unwrap();
    assert_eq!(
        realm.to_rust_string(v).unwrap(),
        "535646014752996758513987364113720867507400997927597611767125|446616|-1024|6cdb|-9223372036854775808|ffffffffffffffff|true|true|bigint|true"
    );
}

#[test]
fn var_shadows_function_self_name() {
    let mut realm = crate::Realm::new();
    let v = realm
        .eval(
            r#"
            function n(e, t, r) {
              if (r || 2 == arguments.length) for (var n, o = 0, a = t.length; o < a; o++) !n && o in t || (n || (n = Array.prototype.slice.call(t, 0, o)), n[o] = t[o]);
              return e.concat(n || Array.prototype.slice.call(t));
            }
            const g = function h() { var h; return typeof h; };
            const k = function k2() { return typeof k2; };
            function p() { function p() { return 1 } return p(); }
            [JSON.stringify(n(n([], ["a"], !1), ["b"], !1)), JSON.stringify(n([0], [1, 2])), g(), k(), p()].join('|')
            "#,
            "t.js",
        )
        .unwrap();
    assert_eq!(realm.to_rust_string(v).unwrap(), r#"["a","b"]|[0,1,2]|undefined|function|1"#);
}

#[test]
fn conditional_followed_by_decimal_fraction() {
    let mut realm = crate::Realm::new();
    let v = realm.eval("const e = 80, o = null; [e>=75?.75:e>=50?.5:0, true?.001:1, o?.x, (o??{a:1})?.a].join()", "t.js").unwrap();
    assert_eq!(realm.to_rust_string(v).unwrap(), "0.75,0.001,,1");
}

#[test]
fn watchdog_interrupts_infinite_loops() {
    let mut realm = crate::Realm::new();
    realm.set_deadline(Some(std::time::Instant::now() + std::time::Duration::from_millis(200)));
    let t = std::time::Instant::now();
    let r = realm.eval("let n = 0; while (true) { try { n++; } catch (e) {} finally { } }", "t.js");
    assert!(r.is_err());
    assert!(realm.timed_out());
    assert!(t.elapsed() < std::time::Duration::from_secs(2));
    realm.set_deadline(None);
    let v = realm.eval("1 + 1", "t.js").unwrap();
    assert_eq!(realm.to_rust_string(v).unwrap(), "2");
}

fn host_work(_realm: &mut crate::Realm, _this: crate::Value, args: &[crate::Value]) -> crate::JsResult<crate::Value> {
    // Host work done for a script (like a forced layout): excluded.
    let ms = args.first().and_then(|v| v.as_number()).unwrap_or(0.0);
    let t = std::time::Instant::now();
    std::thread::sleep(std::time::Duration::from_millis(ms as u64));
    crate::exclude_from_watchdog(t.elapsed());
    Ok(crate::Value::Undefined)
}

#[test]
fn watchdog_does_not_count_excluded_host_time() {
    let mut realm = crate::Realm::new();
    let g = realm.global();
    realm.define_method(g, "hostWork", 1, host_work);
    realm.set_deadline(Some(std::time::Instant::now() + std::time::Duration::from_millis(150)));
    // 8 x 40 ms of host work in a loop: well past the deadline in wall time.
    let v = realm.eval("let k = 0; for (let i = 0; i < 8; i++) { hostWork(40); for (let j = 0; j < 1000; j++) k++; } k", "t.js");
    assert!(v.is_ok(), "host time was charged to the script");
    assert!(!realm.timed_out());
    // Script time is still charged.
    let r = realm.eval("while (true) {}", "t.js");
    assert!(r.is_err() && realm.timed_out());
    realm.set_deadline(None);
}

fn nested_task(realm: &mut crate::Realm, _this: crate::Value, _args: &[crate::Value]) -> crate::JsResult<crate::Value> {
    // A host callback reached from script tries to start a task.
    assert!(!realm.begin_task(), "nested begin_task must keep the running task's deadline");
    Ok(crate::Value::Undefined)
}

#[test]
fn task_budget_restarts_only_between_tasks() {
    let mut realm = crate::Realm::new();
    let g = realm.global();
    realm.define_method(g, "nested", 0, nested_task);
    realm.set_task_budget(Some(std::time::Duration::from_millis(150)));
    assert!(realm.begin_task());
    let t = std::time::Instant::now();
    let r = realm.eval("while (true) { nested(); }", "t.js");
    assert!(r.is_err() && realm.timed_out());
    assert!(t.elapsed() < std::time::Duration::from_secs(2));
    // The next task starts with a fresh budget.
    assert!(realm.begin_task());
    assert!(!realm.timed_out());
    let v = realm.eval("1 + 2", "t.js").unwrap();
    assert_eq!(v.as_number(), Some(3.0));
}

#[test]
fn timed_out_microtask_checkpoint_keeps_remaining_jobs() {
    let mut realm = crate::Realm::new();
    realm.set_task_budget(Some(std::time::Duration::from_millis(100)));
    realm.eval("globalThis.done = 0; Promise.resolve().then(() => { while (true) {} }); for (let i = 0; i < 50; i++) Promise.resolve().then(() => done++);", "t.js").unwrap();
    let errors = std::rc::Rc::new(std::cell::Cell::new(0));
    let e2 = errors.clone();
    realm.set_console_sink(Box::new(move |_, _| e2.set(e2.get() + 1)));
    assert!(realm.begin_task());
    realm.run_jobs();
    // The reaction's error rejects its promise (an unhandled rejection
    // the host reports); the other jobs are not failed in cascade.
    assert!(errors.get() <= 1, "one timeout, not a cascade ({})", errors.get());
    assert_eq!(realm.take_unhandled_rejections().len(), 1);
    assert!(realm.has_pending_jobs());
    assert!(realm.begin_task());
    realm.run_jobs();
    let v = realm.eval("done", "t.js").unwrap();
    assert_eq!(v.as_number(), Some(50.0));
}

#[test]
fn watchdog_interrupts_catastrophic_regex() {
    let mut realm = crate::Realm::new();
    realm.set_deadline(Some(std::time::Instant::now() + std::time::Duration::from_millis(200)));
    let t = std::time::Instant::now();
    let r = realm.eval("/^(a+)+$/.test('a'.repeat(40) + 'b'); while (true) {}", "t.js");
    assert!(r.is_err());
    assert!(t.elapsed() < std::time::Duration::from_secs(3), "{:?}", t.elapsed());
    realm.set_deadline(None);
}

#[test]
fn finally_inlined_after_block_scope_resolves_outer_names() {
    // `return` from inside a block that declares `let` bindings pops that
    // scope before the inlined finalizer runs; names in the finalizer must
    // still resolve to the outer bindings (WordPress hooks.js pattern).
    eq("function f(){ let c = {s: new Set([1])}; function h(){ try { let u = 1; return u } finally { c.s.delete(1) } } h(); return c.s.size } f()", "0");
    eq("function h(c){ try { let u = 2; { let w = 3; return u + w } } finally { c.n++ } } var o = {n: 0}; h(o) + o.n", "6");
    eq("var r = []; function g(x){ for (const k of [1, 2]) { try { let t = k; if (t === 2) break; } finally { r.push(x + k) } } return r.join() } g('a')", "a1,a2");
    eq("function h(){ let a = 'outer'; try { let a = 'inner'; return a } finally { a += '!' } } h()", "inner");
}

#[test]
fn late_rejection_handlers_are_not_reported_unhandled() {
    let mut realm = Realm::new();
    realm
        .eval(
            "function f() { return Promise.reject(new Error('handled-late')); }\
             f().then(() => 1, () => 2);\
             (async () => { await null; const p = Promise.reject(7); p.catch(() => {}); })();\
             Promise.reject(new Error('really-unhandled'));",
            "t.js",
        )
        .unwrap();
    realm.run_jobs();
    let left: Vec<String> = realm.take_unhandled_rejections().iter().map(|v| realm.error_to_string(v)).collect();
    assert_eq!(left.len(), 1, "{left:?}");
    assert!(left[0].contains("really-unhandled"), "{left:?}");
}

#[test]
fn named_function_expression_binding_is_immutable() {
    // Acid3 test 93.
    eq("var vartest = 0; var functest; var value = (function functest(arg) { if (arg) return 1; vartest = 1; functest = function (arg) { return 2; }; return functest(true); })(false); [vartest, value, typeof functest].join()", "1,1,undefined");
    throws("(function f() { \"use strict\"; f = 1; })()", "constant");
    // Shadowed by a var or parameter of the same name: an ordinary binding.
    eq("(function f(f) { f = 3; return f; })(1)", "3");
    // A function *declaration* may reassign its own name (Babel's
    // `_typeof` helper, inside Svelte's compiler bundle).
    eq("(function(){ 'use strict'; function t(e) { return t = function (x) { return 'ok' + x; }, t(e); } return t(1) + t(2); })()", "ok1ok2");
    eq("(function(){ function t() { t = 5; } t(); return t; })()", "5");
}

#[test]
fn set_with_a_proxy_receiver_reaches_the_target() {
    // Vue's reactive(): set trap → Reflect.set(target, key, value, receiver).
    eq("var raw = { x: 1 }; var p = new Proxy(raw, { set: function (t, k, v, r) { return Reflect.set(t, k, v, r); } }); p.x = 2; p.y = 3; [raw.x, raw.y, Object.keys(p).join('')].join()", "2,3,xy");
    eq("var q = new Proxy({ y: 1 }, {}); Reflect.set(q, 'y', 5, q); q.y", "5");
    eq("var log = []; var q = new Proxy({}, { defineProperty: function (t, k, d) { log.push(k + '=' + d.value); return Reflect.defineProperty(t, k, d); } }); Reflect.set({}, 'z', 7, q); log.join() + ':' + q.z", "z=7:7");
}

#[test]
fn with_statement() {
    // Names found on the object (get, set, call with `this`, typeof, delete).
    eq("var o = { a: 1, f: function () { return this === o; } }; var a = 'outer'; var r; with (o) { a = a + 1; r = [a, f(), typeof a, typeof zzz] } [o.a, a, r.join()].join()", "2,outer,2,true,number,undefined");
    eq("var o = { x: 1 }; with (o) { delete x; } 'x' in o", "false");
    // Not found: the static binding; assignment to an undeclared name
    // creates a global.
    eq("(function () { var local = 5; var o = {}; with (o) { local = local + 1; newGlobal = 7; } return [local, o.local, typeof newGlobal, 'newGlobal' in o].join(); })()", "6,,number,false");
    // Block bindings inside the body shadow the object; nested closures
    // capture through `with`.
    eq("var o = { v: 'obj' }; var fs = []; with (o) { let w = 'block'; fs.push(function () { return v + ':' + w; }); } o.v = 'changed'; fs[0]()", "changed:block");
    eq("function mk(o) { with (o) { return function () { return n * 2; }; } } var o = { n: 3 }; var g = mk(o); o.n = 10; g()", "20");
    // Symbol.unscopables hides names.
    eq("var keys = 'outer'; var r; with ([1, 2]) { r = [typeof keys, length].join(); } r", "string,2");
    // Nested with: innermost object first, then outer, then the scope.
    eq("var a = { x: 'a' }, b = { y: 'b' }; var y = 'scope'; var r; with (a) { with (b) { r = x + y; } } r", "ab");
    // Vue 3 runtime-compiled render functions.
    eq(
        "var Vue = { toDisplayString: function (v) { return '[' + v + ']'; } }; var render = new Function('Vue', 'return function render(_ctx, _cache) { with (_ctx) { const { toDisplayString: _toDisplayString } = Vue; return _toDisplayString(count) + msg } }')(Vue); var ctx = new Proxy({ count: 2, msg: '!' }, { has: function (t, k) { return k in t; } }); render(ctx)",
        "[2]!",
    );
    // Alpine.js: async function with `with (scope)`.
    eq(
        "var AsyncFunction = Object.getPrototypeOf(async function () {}).constructor; var fn = new AsyncFunction(['__self', 'scope'], 'with (scope) { __self.result = n + 1 }; __self.finished = true; return __self.result;'); var s = {}; var p = fn(s, { n: 41 }); [p instanceof Promise, s.finished, s.result, AsyncFunction.name, AsyncFunction !== Function].join()",
        "true,true,42,AsyncFunction,true",
    );
    // `with` is a SyntaxError in strict code.
    throws("'use strict'; with ({}) {}", "SyntaxError");
    // ToObject on the operand.
    throws("with (null) {}", "TypeError");
    eq("var r; with ('abc') { r = length; } r", "3");
}

#[test]
fn finally_does_not_see_bindings_of_the_try_block() {
    // A `let` in the try block shadows an outer binding; a `return` from
    // the block inlines the finalizer, which must resolve the outer name
    // (Svelte's `hydrate()` restored its hydration flag from the wrong
    // `r` and left hydration on).
    eq("(function(){ var out=[]; function f(){ let r='outer'; try { let r='inner'; return r; } finally { out.push(r); } } f(); return out.join(); })()", "outer");
    eq("(function(){ var out=[]; function f(){ let r='o'; try { let r='i'; for (let k of [1]) { let r='loop'; return r; } } finally { out.push(r); } } return f() + out.join(); })()", "loopo");
    eq("(function(){ var out=[]; for (let i = 0; i < 2; i++) { let r = 'x' + i; try { let r = 'y'; if (i == 0) continue; break; } finally { out.push(r); } } return out.join(); })()", "x0,x1");
}

/// Run `f` on a thread with `stack` bytes of native stack (the browser runs
/// page scripts on large-stack threads).
fn on_stack<R: Send + 'static>(stack: usize, f: impl FnOnce() -> R + Send + 'static) -> R {
    std::thread::Builder::new().stack_size(stack).spawn(f).unwrap().join().unwrap()
}

/// Legitimate deep recursion of ~10k frames works like in Chrome, also when
/// every level re-enters the interpreter from native code (`call`,
/// `apply`, `forEach`, getters, `valueOf`, Proxy traps, bound functions).
#[test]
fn deep_recursion_of_ten_thousand_frames() {
    on_stack(256 << 20, || {
        eq("function f(n) { return n === 0 ? 0 : 1 + f(n - 1); } f(10000)", "10000");
        eq("function f(n) { return n === 0 ? 0 : 1 + f.call(null, n - 1); } f(10000)", "10000");
        eq("function f(n) { return n === 0 ? 0 : 1 + f.apply(null, [n - 1]); } f(10000)", "10000");
        eq("var g = function (n) { return n === 0 ? 0 : 1 + g(n - 1); }.bind(null); g(5000)", "5000");
        eq("function f(n) { var r = 0; if (n > 0) [n - 1].forEach(function (m) { r = 1 + f(m); }); return r; } f(5000)", "5000");
        eq("var o = { n: 5000, get d() { if (this.n === 0) return 0; this.n--; return 1 + this.d; } }; o.d", "5000");
        eq("function V(n) { this.n = n; } V.prototype.valueOf = function () { return this.n === 0 ? 0 : 1 + +new V(this.n - 1); }; +new V(5000)", "5000");
        eq("var p = new Proxy({}, { get: function (t, k) { var n = +k; return n === 0 ? 0 : 1 + p[n - 1]; } }); p[3000]", "3000");
        eq("class A { constructor(n) { this.d = n === 0 ? 0 : 1 + new A(n - 1).d; } } new A(5000).d", "5000");
        eq("JSON.stringify(JSON.parse('['.repeat(3000) + ']'.repeat(3000))).length", "6000");
    });
}

/// Unbounded recursion through any re-entry path throws a catchable
/// RangeError instead of overflowing the native stack, even on a small
/// thread stack.
#[test]
fn runaway_recursion_is_a_range_error_not_a_crash() {
    for stack in [2usize << 20, 64 << 20] {
        on_stack(stack, || {
            throws("function r() { return r(); } r()", "Maximum call stack size exceeded");
            throws("function r() { return r.call(null); } r()", "Maximum call stack size exceeded");
            throws("function r() { return r.apply(null, []); } r()", "Maximum call stack size exceeded");
            throws("var o = { get x() { return this.x; } }; o.x", "Maximum call stack size exceeded");
            throws("var o = { toString: function () { return '' + o; } }; '' + o", "Maximum call stack size exceeded");
            throws("var o = { valueOf: function () { return +o; } }; +o", "Maximum call stack size exceeded");
            throws("var p = new Proxy({}, { get: function (t, k) { return p[k]; } }); p.x", "Maximum call stack size exceeded");
            throws("function r() { [1].forEach(r); } r()", "Maximum call stack size exceeded");
            throws("class A { constructor() { new A(); } } new A()", "Maximum call stack size exceeded");
            throws("JSON.parse('['.repeat(1e6) + ']'.repeat(1e6))", "Maximum call stack size exceeded");
            throws("var o = {}; for (var i = 0; i < 1e6; i++) o = { o: o }; JSON.stringify(o)", "Maximum call stack size exceeded");
            throws("var o = {}; Object.defineProperty(o, 'x', { get: function () { return Reflect.get(o, 'x'); } }); o.x", "Maximum call stack size exceeded");
            // The realm stays usable afterwards.
            eq("var depth = 0; function r() { depth++; r.call(null); } try { r(); } catch (e) { depth > 100 && e instanceof RangeError }", "true");
        });
    }
}

/// A regular expression literal evaluated inside a native constructor's
/// callback (a Promise executor) is a RegExp, not an object with the
/// constructor's prototype (Google's bar script ran `/re/.test(x)` from a
/// Promise executor: "undefined is not a function").
#[test]
fn regexp_literal_inside_promise_executor() {
    eq("var r; new Promise(function (res) { var re = /^\\s*-?0x/i; r = [re instanceof RegExp, typeof re.test, re.test(' 0x1f'), Object.getPrototypeOf(re) === RegExp.prototype].join(); res(); }); r", "true,function,true,true");
    eq("var r; new Promise(function () { r = 'a1b2'.replace(/\\d/g, '#'); }); r", "a#b#");
    eq("class P extends Promise {} var r; new P(function () { r = /x/.exec('yx').index; }); r", "1");
}

/// test262 used to abort the process: built-ins stepping an endless
/// iterator collected it whole before calling back (Array.from with a
/// throwing mapper, Promise combinators with a throwing `then`), growing
/// until an allocation failed. They now step lazily and close the
/// iterator; runaway heaps and huge array-likes throw instead of aborting.
#[test]
fn no_abort_on_endless_iterators_and_huge_lengths() {
    let endless = "var closed = 0; var it = { [Symbol.iterator]() { return { next() { return { done: false, value: 1 }; }, return() { closed++; return {}; } }; } };";
    eq(&format!("{endless} try {{ Array.from(it, function () {{ throw 1; }}); }} catch (e) {{}} closed"), "1");
    eq(&format!("{endless} var p = Promise.resolve(); p.then = function () {{ throw 1; }}; Promise.resolve = function () {{ return p; }}; Promise.all(it); closed"), "1");
    eq(&format!("{endless} try {{ Object.fromEntries(it); }} catch (e) {{}} closed"), "1");
    throws("Array.prototype.toReversed.call({ length: 2 ** 32 })", "Invalid array length");
    throws("Math.max.apply(null, { length: 2 ** 40 })", "RangeError");
    // A heap past the realm's limit stops the script (uncatchable).
    let mut realm = Realm::new();
    realm.heap_limit = 200_000;
    let r = realm.eval("var keep = []; try { for (;;) keep.push({}); } catch (e) { 'caught' }", "oom.js");
    let e = r.expect_err("out of memory");
    assert!(realm.error_to_string(&e).contains("Out of memory"));
}

/// Direct eval runs in the caller's scope; Annex B block functions; private
/// brand checks; generator argument binding at the call.
#[test]
fn direct_eval_annexb_private_generators() {
    eq("function f(a) { var b = 2; eval('var c = a + b; function h() { return c * 10; }'); return [c, h(), typeof c].join(); } f(1)", "3,30,number");
    eq("function f() { let q = 1; return eval('q + 1'); } f()", "2");
    eq("function f() { 'use strict'; eval('var z = 1'); return typeof z; } f()", "undefined");
    eq("function f() { let y = 1; { try { eval('var y = 2'); } catch (e) { return e.name; } } } f()", "SyntaxError");
    eq("function f() { var before = typeof bf; { function bf() {} } return before + ',' + typeof bf; } f()", "undefined,function");
    eq("class A { #x = 1; static rd(o) { return o.#x; } } try { A.rd({}); } catch (e) { e.name }", "TypeError");
    eq("class A { #m() {} static has(o) { return #m in o; } } [A.has(new A()), A.has(A.prototype)].join()", "true,false");
    eq("function* g([a]) {} try { g(null); 'no' } catch (e) { e.name }", "TypeError");
    // generator.return runs finally blocks; destructuring closes iterators.
    eq("var log = []; function* g() { try { yield 1; } finally { log.push('f'); } } var it = g(); it.next(); var r = it.return(5); log.join() + r.value + r.done", "f5true");
    eq("var closed = 0; var it = { [Symbol.iterator]() { return { next() { return { value: 1, done: false }; }, return() { closed++; return {}; } }; } }; try { var [a = (() => { throw 1; })()] = [undefined]; } catch (e) {} var [b] = it; closed", "1");
    eq("var closed = 0; var it = { [Symbol.iterator]() { return { next() { return { value: 1, done: false }; }, return() { closed++; return {}; } }; } }; try { var [{}] = { [Symbol.iterator]() { return { next() { return { value: null, done: false }; }, return() { closed++; return {}; } }; } }; } catch (e) {} closed", "1");
    eq("try { (({}) => 1)(null); 'no' } catch (e) { e.name }", "TypeError");
    // yield* forwards throw/return and hands on the inner result objects.
    eq("var log = []; function* inner() { try { yield 1; yield 2; } finally { log.push('in'); } } function* outer() { try { var r = yield* inner(); log.push('r' + r); } finally { log.push('out'); } } var it = outer(); it.next(); var x = it.return(9); log.join() + ':' + x.value + x.done", "in,out:9true");
    eq("function* inner() { try { yield 1; } catch (e) { yield 'caught ' + e; } } function* outer() { yield* inner(); } var it = outer(); it.next(); it.throw('x').value", "caught x");
    eq("var res = { value: 1, done: false }; var inner = { [Symbol.iterator]() { return { next() { return res; } }; } }; function* g() { yield* inner; } g().next() === res", "true");
    eq("function* g() { return yield* [1, 2]; } [...g()].join()", "1,2");
    // References inside `with` resolve once, before the right-hand side.
    eq("var x = 0; var scope = { x: 1 }; with (scope) { x += (delete scope.x, 2); } scope.x + ',' + x", "3,0");
    eq("var x = 0; var scope = { x: 1 }; with (scope) { x = (delete scope.x, 2); } scope.x + ',' + x", "2,0");
    eq("function f() { var x = 0; var inner = (function () { x = (eval('var x;'), 1); return x; })(); return inner + ',' + x; } f()", "undefined,1");
    eq("var o = { n: 1 }; with (o) { n++; } o.n", "2");
    // Parameter expressions get their own environment.
    eq("var x = 'outside'; var pp, pb; function f(_ = pp = function () { return x; }) { var x = 'inside'; pb = function () { return x; }; } f(); pp() + ',' + pb()", "outside,inside");
    eq("function f(a, b = () => a) { var a; return a + ',' + b(); } f(1)", "1,1");
    eq("function f(a = 1, ...r) { function g() { return a; } return g() + r.length; } f(undefined, 2, 3)", "3");
    eq("function f(a = b, b) {} try { f(); 'no' } catch (e) { e.name }", "ReferenceError");
    eq("function f(a, b = a + 1) { a = 5; return a + b; } f(1)", "7");
    eq("var g = (x = () => y, y = 2) => x(); g()", "2");
    eq("function f(p = eval('var arguments')) {} try { f(); 'no' } catch (e) { e.name }", "SyntaxError");
    eq("var k = { toString() { throw 1; } }; var b = null; try { b[k] ^= 1; 'no' } catch (e) { typeof e === 'object' ? e.name : 'key' }", "TypeError");
    eq("var it = { [Symbol.iterator]() { return { next() { return { done: false }; }, return() { throw 42; } }; } }; var c = 0, f = 0; function t() { for (var x of it) { try { return 1; } catch (e) { c++; } finally { f++; } } } var r; try { t(); } catch (e) { r = e; } [r, c, f].join()", "42,0,1");
    eq("String([eval('1; do { } while (false)'), eval('2; do { 3; } while (false)'), eval('1; if (true) {}'), eval('1; {}')])", ",3,,1");
    // Lone surrogates in string literals (and eval'd source) survive.
    eq(r#"['\uD800'.length, '\uD800'.charCodeAt(0), '\u{DC00}'.charCodeAt(0), '😀'.length, `\uD800`.charCodeAt(0), eval("'\\uD800'").charCodeAt(0), eval("'" + '\uDBFF' + "'").charCodeAt(0)].join()"#, "1,55296,56320,2,55296,55296,56319");
    eq("function f(p = eval('var q = 1'), r = q) { return r; } f()", "1");
    // Assignment targets are evaluated before the iterator steps.
    eq("var log = []; var o = { set x(v) { log.push('set ' + v); } }; function t() { log.push('ref'); return o; } var it = { [Symbol.iterator]() { return { next() { log.push('next'); return { value: 1, done: false }; }, return() { log.push('ret'); return {}; } }; } }; [t().x] = it; log.join()", "ref,next,set 1,ret");
    eq("var closed = 0; var it = { [Symbol.iterator]() { return { next() { return { done: false }; }, return() { closed++; return {}; } }; } }; function th() { throw 'lhs'; } try { [...{}[th()]] = it; } catch (e) {} closed", "1");
    // Derived constructors: primitive returns are TypeErrors thrown to the caller.
    eq("class B {} class C extends B { constructor() { super(); try { return 1; } catch (e) { return {caught: true}; } } } try { new C(); 'no' } catch (e) { e.name }", "TypeError");
    eq("class B {} class C extends B { constructor() { try { return; } catch (e) {} } } try { new C(); 'no' } catch (e) { e.name }", "ReferenceError");
    eq("class B { constructor() { return 1; } } typeof new B()", "object");
    // Indirect eval: lexical declarations stay local, vars are deletable globals.
    eq("var a = (0, eval)('let ie = 1; ie + 1'); [a, typeof ie].join()", "2,undefined");
    eq("(0, eval)('var iv = 2; function ifn() { return this; }'); [iv, delete iv, typeof iv, ifn() === globalThis].join()", "2,true,undefined,true");
    eq("try { (0, eval)('let tz = tz;'); 'no' } catch (e) { e.name }", "ReferenceError");
    eq("var r = []; for (var i = 0; i < 2; i++) { try { throw 1; } finally { r.push(i); continue; } } r.join()", "0,1");
}

/// Unicode identifiers: Other_ID_Start (U+2118, in ESPN's bundle),
/// combining marks and connector punctuation inside names.
#[test]
fn unicode_identifier_characters() {
    eq("var \u{2118} = 2; var a\u{2118} = 3; \u{2118} + a\u{2118}", "5");
    eq("var e\u{301} = 4; var x\u{203f}y = 5; e\u{301} + x\u{203f}y", "9");
    eq("var \u{212e}\u{b7} = 1; \u{212e}\u{b7}", "1");
}

/// Direct `eval` sees the calling function's bindings (SunSpider's
/// date-format-tofte calls functions declared after a `return` from eval
/// code built at run time); functions it creates close over that scope.
#[test]
fn direct_eval_sees_caller_scope() {
    eq("(function () { return eval('Y()'); function Y() { return 'ok'; } })()", "ok");
    eq("(function () { var v = 'v'; let l = 'l'; return eval('v + l'); })()", "vl");
    eq("(function (p) { return eval('p'); })('param')", "param");
    eq("var o = { k: 1, m: function (s) { var self = this; function Y() { return self === o; } var a = s.split(''); for (var i = 0; i < a.length; i++) a[i] = eval(a[i] + '()'); return a.join(); } }; o.m('YY')", "true,true");
    eq("Date.prototype.fmt = function (s) { var self = this; var a = s.split(''); for (var i = 0; i < a.length; i++) a[i] = eval(a[i] + '()'); return a.join(''); function Y() { return self instanceof Date; } }; new Date().fmt('Y')", "true");
    eq("(function () { var a = 1; eval('a = 5'); return a; })()", "5");
    eq("(function () { var a = 1; eval('var a = 7'); return a; })()", "7");
    eq("(function () { var n = 10; var f = eval('(function () { return ++n; })'); f(); return f() + ':' + n; })()", "12:12");
    eq("(function () { var a = 1; return (function () { var b = 2; return (function () { return eval('a + b'); })(); })(); })()", "3");
    eq("(function () { let r = 0; { let b = 4; r = eval('b * 2'); } return r; })()", "8");
    eq("(function () { var fs = []; for (let i = 0; i < 3; i++) fs.push(eval('() => i')); return fs.map(f => f()).join(); })()", "0,1,2");
    eq("(function (a, b) { return eval('arguments.length + a + b'); })(1, 2)", "5");
    eq("var o = { v: 9, m: function () { return eval('this.v'); } }; o.m()", "9");
    eq("var o = { k: 42, m: function () { return (() => eval('this.k'))(); } }; o.m()", "42");
    eq("(function () { function* g() { var k = 3; yield eval('k * 2'); } return g().next().value; })()", "6");
    eq("class C { constructor() { this.z = 5; } m() { var w = 2; return eval('this.z * w'); } } new C().m()", "10");
    eq("(function () { var a = 2; return eval(\"eval('a * 3')\"); })()", "6");
    eq("(function () { try { throw 7; } catch (err) { return eval('err + 1'); } })()", "8");
    // Lexical declarations stay inside the eval; strict eval keeps vars too.
    eq("(function () { eval('let q = 1'); eval('let q = 2'); return typeof q; })()", "undefined");
    eq("(function () { 'use strict'; eval('var s1 = 1'); return typeof s1; })()", "undefined");
    // Sloppy vars the caller does not declare become globals (as before).
    eq("(function () { eval('var zz1 = 3'); })(); typeof zz1", "undefined");
    eq("(function () { return eval('1; 2; if (true) { 3 }'); })()", "3");
    // Indirect eval stays global.
    eq("(function () { var loc = 1; var e = eval; return e('typeof loc'); })()", "undefined");
    throws("(function () { eval('var 1x'); })()", "SyntaxError");
}

/// Map / Set hash table: SameValueZero keys, insertion order, deletion and
/// compaction while iterators are live.
#[test]
fn map_set_keys_and_iteration() {
    eq("new Map([[-0, 'a']]).get(0) + new Map([[0, 'b']]).get(-0)", "ab");
    eq("Object.is([...new Set([-0])][0], 0) && Object.is(new Map().set(-0, 1).keys().next().value, 0)", "true");
    eq("var m = new Map([[NaN, 1]]); m.set(NaN, 2); m.get(NaN) + ':' + m.size + ':' + new Set([NaN, 0 / 0, NaN]).size", "2:1:1");
    eq("var m = new Map(); m.set('a' + 'b', 1); m.get('ab') + ':' + m.has('a' + 'b') + ':' + m.has('ba')", "1:true:false");
    eq("var k = {}, m = new Map([[k, 1], [{}, 2]]); m.get(k) + ':' + m.get({}) + ':' + new Map([[1n, 'x']]).get(1n)", "1:undefined:x");
    eq("var m = new Map([['1', 's'], [1, 'n'], [true, 'b'], [null, 'z'], [undefined, 'u']]); [m.get('1'), m.get(1), m.get(true), m.get(null), m.get(undefined), m.size].join()", "s,n,b,z,u,5");
    eq("var m = new Map([[1, 1], [2, 2], [3, 3]]); var out = []; for (var [k] of m) { out.push(k); if (k === 1) m.delete(2); } out.join()", "1,3");
    eq("var s = new Set([1]); var out = []; for (var v of s) { out.push(v); if (v < 4) s.add(v + 1); } out.join()", "1,2,3,4");
    eq("var s = new Set([1, 2, 3]); var it = s.values(); it.next(); s.clear(); s.add(9); it.next().value + ':' + s.size", "9:1");
    eq("var m = new Map(); for (let i = 0; i < 100; i++) m.set(i, i); var it = m.keys(); it.next(); for (let i = 0; i < 90; i++) m.delete(i); [...it].join() + ':' + m.size", "90,91,92,93,94,95,96,97,98,99:10");
    eq("var m = new Map(); for (let i = 0; i < 1000; i++) { m.set(i, i); m.delete(i - 1); } [...m.keys()].join() + ':' + m.size + ':' + m.get(999)", "999:1:999");
    eq("var s = new Set([1]); var it = s.values(); it.next(); it.next(); s.add(2); it.next().done", "true");
    eq("var m = new Map([[1, 'a'], [2, 'b'], [3, 'c']]); var seen = []; m.forEach(function (v, k) { seen.push(v); if (k === 1) { m.delete(2); m.set(4, 'd'); } }); seen.join()", "a,c,d");
    eq("var s = new Set(); for (let i = 0; i < 50; i++) s.add('k' + i); for (let i = 0; i < 45; i++) s.delete('k' + i); s.add('k0'); [...s].join()", "k45,k46,k47,k48,k49,k0");
    eq("var m = new Map(); for (let i = 0; i < 40; i++) m.set(i, i); for (let i = 0; i < 39; i++) m.delete(i); var c = structuredClone(m); c.size + ':' + c.get(39)", "1:39");
    eq("var s = new Set([3, 1, 2]); s.delete(1); s.add(1); [...s].join() + ':' + [...s.entries()].map(e => e.join('=')).join()", "3,2,1:3=3,2=2,1=1");
}

/// Iteration fast paths (array / generator / string / Map iterators driven
/// without result objects) behave like the protocol.
#[test]
fn iterator_fast_paths() {
    eq("Array.prototype[Symbol.iterator] = function* () { yield 42; }; var [a] = [1]; a", "42");
    eq("Object.getPrototypeOf([][Symbol.iterator]()).next = function () { return { done: true }; }; var r = []; for (var x of [1, 2]) r.push(x); r.length", "0");
    eq("var [a, b] = [, 2]; String(a) + b", "undefined2");
    eq("Array.prototype[0] = 'p'; var [x] = [,]; x", "p");
    eq("var arr = [1]; Object.defineProperty(arr, 1, { get() { return 'g'; } }); [...arr].join()", "1,g");
    eq("function* g() { yield 1; return 2; } [...g()].join()", "1");
    eq("[...'a\u{1F600}b'].length", "3");
    eq("var m = new Map([[1, 'a'], [2, 'b']]); var r = []; for (const [k, v] of m) r.push(k + v); r.join()", "1a,2b");
    eq("function f() { return [...arguments].join(); } f(1, 2, 3)", "1,2,3");
    eq("function* g() { var x = yield 1; yield x * 2; } var it = g(); it.next(); it.next(21).value", "42");
    eq("var log = []; var it = { [Symbol.iterator]() { return { i: 0, next() { return { value: this.i, done: this.i++ > 2 }; }, return() { log.push('closed'); return {}; } }; } }; for (var v of it) { if (v === 1) break; } log.join()", "closed");
    eq("function* g() { try { yield 1; yield 2; } finally { } } var r = []; for (var v of g()) { r.push(v); break; } r.join()", "1");
    eq("var a = [1, 2, 3]; var r = []; for (var v of a) { r.push(v); if (v === 1) a.push(4); } r.join()", "1,2,3,4");
    eq("var it = [1, 2][Symbol.iterator](); it.next(); it.next(); it.next(); it.next().done", "true");
    eq("var [p, ...q] = 'xyz'; p + q.join('')", "xyz");
}

/// Regular expression fast paths: compiled case folding, class bitmaps,
/// start-character skipping, anchors, and the result-free replace / match
/// / test paths.
#[test]
fn regexp_fast_paths() {
    eq("/AbC/i.test('xaBc') + ':' + /[a-z]+/i.exec('12ABC')[0] + ':' + /[^a-z]/i.test('Q')", "true:ABC:false");
    eq("/\\u212A/i.test('k') + ':' + /\\u212A/iu.test('k') + ':' + /\u{df}/i.test('SS')", "false:true:false");
    eq("var r = /a/y; r.lastIndex = 1; var t1 = r.test('ba'); var li = r.lastIndex; var t2 = r.test('ba'); t1 + ':' + li + ':' + t2 + ':' + r.lastIndex", "true:2:false:0");
    eq("/^b/.test('ab') + ':' + /^b/m.test('a\\nb') + ':' + 'aab'.replace(/^a/g, 'x')", "false:true:xab");
    eq("/[xy]z|q/.exec('aaqxz').index + ':' + /[\u{e9}]/.test('caf\u{e9}') + ':' + /\\bfoo/.exec('a foo').index", "2:true:2");
    eq("'a-b-c'.replace(/-/g, '+') + ':' + 'abc'.replace(/b/g, '[$&]') + ':' + 'abc'.replace(/x*/g, '-')", "a+b+c:a[b]c:-a-b-c-");
    eq("'aXbX'.match(/x/gi).join() + ':' + 'abc'.match(/z/g) + ':' + 'abc'.match(/x*/g).length", "X,X:null:4");
    eq("var r = /b/g; r.lastIndex = 2; 'abcb'.match(r).length + ':' + r.lastIndex", "2:0");
    eq("/./u.exec('\u{1F600}')[0].length + ':' + '\u{1F600}\u{1F600}'.replace(/(?:)/gu, '-').length", "2:7");
    eq("('agggtaaaxTTTACCCT'.match(/agggtaaa|tttaccct/ig) || []).length", "2");
    eq("'x'.replace(/(x)/, '$1$1') + ':' + 'abc'.replace(/(?<l>b)/, '[$<l>]')", "xx:a[b]c");
    eq("var r = /a/g; r.exec('aa'); r.test('aa') + ':' + r.lastIndex", "true:2");
    eq("/(b)/.test('abc'); RegExp.$1", "b");
}

/// WeakMap / WeakSet hold their keys weakly (ephemerons): an entry whose
/// key is otherwise unreachable is dropped by a collection, and a value is
/// kept alive by its entry while the key is.
#[test]
fn weak_collections_drop_entries_with_their_keys() {
    let mut realm = Realm::new();
    let src = "var wm = new WeakMap(), ws = new WeakSet(); var keep = {};
        (function () { var k2 = {}; wm.set(keep, { inner: k2 }); wm.set(k2, 'chained'); })();
        (function () { for (var i = 0; i < 1000; i++) { var o = {}; wm.set(o, [i]); ws.add(o); } })();
        ws.add(keep); [wm, ws]";
    let v = realm.eval(src, "weak.js").unwrap();
    let arr = v.as_object().unwrap();
    let (wm, ws) = match &realm.obj(arr).kind {
        crate::heap::ObjectKind::Array { elements, .. } => (elements[0].as_object().unwrap(), elements[1].as_object().unwrap()),
        _ => unreachable!(),
    };
    let size = |realm: &Realm, id| match &realm.obj(id).kind {
        crate::heap::ObjectKind::WeakMap(m) | crate::heap::ObjectKind::WeakSet(m) => m.size(),
        _ => usize::MAX,
    };
    assert_eq!(size(&realm, wm), 1002);
    let _root = realm.root(v);
    realm.gc();
    assert!(size(&realm, wm) <= 3, "WeakMap kept {} entries", size(&realm, wm));
    assert!(size(&realm, ws) <= 2, "WeakSet kept {} entries", size(&realm, ws));
    let r = realm.eval("wm.get(wm.get(keep).inner) + ':' + ws.has(keep) + ':' + wm.has({})", "weak2.js").unwrap();
    assert_eq!(realm.to_string(r).unwrap().to_rust_string(), "chained:true:false");
}
