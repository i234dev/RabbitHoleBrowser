//! Builtin scripts compiled once per thread and run as fresh copies in
//! every realm (`Realm::eval_builtin`): each realm gets its own caches.
use crate::Realm;

/// Long enough for the lazy parse (function bodies compile on first call).
fn builtin() -> &'static str {
    static SRC: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SRC.get_or_init(|| {
        let mut s = String::from(
            r#"
var lib = (function () {
  'use strict';
  function getX(o) { return o.x; }
  function sum(list) { let t = 0; for (const o of list) t += getX(o); return t; }
  class Point { constructor(x, y) { this.x = x; this.y = y; } norm() { return Math.hypot(this.x, this.y); } }
  const tag = (s, ...v) => s.raw.join('|') + v.join(',');
  function outer(n) { function inner(k) { return k * n; } return inner(n + 1); }
  return { getX, sum, Point, tag, outer, made: [] };
})();
lib.made.push(typeof globalThis.realmMark);
"#,
        );
        s.push_str("/*");
        s.push_str(&"x".repeat(20 * 1024));
        s.push_str("*/\n");
        s
    })
}

fn run(realm: &mut Realm, src: &str) -> String {
    let v = realm.eval(src, "t").expect("eval");
    realm.to_rust_string(v).expect("string")
}

#[test]
fn copies_run_independently_in_each_realm() {
    let src: &'static str = builtin();
    let mut a = Realm::new();
    a.eval("var realmMark = 1", "a").unwrap();
    a.eval_builtin(src, "rabbithole:test-builtin.js").unwrap();
    // Fill the property caches with one object layout.
    assert_eq!(run(&mut a, "let s = 0; for (let i = 0; i < 20000; i++) s += lib.getX({x: 1}); s"), "20000");
    assert_eq!(run(&mut a, "lib.sum([{x: 1}, {x: 2}, {x: 3}])"), "6");
    let mut b = Realm::new();
    b.eval_builtin(src, "rabbithole:test-builtin.js").unwrap();
    // Other layouts in the second realm (the first's caches must not apply).
    assert_eq!(run(&mut b, "let s = 0; for (let i = 0; i < 20000; i++) s += lib.getX({y: 5, z: 6, x: 2}); s"), "40000");
    assert_eq!(run(&mut b, "lib.sum([{a: 0, x: 1}, {b: 0, c: 0, x: 2}])"), "3");
    assert_eq!(run(&mut b, "new lib.Point(3, 4).norm()"), "5");
    assert_eq!(run(&mut b, "lib.tag`a${1}b${2}c`"), "a|b|c1,2");
    assert_eq!(run(&mut b, "lib.outer(4)"), "20");
    assert_eq!(run(&mut b, "lib.made.join()"), "undefined");
    assert_eq!(run(&mut a, "lib.made.join()"), "number");
    // Each realm has its own functions and template objects.
    assert_eq!(run(&mut a, "lib.getX({x: 'a'}) + (new lib.Point(6, 8).norm())"), "a10");
    assert_eq!(run(&mut a, "lib.tag`a${1}b${2}c`"), "a|b|c1,2");
}

/// Functions of the embedder's built-in scripts print as built-in
/// functions; page functions keep their source text.
#[test]
fn builtin_script_functions_print_as_native_code() {
    let src: &'static str = builtin();
    let mut r = Realm::new();
    r.eval_builtin(src, "rabbithole:test-builtin.js").unwrap();
    assert_eq!(run(&mut r, "Function.prototype.toString.call(lib.getX)"), "function getX() { [native code] }");
    assert_eq!(run(&mut r, "String(lib.Point)"), "function Point() { [native code] }");
    assert_eq!(run(&mut r, "String(lib.tag)"), "function tag() { [native code] }");
    assert_eq!(run(&mut r, "String(new lib.Point(1, 2).norm)"), "function norm() { [native code] }");
    assert_eq!(run(&mut r, "function page(a) { return a; } String(page)"), "function page(a) { return a; }");
}
