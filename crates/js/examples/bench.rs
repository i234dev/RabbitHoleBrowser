//! Quick performance probe: `cargo run --release -p rabbithole-js --example bench`
use rabbithole_js::Realm;
use std::time::Instant;

fn main() {
    let mut realm = Realm::new();
    let cases = [
        ("loop", "let s = 0; for (let i = 0; i < 1e6; i++) s += i; s"),
        ("calls", "function f(x) { return x + 1; } let s = 0; for (let i = 0; i < 300000; i++) s = f(s); s"),
        ("objects", "let a = []; for (let i = 0; i < 200000; i++) a.push({x: i, y: [i]}); a.length"),
        ("strings", "let s = ''; for (let i = 0; i < 100000; i++) s += 'ab'; s.length"),
        ("props", "const o = {a: 1, b: 2, c: 3}; let s = 0; for (let i = 0; i < 500000; i++) s += o.a + o.b + o.c; s"),
        ("array_map", "let a = Array.from({length: 200000}, (_, i) => i); a.map(x => x * 2).filter(x => x % 3 == 0).reduce((p, c) => p + c, 0)"),
    ];
    for (name, src) in cases {
        let t = Instant::now();
        let r = realm.eval(src, name);
        let dt = t.elapsed();
        match r {
            Ok(v) => println!("{:<10} {:>8.1} ms  -> {}", name, dt.as_secs_f64() * 1000.0, realm.to_string(v).unwrap()),
            Err(e) => println!("{:<10} ERROR {}", name, realm.error_to_string(&e)),
        }
    }
    println!("heap objects: {}", realm.heap.live_count);
}
