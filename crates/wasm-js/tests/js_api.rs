//! WebAssembly JavaScript API tests (modules written in WAT, run from JS).

use rabbithole_js::{Realm, Value};

fn realm() -> Realm {
    let mut r = Realm::new();
    rabbithole_wasm_js::install(&mut r);
    r
}

/// Define `name` as a Uint8Array with the binary of a WAT module.
fn define_module(r: &mut Realm, name: &str, wat: &str) {
    let bin = rabbithole_wasm::text::wat_to_binary(wat).expect("wat");
    let list: Vec<String> = bin.iter().map(|b| b.to_string()).collect();
    r.eval(&format!("var {name} = new Uint8Array([{}]);", list.join(",")), "setup.js").unwrap();
}

fn eval_str(r: &mut Realm, src: &str) -> String {
    match r.eval(src, "test.js") {
        Ok(v) => r.to_rust_string(v).unwrap(),
        Err(e) => format!("THROW {}", r.error_to_string(&e)),
    }
}

#[test]
fn namespace_and_validate() {
    let mut r = realm();
    define_module(&mut r, "bytes", "(module (func (export \"f\") (result i32) i32.const 42))");
    assert_eq!(eval_str(&mut r, "String(WebAssembly)"), "[object WebAssembly]");
    assert_eq!(eval_str(&mut r, "String(WebAssembly.validate(bytes))"), "true");
    assert_eq!(eval_str(&mut r, "String(WebAssembly.validate(new Uint8Array([0,97,115,109,1,0,0,0,99])))"), "false");
    assert_eq!(eval_str(&mut r, "typeof WebAssembly.CompileError + (new WebAssembly.LinkError('x') instanceof Error)"), "functiontrue");
    assert_eq!(eval_str(&mut r, "try { new WebAssembly.Module(new Uint8Array([1,2,3])) } catch (e) { e instanceof WebAssembly.CompileError ? 'ok' : String(e) }"), "ok");
}

#[test]
fn call_exports_and_imports() {
    let mut r = realm();
    define_module(
        &mut r,
        "bytes",
        r#"(module
            (import "env" "log" (func $log (param i32 f64) (result i32)))
            (func (export "add") (param i32 i32) (result i32) (i32.add (local.get 0) (local.get 1)))
            (func (export "big") (param i64) (result i64) (i64.mul (local.get 0) (i64.const 3)))
            (func (export "callJs") (param i32) (result i32) (call $log (local.get 0) (f64.const 2.5)))
            (func (export "pair") (result i32 f32) (i32.const 7) (f32.const 1.5)))"#,
    );
    let out = eval_str(
        &mut r,
        r#"
        var seen = [];
        var m = new WebAssembly.Module(bytes);
        var i = new WebAssembly.Instance(m, { env: { log: (a, b) => { seen.push(a, b); return a * 10; } } });
        var e = i.exports;
        [e.add(2, 3), e.add.length, typeof e.add.name, e.big(5n) === 15n, e.big(-1n) === -3n, e.callJs(4), seen.join(','),
         e.pair().join(','), Object.isFrozen(e), Object.getPrototypeOf(e) === null, i.exports === e,
         WebAssembly.Module.exports(m).map(x => x.name + ':' + x.kind).join(' '),
         WebAssembly.Module.imports(m).map(x => x.module + '.' + x.name + ':' + x.kind).join(' ')].join('|')
        "#,
    );
    assert_eq!(out, "5|2|string|true|true|40|4,2.5|7,1.5|true|true|true|add:function big:function callJs:function pair:function|env.log:function");
}

#[test]
fn memory_table_global() {
    let mut r = realm();
    define_module(
        &mut r,
        "bytes",
        r#"(module
            (memory (export "mem") 1 4)
            (table (export "tab") 2 funcref)
            (global (export "g") (mut i32) (i32.const 5))
            (func $seven (export "seven") (result i32) (i32.const 7))
            (elem (i32.const 0) $seven)
            (func (export "load") (param i32) (result i32) (i32.load8_u (local.get 0)))
            (func (export "grow") (param i32) (result i32) (memory.grow (local.get 0)))
            (func (export "getg") (result i32) (global.get 0))
            (func (export "calltab") (param i32) (result i32) (call_indirect (result i32) (local.get 0))))"#,
    );
    let out = eval_str(
        &mut r,
        r#"
        var e = new WebAssembly.Instance(new WebAssembly.Module(bytes), {}).exports;
        var b = e.mem.buffer;
        var res = [b.byteLength, e.mem.buffer === b];
        new Uint8Array(b)[100] = 77;
        res.push(e.load(100));
        res.push(e.mem.grow(1), b.byteLength, e.mem.buffer.byteLength, e.mem.buffer !== b);
        res.push(e.grow(1), e.mem.buffer.byteLength / 65536, new Uint8Array(e.mem.buffer)[100]);
        res.push(e.tab.length, e.tab.get(0)(), e.tab.get(0) === e.seven, e.tab.get(1));
        e.tab.set(1, e.seven);
        res.push(e.calltab(1));
        res.push(e.g.value, (e.g.value = 9, e.getg()), e.g instanceof WebAssembly.Global);
        var g2 = new WebAssembly.Global({value: 'i64', mutable: true}, 4n);
        res.push(typeof g2.value, g2.value === 4n);
        var t = new WebAssembly.Table({element: 'anyfunc', initial: 1});
        res.push(t.grow(2), t.length);
        var mem = new WebAssembly.Memory({initial: 1, maximum: 2});
        res.push(mem.buffer.byteLength);
        try { mem.grow(5); } catch (x) { res.push(x instanceof RangeError); }
        res.join('|')
        "#,
    );
    assert_eq!(out, "65536|true|77|1|0|131072|true|2|3|77|2|7|true||7|5|9|true|bigint|true|1|3|65536|true");
}

#[test]
fn traps_and_exceptions() {
    let mut r = realm();
    define_module(
        &mut r,
        "bytes",
        r#"(module
            (tag $t (export "t") (param i32))
            (func (export "trap") unreachable)
            (func (export "div") (param i32) (result i32) (i32.div_s (i32.const 1) (local.get 0)))
            (func $rec (export "rec") (call $rec))
            (func (export "throw") (param i32) (throw $t (local.get 0)))
            (func (export "catchSelf") (param i32) (result i32)
              (block $h (result i32)
                (try_table (catch $t $h) (throw $t (local.get 0)))
                (i32.const -1))))"#,
    );
    let out = eval_str(
        &mut r,
        r#"
        var e = new WebAssembly.Instance(new WebAssembly.Module(bytes), {}).exports;
        var res = [];
        try { e.trap(); } catch (x) { res.push(x instanceof WebAssembly.RuntimeError, x.message); }
        try { e.div(0); } catch (x) { res.push(x.message); }
        try { e.rec(); } catch (x) { res.push(x instanceof RangeError); }
        try { e.throw(42); } catch (x) { res.push(x instanceof WebAssembly.Exception, x.is(e.t), x.getArg(e.t, 0)); }
        res.push(e.catchSelf(5));
        res.join('|')
        "#,
    );
    assert_eq!(out, "true|unreachable|integer divide by zero|true|true|true|42|5");
    // A wasm exception thrown by JS (through an import) is caught by wasm.
    define_module(
        &mut r,
        "bytes2",
        r#"(module
            (import "m" "t" (tag $t (param i32)))
            (import "m" "f" (func $f))
            (func (export "run") (result i32)
              (block $h (result i32)
                (try_table (catch $t $h) (call $f))
                (i32.const -1))))"#,
    );
    let out = eval_str(
        &mut r,
        r#"
        var tag = new WebAssembly.Tag({parameters: ['i32']});
        var i2 = new WebAssembly.Instance(new WebAssembly.Module(bytes2), {m: {t: tag, f: () => { throw new WebAssembly.Exception(tag, [99]); }}});
        String(i2.exports.run())
        "#,
    );
    assert_eq!(out, "99");
}

#[test]
fn async_instantiate() {
    let mut r = realm();
    define_module(&mut r, "bytes", "(module (func (export \"f\") (result i32) i32.const 42))");
    r.eval(
        r#"
        var out = [];
        WebAssembly.instantiate(bytes).then(({module, instance}) => {
            out.push(module instanceof WebAssembly.Module, instance.exports.f());
            return WebAssembly.instantiate(module);
        }).then(inst => out.push(inst instanceof WebAssembly.Instance));
        WebAssembly.compile(new Uint8Array([0])).catch(e => out.push(e instanceof WebAssembly.CompileError));
        out.push('sync');
        "#,
        "a.js",
    )
    .unwrap();
    r.run_jobs();
    assert_eq!(eval_str(&mut r, "out.join('|')"), "sync|true|42|true|true");
}

#[test]
fn watchdog_interrupts_loops() {
    let mut r = realm();
    define_module(&mut r, "bytes", "(module (func (export \"spin\") (loop $l (br $l))))");
    r.eval("var spin = new WebAssembly.Instance(new WebAssembly.Module(bytes)).exports.spin;", "s.js").unwrap();
    r.set_deadline(Some(std::time::Instant::now() + std::time::Duration::from_millis(200)));
    let t0 = std::time::Instant::now();
    let res = r.eval("try { spin(); 'returned' } catch (e) { 'caught' }", "spin.js");
    assert!(res.is_err(), "the timeout must not be catchable");
    assert!(r.timed_out());
    assert!(t0.elapsed() < std::time::Duration::from_secs(5));
    let _ = Value::Undefined;
}

/// wasm-bindgen glue refreshes cached views when `view.byteLength === 0`
/// after `memory.grow` detached the old buffer.
#[test]
fn views_of_detached_memory_are_empty() {
    let mut r = realm();
    let out = eval_str(
        &mut r,
        r#"
        var mem = new WebAssembly.Memory({initial: 1});
        var b = mem.buffer, v = new Int32Array(b), dv = new DataView(b);
        v[0] = 7;
        mem.grow(1);
        var err = '';
        try { dv.getInt32(0); } catch (x) { err = x.constructor.name; }
        [b.byteLength, v.byteLength, v.length, v[0], new Int32Array(mem.buffer)[0], mem.buffer.byteLength, b.detached, mem.buffer.detached, err].join('|')
        "#,
    );
    assert_eq!(out, "0|0|0||7|131072|true|false|TypeError");
}

#[test]
fn gc_objects_cross_js() {
    let mut r = realm();
    define_module(
        &mut r,
        "bytes",
        r#"(module
            (type $p (struct (field $x (mut i32)) (field $y i64)))
            (func (export "make") (param i32) (result anyref) (struct.new $p (local.get 0) (i64.const 7)))
            (func (export "getx") (param anyref) (result i32) (struct.get $p $x (ref.cast (ref $p) (local.get 0))))
            (func (export "i31") (param i32) (result anyref) (ref.i31 (local.get 0)))
            (func (export "id") (param anyref) (result anyref) (local.get 0))
            (func (export "isI31") (param anyref) (result i32) (ref.test (ref i31) (local.get 0))))"#,
    );
    let out = eval_str(
        &mut r,
        r#"
        var e = new WebAssembly.Instance(new WebAssembly.Module(bytes)).exports;
        var o = e.make(41);
        var host = {a: 1};
        [typeof o, e.getx(o), e.make(1) !== o, e.id(o) === o, e.i31(-5), e.isI31(12), e.isI31(host), e.id(host) === host, e.id('s')].join('|')
        "#,
    );
    assert_eq!(out, "object|41|true|true|-5|1|0|true|s");
}
