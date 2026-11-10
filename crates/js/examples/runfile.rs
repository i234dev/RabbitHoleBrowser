//! Time parsing, compiling and running a JS file: `runfile <path>`.
//! Reports wall time and (Linux) thread CPU time.
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
fn cpu_ms() -> f64 {
    #[repr(C)]
    struct Timespec {
        sec: i64,
        nsec: i64,
    }
    extern "C" {
        fn clock_gettime(clk: i32, tp: *mut Timespec) -> i32;
    }
    let mut t = Timespec { sec: 0, nsec: 0 };
    unsafe { clock_gettime(3, &mut t) };
    t.sec as f64 * 1000.0 + t.nsec as f64 / 1e6
}

#[cfg(not(target_os = "linux"))]
fn cpu_ms() -> f64 {
    0.0
}

fn main() {
    let path = std::env::args().nth(1).expect("path");
    let src = std::fs::read_to_string(&path).unwrap();
    let t = Instant::now();
    let c0 = cpu_ms();
    let p = rabbithole_js::parser::parse_program(&src, false);
    eprintln!("parse: {:?} (cpu {:.1} ms) ok={}", t.elapsed(), cpu_ms() - c0, p.is_ok());
    let mut realm = rabbithole_js::Realm::new();
    let t = Instant::now();
    let c0 = cpu_ms();
    let c = realm.compile(&src, &path, false);
    eprintln!("parse+compile: {:?} (cpu {:.1} ms) ok={}", t.elapsed(), cpu_ms() - c0, c.is_ok());
    realm.set_deadline(Some(Instant::now() + Duration::from_secs(3)));
    let t = Instant::now();
    let c0 = cpu_ms();
    let r = realm.eval(&src, &path);
    eprintln!("eval (parse+compile+run): {:?} (cpu {:.1} ms) ok={}", t.elapsed(), cpu_ms() - c0, r.is_ok());
    let t = Instant::now();
    realm.run_jobs();
    eprintln!("jobs: {:?}", t.elapsed());
}
