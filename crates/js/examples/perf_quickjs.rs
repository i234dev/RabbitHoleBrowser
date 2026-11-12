//! Benchmarks for the categories where QuickJS used to be clearly faster
//! (generators, destructuring, spread, closures, regular expressions incl.
//! regexp-dna, Map/Set, calls, exceptions), with peak memory:
//!
//! ```text
//! cargo run --release -p rabbithole-js --example perf_quickjs [filter]
//! cargo run --release -p rabbithole-js --example perf_quickjs -- --dump DIR
//! ```
//!
//! Every case runs in a fresh process (this binary with `--case NAME`), so
//! the peak RSS (VmHWM) is the case's own; the best CPU time of three runs
//! is reported. `--dump DIR` writes each case as `DIR/NAME.js` (ending in
//! `print(result)`) to run the same scripts on another engine.
#[global_allocator]
static ALLOC: rabbithole_js::alloc::RhAlloc = rabbithole_js::alloc::RhAlloc;

use rabbithole_js::{JsResult, Realm, Value};

const CASES: &[(&str, &str)] = &[
    (
        "generators",
        r#"
function* range(n) { for (let i = 0; i < n; i++) yield i; }
function* inner(n) { for (let i = 0; i < n; i++) yield i; }
function* outer() { for (let j = 0; j < 2000; j++) yield* inner(50); }
function* fibs() { let a = 0, b = 1; for (;;) { yield a; const t = a + b; a = b; b = t % 1000007; } }
let s = 0;
for (const v of range(300000)) s += v & 7;
const it = range(200000); let r; while (!(r = it.next()).done) s += r.value & 3;
for (const v of outer()) s += v;
let n = 0; for (const f of fibs()) { s += f & 1; if (++n === 200000) break; }
s += [...range(1000)].length;
s"#,
    ),
    (
        "destructuring",
        r#"
let s = 0;
const pts = []; for (let i = 0; i < 1000; i++) pts.push({ x: i, y: i * 2, z: [i, i + 1] });
for (let k = 0; k < 100; k++) for (const { x, y, z: [a, b] } of pts) s += (x + y + a + b) & 15;
function f({ a, b = 2 }, [c, d]) { return a + b + c + d; }
for (let i = 0; i < 200000; i++) s += f({ a: i }, [i, 1]) & 7;
let p = 1, q = 2;
for (let i = 0; i < 300000; i++) { [p, q] = [q, (p + 1) % 1000]; }
s += p + q;
for (let i = 0; i < 100000; i++) { const { a, ...rest } = { a: i, b: 1, c: 2 }; s += (a + rest.b) & 7; }
for (let i = 0; i < 100000; i++) { const [h, ...t] = [i, 1, 2, 3]; s += (h + t.length) & 7; }
s"#,
    ),
    (
        "spread",
        r#"
let s = 0;
function sum3(a, b, c) { return a + b + c; }
const arr = [1, 2, 3];
for (let i = 0; i < 300000; i++) s += sum3(...arr);
const big = []; for (let i = 0; i < 100; i++) big.push(i);
for (let i = 0; i < 10000; i++) { const c = [...big, i]; s += c.length; }
for (let i = 0; i < 20000; i++) s += Math.max(...big);
const o = { a: 1, b: 2, c: 3 };
for (let i = 0; i < 200000; i++) { const p = { ...o, d: i }; s += p.d & 1; }
for (let i = 0; i < 20000; i++) s += [..."hello world"].length;
function va(...args) { return args.length; }
function fw() { return va(...arguments); }
for (let i = 0; i < 200000; i++) s += fw(i, 1, 2);
s"#,
    ),
    (
        "closures",
        r#"
let s = 0;
function counter() { let n = 0; return { inc() { return ++n; }, get() { return n; } }; }
for (let i = 0; i < 100000; i++) { const c = counter(); c.inc(); c.inc(); s += c.get(); }
function adder(x) { return y => x + y; }
for (let i = 0; i < 300000; i++) s += adder(i)(1) & 7;
const fns = []; for (let i = 0; i < 1000; i++) fns.push(() => i);
for (let k = 0; k < 300; k++) for (let j = 0; j < 1000; j++) s += fns[j]() & 1;
function outer() { let a = 1, b = 2; function mid() { let c = 3; return function inner() { return a + b + c; }; } const f = mid(); let t = 0; for (let i = 0; i < 1000000; i++) t += f(); return t; }
s += outer();
function acc() { let total = 0; const add = v => { total += v; }; for (let i = 0; i < 1000000; i++) add(i & 15); return total; }
s += acc();
s"#,
    ),
    (
        "regexp",
        r#"
let s = 0;
const words = "The quick brown fox jumps over the lazy dog 12345 foo@example.com 2024-01-15".split(" ");
const text = words.join(" ").repeat(200);
for (let i = 0; i < 200; i++) s += (text.match(/\b\w+@\w+\.com\b/g) || []).length;
const dateRe = /(\d{4})-(\d{2})-(\d{2})/;
for (let i = 0; i < 100000; i++) { const m = dateRe.exec("on 2024-01-15 at"); s += +m[3]; }
for (let i = 0; i < 100000; i++) if (/^[a-z]+$/i.test(words[i % words.length])) s++;
for (let i = 0; i < 300; i++) s += text.replace(/o/g, "0").length;
for (let i = 0; i < 100; i++) s += text.replace(/(\w+)@(\w+)/g, (m, a, b) => b + "@" + a).length;
for (let i = 0; i < 300; i++) s += text.split(/\s+/).length;
for (let i = 0; i < 50000; i++) s += "  trim me  ".replace(/^\s+|\s+$/g, "").length;
for (let i = 0; i < 50000; i++) s += "a,b,,c;d e".split(/[,; ]/).length;
s"#,
    ),
    (
        "regexp_dna",
        // The regexp-dna benchmark (SunSpider / Computer Language Benchmarks
        // Game) on an input made by the "fasta" generator.
        r#"
var last = 42, A = 3877, C = 29573, M = 139968;
function rand(max) { last = (last * A + C) % M; return max * last / M; }
var ALU = "GGCCGGGCGCGGTGGCTCACGCCTGTAATCCCAGCACTTTGGGAGGCCGAGGCGGGCGGATCACCTGAGGTCAGGAGTTCGAGACCAGCCTGGCCAACATGGTGAAACCCCGTCTCTACTAAAAATACAAAAATTAGCCGGGCGTGGTGGCGCGCGCCTGTAATCCCAGCTACTCGGGAGGCTGAGGCAGGAGAATCGCTTGAACCCGGGAGGCGGAGGTTGCAGTGAGCCGAGATCGCGCCACTGCACTCCAGCCTGGGCGACAGAGCGAGACTCCGTCTCAAAAA";
var IUB = { a: 0.27, c: 0.12, g: 0.12, t: 0.27, B: 0.02, D: 0.02, H: 0.02, K: 0.02, M: 0.02, N: 0.02, R: 0.02, S: 0.02, V: 0.02, W: 0.02, Y: 0.02 };
var HomoSap = { a: 0.3029549426680, c: 0.1979883004921, g: 0.1975473066391, t: 0.3015094502008 };
function makeCumulative(table) { var last = null, out = []; for (var c in table) { if (last) table[c] += table[last]; last = c; } return table; }
function fastaRepeat(n, seq, out) { var seqi = 0, lenOut = 60; while (n > 0) { if (n < lenOut) lenOut = n; if (seqi + lenOut < seq.length) { out.push(seq.substring(seqi, seqi + lenOut)); seqi += lenOut; } else { var s = seq.substring(seqi); seqi = lenOut - s.length; out.push(s + seq.substring(0, seqi)); } n -= lenOut; } }
function fastaRandom(n, table, out) { var line = new Array(60); makeCumulative(table); while (n > 0) { if (n < line.length) line = new Array(n); for (var i = 0; i < line.length; i++) { var r = rand(1); for (var c in table) { if (r < table[c]) { line[i] = c; break; } } } out.push(line.join('')); n -= line.length; } }
var parts = [];
parts.push(">ONE Homo sapiens alu"); fastaRepeat(2 * 30000, ALU, parts);
parts.push(">TWO IUB ambiguity codes"); fastaRandom(3 * 30000, IUB, parts);
parts.push(">THREE Homo sapiens frequency"); fastaRandom(5 * 30000, HomoSap, parts);
var dnaInput = parts.join("\n") + "\n";
var seqs = [/agggtaaa|tttaccct/ig, /[cgt]gggtaaa|tttaccc[acg]/ig, /a[act]ggtaaa|tttacc[agt]t/ig, /ag[act]gtaaa|tttac[agt]ct/ig, /agg[act]taaa|ttta[agt]cct/ig, /aggg[acg]aaa|ttt[cgt]ccct/ig, /agggt[cgt]aa|tt[acg]accct/ig, /agggta[cgt]a|t[acg]taccct/ig, /agggtaa[cgt]|[acg]ttaccct/ig];
var subs = { B: '(c|g|t)', D: '(a|g|t)', H: '(a|c|t)', K: '(g|t)', M: '(a|c)', N: '(a|c|g|t)', R: '(a|g)', S: '(c|g)', V: '(a|c|g)', W: '(a|t)', Y: '(c|t)' };
var out = [];
for (var rep = 0; rep < 3; rep++) {
  var input = dnaInput.replace(/>.*\n|\n/g, "");
  var counts = [];
  for (var i = 0; i < seqs.length; i++) counts.push((input.match(seqs[i]) || []).length);
  for (var k in subs) input = input.replace(new RegExp(k, 'g'), subs[k]);
  out.push(dnaInput.length + ' ' + input.length + ' ' + counts.join(','));
}
out.join(';')"#,
    ),
    (
        "map_set",
        r#"
let s = 0;
const m = new Map();
for (let i = 0; i < 200000; i++) m.set(i, i * 2);
for (let k = 0; k < 5; k++) for (let i = 0; i < 200000; i++) s += m.get(i) & 3;
for (let i = 0; i < 200000; i += 2) m.delete(i);
for (const [k, v] of m) s += (v - k) & 3;
const sm = new Map(); const keys = []; for (let i = 0; i < 10000; i++) keys.push("key" + i);
for (let r = 0; r < 20; r++) for (const k of keys) sm.set(k, (sm.get(k) || 0) + 1);
const objs = []; for (let i = 0; i < 10000; i++) objs.push({ i });
const om = new Map(); for (const o of objs) om.set(o, o.i);
for (let r = 0; r < 20; r++) for (const o of objs) s += om.get(o) & 1;
const st = new Set(); for (let i = 0; i < 200000; i++) st.add(i % 50000);
for (let i = 0; i < 200000; i++) if (st.has(i)) s++;
st.forEach(v => { s += v & 1; });
for (let r = 0; r < 50; r++) { const t = new Set(); for (let i = 0; i < 1000; i++) t.add("s" + (i % 100)); s += t.size; }
const q = new Map(); for (let i = 0; i < 300000; i++) { q.set(i, i); if (i >= 100) q.delete(i - 100); }
s + q.size"#,
    ),
    (
        "calls",
        r#"
function add(a, b) { return a + b; }
function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); }
const o = { v: 1, get(x) { return this.v + x; } };
let s = 0;
for (let i = 0; i < 2000000; i++) s = add(s, i) % 1000003;
for (let i = 0; i < 1000000; i++) s += o.get(i) & 1;
s += fib(27);
function va() { return arguments.length; }
for (let i = 0; i < 300000; i++) s += va(i, i);
for (let i = 0; i < 300000; i++) s += add.call(null, i, 1) & 1;
s"#,
    ),
    (
        "exceptions",
        r#"
let s = 0;
for (let i = 0; i < 100000; i++) { try { throw new Error("e" + i); } catch (e) { s += e.message.length; } }
function thrower(d) { if (d === 0) throw new TypeError("deep"); return thrower(d - 1) + 1; }
for (let i = 0; i < 20000; i++) { try { thrower(10); } catch (e) { s++; } }
for (let i = 0; i < 300000; i++) { try { if (i & 1) throw i; s++; } catch (e) { s += 2; } finally { s++; } }
for (let i = 0; i < 100000; i++) { try { null.x; } catch (e) { s += e instanceof TypeError ? 1 : 0; } }
s"#,
    ),
];

#[cfg(target_os = "linux")]
fn cpu_ms() -> f64 {
    #[repr(C)]
    struct Timespec {
        sec: i64,
        nsec: i64,
    }
    extern "C" {
        fn clock_gettime(clk: i32, tp: *mut Timespec) -> i32;
    }
    let mut t = Timespec { sec: 0, nsec: 0 };
    // CLOCK_THREAD_CPUTIME_ID
    unsafe { clock_gettime(3, &mut t) };
    t.sec as f64 * 1000.0 + t.nsec as f64 / 1e6
}

#[cfg(not(target_os = "linux"))]
fn cpu_ms() -> f64 {
    thread_local!(static START: std::time::Instant = std::time::Instant::now());
    START.with(|s| s.elapsed().as_secs_f64() * 1000.0)
}

fn peak_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmHWM:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse().ok()))
        .unwrap_or(0)
}

fn print(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let parts: Vec<String> = args.iter().map(|a| realm.to_string(a.clone()).map(|s| s.to_rust_string()).unwrap_or_default()).collect();
    println!("{}", parts.join(" "));
    Ok(Value::Undefined)
}

/// Run one case in this process: "ms peak_kb result".
fn run_case(name: &str) {
    let src = CASES.iter().find(|(n, _)| *n == name).map(|(_, s)| *s).expect("unknown case");
    let mut realm = Realm::new();
    let g = realm.global();
    realm.define_method(g, "print", 1, print);
    let t = cpu_ms();
    let r = realm.eval(src, name);
    let dt = cpu_ms() - t;
    let out = match r {
        Ok(v) => realm.to_string(v).map(|s| s.to_rust_string()).unwrap_or_default(),
        Err(e) => format!("ERROR {}", realm.error_to_string(&e)),
    };
    println!("{dt:.2} {} {out}", peak_kb());
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--case") => return run_case(&args[2]),
        Some("--dump") => {
            let dir = std::path::Path::new(&args[2]);
            std::fs::create_dir_all(dir).expect("create dir");
            for (name, src) in CASES {
                // The completion value becomes an explicit print.
                let body = src.trim_end();
                let (head, last) = body.rsplit_once('\n').unwrap_or(("", body));
                std::fs::write(dir.join(format!("{name}.js")), format!("{head}\nprint({last});\n")).expect("write");
            }
            return;
        }
        _ => {}
    }
    let filter = args.get(1).cloned();
    let exe = std::env::current_exe().expect("current exe");
    println!("{:<14} {:>10} {:>10}  result", "case", "cpu ms", "peak MB");
    for (name, _) in CASES {
        if filter.as_deref().is_some_and(|f| !name.contains(f)) {
            continue;
        }
        let mut best = f64::MAX;
        let mut peak = 0u64;
        let mut result = String::new();
        for _ in 0..3 {
            let out = std::process::Command::new(&exe).args(["--case", name]).output().expect("run case");
            let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let mut it = line.splitn(3, ' ');
            let ms: f64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(f64::NAN);
            let kb: u64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            best = best.min(ms);
            peak = peak.max(kb);
            result = it.next().unwrap_or("").to_string();
        }
        println!("{:<14} {:>10.1} {:>10.1}  {}", name, best, peak as f64 / 1024.0, result);
    }
}
