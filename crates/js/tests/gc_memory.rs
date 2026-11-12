//! Garbage made by calls, closures, exceptions, callbacks of built-ins and
//! generators is collected while the script runs: the peak memory of the
//! process stays bounded (it used to grow to hundreds of megabytes, because
//! collections were triggered by object counts only and never ran inside
//! a native function's callbacks).
//!
//! One test per binary (peak RSS is per process), with the engine's
//! allocator, whose byte accounting drives the collector.

#[global_allocator]
static ALLOC: rabbithole_js::alloc::RhAlloc = rabbithole_js::alloc::RhAlloc;

fn peak_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

#[test]
fn garbage_heavy_scripts_keep_peak_memory_bounded() {
    let Some(_) = peak_rss_kb() else { return };
    let cases: &[(&str, &str, &str)] = &[
        ("10M calls", "function f(x) { return x + 1; } var s = 0; for (var i = 0; i < 1e7; i++) s = f(s); s", "10000000"),
        ("1M closures", "function mk(n) { return function () { return n + 1; }; } var s = 0; for (var i = 0; i < 1e6; i++) s += mk(i)(); s", "500000500000"),
        ("1M throws", "var n = 0; for (var i = 0; i < 1e6; i++) { try { throw new Error('e' + i); } catch (e) { n += e.message.length > 0 ? 1 : 0; } } n", "1000000"),
        ("forEach callbacks", "var t = 0; new Array(500000).fill(0).forEach(function (_, i) { var o = { a: 'x' + i, b: [i] }; t += o.b[0] & 1; }); t", "250000"),
        ("300k WeakMap keys", "var wm = new WeakMap(), n = 0; for (var i = 0; i < 3e5; i++) { var k = {}; wm.set(k, new Array(20).fill(i)); n += wm.has(k) ? 1 : 0; } n", "300000"),
        ("generator bodies","function* g() { let [a, b] = [0, 1]; for (;;) { yield a; [a, b] = [b, (a + b) % 1000]; } } var n = 0, s = 0; for (const v of g()) { s += v; if (++n === 500000) break; } n", "500000"),
    ];
    for (name, src, expected) in cases {
        let mut realm = rabbithole_js::Realm::new();
        let before = peak_rss_kb().unwrap();
        let v = realm.eval(src, name).unwrap_or_else(|e| panic!("{name}: {}", realm.error_to_string(&e)));
        let out = realm.to_string(v).unwrap().to_rust_string();
        assert_eq!(&out, expected, "{name}");
        let grown = peak_rss_kb().unwrap().saturating_sub(before);
        eprintln!("{name}: peak RSS grew {grown} KB");
        assert!(grown < 24 * 1024, "{name}: peak RSS grew {grown} KB");
    }
}
