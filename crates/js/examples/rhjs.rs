//! rhjs: run JavaScript files with the engine alone (no DOM, no browser), for
//! benchmarks: `cargo run --release -p rabbithole-js --features jit --example rhjs -- file.js`.
//! Files are evaluated in order in one realm, as classic scripts, then
//! the job queue is drained. `print(...)` and `console.log(...)` write to stdout.
//! Exit code 1 on an uncaught error or an unhandled rejection.

// The browser binary uses the engine's allocator; so does the shell.
#[global_allocator]
static ALLOC: rabbithole_js::alloc::RhAlloc = rabbithole_js::alloc::RhAlloc;

use rabbithole_js::{JsResult, Realm, Value};

fn print(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let mut out = String::new();
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&realm.to_string(a.clone())?.to_rust_string());
    }
    println!("{}", out);
    Ok(Value::Undefined)
}

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("usage: rhjs file.js [file.js...]");
        std::process::exit(2);
    }
    let mut realm = Realm::new();
    realm.set_console_sink(Box::new(|_level, text| println!("{}", text)));
    let g = realm.global();
    realm.define_method(g, "print", 1, print);

    for path in &files {
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("rhjs: {}: {}", path, e);
                std::process::exit(2);
            }
        };
        if let Err(e) = realm.eval(&src, path) {
            let msg = realm.error_to_string(&e);
            eprintln!("rhjs: uncaught: {}", msg);
            std::process::exit(1);
        }
        realm.run_jobs();
    }
    let rejections = realm.take_unhandled_rejections();
    if let Some(r) = rejections.first() {
        let msg = realm.error_to_string(r);
        eprintln!("rhjs: unhandled rejection: {}", msg);
        std::process::exit(1);
    }
}
