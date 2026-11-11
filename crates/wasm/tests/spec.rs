//! WebAssembly specification test suite (https://github.com/WebAssembly/testsuite).
//!
//! The suite is not vendored: set `RH_WASM_TESTSUITE` or clone it to
//! `/mnt/hdd5tb/rabbithole/suites/wasm-testsuite`. `spec_quick` runs a fast
//! subset by default; `cargo test -p rabbithole-wasm --release -- --ignored
//! spec_full` runs everything (`RH_WAST_FILTER=substr` to select files,
//! `RH_WAST_OUT=path` to write the results table, `RH_WAST_VERBOSE=1` for
//! failure details).

use rabbithole_wasm::spec::{run_script, Stats};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn suite_dir() -> Option<PathBuf> {
    let p = std::env::var("RH_WASM_TESTSUITE").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/mnt/hdd5tb/rabbithole/suites/wasm-testsuite"));
    if p.join("i32.wast").exists() {
        Some(p)
    } else {
        None
    }
}

/// Proposal a test file belongs to (files at the top level are the wasm
/// 3.0 core suite, which merged several proposals).
pub fn category(rel: &str) -> &'static str {
    let name = rel.rsplit('/').next().unwrap_or(rel).trim_end_matches(".wast");
    if rel.starts_with("legacy/") {
        return "exceptions-legacy";
    }
    if rel.starts_with("proposals/") {
        let p = rel.split('/').nth(1).unwrap_or("");
        return match p {
            "threads" => "threads",
            "custom-page-sizes" => "custom-page-sizes",
            "wide-arithmetic" => "wide-arithmetic",
            "custom-descriptors" => "custom-descriptors",
            "compact-import-section" => "compact-import-section",
            "extended-name-section" => "extended-name-section",
            _ => "other-proposal",
        };
    }
    if rel.starts_with("custom/") {
        return "annotations";
    }
    if name.starts_with("relaxed_") || name.contains("_relaxed_") {
        return "relaxed-simd";
    }
    if name.starts_with("simd_") {
        if name == "simd_memory-multi" {
            return "multi-memory";
        }
        return "simd";
    }
    if (name.ends_with("64") && name != "f64" && name != "i64") || name.contains("64-") {
        return "memory64";
    }
    let multi = [
        "address0", "address1", "align0", "binary0", "data0", "data1", "data_drop0", "exports0", "float_exprs0", "float_exprs1", "float_memory0", "imports0", "imports1", "imports2", "imports3",
        "imports4", "linking0", "linking1", "linking2", "linking3", "load0", "load1", "load2", "memory_copy0", "memory_copy1", "memory_fill0", "memory_init0", "memory_size0", "memory_size1",
        "memory_size2", "memory_size3", "memory_trap0", "memory_trap1", "start0", "store0", "store1", "store2", "traps0", "memory-multi", "memory_size_import",
    ];
    if multi.contains(&name) {
        return "multi-memory";
    }
    let gc = [
        "array", "array_copy", "array_fill", "array_init_data", "array_init_elem", "array_new_data", "array_new_elem", "binary-gc", "br_on_cast", "br_on_cast_fail", "cast", "extern", "i31",
        "ref_cast", "ref_eq", "ref_test", "struct", "type-canon", "type-equivalence", "type-rec", "type-subtyping",
    ];
    if gc.contains(&name) {
        return "gc";
    }
    let fref = ["call_ref", "return_call_ref", "ref_as_non_null", "br_on_null", "br_on_non_null", "local_init"];
    if fref.contains(&name) {
        return "function-references";
    }
    let eh = ["tag", "throw", "throw_ref", "try_table"];
    if eh.contains(&name) {
        return "exceptions";
    }
    if name == "return_call" || name == "return_call_indirect" {
        return "tail-call";
    }
    if name == "switch" {
        return "stack-switching";
    }
    "core"
}

fn wast_files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(base: &Path, d: &Path, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.path());
        for e in entries {
            let p = e.path();
            if p.is_dir() {
                walk(base, &p, out);
            } else if p.extension().is_some_and(|x| x == "wast") {
                out.push(p.strip_prefix(base).unwrap().to_string_lossy().into_owned());
            }
        }
    }
    walk(dir, dir, &mut out);
    out
}

fn run_file(dir: &Path, rel: &str) -> Stats {
    let text = match std::fs::read(dir.join(rel)) {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(_) => return Stats::default(),
    };
    run_script(&text)
}

/// Files of the fast default subset (all must pass completely).
const QUICK: &[&str] = &[
    "address.wast",
    "align.wast",
    "binary-leb128.wast",
    "block.wast",
    "br.wast",
    "br_if.wast",
    "br_table.wast",
    "bulk.wast",
    "call.wast",
    "call_indirect.wast",
    "comments.wast",
    "const.wast",
    "conversions.wast",
    "custom.wast",
    "data.wast",
    "elem.wast",
    "endianness.wast",
    "exports.wast",
    "f32.wast",
    "f64.wast",
    "f32_bitwise.wast",
    "f64_cmp.wast",
    "fac.wast",
    "float_exprs.wast",
    "float_literals.wast",
    "float_memory.wast",
    "float_misc.wast",
    "forward.wast",
    "func.wast",
    "func_ptrs.wast",
    "global.wast",
    "i32.wast",
    "i64.wast",
    "if.wast",
    "imports.wast",
    "int_exprs.wast",
    "int_literals.wast",
    "labels.wast",
    "left-to-right.wast",
    "linking.wast",
    "load.wast",
    "local_get.wast",
    "local_set.wast",
    "local_tee.wast",
    "loop.wast",
    "memory.wast",
    "memory_copy.wast",
    "memory_fill.wast",
    "memory_grow.wast",
    "memory_init.wast",
    "memory_size.wast",
    "memory_trap.wast",
    "names.wast",
    "nop.wast",
    "ref_func.wast",
    "ref_is_null.wast",
    "ref_null.wast",
    "return.wast",
    "select.wast",
    "stack.wast",
    "start.wast",
    "store.wast",
    "switch.wast",
    "table.wast",
    "table_copy.wast",
    "table_fill.wast",
    "table_get.wast",
    "table_grow.wast",
    "table_set.wast",
    "table_size.wast",
    "token.wast",
    "traps.wast",
    "type.wast",
    "unreachable.wast",
    "unreached-invalid.wast",
    "unreached-valid.wast",
    "unwind.wast",
    "utf8-custom-section-id.wast",
    "utf8-import-field.wast",
    "utf8-import-module.wast",
    "utf8-invalid-encoding.wast",
];

#[test]
fn spec_quick() {
    let Some(dir) = suite_dir() else {
        eprintln!("wasm testsuite not found; skipping");
        return;
    };
    let only: Vec<String> = std::env::var("RH_WAST_QUICK").map(|s| s.split(',').map(String::from).collect()).unwrap_or_default();
    let mut bad = Vec::new();
    for f in QUICK {
        if !only.is_empty() && !only.iter().any(|o| f.contains(o.as_str())) {
            continue;
        }
        let st = run_file(&dir, f);
        if st.failed > 0 {
            bad.push(format!("{f}: {}/{} passed; first failures: {:?}", st.passed, st.total(), &st.failures[..st.failures.len().min(3)]));
        }
    }
    assert!(bad.is_empty(), "spec failures:\n{}", bad.join("\n"));
}

#[test]
#[ignore]
fn spec_full() {
    let Some(dir) = suite_dir() else {
        eprintln!("wasm testsuite not found; skipping");
        return;
    };
    let filter = std::env::var("RH_WAST_FILTER").ok();
    let verbose = std::env::var("RH_WAST_VERBOSE").is_ok();
    let mut per_cat: BTreeMap<&str, Stats> = BTreeMap::new();
    let mut lines = Vec::new();
    for rel in wast_files(&dir) {
        if let Some(f) = &filter {
            if !f.split(',').any(|f| rel.contains(f)) {
                continue;
            }
        }
        let t0 = std::time::Instant::now();
        let st = run_file(&dir, &rel);
        let cat = category(&rel);
        let pct = if st.total() > 0 { 100.0 * st.passed as f64 / st.total() as f64 } else { 100.0 };
        let line = format!("{rel:<45} {cat:<22} {:>6}/{:<6} {pct:6.2}%  ({:.0?})", st.passed, st.total(), t0.elapsed());
        eprintln!("{line}");
        if verbose {
            for f in st.failures.iter().take(10) {
                let f: String = f.chars().take(400).collect();
                eprintln!("    {f}");
            }
        }
        lines.push(line);
        per_cat.entry(cat).or_default().add(&st);
    }
    let mut summary = vec![String::new(), "Per proposal:".to_string()];
    let mut total = Stats::default();
    for (cat, st) in &per_cat {
        let pct = if st.total() > 0 { 100.0 * st.passed as f64 / st.total() as f64 } else { 100.0 };
        summary.push(format!("  {cat:<24} {:>7}/{:<7} {pct:6.2}%", st.passed, st.total()));
        total.add(st);
    }
    let pct = 100.0 * total.passed as f64 / total.total().max(1) as f64;
    summary.push(format!("  {:<24} {:>7}/{:<7} {pct:6.2}%", "TOTAL", total.passed, total.total()));
    for s in &summary {
        eprintln!("{s}");
    }
    if let Ok(out) = std::env::var("RH_WAST_OUT") {
        let mut text = String::from("# WebAssembly spec testsuite results (rabbithole-wasm)\n# file  category  passed/total  percent  (time)\n");
        for l in &lines {
            text.push_str(l);
            text.push('\n');
        }
        for s in &summary {
            text.push_str(s);
            text.push('\n');
        }
        std::fs::write(out, text).unwrap();
    }
}
