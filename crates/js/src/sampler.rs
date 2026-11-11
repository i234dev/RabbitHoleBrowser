//! Built-in sampling profiler for machines without `perf` (Linux x86-64).
//!
//! When `RH_PROF=<file>` is set, the first realm starts a SIGPROF interval
//! timer; each sample records the interrupted instruction pointer and (in
//! builds with `-Cforce-frame-pointers=yes`) the return addresses of the
//! frame-pointer chain. At process exit the samples are written to the file
//! (one line per sample), preceded by the runtime address of
//! `rh_sampler_marker` and followed by a copy of `/proc/self/maps`, so an
//! offline script can symbolize them against an unstripped binary.

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod imp {
    use std::sync::atomic::{AtomicUsize, Ordering};

    const DEPTH: usize = 16;
    const MAX: usize = 1 << 16;
    static mut SAMPLES: [[u64; DEPTH]; MAX] = [[0; DEPTH]; MAX];
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    static STARTED: std::sync::Once = std::sync::Once::new();

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
        fn atexit(cb: extern "C" fn()) -> i32;
    }

    #[inline(never)]
    #[no_mangle]
    pub extern "C" fn rh_sampler_marker() {}

    extern "C" fn on_prof(_sig: i32, _info: *mut u8, ctx: *mut u8) {
        // ucontext_t.uc_mcontext.gregs[REG_RIP / REG_RBP / REG_RSP]
        // SAFETY: the kernel passes a valid ucontext_t.
        let greg = |n: usize| unsafe { *(ctx.add(40 + n * 8) as *const u64) };
        let (rip, mut rbp, rsp) = (greg(16), greg(10), greg(15));
        let i = COUNT.fetch_add(1, Ordering::Relaxed);
        if i >= MAX {
            return;
        }
        let mut chain = [0u64; DEPTH];
        chain[0] = rip;
        let mut n = 1;
        // Follow the frame-pointer chain while it stays on this stack.
        while n < DEPTH && rbp >= rsp && rbp < rsp + (8 << 20) && rbp % 8 == 0 {
            // SAFETY: bounded to the interrupted thread's stack.
            let ret = unsafe { *((rbp + 8) as *const u64) };
            let next = unsafe { *(rbp as *const u64) };
            if ret == 0 {
                break;
            }
            chain[n] = ret;
            n += 1;
            if next <= rbp {
                break;
            }
            rbp = next;
        }
        // SAFETY: each index is written once (atomic counter).
        unsafe { SAMPLES[i] = chain };
    }

    extern "C" fn dump() {
        let Some(path) = std::env::var_os("RH_PROF") else { return };
        let t = Itimerval { interval: Timeval { sec: 0, usec: 0 }, value: Timeval { sec: 0, usec: 0 } };
        // SAFETY: plain libc calls.
        unsafe { setitimer(2, &t, std::ptr::null_mut()) };
        let n = COUNT.load(Ordering::Relaxed).min(MAX);
        let mut out = format!("marker {:x}\n", rh_sampler_marker as *const () as usize);
        for i in 0..n {
            // SAFETY: the timer is stopped.
            let s = unsafe { SAMPLES[i] };
            let line: Vec<String> = s.iter().take_while(|a| **a != 0).map(|a| format!("{:x}", a)).collect();
            out.push_str(&line.join(" "));
            out.push('\n');
        }
        let _ = std::fs::write(&path, out);
        if let Ok(maps) = std::fs::read_to_string("/proc/self/maps") {
            let mut p = std::path::PathBuf::from(&path);
            p.set_extension("maps");
            let _ = std::fs::write(p, maps);
        }
    }

    pub fn start_from_env() {
        if std::env::var_os("RH_PROF").is_none() {
            return;
        }
        STARTED.call_once(|| {
            let act = SigAction { handler: on_prof as *const () as usize, mask: [0; 16], flags: 4 | 0x1000_0000, restorer: 0 };
            // SAFETY: installs a handler that only writes to static storage.
            unsafe {
                sigaction(27, &act, std::ptr::null_mut());
                let t = Itimerval { interval: Timeval { sec: 0, usec: 500 }, value: Timeval { sec: 0, usec: 500 } };
                setitimer(2, &t, std::ptr::null_mut());
                atexit(dump);
            }
        });
    }
}

/// Start sampling when `RH_PROF` is set (see the module docs).
pub fn start_from_env() {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    imp::start_from_env();
}

/// JavaScript-level profile: with `RH_JSPROF=<file>`, every VM safe point
/// (each tick of backward jumps and calls) counts the running function and
/// its callers; the table is rewritten to the file every few thousand
/// samples (`self  inclusive  function@file:line`).
pub(crate) mod js {
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        static PATH: Option<String> = std::env::var("RH_JSPROF").ok();
        static TABLE: RefCell<(u64, HashMap<String, (u64, u64)>)> = RefCell::new((0, HashMap::new()));
    }

    pub(crate) fn enabled() -> bool {
        PATH.with(|p| p.is_some())
    }

    /// Record one sample: `frames` innermost first.
    pub(crate) fn sample(frames: &[String]) {
        TABLE.with(|t| {
            let mut t = t.borrow_mut();
            t.0 += 1;
            let mut seen = std::collections::HashSet::new();
            for (i, f) in frames.iter().enumerate() {
                if !seen.insert(f.as_str()) {
                    continue;
                }
                let e = t.1.entry(f.clone()).or_insert((0, 0));
                if i == 0 {
                    e.0 += 1;
                }
                e.1 += 1;
            }
            if t.0 % 2048 == 0 {
                let mut rows: Vec<(&String, &(u64, u64))> = t.1.iter().collect();
                rows.sort_by(|a, b| b.1 .1.cmp(&a.1 .1));
                let mut out = format!("samples {}\n", t.0);
                for (k, (s, c)) in rows.iter().take(300) {
                    out.push_str(&format!("{s:8} {c:8}  {k}\n"));
                }
                PATH.with(|p| {
                    if let Some(p) = p {
                        let _ = std::fs::write(p, out);
                    }
                });
            }
        });
    }
}
