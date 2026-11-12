//! Profile the interpreter on a real workload: rollup's SWC parser
//! (wasm-bindgen ABI, e.g. the svelte playground's bindings_wasm_bg.wasm)
//! parsing a JavaScript file, without a JS engine.
//!
//! `cargo run --release -p rabbithole-wasm [--features opcount] --example wasm_parse_prof -- rollup_bg.wasm input.js [repeat]`

use rabbithole_wasm::runtime::*;
use rabbithole_wasm::module::ImportDesc;
use rabbithole_wasm::{CompiledModule, Val};
use std::rc::Rc;

struct H;

fn cpu() -> std::time::Duration {
    #[repr(C)]
    struct Ts {
        s: i64,
        ns: i64,
    }
    extern "C" {
        fn clock_gettime(c: i32, t: *mut Ts) -> i32;
    }
    let mut t = Ts { s: 0, ns: 0 };
    // SAFETY: plain libc call (CLOCK_THREAD_CPUTIME_ID = 3).
    unsafe { clock_gettime(3, &mut t) };
    std::time::Duration::new(t.s as u64, t.ns as u32)
}

impl Host for H {
    fn call_host(&mut self, _store: &Store, _func: &Rc<FuncInst>, _args: &[Val]) -> Result<Vec<Val>, Trap> {
        // drop_ref / stack / error: nothing; new: an object id; length: 0.
        Ok(_func.ft.results.iter().map(|_| Val::I32(128)).collect())
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let wasm = std::fs::read(&args[1]).expect("wasm");
    let src = std::fs::read(&args[2]).expect("input");
    let repeat: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
    let t0 = std::time::Instant::now();
    let m = Rc::new(CompiledModule::new(&wasm).expect("compile"));
    println!("compile {:?}", t0.elapsed());
    let store = Store::new();
    let imports: Vec<Extern> = m.module.imports.iter().map(|i| {
        let ImportDesc::Func(t) = i.desc else { panic!("non-function import") };
        let ft = rabbithole_wasm::types::canonical_func_type(m.module.canon[t as usize]);
        Extern::Func(store.host_func(ft, Rc::new(())))
    }).collect();
    let inst = store.instantiate(&mut H, &m, &imports).expect("instantiate");
    let f = |n: &str| match inst.export(n) {
        Some(Extern::Func(f)) => f.clone(),
        _ => panic!("{n}"),
    };
    let (sp, malloc, free, parse) = (f("__wbindgen_add_to_stack_pointer"), f("__wbindgen_export2"), f("__wbindgen_export"), f("parse"));
    let Some(Extern::Memory(mem)) = inst.export("memory").cloned() else { panic!() };
    let t1 = std::time::Instant::now();
    let c1 = cpu();
    let mut ast = 0;
    for _ in 0..repeat {
        let ret = store.call(&mut H, &sp, &[Val::I32(-16)]).unwrap()[0];
        let Val::I32(ret) = ret else { panic!() };
        let Val::I32(p) = store.call(&mut H, &malloc, &[Val::I32(src.len() as i32), Val::I32(1)]).unwrap()[0] else { panic!() };
        {
            let (base, _) = mem.raw();
            unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), base.add(p as u32 as usize), src.len()) };
        }
        store.call(&mut H, &parse, &[Val::I32(ret), Val::I32(p), Val::I32(src.len() as i32), Val::I32(0), Val::I32(0)]).unwrap();
        let (base, _) = mem.raw();
        let rp = unsafe { std::ptr::read_unaligned(base.add(ret as u32 as usize) as *const i32) };
        let rn = unsafe { std::ptr::read_unaligned(base.add(ret as u32 as usize + 4) as *const i32) };
        ast = rn;
        store.call(&mut H, &free, &[Val::I32(rp), Val::I32(rn), Val::I32(1)]).unwrap();
        store.call(&mut H, &sp, &[Val::I32(16)]).unwrap();
    }
    let el = t1.elapsed();
    let cel = cpu() - c1;
    println!("cpu {cel:?}");
    println!("parse {} bytes x{repeat}: {:?} ({:.1} KB/s), ast {} bytes", src.len(), el, (src.len() * repeat) as f64 / 1024.0 / el.as_secs_f64(), ast);
    #[cfg(feature = "opcount")]
    {
        let counts = rabbithole_wasm::interp::op_counts();
        let total: u64 = counts.iter().map(|c| c.1).sum();
        println!("{total} instructions ({:.1} ns cpu each)", cel.as_nanos() as f64 / total as f64);
        for (name, n) in counts.iter().take(45) {
            println!("{name:<16} {n:>12} {:5.2}%", 100.0 * *n as f64 / total as f64);
        }
    }
}
