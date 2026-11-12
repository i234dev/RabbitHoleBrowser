//! Run JavaScript files against the WebAssembly API (for real-world module
//! tests without a browser page):
//! `run_js name=file.wasm a.js b.js` defines `name` as a Uint8Array with the
//! file's bytes, then evaluates the scripts in order and drains the job
//! queue. A minimal TextEncoder/TextDecoder is provided.

use rabbithole_js::Realm;

const POLYFILL: &str = r#"
if (typeof TextEncoder === 'undefined') {
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
}
if (typeof TextDecoder === 'undefined') {
  globalThis.TextDecoder = class TextDecoder {
    constructor(label = 'utf-8', opts = {}) { this.encoding = 'utf-8'; }
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
}
"#;

fn main() {
    let mut realm = Realm::new();
    rabbithole_wasm_js::install(&mut realm);
    realm.set_console_sink(Box::new(|level, text| println!("[{level}] {text}")));
    realm.eval(POLYFILL, "polyfill.js").unwrap();
    for a in std::env::args().skip(1) {
        if let Some((name, path)) = a.split_once('=') {
            let bytes = std::fs::read(path).expect("read bytes");
            let list: Vec<String> = bytes.iter().map(|b| b.to_string()).collect();
            realm.eval(&format!("var {name} = new Uint8Array([{}]);", list.join(",")), "bytes.js").unwrap();
            continue;
        }
        let src = std::fs::read_to_string(&a).expect("read script");
        let t0 = std::time::Instant::now();
        if let Err(e) = realm.eval(&src, &a) {
            println!("uncaught: {}", realm.error_to_string(&e));
        }
        realm.run_jobs();
        eprintln!("{a}: {:?}", t0.elapsed());
    }
    for r in realm.take_unhandled_rejections() {
        println!("unhandled rejection: {}", realm.error_to_string(&r));
    }
}
