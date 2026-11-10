//! Time parsing, compiling and running a JS file: `runfile <path>`.
use std::time::{Duration, Instant};
fn main() {
    let path = std::env::args().nth(1).expect("path");
    let src = std::fs::read_to_string(&path).unwrap();
    let t = Instant::now();
    let p = rabbithole_js::parser::parse_program(&src, false);
    eprintln!("parse: {:?} ok={}", t.elapsed(), p.is_ok());
    let mut realm = rabbithole_js::Realm::new();
    let t = Instant::now();
    let c = realm.compile(&src, &path, false);
    eprintln!("compile: {:?} ok={}", t.elapsed(), c.is_ok());
    realm.set_deadline(Some(Instant::now() + Duration::from_secs(3)));
    let t = Instant::now();
    let r = realm.eval(&src, &path);
    eprintln!("eval: {:?} ok={}", t.elapsed(), r.is_ok());
    let t = Instant::now();
    realm.run_jobs();
    eprintln!("jobs: {:?}", t.elapsed());
}
