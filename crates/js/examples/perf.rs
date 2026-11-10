//! Interpreter micro-benchmarks: `cargo run --release -p rabbithole-js --example perf [filter]`
//!
//! Every case runs in a fresh realm; the best of three runs is reported.
//! On Linux the thread CPU time is measured (robust on a loaded machine);
//! elsewhere wall-clock time.
use rabbithole_js::Realm;

#[cfg(target_os = "linux")]
fn now_ms() -> f64 {
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
fn now_ms() -> f64 {
    thread_local!(static START: std::time::Instant = std::time::Instant::now());
    START.with(|s| s.elapsed().as_secs_f64() * 1000.0)
}

const CASES: &[(&str, &str)] = &[
    ("empty_loop", "(function(){ for (var i=0;i<10000000;i++){} return i; })()"),
    ("global_loop", "var s=0; for (var i=0;i<1000000;i++){ s+=i%7; } s"),
    ("local_loop", "(function(){ var s=0; for (var i=0;i<1000000;i++){ s+=i%7; } return s; })()"),
    ("let_loop", "let s = 0; for (let i = 0; i < 1e6; i++) s += i; s"),
    ("push_objects", "var o=[]; for (var j=0;j<100000;j++){ o.push({a:j,b:String(j)}); } o.length"),
    ("calls", "function f(x) { return x + 1; } let s = 0; for (let i = 0; i < 300000; i++) s = f(s); s"),
    ("closure_calls", "function mk(){ var n=0; return function(){ return ++n; }; } var c=mk(); var t=0; for (var i=0;i<300000;i++) t+=c(); t"),
    ("method_calls", "function P(x){ this.x=x; } P.prototype.get=function(){ return this.x; }; var p=new P(3), s=0; for (var i=0;i<300000;i++) s+=p.get(); s"),
    ("new_objects", "function P(x,y){ this.x=x; this.y=y; } var s=0; for (var i=0;i<200000;i++){ var p=new P(i,1); s+=p.x+p.y; } s"),
    ("props", "const o = {a: 1, b: 2, c: 3}; let s = 0; for (let i = 0; i < 500000; i++) s += o.a + o.b + o.c; s"),
    ("prop_set", "var o={x:0,y:0}; for (var i=0;i<500000;i++){ o.x=i; o.y=o.x+1; } o.y"),
    ("array_rw", "var a=[]; for (var i=0;i<100000;i++) a[i]=i; var s=0; for (var k=0;k<5;k++) for (var i=0;i<100000;i++) s+=a[i]; s"),
    ("strings", "let s = ''; for (let i = 0; i < 100000; i++) s += 'ab'; s.length"),
    ("string_ops", "var t=0; for (var i=0;i<100000;i++){ var s='abc'+i; t+=s.length+s.charCodeAt(1)+s.indexOf('c'); } t"),
    ("array_map", "let a = Array.from({length: 200000}, (_, i) => i); a.map(x => x * 2).filter(x => x % 3 == 0).reduce((p, c) => p + c, 0)"),
    ("fib", "function fib(n){ return n<2?n:fib(n-1)+fib(n-2); } fib(25)"),
    ("json", "var o={a:[1,2,3],b:'x',c:{d:true}}; var n=0; for (var i=0;i<20000;i++){ n+=JSON.parse(JSON.stringify(o)).a.length; } n"),
    ("for_in", "var o={a:1,b:2,c:3,d:4}; var n=0; for (var i=0;i<50000;i++){ for (var k in o) n+=o[k]; } n"),
    ("try_catch", "var n=0; for (var i=0;i<100000;i++){ try { if (i%10==0) throw i; n++; } catch(e){ n+=2; } } n"),
    ("arguments", "function f(){ return arguments.length+arguments[0]; } var s=0; for (var i=0;i<200000;i++) s+=f(i,2); s"),
    ("switch", "var s=0; for (var i=0;i<500000;i++){ switch(i&3){ case 0: s+=1; break; case 1: s+=2; break; default: s+=3; } } s"),
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("-e") {
        let mut realm = Realm::new();
        let t = now_ms();
        let r = realm.eval(&args[2], "inline");
        let dt = now_ms() - t;
        let out = match r {
            Ok(v) => realm.to_string(v).map(|s| s.to_rust_string()).unwrap_or_default(),
            Err(e) => format!("ERROR {}", realm.error_to_string(&e)),
        };
        println!("{:.2} ms -> {}", dt, out);
        return;
    }
    let filter = args.get(1).cloned();
    let mut total = 0.0;
    for (name, src) in CASES {
        if let Some(f) = &filter {
            if !name.contains(f.as_str()) {
                continue;
            }
        }
        let mut best = f64::MAX;
        let mut result = String::new();
        for _ in 0..3 {
            let mut realm = Realm::new();
            let t = now_ms();
            let r = realm.eval(src, name);
            let dt = now_ms() - t;
            best = best.min(dt);
            result = match r {
                Ok(v) => realm.to_string(v).map(|s| s.to_rust_string()).unwrap_or_default(),
                Err(e) => format!("ERROR {}", realm.error_to_string(&e)),
            };
        }
        total += best;
        println!("{:<14} {:>9.2} ms  -> {}", name, best, result);
    }
    println!("{:<14} {:>9.2} ms", "TOTAL", total);
}
