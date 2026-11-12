//! Summarize a module: imports, exports, and the post-MVP features its code
//! uses. `cargo run --release -p rabbithole-wasm --example wasm_info -- file.wasm`

use rabbithole_wasm::ir::Op;
use rabbithole_wasm::module::ImportDesc;
use std::collections::BTreeMap;

fn main() {
    let path = std::env::args().nth(1).expect("usage: wasm_info <file.wasm>");
    let bytes = std::fs::read(&path).expect("read");
    let m = rabbithole_wasm::CompiledModule::new(&bytes).expect("compile");
    let md = &m.module;
    println!("{} bytes, {} types, {} functions ({} imported), {} tables, {} memories, {} globals, {} tags", bytes.len(), md.types.len(), md.funcs.len(), md.num_imported_funcs, md.tables.len(), md.memories.len(), md.globals.len(), md.tags.len());
    for i in &md.imports {
        let d = match &i.desc {
            ImportDesc::Func(t) => format!("func {:?}", md.type_func(*t)),
            other => format!("{other:?}"),
        };
        println!("import {}.{}: {d}", i.module, i.name);
    }
    let names: Vec<String> = md.exports.iter().map(|e| format!("{}:{}", e.name, e.kind.name())).collect();
    println!("exports: {}", names.join(" "));
    for (n, _) in &md.customs {
        println!("custom section: {n}");
    }
    let mut feats: BTreeMap<&str, usize> = BTreeMap::new();
    for code in &m.codes {
        for i in code.instrs.iter() {
            let f = match i.op {
                Op::Simd | Op::Simd3 | Op::SimdLoad | Op::SimdStore | Op::Shuffle => "simd",
                Op::MemoryCopy | Op::MemoryFill | Op::MemoryInit | Op::DataDrop | Op::TableCopy | Op::TableInit | Op::ElemDrop => "bulk-memory",
                Op::I32Extend8S | Op::I32Extend16S | Op::I64Extend8S | Op::I64Extend16S | Op::I64Extend32S => "sign-extension",
                Op::I32TruncSatF32S | Op::I32TruncSatF32U | Op::I32TruncSatF64S | Op::I32TruncSatF64U | Op::I64TruncSatF32S | Op::I64TruncSatF32U | Op::I64TruncSatF64S | Op::I64TruncSatF64U => "nontrapping-float-to-int",
                Op::TableGet | Op::TableSet | Op::TableGrow | Op::TableSize | Op::TableFill | Op::RefFunc => "reference-types",
                Op::ReturnCall | Op::ReturnCallIndirect | Op::ReturnCallRef => "tail-call",
                Op::Throw | Op::ThrowRef | Op::Rethrow => "exceptions",
                Op::Gc | Op::BrOnCast | Op::BrOnCastFail => "gc",
                Op::Atomic => "threads",
                Op::CallRef => "function-references",
                Op::MemLoad | Op::MemStore => "multi-memory/memory64/large offsets",
                _ => continue,
            };
            *feats.entry(f).or_default() += 1;
        }
        if !code.handlers.is_empty() {
            *feats.entry("exceptions").or_default() += 1;
        }
    }
    let multi = md.types.iter().filter_map(|t| t.as_func()).any(|f| f.results.len() > 1);
    if multi {
        feats.insert("multi-value", 1);
    }
    if md.globals.iter().take(md.num_imported_globals as usize).any(|g| g.mutable) {
        feats.insert("mutable-globals", 1);
    }
    println!("features (instruction counts): {feats:?}");
}
