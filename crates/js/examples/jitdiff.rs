//! Differential testing of the baseline compiler against the interpreter.
//!
//! `cargo run --release -p rabbithole-js --features jit --example jitdiff -- [options] [files.js]`
//!
//! * files: each script runs in fresh realms with the interpreter only,
//!   with every function compiled at its first run (threshold 0), and with
//!   threshold 2 (hot loops enter native code mid-run); completion value,
//!   exception and console output must match.
//! * `--fuzz N [--seed S]`: N random programs (mixed types, loops, calls,
//!   try/catch, getters and `valueOf` with side effects, string
//!   concatenation, bit operations, NaN / -0 / huge values) checked the
//!   same way; the seed of each failure is printed so it can be replayed
//!   with `--fuzz 1 --seed S --show`.
//! * `--perf`: the interpreter micro-benchmarks as a corpus.
#[global_allocator]
static ALLOC: rabbithole_js::alloc::RhAlloc = rabbithole_js::alloc::RhAlloc;

use rabbithole_js::Realm;
use std::cell::RefCell;
use std::rc::Rc;

fn run_mode(src: &str, jit: Option<u32>) -> String {
    let mut realm = Realm::new();
    match jit {
        Some(t) => realm.set_jit(true, t),
        None => realm.set_jit(false, 0),
    }
    let out = Rc::new(RefCell::new(String::new()));
    let o = out.clone();
    realm.set_console_sink(Box::new(move |_, msg| {
        let mut o = o.borrow_mut();
        o.push_str(&msg);
        o.push('\n');
    }));
    let r = realm.eval(src, "jitdiff.js");
    realm.run_jobs();
    let head = match r {
        Ok(v) => match realm.to_string(v) {
            Ok(s) => format!("ok {}", s.to_rust_string()),
            Err(e) => format!("tostring-error {}", realm.error_to_string(&e)),
        },
        Err(e) => format!("error {}", realm.error_to_string(&e)),
    };
    let console = out.borrow().clone();
    format!("{head}\n{console}")
}

/// Err(report) when a mode differs; Ok(true) when the program threw.
fn diff(src: &str) -> Result<bool, String> {
    let vm = run_mode(src, None);
    for t in [0, 2] {
        let j = run_mode(src, Some(t));
        if j != vm {
            return Err(format!("threshold {t}:\n--- interpreter\n{vm}\n--- native\n{j}"));
        }
    }
    Ok(vm.starts_with("error"))
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len() as u64) as usize]
    }
}

const ATOMS: &[&str] = &[
    "0", "1", "-1", "2", "7", "0.5", "-0", "NaN", "Infinity", "-Infinity", "2147483647", "4294967296", "1e21", "3.75", "'a'", "'12'", "''", "true", "false", "null",
    "undefined", "o", "arr", "big", "obj2",
];
const BINOPS: &[&str] = &["+", "-", "*", "/", "%", "&", "|", "^", "<<", ">>", ">>>", "<", "<=", ">", ">=", "==", "!=", "===", "!==", "**", "&&", "||", "??"];

fn expr(r: &mut Rng, vars: &[&str], depth: u32) -> String {
    if depth == 0 || r.below(3) == 0 {
        return if r.below(2) == 0 { r.pick(vars).to_string() } else { r.pick(ATOMS).to_string() };
    }
    match r.below(9) {
        0..=4 => {
            let (a, b) = (expr(r, vars, depth - 1), expr(r, vars, depth - 1));
            let op = r.pick(BINOPS);
            if op == "**" {
                format!("(({a}) % 5) ** (({b}) % 3)")
            } else {
                format!("({a} {op} {b})")
            }
        }
        5 => format!("(- {})", expr(r, vars, depth - 1)),
        6 => format!("(!{})", expr(r, vars, depth - 1)),
        7 => format!("(~{})", expr(r, vars, depth - 1)),
        _ => format!("({} ? {} : {})", expr(r, vars, depth - 1), expr(r, vars, depth - 1), expr(r, vars, depth - 1)),
    }
}

fn stmt(r: &mut Rng, vars: &[&str], depth: u32, out: &mut String, ind: usize) {
    let pad = " ".repeat(ind);
    let v = r.pick(vars);
    match r.below(if depth == 0 { 4 } else { 9 }) {
        // `clip` keeps strings short (doubling a string in a loop would
        // otherwise exhaust memory before the length limit).
        0 | 1 => out.push_str(&format!("{pad}{v} = clip({});\n", expr(r, vars, 3))),
        2 => out.push_str(&format!("{pad}{v} = clip({v} {} {});\n", r.pick(&["+", "-", "*", "|", "&", "^"]), expr(r, vars, 2))),
        3 => out.push_str(&format!("{pad}{v}{};\n", r.pick(&["++", "--"]))),
        4 => {
            out.push_str(&format!("{pad}if ({}) {{\n", expr(r, vars, 2)));
            stmt(r, vars, depth - 1, out, ind + 2);
            out.push_str(&format!("{pad}}} else {{\n"));
            stmt(r, vars, depth - 1, out, ind + 2);
            out.push_str(&format!("{pad}}}\n"));
        }
        5 => {
            let n = r.below(40) + 1;
            out.push_str(&format!("{pad}for (let k{depth} = 0; k{depth} < {n}; k{depth}++) {{\n"));
            stmt(r, vars, depth - 1, out, ind + 2);
            stmt(r, vars, depth - 1, out, ind + 2);
            if r.below(4) == 0 {
                out.push_str(&format!("{pad}  if ({}) break;\n", expr(r, vars, 1)));
            }
            out.push_str(&format!("{pad}}}\n"));
        }
        6 => {
            out.push_str(&format!("{pad}try {{\n"));
            stmt(r, vars, depth - 1, out, ind + 2);
            out.push_str(&format!("{pad}  if ({}) throw {};\n", expr(r, vars, 1), expr(r, vars, 1)));
            out.push_str(&format!("{pad}}} catch (e) {{ {v} = typeof e; }}\n"));
        }
        7 => out.push_str(&format!("{pad}{v} = h({}, {});\n", expr(r, vars, 2), expr(r, vars, 2))),
        _ => out.push_str(&format!("{pad}log({v});\n")),
    }
}

fn program(seed: u64) -> String {
    let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let vars = ["a", "b", "c", "d", "s"];
    let mut src = String::from(
        "var seen = [];\nfunction log(x) { seen.push(typeof x === 'number' && x !== x ? 'NaN' : (Object.is(x, -0) ? '-0' : String(x))); }\n\
         var calls = 0;\nvar o = { valueOf() { calls++; return 3; }, toString() { return 'O'; } };\n\
         var obj2 = { get g() { calls += 2; return 5; } };\nvar arr = [1, 'x', 3];\nvar big = 9007199254740993;\n\
         function h(x, y) { calls++; if (typeof x === 'string') return clip(x + y); return (x * 2) - y; }\n\
         function clip(x) { return typeof x === 'string' && x.length > 40 ? x.slice(0, 40) : x; }\n",
    );
    for f in 0..3 {
        src.push_str(&format!("function f{f}(a, b) {{\n  var c = {}, d = {}, s = {};\n", expr(&mut r, &["a", "b"], 2), r.pick(ATOMS), r.pick(ATOMS)));
        for _ in 0..(r.below(5) + 2) {
            stmt(&mut r, &vars, 3, &mut src, 2);
        }
        src.push_str(&format!("  return {};\n}}\n", expr(&mut r, &vars, 2)));
    }
    src.push_str("var acc = [];\nfor (var i = 0; i < 25; i++) {\n  try { acc.push(String(f0(i, i * 1.5)), String(f1('x' + i, i)), String(f2(i & 3 ? i : null, o))); } catch (e) { acc.push('E:' + (e && e.name)); }\n}\n");
    src.push_str("log(calls); seen.join(',') + '|' + acc.join(',')\n");
    src
}

const PERF: &[&str] = &[
    "(function(){ for (var i=0;i<100000;i++){} return i; })()",
    "var s=0; for (var i=0;i<100000;i++){ s+=i%7; } s",
    "function f(x) { return x + 1; } let s = 0; for (let i = 0; i < 30000; i++) s = f(s); s",
    "function mk(){ var n=0; return function(){ return ++n; }; } var c=mk(); var t=0; for (var i=0;i<30000;i++) t+=c(); t",
    "function P(x){ this.x=x; } P.prototype.get=function(){ return this.x; }; var p=new P(3), s=0; for (var i=0;i<30000;i++) s+=p.get(); s",
    "const o = {a: 1, b: 2, c: 3}; let s = 0; for (let i = 0; i < 50000; i++) s += o.a + o.b + o.c; s",
    "var a=[]; for (var i=0;i<10000;i++) a[i]=i; var s=0; for (var k=0;k<5;k++) for (var i=0;i<10000;i++) s+=a[i]; s",
    "let s = ''; for (let i = 0; i < 10000; i++) s += 'ab'; s.length",
    "var t=0; for (var i=0;i<10000;i++){ var s='abc'+i; t+=s.length+s.charCodeAt(1)+s.indexOf('c'); } t",
    "let a = Array.from({length: 20000}, (_, i) => i); a.map(x => x * 2).filter(x => x % 3 == 0).reduce((p, c) => p + c, 0)",
    "function fib(n){ return n<2?n:fib(n-1)+fib(n-2); } fib(18)",
    "var o={a:[1,2,3],b:'x',c:{d:true}}; var n=0; for (var i=0;i<2000;i++){ n+=JSON.parse(JSON.stringify(o)).a.length; } n",
    "var o={a:1,b:2,c:3,d:4}; var n=0; for (var i=0;i<5000;i++){ for (var k in o) n+=o[k]; } n",
    "var n=0; for (var i=0;i<10000;i++){ try { if (i%10==0) throw i; n++; } catch(e){ n+=2; } } n",
    "function f(){ return arguments.length+arguments[0]; } var s=0; for (var i=0;i<20000;i++) s+=f(i,2); s",
    "var s=0; for (var i=0;i<50000;i++){ switch(i&3){ case 0: s+=1; break; case 1: s+=2; break; default: s+=3; } } s",
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut fuzz = 0u64;
    let mut seed = 1u64;
    let mut show = false;
    let mut perf = false;
    let mut files = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--fuzz" => {
                i += 1;
                fuzz = args[i].parse().expect("count");
            }
            "--seed" => {
                i += 1;
                seed = args[i].parse().expect("seed");
            }
            "--show" => show = true,
            "--perf" => perf = true,
            f => files.push(f.to_string()),
        }
        i += 1;
    }
    let failures = std::cell::Cell::new(0u64);
    let total = std::cell::Cell::new(0u64);
    let threw = std::cell::Cell::new(0u64);
    let check = |name: &str, src: &str| {
        total.set(total.get() + 1);
        match diff(src) {
            Err(d) => {
                failures.set(failures.get() + 1);
                println!("DIFF {name}\n{d}");
                if show {
                    println!("--- source\n{src}");
                }
            }
            Ok(true) => threw.set(threw.get() + 1),
            Ok(false) => {}
        }
    };
    for f in &files {
        let src = std::fs::read_to_string(f).expect("read");
        check(f, &src);
    }
    if perf {
        for (k, src) in PERF.iter().enumerate() {
            check(&format!("perf#{k}"), src);
        }
    }
    for n in 0..fuzz {
        let s = seed + n;
        let src = program(s);
        if show && fuzz == 1 {
            println!("{src}");
        }
        check(&format!("seed {s}"), &src);
        if (n + 1) % 1000 == 0 {
            eprintln!("{} programs, {} differences", n + 1, failures.get());
        }
    }
    println!("{} programs ({} ended in an uncaught exception), {} differences", total.get(), threw.get(), failures.get());
    std::process::exit(if failures.get() == 0 { 0 } else { 1 });
}
