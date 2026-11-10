//! Minimal sampling profiler for the engine (Linux x86-64 only), for
//! machines without `perf`: `prof <parse|compile|run> <file.js | -e src> [reps]`.
//!
//! A SIGPROF interval timer records the interrupted instruction pointer;
//! raw addresses go to `prof.out` together with the runtime address of a
//! marker function, so an offline script can symbolize them against an
//! unstripped build (`CARGO_PROFILE_RELEASE_STRIP=false`).

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod sampler {
    use std::sync::atomic::{AtomicUsize, Ordering};

    const MAX: usize = 1 << 20;
    static mut SAMPLES: [u64; MAX] = [0; MAX];
    static COUNT: AtomicUsize = AtomicUsize::new(0);

    #[repr(C)]
    struct Timeval {
        sec: i64,
        usec: i64,
    }
    #[repr(C)]
    struct Itimerval {
        interval: Timeval,
        value: Timeval,
    }
    #[repr(C)]
    struct SigAction {
        handler: usize,
        mask: [u64; 16],
        flags: i32,
        restorer: usize,
    }
    extern "C" {
        fn sigaction(sig: i32, act: *const SigAction, old: *mut SigAction) -> i32;
        fn setitimer(which: i32, new: *const Itimerval, old: *mut Itimerval) -> i32;
    }

    extern "C" fn on_prof(_sig: i32, _info: *mut u8, ctx: *mut u8) {
        // ucontext_t.uc_mcontext.gregs[REG_RIP]
        let rip = unsafe { *(ctx.add(40 + 16 * 8) as *const u64) };
        let i = COUNT.fetch_add(1, Ordering::Relaxed);
        if i < MAX {
            unsafe { SAMPLES[i] = rip };
        }
    }

    pub fn start(usec: i64) {
        let act = SigAction { handler: on_prof as usize, mask: [0; 16], flags: 4 | 0x1000_0000, restorer: 0 };
        unsafe {
            sigaction(27, &act, std::ptr::null_mut());
            let t = Itimerval { interval: Timeval { sec: 0, usec }, value: Timeval { sec: 0, usec } };
            setitimer(2, &t, std::ptr::null_mut());
        }
    }

    pub fn stop() -> Vec<u64> {
        unsafe {
            let t = Itimerval { interval: Timeval { sec: 0, usec: 0 }, value: Timeval { sec: 0, usec: 0 } };
            setitimer(2, &t, std::ptr::null_mut());
            let n = COUNT.load(Ordering::Relaxed).min(MAX);
            (0..n).map(|i| SAMPLES[i]).collect()
        }
    }
}

#[inline(never)]
#[no_mangle]
pub extern "C" fn rh_prof_marker() {}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let phase = args[1].clone();
    let src = if args[2] == "-e" { args[3].clone() } else { std::fs::read_to_string(&args[2]).unwrap() };
    let reps: usize = args.get(if args[2] == "-e" { 4 } else { 3 }).and_then(|s| s.parse().ok()).unwrap_or(1);
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    sampler::start(250);
    let t = std::time::Instant::now();
    for _ in 0..reps {
        match phase.as_str() {
            "parse" => {
                let _ = rabbithole_js::parser::parse_program(&src, false);
            }
            "compile" => {
                let mut realm = rabbithole_js::Realm::new();
                let _ = realm.compile(&src, "prof.js", false);
            }
            _ => {
                let mut realm = rabbithole_js::Realm::new();
                let r = realm.eval(&src, "prof.js");
                if let Err(e) = r {
                    eprintln!("error: {}", realm.error_to_string(&e));
                }
            }
        }
    }
    eprintln!("{} x{}: {:?}", phase, reps, t.elapsed());
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        let samples = sampler::stop();
        let mut out = format!("marker {:x}\n", rh_prof_marker as usize);
        for s in samples {
            out.push_str(&format!("{:x}\n", s));
        }
        std::fs::write("prof.out", out).unwrap();
        if let Ok(maps) = std::fs::read_to_string("/proc/self/maps") {
            std::fs::write("prof.maps", maps).unwrap();
        }
    }
}
