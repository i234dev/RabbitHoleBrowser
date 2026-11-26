//! Interpreter micro-benchmarks: `cargo run --release -p rabbithole-js --example perf [filter]`
//!
//! Every case runs in a fresh realm; the best of three runs is reported.
//! On Linux the thread CPU time is measured (robust on a loaded machine);
//! elsewhere wall-clock time.
#[global_allocator]
static ALLOC: rabbithole_js::alloc::RhAlloc = rabbithole_js::alloc::RhAlloc;

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
    // Hot functions: the loop lives in a function called many times (what
    // a baseline compiler speeds up without loop OSR).
    ("hot_numeric", "function f(n){ var s=0; for (var i=0;i<n;i++){ s=(s+i*3)%1000003; } return s; } var t=0; for (var k=0;k<200;k++) t+=f(10000); t"),
    ("hot_7ops", "function f(n){ var a=1,b=2,c=0; for (var i=0;i<n;i++){ a=a+i; b=b*1.000001; c=c+a-b; a=a/2; b=b-1; c=c*0.5; } return Math.round(c); } var t=0; for (var k=0;k<100;k++) t+=f(20000); t"),
    ("hot_bits", "function f(n){ var h=0; for (var i=0;i<n;i++){ h=((h<<5)-h+i)|0; h^=h>>>13; } return h; } var t=0; for (var k=0;k<100;k++) t^=f(20000); t"),
    ("hot_array", "function f(a){ var s=0; for (var i=0;i<a.length;i++) s+=a[i]; return s; } var a=[]; for (var i=0;i<10000;i++) a.push(i); var t=0; for (var k=0;k<100;k++) t+=f(a); t"),
    ("hot_props", "function g(o){ return o.x+o.y; } function f(o,n){ var s=0; for (var i=0;i<n;i++) s+=g(o); return s; } var o={x:1,y:2}, t=0; for (var k=0;k<100;k++) t+=f(o,5000); t"),
    ("hot_branchy", "function f(n){ var c=0; for (var i=0;i<n;i++){ if (i%3===0) c+=2; else if (i%5===1) c-=1; else c^=i; } return c; } var t=0; for (var k=0;k<100;k++) t+=f(10000); t"),
    // Shapes of framework code: calls of small functions and methods,
    // polymorphic and inherited reads, absent properties, closures.
    ("call_args", "function add(a,b){ return a+b; } function f(n){ var s=0; for (var i=0;i<n;i++) s=add(s,i)|0; return s; } var t=0; for (var k=0;k<20;k++) t+=f(20000); t"),
    ("call_arrow", "const add=(a,b)=>a+b; function f(n){ let s=0; for (let i=0;i<n;i++) s=add(s,i)|0; return s; } let t=0; for (let k=0;k<20;k++) t+=f(20000); t"),
    ("call_scope", "function mk(k){ function g(x){ var y=x+k; return function(){ return y; }; } return g; } function f(n){ var g=mk(1), s=0; for (var i=0;i<n;i++) s+=g(i)(); return s; } var t=0; for (var k=0;k<10;k++) t+=f(20000); t"),
    ("call_deep", "function a(x){ return b(x)+1; } function b(x){ return c(x)+1; } function c(x){ return x&7; } function f(n){ var s=0; for (var i=0;i<n;i++) s+=a(i); return s; } var t=0; for (var k=0;k<10;k++) t+=f(20000); t"),
    ("class_methods", "class A { constructor(x){ this.x=x; } get2(){ return this.x*2; } } class B extends A { constructor(x){ super(x); this.y=1; } sum(){ return this.get2()+this.y; } } function f(n){ var s=0, o=new B(3); for (var i=0;i<n;i++) s+=o.sum(); return s; } var t=0; for (var k=0;k<10;k++) t+=f(20000); t"),
    ("class_new", "class P { constructor(x,y){ this.x=x; this.y=y; } } function f(n){ var s=0; for (var i=0;i<n;i++){ var p=new P(i,1); s+=p.x+p.y; } return s; } var t=0; for (var k=0;k<10;k++) t+=f(20000); t"),
    ("poly_props", "function g(o){ return o.a+o.b; } function f(os,n){ var s=0; for (var i=0;i<n;i++) s+=g(os[i&3]); return s; } var os=[{a:1,b:2},{b:2,a:1},{a:1,c:3,b:2},{z:0,a:1,b:2}], t=0; for (var k=0;k<10;k++) t+=f(os,20000); t"),
    ("proto_chain", "function A(){} A.prototype.m=function(){ return 1; }; function B(){} B.prototype=Object.create(A.prototype); function C(){} C.prototype=Object.create(B.prototype); function f(o,n){ var s=0; for (var i=0;i<n;i++) s+=o.m(); return s; } var o=new C(), t=0; for (var k=0;k<10;k++) t+=f(o,20000); t"),
    ("absent_props", "function A(){} function B(){} B.prototype=Object.create(A.prototype); function C(){ this.q=1; } C.prototype=Object.create(B.prototype); var key='x'+Math.floor(Math.random()*1e9); function f(os,n){ var s=0; for (var i=0;i<n;i++){ var o=os[i&7]; if (o[key]===undefined) s++; if (o.missing===undefined) s++; } return s; } var os=[]; for (var i=0;i<8;i++) os.push(new C()); var t=0; for (var k=0;k<10;k++) t+=f(os,20000); t"),
    ("closure_vars", "function mk(){ var n=0, m=1; return { inc: function(){ n+=m; return n; } }; } function f(c,n){ var s=0; for (var i=0;i<n;i++) s+=c.inc()&1; return s; } var c=mk(), t=0; for (var k=0;k<10;k++) t+=f(c,20000); t"),
    ("obj_literal", "function mk(i){ return {id:i, name:'n', done:false, next:null}; } function f(n){ var s=0, last=null; for (var i=0;i<n;i++){ var o=mk(i); o.next=last; last=o; s+=o.id; } return s; } var t=0; for (var k=0;k<10;k++) t+=f(20000); t"),
    ("array_cb", "function f(a){ return a.map(function(x){ return x+1; }).filter(function(x){ return x&1; }).length; } var a=[]; for (var i=0;i<1000;i++) a.push(i); var t=0; for (var k=0;k<200;k++) t+=f(a); t"),
    ("absent_poly", "function mk(p){ return Object.create(p); } var ps=[{a:1},{b:1},{c:1},{d:1}].map(function(x){ return Object.create(x); }); var os=[]; for (var i=0;i<16;i++) os.push(mk(ps[i&3])); var key='k'+Math.floor(Math.random()*1e9); function f(n){ var s=0; for (var i=0;i<n;i++){ var o=os[i&15]; if (o[key]===undefined) s++; if (o.nodeType===undefined) s++; } return s; } var t=0; for (var k=0;k<10;k++) t+=f(20000); t"),
    ("forin_copy", "var hop=Object.prototype.hasOwnProperty, RES={key:true,ref:true}; function ce(type,config){ var props={}; for (var k in config){ if (hop.call(config,k) && !RES.hasOwnProperty(k)) props[k]=config[k]; } return {type:type,props:props}; } function f(n){ var s=0; for (var i=0;i<n;i++){ var e=ce('div',{className:'x',id:i,key:'k',onClick:null}); s+=e.props.id; } return s; } var t=0; for (var k=0;k<5;k++) t+=f(20000); t"),
    ("str_build", "function f(n){ var s=''; for (var i=0;i<n;i++) s+='<li class=\"item\">'+i+'</li>'; return s.length; } var t=0; for (var k=0;k<20;k++) t+=f(2000); t"),
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
