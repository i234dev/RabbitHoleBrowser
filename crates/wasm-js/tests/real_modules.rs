//! Real-world modules (not vendored): an Emscripten build (sql.js) and a
//! Rust/wasm-bindgen build (@resvg/resvg-wasm). Fetch them with
//! `scripts`-free curl into `RH_WASM_REAL` (default
//! `/mnt/hdd5tb/rabbithole/suites/wasm-real`):
//!
//! ```text
//! sqljs/sql-wasm.js, sqljs/sql-wasm.wasm   (https://unpkg.com/sql.js@1.10.3/dist/)
//! resvg/index.min.js, resvg/index_bg.wasm  (https://unpkg.com/@resvg/resvg-wasm@2.6.2/)
//! ```
//!
//! `cargo test --release -p rabbithole-wasm-js --test real_modules -- --ignored`

use rabbithole_js::Realm;
use std::path::PathBuf;

const TEXT_CODEC: &str = r#"
globalThis.TextEncoder = class TextEncoder {
  get encoding() { return 'utf-8'; }
  encode(s = '') {
    s = String(s);
    const out = [];
    for (let i = 0; i < s.length; i++) {
      let c = s.codePointAt(i);
      if (c > 0xffff) i++;
      if (c < 0x80) out.push(c);
      else if (c < 0x800) out.push(0xc0 | (c >> 6), 0x80 | (c & 63));
      else if (c < 0x10000) out.push(0xe0 | (c >> 12), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
      else out.push(0xf0 | (c >> 18), 0x80 | ((c >> 12) & 63), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
    }
    return new Uint8Array(out);
  }
  encodeInto(s, dest) {
    const b = this.encode(s);
    const n = Math.min(b.length, dest.length);
    dest.set(b.subarray(0, n));
    return { read: s.length, written: n };
  }
};
globalThis.TextDecoder = class TextDecoder {
  constructor() { this.encoding = 'utf-8'; }
  decode(buf) {
    if (buf === undefined) return '';
    const b = buf instanceof Uint8Array ? buf : new Uint8Array(buf.buffer || buf, buf.byteOffset || 0, buf.byteLength);
    let s = '';
    for (let i = 0; i < b.length;) {
      let c = b[i++];
      if (c >= 0xf0) c = ((c & 7) << 18) | ((b[i++] & 63) << 12) | ((b[i++] & 63) << 6) | (b[i++] & 63);
      else if (c >= 0xe0) c = ((c & 15) << 12) | ((b[i++] & 63) << 6) | (b[i++] & 63);
      else if (c >= 0xc0) c = ((c & 31) << 6) | (b[i++] & 63);
      s += String.fromCodePoint(c);
    }
    return s;
  }
};
var out = [];
"#;

fn dir() -> Option<PathBuf> {
    let d = std::env::var("RH_WASM_REAL").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/mnt/hdd5tb/rabbithole/suites/wasm-real"));
    d.exists().then_some(d)
}

fn run(files: &[(&str, &str)], scripts: &[&str], driver: &str) -> String {
    let d = dir().unwrap();
    let mut r = Realm::new();
    rabbithole_wasm_js::install(&mut r);
    r.eval(TEXT_CODEC, "codec.js").unwrap();
    for (name, path) in files {
        let bytes = std::fs::read(d.join(path)).unwrap();
        let list: Vec<String> = bytes.iter().map(|b| b.to_string()).collect();
        r.eval(&format!("var {name} = new Uint8Array([{}]);", list.join(",")), "bytes.js").unwrap();
    }
    for s in scripts {
        let src = std::fs::read_to_string(d.join(s)).unwrap();
        if let Err(e) = r.eval(&src, s) {
            panic!("{s}: {}", r.error_to_string(&e));
        }
    }
    if let Err(e) = r.eval(driver, "driver.js") {
        panic!("driver: {}", r.error_to_string(&e));
    }
    r.run_jobs();
    let v = r.eval("out.join('\\n')", "out.js").unwrap();
    r.to_rust_string(v).unwrap()
}

#[test]
#[ignore]
fn emscripten_sqljs() {
    if dir().is_none() {
        return;
    }
    let out = run(
        &[("sqlwasm", "sqljs/sql-wasm.wasm")],
        &["sqljs/sql-wasm.js"],
        r#"
        initSqlJs({ wasmBinary: sqlwasm }).then(SQL => {
          const db = new SQL.Database();
          db.run("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT, score REAL);");
          const stmt = db.prepare("INSERT INTO t (name, score) VALUES (?, ?)");
          for (let i = 0; i < 1000; i++) stmt.run(["n" + i, i * 1.5]);
          stmt.free();
          out.push(JSON.stringify(db.exec("SELECT count(*), sum(score), max(name) FROM t WHERE id % 3 = 0")[0].values));
          out.push(db.exec("SELECT group_concat(name, ',') FROM (SELECT name FROM t ORDER BY score DESC LIMIT 3)")[0].values[0][0]);
        }).catch(e => out.push('error ' + e + ' ' + e.stack));
        "#,
    );
    assert_eq!(out, "[[333,249750,\"n998\"]]\nn999,n998,n997");
}

/// The rollup parser the svelte.dev playground's bundler worker loads
/// (`svelte/rollup_bg.wasm` from
/// https://svelte.dev/_app/immutable/workers/assets/bindings_wasm_bg-D3gYxQWQ.wasm).
#[test]
#[ignore]
fn svelte_playground_rollup_wasm() {
    if dir().is_none() {
        return;
    }
    let out = run(
        &[("rollupwasm", "svelte/rollup_bg.wasm")],
        &[],
        r#"
        const m = new WebAssembly.Module(rollupwasm);
        const imports = {};
        for (const i of WebAssembly.Module.imports(m)) {
          imports[i.module] = imports[i.module] || {};
          imports[i.module][i.name] = i.kind === 'function' ? () => 0 : undefined;
        }
        const inst = new WebAssembly.Instance(m, imports);
        out.push(String(WebAssembly.Module.exports(m).length > 5), typeof inst.exports.memory);
        "#,
    );
    assert_eq!(out, "true\nobject");
}

#[test]
#[ignore]
fn wasm_bindgen_resvg() {
    if dir().is_none() {
        return;
    }
    let out = run(
        &[("resvgwasm", "resvg/index_bg.wasm")],
        &["resvg/index.min.js"],
        r##"
        resvg.initWasm(resvgwasm).then(() => {
          const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="64" height="48"><rect x="4" y="4" width="40" height="30" fill="#c33"/></svg>';
          const img = new resvg.Resvg(svg, { background: 'white' }).render();
          const png = img.asPng();
          out.push(img.width + 'x' + img.height, Array.from(png.subarray(0, 8)).join(','), String(png.length > 100));
        }).catch(e => out.push('error ' + e + ' ' + e.stack));
        "##,
    );
    assert_eq!(out, "64x48\n137,80,78,71,13,10,26,10\ntrue");
}
