//! Kernels in WebAssembly (our interpreter) vs the same code in JavaScript
//! (our JS engine): `cargo run --release -p rabbithole-wasm-js --example wasm_bench`.

use rabbithole_js::Realm;
use std::time::Instant;

const KERNELS: &str = include_str!("../../wasm/tests/fixtures/kernels.wat");

const JS: &str = r#"
function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); }
function matmul(n) {
  const A = new Float64Array(n * n), B = new Float64Array(n * n), C = new Float64Array(n * n);
  for (let i = 0; i < n; i++) for (let j = 0; j < n; j++) { A[i * n + j] = i + j; B[i * n + j] = i - j; }
  for (let i = 0; i < n; i++) for (let j = 0; j < n; j++) {
    let s = 0;
    for (let k = 0; k < n; k++) s += A[i * n + k] * B[k * n + j];
    C[i * n + j] = s;
  }
  let tr = 0;
  for (let i = 0; i < n; i++) tr += C[i * (n + 1)];
  return tr;
}
function sha256(blocks, K) {
  const msg = new Uint8Array(blocks * 64);
  for (let i = 0; i < msg.length; i++) msg[i] = (i * 31) & 255;
  const M = new Int32Array(msg.buffer);
  const H = new Int32Array([0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]);
  const W = new Int32Array(64);
  const rotr = (x, n) => (x >>> n) | (x << (32 - n));
  for (let blk = 0; blk < blocks; blk++) {
    for (let t = 0; t < 16; t++) W[t] = M[blk * 16 + t];
    for (let t = 16; t < 64; t++) {
      const w15 = W[t - 15], w2 = W[t - 2];
      const s0 = rotr(w15, 7) ^ rotr(w15, 18) ^ (w15 >>> 3);
      const s1 = rotr(w2, 17) ^ rotr(w2, 19) ^ (w2 >>> 10);
      W[t] = (s0 + s1 + W[t - 16] + W[t - 7]) | 0;
    }
    let a = H[0], b = H[1], c = H[2], d = H[3], e = H[4], f = H[5], g = H[6], h = H[7];
    for (let t = 0; t < 64; t++) {
      const t1 = (h + (rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)) + ((e & f) ^ (~e & g)) + K[t] + W[t]) | 0;
      const t2 = ((rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)) + ((a & b) ^ (a & c) ^ (b & c))) | 0;
      h = g; g = f; f = e; e = (d + t1) | 0; d = c; c = b; b = a; a = (t1 + t2) | 0;
    }
    H[0] += a; H[1] += b; H[2] += c; H[3] += d; H[4] += e; H[5] += f; H[6] += g; H[7] += h;
  }
  return H[0];
}
"#;

fn time(realm: &mut Realm, label: &str, src: &str) -> (String, f64) {
    let t0 = Instant::now();
    let v = realm.eval(src, label).unwrap_or_else(|e| panic!("{label}: {}", realm.error_to_string(&e)));
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    (realm.to_rust_string(v).unwrap(), ms)
}

fn main() {
    let mut realm = Realm::new();
    rabbithole_wasm_js::install(&mut realm);
    let bin = rabbithole_wasm::text::wat_to_binary(KERNELS).expect("kernels.wat");
    let list: Vec<String> = bin.iter().map(|b| b.to_string()).collect();
    realm.eval(&format!("var bytes = new Uint8Array([{}]);", list.join(",")), "setup.js").unwrap();
    realm.eval("var w = new WebAssembly.Instance(new WebAssembly.Module(bytes)).exports; var ws = new WebAssembly.Instance(new WebAssembly.Module(bytes)).exports; var K = new Int32Array(ws.memory.buffer.slice(0, 256));", "setup.js").unwrap();
    realm.eval(JS, "kernels.js").unwrap();
    let cases = [("fib(30)", "w.fib(30)", "fib(30)"), ("matmul(128)", "w.matmul(128)", "matmul(128)"), ("sha256(4096 blocks)", "ws.sha256(4096)", "sha256(4096, K)")];
    println!("{:<22} {:>12} {:>12} {:>8}", "kernel", "wasm ms", "js ms", "js/wasm");
    for (name, wasm, js) in cases {
        let (rw, tw) = time(&mut realm, "wasm", wasm);
        let (rj, tj) = time(&mut realm, "js", js);
        let same = if rw == rj { "" } else { "  (results differ!)" };
        println!("{name:<22} {tw:>12.1} {tj:>12.1} {:>8.2}{same}  [{rw}]", tj / tw);
    }
}
