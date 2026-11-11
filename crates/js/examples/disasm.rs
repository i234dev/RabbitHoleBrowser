//! Print the bytecode of a script: `disasm <file.js | -e source>`.
use rabbithole_js::bytecode::FunctionCode;

fn dump(code: &FunctionCode, depth: usize) {
    let pad = "  ".repeat(depth);
    println!("{}function '{}' regs={} slots={} consts={}", pad, code.name, code.nregs, code.scope_slots, code.constants.len());
    for (i, op) in code.ops.iter().enumerate() {
        println!("{}  {:4} [{}] {:?}", pad, i, code.depths.get(i).copied().unwrap_or(0), op);
    }
    for f in &code.functions {
        dump(f, depth + 1);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = if args.get(1).map(|s| s.as_str()) == Some("-e") { args[2].clone() } else { std::fs::read_to_string(&args[1]).unwrap() };
    let mut realm = rabbithole_js::Realm::new();
    match realm.compile(&src, "disasm", false) {
        Ok(code) => dump(&code, 0),
        Err(e) => println!("error: {}", realm.error_to_string(&e)),
    }
}
