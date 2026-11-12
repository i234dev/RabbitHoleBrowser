//! A Rust panic inside a native function becomes a catchable JavaScript
//! error at the call site; the interpreter's stacks are restored and the
//! realm keeps running.

use crate::realm::Realm;
use crate::value::Value;
use crate::JsResult;
use std::cell::RefCell;
use std::rc::Rc;

fn boom(_: &mut Realm, _: Value, _: &[Value]) -> JsResult<Value> {
    let v: Vec<u32> = Vec::new();
    let i = v.len() + 3;
    Ok(Value::Number(v[i] as f64))
}

/// Calls its argument (a JS function) and then panics: the frames of the
/// nested call are already gone; the panic happens with JS frames below.
fn call_then_panic(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let f = args.first().cloned().unwrap_or(Value::Undefined);
    realm.call(f, Value::Undefined, &[])?;
    panic!("deliberate test panic after callback");
}

/// Calls its argument; the callback itself calls the panicking native
/// (a panic below nested JS frames, caught at the innermost native).
fn call_arg(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let f = args.first().cloned().unwrap_or(Value::Undefined);
    realm.call(f, Value::Undefined, &[])
}

fn realm_with_hooks() -> (Realm, Rc<RefCell<Vec<(String, String)>>>) {
    let mut realm = Realm::new();
    let log = Rc::new(RefCell::new(Vec::new()));
    let l2 = log.clone();
    realm.set_console_sink(Box::new(move |lvl, text| l2.borrow_mut().push((lvl.to_string(), text))));
    let g = realm.global;
    realm.define_method(g, "__panicNow", 0, boom);
    realm.define_method(g, "__callThenPanic", 1, call_then_panic);
    realm.define_method(g, "__callArg", 1, call_arg);
    (realm, log)
}

fn eval_str(realm: &mut Realm, src: &str) -> String {
    match realm.eval(src, "panic-test.js") {
        Ok(v) => realm.to_string(v).map(|s| s.to_rust_string()).unwrap_or_else(|_| "<to_string threw>".into()),
        Err(e) => format!("uncaught: {}", realm.error_to_string(&e)),
    }
}

#[test]
fn panicking_native_throws_catchable_error() {
    let (mut realm, log) = realm_with_hooks();
    let out = eval_str(&mut realm, "var r; try { __panicNow(); r = 'no throw'; } catch (e) { r = (e instanceof Error) + ':' + e.message; } r");
    assert!(out.starts_with("true:internal error: index out of bounds"), "{out}");
    let log = log.borrow();
    let err = log.iter().find(|(l, _)| l == "error").expect("console error");
    assert!(err.1.contains("__panicNow") && err.1.contains("tests_panic.rs"), "{err:?}");
}

#[test]
fn realm_keeps_running_after_panic() {
    let (mut realm, _) = realm_with_hooks();
    // Uncaught: surfaces as an ordinary uncaught error of that script.
    let out = eval_str(&mut realm, "function f(a) { return __panicNow() + a; } f(1)");
    assert!(out.starts_with("uncaught: Error: internal error"), "{out}");
    // The next script runs normally, with locals, calls and loops intact.
    let out = eval_str(&mut realm, "function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); } var s = 0; for (var i = 0; i < 5; i++) { try { __panicNow(); } catch (e) { s += fib(10); } } s");
    assert_eq!(out, "275");
    // Panics while JS frames are nested inside natives (callback -> native
    // -> panic, and native -> callback -> return -> panic).
    let out = eval_str(&mut realm, "var a = []; [1, 2, 3].forEach(function (x) { try { __callArg(function () { return __panicNow(); }); } catch (e) { a.push(x); } }); a.join()");
    assert_eq!(out, "1,2,3");
    let out = eval_str(&mut realm, "var n = 0; try { __callThenPanic(function () { n++; }); } catch (e) { n += 10; } n");
    assert_eq!(out, "11");
    // Generators and async functions resumed below a panic.
    let out = eval_str(&mut realm, "function* g() { yield 1; __panicNow(); yield 2; } var it = g(); it.next(); var m; try { it.next(); } catch (e) { m = e.message; } [m.slice(0, 14), JSON.stringify(it.next())].join('|')");
    assert_eq!(out, "internal error|{\"done\":true}");
    let out = eval_str(&mut realm, "var got = 'pending'; (async function () { await 0; __panicNow(); })().catch(function (e) { got = e.message.slice(0, 14); }); 'ok'");
    assert_eq!(out, "ok");
    realm.run_jobs();
    assert_eq!(eval_str(&mut realm, "got"), "internal error");
    // Constructing through a panicking native.
    let out = eval_str(&mut realm, "try { new __panicNow(); } catch (e) { e.constructor.name + ':' + (e.message.indexOf('internal error') === 0); }");
    assert!(out == "Error:true" || out.starts_with("TypeError"), "{out}");
    assert_eq!(realm.frames.len(), 0);
    assert_eq!(realm.native_depth, 0);
    assert_eq!(realm.vstack.len(), 0);
}
