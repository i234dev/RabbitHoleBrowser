//! Print the register IR of every function of a module (.wat or .wasm).
//!
//! `cargo run --release -p rabbithole-wasm --example wasm_dump -- file.wat`

fn main() {
    let path = std::env::args().nth(1).expect("usage: wasm_dump <file.wat|file.wasm>");
    let data = std::fs::read(&path).expect("read");
    let bin = if data.starts_with(b"\0asm") { data } else { rabbithole_wasm::text::wat_to_binary(&String::from_utf8_lossy(&data)).expect("wat") };
    let m = rabbithole_wasm::CompiledModule::new(&bin).expect("compile");
    for code in &m.codes {
        println!(
            "func {} (params {} locals {} consts {:?} frame {} results {})",
            code.func_index,
            code.param_slots,
            code.local_slots,
            code.consts,
            code.frame_slots,
            code.result_slots
        );
        for (pc, i) in code.instrs.iter().enumerate() {
            println!("  {pc:4}: {:?} x={} a={} b={} c={}", i.op, i.x, i.a, i.b, i.c);
        }
        if !code.br_tables.is_empty() {
            println!("  br_tables {:?}", code.br_tables);
        }
        for h in code.handlers.iter() {
            println!("  handler {h:?}");
        }
    }
}
