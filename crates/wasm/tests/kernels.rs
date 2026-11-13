//! End-to-end check of the benchmark kernels (tests/fixtures/kernels.wat)
//! against reference implementations in Rust.

use rabbithole_wasm::runtime::{Extern, Store};
use rabbithole_wasm::{CompiledModule, NoHost, Val};
use std::rc::Rc;

fn instance() -> (Store, Rc<rabbithole_wasm::Instance>) {
    let bin = rabbithole_wasm::text::wat_to_binary(include_str!("fixtures/kernels.wat")).unwrap();
    let m = Rc::new(CompiledModule::new(&bin).unwrap());
    let store = Store::new();
    let inst = store.instantiate(&mut NoHost, &m, &[]).unwrap();
    (store, inst)
}

fn call(store: &Store, inst: &rabbithole_wasm::Instance, name: &str, args: &[Val]) -> Val {
    let Some(Extern::Func(f)) = inst.export(name) else { panic!("{name}") };
    store.call(&mut NoHost, f, args).unwrap()[0]
}

fn fib(n: i32) -> i32 {
    if n < 2 {
        n
    } else {
        fib(n - 1) + fib(n - 2)
    }
}

fn sha_like(blocks: usize, k: &[u32]) -> i32 {
    let msg: Vec<u8> = (0..blocks * 64).map(|i| (i * 31) as u8).collect();
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    for blk in 0..blocks {
        let mut w = [0u32; 64];
        for t in 0..16 {
            let o = blk * 64 + t * 4;
            w[t] = u32::from_le_bytes([msg[o], msg[o + 1], msg[o + 2], msg[o + 3]]);
        }
        for t in 16..64 {
            let s0 = w[t - 15].rotate_right(7) ^ w[t - 15].rotate_right(18) ^ (w[t - 15] >> 3);
            let s1 = w[t - 2].rotate_right(17) ^ w[t - 2].rotate_right(19) ^ (w[t - 2] >> 10);
            w[t] = s0.wrapping_add(s1).wrapping_add(w[t - 16]).wrapping_add(w[t - 7]);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for t in 0..64 {
            let t1 = hh
                .wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
                .wrapping_add((e & f) ^ (!e & g))
                .wrapping_add(k[t])
                .wrapping_add(w[t]);
            let t2 = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22)).wrapping_add((a & b) ^ (a & c) ^ (b & c));
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    h[0] as i32
}

/// A `local.get` value still on the stack while a nested block (taken or
/// not) or a loop overwrites the local.
#[test]
fn lazy_local_reads_across_blocks() {
    let bin = rabbithole_wasm::text::wat_to_binary(
        r#"(module
          (func (export "f") (param i32) (result i32)
            (local.get 0)
            (if (local.get 0) (then (local.set 0 (i32.const 100))))
            (local.get 0)
            (i32.add))
          (func (export "loop") (result i32) (local i32)
            (local.get 0)
            (loop $l
              (local.set 0 (i32.add (local.get 0) (i32.const 1)))
              (br_if $l (i32.lt_s (local.get 0) (i32.const 10))))
            (local.get 0)
            (i32.add)))"#,
    )
    .unwrap();
    let m = Rc::new(CompiledModule::new(&bin).unwrap());
    let store = Store::new();
    let inst = store.instantiate(&mut NoHost, &m, &[]).unwrap();
    assert_eq!(call(&store, &inst, "f", &[Val::I32(0)]), Val::I32(0));
    assert_eq!(call(&store, &inst, "f", &[Val::I32(5)]), Val::I32(105));
    assert_eq!(call(&store, &inst, "loop", &[]), Val::I32(10));
}

/// Unbounded recursion (direct and through call_indirect) traps instead of
/// overflowing the native stack.
#[test]
fn runaway_recursion_traps() {
    let bin = rabbithole_wasm::text::wat_to_binary(
        r#"(module
          (type $proc (func))
          (table funcref (elem $runaway))
          (func $runaway (export "runaway") (call_indirect (type $proc) (i32.const 0)))
          (func $direct (export "direct") (call $direct)))"#,
    )
    .unwrap();
    let m = Rc::new(CompiledModule::new(&bin).unwrap());
    let store = Store::new();
    let inst = store.instantiate(&mut NoHost, &m, &[]).unwrap();
    for name in ["direct", "runaway"] {
        let Some(Extern::Func(f)) = inst.export(name) else { panic!() };
        let r = store.call(&mut NoHost, f, &[]);
        assert!(matches!(r, Err(rabbithole_wasm::Trap::StackExhausted)), "{name}: {r:?}");
    }
}

#[test]
fn kernels_match_reference() {
    let (store, inst) = instance();
    assert_eq!(call(&store, &inst, "fib", &[Val::I32(20)]), Val::I32(fib(20)));
    assert_eq!(call(&store, &inst, "matmul", &[Val::I32(16)]), Val::F64(0f64.to_bits()));
    let Some(Extern::Memory(mem)) = inst.export("memory") else { panic!() };
    let k: Vec<u32> = {
        let (p, _) = mem.raw();
        (0..64).map(|i| unsafe { std::ptr::read_unaligned((p as *const u32).add(i)) }).collect()
    };
    for blocks in [1, 2, 7, 100, 1100, 4096] {
        assert_eq!(call(&store, &inst, "sha256", &[Val::I32(blocks)]), Val::I32(sha_like(blocks as usize, &k)), "{blocks} blocks");
    }
}

/// Requesting native code on another architecture must preserve execution.
#[cfg(not(target_arch = "x86_64"))]
#[test]
fn unsupported_architecture_uses_interpreter() {
    use rabbithole_wasm::jit;
    use rabbithole_wasm::runtime::FuncKind;
    let (store, inst) = instance();
    assert!(!jit::enabled());
    let Some(Extern::Func(f)) = inst.export("fib") else { panic!("fib export") };
    let FuncKind::Wasm { code, .. } = &f.kind else { panic!("wasm function") };
    assert!(jit::compiled(code).is_none());
    assert!(jit::compile(code).is_none());
    assert!(jit::entry_cells(2).iter().all(|e| e.get().is_null()));
    assert_eq!(store.call(&mut NoHost, f, &[Val::I32(10)]).unwrap(), vec![Val::I32(55)]);
}
