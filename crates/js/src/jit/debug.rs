//! Debugging aid (`RABBITHOLE_JIT_DEBUG=1`, Linux x86-64): on SIGSEGV,
//! SIGILL or SIGTRAP, report the faulting instruction, the address and
//! the frame-pointer chain, then exit (there is no debugger here; native
//! code keeps frame pointers, and dumps give each function's address).

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub(crate) fn install_fault_reporter() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    if std::env::var_os("RABBITHOLE_JIT_DEBUG").is_none() {
        return;
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
        fn _exit(code: i32) -> !;
    }
    extern "C" fn on_fault(sig: i32, info: *mut u8, ctx: *mut u8) {
        // SAFETY: the kernel passes valid siginfo_t / ucontext_t.
        unsafe {
            let greg = |n: usize| *(ctx.add(40 + n * 8) as *const u64);
            let addr = *(info.add(16) as *const u64);
            let (rip, mut rbp, rsp) = (greg(16), greg(10), greg(15));
            eprintln!("[jit] signal {sig} at rip={rip:#x} addr={addr:#x} rsp={rsp:#x} rbx={:#x} r12={:#x}", greg(11), greg(4));
            let mut n = 0;
            while n < 12 && rbp >= rsp && rbp < rsp + (8 << 20) && rbp % 8 == 0 {
                let ret = *((rbp + 8) as *const u64);
                eprintln!("[jit]   frame ret={ret:#x}");
                let next = *(rbp as *const u64);
                if next <= rbp {
                    break;
                }
                rbp = next;
                n += 1;
            }
            _exit(139);
        }
    }
    ONCE.call_once(|| {
        let act = SigAction { handler: on_fault as *const () as usize, mask: [0; 16], flags: 4, restorer: 0 };
        // SAFETY: installs a reporting handler for fatal signals.
        unsafe {
            for sig in [4, 5, 11] {
                sigaction(sig, &act, std::ptr::null_mut());
            }
        }
    });
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
pub(crate) fn install_fault_reporter() {}
